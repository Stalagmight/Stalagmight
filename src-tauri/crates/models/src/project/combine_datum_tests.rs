//! Combine Sections where zero datums hold both sides and the two join
//! stations are one point (#754): two surveys from one entrance, each with
//! its own datum on the same GPS position. Nothing moves, but the joined
//! station unites the two sides' merge groups, so the merge from one section
//! to its datum side becomes a loop closure, the same one a shift would tear:
//! the other section's, unless it carries a datum itself. Every join end,
//! either section kept, a datum on the join section itself or on a section
//! merged to it (at the join or at the far end); one datum alone tears
//! nothing; a merged join never unites two datum groups. Each combine is
//! checked against every invariant the sweeps hold a combine to
//! (`combine_closure_tests::check_combined`), the datums staying apart
//! included.

use super::combine_closure_tests::{
    add, check_combined, check_datums_stay_apart, dist, options, shot, xy,
};
use crate::{
    ClosureStrategy, CombineJoinEnds, CombineJoinKind, CombinePlan, GpsCoordinates, Project,
    SectionEnd, SectionId, StationRef, TornTieKind, TornTieReason, ZeroDatum,
};

fn gps() -> GpsCoordinates {
    GpsCoordinates {
        latitude: 20.0,
        longitude: -87.0,
        depth_reference: 0.0,
    }
}

fn end_index(project: &Project, id: SectionId, end: SectionEnd) -> u32 {
    match end {
        SectionEnd::Start => 0,
        SectionEnd::End => project.get_section(id).unwrap().shots.len() as u32,
    }
}

fn far(end: SectionEnd) -> SectionEnd {
    match end {
        SectionEnd::Start => SectionEnd::End,
        SectionEnd::End => SectionEnd::Start,
    }
}

/// Move `id` so its station `index` sits exactly on `target`.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64)) {
    let now = xy(project, StationRef::new(id, index));
    project.adjust_section_offset(id, target.0 - now.0, target.1 - now.1);
}

/// Hold `at` with a zero datum where it is (`set_zero_datum` would move its
/// side onto the GPS position).
fn hold(project: &mut Project, at: StationRef) {
    let station_ref = project.key_for_ref(at).unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref,
        gps: gps(),
    });
}

/// Where a zero datum holds one side of the join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// Nothing holds it.
    Free,
    /// The join section's own join station: the section carries the datum.
    OnTheJoin,
    /// A one-shot section D whose start is merged at the join station.
    MergedAtTheJoin,
    /// A one-shot section D whose start is merged at the far end.
    MergedAtTheFarEnd,
}

/// K and O meet at `k_end` and `o_end`, on one point (float dust apart),
/// unmerged; each side held as its `Held` says.
struct Entrance {
    project: Project,
    k: SectionId,
    o: SectionId,
    k_end: SectionEnd,
    o_end: SectionEnd,
    /// The section merged to K or O that carries its side's datum.
    dk: Option<SectionId>,
    d_o: Option<SectionId>,
}

impl Entrance {
    fn join_station(&self, section: SectionId) -> StationRef {
        let end = if section == self.k {
            self.k_end
        } else {
            self.o_end
        };
        StationRef::new(section, end_index(&self.project, section, end))
    }
}

