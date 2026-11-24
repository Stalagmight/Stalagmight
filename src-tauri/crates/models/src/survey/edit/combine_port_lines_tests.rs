//! Port of the legacy Combine Lines oracle (`project/combine_lines_tests.rs`,
//! 46 tests on `models::project::Project`) to the station-graph model: the
//! user-visible expectations (where stations sit after the combine, which
//! side moves, which ties are torn, the joined station's name, comment and
//! id, refusal texts, candidate facts), rebuilt on synthetic data.
//!
//! How the legacy constructs map (plan sections in brackets):
//! - a legacy merge is one shared station, a junction (1.2, 5.1): built with
//!   `line_from` (a line starting at the station) or the `merge` test helper;
//! - a legacy line offset is the free piece's `Placement` (3.3): `line_at`,
//!   and `place` here translates it; a legacy station offset is a per-line
//!   hand move (5.6), `hand_move` here (the placement follows when the
//!   station is the placement station, so the chain stays put as a legacy
//!   offset left it);
//! - a legacy aligned closure is a snap: a `Move` closure whose ends are
//!   within 1 cm (3.6), so the snap scenarios close their loops with a
//!   computed shot; a legacy aligned non-`Move` closure, and a legacy merge
//!   that closes a cycle, are `Tie` closures after migration (7.2.1, 7.2.4);
//! - the adjusted-view gauge of a datum-less piece is its `Placement` station
//!   (3.7); the Closures Computed View itself is stream M3 and not on this
//!   branch, so tests about the adjusted geometry assert its inputs (the
//!   gauge station and position, every raw position, the closures);
//! - a closure between two different pieces needs a datum in each (invariant
//!   7), so third lines that legacy left as separate free pieces hang off the
//!   survey where a closure ties them.
//!
//! Fate of each legacy test (the name here when it differs):
//! 1. `all_four_pairings_keep_every_station_in_place`: ported ("the join
//!    merge is gone" = no station of the combined line is a junction).
//! 2. `every_shot_keeps_its_exact_credit_through_every_pairing`: ported.
//! 3. `a_reversed_other_line_is_reciprocated_and_swaps_walls_when_asked`:
//!    ported.
//! 4. `the_kept_line_keeps_its_shot_ids_and_every_uuid_travels`: ported.
//! 5. `a_prepend_rekeys_the_kept_lines_start`: adapted as
//!    `a_prepend_keeps_what_is_on_the_kept_lines_start` (no `StationKey` to
//!    re-key: comment and closure end are on the station; C hangs off K's
//!    far end, invariant 7).
//! 6. `every_reference_on_the_other_line_follows_its_station`: adapted (C
//!    starts at O's middle station, D hangs off C, invariant 7; names and
//!    comments on the station, hand moves per line; the retired line is named
//!    by no `via`).
//! 7. `the_join_station_is_settled_per_the_choices`: adapted (the closure to
//!    a free piece hangs off O instead; the zero-length closure between the
//!    two free join stations cannot exist, invariant 7, its removal by the
//!    fold, #738, is covered by `combine_tests`; the retired name is an
//!    alias, plan 5.1, instead of a `station_names` count).
//! 8. `the_kept_join_station_never_takes_the_other_offset`: ported.
//! 9. `lines_sharing_a_shot_identity_get_fresh_ids`: not applicable (shot
//!    uuids are project-unique by invariant 1; the builder refuses a taken
//!    one).
//! 10. `a_shared_identity_on_the_gauge_shot_does_not_move_the_gauge`: not
//!     applicable (same reason; no gauge shot, 3.7; the gauge cases are 28-30).
//! 11. `an_aligned_closure_from_the_join_into_either_line_becomes_an_ordinary_one`:
//!     adapted into three: as snaps, `..._into_the_other_line_...` (ported)
//!     and `..._into_the_kept_line_...` (an M1 port finding, fixed), and as the
//!     migrated `Tie` the legacy closure is,
//!     `an_aligned_legacy_tie_from_the_join_into_either_line_...` (an M1
//!     port finding, fixed).
//! 12. `different_north_references_convert_and_qualities_take_the_kept_ones`:
//!     ported (O's reference set by the operation, which turns O about the
//!     junction; no re-placing needed).
//! 13. `a_declared_reference_and_the_project_default_combine_and_the_default_wins`:
//!     ported.
//! 14. `a_second_rigid_tie_becomes_a_loop_closure`: adapted into four: snaps
//!     (`..._directly`, `..._through_a_third_line_...`), the unaligned
//!     closure (`an_unaligned_second_closure_stays_an_ordinary_one`), and the
//!     migrated `Tie` form of the legacy cycle merges
//!     (`a_second_migrated_rigid_tie_...`, an M1 port finding, fixed).
//! 15. `a_third_line_merged_at_the_join_stays_attached_as_a_branch`: ported.
//! 16. `a_branch_merged_to_both_join_stations_is_merged_once`: adapted (C
//!     merged to both join stations makes them one station, plan 5.1: the
//!     join is a merged one).
//! 17. `an_unmerged_third_end_at_the_join_is_left_untouched`: ported.
//! 18. `the_station_query_lists_every_partner_at_a_junction`: ported.
//! 19. `the_point_query_lists_every_pair_at_a_junction`: ported.
//! 20. `a_line_through_the_join_stays_merged_as_a_crossing`: ported.
//! 21. `placement_datums_on_both_sides_block`: ported (the second piece's
//!     datum gets the correction that keeps it in place, 4.3).
//! 22. `two_datums_on_the_join_stations_block`: adapted (still refused for
//!     two side-by-side join stations; a merged join is one station already,
//!     which may hold several datums, plan 5.7, so it is not refused).
//! 23. `different_manual_offsets_on_the_join_keep_the_kept_one`: ported.
//! 24. `the_shot_budget_blocks`: ported.
//! 25. `the_shot_budget_is_exact_at_99_999`: ported.
//! 26. `lines_that_do_not_meet_meet_twice_or_are_empty`: ported.
//! 27. `the_tolerance_is_one_millimetre_and_only_a_large_depth_gap_blocks`:
//!     ported.
//! 28. `the_gauge_station_survives_keeping_either_line`: adapted (the gauge
//!     is the `Placement` station, 3.7; nothing to capture; both pairings).
//! 29. `keeping_the_higher_id_leaves_the_adjusted_geometry_in_place`: adapted
//!     (the adjusted view's inputs, M3).
//! 30. `a_one_shot_lower_lines_gauge_keeps_its_side`: adapted (no parity: the
//!     reversed line's `Placement` station is the gauge).
//! 31. `every_shot_keeps_its_record_parity_through_a_combine`: not applicable
//!     (record parity and `counter_parity_shots` are gone, plan 1.3; the
//!     reversal itself is pinned by 3 and 33).
//! 32. `combining_a_fresh_split_at_its_join_restores_the_line`: adapted (the
//!     split makes a junction, plan 12.2, so the station ids and names come
//!     back too).
//! 33. `combining_end_to_end_equals_reversing_then_combining`: ported.
//! 34. `an_unaligned_closure_between_them_becomes_a_loop_inside_the_line`:
//!     adapted (the adjusted view is M3: asserted that the solve takes the
//!     closure and the raw misclosure it distributes).
//! 35. `each_query_builds_one_end_table`: not applicable (a probe counter of
//!     the legacy implementation).
//! 36. `the_station_query_at_a_pile_reads_only_the_ends_there`: adapted as
//!     `the_station_query_at_a_pile_offers_no_pick` (the refusal; the probe
//!     count is not applicable).
//! 37. `the_pair_query_reads_only_the_ends_near_the_join`: adapted as
//!     `the_pair_query_finds_the_join_among_many_lines` (likewise).
//! 38. `a_kept_line_without_shot_id_room_is_refused_but_the_other_can_be_kept`:
//!     ported.
//! 39. `the_shot_id_room_check_is_exact_at_the_edge`: ported.
//! 40. `a_renamed_shot_keeps_its_record_parity`: not applicable (record
//!     parity and uuid remaps are gone, plan 1.3).
//! 41. `a_split_at_a_junction_combines_back_whatever_passes_through`: adapted
//!     (the split makes a junction, plan 12.2): "starts there" and "passes
//!     through" ported; "loops back" (a snap) split out as
//!     `..._and_a_loop_back_stays_snapped` (an M1 port finding, fixed).
//! 42. `combine_keeps_the_component_anchor_with_a_third_line`: adapted (M3:
//!     the gauge and every raw position).
//! 43. `split_keeps_the_component_anchor`: adapted (S's reverse parity put
//!     the gauge on its far end; here S's placement is set there).
//! 44. `a_depth_gap_at_an_unmerged_join_is_reported_never_refused`: ported.
//! 45. `a_side_by_side_combine_keeps_merged_stations_together`: adapted (the
//!     merged stations are one station by construction; a JSON round trip is
//!     the reopen; the moved lines are asserted).
//! 46. `a_hand_moved_join_station_keeps_its_branch_merged`: adapted (likewise;
//!     the #755 fact, `moved_with_joined_station`, is asserted).

use std::collections::BTreeMap;

use crate::{
    AzimuthReference, CombineJoinKind, CombineLinesOptions, CombinePick, CorrectionSource, LineEnd,
    LineId, NorthReference, Person, ShotData, StationId, StationRef, SurveyCredit,
};

use super::super::test_helpers::gps;
use super::super::{ClosureStrategy, Project};
use super::edit_test_helpers::{
    add_datum, assert_valid, empty, line, line_at, line_from, merge, ok, refused, skipped_ids, st,
    xy,
};
use super::{TornTie, TornTieKind, TornTieReason};

/// Float dust for "did not move".
const TOL: f64 = 1e-9;

/// The legacy shot: one azimuth pair, a slope length, left 1 and right 2.
fn shot(azimuth: f64, length: f64, uuid: &str) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        left: Some(1.0),
        right: Some(2.0),
        ..ShotData::default()
    }
}

