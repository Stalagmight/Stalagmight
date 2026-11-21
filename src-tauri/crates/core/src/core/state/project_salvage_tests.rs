//! Salvage (plan 6.6): the recovery dialog's "Attempt Repair" and "Export
//! Salvageable Data" on a version 2 file the load refuses, and the legacy
//! SQLite route they keep.
//!
//! All data is synthetic: projects built in memory, saved by this build, then
//! damaged by editing the canonical text.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use super::AppState;
use crate::core::events::EventSink;
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

/// A 0.6.0 file that opens needs no repair: Attempt Repair writes nothing
/// and says so (0.6.0's REINDEX and VACUUM would only rewrite a healthy
/// original; review finding D-migration-upgrade-9).
#[test]
fn a_legacy_file_that_opens_is_not_repaired() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("legacy.stalx");
    super::stalx_fixtures::legacy_linked_project(&path, None);
    let before = std::fs::read(&path).unwrap();

    let result = state().repair_project_file(&path).unwrap();

    assert!(result.success, "{}", result.message);
    assert!(
        result.message.starts_with("Nothing to repair"),
        "{}",
        result.message
    );
    assert_eq!(result.backup_path, "");
    assert_eq!(std::fs::read(&path).unwrap(), before, "nothing written");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "no backup"
    );
}

/// A 0.6.0 file the open refuses for damage (a merge naming lines that do
/// not exist: a foreign-key violation 0.6.0's open refuses) gets 0.6.0's
/// repair behind a numbered backup, and the answer is the open's: REINDEX
/// and VACUUM leave the violation, so the repair does not say it succeeded
/// (review finding J-docs-tests-truth-6). Injection: report success on the
/// integrity check alone, as before, and this reads "Repair succeeded".
#[test]
fn a_damaged_legacy_file_is_repaired_and_judged_by_the_open() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("legacy.stalx");
    super::stalx_fixtures::legacy_linked_project(&path, None);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO station_merges (station_a_section, station_a_index, \
         station_b_section, station_b_index, merged_at) \
         VALUES (9998, 0, 9999, 0, '2023-11-14T22:13:20+00:00');",
    )
    .unwrap();
    drop(conn);
    assert!(
        !state().open_project(path.clone()).success,
        "the open refuses it"
    );

    let result = state().repair_project_file(&path).unwrap();

    assert!(!result.success, "{}", result.message);
    assert!(
        result
            .message
            .starts_with("Repair completed, but the project still cannot be opened"),
        "{}",
        result.message
    );
    assert!(Path::new(&result.backup_path).exists(), "behind a backup");
}

/// A file this build cannot convert is not damaged: no repair runs, and the
/// message says what it needs. A damaged one is repaired.
#[test]
fn a_conversion_refusal_is_never_repaired() {
    use crate::persistence::migrate::{LegacyError, MigrateError};
    let refused =
        super::project_salvage::legacy_without_repair(Err(&MigrateError::StationIdsExhausted))
            .expect("no repair");
    assert!(!refused.success);
    assert!(
        refused
            .message
            .starts_with("Not repaired: the file is not damaged."),
        "{}",
        refused.message
    );
    assert!(
        refused.message.contains("no station id is left"),
        "{}",
        refused.message
    );
    assert!(
        super::project_salvage::legacy_without_repair(Err(&MigrateError::Legacy(
            LegacyError::Corrupt("damaged".to_owned())
        )))
        .is_none()
    );
}

/// A migration refusal of a file 0.6.0 opened is "this file needs ...", with
/// the reason, never `CORRUPT` (which opens the recovery dialog and its
/// repair); real damage stays `CORRUPT` (review findings
/// D-migration-upgrade-9, J-docs-tests-truth-6). Injection: map every
/// migration error but the three typed ones to `LoadError::corrupt`, as
/// before.
#[test]
fn a_conversion_refusal_opens_as_cannot_convert_never_corrupt() {
    use crate::models::LineId;
    use crate::persistence::migrate::{LegacyError, MigrateError};
    let path = Path::new("/surveys/cave.stalx");
    for refusal in [
        MigrateError::StationIdsExhausted,
        MigrateError::DuplicateLineId(7),
        MigrateError::LineIdOutOfRange(LineId::MAX),
        MigrateError::Invalid(Vec::new()),
        MigrateError::Legacy(LegacyError::TooLarge { max: 1 << 30 }),
    ] {
        let error = super::project_load::migrate_error(path, &refusal);
        assert_eq!(error.code, "CANNOT_CONVERT", "{refusal}");
        assert!(
            error.message.starts_with("This file needs converting"),
            "{}",
            error.message
        );
        assert!(!error.message.contains("corrupt"), "{}", error.message);
        assert_eq!(error.details.as_deref(), Some("/surveys/cave.stalx"));
    }
    let too_large = super::project_load::migrate_error(
        path,
        &MigrateError::Legacy(LegacyError::TooLarge { max: 1 << 30 }),
    );
    assert!(
        too_large
            .message
            .contains("larger than the 1024 MiB this version reads"),
        "{}",
        too_large.message
    );
    let damaged = super::project_load::migrate_error(
        path,
        &MigrateError::Legacy(LegacyError::Corrupt("damaged".to_owned())),
    );
    assert_eq!(damaged.code, "CORRUPT");
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
