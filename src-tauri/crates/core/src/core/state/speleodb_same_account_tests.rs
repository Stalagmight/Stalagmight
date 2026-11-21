//! Two sessions of ONE account against one loopback SpeleoDB (fuzzer finding
//! R3, same-account-overwrite). SpeleoDB locks are per account, so both
//! sessions hold the project's lock and the server cannot tell them apart: the
//! only thing between session B's upload and session A's revision is the
//! client's own check that the server's HEAD is still the copy's BASE.
//!
//! Each session is its own `AppState` on its own thread, with its own cache
//! root and config, as two machines would be. The server keeps a real history:
//! every upload it accepts lands as a new revision on top of the tip, authored
//! by the account, with the description it carried.
//!
//! All data is synthetic: the project is built by the encoder, the server is a
//! loopback listener.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::speleodb_local_state::{cache_path_in, TestCacheRoot};
use super::stalx_fixtures::{extras_for, line_survey, link_on_disk, write_file};
use super::AppState;
use crate::core::Command;
use crate::models::{SpeleoLink, UndoMode};
use crate::speleodb::auth::TokenStore;
use crate::speleodb::{DoneResult, PushResult};

const ID: &str = "11111111-1111-1111-1111-111111111111";
const ME: &str = "diver@example.com";
/// The revision both sessions downloaded.
const BASE: &str = "base000";

/// One revision on the loopback server.
#[derive(Clone, Debug)]
struct Rev {
    id: String,
    parent: Option<String>,
    message: String,
}

/// How the server answers the next upload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Next {
    /// Land it and answer 200 with its revision.
    #[default]
    Accept,
    /// Land it, then drop the connection without an answer: the upload is
    /// on the server, the client cannot know.
    LandSilently,
    /// Drop the connection before landing anything: an unanswered upload that
    /// made no revision.
    DropUnlanded,
}

/// The server's state, shared with the test.
#[derive(Default)]
struct Remote {
    /// Oldest first.
    history: Vec<Rev>,
    holder: Option<String>,
    /// Every upload request, whole.
    uploads: Vec<String>,
    lines: Vec<String>,
    next: Next,
    /// Lands just before the next accepted upload does: another session's
    /// revision, pushed while that upload was on its way (the lost race).
    meanwhile: Option<String>,
    minted: u32,
}

impl Remote {
    fn tip(&self) -> Option<String> {
        self.history.last().map(|r| r.id.clone())
    }

    fn land(&mut self, message: &str) -> String {
        self.minted += 1;
        let id = format!("rev{:04}", self.minted);
        let parent = self.tip();
        self.history.push(Rev {
            id: id.clone(),
            parent,
            message: message.to_owned(),
        });
        id
    }

    fn commits_json(&self) -> String {
        let commits: Vec<_> = self
            .history
            .iter()
            .rev()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "parent_ids": r.parent.iter().collect::<Vec<_>>(),
                    "author_email": ME,
                    "message": r.message,
                })
            })
            .collect();
        serde_json::json!({ "commits": commits }).to_string()
    }
}

type Shared = Arc<Mutex<Remote>>;

/// The upload's description: the multipart `message` part.
fn upload_message(request: &str) -> String {
    request
        .split("name=\"message\"")
        .nth(1)
        .and_then(|part| part.split_once("\r\n\r\n"))
        .and_then(|(_, rest)| rest.split_once("\r\n--"))
        .map(|(message, _)| message.to_owned())
        .unwrap_or_default()
}

/// A loopback SpeleoDB whose history starts at [`BASE`] and whose lock the
/// account [`ME`] holds.
fn server() -> (String, Shared) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let remote = Arc::new(Mutex::new(Remote {
        history: vec![Rev {
            id: BASE.to_owned(),
            parent: None,
            message: "First survey".to_owned(),
        }],
        holder: Some(ME.to_owned()),
        ..Remote::default()
    }));
    let shared = Arc::clone(&remote);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_request(&mut stream);
            let line = request.lines().next().unwrap_or_default().to_owned();
            let Ok(mut remote) = shared.lock() else {
                return;
            };
            remote.lines.push(line.clone());
            let mutex = remote
                .holder
                .as_ref()
                .map_or_else(|| "null".to_owned(), |h| format!(r#"{{"user":"{h}"}}"#));
            let project = super::tests_support::sdk_fixture(&format!(
                r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{mutex}}}"#
            ));
            let (status, body) = if line.contains("/user/") {
                (
                    200,
                    super::tests_support::sdk_fixture(&format!(r#"{{"email":"{ME}"}}"#)),
                )
            } else if line.contains("/revisions/") {
                (200, super::tests_support::sdk_fixture(&remote.commits_json()))
            } else if line.contains("/upload/") {
                remote.uploads.push(request.clone());
                let next = std::mem::take(&mut remote.next);
                if next == Next::DropUnlanded {
                    continue;
                }
                if let Some(message) = remote.meanwhile.take() {
                    remote.land(&message);
                }
                let landed = remote.land(&upload_message(&request));
                if next == Next::LandSilently {
                    continue;
                }
                (
                    200,
                    format!(
                        r#"{{"project":{project},"hexsha":"{landed}","files":["project.stalx"],"message":"Saved","browser_url":null}}"#
                    ),
                )
            } else if line.contains("/release/") {
                remote.holder = None;
                (200, project)
            } else if line.contains("/acquire/") {
                remote.holder = Some(ME.to_owned());
                (200, project)
            } else {
                (200, project)
            };
            drop(remote);
            let reply = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), remote)
}

