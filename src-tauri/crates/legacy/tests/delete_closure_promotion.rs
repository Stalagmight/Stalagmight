//! (d) The frozen Delete Shot carries release/0.6.0's #822 fix (#776): the
//! closure loop of `preview_delete_shot` asks its guards about the graph
//! that includes the merges it has already planned, so a later closure whose
//! ends a planned merge connects stays a loop, and one whose ends it makes
//! one point closes nothing. The release's own model tests
//! (`models/src/project/shots_closure_promotion_tests.rs` at release/0.6.0
//! 1f41128eb), ported to the frozen crate's public API; the frozen copy
//! drops its test modules. Closures are planted directly, as an older or
//! imported file can carry them. Synthetic data only.
#![expect(clippy::unwrap_used, reason = "test setup and assertions")]

use stalagmight_legacy::models::{
    ClosureStrategy, LineData, LineId, LoopClosure, Project, ShotData, ShotId, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, name: &str, shots: Vec<ShotData>) -> LineId {
    project
        .add_line(LineData {
            name: Some(name.to_string()),
            shots,
            ..Default::default()
        })
        .id
}

fn st(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

fn plant(project: &mut Project, a: StationRef, b: StationRef) -> u64 {
    let id = project.next_closure_id.max(1);
    project.next_closure_id = id + 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a: project.key_for_ref(a).unwrap(),
        station_b: project.key_for_ref(b).unwrap(),
        path_length: 20.0,
        misclosure_distance: 0.1,
        error_percentage: 0.5,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    id
}

/// Every merge as a sorted pair of positional refs.
fn merges(p: &Project) -> Vec<(StationRef, StationRef)> {
    let mut out: Vec<_> = p
        .station_merges
        .iter()
        .map(|m| {
            let (a, b) = (
                p.resolve_key(&m.station_a).unwrap(),
                p.resolve_key(&m.station_b).unwrap(),
            );
            if (a.line_id, a.station_index) <= (b.line_id, b.station_index) {
                (a, b)
            } else {
                (b, a)
            }
        })
        .collect();
    out.sort_by_key(|(a, b)| (a.line_id, a.station_index, b.line_id, b.station_index));
    out
}

/// A runs 3 x 20 m east, B 2 x 10 m north from elsewhere, C hangs one 10 m
/// shot off B's end (C:1 merged onto B:2), D is a one-shot line placed by
/// the caller. No datums: nothing here is double-anchored.
fn survey() -> (Project, LineId, LineId, LineId, LineId) {
    let mut p = Project::new("planned".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 20.0); 3]);
    let b = add(&mut p, "B", vec![shot(0.0, 10.0); 2]);
    let c = add(&mut p, "C", vec![shot(270.0, 10.0)]);
    let d = add(&mut p, "D", vec![shot(180.0, 10.0)]);
    p.merge_stations(st(c, 1), st(b, 2)).unwrap();
    (p, a, b, c, d)
}

fn last_shot(p: &Project, line: LineId) -> ShotId {
    p.get_line(line).unwrap().shots.last().unwrap().id
}

#[test]
fn a_closure_the_planned_merges_make_one_point_is_dropped() {
    // D:0 is merged onto A:1. B:2 closes onto A:1, then onto D:0. Deleting B's
    // last shot promotes the first closure to C:1 = A:1, which with D:0 = A:1
    // makes C:1 and D:0 one point: the second closure closes nothing.
    let (mut p, a, b, c, d) = survey();
    p.merge_stations(st(d, 0), st(a, 1)).unwrap();
    plant(&mut p, st(b, 2), st(a, 1));
    plant(&mut p, st(b, 2), st(d, 0));
    let shot_id = last_shot(&p, b);
    let impact = p.preview_delete_shot(b, shot_id).unwrap();
    let promoted: Vec<_> = impact
        .closure_to_merge
        .iter()
        .map(|pair| (pair.station_a, pair.station_b))
        .collect();
    assert_eq!(promoted, vec![(st(c, 1), st(a, 1))], "{impact:?}");
    assert!(impact.replacement_closure_updates.is_empty(), "{impact:?}");
    assert!(impact.closure_rewire_skipped, "{impact:?}");

    p.remove_shot_with_rewire(b, shot_id).unwrap();
    assert!(p.loop_closures.is_empty(), "{:?}", p.loop_closures);
    assert_eq!(merges(&p), vec![(st(a, 1), st(c, 1)), (st(a, 1), st(d, 0))]);
    assert!(p.check_merge_alignment().is_empty());
}

#[test]
fn a_planned_merge_joins_nothing_it_does_not_touch() {
    // B:2 closes onto A:1, then onto D:1, and D is tied to nothing else. The
    // first promotion (C:1 = A:1) does not connect C:1 to D, so the second
    // closure's loop is broken as well: it is promoted too, not re-anchored.
    let (mut p, a, b, c, d) = survey();
    plant(&mut p, st(b, 2), st(a, 1));
    plant(&mut p, st(b, 2), st(d, 1));
    let shot_id = last_shot(&p, b);
    let impact = p.preview_delete_shot(b, shot_id).unwrap();
    let promoted: Vec<_> = impact
        .closure_to_merge
        .iter()
        .map(|pair| (pair.station_a, pair.station_b))
        .collect();
    assert_eq!(
        promoted,
        vec![(st(c, 1), st(a, 1)), (st(c, 1), st(d, 1))],
        "{impact:?}"
    );
    assert!(impact.replacement_closure_updates.is_empty(), "{impact:?}");

    p.remove_shot_with_rewire(b, shot_id).unwrap();
    assert!(p.loop_closures.is_empty(), "{:?}", p.loop_closures);
    assert_eq!(merges(&p), vec![(st(a, 1), st(c, 1)), (st(c, 1), st(d, 1))]);
    assert!(p.check_merge_alignment().is_empty());
}
