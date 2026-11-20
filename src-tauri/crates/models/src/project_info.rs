//! Project-level payloads: the Recents listing's metadata, the statistics
//! panel, the distance-gradient field and the ruler.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{GpsBounds, LineId, ShotId, StationRef};

/// Apply the Ariane `.tml`/`.tmlu` no-data convention to a raw
/// `firstStartAbsoluteElevation` value: Ariane encodes "no elevation
/// recorded" as exactly `0.0`, so an exact zero is treated as absent here
/// (see `Project::surface_elevation`'s doc comment above). This is
/// format-forced, not a design choice: a genuine entrance at exactly 0.0 m
/// above mean sea level is not representable, while any near-zero but
/// nonzero value passes through unchanged.
pub fn ariane_elevation_or_absent(raw: f64) -> Option<f64> {
    (raw.abs() > f64::EPSILON).then_some(raw)
}

/// Project metadata for listing
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProjectMetadata {
    pub name: String,
    pub location: Option<String>,
    #[ts(type = "string")]
    pub file_path: PathBuf,
    #[ts(type = "string")]
    pub modified_at: DateTime<Utc>,
    pub line_count: usize,
    /// Surveyed length, Reference shots excluded (#689). The on-disk listing
    /// computes it in SQL (`Database::load_metadata`) with the same rule as
    /// [`ProjectStats::total_length`], so an opened project reports the same
    /// figure. For a file not yet opened since the shot-kind backfill (#27) it
    /// is an upper bound until its first open, which classifies the migrated
    /// virtual shots and re-saves them.
    pub total_length: f64,
    /// Number of stations in the project (shots + 1 per line)
    pub station_count: usize,
    /// GPS center from first zero datum (lng, lat) - MapLibre order
    pub gps_center: Option<(f64, f64)>,
    /// GPS bounds including all zero datums
    pub gps_bounds: Option<GpsBounds>,
    // [Track B] (#189, D-B14) Whether this `.stalx` is a pulled SpeleoDB project.
    // DERIVED from EXISTS(speleo_link) in load_metadata, never a stored column;
    // tolerant of a missing table on a read-only metadata open.
    pub is_remote: bool,
    /// The linked SpeleoDB project UUID (from `speleo_link.project_id`), or `None`
    /// for a local project. Lets the recents/GlobalView list cross-reference a
    /// single `GET /projects/` lock scan to badge each entry's lock state (#201).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub speleodb_project_id: Option<String>,
}

/// Per-station along-cave distance field from a chosen origin (or the zero-datum
/// entrance default). `distances[i]` corresponds to `station_refs[i]`; `None`
/// marks an unreachable station (JSON can't carry infinity). `max_reachable` is
/// the largest finite distance (0.0 if nothing is reachable), for auto-scaling
/// the gradient legend. Backs the distance-gradient overlay (#34); reuses the
/// shared reachability graph. Since #27 that is literally true: jumps are in
/// (they are swum), references are out (they are not) unless the
/// `gas.include_reference_legs` opt-in is set - see `build_survey_graph`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DistanceField {
    pub distances: Vec<Option<f64>>,
    pub station_refs: Vec<StationRef>,
    pub max_reachable: f64,
}

/// One end of a ruler measurement (#26): either a **station** (set
/// `station_index`; leave `shot_id`/`t` null) or an exact **point on a shot** at
/// fraction `t` ∈ [0,1] from the shot's from-station toward its to-station (set
/// `shot_id` + `t`). `t` ≈ 0 / ≈ 1 collapses to the from/to station.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct RulerEndpoint {
    pub line_id: LineId,
    #[serde(default)]
    pub station_index: Option<u32>,
    #[serde(default)]
    pub shot_id: Option<ShotId>,
    #[serde(default)]
    pub t: Option<f64>,
}

/// A shot on the along-line path, identified for tinting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PathShot {
    pub line_id: LineId,
    pub shot_id: ShotId,
}

/// Result of an along-cave-line ruler query (#26). `along_line` is `None` when the
/// two endpoints lie in different connected components (no path). `path_shots`
/// are the shots traversed from A toward B - whole shots, even when an endpoint
/// only partially crosses one - with zero-cost merge/closure ties skipped (they
/// aren't shots and must not be tinted). Uses the same shots+merges+closures
/// graph as the distance-gradient overlay (#34), so it matches the Display
/// "line distance" view.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AlongLineResult {
    pub along_line: Option<f64>,
    pub path_shots: Vec<PathShot>,
    /// Stations traversed along the path (the chain nodes), in order from A toward
    /// B. Shot-point endpoints contribute their adjacent stations but not a
    /// station of their own. Tinted alongside `path_shots`.
    pub path_stations: Vec<StationRef>,
}

/// Overall project statistics
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProjectStats {
    pub line_count: usize,
    /// Every shot, Reference shots included.
    pub shot_count: usize,
    /// Surveyed length: the sum of [`crate::LineStats::total_length`], which leaves
    /// out Reference shots (#689).
    pub total_length: f64,
    /// Shallowest absolute station z. Filled by the caller from
    /// [`Project::depth_range`], never by [`Project::stats`] (#481).
    pub min_depth: f64,
    /// Deepest absolute station z. Same source as
    /// [`min_depth`](Self::min_depth).
    pub max_depth: f64,
    pub station_count: usize,
    pub loop_count: usize,
    /// Maximum straight-line distance from zero datum to any station
    pub penetration_distance: f64,
    /// The earliest survey date of any shot; absent when no shot has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "string")]
    pub first_survey_date: Option<chrono::NaiveDate>,
    /// The latest survey date of any shot; absent when no shot has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "string")]
    pub last_survey_date: Option<chrono::NaiveDate>,
}
