//! [#576 A11] One poison policy for every lock, driven through the public API.
//!
//! A poisoned mutex is what a panic leaves behind when it unwinds through a guard:
//! the data is still there, but every later `lock()` reports the crash. The lock
//! sites used to answer that three different ways - recover, skip silently, or
//! refuse - so after one internal crash some features carried on, some silently
//! did nothing (an edit could apply without its undo step), and some refused with
//! a bare "Failed to lock ..." until restart. Nobody was told anything.
//!
//! The policy these tests pin: every lock RECOVERS the data it guards and the
//! first recovery raises the `internal-error` event (the frontend's "save your work
//! and restart" banner) exactly once per session. A short, named list of
//! operations REFUSES instead, because continuing would replay or publish what a
//! crashed command may have half-written: undo/redo (all four verbs) and the
//! SpeleoDB upload. Those refuse with a message that says what happened.
//!
//! Each test poisons ONE cell the way a crash does - a thread panics while holding
//! it - and then drives ordinary commands. The per-project cells (project,
//! database, undo stack, command lock, SpeleoDB session, display cache) each have
//! their own test; `a_crash_in_any_cell_is_recovered_and_raises_one_banner` then
//! does the same for EVERY cell the state holds, enumerated through the helper's
//! type by `every_cell`. All data is synthetic.
//!
//! The source-level half of the guarantee is the type: `RecoveringMutex` has no
//! method that returns the std `LockResult`, so a `.lock().unwrap()`,
//! `.lock().expect(..)`, `if let Ok(..) = .lock()` or `.lock().map_err(..)` on a
//! cell does not compile, and `every_lock_the_state_holds_is_a_recovering_cell`
//! makes sure there is no lock in `AppState` that is not one.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::AppState;
use crate::core::events::{EventSink, InternalErrorEvent, IpcEvent};
use crate::core::Command;
use crate::models::poison_policy::LockCell;
use crate::models::{ShotData, SpeleoLink};

/// The banner event's wire name, spelled out so these tests state the contract the
/// frontend listens on (`IPC_EVENTS.INTERNAL_ERROR`).
const INTERNAL_ERROR_EVENT: &str = "internal-error";

#[test]
fn the_banner_event_is_the_declared_ipc_event() {
    assert_eq!(InternalErrorEvent::NAME, INTERNAL_ERROR_EVENT);
}

/// Every lock cell `AppState` holds, by EXHAUSTIVE destructuring: a field added
/// to `AppState` does not compile here until it is classified, and only a
/// `RecoveringMutex` can be listed as a `LockCell` - a std `Mutex` field would
/// have to be named under `_`, in plain sight, which review rejects.
fn every_cell(state: &AppState) -> Vec<&dyn LockCell> {
    let AppState {
        project,
        database,
        initial_save_deferred: _,
        undo_stack,
        config,
        config_write_lock,
        recent_projects,
        speleodb,
        edit_enabled: _,
        force_editing: _,
        display_settings_cache,
        opened_from,
        saves_renamed: _,
        command_lock,
        tile_cache,
        tilemap_cache,
        tile_fetch_stats: _,
        event_sink,
        active_loading,
        unrecognised_shot_kinds,
        history_discarded: _,
        newer_file_schema: _,
        workspace_mode: _,
        unreadable_project_settings,
        adjusted_cache,
        adjusted_cache_generation: _,
        speleodb_session_generation: _,
        speleodb_finished_here: _,
        tree_generation: _,
        speleodb_adoption_gate: _,
        park_after_mutation,
        speleodb_lock_ops,
        poison: _,
    } = state;
    vec![
        project,
        database,
        undo_stack,
        config,
        config_write_lock,
        recent_projects,
        speleodb,
        display_settings_cache,
        opened_from,
        command_lock,
        &**tile_cache,
        &**tilemap_cache,
        event_sink,
        active_loading,
        unrecognised_shot_kinds,
        unreadable_project_settings,
        adjusted_cache,
        park_after_mutation,
        speleodb_lock_ops,
    ]
}

