//! Combine Sections on the model: the four pairings, references, the join,
//! every blocked case, the tolerance, the gauge and the retired-section map.

use chrono::Utc;

use crate::{
    AzimuthReference, ClosureStrategy, CombinePick, CombineSectionsOptions, CorrectionSource,
    GpsCoordinates, LoopClosure, NorthReference, Person, Project, SectionData, SectionEnd,
    SectionId, ShotData, SplitSectionOptions, SplitSide, StationKey, StationMerge, StationRef,
    SurveyCredit, ZeroDatum,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        left: Some(1.0),
        right: Some(2.0),
        ..Default::default()
    }
}

fn add(project: &mut Project, name: &str, shots: Vec<ShotData>) -> SectionId {
    project
        .add_section(SectionData {
            name: Some(name.to_string()),
            color: "#123456".to_string(),
            shots,
            ..Default::default()
        })
        .id
}

fn options() -> CombineSectionsOptions {
    CombineSectionsOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
    }
}

fn xy(project: &Project, station: StationRef) -> (f64, f64) {
    let c = project.compute_station_raw_coords(station).unwrap();
    (c.x, c.y)
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn end_index(project: &Project, id: SectionId, end: SectionEnd) -> u32 {
    match end {
        SectionEnd::Start => 0,
        SectionEnd::End => project.get_section(id).unwrap().shots.len() as u32,
    }
}

/// Move `id` so its station `index` sits at `target` plus `error` (metres,
/// east), leaving the two unmerged.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64), error: f64) {
    let now = xy(project, StationRef::new(id, index));
    project.adjust_section_offset(id, target.0 - now.0 + error, target.1 - now.1);
}

/// K: two shots east from the origin, offset; O: three shots heading north
/// then west, placed so its `other_end` meets K's `kept_end` (merged or
/// co-located within `error`).
fn pair(
    kept_end: SectionEnd,
    other_end: SectionEnd,
    merged: bool,
    error: f64,
) -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Combine".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(80.0, 5.0)]);
    let o = add(
        &mut project,
        "O",
        vec![shot(0.0, 4.0), shot(300.0, 6.0), shot(270.0, 3.0)],
    );
    project.set_section_offset(k, (100.0, -40.0));
    let join_k = StationRef::new(k, end_index(&project, k, kept_end));
    let join_o = StationRef::new(o, end_index(&project, o, other_end));
    if merged {
        project.merge_stations(join_k, join_o).unwrap();
    } else {
        let target = xy(&project, join_k);
        place(&mut project, o, join_o.station_index, target, error);
    }
    (project, k, o)
}

/// Every station of `id`, as world xy.
fn stations(project: &Project, id: SectionId) -> Vec<(f64, f64)> {
    let n = project.get_section(id).unwrap().shots.len() as u32;
    (0..=n)
        .map(|i| xy(project, StationRef::new(id, i)))
        .collect()
}

/// All four pairings, merged and co-located: K stays exactly in place, O's
/// stations move at most the tolerance, the far ends line up and the
/// combined section holds both chains in order.
#[test]
fn all_four_pairings_keep_every_station_in_place() {
    for kept_end in [SectionEnd::Start, SectionEnd::End] {
        for other_end in [SectionEnd::Start, SectionEnd::End] {
            for merged in [true, false] {
                let what = format!("{kept_end:?}-{other_end:?} merged={merged}");
                let (mut project, k, o) = pair(kept_end, other_end, merged, 0.0004);
                let before_k = stations(&project, k);
                let mut before_o = stations(&project, o);
                let candidate = project.combine_candidate_for_sections(k, o).unwrap();
                assert_eq!(candidate.blocked_reason, None, "{what}");
                assert_eq!(candidate.merged, merged, "{what}");
                assert_eq!(candidate.reverses, kept_end == other_end, "{what}");

                project.combine_sections(k, o, &options()).unwrap();

                assert!(project.get_section(o).is_none(), "{what}: O is gone");
                let after = stations(&project, k);
                assert_eq!(after.len(), 2 + 3 + 1, "{what}");
                // O's stations in K's direction of travel, joined at K's end.
                if kept_end == other_end {
                    before_o.reverse();
                }
                let (k_part, o_part) = if kept_end == SectionEnd::End {
                    (&after[..3], &after[2..])
                } else {
                    (&after[3..], &after[..4])
                };
                for (i, (a, b)) in k_part.iter().zip(&before_k).enumerate() {
                    assert!(dist(*a, *b) < 1e-9, "{what}: K station {i} moved");
                }
                for (i, (a, b)) in o_part.iter().zip(&before_o).enumerate() {
                    assert!(
                        dist(*a, *b) <= 0.001,
                        "{what}: O station {i} moved {}",
                        dist(*a, *b)
                    );
                }
                let section = project.get_section(k).unwrap();
                assert_eq!(section.name.as_deref(), Some("Joined"));
                assert_eq!(section.color, "#abcdef");
                assert!(project.check_structural_integrity().is_empty(), "{what}");
                assert!(
                    project.station_merges.is_empty(),
                    "{what}: the join merge is gone"
                );
            }
        }
    }
}

