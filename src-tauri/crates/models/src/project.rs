use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use ts_rs::TS;

use super::{
    AzimuthReference, GpsBounds, LoopClosure, Section, SectionData, SectionId, SectionUpdates,
    ShotData, ShotId, StationAnchor, StationKey, StationMerge, StationMergeWire, StationRef,
    ZeroDatum,
};
use super::{LoopClosureWire, ZeroDatumWire};

// Re-export the sibling module so the relocated `suggest_orphan_placements` (now in
// the `distance` child) keeps resolving `super::placement_suggest` - its `super` is
// now `project`, not `models`.
use super::placement_suggest;

mod closure;
mod coordinates;
mod distance;
// Crate-visible only so the `.plnx` load path can reach the engine's segment-speed bounds
// (`MIN/MAX_SEGMENT_SPEED_M_PER_MIN`) and rank stored speeds on the same domain the engine
// honours (#317). Everything else in here stays behind the `pub use` list below.
pub mod gas;
mod lsq;
mod pathfinding;

// Used only by the `tests` module via `use super::*`; production code in this
// residual file no longer references them after the Phase 5.2/5.3 cluster moves.
#[cfg(test)]
use super::{Coordinates, GpsCoordinates, Shot, ShotUpdates, Station};
#[cfg(test)]
use crate::config::ClosureAdjustmentConfig;

mod adjustment_report;
mod classify;
mod closure_shots;
mod combine_candidate;
mod combine_plan;
mod combine_sections;
mod connectivity;
mod gauge;
mod mutations;
mod shots;
mod split_section;
mod station_rekey;

mod integrity;
mod merge_topology;
mod offsets;
mod relationships;
mod reverse;
mod stats;
mod union_find;
mod uuid_unique;

#[cfg(test)]
mod anchor_fuzz;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod closure_shots_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test module: panics on a failed assumption are the point, and the fuzz sweeps \
              print what they covered and the replay prints each step"
)]
mod combine_closure_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod combine_sections_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod component_gauge_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod split_section_tests;

pub use adjustment_report::{
    AdjustedStations, AdjustmentFailure, AdjustmentFailureReport, AdjustmentReport, SkippedClosure,
};
pub use classify::ClassifyScope;
pub use closure_shots::{station_id_label, ClosureAcrossOneShot, ClosureTieRefusal};
pub use combine_candidate::{COMBINE_DEPTH_NOTICE_M, COMBINE_TOLERANCE_M, MAX_COMBINE_PARTNERS};
pub use gas::{
    CylRole, Cylinder, Diver, GasReachField, PlanMode, PlanSegment, RouteLeg, RouteProfilePoint,
    TankMaterial, Team,
};
pub use gauge::HeldGauge;
pub use integrity::{IntegrityViolation, ViolationSeverity};
pub use merge_topology::MergeTopology;
pub use reverse::{reverse_shots, shot_run_displacement};
pub use shots::MIN_SPLIT_CHILD_LENGTH_M;
pub use uuid_unique::{
    live_shot_uuid_for_stored, remap_project_json, reminted_shot_uuid, reminted_shot_uuid_avoiding,
    stored_shot_uuid_matches, ShotUuidRemap, SHOT_UUID_REMINT_NAMESPACE,
};

/// Relationships that reference a given section (merges, closures, offsets, zero datums).
/// Used by the frontend to show affected relationships before section deletion.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SectionRelationships {
    /// Station merges referencing the section (positional wire form)
    pub merges: Vec<StationMergeWire>,
    /// Loop closures referencing the section (positional wire form)
    pub closures: Vec<LoopClosureWire>,
    /// Whether the section has a section offset
    pub has_section_offset: bool,
    /// Number of station offsets referencing the section
    pub station_offset_count: usize,
    /// Number of zero datums referencing the section
    pub zero_datum_count: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct StationPair {
    pub station_a: StationRef,
    pub station_b: StationRef,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct ClosurePair {
    pub closure_id: u64,
    pub station_a: StationRef,
    pub station_b: StationRef,
}

#[derive(Debug, Clone, Copy)]
enum PathEdge {
    Shot {
        section_id: SectionId,
        shot_index: usize,
        forward: bool,
    },
    Merge,
}

#[derive(Debug, Clone, Copy)]
struct PathStep {
    to: StationRef,
    edge: PathEdge,
}

#[derive(Debug, Clone)]
struct StationPath {
    start: StationRef,
    steps: Vec<PathStep>,
}

#[derive(Debug, Clone, Copy)]
struct HeapItem {
    cost: f64,
    node: StationRef,
}

impl Eq for HeapItem {}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse for min-heap; treat NaN as equal (should not occur).
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct ClosureUpdate {
    pub closure_id: u64,
    pub old_station: StationRef,
    pub new_station: StationRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DeleteShotImpact {
    pub removed_station: StationRef,
    pub removed_merges: Vec<StationPair>,
    pub removed_closures: Vec<ClosurePair>,
    pub replacement_merges: Vec<StationPair>,
    pub replacement_closure_updates: Vec<ClosureUpdate>,
    /// Closures broken by the deletion: the surviving endpoints are no longer
    /// connected through the survey, so the closure is replaced by a plain
    /// station merge instead of being kept as a (false) loop. See issue #8.
    #[serde(default)]
    pub closure_to_merge: Vec<StationPair>,
    pub merge_rewire_skipped: bool,
    pub closure_rewire_skipped: bool,
    #[serde(default)]
    pub shift_blocked: bool,
    #[serde(default)]
    pub shift_blocked_reasons: Vec<String>,
}

/// Which structural case a contiguous shot-range deletion falls into. Computed
/// from the range endpoints relative to the section's shot count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RangeDeleteKind {
    /// The range covers every shot - the whole section is removed.
    WholeSection,
    /// The range starts at the first shot (nothing before); the survivors keep
    /// their absolute position by an offset shift.
    Prefix,
    /// The range ends at the last shot (nothing after); the head is truncated.
    Suffix,
    /// Shots remain both before and after - the section can be split (A=before,
    /// B=after) or gap-closed (shift).
    Interior,
}

/// Aggregate impact of deleting a contiguous range of shots `[start_pos..=end_pos]`
/// from one section. The merge/closure fields mirror [`DeleteShotImpact`] (so the
/// frontend can render them with the same component); `kind` drives the dialog
/// routing and `shift_blocked` reports whether the gap-close path is available.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DeleteShotRangeImpact {
    pub section_id: SectionId,
    pub start_pos: u32,
    pub end_pos: u32,
    pub kind: RangeDeleteKind,
    /// Stations any offered action removes. For an interior range this is the
    /// gap-close (shift) set `(start_pos+1 ..= end_pos+1)` - the superset that
    /// also covers the split's interior set - so the relationship panel never
    /// hides what closing the gap touches. (Prefix/suffix/whole remove exactly
    /// the gap-close set; only an interior split keeps the boundary `end_pos+1`.)
    pub removed_stations: Vec<StationRef>,
    pub removed_merges: Vec<StationPair>,
    pub removed_closures: Vec<ClosurePair>,
    pub replacement_merges: Vec<StationPair>,
    pub replacement_closure_updates: Vec<ClosureUpdate>,
    #[serde(default)]
    pub closure_to_merge: Vec<StationPair>,
    pub merge_rewire_skipped: bool,
    pub closure_rewire_skipped: bool,
    /// True when the gap-close (shift) path is unavailable for this range (the
    /// two sides are already connected or both anchored, or the gap-close drops
    /// a zero datum).
    #[serde(default)]
    pub shift_blocked: bool,
    #[serde(default)]
    pub shift_blocked_reasons: Vec<String>,
    /// Zero datums the SPLIT would drop (anchored on a strictly-interior station
    /// the split removes) - surfaced so the dialog can warn. The gap-close path
    /// is instead blocked when it would drop a datum, so this set is split-only.
    #[serde(default)]
    pub removed_datums: Vec<StationRef>,
    /// Station comments the SPLIT would drop (anchored on a strictly-interior
    /// station the split removes) - surfaced so the dialog can warn before the
    /// annotation disappears.
    #[serde(default)]
    pub removed_comments: Vec<StationRef>,
}

