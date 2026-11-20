//! The steps of the datum command-chain fuzzer (`multi_datum_fuzz_tests`):
//! what a step is, how a trail spells and parses it, how a chain picks the
//! next one, and how it runs through the command path. Synthetic surveys
//! only.
//!
//! On the station graph a slot (`line:index`) is a view address of a station;
//! a junction has several. A datum is addressed through a slot of its station
//! (E1: Set and Remove act on the station's datum attached `via` the slot's
//! line, else its first one), so a datum step that cannot address the datum
//! it names (two datums on one station, both via the same line) is skipped,
//! not run against another datum.

use std::collections::BTreeSet;
use std::fmt;

use super::AppState;
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{
    local_to_gps, ClosureStrategy, CombineJoinEnds, CombineLinesOptions, CombinePick,
    Coordinates, GpsCoordinates, LineEnd, LineId, ShotData, SplitAt, SplitLineOptions, SplitSide,
    StationId, StationRef,
};

/// Accuracies a datum is given: the default (a hard pin), an explicit hard
/// pin, and soft ones up to the largest the app accepts.
pub(super) const ACCURACIES: [Option<f64>; 6] = [
    None,
    Some(0.0),
    Some(0.5),
    Some(5.0),
    Some(50.0),
    Some(1000.0),
];

pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0xA24B_AED4_963E_E407) | 1)
    }

    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub(super) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub(super) fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    pub(super) fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        (!items.is_empty()).then(|| items[self.below(items.len())])
    }
}

// ---------------------------------------------------------------------------
// Steps, as a trail spells them
// ---------------------------------------------------------------------------

/// One step of a chain. Everything a step needs is in it, so a trail replays
/// without the generator; a GPS is given as an offset from where the survey
/// puts the station when the step runs.
#[derive(Debug, Clone)]
pub(super) enum Op {
    /// Set a datum on `at`, its GPS `(dx, dy)` from where the frame puts the
    /// station (from the chain's origin when there is no frame). On a station
    /// that holds a datum it is a GPS edit of that datum (E1).
    Datum {
        at: StationRef,
        dx: f64,
        dy: f64,
        acc: Option<f64>,
    },
    /// Set a datum on `at` with datum `k`'s GPS, bit for bit.
    Twin {
        at: StationRef,
        k: usize,
        acc: Option<f64>,
    },
    /// Set datum `k` again with its own GPS: only the accuracy changes.
    Accuracy { k: usize, acc: Option<f64> },
    /// Remove datum `k`.
    Remove { k: usize },
    Merge { a: StationRef, b: StationRef },
    /// Detach `b`'s line from the station `a` and `b` share.
    Unmerge { a: StationRef, b: StationRef },
    Closure { a: StationRef, b: StationRef },
    /// Split `line` at station `at`. `side_b` is kept in the trail for old
    /// replay strings; the halves share the station now (plan 5.4, S9), so
    /// there is no datum side to choose.
    Split { line: LineId, at: u32, side_b: bool },
    Combine {
        kept: LineId,
        kept_end: LineEnd,
        other: LineId,
        other_end: LineEnd,
    },
    DeleteLine { line: LineId },
    DeleteShot { line: LineId, index: usize },
    /// Add a shot: `pos` -1 before the first shot, `k` after shot `k`, or
    /// `None` at the end.
    AddShot {
        line: LineId,
        pos: Option<i64>,
        azimuth: f64,
        length: f64,
        depth_in: f64,
        depth_out: f64,
    },
    /// Move Line by `(dx, dy)`: a free piece's placement moves (refused on a
    /// datum-held piece).
    MoveLine { line: LineId, dx: f64, dy: f64 },
    Undo,
    Redo,
    Reopen,
}

fn st(s: StationRef) -> String {
    format!("{}:{}", s.line_id, s.station_index)
}

fn acc_text(a: Option<f64>) -> String {
    a.map_or_else(|| "-".to_string(), |v| format!("{v:?}"))
}

