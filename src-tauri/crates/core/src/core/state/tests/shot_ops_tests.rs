//! Shot operations and split_shot_at_distance (#59).
//!
//! Lines 4339-5578 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_holding, state_with_project};

// === Shot Operations Tests ===

#[test]
fn test_create_shot() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    let section_id = create_result.section_id.unwrap();

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: shot(10.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 1);
    assert_eq!(project.sections[0].shots[0].length, 10.0);
}

#[test]
fn test_update_shot() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(15.0),
            comment: Some(Some("Updated shot".to_string())),
            ..Default::default()
        },
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots[0].length, 15.0);
    assert_eq!(
        project.sections[0].shots[0].comment,
        Some("Updated shot".to_string())
    );
}

#[test]
fn test_delete_shot() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    let result = state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 1);
}

#[test]
fn test_delete_first_shot_preserves_station_positions() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project_before = state.get_project_state().unwrap();
    let station_1_before = project_before
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    let station_2_before = project_before
        .compute_station_raw_coords(StationRef::new(section_id, 2))
        .unwrap();

    let first_shot_id = project_before.sections[0].shots[0].id;
    let delete_result = state.execute(Command::DeleteShot {
        section_id,
        shot_id: first_shot_id,
    });
    assert!(delete_result.success);

    let project_after = state.get_project_state().unwrap();
    let station_0_after = project_after
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    let station_1_after = project_after
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();

    let eps = 1e-6;
    // X,Y positions preserved: station_0_after matches station_1_before in X,Y
    assert!(
        (station_0_after.x - station_1_before.x).abs() < eps,
        "x mismatch after delete"
    );
    assert!(
        (station_0_after.y - station_1_before.y).abs() < eps,
        "y mismatch after delete"
    );
    assert!(
        (station_1_after.x - station_2_before.x).abs() < eps,
        "x mismatch after delete"
    );
    assert!(
        (station_1_after.y - station_2_before.y).abs() < eps,
        "y mismatch after delete"
    );
    // Z is absolute from shot depth measurements - after deleting first shot,
    // remaining stations get Z from the remaining shot's absolute depths.
    // Remaining shot: depth_in=10, depth_out=11
    assert!(
        (station_0_after.z - 10.0).abs() < eps,
        "z station 0 after delete should be depth_in of remaining shot"
    );
    assert!(
        (station_1_after.z - 11.0).abs() < eps,
        "z station 1 after delete should be depth_out of remaining shot"
    );
}

#[test]
fn test_delete_shot_rewires_merges() {
    let state = state_with_project("Test");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(7.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(8.0)],
        })
        .section_id
        .unwrap();

    let merge_ab = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 0),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_ab.success);

    let merge_ac = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 0),
        station_b: StationRef::new(sec_c, 0),
    });
    assert!(merge_ac.success);

    let project_before = state.get_project_state().unwrap();
    let first_shot_id = project_before
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;

    let delete_result = state.execute(Command::DeleteShot {
        section_id: sec_a,
        shot_id: first_shot_id,
    });
    assert!(delete_result.success);

    let project_after = state.get_project_state().unwrap();
    let has_merge_bc = project_after.station_merges.iter().any(|m| {
        (project_after.key_is_ref(&m.station_a, StationRef::new(sec_b, 0))
            && project_after.key_is_ref(&m.station_b, StationRef::new(sec_c, 0)))
            || (project_after.key_is_ref(&m.station_a, StationRef::new(sec_c, 0))
                && project_after.key_is_ref(&m.station_b, StationRef::new(sec_b, 0)))
    });
    assert!(
        has_merge_bc,
        "Expected replacement merge between B:0 and C:0"
    );
}