/// Give every shot of `id` a distinct credit (one shot left uncredited).
fn credit_every_shot(project: &mut Project, id: SectionId, who: &str) {
    for (i, shot) in project
        .get_section_mut(id)
        .unwrap()
        .shots
        .iter_mut()
        .enumerate()
    {
        shot.set_credit(if i == 1 {
            SurveyCredit::default()
        } else {
            SurveyCredit {
                surveyors: Person::named(&format!("{who}{i}")).into_iter().collect(),
                explorers: Person::named(who).into_iter().collect(),
                date: chrono::NaiveDate::from_ymd_opt(2026, 1 + i as u32, 5),
            }
        });
    }
}

/// Every shot's credit, keyed by its uuid.
fn credits(project: &Project) -> std::collections::BTreeMap<String, Option<SurveyCredit>> {
    project
        .sections
        .iter()
        .flat_map(|s| s.shots.iter())
        .map(|s| (s.uuid.clone().unwrap(), s.credit.as_deref().cloned()))
        .collect()
}

/// Credit lives on the shot: every shot keeps its own exactly, the empty one
/// included, through all four pairings (two of them reverse the other
/// section) keeping either section.
#[test]
fn every_shot_keeps_its_exact_credit_through_every_pairing() {
    for kept_end in [SectionEnd::Start, SectionEnd::End] {
        for other_end in [SectionEnd::Start, SectionEnd::End] {
            for keep_lower in [true, false] {
                let (mut project, k, o) = pair(kept_end, other_end, true, 0.0);
                credit_every_shot(&mut project, k, "K");
                credit_every_shot(&mut project, o, "O");
                let before = credits(&project);
                let (kept, other) = if keep_lower { (k, o) } else { (o, k) };

                project.combine_sections(kept, other, &options()).unwrap();

                assert_eq!(
                    credits(&project),
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
fn a_reversed_other_section_is_reciprocated_and_swaps_walls_when_asked() {
    for swap_lr in [true, false] {
        let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::End, true, 0.0);
        project
            .combine_sections(
                k,
                o,
                &CombineSectionsOptions {
                    swap_lr,
                    ..options()
                },
            )
            .unwrap();
        let shots = &project.get_section(k).unwrap().shots;
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

/// Shot ids: K keeps its own, O's shots get fresh ones from K's counter; uuids
/// all travel.
#[test]
fn the_kept_section_keeps_its_shot_ids_and_every_uuid_travels() {
    let (mut project, k, o) = pair(SectionEnd::Start, SectionEnd::End, true, 0.0);
    let k_ids: Vec<u32> = project
        .get_section(k)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();
    let mut uuids: Vec<String> = project
        .get_section(o)
        .unwrap()
        .shots
        .iter()
        .chain(&project.get_section(k).unwrap().shots)
        .map(|s| s.uuid.clone().unwrap())
        .collect();
    project.combine_sections(k, o, &options()).unwrap();
    let section = project.get_section(k).unwrap();
    let ids: Vec<u32> = section.shots.iter().map(|s| s.id).collect();
    assert_eq!(&ids[3..], k_ids.as_slice(), "K's shots keep their ids");
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len());
    let mut after: Vec<String> = section
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect();
    after.sort();
    uuids.sort();
    assert_eq!(after, uuids);
}

/// Prepending moves K's station 0: everything keyed `SectionStart` on K has to
/// follow it to its new index.
#[test]
fn a_prepend_rekeys_the_kept_sections_start() {
    let (mut project, k, o) = pair(SectionEnd::Start, SectionEnd::End, false, 0.0);
    let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
    let start = StationRef::new(k, 0);
    project.station_comments.insert(
        StationKey::new(k, crate::StationAnchor::SectionStart),
        "K start".to_string(),
    );
    push_closure(&mut project, start, StationRef::new(c, 1), false);

    project.combine_sections(k, o, &options()).unwrap();

    let joined = StationRef::new(k, 3);
    assert_eq!(
        project.station_comment_for_ref(joined).map(String::as_str),
        Some("K start")
    );
    let closure = &project.loop_closures[0];
    assert_eq!(project.resolve_key(&closure.station_a), Some(joined));
    assert!(project
        .station_comment_for_ref(StationRef::new(k, 0))
        .is_none());
}

fn push_closure(project: &mut Project, a: StationRef, b: StationRef, aligned: bool) {
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
        path_length: 1.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: aligned,
    });
}

fn push_merge(project: &mut Project, a: StationRef, b: StationRef) {
    let (station_a, station_b) = (
        project.key_for_ref(a).unwrap(),
        project.key_for_ref(b).unwrap(),
    );
    project.record_station_merge(StationMerge {
        station_a,
        station_b,
        merged_at: Utc::now(),
    });
}

fn gps() -> GpsCoordinates {
    GpsCoordinates {
        latitude: 20.0,
        longitude: -87.0,
        depth_reference: 0.0,
    }
}

/// Every kind of reference on O (and one on a third section pointing at O)
/// lands on the same physical station of the combined section, for a pairing
/// that reverses O.
#[test]
fn every_reference_on_the_other_section_follows_its_station() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::End, true, 0.0);
    let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
    let d = add(&mut project, "D", vec![shot(180.0, 5.0)]);
    // O is three shots; reversed and appended at K's end (index 2), so O's
    // station j lands on K station 2 + (3 - j).
    let o_mid = StationRef::new(o, 1);
    let o_far = StationRef::new(o, 0);
    project
        .merge_stations(o_mid, StationRef::new(c, 0))
        .unwrap();
    push_closure(&mut project, o_far, StationRef::new(d, 1), false);
    project
        .set_zero_datum(StationRef::new(o, 2), gps())
        .unwrap();
    for (station, text) in [(o_mid, "mid"), (o_far, "far")] {
        let key = project.key_for_ref(station).unwrap();
        project
            .station_comments
            .insert(key.clone(), format!("comment {text}"));
        project
            .station_names
            .insert(key.clone(), format!("name {text}"));
        project.station_offsets.insert(key, (0.25, 0.0, 0.0));
    }
    let before_mid = xy(&project, o_mid);

    project.combine_sections(k, o, &options()).unwrap();

    let mid = StationRef::new(k, 4);
    let far = StationRef::new(k, 5);
    assert!(dist(xy(&project, mid), before_mid) < 0.001);
    assert_eq!(
        project.station_comment_for_ref(mid).map(String::as_str),
        Some("comment mid")
    );
    assert_eq!(
        project.station_comment_for_ref(far).map(String::as_str),
        Some("comment far")
    );
    assert_eq!(
        project.station_name_for_ref(mid).map(String::as_str),
        Some("name mid")
    );
    assert_eq!(
        project.station_name_for_ref(far).map(String::as_str),
        Some("name far")
    );
    assert_eq!(project.station_offset_for_ref(far), Some((0.25, 0.0, 0.0)));
    let merge = &project.station_merges[0];
    let ends = [
        project.resolve_key(&merge.station_a),
        project.resolve_key(&merge.station_b),
    ];
    assert!(ends.contains(&Some(mid)) && ends.contains(&Some(StationRef::new(c, 0))));
    let closure = &project.loop_closures[0];
    assert_eq!(project.resolve_key(&closure.station_a), Some(far));
    assert_eq!(
        project.resolve_key(&project.zero_datums[0].station_ref),
        Some(StationRef::new(k, 3))
    );
    let mut names_o = false;
    project.for_each_station_ref(|_, key| names_o |= key.section_id == o);
    assert!(!names_o, "no reference still names the retired section");
    assert!(project.check_structural_integrity().is_empty());
}

/// The join station: comments and names per the choice, an identical one kept
/// once, the kept station's offset winning, a zero-length closure dropped and
/// a closure to another station moved onto the joined station.
#[test]
fn the_join_station_is_settled_per_the_choices() {
    for (pick, comment) in [
        (CombinePick::Kept, "kept"),
        (CombinePick::Other, "other"),
        (CombinePick::Both, "kept\nother"),
    ] {
        let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
        let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
        let (join_k, join_o) = (StationRef::new(k, 2), StationRef::new(o, 0));
        let (key_k, key_o) = (
            project.key_for_ref(join_k).unwrap(),
            project.key_for_ref(join_o).unwrap(),
        );
        project
            .station_comments
            .insert(key_k.clone(), "kept".to_string());
        project
            .station_comments
            .insert(key_o.clone(), "other".to_string());
        project
            .station_names
            .insert(key_k.clone(), "S9".to_string());
        project
            .station_names
            .insert(key_o.clone(), "S9".to_string());
        project.station_offsets.insert(key_k, (0.0004, 0.0, 0.0));
        project.station_offsets.insert(key_o, (0.0, 0.0, 0.0));
        push_closure(&mut project, join_k, join_o, false);
        push_closure(&mut project, join_o, StationRef::new(c, 1), false);
        let candidate = project.combine_candidate_for_sections(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None);
        assert_eq!(candidate.comment_a.as_deref(), Some("kept"));
        assert_eq!(
            candidate.station_name_a, None,
            "identical names are no conflict"
        );

        project
            .combine_sections(
                k,
                o,
                &CombineSectionsOptions {
                    comment_pick: pick,
                    ..options()
                },
            )
            .unwrap();

        let joined = StationRef::new(k, 2);
        assert_eq!(
            project.station_comment_for_ref(joined).map(String::as_str),
            Some(comment)
        );
        assert_eq!(
            project.station_name_for_ref(joined).map(String::as_str),
            Some("S9")
        );
        assert_eq!(project.station_names.len(), 1);
        assert_eq!(
            project.station_offset_for_ref(joined),
            Some((0.0004, 0.0, 0.0))
        );
        assert_eq!(project.station_offsets.len(), 1);
        assert_eq!(
            project.loop_closures.len(),
            1,
            "the zero-length closure is dropped"
        );
        assert_eq!(
            project.resolve_key(&project.loop_closures[0].station_a),
            Some(joined)
        );
    }
}

fn blocked(project: &Project, k: SectionId, o: SectionId) -> String {
    project
        .combine_candidate_for_sections(k, o)
        .unwrap()
        .blocked_reason
        .unwrap_or_default()
}

#[test]
fn different_north_references_or_qualities_block() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    project.get_section_mut(o).unwrap().azimuth_reference = Some(AzimuthReference::from_parts(
        NorthReference::Magnetic,
        5.0,
        CorrectionSource::Manual,
    ));
    assert!(blocked(&project, k, o).contains("north references"));
    let mut clone = project.clone();
    assert!(clone.combine_sections(k, o, &options()).is_err());

    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    project.get_section_mut(o).unwrap().quality = 2.0;
    assert!(blocked(&project, k, o).contains("quality"));
}

