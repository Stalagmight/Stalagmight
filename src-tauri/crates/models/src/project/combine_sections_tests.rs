//! Combine Sections (#707) on the model: every pairing keeps the cave where it
//! was, credit is carried as per-shot overrides, the joined station settles
//! per the caller's choices, and every refusal leaves the project untouched.

use chrono::{NaiveDate, Utc};

use super::*;
use crate::{
    ClosureStrategy, CombineOptions, CombinePairing, ShotCredit, SplitSectionOptions, SplitSide,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

/// `shot` reading `depth` at its start and one metre deeper at its end.
fn sloped(azimuth: f64, length: f64, depth: f64) -> ShotData {
    ShotData {
        depth_in: depth,
        depth_out: depth + 1.0,
        ..shot(azimuth, length)
    }
}

fn options() -> CombineOptions {
    CombineOptions {
        name: "Joined".to_string(),
        color: "#123456".to_string(),
        comment_side: SplitSide::A,
        name_side: SplitSide::A,
        offset_side: SplitSide::A,
    }
}

fn xy(project: &Project, section: SectionId, index: u32) -> (f64, f64) {
    let (x, y) = project
        .compute_station_raw_xy(StationRef::new(section, index))
        .unwrap();
    ((x * 1e6).round() / 1e6, (y * 1e6).round() / 1e6)
}

fn shot_count(project: &Project, section: SectionId) -> u32 {
    project.get_section(section).unwrap().shots.len() as u32
}

/// Translate `section` so its station `index` lies on `target`.
fn place(project: &mut Project, section: SectionId, index: u32, target: (f64, f64)) {
    project.set_section_offset(section, (0.0, 0.0));
    let (x, y) = project
        .compute_station_raw_xy(StationRef::new(section, index))
        .unwrap();
    project.set_section_offset(section, (target.0 - x, target.1 - y));
}

/// "Kept": two sloping shots east then north, offset away from the origin.
/// "Other": three sloping shots, placed so `pairing`'s two ends meet.
fn joined(pairing: CombinePairing) -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Combine".to_string(), None);
    let kept = project
        .add_section(SectionData {
            name: Some("Kept".to_string()),
            shots: vec![sloped(90.0, 10.0, 10.0), sloped(0.0, 5.0, 11.0)],
            ..Default::default()
        })
        .id;
    let other = project
        .add_section(SectionData {
            name: Some("Other".to_string()),
            shots: vec![
                sloped(30.0, 4.0, 20.0),
                sloped(120.0, 6.0, 21.0),
                sloped(200.0, 3.0, 22.0),
            ],
            ..Default::default()
        })
        .id;
    project.set_section_offset(kept, (100.0, -40.0));
    let (k, o) = pairing.ends(2, 3);
    let target = xy(&project, kept, k);
    place(&mut project, other, o, target);
    (project, kept, other)
}

fn uuids(project: &Project, section: SectionId) -> Vec<String> {
    project
        .get_section(section)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect()
}

fn lengths(project: &Project, section: SectionId) -> Vec<f64> {
    project
        .get_section(section)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect()
}

