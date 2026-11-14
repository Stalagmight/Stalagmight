//! Combine Sections on the model: the four pairings, references, the join,
//! every blocked case, the tolerance, the gauge and the retired-section map.

use chrono::Utc;

use crate::{
    AzimuthReference, ClosureStrategy, CombineJoinEnds, CombineJoinKind, CombinePick,
    CombineSectionsOptions, CorrectionSource, GpsCoordinates, LoopClosure, NorthReference, Person,
    Project, SectionData, SectionEnd, SectionId, ShotData, SplitSectionOptions, SplitSide,
    StationKey, StationMerge, StationRef, SurveyCredit, TornTie, TornTieKind, TornTieReason,
    ZeroDatum,
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
        join: None,
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
        assert_eq!(
            project.station_offsets.len(),
            1,
            "the other station's offset is dropped"
        );
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

/// Only the other join station was moved (by less than the tolerance): its
/// offset is dropped so the kept section's end stays exactly where it was.
#[test]
fn the_kept_join_station_never_takes_the_other_offset() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
    let key = project.key_for_ref(StationRef::new(o, 0)).unwrap();
    project.station_offsets.insert(key, (0.0006, 0.0, 0.0));
    let before = xy(&project, StationRef::new(k, 2));
    project.combine_sections(k, o, &options()).unwrap();
    assert!(project.station_offsets.is_empty());
    assert!(dist(xy(&project, StationRef::new(k, 2)), before) < 1e-12);
}

fn blocked(project: &Project, k: SectionId, o: SectionId) -> String {
    let reason = project
        .combine_candidate_for_sections(k, o)
        .unwrap()
        .blocked_reason
        .unwrap_or_default();
    if !reason.is_empty() {
        assert_refused_without_a_change(project, k, o, &reason);
    }
    reason
}

/// A blocked candidate is refused by the combine itself, with the same reason
/// and nothing changed, keeping either section (pattern from #713).
fn assert_refused_without_a_change(project: &Project, k: SectionId, o: SectionId, reason: &str) {
    for (kept, other) in [(k, o), (o, k)] {
        let mut copy = project.clone();
        let before = serde_json::to_string(&copy).unwrap();
        let error = copy.combine_sections(kept, other, &options()).unwrap_err();
        assert_eq!(error, reason, "keeping {kept}");
        assert_eq!(
            serde_json::to_string(&copy).unwrap(),
            before,
            "keeping {kept}"
        );
    }
}

/// The same shot uuid in both sections would put one shot in the combined
/// section twice (from #713): the other section's copy gets a new one, and a
/// station key on it follows.
#[test]
fn sections_sharing_a_shot_identity_get_fresh_ids() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let shared = project.get_section(k).unwrap().shots[1].uuid.clone();
    project.get_section_mut(o).unwrap().shots[2].uuid = shared.clone();
    project.rebuild_shot_uuid_index();
    let comment = StationRef::new(o, 3);
    let key = project.key_for_ref(comment).unwrap();
    project.station_comments.insert(key, "far end".to_string());
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.repeated_shot_ids, 1);

    project.combine_sections(k, o, &options()).unwrap();
    let uuids: Vec<String> = project
        .get_section(k)
        .unwrap()
        .shots
        .iter()
        .filter_map(|s| s.uuid.clone())
        .collect();
    let unique: std::collections::HashSet<&String> = uuids.iter().collect();
    assert_eq!(unique.len(), uuids.len(), "{uuids:?}");
    assert_eq!(uuids[1], shared.unwrap(), "the kept copy keeps its id");
    let far = project
        .station_comments
        .iter()
        .find(|(_, text)| text.as_str() == "far end")
        .map(|(key, _)| project.resolve_key(key));
    assert_eq!(far, Some(Some(StationRef::new(k, 5))));
}

/// The component's gauge sits on the lower-id section, whose gauge shot
/// shares its identity with a shot of the section kept. The identity is
/// renewed before the gauge is held, so the gauge stays on its own physical
/// station, not on the kept section's copy of the shared shot.
#[test]
fn a_shared_identity_on_the_gauge_shot_does_not_move_the_gauge() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let shared = project.get_section(k).unwrap().shots[0].uuid.clone();
    project.get_section_mut(o).unwrap().shots[1].uuid = shared;
    project.rebuild_shot_uuid_index();
    let before = xy(&project, project.adjustment_anchor_for_tests(k).unwrap());

    // Keep the higher id: K, which held the gauge, is the one retired.
    project.combine_sections(o, k, &options()).unwrap();
    let anchor = project.adjustment_anchor_for_tests(o).unwrap();
    assert!(
        dist(xy(&project, anchor), before) < 0.001,
        "the gauge moved to {anchor:?}"
    );
}

/// An aligned closure from the merged join into either section is a second
/// rigid tie (from #713): it becomes an ordinary closure inside the combined
/// section, and the plan says so.
#[test]
fn an_aligned_closure_from_the_join_into_either_section_becomes_an_ordinary_one() {
    for (from, to) in [((true, 2), (false, 1)), ((false, 0), (true, 1))] {
        let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
        let at = |(is_k, index): (bool, u32)| StationRef::new(if is_k { k } else { o }, index);
        push_closure(&mut project, at(from), at(to), true);
        let candidate = project.combine_candidate_for_sections(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "{from:?} -> {to:?}");
        let torn = &candidate.plan_keeping_a.torn_ties;
        assert_eq!(torn.len(), 1, "{from:?} -> {to:?}");
        assert_eq!(torn[0].kind, TornTieKind::SnappedClosure);
        assert_eq!(candidate.plan_keeping_a.moving_section, None);

        project.combine_sections(k, o, &options()).unwrap();
        let closure = &project.loop_closures[0];
        assert!(!closure.is_aligned, "{from:?} -> {to:?}");
        assert_eq!(closure.strategy, ClosureStrategy::Batch);
    }
}

