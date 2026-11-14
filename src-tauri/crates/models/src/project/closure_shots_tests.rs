//! Loop closures across one shot (#737): the shared helper, the creation-time
//! refusal (single and batch paths; the ONLY refusal it adds - a redundant tie
//! is still accepted), the solve skipping them, and the adjustment failures
//! the view now reports instead of only logging.

use crate::config::ClosureAdjustmentConfig;
use crate::{
    closure_across_one_shot_refusal, ClosureAcrossOneShot, ClosureStrategy, GpsCoordinates,
    LoopClosure, Project, SectionData, SectionId, ShotData, Station, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, shots: Vec<ShotData>) -> SectionId {
    project
        .add_section(SectionData {
            shots,
            ..Default::default()
        })
        .id
}

fn st(section: SectionId, index: u32) -> StationRef {
    StationRef::new(section, index)
}

/// Plant a closure directly, bypassing `create_loop_closure`'s refusal - the
/// shape a file written before #737 (or another tool) can carry.
fn plant(project: &mut Project, a: StationRef, b: StationRef, strategy: ClosureStrategy) -> u64 {
    let (station_a, station_b) = (
        project.key_for_ref(a).unwrap(),
        project.key_for_ref(b).unwrap(),
    );
    project.normalize_next_closure_id();
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a,
        station_b,
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned: false,
    });
    id
}

fn coords(stations: &[Station], at: StationRef) -> (f64, f64) {
    let s = stations.iter().find(|s| s.station_ref == at).unwrap();
    (s.coordinates.x, s.coordinates.y)
}

fn gap(stations: &[Station], a: StationRef, b: StationRef) -> f64 {
    let (pa, pb) = (coords(stations, a), coords(stations, b));
    (pa.0 - pb.0).hypot(pa.1 - pb.1)
}

/// The issue's survey: A two shots east, B north/east/south, merged at their
/// starts, with the far ends B:3 and A:2 0.5 m apart.
fn issue_loop() -> (Project, SectionId, SectionId) {
    let mut p = Project::new("737".to_string(), None);
    let a = add(&mut p, vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let b = add(
        &mut p,
        vec![shot(0.0, 10.0), shot(90.0, 20.5), shot(180.0, 10.0)],
    );
    p.merge_stations(st(a, 0), st(b, 0)).unwrap();
    p.create_loop_closure(st(b, 3), st(a, 2), ClosureStrategy::Batch)
        .unwrap();
    (p, a, b)
}

const SPANS_A1_A2: &str = "This closure would join the two ends of one shot (0:1 to 0:2) and squeeze that shot to zero length. Pick two stations that are not the ends of a single shot.";

// ---- the helper -------------------------------------------------------------

#[test]
fn a_closure_directly_across_one_shot_is_reported_with_its_shot() {
    let (mut p, a, _) = issue_loop();
    let id = plant(&mut p, st(a, 1), st(a, 2), ClosureStrategy::Batch);
    assert_eq!(
        p.closures_across_one_shot(),
        vec![ClosureAcrossOneShot {
            closure_id: id,
            station_a: st(a, 1),
            station_b: st(a, 2),
            collapsed_shot: (a, 1),
        }]
    );
    let only = &p.closures_across_one_shot()[0];
    assert_eq!(only.shot_ends(), (st(a, 1), st(a, 2)));
}

#[test]
fn a_closure_between_branches_merged_to_both_ends_of_a_shot_is_reported() {
    let (mut p, a, _) = issue_loop();
    // Two side passages starting at A:1 and A:2; a closure between their
    // starts ties A:1 and A:2 through the merges.
    let c = add(&mut p, vec![shot(0.0, 5.0)]);
    let d = add(&mut p, vec![shot(0.0, 5.0)]);
    p.merge_stations(st(c, 0), st(a, 1)).unwrap();
    p.merge_stations(st(d, 0), st(a, 2)).unwrap();
    let id = plant(&mut p, st(c, 0), st(d, 0), ClosureStrategy::Batch);
    let found = p.closures_across_one_shot();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].closure_id, id);
    assert_eq!(found[0].collapsed_shot, (a, 1));
    // And creation refuses the same pair, naming the shot, not the branches.
    let err = p
        .create_loop_closure(st(c, 0), st(d, 0), ClosureStrategy::Batch)
        .unwrap_err();
    // The planted closure already makes this pair a duplicate; drop it and ask again.
    assert_eq!(err, "A loop closure already exists between these stations.");
    p.loop_closures.retain(|lc| lc.id != id);
    let err = p
        .create_loop_closure(st(c, 0), st(d, 0), ClosureStrategy::Batch)
        .unwrap_err();
    assert_eq!(err, SPANS_A1_A2);
}

