//! Seeded property test: random sequences of the line and shot operations
//! (create, insert, prepend, update, kind, delete, split at distance, range
//! delete in both modes, delete line, reverse, split line, combine), mixed
//! with closures, datums and merges. After every step the model validates
//! (an operation may refuse, never leave an invalid model: `EditError::Invalid`
//! is a failure), a refusal leaves the project untouched, and the same seed
//! gives the same bits (the model with shot uuids normalised, and every
//! position).

use crate::{
    CombineLinesOptions, CombinePick, LineId, ShotData, ShotKind, ShotUpdates, SplitAt, StationId,
};

use super::super::test_helpers::{gps, meta};
use super::super::{ClosureStrategy, Project, StartAt};
use super::edit_test_helpers::{add_datum, merge};
use super::{EditError, RangeDeleteMode, ShotPlace, SplitOptions};

/// A small deterministic generator (64-bit LCG, high bits).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn unit(&mut self) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
}

/// The state of one run: the project and a uuid counter.
struct Run {
    p: Project,
    rng: Lcg,
    uuids: u64,
    seed: u64,
}

impl Run {
    fn uuid(&mut self) -> String {
        self.uuids += 1;
        format!("f{}-{}", self.seed, self.uuids)
    }

    fn shot(&mut self) -> ShotData {
        let azimuth = (self.rng.below(36) * 10) as f64;
        let length = 1.0 + self.rng.below(20) as f64;
        let depth_in = self.rng.below(5) as f64;
        let depth_out = depth_in + self.rng.unit() * 0.5;
        ShotData {
            uuid: Some(self.uuid()),
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            length,
            depth_in,
            depth_out,
            ..ShotData::default()
        }
    }

    fn line(&mut self) -> Option<LineId> {
        let lines = self.p.lines();
        (!lines.is_empty()).then(|| lines[self.rng.below(lines.len())].id)
    }

    fn station(&mut self) -> Option<StationId> {
        let stations: Vec<StationId> = self.p.topology().stations().to_vec();
        (!stations.is_empty()).then(|| stations[self.rng.below(stations.len())])
    }

    fn shot_of(&mut self, line: LineId) -> Option<(usize, crate::ShotId)> {
        let shots = &self.p.line(line)?.shots;
        if shots.is_empty() {
            return None;
        }
        let i = self.rng.below(shots.len());
        Some((i, shots[i].id))
    }

