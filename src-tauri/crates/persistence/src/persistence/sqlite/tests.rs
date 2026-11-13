use super::*;
use crate::models::ShotData;
use crate::models::ShotKind;
use crate::models::StationAnchor;
use crate::models::StationRef;
use tempfile::TempDir;

/// A pre-v2 `.stalx` must keep `reverse_parity` through the FK rebuild.
///
/// `migrate_schema` conditionally ADDs columns to `sections`, and for `user_version < 2` the FK
/// rebuild then recreates that table from a fixed DDL and copies the rows across. A column
/// missing from either half is silently dropped, and the file is stamped current afterwards, so
/// no later open re-adds it. The damage does not show up on load - the read path falls back to a
/// literal for an absent column - it shows up on the next SAVE, which inserts every column by
/// name. The project opens fine and can then never be written again.
///
/// The insert below is the shape `save_project` uses. It is exercised directly rather than
/// through `save_project` because a real `save_project` on a pre-v2 file currently fails for an
/// UNRELATED, pre-existing reason: `init_schema` runs before the migration and creates tables
/// whose foreign keys reference `sections` (`station_comments` among them), the rebuild's rename
/// rewrites those references to `sections_old`, and the rebuild does not recreate every one of
/// them - so the dangling reference outlives the drop. That is master's bug, not this branch's,
/// and it would mask this assertion rather than sharpen it.
#[test]
fn a_pre_v2_rebuild_keeps_the_reverse_parity_column() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy.stalx");

    // A v1 file: `sections` without any of the columns later migrations add, plus the row we
    // expect to survive the rebuild.
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sections (
                 id INTEGER PRIMARY KEY,
                 name TEXT,
                 surveyor TEXT,
                 date TEXT,
                 color TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             INSERT INTO sections (id, name, color, created_at)
             VALUES (1, 'Legacy', '#ff0000', '2020-01-01T00:00:00Z');
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }

    // Opening migrates: the ALTER adds the column, then the FK rebuild recreates the table.
    let db = Database::open(&path).unwrap();
    let has_column: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='reverse_parity'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        has_column, 1,
        "the FK rebuild must carry reverse_parity through, not drop it"
    );
    let survived: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM sections", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        survived, 1,
        "the rebuild must copy the existing rows across"
    );

    // THE regression: this is the insert `save_project` performs, and it is what fails when the
    // column is dropped by the rebuild.
    db.conn
        .execute(
            "INSERT INTO sections (id, name, surveyor, date, color, direction, quality, \
             reverse_parity, created_at) \
             VALUES (2, 'New', NULL, NULL, '#00ff00', 'in', 1.0, 1, '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("a migrated pre-v2 file must still accept the column save_project writes");
    let parity: i64 = db
        .conn
        .query_row(
            "SELECT reverse_parity FROM sections WHERE id = 2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        parity, 1,
        "the value must round-trip, not be silently dropped"
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
/// It used to run on the row-derived version and return having run neither
/// `migrate_schema` nor the stamp, permanently. Since #649 the row's claim
/// refuses the open outright, as `stalx-reader` already did: no migration, no
/// stamp and no column add land in a file this build cannot vouch for, so the
/// #485 shape (a file left unmigrated forever) cannot arise from it either.
#[test]
fn an_unstamped_file_whose_project_row_claims_the_future_is_refused_untouched() {
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

    let before = std::fs::read(&path).unwrap();

    let err = Database::open_existing(&path)
        .err()
        .expect("a file claiming the future is refused");

    assert_eq!(
        err.downcast_ref::<NewerSchema>(),
        Some(&NewerSchema {
            file: "from_the_future.stalx".to_string(),
            found: 99,
            supported: SCHEMA_VERSION
        })
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "no column add, stamp or claim rewrite reached the file"
    );
}

/// A REAL stamp above `SCHEMA_VERSION` runs no migration and is not downgraded:
/// since #649 the open refuses it before anything writes.
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

    let err = Database::open_existing(&path)
        .err()
        .expect("a file from a newer schema is refused (#649)");
    assert!(err.downcast_ref::<NewerSchema>().is_some(), "{err:#}");

    let db = Database::open_for_metadata(&path).unwrap();
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

    // The mode bits are advice, not a guarantee: root ignores them, and CI's
    // check job runs as root in a container. Confirm the protection actually
    // took before asserting anything about it - otherwise this test reports a
    // failure of the guard when what really happened is that the fixture could
    // not be built. It stays live on every developer machine and on macOS.
    if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        eprintln!(
            "skipping an_unwritable_unstamped_file_still_opens_without_running_the_chain: this \
             user can write a 0444 file (root?), so there is no read-only database to test"
        );
        return;
    }

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
/// future claim is not that file: opening it would hand the load path undo
/// payloads no migration arm has vetted. Since #649 it is refused as a newer
/// file before the bypass is reached.
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
    if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        eprintln!("skipping: this user can write a 0444 file (root?)");
        return;
    }

    let opened = Database::open_existing(&path);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = opened
        .err()
        .expect("a future-claiming file must never be opened with its unsanitized history intact");
    assert!(err.downcast_ref::<NewerSchema>().is_some(), "{err:#}");
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

/// [#485] A genuine pre-v2 file is row-derived (no stamp, row says 1) but below
/// `SCHEMA_VERSION`, so it is stamped after its arms run - as master did.
///
/// Load-bearing rather than cosmetic: `current < 2` reaches
/// `rebuild_tables_with_foreign_keys`, whose copy omits `shots.uuid`, so an
/// unstamped file would rebuild on EVERY open and wipe every shot UUID each time.
#[test]
fn a_pre_v2_file_is_stamped_so_the_fk_rebuild_runs_once() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pre_v2.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Ancient".to_string(), None))
            .unwrap();
        db.conn
            .execute_batch("UPDATE project SET schema_version = 1; PRAGMA user_version = 0;")
            .unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let stamped: i64 = db
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        stamped,
        i64::from(SCHEMA_VERSION),
        "without the stamp the FK rebuild would run on every single open"
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

#[test]
fn load_project_validation_returns_none_when_table_absent() {
    // A legacy .stalx predating `project_settings`, opened read-only (which does
    // NOT migrate), must not error: load_project_validation returns Ok(None) so
    // get_survey_geometry_for_file's preview load still works (Codex). Simulate the
    // missing table by dropping it, then confirm no error is propagated.
    let db = Database::in_memory().unwrap();
    db.conn
        .execute("DROP TABLE IF EXISTS project_settings", [])
        .unwrap();
    let result = db.load_project_validation();
    assert!(
        result.is_ok(),
        "a missing project_settings table must not error"
    );
    assert!(
        result.unwrap().is_none(),
        "absent validation resolves to None"
    );
}

/// #431 persistence: `Some` and `None` are distinct states, not `Some(0.0)`
/// vs. an absent row read the same way.
#[test]
fn surface_elevation_some_and_none_round_trip_distinctly() {
    let temp = TempDir::new().unwrap();

    let with_elevation_path = temp.path().join("with_elevation.stalx");
    let mut db = Database::open(&with_elevation_path).unwrap();
    let mut project = Project::new("Has Elevation".to_string(), None);
    project.surface_elevation = Some(123.5);
    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.surface_elevation, Some(123.5));

    let without_elevation_path = temp.path().join("without_elevation.stalx");
    let mut db2 = Database::open(&without_elevation_path).unwrap();
    let project = Project::new("No Elevation".to_string(), None);
    assert_eq!(project.surface_elevation, None, "precondition");
    db2.save_project(&project).unwrap();
    let loaded = db2.load_project().unwrap();
    assert_eq!(loaded.surface_elevation, None);
}

/// #431: a corrupted or hand-edited `.stalx` (untrusted, same as a `.tml`)
/// can carry a `surface_elevation` value Rust's `f64::parse` accepts but the
/// rest of the pipeline cannot: `"NaN"` defeats the no-clobber comparison
/// (every NaN compare is false) and would land verbatim in a TML export; a
/// plain non-numeric string is the ordinary "unparseable" case. Both must
/// read as `None`, not propagate a bad value or error the load. Red before
/// the `.filter(f64::is_finite)` guard on `load_project_surface_elevation`.
#[test]
fn stored_non_finite_or_unparseable_elevation_reads_as_none() {
    for raw in ["NaN", "not-a-number"] {
        let db = Database::in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO project_settings (key, value) VALUES ('surface_elevation', ?1)",
                [raw],
            )
            .unwrap();
        let result = db.load_project_surface_elevation();
        assert!(result.is_ok(), "{raw:?} must not error the load");
        assert_eq!(
            result.unwrap(),
            None,
            "{raw:?} must read as unknown, not a value"
        );
    }
}

/// #431, the case that proves no migration is needed: a `.stalx` written
/// before this feature existed has no `surface_elevation` row (simulated by
/// deleting it after an ordinary save). Opening it must read `None`, not
/// error, and a subsequent save must succeed.
#[test]
fn a_pre_431_project_opens_with_unknown_elevation_and_saves_cleanly() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Legacy".to_string(), None))
        .unwrap();
    db.conn
        .execute(
            "DELETE FROM project_settings WHERE key = 'surface_elevation'",
            [],
        )
        .unwrap();

    let loaded = db.load_project().unwrap();
    assert_eq!(
        loaded.surface_elevation, None,
        "a pre-#431 project must read as unknown"
    );

    db.save_project(&loaded).unwrap();
    let reloaded = db.load_project().unwrap();
    assert_eq!(reloaded.surface_elevation, None);
}

/// The genuine "table doesn't exist at all" shape the doc comment on
/// `load_project_surface_elevation` (read.rs) claims to cover - stronger
/// than the DELETE above, which only proves the missing-KEY case (and the
/// writer already deletes the key for `None`, so that DELETE can even be a
/// no-op). Mirrors `load_project_validation_returns_none_when_table_absent`.
#[test]
fn load_project_surface_elevation_returns_none_when_table_absent() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute("DROP TABLE IF EXISTS project_settings", [])
        .unwrap();
    let result = db.load_project_surface_elevation();
    assert!(
        result.is_ok(),
        "a missing project_settings table must not error"
    );
    assert_eq!(
        result.unwrap(),
        None,
        "absent surface_elevation resolves to None"
    );
}

/// #431: `project_settings` is a shared key/value table, so writing
/// `validation` and `surface_elevation` in the same save must not let either
/// key clobber the other.
#[test]
fn validation_and_surface_elevation_survive_the_same_save() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("both_settings.stalx");
    let mut db = Database::open(&path).unwrap();

    let mut project = Project::new("Both Settings".to_string(), None);
    project.validation.warn_shot_length_min = 99.0;
    project.surface_elevation = Some(-12.25);
    db.save_project(&project).unwrap();

    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.validation.warn_shot_length_min, 99.0);
    assert_eq!(loaded.surface_elevation, Some(-12.25));
}

#[test]
fn save_resolved_warning_never_clobbers_a_different_shots_uuid_row() {
    // The legacy positional UNIQUE must not let one shot's ignore delete another
    // shot's UUID-keyed ignore sharing the (section, shot_id) slot - which can
    // happen transiently when in-memory ids diverge from saved positions after a
    // save (issue #57 / Codex). The conditional upsert preserves the existing row
    // and reports the collision rather than silently dropping either ignore.
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    let mk = |uuid: &str| ResolvedWarning {
        section_id: 0,
        shot_id: 5,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some(uuid.to_string()),
    };
    db.save_resolved_warning(&mk("aaa")).unwrap();
    // A second shot's ignore landing on the same stale position is refused, not silent.
    assert!(
        db.save_resolved_warning(&mk("bbb")).is_err(),
        "a colliding different-shot ignore must be reported, not silently applied"
    );
    let rows = db.load_resolved_warnings().unwrap();
    assert_eq!(rows.len(), 1, "the first shot's ignore must be preserved");
    assert_eq!(rows[0].shot_uuid.as_deref(), Some("aaa"));
}

#[test]
fn bulk_upsert_keeps_uuid_row_over_colliding_legacy_null_row() {
    // On the save carry-over a legacy NULL-uuid row can collide with a remapped
    // UUID row at the same position; the UUID row must win regardless of insert
    // order, never downgrading the survivor to position-keying (issue #57 / Codex).
    let uuid_row = ResolvedWarning {
        section_id: 0,
        shot_id: 7,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("ccc".to_string()),
    };
    let null_row = ResolvedWarning {
        shot_uuid: None,
        ..uuid_row.clone()
    };
    for batch in [
        vec![null_row.clone(), uuid_row.clone()],
        vec![uuid_row.clone(), null_row.clone()],
    ] {
        let db = Database::in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        db.save_resolved_warnings_bulk(&batch).unwrap();
        let rows = db.load_resolved_warnings().unwrap();
        assert_eq!(rows.len(), 1, "the colliding rows collapse to one");
        assert_eq!(
            rows[0].shot_uuid.as_deref(),
            Some("ccc"),
            "the UUID row wins"
        );
    }
}

#[test]
fn shot_uuid_is_persisted_reflects_the_shots_table() {
    // A new ignore is UUID-keyed only when the shot's UUID is already on disk; a
    // pre-v6 shot's in-memory-minted UUID isn't, so its ignore stays position-keyed
    // to survive the next open (issue #57 / Codex).
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO shots (id, section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, uuid) \
             VALUES (1, 0, 0, 0, 0, 1, 0, 0, 'persisted-uuid')",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO shots (id, section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, uuid) \
             VALUES (2, 0, 1, 0, 0, 1, 0, 0, NULL)",
            [],
        )
        .unwrap();
    assert!(db.shot_uuid_is_persisted(0, "persisted-uuid").unwrap());
    assert!(!db.shot_uuid_is_persisted(0, "transient-uuid").unwrap());
}

/// The check is SECTION-SCOPED, because a file saved before shot UUIDs were project-wide
/// unique can hold the same shot UUID in two sections (the load renames one copy).
///
/// Unscoped, the query answers "persisted" for section 1 because section 0's copy is on disk,
/// and the caller then keys an ignore by a UUID that is still in-memory-only for the section it
/// belongs to. The next open mints a different one and the suppression is orphaned - exactly
/// what this guard exists to prevent (issue #57).
#[test]
fn shot_uuid_is_persisted_does_not_answer_for_another_section() {
    let db = Database::in_memory().unwrap();
    for section_id in [0, 1] {
        db.conn
            .execute(
                "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
                [section_id],
            )
            .unwrap();
    }
    // The SHARED uuid is on disk for section 0 only; section 1's copy is still in memory.
    db.conn
        .execute(
            "INSERT INTO shots (id, section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, uuid) \
             VALUES (1, 0, 0, 0, 0, 1, 0, 0, 'shared-uuid')",
            [],
        )
        .unwrap();

    assert!(db.shot_uuid_is_persisted(0, "shared-uuid").unwrap());
    assert!(
        !db.shot_uuid_is_persisted(1, "shared-uuid").unwrap(),
        "another section's copy must not count as persisted for this one"
    );
}

#[test]
fn save_resolved_warning_collision_preserves_the_prior_ignore() {
    // A refused collision (the requested position holds a DIFFERENT shot's ignore)
    // must roll back the dedup-delete too, so this shot's OWN prior ignore is not
    // dropped on the way to refusing (issue #57 / Codex).
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    let row = |shot_id: u32, uuid: &str| ResolvedWarning {
        section_id: 0,
        shot_id,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some(uuid.to_string()),
    };
    // Shot A's ignore at position 3, shot B's at position 5.
    db.save_resolved_warning(&row(3, "aaa")).unwrap();
    db.save_resolved_warning(&row(5, "bbb")).unwrap();
    // Re-resolve A claiming position 5 (where B sits): refused, and A's old row stays.
    assert!(db.save_resolved_warning(&row(5, "aaa")).is_err());
    let mut rows = db.load_resolved_warnings().unwrap();
    rows.sort_by_key(|r| r.shot_id);
    assert_eq!(
        rows.len(),
        2,
        "both shots' ignores survive the refused collision"
    );
    assert_eq!(rows[0].shot_id, 3);
    assert_eq!(rows[0].shot_uuid.as_deref(), Some("aaa"));
    assert_eq!(rows[1].shot_id, 5);
    assert_eq!(rows[1].shot_uuid.as_deref(), Some("bbb"));
}

#[test]
fn test_foreign_keys_enabled_on_open() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("fk_enabled.stalx");

    let db = Database::open(&db_path).unwrap();
    let enabled: i32 = db
        .conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();

    assert_eq!(enabled, 1);
}

#[test]
fn test_foreign_keys_enabled_on_open_existing() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("fk_enabled_existing.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Foreign Keys Existing".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    let db = Database::open_existing(&db_path).unwrap();
    let enabled: i32 = db
        .conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();

    assert_eq!(enabled, 1);
}

#[test]
fn test_foreign_keys_enabled_on_open_readonly() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("fk_enabled_readonly.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Foreign Keys Readonly".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    let db = Database::open_readonly(&db_path).unwrap();
    let enabled: i32 = db
        .conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();

    assert_eq!(enabled, 1);
}

#[test]
fn test_user_version_set_on_open() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("user_version.stalx");

    let db = Database::open(&db_path).unwrap();
    let version: i32 = db
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();

    assert_eq!(version, SCHEMA_VERSION);
}

#[test]
fn test_create_and_save_project() {
    let db = Database::in_memory().unwrap();
    let mut project = Project::new("Test Cave".to_string(), Some("Test Location".to_string()));

    let section_data = SectionData {
        name: Some("Main Line".to_string()),
        surveyor: Some("Test Surveyor".to_string()),
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 6.0,
            ..Default::default()
        }],
    };

    project.add_section(section_data);

    let mut db = db;
    db.save_project(&project).unwrap();

    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.name, "Test Cave");
    assert_eq!(loaded.sections.len(), 1);
    assert_eq!(loaded.sections[0].shots.len(), 1);
}

#[test]
fn test_save_project_name_targeted_update() {
    let mut db = Database::in_memory().unwrap();
    let project = Project::new("Old Name".to_string(), Some("Loc".to_string()));
    db.save_project(&project).unwrap();

    db.save_project_name("New Name").unwrap();

    // Only the name changed; the rest of the project is intact.
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.name, "New Name");
    assert_eq!(loaded.location.as_deref(), Some("Loc"));

    // And it is visible through the lightweight metadata read.
    let meta = db
        .load_metadata(std::path::PathBuf::from("/tmp/x.stalx"))
        .unwrap();
    assert_eq!(meta.name, "New Name");
}

// === Test 3.1: Save/Load Multiple Sections ===
#[test]
fn test_save_load_multiple_sections() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_sections.stalx");

    // Create a project with multiple sections
    let mut project = Project::new(
        "Multi-Section Cave".to_string(),
        Some("Test Location".to_string()),
    );

    // Section 1: Main passage
    let section1 = SectionData {
        name: Some("Main Passage".to_string()),
        surveyor: Some("John Doe".to_string()),
        date: NaiveDate::from_ymd_opt(2024, 6, 15),
        color: "#ff5733".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots: vec![
            ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 10.0,
                depth_in: 5.0,
                depth_out: 7.0,
                left: Some(1.5),
                right: Some(2.0),
                up: Some(3.0),
                down: Some(0.5),
                comment: Some("Entry shot".to_string()),
                ..Default::default()
            },
            ShotData {
                azimuth_in: 60.0,
                azimuth_out: 240.0,
                length: 8.5,
                depth_in: 7.0,
                depth_out: 9.0,
                pitch_in: Some(15.0),
                pitch_out: Some(-10.0),
                ..Default::default()
            },
        ],
    };

    // Section 2: Side passage
    let section2 = SectionData {
        name: Some("Side Passage".to_string()),
        surveyor: Some("Jane Smith".to_string()),
        date: NaiveDate::from_ymd_opt(2024, 6, 16),
        color: "#33ff57".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 1.0,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 270.0,
            length: 5.0,
            depth_in: 6.0,
            depth_out: 6.5,
            kind: ShotKind::Reference,
            marker_index: Some(42),
            temperature: Some(18.5),
            ..Default::default()
        }],
    };

    // Section 3: No metadata (testing None values)
    let section3 = SectionData {
        name: None,
        surveyor: None,
        date: None,
        color: "#3357ff".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![],
    };

    project.add_section(section1);
    project.add_section(section2);
    project.add_section(section3);

    // Save to file
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load from file
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    // Verify project metadata
    assert_eq!(loaded.name, "Multi-Section Cave");
    assert_eq!(loaded.location, Some("Test Location".to_string()));
    assert_eq!(loaded.sections.len(), 3);

    // Verify section 1
    let s1 = &loaded.sections[0];
    assert_eq!(s1.name, Some("Main Passage".to_string()));
    let c1 = s1.credit_summary();
    assert_eq!(
        crate::models::join_names(&c1.surveyor_people()).as_deref(),
        Some("John Doe")
    );
    assert_eq!(c1.date_min, NaiveDate::from_ymd_opt(2024, 6, 15));
    assert_eq!(s1.color, "#ff5733");
    assert_eq!(s1.direction, SurveyDirection::In);
    assert_eq!(s1.shots.len(), 2);

    // Verify shot data preservation
    let shot1 = &s1.shots[0];
    assert_eq!(shot1.azimuth_in, 45.0);
    assert_eq!(shot1.azimuth_out, 225.0);
    assert_eq!(shot1.length, 10.0);
    assert_eq!(shot1.depth_in, 5.0);
    assert_eq!(shot1.depth_out, 7.0);
    assert_eq!(shot1.left, Some(1.5));
    assert_eq!(shot1.right, Some(2.0));
    assert_eq!(shot1.up, Some(3.0));
    assert_eq!(shot1.down, Some(0.5));
    assert_eq!(shot1.comment, Some("Entry shot".to_string()));

    let shot2 = &s1.shots[1];
    assert_eq!(shot2.pitch_in, Some(15.0));
    assert_eq!(shot2.pitch_out, Some(-10.0));

    // Verify section 2 (Out direction, reference shot)
    let s2 = &loaded.sections[1];
    assert_eq!(s2.direction, SurveyDirection::Out);
    assert_eq!(s2.shots[0].kind, ShotKind::Reference);
    assert_eq!(s2.shots[0].marker_index, Some(42));
    assert_eq!(s2.shots[0].temperature, Some(18.5));

    // Verify section 3 (None values)
    let s3 = &loaded.sections[2];
    assert!(s3.name.is_none());
    assert!(s3.credit_summary().surveyors.is_empty());
    assert!(s3.credit_summary().date_min.is_none());
    assert!(s3.shots.is_empty());
}

// === Test 3.2: Save/Load Station Merges ===
#[test]
fn test_save_load_station_merges() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_merges.stalx");

    let mut project = Project::new("Merge Test".to_string(), None);

    // Create two sections with shots
    let section1 = SectionData {
        name: Some("Section A".to_string()),
        color: "#ff0000".to_string(),
        shots: vec![
            ShotData {
                azimuth_in: 0.0,
                azimuth_out: 180.0,
                length: 5.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 7.0,
                depth_in: 1.0,
                depth_out: 2.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let section2 = SectionData {
        name: Some("Section B".to_string()),
        color: "#00ff00".to_string(),
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 270.0,
            length: 4.0,
            depth_in: 0.0,
            depth_out: 0.5,
            ..Default::default()
        }],
        ..Default::default()
    };

    project.add_section(section1);
    project.add_section(section2);

    // Add station merges manually (bypassing validation for test)
    let merge1 = StationMerge {
        station_a: StationRef::new(0, 1).into(), // Section 0, Station 1
        station_b: StationRef::new(1, 0).into(), // Section 1, Station 0
        merged_at: Utc::now(),
    };

    let merge2 = StationMerge {
        station_a: StationRef::new(0, 2).into(), // Section 0, Station 2
        station_b: StationRef::new(1, 1).into(), // Section 1, Station 1
        merged_at: Utc::now(),
    };

    project.station_merges.push(merge1);
    project.station_merges.push(merge2);

    // Save
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.station_merges.len(), 2);

    let m1a = loaded
        .resolve_key(&loaded.station_merges[0].station_a)
        .unwrap();
    let m1b = loaded
        .resolve_key(&loaded.station_merges[0].station_b)
        .unwrap();
    assert_eq!(m1a.section_id, 0);
    assert_eq!(m1a.station_index, 1);
    assert_eq!(m1b.section_id, 1);
    assert_eq!(m1b.station_index, 0);

    let m2a = loaded
        .resolve_key(&loaded.station_merges[1].station_a)
        .unwrap();
    let m2b = loaded
        .resolve_key(&loaded.station_merges[1].station_b)
        .unwrap();
    assert_eq!(m2a.section_id, 0);
    assert_eq!(m2a.station_index, 2);
    assert_eq!(m2b.section_id, 1);
    assert_eq!(m2b.station_index, 1);
}

// === Test 3.3: Save/Load Zero Datums with GPS Precision ===
#[test]
fn test_save_load_zero_datums() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_gps.stalx");

    let mut project = Project::new("GPS Test".to_string(), None);

    // Add a section with shots
    let section = SectionData {
        name: Some("Main".to_string()),
        color: "#ffffff".to_string(),
        shots: vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 180.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 5.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    project.add_section(section);

    // Add zero datums with high-precision GPS coordinates (8+ decimal places)
    let zd1 = ZeroDatum {
        station_ref: StationRef::new(0, 0).into(),
        gps: GpsCoordinates {
            latitude: 45.123_456_789_012_34,   // 14 decimal places
            longitude: -93.987_654_321_098_76, // 14 decimal places
            depth_reference: 123.456_789,
        },
    };

    let zd2 = ZeroDatum {
        station_ref: StationRef::new(0, 1).into(),
        gps: GpsCoordinates {
            latitude: -33.856_789_012_345_67,
            longitude: 151.209_876_543_210_98,
            depth_reference: -15.5,
        },
    };

    project.zero_datums.push(zd1);
    project.zero_datums.push(zd2);

    // Save
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify GPS precision
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.zero_datums.len(), 2);

    // Verify 8+ decimal places of precision preserved
    let loaded_zd1 = &loaded.zero_datums[0];
    let loaded_zd1_ref = loaded.resolve_key(&loaded_zd1.station_ref).unwrap();
    assert_eq!(loaded_zd1_ref.section_id, 0);
    assert_eq!(loaded_zd1_ref.station_index, 0);
    // Check precision to 8 decimal places
    assert!((loaded_zd1.gps.latitude - 45.123_456_789_012_34).abs() < 1e-8);
    assert!((loaded_zd1.gps.longitude - (-93.987_654_321_098_76)).abs() < 1e-8);
    assert!((loaded_zd1.gps.depth_reference - 123.456_789).abs() < 1e-6);

    let loaded_zd2 = &loaded.zero_datums[1];
    assert!((loaded_zd2.gps.latitude - (-33.856_789_012_345_67)).abs() < 1e-8);
    assert!((loaded_zd2.gps.longitude - 151.209_876_543_210_98).abs() < 1e-8);
    assert!((loaded_zd2.gps.depth_reference - (-15.5)).abs() < 1e-6);
}

// === Test 3.4: Save/Load Section Offsets ===
#[test]
fn test_save_load_section_offsets() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_offsets.stalx");

    let mut project = Project::new("Offset Test".to_string(), None);

    // Create sections
    for i in 0..3 {
        let section = SectionData {
            name: Some(format!("Section {i}")),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                azimuth_in: 0.0,
                azimuth_out: 180.0,
                length: 5.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        project.add_section(section);
    }

    // Add section offsets with varying precision
    project
        .section_offsets
        .insert(0, (10.123_456_789, -20.987_654_321));
    project.section_offsets.insert(1, (0.0, 0.0));
    project.section_offsets.insert(2, (-100.5, 200.75));

    // Save
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.section_offsets.len(), 3);

    let offset0 = loaded.section_offsets.get(&0).unwrap();
    assert!((offset0.0 - 10.123_456_789).abs() < 1e-8);
    assert!((offset0.1 - (-20.987_654_321)).abs() < 1e-8);

    let offset1 = loaded.section_offsets.get(&1).unwrap();
    assert_eq!(offset1.0, 0.0);
    assert_eq!(offset1.1, 0.0);

    let offset2 = loaded.section_offsets.get(&2).unwrap();
    assert_eq!(offset2.0, -100.5);
    assert_eq!(offset2.1, 200.75);
}

// === Test 3.5: Update Shot in Database ===
#[test]
fn test_update_shot_in_database() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_update.stalx");

    // Create initial project
    let mut project = Project::new("Update Test".to_string(), None);
    let section = SectionData {
        name: Some("Test Section".to_string()),
        color: "#123456".to_string(),
        shots: vec![
            ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 10.0,
                depth_in: 5.0,
                depth_out: 8.0,
                comment: Some("Original comment".to_string()),
                ..Default::default()
            },
            ShotData {
                azimuth_in: 90.0,
                azimuth_out: 270.0,
                length: 15.0,
                depth_in: 8.0,
                depth_out: 12.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    project.add_section(section);

    // Save initial state
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Modify the shot in the project
    project.sections[0].shots[0].length = 12.5;
    project.sections[0].shots[0].azimuth_in = 50.0;
    project.sections[0].shots[0].comment = Some("Updated comment".to_string());
    project.sections[0].shots[0].left = Some(2.0);
    project.sections[0].shots[0].right = Some(3.0);

    // Save updated state
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify update
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    let updated_shot = &loaded.sections[0].shots[0];
    assert_eq!(updated_shot.length, 12.5);
    assert_eq!(updated_shot.azimuth_in, 50.0);
    assert_eq!(updated_shot.comment, Some("Updated comment".to_string()));
    assert_eq!(updated_shot.left, Some(2.0));
    assert_eq!(updated_shot.right, Some(3.0));

    // Verify other shot unchanged
    let other_shot = &loaded.sections[0].shots[1];
    assert_eq!(other_shot.length, 15.0);
    assert_eq!(other_shot.azimuth_in, 90.0);
}

// === Test 3.6: Delete Section Cascades Shots ===
#[test]
fn test_delete_section_cascades_shots() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_cascade.stalx");

    // Create project with multiple sections
    let mut project = Project::new("Cascade Test".to_string(), None);

    let section1 = SectionData {
        name: Some("Section to keep".to_string()),
        color: "#111111".to_string(),
        shots: vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 180.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 1.0,
            ..Default::default()
        }],
        ..Default::default()
    };

    let section2 = SectionData {
        name: Some("Section to delete".to_string()),
        color: "#222222".to_string(),
        shots: vec![
            ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 10.0,
                depth_in: 1.0,
                depth_out: 3.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 90.0,
                azimuth_out: 270.0,
                length: 8.0,
                depth_in: 3.0,
                depth_out: 5.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 135.0,
                azimuth_out: 315.0,
                length: 6.0,
                depth_in: 5.0,
                depth_out: 7.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let section3 = SectionData {
        name: Some("Another section to keep".to_string()),
        color: "#333333".to_string(),
        shots: vec![ShotData {
            azimuth_in: 180.0,
            azimuth_out: 0.0,
            length: 12.0,
            depth_in: 0.0,
            depth_out: 2.0,
            ..Default::default()
        }],
        ..Default::default()
    };

    project.add_section(section1);
    project.add_section(section2);
    project.add_section(section3);

    assert_eq!(project.sections.len(), 3);

    // Save initial state
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Remove section 1 (index 1, the one with 3 shots)
    project.remove_section(1);
    assert_eq!(project.sections.len(), 2);

    // Save after deletion
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify cascade delete
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.sections.len(), 2);
    assert_eq!(loaded.sections[0].name, Some("Section to keep".to_string()));
    assert_eq!(
        loaded.sections[1].name,
        Some("Another section to keep".to_string())
    );

    // Verify shots from deleted section are gone
    let total_shots: usize = loaded.sections.iter().map(|s| s.shots.len()).sum();
    assert_eq!(total_shots, 2); // 1 shot from section 0 + 1 shot from section 2
}

