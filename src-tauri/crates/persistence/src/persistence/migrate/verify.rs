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
//!   translation" that holds a demoted merge (cycle or same-line), or with
//!   "multiple placement zero datums in a single constraint group" where the
//!   migrated solve skips a demoted merge as across one shot:
//!   [`ExpectedClass::TieNoLongerFails`] (D11: merges were united before
//!   the across-one-shot check; united, the skipped merge joined two
//!   placement datums' groups);
//! - else, in a component where the migrated solve skips a demoted merge as
//!   across one shot and holds its two stations apart:
//!   [`ExpectedClass::MergeTieSkipped`] (D11, G10: 0.6.0 united that merge
//!   and solved its two stations as one point);
//! - else, in a component 0.6.0 showed measured without a failure (every
//!   legacy adjusted slot equals its raw one) that holds a migrated `Tie`:
//!   [`ExpectedClass::TieSolved`] (D10: a `Tie` is eligible);
//! - else, in a component holding a shot whose least-squares sigma the
//!   solve floors ([`ShotSigmas`], plan Deviations G9, C24 F2), which 0.6.0
//!   weighed with a huge, infinite or zero weight:
//!   [`ExpectedClass::LegacyWeightsDegenerate`];
//! - else, in a component 0.6.0's solve failed with "inconsistent line
//!   translation" whose shots 0.6.0's destructive rewrite cannot hold to
//!   the check's 1e-6 m ([`legacy_rewrite_precision_m`], G9, C24 F3):
//!   [`ExpectedClass::LegacyRewriteFailed`];
//! - else, in a component with a raw [`ExpectedClass::TornMergeCollapsed`]
//!   slot: that class;
//! - else, horizontally within the tolerance plus the precision of 0.6.0's
//!   solve of the component ([`legacy_solve_precision_m`], G9, C24 F3):
//!   [`ExpectedClass::LegacySolvePrecision`];
//! - anything else: a mismatch.
//!
//! The three G9 classes are detected from the legacy inputs (its shots, its
//! raw layout and its solve's own failure), never from the positions
//! compared; the bounded one explains a slot only up to its bound. The D11
//! classes are detected from the demoted merges and the migrated solve's
//! across-one-shot skips.
//! The components each solve failed (and showed measured) must be the same,
//! except a legacy failure the merges 0.6.0 united first (D11), degenerate
//! legacy weights or a failed rewrite explains.
//! A slot counts once per class (the larger of its two axes' differences
//! when only one axis is out).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use stalagmight_legacy::LegacySurvey;

use crate::models::config::ClosureAdjustmentConfig;
use crate::models::survey::{ClosureStrategy, Project, RawPositions, ShotSigmas, Topology};
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

/// 0.6.0's error text for a constraint group holding two placement datums
/// (`lsq.rs`, frozen).
const MULTIPLE_PLACEMENTS: &str = "Multiple placement zero datums in a single constraint group";

/// How closely 0.6.0's destructive Closures Computed View can show a shot
/// where its solve put it, metres (plan Deviations G9, C24 finding F3).
///
/// 0.6.0 writes each solved shot back as a slope length
/// `L = sqrt(h^2 + dz^2)` from the solved horizontal `h` and the stored depth
/// change, then shows the survey from those shots, re-deriving
/// `h' = sqrt(L^2 - dz^2)`. Rounding (unit `eps`) moves `h'^2` by at most
/// about `3 eps L^2`, so `h'` by at most `3 eps L^2 / h`, and by up to
/// `sqrt(3 eps) L` when the shot is near vertical (`h` near 0: a small
/// correction is lost or quantised); the bearing's round trip through
/// degrees adds a few `eps L`. Bounded here with margin, from the shot as
/// the file holds it (`h` its measured horizontal, `L` the larger of its
/// slope length and depth change): an ordinary shot holds a few 1e-15 of its
/// length, a 34 m vertical one 1e-6 m, a 100 km one 3e-3 m. A non-finite
/// reading gives 0 (such a slot is [`ExpectedClass::LegacyNotFinite`]).
pub(super) fn legacy_rewrite_precision_m(length: f64, depth_change: f64, horizontal: f64) -> f64 {
    let slope = length.abs().max(depth_change.abs());
    if !(slope.is_finite() && horizontal.is_finite()) {
        return 0.0;
    }
    let eps = f64::EPSILON;
    let near_vertical = 2.0 * eps.sqrt() * slope;
    let rounding = if horizontal > 0.0 {
        4.0 * eps * slope * slope / horizontal
    } else {
        f64::INFINITY
    };
    rounding.min(near_vertical) + 8.0 * eps * slope
}

