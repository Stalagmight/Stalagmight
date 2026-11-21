//! Detach / unmerge (plan 5.2): one line gets its own copy of a junction, and
//! what its member of the old merge owned (`via` hints) goes with it.

use crate::{DatumRole, StationRef};

use super::ops_test_helpers::{
    assert_shifted, assert_still, assert_valid, branch, free_line, gps_at, ok, origin, run, xy,
    xy_all,
};
use super::test_helpers::{at, project};
use super::{AnchorReport, ClosureEnd, OpError};

const TOL: f64 = 1e-9;

#[test]
fn detaching_after_a_merge_gives_two_pieces_where_they_were() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_stations(a2, b0));
    let merged = xy_all(&p);

    // Detaching B, the survivor's own line: A knows the junction by its
    // alias only, so B keeps it and A gets its own station back (I-1).
    let out = ok(p.unmerge_station_pair(StationRef::new(a, 2), StationRef::new(b, 0)));
    assert_valid(&p);
    let report = out.value;
    assert_eq!((report.line, report.station), (b, b0));
    assert!(report.kept && report.restored_alias);
    let back = report.new_station;
    assert_eq!(back, a2);
    assert_eq!(at(&p, b, 0), b0, "b keeps the junction's id");
    assert_eq!(at(&p, a, 2), a2, "a's slot got its own station back");
    assert_eq!(p.stations[&a2].name, "2");
    assert!(p.stations[&a2].comment.is_none() && p.stations[&a2].aliases.is_empty());
    assert!(p.stations[&b0].aliases.is_empty());
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
        p.unmerge_station_pair(StationRef::new(a, 0), StationRef::new(b, 0)),
        Err(OpError::NotDirectlyMerged)
    );
    assert_eq!(
        p.unmerge_station_pair(StationRef::new(a, 1), StationRef::new(a, 1)),
        Err(OpError::NotDirectlyMerged)
    );
    assert_eq!(
        p.unmerge_station_pair(StationRef::new(a, 9), StationRef::new(b, 0)),
        Err(OpError::UnknownSlot(StationRef::new(a, 9)))
    );
    assert_eq!(
        p.detach_line(at(&p, a, 0), a),
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
    let out = ok(p.unmerge_station(StationRef::new(a, 0)));
    assert_eq!(out.value, None);
    assert_eq!(out.anchors, AnchorReport::default());
    assert_eq!(p, before);

    let junction = at(&p, a, 1);
    let out = ok(p.unmerge_station(StationRef::new(b, 0)));
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
    ok(p.detach_line(hub, b));
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
    let report = ok(p.detach_line(hub, b)).value;
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
    ok(p.merge_stations(a2, b0));
    let merged = xy_all(&p);
    let b_stations: Vec<_> = p.line_stations(b).unwrap()[1..].to_vec();
    assert!((xy(&p, b_stations[0])[0]).abs() < 1e-6, "B moved onto A");

    let out = ok(p.unmerge_station_pair(StationRef::new(a, 2), StationRef::new(b, 0)));
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
    let out = ok(p.detach_line(hub, b));
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
    let via_a = ok(p.create_closure(hub, Some(a), b2, Some(b), super::ClosureStrategy::Batch))
        .value
        .closure
        .id;
    let via_b = ok(p.create_closure(hub, Some(b), a3, Some(a), super::ClosureStrategy::Batch))
        .value
        .closure
        .id;
    let out = ok(p.detach_line(hub, b));
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

/// Review finding UX-4: merging by mistake and detaching gives the line back
/// as it was. A:2 merged into B:0 (A moves onto B; B:0 survives, A:2's id and
/// name are an alias naming A). Detaching A gives A's slot that id and name
/// back, and the alias leaves the junction; detaching B (the survivor's own
/// line) mints a fresh station, as before. The preview says the same.
#[test]
fn detaching_the_line_an_alias_names_gives_it_its_id_and_name_back() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let a2_name = p.stations[&a2].name.clone();
    ok(p.merge_stations(a2, b0));
    assert_eq!(p.stations[&b0].aliases[0].line, Some(a));
    let merged = p.clone();

    let preview = p.detach_naming(b0, a).unwrap();
    assert_eq!(
        (preview.id, preview.name.as_str(), preview.restored_alias),
        (a2, a2_name.as_str(), true)
    );
    let out = ok(p.detach_line(b0, a));
    assert_valid(&p);
    assert!(out.value.restored_alias);
    assert_eq!(out.value.new_station, a2);
    assert_eq!(at(&p, a, 2), a2, "A's slot is A:2 again");
    assert_eq!(p.stations[&a2].name, a2_name);
    assert!(
        p.stations[&b0].aliases.is_empty(),
        "the alias left the junction"
    );
    assert_eq!(p.counters.next_station_id, merged.counters.next_station_id);

    // The survivor's own line: the only other line knows the junction by
    // its alias, so that line gets it back and B keeps the junction (I-1);
    // no id is minted. The preview says so.
    let mut q = merged.clone();
    let preview = q.detach_naming(b0, b).unwrap();
    assert!(preview.restored_alias && preview.kept);
    assert_eq!((preview.id, preview.name.as_str()), (a2, a2_name.as_str()));
    let out = ok(q.detach_line(b0, b));
    assert_valid(&q);
    assert!(out.value.restored_alias && out.value.kept);
    assert_eq!(out.value.new_station, a2);
    assert_eq!((at(&q, a, 2), at(&q, b, 0)), (a2, b0));
    assert_eq!(q.stations[&a2].name, a2_name);
    assert_eq!(q.counters.next_station_id, merged.counters.next_station_id);
}

