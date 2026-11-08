//! [#576 NF-FIX3A-2 / RV4-RUST-M-1 / C2] A file import runs off the main thread,
//! so a Cancel dispatched while it reads its file stops it, and its progress
//! reaches the window while it runs.
//!
//! Tauri dispatches every IPC call from the main thread: its handler parses the
//! arguments and, for a plain (non-`async`) command, runs the command right there
//! (tauri-macros `body_blocking`). So the test models that thread directly: ONE
//! thread - this one - hands each request to the real `generate_handler!` wiring
//! through `Webview::on_message`, exactly as the webview's IPC handler does, and
//! then dispatches the user's Cancel. A command that runs on the dispatching
//! thread holds it until the import has finished, so the Cancel is only
//! dispatched after the import landed - the defect. A command on the blocking
//! pool returns the thread at once, the Cancel reaches the import in its read
//! phase, and nothing is committed.
//!
//! The import is held in its read phase by `import_runner::parse_gate` (at most
//! `HOLD_LIMIT`), keyed by its file path. Every file is synthetic, written into a
//! temp dir here.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponse};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{App, Listener, Manager, WebviewWindow, WebviewWindowBuilder};
use tempfile::TempDir;

use super::command_tests::made_for;
use super::import_command_tests::{
    synthetic_compass_dat, synthetic_tmlu, synthetic_walls_srv, write,
};
use super::import_runner::parse_gate;
use crate::core::state::AppState;
use stalagmight_core::test_fixtures::state_with_project;

/// A `tauri::test` app wired with the real import commands and `cancel_loading`,
/// managing an app state with an open, empty project, plus its window.
fn app_with_import_commands() -> (App<MockRuntime>, WebviewWindow<MockRuntime>) {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            crate::commands::import::execute_tml_import,
            crate::commands::import::execute_compass_import,
            crate::commands::import::execute_walls_import,
            crate::commands::import::execute_gpx_import,
            crate::commands::project::cancel_loading,
        ])
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let state = state_with_project("CaveB");
    app.manage(state);
    let window = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("window");
    (app, window)
}

fn request(cmd: &str, body: Value) -> InvokeRequest {
    InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "http://tauri.localhost".parse().expect("url"),
        body: InvokeBody::Json(body),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    }
}

/// Hand `request` to the app's IPC handler on THIS thread, as the webview's
/// handler does on the main thread; the response lands on the returned channel.
fn dispatch(window: &WebviewWindow<MockRuntime>, request: InvokeRequest) -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    window.as_ref().clone().on_message(
        request,
        Box::new(move |_webview, _cmd, response, _callback, _error| {
            let answer = match response {
                InvokeResponse::Ok(body) => body.deserialize::<Value>().unwrap_or(Value::Null),
                InvokeResponse::Err(error) => json!({ "ipc_error": error.0 }),
            };
            let _ = tx.send(answer);
        }),
    );
    rx
}

/// A command's answer as the webview receives it (#576 C4): the resolved value,
/// or the rejected `CommandError` object.
fn answer_of(answer: Value) -> Result<Value, Value> {
    match answer.get("ipc_error") {
        Some(error) => Err(error.clone()),
        None => Ok(answer),
    }
}

/// The rejected answer's display message, if it was rejected.
fn error_message(answer: &Result<Value, Value>) -> Option<&str> {
    answer
        .as_ref()
        .err()
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
}

/// The rejected answer's routing code (`CommandError::code`), if it was rejected.
fn error_code(answer: &Result<Value, Value>) -> Option<&str> {
    answer
        .as_ref()
        .err()
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
}

/// One importer's command, the synthetic file it imports and its IPC arguments.
struct ImportCase {
    command: &'static str,
    path: PathBuf,
    args: Value,
}