/// A second rigid tie between the two (directly, through a third section, or
/// an aligned closure) would close a loop inside the combined section.
#[test]
fn a_second_rigid_tie_blocks() {
    // Directly: K start merged to O end as well.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    push_merge(&mut project, StationRef::new(k, 0), StationRef::new(o, 2));
    assert!(
        blocked(&project, k, o).contains("connected elsewhere"),
        "{}",
        blocked(&project, k, o)
    );

    // Through C: K's start and O's end are both merged to C.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let c = add(&mut project, "C", vec![shot(0.0, 1.0)]);
    push_merge(&mut project, StationRef::new(k, 0), StationRef::new(c, 0));
    push_merge(&mut project, StationRef::new(o, 3), StationRef::new(c, 1));
    assert!(blocked(&project, k, o).contains("connected elsewhere"));

    // An aligned closure blocks; an unaligned one stays a loop to adjust.
    for (aligned, blocks) in [(true, true), (false, false)] {
        let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
        push_closure(
            &mut project,
            StationRef::new(k, 0),
            StationRef::new(o, 3),
            aligned,
        );
        assert_eq!(
            !blocked(&project, k, o).is_empty(),
            blocks,
            "aligned {aligned}"
        );
    }
}

#[test]
fn a_branch_blocks() {
    // A third section merged at the join.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
    project
        .merge_stations(StationRef::new(c, 0), StationRef::new(k, 2))
        .unwrap();
    assert!(blocked(&project, k, o).contains("branch"));
    let at_join = project.combine_candidate(StationRef::new(k, 2)).unwrap();
    assert!(at_join.blocked_reason.unwrap().contains("branch"));

    // A third section's end co-located with an unmerged join.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
    let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
    let target = xy(&project, StationRef::new(k, 2));
    place(&mut project, c, 0, target, 0.0);
    assert!(blocked(&project, k, o).contains("branch"));

    // Merged to the middle of another section.
    let mut project = Project::new("Branch".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let c = add(&mut project, "C", vec![shot(0.0, 3.0), shot(0.0, 3.0)]);
    project
        .merge_stations(StationRef::new(k, 1), StationRef::new(c, 1))
        .unwrap();
    let at_end = project.combine_candidate(StationRef::new(k, 1)).unwrap();
    assert!(at_end.blocked_reason.unwrap().contains("branch"));
    assert!(
        project.combine_candidate(StationRef::new(k, 0)).is_none(),
        "nothing touches"
    );
    assert!(
        project.combine_candidate(StationRef::new(c, 1)).is_none(),
        "not an end"
    );
}

#[test]
fn zero_datums_on_both_sides_block() {
    // Co-located, each rigid group anchored.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
    project
        .set_zero_datum(StationRef::new(k, 0), gps())
        .unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: project.key_for_ref(StationRef::new(o, 3)).unwrap(),
        gps: gps(),
    });
    assert!(blocked(&project, k, o).contains("zero datum"));

    // Both join stations carry one.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    for station in [StationRef::new(k, 2), StationRef::new(o, 0)] {
        project.zero_datums.push(ZeroDatum {
            station_ref: project.key_for_ref(station).unwrap(),
            gps: gps(),
        });
    }
    assert!(blocked(&project, k, o).contains("zero datum"));
}

