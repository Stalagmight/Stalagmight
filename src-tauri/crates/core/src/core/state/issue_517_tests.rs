//! [#517] A save must not turn "I could not read the ignore list" into "there
//! are no ignores".
//!
//! `save_project` (`project_io.rs`, "carry resolved warnings across save")
//! snapshots `resolved_warnings` off the live handle and re-applies it to the
//! fresh `.stalx.tmp` it renames over the destination, so that snapshot IS the
//! saved file's ignore list. #485 closed the migration-skip that made the read
//! fail; this pins the amplifier behind it, which survived that fix, and the
//! split it turns on:
//!
//! * a SHAPE the current SELECT cannot name (a pre-v8 table with no
//!   `shot_uuid`, reachable with a FORGED `PRAGMA user_version` claiming the
//!   chain already ran) is read verbatim and carried - on the display path too,
//!   or the user would be shown ignores the save is quietly preserving;
//! * a hostile VALUE (SQLite columns are untyped, so any cell can be a BLOB) is
//!   coerced and carried inert, because refusing on one poisoned row would make
//!   a file the user merely opened permanently unsaveable;
//! * only a STRUCTURAL failure - a missing key column, a table that cannot be
//!   read at all - refuses the save, instead of writing a file with zero
//!   ignores over the one that had them.
//!
//! All data here is synthetic: a project built in memory, saved by this build,
//! then hand-narrowed. No fixture is read or written.

use tempfile::TempDir;

use super::AppState;
use crate::models::CommandErrorCode;
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;

const IGNORED_CODE: &str = "SHORT_SHOT";
const RESOLVED_AT: &str = "2026-01-01T00:00:00+00:00";

/// The pre-v8 `resolved_warnings` shape: every current column except
/// `shot_uuid`.
const PRE_V8_COLUMNS: &str = "section_id INTEGER NOT NULL, \
     shot_id INTEGER NOT NULL, \
     warning_code TEXT NOT NULL, \
     resolved_at TEXT NOT NULL, \
     resolved_comment TEXT, \
     UNIQUE(section_id, shot_id, warning_code)";

/// A table whose rows do not say WHICH warning was ignored - no amount of
/// leniency recovers that.
const UNREADABLE_COLUMNS: &str = "section_id INTEGER NOT NULL, \
     shot_id INTEGER NOT NULL, \
     resolved_at TEXT NOT NULL";

/// The hostile-values shape: `shot_id` and `warning_code` are declared with NO
/// type, so the cells keep whatever the file put in them.
///
/// This matters, and it is not a detail. A column declared `INTEGER` has
/// INTEGER AFFINITY, and SQLite silently CONVERTS a numeric-looking TEXT into
/// an integer on insert - so a fixture that stores a text id in a declared
/// INTEGER column is not testing a text id at all. The tests below assert
/// `typeof()` afterwards rather than trusting this.
const UNTYPED_ID_COLUMNS: &str = "section_id INTEGER NOT NULL, \
     shot_id, \
     warning_code, \
     resolved_at, \
     resolved_comment";

/// What the seeded file holds, as this build wrote it.
struct Seeded {
    section_id: u32,
    shot_id: u32,
    shot_uuid: String,
}

