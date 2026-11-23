//! Merge stations (plan 5.1, 4.2) and the Merge dialog's check, ported from
//! today's `multi_datum_tests` / `datum_ui_tests` merge cases and
//! `closure_sweep_tests`' #738 rule.

use crate::{DatumRole, StationRef};

use super::ops_test_helpers::{
    assert_shifted, assert_still, assert_valid, free_line, gps_at, ok, origin, run, xy, xy_all,
};
use super::test_helpers::{at, project};
use super::{ArchivedStation, ClosureStrategy, LineId, OpError, Project};

const TOL: f64 = 1e-9;

/// Two free pieces: A east from (0, 0), 2 x 10 m; B north from (100, 50),
/// 2 x 5 m.
fn two_free_pieces() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    (p, a, b)
}

/// Two separately placed pieces (today's fixture): A (the frame, datum A:0)
/// runs 0..100 m north; B is placed by its own GPS so that B:0 sits 30 m east
/// of A:2.
fn two_placed_pieces() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    assert_eq!(
        p.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    let b0 = xy(&p, at(&p, b, 0));
    assert!((b0[0] - 30.0).abs() < 0.01 && (b0[1] - 100.0).abs() < 0.01);
    (p, a, b)
}

#[test]
fn merging_two_free_pieces_moves_a_onto_b_and_keeps_bs_placement() {
    let (mut p, a, b) = two_free_pieces();
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let a2_name = p.stations[&a2].name.clone();
    let before = xy_all(&p);
    let out = ok(p.merge_keeping_stationary(a2, b0));
    assert_valid(&p);
    assert_eq!(out.value.kept, b0);
    assert_eq!(out.value.archived, a2);
    assert_eq!((out.value.stationary, out.value.moving), (b0, a2));
    assert_eq!(out.value.moving_lines, vec![a]);
    assert!(!p.stations.contains_key(&a2));
    assert_eq!(p.line_stations(a).unwrap().last(), Some(&b0));
    assert!(p.topology().is_junction(b0));
    assert_eq!(
        p.stations[&b0].archived,
        vec![ArchivedStation {
            id: a2,
            name: a2_name,
            comment: None,
            external: None,
            // A's own station there (RS 4.1 step 2).
            lines: vec![a],
        }]
    );
    // B stays; A moves by (100 - 20, 50 - 0).
    let after = xy_all(&p);
    let a_stations: Vec<_> = p.line_stations(a).unwrap().to_vec();
    assert_shifted(&before, &after, TOL, |s| {
        if a_stations.contains(&s) && s != b0 {
            [80.0, 50.0]
        } else {
            [0.0, 0.0]
        }
    });
    assert_eq!(p.placements.len(), 1);
    assert_eq!(p.placements[0].station, b0);
    assert!(out.sweep.is_empty());
}

#[test]
fn a_placed_piece_stays_and_the_free_one_moves_whichever_is_picked_first() {
    for a_first in [true, false] {
        let (mut p, a, b) = two_free_pieces();
        ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
        let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
        let before = xy_all(&p);
        let out = if a_first {
            ok(p.merge_keeping_stationary(a2, b0))
        } else {
            ok(p.merge_keeping_stationary(b0, a2))
        };
        assert_valid(&p);
        assert_eq!(out.value.kept, a2, "the placed side is kept here");
        assert_eq!(out.value.moving_lines, vec![b]);
        let delta = [
            before[&a2][0] - before[&b0][0],
            before[&a2][1] - before[&b0][1],
        ];
        let a_stations: Vec<_> = p.line_stations(a).unwrap().to_vec();
        assert_shifted(&before, &xy_all(&p), TOL, |s| {
            if a_stations.contains(&s) {
                [0.0, 0.0]
            } else {
                delta
            }
        });
        assert!(p.placements.is_empty(), "the moving piece's placement goes");
    }
}