fn cases(dir: &TempDir) -> Vec<ImportCase> {
    let gpx = write(
        dir,
        "CaveB.gpx",
        r#"<gpx version="1.1"><trk><name>Surface</name><trkseg>
            <trkpt lat="0.0" lon="0.0"/>
            <trkpt lat="0.0" lon="0.0005"><ele>350</ele></trkpt>
            <trkpt lat="0.0" lon="0.0010"><ele>349</ele></trkpt>
        </trkseg></trk></gpx>"#,
    );
    vec![
        ImportCase {
            command: "execute_tml_import",
            path: synthetic_tmlu(dir),
            args: json!({ "includeExcluded": false, "commentsOnStations": false }),
        },
        ImportCase {
            command: "execute_compass_import",
            path: synthetic_compass_dat(dir),
            args: json!({
                "includeExcluded": false,
                "applyCorrections": true,
                "commentsOnStations": false,
            }),
        },
        ImportCase {
            command: "execute_walls_import",
            path: synthetic_walls_srv(dir),
            args: json!({ "commentsOnStations": false, "includeSplays": false }),
        },
        ImportCase {
            command: "execute_gpx_import",
            path: gpx,
            args: json!({
                "importWaypoints": true,
                "importTracks": true,
                "importRoutes": true,
                "sectionPlacements": [],
                "depthInverted": false,
            }),
        },
    ]
}

fn body_for(case: &ImportCase, app: &App<MockRuntime>) -> Value {
    let mut body = case.args.clone();
    body["path"] = json!(case.path);
    // [#576 W44-NF3] The name the dialog gives its import; its Cancel names it too.
    body["loadToken"] = json!(case.command);
    // [#576 FZ-FZ-INT-7] What the frontend sends with an edit: the open project.
    body["expectedProject"] = match made_for(app) {
        Some(open) => json!({ "createdAt": open.created_at, "filePath": open.file_path }),
        None => Value::Null,
    };
    body
}

/// What one dispatched import did, as the main thread saw it.
struct Observed {
    /// How long handing the import to the IPC handler held the dispatching thread.
    dispatch_held_thread: Duration,
    /// Whether the import was still running when its read-phase progress event
    /// was in hand.
    running_when_progress_seen: bool,
    progress_phase: String,
    result: Result<Value, Value>,
    sections: usize,
    undo_available: bool,
}

