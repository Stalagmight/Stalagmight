//! The Closures Computed View solves the closure adjustment on a throwaway copy
//! of the project and keeps only the copy's station coordinates (#576 W38-NF-3).
//! Re-measuring the copy's closures on the adjusted geometry - one path search
//! per closure, on every recompute - fed nothing it returns, so that path skips
//! it. The live project's own `adjust_loop_closures` still re-measures.
//!
//! The oracle keeps the old view algorithm verbatim (each component through the
//! re-measuring `adjust_loop_closures`); the view must return the same station
//! coordinates, bit for bit.

use super::*;
use crate::project::closure::remeasure_probe;
use crate::{GpsCoordinates, LoopClosure, SectionData, ShotData, StationMerge, ZeroDatum};
use chrono::Utc;

/// A deterministic generator (the one `closure_determinism_tests` uses).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }

    fn real(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.below(1_000_000) as f64 / 1_000_000.0)
    }
}

fn shot(rng: &mut Lcg, azimuth: f64, length: f64) -> ShotData {
    let azimuth = azimuth + rng.real(-2.0, 2.0);
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: length + rng.real(-0.4, 0.4),
        depth_in: rng.real(5.0, 6.0),
        depth_out: rng.real(5.0, 6.0),
        ..Default::default()
    }
}

fn add_section(p: &mut Project, shots: Vec<ShotData>) -> SectionId {
    p.add_section(SectionData {
        shots,
        ..Default::default()
    })
    .id
}

fn merge(p: &mut Project, a: StationRef, b: StationRef) {
    p.station_merges.push(StationMerge {
        station_a: p.key_for_ref(a).unwrap(),
        station_b: p.key_for_ref(b).unwrap(),
        merged_at: Utc::now(),
    });
}

/// `components` unconnected ladders of `rungs` loops each. A ladder is a main
/// line of `rungs` shots east and, per main-line shot, a three-shot side section
/// merged at its start that goes north, east and south to come back near its
/// end, tied there by a loop closure (the misclosure is the shots' noise). The
/// first ladder hangs off a zero datum; the others take the default anchor, so a
/// recompute solves one component per ladder.
fn ladders(components: usize, rungs: usize, seed: u64) -> Project {
    let mut rng = Lcg(seed);
    let mut p = Project::new(format!("CaveA {seed}"), None);
    let mut pairs = Vec::new();
    for component in 0..components {
        let main_shots = (0..rungs).map(|_| shot(&mut rng, 90.0, 10.0)).collect();
        let main = add_section(&mut p, main_shots);
        for i in 0..rungs as u32 {
            let side_shots = vec![
                shot(&mut rng, 0.0, 5.0),
                shot(&mut rng, 90.0, 10.0),
                shot(&mut rng, 180.0, 5.0),
            ];
            let side = add_section(&mut p, side_shots);
            p.ensure_shot_uuids();
            merge(&mut p, StationRef::new(main, i), StationRef::new(side, 0));
            pairs.push((StationRef::new(side, 3), StationRef::new(main, i + 1)));
        }
        if component == 0 {
            p.zero_datums.push(ZeroDatum {
                station_ref: p.key_for_ref(StationRef::new(main, 0)).unwrap(),
                gps: GpsCoordinates::new(20.5, -87.4, 0.0).unwrap(),
            });
        }
    }
    p.redrive_offsets_after_load();
    for created in p.create_loop_closures_batch(&pairs) {
        created.unwrap();
    }
    p
}

/// The adjusted view as it was before #576 W38-NF-3, verbatim, with each
/// component solved by the public `adjust_loop_closures`, which also
/// re-measures the component's closures on the copy.
fn adjusted_view_with_remeasure(
    project: Project,
    config: &ClosureAdjustmentConfig,
) -> Vec<Station> {
    let anchors = project.plan_adjustment_anchors();
    let mut working = project;
    for anchor in anchors {
        let snapshot = working.clone();
        if working
            .adjust_loop_closures(anchor, ClosureStrategy::LeastSquaresWeighted, config)
            .is_err()
        {
            working = snapshot;
        }
    }
    working.compute_station_coordinates()
}

/// Every closure's stored values, as bits.
fn closure_bits(p: &Project) -> Vec<(u64, [u64; 5], bool)> {
    p.loop_closures
        .iter()
        .map(|c: &LoopClosure| {
            (
                c.id,
                [
                    c.path_length.to_bits(),
                    c.misclosure_distance.to_bits(),
                    c.error_percentage.to_bits(),
                    c.horizontal_misclosure.to_bits(),
                    c.vertical_misclosure.to_bits(),
                ],
                c.is_aligned,
            )
        })
        .collect()
}

/// Panic naming the first station whose coordinates differ by a single bit, or
/// whose relationship lists differ.
fn assert_same_stations(expected: &[Station], actual: &[Station], label: &str) {
    assert_eq!(expected.len(), actual.len(), "{label}: station count");
    for (e, a) in expected.iter().zip(actual) {
        assert_eq!(e.station_ref, a.station_ref, "{label}: station order");
        let bits = |s: &Station| {
            [
                s.coordinates.x.to_bits(),
                s.coordinates.y.to_bits(),
                s.coordinates.z.to_bits(),
            ]
        };
        assert_eq!(
            bits(e),
            bits(a),
            "{label}: {:?} at {:?} vs {:?}",
            e.station_ref,
            e.coordinates,
            a.coordinates
        );
        assert_eq!(
            serde_json::to_string(e).unwrap(),
            serde_json::to_string(a).unwrap(),
            "{label}: {:?}",
            e.station_ref
        );
    }
}

