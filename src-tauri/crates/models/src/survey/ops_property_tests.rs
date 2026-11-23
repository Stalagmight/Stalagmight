//! Seeded property test over random sequences of the M2 commands: after
//! every step the model is valid; a refused command leaves the project
//! exactly as it was; the commands that may not move anything do not; and
//! the same seed gives the same model, bit for bit.

use std::collections::BTreeMap;

use crate::{LineId, ShotData, StationId, StationRef};

use super::ops_test_helpers::{gps_at, xy_all};
use super::test_helpers::{meta, project, shot};
use super::{ClosureStrategy, OpError, OpOutcome, Project, StartAt};

/// splitmix64: small, seedable, the same everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        if items.is_empty() {
            None
        } else {
            Some(items[self.below(items.len())])
        }
    }
}

/// Shots with deterministic uuids `{prefix}-{n}`.
fn shots(rng: &mut Rng, count: usize, prefix: &str, next: &mut u64) -> Vec<ShotData> {
    (0..count)
        .map(|_| {
            let depth = rng.range(0.0, 30.0);
            let mut data = shot(
                rng.range(0.0, 360.0),
                rng.range(2.0, 25.0),
                depth,
                depth + rng.range(-1.0, 1.0),
            );
            data.uuid = Some(format!("{prefix}-{next}"));
            *next += 1;
            data
        })
        .collect()
}

/// A free line (its own piece) with a placement at a random position.
fn add_free_line(p: &mut Project, rng: &mut Rng, prefix: &str, next: &mut u64) -> Option<LineId> {
    let count = 1 + rng.below(4);
    let data = shots(rng, count, prefix, next);
    let line = p.add_line(meta("free"), StartAt::New, data).ok()?;
    if rng.below(4) == 0 {
        // A legacy hand move (only migrated files carry them).
        let chain = p.line_stations(line)?.to_vec();
        let at = rng.pick(&chain)?;
        let hand = [
            rng.range(-0.5, 0.5),
            rng.range(-0.5, 0.5),
            rng.range(-0.2, 0.2),
        ];
        p.line_mut(line)?.hand_moves.insert(at, hand);
    }
    let start = p.line(line)?.start;
    p.set_placement(start, rng.range(-500.0, 500.0), rng.range(-500.0, 500.0))
        .ok()?;
    Some(line)
}

/// A small valid source project for Add.
fn random_source(rng: &mut Rng, tag: usize) -> Project {
    let mut s = project();
    let prefix = format!("src{tag}");
    let mut next = 0;
    let lines = 1 + rng.below(3);
    for _ in 0..lines {
        add_free_line(&mut s, rng, &prefix, &mut next);
    }
    let stations = s.topology().stations().to_vec();
    if let (Some(from), true) = (rng.pick(&stations), rng.below(2) == 0) {
        let count = 1 + rng.below(2);
        let data = shots(rng, count, &prefix, &mut next);
        let _ = s.add_line(meta("branch"), StartAt::Station(from), data);
    }
    if rng.below(2) == 0 {
        let stations = s.topology().stations().to_vec();
        if let Some(station) = rng.pick(&stations) {
            let gps = gps_at(rng.range(-3000.0, 3000.0), rng.range(-3000.0, 3000.0));
            let _ = s.set_datum(station, None, gps, None);
        }
    }
    assert_eq!(s.validate(), Vec::new(), "random source must be valid");
    s
}

/// Every slot of the project.
fn slots(p: &Project) -> Vec<StationRef> {
    p.lines()
        .iter()
        .flat_map(|line| (0..line.station_count()).map(move |i| StationRef::new(line.id, i as u32)))
        .collect()
}

/// What a step must leave unchanged: everything (refusal), or positions.
enum Expect {
    Anything,
    NoMove,
}

/// Run one command, check the outcome, return a label for the log.
fn check<T>(
    p: &Project,
    before: &Project,
    expect: Expect,
    label: String,
    result: Result<OpOutcome<T>, OpError>,
) -> String {
    match result {
        Ok(_) => {
            if matches!(expect, Expect::NoMove) {
                let (b, a) = (xy_all(before), xy_all(p));
                for (station, xy) in &a {
                    if let Some(old) = b.get(station) {
                        assert_eq!(
                            (old[0].to_bits(), old[1].to_bits()),
                            (xy[0].to_bits(), xy[1].to_bits()),
                            "{label}: station {station} moved"
                        );
                    }
                }
            }
            format!("{label}: ok")
        }
        Err(error) => {
            assert_eq!(
                p, before,
                "{label}: a refusal ({error}) changed the project"
            );
            format!("{label}: refused ({error})")
        }
    }
}

