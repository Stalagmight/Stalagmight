//! The steps of the datum command-chain fuzzer (`multi_datum_fuzz_tests`):
//! what a step is, how a trail spells and parses it, how a chain picks the
//! next one, and how it runs through the command path. Synthetic surveys
//! only.

use std::collections::HashSet;
use std::fmt;

use super::AppState;
use crate::core::Command;
use crate::models::{
    local_to_gps, ClosureStrategy, CombineJoinEnds, CombinePick, CombineSectionsOptions,
    GpsCoordinates, Project, SectionEnd, SectionId, ShotData, SplitAt, SplitSectionOptions,
    SplitSide, StationRef,
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
    /// station (from the chain's origin when there is no frame).
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
    Accuracy {
        k: usize,
        acc: Option<f64>,
    },
    /// Remove datum `k`.
    Remove {
        k: usize,
    },
    Merge {
        a: StationRef,
        b: StationRef,
    },
    Unmerge {
        a: StationRef,
        b: StationRef,
    },
    Closure {
        a: StationRef,
        b: StationRef,
    },
    /// Split `section` at station `at`; a datum there goes to half B when
    /// `side_b`.
    Split {
        section: SectionId,
        at: u32,
        side_b: bool,
    },
    Combine {
        kept: SectionId,
        kept_end: SectionEnd,
        other: SectionId,
        other_end: SectionEnd,
    },
    DeleteSection {
        section: SectionId,
    },
    DeleteShot {
        section: SectionId,
        index: usize,
    },
    /// Add a shot: `pos` -1 before the first shot, `k` after shot `k`, or
    /// `None` at the end.
    AddShot {
        section: SectionId,
        pos: Option<i64>,
        azimuth: f64,
        length: f64,
        depth_in: f64,
        depth_out: f64,
    },
    Undo,
    Redo,
    Reopen,
}

fn st(s: StationRef) -> String {
    format!("{}:{}", s.section_id, s.station_index)
}

fn acc_text(a: Option<f64>) -> String {
    a.map_or_else(|| "-".to_string(), |v| format!("{v:?}"))
}

