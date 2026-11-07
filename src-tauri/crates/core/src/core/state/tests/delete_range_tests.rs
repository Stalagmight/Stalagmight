//! Delete shot and split section; bulk shot range deletion (#95), part 1.
//!
//! Lines 12907-14125 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === Delete Shot and Split Section Tests ===

#[test]
fn test_split_section_basic() {
    // 5-shot section, split at shot index 2 -> A has 2 shots, B has 2 shots
    let state = state_with_project("Split Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: Some("TestSurveyor".to_string()),
        date: None,
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0), shot(6.0)],
    });
    assert!(create_result.success);
    let section_id = create_result.section_id.unwrap();

    // Get the shot_id for the middle shot (index 2)
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        let section = proj.get_section(section_id).unwrap();
        section.shots[2].id
    };

    let result = state.execute_delete_shot_and_split(
        section_id,
        shot_id,
        "Section A".to_string(),
        "Section B".to_string(),
    );

    assert!(result.success, "Split should succeed: {:?}", result.error);
    let new_section_id = result.section_id.unwrap();

    // Verify sections
    let proj = state.get_project_state().unwrap();
    let section_a = proj.get_section(section_id).unwrap();
    let section_b = proj.get_section(new_section_id).unwrap();

    assert_eq!(section_a.name, Some("Section A".to_string()));
    assert_eq!(section_a.shots.len(), 2);
    assert_eq!(section_a.shots[0].length, 5.0);
    assert_eq!(section_a.shots[1].length, 7.0);

    assert_eq!(section_b.name, Some("Section B".to_string()));
    assert_eq!(section_b.shots.len(), 2);
    assert_eq!(section_b.shots[0].length, 4.0);
    assert_eq!(section_b.shots[1].length, 6.0);
}

#[test]
fn test_split_preserves_metadata() {
    let state = state_with_project("Metadata Test");

    let date = chrono::NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: Some("Jane".to_string()),
        date: Some(date),
        color: "#aabbcc".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![shot(5.0), shot(7.0), shot(3.0)],
    });
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);
    let new_id = result.section_id.unwrap();

    let proj = state.get_project_state().unwrap();
    let section_a = proj.get_section(section_id).unwrap();
    let section_b = proj.get_section(new_id).unwrap();

    // Both sections inherit metadata
    assert_eq!(section_a.surveyor, Some("Jane".to_string()));
    assert_eq!(section_a.date, Some(date));
    assert_eq!(section_a.color, "#aabbcc");
    assert_eq!(section_a.direction, SurveyDirection::Out);

    assert_eq!(section_b.surveyor, Some("Jane".to_string()));
    assert_eq!(section_b.date, Some(date));
    assert_eq!(section_b.color, "#aabbcc");
    assert_eq!(section_b.direction, SurveyDirection::Out);
}

#[test]
fn test_split_recalculates_offset_b() {
    let state = state_with_project("Offset Test");

    // Create section with known geometry: 3 shots going east (azimuth 90)
    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(10.0).with_azimuths(90.0, 90.0), // East
            shot(5.0).with_azimuths(90.0, 90.0),  // East (deleted)
            shot(8.0).with_azimuths(90.0, 90.0),  // East
        ],
    });
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);
    let new_id = result.section_id.unwrap();

    // Section B offset should be at (accumulated shots_before + deleted shot)
    // Shot lengths are slope distances; horizontal = sqrt(length^2 - dz^2)
    // With azimuth 90, sin(90) = 1, cos(90) = 0
    // So bx = horizontal_sum, by = 0 (approximately)
    let proj = state.get_project_state().unwrap();
    let b_offset = proj.section_offsets().get(&new_id);
    assert!(b_offset.is_some(), "Section B should have an offset");
    let (bx, by) = b_offset.unwrap();
    let expected =
        (10.0_f64.powi(2) - 1.0_f64.powi(2)).sqrt() + (5.0_f64.powi(2) - 1.0_f64.powi(2)).sqrt();
    assert!(
        (*bx - expected).abs() < 0.001,
        "bx should be ~{expected}, got {bx}"
    );
    assert!(by.abs() < 0.001, "by should be ~0, got {by}");
}