/// One whole request (headers and the `Content-Length` body).
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    while let Ok(count) = stream.read(&mut chunk) {
        if count == 0 {
            break;
        }
        request.extend_from_slice(chunk.get(..count).unwrap_or_default());
        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
            let length = headers
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

type Job = Box<dyn FnOnce(&AppState) + Send>;

/// One session of the account: an `AppState` on a thread of its own, with its
/// own cache root and config (both are per thread in tests), editing the
/// project it downloaded at [`BASE`].
struct Session {
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
}

impl Session {
    fn start(root: &Path, endpoint: &str) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let root = root.to_path_buf();
        let path = cache_path_in(&root, ID);
        let endpoint = endpoint.to_owned();
        let worker = std::thread::spawn(move || {
            crate::test_helpers::with_isolated_config(|| {
                let _cache = TestCacheRoot::set(&root);
                let state = editing(&root, &endpoint);
                for job in inbox {
                    job(&state);
                }
            });
        });
        Self {
            jobs: Some(jobs),
            worker: Some(worker),
            path,
        }
    }

    /// Run `f` on this session's thread and wait for its answer.
    fn run<R: Send + 'static>(&self, f: impl FnOnce(&AppState) -> R + Send + 'static) -> R {
        let (answer, reply) = mpsc::channel();
        self.jobs
            .as_ref()
            .expect("session open")
            .send(Box::new(move |state: &AppState| {
                let _ = answer.send(f(state));
            }))
            .expect("session alive");
        reply.recv().expect("the session answered (see its panic above)")
    }

    /// The link as this session's copy holds it on disk.
    fn link_on_disk(&self) -> SpeleoLink {
        link_on_disk(&self.path).expect("linked")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.jobs.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// A Full-mode linked copy at its cache path under `root`, at [`BASE`], open in
/// a signed-in edit session against `endpoint`.
fn editing(root: &Path, endpoint: &str) -> AppState {
    let path = cache_path_in(root, ID);
    let link = SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: ID.to_owned(),
        project_name: "Remote".to_owned(),
        base_commit_sha: Some(BASE.to_owned()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: Some(chrono::Utc::now().to_rfc3339()),
        undo_mode: Some(UndoMode::Full),
    };
    write_file(&path, &line_survey("Remote"), &extras_for(Some(&link)));
    let state = AppState::new().expect("state");
    assert!(state.open_project(path).success, "open the cache copy");
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

/// An edit: reverse the fixture's line.
fn edit(state: &AppState) {
    let line_id = state
        .get_project_state()
        .expect("open")
        .lines()
        .first()
        .expect("the fixture line")
        .id;
    let result = state.execute(Command::ReverseLine {
        line_id,
        swap_left_right: false,
    });
    assert!(result.success, "{:?}", result.error);
}

fn push(message: &'static str) -> impl FnOnce(&AppState) -> PushResult + Send + 'static {
    move |state| {
        edit(state);
        state
            .speleodb_push_only(Some(message.to_owned()))
            .expect("the push answers")
    }
}

#[test]
fn a_second_session_of_the_same_account_is_refused_and_uploads_nothing() {
    // The fuzzer's R3 trace. Both sessions hold the lock (it is the account's).
    // A uploads; B, still on BASE, then uploads too. B's upload must not land:
    // its copy does not have A's work, so the server's HEAD would silently lose
    // it (only the history would still hold it). B is told through the
    // divergence prompt, keeps its work, its lock and its BASE.
    // Injection: let `classify_ancestry` take every commit the account authored
    // as this session's own (the old authorship shortcut) and B's push adopts
    // A's revision and uploads over it (two uploads, B on top).
    let temp = tempfile::tempdir().expect("temp");
    let (endpoint, remote) = server();
    let a = Session::start(&temp.path().join("a"), &endpoint);
    let b = Session::start(&temp.path().join("b"), &endpoint);

    let pushed = a.run(push("From A"));
    assert!(matches!(pushed, PushResult::Pushed { .. }), "{pushed:?}");
    let a_head = remote.lock().expect("remote").tip();
    assert_ne!(a_head.as_deref(), Some(BASE), "A's revision landed");

    let refused = b.run(push("From B"));
    assert!(
        matches!(refused, PushResult::DivergenceNeeded),
        "B is refused: {refused:?}"
    );
    let done = b.run(|state| {
        state
            .speleodb_done(Some("From B".to_owned()))
            .expect("Done answers")
    });
    assert!(
        matches!(done, DoneResult::DivergenceNeeded),
        "Done (the close, quit and sign-out uploads) is refused the same way: {done:?}"
    );

    {
        let remote = remote.lock().expect("remote");
        assert_eq!(remote.uploads.len(), 1, "nothing of B's was uploaded");
        assert_eq!(remote.tip(), a_head, "A's revision is still HEAD");
        assert!(
            !remote.lines.iter().any(|l| l.contains("/release/")),
            "B kept its lock"
        );
    }
    let on_disk = b.link_on_disk();
    assert!(on_disk.has_unpushed_edits, "B's work is still un-uploaded");
    assert_eq!(on_disk.base_commit_sha.as_deref(), Some(BASE), "BASE unchanged");
    let (editing, can_undo) = b.run(|state| (state.edit_enabled(), state.can_undo()));
    assert!(editing && can_undo, "B keeps its lock and its history");
}

#[test]
fn our_own_unanswered_upload_is_adopted_on_the_retry() {
    // The recovery the authorship test was there for (#193) still works: an
    // upload of THIS session whose answer was lost did land, so the retry adopts
    // it and uploads on top instead of calling the user's own revision a
    // conflict.
    let temp = tempfile::tempdir().expect("temp");
    let (endpoint, remote) = server();
    let a = Session::start(&temp.path().join("a"), &endpoint);

    remote.lock().expect("remote").next = Next::LandSilently;
    let lost = a.run(push("From A"));
    assert!(matches!(lost, PushResult::PushUnconfirmed), "{lost:?}");
    let first = remote.lock().expect("remote").tip();
    assert_ne!(first.as_deref(), Some(BASE), "the unanswered upload landed");

    let retried = a.run(|state| {
        state
            .speleodb_push_only(Some("From A".to_owned()))
            .expect("the retry answers")
    });
    assert!(
        matches!(retried, PushResult::Pushed { .. }),
        "our own landed upload is adopted, not a conflict: {retried:?}"
    );
    let head = {
        let remote = remote.lock().expect("remote");
        assert_eq!(remote.uploads.len(), 2);
        let top = remote.history.last().expect("history");
        assert_eq!(top.parent, first, "the retry landed on top of our own");
        remote.tip()
    };
    let on_disk = a.link_on_disk();
    assert_eq!(on_disk.base_commit_sha, head);
    assert!(!on_disk.has_unpushed_edits);
}

#[test]
fn an_unanswered_upload_is_no_licence_to_adopt_another_sessions_revision() {
    // B's upload dropped before it landed, and A pushed meanwhile: one revision
    // of the account on BASE, exactly where B's own would be. It is not B's (it
    // carries A's description), so B's retry must be refused, not adopt it and
    // land over it.
    // Injection: drop the description test from the unanswered-upload match in
    // `classify_ancestry` and the retry uploads (three uploads).
    let temp = tempfile::tempdir().expect("temp");
    let (endpoint, remote) = server();
    let a = Session::start(&temp.path().join("a"), &endpoint);
    let b = Session::start(&temp.path().join("b"), &endpoint);

    remote.lock().expect("remote").next = Next::DropUnlanded;
    let dropped = b.run(push("From B"));
    assert!(matches!(dropped, PushResult::PushUnconfirmed), "{dropped:?}");
    assert_eq!(remote.lock().expect("remote").tip().as_deref(), Some(BASE));

    let pushed = a.run(push("From A"));
    assert!(matches!(pushed, PushResult::Pushed { .. }), "{pushed:?}");
    let a_head = remote.lock().expect("remote").tip();

    let retried = b.run(|state| {
        state
            .speleodb_push_only(Some("From B".to_owned()))
            .expect("the retry answers")
    });
    assert!(
        matches!(retried, PushResult::DivergenceNeeded),
        "A's revision is not B's unanswered upload: {retried:?}"
    );
    let remote = remote.lock().expect("remote");
    assert_eq!(remote.uploads.len(), 2, "B's retry uploaded nothing");
    assert_eq!(remote.tip(), a_head);
    drop(remote);
    assert_eq!(b.link_on_disk().base_commit_sha.as_deref(), Some(BASE));
}