/// Two shots that both trip `SHORT_SHOT` (under the default 0.3 m minimum) and
/// that the v12 pitch-flip migration cannot vote on, so no arm of the migration
/// chain can be what an assertion below is measuring (the reasoning is spelled
/// out in `issue_485_tests::synthetic_project`).
///
/// The first shot is the one the fixtures ignore; the second is the control that
/// must KEEP its warning, so "suppressed" is never confused with "the warning
/// list came back empty".
fn synthetic_project() -> Project {
    let unflippable = |length: f64| ShotData {
        length,
        pitch_in: None,
        pitch_out: None,
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let mut project = Project::new("Forged".to_string(), None);
    project.add_section(SectionData {
        color: "#abcdef".to_string(),
        shots: vec![unflippable(0.1), unflippable(0.2)],
        ..Default::default()
    });
    project
}

/// Write a `.stalx` this build considers current, and report the ids it chose.
fn seed_current_file(path: &std::path::Path) -> Seeded {
    let mut db = Database::open(path).unwrap();
    db.save_project(&synthetic_project()).unwrap();
    let saved = db.load_project().unwrap();
    Seeded {
        section_id: saved.sections[0].id,
        shot_id: saved.sections[0].shots[0].id,
        shot_uuid: saved.sections[0].shots[0].uuid.clone().unwrap(),
    }
}

/// Replace `resolved_warnings` with `ddl` holding `values_rows`, WITHOUT
/// touching the stamp `Database::open` wrote.
///
/// That is the hostile shape: `PRAGMA user_version` says the whole migration
/// chain ran - so `ensure_schema_version` fast-paths and no `ALTER` ever fires
/// (#516 closed the ACCIDENTAL route into that state, not a deliberate one) -
/// while the table underneath is whatever `ddl` says. The stamp is read back
/// from the file rather than hard-coded, so this keeps meaning "the CURRENT
/// version" after the next schema bump.
fn forge_ignore_table(path: &std::path::Path, ddl: &str, columns: &str, values_rows: &[String]) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let stamped: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let inserts = values_rows
        .iter()
        .map(|v| format!("INSERT INTO resolved_warnings ({columns}) VALUES ({v});"))
        .collect::<Vec<_>>()
        .join("\n");
    conn.execute_batch(&format!(
        "DROP TABLE resolved_warnings;
         CREATE TABLE resolved_warnings (
            id INTEGER PRIMARY KEY,
            {ddl},
            FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
         );
         {inserts}"
    ))
    .unwrap();

    // The fixture must really be the shape under test, or every assertion in
    // the tests below could pass vacuously.
    assert!(
        stamped >= 8,
        "the current schema must be past the v8 shot_uuid migration, got {stamped}"
    );
    let still_stamped: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        still_stamped, stamped,
        "the forged file must still CLAIM the current schema"
    );
    let has_uuid_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_uuid_column, 0, "the forged table must be pre-v8");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM resolved_warnings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        rows,
        values_rows.len() as i64,
        "the forged file must hold every row the fixture asked for"
    );
}

/// How many rows the file at `path` holds, read WITHOUT this crate's loaders so
/// a bug in them cannot make an assertion pass.
fn raw_row_count(path: &std::path::Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM resolved_warnings", [], |r| r.get(0))
        .unwrap()
}

/// THE forged-stamp regression guard, on BOTH sides of the file.
///
/// A hostile (or merely hand-edited) file can stamp the current `user_version`
/// over pre-v8 tables, which sends `ensure_schema_version` down its fast path
/// and leaves `shot_uuid` missing forever. The save must carry the ignore, and
/// the warning list the user is looking at must AGREE - a save that quietly
/// preserves rows the display says are not there is its own defect.
///
/// Red before the fix: swap `load_resolved_warnings_verbatim` back to
/// `load_resolved_warnings` in `project_io.rs` and the save arm fails; swap it
/// back in `core::state::warnings` and the `is_resolved` assertion fails, with
/// the save assertions still green.
#[test]
fn a_forged_current_stamp_over_pre_v8_tables_does_not_erase_the_ignore_list() {
    // open_project records the path in recent projects (global config).
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("forged.stalx");
    let seeded = seed_current_file(&path);
    forge_ignore_table(
        &path,
        PRE_V8_COLUMNS,
        "section_id, shot_id, warning_code, resolved_at",
        &[format!(
            "{}, {}, '{IGNORED_CODE}', '{RESOLVED_AT}'",
            seeded.section_id, seeded.shot_id
        )],
    );

    let state = AppState::new().unwrap();
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open failed: {:?}", opened.error);

    // The DISPLAY path, before any save: the ignored shot's warning must come
    // back suppressed, and the OTHER short shot's must not - otherwise this
    // would also pass on an empty warning list.
    let project = state.get_project_state().unwrap();
    let warnings = state.compute_project_warnings(&project, None);
    let short: Vec<_> = warnings.iter().filter(|w| w.code == IGNORED_CODE).collect();
    assert_eq!(
        short.len(),
        2,
        "both short shots must still be REPORTED, ignored or not"
    );
    assert!(
        short
            .iter()
            .find(|w| w.shot_index == 0)
            .unwrap()
            .is_resolved,
        "the recorded ignore must suppress the warning the user is looking at"
    );
    assert!(
        !short
            .iter()
            .find(|w| w.shot_index == 1)
            .unwrap()
            .is_resolved,
        "and must suppress ONLY the shot it was recorded against"
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
        "the ignore must survive a save through a forged-stamp pre-v8 file"
    );
    assert_eq!(rows[0].warning_code, IGNORED_CODE);
    assert_eq!(
        rows[0].shot_uuid.as_deref(),
        Some(seeded.shot_uuid.as_str()),
        "and must still name the shot it was recorded against"
    );
}

