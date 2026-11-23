//! Dive-plan (`.plnx`) orchestration on [`AppState`] (#244): derive the sibling path
//! from the open project, delegate CRUD to [`PlnxStore`], and translate between the IPC
//! snapshot (transient positional `StationRef`) and the on-disk form (stable
//! [`StoredAnchor`]s). Anchoring on save and re-placing on load means a saved plan tracks
//! the SAME physical station across survey edits. The gas inputs are the persisted
//! payload; the reachability field is always recomputed by the caller.
//!
//! # Anchors on the station graph (plan 10.3)
//!
//! The `.plnx` file is unchanged. A pin is still written as the shots its station sits
//! between ([`PlanAnchor`], #24 / #310), and this build adds the station's id
//! (`PlanAnchor::station`). Placing a pin:
//!
//! 1. **Its station, by id.** A station keeps its id through a reverse, an insert or a
//!    delete elsewhere, a split (Split Line Here makes the split station a junction) and a
//!    combine, and a merge keeps a retired id as an alias of the survivor. While that
//!    station exists and the anchor's own shots vouch for it (it is an end of one of them,
//!    or on a line that holds one: `Resolver::vouches`), the pin is on it, shown on the
//!    line that holds its shots now. This is exact: no parity, no position, no length guard.
//! 2. **Its neighbours**, when the station is gone (its shots were deleted), its shots no
//!    longer vouch for it (a detach, or its line left a junction), or the anchor has no id
//!    (0.6.0 wrote it): 0.6.0's rule on the line that holds the shots now. Both present:
//!    the end of `a` that faces `b`. One present: the saved side of it, flipped when the
//!    shot's record was reversed since. A terminal pin whose shot was split reports stale
//!    (`terminal_shot_length_bits`).
//! 3. **A pre-#24 key** (`StoredAnchor::Legacy`): `LineStart(line)` is `line.start`,
//!    `AfterShot(uuid)` is that shot's `to` (shot uuids are project-unique, so the shot is
//!    found in whichever line holds it now), `Index(i)` is the slot.
//!
//! **Record parity** (rule 2, one neighbour). 0.6.0 kept a parity per line (and per
//! combined shot) that flipped with every reverse, and a pin recorded it. A station-graph
//! project keeps none: a shot's orientation IS its line's direction now. So:
//! - a project migrated from 0.6.0 in this session carries 0.6.0's record parity
//!   (`FileState::legacy_parity`: reversed lines, counter-parity shots by uuid), and a
//!   0.6.0 pin is read against it, as 0.6.0 read it. The parity is the one at migration:
//!   a reverse in this session is not in it, and a shot moved to a new line by a split
//!   reads that line's (unreversed) parity;
//! - otherwise (a version 2 project, or an anchor this build wrote) the saved side is read
//!   literally against the shot's current direction. An anchor this build writes records
//!   `parity: false`; its id makes the parity moot unless rule 2 places it.
//!
//! The record parity lives only in memory (a version 2 file has no place for it), so
//! [`AppState::upgrade_dive_plan_anchors`] rewrites a migrated project's 0.6.0 pins in the
//! station-graph form while the parity is still known. It runs on every write that upgrades
//! the `.stalx`: the upgrading save, and a Recents rename or a SpeleoDB link strip of a closed
//! legacy file ([`upgrade_dive_plan_anchors_beside`]). Without it, a 0.6.0 one-neighbour pin on a line reversed
//! after the pin was saved lands one station off once the upgraded file is reopened.
//!
//! Design: `git show de8eef91:docs/plans/2026-07-06_dive-plan-persistence.md` (doc removed from master in #229).

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::config::GasConfig;
use crate::models::gas::{MAX_SEGMENT_SPEED_M_PER_MIN, MIN_SEGMENT_SPEED_M_PER_MIN};
use crate::models::survey::{Project, ShotLocation};
use crate::models::{
    Cylinder, DivePlan, DivePlanMeta, DivePlanSnapshot, Diver, LineId, MarkedPoint, PlanAnchorSet,
    PlanSegment, StationAnchor, StationId, StationKey, StationRef, DIVE_PLAN_SCHEMA_VERSION,
};
use crate::persistence::migrate::LegacyParity;
use crate::persistence::plnx::{PlnxMeta, PlnxStore};

/// Upper bound on a dive-plan name, enforced on the trusted write path independent of
/// the UI input widget (guards an unbounded untrusted string from a hand-edited `.plnx`; #244).
const MAX_PLAN_NAME_CHARS: usize = 200;

/// Upper bound on a marked point's optional label (#298). Enforced on BOTH directions: the
/// write path rejects (a buggy or compromised frontend), and the read path truncates - because
/// the threat this actually names, a hand-edited `.plnx`, enters through the READ path, where a
/// write-side check can never see it.
const MAX_MARKED_LABEL_CHARS: usize = 120;

/// Upper bound on a plan's ordered way points (#298). Every way point costs the engine a full
/// extra Dijkstra and can extend the returned route by the whole station count, so an unbounded
/// list from a hand-edited `.plnx` is a compute/memory amplifier, not just untidy data. 64 is far
/// past any real dive (the ballot's use case is "hit this passage on the way") while staying
/// trivially cheap. Enforced on save (reject), on load (truncate), and on the live compute command
/// (reject - see `commands::gas`), so the bound cannot be walked past from any direction.
pub const MAX_WAYPOINTS: usize = 64;

/// Upper bound on a plan's per-span pace/propulsion segments (#317). A plan has exactly one
/// segment per span, and a span is bounded by way points, so the ceiling on segments tracks the
/// ceiling on way points plus the one span left over on either end: `waypoints.len() + 1`.
pub const MAX_SEGMENTS: usize = MAX_WAYPOINTS + 1;

/// Upper bound on a plan's marked points (#298). They cost nothing in the engine - they never
/// reach it - but each one is a DOM element plus a station lookup on the frontend, so the same
/// hand-edited-file reasoning applies.
const MAX_MARKED_POINTS: usize = 500;

/// Upper bound on ONE call to the anchor-translation pair (#310), which unlike every sibling list
/// had no bound at all. Each entry costs a few index lookups under the project mutex - the same
/// lock the map, the data table and every mutation contend for - so an unbounded batch is a
/// lock-hold amplifier, not merely slow.
///
/// Derived from the caps that already bound what a plan can hold, rather than picked:
///
/// ```text
///   2                   origin + endpoint
/// + MAX_WAYPOINTS       the ordered way points
/// + MAX_MARKED_POINTS   the marked points
/// + MAX_WAYPOINTS       stage-drop overrides (capped at the way-point bound on load)
/// = 630 pins in one plan
/// x 2                   the frontend legitimately re-anchors the LIVE plan and its saved
///                       baseline in one pass
/// = 1260
/// ```
const MAX_PLAN_ANCHOR_BATCH: usize = 2 * (2 + MAX_WAYPOINTS + MAX_MARKED_POINTS + MAX_WAYPOINTS);

/// The open project's `.plnx` sibling path plus the identity we stamp/verify it
/// with (`created_at` is immutable, so it survives a file rename).
struct PlnxContext {
    path: PathBuf,
    project_created_at: String,
    project_name: String,
}