/// The legacy dialog options: renamed "Joined", walls swapped on a reverse.
fn options() -> CombineLinesOptions {
    CombineLinesOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The chain index of `line`'s `end`.
fn end_index(p: &Project, line: LineId, end: LineEnd) -> usize {
    match end {
        LineEnd::Start => 0,
        LineEnd::End => p.line(line).unwrap().shots.len(),
    }
}

/// The station chain of `line`.
fn chain(p: &Project, line: LineId) -> Vec<StationId> {
    p.line_stations(line).unwrap().to_vec()
}

/// Every station of `line`, as horizontal positions along the chain (the
/// legacy `stations`).
fn stations(p: &Project, line: LineId) -> Vec<[f64; 2]> {
    let positions = p.raw_positions();
    chain(p, line).iter().map(|s| xy(&positions, *s)).collect()
}

/// The position of station `s`.
fn at(p: &Project, s: StationId) -> [f64; 2] {
    xy(&p.raw_positions(), s)
}

/// Translate the free piece holding `line` so its station `index` sits at
/// `target` plus `error` metres east (the legacy `place`, which moved a line
/// offset): the piece's `Placement` moves.
fn place(p: &mut Project, line: LineId, index: usize, target: [f64; 2], error: f64) {
    let station = st(p, line, index);
    let now = at(p, station);
    let (dx, dy) = (target[0] + error - now[0], target[1] - now[1]);
    let i = {
        let topology = p.topology();
        p.placements
            .iter()
            .position(|pl| topology.connected(pl.station, station))
            .unwrap()
    };
    p.placements[i].x += dx;
    p.placements[i].y += dy;
}

/// A legacy station offset: a hand move of `station` on `line`. When the
/// station is its piece's placement station the placement follows, so the
/// line's chain stays put (a legacy offset moved only the shown position).
fn hand_move(p: &mut Project, line: LineId, station: StationId, d: [f64; 3]) {
    p.line_mut(line).unwrap().hand_moves.insert(station, d);
    if let Some(pl) = p.placements.iter_mut().find(|pl| pl.station == station) {
        pl.x += d[0];
        pl.y += d[1];
    }
}

/// K: two shots east from (100, -40); O: three shots heading north then
/// west, placed so its `other_end` meets K's `kept_end`: one station (merged,
/// O's piece moved onto K's join station, which survives) or side by side
/// within `error` metres east.
fn pair(
    kept_end: LineEnd,
    other_end: LineEnd,
    merged: bool,
    error: f64,
) -> (Project, LineId, LineId) {
    let mut p = empty();
    let k = line_at(
        &mut p,
        "K",
        [100.0, -40.0],
        vec![shot(90.0, 10.0, "k0"), shot(80.0, 5.0, "k1")],
    );
    let o = line(
        &mut p,
        "O",
        vec![
            shot(0.0, 4.0, "o0"),
            shot(300.0, 6.0, "o1"),
            shot(270.0, 3.0, "o2"),
        ],
    );
    let join_k = st(&p, k, end_index(&p, k, kept_end));
    let index_o = end_index(&p, o, other_end);
    let target = at(&p, join_k);
    place(&mut p, o, index_o, target, if merged { 0.0 } else { error });
    if merged {
        let join_o = st(&p, o, index_o);
        merge(&mut p, join_o, join_k);
    }
    assert_valid(&p);
    (p, k, o)
}

/// The refusal the pair query gives (empty when combinable); a refusal is
/// also checked on the combine itself, keeping either line.
fn blocked(p: &Project, k: LineId, o: LineId) -> String {
    let reason = p
        .combine_candidate_for_lines(k, o)
        .unwrap()
        .blocked_reason
        .unwrap_or_default();
    if !reason.is_empty() {
        assert_refused_without_a_change(p, k, o, &reason);
    }
    reason
}

/// A blocked candidate is refused by the combine itself, with the same reason
/// and nothing changed, keeping either line (pattern from #713).
fn assert_refused_without_a_change(p: &Project, k: LineId, o: LineId, reason: &str) {
    for (kept, other) in [(k, o), (o, k)] {
        let mut copy = p.clone();
        let text = refused(copy.combine_lines(kept, other, &options()));
        assert_eq!(text, reason, "keeping {kept}");
        assert_eq!(copy, *p, "keeping {kept}: nothing changed");
    }
}

/// All four pairings, merged and co-located: K stays exactly in place, O's
/// stations move at most the tolerance, the far ends line up and the
/// combined line holds both chains in order.
#[test]
fn all_four_pairings_keep_every_station_in_place() {
    for kept_end in [LineEnd::Start, LineEnd::End] {
        for other_end in [LineEnd::Start, LineEnd::End] {
            for merged in [true, false] {
                let what = format!("{kept_end:?}-{other_end:?} merged={merged}");
                let (mut p, k, o) = pair(kept_end, other_end, merged, 0.0004);
                let before_k = stations(&p, k);
                let mut before_o = stations(&p, o);
                let candidate = p.combine_candidate_for_lines(k, o).unwrap();
                assert_eq!(candidate.blocked_reason, None, "{what}");
                let kind = if merged {
                    CombineJoinKind::Merged
                } else {
                    CombineJoinKind::CoLocated
                };
                assert_eq!(candidate.join_kind, kind, "{what}");
                assert_eq!(candidate.reverses, kept_end == other_end, "{what}");

                ok(p.combine_lines(k, o, &options()));

                assert!(p.line(o).is_none(), "{what}: O is gone");
                let after = stations(&p, k);
                assert_eq!(after.len(), 2 + 3 + 1, "{what}");
                // O's stations in K's direction of travel, joined at K's end.
                if kept_end == other_end {
                    before_o.reverse();
                }
                let (k_part, o_part) = if kept_end == LineEnd::End {
                    (&after[..3], &after[2..])
                } else {
                    (&after[3..], &after[..4])
                };
                for (i, (a, b)) in k_part.iter().zip(&before_k).enumerate() {
                    assert!(dist(*a, *b) < TOL, "{what}: K station {i} moved");
                }
                for (i, (a, b)) in o_part.iter().zip(&before_o).enumerate() {
                    assert!(
                        dist(*a, *b) <= 0.001,
                        "{what}: O station {i} moved {}",
                        dist(*a, *b)
                    );
                }
                let combined = p.line(k).unwrap();
                assert_eq!(combined.name.as_deref(), Some("Joined"));
                assert_eq!(combined.color, "#abcdef");
                assert_valid(&p);
                // Legacy: the join merge is gone. Here: no station of the
                // combined line is shared with another line.
                for s in chain(&p, k) {
                    assert!(!p.topology().is_junction(s), "{what}: {s} is a junction");
                }
            }
        }
    }
}

/// Give every shot of `line` a distinct credit (shot 1 left uncredited).
fn credit_every_shot(p: &mut Project, line: LineId, who: &str) {
    for (i, s) in p.line_mut(line).unwrap().shots.iter_mut().enumerate() {
        s.credit = if i == 1 {
            None
        } else {
            super::shots::stored_credit(SurveyCredit {
                surveyors: Person::named(&format!("{who}{i}")).into_iter().collect(),
                explorers: Person::named(who).into_iter().collect(),
                date: chrono::NaiveDate::from_ymd_opt(2026, 1 + i as u32, 5),
            })
        };
    }
}

/// Every shot's credit, keyed by its uuid.
fn credits(p: &Project) -> BTreeMap<String, Option<SurveyCredit>> {
    p.lines()
        .iter()
        .flat_map(|l| l.shots.iter())
        .map(|s| (s.uuid.clone(), s.credit.as_deref().cloned()))
        .collect()
}

/// Credit lives on the shot: every shot keeps its own exactly, the empty one
/// included, through all four pairings (two of them reverse the other
/// line) keeping either line.
#[test]
fn every_shot_keeps_its_exact_credit_through_every_pairing() {
    for kept_end in [LineEnd::Start, LineEnd::End] {
        for other_end in [LineEnd::Start, LineEnd::End] {
            for keep_lower in [true, false] {
                let (mut p, k, o) = pair(kept_end, other_end, true, 0.0);
                credit_every_shot(&mut p, k, "K");
                credit_every_shot(&mut p, o, "O");
                let before = credits(&p);
                assert!(before.values().filter(|c| c.is_some()).count() == 3);
                let (kept, other) = if keep_lower { (k, o) } else { (o, k) };

                ok(p.combine_lines(kept, other, &options()));

                assert_eq!(
                    credits(&p),
                    before,
                    "{kept_end:?}-{other_end:?} keep_lower {keep_lower}"
                );
            }
        }
    }
}

/// End to end: O is reversed first. Its bearings are reciprocated, and the
/// left/right walls swap only when asked.
#[test]
fn a_reversed_other_line_is_reciprocated_and_swaps_walls_when_asked() {
    for swap_lr in [true, false] {
        let (mut p, k, o) = pair(LineEnd::End, LineEnd::End, true, 0.0);
        ok(p.combine_lines(
            k,
            o,
            &CombineLinesOptions {
                swap_lr,
                ..options()
            },
        ));
        let shots = &p.line(k).unwrap().shots;
        // O's last shot (270 degrees) now leaves the join, reciprocated.
        let first_of_o = &shots[2];
        assert_eq!(first_of_o.azimuth_in, 90.0);
        assert_eq!(first_of_o.length, 3.0);
        let walls = (first_of_o.left, first_of_o.right);
        if swap_lr {
            assert_eq!(walls, (Some(2.0), Some(1.0)));
        } else {
            assert_eq!(walls, (Some(1.0), Some(2.0)));
        }
    }
}

/// Shot ids: K keeps its own, O's shots get fresh ones from K's counter;
/// uuids all travel.
#[test]
fn the_kept_line_keeps_its_shot_ids_and_every_uuid_travels() {
    let (mut p, k, o) = pair(LineEnd::Start, LineEnd::End, true, 0.0);
    let k_ids: Vec<u32> = p.line(k).unwrap().shots.iter().map(|s| s.id).collect();
    let mut uuids: Vec<String> = p
        .line(o)
        .unwrap()
        .shots
        .iter()
        .chain(&p.line(k).unwrap().shots)
        .map(|s| s.uuid.clone())
        .collect();
    ok(p.combine_lines(k, o, &options()));
    let combined = p.line(k).unwrap();
    let ids: Vec<u32> = combined.shots.iter().map(|s| s.id).collect();
    assert_eq!(&ids[3..], k_ids.as_slice(), "K's shots keep their ids");
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len());
    let mut after: Vec<String> = combined.shots.iter().map(|s| s.uuid.clone()).collect();
    after.sort();
    uuids.sort();
    assert_eq!(after, uuids);
}

/// Prepending moves K's station 0 to a later index: what is on that station
/// (a comment, a closure end) is on the station, so it follows (adapted: no
/// `StationKey` to re-key; C hangs off K's far end so the closure is inside
/// one piece, invariant 7).
#[test]
fn a_prepend_keeps_what_is_on_the_kept_lines_start() {
    let (mut p, k, o) = pair(LineEnd::Start, LineEnd::End, false, 0.0);
    let (k0, k2) = (st(&p, k, 0), st(&p, k, 2));
    let c = line_from(&mut p, "C", k2, vec![shot(180.0, 5.0, "c0")]);
    let c1 = st(&p, c, 1);
    p.station_mut(k0).unwrap().comment = Some("K start".to_string());
    p.push_closure(k0, Some(k), c1, Some(c), ClosureStrategy::Batch);
    assert_valid(&p);

    ok(p.combine_lines(k, o, &options()));

    assert_eq!(st(&p, k, 3), k0, "K's start is the joined station 3");
    assert_eq!(p.station(k0).unwrap().comment.as_deref(), Some("K start"));
    let closure = p.closures[0];
    assert_eq!((closure.a, closure.a_via), (k0, Some(k)));
    assert!(p.station(st(&p, k, 0)).unwrap().comment.is_none());
    assert_valid(&p);
}