/// A different north reference no longer blocks: the other section's
/// bearings turn to read against the kept section's reference, so every
/// station stays where it is, whichever section is kept. A different quality
/// takes the kept section's.
#[test]
fn different_north_references_convert_and_qualities_take_the_kept_ones() {
    for keep_k in [true, false] {
        let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
        let declared =
            AzimuthReference::from_parts(NorthReference::Magnetic, 5.0, CorrectionSource::Manual);
        project.get_section_mut(o).unwrap().azimuth_reference = Some(declared);
        project.adjust_section_offset(o, 0.0, 0.0);
        // Re-place O on K's end after the rotation the declination applied.
        let target = xy(&project, StationRef::new(k, 2));
        let now = xy(&project, StationRef::new(o, 0));
        project.adjust_section_offset(o, target.0 - now.0, target.1 - now.1);
        project.get_section_mut(o).unwrap().quality = 2.0;
        let candidate = project.combine_candidate_for_sections(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "keep K {keep_k}");
        let rotation = if keep_k {
            candidate.plan_keeping_a.azimuth_rotation_deg
        } else {
            candidate.plan_keeping_b.azimuth_rotation_deg
        };
        assert_eq!(rotation, Some(if keep_k { 5.0 } else { -5.0 }));
        let mut before: Vec<(f64, f64)> = stations(&project, k);
        before.extend(stations(&project, o).into_iter().skip(1));
        let quality = if keep_k { 1.0 } else { 2.0 };

        let (kept, other) = if keep_k { (k, o) } else { (o, k) };
        project.combine_sections(kept, other, &options()).unwrap();
        let section = project.get_section(kept).unwrap();
        assert_eq!(section.azimuth_reference, (!keep_k).then_some(declared));
        assert!((section.quality - quality).abs() < 1e-12, "keep K {keep_k}");
        let mut after = stations(&project, kept);
        if !keep_k {
            // O kept: K was prepended, so the order is the same.
            assert_eq!(after.len(), before.len());
        }
        for (want, got) in before.iter().zip(after.drain(..)) {
            assert!(
                dist(*want, got) < 1e-6,
                "keep K {keep_k}: {want:?} vs {got:?}"
            );
        }
    }
}

/// The stored references are compared, not the resolved ones: O declares the
/// very reference the project default resolves K to. The combined section
/// keeps only K's `None`, so the plan reports a conversion (by 0 degrees: O's
/// readings stay, and follow the project default from now on).
#[test]
fn a_declared_reference_and_the_project_default_combine_and_the_default_wins() {
    let declared =
        AzimuthReference::from_parts(NorthReference::Magnetic, 5.0, CorrectionSource::Manual);
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    project.default_azimuth_reference = Some(declared);
    project.get_section_mut(o).unwrap().azimuth_reference = Some(declared);
    assert_eq!(
        project.azimuth_reference_for_id(k),
        project.azimuth_reference_for_id(o),
        "setup: both resolve alike"
    );
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(
        candidate.plan_keeping_a.azimuth_rotation_deg,
        Some(0.0),
        "the values stay; the reference O follows changes"
    );
    let readings: Vec<f64> = project
        .get_section(o)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.azimuth_in)
        .collect();
    project.combine_sections(k, o, &options()).unwrap();
    let section = project.get_section(k).unwrap();
    assert_eq!(section.azimuth_reference, None);
    let after: Vec<f64> = section.shots[2..].iter().map(|s| s.azimuth_in).collect();
    assert_eq!(after, readings);

    // Both declaring the same one: nothing to convert.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    for id in [k, o] {
        project.get_section_mut(id).unwrap().azimuth_reference = Some(declared);
    }
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.plan_keeping_a.azimuth_rotation_deg, None);
}

/// A second rigid tie between the two (directly, through a third section, or
/// an aligned closure) would close a loop inside the combined section: it
/// becomes an ordinary loop closure, the one nearest the other section.
#[test]
fn a_second_rigid_tie_becomes_a_loop_closure() {
    // Directly: K start merged to O end as well (a file from before the
    // merge check). The combined section is not merged to itself.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    push_merge(&mut project, StationRef::new(k, 0), StationRef::new(o, 3));
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Merged);
    let torn = &candidate.plan_keeping_a.torn_ties;
    assert_eq!(torn.len(), 1);
    assert_eq!(torn[0].kind, TornTieKind::Merge);
    project.combine_sections(k, o, &options()).unwrap();
    assert!(
        project.station_merges.is_empty(),
        "{:?}",
        project.station_merges
    );
    let closure = &project.loop_closures[0];
    let mut ends = [
        project.resolve_key(&closure.station_a).unwrap(),
        project.resolve_key(&closure.station_b).unwrap(),
    ];
    ends.sort_by_key(|r| r.station_index);
    assert_eq!(ends, [StationRef::new(k, 0), StationRef::new(k, 5)]);
    assert_eq!(closure.strategy, ClosureStrategy::Batch);

    // Through C: K's start and O's end are both merged to C. The merge into
    // O tears; C stays merged to K.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let c = add(&mut project, "C", vec![shot(0.0, 1.0)]);
    push_merge(&mut project, StationRef::new(k, 0), StationRef::new(c, 0));
    push_merge(&mut project, StationRef::new(o, 3), StationRef::new(c, 1));
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(
        candidate.plan_keeping_a.torn_ties,
        vec![TornTie {
            station_a: StationRef::new(o, 3),
            station_b: StationRef::new(c, 1),
            kind: TornTieKind::Merge,
            reason: TornTieReason::Loop,
        }]
    );
    project.combine_sections(k, o, &options()).unwrap();
    assert_eq!(
        merged_to(&project, StationRef::new(k, 0)),
        vec![StationRef::new(c, 0)]
    );
    let closure = &project.loop_closures[0];
    let ends = [
        project.resolve_key(&closure.station_a).unwrap(),
        project.resolve_key(&closure.station_b).unwrap(),
    ];
    assert_eq!(ends, [StationRef::new(k, 5), StationRef::new(c, 1)]);

    // An aligned closure becomes an ordinary one; an unaligned one stays.
    for aligned in [true, false] {
        let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
        push_closure(
            &mut project,
            StationRef::new(k, 0),
            StationRef::new(o, 3),
            aligned,
        );
        let candidate = project.combine_candidate_for_sections(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "aligned {aligned}");
        assert_eq!(
            candidate.plan_keeping_a.torn_ties.len(),
            usize::from(aligned),
            "aligned {aligned}"
        );
        project.combine_sections(k, o, &options()).unwrap();
        assert!(!project.loop_closures[0].is_aligned);
    }
}

