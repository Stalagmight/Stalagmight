//! The line commands on the station graph: Create Line, Delete Line, Update
//! Line Metadata and Reverse Line (plan 5.4, 9.1).
//!
//! Each runs inside the execute pipeline (`commands.rs`): `apply` mutates
//! the survey through the model's line operations, which refuse in 0.6.0's
//! words and settle anchors themselves; the pipeline validates, captures and
//! records around it. The result fields are 0.6.0's: a created line's id and
//! its shots' validation warnings, the closures a delete's sweep removed, a
//! reverse's line and re-emitted warnings.

use std::collections::{BTreeMap, HashSet};

use super::commands::{Applied, Record};
use super::AppState;
use crate::core::{
    Command, CommandResult, ReversedLine, ValidationWarning, Validator, WarningSeverity,
};
use crate::models::station_naming::{dedupe_line_name, seed_line_name_from};
use crate::models::survey::changeset::EntryLimits;
use crate::models::survey::{ClosureId, LineMeta, Project, StartAt};
use crate::models::{
    LineId, SectionId, ShotData, SurveyDirection, CLOSURE_REMOVED_MERGED_ENDS, LINE_GONE,
};

/// What a new line is called in its shots' labels when it has no name yet
/// (0.6.0's `create_line`).
const NEW_LINE_LABEL: &str = "New line";

/// The refusal for a Section id the project does not hold (0.6.0's words,
/// shared by the Section commands).
pub(super) fn missing_section(id: SectionId) -> String {
    format!("Section {id} not found; it may have been deleted")
}

/// Add `id` to a record's refs once, within what a persisted entry may hold
/// (`EntryLimits::max_refs`), so a huge batch can never write an entry the
/// next open refuses.
pub(super) fn push_ref<T: PartialEq>(refs: &mut Vec<T>, id: T) {
    if refs.len() < EntryLimits::default().max_refs && !refs.contains(&id) {
        refs.push(id);
    }
}

/// The name a line created by a command gets: 0.6.0's `ensure_line_names`
/// applied to it (trimmed; none or blank seeds the smallest free `Line n`; a
/// name another line holds becomes `name (k)`).
fn settled_line_name(project: &Project, wanted: Option<&str>) -> String {
    let taken: HashSet<&str> = project
        .lines()
        .iter()
        .filter_map(|l| l.name.as_deref().map(str::trim))
        .collect();
    let is_taken = |n: &str| taken.contains(n);
    match wanted.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) if !is_taken(name) => name.to_string(),
        Some(name) => dedupe_line_name(name, &is_taken),
        None => seed_line_name_from(&mut 1, &is_taken),
    }
}

impl AppState {
    /// Line names are required and unique in the project, so a line the USER
    /// creates (the `create_line` IPC commands) must be named, and not after
    /// another line ("A line named X already exists"). Asked on the UI path
    /// only: the automatic paths (a batch, imports, split and combine
    /// defaults) are named and made unique by the command itself. `None`
    /// when the name is fine or no project is open (the command then fails
    /// on its own).
    pub fn new_line_name_refusal(&self, name: Option<&str>) -> Option<CommandResult> {
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = guard.as_ref()?;
        project
            .validate_line_name(name.unwrap_or_default(), &[])
            .err()
            .map(|e| CommandResult::failure(e.to_string()))
    }
}

/// A new line as Create Line describes it.
pub(super) struct NewLine {
    pub(super) name: Option<String>,
    pub(super) surveyor: Option<String>,
    pub(super) date: Option<chrono::NaiveDate>,
    pub(super) color: String,
    pub(super) direction: Option<SurveyDirection>,
    pub(super) shots: Vec<ShotData>,
    pub(super) section: Option<SectionId>,
}

