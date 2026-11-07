//! Vacuumed push snapshots and what sanitize_push_snapshot keeps.
//!
//! Lines 6273-6854 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

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