    /// One random step; returns its name and whether it changed the
    /// project.
    fn step(&mut self, n: usize) -> (&'static str, bool) {
        let before = self.p.clone();
        let (name, result): (&'static str, Result<(), EditError>) = match self.rng.below(19) {
            0 | 1 => {
                let shots = (0..=self.rng.below(4)).map(|_| self.shot()).collect();
                let at = [self.rng.unit() * 100.0, self.rng.unit() * 100.0];
                let start = match self.station() {
                    Some(s) if self.rng.below(2) == 0 => StartAt::Station(s),
                    _ => StartAt::New,
                };
                let name = format!("L{n}");
                let r = self.p.create_line(meta(&name), start, shots, Some(at));
                ("create line", r.map(|_| ()))
            }
            2 | 3 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let data = self.shot();
                let place = match (self.rng.below(4), self.shot_of(line)) {
                    (0, Some((_, id))) => ShotPlace::After(id),
                    (1, Some((_, id))) => ShotPlace::Before(id),
                    (2, _) => ShotPlace::Start,
                    _ => ShotPlace::End,
                };
                (
                    "create shot",
                    self.p.create_shot(line, data, place).map(|_| ()),
                )
            }
            4 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let Some((_, id)) = self.shot_of(line) else {
                    return ("skip", false);
                };
                let updates = if self.rng.below(2) == 0 {
                    ShotUpdates {
                        length: Some(1.0 + self.rng.below(30) as f64),
                        azimuth_in: Some((self.rng.below(36) * 10) as f64),
                        ..ShotUpdates::default()
                    }
                } else {
                    ShotUpdates {
                        comment: Some(Some(format!("c{n}"))),
                        ..ShotUpdates::default()
                    }
                };
                (
                    "update shot",
                    self.p.update_shot(line, id, &updates).map(|_| ()),
                )
            }
            5 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let Some((_, id)) = self.shot_of(line) else {
                    return ("skip", false);
                };
                (
                    "set kind",
                    self.p.set_shot_kind(line, id, ShotKind::Jump).map(|_| ()),
                )
            }
            6 | 7 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let Some((_, id)) = self.shot_of(line) else {
                    return ("skip", false);
                };
                ("delete shot", self.p.delete_shot(line, id).map(|_| ()))
            }
            8 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let Some((i, id)) = self.shot_of(line) else {
                    return ("skip", false);
                };
                let length = self.p.line(line).map_or(0.0, |l| l.shots[i].length);
                let distance = length * (0.1 + 0.8 * self.rng.unit());
                let uuid = self.uuid();
                let r = self
                    .p
                    .split_shot_at_distance(line, id, distance, Some(uuid));
                ("split shot", r.map(|_| ()))
            }
            9 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let (Some((_, a)), Some((_, b))) = (self.shot_of(line), self.shot_of(line)) else {
                    return ("skip", false);
                };
                let mode = if self.rng.below(2) == 0 {
                    RangeDeleteMode::Shift
                } else {
                    RangeDeleteMode::Split {
                        name_a: format!("A{n}"),
                        name_b: format!("B{n}"),
                    }
                };
                let preview = self.p.preview_delete_shot_range(line, a, b);
                let r = self.p.delete_shot_range(line, a, b, &mode);
                if let (Ok(preview), Ok(_), RangeDeleteMode::Shift) = (&preview, &r, &mode) {
                    assert!(!preview.shift_blocked, "seed {} step {n}", self.seed);
                }
                ("delete range", r.map(|_| ()))
            }
            10 => {
                if self.p.lines().len() < 3 {
                    return ("skip", false);
                }
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                ("delete line", self.p.delete_line(line).map(|_| ()))
            }
            11 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let swap = self.rng.below(2) == 0;
                ("reverse", self.p.reverse_line(line, swap).map(|_| ()))
            }
            12 => {
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let n_shots = self.p.line(line).map_or(0, |l| l.shots.len());
                let at = if n_shots > 0 && self.rng.below(3) == 0 {
                    let (_, id) = self.shot_of(line).unwrap_or((0, 0));
                    SplitAt::Shot {
                        shot_id: id,
                        distance: 0.5,
                    }
                } else {
                    SplitAt::Station {
                        station_index: self.rng.below(n_shots + 1) as u32,
                    }
                };
                let options = SplitOptions {
                    name_a: format!("SA{n}"),
                    name_b: format!("SB{n}"),
                    color_a: "#000000".to_string(),
                    color_b: "#ffffff".to_string(),
                };
                (
                    "split line",
                    self.p.split_line(line, at, &options).map(|_| ()),
                )
            }
            13..=15 => {
                // From a line end (the station query), else any two lines.
                let Some(line) = self.line() else {
                    return ("skip", false);
                };
                let n_shots = self.p.line(line).map_or(0, |l| l.shots.len());
                let end = if self.rng.below(2) == 0 { 0 } else { n_shots };
                let slot = crate::StationRef::new(line, end as u32);
                let offered: Vec<_> = self
                    .p
                    .combine_candidates(slot)
                    .into_iter()
                    .filter(|c| c.blocked_reason.is_none())
                    .collect();
                let candidate = if offered.is_empty() {
                    let Some(other) = self.line() else {
                        return ("skip", false);
                    };
                    let Some(candidate) = self.p.combine_candidate_for_lines(line, other) else {
                        return ("skip", false);
                    };
                    candidate
                } else {
                    offered[self.rng.below(offered.len())].clone()
                };
                let keep_a = self.rng.below(2) == 0;
                let (kept, other) = if keep_a {
                    (candidate.line_a, candidate.line_b)
                } else {
                    (candidate.line_b, candidate.line_a)
                };
                let join = if keep_a {
                    crate::CombineJoinEnds {
                        kept: candidate.end_a,
                        other: candidate.end_b,
                    }
                } else {
                    crate::CombineJoinEnds {
                        kept: candidate.end_b,
                        other: candidate.end_a,
                    }
                };
                let options = CombineLinesOptions {
                    name: format!("C{n}"),
                    color: "#123456".to_string(),
                    swap_lr: self.rng.below(2) == 0,
                    comment_pick: CombinePick::Both,
                    name_pick: CombinePick::Kept,
                    join: Some(join),
                };
                let r = self.p.combine_lines(kept, other, &options);
                if let Ok(edited) = &r {
                    let plan = if keep_a {
                        &candidate.plan_keeping_a
                    } else {
                        &candidate.plan_keeping_b
                    };
                    let mut said = plan.clone();
                    let mut did = edited.value.clone();
                    // The preview ran without the dialog's name and picks.
                    said.station_renames = None;
                    did.station_renames = None;
                    assert_eq!(did, said, "seed {} step {n}: preview != combine", self.seed);
                }
                ("combine", r.map(|_| ()))
            }
            16 | 17 => {
                // A closure inside one piece (always valid).
                let (Some(a), Some(b)) = (self.station(), self.station()) else {
                    return ("skip", false);
                };
                if a == b || !self.p.topology().connected(a, b) {
                    return ("skip", false);
                }
                self.p
                    .push_closure(a, None, b, None, ClosureStrategy::Batch);
                ("closure", Ok(()))
            }
            _ => {
                // A datum, or a merge of two pieces (junctions for combine).
                let (Some(a), Some(b)) = (self.station(), self.station()) else {
                    return ("skip", false);
                };
                if self.rng.below(3) == 0 {
                    if self.p.datums.len() < 3 {
                        let lat = 10.0 + self.rng.unit() * 0.001;
                        add_datum(&mut self.p, a, gps(lat, 20.0));
                    }
                    ("datum", Ok(()))
                } else {
                    let topology = self.p.topology();
                    if topology.connected(a, b) {
                        return ("skip", false);
                    }
                    let held = self.p.placement_datums();
                    let (ca, cb) = (topology.component_of(a), topology.component_of(b));
                    let both_held = matches!((ca, cb), (Some(x), Some(y)) if held[x].is_some() && held[y].is_some());
                    if both_held {
                        return ("skip", false);
                    }
                    let a_held = ca.is_some_and(|x| held[x].is_some());
                    let (moving, stationary) = if a_held { (b, a) } else { (a, b) };
                    merge(&mut self.p, moving, stationary);
                    ("merge", Ok(()))
                }
            }
        };
        let done = result.is_ok();
        match result {
            Ok(()) => {}
            Err(EditError::Refused(_)) => {
                assert_eq!(
                    self.p, before,
                    "seed {} step {n}: a refused {name} changed the project",
                    self.seed
                );
            }
            Err(EditError::Invalid(violations)) => {
                assert!(
                    violations.is_empty(),
                    "seed {} step {n}: {name} left an invalid model: {violations:?}",
                    self.seed
                );
            }
        }
        let violations = self.p.validate();
        assert!(
            violations.is_empty(),
            "seed {} step {n}: invalid after {name}: {violations:?}",
            self.seed
        );
        (name, done)
    }
}