#[test]
fn test_split_remaps_merges() {
    let state = state_with_project("Merge Remap Test");

    // Create two sections
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToSplit".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0)],
    });
    let section_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let section_b_id = create_b.section_id.unwrap();

    // Merge station 3 of section A (after split, belongs to B index 0) with station 0 of section B
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a_id, 3), // This is after split point
        station_b: StationRef::new(section_b_id, 0),
    });
    assert!(
        merge_result.success,
        "Merge should succeed: {:?}",
        merge_result.error
    );

    // Now split at shot index 1
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_a_id).unwrap().shots[1].id
    };

    let result = state.execute_delete_shot_and_split(
        section_a_id,
        shot_id,
        "A".to_string(),
        "B".to_string(),
    );
    assert!(result.success);
    let new_section_id = result.section_id.unwrap();

    // The merge with station_index 3 should now be remapped to new_section_id, station_index 1
    // (original index 3, split_station = 2, so new index = 3 - 2 = 1)
    let proj = state.get_project_state().unwrap();
    let has_remapped_merge = proj.station_merges.iter().any(|m| {
        let a = proj.resolve_key(&m.station_a);
        let b = proj.resolve_key(&m.station_b);
        (a.is_some_and(|sr| sr.section_id == new_section_id && sr.station_index == 1)
            && m.station_b.section_id == section_b_id)
            || (b.is_some_and(|sr| sr.section_id == new_section_id && sr.station_index == 1)
                && m.station_a.section_id == section_b_id)
    });
    assert!(
        has_remapped_merge,
        "Merge should be remapped to new section with adjusted index"
    );
}

#[test]
fn test_split_remaps_closures() {
    let state = state_with_project("Closure Remap Test");

    // Create two sections and merge them first so we can create a closure
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToSplit".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0)],
    });
    let sec_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_b_id = create_b.section_id.unwrap();

    // First merge station 0 of A with station 0 of B (so they are connected)
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a_id, 0),
        station_b: StationRef::new(sec_b_id, 0),
    });
    assert!(merge_result.success);

    // Create a loop closure between station 4 of A (will be in B after split) and station 1 of B
    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(sec_a_id, 4),
        station_b: StationRef::new(sec_b_id, 1),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(
        closure_result.success,
        "Closure should succeed: {:?}",
        closure_result.error
    );

    // Split at shot index 2
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(sec_a_id).unwrap().shots[2].id
    };

    let result = state.execute_delete_shot_and_split(
        sec_a_id,
        shot_id,
        "A".to_string(),
        "B-split".to_string(),
    );
    assert!(result.success);
    let new_section_id = result.section_id.unwrap();

    // The closure's A-side is remapped: (sec_a_id, 4) -> (new_section_id, 1)
    // (split_station = 3, so 4 - 3 = 1). But the split detaches new_section so
    // that closure is now its ONLY connection to B - a sole bridge is a merge,
    // not a loop (#8), so it is converted. Assert it was remapped AND merged.
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.loop_closures.is_empty(),
        "the orphaned (sole-bridge) closure should have been converted to a merge: {:?}",
        proj.loop_closures
    );
    let has_remapped_merge = proj.station_merges.iter().any(|m| {
        proj.resolve_key(&m.station_a)
            .is_some_and(|sr| sr.section_id == new_section_id && sr.station_index == 1)
            || proj
                .resolve_key(&m.station_b)
                .is_some_and(|sr| sr.section_id == new_section_id && sr.station_index == 1)
    });
    assert!(
        has_remapped_merge,
        "the remapped closure should have become a merge on the new section: {:?}",
        proj.station_merges
    );
}

// === Bulk shot range deletion (#95) ===

