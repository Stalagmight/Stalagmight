//! The least-squares anchor of a component across Combine Sections and Split
//! Section Here (`project/gauge.rs`): over every pairing, every combination of
//! reverse parities, every order the sections were created in (which decides
//! the lowest id, and so which section anchors the component), either kept
//! section, and a captured or uncaptured gauge, the closure-adjusted position
//! of every station is the same after the edit as before.
//!
//! Stations are matched by their MEASURED position, which both edits keep for
//! every station: a combine renumbers the kept section's stations and moves
//! the other section's into it, a split duplicates the cut station, so a
//! station's reference is not its identity across the edit. The few stations
//! that share a measured position are merged (one node in the adjustment), so
//! every match must agree.

use crate::config::ClosureAdjustmentConfig;
use crate::{
    ClosureStrategy, CombinePick, CombineSectionsOptions, LoopClosure, Project, SectionData,
    SectionId, ShotData, SplitSectionOptions, SplitSide, StationRef,
};

use super::gauge::gauge_pair_for_station;

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, name: &str, shots: Vec<ShotData>) -> SectionId {
    project
        .add_section(SectionData {
            name: Some(name.to_string()),
            shots,
            ..Default::default()
        })
        .id
}

fn raw(project: &Project, at: StationRef) -> (f64, f64) {
    let c = project.compute_station_raw_coords(at).unwrap();
    (c.x, c.y)
}

/// Move `id` so its station `index` sits on `target`.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64)) {
    let now = raw(project, StationRef::new(id, index));
    project.adjust_section_offset(id, target.0 - now.0, target.1 - now.1);
}

/// An unaligned, least-squares closure between `a` and `b`.
fn close(project: &mut Project, a: StationRef, b: StationRef) {
    let (station_a, station_b) = (
        project.key_for_ref(a).unwrap(),
        project.key_for_ref(b).unwrap(),
    );
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a,
        station_b,
        path_length: 20.0,
        misclosure_distance: 1.0,
        error_percentage: 5.0,
        horizontal_misclosure: 1.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    });
}

/// Every station's measured and closure-adjusted position.
fn positions(project: &Project) -> Vec<((f64, f64), (f64, f64))> {
    project
        .compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default())
        .into_iter()
        .map(|s| {
            (
                raw(project, s.station_ref),
                (s.coordinates.x, s.coordinates.y),
            )
        })
        .collect()
}

/// The worst disagreement between a station's adjusted position before and
/// after, over every pair of stations at the same measured position; and the
/// count of stations with no counterpart (there must be none).
fn worst_shift(
    before: &[((f64, f64), (f64, f64))],
    after: &[((f64, f64), (f64, f64))],
) -> (f64, usize) {
    let same = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).hypot(p.1 - q.1) < 1e-6;
    let mut worst: f64 = 0.0;
    let mut unmatched = 0;
    for (measured, adjusted) in before {
        let matches: Vec<_> = after.iter().filter(|(m, _)| same(*m, *measured)).collect();
        if matches.is_empty() {
            unmatched += 1;
        }
        for (_, moved) in matches {
            worst = worst.max((moved.0 - adjusted.0).hypot(moved.1 - adjusted.1));
        }
    }
    unmatched += after
        .iter()
        .filter(|(m, _)| !before.iter().any(|(b, _)| same(*b, *m)))
        .count();
    (worst, unmatched)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Role {
    Kept,
    Other,
    Third,
}

/// The creation orders of the three sections: the first created has the
/// lowest id and anchors the component.
const ORDERS: [[Role; 3]; 6] = [
    [Role::Kept, Role::Other, Role::Third],
    [Role::Kept, Role::Third, Role::Other],
    [Role::Other, Role::Kept, Role::Third],
    [Role::Other, Role::Third, Role::Kept],
    [Role::Third, Role::Kept, Role::Other],
    [Role::Third, Role::Other, Role::Kept],
];

