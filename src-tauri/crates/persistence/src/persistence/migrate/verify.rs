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
//! - within `DUST_PER_HOP_M x (hops + 1)` (the cascade declines corrections
//!   up to 1e-6 m per axis per hop, plan 7.2.7; `hops` = junctions between
//!   the anchor and the line): equal;
//! - within the accumulated gap and the path holds a gap above the dust: the
//!   expected class [`ExpectedClass::TornMergeCollapsed`];
//! - anything else: a mismatch.
//!
//! **Depth.** Within 1e-9 m, else [`ExpectedClass::SnapAlignmentChanged`]
//! when the station's depth-tie group differs only because a snap's derived
//! alignment differs from its legacy flag, else a mismatch.
//!
//! **Closures Computed View** (`verify_adjusted`). The migrated project's
//! least-squares view (M3, solved with the config the legacy open used)
//! against `legacy.adjusted_positions`, slot by slot, per least-squares
//! component (lines joined by shared stations and closures, as 0.6.0's
//! merges and closures joined them):
//!
//! - within `ADJUSTED_TOLERANCE_M` plus the slot's raw dust bound
//!   horizontally, and `DEPTH_TOLERANCE_M` in depth: equal;
//! - else, in a component 0.6.0's solve failed with "inconsistent line
//!   translation" that holds a demoted merge (cycle or same-line):
//!   [`ExpectedClass::TieNoLongerFails`] (D11: merges were united before
//!   the across-one-shot check);
//! - else, in a component 0.6.0 showed measured without a failure (every
//!   legacy adjusted slot equals its raw one) that holds a migrated `Tie`:
//!   [`ExpectedClass::TieSolved`] (D10: a `Tie` is eligible);
//! - else, in a component with a raw [`ExpectedClass::TornMergeCollapsed`]
//!   slot: that class;
//! - anything else: a mismatch.
//!
//! The components each solve failed (and showed measured) must be the same,
//! except a legacy translation failure a demoted merge explains (D11).
//! A slot counts once per class (the larger of its two axes' differences
//! when only one axis is out).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use stalagmight_legacy::LegacySurvey;

use crate::models::config::ClosureAdjustmentConfig;
use crate::models::survey::{ClosureStrategy, Project, RawPositions, Topology};
use crate::models::{LineId, StationId, StationRef};

use super::assemble::{Built, TieOrigin};
use super::merges::UnionFind;
use super::report::{
    AdjustedCheck, AdjustedVerification, Axis, ClassStats, ExpectedClass, Mismatch, Verification,
};

/// The dust the 0.6.0 cascade leaves per merge hop: it declines a correction
/// when both `|dx|` and `|dy|` are within `OFFSET_DUST_M` (1e-6 m), so a hop
/// can keep up to `sqrt(2) * 1e-6` m horizontally.
pub const DUST_PER_HOP_M: f64 = 1e-6 * std::f64::consts::SQRT_2;

/// Depth tolerance (depths are means of the same readings).
pub const DEPTH_TOLERANCE_M: f64 = 1e-9;

/// Closures Computed View tolerance (plan 7.2.7), on top of the slot's raw
/// dust bound: the solve on stations matches 0.6.0's within it (M3's D13).
pub const ADJUSTED_TOLERANCE_M: f64 = 1e-6;

/// 0.6.0's error text for a component whose merged lines could not be
/// translated consistently (`adjustment_report.rs`, frozen).
const INCONSISTENT_TRANSLATION: &str =
    "Least squares adjustment produced inconsistent line translation";

pub(super) fn verify(
    built: &Built,
    legacy: &LegacySurvey,
    adjustment: &ClosureAdjustmentConfig,
) -> Verification {
    let project = &built.project;
    let positions = project.raw_positions();
    let topology = project.topology();
    let lines = line_bounds(built, legacy, topology);
    let changed_ties = changed_tie_groups(built, project, &positions);

    let mut v = Verification::default();
    let mut torn_lines: BTreeSet<LineId> = BTreeSet::new();
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
                v.mismatches
                    .push(mismatch(slot, Axis::Missing, f64::INFINITY, 0.0));
                continue;
            };
            if !old.iter().all(|c| c.is_finite()) {
                // The old build had no position to show here (a non-finite
                // reading upstream); the migrated one sanitised it.
                expect(&mut v, ExpectedClass::LegacyNotFinite, 0.0);
                continue;
            }
            let dxy = (new[0] - old[0]).hypot(new[1] - old[1]);
            let dz = (new[2] - old[2]).abs();
            v.max_xy_m = v.max_xy_m.max(dxy);
            v.max_z_m = v.max_z_m.max(dz);
            if dxy.is_nan() || dxy > dust {
                if gap > dust && dxy <= gap + dust {
                    expect(&mut v, ExpectedClass::TornMergeCollapsed, dxy);
                    torn_lines.insert(line.id);
                } else {
                    v.mismatches
                        .push(mismatch(slot, Axis::Horizontal, dxy, dust));
                }
            }
            if dz.is_nan() || dz > DEPTH_TOLERANCE_M {
                let station = topology.station_at(slot);
                if station.is_some_and(|s| changed_ties.contains(&s)) {
                    expect(&mut v, ExpectedClass::SnapAlignmentChanged, dz);
                } else {
                    v.mismatches
                        .push(mismatch(slot, Axis::Depth, dz, DEPTH_TOLERANCE_M));
                }
            }
        }
    }
    v.adjusted = verify_adjusted(built, legacy, &positions, &lines, &torn_lines, adjustment);
    v
}