#[test]
fn test_delete_range_split_interior() {
    let state = state_with_project("Range Split");
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: Some("Surv".to_string()),
        date: None,
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    // Delete shots [2..=3] (the 3.0 and 4.0 shots) and split.
    let result =
        state.execute_delete_shot_range_split(section_id, s2, s3, "A".to_string(), "B".to_string());
    assert!(
        result.success,
        "range split should succeed: {:?}",
        result.error
    );
    let new_id = result.section_id.unwrap();

    let proj = state.get_project_state().unwrap();
    let a = proj.get_section(section_id).unwrap();
    let b = proj.get_section(new_id).unwrap();
    assert_eq!(a.name, Some("A".to_string()));
    assert_eq!(
        a.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![5.0, 7.0]
    );
    assert_eq!(b.name, Some("B".to_string()));
    assert_eq!(
        b.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![6.0, 8.0]
    );
    // Metadata is inherited by both halves.
    assert_eq!(b.surveyor, Some("Surv".to_string()));
    assert_eq!(b.direction, SurveyDirection::Out);
}

#[test]
fn test_delete_range_split_preserves_after_position() {
    // East-going shots: section B's offset should equal the displacement of
    // (shots before the range ++ the deleted shots).
    let state = state_with_project("Range Offset");
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(10.0).with_azimuths(90.0, 90.0), // before
            shot(5.0).with_azimuths(90.0, 90.0),  // deleted
            shot(7.0).with_azimuths(90.0, 90.0),  // deleted
            shot(8.0).with_azimuths(90.0, 90.0),  // after
        ],
    });
    let section_id = create.section_id.unwrap();
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let result =
        state.execute_delete_shot_range_split(section_id, s1, s2, "A".to_string(), "B".to_string());
    assert!(result.success, "{:?}", result.error);
    let new_id = result.section_id.unwrap();

    let proj = state.get_project_state().unwrap();
    let (bx, by) = *proj.section_offsets().get(&new_id).expect("B offset");
    let h = |l: f64| (l * l - 1.0_f64).sqrt();
    let expected = h(10.0) + h(5.0) + h(7.0);
    assert!((bx - expected).abs() < 0.001, "bx ~{expected}, got {bx}");
    assert!(by.abs() < 0.001, "by ~0, got {by}");
}

#[test]
fn test_delete_range_shift_interior() {
    let state = state_with_project("Range Shift");
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let result = state.execute_delete_shot_range_shift(section_id, s2, s3);
    assert!(
        result.success,
        "range shift should succeed: {:?}",
        result.error
    );

    // No new section - the survivors gap-close into one section [5,7,6,8].
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.sections.len(), 1);
    let sec = proj.get_section(section_id).unwrap();
    assert_eq!(
        sec.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![5.0, 7.0, 6.0, 8.0]
    );
}

#[test]
fn test_delete_range_prefix_shift() {
    // Delete shots [0..=1] of a 5-shot section; the survivors keep their absolute
    // positions (the section offset is shifted to compensate).
    let state = state_with_project("Prefix");
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(10.0).with_azimuths(90.0, 90.0),
            shot(5.0).with_azimuths(90.0, 90.0),
            shot(7.0).with_azimuths(90.0, 90.0),
            shot(8.0).with_azimuths(90.0, 90.0),
            shot(6.0).with_azimuths(90.0, 90.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let before_abs = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let kept_before = *before_abs.get(&StationRef::new(section_id, 2)).unwrap();
    let (s0, s1) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[0].id, sec.shots[1].id)
    };
    let result = state.execute_delete_shot_range_shift(section_id, s0, s1);
    assert!(result.success, "{:?}", result.error);

    let proj = state.get_project_state().unwrap();
    let sec = proj.get_section(section_id).unwrap();
    assert_eq!(
        sec.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![7.0, 8.0, 6.0]
    );
    // Old station 2 (start of the first surviving shot) is now station 0, same place.
    let after_abs = abs_coord_map(&proj);
    let kept_after = *after_abs.get(&StationRef::new(section_id, 0)).unwrap();
    assert_xy_close(kept_before, kept_after);
}