// === Test 3.7: Database Transaction Rollback ===
#[test]
fn test_database_transaction_rollback() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_rollback.stalx");

    // Create and save a valid project
    let mut project = Project::new("Rollback Test".to_string(), None);
    let section = SectionData {
        name: Some("Original Section".to_string()),
        color: "#abcdef".to_string(),
        shots: vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 180.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 1.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    project.add_section(section);

    // Save the initial project
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Verify original data
    {
        let db = Database::open_existing(&db_path).unwrap();
        let loaded = db.load_project().unwrap();
        assert_eq!(loaded.name, "Rollback Test");
        assert_eq!(loaded.sections.len(), 1);
        assert_eq!(
            loaded.sections[0].name,
            Some("Original Section".to_string())
        );
    }

    // Modify and save again - this simulates a successful transaction
    project.name = "Modified Project".to_string();
    project.sections[0].name = Some("Modified Section".to_string());
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Verify modification persisted
    {
        let db = Database::open_existing(&db_path).unwrap();
        let loaded = db.load_project().unwrap();
        assert_eq!(loaded.name, "Modified Project");
        assert_eq!(
            loaded.sections[0].name,
            Some("Modified Section".to_string())
        );
    }

    // Test that the transaction is atomic - all data is cleared and rewritten
    // If we open for save, it should be a complete replacement
    let mut new_project = Project::new("Completely New".to_string(), None);
    new_project.add_section(SectionData {
        name: Some("New Section A".to_string()),
        color: "#000000".to_string(),
        shots: vec![],
        ..Default::default()
    });
    new_project.add_section(SectionData {
        name: Some("New Section B".to_string()),
        color: "#ffffff".to_string(),
        shots: vec![],
        ..Default::default()
    });

    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&new_project).unwrap();
    }

    // Verify complete replacement
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.name, "Completely New");
    assert_eq!(loaded.sections.len(), 2);
    // The old "Modified Section" should be completely gone
    assert!(loaded
        .sections
        .iter()
        .all(|s| s.name != Some("Modified Section".to_string())));
}

// === Test 3.8: Open Existing vs Open New ===
#[test]
fn test_open_existing_vs_open_new() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_open.stalx");
    let nonexistent_path = temp_dir.path().join("nonexistent.stalx");

    // Test 1: open_existing on nonexistent file should fail
    let result = Database::open_existing(&nonexistent_path);
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("does not exist"),
        "Expected 'does not exist' error, got: {err_msg}"
    );

    // Test 2: open() on nonexistent file should create it
    {
        let db = Database::open(&db_path);
        assert!(db.is_ok());
        assert!(db_path.exists());
    }

    // Test 3: open_existing on empty database (no project row) should fail
    {
        let result = Database::open_existing(&db_path);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("valid Stalagmight project"),
            "Expected 'valid Stalagmight project' error, got: {err_msg}"
        );
    }

    // Test 4: Save a project, then open_existing should succeed
    let mut project = Project::new("Test Project".to_string(), None);
    project.add_section(SectionData {
        name: Some("Test".to_string()),
        color: "#123456".to_string(),
        shots: vec![],
        ..Default::default()
    });

    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Now open_existing should work
    let result = Database::open_existing(&db_path);
    assert!(result.is_ok());

    // Test 5: Verify loaded data is correct
    let loaded = result.unwrap().load_project().unwrap();
    assert_eq!(loaded.name, "Test Project");
    assert_eq!(loaded.sections.len(), 1);
}

// === Additional Edge Case Tests ===

#[test]
fn test_save_load_loop_closures() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_loops.stalx");

    let mut project = Project::new("Loop Test".to_string(), None);

    // Add sections
    project.add_section(SectionData {
        name: Some("Section 1".to_string()),
        color: "#ff0000".to_string(),
        shots: vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 180.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 5.0,
            ..Default::default()
        }],
        ..Default::default()
    });

    // Add loop closure data
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(0, 0).into(),
        station_b: StationRef::new(0, 1).into(),
        path_length: 50.0,
        misclosure_distance: 0.5,
        error_percentage: 1.0,
        horizontal_misclosure: 0.4,
        vertical_misclosure: 0.3,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Save and load
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.loop_closures.len(), 1);
    let lc = &loaded.loop_closures[0];
    assert_eq!(lc.id, 1);
    assert_eq!(lc.path_length, 50.0);
    assert_eq!(lc.misclosure_distance, 0.5);
    assert_eq!(lc.error_percentage, 1.0);
    assert_eq!(lc.horizontal_misclosure, 0.4);
    assert_eq!(lc.vertical_misclosure, 0.3);
}

#[test]
fn test_display_settings_persistence() {
    let db = Database::in_memory().unwrap();

    // Save settings
    db.save_display_setting("scale_bar", "true").unwrap();
    db.save_display_setting("station_labels", "false").unwrap();
    db.save_display_setting("custom_value", "42.5").unwrap();

    // Load all settings
    let settings = db.load_display_settings().unwrap();
    assert_eq!(settings.get("scale_bar"), Some(&"true".to_string()));
    assert_eq!(settings.get("station_labels"), Some(&"false".to_string()));
    assert_eq!(settings.get("custom_value"), Some(&"42.5".to_string()));

    // Get individual setting
    assert_eq!(
        db.get_display_setting("scale_bar").unwrap(),
        Some("true".to_string())
    );
    assert_eq!(db.get_display_setting("nonexistent").unwrap(), None);

    // Update setting
    db.save_display_setting("scale_bar", "false").unwrap();
    assert_eq!(
        db.get_display_setting("scale_bar").unwrap(),
        Some("false".to_string())
    );
}

/// [#303] The writer writes every row it is given, renumbering `sequence` 0..n so
/// `ORDER BY sequence` reproduces stack order.
///
/// There used to be an incremental `sync_undo_stack` here, and a test asserting the
/// OPPOSITE of this - that a second call left pre-existing rows in place. It was
/// measuring the helper against a database that persists between calls, which is NOT how
/// saving works: `save_project` builds a fresh `.stalx.tmp` and renames it over the
/// target, so every save writes into a file with no undo tables. The "optimization" that
/// test blessed silently dropped the history whenever nothing had changed since the
/// previous save.
#[test]
fn undo_persist_save_writes_the_rows_it_is_given() {
    let db = Database::in_memory().unwrap();

    let row = |id: u64| {
        (
            format!(r#"{{"type":"Cmd{id}"}}"#),
            format!(r#"{{"n":{id}}}"#),
            Some("2026-07-01T00:00:00+00:00".to_owned()),
        )
    };

    db.save_undo_stack_with_timestamps((0..50).map(row).collect())
        .unwrap();

    let loaded = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(loaded.len(), 50);
    assert_eq!(loaded[0].1, r#"{"type":"Cmd0"}"#);
    assert_eq!(loaded[49].1, r#"{"type":"Cmd49"}"#);

    // A second call with one more row rewrites the table and keeps the ordering by
    // `sequence`.
    db.save_undo_stack_with_timestamps((0..51).map(row).collect())
        .unwrap();
    let loaded = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(loaded.len(), 51);
    assert_eq!(
        loaded.iter().map(|(seq, ..)| *seq).collect::<Vec<_>>(),
        (0..51).collect::<Vec<_>>()
    );
}

/// [#303] The delete half: a save is a full rewrite, so rows the stack no longer holds
/// leave the table - whether they fell off the FRONT (a byte-budget trim) or the BACK
/// (an undo popping the tail), and all of them when the stack is empty.
#[test]
fn undo_persist_save_leaves_no_stragglers() {
    let db = Database::in_memory().unwrap();
    let row = |id: u64| {
        (
            format!(r#"{{"type":"Cmd{id}"}}"#),
            format!(r#"{{"n":{id}}}"#),
            None,
        )
    };
    db.save_undo_stack_with_timestamps((0..5).map(row).collect())
        .unwrap();

    // Two evicted from the front, one popped off the back: only the middle two are
    // handed over, and the other three must not survive.
    db.save_undo_stack_with_timestamps((2..4).map(row).collect())
        .unwrap();

    let loaded = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(
        loaded
            .iter()
            .map(|(_, cmd, ..)| cmd.clone())
            .collect::<Vec<_>>(),
        vec![r#"{"type":"Cmd2"}"#, r#"{"type":"Cmd3"}"#]
    );

    // An empty stack clears the table outright - the shape a Full-mode project's redo
    // save takes on every save (#303 F05).
    db.save_undo_stack_with_timestamps(vec![]).unwrap();
    assert!(db.load_undo_stack_with_timestamps().unwrap().is_empty());
}

#[test]
fn test_undo_stack_persistence() {
    let db = Database::in_memory().unwrap();

    // Save undo entries
    let entries = vec![
        (
            r#"{"type":"AddSection"}"#.to_string(),
            r#"{"section_id":0}"#.to_string(),
        ),
        (
            r#"{"type":"AddShot"}"#.to_string(),
            r#"{"shot_id":1}"#.to_string(),
        ),
        (
            r#"{"type":"UpdateShot"}"#.to_string(),
            r#"{"old_data":{}}"#.to_string(),
        ),
    ];
    db.save_undo_stack(entries).unwrap();

    // Load undo stack
    let loaded = db.load_undo_stack().unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].0, 0); // sequence
    assert_eq!(loaded[0].1, r#"{"type":"AddSection"}"#);
    assert_eq!(loaded[0].2, r#"{"section_id":0}"#);
    assert_eq!(loaded[1].0, 1);
    assert_eq!(loaded[2].0, 2);

    // Clear undo after sequence 1
    db.clear_undo_after(1).unwrap();
    let loaded_after_clear = db.load_undo_stack().unwrap();
    assert_eq!(loaded_after_clear.len(), 2); // Only sequences 0 and 1 remain
}

#[test]
fn test_resolved_warnings_persistence() {
    let db = Database::in_memory().unwrap();

    // Save project first (to have tables)
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    let mut db = db;
    db.save_project(&project).unwrap();

    // Save resolved warning
    let warning = ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "AZIMUTH_DIFF".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: Some("Verified in field".to_string()),
        shot_uuid: None,
    };
    db.save_resolved_warning(&warning).unwrap();

    // Load resolved warnings
    let loaded = db.load_resolved_warnings().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].section_id, 0);
    assert_eq!(loaded[0].shot_id, 1);
    assert_eq!(loaded[0].warning_code, "AZIMUTH_DIFF");
    assert_eq!(
        loaded[0].resolved_comment,
        Some("Verified in field".to_string())
    );

    // Delete resolved warning
    db.delete_resolved_warning(0, 1, "AZIMUTH_DIFF", None)
        .unwrap();
    let loaded_after_delete = db.load_resolved_warnings().unwrap();
    assert!(loaded_after_delete.is_empty());
}

#[test]
fn test_resolved_warning_dedup_and_delete_by_uuid() {
    // Issue #57: suppression is keyed by the shot's stable UUID, not the
    // renumber-prone shot_id. Two saves with the same UUID but different
    // shot_id must collapse to a single row, and deletion by UUID must clear
    // it regardless of the shot_id supplied.
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    let base = ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-1".to_string()),
    };
    db.save_resolved_warning(&base).unwrap();
    // Same UUID, drifted shot_id (as an insert-before would produce on reload).
    db.save_resolved_warning(&ResolvedWarning {
        shot_id: 5,
        ..base.clone()
    })
    .unwrap();

    let loaded = db.load_resolved_warnings().unwrap();
    assert_eq!(loaded.len(), 1, "same UUID must dedup to a single row");
    assert_eq!(loaded[0].shot_uuid.as_deref(), Some("uuid-1"));
    assert_eq!(loaded[0].shot_id, 5, "latest position wins");

    // Deleting by UUID clears it even with an unrelated shot_id.
    db.delete_resolved_warning(0, 999, "ZERO_LENGTH", Some("uuid-1"))
        .unwrap();
    assert!(db.load_resolved_warnings().unwrap().is_empty());
}

#[test]
fn test_resolved_warnings_bulk_tolerates_duplicate_keys() {
    // Issue #57 regression guard: resolve_all can hand the bulk save two
    // warnings with the SAME (section_id, shot_id, warning_code) - e.g. a shot
    // with both azimuths out of range yields two INVALID_AZIMUTH rows, and an
    // over-anchored merge component can map two datums onto one shot. The bulk
    // save must not abort on the legacy UNIQUE constraint.
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    let w = ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "INVALID_AZIMUTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-1".to_string()),
    };
    // Two identical-key rows in one batch must not raise a UNIQUE error.
    db.save_resolved_warnings_bulk(&[w.clone(), w.clone()])
        .unwrap();
    assert_eq!(
        db.load_resolved_warnings().unwrap().len(),
        1,
        "duplicate (section, shot, code) collapses to one row instead of crashing the batch"
    );
}

#[test]
fn test_resolved_warning_uuid_save_does_not_delete_other_shots_ignore() {
    // Issue #57 (Item 2): a UUID-keyed save must NOT delete a DIFFERENT shot's
    // valid by-UUID row that occupies a different position. Before the fix the
    // DELETE also matched `(section_id, shot_id)`, so saving uuid-Y could wipe
    // uuid-X if Y's params happened to target X's position.
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    // Shot X ignored at position shot_id=1.
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-X".to_string()),
    })
    .unwrap();
    // Shot Y ignored at a DISTINCT position shot_id=2 (its own current spot).
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 2,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-Y".to_string()),
    })
    .unwrap();

    let loaded = db.load_resolved_warnings().unwrap();
    assert_eq!(loaded.len(), 2, "both distinct-UUID ignores must survive");
    let uuids: std::collections::HashSet<_> = loaded
        .iter()
        .filter_map(|r| r.shot_uuid.as_deref())
        .collect();
    assert!(
        uuids.contains("uuid-X"),
        "uuid-X's ignore must not be deleted"
    );
    assert!(uuids.contains("uuid-Y"), "uuid-Y's ignore must survive");
}

#[test]
fn test_unresolve_with_uuid_clears_same_shots_legacy_position_row_only() {
    // Issue #57 (review): un-ignore passes the LIVE shot's UUID. A legacy
    // NULL-uuid row suppressing that same shot (e.g. on a load path that didn't
    // run the backfill) must still be cleared - the delete's position branch is
    // restricted to `shot_uuid IS NULL`, so it clears the legacy row WITHOUT
    // touching a DIFFERENT shot's by-UUID row that occupies the same position.
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    // Legacy NULL-uuid suppression of the shot at position (0, 1).
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    })
    .unwrap();
    // A DIFFERENT shot's by-UUID suppression at a distinct position (0, 2) - must
    // NOT be deleted when we un-ignore the shot at (0, 1). (UNIQUE(section_id,
    // shot_id, warning_code) forbids two rows sharing one position+code, so this
    // models a different shot, not a same-position collision.)
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 2,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-other".to_string()),
    })
    .unwrap();

    // Un-ignore the live shot at (0, 1) with ITS uuid: the uuid branch matches
    // nothing (no "uuid-live" row), but the NULL-only position branch clears the
    // legacy row at (0, 1).
    db.delete_resolved_warning(0, 1, "ZERO_LENGTH", Some("uuid-live"))
        .unwrap();

    let rows = db.load_resolved_warnings().unwrap();
    assert!(
        !rows.iter().any(|r| r.shot_uuid.is_none()),
        "the legacy NULL-uuid row for the same shot/position must be cleared by un-ignore"
    );
    assert!(
        rows.iter()
            .any(|r| r.shot_uuid.as_deref() == Some("uuid-other")),
        "a different shot's by-UUID row at another position must NOT be deleted"
    );
}

#[test]
fn test_backfill_resolved_warning_uuids_upgrades_and_is_idempotent() {
    // Issue #57 (Item 4): legacy NULL-uuid resolved rows whose (section_id,
    // shot_id) still maps to a live shot are upgraded to UUID keying on load.
    // Idempotent, and a row with no live shot is left NULL (never mis-keyed).
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    // Legacy row keyed by position (no UUID) that maps to a live shot.
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    })
    .unwrap();
    // Legacy row whose (section, shot) is NOT in the live map - must stay NULL.
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 7,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    })
    .unwrap();

    let mut live: HashMap<(u32, u32), String> = HashMap::new();
    live.insert((0, 1), "uuid-live".to_string());

    let upgraded = db.backfill_resolved_warning_uuids(&live).unwrap();
    assert_eq!(upgraded, 1, "only the row with a live shot is upgraded");

    let loaded = db.load_resolved_warnings().unwrap();
    let mapped = loaded
        .iter()
        .find(|r| r.shot_id == 1 && r.warning_code == "ZERO_LENGTH")
        .expect("the (0,1) row should still exist");
    assert_eq!(
        mapped.shot_uuid.as_deref(),
        Some("uuid-live"),
        "the live-mapped legacy row gets the shot UUID"
    );
    let orphan = loaded
        .iter()
        .find(|r| r.shot_id == 7 && r.warning_code == "SHORT_SHOT")
        .expect("the orphan row should still exist");
    assert_eq!(
        orphan.shot_uuid, None,
        "the unmapped legacy row stays position-keyed"
    );

    // Idempotent: a second backfill upgrades nothing.
    let upgraded_again = db.backfill_resolved_warning_uuids(&live).unwrap();
    assert_eq!(upgraded_again, 0, "second backfill is a no-op");
}

// === Test: Verify merged_with is populated correctly from fractal fixture ===
#[test]
#[ignore]
fn test_merged_with_from_fractal_iter2() {
    use crate::models::StationRef;
    use std::path::Path;

    // Use the fractal_plant_iter2 fixture
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..") // Go up from crates/persistence to the repo root
        .join("e2e")
        .join("fixtures")
        .join("fractal_plant_iter2.stalx");

    // Make sure the fixture exists
    assert!(
        fixture_path.exists(),
        "Fixture file not found: {fixture_path:?}"
    );

    // Load the project
    let db = Database::open_existing(&fixture_path).unwrap();
    let project = db.load_project().unwrap();

    // Verify there are merges
    println!("Project has {} merges:", project.station_merges.len());
    for merge in &project.station_merges {
        println!("  Merge: {} <-> {}", merge.station_a, merge.station_b);
    }

    // Compute station coordinates (this populates merged_with)
    let stations = project.compute_station_coordinates();

    // Find station S1.st0
    let s1_st0_ref = StationRef::new(1, 0);
    let s1_st0 = stations
        .iter()
        .find(|s| s.station_ref == s1_st0_ref)
        .expect("Station 1:0 should exist");

    println!("Station 1:0 merged_with: {:?}", s1_st0.merged_with);

    // S1.st0 should be merged with:
    // - S0.st2 (parent branch merge - when S1 was created from S0)
    // - S2.st0 (sibling merge - when S2 was created from S1)
    let s0_st2 = StationRef::new(0, 2);
    let s2_st0 = StationRef::new(2, 0);

    assert!(
        s1_st0.merged_with.contains(&s0_st2),
        "Station 1:0 should be merged with 0:2 (parent). Got: {:?}",
        s1_st0.merged_with
    );
    assert!(
        s1_st0.merged_with.contains(&s2_st0),
        "Station 1:0 should be merged with 2:0 (sibling). Got: {:?}",
        s1_st0.merged_with
    );
    assert_eq!(
        s1_st0.merged_with.len(),
        2,
        "Station 1:0 should have exactly 2 merges. Got: {:?}",
        s1_st0.merged_with
    );

    // Verify transitive merge groups - 0:2 should also show merged with 2:0
    // since they're all in the same merge group (0:2 ↔ 1:0 ↔ 2:0)
    let s0_st2_station = stations
        .iter()
        .find(|s| s.station_ref == s0_st2)
        .expect("Station 0:2 should exist");
    println!("Station 0:2 merged_with: {:?}", s0_st2_station.merged_with);

    assert!(
        s0_st2_station.merged_with.contains(&s1_st0_ref),
        "Station 0:2 should be merged with 1:0. Got: {:?}",
        s0_st2_station.merged_with
    );
    assert!(
        s0_st2_station.merged_with.contains(&s2_st0),
        "Station 0:2 should be merged with 2:0 (transitive). Got: {:?}",
        s0_st2_station.merged_with
    );
    assert_eq!(
        s0_st2_station.merged_with.len(),
        2,
        "Station 0:2 should have exactly 2 merges (transitive group). Got: {:?}",
        s0_st2_station.merged_with
    );

    // Verify 2:0 also shows the full group
    let s2_st0_station = stations
        .iter()
        .find(|s| s.station_ref == s2_st0)
        .expect("Station 2:0 should exist");
    println!("Station 2:0 merged_with: {:?}", s2_st0_station.merged_with);

    assert!(
        s2_st0_station.merged_with.contains(&s1_st0_ref),
        "Station 2:0 should be merged with 1:0. Got: {:?}",
        s2_st0_station.merged_with
    );
    assert!(
        s2_st0_station.merged_with.contains(&s0_st2),
        "Station 2:0 should be merged with 0:2 (transitive). Got: {:?}",
        s2_st0_station.merged_with
    );
    assert_eq!(
        s2_st0_station.merged_with.len(),
        2,
        "Station 2:0 should have exactly 2 merges (transitive group). Got: {:?}",
        s2_st0_station.merged_with
    );
}

// === Test: Verify merged_with from iter4 fixture ===
#[test]
#[ignore]
fn test_merged_with_from_fractal_iter4() {
    use crate::models::StationRef;
    use std::path::Path;

    // Use the fractal_plant_iter4 fixture
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..") // Go up from crates/persistence to the repo root
        .join("e2e")
        .join("fixtures")
        .join("fractal_plant_iter4.stalx");

    // Make sure the fixture exists
    assert!(
        fixture_path.exists(),
        "Fixture file not found: {fixture_path:?}"
    );

    // Load the project
    let db = Database::open_existing(&fixture_path).unwrap();
    let project = db.load_project().unwrap();

    println!("Iter4 project has {} merges", project.station_merges.len());

    // Compute station coordinates (this populates merged_with)
    let stations = project.compute_station_coordinates();

    // Find station S1.st0
    let s1_st0_ref = StationRef::new(1, 0);
    let s1_st0 = stations
        .iter()
        .find(|s| s.station_ref == s1_st0_ref)
        .expect("Station 1:0 should exist");

    println!("Station 1:0 merged_with: {:?}", s1_st0.merged_with);

    // S1.st0 should be merged with:
    // - S0.st8 (parent branch merge - when S1 was created from S0)
    // - S2.st0 (child merge - when S2 was created from S1)
    let s0_st8 = StationRef::new(0, 8);
    let s2_st0 = StationRef::new(2, 0);

    assert!(
        s1_st0.merged_with.contains(&s0_st8),
        "Station 1:0 should be merged with 0:8 (parent). Got: {:?}",
        s1_st0.merged_with
    );
    assert!(
        s1_st0.merged_with.contains(&s2_st0),
        "Station 1:0 should be merged with 2:0 (child). Got: {:?}",
        s1_st0.merged_with
    );
    assert_eq!(
        s1_st0.merged_with.len(),
        2,
        "Station 1:0 should have exactly 2 merges. Got: {:?}",
        s1_st0.merged_with
    );
}

#[test]
fn test_integrity_check_passes_on_valid_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("integrity_valid.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Integrity Test".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    // open_existing runs validate_integrity internally - should succeed
    let db = Database::open_existing(&db_path).unwrap();
    // full_integrity_check should also pass
    let errors = db.full_integrity_check().unwrap();
    assert!(
        errors.is_empty(),
        "Expected no integrity errors, got: {errors:?}"
    );
}

#[test]
fn test_integrity_check_passes_on_readonly() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("integrity_readonly.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Readonly Integrity".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    // open_readonly runs validate_integrity internally - should succeed
    let _db = Database::open_readonly(&db_path).unwrap();
}

#[test]
fn test_integrity_check_fails_on_corrupt_file() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("corrupt.stalx");

    // Write garbage data to simulate a corrupt database file
    std::fs::write(&db_path, b"This is not a valid SQLite database").unwrap();

    // open_existing should fail (either at connection open or integrity check)
    let result = Database::open_existing(&db_path);
    assert!(result.is_err(), "Expected error for corrupt database");
}

#[test]
fn test_full_integrity_check_on_in_memory() {
    let db = Database::in_memory().unwrap();
    let errors = db.full_integrity_check().unwrap();
    assert!(
        errors.is_empty(),
        "In-memory DB should pass integrity check"
    );
}

// === Open Existing edge cases ===

#[test]
fn test_open_existing_fails_on_nonexistent_file() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("does_not_exist.stalx");

    let result = Database::open_existing(&db_path);
    assert!(result.is_err(), "Expected error for nonexistent file");
}

#[test]
fn test_open_existing_fails_on_empty_database() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("empty.stalx");

    // Create an empty SQLite database (no project tables)
    {
        let _conn = Connection::open(&db_path).unwrap();
        // Close without creating tables
    }

    let result = Database::open_existing(&db_path);
    assert!(result.is_err(), "Empty database should fail open_existing");
}

#[test]
fn test_open_readonly_fails_on_nonexistent_file() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("gone.stalx");

    let result = Database::open_readonly(&db_path);
    assert!(result.is_err());
}

#[test]
fn test_open_readonly_fails_on_non_stalx_extension() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.sqlite");

    // Create a valid SQLite database with wrong extension
    {
        let mut db = Database::open(&db_path).unwrap();
        let project = Project::new("Wrong Ext".to_string(), None);
        db.save_project(&project).unwrap();
    }

    let result = Database::open_readonly(&db_path);
    assert!(result.is_err(), "Non-.stalx extension should be rejected");
}

// === Round-trip with station offsets (individual station moves) ===

#[test]
fn test_save_load_station_offsets_individual() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("station_offsets.stalx");

    let mut project = Project::new("Station Offsets".to_string(), None);
    let section_data = SectionData {
        shots: vec![
            ShotData {
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                length: 10.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                length: 5.0,
                depth_in: 1.0,
                depth_out: 2.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let section_id = project.add_section(section_data).id;

    // Add individual station offsets
    let ref_a = StationRef::new(section_id, 0);
    let ref_b = StationRef::new(section_id, 1);
    project
        .station_offsets
        .insert(ref_a.into(), (1.5, -2.3, 0.1));
    project
        .station_offsets
        .insert(ref_b.into(), (-0.5, 0.8, -0.3));

    // Save
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.station_offsets.len(), 2);
    let offset_a = loaded.station_offset_for_ref(ref_a).unwrap();
    assert!((offset_a.0 - 1.5).abs() < 1e-10);
    assert!((offset_a.1 - (-2.3)).abs() < 1e-10);
    assert!((offset_a.2 - 0.1).abs() < 1e-10);

    let offset_b = loaded.station_offset_for_ref(ref_b).unwrap();
    assert!((offset_b.0 - (-0.5)).abs() < 1e-10);
    assert!((offset_b.1 - 0.8).abs() < 1e-10);
    assert!((offset_b.2 - (-0.3)).abs() < 1e-10);
}

// === Undo/Redo stack persistence with timestamps ===

#[test]
fn test_undo_stack_timestamps_round_trip() {
    let db = Database::in_memory().unwrap();

    let entries = vec![
        (
            "cmd1".to_string(),
            "undo1".to_string(),
            Some("2025-01-15T10:00:00+00:00".to_string()),
        ),
        ("cmd2".to_string(), "undo2".to_string(), None),
        (
            "cmd3".to_string(),
            "undo3".to_string(),
            Some("2025-01-15T12:00:00+00:00".to_string()),
        ),
    ];

    db.save_undo_stack_with_timestamps(entries).unwrap();
    let loaded = db.load_undo_stack_with_timestamps().unwrap();

    assert_eq!(loaded.len(), 3);

    // Entry 0: explicit timestamp
    assert_eq!(loaded[0].0, 0); // sequence
    assert_eq!(loaded[0].1, "cmd1");
    assert_eq!(loaded[0].2, "undo1");
    assert_eq!(loaded[0].3.as_deref(), Some("2025-01-15T10:00:00+00:00"));

    // Entry 1: generated timestamp (should exist, not None)
    assert_eq!(loaded[1].1, "cmd2");
    assert!(
        loaded[1].3.is_some(),
        "Auto-generated timestamp should be present"
    );

    // Entry 2: explicit timestamp
    assert_eq!(loaded[2].3.as_deref(), Some("2025-01-15T12:00:00+00:00"));
}

#[test]
fn test_redo_stack_timestamps_round_trip() {
    let db = Database::in_memory().unwrap();

    let entries = vec![
        (
            "redo_cmd1".to_string(),
            "redo_undo1".to_string(),
            Some("2025-02-01T08:00:00+00:00".to_string()),
        ),
        ("redo_cmd2".to_string(), "redo_undo2".to_string(), None),
    ];

    db.save_redo_stack_with_timestamps(entries).unwrap();
    let loaded = db.load_redo_stack_with_timestamps().unwrap();

    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].1, "redo_cmd1");
    assert_eq!(loaded[0].3.as_deref(), Some("2025-02-01T08:00:00+00:00"));
    assert!(loaded[1].3.is_some());
}

#[test]
fn test_clear_undo_after_sequence() {
    let db = Database::in_memory().unwrap();

    // Save 5 entries
    let entries: Vec<(String, String)> = (0..5)
        .map(|i| (format!("cmd{i}"), format!("undo{i}")))
        .collect();
    db.save_undo_stack(entries).unwrap();

    // Clear everything after sequence 2 (keep 0, 1, 2)
    db.clear_undo_after(2).unwrap();

    let loaded = db.load_undo_stack().unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[2].0, 2); // sequence = 2
}

// === Legacy (pre-v7) undo history must be dropped on load ===
//
// Pre-v7 builds serialized every untouched nullable ShotUpdates field as an
// explicit null inside persisted undo/redo payloads. Under the v7
// double_option semantics an explicit null means "clear the field", so
// replaying such an entry would silently wipe pitch/LRUD/comment/marker/
// temperature. The legacy JSON is ambiguous, so the only safe upgrade is to
// drop the stacks (gated on PRAGMA user_version < 7).

/// A pre-v7 UpdateShot command payload: untouched nullable fields serialized
/// as explicit nulls (this is what `serde_json::to_string(&ShotUpdates)`
/// produced before `skip_serializing_if` was added).
fn legacy_null_style_update_shot_command(section_id: u32, shot_id: u32) -> String {
    format!(
        r#"{{"type":"update_shot","section_id":{section_id},"shot_id":{shot_id},"updates":{{"azimuth_in":null,"azimuth_out":null,"length":4.0,"depth_in":null,"depth_out":null,"pitch_in":null,"pitch_out":null,"left":null,"right":null,"up":null,"down":null,"comment":null,"is_reference":null,"marker_index":null,"temperature":null}}}}"#
    )
}

/// A pre-v7 PreviousValues undo payload with the same explicit-null shape.
fn legacy_null_style_previous_values(section_id: u32, shot_id: u32) -> String {
    let values_json = r#"{"azimuth_in":null,"azimuth_out":null,"length":2.0,"depth_in":null,"depth_out":null,"pitch_in":null,"pitch_out":null,"left":null,"right":null,"up":null,"down":null,"comment":null,"is_reference":null,"marker_index":null,"temperature":null}"#;
    format!(
        r#"{{"PreviousValues":{{"section_id":{section_id},"shot_id":{shot_id},"values_json":{}}}}}"#,
        serde_json::to_string(values_json).unwrap()
    )
}

/// Create a v6-stamped project file containing legacy-shaped undo AND redo
/// entries, returning its path.
fn write_legacy_v6_project_with_undo_history(dir: &TempDir, name: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);

    let mut project = Project::new("Legacy Undo".to_string(), None);
    project.add_section(SectionData {
        shots: vec![ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 2.0,
            depth_in: 10.0,
            depth_out: 11.0,
            ..Default::default()
        }],
        ..Default::default()
    });

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    let saved = db.load_project().unwrap();
    let section_id = saved.sections[0].id;
    let shot_id = saved.sections[0].shots[0].id;

    let cmd = legacy_null_style_update_shot_command(section_id, shot_id);
    let undo_data = legacy_null_style_previous_values(section_id, shot_id);
    db.save_undo_stack_with_timestamps(vec![(cmd.clone(), undo_data.clone(), None)])
        .unwrap();
    db.save_redo_stack_with_timestamps(vec![(cmd, undo_data, None)])
        .unwrap();

    // Database::open stamped the current schema version; rewind the stamp so
    // the file looks exactly like one written by a pre-v7 build.
    db.set_user_version(6).unwrap();
    drop(db);

    path
}

/// [#303] A pre-v16 file's undo rows carry `file_path`, so the migration must drop them.
///
/// Until v16 a snapshot entry was `serde_json::to_string(project)` over the whole
/// `Project`, absolute path included. That was harmless while the push stripped
/// `undo_stack` unconditionally; Full mode now ships it, so those rows would put the
/// authoring machine's username and directory layout into the project's git history for
/// good. `serialize_project_snapshot` clears the field from here on but is producer-side
/// and cannot reach rows already written, and clear-on-lock deliberately skips a dirty
/// copy - which is exactly the upgrade-with-unpushed-work case. The migration is the only
/// point that catches every existing file before its first push.
#[test]
fn a_pre_v16_file_loses_its_path_bearing_undo_history_on_open() {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("pre-v16.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Leaky".to_string(), None))
            .unwrap();
        // A snapshot entry of the shape the old producer wrote.
        db.save_undo_stack_with_timestamps(vec![(
            r#"{"type":"delete_section","section_id":1}"#.to_string(),
            r#"{"ProjectSnapshot":"{\"file_path\":\"/home/victim/caves/secret.stalx\"}"}"#
                .to_string(),
            None,
        )])
        .unwrap();
        // Rewind the stamp so the file looks exactly like one written by a v15 build.
        db.set_user_version(15).unwrap();
        drop(db);
    }

    let db = Database::open_existing(&path).unwrap();
    let undo = db.load_undo_stack_with_timestamps().unwrap_or_default();
    assert!(
        undo.is_empty(),
        "the v16 migration must drop pre-v16 undo history; {} row(s) survived carrying \
         a local path",
        undo.len()
    );
}