/// Create a free line (0.6.0's `create_line_command` and `create_line`):
/// refused when its Section is gone, for too many shots, the line-id budget,
/// a non-finite value or a zero or negative length; the shots' uuids are the
/// app's to mint (#423); the legacy surveyor and date become the shots'
/// credit; the new piece sits with station 0 at the local origin; the line is
/// named and made unique as 0.6.0's normalization named it. Returns the line
/// and the shots' validation warnings (per-project thresholds, #57).
pub(super) fn create_free_line(
    project: &mut Project,
    mut line: NewLine,
) -> Result<(LineId, Vec<ValidationWarning>), String> {
    if let Some(section) = line.section {
        if !project.sections.iter().any(|s| s.id == section) {
            return Err(missing_section(section));
        }
    }
    for shot in &mut line.shots {
        shot.uuid = None;
    }
    let trimmed = line
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    let label_name = trimmed.as_deref().unwrap_or(NEW_LINE_LABEL).to_string();
    let warnings: Vec<ValidationWarning> = line
        .shots
        .iter()
        .enumerate()
        .flat_map(|(i, shot)| {
            Validator::validate_shot(shot, &project.settings.validation, || {
                format!("{label_name}: {i} \u{2192} {}", i + 1)
            })
        })
        .collect();
    let final_name = settled_line_name(project, trimmed.as_deref());
    let meta = LineMeta {
        name: trimmed,
        color: line.color,
        direction: line.direction.unwrap_or_default(),
        quality: 1.0,
        azimuth_reference: None,
        section_id: line.section,
        legacy_surveyor: line.surveyor,
        legacy_date: line.date,
    };
    let id = project
        .create_line(meta, StartAt::New, line.shots, None)
        .map_err(|e| e.to_string())?
        .value;
    // Named after the refusals, which word the shots with the name the user
    // gave (or "New line"), as 0.6.0's normalization ran after the command.
    if let Some(l) = project.line_mut(id) {
        l.name = Some(final_name);
    }
    tracing::debug!(
        line_id = id,
        purpose = "add line to project",
        "Line created"
    );
    Ok((id, warnings))
}

/// A shot as the user reads it after the edit: `Line: from -> to`, each
/// station by the name that line shows it by (I-3).
fn shot_label(project: &Project, line: LineId, index: usize) -> String {
    let name = |i: usize| {
        project
            .line(line)
            .and_then(|l| l.station(i))
            .and_then(|s| project.shown_name(s))
            .map_or_else(|| i.to_string(), str::to_string)
    };
    format!(
        "{}: {} \u{2192} {}",
        project.line_label_of(line),
        name(index),
        name(index + 1)
    )
}

/// Every shot of `line` checked against the project's thresholds, labelled
/// as the user reads it now.
fn line_warnings(project: &Project, line: LineId) -> Vec<ValidationWarning> {
    let Some(l) = project.line(line) else {
        return Vec::new();
    };
    l.shots
        .iter()
        .enumerate()
        .flat_map(|(i, shot)| {
            Validator::validate_shot(&shot.to_shot_data(), &project.settings.validation, || {
                shot_label(project, line, i)
            })
        })
        .collect()
}

/// The closures on the project, each named by its ends as the user reads
/// them now (`Line / station`), for warnings about closures an edit removes.
fn closure_labels(project: &Project) -> BTreeMap<ClosureId, (String, String)> {
    project
        .closures
        .iter()
        .map(|c| {
            (
                c.id,
                (
                    project.station_text(c.a, c.a_via),
                    project.station_text(c.b, c.b_via),
                ),
            )
        })
        .collect()
}

/// The warnings for closures a delete's sweep removed because another
/// closure's promotion made their two ends one station (#738), named by the
/// stations they had before the delete (0.6.0's `merged_ends_warnings`). A
/// closure never has two equal ends before an edit (invariant 7), so the
/// cause is always a promotion.
fn swept_closure_warnings(
    labels: &BTreeMap<ClosureId, (String, String)>,
    removed: &[crate::models::survey::LoopClosure],
) -> Vec<ValidationWarning> {
    removed
        .iter()
        .map(|closure| {
            let (a, b) = labels
                .get(&closure.id)
                .cloned()
                .unwrap_or_else(|| ("a station".to_string(), "a station".to_string()));
            ValidationWarning {
                code: CLOSURE_REMOVED_MERGED_ENDS.to_string(),
                message: format!(
                    "Removed loop closure {a} to {b}: another loop closure became a merge, \
                     which made its two stations one point."
                ),
                severity: WarningSeverity::Warning,
            }
        })
        .collect()
}

