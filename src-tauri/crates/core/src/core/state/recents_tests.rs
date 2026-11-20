//! Recents on the station-graph core (plan 6.1, 6.5, 10.4; F13): the
//! metadata comes from a file's header (version 2: `summary`, `project`,
//! `speleodb`; legacy: `migrate::legacy_header`), and renaming a project by
//! path rewrites a closed version 2 file with every other member verbatim,
//! upgrades a closed legacy file first, and renames the open project durably.
//!
//! All data is synthetic: projects built in memory and saved by this build.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point; the metadata is the \
              stored figures, compared bit for bit"
)]

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::survey::{LineMeta, Project, StartAt, ZeroDatum};
use crate::models::{GpsCoordinates, ShotData, ShotKind, ShotUpdates};
use crate::persistence::stalx_v2::{self, Extras, Format, Mode, SpeleoDbLink};

fn shot(length: f64, kind: ShotKind) -> ShotData {
    ShotData {
        length,
        azimuth_in: 10.0,
        azimuth_out: 10.0,
        depth_in: 1.0,
        depth_out: 1.0,
        kind,
        ..ShotData::default()
    }
}

/// Two lines meeting at a junction (5 stations, 4 shots, one of them a
/// Reference shot), tied to GPS at the first line's start.
fn survey(name: &str) -> Project {
    let mut p = Project::new(name, Some("Yucatan".to_string()));
    let main = p
        .add_line(
            LineMeta::default(),
            StartAt::New,
            vec![shot(10.0, ShotKind::Normal), shot(5.0, ShotKind::Normal)],
        )
        .unwrap();
    let junction = p.line(main).unwrap().station(1).unwrap();
    p.add_line(
        LineMeta::default(),
        StartAt::Station(junction),
        vec![shot(2.5, ShotKind::Jump), shot(7.0, ShotKind::Reference)],
    )
    .unwrap();
    let start = p.line(main).unwrap().start;
    p.push_datum(ZeroDatum {
        station: start,
        via: None,
        gps: GpsCoordinates::new(20.5, -87.25, 0.0).unwrap(),
        accuracy_m: None,
        entrance: None,
        correction: None,
    });
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    p
}

fn write_v2(path: &Path, project: &Project, extras: &Extras) {
    std::fs::write(
        path,
        stalx_v2::encode(project, extras, Mode::Normal).unwrap(),
    )
    .unwrap();
}

fn linked() -> Extras {
    Extras {
        speleodb: Some(SpeleoDbLink {
            endpoint: "https://sdb.example".to_string(),
            project_id: "11111111-2222-3333-4444-555555555555".to_string(),
            project_name: "Remote".to_string(),
            undo_mode: None,
        }),
        ..Extras::default()
    }
}

/// A legacy SQLite project with one 10 m line and a datum at its start.
fn legacy_file(path: &Path, name: &str) {
    use crate::models::{LineData, StationRef};
    let mut legacy = crate::models::Project::new(name.to_string(), None);
    let line = legacy
        .add_line(LineData {
            shots: vec![shot(10.0, ShotKind::Normal), shot(4.0, ShotKind::Reference)],
            ..Default::default()
        })
        .id;
    legacy
        .set_zero_datum(
            StationRef::new(line, 0),
            GpsCoordinates::new(-10.0, 30.0, 0.0).unwrap(),
        )
        .unwrap();
    let mut db = crate::persistence::Database::open(path).unwrap();
    db.save_project(&legacy).unwrap();
}

fn recents_of(state: &AppState, paths: &[&Path]) -> Vec<crate::models::ProjectMetadata> {
    for path in paths.iter().rev() {
        state.note_recent_project(path);
    }
    state.get_recent_projects()
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "{:?}", r.error);
    state
}

/// The survey members of a version 2 file, decoded, with the name blanked:
/// what a rename must leave as it was.
fn body_without_name(bytes: &[u8]) -> (Project, Extras) {
    let mut decoded = stalx_v2::decode(bytes, &Default::default()).unwrap();
    decoded.project.name = String::new();
    decoded.project.file_path = None;
    (decoded.project, decoded.extras)
}

