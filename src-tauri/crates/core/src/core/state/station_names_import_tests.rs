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
        station_external: Vec::new(),
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
/// junction keeps its name and its id, and the file's own name for it is kept
/// as an alias on the branch under a retired id (I-import-export-4).
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
        .map(|s| project.shown_name(s).unwrap().to_string())
        .collect();
    // The file's name for the junction is not the junction's: it is
    // archived (a record naming the branch, RS 4.1), the branch shows the
    // kept junction "1", and the branch's own "1" takes the letter. RS-X:
    // the record carries the file's comment, and comments are not joined.
    assert_eq!(branch_names, named(&["1", "1a", "2", "3"]));
    assert_eq!(project.shown_name(junction), Some("1"));
    let archived = &project.station(junction).unwrap().archived;
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].name, "Z9", "the file's name");
    assert_eq!(archived[0].lines, vec![branch.id], "the branch's own");
    assert!(
        project.station(archived[0].id).is_none()
            && archived[0].id < project.counters.next_station_id,
        "a retired id, never a live one"
    );
    assert!(project.validate().is_empty());
    // "1a" is a seed, not a name the file gave: nothing to report.
    assert_eq!(
        result.import_receipt.unwrap().warnings,
        Vec::<String>::new()
    );
}

// ----- external ids (plan 10.5) ---------------------------------------------------

/// The external id of each station of the (single) imported line.
fn externals(project: &Project) -> Vec<Option<String>> {
    let line = project.lines().first().unwrap();
    line.stations()
        .map(|s| project.station(s).unwrap().external.clone())
        .collect()
}

#[test]
fn imported_external_ids_land_on_their_stations_mirrored_and_undo_with_the_import() {
    let mut forward = config(false, &[], &[]);
    forward.station_external = vec![
        (0, "A1".to_string()),
        (1, "A2".to_string()),
        (3, "A4".to_string()),
    ];
    let state = import(forward);
    let some = |v: &str| Some(v.to_string());
    assert_eq!(
        externals(&state.get_project_state().unwrap()),
        vec![some("A1"), some("A2"), None, some("A4")]
    );
    assert!(state.undo().success);
    assert!(state.get_project_state().unwrap().stations.is_empty());
    assert!(state.redo().success);
    assert_eq!(
        externals(&state.get_project_state().unwrap()),
        vec![some("A1"), some("A2"), None, some("A4")],
        "redo brings them back"
    );

    // Indexes are the file's: a reversed import puts them on the same points.
    let mut reversed = config(true, &[], &[]);
    reversed.station_external = vec![(0, "A1".to_string()), (3, "A4".to_string())];
    let state = import(reversed);
    assert_eq!(
        externals(&state.get_project_state().unwrap()),
        vec![some("A4"), None, None, some("A1")]
    );
}

/// A line placed onto a station of an earlier line of the batch: the
/// junction keeps the id its own line gave it.
#[test]
fn a_joined_station_keeps_the_external_id_it_already_has() {
    let mut first = config(false, &[], &[]);
    first.station_external = vec![(0, "A1".to_string()), (3, "A4".to_string())];
    let mut second = config(false, &[], &[]);
    second.line_index = 1;
    second.name = Some("B01".to_string());
    second.placement = ImportPlacement::FromImporting {
        import_station: 0,
        target_line_index: 0,
        target_station_index: 3,
    };
    second.station_external = vec![(0, "B1".to_string()), (1, "B2".to_string())];
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Externals".to_string(),
        location: None,
    });
    let result = state.execute_import_lines(
        vec![first, second],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    let b = &project.lines()[1];
    let ext = |s| project.station(s).unwrap().external.clone();
    assert_eq!(
        ext(b.start),
        Some("A4".to_string()),
        "the junction keeps A4"
    );
    assert_eq!(ext(b.station(1).unwrap()), Some("B2".to_string()));
}

// ----- a file's own names at a junction (I-import-export-4) -------------------

