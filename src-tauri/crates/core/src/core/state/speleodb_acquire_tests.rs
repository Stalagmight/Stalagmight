//! An acquire whose outcome is unknown (sdk#4): a transport failure, a 5xx or an
//! unreadable reply may arrive after the server committed the mutex to us. Start
//! Editing re-reads the holder and adopts the lock when it is ours, reports the
//! contention when it is someone else's, and - when it still cannot tell - leaves
//! a provisional claim that a later holder-checked release can hand back, never a
//! blind release. Synthetic throughout: a stateful loopback SpeleoDB.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::AppState;
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;
use crate::speleodb::StartEditingResult;

const ID: &str = "44444444-4444-4444-4444-444444444444";
const ME: &str = "u@x";

/// How the stub answers the acquire.
#[derive(Clone, Copy)]
enum AcquireReply {
    /// 502 after taking the mutex: a proxy that gave up on a slow reply.
    TakesThenBadGateway,
    /// 502 WITHOUT taking it.
    BadGatewayOnly,
    /// Close the connection without a reply after taking the mutex.
    TakesThenResets,
}

/// What a project read after the acquire reports.
#[derive(Clone, Copy)]
enum AfterAcquire {
    /// The truth, as the acquire left it.
    Truth,
    /// Another account holds the mutex.
    SomeoneElse,
    /// Reads fail with a 503.
    Unreadable,
    /// The first `n` reads fail with a 503, then the truth.
    UnreadableFor(usize),
}

struct Stub {
    base: String,
    log: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    fn requests(&self, needle: &str) -> usize {
        self.log
            .lock()
            .expect("log")
            .iter()
            .filter(|line| line.contains(needle))
            .count()
    }
}

/// Project + revision body: one root commit, `abc123`, so a copy based on it
/// reconciles UpToDate. `holder` is the mutex.
fn body(holder: Option<&str>) -> String {
    let mutex = holder.map_or_else(String::new, |user| {
        format!(r#","active_mutex":{{"user":"{user}"}}"#)
    });
    super::tests_support::sdk_fixture(&format!(
        r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE"{mutex},
            "commits":[{{"id":"abc123","parent_ids":[],"author_email":"{ME}"}}]}}"#
    ))
}

fn respond(stream: &mut std::net::TcpStream, status: &str, payload: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.flush();
}

fn serve(acquire: AcquireReply, after: AfterAcquire) -> Stub {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("addr").port()
    );
    let log = Arc::new(Mutex::new(Vec::new()));
    let server_log = Arc::clone(&log);
    std::thread::spawn(move || {
        let mut held_by_me = false;
        let mut acquired = false;
        let mut failed_reads = 0;
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
            let mut request = Vec::new();
            let mut chunk = vec![0u8; 8 * 1024];
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
            server_log.lock().expect("log").push(line.clone());
            if line.starts_with("POST") && line.contains("/acquire/") {
                acquired = true;
                match acquire {
                    AcquireReply::TakesThenBadGateway => {
                        held_by_me = true;
                        respond(
                            &mut stream,
                            "502 Bad Gateway",
                            r#"{"detail":"bad gateway"}"#,
                        );
                    }
                    AcquireReply::BadGatewayOnly => {
                        respond(
                            &mut stream,
                            "502 Bad Gateway",
                            r#"{"detail":"bad gateway"}"#,
                        );
                    }
                    AcquireReply::TakesThenResets => held_by_me = true,
                }
                continue;
            }
            if line.starts_with("POST") && line.contains("/release/") {
                held_by_me = false;
                respond(&mut stream, "200 OK", &body(None));
                continue;
            }
            // Every read: the pre-acquire probe, the re-read, the revisions, a
            // release's holder check.
            let holder = match (acquired, after) {
                (true, AfterAcquire::Unreadable) => {
                    respond(&mut stream, "503 Service Unavailable", "{}");
                    continue;
                }
                (true, AfterAcquire::UnreadableFor(n)) if failed_reads < n => {
                    failed_reads += 1;
                    respond(&mut stream, "503 Service Unavailable", "{}");
                    continue;
                }
                (true, AfterAcquire::SomeoneElse) => Some("someone.else@x"),
                _ => held_by_me.then_some(ME),
            };
            respond(&mut stream, "200 OK", &body(holder));
        }
    });
    Stub { base, log }
}

/// A clean linked copy at BASE `abc123`, open read-only, signed in on `base`.
fn open_linked(dir: &Path, base: &str) -> AppState {
    let path = dir.join("linked.stalx");
    let mut db = Database::open(&path).expect("open db");
    db.save_project(&Project::new("Linked".to_owned(), None))
        .expect("save project");
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: base.to_owned(),
        project_id: ID.to_owned(),
        project_name: "Linked".to_owned(),
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
        session.endpoint = Some(base.to_owned());
        session.email = Some(ME.to_owned());
        session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token("tok");
        session.rehydrate_attempted = true;
    }
    assert!(state.open_project(path).success, "open");
    assert!(!state.edit_enabled(), "a linked open is read-only");
    state
}