/// A complete cave survey project
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Project {
    /// Project name (typically cave system name)
    pub name: String,
    /// Optional location description
    pub location: Option<String>,
    /// Creation timestamp
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp
    #[ts(type = "string")]
    pub modified_at: DateTime<Utc>,
    /// All sections in this project
    pub sections: Vec<Section>,
    /// Station merges. Stored with position-independent [`StationKey`] endpoints;
    /// the TS/frontend shape is the positional wire form (resolved at IPC encode).
    /// Reads like a `[StationMerge]` slice; [`MergeTopology`] additionally owns
    /// the merge-only section adjacency memo and keeps it honest across every
    /// mutation (serde shape is unchanged - the bare merge array).
    #[ts(type = "Array<StationMerge>")]
    pub station_merges: MergeTopology,
    /// Zero datums
    #[ts(type = "Array<ZeroDatum>")]
    pub zero_datums: Vec<ZeroDatum>,
    /// Detected loop closures
    #[serde(default)]
    #[ts(type = "Array<LoopClosure>")]
    pub loop_closures: Vec<LoopClosure>,
    /// File path if saved (None for new unsaved projects)
    #[ts(type = "string | null")]
    pub file_path: Option<PathBuf>,
    /// Next section ID to assign
    pub next_section_id: SectionId,
    /// Section offsets (x, y) applied when sections are merged/aligned
    #[serde(default, serialize_with = "serialize_section_offsets")]
    pub section_offsets: HashMap<SectionId, (f64, f64)>,
    /// Station offsets (x, y, z) applied when individual stations are moved
    #[serde(
        default,
        serialize_with = "serialize_station_offsets",
        deserialize_with = "deserialize_station_offsets"
    )]
    #[ts(skip)]
    pub station_offsets: HashMap<StationKey, (f64, f64, f64)>,
    /// Station comments keyed by StationRef
    #[serde(
        default,
        serialize_with = "serialize_station_texts",
        deserialize_with = "deserialize_station_texts"
    )]
    #[ts(skip)]
    pub station_comments: HashMap<StationKey, String>,
    /// Station names: a short free-text identifier per station, such as the
    /// station number an MNemo dump gave it (a DMP import records it here).
    /// Not unique - MNemo numbers restart at 0 in every section - so it is
    /// always shown next to the positional `S:I` id, never instead of it.
    /// Stored in its own `station_names` table so readers that know only the
    /// v18 tables (OpenSpeleo's Python `stalagmight_lib`, which SpeleoDB maps
    /// with - see `station_texts` in the persistence crate) are unaffected.
    #[serde(
        default,
        serialize_with = "serialize_station_texts",
        deserialize_with = "deserialize_station_texts"
    )]
    #[ts(skip)]
    pub station_names: HashMap<StationKey, String>,
    /// SHA256 hashes of imported DMP files (for duplicate detection)
    #[serde(default)]
    pub imported_file_hashes: Vec<String>,
    /// Content hashes of imported DMP sections (for section-level duplicate detection)
    #[serde(default)]
    pub imported_section_hashes: Vec<String>,
    /// Monotonic counter for closure IDs. Never reused, even after deletions.
    #[serde(default)]
    pub next_closure_id: u64,
    /// Per-project validation thresholds (#57). Stored in the `.stalx` so they
    /// travel with the file. New projects are seeded from
    /// the global config (the "defaults for new projects"); thereafter this is
    /// authoritative. Defaults on legacy projects until populated on load.
    #[serde(default)]
    pub validation: crate::config::ValidationConfig,

    /// Entrance elevation (#431): +up, metres above mean sea level.
    /// `None` = unknown. Project-level rather than per-entrance (#431): the
    /// format carries one elevation per file regardless of entrance count,
    /// and for entrances joined through the cave the relative elevation is
    /// already derived from the survey geometry, so a per-entrance field
    /// would be a second source of truth. Set from
    /// an Ariane import's `firstStartAbsoluteElevation` (never clobbered by a
    /// later import once set) and read back by TML export; nothing else
    /// consumes it, and the geodesy conversion (`models::geo::gps_to_local`)
    /// deliberately ignores it.
    #[serde(default)]
    pub surface_elevation: Option<f64>,

    /// The north reference a section inherits when it declares none (#438).
    ///
    /// `None` - the value on every project that predates the field, and on every
    /// project whose sections all declare their own - resolves to
    /// [`AzimuthReference::default`], "nothing recorded", correction 0. So this
    /// being absent is exactly the pre-#438 geometry.
    ///
    /// The per-SECTION field is the authoritative one (the formats declare
    /// declination per survey); this exists so a user-level "this project is
    /// true north" policy has somewhere to live without touching sections that
    /// carry a file-declared value of their own.
    #[serde(default)]
    #[ts(optional)]
    pub default_azimuth_reference: Option<AzimuthReference>,

    /// Shots whose RECORD orientation runs against their section's
    /// [`Section::reverse_parity`], by uuid: a shot's record parity is its
    /// section's parity, flipped when its uuid is here
    /// ([`Self::shot_record_parity`]).
    ///
    /// A parity flips with every reverse of the record, which is what a
    /// single-neighbour dive-plan pin reads to know which end of its shot it
    /// sits on. While every shot of a section shares the section's parity, the
    /// section's flag says it all. Combine Sections breaks that: the other
    /// section's shots join a section whose parity can differ from theirs, and
    /// they are recorded here (toggled) so every shot keeps its own. Because
    /// this lives on the project, every undo snapshot carries it with the
    /// shots, so an undo puts the shots and their orientation back together.
    ///
    /// Persisted in `project_settings` (no schema bump); omitted from serde
    /// when empty, so a project that never combined serializes as before.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    #[ts(skip)]
    pub counter_parity_shots: BTreeSet<String>,

    /// Stations that hold their component fixed in the least-squares
    /// adjustment in place of the lowest-id section's gauge station
    /// (`default_anchor_for_component` reads them first, in order): a Split
    /// Section Here, Combine Sections or delete-and-split kept a component's
    /// gauge on its physical station, and the section that anchors the
    /// component afterwards does not hold that station (see `project/gauge.rs`).
    /// Named by shot uuids, so they follow their shots through later edits.
    ///
    /// Part of the project, so every undo snapshot carries it. Persisted in
    /// `project_settings` (no schema bump), pruned to the entries that still
    /// name a station on save and on load; omitted from serde when empty. An
    /// older build ignores it and falls back to the lowest-id rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub held_gauges: Vec<HeldGauge>,

    /// Transient flag (NOT persisted) - when true,
    /// [`Self::cascade_connection_offsets`] returns immediately. Used by
    /// batch importers to skip the per-call O(N) cascade and run a
    /// single cascade after all sections / merges are in place. Callers
    /// must set the flag, run their batch, clear the flag, and then
    /// invoke `cascade_connection_offsets` themselves to leave the
    /// project in a consistent state.
    #[serde(skip, default)]
    #[ts(skip)]
    pub cascade_deferred: bool,

    /// Section-id → `sections[i]` index cache (transient, NOT persisted).
    /// Lookups via [`Self::get_section`] / [`Self::get_section_mut`]
    /// consult this map first to avoid the O(N) linear scan that used to
    /// dominate `merge_stations` and `compute_station_raw_coords` on
    /// large imports (~4 lookups per merge × 14 k merges = ~400 M ops).
    /// Maintained incrementally by `add_section` / `remove_section`.
    /// After deserialization the map is empty and a linear fallback
    /// kicks in - callers that hammer the lookup (e.g. the batch
    /// importer) should call [`Self::rebuild_section_index`] first.
    #[serde(skip, default)]
    #[ts(skip)]
    pub section_index_cache: HashMap<SectionId, usize>,

    /// `(section_id, shot_uuid)` → shot vector index. Transient, NOT persisted.
    /// Turns `resolve_key`'s `AfterShot` arm from an O(shots) linear scan with a
    /// per-element UUID string compare into an O(1) lookup - the dominant cost
    /// when building merge/closure connectivity over a large import (issue:
    /// importing a big Walls project pinned one core for minutes). Populated only
    /// by [`Self::rebuild_shot_uuid_index`] (batch import, post-load, snapshot
    /// restore); empty after deserialization, where `resolve_key` falls back to
    /// the linear scan. `resolve_key` re-checks the cached index against the
    /// shot's UUID, so a stale entry is harmless (it falls back to the scan).
    #[serde(skip, default)]
    #[ts(skip)]
    pub shot_uuid_index: HashMap<(SectionId, String), u32>,

    /// Fingerprint of a merge residual the offset solver has already proven it
    /// cannot reduce. Transient, NOT persisted. Read and written by exactly one
    /// function, [`Self::realign_merges`], which skips its re-solve while the
    /// live state still fingerprints identically - see the doc comment there for
    /// what the fingerprint covers and why the skip cannot hide a healable drift.
    ///
    /// Never persisted and never compared: it is out of the derived serde (so no
    /// snapshot, `.stalx` or IPC payload can see it), and `Project` has no
    /// `PartialEq`, so no test or oracle compares it either. A `Clone` carries it,
    /// which is correct rather than merely tolerable - the clone IS the state the
    /// fingerprint was taken of, so it stays as valid for the copy as for the
    /// original. A deserialized project starts with `None` (no skip until this
    /// build proves a fixpoint of its own), and the load path re-derives offsets
    /// through [`Self::resolve_offsets`], which does not consult this at all.
    #[serde(skip, default)]
    #[ts(skip)]
    pub realign_fixpoint: Option<offsets::RealignFixpoint>,
}

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
    pub section_count: usize,
    /// Surveyed length, Reference shots excluded (#689). The on-disk listing
    /// computes it in SQL (`Database::load_metadata`) with the same rule as
    /// [`ProjectStats::total_length`], so an opened project reports the same
    /// figure. For a file not yet opened since the shot-kind backfill (#27) it
    /// is an upper bound until its first open, which classifies the migrated
    /// virtual shots and re-saves them.
    pub total_length: f64,
    /// Number of stations in the project (shots + 1 per section)
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
    pub section_id: SectionId,
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
    pub section_id: SectionId,
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
    pub section_count: usize,
    /// Every shot, Reference shots included.
    pub shot_count: usize,
    /// Surveyed length: the sum of [`crate::SectionStats::total_length`], which leaves
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