#[test]
fn every_pairing_keeps_shot_order_lengths_and_station_positions() {
    for pairing in CombinePairing::ALL {
        let (mut project, kept, other) = joined(pairing);
        let candidate = project.combine_candidate(other, kept).unwrap();
        assert_eq!(candidate.pairing, pairing);
        assert_eq!(candidate.kept, kept, "the lower id is kept");
        assert_eq!(candidate.blocked, None, "{pairing:?}");
        assert_eq!(candidate.reverse_other, pairing.reverses_other());

        let kept_xy: Vec<_> = (0..=2).map(|i| xy(&project, kept, i)).collect();
        let mut other_xy: Vec<_> = (0..=3).map(|i| xy(&project, other, i)).collect();
        let (kept_uuids, mut other_uuids) = (uuids(&project, kept), uuids(&project, other));
        let (kept_lengths, mut other_lengths) = (lengths(&project, kept), lengths(&project, other));
        if pairing.reverses_other() {
            other_xy.reverse();
            other_uuids.reverse();
            other_lengths.reverse();
        }
        let (expected_xy, expected_uuids, expected_lengths): (Vec<_>, Vec<_>, Vec<_>) =
            if pairing.prepends() {
                (
                    [other_xy, kept_xy[1..].to_vec()].concat(),
                    [other_uuids, kept_uuids].concat(),
                    [other_lengths, kept_lengths].concat(),
                )
            } else {
                (
                    [kept_xy, other_xy[1..].to_vec()].concat(),
                    [kept_uuids, other_uuids].concat(),
                    [kept_lengths, other_lengths].concat(),
                )
            };

        project.combine_sections(kept, other, &options()).unwrap();

        assert!(project.get_section(other).is_none(), "{pairing:?}");
        assert!(!project.section_offsets.contains_key(&other));
        let section = project.get_section(kept).unwrap();
        assert_eq!(section.name.as_deref(), Some("Joined"));
        assert_eq!(section.color, "#123456");
        assert_eq!(uuids(&project, kept), expected_uuids, "{pairing:?}");
        assert_eq!(lengths(&project, kept), expected_lengths, "{pairing:?}");
        for (i, expected) in expected_xy.iter().enumerate() {
            let at = xy(&project, kept, i as u32);
            assert!(
                (at.0 - expected.0).abs() < 1e-5 && (at.1 - expected.1).abs() < 1e-5,
                "{pairing:?} station {i}: {at:?} != {expected:?}"
            );
        }
        assert_eq!(shot_count(&project, kept), 5);
        let ids: HashSet<u32> = section.shots.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 5, "fresh, distinct shot ids");
    }
}

#[test]
fn a_prepend_keeps_the_kept_sections_gauge_station() {
    let (mut project, kept, other) = joined(CombinePairing::KeptStartOtherEnd);
    let first = uuids(&project, kept)[0].clone();

    project.combine_sections(kept, other, &options()).unwrap();

    let section = project.get_section(kept).unwrap();
    assert_eq!(section.gauge_shot_uuid.as_deref(), Some(first.as_str()));
}

fn day(m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, d).unwrap()
}

#[test]
fn credit_is_written_only_where_the_other_sections_value_differs() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let section = project.get_section_mut(kept).unwrap();
    section.surveyor = Some("Bob".to_string());
    section.date = Some(day(3, 2));
    let section = project.get_section_mut(other).unwrap();
    section.surveyor = None;
    section.date = Some(day(4, 11));
    section.shots[1].credit = Some(ShotCredit {
        surveyor: Some(Some("Ann".to_string())),
        date: None,
    });
    section.shots[2].credit = Some(ShotCredit {
        surveyor: Some(Some("Bob".to_string())),
        date: Some(Some(day(3, 2))),
    });

    let candidate = project.combine_candidate(kept, other).unwrap();
    let runs: Vec<_> = candidate
        .credit
        .iter()
        .map(|r| (r.surveyor.as_deref(), r.date.as_deref(), r.length_m))
        .collect();
    assert_eq!(
        runs,
        vec![
            (Some("Bob"), Some("2026-03-02"), 18.0),
            (None, Some("2026-04-11"), 4.0),
            (Some("Ann"), Some("2026-04-11"), 6.0),
        ]
    );

    project.combine_sections(kept, other, &options()).unwrap();

    let section = project.get_section(kept).unwrap();
    assert_eq!(
        section.surveyor.as_deref(),
        Some("Bob"),
        "nothing is folded"
    );
    let credits: Vec<_> = section.shots.iter().map(|s| s.credit.clone()).collect();
    assert_eq!(
        credits,
        vec![
            None,
            None,
            Some(ShotCredit {
                surveyor: Some(None),
                date: Some(Some(day(4, 11))),
            }),
            Some(ShotCredit {
                surveyor: Some(Some("Ann".to_string())),
                date: Some(Some(day(4, 11))),
            }),
            None,
        ]
    );
}

fn comment_at(project: &Project, at: StationRef) -> Option<String> {
    let key = project.key_for_ref(at)?;
    project.station_comments.get(&key).cloned()
}