/// One combine shape: K (two shots east from the origin) and O (three shots)
/// merged end to end, `o_at_k_end` choosing K's physical end or start for the
/// join and `o_start_joins` O's start or end; a third section T merged at K's
/// other end, or at the join itself (`t_at_join`, a junction), closes a loop
/// onto K's middle station 1 m short. Then each of K and O is reversed when
/// asked, which keeps every station where it is.
fn combine_shape(
    order: [Role; 3],
    o_at_k_end: bool,
    o_start_joins: bool,
    reverse: (bool, bool),
) -> (Project, SectionId, SectionId) {
    combine_shape_with(order, o_at_k_end, o_start_joins, reverse, false).0
}

fn combine_shape_with(
    order: [Role; 3],
    o_at_k_end: bool,
    o_start_joins: bool,
    reverse: (bool, bool),
    t_at_join: bool,
) -> ((Project, SectionId, SectionId), SectionId) {
    let mut project = Project::new("Matrix".to_string(), None);
    let mut ids = [0; 3];
    for role in order {
        let (name, shots) = match role {
            Role::Kept => ("K", vec![shot(90.0, 10.0), shot(90.0, 10.0)]),
            Role::Other => (
                "O",
                vec![shot(200.0, 4.0), shot(300.0, 6.0), shot(250.0, 3.0)],
            ),
            Role::Third => {
                // T heads back over K to 1 m short of its middle station,
                // from whichever end of K it starts at.
                let t_starts_east = o_at_k_end == t_at_join;
                let across = if t_starts_east { 270.0 } else { 90.0 };
                (
                    "T",
                    vec![shot(0.0, 5.0), shot(across, 10.0), shot(180.0, 4.0)],
                )
            }
        };
        ids[role as usize] = add(&mut project, name, shots);
    }
    let [k, o, t] = ids;
    let (k_join, k_free) = if o_at_k_end { (2, 0) } else { (0, 2) };
    let o_join = if o_start_joins { 0 } else { 3 };
    let at = raw(&project, StationRef::new(k, k_join));
    place(&mut project, o, o_join, at);
    project
        .merge_stations(StationRef::new(k, k_join), StationRef::new(o, o_join))
        .unwrap();
    let t_at = if t_at_join { k_join } else { k_free };
    let at = raw(&project, StationRef::new(k, t_at));
    place(&mut project, t, 0, at);
    project
        .merge_stations(StationRef::new(k, t_at), StationRef::new(t, 0))
        .unwrap();
    close(&mut project, StationRef::new(t, 3), StationRef::new(k, 1));
    if reverse.0 {
        assert!(project.reverse_section(k, false));
    }
    if reverse.1 {
        assert!(project.reverse_section(o, false));
    }
    ((project, k, o), t)
}

