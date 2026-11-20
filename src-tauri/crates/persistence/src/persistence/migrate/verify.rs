//! Verification (plan 7.2.7): the migrated project's raw positions against
//! the legacy ones, slot by slot.
//!
//! **Horizontal.** Each component is laid out from its anchor, which holds
//! the legacy position of one slot exactly (a correction or a placement), so
//! a slot can only differ by what the old cascade left between two members
//! of a merge that is now one station. The comparison mirrors the solver's
//! traversal (a BFS from the anchor, a station's lines in line-id order,
//! each line walked outwards from where it was reached) and accumulates, per
//! line, the legacy gaps between the member the traversal arrived on and the
//! member of that line: the bound on how far the line can have moved.
//!
//! - within `1e-6 m x (hops + 1)` (the cascade leaves up to 1e-6 m of dust
//!   per hop; `hops` = junctions between the anchor and the line): equal;
//! - within the accumulated gap and the path holds a gap above the dust: the
//!   expected class [`ExpectedClass::TornMergeCollapsed`];
//! - anything else: a mismatch.
//!
//! **Depth.** Within 1e-9 m, else [`ExpectedClass::SnapAlignmentChanged`]
//! when the station's depth-tie group differs only because a snap's derived
//! alignment differs from its legacy flag, else a mismatch.
//!
//! **Closures Computed View.** See [`verify_adjusted`].

use std::collections::{BTreeMap, VecDeque};

use stalagmight_legacy::LegacySurvey;

use crate::models::survey::{ClosureStrategy, Project, RawPositions, Topology};
use crate::models::{StationId, StationRef};

use super::assemble::Built;
use super::merges::UnionFind;
use super::report::{AdjustedCheck, Axis, ClassStats, ExpectedClass, Mismatch, Verification};

/// The dust the 0.6.0 cascade leaves per merge hop (`OFFSET_DUST_M`).
pub const DUST_PER_HOP_M: f64 = 1e-6;

/// Depth tolerance (depths are means of the same readings).
pub const DEPTH_TOLERANCE_M: f64 = 1e-9;

pub(super) fn verify(built: &Built, legacy: &LegacySurvey) -> Verification {
    let project = &built.project;
    let positions = project.raw_positions();
    let topology = project.topology();
    let lines = line_bounds(built, legacy, topology);
    let changed_ties = changed_tie_groups(built, project, &positions);

    let mut v = Verification::default();
    for (position, line) in project.lines().iter().enumerate() {
        let (hops, gap) = lines.get(position).copied().unwrap_or((1, 0.0));
        let dust = DUST_PER_HOP_M * f64::from(hops);
        for index in 0..line.station_count() {
            let slot = StationRef::new(line.id, index as u32);
            v.slots_checked += 1;
            let legacy_slot = stalagmight_legacy::Slot {
                line_id: slot.line_id,
                station_index: slot.station_index,
            };
            let (Some(new), Some(old)) = (
                positions.slot(project, slot),
                legacy.raw_positions.get(&legacy_slot),
            ) else {
                v.mismatches.push(mismatch(slot, Axis::Missing, f64::INFINITY, 0.0));
                continue;
            };
            let dxy = (new[0] - old[0]).hypot(new[1] - old[1]);
            let dz = (new[2] - old[2]).abs();
            v.max_xy_m = v.max_xy_m.max(dxy);
            v.max_z_m = v.max_z_m.max(dz);
            if !(dxy <= dust) {
                if gap > dust && dxy <= gap + dust {
                    expect(&mut v, ExpectedClass::TornMergeCollapsed, dxy);
                } else {
                    v.mismatches
                        .push(mismatch(slot, Axis::Horizontal, dxy, dust));
                }
            }
            if !(dz <= DEPTH_TOLERANCE_M) {
                let station = topology.station_at(slot);
                if station.is_some_and(|s| changed_ties.contains_key(&s)) {
                    expect(&mut v, ExpectedClass::SnapAlignmentChanged, dz);
                } else {
                    v.mismatches
                        .push(mismatch(slot, Axis::Depth, dz, DEPTH_TOLERANCE_M));
                }
            }
        }
    }
    v.adjusted = verify_adjusted(project, legacy);
    v
}

/// The Closures Computed View comparison (plan 7.2.7): every slot's adjusted
/// position within 1e-6 m of `legacy.adjusted_positions`.
///
/// Not run yet: the station-graph least squares (stream M3, plan 3.7) is not
/// on this branch. TODO(G/M3): once `Project` exposes the adjusted solve
/// (taking the closure-adjustment config the legacy open used,
/// `OpenOptions::closure_adjustment`), compare per slot:
/// - `|adjusted_new(slot).xy - legacy.adjusted_positions[slot].xy| <= 1e-6`
///   and the same for z (depth is never adjusted, so z equals the raw z);
/// - the components the legacy solve reported in `adjustment_failures`
///   (shown measured) must be the ones the new solve falls back on;
/// - expected classes, reported like the raw ones: a slot behind a
///   [`ExpectedClass::TornMergeCollapsed`] merge (its raw position already
///   moved), a component whose legacy least-squares component spanned
///   several migrated components (closures between two datum pieces: the
///   legacy solve held one gauge for both), and a same-line merge demoted to
///   a `Tie` (the legacy solve refused the inconsistent per-line translation
///   and fell back to raw; the new solve skips the tie as across one shot);
/// - anything else is a mismatch, as for the raw view.
pub(super) fn verify_adjusted(_project: &Project, legacy: &LegacySurvey) -> AdjustedCheck {
    if legacy.adjusted_positions.is_none() {
        return AdjustedCheck::NoLegacyPositions;
    }
    AdjustedCheck::AwaitingSolver
}