/// The stations `section_id`'s station `index` is merged to, as refs.
fn merged_to(project: &Project, at: StationRef) -> Vec<StationRef> {
    let mut out: Vec<StationRef> = project
        .station_merges
        .iter()
        .filter_map(|m| {
            let (a, b) = (
                project.resolve_key(&m.station_a)?,
                project.resolve_key(&m.station_b)?,
            );
            (a == at).then_some(b).or((b == at).then_some(a))
        })
        .collect();
    out.sort_by_key(|r| (r.section_id, r.station_index));
    out
}

/// A third section C merged at the join (to K's end, the kept join
/// station): the pair combines, and C stays merged to the joined station, now
/// in the middle of the combined section, whether O is appended to K or K is
/// prepended to O.
#[test]
fn a_third_section_merged_at_the_join_stays_attached_as_a_branch() {
    for keep_k in [true, false] {
        let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
        let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
        let target = xy(&project, StationRef::new(k, 2));
        place(&mut project, c, 0, target, 0.0);
        project
            .merge_stations(StationRef::new(c, 0), StationRef::new(k, 2))
            .unwrap();
        let candidate = project.combine_candidate_for_sections(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "keep K {keep_k}");
        assert_eq!(candidate.branches, vec![c]);
        assert!(candidate.loose_ends.is_empty());
        let before = xy(&project, StationRef::new(c, 1));

        let (kept, other) = if keep_k { (k, o) } else { (o, k) };
        project.combine_sections(kept, other, &options()).unwrap();

        // K has two shots: the joined station is the combined section's
        // station 2 either way (O appended to K, or K prepended to O).
        let joined = StationRef::new(kept, 2);
        assert_eq!(
            merged_to(&project, joined),
            vec![StationRef::new(c, 0)],
            "keep K {keep_k}: C stays merged to the joined station, once"
        );
        let combined = project.get_section(kept).unwrap().shots.len();
        assert!(joined.station_index > 0 && (joined.station_index as usize) < combined);
        assert!(dist(xy(&project, StationRef::new(c, 1)), before) < 1e-9);
        assert!(project.check_structural_integrity().is_empty());
    }
}

/// K's end and O's start side by side (not merged to each other), C merged
/// to BOTH: the two join stations are one group through C, and after the
/// combine both of C's merges name the joined station, so one is dropped.
#[test]
fn a_branch_merged_to_both_join_stations_is_merged_once() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
    let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
    let target = xy(&project, StationRef::new(k, 2));
    place(&mut project, c, 0, target, 0.0);
    for join in [StationRef::new(k, 2), StationRef::new(o, 0)] {
        project.merge_stations(StationRef::new(c, 0), join).unwrap();
    }
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert!(candidate.merged, "one group, through C");
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.branches, vec![c]);
    project.combine_sections(k, o, &options()).unwrap();
    assert_eq!(
        merged_to(&project, StationRef::new(k, 2)),
        vec![StationRef::new(c, 0)]
    );
    assert_eq!(project.station_merges.len(), 1);
}

/// A third section's end co-located with an unmerged join is not merged to
/// it: the combine goes ahead, reports it, and leaves it as it was.
#[test]
fn an_unmerged_third_end_at_the_join_is_left_untouched() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
    let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
    let target = xy(&project, StationRef::new(k, 2));
    place(&mut project, c, 0, target, 0.0);
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.loose_ends, vec![c]);
    assert!(candidate.branches.is_empty());
    let c_before = serde_json::to_string(project.get_section(c).unwrap()).unwrap();

    project.combine_sections(k, o, &options()).unwrap();
    assert_eq!(
        serde_json::to_string(project.get_section(c).unwrap()).unwrap(),
        c_before
    );
    assert!(merged_to(&project, StationRef::new(c, 0)).is_empty());
}

/// At a junction the station menu lists every partner, each answered on its
/// own; any one pair combines, and the rest stay at the joined station.
#[test]
fn the_station_query_lists_every_partner_at_a_junction() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let c = add(&mut project, "C", vec![shot(180.0, 5.0)]);
    let target = xy(&project, StationRef::new(k, 2));
    place(&mut project, c, 0, target, 0.0);
    project
        .merge_stations(StationRef::new(c, 0), StationRef::new(k, 2))
        .unwrap();
    // A fourth, D, whose quality differs: it combines too (taking the kept
    // section's quality).
    let d = add(&mut project, "D", vec![shot(270.0, 4.0)]);
    place(&mut project, d, 0, target, 0.0);
    project
        .merge_stations(StationRef::new(d, 0), StationRef::new(k, 2))
        .unwrap();
    project.get_section_mut(d).unwrap().quality = 2.0;

    let candidates = project.combine_candidates(StationRef::new(k, 2));
    let partners: Vec<(SectionId, bool)> = candidates
        .iter()
        .map(|c| (c.section_b, c.blocked_reason.is_none()))
        .collect();
    assert_eq!(partners, vec![(o, true), (c, true), (d, true)]);
    // Each candidate names the others at the point as branches.
    assert_eq!(candidates[1].branches, vec![o, d]);

    project.combine_sections(k, c, &options()).unwrap();
    assert_eq!(
        merged_to(&project, StationRef::new(k, 2)),
        vec![StationRef::new(o, 0), StationRef::new(d, 0)]
    );
}

