//! Command execution (fn-34-nvq) and validation edge cases.
//!
//! Lines 7763-8866 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === Command Execution Tests (fn-34-nvq spec) ===
// These tests verify specific command behaviors as specified in the epic.

#[test]
fn test_execute_create_section_command() {
    // Verify CreateSection creates section correctly with all fields
    use chrono::NaiveDate;
    let state = state_with_project("Test");

    let test_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
    let result = state.execute(Command::CreateSection {
        name: Some("Main Passage".to_string()),
        surveyor: Some("Alice".to_string()),
        date: Some(test_date),
        color: "#00ff00".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![shot(10.0), shot(15.0), shot(8.0)],
    });

    assert!(result.success);
    let section_id = result.section_id.expect("Section ID should be returned");

    let project = state.get_project_state().unwrap();
    let section = project
        .get_section(section_id)
        .expect("Section should exist");

    assert_eq!(section.name, Some("Main Passage".to_string()));
    assert_eq!(section.surveyor, Some("Alice".to_string()));
    assert_eq!(section.date, Some(test_date));
    assert_eq!(section.color, "#00ff00");
    assert_eq!(section.direction, SurveyDirection::Out);
    assert_eq!(section.shots.len(), 3);
    assert_eq!(section.shots[0].length, 10.0);
    assert_eq!(section.shots[1].length, 15.0);
    assert_eq!(section.shots[2].length, 8.0);
}

#[test]
fn test_execute_merge_stations_command() {
    // Verify MergeStations updates section offsets correctly
    let state = state_with_project("Test");

    // Create two sections
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0)],
    });
    let section_a_id = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0), shot(6.0)],
    });
    let section_b_id = result_b.section_id.unwrap();

    // Merge station 1 of section A with station 0 of section B
    let station_a = StationRef::new(section_a_id, 1);
    let station_b = StationRef::new(section_b_id, 0);

    let result = state.execute(Command::MergeStations {
        station_a,
        station_b,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();

    // Verify merge was recorded
    assert_eq!(project.station_merges.len(), 1);
    let merge = &project.station_merges[0];
    assert!(project.key_is_ref(&merge.station_a, station_a));
    assert!(project.key_is_ref(&merge.station_b, station_b));

    // Verify section offsets were updated - one section should have an offset
    // (the moving section gets an offset to align with the anchor)
    let has_offset = project.section_offsets().contains_key(&section_a_id)
        || project.section_offsets().contains_key(&section_b_id);
    assert!(has_offset, "Section offsets should be updated after merge");
}

#[test]
fn test_execute_set_zero_datum_command() {
    // Verify SetZeroDatum stores GPS coordinates correctly
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
    let station_ref = StationRef::new(section_id, 0);
    let gps = GpsCoordinates::new(48.8584, 2.2945, 35.0).unwrap();

    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.zero_datums.len(), 1);

    let datum = &project.zero_datums[0];
    assert!(project.key_is_ref(&datum.station_ref, station_ref));
    assert!((datum.gps.latitude - 48.8584).abs() < 0.0001);
    assert!((datum.gps.longitude - 2.2945).abs() < 0.0001);
    assert!((datum.gps.depth_reference - 35.0).abs() < 0.0001);
}