#[test]
fn test_delete_range_suffix_shift() {
    // Delete shots [3..=4] of a 5-shot section; the head stays put.
    let state = state_with_project("Suffix");
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(10.0).with_azimuths(90.0, 90.0),
            shot(5.0).with_azimuths(90.0, 90.0),
            shot(7.0).with_azimuths(90.0, 90.0),
            shot(8.0).with_azimuths(90.0, 90.0),
            shot(6.0).with_azimuths(90.0, 90.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let head_before = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let (s3, s4) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[3].id, sec.shots[4].id)
    };
    let result = state.execute_delete_shot_range_shift(section_id, s3, s4);
    assert!(result.success, "{:?}", result.error);

    let proj = state.get_project_state().unwrap();
    let sec = proj.get_section(section_id).unwrap();
    assert_eq!(
        sec.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![10.0, 5.0, 7.0]
    );
    let after = abs_coord_map(&proj);
    for i in 0..=3u32 {
        let r = StationRef::new(section_id, i);
        assert_xy_close(*head_before.get(&r).unwrap(), *after.get(&r).unwrap());
    }
}

#[test]
fn test_delete_range_split_remaps_merge() {
    let state = state_with_project("Range Merge Remap");
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToSplit".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_c = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_c = create_c.section_id.unwrap();
    // Merge A:5 (-> new section index 1 after split [2..=3], split_station 4) with C:0.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 5),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let result =
        state.execute_delete_shot_range_split(sec_a, s2, s3, "A".to_string(), "B".to_string());
    assert!(result.success, "{:?}", result.error);
    let new_id = result.section_id.unwrap();

    let proj = state.get_project_state().unwrap();
    let has_remapped = proj.station_merges.iter().any(|m| {
        let a = proj.resolve_key(&m.station_a);
        let b = proj.resolve_key(&m.station_b);
        (a.is_some_and(|sr| sr.section_id == new_id && sr.station_index == 1)
            && m.station_b.section_id == sec_c)
            || (b.is_some_and(|sr| sr.section_id == new_id && sr.station_index == 1)
                && m.station_a.section_id == sec_c)
    });
    assert!(
        has_remapped,
        "merge A:5 should remap to new section index 1: {:?}",
        proj.station_merges
    );
}

#[test]
fn test_delete_range_preserves_interior_merge_connectivity() {
    // Two other sections (P, Q) bridge ONLY through an interior station A:3 that
    // the range delete removes. A replacement merge must keep P and Q connected.
    let state = state_with_project("Interior Bridge");
    let create_a = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_p = state.execute(Command::CreateSection {
        name: Some("P".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_p = create_p.section_id.unwrap();
    let create_q = state.execute(Command::CreateSection {
        name: Some("Q".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_q = create_q.section_id.unwrap();
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_p, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_q, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );
    {
        let proj = state.get_project_state().unwrap();
        assert!(
            proj.are_stations_merged(StationRef::new(sec_p, 0), StationRef::new(sec_q, 0)),
            "P and Q should be connected before the delete"
        );
    }

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    // Range [2..=3] removes interior station A:3 (the only P<->Q bridge).
    let result =
        state.execute_delete_shot_range_split(sec_a, s2, s3, "A".to_string(), "B".to_string());
    assert!(result.success, "{:?}", result.error);

    let proj = state.get_project_state().unwrap();
    assert!(
        proj.are_stations_merged(StationRef::new(sec_p, 0), StationRef::new(sec_q, 0)),
        "P and Q must stay connected via a replacement merge after the interior bridge is \
         removed: {:?}",
        proj.station_merges
    );
}

#[test]
fn test_delete_range_shift_blocked_aborts() {
    // A loop around section A's middle makes the shift path unavailable; the
    // shift range delete must fail and leave the project untouched.
    let state = state_with_project("Shift Blocked");
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0)],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // Form a loop directly (A:0-B:0 and A:4-B:1). MergeStations refuses the second
    // edge because the stations are already connected, so push raw merges like the
    // model-level shift_blocked test does.
    {
        let mut guard = state.project.lock();
        let proj = guard.as_mut().unwrap();
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_a, 0).into(),
            station_b: StationRef::new(sec_b, 0).into(),
            merged_at: chrono::Utc::now(),
        });
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_a, 4).into(),
            station_b: StationRef::new(sec_b, 1).into(),
            merged_at: chrono::Utc::now(),
        });
    }

    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let result = state.execute_delete_shot_range_shift(sec_a, s1, s2);
    assert!(!result.success, "shift over a loop must be blocked");
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.get_section(sec_a).unwrap().shots.len(),
        4,
        "blocked shift must not mutate the project"
    );
}