/// Run `steps` random steps from `seed`; returns the final project and the
/// operations that went through.
fn run(seed: u64, steps: usize) -> (Project, Vec<&'static str>) {
    let mut state = Run {
        p: super::edit_test_helpers::empty(),
        rng: Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5),
        uuids: 0,
        seed,
    };
    let mut names = Vec::new();
    for n in 0..steps {
        let (name, done) = state.step(n);
        if done {
            names.push(name);
        }
    }
    (state.p, names)
}

/// A random valid survey: the project `steps` random operations from `seed`
/// leave (closures, datums, merges and free pieces among them).
pub(super) fn random_survey(seed: u64, steps: usize) -> Project {
    run(seed, steps).0
}

/// The model with every shot uuid replaced by its position (uuids minted by
/// a split inside a shot are random v4s).
fn normalised(p: &Project) -> Project {
    let mut q = p.clone();
    for line in q.lines_mut() {
        let id = line.id;
        for (i, shot) in line.shots.iter_mut().enumerate() {
            shot.uuid = format!("{id}-{i}");
        }
    }
    q
}

fn position_bits(p: &Project) -> Vec<(StationId, [u64; 3])> {
    p.raw_positions()
        .iter()
        .map(|(s, [x, y, z])| (s, [x.to_bits(), y.to_bits(), z.to_bits()]))
        .collect()
}

#[test]
fn random_operation_sequences_keep_the_model_valid_and_are_deterministic() {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for seed in 0..32 {
        let (a, names) = run(seed, 80);
        let (b, _) = run(seed, 80);
        assert_eq!(
            normalised(&a),
            normalised(&b),
            "seed {seed}: same seed, same model"
        );
        assert_eq!(
            position_bits(&a),
            position_bits(&b),
            "seed {seed}: same bits"
        );
        for name in names {
            *counts.entry(name).or_default() += 1;
        }
    }
    // Every operation went through somewhere.
    for op in [
        "create line",
        "create shot",
        "update shot",
        "delete shot",
        "split shot",
        "delete range",
        "reverse",
        "split line",
        "combine",
    ] {
        assert!(
            counts.get(op).copied().unwrap_or(0) >= 10,
            "{op} rarely went through: {counts:?}"
        );
    }
}