/// The station menu's query lists every pair at a junction: the clicked
/// section with each partner first, then the pairs of the others, each joined
/// at this point. With K's end, O's start, C's start and D's start merged
/// there: 3 + 3 pairs, every one combinable.
#[test]
fn the_point_query_lists_every_pair_at_a_junction() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let target = xy(&project, StationRef::new(k, 2));
    let mut others = vec![o];
    for name in ["C", "D"] {
        let id = add(&mut project, name, vec![shot(180.0, 5.0)]);
        place(&mut project, id, 0, target, 0.0);
        project
            .merge_stations(StationRef::new(id, 0), StationRef::new(k, 2))
            .unwrap();
        others.push(id);
    }
    let (c, d) = (others[1], others[2]);
    let pairs: Vec<(SectionId, SectionId, bool)> = project
        .combine_candidates_at_point(StationRef::new(k, 2))
        .iter()
        .map(|x| (x.section_a, x.section_b, x.blocked_reason.is_none()))
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
    // The clicked section's own list is unchanged.
    assert_eq!(project.combine_candidates(StationRef::new(k, 2)).len(), 3);
    // A pair of the others combines at this point, and K stays attached.
    project.combine_sections(c, d, &options()).unwrap();
    assert!(merged_to(&project, StationRef::new(k, 2))
        .iter()
        .any(|at| at.section_id == c));
}

/// A section merged at the join at a station in its MIDDLE passes through:
/// the combine goes ahead and it stays merged to the joined station (a
/// crossing). What still offers nothing to join: an end merged only to the
/// middle of another section, a station that is not an end, and an end
/// nothing touches.
#[test]
fn a_section_through_the_join_stays_merged_as_a_crossing() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let c = add(&mut project, "C", vec![shot(0.0, 3.0), shot(0.0, 3.0)]);
    let target = xy(&project, StationRef::new(k, 2));
    place(&mut project, c, 1, target, 0.0);
    project
        .merge_stations(StationRef::new(c, 1), StationRef::new(k, 2))
        .unwrap();
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.passing_through, vec![c]);
    assert!(candidate.branches.is_empty());
    project.combine_sections(k, o, &options()).unwrap();
    assert_eq!(
        merged_to(&project, StationRef::new(k, 2)),
        vec![StationRef::new(c, 1)]
    );
    assert!(project.check_structural_integrity().is_empty());

    let mut project = Project::new("Through".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let c = add(&mut project, "C", vec![shot(0.0, 3.0), shot(0.0, 3.0)]);
    let target = xy(&project, StationRef::new(k, 1));
    place(&mut project, c, 1, target, 0.0);
    project
        .merge_stations(StationRef::new(k, 1), StationRef::new(c, 1))
        .unwrap();
    let at_end = project.combine_candidate(StationRef::new(k, 1)).unwrap();
    assert!(at_end
        .blocked_reason
        .unwrap()
        .contains("Merged to the middle of another section"));
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
        accuracy_m: None,
    });
    assert!(blocked(&project, k, o).contains("zero datum"));

    // Both join stations carry one.
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    for station in [StationRef::new(k, 2), StationRef::new(o, 0)] {
        project.zero_datums.push(ZeroDatum {
            station_ref: project.key_for_ref(station).unwrap(),
            gps: gps(),
            accuracy_m: None,
        });
    }
    assert!(blocked(&project, k, o).contains("zero datum"));
}

/// Join stations moved by different manual offsets: the joined station keeps
/// the kept one's (the fixed side's), and the plan says how far the other
/// moves.
#[test]
fn different_manual_offsets_on_the_join_keep_the_kept_one() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.0);
    let key = project.key_for_ref(StationRef::new(o, 0)).unwrap();
    project.station_offsets.insert(key, (0.0, 0.0, 0.25));
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    let change = candidate.plan_keeping_a.join_offset_change_m.unwrap();
    assert!((change - 0.25).abs() < 1e-9, "{change}");
    project.combine_sections(k, o, &options()).unwrap();
    assert_eq!(project.station_offset_for_ref(StationRef::new(k, 2)), None);
}

/// Two sections of `k_shots` and `o_shots`, merged end to start.
fn budget_pair(k_shots: u32, o_shots: u32) -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Budget".to_string(), None);
    let k = add(
        &mut project,
        "K",
        (0..k_shots).map(|_| shot(90.0, 1.0)).collect(),
    );
    let o = add(
        &mut project,
        "O",
        (0..o_shots).map(|_| shot(90.0, 1.0)).collect(),
    );
    project
        .merge_stations(StationRef::new(k, k_shots), StationRef::new(o, 0))
        .unwrap();
    (project, k, o)
}

#[test]
fn the_shot_budget_blocks() {
    let (project, k, o) = budget_pair(99_995, 5);
    assert!(blocked(&project, k, o).contains("at most 99999"));
}

/// Exactly at the edge: 99,999 shots between them (the most a section may
/// hold) combine, one more does not.
#[test]
fn the_shot_budget_is_exact_at_99_999() {
    let (mut project, k, o) = budget_pair(99_994, 5);
    assert_eq!(project.combine_refusal(k, o, None), None);
    project.combine_sections(k, o, &options()).unwrap();
    assert_eq!(project.get_section(k).unwrap().shots.len(), 99_999);

    let (project, k, o) = budget_pair(99_994, 6);
    let reason = blocked(&project, k, o);
    assert!(
        reason.contains("would have 100000 shots (at most 99999)"),
        "{reason}"
    );
}

