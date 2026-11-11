//! Synchronous commands never wait on SpeleoDB network I/O.
//!
//! Tauri runs synchronous commands on the main thread, so anything they wait on
//! freezes the whole window. These tests pin the three ways that used to happen:
//!
//! - config writes (panel sizes, column widths, the Settings save) queued behind the
//!   `speleodb_lock_ops` span, which uploads and downloads hold for minutes;
//! - Switch to Read Only Mode held `command_lock` and `project` across its release
//!   POST, so every edit, undo, save and `get_project_state` waited for it;
//! - the #202 lock re-verify ran its GET inline in `execute()`, under `command_lock`.
//!
//! Each network test points the session at a loopback server that holds its reply
//! until the test lets it go. While the HTTP call is provably in flight (the server
//! has the request), the test checks that the synchronous side completes and that
//! the guards it needs are free.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::tests_support::sdk_fixture;
use super::AppState;
use crate::config::Config;
use crate::core::command::{EDIT_DISABLED_CODE, EDIT_REVERIFY_DIVERGED_CODE};
use crate::core::Command;
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;
use crate::speleodb::auth::TokenStore;
use crate::speleodb::SpeleoReleaseContext;
use crate::test_helpers::{with_isolated_config, ConfigHandoff};

const ID: &str = "11111111-1111-1111-1111-111111111111";
const ME: &str = "diver@example.com";
/// Generous for a local, uncontended write; far below any SpeleoDB timeout.
const PROMPT: Duration = Duration::from_secs(2);
/// How long a call that SHOULD be waiting is given to (wrongly) finish.
const STILL_WAITING: Duration = Duration::from_millis(300);

/// One scripted reply: HTTP status, JSON body, and whether to hold it until the
/// test releases the server.
struct Reply {
    status: u16,
    body: String,
    hold: bool,
}

fn reply(status: u16, body: &str, hold: bool) -> Reply {
    Reply {
        status,
        body: sdk_fixture(body),
        hold,
    }
}

/// A project body whose write lock is held by `holder`.
fn project_held_by(holder: &str) -> String {
    format!(
        r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{{"user":"{holder}"}}}}"#
    )
}

/// A loopback server answering one request per connection, in order, from a
/// script. A held reply reports its request line on `arrived` and waits for
/// `release` before it is sent, so the test knows the client is inside that call.
struct ScriptedServer {
    url: String,
    arrived: mpsc::Receiver<String>,
    release: mpsc::Sender<()>,
    handle: std::thread::JoinHandle<()>,
}

impl ScriptedServer {
    fn start(script: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let (arrived_tx, arrived) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            for step in script {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let request = read_request(&mut stream);
                if step.hold {
                    let _ = arrived_tx.send(request);
                    if release_rx.recv_timeout(Duration::from_secs(30)).is_err() {
                        return;
                    }
                }
                let reason = if step.status == 200 { "OK" } else { "Error" };
                let response = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    step.status,
                    step.body.len(),
                    step.body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            arrived,
            release,
            handle,
        }
    }

    /// Block until a held request is in, returning its request line.
    fn wait_for_held_request(&self) -> String {
        self.arrived
            .recv_timeout(Duration::from_secs(10))
            .expect("the client reached the held request")
    }

    fn let_it_answer(&self) {
        self.release.send(()).expect("server still waiting");
    }

    fn finish(self) {
        self.handle.join().expect("server thread");
    }
}

/// Read one HTTP request (headers plus a `Content-Length` body) and return its
/// request line.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut data = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..end]).into_owned();
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while data.len() < end + 4 + length {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => data.extend_from_slice(&chunk[..n]),
                }
            }
            return head.lines().next().unwrap_or_default().to_owned();
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return String::new(),
            Ok(n) => data.extend_from_slice(&chunk[..n]),
        }
    }
}

