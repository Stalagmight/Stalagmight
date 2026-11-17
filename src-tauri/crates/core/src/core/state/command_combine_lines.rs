//! Combine Lines as one atomic undo/redo unit: the inverse of Split Line
//! Here (`command_split_line.rs`), and built the same way.

use super::AppState;
use crate::core::{Command, CommandResult, ValidationWarning, WarningSeverity};
use std::collections::HashMap;

use crate::models::{
    line_label, station_id_label, CombineCandidate, CombineLinesOptions, CombineRemovedClosure,
    CombineRemovedKind, LineId, LineUpdates, StationRef, CLOSURE_REMOVED_ACROSS_ONE_SHOT,
};

/// The warnings a combine reports for the ties it removed because, combined,
/// the loop closure each is or would become joins the two ends of one shot
/// (#746). Named as the Combine dialog listed them, by the stations they had
/// BEFORE the combine, each with its line's name then (`labels`, taken
/// before the combine): the other line is gone afterwards, and the kept
/// one may be renamed, so a bare "2:0" would name a line the user cannot
/// find. The frontend shows them in one toast with the other removed-closure
/// warnings (`notifyRemovedClosures`).
pub(super) fn combine_removed_warnings(
    removed: &[CombineRemovedClosure],
    labels: &HashMap<LineId, String>,
) -> Vec<ValidationWarning> {
    let station = |at: StationRef| match labels.get(&at.line_id) {
        Some(name) => format!("{name} {}", station_id_label(at)),
        None => station_id_label(at),
    };
    removed
        .iter()
        .map(|removed| {
            let (a, b) = (station(removed.station_a), station(removed.station_b));
            let what = match removed.kind {
                CombineRemovedKind::Merge => format!("the merge of {a} and {b}"),
                CombineRemovedKind::SnappedClosure => {
                    format!("the snapped closure between {a} and {b}")
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

impl AppState {
    /// Read-only: what can be combined at the point `station` ends at: the
    /// clicked line with each other line ending there first, then (at a
    /// junction) every pair of the others, each with its own reason when
    /// blocked (`Project::combine_candidates_at_point`). Empty when `station`
    /// is not a line end or nothing else touches it. Answered under the project
    /// lock without cloning the project: the context menu asks on every
    /// right-click of an end station.
    pub fn combine_candidates(&self, station: StationRef) -> Result<Vec<CombineCandidate>, String> {
        self.with_project(|project| Ok(project.combine_candidates_at_point(station)))
    }

    /// Read-only: can lines `a` and `b` be combined (the Sections panel's
    /// two-line selection)? `None` when either is missing or they are the
    /// same line.
    pub fn combine_candidate_for_lines(
        &self,
        a: LineId,
        b: LineId,
    ) -> Result<Option<CombineCandidate>, String> {
        self.with_project(|project| Ok(project.combine_candidate_for_lines(a, b)))
    }

    /// Combine `other` into `kept` (see `Project::combine_lines`). One undo
    /// entry (`ProjectSnapshotPair`) covers the optional reverse of `other` and
    /// the combine, and the closures across one shot the combine removes
    /// (#746), so one Undo restores them too. A refusal (the candidate's
    /// blocked reason) changes nothing and pushes no undo entry. Returns
    /// `kept` as `line_id`, and a `CLOSURE_REMOVED_ACROSS_ONE_SHOT` warning
    /// per removed tie.
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

        // Refuse before the snapshot: the check is read-only, so a blocked
        // combine costs no serialization and cannot touch the project.
        {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            if let Some(reason) = project.combine_refusal(kept, other, options.join) {
                return CommandResult::failure(reason);
            }
            // A heal, before the pins and the snapshot below: shots the two
            // lines both claim get their own identity first, so every pin,
            // the held component gauge and the undo step name each shot by the
            // identity it keeps (see `Project::renew_shared_shot_uuids`).
            let renewed = project.renew_shared_shot_uuids(kept, other);
            if renewed > 0 {
                tracing::info!(kept, other, renewed, "Renewed shared shot identities");
            }
        }

        // The other line's shots move into the kept one (reversed, when
        // the ends match), and a prepend renumbers the kept one's stations:
        // position-keyed ignores on either would lapse (see the pin's doc).
        self.pin_position_keyed_ignores_before_move(&[kept, other]);

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // This path bypasses `execute()`, so it carries its own frame check
        // (#469): a zero datum on the other line moves to the kept one,
        // which re-keys the frame-defining station.
        let datums_before = self.datum_layout();
        let label_updates = LineUpdates {
            name: Some(Some(options.name.clone())),
            color: Some(options.color.clone()),
            ..Default::default()
        };
        let warnings;

        {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            // Every line's name as it is now, for the warnings below: they
            // name stations as they were before the combine.
            let labels: HashMap<LineId, String> = project
                .lines
                .iter()
                .map(|s| (s.id, line_label(s)))
                .collect();
            let outcome = match project.combine_lines_reporting(kept, other, &options) {
                Ok(outcome) => outcome,
                Err(e) => {
                    // The model re-checks before it mutates, so this is
                    // reachable only if the project changed under the check
                    // above; put it back regardless.
                    let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                    return CommandResult::failure(e);
                }
            };
            // No orphan-closure sweep (the split runs one): a combine joins two
            // lines and never strands a line on a lone closure. The
            // closures it would leave across one shot it removes itself, from
            // the plan the dialog listed them from (#746).
            for tie in &outcome.removed_across_one_shot {
                tracing::info!(
                    station_a = ?tie.station_a,
                    station_b = ?tie.station_b,
                    kind = ?tie.kind,
                    purpose = "drop a tie a combine left across one shot",
                    "Combine removed a loop closure that would span a single shot"
                );
            }
            // A closure replaced by one the Closures Computed View adjusts on
            // the same two stations (#749): the dialog listed it, and the
            // connection stays, so no toast; the log says which.
            for replaced in &outcome.replaced {
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
            warnings = combine_removed_warnings(&outcome.removed_across_one_shot, &labels);
            project.touch();
        }

        self.settle_datum_layout(&datums_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };
        // The label only names the operation in history. It must not contain a
        // ReverseLine even when `other` was reversed: undo/redo settle
        // reversed lines off this command, and `other` no longer exists.
        let label = Command::Batch {
            commands: vec![
                Command::UpdateLineMetadata {
                    line_id: kept,
                    updates: label_updates,
                },
                Command::DeleteLine { line_id: other },
            ],
        };
        self.push_snapshot_undo(label, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): set-only; a no-op for a local project.
        self.mark_remote_forward_edit();

        tracing::info!(kept, other, "Combine lines completed");
        CommandResult::success()
            .with_line_id(kept)
            .with_line_ids(vec![kept])
            .with_warnings(warnings)
    }
}
