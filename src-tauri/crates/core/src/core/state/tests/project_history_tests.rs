//! The undo history a reopened project restores; merges that keep offsets, datums and comments.
//!
//! Lines 761-1700 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

#[test]
fn test_open_project_restores_undo_redo_and_assigns_closure_ids() {
    let temp_dir = TempDir::new().unwrap();
    let project_path = temp_dir.path().join("restore.stalx");

    let mut project = Project::new("Restore".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(2.0)],
            ..Default::default()
        })
        .id;

    project.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(section_id, 0).into(),
        station_b: StationRef::new(section_id, 1).into(),
        path_length: 2.0,
        misclosure_distance: 0.1,
        error_percentage: 5.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    let mut db = Database::open(&project_path).unwrap();
    db.save_project(&project).unwrap();

    // The UNDO entry needs real undo data: `load_from_persisted_with_timestamps`
    // drops `UndoData::None` from the undo stack, so `None` here would test that
    // filter instead of the restore. The REDO entry deliberately keeps `None` -
    // the redo stack is not filtered (a `None` redo entry re-executes its command
    // correctly), and `redo_len() == 1` below is the pin for that.
    let undo_entry = UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CreateProject {
            name: "Undo Project".to_string(),
            location: None,
        },
        undo_data: UndoData::InverseCommand(Command::CloseProject),
        timestamp: None,
    };
    let redo_entry = UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::None,
        timestamp: None,
    };

    let undo_persist = UndoStack::serialize_entry_with_timestamp(&undo_entry).unwrap();
    let redo_persist = UndoStack::serialize_entry_with_timestamp(&redo_entry).unwrap();
    db.save_undo_stack_with_timestamps(vec![undo_persist])
        .unwrap();
    db.save_redo_stack_with_timestamps(vec![redo_persist])
        .unwrap();
    drop(db);

    let state = AppState::new().unwrap();
    let result = state.open_project(project_path);
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.loop_closures.len(), 1);
    assert_ne!(project.loop_closures[0].id, 0);

    let undo_stack = state.undo_stack.lock();
    assert_eq!(undo_stack.len(), 1);
    assert_eq!(undo_stack.redo_len(), 1);

    let undo_entry = undo_stack.undo_entries().next().unwrap();
    match &undo_entry.command {
        Command::CreateProject { name, location } => {
            assert_eq!(name, "Undo Project");
            assert!(location.is_none());
        }
        other => panic!("Unexpected undo entry: {other:?}"),
    }

    let redo_entry = undo_stack.redo_entries().next().unwrap();
    match &redo_entry.command {
        Command::CloseProject => {}
        other => panic!("Unexpected redo entry: {other:?}"),
    }
}

/// [#303] A Full-mode linked project opens with ALL of its persisted history, however
/// far past the entry-count window it reaches.
///
/// The bound has to be armed BEFORE the restore, and that ordering is the whole test:
/// `load_from_persisted_with_timestamps` ends in `enforce_depth_limit`, so whatever bound
/// is in force at that instant is the one that trims. Arming afterwards meant the restore
/// ran under the startup default `Count(undo_history_limit)` and evicted everything older
/// than the window on the way in - which Count -> Bytes cannot undo. The next
/// full-rewrite save then wrote the truncation to disk and the next push shipped a commit
/// missing the history it was meant to carry.
#[test]
fn opening_a_full_mode_project_restores_history_deeper_than_the_count_window() {
    // `open_project` writes the recent-projects list into the global config, and the
    // entry-count window this test reasons about is read from it too.
    crate::test_helpers::with_isolated_config(|| {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("deep_history.stalx");

        // Half again over the 100-entry default window, so the shortfall is unambiguous.
        const PERSISTED: usize = 150;
        let now = chrono::Utc::now();
        let entries: Vec<_> = (0..PERSISTED)
            .map(|i| {
                let entry = UndoEntry {
                    replayed_at: None,
                    command_bytes: None,
                    command: Command::CreateProject {
                        name: format!("Entry {i}"),
                        location: None,
                    },
                    // Real undo data: the loader drops `UndoData::None` from the undo
                    // stack, which would test that filter instead of the depth bound.
                    undo_data: UndoData::InverseCommand(Command::CloseProject),
                    timestamp: Some(now + chrono::Duration::seconds(i as i64)),
                };
                UndoStack::serialize_entry_with_timestamp(&entry).unwrap()
            })
            .collect();

        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Deep".to_string(), None))
            .unwrap();
        // Full mode SET ON THE LINK, not inherited from the global default: this test is
        // about the arming order, so the policy must not depend on a config value another
        // test could reasonably change.
        db.save_speleo_link(Some(&SpeleoLink {
            undo_mode: Some(crate::models::UndoMode::Full),
            ..fixture_speleo_link()
        }))
        .unwrap();
        db.save_undo_stack_with_timestamps(entries).unwrap();
        drop(db);

        let state = AppState::new().unwrap();
        assert!(state.open_project(path).success);

        let stack = state.undo_stack.lock();
        assert_eq!(
            stack.len(),
            PERSISTED,
            "a Full-mode project is on the byte budget, so every persisted entry must \
             survive the open - a short stack means the count window was still armed \
             when the entries were restored"
        );
        // The oldest is the one a count trim evicts first, so name it explicitly.
        assert!(
            matches!(
                &stack.undo_entries().last().unwrap().command,
                Command::CreateProject { name, .. } if name == "Entry 0"
            ),
            "the OLDEST entry is what a front eviction takes, so it is the one worth pinning"
        );
    });
}