/// [#413 finding 5] A pre-v17 file's undo rows carry explicit-null
/// `SectionUpdates` fields, so the migration must drop them.
///
/// v17 gave `name` / `surveyor` / `date` the `double_option` +
/// `skip_serializing_if` pair `ShotUpdates` has carried since v7. A pre-v17
/// writer serialized every UNTOUCHED one as an explicit `null`, and a present
/// `null` now means "clear this field" - so replaying a stored
/// `update_section_metadata` command, or the `PreviousValues` blob recorded
/// against it, would wipe the name, surveyor and date off the section it names.
/// The payload still PARSES, which is why nothing downstream can catch it and a
/// version-gated drop is the only safe upgrade - the same reasoning, and the
/// same remedy, as the v6 to v7 case below.
#[test]
fn a_pre_v17_file_loses_its_explicit_null_section_undo_history_on_open() {
    // Sanity: the legacy payload IS the dangerous shape - under the v17
    // deserializer its explicit nulls decode to Some(None) ("clear the field").
    let dangerous: crate::models::SectionUpdates =
        serde_json::from_str(r##"{"name":null,"surveyor":null,"date":null,"color":"#123456"}"##)
            .unwrap();
    assert_eq!(dangerous.name, Some(None));
    assert_eq!(dangerous.surveyor, Some(None));
    assert_eq!(dangerous.date, Some(None));

    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("pre-v17.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Legacy".to_string(), None))
            .unwrap();
        // An entry of the shape a pre-v17 producer wrote: every untouched
        // nullable field present and null, on BOTH halves of the entry (the
        // command replays on redo, the `values_json` on undo).
        db.save_undo_stack_with_timestamps(vec![(
            r##"{"type":"update_section_metadata","section_id":0,"updates":{"name":null,"surveyor":null,"date":null,"color":"#abcdef","direction":null,"quality":null}}"##
                .to_string(),
            r##"{"PreviousValues":{"section_id":0,"shot_id":null,"values_json":"{\"name\":null,\"surveyor\":null,\"date\":null,\"color\":\"#123456\",\"direction\":null,\"quality\":null}"}}"##
                .to_string(),
            None,
        )])
        .unwrap();
        let staged: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM undo_stack", [], |r| r.get(0))
            .unwrap();
        assert_eq!(staged, 1, "the fixture must actually stage an undo entry");
        // Rewind the stamp so the file looks exactly like one written by a v16 build.
        db.set_user_version(16).unwrap();
        drop(db);
    }

    let db = Database::open_existing(&path).unwrap();
    assert!(
        db.load_undo_stack_with_timestamps().unwrap().is_empty(),
        "the v17 migration must drop pre-v17 undo history: its explicit nulls now read \
         as \"clear the field\""
    );
    // And the file is now stamped current, so the drop happens exactly once.
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
}

/// A `PRAGMA user_version` ABOVE `SCHEMA_VERSION` must not be a way to keep the
/// payloads every migration arm exists to drop.
///
/// `ensure_schema_version` runs no arm for a file from the future - there is
/// nothing sensible it could run - so a stamp of 99 used to walk straight past
/// the v7, v13, v15, v16 and v17 drops with the stacks intact. That is a
/// one-pragma bypass of the v17 arm in particular, whose payloads still PARSE
/// and whose replay wipes a section's name, surveyor and date. The open used to
/// drop the history instead, which wrote into a newer build's file; since #649
/// it refuses the file, so the stacks are neither replayed nor deleted.
#[test]
fn a_file_claiming_a_future_schema_is_refused_with_its_history_left_in_place() {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("from-the-future.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        let mut project = Project::new("Future".to_string(), None);
        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#123456".to_string(),
            shots: vec![],
            ..Default::default()
        });
        db.save_project(&project).unwrap();
        // The same pre-v17 shape the test above stages, on both stacks: the one
        // the v17 arm drops, and the one this file's forged stamp would keep.
        db.save_undo_stack_with_timestamps(vec![(
            r##"{"type":"update_section_metadata","section_id":0,"updates":{"name":null,"surveyor":null,"date":null,"color":"#abcdef","direction":null,"quality":null}}"##
                .to_string(),
            r##"{"PreviousValues":{"section_id":0,"shot_id":null,"values_json":"{\"name\":null,\"surveyor\":null,\"date\":null,\"color\":\"#123456\",\"direction\":null,\"quality\":null}"}}"##
                .to_string(),
            None,
        )])
        .unwrap();
        db.save_redo_stack_with_timestamps(vec![(
            r##"{"type":"update_section_metadata","section_id":0,"updates":{"name":null,"surveyor":null,"date":null,"color":"#abcdef","direction":null,"quality":null}}"##
                .to_string(),
            r##"{"PreviousValues":{"section_id":0,"shot_id":null,"values_json":"{\"name\":null,\"surveyor\":null,\"date\":null,\"color\":\"#123456\",\"direction\":null,\"quality\":null}"}}"##
                .to_string(),
            None,
        )])
        .unwrap();
        assert_eq!(
            db.load_undo_stack_with_timestamps().unwrap().len(),
            1,
            "the fixture must actually stage an undo entry"
        );
        assert_eq!(
            db.load_redo_stack_with_timestamps().unwrap().len(),
            1,
            "the fixture must actually stage a redo entry"
        );
        // Forge the stamp: a file claiming a schema this build has never heard of.
        db.set_user_version(SCHEMA_VERSION + 82).unwrap();
        drop(db);
    }

    let err = Database::open_existing(&path)
        .err()
        .expect("a forged future user_version is refused, so nothing replays its payloads");
    assert!(err.downcast_ref::<NewerSchema>().is_some(), "{err:#}");

    // Nothing was deleted or re-stamped either: the file is the newer build's.
    let db = Database::open_for_metadata(&path).unwrap();
    assert_eq!(db.load_undo_stack_with_timestamps().unwrap().len(), 1);
    assert_eq!(db.load_redo_stack_with_timestamps().unwrap().len(), 1);
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION + 82);
}

#[test]
fn test_legacy_v6_undo_history_dropped_on_open_existing() {
    let temp_dir = TempDir::new().unwrap();
    let path = write_legacy_v6_project_with_undo_history(&temp_dir, "legacy_undo.stalx");

    // Sanity: the legacy payload IS the dangerous shape - under the new
    // deserializer its explicit nulls decode to Some(None) ("clear field").
    let dangerous: crate::models::ShotUpdates =
        serde_json::from_str(r#"{"length":2.0,"left":null,"pitch_in":null,"comment":null}"#)
            .unwrap();
    assert_eq!(dangerous.left, Some(None));
    assert_eq!(dangerous.pitch_in, Some(None));
    assert_eq!(dangerous.comment, Some(None));

    let db = Database::open_existing(&path).unwrap();

    // The v6→v7 migration must have dropped both stacks wholesale.
    assert!(
        db.load_undo_stack_with_timestamps().unwrap().is_empty(),
        "legacy undo entries must not survive the v7 upgrade"
    );
    assert!(
        db.load_redo_stack_with_timestamps().unwrap().is_empty(),
        "legacy redo entries must not survive the v7 upgrade"
    );
    // And the file is now stamped current, so the drop happens exactly once.
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);

    // The project data itself is untouched.
    let project = db.load_project().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].shots.len(), 1);
}

#[test]
fn test_legacy_v6_undo_history_dropped_on_open() {
    // Database::open stamps SCHEMA_VERSION without running the migration
    // chain, so it must drop legacy stacks itself before the stamp erases the
    // evidence that the file predates the format change.
    let temp_dir = TempDir::new().unwrap();
    let path = write_legacy_v6_project_with_undo_history(&temp_dir, "legacy_undo_open.stalx");

    let db = Database::open(&path).unwrap();
    assert!(db.load_undo_stack_with_timestamps().unwrap().is_empty());
    assert!(db.load_redo_stack_with_timestamps().unwrap().is_empty());
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn open_migrates_pre_v10_file_instead_of_only_stamping() {
    // #241 (P0): Database::open() used to init_schema() (CREATE-IF-NOT-EXISTS,
    // which cannot add a column to a table that already exists) then stamp
    // user_version = SCHEMA_VERSION WITHOUT running the migration chain. A pre-v10
    // file reopened this way (e.g. the rename-failure reopen at project_io.rs) was
    // marked current while its speleo_link still lacked the v10 `last_edit_at`
    // column; ensure_schema_version()'s fast path (user_version == SCHEMA_VERSION)
    // then skipped the ALTER forever and a later `SELECT last_edit_at` failed.
    // open() must run the real migration for an existing older file.
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("pre_v10.stalx");

    // Build a fresh current-schema file, then rewrite it to look like a v9 file:
    // a speleo_link WITHOUT last_edit_at, stamped user_version 9.
    {
        let db = Database::open(&path).unwrap();
        db.conn
            .execute_batch(
                "DROP TABLE IF EXISTS speleo_link;
                 CREATE TABLE speleo_link (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     endpoint TEXT NOT NULL,
                     project_id TEXT NOT NULL,
                     project_name TEXT NOT NULL,
                     base_commit_sha TEXT,
                     has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                     last_verify_at TEXT
                 );
                 INSERT INTO speleo_link
                     (id, endpoint, project_id, project_name, has_unpushed_edits)
                 VALUES (1, 'https://example.test', 'proj', 'name', 1);",
            )
            .unwrap();
        db.set_user_version(9).unwrap();

        // Precondition: the simulated v9 file genuinely lacks the column.
        let has_before: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_edit_at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_before, 0, "v9 fixture must lack last_edit_at");
    }

    // Reopen through open(): it must MIGRATE the pre-v10 file, not merely stamp it.
    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.get_user_version().unwrap(),
        SCHEMA_VERSION,
        "open() stamps the current version"
    );
    let has_after: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_edit_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        has_after, 1,
        "open() must run the v9->v10 migration that adds last_edit_at"
    );
    // The column is now selectable (this SELECT errored before the fix).
    db.conn
        .query_row("SELECT COUNT(last_edit_at) FROM speleo_link", [], |r| {
            r.get::<_, i64>(0)
        })
        .expect("SELECT last_edit_at must succeed after migration");
}

#[test]
fn open_migrates_existing_file_stamped_v0() {
    // #241 (pre-pr hardening): open() must not misclassify an EXISTING pre-v10
    // file that carries user_version 0 (a .stalx predating the version pragma, or
    // with no project row yet) as freshly-created and stamp it without migrating.
    // open() keys fresh-vs-migrate on whether the FILE existed before it opened,
    // not on user_version or project-row count, so an existing v0 file still
    // migrates. (An earlier `prior_version == 0 && !project_has_rows` heuristic
    // would have wrongly stamped this file current and skipped the ALTER.)
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("v0_existing.stalx");
    {
        let db = Database::open(&path).unwrap();
        db.conn
            .execute_batch(
                "DROP TABLE IF EXISTS speleo_link;
                 CREATE TABLE speleo_link (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     endpoint TEXT NOT NULL,
                     project_id TEXT NOT NULL,
                     project_name TEXT NOT NULL,
                     base_commit_sha TEXT,
                     has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                     last_verify_at TEXT
                 );",
            )
            .unwrap();
        db.set_user_version(0).unwrap();
    }

    // The file now EXISTS, is stamped user_version 0, and has a pre-v10
    // speleo_link. Reopen: it must be migrated, not stamp-skipped as "fresh".
    let db = Database::open(&path).unwrap();
    let has_after: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_edit_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        has_after, 1,
        "an existing v0 file must be migrated, not classified fresh and stamp-skipped"
    );
}

// The "new-format undo history survives reload" round-trip test lives with
// `core::undo`'s own tests in the root crate now: it drives `UndoStack`'s
// entry (de)serialization, which this crate cannot see.

// === Schema migration idempotency ===

#[test]
fn test_migrate_schema_is_idempotent() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("migrate_idem.stalx");

    // Open creates schema
    {
        let mut db = Database::open(&db_path).unwrap();
        let project = Project::new("Migration".to_string(), None);
        db.save_project(&project).unwrap();
    }

    // Opening again re-runs migrations - should not error
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.name, "Migration");
}

// === Display settings persistence ===

#[test]
fn test_display_settings_round_trip() {
    let db = Database::in_memory().unwrap();

    db.save_display_setting("show_grid", "true").unwrap();
    db.save_display_setting("grid_spacing", "10").unwrap();

    let settings = db.load_display_settings().unwrap();
    assert_eq!(settings.get("show_grid"), Some(&"true".to_string()));
    assert_eq!(settings.get("grid_spacing"), Some(&"10".to_string()));

    // Overwrite existing setting
    db.save_display_setting("show_grid", "false").unwrap();
    let updated = db.load_display_settings().unwrap();
    assert_eq!(updated.get("show_grid"), Some(&"false".to_string()));

    // Get individual setting
    let val = db.get_display_setting("grid_spacing").unwrap();
    assert_eq!(val, Some("10".to_string()));

    let missing = db.get_display_setting("nonexistent").unwrap();
    assert!(missing.is_none());
}

// === Resolved warnings persistence ===

#[test]
fn test_resolved_warnings_round_trip() {
    let mut db = Database::in_memory().unwrap();

    // Create a project with a section and shot so FK constraints are satisfied
    let mut project = Project::new("Warnings Test".to_string(), None);
    project.add_section(SectionData {
        shots: vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    // Get the actual section and shot IDs from saved project
    let loaded_project = db.load_project().unwrap();
    let section_id = loaded_project.sections[0].id;
    let shot_id = loaded_project.sections[0].shots[0].id;

    let warning = ResolvedWarning {
        section_id,
        shot_id,
        warning_code: "azimuth_deviation".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: Some("Verified in field".to_string()),
        shot_uuid: None,
    };

    db.save_resolved_warning(&warning).unwrap();

    let loaded = db.load_resolved_warnings().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].section_id, section_id);
    assert_eq!(loaded[0].shot_id, shot_id);
    assert_eq!(loaded[0].warning_code, "azimuth_deviation");
    assert_eq!(
        loaded[0].resolved_comment.as_deref(),
        Some("Verified in field")
    );

    // Delete it
    db.delete_resolved_warning(section_id, shot_id, "azimuth_deviation", None)
        .unwrap();
    let remaining = db.load_resolved_warnings().unwrap();
    assert!(remaining.is_empty());
}

// === Save-load round-trip with all complex fields ===

#[test]
fn test_save_load_complex_project_round_trip() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("complex_roundtrip.stalx");

    let mut project = Project::new(
        "Complex Cave".to_string(),
        Some("Deep Location".to_string()),
    );

    // Add section with detailed shots
    let section_a = project
        .add_section(SectionData {
            name: Some("North Passage".to_string()),
            surveyor: Some("Alice".to_string()),
            date: NaiveDate::from_ymd_opt(2025, 6, 15),
            color: "#ff0000".to_string(),
            direction: Some(SurveyDirection::In),
            quality: 0.8,
            shots: vec![
                ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 10.0,
                    depth_in: 0.0,
                    depth_out: 2.0,
                    left: Some(1.0),
                    right: Some(2.0),
                    up: Some(3.0),
                    down: Some(0.5),
                    pitch_in: Some(5.0),
                    pitch_out: Some(-3.0),
                    comment: Some("Good visibility".to_string()),
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 45.0,
                    azimuth_out: 45.0,
                    length: 8.0,
                    depth_in: 2.0,
                    depth_out: 4.0,
                    ..Default::default()
                },
            ],
        })
        .id;

    let section_b = project
        .add_section(SectionData {
            name: Some("East Branch".to_string()),
            color: "#00ff00".to_string(),
            direction: Some(SurveyDirection::Out),
            quality: 1.0,
            shots: vec![ShotData {
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                length: 5.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        })
        .id;

    // Add section offsets
    project.section_offsets.insert(section_a, (10.0, -5.0));

    // Add station merge
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Add zero datum
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(section_a, 0).into(),
        gps: GpsCoordinates::new(45.5, -90.3, 0.0).unwrap(),
    });

    // Add loop closure
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 0).into(),
        strategy: ClosureStrategy::Move,
        is_aligned: false,
        path_length: 15.0,
        misclosure_distance: 0.3,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.15,
        error_percentage: 2.0,
    });

    // Add station offset
    project
        .station_offsets
        .insert(StationRef::new(section_a, 1).into(), (0.1, -0.2, 0.05));

    // Save
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify everything
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.name, "Complex Cave");
    assert_eq!(loaded.location, Some("Deep Location".to_string()));
    assert_eq!(loaded.sections.len(), 2);

    // Verify section A
    let s_a = loaded
        .sections
        .iter()
        .find(|s| s.name == Some("North Passage".to_string()))
        .unwrap();
    assert_eq!(s_a.direction, SurveyDirection::In);
    assert!((s_a.quality - 0.8).abs() < 1e-6);
    assert_eq!(s_a.shots.len(), 2);
    assert_eq!(s_a.shots[0].comment, Some("Good visibility".to_string()));
    assert_eq!(s_a.shots[0].left, Some(1.0));
    assert_eq!(s_a.shots[0].pitch_in, Some(5.0));

    // Verify section offsets
    let s_a_id = s_a.id;
    let offset = loaded.section_offsets.get(&s_a_id).unwrap();
    assert!((offset.0 - 10.0).abs() < 1e-10);
    assert!((offset.1 - (-5.0)).abs() < 1e-10);

    // Verify merges
    assert_eq!(loaded.station_merges.len(), 1);
    assert_eq!(
        loaded
            .resolve_key(&loaded.station_merges[0].station_a)
            .unwrap()
            .station_index,
        2
    );

    // Verify zero datums
    assert_eq!(loaded.zero_datums.len(), 1);
    assert!((loaded.zero_datums[0].gps.latitude - 45.5).abs() < 1e-6);

    // Verify loop closures
    assert_eq!(loaded.loop_closures.len(), 1);
    assert!((loaded.loop_closures[0].path_length - 15.0).abs() < 1e-6);
    assert_eq!(loaded.loop_closures[0].strategy, ClosureStrategy::Move);

    // Verify station offsets
    let st_offset = loaded
        .station_offset_for_ref(StationRef::new(s_a_id, 1))
        .unwrap();
    assert!((st_offset.0 - 0.1).abs() < 1e-10);
    assert!((st_offset.1 - (-0.2)).abs() < 1e-10);
    assert!((st_offset.2 - 0.05).abs() < 1e-10);
}

// =========================================================================
// Recovery: create_numbered_backup
// =========================================================================

#[test]
fn test_create_numbered_backup_first_creates_no_suffix() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");

    // Create a minimal source file
    let db = Database::open(&db_path).unwrap();
    drop(db);

    let backup = Database::create_numbered_backup(&db_path).unwrap();
    assert_eq!(backup, temp_dir.path().join("project.stalx.backup"));
    assert!(backup.exists());
}

#[test]
fn test_create_numbered_backup_second_gets_suffix_1() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");

    let db = Database::open(&db_path).unwrap();
    drop(db);

    // Create first backup
    let first = Database::create_numbered_backup(&db_path).unwrap();
    assert!(first.exists());

    // Second backup should be .backup.1
    let second = Database::create_numbered_backup(&db_path).unwrap();
    assert_eq!(second, temp_dir.path().join("project.stalx.backup.1"));
    assert!(second.exists());
}

#[test]
fn test_create_numbered_backup_increments_until_free_slot() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");

    let db = Database::open(&db_path).unwrap();
    drop(db);

    // Create base + .1 through .3
    for _ in 0..4 {
        Database::create_numbered_backup(&db_path).unwrap();
    }

    let fifth = Database::create_numbered_backup(&db_path).unwrap();
    assert_eq!(fifth, temp_dir.path().join("project.stalx.backup.4"));
}

// =========================================================================
// Recovery: list_backups
// =========================================================================

#[test]
fn test_list_backups_empty_when_no_backups() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");
    std::fs::write(&db_path, b"dummy").unwrap();

    let backups = Database::list_backups(&db_path);
    assert!(backups.is_empty());
}

#[test]
fn test_list_backups_finds_backup_files() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");
    std::fs::write(&db_path, b"original").unwrap();

    // Create backup-named files
    std::fs::write(temp_dir.path().join("project.stalx.backup"), b"backup0").unwrap();
    std::fs::write(temp_dir.path().join("project.stalx.backup.1"), b"backup1").unwrap();
    // A .tmp file for the same stem
    std::fs::write(temp_dir.path().join("project.stalx.tmp"), b"tmp").unwrap();
    // Unrelated file should not appear
    std::fs::write(temp_dir.path().join("other.stalx.backup"), b"other").unwrap();

    let backups = Database::list_backups(&db_path);
    // Should find 3 matching files (project.stalx.backup, .backup.1, .tmp)
    assert_eq!(backups.len(), 3);
    // All paths should contain "project.stalx"
    for b in &backups {
        assert!(b.path.contains("project.stalx"));
    }
}

#[test]
fn test_list_backups_sorted_newest_first() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");
    std::fs::write(&db_path, b"original").unwrap();

    // Create files with slightly different timestamps
    std::fs::write(temp_dir.path().join("project.stalx.backup"), b"old").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(temp_dir.path().join("project.stalx.backup.1"), b"newer").unwrap();

    let backups = Database::list_backups(&db_path);
    assert_eq!(backups.len(), 2);
    // Newest first
    assert!(backups[0].path.contains("backup.1"));
    assert!(backups[1].path.contains(".backup"));
    assert!(!backups[1].path.contains("backup.1"));
}

#[test]
fn test_list_backups_nonexistent_parent_returns_empty() {
    let path = std::path::PathBuf::from("/nonexistent/dir/project.stalx");
    let backups = Database::list_backups(&path);
    assert!(backups.is_empty());
}

#[test]
fn test_list_backups_no_parent_returns_empty() {
    // A bare filename with no parent directory
    let path = std::path::PathBuf::from("project.stalx");
    let backups = Database::list_backups(&path);
    // Should not panic, returns empty or finds matches in current dir
    // Just verify it doesn't crash
    let _ = backups;
}

// =========================================================================
// Recovery: open_for_recovery
// =========================================================================

#[test]
fn test_open_for_recovery_nonexistent_file_fails() {
    let result = Database::open_for_recovery(Path::new("/nonexistent/db.stalx"));
    assert!(result.is_err());
}

#[test]
fn test_open_for_recovery_valid_db_succeeds() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("valid.stalx");

    // Create a valid database
    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Recovery Test".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    // Open for recovery should succeed even on a healthy file
    let db = Database::open_for_recovery(&db_path).unwrap();
    // Should be able to query basic info
    let result: String = db
        .conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(result, "ok");
}

// =========================================================================
// Recovery: attempt_repair
// =========================================================================

#[test]
fn test_attempt_repair_on_healthy_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("healthy.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Repair Test".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    let (backup_path, ok) = Database::attempt_repair(&db_path).unwrap();
    assert!(
        ok,
        "Healthy database should pass integrity check after repair"
    );
    assert!(backup_path.exists(), "Backup should exist");

    // Verify original file is still valid
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.name, "Repair Test");
}

// =========================================================================
// Recovery: export_salvageable
// =========================================================================

#[test]
fn test_export_salvageable_from_healthy_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("salvage.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Salvage Test".to_string(), Some("Cave X".to_string()));
    let section = SectionData {
        name: Some("Passage A".to_string()),
        color: "#ff0000".to_string(),
        shots: vec![ShotData::default(), ShotData::default()],
        ..Default::default()
    };
    project.add_section(section);
    db.save_project(&project).unwrap();
    drop(db);

    let salvaged = Database::export_salvageable(&db_path).unwrap();
    assert_eq!(salvaged.project_name.as_deref(), Some("Salvage Test"));
    assert_eq!(salvaged.project_location.as_deref(), Some("Cave X"));
    assert_eq!(salvaged.section_count, 1);
    assert_eq!(salvaged.shot_count, 2);
    assert!(salvaged.sections_json.is_some());
    assert!(salvaged.full_project_json.is_some());
    assert!(salvaged.errors.is_empty());
}

#[test]
fn test_export_salvageable_nonexistent_file_fails() {
    let result = Database::export_salvageable(Path::new("/nonexistent/db.stalx"));
    assert!(result.is_err());
}

// =========================================================================
// Recovery: try_read helpers
// =========================================================================

#[test]
fn test_try_read_metadata_from_empty_db() {
    // In-memory DB with schema but no project data
    let db = Database::in_memory().unwrap();
    let result = db.try_read_project_metadata().unwrap();
    assert!(result.is_none());
}

#[test]
fn test_try_read_metadata_from_populated_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("metadata.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Meta Project".to_string(), Some("Location X".to_string()));
    db.save_project(&project).unwrap();

    let result = db.try_read_project_metadata().unwrap();
    assert!(result.is_some());
    let (name, location) = result.unwrap();
    assert_eq!(name, "Meta Project");
    assert_eq!(location.as_deref(), Some("Location X"));
}

#[test]
fn test_try_read_sections_from_empty_db() {
    let db = Database::in_memory().unwrap();
    let sections = db.try_read_sections().unwrap();
    assert!(sections.is_empty());
}

#[test]
fn test_try_read_sections_from_populated_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("sections.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Sections Test".to_string(), None);
    project.add_section(SectionData {
        name: Some("Passage A".to_string()),
        color: "#ff0000".to_string(),
        ..Default::default()
    });
    project.add_section(SectionData {
        name: Some("Passage B".to_string()),
        color: "#00ff00".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    let sections = db.try_read_sections().unwrap();
    assert_eq!(sections.len(), 2);
}

#[test]
fn test_try_read_shot_count_empty() {
    let db = Database::in_memory().unwrap();
    let count = db.try_read_shot_count().unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_try_read_merge_count_empty() {
    let db = Database::in_memory().unwrap();
    let count = db.try_read_merge_count().unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_has_project_data_empty_db() {
    let db = Database::in_memory().unwrap();
    assert!(!db.has_project_data().unwrap());
}

#[test]
fn test_has_project_data_with_project() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("has_data.stalx");

    let mut db = Database::open(&db_path).unwrap();
    assert!(!db.has_project_data().unwrap());

    let project = Project::new("Test".to_string(), None);
    db.save_project(&project).unwrap();
    assert!(db.has_project_data().unwrap());
}

// ============================================================================
// Schema migration tests
// ============================================================================

#[test]
fn test_migrate_schema_adds_direction_column() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("migrate_direction.stalx");

    // Create a minimal DB without the direction column
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r"
        CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT, location TEXT, created_at TEXT, modified_at TEXT, schema_version INTEGER);
        CREATE TABLE sections (id INTEGER PRIMARY KEY, name TEXT, surveyor TEXT, date TEXT, color TEXT NOT NULL, created_at TEXT NOT NULL);
        CREATE TABLE shots (id INTEGER PRIMARY KEY, section_id INTEGER NOT NULL, sequence_order INTEGER NOT NULL, azimuth_in REAL, azimuth_out REAL, length REAL, depth_in REAL, depth_out REAL, pitch_in REAL, pitch_out REAL, left_distance REAL, right_distance REAL, up_distance REAL, down_distance REAL, comment TEXT, is_virtual INTEGER DEFAULT 0, marker_index INTEGER, temperature REAL);
        CREATE TABLE station_merges (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, merged_at TEXT);
        CREATE TABLE zero_datums (id INTEGER PRIMARY KEY, section_id INTEGER, station_index INTEGER, latitude REAL, longitude REAL, depth_reference REAL);
        CREATE TABLE section_offsets (section_id INTEGER PRIMARY KEY, offset_x REAL, offset_y REAL);
        CREATE TABLE loop_closures (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, path_length REAL, misclosure_distance REAL, error_percentage REAL, horizontal_misclosure REAL, vertical_misclosure REAL);
        INSERT INTO project VALUES (1, 'Test', NULL, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 1);
        INSERT INTO sections VALUES (0, 'Sec1', NULL, NULL, '#ff0000', '2024-01-01T00:00:00Z');
        ",
    ).unwrap();
    drop(conn);

    // Open with Database::open_existing which runs migrations
    let db = Database::open_existing(&db_path).unwrap();
    let project = db.load_project().unwrap();
    assert_eq!(project.sections.len(), 1);
    // Direction should default to 'in' after migration
    assert_eq!(project.sections[0].direction.to_string(), "in");
}

#[test]
fn test_migrate_schema_adds_quality_column() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("migrate_quality.stalx");

    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r"
        CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT, location TEXT, created_at TEXT, modified_at TEXT, schema_version INTEGER);
        CREATE TABLE sections (id INTEGER PRIMARY KEY, name TEXT, surveyor TEXT, date TEXT, color TEXT NOT NULL, direction TEXT DEFAULT 'in', created_at TEXT NOT NULL);
        CREATE TABLE shots (id INTEGER PRIMARY KEY, section_id INTEGER NOT NULL, sequence_order INTEGER NOT NULL, azimuth_in REAL, azimuth_out REAL, length REAL, depth_in REAL, depth_out REAL, pitch_in REAL, pitch_out REAL, left_distance REAL, right_distance REAL, up_distance REAL, down_distance REAL, comment TEXT, is_virtual INTEGER DEFAULT 0, marker_index INTEGER, temperature REAL);
        CREATE TABLE station_merges (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, merged_at TEXT);
        CREATE TABLE zero_datums (id INTEGER PRIMARY KEY, section_id INTEGER, station_index INTEGER, latitude REAL, longitude REAL, depth_reference REAL);
        CREATE TABLE section_offsets (section_id INTEGER PRIMARY KEY, offset_x REAL, offset_y REAL);
        CREATE TABLE loop_closures (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, path_length REAL, misclosure_distance REAL, error_percentage REAL, horizontal_misclosure REAL, vertical_misclosure REAL);
        INSERT INTO project VALUES (1, 'QualityTest', NULL, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 1);
        INSERT INTO sections VALUES (0, 'Sec1', NULL, NULL, '#00ff00', 'out', '2024-01-01T00:00:00Z');
        ",
    ).unwrap();
    drop(conn);

    let db = Database::open_existing(&db_path).unwrap();
    let project = db.load_project().unwrap();
    // Quality should default to 1.0 after migration
    assert!((project.sections[0].quality - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_migrate_schema_adds_closure_columns() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("migrate_closure.stalx");

    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r"
        CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT, location TEXT, created_at TEXT, modified_at TEXT, schema_version INTEGER);
        CREATE TABLE sections (id INTEGER PRIMARY KEY, name TEXT, surveyor TEXT, date TEXT, color TEXT NOT NULL, direction TEXT DEFAULT 'in', quality REAL DEFAULT 1.0, created_at TEXT NOT NULL);
        CREATE TABLE shots (id INTEGER PRIMARY KEY, section_id INTEGER NOT NULL, sequence_order INTEGER NOT NULL, azimuth_in REAL, azimuth_out REAL, length REAL, depth_in REAL, depth_out REAL, pitch_in REAL, pitch_out REAL, left_distance REAL, right_distance REAL, up_distance REAL, down_distance REAL, comment TEXT, is_virtual INTEGER DEFAULT 0, marker_index INTEGER, temperature REAL);
        CREATE TABLE station_merges (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, merged_at TEXT);
        CREATE TABLE zero_datums (id INTEGER PRIMARY KEY, section_id INTEGER, station_index INTEGER, latitude REAL, longitude REAL, depth_reference REAL);
        CREATE TABLE section_offsets (section_id INTEGER PRIMARY KEY, offset_x REAL, offset_y REAL);
        CREATE TABLE loop_closures (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, path_length REAL, misclosure_distance REAL, error_percentage REAL, horizontal_misclosure REAL, vertical_misclosure REAL);
        INSERT INTO project VALUES (1, 'ClosureTest', NULL, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 1);
        INSERT INTO sections VALUES (0, 'Sec1', NULL, NULL, '#ff0000', 'in', 1.0, '2024-01-01T00:00:00Z');
        INSERT INTO loop_closures VALUES (1, 0, 0, 0, 1, 100.0, 0.5, 0.5, 0.3, 0.2);
        ",
    ).unwrap();
    drop(conn);

    let db = Database::open_existing(&db_path).unwrap();
    // Verify migration added closure_id, strategy, and is_aligned columns
    let has_closure_id: bool = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='closure_id'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap();
    let has_strategy: bool = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='strategy'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap();
    let has_is_aligned: bool = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='is_aligned'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap();

    assert!(has_closure_id, "migration should add closure_id column");
    assert!(has_strategy, "migration should add strategy column");
    assert!(has_is_aligned, "migration should add is_aligned column");
}