#[test]
fn every_lock_the_state_holds_is_a_recovering_cell() {
    let state = AppState::new().unwrap();
    let cells = every_cell(&state);
    let names: BTreeSet<&str> = cells.iter().map(|cell| cell.cell_name()).collect();
    println!(
        "AppState lock cells: {} listed, {} registered with its poison monitor: {names:?}",
        cells.len(),
        state.poison_monitor().registered_cells(),
    );
    assert_eq!(
        cells.len(),
        state.poison_monitor().registered_cells(),
        "every cell created against the state's monitor is listed, and nothing else"
    );
    assert_eq!(names.len(), cells.len(), "each cell has its own name");
}

/// Poison `cell` the way a crash does: a thread panics while holding it.
fn poison_cell(cell: &dyn LockCell) {
    let crashed = std::thread::scope(|scope| {
        scope
            .spawn(|| cell.hold(&mut || panic!("synthetic panic in a cell (#576 A11 test)")))
            .join()
    });
    assert!(crashed.is_err(), "the helper thread panicked on purpose");
    assert!(cell.cell_poisoned());
}

/// Drive one operation through every cell: `(operation, answered)`. Undo is the
/// one refusing site here; it answers `true` when it refused for the right reason
/// or succeeded on a cell it does not refuse on.
fn sweep(state: &AppState, sid: u32, dir: &Path, refusing: bool) -> Vec<(&'static str, bool)> {
    let mut out = Vec::new();
    out.push(("edit", add_shot(state, sid).success));
    out.push(("read project", shot_count(state, sid) == Some(4)));
    out.push((
        "history view",
        state.get_undo_stack().len() == 2 && state.can_undo(),
    ));
    let undo = state.undo();
    out.push((
        "undo",
        if refusing {
            !undo.success && is_crash_refusal(undo.error.as_deref())
        } else {
            undo.success && state.redo().success
        },
    ));
    out.push(("save", save_to(state, dir).success));
    out.push((
        "display setting",
        state.save_display_setting("map.style", "dark").is_ok()
            && state
                .get_display_setting("map.style")
                .ok()
                .flatten()
                .as_deref()
                == Some("dark"),
    ));
    out.push((
        "config",
        state
            .modify_config(|c| {
                c.general.auto_save_interval_seconds = 61;
                Ok(())
            })
            .is_ok()
            && state.get_config().general.auto_save_interval_seconds == 61,
    ));
    out.push(("recent projects", state.get_recent_projects().len() == 1));
    out.push(("speleodb status", state.speleodb_session_status().is_ok()));
    out.push(("speleodb logout", state.speleodb_logout(true).is_ok()));
    out.push((
        "adjusted view",
        matches!(state.adjusted_stations_checked(), Ok(Some(ref s)) if !s.is_empty()),
    ));
    let token = state.start_named_load(Some("load"));
    state.cancel_loading(Some("load"));
    state.clear_active_load(&token);
    out.push((
        "load token",
        token.load(std::sync::atomic::Ordering::SeqCst),
    ));
    out.push((
        "unrecognised kinds",
        state.take_unrecognised_shot_kinds(0).is_empty(),
    ));
    out.push((
        "unread settings",
        state.unreadable_project_settings().is_empty(),
    ));
    out.push(("event sink", state.get_event_sink().is_some()));
    out.push(("tile cache", {
        drop(state.tile_cache().lock());
        drop(state.tilemap_cache().lock());
        true
    }));
    out.push(("close", state.close_project().success));
    out
}

#[test]
fn a_crash_in_any_cell_is_recovered_and_raises_one_banner() {
    let count = every_cell(&AppState::new().unwrap()).len();
    for index in 0..count {
        let _cfg = crate::test_helpers::isolated_config();
        let dir = tempfile::TempDir::new().unwrap();
        let (state, sink, sid) = open_state();
        let cell = every_cell(&state)[index];
        let name = cell.cell_name();
        poison_cell(cell);

        let refusing = matches!(name, "command_lock" | "project" | "undo_stack");
        let outcomes = sweep(&state, sid, dir.path(), refusing);
        let failed: Vec<&str> = outcomes
            .iter()
            .filter(|(_, answered)| !answered)
            .map(|(op, _)| *op)
            .collect();
        println!(
            "{name} poisoned: {} operations, failed {failed:?}; banners {:?}",
            outcomes.len(),
            sink.banner_cells(),
        );
        assert!(
            failed.is_empty(),
            "{name}: every operation recovers: {failed:?}"
        );
        assert_eq!(
            sink.banner_cells(),
            vec![name.to_owned()],
            "{name}: one banner"
        );
    }
}