#[test]
fn merging_two_placed_pieces_moves_the_later_one_and_makes_its_datum_a_fix() {
    for b_first in [false, true] {
        let (mut p, a, b) = two_placed_pieces();
        let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
        let a_before: Vec<_> = (0..=2).map(|i| xy(&p, at(&p, a, i))).collect();
        let shift = p.merge_datum_shift(a2, b0).expect("two placed pieces");
        assert_eq!(shift.moving_lines, vec![b]);
        assert_eq!(shift.datum, 1);
        assert_eq!(shift.datum_station, b0);
        assert!((shift.shift_m - 30.0).abs() < 0.01, "{}", shift.shift_m);
        let b_before = xy(&p, b0);

        let out = if b_first {
            ok(p.merge_keeping_stationary(b0, a2))
        } else {
            ok(p.merge_keeping_stationary(a2, b0))
        };
        assert_valid(&p);
        assert_eq!(out.value.kept, a2);
        let a_after: Vec<_> = (0..=2).map(|i| xy(&p, at(&p, a, i))).collect();
        assert_eq!(a_before, a_after, "the earlier placement holds its piece");
        let b_after = xy(&p, at(&p, b, 0));
        let moved = (b_after[0] - b_before[0]).hypot(b_after[1] - b_before[1]);
        assert!(
            (moved - shift.shift_m).abs() < 1e-6,
            "preview said {}",
            shift.shift_m
        );
        assert_eq!(
            p.datum_roles(),
            vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
        );
        assert_eq!(p.datums[1].correction, None);
        assert_eq!(p.datums[1].station, a2, "the fix sits on the junction");
        assert_eq!(p.datums[1].via, Some(b), "and keeps its via hint");
        let gap = p.datum_gap(a2, &p.datums[1].gps).unwrap();
        assert!((gap.gap_m - 30.0).abs() < 0.01, "gap {}", gap.gap_m);
    }
}

#[test]
fn the_merge_preview_is_silent_unless_both_pieces_are_placed() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [10.0, 0.0], run(0.0, 2, 50.0));
    let c = free_line(&mut p, [20.0, 0.0], run(0.0, 2, 50.0));
    let (a2, b0, c0) = (at(&p, a, 2), at(&p, b, 0), at(&p, c, 0));
    assert_eq!(p.merge_datum_shift(a2, b0), None, "nothing placed");
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    assert_eq!(p.merge_datum_shift(a2, b0), None, "one side placed");
    assert_eq!(p.merge_datum_shift(c0, a2), None, "one side placed");
}

#[test]
fn a_merge_is_refused_for_one_station_unknown_stations_and_one_piece() {
    let (mut p, a, b) = two_free_pieces();
    let (a0, a2, b0) = (at(&p, a, 0), at(&p, a, 2), at(&p, b, 0));
    let before = p.clone();
    assert_eq!(
        p.merge_keeping_stationary(a0, a0),
        Err(OpError::AlreadyMerged)
    );
    assert_eq!(
        p.merge_keeping_stationary(a0, a2),
        Err(OpError::AlreadyConnected)
    );
    assert_eq!(
        p.merge_keeping_stationary(a0, 999),
        Err(OpError::UnknownStation(999))
    );
    assert_eq!(
        p.merge_stations(a2, b0, a0, &[]),
        Err(OpError::InvalidKeep(a0)),
        "a merge keeps one of its two stations"
    );
    ok(p.merge_keeping_stationary(a2, b0));
    let b2 = at(&p, b, 2);
    assert_eq!(
        p.merge_keeping_stationary(a0, b2),
        Err(OpError::AlreadyConnected),
        "the halves of one piece are already connected"
    );
    assert_eq!(
        OpError::AlreadyConnected.to_string(),
        "Stations are already connected. Use Create Loop Closure instead."
    );
    assert_ne!(p, before);
}

/// RS 4.1: the kept station is shown on every line and keeps its comment;
/// the other one keeps its own comment in its record (nothing is joined).
/// A:0 is named "0" like the kept B:0, which A now shows at its end: the
/// merge with no answer refuses (N8), the dialog's answer (rule L's letter,
/// the preview's suggestion) lets it through, and so does the no-dialog form.
/// Injection: join the comments in `merge_ids` and the kept comment reads
/// both notes.
#[test]
fn the_kept_station_is_shown_everywhere_and_comments_stay_apart() {
    let (mut p, a, b) = two_free_pieces();
    let (a0, a2, b0) = (at(&p, a, 0), at(&p, a, 2), at(&p, b, 0));
    p.station_mut(b0).unwrap().comment = Some("kept note".to_string());
    p.station_mut(a2).unwrap().comment = Some("archived note".to_string());
    assert_eq!(p.stations[&a0].name, p.stations[&b0].name);
    let before = p.clone();
    assert_eq!(
        p.merge_stations(a2, b0, b0, &[]),
        Err(OpError::NameTaken {
            line: a,
            name: "0".to_string()
        })
    );
    assert_eq!(p, before, "a refused merge changes nothing");
    let preview = p.merge_preview(a2, b0, b0).unwrap();
    assert_eq!(preview.collisions.len(), 1);
    let letter = &preview.collisions[0].suggestion;
    assert_eq!((letter.station, letter.line), (a0, a));
    assert_eq!((letter.from.as_str(), letter.to.as_str()), ("0", "0a"));
    assert_eq!(
        preview.hidden,
        (a2, "2".to_string(), Some("archived note".to_string()))
    );
    assert_eq!(
        preview.renamed_lines,
        vec![a],
        "A shows the junction as 0 now"
    );
    ok(p.merge_stations(a2, b0, b0, &[(a0, "0a".to_string())]));
    assert_valid(&p);
    assert_eq!(p.stations[&b0].name, "0");
    assert_eq!(p.stations[&a0].name, "0a");
    assert!(!p.is_native(b0, a));
    assert!(p.is_native(b0, b));
    assert_eq!(p.shown_name(b0), Some("0"));
    assert_eq!(p.stations[&b0].comment.as_deref(), Some("kept note"));
    assert_eq!(
        p.stations[&b0].archived[0].comment.as_deref(),
        Some("archived note")
    );

    // Without a dialog rule L letters A's own 0 (shown on one line), never
    // the kept junction (shown on two).
    let mut auto = before;
    let out = ok(auto.merge_stations_auto(a2, b0, b0));
    assert_eq!(out.identities.lettered.len(), 1);
    assert_eq!(out.identities.lettered[0].station, a0);
    assert_eq!(auto.stations[&a0].name, "0a");
    assert_eq!(auto, p, "the dialog's answer is rule L's");
}