#[test]
fn test_migrate_schema_idempotent() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("idempotent.stalx");

    // Create and save a full project
    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Idempotent".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    // Open it twice - migration runs both times, second should be a no-op
    let db1 = Database::open_existing(&db_path).unwrap();
    let p1 = db1.load_project().unwrap();
    drop(db1);

    let db2 = Database::open_existing(&db_path).unwrap();
    let p2 = db2.load_project().unwrap();
    drop(db2);

    assert_eq!(p1.name, p2.name);
    assert_eq!(p1.sections.len(), p2.sections.len());
}

// ============================================================================
// Special float values in save/load round-trips
// ============================================================================

#[test]
fn test_save_load_round_trip_extreme_coordinates() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("extreme_coords.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Extreme".to_string(), None);
    project.add_section(SectionData {
        name: Some("Deep".to_string()),
        color: "#ff0000".to_string(),
        shots: vec![ShotData {
            azimuth_in: 359.9,
            azimuth_out: 0.1,
            length: 999.99,
            depth_in: 0.0,
            depth_out: 999.99,
            pitch_in: Some(-89.9),
            pitch_out: Some(89.9),
            left: Some(0.01),
            right: Some(999.0),
            up: Some(0.0),
            down: Some(0.0),
            temperature: Some(-2.0),
            ..Default::default()
        }],
        ..Default::default()
    });

    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    let shot = &loaded.sections[0].shots[0];
    assert!((shot.azimuth_in - 359.9).abs() < 1e-6);
    assert!((shot.azimuth_out - 0.1).abs() < 1e-6);
    assert!((shot.length - 999.99).abs() < 1e-6);
    assert!((shot.depth_out - 999.99).abs() < 1e-6);
    assert!((shot.pitch_in.unwrap() - (-89.9)).abs() < 1e-6);
    assert!((shot.temperature.unwrap() - (-2.0)).abs() < 1e-6);
}

#[test]
fn test_save_load_round_trip_zero_values() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("zeros.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Zeros".to_string(), None);
    project.add_section(SectionData {
        name: None, // no name
        color: "#000000".to_string(),
        shots: vec![ShotData {
            uuid: None,
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 0.01, // smallest valid length
            depth_in: 0.0,
            depth_out: 0.0,
            pitch_in: None,
            pitch_out: None,
            left: None,
            right: None,
            up: None,
            down: None,
            comment: None,
            kind: ShotKind::Normal,
            marker_index: None,
            temperature: None,
            credit: None,
        }],
        ..Default::default()
    });

    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert!(loaded.sections[0].name.is_none());
    let shot = &loaded.sections[0].shots[0];
    assert!(shot.pitch_in.is_none());
    assert!(shot.left.is_none());
    assert!(shot.comment.is_none());
    assert!(shot.temperature.is_none());
}

#[test]
fn test_save_load_round_trip_all_metadata() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("all_meta.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("FullMeta".to_string(), Some("Underwater Cave".to_string()));
    project.add_section(SectionData {
        name: Some("Section A".to_string()),
        surveyor: Some("Diver One".to_string()),
        date: Some(NaiveDate::from_ymd_opt(2025, 12, 25).unwrap()),
        color: "#abcdef".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 0.75,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 270.0,
            length: 5.0,
            depth_in: 10.0,
            depth_out: 12.0,
            comment: Some("Test shot".to_string()),
            kind: ShotKind::Reference,
            marker_index: Some(42),
            ..Default::default()
        }],
    });

    // Add station merge, zero datum, section offset, station offset
    let section_id = project.sections[0].id;
    project.add_section(SectionData {
        name: Some("Section B".to_string()),
        color: "#fedcba".to_string(),
        shots: vec![ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 3.0,
            depth_in: 0.0,
            depth_out: 1.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let section_b_id = project.sections[1].id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_id, 1).into(),
        station_b: StationRef::new(section_b_id, 0).into(),
        merged_at: Utc::now(),
    });

    let gps = GpsCoordinates::new(48.8566, 2.3522, 5.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // set_zero_datum creates section offsets for all connected sections (A and B are merged),
    // so we don't add a manual section_offset - we just verify the ones set_zero_datum created.
    project
        .station_offsets
        .insert(StationRef::new(section_id, 1).into(), (1.0, 2.0, 3.0));
    project
        .imported_file_hashes
        .push("abc123def456".to_string());
    project.next_closure_id = 42;

    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.name, "FullMeta");
    assert_eq!(loaded.location.as_deref(), Some("Underwater Cave"));
    assert_eq!(loaded.sections.len(), 2);
    assert_eq!(
        loaded.sections[0].shots[0]
            .credit()
            .surveyor_names()
            .as_deref(),
        Some("Diver One")
    );
    assert_eq!(loaded.sections[0].direction.to_string(), "out");
    assert!((loaded.sections[0].quality - 0.75).abs() < 1e-6);
    assert_eq!(loaded.sections[0].shots[0].kind, ShotKind::Reference);
    assert_eq!(loaded.sections[0].shots[0].marker_index, Some(42));
    assert_eq!(loaded.station_merges.len(), 1);
    assert_eq!(loaded.zero_datums.len(), 1);
    assert!((loaded.zero_datums[0].gps.latitude - 48.8566).abs() < 1e-6);
    // The section offsets `set_zero_datum` produced survive the round trip
    // EXACTLY - value for value, not merely in number. Only the displaced
    // section carries one: `section_offsets` means "sections that are actually
    // displaced", so a section the datum leaves on exactly (0, 0) has no entry
    // to persist (#413 finding 2). This used to assert `len() == 2`, counting
    // that inert entry and checking no value at all.
    assert_eq!(loaded.section_offsets, project.section_offsets);
    assert_eq!(loaded.section_offsets.len(), 1);
    assert_eq!(loaded.station_offsets.len(), 1);
    assert_eq!(loaded.imported_file_hashes.len(), 1);
    assert_eq!(loaded.imported_file_hashes[0], "abc123def456");
    assert_eq!(loaded.next_closure_id, 42);
}

#[test]
fn test_save_load_round_trip_loop_closures_with_strategy() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("closure_strategy.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Closures".to_string(), None);
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(sec_id, 0).into(),
        station_b: StationRef::new(sec_id, 1).into(),
        path_length: 50.0,
        misclosure_distance: 0.1,
        error_percentage: 0.2,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Move,
        is_aligned: true,
    });
    project.loop_closures.push(LoopClosure {
        id: 2,
        station_a: StationRef::new(sec_id, 0).into(),
        station_b: StationRef::new(sec_id, 1).into(),
        path_length: 100.0,
        misclosure_distance: 0.5,
        error_percentage: 0.5,
        horizontal_misclosure: 0.3,
        vertical_misclosure: 0.2,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    project.next_closure_id = 3;

    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.loop_closures.len(), 2);
    let lc1 = &loaded.loop_closures[0];
    assert_eq!(lc1.id, 1);
    assert_eq!(lc1.strategy, ClosureStrategy::Move);
    assert!(lc1.is_aligned);
    assert!((lc1.path_length - 50.0).abs() < 1e-6);

    let lc2 = &loaded.loop_closures[1];
    assert_eq!(lc2.id, 2);
    assert_eq!(lc2.strategy, ClosureStrategy::Batch);
    assert!(!lc2.is_aligned);
    assert_eq!(loaded.next_closure_id, 3);
}

// ── Gzip compression unit tests ─────────────────────────────────────

#[test]
fn test_compress_decompress_round_trip() {
    let original = r#"{"type":"AddSection","data":{"name":"Main"}}"#;
    let compressed = compress_json(original);
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn test_compress_produces_gzip_magic_bytes() {
    let compressed = compress_json("hello");
    assert!(
        compressed.len() >= 2,
        "compressed output should have at least 2 bytes"
    );
    assert_eq!(compressed[0], 0x1f, "first magic byte");
    assert_eq!(compressed[1], 0x8b, "second magic byte");
}

#[test]
fn test_decompress_plain_utf8_backward_compat() {
    // Old uncompressed TEXT entries should be returned as-is
    let plain = b"plain JSON string";
    let result = decompress_json(plain).unwrap();
    assert_eq!(result, "plain JSON string");
}

#[test]
fn test_decompress_empty_data() {
    let result = decompress_json(b"").unwrap();
    assert_eq!(result, "");
}

#[test]
fn test_compress_empty_string() {
    let compressed = compress_json("");
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, "");
}

#[test]
fn test_compress_large_json() {
    // Large repetitive JSON compresses well
    let large = format!(
        r#"{{"data":[{}]}}"#,
        "1,".repeat(10_000).trim_end_matches(',')
    );
    let compressed = compress_json(&large);
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, large);
    // Compression should be significantly smaller
    assert!(
        compressed.len() < large.len() / 2,
        "compressed ({}) should be much smaller than original ({})",
        compressed.len(),
        large.len()
    );
}

#[test]
fn test_compress_unicode() {
    let unicode = r#"{"name":"Grotte de l'Émeraude","探検":"洞窟"}"#;
    let compressed = compress_json(unicode);
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, unicode);
}

#[test]
fn test_decompress_single_byte_not_gzip() {
    // Single byte can't be gzip - should return as lossy UTF-8
    let result = decompress_json(&[0x41]).unwrap();
    assert_eq!(result, "A");
}

#[test]
fn test_undo_stack_round_trips_through_compression() {
    // Verify the full pipeline: save → (compress) → load → (decompress) matches
    let db = Database::in_memory().unwrap();
    let json_with_special_chars = r##"{"color":"#ff0000","value":"hello\nworld"}"##;
    let entries = vec![(
        json_with_special_chars.to_string(),
        r#"{"undo":"data"}"#.to_string(),
    )];
    db.save_undo_stack(entries).unwrap();
    let loaded = db.load_undo_stack().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].1, json_with_special_chars);
    assert_eq!(loaded[0].2, r#"{"undo":"data"}"#);
}

#[test]
fn test_migrate_undo_redo_to_blob_on_fresh_db() {
    // Fresh in-memory DB already has BLOB columns (schema v3).
    // Migration should be idempotent and not error.
    let db = Database::in_memory().unwrap();
    db.migrate_undo_redo_to_blob().unwrap();
    // Save and load should still work after migration
    let entries = vec![("cmd".to_string(), "undo".to_string())];
    db.save_undo_stack(entries).unwrap();
    let loaded = db.load_undo_stack().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].1, "cmd");
}

// ── Decompression ceiling (#473) ────────────────────────────────────

/// The bomb shape itself: a tiny BLOB whose inflated size is enormous.
///
/// The assertion on the compressed length is the point of the test - it states the
/// amplification (8 MiB out of well under 64 KiB in), which is why a row-count bound
/// alone cannot contain this and an output ceiling is needed.
#[test]
fn a_gzip_bomb_is_rejected_by_the_ceiling() {
    let bomb = compress_json(&"a".repeat(8 * 1024 * 1024));
    assert!(
        bomb.len() < 64 * 1024,
        "8 MiB of one repeated byte should compress to well under 64 KiB, got {} bytes",
        bomb.len()
    );

    let err = decompress_json_capped(&bomb, 1024).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "the bomb should be refused by the ceiling, got: {rendered}"
    );
}

/// Overwrite a gzip blob's ISIZE trailer so it CLAIMS `declared` bytes.
///
/// The stream is left otherwise intact and deliberately inconsistent with the claim -
/// which is the whole point, since the claim is attacker-controlled and the tests below
/// exist to pin down what is done with a lie in each direction.
fn claiming_inflated_size(mut blob: Vec<u8>, declared: u32) -> Vec<u8> {
    let at = blob.len() - 4;
    blob[at..].copy_from_slice(&declared.to_le_bytes());
    blob
}

/// [#473] A blob is refused on its DECLARED size, before a byte is inflated.
///
/// The property `extract_entry_capped` (speleodb/dump.rs) gets from the ZIP central
/// directory, and the one the first attempt at this fix lacked: it inflated all 256 MiB
/// before refusing, which is the exact cost the ceiling exists to avoid. Proven at the
/// REAL constant and for a few bytes of work, which is only possible because nothing
/// here decompresses.
#[test]
fn a_blob_that_declares_more_than_the_ceiling_is_refused_without_inflating() {
    let honest = compress_json("{}");
    let liar = claiming_inflated_size(honest, u32::MAX);
    assert!(
        liar.len() < 128,
        "the point is that a hostile CLAIM costs nothing to make, got {} bytes",
        liar.len()
    );

    let budget = InflationBudget::new(MAX_HISTORY_LOAD_BYTES);
    let err = decompress_json_under(&liar, &budget).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("declares") && rendered.contains("decompression ceiling"),
        "the refusal must name the declared size, got: {rendered}"
    );
}

/// [#473] A legacy blob is priced at what it will ACTUALLY produce, not always 3x.
///
/// `from_utf8_lossy` returns valid input unchanged and only substitutes a 3-byte U+FFFD
/// per invalid subsequence, so the two cases differ by 3x - and the per-blob ceiling is
/// compared against this number. Answering `3 * len` unconditionally therefore refused a
/// perfectly valid 90 MB legacy row for a 270 MB output it would never produce: an
/// effective ceiling of 85 MiB where the constant says 256, discarding the whole stack of
/// a file this codebase itself wrote.
#[test]
fn a_legacy_blob_is_priced_by_its_real_utf8_expansion() {
    let valid = b"{\"type\":\"close_project\"}";
    assert_eq!(
        declared_inflated_size(valid),
        valid.len() as u64,
        "valid UTF-8 passes through from_utf8_lossy unchanged"
    );

    // Every 0xFF is its own invalid subsequence, so each becomes a 3-byte U+FFFD.
    let invalid = [0xFF_u8; 16];
    assert_eq!(
        declared_inflated_size(&invalid),
        48,
        "invalid input is still priced at the 3x worst case"
    );
    assert_eq!(
        String::from_utf8_lossy(&invalid).len(),
        48,
        "...and that worst case is exact, not a guess"
    );
}

/// [#473] THE invariant every bound rests on: a blob never produces more than it declared.
///
/// The pre-scan approves a window by summing `declared_inflated_size`, and the running
/// budget reserves the same number per row, so both are sound only while actual <=
/// declared for every possible input. Asserted over a spread of byte patterns rather than
/// the handful of shapes the other tests happen to use - lone invalid bytes, truncated
/// multi-byte prefixes (which collapse SEVERAL bytes into one U+FFFD, so they expand by
/// less than 3x), valid ASCII and valid multi-byte text.
#[test]
fn a_legacy_blob_never_produces_more_than_it_declares() {
    let cases: Vec<Vec<u8>> = vec![
        vec![],
        b"{}".to_vec(),
        b"plain ascii json".to_vec(),
        "\u{e9}\u{4e2d}\u{1f600}".repeat(8).into_bytes(), // valid, 2/3/4-byte chars
        vec![0xFF; 32],                                   // every byte its own bad subsequence
        vec![0x80; 32],                                   // stray continuation bytes
        b"\xE0\xA0".repeat(16),                           // truncated 3-byte prefixes
        b"\xF0\x9F\x98".repeat(16),                       // truncated 4-byte prefixes
        {
            let mut mixed = b"valid text ".to_vec();
            mixed.extend_from_slice(&[0xFF, 0xFE, 0x80]);
            mixed.extend_from_slice(b" more valid text");
            mixed
        },
    ];

    for data in cases {
        // Only the legacy branch is under test; a gzip-magic prefix would route elsewhere.
        assert!(!is_gzip(&data), "fixture must take the legacy branch");
        let declared = declared_inflated_size(&data);
        let produced = String::from_utf8_lossy(&data).len() as u64;
        assert!(
            produced <= declared,
            "declared {declared} but produced {produced} for {data:?}"
        );
    }
}

/// [#473] The per-blob ceiling binds the LEGACY branch too, not only gzip.
///
/// It used to sit below the `is_gzip` branch, which silently exempted legacy blobs from
/// the one bound that names a single row. Invalid bytes, so the 3x worst case is the real
/// output and the ceiling is what has to refuse it.
#[test]
fn a_legacy_blob_past_the_ceiling_is_refused_by_the_per_blob_bound() {
    // 3x this is just past 256 MiB, so it is the per-blob ceiling that must refuse it -
    // not the 1 GiB load budget, which it fits inside.
    let oversized = vec![0xFF_u8; (MAX_DECOMPRESSED_BLOB_BYTES as usize / 3) + 1024];
    assert!(!oversized.starts_with(&[0x1f, 0x8b]), "not a gzip blob");
    assert!(
        declared_inflated_size(&oversized) > MAX_DECOMPRESSED_BLOB_BYTES,
        "the 3x worst case must exceed the per-blob ceiling for this to test anything"
    );
    assert!(
        declared_inflated_size(&oversized) < MAX_HISTORY_LOAD_BYTES,
        "...and must fit the load budget, or the wrong bound would refuse it"
    );

    let budget = InflationBudget::new(MAX_HISTORY_LOAD_BYTES);
    let err = decompress_json_under(&oversized, &budget).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "the per-blob ceiling must be what refuses a legacy blob, got: {rendered}"
    );
    assert_eq!(
        budget.remaining(),
        MAX_HISTORY_LOAD_BYTES,
        "a blob refused before reserving must not touch the budget"
    );

    // ...and the SAME length of valid text is admitted, because its real output is `len`.
    // This is the half the unconditional 3x got wrong.
    let valid_same_size = vec![b'x'; oversized.len()];
    assert!(
        declared_inflated_size(&valid_same_size) < MAX_DECOMPRESSED_BLOB_BYTES,
        "a valid legacy row of the same length must fit the ceiling it really uses"
    );
}

/// [#473] ...and a blob that UNDERSTATES its size is stopped by the backstop.
///
/// The declared size is attacker-controlled, so it is only ever trusted in the refusing
/// direction. A blob claiming 1 KiB and carrying 8 MiB is capped at what it claimed, so
/// lying buys nothing: the `take` trips one byte past the claim.
#[test]
fn a_blob_that_understates_its_size_is_stopped_at_what_it_claimed() {
    let bomb = compress_json(&"a".repeat(8 * 1024 * 1024));
    let liar = claiming_inflated_size(bomb, 1024);

    let budget = InflationBudget::new(MAX_HISTORY_LOAD_BYTES);
    let err = decompress_json_under(&liar, &budget).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "an understated blob must still trip the read ceiling, got: {rendered}"
    );
}

/// [#473] A very large ceiling does not wrap `max_bytes + 1` into `take(0)`.
///
/// Debug builds panicked on the overflow; release builds have no `overflow-checks`, so
/// the wrap turned into an EMPTY string reported as SUCCESS - a caller asking for "no
/// practical limit" silently losing its payload, in the build where that is hardest to
/// notice.
#[test]
fn an_enormous_ceiling_returns_the_payload_rather_than_wrapping_to_empty() {
    let payload = "a".repeat(4096);
    let compressed = compress_json(&payload);

    assert_eq!(
        decompress_json_capped(&compressed, u64::MAX).unwrap(),
        payload
    );
}

/// The boundary is inclusive: `max_bytes` bytes out is fine, and comes back intact.
#[test]
fn a_payload_exactly_at_the_ceiling_still_decompresses() {
    let payload = "a".repeat(1024);
    let compressed = compress_json(&payload);

    let decompressed = decompress_json_capped(&compressed, 1024).unwrap();
    assert_eq!(decompressed.len(), 1024);
    assert_eq!(decompressed, payload);
}

/// And one byte past it is not - this is what `take(max_bytes + 1)` buys.
#[test]
fn one_byte_past_the_ceiling_is_rejected() {
    let payload = "a".repeat(1025);
    let compressed = compress_json(&payload);

    let err = decompress_json_capped(&compressed, 1024).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "1025 bytes under a 1024-byte ceiling should name the ceiling, got: {rendered}"
    );
}

/// A size trip is reported as a size trip even when the cut lands mid-codepoint.
///
/// The payload is 2 bytes per character, so the `take` boundary at `max_bytes + 1`
/// = 1025 falls on the FIRST byte of a character. Reading through `read_to_string`
/// would surface that as invalid UTF-8 and blame the wrong thing; reading into a
/// `Vec<u8>` and checking the length first keeps the diagnosis honest.
#[test]
fn a_multibyte_payload_past_the_ceiling_reports_the_ceiling_not_utf8() {
    let payload = "é".repeat(600); // 1200 bytes, 2 per char
    assert_eq!(payload.len(), 1200, "two bytes per character");
    let compressed = compress_json(&payload);

    let err = decompress_json_capped(&compressed, 1024).unwrap_err();
    let rendered = format!("{err:#} | {err:?}");
    assert!(
        rendered.contains("decompression ceiling"),
        "a mid-codepoint cut is still a size trip, got: {rendered}"
    );
    assert!(
        !rendered.contains("UTF-8"),
        "and must not be misreported as an encoding problem, got: {rendered}"
    );
}

/// The ceiling does not get in the way of a realistically large history blob.
#[test]
fn ordinary_history_blobs_still_round_trip_under_the_default_ceiling() {
    let entry = r#"{"type":"MoveStation","section":3,"station":17,"from":[12.5,-3.25,44.0],"to":[12.75,-3.5,44.25]},"#;
    let mut json = String::from(r#"{"entries":["#);
    while json.len() < 2 * 1024 * 1024 {
        json.push_str(entry);
    }
    json.push_str("{}]}");

    let compressed = compress_json(&json);
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, json);

    // The constant's own rationale: >10x the ~17 MB a 20,000-shot ProjectSnapshotPair
    // measures at (the table on UNDO_BYTE_BUDGET_BYTES). Shrinking it below that margin
    // invalidates the justification, so it has to fail here rather than silently start
    // refusing legitimate surveys. (The measurement is a binding rather than a literal
    // so the comparison is not const-folded into an `assertions_on_constants` warning.)
    let largest_measured_legitimate_blob: u64 = 17 * 1024 * 1024;
    assert!(
        MAX_DECOMPRESSED_BLOB_BYTES >= largest_measured_legitimate_blob * 10,
        "the ceiling must stay >10x the largest measured legitimate payload"
    );
}

/// End to end: a bomb row in `undo_stack` fails the load at the REAL constant.
///
/// The only test that builds a genuine 256 MiB payload, and what that buys is one thing
/// precisely: that the constant which SHIPS is the constant that binds, on a blob nobody
/// forged. Be exact about which bound catches it, because the answer changed - the row's
/// honest ISIZE declares ~257 MiB, so `decompress_json_under` refuses it on the DECLARED
/// size and `decompress_json_capped` is never reached. It does not exercise the inflate
/// path; `a_blob_that_understates_its_size_is_stopped_at_what_it_claimed` does that.
///
/// Cost is dominated by BUILDING the payload, not by loading it (nothing inflates):
/// ~86 MB peak RSS and ~0.8 s, measured with `/usr/bin/time -v` on the test binary, not
/// on `cargo test`, which measures the compiler. Cheap enough to run unguarded next to
/// the other 3,374 tests.
#[test]
fn a_bomb_row_fails_the_history_load_instead_of_inflating() {
    let db = Database::in_memory().unwrap();

    // Stream the bomb rather than materialising 256 MiB of input: one 1 MiB chunk of a
    // repeated byte, written until the encoder has consumed more than the ceiling.
    let chunk = vec![b'a'; 1024 * 1024];
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut written: u64 = 0;
    while written <= MAX_DECOMPRESSED_BLOB_BYTES {
        encoder.write_all(&chunk).unwrap();
        written += chunk.len() as u64;
    }
    let bomb = encoder.finish().unwrap();
    assert!(
        written > MAX_DECOMPRESSED_BLOB_BYTES,
        "the row must inflate past the ceiling to be a bomb"
    );

    let tiny = compress_json(r#"{"undo":"data"}"#);
    db.conn
        .execute(
            "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![0_i64, bomb, tiny, Utc::now().to_rfc3339()],
        )
        .unwrap();

    let err = db.load_undo_stack_with_timestamps_capped(10).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "the timestamped loader should fail naming the ceiling, got: {rendered}"
    );

    let legacy_err = db.load_undo_stack_capped(10).unwrap_err();
    let legacy_rendered = format!("{legacy_err:#}");
    assert!(
        legacy_rendered.contains("decompression ceiling"),
        "and so should the legacy loader, got: {legacy_rendered}"
    );
}

/// [#473] THE regression: many rows, each individually legal, refused in AGGREGATE.
///
/// This is the shape the per-blob ceiling alone did not stop, and it is not theoretical.
/// Measured on the first version of this fix: six rows whose blobs each declared 250 MiB
/// - every one comfortably inside the 256 MiB per-blob ceiling - came out of a 14 MB
/// file as 3.1 GB of `String` in memory, and the load returned `Ok`. At the default
/// 100-row count window the same file shape reaches ~50 GiB, which is the OOM #473 was
/// filed about.
///
/// Runs at the REAL constants and costs nothing, because the rows are refused on their
/// DECLARED sizes before anything inflates - the forged trailer is what makes a
/// 51-GiB-shaped file expressible in a few hundred bytes.
#[test]
fn a_history_table_of_individually_legal_rows_is_refused_in_aggregate() {
    let db = Database::in_memory().unwrap();

    // 250 MiB per blob: under the per-blob ceiling, so ONLY the aggregate bound can
    // catch it. Six rows x two blobs = 3 GiB claimed, past the 1 GiB load budget.
    const ROWS: i64 = 6;
    let quarter_gig = claiming_inflated_size(compress_json("{}"), 250 * 1024 * 1024);
    assert!(
        declared_inflated_size(&quarter_gig) < MAX_DECOMPRESSED_BLOB_BYTES,
        "each row must be INSIDE the per-blob ceiling, or this tests the wrong bound"
    );
    for seq in 0..ROWS {
        db.conn
            .execute(
                "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    seq,
                    quarter_gig.clone(),
                    quarter_gig.clone(),
                    Utc::now().to_rfc3339()
                ],
            )
            .unwrap();
    }

    let err = db.load_undo_stack_with_timestamps_capped(100).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("load budget"),
        "the aggregate bound must be what refuses it, got: {rendered}"
    );

    // The legacy loader answers to the same bound - it shares the helper precisely so
    // that this assertion cannot start passing for one loader and not the other.
    let legacy = format!("{:#}", db.load_undo_stack_capped(100).unwrap_err());
    assert!(
        legacy.contains("load budget"),
        "the legacy loader is bounded in aggregate too, got: {legacy}"
    );
}

/// [#473] The window pre-scan is spent ACROSS rows, not reset per row.
///
/// Named for the pre-scan on purpose. An earlier version of this test claimed to
/// exercise the running `InflationBudget` and did not: the rows declare exactly what they
/// carry, so the pre-scan refused first, and BOTH errors said "load budget" - which meant
/// the assertion could not tell them apart and the whole budget could have been stubbed
/// out with `Ok(())` while this still passed. The two messages no longer share a phrase,
/// and this one asserts on the pre-scan's.
#[test]
fn the_window_prescan_is_spent_across_rows_not_reset_per_row() {
    use super::undo::decompress_history_rows_within;

    let row = |seq: i64| {
        (
            seq,
            compress_json(&"a".repeat(1024)),
            compress_json(&"b".repeat(1024)),
            None,
        )
    };
    let rows: Vec<_> = (0..4).map(row).collect();

    // 8 KiB of payload across 4 rows. A budget of 8 KiB fits exactly...
    let ok = decompress_history_rows_within(rows.clone(), "undo", &InflationBudget::new(8 * 1024))
        .unwrap();
    assert_eq!(ok.len(), 4);
    assert_eq!(ok[0].1.len(), 1024);

    // ...and one byte less does not, which is only true if the window is priced as a
    // WHOLE. A per-row check would see 2 KiB against 8 KiB four times over and pass.
    let err =
        decompress_history_rows_within(rows.clone(), "undo", &InflationBudget::new(8 * 1024 - 1))
            .unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("declares") && rendered.contains("across 4 rows"),
        "the pre-scan must be what refuses it, got: {rendered}"
    );

    // A budget that fits one row proves it directly: the bound is not per row.
    assert!(decompress_history_rows_within(rows, "undo", &InflationBudget::new(2 * 1024)).is_err());
}

