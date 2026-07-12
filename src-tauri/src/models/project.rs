use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use ts_rs::TS;

use super::{
    ClosureStrategy, CollabState, GpsBounds, LoopClosure, Marker, Section,
    SectionData, SectionId, SectionUpdates, ShotData, ShotId, StationAnchor, StationKey,
    StationMerge, StationMergeWire, StationRef, ZeroDatum,
};
use super::{LoopClosureWire, ZeroDatumWire};

// Re-export the sibling module so the relocated `suggest_orphan_placements` (now in
// the `distance` child) keeps resolving `super::placement_suggest` — its `super` is
// now `project`, not `models`.
use super::placement_suggest;

mod closure;
mod coordinates;
mod distance;
mod gas;
mod lsq;
mod pathfinding;

// Used only by the `tests` module via `use super::*`; production code in this
// residual file no longer references them after the Phase 5.2/5.3 cluster moves.
#[cfg(test)]
use super::{Coordinates, GpsCoordinates, Shot, ShotUpdates, Station};
#[cfg(test)]
use crate::config::ClosureAdjustmentConfig;

mod connectivity;
mod mutations;
mod shots;

mod integrity;
mod offsets;
mod relationships;
mod stats;

pub use gas::{CylRole, Cylinder, Diver, GasReachField, PlanMode, RouteLeg, TankMaterial, Team};
pub use integrity::{IntegrityViolation, ViolationSeverity};
pub use shots::MIN_SPLIT_CHILD_LENGTH_M;

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
    /// The range covers every shot — the whole section is removed.
    WholeSection,
    /// The range starts at the first shot (nothing before); the survivors keep
    /// their absolute position by an offset shift.
    Prefix,
    /// The range ends at the last shot (nothing after); the head is truncated.
    Suffix,
    /// Shots remain both before and after — the section can be split (A=before,
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
    /// gap-close (shift) set `(start_pos+1 ..= end_pos+1)` — the superset that
    /// also covers the split's interior set — so the relationship panel never
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
    /// the split removes) — surfaced so the dialog can warn. The gap-close path
    /// is instead blocked when it would drop a datum, so this set is split-only.
    #[serde(default)]
    pub removed_datums: Vec<StationRef>,
    /// Station comments the SPLIT would drop (anchored on a strictly-interior
    /// station the split removes) — surfaced so the dialog can warn before the
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
    #[ts(type = "Array<StationMerge>")]
    pub station_merges: Vec<StationMerge>,
    /// Zero datums
    #[ts(type = "Array<ZeroDatum>")]
    pub zero_datums: Vec<ZeroDatum>,
    /// Detected loop closures
    #[serde(default)]
    #[ts(type = "Array<LoopClosure>")]
    pub loop_closures: Vec<LoopClosure>,
    /// Survey markers placed along shots.
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Monotonic counter for marker IDs. Never reused, even after deletions.
    #[serde(default)]
    pub next_marker_id: u64,
    /// File path if saved (None for new unsaved projects)
    #[ts(type = "string | null")]
    pub file_path: Option<PathBuf>,
    /// Next section ID to assign
    pub next_section_id: SectionId,
    /// Section offsets (x, y) applied when sections are merged/aligned
    #[serde(default)]
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
        serialize_with = "serialize_station_comments",
        deserialize_with = "deserialize_station_comments"
    )]
    #[ts(skip)]
    pub station_comments: HashMap<StationKey, String>,
    /// SHA256 hashes of imported DMP files (for duplicate detection)
    #[serde(default)]
    pub imported_file_hashes: Vec<String>,
    /// Content hashes of imported DMP sections (for section-level duplicate detection)
    #[serde(default)]
    pub imported_section_hashes: Vec<String>,
    /// Monotonic counter for closure IDs. Never reused, even after deletions.
    #[serde(default)]
    pub next_closure_id: u64,
    /// Collaboration mode state (sibling .tml linkage + baseline). None when off.
    #[serde(default)]
    pub collab: Option<CollabState>,

    /// Per-project validation thresholds (#57). Stored in the `.stalx` so they
    /// travel with the file between collaborators. New projects are seeded from
    /// the global config (the "defaults for new projects"); thereafter this is
    /// authoritative. Defaults on legacy projects until populated on load.
    #[serde(default)]
    pub validation: crate::config::ValidationConfig,

    /// Transient flag (NOT persisted) — when true,
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
    /// kicks in — callers that hammer the lookup (e.g. the batch
    /// importer) should call [`Self::rebuild_section_index`] first.
    #[serde(skip, default)]
    #[ts(skip)]
    pub section_index_cache: HashMap<SectionId, usize>,

    /// Merge-only section adjacency cache: section_id → [neighbor section
    /// ids via station_merges]. Transient, NOT persisted. Maintained
    /// incrementally by `add_station_merge` / `remove_station_merge`;
    /// any structural mutation outside those entry points is expected
    /// to call `rebuild_merge_adjacency`. The cache exists to turn the
    /// per-BFS-step iteration over all `station_merges` (O(N) inner)
    /// into a HashMap lookup (O(1) inner). For a tree-shape import that
    /// converts `get_merged_section_ids` from O(N²) to O(N) per call.
    #[serde(skip, default)]
    #[ts(skip)]
    pub merge_adjacency_cache: HashMap<SectionId, Vec<SectionId>>,

    /// `(section_id, shot_uuid)` → shot vector index. Transient, NOT persisted.
    /// Turns `resolve_key`'s `AfterShot` arm from an O(shots) linear scan with a
    /// per-element UUID string compare into an O(1) lookup — the dominant cost
    /// when building merge/closure connectivity over a large import (issue:
    /// importing a big Walls project pinned one core for minutes). Populated only
    /// by [`Self::rebuild_shot_uuid_index`] (batch import, post-load, snapshot
    /// restore); empty after deserialization, where `resolve_key` falls back to
    /// the linear scan. `resolve_key` re-checks the cached index against the
    /// shot's UUID, so a stale entry is harmless (it falls back to the scan).
    #[serde(skip, default)]
    #[ts(skip)]
    pub shot_uuid_index: HashMap<(SectionId, String), u32>,
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
    pub total_length: f64,
    /// Number of stations in the project (shots + 1 per section)
    pub station_count: usize,
    /// GPS center from first zero datum (lng, lat) - MapLibre order
    pub gps_center: Option<(f64, f64)>,
    /// GPS bounds including all zero datums
    pub gps_bounds: Option<GpsBounds>,
    /// Whether Ariane collaboration mode is enabled for this project. Lets the
    /// recent-projects panel show/toggle the state without opening the project.
    pub collab_enabled: bool,
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
/// shared reachability graph (jumps in, references out — see `build_survey_graph`).
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
/// are the shots traversed from A toward B — whole shots, even when an endpoint
/// only partially crosses one — with zero-cost merge/closure ties skipped (they
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
    pub shot_count: usize,
    pub total_length: f64,
    pub min_depth: f64,
    pub max_depth: f64,
    pub station_count: usize,
    pub loop_count: usize,
    /// Maximum straight-line distance from zero datum to any station
    pub penetration_distance: f64,
}