#[test]
fn test_delete_shot_rejects_frozen_shot() {
    // Issue #57 (Item 3): a frozen shot (geometry anchored to
    // an adjusted survey network) must not be deletable via the resolve flow.
    // Build a two-section loop with a datum at each end so every shot between the
    // datums is frozen, then assert DeleteShot is refused and the shot survives.
    //
    // The loop topology (a:0<->b:0 and a:2<->b:2, two datums) is built in memory
    // directly - the command-level MergeStations validation refuses merging two
    // stations already in the same connected component, which the second
    // end-to-end merge would be. The real delete_shot command guard is still
    // exercised below.
    let mut project = Project::new("Frozen".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![shot(5.0), shot(7.0)],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![shot(5.0), shot(7.0)],
            ..Default::default()
        })
        .id;

    // Close the loop: both ends of A merged onto both ends of B.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 0).into(),
        station_b: StationRef::new(sec_b, 0).into(),
        merged_at: chrono::Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 2).into(),
        station_b: StationRef::new(sec_b, 2).into(),
        merged_at: chrono::Utc::now(),
    });
    // Two datums, one per section head, freeze every shot in the loop.
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_a, 0).into(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_b, 0).into(),
        gps: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
    });

    // SELF-VERIFY the chosen shot is genuinely frozen before asserting the
    // rejection, so the test fails loudly if the setup didn't freeze it.
    let frozen = project.compute_frozen_shots();
    assert!(
        frozen
            .iter()
            .any(|f| f.section_id == sec_a && f.shot_index == 0),
        "precondition: sec_a shot[0] must be frozen by the loop+two-datum setup"
    );
    let target_shot_id = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;
    let shots_before = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .len();

    let state = state_holding(project);

    // The delete must be refused with a clear frozen message.
    let result = state.execute(Command::DeleteShot {
        section_id: sec_a,
        shot_id: target_shot_id,
    });
    assert!(!result.success, "deleting a frozen shot must be refused");
    assert!(
        result.error.as_deref().unwrap_or("").contains("frozen"),
        "error must mention the shot is frozen, got: {:?}",
        result.error
    );

    // The shot still exists - nothing was removed.
    let after = state.get_project_state().unwrap();
    let shots_after = after
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .len();
    assert_eq!(
        shots_after, shots_before,
        "the frozen shot must still exist"
    );
}

#[test]
fn test_delete_shot_and_split_rejects_frozen_shot() {
    // #241 (P0): the single-shot delete+split path (execute_delete_shot_and_split)
    // must enforce the same frozen-geometry guard as delete_shot and the range
    // split/shift paths. Without it, a frozen MID-section shot can be deleted
    // through the split, silently breaking an LSQ-adjusted network. Build a
    // two-section loop with a datum at each end so every shot is frozen, then
    // split at a genuine INTERIOR shot (so it clears the edge-shot pre-check and
    // reaches the frozen guard) and assert it is refused with nothing changed.
    let mut project = Project::new("FrozenSplit".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![shot(5.0), shot(7.0), shot(4.0)],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![shot(5.0), shot(7.0), shot(4.0)],
            ..Default::default()
        })
        .id;

    // Close the loop: both ends of A merged onto both ends of B (3 shots => the
    // section tail is station index 3).
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 0).into(),
        station_b: StationRef::new(sec_b, 0).into(),
        merged_at: chrono::Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 3).into(),
        station_b: StationRef::new(sec_b, 3).into(),
        merged_at: chrono::Utc::now(),
    });
    // Two datums, one per section head, freeze every shot in the loop.
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_a, 0).into(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_b, 0).into(),
        gps: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
    });

    // SELF-VERIFY the interior split shot (index 1) is genuinely frozen.
    let frozen = project.compute_frozen_shots();
    assert!(
        frozen
            .iter()
            .any(|f| f.section_id == sec_a && f.shot_index == 1),
        "precondition: sec_a interior shot[1] must be frozen by the loop+two-datum setup"
    );

    let sec_a_shots = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .clone();
    let mid_shot_id = sec_a_shots[1].id;
    let shots_before = sec_a_shots.len();
    let sections_before = project.sections.len();

    let state = state_holding(project);

    let result = state.execute_delete_shot_and_split(
        sec_a,
        mid_shot_id,
        "A-1".to_string(),
        "A-2".to_string(),
    );
    assert!(
        !result.success,
        "splitting by deleting a frozen shot must be refused"
    );
    assert!(
        result.error.as_deref().unwrap_or("").contains("frozen"),
        "error must mention the shot is frozen, got: {:?}",
        result.error
    );

    // Nothing changed: no new section, the frozen shot still exists.
    let after = state.get_project_state().unwrap();
    assert_eq!(
        after.sections.len(),
        sections_before,
        "a refused split must not create a new section"
    );
    let shots_after = after
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .len();
    assert_eq!(
        shots_after, shots_before,
        "the frozen shot must still exist"
    );
}