#[test]
fn an_acquire_whose_reply_was_lost_is_adopted_when_the_lock_is_ours() {
    for reply in [
        AcquireReply::TakesThenBadGateway,
        AcquireReply::TakesThenResets,
    ] {
        let temp = tempfile::TempDir::new().expect("temp");
        let stub = serve(reply, AfterAcquire::Truth);
        let state = open_linked(temp.path(), &stub.base);

        let result = state.speleodb_start_editing();

        assert!(
            matches!(result, Ok(StartEditingResult::Editing)),
            "the server says the lock is ours, so editing opens: {result:?}"
        );
        assert!(state.edit_enabled());
        assert!(
            state.speleodb_lock_acquired_here(ID),
            "the probe found the mutex free, so this call took it: an authored claim"
        );
        assert_eq!(stub.requests("/release/"), 0, "an adopted lock is kept");
    }
}

#[test]
fn an_ambiguous_acquire_reports_another_holder_as_contention() {
    let temp = tempfile::TempDir::new().expect("temp");
    let stub = serve(AcquireReply::TakesThenBadGateway, AfterAcquire::SomeoneElse);
    let state = open_linked(temp.path(), &stub.base);

    let result = state.speleodb_start_editing();

    assert!(
        matches!(
            &result,
            Ok(StartEditingResult::LockedBy { email: Some(email) }) if email == "someone.else@x"
        ),
        "{result:?}"
    );
    assert!(!state.edit_enabled());
    assert!(
        !state.speleodb_holds_claim(ID),
        "nothing of ours to point at"
    );
}

#[test]
fn an_acquire_that_did_not_land_reports_the_failure() {
    let temp = tempfile::TempDir::new().expect("temp");
    let stub = serve(AcquireReply::BadGatewayOnly, AfterAcquire::Truth);
    let state = open_linked(temp.path(), &stub.base);

    assert!(state.speleodb_start_editing().is_err(), "the 502 surfaces");
    assert!(!state.edit_enabled());
    assert!(
        !state.speleodb_holds_claim(ID),
        "the server says nobody holds it"
    );
    assert_eq!(stub.requests("/release/"), 0);
}

#[test]
fn an_unknowable_acquire_leaves_a_provisional_claim_and_never_releases_blind() {
    let temp = tempfile::TempDir::new().expect("temp");
    let stub = serve(AcquireReply::TakesThenBadGateway, AfterAcquire::Unreadable);
    let state = open_linked(temp.path(), &stub.base);

    assert!(state.speleodb_start_editing().is_err(), "the 502 surfaces");

    assert!(!state.edit_enabled(), "a provisional claim opens nothing");
    assert!(
        state.speleodb_holds_claim(ID),
        "but it is the pointer a later holder-checked release needs"
    );
    assert!(
        !state.speleodb_lock_acquired_here(ID),
        "and it licenses nothing"
    );
    assert_eq!(stub.requests("/release/"), 0, "nothing was released blind");

    // The close's lifecycle release asks again, still cannot tell, and keeps it -
    // reporting the uncertainty, so the close stays open for the user to decide.
    assert!(state.speleodb_release_held_lock_on_close().is_err());
    assert_eq!(stub.requests("/release/"), 0);
    assert!(state.speleodb_holds_claim(ID));
}

#[test]
fn a_provisional_claim_is_handed_back_once_the_holder_is_readable() {
    // The acquire landed and its reply was lost while reads were down (all three
    // attempts of Start Editing's re-read); by the time the project closes the
    // server answers again and says the lock is ours - so the close hands it back.
    let temp = tempfile::TempDir::new().expect("temp");
    let stub = serve(
        AcquireReply::TakesThenBadGateway,
        AfterAcquire::UnreadableFor(3),
    );
    let state = open_linked(temp.path(), &stub.base);
    assert!(state.speleodb_start_editing().is_err());
    assert!(state.speleodb_holds_claim(ID), "a provisional claim");
    assert!(!state.edit_enabled());

    assert_eq!(
        state
            .speleodb_release_held_lock_on_close()
            .expect("the holder is readable"),
        Some(crate::core::LockReleaseOutcome::Released)
    );

    assert_eq!(
        stub.requests("/release/"),
        1,
        "the holder is readable and it is us: the lock goes back"
    );
    assert!(!state.speleodb_holds_claim(ID), "and the claim is over");
}