/// Every kind of reference on O (and one on a third line pointing at O)
/// lands on the same physical station of the combined line, for a pairing
/// that reverses O (adapted: C starts at O's middle station, a junction; D
/// hangs off C so the closure to O's far end is inside one piece).
#[test]
fn every_reference_on_the_other_line_follows_its_station() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::End, true, 0.0);
    // O is three shots; reversed and appended at K's end (index 2), so O's
    // station j lands on K station 2 + (3 - j).
    let (o_far, o_mid, o_2) = (st(&p, o, 0), st(&p, o, 1), st(&p, o, 2));
    let c = line_from(&mut p, "C", o_mid, vec![shot(180.0, 5.0, "c0")]);
    let c1 = st(&p, c, 1);
    let d = line_from(&mut p, "D", c1, vec![shot(180.0, 5.0, "d0")]);
    let d1 = st(&p, d, 1);
    p.push_closure(o_far, Some(o), d1, Some(d), ClosureStrategy::Batch);
    add_datum(&mut p, o_2, gps(20.0, -87.0));
    for (station, text) in [(o_mid, "mid"), (o_far, "far")] {
        let record = p.station_mut(station).unwrap();
        record.comment = Some(format!("comment {text}"));
        record.name = format!("name {text}");
        hand_move(&mut p, o, station, [0.25, 0.0, 0.0]);
    }
    assert_valid(&p);
    let before_mid = at(&p, o_mid);

    ok(p.combine_lines(k, o, &options()));

    assert_eq!(st(&p, k, 4), o_mid);
    assert_eq!(st(&p, k, 5), o_far);
    assert!(dist(at(&p, o_mid), before_mid) < 0.001);
    let station = |s: StationId| p.station(s).unwrap().clone();
    assert_eq!(station(o_mid).comment.as_deref(), Some("comment mid"));
    assert_eq!(station(o_far).comment.as_deref(), Some("comment far"));
    assert_eq!(station(o_mid).name, "name mid");
    assert_eq!(station(o_far).name, "name far");
    assert_eq!(p.line(k).unwrap().hand(o_far), [0.25, 0.0, 0.0]);
    // C stays on O's middle station (legacy: still merged to it).
    assert_eq!(p.topology().slot_on(o_mid, c), Some(StationRef::new(c, 0)));
    let closure = p.closures[0];
    assert_eq!(closure.a, o_far);
    assert_eq!(p.datums[0].station, st(&p, k, 3));
    // No reference still names the retired line.
    assert!(p
        .closures
        .iter()
        .all(|c| c.a_via != Some(o) && c.b_via != Some(o)));
    assert!(p.datums.iter().all(|d| d.via != Some(o)));
    assert_valid(&p);
}

/// The join station: comments per the choice (an identical one kept once),
/// the kept station's name, the kept station's hand move winning, and a
/// closure to another station moved onto the joined station. Adapted: the
/// closures of the legacy test from the join stations to two free pieces are
/// invalid here (invariant 7), so C hangs off O's far end; the legacy
/// zero-length closure between the two join stations (dropped by the fold,
/// #738) cannot sit between two free pieces either: the existing
/// `combine_tests::a_closure_inside_one_piece_tears_the_junction_nearest_the_moving_line`
/// covers it.
#[test]
fn the_join_station_is_settled_per_the_choices() {
    for (pick, comment) in [
        (CombinePick::Kept, "kept"),
        (CombinePick::Other, "other"),
        (CombinePick::Both, "kept\nother"),
    ] {
        let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0);
        let (join_k, join_o, o3) = (st(&p, k, 2), st(&p, o, 0), st(&p, o, 3));
        let c = line_from(&mut p, "C", o3, vec![shot(180.0, 5.0, "c0")]);
        let c1 = st(&p, c, 1);
        for (s, text) in [(join_k, "kept"), (join_o, "other")] {
            let record = p.station_mut(s).unwrap();
            record.comment = Some(text.to_string());
            record.name = "S9".to_string();
        }
        hand_move(&mut p, k, join_k, [0.0004, 0.0, 0.0]);
        hand_move(&mut p, o, join_o, [0.0, 0.0, 0.0]);
        p.push_closure(join_o, Some(o), c1, Some(c), ClosureStrategy::Batch);
        assert_valid(&p);
        let candidate = p.combine_candidate_for_lines(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None);
        assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated);
        assert_eq!(candidate.comment_a.as_deref(), Some("kept"));

        ok(p.combine_lines(
            k,
            o,
            &CombineLinesOptions {
                comment_pick: pick,
                ..options()
            },
        ));

        assert_eq!(st(&p, k, 2), join_k);
        let joined = p.station(join_k).unwrap();
        assert_eq!(joined.comment.as_deref(), Some(comment), "{pick:?}");
        assert_eq!(joined.name, "S9");
        // The other one is archived in the joined station (history: its
        // only line was absorbed).
        assert_eq!(joined.archived.len(), 1);
        assert_eq!(joined.archived[0].id, join_o);
        assert_eq!(joined.archived[0].name, "S9");
        assert!(p.station(join_o).is_none());
        assert_eq!(p.line(k).unwrap().hand(join_k), [0.0004, 0.0, 0.0]);
        let hands: usize = p.lines().iter().map(|l| l.hand_moves.len()).sum();
        assert_eq!(hands, 1, "the other station's hand move is dropped");
        assert_eq!(p.closures.len(), 1);
        assert_eq!(p.closures[0].a, join_k);
        assert_valid(&p);
    }
}

/// Only the other join station was moved by hand (by less than the
/// tolerance): its hand move is dropped so the kept line's end stays exactly
/// where it was.
#[test]
fn the_kept_join_station_never_takes_the_other_offset() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0);
    let join_o = st(&p, o, 0);
    hand_move(&mut p, o, join_o, [0.0006, 0.0, 0.0]);
    let join_k = st(&p, k, 2);
    let before = at(&p, join_k);
    ok(p.combine_lines(k, o, &options()));
    assert!(p.lines().iter().all(|l| l.hand_moves.is_empty()));
    assert!(dist(at(&p, st(&p, k, 2)), before) < 1e-12);
}

/// A flat shot from `from` to `to` (closes a loop to float dust, so a `Move`
/// closure between its end and a station at `to` is a snap).
fn closing_shot(from: [f64; 2], to: [f64; 2], uuid: &str) -> ShotData {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    shot(
        dx.atan2(dy).to_degrees().rem_euclid(360.0),
        dx.hypot(dy),
        uuid,
    )
}

/// Is closure `i` a snap now?
fn snapped(p: &Project, i: usize) -> bool {
    p.closures[i].strategy == ClosureStrategy::Move
        && p.closure_is_aligned(&p.raw_positions(), &p.closures[i])
}

/// The legacy test's two aligned closures from the merged join into either
/// line, as snaps (an aligned `Move` closure needs its ends within 1 cm): O
/// loops back onto the join (the join into the other line), or K loops so
/// its start sits on its end, the join (the join into the kept line).
fn aligned_from_the_join(into_kept: bool) -> (Project, LineId, LineId) {
    let mut p = empty();
    let k_shots = if into_kept {
        vec![
            shot(90.0, 10.0, "k0"),
            shot(0.0, 5.0, "k1"),
            shot(270.0, 10.0, "k2"),
            shot(180.0, 5.0, "k3"),
        ]
    } else {
        vec![shot(90.0, 10.0, "k0"), shot(80.0, 5.0, "k1")]
    };
    let k = line_at(&mut p, "K", [100.0, -40.0], k_shots);
    let n = p.line(k).unwrap().shots.len();
    let join = st(&p, k, n);
    let o_shots = if into_kept {
        vec![
            shot(0.0, 4.0, "o0"),
            shot(300.0, 6.0, "o1"),
            shot(270.0, 3.0, "o2"),
        ]
    } else {
        vec![
            shot(0.0, 4.0, "o0"),
            shot(90.0, 3.0, "o1"),
            shot(180.0, 4.0, "o2"),
            shot(270.0, 3.0, "o3"),
        ]
    };
    let o = line_from(&mut p, "O", join, o_shots);
    if into_kept {
        let k0 = st(&p, k, 0);
        p.push_closure(join, Some(o), k0, Some(k), ClosureStrategy::Move);
    } else {
        let o4 = st(&p, o, 4);
        p.push_closure(join, Some(k), o4, Some(o), ClosureStrategy::Move);
    }
    assert!(snapped(&p, 0), "setup: a snap");
    assert_valid(&p);
    (p, k, o)
}

/// Assert the plan and the result of a combine of `k` and `o` whose one
/// closure is a second rigid tie: torn into an ordinary closure, nothing
/// moves.
fn assert_tie_becomes_ordinary(mut p: Project, k: LineId, o: LineId, what: &str) {
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None, "{what}");
    let torn = &candidate.plan_keeping_a.torn_ties;
    assert_eq!(torn.len(), 1, "{what}: {torn:?}");
    assert_eq!(torn[0].kind, TornTieKind::SnappedClosure, "{what}");
    assert_eq!(candidate.plan_keeping_a.moving_line, None, "{what}");

    ok(p.combine_lines(k, o, &options()));
    let closure = p.closures[0];
    assert_eq!(closure.strategy, ClosureStrategy::Batch, "{what}");
    assert!(
        !p.closure_is_aligned(&p.raw_positions(), &closure),
        "{what}"
    );
    assert_valid(&p);
}

/// An aligned closure (a snap) from the merged join into the other line is a
/// second rigid tie (from #713): it becomes an ordinary closure inside the
/// combined line, and the plan says so.
#[test]
fn an_aligned_closure_from_the_join_into_the_other_line_becomes_an_ordinary_one() {
    let (p, k, o) = aligned_from_the_join(false);
    assert_tie_becomes_ordinary(p, k, o, "join -> O");
}

/// The same from the merged join into the KEPT line (legacy: from O's member
/// of the merge to K's station 1).
#[test]
fn an_aligned_closure_from_the_join_into_the_kept_line_becomes_an_ordinary_one() {
    let (p, k, o) = aligned_from_the_join(true);
    assert_tie_becomes_ordinary(p, k, o, "join -> K");
}

/// The legacy test literally: an aligned non-`Move` closure (stored
/// `is_aligned`, ends 4 m and 5 m apart) is a `Tie` in the new model (plan
/// 7.2.4: "keeps today's behaviour for those ties").
#[test]
fn an_aligned_legacy_tie_from_the_join_into_either_line_becomes_an_ordinary_one() {
    for into_kept in [false, true] {
        let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
        let join = st(&p, k, 2);
        if into_kept {
            let k1 = st(&p, k, 1);
            p.push_closure(join, Some(o), k1, Some(k), ClosureStrategy::Tie);
        } else {
            let o1 = st(&p, o, 1);
            p.push_closure(join, Some(k), o1, Some(o), ClosureStrategy::Tie);
        }
        assert_valid(&p);
        assert_tie_becomes_ordinary(p, k, o, &format!("Tie, into kept {into_kept}"));
    }
}