/// [#469] A count-window project opens the NEWEST window of a bloated undo table and
/// reads nothing beyond it - and what it reads is a WORKING history.
///
/// Nothing in the schema bounds `undo_stack`, and a `.stalx` is a file the app opens
/// rather than one it can trust: the loader used to `SELECT` and gunzip every row, and
/// only the restore that followed applied the entry window. A long-lived (or crafted)
/// file could therefore exhaust memory during open with the correct bound already armed
/// - it was applied one step too late to matter. The bound has to be in the QUERY, which
/// is what the row counts below pin.
///
/// The undo at the end is the other half: dropping the oldest rows must not corrupt the
/// sequence bookkeeping. It cannot, because the loaded sequences are discarded and every
/// save renumbers 0..n - which the final assertion checks rather than assumes.
#[test]
fn opening_a_count_window_project_reads_only_the_newest_window_of_a_bloated_table() {
    // `open_project`/`save_project` record the path in the global config, and the entry
    // window this test reasons about is read from it.
    crate::test_helpers::with_isolated_config(|| {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("bloated_history.stalx");

        let mut project = Project::new("Bloated".to_string(), None);
        project.add_section(SectionData {
            shots: vec![ShotData {
                comment: Some("edit 399".to_string()),
                ..shot(2.0)
            }],
            ..Default::default()
        });

        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // Ids as the loader assigns them, so the persisted entries address the shot the
        // reopened project actually has.
        let saved = db.load_project().unwrap();
        let section_id = saved.sections[0].id;
        let shot_id = saved.sections[0].shots[0].id;

        // Four times the 100-entry default window, so a load that read the whole table
        // and trimmed afterwards is distinguishable from one that never read it.
        const PERSISTED: usize = 400;
        let now = chrono::Utc::now();
        let rows: Vec<_> = (0..PERSISTED)
            .map(|i| {
                let entry = UndoEntry {
                    replayed_at: None,
                    command_bytes: None,
                    command: Command::UpdateShot {
                        section_id,
                        shot_id,
                        updates: ShotUpdates {
                            comment: Some(Some(format!("edit {i}"))),
                            ..Default::default()
                        },
                    },
                    undo_data: UndoData::PreviousValues {
                        section_id,
                        shot_id: Some(shot_id),
                        shot_uuid: None,
                        values_json: serde_json::to_string(&ShotUpdates {
                            comment: Some(Some(format!("comment {i}"))),
                            ..Default::default()
                        })
                        .unwrap(),
                        previous_offsets: HashMap::new(),
                    },
                    timestamp: Some(now + chrono::Duration::seconds(i as i64)),
                };
                UndoStack::serialize_entry_with_timestamp(&entry).unwrap()
            })
            .collect();
        db.save_undo_stack_with_timestamps(rows.clone()).unwrap();
        db.save_redo_stack_with_timestamps(rows).unwrap();
        drop(db);

        // The row that makes this test decisive rather than vacuous: the OLDEST entry in
        // each table is given a blob that claims to be gzip and is not, so any read that
        // reaches it fails - and a failed history load restores NOTHING. Without a
        // windowed query the assertions below therefore see an empty stack, whatever the
        // in-memory trim would have done afterwards.
        let conn = rusqlite::Connection::open(&path).unwrap();
        for table in ["undo_stack", "redo_stack"] {
            conn.execute(
                &format!("UPDATE {table} SET command_json = X'1F8B0000FFFF' WHERE sequence = 0"),
                [],
            )
            .unwrap();
        }
        drop(conn);
        assert!(
            Database::open_existing(&path)
                .unwrap()
                .load_undo_stack_with_timestamps()
                .is_err(),
            "the poison row must really poison a read that reaches it, or the window \
             assertions below prove nothing"
        );

        let state = AppState::new().unwrap();
        let window = state.undo_stack.lock().history_limit();
        assert!(
            window < PERSISTED,
            "the fixture must overflow the window it is testing: {window} vs {PERSISTED}"
        );
        assert!(state.open_project(path.clone()).success);

        {
            let stack = state.undo_stack.lock();
            assert_eq!(
                stack.len(),
                window,
                "a count-window project keeps its window and no more"
            );
            assert_eq!(
                stack.redo_len(),
                window,
                "the redo table is bounded the same way - a `.stalx` persists both"
            );
            assert!(
                matches!(
                    &stack.undo_entries().next().unwrap().command,
                    Command::UpdateShot { updates, .. }
                        if updates.comment == Some(Some("edit 399".to_string()))
                ),
                "the window has to be the NEWEST rows: the newest entry is the next undo"
            );
            assert!(
                matches!(
                    &stack.undo_entries().last().unwrap().command,
                    Command::UpdateShot { updates, .. }
                        if updates.comment == Some(Some("edit 300".to_string()))
                ),
                "...and it must be contiguous down to exactly one window's depth"
            );
        }

        // A working history, not merely a short one.
        assert!(state.undo().success);
        assert_eq!(
            state.get_project_state().unwrap().sections[0].shots[0]
                .comment
                .as_deref(),
            Some("comment 399"),
            "the newest restored entry must still undo to the state it recorded"
        );

        // And the rows dropped with the oldest history leave no hole behind: the save
        // renumbers the table from 0, exactly as it does for a history that was never
        // truncated.
        assert!(state.save_project(Some(path.clone())).success);
        let persisted = Database::open_existing(&path)
            .unwrap()
            .load_undo_stack_with_timestamps()
            .unwrap();
        assert!(!persisted.is_empty());
        assert_eq!(
            persisted.iter().map(|(seq, ..)| *seq).collect::<Vec<_>>(),
            (0..persisted.len() as i64).collect::<Vec<_>>(),
            "sequences are contiguous from 0 after a truncated load, so nothing that \
             reads them can trip over the dropped rows"
        );
    });
}