#[test]
fn chained_closures_report_the_one_that_closes_the_shot_in_vector_order() {
    // c1 ties A:1 to a far station X; c2 then ties X to A:2, which closes the
    // shot A:1-A:2 through c1. Only the LATER closure is skipped, and the order
    // decides it: swap them and the other one is.
    let build = |first_to_a2: bool| {
        let (mut p, a, _) = issue_loop();
        let far = add(&mut p, vec![shot(180.0, 30.0), shot(90.0, 10.0)]);
        p.merge_stations(st(far, 0), st(a, 0)).unwrap();
        let x = st(far, 2);
        let (first, second) = if first_to_a2 {
            (
                plant(&mut p, x, st(a, 2), ClosureStrategy::Batch),
                plant(&mut p, st(a, 1), x, ClosureStrategy::LeastSquaresWeighted),
            )
        } else {
            (
                plant(&mut p, st(a, 1), x, ClosureStrategy::LeastSquaresWeighted),
                plant(&mut p, x, st(a, 2), ClosureStrategy::Batch),
            )
        };
        (p, a, first, second)
    };
    for first_to_a2 in [false, true] {
        let (p, a, first, second) = build(first_to_a2);
        let found = p.closures_across_one_shot();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].closure_id, second,
            "the later closure is the skipped one"
        );
        assert_ne!(found[0].closure_id, first);
        assert_eq!(found[0].collapsed_shot, (a, 1));
        // Deterministic: asking twice gives the same answer.
        assert_eq!(found, p.closures_across_one_shot());
    }
}

#[test]
fn a_move_closure_does_not_tie_stations_for_the_solve() {
    // A Move closure is not solved, so it puts nothing in a knot: the Batch
    // closure from its far end to A:2 closes a real loop, not a shot.
    let (mut p, a, _) = issue_loop();
    let far = add(&mut p, vec![shot(180.0, 30.0)]);
    p.merge_stations(st(far, 0), st(a, 0)).unwrap();
    plant(&mut p, st(a, 1), st(far, 1), ClosureStrategy::Move);
    plant(&mut p, st(far, 1), st(a, 2), ClosureStrategy::Batch);
    assert!(p.closures_across_one_shot().is_empty());
}

#[test]
fn a_closure_across_one_shot_is_refused_even_when_another_route_joins_its_stations() {
    // A side passage C runs from A:1 back to A:2: merged at A:1, closed to A:2.
    // A:1 and A:2 are then joined by two routes (the shot, and C), yet a
    // closure A:1-A:2 would still put both ends of the shot A:1-A:2 at one
    // point - so it is refused, and planted it is skipped, with the shot named.
    let (mut p, a, _) = issue_loop();
    let c = add(
        &mut p,
        vec![shot(0.0, 5.0), shot(90.0, 10.0), shot(180.0, 5.0)],
    );
    p.merge_stations(st(c, 0), st(a, 1)).unwrap();
    p.create_loop_closure(st(c, 3), st(a, 2), ClosureStrategy::Batch)
        .unwrap();
    assert!(p.closures_across_one_shot().is_empty());
    let err = p
        .create_loop_closure(st(a, 1), st(a, 2), ClosureStrategy::Batch)
        .unwrap_err();
    assert_eq!(err, SPANS_A1_A2);
    plant(&mut p, st(a, 1), st(a, 2), ClosureStrategy::Batch);
    assert_eq!(p.closures_across_one_shot()[0].collapsed_shot, (a, 1));
}

#[test]
fn a_closure_across_two_shots_is_a_real_loop() {
    let (mut p, a, _) = issue_loop();
    assert!(p.closure_spans_one_shot(st(a, 0), st(a, 2)).is_none());
    p.create_loop_closure(st(a, 0), st(a, 2), ClosureStrategy::Batch)
        .unwrap();
    assert!(p.closures_across_one_shot().is_empty());
}

