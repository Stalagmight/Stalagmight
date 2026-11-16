//! Combine Sections and snapped closures (#756). A snap from the side that
//! moves into a part a zero datum holds tears because that datum keeps the
//! part where it is, and the plan says so (the zero-datum reason, so the
//! dialog names the section that stays) where it used to give the loop
//! reason. And why the issue's own
//! survey, a snapped part held to the rest by loop closures alone, is no
//! survey the app leaves: a reopen settles it before any combine can see it.

use super::combine_closure_tests::{add, check_combined, dist, options, shot, xy};
use crate::{
    ClosureStrategy, CombineJoinEnds, CombinePlan, GpsCoordinates, LoopClosure, Project,
    SectionEnd, SectionId, StationRef, TornTieKind, TornTieReason, ZeroDatum,
};

/// Move `id` so its station `index` sits exactly on `target`.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64)) {
    let now = xy(project, StationRef::new(id, index));
    project.adjust_section_offset(id, target.0 - now.0, target.1 - now.1);
}

/// A one-shot section from `from` to `to`, placed on `from`.
fn bridge(project: &mut Project, name: &str, from: StationRef, to: StationRef) -> SectionId {
    let (a, b) = (xy(project, from), xy(project, to));
    let azimuth = (b.0 - a.0).atan2(b.1 - a.1).to_degrees().rem_euclid(360.0);
    let id = add(project, name, vec![shot(azimuth, dist(a, b))]);
    place(project, id, 0, a);
    id
}

/// A loop closure as a file stores it, measured off the current geometry.
fn plant(
    project: &mut Project,
    a: StationRef,
    b: StationRef,
    strategy: ClosureStrategy,
    is_aligned: bool,
) {
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    let gap = dist(xy(project, a), xy(project, b));
    project.loop_closures.push(LoopClosure {
        id,
        station_a: project.key_for_ref(a).unwrap(),
        station_b: project.key_for_ref(b).unwrap(),
        path_length: 10.0,
        misclosure_distance: gap,
        error_percentage: gap * 10.0,
        horizontal_misclosure: gap,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned,
    });
}

/// What a reopen's settling leaves of `project`: its merges, and each loop
/// closure as (id, strategy, snapped).
fn settled(project: &Project) -> (usize, Vec<(u64, ClosureStrategy, bool)>) {
    let mut reopened = project.clone();
    reopened.redrive_offsets_after_load();
    let closures = reopened
        .loop_closures
        .iter()
        .map(|c| (c.id, c.strategy, c.is_aligned))
        .collect();
    (reopened.station_merges.len(), closures)
}

/// Does a reopen leave `project` as it is (every closure closing a true loop,
/// none a sole bridge)?
fn reopen_settles_nothing(project: &Project) -> bool {
    let closures = project
        .loop_closures
        .iter()
        .map(|c| (c.id, c.strategy, c.is_aligned))
        .collect();
    settled(project) == (project.station_merges.len(), closures)
}

