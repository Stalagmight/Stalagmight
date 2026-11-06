//! [#576 RV8c-2] A pinned plan is monotone in its gas, measured against the plan without its pins.
//!
//! The rule this replaced (NF-PGAS3-1) priced a pin only when the pin-free route passed it, so
//! LESS gas - which can move the pin-free route to another branch, or end it short of the pin -
//! dropped the pin, handed back the gas it stranded, and reached further (RV8c-2: 27 of 60,000
//! cross-build scenarios, and on one build alone with fills below the Z == 1 knee). With the pin
//! deciding the route (D1-D3) the route does not depend on the gas at all, so for one fixed pinned
//! plan lowering a gas input can only shrink it.
//!
//! Two sweeps share one set of checks. The tree sweep: random tree caves, rigs, endpoints, way
//! points, scooters and pins (anywhere, including off every route), one input lowered at a time -
//! a back-gas fill, a stage fill, a cylinder's size. Fills stay where the strategy's own
//! per-cylinder credit is monotone in them: at or below the rating under the conservative cap, up
//! to 300 bar without it. Above the rating under the cap, and under Free with a usable allowance
//! below the fill, the decided gauge rule (#576 A18(e)) credits a LOWER fill MORE even without a
//! pin - the same usable bar sits lower on the Z curve - so there only the cylinder's size is
//! lowered. The loop sweep (#576 RV9a-1): caves with a deep loop off a shallow line - one end
//! merged onto it, the other tied to it by a loop closure - so a station on the line is reached two
//! ways, along the line on less gas or round the loop on less scooter range; most plans scootered,
//! with the range set near the line's length, half the drops pinned on the loop and most endpoints
//! on the line beyond it. It lowers every fill, those windows included (overfills under the cap
//! too), a Free allowance, a cylinder's size, and the scooter's range (a unit's burn time, or a
//! larger reserve). Each has a twin drawing scooter plans with one or two way points, every span
//! scootered - in the loop caves half the way points on the line past the loop - so the legs to
//! way points and pins are checked as the open part of the route is (#576 NF-W12GAS-1-D2: every
//! leg takes the cheapest gas its range covers).
//!
//! So what is asserted is scoped to what the PINS may do, not that less gas never shows more reach
//! (#576 RV9a-5-D1, "keep physics, document": in those windows a lower fill shows a larger budget
//! with or without pins, docs/gas-planning.md "A lower fill can hold more gas"). For every
//! lowering, with nothing set aside:
//! - the pin rules: the pinned budget rises by no more than the plan without pins' budget does,
//!   and not at all where that one does not rise - except where the lowering moves a pinned stage
//!   onto its default landing (#576 RV10a-1: in those windows a lower fill raises the stage's safe
//!   gas and can carry its default drop onto a deeper pin, which then reproduces the default, and
//!   the next stage starts at the default drop rather than the pin), where the pinned budget can
//!   rise by more but stays at or below the plan without pins' - counted; and a lowering that does
//!   not raise the pinned budget paints no new station, adds no turnaround, turns on a station
//!   painted before it, and on a gas lowering does not move the turn later (the route through the
//!   pins does not depend on the gas, D1-D4; a gas lowering never changes the route it commits
//!   through the way points and pins, asserted). A range lowering can change the route of a plan
//!   whose pins decide nothing (NF-W11GAS-1-D1: less range puts the cheaper way out of range and
//!   the plan takes a way round on more gas), so there the turn's route gas may rise - counted,
//!   within the budget and the lowered range. It can also send a leg to a way point or a pin
//!   another way (NF-W12GAS-1-D2), seen as the committed route changing: that plan is another
//!   dive - it passes other stations, its pins strand another share of the stages, and with less
//!   range spent on the way it can reach further after them - so it is held to what every plan
//!   is held to below, and counted;
//! - the tree sweep's inputs keep the plan without pins from gaining, so there the pinned budget
//!   never rises at all; the loop sweep counts the lowerings that raise the plan without pins.
//!
//! For every plan, lowered or not, pinned against pin-free: the pinned plan never has more budget,
//! never prices a station below the cheapest gas from the origin (the plan without pins, its rig
//! and its way points), never prices a station it paints below the plan without pins, and paints
//! only stations that plan paints; nor does the plan without pins price one below that cheapest
//! gas. Every station of a plan's route up to its turnaround, and the turnaround, is painted, and
//! the turn spends no more scooter range than the rig has (RV9a-1: the pinned dive turned round a
//! loop on a station the overlay left dark) - where every span is scootered at one speed, worked
//! out from the route's own shots too, and equal to what the field reports. Each sweep is a fixed
//! set of plans from a fixed seed, so it runs the same cases in the same few seconds every time;
//! and it runs again on three more seeds ([`RESEEDS`], #576 RV10a-1), each its own fixed set of
//! plans, so a rule that holds only on the seed a sweep was first run on fails here.
//!
//! Round a loop on a scooter a station is reached two ways: along the line on less gas, round the
//! loop on less range. Until NF-W11GAS-1 the plan without pins priced the range along its
//! gas-cheapest way only, so the pinned walk could paint a station that plan left dark on range
//! alone (375 on the loop sweep's seed); it now takes the cheapest-gas way its range covers, so a
//! pinned walk - one of those ways - paints nothing it leaves dark, and the sweep counts the
//! stations it paints on the way round instead. Out of range, the pinned walk can price a station
//! below that way, as the station is dark on it, so that price is compared with the cheapest gas
//! only. In a tree cave there is one way to each station, and nothing changes.
//!
//! The corner this sweep used to set aside is asserted now (#576 RV8c-2-D5, decided "next stage
//! starts at pin"): a stage pinned past its own tank-empty point ahead of a later stage. Its drop
//! snaps back to the empty point (#246), and the next stage used to be breathed from there, so a
//! SMALLER first stage emptied sooner, handed more of the route to the next one, and was credited
//! less of the stranding - 18 of 563 such lowerings raised the budget, one from 1302 to 1805 L
//! (rebuilt below). The next stage now starts at the pin.

#![expect(
    clippy::unwrap_used,
    reason = "tests construct from known-valid literals; a failed unwrap should fail the test"
)]

use super::model::{CylRole, Cylinder, Diver, GasReachField, PlanSegment, TankMaterial};
use super::physics::{deliverable_gas, edge_gas_basis};
use crate::config::{DpvRig, DpvUnit, GasConfig, ReservePreset};
use crate::{ClosureStrategy, Project, SectionData, ShotData, StationRef};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
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
}

fn random_shot(r: &mut Rng, depth: &mut f64, length: (f64, f64), drift: (f64, f64)) -> ShotData {
    let from = *depth;
    let length = r.range(length.0, length.1);
    let azimuth_in = r.range(0.0, 360.0);
    *depth = (*depth + r.range(drift.0, drift.1)).max(0.0);
    ShotData {
        length,
        azimuth_in,
        azimuth_out: 0.0,
        depth_in: from,
        depth_out: *depth,
        ..Default::default()
    }
}