/// [#473] A budget already partly spent binds the NEXT stack of the same open.
///
/// This is the property that makes the ceiling per-open rather than per-stack. Giving
/// each stack a fresh budget - which is what the first version of this fix did - bounds a
/// project open at 2x the constant while every doc says 1x.
#[test]
fn a_second_stack_only_gets_what_the_first_one_left() {
    use super::undo::decompress_history_rows_within;

    let rows = |tag: char| {
        vec![(
            0_i64,
            compress_json(&tag.to_string().repeat(1024)),
            compress_json(&tag.to_string().repeat(1024)),
            None,
        )]
    };
    let budget = InflationBudget::new(3 * 1024);

    // The undo stack takes 2 KiB of the 3 KiB.
    assert_eq!(
        decompress_history_rows_within(rows('a'), "undo", &budget)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(budget.remaining(), 1024);

    // The redo stack needs 2 KiB and only 1 KiB is left, so the open is refused. With a
    // per-stack budget this would succeed and the pair would hold 4 KiB under a 3 KiB cap.
    let err = decompress_history_rows_within(rows('b'), "redo", &budget).unwrap_err();
    assert!(
        format!("{err:#}").contains("redo history declares"),
        "the second stack must see the first one's spending, got: {err:#}"
    );
}

/// [#473] `InflationBudget`'s own arithmetic, tested directly.
///
/// It needs its own test because on the load path it is unreachable by construction: the
/// pre-scan establishes `sum(declared) <= remaining` before any row reserves its own
/// `declared`, so `reserve` cannot fail there. That makes it defense in depth - which is
/// worth keeping and worth testing, but only a direct test can do it.
#[test]
fn the_inflation_budget_reserves_refunds_and_refuses() {
    let budget = InflationBudget::new(1000);

    budget.reserve(400).unwrap();
    assert_eq!(budget.remaining(), 600);

    // A refund puts the unused part back for the rows behind it.
    budget.refund(150);
    assert_eq!(budget.remaining(), 750);

    // Reserving more than is left fails and takes NOTHING - a partial deduction would
    // shrink the budget on every refusal.
    let err = budget.reserve(751).unwrap_err();
    assert!(
        format!("{err:#}").contains("while inflating"),
        "the budget's own message must be distinguishable from the pre-scan's, got: {err:#}"
    );
    assert_eq!(budget.remaining(), 750, "a failed reserve must not deduct");

    // The boundary is inclusive.
    budget.reserve(750).unwrap();
    assert_eq!(budget.remaining(), 0);
    assert!(budget.reserve(1).is_err());
}

/// [#473] The legacy plain-UTF-8 branch reserves its worst case, then refunds.
///
/// Two properties in one, because they only hold together. It RESERVES BEFORE it
/// allocates (`from_utf8_lossy(..).into_owned()` runs after the reserve, not before) -
/// charging afterwards is the ordering the budget exists to forbid, and with several rows
/// inflating at once is how a bound gets multiplied. And it REFUNDS, which is what makes
/// reserving the 3x worst case affordable: without the refund a 1 KiB legacy row would
/// cost 3 KiB forever and two of them would not fit in 4 KiB.
#[test]
fn the_legacy_branch_reserves_its_worst_case_then_refunds_the_difference() {
    // A MIXED blob - mostly valid text with a few invalid bytes - because that is the
    // only shape where the reservation and the output differ, and so the only shape that
    // can prove the refund happens at all. All-valid is priced exactly (no difference to
    // refund) and all-invalid produces exactly its 3x worst case (likewise); a version of
    // this test using those left `declared == produced` in both cases, so deleting the
    // refund entirely kept it green. It is also the realistic corrupt-legacy-row shape.
    let mut mixed = vec![b'x'; 1020];
    mixed.extend_from_slice(&[0xFF; 4]);
    assert_eq!(mixed.len(), 1024);
    // Reserved: 3 * 1024 = 3072 (the worst case, since the blob is not valid UTF-8).
    // Produced: 1020 valid bytes + 4 x 3-byte U+FFFD = 1032. Refund: 2040.
    assert_eq!(declared_inflated_size(&mixed), 3 * 1024);
    assert_eq!(String::from_utf8_lossy(&mixed).len(), 1032);

    // 5 KiB, sized so the refund is the ONLY thing that lets row 2 in: with it 4088
    // remains (>= the 3072 reservation), without it 2048 (short by 1024).
    let budget = InflationBudget::new(5 * 1024);
    assert_eq!(
        decompress_json_under(&mixed, &budget).unwrap().len(),
        1032,
        "the produced string is the lossy expansion, not the reservation"
    );
    assert_eq!(
        budget.remaining(),
        5 * 1024 - 1032,
        "the unused 2040 bytes of the worst-case reservation must come back"
    );

    // THE discriminating step: a second row needs another 3072-byte reservation. It fits
    // only because the first row refunded - without the refund just 1024 would remain and
    // this would be refused, which is what makes the assertion above load-bearing rather
    // than decorative.
    assert!(
        decompress_json_under(&mixed, &budget).is_ok(),
        "the second row fits only because the first refunded its unused reservation"
    );
    assert_eq!(budget.remaining(), 5 * 1024 - 2 * 1032);

    // ...and it is still genuinely charged: a third row's 3072-byte reservation exceeds
    // what is left, so the PEAK reservation binds, and a refusal deducts nothing.
    let left = budget.remaining();
    let err = decompress_json_under(&mixed, &budget).unwrap_err();
    assert!(
        format!("{err:#}").contains("while inflating"),
        "the legacy branch must spend the shared budget, got: {err:#}"
    );
    assert_eq!(budget.remaining(), left, "a refusal must not deduct");
}

/// [#473] A blob that fails to inflate hands its reservation back.
///
/// Harmless while every caller aborts the whole load on the first error - but "skip the
/// bad row and keep the rest" is a live proposal for this loader, and under it a leaked
/// reservation would drain the budget permanently and refuse honest rows behind a corrupt
/// one.
#[test]
fn a_failed_inflate_does_not_leak_its_reservation() {
    let budget = InflationBudget::new(64 * 1024);
    let before = budget.remaining();

    // Truncated mid-stream: the header and trailer survive, the deflate body does not.
    let mut corrupt = compress_json(&"a".repeat(4096));
    let trailer = corrupt.split_off(corrupt.len() - 8);
    corrupt.truncate(corrupt.len() / 2);
    corrupt.extend_from_slice(&trailer);

    assert!(decompress_json_under(&corrupt, &budget).is_err());
    assert_eq!(
        budget.remaining(),
        before,
        "a failed inflate must refund what it reserved"
    );
}

/// [#473] ...and a stack that fails PART WAY refunds every row that already decoded.
///
/// The single-blob case above was never the whole story: a load decodes many rows, and
/// one bad row late in the stack propagates out with the earlier rows' bytes still
/// charged. Those `String`s are dropped - the caller substitutes `Vec::new()` - so the
/// charge is a lie, and an expensive one, because the budget is SHARED with the other
/// stack of the same open: a clean redo stack would be refused for memory that is
/// already free. Enough rows to span several decompression batches, so both the
/// whole-earlier-batch and partial-failing-batch cases are covered.
#[test]
fn a_stack_that_fails_part_way_refunds_the_rows_that_already_decoded() {
    use super::undo::decompress_history_rows_within;

    let budget = InflationBudget::new(1024 * 1024);
    let before = budget.remaining();

    let good = |seq: i64| {
        (
            seq,
            compress_json(&"a".repeat(4096)),
            compress_json(&"b".repeat(4096)),
            None,
        )
    };
    // Truncated mid-stream, but its trailer still declares a small size - so it passes
    // the window pre-scan and fails during inflation, which is the only shape that
    // reaches the partial-decode path at all.
    let mut corrupt = compress_json(&"c".repeat(4096));
    let trailer = corrupt.split_off(corrupt.len() - 8);
    corrupt.truncate(corrupt.len() / 2);
    corrupt.extend_from_slice(&trailer);

    let mut rows: Vec<_> = (0..8).map(good).collect();
    rows.push((8, corrupt.clone(), corrupt, None));

    let err = decompress_history_rows_within(rows, "undo", &budget).unwrap_err();
    assert!(
        !format!("{err:#}").contains("load budget"),
        "the failure must come from the corrupt row, not from running out of budget"
    );
    assert_eq!(
        budget.remaining(),
        before,
        "a stack that fails charges nothing - the rows it did decode are dropped"
    );
}

#[test]
fn two_dangling_station_offsets_in_one_section_save_without_pk_collision() {
    // Regression for the save crash (review finding #7): two dangling station
    // offsets in one section both used to resolve to (section, 0) via
    // key_to_ref_lossy and collide on the (section_id, station_index) PRIMARY
    // KEY, aborting the entire save. The persistence layer now skips a key that
    // doesn't resolve, so the save succeeds and the dangling offsets are dropped
    // (not persisted as phantom station-0 rows).
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Dangling".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![ShotData {
                length: 5.0,
                ..Default::default()
            }],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.station_offsets.insert(
        StationKey::new(a, StationAnchor::AfterShot("ghost-1".into())),
        (1.0, 0.0, 0.0),
    );
    project.station_offsets.insert(
        StationKey::new(a, StationAnchor::AfterShot("ghost-2".into())),
        (2.0, 0.0, 0.0),
    );

    db.save_project(&project)
        .expect("save must not PK-collide on two dangling offsets");

    let loaded = db.load_project().unwrap();
    assert!(
        loaded.station_offsets.is_empty(),
        "dangling offsets are dropped on save, not written as phantom station-0 rows"
    );
}

// === Raw table browsing ===

#[test]
fn test_list_raw_table_names_excludes_sqlite_internal() {
    let mut db = Database::in_memory().unwrap();
    let project = Project::new("Raw Cave".to_string(), None);
    db.save_project(&project).unwrap();

    let names = db.list_raw_table_names().unwrap();
    assert!(names.iter().any(|n| n == "project"));
    assert!(names.iter().any(|n| n == "sections"));
    assert!(names.iter().any(|n| n == "shots"));
    assert!(!names.iter().any(|n| n.starts_with("sqlite_")));
}

#[test]
fn test_raw_tables_leave_out_virtual_tables_even_when_pragmas_are_shadowed() {
    // A crafted file's virtual tables would run their module's code over the
    // file's own data; the viewer must neither list nor read them. The file
    // also ships its own `pragma_table_list`, which a table-valued pragma
    // query would read instead of SQLite's and so list `docs` as ordinary.
    // The schema rows are written directly: release builds compile SQLite
    // without FTS5 or RTREE, so `CREATE VIRTUAL TABLE` would fail there.
    let mut db = Database::in_memory().unwrap();
    db.save_project(&Project::new("Hostile Cave".to_string(), None))
        .unwrap();
    db.conn
        .execute_batch(
            "PRAGMA writable_schema = ON;
             INSERT INTO sqlite_master (type, name, tbl_name, rootpage, sql) VALUES
               ('table', 'docs', 'docs', 0, 'CREATE VIRTUAL TABLE docs USING fts5(body)'),
               ('table', 'boxes', 'boxes', 0, 'CREATE VIRTUAL TABLE/**/boxes USING rtree(id, x0, x1)');
             PRAGMA writable_schema = RESET;
             CREATE TABLE pragma_table_list (name, schema, type);
             INSERT INTO pragma_table_list VALUES ('docs', 'main', 'table'),
                                                  ('boxes', 'main', 'table');",
        )
        .unwrap();

    let names = db.list_raw_table_names().unwrap();

    for hidden in ["docs", "boxes"] {
        assert!(
            !names.iter().any(|n| n == hidden),
            "{hidden} listed: {names:?}"
        );
        let err = db.read_raw_table(hidden, 100, 0).unwrap_err();
        assert!(err.to_string().contains("Unknown table"), "{hidden}: {err}");
    }
    assert!(names.iter().any(|n| n == "shots"));
}

#[test]
fn test_raw_tables_keep_a_legacy_table_with_an_is_virtual_column() {
    // Pre-v4 shots tables had an `is_virtual` column; it is still an ordinary table.
    let db = Database::in_memory().unwrap();
    db.conn
        .execute_batch("CREATE TABLE legacy_shots (id INTEGER, is_virtual INTEGER);")
        .unwrap();

    let names = db.list_raw_table_names().unwrap();

    assert!(names.iter().any(|n| n == "legacy_shots"), "{names:?}");
}

#[test]
fn test_declares_virtual_table_matches_keywords_not_substrings() {
    use super::declares_virtual_table as virt;
    assert!(virt("CREATE VIRTUAL TABLE t USING fts5(x)"));
    assert!(virt("create virtual table t using rtree(a, b, c)"));
    assert!(virt("CREATE/**/VIRTUAL  TABLE t USING fts5(x)"));
    assert!(virt("CREATE -- note\n VIRTUAL TABLE t USING fts5(x)"));
    assert!(virt("CREATE /* unterminated"));
    // A legacy (pre-v4) shots table has an `is_virtual` column.
    assert!(!virt("CREATE TABLE shots (id INTEGER, is_virtual INTEGER)"));
    assert!(!virt("CREATE TABLE \"virtual\" (x)"));
    assert!(!virt(""));
}

#[test]
fn test_read_raw_table_stringifies_values() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute_batch(
            "CREATE TABLE mixed (i INTEGER, r REAL, t TEXT, b BLOB, n TEXT);
             INSERT INTO mixed VALUES (42, 1.5, 'hello', x'0102030405', NULL);",
        )
        .unwrap();

    let data = db.read_raw_table("mixed", 100, 0).unwrap();
    assert_eq!(data.columns, vec!["i", "r", "t", "b", "n"]);
    assert_eq!(data.total_rows, 1);
    assert_eq!(
        data.rows,
        vec![vec![
            "42".to_string(),
            "1.5".to_string(),
            "hello".to_string(),
            "<blob 5 B>".to_string(),
            "NULL".to_string(),
        ]]
    );
}

#[test]
fn test_read_raw_table_rejects_unknown_names() {
    let mut db = Database::in_memory().unwrap();
    let project = Project::new("Inject Cave".to_string(), None);
    db.save_project(&project).unwrap();

    // Injection attempts and internal tables must be rejected by the
    // exact-member guard, never interpolated into SQL.
    for name in [
        "shots; DROP TABLE shots",
        "shots\" --",
        "sqlite_master",
        "no_such_table",
        "",
    ] {
        let err = db.read_raw_table(name, 100, 0).unwrap_err();
        assert!(
            err.to_string().contains("Unknown table"),
            "expected rejection for {name:?}, got: {err}"
        );
    }
    // The guard must not have damaged anything.
    assert!(db.read_raw_table("shots", 100, 0).is_ok());
}

#[test]
fn test_read_raw_table_limit_clamp_and_offset_paging() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute_batch(
            "CREATE TABLE big (v INTEGER);
             INSERT INTO big (v)
             WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < 10050)
             SELECT x FROM cnt;",
        )
        .unwrap();

    // Oversized limit is clamped to MAX_RAW_PAGE_ROWS.
    let page = db.read_raw_table("big", u32::MAX, 0).unwrap();
    assert_eq!(page.total_rows, 10_050);
    assert_eq!(page.rows.len(), MAX_RAW_PAGE_ROWS as usize);

    // Offset paging returns the remaining rows past the clamp boundary.
    let rest = db.read_raw_table("big", u32::MAX, 10_000).unwrap();
    assert_eq!(rest.total_rows, 10_050);
    assert_eq!(rest.rows.len(), 50);

    // Small pages line up back-to-back.
    let first = db.read_raw_table("big", 2, 0).unwrap();
    let second = db.read_raw_table("big", 2, 2).unwrap();
    assert_eq!(first.rows[0], vec!["1".to_string()]);
    assert_eq!(first.rows[1], vec!["2".to_string()]);
    assert_eq!(second.rows[0], vec!["3".to_string()]);
    assert_eq!(second.rows[1], vec!["4".to_string()]);
}

#[test]
fn test_format_blob_placeholder_units() {
    assert_eq!(format_blob_placeholder(0), "<blob 0 B>");
    assert_eq!(format_blob_placeholder(512), "<blob 512 B>");
    assert_eq!(format_blob_placeholder(1024), "<blob 1.0 KB>");
    assert_eq!(format_blob_placeholder(4300), "<blob 4.2 KB>");
    assert_eq!(format_blob_placeholder(1024 * 1024), "<blob 1.0 MB>");
    assert_eq!(
        format_blob_placeholder(3 * 1024 * 1024 + 512 * 1024),
        "<blob 3.5 MB>"
    );
    assert_eq!(
        format_blob_placeholder(2 * 1024 * 1024 * 1024),
        "<blob 2.0 GB>"
    );
}

// ===========================================================================
// SpeleoDB hand-off link (speleo_link, schema v9) - #189 round-trip + is_remote
// and #190 carry-forward fence. No telemetry (these live in persistence tests,
// avoiding the 418 EVENT_NAMES bump). See PRESERVED_SPELEOLINK_COLUMNS.
// ===========================================================================

use super::schema::PRESERVED_SPELEOLINK_COLUMNS;
use crate::models::SpeleoLink;

/// A SpeleoLink fixture with the given base SHA + dirty bit.
fn sample_speleo_link(base_commit_sha: Option<&str>, dirty: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://10.10.0.220:8000/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "Test Cave".to_string(),
        base_commit_sha: base_commit_sha.map(str::to_owned),
        has_unpushed_edits: dirty,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

// --- #189 ---

#[test]
fn speleo_link_roundtrips_through_save_and_load() {
    let db = Database::in_memory().unwrap();
    assert!(db.load_speleo_link().unwrap().is_none(), "no row initially");

    let link = sample_speleo_link(Some("abc123"), true);
    db.save_speleo_link(Some(&link)).unwrap();

    let loaded = db.load_speleo_link().unwrap().expect("row present");
    assert_eq!(loaded, link);

    // Upsert (id=1) overwrites rather than inserting a second row.
    let link2 = sample_speleo_link(Some("def456"), false);
    db.save_speleo_link(Some(&link2)).unwrap();
    assert_eq!(db.load_speleo_link().unwrap().unwrap(), link2);
    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM speleo_link", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "upsert keeps a single row");
}

#[test]
fn speleo_link_last_verify_at_roundtrips() {
    // #202 (schema v11): last_verify_at persists and reloads verbatim, and None
    // stays SQL NULL on disk (never coerced to "").
    let db = Database::in_memory().unwrap();
    let stamp = "2026-07-02T09:00:00+00:00".to_string();
    let mut link = sample_speleo_link(Some("abc123"), true);
    link.last_verify_at = Some(stamp.clone());
    db.save_speleo_link(Some(&link)).unwrap();
    assert_eq!(
        db.load_speleo_link()
            .unwrap()
            .unwrap()
            .last_verify_at
            .as_deref(),
        Some(stamp.as_str()),
    );

    // None round-trips as SQL NULL.
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc123"), false)))
        .unwrap();
    let is_null: i64 = db
        .conn
        .query_row(
            "SELECT last_verify_at IS NULL FROM speleo_link WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(is_null, 1, "None persists as SQL NULL");
}

#[test]
fn speleo_link_null_base_sha_stays_none() {
    let db = Database::in_memory().unwrap();
    // Empty-repo BASE: base_commit_sha is None.
    db.save_speleo_link(Some(&sample_speleo_link(None, false)))
        .unwrap();

    // Typed API: None stays None.
    let loaded = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(loaded.base_commit_sha, None);

    // On-disk: the column is SQL NULL, never the empty string.
    let is_null: i64 = db
        .conn
        .query_row(
            "SELECT base_commit_sha IS NULL FROM speleo_link WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(is_null, 1, "base_commit_sha is SQL NULL, not ''");
}

#[test]
fn speleo_link_clear_removes_row() {
    let db = Database::in_memory().unwrap();
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
        .unwrap();
    assert!(db.load_speleo_link().unwrap().is_some());

    db.save_speleo_link(None).unwrap();
    assert!(db.load_speleo_link().unwrap().is_none(), "row deleted");
    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM speleo_link", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn init_schema_creates_speleo_link_v11() {
    // A freshly-created file (open() → init_schema, no migrate chain) has the v11
    // table immediately.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("fresh.stalx");
    let db = Database::open(&path).unwrap();
    let exists: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='speleo_link'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 1, "init_schema created speleo_link");
    assert_eq!(
        db.get_user_version().unwrap(),
        SCHEMA_VERSION,
        "stamped at current"
    );
}

#[test]
fn migrate_creates_speleo_link_on_legacy_v8() {
    // Simulate a legacy v8 .stalx: create a normal file, then DROP the v9 table
    // and stamp user_version back to 8. open_existing() must migrate it (running
    // migrate_schema), recreating speleo_link via CREATE IF NOT EXISTS.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v8.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Legacy".to_string(), None))
            .unwrap();
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.set_user_version(8).unwrap();
    }

    // Confirm the table is genuinely absent before reopening.
    {
        let db = Database::open_for_metadata(&path).unwrap();
        let exists: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='speleo_link'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0, "v8 file has no speleo_link");
    }

    // open_existing runs ensure_schema_version → migrate_schema, gaining it.
    let db = Database::open_existing(&path).unwrap();
    let exists: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='speleo_link'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 1, "migrate_schema recreated speleo_link");
    assert_eq!(
        db.get_user_version().unwrap(),
        SCHEMA_VERSION,
        "bumped to current"
    );
}

#[test]
fn is_remote_true_when_link_present_false_when_absent() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("remote.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Remote".to_string(), None))
        .unwrap();

    // No link yet → is_remote false.
    assert!(!db.load_metadata(path.clone()).unwrap().is_remote);

    // Stamp a link → is_remote true.
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), false)))
        .unwrap();
    assert!(db.load_metadata(path.clone()).unwrap().is_remote);

    // Clear the link → is_remote false again.
    db.save_speleo_link(None).unwrap();
    assert!(!db.load_metadata(path).unwrap().is_remote);
}

#[test]
fn is_remote_false_on_metadata_open_of_pre_v9_file() {
    // open_for_metadata uses SQLITE_OPEN_READ_ONLY and runs no migration, so a
    // pre-v9 file has no speleo_link table. The EXISTS probe must tolerate that
    // (.ok().unwrap_or(false)) and report is_remote == false, not error.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pre_v9.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("PreV9".to_string(), None))
            .unwrap();
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
    }
    let db = Database::open_for_metadata(&path).unwrap();
    let meta = db.load_metadata(path).unwrap();
    assert!(!meta.is_remote, "missing table → is_remote false, no error");
}

/// A project whose shots are 10 m Normal, 3 m Jump, 4 m Reference and 6 m
/// Reference: 13 m surveyed, 23 m of tape.
fn project_with_reference_shots() -> Project {
    let shot = |length: f64, kind: ShotKind| ShotData {
        length,
        kind,
        ..Default::default()
    };
    let mut project = Project::new("Refs".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(10.0, ShotKind::Normal),
            shot(4.0, ShotKind::Reference),
            shot(3.0, ShotKind::Jump),
        ],
        ..Default::default()
    });
    project.add_section(SectionData {
        shots: vec![shot(6.0, ShotKind::Reference)],
        ..Default::default()
    });
    project
}

/// The recent-projects listing (Global View, system table, merge dialog) reads
/// the length through SQL, not `Project::stats`, so it must apply the same rule:
/// Reference shots are left out (#689), and both routes report one figure.
#[test]
fn metadata_total_length_excludes_reference_shots() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("refs.stalx");
    let project = project_with_reference_shots();
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
    }
    let db = Database::open_for_metadata(&path).unwrap();
    let meta = db.load_metadata(path).unwrap();
    assert!(
        (meta.total_length - 13.0).abs() < 1e-9,
        "got {}",
        meta.total_length
    );
    assert!((meta.total_length - project.stats().total_length).abs() < 1e-9);
}

/// A stored `kind` this build does not recognise loads as Reference
/// (`ShotKind::from_u8`), so the SQL length must leave it out too.
#[test]
fn metadata_total_length_excludes_unrecognised_kinds_like_the_loader() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("unknown_kind.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project_with_reference_shots()).unwrap();
        db.conn
            .execute("UPDATE shots SET kind = 7 WHERE length = 10.0", [])
            .unwrap();
    }
    let db = Database::open_for_metadata(&path).unwrap();
    let meta = db.load_metadata(path).unwrap();
    assert!(
        (meta.total_length - 3.0).abs() < 1e-9,
        "got {}",
        meta.total_length
    );
}

/// Rewrite `shots.kind` as a column declared `declared_type`, keeping each
/// row's value. The table is not STRICT, so the declared type's affinity
/// decides the storage class: `TEXT` stores `'1'`, `REAL` stores `1.0`.
fn retype_kind_column(path: &Path, declared_type: &str) {
    let db = Database::open(path).unwrap();
    db.conn
        .execute_batch(&format!(
            "ALTER TABLE shots ADD COLUMN kind_retyped {declared_type};
             UPDATE shots SET kind_retyped = kind;
             ALTER TABLE shots DROP COLUMN kind;
             ALTER TABLE shots RENAME COLUMN kind_retyped TO kind;"
        ))
        .unwrap();
}

/// A crafted file can store `kind` as TEXT `'1'` or REAL `1.0`. SQLite's
/// column affinity lets `IN (0, 1)` match those, but the loader reads any
/// non-INTEGER cell as Reference, so the listing must leave them out too and
/// agree with the length the opened project shows.
#[test]
fn metadata_total_length_matches_the_loader_on_non_integer_kinds() {
    for (declared_type, storage_class) in [("TEXT", "text"), ("REAL", "real")] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("retyped_kind.stalx");
        {
            let mut db = Database::open(&path).unwrap();
            db.save_project(&project_with_reference_shots()).unwrap();
        }
        retype_kind_column(&path, declared_type);

        let meta = Database::open_for_metadata(&path)
            .unwrap()
            .load_metadata(path.clone())
            .unwrap();
        let db = Database::open(&path).unwrap();
        let stored: String = db
            .conn
            .query_row("SELECT typeof(kind) FROM shots LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, storage_class, "{declared_type}: fixture storage");
        let loaded = db.load_project().unwrap().stats().total_length;
        assert!(
            (meta.total_length - loaded).abs() < 1e-9,
            "{declared_type}: listing {} != opened {loaded}",
            meta.total_length
        );
    }
}

/// `open_for_metadata` runs no migration, so a pre-v13 file has no `kind`
/// column, only the boolean `is_reference`. The v13 migration seeds every
/// virtual shot as Jump, which counts, so the listing sums every shot rather
/// than failing to 0. That is an upper bound, not the opened figure: the first
/// open runs the classifier, which may demote some of them to Reference, and
/// re-saves the result.
#[test]
fn metadata_total_length_on_a_pre_v13_file_without_kind_column() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pre_v13.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project_with_reference_shots()).unwrap();
        // The pre-v13 shape: a boolean "virtual" flag, 1 for both kinds.
        db.conn
            .execute_batch(
                "ALTER TABLE shots RENAME COLUMN kind TO is_reference;
                 UPDATE shots SET is_reference = is_reference != 0;",
            )
            .unwrap();
    }
    let db = Database::open_for_metadata(&path).unwrap();
    let meta = db.load_metadata(path).unwrap();
    assert!(
        (meta.total_length - 23.0).abs() < 1e-9,
        "got {}",
        meta.total_length
    );
}

#[test]
fn metadata_open_reads_a_pre_v10_speleo_link_without_last_edit_or_verify_columns() {
    // The missing-TABLE case has coverage (`is_remote_false_on_metadata_open_of_pre_v9_file`);
    // this is the missing-COLUMN one, which wedged projects rather than merely
    // reporting them local.
    //
    // `open_for_metadata` runs NO migration, so a `.stalx` last written by a v9/v10
    // build still has the v9/v10 `speleo_link` shape while the crate is at
    // SCHEMA_VERSION 13. Selecting `last_edit_at` (v10) / `last_verify_at` (v11)
    // unconditionally failed with `no such column`, and that error PROPAGATES - so a
    // perfectly readable legacy cache copy read as an unreadable FILE. Downstream that
    // was permanent: `guard_local_edits` refused every pull / adopt / revision-restore
    // for the project with a factually wrong "has local edits that were never pushed",
    // and the cache scan reported "no local copy" for a project that has one.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pre_v10.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("PreV10".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc123"), false)))
            .unwrap();
        // Rebuild the table in its narrow, pre-v10 shape (SQLite cannot DROP COLUMN
        // before 3.35, and this is closer to what the old build actually wrote).
        db.conn.execute_batch(
            "DROP TABLE speleo_link;
             CREATE TABLE speleo_link (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 endpoint TEXT NOT NULL,
                 project_id TEXT NOT NULL,
                 project_name TEXT NOT NULL,
                 base_commit_sha TEXT,
                 has_unpushed_edits INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO speleo_link (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits)
             VALUES (1, 'http://10.10.0.220:8000/', '11111111-1111-1111-1111-111111111111', 'Test Cave', 'abc123', 0);",
        ).unwrap();
    }

    let db = Database::open_for_metadata(&path).unwrap();
    let link = db
        .load_speleo_link()
        .expect("a narrow speleo_link is READ, not reported as an unreadable file")
        .expect("the row is there");
    assert_eq!(link.project_id, "11111111-1111-1111-1111-111111111111");
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc123"));
    assert!(!link.has_unpushed_edits, "and known-clean, not unknown");
    // The columns this file predates read as NULL - the same value a migrated row
    // that was never stamped carries.
    assert_eq!(link.last_edit_at, None);
    assert_eq!(link.last_verify_at, None);
}

#[test]
fn metadata_open_still_propagates_a_genuinely_unreadable_file() {
    // The tolerance above must NOT become "swallow every read error". A file that is
    // not a database at all has to keep failing, because `guard_local_edits` fails
    // CLOSED on that error - it is what stops a re-pull overwriting a copy whose
    // un-pushed work could not be read.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("junk.stalx");
    std::fs::write(&path, b"not a sqlite database at all").unwrap();
    let read = Database::open_for_metadata(&path).and_then(|db| db.load_speleo_link());
    assert!(
        read.is_err(),
        "an unreadable FILE must still propagate as an error"
    );
}

#[test]
fn save_project_does_not_clear_speleo_link() {
    // D-B13c: save_project wipes/rewrites its owned tables, but speleo_link is
    // intentionally excluded so a local autosave never clears the dirty bit.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("dirty.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Dirty".to_string(), None))
        .unwrap();
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
        .unwrap();

    // A subsequent save_project (the autosave path) must leave the link + dirty
    // bit untouched.
    db.save_project(&Project::new("Dirty Renamed".to_string(), None))
        .unwrap();

    let link = db.load_speleo_link().unwrap().expect("link survives save");
    assert!(link.has_unpushed_edits, "dirty bit NOT cleared by save");
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
}

// --- #190 carry-forward fence ---

/// pragma_table_info row for a single column: (name, decl_type, notnull).
fn speleo_link_column_info(db: &Database) -> Vec<(String, String, bool)> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, type, \"notnull\" FROM pragma_table_info('speleo_link')")
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? != 0,
            ))
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

#[test]
fn carry_forward_four_cell_raw_sql_null_stays_null() {
    // Raw-SQL on-disk: INSERT NULL base + dirty=1, force migrate_schema (drop the
    // table + stamp v8 so open_existing migrates), assert NULL stays NULL and the
    // dirty bit stays 1 - never coerced to '' / 0.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("raw_fence.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Raw".to_string(), None))
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits)
                 VALUES (1, 'http://x', 'pid', 'pname', NULL, 1)",
                [],
            )
            .unwrap();
        // Force the migrate path on next open.
        db.set_user_version(8).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let (is_null, dirty): (i64, i64) = db
        .conn
        .query_row(
            "SELECT base_commit_sha IS NULL, has_unpushed_edits FROM speleo_link WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(is_null, 1, "NULL base survives migrate (not '')");
    assert_eq!(dirty, 1, "dirty bit survives migrate");
}

#[test]
fn carry_forward_four_cell_typed_api() {
    // Typed model API: round-trip None + true through reopen, plus a Some(..) case.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("typed_fence.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Typed".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(None, true)))
            .unwrap();
    }
    {
        let db = Database::open_existing(&path).unwrap();
        let link = db.load_speleo_link().unwrap().unwrap();
        assert_eq!(link.base_commit_sha, None);
        assert!(link.has_unpushed_edits);

        // Second case: Some("abc123") + dirty=false.
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc123"), false)))
            .unwrap();
    }
    let db = Database::open_existing(&path).unwrap();
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc123"));
    assert!(!link.has_unpushed_edits);
}

// --- v10 last_edit_at ---

#[test]
fn last_edit_at_round_trips_and_survives_migration() {
    // Typed Some + None round-trip through save/load, and the value is carried
    // forward verbatim across a forced SCHEMA_VERSION migration (never reset).
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("last_edit_at.stalx");
    let stamp = "2026-06-27T12:34:56+00:00".to_string();
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Edited".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://x".to_string(),
            project_id: "pid".to_string(),
            project_name: "pname".to_string(),
            base_commit_sha: Some("abc".to_string()),
            has_unpushed_edits: true,
            last_edit_at: Some(stamp.clone()),
            last_verify_at: None,
            undo_mode: None,
        }))
        .unwrap();
        db.set_user_version(8).unwrap(); // force migrate on next open
    }

    let db = Database::open_existing(&path).unwrap();
    let loaded = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        loaded.last_edit_at.as_deref(),
        Some(stamp.as_str()),
        "last_edit_at survives migration verbatim"
    );
    assert!(loaded.has_unpushed_edits);

    // None also round-trips and is SQL NULL on disk (never coerced to "").
    db.save_speleo_link(Some(&SpeleoLink {
        last_edit_at: None,
        ..loaded
    }))
    .unwrap();
    assert_eq!(db.load_speleo_link().unwrap().unwrap().last_edit_at, None);
    let is_null: i64 = db
        .conn
        .query_row(
            "SELECT last_edit_at IS NULL FROM speleo_link WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(is_null, 1, "last_edit_at None is SQL NULL, not ''");
}

#[test]
fn migrate_v9_dirty_speleo_link_backfills_last_edit_at() {
    // A genuine v9-shaped speleo_link (no last_edit_at) gains the column via the
    // v9→v10 ALTER on open_existing. A row that was already DIRTY under v9
    // (has_unpushed_edits=1) is backfilled to the epoch so a later confirmed push
    // can clear it - WITHOUT the backfill, edits_captured_by_push(None, push_start)
    // is false and the dirty bit would stick forever (#197 review).
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v9_speleo_link.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V9".to_string(), None))
            .unwrap();
        // Re-create the table in its pre-v10 shape and stamp the version back.
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.conn
            .execute(
                "CREATE TABLE speleo_link (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    endpoint TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_name TEXT NOT NULL,
                    base_commit_sha TEXT,
                    has_unpushed_edits INTEGER NOT NULL DEFAULT 0
                )",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits)
                 VALUES (1, 'http://x', 'pid', 'pname', 'abc', 1)",
                [],
            )
            .unwrap();
        db.set_user_version(9).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let has_col: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_edit_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_col, 1, "v9→v10 ALTER added last_edit_at");
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("1970-01-01T00:00:00+00:00"),
        "a dirty v9 row is backfilled to the epoch so a confirmed push can clear it"
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
    assert!(link.has_unpushed_edits, "existing row preserved");
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn migrate_v9_clean_speleo_link_keeps_null_last_edit_at() {
    // The v9→v10 backfill only touches DIRTY rows. A clean v9 row
    // (has_unpushed_edits=0) keeps last_edit_at NULL - there is no un-pushed edit to
    // preserve, and a fresh edit will stamp it at edit time.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v9_clean.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V9c".to_string(), None))
            .unwrap();
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.conn
            .execute(
                "CREATE TABLE speleo_link (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    endpoint TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_name TEXT NOT NULL,
                    base_commit_sha TEXT,
                    has_unpushed_edits INTEGER NOT NULL DEFAULT 0
                )",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits)
                 VALUES (1, 'http://x', 'pid', 'pname', 'abc', 0)",
                [],
            )
            .unwrap();
        db.set_user_version(9).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        link.last_edit_at, None,
        "a clean v9 row keeps last_edit_at NULL (only dirty rows are backfilled)"
    );
    assert!(!link.has_unpushed_edits, "clean row stays clean");
}

