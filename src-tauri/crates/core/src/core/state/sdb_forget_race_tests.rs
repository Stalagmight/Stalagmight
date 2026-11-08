//! [#576 W88] The 401 auto-drop (`speleodb_forget`) and an in-flight Start editing.
//!
//! A dead token met on any SpeleoDB call - a background project list, say - runs
//! `speleodb_forget`, which shuts the edit gate and clears the session. It took no
//! lock-ops guard and bumped no session generation, unlike logout, so it could land
//! INSIDE a Start editing: between that flow's account read and its commit. The
//! commit then opened the edit gate for a session that was gone - editing a remote
//! project nobody is signed in to push. Now the auto-drop runs in the lock-ops span
//! and bumps the generation, as logout does: a Start editing in flight finishes
//! first and the drop then shuts its gate, and a 401 met INSIDE a span (Start
//! editing's own acquire, a release verb) drops the session in that span without
//! waiting on itself.
//!
//! Driven against loopback stubs. All data is synthetic, built in code.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use super::speleodb::{
    authed_session_at, link_open_copy_to, remote_readonly_fixture, routing_stub,
    FREE_MUTEX_AT_BASE_BODY, REMOTE_ID,
};
use super::AppState;

/// Long enough for a request that is not blocked to finish many times over.
const SETTLE: Duration = Duration::from_millis(400);
/// What a blocked flow may take before the test reads it as a deadlock.
const DEADLINE: Duration = Duration::from_secs(20);

/// A loopback stub answering every request with `body`, except that it holds the
/// FIRST revision-history read until the test lets it go: it reports that read on
/// the returned receiver, then waits on the returned sender's channel. Start
/// editing's UpToDate arm reads the history after the acquire and commits right
/// after it, so a test holding that read holds the flow between its account read
/// and its commit. Keep the listener bound for the whole test.
fn stub_holding_the_history_read(
    body: &'static str,
) -> (String, TcpListener, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let accept_on = listener.try_clone().expect("clone listener");
    let (arrived_tx, arrived_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let held_once = AtomicBool::new(false);
        while let Ok((mut stream, _)) = accept_on.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
            let mut req = Vec::new();
            let mut chunk = vec![0u8; 8 * 1024];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&chunk[..n]);
            }
            let line = String::from_utf8_lossy(&req)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            if line.starts_with("GET ")
                && line.contains("/revisions/")
                && !held_once.swap(true, Ordering::SeqCst)
            {
                let _ = arrived_tx.send(());
                let _ = release_rx.recv_timeout(DEADLINE);
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    (
        format!("http://127.0.0.1:{port}"),
        listener,
        arrived_rx,
        release_tx,
    )
}

/// Whether the open state is signed in.
fn signed_in(state: &AppState) -> bool {
    state
        .speleodb_session_status()
        .is_ok_and(|status| status.logged_in)
}