#[test]
fn a_closure_that_collapses_two_shots_reports_the_lowest() {
    // A:1 merged to B:1 and a real loop closure A:2-B:2 make two knots,
    // {A:1, B:1} and {A:2, B:2}. A closure joining them collapses BOTH shots
    // A:1-A:2 and B:1-B:2; the lowest (section, shot) is the one reported.
    let mut p = Project::new("two".to_string(), None);
    let a = add(&mut p, vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let b = add(&mut p, vec![shot(0.0, 10.0), shot(0.0, 10.0)]);
    p.merge_stations(st(a, 1), st(b, 1)).unwrap();
    p.create_loop_closure(st(a, 2), st(b, 2), ClosureStrategy::Batch)
        .unwrap();
    assert_eq!(
        p.closure_spans_one_shot(st(b, 1), st(a, 2)),
        Some((a.min(b), 1))
    );
    assert_eq!(
        p.closure_spans_one_shot(st(a, 1), st(b, 2)),
        Some((a.min(b), 1))
    );
    plant(&mut p, st(b, 1), st(a, 2), ClosureStrategy::Batch);
    assert_eq!(
        p.closures_across_one_shot()[0].collapsed_shot,
        (a.min(b), 1)
    );
}

#[test]
fn dangling_and_ineligible_closures_are_ignored() {
    let (mut p, a, _) = issue_loop();
    let id = plant(
        &mut p,
        st(a, 1),
        st(a, 2),
        ClosureStrategy::ProportionalByLength,
    );
    assert!(p.closures_across_one_shot().is_empty());
    // Dangle it: its anchoring shot is gone, so it resolves to nothing.
    let lc = p.loop_closures.iter_mut().find(|lc| lc.id == id).unwrap();
    lc.strategy = ClosureStrategy::Batch;
    lc.station_b.anchor = crate::StationAnchor::AfterShot("gone".to_string());
    assert!(p.closures_across_one_shot().is_empty());
}

// ---- refusal at creation -----------------------------------------------------

#[test]
fn creation_refuses_a_closure_across_one_shot_for_every_strategy() {
    let (mut p, a, _) = issue_loop();
    for strategy in [
        ClosureStrategy::Batch,
        ClosureStrategy::LeastSquaresWeighted,
        ClosureStrategy::Move,
    ] {
        let err = p
            .create_loop_closure(st(a, 1), st(a, 2), strategy)
            .unwrap_err();
        assert_eq!(err, SPANS_A1_A2, "{strategy:?}");
        // Either order of the two stations.
        let err = p
            .create_loop_closure(st(a, 2), st(a, 1), strategy)
            .unwrap_err();
        assert_eq!(err, SPANS_A1_A2, "{strategy:?}");
    }
    assert_eq!(p.loop_closures.len(), 1, "nothing was created");
    assert_eq!(p.closure_spans_one_shot(st(a, 1), st(a, 2)), Some((a, 1)));
    assert_eq!(closure_across_one_shot_refusal((a, 1)), SPANS_A1_A2);
}

/// Positions of every station, keyed for comparison.
fn positions(stations: &[Station]) -> Vec<(StationRef, (f64, f64))> {
    let mut v: Vec<_> = stations
        .iter()
        .map(|s| (s.station_ref, (s.coordinates.x, s.coordinates.y)))
        .collect();
    v.sort_by_key(|(r, _)| (r.section_id, r.station_index));
    v
}

fn assert_same_positions(a: &[Station], b: &[Station]) {
    let (a, b) = (positions(a), positions(b));
    assert_eq!(a.len(), b.len());
    for ((ra, pa), (rb, pb)) in a.iter().zip(&b) {
        assert_eq!(ra, rb);
        assert!(
            (pa.0 - pb.0).abs() < 1e-9 && (pa.1 - pb.1).abs() < 1e-9,
            "{ra:?}: {pa:?} vs {pb:?}"
        );
    }
}

#[test]
fn a_redundant_tie_is_still_created_and_the_solve_handles_it() {
    // A:2 and B:3 are one point through the closure; a third station merged
    // onto B:3 is then tied to A:2 too. A closure C:0-A:2 adds nothing - and
    // is accepted exactly as before #737, for every strategy, because it is
    // harmless to the solve (the union is a no-op) and refusing it would drop
    // such closures from imported files. Only a tie across ONE shot is refused.
    let build = || {
        let (mut p, a, b) = issue_loop();
        let c = add(&mut p, vec![shot(0.0, 5.0)]);
        p.merge_stations(st(c, 0), st(b, 3)).unwrap();
        (p, a, b, c)
    };
    let config = ClosureAdjustmentConfig::default();
    for strategy in [
        ClosureStrategy::Batch,
        ClosureStrategy::LeastSquaresWeighted,
        ClosureStrategy::Move,
    ] {
        let (mut p, a, b, c) = build();
        let reference = p.compute_adjusted_stations(&config);
        assert!(p.closure_spans_one_shot(st(c, 0), st(a, 2)).is_none());
        let created = p.create_loop_closure(st(c, 0), st(a, 2), strategy);
        assert!(
            created.is_ok(),
            "{strategy:?} redundant tie refused: {created:?}"
        );
        let created = created.unwrap();
        assert_eq!(p.loop_closures.len(), 2);
        // Not reported, not skipped, not a warning.
        assert!(p.closures_across_one_shot().is_empty());
        // The solve handles it: no failure, the loop stays closed, and the
        // geometry is exactly what it was without the redundant closure.
        let report = p.compute_adjusted_stations(&config);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(gap(&report.stations, st(b, 3), st(a, 2)) < 1e-6);
        assert_same_positions(&report.stations, &reference.stations);
        let mut working = p.clone();
        let anchor = working.adjustment_anchor_for_tests(a).unwrap();
        assert_eq!(
            working.adjust_loop_closures(anchor, ClosureStrategy::LeastSquaresWeighted, &config),
            Ok(())
        );
        if strategy != ClosureStrategy::Move {
            assert!(
                working
                    .loop_closures
                    .iter()
                    .find(|lc| lc.id == created.id)
                    .unwrap()
                    .is_aligned,
                "an eligible redundant tie takes part in the solve like any closure"
            );
        }
    }

    // A tie made only of closures (A:1 to X, X to B:2, then A:1 to B:2 again
    // through a station merged onto X) is accepted the same way.
    let (mut p, a, b) = issue_loop();
    let far = add(&mut p, vec![shot(180.0, 30.0), shot(90.0, 10.0)]);
    p.merge_stations(st(far, 0), st(a, 0)).unwrap();
    let x = st(far, 2);
    p.create_loop_closure(st(a, 1), x, ClosureStrategy::Batch)
        .unwrap();
    p.create_loop_closure(x, st(b, 2), ClosureStrategy::Batch)
        .unwrap();
    let reference = p.compute_adjusted_stations(&config);
    p.create_loop_closure(st(a, 1), st(b, 2), ClosureStrategy::Batch)
        .unwrap();
    assert!(p.closures_across_one_shot().is_empty());
    let report = p.compute_adjusted_stations(&config);
    assert!(report.failures.is_empty());
    assert_same_positions(&report.stations, &reference.stations);

    // Merged pairs keep their own, older refusal.
    let (mut p, _, b, c) = build();
    let err = p
        .create_loop_closure(st(c, 0), st(b, 3), ClosureStrategy::Batch)
        .unwrap_err();
    assert!(
        err.starts_with("These stations are already merged."),
        "{err}"
    );
}

#[test]
fn batch_creation_accepts_a_redundant_tie_like_the_single_path() {
    // The importer's path: a file can list the same tie twice through
    // different stations. Both closures are created, as before #737, and the
    // batch agrees with the single path closure for closure.
    fn build() -> (Project, Vec<(StationRef, StationRef)>) {
        let (mut p, a, b) = issue_loop();
        let c = add(&mut p, vec![shot(0.0, 5.0)]);
        p.merge_stations(st(c, 0), st(b, 3)).unwrap();
        let far = add(&mut p, vec![shot(180.0, 30.0), shot(90.0, 10.0)]);
        p.merge_stations(st(far, 0), st(a, 0)).unwrap();
        let pairs = vec![
            (st(c, 0), st(a, 2)),   // redundant: merge + existing closure
            (st(a, 1), st(far, 2)), // real loop
            (st(far, 2), st(b, 2)), // real loop
            (st(a, 1), st(b, 2)),   // redundant: through the two closures above
        ];
        (p, pairs)
    }
    let (mut seq, pairs) = build();
    let mut bat = seq.clone();
    let seq_results: Vec<_> = pairs
        .iter()
        .map(|&(x, y)| seq.create_loop_closure(x, y, ClosureStrategy::Batch))
        .collect();
    let bat_results = bat.create_loop_closures_batch(&pairs);
    for (s, t) in seq_results.iter().zip(&bat_results) {
        assert!(s.is_ok() && t.is_ok(), "seq {s:?} vs batch {t:?}");
    }
    let keys = |p: &Project| {
        p.loop_closures
            .iter()
            .map(|lc| (lc.id, lc.station_a.clone(), lc.station_b.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&seq), keys(&bat));
    assert_eq!(bat.loop_closures.len(), 5);
    assert!(bat.closures_across_one_shot().is_empty());
    let solved = bat.compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    assert!(solved.failures.is_empty(), "{:?}", solved.failures);
    // The two real loops moved things; the redundant ties added nothing on top.
    let mut without = bat.clone();
    let redundant: Vec<u64> = vec![bat.loop_closures[1].id, bat.loop_closures[4].id];
    without
        .loop_closures
        .retain(|lc| !redundant.contains(&lc.id));
    assert_same_positions(
        &solved.stations,
        &without
            .compute_adjusted_stations(&ClosureAdjustmentConfig::default())
            .stations,
    );
}

#[test]
fn creation_judges_against_the_knots_the_solve_uses() {
    // A closure the solve SKIPS ties nothing, so it cannot make a later pair
    // "across one shot".
    let (mut p, a, _) = issue_loop();
    plant(&mut p, st(a, 1), st(a, 2), ClosureStrategy::Batch);
    // With the skipped A:1-A:2 counted, A:0-A:2 would be one shot (A:0-A:1).
    assert!(p.closure_spans_one_shot(st(a, 0), st(a, 2)).is_none());
}

#[test]
fn batch_creation_refuses_the_same_pairs_with_the_same_messages() {
    fn build() -> (Project, Vec<(StationRef, StationRef)>) {
        let (mut p, a, b) = issue_loop();
        let far = add(&mut p, vec![shot(180.0, 30.0), shot(90.0, 10.0)]);
        p.merge_stations(st(far, 0), st(a, 0)).unwrap();
        let c = add(&mut p, vec![shot(0.0, 5.0)]);
        p.merge_stations(st(c, 0), st(b, 3)).unwrap();
        let pairs = vec![
            (st(a, 1), st(a, 2)),   // one shot directly
            (st(a, 1), st(far, 2)), // valid loop
            (st(far, 2), st(a, 2)), // one shot through the pair above (same batch)
            (st(c, 0), st(a, 2)),   // already tied through merge + closure: created
            (st(a, 0), st(far, 1)), // A:0 is merged to far:0, so one shot
            (st(b, 1), st(far, 1)), // valid loop
            (st(c, 1), st(far, 1)), // valid loop
        ];
        (p, pairs)
    }
    let (mut seq, pairs) = build();
    let mut bat = seq.clone();
    let seq_results: Vec<_> = pairs
        .iter()
        .map(|&(x, y)| seq.create_loop_closure(x, y, ClosureStrategy::Batch))
        .collect();
    let bat_results = bat.create_loop_closures_batch(&pairs);
    let errs = |r: &[Result<LoopClosure, String>]| -> Vec<Option<String>> {
        r.iter().map(|r| r.as_ref().err().cloned()).collect()
    };
    assert_eq!(errs(&seq_results), errs(&bat_results));
    let errs = errs(&bat_results);
    assert_eq!(errs[0].as_deref(), Some(SPANS_A1_A2));
    assert!(errs[1].is_none());
    assert_eq!(errs[2].as_deref(), Some(SPANS_A1_A2));
    assert!(
        errs[3].is_none(),
        "a redundant tie is accepted: {:?}",
        errs[3]
    );
    assert!(errs[4]
        .as_deref()
        .is_some_and(|e| e.contains("one shot (2:0 to 2:1)")));
    assert!(errs[5].is_none() && errs[6].is_none(), "{errs:?}");
    let ids = |p: &Project| {
        p.loop_closures
            .iter()
            .map(|lc| (lc.id, lc.station_a.clone(), lc.station_b.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&seq), ids(&bat));
    assert!(bat.closures_across_one_shot().is_empty());
}

// ---- the solve ---------------------------------------------------------------

#[test]
fn the_solve_skips_a_closure_across_one_shot_and_keeps_the_real_loop_closed() {
    let (mut p, a, b) = issue_loop();
    let config = ClosureAdjustmentConfig::default();
    let (b3, a2) = (st(b, 3), st(a, 2));
    let before = gap(&p.compute_adjusted_station_coordinates(&config), b3, a2);
    assert!(before < 1e-6, "the loop is closed before: {before}");
    // Merge tool A:1 then A:2 used to create this; plant it as an old file would.
    let id = plant(&mut p, st(a, 1), a2, ClosureStrategy::Batch);

    let mut working = p.clone();
    let anchor = working.adjustment_anchor_for_tests(a).unwrap();
    let solve =
        working.adjust_loop_closures(anchor, ClosureStrategy::LeastSquaresWeighted, &config);
    assert!(solve.is_ok(), "{solve:?}");
    // The skipped closure is not marked aligned; the real one is.
    for lc in &working.loop_closures {
        assert_eq!(lc.is_aligned, lc.id != id, "closure {}", lc.id);
    }

    let report = p.compute_adjusted_stations(&config);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    let after = gap(&report.stations, b3, a2);
    assert!(after < 1e-6, "the loop B:3-A:2 stays closed: {after}");
    // The shot the closure spans keeps its measured length.
    let shot_len = gap(&report.stations, st(a, 1), a2);
    assert!(shot_len > 9.0, "A:1-A:2 was not collapsed: {shot_len}");
}

#[test]
fn skipping_a_closure_solves_exactly_as_if_it_did_not_exist() {
    let (mut p, a, _) = issue_loop();
    let config = ClosureAdjustmentConfig::default();
    let without = p.compute_adjusted_station_coordinates(&config);
    plant(
        &mut p,
        st(a, 1),
        st(a, 2),
        ClosureStrategy::LeastSquaresWeighted,
    );
    let with = p.compute_adjusted_station_coordinates(&config);
    assert_eq!(without.len(), with.len());
    for s in &without {
        let (x, y) = coords(&with, s.station_ref);
        assert!(
            (x - s.coordinates.x).abs() < 1e-9 && (y - s.coordinates.y).abs() < 1e-9,
            "{:?} moved",
            s.station_ref
        );
    }
}

#[test]
fn a_component_whose_only_closure_spans_one_shot_solves_to_measured_without_error() {
    let mut p = Project::new("only".to_string(), None);
    let a = add(&mut p, vec![shot(90.0, 10.0), shot(0.0, 10.0)]);
    plant(&mut p, st(a, 1), st(a, 2), ClosureStrategy::Batch);
    let config = ClosureAdjustmentConfig::default();
    let mut working = p.clone();
    let anchor = working.adjustment_anchor_for_tests(a).unwrap();
    assert_eq!(
        working.adjust_loop_closures(anchor, ClosureStrategy::LeastSquaresWeighted, &config),
        Ok(())
    );
    assert!(!working.loop_closures[0].is_aligned, "nothing was adjusted");
    let report = p.compute_adjusted_stations(&config);
    assert!(report.failures.is_empty());
    let measured = p.compute_station_coordinates();
    for s in &measured {
        assert_eq!(
            coords(&report.stations, s.station_ref),
            (s.coordinates.x, s.coordinates.y)
        );
    }
    // A component with no eligible closure at all still reports that as an error.
    let mut none = Project::new("none".to_string(), None);
    let n = add(&mut none, vec![shot(90.0, 10.0)]);
    assert!(none
        .adjust_loop_closures(st(n, 0), ClosureStrategy::LeastSquaresWeighted, &config)
        .is_err());
}

#[test]
fn random_closures_never_fail_the_solve_and_match_the_solve_without_the_skipped_ones() {
    // Adversarial: random chains and side passages with random Batch/LSW
    // closures planted between random stations (no datums), so ties across one
    // shot arise directly, through merges and through other closures. The solve
    // must never fail, and dropping the reported closures must not change it.
    let config = ClosureAdjustmentConfig::default();
    let mut seed: u64 = 0x0737_5eed;
    let mut next = |m: u64| -> u64 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % m
    };
    let mut saw_skipped = 0;
    for _case in 0..60 {
        let mut p = Project::new("fuzz".to_string(), None);
        let mut sections = Vec::new();
        for i in 0..(2 + next(3)) {
            let shots = (0..=next(4))
                .map(|_| shot(next(360) as f64, 3.0 + next(10) as f64))
                .collect();
            let id = add(&mut p, shots);
            if i > 0 {
                let prev: SectionId = sections[next(sections.len() as u64) as usize];
                let len = p.get_section(prev).unwrap().shots.len() as u64;
                let _ = p.merge_stations(st(id, 0), st(prev, next(len + 1) as u32));
            }
            sections.push(id);
        }
        let stations: Vec<StationRef> = p
            .sections
            .iter()
            .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| st(s.id, i)))
            .collect();
        for _ in 0..=next(4) {
            let x = stations[next(stations.len() as u64) as usize];
            let y = stations[next(stations.len() as u64) as usize];
            if x != y {
                let strategy = if next(2) == 0 {
                    ClosureStrategy::Batch
                } else {
                    ClosureStrategy::LeastSquaresWeighted
                };
                plant(&mut p, x, y, strategy);
            }
        }
        let skipped = p.closures_across_one_shot();
        saw_skipped += usize::from(!skipped.is_empty());
        let report = p.compute_adjusted_stations(&config);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        let mut pruned = p.clone();
        pruned
            .loop_closures
            .retain(|lc| !skipped.iter().any(|s| s.closure_id == lc.id));
        assert!(pruned.closures_across_one_shot().is_empty());
        let reference = pruned.compute_adjusted_station_coordinates(&config);
        for s in &reference {
            let (x, y) = coords(&report.stations, s.station_ref);
            assert!(
                (x - s.coordinates.x).abs() < 1e-6 && (y - s.coordinates.y).abs() < 1e-6,
                "{:?}: {:?} vs {:?}",
                s.station_ref,
                (x, y),
                (s.coordinates.x, s.coordinates.y)
            );
        }
    }
    assert!(
        saw_skipped > 5,
        "the generator must reach the case: {saw_skipped}"
    );
}