/// Records every event the state emits.
#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl RecordingSink {
    /// How many internal-error banners the state has raised so far.
    fn banners(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(event, _)| event == INTERNAL_ERROR_EVENT)
            .count()
    }

    /// The `cell` field of every banner payload, in order.
    fn banner_cells(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(event, _)| event == INTERNAL_ERROR_EVENT)
            .map(|(_, payload)| payload["cell"].as_str().unwrap_or("?").to_owned())
            .collect()
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_owned(), payload));
    }

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

/// Poison `$cell` the way a crash does: a thread panics while holding its guard.
macro_rules! poison {
    ($cell:expr) => {
        std::thread::scope(|scope| {
            let crashed = scope
                .spawn(|| {
                    let _held = $cell.lock();
                    panic!("synthetic panic while holding a lock (#576 A11 test)");
                })
                .join();
            assert!(crashed.is_err(), "the helper thread panicked on purpose");
        })
    };
}

/// A state with a recording sink and an open project holding one section of three
/// shots. Returns the state, the sink and the section id.
fn open_state() -> (AppState, Arc<RecordingSink>, u32) {
    let state = AppState::new().unwrap();
    let sink = Arc::new(RecordingSink::default());
    state.set_event_sink(Arc::clone(&sink) as Arc<dyn EventSink>);
    assert!(
        state
            .execute(Command::CreateProject {
                name: "CaveA".to_owned(),
                location: None,
            })
            .success
    );
    let created = state.execute(Command::CreateSection {
        name: Some("SectionA".to_owned()),
        surveyor: None,
        date: None,
        color: "#336699".to_owned(),
        direction: None,
        shots: (0..3)
            .map(|i| ShotData {
                length: 5.0,
                azimuth_in: 30.0 * f64::from(i),
                azimuth_out: 30.0 * f64::from(i),
                ..Default::default()
            })
            .collect(),
    });
    assert!(created.success, "{:?}", created.error);
    (state, sink, created.section_id.unwrap())
}

/// An ordinary forward edit: append a shot to the section.
fn add_shot(state: &AppState, section_id: u32) -> crate::core::CommandResult {
    state.execute(Command::CreateShot {
        section_id,
        shot: ShotData {
            length: 2.5,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: None,
    })
}

fn shot_count(state: &AppState, section_id: u32) -> Option<usize> {
    state.get_project_state().and_then(|project| {
        project
            .sections
            .iter()
            .find(|s| s.id == section_id)
            .map(|s| s.shots.len())
    })
}

fn save_to(state: &AppState, dir: &Path) -> crate::core::CommandResult {
    state.execute(Command::SaveProject {
        path: Some(dir.join("CaveA.stalx")),
    })
}

/// True when `message` is the refusal the poison policy gives: it names the
/// internal error and tells the user what to do.
fn is_crash_refusal(message: Option<&str>) -> bool {
    message.is_some_and(|m| m.contains("internal error") && m.contains("restart"))
}

#[test]
fn a_poisoned_project_lock_recovers_and_raises_the_banner_once() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = tempfile::TempDir::new().unwrap();
    let (state, sink, sid) = open_state();

    poison!(state.project);

    let edit = add_shot(&state, sid);
    let shots = shot_count(&state, sid);
    let saved = save_to(&state, dir.path());
    let again = add_shot(&state, sid);
    println!(
        "project poisoned: edit -> success={} error={:?}; shots now {:?}; save -> \
         success={} error={:?}; second edit -> success={}; banners raised: {} {:?}",
        edit.success,
        edit.error,
        shots,
        saved.success,
        saved.error,
        again.success,
        sink.banners(),
        sink.banner_cells(),
    );

    assert!(edit.success, "the next edit recovers: {:?}", edit.error);
    assert_eq!(shots, Some(4), "and it reached the project");
    assert!(saved.success, "saving keeps working: {:?}", saved.error);
    assert!(again.success, "and so does every later edit");
    assert_eq!(sink.banners(), 1, "the banner is raised once, not per lock");
    assert_eq!(sink.banner_cells(), vec!["project".to_owned()]);
}

