//! [#576 FZ-ORACLE] An independent differential oracle for the gas reach field.
//!
//! The engine (`gas.rs`, `range_route.rs`, `pinned.rs`, `scooter.rs`) answers "where can this dive
//! go" through a stack of searches and rules. This module answers the safety half of the same
//! question from first principles, on its own code, and requires the field never to promise more
//! than that answer allows:
//!
//! * its own graph, from the project's shots, station merges and loop closures: a reference leg
//!   only when `include_reference_legs` opts it in, jump and opted-in reference legs at the jump
//!   RMV, merges and closures as zero-length ties;
//! * gas restated from the published formula (`rmv x survey mult x ata(mean depth) x length /
//!   speed`, docs/gas-planning.md), depth from the measured coordinates;
//! * scooter range from the shot lengths and the rig: the cap is the weaker unit's `burn x cruise`
//!   less the reserve, and a metre ridden at `v` costs `max_u weaker / (burn_u x min(v, cruise_u))`
//!   of it (each unit's battery runs by its own clock);
//! * the plan segment of every route leg from the field's own way-point route indices;
//! * an exact, uncapped two-criteria (gas, range) label search over (station, way points passed),
//!   seeded at the origin or - with none, or one that no longer resolves - at every zero datum.
//!
//! Safety checks - a failure is a field that promises more than the gas or the scooter allows,
//! and is always reported HIGH (tag `UNSAFE`):
//! * every painted station has a route that respects the way-point order, is in range and is in
//!   budget, and its reported gas is not below the cheapest such route;
//! * every route leg is a real shot or tie the diver can swim, the route starts at a root, the
//!   recomputed range is within the cap at every leg and the recomputed gas at the turn is within
//!   the budget;
//! * the budget never exceeds the strategy's share of the physical gas in the enabled open-circuit
//!   cylinders (a third for All-thirds, a half for Half-Minus, all of it for Free);
//! * the reported range cap is not above the recomputed one;
//! * a pinned plan never has a bigger budget than, nor paints a station dark in, the same plan
//!   without its pins.
//!
//! Consistency checks (tag `REPORT`): the reported scooter distance, runtime, way-point gas and the
//! outbound profile (gas, time) match the route's own; way points sit at their route indices in
//! order; jump legs are flagged; every route station, the turn and every frontier station are
//! painted; nothing is painted without a turn.
//!
//! The generator covers loop caves, grid mazes, diamond chains (2^k routes trading gas against
//! range), caves in several disconnected parts, jump and reference legs (connector sections and
//! converted shots, opted in or not), plans with no origin (zero datums: none, one, several) or an
//! unresolvable one, way points (repeated, on the origin, in another part, unresolvable), segment
//! lists of the wrong length or with unusable speeds, scooter rigs, disabled and non-open-circuit
//! cylinders, every reserve preset, and pinned stage drops.
//!
//! Each seed is one plan, checked without its pins and, when it has some, with them. CI runs a
//! fixed range plus the regression seeds. Replay one seed or run a long range with
//! `GAS_ORACLE_SEED=<seed>` or `GAS_ORACLE_SEEDS=<count>` (from `GAS_ORACLE_FIRST=<first>`):
//! `GAS_ORACLE_SEEDS=200000 GAS_ORACLE_FIRST=1000000 cargo test --release -p stalagmight-models
//! --lib gas::oracle_tests -- --nocapture`.
#![expect(
    clippy::unwrap_used,
    reason = "tests construct from known-valid literals; a failed unwrap should fail the test"
)]

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};

use super::model::{CylRole, Cylinder, Diver, GasReachField, PlanSegment, TankMaterial};
use super::{MAX_SEGMENT_SPEED_M_PER_MIN, MIN_SEGMENT_SPEED_M_PER_MIN};
use crate::config::{DpvRig, DpvUnit, GasConfig, ReservePreset};
use crate::{
    ClosureStrategy, GpsCoordinates, Project, SectionData, ShotData, ShotKind, StationKey,
    StationRef, ZeroDatum,
};

/// Seeds every CI run checks: `0..CI_SEEDS`.
const CI_SEEDS: u64 = 240;

/// Seeds that once failed (or once reached a rare shape); checked on every run.
const REGRESSION_SEEDS: &[u64] = &[];

/// Seeds of findings still open, as (finding, seed, the check classes it fails with). The sweep
/// keeps them out of its CI range; [`open_findings_still_reproduce`] requires each to fail with
/// exactly those classes, so the finding stays pinned while it is open and its fix shows up as
/// that test failing - then the seeds move to [`REGRESSION_SEEDS`].
///
/// FZ-ORACLE-1: no origin (or one that no longer resolves), zero datums in several disconnected
/// parts, and two stage drops pinned in different parts. Each pin is reachable from SOME datum, so
/// both count as usable, but the second pin's leg from the first cannot be flown: the pinned
/// field then paints nothing - not even the roots - yet still reports a turn and a route toward
/// the first pin. More conservative than the truth, never less; what the plan should do instead
/// is a product choice.
const OPEN_FINDINGS: &[(&str, u64, &[&str])] = &[
    ("FZ-ORACLE-1", 0x2a48, &["REPORT turn-dark"]),
    (
        "FZ-ORACLE-1",
        0xbd4b,
        &["REPORT route-station-dark", "REPORT turn-dark"],
    ),
];

/// splitmix64: tiny, seedable, and the same stream on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

// ---- generator --------------------------------------------------------------------------------

/// One connected part of a cave as built: its stations, its entrance, and a length the scooter
/// range is drawn around so that the cap binds on some plans and not on others.
struct Part {
    stations: Vec<StationRef>,
    entrance: StationRef,
    ref_len: f64,
}

const SHAPES: [&str; 5] = ["loopy", "grid", "big-grid", "diamonds", "parts"];

fn shots(r: &mut Rng, n: usize, depth: (f64, f64), len: (f64, f64)) -> Vec<ShotData> {
    let mut d = r.range(depth.0, depth.1);
    (0..n)
        .map(|_| {
            let from = d;
            d = (d + r.range(-4.0, 4.0)).clamp(depth.0.max(0.0), depth.1 + 5.0);
            // Now and then a zero-length shot: free on gas and range, like a tie, but a shot.
            let length = if r.chance(0.02) {
                0.0
            } else {
                r.range(len.0, len.1)
            };
            ShotData {
                length,
                azimuth_in: r.range(0.0, 360.0),
                azimuth_out: 0.0,
                depth_in: from,
                depth_out: d,
                ..Default::default()
            }
        })
        .collect()
}

fn add_line(p: &mut Project, shots: Vec<ShotData>) -> u32 {
    p.add_section(SectionData {
        shots,
        ..Default::default()
    })
    .id
}