/// # THE DETERMINISTIC-SERIALIZATION RULE FOR THE HASHMAPS
///
/// `section_offsets`, `station_offsets`, `station_comments` and `station_names`
/// are `HashMap`s,
/// and a `HashMap`'s iteration order is a function of its RandomState seed and
/// its insertion history - NOT of its contents. Emitting them in iteration order
/// makes the serialized `Project` a function of how the map was built rather
/// than of what the project IS, and two things read that output as an identity:
///
/// * the whole-project undo snapshots (`UndoData::ProjectSnapshotPair`, written
///   by `serde_json::to_string(project)` and regenerated on every undo/redo), so
///   a state that returned exactly where it started produced a DIFFERENT blob;
/// * the `.stalx` save, so a re-save of an untouched project rewrote permuted
///   bytes and re-rolled the save/load fixpoint.
///
/// So every one of them sorts its keys before it emits them. Ascending by
/// section id for `section_offsets`; by [`station_key_order`] for the
/// `StationKey`-keyed maps, whose entries are emitted as a `Vec` of pairs anyway
/// (serde_json cannot use a struct as an object key). The DESERIALIZE side is
/// untouched: order was never load-bearing on the way in, and both shapes
/// (object / array of pairs) are unchanged, so this is not a format change.
fn serialize_section_offsets<S: serde::Serializer>(
    map: &HashMap<SectionId, (f64, f64)>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let ordered: BTreeMap<SectionId, (f64, f64)> = map.iter().map(|(k, v)| (*k, *v)).collect();
    ordered.serialize(serializer)
}

/// Total order over [`StationKey`], for the maps keyed by one. Section
/// first, then the anchor variant, then the variant's payload. Distinct keys
/// never tie - a key IS its section plus its anchor - so this is a total order
/// on any map's key set, which is what makes the sorted output a function of the
/// contents alone. (`StationKey` deliberately has no `Ord` of its own: nothing
/// else in the model ranks stations, and an order that only serialization needs
/// belongs next to it.)
fn station_key_order(key: &StationKey) -> (SectionId, u8, &str, u32) {
    match &key.anchor {
        StationAnchor::SectionStart => (key.section_id, 0, "", 0),
        StationAnchor::AfterShot(uuid) => (key.section_id, 1, uuid.as_str(), 0),
        StationAnchor::Index(index) => (key.section_id, 2, "", *index),
    }
}

/// Serialize station_offsets as a Vec of (StationRef, (f64, f64, f64)) tuples
/// to avoid serde_json's requirement that map keys be strings. Key-ordered - see
/// [`serialize_section_offsets`].
fn serialize_station_offsets<S: serde::Serializer>(
    map: &HashMap<StationKey, (f64, f64, f64)>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut entries: Vec<(&StationKey, &(f64, f64, f64))> = map.iter().collect();
    entries.sort_by(|(a, _), (b, _)| station_key_order(a).cmp(&station_key_order(b)));
    entries.serialize(serializer)
}

/// Deserialize station_offsets from a Vec of (StationRef, (f64, f64, f64)) tuples.
fn deserialize_station_offsets<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<StationKey, (f64, f64, f64)>, D::Error> {
    let entries: Vec<(StationKey, (f64, f64, f64))> = Vec::deserialize(deserializer)?;
    Ok(entries.into_iter().collect())
}

/// Serialize a station-keyed text map (`station_comments`, `station_names`) as
/// a Vec of (StationKey, String) tuples. Key-ordered - see
/// [`serialize_section_offsets`].
fn serialize_station_texts<S: serde::Serializer>(
    map: &HashMap<StationKey, String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut entries: Vec<(&StationKey, &String)> = map.iter().collect();
    entries.sort_by(|(a, _), (b, _)| station_key_order(a).cmp(&station_key_order(b)));
    entries.serialize(serializer)
}

/// Deserialize a station-keyed text map from a Vec of (StationKey, String) tuples.
fn deserialize_station_texts<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<StationKey, String>, D::Error> {
    let entries: Vec<(StationKey, String)> = Vec::deserialize(deserializer)?;
    Ok(entries.into_iter().collect())
}

/// IPC serialization view of [`Project`]. The stored `Project` keeps its
/// `StationKey`-based serde (lossless, required by the undo snapshot path), but
/// the frontend and the ts-rs `Project` type expect the three cross-reference
/// collections in positional wire form (`StationRef { section_id, station_index }`).
/// This wrapper forwards every ts-exported field and re-encodes those three to
/// wire, dropping dangling refs (the same resolution the binary load path's
/// `ProjectHeaderBinary` does). `#[ts(skip)]` fields on `Project` - the
/// `StationKey`-keyed `station_offsets`/`station_comments`/`station_names` and the transient
/// caches - are intentionally omitted to match the generated `Project` TS type.
struct ProjectIpc<'a>(&'a Project);

impl Serialize for ProjectIpc<'_> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let p = self.0;
        let station_merges: Vec<_> = p
            .station_merges
            .iter()
            .filter_map(|m| p.merge_to_wire(m))
            .collect();
        let zero_datums: Vec<_> = p
            .zero_datums
            .iter()
            .filter_map(|z| p.datum_to_wire(z))
            .collect();
        let loop_closures: Vec<_> = p
            .loop_closures
            .iter()
            .filter_map(|c| p.closure_to_wire(c))
            .collect();
        // Key-ordered, for the same reason the stored serde is (see
        // `serialize_section_offsets`): this writer emits the field itself rather
        // than through the field attribute, so it would otherwise be the one path
        // left where the same project produces two different payloads.
        let section_offsets: BTreeMap<SectionId, (f64, f64)> =
            p.section_offsets.iter().map(|(k, v)| (*k, *v)).collect();
        // Field set + order MUST stay in lockstep with the generated Project.ts
        // (17 ts-exported fields). Adding a ts-exported field to `Project`
        // requires adding it here too, or it silently drops off the IPC payload.
        let mut st = serializer.serialize_struct("Project", 17)?;
        st.serialize_field("name", &p.name)?;
        st.serialize_field("location", &p.location)?;
        st.serialize_field("created_at", &p.created_at)?;
        st.serialize_field("modified_at", &p.modified_at)?;
        st.serialize_field("sections", &p.sections)?;
        st.serialize_field("station_merges", &station_merges)?;
        st.serialize_field("zero_datums", &zero_datums)?;
        st.serialize_field("loop_closures", &loop_closures)?;
        st.serialize_field("file_path", &p.file_path)?;
        st.serialize_field("next_section_id", &p.next_section_id)?;
        st.serialize_field("section_offsets", &section_offsets)?;
        st.serialize_field("imported_file_hashes", &p.imported_file_hashes)?;
        st.serialize_field("imported_section_hashes", &p.imported_section_hashes)?;
        st.serialize_field("next_closure_id", &p.next_closure_id)?;
        st.serialize_field("validation", &p.validation)?;
        st.serialize_field("surface_elevation", &p.surface_elevation)?;
        st.serialize_field("default_azimuth_reference", &p.default_azimuth_reference)?;
        st.end()
    }
}

