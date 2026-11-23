//! The shot family of the execute pipeline (plan 9.1, 5.4): create, update,
//! delete and set the kind of one shot, on the station-graph model
//! (`survey::edit::shots`). The survey operations settle anchors, run the
//! orphan-closure sweep and remove the closures a delete newly leaves across
//! one shot (#737); this layer adds what is the command's, not the model's:
//! the frozen-shot refusals (S12), the identity rule for a new shot (#423),
//! the create-time validation warnings, and the warnings that name the loop
//! closures an edit removed.
//!
//! The closure warnings are shared with the delete-and-split recipes
//! (`command_delete_split.rs`): a removed closure is named by its stations
//! as the user read them BEFORE the edit ([`ClosureNames`]), because the edit
//! can delete a station or retire it into an alias.

use std::collections::BTreeMap;

use crate::core::{Command, ValidationWarning, Validator, WarningSeverity};
use crate::models::survey::{
    ClosureId, EditReport, LoopClosure, Project, ShotEditRun, ShotPlace, SweepReport,
};
use crate::models::{LineId, CLOSURE_REMOVED_ACROSS_ONE_SHOT, CLOSURE_REMOVED_MERGED_ENDS};

/// The refusal of a delete that would remove a frozen shot (S12).
pub(super) const FROZEN_DELETE: &str =
    "Cannot delete a frozen shot. This shot is part of an adjusted survey network.";

/// [`FROZEN_DELETE`] for a range that holds a frozen shot.
pub(super) const FROZEN_RANGE_DELETE: &str = "Cannot delete a frozen shot. This range includes \
     shots that are part of an adjusted survey network.";

/// Whether any of the shots at `indices` of `line` is frozen: part of an
/// adjusted survey network (Tarjan bridges over shots and aligned closures
/// between two placement datums). The model's operations do not refuse a
/// frozen shot; the command does (S12). Without a datum nothing is frozen, so
/// the raw layout is not even computed.
pub(super) fn any_shot_frozen(
    project: &Project,
    line: LineId,
    indices: impl IntoIterator<Item = usize>,
) -> bool {
    if project.datums.is_empty() {
        return false;
    }
    let aligned = project.closure_alignment(&project.raw_positions());
    let frozen = project.frozen_shots(&aligned);
    if frozen.is_empty() {
        return false;
    }
    indices.into_iter().any(|index| {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        frozen
            .iter()
            .any(|f| f.line_id == line && f.shot_index == index)
    })
}

/// The index of shot `shot_id` on `line_id`, if both exist.
pub(super) fn shot_index(project: &Project, line_id: LineId, shot_id: u32) -> Option<usize> {
    project
        .line(line_id)?
        .shots
        .iter()
        .position(|s| s.id == shot_id)
}

/// A shot as the user reads it: `Main Line: 4 \u{2192} 5` (the line's
/// label, then the stations it runs between, a dotted slot or a loop end
/// qualified: [`Project::shot_text`], RS-FX5).
pub(super) fn shot_label(project: &Project, line: LineId, index: usize) -> String {
    project.shot_text(line, index)
}

/// Every loop closure's two stations as the user read them before an edit
/// (`Main Line / 4`, each end on its `via` line), so the warnings for the
/// closures the edit removes name the stations the user created them
/// between: the edit may delete a station or retire it into an alias, and a
/// name read afterwards would be gone. Empty (and free) on a project without
/// closures.
#[derive(Debug, Default)]
pub(super) struct ClosureNames(BTreeMap<ClosureId, (String, String)>);

impl ClosureNames {
    /// The names of `project`'s closures now.
    pub(super) fn of(project: &Project) -> Self {
        Self(
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
                .collect(),
        )
    }

    /// `closure`'s two names as recorded, else as `after` reads them.
    fn pair(&self, after: &Project, closure: &LoopClosure) -> (String, String) {
        self.0.get(&closure.id).cloned().unwrap_or_else(|| {
            (
                after.station_text(closure.a, closure.a_via),
                after.station_text(closure.b, closure.b_via),
            )
        })
    }
}

/// Why the sweep after an edit's own promotion removed a closure (#738):
/// the end of a [`merged_ends_warnings`] message.
pub(super) const ONE_POINT_BY_PROMOTION: &str =
    "another loop closure became a merge, which made its two stations one point.";

