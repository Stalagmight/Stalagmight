//! Dive-plan (`.plnx`) orchestration on [`AppState`] (#244): derive the sibling path
//! from the open project, delegate CRUD to [`PlnxStore`], and translate between the IPC
//! snapshot (transient positional `StationRef`) and the on-disk form (stable
//! [`StationKey`] anchors). Canonicalizing to `AfterShot(uuid)` on save and re-resolving
//! on load means a saved plan tracks the SAME physical station across survey edits, the
//! way merges/closures/datums already persist references. The gas inputs are the
//! persisted payload; the reachability field is always recomputed by the caller.
//!
//! Design: `git show de8eef91:docs/plans/2026-07-06_dive-plan-persistence.md` (doc removed from master in #229).

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::AppState;
use crate::config::GasConfig;
use crate::models::project::gas::{MAX_SEGMENT_SPEED_M_PER_MIN, MIN_SEGMENT_SPEED_M_PER_MIN};
use crate::models::{
    Cylinder, DivePlan, DivePlanMeta, DivePlanSnapshot, Diver, MarkedPoint, PlanAnchorSet,
    PlanSegment, Project, Section, StationRef, DIVE_PLAN_SCHEMA_VERSION,
};
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
/// had no bound at all. Each entry costs a section scan under the project mutex - the same lock the
/// map, the data table and every mutation contend for - so an unbounded batch is a lock-hold
/// amplifier, not merely slow.
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
/// station references are persisted as stable [`StationKey`] anchors (`AfterShot(uuid)`),
/// NOT the transient positional `StationRef`. Canonicalized via [`Project::key_for_ref`]
/// on save and re-resolved via [`Project::resolve_key`] on load, so a plan tracks the
/// same physical station across survey edits. Not an IPC type (no `TS` derive); the wire
/// form stays `StationRef`. Serde-default tolerant like the IPC snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedDivePlanSnapshot {
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

/// Describe the station at `r` by its neighbouring shots.
fn to_anchor(project: &Project, r: Option<StationRef>) -> Option<StoredAnchor> {
    let r = r?;
    let section = project.get_section(r.section_id)?;
    let i = r.station_index as usize;
    if i > section.shots.len() {
        return None;
    }
    // Station `i` is bounded by the shot before it and the shot after it; either is absent at
    // the ends of the section. A SHOT-LESS section - a GPX waypoint imports as exactly one, a
    // single station with no shots at all - has neither, and that is not an error: `None` on
    // both sides is a complete description of "the only station this section has".
    let before = i.checked_sub(1).and_then(|p| section.shots.get(p));
    let after = section.shots.get(i);
    let a = before.and_then(|s| s.uuid.clone());
    let b = after.and_then(|s| s.uuid.clone());
    if before.is_some() && a.is_none() {
        return None;
    }
    if after.is_some() && b.is_none() {
        return None;
    }
    // A terminal station has exactly one neighbour; record its length so a later split of that
    // shot is detectable (see `terminal_shot_length_bits`). An interior station is placed by the
    // pair and needs no such guard; a shot-less section has no neighbour at all.
    let terminal_shot_length_bits = match (before, after) {
        (Some(shot), None) | (None, Some(shot)) => Some(shot.length.to_bits()),
        _ => None,
    };
    Some(StoredAnchor::Pair(PlanAnchor {
        section_id: r.section_id,
        a,
        b,
        // `a` is the shot BEFORE the station, so the station is on its "after" side.
        after_a: true,
        parity: section.reverse_parity,
        terminal_shot_length_bits,
    }))
}

/// Place a stored anchor against the project as it stands now.
///
/// Two cases, and no others:
///
/// 1. **Both neighbours present** - the station is the end of `a` that FACES `b`, derived from
///    their current positions. Survives a reverse (both move together), shots inserted between
///    them, and a split of either (they simply stop being adjacent; the side is unchanged).
/// 2. **One neighbour present** - a terminal station, or one whose partner was deleted. Take the
///    saved side, flipped when the section's parity says a reverse happened since. Position is
///    NOT consulted: an earlier version inferred the side from "is `a` at an end of the
///    section", which reads the same after a reverse as after a delete-then-append and so was
///    wrong for one of them whichever way it guessed.
///
/// Splitting the shot a terminal anchor names FAILS SAFE rather than guessing. The split gives
/// the new half a fresh UUID and keeps the old one on the shortened tail, so afterwards no shot
/// in the file has that station as an endpoint - nothing is left to anchor to, and any answer
/// would silently land on the split point. `terminal_shot_length_bits` detects it (the tail is
/// necessarily shorter, and the station is no longer at an end of the section) and the reference
/// is reported STALE, so the UI can ask the diver to re-pin it.
fn from_anchor(project: &Project, anchor: &StoredAnchor, stale: &mut u32) -> Option<StationRef> {
    let resolved = resolve_stored(project, anchor);
    if resolved.is_none() {
        *stale += 1;
    }
    resolved
}

/// The section `anchor`'s shots live in now: its recorded section while that
/// still holds one of them (or the anchor names none, a shot-less section),
/// else the first section that does. Shot uuids are project-unique, and a Split
/// Section Here moves the shots past the cut into a new section.
fn anchor_section<'p>(project: &'p Project, anchor: &PlanAnchor) -> Option<&'p Section> {
    let holds = |section: &Section| {
        [&anchor.a, &anchor.b]
            .into_iter()
            .any(|uuid| PlanAnchor::position_of(project, section, uuid).is_some())
    };
    let recorded = project.get_section(anchor.section_id);
    if let Some(section) = recorded {
        if (anchor.a.is_none() && anchor.b.is_none()) || holds(section) {
            return Some(section);
        }
    }
    project.sections.iter().find(|s| holds(s)).or(recorded)
}

fn resolve_anchor(project: &Project, anchor: &PlanAnchor) -> Option<StationRef> {
    let section = anchor_section(project, anchor)?;
    let n = section.shots.len();
    let pa = PlanAnchor::position_of(project, section, &anchor.a);
    let pb = PlanAnchor::position_of(project, section, &anchor.b);
    // A reverse since this anchor was written flips what "after" means.
    let flipped = anchor.parity != section.reverse_parity;
    let index = match (pa, pb) {
        // (1) Both present: the station is the end of `a` that faces `b`.
        (Some(pa), Some(pb)) if pa < pb => pa + 1,
        (Some(pa), Some(_)) => pa,
        // (2) One present: the saved side, corrected for orientation.
        (Some(p), None) => {
            if anchor.after_a != flipped {
                p + 1
            } else {
                p
            }
        }
        (None, Some(p)) => {
            if anchor.after_a != flipped {
                p
            } else {
                p + 1
            }
        }
        // Recorded with no neighbours at all: a shot-less section's single station, which needs
        // none. Distinguished from "both neighbours were DELETED" by the stored anchor rather
        // than by the lookup, since a missing uuid and a missing shot both come back as `None`.
        (None, None) if anchor.a.is_none() && anchor.b.is_none() => 0,
        // Neither neighbour survives: nothing left to anchor on.
        (None, None) => return None,
    };
    if index > n {
        return None;
    }
    // Fail SAFE on the one shape the pair cannot survive. If a terminal anchor's station is no
    // longer at an end of the section, the section was extended past it OR its shot was split -
    // and only the split changes that shot's length. A split leaves nothing in the file naming
    // this station, so resolving would silently land on the split point; report it stale
    // instead and let the UI ask for a re-pin.
    if let Some(saved_bits) = anchor.terminal_shot_length_bits {
        let still_terminal = index == 0 || index == n;
        if !still_terminal {
            let moved = pa
                .or(pb)
                .and_then(|p| section.shots.get(p))
                .is_none_or(|shot| !terminal_length_matches(saved_bits, shot.length));
            if moved {
                return None;
            }
        }
    }
    Some(StationRef::new(section.id, index as u32))
}

/// Place a stored station, whichever form it was written in.
fn resolve_stored(project: &Project, stored: &StoredAnchor) -> Option<StationRef> {
    match stored {
        StoredAnchor::Pair(anchor) => resolve_anchor(project, anchor),
        StoredAnchor::Legacy(key) => project.resolve_key(key).or_else(|| {
            // A pre-#24 key anchored on an id the load renamed still places
            // through the derived id.
            use crate::models::{StationAnchor, StationKey};
            let StationAnchor::AfterShot(old) = &key.anchor else {
                return None;
            };
            let after = |id: &str| {
                StationKey::new(key.section_id, StationAnchor::AfterShot(id.to_string()))
            };
            let live = crate::models::live_shot_uuid_for_stored(key.section_id, old, |id| {
                project.resolve_key(&after(id)).is_some()
            })?;
            project.resolve_key(&after(&live))
        }),
    }
}

/// Resolve a stored LIST of anchors, DROPPING any that no longer place (each counted into
/// `stale`), and reporting - as a same-length boolean mask over the INPUT positions - which
/// entries survived, echoing back the anchor each surviving ref came from. Used for the way
/// points and marked points (#298), where - unlike the single optional origin/endpoint - there is
/// no "null it out" slot: a dangling entry simply leaves the list, and the UI warns off
/// `stale_ref_count`. Dropping is also what keeps a loaded plan safe to compute: the engine
/// treats an unresolvable way point as a hard stall, so carrying a dead one through would blank
/// the whole field on load.
///
/// The mask exists for `segments` (#317): the segment list is positional against way points, so
/// reconciling it after a drop needs to know exactly WHICH way point vanished, not just how many.
/// See `merge_segments_for_surviving_waypoints`. The echoed anchors exist for #310: the loaded
/// plan hands them to the frontend so a live pin stays anchor-backed rather than positional.
fn from_anchors_with_survival(
    project: &Project,
    anchors: Vec<StoredAnchor>,
    stale: &mut u32,
) -> (Vec<StationRef>, Vec<Option<StoredAnchor>>, Vec<bool>) {
    let mut refs = Vec::with_capacity(anchors.len());
    let mut kept = Vec::with_capacity(anchors.len());
    let mut survived = Vec::with_capacity(anchors.len());
    for a in anchors {
        match from_anchor(project, &a, stale) {
            Some(r) => {
                refs.push(r);
                kept.push(Some(a));
                survived.push(true);
            }
            None => survived.push(false),
        }
    }
    (refs, kept, survived)
}

