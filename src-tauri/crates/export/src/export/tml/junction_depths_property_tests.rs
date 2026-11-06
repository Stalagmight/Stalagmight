//! [#576 TML-DEPTH-D1] The junction rule as a property over random shot trees:
//! continuations, branch starts whose reading disagrees with the station they
//! hang off (or agrees with it), a section's second piece at the same station
//! (a section merged part-way along), explicit second readings, zero-length
//! ties, second entrances and closures.
//!
//! For every tree, in both profiles:
//! - every station's exported depth is its true depth, except the junctions
//!   whose readings disagree, and each of those is the mean of its readings,
//!   one per section;
//! - the receipt counts exactly those junctions and their widest spread;
//! - `|Depth - parent.Depth| <= Length` holds on the values as written;
//! - a shot keeps its tape unless it touches a moved station, and then its
//!   length is `sqrt(h^2 + dz'^2)` for its measured run `h`, rounded up by at
//!   most the written micrometre;
//! - plan positions, read back the way our importer reads the file, are
//!   unchanged. The bound allows only what that micrometre round-up can move a
//!   run by: `sqrt(h^2 + 2 L' e) - h` for a round-up `e`, which is about a
//!   micrometre for a shallow shot and grows towards `sqrt(2 L' e)` (about
//!   1.4 mm on a 1 m tie) as the shot goes vertical - the file's six decimals
//!   cannot hold a steeper shot's plan any closer.

use std::collections::HashMap;

use super::{
    as_written, enforce_ariane_geometry_invariants, round_as_written, schema, ExportShot,
    TmlProfile,
};
use crate::models::SectionId;

/// splitmix64: deterministic and dependency-free.
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

    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

fn row(id: i64, from_id: i64, shot_type: &'static str, depth: f64) -> ExportShot {
    ExportShot::synthesized(
        id,
        from_id,
        shot_type,
        "S".to_string(),
        "0xff0000ff".to_string(),
        (depth, depth),
    )
}

/// A random survey tree, every row after its parent.
fn tree(rng: &mut Rng) -> Vec<ExportShot> {
    let n = 2 + rng.below(40);
    let mut shots = vec![row(0, -1, schema::TYPE_START, rng.range(0.0, 30.0))];
    for id in 1..n as i64 {
        if rng.chance(0.05) {
            // A second entrance.
            shots.push(row(id, -1, schema::TYPE_START, rng.range(0.0, 30.0)));
            continue;
        }
        let parent = &shots[rng.below(shots.len())];
        let (mut from, parent_depth) = (parent.id, parent.depth);
        let offset = if rng.chance(0.25) {
            0.0
        } else {
            rng.range(-3.0, 3.0)
        };
        // (start, sentinel, branch start, reversed leg)
        let (mut start, sentinel, branch, mut reversed) = match rng.below(10) {
            0..=3 => (parent_depth, true, false, false),
            4..=7 => (parent_depth + offset, true, true, false),
            // A second reading inside one section: an explicit DepthIn.
            8 => (parent_depth + rng.range(-0.5, 0.5), false, false, false),
            // A reversed leg starting a branch with an explicit DepthIn.
            _ => (parent_depth + offset, false, true, true),
        };
        // The section a branch start belongs to: a new one, or - a section
        // merged part-way along - the second piece of one already hung at that
        // station, mostly its reversed block, starting from the same reading
        // or (a second reading inside that section) another.
        let mut section = id as SectionId;
        if branch && rng.chance(0.3) {
            let pieces: Vec<(i64, SectionId, f64)> = shots
                .iter()
                .filter_map(|s| s.branch_of.map(|of| (s.from_id, of, s.depth_in)))
                .collect();
            if !pieces.is_empty() {
                let (at, of, reading) = pieces[rng.below(pieces.len())];
                (from, section) = (at, of);
                reversed = rng.chance(0.7);
                let station_depth = shots.iter().find(|s| s.id == at).map_or(0.0, |s| s.depth);
                start = if rng.chance(0.5) {
                    reading
                } else {
                    station_depth + rng.range(-3.0, 3.0)
                };
            }
        }
        let (length, change) = if rng.chance(0.1) {
            (0.0, 0.0) // a zero-length tie
        } else {
            let length = rng.range(0.5, 20.0);
            (length, rng.range(-0.9, 0.9) * length)
        };
        let mut shot = row(id, from, schema::TYPE_REAL, start + change);
        shot.depth_in = start;
        shot.length = length;
        shot.azimuth = rng.range(0.0, 360.0);
        shot.depth_in_same_as_prev = sentinel;
        shot.branch_of = branch.then_some(section);
        shot.reversed = reversed;
        shots.push(shot);
    }
    for _ in 0..rng.below(3) {
        let a = rng.below(shots.len());
        let b = rng.below(shots.len());
        if a == b {
            continue;
        }
        let id = shots.len() as i64;
        let (depth_a, depth_b) = (shots[a].depth, shots[b].depth);
        let mut closure = row(id, shots[a].id, schema::TYPE_CLOSURE, depth_b);
        closure.depth_in = depth_a;
        closure.length = (depth_b - depth_a).abs();
        closure.closure_to_id = shots[b].id;
        shots.push(closure);
    }
    shots
}

