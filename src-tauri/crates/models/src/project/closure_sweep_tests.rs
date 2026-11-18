//! The orphan-closure sweep and closures between merged stations (#738): what
//! it removes, what it never removes, and what its scoped form (the one an
//! inverse runs) leaves as recorded. Synthetic data only.

use std::collections::HashSet;

use crate::{
    ClosureStrategy, LineData, LineId, LoopClosure, Project, ShotData, SplitLineOptions, SplitSide,
    StationRef,
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

/// Plant a closure directly, as a file written before #738 can carry it
/// (creation refuses one between merged stations).
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

fn ids(closures: &[LoopClosure]) -> Vec<u64> {
    closures.iter().map(|c| c.id).collect()
}

/// R is a loop on itself (east, north, west, south to 0.2 m short of its
/// start); T starts merged at R:1 and U at T:0, so R:1, T:0 and U:0 are one
/// point; V starts merged at R:3.
fn network() -> (Project, [LineId; 4]) {
    let mut p = Project::new("sweep".to_string(), None);
    let r = add(
        &mut p,
        "R",
        vec![
            shot(90.0, 10.0),
            shot(0.0, 10.0),
            shot(270.0, 10.0),
            shot(180.0, 9.8),
        ],
    );
    let t = add(&mut p, "T", vec![shot(90.0, 5.0), shot(0.0, 5.0)]);
    let u = add(&mut p, "U", vec![shot(45.0, 5.0)]);
    let v = add(&mut p, "V", vec![shot(0.0, 5.0)]);
    p.merge_stations(st(r, 1), st(t, 0)).unwrap();
    p.merge_stations(st(t, 0), st(u, 0)).unwrap();
    p.merge_stations(st(r, 3), st(v, 0)).unwrap();
    (p, [r, t, u, v])
}

/// A closure already between merged stations - on a directly merged pair, or
/// through a chain of merges - is removed whenever the sweep runs (the load
/// is one of those runs). Nothing else is: not a real loop, not a closure
/// whose two lines are merged elsewhere, not one tying a station to a
/// station merely co-located with it, not one with an end that no longer
/// resolves. No merge is added for a removal.
#[test]
fn a_closure_already_between_merged_stations_goes_and_nothing_else_does() {
    let (mut p, [r, t, u, v]) = network();
    let real = p
        .create_loop_closure(st(r, 4), st(r, 0), ClosureStrategy::Batch)
        .unwrap()
        .id;
    // T and V both hang off R, at different points: connected, not merged.
    let across = p
        .create_loop_closure(st(t, 2), st(v, 1), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let direct = plant(&mut p, st(t, 0), st(r, 1));
    let chained = plant(&mut p, st(u, 0), st(r, 1));
    // U:0 sits on R:1's point by a merge chain, and R:1 itself is where T
    // and U start; R:2 is a different station, 10 m away.
    let elsewhere = plant(&mut p, st(u, 0), st(r, 2));
    // An end that no longer resolves: its shot is gone.
    let dangling = plant(&mut p, st(t, 2), st(r, 1));
    p.get_line_mut(t).unwrap().shots.pop();
    p.rebuild_shot_uuid_index();
    assert!(p
        .resolve_key(&p.loop_closures.last().unwrap().station_a)
        .is_none());
    let merges = p.station_merges.len();

    let sweep = p.sweep_orphan_closures();

    assert_eq!(sweep.converted, 0);
    assert_eq!(ids(&sweep.removed), [direct, chained]);
    // `across` lost its T end with the shot too, so it dangles: kept.
    assert_eq!(ids(&p.loop_closures), [real, across, elsewhere, dangling]);
    assert_eq!(p.station_merges.len(), merges);

    // Nothing left for a second run.
    let again = p.sweep_orphan_closures();
    assert_eq!((again.converted, again.removed.len()), (0, 0));
}

/// The four closures from the test above, but all ends resolving: still only
/// the two between merged stations go.
#[test]
fn the_sweep_removes_a_closure_only_when_its_stations_are_merged() {
    let (mut p, [r, t, u, v]) = network();
    let real = p
        .create_loop_closure(st(r, 4), st(r, 0), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let across = p
        .create_loop_closure(st(t, 2), st(v, 1), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let direct = plant(&mut p, st(t, 0), st(r, 1));
    let elsewhere = plant(&mut p, st(u, 0), st(r, 2));
    let chained = plant(&mut p, st(r, 1), st(u, 0));
    let before = p.clone();

    let sweep = p.sweep_orphan_closures();

    assert_eq!(ids(&sweep.removed), [direct, chained]);
    for closure in &sweep.removed {
        let a = before.resolve_key(&closure.station_a).unwrap();
        let b = before.resolve_key(&closure.station_b).unwrap();
        assert!(before.are_stations_merged(a, b), "{a:?} - {b:?}");
    }
    for closure in &p.loop_closures {
        let a = p.resolve_key(&closure.station_a).unwrap();
        let b = p.resolve_key(&closure.station_b).unwrap();
        assert!(!p.are_stations_merged(a, b), "{a:?} - {b:?}");
    }
    assert_eq!(ids(&p.loop_closures), [real, across, elsewhere]);
}

fn split_options() -> SplitLineOptions {
    SplitLineOptions {
        name_a: "S-A".to_string(),
        name_b: "S-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    }
}

/// The scoped sweep, the one an inverse runs: a closure between merged
/// stations that was there BEFORE the mutation is part of the state the
/// history recorded, so it stays; one the mutation's own promotion makes
/// redundant goes. Here the mutation is the issue's split (two closures tie
/// S:3 to the point {S:1, T:0}), with a recorded closure T:1 - W:0 whose
/// stations W's merge already made one point.
#[test]
fn the_scoped_sweep_removes_what_its_promotion_merged_and_keeps_what_it_found() {
    let mut p = Project::new("scoped".to_string(), None);
    let s = add(
        &mut p,
        "S",
        vec![shot(90.0, 10.0), shot(0.0, 10.0), shot(180.0, 9.9)],
    );
    let t = add(&mut p, "T", vec![shot(90.0, 5.0)]);
    let w = add(&mut p, "W", vec![shot(0.0, 5.0)]);
    p.merge_stations(st(s, 1), st(t, 0)).unwrap();
    p.merge_stations(st(t, 1), st(w, 0)).unwrap();
    let first = p
        .create_loop_closure(st(s, 3), st(s, 1), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let second = p
        .create_loop_closure(st(t, 0), st(s, 3), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let recorded = plant(&mut p, st(t, 1), st(w, 0));
    let preexisting = p.swept_closure_ids();
    assert_eq!(preexisting, HashSet::from([recorded]));

    p.split_line_at_station(s, 2, &split_options()).unwrap();
    let sweep = p.convert_closures_orphaned_since(&preexisting);
    p.settle_minted_merges(&sweep.minted);

    assert_eq!(sweep.minted.len(), 1, "{first} became the merge");
    assert_eq!(ids(&sweep.removed), [second]);
    assert_eq!(ids(&p.loop_closures), [recorded]);
    // The whole-project sweep, a forward edit's, takes the recorded one too.
    assert_eq!(ids(&p.sweep_orphan_closures().removed), [recorded]);
    assert!(p.loop_closures.is_empty());
}

/// A closure from a station to itself closes nothing either, and goes too. It
/// used to be kept: it is in one line, so it was never an orphan.
#[test]
fn a_closure_from_a_station_to_itself_goes() {
    let (mut p, [r, t, _, _]) = network();
    let real = p
        .create_loop_closure(st(r, 4), st(r, 0), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let to_itself = plant(&mut p, st(r, 2), st(r, 2));
    let also_to_itself = plant(&mut p, st(t, 1), st(t, 1));

    let sweep = p.sweep_orphan_closures();

    assert_eq!(ids(&sweep.removed), [to_itself, also_to_itself]);
    assert_eq!(sweep.removed_positions, [1, 2]);
    assert_eq!(ids(&p.loop_closures), [real]);
}

/// A merge never leaves a closure between merged stations (#738). Every
/// closure shape the app keeps, with each end merged to the other, directly or
/// through the other's merge group: a loop on one line and two branches off
/// one point tied at their far ends are refused as already connected, and
/// nothing changes. Two anchored surveys tied by a closure MAY merge (#741: the
/// later placement datum becomes a GPS fix); the merge makes that closure's two
/// ends one point, and the sweep the merge command runs removes it.
#[test]
fn a_merge_never_leaves_a_closure_between_merged_stations() {
    let (mut p, [r, t, u, v]) = network();
    p.create_loop_closure(st(r, 4), st(r, 0), ClosureStrategy::Batch)
        .unwrap();
    p.create_loop_closure(st(t, 2), st(v, 1), ClosureStrategy::Batch)
        .unwrap();
    let before = serde_json::to_string(&p).unwrap();
    for (x, y) in [
        (st(r, 4), st(r, 0)),
        (st(t, 2), st(v, 1)),
        (st(t, 2), st(r, 3)),
        (st(u, 0), st(v, 1)),
    ] {
        let refused = p.merge_stations(x, y).unwrap_err();
        assert!(
            refused.contains("already connected"),
            "{x:?} {y:?}: {refused}"
        );
    }
    assert_eq!(serde_json::to_string(&p).unwrap(), before);

    // Two anchored surveys, tied by a closure (a sole bridge the sweep must
    // leave, since promoting it would weld two anchors into one group).
    let mut q = Project::new("anchored".to_string(), None);
    let a = add(&mut q, "A", vec![shot(90.0, 10.0)]);
    let b = add(&mut q, "B", vec![shot(0.0, 10.0)]);
    let c = add(&mut q, "C", vec![shot(0.0, 3.0)]);
    q.merge_stations(st(b, 1), st(c, 0)).unwrap();
    let gps = |lat| crate::GpsCoordinates::new(lat, -87.4, 0.0).unwrap();
    q.set_zero_datum(st(a, 0), gps(20.5)).unwrap();
    q.set_zero_datum(st(b, 0), gps(20.6)).unwrap();
    q.create_loop_closure(st(a, 1), st(b, 1), ClosureStrategy::Batch)
        .unwrap();
    assert_eq!(q.sweep_orphan_closures().converted, 0, "the bridge stays");
    let bridge = q.loop_closures[0].id;
    for (x, y) in [(st(a, 1), st(b, 1)), (st(a, 1), st(c, 0))] {
        let mut merged = q.clone();
        merged.merge_stations(x, y).unwrap();
        let sweep = merged.sweep_orphan_closures();
        assert_eq!(ids(&sweep.removed), [bridge], "{x:?} {y:?}");
        assert_eq!(sweep.converted, 0, "{x:?} {y:?}");
        assert!(merged.loop_closures.is_empty(), "{x:?} {y:?}");
        assert!(merged.are_stations_merged(st(a, 1), st(b, 1)));
    }
}