#[test]
fn test_undo_set_zero_datum_restores_section_offsets() {
    // Verify undo restores exact previous section_offsets, including
    // removing keys that were newly created by the command
    let state = state_with_project("Test");

    // Create two sections with shots
    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let section_b = result_b.section_id.unwrap();

    // Set an existing offset on section A only
    // (section B has no offset - key doesn't exist)
    {
        let mut proj = state.project.lock();
        let project = proj.as_mut().unwrap();
        project.set_section_offset(section_a, (100.0, 200.0));
    }

    // Merge the sections so they're connected
    state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a, 1),
        station_b: StationRef::new(section_b, 0),
    });

    // Clear undo stack to focus on SetZeroDatum
    state.undo_stack.lock().clear();

    // Capture pre-command state
    let (pre_offset_a, pre_offset_b, pre_datum_count) = {
        let proj = state.project.lock();
        let project = proj.as_ref().unwrap();
        (
            project.section_offsets().get(&section_a).copied(),
            project.section_offsets().get(&section_b).copied(),
            project.zero_datums.len(),
        )
    };

    // Set zero datum - this will create/modify offsets for both connected sections
    let station_ref = StationRef::new(section_a, 0);
    let gps = GpsCoordinates::new(45.0, -73.0, 100.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
    });
    assert!(result.success);

    // Verify offsets changed after SetZeroDatum
    let (post_offset_a, post_offset_b, post_datum_count) = {
        let proj = state.project.lock();
        let project = proj.as_ref().unwrap();
        (
            project.section_offsets().get(&section_a).copied(),
            project.section_offsets().get(&section_b).copied(),
            project.zero_datums.len(),
        )
    };
    assert_ne!(
        pre_offset_a, post_offset_a,
        "Section A offset should change"
    );
    assert!(
        post_offset_b.is_some(),
        "Section B should now have an offset"
    );
    assert_eq!(
        post_datum_count,
        pre_datum_count + 1,
        "Datum should be added"
    );

    // Undo the SetZeroDatum
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    // Verify exact previous state is restored
    let (restored_offset_a, restored_offset_b, restored_datum_count) = {
        let proj = state.project.lock();
        let project = proj.as_ref().unwrap();
        (
            project.section_offsets().get(&section_a).copied(),
            project.section_offsets().get(&section_b).copied(),
            project.zero_datums.len(),
        )
    };

    // Section A should have its original offset restored
    assert_eq!(
        restored_offset_a, pre_offset_a,
        "Section A offset should be restored to original value"
    );

    // Section B key should be removed (it didn't exist before)
    assert_eq!(
        restored_offset_b, pre_offset_b,
        "Section B offset key should be removed (was None before)"
    );
    assert!(
        pre_offset_b.is_none() && restored_offset_b.is_none(),
        "Section B offset key must not exist after undo"
    );

    // Datum should be removed
    assert_eq!(
        restored_datum_count, pre_datum_count,
        "Datum should be removed after undo"
    );
}

#[test]
fn test_execute_update_shot_command() {
    // Verify UpdateShot modifies shot data correctly
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 270.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 6.0,
            left: None,
            right: None,
            up: None,
            down: None,
            comment: None,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Update multiple fields
    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            azimuth_in: Some(45.0),
            length: Some(20.0),
            left: Some(Some(1.5)),
            right: Some(Some(2.0)),
            up: Some(Some(3.0)),
            down: Some(Some(0.5)),
            comment: Some(Some("Narrow squeeze".to_string())),
            ..Default::default()
        },
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.sections[0].shots[0];

    assert_eq!(shot.azimuth_in, 45.0);
    assert_eq!(shot.azimuth_out, 270.0); // Unchanged
    assert_eq!(shot.length, 20.0);
    assert_eq!(shot.depth_in, 5.0); // Unchanged
    assert_eq!(shot.left, Some(1.5));
    assert_eq!(shot.right, Some(2.0));
    assert_eq!(shot.up, Some(3.0));
    assert_eq!(shot.down, Some(0.5));
    assert_eq!(shot.comment, Some("Narrow squeeze".to_string()));
}

#[test]
fn test_update_shot_rejects_non_finite_nested_field_atomically() {
    // ShotUpdates::all_finite must reject a non-finite value hidden in a nested
    // `Option<Option<f64>>` field (pitch_in), not just the flat `length` case
    // covered elsewhere. The whole update is rejected atomically: a valid field
    // set alongside the poisoned one (azimuth_in) must NOT be partially applied.
    let state = state_with_project("T");
    let create = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 10.0,
            length: 5.0,
            ..Default::default()
        }],
    });
    let section_id = create.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            azimuth_in: Some(45.0),         // valid -- must NOT be applied
            pitch_in: Some(Some(f64::NAN)), // poisoned nested field
            ..Default::default()
        },
    });
    assert!(
        !result.success,
        "a non-finite nested field must be rejected"
    );
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("non-finite"));

    let after = state.get_project_state().unwrap();
    let shot = &after.sections[0].shots[0];
    assert_eq!(
        shot.azimuth_in, 10.0,
        "no partial apply: azimuth_in unchanged"
    );
    assert_eq!(shot.pitch_in, None, "poisoned pitch_in must not be written");
}

