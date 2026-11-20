use super::AppState;
use crate::core::{CommandResult, ValidationWarning, Validator, WarningSeverity};
use crate::models::{
    ClosurePair, ClosuresBeforeEdit, ClosuresBeforeSweep, LineId, Project, RemovedClosure,
    ShotData, ShotId, ShotKind, ShotUpdates, SurveyCredit, CLOSURE_REMOVED_ACROSS_ONE_SHOT,
    CLOSURE_REMOVED_MERGED_ENDS, FEATURE_ID_STRIDE,
};

/// The warning a delete reports for a loop closure it removed because the
/// delete left it spanning a single shot (#737). Names the closure by its
/// stations as the user read them BEFORE the delete (`before`, from
/// `Project::closures_before_edit`): a gap-close renumbers stations, and the
/// user created the closure between `Main / 1` and `Main / 5`, not whatever
/// the delete made of them. A closure `before` cannot name (it had no
/// resolved ends then) is named by its stations now.
pub(super) fn closure_removed_message(
    project: &Project,
    before: &ClosuresBeforeEdit,
    closure: &ClosurePair,
) -> String {
    let (a, b) = before.labels(closure.closure_id).map_or_else(
        || {
            (
                project.station_text(closure.station_a),
                project.station_text(closure.station_b),
            )
        },
        |(a, b)| (a.to_string(), b.to_string()),
    );
    format!(
        "Removed loop closure {a} to {b}: closing the gap left its two stations at the two ends of one shot."
    )
}

/// What made the two stations of a closure the sweep removed one point, so
/// its warning can say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OnePointCause {
    /// The edit's own promotion of another closure to a merge (#738).
    Promotion,
    /// A user merge, which may join two placed pieces since #741.
    Merge,
}

impl OnePointCause {
    /// The cause for the sweep after `cmd`.
    pub(super) fn of(cmd: &crate::core::Command) -> Self {
        if matches!(cmd, crate::core::Command::MergeStations { .. }) {
            Self::Merge
        } else {
            Self::Promotion
        }
    }
}

/// The warnings an edit reports for the loop closures its orphan-closure sweep
/// removed because their two stations are one point (#738), each named by the
/// stations it had BEFORE the edit. The frontend shows them in one toast with
/// the closures-across-one-shot ones (`notifyRemovedClosures`).
pub(super) fn merged_ends_warnings(
    removed: &[RemovedClosure],
    cause: OnePointCause,
) -> Vec<ValidationWarning> {
    removed
        .iter()
        .map(|removed| ValidationWarning {
            code: CLOSURE_REMOVED_MERGED_ENDS.to_string(),
            message: format!(
                "Removed loop closure {} to {}: {}",
                removed.labels.0,
                removed.labels.1,
                match (removed.already_one_point, cause) {
                    (true, _) => "its two stations were already one point.",
                    (false, OnePointCause::Promotion) => {
                        "another loop closure became a merge, which made its two stations one point."
                    }
                    (false, OnePointCause::Merge) => {
                        "the merge made its two stations one point."
                    }
                }
            ),
            severity: WarningSeverity::Warning,
        })
        .collect()
}

/// After an edit that ran the whole-project sweep: log what it removed, and
/// turn it into [`merged_ends_warnings`] named against `before` (the capture
/// the edit took before it changed anything).
pub(super) fn report_swept_closures(
    project: &Project,
    before: &ClosuresBeforeSweep,
    sweep: &crate::models::OrphanClosureSweep,
    context: &str,
    cause: OnePointCause,
) -> Vec<ValidationWarning> {
    sweep.log_removed(context);
    merged_ends_warnings(&project.removed_closures_named(before, sweep), cause)
}

/// After a delete with shift: remove every loop closure the delete left tying
/// the two ends of one shot together that did not before it (#737), and
/// report each one for the user, by the stations it had before the delete.
///
/// A gap-close shortens the path between a closure's two ends - closure
/// A:1-A:4 with shots 1..2 deleted becomes A:1-A:2, one shot - so the closure
/// that closed a real loop now closes none, and the solve would only skip it.
/// It goes with the delete, inside the same undo unit (both callers record a
/// project snapshot pair), so one Undo restores the shots AND the closure.
///
/// Deliberately NOT part of `remove_shot_with_rewire*`: that is also the
/// inverse of `CreateShot`, and an inverse must only invert (#413). A closure
/// that already spanned one shot before the delete is left alone (it carries
/// its own warning); only what this delete caused is undone here. Removing a
/// skipped closure never changes another closure's status, because a skipped
/// closure unites nothing. The range-delete preview lists the same closures
/// from the same `Project::closures_newly_across_one_shot`.
pub(super) fn remove_closures_newly_across_one_shot(
    project: &mut Project,
    before: &ClosuresBeforeEdit,
) -> Vec<ValidationWarning> {
    let newly = project.closures_newly_across_one_shot(before);
    let mut warnings = Vec::with_capacity(newly.len());
    for closure in newly {
        if project.remove_loop_closure(closure.closure_id).is_err() {
            continue;
        }
        tracing::info!(
            closure_id = closure.closure_id,
            station_a = ?closure.station_a,
            station_b = ?closure.station_b,
            purpose = "drop a closure a delete left across one shot",
            "Removed loop closure that now spans a single shot"
        );
        warnings.push(ValidationWarning {
            code: CLOSURE_REMOVED_ACROSS_ONE_SHOT.to_string(),
            message: closure_removed_message(project, before, &closure),
            severity: WarningSeverity::Warning,
        });
    }
    warnings
}

