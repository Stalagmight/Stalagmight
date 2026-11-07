//! [#576 A5, A18(f), NF-AN3-6] Every project-slot commit writes every per-project
//! cell.
//!
//! A project-slot commit (both loads, both creates, close) installs a new session,
//! and every cell that describes the OPEN project has to be written by it - a cell
//! one commit forgets keeps the previous project's value. These tests open a
//! SpeleoDB-linked Full-mode project whose file carries a far-future inherited-
//! history watermark, dirty the transient cells no open sets, then run each commit
//! kind and read every cell back.
//!
//! What the previous project leaves behind when a commit forgets a cell:
//! - the Full-mode byte budget (A18(f)): the new project's history is bounded by
//!   32 MiB instead of the configured step limit;
//! - the inherited-history watermark (NF-AN3-6): every new undo stamp is floored
//!   at the old project's instant, a day in the future here;
//! - the database handle (the in-memory create): saves and link stamps would
//!   reach the previous project's file.
//!
//! All data is synthetic: projects built in memory and saved by this build.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use tempfile::TempDir;

use super::workspace_mode::WorkspaceMode;
use super::AppState;
use crate::core::{Command, DepthLimit, UndoData};
use crate::models::{Project, SectionData, ShotData, SpeleoLink, UndoMode};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

fn one_section_project(name: &str) -> Project {
    let mut project = Project::new(name.to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        shots: vec![ShotData {
            length: 4.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    project
}

/// A day ahead of now: far enough that a new stamp floored by it is unmistakable.
fn future_watermark() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() + chrono::Duration::days(1)
}

/// A saved SpeleoDB-linked project in Full undo mode whose file carries an
/// inherited-history watermark a day in the future.
fn linked_full_mode_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut db = Database::open(&path).unwrap();
    db.save_project(&one_section_project("CaveA")).unwrap();
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "CaveA".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: Some(UndoMode::Full),
    }))
    .unwrap();
    db.save_speleo_undo_watermark(Some(&future_watermark().to_rfc3339()))
        .unwrap();
    drop(db);
    path
}

fn local_file(dir: &Path, name: &str, project_name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut db = Database::open(&path).unwrap();
    db.save_project(&one_section_project(project_name)).unwrap();
    drop(db);
    path
}

/// Would the NEXT undo entry be stamped above the old project's watermark? Pushes
/// a probe entry, reads its stamp, and pops it again.
fn next_stamp_is_floored(state: &AppState) -> bool {
    let mut stack = state.undo_stack.lock();
    stack.push_with_data(Command::RepairIntegrity, UndoData::None);
    let stamp = stack.undo_entries().last().and_then(|e| e.timestamp);
    let _ = stack.pop_undo();
    // An unfloored stamp is the wall clock; a floored one sits a day ahead.
    stamp.is_some_and(|s| s > chrono::Utc::now() + chrono::Duration::hours(1))
}

/// Every per-project cell a project-slot commit must write.
#[derive(Debug, PartialEq)]
struct Cells {
    depth: DepthLimit,
    stamp_floored_by_old_watermark: bool,
    undo_len: usize,
    redo_len: usize,
    edit_enabled: bool,
    force_editing: bool,
    history_discarded: bool,
    newer_file_schema: Option<i32>,
    workspace: WorkspaceMode,
    unreadable_settings: Vec<String>,
    initial_save_deferred: bool,
    display_cache: Vec<(String, String)>,
    /// Name of the project the live database handle holds; `None` = no handle.
    database_holds: Option<String>,
    open_project: Option<String>,
}

fn read_cells(state: &AppState) -> Cells {
    let (depth, undo_len, redo_len) = {
        let stack = state.undo_stack.lock();
        (stack.depth_limit(), stack.len(), stack.redo_len())
    };
    let mut display_cache: Vec<(String, String)> = state
        .display_settings_cache
        .lock()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    display_cache.sort();
    let database_holds = state.database.lock().as_ref().map(|db| {
        db.load_project()
            .map_or_else(|e| format!("<unreadable: {e}>"), |p| p.name)
    });
    Cells {
        depth,
        stamp_floored_by_old_watermark: next_stamp_is_floored(state),
        undo_len,
        redo_len,
        edit_enabled: state.edit_enabled(),
        force_editing: state.force_editing(),
        history_discarded: state.history_discarded(),
        newer_file_schema: state.newer_file_schema(),
        workspace: state.workspace_mode(),
        unreadable_settings: state.unreadable_project_settings(),
        initial_save_deferred: state.initial_save_deferred.load(Ordering::Relaxed),
        display_cache,
        database_holds,
        open_project: state.get_project_state().map(|p| p.name),
    }
}