/// What one least-squares component is, for classifying its differences.
#[derive(Debug, Clone, Default)]
struct SolveFacts {
    lines: Vec<LineId>,
    /// It holds a migrated `Tie` (any origin).
    tie: bool,
    /// It holds a demoted merge (cycle or same-line).
    merge_tie: bool,
    /// 0.6.0 showed every slot of it measured (adjusted == raw).
    legacy_measured: bool,
    legacy_failed: bool,
    /// 0.6.0's solve failed it with "inconsistent line translation".
    legacy_translation_failed: bool,
    new_failed: bool,
    /// One of its slots moved in the raw view (a torn merge collapsed).
    torn: bool,
}

impl SolveFacts {
    fn class(&self) -> Option<ExpectedClass> {
        if self.new_failed {
            None
        } else if self.merge_tie && self.legacy_translation_failed {
            Some(ExpectedClass::TieNoLongerFails)
        } else if self.tie && self.legacy_measured && !self.legacy_failed {
            Some(ExpectedClass::TieSolved)
        } else if self.torn {
            Some(ExpectedClass::TornMergeCollapsed)
        } else {
            None
        }
    }
}

/// The Closures Computed View comparison (module docs).
fn verify_adjusted(
    built: &Built,
    legacy: &LegacySurvey,
    raw: &RawPositions,
    lines: &[(u32, f64)],
    torn_lines: &BTreeSet<LineId>,
    adjustment: &ClosureAdjustmentConfig,
) -> AdjustedCheck {
    let Some(old_adjusted) = legacy.adjusted_positions.as_ref() else {
        return AdjustedCheck::NoLegacyPositions;
    };
    let project = &built.project;
    let adjusted = project.adjusted_positions_from(raw, adjustment);
    let old_slot = |slot: StationRef| stalagmight_legacy::Slot {
        line_id: slot.line_id,
        station_index: slot.station_index,
    };

    // Least-squares components: shot components joined by every closure.
    let topology = project.topology();
    let mut uf = UnionFind::new(topology.components().len());
    for c in &project.closures {
        if let (Some(a), Some(b)) = (topology.component_of(c.a), topology.component_of(c.b)) {
            uf.union(a, b);
        }
    }
    let mut index_of_root: BTreeMap<usize, usize> = BTreeMap::new();
    let mut facts: Vec<SolveFacts> = Vec::new();
    let mut solve_of_line: BTreeMap<LineId, usize> = BTreeMap::new();
    for (c, component) in topology.components().iter().enumerate() {
        let root = uf.find(c);
        let k = *index_of_root.entry(root).or_insert_with(|| {
            facts.push(SolveFacts {
                legacy_measured: true,
                ..SolveFacts::default()
            });
            facts.len() - 1
        });
        for &line in &component.lines {
            facts[k].lines.push(line);
            solve_of_line.insert(line, k);
        }
    }
    let solve_of_station = |s: StationId| {
        topology
            .first_slot(s)
            .and_then(|slot| solve_of_line.get(&slot.line_id).copied())
    };
    for c in &project.closures {
        let Some(origin) = built.tie_origins.get(&c.id) else {
            continue;
        };
        if let Some(k) = solve_of_station(c.a) {
            facts[k].tie = true;
            facts[k].merge_tie |= *origin != TieOrigin::AlignedClosure;
        }
    }
    for line in torn_lines {
        if let Some(&k) = solve_of_line.get(line) {
            facts[k].torn = true;
        }
    }
    let mut v = AdjustedVerification {
        failures_legacy: legacy.adjustment_failures.len(),
        failures_new: adjusted.failures().len(),
        ..AdjustedVerification::default()
    };
    let mut legacy_failed_lines: Vec<(usize, Vec<LineId>)> = Vec::new();
    for failure in &legacy.adjustment_failures {
        if let Some(&k) = failure.line_ids.iter().find_map(|l| solve_of_line.get(l)) {
            facts[k].legacy_failed = true;
            facts[k].legacy_translation_failed |= failure.error == INCONSISTENT_TRANSLATION;
            legacy_failed_lines.push((k, failure.line_ids.clone()));
        }
    }
    for failure in adjusted.failures() {
        if let Some(&k) = failure.lines.iter().find_map(|l| solve_of_line.get(l)) {
            facts[k].new_failed = true;
            if !facts[k].legacy_failed {
                v.failure_mismatches.push(failure.lines.clone());
            }
        }
    }
    // A legacy failure the new solve does not repeat is explained only by a
    // demoted merge behind a translation failure (D11); else both must fail.
    for (k, lines) in legacy_failed_lines {
        let f = &facts[k];
        if !f.new_failed && !(f.merge_tie && f.legacy_translation_failed) {
            v.failure_mismatches.push(lines);
        }
    }
    for line in project.lines() {
        let Some(&k) = solve_of_line.get(&line.id) else {
            continue;
        };
        for index in 0..line.station_count() {
            let slot = old_slot(StationRef::new(line.id, index as u32));
            if legacy
                .adjusted_positions
                .as_ref()
                .and_then(|a| a.get(&slot))
                != legacy.raw_positions.get(&slot)
            {
                facts[k].legacy_measured = false;
            }
        }
    }

    for (position, line) in project.lines().iter().enumerate() {
        let (hops, _) = lines.get(position).copied().unwrap_or((1, 0.0));
        let tolerance = ADJUSTED_TOLERANCE_M + DUST_PER_HOP_M * f64::from(hops);
        let class = solve_of_line.get(&line.id).and_then(|&k| facts[k].class());
        for index in 0..line.station_count() {
            let slot = StationRef::new(line.id, index as u32);
            v.slots_checked += 1;
            let (Some(new), Some(old)) = (
                adjusted.slot(project, slot),
                old_adjusted.get(&old_slot(slot)),
            ) else {
                v.mismatches
                    .push(mismatch(slot, Axis::Missing, f64::INFINITY, 0.0));
                continue;
            };
            if !old.iter().all(|c| c.is_finite()) {
                expect_in(&mut v.expected, ExpectedClass::LegacyNotFinite, 0.0);
                continue;
            }
            let dxy = (new[0] - old[0]).hypot(new[1] - old[1]);
            let dz = (new[2] - old[2]).abs();
            v.max_xy_m = v.max_xy_m.max(dxy);
            v.max_z_m = v.max_z_m.max(dz);
            let xy_out = dxy.is_nan() || dxy > tolerance;
            let z_out = dz.is_nan() || dz > DEPTH_TOLERANCE_M;
            match class {
                Some(class) if xy_out || z_out => {
                    let d = if xy_out { dxy.max(dz) } else { dz };
                    expect_in(&mut v.expected, class, d);
                }
                Some(_) => {}
                None => {
                    if xy_out {
                        v.mismatches
                            .push(mismatch(slot, Axis::Horizontal, dxy, tolerance));
                    }
                    if z_out {
                        v.mismatches
                            .push(mismatch(slot, Axis::Depth, dz, DEPTH_TOLERANCE_M));
                    }
                }
            }
        }
    }
    AdjustedCheck::Compared(v)
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
    expect_in(&mut v.expected, class, displacement);
}

fn expect_in(
    expected: &mut BTreeMap<ExpectedClass, ClassStats>,
    class: ExpectedClass,
    displacement: f64,
) {
    let stats: &mut ClassStats = expected.entry(class).or_default();
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
) -> BTreeSet<StationId> {
    let topology = project.topology();
    let flipped = project.closures.iter().any(|c| {
        c.strategy == ClosureStrategy::Move
            && built.legacy_snap_alignment.get(&c.id).copied()
                != Some(project.closure_is_aligned(positions, c))
    });
    if !flipped {
        return BTreeSet::new();
    }
    let n = topology.stations().len();
    let groups = |legacy_flags: bool| {
        let mut uf = UnionFind::new(n);
        for c in &project.closures {
            let tied = match c.strategy {
                ClosureStrategy::Tie => true,
                ClosureStrategy::Move if legacy_flags => built
                    .legacy_snap_alignment
                    .get(&c.id)
                    .copied()
                    .unwrap_or(false),
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
        .map(|(_, &s)| s)
        .collect()
}