#[test]
fn different_manual_offsets_on_the_join_block() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
    let key = project.key_for_ref(StationRef::new(o, 0)).unwrap();
    project.station_offsets.insert(key, (0.0, 0.0, 0.002));
    assert!(blocked(&project, k, o).contains("moved by different amounts"));
}

#[test]
fn the_shot_budget_blocks() {
    let mut project = Project::new("Budget".to_string(), None);
    let k = add(
        &mut project,
        "K",
        (0..99_995).map(|_| shot(90.0, 1.0)).collect(),
    );
    let o = add(&mut project, "O", (0..5).map(|_| shot(90.0, 1.0)).collect());
    project
        .merge_stations(StationRef::new(k, 99_995), StationRef::new(o, 0))
        .unwrap();
    assert!(blocked(&project, k, o).contains("at most 99999"));
}

#[test]
fn sections_that_do_not_meet_meet_twice_or_are_empty_block() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.5);
    assert!(blocked(&project, k, o).contains("do not meet"));
    assert!(project.combine_sections(k, o, &options()).is_err());

    // Both ends co-located with the other's, untied: ambiguous.
    let mut project = Project::new("Twice".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(270.0, 10.0)]);
    let target = xy(&project, StationRef::new(k, 1));
    place(&mut project, o, 0, target, 0.0);
    assert!(blocked(&project, k, o).contains("both ends"));
    let at = project.combine_candidate(StationRef::new(k, 1)).unwrap();
    assert!(
        at.blocked_reason.unwrap().contains("both ends"),
        "the station menu agrees"
    );

    let mut project = Project::new("Empty".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", Vec::new());
    let target = xy(&project, StationRef::new(k, 1));
    place(&mut project, o, 0, target, 0.0);
    assert!(blocked(&project, k, o).contains("no shots"));
}