/// Open the linked Full-mode project, then dirty every transient cell no open
/// writes on its own, so a commit that forgets one shows up as a stale value.
fn dirty_previous_session(state: &AppState, dir: &Path) {
    let linked = linked_full_mode_file(dir, "previous.stalx");
    assert!(
        state.open_project(linked).success,
        "the linked fixture opens"
    );
    // Preconditions: the open armed the Full-mode byte budget and the watermark,
    // so the cells under test really do hold the previous project's values.
    assert!(
        matches!(state.undo_stack.lock().depth_limit(), DepthLimit::Bytes(_)),
        "a Full-mode linked project runs on the byte budget"
    );
    assert!(next_stamp_is_floored(state), "the watermark is in force");
    state
        .undo_stack
        .lock()
        .push_with_data(Command::RepairIntegrity, UndoData::None);
    state.set_force_editing(true);
    state.set_history_discarded(true);
    state.set_newer_file_schema(Some(9_999));
    state.set_unreadable_project_settings(vec!["validation".to_string()]);
    state.set_workspace_mode(WorkspaceMode::Plan);
    state
        .display_settings_cache
        .lock()
        .insert("showGrid".to_string(), "stale".to_string());
    state.initial_save_deferred.store(true, Ordering::Relaxed);
}

/// The cells of a fresh LOCAL session: what every commit must leave, bar the
/// project, the handle, the deferred flag and the display cache.
fn fresh(state: &AppState) -> Cells {
    Cells {
        depth: DepthLimit::Count(state.undo_stack.lock().history_limit()),
        stamp_floored_by_old_watermark: false,
        undo_len: 0,
        redo_len: 0,
        edit_enabled: true,
        force_editing: false,
        history_discarded: false,
        newer_file_schema: None,
        workspace: WorkspaceMode::Survey,
        unreadable_settings: Vec::new(),
        initial_save_deferred: false,
        display_cache: Vec::new(),
        database_holds: None,
        open_project: None,
    }
}

/// Run one commit kind over a dirtied session and compare every cell.
fn check(
    kind: &str,
    commit: impl FnOnce(&AppState, &Path),
    expected: impl FnOnce(Cells) -> Cells,
) -> bool {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let state = state_with_null_sink();
    dirty_previous_session(&state, temp.path());
    commit(&state, temp.path());
    let actual = read_cells(&state);
    let expected = expected(fresh(&state));
    let ok = actual == expected;
    println!(
        "{kind}: {}",
        if ok {
            "every cell fresh"
        } else {
            "STALE CELLS"
        }
    );
    if !ok {
        for (field, got, want) in diff(&actual, &expected) {
            println!("    {field}: got {got}, expected {want}");
        }
    }
    ok
}

fn diff(actual: &Cells, expected: &Cells) -> Vec<(&'static str, String, String)> {
    let mut out = Vec::new();
    macro_rules! cmp {
        ($($field:ident),*) => {$(
            if actual.$field != expected.$field {
                out.push((stringify!($field), format!("{:?}", actual.$field), format!("{:?}", expected.$field)));
            }
        )*};
    }
    cmp!(
        depth,
        stamp_floored_by_old_watermark,
        undo_len,
        redo_len,
        edit_enabled,
        force_editing,
        history_discarded,
        newer_file_schema,
        workspace,
        unreadable_settings,
        initial_save_deferred,
        display_cache,
        database_holds,
        open_project
    );
    out
}