/// K runs east then east-north-east from (100, -40), O west then
/// west-south-west, placed so O's `o_end` is exactly K's `k_end` (plus
/// `residual`, metres east and north). Merges come first and datums last,
/// held where they are, so nothing moves the join apart.
fn entrance(
    k_end: SectionEnd,
    o_end: SectionEnd,
    k_held: Held,
    o_held: Held,
    residual: (f64, f64),
) -> Entrance {
    let mut project = Project::new("Entrance".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(80.0, 5.0)]);
    let o = add(&mut project, "O", vec![shot(270.0, 8.0), shot(260.0, 4.0)]);
    project.set_section_offset(k, (100.0, -40.0));
    let at = xy(&project, StationRef::new(k, end_index(&project, k, k_end)));
    let o_join = end_index(&project, o, o_end);
    place(
        &mut project,
        o,
        o_join,
        (at.0 + residual.0, at.1 + residual.1),
    );
    let mut datums = Vec::new();
    let mut held_by = |project: &mut Project, s: SectionId, end: SectionEnd, held: Held| {
        let station = |project: &Project, end| StationRef::new(s, end_index(project, s, end));
        match held {
            Held::Free => None,
            Held::OnTheJoin => {
                datums.push(station(project, end));
                None
            }
            Held::MergedAtTheJoin | Held::MergedAtTheFarEnd => {
                let at = if held == Held::MergedAtTheJoin {
                    station(project, end)
                } else {
                    station(project, far(end))
                };
                let name = format!("D{s}");
                let d = add(project, &name, vec![shot(0.0, 5.0)]);
                let p = xy(project, at);
                place(project, d, 0, p);
                // D's group moves onto the station, not the other way.
                project.merge_stations(StationRef::new(d, 0), at).unwrap();
                datums.push(StationRef::new(d, 1));
                Some(d)
            }
        }
    };
    let dk = held_by(&mut project, k, k_end, k_held);
    let d_o = held_by(&mut project, o, o_end, o_held);
    for at in datums {
        hold(&mut project, at);
    }
    Entrance {
        project,
        k,
        o,
        k_end,
        o_end,
        dk,
        d_o,
    }
}

/// Combine `e` keeping `kept`, checked against every invariant
/// `check_combined` holds a combine to. Returns the plan the station menu
/// offered and the combined project.
fn combine(e: &Entrance, kept: SectionId, what: &str) -> (CombinePlan, Project) {
    let (other, join) = if kept == e.k {
        (
            e.o,
            CombineJoinEnds {
                kept: e.k_end,
                other: e.o_end,
            },
        )
    } else {
        (
            e.k,
            CombineJoinEnds {
                kept: e.o_end,
                other: e.k_end,
            },
        )
    };
    let candidate = e
        .project
        .combine_candidate_joining(kept, other, Some(join))
        .unwrap();
    assert_eq!(candidate.blocked_reason, None, "{what}");
    let plan = if candidate.section_a == kept {
        candidate.plan_keeping_a.clone()
    } else {
        candidate.plan_keeping_b.clone()
    };
    let mut after = e.project.clone();
    after
        .combine_sections(kept, other, &options(Some(join)))
        .unwrap_or_else(|err| panic!("{what}: {err}"));
    check_combined(&e.project, &after, &candidate, kept, what, &[]);
    (plan, after)
}

/// Every station of `id`, measured.
fn stations(project: &Project, id: SectionId) -> Vec<(f64, f64)> {
    let n = project.get_section(id).unwrap().shots.len() as u32;
    (0..=n)
        .map(|i| xy(project, StationRef::new(id, i)))
        .collect()
}

const ENDS: [SectionEnd; 2] = [SectionEnd::Start, SectionEnd::End];

