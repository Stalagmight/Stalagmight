//! A run of shot updates against the same updates one at a time through
//! [`Project::update_shot`] (D2): the same refusal at the same update, and
//! otherwise the same stored model and raw layout, to the bit, after the
//! frozen-shot check the command makes before each geometry update.

use std::collections::BTreeMap;

use crate::{LineId, ShotKind, ShotUpdates, StationId};

use super::super::test_helpers::{add, flat, gps};
use super::super::{ClosureStrategy, Placement, Project, StartAt, ZeroDatum};
use super::edit_fuzz_tests::random_survey;
use super::edit_test_helpers::empty;
use super::{EditError, ShotEditRun};

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
}

/// The command's frozen check on the whole project (`any_shot_frozen`).
fn frozen(p: &Project, line: LineId, index: usize) -> bool {
    if p.datums.is_empty() {
        return false;
    }
    let aligned = p.closure_alignment(&p.raw_positions());
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    p.frozen_shots(&aligned)
        .iter()
        .any(|f| f.line_id == line && f.shot_index == index)
}

const FROZEN: &str = "frozen";

/// One update as the command makes it, one at a time.
fn one_by_one(
    p: &mut Project,
    edits: &[(LineId, crate::ShotId, ShotUpdates)],
) -> Result<(), (usize, String)> {
    for (k, (line, shot, updates)) in edits.iter().enumerate() {
        if updates.touches_geometry() {
            let index = p
                .line(*line)
                .and_then(|l| l.shots.iter().position(|s| s.id == *shot));
            if index.is_some_and(|i| frozen(p, *line, i)) {
                return Err((k, FROZEN.to_string()));
            }
        }
        p.update_shot(*line, *shot, updates)
            .map_err(|e| (k, e.to_string()))?;
    }
    Ok(())
}

/// The same updates as one run.
fn as_a_run(
    p: &mut Project,
    edits: &[(LineId, crate::ShotId, ShotUpdates)],
) -> Result<(), (usize, String)> {
    let mut run = ShotEditRun::new(p);
    for (k, (line, shot, updates)) in edits.iter().enumerate() {
        if updates.touches_geometry() {
            let index = run
                .project()
                .line(*line)
                .and_then(|l| l.shots.iter().position(|s| s.id == *shot));
            if index.is_some_and(|i| run.is_frozen(*line, i)) {
                return Err((k, FROZEN.to_string()));
            }
        }
        run.update_shot(*line, *shot, updates)
            .map_err(|e| (k, e.to_string()))?;
    }
    run.finish().map_err(|e| (edits.len(), e.to_string()))
}

/// Every stored number and every raw position, as bits.
fn bits(p: &Project) -> (String, Vec<(StationId, [u64; 3])>) {
    let text = serde_json::to_string(p).unwrap();
    let positions = p
        .raw_positions()
        .iter()
        .map(|(s, [x, y, z])| (s, [x.to_bits(), y.to_bits(), z.to_bits()]))
        .collect();
    (text, positions)
}

/// A random update of a random shot: geometry, kind, annotation, or one
/// that is refused.
fn random_edit(p: &Project, rng: &mut Lcg) -> Option<(LineId, crate::ShotId, ShotUpdates)> {
    let lines: Vec<LineId> = p
        .lines()
        .iter()
        .filter(|l| !l.shots.is_empty())
        .map(|l| l.id)
        .collect();
    if lines.is_empty() {
        return None;
    }
    let line = lines[rng.below(lines.len())];
    let shots = &p.line(line)?.shots;
    let shot = shots[rng.below(shots.len())].id;
    let updates = match rng.below(12) {
        0..=3 => ShotUpdates {
            length: Some(0.5 + rng.below(40) as f64 * 0.37),
            ..ShotUpdates::default()
        },
        4 | 5 => ShotUpdates {
            azimuth_in: Some(rng.below(3600) as f64 / 10.0),
            azimuth_out: Some(rng.below(3600) as f64 / 10.0),
            ..ShotUpdates::default()
        },
        6 => ShotUpdates {
            depth_out: Some(rng.below(50) as f64 * 0.3),
            pitch_in: Some(Some(rng.below(60) as f64 - 30.0)),
            ..ShotUpdates::default()
        },
        7 => ShotUpdates {
            kind: Some(if rng.below(2) == 0 {
                ShotKind::Jump
            } else {
                ShotKind::Normal
            }),
            ..ShotUpdates::default()
        },
        8 | 9 => ShotUpdates {
            left: Some(Some(rng.below(30) as f64 * 0.1)),
            comment: Some(Some(format!("c{}", rng.below(100)))),
            ..ShotUpdates::default()
        },
        10 => ShotUpdates {
            length: Some(if rng.below(2) == 0 { 0.0 } else { -2.0 }),
            ..ShotUpdates::default()
        },
        _ => ShotUpdates {
            length: Some(2.0 + rng.below(9) as f64),
            kind: Some(ShotKind::Reference),
            ..ShotUpdates::default()
        },
    };
    Some((line, shot, updates))
}