fn end_text(e: SectionEnd) -> &'static str {
    match e {
        SectionEnd::Start => "start",
        SectionEnd::End => "end",
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
            Op::Split {
                section,
                at,
                side_b,
            } => write!(
                f,
                "split({section},{at},{})",
                if *side_b { "b" } else { "a" }
            ),
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
            Op::DeleteSection { section } => write!(f, "delsec({section})"),
            Op::DeleteShot { section, index } => write!(f, "delshot({section},{index})"),
            Op::AddShot {
                section,
                pos,
                azimuth,
                length,
                depth_in,
                depth_out,
            } => write!(
                f,
                "addshot({section},{},{azimuth:?},{length:?},{depth_in:?},{depth_out:?})",
                pos.map_or_else(|| "end".to_string(), |p| p.to_string())
            ),
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

fn parse_end(text: &str) -> (SectionId, SectionEnd) {
    let (s, e) = text.split_once(':').unwrap();
    let end = if e == "start" {
        SectionEnd::Start
    } else {
        SectionEnd::End
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
            section: a[0].parse().unwrap(),
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
        "delsec" => Op::DeleteSection {
            section: a[0].parse().unwrap(),
        },
        "delshot" => Op::DeleteShot {
            section: a[0].parse().unwrap(),
            index: int(1),
        },
        "addshot" => Op::AddShot {
            section: a[0].parse().unwrap(),
            pos: (a[1] != "end").then(|| a[1].parse().unwrap()),
            azimuth: num(2),
            length: num(3),
            depth_in: num(4),
            depth_out: num(5),
        },
        "undo" => Op::Undo,
        "redo" => Op::Redo,
        "reopen" => Op::Reopen,
        other => panic!("unknown step {other:?}"),
    }
}

/// Every station of `p`, section by section.
pub(super) fn every_station(p: &Project) -> Vec<StationRef> {
    p.sections
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

/// The station a datum step lands on: station 0, a section that already
/// holds a datum, the frame's merge component, or anywhere.
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
        let held: HashSet<SectionId> = p.all_datum_refs().iter().map(|s| s.section_id).collect();
        let on: Vec<_> = all
            .iter()
            .copied()
            .filter(|s| held.contains(&s.section_id))
            .collect();
        if !on.is_empty() {
            return rng.pick(&on);
        }
    }
    if roll < 60 {
        if let Some(frame) = p
            .frame_datum()
            .and_then(|zd| p.resolve_key(&zd.station_ref))
        {
            let group = p.get_merged_section_ids(frame.section_id);
            let near: Vec<_> = all
                .iter()
                .copied()
                .filter(|s| group.contains(&s.section_id))
                .collect();
            return rng.pick(&near);
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
    let datums = p.zero_datums.len();
    let acc = ACCURACIES[rng.below(ACCURACIES.len())];
    let roll = rng.below(100);
    // A new GPS on the frame datum re-places every other placed piece
    // (`frame-gps-edit`, #769): mostly re-set only its accuracy instead, so
    // chains keep the layout they built.
    let frame = p
        .zero_datums
        .iter()
        .position(|zd| p.resolve_key(&zd.station_ref).is_some());
    let frame_at = frame.and_then(|f| p.resolve_key(&p.zero_datums[f].station_ref));
    let spare_frame = |rng: &mut Rng, at: StationRef| -> Option<Op> {
        (Some(at) == frame_at && !rng.chance(15)).then(|| Op::Accuracy {
            k: frame.unwrap_or(0),
            acc,
        })
    };
    Some(match roll {
        0..=17 => {
            let at = datum_station(rng, p)?;
            match spare_frame(rng, at) {
                Some(op) => op,
                None => datum_op(rng, at),
            }
        }
        18..=21 if datums > 0 => {
            let at = datum_station(rng, p)?;
            if let Some(op) = spare_frame(rng, at) {
                op
            } else {
                let k = rng.below(datums);
                // Half the time the copy keeps the source's accuracy too:
                // twins nothing stored tells apart (#778).
                let acc = match p.zero_datums.get(k) {
                    Some(zd) if rng.chance(50) => zd.accuracy_m,
                    _ => acc,
                };
                Op::Twin { at, k, acc }
            }
        }
        22..=29 if datums > 0 => Op::Accuracy {
            k: rng.below(datums),
            acc,
        },
        30..=37 if datums > 0 => {
            // The frame on purpose now and then.
            let k = match frame {
                Some(f) if rng.chance(40) => f,
                _ => rng.below(datums),
            };
            Op::Remove { k }
        }
        38..=47 => {
            // Two separately placed pieces on purpose now and then.
            let placed = p.placement_datum_refs();
            let (a, b) = if placed.len() >= 2 && rng.chance(40) {
                let (x, y) = (rng.pick(&placed)?, rng.pick(&placed)?);
                let ga = p.get_merged_section_ids(x.section_id);
                let gb = p.get_merged_section_ids(y.section_id);
                let in_a: Vec<_> = all
                    .iter()
                    .copied()
                    .filter(|s| ga.contains(&s.section_id))
                    .collect();
                let in_b: Vec<_> = all
                    .iter()
                    .copied()
                    .filter(|s| gb.contains(&s.section_id))
                    .collect();
                (rng.pick(&in_a)?, rng.pick(&in_b)?)
            } else {
                (rng.pick(&all)?, rng.pick(&all)?)
            };
            Op::Merge { a, b }
        }
        48..=53 if !p.station_merges.is_empty() => {
            let m = &p.station_merges[rng.below(p.station_merges.len())];
            Op::Unmerge {
                a: p.resolve_key(&m.station_a)?,
                b: p.resolve_key(&m.station_b)?,
            }
        }
        54..=57 => Op::Closure {
            a: rng.pick(&all)?,
            b: rng.pick(&all)?,
        },
        58..=62 => {
            let long: Vec<_> = p.sections.iter().filter(|s| s.shots.len() >= 2).collect();
            let s = long.get(rng.below(long.len()))?;
            Op::Split {
                section: s.id,
                at: 1 + rng.below(s.shots.len() - 1) as u32,
                side_b: rng.chance(50),
            }
        }
        63..=67 => {
            let mut offered = Vec::new();
            for s in &p.sections {
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
                    kept: c.section_a,
                    kept_end: c.end_a,
                    other: c.section_b,
                    other_end: c.end_b,
                }
            } else {
                Op::Combine {
                    kept: c.section_b,
                    kept_end: c.end_b,
                    other: c.section_a,
                    other_end: c.end_a,
                }
            }
        }
        68..=70 if p.sections.len() > 1 => Op::DeleteSection {
            section: p.sections[rng.below(p.sections.len())].id,
        },
        71..=75 => {
            let with: Vec<_> = p.sections.iter().filter(|s| !s.shots.is_empty()).collect();
            let s = with.get(rng.below(with.len()))?;
            Op::DeleteShot {
                section: s.id,
                index: rng.below(s.shots.len()),
            }
        }
        76..=80 => {
            let s = &p.sections[rng.below(p.sections.len())];
            let pos = match rng.below(3) {
                0 => None,
                1 if !s.shots.is_empty() => Some(-1),
                _ if !s.shots.is_empty() => Some(rng.below(s.shots.len()) as i64),
                _ => None,
            };
            let depth = rng.range(0.0, 30.0);
            let shot = random_shot(rng, depth);
            Op::AddShot {
                section: s.id,
                pos,
                azimuth: shot.azimuth_in,
                length: shot.length,
                depth_in: shot.depth_in,
                depth_out: shot.depth_out,
            }
        }
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
    let (frame, x, y) = match p.frame_datum() {
        Some(zd) => {
            let c = p.compute_station_raw_coords(at)?;
            (zd.gps, c.x, c.y)
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
    let datum_at = |k: usize| {
        p.zero_datums
            .get(k)
            .and_then(|zd| Some((p.resolve_key(&zd.station_ref)?, zd.gps)))
    };
    let result = match op {
        Op::Datum { at, dx, dy, acc } => match gps_near(origin, p, *at, *dx, *dy) {
            Some(gps) => set(*at, gps, *acc),
            None => return Applied::Refused("no GPS".to_string()),
        },
        Op::Twin { at, k, acc } => match p.zero_datums.get(*k) {
            Some(zd) => set(*at, zd.gps, *acc),
            None => return Applied::Refused("no datum".to_string()),
        },
        Op::Accuracy { k, acc } => match datum_at(*k) {
            Some((at, gps)) => set(at, gps, *acc),
            None => return Applied::Refused("no datum".to_string()),
        },
        Op::Remove { k } => match datum_at(*k) {
            Some((at, _)) => state.execute(Command::RemoveZeroDatum { station_ref: at }),
            None => return Applied::Refused("no datum".to_string()),
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
        Op::Split {
            section,
            at,
            side_b,
        } => {
            let side = if *side_b { SplitSide::B } else { SplitSide::A };
            state.execute_split_section(
                *section,
                SplitAt::Station { station_index: *at },
                SplitSectionOptions {
                    name_a: format!("S{section}a"),
                    name_b: format!("S{section}b"),
                    color_a: "#123456".to_string(),
                    color_b: "#654321".to_string(),
                    comment_side: SplitSide::A,
                    datum_side: side,
                },
            )
        }
        Op::Combine {
            kept,
            kept_end,
            other,
            other_end,
        } => state.execute_combine_sections(
            *kept,
            *other,
            CombineSectionsOptions {
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
        Op::DeleteSection { section } => state.execute(Command::DeleteSection {
            section_id: *section,
        }),
        Op::DeleteShot { section, index } => {
            match p.get_section(*section).and_then(|s| s.shots.get(*index)) {
                Some(shot) => state.execute(Command::DeleteShot {
                    section_id: *section,
                    shot_id: shot.id,
                }),
                None => return Applied::Refused("no shot".to_string()),
            }
        }
        Op::AddShot {
            section,
            pos,
            azimuth,
            length,
            depth_in,
            depth_out,
        } => {
            let ids: Vec<_> = p
                .get_section(*section)
                .map(|s| s.shots.iter().map(|x| x.id).collect())
                .unwrap_or_default();
            let (after, before) = match pos {
                Some(-1) => (None, ids.first().copied()),
                Some(k) => (ids.get(*k as usize).copied(), None),
                None => (None, None),
            };
            state.execute(Command::CreateShot {
                section_id: *section,
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
        Op::Undo | Op::Redo | Op::Reopen => {
            return Applied::Refused("not a forward step".to_string())
        }
    };
    if result.success {
        Applied::Done
    } else {
        Applied::Refused(result.error.unwrap_or_default())
    }
}
