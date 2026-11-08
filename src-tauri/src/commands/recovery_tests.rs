//! [#576 C15, NF-AN2-7] Restoring a project from the recovery dialog.
//!
//! Driven through the IPC command itself (`recovery::restore_from_backup`), so
//! the guarantees are the ones the dialog gets:
//! - every restore keeps the file it replaces as a NUMBERED copy, so a second
//!   restore never destroys the only copy of the original;
//! - a candidate that is not a usable project is refused before anything is
//!   touched;
//!
//! The rollback-journal cases (an interrupted save's `.tmp`, a journal left
//! beside the project) need raw SQLite to build, so they live with
//! `Database::restore_from_backup` in the persistence crate.
//!
//! All files are synthetic projects this build writes; no fixture is read.

use std::path::Path;

use tempfile::TempDir;

use super::recovery;
use crate::models::{Project, SectionData, ShotData, StationRef};
use crate::persistence::Database;

/// A valid project file named `name` at `path`.
fn project_file(path: &Path, name: &str) {
    let mut db = Database::open(path).unwrap();
    db.save_project(&Project::new(name.to_string(), None))
        .unwrap();
}

/// The project name stored in `path`, read through the ordinary open path.
fn project_name(path: &Path) -> String {
    Database::open_existing(path)
        .unwrap()
        .load_project()
        .unwrap()
        .name
}

/// Every pre-restore copy beside `project` and the project name each holds.
fn pre_restore_copies(project: &Path) -> Vec<(String, String)> {
    let stem = project.file_name().unwrap().to_string_lossy().into_owned();
    let mut copies: Vec<(String, String)> = std::fs::read_dir(project.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with(&format!("{stem}.pre-restore")) && !name.ends_with("-journal")
        })
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let held = Database::open_existing(&p)
                .and_then(|db| db.load_project())
                .map_or_else(|e| format!("<unreadable: {e}>"), |project| project.name);
            (name, held)
        })
        .collect();
    copies.sort();
    copies
}

fn restore(project: &Path, candidate: &Path) -> Result<String, crate::core::CommandError> {
    tauri::async_runtime::block_on(recovery::restore_from_backup(
        project.to_path_buf(),
        candidate.to_path_buf(),
    ))
}

/// [#576 C15] Two restores in a row keep BOTH files they replaced. The safety
/// copy used to be one fixed name overwritten by every restore, so trying a
/// second backup after a first bad one destroyed the only copy of the original.
#[test]
fn a_second_restore_keeps_the_original_file() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "Original");
    let first = temp.path().join("CaveA.stalx.backup");
    project_file(&first, "BackupOne");
    let second = temp.path().join("CaveA.stalx.backup.1");
    project_file(&second, "BackupTwo");

    restore(&project, &first).unwrap();
    restore(&project, &second).unwrap();

    let copies = pre_restore_copies(&project);
    println!("project now holds: {}", project_name(&project));
    println!("pre-restore copies: {copies:?}");
    assert_eq!(project_name(&project), "BackupTwo");
    let held: Vec<&str> = copies.iter().map(|(_, name)| name.as_str()).collect();
    assert!(
        held.contains(&"Original") && held.contains(&"BackupOne"),
        "both replaced files must survive, got {copies:?}"
    );
}

/// [#576 C15] A candidate that is not a usable project (here: leftover bytes
/// that are not a database at all) is refused, and the project is untouched.
/// It used to be copied over the project with nothing checked but `exists()`.
#[test]
fn an_unusable_candidate_is_refused_and_the_project_is_untouched() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "Original");
    let before = std::fs::read(&project).unwrap();
    let debris = temp.path().join("CaveA.stalx.tmp");
    std::fs::write(&debris, b"leftover bytes of a save that never finished").unwrap();

    let outcome = restore(&project, &debris);
    println!("restore -> {outcome:?}");
    assert!(outcome.is_err(), "an unusable candidate must be refused");
    assert!(
        std::fs::read(&project).unwrap() == before,
        "the project file must be exactly as it was"
    );
}

// --- [#576 HFI-3] Export Salvageable Data: the backend writes the file ---------