/// Hand moves on some stations (as a migrated legacy survey carries), so the
/// re-walk's arithmetic is exercised with them.
fn with_hand_moves(p: &mut Project, rng: &mut Lcg) {
    let topology = p.topology_arc();
    for line in p.lines_mut() {
        let Some(chain) = topology.chain(line.id) else {
            continue;
        };
        for &station in chain {
            if rng.below(9) == 0 {
                let h = [
                    rng.below(100) as f64 * 0.013 - 0.6,
                    rng.below(100) as f64 * 0.011 - 0.5,
                    0.0,
                ];
                line.hand_moves.insert(station, h);
            }
        }
    }
}

#[test]
fn a_run_is_its_updates_one_at_a_time_to_the_bit() {
    let mut runs: BTreeMap<&str, usize> = BTreeMap::new();
    for seed in 0..120 {
        let mut p = random_survey(seed, 60);
        let mut rng = Lcg(seed ^ 0x5eed_2026);
        if seed % 3 == 0 {
            with_hand_moves(&mut p, &mut rng);
        }
        assert!(p.validate().is_empty(), "seed {seed}: the survey is valid");
        let count = 1 + rng.below(40);
        let edits: Vec<_> = (0..count)
            .filter_map(|_| random_edit(&p, &mut rng))
            .collect();
        let (mut a, mut b) = (p.clone(), p.clone());
        let one = one_by_one(&mut a, &edits);
        let run = as_a_run(&mut b, &edits);
        assert_eq!(run, one, "seed {seed}: the same refusal at the same update");
        match one {
            Ok(()) => {
                assert_eq!(
                    bits(&b),
                    bits(&a),
                    "seed {seed}: the same model, to the bit"
                );
                *runs.entry("applied").or_default() += 1;
            }
            Err((_, message)) if message == FROZEN => *runs.entry("frozen").or_default() += 1,
            Err(_) => *runs.entry("refused").or_default() += 1,
        }
    }
    assert!(runs.get("applied").copied().unwrap_or(0) >= 30, "{runs:?}");
    assert!(runs.get("refused").copied().unwrap_or(0) >= 10, "{runs:?}");
}

/// A free piece of three lines: each update holds its own line's first
/// station where the previous update left it, so the placement moves at
/// every step, and a run must land on the same bits.
#[test]
fn a_run_on_a_free_piece_moves_its_placement_as_each_update_does() {
    let mut p = empty();
    let root = add(&mut p, StartAt::New, vec![flat(10.0, 3.0); 6]);
    let mid = p.line(root).unwrap().shots[2].to;
    let branch = add(&mut p, StartAt::Station(mid), vec![flat(100.0, 2.5); 5]);
    let far = p.line(branch).unwrap().shots[3].to;
    let twig = add(&mut p, StartAt::Station(far), vec![flat(200.0, 1.75); 4]);
    let start = p.line(root).unwrap().start;
    p.placements.push(Placement {
        station: start,
        x: 5.25,
        y: -3.5,
    });
    assert!(p.validate().is_empty());
    let mut edits = Vec::new();
    for (k, line) in [root, branch, twig, root, twig, branch]
        .into_iter()
        .enumerate()
    {
        let shot = p.line(line).unwrap().shots[k % 3].id;
        edits.push((
            line,
            shot,
            ShotUpdates {
                length: Some(1.0 + k as f64 * 0.731),
                azimuth_in: Some(17.0 * k as f64),
                ..ShotUpdates::default()
            },
        ));
    }
    let (mut a, mut b) = (p.clone(), p.clone());
    one_by_one(&mut a, &edits).unwrap();
    as_a_run(&mut b, &edits).unwrap();
    assert_ne!(a.placements, p.placements, "the placement moved");
    assert_eq!(bits(&b), bits(&a));
}