/// `#[serde(serialize_with = "...")]` target for every IPC struct that carries a
/// `Project` to the frontend (`ProjectState`, `LoadProjectFullResult`,
/// `CreateProjectFullResult`), so they all emit the positional wire shape the
/// frontend reads rather than the internal `StationKey` shape derived serde emits.
pub fn serialize_project_ipc<S: serde::ser::Serializer>(
    p: &Project,
    s: S,
) -> Result<S::Ok, S::Error> {
    Serialize::serialize(&ProjectIpc(p), s)
}

impl Project {
    /// Create a new empty project
    pub fn new(name: String, location: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            name,
            location,
            created_at: now,
            modified_at: now,
            sections: Vec::new(),
            station_merges: MergeTopology::default(),
            zero_datums: Vec::new(),
            loop_closures: Vec::new(),
            file_path: None,
            next_section_id: 0,
            section_offsets: HashMap::new(),
            station_offsets: HashMap::new(),
            station_comments: HashMap::new(),
            station_names: HashMap::new(),
            imported_file_hashes: Vec::new(),
            imported_section_hashes: Vec::new(),
            next_closure_id: 1,
            validation: crate::config::ValidationConfig::default(),
            surface_elevation: None,
            default_azimuth_reference: None,
            counter_parity_shots: BTreeSet::new(),
            held_gauges: Vec::new(),
            cascade_deferred: false,
            section_index_cache: HashMap::new(),
            shot_uuid_index: HashMap::new(),
            realign_fixpoint: None,
        }
    }

    /// Assign sequential IDs to any loop closures that have id=0 (legacy projects),
    /// and re-derive the alignment claims the file carries.
    /// Updates `next_closure_id` to be greater than all existing IDs.
    ///
    /// The alignment half used to FORCE `is_aligned = true` on every
    /// [`ClosureStrategy::Move`] closure on every open, which is why fixing the
    /// creation stamp alone would not have been enough: a closure honestly saved
    /// as `0` came straight back as `true`, and memory and disk would have
    /// disagreed from that open onward (the #413 finding-10 convergence shape).
    /// It re-MEASURES now, through the one predicate
    /// ([`Self::remeasure_closure_alignment`]), so the round trip converges - and
    /// the load re-measures once more after `redrive_offsets_after_load`'s
    /// zero-trust re-solve, which is the geometry the opened project actually has.
    pub fn assign_closure_ids(&mut self) {
        // First, ensure next_closure_id is at least max(existing) + 1
        let max_id = self.loop_closures.iter().map(|lc| lc.id).max().unwrap_or(0);
        if self.next_closure_id <= max_id {
            self.next_closure_id = max_id + 1;
        }
        // Assign IDs to any closures with id=0
        for lc in &mut self.loop_closures {
            if lc.id == 0 {
                lc.id = self.next_closure_id;
                self.next_closure_id += 1;
            }
        }
        self.remeasure_closure_alignment();
    }

    /// Mark the project as modified
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
    }

    // === Section operations ===

    /// Add a new section to the project
    pub fn add_section(&mut self, data: SectionData) -> &Section {
        let section = Section::new(self.next_section_id, data);
        self.next_section_id += 1;
        let new_idx = self.sections.len();
        self.section_index_cache.insert(section.id, new_idx);
        // Maintain the shot-UUID index incrementally so `resolve_key`'s AfterShot
        // lookup stays O(1) as the batch importer creates sections AND the
        // merges/closures that reference them by shot UUID (the import's hot
        // path). `Section::new` mints a UUID for every shot, so they are all
        // present here.
        for (i, shot) in section.shots.iter().enumerate() {
            if let Some(u) = &shot.uuid {
                // First-occurrence wins: mirror the cold-path `resolve_key`,
                // which uses `shots.iter().position(...)` (first match). Under a
                // duplicated within-section UUID, `or_insert` keeps the earliest
                // index so the warm cache agrees with the cold scan.
                self.shot_uuid_index
                    .entry((section.id, u.clone()))
                    .or_insert(i as u32);
            }
        }
        self.sections.push(section);
        self.touch();
        &self.sections[new_idx]
    }

    /// Get a section by ID. First tries the O(1) `section_index_cache`;
    /// if the cache is stale or empty (e.g. after deserialization), falls
    /// back to an O(N) linear scan so we never return a wrong answer.
    pub fn get_section(&self, section_id: SectionId) -> Option<&Section> {
        if let Some(&i) = self.section_index_cache.get(&section_id) {
            if let Some(s) = self.sections.get(i) {
                if s.id == section_id {
                    return Some(s);
                }
            }
        }
        self.sections.iter().find(|s| s.id == section_id)
    }

    /// Get a mutable reference to a section by ID. Same cache + fallback
    /// strategy as [`Self::get_section`].
    pub fn get_section_mut(&mut self, section_id: SectionId) -> Option<&mut Section> {
        let i = self.section_index_cache.get(&section_id).copied();
        if let Some(i) = i {
            if let Some(s) = self.sections.get(i) {
                if s.id == section_id {
                    return self.sections.get_mut(i);
                }
            }
        }
        self.sections.iter_mut().find(|s| s.id == section_id)
    }

    /// Update a section's metadata
    pub fn update_section(
        &mut self,
        section_id: SectionId,
        updates: &SectionUpdates,
    ) -> Option<&Section> {
        if updates.surveyor.is_some() || updates.date.is_some() {
            // The legacy surveyor/date land on every shot; give each an
            // identity first so an undo can address it after a reload.
            self.get_section(section_id)?;
            self.ensure_section_shot_uuids(section_id);
            self.reindex_section_shot_uuids(section_id);
        }
        let section = self.get_section_mut(section_id)?;
        section.apply_updates(updates);
        self.touch();
        self.get_section(section_id)
    }

    /// Remove a section by ID
    pub fn remove_section(&mut self, section_id: SectionId) -> Option<Section> {
        let pos = self.sections.iter().position(|s| s.id == section_id)?;
        // Removing shifts every later section's index down by one - rather
        // than touching N cache entries, we drop the cache and let the
        // linear fallback rebuild it on demand (removes are rare relative
        // to lookups). Callers running a batch can call
        // `rebuild_section_index()` to restore O(1) lookups afterwards.
        self.section_index_cache.clear();
        self.touch();
        Some(self.sections.remove(pos))
    }

    /// Mint v4 UUIDs for any shot whose `uuid` field is `None` and write
    /// them back into the in-memory project.
    ///
    /// The UUID is the station identity every cross-reference anchors to
    /// (`StationKey::AfterShot`), and it is the `<UUID>` the Ariane export
    /// emits - which is what keeps the `.tml` SpeleoDB renders byte-stable
    /// across re-exports.
    ///
    /// The save flow needs this before either the SQLite INSERT or the TML
    /// export runs. Without it, pre-v6 .stalx files loaded today have
    /// `Shot::uuid = None` everywhere; the SQLite INSERT path mints fresh
    /// UUIDs at write time but does NOT update the in-memory shot, so a
    /// later export in the SAME save would mint a SECOND, different set.
    /// Calling this once at the top of `save_project` guarantees every
    /// consumer receives the same UUIDs.
    pub fn ensure_shot_uuids(&mut self) {
        for section in &mut self.sections {
            for shot in &mut section.shots {
                if shot.uuid.as_deref().map_or(true, str::is_empty) {
                    shot.uuid = Some(uuid::Uuid::new_v4().to_string());
                }
            }
        }
    }

    /// Move every section's LEGACY surveyor/date onto its uncredited shots
    /// ([`Section::absorb_legacy_credit`]). Run wherever a `Project` or
    /// `Section` comes back from JSON written before the credit moved onto
    /// the shots - an undo snapshot, an older history entry - so the people it
    /// named are not silently lost. Returns how many sections changed.
    pub fn absorb_legacy_section_credit(&mut self) -> usize {
        self.sections
            .iter_mut()
            .map(Section::absorb_legacy_credit)
            .filter(|changed| *changed)
            .count()
    }

    /// Mint a UUID for every shot of ONE section that lacks one.
    ///
    /// Section-scoped twin of [`Self::ensure_shot_uuids`], for callers that only
    /// need one section's `AfterShot` anchors to be buildable (e.g.
    /// [`Self::reverse_section`]). The project-wide version walks every shot of
    /// every section, so calling it per section turns a batch of N single-section
    /// commands into N full-project passes - and mints UUIDs in sections the
    /// command never touched.
    ///
    /// Does NOT update [`Self::shot_uuid_index`]; pair it with
    /// [`Self::reindex_section_shot_uuids`] when the section's shots also move.
    pub fn ensure_section_shot_uuids(&mut self, section_id: SectionId) {
        let Some(section) = self.get_section_mut(section_id) else {
            return;
        };
        for shot in &mut section.shots {
            if shot.uuid.as_deref().map_or(true, str::is_empty) {
                shot.uuid = Some(uuid::Uuid::new_v4().to_string());
            }
        }
    }

    /// Re-mint the UUID of any shot that shares a UUID with an earlier shot in
    /// the SAME section, keeping the first occurrence (AfterShot anchors resolve
    /// to the first match via `position()`, so they stay valid). Returns the
    /// number re-minted. Restores the in-section uniqueness `resolve_key` relies
    /// on; the companion detection lives in [`check_structural_integrity`].
    /// Empty UUIDs are left for [`ensure_shot_uuids`].
    pub fn ensure_unique_shot_uuids(&mut self) -> usize {
        let mut reminted = 0;
        for section in &mut self.sections {
            let mut seen: HashSet<String> = HashSet::new();
            for shot in &mut section.shots {
                let Some(u) = shot.uuid.clone() else { continue };
                if u.is_empty() {
                    continue;
                }
                // insert() == false means this UUID already appeared in-section.
                if !seen.insert(u) {
                    let fresh = uuid::Uuid::new_v4().to_string();
                    seen.insert(fresh.clone());
                    shot.uuid = Some(fresh);
                    reminted += 1;
                }
            }
        }
        if reminted > 0 {
            self.touch();
        }
        reminted
    }

    /// Map every shot that has a UUID to its `(section_id, shot_id)` position.
    /// Shared by both load entry points (`open_project` and
    /// `do_load_project_full`) to feed `backfill_resolved_warning_uuids`, so the
    /// two sites can't diverge. Shots with no UUID are skipped - call
    /// [`ensure_shot_uuids`] first if every shot must be represented.
    pub fn shot_uuid_by_position(&self) -> HashMap<(u32, u32), String> {
        self.sections
            .iter()
            .flat_map(|s| {
                s.shots
                    .iter()
                    .filter_map(move |sh| sh.uuid.clone().map(|u| ((s.id, sh.id), u)))
            })
            .collect()
    }

    /// Rebuild the section-id → index cache from the current `sections`
    /// vector. Cheap (O(N)) and safe to call any time; call this after a
    /// bulk mutation (e.g. snapshot rollback) or before a hot lookup loop
    /// when you can't guarantee the cache is current.
    pub fn rebuild_section_index(&mut self) {
        self.section_index_cache.clear();
        self.section_index_cache.reserve(self.sections.len());
        for (i, s) in self.sections.iter().enumerate() {
            self.section_index_cache.insert(s.id, i);
        }
    }

    /// Rebuild the `(section_id, shot_uuid)` → shot-index cache from the current
    /// `sections`. O(total shots). Call before a hot `resolve_key` loop (batch
    /// import, post-load, snapshot restore). Because `resolve_key` re-checks the
    /// cached index against the shot's UUID, a stale entry is harmless - it falls
    /// back to the linear scan - so this only needs to run often enough to keep
    /// the lookups fast, not for correctness.
    pub fn rebuild_shot_uuid_index(&mut self) {
        self.shot_uuid_index.clear();
        for s in &self.sections {
            for (i, shot) in s.shots.iter().enumerate() {
                if let Some(u) = &shot.uuid {
                    // First-occurrence wins: mirror the cold-path `resolve_key`,
                    // which uses `shots.iter().position(...)` (first match). Under
                    // a duplicated within-section UUID, `or_insert` keeps the
                    // earliest index so the warm cache agrees with the cold scan.
                    self.shot_uuid_index
                        .entry((s.id, u.clone()))
                        .or_insert(i as u32);
                }
            }
        }
    }

    /// Refresh [`Self::shot_uuid_index`] for ONE section whose shots were
    /// reordered or re-minted, without the whole-project pass
    /// [`Self::rebuild_shot_uuid_index`] performs.
    ///
    /// A stale entry is never *wrong* - `resolve_key` re-checks the cached index
    /// against the shot's UUID and falls back to a linear scan - but an operation
    /// that permutes every index in a section (a reverse) invalidates every one of
    /// its entries at once, so every later `resolve_key` into that section pays the
    /// O(shots) scan until something else rebuilds the cache.
    pub fn reindex_section_shot_uuids(&mut self, section_id: SectionId) {
        self.shot_uuid_index
            .retain(|(sid, _), _| *sid != section_id);
        let Some(section) = self.get_section(section_id) else {
            return;
        };
        // Collected first: `section` borrows self immutably, the insert needs it
        // mutably.
        let entries: Vec<((SectionId, String), u32)> = section
            .shots
            .iter()
            .enumerate()
            .filter_map(|(i, shot)| shot.uuid.clone().map(|u| ((section_id, u), i as u32)))
            .collect();
        for (key, index) in entries {
            // First-occurrence wins, as in `rebuild_shot_uuid_index`.
            self.shot_uuid_index.entry(key).or_insert(index);
        }
    }

    /// Record a new merge. Thin wrapper over [`MergeTopology::push`] (which
    /// keeps the adjacency memo current) - kept as the named entry point the
    /// merge paths call.
    pub fn record_station_merge(&mut self, merge: StationMerge) {
        self.station_merges.push(merge);
    }

    /// The north reference in force for `section` (#438): the section's own,
    /// else the project default, else "nothing recorded" (correction 0).
    ///
    /// THE single resolution point. Geometry calls it once per section and
    /// passes the (`Copy`) result into the per-shot accessor, so the correction
    /// costs one addition per shot and cannot disagree between two walks.
    pub fn azimuth_reference_for(&self, section: &Section) -> AzimuthReference {
        section
            .azimuth_reference
            .or(self.default_azimuth_reference)
            .unwrap_or_default()
    }

    /// Every section's resolved reference, by id.
    ///
    /// For the paths that cannot hold a `&self` borrow while they work - the
    /// closure solve and the least-squares update both walk `sections.iter_mut()`
    /// - so that they still resolve through [`Self::azimuth_reference_for`]
    /// instead of re-implementing the section-then-project fallback. The values
    /// are `Copy` and the map is built once per solve.
    pub(crate) fn azimuth_reference_map(&self) -> HashMap<SectionId, AzimuthReference> {
        self.sections
            .iter()
            .map(|section| (section.id, self.azimuth_reference_for(section)))
            .collect()
    }

    /// [`Self::azimuth_reference_for`] by id. A section id that does not resolve
    /// falls back to the project default rather than erroring: the callers are
    /// geometry paths that have already validated the id, and inventing a
    /// correction for an unknown section is the one wrong answer.
    pub fn azimuth_reference_for_id(&self, section_id: SectionId) -> AzimuthReference {
        self.get_section(section_id)
            .map(|section| self.azimuth_reference_for(section))
            .unwrap_or_else(|| self.default_azimuth_reference.unwrap_or_default())
    }

    /// Compute the (dx, dy) horizontal displacement of a shot from its data,
    /// in the same true-north frame the coordinate walk uses.
    ///
    /// Routed through [`ShotData::average_azimuth_true`] rather than re-deriving
    /// the mean: this feeds the persisted `section_offsets` compensation that
    /// insert/prepend/delete apply, so a private copy that drifts from the
    /// coordinate walk moves the section by twice the shot vector and saves it.
    /// An inlined `%` copy did exactly that once `average_azimuth_of` moved to
    /// `rem_euclid` - the two disagreed by 180 degrees for readings more than
    /// 540 apart, which validation only warns about.
    ///
    /// `reference` is why this takes an argument at all (#438): the compensation
    /// has to cancel the vector the WALK produced, and the walk is corrected. A
    /// caller that passes the default here while the section carries a
    /// declination shifts that section by the difference on every insert.
    pub fn shot_displacement(data: &ShotData, reference: AzimuthReference) -> (f64, f64) {
        let az_rad = data.average_azimuth_true_rad(reference);
        let horizontal = data.horizontal_length();
        (horizontal * az_rad.sin(), horizontal * az_rad.cos())
    }

    // === Section offset helpers ===

    /// Shift all StationRef indices in the project for the given section.
    ///
    /// All station references where `section_id` matches AND `station_index >= from_index`
    /// have their `station_index` shifted by `delta`. This is needed when inserting or
    /// removing shots, which changes station indices for all subsequent stations.
    ///
    /// Affects: station_merges, loop_closures, zero_datums, station_offsets,
    /// station_comments, station_names - all routed through [`Self::map_station_refs`].
    /// THE single enumeration of every [`StationKey`]-bearing collection on the
    /// project, for READ access. `check_structural_integrity` and
    /// `reanchor_station_refs` consume this so a newly-added ref-bearing collection
    /// can't be silently missed; the `label` distinguishes categories.
    pub fn for_each_station_ref(&self, mut f: impl FnMut(&str, StationKey)) {
        for m in &self.station_merges {
            f("merge", m.station_a.clone());
            f("merge", m.station_b.clone());
        }
        for c in &self.loop_closures {
            f("loop closure", c.station_a.clone());
            f("loop closure", c.station_b.clone());
        }
        for zd in &self.zero_datums {
            f("zero datum", zd.station_ref.clone());
        }
        for k in self.station_offsets.keys() {
            f("station offset", k.clone());
        }
        for k in self.station_comments.keys() {
            f("station comment", k.clone());
        }
        for k in self.station_names.keys() {
            f("station name", k.clone());
        }
    }

    /// THE single enumeration for REWRITING every [`StationKey`]-bearing collection
    /// (rewrite twin of [`Self::for_each_station_ref`]). Vec fields mapped in place;
    /// the offset/comment HashMaps rebuilt with transformed keys. Used by
    /// [`Self::reanchor_station_refs`]. (Keys are position-independent, so
    /// insert/delete no longer rewrite them - `shift_station_refs` is gone.)
    pub fn map_station_refs(&mut self, mut f: impl FnMut(StationKey) -> StationKey) {
        // `iter_mut` invalidates the merge-adjacency memo: `f` can move an
        // endpoint to another section (the delete-shot split does exactly that),
        // which rewrites the very graph the memo describes.
        for m in self.station_merges.iter_mut() {
            m.station_a = f(m.station_a.clone());
            m.station_b = f(m.station_b.clone());
        }
        for c in &mut self.loop_closures {
            c.station_a = f(c.station_a.clone());
            c.station_b = f(c.station_b.clone());
        }
        for zd in &mut self.zero_datums {
            zd.station_ref = f(zd.station_ref.clone());
        }
        let offsets = std::mem::take(&mut self.station_offsets);
        self.station_offsets = offsets.into_iter().map(|(k, v)| (f(k), v)).collect();
        let comments = std::mem::take(&mut self.station_comments);
        self.station_comments = comments.into_iter().map(|(k, v)| (f(k), v)).collect();
        let names = std::mem::take(&mut self.station_names);
        self.station_names = names.into_iter().map(|(k, v)| (f(k), v)).collect();
    }

    fn station_ref_key(station_ref: &StationRef) -> (SectionId, u32) {
        (station_ref.section_id, station_ref.station_index)
    }

    fn has_direct_merge(&self, a: StationRef, b: StationRef) -> bool {
        self.station_merges.iter().any(|m| {
            (self.key_is_ref(&m.station_a, a) && self.key_is_ref(&m.station_b, b))
                || (self.key_is_ref(&m.station_a, b) && self.key_is_ref(&m.station_b, a))
        })
    }

    fn get_merge_group(&self, station_ref: StationRef) -> HashSet<StationRef> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(station_ref);
        queue.push_back(station_ref);

        while let Some(current) = queue.pop_front() {
            for merge in &self.station_merges {
                // Compare/resolve through the stored keys (never positional ==).
                let neighbor = if self.key_is_ref(&merge.station_a, current) {
                    self.resolve_key(&merge.station_b)
                } else if self.key_is_ref(&merge.station_b, current) {
                    self.resolve_key(&merge.station_a)
                } else {
                    None
                };
                if let Some(next) = neighbor {
                    if visited.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }

        visited
    }

    fn build_merge_components(
        &self,
        stations: &HashSet<StationRef>,
        removed_station: StationRef,
    ) -> Vec<Vec<StationRef>> {
        let mut adjacency: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for station in stations {
            adjacency.entry(*station).or_default();
        }

        for merge in &self.station_merges {
            let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) else {
                continue;
            };
            if a == removed_station || b == removed_station {
                continue;
            }
            if stations.contains(&a) && stations.contains(&b) {
                adjacency.entry(a).or_default().push(b);
                adjacency.entry(b).or_default().push(a);
            }
        }

        let mut components = Vec::new();
        let mut visited = HashSet::new();

        let mut station_list: Vec<StationRef> = stations.iter().copied().collect();
        station_list.sort_by_key(Self::station_ref_key);

        for station in station_list {
            if visited.contains(&station) {
                continue;
            }
            let mut component = Vec::new();
            let mut queue = VecDeque::new();
            visited.insert(station);
            queue.push_back(station);

            while let Some(current) = queue.pop_front() {
                component.push(current);
                if let Some(neighbors) = adjacency.get(&current) {
                    for neighbor in neighbors {
                        if visited.insert(*neighbor) {
                            queue.push_back(*neighbor);
                        }
                    }
                }
            }

            component.sort_by_key(Self::station_ref_key);
            components.push(component);
        }

        components
    }

    // === Shot operations ===

    // === Station operations ===

    // === Coordinate computation ===

    // ── Frozen shots ─────────────────────────────────────────────────
}

