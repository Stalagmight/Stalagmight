//! Detach / unmerge (plan 5.2): one line gets its own copy of a junction, and
//! what its member of the old merge owned (`via` hints) goes with it.

use crate::{DatumRole, StationRef};

use super::ops_test_helpers::{
    assert_shifted, assert_still, assert_valid, branch, free_line, gps_at, ok, origin, run, xy,
    xy_all,
};
use super::test_helpers::{at, project};
use super::{AnchorReport, ClosureEnd, NameAnswers, OpError};

const TOL: f64 = 1e-9;

#[test]
fn detaching_after_a_merge_gives_two_pieces_where_they_were() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_keeping_stationary(a2, b0));
    let merged = xy_all(&p);

    // Detaching B, the kept station's own line: a record names A, so B keeps
    // the junction and A gets its own station back (RS 4.2 case 2).
    let out = ok(p.unmerge_station_pair(
        StationRef::new(a, 2),
        StationRef::new(b, 0),
        NameAnswers::Renames(&[]),
        None,
    ));
    assert_valid(&p);
    let report = out.value;
    assert_eq!((report.line, report.station), (b, b0));
    assert!(report.kept && report.restored);
    let back = report.new_station;
    assert_eq!(back, a2);
    assert_eq!(at(&p, b, 0), b0, "b keeps the junction's id");
    assert_eq!(at(&p, a, 2), a2, "a's slot got its own station back");
    assert_eq!(p.stations[&a2].name, "2");
    assert!(p.stations[&a2].comment.is_none() && p.stations[&a2].archived.is_empty());
    assert!(p.stations[&b0].archived.is_empty());
    assert!(!p.topology().connected(b0, a2));
    assert_eq!(p.placements.len(), 2);
    let after = xy_all(&p);
    assert_still(&merged, &after, TOL);
    assert_eq!(
        after[&a2], merged[&b0],
        "the station given back sits where the junction was"
    );
}

#[test]
fn the_pair_form_needs_one_station_on_two_lines() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let a1 = at(&p, a, 1);
    let b = branch(&mut p, a1, run(0.0, 1, 5.0));
    let before = p.clone();
    assert_eq!(
        p.unmerge_station_pair(
            StationRef::new(a, 0),
            StationRef::new(b, 0),
            NameAnswers::Renames(&[]),
            None
        ),
        Err(OpError::NotDirectlyMerged)
    );
    assert_eq!(
        p.unmerge_station_pair(
            StationRef::new(a, 1),
            StationRef::new(a, 1),
            NameAnswers::Renames(&[]),
            None
        ),
        Err(OpError::NotDirectlyMerged)
    );
    assert_eq!(
        p.unmerge_station_pair(
            StationRef::new(a, 9),
            StationRef::new(b, 0),
            NameAnswers::Renames(&[]),
            None
        ),
        Err(OpError::UnknownSlot(StationRef::new(a, 9)))
    );
    assert_eq!(
        p.detach_line(at(&p, a, 0), a, &[], None),
        Err(OpError::NotDirectlyMerged)
    );
    assert_eq!(p, before);
}

#[test]
fn the_station_form_detaches_the_slots_line_and_is_a_no_op_off_a_junction() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let a1 = at(&p, a, 1);
    let b = branch(&mut p, a1, run(0.0, 2, 5.0));
    let before = p.clone();
    let out = ok(p.unmerge_station(StationRef::new(a, 0), NameAnswers::Renames(&[]), None));
    assert_eq!(out.value, None);
    assert_eq!(out.anchors, AnchorReport::default());
    assert_eq!(p, before);

    let junction = at(&p, a, 1);
    let out = ok(p.unmerge_station(StationRef::new(b, 0), NameAnswers::Renames(&[]), None));
    assert_valid(&p);
    let report = out.value.unwrap();
    assert_eq!((report.line, report.station), (b, junction));
    assert_eq!(at(&p, a, 1), junction);
    assert_ne!(at(&p, b, 0), junction);
}

#[test]
fn a_hub_of_three_lines_detaches_the_chosen_line_only() {
    // Declared change (plan 5.2): the other two lines stay joined.
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let hub = at(&p, a, 1);
    let b = branch(&mut p, hub, run(0.0, 2, 5.0));
    let c = branch(&mut p, hub, run(180.0, 2, 5.0));
    let before = xy_all(&p);
    ok(p.detach_line(hub, b, &[], None));
    assert_valid(&p);
    assert_eq!(p.topology().components().len(), 2);
    assert!(p.topology().connected(at(&p, a, 0), at(&p, c, 2)));
    assert!(!p.topology().connected(at(&p, a, 0), at(&p, b, 2)));
    assert_eq!(p.topology().lines_of(hub), vec![a, c]);
    assert_still(&before, &xy_all(&p), TOL);
}