/// An open, clean, linked project at `endpoint`, signed in as [`ME`] there, with
/// the edit gate open and this process's lock claim recorded: an edit session.
/// The config names the same instance, so a Settings save can change it.
fn editing_session(dir: &Path, endpoint: &str) -> AppState {
    let path = dir.join("remote.stalx");
    let mut db = Database::open(&path).expect("open db");
    db.save_project(&Project::new("Remote".to_owned(), None))
        .expect("save project");
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: ID.to_owned(),
        project_name: "Remote".to_owned(),
        base_commit_sha: Some("abc123".to_owned()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }))
    .expect("save link");
    drop(db);

    let state = AppState::new().expect("state");
    {
        let mut config = state.config.lock().expect("config");
        config.speleodb.endpoint = Some(endpoint.to_owned());
        config.speleodb.email = Some(ME.to_owned());
        config.speleodb.token.0 = Some("tok-1".to_owned());
        config.save().expect("save config");
    }
    assert!(state.open_project(path).success, "open the linked project");
    {
        let mut session = state.speleodb.lock().expect("session");
        session.endpoint = Some(endpoint.to_owned());
        session.email = Some(ME.to_owned());
        session.token_store = TokenStore::from_in_memory_token("tok-1");
        session.rehydrate_attempted = true;
    }
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(ID));
    state
}

/// The consent payload Switch to Read Only Mode is issued with.
fn release_context(state: &AppState, endpoint: &str) -> SpeleoReleaseContext {
    let modified_at = state
        .project
        .lock()
        .expect("project")
        .as_ref()
        .expect("open project")
        .modified_at
        .to_rfc3339();
    SpeleoReleaseContext {
        project_modified_at: modified_at,
        session_generation: state.speleodb_session_generation().to_string(),
        project_id: ID.to_owned(),
        endpoint: endpoint.to_owned(),
        user_email: ME.to_owned(),
    }
}

/// A small edit through the `execute()` chokepoint.
fn an_edit(name: &str) -> Command {
    Command::CreateSection {
        name: Some(name.to_owned()),
        surveyor: None,
        date: None,
        color: "#336699".to_owned(),
        direction: None,
        shots: Vec::new(),
    }
}

/// Every guard a synchronous command could need is free right now.
fn assert_no_guard_is_held(state: &AppState, during: &str) {
    assert!(state.project.try_lock().is_ok(), "`project` held {during}");
    assert!(
        state.command_lock.try_lock().is_ok(),
        "`command_lock` held {during}"
    );
    assert!(
        state.database.try_lock().is_ok(),
        "`database` held {during}"
    );
    assert!(
        state.undo_stack.try_lock().is_ok(),
        "`undo_stack` held {during}"
    );
    assert!(state.config.try_lock().is_ok(), "`config` held {during}");
    assert!(
        state.config_write_lock.try_lock().is_ok(),
        "`config_write_lock` held {during}"
    );
    assert!(state.speleodb.try_lock().is_ok(), "session held {during}");
}

// --- config writes and the lock-ops span ------------------------------------------

#[test]
fn config_writes_complete_while_a_lock_operation_holds_the_span() {
    // An upload or download holds lock-ops for its whole span (minutes). A panel
    // resize used to queue behind it on the main thread.
    with_isolated_config(|| {
        let state = AppState::new().expect("state");
        let config = ConfigHandoff::capture();
        let lock_ops = state.speleodb_lock_ops_guard();
        let (done, finished) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _config = config.adopt();
                let started = Instant::now();
                state
                    .modify_config(|c| {
                        c.panels.sections_panel_width = 333;
                        Ok(())
                    })
                    .expect("partial write");
                let mut whole = state.get_config();
                whole.general.undo_history_limit = 77;
                state
                    .update_config(whole)
                    .expect("whole write, same instance");
                done.send(started.elapsed()).expect("report");
            });
            let outcome = finished.recv_timeout(PROMPT);
            // Let a writer that (wrongly) queued go before asserting, so a
            // regression fails here instead of deadlocking the scope's join.
            drop(lock_ops);
            let took = outcome.expect("config writes must not wait for the lock-ops span");
            assert!(took < PROMPT, "took {took:?}");
        });
        let saved = Config::load().expect("load");
        assert_eq!(saved.panels.sections_panel_width, 333);
        assert_eq!(saved.general.undo_history_limit, 77);
    });
}