/// The issue's survey (#756): K's end and O's start 5 mm apart, tied by a
/// loop closure; N merged at K's start, M at O's end; Z snapped to O's
/// station 1 and merged to W; W tied to M by an ordinary loop closure. No
/// merge ties Z or W to anything else, and K's side (K, N) is tied to O's
/// (O, M) by the join closure alone: planted as a file can carry them (the
/// app snaps only stations already connected).
fn issue_shape() -> Project {
    let mut project = Project::new("Strand".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(80.0, 5.0)]);
    let o = add(&mut project, "O", vec![shot(0.0, 4.0), shot(300.0, 6.0)]);
    let n = add(&mut project, "N", vec![shot(0.0, 3.0)]);
    let m = add(&mut project, "M", vec![shot(90.0, 3.0)]);
    let z = add(&mut project, "Z", vec![shot(180.0, 5.0)]);
    let w = add(&mut project, "W", vec![shot(90.0, 4.0)]);
    project.set_section_offset(k, (100.0, -40.0));
    let k2 = xy(&project, StationRef::new(k, 2));
    place(&mut project, o, 0, (k2.0 + 0.004, k2.1 - 0.003));
    let k0 = xy(&project, StationRef::new(k, 0));
    place(&mut project, n, 0, k0);
    project
        .merge_stations(StationRef::new(k, 0), StationRef::new(n, 0))
        .unwrap();
    let o2 = xy(&project, StationRef::new(o, 2));
    place(&mut project, m, 0, o2);
    project
        .merge_stations(StationRef::new(o, 2), StationRef::new(m, 0))
        .unwrap();
    let o1 = xy(&project, StationRef::new(o, 1));
    place(&mut project, z, 0, o1);
    let z1 = xy(&project, StationRef::new(z, 1));
    place(&mut project, w, 0, z1);
    project
        .merge_stations(StationRef::new(w, 0), StationRef::new(z, 1))
        .unwrap();
    plant(
        &mut project,
        StationRef::new(o, 1),
        StationRef::new(z, 0),
        ClosureStrategy::Move,
        true,
    );
    plant(
        &mut project,
        StationRef::new(w, 1),
        StationRef::new(m, 1),
        ClosureStrategy::Batch,
        false,
    );
    plant(
        &mut project,
        StationRef::new(k, 2),
        StationRef::new(o, 0),
        ClosureStrategy::Batch,
        false,
    );
    project
}

/// Why #756 is not reachable in the app: its survey is not one the app
/// leaves. The snap and the join closure each join two parts no merge ties
/// (Z and W to O; K's side to O's), and a reopen (`redrive_offsets_after_load`,
/// the orphan-closure sweep first) makes both merges, after which W's
/// closure to M closes a true loop. So a project the app opens never holds
/// the issue's state, Z and W tied to the rest by loop closures alone.
#[test]
fn the_issue_survey_is_one_a_reopen_already_settles() {
    let project = issue_shape();
    // The snap, W's closure to M, the join closure, in that order.
    let w_to_m = project.loop_closures[1].id;
    assert!(!reopen_settles_nothing(&project), "planted unsettled");
    let (merges, closures) = settled(&project);
    assert_eq!(
        merges,
        project.station_merges.len() + 2,
        "the snap and the join closure become merges"
    );
    assert_eq!(
        closures,
        vec![(w_to_m, ClosureStrategy::Batch, false)],
        "W's closure to M stays"
    );
    let mut reopened = project;
    reopened.redrive_offsets_after_load();
    assert!(reopen_settles_nothing(&reopened), "a second reopen");
}

/// The torn ties of `plan`, as (kind, ends sorted, reason, held end), merges
/// first. The held end is the one the dialog names for a tie torn for a zero
/// datum: the end on a section that does not move.
fn torn(
    plan: &CombinePlan,
) -> Vec<(
    TornTieKind,
    [StationRef; 2],
    TornTieReason,
    Option<StationRef>,
)> {
    let mut torn: Vec<_> = plan
        .torn_ties
        .iter()
        .map(|t| {
            let mut ends = [t.station_a, t.station_b];
            ends.sort_by_key(|at| (at.section_id, at.station_index));
            let held = (t.reason == TornTieReason::ZeroDatum)
                .then(|| {
                    [t.station_a, t.station_b]
                        .into_iter()
                        .find(|at| !plan.moved_sections.contains(&at.section_id))
                })
                .flatten();
            (t.kind, ends, t.reason, held)
        })
        .collect();
    torn.sort_by_key(|(kind, ..)| *kind == TornTieKind::SnappedClosure);
    torn
}