/// Serialize station_offsets as a Vec of (StationRef, (f64, f64, f64)) tuples
/// to avoid serde_json's requirement that map keys be strings.
fn serialize_station_offsets<S: serde::Serializer>(
    map: &HashMap<StationKey, (f64, f64, f64)>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let entries: Vec<(&StationKey, &(f64, f64, f64))> = map.iter().collect();
    entries.serialize(serializer)
}

/// Deserialize station_offsets from a Vec of (StationRef, (f64, f64, f64)) tuples.
fn deserialize_station_offsets<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<StationKey, (f64, f64, f64)>, D::Error> {
    let entries: Vec<(StationKey, (f64, f64, f64))> = Vec::deserialize(deserializer)?;
    Ok(entries.into_iter().collect())
}

/// Serialize station_comments as a Vec of (StationRef, String) tuples.
fn serialize_station_comments<S: serde::Serializer>(
    map: &HashMap<StationKey, String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let entries: Vec<(&StationKey, &String)> = map.iter().collect();
    entries.serialize(serializer)
}

/// Deserialize station_comments from a Vec of (StationRef, String) tuples.
fn deserialize_station_comments<'de, D: serde::Deserializer<'de>>(
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
/// `ProjectHeaderBinary` does). `#[ts(skip)]` fields on `Project` — the
/// `StationKey`-keyed `station_offsets`/`station_comments` and the transient
/// caches — are intentionally omitted to match the generated `Project` TS type.
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
        // Field set + order MUST stay in lockstep with the generated Project.ts
        // (16 ts-exported fields). Adding a ts-exported field to `Project`
        // requires adding it here too, or it silently drops off the IPC payload.
        let mut st = serializer.serialize_struct("Project", 18)?;
        st.serialize_field("name", &p.name)?;
        st.serialize_field("location", &p.location)?;
        st.serialize_field("created_at", &p.created_at)?;
        st.serialize_field("modified_at", &p.modified_at)?;
        st.serialize_field("sections", &p.sections)?;
        st.serialize_field("station_merges", &station_merges)?;
        st.serialize_field("zero_datums", &zero_datums)?;
        st.serialize_field("loop_closures", &loop_closures)?;
        st.serialize_field("markers", &p.markers)?;
        st.serialize_field("next_marker_id", &p.next_marker_id)?;
        st.serialize_field("file_path", &p.file_path)?;
        st.serialize_field("next_section_id", &p.next_section_id)?;
        st.serialize_field("section_offsets", &p.section_offsets)?;
        st.serialize_field("imported_file_hashes", &p.imported_file_hashes)?;
        st.serialize_field("imported_section_hashes", &p.imported_section_hashes)?;
        st.serialize_field("next_closure_id", &p.next_closure_id)?;
        st.serialize_field("collab", &p.collab)?;
        st.serialize_field("validation", &p.validation)?;
        st.end()
    }
}