#[test]
fn an_instance_change_still_waits_for_the_lock_operation() {
    // The narrowing keeps the one case that needs the span: a new instance ends the
    // session, which must not land inside another lock operation.
    with_isolated_config(|| {
        let state = AppState::new().expect("state");
        let config = ConfigHandoff::capture();
        let lock_ops = state.speleodb_lock_ops_guard();
        let (done, finished) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _config = config.adopt();
                let mut moved = state.get_config();
                moved.speleodb.endpoint = Some("https://other.example.com".to_owned());
                state.update_config(moved).expect("instance change");
                done.send(()).expect("report");
            });
            assert!(
                finished.recv_timeout(STILL_WAITING).is_err(),
                "an instance change must wait for the lock operation in flight"
            );
            drop(lock_ops);
            finished
                .recv_timeout(Duration::from_secs(5))
                .expect("and completes once the span ends");
        });
        assert_eq!(
            state.get_config().speleodb.endpoint.as_deref(),
            Some("https://other.example.com")
        );
    });
}

#[test]
fn a_partial_write_keeps_the_live_speleodb_section() {
    // A sign-in writes `config` without `config_write_lock`. One landing between a
    // partial write's clone and its save used to read as an instance change: the
    // panel write cleared the new token and ended the session.
    with_isolated_config(|| {
        let state = AppState::new().expect("state");
        state
            .modify_config(|c| {
                {
                    let mut live = state.config.lock().expect("config");
                    live.speleodb.endpoint = Some("https://signed-in.example.com".to_owned());
                    live.speleodb.email = Some(ME.to_owned());
                    live.speleodb.token.0 = Some("fresh".to_owned());
                }
                c.panels.detail_panel_width = 444;
                Ok(())
            })
            .expect("partial write");
        let live = state.get_config();
        assert_eq!(
            live.speleodb.endpoint.as_deref(),
            Some("https://signed-in.example.com")
        );
        assert_eq!(live.speleodb.token.0.as_deref(), Some("fresh"));
        assert_eq!(live.panels.detail_panel_width, 444);
    });
}

#[test]
fn config_writes_do_not_wait_behind_an_instance_change_release() {
    // Saving a new instance while editing hands the old project's lock back. That
    // POST used to run under `config_write_lock`, so the next panel resize waited
    // for it on the main thread. `update_config` now refuses an instance change
    // while connected (Disconnect first), so this drives the write behind it, as a
    // sign-in landing after that check would reach it.
    let dir = tempfile::tempdir().expect("temp dir");
    with_isolated_config(|| {
        // The holder check answers first (lock#4), then the release POST is held.
        let server = ScriptedServer::start(vec![
            reply(200, &project_held_by(ME), false),
            reply(200, &project_held_by(ME), true),
        ]);
        let state = editing_session(dir.path(), &server.url);
        let config = ConfigHandoff::capture();
        let (done, finished) = mpsc::channel();
        std::thread::scope(|scope| {
            let saver = scope.spawn(|| {
                let _config = config.adopt();
                let mut moved = state.get_config();
                moved.speleodb.endpoint = Some("https://other.example.com".to_owned());
                state.write_whole_config(moved)
            });
            let request = server.wait_for_held_request();
            assert!(
                request.starts_with("POST "),
                "the release is in flight: {request}"
            );
            assert_no_guard_is_held(&state, "during the instance-change release POST");
            assert!(!state.edit_enabled(), "the gate is shut before the POST");

            scope.spawn(|| {
                let _config = config.adopt();
                state
                    .modify_config(|c| {
                        c.panels.data_table_height = 222;
                        Ok(())
                    })
                    .expect("partial write");
                done.send(()).expect("report");
            });
            finished
                .recv_timeout(PROMPT)
                .expect("a panel write completes while the release POST is in flight");

            server.let_it_answer();
            saver
                .join()
                .expect("saver thread")
                .expect("the instance change succeeds");
        });
        server.finish();
        assert!(!state.speleodb_session_status().expect("status").logged_in);
        assert!(!state.speleodb_lock_acquired_here(ID), "the claim ended");
        let saved = Config::load().expect("load");
        assert_eq!(saved.panels.data_table_height, 222);
        assert_eq!(
            saved.speleodb.endpoint.as_deref(),
            Some("https://other.example.com")
        );
    });
}

// --- Switch to Read Only Mode -------------------------------------------------------