// === split_shot_at_distance tests (#59: insert a station mid-shot) ===

#[test]
fn test_split_shot_at_distance_happy_path() {
    let mut project = Project::new("Split".to_string(), None);
    let sec = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![
                shot(5.0),
                ShotData {
                    uuid: None,
                    azimuth_in: 30.0,
                    azimuth_out: 32.0,
                    length: 10.0,
                    depth_in: 10.0,
                    depth_out: 20.0,
                    pitch_in: Some(10.0),
                    pitch_out: Some(-5.0),
                    left: Some(1.0),
                    right: Some(2.0),
                    up: Some(0.5),
                    down: Some(0.25),
                    comment: Some("mid comment".to_string()),
                    kind: ShotKind::Normal,
                    marker_index: Some(3),
                    temperature: Some(15.0),
                },
                shot(5.0),
            ],
            ..Default::default()
        })
        .id;

    let target = project.sections[0].shots[1].clone();
    let target_shot_id = target.id;
    let target_uuid = target.uuid;

    let state = state_holding(project);

    let result = state.execute_split_shot_at_distance(sec, target_shot_id, 4.0);
    assert!(result.success, "{:?}", result.error);
    let new_shot_id = result.shot_id.expect("new shot id");

    let after = state.get_project_state().unwrap();
    let section = after.get_section(sec).unwrap();
    assert_eq!(section.shots.len(), 4);

    let head = &section.shots[1];
    let tail = &section.shots[2];

    assert_eq!(head.id, new_shot_id);
    assert_ne!(
        head.uuid, target_uuid,
        "the head shot must get a fresh uuid"
    );
    assert_eq!(head.length, 4.0);
    assert_eq!(head.depth_in, 10.0);
    assert!(
        (head.depth_out - 14.0).abs() < 1e-9,
        "mid depth interpolated"
    );
    assert_eq!(head.azimuth_in, 30.0);
    assert_eq!(head.azimuth_out, 32.0);
    assert_eq!(head.pitch_in, Some(10.0));
    assert_eq!(head.pitch_out, Some(-5.0));
    assert_eq!(head.left, Some(1.0));
    assert_eq!(head.right, Some(2.0));
    assert_eq!(head.up, Some(0.5));
    assert_eq!(head.down, Some(0.25));
    assert_eq!(head.comment, None);
    assert_eq!(head.marker_index, None);
    assert_eq!(head.temperature, Some(15.0));
    assert!(!head.kind.is_virtual());

    assert_eq!(
        tail.id, target_shot_id,
        "the tail keeps the original shot id"
    );
    assert_eq!(tail.uuid, target_uuid, "the tail keeps the original uuid");
    assert_eq!(tail.length, 6.0);
    assert!((tail.depth_in - 14.0).abs() < 1e-9);
    assert_eq!(tail.depth_out, 20.0);
    assert_eq!(tail.comment, Some("mid comment".to_string()));
    assert_eq!(tail.marker_index, Some(3));

    // Both children pin the parent shot's LRUD unchanged (a deliberate
    // mid-station estimate -- see the split_shot_at_distance docstring).
    assert_eq!(head.left, tail.left, "head LRUD must equal tail LRUD");
    assert_eq!(head.right, tail.right, "head LRUD must equal tail LRUD");
    assert_eq!(head.up, tail.up, "head LRUD must equal tail LRUD");
    assert_eq!(head.down, tail.down, "head LRUD must equal tail LRUD");
    assert_eq!(
        (head.left, head.right, head.up, head.down),
        (Some(1.0), Some(2.0), Some(0.5), Some(0.25)),
        "head/tail LRUD must equal the parent shot's LRUD"
    );
}

