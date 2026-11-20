//! Salvage (plan 6.6): the recovery dialog's "Attempt Repair" and "Export
//! Salvageable Data" on a version 2 file the load refuses, the legacy SQLite
//! route they keep, and the open project's integrity Repair.
//!
//! All data is synthetic: projects built in memory, saved by this build, then
//! damaged by editing the canonical text.
#![expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use super::AppState;
use crate::core::events::EventSink;
use crate::core::Command;
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::ShotData;
use crate::persistence::stalx_v2::{self, Extras, Mode, SpeleoDbLink};

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

fn state() -> AppState {
    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(NullSink));
    state
}

/// Two lines, the second hanging off the first's last station, placed.
fn survey() -> Project {
    let shot = |length: f64| ShotData {
        length,
        azimuth_in: 45.0,
        azimuth_out: 45.0,
        ..ShotData::default()
    };
    let mut p = Project::new("Salvage", None);
    let main = p
        .add_line(
            LineMeta::default(),
            StartAt::New,
            vec![shot(3.0), shot(4.0)],
        )
        .unwrap();
    let end = p.line(main).unwrap().station(2).unwrap();
    p.add_line(LineMeta::default(), StartAt::Station(end), vec![shot(2.0)])
        .unwrap();
    let start = p.line(main).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    p
}

/// The canonical text of [`survey`] with `extras`.
fn text(extras: &Extras) -> String {
    String::from_utf8(stalx_v2::encode(&survey(), extras, Mode::Normal).unwrap()).unwrap()
}

/// `text` without its `member` (from `"<member>":[` to the `],` line end).
fn without(text: &str, member: &str) -> String {
    let at = text.find(&format!("\"{member}\":[")).unwrap();
    let end = at + text[at..].find("],\n").unwrap() + 3;
    format!("{}{}", &text[..at], &text[end..])
}

/// A file whose piece has no anchor (the placement is gone): the load
/// refuses it, salvage places the piece at the origin.
fn damaged(dir: &Path, extras: &Extras) -> PathBuf {
    let path = dir.join("cave.stalx");
    std::fs::write(&path, without(&text(extras), "placements")).unwrap();
    path
}

#[test]
fn the_load_refuses_the_damaged_file_and_points_at_salvage() {
    let dir = TempDir::new().unwrap();
    let path = damaged(dir.path(), &Extras::default());

    let Err(err) = state().do_load_project_full(path) else {
        unreachable!("the load must refuse a survey that breaks an invariant");
    };

    assert_eq!(err.code, "CORRUPT");
    assert!(
        err.message.contains("cannot be opened normally"),
        "{}",
        err.message
    );
}

/// Attempt Repair: the damaged file is kept as `.backup`, the repaired one
/// takes its place, opens, and lists what was repaired.
#[test]
fn attempt_repair_repairs_in_place_behind_a_backup() {
    let dir = TempDir::new().unwrap();
    let path = damaged(dir.path(), &Extras::default());
    let original = std::fs::read(&path).unwrap();

    let result = state().repair_project_file(&path).unwrap();

    assert!(result.success, "{}", result.message);
    let backup = dir.path().join("cave.stalx.backup");
    assert_eq!(Path::new(&result.backup_path), backup);
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    assert!(
        result
            .message
            .starts_with("Repair succeeded: 1 fix(es): placed the piece of station"),
        "{}",
        result.message
    );
    assert!(
        result
            .message
            .ends_with("The damaged original is kept as cave.stalx.backup."),
        "{}",
        result.message
    );
    let opened = state();
    opened.do_load_project_full(path).unwrap();
    let repaired = opened.get_project_state().unwrap();
    assert_eq!(repaired.placements.len(), 1);
    assert_eq!(repaired.lines().len(), 2);
    assert!(
        !opened.can_undo(),
        "the history describes the file before the repair"
    );
}

/// A second repair never overwrites the first backup.
#[test]
fn attempt_repair_numbers_its_backups() {
    let dir = TempDir::new().unwrap();
    let path = damaged(dir.path(), &Extras::default());
    std::fs::write(dir.path().join("cave.stalx.backup"), b"an older backup").unwrap();

    let result = state().repair_project_file(&path).unwrap();

    assert_eq!(
        Path::new(&result.backup_path),
        dir.path().join("cave.stalx.backup.1")
    );
    assert_eq!(
        std::fs::read(dir.path().join("cave.stalx.backup")).unwrap(),
        b"an older backup"
    );
}

/// A SpeleoDB-managed copy keeps its link and is marked as having unpushed
/// edits: it no longer matches the revision it was based on.
#[test]
fn a_repaired_managed_copy_is_marked_unpushed() {
    let dir = TempDir::new().unwrap();
    let extras = Extras {
        speleodb: Some(SpeleoDbLink {
            endpoint: "https://sdb.example".to_string(),
            project_id: "11111111-2222-3333-4444-555555555555".to_string(),
            project_name: "Remote".to_string(),
            undo_mode: None,
        }),
        ..Extras::default()
    };
    let path = damaged(dir.path(), &extras);

    assert!(state().repair_project_file(&path).unwrap().success);

    let decoded = stalx_v2::decode(
        &std::fs::read(&path).unwrap(),
        &stalx_v2::DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(decoded.extras.speleodb, extras.speleodb);
    assert!(decoded.extras.local.speleodb.unpushed_edits);
}

/// A file salvage cannot rebuild (torn: it does not parse) is reported as
/// not repaired, and nothing is written.
#[test]
fn a_file_that_cannot_be_rebuilt_is_left_as_it_is() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    let full = text(&Extras::default());
    std::fs::write(&path, &full[..full.len() / 2]).unwrap();
    let before = std::fs::read(&path).unwrap();

    let result = state().repair_project_file(&path).unwrap();

    assert!(!result.success);
    assert!(result.backup_path.is_empty());
    assert!(
        result
            .message
            .starts_with("Repair could not rebuild the project ("),
        "{}",
        result.message
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!dir.path().join("cave.stalx.backup").exists());
}