/// SQLite columns are untyped, so a hand-written file can put a BLOB in any of
/// them. That must not cost the user their SAVE: one poisoned row would
/// otherwise fail the carry-over read, hit the refusal, and leave the project
/// unsaveable for good - turning #517's silent data loss into an availability
/// denial from a file that was merely opened.
///
/// Red before the fix: drop the `coerce_*` helpers (read the cells with
/// `row.get::<String>` / `row.get::<u32>`) and the save is refused, so the
/// `success` assertion fails. Red again if `MAX_ID_TEXT_BYTES` is removed from
/// `coerce_id`: the padded-id row below then parses and is carried, so the
/// carried count is 3.
#[test]
fn hostile_column_types_are_carried_inert_instead_of_making_the_project_unsaveable() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("hostile.stalx");
    let seeded = seed_current_file(&path);
    forge_ignore_table(
        &path,
        UNTYPED_ID_COLUMNS,
        "section_id, shot_id, warning_code, resolved_at",
        &[
            // Readable, and the row whose survival is the point.
            format!(
                "{}, {}, '{IGNORED_CODE}', '{RESOLVED_AT}'",
                seeded.section_id, seeded.shot_id
            ),
            // A code and a timestamp that are not text at all, and not even
            // valid UTF-8. Carried inert: it suppresses nothing, but it is the
            // user's row and the save has no business dropping it.
            format!(
                "{}, {}, X'FFFE', X'FFFE'",
                seeded.section_id, seeded.shot_id
            ),
            // Names no shot, so it can never suppress anything on any project.
            // Dropped - the one case where dropping loses nothing.
            format!(
                "{}, X'FFFE', 'LONG_SHOT', '{RESOLVED_AT}'",
                seeded.section_id
            ),
            // A multi-megabyte TEXT id. Rejected on LENGTH before it is decoded
            // or parsed: an id is at most 10 digits, so validating megabytes of
            // UTF-8 to reach that answer is work a hostile file must not be able
            // to buy, once per row per save.
            format!(
                "{}, printf('%.*c', 4000000, '7'), 'AZIMUTH_DEVIATION', '{RESOLVED_AT}'",
                seeded.section_id
            ),
            // The same guard, made observable: 65 characters of zero-padding in
            // front of a LIVE shot id. Without the byte cap this parses happily
            // and the row is carried; with it, the cell is refused for its
            // length before `parse` ever runs. No file a user made looks like
            // this, which is exactly why the cheap check is safe.
            format!(
                "{}, printf('%065d', {}), 'DEPTH_MISMATCH', '{RESOLVED_AT}'",
                seeded.section_id, seeded.shot_id
            ),
        ],
    );

    // The cells really are the types this test is about. Without this the
    // padded id would be silently converted to an INTEGER by column affinity
    // and the byte guard below would never be what decides its fate - the
    // fixture would look like it was testing the guard while testing nothing.
    let stored_types: Vec<String> = {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare("SELECT typeof(shot_id) FROM resolved_warnings ORDER BY id")
            .unwrap();
        let types = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        types
    };
    assert_eq!(
        stored_types,
        vec!["integer", "integer", "blob", "text", "text"],
        "the fixture's cell types must be the ones under test"
    );

    let state = AppState::new().unwrap();
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open failed: {:?}", opened.error);

    let saved = state.save_project(Some(path.clone()));
    assert!(
        saved.success,
        "one nonsense row must not make the project unsaveable: {:?}",
        saved.error
    );

    let rows = Database::open_existing(&path)
        .unwrap()
        .load_resolved_warnings()
        .unwrap();
    assert!(
        rows.iter().any(|r| r.warning_code == IGNORED_CODE),
        "the readable ignore must survive alongside the nonsense one"
    );
    assert_eq!(
        rows.len(),
        2,
        "the two rows that name a shot are carried; none of the three that name none is"
    );
}

