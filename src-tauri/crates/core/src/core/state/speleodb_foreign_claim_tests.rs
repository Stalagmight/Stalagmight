//! A release stands down only for a later session of the RELEASING account
//! (F4 of the SpeleoDB protocol fuzzer, "foreign claim stand-down").
//!
//! A release that runs after the session that decided it has ended (a project
//! switch, a quit or the update restart, a delete, Start Editing's unwind after a
//! boundary, the stranded-claim cleanup) sends nothing when a later session has
//! claimed the same project's lock meanwhile: the server treats a re-acquire by
//! the account that already holds the lock as a no-op, so that session holds the
//! very lock the release would free (F8). The check matched a claim of ANY
//! account, though, and claims of other accounts are kept across a sign-in by
//! design (each is the only pointer to a lock that account may still hold). So
//! the owner's stale claim on a project stood down the collaborator's own
//! release of the same project, nothing was sent, and the collaborator's lock
//! stayed held on the server - SpeleoDB never expires a lock on its own.
//!
//! The fuzzer's trace, replayed here with the app's own verbs: the owner signs
//! in, opens and locks a project; the owner's other session hands the lock back;
//! the collaborator signs in on the same server without signing out; the
//! collaborator takes the lock; the update restart releases. Synthetic
//! throughout: a stateful loopback SpeleoDB that keeps the project's mutex.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::speleodb_lock_release::LockReleaseParams;
use super::speleodb_session::LockClaim;
use super::stalx_fixtures::write_v2;
use super::tests_support::sdk_fixture;
use super::AppState;
use crate::models::SpeleoLink;
use crate::speleodb::{client, StartEditingResult};

const ID: &str = "66666666-6666-6666-6666-666666666666";
const OWNER: &str = "owner@x";
const COLLAB: &str = "collab@x";
/// The two accounts' tokens: placeholders the stub tells the accounts apart by,
/// not credentials.
const OWNER_TOKEN: &str = "<your_token_here>";
const COLLAB_TOKEN: &str = "[example]";

/// One request as the stub saw it: the request line and the account whose
/// token it carried.
type Seen = Arc<Mutex<Vec<(String, Option<&'static str>)>>>;

/// A loopback SpeleoDB with one project whose write lock it really keeps, like
/// the server: an acquire takes the lock when nobody or the caller holds it
/// (another holder answers 400), a release by the holder frees it, and every
/// project read reports the holder.
struct Server {
    base: String,
    holder: Arc<Mutex<Option<&'static str>>>,
    seen: Seen,
}

impl Server {
    fn start(holder: Option<&'static str>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let base = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().expect("addr").port()
        );
        let holder = Arc::new(Mutex::new(holder));
        let seen: Seen = Arc::default();
        let (mutex, log) = (Arc::clone(&holder), Arc::clone(&seen));
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let request = read_request(&mut stream);
                let line = request.lines().next().unwrap_or_default().to_owned();
                let account = account_of(&request);
                log.lock().expect("log").push((line.clone(), account));
                let (status, body) = answer(&line, account, &mutex);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.flush();
            }
        });
        Self { base, holder, seen }
    }

    /// Who holds the project's lock on the server now.
    fn holder(&self) -> Option<&'static str> {
        *self.holder.lock().expect("holder")
    }

    fn set_holder(&self, holder: Option<&'static str>) {
        *self.holder.lock().expect("holder") = holder;
    }

    /// How many release requests `account` sent.
    fn releases_by(&self, account: &str) -> usize {
        self.seen
            .lock()
            .expect("log")
            .iter()
            .filter(|(line, by)| is_release(line) && *by == Some(account))
            .count()
    }

    /// How many release requests anybody sent.
    fn releases(&self) -> usize {
        self.seen
            .lock()
            .expect("log")
            .iter()
            .filter(|(line, _)| is_release(line))
            .count()
    }
}

fn is_release(line: &str) -> bool {
    line.starts_with("POST") && line.contains("/release")
}

/// The request's head and body: read up to the end of the headers, then the
/// `Content-Length` bytes, so no request waits out a read timeout.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let head_end = request
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|at| at + 4);
        if let Some(head_end) = head_end {
            let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if request.len() >= head_end + length {
                break;
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(chunk.get(..n).unwrap_or_default()),
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

/// The account whose bearer token the request carries.
fn account_of(request: &str) -> Option<&'static str> {
    let token = request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("authorization")
            .then(|| value.split_whitespace().last())?
    })?;
    [(OWNER_TOKEN, OWNER), (COLLAB_TOKEN, COLLAB)]
        .into_iter()
        .find_map(|(known, account)| (known == token).then_some(account))
}