fn end_text(e: LineEnd) -> &'static str {
    match e {
        LineEnd::Start => "start",
        LineEnd::End => "end",
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Op::Datum { at, dx, dy, acc } => {
                write!(f, "datum({},{dx:?},{dy:?},{})", st(*at), acc_text(*acc))
            }
            Op::Twin { at, k, acc } => write!(f, "twin({},{k},{})", st(*at), acc_text(*acc)),
            Op::Accuracy { k, acc } => write!(f, "acc({k},{})", acc_text(*acc)),
            Op::Remove { k } => write!(f, "rm({k})"),
            Op::Merge { a, b } => write!(f, "merge({},{})", st(*a), st(*b)),
            Op::Unmerge { a, b } => write!(f, "unmerge({},{})", st(*a), st(*b)),
            Op::Closure { a, b } => write!(f, "closure({},{})", st(*a), st(*b)),
            Op::Split { line, at, side_b } => {
                write!(f, "split({line},{at},{})", if *side_b { "b" } else { "a" })
            }
            Op::Combine {
                kept,
                kept_end,
                other,
                other_end,
            } => write!(
                f,
                "combine({kept}:{},{other}:{})",
                end_text(*kept_end),
                end_text(*other_end)
            ),
            Op::DeleteLine { line } => write!(f, "delsec({line})"),
            Op::DeleteShot { line, index } => write!(f, "delshot({line},{index})"),
            Op::AddShot {
                line,
                pos,
                azimuth,
                length,
                depth_in,
                depth_out,
            } => write!(
                f,
                "addshot({line},{},{azimuth:?},{length:?},{depth_in:?},{depth_out:?})",
                pos.map_or_else(|| "end".to_string(), |p| p.to_string())
            ),
            Op::MoveLine { line, dx, dy } => write!(f, "moveline({line},{dx:?},{dy:?})"),
            Op::Undo => write!(f, "undo"),
            Op::Redo => write!(f, "redo"),
            Op::Reopen => write!(f, "reopen"),
        }
    }
}

fn parse_station(text: &str) -> StationRef {
    let (s, i) = text.split_once(':').unwrap();
    StationRef::new(s.trim().parse().unwrap(), i.trim().parse().unwrap())
}

fn parse_acc(text: &str) -> Option<f64> {
    (text != "-").then(|| text.parse().unwrap())
}

fn parse_end(text: &str) -> (LineId, LineEnd) {
    let (s, e) = text.split_once(':').unwrap();
    let end = if e == "start" {
        LineEnd::Start
    } else {
        LineEnd::End
    };
    (s.parse().unwrap(), end)
}

pub(super) fn parse_op(text: &str) -> Op {
    let text = text.trim();
    let (name, args) = match text.split_once('(') {
        Some((name, rest)) => (name, rest.trim_end_matches(')')),
        None => (text, ""),
    };
    let a: Vec<&str> = args.split(',').map(str::trim).collect();
    let num = |i: usize| -> f64 { a[i].parse().unwrap() };
    let int = |i: usize| -> usize { a[i].parse().unwrap() };
    match name {
        "datum" => Op::Datum {
            at: parse_station(a[0]),
            dx: num(1),
            dy: num(2),
            acc: parse_acc(a[3]),
        },
        "twin" => Op::Twin {
            at: parse_station(a[0]),
            k: int(1),
            acc: parse_acc(a[2]),
        },
        "acc" => Op::Accuracy {
            k: int(0),
            acc: parse_acc(a[1]),
        },
        "rm" => Op::Remove { k: int(0) },
        "merge" => Op::Merge {
            a: parse_station(a[0]),
            b: parse_station(a[1]),
        },
        "unmerge" => Op::Unmerge {
            a: parse_station(a[0]),
            b: parse_station(a[1]),
        },
        "closure" => Op::Closure {
            a: parse_station(a[0]),
            b: parse_station(a[1]),
        },
        "split" => Op::Split {
            line: a[0].parse().unwrap(),
            at: a[1].parse().unwrap(),
            side_b: a[2] == "b",
        },
        "combine" => {
            let ((kept, kept_end), (other, other_end)) = (parse_end(a[0]), parse_end(a[1]));
            Op::Combine {
                kept,
                kept_end,
                other,
                other_end,
            }
        }
        "delsec" => Op::DeleteLine {
            line: a[0].parse().unwrap(),
        },
        "delshot" => Op::DeleteShot {
            line: a[0].parse().unwrap(),
            index: int(1),
        },
        "addshot" => Op::AddShot {
            line: a[0].parse().unwrap(),
            pos: (a[1] != "end").then(|| a[1].parse().unwrap()),
            azimuth: num(2),
            length: num(3),
            depth_in: num(4),
            depth_out: num(5),
        },
        "moveline" => Op::MoveLine {
            line: a[0].parse().unwrap(),
            dx: num(1),
            dy: num(2),
        },
        "undo" => Op::Undo,
        "redo" => Op::Redo,
        "reopen" => Op::Reopen,
        other => panic!("unknown step {other:?}"),
    }
}