impl PlnxContext {
    /// The `.plnx` beside the `.stalx` at `stalx`, for `project` (its identity).
    fn beside(stalx: &Path, project: &Project) -> Self {
        let stem = stalx.file_stem().map_or_else(
            || "dive_plans".to_string(),
            |s| s.to_string_lossy().to_string(),
        );
        let path = match stalx.parent() {
            Some(dir) => dir.join(format!("{stem}.plnx")),
            None => PathBuf::from(format!("{stem}.plnx")),
        };
        Self {
            path,
            project_created_at: project.created_at.to_rfc3339(),
            project_name: project.name.clone(),
        }
    }
}

/// Whether an existing `.plnx` belongs to the open project.
#[derive(PartialEq, Eq)]
enum Ownership {
    /// No identity stamped yet (a brand-new file we may claim).
    Fresh,
    /// Stamped with this project's `created_at`.
    Owned,
    /// Stamped with a different project (a `.plnx` copied next to an unrelated
    /// project that happens to share the filename) - never read or write it.
    Foreign,
}

/// Classify a stored identity (or its absence) against the open project's identity. Pure
/// so both the pre-open probe path and (if ever needed) a post-open path share one
/// classifier.
///
/// The identity is `created_at.to_rfc3339()` of the project in memory, compared as a
/// string: the station-graph model keeps the creation instant to the nanosecond (the
/// `.stalx` v2 writer and the migration both round-trip it), so a plan file stamped by
/// 0.6.0 still classifies as `Owned` after the upgrade.
fn classify_ownership(stored: Option<String>, ctx: &PlnxContext) -> Ownership {
    match stored {
        None => Ownership::Fresh,
        Some(s) if s == ctx.project_created_at => Ownership::Owned,
        Some(_) => Ownership::Foreign,
    }
}

/// Determine ownership of the `.plnx` at `ctx.path` WITHOUT ever opening it via
/// [`PlnxStore::open`] (which unconditionally initializes schema + `user_version`) -
/// every command must resolve ownership before opening, so a `Foreign` file is never
/// mutated (#244 Copilot finding). A non-existent file is `Fresh` (nothing to protect;
/// the write paths create it via `PlnxStore::open`).
fn probe_ownership(ctx: &PlnxContext) -> Result<Ownership, String> {
    if !ctx.path.exists() {
        return Ok(Ownership::Fresh);
    }
    let stored = PlnxStore::probe_stored_project_created_at(&ctx.path)
        .map_err(|e| format!("Could not read the dive-plan file: {e}"))?;
    Ok(classify_ownership(stored, ctx))
}

fn to_meta(m: PlnxMeta) -> DivePlanMeta {
    DivePlanMeta {
        id: m.id,
        name: m.name,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn default_persisted_diver() -> Diver {
    Diver {
        rmv_l_per_min: 20.0,
        label: None,
    }
}

/// On-disk form of a dive plan (#244): identical to the IPC [`DivePlanSnapshot`] except
/// station references are persisted as stable [`StoredAnchor`]s, NOT the transient
/// positional `StationRef`. Anchored on save and placed again on load (module docs), so a
/// plan tracks the same physical station across survey edits. Not an IPC type (no `TS`
/// derive); the wire form stays `StationRef`. Serde-default tolerant like the IPC snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PersistedDivePlanSnapshot {
    #[serde(default)]
    origin: Option<StoredAnchor>,
    #[serde(default)]
    endpoint: Option<StoredAnchor>,
    #[serde(default)]
    waypoints: Vec<StoredAnchor>,
    #[serde(default = "default_persisted_diver")]
    diver: Diver,
    #[serde(default)]
    cylinders: Vec<Cylinder>,
    #[serde(default)]
    gas_config: GasConfig,
    #[serde(default)]
    stage_drop_overrides: Vec<Option<StoredAnchor>>,
    #[serde(default)]
    marked_points: Vec<PersistedMarkedPoint>,
    /// Per-span pace/propulsion (#317), persisted VERBATIM: a segment names a span between way
    /// points, not a station, so there is no reference inside it to canonicalize - no
    /// `PersistedSegment` mirror type is needed here, unlike `PersistedMarkedPoint` above. What
    /// still needs handling is the INDEXING: dropping way point `k` on load merges spans `k` and
    /// `k+1` into one, so `segments` must be reconciled against which way points survived - see
    /// `merge_segments_for_surviving_waypoints`.
    #[serde(default)]
    segments: Vec<PlanSegment>,
}

impl PersistedDivePlanSnapshot {
    /// Every station reference in the plan.
    fn anchors(&self) -> impl Iterator<Item = &StoredAnchor> {
        self.origin
            .iter()
            .chain(&self.endpoint)
            .chain(&self.waypoints)
            .chain(self.stage_drop_overrides.iter().flatten())
            .chain(self.marked_points.iter().map(|m| &m.station))
    }
}

/// On-disk form of a [`MarkedPoint`] (#298): the annotated station as a stable anchor.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedMarkedPoint {
    station: StoredAnchor,
    #[serde(default)]
    label: Option<String>,
}

/// The `.plnx` storage form of a station reference. The definitions live in
/// [`crate::models::dive_plan`] since the crate split; re-exported here so every
/// `crate::core::state::dive_plan::{PlanAnchor, StoredAnchor}` path keeps compiling, and
/// because this module is the code that builds and places them.
pub(crate) use crate::models::dive_plan::{PlanAnchor, StoredAnchor};

/// Whether a terminal anchor's recorded shot length still describes `shot_length`.
///
/// Deliberately NOT `saved_bits == shot.length.to_bits()`. What this guard detects is a SPLIT,
/// which shortens the shot by at least `MIN_SPLIT_CHILD_LENGTH_M` (0.01 m) - a difference nine
/// orders of magnitude larger than the tolerance here. Exact bit equality bought nothing over
/// that and is not preserved by a JSON round trip through JavaScript, where a `u64` is a double
/// (see `terminal_shot_length_bits`).
fn terminal_length_matches(saved_bits: u64, shot_length: f64) -> bool {
    let saved = f64::from_bits(saved_bits);
    if !saved.is_finite() || !shot_length.is_finite() {
        return false;
    }
    // Relative, floored at 1.0 so a sub-metre shot is not held to an absurd absolute bound.
    let scale = saved.abs().max(shot_length.abs()).max(1.0);
    (saved - shot_length).abs() <= 1e-9 * scale
}

