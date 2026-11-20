use super::AppState;
use crate::models::{ConnectivityResult, GpsCoordinates, LineId, StationRef, ZeroDatumPreview};

impl AppState {
    /// What setting a zero datum with `coordinates` on `station_ref` would do
    /// (read-only, NOT a Command): its role, and for a GPS fix the gap between
    /// the GPS and the surveyed position (#741). `Ok(None)` when the station does
    /// not exist; `Err` when no project is open or the GPS is invalid.
    pub fn preview_zero_datum(
        &self,
        station_ref: StationRef,
        coordinates: GpsCoordinates,
    ) -> Result<Option<ZeroDatumPreview>, String> {
        let _ = (&station_ref, &coordinates);
        crate::core::stub::not_ported("command_query::preview_zero_datum")
    }

    /// Check merge connectivity between two stations (read-only)
    pub fn check_merge_connectivity(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> Result<ConnectivityResult, String> {
        let _ = (&station_a, &station_b);
        crate::core::stub::not_ported("command_query::check_merge_connectivity")
    }

    /// Get all line IDs connected through merges and aligned closures (rigid groups).
    pub fn get_rigid_connected_line_ids(&self, line_id: LineId) -> Result<Vec<LineId>, String> {
        let _ = (&line_id,);
        crate::core::stub::not_ported("command_query::get_rigid_connected_line_ids")
    }

    /// Get shot-line geometry for all lines in the segment containing the given line.
    /// Coordinates are relative to the given line's station 0.
    pub fn get_segment_geometry(
        &self,
        line_id: LineId,
    ) -> Result<crate::models::SegmentGeometry, String> {
        let _ = (&line_id,);
        crate::core::stub::not_ported("command_query::get_segment_geometry")
    }

    /// Get all line IDs anchored by a zero datum.
    /// A zero datum anchors every line reachable through merges AND aligned closures,
    /// since unaligned closures are not rigidly constrained yet.
    pub fn get_anchored_line_ids(&self) -> Result<Vec<LineId>, String> {
        crate::core::stub::not_ported("command_query::get_anchored_line_ids")
    }
}