/// Shot with every nullable field populated - the fields a replayed legacy
/// (pre-v7) undo entry would silently clear under the v7 `double_option`
/// null-means-clear semantics.
fn shot_with_nullable_fields() -> ShotData {
    shot(2.0)
        .with_pitches(Some(5.0), Some(-5.0))
        .with_lrud(Some(1.5), Some(2.5), Some(0.5), Some(3.5))
        .with_comment("original")
        .with_marker_index(2)
        .with_temperature(14.5)
}

#[test]
fn test_open_project_drops_legacy_null_style_undo_history() {
    // open_project records the path in recent projects (stored in the global
    // config), so isolate the config from the real file.
    let _cfg = crate::test_helpers::isolated_config();
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("legacy_undo.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot_with_nullable_fields()],
        ..Default::default()
    });

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    let saved = db.load_project().unwrap();
    let section_id = saved.sections[0].id;
    let shot_id = saved.sections[0].shots[0].id;

    // Persist undo/redo entries exactly as a pre-v7 build wrote them: every
    // untouched nullable field serialized as an explicit null, in both the
    // command payload and the PreviousValues snapshot.
    let legacy_command = format!(
        r#"{{"type":"update_shot","section_id":{section_id},"shot_id":{shot_id},"updates":{{"azimuth_in":null,"azimuth_out":null,"length":4.0,"depth_in":null,"depth_out":null,"pitch_in":null,"pitch_out":null,"left":null,"right":null,"up":null,"down":null,"comment":null,"is_reference":null,"marker_index":null,"temperature":null}}}}"#
    );
    let legacy_values = r#"{"azimuth_in":null,"azimuth_out":null,"length":2.0,"depth_in":null,"depth_out":null,"pitch_in":null,"pitch_out":null,"left":null,"right":null,"up":null,"down":null,"comment":null,"is_reference":null,"marker_index":null,"temperature":null}"#;
    let legacy_undo_data = format!(
        r#"{{"PreviousValues":{{"section_id":{section_id},"shot_id":{shot_id},"values_json":{}}}}}"#,
        serde_json::to_string(legacy_values).unwrap()
    );
    db.save_undo_stack_with_timestamps(vec![(
        legacy_command.clone(),
        legacy_undo_data.clone(),
        None,
    )])
    .unwrap();
    db.save_redo_stack_with_timestamps(vec![(legacy_command, legacy_undo_data, None)])
        .unwrap();
    drop(db);

    // Rewind the version stamp so the file looks exactly like one written by
    // a pre-v7 build (Database::open stamped the current schema version).
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("PRAGMA user_version = 6", []).unwrap();
    drop(conn);

    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success);

    // The legacy stacks are dropped wholesale - nothing to undo or redo, so
    // nothing can replay the explicit nulls and clear fields.
    {
        let undo_stack = state.undo_stack.lock();
        assert_eq!(undo_stack.len(), 0, "legacy undo entries must be dropped");
        assert_eq!(
            undo_stack.redo_len(),
            0,
            "legacy redo entries must be dropped"
        );
    }

    // The shot's nullable fields are intact.
    let project = state.get_project_state().unwrap();
    let shot = &project.sections[0].shots[0];
    assert_eq!(shot.pitch_in, Some(5.0));
    assert_eq!(shot.left, Some(1.5));
    assert_eq!(shot.comment.as_deref(), Some("original"));
    assert_eq!(shot.marker_index, Some(2));
    assert_eq!(shot.temperature, Some(14.5));
}