/// Describe the station at `slot` by its id and the shots either side of it on that line.
///
/// Station `i` is bounded by the shot before it and the shot after it; either is absent at
/// the ends of the line. A SHOT-LESS line - a GPX waypoint imports as exactly one, a single
/// station with no shots at all - has neither, and that is not an error: `None` on both sides
/// (and the station id) is a complete description of "the only station this line has".
fn to_anchor(project: &Project, slot: Option<StationRef>) -> Option<StoredAnchor> {
    let slot = slot?;
    let line = project.line(slot.line_id)?;
    let i = usize::try_from(slot.station_index).ok()?;
    let station = line.station(i)?;
    let before = i.checked_sub(1).and_then(|p| line.shots.get(p));
    let after = line.shots.get(i);
    // A terminal station has exactly one neighbour; record its length so a later split of that
    // shot is detectable when the station itself is gone (see `terminal_shot_length_bits`). An
    // interior station is placed by the pair and needs no such guard; a shot-less line has no
    // neighbour at all.
    let terminal_shot_length_bits = match (before, after) {
        (Some(shot), None) | (None, Some(shot)) => Some(shot.length.to_bits()),
        _ => None,
    };
    Some(StoredAnchor::Pair(PlanAnchor {
        line_id: slot.line_id,
        a: before.map(|s| s.uuid.clone()),
        b: after.map(|s| s.uuid.clone()),
        // `a` is the shot BEFORE the station, so the station is on its "after" side.
        after_a: true,
        // A station-graph project keeps no record parity (module docs): the side is read
        // against the shot's direction as it stands, and the station id makes it moot.
        parity: false,
        parity_b: None,
        terminal_shot_length_bits,
        station: Some(station),
    }))
}

/// Places stored anchors against one project state (module docs). Built per batch: the
/// project's own indexes (topology, shot uuids) are cached on the project, and the alias
/// index is built at most once, the first time a retired id is looked up.
struct Resolver<'p> {
    project: &'p Project,
    /// 0.6.0's record parity, while a project migrated from it is open.
    legacy: Option<&'p LegacyParity>,
    /// Retired station id -> the station it was merged into.
    aliases: OnceCell<BTreeMap<StationId, StationId>>,
}

impl<'p> Resolver<'p> {
    fn new(project: &'p Project, legacy: Option<&'p LegacyParity>) -> Self {
        Self {
            project,
            legacy,
            aliases: OnceCell::new(),
        }
    }

    /// Place a stored station, whichever form it was written in.
    fn place(&self, stored: &StoredAnchor) -> Option<StationRef> {
        match stored {
            StoredAnchor::Pair(anchor) => self.place_pair(anchor),
            StoredAnchor::Legacy(key) => self.place_key(key),
        }
    }

    /// Place, counting a reference that no longer places into `stale`.
    fn place_counting(&self, stored: &StoredAnchor, stale: &mut u32) -> Option<StationRef> {
        let placed = self.place(stored);
        if placed.is_none() {
            *stale += 1;
        }
        placed
    }

    /// Where the shot a stored uuid names is now, in whichever line holds it: the stored id,
    /// else the id a uniqueness heal gave that shot in `line_id` (0.6.0's one rule for keys
    /// stored outside the project, `live_shot_uuid_for_stored`).
    fn locate(&self, line_id: LineId, uuid: &str) -> Option<ShotLocation> {
        let index = self.project.shot_uuids();
        let live =
            crate::models::live_shot_uuid_for_stored(line_id, uuid, |id| index.contains(id))?;
        index.get(&live)
    }

    /// The live station `id` names: itself, or the survivor it was merged into.
    fn live_station(&self, id: StationId) -> Option<StationId> {
        if self.project.stations.contains_key(&id) {
            return Some(id).filter(|s| !self.project.slots_of(*s).is_empty());
        }
        let aliases = self.aliases.get_or_init(|| {
            self.project
                .stations
                .values()
                .flat_map(|s| s.archived.iter().map(move |record| (record.id, s.id)))
                .collect()
        });
        aliases.get(&id).copied()
    }

    /// The slot `station` shows on: on the first of `lines` it is on, else its first slot.
    fn slot_of(&self, station: StationId, lines: &[Option<LineId>]) -> Option<StationRef> {
        let topology = self.project.topology();
        lines
            .iter()
            .flatten()
            .find_map(|line| topology.slot_on(station, *line))
            .or_else(|| topology.first_slot(station))
    }

    /// The line `anchor`'s shots live in now: its recorded line while that still holds one of
    /// them (or the anchor names none, a shot-less line), else the first line in list order
    /// that holds one. A Split Line Here moves the shots past the cut into a new line, Combine
    /// Lines moves the other line's shots into the kept one, and an undo of either moves them
    /// back.
    fn anchor_line(
        &self,
        anchor: &PlanAnchor,
        a: Option<ShotLocation>,
        b: Option<ShotLocation>,
    ) -> Option<LineId> {
        let recorded = self.project.line(anchor.line_id).map(|l| l.id);
        let holders = || a.into_iter().chain(b).map(|loc| loc.line);
        if let Some(line) = recorded {
            if (anchor.a.is_none() && anchor.b.is_none()) || holders().any(|l| l == line) {
                return Some(line);
            }
        }
        holders()
            .min_by_key(|line| self.project.line_position(*line).unwrap_or(usize::MAX))
            .or(recorded)
    }

    fn place_pair(&self, anchor: &PlanAnchor) -> Option<StationRef> {
        let a = anchor
            .a
            .as_deref()
            .and_then(|u| self.locate(anchor.line_id, u));
        let b = anchor
            .b
            .as_deref()
            .and_then(|u| self.locate(anchor.line_id, u));
        let line = self.anchor_line(anchor, a, b);
        // (1) The station itself, while it exists and the anchor's shots vouch for it.
        if let Some(station) = anchor
            .station
            .and_then(|id| self.live_station(id))
            .filter(|s| self.vouches(anchor, *s, [a, b]))
        {
            return self.slot_of(station, &[line, a.map(|l| l.line), b.map(|l| l.line)]);
        }
        // (2) Its neighbours.
        self.place_by_neighbours(anchor, line?, a, b)
    }

    /// Whether the anchor's own shots vouch for `station`: it is an end of one of them, or on
    /// a line that holds one (a shot-less line's pin: on its recorded line).
    ///
    /// A Station ID is unique in one project's lineage, not across lineages: two machines
    /// editing copies of one project mint the same next id for different stations, and an
    /// anchor made on one copy (a discarded local edit, then a pull of the other's revision)
    /// would otherwise land on an unrelated station. Shot uuids are global, so a station the
    /// anchor's shots touch is the one it was made on. What this gives up is a pin whose shots
    /// no longer touch or hold its station: a line detached from a junction, a line that left
    /// one by a delete, a deleted line at a junction. Those place by the neighbours, as 0.6.0
    /// placed them.
    fn vouches(
        &self,
        anchor: &PlanAnchor,
        station: StationId,
        shots: [Option<ShotLocation>; 2],
    ) -> bool {
        let topology = self.project.topology();
        if anchor.a.is_none() && anchor.b.is_none() {
            return topology.slot_on(station, anchor.line_id).is_some();
        }
        shots.into_iter().flatten().any(|loc| {
            topology.slot_on(station, loc.line).is_some()
                || self.project.line(loc.line).is_some_and(|line| {
                    line.shot_from(loc.index) == Some(station)
                        || line
                            .shots
                            .get(loc.index)
                            .is_some_and(|shot| shot.to == station)
                })
        })
    }