/// How far 0.6.0's floating-point solve of one component can stray from
/// the exact least squares, metres (plan Deviations G9, C24 finding F3):
/// `eps x kappa x scale`, the form of an elimination's forward-error bound,
/// with `kappa` the spread of the component's per-axis shot weights (largest
/// over smallest), which sets the normal matrix's condition number, and
/// `scale` the largest coordinate its solve handles: of its raw layout, and
/// of the GPS positions its datums hold or pull it to (0.6.0 solved for
/// absolute coordinates, so its rounding scales with the solution; farm seed
/// 34019254's fixes held a 17 m survey 2 km away). The weights are the floored ones
/// ([`ShotSigmas::axes`]): they are 0.6.0's wherever no shot is under the
/// floor, and a component with one is [`ExpectedClass::LegacyWeightsDegenerate`]
/// (a degenerate weight would make this bound vacuous). 0.6.0 solved
/// spur stations through the normal equations too and placed an all-spur
/// line by its attachment group's solved position, which carries that
/// rounding; the station graph lays spurs out from their measured shots. An
/// ordinary survey's weights span about 1e3 (1e-11 m at 100 m, 2e-10 m at
/// 1 km). A 100 km shot's cross-axis weight is 1e-7 next to hundreds or more
/// along short shots: there the bound is a worst case of metres, where the
/// fuzz saw 1e-5 to 2e-4 m.
pub(super) fn legacy_solve_precision_m(weights: (f64, f64), scale: f64) -> f64 {
    let (smallest, largest) = weights;
    if !(smallest > 0.0 && largest.is_finite() && scale.is_finite()) {
        return 0.0;
    }
    f64::EPSILON * (largest / smallest) * scale
}

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
    /// 0.6.0's solve failed it because a constraint group held two
    /// placement datums.
    legacy_placements_failed: bool,
    /// The migrated solve skips a demoted merge in it as across one shot
    /// (D11) and holds the merge's two stations apart, where 0.6.0 united
    /// them before its across-one-shot check.
    merge_tie_skipped: bool,
    new_failed: bool,
    /// One of its slots moved in the raw view (a torn merge collapsed).
    torn: bool,
    /// One of its legacy shots has a least-squares sigma under the floor:
    /// 0.6.0's weight there was huge, infinite or zero (C24 F2).
    degenerate_weights: bool,
    /// The sum of its legacy shots' rewrite precisions
    /// ([`legacy_rewrite_precision_m`]): how far 0.6.0's rewrite of its
    /// solve can stray (C24 F3).
    rewrite_precision_m: f64,
    /// The smallest and largest finite per-axis weight of its legacy shots
    /// (floored, [`legacy_solve_precision_m`]).
    weights: (f64, f64),
    /// Its largest horizontal coordinate (absolute, legacy): of its raw
    /// layout and of its datums' local GPS positions.
    scale_m: f64,
}

impl SolveFacts {
    /// 0.6.0's line-translation check (1e-6 m per axis) can fail from its
    /// shot rewrite's precision alone.
    fn rewrite_imprecise(&self) -> bool {
        self.rewrite_precision_m > ADJUSTED_TOLERANCE_M
    }

    /// 0.6.0 failed it because it united its merges before the
    /// across-one-shot check (D11): a translation failure over a demoted
    /// merge, or two placement datums in one group joined by a demoted merge
    /// the migrated solve skips as across one shot.
    fn legacy_merge_first_failure(&self) -> bool {
        self.merge_tie
            && (self.legacy_translation_failed
                || self.legacy_placements_failed && self.merge_tie_skipped)
    }

    /// Whether a legacy failure the new solve does not repeat is explained.
    fn legacy_failure_explained(&self) -> bool {
        self.new_failed
            || self.legacy_merge_first_failure()
            || self.degenerate_weights
            || self.legacy_translation_failed && self.rewrite_imprecise()
    }

