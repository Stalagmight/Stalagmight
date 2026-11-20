use super::AppState;
use crate::core::CommandResult;
use crate::models::{normalize_azimuth, Coordinates, LineId, StationRef};

impl AppState {
    pub(super) fn move_line(&self, line_id: LineId, offset: Coordinates) -> CommandResult {
        // "Adjust Position" on a line moves the WHOLE rigid group it belongs to
        // (its merge partners and aligned-closure-connected lines), not just this
        // one line. A merged line is rigidly bound to its partner: moving it
        // alone would shove its merged station off the partner's, and the
        // post-command geometry reconcile would then either snap it back (silently
        // discarding the move when this isn't the group's min-id anchor) or drag the
        // partner along. Moving the rigid group as one body keeps every merge aligned,
        // so the move sticks. For a standalone line the rigid group is just itself,
        // so this is identical to the previous single-line move. Delegating to
        // `move_segment` also upgrades the datum guard from "this line holds a
        // datum" to "the group holds a datum" (a partner's datum now blocks too).
        self.move_segment(line_id, offset)
    }

    pub(super) fn move_segment(&self, line_id: LineId, offset: Coordinates) -> CommandResult {
        let _ = (&line_id, &offset);
        crate::core::stub::not_ported("command_move::move_segment")
    }

    pub(super) fn move_station(
        &self,
        station_ref: StationRef,
        offset: Coordinates,
    ) -> CommandResult {
        let _ = (&station_ref, &offset);
        crate::core::stub::not_ported("command_move::move_station")
    }
}
