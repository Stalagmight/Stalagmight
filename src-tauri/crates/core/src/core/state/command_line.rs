use super::AppState;
use crate::core::{CommandResult, Validator};
use crate::models::{
    LineData, LineId, LineUpdates, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
};

impl AppState {
    pub(super) fn create_line(&self, data: LineData) -> CommandResult {
        let _ = (&data,);
        crate::core::stub::not_ported("command_line::create_line")
    }

    pub(super) fn delete_line(&self, line_id: LineId) -> CommandResult {
        let _ = (&line_id,);
        crate::core::stub::not_ported("command_line::delete_line")
    }

    /// Reverse a line end-to-end (issue #24).  The far end becomes station 0
    /// and every station keeps its world coordinate; see
    /// [`crate::models::Project::reverse_line`] for the transform.
    ///
    /// Frozen shots are deliberately NOT consulted here.  `update_shot` rejects
    /// azimuth/depth/pitch edits on a frozen shot because those move the station
    /// it is pinned to, but reverse re-associates each reading with the other end
    /// of the same physical line and leaves the line itself untouched, so the
    /// frozen invariant still holds.  Applying the frozen check would block
    /// reverse on any datum-anchored line for no benefit.
    pub(super) fn reverse_line(&self, line_id: LineId, swap_lr: bool) -> CommandResult {
        let _ = (&line_id, &swap_lr);
        crate::core::stub::not_ported("command_line::reverse_line")
    }

    pub(super) fn update_line(&self, line_id: LineId, updates: LineUpdates) -> CommandResult {
        let _ = (&line_id, &updates);
        crate::core::stub::not_ported("command_line::update_line")
    }
}