#[test]
fn the_lines_hand_move_goes_to_the_new_station() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let hub = at(&p, a, 1);
    let b = branch(&mut p, hub, run(0.0, 2, 5.0));
    p.line_mut(b)
        .unwrap()
        .hand_moves
        .insert(hub, [0.5, 0.0, 0.0]);
    p.line_mut(a)
        .unwrap()
        .hand_moves
        .insert(hub, [0.0, 0.25, 0.0]);
    let before = xy_all(&p);
    let report = ok(p.detach_line(hub, b, &[], None)).value;
    assert_valid(&p);
    assert!(report.moved_hand_move);
    let fresh = report.new_station;
    assert_eq!(
        p.line(b).unwrap().hand_moves.get(&fresh),
        Some(&[0.5, 0.0, 0.0])
    );
    assert_eq!(
        p.line(a).unwrap().hand_moves.get(&hub),
        Some(&[0.0, 0.25, 0.0])
    );
    assert_still(&before, &xy_all(&p), TOL);
}

#[test]
fn unmerging_places_the_detached_piece_on_its_own_gps() {
    // Today's #741 case: A placed (frame), B placed by its own GPS 30 m east,
    // merged (B moves west, its datum a fix), then unmerged: B's fix is
    // promoted and B goes back onto its GPS; A never moves.
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_keeping_stationary(a2, b0));
    let merged = xy_all(&p);
    let b_stations: Vec<_> = p.line_stations(b).unwrap()[1..].to_vec();
    assert!((xy(&p, b_stations[0])[0]).abs() < 1e-6, "B moved onto A");

    let out = ok(p.unmerge_station_pair(
        StationRef::new(a, 2),
        StationRef::new(b, 0),
        NameAnswers::Renames(&[]),
        None,
    ));
    assert_valid(&p);
    assert_eq!(out.value.moved_datums, vec![1]);
    assert_eq!(
        p.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    assert_eq!(out.anchors.placed_on_gps, vec![1]);
    let after = xy_all(&p);
    assert_shifted(&merged, &after, 1e-6, |s| {
        if b_stations.contains(&s) {
            [30.0, 0.0]
        } else {
            [0.0, 0.0]
        }
    });
    let b0_now = at(&p, b, 0);
    let pos = xy(&p, b0_now);
    assert!((pos[0] - 30.0).abs() < 0.01 && (pos[1] - 100.0).abs() < 0.01);
}

#[test]
fn a_placement_datum_attached_via_the_detached_line_goes_with_it() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let hub = at(&p, a, 1);
    let b = branch(&mut p, hub, run(0.0, 2, 5.0));
    ok(p.set_datum_at(StationRef::new(b, 0), origin(), None));
    let before = xy_all(&p);
    let out = ok(p.detach_line(hub, b, &[], None));
    assert_valid(&p);
    let fresh = out.value.new_station;
    assert_eq!(p.datums[0].station, fresh);
    assert_eq!(p.datums[0].via, Some(b));
    assert!(
        !out.anchors.frame_changed,
        "the frame datum is the same datum"
    );
    assert_eq!(
        p.placements.len(),
        1,
        "a's piece gets a placement where it is"
    );
    assert_still(&before, &xy_all(&p), TOL);
}

#[test]
fn closure_ends_attached_via_the_line_move_and_others_stay() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let hub = at(&p, a, 1);
    let b = branch(&mut p, hub, run(0.0, 2, 5.0));
    let (a3, b2) = (at(&p, a, 3), at(&p, b, 2));
    // Two closures from the hub: one attached via a, one via b. Each closes a
    // loop inside the piece now.
    let via_a = ok(p.create_closure(
        hub,
        Some(a),
        b2,
        Some(b),
        super::ClosureStrategy::Batch,
        None,
        &[],
    ))
    .value
    .closure
    .id;
    let via_b = ok(p.create_closure(
        hub,
        Some(b),
        a3,
        Some(a),
        super::ClosureStrategy::Batch,
        None,
        &[],
    ))
    .value
    .closure
    .id;
    let out = ok(p.detach_line(hub, b, &[], None));
    assert_valid(&p);
    assert_eq!(out.value.moved_closure_ends, vec![(via_b, ClosureEnd::A)]);
    // via_a now joins the hub (a's piece) to b2 (b's piece): a sole bridge,
    // promoted by the sweep, which merges the hub onto b2 (neither piece is
    // placed, so end a's piece moves). via_b then closes a loop and stays.
    assert_eq!(out.sweep.promoted.len(), 1);
    assert_eq!(out.sweep.promoted[0].closure.id, via_a);
    assert!(p.closures.iter().any(|c| c.id == via_b));
    assert!(p.topology().connected(at(&p, a, 0), at(&p, b, 0)));
}