/// The warning for a closure a delete removed because it left the closure
/// across one shot (#737), as 0.6.0 worded it.
pub(super) fn closure_removed_message(
    names: &ClosureNames,
    after: &Project,
    c: &LoopClosure,
) -> String {
    let (a, b) = names.pair(after, c);
    format!(
        "Removed loop closure {a} to {b}: closing the gap left its two stations at the two ends of one shot."
    )
}

/// The warnings for the closures an edit's sweep removed because their two
/// ends became one station (#738), each named as before the edit, ending
/// with `why` (what made them one point, e.g. [`ONE_POINT_BY_PROMOTION`]).
pub(super) fn merged_ends_warnings(
    names: &ClosureNames,
    after: &Project,
    sweep: &SweepReport,
    why: &str,
) -> Vec<ValidationWarning> {
    sweep
        .removed
        .iter()
        .map(|c| {
            let (a, b) = names.pair(after, c);
            ValidationWarning {
                code: CLOSURE_REMOVED_MERGED_ENDS.to_string(),
                message: format!("Removed loop closure {a} to {b}: {why}"),
                severity: WarningSeverity::Warning,
            }
        })
        .collect()
}

/// Log what an applied sweep did (the model logs nothing: a preview runs the
/// same sweep on a copy).
pub(super) fn log_sweep(sweep: &SweepReport, context: &str) {
    if !sweep.promoted.is_empty() {
        tracing::info!(
            promoted = sweep.promoted.len(),
            context,
            purpose = "a loop closure left as the only connection is a merge",
            "Promoted loop closures to merges"
        );
    }
    if !sweep.removed.is_empty() {
        tracing::info!(
            removed = sweep.removed.len(),
            context,
            purpose = "a loop closure between merged stations closes no loop",
            "Removed loop closures whose two stations are one point"
        );
        tracing::debug!(
            closure_ids = ?sweep.removed.iter().map(|c| c.id).collect::<Vec<_>>(),
            context,
            "Loop closures removed for merged ends"
        );
    }
}

/// Every warning an edit reports for the loop closures it removed: first
/// those it left across one shot (#737), then those its sweep removed
/// because their ends became one point (#738), each named as the user read
/// it before the edit. The frontend shows them in one toast
/// (`notifyRemovedClosures`). A promotion itself is logged, not reported
/// (0.6.0).
pub(super) fn closure_warnings(
    names: &ClosureNames,
    after: &Project,
    report: &EditReport,
    context: &str,
) -> Vec<ValidationWarning> {
    log_sweep(&report.sweep, context);
    let mut warnings: Vec<ValidationWarning> = report
        .removed_across_one_shot
        .iter()
        .map(|c| {
            tracing::info!(
                closure_id = c.id,
                context,
                purpose = "drop a closure a delete left across one shot",
                "Removed loop closure that now spans a single shot"
            );
            ValidationWarning {
                code: CLOSURE_REMOVED_ACROSS_ONE_SHOT.to_string(),
                message: closure_removed_message(names, after, c),
                severity: WarningSeverity::Warning,
            }
        })
        .collect();
    warnings.extend(merged_ends_warnings(
        names,
        after,
        &report.sweep,
        ONE_POINT_BY_PROMOTION,
    ));
    warnings
}

/// The create-time validation warnings of the shot at `index` of `line`
/// (per-project thresholds, #57, so they match the panel), naming the shot
/// as the user will read it after the insert.
fn created_shot_warnings(project: &Project, line: LineId, index: usize) -> Vec<ValidationWarning> {
    let Some(shot) = project.line(line).and_then(|l| l.shots.get(index)) else {
        return Vec::new();
    };
    Validator::validate_shot(&shot.to_shot_data(), &project.settings.validation, || {
        shot_label(project, line, index)
    })
}

