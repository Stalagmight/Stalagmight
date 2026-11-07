//! Zero-datum validation errors and the connected-datum lookup.
//!
//! Lines 2351-2541 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn set_zero_datum_error_includes_section_name() {
    // Error message should include section name if available
    let mut project = Project::new("NamedSection".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            name: Some("Main Passage".to_string()),
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Attempt duplicate
    let result = project.set_zero_datum(StationRef::new(section_id, 1), gps);
    let err = result.unwrap_err();

    // Error should include section name
    assert!(
        err.contains("Main Passage"),
        "Error should include section name: {err}"
    );
}

#[test]
fn set_zero_datum_error_includes_station_number() {
    // Error message should include station numbers (0-indexed to match canonical IDs)
    let mut project = Project::new("StationNumbers".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set first datum on station 0
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Attempt to set on station 2
    let result = project.set_zero_datum(StationRef::new(section_id, 2), gps);
    let err = result.unwrap_err();

    // Error should reference stations with 0-indexed format (e.g., "Section 0 #2")
    assert!(err.contains("#2"), "New station should include #2: {err}");
    assert!(
        err.contains("#0"),
        "Existing datum station should include #0: {err}"
    );
}

#[test]
fn set_zero_datum_validation_runs_before_state_changes() {
    // Validation should reject before any section offsets are modified
    let mut project = Project::new("NoStateChange".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Record state before failed attempt
    let offset_before = project.section_offsets.get(&section_id).copied();
    let datums_count_before = project.zero_datums.len();

    // Attempt duplicate (should fail)
    let result = project.set_zero_datum(StationRef::new(section_id, 1), gps);
    assert!(result.is_err());

    // State should be unchanged
    let offset_after = project.section_offsets.get(&section_id).copied();
    assert_eq!(
        offset_before, offset_after,
        "Section offset should not change on validation failure"
    );
    assert_eq!(
        datums_count_before,
        project.zero_datums.len(),
        "Zero datums count should not change"
    );
}

#[test]
fn set_zero_datum_allows_resetting_same_station() {
    // Re-setting datum on the same station (updating GPS) should succeed
    let mut project = Project::new("ResetSame".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum
    let gps1 = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps1)
        .unwrap();

    // Re-set on same station with different GPS - should succeed
    let gps2 = GpsCoordinates::new(46.0, -91.0, 0.0).unwrap();
    let result = project.set_zero_datum(StationRef::new(section_id, 0), gps2);
    assert!(result.is_ok(), "Resetting same station should succeed");

    // Should still have only one datum
    assert_eq!(project.zero_datums.len(), 1);
    assert_eq!(project.zero_datums[0].gps.latitude, 46.0);
}

#[test]
fn store_zero_datum_without_repositioning_validates_duplicate() {
    // The store-only helper should also validate for duplicates
    let mut project = Project::new("StoreOnlyValidation".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum normally
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Attempt store-only on different station - should fail
    let result =
        project.store_zero_datum_without_repositioning(StationRef::new(section_id, 1), gps);
    assert!(
        result.is_err(),
        "Store-only should also validate for duplicates"
    );
}

#[test]
fn find_connected_zero_datum_uses_station_connectivity() {
    // Validation should use station-level connectivity (through shots within section)
    // not just section-level connectivity
    let mut project = Project::new("StationConnectivity".to_string(), None);

    // Single section with multiple stations - all connected through shots
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set datum on station 1 (middle)
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 1), gps)
        .unwrap();

    // Station 0 is connected to station 1 through shot 0
    let result_0 = project.set_zero_datum(StationRef::new(section_id, 0), gps);
    assert!(
        result_0.is_err(),
        "Station 0 should be connected to datum at station 1"
    );

    // Station 3 is connected to station 1 through shots 1 and 2
    let result_3 = project.set_zero_datum(StationRef::new(section_id, 3), gps);
    assert!(
        result_3.is_err(),
        "Station 3 should be connected to datum at station 1"
    );
}