#[test]
fn test_split_shot_at_distance_preserves_anchor_on_endpoint_station() {
    // A cross-reference anchored on the parent shot's ENDPOINT station (the
    // boundary right after it) must resolve to the shifted-by-one endpoint
    // after the split, NOT to the new mid station -- the original shot's uuid
    // staying on the tail child is what makes this automatic.
    // sec_a's first two (flat, opposite-azimuth, equal-length) shots cancel
    // out, so station A:2 lands back at the section's own origin (0,0,0) --
    // matching sec_b's station B:0 (always at its own origin) without needing
    // any offset-alignment machinery, so the merge is geometrically valid.
    let mut project = Project::new("Anchor".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 3.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 180.0,
                    azimuth_out: 180.0,
                    length: 3.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                },
                shot(5.0),
            ],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![shot(5.0)],
            ..Default::default()
        })
        .id;

    let target_shot_id = project.sections[0].shots[1].id;
    let endpoint_key = project.key_for_ref(StationRef::new(sec_a, 2)).unwrap();
    project.station_merges.push(StationMerge {
        station_a: endpoint_key.clone(),
        station_b: project.key_for_ref(StationRef::new(sec_b, 0)).unwrap(),
        merged_at: chrono::Utc::now(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: endpoint_key.clone(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    project
        .station_comments
        .insert(endpoint_key, "endpoint note".to_string());

    let state = state_holding(project);

    let result = state.execute_split_shot_at_distance(sec_a, target_shot_id, 1.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();

    let merge = after
        .station_merges
        .iter()
        .find(|m| {
            after.key_is_ref(&m.station_a, StationRef::new(sec_b, 0))
                || after.key_is_ref(&m.station_b, StationRef::new(sec_b, 0))
        })
        .expect("merge survives the split");
    let our_key = if after.key_is_ref(&merge.station_a, StationRef::new(sec_b, 0)) {
        &merge.station_b
    } else {
        &merge.station_a
    };
    assert_eq!(
        after.resolve_key(our_key),
        Some(StationRef::new(sec_a, 3)),
        "the endpoint merge must resolve to index p+2, not the new mid station"
    );

    assert_eq!(after.zero_datums.len(), 1);
    assert_eq!(
        after.resolve_key(&after.zero_datums[0].station_ref),
        Some(StationRef::new(sec_a, 3))
    );

    assert_eq!(after.station_comments.len(), 1);
    let (comment_key, comment) = after.station_comments.iter().next().unwrap();
    assert_eq!(comment, "endpoint note");
    assert_eq!(
        after.resolve_key(comment_key),
        Some(StationRef::new(sec_a, 3))
    );
}

#[test]
fn test_split_shot_at_distance_preserves_loop_closure_anchor_on_endpoint_station() {
    // Same shape as test_split_shot_at_distance_preserves_anchor_on_endpoint_station,
    // but for a LoopClosure anchored on the parent shot's endpoint station -- the
    // one anchor class that test doesn't cover. sec_a's first two (flat,
    // opposite-azimuth, equal-length) shots cancel out, so station A:2 lands
    // back at the section's own origin (0,0,0) -- a genuine loop back to A:0.
    let mut project = Project::new("ClosureAnchor".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 3.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 180.0,
                    azimuth_out: 180.0,
                    length: 3.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                },
                shot(5.0),
            ],
            ..Default::default()
        })
        .id;

    let target_shot_id = project.sections[0].shots[1].id;
    let start_key = project.key_for_ref(StationRef::new(sec_a, 0)).unwrap();
    let endpoint_key = project.key_for_ref(StationRef::new(sec_a, 2)).unwrap();
    project.loop_closures.push(LoopClosure {
        id: 0,
        station_a: endpoint_key,
        station_b: start_key,
        path_length: 6.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    let state = state_holding(project);

    let result = state.execute_split_shot_at_distance(sec_a, target_shot_id, 1.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    assert_eq!(after.loop_closures.len(), 1);
    let closure = &after.loop_closures[0];
    assert_eq!(
        after.resolve_key(&closure.station_a),
        Some(StationRef::new(sec_a, 3)),
        "the endpoint closure anchor must resolve to index p+2, not the new mid station"
    );
    assert_eq!(
        after.resolve_key(&closure.station_b),
        Some(StationRef::new(sec_a, 0)),
        "the closure's other endpoint (before the split point) is unaffected"
    );
}

#[test]
fn test_split_shot_at_distance_first_shot_keeps_section_start_anchor() {
    let mut project = Project::new("FirstSplit".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![shot(10.0), shot(5.0)],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![shot(5.0)],
            ..Default::default()
        })
        .id;
    let target_shot_id = project.sections[0].shots[0].id;

    let start_key = project.key_for_ref(StationRef::new(sec_a, 0)).unwrap();
    assert_eq!(start_key.anchor, StationAnchor::SectionStart);
    project.station_merges.push(StationMerge {
        station_a: start_key.clone(),
        station_b: project.key_for_ref(StationRef::new(sec_b, 0)).unwrap(),
        merged_at: chrono::Utc::now(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: start_key,
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });

    let state = state_holding(project);

    let result = state.execute_split_shot_at_distance(sec_a, target_shot_id, 4.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    assert_eq!(
        after.resolve_key(&after.zero_datums[0].station_ref),
        Some(StationRef::new(sec_a, 0)),
        "SectionStart-anchored datum must still resolve to station 0"
    );
    let merge = after
        .station_merges
        .iter()
        .find(|m| {
            after.key_is_ref(&m.station_a, StationRef::new(sec_b, 0))
                || after.key_is_ref(&m.station_b, StationRef::new(sec_b, 0))
        })
        .expect("merge survives");
    let our_key = if after.key_is_ref(&merge.station_a, StationRef::new(sec_b, 0)) {
        &merge.station_b
    } else {
        &merge.station_a
    };
    assert_eq!(after.resolve_key(our_key), Some(StationRef::new(sec_a, 0)));
}

#[test]
fn test_split_shot_at_distance_rejects_invalid_distances() {
    let mut project = Project::new("Invalid".to_string(), None);
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![shot(10.0)],
        ..Default::default()
    });
    let sec = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;
    let before_json = serde_json::to_string(&project).unwrap();

    let state = state_holding(project);

    // 0.005 and length - 0.005 = 9.995 both fall under the MIN_SPLIT_CHILD_LENGTH_M
    // (0.01m) floor and must be rejected even though they are strictly between
    // 0 and the shot's length.
    for distance in [0.0, 10.0, -1.0, f64::NAN, f64::INFINITY, 11.0, 0.005, 9.995] {
        let result = state.execute_split_shot_at_distance(sec, shot_id, distance);
        assert!(!result.success, "distance {distance} must be rejected");
    }

    let after = state.get_project_state().unwrap();
    let after_json = serde_json::to_string(&after).unwrap();
    assert_eq!(
        before_json, after_json,
        "a rejected split must leave the project untouched"
    );
}

#[test]
fn test_split_shot_at_distance_accepts_exact_min_child_length_boundary() {
    // A 1m shot split exactly at the MIN_SPLIT_CHILD_LENGTH_M (0.01m) floor
    // must be accepted: both children end up exactly at the floor, not below it.
    let mut project = Project::new("Boundary".to_string(), None);
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    let sec = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;

    let state = state_holding(project);

    let result = state.execute_split_shot_at_distance(sec, shot_id, MIN_SPLIT_CHILD_LENGTH_M);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    let section = after.get_section(sec).unwrap();
    assert_eq!(section.shots.len(), 2);
    assert!((section.shots[0].length - MIN_SPLIT_CHILD_LENGTH_M).abs() < 1e-12);
    assert!((section.shots[1].length - (1.0 - MIN_SPLIT_CHILD_LENGTH_M)).abs() < 1e-12);
}

#[test]
fn test_split_shot_at_distance_rejects_frozen_shot() {
    // Mirrors test_delete_shot_and_split_rejects_frozen_shot: a two-section
    // loop with a datum at each end freezes every shot in it.
    let mut project = Project::new("FrozenSplit".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![shot(5.0), shot(7.0), shot(4.0)],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![shot(5.0), shot(7.0), shot(4.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 0).into(),
        station_b: StationRef::new(sec_b, 0).into(),
        merged_at: chrono::Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 3).into(),
        station_b: StationRef::new(sec_b, 3).into(),
        merged_at: chrono::Utc::now(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_a, 0).into(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_b, 0).into(),
        gps: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
    });

    let frozen = project.compute_frozen_shots();
    assert!(
        frozen
            .iter()
            .any(|f| f.section_id == sec_a && f.shot_index == 1),
        "precondition: sec_a interior shot[1] must be frozen"
    );
    let target_shot_id = project.sections[0].shots[1].id;
    let shots_before = project.sections[0].shots.len();

    let state = state_holding(project);

    let result = state.execute_split_shot_at_distance(sec_a, target_shot_id, 3.0);
    assert!(!result.success, "splitting a frozen shot must be refused");
    assert!(
        result.error.as_deref().unwrap_or("").contains("frozen"),
        "error must mention the shot is frozen, got: {:?}",
        result.error
    );

    let after = state.get_project_state().unwrap();
    assert_eq!(
        after.get_section(sec_a).unwrap().shots.len(),
        shots_before,
        "the frozen shot must not be split"
    );
}

#[test]
fn test_split_shot_at_distance_undo_redo_round_trip() {
    let state = state_with_project("Split Undo");
    let create_result = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(10.0), shot(5.0)],
    });
    let section_id = create_result.section_id.unwrap();

    let before_json = {
        let proj = state.get_project_state().unwrap();
        serde_json::to_string(&proj).unwrap()
    };
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result = state.execute_split_shot_at_distance(section_id, shot_id, 4.0);
    assert!(result.success, "{:?}", result.error);
    let split_json = {
        let proj = state.get_project_state().unwrap();
        assert_eq!(proj.get_section(section_id).unwrap().shots.len(), 4);
        serde_json::to_string(&proj).unwrap()
    };

    let undo_result = state.undo();
    assert!(undo_result.success, "{:?}", undo_result.error);
    let after_undo = state.get_project_state().unwrap();
    assert_eq!(after_undo.get_section(section_id).unwrap().shots.len(), 3);
    assert_eq!(
        after_undo.get_section(section_id).unwrap().shots[1].id,
        shot_id,
        "undo restores the original single shot"
    );
    assert_eq!(
        serde_json::to_string(&after_undo).unwrap(),
        before_json,
        "undo must restore the exact pre-split project"
    );

    let redo_result = state.redo();
    assert!(redo_result.success, "{:?}", redo_result.error);
    let after_redo = state.get_project_state().unwrap();
    assert_eq!(
        serde_json::to_string(&after_redo).unwrap(),
        split_json,
        "redo must re-split identically"
    );
}