/// Both sides held by zero datums (N's on K's side, D's on O's), K's end and
/// O's start 5 mm apart and tied by a loop closure (two anchored surveys may
/// be tied so). D runs from O's end, merged there, back to O's station 1,
/// snapped there. Keeping K, O moves: it tears its merge to D for D's datum,
/// and the snap from O into D's part tears too. It tears because D's datum
/// holds that part where it is, so its reason is the datum and the dialog
/// names D as the section that stays; it used to give the loop reason, and
/// the dialog then said the loop's misclosure moves to it. Keeping O, K moves
/// and tears its merge to N; the snap stays inside O's side.
#[test]
fn a_snap_into_a_part_a_zero_datum_holds_tears_for_the_datum() {
    let mut project = Project::new("Held".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(80.0, 5.0)]);
    let o = add(&mut project, "O", vec![shot(0.0, 4.0), shot(300.0, 6.0)]);
    let n = add(&mut project, "N", vec![shot(0.0, 3.0)]);
    project.set_section_offset(k, (100.0, -40.0));
    let k2 = xy(&project, StationRef::new(k, 2));
    place(&mut project, o, 0, (k2.0 + 0.004, k2.1 - 0.003));
    let k0 = xy(&project, StationRef::new(k, 0));
    place(&mut project, n, 0, k0);
    project
        .merge_stations(StationRef::new(k, 0), StationRef::new(n, 0))
        .unwrap();
    let d = bridge(
        &mut project,
        "D",
        StationRef::new(o, 2),
        StationRef::new(o, 1),
    );
    project
        .merge_stations(StationRef::new(o, 2), StationRef::new(d, 0))
        .unwrap();
    for at in [StationRef::new(n, 1), StationRef::new(d, 1)] {
        project.zero_datums.push(ZeroDatum {
            station_ref: project.key_for_ref(at).unwrap(),
            gps: GpsCoordinates {
                latitude: 20.0,
                longitude: -87.0,
                depth_reference: 0.0,
            },
            accuracy_m: None,
            entrance: None,
        });
    }
    plant(
        &mut project,
        StationRef::new(k, 2),
        StationRef::new(o, 0),
        ClosureStrategy::Batch,
        false,
    );
    let snapped = project
        .create_loop_closure(
            StationRef::new(o, 1),
            StationRef::new(d, 1),
            ClosureStrategy::Move,
        )
        .unwrap();
    assert!(snapped.is_aligned, "setup: a snap");
    assert!(reopen_settles_nothing(&project), "a survey the app leaves");

    let keep_k = CombineJoinEnds {
        kept: SectionEnd::End,
        other: SectionEnd::Start,
    };
    let c = project
        .combine_candidate_joining(k, o, Some(keep_k))
        .unwrap();
    assert_eq!(c.blocked_reason, None);
    let plan = if c.section_a == k {
        &c.plan_keeping_a
    } else {
        &c.plan_keeping_b
    };
    assert_eq!(plan.moving_section, Some(o));
    assert_eq!(plan.moved_sections, vec![o]);
    assert_eq!(
        torn(plan),
        vec![
            (
                TornTieKind::Merge,
                [StationRef::new(o, 2), StationRef::new(d, 0)],
                TornTieReason::ZeroDatum,
                Some(StationRef::new(d, 0)),
            ),
            (
                TornTieKind::SnappedClosure,
                [StationRef::new(o, 1), StationRef::new(d, 1)],
                TornTieReason::ZeroDatum,
                Some(StationRef::new(d, 1)),
            ),
        ]
    );
    let mut after = project.clone();
    after
        .combine_sections(k, o, &options(Some(keep_k)))
        .unwrap();
    check_combined(&project, &after, &c, k, "keep K", &[]);
    assert!(reopen_settles_nothing(&after), "a reopen settles nothing");

    let keep_o = CombineJoinEnds {
        kept: SectionEnd::Start,
        other: SectionEnd::End,
    };
    let c = project
        .combine_candidate_joining(o, k, Some(keep_o))
        .unwrap();
    let plan = if c.section_a == o {
        &c.plan_keeping_a
    } else {
        &c.plan_keeping_b
    };
    assert_eq!(plan.moving_section, Some(k));
    assert_eq!(
        torn(plan),
        vec![(
            TornTieKind::Merge,
            [StationRef::new(k, 0), StationRef::new(n, 0)],
            TornTieReason::ZeroDatum,
            Some(StationRef::new(n, 0)),
        )]
    );
    let mut after = project.clone();
    after
        .combine_sections(o, k, &options(Some(keep_o)))
        .unwrap();
    check_combined(&project, &after, &c, o, "keep O", &[]);
}