/// Sections that meet nowhere are refused (merge two ends or close a loop
/// first); sections meeting at both ends join at one (the two-section query
/// picks one that needs no reverse, the station menu the clicked end); a
/// section with no shots is absorbed at its one station.
#[test]
fn sections_that_do_not_meet_meet_twice_or_are_empty() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, false, 0.5);
    let reason = blocked(&project, k, o);
    assert!(reason.contains("do not meet"), "{reason}");
    assert!(reason.contains("close a loop"), "{reason}");
    assert!(project.combine_sections(k, o, &options()).is_err());

    // Both ends co-located with the other's, untied: either join works.
    let mut project = Project::new("Twice".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(270.0, 10.0)]);
    let target = xy(&project, StationRef::new(k, 1));
    place(&mut project, o, 0, target, 0.0);
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(
        (candidate.end_a, candidate.end_b),
        (SectionEnd::End, SectionEnd::Start)
    );
    let at_start = project.combine_candidate(StationRef::new(k, 0)).unwrap();
    assert_eq!(at_start.blocked_reason, None);
    assert_eq!(
        (at_start.end_a, at_start.end_b),
        (SectionEnd::Start, SectionEnd::End)
    );
    let mut clone = project.clone();
    clone
        .combine_sections(
            k,
            o,
            &CombineSectionsOptions {
                join: Some(CombineJoinEnds {
                    kept: SectionEnd::Start,
                    other: SectionEnd::End,
                }),
                ..options()
            },
        )
        .unwrap();
    assert_eq!(
        stations(&clone, k)[1..],
        stations(&project, k)[..],
        "joined at K's start: O comes first"
    );

    let mut project = Project::new("Empty".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", Vec::new());
    let target = xy(&project, StationRef::new(k, 1));
    place(&mut project, o, 0, target, 0.0);
    let before = stations(&project, k);
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert!(!candidate.reverses);
    project.combine_sections(k, o, &options()).unwrap();
    assert_eq!(stations(&project, k), before);
    assert!(project.get_section(o).is_none());
}