/// A different north reference does not block: the other line's bearings
/// turn to read against the kept line's reference, so every station stays
/// where it is, whichever line is kept. A different quality takes the kept
/// line's.
#[test]
fn different_north_references_convert_and_qualities_take_the_kept_ones() {
    for keep_k in [true, false] {
        let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
        let declared =
            AzimuthReference::from_parts(NorthReference::Magnetic, 5.0, CorrectionSource::Manual);
        // A geometry edit of O: O's first station (the join) stays, O turns.
        ok(p.set_line_azimuth_reference(o, Some(declared)));
        p.line_mut(o).unwrap().quality = 2.0;
        let candidate = p.combine_candidate_for_lines(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "keep K {keep_k}");
        let rotation = if keep_k {
            candidate.plan_keeping_a.azimuth_rotation_deg
        } else {
            candidate.plan_keeping_b.azimuth_rotation_deg
        };
        assert_eq!(rotation, Some(if keep_k { 5.0 } else { -5.0 }));
        let mut before: Vec<[f64; 2]> = stations(&p, k);
        before.extend(stations(&p, o).into_iter().skip(1));
        let quality = if keep_k { 1.0 } else { 2.0 };

        let (kept, other) = if keep_k { (k, o) } else { (o, k) };
        ok(p.combine_lines(kept, other, &options()));
        let combined = p.line(kept).unwrap();
        assert_eq!(combined.azimuth_reference, (!keep_k).then_some(declared));
        assert!(
            (combined.quality - quality).abs() < 1e-12,
            "keep K {keep_k}"
        );
        let after = stations(&p, kept);
        assert_eq!(after.len(), before.len());
        for (want, got) in before.iter().zip(&after) {
            assert!(
                dist(*want, *got) < 1e-6,
                "keep K {keep_k}: {want:?} vs {got:?}"
            );
        }
        assert_valid(&p);
    }
}

/// The stored references are compared, not the resolved ones: O declares the
/// very reference the project default resolves K to. The combined line
/// keeps only K's `None`, so the plan reports a conversion (by 0 degrees: O's
/// readings stay, and follow the project default from now on).
#[test]
fn a_declared_reference_and_the_project_default_combine_and_the_default_wins() {
    let declared =
        AzimuthReference::from_parts(NorthReference::Magnetic, 5.0, CorrectionSource::Manual);
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    p.settings.default_azimuth_reference = Some(declared);
    p.line_mut(o).unwrap().azimuth_reference = Some(declared);
    assert_eq!(
        p.azimuth_reference_for(p.line(k).unwrap()),
        p.azimuth_reference_for(p.line(o).unwrap()),
        "setup: both resolve alike"
    );
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(
        candidate.plan_keeping_a.azimuth_rotation_deg,
        Some(0.0),
        "the values stay; the reference O follows changes"
    );
    let readings: Vec<f64> = p
        .line(o)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.azimuth_in)
        .collect();
    ok(p.combine_lines(k, o, &options()));
    let combined = p.line(k).unwrap();
    assert_eq!(combined.azimuth_reference, None);
    let after: Vec<f64> = combined.shots[2..].iter().map(|s| s.azimuth_in).collect();
    assert_eq!(after, readings);

    // Both declaring the same one: nothing to convert.
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    for id in [k, o] {
        p.line_mut(id).unwrap().azimuth_reference = Some(declared);
    }
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.plan_keeping_a.azimuth_rotation_deg, None);
}

/// The pair, merged end to start, with O's last shot replaced by one that
/// ends on K's start (`onto_k_start`) or with a third line C from K's start
/// whose one shot ends on O's end.
fn second_tie_pair(through_c: bool) -> (Project, LineId, LineId, Option<LineId>) {
    let mut p = empty();
    let k = line_at(
        &mut p,
        "K",
        [100.0, -40.0],
        vec![shot(90.0, 10.0, "k0"), shot(80.0, 5.0, "k1")],
    );
    let (k0, k2) = (st(&p, k, 0), st(&p, k, 2));
    let o = line_from(
        &mut p,
        "O",
        k2,
        vec![shot(0.0, 4.0, "o0"), shot(300.0, 6.0, "o1")],
    );
    let o2 = st(&p, o, 2);
    if through_c {
        ok(p.append_shot(o, shot(270.0, 3.0, "o2")));
        let o3 = st(&p, o, 3);
        let c_shot = closing_shot(at(&p, k0), at(&p, o3), "c0");
        let c = line_from(&mut p, "C", k0, vec![c_shot]);
        let c1 = st(&p, c, 1);
        p.push_closure(o3, Some(o), c1, Some(c), ClosureStrategy::Move);
        assert!(snapped(&p, 0), "setup: a snap");
        assert_valid(&p);
        (p, k, o, Some(c))
    } else {
        ok(p.append_shot(o, closing_shot(at(&p, o2), at(&p, k0), "o2")));
        let o3 = st(&p, o, 3);
        p.push_closure(k0, Some(k), o3, Some(o), ClosureStrategy::Move);
        assert!(snapped(&p, 0), "setup: a snap");
        assert_valid(&p);
        (p, k, o, None)
    }
}

/// A second rigid tie between the two lines would close a loop inside the
/// combined line: it becomes an ordinary loop closure. Directly: K's start
/// snapped to O's end (legacy: merged, from a file before the merge check;
/// as a snap here, the aligned tie new commands make).
#[test]
fn a_second_rigid_tie_becomes_a_loop_closure_directly() {
    let (mut p, k, o, _) = second_tie_pair(false);
    let (k0, o3) = (st(&p, k, 0), st(&p, o, 3));
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Merged);
    let torn = &candidate.plan_keeping_a.torn_ties;
    assert_eq!(torn.len(), 1, "{torn:?}");
    assert_eq!(torn[0].kind, TornTieKind::SnappedClosure);
    assert_eq!(torn[0].reason, TornTieReason::Loop);
    ok(p.combine_lines(k, o, &options()));
    // The combined line is not tied to itself by a snap.
    let closure = p.closures[0];
    assert_eq!((closure.a, closure.b), (k0, o3));
    assert_eq!(st(&p, k, 0), k0);
    assert_eq!(st(&p, k, 5), o3);
    assert_eq!(closure.strategy, ClosureStrategy::Batch);
    assert_valid(&p);
}

/// Through C: K's start and O's end are both tied to C (C starts at K's start
/// and its end is snapped to O's end). The tie into O tears; C stays on K's
/// start.
#[test]
fn a_second_rigid_tie_through_a_third_line_becomes_a_loop_closure() {
    let (mut p, k, o, c) = second_tie_pair(true);
    let c = c.unwrap();
    let (k0, o3, c1) = (st(&p, k, 0), st(&p, o, 3), st(&p, c, 1));
    let id = p.closures[0].id;
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(
        candidate.plan_keeping_a.torn_ties,
        vec![TornTie {
            kind: TornTieKind::SnappedClosure,
            reason: TornTieReason::Loop,
            station: o3,
            slot: StationRef::new(o, 3),
            other_slot: StationRef::new(c, 1),
            closure: Some(id),
        }]
    );
    ok(p.combine_lines(k, o, &options()));
    assert_eq!(p.topology().slot_on(k0, c), Some(StationRef::new(c, 0)));
    assert_eq!(st(&p, k, 0), k0);
    let closure = p.closures[0];
    assert_eq!((closure.a, closure.b), (o3, c1));
    assert_eq!(st(&p, k, 5), o3);
    assert_eq!(closure.strategy, ClosureStrategy::Batch);
    assert_valid(&p);
}

/// An unaligned closure between K's start and O's end is an ordinary loop
/// closure already: nothing is torn and it stays as it is.
#[test]
fn an_unaligned_second_closure_stays_an_ordinary_one() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    let (k0, o3) = (st(&p, k, 0), st(&p, o, 3));
    p.push_closure(k0, Some(k), o3, Some(o), ClosureStrategy::Batch);
    assert_valid(&p);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert!(candidate.plan_keeping_a.torn_ties.is_empty());
    ok(p.combine_lines(k, o, &options()));
    let closure = p.closures[0];
    assert_eq!((closure.a, closure.b), (k0, o3));
    assert_eq!(closure.strategy, ClosureStrategy::Batch);
}

/// The legacy test's two rigid ties as the migration writes them (plan
/// 7.2.1: a merge that would close a cycle becomes a `Tie` closure at the
/// front of the list): K's start tied to O's end directly, and through C (C
/// merged to K's start, its end tied to O's end).
#[test]
fn a_second_migrated_rigid_tie_becomes_a_loop_closure() {
    for through_c in [false, true] {
        let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
        let (k0, o3) = (st(&p, k, 0), st(&p, o, 3));
        let c = through_c.then(|| line_from(&mut p, "C", k0, vec![shot(0.0, 1.0, "c0")]));
        let (a, b) = match c {
            Some(c) => (o3, st(&p, c, 1)),
            None => (k0, o3),
        };
        let id = p.mint_closure_id();
        p.closures.insert(
            0,
            super::super::LoopClosure {
                id,
                a,
                a_via: None,
                b,
                b_via: None,
                strategy: ClosureStrategy::Tie,
                archived: None,
            },
        );
        assert_valid(&p);
        let what = format!("through C {through_c}");
        let candidate = p.combine_candidate_for_lines(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "{what}");
        assert_eq!(candidate.plan_keeping_a.torn_ties.len(), 1, "{what}");
        assert_eq!(
            candidate.plan_keeping_a.torn_ties[0].reason,
            TornTieReason::Loop,
            "{what}"
        );
        ok(p.combine_lines(k, o, &options()));
        let closure = p.closures[0];
        assert_eq!((closure.a, closure.b), (a, b), "{what}");
        assert_eq!(closure.strategy, ClosureStrategy::Batch, "{what}");
        if let Some(c) = c {
            assert_eq!(p.topology().slot_on(k0, c), Some(StationRef::new(c, 0)));
        }
        assert_valid(&p);
    }
}