#[test]
fn a_version_2_entry_is_read_from_its_header() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    let project = survey("Cave");
    write_v2(&path, &project, &linked());
    let state = AppState::new().unwrap();

    let recent = recents_of(&state, &[&path]);

    assert_eq!(recent.len(), 1);
    let m = &recent[0];
    assert_eq!(m.name, "Cave");
    assert_eq!(m.location.as_deref(), Some("Yucatan"));
    assert_eq!(m.file_path, path);
    assert_eq!(m.modified_at, project.modified_at);
    assert_eq!(m.line_count, 2);
    assert_eq!(m.station_count, 5, "a junction is one station");
    assert_eq!(
        m.total_length, 17.5,
        "Reference shots are not surveyed length"
    );
    assert_eq!(m.gps_center, Some((-87.25, 20.5)));
    let bounds = m.gps_bounds.as_ref().unwrap();
    assert_eq!(
        (bounds.west, bounds.south, bounds.east, bounds.north),
        (-87.25, 20.5, -87.25, 20.5)
    );
    assert!(m.is_remote);
    assert_eq!(
        m.speleodb_project_id.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
}

/// The listing never decodes the body: a file whose survey breaks an
/// invariant (the open refuses it) still lists from its header.
#[test]
fn the_listing_does_not_decode_the_body() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("damaged.stalx");
    let text = String::from_utf8(
        stalx_v2::encode(&survey("Damaged"), &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let at = text.find("\"datums\":[").unwrap();
    let end = at + text[at..].find("],\n").unwrap() + 3;
    std::fs::write(&path, format!("{}{}", &text[..at], &text[end..])).unwrap();
    assert!(stalx_v2::decode(&std::fs::read(&path).unwrap(), &Default::default()).is_err());
    let state = AppState::new().unwrap();

    let recent = recents_of(&state, &[&path]);

    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].name, "Damaged");
    assert_eq!(recent[0].line_count, 2);
}

/// A version 2 file without the `summary` cache (a third-party writer may
/// leave it out) is decoded to list it.
#[test]
fn a_version_2_file_without_a_summary_is_decoded() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bare.stalx");
    let text = String::from_utf8(
        stalx_v2::encode(&survey("Bare"), &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let at = text.find("\"summary\":{").unwrap();
    let end = at + text[at..].find("},\n").unwrap() + 3;
    std::fs::write(&path, format!("{}{}", &text[..at], &text[end..])).unwrap();
    assert!(stalx_v2::probe_path(&path).unwrap().summary.is_none());
    let state = AppState::new().unwrap();

    let recent = recents_of(&state, &[&path]);

    assert_eq!(recent[0].name, "Bare");
    assert_eq!(
        (
            recent[0].line_count,
            recent[0].station_count,
            recent[0].total_length
        ),
        (2, 5, 17.5)
    );
}

/// A legacy file lists through the legacy header: slots (0.6.0's count),
/// length without Reference shots, the datum's GPS; the file is only read.
#[test]
fn a_legacy_entry_is_read_through_the_legacy_header() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("legacy.stalx");
    legacy_file(&path, "Legacy");
    let before = std::fs::read(&path).unwrap();
    let state = AppState::new().unwrap();

    let recent = recents_of(&state, &[&path]);

    assert_eq!(recent.len(), 1);
    let m = &recent[0];
    assert_eq!(m.name, "Legacy");
    assert_eq!(m.line_count, 1);
    assert_eq!(m.station_count, 3, "slots: two shots plus the start");
    assert_eq!(m.total_length, 10.0);
    assert_eq!(m.gps_center, Some((30.0, -10.0)));
    assert!(!m.is_remote);
    assert_eq!(std::fs::read(&path).unwrap(), before, "only read");
}

/// A file that is neither format, or is gone, is skipped; the others list in
/// recents order.
#[test]
fn unreadable_entries_are_skipped() {
    let dir = TempDir::new().unwrap();
    let good = dir.path().join("good.stalx");
    write_v2(&good, &survey("Good"), &Extras::default());
    let junk = dir.path().join("junk.stalx");
    std::fs::write(&junk, b"not a project").unwrap();
    let gone = dir.path().join("gone.stalx");
    let legacy = dir.path().join("legacy.stalx");
    legacy_file(&legacy, "Legacy");
    let state = AppState::new().unwrap();

    let names: Vec<String> = recents_of(&state, &[&good, &junk, &gone, &legacy])
        .into_iter()
        .map(|m| m.name)
        .collect();

    assert_eq!(names, ["Good", "Legacy"]);
}

