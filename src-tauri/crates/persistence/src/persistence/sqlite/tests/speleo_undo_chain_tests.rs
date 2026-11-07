//! The link's undo chain and watermark in a push snapshot, its dirty bit and undo-mode edits.
//!
//! Lines 5681-6272 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`). `speleo_link_column_info` (former
//! lines 5193-5212, in `speleo_link_tests.rs`'s range) leads this file: only it uses that helper.

use super::*;

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