#[test]
fn a_comment_on_the_other_section_follows_its_station() {
    for pairing in CombinePairing::ALL {
        let (mut project, kept, other) = joined(pairing);
        let key = project.key_for_ref(StationRef::new(other, 1)).unwrap();
        project.station_comments.insert(key, "pinch".to_string());

        project.combine_sections(kept, other, &options()).unwrap();

        // Other's station 1, wherever the pairing put it.
        let index = match pairing {
            CombinePairing::KeptEndOtherStart => 3,
            CombinePairing::KeptEndOtherEnd => 4,
            CombinePairing::KeptStartOtherEnd => 1,
            CombinePairing::KeptStartOtherStart => 2,
        };
        assert_eq!(
            comment_at(&project, StationRef::new(kept, index)).as_deref(),
            Some("pinch"),
            "{pairing:?}"
        );
        assert_eq!(project.station_comments.len(), 1);
    }
}

fn datum(project: &mut Project, at: StationRef) {
    let station_ref = project.key_for_ref(at).unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref,
        gps: GpsCoordinates {
            latitude: 20.0,
            longitude: -87.0,
            depth_reference: 0.0,
        },
    });
}

/// Puts a comment, a name and a z-only manual offset on both joined ends
/// (kept's station 2, other's station 0), and a zero datum on other's.
fn both_ends_loaded() -> (Project, SectionId, SectionId) {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    for (at, label, z) in [
        (StationRef::new(kept, 2), "kept", 0.5),
        (StationRef::new(other, 0), "other", 0.25),
    ] {
        let key = project.key_for_ref(at).unwrap();
        project
            .station_comments
            .insert(key.clone(), label.to_string());
        project.station_names.insert(key.clone(), label.to_string());
        project.station_offsets.insert(key, (0.0, 0.0, z));
    }
    datum(&mut project, StationRef::new(other, 0));
    (project, kept, other)
}

#[test]
fn the_joined_station_keeps_what_the_caller_picks() {
    for (comment_side, name_side, offset_side, comment, name, z) in [
        (
            SplitSide::A,
            SplitSide::A,
            SplitSide::A,
            "kept",
            "kept",
            0.5,
        ),
        (
            SplitSide::B,
            SplitSide::B,
            SplitSide::B,
            "other",
            "other",
            0.25,
        ),
        (
            SplitSide::Both,
            SplitSide::A,
            SplitSide::B,
            "kept\nother",
            "kept",
            0.25,
        ),
    ] {
        let (mut project, kept, other) = both_ends_loaded();
        let candidate = project.combine_candidate(kept, other).unwrap();
        assert!(candidate.comment_conflict && candidate.name_conflict && candidate.offset_conflict);

        project
            .combine_sections(
                kept,
                other,
                &CombineOptions {
                    comment_side,
                    name_side,
                    offset_side,
                    ..options()
                },
            )
            .unwrap();

        let at = StationRef::new(kept, 2);
        let key = project.key_for_ref(at).unwrap();
        assert_eq!(comment_at(&project, at).as_deref(), Some(comment));
        assert_eq!(
            project.station_names.get(&key).map(String::as_str),
            Some(name)
        );
        assert_eq!(project.station_offsets.get(&key), Some(&(0.0, 0.0, z)));
        assert_eq!(project.station_comments.len(), 1);
        assert_eq!(project.station_names.len(), 1);
        assert_eq!(project.station_offsets.len(), 1);
        assert_eq!(project.zero_datums.len(), 1);
        assert_eq!(
            project.resolve_key(&project.zero_datums[0].station_ref),
            Some(at)
        );
    }
}

#[test]
fn a_value_on_one_end_only_survives_whatever_the_choice() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let key = project.key_for_ref(StationRef::new(other, 0)).unwrap();
    project
        .station_comments
        .insert(key.clone(), "other".to_string());
    project.station_names.insert(key, "O0".to_string());
    assert!(
        !project
            .combine_candidate(kept, other)
            .unwrap()
            .comment_conflict
    );

    project.combine_sections(kept, other, &options()).unwrap();

    let at = StationRef::new(kept, 2);
    assert_eq!(comment_at(&project, at).as_deref(), Some("other"));
    let key = project.key_for_ref(at).unwrap();
    assert_eq!(
        project.station_names.get(&key).map(String::as_str),
        Some("O0")
    );
}