#[test]
fn switching_to_read_only_holds_no_guard_across_the_release_post() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_isolated_config(|| {
        let server = ScriptedServer::start(vec![
            reply(200, &project_held_by(ME), false),
            reply(200, &format!(r#"{{"id":"{ID}","name":"Remote"}}"#), true),
        ]);
        let state = editing_session(dir.path(), &server.url);
        let expected = release_context(&state, &server.url);
        std::thread::scope(|scope| {
            let switch = scope.spawn(|| {
                state
                    .speleodb_release_current_lock(Some(expected))
                    .map_err(|e| e.message)
            });
            let request = server.wait_for_held_request();
            assert!(
                request.starts_with("POST "),
                "the release is in flight: {request}"
            );
            assert_no_guard_is_held(&state, "during the read-only release POST");

            let started = Instant::now();
            assert!(state.get_project_state().is_some(), "project state reads");
            let edit = state.execute(an_edit("during the release"));
            assert!(started.elapsed() < PROMPT, "took {:?}", started.elapsed());
            assert!(
                edit.error
                    .as_deref()
                    .is_some_and(|e| e.starts_with(EDIT_DISABLED_CODE)),
                "an edit during the release is refused as read-only, got {edit:?}"
            );

            server.let_it_answer();
            switch
                .join()
                .expect("switch thread")
                .expect("the switch succeeds");
        });
        server.finish();
        assert!(!state.edit_enabled(), "read-only afterwards");
        assert!(!state.speleodb_lock_acquired_here(ID), "the claim ended");
    });
}

#[test]
fn a_failed_read_only_release_gives_the_session_its_gate_back() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_isolated_config(|| {
        let server = ScriptedServer::start(vec![
            reply(200, &project_held_by(ME), false),
            reply(500, r#"{"detail":"synthetic failure"}"#, true),
        ]);
        let state = editing_session(dir.path(), &server.url);
        let expected = release_context(&state, &server.url);
        std::thread::scope(|scope| {
            let switch = scope.spawn(|| {
                state
                    .speleodb_release_current_lock(Some(expected))
                    .map_err(|e| e.message)
            });
            let _ = server.wait_for_held_request();
            assert!(!state.edit_enabled(), "shut while the POST is in flight");
            // (A status refresh landing now used to drop the open project's claim
            // on seeing the shut gate; the refresh no longer touches claims.)
            server.let_it_answer();
            assert!(
                switch.join().expect("switch thread").is_err(),
                "a failed release reports the failure"
            );
        });
        server.finish();
        assert!(
            state.edit_enabled(),
            "the lock is still ours, so the session keeps editing as before the call"
        );
        assert!(
            state.speleodb_lock_acquired_here(ID),
            "and keeps its claim, even though a refresh ran during the POST"
        );
    });
}

#[test]
fn a_failed_read_only_release_after_the_session_ended_keeps_the_claim_for_the_janitor() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_isolated_config(|| {
        let server = ScriptedServer::start(vec![
            reply(200, &project_held_by(ME), false),
            reply(500, r#"{"detail":"synthetic failure"}"#, true),
        ]);
        let state = editing_session(dir.path(), &server.url);
        let expected = release_context(&state, &server.url);
        std::thread::scope(|scope| {
            let switch = scope.spawn(|| {
                state
                    .speleodb_release_current_lock(Some(expected))
                    .map_err(|e| e.message)
            });
            let _ = server.wait_for_held_request();
            // A close or switch during the POST: a session boundary. With the
            // gate shut and the claim off, it has nothing of its own to release.
            {
                let _project = state.project.lock().expect("project");
                state.bump_speleodb_session();
            }
            server.let_it_answer();
            assert!(switch.join().expect("switch thread").is_err());
        });
        server.finish();
        assert!(
            !state.edit_enabled(),
            "the boundary owns the gate now; a failed release must not reopen it"
        );
        assert!(
            state.speleodb_lock_acquired_here(ID),
            "the mutex is still held on the server, so its claim survives for a retry"
        );
    });
}

// --- the #202 lock re-verify --------------------------------------------------------

/// Wait until the background re-verify has its answer.
fn wait_for_reverify(state: &AppState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let settled = state
            .speleodb_reverify
            .lock()
            .expect("reverify slot")
            .as_ref()
            .is_some_and(|pending| pending.probe.is_finished());
        if settled {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the background re-verify never finished");
}

#[test]
fn a_due_lock_reverify_never_waits_on_the_network() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_isolated_config(|| {
        let server = ScriptedServer::start(vec![reply(
            200,
            &project_held_by("other@example.com"),
            true,
        )]);
        let state = editing_session(dir.path(), &server.url);

        let started = Instant::now();
        let first = state.execute(an_edit("triggers the re-verify"));
        assert!(first.success, "the triggering edit goes ahead: {first:?}");
        let request = server.wait_for_held_request();
        assert!(
            request.starts_with("GET "),
            "the probe is in flight: {request}"
        );
        assert!(
            started.elapsed() < PROMPT,
            "the edit returned while the probe was still waiting: {:?}",
            started.elapsed()
        );
        assert_no_guard_is_held(&state, "during the re-verify GET");
        let second = state.execute(an_edit("while the probe is out"));
        assert!(
            second.success,
            "edits keep working while it runs: {second:?}"
        );

        server.let_it_answer();
        wait_for_reverify(&state);
        server.finish();

        let refused = state.execute(an_edit("after the lock was lost"));
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with(EDIT_REVERIFY_DIVERGED_CODE)),
            "the next edit routes to the divergence prompt, got {refused:?}"
        );
        assert!(!state.edit_enabled(), "and the gate is shut");
        let after = state.execute(an_edit("read-only now"));
        assert!(
            after
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with(EDIT_DISABLED_CODE)),
            "later edits are plain read-only refusals, got {after:?}"
        );
    });
}