fn line_refs(id: u32, shots: usize) -> impl Iterator<Item = StationRef> {
    (0..=shots).map(move |i| StationRef::new(id, i as u32))
}

/// Hang a new section's first station on `at`: a merge, or a closure where the merge is refused.
fn attach(p: &mut Project, at: StationRef, start: StationRef) {
    if p.merge_stations(at, start).is_err() {
        let _ = p.create_loop_closure(at, start, ClosureStrategy::Batch);
    }
}

/// Tie two stations together: a merge (sometimes) or a loop closure. Either may be refused, which
/// leaves the cave with one connection fewer - a valid input all the same.
fn join(r: &mut Rng, p: &mut Project, a: StationRef, b: StationRef) {
    if a == b {
        return;
    }
    if r.chance(0.35) && p.merge_stations(a, b).is_ok() {
        return;
    }
    let _ = p.create_loop_closure(a, b, ClosureStrategy::Batch);
}

/// A line with side sections hung on it, most tied back to another station: loops whose two ways
/// round trade a shallow long line against a deep short one.
fn loopy_part(r: &mut Rng, p: &mut Project, small: bool) -> Part {
    let n0 = if small { 2 + r.pick(4) } else { 4 + r.pick(6) };
    let line_shots = shots(r, n0, (0.0, 12.0), (40.0, 220.0));
    let ref_len = line_shots.iter().map(|s| s.length).sum();
    let line = add_line(p, line_shots);
    let mut stations: Vec<StationRef> = line_refs(line, n0).collect();
    let sides = if small { 1 + r.pick(2) } else { 2 + r.pick(5) };
    for _ in 0..sides {
        let k = 1 + r.pick(4);
        let s = if r.chance(0.5) {
            shots(r, k, (20.0, 50.0), (20.0, 120.0))
        } else {
            shots(r, k, (0.0, 15.0), (40.0, 250.0))
        };
        let id = add_line(p, s);
        let at = stations[r.pick(stations.len())];
        attach(p, at, StationRef::new(id, 0));
        if r.chance(0.75) {
            let to = stations[r.pick(stations.len())];
            join(r, p, StationRef::new(id, k as u32), to);
        }
        stations.extend(line_refs(id, k));
    }
    Part {
        entrance: StationRef::new(line, 0),
        stations,
        ref_len,
    }
}

/// A grid maze: rows (row 0 the shallow line, deeper rows deeper and shorter), rungs between rows
/// at random columns. Many routes per station that trade gas against length.
fn grid_part(r: &mut Rng, p: &mut Project, big: bool) -> Part {
    let rows = if big { 4 + r.pick(3) } else { 2 + r.pick(3) };
    let cols = if big { 5 + r.pick(4) } else { 2 + r.pick(4) };
    let mut row_ids = Vec::new();
    let mut stations = Vec::new();
    let mut ref_len = 0.0;
    for row in 0..rows {
        let depth = (row as f64 * 10.0, row as f64 * 10.0 + 8.0);
        let len_hi = 220.0 - row as f64 * 35.0;
        let s = shots(r, cols, depth, (30.0, len_hi.max(40.0)));
        if row == 0 {
            ref_len = s.iter().map(|x| x.length).sum();
        }
        let id = add_line(p, s);
        row_ids.push(id);
        stations.extend(line_refs(id, cols));
        if row > 0 {
            attach(
                p,
                StationRef::new(row_ids[row - 1], 0),
                StationRef::new(id, 0),
            );
        }
    }
    for row in 0..rows - 1 {
        for c in 1..=cols {
            if r.chance(if big { 0.7 } else { 0.55 }) {
                let k = 1 + r.pick(2);
                let s = shots(
                    r,
                    k,
                    (row as f64 * 10.0, row as f64 * 10.0 + 12.0),
                    (10.0, 60.0),
                );
                let id = add_line(p, s);
                attach(
                    p,
                    StationRef::new(row_ids[row], c as u32),
                    StationRef::new(id, 0),
                );
                let _ = p.create_loop_closure(
                    StationRef::new(id, k as u32),
                    StationRef::new(row_ids[row + 1], c as u32),
                    ClosureStrategy::Batch,
                );
                stations.extend(line_refs(id, k));
            }
        }
    }
    Part {
        entrance: StationRef::new(row_ids[0], 0),
        stations,
        ref_len,
    }
}

/// A chain of diamonds: each a shallow long pair of shots or a deep short shot between two
/// junctions, so 2^k routes reach the last junction with distinct (gas, range) trade-offs.
fn diamond_part(r: &mut Rng, p: &mut Project, small: bool) -> Part {
    let k = if small { 2 + r.pick(3) } else { 5 + r.pick(4) };
    let line_shots: Vec<ShotData> = (0..2 * k)
        .map(|_| {
            let d = r.range(0.0, 6.0);
            ShotData {
                length: r.range(60.0, 160.0),
                azimuth_in: r.range(0.0, 360.0),
                depth_in: d,
                depth_out: d,
                ..Default::default()
            }
        })
        .collect();
    let ref_len: f64 = line_shots.iter().map(|s| s.length).sum();
    let line = add_line(p, line_shots);
    let mut stations: Vec<StationRef> = line_refs(line, 2 * k).collect();
    for i in 0..k {
        let d = r.range(25.0, 55.0);
        let id = add_line(
            p,
            vec![ShotData {
                length: r.range(40.0, 150.0),
                azimuth_in: r.range(0.0, 360.0),
                depth_in: d,
                depth_out: d,
                ..Default::default()
            }],
        );
        attach(
            p,
            StationRef::new(line, 2 * i as u32),
            StationRef::new(id, 0),
        );
        let _ = p.create_loop_closure(
            StationRef::new(id, 1),
            StationRef::new(line, 2 * i as u32 + 2),
            ClosureStrategy::Batch,
        );
        stations.extend(line_refs(id, 1));
    }
    Part {
        entrance: StationRef::new(line, 0),
        stations,
        ref_len: ref_len * 0.6,
    }
}