impl Project {
    /// Tolerance (metres) at which a drifted merge is REPORTED to the user by
    /// [`Self::check_merge_alignment`] (and its closure twin). The old
    /// release-mode alignment check used 5 cm and this keeps that: a survey
    /// notice that fires under 5 cm would be noise next to real field accuracy.
    ///
    /// This is a *reporting* threshold and nothing more. It is deliberately
    /// LOOSER than [`Self::MERGE_ALIGNMENT_INVARIANT_M`], and the ordering may
    /// only ever be `INVARIANT <= TOLERANCE`: the self-heal is gated on the
    /// invariant, so the app can never decline to repair something it also
    /// asserts cannot exist.
    pub const MERGE_ALIGNMENT_TOLERANCE_M: f64 = 0.05;

    /// The gap (metres) at which a merge stops being aligned as far as the CODE
    /// is concerned: the threshold [`Self::debug_assert_merge_alignment`] panics
    /// a test build on, and the gate [`Self::realign_merges`] self-heals at.
    ///
    /// One constant for both on purpose (#413 findings 14 and 20). The assert
    /// used a hard-coded 1 cm while the heal ran off the 5 cm reporting
    /// tolerance, so the whole 1-5 cm band was a state the app asserted could
    /// not exist and produced on purpose - a shot edit inside an unanchored
    /// merge group left exactly that, silently, and the next command that
    /// asserted panicked on a merge it never touched. Tying the two together
    /// makes "the app never leaves a merge in a state it would assert on" a
    /// property rather than a coincidence of two literals.
    ///
    /// 1 cm rather than 0: float dust is ~1e-9 m per station even across
    /// thousands of shots, so this is far above the noise floor while still
    /// being an order of magnitude tighter than anything a user is told about.
    /// If it ever needs to move, it must move DOWN or the assert must move with
    /// it - never up alone.
    pub const MERGE_ALIGNMENT_INVARIANT_M: f64 = 0.01;

