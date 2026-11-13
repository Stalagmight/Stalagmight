//! `ImportSectionConfig::station_names` at the state layer: the names land on
//! the stations they were given for, mirror with a reversed import exactly like
//! station comments, and undo/redo of the import takes them away and back.

use tempfile::TempDir;

use super::{AppState, VirtualShotMode};
use crate::core::Command;
use crate::models::{
    ImportPlacement, ImportSectionConfig, Project, SectionData, ShotData, StationRef,
};
use crate::persistence::Database;

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn config(reverse: bool, names: &[(u32, &str)], comments: &[(u32, &str)]) -> ImportSectionConfig {
    let pairs = |entries: &[(u32, &str)]| {
        entries
            .iter()
            .map(|(index, text)| (*index, (*text).to_string()))
            .collect()
    };
    ImportSectionConfig {
        section_index: 0,
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

fn import(config: ImportSectionConfig) -> AppState {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Names".to_string(),
        location: None,
    });
    let result = state.execute_import_sections(
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

/// The name of each station of the (single) imported section, in station order.
fn names(project: &Project) -> Vec<Option<String>> {
    let section = project.sections.first().unwrap();
    (0..=section.shots.len() as u32)
        .map(|i| {
            project
                .station_name_for_ref(StationRef::new(section.id, i))
                .cloned()
        })
        .collect()
}

fn named(values: &[Option<&str>]) -> Vec<Option<String>> {
    values.iter().map(|v| v.map(str::to_string)).collect()
}

#[test]
fn imported_names_land_on_their_stations_and_undo_redo_with_the_import() {
    let state = import(config(
        false,
        &[(0, "0"), (1, "1"), (2, "2"), (3, "3")],
        &[(2, "restriction")],
    ));
    let project = state.get_project_state().unwrap();
    assert_eq!(
        names(&project),
        named(&[Some("0"), Some("1"), Some("2"), Some("3")])
    );
    let section = project.sections.first().unwrap().id;
    assert_eq!(
        project
            .station_comment_for_ref(StationRef::new(section, 2))
            .map(String::as_str),
        Some("restriction"),
        "a name and a comment on one station are independent"
    );

    // And the computed station carries both.
    let stations = project.compute_station_coordinates();
    let middle = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(section, 2))
        .unwrap();
    assert_eq!(middle.name.as_deref(), Some("2"));
    assert_eq!(middle.comment.as_deref(), Some("restriction"));

    assert!(state.undo().success);
    let undone = state.get_project_state().unwrap();
    assert!(undone.sections.is_empty());
    assert!(undone.station_names.is_empty(), "undo takes the names away");

    assert!(state.redo().success);
    assert_eq!(
        names(&state.get_project_state().unwrap()),
        named(&[Some("0"), Some("1"), Some("2"), Some("3")]),
        "redo brings them back on the same stations"
    );
}

/// Indexes are given in the FILE's order; the backend mirrors them for a
/// reversed import, so the file's station 0 - MNemo "0" - ends up on the
/// station now numbered 3, still the same physical point.
#[test]
fn a_reversed_import_mirrors_names_onto_the_same_physical_stations() {
    let state = import(config(true, &[(0, "0"), (1, "1"), (3, "3")], &[]));
    assert_eq!(
        names(&state.get_project_state().unwrap()),
        named(&[Some("3"), None, Some("1"), Some("0")])
    );
}

#[test]
fn a_name_past_the_last_station_is_dropped_either_way() {
    for reverse in [false, true] {
        let state = import(config(reverse, &[(7, "nowhere"), (1, "1")], &[]));
        let project = state.get_project_state().unwrap();
        assert_eq!(project.station_names.len(), 1, "reverse={reverse}");
        assert!(!project.station_names.values().any(|n| n == "nowhere"));
    }
}

/// Merging projects remaps every section id; each source's names must land on
/// the same station of its section's NEW id, in the merged file.
#[test]
fn merged_projects_keep_each_station_name_on_its_station() {
    let dir = TempDir::new().unwrap();
    let mut sources = Vec::new();
    for (label, count) in [("A", 2u32), ("B", 3)] {
        let mut project = Project::new(label.to_string(), None);
        let id = project
            .add_section(SectionData {
                name: Some(format!("Section {label}")),
                color: "#4ecdc4".to_string(),
                shots: (0..count).map(|i| shot(f64::from(i) * 40.0, 5.0)).collect(),
                ..Default::default()
            })
            .id;
        project.ensure_shot_uuids();
        // Distinct per source, so a name landing in the wrong section shows.
        for index in 0..=count {
            let key = project.key_for_ref(StationRef::new(id, index)).unwrap();
            project.station_names.insert(key, format!("{label}{index}"));
        }
        let path = dir.path().join(format!("{label}.stalx"));
        Database::open(&path)
            .unwrap()
            .save_project(&project)
            .unwrap();
        sources.push(path);
    }

    let output = dir.path().join("merged.stalx");
    let state = AppState::new().unwrap();
    let result = state.execute(Command::MergeProjects {
        source_paths: sources,
        output_path: output.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "merge failed: {:?}", result.error);

    let merged = Database::open_existing(&output)
        .unwrap()
        .load_project()
        .unwrap();
    for (label, count) in [("A", 2u32), ("B", 3)] {
        let section = merged
            .sections
            .iter()
            .find(|s| s.name.as_deref() == Some(&format!("Section {label}")))
            .unwrap();
        for index in 0..=count {
            assert_eq!(
                merged
                    .station_name_for_ref(StationRef::new(section.id, index))
                    .map(String::as_str),
                Some(format!("{label}{index}").as_str()),
                "{label}:{index}"
            );
        }
    }
    assert_eq!(merged.station_names.len(), 3 + 4);
}