fn run(length: f64, change: f64) -> f64 {
    (length * length - change * change).max(0.0).sqrt()
}

/// What the rule says each station is exported at, worked out from the input
/// alone: `(depth, averaged)` per row, plus the receipt.
fn expected(shots: &[ExportShot]) -> (Vec<(f64, bool)>, usize, f64) {
    let mut out: Vec<(f64, bool)> = shots.iter().map(|s| (s.depth, false)).collect();
    let (mut count, mut widest) = (0usize, 0.0f64);
    for (p, station) in shots.iter().enumerate() {
        if station.shot_type == schema::TYPE_CLOSURE {
            continue;
        }
        // One reading per section hung there: its first reversed piece's
        // start (the depth it arrives with), else its first piece's.
        let mut readings = vec![station.depth];
        let mut sections: Vec<SectionId> = Vec::new();
        let pieces: Vec<&ExportShot> = shots
            .iter()
            .filter(|k| k.from_id == station.id && k.branch_of.is_some())
            .collect();
        for k in &pieces {
            let Some(of) = k.branch_of.filter(|of| !sections.contains(of)) else {
                continue;
            };
            sections.push(of);
            let arriving = pieces
                .iter()
                .find(|r| r.branch_of == Some(of) && r.reversed);
            readings.push(arriving.unwrap_or(k).depth_in);
        }
        let high = readings.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let low = readings.iter().copied().fold(f64::INFINITY, f64::min);
        if high - low > 1e-9 {
            out[p] = (readings.iter().sum::<f64>() / readings.len() as f64, true);
            count += 1;
            widest = widest.max(high - low);
        }
    }
    (out, count, widest)
}