/// Jump and reference legs: one-shot connector sections between random stations (which may join
/// two otherwise disconnected parts), and ordinary shots converted in place.
fn add_virtual_legs(r: &mut Rng, p: &mut Project, stations: &mut Vec<StationRef>) {
    if r.chance(0.55) {
        for _ in 0..=r.pick(3) {
            let a = stations[r.pick(stations.len())];
            let b = stations[r.pick(stations.len())];
            if a == b {
                continue;
            }
            let d = r.range(0.0, 45.0);
            let kind = if r.chance(0.5) {
                ShotKind::Jump
            } else {
                ShotKind::Reference
            };
            let id = add_line(
                p,
                vec![ShotData {
                    length: r.range(5.0, 90.0),
                    azimuth_in: r.range(0.0, 360.0),
                    depth_in: d,
                    depth_out: d,
                    kind,
                    ..Default::default()
                }],
            );
            attach(p, a, StationRef::new(id, 0));
            join(r, p, StationRef::new(id, 1), b);
            stations.extend(line_refs(id, 1));
        }
    }
    if r.chance(0.35) {
        for section in &mut p.sections {
            for shot in &mut section.shots {
                if r.chance(0.1) {
                    shot.kind = if r.chance(0.5) {
                        ShotKind::Jump
                    } else {
                        ShotKind::Reference
                    };
                }
            }
        }
    }
}

struct Plan {
    seed: u64,
    shape: &'static str,
    p: Project,
    parts: usize,
    origin: Option<StationRef>,
    endpoint: Option<StationRef>,
    waypoints: Vec<StationRef>,
    segments: Vec<PlanSegment>,
    pins: Vec<Option<StationRef>>,
    rmv: f64,
    rig: Vec<Cylinder>,
    cfg: GasConfig,
}

fn cylinder(r: &mut Rng, role: CylRole, conservative: bool) -> Cylinder {
    let rated = [176.0, 200.0, 207.0, 232.0, 300.0][r.pick(5)];
    let fill = if conservative {
        r.range(30.0, rated)
    } else {
        r.range(30.0, 300.0)
    };
    Cylinder {
        usable_gas_bar: r.range(0.0, 250.0),
        enabled: !r.chance(0.08),
        ..Cylinder::new(
            r.range(3.0, 24.0),
            rated,
            fill,
            TankMaterial::Steel,
            role,
            None,
        )
        .unwrap()
    }
}

fn draw(seed: u64) -> Plan {
    let mut r = Rng(seed ^ 0x0FA5_0576_0AC1_E000);
    let shape_ix = (seed % SHAPES.len() as u64) as usize;
    let mut p = Project::new("FZ-ORACLE".to_string(), None);
    let parts: Vec<Part> = match shape_ix {
        0 => vec![loopy_part(&mut r, &mut p, false)],
        1 => vec![grid_part(&mut r, &mut p, false)],
        2 => vec![grid_part(&mut r, &mut p, true)],
        3 => vec![diamond_part(&mut r, &mut p, false)],
        _ => (0..2 + r.pick(3))
            .map(|_| match r.pick(3) {
                0 => loopy_part(&mut r, &mut p, true),
                1 => grid_part(&mut r, &mut p, false),
                _ => diamond_part(&mut r, &mut p, true),
            })
            .collect(),
    };
    let mut st: Vec<StationRef> = parts.iter().flat_map(|x| x.stations.clone()).collect();
    add_virtual_legs(&mut r, &mut p, &mut st);
    let ghost = StationRef::new(p.sections.iter().map(|s| s.id).max().unwrap_or(0) + 1000, 0);
    let entrance = parts[r.pick(parts.len())].entrance;
    let ref_len = parts.iter().map(|x| x.ref_len).fold(0.0, f64::max);

    // Origin: the entrance, any station, none (the zero datums), or one that no longer resolves
    // (falls back to the zero datums too).
    let roll = r.unit();
    let origin = if roll < 0.45 {
        Some(entrance)
    } else if roll < 0.62 {
        Some(st[r.pick(st.len())])
    } else if roll < 0.9 {
        None
    } else {
        Some(ghost)
    };
    let datums = if origin.is_none() || origin == Some(ghost) {
        [0, 1, 1, 2, 3][r.pick(5)]
    } else {
        usize::from(r.chance(0.3))
    };
    let mut seen = HashSet::new();
    for _ in 0..datums {
        let s = if r.chance(0.5) {
            parts[r.pick(parts.len())].entrance
        } else {
            st[r.pick(st.len())]
        };
        if seen.insert(s) {
            p.zero_datums.push(ZeroDatum {
                station_ref: s.into(),
                gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
            });
        }
    }

    let preset = [
        ReservePreset::AllThirds,
        ReservePreset::OneStage,
        ReservePreset::TwoStage,
        ReservePreset::Free,
    ][r.pick(4)];
    let conservative = r.chance(0.6);
    let mut rig: Vec<Cylinder> = (0..=r.pick(2))
        .map(|_| cylinder(&mut r, CylRole::BackGas, conservative))
        .collect();
    let stages = r.pick(4);
    rig.extend((0..stages).map(|_| cylinder(&mut r, CylRole::Stage, conservative)));
    if r.chance(0.15) {
        let role = [CylRole::DilOut, CylRole::Bailout, CylRole::Deco][r.pick(3)];
        let at = r.pick(rig.len() + 1);
        rig.insert(at, cylinder(&mut r, role, conservative));
    }
    let mut cfg = GasConfig {
        default_reserve: preset,
        conservative_tank_gas: conservative,
        ..GasConfig::default()
    };
    cfg.reserve_profile.stage_reserve_bar = r.range(0.0, 60.0);
    cfg.reserve_profile.all_thirds_safety_margin_bar = r.range(0.0, 20.0);
    cfg.survey_rmv_multiplier = r.range(1.0, 2.0);
    cfg.jump_rmv_multiplier = r.range(1.0, 2.5);
    cfg.include_reference_legs = r.chance(0.4);
    cfg.swim_speed_m_per_min = r.range(8.0, 25.0);
    cfg.survey_speed_m_per_min = if r.chance(0.5) {
        Some(r.range(4.0, 25.0))
    } else {
        None
    };
    let scootered = r.chance(0.85);
    if scootered {
        let reserve = r.range(0.0, 0.4);
        let target = r.range(0.2, 1.6) * ref_len;
        let (sp1, sp2) = (r.range(25.0, 60.0), r.range(25.0, 60.0));
        let b1 = target / ((1.0 - reserve) * sp1) * r.range(1.0, 1.6);
        let b2 = target / ((1.0 - reserve) * sp2) * r.range(1.0, 1.6);
        cfg.dpv = Some(DpvRig {
            primary: DpvUnit {
                speed_m_per_min: sp1,
                burn_minutes: b1,
            },
            backup: DpvUnit {
                speed_m_per_min: sp2,
                burn_minutes: b2,
            },
            range_reserve_fraction: reserve,
        });
    }
    cfg.normalize();

    let pick_station = |r: &mut Rng| {
        if r.chance(0.03) {
            ghost
        } else {
            st[r.pick(st.len())]
        }
    };
    let endpoint = if r.chance(0.65) {
        Some(pick_station(&mut r))
    } else {
        None
    };
    let mut waypoints: Vec<StationRef> = Vec::new();
    if r.chance(0.5) {
        for _ in 0..=r.pick(3) {
            let w = match (r.pick(10), waypoints.last(), origin) {
                (0, Some(&last), _) => last,
                (1, _, Some(o)) => o,
                _ => pick_station(&mut r),
            };
            waypoints.push(w);
        }
    }
    let spans = match r.pick(10) {
        0 => r.pick(waypoints.len() + 1),
        1 => waypoints.len() + 2 + r.pick(2),
        _ => waypoints.len() + 1,
    };
    let segments: Vec<PlanSegment> = (0..spans)
        .map(|_| PlanSegment {
            speed_m_per_min: if r.chance(0.05) {
                Some([0.0, -5.0, f64::NAN, 5000.0, 0.01][r.pick(5)])
            } else if r.chance(0.85) {
                Some(r.range(8.0, 70.0))
            } else {
                None
            },
            on_scooter: scootered && r.chance(0.8),
        })
        .collect();
    let pins: Vec<Option<StationRef>> = (0..r.pick(4))
        .map(|_| {
            if r.chance(0.85) {
                Some(pick_station(&mut r))
            } else {
                None
            }
        })
        .collect();
    Plan {
        seed,
        shape: SHAPES[shape_ix],
        p,
        parts: parts.len(),
        origin,
        endpoint,
        waypoints,
        segments,
        pins,
        rmv: r.range(12.0, 30.0),
        rig,
        cfg,
    }
}