fn merge(project: &Project, a: StationRef, b: StationRef) -> StationMerge {
    StationMerge {
        station_a: project.key_for_ref(a).unwrap(),
        station_b: project.key_for_ref(b).unwrap(),
        merged_at: Utc::now(),
    }
}

fn resolved_merges(project: &Project) -> Vec<(StationRef, StationRef)> {
    project
        .station_merges
        .iter()
        .map(|m| {
            (
                project.resolve_key(&m.station_a).unwrap(),
                project.resolve_key(&m.station_b).unwrap(),
            )
        })
        .collect()
}

/// A one-shot "Side" section whose start lies on kept's end.
fn add_side(project: &mut Project, kept: SectionId) -> SectionId {
    let side = project
        .add_section(SectionData {
            shots: vec![shot(270.0, 4.0)],
            ..Default::default()
        })
        .id;
    let target = xy(project, kept, 2);
    place(project, side, 0, target);
    side
}

#[test]
fn the_join_merge_is_dropped_and_repeated_merges_collapse() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let side = add_side(&mut project, kept);
    let (k, o, s) = (
        StationRef::new(kept, 2),
        StationRef::new(other, 0),
        StationRef::new(side, 0),
    );
    for (a, b) in [(k, o), (k, s), (o, s)] {
        let m = merge(&project, a, b);
        project.station_merges.push(m);
    }
    assert_eq!(
        project.combine_candidate(kept, other).unwrap().blocked,
        None
    );

    project.combine_sections(kept, other, &options()).unwrap();

    assert_eq!(resolved_merges(&project), vec![(k, s)]);
}

#[test]
fn a_join_through_a_third_station_is_not_a_second_connection() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let side = add_side(&mut project, kept);
    let (k, o, s) = (
        StationRef::new(kept, 2),
        StationRef::new(other, 0),
        StationRef::new(side, 0),
    );
    for (a, b) in [(k, s), (o, s)] {
        let m = merge(&project, a, b);
        project.station_merges.push(m);
    }

    project.combine_sections(kept, other, &options()).unwrap();

    assert_eq!(resolved_merges(&project), vec![(k, s)]);
}

fn blocked(project: &Project, kept: SectionId, other: SectionId) -> String {
    project
        .combine_candidate(kept, other)
        .unwrap()
        .blocked
        .unwrap_or_default()
}

/// Refused with `reason` in the message, and nothing changed.
fn assert_refused(mut project: Project, kept: SectionId, other: SectionId, reason: &str) {
    assert!(blocked(&project, kept, other).contains(reason), "{reason}");
    let before = serde_json::to_string(&project).unwrap();
    let error = project
        .combine_sections(kept, other, &options())
        .unwrap_err();
    assert!(error.contains(reason), "{error}");
    assert_eq!(serde_json::to_string(&project).unwrap(), before);
}

fn closure(project: &Project, a: StationRef, b: StationRef, is_aligned: bool) -> LoopClosure {
    LoopClosure {
        id: 99,
        station_a: project.key_for_ref(a).unwrap(),
        station_b: project.key_for_ref(b).unwrap(),
        path_length: 30.0,
        misclosure_distance: 0.4,
        error_percentage: 1.3,
        horizontal_misclosure: 0.4,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned,
    }
}

#[test]
fn every_block_reason_refuses_without_a_change() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    project.get_section_mut(other).unwrap().azimuth_reference = Some(AzimuthReference {
        correction: 5.0,
        ..AzimuthReference::default()
    });
    assert_refused(project, kept, other, "azimuth references");

    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    project.get_section_mut(other).unwrap().quality = 2.0;
    assert_refused(project, kept, other, "quality");

    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let side = add_side(&mut project, kept);
    for (a, b) in [
        (StationRef::new(kept, 1), StationRef::new(side, 1)),
        (StationRef::new(side, 1), StationRef::new(other, 2)),
    ] {
        let m = merge(&project, a, b);
        project.station_merges.push(m);
    }
    assert_refused(project, kept, other, "another station");

    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let c = closure(
        &project,
        StationRef::new(kept, 1),
        StationRef::new(other, 2),
        true,
    );
    project.loop_closures.push(c);
    assert_refused(project, kept, other, "another station");

    // Other is kept's two shots walked back, so both pairs of ends meet.
    let (mut project, kept, _) = joined(CombinePairing::KeptEndOtherStart);
    let ring = project
        .add_section(SectionData {
            shots: vec![sloped(180.0, 5.0, 12.0), sloped(270.0, 10.0, 11.0)],
            ..Default::default()
        })
        .id;
    let target = xy(&project, kept, 2);
    place(&mut project, ring, 0, target);
    assert_refused(project, kept, ring, "ring");
}

