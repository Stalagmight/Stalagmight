//! What the read-only queries tell the UI about GPS fixes (#741), through the
//! command layer: the merge check's datum shift (the Merge dialog) and the
//! adjustment report's far fixes (the notice above the map). Synthetic
//! surveys only.

use super::topology_kit::{empty, gps_at, line_at, merge, north, open, origin, set_datum, slot};

#[test]
fn the_merge_check_says_which_placed_piece_moves_and_by_how_much() {
    let mut p = empty("TwoPieces");
    let a = line_at(&mut p, "A", [0.0, 0.0], north(2, 50.0));
    let b = line_at(&mut p, "B", [0.0, 0.0], north(2, 50.0));
    let opened = open(&p);
    let state = &opened.state;
    let (end_a, start_b) = (slot(a, 2), slot(b, 0));

    // One placed piece: a merge moves the unplaced one, which says nothing.
    set_datum(state, slot(a, 0), origin(), None);
    let one = state.check_merge_connectivity(end_a, start_b).unwrap();
    assert!(one.can_merge);
    assert_eq!(one.datum_shift, None);

    // B placed by its own GPS, 30 m east of A:2.
    set_datum(state, start_b, gps_at(30.0, 100.0), None);
    let two = state.check_merge_connectivity(start_b, end_a).unwrap();
    assert!(two.can_merge);
    let shift = two.datum_shift.expect("two placed pieces");
    assert_eq!(shift.moving_line_ids, vec![b]);
    assert_eq!(shift.datum_station, start_b);
    assert!(
        (shift.shift_m - 30.0).abs() < 0.01,
        "shift {}",
        shift.shift_m
    );
}

#[test]
fn the_adjustment_report_names_a_fix_that_fails_the_gap_check() {
    let mut p = empty("Traverse");
    let a = line_at(&mut p, "A", [0.0, 0.0], north(5, 50.0));
    let b = line_at(&mut p, "B", [0.0, 0.0], north(5, 50.0));
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 5), slot(b, 0));
    set_datum(state, slot(a, 0), origin(), None);
    assert!(state
        .get_adjustment_report()
        .unwrap()
        .far_gps_fixes
        .is_empty());

    // 40 m off over 500 m of survey: more than the 25 m threshold.
    let fix = slot(b, 5);
    set_datum(state, fix, gps_at(40.0, 500.0), None);
    let far = state.get_adjustment_report().unwrap().far_gps_fixes;
    assert_eq!(far.len(), 1, "{far:?}");
    assert_eq!(far[0].station, fix);
    assert!((far[0].gap_m - 40.0).abs() < 0.01, "gap {}", far[0].gap_m);
    assert!(far[0].gap_m > far[0].threshold_m);
}
