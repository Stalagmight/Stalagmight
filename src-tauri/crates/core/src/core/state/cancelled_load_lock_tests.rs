//! [#576 RV4-R-C2] A load or create cancelled while it waits for `command_lock`
//! does not commit.
//!
//! A1 made the load's commit wait for `command_lock` (an in-flight command: an
//! autosave, a batch import), but its last cancellation check ran BEFORE that
//! wait. A newer load's `start_new_load` cancels the waiting one, yet it still
//! committed once the lock freed - and with no fairness on the mutex the older
//! project could land last, so the backend held a project the frontend had
//! moved past. The finalize now re-checks once it holds the lock, still before
//! any state is written.
//!
//! The waits below give the load time to reach its commit and block on the
//! held lock; with the fix both orders (cancelled before the last pre-lock check
//! or while waiting) refuse, so the test cannot pass by luck after the fix. All
//! data is synthetic, in a temp dir.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::AppState;
use crate::models::{Project, SectionData, ShotData};

struct Recorder(Arc<Mutex<Vec<String>>>);

impl crate::core::events::EventSink for Recorder {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.0.lock().unwrap().push(format!("{event} {payload}"));
    }
    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

fn saved(dir: &Path, file: &str, name: &str) -> PathBuf {
    let path = dir.join(file);
    let mut project = Project::new(name.to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        shots: vec![ShotData {
            length: 3.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = crate::persistence::Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    path
}

/// Hold `command_lock` (an in-flight command), run `work` on another thread until
/// its progress events show `phase`, give it time to block on the lock, start a
/// newer load (which cancels it), then free the lock and return its outcome.
fn cancelled_while_waiting<T: Send>(
    state: &AppState,
    events: &Arc<Mutex<Vec<String>>>,
    phase: &str,
    work: impl FnOnce() -> T + Send,
) -> T {
    std::thread::scope(|scope| {
        let held = state.command_lock.lock();
        let running = scope.spawn(work);
        let started = Instant::now();
        while !events.lock().unwrap().iter().any(|e| e.contains(phase)) {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "never reached {phase}: {:?}",
                events.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(300));
        let _newer = state.start_new_load();
        drop(held);
        running.join().unwrap()
    })
}

#[test]
fn a_load_cancelled_while_waiting_for_the_command_lock_does_not_commit() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "a.stalx", "CaveA");
    let b = saved(temp.path(), "b.stalx", "CaveB");
    let state = AppState::new().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    state.set_event_sink(Arc::new(Recorder(Arc::clone(&events))));
    assert!(state.open_project(a).success);

    let outcome = cancelled_while_waiting(&state, &events, "\"finalizing\"", || {
        state.do_load_project_full(b.clone())
    });

    let open = state.get_project_state().map(|p| p.name);
    println!(
        "load cancelled while waiting for command_lock: returned {}; open project now {open:?}",
        match &outcome {
            Ok(_) => "Ok (committed)".to_owned(),
            Err(e) => format!("Err({})", e.code),
        }
    );
    assert!(outcome.is_err(), "the cancelled load committed {open:?}");
    assert_eq!(
        open.as_deref(),
        Some("CaveA"),
        "the open project is untouched"
    );
}

#[test]
fn a_create_cancelled_while_waiting_for_the_command_lock_does_not_commit() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "a.stalx", "CaveA");
    let state = AppState::new().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    state.set_event_sink(Arc::new(Recorder(Arc::clone(&events))));
    assert!(state.open_project(a).success);

    let outcome = cancelled_while_waiting(&state, &events, "\"saving_settings\"", || {
        state.do_create_project_full("CaveNew".to_string(), None, None, None, true)
    });

    let open = state.get_project_state().map(|p| p.name);
    println!(
        "create cancelled while waiting for command_lock: returned {}; open project now {open:?}",
        match &outcome {
            Ok(_) => "Ok (committed)".to_owned(),
            Err(e) => format!("Err({})", e.code),
        }
    );
    assert!(outcome.is_err(), "the cancelled create committed {open:?}");
    assert_eq!(
        open.as_deref(),
        Some("CaveA"),
        "the open project is untouched"
    );
}
