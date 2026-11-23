//! Combine Lines as one atomic undo/redo unit: the inverse of Split Line
//! Here (`command_split_line.rs`), and built the same way, on the station
//! graph's `combine_lines` (plan 5.4; `docs/survey-model.md`, "Line and shot
//! operations"). The model decides the join, which side moves, the ties it
//! tears, the closures it removes across one shot (#746) or replaces (#749)
//! and the station renames; this layer adds the 0.6.0 command contract: the
//! edit gates, one history entry with 0.6.0's label, the empty-name refusal,
//! and the warnings naming the removed closures as the dialog listed them.

use super::commands::Record;
use super::labels::combine_lines_label;
use super::AppState;
use crate::core::{CommandResult, ValidationWarning, WarningSeverity};
use crate::models::survey::changeset::{HistoryRefs, PreviewRow, RecordedPreview};
use crate::models::survey::Project;
use crate::models::{
    CombineCandidate, CombineLinesOptions, CombinePlan, CombineRemovedClosure, CombineRemovedKind,
    LineId, StationRef, CLOSURE_REMOVED_ACROSS_ONE_SHOT,
};

/// 0.6.0's refusal of an empty combined name (a rename's).
const EMPTY_NAME: &str = "A line name cannot be empty";

/// A station as the user reads it on its slot, `Line name / station name`
/// (the name that line shows it by, I-3), `a station of Line name` when the
/// slot names no station, `a station` when its line is gone too. Never the
/// positional `line:index` handle.
fn station_text(project: &Project, at: StationRef) -> String {
    if project.line(at.line_id).is_none() {
        return "a station".to_string();
    }
    let line = project.line_label_of(at.line_id);
    project
        .topology()
        .station_at(at)
        .and_then(|id| project.shown_name(id))
        .map_or_else(
            || format!("a station of {line}"),
            |name| {
                if name.trim().is_empty() {
                    format!("{line} / {}", at.station_index)
                } else {
                    // A dotted slot or a loop end qualified (RS-FX5).
                    project
                        .slot_text(at)
                        .unwrap_or_else(|| format!("{line} / {}", name.trim()))
                }
            },
        )
}

/// The warnings a combine reports for the ties it removed because, combined,
/// the loop closure each is or would become joins the two ends of one shot
/// (#746). Named as the Combine dialog listed them, by the stations they had
/// BEFORE the combine (`before` is that state; the plan's slots address it):
/// the other line is gone afterwards, the kept one may be renamed and a
/// combine may letter both lines' station names, so a station named after
/// the combine would name one the user cannot find. The frontend shows them
/// in one toast with the other removed-closure warnings
/// (`notifyRemovedClosures`).
pub(super) fn combine_removed_warnings(
    before: &Project,
    removed: &[CombineRemovedClosure],
) -> Vec<ValidationWarning> {
    removed
        .iter()
        .map(|removed| {
            let (a, b) = (
                station_text(before, removed.station_a),
                station_text(before, removed.station_b),
            );
            let what = match removed.kind {
                CombineRemovedKind::Merge => format!("the merge of {a} and {b}"),
                CombineRemovedKind::SnappedClosure => {
                    format!("the snapped closure between {a} and {b}")
                }
                CombineRemovedKind::FormerSnappedClosure => {
                    format!("loop closure {a} to {b}, left from a snap whose stations had moved apart")
                }
                CombineRemovedKind::LoopClosure => format!("loop closure {a} to {b}"),
            };
            ValidationWarning {
                code: CLOSURE_REMOVED_ACROSS_ONE_SHOT.to_string(),
                message: format!(
                    "Removed {what}: combining the lines left its two stations at the two ends of one shot."
                ),
                severity: WarningSeverity::Warning,
            }
        })
        .collect()
}

/// Log what the combine did to the closures, as 0.6.0 did: the ties removed
/// across one shot (each also a warning), and the closures replaced by one
/// the Closures Computed View adjusts on the same two stations (#749: the
/// dialog listed it and the connection stays, so no toast).
fn log_closure_changes(plan: &CombinePlan) {
    for tie in plan.closures_removed_across_one_shot.iter().flatten() {
        tracing::info!(
            station_a = ?tie.station_a,
            station_b = ?tie.station_b,
            kind = ?tie.kind,
            purpose = "drop a tie a combine left across one shot",
            "Combine removed a loop closure that would span a single shot"
        );
    }
    for replaced in plan.closures_replaced.iter().flatten() {
        tracing::info!(
            station_a = ?replaced.station_a,
            station_b = ?replaced.station_b,
            kind = ?replaced.kind,
            by_station_a = ?replaced.by_station_a,
            by_station_b = ?replaced.by_station_b,
            by_kind = ?replaced.by_kind,
            purpose = "keep the closure the solve takes when a combine makes two identical",
            "Combine replaced a loop closure with one on the same two stations"
        );
    }
}