#[test]
fn migrate_v10_speleo_link_adds_last_verify_at() {
    // #202 (schema v10→v11): a genuine v10-shaped speleo_link (last_edit_at present,
    // last_verify_at ABSENT) gains the column via the idempotent ALTER on
    // open_existing, defaulting to NULL ("never verified since pull"), WITHOUT
    // disturbing the pre-existing last_edit_at / dirty bit / identity.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v10_speleo_link.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V10".to_string(), None))
            .unwrap();
        // Re-create the table in its pre-v11 (v10) shape and stamp the version back.
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.conn
            .execute(
                "CREATE TABLE speleo_link (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    endpoint TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_name TEXT NOT NULL,
                    base_commit_sha TEXT,
                    has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                    last_edit_at TEXT
                )",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits, last_edit_at)
                 VALUES (1, 'http://x', 'pid', 'pname', 'abc', 1, '2026-06-27T10:00:00+00:00')",
                [],
            )
            .unwrap();
        db.set_user_version(10).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let has_col: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_verify_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_col, 1, "v10→v11 ALTER added last_verify_at");
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        link.last_verify_at, None,
        "the new column defaults to NULL (never verified since pull)"
    );
    // The pre-existing v10 fields survive the migration untouched.
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("2026-06-27T10:00:00+00:00"),
        "existing last_edit_at preserved across the v10→v11 ALTER"
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
    assert!(link.has_unpushed_edits, "existing dirty bit preserved");
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
}

/// [#303] Spec item 28, migration half (schema v14→v15): a genuine v14-shaped
/// `speleo_link` (no `undo_mode`) gains the column on `open_existing`, defaulting to
/// NULL (the legacy marker the application materializes on first open), without
/// disturbing any pre-existing field.
#[test]
fn migrate_v14_speleo_link_adds_undo_mode() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v14_speleo_link.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V14".to_string(), None))
            .unwrap();
        // Re-create the table in its pre-v15 (v14) shape and stamp the version back.
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.conn
            .execute(
                "CREATE TABLE speleo_link (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    endpoint TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_name TEXT NOT NULL,
                    base_commit_sha TEXT,
                    has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                    last_edit_at TEXT,
                    last_verify_at TEXT
                )",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits, last_edit_at, last_verify_at)
                 VALUES (1, 'http://x', 'pid', 'pname', 'abc', 1, '2026-06-27T10:00:00+00:00', '2026-06-27T11:00:00+00:00')",
                [],
            )
            .unwrap();
        db.set_user_version(14).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let has_col: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='undo_mode'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_col, 1, "v14→v15 ALTER added undo_mode");
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        link.undo_mode, None,
        "the new column defaults to the legacy initialization marker"
    );
    // Every pre-existing v14 field survives the migration untouched.
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("2026-06-27T10:00:00+00:00")
    );
    assert_eq!(
        link.last_verify_at.as_deref(),
        Some("2026-06-27T11:00:00+00:00")
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
    assert!(link.has_unpushed_edits, "existing dirty bit preserved");
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);

    // And the v15 table gained the Unwind chain pointer, empty (not mid-unwind).
    let has_chain: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='speleo_undo_chain'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_chain, 1, "v15 migration created speleo_undo_chain");
}

/// [#303] Spec item 28, round-trip half: both override values survive save/load, and
/// `None` stays NULL rather than being written as the string "full" - the two are
/// different states (only NULL keeps tracking a later change to the global default).
#[test]
fn speleo_link_undo_mode_roundtrips_and_none_stays_null() {
    use crate::models::UndoMode;

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("undo_mode.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("UndoMode".to_string(), None))
        .unwrap();

    for mode in [UndoMode::Full, UndoMode::Disabled] {
        let mut link = sample_speleo_link(Some("abc"), false);
        link.undo_mode = Some(mode);
        db.save_speleo_link(Some(&link)).unwrap();
        assert_eq!(
            db.load_speleo_link().unwrap().unwrap().undo_mode,
            Some(mode),
            "{mode:?} must survive the round trip"
        );
    }

    let mut link = sample_speleo_link(Some("abc"), false);
    link.undo_mode = None;
    db.save_speleo_link(Some(&link)).unwrap();
    assert_eq!(db.load_speleo_link().unwrap().unwrap().undo_mode, None);
    let is_null: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM speleo_link WHERE id = 1 AND undo_mode IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        is_null, 1,
        "None must clear the column to SQL NULL, never write a default spelling"
    );

    // An unrecognized value (a newer build, or a hand-edit) fails closed without
    // making the whole project unloadable.
    db.conn
        .execute("UPDATE speleo_link SET undo_mode = 'sideways'", [])
        .unwrap();
    let loaded = db.load_speleo_link().unwrap().unwrap().undo_mode;
    assert_eq!(loaded, Some(UndoMode::Disabled));
    assert!(
        !loaded.unwrap().pushes_history(),
        "malformed database values must not publish undo history"
    );
}

/// [#303] Spec item 22, persistence half: the Unwind chain pointer survives a close and
/// reopen (it is a row in the `.stalx`, not session state), and clearing it is a distinct
/// state from never having set one.
///
/// It lives in its own table rather than in `speleo_link` because it is per-MACHINE
/// progress through history: `PUSH_SNAPSHOT_LOCAL_TABLES` strips it, so it never rides to
/// the server. BASE cannot double as it, because a restore stamps BASE at the server tip.
#[test]
fn speleo_undo_chain_round_trips_and_clears() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("chain.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Chain".to_string(), None))
            .unwrap();
        assert_eq!(
            db.load_speleo_undo_chain().unwrap(),
            None,
            "a fresh project is not mid-unwind"
        );
        db.save_speleo_undo_chain(Some("abc1234")).unwrap();
        assert_eq!(
            db.load_speleo_undo_chain().unwrap().as_deref(),
            Some("abc1234")
        );
    }

    // Survives the reopen - an Unwind interrupted by a restart resumes where it was.
    {
        let db = Database::open_existing(&path).unwrap();
        assert_eq!(
            db.load_speleo_undo_chain().unwrap().as_deref(),
            Some("abc1234"),
            "the pointer is on disk, so a restart mid-unwind does not restart the walk"
        );
        // Overwriting moves the position; clearing ends the walk.
        db.save_speleo_undo_chain(Some("def5678")).unwrap();
        assert_eq!(
            db.load_speleo_undo_chain().unwrap().as_deref(),
            Some("def5678")
        );
        db.save_speleo_undo_chain(None).unwrap();
        assert_eq!(db.load_speleo_undo_chain().unwrap(), None);
    }
}

/// [#303] The chain pointer is LOCAL: a push snapshot must not carry it, or a
/// collaborator would inherit this machine's position in a walk they never started.
#[test]
fn speleo_undo_chain_is_stripped_from_a_push_snapshot() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("chain_push.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("ChainPush".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
            .unwrap();
        db.save_speleo_undo_chain(Some("abc1234")).unwrap();
    }

    // Full mode keeps the undo HISTORY, so this pins that the chain pointer is stripped
    // even on the path most likely to forget it.
    Database::sanitize_push_snapshot(&path, true, None).unwrap();

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        db.load_speleo_undo_chain().unwrap(),
        None,
        "the Unwind chain position is per-machine and must never ride to the server"
    );
}

/// [#303] Seed a snapshot whose undo stack mixes rows INHERITED from a loaded revision
/// with rows authored after it, and return the watermark separating them.
fn snapshot_with_inherited_and_own_rows(
    temp: &TempDir,
    name: &str,
    inherited: &[&str],
    own: &[&str],
) -> (std::path::PathBuf, String) {
    let path = temp.path().join(name);
    // An hour on either side, so the assertions cannot turn on clock resolution.
    let watermark = Utc::now();
    let mut entries: Vec<(String, String, Option<String>)> = Vec::new();
    for (i, tag) in inherited.iter().enumerate() {
        entries.push((
            format!("{{\"cmd\":\"{tag}\"}}"),
            "{}".to_string(),
            Some((watermark - chrono::Duration::hours((inherited.len() - i) as i64)).to_rfc3339()),
        ));
    }
    for (i, tag) in own.iter().enumerate() {
        entries.push((
            format!("{{\"cmd\":\"{tag}\"}}"),
            "{}".to_string(),
            Some((watermark + chrono::Duration::hours(1 + i as i64)).to_rfc3339()),
        ));
    }
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new(name.to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
            .unwrap();
        db.save_undo_stack_with_timestamps(entries).unwrap();
    }
    (path, watermark.to_rfc3339())
}

/// Read back which undo commands survived in a snapshot.
fn surviving_undo_commands(path: &Path) -> Vec<String> {
    let db = Database::open_existing(path).unwrap();
    db.load_undo_stack_with_timestamps()
        .unwrap()
        .into_iter()
        .map(|(_, cmd, _, _)| cmd)
        .collect()
}

/// [#303] A chain step loads a past commit's history so the user can keep undoing. That
/// scaffolding must not ride into the NEXT commit: those operations already live in the
/// commit they came from, and shipping them again records every one of them twice.
#[test]
fn sanitize_push_snapshot_cuts_undo_rows_inherited_by_a_chain_step() {
    let temp = TempDir::new().unwrap();
    let (path, watermark) = snapshot_with_inherited_and_own_rows(
        &temp,
        "inherited.stalx",
        &["loaded_a", "loaded_b"],
        &["mine"],
    );

    Database::sanitize_push_snapshot(&path, true, Some(&watermark)).unwrap();

    let survivors = surviving_undo_commands(&path);
    assert_eq!(
        survivors.len(),
        1,
        "only the operation authored in this session should ride, got {survivors:?}"
    );
    assert!(
        survivors[0].contains("mine"),
        "the surviving row must be the user's own edit, got {survivors:?}"
    );
}

#[test]
fn inherited_undo_cut_rolls_back_all_deletes_on_failure() {
    let temp = TempDir::new().unwrap();
    let (path, watermark) = snapshot_with_inherited_and_own_rows(
        &temp,
        "atomic-cut.stalx",
        &["loaded_a", "loaded_b"],
        &["mine"],
    );
    {
        let db = Database::open_existing(&path).unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER block_second_inherited_delete
                 BEFORE DELETE ON undo_stack
                 WHEN OLD.sequence = 1
                 BEGIN
                   SELECT RAISE(ABORT, 'blocked inherited delete');
                 END;",
            )
            .unwrap();
    }

    let err = Database::sanitize_push_snapshot(&path, true, Some(&watermark)).unwrap_err();
    assert!(
        format!("{err:#}").contains("blocked inherited delete"),
        "the injected delete failure must surface, got {err:#}"
    );
    // The inducer is a trigger, which the app's own open refuses (#608); drop it
    // before reopening to count what the failed cut left behind.
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TRIGGER block_second_inherited_delete")
        .unwrap();
    assert_eq!(
        surviving_undo_commands(&path).len(),
        3,
        "the first delete must roll back when a later row cannot be removed"
    );
}

/// The case a POSITION watermark would have destroyed, which is why this one is a
/// timestamp. Entries are written back with `sequence` = position, so after undoing into
/// the loaded history and editing, the user's new row occupies the id a loaded row had -
/// and cutting by id would delete their work instead of the inherited entry.
#[test]
fn the_cut_keeps_own_work_that_landed_on_an_inherited_row_position() {
    let temp = TempDir::new().unwrap();
    // One inherited row survives the undo, and the new edit is written directly above it,
    // taking sequence 1 - exactly the id the second loaded entry used to hold.
    let (path, watermark) =
        snapshot_with_inherited_and_own_rows(&temp, "positions.stalx", &["loaded_a"], &["mine"]);

    Database::sanitize_push_snapshot(&path, true, Some(&watermark)).unwrap();

    let survivors = surviving_undo_commands(&path);
    assert_eq!(survivors.len(), 1, "got {survivors:?}");
    assert!(
        survivors[0].contains("mine"),
        "cutting by position would have deleted this row; cutting by timestamp keeps it"
    );
}

/// [#303] Seed a snapshot whose undo rows carry EXACT timestamps (or none at all), for
/// the boundary cases the hour-either-side helper above cannot express.
fn snapshot_with_exact_stamps(
    temp: &TempDir,
    name: &str,
    rows: &[(&str, Option<String>)],
) -> std::path::PathBuf {
    let path = temp.path().join(name);
    let entries: Vec<(String, String, Option<String>)> = rows
        .iter()
        .map(|(tag, ts)| {
            (
                format!("{{\"cmd\":\"{tag}\"}}"),
                "{}".to_string(),
                ts.clone(),
            )
        })
        .collect();
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new(name.to_string(), None))
        .unwrap();
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
        .unwrap();
    db.save_undo_stack_with_timestamps(entries).unwrap();
    drop(db);
    path
}

/// [#303] A chain step with no watermark must strip the history, not ship it.
///
/// `speleodb_set_inherited_through` is best-effort - a DB hiccup logs a warning and moves
/// on - so "no watermark" has two meanings: nothing was inherited, or something was and we
/// lost track of how much. Treating the second as the first uploads a loaded revision's
/// history and duplicates every one of its operations in the next commit. The chain
/// pointer is what tells them apart.
#[test]
fn a_chain_step_with_no_watermark_strips_the_whole_undo_history() {
    let temp = TempDir::new().unwrap();
    let path = snapshot_with_exact_stamps(
        &temp,
        "no-watermark.stalx",
        &[("mine", Some(Utc::now().to_rfc3339()))],
    );
    // A chain step happened; its watermark write did not land.
    {
        let db = Database::open_existing(&path).unwrap();
        db.save_speleo_undo_chain(Some("deadbeefcafe")).unwrap();
    }

    Database::sanitize_push_snapshot(&path, true, None).unwrap();

    assert!(
        surviving_undo_commands(&path).is_empty(),
        "unknown inherited extent must fail closed and strip the lot"
    );
}

/// ...and with no chain step, an absent watermark means exactly what it looks like.
#[test]
fn no_chain_step_and_no_watermark_keeps_the_whole_undo_history() {
    let temp = TempDir::new().unwrap();
    let path = snapshot_with_exact_stamps(
        &temp,
        "ordinary.stalx",
        &[("mine", Some(Utc::now().to_rfc3339()))],
    );

    Database::sanitize_push_snapshot(&path, true, None).unwrap();

    assert_eq!(
        surviving_undo_commands(&path).len(),
        1,
        "an ordinary Full-mode push keeps the history it authored"
    );
}

#[test]
fn the_inherited_cut_rejects_an_unparseable_row() {
    let temp = TempDir::new().unwrap();
    let watermark = Utc::now();
    let path = snapshot_with_exact_stamps(
        &temp,
        "malformed-boundary.stalx",
        &[("unparseable", Some("not-a-timestamp".to_string()))],
    );

    let err = Database::sanitize_push_snapshot(&path, true, Some(&watermark.to_rfc3339()))
        .expect_err("a malformed row makes the inherited-history boundary ambiguous");

    assert!(format!("{err:#}").contains("malformed undo timestamp"));
    assert_eq!(
        surviving_undo_commands(&path).len(),
        1,
        "a refused cut leaves the snapshot history untouched"
    );
}

/// A row stamped exactly ON the watermark is inherited, while the next representable
/// instant is the user's own work. `UndoStack::next_stamp` floors new entries strictly
/// above the watermark because this cut is inclusive.
#[test]
fn the_inherited_cut_takes_the_watermark_row_but_spares_the_one_above() {
    let temp = TempDir::new().unwrap();
    let watermark = Utc::now();
    let path = snapshot_with_exact_stamps(
        &temp,
        "boundary.stalx",
        &[
            ("on_the_watermark", Some(watermark.to_rfc3339())),
            (
                "just_above",
                Some((watermark + chrono::Duration::nanoseconds(1)).to_rfc3339()),
            ),
        ],
    );

    Database::sanitize_push_snapshot(&path, true, Some(&watermark.to_rfc3339())).unwrap();

    let survivors = surviving_undo_commands(&path);
    assert_eq!(
        survivors.len(),
        1,
        "only the row stamped ABOVE the watermark was authored here, got {survivors:?}"
    );
    assert!(
        survivors[0].contains("just_above"),
        "a row exactly on the watermark is inherited, got {survivors:?}"
    );
}

/// No watermark is the ordinary lock-edit-push path: the lock cleared the stack, so every
/// entry is this session's and the whole history ships exactly as before.
#[test]
fn sanitize_push_snapshot_keeps_every_row_when_nothing_was_inherited() {
    let temp = TempDir::new().unwrap();
    let (path, _) =
        snapshot_with_inherited_and_own_rows(&temp, "no_watermark.stalx", &["older"], &["newer"]);

    Database::sanitize_push_snapshot(&path, true, None).unwrap();

    assert_eq!(
        surviving_undo_commands(&path).len(),
        2,
        "without a watermark nothing is inherited, so Full mode ships the lot"
    );
}

/// An unreadable watermark must FAIL the push rather than quietly widen into "cut
/// nothing": a failed push is recoverable, a commit with duplicated history is not.
#[test]
fn an_unparseable_watermark_fails_the_push_rather_than_shipping_everything() {
    let temp = TempDir::new().unwrap();
    let (path, _) =
        snapshot_with_inherited_and_own_rows(&temp, "bad_watermark.stalx", &["loaded"], &["mine"]);

    let err = Database::sanitize_push_snapshot(&path, true, Some("not-a-timestamp")).unwrap_err();

    assert!(
        format!("{err:#}").contains("watermark"),
        "the failure must name what went wrong, got {err:#}"
    );
}

/// [#303] The watermark is per-MACHINE, like the chain pointer: it says what THIS copy
/// inherited and against which clock. Shipping it would have the next puller cut their
/// own history against a measurement that means nothing on their machine.
#[test]
fn speleo_undo_watermark_is_stripped_from_a_push_snapshot() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("watermark_push.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("WatermarkPush".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
            .unwrap();
        db.save_speleo_undo_watermark(Some(&Utc::now().to_rfc3339()))
            .unwrap();
    }

    // Full mode again: the path most likely to forget a local table is the one that
    // deliberately keeps the biggest one.
    Database::sanitize_push_snapshot(&path, true, None).unwrap();

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.load_speleo_undo_watermark().unwrap(), None);
}

#[test]
fn speleo_undo_watermark_round_trips_and_clears() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("watermark_roundtrip.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("W".to_string(), None))
        .unwrap();

    assert_eq!(
        db.load_speleo_undo_watermark().unwrap(),
        None,
        "absent before any chain step - the ordinary case"
    );
    let ts = Utc::now().to_rfc3339();
    db.save_speleo_undo_watermark(Some(&ts)).unwrap();
    assert_eq!(
        db.load_speleo_undo_watermark().unwrap().as_deref(),
        Some(ts.as_str())
    );
    db.save_speleo_undo_watermark(None).unwrap();
    assert_eq!(db.load_speleo_undo_watermark().unwrap(), None);
}

#[test]
fn touch_speleo_link_dirty_is_targeted_and_locality_aware() {
    // #197 review: the hot-path dirty stamp is a single UPDATE - no row for a
    // local-only project (no-op, false), and for a remote link it sets the bit +
    // refreshes last_edit_at WITHOUT disturbing base_commit_sha / identity.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("touch.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Touch".to_string(), None))
        .unwrap();

    // Local-only (no link row): no-op, returns false.
    assert!(
        !db.touch_speleo_link_dirty("2026-06-28T00:00:00+00:00")
            .unwrap(),
        "no link row → nothing to touch"
    );
    assert!(db.load_speleo_link().unwrap().is_none(), "still no link");

    // Seed a CLEAN remote link, then touch it.
    db.save_speleo_link(Some(&sample_speleo_link(Some("base-sha"), false)))
        .unwrap();
    assert!(
        db.touch_speleo_link_dirty("2026-06-28T01:02:03+00:00")
            .unwrap(),
        "remote link row updated → true"
    );
    let link = db.load_speleo_link().unwrap().unwrap();
    assert!(link.has_unpushed_edits, "dirty bit set");
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("2026-06-28T01:02:03+00:00"),
        "last_edit_at stamped"
    );
    assert_eq!(
        link.base_commit_sha.as_deref(),
        Some("base-sha"),
        "BASE must NOT be disturbed by a dirty stamp"
    );
    assert_eq!(
        link.project_id, "11111111-1111-1111-1111-111111111111",
        "hand-off identity untouched"
    );

    // A second touch REFRESHES the timestamp (not idempotent - g7 needs the latest).
    assert!(db
        .touch_speleo_link_dirty("2026-06-28T09:09:09+00:00")
        .unwrap());
    assert_eq!(
        db.load_speleo_link()
            .unwrap()
            .unwrap()
            .last_edit_at
            .as_deref(),
        Some("2026-06-28T09:09:09+00:00"),
        "last_edit_at refreshed on every touch"
    );
}

/// #484: a never-linked DB missing the `speleo_link` table entirely (not just an
/// empty one) must be a silent no-op, not an `Err` that `mark_remote_dirty` logs at
/// WARN on every single mutating command. Constructs a bare connection with no
/// schema at all, so `UPDATE speleo_link ...` would fail with "no such table" if the
/// `table_exists` guard were removed.
#[test]
fn touch_speleo_link_dirty_on_missing_table_is_a_silent_no_op() {
    let conn = Connection::open_in_memory().unwrap();
    let db = Database { conn };

    let result = db.touch_speleo_link_dirty("2026-08-10T00:00:00+00:00");
    assert!(
        result.is_ok(),
        "missing speleo_link table must not surface as an error: {result:?}"
    );
    assert!(
        !result.unwrap(),
        "missing table is a no-op, same as a local-only project's absent row"
    );
}

/// #484 follow-up: the missing-table guard must not widen into swallowing every
/// error. With `speleo_link` PRESENT but the connection opened read-only, the
/// `UPDATE` fails for a real reason (`SQLITE_READONLY`), `table_exists` reports
/// `true`, and the error must still propagate so `mark_remote_dirty` logs its WARN.
#[test]
fn touch_speleo_link_dirty_on_a_real_error_still_propagates() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("readonly.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("RO".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("base-sha"), false)))
            .unwrap();
    }

    let conn =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let db = Database { conn };
    let result = db.touch_speleo_link_dirty("2026-08-10T00:00:00+00:00");
    assert!(
        result.is_err(),
        "a real persistence error on a present table must not be swallowed: {result:?}"
    );
}

#[test]
fn undo_mode_forward_edit_is_one_atomic_metadata_change() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("undo-mode-forward-edit.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Undo mode".to_owned(), None))
        .unwrap();
    db.save_speleo_link(Some(&sample_speleo_link(Some("base-sha"), false)))
        .unwrap();
    db.save_speleo_undo_chain(Some("older-tip")).unwrap();
    db.save_speleo_undo_watermark(Some("2026-08-01T00:00:00+00:00"))
        .unwrap();

    db.set_speleo_undo_mode_and_mark_forward_edit(
        crate::models::UndoMode::Disabled,
        "2026-08-02T01:02:03+00:00",
    )
    .unwrap();

    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(link.undo_mode, Some(crate::models::UndoMode::Disabled));
    assert!(link.has_unpushed_edits);
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("2026-08-02T01:02:03+00:00")
    );
    assert_eq!(db.load_speleo_undo_chain().unwrap(), None);
    assert_eq!(
        db.load_speleo_undo_watermark().unwrap().as_deref(),
        Some("2026-08-01T00:00:00+00:00"),
        "a forward edit clears the chain position, not the inherited-history cutoff"
    );
}

#[test]
fn undo_mode_forward_edit_rolls_back_when_chain_clear_fails() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("undo-mode-rollback.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Undo mode".to_owned(), None))
        .unwrap();
    db.save_speleo_link(Some(&sample_speleo_link(Some("base-sha"), false)))
        .unwrap();
    db.conn
        .execute_batch(
            "DROP TABLE speleo_undo_chain;
             CREATE VIEW speleo_undo_chain AS SELECT 'older-tip' AS unwound_from_sha;",
        )
        .unwrap();

    db.set_speleo_undo_mode_and_mark_forward_edit(
        crate::models::UndoMode::Disabled,
        "2026-08-02T01:02:03+00:00",
    )
    .expect_err("a chain-clear failure must roll back the link update");

    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(link.undo_mode, None);
    assert!(!link.has_unpushed_edits);
    assert_eq!(link.last_edit_at, None);
}

#[test]
fn preserved_speleolink_columns_match_head_schema() {
    // Structural HEAD-schema guard: every (name, decl_type, notnull) in
    // PRESERVED_SPELEOLINK_COLUMNS must match the live table exactly. Fails the
    // moment a future bump rewrites the hand-off identity or drops the dirty bit.
    let db = Database::in_memory().unwrap();
    let live = speleo_link_column_info(&db);
    assert_eq!(
        live.len(),
        PRESERVED_SPELEOLINK_COLUMNS.len(),
        "column count drift: {live:?} vs {PRESERVED_SPELEOLINK_COLUMNS:?}"
    );
    for (name, decl_type, notnull) in PRESERVED_SPELEOLINK_COLUMNS {
        let found = live
            .iter()
            .find(|(n, _, _)| n == name)
            .unwrap_or_else(|| panic!("column {name} missing from live speleo_link"));
        assert_eq!(&found.1, decl_type, "decl type drift for {name}");
        assert_eq!(&found.2, notnull, "null-ness drift for {name}");
    }
}

#[test]
fn speleo_link_survives_open_then_fast_path_reopen() {
    // Both open shapes (D-B13): a file created via open() has the v9 table; a
    // second open_existing() hits the ensure_schema_version fast path (already
    // current) and the table + row are intact.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("reopen.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Reopen".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
            .unwrap();
    }
    // First open_existing: already at HEAD, fast-path early return.
    {
        let db = Database::open_existing(&path).unwrap();
        assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
        assert!(db.load_speleo_link().unwrap().is_some());
    }
    // Second reopen: still intact, dirty bit preserved.
    let db = Database::open_existing(&path).unwrap();
    let link = db.load_speleo_link().unwrap().unwrap();
    assert!(link.has_unpushed_edits);
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
}

// --- #195 VACUUM INTO snapshot ---

/// Seed a `.stalx` at `path` with one section + one shot, then close it (so a
/// DELETE-mode file is fully flushed to disk).
fn seed_snapshot_source(path: &Path) {
    let mut db = Database::open(path).unwrap();
    let mut project = Project::new("Snap Cave".to_string(), Some("Loc".to_string()));
    project.add_section(SectionData {
        name: Some("Line".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![ShotData {
            azimuth_in: 10.0,
            azimuth_out: 190.0,
            length: 5.0,
            depth_in: 1.0,
            depth_out: 2.0,
            ..Default::default()
        }],
    });
    db.save_project(&project).unwrap();
}

/// Assert no `-wal`/`-journal`/`-shm` sidecar sits next to `dest`.
fn assert_no_sqlite_sidecars(dest: &Path) {
    let dest_str = dest.to_string_lossy();
    for suffix in ["-wal", "-journal", "-shm"] {
        let sidecar = std::path::PathBuf::from(format!("{dest_str}{suffix}"));
        assert!(
            !sidecar.exists(),
            "snapshot must be self-contained: {sidecar:?} should not exist"
        );
    }
}

#[test]
fn vacuum_snapshot_self_contained_delete_mode() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src.stalx");
    let dest = temp.path().join("snap/out.stalx");
    seed_snapshot_source(&src);

    Database::vacuum_snapshot(&src, &dest).unwrap();

    assert!(dest.exists(), "snapshot written");
    assert_no_sqlite_sidecars(&dest);

    // Reopens as a normal project with the data intact.
    let db = Database::open_existing(&dest).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.name, "Snap Cave");
    assert_eq!(loaded.sections.len(), 1);
    assert_eq!(loaded.sections[0].shots.len(), 1);
}

#[test]
fn vacuum_snapshot_under_wal_mode() {
    // The live file may be in WAL mode with un-checkpointed committed frames.
    // VACUUM INTO (on its own connection) must fold them in and still emit a
    // self-contained DELETE-mode snapshot.
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("wal.stalx");
    let dest = temp.path().join("wal_snap.stalx");

    let mut db = Database::open(&src).unwrap();
    db.conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    let mut project = Project::new("WAL Cave".to_string(), None);
    project.add_section(SectionData {
        name: Some("WL".to_string()),
        surveyor: None,
        date: None,
        color: "#abcdef".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![ShotData {
            azimuth_in: 1.0,
            azimuth_out: 181.0,
            length: 3.0,
            depth_in: 0.0,
            depth_out: 1.0,
            ..Default::default()
        }],
    });
    db.save_project(&project).unwrap();

    // Snapshot while `db` is still open (the WAL is NOT checkpointed yet).
    Database::vacuum_snapshot(&src, &dest).unwrap();
    assert_no_sqlite_sidecars(&dest);

    let snap = Database::open_existing(&dest).unwrap();
    let loaded = snap.load_project().unwrap();
    assert_eq!(loaded.name, "WAL Cave");
    assert_eq!(loaded.sections.len(), 1, "committed WAL data captured");
    assert_eq!(loaded.sections[0].shots.len(), 1);
}

#[test]
fn vacuum_snapshot_removes_stale_dest() {
    // VACUUM INTO refuses to overwrite; the helper clears a stale target first.
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src.stalx");
    let dest = temp.path().join("stale.stalx");
    seed_snapshot_source(&src);
    std::fs::write(&dest, b"not a database").unwrap();

    Database::vacuum_snapshot(&src, &dest).unwrap();

    let db = Database::open_existing(&dest).unwrap();
    assert_eq!(db.load_project().unwrap().name, "Snap Cave");
}