#[test]
fn hand_moves_datums_and_closure_ends_follow_the_retired_station() {
    let (mut p, a, b) = two_free_pieces();
    let (a0, a2, b0) = (at(&p, a, 0), at(&p, a, 2), at(&p, b, 0));
    p.line_mut(a)
        .unwrap()
        .hand_moves
        .insert(a2, [0.25, -0.5, 0.0]);
    ok(p.create_closure(a0, Some(a), a2, Some(a), ClosureStrategy::Batch));
    let out = ok(p.merge_keeping_stationary(a2, b0));
    assert_valid(&p);
    assert_eq!(out.value.kept, b0);
    assert_eq!(
        p.line(a).unwrap().hand_moves.get(&b0),
        Some(&[0.25, -0.5, 0.0])
    );
    assert!(!p.line(a).unwrap().hand_moves.contains_key(&a2));
    assert_eq!(p.closures[0].b, b0);
    assert_eq!(p.closures[0].b_via, Some(a));
    assert!(p.topology().connected(a0, b0));
}

#[test]
fn a_closure_between_the_merged_stations_is_removed_by_the_sweep() {
    let (mut p, a, b) = two_placed_pieces();
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let closure = ok(p.create_closure(a2, Some(a), b0, Some(b), ClosureStrategy::Batch))
        .value
        .closure;
    let out = ok(p.merge_keeping_stationary(a2, b0));
    assert_valid(&p);
    assert!(p.closures.is_empty());
    // Reported as the sweep found it: the merge had already put both ends on
    // the survivor. The command layer names it by id from its own snapshot.
    let mut found = closure;
    found.b = a2;
    assert_eq!(out.sweep.removed, vec![found]);
    assert_eq!(out.sweep.removed_positions, vec![0]);
}

#[test]
fn the_merge_check_routes_connected_pairs_to_the_closure_dialog() {
    let (mut p, a, b) = two_free_pieces();
    let (a0, a1, a2, b0) = (at(&p, a, 0), at(&p, a, 1), at(&p, a, 2), at(&p, b, 0));

    let check = p.merge_check(a0, a2).unwrap();
    assert!(check.connected && !check.can_merge);
    assert_eq!(check.reason.as_deref(), Some("Creates survey loop"));
    let preview = check.preview.unwrap();
    assert!((preview.path_length - 20.0).abs() < 1e-9);
    assert!((preview.horizontal_misclosure - 20.0).abs() < 1e-9);
    assert!((preview.error_percentage - 100.0).abs() < 1e-9);
    assert_eq!(check.closure_refusal, None);
    assert!(check.datum_shift.is_none());

    let check = p.merge_check(a0, a1).unwrap();
    let refusal = check.closure_refusal.unwrap();
    assert!(
        refusal.contains("would join the two ends of one shot (L0 / 0 to L0 / 1)"),
        "{refusal}"
    );

    let check = p.merge_check(a2, b0).unwrap();
    assert!(!check.connected && check.can_merge);
    assert!(check.preview.is_none() && check.reason.is_none());
    assert!(check.datum_shift.is_none(), "neither piece is placed");

    assert_eq!(p.merge_check(a0, a0).err(), Some(OpError::AlreadyMerged));
    ok(p.create_closure(a0, None, a2, None, ClosureStrategy::Batch));
    assert_eq!(p.merge_check(a2, a0).err(), Some(OpError::DuplicateClosure));

    let (p, a, b) = two_placed_pieces();
    let check = p.merge_check(at(&p, a, 2), at(&p, b, 0)).unwrap();
    assert!(check.can_merge);
    assert_eq!(check.datum_shift.unwrap().moving_lines, vec![b]);
}

