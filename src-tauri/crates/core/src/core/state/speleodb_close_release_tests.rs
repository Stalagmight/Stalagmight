//! A clean close, project switch or quit whose lock release fails or is uncertain
//! never closes silently (PR #668's "never strand a lock silently", on the
//! holder-checked release model). The release reports what it came to on the reply
//! of the call that released (`CommandResult::lock_release`); one that failed or is
//! uncertain closes nothing, keeps the claim unbound (editing paused) and leaves the
//! user to choose: Retry (the same call again), Close and keep the lock (the claim
//! marked kept, for the next sign-in's held-lock prompt), or Cancel.
//!
//! Ported from #668's `confirmed_close_releases_or_keeps_the_project_open_for_retry`
//! (the 503, the no-op release and the lost-response cases),
//! `confirmed_close_without_credentials_keeps_the_project_open` and
//! `pending_close_release_poisoned_session_refuses_teardown`, with their
//! expectations moved to the dialog model: the project stays open until the user
//! chooses. Synthetic HTTP throughout.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::tests_support::sdk_fixture;
use super::AppState;
use crate::core::LockReleaseOutcome;
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;
use crate::speleodb::auth::TokenStore;

const ID: &str = "77777777-7777-7777-7777-777777777777";
const ME: &str = "u@x";
const SUCCESSOR: &str = "other@x";

/// What the stub does with one request.
enum Reply {
    /// Answer with this status and (already SDK-shaped) body.
    Json(u16, String),
    /// Read the request, then hang up without answering: a release that
    /// committed on the server and whose reply was lost on the way back.
    Drop,
}

/// The project as SpeleoDB reports it, its lock held by `holder` (or nobody).
fn project(holder: Option<&str>) -> Reply {
    let mutex = holder.map_or_else(String::new, |user| {
        format!(r#","active_mutex":{{"user":"{user}"}}"#)
    });
    Reply::Json(
        200,
        sdk_fixture(&format!(
            r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE"{mutex}}}"#
        )),
    )
}

fn unavailable() -> Reply {
    Reply::Json(503, r#"{"detail":"Synthetic release failure"}"#.to_owned())
}

/// Every request line the stub saw, and whether the edit gate was open when each
/// release POST arrived.
#[derive(Default)]
struct Seen {
    lines: Vec<String>,
    gate_open_at_release: Vec<bool>,
}

impl Seen {
    fn releases(&self) -> usize {
        self.lines.iter().filter(|line| is_release(line)).count()
    }
}

fn is_release(line: &str) -> bool {
    line.starts_with("POST") && line.contains("/release")
}

/// The app state a stub may look at while a release is in flight.
type Observed = Arc<Mutex<Option<Arc<AppState>>>>;

/// A loopback SpeleoDB scripted per request: `reply(line, releases_so_far)`.
fn scripted(
    reply: impl Fn(&str, usize) -> Reply + Send + 'static,
) -> (String, Arc<Mutex<Seen>>, Observed) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let observed: Observed = Arc::default();
    let (log, watch) = (Arc::clone(&seen), Arc::clone(&observed));
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
            }
            let line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let releases_before = {
                let mut seen = log.lock().expect("log");
                if is_release(&line) {
                    let gate = watch
                        .lock()
                        .expect("observed")
                        .as_ref()
                        .is_some_and(|state| state.edit_enabled());
                    seen.gate_open_at_release.push(gate);
                }
                let before = seen.releases();
                seen.lines.push(line.clone());
                before
            };
            match reply(&line, releases_before) {
                Reply::Drop => drop(stream),
                Reply::Json(status, body) => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.flush();
                }
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), seen, observed)
}

/// A clean linked project open in an edit session, signed in as [`ME`] against
/// `server`. `authored`: Start Editing's probe found the mutex free (an inherited
/// lock is the same release, without the authorship licence).
fn editing(dir: &Path, server: &str, authored: bool) -> (Arc<AppState>, PathBuf) {
    let path = dir.join("remote.stalx");
    let mut db = Database::open(&path).expect("open db");
    db.save_project(&Project::new("Remote".to_owned(), None))
        .expect("save project");
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: server.to_owned(),
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
        let mut session = state.speleodb_session_lock().expect("session");
        session.endpoint = Some(server.to_owned());
        session.email = Some(ME.to_owned());
        session.token_store = TokenStore::from_in_memory_token("tok");
        session.rehydrate_attempted = true;
    }
    assert!(state.open_project(path.clone()).success, "open");
    let (generation, link) = state.speleodb_session_snapshot().expect("snapshot");
    state
        .speleodb_commit_editing(Some(generation), &link.project_id, authored)
        .expect("commit");
    assert!(state.edit_enabled(), "fixture: an edit session");
    (Arc::new(state), path)
}