/// A third line C at the join (on K's end, the joined station): the pair
/// combines, and C stays on the joined station, now in the middle of the
/// combined line, whether O is appended to K or K is prepended to O.
#[test]
fn a_third_line_merged_at_the_join_stays_attached_as_a_branch() {
    for keep_k in [true, false] {
        let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
        let join = st(&p, k, 2);
        let c = line_from(&mut p, "C", join, vec![shot(180.0, 5.0, "c0")]);
        let c1 = st(&p, c, 1);
        let candidate = p.combine_candidate_for_lines(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "keep K {keep_k}");
        assert_eq!(candidate.branches, vec![c]);
        assert!(candidate.loose_ends.is_empty());
        let before = at(&p, c1);

        let (kept, other) = if keep_k { (k, o) } else { (o, k) };
        ok(p.combine_lines(kept, other, &options()));

        // K has two shots: the joined station is the combined line's
        // station 2 either way (O appended to K, or K prepended to O).
        assert_eq!(st(&p, kept, 2), join, "keep K {keep_k}");
        assert_eq!(
            p.topology().lines_of(join),
            vec![kept, c],
            "keep K {keep_k}"
        );
        assert_eq!(p.topology().slot_on(join, c), Some(StationRef::new(c, 0)));
        let combined = p.line(kept).unwrap().shots.len();
        assert!(combined > 2);
        assert!(dist(at(&p, c1), before) < TOL);
        assert_valid(&p);
    }
}

/// K's end and O's start side by side, C merged to BOTH (adapted: merging C's
/// start to both makes the three one station, plan 5.1, so the join is
/// merged): the pair combines and C stays on the joined station, once.
#[test]
fn a_branch_merged_to_both_join_stations_is_merged_once() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0);
    let (join_k, join_o) = (st(&p, k, 2), st(&p, o, 0));
    let c = line_from(&mut p, "C", join_k, vec![shot(180.0, 5.0, "c0")]);
    merge(&mut p, join_o, join_k);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(
        candidate.join_kind,
        CombineJoinKind::Merged,
        "one station, through C"
    );
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.branches, vec![c]);
    ok(p.combine_lines(k, o, &options()));
    assert_eq!(p.topology().lines_of(join_k), vec![k, c]);
    assert_eq!(p.topology().slots_of(join_k).len(), 2);
    assert_valid(&p);
}

/// A third line's end side by side with an unmerged join is not merged to
/// it: the combine goes ahead, reports it, and leaves it as it was.
#[test]
fn an_unmerged_third_end_at_the_join_is_left_untouched() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0);
    let target = at(&p, st(&p, k, 2));
    let c = line_at(&mut p, "C", target, vec![shot(180.0, 5.0, "c0")]);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.loose_ends, vec![c]);
    assert!(candidate.branches.is_empty());
    let c_before = p.line(c).unwrap().clone();
    let c_at = stations(&p, c);

    ok(p.combine_lines(k, o, &options()));
    assert_eq!(*p.line(c).unwrap(), c_before);
    assert_eq!(stations(&p, c), c_at);
    assert!(!p.topology().is_junction(st(&p, c, 0)));
}

/// At a junction the station menu lists every partner, each answered on its
/// own; any one pair combines, and the rest stay at the joined station.
#[test]
fn the_station_query_lists_every_partner_at_a_junction() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    let join = st(&p, k, 2);
    let c = line_from(&mut p, "C", join, vec![shot(180.0, 5.0, "c0")]);
    // A fourth, D, whose quality differs: it combines too (taking the kept
    // line's quality).
    let d = line_from(&mut p, "D", join, vec![shot(270.0, 4.0, "d0")]);
    p.line_mut(d).unwrap().quality = 2.0;

    let candidates = p.combine_candidates(StationRef::new(k, 2));
    let partners: Vec<(LineId, bool)> = candidates
        .iter()
        .map(|x| (x.line_b, x.blocked_reason.is_none()))
        .collect();
    assert_eq!(partners, vec![(o, true), (c, true), (d, true)]);
    // Each candidate names the others at the point as branches.
    assert_eq!(candidates[1].branches, vec![o, d]);

    ok(p.combine_lines(k, c, &options()));
    let others: Vec<StationRef> = p
        .topology()
        .slots_of(join)
        .iter()
        .copied()
        .filter(|slot| slot.line_id != k)
        .collect();
    assert_eq!(others, vec![StationRef::new(o, 0), StationRef::new(d, 0)]);
}

/// The station menu's query lists every pair at a junction: the clicked
/// line with each partner first, then the pairs of the others, each joined
/// at this point. With K's end, O's start, C's start and D's start on one
/// station: 3 + 3 pairs, every one combinable.
#[test]
fn the_point_query_lists_every_pair_at_a_junction() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    let join = st(&p, k, 2);
    let c = line_from(&mut p, "C", join, vec![shot(180.0, 5.0, "c0")]);
    let d = line_from(&mut p, "D", join, vec![shot(180.0, 5.0, "d0")]);
    let pairs: Vec<(LineId, LineId, bool)> = p
        .combine_candidates_at_point(StationRef::new(k, 2))
        .iter()
        .map(|x| (x.line_a, x.line_b, x.blocked_reason.is_none()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            (k, o, true),
            (k, c, true),
            (k, d, true),
            (o, c, true),
            (o, d, true),
            (c, d, true),
        ]
    );
    // The clicked line's own list is unchanged.
    assert_eq!(p.combine_candidates(StationRef::new(k, 2)).len(), 3);
    // A pair of the others combines at this point, and K stays attached.
    ok(p.combine_lines(c, d, &options()));
    assert!(p.topology().lines_of(join).contains(&c));
    assert!(p.topology().lines_of(join).contains(&k));
    assert_valid(&p);
}

/// A line through the join at a station in its MIDDLE passes through: the
/// combine goes ahead and it stays on the joined station (a crossing). What
/// still offers nothing to join: an end on the middle of another line only, a
/// station that is not an end, and an end nothing touches.
#[test]
fn a_line_through_the_join_stays_merged_as_a_crossing() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    let join = st(&p, k, 2);
    let c = line(
        &mut p,
        "C",
        vec![shot(0.0, 3.0, "c0"), shot(0.0, 3.0, "c1")],
    );
    let target = at(&p, join);
    place(&mut p, c, 1, target, 0.0);
    let c1 = st(&p, c, 1);
    merge(&mut p, c1, join);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.passing_through, vec![c]);
    assert!(candidate.branches.is_empty());
    ok(p.combine_lines(k, o, &options()));
    assert_eq!(p.topology().slot_on(join, c), Some(StationRef::new(c, 1)));
    assert_eq!(st(&p, k, 2), join);
    assert_valid(&p);

    let mut p = empty();
    let k = line(&mut p, "K", vec![shot(90.0, 10.0, "k0")]);
    let c = line(
        &mut p,
        "C",
        vec![shot(0.0, 3.0, "c0"), shot(0.0, 3.0, "c1")],
    );
    let target = at(&p, st(&p, k, 1));
    place(&mut p, c, 1, target, 0.0);
    let (k1, c1) = (st(&p, k, 1), st(&p, c, 1));
    merge(&mut p, k1, c1);
    let at_end = p.combine_candidate(StationRef::new(k, 1)).unwrap();
    assert!(at_end
        .blocked_reason
        .unwrap()
        .contains("Merged to the middle of another line"));
    assert!(
        p.combine_candidate(StationRef::new(k, 0)).is_none(),
        "nothing touches"
    );
    assert!(
        p.combine_candidate(StationRef::new(c, 1)).is_none(),
        "not an end"
    );
}

/// Put a datum on `station` of a free piece and give it the correction that
/// keeps the piece where it is (the legacy test pushed the datum without
/// moving anything; here a new placement datum would put its piece on its
/// GPS).
fn add_datum_in_place(p: &mut Project, station: StationId, at_gps: crate::GpsCoordinates) {
    let here = at(p, station);
    add_datum(p, station, at_gps);
    let i = p.datums.len() - 1;
    p.datums[i].correction = p.correction_to_hold(i, here);
    assert!(dist(at(p, station), here) < TOL, "setup: the piece stays");
}

/// Remove the datums on `station` through the anchor protocol (a piece left
/// without one gets a placement where it is).
fn remove_datums_on(p: &mut Project, station: StationId) {
    let before = p.anchor_snapshot();
    p.datums.retain(|d| d.station != station);
    p.settle_anchors(&before, &super::super::EditContext::none());
    assert_valid(p);
}

#[test]
fn placement_datums_on_both_sides_block() {
    // Side by side, each piece placed by its own datum.
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0);
    let (k0, o3) = (st(&p, k, 0), st(&p, o, 3));
    add_datum(&mut p, k0, gps(20.0, -87.0));
    add_datum_in_place(&mut p, o3, gps(20.0, -87.0));
    assert_valid(&p);
    assert!(blocked(&p, k, o).contains("zero datum"));

    // A GPS fix elsewhere on the other line does not block a merged join:
    // one piece, so the second datum is a fix (#741) and holds nothing.
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    let (k0, o3) = (st(&p, k, 0), st(&p, o, 3));
    add_datum(&mut p, k0, gps(20.0, -87.0));
    add_datum(&mut p, o3, gps(20.0, -87.0));
    assert_eq!(p.datums.len(), 2);
    assert_eq!(p.placement_datums(), vec![Some(0)], "datum 1 is a GPS fix");
    assert_eq!(blocked(&p, k, o), "", "a GPS fix does not block a combine");
}

/// A datum on each of two side-by-side join stations: combining would make
/// them one station holding two datums, so it is refused until one goes.
/// Adapted: legacy refused this for a MERGED join too (a placement datum and
/// a GPS fix on the two members of one merge); here a merged join is already
/// one station, which may hold several datums (plan 5.7), so the combine
/// folds nothing and is not refused.
#[test]
fn two_datums_on_the_join_stations_block() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0);
    let (join_k, join_o) = (st(&p, k, 2), st(&p, o, 0));
    add_datum(&mut p, join_k, gps(20.0, -87.0));
    add_datum_in_place(&mut p, join_o, gps(20.0, -87.0));
    let reason = blocked(&p, k, o);
    assert!(reason.contains("each hold a zero datum"), "{reason}");
    let before = p.datums.clone();
    assert!(p.combine_lines(k, o, &options()).is_err());
    assert_eq!(p.datums, before, "nothing changed");

    // With either datum removed, the combine goes ahead and the one left
    // stays on the joined station.
    remove_datums_on(&mut p, join_o);
    assert_eq!(blocked(&p, k, o), "");
    ok(p.combine_lines(k, o, &options()));
    assert_eq!(p.datums.len(), 1);
    assert_eq!(p.datums[0].station, join_k);

    // Merged: the two datums are on one station already (plan 5.7).
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    let join = st(&p, k, 2);
    add_datum(&mut p, join, gps(20.0, -87.0));
    add_datum(&mut p, join, gps(20.0, -87.0));
    assert_eq!(p.placement_datums(), vec![Some(0)]);
    assert_eq!(blocked(&p, k, o), "");
    ok(p.combine_lines(k, o, &options()));
    assert!(p.datums.iter().all(|d| d.station == join));
    assert_eq!(p.datums.len(), 2);
    assert_valid(&p);
}