#[test]
fn test_update_shot_rejects_positive_infinity() {
    // Only NaN was covered before; +inf must be rejected too (`all_finite`
    // checks `is_finite`, which is false for both).
    let state = state_with_project("T");
    let create = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
    });
    let section_id = create.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            left: Some(Some(f64::INFINITY)),
            ..Default::default()
        },
    });
    assert!(!result.success, "+inf must be rejected");
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("non-finite"));
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots[0].left,
        None,
        "poisoned left must not be written"
    );
}

#[test]
fn test_import_shot_updates_reject_atomically_when_one_is_poisoned() {
    // Batch atomicity: two existing-shot updates where the SECOND carries a
    // non-finite value (+inf). The import validates every update before any
    // mutation, so the whole batch is rejected -- NEITHER shot may change, not
    // even the first (valid) one.
    use crate::models::ExistingShotUpdate;
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = state_with_project("T");

    let seed = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(5.0), shot(6.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
    };
    let r = state.execute_import_sections(
        vec![seed],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "seed import failed: {:?}", r.error);
    let project = state.get_project_state().unwrap();
    let section_id = project.sections[0].id;
    let shot0 = project.sections[0].shots[0].id;
    let shot1 = project.sections[0].shots[1].id;
    let len0 = project.sections[0].shots[0].length;
    let len1 = project.sections[0].shots[1].length;

    let incoming = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Incoming".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(3.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
    };
    let r = state.execute_import_sections(
        vec![incoming],
        Vec::new(),
        None,
        Vec::new(),
        vec![
            ExistingShotUpdate {
                section_id,
                shot_id: shot0,
                updates: ShotUpdates {
                    length: Some(9.0), // valid
                    ..Default::default()
                },
            },
            ExistingShotUpdate {
                section_id,
                shot_id: shot1,
                updates: ShotUpdates {
                    length: Some(f64::INFINITY), // poisoned
                    ..Default::default()
                },
            },
        ],
        VirtualShotMode::Auto,
    );
    assert!(
        !r.success,
        "a batch with one poisoned update must be rejected"
    );

    let after = state.get_project_state().unwrap();
    assert_eq!(
        after.sections[0].shots[0].length, len0,
        "the first (valid) update must NOT apply when the batch is rejected"
    );
    assert_eq!(
        after.sections[0].shots[1].length, len1,
        "the second (poisoned) update must NOT apply"
    );
}

#[test]
fn doubly_anchored_guard_reports_receipt_warning_and_skips_the_shot() {
    // F12b: a section with zero datums on BOTH sides of a modified shot range
    // is exactly the case the doubly-anchored guard exists for - the update
    // must be silently kept (not applied) AND the receipt must say so, not
    // just log it. The two zero datums are pushed directly onto the project
    // (bypassing the command layer's "one zero datum per connected group"
    // guard, which exists to stop a USER from creating this state through the
    // UI) because the guard under test only cares about the raw
    // `zero_datums` data, and this is the cheapest fixture that produces it.
    use crate::models::ZeroDatum;

    let state = state_with_project("T");

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(5.0), shot(5.0), shot(5.0)],
        })
        .section_id
        .unwrap();

    let (mid_shot_id, orig_length) = {
        let mut guard = state.project.lock();
        let project = guard.as_mut().unwrap();
        let key_front = project.key_for_ref(StationRef::new(a, 0)).unwrap();
        let key_back = project.key_for_ref(StationRef::new(a, 4)).unwrap();
        let gps = GpsCoordinates::new(10.0, 20.0, 0.0).unwrap();
        project.zero_datums.push(ZeroDatum {
            station_ref: key_front,
            gps: gps.clone(),
        });
        project.zero_datums.push(ZeroDatum {
            station_ref: key_back,
            gps,
        });
        let shot = &project.sections[0].shots[1];
        (shot.id, shot.length)
    };

    // A trivial carrier section: `execute_import_sections_inner` discards
    // `existing_shot_updates` outright when `sections` is empty (the DMP
    // wizard always sends updates alongside the sections they belong to).
    let carrier = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Carrier".to_string()),
        surveyor: None,
        date: None,
        color: "#222222".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(1.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
    };

    let r = state.execute_import_sections(
        vec![carrier],
        Vec::new(),
        None,
        Vec::new(),
        vec![crate::models::ExistingShotUpdate {
            section_id: a,
            shot_id: mid_shot_id,
            updates: ShotUpdates {
                length: Some(orig_length + 50.0),
                ..Default::default()
            },
        }],
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);

    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert!(
        receipt
            .warnings
            .iter()
            .any(|w| w.contains("anchored on both sides")),
        "receipt warnings must name the doubly-anchored skip, got {:?}",
        receipt.warnings
    );

    let after = state.get_project_state().unwrap();
    let after_shot = after.sections[0]
        .shots
        .iter()
        .find(|s| s.id == mid_shot_id)
        .unwrap();
    assert_eq!(
        after_shot.length, orig_length,
        "the doubly-anchored shot must keep its original length, not the update"
    );
}

