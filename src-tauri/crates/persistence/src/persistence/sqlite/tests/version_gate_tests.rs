//! Opening files of other schema versions: the pre-release refusal, unstamped and future stamps,
//! and the v14 offset migration.
//!
//! Lines 8-555 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

/// [#576 B1-D1] A file from before the first public release is REFUSED, with a
/// message that says what to do, and left exactly as it was.
///
/// The first release (v0.0.3) already shipped schema 2 and stamped
/// `PRAGMA user_version`, so the only files whose stamp and project row are both
/// below 2 are pre-release test files (or hand-edited ones). They used to go
/// through a frozen "rebuild with foreign keys" step that had fallen four
/// `sections` columns behind, dropped `shots.uuid`, `loop_closures.is_aligned`
/// and `resolved_warnings.shot_uuid`, left `station_comments` pointing at a
/// dropped `sections_old`, and then stamped the file current. docs/saving.md
/// already said such files have no migration path; now the code agrees.
#[test]
fn a_pre_release_file_is_refused_with_a_reimport_message_and_left_untouched() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pre_release.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Ancient".to_string(), None))
            .unwrap();
        // What a schema-1 build left behind: a project row saying 1 and no stamp
        // (schema-1 builds never wrote `user_version`).
        db.conn
            .execute_batch("UPDATE project SET schema_version = 1; PRAGMA user_version = 0;")
            .unwrap();
    }
    let before = std::fs::read(&path).unwrap();

    let refused = Database::open_existing(&path);
    let (message, typed) = match refused {
        Ok(_) => (String::from("<opened>"), None),
        Err(e) => (
            format!("{e:#}"),
            e.downcast_ref::<PreReleaseFile>().copied(),
        ),
    };
    println!("open_existing(stamp 0, row 1) -> {message}; typed: {typed:?}");
    // [#576 C4] Its own error TYPE, which the load path turns into the
    // PRE_RELEASE_FILE code - no longer a prefix on the message.
    assert_eq!(
        typed,
        Some(PreReleaseFile { version: 1 }),
        "a pre-release file must be refused with its own error, got: {message}"
    );
    assert!(
        !message.contains("PRE_RELEASE_FILE"),
        "the message is display text, with no code prefix: {message}"
    );
    assert!(
        message.contains("import it again"),
        "the refusal must tell the user to re-import from the survey source, got: {message}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a refused file must be left byte-for-byte as it was"
    );
}

/// [#576 B1-D1] The same refusal on the other open path: `Database::open` reaches
/// the migration chain for an existing file too, and must not migrate a
/// pre-release project either (here stamped 1 as well, a stamp no build wrote).
#[test]
fn a_pre_release_file_is_refused_by_database_open_too() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("stamped_one.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Ancient".to_string(), None))
            .unwrap();
        db.conn
            .execute_batch("UPDATE project SET schema_version = 1; PRAGMA user_version = 1;")
            .unwrap();
    }

    let (message, typed) = match Database::open(&path) {
        Ok(_) => (String::from("<opened>"), None),
        Err(e) => (
            format!("{e:#}"),
            e.downcast_ref::<PreReleaseFile>().copied(),
        ),
    };
    println!("Database::open(stamp 1, row 1) -> {message}; typed: {typed:?}");
    assert_eq!(
        typed,
        Some(PreReleaseFile { version: 1 }),
        "a pre-release project must be refused on this path too, got: {message}"
    );
}

/// [#576 B1-D1] What the refusal must NOT catch: an existing EMPTY file. Android's
/// document picker creates a 0-byte placeholder at the chosen path before the app
/// writes it (`next_available_project_path` treats such a file as free), so
/// `Database::open` meets a file that exists, carries no stamp and has no project
/// row - a new project, not an old one.
#[test]
fn an_empty_placeholder_file_still_becomes_a_project() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("placeholder.stalx");
    std::fs::write(&path, b"").unwrap();

    let mut db = Database::open(&path).expect("an empty placeholder must open as a new project");
    db.save_project(&Project::new("CaveA".to_string(), None))
        .unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(db.load_project().unwrap().name, "CaveA");
    // [#576 B6-D2] ...and it is not an upgrade: no pre-upgrade copy of the empty file.
    let names: Vec<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n.contains(".backup")),
        "no copy of an empty placeholder: {names:?}"
    );
}