/// Review finding UX-4, RS 4.2: merging by mistake and detaching gives the
/// line back as it was. A:2 merged into B:0 (A moves onto B; B:0 is kept,
/// A:2 is a record naming A). Detaching A gives A's slot that identity back
/// byte for byte (id, name, comment, external id), and the record leaves the
/// junction; detaching B (the kept station's own line) keeps the junction on
/// B and gives A its station back instead (case 2). The previews say the
/// same; no id is minted.
#[test]
fn detaching_the_line_a_record_names_gives_it_its_station_back() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    p.station_mut(a2).unwrap().comment = Some("A's note".to_string());
    p.station_mut(a2).unwrap().external = Some("A2-ext".to_string());
    let identity = p.stations[&a2].clone();
    ok(p.merge_keeping_stationary(a2, b0));
    assert_eq!(p.stations[&b0].archived[0].lines, vec![a]);
    let merged = p.clone();

    let preview = p.detach_naming(b0, a, None).unwrap();
    assert_eq!(
        (preview.id, preview.name.as_str(), preview.restored),
        (a2, "2", true)
    );
    assert_eq!(preview.comment.as_deref(), Some("A's note"));
    assert!(preview.together.is_empty() && preview.collisions.is_empty());
    let out = ok(p.detach_line(b0, a, &[], None));
    assert_valid(&p);
    assert!(out.value.restored);
    assert_eq!(out.value.new_station, a2);
    assert_eq!(at(&p, a, 2), a2, "A's slot is A:2 again");
    assert_eq!(p.stations[&a2], identity, "byte for byte");
    assert!(
        p.stations[&b0].archived.is_empty(),
        "the record left the junction"
    );
    assert_eq!(p.counters.next_station_id, merged.counters.next_station_id);

    // The kept station's own line: records name every other line, so that
    // line gets its station back and B keeps the junction.
    let mut q = merged.clone();
    let preview = q.detach_naming(b0, b, None).unwrap();
    assert!(preview.restored && preview.kept);
    assert_eq!((preview.id, preview.name.as_str()), (a2, "2"));
    let out = ok(q.detach_line(b0, b, &[], None));
    assert_valid(&q);
    assert!(out.value.restored && out.value.kept);
    assert_eq!(out.value.new_station, a2);
    assert_eq!((at(&q, a, 2), at(&q, b, 0)), (a2, b0));
    assert_eq!(q.stations[&a2], identity);
    assert_eq!(q.counters.next_station_id, merged.counters.next_station_id);
}

/// A junction of a third line that is native there too (a line created at
/// the station): detaching the kept station's line leaves a native line, so
/// the detach mints a fresh station named as the junction (RS 4.2 case 3).
#[test]
fn detaching_a_line_mints_when_another_line_is_native_too() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_keeping_stationary(a2, b0));
    let c = branch(&mut p, b0, run(180.0, 1, 4.0));
    assert_valid(&p);
    let counter = p.counters.next_station_id;
    let out = ok(p.detach_line(b0, b, &[], None));
    assert_valid(&p);
    assert!(!out.value.kept && !out.value.restored);
    assert_eq!(out.value.new_station, counter);
    assert_eq!((at(&p, a, 2), at(&p, c, 0)), (b0, b0));
    assert_eq!(p.shown_name(b0), Some("0"));
    assert_eq!(p.stations[&counter].name, "0");
    assert!(p.stations[&counter].comment.is_none());
}

/// RS 4.2: an archived station's name counts on no line, so A's station 1
/// may take it while A shows the kept junction. Detach then shows it anew on
/// A next to that name: with no answer it refuses (N8), the preview offers
/// rule L's letter (the restored station is new on A, A:1 is not, each shown
/// on one line), a rename answers it, and the no-dialog form letters it.
/// Injection: count record names in `check_names` and the rename of A:1 is
/// refused.
#[test]
fn an_archived_name_is_free_and_its_restore_asks_on_a_collision() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a1, a2, b0) = (at(&p, a, 1), at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_keeping_stationary(a2, b0));
    assert!(p.conflicts_with_name(a1, "2").is_empty());
    p.station_mut(a1).unwrap().name = "2".to_string();
    assert_valid(&p);
    let before = p.clone();
    assert_eq!(
        p.detach_line(b0, a, &[], None).err(),
        Some(OpError::NameTaken {
            line: a,
            name: "2".to_string()
        })
    );
    assert_eq!(p, before);
    let naming = p.detach_naming(b0, a, None).unwrap();
    assert_eq!(naming.collisions.len(), 1);
    let letter = &naming.collisions[0];
    assert_eq!((letter.station, letter.line, letter.other), (a2, a, a1));
    assert_eq!(letter.to, "2a");
    let mut auto = p.clone();
    ok(p.detach_line(b0, a, &[(a2, "2b".to_string())], None));
    assert_valid(&p);
    assert_eq!(p.stations[&a2].name, "2b");
    ok(auto.detach_line_auto(b0, a, None));
    assert_eq!(auto.stations[&a2].name, "2a");
    assert_eq!(auto.stations[&a1].name, "2", "the older station keeps it");
}