/// `UpdateShot`s (one, or a batch's run of them: a data-table paste) as
/// one [`ShotEditRun`]: each exactly as its own `update_shot` after the
/// frozen-shot refusal (S12: the model does not refuse frozen shots; the
/// command does), in one survey pass. Any other command in `cmds` is
/// skipped. On `Err` the caller rolls back, and on `Ok` it validates the
/// final state (`run_recorded`).
pub(super) fn apply_updates(project: &mut Project, cmds: &[Command]) -> Result<(), String> {
    let mut run = ShotEditRun::new(project);
    for cmd in cmds {
        let Command::UpdateShot {
            line_id,
            shot_id,
            updates,
        } = cmd
        else {
            continue;
        };
        if updates.touches_geometry() {
            let index = shot_index(run.project(), *line_id, *shot_id);
            if index.is_some_and(|i| run.is_frozen(*line_id, i)) {
                return Err(super::commands::FROZEN_GEOMETRY.to_string());
            }
        }
        run.update_shot(*line_id, *shot_id, updates)
            .map_err(|e| e.to_string())?;
    }
    // `run_recorded` validates the final state before it records it.
    run.end();
    Ok(())
}

/// The execute pipeline's mutation for the shot family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit), for rows only the result can say.
pub(super) fn apply(
    project: &mut Project,
    record: &mut super::commands::Record,
    cmd: &Command,
) -> Result<super::commands::Applied, String> {
    match cmd {
        Command::CreateShot {
            line_id,
            shot,
            after_shot_id,
            before_shot_id,
        } => {
            // Identity is not the caller's to name (#423): a client-chosen
            // uuid could claim an existing shot's place in every reference
            // that names it. Cleared here, for every constructor of the
            // command (the IPC wrapper clears it too); undo and redo never
            // replay this command, they step the recorded changeset.
            let mut data = shot.clone();
            data.uuid = None;
            let place = match (after_shot_id, before_shot_id) {
                (Some(_), Some(_)) => {
                    return Err("Cannot specify both after_shot_id and before_shot_id".to_string())
                }
                (Some(after), None) => ShotPlace::After(*after),
                (None, Some(before)) => ShotPlace::Before(*before),
                (None, None) => ShotPlace::End,
            };
            let created = project
                .create_shot(*line_id, data, place)
                .map_err(|e| e.to_string())?
                .value;
            tracing::debug!(
                line_id,
                shot_id = created.shot,
                purpose = "add shot to line",
                "Shot created"
            );
            record.refs.shots.push(created.uuid.clone());
            record.refs.stations.push(created.station);
            Ok(super::commands::Applied {
                warnings: created_shot_warnings(project, *line_id, created.index),
                shot_id: Some(created.shot),
                ..super::commands::Applied::default()
            })
        }
        Command::UpdateShot { .. } => {
            apply_updates(project, std::slice::from_ref(cmd))?;
            Ok(super::commands::Applied::default())
        }
        Command::DeleteShot { line_id, shot_id } => {
            // Checked before the shift-blocked refusal (the model's), so a
            // frozen shot reports the clear frozen message rather than the
            // misleading "Use split instead." A missing shot falls through
            // to the model's own refusal.
            if shot_index(project, *line_id, *shot_id)
                .is_some_and(|i| any_shot_frozen(project, *line_id, [i]))
            {
                return Err(FROZEN_DELETE.to_string());
            }
            let names = ClosureNames::of(project);
            let edited = project
                .delete_shot(*line_id, *shot_id)
                .map_err(|e| e.to_string())?;
            tracing::debug!(
                line_id,
                shot_id,
                station_deleted = edited.value.station_deleted,
                purpose = "remove shot from its line",
                "Shot deleted"
            );
            record.refs.stations.push(edited.value.station);
            Ok(super::commands::Applied {
                warnings: closure_warnings(&names, project, &edited.report, "delete shot"),
                identities: project.identity_notice(&edited.report.identities),
                ..super::commands::Applied::default()
            })
        }
        Command::SetShotKind {
            line_id,
            shot_id,
            kind,
        } => {
            project
                .set_shot_kind(*line_id, *shot_id, *kind)
                .map_err(|e| e.to_string())?;
            tracing::info!(
                line_id,
                shot_id,
                kind = kind.label(),
                purpose = "set shot kind (permanent/jump/reference)",
                "Shot kind updated"
            );
            Ok(super::commands::Applied::default())
        }
        _ => Err(format!(
            "{} is not a shot command",
            crate::core::command_kind(cmd)
        )),
    }
}