#[test]
fn zero_datum_import_placement_sets_receipt_zero_datum_created() {
    // F12c: a section placed via `ImportPlacement::ZeroDatum` must name itself
    // on the receipt, not just in the trace logs.
    let state = state_with_project("T");

    let config = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("GPS Anchor".to_string()),
        surveyor: None,
        date: None,
        color: "#333333".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(5.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::ZeroDatum {
            coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
            station_index: 0,
        },
        azimuth_reference: None,
    };

    let r = state.execute_import_sections(
        vec![config],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);

    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert_eq!(
        receipt.zero_datum_created.as_deref(),
        Some("GPS Anchor"),
        "receipt must name the section a zero datum was created on"
    );
}

#[test]
fn multiple_zero_datum_placements_name_the_first_and_count_the_rest() {
    // F12c/item-9: a batch that creates more than one zero datum must not
    // let naming only the first one read as a claim of completeness.
    let state = state_with_project("T");

    let make = |name: &str, station_index: u32| ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#333333".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(5.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::ZeroDatum {
            coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
            station_index,
        },
        azimuth_reference: None,
    };

    let r = state.execute_import_sections(
        vec![make("First Anchor", 0), make("Second Anchor", 0)],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);

    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert_eq!(
        receipt.zero_datum_created.as_deref(),
        Some("First Anchor (and 1 more)"),
        "must name the first section and count the rest, not claim completeness"
    );
}

#[test]
fn import_with_no_zero_datum_placement_leaves_receipt_field_none() {
    // Negative case for F12c: an ordinary Origin-placed import must not claim
    // a zero datum was created.
    let state = state_with_project("T");

    let config = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Plain".to_string()),
        surveyor: None,
        date: None,
        color: "#444444".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(5.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
    };

    let r = state.execute_import_sections(
        vec![config],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);

    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert_eq!(receipt.zero_datum_created, None);
}