fn field(plan: &Plan, pins: &[Option<StationRef>]) -> GasReachField {
    plan.p.compute_gas_reach_field_with_overrides(
        plan.origin.as_ref(),
        plan.endpoint.as_ref(),
        &plan.waypoints,
        &plan.segments,
        &Diver::new(plan.rmv, None).unwrap(),
        &plan.rig,
        &plan.cfg,
        pins,
    )
}

// ---- the oracle -------------------------------------------------------------------------------

/// One edge of the oracle's own graph.
#[derive(Clone, Copy)]
struct Edge {
    to: usize,
    /// Plain length (m): what a scootered metre is charged on, and what the runtime is timed on.
    len: f64,
    /// `ata(mean depth) x length`, times the jump RMV multiplier on a virtual leg: litres per unit
    /// of `rmv / speed`.
    basis: f64,
    /// The shot's kind; `None` for a merge or closure tie.
    kind: Option<ShotKind>,
    /// Whether the diver may swim it (a reference leg only when opted in).
    swimmable: bool,
}

/// One plan segment as the oracle prices it.
#[derive(Clone, Copy)]
struct Seg {
    litres_per_basis: f64,
    range_per_metre: f64,
    speed: f64,
}

struct Oracle {
    idx: HashMap<StationRef, usize>,
    adj: Vec<Vec<Edge>>,
    roots: Vec<usize>,
    /// The way points as nodes; `usize::MAX` for one that does not resolve (never passed).
    wps: Vec<usize>,
    seg: Vec<Seg>,
    limit: f64,
}

/// Ambient pressure (ATA) at a depth in metres, positive down; the surface floors it at 1.
fn ata(depth: f64) -> f64 {
    1.0 + depth.max(0.0) / 10.0
}

/// The usable one-way range (m): the weaker unit's `burn x cruise`, less the reserve.
fn range_cap(rig: &DpvRig) -> f64 {
    let weaker = (rig.primary.burn_minutes * rig.primary.speed_m_per_min)
        .min(rig.backup.burn_minutes * rig.backup.speed_m_per_min);
    (weaker * (1.0 - rig.range_reserve_fraction)).max(0.0)
}

/// Range a metre ridden at `speed` costs against [`range_cap`]: unit `u` spends
/// `1 / (burn_u x min(speed, cruise_u))` of its battery per metre, scaled to the cap's metres; the
/// harder-drained unit binds. The reserve scales cap and battery alike and cancels.
fn range_per_metre(rig: &DpvRig, speed: f64) -> f64 {
    let weaker = (rig.primary.burn_minutes * rig.primary.speed_m_per_min)
        .min(rig.backup.burn_minutes * rig.backup.speed_m_per_min);
    let unit = |u: &DpvUnit| weaker / (u.burn_minutes * speed.min(u.speed_m_per_min));
    unit(&rig.primary).max(unit(&rig.backup))
}

fn oracle(plan: &Plan, f: &GasReachField) -> Oracle {
    let p = &plan.p;
    let cfg = &plan.cfg;
    let idx: HashMap<StationRef, usize> = f
        .station_refs
        .iter()
        .enumerate()
        .map(|(i, &s)| (s, i))
        .collect();
    let depth: HashMap<StationRef, f64> = p
        .compute_station_coordinates()
        .iter()
        .map(|s| (s.station_ref, s.coordinates.z))
        .collect();
    let mut adj = vec![Vec::new(); f.station_refs.len()];
    for s in &p.sections {
        for (j, shot) in s.shots.iter().enumerate() {
            let a = StationRef::new(s.id, j as u32);
            let b = StationRef::new(s.id, j as u32 + 1);
            let (Some(&ia), Some(&ib)) = (idx.get(&a), idx.get(&b)) else {
                continue;
            };
            let virt = matches!(shot.kind, ShotKind::Jump | ShotKind::Reference);
            let len = shot.length.max(0.0);
            let mult = if virt { cfg.jump_rmv_multiplier } else { 1.0 };
            let basis = ata(0.5 * (depth[&a] + depth[&b])) * len * mult;
            let swimmable = shot.kind != ShotKind::Reference || cfg.include_reference_legs;
            for (from, to) in [(ia, ib), (ib, ia)] {
                adj[from].push(Edge {
                    to,
                    len,
                    basis,
                    kind: Some(shot.kind),
                    swimmable,
                });
            }
        }
    }
    let ties: Vec<(StationKey, StationKey)> = p
        .station_merges
        .iter()
        .map(|m| (m.station_a.clone(), m.station_b.clone()))
        .chain(
            p.loop_closures
                .iter()
                .map(|c| (c.station_a.clone(), c.station_b.clone())),
        )
        .collect();
    for (a, b) in ties {
        let (Some(a), Some(b)) = (p.resolve_key(&a), p.resolve_key(&b)) else {
            continue;
        };
        let (Some(&ia), Some(&ib)) = (idx.get(&a), idx.get(&b)) else {
            continue;
        };
        for (from, to) in [(ia, ib), (ib, ia)] {
            adj[from].push(Edge {
                to,
                len: 0.0,
                basis: 0.0,
                kind: None,
                swimmable: true,
            });
        }
    }
    let datum_roots = || -> Vec<usize> {
        p.zero_datums
            .iter()
            .filter_map(|zd| p.resolve_key(&zd.station_ref))
            .filter_map(|s| idx.get(&s).copied())
            .collect()
    };
    let roots = match plan.origin.and_then(|o| idx.get(&o).copied()) {
        Some(i) => vec![i],
        None => datum_roots(),
    };
    let wps = plan
        .waypoints
        .iter()
        .map(|w| idx.get(w).copied().unwrap_or(usize::MAX))
        .collect();
    let in_rmv = plan.rmv * cfg.survey_rmv_multiplier;
    let default_speed = cfg
        .survey_speed_m_per_min
        .unwrap_or(cfg.swim_speed_m_per_min);
    let seg = (0..=plan.waypoints.len())
        .map(|k| {
            let s = plan.segments.get(k);
            let speed = s
                .and_then(|s| s.speed_m_per_min)
                .filter(|v| (MIN_SEGMENT_SPEED_M_PER_MIN..=MAX_SEGMENT_SPEED_M_PER_MIN).contains(v))
                .unwrap_or(default_speed);
            let on = s.is_some_and(|s| s.on_scooter);
            Seg {
                litres_per_basis: in_rmv / speed,
                range_per_metre: match (&cfg.dpv, on) {
                    (Some(rig), true) => range_per_metre(rig, speed),
                    _ => 0.0,
                },
                speed,
            }
        })
        .collect();
    Oracle {
        idx,
        adj,
        roots,
        wps,
        seg,
        limit: cfg.dpv.as_ref().map_or(f64::INFINITY, range_cap),
    }
}