/// [#485] `project.schema_version` may not certify that migrations have run.
///
/// It is an ordinary column: `save_project` stamps it with the writing build's
/// constant, and it travels with the file through every copy, push/pull and hand
/// edit. `PRAGMA user_version` is the only value this app writes exclusively at
/// the end of a completed migration chain. When `ensure_schema_version`'s
/// "already current" fast path was gated on the project row, a file with an
/// unstamped `user_version` skipped `migrate_schema` on EVERY connection that
/// ever opened it - and skipped the stamp too, so the next open repeated the
/// skip. The v8 `resolved_warnings.shot_uuid` ALTER is one of the columns left
/// missing, which is what makes the save path's carry-over read fail with
/// `no such column: shot_uuid` and drop every ignore.
#[test]
fn a_project_row_claiming_the_current_version_cannot_skip_the_migration_chain() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("unstamped.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Unstamped".to_string(), None))
            .unwrap();
        // Narrow `resolved_warnings` back to its pre-v8 shape, then clear the
        // migration stamp while leaving the project row's claim intact.
        db.conn
            .execute_batch(
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
                 PRAGMA user_version = 0;",
            )
            .unwrap();
        let claimed: i64 = db
            .conn
            .query_row("SELECT schema_version FROM project LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            claimed,
            i64::from(SCHEMA_VERSION),
            "precondition: the project row claims the current version"
        );
    }

    let db = Database::open_existing(&path).unwrap();

    let has_column: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        has_column, 1,
        "the v8 ALTER must run: an unstamped file has no proof its migrations happened"
    );
    // ...and the read the save path performs must now work at all.
    db.load_resolved_warnings()
        .expect("load_resolved_warnings must not fail with `no such column: shot_uuid`");
    // ...but the file is NOT certified on the way out. `current` came from the
    // row, so the arms above it were skipped on an unverified number: stamping
    // would permanently certify a file whose data may genuinely be pre-v12 with
    // a lying row, and would destroy the `user_version == 0` evidence that
    // something is off. The idempotent rescan on each open is the accepted cost.
    let stamped: i64 = db
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        stamped, 0,
        "a row-derived version must not earn the stamp that claims every arm ran"
    );
}

/// [#485] The same shape, one branch earlier: the future-version bail.
///
/// It also ran on the row-derived version, so an unstamped file whose project
/// row claims 99 returned having run neither `migrate_schema` nor the stamp -
/// permanently, since the next open repeats the decision. The row's word is
/// enough to skip the version-gated arms and to drop the history they exist to
/// sanitize; it is not enough to skip the idempotent column adds this build
/// needs to read the file at all.
#[test]
fn an_unstamped_file_whose_project_row_claims_the_future_still_gains_shot_uuid() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("from_the_future.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Future".to_string(), None))
            .unwrap();
        db.conn
            .execute_batch(
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
                 UPDATE project SET schema_version = 99;
                 PRAGMA user_version = 0;",
            )
            .unwrap();
    }

    let db = Database::open_existing(&path).expect("a file claiming the future still opens");

    let has_column: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        has_column, 1,
        "the idempotent column adds must run even when the row claims the future"
    );
    let stamped: i64 = db
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stamped, 0, "a claim from the future certifies nothing");
    let claimed: i64 = db
        .conn
        .query_row("SELECT schema_version FROM project LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        claimed, 99,
        "the claim itself must survive: the next open reads it to pick its arms"
    );
}

/// A REAL stamp above `SCHEMA_VERSION` is unchanged by #485: run nothing, drop
/// the history no arm has sanitized, return. Pinned because the branch above it
/// now falls through, and the two must not be collapsed.
#[test]
fn a_genuinely_stamped_future_file_still_runs_no_migration() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("stamped_future.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Future".to_string(), None))
            .unwrap();
        db.conn
            .execute_batch(
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
                 PRAGMA user_version = 99;",
            )
            .unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let has_column: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        has_column, 0,
        "a file this build has never heard of runs no migration on its own stamp"
    );
    let stamped: i64 = db
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stamped, 99, "and is not downgraded");
}

/// [#485] A `.stalx` on write-protected media must still OPEN.
///
/// `open_existing` asks for `SQLITE_OPEN_READ_WRITE`, but the unix VFS falls
/// back to read-only instead of failing the open, so the refusal only lands on
/// the first write - here the v8 `ALTER TABLE`, which returns SQLITE_READONLY
/// and propagates straight out of `open_existing`. This file class reached no
/// write at all before #485, so refusing it now would be a regression that costs
/// the user their project rather than an ignore.
#[test]
#[cfg(unix)]
fn an_unwritable_unstamped_file_still_opens_without_running_the_chain() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("readonly.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("ReadOnly".to_string(), None))
            .unwrap();
        db.conn
            .execute_batch(
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
                 PRAGMA user_version = 0;",
            )
            .unwrap();
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

    // The mode bits are advice, not a guarantee: root ignores them. Confirm the
    // protection took before asserting anything about it, and fail when it did
    // not - a skip would pass on nothing (#576 NF-ROOT-PERM; CI runs the Rust
    // tests unprivileged).
    crate::models::test_support::assert_file_refuses_writes(&path);

    let opened = Database::open_existing(&path);
    // Restore before asserting so a failure cannot leave TempDir unable to clean up.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let db = opened.expect("a write-protected project must still open");

    // Degraded, exactly as before #485: the column is still missing, because
    // nothing may be written here. That no longer costs the ignores on a Save
    // As - `load_resolved_warnings_verbatim` reads this shape (#517) - but the
    // schema itself stays as the read-only file holds it.
    let has_column: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_column, 0, "nothing was written to a read-only file");
}