#[test]
fn test_split_shot_at_distance_preserves_downstream_geometry() {
    let state = state_with_project("Geo");
    let create_result = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            ShotData {
                azimuth_in: 10.0,
                azimuth_out: 10.0,
                length: 5.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 50.0,
                azimuth_out: 50.0,
                length: 8.0,
                depth_in: 1.0,
                depth_out: 4.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 120.0,
                azimuth_out: 120.0,
                length: 6.0,
                depth_in: 4.0,
                depth_out: 2.0,
                ..Default::default()
            },
        ],
    });
    let section_id = create_result.section_id.unwrap();

    let before_offset = state
        .get_project_state()
        .unwrap()
        .section_offset(section_id);
    let before_coords = abs_coord_map(&state.get_project_state().unwrap());
    let start_before = before_coords[&StationRef::new(section_id, 0)];
    let last_before = before_coords[&StationRef::new(section_id, 3)];

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result = state.execute_split_shot_at_distance(section_id, shot_id, 3.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    assert_eq!(
        before_offset,
        after.section_offset(section_id),
        "a collinear split must not require a section-offset adjustment"
    );

    let after_coords = abs_coord_map(&after);
    assert_coords_close(start_before, after_coords[&StationRef::new(section_id, 0)]);
    // Station 3 shifts to index 4 (one more shot inserted before it); its
    // absolute position must be exactly preserved.
    assert_coords_close(last_before, after_coords[&StationRef::new(section_id, 4)]);
}