/// `#[serde(serialize_with = "...")]` target for every IPC struct that carries a
/// `Project` to the frontend (`ProjectState`, `LoadProjectFullResult`,
/// `CreateProjectFullResult`), so they all emit the positional wire shape the
/// frontend reads rather than the internal `StationKey` shape derived serde emits.
pub(crate) fn serialize_project_ipc<S: serde::ser::Serializer>(
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
            station_merges: Vec::new(),
            zero_datums: Vec::new(),
            loop_closures: Vec::new(),
            markers: Vec::new(),
            next_marker_id: 1,
            file_path: None,
            next_section_id: 0,
            section_offsets: HashMap::new(),
            station_offsets: HashMap::new(),
            station_comments: HashMap::new(),
            imported_file_hashes: Vec::new(),
            imported_section_hashes: Vec::new(),
            next_closure_id: 1,
            collab: None,
            validation: crate::config::ValidationConfig::default(),
            cascade_deferred: false,
            section_index_cache: HashMap::new(),
            merge_adjacency_cache: HashMap::new(),
            shot_uuid_index: HashMap::new(),
        }
    }

    /// Assign sequential IDs to any loop closures that have id=0 (legacy projects).
    /// Updates `next_closure_id` to be greater than all existing IDs.
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
            if lc.strategy == ClosureStrategy::Move {
                lc.is_aligned = true;
            }
        }
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
        let section = self.get_section_mut(section_id)?;
        section.apply_updates(updates);
        self.touch();
        self.get_section(section_id)
    }

    /// Remove a section by ID
    pub fn remove_section(&mut self, section_id: SectionId) -> Option<Section> {
        let pos = self.sections.iter().position(|s| s.id == section_id)?;
        // Removing shifts every later section's index down by one — rather
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
    /// The save flow needs this before either the SQLite INSERT or the
    /// TML export runs.  Without it, pre-v6 .stalx files loaded today
    /// have `Shot::uuid = None` everywhere; the SQLite INSERT path
    /// mints fresh UUIDs at write time but does NOT update the in-memory
    /// shot, so a subsequent `export_tml_with_baseline` call in the
    /// SAME save sees the shots still as `uuid: None` and mints a
    /// SECOND, different set of UUIDs for the .tml.  The DB and the
    /// sibling .tml then disagree on every shot's identity until the
    /// next reload, breaking the collaboration round-trip.  Calling
    /// this once at the top of `save_project` guarantees both sides
    /// receive the same UUIDs.
    pub fn ensure_shot_uuids(&mut self) {
        for section in &mut self.sections {
            for shot in &mut section.shots {
                if shot.uuid.as_deref().map_or(true, str::is_empty) {
                    shot.uuid = Some(uuid::Uuid::new_v4().to_string());
                }
            }
        }
    }

    /// Re-mint the UUID of any shot that shares a UUID with an earlier shot in
    /// the SAME section, keeping the first occurrence (AfterShot anchors resolve
    /// to the first match via `position()`, so they stay valid). Returns the
    /// number re-minted. Restores the per-section uniqueness `resolve_key` relies
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
    /// two sites can't diverge. Shots with no UUID are skipped — call
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

    /// Rebuild the merge-only section adjacency cache from
    /// `station_merges`. O(N) where N is the merge count. Required
    /// after any path that mutates `station_merges` outside the
    /// `record_station_merge` / `remove_station_merges_matching`
    /// helpers (e.g. a serde-deserialised project, or a snapshot
    /// rollback).
    pub fn rebuild_merge_adjacency(&mut self) {
        self.merge_adjacency_cache.clear();
        // Each merge contributes one entry in each direction.
        self.merge_adjacency_cache.reserve(self.sections.len());
        for m in &self.station_merges {
            self.merge_adjacency_cache
                .entry(m.station_a.section_id)
                .or_default()
                .push(m.station_b.section_id);
            self.merge_adjacency_cache
                .entry(m.station_b.section_id)
                .or_default()
                .push(m.station_a.section_id);
        }
    }

    /// Rebuild the `(section_id, shot_uuid)` → shot-index cache from the current
    /// `sections`. O(total shots). Call before a hot `resolve_key` loop (batch
    /// import, post-load, snapshot restore). Because `resolve_key` re-checks the
    /// cached index against the shot's UUID, a stale entry is harmless — it falls
    /// back to the linear scan — so this only needs to run often enough to keep
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

    /// Push a new merge AND incrementally update the adjacency cache.
    /// Always use this instead of writing to `station_merges` directly
    /// so the cache stays in sync.
    pub(crate) fn record_station_merge(&mut self, merge: StationMerge) {
        // Guard against the half-populated-cache trap: `delete_section` /
        // `delete_shot` clear `merge_adjacency_cache` after retaining
        // surviving merges in `station_merges`, leaving the cache empty
        // and the merges intact.  If we then incrementally insert just
        // this new merge, the cache becomes non-empty but only contains
        // THIS edge — and `get_merged_section_ids` treats any non-empty
        // cache as authoritative, silently ignoring every surviving
        // merge from before the delete.  Rebuild from `station_merges`
        // first when the cache is empty but merges exist so the
        // incremental insert lands on a whole base.
        if self.merge_adjacency_cache.is_empty() && !self.station_merges.is_empty() {
            self.rebuild_merge_adjacency();
        }
        self.merge_adjacency_cache
            .entry(merge.station_a.section_id)
            .or_default()
            .push(merge.station_b.section_id);
        self.merge_adjacency_cache
            .entry(merge.station_b.section_id)
            .or_default()
            .push(merge.station_a.section_id);
        self.station_merges.push(merge);
    }

    /// Compute the (dx, dy) horizontal displacement of a shot from its data.
    /// Uses the same average azimuth calculation as coordinate computation.
    pub(crate) fn shot_displacement(data: &ShotData) -> (f64, f64) {
        let diff = (data.azimuth_out - data.azimuth_in + 540.0) % 360.0 - 180.0;
        let avg_az = (data.azimuth_in + diff / 2.0 + 360.0) % 360.0;
        let az_rad = avg_az.to_radians();
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
    /// station_comments — all routed through [`Self::map_station_refs`].
    /// THE single enumeration of every [`StationKey`]-bearing collection on the
    /// project, for READ access. `check_structural_integrity` and
    /// `reanchor_station_refs` consume this so a newly-added ref-bearing collection
    /// can't be silently missed; the `label` distinguishes categories.
    pub(crate) fn for_each_station_ref(&self, mut f: impl FnMut(&str, StationKey)) {
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
    }

    /// THE single enumeration for REWRITING every [`StationKey`]-bearing collection
    /// (rewrite twin of [`Self::for_each_station_ref`]). Vec fields mapped in place;
    /// the offset/comment HashMaps rebuilt with transformed keys. Used by
    /// [`Self::reanchor_station_refs`]. (Keys are position-independent, so
    /// insert/delete no longer rewrite them — `shift_station_refs` is gone.)
    pub(crate) fn map_station_refs(&mut self, mut f: impl FnMut(StationKey) -> StationKey) {
        for m in &mut self.station_merges {
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
    /// Tolerance (metres) within which two merged stations are considered the
    /// same physical point. The old release-mode alignment check used 5cm.
    pub const MERGE_ALIGNMENT_TOLERANCE_M: f64 = 0.05;

    /// Per-section cumulative LOCAL `(x, y)` for every station index
    /// (`0..=shots.len()`), before section/station offsets are applied. Built
    /// once so callers that look up many stations — the alignment check and the
    /// offset solver — avoid re-walking a section's shots on every lookup. This
    /// is what keeps those operations `O(shots + merges)` instead of the
    /// `O(merges × depth)` that made the per-mutation alignment check too costly.
    /// Resolve a stored, position-independent [`StationKey`] to a transient
    /// positional [`StationRef`] against the CURRENT section structure.
    /// `None` if the section is gone or the anchoring shot was deleted (which is
    /// exactly the "dangling reference" an integrity check should surface).
    ///
    /// This is the read side of the stable-identity boundary: cross-references are
    /// stored as `StationKey`, and the solver/IPC resolve them to a positional
    /// `StationRef` here. `O(shots)` per call — callers that resolve many keys for
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
    /// this stored ref the same physical station as this positional ref" — never
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
    /// dangles (its anchoring shot was deleted) — a dangling merge is invalid and
    /// must be omitted rather than shown at a phantom station.
    pub(crate) fn merge_to_wire(&self, m: &StationMerge) -> Option<StationMergeWire> {
        Some(StationMergeWire {
            station_a: self.resolve_key(&m.station_a)?,
            station_b: self.resolve_key(&m.station_b)?,
            merged_at: m.merged_at,
        })
    }

    /// Positional wire form of a stored zero datum, or `None` if its ref dangles.
    pub(crate) fn datum_to_wire(&self, zd: &ZeroDatum) -> Option<ZeroDatumWire> {
        Some(ZeroDatumWire {
            station_ref: self.resolve_key(&zd.station_ref)?,
            gps: zd.gps,
        })
    }

    /// Positional wire form of a stored loop closure, or `None` if either
    /// endpoint dangles.
    pub(crate) fn closure_to_wire(&self, c: &LoopClosure) -> Option<LoopClosureWire> {
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
             refs than for_each_station_ref produced — station-ref traversal order \
             diverged between the read and write passes"
        );
    }

    fn section_local_coords(&self) -> HashMap<SectionId, Vec<(f64, f64)>> {
        let mut map = HashMap::with_capacity(self.sections.len());
        for section in &self.sections {
            let mut coords = Vec::with_capacity(section.shots.len() + 1);
            let (mut x, mut y) = (0.0_f64, 0.0_f64);
            coords.push((x, y));
            for shot in &section.shots {
                let azimuth_rad = shot.average_azimuth().to_radians();
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
    /// transient positional [`StationRef`] keys against the CURRENT structure —
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
    /// each stored key to its positional ref — sound for ANY anchor kind
    /// (canonical `AfterShot`/`SectionStart` after reanchor, or transitional
    /// `Index` straight off a raw load), unlike building a canonical key (which
    /// would never match an `Index` entry). `None` when there is no nudge (the
    /// common case — short-circuits before touching the map).
    pub(crate) fn station_offset_for_ref(&self, sr: StationRef) -> Option<(f64, f64, f64)> {
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
    pub(crate) fn station_comment_for_ref(&self, sr: StationRef) -> Option<&String> {
        self.station_comments
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