/// A random tree cave: a first line, then branches each joined at its first station to a random
/// station already in the cave. Returns the project, the origin and every station.
fn random_cave(r: &mut Rng) -> (Project, StationRef, Vec<StationRef>) {
    let mut p = Project::new("Gas".to_string(), None);
    let mut stations: Vec<StationRef> = Vec::new();
    let sections = 2 + r.pick(4);
    for k in 0..sections {
        let legs = if k == 0 { 3 + r.pick(7) } else { 1 + r.pick(7) };
        let mut depth = r.range(0.0, 20.0);
        let shots = (0..legs)
            .map(|_| {
                let from = depth;
                depth = (depth + r.range(-5.0, 8.0)).max(0.0);
                ShotData {
                    length: r.range(20.0, 200.0),
                    azimuth_in: r.range(0.0, 360.0),
                    azimuth_out: 0.0,
                    depth_in: from,
                    depth_out: depth,
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
        let id = p
            .add_section(SectionData {
                shots,
                ..Default::default()
            })
            .id;
        if k > 0 {
            let at = stations[r.pick(stations.len())];
            p.merge_stations(at, StationRef::new(id, 0)).unwrap();
        }
        stations.extend((0..=legs).map(|i| StationRef::new(id, i as u32)));
    }
    let origin = stations[0];
    (p, origin, stations)
}

/// A cave with a loop (#576 RV9a-1): a shallow line from the origin, a deep loop of two or three
/// legs that leaves it at one station and comes back to it two to five stations on - closed by a
/// loop closure or by merging its last station with the line's - and up to two side passages,
/// some of them tied back into the cave too. A station past the loop is reached two ways: along
/// the line, shallow, the cheaper gas; or round the loop, shorter, the cheaper scooter range.
struct LoopCave {
    project: Project,
    origin: StationRef,
    stations: Vec<StationRef>,
    /// The loop's own stations, short of the one it closes on.
    on_loop: Vec<StationRef>,
    /// The line's stations past the loop, and the line's length to where the loop closes.
    beyond_loop: Vec<StationRef>,
    to_loop_end_m: f64,
}

fn random_loop_cave(r: &mut Rng) -> LoopCave {
    let mut p = Project::new("Gas".to_string(), None);
    let legs = 5 + r.pick(5);
    let mut depth = r.range(0.0, 10.0);
    let line_shots: Vec<ShotData> = (0..legs)
        .map(|_| {
            let mut s = random_shot(r, &mut depth, (50.0, 250.0), (-3.0, 3.0));
            s.depth_out = s.depth_out.min(15.0);
            depth = s.depth_out;
            s
        })
        .collect();
    let lengths: Vec<f64> = line_shots.iter().map(|s| s.length).collect();
    let line = p
        .add_section(SectionData {
            shots: line_shots,
            ..Default::default()
        })
        .id;
    let line_at = |i: usize| StationRef::new(line, i as u32);
    let leaves = r.pick(legs - 1);
    let closes = (leaves + 2 + r.pick(4)).min(legs);
    let loop_legs = 2 + r.pick(2);
    let mut depth = r.range(25.0, 45.0);
    let loop_shots: Vec<ShotData> = (0..loop_legs)
        .map(|_| random_shot(r, &mut depth, (40.0, 150.0), (-5.0, 5.0)))
        .collect();
    let deep = p
        .add_section(SectionData {
            shots: loop_shots,
            ..Default::default()
        })
        .id;
    // One end merged onto the line, the other tied to it by a loop closure (a merge refuses to
    // join two stations already connected), either way round.
    let (first, last) = (
        StationRef::new(deep, 0),
        StationRef::new(deep, loop_legs as u32),
    );
    let (merged, tied) = if r.unit() < 0.5 {
        ((line_at(leaves), first), (last, line_at(closes)))
    } else {
        ((line_at(closes), last), (first, line_at(leaves)))
    };
    p.merge_stations(merged.0, merged.1).unwrap();
    p.create_loop_closure(tied.0, tied.1, ClosureStrategy::Batch)
        .unwrap();
    let mut stations: Vec<StationRef> = (0..=legs).map(line_at).collect();
    stations.extend((0..=loop_legs).map(|i| StationRef::new(deep, i as u32)));
    for _ in 0..r.pick(3) {
        let side_legs = 1 + r.pick(4);
        let mut depth = r.range(0.0, 20.0);
        let shots: Vec<ShotData> = (0..side_legs)
            .map(|_| random_shot(r, &mut depth, (20.0, 200.0), (-5.0, 8.0)))
            .collect();
        let id = p
            .add_section(SectionData {
                shots,
                ..Default::default()
            })
            .id;
        let at = stations[r.pick(stations.len())];
        p.merge_stations(at, StationRef::new(id, 0)).unwrap();
        if r.unit() < 0.3 {
            let to = stations[r.pick(stations.len())];
            // A tie the model refuses (the two are already one station) leaves a plain branch.
            let _ = p.create_loop_closure(
                StationRef::new(id, side_legs as u32),
                to,
                ClosureStrategy::Batch,
            );
        }
        stations.extend((0..=side_legs).map(|i| StationRef::new(id, i as u32)));
    }
    LoopCave {
        project: p,
        origin: line_at(0),
        stations,
        on_loop: (1..loop_legs)
            .map(|i| StationRef::new(deep, i as u32))
            .collect(),
        beyond_loop: (closes + 1..=legs).map(line_at).collect(),
        to_loop_end_m: lengths[..closes].iter().sum(),
    }
}

/// A random cylinder. Under the conservative cap its fill stays at or below the rating, unless
/// `overfill`, which lets it run up to 15% past it (to 300 bar at most).
fn random_cylinder(r: &mut Rng, role: CylRole, conservative: bool, overfill: bool) -> Cylinder {
    let rated = [176.0, 200.0, 207.0, 232.0, 300.0][r.pick(5)];
    let fill = if conservative {
        let top = if overfill {
            (rated * 1.15_f64).min(300.0).max(rated)
        } else {
            rated
        };
        r.range(30.0, top)
    } else {
        r.range(30.0, 300.0)
    };
    Cylinder {
        usable_gas_bar: r.range(0.0, 250.0),
        ..Cylinder::new(
            r.range(3.0, 20.0),
            rated,
            fill,
            TankMaterial::Steel,
            role,
            None,
        )
        .unwrap()
    }
}

/// One drawn plan: everything a field is computed from, bar the cylinders and the config the
/// sweep lowers.
struct Plan {
    project: Project,
    origin: StationRef,
    endpoint: Option<StationRef>,
    waypoints: Vec<StationRef>,
    segments: Vec<PlanSegment>,
    pins: Vec<Option<StationRef>>,
    preset: ReservePreset,
    conservative: bool,
    backs: usize,
    rig: Vec<Cylinder>,
    cfg: GasConfig,
}

impl Plan {
    fn field(
        &self,
        rig: &[Cylinder],
        cfg: &GasConfig,
        pins: &[Option<StationRef>],
    ) -> GasReachField {
        self.field_and_chain(rig, cfg, pins).0
    }

    /// The field, and the route the plan commits through its way points and pins
    /// ([`Project::gas_reach_field_and_chain`]).
    fn field_and_chain(
        &self,
        rig: &[Cylinder],
        cfg: &GasConfig,
        pins: &[Option<StationRef>],
    ) -> (GasReachField, Vec<StationRef>) {
        self.project.gas_reach_field_and_chain(
            Some(&self.origin),
            self.endpoint.as_ref(),
            &self.waypoints,
            &self.segments,
            &Diver::new(20.0, None).unwrap(),
            rig,
            cfg,
            pins,
        )
    }

    fn scootered(&self) -> bool {
        self.cfg.dpv.is_some()
    }
}

/// The strategy, the rig and the stage reserve, drawn the same way for both sweeps (`overfill`
/// as in [`random_cylinder`]).
fn random_rig(
    r: &mut Rng,
    overfill: bool,
) -> (ReservePreset, bool, usize, Vec<Cylinder>, GasConfig) {
    let preset = [
        ReservePreset::OneStage,
        ReservePreset::TwoStage,
        ReservePreset::Free,
    ][r.pick(3)];
    let conservative = r.unit() < 0.7;
    let backs = 1 + r.pick(2);
    let mut rig: Vec<Cylinder> = (0..backs)
        .map(|_| random_cylinder(r, CylRole::BackGas, conservative, overfill))
        .collect();
    rig.extend((0..=r.pick(3)).map(|_| random_cylinder(r, CylRole::Stage, conservative, overfill)));
    let mut cfg = GasConfig {
        default_reserve: preset,
        conservative_tank_gas: conservative,
        ..GasConfig::default()
    };
    cfg.reserve_profile.stage_reserve_bar = r.range(0.0, 60.0);
    (preset, conservative, backs, rig, cfg)
}

/// The tree sweep's plan, drawn exactly as the sweep always has.
fn tree_plan(r: &mut Rng) -> Plan {
    draw_tree_plan(r, false)
}

/// [#576 NF-W12GAS-1] The tree sweep's plan on a scooter with one or two way points, every span
/// scootered, so each turn past a way point has its range worked out from the route.
fn tree_waypoint_plan(r: &mut Rng) -> Plan {
    draw_tree_plan(r, true)
}

/// `legs`: a scooter on every span and one or two way points (else the tree sweep's own draw,
/// unchanged).
fn draw_tree_plan(r: &mut Rng, legs: bool) -> Plan {
    let (project, origin, stations) = random_cave(r);
    let (preset, conservative, backs, rig, mut cfg) = random_rig(r, false);
    let scootered = legs || r.unit() < 0.25;
    if scootered {
        cfg.dpv = Some(DpvRig {
            primary: DpvUnit {
                speed_m_per_min: r.range(25.0, 60.0),
                burn_minutes: r.range(5.0, 60.0),
            },
            backup: DpvUnit {
                speed_m_per_min: r.range(25.0, 60.0),
                burn_minutes: r.range(5.0, 60.0),
            },
            range_reserve_fraction: r.range(0.0, 0.4),
        });
    }
    cfg.normalize();
    let endpoint = (r.unit() < 0.5).then(|| stations[r.pick(stations.len())]);
    let waypoints: Vec<StationRef> = if legs {
        (0..=r.pick(2))
            .map(|_| stations[r.pick(stations.len())])
            .collect()
    } else if r.unit() < 0.25 {
        vec![stations[r.pick(stations.len())]]
    } else {
        Vec::new()
    };
    let segments: Vec<PlanSegment> = (0..=waypoints.len())
        .map(|_| PlanSegment {
            speed_m_per_min: None,
            on_scooter: scootered && (legs || r.unit() < 0.7),
        })
        .collect();
    let pins: Vec<Option<StationRef>> = (0..=r.pick(2))
        .map(|_| (r.unit() < 0.85).then(|| stations[r.pick(stations.len())]))
        .collect();
    Plan {
        project,
        origin,
        endpoint,
        waypoints,
        segments,
        pins,
        preset,
        conservative,
        backs,
        rig,
        cfg,
    }
}

/// The loop sweep's plan: a [`random_loop_cave`], cylinders that can be overfilled under the cap,
/// a scooter on four plans in five, ridden below both units' cruise so each metre of line spends
/// more than a metre of range, with the usable range set to between 0.3 and 1.5 times the line's
/// length to the loop's end; most endpoints on the line past the loop, half the pins on the loop.
fn loop_plan(r: &mut Rng) -> Plan {
    draw_loop_plan(r, false)
}

/// [#576 NF-W12GAS-1] The loop sweep's plan on a scooter with one or two way points, every span
/// scootered, half the way points on the line past the loop - so the leg to one is two ways from
/// the station before it, along the line on less gas or round the loop on less range - a quarter
/// on the loop, the rest anywhere; the pins as the loop sweep draws them.
fn loop_waypoint_plan(r: &mut Rng) -> Plan {
    draw_loop_plan(r, true)
}

/// `legs`: a scooter on every span and one or two way points (else the loop sweep's own draw,
/// unchanged).
fn draw_loop_plan(r: &mut Rng, legs: bool) -> Plan {
    let cave = random_loop_cave(r);
    let (preset, conservative, backs, rig, mut cfg) = random_rig(r, true);
    let scootered = legs || r.unit() < 0.8;
    let speed = r.range(10.0, 22.0);
    if scootered {
        // A unit ridden at `speed`, below its cruise, covers `speed x burn` of line; the rig's
        // reach in line is the shorter unit's, less the reserve.
        let reserve = r.range(0.0, 0.4);
        let line_m = r.range(0.3, 1.5) * cave.to_loop_end_m;
        let short_burn = line_m / ((1.0 - reserve) * speed);
        let long_burn = short_burn * r.range(1.0, 2.0);
        let (primary_burn, backup_burn) = if r.unit() < 0.5 {
            (short_burn, long_burn)
        } else {
            (long_burn, short_burn)
        };
        cfg.dpv = Some(DpvRig {
            primary: DpvUnit {
                speed_m_per_min: r.range(25.0, 60.0),
                burn_minutes: primary_burn,
            },
            backup: DpvUnit {
                speed_m_per_min: r.range(25.0, 60.0),
                burn_minutes: backup_burn,
            },
            range_reserve_fraction: reserve,
        });
    }
    cfg.normalize();
    let stations = &cave.stations;
    let endpoint = (r.unit() < 0.8).then(|| {
        if !cave.beyond_loop.is_empty() && r.unit() < 0.7 {
            cave.beyond_loop[r.pick(cave.beyond_loop.len())]
        } else {
            stations[r.pick(stations.len())]
        }
    });
    let waypoints: Vec<StationRef> = if legs {
        (0..=r.pick(2))
            .map(|_| match r.pick(4) {
                0 | 1 if !cave.beyond_loop.is_empty() => {
                    cave.beyond_loop[r.pick(cave.beyond_loop.len())]
                }
                2 => cave.on_loop[r.pick(cave.on_loop.len())],
                _ => stations[r.pick(stations.len())],
            })
            .collect()
    } else if r.unit() < 0.15 {
        vec![stations[r.pick(stations.len())]]
    } else {
        Vec::new()
    };
    let segments: Vec<PlanSegment> = (0..=waypoints.len())
        .map(|_| PlanSegment {
            speed_m_per_min: Some(speed),
            on_scooter: scootered && (legs || r.unit() < 0.9),
        })
        .collect();
    let pins: Vec<Option<StationRef>> = (0..=r.pick(2))
        .map(|_| {
            (r.unit() < 0.85).then(|| {
                if r.unit() < 0.5 {
                    cave.on_loop[r.pick(cave.on_loop.len())]
                } else {
                    stations[r.pick(stations.len())]
                }
            })
        })
        .collect();
    Plan {
        project: cave.project,
        origin: cave.origin,
        endpoint,
        waypoints,
        segments,
        pins,
        preset,
        conservative,
        backs,
        rig,
        cfg,
    }
}

/// Metres of range one metre of line costs when ridden at `speed`, worked out from the rig's own
/// numbers (docs/gas-planning.md "Segments and the DPV"), not read off the engine: whichever
/// unit's battery a metre drains harder, scaled to the weaker unit's line.
pub(super) fn range_per_metre_of(rig: &DpvRig, speed: f64) -> f64 {
    let weaker = rig.primary.range_m().min(rig.backup.range_m());
    let term = |u: &DpvUnit| weaker / (u.burn_minutes * speed.min(u.speed_m_per_min));
    term(&rig.primary).max(term(&rig.backup)).max(1.0)
}

/// The line `f`'s route swims to its turnaround, summed from the project's own shot lengths (a
/// merge or closure tie swims nothing) - what the scooter is charged for, independently of the
/// engine's riders.
pub(super) fn route_line_m(project: &Project, f: &GasReachField) -> f64 {
    f.route
        .iter()
        .filter(|l| {
            l.from.section_id == l.to.section_id
                && l.from.station_index.abs_diff(l.to.station_index) == 1
        })
        .map(|l| {
            let section = project
                .sections
                .iter()
                .find(|s| s.id == l.from.section_id)
                .unwrap();
            section.shots[l.from.station_index.min(l.to.station_index) as usize].length
        })
        .sum()
}

fn reach_within(smaller: &GasReachField, larger: &GasReachField) -> Option<StationRef> {
    debug_assert_eq!(smaller.station_refs, larger.station_refs);
    smaller
        .reachable
        .iter()
        .zip(&larger.reachable)
        .position(|(&a, &b)| a && !b)
        .map(|i| smaller.station_refs[i])
}

/// Whether lowering stage `k` (its index among the plan's stage drops) is the D5 corner the module
/// doc names - counted so the sweep shows it reaches it: stage `k` is pinned, has a later stage
/// behind it, and reads empty at its drop in either plan - breathed to tank-empty before its pin.
/// In this sweep's domain (fills at or below the rating under the cap, or no cap) a drained
/// stage's gauge reads 0.
fn drains_ahead_of_another(
    k: Option<usize>,
    pins: &[Option<StationRef>],
    plans: [&GasReachField; 2],
) -> bool {
    let Some(k) = k else { return false };
    pins.get(k).copied().flatten().is_some()
        && plans
            .iter()
            .any(|f| k + 1 < f.stage_drop_bars.len() && f.stage_drop_bars[k] < 1e-6)
}

/// Route gas at the turnaround: the last outbound profile point (0 for a route with no swum leg);
/// `None` with no turnaround.
fn turn_gas(f: &GasReachField) -> Option<f64> {
    f.final_turn_ref?;
    Some(
        f.route_profile
            .iter()
            .rev()
            .find(|p| p.outbound)
            .map_or(0.0, |p| p.gas_l),
    )
}

/// RV9a-1: every station of `f`'s route up to its turnaround, and the turnaround, is painted, and
/// the turn spends no more scooter range than the rig has. Where every span is scootered at the
/// plan's one speed (NF-W11GAS-1) the range is also worked out from the route itself - its shots'
/// line times what a metre costs on `cfg`'s rig ([`route_line_m`], [`range_per_metre_of`]) - and
/// must match what the field reports and fit the range; `t.range_oracle_turns` counts those turns.
/// Returns how many route stations it checked and whether it worked the range out from the route,
/// or what it found wrong. A plan with no turnaround has no route to check.
fn route_is_painted(
    plan: &Plan,
    cfg: &GasConfig,
    f: &GasReachField,
    t: &mut Tally,
) -> Result<(usize, bool), String> {
    let Some(turn) = f.final_turn_ref else {
        return Ok((0, false));
    };
    let mut range_from_route = false;
    if let Some(limit) = f.scooter_range_limit_m {
        if f.scooter_distance_m > limit + 1e-6 {
            return Err(format!(
                "the turn spends {:.2} m of a {limit:.2} m scooter range",
                f.scooter_distance_m
            ));
        }
        if let (Some(rig), true) = (&cfg.dpv, plan.segments.iter().all(|s| s.on_scooter)) {
            let speed = plan.segments[0].speed_m_per_min.unwrap_or(
                cfg.survey_speed_m_per_min
                    .unwrap_or(cfg.swim_speed_m_per_min),
            );
            let along_route = route_line_m(&plan.project, f) * range_per_metre_of(rig, speed);
            if (along_route - f.scooter_distance_m).abs() > 1e-6 * along_route.max(1.0)
                || along_route > limit + 1e-6
            {
                return Err(format!(
                    "the route to the turn spends {along_route:.2} m of a {limit:.2} m range (the \
                     field says {:.2} m)",
                    f.scooter_distance_m
                ));
            }
            t.range_oracle_turns += 1;
            range_from_route = true;
        }
    }
    let route: Vec<StationRef> = f
        .route
        .first()
        .map(|l| l.from)
        .into_iter()
        .chain(f.route.iter().map(|l| l.to))
        .chain([turn])
        .collect();
    for s in &route {
        let i = f.station_refs.iter().position(|r| r == s).unwrap();
        if !f.reachable[i] {
            return Err(format!("route station {s:?} is dark (turn {turn:?})"));
        }
    }
    Ok((route.len(), range_from_route))
}

/// Lower one input of `plan`: a cylinder's fill or size, or - with `every_input` - a Free
/// allowance or the scooter's range. Without `every_input` (the tree sweep) a fill is lowered only
/// where the strategy's own credit is monotone in it (module doc); with it, any fill. Returns what
/// it did, the lowered rig and config, and the cylinder it lowered, if it was one.
fn lower_one(
    r: &mut Rng,
    plan: &Plan,
    every_input: bool,
) -> (String, Vec<Cylinder>, GasConfig, Option<usize>) {
    let mut rig = plan.rig.clone();
    let mut cfg = plan.cfg.clone();
    if every_input && plan.scootered() && r.unit() < 0.3 {
        let dpv = cfg.dpv.as_mut().unwrap();
        let what = match r.pick(3) {
            0 => {
                let from = dpv.primary.burn_minutes;
                dpv.primary.burn_minutes = (from * r.range(0.3, 0.99)).max(1.0);
                format!(
                    "primary burn {from:.2} -> {:.2} min",
                    dpv.primary.burn_minutes
                )
            }
            1 => {
                let from = dpv.backup.burn_minutes;
                dpv.backup.burn_minutes = (from * r.range(0.3, 0.99)).max(1.0);
                format!(
                    "backup burn {from:.2} -> {:.2} min",
                    dpv.backup.burn_minutes
                )
            }
            _ => {
                let from = dpv.range_reserve_fraction;
                dpv.range_reserve_fraction = (from + r.range(0.01, 0.4)).min(0.9);
                format!(
                    "range reserve {from:.3} -> {:.3}",
                    dpv.range_reserve_fraction
                )
            }
        };
        return (what, rig, cfg, None);
    }
    let i = r.pick(rig.len());
    let c = &mut rig[i];
    let fill_is_monotone = plan.preset != ReservePreset::Free
        && (!plan.conservative || c.fill_bar <= c.rated_pressure_bar);
    let what = if every_input && plan.preset == ReservePreset::Free && r.unit() < 0.25 {
        let from = c.usable_gas_bar;
        c.usable_gas_bar = from * r.range(0.0, 0.99);
        format!(
            "{:?} {i} usable {from:.2} -> {:.2} bar",
            c.role, c.usable_gas_bar
        )
    } else if (every_input || fill_is_monotone) && r.unit() < 0.6 {
        let from = c.fill_bar;
        c.fill_bar = (from * r.range(0.4, 0.99)).max(5.0);
        format!("{:?} {i} fill {from:.2} -> {:.2}", c.role, c.fill_bar)
    } else {
        let from = c.water_volume_l;
        c.water_volume_l = (from * r.range(0.4, 0.99)).max(0.5);
        format!(
            "{:?} {i} size {from:.3} -> {:.3} L",
            c.role, c.water_volume_l
        )
    };
    (what, rig, cfg, Some(i))
}

#[derive(Default)]
struct Tally {
    plans: usize,
    runs: usize,
    lowered_budget: usize,
    lowered_range: usize,
    /// Lowerings that raised the budget of the plan without pins: a Free allowance below the fill
    /// or an overfill under the cap, taken lower on the Z curve (module doc).
    pin_free_rose: usize,
    /// Lowerings that raised the pinned budget - each within what the plan without pins gained.
    pinned_rose: usize,
    drained_corner: usize,
    scootered: usize,
    range_bound_turns: usize,
    route_stations: usize,
    /// Stations a pinned plan paints that the plan without its pins leaves dark on range alone.
    /// Until NF-W11GAS-1 that plan priced the range along its gas-cheapest way only, and round a
    /// loop this was allowed; it now takes a way its range covers, so this must stay 0.
    range_only: usize,
    /// Stations the plan without pins paints on more gas than their cheapest way: round a loop,
    /// the cheaper gas is out of scooter range and the plan takes the way round (NF-W11GAS-1-D1).
    rerouted: usize,
    /// Turns of the plan without pins on such a station.
    rerouted_turns: usize,
    /// Turns of scootered plans (pinned or not) whose range was worked out from the route.
    range_oracle_turns: usize,
    /// Scootered plans without pins checked, lowered or not.
    pin_free_scootered: usize,
    /// Range lowerings after which the plan turns on more route gas: the way its lowered range
    /// covers costs more gas than the way it took before (NF-W11GAS-1-D1).
    range_rerouted_turns: usize,
    /// Of the turns whose range was worked out from the route: those past a way point, and those
    /// on a route the pins decide (it differs from the plan without pins') (NF-W12GAS-1-D2).
    waypoint_oracle_turns: usize,
    pinned_oracle_turns: usize,
    /// Range lowerings that sent a leg to a way point or a pin another way (NF-W12GAS-1-D2: the
    /// route [`Plan::field_and_chain`] commits changed); of them, those that raised the pinned
    /// budget (the pins then strand another share of the stages), painted a station dark before,
    /// or turned on one.
    range_moved_way: usize,
    range_moved_rose: usize,
    range_moved_painted: usize,
    range_moved_turned: usize,
    /// Lowerings that raised the pinned budget by more than the plan without pins' by moving a
    /// pinned stage onto its default landing (RV10a-1, [`pin_moved_onto_default`]).
    default_landing_rises: usize,
    violations: Vec<String>,
}

impl Tally {
    /// Add another sweep's counts to these (a reseeded run of the same sweep).
    fn add(&mut self, o: Tally) {
        self.plans += o.plans;
        self.runs += o.runs;
        self.lowered_budget += o.lowered_budget;
        self.lowered_range += o.lowered_range;
        self.pin_free_rose += o.pin_free_rose;
        self.pinned_rose += o.pinned_rose;
        self.drained_corner += o.drained_corner;
        self.scootered += o.scootered;
        self.range_bound_turns += o.range_bound_turns;
        self.route_stations += o.route_stations;
        self.range_only += o.range_only;
        self.rerouted += o.rerouted;
        self.rerouted_turns += o.rerouted_turns;
        self.range_oracle_turns += o.range_oracle_turns;
        self.pin_free_scootered += o.pin_free_scootered;
        self.range_rerouted_turns += o.range_rerouted_turns;
        self.waypoint_oracle_turns += o.waypoint_oracle_turns;
        self.pinned_oracle_turns += o.pinned_oracle_turns;
        self.range_moved_way += o.range_moved_way;
        self.range_moved_rose += o.range_moved_rose;
        self.range_moved_painted += o.range_moved_painted;
        self.range_moved_turned += o.range_moved_turned;
        self.default_landing_rises += o.default_landing_rises;
        self.violations.extend(o.violations);
    }
}

/// The pinned plan never exceeds the plan without its pins: never more budget, never a station
/// priced below the cheapest gas from the origin, never one painted that the plan without pins
/// leaves dark - and where it paints one, never priced below that plan. `cheapest` is the plan
/// without pins, without its scooter rig and without its way points: the least gas of any route
/// from the origin to each station (every span of a sweep's plan flies at one speed, so one span
/// prices every leg alike), a floor no plan's price can go under, the plan without pins' own
/// included. A station the pinned walk reaches out of range can be priced below the in-range way
/// the plan without pins takes to it - it is dark, so that price claims no reach - so the
/// comparison with the scootered plan is over the stations the pinned plan paints, and with the
/// floor over all. `gas_only` is the plan without pins with no rig but its own way points, which
/// is what the plan without pins reported before it priced the range along the ways it takes
/// (NF-W11GAS-1-D1, NF-W12GAS-1-D2): a station it paints on more gas than that is counted as
/// rerouted.
fn check_against_pin_free(
    plan: &Plan,
    cfg: &GasConfig,
    fields: [&GasReachField; 4],
    t: &mut Tally,
) -> Vec<String> {
    let [pinned, pin_free, gas_only, cheapest] = fields;
    let mut found = Vec::new();
    if pinned.budget_litres > pin_free.budget_litres + 1e-9 {
        found.push(format!(
            "pinned budget {:.3} L above the pin-free {:.3} L",
            pinned.budget_litres, pin_free.budget_litres
        ));
    }
    let below = |with: f64, without: f64| with < without - 1e-6 * without.max(1.0);
    for (i, s) in pinned.station_refs.iter().enumerate() {
        let (with, without, floor, unranged) = (
            pinned.gas_to_reach[i],
            pin_free.gas_to_reach[i],
            cheapest.gas_to_reach[i],
            gas_only.gas_to_reach[i],
        );
        match (with, without, floor) {
            (Some(with), _, Some(floor)) if below(with, floor) => {
                found.push(format!(
                    "pin priced {s:?} at {with:.3} < the cheapest {floor:.3} L"
                ));
            }
            (Some(with), Some(without), _) if pinned.reachable[i] && below(with, without) => {
                found.push(format!("pin painted {s:?} at {with:.3} < {without:.3} L"));
            }
            (Some(_), None, _) => found.push(format!("pin priced unreachable {s:?}")),
            _ => {}
        }
        if let (Some(without), Some(floor)) = (without, floor) {
            if below(without, floor) {
                found.push(format!(
                    "the plan without pins priced {s:?} at {without:.3} < the cheapest {floor:.3} L"
                ));
            }
        }
        if pin_free.reachable[i]
            && matches!((without, unranged), (Some(w), Some(c)) if w > c + 1e-6 * c.max(1.0))
        {
            t.rerouted += 1;
            t.rerouted_turns += usize::from(pin_free.final_turn_ref == Some(*s));
        }
        if !pinned.reachable[i] || pin_free.reachable[i] {
            continue;
        }
        if without.is_some_and(|g| g <= pin_free.budget_litres) {
            t.range_only += 1;
        }
        found.push(format!("pin made {s:?} reachable"));
    }
    let pins_route = pinned.route != pin_free.route;
    for (f, which, pins_route) in [
        (pinned, "pinned", pins_route),
        (pin_free, "pin-free", false),
    ] {
        match route_is_painted(plan, cfg, f, t) {
            Ok((n, range_from_route)) => {
                t.route_stations += n;
                // NF-W12GAS-1-D2: turns past a way point, or on a route the pins decide, whose
                // range was worked out from the route itself.
                t.waypoint_oracle_turns +=
                    usize::from(range_from_route && !plan.waypoints.is_empty());
                t.pinned_oracle_turns += usize::from(range_from_route && pins_route);
            }
            Err(e) => found.push(format!("{which}: {e}")),
        }
    }
    t.pin_free_scootered += usize::from(cfg.dpv.is_some());
    found
}

/// The reference fields [`check_against_pin_free`] reads besides the plan without pins: that plan
/// without its scooter rig (`gas_only`, the plan without pins itself when it has no rig), and that
/// one without its way points too (`cheapest`, `gas_only` itself when there are none).
fn reference_fields(
    plan: &Plan,
    rig: &[Cylinder],
    cfg: &GasConfig,
    pin_free: &GasReachField,
) -> (GasReachField, GasReachField) {
    let no_rig = GasConfig {
        dpv: None,
        ..cfg.clone()
    };
    let gas_only = if cfg.dpv.is_none() {
        pin_free.clone()
    } else {
        plan.field(rig, &no_rig, &[])
    };
    if plan.waypoints.is_empty() {
        return (gas_only.clone(), gas_only);
    }
    // One span at the plan's own speed: every span of a sweep's plan flies at it.
    let cheapest = plan.project.compute_gas_reach_field_with_overrides(
        Some(&plan.origin),
        None,
        &[],
        &plan.segments[..1],
        &Diver::new(20.0, None).unwrap(),
        rig,
        &no_rig,
        &[],
    );
    (gas_only, cheapest)
}

/// [#576 RV10a-1] Whether a lowering moved a pinned stage onto its default landing: priced as a pin
/// before (its drop gauge is not the strategy default the plan without pins reads) and as the
/// default after. In the windows RV9a-5-D1 keeps - a Free allowance below the fill, an overfill
/// under the cap - a lower fill raises the stage's safe IN gas and can carry the default threshold
/// onto a deeper pin; the pin then reproduces the default (a pin on its default station is the
/// default, `price_stage_drops`), and the next stage starts at the threshold instead of the pin,
/// so it is credited the route between them. The pinned budget can then rise by more than the
/// plan without pins' does - never above that plan's, which every lowered plan is checked against.
fn pin_moved_onto_default(
    pins: &[Option<StationRef>],
    before: [&GasReachField; 2],
    after: [&GasReachField; 2],
) -> bool {
    let default = |pinned: &GasReachField, pin_free: &GasReachField, k: usize| {
        matches!(
            (pinned.stage_drop_bars.get(k), pin_free.stage_drop_bars.get(k)),
            (Some(a), Some(b)) if (a - b).abs() <= 1e-9 * b.abs().max(1.0)
        )
    };
    pins.iter().enumerate().any(|(k, p)| {
        p.is_some()
            && k < before[0].stage_drop_bars.len()
            && !default(before[0], before[1], k)
            && default(after[0], after[1], k)
    })
}

fn sweep(seed: u64, plans: usize, draw: fn(&mut Rng) -> Plan, every_input: bool) -> Tally {
    let mut r = Rng(seed);
    let mut t = Tally {
        plans,
        ..Tally::default()
    };
    for case in 0..plans {
        let plan = draw(&mut r);
        let pins = &plan.pins;
        let (pinned, committed) = plan.field_and_chain(&plan.rig, &plan.cfg, pins);
        let pin_free = plan.field(&plan.rig, &plan.cfg, &[]);
        t.scootered += usize::from(plan.scootered());
        t.range_bound_turns += usize::from(pinned.turn_on_scooter_range);
        let label = |what: &str| {
            format!(
                "case {case} {:?} cons {} dpv {} end {:?} wp {:?} pins {pins:?}: {what}",
                plan.preset,
                plan.conservative,
                plan.scootered(),
                plan.endpoint,
                plan.waypoints
            )
        };
        let (gas_only, cheapest) = reference_fields(&plan, &plan.rig, &plan.cfg, &pin_free);
        let found = check_against_pin_free(
            &plan,
            &plan.cfg,
            [&pinned, &pin_free, &gas_only, &cheapest],
            &mut t,
        );
        t.violations.extend(found.iter().map(|f| label(f)));
        for _ in 0..3 {
            let (what, rig, cfg, lowered_cyl) = lower_one(&mut r, &plan, every_input);
            t.runs += 1;
            t.lowered_range += usize::from(lowered_cyl.is_none());
            let (after, committed_after) = plan.field_and_chain(&rig, &cfg, pins);
            let free_after = plan.field(&rig, &cfg, &[]);
            let (rise, free_rise) = (
                after.budget_litres - pinned.budget_litres,
                free_after.budget_litres - pin_free.budget_litres,
            );
            t.pin_free_rose += usize::from(free_rise > 1e-9);
            t.pinned_rose += usize::from(rise > 1e-9);
            if rise < -1e-9 {
                t.lowered_budget += 1;
            }
            // NF-W12GAS-1-D2: a range lowering can send a leg to a way point or a pin another way
            // - the cheaper way is out of the lowered range, and the leg takes a way round on more
            // gas - seen as the route the plan commits through them changing. The plan is then
            // another dive: the pins strand another share of the stages, it passes other stations,
            // and with less range spent on the way there it can reach further after them, so the
            // comparisons below with the plan before do not apply to it. What is asserted of it is
            // what is asserted of every plan, lowered or not (`check_against_pin_free`): no more
            // budget than the plan without pins, nothing painted that plan leaves dark, and its
            // route and turn painted and within the range, worked out from its own shots. Counted,
            // not set aside. Nothing else moves a leg: the way does not depend on the gas (D1-D4),
            // which is asserted too.
            let moved = lowered_cyl.is_none() && committed_after != committed;
            t.range_moved_way += usize::from(moved);
            let mut found: Vec<String> = Vec::new();
            if lowered_cyl.is_some() && committed_after != committed {
                found.push(format!(
                    "{what}: a gas lowering sent a leg to a way point or a pin another way"
                ));
            }
            // The pin rules, scoped against the plan without pins: the pins never add budget a
            // lowering takes away - the pinned budget rises by no more than that plan's, and not
            // at all where that plan's does not. RV10a-1: in the windows RV9a-5-D1 keeps, a lower
            // fill can carry a stage's default drop onto a deeper pin, and then the pinned budget
            // can rise by more - up to, never past, the plan without pins' (checked below).
            if rise > free_rise.max(0.0) + 1e-9 {
                if moved {
                    t.range_moved_rose += 1;
                } else if pin_moved_onto_default(pins, [&pinned, &pin_free], [&after, &free_after])
                {
                    t.default_landing_rises += 1;
                } else {
                    found.push(format!(
                        "{what}: budget {:.3} -> {:.3} L, the plan without pins {:.3} -> {:.3} L",
                        pinned.budget_litres,
                        after.budget_litres,
                        pin_free.budget_litres,
                        free_after.budget_litres
                    ));
                }
            }
            // One fixed plan on no more budget (the range never rises either): nothing is newly
            // painted, no turnaround appears, and the turn is a station the plan painted before.
            // Its route does not depend on the gas, so a gas lowering turns no later on it. A range
            // lowering can change the route of a plan without pins (NF-W11GAS-1-D1): the cheaper
            // way is out of the lowered range and the plan takes a way round on more gas, to the
            // same station or one it painted before, within the budget and the lowered range
            // (`route_is_painted`); those turns are counted, not set aside.
            if moved {
                t.range_moved_painted += usize::from(reach_within(&after, &pinned).is_some());
                t.range_moved_turned += usize::from(after.final_turn_ref.is_some_and(|turn| {
                    let i = pinned.station_refs.iter().position(|r| *r == turn).unwrap();
                    !pinned.reachable[i]
                }));
            } else if rise <= 1e-9 {
                if let Some(s) = reach_within(&after, &pinned) {
                    found.push(format!("{what}: {s:?} became reachable"));
                }
                if let Some(turn) = after.final_turn_ref {
                    let i = pinned.station_refs.iter().position(|r| *r == turn).unwrap();
                    if !pinned.reachable[i] {
                        found.push(format!("{what}: turns at {turn:?}, dark before"));
                    }
                }
                match (turn_gas(&pinned), turn_gas(&after)) {
                    (Some(before), Some(now)) if now > before + 1e-6 && lowered_cyl.is_none() => {
                        t.range_rerouted_turns += 1;
                    }
                    (Some(before), Some(now)) if now > before + 1e-6 => found.push(format!(
                        "{what}: turn moved later, {before:.2} -> {now:.2} L of route gas ({:?} \
                         -> {:?})",
                        pinned.final_turn_ref, after.final_turn_ref
                    )),
                    (None, Some(_)) => found.push(format!("{what}: a turnaround appeared")),
                    _ => {}
                }
            }
            let (gas_only_after, cheapest_after) = reference_fields(&plan, &rig, &cfg, &free_after);
            let lowered = check_against_pin_free(
                &plan,
                &cfg,
                [&after, &free_after, &gas_only_after, &cheapest_after],
                &mut t,
            );
            found.extend(lowered.iter().map(|f| format!("{what}: lowered: {f}")));
            let stage_k = lowered_cyl
                .filter(|&i| plan.rig[i].role == CylRole::Stage)
                .map(|i| i - plan.backs);
            if drains_ahead_of_another(stage_k, pins, [&pinned, &after]) {
                t.drained_corner += 1;
            }
            t.violations.extend(found.iter().map(|f| label(f)));
        }
    }
    t
}

fn report(name: &str, t: &Tally) {
    println!(
        "pinned monotonicity sweep, {name}: {} lowered inputs ({} of them the scooter's range) \
         over {} pinned plans ({} scootered, {} turning on the range), {} lowered the budget, {} \
         raised the budget of the plan without pins (Free allowance or overfill) and {} the pinned \
         budget, {} in the D5 corner (a stage drained ahead of another), {} route stations up to the turn checked \
         painted, {} stations painted that the plan without pins leaves dark on range alone, 0 \
         set aside, {} violations; the plan without pins: {} scootered fields checked, {} stations \
         painted on more gas than their cheapest way (the in-range way round), {} turns there; {} \
         turns whose range was worked out from the route ({} past a way point, {} on a route the \
         pins decide); {} range lowerings turning on more route gas, on a way round the lowered \
         range covers; {} range lowerings sending a leg to a way point or a pin another way ({} \
         raising the pinned budget, {} painting a station dark before, {} turning on one); {} \
         lowerings raising the pinned budget by moving a pin onto its default landing, within the \
         plan without pins",
        t.runs,
        t.lowered_range,
        t.plans,
        t.scootered,
        t.range_bound_turns,
        t.lowered_budget,
        t.pin_free_rose,
        t.pinned_rose,
        t.drained_corner,
        t.route_stations,
        t.range_only,
        t.violations.len(),
        t.pin_free_scootered,
        t.rerouted,
        t.rerouted_turns,
        t.range_oracle_turns,
        t.waypoint_oracle_turns,
        t.pinned_oracle_turns,
        t.range_rerouted_turns,
        t.range_moved_way,
        t.range_moved_rose,
        t.range_moved_painted,
        t.range_moved_turned,
        t.default_landing_rises
    );
    for v in t.violations.iter().take(8) {
        println!("  {v}");
    }
}

/// [#576 RV10a-1] The seeds every sweep is also run on, each over a fixed set of plans of its own,
/// so a rule that holds only on the seed a sweep was first run on fails here rather than on the
/// next reseed. On 0x5EED_0004 the review's reseed found the loop sweep's default-landing rise.
const RESEEDS: [u64; 3] = [0x5EED_0004, 0x5EED_0007, 0x5EED_0008];

/// The sweep on `seed` over `plans` plans, and on each of [`RESEEDS`] over `reseed_plans`: the
/// first run's tally (what each sweep's must-bite counts read) and the reseeded runs' together.
fn sweeps(
    seed: u64,
    (plans, reseed_plans): (usize, usize),
    draw: fn(&mut Rng) -> Plan,
    every_input: bool,
) -> (Tally, Tally) {
    let main = sweep(farm_seed(seed), plans, draw, every_input);
    let mut reseeded = Tally::default();
    for s in RESEEDS {
        reseeded.add(sweep(farm_seed(s), reseed_plans, draw, every_input));
    }
    (main, reseeded)
}

/// `seed`, or with `STALAGMIGHT_GAS_PIN_SEED=<n>` set a seed of its own per `n`, so a long run
/// can sweep plans no CI run has drawn: every sweep and every reseed moves together, each to a
/// different seed. Unset, every sweep runs the fixed seeds above, as it always has.
fn farm_seed(seed: u64) -> u64 {
    match farm_pin_seed() {
        Some(n) => seed ^ n.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
        None => seed,
    }
}

/// The `n` of `STALAGMIGHT_GAS_PIN_SEED=<n>`, when a long run set one.
fn farm_pin_seed() -> Option<u64> {
    std::env::var("STALAGMIGHT_GAS_PIN_SEED")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
}

#[test]
fn in_a_tree_cave_less_gas_never_lets_a_pinned_plan_gain_more_than_without_its_pins() {
    let (t, reseeded) = sweeps(0x2545_F491_4F6C_DD1D, (2500, 1000), tree_plan, false);
    report("tree caves", &t);
    report("tree caves, reseeded", &reseeded);
    assert!(
        reseeded.violations.is_empty() && reseeded.pin_free_rose == 0,
        "reseeded: {} violations, {} lowerings raising the plan without pins",
        reseeded.violations.len(),
        reseeded.pin_free_rose
    );
    assert!(
        t.runs >= 7500 && t.lowered_budget > t.runs / 4,
        "the sweep must bite"
    );
    assert!(
        t.drained_corner > 100,
        "the sweep must reach the D5 corner ({})",
        t.drained_corner
    );
    assert_eq!(t.range_only, 0, "a tree cave has one way to each station");
    // NF-W11GAS-1: one way to each station, so the plan without pins never needs the way round,
    // and every turn spends no more range than the rig has, read off the route itself.
    assert_eq!(t.rerouted, 0, "a tree cave has one way to each station");
    assert!(
        t.pin_free_scootered > 1000 && t.range_oracle_turns > 500,
        "the sweep must check scootered plans ({} without pins, {} turns off the route)",
        t.pin_free_scootered,
        t.range_oracle_turns
    );
    // This sweep's inputs stay where the strategy's credit is monotone in them, so the plan
    // without pins never gains, and the pin rules say the pinned budget never rises at all.
    assert_eq!(
        t.pin_free_rose, 0,
        "test domain: a lowering raised the plan without pins"
    );
    assert!(
        t.violations.is_empty(),
        "{} pinned plans gained more than the plan without their pins on less gas, or turned on a \
         station the map leaves dark",
        t.violations.len()
    );
}

#[test]
fn round_a_loop_less_gas_or_range_never_lets_a_pinned_plan_gain_more_than_without_its_pins() {
    // #576 RV9a-1: the sweep above round a deep loop, where the pinned walk and the plan without
    // pins take different ways to a station, most plans scootered, the range lowered too, and
    // (RV9a-5-D1) every fill, a Free allowance below the fill and an overfill under the cap
    // included - where a lower fill can raise the budget, pins or none.
    let (t, reseeded) = sweeps(0x9A1C_0FFE_E5EE_D001, (2500, 1000), loop_plan, true);
    report("caves with a loop", &t);
    report("caves with a loop, reseeded", &reseeded);
    assert!(
        reseeded.violations.is_empty(),
        "reseeded: {} violations",
        reseeded.violations.len()
    );
    assert!(
        t.runs >= 7500 && t.lowered_budget > t.runs / 5,
        "the sweep must bite"
    );
    assert!(
        t.scootered > 1750 && t.lowered_range > 1000 && t.range_bound_turns > 300,
        "the sweep must fly scooters round the loops ({} scootered, {} range lowerings, {} turns \
         on the range)",
        t.scootered,
        t.lowered_range,
        t.range_bound_turns
    );
    assert!(
        t.violations.is_empty(),
        "{} pinned plans gained more than the plan without their pins on less gas or range, \
         reached past it on gas, or turned on a station the map leaves dark",
        t.violations.len()
    );
    // The geometry RV9a-1 needs: a station reached in range only round the loop. The pinned walk
    // took the dive there and the plan without pins, pricing the range along the line, did not
    // (375 stations on this seed before NF-W11GAS-1); it now takes the way round itself, so no pin
    // paints a station it leaves dark, and the way round is what it paints them on.
    assert_eq!(
        t.range_only, 0,
        "a pinned plan painted a station the plan without pins leaves dark on range"
    );
    assert!(
        t.rerouted > 100 && t.rerouted_turns > 20,
        "the sweep must reach stations painted only on the way round ({} stations, {} turns)",
        t.rerouted,
        t.rerouted_turns
    );
    assert!(
        t.pin_free_scootered > 7000 && t.range_oracle_turns > 5000,
        "the sweep must check scootered plans ({} without pins, {} turns off the route)",
        t.pin_free_scootered,
        t.range_oracle_turns
    );
    // And the windows RV9a-5-D1 keeps: lowerings that raise the plan without pins.
    assert!(
        t.pin_free_rose > 50,
        "the sweep must reach the Free and overfill windows ({})",
        t.pin_free_rose
    );
}

#[test]
fn in_a_tree_cave_a_scooter_plan_through_way_points_turns_within_its_range() {
    // #576 NF-W12GAS-1: the tree sweep with one or two way points on a scooter, every span
    // scootered, so every turn past a way point - and every turn on a route its pins decide - has
    // its range worked out from the route's own shots and checked against what the field reports
    // and the rig's range. One way to each station, so no leg can take another way.
    let (t, reseeded) = sweeps(
        0x7EE5_3A1D_0C0F_FEE5,
        (1500, 500),
        tree_waypoint_plan,
        false,
    );
    report("tree caves, scooter plans with way points", &t);
    report(
        "tree caves, scooter plans with way points, reseeded",
        &reseeded,
    );
    for (which, t) in [("seeded", &t), ("reseeded", &reseeded)] {
        assert!(
            t.violations.is_empty(),
            "{which}: {} violations",
            t.violations.len()
        );
        assert_eq!(
            (t.rerouted, t.range_moved_way, t.pin_free_rose),
            (0, 0, 0),
            "{which}: a tree cave has one way to each station, and this domain keeps the credit \
             monotone"
        );
    }
    // The floors are what the fixed seeds reach (1306 and 179 turns). A farm seed draws other
    // caves: over 240 of them the counts were 1319 +- 99 and 167 +- 25, the fewest 1089 and 107,
    // and the farm met 95 on routes the pins decide (#576 W27). So a farm seed is held to a floor
    // no draw comes near, which still fails a sweep that stopped reaching either domain.
    let (waypoint_floor, pinned_floor) = if farm_pin_seed().is_some() {
        (500, 40)
    } else {
        (1000, 100)
    };
    assert!(
        t.waypoint_oracle_turns > waypoint_floor && t.pinned_oracle_turns > pinned_floor,
        "the sweep must check the range at turns past way points ({}) and on routes the pins \
         decide ({})",
        t.waypoint_oracle_turns,
        t.pinned_oracle_turns
    );
}

#[test]
fn round_a_loop_the_legs_to_way_points_and_pins_take_a_way_their_range_covers() {
    // #576 NF-W12GAS-1-D2: the loop sweep with one or two way points on a scooter, every span
    // scootered, half the way points on the line past the loop: the leg to such a way point - or
    // to a pin on the loop - is two ways from the station before it, and the cheaper gas is often
    // out of range. Every leg takes the cheapest gas its range covers, so the plan reaches its way
    // points and pins round the loop, and every check of the loop sweep holds: no pin paints a
    // station the plan without pins leaves dark, every route and turn is painted and within the
    // range worked out from its own shots. A range lowering that sends a leg another way is a
    // different dive, and counted.
    let (t, reseeded) = sweeps(0x5CA1_AB1E_0BAD_F00D, (1500, 500), loop_waypoint_plan, true);
    report("caves with a loop, scooter plans with way points", &t);
    report(
        "caves with a loop, scooter plans with way points, reseeded",
        &reseeded,
    );
    for (which, t) in [("seeded", &t), ("reseeded", &reseeded)] {
        assert!(
            t.violations.is_empty(),
            "{which}: {} violations",
            t.violations.len()
        );
    }
    assert!(
        t.waypoint_oracle_turns > 2500 && t.pinned_oracle_turns > 300,
        "the sweep must check the range at turns past way points ({}) and on routes the pins \
         decide ({})",
        t.waypoint_oracle_turns,
        t.pinned_oracle_turns
    );
    assert!(
        t.rerouted > 100 && t.range_moved_way > 20,
        "the sweep must reach legs that take the way round ({} stations painted on it, {} range \
         lowerings sending a leg another way)",
        t.rerouted,
        t.range_moved_way
    );
}

/// Every simple route from `from` in a graph whose edges carry (gas, range): for each station, the
/// least gas over all routes, and the least gas over the routes within `limit` of range.
fn cheapest_by_enumeration(
    edges: &[Vec<(usize, f64, f64)>],
    from: usize,
    limit: f64,
) -> (Vec<f64>, Vec<f64>) {
    fn walk(
        edges: &[Vec<(usize, f64, f64)>],
        at: usize,
        (gas, range): (f64, f64),
        limit: f64,
        on_route: &mut [bool],
        best: &mut (Vec<f64>, Vec<f64>),
    ) {
        best.0[at] = best.0[at].min(gas);
        if range <= limit {
            best.1[at] = best.1[at].min(gas);
        }
        on_route[at] = true;
        for &(v, g, s) in &edges[at] {
            if !on_route[v] && g.is_finite() {
                walk(edges, v, (gas + g, range + s), limit, on_route, best);
            }
        }
        on_route[at] = false;
    }
    let n = edges.len();
    let mut best = (vec![f64::INFINITY; n], vec![f64::INFINITY; n]);
    walk(
        edges,
        from,
        (0.0, 0.0),
        limit,
        &mut vec![false; n],
        &mut best,
    );
    best
}

/// The project's graph with each edge's (gas, range) as `plan` flies it, every span on the
/// scooter at its first span's speed, worked out from the rig and the config (not the engine's
/// riders), and the station index it is keyed by.
fn priced_edges(
    plan: &Plan,
    rig: &DpvRig,
) -> (
    Vec<Vec<(usize, f64, f64)>>,
    std::collections::HashMap<StationRef, usize>,
) {
    let speed = plan.segments[0].speed_m_per_min.unwrap();
    let cfg = &plan.cfg;
    let coords = plan.project.compute_station_coordinates();
    let index = Project::station_index_map(&coords);
    let z: Vec<f64> = coords.iter().map(|s| s.coordinates.z).collect();
    let basis = plan
        .project
        .build_survey_graph_weighted(coords.len(), &index, |shot, fi, ti| {
            if !shot.kind.is_swum() && !cfg.include_reference_legs {
                return f64::INFINITY;
            }
            let mult = if shot.kind.is_virtual() {
                cfg.jump_rmv_multiplier
            } else {
                1.0
            };
            edge_gas_basis(shot.length.max(0.0), z[fi], z[ti]) * mult
        });
    let lens = plan
        .project
        .build_survey_graph_weighted(coords.len(), &index, |shot, _, _| shot.length.max(0.0));
    let (gas_per_basis, per_metre) = (
        20.0 * cfg.survey_rmv_multiplier / speed,
        range_per_metre_of(rig, speed),
    );
    let edges = basis
        .iter()
        .zip(&lens)
        .map(|(b, l)| {
            b.iter()
                .zip(l)
                .map(|(&(v, w), &(_, len))| (v, w * gas_per_basis, len * per_metre))
                .collect()
        })
        .collect();
    (edges, index)
}

#[test]
fn a_way_point_leg_takes_the_cheapest_gas_its_range_covers() {
    // #576 NF-W12GAS-1-D2, checked against every simple route: on loop caves with one way point
    // on the scooter (the way-point loop sweep's draw), the way point is reached on the least gas
    // of the routes from the origin within the range - the way round the loop where the cheaper
    // gas is out of range - or, where no route is in range, on the least gas of all: the
    // gas-cheapest way the leg then keeps, on which the plan cannot fly the way point.
    let mut r = Rng(0x0057_6A11_0E12_5EED);
    let (mut plans, mut round, mut none_in_range) = (0usize, 0usize, 0usize);
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0);
    for _ in 0..4000 {
        let plan = loop_waypoint_plan(&mut r);
        let Some(rig) = plan.cfg.dpv.clone() else {
            continue;
        };
        if plan.waypoints.len() != 1 {
            continue;
        }
        let field = plan.field(&plan.rig, &plan.cfg, &[]);
        let (edges, index) = priced_edges(&plan, &rig);
        let limit = field.scooter_range_limit_m.unwrap();
        let (cheapest, in_range) = cheapest_by_enumeration(&edges, index[&plan.origin], limit);
        let w = index[&plan.waypoints[0]];
        let expected = if in_range[w].is_finite() {
            in_range[w]
        } else {
            cheapest[w]
        };
        let got = field.waypoint_gas_litres[0].unwrap();
        assert!(
            near(got, expected),
            "way point {:?} reached on {got:.6} L; the cheapest gas in range is {:.6} L, the \
             cheapest of all {:.6} L",
            plan.waypoints[0],
            in_range[w],
            cheapest[w]
        );
        plans += 1;
        round += usize::from(in_range[w].is_finite() && in_range[w] > cheapest[w] + 1e-9);
        none_in_range += usize::from(!in_range[w].is_finite());
    }
    println!(
        "way-point legs by enumeration: {plans} plans, {round} way points reached on more gas than \
         their cheapest way (the in-range way round), {none_in_range} with no way in range"
    );
    assert!(
        plans > 1500 && round > 60 && none_in_range > 100,
        "the check must bite ({plans} plans, {round} round the loop, {none_in_range} out of range)"
    );
}

#[test]
fn without_pins_a_scooter_plan_paints_each_station_at_the_cheapest_gas_its_range_covers() {
    // #576 NF-W11GAS-1-D1, checked against every simple route: on loop caves (the loop sweep's
    // draw, single-span scooter plans without way points), a station is painted exactly when some
    // route from the origin reaches it within the budget and the range, and it is priced at the
    // least gas among those routes. The engine's search is the decided rule, not a heuristic.
    let mut r = Rng(0x0057_6A11_0E11_5EED);
    let (mut plans, mut stations, mut rerouted, mut by_range) = (0usize, 0usize, 0usize, 0usize);
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0);
    for _ in 0..2000 {
        let plan = loop_plan(&mut r);
        let Some(rig) = plan.cfg.dpv.clone() else {
            continue;
        };
        if !plan.waypoints.is_empty() || !plan.segments.iter().all(|s| s.on_scooter) {
            continue;
        }
        let field = plan.field(&plan.rig, &plan.cfg, &[]);
        let (edges, index) = priced_edges(&plan, &rig);
        let origin = index[&plan.origin];
        let limit = field.scooter_range_limit_m.unwrap();
        let (cheapest, in_range) = cheapest_by_enumeration(&edges, origin, limit);
        plans += 1;
        for i in 0..edges.len() {
            let (budget, s) = (field.budget_litres, field.station_refs[i]);
            if near(in_range[i], budget) {
                continue; // on the budget's edge, a rounding either way
            }
            stations += 1;
            assert_eq!(
                field.reachable[i],
                in_range[i] <= budget,
                "{s:?}: painted {} but the cheapest gas in range is {:.3} of {budget:.3} L",
                field.reachable[i],
                in_range[i]
            );
            if field.reachable[i] {
                let priced = field.gas_to_reach[i].unwrap();
                assert!(
                    near(priced, in_range[i]),
                    "{s:?} priced {priced:.6} L, the cheapest in range is {:.6} L",
                    in_range[i]
                );
                rerouted += usize::from(in_range[i] > cheapest[i] + 1e-9);
            } else {
                by_range += usize::from(cheapest[i] <= budget);
            }
        }
    }
    println!(
        "cheapest gas in range, by enumeration: {plans} plans, {stations} stations checked, \
         {rerouted} painted on more gas than their cheapest way, {by_range} dark on range alone"
    );
    assert!(plans > 1000 && stations > 10000, "the check must bite");
    assert!(
        rerouted > 150 && by_range > 40,
        "the check must reach both ways round"
    );
}

/// The sweep's case 791, rebuilt: a five-leg line (lengths and depths as the sweep drew them,
/// rounded to the millimetre), with two side passages joined at its end, 0:5.
fn case_791_cave() -> (Project, StationRef, [StationRef; 2]) {
    let mut p = Project::new("Gas".to_string(), None);
    let leg = |length: f64, depth_in: f64, depth_out: f64| ShotData {
        length,
        depth_in,
        depth_out,
        ..Default::default()
    };
    let line = p
        .add_section(SectionData {
            shots: vec![
                leg(114.447, 0.414, 4.986),
                leg(123.952, 4.986, 7.380),
                leg(195.989, 7.380, 4.909),
                leg(144.319, 4.909, 12.504),
                leg(23.379, 12.504, 9.073),
            ],
            ..Default::default()
        })
        .id;
    let end = StationRef::new(line, 5);
    let mut side = |shots: Vec<ShotData>| {
        let id = p
            .add_section(SectionData {
                shots,
                ..Default::default()
            })
            .id;
        p.merge_stations(end, StationRef::new(id, 0)).unwrap();
        StationRef::new(id, 0)
    };
    let first = side(vec![leg(150.895, 9.073, 8.289), leg(35.917, 8.289, 6.731)]);
    let second = side(vec![
        leg(161.595, 9.073, 12.904),
        leg(113.914, 12.904, 16.607),
    ]);
    (p, StationRef::new(line, 0), [first, second])
}

#[test]
fn case_791_a_smaller_first_stage_no_longer_hands_the_route_to_the_second() {
    // #576 RV8c-2-D5, the W9-GAS sweep's worst set-aside case. TwoStage under the conservative cap,
    // the endpoint back at the entrance, both drops pinned at the end of the line (stage 1 on one
    // side passage's first station, stage 2 on the other's - the same place, 0:5). Stage 1 holds
    // more than the route to the pin, so stage 2 is dropped there full and its gas stranded:
    // 1302 L, and the dive turns at 0:2, short of the pins.
    //
    // Lowering stage 1's fill from 130.61 to 54.05 bar empties it before the pin. Breathed from
    // that empty point, stage 2 was credited the rest of the way to the pin and the budget went UP
    // to 1805 L - the turn moved on to 0:3 on less gas. Starting stage 2 at the pin, it stays full
    // and stranded, and the budget falls by exactly what the pin-free plan loses.
    let (project, origin, [first_side, second_side]) = case_791_cave();
    let diver = Diver::new(20.0, None).unwrap();
    let mut cfg = GasConfig {
        default_reserve: ReservePreset::TwoStage,
        conservative_tank_gas: true,
        ..GasConfig::default()
    };
    cfg.reserve_profile.stage_reserve_bar = 1.936;
    cfg.normalize();
    let cylinder = |volume: f64, rated: f64, fill: f64, role: CylRole| {
        Cylinder::new(volume, rated, fill, TankMaterial::Steel, role, None).unwrap()
    };
    let rig = |stage_1_fill: f64| {
        [
            cylinder(15.696, 300.0, 114.360, CylRole::BackGas),
            cylinder(15.986, 207.0, stage_1_fill, CylRole::Stage),
            cylinder(13.002, 232.0, 205.065, CylRole::Stage),
        ]
    };
    let pins = [Some(second_side), Some(first_side)];
    let plan = |stage_1_fill: f64, pins: &[Option<StationRef>]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            Some(&origin),
            &[],
            &[],
            &diver,
            &rig(stage_1_fill),
            &cfg,
            pins,
        )
    };
    let full = plan(130.61, &pins);
    let lowered = plan(54.05, &pins);
    let (free_full, free_lowered) = (plan(130.61, &[]), plan(54.05, &[]));
    let show = |label: &str, f: &GasReachField| {
        println!(
            "CASE-791 {label}: budget {:.2} L, turn {:?}, drop bars {:?}, over {:?}",
            f.budget_litres,
            f.final_turn_ref.map(|s| (s.section_id, s.station_index)),
            f.stage_drop_bars
                .iter()
                .map(|b| (b * 100.0).round() / 100.0)
                .collect::<Vec<_>>(),
            f.stage_drop_over_reserve
        );
    };
    show("stage 1 at 130.61 bar", &full);
    show("stage 1 at 54.05 bar", &lowered);
    assert!(
        lowered.stage_drop_bars[0] < 1e-6,
        "the lowered stage 1 must run dry before its pin ({:.2} bar)",
        lowered.stage_drop_bars[0]
    );
    for (f, what) in [(&full, "130.61"), (&lowered, "54.05")] {
        assert!(
            (f.stage_drop_bars[1] - 205.065).abs() < 1e-6,
            "stage 1 at {what} bar: stage 2 breathed before the shared pin ({:.2} bar left)",
            f.stage_drop_bars[1]
        );
    }
    assert!(
        lowered.budget_litres < full.budget_litres,
        "less stage gas raised the budget: {:.2} -> {:.2} L",
        full.budget_litres,
        lowered.budget_litres
    );
    let free_loss = free_full.budget_litres - free_lowered.budget_litres;
    assert!(
        ((full.budget_litres - lowered.budget_litres) - free_loss).abs() < 1e-6,
        "the pinned plan loses {:.2} L, the plan without pins {free_loss:.2} L",
        full.budget_litres - lowered.budget_litres
    );
    assert!(
        reach_within(&lowered, &full).is_none(),
        "less gas painted a station"
    );
    let (before, after) = (turn_gas(&full).unwrap(), turn_gas(&lowered).unwrap());
    assert!(
        after <= before + 1e-9,
        "the turn moved later: {before:.2} -> {after:.2} L"
    );
}