fn observe(observed: &Observed, state: &Arc<AppState>) {
    *observed.lock().expect("observed") = Some(Arc::clone(state));
}

fn claim_is_kept(state: &AppState) -> bool {
    state.speleodb_claim_is_kept(ID)
}

/// The state a failed or uncertain release must leave: nothing closed, editing
/// paused, and the claim kept (unbound) for the user's Retry.
fn assert_still_open_for_the_user(
    state: &AppState,
    result: &crate::core::CommandResult,
    case: &str,
) {
    assert!(!result.success, "{case}: the close did not happen");
    assert_eq!(
        result.lock_release,
        Some(LockReleaseOutcome::Failed),
        "{case}: the failure is reported as a release failure"
    );
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("could not be handed back")),
        "{case}: with a reason the dialog can show: {:?}",
        result.error
    );
    assert!(
        state.get_project_state().is_some(),
        "{case}: the project is still open"
    );
    assert!(!state.edit_enabled(), "{case}: editing is paused");
    assert!(
        state.speleodb_holds_claim(ID),
        "{case}: the claim is kept for the retry"
    );
    assert!(
        !claim_is_kept(state),
        "{case}: but not marked kept - nobody chose that"
    );
}

#[test]
fn a_clean_close_releases_then_closes_and_reports_it() {
    // #668 case (false, false, false, false): the ordinary close.
    let temp = tempfile::TempDir::new().expect("temp");
    let (server, seen, observed) = scripted(|line, _| {
        if is_release(line) {
            project(None)
        } else {
            project(Some(ME))
        }
    });
    let (state, path) = editing(temp.path(), &server, true);
    observe(&observed, &state);
    let before = std::fs::read(&path).expect("saved");
    let closed = state.speleodb_close_releasing_lock();
    assert!(closed.success, "{:?}", closed.error);
    assert_eq!(closed.lock_release, Some(LockReleaseOutcome::Released));
    assert!(state.get_project_state().is_none());
    let seen = seen.lock().expect("log");
    assert_eq!(seen.releases(), 1);
    assert_eq!(
        seen.gate_open_at_release,
        vec![false],
        "editing is paused before the release is sent"
    );
    assert_eq!(std::fs::read(&path).expect("file remains"), before);
}

#[test]
fn a_lock_someone_else_holds_closes_without_a_release() {
    // #668 case (false, true, false, false): another account holds the lock.
    let temp = tempfile::TempDir::new().expect("temp");
    let (server, seen, _observed) = scripted(|_, _| project(Some(SUCCESSOR)));
    let (state, _path) = editing(temp.path(), &server, true);
    let closed = state.speleodb_close_releasing_lock();
    assert!(closed.success, "{:?}", closed.error);
    assert_eq!(closed.lock_release, Some(LockReleaseOutcome::NotHeld));
    assert!(state.get_project_state().is_none());
    assert_eq!(seen.lock().expect("log").releases(), 0, "never released");
    assert!(!state.speleodb_holds_claim(ID), "the claim is over");
}