/// The join is the same point within 1 mm horizontally; depth never counts.
#[test]
fn the_tolerance_is_one_millimetre_and_only_a_large_depth_gap_blocks() {
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

    // The horizontal test ignores depth: the two ends read the depth of
    // different shots (the state a fresh split leaves). A gap of any size is
    // reported for the dialog, never refused.
    for gap in [0.9, 25.0] {
        let (project, k, o) = depth_gap_pair(gap, false);
        let candidate = project.combine_candidate_for_sections(k, o).unwrap();
        assert_eq!(candidate.blocked_reason, None, "{gap} m");
        assert!(candidate.depth_difference_m.is_some(), "{gap} m");
    }
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

fn adjusted_xy(project: &Project, at: StationRef) -> (f64, f64) {
    let station = project
        .compute_adjusted_station_coordinates(&crate::config::ClosureAdjustmentConfig::default())
        .into_iter()
        .find(|s| s.station_ref == at)
        .unwrap();
    (station.coordinates.x, station.coordinates.y)
}

/// Keeping the HIGHER id must keep the lower id's gauge station, which the
/// component used, or the closure-adjusted geometry jumps by the adjustment
/// residual (test shape from #713, which measured a 3.4 m jump without it).
#[test]
fn keeping_the_higher_id_leaves_the_adjusted_geometry_in_place() {
    for other_end in [SectionEnd::Start, SectionEnd::End] {
        let (mut project, low, high) = pair(SectionEnd::End, other_end, true, 0.0);
        // A loop with a real misclosure, so the solve has a residual to place.
        push_closure(
            &mut project,
            StationRef::new(low, 1),
            StationRef::new(high, 2),
            false,
        );
        project.loop_closures[0].misclosure_distance = 0.4;
        project.capture_missing_gauge_anchors();
        let probe = project.key_for_ref(StationRef::new(high, 1)).unwrap();
        let before = adjusted_xy(&project, StationRef::new(high, 1));

        project.combine_sections(high, low, &options()).unwrap();

        let after = adjusted_xy(&project, project.resolve_key(&probe).unwrap());
        assert!(
            dist(before, after) < 1e-5,
            "{other_end:?}: {before:?} -> {after:?}"
        );
    }
}

/// A one-shot lower section, reversed, names its gauge side by its parity;
/// the combined section has the kept one's parity, so the station is named
/// afresh (from #713).
#[test]
fn a_one_shot_lower_sections_gauge_keeps_its_side() {
    let mut project = Project::new("Gauge".to_string(), None);
    let low = add(&mut project, "Low", vec![shot(90.0, 4.0)]);
    assert!(project.reverse_section(low, false));
    project.capture_missing_gauge_anchors();
    let high = add(&mut project, "High", vec![shot(0.0, 5.0), shot(45.0, 3.0)]);
    let target = xy(&project, StationRef::new(low, 0));
    place(&mut project, high, 2, target, 0.0);
    project
        .merge_stations(StationRef::new(low, 0), StationRef::new(high, 2))
        .unwrap();
    let before = xy(&project, project.adjustment_anchor_for_tests(low).unwrap());

    project.combine_sections(high, low, &options()).unwrap();

    let after = xy(&project, project.adjustment_anchor_for_tests(high).unwrap());
    assert!(dist(before, after) < 1e-9, "{before:?} -> {after:?}");
}

/// Every shot's RECORD parity (`Project::shot_record_parity`), by uuid.
fn record_parities(project: &Project) -> std::collections::BTreeMap<String, bool> {
    project
        .sections
        .iter()
        .flat_map(|s| {
            s.shots.iter().map(move |shot| {
                (
                    shot.uuid.clone().unwrap(),
                    project.shot_record_parity(s, shot.uuid.as_deref()),
                )
            })
        })
        .collect()
}

/// A shot's record parity flips exactly when its record is reversed: the
/// combine reverses the other section's records when the ends match, and
/// otherwise every shot keeps its own record parity, whatever the kept
/// section's `reverse_parity` (the other section's shots are recorded in
/// `counter_parity_shots` when it differs). Every pairing, every parity mix,
/// keeping either section; a second combine stacks on the first.
#[test]
fn every_shot_keeps_its_record_parity_through_a_combine() {
    for kept_end in [SectionEnd::Start, SectionEnd::End] {
        for other_end in [SectionEnd::Start, SectionEnd::End] {
            for (reverse_k, reverse_o) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                for keep_lower in [true, false] {
                    let what = format!(
                        "{kept_end:?}-{other_end:?} reversed {reverse_k}/{reverse_o} keep_lower {keep_lower}"
                    );
                    let (mut project, k, o) = pair(kept_end, other_end, false, 0.0);
                    for (id, reverse) in [(k, reverse_k), (o, reverse_o)] {
                        if reverse {
                            assert!(project.reverse_section(id, false));
                        }
                    }
                    // The reverses moved the ends: re-place O so the pairing's
                    // two ends meet again, and merge them.
                    let end = |p: &Project, id: SectionId, e: SectionEnd| {
                        StationRef::new(id, end_index(p, id, e))
                    };
                    let target = xy(&project, end(&project, k, kept_end));
                    let at_o = end(&project, o, other_end);
                    place(&mut project, o, at_o.station_index, target, 0.0);
                    project
                        .merge_stations(end(&project, k, kept_end), at_o)
                        .unwrap();
                    let (kept, other) = if keep_lower { (k, o) } else { (o, k) };
                    let reverses = project
                        .combine_candidate_for_sections(k, o)
                        .unwrap()
                        .reverses;
                    let other_uuids: Vec<String> = project
                        .get_section(other)
                        .unwrap()
                        .shots
                        .iter()
                        .map(|s| s.uuid.clone().unwrap())
                        .collect();
                    let mut expected = record_parities(&project);
                    if reverses {
                        for uuid in &other_uuids {
                            let parity = expected.get_mut(uuid).unwrap();
                            *parity = !*parity;
                        }
                    }

                    project.combine_sections(kept, other, &options()).unwrap();

                    assert_eq!(record_parities(&project), expected, "{what}");
                }
            }
        }
    }
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

/// An unaligned closure between the two sections does not block: after the
/// combine it is a loop closure inside one section, which the adjustment
/// solves like any other.
#[test]
fn an_unaligned_closure_between_them_becomes_a_loop_inside_the_section() {
    let mut project = Project::new("Loop".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(0.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(270.0, 10.0), shot(180.0, 9.5)]);
    project
        .merge_stations(StationRef::new(k, 2), StationRef::new(o, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(k, 0),
            StationRef::new(o, 2),
            ClosureStrategy::default(),
        )
        .unwrap();
    assert_eq!(
        project
            .combine_candidate_for_sections(k, o)
            .unwrap()
            .blocked_reason,
        None
    );

    project.combine_sections(k, o, &options()).unwrap();

    let closure = &project.loop_closures[0];
    assert_eq!(
        project.resolve_key(&closure.station_a),
        Some(StationRef::new(k, 0))
    );
    assert_eq!(
        project.resolve_key(&closure.station_b),
        Some(StationRef::new(k, 4))
    );
    let adjusted = project
        .compute_adjusted_station_coordinates(&crate::config::ClosureAdjustmentConfig::default());
    let at = |i: u32| {
        adjusted
            .iter()
            .find(|s| s.station_ref == StationRef::new(k, i))
            .unwrap()
            .coordinates
    };
    let (start, end) = (at(0), at(4));
    assert!(
        (start.x - end.x).hypot(start.y - end.y) < 0.01,
        "the adjustment closes the loop inside the one section"
    );
}

/// Each query walks every section's shots once for its end table, the
/// station query included (it used to build a second one for the pair).
#[test]
fn each_query_builds_one_end_table() {
    use super::combine_candidate::END_TABLE_BUILDS;
    let (project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let builds = || END_TABLE_BUILDS.with(std::cell::Cell::get);
    let before = builds();
    assert!(project.combine_candidate(StationRef::new(k, 2)).is_some());
    assert_eq!(builds() - before, 1, "the station query");
    let before = builds();
    assert!(project.combine_candidate_for_sections(k, o).is_some());
    assert_eq!(builds() - before, 1, "the pair query");
}

/// `spread` one-shot sections far apart from each other, then `pile` one-shot
/// sections that all start at one point (as unplaced sections all start at the
/// origin), their far ends fanned out so no two meet.
fn spread_then_pile(spread: u32, pile: u32) -> (Project, Vec<SectionId>) {
    let mut project = Project::new("Pile".to_string(), None);
    for i in 0..spread {
        let id = add(&mut project, "Spread", vec![shot(90.0, 10.0)]);
        project.adjust_section_offset(id, 100.0 * f64::from(i + 1), 500.0);
    }
    let piled = (0..pile)
        .map(|i| add(&mut project, "Pile", vec![shot(0.5 * f64::from(i), 10.0)]))
        .collect();
    (project, piled)
}

fn probes() -> usize {
    super::combine_candidate::END_TABLE_PROBES.with(std::cell::Cell::get)
}

/// The station query at a pile of `K` section ends reads O(K) ends, however
/// many other sections the project holds: one lookup for the partners, and
/// one pairing's touch (which lists the other ends there). Past
/// `MAX_COMBINE_PARTNERS` the pile is not a junction to pick from, so no
/// other partner is touched. It used to scan every end once per partner.
#[test]
fn the_station_query_at_a_pile_reads_only_the_ends_there() {
    let (project, piled) = spread_then_pile(400, 200);
    let before = probes();
    let candidate = project
        .combine_candidate(StationRef::new(piled[0], 0))
        .unwrap();
    let read = probes() - before;
    assert_eq!(
        candidate.blocked_reason.as_deref(),
        Some("200 section ends meet here: select the two to combine in the Sections panel")
    );
    assert!(read <= 2 * 200 + 16, "{read} ends read for a pile of 200");
}

/// The pair query for two sections that meet alone reads a handful of ends,
/// not every end in the project (the third-end check used to scan them all).
#[test]
fn the_pair_query_reads_only_the_ends_near_the_join() {
    let (mut project, _) = spread_then_pile(400, 0);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(0.0, 10.0)]);
    let target = xy(&project, StationRef::new(k, 1));
    place(&mut project, o, 0, target, 0.0);
    let before = probes();
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    let read = probes() - before;
    assert_eq!(candidate.blocked_reason, None);
    assert!(read <= 8, "{read} ends read");
}

/// The other section's shots get ids from the kept section's `next_shot_id`,
/// which only grows in a session. With no room left in the kept section's id
/// window they would take the next section's ids, so keeping it is refused
/// (with nothing changed) while keeping the other one still works.
#[test]
fn a_kept_section_without_shot_id_room_is_refused_but_the_other_can_be_kept() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let window_end = (k + 1) * crate::FEATURE_ID_STRIDE;
    project.get_section_mut(k).unwrap().next_shot_id = window_end - 2;

    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    let reason = candidate.blocked_keeping_a.clone().unwrap();
    assert!(reason.contains("used up its shot numbers"), "{reason}");
    assert_eq!(candidate.blocked_keeping_b, None);
    assert_eq!(project.combine_refusal(k, o, None), Some(reason.clone()));
    let before = serde_json::to_string(&project).unwrap();
    assert_eq!(
        project
            .clone()
            .combine_sections(k, o, &options())
            .unwrap_err(),
        reason
    );
    assert_eq!(serde_json::to_string(&project).unwrap(), before);

    project.combine_sections(o, k, &options()).unwrap();
    let ids: Vec<u32> = project
        .get_section(o)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();
    let window = o * crate::FEATURE_ID_STRIDE..(o + 1) * crate::FEATURE_ID_STRIDE;
    assert!(ids.iter().all(|id| window.contains(id)), "{ids:?}");
}