    /// The displacement (metres) below which a section offset is float dust
    /// rather than geometry, and is therefore normalized away instead of stored.
    ///
    /// It is what makes "not displaced" have exactly one spelling - see the
    /// invariant on [`Self::write_section_offset`], which no `section_offsets`
    /// entry in memory or on disk may violate.
    ///
    /// One constant for the six places that decide it (#413 finding 2's dust
    /// half): [`Self::write_section_offset`], the offset solver's
    /// [`Self::cascade_bfs`] correction gate, the two merge-BFS gates in
    /// `reconcile_after_edit`, [`Self::settle_minted_merges`], and
    /// [`Self::renormalize_frame`]'s idempotence early-return. They used to
    /// disagree - the merge BFS declined below 1e-12 while `cascade_bfs`
    /// "corrected" a femtometre, CREATING a `section_offsets` key for a section
    /// that is not displaced - and the difference between a key and no key is
    /// structural, not numeric: it survives into the `.stalx`, and two routes to
    /// the same geometry stopped comparing equal.
    ///
    /// # Why the bound is where it is (#469, re-measured)
    ///
    /// The thing being gated is ABSOLUTE and the thing it swallows is RELATIVE,
    /// and that is the whole difficulty. An adjust-and-adjust-back is not the
    /// identity in IEEE (`(c + d) - d != c`), and the residue two routes to the
    /// same geometry differ by is a few ULPs OF THE COORDINATES BEING SUMMED -
    /// so it grows with how far the survey sits from its georeference origin,
    /// which is not a quantity any single write can see. The bound therefore has
    /// to clear the residue at the largest coordinates the app can legitimately
    /// hold, not at the ones a typical project happens to use.
    ///
    /// That ceiling is knowable. [`crate::GpsCoordinates::validate`]
    /// pins latitude to `[-90, 90]` and longitude to `[-180, 180]`, and
    /// `gps_to_local`'s ENU projection turns an angular separation `theta` into
    /// a horizontal displacement of `R * sin(theta)`, maximal at a quarter turn:
    /// however hostile or mistaken a GPS fix is, one datum can put the frame
    /// origin at most ~6.38e6 m (`WGS84_A`) away, measured by
    /// `the_dust_gate_clears_the_residue_at_the_georeference_ceiling`.
    /// Coordinates go past that only by STACKING - offsets on top of a
    /// georeference on top of another - which is what the horizon corpus below
    /// reaches. #469 item 1 (the frame renormalisation) is what put ordinary
    /// projects anywhere near any of it: the survey is translated onto whichever
    /// datum defines the frame, so coordinates carry the INTER-DATUM distance
    /// rather than the cave's own extent.
    ///
    /// Measured on this branch, with the gate sites instrumented and the whole
    /// distribution of `max(|dx|, |dy|)` recorded per site:
    ///
    /// | corpus | coordinates reached | worst residue |
    /// |---|---|---|
    /// | `CLOSURE_SHAPES=1` 40x150, real GPS spread (~46 km) | 7.9e4 m | 1.46e-11 m |
    /// | `COMBINED=1` 40x300, real GPS spread | 7.9e4 m | 1.96e-11 m |
    /// | `CLOSURE_SHAPES=1` 40x150, datums scattered worldwide | 5.7e6 m | 9.31e-10 m |
    /// | `COMBINED=1` 40x300, datums scattered worldwide | 4.7e6 m | 9.31e-10 m |
    /// | `HORIZON=1` 3x5000, datums scattered worldwide | 1.8e7 m | 4.66e-9 m |
    ///
    /// Every one of those is one to five ULPs of the coordinate magnitude
    /// (1.46e-11 is exactly 2^-36, one ULP at 7.9e4 m; 4.66e-9 is five ULPs at
    /// 4.2e6 m), which is the shape the argument above predicts. The worst case
    /// the app can produce is therefore ~5e-9 m, and it was measured PAST the
    /// georeference ceiling (the horizon run reached 1.8e7 m because section
    /// offsets stack on top of the inter-datum distance).
    ///
    /// 1e-6 m is a micrometre: two decades and a bit above that worst case, four
    /// decades above the residue a real-world georeference produces, and four
    /// decades below [`Self::MERGE_ALIGNMENT_INVARIANT_M`] - the ordering the
    /// whole chain depends on and `the_dust_gate_is_decades_clear_of_both_ends`
    /// enforces. Nothing physical lives in the gap: survey instruments read
    /// centimetres, and this app reports and asserts in centimetres.
    ///
    /// One ordering does NOT hold, and it is the one worth writing down: the fuzz
    /// comparison oracle's absolute tolerance is 1e-9 m (`TOLERANCE_ABS`,
    /// `core/state/fuzz_undo_tests.rs`), three decades BELOW this gate, so a
    /// difference anywhere in `(0, OFFSET_DUST_M)` is one the oracle WOULD report.
    /// It never has to, because that band is unreachable rather than merely
    /// unlikely: [`Self::write_section_offset`] is the single writer and it removes
    /// the key instead of writing a value inside the band, and
    /// [`Self::normalize_section_offsets`] drops any a legacy file carried, so no
    /// state the fuzzer can reach holds a `section_offsets` value under the gate at
    /// all. Two compared states can therefore only disagree inside the band via the
    /// STRADDLE below - a TRUE offset landing within one float residue (~5e-9 m,
    /// the worst measured above) of the gate boundary itself, so one route writes
    /// just over it and the other normalizes just under. That difference is
    /// micrometre-scale and the oracle sees it, which is the intended outcome: the
    /// answer is to re-measure this constant, never to widen that tolerance.
    ///
    /// # The failure this replaces, and the one to watch for
    ///
    /// The old 1e-12 was chosen against a corpus measurement of 1.51e-14 taken
    /// when coordinates were metre-scale (#413 finding 2). Item 1's
    /// renormalisation moved them four decades, the residue moved with them, and
    /// `CLOSURE_SHAPES=1` seed 18 then found the consequence: two cascades over
    /// the same geometry, seeded differently, landed on either side of the gate,
    /// so one wrote a picometre `section_offsets` key and the other wrote none.
    /// A key is structural, so the two states stopped comparing equal.
    ///
    /// That is the failure mode to watch for, and it is a STRADDLE rather than a
    /// magnitude: it happens when the residue distribution reaches the gate, not
    /// when it is large. So the bound stays a deterministic pure function of the
    /// value written - never a scale-aware or context-dependent threshold, which
    /// would let two routes disagree BY DESIGN - and if a residue ever
    /// approaches 1e-6 m the answer is to re-measure and re-argue this constant
    /// with the numbers above, not to widen an oracle. It is a NORMALIZATION
    /// applied when a value is written, never a tolerance on a comparison.
    pub const OFFSET_DUST_M: f64 = 1e-6;