/// Demo finding V-1, RS 4.2 case 2: on a hub where two lines were merged
/// onto a third line's station, the preview says what EVERY line ends up
/// with, and the detach then does exactly that. Keeping the owner's line,
/// the other two come off together onto one node, whose identity is the
/// record `rest_keep` picks (default rule K: equal lengths, the lowest
/// Station ID, Far Room's); the other record stays archived in it.
/// Detaching the second line alone gives it its own station back.
/// Injection: read every line's station off the junction before the detach
/// and the owner's preview gives both others the junction's id.
#[test]
fn the_detach_preview_says_what_every_line_of_a_hub_ends_up_with() {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let far = free_line(&mut p, [100.0, 50.0], run(0.0, 1, 5.0));
    let up = free_line(&mut p, [-100.0, 50.0], run(0.0, 1, 5.0));
    let (hub, far0, up0) = (at(&p, main, 2), at(&p, far, 0), at(&p, up, 0));
    ok(p.merge_keeping_stationary(far0, hub));
    ok(p.merge_keeping_stationary(up0, hub));
    assert_valid(&p);
    let merged = p.clone();
    let outcome = |naming: &super::DetachNaming, line| {
        naming
            .lines
            .iter()
            .find(|o| o.line == line)
            .cloned()
            .unwrap()
    };

    // Keep Main Line: Far Room and Upstream come off together.
    let naming = p.detach_naming(hub, main, None).unwrap();
    assert!(naming.kept);
    assert_eq!((naming.id, naming.name.as_str()), (far0, "0"));
    assert_eq!(naming.rest_choices, vec![far0, up0]);
    assert_eq!(naming.rest_default, Some(far0));
    assert_eq!(naming.lines.len(), 3);
    let m = outcome(&naming, main);
    assert_eq!((m.station, m.comes_off), (hub, false));
    assert!(m.joined_with.is_empty());
    let f = outcome(&naming, far);
    assert_eq!((f.station, f.comes_off), (far0, true));
    assert_eq!(f.joined_with, vec![up]);
    let u = outcome(&naming, up);
    assert_eq!(
        (u.station, u.name.as_str(), u.comes_off),
        (far0, "0", true),
        "Upstream shares Far Room's station, its own archived there"
    );
    assert_eq!(u.joined_with, vec![far]);
    ok(p.detach_line(hub, main, &[], None));
    assert_valid(&p);
    for o in &naming.lines {
        let slot = p.topology().slot_on(o.station, o.line);
        assert!(
            slot.is_some(),
            "{o:?}: the line is on the station the preview names"
        );
        assert_eq!(p.shown_name(o.station), Some(o.name.as_str()));
    }
    assert_eq!(p.stations[&far0].archived[0].id, up0);
    assert_eq!(p.stations[&far0].archived[0].lines, vec![up]);

    // The other record picked instead.
    let mut r = merged.clone();
    let out = ok(r.detach_line(hub, main, &[], Some(up0)));
    assert_eq!(out.value.new_station, up0);
    assert_eq!(r.stations[&up0].archived[0].id, far0);
    assert_valid(&r);
    assert_eq!(
        merged.clone().detach_line(hub, main, &[], Some(hub)).err(),
        Some(OpError::InvalidKeep(hub))
    );

    // Detach Upstream alone: it gets its own station back; the others stay.
    let mut q = merged;
    let naming = q.detach_naming(hub, up, None).unwrap();
    assert!(!naming.kept && naming.restored);
    assert_eq!(naming.id, up0);
    let u = outcome(&naming, up);
    assert_eq!((u.station, u.comes_off), (up0, true));
    assert!(u.joined_with.is_empty());
    let m = outcome(&naming, main);
    assert_eq!(
        (m.station, m.comes_off, m.joined_with),
        (hub, false, vec![far])
    );
    let f = outcome(&naming, far);
    assert_eq!((f.station, f.comes_off), (hub, false));
    ok(q.detach_line(hub, up, &[], None));
    assert_eq!(at(&q, up, 0), up0);
    assert_eq!((at(&q, main, 2), at(&q, far, 0)), (hub, hub));
}