#[test]
fn a_second_save_with_no_new_edit_still_persists_the_undo_history() {
    // [#303] THE regression guard for the bug the first cut of the hybrid stack
    // shipped: consecutive saves with nothing changed in between.
    //
    // `save_project` writes a FRESH `.stalx.tmp` and renames it over the target, then
    // writes the undo tables into the reopened file. A persist that "optimized" by
    // skipping rows it believed were already on disk therefore wrote NOTHING into a file
    // that had NOTHING - and the history vanished. Live, that is what a SpeleoDB push
    // hits: its own flush is a save, and it very often follows another one (an autosave,
    // or the save the frontend runs after a reload), so the commit it produced carried
    // no undo history at all.
    //
    // Two saves, one edit. The second must leave the history on disk.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("twice.stalx");

    let state = AppState::new().unwrap();
    assert!(state.create_project("Twice".to_string(), None).success);
    assert!(
        state
            .execute(Command::CreateSection {
                name: Some("only-edit".to_string()),
                surveyor: None,
                date: None,
                color: "#abcdef".to_string(),
                direction: None,
                shots: vec![ShotData {
                    length: 5.0,
                    ..Default::default()
                }],
            })
            .success
    );

    assert!(state.save_project(Some(path.clone())).success);
    let after_first = Database::open_existing(&path)
        .unwrap()
        .load_undo_stack_with_timestamps()
        .unwrap()
        .len();
    assert_eq!(
        after_first, 1,
        "the first save persists the edit's undo entry"
    );

    // No edit in between - exactly the shape a push's flush after an autosave has.
    assert!(state.save_project(Some(path.clone())).success);
    let after_second = Database::open_existing(&path)
        .unwrap()
        .load_undo_stack_with_timestamps()
        .unwrap()
        .len();
    assert_eq!(
        after_second, 1,
        "a save with nothing changed must still write the history - the file it writes \
         into was just rebuilt, so anything it skips is gone for good"
    );
}