/// [#485] A write-protected file whose row claims the FUTURE does NOT take the
/// read-only bypass.
///
/// The bypass exists so a file that reached no write before #485 still opens. A
/// future claim is not that file: its branch drops the undo history precisely
/// because no arm has sanitized those payloads, and bypassing that would hand
/// the load path pre-v17 entries that replay into a wiped section name, surveyor
/// and date. Master derived `current` from the row too and hit the same DELETE,
/// so failing here is master's behaviour, not a regression.
#[test]
#[cfg(unix)]
fn an_unwritable_file_claiming_the_future_is_refused_rather_than_left_unsanitized() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("readonly_future.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Future".to_string(), None))
            .unwrap();
        // A persisted undo entry is what the DELETE exists to remove, so the
        // write this test expects is a real one and not a no-op.
        db.save_undo_stack_with_timestamps(vec![(
            r#"{"type":"close_project"}"#.to_string(),
            r#"{"InverseCommand":{"type":"close_project"}}"#.to_string(),
            None,
        )])
        .unwrap();
        db.conn
            .execute_batch("UPDATE project SET schema_version = 99; PRAGMA user_version = 0;")
            .unwrap();
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    crate::models::test_support::assert_file_refuses_writes(&path);

    let opened = Database::open_existing(&path);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        opened.is_err(),
        "a future-claiming file must never be opened with its unsanitized history intact"
    );
}

/// [#485] Withholding the stamp may not re-arm a one-shot migration.
///
/// An unstamped file whose row says 12 re-derives 12 on every open and re-enters
/// the `current < 13` arm, where `migrate_is_reference_to_kind` unconditionally
/// re-inserts `pending_kind_classification`. The load path honours that flag
/// ONCE, by running the classifier over the whole project and writing its
/// verdicts through - so re-arming it silently overwrites the user's own
/// Jump/Reference choices on every open. Master stamped these files; so do we.
#[test]
fn an_unstamped_file_claiming_a_mid_range_version_is_stamped_and_does_not_re_arm() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("row_says_12.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Mid".to_string(), None))
            .unwrap();
        db.conn
            .execute_batch(
                "DELETE FROM project_settings WHERE key = 'pending_kind_classification';
                 UPDATE project SET schema_version = 12;
                 PRAGMA user_version = 0;",
            )
            .unwrap();
    }

    {
        let db = Database::open_existing(&path).unwrap();
        let stamped: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            stamped,
            i64::from(SCHEMA_VERSION),
            "a row-derived version BELOW SCHEMA_VERSION must still be stamped once its arms run"
        );
        assert!(
            db.is_kind_classification_pending(),
            "precondition: the v12->v13 arm really did run and arm the one-shot classifier"
        );
        // What the load path does with it, exactly once.
        db.clear_pending_kind_classification().unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert!(
        !db.is_kind_classification_pending(),
        "the second open must take the fast path, not re-arm the classifier and overwrite the \
         user's Jump/Reference choices"
    );
}

/// The v14 azimuth migration must leave `section_offsets` exactly as it found them.
///
/// An offset is an accumulated translation from past head insert/remove edits, not a function
/// of the current shots, so one banked under the old antipodal bearing cannot be recomputed -
/// only replaying the edit history would do it. Zeroing them instead would move every section
/// that has one, including the overwhelming majority whose azimuths were always in range. The
/// decision is to correct the readings and leave the translation; this pins it so a later
/// "tidy-up" cannot quietly start rewriting placements.
#[test]
fn migration_v14_leaves_section_offsets_untouched() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v13.stalx");

    {
        let db = Database::open(&path).unwrap();
        db.conn
            .execute(
                "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2020-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        // An out-of-range PAIR - the shape the migration exists for.
        db.conn
            .execute(
                "INSERT INTO shots (id, section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out) \
                 VALUES (1, 0, 0, 0.0, -600.0, 10.0, 0.0, 0.0)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO section_offsets (section_id, offset_x, offset_y) VALUES (0, 12.5, -7.25)",
                [],
            )
            .unwrap();
        // Back-date so the migration actually runs on the next open.
        db.conn.execute("PRAGMA user_version = 13", []).unwrap();
    }

    let db = Database::open(&path).unwrap();

    let (x, y): (f64, f64) = db
        .conn
        .query_row(
            "SELECT offset_x, offset_y FROM section_offsets WHERE section_id = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (x, y),
        (12.5, -7.25),
        "the accumulated translation must survive the migration verbatim"
    );

    // ...while the readings it exists for WERE corrected, so this is not a vacuous assertion.
    let (az_in, az_out): (f64, f64) = db
        .conn
        .query_row(
            "SELECT azimuth_in, azimuth_out FROM shots WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(az_in, 0.0);
    assert_eq!(az_out, 120.0, "-600 normalises into [0, 360)");
}