impl AppState {
    pub(super) fn create_shot(
        &self,
        line_id: LineId,
        mut data: ShotData,
        after_shot_id: Option<ShotId>,
        before_shot_id: Option<ShotId>,
    ) -> CommandResult {
        let _ = (&line_id, &data, &after_shot_id, &before_shot_id);
        crate::core::stub::not_ported("command_shot::create_shot")
    }

    pub(super) fn update_shot(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        updates: ShotUpdates,
    ) -> CommandResult {
        self.write_shot_values(line_id, shot_id, updates, true)
    }

    /// [`Self::update_shot`] for an INVERSE: the same write, with the
    /// frozen-geometry refusal skipped.
    ///
    /// **If a forward command was permitted, its inverse must be too** (#413,
    /// findings 3 and 6; docs/undo-redo.md). The frozen check says what a USER
    /// may do - do not edit geometry the survey network is solved against - and
    /// it is a question about the state the edit STARTS from, which for an undo
    /// is the state the forward command PRODUCED, not the one it was judged
    /// against. Those differ whenever the edit itself changed what is frozen,
    /// and one route does it in a single command: a shot edit that pulls a
    /// snap-to-merge closure's endpoints into coincidence promotes the closure to
    /// a rigid edge (#469 item 2), which closes an anchored loop and freezes the
    /// very shot that was just edited. `undo_inner`'s epilogue pushes a refused
    /// entry back, so that would wedge the history permanently, with every older
    /// entry sealed off beneath it.
    ///
    /// Only the intent guard is skipped. The non-finite and length checks are
    /// about the VALUES being written, not about permission, and an inverse
    /// writes values a forward command already wrote - so they can only fire on a
    /// payload that was invalid when it was recorded, which is a different bug
    /// and should still be refused.
    pub(super) fn restore_shot_values(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        updates: ShotUpdates,
    ) -> CommandResult {
        self.write_shot_values(line_id, shot_id, updates, false)
    }

    fn write_shot_values(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        updates: ShotUpdates,
        enforce_frozen: bool,
    ) -> CommandResult {
        let _ = (&line_id, &shot_id, &updates, &enforce_frozen);
        crate::core::stub::not_ported("command_shot::write_shot_values")
    }

    pub(super) fn delete_shot(&self, line_id: LineId, shot_id: ShotId) -> CommandResult {
        let _ = (&line_id, &shot_id);
        crate::core::stub::not_ported("command_shot::delete_shot")
    }

    /// Remove the shot a `CreateShot` created, as the exact inverse of the
    /// insert that made it. Undo's path - NEVER a user's.
    ///
    /// This deliberately does not go through [`Self::delete_shot`]. That
    /// function's guards encode USER INTENT: refuse to tear a loop apart,
    /// refuse to touch collaborator-locked geometry. An inverse must not
    /// consult them, because **if a forward command was permitted, its inverse
    /// must be too** - and the shot being removed is one that did not exist
    /// before the command being undone, so removing it cannot break anything
    /// the insert did not already build. Routing undo through the user-facing
    /// delete produced an undo that FAILS outright, and `undo_inner` pushes a
    /// failed entry back onto the stack, so it fails identically forever and
    /// every entry beneath it becomes permanently unreachable - a wedged
    /// history (#413, finding 6). Both of `delete_shot`'s refusals fire in
    /// practice: the frozen-shot check and the shift-blocked loop/datum pair,
    /// so special-casing either one is not a fix.
    ///
    /// The guards keep applying to a USER delete: `Command::DeleteShot` still
    /// runs `delete_shot`, guards and all.
    ///
    /// `appended_into_empty_line` is the recorded shape of the insert being
    /// undone (both anchors absent and position 0 - an `add_shot` append into a
    /// line that had no shots); it is re-checked against the live line
    /// before it is honored.
    /// Give a line that is shot-less again the legacy pair its first shot
    /// took (the undo of that shot's creation).
    pub(super) fn restore_consumed_legacy(
        &self,
        line_id: LineId,
        pair: &(Option<String>, Option<chrono::NaiveDate>),
    ) {
        let _ = (&line_id, &pair);
        crate::core::stub::not_ported("command_shot::restore_consumed_legacy")
    }

    pub(super) fn remove_created_shot(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        appended_into_empty_line: bool,
    ) -> CommandResult {
        let _ = (&line_id, &shot_id, &appended_into_empty_line);
        crate::core::stub::not_ported("command_shot::remove_created_shot")
    }

    pub(super) fn set_shot_kind(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        kind: ShotKind,
    ) -> CommandResult {
        let _ = (&line_id, &shot_id, &kind);
        crate::core::stub::not_ported("command_shot::set_shot_kind")
    }
}