    fn class(&self) -> Option<ExpectedClass> {
        if self.new_failed {
            None
        } else if self.legacy_merge_first_failure() {
            Some(ExpectedClass::TieNoLongerFails)
        } else if self.merge_tie_skipped && !self.legacy_failed {
            Some(ExpectedClass::MergeTieSkipped)
        } else if self.tie && self.legacy_measured && !self.legacy_failed {
            Some(ExpectedClass::TieSolved)
        } else if self.degenerate_weights {
            Some(ExpectedClass::LegacyWeightsDegenerate)
        } else if self.legacy_translation_failed && self.rewrite_imprecise() {
            Some(ExpectedClass::LegacyRewriteFailed)
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
                weights: (f64::INFINITY, 0.0),
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
    // A demoted merge the migrated solve skips as across one shot, whose two
    // stations it then solves apart: 0.6.0 united the merge first (D11).
    for skipped in adjusted.skipped() {
        let demoted_merge = built
            .tie_origins
            .get(&skipped.closure_id)
            .is_some_and(|origin| *origin != TieOrigin::AlignedClosure);
        let apart = match (adjusted.xy(skipped.a), adjusted.xy(skipped.b)) {
            (Some(a), Some(b)) => (a[0] - b[0]).hypot(a[1] - b[1]) > ADJUSTED_TOLERANCE_M,
            _ => false,
        };
        if let (true, true, Some(k)) = (demoted_merge, apart, solve_of_station(skipped.a)) {
            facts[k].merge_tie_skipped = true;
        }
    }
    // From the legacy shots, as 0.6.0 weighed and rewrote them.
    let lp = &legacy.project;
    for line in &lp.lines {
        let Some(&k) = solve_of_line.get(&line.id) else {
            continue;
        };
        let reference = lp.azimuth_reference_for(line);
        let f = &mut facts[k];
        for shot in &line.shots {
            let azimuth = shot.average_azimuth_true_rad(reference);
            let sigmas = ShotSigmas::of(adjustment, line.quality, shot.length, azimuth);
            f.degenerate_weights |= sigmas.floored();
            for sigma in sigmas.axes {
                let weight = 1.0 / (sigma * sigma);
                if weight.is_finite() && weight > 0.0 {
                    f.weights = (f.weights.0.min(weight), f.weights.1.max(weight));
                }
            }
            f.rewrite_precision_m += legacy_rewrite_precision_m(
                shot.length,
                shot.depth_change(),
                shot.horizontal_length(),
            );
        }
        for index in 0..=line.shots.len() {
            let slot = stalagmight_legacy::Slot {
                line_id: line.id,
                station_index: index as u32,
            };
            if let Some(p) = legacy.raw_positions.get(&slot) {
                let magnitude = p[0].abs().max(p[1].abs());
                if magnitude.is_finite() {
                    f.scale_m = f.scale_m.max(magnitude);
                }
            }
        }
    }
    // The positions 0.6.0's solve held or pulled the component to: its
    // datums' local GPS positions (farm seed 34019254: GPS fixes 2 km off a
    // 17 m raw layout).
    if let Some(frame) = lp.frame_datum() {
        for datum in &lp.zero_datums {
            let Some(&k) = lp
                .resolve_key(&datum.station_ref)
                .and_then(|slot| solve_of_line.get(&slot.line_id))
            else {
                continue;
            };
            let (east, north) = stalagmight_legacy::models::gps_to_local(&datum.gps, &frame.gps);
            let magnitude = east.abs().max(north.abs());
            if magnitude.is_finite() {
                facts[k].scale_m = facts[k].scale_m.max(magnitude);
            }
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
            facts[k].legacy_placements_failed |= failure.error == MULTIPLE_PLACEMENTS;
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
    // demoted merge behind a translation failure (D11), by degenerate legacy
    // weights, or by a translation failure the rewrite's precision explains
    // (G9); else both must fail.
    for (k, lines) in legacy_failed_lines {
        if !facts[k].legacy_failure_explained() {
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
        let k = solve_of_line.get(&line.id).copied();
        let class = k.and_then(|k| facts[k].class());
        let precision = k.map_or(0.0, |k| {
            legacy_solve_precision_m(facts[k].weights, facts[k].scale_m)
        });
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
                None if xy_out && !z_out && dxy <= tolerance + precision => {
                    expect_in(&mut v.expected, ExpectedClass::LegacySolvePrecision, dxy);
                }
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