#[test]
fn test_delete_range_undo_redo_roundtrip() {
    let state = state_with_project("Range Undo");
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(10.0).with_azimuths(90.0, 90.0),
            shot(5.0).with_azimuths(45.0, 45.0),
            shot(7.0).with_azimuths(90.0, 90.0),
            shot(8.0).with_azimuths(135.0, 135.0),
            shot(6.0).with_azimuths(90.0, 90.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let before_abs = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let undo_len_before = state.get_undo_stack().len();
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let result =
        state.execute_delete_shot_range_split(section_id, s1, s2, "A".to_string(), "B".to_string());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        state.get_undo_stack().len(),
        undo_len_before + 1,
        "the whole range split is one undo entry"
    );

    assert!(state.undo().success);
    let after_undo = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    assert_eq!(before_abs.len(), after_undo.len());
    for (r, c) in &before_abs {
        let c2 = after_undo
            .get(r)
            .unwrap_or_else(|| panic!("missing {r:?} after undo"));
        assert_coords_close(*c, *c2);
    }

    assert!(state.redo().success);
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.sections.len(),
        2,
        "redo restores the split (two sections)"
    );
}

#[test]
fn test_preview_delete_shot_range_classifies_kind() {
    let state = state_with_project("Kind");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0), shot(6.0)],
    });
    let section_id = create.section_id.unwrap();
    let proj = state.get_project_state().unwrap();
    let ids: Vec<ShotId> = proj
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();

    use crate::models::RangeDeleteKind;
    assert_eq!(
        proj.preview_delete_shot_range(section_id, ids[0], ids[4])
            .unwrap()
            .kind,
        RangeDeleteKind::WholeSection
    );
    assert_eq!(
        proj.preview_delete_shot_range(section_id, ids[0], ids[2])
            .unwrap()
            .kind,
        RangeDeleteKind::Prefix
    );
    assert_eq!(
        proj.preview_delete_shot_range(section_id, ids[2], ids[4])
            .unwrap()
            .kind,
        RangeDeleteKind::Suffix
    );
    let interior = proj
        .preview_delete_shot_range(section_id, ids[1], ids[3])
        .unwrap();
    assert_eq!(interior.kind, RangeDeleteKind::Interior);
    // Reversed endpoints normalize to the same range.
    let rev = proj
        .preview_delete_shot_range(section_id, ids[3], ids[1])
        .unwrap();
    assert_eq!((rev.start_pos, rev.end_pos), (1, 3));
}

#[test]
fn test_delete_range_split_rejects_non_interior() {
    let state = state_with_project("Reject");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0)],
    });
    let section_id = create.section_id.unwrap();
    let ids: Vec<ShotId> = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id)
            .unwrap()
            .shots
            .iter()
            .map(|s| s.id)
            .collect()
    };
    // Prefix range [0..=1] is not interior - split must refuse.
    assert!(
        !state
            .execute_delete_shot_range_split(
                section_id,
                ids[0],
                ids[1],
                "A".to_string(),
                "B".to_string()
            )
            .success
    );
    // Whole-section shift must refuse (delete the section instead).
    assert!(
        !state
            .execute_delete_shot_range_shift(section_id, ids[0], ids[2])
            .success
    );

    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.get_section(section_id).unwrap().shots.len(),
        3,
        "rejections must not mutate"
    );
}