/// The history preview rows 0.6.0 showed for a combine: the kept line by
/// the name it has now (`after`), the other by the name it had (`before`).
fn combine_rows(before: &Project, after: &Project, kept: LineId, other: LineId) -> RecordedPreview {
    let update = if after.line(kept).is_some() {
        format!("Update line \"{}\" metadata", after.line_label_of(kept))
    } else {
        "Update line metadata".to_string()
    };
    let delete = before
        .line(other)
        .and_then(|l| l.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(
            || "Delete line".to_string(),
            |name| format!("Delete line \"{name}\""),
        );
    RecordedPreview {
        rows: [update, delete]
            .into_iter()
            .map(|field| PreviewRow {
                field,
                ..PreviewRow::default()
            })
            .collect(),
        more: 0,
    }
}

impl AppState {
    /// Read-only: what can be combined at the point `station` ends at: the
    /// clicked line with each other line ending there first, then (at a
    /// junction) every pair of the others, each with its own reason when
    /// blocked (`Project::combine_candidates_at_point`), with both plans.
    /// Empty when `station` is not a line end or nothing else touches it.
    /// Answered under the project lock without cloning the project: the
    /// context menu asks on every right-click of an end station.
    pub fn combine_candidates(&self, station: StationRef) -> Result<Vec<CombineCandidate>, String> {
        self.with_project(|project| {
            Ok(project
                .combine_candidates_at_point(station)
                .iter()
                .map(|candidate| project.combine_candidate_wire(candidate))
                .collect())
        })
    }

    /// Read-only: can lines `a` and `b` be combined (the Sections panel's
    /// two-line selection)? `None` when either is missing or they are the
    /// same line.
    pub fn combine_candidate_for_lines(
        &self,
        a: LineId,
        b: LineId,
    ) -> Result<Option<CombineCandidate>, String> {
        self.with_project(|project| {
            Ok(project
                .combine_candidate_for_lines(a, b)
                .map(|candidate| project.combine_candidate_wire(&candidate)))
        })
    }

    /// Combine `other` into `kept` (see `Project::combine_lines`). One undo
    /// entry covers the optional reverse of `other`, the combine and the
    /// closures across one shot the combine removes (#746), so one Undo
    /// restores them too. A refusal (the candidate's blocked reason, an empty
    /// or taken name) changes nothing and pushes no undo entry. Returns
    /// `kept` as `line_id`, and a `CLOSURE_REMOVED_ACROSS_ONE_SHOT` warning
    /// per removed tie, named by its stations before the combine.
    pub fn execute_combine_lines(
        &self,
        kept: LineId,
        other: LineId,
        options: CombineLinesOptions,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6).
        if let Err(f) = self.guard_mutation("Combine lines") {
            return f;
        }
        tracing::info!(kept, other, "Executing combine_lines as single undo unit");
        let result = self.run_recorded_from(
            |project| {
                let mut lines: Vec<LineId> = [kept, other]
                    .into_iter()
                    .filter(|&id| project.line(id).is_some())
                    .collect();
                lines.sort_unstable();
                Record {
                    kind: "batch".to_string(),
                    // Both lines by the names they have before the combine
                    // (the other one is gone after it, and the kept one may
                    // be renamed).
                    label: combine_lines_label(project, kept, other, &options.name),
                    refs: HistoryRefs {
                        lines,
                        ..HistoryRefs::default()
                    },
                    preview: RecordedPreview::default(),
                }
            },
            |project, record, before| {
                // The combined line's name replaces both lines' names: refused
                // like a rename when empty (the model validates a non-empty one
                // itself, after the join's refusals, as 0.6.0 did).
                if options.name.trim().is_empty() {
                    return Err(project
                        .combine_refusal(kept, other, options.join)
                        .unwrap_or_else(|| EMPTY_NAME.to_string()));
                }
                // The model's combine is all or nothing; the pipeline's
                // snapshot is the state the dialog showed.
                let edited = project
                    .combine_lines(kept, other, &options)
                    .map_err(|e| e.to_string())?;
                // Named in the state the dialog showed: before the combine.
                let plan = before.combine_plan_wire(&edited.value);
                log_closure_changes(&plan);
                let warnings = combine_removed_warnings(
                    before,
                    plan.closures_removed_across_one_shot
                        .as_deref()
                        .unwrap_or_default(),
                );
                record.preview = combine_rows(before, project, kept, other);
                Ok((warnings, project.identity_notice(&edited.report.identities)))
            },
        );
        match result {
            Ok((warnings, identities)) => {
                tracing::info!(kept, other, "Combine lines completed");
                CommandResult::success()
                    .with_line_id(kept)
                    .with_line_ids(vec![kept])
                    .with_warnings(warnings)
                    .with_identities(identities)
            }
            Err(failure) => failure,
        }
    }
}
