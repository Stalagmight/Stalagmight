//! [#576 NF-W6A-4] The open project and its closure-adjusted geometry, from ONE
//! settled version.
//!
//! A reader that renders the project with its adjusted stations (the image export
//! and its preview) used to take the project and the adjusted solve as two reads:
//! a command landing between them paired the project of one version with the
//! stations of another (a section renamed, deleted or extended in one and not the
//! other). Here the snapshot is taken between commands, tagged with the memo's
//! invalidation generation in the same `command_lock` span, and its adjusted
//! stations are either the memo's - only while no command has run since the
//! snapshot, so the memo can only describe this very project - or a solve of
//! this snapshot itself.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::app_state::{adjusted_config_hash, AdjustedStationsCache};
use super::AppState;
use crate::models::{Project, Station};

/// A settled snapshot of the open project, for reading it together with its
/// adjusted geometry ([`AppState::adjusted_stations_of`]).
#[derive(Debug, Clone)]
pub struct SettledProject {
    /// The project as it stood between two commands.
    pub project: Arc<Project>,
    /// The adjusted-stations memo's generation when `project` was taken.
    generation: u64,
}

impl AppState {
    /// The open project as it stands between commands (see
    /// [`Self::settled_snapshot`]), tagged so its adjusted stations can be
    /// fetched for exactly this version. `None` when no project is open. Not
    /// reentrant: never call it inside a command span.
    pub fn settled_project(&self) -> Option<SettledProject> {
        let _between_commands = self.hold_commands();
        let generation = self.adjusted_cache_generation.load(Ordering::SeqCst);
        let project = self.project_snapshot()?;
        Some(SettledProject {
            project,
            generation,
        })
    }

    /// The closure-adjusted stations of `settled.project` - measured where no
    /// closure applies, as [`Self::get_adjusted_stations`] answers. The memo answers
    /// when nothing has invalidated it since the snapshot (every command does);
    /// otherwise this solves a copy of the snapshot, and stores the result only if
    /// still nothing has.
    pub fn adjusted_stations_of(&self, settled: &SettledProject) -> Vec<Station> {
        let config = self.get_config();
        let config_hash = adjusted_config_hash(&config.closure_adjustment);
        let version = settled
            .project
            .modified_at
            .timestamp_nanos_opt()
            .unwrap_or(0);
        {
            let cache = self.adjusted_cache.lock();
            let unchanged =
                self.adjusted_cache_generation.load(Ordering::SeqCst) == settled.generation;
            if let Some(entry) = cache.as_ref() {
                if unchanged && entry.version == version && entry.config_hash == config_hash {
                    return entry.stations.clone();
                }
            }
        }
        let stations = Project::clone(&settled.project)
            .into_adjusted_station_coordinates(&config.closure_adjustment);
        let mut cache = self.adjusted_cache.lock();
        if self.adjusted_cache_generation.load(Ordering::SeqCst) == settled.generation {
            *cache = Some(AdjustedStationsCache {
                version,
                config_hash,
                stations: stations.clone(),
            });
        }
        stations
    }
}