/// The join is the same point within 1 mm horizontally; depth never counts.
#[test]
fn the_tolerance_is_one_millimetre_and_depth_never_blocks() {
    let (project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0009);
    assert_eq!(
        project
            .combine_candidate_for_sections(k, o)
            .unwrap()
            .blocked_reason,
        None
    );
    let (project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0011);
    assert!(blocked(&project, k, o).contains("do not meet"));

    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
    for shot in &mut project.get_section_mut(o).unwrap().shots {
        shot.depth_in += 25.0;
        shot.depth_out += 25.0;
    }
    assert_eq!(
        project
            .combine_candidate_for_sections(k, o)
            .unwrap()
            .blocked_reason,
        None
    );
}

/// The component's gauge is the lower section id's; combining keeps that
/// physical station pinned whichever section is kept, including a captured
/// pair on a reversed section.
#[test]
fn the_gauge_station_survives_keeping_either_section() {
    for keep_lower in [true, false] {
        for reversed in [false, true] {
            let (mut project, k, o) = pair(SectionEnd::Start, SectionEnd::End, true, 0.0);
            if reversed {
                assert!(project.reverse_section(k, false));
            }
            project.capture_missing_gauge_anchors();
            let before = xy(&project, project.adjustment_anchor_for_tests(k).unwrap());
            let (kept, other) = if keep_lower { (k, o) } else { (o, k) };

            project.combine_sections(kept, other, &options()).unwrap();

            let anchor = project.adjustment_anchor_for_tests(kept).unwrap();
            assert!(
                dist(xy(&project, anchor), before) < 0.001,
                "keep_lower {keep_lower}, reversed {reversed}: the gauge moved"
            );
            assert!(project.get_section(kept).unwrap().gauge_shot_uuid.is_some());
        }
    }
}