    /// 0.6.0's placement by the neighbouring shots on `line_id` (module docs, rule 2).
    ///
    /// 1. **Both neighbours present** - the station is the end of `a` that FACES `b`, derived
    ///    from their current positions. Survives a reverse (both move together), shots
    ///    inserted between them, and a split of either (they simply stop being adjacent).
    /// 2. **One neighbour present** - a terminal station, or one whose partner was deleted.
    ///    Take the saved side, flipped when the shot's record was reversed since. Position is
    ///    NOT consulted: it reads the same after a reverse as after a delete-then-append.
    ///
    /// Splitting the shot a terminal anchor names FAILS SAFE rather than guessing: the split
    /// keeps the old uuid on the far half, so no shot names the station any more, and
    /// `terminal_shot_length_bits` detects it (the shot is shorter, and the side is no longer
    /// an end of the line).
    fn place_by_neighbours(
        &self,
        anchor: &PlanAnchor,
        line_id: LineId,
        a: Option<ShotLocation>,
        b: Option<ShotLocation>,
    ) -> Option<StationRef> {
        let line = self.project.line(line_id)?;
        let n = line.shots.len();
        let on_line =
            |loc: Option<ShotLocation>| loc.filter(|l| l.line == line_id).map(|l| l.index);
        let (pa, pb) = (on_line(a), on_line(b));
        let flipped_a = self.flipped(anchor, anchor.parity, line_id, pa);
        let flipped_b = self.flipped(
            anchor,
            anchor.parity_b.unwrap_or(anchor.parity),
            line_id,
            pb,
        );
        let index = match (pa, pb) {
            (Some(pa), Some(pb)) if pa < pb => pa + 1,
            (Some(pa), Some(_)) => pa,
            // The saved side of `a` (after it), or the other side once its record flipped.
            (Some(p), None) => {
                if anchor.after_a == flipped_a {
                    p
                } else {
                    p + 1
                }
            }
            // The side of `b` the station sat on (before it), likewise.
            (None, Some(p)) => {
                if anchor.after_a == flipped_b {
                    p + 1
                } else {
                    p
                }
            }
            // Recorded with no neighbours at all: a shot-less line's single station, which
            // needs none. Distinguished from "both neighbours were DELETED" by the stored
            // anchor, since a missing uuid and a missing shot both come back as `None`.
            (None, None) if anchor.a.is_none() && anchor.b.is_none() => 0,
            (None, None) => return None,
        };
        if index > n {
            return None;
        }
        if let Some(saved_bits) = anchor.terminal_shot_length_bits {
            let still_terminal = index == 0 || index == n;
            if !still_terminal {
                let moved = pa
                    .or(pb)
                    .and_then(|p| line.shots.get(p))
                    .is_none_or(|shot| !terminal_length_matches(saved_bits, shot.length));
                if moved {
                    return None;
                }
            }
        }
        Some(StationRef::new(line_id, u32::try_from(index).ok()?))
    }

    /// Whether the record of the neighbour at `position` on `line_id` was reversed since the
    /// anchor recorded `stored` (module docs, "Record parity"): only a 0.6.0 anchor on a
    /// project migrated in this session can tell; everything else reads the side literally.
    fn flipped(
        &self,
        anchor: &PlanAnchor,
        stored: bool,
        line_id: LineId,
        position: Option<usize>,
    ) -> bool {
        let (Some(legacy), None, Some(position)) = (self.legacy, anchor.station, position) else {
            return false;
        };
        let Some(shot) = self
            .project
            .line(line_id)
            .and_then(|l| l.shots.get(position))
        else {
            return false;
        };
        let parity = legacy.reversed_lines.contains(&line_id)
            ^ legacy.counter_parity_shots.contains(&shot.uuid);
        stored != parity
    }

    /// A pre-#24 key (module docs, rule 3): exactly the station 0.6.0 resolved it to, in
    /// whichever line holds its shot now, shown on the key's line when the station is on it.
    fn place_key(&self, key: &StationKey) -> Option<StationRef> {
        let (station, holder) = match &key.anchor {
            StationAnchor::LineStart => (self.project.line(key.line_id)?.start, None),
            StationAnchor::AfterShot(uuid) => {
                let loc = self.locate(key.line_id, uuid)?;
                let shot = self.project.line(loc.line)?.shots.get(loc.index)?;
                (shot.to, Some(loc.line))
            }
            StationAnchor::Index(i) => (
                self.project.station_at(StationRef::new(key.line_id, *i))?,
                None,
            ),
        };
        self.slot_of(station, &[Some(key.line_id), holder])
    }

    /// Resolve a stored LIST of anchors, DROPPING any that no longer place (each counted into
    /// `stale`), and reporting - as a same-length boolean mask over the INPUT positions - which
    /// entries survived. Used for the way points and marked points (#298), where - unlike the
    /// single optional origin/endpoint - there is no "null it out" slot: a dangling entry simply
    /// leaves the list, and the UI warns off `stale_ref_count`. Dropping is also what keeps a
    /// loaded plan safe to compute: the engine treats an unresolvable way point as a hard
    /// stall, so carrying a dead one through would blank the whole field on load.
    ///
    /// The mask exists for `segments` (#317): the segment list is positional against way
    /// points, so reconciling it after a drop needs to know exactly WHICH way point vanished,
    /// not just how many. See `merge_segments_for_surviving_waypoints`.
    fn place_list<'a>(
        &self,
        anchors: impl ExactSizeIterator<Item = &'a StoredAnchor>,
        stale: &mut u32,
    ) -> (Vec<StationRef>, Vec<bool>) {
        let mut refs = Vec::with_capacity(anchors.len());
        let mut survived = Vec::with_capacity(anchors.len());
        for anchor in anchors {
            match self.place_counting(anchor, stale) {
                Some(r) => {
                    refs.push(r);
                    survived.push(true);
                }
                None => survived.push(false),
            }
        }
        (refs, survived)
    }

    /// Place an optional anchor (`None` stays `None`, uncounted).
    fn place_optional(&self, anchor: Option<&StoredAnchor>, stale: &mut u32) -> Option<StationRef> {
        anchor.and_then(|a| self.place_counting(a, stale))
    }
}

/// A span the diver never gave a speed of its own: it flies at the plan speed, off the scooter.
fn inherited_segment() -> PlanSegment {
    PlanSegment {
        speed_m_per_min: None,
        on_scooter: false,
    }
}

/// The speed a span will ACTUALLY be flown at, which is the only basis on which two spans can be
/// compared.
///
/// The validity filter is not a nicety: it is the engine's, deliberately, because a stored number
/// the engine will not honour must not be allowed to win a comparison here either. `resolve_segments`
/// (`models/project/gas/scooter.rs`) falls back to the plan speed for anything outside
/// `[MIN_SEGMENT_SPEED_M_PER_MIN, MAX_SEGMENT_SPEED_M_PER_MIN]`, so a hand-edited `.plnx` holding a
/// denormal like `1e-320` would otherwise read as "the slowest span there is" to the fold below,
/// which would keep it - and the engine would then discard it and fly the merged span at the PLAN
/// speed, faster than either half. That is the exact direction these merges exist to prevent, so
/// both sides have to agree on which numbers are real.
fn effective_speed(seg: &PlanSegment, plan_speed: f64) -> f64 {
    seg.speed_m_per_min
        .filter(|v| (MIN_SEGMENT_SPEED_M_PER_MIN..=MAX_SEGMENT_SPEED_M_PER_MIN).contains(v))
        .unwrap_or(plan_speed)
}