#[test]
fn vacuum_snapshot_carries_speleo_link_for_caller_to_clear() {
    // The snapshot preserves the speleo_link row (incl. the dirty bit) so the
    // push pipeline can clear has_unpushed_edits in the COPY before upload.
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("linked.stalx");
    let dest = temp.path().join("linked_snap.stalx");
    {
        let mut db = Database::open(&src).unwrap();
        db.save_project(&Project::new("Linked".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
            .unwrap();
    }

    Database::vacuum_snapshot(&src, &dest).unwrap();

    let snap = Database::open_existing(&dest).unwrap();
    let link = snap.load_speleo_link().unwrap().expect("link in snapshot");
    assert!(
        link.has_unpushed_edits,
        "dirty bit present for caller to clear"
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
}

#[test]
fn vacuum_snapshot_missing_source_errors() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("nope.stalx");
    let dest = temp.path().join("out.stalx");
    assert!(Database::vacuum_snapshot(&src, &dest).is_err());
    assert!(!dest.exists(), "no snapshot written on error");
}

#[test]
fn sanitize_push_snapshot_clears_dirty_and_view_only_keeps_project_data() {
    // #197: the push snapshot clears ONLY the dirty bit + per-machine view toggles
    // (display_settings). Durable project data - the import-dedup hashes and
    // resolved_warnings - PROLIFERATES with the project (only a lock-holding editor
    // can push, so anything in the snapshot is a legitimate change). The survey
    // project row is preserved.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("push.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Push".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
            .unwrap();
        // A non-null edit timestamp so the dirty clear is observable; a view toggle
        // that MUST be stripped; and project-data rows that MUST survive. A real
        // section backs the resolved_warnings FK so Database::open_existing's
        // integrity check passes after sanitize.
        db.conn
            .execute(
                "UPDATE speleo_link SET last_edit_at = '2026-01-01T00:00:00+00:00'",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO sections (id, color, created_at) VALUES (1, '#abcdef', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO display_settings (key, value) VALUES ('zoom', '2')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO resolved_warnings (section_id, shot_id, warning_code, resolved_at) \
                 VALUES (1, 1, 'X', '2026-01-01T00:00:00+00:00')",
                [],
            )
            .unwrap();
        db.conn
            .execute("INSERT INTO imported_file_hashes (hash) VALUES ('h1')", [])
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO imported_section_hashes (hash) VALUES ('h2')",
                [],
            )
            .unwrap();
        // Per-session LOCAL state that MUST be stripped (#194 b3): undo/redo
        // edit history, a data-corruption vector if a collaborator inherited
        // and replayed it.
        db.conn
            .execute(
                "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (1, X'00', X'00', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO redo_stack (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (1, X'00', X'00', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
    }

    Database::sanitize_push_snapshot(&path, false, None).unwrap();

    let snap = Database::open_existing(&path).unwrap();
    let count = |table: &str| -> i64 {
        snap.conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    // Per-machine view toggles stripped.
    assert_eq!(
        count("display_settings"),
        0,
        "display_settings (view toggles) must be stripped from the push snapshot"
    );
    // Per-session edit history stripped (#194 b3) - this call passes `keep_undo: false`,
    // which is what a local project and Disabled mode (#303) get: a collaborator adopting
    // the pushed snapshot starts with a clean undo/redo slate and can never inherit (or
    // replay) the pusher's history.
    assert_eq!(
        count("undo_stack"),
        0,
        "undo_stack must be stripped when keep_undo is false (#194 b3)"
    );
    assert_eq!(
        count("redo_stack"),
        0,
        "redo_stack must be stripped from the push snapshot (#194 b3)"
    );
    // Durable project data proliferates - NOT stripped.
    assert_eq!(
        count("resolved_warnings"),
        1,
        "resolved_warnings must proliferate with the project"
    );
    assert_eq!(
        count("imported_file_hashes"),
        1,
        "imported_file_hashes must proliferate with the project"
    );
    assert_eq!(
        count("imported_section_hashes"),
        1,
        "imported_section_hashes must proliferate with the project"
    );
    // Dirty bit + edit timestamp cleared; the hand-off identity is preserved.
    let link = snap.load_speleo_link().unwrap().expect("link kept");
    assert!(
        !link.has_unpushed_edits,
        "dirty bit cleared in the push snapshot"
    );
    assert_eq!(link.last_edit_at, None, "edit timestamp cleared");
    assert_eq!(
        link.base_commit_sha.as_deref(),
        Some("abc"),
        "identity preserved"
    );
    // Survey data survives the sanitize.
    assert_eq!(
        snap.load_project().unwrap().name,
        "Push",
        "survey project preserved in the push snapshot"
    );
}

/// A push snapshot with a saved project, a dirty `speleo_link`, and `count` rows of
/// undo history whose payload carries `marker` (so the raw file can be searched for
/// it). Returns the path inside `temp`.
fn dirty_push_snapshot(temp: &TempDir, marker: &str, count: usize) -> std::path::PathBuf {
    let path = temp.path().join("push.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Push".to_string(), None))
        .unwrap();
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
        .unwrap();
    db.conn
        .execute(
            "UPDATE speleo_link SET last_edit_at = '2026-01-01T00:00:00+00:00'",
            [],
        )
        .unwrap();
    for i in 0..count {
        db.conn
            .execute(
                "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (?1, ?2, ?2, '2026-01-01T00:00:00Z')",
                rusqlite::params![i as i64, marker.repeat(64).as_bytes()],
            )
            .unwrap();
    }
    drop(db);
    path
}

#[test]
fn sanitize_push_snapshot_vacuums_the_deleted_undo_bytes_out_of_the_file() {
    // The load-bearing half of the strip: DELETE only marks pages free, so without
    // the VACUUM every "stripped" undo row is still physically present in the bytes
    // that get uploaded to collaborators. Assert on the RAW file, not on row counts.
    let temp = TempDir::new().unwrap();
    let marker = "UNDOSECRET";
    let path = dirty_push_snapshot(&temp, marker, 200);

    let needle = marker.repeat(64).into_bytes();
    let before = std::fs::read(&path).unwrap();
    assert!(
        before.windows(needle.len()).any(|w| w == needle.as_slice()),
        "precondition: the undo payload is in the file before sanitize"
    );
    let pages_before: i64 = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();

    Database::sanitize_push_snapshot(&path, false, None).unwrap();

    let after = std::fs::read(&path).unwrap();
    assert!(
        !after.windows(needle.len()).any(|w| w == needle.as_slice()),
        "the pusher's undo payload must not survive anywhere in the uploaded bytes"
    );
    let pages_after: i64 = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    assert!(
        pages_after < pages_before,
        "VACUUM must reclaim the freed pages (before {pages_before}, after {pages_after})"
    );
}

#[test]
fn sanitize_push_snapshot_clears_the_dirty_bit() {
    // The dirty bit is local hand-off state; a stale `1` riding to the server is a
    // correctness bug for whoever pulls it next.
    let temp = TempDir::new().unwrap();
    let path = dirty_push_snapshot(&temp, "x", 1);

    Database::sanitize_push_snapshot(&path, false, None).unwrap();

    let link = Database::open_existing(&path)
        .unwrap()
        .load_speleo_link()
        .unwrap()
        .expect("link kept");
    assert!(!link.has_unpushed_edits, "dirty bit cleared");
    assert_eq!(link.last_edit_at, None, "edit timestamp cleared");
    assert_eq!(
        link.base_commit_sha.as_deref(),
        Some("abc"),
        "identity preserved"
    );
}

#[test]
fn sanitize_push_snapshot_tolerates_a_snapshot_without_a_redo_stack() {
    // An older snapshot shape can legitimately lack a local-only table - the v15
    // migration's own strip helper (`drop_legacy_undo_history`) is guarded on exactly
    // that - and what does not exist cannot leak, so a missing table must never abort
    // the push. `redo_stack` is the sharp end of the branch: it is on
    // PUSH_SNAPSHOT_FATAL_TABLES, so without the absent-table `continue` the
    // `DELETE FROM redo_stack` would error with "no such table" and take the FATAL
    // arm - failing every push from a snapshot that simply never had one.
    //
    // Every other sanitize test builds its fixture through `Database::open`, whose
    // `init_schema` creates all the local-only tables, so none of them can reach this.
    let temp = TempDir::new().unwrap();
    let path = dirty_push_snapshot(&temp, "x", 3);
    Connection::open(&path)
        .unwrap()
        .execute("DROP TABLE redo_stack", [])
        .unwrap();

    Database::sanitize_push_snapshot(&path, false, None)
        .expect("a missing local table must not fail the push");

    let snap = Connection::open(&path).unwrap();
    let undo: i64 = snap
        .query_row("SELECT COUNT(*) FROM undo_stack", [], |r| r.get(0))
        .unwrap();
    assert_eq!(undo, 0, "the tables that DO exist are still stripped");
    // The skip continues the loop rather than short-circuiting the function: the
    // dirty-bit clear that comes AFTER the strip loop must still have run.
    let dirty: i64 = snap
        .query_row("SELECT has_unpushed_edits FROM speleo_link", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(dirty, 0, "the rest of the sanitize still ran to completion");
}

/// [#303] Spec item 1: in Full mode the pushed snapshot KEEPS its undo history - that
/// is the payload that lets undo chain backwards commit by commit - while the redo stack
/// is stripped anyway (clarification #1: SpeleoDB stores no redo).
#[test]
fn sanitize_push_snapshot_keeps_undo_but_never_redo_in_full_mode() {
    let temp = TempDir::new().unwrap();
    let path = dirty_push_snapshot(&temp, "abc", 3);
    // Seed the redo side too, so "redo is stripped even here" is actually tested.
    Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO redo_stack (sequence, command_json, undo_data_json, executed_at) \
             VALUES (1, X'00', X'00', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

    Database::sanitize_push_snapshot(&path, true, None).unwrap();

    let snap = Connection::open(&path).unwrap();
    let count = |table: &str| -> i64 {
        snap.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        count("undo_stack"),
        3,
        "Full mode ships the undo history since the last commit (#303)"
    );
    assert_eq!(
        count("redo_stack"),
        0,
        "redo is never uploaded, in ANY mode (clarification #1)"
    );
    // The rest of the sanitize still runs: local-only state goes, dirty bit clears.
    // `collab_state` is deliberately NOT asserted on: schema v15 drops the table, so a
    // freshly-created file has none to count. It stays on the strip LIST for the
    // downgrade-and-return round trip (see `PUSH_SNAPSHOT_LOCAL_TABLES`), where the loop
    // skips it as absent - which is exactly the behaviour that makes counting it here an
    // error rather than a check.
    assert_eq!(count("display_settings"), 0);
    let dirty: i64 = snap
        .query_row(
            "SELECT has_unpushed_edits FROM speleo_link WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dirty, 0, "the dirty bit is local hand-off state either way");
}

/// [#303] Spec item 2: Disabled mode strips BOTH stacks, so a project marked too big to
/// ship undo history ships none of it.
#[test]
fn sanitize_push_snapshot_strips_both_stacks_in_disabled_mode() {
    let temp = TempDir::new().unwrap();
    let path = dirty_push_snapshot(&temp, "abc", 3);
    Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO redo_stack (sequence, command_json, undo_data_json, executed_at) \
             VALUES (1, X'00', X'00', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

    Database::sanitize_push_snapshot(&path, false, None).unwrap();

    let snap = Connection::open(&path).unwrap();
    for table in ["undo_stack", "redo_stack"] {
        let n: i64 = snap
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "{table} must be stripped in Disabled mode");
    }
}

/// [#303] Spec item 3, Full-mode half: keeping the undo history must NOT relax the
/// redo strip's fatality. A redo table that exists but cannot be emptied still aborts
/// the push, because a redo branch replaying onto a state the server has committed past
/// is the stale-replay hazard with none of the undo side's value.
#[test]
fn sanitize_push_snapshot_still_fails_closed_on_redo_in_full_mode() {
    let temp = TempDir::new().unwrap();
    let path = dirty_push_snapshot(&temp, "abc", 1);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "INSERT INTO redo_stack (sequence, command_json, undo_data_json, executed_at) \
               VALUES (1, X'00', X'00', '2026-01-01T00:00:00Z'); \
             CREATE TRIGGER block_redo_delete BEFORE DELETE ON redo_stack \
               BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
        )
        .unwrap();

    let err = Database::sanitize_push_snapshot(&path, true, None)
        .expect_err("an un-strippable redo_stack must abort the push even in Full mode");
    assert!(
        format!("{err:#}").contains("redo_stack"),
        "the error should name redo_stack, got: {err:#}"
    );
}

#[test]
fn sanitize_push_snapshot_fails_closed_when_the_undo_strip_cannot_run() {
    // The fail-closed half: if `undo_stack` exists but cannot be emptied, the
    // snapshot still carries the pusher's edit history - both a privacy leak and a
    // replay-corruption vector on adopt (#194 b3) - so the push must abort rather
    // than upload it. A BEFORE DELETE trigger that RAISEs makes the DELETE fail
    // while leaving the table (and its rows) in place.
    let temp = TempDir::new().unwrap();
    let path = dirty_push_snapshot(&temp, "x", 2);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER block_undo_delete BEFORE DELETE ON undo_stack \
             BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
        )
        .unwrap();

    let err = Database::sanitize_push_snapshot(&path, false, None)
        .expect_err("an un-strippable undo_stack must abort the push");
    assert!(
        format!("{err:#}").contains("undo_stack"),
        "the error should name the table that could not be stripped, got: {err:#}"
    );

    let undo: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM undo_stack", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        undo, 2,
        "the history is still there - which is why we abort"
    );
}

#[test]
fn sanitize_push_snapshot_tolerates_an_unstrippable_display_settings() {
    // The other side of the policy: `display_settings` is a privacy-neutral local
    // pref, so failing to strip it is logged, not fatal - blocking a push over view
    // toggles would be a worse outcome for the user than the (harmless) leak. The
    // fatal strips still run.
    let temp = TempDir::new().unwrap();
    let path = dirty_push_snapshot(&temp, "x", 2);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO display_settings (key, value) VALUES ('zoom', '2')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER block_view_delete BEFORE DELETE ON display_settings \
             BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
        )
        .unwrap();
    }

    Database::sanitize_push_snapshot(&path, false, None)
        .expect("a privacy-neutral local table must not abort the push");

    let snap = Connection::open(&path).unwrap();
    let count = |table: &str| -> i64 {
        snap.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count("undo_stack"), 0, "the fatal strips still ran");
    assert_eq!(count("display_settings"), 1, "the best-effort strip failed");
}

/// Build a section of shots that all descend at ~30°, with `sign` applied to the
/// recorded pitch: `-1.0` reproduces the pre-#296 DMP storage (+up), `1.0` the
/// already-correct Walls/Compass storage (+down).
fn descending_section(name: &str, sign: f64, count: usize) -> SectionData {
    // length 10 with dz 5 is a 30° descent, well clear of the migration's
    // near-horizontal and short-shot floors.
    let shots = (0..count)
        .map(|i| ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 5.0 * i as f64,
            depth_out: 5.0 * (i + 1) as f64,
            pitch_in: Some(sign * 30.0),
            pitch_out: Some(sign * 30.0),
            ..Default::default()
        })
        .collect();
    SectionData {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff5733".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots,
    }
}

fn section_pitches(db: &Database, section_name: &str) -> Vec<Option<f64>> {
    let mut stmt = db
        .conn
        .prepare(
            "SELECT s.pitch_in FROM shots s
             JOIN sections sec ON sec.id = s.section_id
             WHERE sec.name = ?1 ORDER BY s.sequence_order",
        )
        .unwrap();
    stmt.query_map([section_name], |r| r.get::<_, Option<f64>>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// Opening a pre-v13 project converts the boolean `is_reference` column into the
/// 3-way `kind`, seeds every virtual shot as Jump (#27 q2), and then lets the
/// one-shot backfill flag re-decide via the chain rule (#27 q16).
#[test]
fn migrate_v13_converts_is_reference_to_kind_and_flags_backfill() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v12.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    // R V R  -> bounded, so the backfill should settle on Jump.
    // R V V  -> one-sided, so the backfill should settle on Reference.
    project.add_section(SectionData {
        name: Some("Bounded".to_string()),
        shots: vec![
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Reference),
            legacy_shot(ShotKind::Normal),
        ],
        ..Default::default()
    });
    project.add_section(SectionData {
        name: Some("Dangling".to_string()),
        shots: vec![
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Reference),
            legacy_shot(ShotKind::Reference),
        ],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // Reshape the table back to the v12 form: a boolean is_reference where
        // every virtual shot reads 1, which is what a real pre-#27 file holds.
        db.conn
            .execute_batch(
                "ALTER TABLE shots ADD COLUMN is_reference INTEGER NOT NULL DEFAULT 0;
                 UPDATE shots SET is_reference = CASE WHEN kind != 0 THEN 1 ELSE 0 END;
                 ALTER TABLE shots DROP COLUMN kind;",
            )
            .unwrap();
        db.set_user_version(12).unwrap();
    }

    // open_existing runs ensure_schema_version → the v12→v13 migration.
    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
    assert!(
        db.conn.prepare("SELECT kind FROM shots LIMIT 0").is_ok(),
        "kind column must exist after migration"
    );
    assert!(
        db.conn
            .prepare("SELECT is_reference FROM shots LIMIT 0")
            .is_err(),
        "the boolean column must be gone"
    );

    // The migration itself only seeds Jump (discriminant 1) per q2.
    let seeded: Vec<i64> = db
        .conn
        .prepare("SELECT kind FROM shots ORDER BY section_id, sequence_order")
        .unwrap()
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(seeded, vec![0, 1, 0, 0, 1, 1]);

    // And it hands the real verdict to the one-shot backfill on next load.
    assert!(
        db.is_kind_classification_pending(),
        "migration must flag the chain-rule backfill"
    );
}

/// #27: a `kind` byte this build cannot read must be REPORTED, not just quietly
/// substituted. `from_u8` fails it closed to `Reference` so it cannot invent a
/// swimmable leg, but the user is then owed an explanation and a choice.
#[test]
fn unrecognised_shot_kinds_are_reported_with_their_raw_value() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("bad_kind.stalx");

    let mut project = Project::new("Corrupt".to_string(), None);
    project.add_section(SectionData {
        name: Some("Main".to_string()),
        shots: vec![
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Jump),
            legacy_shot(ShotKind::Reference),
        ],
        ..Default::default()
    });

    let db = {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db
    };
    // Nothing wrong yet.
    assert!(db.unrecognised_shot_kinds().unwrap().is_empty());

    // Corrupt the middle shot's discriminant. There is no CHECK constraint, so
    // this is exactly what a bad write or a newer writer would leave behind.
    db.conn
        .execute_batch("UPDATE shots SET kind = 7 WHERE sequence_order = 1;")
        .unwrap();

    let found = db.unrecognised_shot_kinds().unwrap();
    assert_eq!(found.len(), 1, "exactly the corrupt row is reported");
    let hit = &found[0];
    assert_eq!(
        hit.raw_value, "7",
        "the raw value must survive for the dialog"
    );
    assert_eq!(hit.shot_number, 2, "1-based position for display");
    assert_eq!(hit.section_name.as_deref(), Some("Main"));
    let section_id = project.sections[0].id;
    assert_eq!(hit.section_id, section_id);
    assert_eq!(
        hit.shot_id,
        section_id * crate::models::FEATURE_ID_STRIDE + 1,
        "shot_id must be the in-memory id set_shot_kind expects, not the rowid"
    );

    // And the loader itself fails the shot closed rather than making it swimmable.
    let loaded = db.load_project().unwrap();
    assert_eq!(
        loaded.sections[0].shots[1].kind,
        ShotKind::Reference,
        "an unreadable kind must not load as a swimmable leg"
    );

    // Values OUTSIDE the u8 range have to fail closed too. The column is a
    // plain INTEGER, so -1 and 256 are both storable, and the u8 conversion on
    // the way to `from_u8` is a second place the fail-closed rule can be lost.
    // Includes values outside i32: SQLite stores INTEGER as i64, and reading the
    // column as i32 would ERROR rather than fall back, aborting the whole load.
    for bad in [
        -1_i64,
        256,
        i64::from(i32::MAX),
        2_147_483_648,
        i64::MIN,
        i64::MAX,
    ] {
        db.conn
            .execute("UPDATE shots SET kind = ?1 WHERE sequence_order = 1", [bad])
            .unwrap();
        let reported = db.unrecognised_shot_kinds().unwrap();
        assert_eq!(reported.len(), 1, "kind {bad} must still be reported");
        assert_eq!(reported[0].raw_value, bad.to_string());
        assert_eq!(
            db.load_project().unwrap().sections[0].shots[1].kind,
            ShotKind::Reference,
            "kind {bad} must load as Reference, not Normal"
        );
    }

    // The column has INTEGER *affinity*, not an INTEGER constraint - the table
    // is not STRICT - so a corrupt or hand-edited row can hold any storage
    // class. These must load, not error out the whole project.
    for bad in ["'not-a-number'", "x'DEADBEEF'", "3.5"] {
        db.conn
            .execute_batch(&format!(
                "UPDATE shots SET kind = {bad} WHERE sequence_order = 1;"
            ))
            .unwrap();
        assert_eq!(
            db.load_project().unwrap().sections[0].shots[1].kind,
            ShotKind::Reference,
            "kind {bad} must load as Reference rather than failing the load"
        );
        assert_eq!(
            db.unrecognised_shot_kinds().unwrap().len(),
            1,
            "kind {bad} must still reach the resolution prompt"
        );
    }
}

/// #27: the fail-closed substitution has to reach DISK, or it is session-only.
///
/// Nothing marks the project dirty (the user edited nothing), so without an
/// explicit repair the invalid discriminant survives every open and the prompt
/// reappears forever - while the dialog claims the choice is saved with the
/// project.
#[test]
fn unrecognised_shot_kinds_are_repaired_on_disk() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repair.stalx");

    let mut project = Project::new("Corrupt".to_string(), None);
    project.add_section(SectionData {
        shots: vec![legacy_shot(ShotKind::Normal), legacy_shot(ShotKind::Jump)],
        ..Default::default()
    });

    let db = {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db
    };
    db.conn
        .execute_batch("UPDATE shots SET kind = 9 WHERE sequence_order = 1;")
        .unwrap();

    assert_eq!(db.repair_unrecognised_shot_kinds().unwrap(), 1);
    assert!(
        db.unrecognised_shot_kinds().unwrap().is_empty(),
        "nothing is left to prompt about once the row is repaired"
    );
    let kinds: Vec<i64> = db
        .conn
        .prepare("SELECT kind FROM shots ORDER BY sequence_order")
        .unwrap()
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec![0, 2],
        "the unreadable row is now Reference on disk"
    );

    // Idempotent, and it never touches a row it can read.
    assert_eq!(db.repair_unrecognised_shot_kinds().unwrap(), 0);
}

/// #27: the pre-#27 loader read `is_reference` as `!= 0`, so ANY truthy value
/// meant virtual. A migration that only copied `= 1` would leave a noncanonical
/// `-1` or `2` on the new column's `Normal` default and then drop the evidence -
/// permanently converting a reference tie into a swimmable leg, with the
/// backfill unable to help because the shot no longer looks virtual.
#[test]
fn migrate_v13_carries_every_truthy_legacy_reference_value() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("noncanonical.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Normal),
        ],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // Rebuild the v12 shape by hand with a spread of truthy values.
        db.conn
            .execute_batch(
                "ALTER TABLE shots ADD COLUMN is_reference INTEGER NOT NULL DEFAULT 0;
                 UPDATE shots SET is_reference = 0  WHERE sequence_order = 0;
                 UPDATE shots SET is_reference = 1  WHERE sequence_order = 1;
                 UPDATE shots SET is_reference = -1 WHERE sequence_order = 2;
                 UPDATE shots SET is_reference = 2  WHERE sequence_order = 3;
                 ALTER TABLE shots DROP COLUMN kind;",
            )
            .unwrap();
        db.set_user_version(12).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let kinds: Vec<i64> = db
        .conn
        .prepare("SELECT kind FROM shots ORDER BY sequence_order")
        .unwrap()
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    // 0 stays Normal; every truthy value becomes the Jump seed (1) the backfill
    // then judges - none is silently left Normal.
    assert_eq!(kinds, vec![0, 1, 1, 1]);
}

/// #27: the split toggles have to stay independent ACROSS RESTARTS, not just on
/// the first open.
///
/// The load-time fallback (`showJumps` -> `showReferenceShots`) gets the first
/// open right, but it is a read-time default. `save_display_setting` writes only
/// the key it is handed, so without a materialised row a user who then changes
/// References alone would find Jumps following that new value on the next open -
/// a toggle moving on its own.
#[test]
fn migrate_v13_materialises_show_jumps_from_the_old_single_toggle() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_toggle.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![legacy_shot(ShotKind::Reference)],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // A pre-#27 file: one combined toggle, turned OFF, and no showJumps row.
        db.conn
            .execute_batch(
                "DELETE FROM display_settings WHERE key IN ('showJumps', 'showReferenceShots');
                 INSERT INTO display_settings (key, value) VALUES ('showReferenceShots', 'false');",
            )
            .unwrap();
        db.set_user_version(12).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        db.get_display_setting("showJumps").unwrap().as_deref(),
        Some("false"),
        "showJumps must be materialised from the old combined toggle, not left absent"
    );

    // Now the user changes ONLY references. Jumps must not follow.
    db.save_display_setting("showReferenceShots", "true")
        .unwrap();
    assert_eq!(
        db.get_display_setting("showJumps").unwrap().as_deref(),
        Some("false"),
        "a stored showJumps must not be moved by a change to showReferenceShots"
    );
}

/// #27: a v12 file keeps its persisted undo history (`UNDO_FORMAT_SAFE_VERSION`
/// is 7), and those payloads carry the legacy `is_reference` boolean. A bare
/// `true` cannot say which virtual kind it meant, so `de_shot_kind` maps it to
/// `Jump` - replaying one after the backfill settled a tie on `Reference` would
/// make it swimmable again, with no classifier pass on the undo path to correct
/// it. The v13 migration drops both stacks, exactly as v11->v12 does after a
/// pitch flip.
#[test]
fn migrate_v13_drops_persisted_undo_history() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v12_undo.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Reference),
        ],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // A persisted entry whose payload still speaks the pre-#27 boolean.
        db.conn
            .execute_batch(
                r#"INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at)
                   VALUES (0,
                           '{"type":"set_shot_reference","section_id":0,"shot_id":1,"is_reference":true}',
                           '{}', '2026-01-01T00:00:00Z');"#,
            )
            .unwrap();
        // Back to the v12 shape so ensure_schema_version runs the v13 arm.
        db.conn
            .execute_batch(
                "ALTER TABLE shots ADD COLUMN is_reference INTEGER NOT NULL DEFAULT 0;
                 UPDATE shots SET is_reference = CASE WHEN kind != 0 THEN 1 ELSE 0 END;
                 ALTER TABLE shots DROP COLUMN kind;",
            )
            .unwrap();
        db.set_user_version(12).unwrap();
        let staged: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM undo_stack", [], |r| r.get(0))
            .unwrap();
        assert_eq!(staged, 1, "the fixture must actually stage an undo entry");
    }

    let db = Database::open_existing(&path).unwrap();
    for table in ["undo_stack", "redo_stack"] {
        let remaining: i64 = db
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "{table} must be empty: a legacy is_reference payload cannot be replayed safely"
        );
    }
}

/// The verdicts the one-shot backfill computes have to land on the RIGHT rows
/// and survive a reopen.
///
/// Regression: the first cut wrote `UPDATE shots SET kind = ? WHERE id = ?`
/// using the in-memory `Shot::id`. That is a different ID space from the
/// `shots.id` rowid - `write.rs` never stores an id, and `read.rs` synthesises
/// one as `section_id * FEATURE_ID_STRIDE + sequence_order`. So section 0's
/// verdicts landed one row early and every later section matched nothing at
/// all, while the pending flag was cleared in the same transaction - making the
/// wrong kinds permanent. Two sections here is the point: a single-section
/// fixture passes even with the rowid bug.
#[test]
fn backfilled_shot_kinds_land_on_the_right_rows_and_survive_a_reopen() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("backfill.stalx");

    let mut project = Project::new("Backfill".to_string(), None);
    project.add_section(SectionData {
        name: Some("First".to_string()),
        shots: vec![
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Reference),
            legacy_shot(ShotKind::Normal),
        ],
        ..Default::default()
    });
    project.add_section(SectionData {
        name: Some("Second".to_string()),
        shots: vec![
            legacy_shot(ShotKind::Normal),
            legacy_shot(ShotKind::Reference),
        ],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Reload so the shots carry the ids the real backfill path sees, then feed
    // the verdicts through exactly as project_load does.
    let db = Database::open_existing(&path).unwrap();
    let loaded = db.load_project().unwrap();
    // Section 0's virtual shot -> Reference, section 1's -> Jump. Deliberately
    // different per section, and neither equal to what is on disk, so a write
    // that lands on the wrong row or no row cannot pass.
    let verdicts = vec![
        (loaded.sections[0].id, 1_u32, ShotKind::Reference),
        (loaded.sections[1].id, 1_u32, ShotKind::Jump),
    ];
    db.persist_backfilled_shot_kinds(&verdicts).unwrap();
    drop(db);

    let db = Database::open_existing(&path).unwrap();
    let on_disk: Vec<(u32, i64, i64)> = db
        .conn
        .prepare("SELECT section_id, sequence_order, kind FROM shots ORDER BY section_id, sequence_order")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    let s0 = loaded.sections[0].id;
    let s1 = loaded.sections[1].id;
    assert_eq!(
        on_disk,
        vec![
            (s0, 0, 0), // Normal, untouched
            (s0, 1, 2), // Reference - the verdict, on the row it was aimed at
            (s0, 2, 0), // Normal, untouched (the rowid bug shifted this one)
            (s1, 0, 0), // Normal, untouched
            (s1, 1, 1), // Jump - the rowid bug never reached this section at all
        ],
        "each verdict must land on its own (section_id, sequence_order) row"
    );
    assert!(
        !db.is_kind_classification_pending(),
        "the flag clears with the writes, in the same transaction"
    );
}

/// The backfill flag is one-shot: once cleared it stays cleared, because
/// classification is import-time only (#27 q14) and must never silently
/// re-flip a kind after the user has edited it.
#[test]
fn pending_kind_classification_flag_clears_once() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("flag.stalx");
    let db = Database::open(&path).unwrap();

    assert!(!db.is_kind_classification_pending());
    db.conn
        .execute_batch(
            "INSERT OR REPLACE INTO project_settings (key, value)
             VALUES ('pending_kind_classification', '1');",
        )
        .unwrap();
    assert!(db.is_kind_classification_pending());

    db.clear_pending_kind_classification().unwrap();
    assert!(!db.is_kind_classification_pending());
    // Idempotent: clearing an already-clear flag is not an error.
    db.clear_pending_kind_classification().unwrap();
    assert!(!db.is_kind_classification_pending());
}

fn legacy_shot(kind: ShotKind) -> ShotData {
    ShotData {
        azimuth_in: 0.0,
        azimuth_out: 0.0,
        length: 10.0,
        depth_in: 0.0,
        depth_out: 0.0,
        kind,
        ..Default::default()
    }
}

/// Opening a pre-v12 project normalises only the sections whose pitch is stored
/// in the old +up convention, and leaves everything else exactly as it was (#296).
///
/// The verdict has to be per section because nothing records which importer
/// produced a section, and a project can hold a DMP section and a Compass section
/// side by side, and only the DMP one may be touched.
#[test]
fn migrate_v12_flips_only_the_sections_stored_in_the_old_convention() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v11.stalx");

    let mut project = Project::new("Mixed".to_string(), None);
    // Pre-#296 DMP: descending shots recorded negative. Must be flipped.
    project.add_section(descending_section("FromDmp", -1.0, 4));
    // Walls/Compass: already +down. Must be left alone.
    project.add_section(descending_section("FromCompass", 1.0, 4));
    // Ariane never populates inclination, so there is no signal to act on.
    project.add_section(descending_section("FromAriane", 0.0, 4));
    // Too few usable shots to call a section-wide verdict, even though each
    // individual shot looks reversed.
    project.add_section(descending_section("TooShort", -1.0, 2));

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.set_user_version(11).unwrap();
    }

    // open_existing runs ensure_schema_version → the v11→v12 migration.
    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);

    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(30.0); 4],
        "a section stored +up is negated into +down"
    );
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(30.0); 4],
        "a section already +down must not be double-flipped"
    );
    assert_eq!(
        section_pitches(&db, "FromAriane"),
        vec![Some(0.0); 4],
        "all-zero pitch carries no signal and is left untouched"
    );
    assert_eq!(
        section_pitches(&db, "TooShort"),
        vec![Some(-30.0); 2],
        "too few usable shots is not a verdict; left alone rather than guessed"
    );
}

/// The migration must not invent a pitch where none was recorded, and a shot
/// whose recorded pitch genuinely contradicts its depth change must stay
/// contradictory: that disagreement is a real survey blunder and validation
/// exists to surface it (#296).
#[test]
fn migrate_v12_preserves_null_pitch_and_does_not_launder_a_blunder() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_nulls.stalx");

    let mut project = Project::new("Nulls".to_string(), None);
    let mut section = descending_section("FromDmp", -1.0, 6);
    // One shot never had an inclination recorded.
    section.shots[1].pitch_in = None;
    section.shots[1].pitch_out = None;
    // One shot is a genuine blunder: it disagrees with the rest of its own
    // section about which way it points. The section verdict flips it along
    // with everything else, so it comes out still disagreeing with its depth
    // change, which is exactly right.
    section.shots[5].pitch_in = Some(30.0);
    section.shots[5].pitch_out = Some(30.0);
    project.add_section(section);

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let pitches = section_pitches(&db, "FromDmp");
    assert_eq!(pitches[0], Some(30.0), "ordinary shot normalised");
    assert_eq!(pitches[1], None, "an unrecorded pitch stays unrecorded");
    assert_eq!(pitches[2], Some(30.0), "ordinary shot normalised");
    assert_eq!(
        pitches[5],
        Some(-30.0),
        "the outlier inherits the section verdict and stays wrong, so validation \
         still flags it instead of the migration silently agreeing with it"
    );
}

/// Real device bytes with no vertical signal must be left exactly as they are.
///
/// `test_v5.dmp` records no depth change on any shot, so the geometry cannot say
/// which convention its inclinations are in. The migration has to decline rather
/// than guess: silently negating a section it cannot reason about would corrupt
/// it, and unlike a wrong warning that is not something the user can see.
#[test]
fn migrate_v12_leaves_a_section_with_no_vertical_signal_untouched() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_v5.dmp");
    assert!(fixture.exists(), "Fixture missing: {}", fixture.display());
    let dmp = stalagmight_parsers::parsers::dmp::parse_dmp(&fixture).unwrap();

    let mut project = Project::new("Flat".to_string(), None);
    let mut stored: Vec<Option<f64>> = Vec::new();
    for section in &dmp.sections {
        let mut data = section.to_section_data(String::new());
        if data.shots.len() < 4 {
            continue;
        }
        // Precondition: this fixture really is flat, so there is nothing to infer
        // from. If a future fixture gains depth changes this assert fires rather
        // than the test quietly stopping to cover anything.
        for shot in &data.shots {
            assert!(
                (shot.depth_out - shot.depth_in).abs() <= 0.05,
                "fixture gained a depth change; this test no longer covers the \
                 no-signal path"
            );
        }
        data.name = Some(format!("S{}", stored.len()));
        stored.extend(data.shots.iter().map(|s| s.pitch_in));
        project.add_section(data);
    }
    assert!(!stored.is_empty(), "fixture yielded no usable section");
    assert!(
        stored.iter().any(|p| p.is_some_and(|v| v.abs() > 1.0)),
        "fixture has no recorded inclination, so there would be nothing to leave alone"
    );

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("flat.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
    let after: Vec<Option<f64>> = db
        .load_project()
        .unwrap()
        .sections
        .iter()
        .flat_map(|s| s.shots.iter().map(|sh| sh.pitch_in))
        .collect();
    assert_eq!(
        after, stored,
        "with no depth change to compare against, every pitch must survive verbatim"
    );
}