#[test]
fn test_delete_range_interior_shift_blocked_by_tail_datum() {
    // Regression (#95): an interior-range SHIFT gap-closes THROUGH station
    // end_pos+1 (which the split keeps as B:0). A zero datum anchored there must
    // block the shift - the preview previously scanned only the split's interior
    // set and missed it, so the gap-close silently destroyed the datum.
    let state = state_with_project("Tail Datum");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0), shot(6.0)],
    });
    let section_id = create.section_id.unwrap();
    // Datum on station 3 = end_pos+1 for the interior range [1..=2].
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(section_id, 3),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    {
        let proj = state.get_project_state().unwrap();
        let impact = proj.preview_delete_shot_range(section_id, s1, s2).unwrap();
        assert!(
            impact.shift_blocked,
            "shift must be blocked: it would drop the datum on station end_pos+1"
        );
        assert!(
            impact.removed_datums.is_empty(),
            "the split (display) set keeps station end_pos+1, so its datum is not in removed_datums"
        );
    }
    // The shift executor must refuse and leave the datum intact.
    let result = state.execute_delete_shot_range_shift(section_id, s1, s2);
    assert!(
        !result.success,
        "interior shift over a tail datum must be blocked"
    );
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.zero_datums.len(),
        1,
        "blocked shift must not drop the datum"
    );
    assert_eq!(
        proj.get_section(section_id).unwrap().shots.len(),
        5,
        "blocked shift must not mutate the section"
    );
}

#[test]
fn test_delete_range_prefix_shift_blocked_by_datum() {
    // Coverage (#95): a prefix range whose removed boundary station carries a
    // zero datum must block the shift rather than silently drop the anchor.
    let state = state_with_project("Prefix Datum");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0)],
    });
    let section_id = create.section_id.unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(section_id, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    let (s0, s1) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[0].id, sec.shots[1].id)
    };
    {
        let proj = state.get_project_state().unwrap();
        let impact = proj.preview_delete_shot_range(section_id, s0, s1).unwrap();
        assert!(
            impact.shift_blocked,
            "prefix shift dropping a datum must be blocked"
        );
        assert!(
            !impact.removed_datums.is_empty(),
            "the removed station-0 datum is surfaced"
        );
    }
    let result = state.execute_delete_shot_range_shift(section_id, s0, s1);
    assert!(!result.success, "prefix shift over a datum must be blocked");
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.zero_datums.len(),
        1,
        "blocked shift must not drop the datum"
    );
}

#[test]
fn test_delete_range_preview_surfaces_interior_split_datum() {
    // #95: a zero datum on a strictly-interior station the split drops must be
    // surfaced in removed_datums so the dialog can warn (the interior split is
    // not lossless for interior-station datums/comments).
    let state = state_with_project("Interior Datum");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0), shot(6.0)],
    });
    let section_id = create.section_id.unwrap();
    // Datum on station 2 - strictly interior to the range [1..=2] (interior set {2}).
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(section_id, 2),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(section_id, s1, s2).unwrap();
    assert!(
        impact.removed_datums.iter().any(|sr| sr.station_index == 2),
        "the interior-station datum the split drops must be surfaced: {:?}",
        impact.removed_datums
    );
}

#[test]
fn test_delete_range_preview_reports_replacement_merge() {
    // #95: when a removed interior station bridges two external sections, the
    // preview must report the replacement merge the delete adds (not just the
    // removed merges), so the dialog doesn't overstate connectivity loss.
    let state = state_with_project("Replacement Preview");
    let create_a = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_p = state.execute(Command::CreateSection {
        name: Some("P".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_p = create_p.section_id.unwrap();
    let create_q = state.execute(Command::CreateSection {
        name: Some("Q".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_q = create_q.section_id.unwrap();
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_p, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_q, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );

    // Range [2..=3] removes the bridging interior station A:3.
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        !impact.replacement_merges.is_empty(),
        "removing the bridging interior station must report a replacement merge: {impact:?}"
    );
}