// ---- failures are reported, not only logged ----------------------------------

#[test]
fn a_closure_joining_two_datums_is_reported_as_a_failure_and_a_sibling_still_adjusts() {
    let mut p = Project::new("datums".to_string(), None);
    // Component 1: two datum-anchored sections tied datum-to-datum by a closure,
    // which creation allows ("both sections have fixed coordinates").
    let x = add(&mut p, vec![shot(90.0, 10.0)]);
    let y = add(&mut p, vec![shot(0.0, 10.0)]);
    p.set_zero_datum(st(x, 0), GpsCoordinates::new(0.0, 0.0, 0.0).unwrap())
        .unwrap();
    p.set_zero_datum(st(y, 0), GpsCoordinates::new(0.0001, 0.0001, 0.0).unwrap())
        .unwrap();
    p.create_loop_closure(st(x, 0), st(y, 0), ClosureStrategy::Batch)
        .unwrap();
    // Component 2: the issue's loop, nowhere near it.
    let a = add(&mut p, vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let b = add(
        &mut p,
        vec![shot(0.0, 10.0), shot(90.0, 20.5), shot(180.0, 10.0)],
    );
    p.merge_stations(st(a, 0), st(b, 0)).unwrap();
    p.create_loop_closure(st(b, 3), st(a, 2), ClosureStrategy::Batch)
        .unwrap();

    let report = p.compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    let failure = &report.failures[0];
    assert_eq!(failure.section_ids, {
        let mut v = vec![x, y];
        v.sort_unstable();
        v
    });
    assert!(failure.section_ids.contains(&failure.anchor.section_id));
    assert_eq!(
        failure.error,
        "Multiple zero datums in a single constraint group"
    );
    assert!(
        failure
            .user_reason()
            .starts_with("loop closures tie two zero datums"),
        "{}",
        failure.user_reason()
    );
    // The sibling component is adjusted all the same.
    assert!(gap(&report.stations, st(b, 3), st(a, 2)) < 1e-6);
    // And the plain form still returns the same geometry.
    let plain = p.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
    for s in &plain {
        assert_eq!(
            coords(&report.stations, s.station_ref),
            (s.coordinates.x, s.coordinates.y)
        );
    }
}

#[test]
fn every_known_solver_error_has_a_plain_reason() {
    let failure = |error: &str| crate::AdjustmentFailure {
        anchor: st(0, 0),
        section_ids: vec![0],
        error: error.to_string(),
    };
    for (raw, starts) in [
        (
            "Multiple zero datums in a single constraint group",
            "loop closures tie two zero datums",
        ),
        (
            "Least squares adjustment produced inconsistent section translation",
            "the loop closures here could not be solved",
        ),
        (
            "Adjustment would move a zero datum station",
            "the adjustment would have moved a zero datum",
        ),
        ("Missing station root", "the loop-closure solve failed"),
    ] {
        let reason = failure(raw).user_reason();
        assert!(reason.starts_with(starts), "{raw} -> {reason}");
        assert!(
            !reason.contains(raw),
            "the raw text stays in the log: {reason}"
        );
    }
}

// ---- wire shape (#441 rule: an omitted field is declared optional) -----------

#[test]
fn the_new_wire_fields_are_omitted_when_absent_and_declared_optional() {
    use ts_rs::TS;

    let warning = crate::ProjectWarning {
        section_id: 0,
        shot_id: 1,
        shot_index: 0,
        code: crate::CLOSURE_ACROSS_ONE_SHOT.to_string(),
        message: String::new(),
        severity: crate::WarningSeverity::Error,
        is_resolved: false,
        shot_uuid: None,
        datum_station_index: None,
        datum_refs: None,
        closure_id: None,
    };
    let absent = serde_json::to_value(&warning).unwrap();
    assert!(absent.get("closureId").is_none(), "{absent}");
    let present = serde_json::to_value(crate::ProjectWarning {
        closure_id: Some(7),
        ..warning
    })
    .unwrap();
    assert_eq!(present["closureId"], serde_json::json!(7));
    let decl = crate::ProjectWarning::decl();
    assert!(decl.contains("closureId?: number"), "{decl}");

    let connectivity = crate::ConnectivityResult {
        connected: true,
        preview: None,
        can_merge: false,
        reason: None,
        closure_refusal: None,
    };
    let absent = serde_json::to_value(&connectivity).unwrap();
    assert!(absent.get("closure_refusal").is_none(), "{absent}");
    // `reason` is the control: always on the wire, so required in TS.
    assert!(absent.get("reason").is_some());
    let decl = crate::ConnectivityResult::decl();
    assert!(decl.contains("closure_refusal?: string"), "{decl}");
    assert!(decl.contains("reason: string | null"), "{decl}");
    // A pre-#737 payload without the key still reads.
    let old: crate::ConnectivityResult = serde_json::from_value(
        serde_json::json!({"connected": true, "preview": null, "can_merge": false, "reason": null}),
    )
    .unwrap();
    assert!(old.closure_refusal.is_none());
}

// ---- cost and a brute-force reference -----------------------------------------

/// A Batch closure with positional keys, cheap to resolve, for the large tests.
fn plant_positional(project: &mut Project, id: u64, a: StationRef, b: StationRef) {
    project.loop_closures.push(LoopClosure {
        id,
        station_a: a.into(),
        station_b: b.into(),
        path_length: 1.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
}

#[test]
fn many_skipped_closures_between_two_big_knots_stay_cheap() {
    // A hostile or legacy file: one section of 20_000 shots, closures tying
    // every even station into one knot and every odd station into another,
    // then 60_000 closures each joining an even station to an odd one. Every
    // one of those collapses a shot, so every one is skipped and none unites
    // anything: the sweep asks about the same two 10_000-station knots 60_000
    // times. Walking a knot per question was ~6e8 steps (minutes in debug);
    // the contracted adjacency answers each in O(1).
    const SHOTS: u32 = 20_000;
    const CROSS: u32 = 60_000;
    let mut p = Project::new("stress".to_string(), None);
    let s = add(&mut p, (0..SHOTS).map(|_| shot(90.0, 1.0)).collect());
    let mut id = 1;
    for i in 0..(SHOTS - 1) {
        // (0,2), (1,3), (2,4), ...: two chains, evens and odds.
        plant_positional(&mut p, id, st(s, i), st(s, i + 2));
        id += 1;
    }
    let chains = id - 1;
    for k in 0..CROSS {
        let even = 2 * (k % (SHOTS / 2));
        let odd = 2 * ((k * 7 + 3) % (SHOTS / 2)) + 1;
        plant_positional(&mut p, id, st(s, even), st(s, odd));
        id += 1;
    }
    let started = std::time::Instant::now();
    let skipped = p.closures_across_one_shot();
    let elapsed = started.elapsed();
    assert_eq!(
        skipped.len(),
        CROSS as usize,
        "every even-odd closure is skipped"
    );
    assert!(skipped.iter().all(|c| c.closure_id > chains));
    // Two knots, one shot between every even and odd neighbour: the lowest is shot 0.
    assert!(skipped.iter().all(|c| c.collapsed_shot == (s, 0)));
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "sweep took {elapsed:?}"
    );
    // The creation-time question about the same two knots is as cheap.
    let started = std::time::Instant::now();
    assert_eq!(
        p.closure_spans_one_shot(st(s, 10), st(s, 9_999)),
        Some((s, 0))
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

/// The rule, spelled out the slow way: a plain union-find over stations; a
/// closure is skipped when some shot of the project has one end in each of its
/// two (distinct) knots, the lowest such shot is reported, and a skipped
/// closure unites nothing.
fn across_one_shot_by_brute_force(p: &Project) -> Vec<ClosureAcrossOneShot> {
    use std::collections::HashMap;
    fn find(parent: &HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
        let mut r = x;
        while let Some(&n) = parent.get(&r) {
            if n == r {
                break;
            }
            r = n;
        }
        r
    }
    let mut parent: HashMap<StationRef, StationRef> = HashMap::new();
    for section in &p.sections {
        for i in 0..=section.shots.len() as u32 {
            parent.insert(st(section.id, i), st(section.id, i));
        }
    }
    let union = |parent: &mut HashMap<StationRef, StationRef>, a, b| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent.insert(ra, rb);
        }
    };
    for m in &p.station_merges {
        if let (Some(a), Some(b)) = (p.resolve_key(&m.station_a), p.resolve_key(&m.station_b)) {
            union(&mut parent, a, b);
        }
    }
    let mut out = Vec::new();
    for lc in &p.loop_closures {
        if !matches!(
            lc.strategy,
            ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted
        ) {
            continue;
        }
        let (Some(a), Some(b)) = (p.resolve_key(&lc.station_a), p.resolve_key(&lc.station_b))
        else {
            continue;
        };
        let (ra, rb) = (find(&parent, a), find(&parent, b));
        if ra == rb {
            continue;
        }
        let mut lowest: Option<(SectionId, usize)> = None;
        for section in &p.sections {
            for i in 0..section.shots.len() {
                let x = find(&parent, st(section.id, i as u32));
                let y = find(&parent, st(section.id, i as u32 + 1));
                if (x == ra && y == rb) || (x == rb && y == ra) {
                    let c = (section.id, i);
                    lowest = Some(lowest.map_or(c, |l| l.min(c)));
                }
            }
        }
        match lowest {
            Some(collapsed_shot) => out.push(ClosureAcrossOneShot {
                closure_id: lc.id,
                station_a: a,
                station_b: b,
                collapsed_shot,
            }),
            None => union(&mut parent, a, b),
        }
    }
    out
}

#[test]
fn the_helper_matches_a_brute_force_reference_on_random_projects() {
    let mut seed: u64 = 0x0b2c_7e57;
    let mut next = |m: u64| -> u64 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % m.max(1)
    };
    let mut skipped_total = 0;
    for _case in 0..200 {
        let mut p = Project::new("ref".to_string(), None);
        let mut sections: Vec<SectionId> = Vec::new();
        for i in 0..=next(4) {
            let id = add(
                &mut p,
                (0..=next(5)).map(|_| shot(next(360) as f64, 2.0)).collect(),
            );
            if i > 0 {
                let prev = sections[next(sections.len() as u64) as usize];
                let len = p.get_section(prev).unwrap().shots.len() as u64;
                let _ = p.merge_stations(st(id, next(2) as u32), st(prev, next(len + 1) as u32));
            }
            sections.push(id);
        }
        let stations: Vec<StationRef> = p
            .sections
            .iter()
            .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| st(s.id, i)))
            .collect();
        let pick = |r: u64| stations[r as usize % stations.len()];
        for _ in 0..=next(8) {
            let (x, y) = (pick(next(1000)), pick(next(1000)));
            let strategy = match next(4) {
                0 => ClosureStrategy::Move,
                1 => ClosureStrategy::LeastSquaresWeighted,
                _ => ClosureStrategy::Batch,
            };
            plant(&mut p, x, y, strategy);
        }
        let expected = across_one_shot_by_brute_force(&p);
        assert_eq!(p.closures_across_one_shot(), expected);
        skipped_total += expected.len();
        // The creation-time question agrees too: on top of the knots the
        // reference built, would a new closure collapse a shot, and which?
        let (x, y) = (pick(next(1000)), pick(next(1000)));
        let mut with_probe = p.clone();
        plant(&mut with_probe, x, y, ClosureStrategy::Batch);
        let probe_id = with_probe.loop_closures.last().unwrap().id;
        let reference = across_one_shot_by_brute_force(&with_probe)
            .into_iter()
            .find(|c| c.closure_id == probe_id)
            .map(|c| c.collapsed_shot);
        assert_eq!(p.closure_spans_one_shot(x, y), reference, "{x:?} {y:?}");
    }
    assert!(
        skipped_total > 20,
        "the generator must reach the case: {skipped_total}"
    );
}