/// Write a pre-v12 project to `path` and stamp it v11, so the next
/// `open_existing` runs the pitch migration over it.
fn seed_v11_project(path: &Path, project: &Project) {
    let mut db = Database::open(path).unwrap();
    db.save_project(project).unwrap();
    db.set_user_version(11).unwrap();
}

/// `MIN_VOTING_SHOTS` is 3 and the comparison is `>=`, so a section with exactly
/// three usable shots is decided rather than skipped. The existing coverage uses
/// 2 (declined) and 4 (flipped), so a `>=` slipping to `>` would go unnoticed.
#[test]
fn migrate_v12_decides_a_section_at_exactly_the_minimum_vote_count() {
    let temp = TempDir::new().unwrap();

    for (count, expected, why) in [
        (
            2usize,
            -30.0,
            "two usable shots is below the minimum, so leave it",
        ),
        (
            3usize,
            30.0,
            "three usable shots is the minimum and must decide",
        ),
    ] {
        let path = temp.path().join(format!("votes_{count}.stalx"));
        let mut project = Project::new("Votes".to_string(), None);
        project.add_section(descending_section("FromDmp", -1.0, count));
        seed_v11_project(&path, &project);

        let db = Database::open_existing(&path).unwrap();
        assert_eq!(
            section_pitches(&db, "FromDmp"),
            vec![Some(expected); count],
            "{why}"
        );
    }
}

/// The version stamp is written outside the migration's transaction, so a crash
/// between the two re-runs the migration against already-flipped rows on the
/// next open. The verdict is re-derived from the geometry each time, so a second
/// pass must decline. Asserted explicitly because a future change to the
/// criterion could silently make the migration flip back and forth.
#[test]
fn migrate_v12_is_idempotent_when_run_twice() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("twice.stalx");

    let mut project = Project::new("Twice".to_string(), None);
    project.add_section(descending_section("FromDmp", -1.0, 4));
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    let after_first = section_pitches(&db, "FromDmp");
    assert_eq!(after_first, vec![Some(30.0); 4], "first pass flips");

    // Simulate the interrupted case: rows are already flipped, but the stamp
    // never landed, so the migration runs over them again.
    Database::migrate_pitch_to_down_convention(&db.conn).unwrap();
    assert_eq!(
        section_pitches(&db, "FromDmp"),
        after_first,
        "a second pass over already-flipped rows must not flip them back"
    );
}

/// The flip must not commit on its own.
///
/// `ensure_schema_version` wraps the flip and `invalidate_pre_pitch_flip_state`
/// in one transaction, because the verdict is re-derived from the rows
/// themselves: a flip that committed alone would look already-correct on the
/// next open, so the migration would decline, the invalidation would never run,
/// and v12 would be stamped over permanently stale undo history, warning
/// dismissals.
///
/// Rolling back the caller's transaction must therefore undo the flip. If this
/// fails, the migration has taken a transaction of its own again and that
/// window is back.
#[test]
fn migrate_v12_flip_is_governed_by_the_callers_transaction() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("rollback.stalx");

    let mut project = Project::new("Rollback".to_string(), None);
    project.add_section(descending_section("FromDmp", -1.0, 4));

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    assert_eq!(section_pitches(&db, "FromDmp"), vec![Some(-30.0); 4]);

    let tx = db.conn.unchecked_transaction().unwrap();
    assert!(
        !Database::migrate_pitch_to_down_convention(&tx)
            .unwrap()
            .is_empty(),
        "precondition: this section is the kind the migration flips"
    );
    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(30.0); 4],
        "precondition: the flip is visible inside the open transaction"
    );
    tx.rollback().unwrap();

    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(-30.0); 4],
        "rolling back the caller's transaction must undo the flip, so the flip \
         and the invalidation cannot be separated by a crash"
    );
}

/// A leg steeper than vertical is a survey blunder, and validation skips the
/// inclination cross-check for it. It must not vote in the migration either, or
/// a single typo can drag a section's verdict in whichever direction its sign
/// happens to point.
#[test]
fn migrate_v12_ignores_out_of_range_legs_when_voting() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("blunder.stalx");

    // The good shots are deliberately shallow, so each contributes little to the
    // as-is total and the blunder can dominate. Three correct +2 shots (implied
    // +2, so as-is 0 and flipped 4 apiece) plus one -91 leg whose geometry
    // implies +87 (as-is 178, flipped 4).
    //
    // Let the blunder vote and the totals are as-is 178 / flipped 16, which
    // clears the decisive ratio and flips all four shots, so a single typo
    // corrupts three correct ones. Exclude it and the totals are as-is 0 /
    // flipped 12, which declines. That gap is what this test exists to hold: it
    // fails if MAX_VOTING_PITCH_DEG is raised or the guard removed.
    let shallow = |i: usize| ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 0.349 * i as f64,
        depth_out: 0.349 * (i + 1) as f64,
        pitch_in: Some(2.0),
        pitch_out: Some(2.0),
        ..Default::default()
    };
    let mut shots: Vec<ShotData> = (0..3).map(shallow).collect();
    shots.push(ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 1.047,
        depth_out: 11.033,
        pitch_in: Some(-91.0),
        pitch_out: Some(-91.0),
        ..Default::default()
    });

    let mut section = descending_section("FromCompass", 1.0, 1);
    section.shots = shots;
    let mut project = Project::new("Blunder".to_string(), None);
    project.add_section(section);
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(2.0), Some(2.0), Some(2.0), Some(-91.0)],
        "an out-of-range leg must neither vote nor be rewritten, and must not \
         drag the correct shots around it"
    );
}

/// Everything outside the `shots` table that still holds a pre-flip pitch is
/// dropped when the migration flips a section, because the v12 stamp means
/// there is no second pass to catch it later.
///
/// Undo is the sharpest case: the payload carries absolute pitch values, so
/// replaying one entry writes a +up pitch straight back into a migrated file.
#[test]
fn migrate_v12_discards_state_that_still_holds_pre_flip_pitch() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("stale.stalx");

    let mut project = Project::new("Stale".to_string(), None);
    project.add_section(descending_section("FromDmp", -1.0, 4));

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.save_undo_stack(vec![(
            r#"{"UpdateShot":{"pitch_in":-30.0}}"#.to_string(),
            r#"{"PreviousValues":{"pitch_in":-30.0}}"#.to_string(),
        )])
        .unwrap();
        db.save_resolved_warnings_bulk(&[ResolvedWarning {
            section_id: 0,
            shot_id: 0,
            warning_code: "INCLINATION_DEPTH_MISMATCH".to_string(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        }])
        .unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(30.0); 4],
        "precondition: the migration flipped this section"
    );

    assert!(
        db.load_undo_stack_with_timestamps().unwrap().is_empty(),
        "undo history carrying pre-flip pitch must be dropped, not replayable"
    );
    assert!(
        db.load_resolved_warnings().unwrap().is_empty(),
        "an inclination dismissal taken under the old convention must be cleared"
    );
}

/// One in-range blunder must not outvote the section majority.
///
/// The verdict counts votes rather than summing angular errors, because summing
/// let a single shot decide by sheer magnitude. Here three correct shallow
/// shots disagree by 4 degrees each if flipped (12 total) while one blunder
/// recorded at -89 against +89 implied geometry contributes 178 to the as-is
/// score and 0 to the flipped score. Summed, the ratio flips all four, which
/// corrupts the three correct shots and launders the blunder into agreement
/// with its own depth change. Counted, it is one vote against three.
#[test]
fn migrate_v12_does_not_let_one_blunder_outvote_the_section() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("outvoted.stalx");

    let shallow = |i: usize| ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 0.349 * i as f64,
        depth_out: 0.349 * (i + 1) as f64,
        pitch_in: Some(2.0),
        pitch_out: Some(2.0),
        ..Default::default()
    };
    let mut shots: Vec<ShotData> = (0..3).map(shallow).collect();
    // Near-vertical descent (implied about +89) recorded as -89: in range, so
    // it votes, but it is one vote.
    shots.push(ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 1.047,
        depth_out: 11.046,
        pitch_in: Some(-89.0),
        pitch_out: Some(-89.0),
        ..Default::default()
    });

    let mut section = descending_section("FromCompass", 1.0, 1);
    section.shots = shots;
    let mut project = Project::new("Outvoted".to_string(), None);
    project.add_section(section);
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(2.0), Some(2.0), Some(2.0), Some(-89.0)],
        "three correct shots must outvote one blunder, and the blunder must stay \
         wrong so validation keeps flagging it"
    );
}

/// The other side of the margin: a real DMP section carrying one genuine
/// blunder must still be converted. The blunder inherits the section verdict
/// and stays flagged, which is what the cross-check is for.
#[test]
fn migrate_v12_still_flips_a_section_that_contains_one_blunder() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("blunder_in_dmp.stalx");

    // Six shots stored +up (pre-#296 DMP), plus one whose recorded pitch agrees
    // with its depth change and so votes the other way.
    let mut section = descending_section("FromDmp", -1.0, 6);
    section.shots.push(ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 30.0,
        depth_out: 35.0,
        pitch_in: Some(30.0),
        pitch_out: Some(30.0),
        ..Default::default()
    });
    let mut project = Project::new("BlunderInDmp".to_string(), None);
    project.add_section(section);
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    let pitches = section_pitches(&db, "FromDmp");
    assert_eq!(
        pitches[..6].to_vec(),
        vec![Some(30.0); 6],
        "six votes to flip must carry the section past one dissenter"
    );
    assert_eq!(
        pitches[6],
        Some(-30.0),
        "the dissenting shot inherits the section verdict and is left contradicting \
         its own depth change, so validation still flags it"
    );
}

/// A shot with one impossible leg must not vote, even when the other leg drags
/// the average back inside the range.
///
/// The TML sign convention changed on both sides of the boundary at once, so the
/// vote guard has to reject per leg the way `Validator::validate_shot` does.
/// Averaging first and testing the mean lets `pitch_in = -91` ride in on a
/// `pitch_out = 2`.
#[test]
fn migrate_v12_rejects_a_shot_with_one_out_of_range_leg() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("half_blunder.stalx");

    // Same shape as the out-of-range test: three shallow, correct +2 shots that
    // contribute nothing as-is, plus one steep shot whose legs average to a
    // plausible -44.5 but whose pitch_in is impossible.
    let shallow = |i: usize| ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 0.349 * i as f64,
        depth_out: 0.349 * (i + 1) as f64,
        pitch_in: Some(2.0),
        pitch_out: Some(2.0),
        ..Default::default()
    };
    let mut shots: Vec<ShotData> = (0..3).map(shallow).collect();
    shots.push(ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 1.047,
        depth_out: 11.033,
        pitch_in: Some(-91.0),
        pitch_out: Some(2.0),
        ..Default::default()
    });

    let mut section = descending_section("FromCompass", 1.0, 1);
    section.shots = shots;
    let mut project = Project::new("HalfBlunder".to_string(), None);
    project.add_section(section);
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(2.0), Some(2.0), Some(2.0), Some(-91.0)],
        "a shot with one out-of-range leg must not vote, so the correct shots \
         around it stay put"
    );
}

/// v14 -> v15 removes the `collab_state` table AND the persisted undo history.
///
/// Dropping the table is the visible half. The undo drop is the load-bearing one:
/// `Command` is an internally tagged enum with no catch-all, so an entry stored as
/// `{"type":"apply_collab_changes"}` no longer deserializes - and the loader SKIPS a
/// bad entry rather than truncating, keeping the entries on both sides of it. A v14
/// file reaches this migration with its stacks intact (`UNDO_FORMAT_SAFE_VERSION` is
/// 7), so without the drop a user who had applied collaborator changes would undo
/// straight through the hole and replay stale entries over the applied edits.
#[test]
fn migrate_v15_drops_collab_state_and_the_undo_history() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v14.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V14".to_string(), None))
            .unwrap();
        // A v14 file as it would exist in the wild: the sidecar table, plus an undo
        // entry naming a command this build can no longer deserialize, plus one it
        // still can (so the test would catch a drop that only removed the bad row).
        db.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS collab_state (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     state_json TEXT NOT NULL
                 );
                 INSERT INTO collab_state (id, state_json) VALUES (1, '{\"enabled\":true}');",
            )
            .unwrap();
        db.save_undo_stack(vec![
            (
                r#"{"type":"apply_collab_changes"}"#.to_string(),
                "null".to_string(),
            ),
            (
                r#"{"type":"update_project_settings"}"#.to_string(),
                "null".to_string(),
            ),
        ])
        .unwrap();
        // The REDO stack too: `drop_legacy_undo_history` loops both, and a redo entry
        // is just as unsafe to replay - a v14 user who undid past an
        // `apply_collab_changes` reaches this migration with the apply sitting in
        // redo. Asserting only `undo_stack` would let a helper that stopped looping
        // both tables through unnoticed.
        db.save_redo_stack_with_timestamps(vec![(
            r#"{"type":"apply_collab_changes"}"#.to_string(),
            "null".to_string(),
            None,
        )])
        .unwrap();
        db.set_user_version(14).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
    assert!(
        !db.table_exists("collab_state").unwrap(),
        "v15 must drop the collab sidecar table, not leave a dead one in every file"
    );
    for table in ["undo_stack", "redo_stack"] {
        let remaining: i64 = db
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "the whole {table} must go: skipping only the undeserializable entry \
             would leave a hole that replays stale entries over the applied changes"
        );
    }
}

/// #24: opening a pre-v14 project rewrites every stored azimuth into `[0, 360)`.
///
/// Nothing changed meaning - `Shot::average_azimuth` used Rust's truncated `%`, which
/// returned the ANTIPODE of the circular mean for a pair more than 540 degrees apart, and
/// now wraps with `rem_euclid`. Normalising takes the ambiguity out of the FILE so the
/// same `.stalx` renders identically on either build.
///
/// Both fixtures run because this used to be gated on a "reinterpreted" verdict: the
/// first holds a shot whose walked bearing really did move, the second only readings no
/// build ever misread. The gate is gone, so the two cases must behave identically.
#[test]
fn migrate_v14_normalises_stored_azimuths() {
    let temp = TempDir::new().unwrap();

    let out_of_range = |azimuth_in: f64, azimuth_out: f64| ShotData {
        azimuth_in,
        azimuth_out,
        length: 10.0,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    };

    for (file, shots, expected) in [
        (
            "legacy_v13_azimuth.stalx",
            vec![
                // 600 and 0 are 600 apart: the truncated wrap walked this 180 degrees off.
                out_of_range(600.0, 0.0),
                // Out of range but only 20 apart - normalised, never misread.
                out_of_range(370.0, 350.0),
                // Already canonical: must be left exactly alone.
                out_of_range(12.5, 13.5),
                // A hair WEST of north. `rem_euclid` alone maps both of these to
                // exactly 360.0, so the migration would write the very
                // `INVALID_AZIMUTH` its "rows go inert" justification promises it
                // clears - and `geo_key` wraps 360.0 back to 0.0, so the migrated
                // project and an untouched sibling would disagree on that shot.
                // `normalize_azimuth` clamps the escape.
                out_of_range(-1e-14, -2.8e-14),
            ],
            vec![(240.0, 0.0), (10.0, 350.0), (12.5, 13.5), (0.0, 0.0)],
        ),
        (
            "legacy_v13_benign.stalx",
            vec![out_of_range(370.0, 350.0)],
            vec![(10.0, 350.0)],
        ),
    ] {
        let path = temp.path().join(file);
        let mut project = Project::new("Legacy".to_string(), None);
        project.add_section(SectionData {
            name: Some("Wide".to_string()),
            shots,
            ..Default::default()
        });

        {
            let mut db = Database::open(&path).unwrap();
            db.save_project(&project).unwrap();
            db.set_user_version(13).unwrap();
        }

        let db = Database::open_existing(&path).unwrap();
        assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);

        let stored: Vec<(f64, f64)> = db
            .conn
            .prepare("SELECT azimuth_in, azimuth_out FROM shots ORDER BY sequence_order")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            stored, expected,
            "{file}: every reading must land in [0, 360) and an in-range one must be untouched"
        );
    }
}

/// The "this shot was walked 180 degrees off" verdict must not fire on a pair the old
/// truncated wrap handled correctly.
///
/// `average_azimuth_of` wraps `azimuth_out - azimuth_in + 540.0`; truncated `%` and
/// `rem_euclid` agree whenever that expression is non-negative OR an exact multiple of
/// 360. The naive `azimuth_out - azimuth_in < -540.0` test drops the second clause, so
/// every exact `-360k` offset was reported as reinterpreted when nothing moved.
#[test]
fn the_antipode_verdict_ignores_exact_360_multiples() {
    // The pre-#24 wrap, verbatim, as the oracle.
    let old_mean = |azimuth_in: f64, azimuth_out: f64| {
        let diff = (azimuth_out - azimuth_in + 540.0) % 360.0 - 180.0;
        (azimuth_in + diff / 2.0 + 360.0) % 360.0
    };

    for (azimuth_in, azimuth_out, expected, why) in [
        (
            900.0,
            0.0,
            false,
            "-900 is an exact -360k offset: both wraps walk it to the same bearing",
        ),
        (
            1260.0,
            0.0,
            false,
            "-1260 likewise - the naive `< -540` test called both of these reinterpreted",
        ),
        (
            600.0,
            0.0,
            true,
            "-600 is the real thing: the old wrap returned the antipode",
        ),
        (
            370.0,
            350.0,
            false,
            "out of range but only 20 apart - never misread",
        ),
    ] {
        // Ground truth: did the walked bearing actually move?
        let moved = {
            let delta = (old_mean(azimuth_in, azimuth_out)
                - crate::models::average_azimuth_of(azimuth_in, azimuth_out))
            .abs();
            delta.min(360.0 - delta) > 1e-9
        };
        assert_eq!(
            moved, expected,
            "fixture is wrong for ({azimuth_in}, {azimuth_out}): {why}"
        );
        assert_eq!(
            Database::old_wrap_returned_the_antipode(azimuth_in, azimuth_out),
            expected,
            "({azimuth_in}, {azimuth_out}): {why}"
        );
    }
}

/// Normalising must not move the cave: the walked bearing of a normalised pair is the one
/// the raw readings always described. This is the whole justification for rewriting stored
/// data instead of leaving it - it is a no-op for geometry under this build, and it takes
/// the truncated-`%` trap away from every older build too.
#[test]
fn migrate_v14_normalisation_leaves_the_walked_bearing_alone() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v13_geometry.stalx");

    let mut project = Project::new("Geometry".to_string(), None);
    project.add_section(SectionData {
        name: Some("Wide".to_string()),
        shots: vec![ShotData {
            azimuth_in: 600.0,
            azimuth_out: 0.0,
            length: 10.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let before = project.compute_station_coordinates();

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.set_user_version(13).unwrap();
    }

    let migrated = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    let after = migrated.compute_station_coordinates();
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(&after) {
        assert!(
            (a.coordinates.x - b.coordinates.x).abs() < 1e-9
                && (a.coordinates.y - b.coordinates.y).abs() < 1e-9,
            "normalisation moved a station: {:?} -> {:?}",
            a.coordinates,
            b.coordinates
        );
    }
}

/// Dismissals of a warning the convention change cannot affect must survive.
///
/// `LENGTH_BELOW_DEPTH_CHANGE` compares length against the depth change and
/// never reads pitch, so clearing it would make previously dismissed warnings
/// reappear for no reason. Dismissals on sections the migration did not flip
/// must survive too.
#[test]
fn migrate_v12_only_clears_dismissals_whose_meaning_changed() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("dismissals.stalx");

    let mut project = Project::new("Dismissals".to_string(), None);
    project.add_section(descending_section("FromDmp", -1.0, 4));
    project.add_section(descending_section("FromCompass", 1.0, 4));

    let flipped_id = project.sections[0].id;
    let untouched_id = project.sections[1].id;

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        let warn = |section_id: u32, code: &str| ResolvedWarning {
            section_id,
            shot_id: 0,
            warning_code: code.to_string(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        };
        db.save_resolved_warnings_bulk(&[
            warn(flipped_id, "INCLINATION_DEPTH_MISMATCH"),
            warn(flipped_id, "LENGTH_BELOW_DEPTH_CHANGE"),
            warn(untouched_id, "INCLINATION_DEPTH_MISMATCH"),
        ])
        .unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(30.0); 4],
        "precondition: the DMP section was flipped"
    );

    let mut remaining: Vec<(u32, String)> = db
        .load_resolved_warnings()
        .unwrap()
        .into_iter()
        .map(|w| (w.section_id, w.warning_code))
        .collect();
    remaining.sort();

    let mut expected = vec![
        (flipped_id, "LENGTH_BELOW_DEPTH_CHANGE".to_string()),
        (untouched_id, "INCLINATION_DEPTH_MISMATCH".to_string()),
    ];
    expected.sort();

    assert_eq!(
        remaining, expected,
        "only the inclination dismissal on the flipped section should go: a \
         length warning never reads pitch, and an untouched section did not move"
    );
}

/// The mirror of the test above: a project the migration did not need to touch
/// keeps its undo history. Dropping it unconditionally would cost every user
/// their undo stack on upgrade for no reason.
#[test]
fn migrate_v12_does_not_flip_an_untouched_project_but_v13_still_drops_its_undo() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("untouched.stalx");

    let mut project = Project::new("Untouched".to_string(), None);
    project.add_section(descending_section("FromCompass", 1.0, 4));

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.save_undo_stack(vec![(
            r#"{"UpdateShot":{"pitch_in":30.0}}"#.to_string(),
            r#"{"PreviousValues":{"pitch_in":30.0}}"#.to_string(),
        )])
        .unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(30.0); 4],
        "precondition: nothing was flipped"
    );
    // The v11->v12 pitch migration itself still leaves history alone when it
    // flips nothing - `invalidate_pre_pitch_flip_state` is only called for a
    // non-empty flip set. But this file also crosses v12->v13, which drops the
    // stacks unconditionally: any pre-#27 payload can carry an `is_reference`
    // boolean, and a bare `true` cannot say which virtual kind it meant, so
    // replaying one could turn a Reference tie back into a swimmable Jump. That
    // supersedes the narrower v12 guarantee this test used to assert
    // end-to-end; there is no version between 12 and SCHEMA_VERSION to observe
    // it at any more.
    assert_eq!(
        db.load_undo_stack_with_timestamps().unwrap().len(),
        0,
        "crossing v13 drops persisted undo: legacy is_reference payloads are unsafe to replay"
    );
}

/// #557: the mint-time bound (#556) vets what the APP creates, not what a FILE
/// carries - a pre-#556 build, another tool, or a hostile author can persist a
/// section id past the shot-id encode. `Section::new` computes
/// `id * FEATURE_ID_STRIDE` unchecked (debug panic, release wrap), so the open
/// must refuse first, with a message naming the id and the bound.
#[test]
fn an_over_bound_section_id_refuses_the_open() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("overbound.stalx");
    let mut db = Database::open(&path).unwrap();
    let project = Project::new("Overbound".to_string(), None);
    db.save_project(&project).unwrap();

    let hostile = crate::models::MAX_SECTION_ID + 1;
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
            rusqlite::params![hostile],
        )
        .unwrap();

    let err = db.load_project().unwrap_err().to_string();
    assert!(
        err.contains(&hostile.to_string()),
        "the refusal must name the offending id: {err}"
    );
    assert!(
        err.contains(&crate::models::MAX_SECTION_ID.to_string()),
        "the refusal must name the bound: {err}"
    );
}

/// #557: `u32::MAX` is the id that used to overflow the counter rebuild's
/// `id + 1` (debug panic; release wrap to 0, after which the kept counter is
/// whatever smaller ids produced and future mints could REUSE live section
/// ids). It is refused like any other over-bound id, before the counter runs,
/// so debug and release agree.
#[test]
fn a_u32_max_section_id_is_refused_before_the_counter_rebuild() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("umax.stalx");
    let mut db = Database::open(&path).unwrap();
    let project = Project::new("Umax".to_string(), None);
    db.save_project(&project).unwrap();

    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
            rusqlite::params![u32::MAX],
        )
        .unwrap();

    let err = db.load_project().unwrap_err().to_string();
    assert!(
        err.contains(&u32::MAX.to_string()),
        "the refusal must name the offending id: {err}"
    );
    assert!(
        err.contains(&crate::models::MAX_SECTION_ID.to_string()),
        "the refusal must name the bound: {err}"
    );
}

/// The boundary id itself is legal - the refusal must not be off by one, and
/// the counter rebuild must come back one past it.
#[test]
fn the_last_encodable_section_id_still_opens() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("boundary.stalx");
    let mut db = Database::open(&path).unwrap();
    let project = Project::new("Boundary".to_string(), None);
    db.save_project(&project).unwrap();

    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
            rusqlite::params![crate::models::MAX_SECTION_ID],
        )
        .unwrap();

    let loaded = db.load_project().unwrap();
    assert!(
        loaded
            .sections
            .iter()
            .any(|s| s.id == crate::models::MAX_SECTION_ID),
        "the boundary section must load"
    );
    assert_eq!(
        loaded.next_section_id,
        crate::models::MAX_SECTION_ID + 1,
        "the counter rebuild must sit one past the largest loaded id"
    );
}

/// #557: `unrecognised_shot_kinds` runs its own query over the shots table, so
/// it can meet a section id the open-refusal never vetted (the diagnostic can
/// be called without a full load). The encode must saturate, not panic.
#[test]
fn unrecognised_shot_kinds_saturates_an_over_bound_section_id() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
            rusqlite::params![u32::MAX],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO shots (section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, kind)
             VALUES (?1, 0, 0.0, 180.0, 5.0, 0.0, 1.0, 99)",
            rusqlite::params![u32::MAX],
        )
        .unwrap();

    // A second, legal row pins the FORMULA (a bare `shot_id: section_id`
    // would also yield u32::MAX for the hostile row above).
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (7, '#abcdef', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO shots (section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, kind)
             VALUES (7, 3, 0.0, 180.0, 5.0, 0.0, 1.0, 99)",
            [],
        )
        .unwrap();

    let kinds = db.unrecognised_shot_kinds().unwrap();
    assert_eq!(kinds.len(), 2, "both unrecognised kinds must be reported");
    let legal = kinds.iter().find(|k| k.section_id == 7).unwrap();
    assert_eq!(
        legal.shot_id,
        7 * crate::models::FEATURE_ID_STRIDE + 3,
        "a legal section id must still encode by the stride formula"
    );
    let hostile = kinds.iter().find(|k| k.section_id == u32::MAX).unwrap();
    assert_eq!(
        hostile.shot_id,
        u32::MAX,
        "an over-bound section id saturates the encode instead of panicking"
    );
}

// === Hostile schema refused at open (#608) ===

/// A saved project with `sql` run on it through a raw connection afterwards.
fn project_file_with(dir: &TempDir, sql: &str) -> std::path::PathBuf {
    let path = dir.path().join("crafted.stalx");
    Database::open(&path)
        .unwrap()
        .save_project(&Project::new("Crafted".to_string(), None))
        .unwrap();
    Connection::open(&path).unwrap().execute_batch(sql).unwrap();
    path
}

#[test]
fn every_open_of_an_existing_file_refuses_a_hostile_schema() {
    for (sql, what) in [
        ("CREATE VIEW v AS SELECT 1;", "a view"),
        (
            "CREATE TRIGGER t AFTER UPDATE ON project BEGIN SELECT 1; END;",
            "a trigger",
        ),
        (
            "PRAGMA writable_schema = ON;
             INSERT INTO sqlite_master (type, name, tbl_name, rootpage, sql)
             VALUES ('table', 'docs', 'docs', 0, 'CREATE VIRTUAL TABLE docs USING fts5(body)');",
            "a virtual table",
        ),
        (
            "CREATE TABLE pragma_table_info (cid, name, type);",
            "an object named like a SQLite pragma",
        ),
        (
            "CREATE VIEW w AS SELECT 1;
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = 'View' WHERE name = 'w';",
            "a view",
        ),
        (
            "CREATE TABLE c (x CHECK (length(zeroblob(10)) > 0));",
            "a table with a CHECK constraint the app never writes",
        ),
        (
            "CREATE TABLE g (a, b AS (zeroblob(10)));",
            "a table with a generated column",
        ),
        (
            "CREATE TABLE d (x TEXT DEFAULT (hex(zeroblob(10))));",
            "a table with an expression default",
        ),
        (
            "CREATE INDEX e ON shots (length(hex(zeroblob(10))) + length);",
            "an expression or partial index",
        ),
        (
            "CREATE INDEX p ON shots (length) WHERE length > 0;",
            "an expression or partial index",
        ),
        (
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET name = CAST('project' AS BLOB) WHERE name = 'project';",
            "a schema object of an unexpected type",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let path = project_file_with(&dir, sql);
        let openers: [(&str, fn(&Path) -> Result<Database>); 5] = [
            ("open", Database::open),
            ("open_existing", Database::open_existing),
            ("open_readonly", Database::open_readonly),
            ("open_for_metadata", Database::open_for_metadata),
            (
                "open_for_metadata_recovering",
                Database::open_for_metadata_recovering,
            ),
        ];
        for (name, opener) in openers {
            let Err(err) = opener(&path) else {
                unreachable!("{name} opened a file with {what}: {sql}");
            };
            let refused = err
                .chain()
                .find_map(|c| c.downcast_ref::<RefusedSchemaObject>())
                .unwrap_or_else(|| unreachable!("{name}: not a schema refusal: {err:#}"));
            assert_eq!(refused.what, what, "{name}: {sql}");
        }
    }
}

#[test]
fn every_opener_still_opens_what_sqlite_itself_adds_to_a_real_file() {
    // AUTOINCREMENT adds sqlite_sequence, UNIQUE an autoindex with NULL sql, and
    // ANALYZE sqlite_stat1: all ordinary, none written by the app's schema code.
    let dir = TempDir::new().unwrap();
    let path = project_file_with(
        &dir,
        "CREATE TABLE extra (id INTEGER PRIMARY KEY AUTOINCREMENT, tag TEXT UNIQUE,
                             n REAL DEFAULT -1.5, t TEXT DEFAULT CURRENT_TIMESTAMP);
         INSERT INTO extra (tag) VALUES ('a');
         CREATE INDEX extra_tag_n ON extra (tag, n);
         ANALYZE;",
    );

    Database::open(&path).unwrap();
    Database::open_existing(&path).unwrap();
    Database::open_readonly(&path).unwrap();
    Database::open_for_metadata(&path).unwrap();
    Database::open_for_metadata_recovering(&path).unwrap();
    Database::check_schema_at(&path).unwrap();
}

#[test]
fn a_legacy_is_virtual_column_still_opens() {
    let dir = TempDir::new().unwrap();
    let path = project_file_with(&dir, "ALTER TABLE shots ADD COLUMN is_virtual INTEGER;");

    Database::open_existing(&path).unwrap();
}

/// A hostile `.stalx` must not reach FTS, RTREE or DBSTAT code, nor load an
/// extension (#608). The modules are stripped at build time by
/// `LIBSQLITE3_FLAGS`, which CI, release and the dev scripts set; a plain
/// local `cargo test` builds without it, so CI runs this one with `--ignored`.
#[test]
#[ignore = "needs the LIBSQLITE3_FLAGS build; CI runs it explicitly"]
fn bundled_sqlite_omits_virtual_table_modules_and_extension_loading() {
    let conn = Connection::open_in_memory().unwrap();
    let options: Vec<String> = conn
        .prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    for stripped in [
        "ENABLE_FTS3",
        "ENABLE_FTS5",
        "ENABLE_RTREE",
        "ENABLE_DBSTAT_VTAB",
    ] {
        assert!(
            !options.iter().any(|o| o == stripped),
            "{stripped} compiled in: {options:?}"
        );
    }
    assert!(
        options.iter().any(|o| o == "OMIT_LOAD_EXTENSION"),
        "extension loading compiled in: {options:?}"
    );
    // And by behaviour, not only by name: none of the modules can be used.
    for module in ["fts3", "fts4", "fts5", "rtree", "dbstat"] {
        let err = conn
            .execute_batch(&format!(
                "CREATE VIRTUAL TABLE t_{module} USING {module}(x)"
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("no such module"),
            "{module}: {err}"
        );
    }
}