    /// Per-section cumulative LOCAL `(x, y)` for every station index
    /// (`0..=shots.len()`), before section/station offsets are applied. Built
    /// once so callers that look up many stations - the alignment check and the
    /// offset solver - avoid re-walking a section's shots on every lookup. This
    /// is what keeps those operations `O(shots + merges)` instead of the
    /// `O(merges × depth)` that made the per-mutation alignment check too costly.
    /// Resolve a stored, position-independent [`StationKey`] to a transient
    /// positional [`StationRef`] against the CURRENT section structure.
    /// `None` if the section is gone or the anchoring shot was deleted (which is
    /// exactly the "dangling reference" an integrity check should surface).
    ///
    /// This is the read side of the stable-identity boundary: cross-references are
    /// stored as `StationKey`, and the solver/IPC resolve them to a positional
    /// `StationRef` here. `O(shots)` per call - callers that resolve many keys for
    /// the same section should hoist the lookup.
    pub fn resolve_key(&self, key: &StationKey) -> Option<StationRef> {
        let section = self.get_section(key.section_id)?;
        let index = match &key.anchor {
            StationAnchor::SectionStart => 0,
            StationAnchor::AfterShot(uuid) => {
                // O(1) via the shot-UUID index when warm. The index re-check
                // makes a stale/absent entry harmless: it falls back to the
                // authoritative linear scan, so the cache can never resolve to the
                // wrong station and never needs eager invalidation for safety.
                if let Some(&i) = self.shot_uuid_index.get(&(key.section_id, uuid.clone())) {
                    if section
                        .shots
                        .get(i as usize)
                        .and_then(|s| s.uuid.as_deref())
                        == Some(uuid.as_str())
                    {
                        return Some(StationRef::new(key.section_id, i + 1));
                    }
                }
                let pos = section
                    .shots
                    .iter()
                    .position(|s| s.uuid.as_deref() == Some(uuid.as_str()))?;
                (pos + 1) as u32
            }
            StationAnchor::Index(i) => {
                if *i as usize > section.shots.len() {
                    return None;
                }
                *i
            }
        };
        Some(StationRef::new(key.section_id, index))
    }

    /// True if the stored key resolves to exactly `sr` (the sound way to ask "is
    /// this stored ref the same physical station as this positional ref" - never
    /// compare a positional `Index` key to a stored `AfterShot` key directly).
    pub fn key_is_ref(&self, key: &StationKey, sr: StationRef) -> bool {
        self.resolve_key(key) == Some(sr)
    }