/// The stations of `station`'s piece.
fn piece(p: &Project, station: StationId) -> Vec<StationId> {
    let topology = p.topology();
    topology
        .component_of(station)
        .and_then(|c| topology.component(c))
        .map(|c| c.stations.clone())
        .unwrap_or_default()
}

/// Every station of `before` still present moved by `shift(s)` (`None`: not
/// checked), within `tolerance`.
fn assert_moves(
    before: &BTreeMap<StationId, [f64; 2]>,
    after: &BTreeMap<StationId, [f64; 2]>,
    tolerance: f64,
    label: &str,
    shift: impl Fn(StationId) -> Option<[f64; 2]>,
) {
    for (station, old) in before {
        let (Some(now), Some(d)) = (after.get(station), shift(*station)) else {
            continue;
        };
        let gap = (now[0] - old[0] - d[0]).hypot(now[1] - old[1] - d[1]);
        assert!(
            gap <= tolerance,
            "{label}: station {station} is {gap} m off its expected move {d:?}"
        );
    }
}

fn step(p: &mut Project, rng: &mut Rng, next: &mut u64, tag: &mut usize) -> String {
    let before = p.clone();
    let stations: Vec<StationId> = p.topology().stations().to_vec();
    let all_slots = slots(p);
    let lines: Vec<LineId> = p.lines().iter().map(|l| l.id).collect();
    match rng.below(14) {
        0 => {
            add_free_line(p, rng, "s", next);
            "add free line".to_string()
        }
        1 => {
            let Some(from) = rng.pick(&stations) else {
                return "branch: none".to_string();
            };
            let count = 1 + rng.below(3);
            let data = shots(rng, count, "s", next);
            let _ = p.add_line(meta("branch"), StartAt::Station(from), data);
            format!("branch from {from}")
        }
        2 => {
            let (Some(a), Some(b)) = (rng.pick(&stations), rng.pick(&stations)) else {
                return "merge: none".to_string();
            };
            let (stationary, moving) = p.merge_direction(a, b);
            let (fixed, moved) = (piece(p, stationary), piece(p, moving));
            let xy_before = xy_all(p);
            let result = p.merge_keeping_stationary(a, b);
            if result.is_ok() {
                // The stationary piece stays; the moving one moves rigidly
                // so its station lands on the survivor.
                let d = [
                    xy_before[&stationary][0] - xy_before[&moving][0],
                    xy_before[&stationary][1] - xy_before[&moving][1],
                ];
                assert_moves(&xy_before, &xy_all(p), 1e-6, "merge", |s| {
                    if fixed.contains(&s) {
                        Some([0.0, 0.0])
                    } else if moved.contains(&s) {
                        Some(d)
                    } else {
                        None
                    }
                });
            }
            check(
                p,
                &before,
                Expect::Anything,
                format!("merge {a} {b}"),
                result,
            )
        }
        3 => {
            let junctions: Vec<StationRef> = all_slots
                .iter()
                .copied()
                .filter(|slot| {
                    p.station_at(*slot)
                        .is_some_and(|s| p.topology().is_junction(s))
                })
                .collect();
            let Some(slot) = rng.pick(&junctions) else {
                return "detach: none".to_string();
            };
            let xy_before = xy_all(p);
            let result = p.unmerge_station(slot, crate::survey::NameAnswers::RuleL, None);
            if let Ok(OpOutcome {
                value: Some(report),
                anchors,
                sweep,
                ..
            }) = &result
            {
                // Nothing moves unless a fix that ended up in a piece of its
                // own was promoted onto its GPS, or the sweep promoted a
                // closure.
                if anchors.placed_on_gps.is_empty() && sweep.is_empty() {
                    let after = xy_all(p);
                    assert_moves(&xy_before, &after, 1e-9, "detach", |_| Some([0.0, 0.0]));
                    let at = after[&report.new_station];
                    let was = xy_before[&report.station];
                    assert!((at[0] - was[0]).hypot(at[1] - was[1]) <= 1e-9);
                }
            }
            check(
                p,
                &before,
                Expect::Anything,
                format!("detach {slot:?}"),
                result,
            )
        }
        4 => {
            let Some(a) = rng.pick(&all_slots) else {
                return "closure: none".to_string();
            };
            // Half the time, the other end in the same piece (a loop).
            let topology = p.topology_arc();
            let piece = p.station_at(a).and_then(|s| topology.component_of(s));
            let same_piece: Vec<StationRef> = all_slots
                .iter()
                .copied()
                .filter(|slot| p.station_at(*slot).and_then(|s| topology.component_of(s)) == piece)
                .collect();
            let pool = if rng.below(2) == 0 {
                &same_piece
            } else {
                &all_slots
            };
            let Some(b) = rng.pick(pool) else {
                return "closure: none".to_string();
            };
            let strategy = [
                ClosureStrategy::Batch,
                ClosureStrategy::Move,
                ClosureStrategy::LeastSquaresWeighted,
            ][rng.below(3)];
            let result = p.create_closure_at(a, b, strategy, None, &[]);
            check(
                p,
                &before,
                Expect::NoMove,
                format!("closure {a:?} {b:?}"),
                result,
            )
        }
        5 => {
            let ids: Vec<_> = p.closures.iter().map(|c| c.id).collect();
            let id = rng.pick(&ids).unwrap_or(9999);
            let result = p.remove_closure(id, &[]);
            check(
                p,
                &before,
                Expect::NoMove,
                format!("remove closure {id}"),
                result,
            )
        }
        6 => {
            let Some(slot) = rng.pick(&all_slots) else {
                return "datum: none".to_string();
            };
            let gps = gps_at(rng.range(-3000.0, 3000.0), rng.range(-3000.0, 3000.0));
            let accuracy = [None, Some(rng.range(0.0, 50.0))][rng.below(2)];
            let xy_before = xy_all(p);
            let result = p.set_datum_at(slot, gps, accuracy);
            if let Ok(outcome) = &result {
                let set = outcome.value;
                let fix_added = set.change == super::DatumChange::Added
                    && set.role == Some(crate::DatumRole::GpsFix);
                if fix_added || set.change == super::DatumChange::AccuracyOnly {
                    assert_moves(&xy_before, &xy_all(p), 0.0, "fix", |_| Some([0.0, 0.0]));
                }
            }
            check(
                p,
                &before,
                Expect::Anything,
                format!("set datum {slot:?}"),
                result,
            )
        }
        7 => {
            // Only a datum its own slot reaches (two datums on one station
            // with one hint would make this a GPS edit of the other one).
            let reachable: Vec<usize> = (0..p.datums.len())
                .filter(|&i| p.datum_index_for(p.datums[i].station, p.datums[i].via) == Some(i))
                .collect();
            let Some(index) = rng.pick(&reachable) else {
                return "accuracy: none".to_string();
            };
            let d = p.datums[index];
            let result = p.set_datum(d.station, d.via, d.gps, Some(rng.range(0.0, 9.0)));
            check(
                p,
                &before,
                Expect::NoMove,
                format!("accuracy {index}"),
                result,
            )
        }
        8 => {
            let index = rng.below(p.datums.len() + 1);
            let result = p.remove_datum(index);
            check(
                p,
                &before,
                Expect::Anything,
                format!("remove datum {index}"),
                result,
            )
        }
        9 => {
            let Some(line) = rng.pick(&lines) else {
                return "segment: none".to_string();
            };
            let offset = [rng.range(-20.0, 20.0), rng.range(-20.0, 20.0), 0.0];
            let xy_before = xy_all(p);
            let result = p.move_segment(line, offset);
            if let Ok(outcome) = &result {
                let topology = p.topology_arc();
                let group: Vec<StationId> = outcome
                    .value
                    .lines
                    .iter()
                    .filter_map(|&l| topology.chain(l))
                    .flatten()
                    .copied()
                    .collect();
                assert_moves(&xy_before, &xy_all(p), 1e-9, "segment", |s| {
                    Some(if group.contains(&s) {
                        [offset[0], offset[1]]
                    } else {
                        [0.0, 0.0]
                    })
                });
            }
            check(
                p,
                &before,
                Expect::Anything,
                format!("segment {line}"),
                result,
            )
        }
        10 => {
            let Some(station) = rng.pick(&stations) else {
                return "move station: none".to_string();
            };
            let offset = [
                rng.range(-3.0, 3.0),
                rng.range(-3.0, 3.0),
                rng.range(-0.5, 0.5),
            ];
            let xy_before = xy_all(p);
            let result = p.move_station(station, offset);
            if result.is_ok() {
                assert_moves(&xy_before, &xy_all(p), 1e-6, "move station", |s| {
                    Some(if s == station {
                        [offset[0], offset[1]]
                    } else {
                        [0.0, 0.0]
                    })
                });
            }
            check(
                p,
                &before,
                Expect::Anything,
                format!("move {station}"),
                result,
            )
        }
        11 => {
            *tag += 1;
            let source = random_source(rng, *tag);
            let xy_before = xy_all(p);
            let result = p.append_project(&source);
            if result.is_ok() {
                assert_moves(&xy_before, &xy_all(p), 0.0, "append", |_| Some([0.0, 0.0]));
            }
            check(p, &before, Expect::Anything, "append".to_string(), result)
        }
        12 => {
            let Some(index) = rng.pick(&(0..p.datums.len()).collect::<Vec<_>>()) else {
                return "entrance: none".to_string();
            };
            let answer = [None, Some(true), Some(false)][rng.below(3)];
            let result = p.set_datum_entrance(index, answer);
            assert!(result.is_ok());
            assert_eq!(
                xy_all(p),
                xy_all(&before),
                "an entrance answer moved something"
            );
            format!("entrance {index}")
        }
        _ => {
            // Two slots of one junction (sometimes of two stations).
            let junctions: Vec<StationId> = stations
                .iter()
                .copied()
                .filter(|&s| p.topology().is_junction(s))
                .collect();
            let (a, b) = match rng.pick(&junctions) {
                Some(junction) if rng.below(4) != 0 => {
                    let own = p.slots_of(junction).to_vec();
                    (rng.pick(&own), rng.pick(&own))
                }
                _ => (rng.pick(&all_slots), rng.pick(&all_slots)),
            };
            let (Some(a), Some(b)) = (a, b) else {
                return "pair detach: none".to_string();
            };
            let result = p.unmerge_station_pair(a, b, crate::survey::NameAnswers::RuleL, None);
            check(
                p,
                &before,
                Expect::Anything,
                format!("pair detach {a:?} {b:?}"),
                result,
            )
        }
    }
}