/// The execute pipeline's mutation for the line family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit), for refs only the result can say.
pub(super) fn apply(
    project: &mut Project,
    record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    match cmd {
        Command::CreateLine {
            name,
            surveyor,
            date,
            color,
            direction,
            shots,
            cave_section_id,
        } => {
            let (line_id, warnings) = create_free_line(
                project,
                NewLine {
                    name: name.clone(),
                    surveyor: surveyor.clone(),
                    date: *date,
                    color: color.clone(),
                    direction: *direction,
                    shots: shots.clone(),
                    section: *cave_section_id,
                },
            )?;
            push_ref(&mut record.refs.lines, line_id);
            if let Some(section) = cave_section_id {
                push_ref(&mut record.refs.sections, *section);
            }
            Ok(Applied {
                warnings,
                line_id: Some(line_id),
                ..Applied::default()
            })
        }
        Command::DeleteLine { line_id } => {
            if project.line(*line_id).is_none() {
                return Err(LINE_GONE.to_string());
            }
            let labels = closure_labels(project);
            let deleted = project.delete_line(*line_id).map_err(|e| e.to_string())?;
            tracing::debug!(
                line_id,
                stations_deleted = deleted.value.stations_deleted.len(),
                closures_removed = deleted.value.closures_removed.len(),
                promoted = deleted.report.sweep.promoted.len(),
                purpose = "remove line and cleanup references",
                "Line deleted"
            );
            Ok(Applied {
                warnings: swept_closure_warnings(&labels, &deleted.report.sweep.removed),
                identities: project.identity_notice(&deleted.report.identities),
                ..Applied::default()
            })
        }
        Command::UpdateLineMetadata { line_id, updates } => {
            // The rename refusals (empty, forbidden characters, another line's
            // name) are the model's, in 0.6.0's words; a forward command only,
            // since undo and redo never come through here.
            project
                .update_line_meta(*line_id, updates)
                .map_err(|e| e.to_string())?;
            tracing::debug!(
                line_id,
                purpose = "update line metadata or shots",
                "Line updated"
            );
            Ok(Applied::default())
        }
        Command::ReverseLine {
            line_id,
            swap_left_right,
        } => {
            // Frozen shots are deliberately NOT consulted: a reverse
            // re-associates each reading with the other end of the same
            // physical shot and moves no station, so the frozen invariant
            // holds (0.6.0's rule).
            let shot_count = project
                .line(*line_id)
                .map(|l| u32::try_from(l.shots.len()).unwrap_or(u32::MAX))
                .ok_or_else(|| LINE_GONE.to_string())?;
            if shot_count == 0 {
                // A shot-less line reverses to itself: a provable no-op (the
                // empty capture keeps the redo branch), reported as 0.6.0's
                // early success with nothing in it.
                return Ok(Applied::default());
            }
            project
                .reverse_line(*line_id, *swap_left_right)
                .map_err(|e| e.to_string())?;
            tracing::debug!(
                line_id,
                swap_left_right,
                purpose = "reverse line end-to-end",
                "Line reversed"
            );
            // Swapped depths and reciprocated azimuths can newly satisfy or
            // trip the discrepancy checks, so the warnings are re-emitted.
            Ok(Applied {
                warnings: line_warnings(project, *line_id),
                line_id: Some(*line_id),
                reversed: vec![ReversedLine {
                    line_id: *line_id,
                    shot_count,
                }],
                ..Applied::default()
            })
        }
        _ => Err(format!(
            "{} is not a line command",
            crate::core::command_kind(cmd)
        )),
    }
}