/// A main line of 14 shots (stations named "0" to "14" by the seed rule),
/// and a DMP-style branch whose file names its stations "0" to "14" (MNemo
/// numbers), placed with its station 0 on main's station "12".
fn main_and_branch_onto_12() -> (AppState, crate::core::CommandResult) {
    let mut main = config(false, &[], &[]);
    main.shots = (0..14).map(|i| shot(f64::from(i * 7 % 360), 3.0)).collect();
    let state = import(main);
    let main_id = state.get_project_state().unwrap().lines()[0].id;
    let mut branch = config(false, &[], &[]);
    branch.name = Some("B01".to_string());
    branch.shots = (0..14)
        .map(|i| shot(f64::from(i * 11 % 360), 2.0))
        .collect();
    branch.station_names = (0..=14u32).map(|i| (i, i.to_string())).collect();
    branch.placement = ImportPlacement::FromExisting {
        import_station: 0,
        existing_station: crate::models::StationRef::new(main_id, 12),
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
    (state, result)
}

fn line_names(project: &Project, position: usize) -> Vec<String> {
    project.lines()[position]
        .stations()
        .map(|s| project.station(s).unwrap().name.clone())
        .collect()
}

/// The file names the joined station "0": that station is archived in the
/// junction (a record naming the branch, under a retired id, RS 4.1), the
/// branch shows the kept junction "12", so its own station "12" takes the
/// letter and the receipt says so. Detaching the branch gives it back the
/// file's "0". Undo takes the record away with the import; a save and
/// reopen keeps it.
#[test]
fn a_join_archives_the_files_station_and_letters_a_clash() {
    let (state, result) = main_and_branch_onto_12();
    let project = state.get_project_state().unwrap();
    let junction = project.lines()[0].station(12).unwrap();
    let branch = project.lines()[1].id;
    assert_eq!(project.lines()[1].start, junction);
    let mut expected: Vec<String> = (0..=14).map(|i| i.to_string()).collect();
    expected[0] = "12".to_string();
    expected[12] = "12a".to_string();
    assert_eq!(line_names(&project, 1), expected, "the branch");
    assert_eq!(
        project.station(junction).unwrap().name,
        "12",
        "main keeps its name"
    );
    let aliases = project.station(junction).unwrap().archived.clone();
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].name, "0", "the file's name is kept");
    assert_eq!(aliases[0].lines, vec![branch], "on the branch");
    assert!(project.station(aliases[0].id).is_none());
    assert!(project.validate().is_empty());
    assert_eq!(
        result.import_receipt.unwrap().warnings,
        vec![
            "Line \"B01\" joins station \"12\", so its own station \"12\" was renamed \"12a\""
                .to_string()
        ]
    );
    let naming = project.detach_naming(junction, branch, None).unwrap();
    assert_eq!(
        (naming.id, naming.name.as_str()),
        (aliases[0].id, "0"),
        "a detach gives the branch the file's station back"
    );

    // Saved and reopened, the alias is in the file.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("join.stalx");
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path });
    assert!(opened.success, "{:?}", opened.error);
    let project = reopened.get_project_state().unwrap();
    assert_eq!(project.station(junction).unwrap().archived, aliases);

    // One undo takes the branch and the record away.
    assert!(state.undo().success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.lines().len(), 1);
    assert!(project.station(junction).unwrap().archived.is_empty());
    assert!(state.redo().success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.station(junction).unwrap().archived, aliases);
}