/// Join stations moved by different hand moves: the joined station keeps the
/// kept one's (the fixed side's), and the plan says how far the other moves.
#[test]
fn different_manual_offsets_on_the_join_keep_the_kept_one() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0);
    let join_o = st(&p, o, 0);
    hand_move(&mut p, o, join_o, [0.0, 0.0, 0.25]);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    let change = candidate.plan_keeping_a.join_offset_change_m.unwrap();
    assert!((change - 0.25).abs() < 1e-9, "{change}");
    ok(p.combine_lines(k, o, &options()));
    let joined = st(&p, k, 2);
    assert_eq!(p.line(k).unwrap().hand_moves.get(&joined), None);
    assert!(p.lines().iter().all(|l| l.hand_moves.is_empty()));
}

/// Two lines of `k_shots` and `o_shots`, merged end to start.
fn budget_pair(k_shots: usize, o_shots: usize) -> (Project, LineId, LineId) {
    let mut p = empty();
    let k = line(
        &mut p,
        "K",
        super::edit_test_helpers::run("k", 90.0, 1.0, k_shots),
    );
    let end = st(&p, k, k_shots);
    let o = line_from(
        &mut p,
        "O",
        end,
        super::edit_test_helpers::run("o", 90.0, 1.0, o_shots),
    );
    (p, k, o)
}

#[test]
fn the_shot_budget_blocks() {
    let (p, k, o) = budget_pair(99_995, 5);
    assert!(blocked(&p, k, o).contains("at most 99999"));
}

/// Exactly at the edge: 99,999 shots between them (the most a line may
/// hold) combine, one more does not.
#[test]
fn the_shot_budget_is_exact_at_99_999() {
    let (mut p, k, o) = budget_pair(99_994, 5);
    assert_eq!(p.combine_refusal(k, o, None), None);
    ok(p.combine_lines(k, o, &options()));
    assert_eq!(p.line(k).unwrap().shots.len(), 99_999);

    let (p, k, o) = budget_pair(99_994, 6);
    let reason = blocked(&p, k, o);
    assert!(
        reason.contains("would have 100000 shots (at most 99999)"),
        "{reason}"
    );
}

/// Lines that meet nowhere are refused (merge two ends or close a loop
/// first); lines meeting at both ends join at one (the two-line query
/// picks one that needs no reverse, the station menu the clicked end); a
/// line with no shots is absorbed at its one station.
#[test]
fn lines_that_do_not_meet_meet_twice_or_are_empty() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.5);
    let reason = blocked(&p, k, o);
    assert!(reason.contains("do not meet"), "{reason}");
    assert!(reason.contains("close a loop"), "{reason}");
    assert!(p.combine_lines(k, o, &options()).is_err());

    // Both ends side by side with the other's, untied: either join works.
    let mut p = empty();
    let k = line(&mut p, "K", vec![shot(90.0, 10.0, "k0")]);
    let o = line(&mut p, "O", vec![shot(270.0, 10.0, "o0")]);
    let target = at(&p, st(&p, k, 1));
    place(&mut p, o, 0, target, 0.0);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(
        (candidate.end_a, candidate.end_b),
        (LineEnd::End, LineEnd::Start)
    );
    let at_start = p.combine_candidate(StationRef::new(k, 0)).unwrap();
    assert_eq!(at_start.blocked_reason, None);
    assert_eq!(
        (at_start.end_a, at_start.end_b),
        (LineEnd::Start, LineEnd::End)
    );
    let mut clone = p.clone();
    ok(clone.combine_lines(
        k,
        o,
        &CombineLinesOptions {
            join: Some(crate::CombineJoinEnds {
                kept: LineEnd::Start,
                other: LineEnd::End,
            }),
            ..options()
        },
    ));
    assert_eq!(
        stations(&clone, k)[1..],
        stations(&p, k)[..],
        "joined at K's start: O comes first"
    );

    let mut p = empty();
    let k = line(&mut p, "K", vec![shot(90.0, 10.0, "k0")]);
    let target = at(&p, st(&p, k, 1));
    let o = line_at(&mut p, "O", target, Vec::new());
    let before = stations(&p, k);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert!(!candidate.reverses);
    ok(p.combine_lines(k, o, &options()));
    assert_eq!(stations(&p, k), before);
    assert!(p.line(o).is_none());
    assert_valid(&p);
}

/// K's end and O's start side by side (or one station), with all of O `gap`
/// metres deeper than K's end (every shot shifted, so no slope and no
/// horizontal position changes).
fn depth_gap_pair(gap: f64, merged: bool) -> (Project, LineId, LineId) {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, merged, 0.0);
    for s in &mut p.line_mut(o).unwrap().shots {
        s.depth_in += gap;
        s.depth_out += gap;
    }
    assert_valid(&p);
    (p, k, o)
}

/// The join is the same point within 1 mm horizontally; depth never counts.
#[test]
fn the_tolerance_is_one_millimetre_and_only_a_large_depth_gap_blocks() {
    let (p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0009);
    assert_eq!(
        p.combine_candidate_for_lines(k, o).unwrap().blocked_reason,
        None
    );
    let (p, k, o) = pair(LineEnd::End, LineEnd::Start, false, 0.0011);
    assert!(blocked(&p, k, o).contains("do not meet"));

    // The horizontal test ignores depth: the two ends read the depth of
    // different shots. A gap of any size is reported for the dialog, never
    // refused.
    for gap in [0.9, 25.0] {
        let (p, k, o) = depth_gap_pair(gap, false);
        let candidate = p.combine_candidate_for_lines(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "{gap} m");
        assert!(candidate.depth_difference_m.is_some(), "{gap} m");
    }
}

/// The adjusted-view gauge of `line`'s piece: its one `Placement` station
/// (plan 3.7), and where it sits.
fn gauge(p: &Project, line: LineId) -> (StationId, [f64; 2]) {
    let s = st(p, line, 0);
    let found: Vec<StationId> = {
        let topology = p.topology();
        p.placements
            .iter()
            .filter(|pl| topology.connected(pl.station, s))
            .map(|pl| pl.station)
            .collect()
    };
    assert_eq!(found.len(), 1, "one placement per free piece");
    (found[0], at(p, found[0]))
}

/// The component's gauge (adapted: the piece's `Placement` station, plan
/// 3.7, not the lower line id's gauge shot): combining keeps that station
/// the gauge, where it was, whichever line is kept, on a reversed line too.
/// The legacy pairing (start to end, the gauge on the join) and end to start
/// (the gauge away from the join).
#[test]
fn the_gauge_station_survives_keeping_either_line() {
    for (kept_end, other_end) in [
        (LineEnd::Start, LineEnd::End),
        (LineEnd::End, LineEnd::Start),
    ] {
        for keep_lower in [true, false] {
            for reversed in [false, true] {
                let what = format!(
                    "{kept_end:?}-{other_end:?} keep_lower {keep_lower} reversed {reversed}"
                );
                let (mut p, k, o) = pair(kept_end, other_end, true, 0.0);
                if reversed {
                    ok(p.reverse_line(k, false));
                }
                let (g, before) = gauge(&p, k);
                let (kept, other) = if keep_lower { (k, o) } else { (o, k) };

                ok(p.combine_lines(kept, other, &options()));

                let (g_after, after) = gauge(&p, kept);
                assert_eq!(g_after, g, "{what}: the gauge station changed");
                assert!(dist(after, before) < TOL, "{what}: the gauge moved");
                assert_valid(&p);
            }
        }
    }
}

/// Keeping the HIGHER id must keep the lower id's gauge station, which the
/// component used, or the closure-adjusted geometry jumps by the adjustment
/// residual (from #713). Adapted: the Closures Computed View is stream M3
/// and not on this branch; its inputs are asserted instead (every raw
/// position, the gauge station and its position, the closure).
#[test]
fn keeping_the_higher_id_leaves_the_adjusted_geometry_in_place() {
    for other_end in [LineEnd::Start, LineEnd::End] {
        let (mut p, low, high) = pair(LineEnd::End, other_end, true, 0.0);
        let (l1, h2) = (st(&p, low, 1), st(&p, high, 2));
        p.push_closure(l1, Some(low), h2, Some(high), ClosureStrategy::Batch);
        assert_valid(&p);
        assert!(skipped_ids(&p).is_empty(), "the solve takes it");
        let (g, g_at) = gauge(&p, low);
        let before = p.raw_positions();

        ok(p.combine_lines(high, low, &options()));

        assert_eq!(gauge(&p, high), (g, g_at), "{other_end:?}");
        super::edit_test_helpers::assert_still(&before, &p.raw_positions(), TOL, &[]);
        let closure = p.closures[0];
        assert_eq!((closure.a, closure.b), (l1, h2));
        assert_eq!(closure.strategy, ClosureStrategy::Batch);
        assert!(skipped_ids(&p).is_empty());
    }
}

/// A one-shot lower line, reversed, keeps its gauge station through a combine
/// that keeps the higher line (from #713; adapted: the gauge is the piece's
/// `Placement` station, which reversing does not move, plan 3.7).
#[test]
fn a_one_shot_lower_lines_gauge_keeps_its_side() {
    let mut p = empty();
    let low = line(&mut p, "Low", vec![shot(90.0, 4.0, "l0")]);
    ok(p.reverse_line(low, false));
    let high = line(
        &mut p,
        "High",
        vec![shot(0.0, 5.0, "h0"), shot(45.0, 3.0, "h1")],
    );
    let target = at(&p, st(&p, low, 0));
    place(&mut p, high, 2, target, 0.0);
    let (h2, l0) = (st(&p, high, 2), st(&p, low, 0));
    // High's piece moves onto Low's start: Low's placement is the gauge.
    merge(&mut p, h2, l0);
    let (g, before) = gauge(&p, low);
    assert_eq!(g, st(&p, low, 1), "setup: the gauge is Low's far end");

    ok(p.combine_lines(high, low, &options()));

    let (g_after, after) = gauge(&p, high);
    assert_eq!(g_after, g);
    assert!(dist(before, after) < TOL, "{before:?} -> {after:?}");
}

/// The legacy four-shot main line.
fn main_shots() -> Vec<ShotData> {
    vec![
        shot(90.0, 10.0, "m0"),
        shot(45.0, 5.0, "m1"),
        shot(0.0, 7.0, "m2"),
        shot(300.0, 3.0, "m3"),
    ]
}

fn split_options() -> super::super::SplitOptions {
    super::super::SplitOptions {
        name_a: "Main-A".to_string(),
        name_b: "Main-B".to_string(),
        color_a: "#123456".to_string(),
        color_b: "#123456".to_string(),
    }
}

fn split_at_station(p: &mut Project, line: LineId, index: u32) -> LineId {
    ok(p.split_line(
        line,
        crate::SplitAt::Station {
            station_index: index,
        },
        &split_options(),
    ))
}

