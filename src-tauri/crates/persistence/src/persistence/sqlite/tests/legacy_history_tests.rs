//! Legacy and future-schema undo history on open, read-only opens and the stamp migrations.
//!
//! Lines 2469-2994 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

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

/// [#576 B6-D1] A file stamped ABOVE `SCHEMA_VERSION` is opened without a single
/// write: no migration, and no undo purge either.
///
/// `ensure_schema_version` runs no arm for a file from the future - there is
/// nothing sensible it could run - so its stacks still hold payloads no arm here
/// has vetted (a forged stamp of 99 over pre-v17 entries whose replay wipes a
/// section's name, surveyor and date). This build used to DELETE them on open.
/// It no longer does: a file a newer build wrote is opened read-only, its history
/// belongs to that build, and the load path keeps it out of this build's undo
/// stack instead (core `newer_schema_tests::a_newer_files_history_is_never_restored`).
/// The file itself - history, stamp and every byte - is left as the newer build
/// wrote it.
#[test]
fn a_file_stamped_with_a_future_schema_opens_without_a_single_write() {
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

    let bytes_before = std::fs::read(&path).unwrap();
    let mtime_before = std::fs::metadata(&path).unwrap().modified().unwrap();

    let db = Database::open_existing(&path).unwrap();
    let undo = db.load_undo_stack_with_timestamps().unwrap().len();
    let redo = db.load_redo_stack_with_timestamps().unwrap().len();
    println!("after open_existing: undo rows = {undo}, redo rows = {redo}");
    assert_eq!(
        undo, 1,
        "the open must leave a newer file's undo history alone"
    );
    assert_eq!(
        redo, 1,
        "the open must leave a newer file's redo history alone"
    );
    // The stamp itself is left alone - this build has not migrated anything and
    // must not claim it has - and the project still loads.
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION + 82);
    let project = db.load_project().unwrap();
    assert_eq!(project.name, "Future");
    assert_eq!(project.sections.len(), 1);
    drop(db);

    let bytes_after = std::fs::read(&path).unwrap();
    let mtime_after = std::fs::metadata(&path).unwrap().modified().unwrap();
    println!(
        "bytes identical = {}, mtime identical = {}",
        bytes_before == bytes_after,
        mtime_before == mtime_after
    );
    assert!(
        bytes_before == bytes_after,
        "opening a newer file must not change a byte of it"
    );
    assert_eq!(mtime_before, mtime_after, "nor touch its mtime");
}

/// [#576 B6-D1] `newer_schema_version` reads the stamp, and only a stamp ABOVE
/// this build's counts: a current file and an unstamped one whose project row
/// merely claims the future are not "newer" (the row is an unverified claim,
/// #485, and that shape is not one any build writes).
#[test]
fn newer_schema_version_reports_only_a_stamp_above_this_build() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("version.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("CaveA".to_string(), None))
            .unwrap();
        assert_eq!(
            db.newer_schema_version(),
            None,
            "a current file is not newer"
        );
        db.conn
            .execute_batch("UPDATE project SET schema_version = 99; PRAGMA user_version = 0;")
            .unwrap();
        assert_eq!(
            db.newer_schema_version(),
            None,
            "a row's claim without the stamp is not a newer file"
        );
        db.set_user_version(SCHEMA_VERSION + 1).unwrap();
        assert_eq!(db.newer_schema_version(), Some(SCHEMA_VERSION + 1));
    }
}

/// [#576 B6-D1] The session handle for a newer file is read-only: reads work,
/// every write - including the ungated display-setting one - fails, and the file
/// is left exactly as it was.
#[test]
fn into_read_only_reads_but_cannot_write() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("read_only_view.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("CaveA".to_string(), None))
            .unwrap();
        db.set_user_version(SCHEMA_VERSION + 1).unwrap();
    }
    let bytes_before = std::fs::read(&path).unwrap();

    let db = Database::open_existing(&path)
        .unwrap()
        .into_read_only(&path)
        .unwrap();
    assert_eq!(db.load_project().unwrap().name, "CaveA");
    assert_eq!(db.newer_schema_version(), Some(SCHEMA_VERSION + 1));
    let write = db.save_display_setting("showGrid", "true");
    println!("display-setting write through the read-only handle: {write:?}");
    assert!(
        write.is_err(),
        "a write through the read-only handle must fail"
    );
    assert!(
        db.save_project_name("Renamed").is_err(),
        "and so must a project write"
    );
    drop(db);

    assert!(
        std::fs::read(&path).unwrap() == bytes_before,
        "nothing reached the file"
    );
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
