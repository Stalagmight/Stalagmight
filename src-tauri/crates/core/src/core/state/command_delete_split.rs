use super::AppState;

use crate::core::{Command, CommandResult};
use crate::models::{
    LineData, LineId, Project, ShotData, ShotId, StationKey, StationRef, FEATURE_ID_STRIDE,
    MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
};

/// Every split path (both delete-and-split paths and Split Line Here) turns
/// one line into two, so each MINTS a line id (#555) through the same
/// `Project::add_line` door `create_line` guards, while going nowhere
/// near `create_line` itself. Refused before any path mutates anything: a
/// split that ran out of ids half-way would leave line A truncated with
/// nowhere to put its tail.
pub(super) fn line_budget_refusal(project: &Project, line_id: LineId) -> Option<CommandResult> {
    if project.next_line_id <= MAX_LINE_ID {
        return None;
    }
    Some(CommandResult::failure(format!(
        "Cannot split {}: to split it into two lines needs \
         a new line id, but the project has reached the maximum number of lines \
         (limit = {MAX_LINES_IN_ID_SPACE}); shot ids encode as \
         line_id * {FEATURE_ID_STRIDE} + position in 32 bits, so line ids \
         past {MAX_LINE_ID} cannot hold shots",
        project
            .line_label(line_id)
            .map_or_else(|| "the line".to_string(), |name| format!("line \"{name}\""))
    )))
}

impl AppState {

    /// The two halves' names of a delete-and-split of `line_id`, trimmed, or
    /// the refusal (`Project::validate_replacement_line_names`: empty, or
    /// another line's, as a rename is refused). A missing line or project is
    /// left for the command to report.
    fn validate_split_names(
        &self,
        line_id: LineId,
        name_a: &str,
        name_b: &str,
    ) -> Result<(String, String), CommandResult> {
        let _ = (&line_id, &name_a, &name_b);
        crate::core::stub::not_ported("command_delete_split::validate_split_names")
    }

    /// Execute a delete-shot-and-split-line operation as a single atomic undo/redo unit.
    /// Deletes a mid-line shot and splits the line into two, with user-provided names.
    /// Uses `ProjectSnapshotPair` for single-undo. Follows `execute_create_and_merge()` pattern.
    pub fn execute_delete_shot_and_split(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        name_a: String,
        name_b: String,
    ) -> CommandResult {
        let _ = (&line_id, &shot_id, &name_a, &name_b);
        crate::core::stub::not_ported("command_delete_split::execute_delete_shot_and_split")
    }

    /// Delete a contiguous *interior* range of shots and split the line into
    /// two: line A keeps the shots before the range (in place), line B is a
    /// new line holding the shots after the range (offset so it keeps its
    /// absolute position). Generalizes [`Self::execute_delete_shot_and_split`]
    /// from a single shot to `[start_pos..=end_pos]`. Commits as ONE
    /// `ProjectSnapshotPair` undo entry. The range must be interior (shots remain
    /// before AND after); prefix/suffix/whole ranges use the shift or
    /// line-delete paths.
    pub fn execute_delete_shot_range_split(
        &self,
        line_id: LineId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
        name_a: String,
        name_b: String,
    ) -> CommandResult {
        let _ = (&line_id, &start_shot_id, &end_shot_id, &name_a, &name_b);
        crate::core::stub::not_ported("command_delete_split::execute_delete_shot_range_split")
    }

    /// Delete a contiguous range of shots by gap-closing (shifting) - the
    /// survivors rejoin and the line stays single. ATOMIC: the range's shots
    /// go in forward order inside ONE project lock and ONE `ProjectSnapshotPair`
    /// undo unit, with the geometry cascade DEFERRED to a single reconcile at the
    /// end (each step is a valid single-shot gap-close: head/before stays for
    /// interior & suffix ranges, tail/after stays for prefix ranges). Refuses
    /// when the range is shift-blocked or would empty the line (use split /
    /// line delete respectively).
    ///
    /// # Not a composition of `n` `Command::DeleteShot`s, on purpose
    ///
    /// This comment used to say the method "generalizes" the single-shot
    /// `delete_shot` by deleting the range's shots in forward order, which reads
    /// as a promise that the range is the same edit as the `n` singles. It is
    /// not, and the promise was the defect (issue #413 finding 22, resolved as
    /// intended behaviour). What holds:
    ///
    /// * for a range that removes NO station a merge is anchored to, the two are
    ///   identical - measured over prefix and interior ranges, they agree to the
    ///   last bit;
    /// * for a range whose removed stations DO include one a merged partner
    ///   line is anchored to (the doomed-tail-anchor shape), they deliberately
    ///   DIVERGE. `AppState::execute` runs the geometry epilogue after every
    ///   command, so the single-delete sequence re-anchors the partner while the
    ///   merge is still live, drags it along as the line shrinks, and then
    ///   deletes the merge - leaving the partner displaced with no edge to pull
    ///   it back. Deferring to one final cascade leaves it where the surveyor put
    ///   it, which is what this method does.
    ///
    /// The atomic behaviour is the reviewed intent, pinned by
    /// `test_delete_range_shift_does_not_displace_free_floating_merged_line`
    /// and `test_delete_range_suffix_shift_keeps_head_with_doomed_tail_anchor`
    /// (PR #95, `core/state/tests.rs`), and asserted on random topology by the
    /// metamorphic pair `range-shift-is-n-single-deletes`
    /// (`core/state/metamorphic_undo_tests.rs`), which requires equality on every
    /// other shape.
    pub fn execute_delete_shot_range_shift(
        &self,
        line_id: LineId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
    ) -> CommandResult {
        let _ = (&line_id, &start_shot_id, &end_shot_id);
        crate::core::stub::not_ported("command_delete_split::execute_delete_shot_range_shift")
    }

    /// Execute a delete-last-shot-and-line operation as a single atomic undo/redo unit.
    /// When a line has exactly 1 shot remaining, this removes the line (and its
    /// only shot) along with all line references (merges, closures, line_offsets,
    /// station_offsets, zero_datums). Uses `ProjectSnapshotPair` for single-undo.
    pub fn execute_delete_last_shot_and_line(
        &self,
        line_id: LineId,
        shot_id: ShotId,
    ) -> CommandResult {
        let _ = (&line_id, &shot_id);
        crate::core::stub::not_ported("command_delete_split::execute_delete_last_shot_and_line")
    }
}