/// A junction of a third line that knows it by its own name: detaching the
/// survivor's line leaves a line showing the station's own name, so the
/// detach mints a fresh station for the detached line as before.
#[test]
fn detaching_a_line_mints_when_another_line_shows_the_junctions_own_name() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_stations(a2, b0));
    let c = branch(&mut p, b0, run(180.0, 1, 4.0));
    assert_valid(&p);
    let counter = p.counters.next_station_id;
    let out = ok(p.detach_line(b0, b));
    assert_valid(&p);
    assert!(!out.value.kept && !out.value.restored_alias);
    assert_eq!(out.value.new_station, counter);
    assert_eq!((at(&p, a, 2), at(&p, c, 0)), (b0, b0));
    assert_eq!(p.name_on(b0, a), Some("2"));
    assert_eq!(p.stations[&counter].name, "0");
}

/// The name a line shows a junction by is held on that line like any other
/// (invariant 8, I-3), so a Detach always gives the alias's name back with
/// its id, and a rename that would take it on that line is refused.
/// Injection: drop the alias's name from `check_names` (the old exemption)
/// and the direct rename below validates.
#[test]
fn the_alias_name_is_held_on_its_line_so_detach_gives_it_back() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a1, a2, b0) = (at(&p, a, 1), at(&p, a, 2), at(&p, b, 0));
    let a2_name = p.stations[&a2].name.clone();
    ok(p.merge_stations(a2, b0));
    assert_eq!(
        p.conflicts_with_name_on(a1, a, &a2_name),
        vec![super::NameConflict { line: a, other: b0 }]
    );
    let mut renamed = p.clone();
    renamed.station_mut(a1).unwrap().name = a2_name.clone();
    assert!(!renamed.is_valid(), "A would show {a2_name} twice");
    let out = ok(p.detach_line(b0, a));
    assert_valid(&p);
    assert_eq!(out.value.new_station, a2, "the id comes back");
    assert_eq!(p.stations[&a2].name, a2_name, "and the name");
}

/// Demo finding V-1: on a hub where two lines were merged onto a third line's
/// station, the preview says what EVERY line ends up with, and the detach then
/// does exactly that. Keeping the owner's line, the other two come off
/// together and stay joined on one station, the first one's (19 in the demo);
/// the second keeps its own Station ID (22) only as an alias, and the preview
/// says so (`alias_id`) instead of claiming it gets its own station back.
/// Detaching the second line alone gives it its own station back. Injection:
/// read every line's station off the junction before the detach and the
/// owner's preview gives both others the junction's id.
#[test]
fn the_detach_preview_says_what_every_line_of_a_hub_ends_up_with() {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let far = free_line(&mut p, [100.0, 50.0], run(0.0, 1, 5.0));
    let up = free_line(&mut p, [-100.0, 50.0], run(0.0, 1, 5.0));
    let (hub, far0, up0) = (at(&p, main, 2), at(&p, far, 0), at(&p, up, 0));
    ok(p.merge_stations(far0, hub));
    ok(p.merge_stations(up0, hub));
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
    let naming = p.detach_naming(hub, main).unwrap();
    assert!(naming.kept);
    assert_eq!((naming.id, naming.name.as_str()), (far0, "0"));
    assert_eq!(naming.lines.len(), 3);
    let m = outcome(&naming, main);
    assert_eq!((m.station, m.comes_off, m.alias_id), (hub, false, None));
    assert!(m.joined_with.is_empty());
    let f = outcome(&naming, far);
    assert_eq!((f.station, f.comes_off, f.alias_id), (far0, true, None));
    assert_eq!(f.joined_with, vec![up]);
    let u = outcome(&naming, up);
    assert_eq!(
        (u.station, u.name.as_str(), u.comes_off, u.alias_id),
        (far0, "0", true, Some(up0)),
        "Upstream shares Far Room's station and keeps its own id as an alias"
    );
    assert_eq!(u.joined_with, vec![far]);
    ok(p.detach_line(hub, main));
    assert_valid(&p);
    for o in &naming.lines {
        let slot = p.topology().slot_on(o.station, o.line);
        assert!(
            slot.is_some(),
            "{o:?}: the line is on the station the preview names"
        );
        assert_eq!(p.name_on(o.station, o.line), Some(o.name.as_str()));
        assert_eq!(
            p.line_alias(o.station, o.line)
                .map(|a| a.id)
                .filter(|&id| id != o.station),
            o.alias_id
        );
    }

    // Detach Upstream alone: it gets its own station back; the others stay.
    let mut q = merged;
    let naming = q.detach_naming(hub, up).unwrap();
    assert!(!naming.kept && naming.restored_alias);
    assert_eq!(naming.id, up0);
    let u = outcome(&naming, up);
    assert_eq!((u.station, u.comes_off, u.alias_id), (up0, true, None));
    assert!(u.joined_with.is_empty());
    let m = outcome(&naming, main);
    assert_eq!(
        (m.station, m.comes_off, m.joined_with),
        (hub, false, vec![far])
    );
    let f = outcome(&naming, far);
    assert_eq!(
        (f.station, f.comes_off, f.alias_id),
        (hub, false, Some(far0))
    );
    ok(q.detach_line(hub, up));
    assert_eq!(at(&q, up, 0), up0);
    assert_eq!((at(&q, main, 2), at(&q, far, 0)), (hub, hub));
}