/// Run `steps` random commands from `seed`; every step leaves a valid model.
fn run_sequence(seed: u64, steps: usize) -> (Project, Vec<String>) {
    let mut rng = Rng(seed);
    let mut p = project();
    let mut next = 0;
    let mut tag = 0;
    for _ in 0..2 {
        add_free_line(&mut p, &mut rng, "s", &mut next);
    }
    let mut log = Vec::new();
    for i in 0..steps {
        let label = step(&mut p, &mut rng, &mut next, &mut tag);
        let violations = p.validate();
        assert!(
            violations.is_empty(),
            "seed {seed}, step {i} ({label}): {violations:?}\nlog: {log:#?}"
        );
        log.push(label);
    }
    (p, log)
}

/// Every position and depth, as bits.
fn position_bits(p: &Project) -> BTreeMap<StationId, [u64; 3]> {
    p.raw_positions()
        .iter()
        .map(|(s, [x, y, z])| (s, [x.to_bits(), y.to_bits(), z.to_bits()]))
        .collect()
}

#[test]
fn random_command_sequences_keep_the_model_valid() {
    let mut accepted = 0;
    let mut total = 0;
    for seed in 0..48 {
        let (_, log) = run_sequence(seed, 70);
        total += log.len();
        accepted += log.iter().filter(|l| l.ends_with(": ok")).count();
    }
    assert!(
        accepted * 5 > total,
        "too few commands succeeded ({accepted} of {total}): the test is not exercising them"
    );
}

#[test]
fn the_same_seed_gives_the_same_model_bit_for_bit() {
    for seed in [3, 17, 29, 41] {
        let (a, log_a) = run_sequence(seed, 70);
        let (b, log_b) = run_sequence(seed, 70);
        assert_eq!(log_a, log_b, "seed {seed}");
        assert_eq!(a, b, "seed {seed}");
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            "seed {seed}"
        );
        assert_eq!(position_bits(&a), position_bits(&b), "seed {seed}");
    }
}