/// An update that pulls a snap's two ends together aligns it, which can
/// freeze a later update's shot: the run sees the new alignment exactly as
/// the one-at-a-time route does. The snap's end hangs off the edited line by
/// a third line, so the run must carry the update downstream to see it.
#[test]
fn a_run_sees_a_snap_its_own_update_aligned_downstream() {
    let mut p = empty();
    // a: east along y = 0 from the datum; a4 at (40, 0).
    let a = add(&mut p, StartAt::New, vec![flat(90.0, 10.0); 4]);
    let a0 = p.line(a).unwrap().start;
    p.push_datum(ZeroDatum::new(a0, gps(20.5, -87.25)));
    let a2 = p.line(a).unwrap().shots[1].to;
    let a4 = p.line(a).unwrap().shots[3].to;
    // b from a2: north, then east to (34.5, 5); c from there ends at
    // (39.5, 0), half a metre short of a4, snapped to it.
    let b = add(
        &mut p,
        StartAt::Station(a2),
        vec![flat(0.0, 5.0), flat(90.0, 14.5)],
    );
    let b_end = p.line(b).unwrap().shots[1].to;
    let c = add(
        &mut p,
        StartAt::Station(b_end),
        vec![flat(90.0, 5.0), flat(180.0, 5.0)],
    );
    let c_end = p.line(c).unwrap().shots[1].to;
    p.push_closure(c_end, Some(c), a4, Some(a), ClosureStrategy::Move);
    assert!(p.validate().is_empty());
    let ids = |p: &Project, line: LineId| -> Vec<crate::ShotId> {
        p.line(line).unwrap().shots.iter().map(|s| s.id).collect()
    };
    let (sa, sb) = (ids(&p, a), ids(&p, b));
    let edits = vec![
        // Moves c with it: the snap aligns and closes a datum-held loop.
        (
            b,
            sb[1],
            ShotUpdates {
                length: Some(15.0),
                ..ShotUpdates::default()
            },
        ),
        // Now inside that loop: frozen.
        (
            a,
            sa[2],
            ShotUpdates {
                length: Some(11.0),
                ..ShotUpdates::default()
            },
        ),
    ];
    assert!(!frozen(&p, a, 2), "not frozen before the run");
    let (mut x, mut y) = (p.clone(), p.clone());
    let one = one_by_one(&mut x, &edits);
    assert_eq!(
        one,
        Err((1, FROZEN.to_string())),
        "the second update is frozen"
    );
    assert_eq!(as_a_run(&mut y, &edits), one);
}

/// A refused update in the middle of a run reports what `update_shot`
/// reports, at the same update.
#[test]
fn a_run_refuses_as_update_shot_does() {
    let mut p = empty();
    let line = add(&mut p, StartAt::New, vec![flat(45.0, 4.0); 3]);
    let shots: Vec<_> = p.line(line).unwrap().shots.iter().map(|s| s.id).collect();
    for updates in [
        ShotUpdates {
            length: Some(0.0),
            ..ShotUpdates::default()
        },
        ShotUpdates {
            length: Some(-1.5),
            ..ShotUpdates::default()
        },
        ShotUpdates {
            azimuth_in: Some(f64::NAN),
            ..ShotUpdates::default()
        },
    ] {
        let edits = vec![
            (
                line,
                shots[0],
                ShotUpdates {
                    length: Some(7.0),
                    ..ShotUpdates::default()
                },
            ),
            (line, shots[1], updates),
        ];
        let (mut a, mut b) = (p.clone(), p.clone());
        let one = one_by_one(&mut a, &edits);
        assert!(matches!(one, Err((1, _))), "{one:?}");
        assert_eq!(as_a_run(&mut b, &edits), one);
    }
    let gone = ShotUpdates {
        length: Some(7.0),
        ..ShotUpdates::default()
    };
    let mut q = p.clone();
    let mut run = ShotEditRun::new(&mut q);
    assert_eq!(
        run.update_shot(line, 9_999, &gone),
        Err(EditError::refused(crate::SHOT_GONE))
    );
}