#[test]
fn sections_sharing_a_shot_identity_are_refused() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let shared = uuids(&project, kept)[1].clone();
    project.get_section_mut(other).unwrap().shots[2].uuid = Some(shared);
    assert_refused(project, kept, other, "share a shot identity");
}

#[test]
fn an_aligned_closure_from_the_merged_join_into_either_section_is_refused() {
    for (from, to) in [((true, 2), (false, 1)), ((false, 0), (true, 1))] {
        let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
        let m = merge(
            &project,
            StationRef::new(kept, 2),
            StationRef::new(other, 0),
        );
        project.station_merges.push(m);
        let at = |(is_kept, index): (bool, u32)| {
            StationRef::new(if is_kept { kept } else { other }, index)
        };
        let c = closure(&project, at(from), at(to), true);
        project.loop_closures.push(c);
        assert_refused(project, kept, other, "another station");
    }
}

#[test]
fn ends_that_only_lie_together_with_a_zero_datum_on_each_side_are_refused() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    datum(&mut project, StationRef::new(kept, 0));
    datum(&mut project, StationRef::new(other, 3));
    assert_refused(project, kept, other, "Both sections already have a zero datum");

    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    datum(&mut project, StationRef::new(kept, 0));
    assert_eq!(blocked(&project, kept, other), "", "one datum is fine");
}

#[test]
fn a_missing_section_is_blocked() {
    let (project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let merges = project.build_station_merge_adjacency();
    let missing = other + 100;
    for (a, b) in [(kept, missing), (missing, other)] {
        let reason = project.combine_blocked(
            a,
            b,
            StationRef::new(a, 0),
            StationRef::new(b, 0),
            &merges,
        );
        assert!(reason.is_some_and(|r| r.contains("no longer exists")));
    }
}

#[test]
fn more_shots_than_a_section_holds_is_refused() {
    let half = Project::max_shots_per_section() / 2 + 1;
    let mut project = Project::new("Big".to_string(), None);
    let kept = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 1.0); half],
            ..Default::default()
        })
        .id;
    let other = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 1.0); half],
            ..Default::default()
        })
        .id;
    let target = xy(&project, kept, half as u32);
    place(&mut project, other, 0, target);
    assert!(blocked(&project, kept, other).contains("at most 99999"));
    assert!(project.combine_sections(kept, other, &options()).is_err());
    assert_eq!(project.sections.len(), 2);
}

#[test]
fn an_unaligned_closure_between_the_sections_stays_as_a_loop() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let c = closure(
        &project,
        StationRef::new(kept, 1),
        StationRef::new(other, 2),
        false,
    );
    project.loop_closures.push(c);
    assert_eq!(blocked(&project, kept, other), "");

    project.combine_sections(kept, other, &options()).unwrap();

    let c = &project.loop_closures[0];
    assert_eq!(
        (
            project.resolve_key(&c.station_a),
            project.resolve_key(&c.station_b)
        ),
        (
            Some(StationRef::new(kept, 1)),
            Some(StationRef::new(kept, 4))
        )
    );
}

#[test]
fn a_closure_between_the_joined_ends_is_dropped() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let c = closure(
        &project,
        StationRef::new(kept, 2),
        StationRef::new(other, 0),
        false,
    );
    project.loop_closures.push(c);

    project.combine_sections(kept, other, &options()).unwrap();

    assert!(project.loop_closures.is_empty());
}

#[test]
fn sections_that_do_not_meet_are_not_candidates() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    project.adjust_section_offset(other, 0.002, 0.0);
    assert!(project.combine_candidate(kept, other).is_none());
    assert!(project.combine_candidate(kept, kept).is_none());
    assert!(project.combine_sections(kept, other, &options()).is_err());
}

