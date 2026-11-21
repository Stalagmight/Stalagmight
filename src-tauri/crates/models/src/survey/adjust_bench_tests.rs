//! A release-build benchmark of the Closures Computed View on loop-heavy
//! surveys (`docs/performance.md`, "Against the absolute-coordinate solve").
//! Nothing is asserted but validity (timings say nothing on a loaded host or
//! in a debug build). Ignored by default:
//!
//! ```text
//! cargo test --release -p stalagmight-models --lib computed_view_bench -- --ignored --nocapture
//! ```
//!
//! `STALAGMIGHT_PERF_RUNS` sets the runs per survey (default 15); each row is
//! `name<TAB>shots<TAB>median ms<TAB>min ms`. The "before" column of the doc
//! is this module run on a tree whose `lsq.rs` and `adjust.rs` are 13523a421's.
//! Fully synthetic data only.

use std::time::Instant;

use crate::config::ClosureAdjustmentConfig;
use crate::{LineId, ShotData, StationId};

use super::changeset::testkit::{gate_survey, gps};
use super::{ClosureStrategy, LineMeta, Project, StartAt, ZeroDatum};

/// splitmix64 in `[0, 1)`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    fn sym(&mut self) -> f64 {
        self.next() * 2.0 - 1.0
    }
}

/// A shot near `azimuth` and `length`: 1.5 degrees and 1 % of noise, so
/// every loop misses by a little.
fn shot(rng: &mut Rng, azimuth: f64, length: f64, tag: &str) -> ShotData {
    let a = (azimuth + rng.sym() * 1.5).rem_euclid(360.0);
    ShotData {
        uuid: Some(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, tag.as_bytes()).to_string()),
        azimuth_in: a,
        azimuth_out: a,
        length: length * (1.0 + rng.sym() * 0.01),
        depth_in: 10.0,
        depth_out: 10.0,
        ..ShotData::default()
    }
}

fn meta(name: String) -> LineMeta {
    LineMeta {
        name: Some(name),
        color: "#4ecdc4".to_string(),
        ..LineMeta::default()
    }
}

/// A `g` x `g` grid of junctions `spacing` metres apart joined by passages
/// of `l` shots; a passage into a junction that already exists ends on a
/// new station closed onto it by a soft closure (a loop per such passage).
fn maze(g: usize, l: usize, spacing: f64, seed: u64) -> Project {
    let mut rng = Rng(seed);
    let mut p = Project::new("Maze", None);
    let mut node: Vec<Option<(StationId, LineId)>> = vec![None; g * g];
    let mut first: Option<LineId> = None;
    let mut k = 0usize;
    for j in 0..g {
        for i in 0..g {
            for (open, (ti, tj), azimuth) in
                [(i + 1 < g, (i + 1, j), 90.0), (j + 1 < g, (i, j + 1), 0.0)]
            {
                if !open {
                    continue;
                }
                k += 1;
                let shots: Vec<ShotData> = (0..l)
                    .map(|s| {
                        shot(
                            &mut rng,
                            azimuth,
                            spacing / l as f64,
                            &format!("m{seed}-{k}-{s}"),
                        )
                    })
                    .collect();
                let src = node[j * g + i];
                let start = src.map_or(StartAt::New, |(s, _)| StartAt::Station(s));
                let id = p
                    .add_line(meta(format!("P{k}")), start, shots)
                    .expect("a passage");
                let stations = p.line_stations(id).expect("a line").to_vec();
                let end = *stations.last().expect("an end");
                if let Some((junction, via)) = node[tj * g + ti] {
                    p.push_closure(end, Some(id), junction, Some(via), ClosureStrategy::Batch);
                } else {
                    if src.is_none() {
                        node[j * g + i] = Some((stations[0], id));
                        first = Some(id);
                    }
                    node[tj * g + ti] = Some((end, id));
                }
            }
        }
    }
    let (entrance, _) = node[0].expect("the entrance");
    p.push_datum(ZeroDatum {
        via: first,
        ..ZeroDatum::new(entrance, gps(20.5, -87.25))
    });
    p
}

/// Every closure soft (the size gate's are snaps, which the solve skips).
fn soft(mut p: Project) -> Project {
    for c in &mut p.closures {
        c.strategy = ClosureStrategy::Batch;
    }
    p
}