#[test]
fn two_placed_pieces_joined_by_a_closure_are_connected_for_the_dialog_but_may_merge() {
    // Today: the check reads any closure as connecting (routes to the closure
    // dialog), while the merge itself only refuses a rigid connection.
    let (mut p, a, b) = two_placed_pieces();
    let (a0, a2, b0, b2) = (at(&p, a, 0), at(&p, a, 2), at(&p, b, 0), at(&p, b, 2));
    ok(p.create_closure(a0, None, b2, None, ClosureStrategy::Batch));
    let check = p.merge_check(a2, b0).unwrap();
    assert!(check.connected && !check.can_merge);
    let before = xy_all(&p);
    ok(p.merge_keeping_stationary(a2, b0));
    assert_valid(&p);
    assert_eq!(
        p.closures.len(),
        1,
        "the closure now closes a loop in one piece"
    );
    let a_stations: Vec<_> = p.line_stations(a).unwrap().to_vec();
    assert_still(
        &before
            .iter()
            .filter(|(s, _)| a_stations.contains(s))
            .map(|(&s, &v)| (s, v))
            .collect(),
        &xy_all(&p),
        TOL,
    );
}

/// A retired station two lines showed natively (a Split Line Here
/// junction) is archived whole: ONE record with its id and both halves'
/// lines, nothing minted (RS 4.1 step 2). Detaching either half then brings
/// it back for both (they were one station). Injection: archive the first
/// native line only (the U1 `alias_line`) and the second half is native at
/// the merged node.
#[test]
fn a_merge_archives_a_split_junction_whole() {
    use super::SplitOptions;
    let (mut p, a, b) = two_free_pieces();
    let (a1, b0) = (at(&p, a, 1), at(&p, b, 0));
    let a2 = p
        .split_line(
            a,
            crate::SplitAt::Station { station_index: 1 },
            &SplitOptions {
                name_a: "A1".to_string(),
                name_b: "A2".to_string(),
                color_a: "#111111".to_string(),
                color_b: "#222222".to_string(),
            },
        )
        .unwrap()
        .value;
    assert_valid(&p);
    let counter = p.counters.next_station_id;
    ok(p.merge_keeping_stationary(a1, b0));
    assert_valid(&p);
    assert_eq!(p.counters.next_station_id, counter, "nothing minted");
    let archived = &p.stations[&b0].archived;
    assert_eq!(archived.len(), 1);
    assert_eq!(
        (archived[0].id, archived[0].lines.clone()),
        (a1, vec![a, a2])
    );
    assert_eq!(p.native_lines(b0), vec![b]);
    let out = ok(p.detach_line(b0, a2, &[], None));
    assert_eq!(out.value.new_station, a1);
    assert_eq!(out.value.lines_off, vec![a, a2]);
    assert!(p.topology().lines_of(b0) == vec![b]);
    assert_valid(&p);
}

/// The Merge dialog's preview (demo issue I-4) is what the merge then does:
/// the moving piece, its shift onto the stationary station, the joined
/// station's depth; and it changes nothing. Injection: measure the shift
/// from the stationary piece only (`after.xy(s)` against itself) and it
/// reads 0 m.
#[test]
fn the_merge_preview_is_what_the_merge_does() {
    let (mut p, a, b) = two_free_pieces();
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let before = p.clone();
    let preview = p.merge_preview(a2, b0, b0).unwrap();
    assert_eq!(p, before, "read-only");
    assert_eq!(preview.stationary, b0);
    assert_eq!(preview.moving_lines, vec![a]);
    assert!((preview.shift_m - 80.0_f64.hypot(50.0)).abs() < 1e-6);
    let xy_before = xy_all(&p);
    ok(p.merge_keeping_stationary(a2, b0));
    let moved = xy_all(&p)
        .iter()
        .filter_map(|(s, [x, y])| {
            let [bx, by] = xy_before.get(s)?;
            Some((x - bx).hypot(y - by))
        })
        .fold(0.0, f64::max);
    assert!((moved - preview.shift_m).abs() < 1e-9);
    let joined = p.raw_positions().z(b0).unwrap();
    assert!((joined - preview.joined_depth_m).abs() < TOL);
    // A refused pair has no preview.
    assert_eq!(p.merge_preview(b0, b0, b0), None);
}
