//! Round-trip fidelity: a project written out (Ariane `.tmlu`, `.stalx`, CSV)
//! and read back keeps its shot data, sections, merges, closures and offsets.
//!
//! Moved out of `tests.rs` unchanged ("Group 1: Round-trip fidelity tests";
//! #576 E8: the monolith is frozen).

use std::collections::HashSet;

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{GpsCoordinates, ShotData, StationRef};
use crate::test_fixtures::{shot, state_with_project};

/// Export project to TML, re-import it, verify shot data matches exactly.
#[test]
fn test_tml_roundtrip_preserves_shot_data() {
    let state = state_with_project("TML Roundtrip");

    let shot = ShotData {
        azimuth_in: 12.5,
        azimuth_out: 192.3,
        length: 8.75,
        depth_in: 3.2,
        depth_out: 5.8,
        left: Some(1.1),
        right: Some(0.9),
        up: Some(2.3),
        down: Some(0.4),
        comment: Some("test note".to_string()),
        ..Default::default()
    };
    let shot2 = ShotData {
        azimuth_in: 280.0,
        azimuth_out: 100.0,
        length: 3.25,
        depth_in: 5.8,
        depth_out: 4.0,
        ..Default::default()
    };
    state.execute(Command::CreateSection {
        name: Some("Main Passage".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot.clone(), shot2.clone()],
    });

    let project = state.get_project_state().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let tml_path = temp_dir.path().join("roundtrip.tmlu");

    crate::export::export_tml(&project, &tml_path).unwrap();
    let parsed = crate::parsers::tml::parse_tml(&tml_path).unwrap();
    let (configs, _closures, _warnings) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, true, false).unwrap();

    assert!(!configs.is_empty(), "should have at least one section");
    let imported_shots = &configs[0].shots;
    assert_eq!(imported_shots.len(), 2, "should have 2 shots");

    let eps = 0.05; // TML format has limited precision
                    // Ariane carries ONE azimuth per shot, and the export writes the bearing
                    // the app draws - the circular mean of the two forward readings (#576
                    // NF-AN5-1) - so that is what has to come back, on both readings.
    let drawn =
        |data: &ShotData| crate::models::average_azimuth_of(data.azimuth_in, data.azimuth_out);
    let s = &imported_shots[0];
    assert!(
        (s.length - 8.75).abs() < eps,
        "length: {} vs 8.75",
        s.length
    );
    assert!(
        (s.azimuth_in - drawn(&shot)).abs() < eps && (s.azimuth_out - drawn(&shot)).abs() < eps,
        "azimuth: {}/{} vs the drawn {}",
        s.azimuth_in,
        s.azimuth_out,
        drawn(&shot)
    );
    assert!(
        (s.depth_in - 3.2).abs() < eps,
        "depth_in: {} vs 3.2",
        s.depth_in
    );
    assert!(
        (s.depth_out - 5.8).abs() < eps,
        "depth_out: {} vs 5.8",
        s.depth_out
    );

    let s2 = &imported_shots[1];
    assert!(
        (s2.length - 3.25).abs() < eps,
        "shot2 length: {} vs 3.25",
        s2.length
    );
    assert!(
        (s2.azimuth_in - drawn(&shot2)).abs() < eps,
        "shot2 azimuth_in: {} vs the drawn {}",
        s2.azimuth_in,
        drawn(&shot2)
    );
}

