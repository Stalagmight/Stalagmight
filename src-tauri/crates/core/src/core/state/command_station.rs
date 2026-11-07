use super::AppState;
use crate::core::CommandResult;
use crate::models::{ClosureStrategy, GpsCoordinates, StationRef};

impl AppState {
    pub(super) fn merge_stations(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            // Check same section (not allowed)
            if station_a.section_id == station_b.section_id {
                return CommandResult::failure(
                    "Cannot merge stations from the same section".to_string(),
                );
            }

            match project.merge_stations(station_a, station_b) {
                Ok(_) => {
                    tracing::debug!(
                        ?station_a,
                        ?station_b,
                        purpose = "connect survey sections",
                        "Stations merged"
                    );
                    CommandResult::success()
                }
                Err(e) => {
                    tracing::warn!(
                        ?station_a,
                        ?station_b,
                        error = %e,
                        purpose = "connect survey sections",
                        "Failed to merge stations"
                    );
                    CommandResult::failure(e)
                }
            }
        })
    }

    pub(super) fn create_loop_closure(
        &self,
        station_a: StationRef,
        station_b: StationRef,
        strategy: ClosureStrategy,
    ) -> CommandResult {
        let mut proj_guard = self.project.lock();

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        match project.create_loop_closure(station_a, station_b, strategy) {
            Ok(_) => {
                tracing::debug!(
                    ?station_a,
                    ?station_b,
                    purpose = "add loop closure to improve accuracy",
                    "Loop closure created"
                );
                CommandResult::success()
            }
            Err(e) => {
                tracing::warn!(
                    ?station_a,
                    ?station_b,
                    error = %e,
                    purpose = "add loop closure to improve accuracy",
                    "Failed to create loop closure"
                );
                CommandResult::failure(e)
            }
        }
    }

    pub(super) fn remove_loop_closure(&self, closure_id: u64) -> CommandResult {
        self.with_project_mut(|project| match project.remove_loop_closure(closure_id) {
            Ok(closure) => {
                tracing::debug!(
                    closure_id,
                    ?closure.station_a,
                    ?closure.station_b,
                    purpose = "remove loop closure from project",
                    "Loop closure removed"
                );
                CommandResult::success()
            }
            Err(e) => {
                tracing::warn!(
                    closure_id,
                    error = %e,
                    purpose = "remove loop closure from project",
                    "Failed to remove loop closure"
                );
                CommandResult::failure(e)
            }
        })
    }

    pub(super) fn unmerge_stations(&self, station_ref: StationRef) -> CommandResult {
        self.with_project_mut(|project| {
            // Find and unmerge all merges involving this station
            let merges_to_remove: Vec<(StationRef, StationRef)> = project
                .station_merges
                .iter()
                .filter(|m| {
                    project.key_is_ref(&m.station_a, station_ref)
                        || project.key_is_ref(&m.station_b, station_ref)
                })
                .filter_map(|m| {
                    match (
                        project.resolve_key(&m.station_a),
                        project.resolve_key(&m.station_b),
                    ) {
                        (Some(a), Some(b)) => Some((a, b)),
                        _ => None,
                    }
                })
                .collect();

            for (a, b) in merges_to_remove {
                project.unmerge_stations(a, b);
            }

            tracing::info!(
                ?station_ref,
                purpose = "disconnect merged stations",
                "Station unmerged"
            );
            CommandResult::success()
        })
    }

    pub(super) fn unmerge_station_pair(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            let (success, _removed_closures) = project.unmerge_stations(station_a, station_b);
            if success {
                tracing::info!(
                    ?station_a,
                    ?station_b,
                    purpose = "disconnect selected merged station pair",
                    "Station pair unmerged"
                );
                CommandResult::success()
            } else {
                tracing::warn!(
                    ?station_a,
                    ?station_b,
                    purpose = "disconnect selected merged station pair",
                    "Failed to unmerge station pair"
                );
                CommandResult::failure("Stations are not directly merged".to_string())
            }
        })
    }

    pub(super) fn set_zero_datum(
        &self,
        station_ref: StationRef,
        gps: GpsCoordinates,
    ) -> CommandResult {
        self.with_project_mut(|project| match project.set_zero_datum(station_ref, gps) {
            Ok(_) => {
                tracing::info!(
                    ?station_ref,
                    purpose = "anchor survey to GPS coordinates",
                    "Zero datum set"
                );
                CommandResult::success()
            }
            Err(e) => {
                tracing::warn!(
                    ?station_ref,
                    error = %e,
                    purpose = "anchor survey to GPS coordinates",
                    "Failed to set zero datum"
                );
                CommandResult::failure(e)
            }
        })
    }

    pub(super) fn remove_zero_datum(&self, station_ref: StationRef) -> CommandResult {
        self.with_project_mut(|project| {
            if project.remove_zero_datum(station_ref) {
                // Removing the over-constraining datum can leave the merge group
                // pinned to a now-absent second anchor with a residual tear. The
                // standard geometry epilogue (realign_merges) is tolerance-gated at
                // 5 cm, so a sub-tolerance tear would persist until a reload. Force a
                // zero-trust re-solve here so removing the offending datum heals the
                // group in-session, exactly as a reload would.
                project.resolve_offsets();
                tracing::debug!(
                    ?station_ref,
                    purpose = "remove GPS anchor",
                    "Zero datum removed"
                );
                CommandResult::success()
            } else {
                CommandResult::failure("Station is not a zero datum".to_string())
            }
        })
    }
}
