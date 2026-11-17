//! [#485] A save must not drop the user's "Ignore" acknowledgements because the
//! connection it reads them through never ran the v8 migration.
//!
//! `save_project` snapshots `resolved_warnings` off the live handle
//! (`project_io.rs`, "carry resolved warnings across save") and re-applies it to
//! the fresh `.stalx.tmp` it renames into place. If that read fails, the save
//! continues with an EMPTY carry-over and every ignore is gone from the file -
//! the whole table is rebuilt from the snapshot, so a failed read is silent
//! data loss, not log noise.
//!
//! All data here is synthetic: a project built in memory, saved by this build,
//! then hand-narrowed to the pre-v8 shape. No fixture is read or written.

use tempfile::TempDir;

use super::AppState;
use crate::models::{LineData, Project, ShotData};
use crate::persistence::Database;

const IGNORED_CODE: &str = "SHORT_SHOT";

/// Two shots that cannot cast a vote in the v12 pitch-flip migration, spelled
/// out rather than left to `Default`.
///
/// `migrate_pitch_to_down_convention` only reads shots with a non-NULL
/// `pitch_in`/`pitch_out`, and only tallies those whose depth change exceeds
/// 0.05 m; a flipped line then has `invalidate_pre_pitch_flip_state` clear
/// `INCLINATION_DEPTH_MISMATCH` dismissals on it. The stamped control below runs
/// with `current = 7`, which reaches that arm, so the fixture must be provably
/// unflippable or the assertion could be measuring the flip instead of the save.
/// Both guards are pinned here: no pitch at all, and no depth change.
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

/// Build a `.stalx` in the shape that reaches the bug: pre-v8 tables
/// (`resolved_warnings` with no `shot_uuid`), one legacy position-keyed ignore,
/// an unstamped `PRAGMA user_version`, and a `project.schema_version` that
/// claims the current version - which is what `save_project` writes into every
/// file it touches, and what travels with the file through every copy.
fn build_unstamped_pre_v8_file(path: &std::path::Path) {
    let mut db = Database::open(path).unwrap();
    db.save_project(&synthetic_project()).unwrap();
    let saved = db.load_project().unwrap();
    let line_id = saved.lines[0].id;
    let shot_id = saved.lines[0].shots[0].id;
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

    // The fixture really is the shape under test, and the bug really is live in
    // it: without this the assertions below could pass vacuously.
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
}

/// THE regression guard. Red before the `stamped == SCHEMA_VERSION` fast-path
/// gate in `ensure_schema_version`: reinstate `current == SCHEMA_VERSION` there
/// and the ignore is gone from the saved file (0 rows), because
/// `load_resolved_warnings` fails with `no such column: shot_uuid` and the save
/// carries an empty set forward.
#[test]
fn ignores_survive_a_save_through_a_database_that_never_ran_the_v8_migration() {
    // open_project records the path in recent projects (global config).
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("unstamped.stalx");
    build_unstamped_pre_v8_file(&path);

    let state = AppState::new().unwrap();
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open failed: {:?}", opened.error);

    // Opening it is what must repair the schema, on the very handle the save
    // then reads through - so assert the v8 backfill HERE, before the save. The
    // same assertion after the save would prove nothing: the carry-over upgrades
    // a NULL-uuid row to its shot's UUID itself (project_io.rs, the `None` arm of
    // the remap), so a post-save `shot_uuid` is `Some` either way.
    let live = state.load_resolved_warnings().unwrap();
    assert_eq!(live.len(), 1, "the open must not lose the legacy ignore");
    assert!(
        live[0].shot_uuid.is_some(),
        "the v8 migration must have run and backfilled the legacy row's shot UUID"
    );

    let saved = state.save_project(Some(path.clone()));
    assert!(saved.success, "save failed: {:?}", saved.error);

    let rows = Database::open_existing(&path)
        .unwrap()
        .load_resolved_warnings()
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the ignore must survive a save through an unstamped pre-v8 file"
    );
    assert_eq!(rows[0].warning_code, IGNORED_CODE);
    assert_eq!(
        rows[0].shot_uuid, live[0].shot_uuid,
        "and must still name the same shot"
    );
}

/// The ordinary legacy shape - a stamped pre-v8 file - already worked. Pinned so
/// the fix above cannot be "corrected" into skipping the normal upgrade path.
///
/// `current = 7` reaches the v12 arm, so this also exercises the pitch flip; the
/// fixture is deliberately unflippable (see `synthetic_project`) and the ignore
/// carries `SHORT_SHOT`, not the `INCLINATION_DEPTH_MISMATCH` code that
/// `invalidate_pre_pitch_flip_state` clears.
#[test]
fn ignores_survive_a_save_through_a_stamped_pre_v8_file() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("stamped.stalx");
    build_unstamped_pre_v8_file(&path);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA user_version = 7;")
        .unwrap();

    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    assert!(state.save_project(Some(path.clone())).success);

    let rows = Database::open_existing(&path)
        .unwrap()
        .load_resolved_warnings()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].shot_uuid.is_some());
}