/// A search label's heap key: (gas, range spent, label index), popped cheapest gas first.
#[derive(Clone, Copy, PartialEq)]
struct Key(f64, f64, usize);

impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Key {
    fn cmp(&self, o: &Self) -> Ordering {
        self.0
            .total_cmp(&o.0)
            .then(self.1.total_cmp(&o.1))
            .then(self.2.cmp(&o.2))
    }
}

/// What the exact search found, per station.
struct Reach {
    /// Cheapest gas of any in-range route from a root that respects the way-point order so far
    /// (any progress through the chain).
    any: Vec<f64>,
    /// Cheapest gas of an in-range route that has passed every way point.
    fin: Vec<f64>,
    /// Most non-dominated labels kept at one (station, progress) state.
    widest_front: usize,
}

/// Exact, uncapped Pareto search over (station, way points passed) on (gas, range). Labels pop in
/// gas order, so a label is non-dominated exactly when it spends less range than every label
/// already settled at its state.
fn search(o: &Oracle, limit: f64) -> Reach {
    let n = o.adj.len();
    let w = o.wps.len();
    let state = |node: usize, k: usize| node * (w + 1) + k;
    let advance = |mut k: usize, node: usize| {
        while k < w && o.wps[k] == node {
            k += 1;
        }
        k
    };
    let mut least = vec![f64::INFINITY; n * (w + 1)];
    let mut kept = vec![0usize; n * (w + 1)];
    let mut any = vec![f64::INFINITY; n];
    let mut fin = vec![f64::INFINITY; n];
    let mut heap = BinaryHeap::new();
    let mut labels: Vec<(usize, usize)> = Vec::new();
    for &root in &o.roots {
        heap.push(Reverse(Key(0.0, 0.0, labels.len())));
        labels.push((root, advance(0, root)));
    }
    while let Some(Reverse(Key(gas, spent, at))) = heap.pop() {
        let (u, k) = labels[at];
        if spent >= least[state(u, k)] {
            continue;
        }
        least[state(u, k)] = spent;
        kept[state(u, k)] += 1;
        any[u] = any[u].min(gas);
        if k == w {
            fin[u] = fin[u].min(gas);
        }
        let s = o.seg[k];
        for e in o.adj[u].iter().filter(|e| e.swimmable) {
            let ng = gas + e.basis * s.litres_per_basis;
            let ns = spent + e.len * s.range_per_metre;
            if ns > limit * (1.0 + 1e-12) + 1e-9 {
                continue;
            }
            let nk = advance(k, e.to);
            if ns >= least[state(e.to, nk)] {
                continue;
            }
            heap.push(Reverse(Key(ng, ns, labels.len())));
            labels.push((e.to, nk));
        }
    }
    Reach {
        any,
        fin,
        widest_front: kept.into_iter().max().unwrap_or(0),
    }
}

/// The physical ceiling on a budget: the strategy's share of the ideal gas (`water x fill`, never
/// less than the Z-derated real gas) in the enabled back-gas and stage cylinders. All-thirds takes
/// a third of each; a Half-Minus plan a quarter or sixth of the back gas and at most half of a
/// stage; Free at most the whole fill. A staged preset the rig is short of stages for is
/// All-thirds.
fn budget_ceiling(plan: &Plan) -> f64 {
    let oc = || {
        plan.rig
            .iter()
            .filter(|c| c.enabled && matches!(c.role, CylRole::BackGas | CylRole::Stage))
    };
    let stages = oc().filter(|c| c.role == CylRole::Stage).count();
    let share = match plan.cfg.default_reserve {
        ReservePreset::OneStage if stages >= 1 => 0.5,
        ReservePreset::TwoStage if stages >= 2 => 0.5,
        ReservePreset::Free => 1.0,
        _ => 1.0 / 3.0,
    };
    share
        * oc()
            .map(|c| c.water_volume_l * c.fill_bar.max(0.0))
            .sum::<f64>()
}

// ---- the checks -------------------------------------------------------------------------------