/// Every slot of `p`, line by line.
pub(super) fn every_station(p: &Project) -> Vec<StationRef> {
    p.lines()
        .iter()
        .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| StationRef::new(s.id, i)))
        .collect()
}

/// `count` random shots, each starting at the depth the last ended.
pub(super) fn shots(rng: &mut Rng, count: usize) -> Vec<ShotData> {
    let mut depth = rng.range(0.0, 30.0);
    (0..count)
        .map(|_| {
            let shot = random_shot(rng, depth);
            depth = shot.depth_out;
            shot
        })
        .collect()
}

/// One shot: random bearing and length, a depth change under the length.
pub(super) fn random_shot(rng: &mut Rng, depth_in: f64) -> ShotData {
    let length = rng.range(3.0, 20.0);
    let depth_out = (depth_in + rng.range(-0.6, 0.6) * length).max(0.0);
    let azimuth = rng.range(0.0, 360.0);
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

/// A random datum step on `at`: GPS 0-100 m from the station, or on it.
pub(super) fn datum_op(rng: &mut Rng, at: StationRef) -> Op {
    let (r, angle) = if rng.chance(10) {
        (0.0, 0.0)
    } else {
        (rng.range(0.0, 100.0), rng.range(0.0, std::f64::consts::TAU))
    };
    Op::Datum {
        at,
        dx: r * angle.sin(),
        dy: r * angle.cos(),
        acc: ACCURACIES[rng.below(ACCURACIES.len())],
    }
}

/// The station of a slot.
pub(super) fn station_at(p: &Project, at: StationRef) -> Option<StationId> {
    p.topology().station_at(at)
}

/// The slot datum `k` is addressed through: its own (`via`) slot, else any
/// slot of its station through which E1 resolves to `k` itself.
pub(super) fn datum_address(p: &Project, k: usize) -> Option<StationRef> {
    let datum = p.datums.get(k)?;
    let topology = p.topology();
    let own = p.datum_slot(datum);
    own.into_iter()
        .chain(topology.slots_of(datum.station).iter().copied())
        .find(|slot| p.datum_index_for(datum.station, Some(slot.line_id)) == Some(k))
}

/// The slots of the stations in `station`'s shot component.
fn component_slots(p: &Project, station: StationId) -> Vec<StationRef> {
    let topology = p.topology();
    let Some(component) = topology.component_of(station) else {
        return Vec::new();
    };
    topology
        .components()
        .get(component)
        .map(|c| {
            c.stations
                .iter()
                .flat_map(|s| topology.slots_of(*s).iter().copied())
                .collect()
        })
        .unwrap_or_default()
}

/// The station a datum step lands on: station 0, a line through a datum's
/// station, the frame's component, or anywhere.
fn datum_station(rng: &mut Rng, p: &Project) -> Option<StationRef> {
    let all = every_station(p);
    let roll = rng.below(100);
    if roll < 25 {
        return rng.pick(
            &all.iter()
                .copied()
                .filter(|s| s.station_index == 0)
                .collect::<Vec<_>>(),
        );
    }
    if roll < 40 {
        let topology = p.topology();
        let held: BTreeSet<LineId> = p
            .datums
            .iter()
            .flat_map(|d| topology.lines_of(d.station))
            .collect();
        let on: Vec<_> = all
            .iter()
            .copied()
            .filter(|s| held.contains(&s.line_id))
            .collect();
        if !on.is_empty() {
            return rng.pick(&on);
        }
    }
    if roll < 60 {
        if let Some(frame) = p.frame_datum().and_then(|f| p.datums.get(f)) {
            let near = component_slots(p, frame.station);
            if !near.is_empty() {
                return rng.pick(&near);
            }
        }
    }
    rng.pick(&all)
}

/// Pick the next step from the project as it is.
pub(super) fn next_op(
    rng: &mut Rng,
    p: &Project,
    state: &AppState,
    can_undo: bool,
    can_redo: bool,
) -> Option<Op> {
    let all = every_station(p);
    let datums = p.datums.len();
    let acc = ACCURACIES[rng.below(ACCURACIES.len())];
    let roll = rng.below(100);
    // A new GPS on the frame datum moves every other placed piece onto its own
    // GPS against the new frame (#769): mostly re-set only its accuracy
    // instead, so chains keep the layout they built.
    let frame = p.frame_datum();
    let frame_station = frame.and_then(|f| p.datums.get(f)).map(|d| d.station);
    let spare_frame = |rng: &mut Rng, at: StationRef| -> Option<Op> {
        (station_at(p, at).is_some() && station_at(p, at) == frame_station && !rng.chance(15))
            .then(|| Op::Accuracy {
                k: frame.unwrap_or(0),
                acc,
            })
    };
    Some(match roll {
        0..=16 => {
            let at = datum_station(rng, p)?;
            match spare_frame(rng, at) {
                Some(op) => op,
                None => datum_op(rng, at),
            }
        }
        17..=20 if datums > 0 => {
            let at = datum_station(rng, p)?;
            if let Some(op) = spare_frame(rng, at) {
                op
            } else {
                let k = rng.below(datums);
                // Half the time the copy keeps the source's accuracy too:
                // twins nothing stored tells apart (#778).
                let acc = match p.datums.get(k) {
                    Some(d) if rng.chance(50) => d.accuracy_m,
                    _ => acc,
                };
                Op::Twin { at, k, acc }
            }
        }
        21..=28 if datums > 0 => Op::Accuracy {
            k: rng.below(datums),
            acc,
        },
        29..=36 if datums > 0 => {
            // The frame on purpose now and then.
            let k = match frame {
                Some(f) if rng.chance(40) => f,
                _ => rng.below(datums),
            };
            Op::Remove { k }
        }
        37..=46 => {
            // Two separately placed pieces on purpose now and then.
            let placed: Vec<StationId> = p.placement_datums().into_iter().flatten().filter_map(|d| p.datums.get(d).map(|d| d.station)).collect();
            let (a, b) = if placed.len() >= 2 && rng.chance(40) {
                let (x, y) = (rng.pick(&placed)?, rng.pick(&placed)?);
                (
                    rng.pick(&component_slots(p, x))?,
                    rng.pick(&component_slots(p, y))?,
                )
            } else {
                (rng.pick(&all)?, rng.pick(&all)?)
            };
            Op::Merge { a, b }
        }
        47..=52 => {
            let topology = p.topology();
            let junctions: Vec<&[StationRef]> = p
                .stations
                .keys()
                .map(|s| topology.slots_of(*s))
                .filter(|slots| slots.len() > 1)
                .collect();
            let slots = junctions.get(rng.below(junctions.len()))?;
            let a = slots[rng.below(slots.len())];
            let others: Vec<StationRef> = slots
                .iter()
                .copied()
                .filter(|s| s.line_id != a.line_id)
                .collect();
            Op::Unmerge {
                a,
                b: rng.pick(&others)?,
            }
        }
        53..=56 => Op::Closure {
            a: rng.pick(&all)?,
            b: rng.pick(&all)?,
        },
        57..=61 => {
            let long: Vec<_> = p.lines().iter().filter(|s| s.shots.len() >= 2).collect();
            let s = long.get(rng.below(long.len()))?;
            Op::Split {
                line: s.id,
                at: 1 + rng.below(s.shots.len() - 1) as u32,
                side_b: rng.chance(50),
            }
        }
        62..=66 => {
            let mut offered = Vec::new();
            for s in p.lines() {
                for end in [0, s.shots.len() as u32] {
                    let at = StationRef::new(s.id, end);
                    offered.extend(
                        state
                            .combine_candidates(at)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|c| c.blocked_reason.is_none()),
                    );
                }
            }
            let c = offered.get(rng.below(offered.len()))?;
            let keep_a = if c.blocked_keeping_a.is_some() {
                false
            } else if c.blocked_keeping_b.is_some() {
                true
            } else {
                rng.chance(50)
            };
            if keep_a {
                Op::Combine {
                    kept: c.line_a,
                    kept_end: c.end_a,
                    other: c.line_b,
                    other_end: c.end_b,
                }
            } else {
                Op::Combine {
                    kept: c.line_b,
                    kept_end: c.end_b,
                    other: c.line_a,
                    other_end: c.end_a,
                }
            }
        }
        67..=69 if p.lines().len() > 1 => Op::DeleteLine {
            line: p.lines()[rng.below(p.lines().len())].id,
        },
        70..=74 => {
            let with: Vec<_> = p.lines().iter().filter(|s| !s.shots.is_empty()).collect();
            let s = with.get(rng.below(with.len()))?;
            Op::DeleteShot {
                line: s.id,
                index: rng.below(s.shots.len()),
            }
        }
        75..=78 => {
            let s = &p.lines()[rng.below(p.lines().len())];
            let pos = match rng.below(3) {
                0 => None,
                1 if !s.shots.is_empty() => Some(-1),
                _ if !s.shots.is_empty() => Some(rng.below(s.shots.len()) as i64),
                _ => None,
            };
            let depth = rng.range(0.0, 30.0);
            let shot = random_shot(rng, depth);
            Op::AddShot {
                line: s.id,
                pos,
                azimuth: shot.azimuth_in,
                length: shot.length,
                depth_in: shot.depth_in,
                depth_out: shot.depth_out,
            }
        }
        79..=80 => Op::MoveLine {
            line: p.lines()[rng.below(p.lines().len())].id,
            dx: rng.range(-30.0, 30.0),
            dy: rng.range(-30.0, 30.0),
        },
        81..=88 if can_undo => Op::Undo,
        89..=94 if can_redo => Op::Redo,
        95..=99 => Op::Reopen,
        _ => return None,
    })
}