#[test]
fn test_split_shot_at_distance_reference_shot_both_children_reference() {
    let mut project = Project::new("Ref".to_string(), None);
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![ShotData {
            kind: ShotKind::Jump,
            ..shot(10.0)
        }],
        ..Default::default()
    });
    let sec = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;

    let state = state_holding(project);

    let result = state.execute_split_shot_at_distance(sec, shot_id, 4.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    let section = after.get_section(sec).unwrap();
    assert!(
        section.shots[0].kind.is_virtual(),
        "head must inherit the shot kind"
    );
    assert!(
        section.shots[1].kind.is_virtual(),
        "tail keeps its own kind"
    );
}

#[test]
fn test_split_shot_at_distance_rejects_at_max_shots_cap() {
    let mut project = Project::new("Cap".to_string(), None);
    let max = crate::models::FEATURE_ID_STRIDE as usize - 1;
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![shot(1.0); max],
        ..Default::default()
    });
    let sec = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;

    let state = state_holding(project);

    let result = state.execute_split_shot_at_distance(sec, shot_id, 0.5);
    assert!(
        !result.success,
        "split must be refused when the section is at the max-shots cap"
    );
    assert!(
        result.error.as_deref().unwrap_or("").contains("maximum"),
        "error should mention the max-shots cap, got: {:?}",
        result.error
    );
}