fn combine_options() -> CombineSectionsOptions {
    CombineSectionsOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

/// Combine never moves the closure-adjusted survey: over every pairing and
/// parity, every creation order (so the anchor is K's, O's or a third
/// section's, including a third section with an id between the two), either
/// section kept, gauges captured by a save or not, and the third section
/// hanging off K's far end or merged at the join itself (a junction, where it
/// stays as a branch). The third section's record parities never change.
#[test]
fn combine_keeps_every_adjusted_station_in_place() {
    let mut cases = 0;
    for order in ORDERS {
        for o_at_k_end in [false, true] {
            for o_start_joins in [false, true] {
                for reverse in [(false, false), (true, false), (false, true), (true, true)] {
                    for keep_k in [true, false] {
                        for (captured, t_at_join) in
                            [(false, false), (true, false), (false, true), (true, true)]
                        {
                            let ((mut project, k, o), t) = combine_shape_with(
                                order,
                                o_at_k_end,
                                o_start_joins,
                                reverse,
                                t_at_join,
                            );
                            if captured {
                                project.capture_missing_gauge_anchors();
                            }
                            let what = format!(
                                "{order:?} O at K's {}, O's {} joins, reversed {reverse:?}, \
                                 keep {}, captured {captured}, T at the join {t_at_join}",
                                if o_at_k_end { "end" } else { "start" },
                                if o_start_joins { "start" } else { "end" },
                                if keep_k { "K" } else { "O" },
                            );
                            let (kept, other) = if keep_k { (k, o) } else { (o, k) };
                            let before = positions(&project);
                            let anchor = project.adjustment_anchor_for_tests(kept).unwrap();
                            let anchored_at = raw(&project, anchor);
                            let t_parities = record_parities(&project, t);

                            project
                                .combine_sections(kept, other, &combine_options())
                                .unwrap_or_else(|e| panic!("{what}: {e}"));
                            assert_eq!(record_parities(&project, t), t_parities, "{what}");

                            let (worst, unmatched) = worst_shift(&before, &positions(&project));
                            assert_eq!(unmatched, 0, "{what}");
                            assert!(worst < 1e-6, "{what}: an adjusted station moved {worst} m");
                            let anchor = project.adjustment_anchor_for_tests(kept).unwrap();
                            let now = raw(&project, anchor);
                            assert!(
                                (now.0 - anchored_at.0).hypot(now.1 - anchored_at.1) < 1e-6,
                                "{what}: the anchor moved to {anchor:?}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 768);
}

/// The record parity of every shot of `section`, in order.
fn record_parities(project: &Project, section: SectionId) -> Vec<bool> {
    let section = project.get_section(section).unwrap();
    section
        .shots
        .iter()
        .map(|shot| project.shot_record_parity(section, shot.uuid.as_deref()))
        .collect()
}

/// One split shape: S, four shots east from the origin; X merged at S's
/// physical station `cut` loops north and back onto S's far end, 1 m short.
/// S's gauge is put on physical station `gauge` (or left uncaptured), then S
/// is reversed when asked, which keeps the gauge on its station.
fn split_shape(
    x_first: bool,
    cut: u32,
    gauge: Option<u32>,
    reversed: bool,
) -> (Project, SectionId, u32) {
    let mut project = Project::new("Matrix".to_string(), None);
    let s_shots = (0..4).map(|_| shot(90.0, 10.0)).collect::<Vec<_>>();
    let x_shots = vec![
        shot(0.0, 10.0),
        shot(90.0, 10.0 * f64::from(4 - cut)),
        shot(180.0, 9.0),
    ];
    let (s, x) = if x_first {
        let x = add(&mut project, "X", x_shots);
        (add(&mut project, "S", s_shots), x)
    } else {
        let s = add(&mut project, "S", s_shots);
        (s, add(&mut project, "X", x_shots))
    };
    let at = raw(&project, StationRef::new(s, cut));
    place(&mut project, x, 0, at);
    project
        .merge_stations(StationRef::new(s, cut), StationRef::new(x, 0))
        .unwrap();
    close(&mut project, StationRef::new(x, 3), StationRef::new(s, 4));
    if let Some(gauge) = gauge {
        let section = project.get_section_mut(s).unwrap();
        let (shot, neighbour) = gauge_pair_for_station(section, gauge as usize).unwrap();
        section.gauge_shot_uuid = Some(shot);
        section.gauge_neighbour_uuid = neighbour;
    }
    let cut = if reversed {
        assert!(project.reverse_section(s, false));
        4 - cut
    } else {
        cut
    };
    (project, s, cut)
}

/// Split never moves the closure-adjusted survey when the halves stay one
/// component (X is merged at the cut, so both copies of the cut station are
/// merged to it): over every mid-station cut, every gauge station of the cut
/// section (and an uncaptured gauge), either orientation, and either section
/// created first.
#[test]
fn split_keeps_every_adjusted_station_in_place() {
    let mut cases = 0;
    for x_first in [false, true] {
        for cut in 1..=3 {
            for gauge in [None, Some(0), Some(1), Some(2), Some(3), Some(4)] {
                for reversed in [false, true] {
                    let (mut project, s, at) = split_shape(x_first, cut, gauge, reversed);
                    let what = format!(
                        "X first {x_first}, cut at physical {cut}, gauge {gauge:?}, reversed \
                         {reversed}"
                    );
                    let before = positions(&project);
                    let anchor = project.adjustment_anchor_for_tests(s).unwrap();
                    let anchored_at = raw(&project, anchor);
                    let options = SplitSectionOptions {
                        name_a: "S-A".to_string(),
                        name_b: "S-B".to_string(),
                        color_a: "#111111".to_string(),
                        color_b: "#222222".to_string(),
                        comment_side: SplitSide::A,
                        datum_side: SplitSide::A,
                    };

                    project
                        .split_section_at_station(s, at, &options)
                        .unwrap_or_else(|e| panic!("{what}: {e}"));

                    let (worst, unmatched) = worst_shift(&before, &positions(&project));
                    assert_eq!(unmatched, 0, "{what}");
                    assert!(worst < 1e-6, "{what}: an adjusted station moved {worst} m");
                    let anchor = project.adjustment_anchor_for_tests(s).unwrap();
                    let now = raw(&project, anchor);
                    assert!(
                        (now.0 - anchored_at.0).hypot(now.1 - anchored_at.1) < 1e-6,
                        "{what}: the anchor moved to {anchor:?}"
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 72);
}

/// A held gauge names its station by shots, so it follows the station through
/// a later reverse of the section that holds it, and a later combine that
/// moves it again; and a held gauge whose shot is gone is pruned.
#[test]
fn a_held_gauge_follows_its_station_and_a_dangling_one_is_pruned() {
    let (mut project, k, o) = combine_shape(
        [Role::Other, Role::Third, Role::Kept],
        true,
        true,
        (false, false),
    );
    project.combine_sections(k, o, &combine_options()).unwrap();
    assert_eq!(
        project.held_gauges.len(),
        1,
        "T anchors, K holds the station"
    );
    let held_at = raw(&project, project.adjustment_anchor_for_tests(k).unwrap());
    let before = positions(&project);

    assert!(project.reverse_section(k, false));
    let now = raw(&project, project.adjustment_anchor_for_tests(k).unwrap());
    assert!((now.0 - held_at.0).hypot(now.1 - held_at.1) < 1e-9);
    let (worst, unmatched) = worst_shift(&before, &positions(&project));
    assert_eq!(unmatched, 0);
    assert!(
        worst < 1e-6,
        "a reverse moved the adjusted survey {worst} m"
    );

    // Drop the shots the entry names: it names nothing and is pruned.
    let held = project.held_gauges[0].clone();
    project
        .get_section_mut(k)
        .unwrap()
        .shots
        .retain(|s| s.uuid.as_deref() != Some(&held.shot));
    assert!(!project.held_gauge_resolves(&held));
    project.prune_held_gauges();
    assert!(project.held_gauges.is_empty());
}

/// Horizontal distance between two stations in a station list.
fn gap_in(stations: &[crate::Station], a: StationRef, b: StationRef) -> f64 {
    let at = |r: StationRef| {
        let s = stations.iter().find(|s| s.station_ref == r).unwrap();
        (s.coordinates.x, s.coordinates.y)
    };
    let (pa, pb) = (at(a), at(b));
    (pa.0 - pb.0).hypot(pa.1 - pb.1)
}

/// Issue #737's synthetic test. Creation now refuses a closure across one shot,
/// so it is planted with `close()`, the shape a file written before the
/// refusal still carries; the solve must skip it and keep the real loop closed.
#[test]
fn a_closure_across_one_shot_does_not_fail_the_component() {
    let mut p = Project::new("F2".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let b = add(
        &mut p,
        "B",
        vec![shot(0.0, 10.0), shot(90.0, 20.5), shot(180.0, 10.0)],
    );
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    let (b3, a2) = (StationRef::new(b, 3), StationRef::new(a, 2));
    p.create_loop_closure(b3, a2, ClosureStrategy::Batch)
        .unwrap();
    let config = ClosureAdjustmentConfig::default();
    let before = gap_in(&p.compute_adjusted_station_coordinates(&config), b3, a2);
    // Merge tool: A:1 then A:2 -> Create Loop Closure is refused now ...
    assert!(p
        .create_loop_closure(StationRef::new(a, 1), a2, ClosureStrategy::Batch)
        .is_err());
    // ... so plant the closure across one shot directly.
    close(&mut p, StationRef::new(a, 1), a2);
    let mut working = p.clone();
    let anchor = working.adjustment_anchor_for_tests(a).unwrap();
    let solve =
        working.adjust_loop_closures(anchor, ClosureStrategy::LeastSquaresWeighted, &config);
    let after = gap_in(&p.compute_adjusted_station_coordinates(&config), b3, a2);
    assert!(solve.is_ok(), "{solve:?}");
    assert!(before < 1e-6, "loop B:3-A:2 closed before: {before}");
    assert!(after < 1e-6, "loop B:3-A:2 stays closed: {after}");
}