/// What a forward step did.
pub(super) enum Applied {
    /// The command refused; the project must be unchanged.
    Refused(String),
    /// Nothing was run (the step names nothing the survey has).
    Skipped(String),
    Done,
}

/// GPS `(dx, dy)` from where the frame puts `at` now (from `origin` when
/// there is no frame).
fn gps_near(
    origin: &GpsCoordinates,
    p: &Project,
    at: StationRef,
    dx: f64,
    dy: f64,
) -> Option<GpsCoordinates> {
    let (frame, x, y) = match p.frame_datum().and_then(|f| p.datums.get(f)) {
        Some(d) => {
            let [x, y, _] = p.raw_positions().slot(p, at)?;
            (d.gps, x, y)
        }
        None => (*origin, 0.0, 0.0),
    };
    let (lat, lon) = local_to_gps(x + dx, y + dy, &frame)?;
    GpsCoordinates::new(lat, lon, 0.0).ok()
}

/// Run a forward step through the command path.
pub(super) fn apply(state: &AppState, origin: &GpsCoordinates, op: &Op, p: &Project) -> Applied {
    let set = |at: StationRef, gps: GpsCoordinates, acc: Option<f64>| {
        state.execute(Command::SetZeroDatum {
            station_ref: at,
            coordinates: gps,
            accuracy_m: acc,
            entrance: None,
        })
    };
    let result = match op {
        Op::Datum { at, dx, dy, acc } => match gps_near(origin, p, *at, *dx, *dy) {
            Some(gps) => set(*at, gps, *acc),
            None => return Applied::Skipped("no GPS".to_string()),
        },
        Op::Twin { at, k, acc } => match p.datums.get(*k) {
            Some(d) => set(*at, d.gps, *acc),
            None => return Applied::Skipped("no datum".to_string()),
        },
        Op::Accuracy { k, acc } => match (datum_address(p, *k), p.datums.get(*k)) {
            (Some(at), Some(d)) => set(at, d.gps, *acc),
            _ => return Applied::Skipped("datum not addressable".to_string()),
        },
        Op::Remove { k } => match datum_address(p, *k) {
            Some(at) => state.execute(Command::RemoveZeroDatum { station_ref: at }),
            None => return Applied::Skipped("datum not addressable".to_string()),
        },
        Op::Merge { a, b } => state.execute(Command::MergeStations {
            station_a: *a,
            station_b: *b,
        }),
        Op::Unmerge { a, b } => state.execute(Command::UnmergeStationPair {
            station_a: *a,
            station_b: *b,
        }),
        Op::Closure { a, b } => state.execute(Command::CreateLoopClosure {
            station_a: *a,
            station_b: *b,
            closure_strategy: ClosureStrategy::Batch,
        }),
        Op::Split { line, at, .. } => state.execute_split_line(
            *line,
            SplitAt::Station { station_index: *at },
            SplitLineOptions {
                name_a: format!("S{line}a"),
                name_b: format!("S{line}b"),
                color_a: "#123456".to_string(),
                color_b: "#654321".to_string(),
                comment_side: SplitSide::A,
                datum_side: SplitSide::A,
            },
        ),
        Op::Combine {
            kept,
            kept_end,
            other,
            other_end,
        } => state.execute_combine_lines(
            *kept,
            *other,
            CombineLinesOptions {
                name: format!("C{kept}"),
                color: "#abcdef".to_string(),
                swap_lr: true,
                comment_pick: CombinePick::Kept,
                name_pick: CombinePick::Kept,
                join: Some(CombineJoinEnds {
                    kept: *kept_end,
                    other: *other_end,
                }),
            },
        ),
        Op::DeleteLine { line } => state.execute(Command::DeleteLine { line_id: *line }),
        Op::DeleteShot { line, index } => {
            match p.line(*line).and_then(|s| s.shots.get(*index)) {
                Some(shot) => state.execute(Command::DeleteShot {
                    line_id: *line,
                    shot_id: shot.id,
                }),
                None => return Applied::Skipped("no shot".to_string()),
            }
        }
        Op::AddShot {
            line,
            pos,
            azimuth,
            length,
            depth_in,
            depth_out,
        } => {
            let ids: Vec<_> = p
                .line(*line)
                .map(|s| s.shots.iter().map(|x| x.id).collect())
                .unwrap_or_default();
            let (after, before) = match pos {
                Some(-1) => (None, ids.first().copied()),
                Some(k) => (ids.get(*k as usize).copied(), None),
                None => (None, None),
            };
            state.execute(Command::CreateShot {
                line_id: *line,
                shot: ShotData {
                    azimuth_in: *azimuth,
                    azimuth_out: *azimuth,
                    length: *length,
                    depth_in: *depth_in,
                    depth_out: *depth_out,
                    ..Default::default()
                },
                after_shot_id: after,
                before_shot_id: before,
            })
        }
        Op::MoveLine { line, dx, dy } => state.execute(Command::MoveLine {
            line_id: *line,
            offset: Coordinates::new(*dx, *dy, 0.0),
        }),
        Op::Undo | Op::Redo | Op::Reopen => {
            return Applied::Skipped("not a forward step".to_string())
        }
    };
    if result.success {
        Applied::Done
    } else {
        Applied::Refused(result.error.unwrap_or_default())
    }
}