    /// Build the stored [`StationKey`] for a transient positional [`StationRef`].
    /// `None` if the section is gone, the index is out of range, or the anchoring
    /// shot has no UUID (shouldn't happen after the load-time mint). Station 0
    /// anchors to `SectionStart`; station `i` to `AfterShot(uuid of shot i-1)`.
    pub fn key_for_ref(&self, sr: StationRef) -> Option<StationKey> {
        let section = self.get_section(sr.section_id)?;
        let anchor = if sr.station_index == 0 {
            StationAnchor::SectionStart
        } else {
            let shot = section.shots.get((sr.station_index - 1) as usize)?;
            StationAnchor::AfterShot(shot.uuid.clone()?)
        };
        Some(StationKey::new(sr.section_id, anchor))
    }

    /// Resolve a stored key to a positional ref for a **non-persistence display
    /// payload only**, falling back to station 0 if the anchor no longer resolves
    /// (a dangling ref). NEVER use this on a save/wire path: collapsing a dangling
    /// key to `(section, 0)` writes a phantom station-0 relationship and can
    /// collide on the `(section_id, station_index)` PRIMARY KEY. Persistence and
    /// wire encoders use [`resolve_key`] and skip what doesn't resolve. The one
    /// legitimate caller is the delete-shot impact preview, where an endpoint may
    /// intentionally be the station being removed.
    pub(crate) fn key_to_ref_lossy(&self, key: &StationKey) -> StationRef {
        self.resolve_key(key)
            .unwrap_or_else(|| StationRef::new(key.section_id, 0))
    }

    /// Positional wire form of a stored merge, or `None` if either endpoint
    /// dangles (its anchoring shot was deleted) - a dangling merge is invalid and
    /// must be omitted rather than shown at a phantom station.
    pub fn merge_to_wire(&self, m: &StationMerge) -> Option<StationMergeWire> {
        Some(StationMergeWire {
            station_a: self.resolve_key(&m.station_a)?,
            station_b: self.resolve_key(&m.station_b)?,
            merged_at: m.merged_at,
        })
    }

    /// Positional wire form of a stored zero datum, or `None` if its ref dangles.
    pub fn datum_to_wire(&self, zd: &ZeroDatum) -> Option<ZeroDatumWire> {
        Some(ZeroDatumWire {
            station_ref: self.resolve_key(&zd.station_ref)?,
            gps: zd.gps,
        })
    }

    /// Positional wire form of a stored loop closure, or `None` if either
    /// endpoint dangles.
    pub fn closure_to_wire(&self, c: &LoopClosure) -> Option<LoopClosureWire> {
        Some(LoopClosureWire {
            id: c.id,
            station_a: self.resolve_key(&c.station_a)?,
            station_b: self.resolve_key(&c.station_b)?,
            path_length: c.path_length,
            misclosure_distance: c.misclosure_distance,
            error_percentage: c.error_percentage,
            horizontal_misclosure: c.horizontal_misclosure,
            vertical_misclosure: c.vertical_misclosure,
            strategy: c.strategy,
            is_aligned: c.is_aligned,
        })
    }

    /// Re-anchor every stored cross-reference from a transitional positional form
    /// (`Index`) to the stable `AfterShot(uuid)` / `SectionStart` form, now that
    /// shots (with UUIDs) are present. Run on load so a v6 file's positional refs
    /// open with stable identity and no `Index` anchor survives into the live model.
    pub fn reanchor_station_refs(&mut self) {
        let canon: Vec<Option<StationKey>> = {
            let mut out = Vec::new();
            self.for_each_station_ref(|_label, key| {
                out.push(self.resolve_key(&key).and_then(|sr| self.key_for_ref(sr)));
            });
            out
        };
        let mut i = 0;
        self.map_station_refs(|key| {
            let next = canon.get(i).cloned().flatten();
            i += 1;
            next.unwrap_or(key)
        });
        // The two passes (for_each_station_ref above, map_station_refs here) must
        // visit the ref-bearing collections in identical order for the positional
        // `canon` alignment to be correct. That holds only because the two HashMaps
        // (station_offsets/comments) are not mutated between the passes. Assert it
        // so a future change that breaks the order coupling fails loudly in debug
        // instead of silently re-anchoring an offset onto the wrong station.
        debug_assert_eq!(
            i,
            canon.len(),
            "reanchor_station_refs: map_station_refs visited a different number of \
             refs than for_each_station_ref produced - station-ref traversal order \
             diverged between the read and write passes"
        );
    }

    fn section_local_coords(&self) -> HashMap<SectionId, Vec<(f64, f64)>> {
        let mut map = HashMap::with_capacity(self.sections.len());
        for section in &self.sections {
            let mut coords = Vec::with_capacity(section.shots.len() + 1);
            let (mut x, mut y) = (0.0_f64, 0.0_f64);
            coords.push((x, y));
            let azimuth_reference = self.azimuth_reference_for(section);
            for shot in &section.shots {
                let azimuth_rad = shot.average_azimuth_true_rad(azimuth_reference);
                let horizontal = shot.horizontal_length();
                x += horizontal * azimuth_rad.sin();
                y += horizontal * azimuth_rad.cos();
                coords.push((x, y));
            }
            map.insert(section.id, coords);
        }
        map
    }

    /// World `(x, y)` of a station using a [`Self::section_local_coords`] map:
    /// section offset + cumulative local position + optional station offset.
    /// Matches the `x`/`y` of [`Self::compute_station_raw_coords`] in `O(1)`.
    fn station_world_xy(
        &self,
        sr: StationRef,
        local: &HashMap<SectionId, Vec<(f64, f64)>>,
        station_offsets: &HashMap<StationRef, (f64, f64, f64)>,
    ) -> Option<(f64, f64)> {
        let &(lx, ly) = local.get(&sr.section_id)?.get(sr.station_index as usize)?;
        let (ox, oy) = self
            .section_offsets
            .get(&sr.section_id)
            .copied()
            .unwrap_or((0.0, 0.0));
        let (mut x, mut y) = (lx + ox, ly + oy);
        if let Some(&(sx, sy, _)) = station_offsets.get(&sr) {
            x += sx;
            y += sy;
        }
        Some((x, y))
    }

    /// The `station_offsets` map (per-station manual nudges) resolved to
    /// transient positional [`StationRef`] keys against the CURRENT structure -
    /// the sound positional view for hot solver paths. Usually empty; entries
    /// whose anchor no longer resolves are dropped (surfaced by integrity).
    fn resolved_station_offsets(&self) -> HashMap<StationRef, (f64, f64, f64)> {
        let mut out = HashMap::with_capacity(self.station_offsets.len());
        for (key, &v) in &self.station_offsets {
            if let Some(sr) = self.resolve_key(key) {
                out.insert(sr, v);
            }
        }
        out
    }

    /// Per-station manual offset for a single positional ref. Found by resolving
    /// each stored key to its positional ref - sound for ANY anchor kind
    /// (canonical `AfterShot`/`SectionStart` after reanchor, or transitional
    /// `Index` straight off a raw load), unlike building a canonical key (which
    /// would never match an `Index` entry). `None` when there is no nudge (the
    /// common case - short-circuits before touching the map).
    pub fn station_offset_for_ref(&self, sr: StationRef) -> Option<(f64, f64, f64)> {
        if self.station_offsets.is_empty() {
            return None;
        }
        self.station_offsets
            .iter()
            .find(|(k, _)| self.resolve_key(k) == Some(sr))
            .map(|(_, v)| *v)
    }

    /// Station comment for a positional ref, found by resolving each stored key
    /// (sound for any anchor kind). For callers/tests that hold a positional ref
    /// rather than a stored key.
    #[cfg(any(test, feature = "test-support"))]
    pub fn station_comment_for_ref(&self, sr: StationRef) -> Option<&String> {
        self.station_comments
            .iter()
            .find(|(k, _)| self.resolve_key(k) == Some(sr))
            .map(|(_, v)| v)
    }

    /// Station name for a positional ref; the `station_names` twin of
    /// [`Self::station_comment_for_ref`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn station_name_for_ref(&self, sr: StationRef) -> Option<&String> {
        self.station_names
            .iter()
            .find(|(k, _)| self.resolve_key(k) == Some(sr))
            .map(|(_, v)| v)
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    clippy::panic,
    clippy::print_stderr,
    clippy::print_stdout
)]
mod tests;

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod station_names_tests;

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod uuid_unique_tests;