#[derive(Default)]
struct Tally {
    plans: usize,
    fields: usize,
    painted: usize,
    turns: usize,
    legs: usize,
    jump_legs: usize,
    reference_legs: usize,
    no_origin_plans: usize,
    ghost_origin_plans: usize,
    multi_root_routes: usize,
    rootless_plans: usize,
    split_plans: usize,
    waypoint_routes: usize,
    pinned_fields: usize,
    scootered_routes: usize,
    turnless_routes: usize,
    range_tight_turns: usize,
    dark_but_feasible: usize,
    widest_front: usize,
    /// (seed, tag, message) per failed check.
    errors: Vec<(u64, &'static str, String)>,
}

fn le(a: f64, b: f64) -> bool {
    a <= b * (1.0 + 1e-9) + 1e-7
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

/// The cheapest swimmable edge from `a` to `b` of the kind the leg claims, by (gas, range).
fn leg_edge(o: &Oracle, a: usize, b: usize, is_jump: bool, s: Seg) -> Result<Edge, &'static str> {
    let all: Vec<&Edge> = o.adj[a].iter().filter(|e| e.to == b).collect();
    if all.is_empty() {
        return Err("UNSAFE route-leg-not-an-edge");
    }
    let swimmable: Vec<&Edge> = all.iter().copied().filter(|e| e.swimmable).collect();
    if swimmable.is_empty() {
        return Err("UNSAFE route-leg-not-swimmable");
    }
    let claimed: Vec<&Edge> = swimmable
        .iter()
        .copied()
        .filter(|e| (e.kind == Some(ShotKind::Jump)) == is_jump)
        .collect();
    if claimed.is_empty() {
        return Err("REPORT route-leg-jump-flag");
    }
    let cost = |e: &Edge| (e.basis * s.litres_per_basis, e.len * s.range_per_metre);
    Ok(**claimed
        .iter()
        .min_by(|x, y| {
            let (cx, cy) = (cost(x), cost(y));
            cx.0.total_cmp(&cy.0).then(cx.1.total_cmp(&cy.1))
        })
        .unwrap())
}

/// One line per field for a replayed seed (`GAS_ORACLE_SEED`): what was asked and what came back.
fn describe(plan: &Plan, f: &GasReachField, tag: &str) {
    println!(
        "  seed {:#x} {} [{tag}] parts {} origin {:?} endpoint {:?} way points {:?} pins {:?} \
         segments {:?} reserve {:?} refs-in {} dpv {:?}",
        plan.seed,
        plan.shape,
        plan.parts,
        plan.origin,
        plan.endpoint,
        plan.waypoints,
        plan.pins,
        plan.segments,
        plan.cfg.default_reserve,
        plan.cfg.include_reference_legs,
        plan.cfg.dpv.as_ref().map(range_cap)
    );
    println!(
        "    budget {:.3} painted {}/{} turn {:?} toward-endpoint {} route {:?} way-point indices \
         {:?} way-point gas {:?} drops {:?} at {:?} scooter {:.3} on-range {}",
        f.budget_litres,
        f.reachable.iter().filter(|&&x| x).count(),
        f.reachable.len(),
        f.final_turn_ref,
        f.turn_toward_endpoint,
        f.route
            .iter()
            .map(|l| (l.to.section_id, l.to.station_index))
            .collect::<Vec<_>>(),
        f.waypoint_route_indices,
        f.waypoint_gas_litres,
        f.stage_drop_refs,
        f.stage_drop_route_indices,
        f.scooter_distance_m,
        f.turn_on_scooter_range
    );
}

fn check_field(plan: &Plan, f: &GasReachField, tag: &str, t: &mut Tally) {
    if std::env::var_os("GAS_ORACLE_SEED").is_some() {
        describe(plan, f, tag);
    }
    t.fields += 1;
    let seed = plan.seed;
    let o = oracle(plan, f);
    let reach = search(&o, o.limit);
    t.widest_front = t.widest_front.max(reach.widest_front);
    let b = f.budget_litres;
    let mut errors: Vec<(&'static str, String)> = Vec::new();
    let mut err = |class: &'static str, m: String| errors.push((class, format!("[{tag}] {m}")));

    // The budget and the range cap.
    let ceiling = budget_ceiling(plan);
    if !le(b, ceiling) {
        err(
            "UNSAFE budget-over-physical-gas",
            format!("budget {b:.3} L over the strategy's share {ceiling:.3} L of the rig"),
        );
    }
    match (f.scooter_range_limit_m, o.limit.is_finite()) {
        (Some(l), true) if !le(l, o.limit) => err(
            "UNSAFE range-cap-over-rig",
            format!("reported cap {l:.3} m over the rig's {:.3} m", o.limit),
        ),
        (Some(l), true) if !close(l, o.limit) => err(
            "REPORT range-cap",
            format!("reported cap {l:.3} m, rig gives {:.3} m", o.limit),
        ),
        (Some(l), false) => err(
            "UNSAFE range-cap-without-rig",
            format!("reported cap {l:.3} m with no rig"),
        ),
        (None, true) => err("REPORT range-cap", "no cap reported for a rig".into()),
        _ => {}
    }
    if !le(f.max_reachable_gas, b) {
        err(
            "UNSAFE max-gas-over-budget",
            format!(
                "max reachable gas {:.3} over budget {b:.3}",
                f.max_reachable_gas
            ),
        );
    }

    // Painted stations: an in-range, in-budget route that respects the way-point order reaches
    // each, and its reported gas is not below the cheapest such route.
    for (s, &painted) in f.reachable.iter().enumerate() {
        if !painted {
            if plan.waypoints.is_empty() && tag == "free" && le(reach.fin[s] * (1.0 + 1e-6), b) {
                t.dark_but_feasible += 1;
            }
            continue;
        }
        t.painted += 1;
        let at = f.station_refs[s];
        if !le(reach.any[s], b) {
            err(
                "UNSAFE painted-beyond-gas-or-range",
                format!(
                    "{at:?} painted, cheapest in-range route {:.3} L over budget {b:.3} L",
                    reach.any[s]
                ),
            );
        }
        match f.gas_to_reach.get(s).copied().flatten() {
            Some(g) if g < reach.any[s] * (1.0 - 1e-9) - 1e-7 => err(
                "UNSAFE gas-understated",
                format!(
                    "{at:?} reports {g:.3} L, cheapest in-range route {:.3} L",
                    reach.any[s]
                ),
            ),
            None => err("REPORT painted-without-gas", format!("{at:?}")),
            _ => {}
        }
    }
    for fr in &f.turn_frontier {
        if o.idx.get(fr).is_none_or(|&i| !f.reachable[i]) {
            err(
                "REPORT frontier-dark",
                format!("frontier {fr:?} is not painted"),
            );
        }
    }

    // The turn and the route to it.
    let Some(turn) = f.final_turn_ref else {
        // No turn: nothing is reachable, or the way-point chain alone is over the budget (its
        // pass-through stations in budget stay painted, checked above, and the route shows the
        // chain as far as it was committed). The chain's legs must still be real.
        if !f.route.is_empty() {
            t.turnless_routes += 1;
        }
        for leg in &f.route {
            let real = match (o.idx.get(&leg.from), o.idx.get(&leg.to)) {
                (Some(&a), Some(&b)) => o.adj[a].iter().any(|e| e.to == b && e.swimmable),
                _ => false,
            };
            if !real {
                err(
                    "UNSAFE chain-leg-not-swimmable",
                    format!("turnless chain leg {:?} -> {:?}", leg.from, leg.to),
                );
            }
        }
        t.errors
            .extend(errors.into_iter().map(|(c, m)| (seed, c, m)));
        return;
    };
    t.turns += 1;
    if o.idx.get(&turn).is_none_or(|&i| !f.reachable[i]) {
        err("REPORT turn-dark", format!("turn {turn:?} is not painted"));
    }
    if f.route.is_empty() {
        if !o.roots.iter().any(|&r| f.station_refs[r] == turn) {
            err(
                "UNSAFE turn-without-route",
                format!("turn {turn:?} is no root, no route"),
            );
        }
        t.errors
            .extend(errors.into_iter().map(|(c, m)| (seed, c, m)));
        return;
    }
    let refs: Vec<StationRef> = std::iter::once(f.route[0].from)
        .chain(f.route.iter().map(|l| l.to))
        .collect();
    let Some(nodes) = refs
        .iter()
        .map(|s| o.idx.get(s).copied())
        .collect::<Option<Vec<usize>>>()
    else {
        err("UNSAFE route-station-unknown", format!("route {refs:?}"));
        t.errors
            .extend(errors.into_iter().map(|(c, m)| (seed, c, m)));
        return;
    };
    if f.route.windows(2).any(|w| w[0].to != w[1].from) {
        err("REPORT route-broken", "legs do not chain".into());
    }
    if refs.last() != Some(&turn) {
        err(
            "REPORT route-end",
            format!("route ends at {:?}, turn {turn:?}", refs.last()),
        );
    }
    if !o.roots.contains(&nodes[0]) {
        err(
            "UNSAFE route-start-not-root",
            format!("route starts at {:?}", refs[0]),
        );
    }
    if o.roots.len() > 1 {
        t.multi_root_routes += 1;
    }
    let wri: Vec<Option<usize>> = f
        .waypoint_route_indices
        .iter()
        .map(|x| x.map(|v| v as usize))
        .collect();
    if !plan.waypoints.is_empty() {
        t.waypoint_routes += 1;
    }
    let mut prev = 0usize;
    for (k, ix) in wri.iter().enumerate() {
        match ix {
            Some(ix) if nodes.get(*ix) == o.wps.get(k) && *ix >= prev => prev = *ix,
            _ => err(
                "REPORT waypoint-index",
                format!(
                    "way point {k} ({:?}) at route index {ix:?}",
                    plan.waypoints.get(k)
                ),
            ),
        }
    }
    if wri.len() != plan.waypoints.len() {
        err(
            "REPORT waypoint-index",
            format!(
                "{} indices for {} way points",
                wri.len(),
                plan.waypoints.len()
            ),
        );
    }
    let (mut gas, mut spent, mut time) = (0.0, 0.0, 0.0);
    let mut cum = vec![(0.0, 0.0)];
    for (i, leg) in f.route.iter().enumerate() {
        let k = wri.iter().flatten().filter(|&&p| p <= i).count();
        let s = o.seg[k.min(o.seg.len() - 1)];
        let e = match leg_edge(&o, nodes[i], nodes[i + 1], leg.is_jump, s) {
            Ok(e) => e,
            Err(class) => {
                err(class, format!("leg {i} {:?} -> {:?}", leg.from, leg.to));
                match leg_edge(&o, nodes[i], nodes[i + 1], !leg.is_jump, s) {
                    Ok(e) => e,
                    Err(_) => break,
                }
            }
        };
        t.legs += 1;
        match e.kind {
            Some(ShotKind::Jump) => t.jump_legs += 1,
            Some(ShotKind::Reference) => t.reference_legs += 1,
            _ => {}
        }
        gas += e.basis * s.litres_per_basis;
        spent += e.len * s.range_per_metre;
        time += e.len / s.speed;
        cum.push((gas, time));
        if !le(spent, o.limit) {
            err(
                "UNSAFE route-over-range",
                format!(
                    "{spent:.3} m of a {:.3} m cap spent by leg {i} (turn {turn:?})",
                    o.limit
                ),
            );
            break;
        }
    }
    if spent > 0.0 {
        t.scootered_routes += 1;
    }
    if o.limit.is_finite() && spent > 0.9 * o.limit {
        t.range_tight_turns += 1;
    }
    if !le(gas, b) {
        err(
            "UNSAFE route-over-budget",
            format!("route to the turn {turn:?} spends {gas:.3} L of {b:.3} L"),
        );
    }
    if !close(spent, f.scooter_distance_m) {
        err(
            "REPORT scooter-distance",
            format!(
                "reported {:.4} m, the route spends {spent:.4} m",
                f.scooter_distance_m
            ),
        );
    }
    if let Some(rt) = f.route_runtime_min {
        if !close(rt, time) {
            err(
                "REPORT runtime",
                format!("reported {rt:.4} min, the route takes {time:.4} min"),
            );
        }
    }
    for (k, g) in f.waypoint_gas_litres.iter().enumerate() {
        if let (Some(g), Some(Some(ix))) = (g, wri.get(k)) {
            if let Some(&(cg, _)) = cum.get(*ix) {
                if !close(*g, cg) {
                    err(
                        "REPORT waypoint-gas",
                        format!("way point {k}: reported {g:.4} L, route {cg:.4} L"),
                    );
                }
            }
        }
    }
    for pt in f.route_profile.iter().filter(|p| p.outbound) {
        let i = pt.route_index as usize;
        let Some(&(cg, ct)) = cum.get(i) else {
            err(
                "REPORT profile",
                format!("profile index {i} past the route"),
            );
            break;
        };
        if refs.get(i) != Some(&pt.station_ref)
            || !close(cg, pt.gas_l)
            || !close(ct, pt.elapsed_min)
        {
            err(
                "REPORT profile",
                format!(
                    "point {i}: {:?} {:.4} L {:.4} min, route {:?} {cg:.4} L {ct:.4} min",
                    pt.station_ref,
                    pt.gas_l,
                    pt.elapsed_min,
                    refs.get(i)
                ),
            );
            break;
        }
    }
    if let Some(&s) = nodes.iter().find(|&&s| !f.reachable[s]) {
        err(
            "REPORT route-station-dark",
            format!("route station {:?} is not painted", f.station_refs[s]),
        );
    }
    t.errors
        .extend(errors.into_iter().map(|(c, m)| (seed, c, m)));
}

fn check_seed(seed: u64, t: &mut Tally) {
    let plan = draw(seed);
    t.plans += 1;
    match plan.origin {
        None => t.no_origin_plans += 1,
        Some(o) if !plan.p.station_exists(o) => t.ghost_origin_plans += 1,
        _ => {}
    }
    if plan.parts > 1 {
        t.split_plans += 1;
    }
    let free = field(&plan, &[]);
    if oracle(&plan, &free).roots.is_empty() {
        t.rootless_plans += 1;
    }
    check_field(&plan, &free, "free", t);
    if plan.pins.iter().any(Option::is_some) {
        t.pinned_fields += 1;
        let pinned = field(&plan, &plan.pins);
        check_field(&plan, &pinned, "pinned", t);
        if !le(pinned.budget_litres, free.budget_litres) {
            t.errors.push((
                seed,
                "UNSAFE pinned-budget-over-pin-free",
                format!(
                    "pinned budget {:.3} > pin-free {:.3}",
                    pinned.budget_litres, free.budget_litres
                ),
            ));
        }
        if let Some(s) =
            (0..pinned.reachable.len()).find(|&s| pinned.reachable[s] && !free.reachable[s])
        {
            t.errors.push((
                seed,
                "UNSAFE pinned-paints-pin-free-dark",
                format!("pinned paints {:?}", pinned.station_refs[s]),
            ));
        }
    }
}

fn env_u64(name: &str) -> Option<u64> {
    let raw = std::env::var(name).ok()?;
    let raw = raw.trim();
    match raw.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => raw.parse().ok(),
    }
}

fn run(seeds: impl Iterator<Item = u64>) -> Tally {
    let mut t = Tally::default();
    for seed in seeds {
        // A panic inside the engine (or the oracle) must still name its seed.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut one = Tally::default();
            check_seed(seed, &mut one);
            one
        }));
        match outcome {
            Ok(one) => merge(&mut t, one),
            Err(_) => t
                .errors
                .push((seed, "PANIC", "panicked (message above)".to_string())),
        }
    }
    t
}