/// The cap has to bound the WORK, not just the output. Counted over rows KEPT it
/// would never trip on a table of pure junk - nothing is kept - so every save and
/// every autosave would scan the whole thing.
///
/// The observable is deliberate: the usable rows sit BEHIND the wall of junk, so
/// a reader that stops at the cap never reaches them and carries nothing, while
/// one that scans the whole table carries them. That difference is the presence
/// or absence of the bound.
///
/// Red before the fix: count the cap over `resolved.len()` instead of rows
/// examined, and the reader walks past the junk to the usable rows at the end -
/// the `is_empty` assertion fails.
#[test]
fn the_row_cap_bounds_the_scan_even_when_no_row_is_usable() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("flooded.stalx");
    let seeded = seed_current_file(&path);

    // Build the wall with SQLite itself: a recursive CTE inserts the rows far
    // faster than round-tripping them, and keeps the fixture synthetic.
    let junk = crate::persistence::MAX_CARRIED_RESOLVED_WARNINGS;
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "DROP TABLE resolved_warnings;
         CREATE TABLE resolved_warnings (
            id INTEGER PRIMARY KEY,
            section_id INTEGER NOT NULL,
            shot_id,
            warning_code TEXT NOT NULL,
            resolved_at TEXT NOT NULL,
            resolved_comment TEXT,
            FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
         );
         WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < {junk})
         INSERT INTO resolved_warnings (section_id, shot_id, warning_code, resolved_at)
            SELECT {}, X'FFFE', 'JUNK_' || n, '{RESOLVED_AT}' FROM seq;
         INSERT INTO resolved_warnings (section_id, shot_id, warning_code, resolved_at)
            VALUES ({}, {}, '{IGNORED_CODE}', '{RESOLVED_AT}');",
        seeded.section_id, seeded.section_id, seeded.shot_id
    ))
    .unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM resolved_warnings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        total,
        junk as i64 + 1,
        "the fixture must hold one usable row behind exactly the cap's worth of junk"
    );
    drop(conn);

    let state = AppState::new().unwrap();
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open failed: {:?}", opened.error);

    let saved = state.save_project(Some(path.clone()));
    assert!(
        saved.success,
        "a flooded table must still save, not refuse: {:?}",
        saved.error
    );

    let rows = Database::open_existing(&path)
        .unwrap()
        .load_resolved_warnings()
        .unwrap();
    assert!(
        rows.is_empty(),
        "the read must stop at the cap, so nothing behind the junk is reached (got {} rows)",
        rows.len()
    );
}