#[test]
fn a_lower_fill_can_be_credited_more_under_a_free_allowance_or_an_overfill_under_the_cap() {
    // #576 RV9a-5-D1, "keep physics, document" (docs/gas-planning.md, "A lower fill can hold more
    // gas"): where a rule spends a set number of bar whatever the fill - a Free allowance below
    // the fill, or the share of the rating an overfill is held to under the cap - a lower fill
    // takes that bar lower in the cylinder, where a bar holds more gas, so it is credited more.
    // The rule itself does not move: each budget is exactly that bar off its own gauge, and the
    // turn gauge reads the fill less that bar, the reserve below it never credited. The doc's
    // numbers, for a 12 L steel tank rated 207 bar, back gas only, under the cap.
    let mut project = Project::new("Gas".to_string(), None);
    let line = project
        .add_section(SectionData {
            shots: vec![ShotData {
                length: 50.0,
                ..Default::default()
            }],
            ..Default::default()
        })
        .id;
    let gas_between = |from: f64, to: f64| {
        deliverable_gas(from, 12.0, f64::INFINITY, false)
            - deliverable_gas(to, 12.0, f64::INFINITY, false)
    };
    let plan = |preset: ReservePreset, fill: f64, usable: f64| {
        let cfg = GasConfig {
            default_reserve: preset,
            ..GasConfig::default()
        };
        assert!(
            cfg.conservative_tank_gas,
            "the doc's numbers are under the cap"
        );
        let tank = Cylinder {
            usable_gas_bar: usable,
            ..Cylinder::new(
                12.0,
                207.0,
                fill,
                TankMaterial::Steel,
                CylRole::BackGas,
                None,
            )
            .unwrap()
        };
        project.compute_gas_reach_field(
            Some(&StationRef::new(line, 0)),
            None,
            &Diver::new(20.0, None).unwrap(),
            &[tank],
            &cfg,
        )
    };
    // (strategy, the bar its rule spends, [(fill, the doc's litres)]): thirds of the 207-bar
    // rating is 69 bar at any fill under the cap; the Free allowance is the diver's 100 bar.
    let cases = [
        (
            ReservePreset::AllThirds,
            69.0,
            [(250.0, 681.0), (207.0, 739.8)],
        ),
        (
            ReservePreset::Free,
            100.0,
            [(250.0, 1018.0), (207.0, 1096.0)],
        ),
    ];
    for (preset, spent, fills) in cases {
        let [high, low] = fills.map(|(fill, litres)| {
            let f = plan(preset, fill, spent);
            let turn = f.backgas_turn_bar.unwrap();
            println!(
                "RV9a-5 {preset:?} at {fill} bar: spends {:.1} bar, turns at {turn:.1} bar, {:.2} L",
                f.cylinder_usable_bars[0], f.budget_litres
            );
            assert!(
                (f.cylinder_usable_bars[0] - spent).abs() < 1e-9,
                "{preset:?} at {fill} bar: the rule spends {spent} bar"
            );
            assert!(
                (turn - (fill - spent)).abs() < 1e-9,
                "{preset:?} at {fill} bar: the turn gauge is the fill less {spent} bar"
            );
            assert!(
                (f.budget_litres - gas_between(fill, fill - spent)).abs() < 1e-9,
                "{preset:?} at {fill} bar: the budget is that bar off its own gauge"
            );
            assert!(
                (f.budget_litres - litres).abs() < 0.5,
                "{preset:?} at {fill} bar: {:.2} L, the doc says {litres} L",
                f.budget_litres
            );
            f.budget_litres
        });
        assert!(
            low > high,
            "{preset:?}: the lower fill holds more gas in the same {spent} bar ({high:.2} -> \
             {low:.2} L)"
        );
    }
}