/// Exactly at the edge: the last id handed out is the window's last one.
#[test]
fn the_shot_id_room_check_is_exact_at_the_edge() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, true, 0.0);
    let window_end = (k + 1) * crate::FEATURE_ID_STRIDE;
    project.get_section_mut(k).unwrap().next_shot_id = window_end - 3;
    assert_eq!(
        project.combine_refusal(k, o, None),
        None,
        "three ids left for three shots"
    );
    project.combine_sections(k, o, &options()).unwrap();
    let last = project
        .get_section(k)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .max();
    assert_eq!(last, Some(window_end - 1));
}

/// A shot renamed by the cross-section uniqueness heal keeps its record parity.
#[test]
fn a_renamed_shot_keeps_its_record_parity() {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::End, true, 0.0);
    project.combine_sections(k, o, &options()).unwrap();
    let section = project.get_section(k).unwrap();
    let shot = section.shots.last().unwrap();
    let (old, parity) = (
        shot.uuid.clone().unwrap(),
        project.shot_record_parity(section, shot.uuid.as_deref()),
    );
    assert!(
        project.counter_parity_shots.contains(&old),
        "setup: a combined shot"
    );
    let mut remap = crate::ShotUuidRemap::default();
    remap.insert(k, old, "renamed".to_string());

    project.apply_shot_uuid_remap(&remap);
    project.follow_counter_parity(&remap);

    let section = project.get_section(k).unwrap();
    assert_eq!(project.shot_record_parity(section, Some("renamed")), parity);
}

/// A split at a junction copies the junction's merges to both halves; the
/// halves combine back, and the side passage stays merged to the joined
/// station (a path through the junction is not a loop to tear). Three
/// shapes: a side passage starting at the junction, one passing through it,
/// and one starting there and looping back onto the main line past the cut
/// with a snapped closure, which stays snapped: the loop is the one the
/// survey had before the split.
#[test]
fn a_split_at_a_junction_combines_back_whatever_passes_through() {
    for shape in ["starts there", "passes through", "loops back"] {
        let mut project = Project::new("Junction".to_string(), None);
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
        let junction = StationRef::new(main, 2);
        let side = add(
            &mut project,
            "Side",
            vec![shot(180.0, 6.0), shot(180.0, 6.0)],
        );
        let at = u32::from(shape == "passes through");
        let target = xy(&project, junction);
        place(&mut project, side, at, target, 0.0);
        project
            .merge_stations(junction, StationRef::new(side, at))
            .unwrap();
        if shape == "loops back" {
            // A rigid tie back onto the main line, past the cut.
            push_closure(
                &mut project,
                StationRef::new(side, 2),
                StationRef::new(main, 4),
                true,
            );
        }
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

        let pair = blocked(&project, main, second);
        let station = project
            .combine_candidates(StationRef::new(main, 2))
            .into_iter()
            .find(|c| c.section_b == second)
            .unwrap()
            .blocked_reason
            .unwrap_or_default();
        assert_eq!((pair.as_str(), station.as_str()), ("", ""), "{shape}");
        project
            .combine_sections(
                main,
                second,
                &CombineSectionsOptions {
                    name: "Main".to_string(),
                    ..options()
                },
            )
            .unwrap();
        assert_eq!(project.get_section(main).unwrap().shots.len(), 4, "{shape}");
        assert_eq!(
            merged_to(&project, junction),
            vec![StationRef::new(side, at)],
            "{shape}: the side passage stays at the junction"
        );
        if shape == "loops back" {
            assert!(
                project.loop_closures[0].is_aligned,
                "the loop the survey had stays snapped"
            );
        }
    }
}