#[test]
fn a_poisoned_undo_stack_still_records_the_edit_and_refuses_history() {
    let _cfg = crate::test_helpers::isolated_config();
    let (state, sink, sid) = open_state();
    let depth_before = state.get_undo_stack().len();

    poison!(state.undo_stack);

    let edit = add_shot(&state, sid);
    let depth_after = state.get_undo_stack().len();
    let undo = state.undo();
    println!(
        "undo stack poisoned: edit -> success={}; undo entries {depth_before} -> \
         {depth_after}; undo -> success={} error={:?}; banners raised: {}",
        edit.success,
        undo.success,
        undo.error,
        sink.banners(),
    );

    assert!(edit.success, "{:?}", edit.error);
    assert_eq!(
        depth_after,
        depth_before + 1,
        "the edit's undo step is recorded, not silently skipped"
    );
    assert!(!undo.success);
    assert!(
        is_crash_refusal(undo.error.as_deref()),
        "undo refuses with a message that says why: {:?}",
        undo.error
    );
    assert_eq!(
        shot_count(&state, sid),
        Some(4),
        "the refusal changed nothing"
    );
    assert_eq!(sink.banners(), 1);
}

#[test]
fn a_poisoned_command_lock_lets_edits_through_and_refuses_history() {
    let _cfg = crate::test_helpers::isolated_config();
    let (state, sink, sid) = open_state();

    poison!(state.command_lock);

    let edit = add_shot(&state, sid);
    let undo = state.undo();
    println!(
        "command lock poisoned: edit -> success={} error={:?}; undo -> error={:?}; \
         banners raised: {}",
        edit.success,
        edit.error,
        undo.error,
        sink.banners(),
    );

    assert!(edit.success, "the next command recovers: {:?}", edit.error);
    assert_eq!(shot_count(&state, sid), Some(4));
    assert!(is_crash_refusal(undo.error.as_deref()), "{:?}", undo.error);
    assert_eq!(sink.banners(), 1);
}

#[test]
fn a_poisoned_database_lock_recovers() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = tempfile::TempDir::new().unwrap();
    let (state, sink, sid) = open_state();
    let first = save_to(&state, dir.path());
    assert!(first.success, "{:?}", first.error);

    poison!(state.database);

    let setting = state.save_display_setting("map.style", "dark");
    let read_back = state.get_display_setting("map.style");
    let edit = add_shot(&state, sid);
    let saved = state.execute(Command::SaveProject { path: None });
    println!(
        "database poisoned: display setting save -> {:?}, read back -> {:?}; edit -> \
         success={}; save -> success={} error={:?}; banners raised: {}",
        setting.as_ref().map_err(ToString::to_string),
        read_back.as_ref().map_err(ToString::to_string),
        edit.success,
        saved.success,
        saved.error,
        sink.banners(),
    );

    assert!(setting.is_ok(), "{setting:?}");
    assert_eq!(read_back.unwrap().as_deref(), Some("dark"));
    assert!(edit.success, "{:?}", edit.error);
    assert!(saved.success, "saving keeps working: {:?}", saved.error);
    assert_eq!(sink.banners(), 1);
}

#[test]
fn a_poisoned_speleodb_session_recovers() {
    let _cfg = crate::test_helpers::isolated_config();
    let (state, sink, _) = open_state();

    poison!(state.speleodb);

    let status = state.speleodb_session_status();
    println!(
        "SpeleoDB session poisoned: session status -> {:?}; banners raised: {}",
        status
            .as_ref()
            .map(|_| "answered")
            .map_err(|e| e.message.clone()),
        sink.banners(),
    );

    assert!(status.is_ok(), "the status read recovers: {status:?}");
    assert_eq!(sink.banners(), 1);
}