/// Collapse two adjacent spans into one, conservatively - the arithmetic shared by every place a
/// segment boundary disappears (#317).
///
/// Both fields go the direction that can only ever make the merged span cost MORE, never less:
///
/// - **Speed: the slower of the two.** Gas is `rmv x ata(depth) x length / speed`, so the slower
///   span is the dearer one. "Slower" is decided on the EFFECTIVE speeds, which is why `plan_speed`
///   is a parameter: a span with no speed of its own inherits the plan's, and comparing `None`
///   against a number without resolving it first would pick the wrong half in one direction or the
///   other. A tie keeps `prev`, so a span inheriting the plan speed keeps inheriting it rather than
///   being frozen at whatever the plan speed happened to be that day.
/// - **`on_scooter`: OR, not AND.** The flag spends the rig's usable range, and range is a CAP
///   (`scootered <= limit`), so LESS spend means MORE reach. ANDing was therefore the
///   reach-EXTENDING direction, not the safe one: merging `{55, true}` with `{55, false}` under AND
///   kept the speed at 55 and dropped the whole first span's range spend, so the plan reached
///   further than either half allowed. Assuming the merged span is scootered spends its range, and
///   spending range can only pull the turn point closer.
fn fold_segments(prev: PlanSegment, next: PlanSegment, plan_speed: f64) -> PlanSegment {
    PlanSegment {
        speed_m_per_min: if effective_speed(&next, plan_speed) < effective_speed(&prev, plan_speed)
        {
            next.speed_m_per_min
        } else {
            prev.speed_m_per_min
        },
        on_scooter: prev.on_scooter || next.on_scooter,
    }
}

/// Force the `segments.len() == waypoint_count + 1` invariant, FOLDING any excess rather than
/// dropping it (#317).
///
/// Everything downstream - the panel's table, the engine's per-span re-weighting, the store's
/// add/remove-way-point arithmetic - takes the invariant as given and none of them heal a
/// violation, so it has to be established on the way past. Both directions are reachable: a plan
/// written before segments existed carries zero rows, and a hand-edited `.plnx` carries any number
/// at all.
///
/// A short list is PADDED with inherited spans - the file described those spans, it simply did not
/// describe the later ones, and an unedited span has always meant "fly it at the plan speed".
///
/// A long list is FOLDED into the final retained span with [`fold_segments`], never truncated. A
/// bare truncate silently discards the diver's statement about how those spans are flown, and every
/// field it discards is discarded in the reach-extending direction: a dropped slow speed becomes the
/// plan speed (cheaper on gas), a dropped `on_scooter` stops spending range. Folding keeps the
/// slowest speed and any scooter use, so an over-long list can only ever cost more than the file
/// claimed.
fn normalize_segment_count(
    mut segments: Vec<PlanSegment>,
    waypoint_count: usize,
    plan_speed: f64,
) -> Vec<PlanSegment> {
    // A plan always has at least the one span from the origin into the open field, so `want >= 1`
    // and the fold below always has a span to fold INTO.
    let want = waypoint_count + 1;
    if segments.len() > want {
        let tail = segments.split_off(want);
        if let Some(last) = segments.last_mut() {
            for seg in tail {
                *last = fold_segments(*last, seg, plan_speed);
            }
        }
    } else {
        segments.resize_with(want, inherited_segment);
    }
    segments
}

/// Merge a persisted segment list against which way points survived resolution (#317).
///
/// ⚠ **This is not the backend half of the store's `mergeSegments`.** The names line up and the
/// arithmetic is shared, but they answer different questions and neither can stand in for the
/// other: this one reconciles DANGLING ANCHORS on load - way points the survey no longer
/// contains, which the user never touched and may not know about - while the store's version
/// implements the user pressing "remove this way point". Only the conservative arithmetic in
/// [`fold_segments`] is common to both, and that is what the tests on either side pin. Do not
/// "unify" them; a change to one is not automatically a change the other wants.
///
/// Segments are positional against way points (`segments.len() == waypoints.len() + 1`): span
/// `k` runs between way point `k-1` and way point `k` (span 0 runs from the origin, the last span
/// covers the open reach field past the final way point). When way point `j` is DROPPED (its
/// station no longer resolves), the two spans it used to separate collapse into one - the diver's
/// actual route no longer stops there, so there is no longer a boundary across which two
/// different speeds could apply.
///
/// `survived[j]` reports whether way point `j` (0-indexed into the pre-drop list) resolved - the
/// mask [`Resolver::place_list`] returns. A length mismatch against `segments` is only
/// reachable via a hand-edited `.plnx` (the trusted write path always saves
/// `segments.len() == waypoints.len() + 1`), and it IS reachable here: this runs on the raw
/// persisted list, before `normalize_segment_count` settles the length. It degrades gracefully by
/// treating any waypoint position past the end of `survived` as surviving, so a malformed file
/// can only ever leave a span un-merged, never merge across a boundary the mask does not
/// actually describe.
fn merge_segments_for_surviving_waypoints(
    segments: Vec<PlanSegment>,
    survived: &[bool],
    plan_speed: f64,
) -> Vec<PlanSegment> {
    let last_index = segments.len().saturating_sub(1);
    let mut merged = Vec::new();
    let mut pending: Option<PlanSegment> = None;
    for (j, seg) in segments.into_iter().enumerate() {
        pending = Some(match pending {
            None => seg,
            Some(prev) => fold_segments(prev, seg, plan_speed),
        });
        let waypoint_survived = survived.get(j).copied().unwrap_or(true);
        if j == last_index || waypoint_survived {
            if let Some(finished) = pending.take() {
                merged.push(finished);
            }
        }
    }
    merged
}

/// The speed a span with no speed of its own is flown at: the plan's survey speed, or its swim
/// speed when the plan is symmetric.
fn plan_speed_of(config: &GasConfig) -> f64 {
    config
        .survey_speed_m_per_min
        .unwrap_or(config.swim_speed_m_per_min)
}

/// Bound one anchor-translation batch (#310), mirroring the way-point and segment caps.
///
/// REJECT rather than truncate, for the reason those two do: the two commands are index-aligned
/// with their input, so a short answer would silently re-anchor only the first `N` pins and leave
/// the rest of the plan holding stale ones - the caller has no way to tell that from a batch where
/// the tail genuinely did not place. The bound is far past any real plan (see
/// [`MAX_PLAN_ANCHOR_BATCH`]), so this is unreachable from normal use.
fn validate_anchor_batch(len: usize) -> Result<(), String> {
    if len > MAX_PLAN_ANCHOR_BATCH {
        return Err(format!(
            "Too many plan anchors: {len} (max {MAX_PLAN_ANCHOR_BATCH})."
        ));
    }
    Ok(())
}

/// Truncate to at most `max` CHARACTERS (not bytes), so a multi-byte label can never be cut
/// mid-scalar into invalid text.
fn truncate_chars(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    s.chars().take(max).collect()
}