#[test]
fn a_401_drop_inside_start_editing_never_leaves_the_gate_open_for_a_signed_out_session() {
    let (base, _listener, arrived, release) =
        stub_holding_the_history_read(FREE_MUTEX_AT_BASE_BODY);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);
    link_open_copy_to(&state, &base);
    let generation_before = state.speleodb_session_generation();
    let config = crate::test_helpers::ConfigHandoff::capture();

    let (started, forget_landed_inside) = std::thread::scope(|scope| {
        let editing = scope.spawn(|| {
            state
                .speleodb_start_editing()
                .map(|r| format!("{r:?}"))
                .map_err(|e| e.message)
        });
        arrived
            .recv_timeout(DEADLINE)
            .expect("Start editing reads the history after its acquire");
        // Start editing is now between its account read and its commit. The 401
        // auto-drop of some other call lands here.
        let (forgot_tx, forgot_rx) = mpsc::channel();
        let config = &config;
        let state = &state;
        scope.spawn(move || {
            let _config = config.adopt();
            state.speleodb_forget();
            let _ = forgot_tx.send(());
        });
        let forget_landed_inside = forgot_rx.recv_timeout(SETTLE).is_ok();
        let _ = release.send(());
        let started = editing.join().expect("the Start editing thread");
        if !forget_landed_inside {
            forgot_rx
                .recv_timeout(DEADLINE)
                .expect("the auto-drop completes once Start editing is done");
        }
        (started, forget_landed_inside)
    });
    let gate = (state.edit_enabled(), state.force_editing());
    let signed = signed_in(&state);
    let generation_after = state.speleodb_session_generation();

    println!(
        "[W88 item 2] 401 auto-drop while Start editing waits on its history read: the \
         drop finished inside the flow: {forget_landed_inside}; start -> {started:?}; \
         afterwards (gate, force) {gate:?}, signed in {signed}, generation \
         {generation_before} -> {generation_after}"
    );
    assert!(
        !gate.0 || signed,
        "the edit gate is open for a session the 401 auto-drop cleared"
    );
    assert!(
        !forget_landed_inside,
        "the auto-drop waits for an in-flight Start editing, as logout does"
    );
    assert!(!signed, "the auto-drop still happens, after the flow");
    assert_eq!(
        gate,
        (false, false),
        "and it shuts the gate the flow opened"
    );
    assert_ne!(
        generation_after, generation_before,
        "the auto-drop is a session boundary"
    );
}

/// Run `f` on a thread of its own, or `None` if it has not finished by [`DEADLINE`]
/// (a deadlock: the thread is left behind).
fn within_deadline<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    let config = crate::test_helpers::ConfigHandoff::capture();
    std::thread::spawn(move || {
        let _config = config.adopt();
        let _ = tx.send(f());
    });
    rx.recv_timeout(DEADLINE).ok()
}

/// The lock answers 401: a dead token met inside the lock-ops span.
fn lock_verbs_unauthorized(line: &str) -> (u16, &'static str) {
    if line.starts_with("POST ") {
        (401, r#"{"detail":"Invalid token."}"#)
    } else {
        (200, FREE_MUTEX_AT_BASE_BODY)
    }
}

#[test]
fn a_401_met_inside_the_lock_ops_span_drops_the_session_without_waiting_on_itself() {
    let (base, _listener, seen) = routing_stub(lock_verbs_unauthorized);

    // Start editing: its acquire answers 401, inside the span Start editing holds.
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);
    link_open_copy_to(&state, &base);
    let state = Arc::new(state);
    let editing = {
        let state = Arc::clone(&state);
        within_deadline(move || {
            let answer = state
                .speleodb_start_editing()
                .map(|r| format!("{r:?}"))
                .map_err(|e| format!("{:?}", e.action));
            (answer, state.edit_enabled(), signed_in(&state))
        })
    };

    // Release lock (the held-lock prompt's per-project release): 401 inside its span.
    let (_released_temp, released) = remote_readonly_fixture();
    *released.speleodb.lock() = authed_session_at(&base);
    let released = Arc::new(released);
    let release = {
        let released = Arc::clone(&released);
        within_deadline(move || {
            let answer = released
                .speleodb_release_lock(REMOTE_ID.to_owned())
                .map_err(|e| format!("{:?}", e.action));
            (answer, signed_in(&released))
        })
    };
    let requests = seen.lock().map(|lines| lines.clone()).unwrap_or_default();

    println!(
        "[W88 item 2] a 401 inside the span: Start editing -> (answer, gate, signed in) \
         {editing:?}; Release lock -> (answer, signed in) {release:?}; requests {requests:?}"
    );
    let (answer, gate, signed) = editing.expect("Start editing finishes: no self-deadlock");
    assert!(answer.is_err(), "the acquire's 401 fails Start editing");
    assert!(!gate && !signed, "signed out, the gate shut");
    let (answer, signed) = release.expect("Release lock finishes: no self-deadlock");
    assert!(answer.is_err(), "the release's 401 fails Release lock");
    assert!(!signed, "signed out");
}