/// A table this build cannot make sense of AT ALL - here one whose rows do not
/// say which warning was ignored. The save rebuilds the file's ignore list from
/// what it reads, so guessing "empty" would destroy the rows on disk; it
/// refuses instead, and the file is left as it was.
///
/// Red before the fix: restore the `Vec::new()` arm and the save reports
/// success, failing both the `!success` assertion and (because the rebuilt file
/// carries no ignores) the "the file still holds its row" one.
#[test]
fn a_save_is_refused_when_the_ignore_list_cannot_be_read_at_all() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("unreadable.stalx");
    let seeded = seed_current_file(&path);
    forge_ignore_table(
        &path,
        UNREADABLE_COLUMNS,
        "section_id, shot_id, resolved_at",
        &[format!(
            "{}, {}, '{RESOLVED_AT}'",
            seeded.section_id, seeded.shot_id
        )],
    );
    let has_code_column: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE \
             name='warning_code'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        has_code_column, 0,
        "the fixture must be missing the column that makes a row readable"
    );

    let state = AppState::new().unwrap();
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open failed: {:?}", opened.error);

    let saved = state.save_project(Some(path.clone()));
    assert!(
        !saved.success,
        "a save that cannot read the ignore list it is about to rewrite must be refused"
    );
    let error = saved.error.unwrap_or_default();
    // The code is what keeps the refusal out of the Review & Repair route,
    // which cannot fix this (`surfaceSaveFailure`).
    assert!(
        saved.code == Some(CommandErrorCode::IgnoresUnreadable)
            && !error.contains("IGNORES_UNREADABLE"),
        "the refusal must carry its routing code, and no marker in its text, got: {error}"
    );
    // ...and it must be the STRUCTURAL refusal, naming the missing column - not
    // a row-level complaint, which no longer refuses anything.
    assert!(
        error.contains("no warning_code column"),
        "the refusal must say what is structurally wrong, got: {error}"
    );

    // The refusal lands before the temp file is created, so the destination is
    // untouched: the row is still there, and no debris was left beside it.
    assert_eq!(
        raw_row_count(&path),
        1,
        "the refused save must leave the old rows in place"
    );
    assert!(
        !path.with_extension("stalx.tmp").exists(),
        "a refused save must not leave a temp file behind"
    );
}

/// The probe itself failing is the other structural arm, and it is not
/// hypothetical: a second connection holding the file blocks the read outright.
/// The save must refuse, not carry an empty list - "I could not look" is not
/// "there is nothing there", however briefly it is true.
///
/// Red before the fix: restore the `Vec::new()` arm and the save succeeds,
/// rebuilding the file with no ignores because another process happened to hold
/// the database for a moment.
#[test]
fn a_save_refuses_while_the_ignore_list_cannot_be_reached() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("locked.stalx");
    let seeded = seed_current_file(&path);
    forge_ignore_table(
        &path,
        PRE_V8_COLUMNS,
        "section_id, shot_id, warning_code, resolved_at",
        &[format!(
            "{}, {}, '{IGNORED_CODE}', '{RESOLVED_AT}'",
            seeded.section_id, seeded.shot_id
        )],
    );

    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);

    // A rollback-journal database (this build sets no WAL) refuses every other
    // connection's read while one holds EXCLUSIVE, and no busy timeout is
    // installed - so this is deterministic, not a race.
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let saved = state.save_project(Some(path.clone()));
    blocker.execute_batch("ROLLBACK;").unwrap();
    drop(blocker);

    assert!(
        !saved.success,
        "a save that cannot reach the ignore list must be refused, not carried empty"
    );
    let error = saved.error.unwrap_or_default();
    assert!(
        saved.code == Some(CommandErrorCode::IgnoresUnreadable)
            && !error.contains("IGNORES_UNREADABLE"),
        "the refusal must carry its routing code, and no marker in its text, got: {error}"
    );
    assert_eq!(
        raw_row_count(&path),
        1,
        "the refused save must leave the old rows in place"
    );
}