#[test]
fn test_undo_after_reload_restores_exactly_the_edited_field() {
    // open_project/save_project record the path in recent projects (stored in
    // the global config), so isolate the config from the real file.
    let _cfg = crate::test_helpers::isolated_config();
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("new_format_undo.stalx");

    let mut project = Project::new("New Format".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot_with_nullable_fields()],
        ..Default::default()
    });

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);

    // Session 1: open, edit ONLY the comment, save (persists the undo stack
    // in the new format).
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    let (section_id, shot_id) = {
        let project = state.get_project_state().unwrap();
        (project.sections[0].id, project.sections[0].shots[0].id)
    };
    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            comment: Some(Some("edited".to_string())),
            ..Default::default()
        },
    });
    assert!(result.success);
    assert!(state.save_project(Some(path.clone())).success);

    // Session 2: fresh app instance, reload, undo.
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(path).success);
    {
        let stack = state2.undo_stack.lock();
        assert!(
            stack.can_undo(),
            "new-format undo history must survive reload"
        );
    }
    let project = state2.get_project_state().unwrap();
    assert_eq!(
        project.sections[0].shots[0].comment.as_deref(),
        Some("edited")
    );

    assert!(state2.undo().success);

    let project = state2.get_project_state().unwrap();
    let shot = &project.sections[0].shots[0];
    // Exactly the edited field is restored...
    assert_eq!(shot.comment.as_deref(), Some("original"));
    // ...and the untouched nullable fields were NOT cleared by the undo.
    assert_eq!(shot.pitch_in, Some(5.0));
    assert_eq!(shot.pitch_out, Some(-5.0));
    assert_eq!(shot.left, Some(1.5));
    assert_eq!(shot.right, Some(2.5));
    assert_eq!(shot.up, Some(0.5));
    assert_eq!(shot.down, Some(3.5));
    assert_eq!(shot.marker_index, Some(2));
    assert_eq!(shot.temperature, Some(14.5));
}

#[test]
fn test_merge_projects_preserves_offsets_zero_datums_and_closure_ids() {
    let temp_dir = TempDir::new().unwrap();

    let mut project_a = Project::new("A".to_string(), None);
    let section_a = project_a
        .add_section(SectionData {
            name: Some("Section A".to_string()),
            shots: vec![shot(1.0)],
            ..Default::default()
        })
        .id;
    project_a.set_section_offset(section_a, (1.0, 2.0));
    project_a
        .station_offsets
        .insert(StationRef::new(section_a, 0).into(), (0.5, -0.5, 1.0));
    let gps_a = GpsCoordinates::new(10.0, 20.0, 0.0).unwrap();
    project_a
        .store_zero_datum_without_repositioning(StationRef::new(section_a, 0), gps_a)
        .unwrap();
    project_a.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(section_a, 0).into(),
        station_b: StationRef::new(section_a, 1).into(),
        path_length: 1.0,
        misclosure_distance: 0.1,
        error_percentage: 10.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    let mut project_b = Project::new("B".to_string(), None);
    let section_b = project_b
        .add_section(SectionData {
            name: Some("Section B".to_string()),
            shots: vec![shot(2.0)],
            ..Default::default()
        })
        .id;
    project_b.set_section_offset(section_b, (3.0, 4.0));
    project_b
        .station_offsets
        .insert(StationRef::new(section_b, 0).into(), (-1.0, 1.5, 0.0));
    let gps_b = GpsCoordinates::new(-5.0, 40.0, 2.0).unwrap();
    project_b
        .store_zero_datum_without_repositioning(StationRef::new(section_b, 0), gps_b)
        .unwrap();
    project_b.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(section_b, 0).into(),
        station_b: StationRef::new(section_b, 1).into(),
        path_length: 2.0,
        misclosure_distance: 0.2,
        error_percentage: 10.0,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&project_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&project_b).unwrap();
    drop(db_a);
    drop(db_b);

    let state = AppState::new().unwrap();
    let output_path = temp_dir.path().join("merged-output.stalx");
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success);

    let db = Database::open_existing(&output_path).unwrap();
    let merged = db.load_project().unwrap();

    let merged_section_a = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Section A"))
        .unwrap()
        .id;
    let merged_section_b = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Section B"))
        .unwrap()
        .id;

    assert_eq!(
        merged.section_offsets().get(&merged_section_a),
        Some(&(1.0, 2.0))
    );
    assert_eq!(
        merged.section_offsets().get(&merged_section_b),
        Some(&(3.0, 4.0))
    );

    assert_eq!(
        merged.station_offset_for_ref(StationRef::new(merged_section_a, 0)),
        Some((0.5, -0.5, 1.0))
    );
    assert_eq!(
        merged.station_offset_for_ref(StationRef::new(merged_section_b, 0)),
        Some((-1.0, 1.5, 0.0))
    );

    assert!(merged
        .zero_datums
        .iter()
        .any(|zd| zd.station_ref.section_id == merged_section_a && zd.gps.latitude == 10.0));
    assert!(merged
        .zero_datums
        .iter()
        .any(|zd| zd.station_ref.section_id == merged_section_b && zd.gps.latitude == -5.0));

    let mut closure_ids = HashSet::new();
    for closure in &merged.loop_closures {
        assert_ne!(closure.id, 0, "closure IDs should be reassigned");
        closure_ids.insert(closure.id);
    }
    assert_eq!(closure_ids.len(), merged.loop_closures.len());

    assert!(merged.loop_closures.iter().any(|c| {
        c.station_a.section_id == merged_section_a && c.station_b.section_id == merged_section_a
    }));
    assert!(merged.loop_closures.iter().any(|c| {
        c.station_a.section_id == merged_section_b && c.station_b.section_id == merged_section_b
    }));
}

