use super::AppState;
use crate::models::{ConnectivityResult, GpsCoordinates, SectionId, StationRef, ZeroDatumPreview};

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
        GpsCoordinates::validate(
            coordinates.latitude,
            coordinates.longitude,
            coordinates.depth_reference,
        )?;
        self.with_project(|project| Ok(project.preview_zero_datum(station_ref, &coordinates)))
    }

    /// Check merge connectivity between two stations (read-only)
    pub fn check_merge_connectivity(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> Result<ConnectivityResult, String> {
        let proj_guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let project = proj_guard
            .as_ref()
            .ok_or_else(|| "No project open".to_string())?;

        // Validate stations exist
        if !project.station_exists(station_a) {
            return Err("Station A does not exist.".to_string());
        }
        if !project.station_exists(station_b) {
            return Err("Station B does not exist.".to_string());
        }

        // Check if stations already merged
        if project.are_stations_merged(station_a, station_b) {
            return Err("These stations are already merged.".to_string());
        }

        // Check for existing closure between them
        if project.are_stations_in_closure(station_a, station_b) {
            return Err("A loop closure already exists between these stations.".to_string());
        }

        // Always compute connectivity (via merges and closures) for the `connected` field
        let connected_a = project.get_connected_section_ids(station_a.section_id);
        let connected = connected_a.contains(&station_b.section_id);
        let preview = if connected {
            Some(project.compute_closure_metrics(station_a, station_b)?)
        } else {
            None
        };

        // Two placed sections MAY merge (#741): the group whose placement datum
        // is later in the list moves to meet the merge, and that datum becomes a
        // GPS fix. So zero datums no longer decide `can_merge`; only a loop does.
        let (can_merge, reason) = if connected {
            // Already connected - merging would create a survey loop
            (false, Some("Creates survey loop".to_string()))
        } else {
            (true, None)
        };

        Ok(ConnectivityResult {
            connected,
            preview,
            can_merge,
            reason,
        })
    }

    /// Get all section IDs connected through merges and aligned closures (rigid groups).
    pub fn get_rigid_connected_section_ids(
        &self,
        section_id: SectionId,
    ) -> Result<Vec<SectionId>, String> {
        // Serialize with command execution to avoid observing intermediate batch state
        let _cmd_guard = self
            .command_lock
            .lock()
            .map_err(|_| "Failed to acquire command lock".to_string())?;
        self.with_project(|project| {
            // Validate section exists
            if !project.sections.iter().any(|s| s.id == section_id) {
                return Err(format!("Section {section_id} not found"));
            }

            // Filter to existing sections (stale merges/closures may reference deleted sections)
            let existing: std::collections::HashSet<SectionId> =
                project.sections.iter().map(|s| s.id).collect();
            let connected = project.get_rigid_connected_section_ids(section_id);
            let mut result: Vec<SectionId> = connected
                .into_iter()
                .filter(|id| existing.contains(id))
                .collect();
            result.sort_unstable();
            Ok(result)
        })
    }

    /// Get shot-line geometry for all sections in the segment containing the given section.
    /// Coordinates are relative to the given section's station 0.
    pub fn get_segment_geometry(
        &self,
        section_id: SectionId,
    ) -> Result<crate::models::SegmentGeometry, String> {
        use crate::models::{SegmentGeometry, SegmentSectionGeometry};

        // Serialize with command execution to avoid observing intermediate batch state
        let _cmd_guard = self
            .command_lock
            .lock()
            .map_err(|_| "Failed to acquire command lock".to_string())?;
        let proj_guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let project = proj_guard
            .as_ref()
            .ok_or_else(|| "No project open".to_string())?;

        // Validate section exists
        if !project.sections.iter().any(|s| s.id == section_id) {
            return Err(format!("Section {section_id} not found"));
        }

        // Get all connected section IDs
        let existing: std::collections::HashSet<SectionId> =
            project.sections.iter().map(|s| s.id).collect();
        let connected = project.get_connected_section_ids(section_id);
        let connected: std::collections::HashSet<SectionId> = connected
            .into_iter()
            .filter(|id| existing.contains(id))
            .collect();

        // Compute all station coordinates
        let all_stations = project.compute_station_coordinates();

        // Find anchor: station 0 of the given section
        let anchor = all_stations
            .iter()
            .find(|s| s.station_ref.section_id == section_id && s.station_ref.station_index == 0)
            .map(|s| (s.coordinates.x, s.coordinates.y))
            .unwrap_or((0.0, 0.0));

        // Group stations by section
        let mut section_stations: std::collections::HashMap<
            SectionId,
            Vec<&crate::models::Station>,
        > = std::collections::HashMap::new();
        for station in &all_stations {
            if connected.contains(&station.station_ref.section_id) {
                section_stations
                    .entry(station.station_ref.section_id)
                    .or_default()
                    .push(station);
            }
        }

        // Build geometry for each section
        let mut sections = Vec::new();
        for &sid in &connected {
            let Some(section) = project.sections.iter().find(|s| s.id == sid) else {
                continue;
            };
            let Some(stations) = section_stations.get(&sid) else {
                continue;
            };

            // Sort stations by station_index to ensure correct shot-line pairing
            let mut sorted_stations = stations.clone();
            sorted_stations.sort_by_key(|s| s.station_ref.station_index);

            // Build shot lines: consecutive station pairs (station i → station i+1)
            let mut shot_lines = Vec::new();
            for pair in sorted_stations.windows(2) {
                let from = pair[0];
                let to = pair[1];
                shot_lines.push([
                    [from.coordinates.x - anchor.0, from.coordinates.y - anchor.1],
                    [to.coordinates.x - anchor.0, to.coordinates.y - anchor.1],
                ]);
            }

            sections.push(SegmentSectionGeometry {
                section_id: sid,
                color: section.color.clone(),
                shot_lines,
            });
        }

        Ok(SegmentGeometry { sections })
    }

    /// Get all section IDs anchored by a zero datum.
    /// A zero datum anchors every section reachable through merges AND aligned closures,
    /// since unaligned closures are not rigidly constrained yet.
    pub fn get_anchored_section_ids(&self) -> Result<Vec<SectionId>, String> {
        // Serialize with command execution to avoid observing intermediate batch state
        let _cmd_guard = self
            .command_lock
            .lock()
            .map_err(|_| "Failed to acquire command lock".to_string())?;
        self.with_project(|project| {
            // Filter to existing sections (stale merges/closures may reference deleted sections)
            let existing: std::collections::HashSet<SectionId> =
                project.sections.iter().map(|s| s.id).collect();
            let mut anchored = std::collections::HashSet::new();
            for zd in &project.zero_datums {
                let connected = project.get_rigid_connected_section_ids(zd.station_ref.section_id);
                anchored.extend(connected.into_iter().filter(|id| existing.contains(id)));
            }
            let mut result: Vec<SectionId> = anchored.into_iter().collect();
            result.sort_unstable();
            Ok(result)
        })
    }
}