/// Where the branch keeps no alias (the file gives the joined station no
/// name of its own), the junction's name is the branch's too, so the
/// branch's own station "12" takes a letter and the receipt says so.
#[test]
fn a_join_without_a_file_name_for_the_junction_letters_and_reports_a_clash() {
    let mut main = config(false, &[], &[]);
    main.shots = (0..14).map(|i| shot(f64::from(i * 7 % 360), 3.0)).collect();
    let state = import(main);
    let main_id = state.get_project_state().unwrap().lines()[0].id;
    let mut branch = config(false, &[], &[]);
    branch.name = Some("B01".to_string());
    branch.shots = (0..14)
        .map(|i| shot(f64::from(i * 11 % 360), 2.0))
        .collect();
    branch.station_names = (1..=14u32).map(|i| (i, i.to_string())).collect();
    branch.placement = ImportPlacement::FromExisting {
        import_station: 0,
        existing_station: crate::models::StationRef::new(main_id, 12),
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
    let junction = project.lines()[0].station(12).unwrap();
    assert!(project.station(junction).unwrap().archived.is_empty());
    let mut expected: Vec<String> = (0..=14).map(|i| i.to_string()).collect();
    expected[0] = "12".to_string();
    expected[12] = "12a".to_string();
    assert_eq!(line_names(&project, 1), expected, "the branch");
    assert!(project.validate().is_empty());
    assert_eq!(
        result.import_receipt.unwrap().warnings,
        vec![
            "Line \"B01\" joins station \"12\", so its own station \"12\" was renamed \"12a\""
                .to_string()
        ]
    );
}

/// The same survey as 0.6.0 left it, built with the frozen 0.6.0 model: its
/// import created the branch with a name per slot and merged the branch's
/// slot 0 into main's slot 12, so the branch kept "0" and "12" and main kept
/// "12". The station graph has one name per station; opening that 0.6.0 file
/// (the migration) and importing (the funnel) both archive the file's "0" in
/// the junction (a record naming the branch), the branch shows the kept
/// "12", and its own "12" takes the letter: the two give the same names, and
/// neither renames main's "12".
#[test]
fn a_join_keeps_what_the_equivalent_0_6_0_survey_keeps() {
    use super::legacy_fixture_kit::{
        frozen, old, read_legacy_file, slot, write_legacy_file, Legacy,
    };

    let mut legacy = Legacy::new("Names".to_string(), None);
    let main_shots: Vec<ShotData> = (0..14).map(|i| shot(f64::from(i * 7 % 360), 3.0)).collect();
    let branch_shots: Vec<ShotData> = (0..14)
        .map(|i| shot(f64::from(i * 11 % 360), 2.0))
        .collect();
    let mut add = |shots: &[ShotData]| {
        legacy
            .add_line(old::LineData {
                shots: shots.iter().map(frozen).collect(),
                quality: 1.0,
                ..Default::default()
            })
            .id
    };
    let main = add(&main_shots);
    let branch = add(&branch_shots);
    for line in [main, branch] {
        for i in 0..=14u32 {
            legacy
                .set_station_name(slot(line, i), &i.to_string())
                .unwrap();
        }
    }
    legacy
        .merge_stations(slot(branch, 0), slot(main, 12))
        .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("legacy.stalx");
    write_legacy_file(&path, &legacy);

    // 0.6.0: a name per slot, nothing renamed by the merge.
    let read = read_legacy_file(&path);
    let per_slot: Vec<String> = read
        .line_station_names(branch)
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect();
    assert_eq!(
        per_slot,
        (0..=14).map(|i| i.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(read.station_name_at(slot(main, 12)), Some("12"));

    // Opened now: the migration keeps "0" as an alias of the junction.
    let opened = AppState::new().unwrap();
    let r = opened.execute(Command::OpenProject { path });
    assert!(r.success, "{:?}", r.error);
    let migrated = opened.get_project_state().unwrap();
    let junction = migrated.lines()[0].station(12).unwrap();
    let alias_names = |p: &Project, s| -> Vec<String> {
        p.station(s)
            .unwrap()
            .archived
            .iter()
            .map(|r| r.name.clone())
            .collect()
    };
    assert_eq!(alias_names(&migrated, junction), named(&["0"]));
    assert_eq!(
        migrated.station(junction).unwrap().archived[0].lines,
        vec![migrated.lines()[1].id]
    );
    let migrated_branch = line_names(&migrated, 1);

    // Imported now: the same alias, unique names, and main's "12" kept.
    let (state, _) = main_and_branch_onto_12();
    let imported = state.get_project_state().unwrap();
    let junction = imported.lines()[0].station(12).unwrap();
    assert_eq!(alias_names(&imported, junction), named(&["0"]));
    assert_eq!(imported.station(junction).unwrap().name, "12");
    assert_eq!(
        imported.station(junction).unwrap().archived[0].lines,
        vec![imported.lines()[1].id]
    );
    let imported_branch = line_names(&imported, 1);
    assert_eq!(imported_branch, migrated_branch);
    // Every station but the junction and the lettered one keeps the name
    // 0.6.0 showed for it.
    for (i, name) in imported_branch.iter().enumerate().skip(1) {
        let expected = if i == 12 { "12a".to_string() } else { i.to_string() };
        assert_eq!(*name, expected);
    }
}