/// A file that opens normally needs nothing: no backup, no write.
#[test]
fn a_sound_file_needs_no_repair() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    std::fs::write(&path, text(&Extras::default())).unwrap();
    let before = std::fs::read(&path).unwrap();

    let result = state().repair_project_file(&path).unwrap();

    assert!(result.success);
    assert_eq!(
        result.message,
        "The project opens normally: nothing needed repairing."
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!dir.path().join("cave.stalx.backup").exists());
}

/// Never the open project, and never a file a newer Stalagmight wrote (#649).
#[test]
fn the_open_project_and_newer_files_are_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    std::fs::write(&path, text(&Extras::default())).unwrap();
    let state = state();
    state.do_load_project_full(path.clone()).unwrap();
    let err = state.repair_project_file(&path).unwrap_err();
    assert_eq!(
        err,
        "Not repaired: cave.stalx is open in Stalagmight. Close it, then repair."
    );

    let newer = dir.path().join("newer.stalx");
    let content = without(&text(&Extras::default()), "placements").replacen(
        "\"version\":2,",
        "\"version\":3,",
        1,
    );
    std::fs::write(&newer, &content).unwrap();
    let err = AppState::new()
        .unwrap()
        .repair_project_file(&newer)
        .unwrap_err();
    assert!(
        err.starts_with("Repair failed: \"newer.stalx\" was saved by a newer version"),
        "{err}"
    );
    assert_eq!(std::fs::read_to_string(&newer).unwrap(), content);
}

/// A legacy SQLite file keeps 0.6.0's repair: a numbered backup, then
/// REINDEX and VACUUM in place.
#[test]
fn a_legacy_file_keeps_the_sqlite_repair() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("legacy.stalx");
    super::legacy_fixture_kit::write_legacy_file(
        &path,
        &super::legacy_fixture_kit::Legacy::new("Legacy".to_string(), None),
    );

    let result = state().repair_project_file(&path).unwrap();

    assert!(result.success, "{}", result.message);
    assert_eq!(
        result.message,
        "Repair succeeded - database integrity restored."
    );
    assert!(Path::new(&result.backup_path).exists());
}

#[test]
fn something_that_is_no_project_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("notes.stalx");
    std::fs::write(&path, b"shopping list").unwrap();

    let err = state().repair_project_file(&path).unwrap_err();

    assert_eq!(err, "Repair failed: this is not a Stalagmight project file");
}

/// Export Salvageable Data: the repaired project as canonical version 2 JSON
/// (it opens once renamed to `.stalx`), counts, the repairs as warnings,
/// nothing written.
#[test]
fn salvage_returns_the_repaired_project_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let path = damaged(dir.path(), &Extras::default());
    let before = std::fs::read(&path).unwrap();

    let salvaged = AppState::salvage_project_file(&path).unwrap();

    assert_eq!(salvaged.project_name.as_deref(), Some("Salvage"));
    assert_eq!(
        (
            salvaged.line_count,
            salvaged.shot_count,
            salvaged.merge_count
        ),
        (2, 3, 1),
        "one junction"
    );
    let lines: serde_json::Value = serde_json::from_str(&salvaged.lines_json.unwrap()).unwrap();
    assert_eq!(lines.as_array().unwrap().len(), 2);
    let copy = dir.path().join("salvaged.stalx");
    std::fs::write(&copy, salvaged.full_project_json.unwrap()).unwrap();
    let opened = state();
    opened.do_load_project_full(copy).unwrap();
    assert_eq!(opened.get_project_state().unwrap().lines().len(), 2);
    assert_eq!(salvaged.errors.len(), 1, "{:?}", salvaged.errors);
    assert!(salvaged.errors[0].starts_with("Repaired: placed the piece"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// When nothing can be rebuilt, the header still names the project and its
/// counts, and the reason is a warning.
#[test]
fn salvage_of_a_torn_file_reports_what_the_header_says() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    let full = text(&Extras::default());
    let cut = full.find("\"stations\":[").unwrap() + 20;
    std::fs::write(&path, &full[..cut]).unwrap();

    let salvaged = AppState::salvage_project_file(&path).unwrap();

    assert_eq!(salvaged.project_name.as_deref(), Some("Salvage"));
    assert_eq!((salvaged.line_count, salvaged.shot_count), (2, 3));
    assert!(salvaged.full_project_json.is_none());
    assert!(
        salvaged.errors[0].starts_with("The project could not be rebuilt: "),
        "{:?}",
        salvaged.errors
    );
}

/// The open project always holds the invariants: the banner's Repair finds
/// nothing, changes nothing and records no undo step.
#[test]
fn the_open_projects_repair_has_nothing_to_do() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cave.stalx");
    std::fs::write(&path, text(&Extras::default())).unwrap();
    let state = AppState::new().unwrap();
    assert!(state.execute(Command::OpenProject { path }).success);
    let before = state.get_project_state().unwrap();

    let result = state.repair_open_project_integrity();

    assert_eq!(
        (result.fixed, result.remaining, result.changed),
        (0, 0, false)
    );
    assert_eq!(state.get_project_state().unwrap(), before);
    assert!(!state.can_undo());
    assert!(!state.has_unsaved_changes());
}