fn mismatch(slot: StationRef, axis: Axis, displacement_m: f64, tolerance_m: f64) -> Mismatch {
    Mismatch {
        line_id: slot.line_id,
        station_index: slot.station_index,
        axis,
        displacement_m,
        tolerance_m,
    }
}

fn expect(v: &mut Verification, class: ExpectedClass, displacement: f64) {
    let stats: &mut ClassStats = v.expected.entry(class).or_default();
    stats.slots += 1;
    stats.max_displacement_m = stats.max_displacement_m.max(displacement);
}

/// Per line (list position): `(hops, gap)`, where `hops` counts the lines
/// walked from the anchor to this one (1 for a line through the anchor) and
/// `gap` sums the legacy distances between the member each line was reached
/// at and the member of the line before it.
fn line_bounds(built: &Built, legacy: &LegacySurvey, topology: &Topology) -> Vec<(u32, f64)> {
    let project = &built.project;
    let legacy_xy = |slot: StationRef| {
        legacy
            .raw_positions
            .get(&stalagmight_legacy::Slot {
                line_id: slot.line_id,
                station_index: slot.station_index,
            })
            .map(|p| [p[0], p[1]])
    };
    let mut lines: Vec<Option<(u32, f64)>> = vec![None; project.lines().len()];
    // Per station: the slot its position was derived through, and that
    // line's (hops, gap).
    let mut reached: BTreeMap<StationId, (StationRef, u32, f64)> = BTreeMap::new();
    for (c, &anchor_dense) in built.anchor_slots.iter().enumerate() {
        let Some(component) = topology.component(c) else {
            continue;
        };
        let anchor_slot = built.slots.slot(anchor_dense);
        let anchor_ref = StationRef::new(anchor_slot.line_id, anchor_slot.station_index);
        let Some(start) = topology.station_at(anchor_ref) else {
            continue;
        };
        if !component.contains(start) {
            continue;
        }
        reached.insert(start, (anchor_ref, 0, 0.0));
        let mut queue = VecDeque::from([start]);
        while let Some(station) = queue.pop_front() {
            let Some(&(via, hops, gap)) = reached.get(&station) else {
                continue;
            };
            let mut slots = topology.slots_of(station).to_vec();
            slots.sort_unstable();
            for slot in slots {
                let Some(position) = topology.line_position(slot.line_id) else {
                    continue;
                };
                if lines[position].is_some() {
                    continue;
                }
                let step = match (legacy_xy(via), legacy_xy(slot)) {
                    (Some(a), Some(b)) => (a[0] - b[0]).hypot(a[1] - b[1]),
                    _ => 0.0,
                };
                let bound = (hops + 1, gap + step);
                lines[position] = Some(bound);
                let Some(chain) = topology.chain_at(position) else {
                    continue;
                };
                let from = slot.station_index as usize;
                let order = (from + 1..chain.len()).chain((0..from).rev());
                for k in order {
                    let next = chain[k];
                    if let std::collections::btree_map::Entry::Vacant(e) = reached.entry(next) {
                        e.insert((StationRef::new(slot.line_id, k as u32), bound.0, bound.1));
                        queue.push_back(next);
                    }
                }
            }
        }
    }
    lines
        .into_iter()
        .map(|bound| bound.unwrap_or((1, 0.0)))
        .collect()
}

/// Stations whose depth-tie group (stations joined by `Tie` closures and
/// aligned snaps) differs between the derived snap alignment and the legacy
/// `is_aligned` flags. Empty when no snap's alignment differs.
fn changed_tie_groups(
    built: &Built,
    project: &Project,
    positions: &RawPositions,
) -> BTreeMap<StationId, ()> {
    let topology = project.topology();
    let flipped = project.closures.iter().any(|c| {
        c.strategy == ClosureStrategy::Move
            && built.legacy_snap_alignment.get(&c.id).copied()
                != Some(project.closure_is_aligned(positions, c))
    });
    if !flipped {
        return BTreeMap::new();
    }
    let n = topology.stations().len();
    let groups = |legacy_flags: bool| {
        let mut uf = UnionFind::new(n);
        for c in &project.closures {
            let tied = match c.strategy {
                ClosureStrategy::Tie => true,
                ClosureStrategy::Move if legacy_flags => {
                    built.legacy_snap_alignment.get(&c.id).copied().unwrap_or(false)
                }
                ClosureStrategy::Move => project.closure_is_aligned(positions, c),
                _ => false,
            };
            if let (true, Some(a), Some(b)) = (tied, topology.dense(c.a), topology.dense(c.b)) {
                uf.union(a, b);
            }
        }
        let mut size = vec![0usize; n];
        let roots: Vec<usize> = (0..n).map(|d| uf.find(d)).collect();
        for &r in &roots {
            size[r] += 1;
        }
        roots.iter().map(|&r| (r, size[r])).collect::<Vec<_>>()
    };
    let (derived, flagged) = (groups(false), groups(true));
    topology
        .stations()
        .iter()
        .enumerate()
        .filter(|&(d, _)| derived[d] != flagged[d])
        .map(|(_, &s)| (s, ()))
        .collect()
}
