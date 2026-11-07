//! Duplicate zero-datum validation.
//!
//! Lines 1386-1542 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// === Duplicate datum validation tests ===

#[test]
fn set_zero_datum_rejects_duplicate_in_same_section() {
    // Cannot set two datums on different stations in the same section
    let mut project = Project::new("DuplicateSameSection".to_string(), None);
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

    // Attempt to set second datum on station 1 (same section) - should fail
    let result = project.set_zero_datum(StationRef::new(section_id, 1), gps);
    assert!(
        result.is_err(),
        "Should reject duplicate datum in same section"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("already has a zero datum"),
        "Error should mention existing datum: {err}"
    );
}

#[test]
fn set_zero_datum_rejects_duplicate_in_connected_group() {
    // Cannot set datum on a station connected through merges to another datum station
    let mut project = Project::new("DuplicateConnected".to_string(), None);

    // Create two sections
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge B.0 to A.1
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Set first datum on section A
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(StationRef::new(a, 0), gps).unwrap();

    // Attempt to set second datum on section B (connected through merge) - should fail
    let result = project.set_zero_datum(StationRef::new(b, 1), gps);
    assert!(
        result.is_err(),
        "Should reject duplicate datum in connected group"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("already has a zero datum"),
        "Error should mention existing datum: {err}"
    );
}

#[test]
fn set_zero_datum_allows_disconnected_groups() {
    // Disconnected section groups can each have their own datum
    let mut project = Project::new("DisconnectedGroups".to_string(), None);

    // Create two unconnected sections
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on section A
    let gps_a = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(a, 0), gps_a)
        .unwrap();

    // Set datum on section B (disconnected) - should succeed
    let gps_b = GpsCoordinates::new(46.0, -91.0, 0.0).unwrap();
    let result = project.set_zero_datum(StationRef::new(b, 0), gps_b);
    assert!(
        result.is_ok(),
        "Disconnected groups should each allow their own datum"
    );
    assert_eq!(project.zero_datums.len(), 2);
}

#[test]
fn set_zero_datum_rejects_across_multi_hop_merge_chain() {
    // Mirrors the reported topology: a far station (like 37:7) is merge-connected to
    // the datum (like 0:0) through a CHAIN of merges, not a direct one. The guard
    // must walk the whole merge component, so a second datum on the far end is
    // rejected.
    let mut project = Project::new("Chain".to_string(), None);
    let mk = |project: &mut Project| {
        project
            .add_section(SectionData {
                shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id
    };
    let (a, b, c, d) = (
        mk(&mut project),
        mk(&mut project),
        mk(&mut project),
        mk(&mut project),
    );

    // Chain A - B - C - D, each merge joining one section's end to the next's start.
    for (x, y) in [(a, b), (b, c), (c, d)] {
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(x, 1).into(),
            station_b: StationRef::new(y, 0).into(),
            merged_at: Utc::now(),
        });
    }

    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(StationRef::new(a, 0), gps).unwrap();

    // D is three hops from A's datum but in the same merge component → reject.
    let result = project.set_zero_datum(StationRef::new(d, 1), gps);
    assert!(
        result.is_err(),
        "a second datum on the far end of a merge chain must be rejected"
    );
    assert!(result.unwrap_err().contains("already has a zero datum"));
    assert_eq!(project.zero_datums.len(), 1);
}