/// The issue's shape on every pairing of the two ends, either section kept,
/// and every way a datum can hold each side (the two join sections never
/// both carrying one: that is refused). Nothing moves, and exactly one merge
/// becomes a loop closure for the datum: the other section's merge to its
/// datum side, or the kept section's when the other carries the datum
/// itself. The datums end in two merge groups and every station stays where
/// it was.
#[test]
fn datums_on_both_sides_of_one_point_tear_one_merge_and_move_nothing() {
    let holds = [
        Held::OnTheJoin,
        Held::MergedAtTheJoin,
        Held::MergedAtTheFarEnd,
    ];
    let mut combined = 0;
    for k_end in ENDS {
        for o_end in ENDS {
            for k_held in holds {
                for o_held in holds {
                    let e = entrance(k_end, o_end, k_held, o_held, (0.0, 0.0));
                    let gap = dist(
                        xy(&e.project, e.join_station(e.k)),
                        xy(&e.project, e.join_station(e.o)),
                    );
                    assert!(gap <= 2.0 * Project::OFFSET_DUST_M, "setup: {gap} m apart");
                    if k_held == Held::OnTheJoin && o_held == Held::OnTheJoin {
                        let candidate = e.project.combine_candidate_for_sections(e.k, e.o);
                        let reason = candidate.and_then(|c| c.blocked_reason);
                        assert!(
                            reason.is_some_and(|r| r.contains("zero datum")),
                            "both carry a datum: refused"
                        );
                        continue;
                    }
                    for kept in [e.k, e.o] {
                        let what = format!(
                            "K {k_end:?} held {k_held:?}, O {o_end:?} held {o_held:?}, keep {}",
                            if kept == e.k { "K" } else { "O" }
                        );
                        let other = if kept == e.k { e.o } else { e.k };
                        let other_held = if kept == e.k { o_held } else { k_held };
                        // The section whose merge tears, and its datum side.
                        let tears = if other_held == Held::OnTheJoin {
                            kept
                        } else {
                            other
                        };
                        let datum_side = if tears == e.k { e.dk } else { e.d_o };
                        let datum_side = datum_side.unwrap();
                        let (plan, after) = combine(&e, kept, &what);
                        assert_eq!(plan.moving_section, None, "{what}");
                        assert!(plan.moved_sections.is_empty(), "{what}");
                        assert_eq!(plan.kept_moves_because, None, "{what}");
                        assert!(plan.shift_m.abs() < 1e-12, "{what}");
                        assert_eq!(plan.torn_ties.len(), 1, "{what}: {plan:?}");
                        let torn = plan.torn_ties[0];
                        assert_eq!(torn.kind, TornTieKind::Merge, "{what}");
                        assert_eq!(torn.reason, TornTieReason::ZeroDatum, "{what}");
                        let mut sections = [torn.station_a.section_id, torn.station_b.section_id];
                        sections.sort_unstable();
                        let mut want = [tears, datum_side];
                        want.sort_unstable();
                        assert_eq!(sections, want, "{what}");
                        // The torn merge is a closure now, between the same
                        // two stations (the combined section's one is K's).
                        assert!(after.over_anchored_merge_components().is_empty(), "{what}");
                        let datum_end = if torn.station_a.section_id == datum_side {
                            torn.station_a
                        } else {
                            torn.station_b
                        };
                        assert!(
                            after.loop_closures.iter().any(|c| {
                                c.strategy == ClosureStrategy::Batch
                                    && [&c.station_a, &c.station_b]
                                        .iter()
                                        .any(|k| after.resolve_key(k) == Some(datum_end))
                            }),
                            "{what}: no closure at {datum_end:?}"
                        );
                        for held in [e.dk, e.d_o].into_iter().flatten() {
                            assert_eq!(stations(&after, held), stations(&e.project, held));
                        }
                        combined += 1;
                    }
                }
            }
        }
    }
    assert_eq!(combined, 4 * 8 * 2);
}

/// The rule does not depend on the gap: the same shape with the two ends
/// 0.5 mm apart (a residual, which moves a side, #740) tears the same merge
/// for the same datum, and moves the section whose merge tore.
#[test]
fn the_same_merge_tears_with_a_residual_as_without_one() {
    for (k_held, o_held) in [
        (Held::MergedAtTheJoin, Held::OnTheJoin),
        (Held::MergedAtTheJoin, Held::MergedAtTheJoin),
        (Held::MergedAtTheFarEnd, Held::MergedAtTheJoin),
        (Held::OnTheJoin, Held::MergedAtTheFarEnd),
    ] {
        let flush = entrance(
            SectionEnd::Start,
            SectionEnd::End,
            k_held,
            o_held,
            (0.0, 0.0),
        );
        let apart = entrance(
            SectionEnd::Start,
            SectionEnd::End,
            k_held,
            o_held,
            (0.0004, -0.0003),
        );
        for keep_k in [true, false] {
            let what = format!("K held {k_held:?}, O held {o_held:?}, keep K {keep_k}");
            let pick = |e: &Entrance| if keep_k { e.k } else { e.o };
            let (in_place, _) = combine(&flush, pick(&flush), &what);
            let (shifted, _) = combine(&apart, pick(&apart), &what);
            assert_eq!(in_place.torn_ties, shifted.torn_ties, "{what}");
            let tore = in_place.torn_ties[0];
            let section = [tore.station_a.section_id, tore.station_b.section_id]
                .into_iter()
                .find(|s| *s == flush.k || *s == flush.o)
                .unwrap();
            assert_eq!(in_place.moving_section, None, "{what}");
            assert_eq!(shifted.moving_section, Some(section), "{what}");
        }
    }
}

