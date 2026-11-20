use super::AppState;
use crate::core::CommandResult;
use crate::models::{ClosureStrategy, GpsCoordinates, StationRef};

impl AppState {
    pub(super) fn merge_stations(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> CommandResult {
        let _ = (&station_a, &station_b);
        crate::core::stub::not_ported("command_station::merge_stations")
    }

    pub(super) fn create_loop_closure(
        &self,
        station_a: StationRef,
        station_b: StationRef,
        strategy: ClosureStrategy,
    ) -> CommandResult {
        let _ = (&station_a, &station_b, &strategy);
        crate::core::stub::not_ported("command_station::create_loop_closure")
    }

    pub(super) fn remove_loop_closure(&self, closure_id: u64) -> CommandResult {
        let _ = (&closure_id,);
        crate::core::stub::not_ported("command_station::remove_loop_closure")
    }

    pub(super) fn unmerge_stations(&self, station_ref: StationRef) -> CommandResult {
        let _ = (&station_ref,);
        crate::core::stub::not_ported("command_station::unmerge_stations")
    }

    pub(super) fn unmerge_station_pair(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> CommandResult {
        let _ = (&station_a, &station_b);
        crate::core::stub::not_ported("command_station::unmerge_station_pair")
    }

    pub(super) fn set_zero_datum(
        &self,
        station_ref: StationRef,
        gps: GpsCoordinates,
        accuracy_m: Option<f64>,
        entrance: Option<bool>,
    ) -> CommandResult {
        let _ = (&station_ref, &gps, &accuracy_m, &entrance);
        crate::core::stub::not_ported("command_station::set_zero_datum")
    }

    pub(super) fn remove_zero_datum(&self, station_ref: StationRef) -> CommandResult {
        let _ = (&station_ref,);
        crate::core::stub::not_ported("command_station::remove_zero_datum")
    }
}