/// A closed version 2 file is renamed in the file: `project.name` and
/// `summary.name` change, every other member is as it was (the history
/// included), and the next listing shows the new name.
#[test]
fn renaming_a_closed_version_2_file_changes_only_its_name() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    write_v2(&path, &survey("Cave"), &linked());
    let before = std::fs::read(&path).unwrap();
    let state = AppState::new().unwrap();

    let r = state.set_project_name_for_path(path.clone(), "  Renamed Cave ".to_string());

    assert!(r.success, "{:?}", r.error);
    let after = std::fs::read(&path).unwrap();
    let header = stalx_v2::probe_bytes(&after);
    assert_eq!(header.project.unwrap().name, "Renamed Cave");
    assert_eq!(header.summary.unwrap().name, "Renamed Cave");
    assert_eq!(body_without_name(&after), body_without_name(&before));
    assert_eq!(recents_of(&state, &[&path])[0].name, "Renamed Cave");
}

/// A closed version 2 file a newer Stalagmight wrote (a higher major
/// version), or one that requires a feature this build does not know, is
/// refused untouched (#649).
#[test]
fn renaming_a_closed_file_this_build_cannot_write_changes_nothing() {
    let dir = TempDir::new().unwrap();
    let text = String::from_utf8(
        stalx_v2::encode(&survey("Cave"), &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let newer = text.replacen("\"version\":2,", "\"version\":3,", 1);
    let requires = text.replacen("\"requires\":[]", "\"requires\":[\"future\"]", 1);
    assert_ne!(requires, text, "the requires member is written");
    for (name, content) in [("newer.stalx", newer), ("requires.stalx", requires)] {
        let path = dir.path().join(name);
        std::fs::write(&path, &content).unwrap();
        let state = AppState::new().unwrap();

        let r = state.set_project_name_for_path(path.clone(), "Renamed".to_string());

        assert!(!r.success, "{name}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content, "{name}");
    }
}

#[test]
fn an_empty_name_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    write_v2(&path, &survey("Cave"), &Extras::default());
    let before = std::fs::read(&path).unwrap();

    let r = AppState::new()
        .unwrap()
        .set_project_name_for_path(path.clone(), "   ".to_string());

    assert_eq!(r.error.as_deref(), Some("Project name cannot be empty"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// A closed legacy file is upgraded first, as its first save would: the
/// SQLite original is kept in the app-data backups folder, and the file is
/// version 2 with the new name and the same survey the open would show.
#[test]
fn renaming_a_closed_legacy_file_upgrades_it_with_a_backup() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let stem = format!("rename-legacy-{}", uuid::Uuid::new_v4());
    let path = dir.path().join(format!("{stem}.stalx"));
    legacy_file(&path, "Legacy");
    let original = std::fs::read(&path).unwrap();
    let opened_before = open(&path).get_project_state().unwrap();
    let state = AppState::new().unwrap();

    let r = state.set_project_name_for_path(path.clone(), "Upgraded".to_string());

    assert!(r.success, "{:?}", r.error);
    let upgraded = std::fs::read(&path).unwrap();
    assert_eq!(stalx_v2::sniff(&upgraded), Format::V2);
    let decoded = stalx_v2::decode(&upgraded, &Default::default()).unwrap();
    assert_eq!(decoded.project.name, "Upgraded");
    let mut expected = opened_before;
    expected.name = "Upgraded".to_string();
    expected.file_path = None;
    let mut got = decoded.project;
    got.file_path = None;
    assert_eq!(got, expected, "the survey the open showed, renamed");
    let backups = super::project_io::upgrade_backup_dir().unwrap();
    let kept: Vec<PathBuf> = std::fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&stem))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(std::fs::read(&kept[0]).unwrap(), original);
    std::fs::remove_file(&kept[0]).unwrap();
}

/// A legacy file a newer schema wrote is not upgraded, and nothing is
/// written (#649).
#[test]
fn renaming_a_newer_legacy_file_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("newer.stalx");
    legacy_file(&path, "Newer");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes
        .get_mut(60..64)
        .unwrap()
        .copy_from_slice(&(crate::persistence::SCHEMA_VERSION + 1).to_be_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let r = AppState::new()
        .unwrap()
        .set_project_name_for_path(path.clone(), "Renamed".to_string());

    assert!(!r.success, "{r:?}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

/// The open version 2 project is renamed in memory and in its file at once,
/// without writing the session's other unsaved edits; the next save passes
/// the external-modification guard and writes everything.
#[test]
fn renaming_the_open_project_is_durable_without_saving_its_edits() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    write_v2(&path, &survey("Cave"), &Extras::default());
    let state = open(&path);
    let (line_id, shot_id) = {
        let p = state.get_project_state().unwrap();
        (p.lines()[0].id, p.lines()[0].shots[0].id)
    };
    let edit = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(11.0),
            ..ShotUpdates::default()
        },
    });
    assert!(edit.success, "{:?}", edit.error);

    let other_spelling = dir.path().join(".").join("cave.stalx");
    let r = state.set_project_name_for_path(other_spelling, "Renamed".to_string());

    assert!(r.success, "{:?}", r.error);
    assert_eq!(state.get_project_state().unwrap().name, "Renamed");
    let on_disk = stalx_v2::decode(&std::fs::read(&path).unwrap(), &Default::default()).unwrap();
    assert_eq!(on_disk.project.name, "Renamed");
    assert_eq!(
        on_disk.project.lines()[0].shots[0].length,
        10.0,
        "the unsaved edit is not written by the rename"
    );
    assert!(state.has_unsaved_changes(), "the edit is still unsaved");
    assert_eq!(
        state.get_undo_stack().len(),
        1,
        "a rename is not an undo step"
    );

    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "{:?}", saved.error);
    let on_disk = stalx_v2::decode(&std::fs::read(&path).unwrap(), &Default::default()).unwrap();
    assert_eq!(on_disk.project.name, "Renamed");
    assert_eq!(on_disk.project.lines()[0].shots[0].length, 11.0);
}