/// Split then combine at the join gives back the line: same shots, same
/// stations (adapted: the same station ids too, since the halves share the
/// split station, plan 12.2), same names, same references.
#[test]
fn combining_a_fresh_split_at_its_join_restores_the_line() {
    let mut p = empty();
    let main = line(&mut p, "Main", main_shots());
    let s3 = st(&p, main, 3);
    p.station_mut(s3).unwrap().comment = Some("past the cut".to_string());
    let before = stations(&p, main);
    let chain_before = chain(&p, main);
    let names_before = super::edit_test_helpers::names(&p, main);
    let shots_before: Vec<String> = p
        .line(main)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect();
    let second = split_at_station(&mut p, main, 2);
    let candidate = p.combine_candidate(StationRef::new(main, 2)).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.line_b, second);

    ok(p.combine_lines(
        main,
        second,
        &CombineLinesOptions {
            name: "Main".to_string(),
            color: "#123456".to_string(),
            ..options()
        },
    ));

    assert_eq!(p.lines().len(), 1);
    let after = stations(&p, main);
    for (a, b) in after.iter().zip(&before) {
        assert!(dist(*a, *b) < TOL);
    }
    let uuids: Vec<String> = p
        .line(main)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect();
    assert_eq!(uuids, shots_before);
    assert_eq!(chain(&p, main), chain_before);
    assert_eq!(super::edit_test_helpers::names(&p, main), names_before);
    assert_eq!(
        p.station(st(&p, main, 3)).unwrap().comment.as_deref(),
        Some("past the cut")
    );
    assert_valid(&p);
}

/// Combining at (K end, O end) equals reversing O, then combining at
/// (K end, O start).
#[test]
fn combining_end_to_end_equals_reversing_then_combining() {
    let (mut direct, k, o) = pair(LineEnd::End, LineEnd::End, true, 0.0);
    let mut manual = direct.clone();
    ok(direct.combine_lines(k, o, &options()));
    ok(manual.reverse_line(o, true));
    assert!(!manual.combine_candidate_for_lines(k, o).unwrap().reverses);
    ok(manual.combine_lines(k, o, &options()));

    let shots = |p: &Project| -> Vec<(String, f64, f64, f64)> {
        p.line(k)
            .unwrap()
            .shots
            .iter()
            .map(|s| (s.uuid.clone(), s.azimuth_in, s.azimuth_out, s.length))
            .collect()
    };
    assert_eq!(shots(&direct), shots(&manual));
    assert_eq!(chain(&direct, k), chain(&manual, k));
    assert_eq!(stations(&direct, k), stations(&manual, k));
}

/// An unaligned closure between the two lines does not block: after the
/// combine it is a loop closure inside one line, which the solve takes like
/// any other (adapted: the Closures Computed View is stream M3; asserted
/// here: the closure's ends on the combined line, its strategy, that the
/// solve does not skip it, and the raw misclosure it will distribute).
#[test]
fn an_unaligned_closure_between_them_becomes_a_loop_inside_the_line() {
    let mut p = empty();
    let k = line(
        &mut p,
        "K",
        vec![shot(90.0, 10.0, "k0"), shot(0.0, 10.0, "k1")],
    );
    let k2 = st(&p, k, 2);
    let o = line_from(
        &mut p,
        "O",
        k2,
        vec![shot(270.0, 10.0, "o0"), shot(180.0, 9.5, "o1")],
    );
    let (k0, o2) = (st(&p, k, 0), st(&p, o, 2));
    p.push_closure(k0, Some(k), o2, Some(o), ClosureStrategy::Batch);
    assert_valid(&p);
    assert_eq!(
        p.combine_candidate_for_lines(k, o).unwrap().blocked_reason,
        None
    );

    ok(p.combine_lines(k, o, &options()));

    let closure = p.closures[0];
    assert_eq!(closure.a, st(&p, k, 0));
    assert_eq!(closure.b, st(&p, k, 4));
    assert_eq!(closure.strategy, ClosureStrategy::Batch);
    assert!(skipped_ids(&p).is_empty());
    let gap = dist(at(&p, closure.a), at(&p, closure.b));
    assert!((gap - 0.5).abs() < 1e-9, "{gap}");
}

/// `spread` one-shot lines far apart from each other, then `pile` one-shot
/// lines that all start at one point (as unplaced lines all start at the
/// origin), their far ends fanned out so no two meet.
fn spread_then_pile(spread: usize, pile: usize) -> (Project, Vec<LineId>) {
    let mut p = empty();
    for i in 0..spread {
        let x = 100.0 * (i + 1) as f64;
        line_at(
            &mut p,
            "Spread",
            [x, 500.0],
            vec![shot(90.0, 10.0, &format!("s{i}"))],
        );
    }
    let piled = (0..pile)
        .map(|i| {
            line(
                &mut p,
                "Pile",
                vec![shot(0.5 * i as f64, 10.0, &format!("p{i}"))],
            )
        })
        .collect();
    (p, piled)
}

/// Past `MAX_COMBINE_PARTNERS` a pile of line ends is not a junction to pick
/// from (adapted: the legacy end-table probe counter, a performance
/// instrument of the old implementation, is not applicable; the refusal is).
#[test]
fn the_station_query_at_a_pile_offers_no_pick() {
    let (p, piled) = spread_then_pile(400, 200);
    let candidate = p.combine_candidate(StationRef::new(piled[0], 0)).unwrap();
    assert_eq!(
        candidate.blocked_reason.as_deref(),
        Some("200 line ends meet here: select the two to combine in the Sections panel")
    );
}

/// The pair query for two lines that meet alone, among many others far away
/// (adapted: the probe count is not applicable; the answer is).
#[test]
fn the_pair_query_finds_the_join_among_many_lines() {
    let (mut p, _) = spread_then_pile(400, 0);
    let k = line(&mut p, "K", vec![shot(90.0, 10.0, "k0")]);
    let o = line(&mut p, "O", vec![shot(0.0, 10.0, "o0")]);
    let target = at(&p, st(&p, k, 1));
    place(&mut p, o, 0, target, 0.0);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert!(candidate.loose_ends.is_empty());
    assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated);
}

/// The other line's shots get ids from the kept line's `next_shot_id`,
/// which only grows in a session. With no room left in the kept line's id
/// window they would take the next line's ids, so keeping it is refused
/// (with nothing changed) while keeping the other one still works.
#[test]
fn a_kept_line_without_shot_id_room_is_refused_but_the_other_can_be_kept() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    let window_end = (k + 1) * crate::FEATURE_ID_STRIDE;
    p.line_mut(k).unwrap().next_shot_id = window_end - 2;

    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    let reason = candidate.blocked_keeping_a.clone().unwrap();
    assert!(reason.contains("used up its shot numbers"), "{reason}");
    assert_eq!(candidate.blocked_keeping_b, None);
    assert_eq!(p.combine_refusal(k, o, None), Some(reason.clone()));
    let mut copy = p.clone();
    assert_eq!(refused(copy.combine_lines(k, o, &options())), reason);
    assert_eq!(copy, p, "nothing changed");

    ok(p.combine_lines(o, k, &options()));
    let window = o * crate::FEATURE_ID_STRIDE..(o + 1) * crate::FEATURE_ID_STRIDE;
    let ids: Vec<u32> = p.line(o).unwrap().shots.iter().map(|s| s.id).collect();
    assert!(ids.iter().all(|id| window.contains(id)), "{ids:?}");
}

/// Exactly at the edge: the last id handed out is the window's last one.
#[test]
fn the_shot_id_room_check_is_exact_at_the_edge() {
    let (mut p, k, o) = pair(LineEnd::End, LineEnd::Start, true, 0.0);
    let window_end = (k + 1) * crate::FEATURE_ID_STRIDE;
    p.line_mut(k).unwrap().next_shot_id = window_end - 3;
    assert_eq!(
        p.combine_refusal(k, o, None),
        None,
        "three ids left for three shots"
    );
    ok(p.combine_lines(k, o, &options()));
    let last = p.line(k).unwrap().shots.iter().map(|s| s.id).max();
    assert_eq!(last, Some(window_end - 1));
}

/// Main split at its station 2, with a side passage at that station that
/// starts there, passes through it, or starts there and comes back to Main's
/// station 4, past the cut, snapped there. Returns the project, Main, the
/// second half, Side, Side's index at the junction and the junction.
fn split_at_a_junction(shape: &str) -> (Project, LineId, LineId, LineId, u32, StationId) {
    let mut p = empty();
    let main = line(&mut p, "Main", main_shots());
    let junction = st(&p, main, 2);
    let at_index = u32::from(shape == "passes through");
    let side = match shape {
        "passes through" => {
            let side = line(
                &mut p,
                "Side",
                vec![shot(180.0, 6.0, "s0"), shot(180.0, 6.0, "s1")],
            );
            let target = at(&p, junction);
            place(&mut p, side, 1, target, 0.0);
            let s1 = st(&p, side, 1);
            merge(&mut p, s1, junction);
            side
        }
        "loops back" => {
            let side = line_from(&mut p, "Side", junction, vec![shot(180.0, 6.0, "s0")]);
            let (s1, m4) = (st(&p, side, 1), st(&p, main, 4));
            ok(p.append_shot(side, closing_shot(at(&p, s1), at(&p, m4), "s1")));
            let s2 = st(&p, side, 2);
            p.push_closure(s2, Some(side), m4, Some(main), ClosureStrategy::Move);
            assert!(snapped(&p, 0), "setup: a snap");
            side
        }
        _ => line_from(
            &mut p,
            "Side",
            junction,
            vec![shot(180.0, 6.0, "s0"), shot(180.0, 6.0, "s1")],
        ),
    };
    assert_valid(&p);
    let second = split_at_station(&mut p, main, 2);
    (p, main, second, side, at_index, junction)
}

/// Combine the halves of [`split_at_a_junction`] back and check the side
/// passage stays at the junction.
fn combine_the_halves_back(shape: &str) -> Project {
    let (mut p, main, second, side, at_index, junction) = split_at_a_junction(shape);
    let pair_reason = blocked(&p, main, second);
    let station_reason = p
        .combine_candidates(StationRef::new(main, 2))
        .into_iter()
        .find(|c| c.line_b == second)
        .unwrap()
        .blocked_reason
        .unwrap_or_default();
    assert_eq!(
        (pair_reason.as_str(), station_reason.as_str()),
        ("", ""),
        "{shape}"
    );
    ok(p.combine_lines(
        main,
        second,
        &CombineLinesOptions {
            name: "Main".to_string(),
            ..options()
        },
    ));
    assert_eq!(p.line(main).unwrap().shots.len(), 4, "{shape}");
    let others: Vec<StationRef> = p
        .topology()
        .slots_of(junction)
        .iter()
        .copied()
        .filter(|slot| slot.line_id != main)
        .collect();
    assert_eq!(
        others,
        vec![StationRef::new(side, at_index)],
        "{shape}: the side passage stays at the junction"
    );
    assert_eq!(st(&p, main, 2), junction);
    assert_valid(&p);
    p
}