#[test]
fn a_failed_or_uncertain_release_keeps_the_project_open_until_retry() {
    // #668's 503, no-op release and lost-response cases, for an authored and an
    // inherited lock. Each first close fails with the project still open; Retry is
    // the same call again and closes once the lock is settled.
    #[derive(Clone, Copy, Debug)]
    enum Case {
        /// The first release answers 503; the retry is accepted.
        Unavailable,
        /// The first release answers 200 but still names this account as holder.
        NoOp,
        /// The first release commits and its reply is lost; a successor takes the
        /// lock before the retry, whose holder read then says "not ours".
        LostReply,
    }
    for case in [Case::Unavailable, Case::NoOp, Case::LostReply] {
        for authored in [true, false] {
            let label = format!("{case:?}, authored={authored}");
            let temp = tempfile::TempDir::new().expect("temp");
            let (server, seen, observed) = scripted(move |line, releases| {
                let lost_and_taken = matches!(case, Case::LostReply) && releases > 0;
                if !is_release(line) {
                    return project(Some(if lost_and_taken { SUCCESSOR } else { ME }));
                }
                match (case, releases) {
                    (Case::Unavailable, 0) => unavailable(),
                    (Case::NoOp, 0) => project(Some(ME)),
                    (Case::LostReply, 0) => Reply::Drop,
                    _ => project(None),
                }
            });
            let (state, path) = editing(temp.path(), &server, authored);
            observe(&observed, &state);
            let before = std::fs::read(&path).expect("saved");

            let first = state.speleodb_close_releasing_lock();
            assert_still_open_for_the_user(&state, &first, &label);

            let retry = state.speleodb_close_releasing_lock();
            assert!(
                retry.success,
                "{label}: the retry closes: {:?}",
                retry.error
            );
            assert_eq!(
                retry.lock_release,
                Some(if matches!(case, Case::LostReply) {
                    LockReleaseOutcome::NotHeld
                } else {
                    LockReleaseOutcome::Released
                }),
                "{label}"
            );
            assert!(state.get_project_state().is_none(), "{label}: closed");
            assert!(!state.speleodb_holds_claim(ID), "{label}: claim over");
            let seen = seen.lock().expect("log");
            assert_eq!(
                seen.releases(),
                if matches!(case, Case::LostReply) {
                    1
                } else {
                    2
                },
                "{label}: a successor's lock is never released"
            );
            assert!(
                seen.gate_open_at_release.iter().all(|open| !open),
                "{label}: no release is sent with editing open"
            );
            assert_eq!(std::fs::read(&path).expect("file"), before, "{label}");
        }
    }
}

#[test]
fn close_and_keep_the_lock_after_a_failure_leaves_it_for_the_next_sign_in() {
    // The dialog's "Close and keep the lock": `close_project` with
    // `release_lock: false`. The claim is marked kept, so neither the
    // stranded-claim cleanup nor a later lifecycle release hands it back behind
    // the user's back; the lock waits for the next sign-in's held-lock prompt.
    let temp = tempfile::TempDir::new().expect("temp");
    let (server, seen, _observed) = scripted(|line, _| {
        if is_release(line) {
            unavailable()
        } else {
            project(Some(ME))
        }
    });
    let (state, _path) = editing(temp.path(), &server, true);
    let first = state.speleodb_close_releasing_lock();
    assert_still_open_for_the_user(&state, &first, "503");

    state.speleodb_keep_open_lock_deliberately();
    assert!(state.close_project().success, "the close proceeds");
    assert!(claim_is_kept(&state), "the claim is kept, on purpose");
    state.speleodb_janitor_stranded_claim();
    assert_eq!(
        seen.lock().expect("log").releases(),
        1,
        "only the failed attempt: the cleanup leaves a kept lock alone"
    );
}

#[test]
fn a_switch_or_quit_settles_the_lock_first_and_reports_a_failure() {
    // The settlement a project switch or a quit runs before leaving: the same
    // release, nothing closed either way.
    let temp = tempfile::TempDir::new().expect("temp");
    let (server, seen, _observed) = scripted(|line, releases| match (is_release(line), releases) {
        (true, 0) => unavailable(),
        (true, _) => project(None),
        (false, _) => project(Some(ME)),
    });
    let (state, _path) = editing(temp.path(), &server, true);

    let first = state.speleodb_settle_open_lock(true);
    assert_still_open_for_the_user(&state, &first, "settle 503");

    let retry = state.speleodb_settle_open_lock(true);
    assert!(retry.success, "{:?}", retry.error);
    assert_eq!(retry.lock_release, Some(LockReleaseOutcome::Released));
    assert!(
        state.get_project_state().is_some(),
        "settling never closes: the switch or quit does that"
    );
    assert!(!state.edit_enabled(), "the claim is over, so is editing");
    // The switch's own (detached) release and the exit's have nothing left to send.
    state.speleodb_release_held_lock_bounded(std::time::Duration::from_secs(2));
    assert_eq!(seen.lock().expect("log").releases(), 2);
}

#[test]
fn keeping_the_lock_on_a_switch_or_quit_stops_every_later_release() {
    let temp = tempfile::TempDir::new().expect("temp");
    let (server, seen, _observed) = scripted(|line, _| {
        if is_release(line) {
            unavailable()
        } else {
            project(Some(ME))
        }
    });
    let (state, _path) = editing(temp.path(), &server, true);
    assert!(!state.speleodb_settle_open_lock(true).success);

    let kept = state.speleodb_settle_open_lock(false);
    assert!(kept.success);
    assert_eq!(kept.lock_release, None, "nothing was released");
    assert!(claim_is_kept(&state));
    // What the switch's load and the app exit run afterwards: both leave it.
    state.speleodb_release_held_lock_bounded(std::time::Duration::from_secs(2));
    state.speleodb_release_held_lock_detached();
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert_eq!(
        seen.lock().expect("log").releases(),
        1,
        "only the failed attempt"
    );
}