fn run_with_cancel_in_read_phase(case: &ImportCase) -> Observed {
    let (app, window) = app_with_import_commands();
    let (progress_tx, progress) = mpsc::channel::<String>();
    app.listen_any("loading-progress", move |event| {
        let _ = progress_tx.send(event.payload().to_string());
    });
    let parked = parse_gate::park(&case.path);

    // The main thread dispatches the import.
    let started = Instant::now();
    let import = dispatch(&window, request(case.command, body_for(case, &app)));
    let dispatch_held_thread = started.elapsed();

    // The read phase is reported while the import is still running...
    let progress_payload = progress
        .recv_timeout(Duration::from_secs(5))
        .expect("the import reports its read phase");
    // `Disconnected` (the responder dropped unanswered) counts as no early answer;
    // the bounded wait for the answer below then fails the test.
    let early = match import.try_recv() {
        Ok(answer) => Some(answer),
        Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
    };
    let running_when_progress_seen = early.is_none();
    let progress_phase = serde_json::from_str::<Value>(&progress_payload)
        .ok()
        .and_then(|v| v.get("phase").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    assert!(
        parked.wait_entered(Duration::from_secs(5)),
        "{}: the import reached its read phase",
        case.command
    );

    // ...and the user presses Cancel: the main thread dispatches it.
    let cancel = dispatch(
        &window,
        request("cancel_loading", json!({ "loadToken": case.command })),
    );
    cancel
        .recv_timeout(Duration::from_secs(5))
        .expect("cancel_loading answers");
    parked.release();

    let answer = early
        .or_else(|| import.recv_timeout(Duration::from_secs(20)).ok())
        .expect("the import answers");
    let result = answer_of(answer);
    let state = app.state::<AppState>();
    let sections = state.get_project_state().expect("project").sections.len();
    Observed {
        dispatch_held_thread,
        running_when_progress_seen,
        progress_phase,
        result,
        sections,
        undo_available: state.can_undo(),
    }
}

#[test]
fn a_cancel_dispatched_while_an_import_reads_its_file_stops_it_with_nothing_committed() {
    let dir = TempDir::new().expect("temp dir");
    // Every importer runs before anything is asserted, so a failing run shows
    // what each of the four did.
    let observed: Vec<(ImportCase, Observed)> = cases(&dir)
        .into_iter()
        .map(|case| {
            let seen = run_with_cancel_in_read_phase(&case);
            (case, seen)
        })
        .collect();
    for (case, seen) in &observed {
        println!(
            "{}: dispatch held the main thread {:.2} s; still running when its '{}' progress \
             arrived: {}; result success={} error={:?}; sections in the project={}; undo \
             available={}",
            case.command,
            seen.dispatch_held_thread.as_secs_f64(),
            seen.progress_phase,
            seen.running_when_progress_seen,
            seen.result.is_ok(),
            seen.result.as_ref().err(),
            seen.sections,
            seen.undo_available,
        );
    }
    for (case, seen) in observed {
        assert!(
            seen.dispatch_held_thread < parse_gate::HOLD_LIMIT / 2,
            "{}: dispatching the import must not hold the main thread for the import \
             (held {:?})",
            case.command,
            seen.dispatch_held_thread
        );
        assert_eq!(
            seen.progress_phase, "import_parsing",
            "{}: the first progress event is the read phase",
            case.command
        );
        assert!(
            seen.running_when_progress_seen,
            "{}: the read-phase progress reaches the window while the import runs",
            case.command
        );
        // [#576 C4] The webview receives the rejection's code, and display text
        // with no marker in it.
        assert!(
            error_code(&seen.result) == Some("IMPORT_CANCELLED")
                && error_message(&seen.result)
                    .is_some_and(|e| e.contains("was cancelled") && !e.contains("IMPORT_CANCELLED")),
            "{}: the Cancel reached the running import: {:?}",
            case.command,
            seen.result.as_ref().err()
        );
        assert!(
            seen.result.is_err(),
            "{}: a cancelled import fails",
            case.command
        );
        assert_eq!(seen.sections, 0, "{}: nothing was imported", case.command);
        assert!(
            !seen.undo_available,
            "{}: nothing was recorded",
            case.command
        );
    }
}

/// A Cancel that reaches the backend BEFORE its import registered still stops that
/// import (Codex review on #630). The dialog shows its Cancel as soon as the import
/// is issued, but the import's IPC leaves only after the frontend's edit queue
/// (`runSerialized`) and the plan-anchor flush, and its body then waits for the
/// blocking pool. So `cancel_loading` can run while no load of that name is in
/// progress: it did nothing then, and the import ran to its end and landed.
#[test]
fn a_cancel_that_reaches_the_backend_before_its_import_still_stops_it() {
    let dir = TempDir::new().expect("temp dir");
    let observed: Vec<(&'static str, Result<Value, Value>, usize, bool)> = cases(&dir)
        .into_iter()
        .map(|case| {
            let (app, window) = app_with_import_commands();
            // The Cancel is dispatched, and answered, before the import is dispatched.
            dispatch(
                &window,
                request("cancel_loading", json!({ "loadToken": case.command })),
            )
            .recv_timeout(Duration::from_secs(5))
            .expect("cancel_loading answers");
            let answer = dispatch(&window, request(case.command, body_for(&case, &app)))
                .recv_timeout(Duration::from_secs(20))
                .expect("the import answers");
            let state = app.state::<AppState>();
            let sections = state.get_project_state().expect("project").sections.len();
            (case.command, answer_of(answer), sections, state.can_undo())
        })
        .collect();
    for (command, result, sections, undo_available) in &observed {
        println!(
            "{command}: Cancel before the import: result success={} code={:?}; sections in \
             the project={sections}; undo available={undo_available}",
            result.is_ok(),
            error_code(result),
        );
    }
    for (command, result, sections, undo_available) in observed {
        assert_eq!(
            error_code(&result),
            Some("IMPORT_CANCELLED"),
            "{command}: the early Cancel stopped the import: {result:?}"
        );
        assert_eq!(sections, 0, "{command}: nothing was imported");
        assert!(!undo_available, "{command}: nothing was recorded");
    }
}

/// The same wiring with nobody pressing Cancel: the import lands, its answer
/// arrives through the handler, and the dispatching thread was free throughout.
#[test]
fn an_import_dispatched_off_the_main_thread_still_lands() {
    let dir = TempDir::new().expect("temp dir");
    for case in cases(&dir) {
        let (app, window) = app_with_import_commands();
        let started = Instant::now();
        let import = dispatch(&window, request(case.command, body_for(&case, &app)));
        let held = started.elapsed();
        let answer = import
            .recv_timeout(Duration::from_secs(20))
            .expect("the import answers");
        let result = answer_of(answer);
        assert!(result.is_ok(), "{}: {:?}", case.command, result.err());
        let sections = app
            .state::<AppState>()
            .get_project_state()
            .expect("project")
            .sections
            .len();
        assert!(sections >= 1, "{}: the import landed", case.command);
        assert!(
            held < Duration::from_secs(1),
            "{}: dispatch held the thread {held:?}",
            case.command
        );
    }
}