/// Whether a stored anchor was written by 0.6.0 (no station id), so that only the record
/// parity the migration carried in memory can place it reliably.
fn written_by_legacy(anchor: &StoredAnchor) -> bool {
    match anchor {
        StoredAnchor::Legacy(_) => true,
        StoredAnchor::Pair(pair) => pair.station.is_none(),
    }
}

/// Rewrite the 0.6.0 anchors of the `.plnx` beside a legacy file that is upgraded to
/// version 2 while it is CLOSED (a Recents rename, a SpeleoDB link strip): the
/// [`AppState::upgrade_dive_plan_anchors`] of the upgrading save, against the project
/// that write holds and the record parity its migration carried (review finding
/// D-migration-upgrade-4). Without it a 0.6.0 one-neighbour pin on a line reversed after
/// the pin was saved lands one station off once the upgraded file is opened: the version
/// 2 file keeps no parity. Returns how many plans were rewritten; a missing `.plnx` or one
/// of another project is left alone.
pub(super) fn upgrade_dive_plan_anchors_beside(
    stalx: &Path,
    project: &Project,
    parity: &LegacyParity,
) -> Result<usize, String> {
    let ctx = PlnxContext::beside(stalx, project);
    if !ctx.path.exists() || probe_ownership(&ctx)? != Ownership::Owned {
        return Ok(0);
    }
    let resolver = Resolver::new(project, Some(parity));
    upgrade_plnx_rows(&ctx, |p| Ok(reanchor_legacy(&resolver, p)))
}

/// The rows half of the anchor upgrade: every plan of the `.plnx` at `ctx.path` that holds
/// a 0.6.0 anchor is rewritten through `reanchor`, keeping its name and `updated_at`.
fn upgrade_plnx_rows(
    ctx: &PlnxContext,
    reanchor: impl Fn(PersistedDivePlanSnapshot) -> Result<PersistedDivePlanSnapshot, String>,
) -> Result<usize, String> {
    let store =
        PlnxStore::open(&ctx.path).map_err(|e| format!("Could not open dive plans: {e}"))?;
    let rows = store
        .list()
        .map_err(|e| format!("Could not read dive plans: {e}"))?;
    let mut rewritten = 0;
    for meta in rows {
        let Some(row) = store
            .get(meta.id)
            .map_err(|e| format!("Could not read the dive plan: {e}"))?
        else {
            continue;
        };
        let persisted: PersistedDivePlanSnapshot = match crate::models::disk_names::from_disk_str(
            &row.snapshot_json,
        ) {
            Ok(p) => p,
            Err(e) => {
                // Left alone: `load_dive_plan` reports it when the plan is opened.
                tracing::warn!(plan = meta.id, error = %e, "an unreadable dive plan was not upgraded");
                continue;
            }
        };
        if !persisted.anchors().any(written_by_legacy) {
            continue;
        }
        let upgraded = reanchor(persisted)?;
        let json = crate::models::disk_names::to_disk_string(&upgraded)
            .map_err(|e| format!("Could not serialize the dive plan: {e}"))?;
        store
            .update(
                meta.id,
                &meta.name,
                &meta.updated_at,
                DIVE_PLAN_SCHEMA_VERSION as i32,
                &json,
            )
            .map_err(|e| format!("Could not save the dive plan: {e}"))?;
        rewritten += 1;
    }
    Ok(rewritten)
}

/// `p` with every 0.6.0 anchor that places (through `r`) replaced by the anchor of its
/// station.
fn reanchor_legacy(
    r: &Resolver<'_>,
    mut p: PersistedDivePlanSnapshot,
) -> PersistedDivePlanSnapshot {
    let renew = |anchor: &mut StoredAnchor| {
        if !written_by_legacy(anchor) {
            return;
        }
        if let Some(fresh) = r.place(anchor).and_then(|s| to_anchor(r.project, Some(s))) {
            *anchor = fresh;
        }
    };
    p.origin.iter_mut().for_each(renew);
    p.endpoint.iter_mut().for_each(renew);
    p.waypoints.iter_mut().for_each(renew);
    p.stage_drop_overrides.iter_mut().flatten().for_each(renew);
    p.marked_points
        .iter_mut()
        .for_each(|m| renew(&mut m.station));
    p
}

impl AppState {
    /// Resolve the `.plnx` sibling path + project identity for the open project.
    /// `Err` (user-facing) if no project is open or it has never been saved (no path
    /// to sit a sibling beside).
    fn plnx_context(&self) -> Result<PlnxContext, String> {
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let Some(p) = guard.as_ref() else {
            return Err("No project open.".to_string());
        };
        // A deferred new project HAS a reserved file_path, but nothing was ever
        // written there - and its first implicit save auto-renames on collision
        // (next_available_project_path), which would strand a .plnx already
        // sitting beside the reserved name (#275 B3). No .stalx on disk means
        // there is nowhere to anchor a sidecar yet, so treat it the same as
        // having no path at all; the flag clears on the first successful save.
        let stalx = match p.file_path.as_ref() {
            Some(path) if !self.initial_save_deferred() => path,
            _ => return Err("Save the project before saving dive plans.".to_string()),
        };
        Ok(PlnxContext::beside(stalx, p))
    }