/// The default display settings a deferred File > New seeds the in-memory cache
/// with (it has no database to read them from).
fn default_display_cache() -> Vec<(String, String)> {
    let d = crate::core::loading::DisplaySettings::default();
    let mut v: Vec<(String, String)> = [
        ("showStations", d.show_stations.to_string()),
        ("showSectionNames", d.show_section_names.to_string()),
        ("showStationLabels", d.show_station_labels.to_string()),
        ("showShotData", d.show_shot_data.to_string()),
        ("showReferenceShots", d.show_reference_shots.to_string()),
        ("showJumps", d.show_jumps.to_string()),
        ("showZeroDatum", d.show_zero_datum.to_string()),
        ("showGrid", d.show_grid.to_string()),
        ("showSatellite", d.show_satellite.to_string()),
        ("showDepthGradient", d.show_depth_gradient.to_string()),
        ("gridSpacing", d.grid_spacing.to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    v.sort();
    v
}

#[test]
fn every_project_slot_commit_writes_every_per_project_cell() {
    let mut all_fresh = true;

    all_fresh &= check(
        "File > New (deferred save)",
        |state, dir| {
            state
                .do_create_project_full(
                    "CaveNew".to_string(),
                    None,
                    None,
                    Some(dir.join("new_deferred.stalx")),
                    true,
                )
                .unwrap();
        },
        |f| Cells {
            initial_save_deferred: true,
            display_cache: default_display_cache(),
            open_project: Some("CaveNew".to_string()),
            ..f
        },
    );

    all_fresh &= check(
        "File > New (saved at once)",
        |state, dir| {
            state
                .do_create_project_full(
                    "CaveNew".to_string(),
                    None,
                    None,
                    Some(dir.join("new_saved.stalx")),
                    false,
                )
                .unwrap();
        },
        |f| Cells {
            database_holds: Some("CaveNew".to_string()),
            open_project: Some("CaveNew".to_string()),
            ..f
        },
    );

    all_fresh &= check(
        "in-memory create (Command::CreateProject)",
        |state, _| {
            let created = state.execute(Command::CreateProject {
                name: "CaveMem".to_string(),
                location: None,
            });
            assert!(created.success, "{:?}", created.error);
        },
        |f| Cells {
            open_project: Some("CaveMem".to_string()),
            ..f
        },
    );

    all_fresh &= check(
        "close",
        |state, _| assert!(state.close_project().success),
        |f| f,
    );

    all_fresh &= check(
        "load (load_project_full)",
        |state, dir| {
            let path = local_file(dir, "local_full.stalx", "CaveLocal");
            state.do_load_project_full(path).unwrap();
        },
        |f| Cells {
            database_holds: Some("CaveLocal".to_string()),
            open_project: Some("CaveLocal".to_string()),
            ..f
        },
    );

    all_fresh &= check(
        "load (legacy open_project)",
        |state, dir| {
            let path = local_file(dir, "local_legacy.stalx", "CaveLocal");
            assert!(state.open_project(path).success);
        },
        |f| Cells {
            database_holds: Some("CaveLocal".to_string()),
            open_project: Some("CaveLocal".to_string()),
            ..f
        },
    );

    assert!(
        all_fresh,
        "a project-slot commit left a cell holding the previous project's value (see above)"
    );
}

/// A project whose persisted undo table holds a row the loader refuses: a gzip blob
/// whose trailer claims far more than one row may inflate to (the same hostile
/// shape `a_history_that_cannot_be_read_sets_the_discarded_flag` uses).
fn unreadable_history_file(dir: &Path, name: &str) -> PathBuf {
    let path = local_file(dir, name, "CaveBroken");
    let mut liar = vec![0x1f, 0x8b, 0x08, 0x00];
    liar.extend_from_slice(&[0_u8; 8]);
    liar.extend_from_slice(&u32::MAX.to_le_bytes());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![0_i64, liar.clone(), liar, "2026-08-08T00:00:00+00:00"],
    )
    .unwrap();
    path
}

/// [#473] Both loads record what their OWN read of the history did - set when it could
/// not be read, cleared when it could - through the shared commit. This replaced a
/// count of `set_history_discarded(history_discarded)` spellings in the source.
#[test]
fn both_loads_record_whether_their_history_could_be_read() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    for (kind, full_load) in [("load_project_full", true), ("open_project", false)] {
        for unreadable in [true, false] {
            let state = state_with_null_sink();
            state.set_history_discarded(!unreadable);
            let path = if unreadable {
                unreadable_history_file(temp.path(), &format!("{kind}-broken.stalx"))
            } else {
                local_file(temp.path(), &format!("{kind}-intact.stalx"), "CaveIntact")
            };
            if full_load {
                state.do_load_project_full(path).unwrap();
            } else {
                assert!(state.open_project(path).success);
            }
            println!(
                "{kind}: history unreadable = {unreadable}, discarded flag = {}",
                state.history_discarded()
            );
            assert_eq!(state.history_discarded(), unreadable, "{kind}");
        }
    }
}