#[test]
fn ends_merged_but_apart_still_meet() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    project.adjust_section_offset(other, 5.0, 0.0);
    let m = merge(
        &project,
        StationRef::new(kept, 2),
        StationRef::new(other, 0),
    );
    project.station_merges.push(m);
    let candidate = project.combine_candidate(kept, other).unwrap();
    assert_eq!(candidate.pairing, CombinePairing::KeptEndOtherStart);
    assert_eq!(candidate.blocked, None);
}

#[test]
fn the_candidate_at_a_station_needs_a_section_end_and_one_partner() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    let found = project
        .combine_candidate_at_station(StationRef::new(other, 0))
        .unwrap();
    assert_eq!((found.kept, found.other), (kept, other));
    assert!(project
        .combine_candidate_at_station(StationRef::new(kept, 1))
        .is_none());
    assert!(project
        .combine_candidate_at_station(StationRef::new(kept, 0))
        .is_none());

    add_side(&mut project, kept);
    assert!(
        project
            .combine_candidate_at_station(StationRef::new(kept, 2))
            .is_none(),
        "two sections end there"
    );
}

#[test]
fn splitting_then_combining_restores_the_section() {
    let mut project = Project::new("Round trip".to_string(), None);
    let main = project
        .add_section(SectionData {
            name: Some("Main".to_string()),
            surveyor: Some("Bob".to_string()),
            shots: vec![
                sloped(90.0, 10.0, 10.0),
                sloped(90.0, 5.0, 12.0),
                sloped(0.0, 7.0, 14.0),
                sloped(45.0, 3.0, 16.0),
            ],
            ..Default::default()
        })
        .id;
    let side = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 4.0)],
            ..Default::default()
        })
        .id;
    project.set_section_offset(main, (100.0, -40.0));
    project
        .merge_stations(StationRef::new(main, 2), StationRef::new(side, 0))
        .unwrap();
    for (index, text) in [(2, "split here"), (3, "past it")] {
        let key = project.key_for_ref(StationRef::new(main, index)).unwrap();
        project.station_comments.insert(key, text.to_string());
    }
    let color = project.get_section(main).unwrap().color.clone();

    let snapshot = |project: &Project| {
        let mut comments: Vec<(StationRef, String)> = project
            .station_comments
            .iter()
            .map(|(k, v)| (project.resolve_key(k).unwrap(), v.clone()))
            .collect();
        comments.sort();
        (
            uuids(project, main),
            lengths(project, main),
            (0..=shot_count(project, main))
                .map(|i| xy(project, main, i))
                .collect::<Vec<_>>(),
            comments,
            resolved_merges(project),
        )
    };
    let before = snapshot(&project);

    let second = project
        .split_section_at_station(
            main,
            2,
            &SplitSectionOptions {
                name_a: "Main-A".to_string(),
                name_b: "Main-B".to_string(),
                color_a: "#111111".to_string(),
                color_b: "#222222".to_string(),
                comment_side: SplitSide::A,
                datum_side: SplitSide::A,
            },
        )
        .unwrap();
    project
        .combine_sections(
            main,
            second,
            &CombineOptions {
                name: "Main".to_string(),
                color: color.clone(),
                ..options()
            },
        )
        .unwrap();

    assert_eq!(snapshot(&project), before);
    assert_eq!(project.sections.len(), 2);
    let section = project.get_section(main).unwrap();
    assert_eq!(section.name.as_deref(), Some("Main"));
    assert_eq!(section.color, color);
    assert!(section.shots.iter().all(|s| s.credit.is_none()));
}

#[test]
fn a_both_side_name_or_offset_choice_is_refused() {
    let (mut project, kept, other) = joined(CombinePairing::KeptEndOtherStart);
    for options in [
        CombineOptions {
            name_side: SplitSide::Both,
            ..options()
        },
        CombineOptions {
            offset_side: SplitSide::Both,
            ..options()
        },
    ] {
        assert!(project.combine_sections(kept, other, &options).is_err());
    }
    assert_eq!(project.sections.len(), 2);
}