#[test]
fn a_reverify_that_confirms_the_lock_changes_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_isolated_config(|| {
        let server = ScriptedServer::start(vec![reply(200, &project_held_by(ME), false)]);
        let state = editing_session(dir.path(), &server.url);
        assert!(state.execute(an_edit("triggers the re-verify")).success);
        wait_for_reverify(&state);
        server.finish();
        let next = state.execute(an_edit("still ours"));
        assert!(next.success, "a confirmed lock keeps editing: {next:?}");
        assert!(state.edit_enabled());
    });
}

#[test]
fn a_reverify_from_an_ended_session_is_ignored() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_isolated_config(|| {
        let server = ScriptedServer::start(vec![reply(
            200,
            &project_held_by("other@example.com"),
            false,
        )]);
        let state = editing_session(dir.path(), &server.url);
        assert!(state.execute(an_edit("triggers the re-verify")).success);
        wait_for_reverify(&state);
        server.finish();
        // A session boundary after the probe started that keeps editing: an
        // in-place reload carrying its edit session into the new generation, the
        // way a fast-forward does. (Any other boundary ends the session and shuts
        // the gate outright - `speleodb_edit_gate`.)
        {
            let _project = state.project.lock().expect("project");
            let link = state.speleodb_open_link().expect("link");
            let from = state.speleodb_session_generation();
            let mut session = state.speleodb.lock().expect("session");
            state.bump_speleodb_session();
            state.speleodb_commit_open_gate(&mut session, Some(&link), Some(from));
        }
        assert!(state.edit_enabled(), "the reload carried the session");
        let next = state.execute(an_edit("new session"));
        assert!(
            next.success,
            "a verdict about an ended session must not refuse this one: {next:?}"
        );
        assert!(state.edit_enabled());
    });
}

#[test]
fn a_reverify_verdict_does_not_outlive_the_gate_it_was_taken_under() {
    // The lock is found lost, the status refresh shuts the gate, and Start Editing
    // re-acquires - all within one session generation. The probe's verdict is about
    // the lock that was lost, not the one just taken.
    let dir = tempfile::tempdir().expect("temp dir");
    with_isolated_config(|| {
        let server = ScriptedServer::start(vec![reply(
            200,
            &project_held_by("other@example.com"),
            false,
        )]);
        let state = editing_session(dir.path(), &server.url);
        assert!(state.execute(an_edit("triggers the re-verify")).success);
        wait_for_reverify(&state);
        server.finish();
        let generation = state.speleodb_session_generation();
        state.set_edit_enabled(false);
        state.set_edit_enabled(true);
        assert_eq!(state.speleodb_session_generation(), generation);
        let next = state.execute(an_edit("under the fresh lock"));
        assert!(
            next.success,
            "a fresh lock must not be refused by the old verdict: {next:?}"
        );
        assert!(state.edit_enabled());
    });
}