#[test]
fn test_legacy_project_seeds_validation_from_global_config() {
    // Issue #57 (Item 5): a legacy project saved before per-project validation
    // (no `validation` row in project_settings) must open with the user's GLOBAL
    // thresholds - the "defaults for new projects" - not the hardcoded
    // ValidationConfig::default(). A project that DOES carry an explicit row keeps
    // its own thresholds.
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();

        // A 1.0 m shot is fine under the hardcoded default (warn_shot_length_min
        // = 0.3) but SHORT under a raised global minimum.
        let custom = crate::config::ValidationConfig {
            warn_shot_length_min: 5.0,
            ..crate::config::ValidationConfig::default()
        };

        // --- Legacy file: saved, then strip the validation row to simulate a
        //     project written before the feature existed. ---
        let legacy_path = dir.path().join("legacy.stalx");
        {
            let mut project = Project::new("Legacy".to_string(), None);
            project.add_section(SectionData {
                shots: vec![shot(1.0)],
                ..Default::default()
            });
            let mut db = Database::open(&legacy_path).unwrap();
            db.save_project(&project).unwrap();
            drop(db);
            let conn = rusqlite::Connection::open(&legacy_path).unwrap();
            conn.execute("DELETE FROM project_settings WHERE key = 'validation'", [])
                .unwrap();
            drop(conn);
        }
        // Sanity: the file now looks legacy (no stored validation row).
        assert!(
            Database::open_existing(&legacy_path)
                .unwrap()
                .load_project_validation()
                .unwrap()
                .is_none(),
            "precondition: legacy file has no stored validation row"
        );

        // Open via the production command path on a fresh AppState whose GLOBAL
        // config carries the customized threshold.
        let state = AppState::new().unwrap();
        let mut config = state.get_config();
        config.validation = custom.clone();
        state.update_config(config).unwrap();

        assert!(state.open_project(legacy_path).success, "open must succeed");

        let opened = state.get_project_state().unwrap();
        assert_eq!(
            opened.validation.warn_shot_length_min, custom.warn_shot_length_min,
            "legacy project must inherit the user's global thresholds"
        );
        // The customized threshold actually drives warnings: the 1.0 m shot is now SHORT.
        assert!(
            state
                .compute_project_warnings(&opened, None)
                .iter()
                .any(|w| w.code == "SHORT_SHOT"),
            "the seeded global threshold must produce the SHORT_SHOT warning"
        );

        // --- Explicit-row file: a project that carries its OWN validation row
        //     (different from global) must NOT be overridden on open. ---
        let explicit_path = dir.path().join("explicit.stalx");
        {
            let mut project = Project::new("Explicit".to_string(), None);
            project.validation = crate::config::ValidationConfig {
                warn_shot_length_min: 0.1,
                ..crate::config::ValidationConfig::default()
            };
            project.add_section(SectionData {
                shots: vec![shot(1.0)],
                ..Default::default()
            });
            let mut db = Database::open(&explicit_path).unwrap();
            db.save_project(&project).unwrap();
        }
        let state2 = AppState::new().unwrap();
        let mut config2 = state2.get_config();
        config2.validation = custom.clone();
        state2.update_config(config2).unwrap();
        assert!(state2.open_project(explicit_path).success);
        let opened2 = state2.get_project_state().unwrap();
        assert_eq!(
            opened2.validation.warn_shot_length_min, 0.1,
            "a project with an explicit validation row keeps its own thresholds"
        );
    });
}