/// Export project to TML with multiple sections, re-import, verify section count and names.
#[test]
fn test_tml_roundtrip_preserves_multiple_sections() {
    let state = state_with_project("Multi Section");
    state.execute(Command::CreateSection {
        name: Some("Gallery A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(3.0)],
    });
    state.execute(Command::CreateSection {
        name: Some("Gallery B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(7.0)],
    });

    let project = state.get_project_state().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let tml_path = temp_dir.path().join("multi.tmlu");

    crate::export::export_tml(&project, &tml_path).unwrap();
    let parsed = crate::parsers::tml::parse_tml(&tml_path).unwrap();
    let (configs, _, _) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, true, false).unwrap();

    assert_eq!(configs.len(), 2, "should have 2 sections");
    // Total shots: 2 + 1 = 3
    let total_shots: usize = configs.iter().map(|c| c.shots.len()).sum();
    assert_eq!(total_shots, 3, "total shots across sections");
}

/// Stalx save/open round-trip preserves merges, closures, and offsets.
#[test]
fn test_stalx_roundtrip_preserves_merges_closures_offsets() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("full_roundtrip.stalx");

    let state = state_with_project("Full Roundtrip");

    // Create two sections with 2 shots each
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(3.0)],
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
            shots: vec![shot(4.0), shot(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:2 with B:0
    let merge_r = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 2),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_r.success, "merge should succeed");

    // Set a zero datum on A:0
    let datum_r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 0),
        coordinates: GpsCoordinates::new(45.5, -73.5, 100.0).unwrap(),
    });
    assert!(datum_r.success, "set zero datum should succeed");

    // Set a station comment
    state.execute(Command::SetStationComment {
        station_ref: StationRef::new(sec_b, 1),
        comment: Some("junction marker".to_string()),
    });

    // Snapshot state before save
    let project_before = state.get_project_state().unwrap();
    assert_eq!(project_before.station_merges.len(), 1);
    assert_eq!(project_before.zero_datums.len(), 1);

    // Save and reopen
    assert!(state.save_project(Some(save_path.clone())).success);
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(save_path).success);
    let project_after = state2.get_project_state().unwrap();

    // Verify merges
    assert_eq!(
        project_after.station_merges.len(),
        1,
        "merge should survive save/open"
    );
    let m = &project_after.station_merges[0];
    let merge_stations: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(merge_stations.contains(&StationRef::new(sec_a, 2)));
    assert!(merge_stations.contains(&StationRef::new(sec_b, 0)));

    // Verify zero datums
    assert_eq!(
        project_after.zero_datums.len(),
        1,
        "datum should survive save/open"
    );
    assert!(project_after.key_is_ref(
        &project_after.zero_datums[0].station_ref,
        StationRef::new(sec_a, 0)
    ));
    assert!((project_after.zero_datums[0].gps.latitude - 45.5).abs() < 1e-6);

    // Verify station comments
    assert_eq!(
        project_after
            .station_comment_for_ref(StationRef::new(sec_b, 1))
            .map(String::as_str),
        Some("junction marker"),
        "comment should survive save/open"
    );

    // Verify section offsets exist for merged sections
    assert!(
        project_after.section_offsets().contains_key(&sec_a)
            || project_after.section_offsets().contains_key(&sec_b),
        "at least one section should have an offset after merge"
    );

    // Verify section data integrity
    assert_eq!(project_after.sections.len(), 2);
    let sec_a_after = project_after
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap();
    let sec_b_after = project_after
        .sections
        .iter()
        .find(|s| s.id == sec_b)
        .unwrap();
    assert_eq!(sec_a_after.shots.len(), 2);
    assert_eq!(sec_b_after.shots.len(), 2);
}

/// CSV export preserves precision for azimuths near 360/0 boundary.
#[test]
fn test_csv_export_preserves_azimuth_precision() {
    let state = state_with_project("CSV Precision");

    let shot = ShotData {
        azimuth_in: 359.9,
        azimuth_out: 0.1,
        length: 4.567,
        depth_in: 12.345,
        depth_out: 12.678,
        left: Some(0.01),
        right: Some(99.99),
        ..Default::default()
    };
    state.execute(Command::CreateSection {
        name: Some("Boundary".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot],
    });

    let project = state.get_project_state().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let csv_path = temp_dir.path().join("precision.csv");
    crate::export::export_csv(&project, &csv_path).unwrap();

    let contents = std::fs::read_to_string(&csv_path).unwrap();
    let data_line = contents.lines().nth(1).unwrap();
    // Verify azimuths aren't truncated: 359.9 should not become 359 or 360
    assert!(
        data_line.contains("359.9"),
        "azimuth_in 359.9 should be preserved, got: {}",
        data_line
    );
    assert!(
        data_line.contains("0.1"),
        "azimuth_out 0.1 should be preserved, got: {}",
        data_line
    );
    // Verify length precision: 4.567 should appear as 4.57 (2 decimal CSV format)
    assert!(
        data_line.contains("4.57"),
        "length should be 4.57, got: {}",
        data_line
    );
}