fn merge(t: &mut Tally, one: Tally) {
    t.plans += one.plans;
    t.fields += one.fields;
    t.painted += one.painted;
    t.turns += one.turns;
    t.legs += one.legs;
    t.jump_legs += one.jump_legs;
    t.reference_legs += one.reference_legs;
    t.no_origin_plans += one.no_origin_plans;
    t.ghost_origin_plans += one.ghost_origin_plans;
    t.multi_root_routes += one.multi_root_routes;
    t.rootless_plans += one.rootless_plans;
    t.split_plans += one.split_plans;
    t.waypoint_routes += one.waypoint_routes;
    t.pinned_fields += one.pinned_fields;
    t.scootered_routes += one.scootered_routes;
    t.turnless_routes += one.turnless_routes;
    t.range_tight_turns += one.range_tight_turns;
    t.dark_but_feasible += one.dark_but_feasible;
    t.widest_front = t.widest_front.max(one.widest_front);
    t.errors.extend(one.errors);
}

fn report(name: &str, t: &Tally) {
    println!(
        "FZ-ORACLE gas {name}: plans {} fields {} painted {} turns {} legs {} (jump {} reference {}) \
         no-origin {} ghost-origin {} rootless {} multi-root-routes {} split-caves {} \
         way-point-routes {} turnless-chains {} pinned {} scootered-routes {} range-tight {} \
         dark-but-feasible {} widest-front {} FAILED {}",
        t.plans,
        t.fields,
        t.painted,
        t.turns,
        t.legs,
        t.jump_legs,
        t.reference_legs,
        t.no_origin_plans,
        t.ghost_origin_plans,
        t.rootless_plans,
        t.multi_root_routes,
        t.split_plans,
        t.waypoint_routes,
        t.turnless_routes,
        t.pinned_fields,
        t.scootered_routes,
        t.range_tight_turns,
        t.dark_but_feasible,
        t.widest_front,
        t.errors.len()
    );
    let mut classes: Vec<&str> = t.errors.iter().map(|e| e.1).collect();
    classes.sort_unstable();
    classes.dedup();
    for class in classes {
        let hits: Vec<_> = t.errors.iter().filter(|e| e.1 == class).collect();
        println!(
            "  {class}: {} (replay GAS_ORACLE_SEED={:#x})",
            hits.len(),
            hits[0].0
        );
        for (seed, _, m) in hits.iter().take(3) {
            println!(
                "    seed {seed:#x} {} {m}",
                SHAPES[(*seed % SHAPES.len() as u64) as usize]
            );
        }
    }
}

