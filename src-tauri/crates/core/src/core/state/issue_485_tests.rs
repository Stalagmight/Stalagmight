//! [#485] A save must not drop the user's "Ignore" acknowledgements because the
//! file they were read from never ran the v8 migration.
//!
//! In 0.6.0 the save re-read `resolved_warnings` off the live SQLite handle,
//! so a pre-v8 table (no `shot_uuid` column) behind an unstamped
//! `PRAGMA user_version` made the read fail and the save carry an EMPTY
//! list. The station-graph core reads a legacy file once, through the frozen
//! 0.6.0 open (which runs the migration chain on a private copy) and the
//! conversion, which re-keys every ignore by its shot's uuid (plan 7.2.9). The
//! ignores are then in memory and ride the first save, which writes version 2.
//! What #485 pinned still holds and is pinned here: an ignore in an unstamped
//! or stamped pre-v8 file survives the upgrading save, naming the same shot.
//!
//! All data here is synthetic: a project built in memory, saved by the legacy
//! writer of this build, then hand-narrowed to the pre-v8 shape. No fixture is
//! read or written.

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{LineData, Project, ShotData};
use crate::persistence::stalx_v2::{self, IgnoredWarning};
use crate::persistence::Database;

const IGNORED_CODE: &str = "SHORT_SHOT";

/// Two shots that cannot cast a vote in the v12 pitch-flip migration (no
/// pitch, no depth change), so an assertion below cannot be measuring the
/// flip's `INCLINATION_DEPTH_MISMATCH` cleanup instead of the carry-over.
fn synthetic_project() -> Project {
    let unflippable = |length: f64| ShotData {
        length,
        pitch_in: None,
        pitch_out: None,
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let mut project = Project::new("Legacy".to_string(), None);
    project.add_line(LineData {
        color: "#abcdef".to_string(),
        shots: vec![unflippable(5.0), unflippable(7.0)],
        ..Default::default()
    });
    project
}

/// Build a `.stalx` in the shape that reached the bug: pre-v8 tables
/// (`resolved_warnings` with no `shot_uuid`), one legacy position-keyed ignore
/// on the first shot, an unstamped `PRAGMA user_version`, and a
/// `project.schema_version` that claims the current version. Returns the
/// ignored shot's uuid as the file stores it.
fn build_unstamped_pre_v8_file(path: &std::path::Path) -> String {
    let mut db = Database::open(path).unwrap();
    db.save_project(&synthetic_project()).unwrap();
    let saved = db.load_project().unwrap();
    let line_id = saved.lines[0].id;
    let shot_id = saved.lines[0].shots[0].id;
    let uuid = saved.lines[0].shots[0].uuid.clone().unwrap();
    drop(db);

    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(&format!(
        "DROP TABLE resolved_warnings;
         CREATE TABLE resolved_warnings (
            id INTEGER PRIMARY KEY,
            section_id INTEGER NOT NULL,
            shot_id INTEGER NOT NULL,
            warning_code TEXT NOT NULL,
            resolved_at TEXT NOT NULL,
            resolved_comment TEXT,
            UNIQUE(section_id, shot_id, warning_code),
            FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
         );
         INSERT INTO resolved_warnings (section_id, shot_id, warning_code, resolved_at)
            VALUES ({line_id}, {shot_id}, '{IGNORED_CODE}', '2026-01-01T00:00:00+00:00');
         PRAGMA user_version = 0;"
    ))
    .unwrap();

    // The fixture really is the shape under test.
    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_column, 0, "the fixture must be pre-v8");
    let stamped: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stamped, 0, "the fixture must carry no migration stamp");
    let claimed: i64 = conn
        .query_row("SELECT schema_version FROM project LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(
        claimed >= 8,
        "the project row must claim a post-v8 version, got {claimed}"
    );
    uuid
}

/// Open `path`, check the ignore is in memory, save (the upgrade to version
/// 2), and return what the saved file holds.
fn ignores_after_the_upgrading_save(path: &std::path::Path) -> (Vec<IgnoredWarning>, String) {
    crate::app_dirs::init_for_test();
    let state = AppState::new().unwrap();
    let opened = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(opened.success, "open failed: {:?}", opened.error);
    let live = state.with_file(|file| file.ignored_warnings().to_vec());
    assert_eq!(live.len(), 1, "the open must not lose the legacy ignore");

    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save failed: {:?}", saved.error);

    let bytes = std::fs::read(path).unwrap();
    let decoded = stalx_v2::decode(&bytes, &Default::default()).unwrap();
    assert_eq!(
        decoded.extras.ignored_warnings, live,
        "the save writes the ignores the open read"
    );
    let first = decoded.project.lines()[0].shots[0].uuid.clone();
    (decoded.extras.ignored_warnings, first)
}

/// Remove the pre-upgrade originals this test's save kept in the shared
/// app-data backups folder.
fn forget_backups_of(path: &std::path::Path) {
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    let folder = super::project_io::upgrade_backup_dir().unwrap();
    for entry in std::fs::read_dir(folder).unwrap().flatten() {
        if entry.file_name().to_string_lossy().starts_with(&stem) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// THE regression guard: an unstamped pre-v8 file's ignore survives the save
/// that upgrades it, keyed by the same shot.
#[test]
fn ignores_survive_a_save_through_a_database_that_never_ran_the_v8_migration() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp
        .path()
        .join(format!("unstamped-{}.stalx", uuid::Uuid::new_v4()));
    let uuid = build_unstamped_pre_v8_file(&path);

    let (rows, first_shot) = ignores_after_the_upgrading_save(&path);
    forget_backups_of(&path);

    assert_eq!(
        rows.len(),
        1,
        "the ignore must survive a save through an unstamped pre-v8 file"
    );
    assert_eq!(rows[0].code, IGNORED_CODE);
    assert_eq!(rows[0].shot, uuid, "and must still name the same shot");
    assert_eq!(rows[0].shot, first_shot);
}

/// The ordinary legacy shape - a stamped pre-v8 file - already worked. Pinned
/// so the route above cannot be "corrected" into skipping the normal upgrade
/// path.
#[test]
fn ignores_survive_a_save_through_a_stamped_pre_v8_file() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp
        .path()
        .join(format!("stamped-{}.stalx", uuid::Uuid::new_v4()));
    let uuid = build_unstamped_pre_v8_file(&path);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA user_version = 7;")
        .unwrap();

    let (rows, _) = ignores_after_the_upgrading_save(&path);
    forget_backups_of(&path);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].shot, uuid);
}