/// `MergeProjects` is classified `affects_geometry`, so it reaches `execute`'s
/// renormalisation epilogue - and that gate now fires on `None -> Some` as well
/// as on `Some -> Some`. An open project with no datum reads `frame_datum_key()
/// == None` on the before side, so if the merge ever swapped its freshly built
/// project into the open slot the gate would find a frame where there had been
/// none and translate the whole survey onto it. It does not: `merge_projects`
/// builds a `Project` locally, writes it to a NEW file and returns, without ever
/// touching `self.project` (there is no reference to it anywhere in
/// `project_merge.rs`), so both halves of the gate read the same unchanged open
/// project and it returns without measuring anything.
///
/// That distinction has teeth: a merged project's datums deliberately do NOT sit
/// at the origin - `store_zero_datum_without_repositioning` copies them exactly
/// so each source's own offsets survive - so a fire here would silently
/// translate the file the user just built.
///
/// Non-vacuous on both sides. The open project carries section offsets that
/// nothing in the merge could produce and no datum at all, so a swap-in is
/// visible in the offsets, in every station coordinate and in `zero_datums`; and
/// the sources DO carry datums, so the `None -> Some` transition the widened
/// gate now admits is genuinely available for it to fire on. Cited by name from
/// `AppState::renormalize_frame_if_changed`'s doc comment.
#[test]
fn merge_projects_does_not_renormalise_the_open_project() {
    let temp_dir = TempDir::new().unwrap();

    // The OPEN project: two free-floating sections, no datum anywhere.
    let state = state_with_project("Open");
    let open_a = state
        .execute(Command::CreateSection {
            name: Some("Open A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(7.0)],
        })
        .section_id
        .unwrap();
    let open_b = state
        .execute(Command::CreateSection {
            name: Some("Open B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(9.0)],
        })
        .section_id
        .unwrap();

    // Offsets nothing in this merge could ever produce, so a swap-in is loud.
    {
        let mut guard = state.project.lock();
        let project = guard.as_mut().unwrap();
        project.set_section_offset(open_a, (137.0, -42.0));
        project.set_section_offset(open_b, (-311.0, 63.0));
        assert!(
            project.zero_datums.is_empty() && project.frame_datum_key().is_none(),
            "the open project must have NO frame, so the gate sees None on the before side"
        );
    }
    let open_offsets_before = {
        let guard = state.project.lock();
        guard.as_ref().unwrap().section_offsets().clone()
    };
    let open_coords_before = abs_coord_map(&state.get_project_state().unwrap());

    // Two sources on disk, both carrying a datum, so the merged project has a
    // frame the gate could latch onto if it ever saw it.
    let mut source_project_a = Project::new("Source A".to_string(), None);
    let source_section_a = source_project_a
        .add_section(SectionData {
            name: Some("Source Section A".to_string()),
            shots: vec![shot(1.0)],
            ..Default::default()
        })
        .id;
    source_project_a.set_section_offset(source_section_a, (5.0, 6.0));
    source_project_a
        .store_zero_datum_without_repositioning(
            StationRef::new(source_section_a, 0),
            GpsCoordinates::new(20.35, -87.4, 0.0).unwrap(),
        )
        .unwrap();

    let mut source_project_b = Project::new("Source B".to_string(), None);
    let source_section_b = source_project_b
        .add_section(SectionData {
            name: Some("Source Section B".to_string()),
            shots: vec![shot(2.0)],
            ..Default::default()
        })
        .id;
    source_project_b.set_section_offset(source_section_b, (-8.0, 3.0));

    let source_a = temp_dir.path().join("source-a.stalx");
    let source_b = temp_dir.path().join("source-b.stalx");
    Database::open(&source_a)
        .unwrap()
        .save_project(&source_project_a)
        .unwrap();
    Database::open(&source_b)
        .unwrap()
        .save_project(&source_project_b)
        .unwrap();

    let output_path = temp_dir.path().join("merged-output.stalx");
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "merge failed: {:?}", result.error);

    // The merged file really does carry a frame - otherwise there is nothing for
    // the epilogue to have renormalised onto and the test below is vacuous.
    let merged = Database::open_existing(&output_path)
        .unwrap()
        .load_project()
        .unwrap();
    assert!(
        merged.frame_datum_key().is_some(),
        "the merged output must have a frame datum for this test to mean anything"
    );

    // ...and the open project is untouched: same offsets, same coordinates, still
    // no datum.
    let open_after = state.get_project_state().unwrap();
    assert_eq!(
        open_after.section_offsets(),
        &open_offsets_before,
        "MergeProjects translated the open project's sections"
    );
    let open_coords_after = abs_coord_map(&open_after);
    assert_eq!(
        open_coords_after.len(),
        open_coords_before.len(),
        "the open project's station set changed"
    );
    for (station_ref, before) in &open_coords_before {
        let after = open_coords_after
            .get(station_ref)
            .unwrap_or_else(|| panic!("{station_ref:?} vanished from the open project"));
        assert_coords_close(*after, *before);
    }
    assert!(
        open_after.zero_datums.is_empty(),
        "the open project gained a datum from a merge that writes a different file"
    );
}

#[test]
fn test_merge_projects_preserves_station_comments() {
    // merge_projects remaps every sibling cross-reference collection; station
    // comments must be carried over too (they were previously dropped silently
    // while offsets/datums/merges/closures were all copied).
    let temp_dir = TempDir::new().unwrap();

    let mut project_a = Project::new("A".to_string(), None);
    let section_a = project_a
        .add_section(SectionData {
            name: Some("Section A".to_string()),
            shots: vec![shot(1.0)],
            ..Default::default()
        })
        .id;
    project_a.ensure_shot_uuids();
    let key_a = project_a
        .key_for_ref(StationRef::new(section_a, 0))
        .unwrap();
    project_a
        .station_comments
        .insert(key_a, "junction with main line".to_string());

    let mut project_b = Project::new("B".to_string(), None);
    let section_b = project_b
        .add_section(SectionData {
            name: Some("Section B".to_string()),
            shots: vec![shot(2.0)],
            ..Default::default()
        })
        .id;
    project_b.ensure_shot_uuids();
    let key_b = project_b
        .key_for_ref(StationRef::new(section_b, 1))
        .unwrap();
    project_b
        .station_comments
        .insert(key_b, "sump start".to_string());

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    Database::open(&source_a)
        .unwrap()
        .save_project(&project_a)
        .unwrap();
    Database::open(&source_b)
        .unwrap()
        .save_project(&project_b)
        .unwrap();

    let state = AppState::new().unwrap();
    let output_path = temp_dir.path().join("merged-output.stalx");
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "merge failed: {:?}", result.error);

    let merged = Database::open_existing(&output_path)
        .unwrap()
        .load_project()
        .unwrap();
    let merged_section_a = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Section A"))
        .unwrap()
        .id;
    let merged_section_b = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Section B"))
        .unwrap()
        .id;

    assert_eq!(
        merged
            .station_comment_for_ref(StationRef::new(merged_section_a, 0))
            .map(String::as_str),
        Some("junction with main line"),
        "section A's station comment must survive the merge"
    );
    assert_eq!(
        merged
            .station_comment_for_ref(StationRef::new(merged_section_b, 1))
            .map(String::as_str),
        Some("sump start"),
        "section B's station comment must survive the merge"
    );
}