#[test]
fn test_execute_delete_shot_command() {
    // Verify DeleteShot removes shot and maintains section integrity
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(9.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Get the middle shot's ID
    let project = state.get_project_state().unwrap();
    let middle_shot_id = project.sections[0].shots[1].id;
    let first_shot_id = project.sections[0].shots[0].id;
    let last_shot_id = project.sections[0].shots[2].id;

    // Delete the middle shot
    let result = state.execute(Command::DeleteShot {
        section_id,
        shot_id: middle_shot_id,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let section = project.get_section(section_id).unwrap();

    // Should have 2 shots remaining
    assert_eq!(section.shots.len(), 2);

    // First and last shots should still exist
    assert!(section.shots.iter().any(|s| s.id == first_shot_id));
    assert!(section.shots.iter().any(|s| s.id == last_shot_id));

    // Middle shot should be gone
    assert!(!section.shots.iter().any(|s| s.id == middle_shot_id));

    // Verify shot lengths are correct (first and last)
    assert_eq!(section.shots[0].length, 5.0);
    assert_eq!(section.shots[1].length, 9.0);
}

#[test]
fn test_execute_create_shot_command() {
    // Verify CreateShot inserts shot at correct position
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(10.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Get the first shot's ID
    let project = state.get_project_state().unwrap();
    let first_shot_id = project.sections[0].shots[0].id;

    // Insert a new shot after the first shot
    let result = state.execute(Command::CreateShot {
        section_id,
        shot: shot(7.5), // Intermediate length
        after_shot_id: Some(first_shot_id),
        before_shot_id: None,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let section = project.get_section(section_id).unwrap();

    // Should have 3 shots now
    assert_eq!(section.shots.len(), 3);

    // Verify order by length: 5.0, 7.5, 10.0
    assert_eq!(section.shots[0].length, 5.0);
    assert_eq!(section.shots[1].length, 7.5);
    assert_eq!(section.shots[2].length, 10.0);
}

#[test]
fn test_execute_delete_section_command() {
    // Verify DeleteSection cleans up merges and related data
    let state = state_with_project("Test");

    // Create two sections
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_a_id = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(7.0)],
    });
    let section_b_id = result_b.section_id.unwrap();

    // Set a zero datum on section A
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(section_a_id, 0),
        coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });

    // Verify initial state
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert_eq!(project.zero_datums.len(), 1);

    // Delete section A
    let result = state.execute(Command::DeleteSection {
        section_id: section_a_id,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();

    // Section should be removed
    assert_eq!(project.sections.len(), 1);
    assert!(project.get_section(section_a_id).is_none());
    assert!(project.get_section(section_b_id).is_some());

    // Section B should be the only one remaining
    assert_eq!(project.sections[0].id, section_b_id);
    assert_eq!(project.sections[0].name, Some("Section B".to_string()));
}

#[test]
fn test_execute_update_section_metadata_command() {
    // Verify UpdateSectionMetadata modifies metadata correctly
    use chrono::NaiveDate;
    let state = state_with_project("Test");

    let initial_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
    let create_result = state.execute(Command::CreateSection {
        name: Some("Original Name".to_string()),
        surveyor: Some("Bob".to_string()),
        date: Some(initial_date),
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![shot(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Update all metadata fields
    let updated_date = NaiveDate::from_ymd_opt(2024, 2, 20).unwrap();
    let result = state.execute(Command::UpdateSectionMetadata {
        section_id,
        updates: SectionUpdates {
            name: Some(Some("Updated Name".to_string())),
            surveyor: Some(Some("Carol".to_string())),
            date: Some(Some(updated_date)),
            color: Some("#0000ff".to_string()),
            direction: Some(SurveyDirection::Out),
            quality: Some(1.25),
        },
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let section = project.get_section(section_id).unwrap();

    assert_eq!(section.name, Some("Updated Name".to_string()));
    assert_eq!(section.surveyor, Some("Carol".to_string()));
    assert_eq!(section.date, Some(updated_date));
    assert_eq!(section.color, "#0000ff");
    assert_eq!(section.direction, SurveyDirection::Out);
    assert_eq!(section.quality, 1.25);

    // Verify shots were not affected
    assert_eq!(section.shots.len(), 1);
    assert_eq!(section.shots[0].length, 5.0);
}

// === Edge Case Tests for Validation Errors ===

#[test]
fn test_execute_command_no_project_open() {
    let state = AppState::new().unwrap();

    // Try to execute commands without an open project
    let commands_to_test = vec![
        Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        },
        Command::DeleteSection { section_id: 0 },
        Command::UpdateSectionMetadata {
            section_id: 0,
            updates: SectionUpdates::default(),
        },
        Command::CreateShot {
            section_id: 0,
            shot: shot(5.0),
            after_shot_id: None,
            before_shot_id: None,
        },
        Command::UpdateShot {
            section_id: 0,
            shot_id: 0,
            updates: ShotUpdates::default(),
        },
        Command::DeleteShot {
            section_id: 0,
            shot_id: 0,
        },
        Command::MergeStations {
            station_a: StationRef::new(0, 0),
            station_b: StationRef::new(1, 0),
        },
        Command::SetZeroDatum {
            station_ref: StationRef::new(0, 0),
            coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
        },
    ];

    for cmd in commands_to_test {
        let result = state.execute(cmd);
        assert!(
            !result.success,
            "Command should fail when no project is open"
        );
        assert!(
            result.error.is_some(),
            "Should have error message when no project open"
        );
    }
}

#[test]
fn test_execute_commands_with_invalid_ids() {
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

    // Test with nonexistent section ID
    let result = state.execute(Command::DeleteSection { section_id: 999 });
    assert!(!result.success);

    let result = state.execute(Command::UpdateSectionMetadata {
        section_id: 999,
        updates: SectionUpdates::default(),
    });
    assert!(!result.success);

    // Test with nonexistent shot ID
    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id: 999_999,
        updates: ShotUpdates::default(),
    });
    assert!(!result.success);

    let result = state.execute(Command::DeleteShot {
        section_id,
        shot_id: 999_999,
    });
    assert!(!result.success);
}