/// Resolve an optional anchor, keeping the anchor itself when it placed (#310).
///
/// The pair is what every single-slot reference needs: the positional ref for this session, and
/// the anchor it came from so the pin can be re-placed after the next survey edit. A dangling
/// anchor yields `(None, None)` - there is no sense in handing back a pin that does not place.
fn resolve_keeping_anchor(
    project: &Project,
    anchor: Option<StoredAnchor>,
    stale: &mut u32,
) -> (Option<StationRef>, Option<StoredAnchor>) {
    let Some(anchor) = anchor else {
        return (None, None);
    };
    match from_anchor(project, &anchor, stale) {
        Some(r) => (Some(r), Some(anchor)),
        None => (None, None),
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
/// mask [`from_anchors_with_survival`] returns. A length mismatch against `segments` is only
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
        let stem = stalx.file_stem().map_or_else(
            || "dive_plans".to_string(),
            |s| s.to_string_lossy().to_string(),
        );
        let path = match stalx.parent() {
            Some(dir) => dir.join(format!("{stem}.plnx")),
            None => PathBuf::from(format!("{stem}.plnx")),
        };
        Ok(PlnxContext {
            path,
            project_created_at: p.created_at.to_rfc3339(),
            project_name: p.name.clone(),
        })
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

    /// Load one plan, resolving its stable station keys against the live survey (dangling
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
        let persisted: PersistedDivePlanSnapshot = serde_json::from_str(&row.snapshot_json)
            .map_err(|e| format!("Could not parse the dive plan: {e}"))?;
        let (snapshot, anchors, stale_ref_count) = self.resolve_persisted(persisted)?;
        Ok(DivePlan {
            meta: to_meta(row.meta),
            snapshot,
            stale_ref_count,
            anchors,
        })
    }

    /// Describe each of `refs` by the shots its station sits between (#310), index-aligned with
    /// the input. A ref that names no live station yields `None` - the same "it does not place"
    /// answer the save path gets, rather than an error that would fail a whole batch.
    ///
    /// This is the live-pin half of what the `.plnx` has always done on save: the frontend holds
    /// the anchor beside the positional ref, so a survey edit moves the pin with its station
    /// instead of silently rebinding it to whatever slid into that index.
    pub fn plan_anchors_for_refs(
        &self,
        refs: Vec<StationRef>,
    ) -> Result<Vec<Option<StoredAnchor>>, String> {
        validate_anchor_batch(refs.len())?;
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let Some(project) = guard.as_ref() else {
            return Err("No project open.".to_string());
        };
        Ok(refs
            .into_iter()
            .map(|r| to_anchor(project, Some(r)))
            .collect())
    }

    /// Place each anchor against the survey as it stands now (#310), index-aligned with the
    /// input; `None` where it no longer places. The read half of [`Self::plan_anchors_for_refs`],
    /// and the same resolution the load path uses - including the legacy pre-#24 form, since an
    /// anchor the frontend is holding may have come straight out of an old `.plnx`.
    pub fn plan_refs_for_anchors(
        &self,
        anchors: Vec<StoredAnchor>,
    ) -> Result<Vec<Option<StationRef>>, String> {
        validate_anchor_batch(anchors.len())?;
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let Some(project) = guard.as_ref() else {
            return Err("No project open.".to_string());
        };
        Ok(anchors.iter().map(|a| resolve_stored(project, a)).collect())
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
        let json = serde_json::to_string(&persisted)
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

    /// Canonicalize an IPC snapshot's positional refs to stable keys against the open
    /// project (for on-disk persistence).
    fn to_persisted(&self, snap: &DivePlanSnapshot) -> Result<PersistedDivePlanSnapshot, String> {
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let Some(project) = guard.as_ref() else {
            return Err("No project open.".to_string());
        };
        if snap.waypoints.len() > MAX_WAYPOINTS {
            return Err("That plan has too many way points.".to_string());
        }
        if snap.marked_points.len() > MAX_MARKED_POINTS {
            return Err("That plan has too many marked points.".to_string());
        }
        // A route point that no longer anchors is REFUSED, not dropped. Dropping is what the load
        // path does (there is no empty slot in a list), but on save it would be data loss the user
        // is never told about: the frontend rebaselines its dirty check on the snapshot it SENT, so
        // the plan would read "saved" while the file on disk quietly held a shorter route. Refusing
        // keeps the file and the panel in agreement, and matches the engine's own doctrine that a
        // way point is a constraint rather than a hint.
        let waypoints = snap
            .waypoints
            .iter()
            .map(|w| {
                to_anchor(project, Some(*w)).ok_or_else(|| {
                    "A way point in this plan no longer exists in the survey. Remove it from the \
                     route and save again."
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
                        "A marked point in this plan no longer exists in the survey. Remove it and \
                         save again."
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
            // Normalised, not truncated. A segment past `waypoints.len() + 1` describes a span that
            // cannot exist for ANY plan, so unlike a dangling way point (refused above) there is
            // nothing to refuse the write over - but the data in it is still the diver's statement
            // about how a span is flown, and `take()` dropped it in the reach-extending direction.
            // `normalize_segment_count` folds it in instead; way points are capped just above, so
            // the result is `<= MAX_SEGMENTS` by construction and that separate cap can no longer
            // bind here.
            segments: normalize_segment_count(
                snap.segments.clone(),
                snap.waypoints.len(),
                plan_speed_of(&snap.gas_config),
            ),
        })
    }

    /// Resolve a persisted (key) snapshot back to the IPC (`StationRef`) form against the
    /// open project, nulling dangling anchors and counting them into `stale_ref_count`. Also
    /// returns the anchors the surviving refs came from, so the loaded plan's live pins stay
    /// anchor-backed without a second round trip (#310).
    fn resolve_persisted(
        &self,
        p: PersistedDivePlanSnapshot,
    ) -> Result<(DivePlanSnapshot, PlanAnchorSet, u32), String> {
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let Some(project) = guard.as_ref() else {
            return Err("No project open.".to_string());
        };
        let mut stale = 0u32;
        let (origin, origin_anchor) = resolve_keeping_anchor(project, p.origin, &mut stale);
        let (endpoint, endpoint_anchor) = resolve_keeping_anchor(project, p.endpoint, &mut stale);
        // The caps are re-applied HERE, not only on save: the threat they exist for is a
        // hand-edited `.plnx`, which by definition never crosses the write path. Every way point
        // costs the engine a full extra Dijkstra and every marked point a DOM node plus a station
        // lookup, so an unbounded list from a doctored file would be a compute amplifier on every
        // recompute. Truncating (rather than refusing the load) keeps the rest of the plan usable.
        let (waypoints, waypoint_anchors, waypoints_survived) = from_anchors_with_survival(
            project,
            p.waypoints.into_iter().take(MAX_WAYPOINTS).collect(),
            &mut stale,
        );
        // Segments are positional against the PRE-drop way points, so the merge must run against
        // the survival mask above BEFORE anything about waypoints changes shape - see
        // `merge_segments_for_surviving_waypoints`. It runs on the RAW persisted list (any
        // length): the mask describes the input positions, so trimming first would misalign it.
        let plan_speed = plan_speed_of(&p.gas_config);
        let segments =
            merge_segments_for_surviving_waypoints(p.segments, &waypoints_survived, plan_speed);
        // The load side of the `segments.len() == waypoints.len() + 1` invariant (#317), which a
        // plan written before segments existed (zero rows) and a hand-edited file (any number of
        // rows) both violate. Way points are already capped just above, so `waypoints.len() + 1 <=
        // MAX_SEGMENTS` holds by construction and that separate cap can no longer bind here.
        let segments = normalize_segment_count(segments, waypoints.len(), plan_speed);
        // Capped on the way IN like the way points and marked points, and for the same reason:
        // the threat is a hand-edited `.plnx`, which by definition never crosses the write
        // path. One entry per stage cylinder is the real bound (`gas.rs` already truncates the
        // engine side to the plan's cylinder count), so anything past the way-point cap is
        // doctored data, and each entry costs a station lookup on every load.
        let mut stage_drop_overrides = Vec::new();
        let mut stage_drop_anchors = Vec::new();
        for k in p.stage_drop_overrides.into_iter().take(MAX_WAYPOINTS) {
            let (r, a) = resolve_keeping_anchor(project, k, &mut stale);
            stage_drop_overrides.push(r);
            stage_drop_anchors.push(a);
        }
        let mut marked_points = Vec::new();
        let mut marked_point_anchors = Vec::new();
        for m in p.marked_points.into_iter().take(MAX_MARKED_POINTS) {
            let Some(station) = from_anchor(project, &m.station, &mut stale) else {
                continue;
            };
            marked_points.push(MarkedPoint {
                station,
                // Same reasoning as the list caps: the save path rejects an over-long label,
                // but a doctored file bypasses it entirely, so truncate on the way in.
                label: m.label.map(|l| truncate_chars(l, MAX_MARKED_LABEL_CHARS)),
            });
            marked_point_anchors.push(Some(m.station));
        }
        let anchors = PlanAnchorSet {
            origin: origin_anchor,
            endpoint: endpoint_anchor,
            waypoints: waypoint_anchors,
            marked_points: marked_point_anchors,
            stage_drop_overrides: stage_drop_anchors,
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
        Ok((snapshot, anchors, stale))
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "tests build from known-valid literals; a failed unwrap should fail the test"
    )]
    use super::*;
    use crate::models::{SectionData, ShotData};
    use tempfile::TempDir;

    fn make_shot() -> ShotData {
        ShotData {
            length: 10.0,
            ..Default::default()
        }
    }

    /// A one-section project with 3 shots (stations 0..=3) and minted shot UUIDs so
    /// `key_for_ref` can anchor to `AfterShot(uuid)`.
    fn project_3_shots() -> (Project, u32) {
        let mut project = Project::new("Test".to_string(), None);
        let sec = project
            .add_section(SectionData {
                shots: vec![make_shot(), make_shot(), make_shot()],
                ..Default::default()
            })
            .id;
        project.ensure_shot_uuids();
        (project, sec)
    }

    /// A one-section project with 6 shots (stations 0..=6), for the #317 segment-merge test:
    /// it needs headroom for an origin, three way points, and an endpoint - five distinct
    /// stations - without any of them coinciding.
    fn project_6_shots() -> (Project, u32) {
        let mut project = Project::new("Test".to_string(), None);
        let sec = project
            .add_section(SectionData {
                shots: vec![
                    make_shot(),
                    make_shot(),
                    make_shot(),
                    make_shot(),
                    make_shot(),
                    make_shot(),
                ],
                ..Default::default()
            })
            .id;
        project.ensure_shot_uuids();
        (project, sec)
    }

    fn ctx(created_at: &str) -> PlnxContext {
        PlnxContext {
            path: PathBuf::from("/tmp/unused.plnx"),
            project_created_at: created_at.to_string(),
            project_name: "P".to_string(),
        }
    }

    #[test]
    fn classify_ownership_reads_fresh_owned_foreign() {
        assert!(classify_ownership(None, &ctx("A")) == Ownership::Fresh);
        assert!(classify_ownership(Some("A".to_string()), &ctx("A")) == Ownership::Owned);
        assert!(classify_ownership(Some("A".to_string()), &ctx("B")) == Ownership::Foreign);
    }

    #[test]
    fn probe_ownership_matches_classify_ownership_against_a_real_file() {
        let dir = std::env::temp_dir().join(format!("plnx_ownership_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("owned.plnx");
        let _ = std::fs::remove_file(&path);

        // No file yet => Fresh.
        let ctx_a = PlnxContext {
            path: path.clone(),
            project_created_at: "A".to_string(),
            project_name: "P".to_string(),
        };
        assert!(probe_ownership(&ctx_a).unwrap() == Ownership::Fresh);

        // Stamp it as "A"'s file (via the normal open+ensure_identity path).
        {
            let store = PlnxStore::open(&path).unwrap();
            store.ensure_identity("A", "P").unwrap();
        }
        assert!(probe_ownership(&ctx_a).unwrap() == Ownership::Owned);

        let ctx_b = PlnxContext {
            path,
            project_created_at: "B".to_string(),
            project_name: "P".to_string(),
        };
        assert!(probe_ownership(&ctx_b).unwrap() == Ownership::Foreign);

        let _ = std::fs::remove_file(dir.join("owned.plnx"));
        let _ = std::fs::remove_dir(&dir);
    }

    /// Regression guard for the Copilot #256 finding: every command called
    /// `PlnxStore::open()` (which unconditionally runs `init_schema` +
    /// `set_user_version`) BEFORE checking ownership, so a `Foreign` `.plnx` (one
    /// stamped with a different project's identity) got its schema/pragma silently
    /// touched before being rejected. Proves the fix by asserting the file's bytes
    /// are bit-for-bit unchanged after every command rejects it.
    ///
    /// Prove-via-injection: reverting the `probe_ownership`-before-`open` guard in
    /// `list_dive_plans`/`load_dive_plan`/`save_dive_plan`/`delete_dive_plan` (i.e.
    /// restoring the old open-then-check order) makes this test fail the final byte
    /// comparison, since `PlnxStore::open` on an already-v1-schema file still issues
    /// a `PRAGMA user_version = 1` write even when the value doesn't change.
    #[test]
    fn foreign_plnx_is_never_opened_by_the_dive_plan_commands() {
        let temp = TempDir::new().unwrap();
        let stalx = temp.path().join("cave.stalx");
        let plnx_path = temp.path().join("cave.plnx");

        // A .plnx belonging to a DIFFERENT project ("Owner"), with a plan in it.
        {
            let store = PlnxStore::open(&plnx_path).unwrap();
            store
                .ensure_identity("2020-01-01T00:00:00Z", "Owner")
                .unwrap();
            store
                .insert("Owner's plan", "2020-01-01T00:00:00Z", 1, "{}")
                .unwrap();
        }
        let bytes_before = std::fs::read(&plnx_path).unwrap();

        let state = AppState::new().unwrap();
        let mut project = Project::new("Mine".to_string(), None);
        project.file_path = Some(stalx);
        // Project::new stamps a fresh created_at (now), which differs from the
        // "2020-01-01T00:00:00Z" stamped above => Foreign.
        *state.project.lock().unwrap() = Some(project);

        let list = state.list_dive_plans().unwrap();
        assert!(
            list.is_empty(),
            "a Foreign file's plans must never be listed"
        );

        let load_err = state.load_dive_plan(1).unwrap_err();
        assert!(load_err.contains("different project"), "got: {load_err}");

        let snapshot: DivePlanSnapshot = serde_json::from_str("{}").unwrap();
        let save_err = state
            .save_dive_plan(None, "New".to_string(), snapshot)
            .unwrap_err();
        assert!(save_err.contains("different project"), "got: {save_err}");

        let delete_err = state.delete_dive_plan(1).unwrap_err();
        assert!(
            delete_err.contains("different project"),
            "got: {delete_err}"
        );

        let bytes_after = std::fs::read(&plnx_path).unwrap();
        assert_eq!(
            bytes_before, bytes_after,
            "a Foreign .plnx must not be mutated by ANY dive-plan command"
        );
    }

    /// #275 B3: a deferred new project has a RESERVED file_path that was never
    /// written. Its first implicit save may auto-rename the .stalx on collision,
    /// which would strand any .plnx created beside the reserved name. So while
    /// the initial save is deferred, every dive-plan command must refuse (same
    /// error as the no-path case) and write nothing; once the flag clears (first
    /// successful save), saving works against the real path.
    #[test]
    fn deferred_initial_save_blocks_dive_plan_persistence() {
        let temp = TempDir::new().unwrap();
        let stalx = temp.path().join("reserved.stalx");
        let plnx_path = temp.path().join("reserved.plnx");

        let state = AppState::new().unwrap();
        let mut project = Project::new("Mine".to_string(), None);
        project.file_path = Some(stalx);
        *state.project.lock().unwrap() = Some(project);
        state.set_initial_save_deferred_for_tests(true);

        let snapshot: DivePlanSnapshot = serde_json::from_str("{}").unwrap();
        let save_err = state
            .save_dive_plan(None, "New".to_string(), snapshot.clone())
            .unwrap_err();
        assert!(
            save_err.contains("Save the project"),
            "deferred project must refuse dive-plan saves, got: {save_err}"
        );
        assert!(
            !plnx_path.exists(),
            "no .plnx may be written beside a reserved-but-unwritten .stalx"
        );
        // list degrades to empty (never an error the UI must shout about).
        assert!(state.list_dive_plans().unwrap().is_empty());
        // load and delete route through the same plnx_context chokepoint and
        // must refuse identically while deferred (no ID-first side path).
        let load_err = state.load_dive_plan(1).unwrap_err();
        assert!(
            load_err.contains("Save the project"),
            "deferred project must refuse dive-plan loads, got: {load_err}"
        );
        let delete_err = state.delete_dive_plan(1).unwrap_err();
        assert!(
            delete_err.contains("Save the project"),
            "deferred project must refuse dive-plan deletes, got: {delete_err}"
        );

        // After the first real save the flag clears and persistence works.
        state.set_initial_save_deferred_for_tests(false);
        let meta = state
            .save_dive_plan(None, "New".to_string(), snapshot)
            .unwrap();
        assert_eq!(meta.name, "New");
        assert!(plnx_path.exists(), "post-save the sidecar lands normally");
    }

    #[test]
    fn an_anchor_records_the_shots_a_station_sits_between() {
        let (project, sec) = project_3_shots();
        // An interior station is bounded on both sides.
        let pair = |r| match to_anchor(&project, Some(r)) {
            Some(StoredAnchor::Pair(p)) => Some(p),
            _ => None,
        };
        let mid = pair(StationRef::new(sec, 2)).expect("an interior station anchors as a pair");
        assert!(mid.a.is_some() && mid.b.is_some());
        // The ends have one neighbour and one free side.
        let start = pair(StationRef::new(sec, 0)).expect("pair anchor");
        assert!(start.a.is_none() && start.b.is_some());
        let end = pair(StationRef::new(sec, 3)).expect("pair anchor");
        assert!(end.a.is_some() && end.b.is_none());
        // A ref into a non-existent section cannot be anchored.
        assert!(to_anchor(&project, Some(StationRef::new(sec + 99, 1))).is_none());
    }

    #[test]
    fn round_trip_is_identity_when_the_survey_is_unchanged() {
        let (project, sec) = project_3_shots();
        let mut stale = 0;
        for idx in 0..=3 {
            let anchor = to_anchor(&project, Some(StationRef::new(sec, idx)));
            assert_eq!(
                resolve_keeping_anchor(&project, anchor, &mut stale).0,
                Some(StationRef::new(sec, idx))
            );
        }
        assert_eq!(stale, 0);
    }

    /// THE point of the pair form (#24): a reverse renumbers every station in the section, and
    /// every stored reference must come back naming the SAME physical station - because it does
    /// not name a position or a side, it names the two shots the station sits between, and a
    /// reverse does not change what a station sits between.
    #[test]
    fn every_anchor_survives_a_reverse() {
        let (mut project, sec) = project_3_shots();
        let anchors: Vec<_> = (0..=3)
            .map(|i| to_anchor(&project, Some(StationRef::new(sec, i))).unwrap())
            .collect();

        assert!(project.reverse_section(sec, false));

        let mut stale = 0;
        // Station i becomes station N - i, and each anchor must follow it there.
        for (i, anchor) in anchors.iter().enumerate() {
            assert_eq!(
                from_anchor(&project, anchor, &mut stale),
                Some(StationRef::new(sec, 3 - i as u32)),
                "anchor for station {i} must name the same physical station after the reverse"
            );
        }
        assert_eq!(stale, 0, "nothing dangles - the shots all still exist");
    }

    /// Extending a passage is routine, and must not disturb a saved route. The far end of the
    /// section stops being terminal, so this exercises the pair falling back to the saved side.
    #[test]
    fn anchors_survive_a_shot_appended_to_the_end() {
        let (mut project, sec) = project_3_shots();
        let end = to_anchor(&project, Some(StationRef::new(sec, 3))).unwrap();
        let mid = to_anchor(&project, Some(StationRef::new(sec, 2))).unwrap();

        project.add_shot(sec, make_shot()).unwrap();

        let mut stale = 0;
        assert_eq!(
            from_anchor(&project, &end, &mut stale),
            Some(StationRef::new(sec, 3)),
            "the old far end is still station 3; the section simply continues past it"
        );
        assert_eq!(
            from_anchor(&project, &mid, &mut stale),
            Some(StationRef::new(sec, 2))
        );
        assert_eq!(stale, 0);
    }

    /// Splitting a shot inserts a new head before it, so an interior anchor's two neighbours
    /// stop being adjacent. It must still name the same junction - resolution takes the end of
    /// one neighbour that FACES the other, which does not require them to touch.
    #[test]
    fn an_interior_anchor_survives_a_split_between_its_neighbours() {
        let (mut project, sec) = project_3_shots();
        let mid = to_anchor(&project, Some(StationRef::new(sec, 2))).unwrap();
        let shot_id = project.get_section(sec).unwrap().shots[1].id;

        let before = project.get_section(sec).unwrap().shots.len();
        project.split_shot_at_distance(sec, shot_id, 0.5).unwrap();
        assert_eq!(project.get_section(sec).unwrap().shots.len(), before + 1);

        let mut stale = 0;
        assert_eq!(
            from_anchor(&project, &mid, &mut stale),
            Some(StationRef::new(sec, 3)),
            "the junction moved one index along because a shot was inserted before it"
        );
        assert_eq!(stale, 0);
    }

    /// Split Section Here moves the shots past the cut into a new section, so an
    /// anchor recorded against the old section id must follow its shots there.
    /// The split station itself stays the first half's end.
    #[test]
    fn anchors_past_a_section_split_resolve_in_the_second_half() {
        let (mut project, sec) = project_n_shots(5);
        let anchors: Vec<_> = (0..=5)
            .map(|i| to_anchor(&project, Some(StationRef::new(sec, i))).unwrap())
            .collect();

        let second = project
            .split_section_at_station(
                sec,
                2,
                &crate::models::SplitSectionOptions {
                    name_a: "A".to_string(),
                    name_b: "B".to_string(),
                    color_a: "#111111".to_string(),
                    color_b: "#222222".to_string(),
                    comment_side: crate::models::SplitSide::A,
                    datum_side: crate::models::SplitSide::A,
                },
            )
            .unwrap();

        let mut stale = 0;
        for (i, anchor) in anchors.iter().enumerate() {
            let i = i as u32;
            let expected = if i <= 2 {
                StationRef::new(sec, i)
            } else {
                StationRef::new(second, i - 2)
            };
            assert_eq!(
                from_anchor(&project, anchor, &mut stale),
                Some(expected),
                "anchor for station {i}"
            );
        }
        assert_eq!(stale, 0);
    }

    /// Build a section of `n` shots and return `(project, section_id)`.
    fn project_n_shots(n: usize) -> (Project, u32) {
        let mut project = Project::new("Test".to_string(), None);
        let sec = project
            .add_section(SectionData {
                shots: (0..n).map(|_| make_shot()).collect(),
                ..Default::default()
            })
            .id;
        project.ensure_shot_uuids();
        (project, sec)
    }

    /// A terminal anchor must survive a reverse IN BOTH DIRECTIONS.
    ///
    /// It has one neighbour and so cannot be placed by the pair; the side comes from the saved
    /// `after_a`, which a reverse flips. Comparing the section's parity against the saved one is
    /// what corrects it.
    #[test]
    fn terminal_anchors_survive_a_reverse_both_ways() {
        let (mut project, sec) = project_n_shots(3);
        let start = to_anchor(&project, Some(StationRef::new(sec, 0))).unwrap();
        let end = to_anchor(&project, Some(StationRef::new(sec, 3))).unwrap();
        let mut stale = 0;

        assert!(project.reverse_section(sec, false));
        assert_eq!(
            from_anchor(&project, &start, &mut stale),
            Some(StationRef::new(sec, 3)),
            "the entrance is now the far end"
        );
        assert_eq!(
            from_anchor(&project, &end, &mut stale),
            Some(StationRef::new(sec, 0))
        );

        // ...and back again.
        assert!(project.reverse_section(sec, false));
        assert_eq!(
            from_anchor(&project, &start, &mut stale),
            Some(StationRef::new(sec, 0))
        );
        assert_eq!(
            from_anchor(&project, &end, &mut stale),
            Some(StationRef::new(sec, 3))
        );
        assert_eq!(stale, 0);
    }

    /// Deleting the head and then appending puts the anchored shot back at position 0 with a
    /// follower - the SAME observable state a reverse produces, but needing the opposite answer.
    /// Parity is what tells them apart; an earlier rule inferred the side from position alone
    /// and got this one wrong.
    #[test]
    fn a_terminal_anchor_survives_delete_then_append() {
        let (mut project, sec) = project_n_shots(2);
        let end = to_anchor(&project, Some(StationRef::new(sec, 2))).unwrap();

        let head = project.get_section(sec).unwrap().shots[0].id;
        project.remove_shot(sec, head).unwrap();
        project.add_shot(sec, make_shot()).unwrap();

        let mut stale = 0;
        assert_eq!(
            from_anchor(&project, &end, &mut stale),
            Some(StationRef::new(sec, 1)),
            "no reverse happened, so the saved side is read literally"
        );
        assert_eq!(stale, 0);
    }

    /// Prepending pushes every later station up by one, and the anchored station goes with it.
    #[test]
    fn a_terminal_anchor_survives_a_prepend() {
        let (mut project, sec) = project_n_shots(2);
        let start = to_anchor(&project, Some(StationRef::new(sec, 0))).unwrap();

        let first = project.get_section(sec).unwrap().shots[0].id;
        project
            .insert_shot_before(sec, first, make_shot(), None, None)
            .unwrap();

        let mut stale = 0;
        assert_eq!(
            from_anchor(&project, &start, &mut stale),
            Some(StationRef::new(sec, 1)),
            "the old entrance is now station 1; a new one sits in front of it"
        );
        assert_eq!(stale, 0);
    }

    /// Splitting the shot a terminal anchor names must FAIL SAFE, not guess.
    ///
    /// The split keeps the original UUID on the shortened tail, so afterwards no shot in the
    /// file has that station as an endpoint - nothing can name it, and any answer would be a
    /// guess that silently lands on the split point. The recorded length is what detects it:
    /// the tail is necessarily shorter, and the station is no longer at an end of the section.
    /// The reference is reported STALE so the UI can ask for a re-pin, which is a warning the
    /// diver can act on rather than a route quietly re-priced against the wrong station.
    #[test]
    fn splitting_the_anchored_end_shot_reports_stale_rather_than_guessing() {
        let (mut project, sec) = project_n_shots(2);
        let start = to_anchor(&project, Some(StationRef::new(sec, 0))).unwrap();

        let first = project.get_section(sec).unwrap().shots[0].id;
        project.split_shot_at_distance(sec, first, 0.5).unwrap();

        let mut stale = 0;
        assert_eq!(
            from_anchor(&project, &start, &mut stale),
            None,
            "the station cannot be named after the split, so it must not be guessed"
        );
        assert_eq!(stale, 1, "and the caller is told, so the UI can warn");
    }

    /// The guard must not fire on an ordinary extension: a prepend leaves the anchored shot's
    /// length alone, so the anchor keeps resolving even though its station is no longer
    /// terminal. Without the length check this shape would be indistinguishable from a split.
    #[test]
    fn extending_past_a_terminal_anchor_does_not_make_it_stale() {
        let (mut project, sec) = project_n_shots(2);
        let start = to_anchor(&project, Some(StationRef::new(sec, 0))).unwrap();

        let first = project.get_section(sec).unwrap().shots[0].id;
        project
            .insert_shot_before(sec, first, make_shot(), None, None)
            .unwrap();

        let mut stale = 0;
        assert_eq!(
            from_anchor(&project, &start, &mut stale),
            Some(StationRef::new(sec, 1)),
            "the shot was not split, so the station is still nameable"
        );
        assert_eq!(stale, 0);
    }

    /// A shot-less section is one station and no shots - what a GPX waypoint imports as. It has
    /// no neighbours to be named by, and that is a complete description rather than a failure.
    ///
    /// The old `SectionKey::SectionStart` form persisted it fine, so anchoring it as
    /// unresolvable would have meant a plan SILENTLY dropping an origin or endpoint on a
    /// waypoint, and outright refusing a way point or marked point on one - losing data purely
    /// by being saved after an upgrade.
    #[test]
    fn a_shot_less_section_anchors_its_only_station() {
        let mut project = Project::new("Test".to_string(), None);
        let sec = project
            .add_section(SectionData {
                shots: Vec::new(),
                ..Default::default()
            })
            .id;

        let anchor = to_anchor(&project, Some(StationRef::new(sec, 0)))
            .expect("a waypoint's only station must be anchorable");
        let mut stale = 0;
        assert_eq!(
            from_anchor(&project, &anchor, &mut stale),
            Some(StationRef::new(sec, 0))
        );
        assert_eq!(stale, 0);
    }

    /// ...and it round-trips through a whole persisted plan, which is the path that used to
    /// drop it: the origin/endpoint null out silently and a way point on it is REFUSED.
    #[test]
    fn a_plan_on_a_shot_less_section_round_trips() {
        let mut project = Project::new("Test".to_string(), None);
        let sec = project
            .add_section(SectionData {
                shots: Vec::new(),
                ..Default::default()
            })
            .id;
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project);

        let only = StationRef::new(sec, 0);
        let snapshot = DivePlanSnapshot {
            origin: Some(only),
            endpoint: Some(only),
            waypoints: vec![only],
            diver: default_persisted_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
            marked_points: vec![MarkedPoint {
                station: only,
                label: Some("wp".to_string()),
            }],
            segments: Vec::new(),
        };

        let persisted = state
            .to_persisted(&snapshot)
            .expect("a way point on a shot-less section must not be refused");
        let (back, _anchors, stale) = state.resolve_persisted(persisted).unwrap();
        assert_eq!(stale, 0);
        assert_eq!(back.origin, Some(only));
        assert_eq!(back.endpoint, Some(only));
        assert_eq!(back.waypoints, vec![only]);
        assert_eq!(back.marked_points[0].station, only);
    }

    /// A single-shot section has two ends that both touch the same one shot, so the pair cannot
    /// tell them apart - but the section's parity can, and that is enough.
    ///
    /// This used to be an accepted inaccuracy: with only a traversal-relative side recorded, a
    /// reverse flipped the meaning and nothing detected it. Recording the orientation alongside
    /// closes it, so the case now resolves correctly rather than being documented as wrong.
    #[test]
    fn a_single_shot_sections_ends_survive_a_reverse() {
        let mut project = Project::new("Test".to_string(), None);
        let sec = project
            .add_section(SectionData {
                shots: vec![make_shot()],
                ..Default::default()
            })
            .id;
        project.ensure_shot_uuids();

        let start = to_anchor(&project, Some(StationRef::new(sec, 0))).unwrap();
        let end = to_anchor(&project, Some(StationRef::new(sec, 1))).unwrap();
        let mut stale = 0;
        assert_eq!(
            from_anchor(&project, &start, &mut stale),
            Some(StationRef::new(sec, 0))
        );
        assert_eq!(
            from_anchor(&project, &end, &mut stale),
            Some(StationRef::new(sec, 1))
        );

        // Reversing a one-shot section swaps its two stations, and both anchors follow.
        assert!(project.reverse_section(sec, false));
        assert_eq!(
            from_anchor(&project, &start, &mut stale),
            Some(StationRef::new(sec, 1))
        );
        assert_eq!(
            from_anchor(&project, &end, &mut stale),
            Some(StationRef::new(sec, 0))
        );
        assert_eq!(stale, 0);
    }

    #[test]
    fn persisted_key_tracks_the_same_physical_station_across_a_shot_deletion() {
        // THE headline regression (#2): a saved ref must follow its station across a survey
        // edit, not silently rebind to whatever is now at the old index.
        let (mut project, sec) = project_3_shots();
        // Station 2 = the boundary after shot index 1.
        let key = to_anchor(&project, Some(StationRef::new(sec, 2))).expect("stable anchor");
        // Delete shot 0: the physical station that was at index 2 is now at index 1.
        project.sections[0].shots.remove(0);
        let mut stale = 0;
        let resolved = from_anchor(&project, &key, &mut stale);
        assert_eq!(stale, 0);
        // Follows the station to its new index - NOT the old positional index 2.
        assert_eq!(resolved, Some(StationRef::new(sec, 1)));
    }

    #[test]
    fn dangling_key_after_a_section_delete_is_nulled_and_counted() {
        let (mut project, sec) = project_3_shots();
        let key = to_anchor(&project, Some(StationRef::new(sec, 2))).expect("stable anchor");
        project.sections.clear(); // the whole section (and its anchoring shot) is gone
        let mut stale = 0;
        assert_eq!(from_anchor(&project, &key, &mut stale), None);
        assert_eq!(stale, 1);
    }

    #[test]
    fn from_key_leaves_none_refs_untouched_and_uncounted() {
        let (project, _sec) = project_3_shots();
        let mut stale = 0;
        assert_eq!(
            resolve_keeping_anchor(&project, None, &mut stale),
            (None, None)
        );
        assert_eq!(stale, 0);
    }

    /// A snapshot carrying way points and marked points, for the #298 persistence tests.
    fn snapshot_with_route_points(sec: u32) -> DivePlanSnapshot {
        DivePlanSnapshot {
            origin: Some(StationRef::new(sec, 0)),
            endpoint: Some(StationRef::new(sec, 3)),
            waypoints: vec![StationRef::new(sec, 2)],
            diver: default_persisted_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
            marked_points: vec![MarkedPoint {
                station: StationRef::new(sec, 2),
                label: Some("photo".to_string()),
            }],
            segments: Vec::new(),
        }
    }

    #[test]
    fn waypoints_and_marked_points_track_their_station_across_a_shot_deletion() {
        // Same headline guarantee the origin/endpoint already had (#244 §2): a saved route point is
        // an anchor, not an index, so a survey edit must move it WITH its physical station rather
        // than silently rebinding it to whatever now sits at the old index. A way point that
        // rebinds is worse than a stale origin - it re-routes the dive.
        let (mut project, sec) = project_3_shots();
        let state = AppState::new().unwrap();
        let persisted = {
            *state.project.lock().unwrap() = Some(project.clone());
            state
                .to_persisted(&snapshot_with_route_points(sec))
                .unwrap()
        };

        // Delete shot 0: the physical station that was at index 2 is now at index 1.
        project.sections[0].shots.remove(0);
        *state.project.lock().unwrap() = Some(project);
        let (snapshot, _anchors, stale) = state.resolve_persisted(persisted).unwrap();

        // The plan's ORIGIN was the section's first station, and deleting the first shot deletes
        // that station outright - so it is correctly reported gone rather than quietly re-bound.
        // The old `SectionStart` form could not say that: it resolved to index 0 whatever now
        // sat there, silently moving the dive's start one station down the passage. Exactly the
        // rebinding this test's own premise calls out.
        assert_eq!(stale, 1, "the origin's station no longer exists");
        assert_eq!(snapshot.origin, None);
        assert_eq!(snapshot.waypoints, vec![StationRef::new(sec, 1)]);
        assert_eq!(snapshot.marked_points.len(), 1);
        assert_eq!(snapshot.marked_points[0].station, StationRef::new(sec, 1));
        assert_eq!(snapshot.marked_points[0].label.as_deref(), Some("photo"));
    }

    /// The reverse renumbers every station, and a plan saved beforehand must come back naming
    /// exactly the stations the user picked (#24).
    ///
    /// Under the old `AfterShot(uuid)` form this was the one survey edit whose damage was
    /// silent: the key still resolved, onto the station next door, so `stale_ref_count` stayed 0
    /// and nothing warned while the route - and the gas plan derived from it - had been re-priced
    /// against different stations. An anchor that names the shots a station sits BETWEEN has
    /// nothing to go wrong: a reverse does not change what a station sits between.
    #[test]
    fn a_reverse_leaves_a_saved_plan_pointing_at_the_same_stations() {
        let (mut project, sec) = project_3_shots();
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project.clone());
        let persisted = state
            .to_persisted(&snapshot_with_route_points(sec))
            .unwrap();

        assert!(project.reverse_section(sec, false));
        *state.project.lock().unwrap() = Some(project);

        let (snapshot, _anchors, stale) = state.resolve_persisted(persisted).unwrap();
        assert_eq!(stale, 0);
        // The plan was saved with origin 0, endpoint 3, way point 2 and a marked point at 2.
        // Those physical stations are now numbered 3, 0, 1 and 1.
        assert_eq!(snapshot.origin, Some(StationRef::new(sec, 3)));
        assert_eq!(snapshot.endpoint, Some(StationRef::new(sec, 0)));
        assert_eq!(snapshot.waypoints, vec![StationRef::new(sec, 1)]);
        assert_eq!(snapshot.marked_points[0].station, StationRef::new(sec, 1));
        assert_eq!(snapshot.marked_points[0].label.as_deref(), Some("photo"));
    }

    /// End to end through the real `.plnx`: the command path has to reach the sidecar,
    /// because `Project::for_each_station_ref` (which the reverse's own ref rewrite walks)
    /// enumerates merges, closures, datums, offsets and comments - not the plan, which
    /// lives outside the project entirely. And it has to reach it on the way BACK too:
    /// undoing a reverse re-reverses the numbering.
    #[test]
    fn a_saved_plan_follows_its_stations_through_a_reverse_and_its_undo() {
        let temp = TempDir::new().unwrap();
        let (mut project, sec) = project_3_shots();
        project.file_path = Some(temp.path().join("cave.stalx"));
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project);

        let saved = state
            .save_dive_plan(None, "Plan".to_string(), snapshot_with_route_points(sec))
            .unwrap();

        let result = state.execute(crate::core::Command::ReverseSection {
            section_id: sec,
            swap_left_right: false,
        });
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            result.reversed_sections,
            Some(vec![crate::core::ReversedSection {
                section_id: sec,
                shot_count: 3,
            }])
        );

        let plan = state.load_dive_plan(saved.id).unwrap();
        assert_eq!(plan.stale_ref_count, 0);
        assert_eq!(plan.snapshot.origin, Some(StationRef::new(sec, 3)));
        assert_eq!(plan.snapshot.endpoint, Some(StationRef::new(sec, 0)));
        assert_eq!(plan.snapshot.waypoints, vec![StationRef::new(sec, 1)]);
        assert_eq!(
            plan.snapshot.marked_points[0].station,
            StationRef::new(sec, 1)
        );
        // The plan still names the same physical stations, so this is not a user edit
        // and must not surface as one in the plan list.
        assert_eq!(plan.meta.updated_at, saved.updated_at);

        let undone = state.undo();
        assert!(undone.success, "{:?}", undone.error);
        assert_eq!(
            undone.reversed_sections,
            Some(vec![crate::core::ReversedSection {
                section_id: sec,
                shot_count: 3,
            }])
        );
        let plan = state.load_dive_plan(saved.id).unwrap();
        assert_eq!(plan.stale_ref_count, 0);
        assert_eq!(plan.snapshot.origin, Some(StationRef::new(sec, 0)));
        assert_eq!(plan.snapshot.endpoint, Some(StationRef::new(sec, 3)));
        assert_eq!(plan.snapshot.waypoints, vec![StationRef::new(sec, 2)]);
    }

    #[test]
    fn dangling_waypoints_and_marked_points_are_dropped_and_counted() {
        // Unlike the single origin/endpoint slots there is nothing to null out, so a dangling entry
        // leaves the list entirely - and MUST, because the engine treats an unresolvable way point
        // as a hard stall that would blank the whole field on load. It still counts as stale so the
        // UI can warn that the loaded plan is not what was saved.
        let (mut project, sec) = project_3_shots();
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project.clone());
        let persisted = state
            .to_persisted(&snapshot_with_route_points(sec))
            .unwrap();

        project.sections.clear(); // every anchoring shot is gone
        *state.project.lock().unwrap() = Some(project);
        let (snapshot, _anchors, stale) = state.resolve_persisted(persisted).unwrap();

        assert!(snapshot.waypoints.is_empty());
        assert!(snapshot.marked_points.is_empty());
        // origin + endpoint + way point + marked point all dangled.
        assert_eq!(stale, 4);
    }

    #[test]
    fn save_refuses_a_route_point_that_no_longer_anchors_rather_than_dropping_it() {
        // Silently dropping was the original behaviour and it was data loss the user could not see:
        // the frontend rebaselines its dirty check on the snapshot it SENT, so the plan would read
        // "saved" while the file held a shorter route. Refusing keeps file and panel in agreement.
        let temp = TempDir::new().unwrap();
        let (mut project, sec) = project_3_shots();
        project.file_path = Some(temp.path().join("cave.stalx"));
        let state = AppState::new().unwrap();

        // A way point on a section that is then deleted cannot be anchored at save time.
        let mut snapshot = snapshot_with_route_points(sec);
        snapshot.waypoints = vec![StationRef::new(sec + 99, 1)];
        *state.project.lock().unwrap() = Some(project.clone());
        let err = state
            .save_dive_plan(None, "Plan".to_string(), snapshot)
            .unwrap_err();
        assert!(err.contains("way point"), "got: {err}");

        // Same for a marked point, with its own message.
        let mut snapshot = snapshot_with_route_points(sec);
        snapshot.waypoints = Vec::new();
        snapshot.marked_points = vec![MarkedPoint {
            station: StationRef::new(sec + 99, 1),
            label: None,
        }];
        let err = state
            .save_dive_plan(None, "Plan".to_string(), snapshot)
            .unwrap_err();
        assert!(err.contains("marked point"), "got: {err}");

        // Sanity: the same snapshot with anchorable points saves fine, so the refusals above are
        // about the dangling ref and not about some unrelated guard.
        project.ensure_shot_uuids();
        *state.project.lock().unwrap() = Some(project);
        assert!(state
            .save_dive_plan(None, "Plan".to_string(), snapshot_with_route_points(sec))
            .is_ok());
    }

    #[test]
    fn over_long_lists_and_labels_are_capped_on_the_way_in() {
        // The caps exist for a hand-edited `.plnx`, which by definition never crosses the save
        // path - so the LOAD path has to enforce them too. Way points are the expensive one: each
        // costs the engine a full extra Dijkstra on every recompute.
        let (project, sec) = project_3_shots();
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project.clone());
        let key = to_anchor(&project, Some(StationRef::new(sec, 2))).unwrap();

        let persisted = PersistedDivePlanSnapshot {
            origin: None,
            endpoint: None,
            waypoints: vec![key.clone(); MAX_WAYPOINTS + 25],
            diver: default_persisted_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
            marked_points: vec![
                PersistedMarkedPoint {
                    station: key,
                    label: Some("x".repeat(MAX_MARKED_LABEL_CHARS + 500)),
                };
                MAX_MARKED_POINTS + 25
            ],
            segments: Vec::new(),
        };
        let (snapshot, _anchors, _stale) = state.resolve_persisted(persisted).unwrap();

        assert_eq!(snapshot.waypoints.len(), MAX_WAYPOINTS);
        assert_eq!(snapshot.marked_points.len(), MAX_MARKED_POINTS);
        assert_eq!(
            snapshot.marked_points[0]
                .label
                .as_ref()
                .unwrap()
                .chars()
                .count(),
            MAX_MARKED_LABEL_CHARS
        );
    }

    #[test]
    fn a_pre_298_plnx_blob_still_loads() {
        // The claim "no schema bump needed" rests on the ON-DISK struct's serde defaults, not the
        // IPC one - `load_dive_plan` parses the blob as PersistedDivePlanSnapshot. Deserialize a
        // blob with neither new field to pin that directly, so dropping a `#[serde(default)]` there
        // cannot make every existing user's plan unloadable with a green suite.
        let json = serde_json::json!({
            "origin": null,
            "endpoint": null,
            "diver": { "rmv_l_per_min": 20.0, "label": null },
            "cylinders": [],
            "gas_config": serde_json::to_value(GasConfig::default()).unwrap(),
            "stage_drop_overrides": []
        })
        .to_string();
        let persisted: PersistedDivePlanSnapshot = serde_json::from_str(&json).unwrap();
        assert!(persisted.waypoints.is_empty());
        assert!(persisted.marked_points.is_empty());
        // Same claim holds for #317's addition: the blob predates segments too, and it must not
        // become unloadable either.
        assert!(persisted.segments.is_empty());
    }

    /// A `.plnx` written before #24 stores `{section_id, anchor}` and must keep working. The
    /// pair form's fields all default, so without the untagged ordering it would swallow a
    /// legacy row and silently blank every reference in it - the whole plan, on upgrade.
    #[test]
    fn a_legacy_station_key_blob_still_resolves() {
        let (project, sec) = project_3_shots();
        let uuid = project.get_section(sec).unwrap().shots[1]
            .uuid
            .clone()
            .unwrap();
        // Exactly the shape the old writer produced.
        let json = serde_json::json!({
            "origin": { "section_id": sec, "anchor": "SectionStart" },
            "endpoint": { "section_id": sec, "anchor": { "AfterShot": uuid } },
            "waypoints": [],
            "diver": { "rmv_l_per_min": 20.0, "label": null },
            "cylinders": [],
            "gas_config": serde_json::to_value(GasConfig::default()).unwrap(),
            "stage_drop_overrides": [],
            "marked_points": []
        })
        .to_string();

        let persisted: PersistedDivePlanSnapshot = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(persisted.origin, Some(StoredAnchor::Legacy(_))),
            "a legacy row must be recognised as legacy, not silently read as an empty pair"
        );

        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project);
        let (snapshot, _anchors, stale) = state.resolve_persisted(persisted).unwrap();
        assert_eq!(stale, 0, "an upgrade must not strand anyone's route");
        assert_eq!(snapshot.origin, Some(StationRef::new(sec, 0)));
        assert_eq!(snapshot.endpoint, Some(StationRef::new(sec, 2)));
    }

    /// A pair-form row must NOT be mistaken for a legacy one.
    #[test]
    fn a_pair_anchor_round_trips_through_json() {
        let (project, sec) = project_3_shots();
        let anchor = to_anchor(&project, Some(StationRef::new(sec, 2))).unwrap();
        let json = serde_json::to_string(&anchor).unwrap();
        let back: StoredAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, anchor);
        assert!(matches!(back, StoredAnchor::Pair(_)));
    }

    #[test]
    fn over_long_marked_point_label_is_rejected() {
        // The UI caps its input; this is the trusted-write-path guard against a hand-edited .plnx
        // (mirrors the plan-name cap).
        let temp = TempDir::new().unwrap();
        let (mut project, sec) = project_3_shots();
        project.file_path = Some(temp.path().join("cave.stalx"));
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project);

        let mut snapshot = snapshot_with_route_points(sec);
        snapshot.marked_points[0].label = Some("x".repeat(MAX_MARKED_LABEL_CHARS + 1));
        let err = state
            .save_dive_plan(None, "Plan".to_string(), snapshot.clone())
            .unwrap_err();
        assert!(err.contains("label is too long"), "got: {err}");

        // Exactly at the cap is fine - the boundary is inclusive.
        snapshot.marked_points[0].label = Some("x".repeat(MAX_MARKED_LABEL_CHARS));
        assert!(state
            .save_dive_plan(None, "Plan".to_string(), snapshot)
            .is_ok());
    }

    #[test]
    fn segments_round_trip_when_the_survey_is_unchanged() {
        // Unlike origin/endpoint/waypoints, a segment carries no station of its own (#317 - see
        // the model doc comment), so with nothing dangling there is nothing to merge: the list
        // must pass through save+load unchanged.
        let (project, sec) = project_3_shots();
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project);

        let mut snapshot = snapshot_with_route_points(sec);
        snapshot.segments = vec![
            PlanSegment {
                speed_m_per_min: Some(12.0),
                on_scooter: true,
            },
            PlanSegment {
                speed_m_per_min: Some(20.0),
                on_scooter: false,
            },
        ];
        let persisted = state.to_persisted(&snapshot).unwrap();
        let (resolved, _anchors, stale) = state.resolve_persisted(persisted).unwrap();

        assert_eq!(stale, 0);
        assert_eq!(resolved.segments, snapshot.segments);
    }

    #[test]
    fn dropping_a_waypoint_merges_its_two_adjacent_segments_to_the_slower_speed_and_ors_the_flag() {
        // THE #317 headline case. Segments are positional against way points, so dropping way
        // point `j` collapses the spans on either side of it (segments `j` and `j+1`) into one.
        // The merge must go the direction that can only cost MORE on both fields - the slower of
        // the two speeds (dearer gas), and scootered if EITHER half was (spends range, and range
        // is a cap, so spending it can only pull the turn point closer).
        //
        // The two segments to merge are built so neither "take the first", "take the second",
        // "max speed" nor "AND the flag" reproduces the correct answer: only min(speed) + (a || b)
        // lands on {8.0, true}.
        let (mut project, sec) = project_6_shots();
        let state = AppState::new().unwrap();

        let snapshot = DivePlanSnapshot {
            origin: Some(StationRef::new(sec, 0)),
            endpoint: Some(StationRef::new(sec, 6)),
            waypoints: vec![
                StationRef::new(sec, 1),
                StationRef::new(sec, 3),
                StationRef::new(sec, 5),
            ],
            diver: default_persisted_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
            marked_points: Vec::new(),
            segments: vec![
                PlanSegment {
                    speed_m_per_min: Some(20.0),
                    on_scooter: true,
                }, // origin -> waypoint 0
                PlanSegment {
                    speed_m_per_min: Some(8.0),
                    on_scooter: false,
                }, // waypoint 0 -> waypoint 1 (merges away)
                PlanSegment {
                    speed_m_per_min: Some(15.0),
                    on_scooter: true,
                }, // waypoint 1 -> waypoint 2 (merges away)
                PlanSegment {
                    speed_m_per_min: Some(30.0),
                    on_scooter: true,
                }, // waypoint 2 -> endpoint
            ],
        };

        *state.project.lock().unwrap() = Some(project.clone());
        let persisted = state.to_persisted(&snapshot).unwrap();

        // Make way point 1's anchor unplaceable. Since #24 an anchor stores the PAIR of shots it
        // sits between and survives losing either one, so both of station 3's neighbours (shot
        // indices 2 and 3) have to go - the shape `resolve_anchor` reports as `(None, None)`.
        // Way points 0 and 2 keep at least one of their own neighbours and still place.
        project.sections[0].shots.remove(3);
        project.sections[0].shots.remove(2);
        *state.project.lock().unwrap() = Some(project);
        let (resolved, _anchors, stale) = state.resolve_persisted(persisted).unwrap();

        assert_eq!(stale, 1, "only way point 1's anchor should dangle");
        assert_eq!(resolved.waypoints.len(), 2);
        assert_eq!(resolved.segments.len(), 3);
        assert_eq!(
            resolved.segments[0],
            PlanSegment {
                speed_m_per_min: Some(20.0),
                on_scooter: true,
            },
            "the untouched leading segment must pass through unchanged"
        );
        assert_eq!(
            resolved.segments[1],
            PlanSegment {
                speed_m_per_min: Some(8.0),
                on_scooter: true,
            },
            "the merged span must take the SLOWER speed and be scootered if EITHER half was"
        );
        assert_eq!(
            resolved.segments[2],
            PlanSegment {
                speed_m_per_min: Some(30.0),
                on_scooter: true,
            },
            "the untouched trailing segment must pass through unchanged"
        );
    }

    /// The flag on its own, with the speeds held equal so nothing else can carry the result.
    ///
    /// `on_scooter` only ever spends the rig's usable RANGE, and range is a cap
    /// (`scootered <= limit`), so less spend means more reach. ANDing the flag was therefore the
    /// reach-EXTENDING direction dressed up as the safe one: the merged span below keeps its 55
    /// m/min - identical gas - while dropping the whole first span's range spend, so the plan
    /// reaches further than either half allowed.
    #[test]
    fn a_merged_span_is_scootered_when_either_half_was() {
        let merged = merge_segments_for_surviving_waypoints(
            vec![
                PlanSegment {
                    speed_m_per_min: Some(55.0),
                    on_scooter: true,
                },
                PlanSegment {
                    speed_m_per_min: Some(55.0),
                    on_scooter: false,
                },
            ],
            // The single way point between them no longer resolves, so the two spans collapse.
            &[false],
            10.0,
        );
        assert_eq!(
            merged,
            vec![PlanSegment {
                speed_m_per_min: Some(55.0),
                on_scooter: true,
            }],
            "with the gas unchanged, dropping the range spend can only extend reach"
        );
    }

    /// The merge and the engine must agree on which stored speeds are REAL.
    ///
    /// `resolve_segments` honours only `[MIN_SEGMENT_SPEED_M_PER_MIN, MAX_SEGMENT_SPEED_M_PER_MIN]`
    /// and flies anything else at the plan speed. A merge that ranked speeds on a looser domain
    /// would hand a denormal from a hand-edited `.plnx` the "slowest span" prize and keep it - and
    /// the engine would then discard it and fly the merged span at the plan's 10 m/min, FASTER than
    /// either half, which is the exact direction the merge exists to prevent.
    #[test]
    fn a_speed_the_engine_will_not_honour_never_wins_the_merge() {
        let merged = merge_segments_for_surviving_waypoints(
            vec![
                PlanSegment {
                    // Below MIN_SEGMENT_SPEED_M_PER_MIN: the engine flies this span at 10.0.
                    speed_m_per_min: Some(1e-320),
                    on_scooter: false,
                },
                PlanSegment {
                    speed_m_per_min: Some(3.0),
                    on_scooter: false,
                },
            ],
            &[false],
            10.0,
        );
        assert_eq!(
            merged.len(),
            1,
            "the way point dangled, so the two spans collapse"
        );
        assert_eq!(
            merged[0].speed_m_per_min,
            Some(3.0),
            "3 m/min is the slowest speed either half will ACTUALLY be flown at"
        );
    }

    #[test]
    fn an_over_long_segment_list_is_folded_into_the_closing_span_not_truncated() {
        // A hand-edited `.plnx` (and a caller that never touches a file) can carry any number of
        // segments; a plan has exactly `waypoints.len() + 1` spans. Both directions have to settle
        // that, and neither may do it by dropping the excess: every field a truncate discards is
        // discarded in the reach-extending direction - a dropped slow speed becomes the plan speed
        // (cheaper gas), a dropped `on_scooter` stops spending range.
        //
        // So the tail here carries values DISTINCT from the spans that survive: the two retained
        // spans inherit (10 m/min, no scooter), the tail runs at 5 and closes at 3 with the scooter
        // on. A truncate leaves the closing span inheriting; the fold leaves it {3.0, true}.
        let temp = TempDir::new().unwrap();
        let (mut project, sec) = project_3_shots();
        project.file_path = Some(temp.path().join("cave.stalx"));
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project);

        let mut long_list = vec![inherited_segment(), inherited_segment()];
        long_list.extend(std::iter::repeat_n(
            PlanSegment {
                speed_m_per_min: Some(5.0),
                on_scooter: false,
            },
            MAX_SEGMENTS + 24,
        ));
        long_list.push(PlanSegment {
            speed_m_per_min: Some(3.0),
            on_scooter: true,
        });
        let folded = PlanSegment {
            speed_m_per_min: Some(3.0),
            on_scooter: true,
        };

        // Save-side. The snapshot carries one way point, so two spans - and `MAX_SEGMENTS` never
        // gets to bind, because `waypoints.len() + 1` is the tighter line.
        let mut snapshot = snapshot_with_route_points(sec);
        snapshot.segments = long_list.clone();
        let persisted = state.to_persisted(&snapshot).unwrap();
        assert_eq!(persisted.segments.len(), 2);
        assert_eq!(
            persisted.segments[1], folded,
            "the discarded tail's slowest speed and its scooter use must reach the disk"
        );

        // Load-side, which is where a doctored file actually enters.
        let mut doctored = persisted;
        doctored.segments = long_list;
        let (resolved, _anchors, _stale) = state.resolve_persisted(doctored).unwrap();
        assert_eq!(resolved.waypoints.len(), 1);
        assert_eq!(resolved.segments.len(), 2);
        assert_eq!(
            resolved.segments[1], folded,
            "and again on the way back in - a load must not drop it either"
        );
    }

    /// Every plan written before #317 has NO segments at all, and nothing downstream heals that:
    /// the panel's table renders one row per stored segment, the engine re-weights per span, and
    /// the store's add/remove-way-point arithmetic indexes into the list assuming it is already
    /// the right length. So a legacy plan loaded with zero rows silently loses the whole feature
    /// for that plan until the user adds and removes a way point.
    ///
    /// The load path is the only chokepoint every plan passes through, so the invariant is
    /// established there.
    #[test]
    fn a_plan_saved_before_segments_existed_loads_one_segment_per_span() {
        let (project, sec) = project_6_shots();
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project.clone());

        let anchor = |i| to_anchor(&project, Some(StationRef::new(sec, i))).unwrap();
        let persisted = PersistedDivePlanSnapshot {
            origin: Some(anchor(0)),
            endpoint: Some(anchor(6)),
            waypoints: vec![anchor(1), anchor(3), anchor(5)],
            diver: default_persisted_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
            marked_points: Vec::new(),
            // The whole point: a pre-#317 blob has no `segments` key at all, which serde defaults
            // to exactly this.
            segments: Vec::new(),
        };

        let (resolved, _anchors, stale) = state.resolve_persisted(persisted).unwrap();
        assert_eq!(stale, 0);
        assert_eq!(resolved.waypoints.len(), 3);
        assert_eq!(
            resolved.segments.len(),
            4,
            "three way points bound four spans, so a loaded plan must always carry four segments"
        );
        // A padded span must INHERIT the plan speed rather than being frozen at some default the
        // diver never chose - `None` is what the engine reads as "fly this at the plan speed".
        assert!(resolved
            .segments
            .iter()
            .all(|s| s.speed_m_per_min.is_none() && !s.on_scooter));
    }

    /// A short-but-not-empty list is the same defect, and is what a `.plnx` written by a build
    /// between the segment field landing and this normalisation looks like.
    #[test]
    fn a_short_segment_list_is_padded_rather_than_left_misaligned() {
        let (project, sec) = project_6_shots();
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project.clone());

        let anchor = |i| to_anchor(&project, Some(StationRef::new(sec, i))).unwrap();
        let persisted = PersistedDivePlanSnapshot {
            origin: Some(anchor(0)),
            endpoint: Some(anchor(6)),
            waypoints: vec![anchor(1), anchor(3), anchor(5)],
            diver: default_persisted_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
            marked_points: Vec::new(),
            segments: vec![PlanSegment {
                speed_m_per_min: Some(42.0),
                on_scooter: true,
            }],
        };

        let (resolved, _anchors, _stale) = state.resolve_persisted(persisted).unwrap();
        assert_eq!(resolved.segments.len(), 4);
        // Padding appends; the spans the file DID describe keep their values and their positions.
        assert_eq!(resolved.segments[0].speed_m_per_min, Some(42.0));
        assert!(resolved.segments[0].on_scooter);
        assert!(resolved.segments[3].speed_m_per_min.is_none());
    }

    /// An `AppState` holding `project`, for the #310 command-level tests.
    fn state_with(project: Project) -> AppState {
        let state = AppState::new().unwrap();
        *state.project.lock().unwrap() = Some(project);
        state
    }

    #[test]
    fn plan_anchor_round_trip_is_identity_on_an_untouched_project() {
        let (project, sec) = project_3_shots();
        let state = state_with(project);

        let refs: Vec<_> = (0..=3).map(|i| StationRef::new(sec, i)).collect();
        let anchors = state.plan_anchors_for_refs(refs.clone()).unwrap();
        assert!(
            anchors.iter().all(Option::is_some),
            "every live station must be describable"
        );
        let back = state
            .plan_refs_for_anchors(anchors.into_iter().flatten().collect())
            .unwrap();
        assert_eq!(back, refs.into_iter().map(Some).collect::<Vec<_>>());
    }

    /// THE #310 guarantee, at the command boundary this time: a pin the frontend is holding must
    /// follow its physical station across a survey edit rather than rebinding to whatever slid
    /// into the old index.
    ///
    /// The `assert_ne` is the injection: it pins that the naive positional answer - keeping the
    /// ref the frontend already had - is a DIFFERENT station here, so a version of
    /// `plan_refs_for_anchors` that echoed its input would fail rather than pass by coincidence.
    #[test]
    fn an_anchored_pin_follows_its_station_across_an_upstream_shot_delete() {
        let (mut project, sec) = project_3_shots();
        let pinned = StationRef::new(sec, 2);
        let state = state_with(project.clone());
        let anchors = state.plan_anchors_for_refs(vec![pinned]).unwrap();
        let anchor = anchors[0].clone().expect("station 2 anchors");

        // Delete shot 0: the physical station that was station 2 is now station 1.
        project.sections[0].shots.remove(0);
        let state = state_with(project);

        let back = state.plan_refs_for_anchors(vec![anchor]).unwrap();
        assert_eq!(back, vec![Some(StationRef::new(sec, 1))]);
        assert_ne!(
            back[0],
            Some(pinned),
            "the positional ref the frontend was holding now names a different station - if it \
             did not, this test could not tell an anchored pin from an echoed one"
        );
    }

    /// The mirror case: an insert upstream pushes the station DOWN the numbering.
    #[test]
    fn an_anchored_pin_follows_its_station_across_an_upstream_insert() {
        let (mut project, sec) = project_3_shots();
        let pinned = StationRef::new(sec, 2);
        let state = state_with(project.clone());
        let anchor = state.plan_anchors_for_refs(vec![pinned]).unwrap()[0]
            .clone()
            .expect("station 2 anchors");

        let first = project.get_section(sec).unwrap().shots[0].id;
        project
            .insert_shot_before(sec, first, make_shot(), None, None)
            .unwrap();
        let state = state_with(project);

        let back = state.plan_refs_for_anchors(vec![anchor]).unwrap();
        assert_eq!(back, vec![Some(StationRef::new(sec, 3))]);
        assert_ne!(back[0], Some(pinned));
    }

    /// A reverse renumbers the whole section, and the terminal pins are the ones that need the
    /// parity correction (`after_a` alone is expressed in traversal terms, which a reverse
    /// flips). All four stations must come back naming the same physical spot.
    #[test]
    fn anchored_pins_survive_a_section_reverse() {
        let (mut project, sec) = project_3_shots();
        let refs: Vec<_> = (0..=3).map(|i| StationRef::new(sec, i)).collect();
        let state = state_with(project.clone());
        let anchors: Vec<_> = state
            .plan_anchors_for_refs(refs.clone())
            .unwrap()
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(anchors.len(), 4);

        assert!(project.reverse_section(sec, false));
        let state = state_with(project);

        let back = state.plan_refs_for_anchors(anchors).unwrap();
        let expected: Vec<_> = (0..=3).map(|i| Some(StationRef::new(sec, 3 - i))).collect();
        assert_eq!(back, expected);
        assert_ne!(
            back,
            refs.into_iter().map(Some).collect::<Vec<_>>(),
            "a reverse genuinely renumbers, so echoing the input would not produce this"
        );
    }

    /// An anchor names the two shots its station sits between and survives losing either one.
    /// Losing BOTH leaves nothing in the file that can name the station, and the honest answer is
    /// `None` - the frontend drops that pin and tells the diver, rather than being handed a
    /// plausible neighbour.
    #[test]
    fn a_pin_whose_both_neighbouring_shots_are_deleted_no_longer_places() {
        let (mut project, sec) = project_3_shots();
        let state = state_with(project.clone());
        let anchor = state
            .plan_anchors_for_refs(vec![StationRef::new(sec, 2)])
            .unwrap()[0]
            .clone()
            .expect("station 2 anchors");

        // Station 2's neighbours are shot indices 1 and 2; remove the later one first so the
        // earlier index stays valid.
        project.sections[0].shots.remove(2);
        project.sections[0].shots.remove(1);
        let state = state_with(project);

        assert_eq!(
            state.plan_refs_for_anchors(vec![anchor]).unwrap(),
            vec![None]
        );
    }

    /// A ref naming no live station is `None`, not an error - one bad pin in a batch must not
    /// fail the other pins with it.
    #[test]
    fn an_unanchorable_ref_is_none_without_failing_the_batch() {
        let (project, sec) = project_3_shots();
        let state = state_with(project);
        let anchors = state
            .plan_anchors_for_refs(vec![
                StationRef::new(sec, 1),
                StationRef::new(sec + 99, 0),
                StationRef::new(sec, 9),
            ])
            .unwrap();
        assert!(anchors[0].is_some());
        assert!(anchors[1].is_none(), "no such section");
        assert!(anchors[2].is_none(), "no such station in that section");
    }

    /// The anchors the load path already had in hand are handed out with the plan, so opening a
    /// plan seeds the live pins without a second round trip - and index-aligned with the RESOLVED
    /// lists, which is the part that is easy to get wrong when a dangling entry has been dropped.
    #[test]
    fn a_loaded_plan_carries_the_anchors_its_refs_resolved_from() {
        let temp = TempDir::new().unwrap();
        let (mut project, sec) = project_6_shots();
        project.file_path = Some(temp.path().join("cave.stalx"));
        let state = state_with(project);

        let snapshot = DivePlanSnapshot {
            origin: Some(StationRef::new(sec, 0)),
            endpoint: Some(StationRef::new(sec, 6)),
            waypoints: vec![StationRef::new(sec, 1), StationRef::new(sec, 3)],
            diver: default_persisted_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: vec![None, Some(StationRef::new(sec, 5))],
            marked_points: vec![MarkedPoint {
                station: StationRef::new(sec, 4),
                label: None,
            }],
            segments: Vec::new(),
        };
        let saved = state
            .save_dive_plan(None, "Plan".to_string(), snapshot)
            .unwrap();

        let plan = state.load_dive_plan(saved.id).unwrap();
        assert_eq!(plan.stale_ref_count, 0);
        assert!(plan.anchors.origin.is_some());
        assert!(plan.anchors.endpoint.is_some());
        assert_eq!(plan.anchors.waypoints.len(), plan.snapshot.waypoints.len());
        assert_eq!(
            plan.anchors.marked_points.len(),
            plan.snapshot.marked_points.len()
        );
        assert_eq!(
            plan.anchors.stage_drop_overrides.len(),
            plan.snapshot.stage_drop_overrides.len()
        );
        assert!(plan.anchors.stage_drop_overrides[0].is_none());
        assert!(plan.anchors.stage_drop_overrides[1].is_some());

        // The handed-out anchors must be the real thing: placing them again reproduces exactly
        // the refs the snapshot carries.
        let waypoint_anchors: Vec<_> = plan.anchors.waypoints.iter().flatten().cloned().collect();
        assert_eq!(
            state.plan_refs_for_anchors(waypoint_anchors).unwrap(),
            plan.snapshot
                .waypoints
                .iter()
                .copied()
                .map(Some)
                .collect::<Vec<_>>()
        );
    }

    /// A dropped way point must leave BOTH lists together, or every anchor after it names the
    /// wrong pin - the same positional trap the anchors exist to close.
    #[test]
    fn a_dropped_waypoint_leaves_the_anchor_list_aligned() {
        let (mut project, sec) = project_6_shots();
        let anchor = |i| to_anchor(&project, Some(StationRef::new(sec, i))).unwrap();
        let persisted = PersistedDivePlanSnapshot {
            origin: None,
            endpoint: None,
            waypoints: vec![anchor(1), anchor(3), anchor(5)],
            diver: default_persisted_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
            marked_points: Vec::new(),
            segments: Vec::new(),
        };

        // Strand the middle way point: station 3 sits between shot indices 2 and 3, and an anchor
        // survives losing either one, so both have to go.
        project.sections[0].shots.remove(3);
        project.sections[0].shots.remove(2);
        let state = state_with(project);

        let (resolved, anchors, stale) = state.resolve_persisted(persisted).unwrap();
        assert_eq!(stale, 1);
        assert_eq!(resolved.waypoints.len(), 2);
        assert_eq!(anchors.waypoints.len(), 2);
        let placed: Vec<_> = state
            .plan_refs_for_anchors(anchors.waypoints.into_iter().flatten().collect())
            .unwrap();
        assert_eq!(
            placed,
            resolved
                .waypoints
                .iter()
                .copied()
                .map(Some)
                .collect::<Vec<_>>()
        );
    }

    /// Serde must carry EVERY field of the untagged enum across the wire, not just the ones a
    /// default fixture happens to exercise.
    ///
    /// The section is reversed first on purpose: on a virgin section `parity` is `false` and the
    /// two terminal stations are the only ones with a length fingerprint, so a field that failed
    /// to serialize would round-trip to its default and look identical. Reversing sets `parity`
    /// true, and asserting the whole struct then actually pins it. (An earlier version of this
    /// test did not, and `#[serde(skip)]` on `parity` passed it.)
    #[test]
    fn an_anchor_survives_a_plain_serde_round_trip() {
        let (mut project, sec) = project_3_shots();
        assert!(project.reverse_section(sec, false));
        assert!(
            project.get_section(sec).unwrap().reverse_parity,
            "the fixture must actually carry a non-default parity"
        );
        let state = state_with(project);
        for i in 0..=3 {
            let anchor = state
                .plan_anchors_for_refs(vec![StationRef::new(sec, i)])
                .unwrap()[0]
                .clone()
                .expect("anchors");
            let json = serde_json::to_string(&anchor).unwrap();
            let back: StoredAnchor = serde_json::from_str(&json).unwrap();
            assert_eq!(back, anchor, "the untagged enum must round-trip exactly");
            assert_eq!(
                state.plan_refs_for_anchors(vec![back]).unwrap(),
                vec![Some(StationRef::new(sec, i))]
            );
        }
    }

    /// What a `u64` becomes after a trip through JavaScript.
    ///
    /// `JSON.parse` has no integer type, so it widens the value to a double; `JSON.stringify`
    /// then writes the shortest decimal that identifies that double, which for a 64-bit value is
    /// not the value you started with. Rust's own shortest-round-trip float printing produces the
    /// same digits, so this is a faithful stand-in without a JS engine.
    fn through_javascript(bits: u64) -> u64 {
        let as_double = bits as f64;
        format!("{as_double}").parse::<u64>().unwrap_or(bits)
    }

    /// An anchor handed to the frontend and handed straight back must still place.
    ///
    /// The hazard is `terminal_shot_length_bits`: a full 64-bit fingerprint that JavaScript
    /// cannot hold exactly. Comparing it bit-for-bit made a round-tripped terminal anchor report
    /// STALE the moment its section was extended - the diver's pin vanishing with a "re-pin this"
    /// warning after an edit that never touched it. Comparing the decoded LENGTH within a
    /// tolerance is immune, and is a strictly better guard anyway: it detects a split, which
    /// changes the length by at least a centimetre.
    #[test]
    fn an_anchor_survives_the_round_trip_through_javascript() {
        let (mut project, sec) = project_n_shots(2);
        let state = state_with(project.clone());
        let anchor = state
            .plan_anchors_for_refs(vec![StationRef::new(sec, 0)])
            .unwrap()[0]
            .clone()
            .expect("the entrance anchors");

        // Mangle exactly the way a trip through the frontend does.
        let mut value = serde_json::to_value(&anchor).unwrap();
        let field = value
            .get_mut("terminal_shot_length_bits")
            .expect("a terminal anchor records its shot's length");
        let original = field.as_u64().expect("stored as an integer");
        let mangled = through_javascript(original);
        assert_ne!(
            mangled, original,
            "if this ever holds, JavaScript grew a 64-bit integer and the guard below is moot"
        );
        *field = serde_json::Value::from(mangled);
        let returned: StoredAnchor = serde_json::from_value(value).unwrap();

        // Extend the section past the pinned station. This is the ONLY shape that reads the
        // fingerprint at all, and it must resolve: nothing was split.
        let first = project.get_section(sec).unwrap().shots[0].id;
        project
            .insert_shot_before(sec, first, make_shot(), None, None)
            .unwrap();
        let state = state_with(project);

        assert_eq!(
            state.plan_refs_for_anchors(vec![returned]).unwrap(),
            vec![Some(StationRef::new(sec, 1))],
            "a pin must not go stale merely because it was held by the frontend for a moment"
        );
    }

    /// ...and the guard it relies on still fires for the thing it exists to catch. A split is
    /// nine orders of magnitude larger than the tolerance, so widening the comparison cannot have
    /// blunted it.
    #[test]
    fn the_terminal_length_guard_still_catches_a_split_after_a_javascript_round_trip() {
        let (mut project, sec) = project_n_shots(2);
        let state = state_with(project.clone());
        let anchor = state
            .plan_anchors_for_refs(vec![StationRef::new(sec, 0)])
            .unwrap()[0]
            .clone()
            .expect("the entrance anchors");

        let mut value = serde_json::to_value(&anchor).unwrap();
        let field = value.get_mut("terminal_shot_length_bits").unwrap();
        let mangled = through_javascript(field.as_u64().unwrap());
        *field = serde_json::Value::from(mangled);
        let returned: StoredAnchor = serde_json::from_value(value).unwrap();

        let first = project.get_section(sec).unwrap().shots[0].id;
        project.split_shot_at_distance(sec, first, 0.5).unwrap();
        let state = state_with(project);

        assert_eq!(
            state.plan_refs_for_anchors(vec![returned]).unwrap(),
            vec![None],
            "the station cannot be named after the split, so it must not be guessed"
        );
    }

    #[test]
    fn plan_anchor_commands_refuse_when_no_project_is_open() {
        let state = AppState::new().unwrap();
        assert!(state.plan_anchors_for_refs(Vec::new()).is_err());
        assert!(state.plan_refs_for_anchors(Vec::new()).is_err());
    }

    /// Both anchor commands are bounded, like every other list a plan can hold.
    ///
    /// They are the one pair that never had a cap, and each entry costs a section scan under the
    /// project mutex - the lock the map, the data table and every mutation contend for - so an
    /// unbounded batch holds that lock for as long as the caller cares to make it. The bound is
    /// derived from the plan's own caps and doubled for the live-plus-baseline pass the frontend
    /// legitimately makes, so a real plan can never reach it.
    #[test]
    fn the_anchor_commands_refuse_an_over_long_batch() {
        let (project, sec) = project_3_shots();
        let anchor = to_anchor(&project, Some(StationRef::new(sec, 2))).unwrap();
        let state = state_with(project);
        let station = StationRef::new(sec, 2);

        // Exactly at the cap is fine - the boundary is inclusive, and every entry is answered.
        assert_eq!(
            state
                .plan_anchors_for_refs(vec![station; MAX_PLAN_ANCHOR_BATCH])
                .unwrap()
                .len(),
            MAX_PLAN_ANCHOR_BATCH
        );
        assert_eq!(
            state
                .plan_refs_for_anchors(vec![anchor.clone(); MAX_PLAN_ANCHOR_BATCH])
                .unwrap()
                .len(),
            MAX_PLAN_ANCHOR_BATCH
        );

        // One past it is refused outright rather than answered short: the reply is index-aligned
        // with the input, so a truncated answer would silently leave the tail of the plan holding
        // stale pins, indistinguishable from pins that genuinely no longer place.
        let err = state
            .plan_anchors_for_refs(vec![station; MAX_PLAN_ANCHOR_BATCH + 1])
            .unwrap_err();
        assert!(err.contains("Too many plan anchors"), "got: {err}");
        let err = state
            .plan_refs_for_anchors(vec![anchor; MAX_PLAN_ANCHOR_BATCH + 1])
            .unwrap_err();
        assert!(err.contains("Too many plan anchors"), "got: {err}");
    }
}