/// The retired id points at the kept section with the parity relation of the
/// other section's shots, and chains through a later combine.
#[test]
fn the_retired_section_records_where_its_shots_went() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::End, true, 0.0);
    project.combine_sections(k, o, &options()).unwrap();
    assert_eq!(
        project.resolve_retired_section(o),
        (k, true),
        "O was reversed to line up"
    );
    assert_eq!(project.resolve_retired_section(k), (k, false));

    let c = add(&mut project, "C", vec![shot(0.0, 2.0)]);
    let end = StationRef::new(k, project.get_section(k).unwrap().shots.len() as u32);
    project.merge_stations(end, StationRef::new(c, 0)).unwrap();
    project.combine_sections(c, k, &options()).unwrap();
    assert_eq!(project.resolve_retired_section(o).0, c);
}

/// Split then combine at the join gives back the section: same shots, same
/// stations, same references.
#[test]
fn combining_a_fresh_split_at_its_join_restores_the_section() {
    let mut project = Project::new("Round trip".to_string(), None);
    let main = add(
        &mut project,
        "Main",
        vec![
            shot(90.0, 10.0),
            shot(45.0, 5.0),
            shot(0.0, 7.0),
            shot(300.0, 3.0),
        ],
    );
    let key = project.key_for_ref(StationRef::new(main, 3)).unwrap();
    project
        .station_comments
        .insert(key, "past the cut".to_string());
    let before = stations(&project, main);
    let shots_before: Vec<String> = project
        .get_section(main)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect();
    let second = project
        .split_section_at_station(
            main,
            2,
            &SplitSectionOptions {
                name_a: "Main-A".to_string(),
                name_b: "Main-B".to_string(),
                color_a: "#123456".to_string(),
                color_b: "#123456".to_string(),
                comment_side: SplitSide::A,
                datum_side: SplitSide::A,
            },
        )
        .unwrap();
    let candidate = project.combine_candidate(StationRef::new(main, 2)).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.section_b, second);

    project
        .combine_sections(
            main,
            second,
            &CombineSectionsOptions {
                name: "Main".to_string(),
                color: "#123456".to_string(),
                ..options()
            },
        )
        .unwrap();

    assert_eq!(project.sections.len(), 1);
    let after = stations(&project, main);
    for (a, b) in after.iter().zip(&before) {
        assert!(dist(*a, *b) < 1e-9);
    }
    let uuids: Vec<String> = project
        .get_section(main)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect();
    assert_eq!(uuids, shots_before);
    assert_eq!(
        project
            .station_comment_for_ref(StationRef::new(main, 3))
            .map(String::as_str),
        Some("past the cut")
    );
}

/// Combining at (K end, O end) equals reversing O, then combining at
/// (K end, O start).
#[test]
fn combining_end_to_end_equals_reversing_then_combining() {
    let (direct, k, o) = pair(SectionEnd::End, SectionEnd::End, true, 0.0);
    let mut direct = direct;
    let mut manual = direct.clone();
    direct.combine_sections(k, o, &options()).unwrap();
    assert!(manual.reverse_section(o, true));
    assert!(
        !manual
            .combine_candidate_for_sections(k, o)
            .unwrap()
            .reverses
    );
    manual.combine_sections(k, o, &options()).unwrap();

    let shots = |p: &Project| -> Vec<(Option<String>, f64, f64, f64)> {
        p.get_section(k)
            .unwrap()
            .shots
            .iter()
            .map(|s| (s.uuid.clone(), s.azimuth_in, s.azimuth_out, s.length))
            .collect()
    };
    assert_eq!(shots(&direct), shots(&manual));
    assert_eq!(stations(&direct, k), stations(&manual, k));
}