/// O (lowest id) -> K (highest id) end to end; a third section T (middle id)
/// hangs off K's far end and closes a loop back onto K with a real
/// misclosure. The component's gauge is O's, the lowest id. Keep K: after the
/// combine O is gone, the lowest id in the component is T, which does not hold
/// the station the adjustment held, and the adjusted geometry must not move.
#[test]
fn combine_keeps_the_component_anchor_with_a_third_section() {
    let mut project = Project::new("Probe".to_string(), None);
    let o = add(&mut project, "O", vec![shot(90.0, 10.0)]);
    let t = add(
        &mut project,
        "T",
        vec![shot(0.0, 10.0), shot(270.0, 10.0), shot(180.0, 9.0)],
    );
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    assert!(o < t && t < k, "ids {o} {t} {k}");
    let target = xy(&project, StationRef::new(o, 1));
    place(&mut project, k, 0, target, 0.0);
    project
        .merge_stations(StationRef::new(o, 1), StationRef::new(k, 0))
        .unwrap();
    let k_end = xy(&project, StationRef::new(k, 2));
    place(&mut project, t, 0, k_end, 0.0);
    project
        .merge_stations(StationRef::new(k, 2), StationRef::new(t, 0))
        .unwrap();
    let (a, b) = (
        project.key_for_ref(StationRef::new(t, 3)).unwrap(),
        project.key_for_ref(StationRef::new(k, 1)).unwrap(),
    );
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a: a,
        station_b: b,
        path_length: 30.0,
        misclosure_distance: 1.0,
        error_percentage: 3.0,
        horizontal_misclosure: 1.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    });
    assert_eq!(
        project
            .combine_candidate_for_sections(k, o)
            .unwrap()
            .blocked_reason,
        None
    );
    let anchor_before = project.adjustment_anchor_for_tests(k).unwrap();
    let before: Vec<_> = (0..=3)
        .map(|i| adjusted_xy(&project, StationRef::new(t, i)))
        .collect();

    project.combine_sections(k, o, &options()).unwrap();

    let anchor_after = project.adjustment_anchor_for_tests(k).unwrap();
    let after: Vec<_> = (0..=3)
        .map(|i| adjusted_xy(&project, StationRef::new(t, i)))
        .collect();
    let worst = before
        .iter()
        .zip(&after)
        .map(|(p, q)| dist(*p, *q))
        .fold(0.0, f64::max);
    assert!(
        worst < 0.001,
        "anchor {anchor_before:?} -> {anchor_after:?}: the adjusted geometry moved {worst:.4} m"
    );
}

/// S (lowest id, reverse parity, so its gauge is its far end) is split at
/// station 2; a section X merged at the cut station keeps the halves in one
/// component and closes a loop onto S's far end. The first half keeps S's id
/// and anchors the component, while the station the adjustment held went to
/// the second half. The adjusted geometry must not move.
#[test]
fn split_keeps_the_component_anchor() {
    let mut project = Project::new("Probe".to_string(), None);
    let s = add(
        &mut project,
        "S",
        vec![
            shot(90.0, 10.0),
            shot(90.0, 10.0),
            shot(90.0, 10.0),
            shot(90.0, 10.0),
        ],
    );
    let x = add(
        &mut project,
        "X",
        vec![shot(0.0, 10.0), shot(90.0, 20.0), shot(180.0, 9.0)],
    );
    project
        .sections
        .iter_mut()
        .find(|sec| sec.id == s)
        .unwrap()
        .reverse_parity = true;
    let cut = xy(&project, StationRef::new(s, 2));
    place(&mut project, x, 0, cut, 0.0);
    project
        .merge_stations(StationRef::new(s, 2), StationRef::new(x, 0))
        .unwrap();
    let (a, b) = (
        project.key_for_ref(StationRef::new(x, 3)).unwrap(),
        project.key_for_ref(StationRef::new(s, 4)).unwrap(),
    );
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a: a,
        station_b: b,
        path_length: 39.0,
        misclosure_distance: 1.0,
        error_percentage: 2.5,
        horizontal_misclosure: 1.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    });
    let anchor_before = project.adjustment_anchor_for_tests(s).unwrap();
    let before: Vec<_> = (0..=3)
        .map(|i| adjusted_xy(&project, StationRef::new(x, i)))
        .collect();

    let opts = SplitSectionOptions {
        name_a: "S-A".to_string(),
        name_b: "S-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    };
    project.split_section_at_station(s, 2, &opts).unwrap();

    let anchor_after = project.adjustment_anchor_for_tests(s).unwrap();
    let after: Vec<_> = (0..=3)
        .map(|i| adjusted_xy(&project, StationRef::new(x, i)))
        .collect();
    let worst = before
        .iter()
        .zip(&after)
        .map(|(p, q)| dist(*p, *q))
        .fold(0.0, f64::max);
    assert!(
        worst < 0.001,
        "anchor {anchor_before:?} -> {anchor_after:?}: the adjusted geometry moved {worst:.4} m"
    );
}

/// K's end and O's start, co-located (or merged), with all of O `gap` metres
/// deeper than K's end (every shot shifted, so no slope and no horizontal
/// position changes).
fn depth_gap_pair(gap: f64, merged: bool) -> (Project, SectionId, SectionId) {
    let (mut project, k, o) = pair(SectionEnd::End, SectionEnd::Start, merged, 0.0);
    for shot in &mut project.get_section_mut(o).unwrap().shots {
        shot.depth_in += gap;
        shot.depth_out += gap;
    }
    (project, k, o)
}

/// Two co-located, unmerged ends each read their own shot's depth; combined,
/// the joined station reads the average of the shot before and after it, so
/// each end moves by half the gap. Above 5 cm the candidate reports the gap
/// for the dialog to show, however large: the user decides. A merged join
/// already averages, so it reports nothing.
#[test]
fn a_depth_gap_at_an_unmerged_join_is_reported_never_refused() {
    use super::combine_candidate::COMBINE_DEPTH_NOTICE_M;
    const { assert!(COMBINE_DEPTH_NOTICE_M < 0.1) };

    let (mut project, k, o) = depth_gap_pair(0.3, false);
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    let gap = candidate.depth_difference_m.unwrap();
    assert!((gap - 0.3).abs() < 1e-9, "{gap}");
    project.combine_sections(k, o, &options()).unwrap();
    let joined = project.compute_station_depth(&[StationRef::new(k, 2)]);
    assert!((joined - 0.15).abs() < 1e-9, "{joined}");

    let (project, k, o) = depth_gap_pair(0.03, false);
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.depth_difference_m, None, "below the notice");

    let (mut project, k, o) = depth_gap_pair(1.5, false);
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert!((candidate.depth_difference_m.unwrap() - 1.5).abs() < 1e-9);
    project.combine_sections(k, o, &options()).unwrap();
    let joined = project.compute_station_depth(&[StationRef::new(k, 2)]);
    assert!((joined - 0.75).abs() < 1e-9, "{joined}");

    let (project, k, o) = depth_gap_pair(1.5, true);
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(
        candidate.blocked_reason, None,
        "a merged join already averages"
    );
    assert_eq!(candidate.depth_difference_m, None);
}