/// An open legacy project is upgraded by its first save, not by a rename: the
/// name changes in memory, the SQLite file is untouched, and the project is
/// marked unsaved so the name is not lost on close.
#[test]
fn renaming_an_open_legacy_project_waits_for_its_first_save() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("legacy.stalx");
    legacy_file(&path, "Legacy");
    let before = std::fs::read(&path).unwrap();
    let state = open(&path);
    assert!(!state.has_unsaved_changes());

    let r = state.set_project_name_for_path(path.clone(), "Renamed".to_string());

    assert!(r.success, "{:?}", r.error);
    assert_eq!(state.get_project_state().unwrap().name, "Renamed");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(state.has_unsaved_changes());
}

/// A rename of the open project whose file changed on disk since it was read
/// is refused with the guard's code, and changes nothing.
#[test]
fn renaming_the_open_project_respects_the_external_modification_guard() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    write_v2(&path, &survey("Cave"), &Extras::default());
    let state = open(&path);
    write_v2(&path, &survey("Changed elsewhere"), &Extras::default());
    let before = std::fs::read(&path).unwrap();

    let r = state.set_project_name_for_path(path.clone(), "Renamed".to_string());

    assert!(
        r.error
            .as_deref()
            .is_some_and(|e| e.contains(super::EXTERNALLY_MODIFIED_CODE)),
        "{r:?}"
    );
    assert_eq!(state.get_project_state().unwrap().name, "Cave");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// Deleting the open project tears the session down first, then trashes the
/// file and drops its recents row.
#[test]
fn deleting_the_open_project_closes_it_then_trashes_it() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    write_v2(&path, &survey("Cave"), &Extras::default());
    let state = open(&path);
    assert_eq!(recents_of(&state, &[]).len(), 1);

    let r = state.delete_project(path.clone());

    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.session_closed, Some(true));
    assert!(state.get_project_state().is_none());
    assert!(!path.exists());
    assert!(state.get_recent_projects().is_empty());
}