/// The seeded sweep: `0..CI_SEEDS` by default (seconds in a debug build), or the env range.
#[test]
fn gas_field_never_promises_more_than_the_independent_oracle_allows() {
    let seeds: Vec<u64> = if let Some(seed) = env_u64("GAS_ORACLE_SEED") {
        vec![seed]
    } else {
        let first = env_u64("GAS_ORACLE_FIRST").unwrap_or(0);
        let count = env_u64("GAS_ORACLE_SEEDS").unwrap_or(CI_SEEDS);
        (first..first + count).collect()
    };
    let default_run = std::env::var_os("GAS_ORACLE_SEED").is_none()
        && std::env::var_os("GAS_ORACLE_FIRST").is_none()
        && std::env::var_os("GAS_ORACLE_SEEDS").is_none();
    // An open finding's seeds are pinned by `open_findings_still_reproduce` instead.
    let open = |s: &u64| OPEN_FINDINGS.iter().any(|f| f.1 == *s);
    let t = run(seeds
        .into_iter()
        .filter(|s| !open(s))
        .chain(REGRESSION_SEEDS.iter().copied()));
    report("sweep", &t);
    if default_run {
        // The CI range must actually reach what the oracle was extended for: a route over a jump
        // and an opted-in reference leg, plans with no origin and with several roots, caves in
        // several parts, way points, pins and routes the scooter cap binds on.
        for (what, n) in [
            ("jump legs on a route", t.jump_legs),
            ("reference legs on a route", t.reference_legs),
            ("plans with no origin", t.no_origin_plans),
            ("plans with an unresolvable origin", t.ghost_origin_plans),
            ("plans with no root", t.rootless_plans),
            ("routes from several roots", t.multi_root_routes),
            ("caves in several parts", t.split_plans),
            ("way-point routes", t.waypoint_routes),
            ("pinned fields", t.pinned_fields),
            ("range-tight turns", t.range_tight_turns),
        ] {
            assert!(n > 0, "the CI seeds no longer reach {what}");
        }
    }
    assert!(
        t.errors.is_empty(),
        "{} oracle check(s) failed - see the classes and replay seeds above",
        t.errors.len()
    );
}

/// The open findings' seeds still fail, and only as their finding does.
#[test]
fn open_findings_still_reproduce() {
    for &(id, seed, classes) in OPEN_FINDINGS {
        let t = run(std::iter::once(seed));
        report(id, &t);
        assert!(
            !t.errors.is_empty(),
            "{id}: seed {seed:#x} passes now - if the finding is fixed, move the seed to \
             REGRESSION_SEEDS"
        );
        for (_, class, m) in &t.errors {
            assert!(
                classes.contains(class),
                "{id}: seed {seed:#x} fails with {class}, not the finding's own classes: {m}"
            );
        }
    }
}
