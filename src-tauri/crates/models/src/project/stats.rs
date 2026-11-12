use std::cmp::Ordering;

use crate::{GpsBounds, Station};

use super::{Project, ProjectMetadata, ProjectStats};

impl Project {
    /// Calculate project statistics
    ///
    /// The station-derived figures - `min_depth`, `max_depth` and
    /// `penetration_distance` - come back as 0.0: they need the computed
    /// stations, which this shot-only walk does not have. The caller fills them
    /// from [`Self::depth_range`] and [`Self::calculate_penetration_with`].
    pub fn stats(&self) -> ProjectStats {
        let mut shot_count = 0;
        let mut total_length = 0.0;
        let mut first_survey_date = None;
        let mut last_survey_date = None;

        for section in &self.sections {
            let section_stats = section.stats();
            shot_count += section_stats.shot_count;
            total_length += section_stats.total_length;
            for date in section.shots.iter().filter_map(|s| s.credit().date) {
                first_survey_date = Some(first_survey_date.map_or(date, |d: chrono::NaiveDate| d.min(date)));
                last_survey_date = Some(last_survey_date.map_or(date, |d: chrono::NaiveDate| d.max(date)));
            }
        }

        // Station count = shots + 1 per section (first station)
        let station_count = shot_count + self.sections.len();

        // Loop count = number of detected loop closures
        let loop_count = self.loop_closures.len();

        ProjectStats {
            section_count: self.sections.len(),
            shot_count,
            total_length,
            min_depth: 0.0, // Calculated separately with stations
            max_depth: 0.0, // Calculated separately with stations
            station_count,
            loop_count,
            penetration_distance: 0.0, // Calculated separately with stations
            first_survey_date,
            last_survey_date,
        }
    }

    /// Absolute depth range `(min, max)` over the computed stations.
    ///
    /// Station z is ABSOLUTE - the average of the surrounding shots'
    /// `depth_out`/`depth_in` (`compute_station_depth`) - never a per-section
    /// delta walk seeded at 0, which is what the project stats used to report
    /// and what disagreed with every depth the UI shows (#481). Empty input is
    /// `(0.0, 0.0)`, matching the frontend's own fallback.
    ///
    /// Associated fn with no `&self` - it is a function of the stations alone.
    /// The caller that holds the stations fills the result onto
    /// [`ProjectStats`], next to the `penetration_distance` it computes via
    /// [`Self::calculate_penetration_with`].
    pub fn depth_range(stations: &[Station]) -> (f64, f64) {
        if stations.is_empty() {
            return (0.0, 0.0);
        }

        stations
            .iter()
            .fold((f64::MAX, f64::MIN), |(min_depth, max_depth), station| {
                (
                    min_depth.min(station.coordinates.z),
                    max_depth.max(station.coordinates.z),
                )
            })
    }

    /// Get metadata for this project
    pub fn metadata(&self) -> Option<ProjectMetadata> {
        let file_path = self.file_path.clone()?;
        let stats = self.stats();

        // Compute GPS center and bounds from zero datums
        let (gps_center, gps_bounds) = self.compute_gps_info();

        Some(ProjectMetadata {
            name: self.name.clone(),
            location: self.location.clone(),
            file_path,
            modified_at: self.modified_at,
            section_count: stats.section_count,
            total_length: stats.total_length,
            station_count: stats.station_count,
            gps_center,
            gps_bounds,
            // [Track B] (#189) is_remote is DERIVED from the on-disk speleo_link
            // table (load_metadata), which this in-memory Project does not carry.
            // The recent-projects listing goes through Database::load_metadata
            // (the authoritative probe); this in-memory metadata() path conserves
            // the prior behaviour with a safe default.
            is_remote: false,
            // Same rationale (#201): the linked project UUID lives in the on-disk
            // speleo_link, not this in-memory Project, so None here.
            speleodb_project_id: None,
        })
    }

    /// Compute GPS center and bounds from zero datums
    /// Returns (center, bounds) where center is from first valid zero datum
    /// and bounds encompass all valid zero datums.
    /// Returns (None, None) if no valid datums exist.
    fn compute_gps_info(&self) -> (Option<(f64, f64)>, Option<GpsBounds>) {
        // Filter to valid datums only to prevent serialization issues
        let valid_datums: Vec<_> = self
            .zero_datums
            .iter()
            .filter(|zd| zd.gps.is_valid())
            .collect();

        if valid_datums.is_empty() {
            return (None, None);
        }

        // Center from first valid zero datum (lng, lat order for MapLibre).
        //
        // Positional over VALID, deliberately not the frame rule
        // (`Project::frame_datum`, the first datum that RESOLVES): this is the
        // "roughly where in the world is this project" pin for the project list
        // and the GlobalView map, computed from GPS alone and never combined with
        // a local coordinate. A dangling datum still says where the cave is, so
        // dropping it here would lose the pin without making any number truer.
        let first_gps = &valid_datums[0].gps;
        let gps_center = Some((first_gps.longitude, first_gps.latitude));

        // Bounds from all valid zero datums with dateline-aware longitude handling
        let mut south = f64::INFINITY;
        let mut north = f64::NEG_INFINITY;

        // Collect all longitudes for dateline-aware calculation
        let mut longitudes: Vec<f64> = Vec::with_capacity(valid_datums.len());

        for zd in &valid_datums {
            south = south.min(zd.gps.latitude);
            north = north.max(zd.gps.latitude);
            longitudes.push(zd.gps.longitude);
        }

        // Calculate longitude bounds with dateline awareness
        // Sort longitudes and find the largest gap - the bounds are on the opposite side
        let (west, east) = Self::compute_longitude_bounds(&longitudes);

        let gps_bounds = Some(GpsBounds {
            north,
            south,
            east,
            west,
        });

        (gps_center, gps_bounds)
    }

    /// Compute longitude bounds with dateline awareness
    /// Returns (west, east) where east < west indicates crossing the dateline
    pub fn compute_longitude_bounds(longitudes: &[f64]) -> (f64, f64) {
        if longitudes.is_empty() {
            return (0.0, 0.0);
        }
        if longitudes.len() == 1 {
            return (longitudes[0], longitudes[0]);
        }

        // Sort longitudes
        let mut sorted = longitudes.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

        // Find the largest gap between consecutive longitudes (including wrap-around)
        let mut max_gap = 0.0f64;
        let mut max_gap_end_idx = 0;

        for i in 0..sorted.len() {
            let next_idx = (i + 1) % sorted.len();
            let gap = if next_idx == 0 {
                // Wrap-around gap from last to first (crossing -180/180)
                (sorted[0] + 360.0) - sorted[sorted.len() - 1]
            } else {
                sorted[next_idx] - sorted[i]
            };

            if gap > max_gap {
                max_gap = gap;
                max_gap_end_idx = next_idx;
            }
        }

        // The bounds are on the opposite side of the largest gap
        // west is at max_gap_end_idx, east is at (max_gap_end_idx - 1 + len) % len
        let west = sorted[max_gap_end_idx];
        let east_idx = (max_gap_end_idx + sorted.len() - 1) % sorted.len();
        let east = sorted[east_idx];

        (west, east)
    }
}