    /// Run `f` with a [`Resolver`] on the open project (and, for a project migrated from
    /// 0.6.0 in this session, its record parity). Lock order: `project`, then `file`.
    fn with_resolver<R>(&self, f: impl FnOnce(&Resolver<'_>) -> R) -> Result<R, String> {
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let Some(project) = guard.as_ref() else {
            return Err("No project open.".to_string());
        };
        let legacy = self.with_file(|file| file.legacy_parity.clone());
        Ok(f(&Resolver::new(project, legacy.as_ref())))
    }

    /// List the dive plans stored next to the open project. Empty (never an error the
    /// UI must shout about) when there is no saved project, no `.plnx` yet, or the
    /// `.plnx` belongs to a different project.
    pub fn list_dive_plans(&self) -> Result<Vec<DivePlanMeta>, String> {
        let Ok(ctx) = self.plnx_context() else {
            return Ok(Vec::new());
        };
        if !ctx.path.exists() {
            return Ok(Vec::new());
        }
        if probe_ownership(&ctx)? == Ownership::Foreign {
            return Ok(Vec::new());
        }
        let store =
            PlnxStore::open(&ctx.path).map_err(|e| format!("Could not open dive plans: {e}"))?;
        let rows = store
            .list()
            .map_err(|e| format!("Could not read dive plans: {e}"))?;
        Ok(rows.into_iter().map(to_meta).collect())
    }

    /// Load one plan, placing its stored anchors against the live survey (dangling
    /// anchors are nulled and counted into `stale_ref_count`).
    pub fn load_dive_plan(&self, id: i64) -> Result<DivePlan, String> {
        let ctx = self.plnx_context()?;
        if !ctx.path.exists() {
            return Err("That dive plan no longer exists.".to_string());
        }
        if probe_ownership(&ctx)? == Ownership::Foreign {
            return Err("These dive plans belong to a different project.".to_string());
        }
        let store =
            PlnxStore::open(&ctx.path).map_err(|e| format!("Could not open dive plans: {e}"))?;
        let row = store
            .get(id)
            .map_err(|e| format!("Could not read the dive plan: {e}"))?
            .ok_or_else(|| "That dive plan no longer exists.".to_string())?;
        // The sidecar keeps the pre-rename names; see `disk_names`.
        let persisted: PersistedDivePlanSnapshot =
            crate::models::disk_names::from_disk_str(&row.snapshot_json)
                .map_err(|e| format!("Could not parse the dive plan: {e}"))?;
        let (snapshot, anchors, stale_ref_count) = self.resolve_persisted(persisted)?;
        Ok(DivePlan {
            meta: to_meta(row.meta),
            snapshot,
            stale_ref_count,
            anchors,
        })
    }

    /// Describe each of `refs` by its station (id and the shots it sits between, #310),
    /// index-aligned with the input. A ref that names no live station yields `None` - the
    /// same "it does not place" answer the save path gets, rather than an error that would
    /// fail a whole batch.
    ///
    /// This is the live-pin half of what the `.plnx` has always done on save: the frontend holds
    /// the anchor beside the positional ref, so a survey edit moves the pin with its station
    /// instead of silently rebinding it to whatever slid into that index.
    pub fn plan_anchors_for_refs(
        &self,
        refs: Vec<StationRef>,
    ) -> Result<Vec<Option<StoredAnchor>>, String> {
        validate_anchor_batch(refs.len())?;
        self.with_resolver(|r| {
            refs.into_iter()
                .map(|slot| to_anchor(r.project, Some(slot)))
                .collect()
        })
    }

    /// Place each anchor against the survey as it stands now (#310), index-aligned with the
    /// input; `None` where it no longer places. The read half of [`Self::plan_anchors_for_refs`],
    /// and the same placement the load path uses - including 0.6.0's forms, since an anchor the
    /// frontend is holding may have come straight out of an old `.plnx`.
    pub fn plan_refs_for_anchors(
        &self,
        anchors: Vec<StoredAnchor>,
    ) -> Result<Vec<Option<StationRef>>, String> {
        validate_anchor_batch(anchors.len())?;
        self.with_resolver(|r| anchors.iter().map(|a| r.place(a)).collect())
    }

    /// Save (insert when `id` is `None`, else update) a plan next to the open
    /// project, writing the `.plnx` atomically (SQLite transaction).
    pub fn save_dive_plan(
        &self,
        id: Option<i64>,
        name: String,
        snapshot: DivePlanSnapshot,
    ) -> Result<DivePlanMeta, String> {
        let ctx = self.plnx_context()?;
        let name = name.trim();
        if name.is_empty() {
            return Err("Give the dive plan a name.".to_string());
        }
        if name.chars().count() > MAX_PLAN_NAME_CHARS {
            return Err("That dive-plan name is too long.".to_string());
        }
        if snapshot
            .marked_points
            .iter()
            .filter_map(|m| m.label.as_ref())
            .any(|l| l.chars().count() > MAX_MARKED_LABEL_CHARS)
        {
            return Err("A marked-point label is too long.".to_string());
        }
        // Resolve ownership BEFORE opening the store: opening unconditionally initializes
        // schema/user_version, which must never happen to a Foreign file (#244 Copilot
        // finding). Fresh vs Owned is stable across the subsequent open() (identity is only
        // ever stamped, never changed by opening), so the probed value is still valid once
        // we open below.
        let ownership_before_open = probe_ownership(&ctx)?;
        if ownership_before_open == Ownership::Foreign {
            return Err(
                "A dive-plan file for a different project already exists next to this project."
                    .to_string(),
            );
        }
        // Canonicalize positional refs to stable keys against the live project BEFORE
        // writing, so the plan survives later survey edits (#2).
        let persisted = self.to_persisted(&snapshot)?;
        let json = crate::models::disk_names::to_disk_string(&persisted)
            .map_err(|e| format!("Could not serialize the dive plan: {e}"))?;
        let store =
            PlnxStore::open(&ctx.path).map_err(|e| format!("Could not open dive plans: {e}"))?;
        if ownership_before_open == Ownership::Fresh {
            store
                .ensure_identity(&ctx.project_created_at, &ctx.project_name)
                .map_err(|e| format!("Could not initialize the dive-plan file: {e}"))?;
        }
        let now = Utc::now().to_rfc3339();
        let sv = DIVE_PLAN_SCHEMA_VERSION as i32;
        let meta = match id {
            Some(id) => store
                .update(id, name, &now, sv, &json)
                .map_err(|e| format!("Could not save the dive plan: {e}"))?
                .ok_or_else(|| "That dive plan no longer exists.".to_string())?,
            None => store
                .insert(name, &now, sv, &json)
                .map_err(|e| format!("Could not save the dive plan: {e}"))?,
        };
        Ok(to_meta(meta))
    }

    /// Delete a plan next to the open project.
    pub fn delete_dive_plan(&self, id: i64) -> Result<(), String> {
        let ctx = self.plnx_context()?;
        if !ctx.path.exists() {
            return Err("That dive plan no longer exists.".to_string());
        }
        if probe_ownership(&ctx)? == Ownership::Foreign {
            return Err("These dive plans belong to a different project.".to_string());
        }
        let store =
            PlnxStore::open(&ctx.path).map_err(|e| format!("Could not open dive plans: {e}"))?;
        if !store
            .delete(id)
            .map_err(|e| format!("Could not delete the dive plan: {e}"))?
        {
            return Err("That dive plan no longer exists.".to_string());
        }
        Ok(())
    }

    /// Rewrite the 0.6.0 anchors of the open project's `.plnx` in the station-graph form
    /// (module docs), while the record parity 0.6.0's one-neighbour pins need is still in
    /// memory: it exists only for a project migrated in this session, and the first save of
    /// the upgraded project is the last moment it is known. Returns how many plans were
    /// rewritten.
    ///
    /// Every 0.6.0 anchor that places is replaced by the anchor of the station it places on
    /// (so it names the same station as before); one that does not place is kept verbatim (it
    /// stays stale, as it was). Nothing else in a plan changes, and each plan keeps its
    /// `updated_at`: this is not a user edit. A no-op (no record parity, no `.plnx`, a plan
    /// file of another project, every anchor already in the new form) writes nothing. Once
    /// done, the record parity is dropped: every anchor the frontend holds was made by this
    /// build ([`Self::load_dive_plan`] hands out fresh ones).
    pub fn upgrade_dive_plan_anchors(&self) -> Result<usize, String> {
        if self.with_file(|file| file.legacy_parity.is_none()) {
            return Ok(0);
        }
        let Ok(ctx) = self.plnx_context() else {
            return Ok(0);
        };
        let rewritten = if ctx.path.exists() && probe_ownership(&ctx)? == Ownership::Owned {
            upgrade_plnx_rows(&ctx, |p| self.with_resolver(|r| reanchor_legacy(r, p)))?
        } else {
            0
        };
        self.with_file(|file| file.legacy_parity = None);
        Ok(rewritten)
    }

    /// Canonicalize an IPC snapshot's positional refs to stable anchors against the open
    /// project (for on-disk persistence).
    fn to_persisted(&self, snap: &DivePlanSnapshot) -> Result<PersistedDivePlanSnapshot, String> {
        if snap.waypoints.len() > MAX_WAYPOINTS {
            return Err("That plan has too many way points.".to_string());
        }
        if snap.marked_points.len() > MAX_MARKED_POINTS {
            return Err("That plan has too many marked points.".to_string());
        }
        self.with_resolver(|r| {
            let project = r.project;
            // A route point that no longer anchors is REFUSED, not dropped. Dropping is what the
            // load path does (there is no empty slot in a list), but on save it would be data
            // loss the user is never told about: the frontend rebaselines its dirty check on the
            // snapshot it SENT, so the plan would read "saved" while the file on disk quietly
            // held a shorter route. Refusing keeps the file and the panel in agreement, and
            // matches the engine's own doctrine that a way point is a constraint rather than a
            // hint.
            let waypoints = snap
                .waypoints
                .iter()
                .map(|w| {
                    to_anchor(project, Some(*w)).ok_or_else(|| {
                        "A way point in this plan no longer exists in the survey. Remove it from \
                         the route and save again."
                            .to_string()
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let marked_points = snap
                .marked_points
                .iter()
                .map(|m| {
                    to_anchor(project, Some(m.station))
                        .map(|station| PersistedMarkedPoint {
                            station,
                            label: m.label.clone(),
                        })
                        .ok_or_else(|| {
                            "A marked point in this plan no longer exists in the survey. Remove \
                             it and save again."
                                .to_string()
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(PersistedDivePlanSnapshot {
                origin: to_anchor(project, snap.origin),
                endpoint: to_anchor(project, snap.endpoint),
                waypoints,
                diver: snap.diver.clone(),
                cylinders: snap.cylinders.clone(),
                gas_config: snap.gas_config.clone(),
                stage_drop_overrides: snap
                    .stage_drop_overrides
                    .iter()
                    .map(|o| to_anchor(project, *o))
                    .collect(),
                marked_points,
                // Normalised, not truncated. A segment past `waypoints.len() + 1` describes a
                // span that cannot exist for ANY plan, so unlike a dangling way point (refused
                // above) there is nothing to refuse the write over - but the data in it is still
                // the diver's statement about how a span is flown, and `take()` dropped it in
                // the reach-extending direction. `normalize_segment_count` folds it in instead;
                // way points are capped just above, so the result is `<= MAX_SEGMENTS` by
                // construction and that separate cap can no longer bind here.
                segments: normalize_segment_count(
                    snap.segments.clone(),
                    snap.waypoints.len(),
                    plan_speed_of(&snap.gas_config),
                ),
            })
        })?
    }

    /// Resolve a persisted (anchor) snapshot back to the IPC (`StationRef`) form against the
    /// open project, nulling dangling anchors and counting them into `stale_ref_count`. Also
    /// returns the anchors of the stations the surviving refs placed on, so the loaded plan's
    /// live pins stay anchor-backed without a second round trip (#310). They are anchors this
    /// build makes (with the station id), not the stored ones: a 0.6.0 anchor needs a record
    /// parity only a migrated session knows, and the pin it placed is the same station.
    fn resolve_persisted(
        &self,
        p: PersistedDivePlanSnapshot,
    ) -> Result<(DivePlanSnapshot, PlanAnchorSet, u32), String> {
        self.with_resolver(|r| {
            let anchor_of = |slot: Option<StationRef>| to_anchor(r.project, slot);
            let mut stale = 0u32;
            let origin = r.place_optional(p.origin.as_ref(), &mut stale);
            let endpoint = r.place_optional(p.endpoint.as_ref(), &mut stale);
            // The caps are re-applied HERE, not only on save: the threat they exist for is a
            // hand-edited `.plnx`, which by definition never crosses the write path. Every way
            // point costs the engine a full extra Dijkstra and every marked point a DOM node
            // plus a station lookup, so an unbounded list from a doctored file would be a
            // compute amplifier on every recompute. Truncating (rather than refusing the load)
            // keeps the rest of the plan usable.
            let (waypoints, waypoints_survived) =
                r.place_list(p.waypoints.iter().take(MAX_WAYPOINTS), &mut stale);
            // Segments are positional against the PRE-drop way points, so the merge must run
            // against the survival mask above BEFORE anything about waypoints changes shape -
            // see `merge_segments_for_surviving_waypoints`. It runs on the RAW persisted list
            // (any length): the mask describes the input positions, so trimming first would
            // misalign it.
            let plan_speed = plan_speed_of(&p.gas_config);
            let segments =
                merge_segments_for_surviving_waypoints(p.segments, &waypoints_survived, plan_speed);
            // The load side of the `segments.len() == waypoints.len() + 1` invariant (#317),
            // which a plan written before segments existed (zero rows) and a hand-edited file
            // (any number of rows) both violate. Way points are already capped just above, so
            // `waypoints.len() + 1 <= MAX_SEGMENTS` holds by construction and that separate cap
            // can no longer bind here.
            let segments = normalize_segment_count(segments, waypoints.len(), plan_speed);
            // Capped on the way IN like the way points and marked points, and for the same
            // reason: the threat is a hand-edited `.plnx`, which by definition never crosses
            // the write path. One entry per stage cylinder is the real bound (`gas.rs` already
            // truncates the engine side to the plan's cylinder count), so anything past the
            // way-point cap is doctored data, and each entry costs a station lookup on every
            // load. An override that no longer places keeps its slot (nulled): its position
            // names a stage.
            let stage_drop_overrides: Vec<Option<StationRef>> = p
                .stage_drop_overrides
                .iter()
                .take(MAX_WAYPOINTS)
                .map(|o| r.place_optional(o.as_ref(), &mut stale))
                .collect();
            let mut marked_points = Vec::new();
            for m in p.marked_points.into_iter().take(MAX_MARKED_POINTS) {
                let Some(station) = r.place_counting(&m.station, &mut stale) else {
                    continue;
                };
                marked_points.push(MarkedPoint {
                    station,
                    // Same reasoning as the list caps: the save path rejects an over-long
                    // label, but a doctored file bypasses it entirely, so truncate on the way in.
                    label: m.label.map(|l| truncate_chars(l, MAX_MARKED_LABEL_CHARS)),
                });
            }
            let anchors = PlanAnchorSet {
                origin: anchor_of(origin),
                endpoint: anchor_of(endpoint),
                waypoints: waypoints.iter().map(|w| anchor_of(Some(*w))).collect(),
                marked_points: marked_points
                    .iter()
                    .map(|m| anchor_of(Some(m.station)))
                    .collect(),
                stage_drop_overrides: stage_drop_overrides.iter().map(|o| anchor_of(*o)).collect(),
            };
            let snapshot = DivePlanSnapshot {
                origin,
                endpoint,
                waypoints,
                diver: p.diver,
                cylinders: p.cylinders,
                gas_config: p.gas_config,
                stage_drop_overrides,
                marked_points,
                segments,
            };
            (snapshot, anchors, stale)
        })
    }
}

#[cfg(test)]
#[path = "dive_plan_tests.rs"]
mod tests;