#[test]
fn a_poisoned_display_settings_cache_recovers() {
    let _cfg = crate::test_helpers::isolated_config();
    // No save path, so display settings live in the in-memory cache.
    let (state, sink, _) = open_state();

    poison!(state.display_settings_cache);

    let setting = state.save_display_setting("map.style", "dark");
    let read_back = state.get_display_setting("map.style");
    let all = state.load_display_settings();
    println!(
        "display cache poisoned: save -> {:?}, read back -> {:?}, load all -> {:?}; \
         banners raised: {}",
        setting.as_ref().map_err(ToString::to_string),
        read_back.as_ref().map_err(ToString::to_string),
        all.as_ref()
            .map(std::collections::HashMap::len)
            .map_err(ToString::to_string),
        sink.banners(),
    );

    assert!(setting.is_ok(), "{setting:?}");
    assert_eq!(read_back.unwrap().as_deref(), Some("dark"));
    assert!(all.is_ok());
    assert_eq!(sink.banners(), 1);
}

/// A remote link for the upload verb. Synthetic: nothing is ever sent, the
/// refusal comes first.
fn synthetic_link() -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://127.0.0.1:1/".to_owned(),
        project_id: "00000000-0000-4000-8000-000000000000".to_owned(),
        project_name: "CaveA".to_owned(),
        base_commit_sha: None,
        has_unpushed_edits: true,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

fn upload(state: &AppState) -> Result<(), String> {
    state
        .speleodb_upload(
            "http://127.0.0.1:1/",
            "tok",
            false,
            &synthetic_link(),
            None,
            false,
            &mut Vec::new(),
        )
        .map(|_| ())
        .map_err(|e| e.message)
}

#[test]
fn the_refusing_sites_refuse_with_a_clear_error() {
    let _cfg = crate::test_helpers::isolated_config();

    // Control: on a healthy state the upload fails too here (there is no cache
    // file for the synthetic project), but for its own reason - never the crash.
    let (healthy, _, _) = open_state();
    let control = upload(&healthy);
    assert!(
        !is_crash_refusal(control.as_ref().err().map(String::as_str)),
        "a healthy upload is not refused as a crash: {control:?}"
    );

    for cell in ["command_lock", "project", "undo_stack"] {
        let (state, sink, sid) = open_state();
        // Something to undo AND something to redo, so neither verb can fail for
        // having nothing to do.
        assert!(add_shot(&state, sid).success);
        assert!(add_shot(&state, sid).success);
        assert!(state.undo().success);
        match cell {
            "command_lock" => poison!(state.command_lock),
            "project" => poison!(state.project),
            _ => poison!(state.undo_stack),
        }
        let shots = shot_count(&state, sid);

        let undo = state.undo();
        let redo = state.redo();
        let undo_to = state.undo_to(0);
        let redo_to = state.redo_to(0);
        let pushed = upload(&state);
        println!(
            "{cell} poisoned: undo -> {:?}; redo -> {:?}; undo_to -> {:?}; redo_to -> \
             {:?}; upload -> {:?}; banners raised: {}",
            undo.error,
            redo.error,
            undo_to.error,
            redo_to.error,
            pushed,
            sink.banners(),
        );

        for (verb, result) in [
            ("undo", &undo),
            ("redo", &redo),
            ("undo_to", &undo_to),
            ("redo_to", &redo_to),
        ] {
            assert!(!result.success, "{cell}: {verb} must refuse");
            assert!(
                is_crash_refusal(result.error.as_deref()),
                "{cell}: {verb} says why it refused: {:?}",
                result.error
            );
        }
        assert!(
            is_crash_refusal(pushed.as_ref().err().map(String::as_str)),
            "{cell}: the upload refuses before anything leaves the machine: {pushed:?}"
        );
        assert_eq!(
            shot_count(&state, sid),
            shots,
            "{cell}: a refused history step changes nothing"
        );
        assert_eq!(sink.banners(), 1, "{cell}: one banner");
    }
}
