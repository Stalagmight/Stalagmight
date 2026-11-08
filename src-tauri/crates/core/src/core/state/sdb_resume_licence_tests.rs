//! [#576 RV4-R-S3] The held-lock prompt's "Continue editing" claim licenses the
//! Done release, NOT the B32 authorship shortcut.
//!
//! The prompt lists every lock the account holds at sign-in. Shown on a second
//! computer while the first is still editing, "Continue editing" there resumes
//! that lock; with a full claim, the poll and the push of that session could
//! explain the first session's commits away as its own unrecorded push - the
//! exact thing B32 exists to stop. The decision (RV2-R-13-RESTART) only asked
//! for Done to release the resumed lock.
//!
//! Driven against a loopback stub whose answer can be switched mid-test (the
//! server's history moves on while this session is editing); every request line
//! is recorded. All data is synthetic.

use std::io::{Read as _, Write as _};
use std::sync::{Arc, Mutex};

use super::speleodb::{authed_session_at, link_open_copy_to, remote_readonly_fixture, REMOTE_ID};
use crate::speleodb::{DoneResult, RemoteState, StartEditingResult};

/// Our own account holds the lock and the server tip IS our BASE.
const AT_BASE: &str = r#"{
    "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
    "active_mutex":{"user":"u@x"},
    "commits":[{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

/// The same, after ANOTHER session on our account pushed def456 on top of BASE.
const GAP_BY_OUR_ACCOUNT: &str = r#"{
    "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
    "active_mutex":{"user":"u@x"},
    "commits":[{"id":"def456","parent_ids":["abc123"],"author_email":"u@x"},
               {"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

type Shared<T> = Arc<Mutex<T>>;

/// A loopback stub answering every request with the CURRENT body and recording
/// each request line. Keep the listener bound for the whole test.
fn switchable_stub(
    body: &'static str,
) -> (
    String,
    std::net::TcpListener,
    Shared<&'static str>,
    Shared<Vec<String>>,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let current = Arc::new(Mutex::new(body));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let accept_on = listener.try_clone().unwrap();
    let (answer, log) = (Arc::clone(&current), Arc::clone(&seen));
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = accept_on.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
            let mut req = Vec::new();
            let mut chunk = vec![0u8; 8 * 1024];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                req.extend_from_slice(chunk.get(..n).unwrap_or_default());
            }
            let text = String::from_utf8_lossy(&req).into_owned();
            if let Ok(mut lines) = log.lock() {
                lines.push(text.lines().next().unwrap_or_default().to_owned());
            }
            let body = *answer.lock().unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), listener, current, seen)
}

#[test]
fn a_resumed_lock_does_not_explain_another_sessions_commit_away() {
    let (base, _listener, body, seen) = switchable_stub(AT_BASE);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);
    link_open_copy_to(&state, &base);

    // "Continue editing" from the held-lock prompt, over our account's lock.
    let started = state.speleodb_resume_held_lock_editing().unwrap();
    assert!(matches!(started, StartEditingResult::Editing));
    assert!(
        state.speleodb_lock_acquired_here(REMOTE_ID),
        "the resumed lock is claimed, so Done may release it"
    );
    // Local work, then the other session (same account) pushes on top of BASE.
    state
        .speleodb_mutate_link(|l| {
            l.has_unpushed_edits = true;
            l.last_edit_at = Some("2026-07-27T10:00:00Z".to_owned());
        })
        .unwrap();
    *body.lock().unwrap() = GAP_BY_OUR_ACCOUNT;

    let polled = state.speleodb_link_status().map(|s| s.remote_state).ok();
    let done = state.speleodb_done(None);
    let link = state.speleodb_open_link().unwrap();
    let uploads = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("/upload/"))
        .count();
    println!(
        "resumed lock, gap commit def456 by u@x: poll remote_state {polled:?}; Done -> \
         {done:?}; BASE after Done {:?}; uploads {uploads}",
        link.base_commit_sha
    );
    assert_eq!(
        polled,
        Some(RemoteState::Conflict),
        "the poll must not read another session's commit as our own landed push"
    );
    assert!(
        matches!(done, Ok(DoneResult::DivergenceNeeded)),
        "Done must raise the divergence, got {done:?}"
    );
    assert_eq!(
        link.base_commit_sha.as_deref(),
        Some("abc123"),
        "BASE must not adopt the other session's commit"
    );
    assert_eq!(
        uploads, 0,
        "nothing uploaded over the other session's revision"
    );
}