/// The project as SpeleoDB reports it: one revision, `abc123`, which the copy
/// is based on, and the lock held by `holder` (or nobody).
fn project(holder: Option<&'static str>) -> String {
    let mutex = holder.map_or_else(String::new, |user| {
        format!(r#","active_mutex":{{"user":"{user}"}}"#)
    });
    sdk_fixture(&format!(
        r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE"{mutex},
            "commits":[{{"id":"abc123","parent_ids":[],"author_email":"{OWNER}"}}]}}"#
    ))
}

fn answer(
    line: &str,
    account: Option<&'static str>,
    holder: &Mutex<Option<&'static str>>,
) -> (&'static str, String) {
    let Some(account) = account else {
        return ("401 Unauthorized", r#"{"detail":"no token"}"#.to_owned());
    };
    let mut holder = holder.lock().expect("holder");
    let path = line.split_whitespace().nth(1).unwrap_or_default();
    if path.starts_with("/api/v2/user") {
        return (
            "200 OK",
            sdk_fixture(&format!(r#"{{"email":"{account}"}}"#)),
        );
    }
    if line.starts_with("POST") && path.contains("/acquire") {
        if holder.is_some_and(|by| by != account) {
            return ("400 Bad Request", r#"{"detail":"locked"}"#.to_owned());
        }
        *holder = Some(account);
        return ("200 OK", project(*holder));
    }
    if is_release(line) {
        if *holder != Some(account) {
            return ("400 Bad Request", r#"{"detail":"not yours"}"#.to_owned());
        }
        *holder = None;
        return ("200 OK", project(None));
    }
    ("200 OK", project(*holder))
}

/// A sink that gives a release thread this process's managed state, as the app
/// does: without one a deferred release has no claims to consult, and skips
/// the reclaim check this file is about.
struct ManagedSink(&'static AppState);

impl crate::core::events::EventSink for ManagedSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        f(self.0);
        true
    }
}

/// Sign in to `server` with `token` from Settings: a token login, which asks
/// the server whose token it is. No sign-out in between, so the previous
/// account's claims stay, as they do across every sign-in.
fn sign_in(state: &AppState, server: &Server, token: &str) {
    state
        .speleodb_login(
            Some(server.base.clone()),
            None,
            None,
            None,
            Some(token.to_owned()),
        )
        .expect("token login against the loopback server");
}

/// The account of every claim on [`ID`] at `base` this process keeps, whoever
/// is signed in.
fn claim_accounts(state: &AppState, base: &str) -> Vec<&'static str> {
    let session = state.speleodb_session_lock().expect("session");
    let mut accounts: Vec<&'static str> = session
        .locks_acquired_here
        .iter()
        .filter(|claim| claim.project_id == ID)
        .filter_map(|claim| {
            [OWNER, COLLAB]
                .into_iter()
                .find(|account| claim.taken_by(Some(base), Some(account)))
        })
        .collect();
    accounts.sort_unstable();
    accounts
}

/// The fuzzer's trace up to the release (steps 17 to 28): the collaborator edits
/// the project, holding its lock, while the owner's claim on it is still kept.
/// The state is leaked so a release thread can reach it through the sink.
fn collaborator_editing_over_the_owners_claim(
    dir: &Path,
    server: &Server,
) -> (&'static AppState, PathBuf) {
    let path = dir.join("remote.stalx");
    write_v2(
        &path,
        "Remote",
        Some(&SpeleoLink {
            endpoint: server.base.clone(),
            project_id: ID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        }),
    );
    let state: &'static AppState = Box::leak(Box::new(AppState::new().expect("state")));
    state.set_event_sink(Arc::new(ManagedSink(state)));

    // The owner opens the project and takes its lock.
    sign_in(state, server, OWNER_TOKEN);
    assert!(state.open_project(path.clone()).success, "open");
    let editing = state.speleodb_start_editing();
    assert!(
        matches!(editing, Ok(StartEditingResult::Editing)),
        "the owner edits: {editing:?}"
    );
    assert_eq!(
        server.holder(),
        Some(OWNER),
        "fixture: the owner holds the lock"
    );

    // The owner's other session (another computer, or the website) hands it
    // back. This process is not told.
    client::release_lock(&server.base, OWNER_TOKEN, false, ID)
        .expect("the other session's release");
    assert_eq!(server.holder(), None, "fixture: nobody holds the lock");

    // The collaborator signs in on the same server, without signing out: the
    // owner's claim stays, as every other account's claim does across a sign-in.
    sign_in(state, server, COLLAB_TOKEN);
    assert_eq!(
        claim_accounts(state, &server.base),
        vec![OWNER],
        "fixture: the owner's claim is kept across the sign-in"
    );

    // The collaborator takes the lock.
    let editing = state.speleodb_start_editing();
    assert!(
        matches!(editing, Ok(StartEditingResult::Editing)),
        "the collaborator edits: {editing:?}"
    );
    assert_eq!(
        server.holder(),
        Some(COLLAB),
        "fixture: the collaborator holds the lock"
    );
    assert_eq!(
        claim_accounts(state, &server.base),
        vec![COLLAB, OWNER],
        "fixture: both claims on the project, one per account"
    );
    assert_eq!(
        server.releases(),
        1,
        "fixture: only the other session's release so far"
    );
    (state, path)
}

#[test]
fn the_update_restart_hands_back_the_collaborators_lock_over_the_owners_stale_claim() {
    // The fuzzer's step 32 (seed 900380): the update restart's release came to
    // "not held" with no request at all, and the collaborator's lock stayed held.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::TempDir::new().expect("temp");
        let server = Server::start(None);
        let (state, _path) = collaborator_editing_over_the_owners_claim(temp.path(), &server);

        state.speleodb_release_held_lock_bounded(Duration::from_secs(5));

        assert_eq!(
            server.releases_by(COLLAB),
            1,
            "the collaborator's release is sent: the owner's claim says nothing about \
             the collaborator's lock"
        );
        assert_eq!(server.holder(), None, "and the lock is free on the server");
        assert_eq!(
            claim_accounts(state, &server.base),
            vec![OWNER],
            "the collaborator's claim is over; the owner's is the owner's to end"
        );
    });
}

/// A release verb, returning what it reported to its caller.
type Release = fn(&'static AppState, &Path) -> String;

/// Every other release of the open project's lock that consults the claims:
/// the user's close, a project switch, and a delete.
fn every_other_release() -> [(&'static str, Release); 3] {
    [
        ("close", |state, _| {
            format!("{:?}", state.speleodb_release_held_lock_on_close())
        }),
        ("switch", |state, _| {
            state.speleodb_release_held_lock_detached();
            String::new()
        }),
        ("delete", |state, path| {
            let result = state.delete_project(path.to_path_buf());
            format!(
                "success {}, warnings {}",
                result.success,
                result.warnings.len()
            )
        }),
    ]
}

#[test]
fn every_release_of_the_collaborators_lock_goes_out_over_the_owners_stale_claim() {
    // Each case is judged on what reached the server, collected so a failure
    // shows every path at once. A config of its own per case: the sign-ins save
    // the session, and the next case's state would start signed in to the last
    // case's server.
    let outcomes: Vec<_> = every_other_release()
        .into_iter()
        .map(|(case, release)| {
            crate::test_helpers::with_isolated_config(|| {
                let temp = tempfile::TempDir::new().expect("temp");
                let server = Server::start(None);
                let (state, path) =
                    collaborator_editing_over_the_owners_claim(temp.path(), &server);

                let reported = release(state, &path);

                // The switch's release runs on its own thread.
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while server.holder().is_some() && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                (case, server.releases_by(COLLAB), server.holder(), reported)
            })
        })
        .collect();
    assert_eq!(
        outcomes,
        vec![
            // The close reports the lock handed back, not merely "not held".
            ("close", 1, None, "Ok(Some(Released))".to_owned()),
            ("switch", 1, None, String::new()),
            // A delete with no lock-release warning.
            ("delete", 1, None, "success true, warnings 0".to_owned()),
        ],
        "(path, the collaborator's release requests, the holder left, what the verb \
         reported)"
    );
}

/// A claim on [`ID`] at `base`, taken as `email`, with no edit session.
fn claim(base: &str, email: &str) -> LockClaim {
    LockClaim::taken(ID, base.to_owned(), email.to_owned(), true)
}

/// Sign `email` in on `base` directly, with the token the stub knows it by.
fn signed_in_as(state: &AppState, base: &str, email: &str, token: &str) {
    let mut session = state.speleodb_session_lock().expect("session");
    session.endpoint = Some(base.to_owned());
    session.email = Some(email.to_owned());
    session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token(token);
    session.rehydrate_attempted = true;
}

fn collaborators_release(base: &str, email: &str) -> LockReleaseParams {
    LockReleaseParams {
        endpoint: base.to_owned(),
        token: COLLAB_TOKEN.to_owned(),
        email: email.to_owned(),
        insecure_http: false,
        project_id: ID.to_owned(),
    }
}

#[test]
fn only_a_claim_of_the_releasing_account_stands_a_release_down() {
    // The release is the collaborator's; the owner is signed in now. A deferred
    // release can outlive the sign-in that captured it, so neither "any account"
    // nor "the account signed in now" is the account the question is about.
    let server = Server::start(Some(COLLAB));
    let state = AppState::new().expect("state");
    signed_in_as(&state, &server.base, OWNER, OWNER_TOKEN);

    // Only the owner's claim on the same project and deployment: no successor of
    // the collaborator's lock, so the release goes out.
    state
        .speleodb_session_lock()
        .expect("session")
        .locks_acquired_here
        .push(claim(&server.base, OWNER));
    state
        .speleodb_release_unless_reclaimed(&collaborators_release(&server.base, COLLAB))
        .expect("released");
    assert_eq!(
        server.releases_by(COLLAB),
        1,
        "another account's claim is no reclaim, signed in or not"
    );
    assert_eq!(server.holder(), None);

    // A later session of the releasing account itself holds a claim (under the
    // case SpeleoDB may give the email): it inherited this very lock, so the
    // release stands down and sends nothing, whoever is signed in now.
    server.set_holder(Some(COLLAB));
    {
        let mut session = state.speleodb_session_lock().expect("session");
        session.locks_acquired_here.clear();
        session
            .locks_acquired_here
            .push(claim(&server.base, "Collab@X"));
    }
    state
        .speleodb_release_unless_reclaimed(&collaborators_release(&server.base, COLLAB))
        .expect("stood down");
    assert_eq!(
        server.releases_by(COLLAB),
        1,
        "the same account's successor keeps the lock: no second release"
    );
    assert_eq!(
        server.holder(),
        Some(COLLAB),
        "still held, by the successor"
    );
}

#[test]
fn the_stranded_claim_cleanup_hands_back_a_lock_beside_another_accounts_claim() {
    // The same confusion in the cleanup lost the claim as well: the cleanup takes
    // the claim out before it releases, stood down for the owner's, and with the
    // release "done" never put it back - the lock stayed held with nothing local
    // left to find it.
    let server = Server::start(Some(COLLAB));
    let state = AppState::new().expect("state");
    signed_in_as(&state, &server.base, COLLAB, COLLAB_TOKEN);
    {
        let mut session = state.speleodb_session_lock().expect("session");
        session.locks_acquired_here.push(claim(&server.base, OWNER));
        session
            .locks_acquired_here
            .push(claim(&server.base, COLLAB));
    }

    state.speleodb_janitor_stranded_claim();

    assert_eq!(
        server.releases_by(COLLAB),
        1,
        "the stranded lock is handed back"
    );
    assert_eq!(server.holder(), None);
    let left: Vec<bool> = state
        .speleodb_session_lock()
        .expect("session")
        .locks_acquired_here
        .iter()
        .map(|claim| claim.taken_by(Some(&server.base), Some(OWNER)))
        .collect();
    assert_eq!(
        left,
        vec![true],
        "only the owner's claim is left, for the owner"
    );
}

#[test]
fn start_editings_unwind_after_a_boundary_ignores_another_accounts_claim() {
    // Start Editing's unwind after a session boundary is the fourth caller of the
    // reclaim check: the abort arm hands back the lock this call took, unless a
    // successor of the same account has claimed it meanwhile.
    let server = Server::start(Some(COLLAB));
    let state = AppState::new().expect("state");
    signed_in_as(&state, &server.base, COLLAB, COLLAB_TOKEN);
    let entry_gen = state.speleodb_session_generation();
    state.close_project();
    assert_ne!(
        state.speleodb_session_generation(),
        entry_gen,
        "fixture: a boundary has passed"
    );
    state
        .speleodb_session_lock()
        .expect("session")
        .locks_acquired_here
        .push(claim(&server.base, OWNER));

    state.speleodb_unwind_release(&collaborators_release(&server.base, COLLAB), entry_gen);

    assert_eq!(
        server.releases_by(COLLAB),
        1,
        "the unwind hands the lock back"
    );
    assert_eq!(server.holder(), None);
}