/// The size gate's survey of `lines` lines, its closures soft, plus `k` soft
/// closures between random stations (at least 3 shots in) of two lines.
fn web(lines: usize, k: usize, seed: u64) -> Project {
    let gate = gate_survey(lines, None);
    let mut p = soft(gate.project);
    let mut rng = Rng(seed);
    let pick = |rng: &mut Rng| gate.lines[(rng.next() * gate.lines.len() as f64) as usize];
    let mut added = 0;
    while added < k {
        let (la, lb) = (pick(&mut rng), pick(&mut rng));
        if la == lb {
            continue;
        }
        let sa = p.line_stations(la).expect("a line").to_vec();
        let sb = p.line_stations(lb).expect("a line").to_vec();
        let a = sa[3 + (rng.next() * (sa.len() - 4) as f64) as usize];
        let b = sb[3 + (rng.next() * (sb.len() - 4) as f64) as usize];
        p.push_closure(a, Some(la), b, Some(lb), ClosureStrategy::Batch);
        added += 1;
    }
    p
}

/// Every 40th line's first shot 1000 times longer (a unit mistake on
/// import: tens of kilometres among metre shots).
fn unit_mistakes(mut p: Project) -> Project {
    let ids: Vec<LineId> = p.lines().iter().map(|l| l.id).collect();
    for id in ids.into_iter().skip(20).step_by(40) {
        if let Some(shot) = p.line_mut(id).and_then(|l| l.shots.first_mut()) {
            shot.length *= 1000.0;
        }
    }
    p
}

#[test]
#[ignore = "a benchmark: run in release with --ignored --nocapture"]
fn computed_view_bench() {
    let runs: usize = std::env::var("STALAGMIGHT_PERF_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);
    let config = ClosureAdjustmentConfig::default();
    let cases: Vec<(&str, Box<dyn Fn() -> Project>)> = vec![
        ("gate-5k", Box::new(|| gate_survey(100, None).project)),
        ("gate-20k", Box::new(|| gate_survey(400, None).project)),
        (
            "gate-5k-soft",
            Box::new(|| soft(gate_survey(100, None).project)),
        ),
        (
            "gate-20k-soft",
            Box::new(|| soft(gate_survey(400, None).project)),
        ),
        ("maze-10x10x28", Box::new(|| maze(10, 28, 60.0, 1))),
        ("maze-20x20x25", Box::new(|| maze(20, 25, 60.0, 2))),
        ("maze-40x40x6", Box::new(|| maze(40, 6, 25.0, 3))),
        ("maze-70x70x2", Box::new(|| maze(70, 2, 10.0, 6))),
        ("web-5k+800", Box::new(|| web(100, 800, 8))),
        ("web-20k+3000", Box::new(|| web(400, 3000, 7))),
        ("web-20k+10000", Box::new(|| web(400, 10_000, 9))),
        (
            "maze-40x40x6-unit",
            Box::new(|| unit_mistakes(maze(40, 6, 25.0, 3))),
        ),
        (
            "maze-70x70x2-unit",
            Box::new(|| unit_mistakes(maze(70, 2, 10.0, 6))),
        ),
        (
            "web-20k+3000-unit",
            Box::new(|| unit_mistakes(web(400, 3000, 7))),
        ),
    ];
    for (name, build) in &cases {
        let p = build();
        let violations = p.validate();
        assert!(violations.is_empty(), "{name}: {:?}", violations.first());
        let raw = p.raw_positions();
        assert!(p
            .adjusted_positions_from(&raw, &config)
            .failures()
            .is_empty());
        let mut times: Vec<f64> = (0..runs)
            .map(|_| {
                let started = Instant::now();
                std::hint::black_box(p.adjusted_positions_from(&raw, &config));
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        times.sort_by(f64::total_cmp);
        let shots: usize = p.lines().iter().map(|l| l.shots.len()).sum();
        println!(
            "{name}\t{shots}\t{:.2}\t{:.2}",
            times.get(times.len() / 2).copied().unwrap_or(f64::NAN),
            times.first().copied().unwrap_or(f64::NAN)
        );
    }
}
