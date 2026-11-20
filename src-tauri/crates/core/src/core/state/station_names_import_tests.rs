//! `ImportLineConfig::station_names` at the state layer: the names land on
//! the stations they were given for, mirror with a reversed import exactly like
//! station comments, and undo/redo of the import takes them away and back.
//! On the station graph a name is the station's own (`Station::name`).

use super::{AppState, VirtualShotMode};
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{ImportLineConfig, ImportPlacement, ShotData};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn config(reverse: bool, names: &[(u32, &str)], comments: &[(u32, &str)]) -> ImportLineConfig {
    let pairs = |entries: &[(u32, &str)]| {
        entries
            .iter()
            .map(|(index, text)| (*index, (*text).to_string()))
            .collect()
    };
    ImportLineConfig {
        cave_section: None,
        line_index: 0,
        name: Some("A01".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(0.0, 10.0), shot(90.0, 5.0), shot(180.0, 7.5)],
        placement: ImportPlacement::Origin,
        station_comments: pairs(comments),
        station_names: pairs(names),
        reverse,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

fn import(config: ImportLineConfig) -> AppState {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Names".to_string(),
        location: None,
    });
    let result = state.execute_import_lines(
        vec![config],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    state
}

/// The name of each station of the (single) imported line, in station order.
fn names(project: &Project) -> Vec<String> {
    let line = project.lines().first().unwrap();
    line.stations()
        .map(|s| project.station(s).unwrap().name.clone())
        .collect()
}

fn named(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

#[test]
fn imported_names_land_on_their_stations_and_undo_redo_with_the_import() {
    let state = import(config(
        false,
        &[(0, "0"), (1, "1"), (2, "2"), (3, "3")],
        &[(2, "restriction")],
    ));
    let project = state.get_project_state().unwrap();
    assert_eq!(names(&project), named(&["0", "1", "2", "3"]));
    let line = project.lines().first().unwrap();
    let middle = line.station(2).unwrap();
    assert_eq!(
        project.station(middle).unwrap().comment.as_deref(),
        Some("restriction"),
        "a name and a comment on one station are independent"
    );

    // And the wire station carries both.
    let wire = state.project_state_wire().unwrap();
    let slot = wire
        .stations
        .iter()
        .find(|s| s.station_ref == crate::models::StationRef::new(line.id, 2))
        .unwrap();
    assert_eq!(slot.name, "2");
    assert_eq!(slot.comment.as_deref(), Some("restriction"));

    assert!(state.undo().success);
    let undone = state.get_project_state().unwrap();
    assert!(undone.lines().is_empty());
    assert!(undone.stations.is_empty(), "undo takes the stations away");

    assert!(state.redo().success);
    assert_eq!(
        names(&state.get_project_state().unwrap()),
        named(&["0", "1", "2", "3"]),
        "redo brings them back on the same stations"
    );
}

/// Indexes are given in the FILE's order; the backend mirrors them for a
/// reversed import, so the file's station 0 - MNemo "0" - ends up on the
/// station now numbered 3, still the same physical point. The station the
/// file does not name sits between "3" and "1", so it takes the Mid rule.
#[test]
fn a_reversed_import_mirrors_names_onto_the_same_physical_stations() {
    let state = import(config(true, &[(0, "0"), (1, "1"), (3, "3")], &[]));
    assert_eq!(
        names(&state.get_project_state().unwrap()),
        named(&["3", "3a", "1", "0"])
    );
}

#[test]
fn a_name_past_the_last_station_is_dropped_either_way() {
    for reverse in [false, true] {
        let state = import(config(reverse, &[(7, "nowhere"), (1, "1")], &[]));
        let project = state.get_project_state().unwrap();
        // The one name that lands, and the rest filled by the creation rules.
        let got = names(&project);
        assert_eq!(got.len(), 4, "reverse={reverse}");
        assert!(!got.iter().any(|n| n == "nowhere"), "reverse={reverse}");
        assert!(got.iter().any(|n| n == "1"), "reverse={reverse}");
    }
}

/// A comment the file gives the station a line is placed on is joined to the
/// junction's own (the merge rule), never silently replacing it; the
/// junction keeps its name and its id.
#[test]
fn a_placement_onto_a_station_joins_the_files_comment_and_keeps_the_name() {
    let state = import(config(false, &[], &[(1, "first visit")]));
    let project = state.get_project_state().unwrap();
    let main = project.lines()[0].id;
    let junction = project.lines()[0].station(1).unwrap();
    let mut branch = config(false, &[(0, "Z9")], &[(0, "second visit")]);
    branch.name = Some("B01".to_string());
    branch.placement = ImportPlacement::FromExisting {
        import_station: 0,
        existing_station: crate::models::StationRef::new(main, 1),
    };
    let result = state.execute_import_lines(
        vec![branch],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    let station = project.station(junction).unwrap();
    assert_eq!(station.name, "1", "the junction keeps its name");
    assert_eq!(
        station.comment.as_deref(),
        Some("first visit\n\nsecond visit")
    );
    let branch = &project.lines()[1];
    assert_eq!(branch.start, junction, "the branch starts ON the station");
    let branch_names: Vec<String> = branch
        .stations()
        .map(|s| project.station(s).unwrap().name.clone())
        .collect();
    // The file's name for the junction is not the junction's; the branch's own
    // stations take the seed rule around it.
    assert_eq!(branch_names, named(&["1", "1a", "2", "3"]));
}