/// FNV-1a over every station's coordinate bits: one number to compare runs by.
fn coordinate_digest(stations: &[Station]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for s in stations {
        for v in [s.coordinates.x, s.coordinates.y, s.coordinates.z] {
            for byte in v.to_bits().to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    hash
}

#[test]
fn the_adjusted_view_does_not_remeasure_closures() {
    let project = ladders(2, 20, 1);
    let config = ClosureAdjustmentConfig::default();
    remeasure_probe::take();

    let _view = project.compute_adjusted_station_coordinates(&config);
    let in_view = remeasure_probe::take();
    println!(
        "closures: {}, re-measured by the adjusted view: {in_view} (expected 0)",
        project.loop_closures.len()
    );
    assert_eq!(in_view, 0);

    // The live project's own adjustment still re-measures its component's closures.
    let mut live = project;
    let anchor = live.plan_adjustment_anchors()[0];
    live.adjust_loop_closures(anchor, ClosureStrategy::LeastSquaresWeighted, &config)
        .unwrap();
    let in_live = remeasure_probe::take();
    println!("re-measured by adjust_loop_closures on one 20-closure component: {in_live}");
    assert_eq!(in_live, 20);
}

/// A GUARD, not the pin of the skip (#576 RV19a-15): the skip may change
/// nothing but the re-measure, so the view's coordinates must equal those of
/// the re-measuring solve to the bit. With the skip reverted both sides run
/// that solve and this passes by construction;
/// `the_adjusted_view_does_not_remeasure_closures` is the test that fails
/// then. This one fails if a change to the skip moves a station.
#[test]
fn adjusted_view_coordinates_are_bit_identical_to_the_remeasuring_solve() {
    let config = ClosureAdjustmentConfig::default();
    let mut shapes = vec![(1, 1, 0), (1, 12, 1), (2, 30, 2), (3, 8, 3)];
    let mut rng = Lcg(0x5eed);
    shapes
        .extend((10..40).map(|seed| (1 + rng.below(3) as usize, 1 + rng.below(15) as usize, seed)));
    for (components, rungs, seed) in shapes {
        let project = ladders(components, rungs, seed);
        let label = format!("{components} x {rungs} (seed {seed})");
        let expected = adjusted_view_with_remeasure(project.clone(), &config);

        // Precondition: the solve really moved the geometry.
        let measured = project.compute_station_coordinates();
        assert_ne!(
            coordinate_digest(&measured),
            coordinate_digest(&expected),
            "{label}: the adjustment should move stations"
        );

        assert_same_stations(
            &expected,
            &project.compute_adjusted_station_coordinates(&config),
            &label,
        );
        assert_same_stations(
            &expected,
            &project.into_adjusted_station_coordinates(&config),
            &label,
        );
    }
}

/// A GUARD, like the test above (#576 RV19a-15): the view solves a copy, so
/// the live project's closures keep their stored values whether or not the
/// copy re-measures - a revert of the skip passes it too. It fails if the view
/// ever writes through to the live closures.
#[test]
fn the_adjusted_view_leaves_the_live_closures_unchanged() {
    let project = ladders(2, 20, 4);
    let before = closure_bits(&project);
    let _view = project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
    assert_eq!(closure_bits(&project), before);
}

/// The saving, on a one-component ladder of many closures: the adjusted view
/// against the old re-measuring algorithm, median of several runs. Also prints
/// digests of the view's coordinates and of the closure values the live
/// project's adjustment stamps, to compare across builds.
#[test]
#[ignore = "timing: cargo test ... view_remeasure_tests::adjusted_view_timing -- --ignored --nocapture"]
fn adjusted_view_timing() {
    use std::time::Instant;
    let config = ClosureAdjustmentConfig::default();
    for rungs in [100, 400] {
        let project = ladders(1, rungs, 7);
        let median = |f: &dyn Fn() -> Vec<Station>| -> (f64, u64) {
            let mut times = Vec::new();
            let mut digest = 0;
            for _ in 0..5 {
                let start = Instant::now();
                let stations = f();
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                digest = coordinate_digest(&stations);
            }
            times.sort_by(f64::total_cmp);
            (times[times.len() / 2], digest)
        };
        let (view_ms, view_digest) =
            median(&|| project.compute_adjusted_station_coordinates(&config));
        let (old_ms, old_digest) =
            median(&|| adjusted_view_with_remeasure(project.clone(), &config));

        let mut live = project.clone();
        let anchor = live.plan_adjustment_anchors()[0];
        live.adjust_loop_closures(anchor, ClosureStrategy::LeastSquaresWeighted, &config)
            .unwrap();
        let mut live_digest: u64 = 0xcbf2_9ce4_8422_2325;
        for (id, bits, aligned) in closure_bits(&live) {
            for v in [
                id,
                bits[0],
                bits[1],
                bits[2],
                bits[3],
                bits[4],
                u64::from(aligned),
            ] {
                live_digest = (live_digest ^ v).wrapping_mul(0x0100_0000_01b3);
            }
        }
        println!(
            "{rungs} closures: adjusted view {view_ms:.1} ms (coords {view_digest:016x}); \
             re-measuring algorithm {old_ms:.1} ms (coords {old_digest:016x}); \
             live adjust_loop_closures closure values {live_digest:016x}"
        );
    }
}