fn check(seed: u64, profile: TmlProfile) {
    // Built twice from the seed: the pass rewrites one copy, the oracle reads
    // the other.
    let input = tree(&mut Rng(seed));
    let input = input.as_slice();
    let mut shots = tree(&mut Rng(seed));
    let junctions = enforce_ariane_geometry_invariants(&mut shots, profile);
    let (want, count, widest) = expected(input);
    let index: HashMap<i64, usize> = input.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    let at = |i: usize| format!("seed {seed} {profile:?} row {}", input[i].id);

    assert_eq!(junctions.averaged, count, "seed {seed}: averaged junctions");
    assert!(
        (junctions.largest_spread - widest).abs() < 1e-12,
        "seed {seed}: spread"
    );

    // The bound as the server checks it: on the decimal strings. A length this
    // pass rewrote holds it in both profiles; the user profile's untouched
    // shots keep the sweep's own arithmetic (rounded depths against the raw
    // tape), which is what that profile has always written.
    let strict = |d: f64, pd: f64, l: f64| (as_written(d) - as_written(pd)).abs() <= as_written(l);
    let sweep = |d: f64, pd: f64, l: f64| {
        (round_as_written(d) - round_as_written(pd)).abs() <= round_as_written(l).max(l)
    };
    let mut plan_model: Vec<(f64, f64)> = vec![(0.0, 0.0); input.len()];
    let mut plan_file = plan_model.clone();
    let mut allowance = vec![1e-9; input.len()];
    for (i, (orig, got)) in input.iter().zip(&shots).enumerate() {
        let parent = index
            .get(&orig.from_id)
            .copied()
            .filter(|_| orig.from_id != -1);
        let closure = orig.shot_type == schema::TYPE_CLOSURE;
        let mut rewritten = false;
        if closure {
            let (Some(a), Some(&b)) = (parent, index.get(&orig.closure_to_id)) else {
                continue;
            };
            if want[a].1 || want[b].1 {
                rewritten = true;
                assert!(
                    (got.depth_in - want[a].0).abs() < 1e-12,
                    "{}: tie start",
                    at(i)
                );
                assert!((got.depth - want[b].0).abs() < 1e-12, "{}: tie end", at(i));
            }
        } else {
            assert!(
                (got.depth - want[i].0).abs() < 1e-12,
                "{}: exported at {}, expected {} (averaged: {})",
                at(i),
                got.depth,
                want[i].0,
                want[i].1
            );
        }
        let Some(p) = parent else {
            continue;
        };
        let measured_run = run(orig.length, orig.depth - orig.depth_in);
        let explicit_breaks =
            !orig.depth_in_same_as_prev && (orig.depth - input[p].depth).abs() > orig.length;
        let off_parent =
            orig.depth_in_same_as_prev && (orig.depth_in - input[p].depth).abs() >= 1e-9;
        let touched = !closure && (want[p].1 || want[i].1 || explicit_breaks || off_parent);
        let holds = if rewritten || touched || profile == TmlProfile::SpeleoDbSidecar {
            strict(got.depth, shots[p].depth, got.length)
        } else {
            sweep(got.depth, shots[p].depth, got.length)
        };
        assert!(
            holds,
            "{}: drops {} over a length of {} (as written; rewritten {})",
            at(i),
            as_written(got.depth) - as_written(shots[p].depth),
            as_written(got.length),
            rewritten || touched
        );
        if closure {
            continue;
        }
        let start = if got.depth_in_same_as_prev {
            shots[p].depth
        } else {
            got.depth_in
        };
        if touched {
            assert!(
                got.depth_in_same_as_prev,
                "{}: a refitted shot starts at its parent",
                at(i)
            );
            let exact = measured_run.hypot(got.depth - start);
            assert!(
                got.length >= exact - 1e-12 && got.length <= exact + 2e-6,
                "{}: length {} for a {measured_run} m run over {} (expected {exact})",
                at(i),
                got.length,
                got.depth - start
            );
        } else {
            assert!(
                got.length >= orig.length && got.length <= orig.length + 1e-6,
                "{}: untouched shot's tape {} became {}",
                at(i),
                orig.length,
                got.length
            );
        }

        let file_run = run(got.length, got.depth - start);
        let bearing = orig.azimuth.to_radians();
        plan_model[i] = (
            plan_model[p].0 + measured_run * bearing.sin(),
            plan_model[p].1 + measured_run * bearing.cos(),
        );
        plan_file[i] = (
            plan_file[p].0 + file_run * bearing.sin(),
            plan_file[p].1 + file_run * bearing.cos(),
        );
        let round_up = got.length - measured_run.hypot(got.depth - start);
        allowance[i] = allowance[p]
            + ((measured_run * measured_run + 2.0 * got.length * round_up.max(0.0)).sqrt()
                - measured_run)
            + 1e-9;
        let drift = (plan_file[i].0 - plan_model[i].0).hypot(plan_file[i].1 - plan_model[i].1);
        assert!(
            drift <= allowance[i],
            "{}: plan moved {drift} m (allowed {})",
            at(i),
            allowance[i]
        );
    }
}

#[test]
fn junctions_average_and_nothing_else_moves() {
    let (mut averaged, mut trees) = (0usize, 0u64);
    for seed in 0..500u64 {
        let input = tree(&mut Rng(seed));
        averaged += expected(&input).1;
        trees += 1;
        check(seed, TmlProfile::User);
        check(seed, TmlProfile::SpeleoDbSidecar);
    }
    // The generator has to keep producing the case under test.
    assert!(
        averaged > trees as usize,
        "only {averaged} averaged junctions in {trees} trees"
    );
}