#[test]
fn a_close_without_a_usable_credential_keeps_the_project_open() {
    // #668 `confirmed_close_without_credentials_keeps_the_project_open`: the
    // signed-in account holds the claim but has no token to hand it back with.
    // Nothing is sent, and the close waits for the user; after signing in again
    // the retry releases and closes.
    let temp = tempfile::TempDir::new().expect("temp");
    let (server, seen, _observed) = scripted(|line, _| {
        if is_release(line) {
            project(None)
        } else {
            project(Some(ME))
        }
    });
    let (state, _path) = editing(temp.path(), &server, true);
    state.speleodb_session_lock().expect("session").token_store = TokenStore::default();

    let first = state.speleodb_close_releasing_lock();
    assert_still_open_for_the_user(&state, &first, "no token");
    assert!(
        seen.lock().expect("log").lines.is_empty(),
        "nothing was asked of the server"
    );

    state.speleodb_session_lock().expect("session").token_store =
        TokenStore::from_in_memory_token("tok");
    let retry = state.speleodb_close_releasing_lock();
    assert!(retry.success, "{:?}", retry.error);
    assert_eq!(retry.lock_release, Some(LockReleaseOutcome::Released));
}

#[test]
fn a_signed_out_close_leaves_the_claim_for_the_next_sign_in() {
    // The contrast to the case above, and a decision rather than a gap: signing out
    // (or a rejected token) already ended the session, and the sign-out flow asked
    // about its locks (Try again / Sign out anyway). The claim belongs to an account
    // no longer signed in, so the close cannot release it and does not ask again;
    // the claim stays for the stranded-claim cleanup after that account's next
    // sign-in, and the lock shows in its held-lock prompt.
    let temp = tempfile::TempDir::new().expect("temp");
    let (server, seen, _observed) = scripted(|_, _| project(Some(ME)));
    let (state, _path) = editing(temp.path(), &server, true);
    state.speleodb_session_lock().expect("session").forget();
    let closed = state.speleodb_close_releasing_lock();
    assert!(closed.success, "{:?}", closed.error);
    assert_eq!(closed.lock_release, None);
    assert!(seen.lock().expect("log").lines.is_empty());
    let session = state.speleodb_session_lock().expect("session");
    assert_eq!(
        session
            .locks_acquired_here
            .iter()
            .filter(|claim| claim.project_id == ID)
            .count(),
        1,
        "the claim survives for the next sign-in"
    );
}

#[test]
#[expect(
    clippy::panic,
    reason = "the panic poisons the session mutex on purpose"
)]
fn an_unreadable_session_refuses_the_close_but_still_lets_the_user_keep_the_lock() {
    // #668 `pending_close_release_poisoned_session_refuses_teardown`: a poisoned
    // session lock cannot say whether this account holds the lock, which is not
    // "it does not". The close is refused with editing paused; the user can still
    // leave with Close and keep the lock.
    let temp = tempfile::TempDir::new().expect("temp");
    let (server, seen, _observed) = scripted(|_, _| project(Some(ME)));
    let (state, _path) = editing(temp.path(), &server, true);
    let poisoner = Arc::clone(&state);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.speleodb.lock().expect("session");
        std::panic::panic_any("poison the session mutex on purpose");
    })
    .join();
    assert!(state.speleodb.is_poisoned(), "precondition");

    let refused = state.speleodb_close_releasing_lock();
    assert!(!refused.success);
    assert_eq!(refused.lock_release, Some(LockReleaseOutcome::Failed));
    assert!(refused
        .error
        .as_deref()
        .is_some_and(|e| e.contains("session lock poisoned")));
    assert!(state.get_project_state().is_some(), "still open");
    assert!(!state.edit_enabled(), "editing is paused");
    assert!(seen.lock().expect("log").lines.is_empty(), "nothing sent");

    state.speleodb_keep_open_lock_deliberately();
    assert!(
        state.close_project().success,
        "Close and keep the lock closes"
    );
}