/// Run the salvage write through the command itself.
fn write_salvage(
    project: &Path,
    dest: &Path,
) -> Result<crate::persistence::SalvageWriteResult, crate::core::CommandError> {
    tauri::async_runtime::block_on(recovery::write_salvaged_data(
        project.to_path_buf(),
        dest.to_path_buf(),
    ))
}

/// The file the dialog chose holds the salvage of the project - the JSON the
/// dialog used to write from the webview (which the fs scope refused in the real
/// app) - into a new folder, and over a file the OS dialog already agreed to
/// replace.
#[test]
fn the_salvage_export_writes_the_chosen_file() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "SalvageMe");
    let folder = temp.path().join("new-empty-folder");
    std::fs::create_dir(&folder).unwrap();
    let dest = folder.join("salvaged.json");

    let written = write_salvage(&project, &dest);
    println!(
        "write_salvaged_data: {written:?}; file written: {}",
        dest.exists()
    );
    written.unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
    println!("salvaged project name in the file: {}", json["name"]);
    assert_eq!(json["name"], "SalvageMe");

    // The same bytes a salvage read of the project gives.
    let read = Database::export_salvageable(&project).unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        read.full_project_json.unwrap()
    );

    // Over an existing file (the OS save dialog asked about this exact name).
    std::fs::write(&dest, b"an older salvage").unwrap();
    write_salvage(&project, &dest).unwrap();
    assert!(std::fs::read_to_string(&dest)
        .unwrap()
        .contains("SalvageMe"));
}

/// A project with two sections (3 and 2 shots) joined by one station merge,
/// saved to `path`.
fn project_with_shots_and_a_merge(path: &Path) {
    let shot = |azimuth: f64| ShotData {
        length: 4.0,
        azimuth_in: azimuth,
        azimuth_out: (azimuth + 180.0) % 360.0,
        depth_in: 2.0,
        depth_out: 2.0,
        ..Default::default()
    };
    let mut project = Project::new("CaveA".to_string(), None);
    let mut add = |azimuth: f64, shots: usize| {
        project
            .add_section(SectionData {
                shots: (0..shots).map(|_| shot(azimuth)).collect(),
                ..Default::default()
            })
            .id
    };
    let (trunk, branch) = (add(0.0, 3), add(90.0, 2));
    project
        .merge_stations(StationRef::new(trunk, 2), StationRef::new(branch, 0))
        .unwrap();
    Database::open(path)
        .unwrap()
        .save_project(&project)
        .unwrap();
}

/// [#576 W30-5] The salvage export answers the counts and warnings of the very
/// salvage it wrote, and the dialog reports those. It answered nothing, and the
/// dialog reported the counts of its own earlier read: a separate read of a
/// damaged file, which need not be the one that was written (the file is
/// damaged; it can read differently twice). Checked on the wire the dialog gets:
/// the counts equal what the written file holds.
#[test]
fn the_salvage_export_answers_the_counts_of_the_file_it_wrote() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_with_shots_and_a_merge(&project);
    let dest = temp.path().join("salvaged.json");

    let answered =
        tauri::async_runtime::block_on(recovery::write_salvaged_data(project, dest.clone()))
            .unwrap();
    let wire = serde_json::to_value(&answered).unwrap();

    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
    let sections = file["sections"].as_array().unwrap();
    let shots: usize = sections
        .iter()
        .map(|section| section["shots"].as_array().unwrap().len())
        .sum();
    let merges = file["station_merges"].as_array().unwrap().len();
    println!("write_salvaged_data answered: {wire}");
    println!(
        "the written file holds: {} sections, {shots} shots, {merges} merges",
        sections.len()
    );
    // Premise: the file is the whole project, so a zero count cannot pass.
    assert_eq!((sections.len(), shots, merges), (2, 5, 1));
    assert_eq!(wire["sectionCount"], sections.len(), "sectionCount");
    assert_eq!(wire["shotCount"], shots, "shotCount");
    assert_eq!(wire["mergeCount"], merges, "mergeCount");
    // An intact project salvages with no warning; the list is on the wire.
    assert_eq!(wire["errors"], serde_json::json!([]), "errors");
    // [#576 Q-W30-2-D1] And it says the file holds the whole project.
    assert_eq!(wire["projectIncluded"], true, "projectIncluded");
}