/// The other half of the distinction: genuinely EMPTY still saves clean. Two
/// spellings of empty - a table with no rows, and (the pre-#57 file) no table
/// at all - because a fix that answered "cannot tell" to both would refuse
/// every save of every legacy project.
///
/// Red if `load_resolved_warnings_verbatim` reported a missing table as an
/// error: the second save would be refused.
#[test]
fn an_absent_or_empty_ignore_list_still_saves_clean() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("empty.stalx");
    seed_current_file(&path);

    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    let saved = state.save_project(Some(path.clone()));
    assert!(
        saved.success,
        "an empty ignore list must save: {:?}",
        saved.error
    );
    assert!(Database::open_existing(&path)
        .unwrap()
        .load_resolved_warnings()
        .unwrap()
        .is_empty());
    state.close_project();

    // Now the pre-#57 shape: no resolved_warnings table at all. The save above
    // left the stamp current, so no migration recreates it before the next save
    // reads it.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE resolved_warnings;")
        .unwrap();

    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    // Re-assert AFTER the open: if some path recreated the table on the way in,
    // the save below would be testing the ordinary empty case again and this
    // half would pass vacuously.
    let table_gone: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='resolved_warnings'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        table_gone, 0,
        "the open must not have recreated the ignore table"
    );

    let saved = state.save_project(Some(path));
    assert!(
        saved.success,
        "a file with no ignore table must save: {:?}",
        saved.error
    );
}

/// The #516 route, end to end: an UNSTAMPED pre-v8 file on write-protected
/// media opens un-migrated by design (running the chain would need a write it
/// cannot make), so its `resolved_warnings` still has no `shot_uuid` on the
/// very handle a Save As then reads through.
///
/// Before this fix that Save As silently produced a copy with zero ignores -
/// stated as the accepted cost in `ensure_schema_version`'s comment. It is not
/// the cost any more, and this is what says so.
///
/// Red before the fix: with the strict loader, the read fails and the copy is
/// written with no ignores (or the save is refused) - either way the
/// destination does not come back holding the row.
#[test]
#[cfg(unix)]
fn an_unwritable_unstamped_pre_v8_file_keeps_its_ignores_through_a_save_as() {
    use std::os::unix::fs::PermissionsExt;

    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("readonly.stalx");
    let seeded = seed_current_file(&source);
    forge_ignore_table(
        &source,
        PRE_V8_COLUMNS,
        "section_id, shot_id, warning_code, resolved_at",
        &[format!(
            "{}, {}, '{IGNORED_CODE}', '{RESOLVED_AT}'",
            seeded.section_id, seeded.shot_id
        )],
    );
    // UNSTAMPED, unlike the forged fixtures: this is the file whose project row
    // is the only thing claiming the current version, which is exactly what
    // `ensure_schema_version`'s read-only bypass keys on.
    rusqlite::Connection::open(&source)
        .unwrap()
        .execute_batch("PRAGMA user_version = 0;")
        .unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o444)).unwrap();

    // Mode bits are advice, not a guarantee: root ignores them. Confirm the
    // protection took before asserting anything about it (the #485 pattern),
    // and fail when it did not - run as root this test could not mean anything
    // (#576 NF-ROOT-PERM; CI runs the Rust tests unprivileged).
    crate::models::test_support::assert_file_refuses_writes(&source);

    let state = AppState::new().unwrap();
    let opened = state.open_project(source.clone());
    assert!(
        opened.success,
        "a write-protected project must still open: {:?}",
        opened.error
    );
    // The migration really did not run - if it had, this would be testing the
    // ordinary migrated path.
    let still_pre_v8: i64 = rusqlite::Connection::open(&source)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still_pre_v8, 0, "nothing was written to a read-only file");

    let dest = temp.path().join("copy.stalx");
    let saved = state.save_project(Some(dest.clone()));
    // Restore before asserting so a failure cannot leave TempDir unable to clean up.
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        saved.success,
        "Save As off write-protected media must work: {:?}",
        saved.error
    );

    let rows = Database::open_existing(&dest)
        .unwrap()
        .load_resolved_warnings()
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the ignore must ride along to the writable copy"
    );
    assert_eq!(rows[0].warning_code, IGNORED_CODE);
    assert_eq!(
        rows[0].shot_uuid.as_deref(),
        Some(seeded.shot_uuid.as_str()),
        "and the copy, which IS writable, gets it UUID-keyed"
    );
}