/// A split at a junction (adapted: the halves share the split station, plan
/// 12.2) combines back, and the side passage stays on the joined station (a
/// path through the junction is not a loop to tear): one starting at the
/// junction and one passing through it.
#[test]
fn a_split_at_a_junction_combines_back_whatever_passes_through() {
    for shape in ["starts there", "passes through"] {
        combine_the_halves_back(shape);
    }
}

/// The third shape: the side passage starts at the junction and loops back
/// onto the main line past the cut with a snapped closure, which stays
/// snapped: the loop is the one the survey had before the split.
#[test]
fn a_split_at_a_junction_combines_back_and_a_loop_back_stays_snapped() {
    let p = combine_the_halves_back("loops back");
    assert!(snapped(&p, 0), "the loop the survey had stays snapped");
}

/// O (lowest id) -> K (highest id) end to end; a third line T (middle id)
/// hangs off K's far end and closes a loop back onto K with a real
/// misclosure. The component's gauge is O's piece's placement (O's start).
/// Keep K: O is gone, and the adjusted geometry must not move. Adapted: the
/// Closures Computed View is stream M3, so its inputs are asserted: the gauge
/// station and its position, every raw position, the closure.
#[test]
fn combine_keeps_the_component_anchor_with_a_third_line() {
    let mut p = empty();
    let o = line(&mut p, "O", vec![shot(90.0, 10.0, "o0")]);
    let t = line_at(
        &mut p,
        "T",
        [50.0, 50.0],
        vec![
            shot(0.0, 10.0, "t0"),
            shot(270.0, 10.0, "t1"),
            shot(180.0, 9.0, "t2"),
        ],
    );
    let k = line_at(
        &mut p,
        "K",
        [-50.0, -50.0],
        vec![shot(90.0, 10.0, "k0"), shot(90.0, 10.0, "k1")],
    );
    assert!(o < t && t < k, "ids {o} {t} {k}");
    let o1 = st(&p, o, 1);
    let target = at(&p, o1);
    place(&mut p, k, 0, target, 0.0);
    let k0 = st(&p, k, 0);
    merge(&mut p, k0, o1);
    let k2 = st(&p, k, 2);
    let target = at(&p, k2);
    place(&mut p, t, 0, target, 0.0);
    let t0 = st(&p, t, 0);
    merge(&mut p, t0, k2);
    let (t3, k1) = (st(&p, t, 3), st(&p, k, 1));
    p.push_closure(
        t3,
        Some(t),
        k1,
        Some(k),
        ClosureStrategy::LeastSquaresWeighted,
    );
    assert_valid(&p);
    assert_eq!(
        p.combine_candidate_for_lines(k, o).unwrap().blocked_reason,
        None
    );
    let (g, g_at) = gauge(&p, k);
    assert_eq!(g, st(&p, o, 0), "setup: the gauge is O's start");
    let before = p.raw_positions();

    ok(p.combine_lines(k, o, &options()));

    assert_eq!(gauge(&p, k), (g, g_at));
    super::edit_test_helpers::assert_still(&before, &p.raw_positions(), TOL, &[]);
    let closure = p.closures[0];
    assert_eq!((closure.a, closure.b), (t3, k1));
    assert_eq!(closure.strategy, ClosureStrategy::LeastSquaresWeighted);
    assert!(skipped_ids(&p).is_empty());
}

/// S is split at station 2; a line X at the cut station keeps the halves in
/// one piece and closes a loop onto S's far end, where the gauge sits
/// (legacy: S's reverse parity put it there; here S's piece's placement is
/// set on S's far end). The station the adjustment holds goes to the second
/// half; the adjusted geometry must not move (adapted as above: the gauge
/// and every raw position).
#[test]
fn split_keeps_the_component_anchor() {
    let mut p = empty();
    let s = line(
        &mut p,
        "S",
        super::edit_test_helpers::run("s", 90.0, 10.0, 4),
    );
    let s2 = st(&p, s, 2);
    let x = line_from(
        &mut p,
        "X",
        s2,
        vec![
            shot(0.0, 10.0, "x0"),
            shot(90.0, 20.0, "x1"),
            shot(180.0, 9.0, "x2"),
        ],
    );
    let s4 = st(&p, s, 4);
    let here = at(&p, s4);
    p.set_placement(s4, here[0], here[1]).unwrap();
    let x3 = st(&p, x, 3);
    p.push_closure(
        x3,
        Some(x),
        s4,
        Some(s),
        ClosureStrategy::LeastSquaresWeighted,
    );
    assert_valid(&p);
    let (g, g_at) = gauge(&p, s);
    assert_eq!(g, s4);
    let before = p.raw_positions();

    let second = split_at_station(&mut p, s, 2);

    assert_eq!(st(&p, second, 2), s4, "the gauge went to the second half");
    assert_eq!(gauge(&p, s), (g, g_at));
    super::edit_test_helpers::assert_still(&before, &p.raw_positions(), TOL, &[]);
    assert_eq!(p.closures.len(), 1);
}

/// Two side-by-side, unmerged ends each read their own shot's depth;
/// combined, the joined station reads the average of the shot before and
/// after it, so each end moves by half the gap. Above 5 cm the candidate
/// reports the gap for the dialog to show, however large: the user decides.
/// A merged join already averages, so it reports nothing.
#[test]
fn a_depth_gap_at_an_unmerged_join_is_reported_never_refused() {
    const { assert!(crate::COMBINE_DEPTH_NOTICE_M < 0.1) };

    let (mut p, k, o) = depth_gap_pair(0.3, false);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    let gap = candidate.depth_difference_m.unwrap();
    assert!((gap - 0.3).abs() < 1e-9, "{gap}");
    ok(p.combine_lines(k, o, &options()));
    let joined = p.raw_positions().z(st(&p, k, 2)).unwrap();
    assert!((joined - 0.15).abs() < 1e-9, "{joined}");

    let (p, k, o) = depth_gap_pair(0.03, false);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.depth_difference_m, None, "below the notice");

    let (mut p, k, o) = depth_gap_pair(1.5, false);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert!((candidate.depth_difference_m.unwrap() - 1.5).abs() < 1e-9);
    ok(p.combine_lines(k, o, &options()));
    let joined = p.raw_positions().z(st(&p, k, 2)).unwrap();
    assert!((joined - 0.75).abs() < 1e-9, "{joined}");

    let (p, k, o) = depth_gap_pair(1.5, true);
    let candidate = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(
        candidate.blocked_reason, None,
        "a merged join already averages"
    );
    assert_eq!(candidate.depth_difference_m, None);
}

/// A reopen recomputes every position from the stored model (a JSON round
/// trip stands in for save and open).
fn reopened(p: &Project) -> Project {
    let mut copy: Project = serde_json::from_str(&serde_json::to_string(p).unwrap()).unwrap();
    copy.assign_session_shot_ids();
    copy
}

/// #740: two line ends side by side within the tolerance (0.6 mm apart,
/// not merged). The combine re-hangs the other line from the kept end, so
/// it moves by the residual, and what is joined to it moves with it (here
/// one station by construction); a reopen moves nothing.
#[test]
fn a_side_by_side_combine_keeps_merged_stations_together() {
    let mut p = empty();
    let m = line(&mut p, "M", vec![shot(0.0, 10.0, "m0")]);
    let o = line_at(&mut p, "O", [-30.0, 30.0], vec![shot(90.0, 10.0, "o0")]);
    let k = line_at(&mut p, "K", [-20.0006, 0.0], vec![shot(90.0, 10.0, "k0")]);
    let (o1, m0) = (st(&p, o, 1), st(&p, m, 0));
    let target = at(&p, m0);
    place(&mut p, o, 1, target, 0.0);
    merge(&mut p, o1, m0);
    let gap = dist(at(&p, st(&p, k, 1)), at(&p, st(&p, o, 0)));
    assert!(
        gap > 1e-4 && gap < 1e-3,
        "setup: the ends are {gap} m apart"
    );
    let before = p.raw_positions();
    let m1 = st(&p, m, 1);

    let report = ok(p.combine_lines(k, o, &options()));

    assert_eq!(report.moved_lines, vec![o, m]);
    // O's far end is K's station 2 now, the station M starts at.
    assert_eq!(st(&p, k, 2), m0);
    assert_eq!(p.topology().slot_on(m0, m), Some(StationRef::new(m, 0)));
    let after = p.raw_positions();
    let shift = [-0.0006, 0.0];
    for s in [m0, m1] {
        let b = xy(&before, s);
        super::edit_test_helpers::near(
            xy(&after, s),
            [b[0] + shift[0], b[1]],
            1e-6,
            "M moved with O",
        );
    }
    super::edit_test_helpers::assert_still(&before, &after, TOL, &[m0, m1]);
    super::edit_test_helpers::assert_still(&after, &reopened(&p).raw_positions(), 0.0, &[]);
}

/// #755: K's end and O's start side by side, O's start moved 0.6 mm east by
/// hand and T starting at it. The joined station keeps K's hand move (none)
/// and drops O's; T moves with the joined station, and the plan says so; a
/// reopen moves nothing.
#[test]
fn a_hand_moved_join_station_keeps_its_branch_merged() {
    let mut p = empty();
    let k = line(&mut p, "K", vec![shot(90.0, 10.0, "k0")]);
    let o = line_at(&mut p, "O", [10.0, 0.0], vec![shot(90.0, 10.0, "o0")]);
    let o0 = st(&p, o, 0);
    hand_move(&mut p, o, o0, [0.0006, 0.0, 0.0]);
    let t = line_from(&mut p, "T", o0, vec![shot(0.0, 5.0, "t0")]);
    let k1 = st(&p, k, 1);
    let gap = dist(at(&p, k1), at(&p, o0));
    assert!((gap - 0.0006).abs() < 1e-9, "setup: {gap} m");
    let k0_at = at(&p, st(&p, k, 0));

    let report = ok(p.combine_lines(k, o, &options()));

    // O's start is K's station 1 now, and T still starts on it.
    assert_eq!(p.topology().slot_on(k1, t), Some(StationRef::new(t, 0)));
    let joined = report.moved_with_joined_station.unwrap();
    assert_eq!((joined.station, joined.lines), (o0, vec![t]));
    assert!((joined.shift_m - 0.0006).abs() < 1e-9, "{}", joined.shift_m);
    assert_eq!(at(&p, st(&p, k, 0)), k0_at);
    assert!(dist(at(&p, k1), at(&p, st(&p, t, 0))) < 1e-12);
    super::edit_test_helpers::assert_still(
        &p.raw_positions(),
        &reopened(&p).raw_positions(),
        0.0,
        &[],
    );
    assert_valid(&p);
}
