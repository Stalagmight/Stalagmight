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

/// The execute pipeline's mutation for the station topology family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit), for rows only the result can say.
pub(super) fn apply(
    project: &mut crate::models::survey::Project,
    record: &mut super::commands::Record,
    cmd: &crate::core::Command,
) -> Result<super::commands::Applied, String> {
    let _ = (&mut *project, &mut *record);
    match cmd {
        crate::core::Command::MergeStations {
            station_a,
            station_b,
        } => {
            let (a, b) = project
                .resolve_slot_pair(*station_a, *station_b)
                .map_err(|e| e.to_string())?;
            project
                .merge_stations(a.station, b.station)
                .map_err(|e| e.to_string())?;
            Ok(super::commands::Applied::default())
        }
        crate::core::Command::UnmergeStations { .. } => {
            Err(super::commands::not_ported("execute::unmerge_stations"))
        }
        crate::core::Command::UnmergeStationPair { .. } => {
            Err(super::commands::not_ported("execute::unmerge_station_pair"))
        }
        crate::core::Command::CreateLoopClosure { .. } => {
            Err(super::commands::not_ported("execute::create_loop_closure"))
        }
        crate::core::Command::RemoveLoopClosure { .. } => {
            Err(super::commands::not_ported("execute::remove_loop_closure"))
        }
        crate::core::Command::SetZeroDatum { .. } => {
            Err(super::commands::not_ported("execute::set_zero_datum"))
        }
        crate::core::Command::RemoveZeroDatum { .. } => {
            Err(super::commands::not_ported("execute::remove_zero_datum"))
        }
        _ => Err(format!(
            "{} is not a station topology command",
            crate::core::command_kind(cmd)
        )),
    }
}