/// [#576 Q-W37-1-D1] The dialog's early check only OPENS the damaged project:
/// it passes a file that opens although it holds no project (the write then
/// salvages what it can), and fails - in the salvage's own words - only where
/// the write's open would: no file, or no database at all.
#[test]
fn the_salvage_check_only_opens_the_file() {
    let check = |path: &Path| {
        tauri::async_runtime::block_on(recovery::check_salvage_source(path.to_path_buf()))
    };
    let temp = TempDir::new().unwrap();
    // A database with the project schema and no project in it.
    let empty = temp.path().join("Empty.stalx");
    drop(Database::open(&empty).unwrap());
    let missing = temp.path().join("Missing.stalx");
    let garbage = temp.path().join("Garbage.stalx");
    std::fs::write(&garbage, b"not a database").unwrap();

    let (empty, missing, garbage) = (check(&empty), check(&missing), check(&garbage));
    println!("opens, holds no project: {empty:?}");
    println!("missing: {missing:?}");
    println!("not a database: {garbage:?}");
    assert!(empty.is_ok());
    assert_eq!(
        missing.unwrap_err().message,
        "Salvage failed: File does not exist"
    );
    assert!(garbage.unwrap_err().message.starts_with("Salvage failed: "));
}

/// Refused like every other write: the app's own folders, a non-JSON name, and
/// the damaged project itself - and nothing is written or touched.
#[test]
fn the_salvage_export_refuses_where_no_command_writes() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "SalvageMe");

    let app = TempDir::new().unwrap();
    let dirs = crate::commands::write_path::AppDirs {
        internal: vec![app.path().to_path_buf()],
        managed_projects: None,
    };
    let inside_app = app.path().join("salvaged.json");
    let refused = recovery::write_salvaged_data_with(&project, &inside_app, &dirs);
    println!("into the app's folder: {refused:?}");
    assert!(refused.unwrap_err().message.contains("app's own settings"));
    assert!(!inside_app.exists(), "nothing written there");

    let not_json = temp.path().join("notes.txt");
    let refused = write_salvage(&project, &not_json);
    println!("to notes.txt: {refused:?}");
    assert!(refused.unwrap_err().message.contains("must end in .json"));
    assert!(!not_json.exists());

    // A damaged project opened under a .json name (the open dialog offers "All
    // Files"): its own salvage must not replace it.
    let odd = temp.path().join("odd.json");
    project_file(&odd, "OddName");
    let before = std::fs::read(&odd).unwrap();
    let refused = write_salvage(&odd, &odd);
    println!("over the damaged project itself: {refused:?}");
    assert!(refused
        .unwrap_err()
        .message
        .contains("damaged project itself"));
    assert!(
        std::fs::read(&odd).unwrap() == before,
        "the project is untouched"
    );
}

#[cfg(unix)]
#[test]
fn the_salvage_export_refuses_a_system_folder() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "SalvageMe");
    let refused = write_salvage(&project, Path::new("/etc/stalagmight-hfi3.json"));
    println!("into /etc: {refused:?}");
    assert!(refused
        .unwrap_err()
        .message
        .contains("inside a system folder"));
}

/// A folder the user may not write to: an error with the reason, nothing written -
/// not a success the dialog would report as saved.
#[cfg(unix)]
#[test]
fn the_salvage_export_into_an_unwritable_folder_fails_and_says_why() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "SalvageMe");
    let locked = temp.path().join("read-only");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root (CAP_DAC_OVERRIDE) ignores the mode bits; fail rather than pass untested.
    crate::models::test_support::assert_dir_refuses_new_files(&locked);

    let dest = locked.join("salvaged.json");
    let written = write_salvage(&project, &dest);

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    println!(
        "into a read-only folder: {written:?}; file written: {}",
        dest.exists()
    );
    let err = written.unwrap_err();
    assert_eq!(
        err.code,
        crate::core::CommandErrorCode::Failed,
        "{}",
        err.message
    );
    assert!(
        err.message.contains("Could not write salvaged.json")
            && err.message.contains("Permission denied"),
        "{}",
        err.message
    );
    assert!(!dest.exists());
}