/// One datum at the entrance (D merged at K's start, or K carrying it) and
/// none on O's side: nothing to keep apart, so nothing tears and nothing
/// moves, as before #754; O joins the one datum group.
#[test]
fn one_datum_at_a_shared_point_tears_nothing() {
    for held in [
        Held::OnTheJoin,
        Held::MergedAtTheJoin,
        Held::MergedAtTheFarEnd,
    ] {
        for (k_held, o_held) in [(held, Held::Free), (Held::Free, held)] {
            let e = entrance(
                SectionEnd::Start,
                SectionEnd::Start,
                k_held,
                o_held,
                (0.0, 0.0),
            );
            for kept in [e.k, e.o] {
                let what = format!("K held {k_held:?}, O held {o_held:?}, keep {kept}");
                let (plan, after) = combine(&e, kept, &what);
                assert_eq!(plan.moving_section, None, "{what}");
                assert!(plan.torn_ties.is_empty(), "{what}: {plan:?}");
                assert_eq!(
                    after.station_merges.len(),
                    e.project.station_merges.len(),
                    "{what}"
                );
                let datum = after.zero_datums[0].station_ref.clone();
                let datum = after.resolve_key(&datum).unwrap();
                assert!(after
                    .get_merged_section_ids(datum.section_id)
                    .contains(&kept));
            }
        }
    }
}

/// Merged ends are one merge group already, so a second datum on their sides
/// is refused before any combine (`set_zero_datum`, `merge_stations`), and
/// the combine of a merged join with one datum (on D, merged at the join or
/// at K's far end) tears nothing. A file can still hold two datums in one
/// merge group (the integrity check reports it): combining there adds no
/// datum to any group, whichever section is kept.
#[test]
fn a_merged_join_never_unites_two_datum_groups() {
    for k_held in [Held::MergedAtTheJoin, Held::MergedAtTheFarEnd] {
        let mut e = entrance(
            SectionEnd::Start,
            SectionEnd::End,
            k_held,
            Held::Free,
            (0.0, 0.0),
        );
        let (k_join, o_join) = (e.join_station(e.k), e.join_station(e.o));
        e.project.merge_stations(o_join, k_join).unwrap();
        let o_far = StationRef::new(e.o, 0);
        let refused = e.project.clone().set_zero_datum(o_far, gps());
        assert!(refused.is_err(), "{k_held:?}: a second datum in one group");
        for kept in [e.k, e.o] {
            let what = format!("merged, K held {k_held:?}, keep {kept}");
            let candidate = e.project.combine_candidate_for_sections(e.k, e.o).unwrap();
            assert_eq!(candidate.join_kind, CombineJoinKind::Merged, "{what}");
            let (plan, after) = combine(&e, kept, &what);
            assert!(plan.torn_ties.is_empty(), "{what}: {plan:?}");
            assert!(after.over_anchored_merge_components().is_empty(), "{what}");
        }

        // The same with a second datum forced onto O (as a file can hold it).
        let mut forced = e.project.clone();
        hold(&mut forced, o_far);
        assert_eq!(forced.over_anchored_merge_components().len(), 1);
        for (kept, other, join) in [
            (
                e.k,
                e.o,
                CombineJoinEnds {
                    kept: SectionEnd::Start,
                    other: SectionEnd::End,
                },
            ),
            (
                e.o,
                e.k,
                CombineJoinEnds {
                    kept: SectionEnd::End,
                    other: SectionEnd::Start,
                },
            ),
        ] {
            let what = format!("merged, forced, K held {k_held:?}, keep {kept}");
            let mut after = forced.clone();
            after
                .combine_sections(kept, other, &options(Some(join)))
                .unwrap_or_else(|err| panic!("{what}: {err}"));
            check_datums_stay_apart(&forced, &after, &what);
            assert!(after.over_anchored_merge_components().len() <= 1, "{what}");
        }
    }
}
