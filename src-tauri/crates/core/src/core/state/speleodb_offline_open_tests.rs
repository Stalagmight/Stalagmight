//! F1: an open whose reads get no reply opens the cached copy offline, whatever
//! way the reply was lost - the connection closed or reset before any reply, a
//! reply head or body cut short - as it already did for a timeout. The rules of
//! that fallback stay as they were: un-uploaded work is still marked so (never
//! passed off as the server's), another account's opens read-only, a reply that
//! is not HTTP or a discard never falls back. A write whose reply is lost the
//! same way stays an outcome nobody knows.
//!
//! All data is synthetic: every copy is a `.stalx` built here, every server a
//! loopback listener that loses its replies on purpose.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::speleodb_cache::LocalEditsPolicy;
use super::speleodb_local_state::{cache_path_in, TestCacheRoot};
use super::stalx_fixtures::{extras_for, line_survey, link_on_disk, write_file, write_v2};
use super::AppState;
use crate::core::Command;
use crate::models::{SpeleoLink, UndoMode};
use crate::speleodb::{
    PushResult, SpeleoDbAction, SpeleoOpenResult, SpeleoReadOnlyOpen, TransportCause,
};

const ID: &str = "11111111-1111-1111-1111-111111111111";
const ME: &str = "diver@example.com";
const BASE: &str = "abc1234";

/// How the loopback server loses a reply.
#[derive(Clone, Copy, Debug)]
enum Lost {
    /// Read the request, then close without a word (the fuzzer's trace, seed
    /// 900366).
    Closed,
    /// Read the request but its last byte, then close: the unread byte makes the
    /// kernel reset the connection.
    Reset,
    /// Close in the middle of the reply head.
    HeadCutShort,
    /// A 200 whose head promises 1000 bytes, then 10 of them and a close.
    BodyCutShort,
    /// Not an HTTP reply at all: the server said something the app cannot read.
    NotHttp,
}

impl Lost {
    /// Every way of losing the reply that leaves the app with no reply at all.
    const NO_REPLY: [Self; 4] = [
        Self::Closed,
        Self::Reset,
        Self::HeadCutShort,
        Self::BodyCutShort,
    ];
}

/// The request line, having read the head and its `Content-Length` body, or the
/// head but its last byte when `keep_last`.
fn read_request(stream: &mut std::net::TcpStream, keep_last: bool) -> String {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let end: &[u8] = if keep_last { b"\r\n\r" } else { b"\r\n\r\n" };
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(end) {
        match stream.read(&mut byte) {
            Ok(1) => request.extend_from_slice(&byte),
            _ => break,
        }
    }
    if !keep_last {
        let head = String::from_utf8_lossy(&request).to_lowercase();
        let length = head
            .lines()
            .find_map(|h| {
                h.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut body = vec![0_u8; length];
        let _ = stream.read_exact(&mut body);
    }
    String::from_utf8_lossy(&request)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The request line waiting on `stream`, without reading it.
fn peek_line(stream: &std::net::TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut head = [0_u8; 1024];
    for _ in 0..100 {
        let peeked = stream.peek(&mut head).unwrap_or(0);
        let text = String::from_utf8_lossy(head.get(..peeked).unwrap_or_default());
        if let Some((line, _)) = text.split_once("\r\n") {
            return line.to_owned();
        }
        if peeked == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    String::new()
}

/// Lose the reply to the request on `stream` as `lost` says.
fn lose_reply(mut stream: std::net::TcpStream, lost: Lost, line: &str) {
    let kind = if line.contains("/download/") {
        "application/octet-stream"
    } else {
        "application/json"
    };
    let reply = match lost {
        Lost::Closed | Lost::Reset => String::new(),
        Lost::HeadCutShort => format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCont"),
        Lost::BodyCutShort => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: 1000\r\n\
             Connection: close\r\n\r\n{{\"commits\""
        ),
        Lost::NotHttp => "SSH-2.0-OpenSSH_9.6\r\nProtocol mismatch.\r\n".to_owned(),
    };
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
    std::thread::sleep(std::time::Duration::from_millis(50));
}

fn answer(stream: &mut std::net::TcpStream, status: u16, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

/// A loopback SpeleoDB that loses the reply, as `lost` says, to every request
/// `faulted` picks, and answers the rest: the project with its lock held by
/// [`ME`], the history at [`BASE`], the account. Records the request lines.
fn server(lost: Lost, faulted: fn(&str) -> bool) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            // The request line, peeked, decides whether to leave a byte unread.
            let fault = faulted(&peek_line(&stream));
            let line = read_request(&mut stream, fault && matches!(lost, Lost::Reset));
            log.lock().expect("log").push(line.clone());
            if fault {
                lose_reply(stream, lost, &line);
            } else if line.contains("/user/") {
                let me = super::tests_support::sdk_fixture(&format!(r#"{{"email":"{ME}"}}"#));
                answer(&mut stream, 200, &me);
            } else if line.contains("/revisions/") {
                let history = super::tests_support::sdk_fixture(&format!(
                    r#"{{"commits":[{{"id":"{BASE}","parent_ids":[]}}]}}"#
                ));
                answer(&mut stream, 200, &history);
            } else {
                let project = super::tests_support::sdk_fixture(&format!(
                    r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE",
                        "active_mutex":{{"user":"{ME}"}}}}"#
                ));
                answer(&mut stream, 200, &project);
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

/// Every GET under `/api/v2/projects/`: the open's history read and download.
fn project_reads(line: &str) -> bool {
    line.starts_with("GET /api/v2/projects/")
}

fn sign_in(state: &AppState, endpoint: &str) {
    let mut session = state.speleodb_session_lock().expect("session lock");
    session.endpoint = Some(endpoint.to_owned());
    session.email = Some(ME.to_owned());
    session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token("tok");
    session.rehydrate_attempted = true;
}

/// A cached copy of [`ID`] under `root`, linked to `endpoint` at [`BASE`].
fn seed(root: &Path, endpoint: &str, dirty: bool) -> PathBuf {
    let path = cache_path_in(root, ID);
    write_v2(
        &path,
        "Cached cave",
        Some(&SpeleoLink {
            endpoint: endpoint.to_owned(),
            project_id: ID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some(BASE.to_owned()),
            has_unpushed_edits: dirty,
            last_edit_at: dirty.then(|| "2026-11-21T10:00:00Z".to_owned()),
            last_verify_at: None,
            undo_mode: None,
        }),
    );
    path
}

fn owner_record(copy: &Path) -> Option<Vec<u8>> {
    std::fs::read(copy.with_file_name("owner.json")).ok()
}

/// A refused open, reduced to what the frontend routes on.
#[derive(Debug)]
struct Refusal {
    action: SpeleoDbAction,
    transport: Option<TransportCause>,
}

fn open(
    state: &AppState,
    copy: &Path,
    policy: LocalEditsPolicy,
) -> Result<SpeleoOpenResult, Refusal> {
    state
        .speleodb_pull_project_to(copy, ID.to_owned(), "Remote".to_owned(), None, policy)
        .map_err(|e| Refusal {
            action: e.action,
            transport: e.transport,
        })
}

#[test]
fn a_clean_copy_opens_offline_when_every_read_loses_its_reply() {
    // The fuzzer's trace (seed 900366, step 10): every GET under
    // /api/v2/projects/ dropped before any reply refused the open with Retry
    // ("Network error talking to SpeleoDB: error sending request") instead of
    // opening the cached copy, as a refused connection or a timeout does.
    for lost in Lost::NO_REPLY {
        crate::test_helpers::with_isolated_config(|| {
            let (url, seen) = server(lost, project_reads);
            let temp = tempfile::tempdir().expect("temp");
            let copy = seed(temp.path(), &url, false);
            let state = AppState::new().expect("state");
            sign_in(&state, &url);
            state.speleodb_record_cache_owner(&copy, &url, ME, "Remote");
            let (bytes, owner) = (std::fs::read(&copy).expect("copy"), owner_record(&copy));

            let opened = open(&state, &copy, LocalEditsPolicy::Protect)
                .unwrap_or_else(|e| panic!("{lost:?}: the copy opens offline, got {e:?}"));
            assert_eq!(opened.offline, Some(true), "{lost:?}");
            assert_eq!(opened.has_unpushed_edits, Some(false), "{lost:?}");
            assert_eq!(opened.read_only, None, "{lost:?}");
            assert_eq!(opened.other_owner, None, "{lost:?}");
            assert_eq!(opened.local_path, copy.to_string_lossy(), "{lost:?}");
            assert_eq!(
                std::fs::read(&copy).expect("copy"),
                bytes,
                "{lost:?}: untouched"
            );
            assert_eq!(
                owner_record(&copy),
                owner,
                "{lost:?}: an offline open records no owner"
            );
            let seen = seen.lock().expect("log");
            assert!(
                seen.iter().any(|line| line.contains("/download/")),
                "{lost:?}: the download was tried: {seen:?}"
            );
            assert!(
                seen.iter().all(|line| project_reads(line)),
                "{lost:?}: {seen:?}"
            );
        });
    }
}

#[test]
fn un_uploaded_work_opens_offline_still_marked_as_not_uploaded() {
    // The fallback opens the copy as it is: this account's un-uploaded edits are
    // reported as such (the UI keeps them marked, never as the server's state),
    // and another account's open read-only, naming whose they are.
    for lost in Lost::NO_REPLY {
        crate::test_helpers::with_isolated_config(|| {
            let (url, _) = server(lost, project_reads);
            let temp = tempfile::tempdir().expect("temp");
            let copy = seed(temp.path(), &url, true);
            let state = AppState::new().expect("state");
            sign_in(&state, &url);
            state.speleodb_record_cache_owner(&copy, &url, ME, "Remote");
            let mine = open(&state, &copy, LocalEditsPolicy::Protect).expect("offline open");
            assert_eq!(mine.offline, Some(true), "{lost:?}");
            assert_eq!(mine.has_unpushed_edits, Some(true), "{lost:?}");
            assert_eq!(mine.read_only, None, "{lost:?}");

            state.speleodb_record_cache_owner(&copy, &url, "someone@example.com", "Remote");
            let theirs = open(&state, &copy, LocalEditsPolicy::Protect).expect("offline open");
            assert_eq!(theirs.offline, Some(true), "{lost:?}");
            assert_eq!(theirs.has_unpushed_edits, Some(true), "{lost:?}");
            assert_eq!(
                theirs.read_only,
                Some(SpeleoReadOnlyOpen::OtherAccount),
                "{lost:?}"
            );
            assert_eq!(theirs.other_owner.as_deref(), Some("someone@example.com"));
            assert!(link_on_disk(&copy).expect("linked").has_unpushed_edits);
        });
    }
}

#[test]
fn a_reply_the_app_cannot_read_or_a_discard_never_falls_back() {
    crate::test_helpers::with_isolated_config(|| {
        // The server answered, just not in HTTP: it was reached, so this is no
        // reason to open the copy instead.
        let (url, _) = server(Lost::NotHttp, project_reads);
        let temp = tempfile::tempdir().expect("temp");
        let copy = seed(temp.path(), &url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &url);
        let refused = open(&state, &copy, LocalEditsPolicy::Protect).expect_err("no fallback");
        assert_eq!(refused.action, SpeleoDbAction::Retry, "{refused:?}");
        assert_eq!(
            refused.transport,
            Some(TransportCause::Unknown),
            "{refused:?}"
        );
    });
    for lost in Lost::NO_REPLY {
        crate::test_helpers::with_isolated_config(|| {
            // Take the server copy and discard local work: with no server copy to
            // take, nothing is opened and the local work stays.
            let (url, _) = server(lost, project_reads);
            let temp = tempfile::tempdir().expect("temp");
            let copy = seed(temp.path(), &url, true);
            let state = AppState::new().expect("state");
            sign_in(&state, &url);
            let refused = open(&state, &copy, LocalEditsPolicy::Discard).expect_err("no fallback");
            assert_eq!(refused.transport, Some(TransportCause::Dropped), "{lost:?}");
            assert!(link_on_disk(&copy).expect("linked").has_unpushed_edits);
        });
    }
}

/// Every upload.
fn uploads(line: &str) -> bool {
    line.contains("/upload/")
}

#[test]
fn an_upload_whose_reply_is_lost_stays_unconfirmed_and_is_sent_once() {
    // A write classified alike (`Dropped`) is still an outcome nobody knows: it
    // may have landed. The push stays dirty at BASE, keeps the lock and the
    // history, and is not replayed.
    for lost in [Lost::Closed, Lost::Reset, Lost::BodyCutShort] {
        crate::test_helpers::with_isolated_config(|| {
            let temp = tempfile::tempdir().expect("temp");
            let _cache = TestCacheRoot::set(temp.path());
            let (url, seen) = server(lost, uploads);
            let path = cache_path_in(temp.path(), ID);
            let link = SpeleoLink {
                endpoint: url.clone(),
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
            assert!(state.open_project(path.clone()).success);
            sign_in(&state, &url);
            state.set_edit_enabled(true);
            state.speleodb_note_lock_acquired(Some(ID));
            let line_id = state
                .get_project_state()
                .expect("open")
                .lines()
                .first()
                .expect("the fixture line")
                .id;
            let edit = state.execute(Command::ReverseLine {
                line_id,
                swap_left_right: false,
            });
            assert!(edit.success, "{:?}", edit.error);

            let pushed = state
                .speleodb_push_only(Some("Synthetic push".to_owned()))
                .expect("a push whose reply is lost is not an error");
            assert!(
                matches!(pushed, PushResult::PushUnconfirmed { .. }),
                "{lost:?}: {pushed:?}"
            );
            let on_disk = link_on_disk(&path).expect("linked");
            assert!(on_disk.has_unpushed_edits, "{lost:?}: still un-uploaded");
            assert_eq!(on_disk.base_commit_sha.as_deref(), Some(BASE), "{lost:?}");
            assert!(state.edit_enabled(), "{lost:?}: the lock is kept");
            assert!(state.can_undo(), "{lost:?}: and the history");
            let seen = seen.lock().expect("log");
            assert_eq!(
                seen.iter().filter(|line| uploads(line)).count(),
                1,
                "{lost:?}: never replayed: {seen:?}"
            );
        });
    }
}

#[test]
fn the_automatic_start_editing_after_an_offline_open_names_the_lost_reply() {
    // After an offline open the frontend's automatic Start Editing runs against the
    // same server, and its reads lose their replies the same way. The refusal
    // keeps the raw diagnostic in `message` ("Network error talking to SpeleoDB:
    // error sending request", for the log) and the cause in `transport`, which is
    // all the frontend words the notice from: the lost reply (`Dropped`), never
    // the catch-all. The copy stays read-only.
    for lost in Lost::NO_REPLY {
        crate::test_helpers::with_isolated_config(|| {
            let temp = tempfile::tempdir().expect("temp");
            let _cache = TestCacheRoot::set(temp.path());
            let (url, _) = server(lost, project_reads);
            let copy = seed(temp.path(), &url, false);
            let state = AppState::new().expect("state");
            sign_in(&state, &url);
            state.speleodb_record_cache_owner(&copy, &url, ME, "Remote");
            let opened = open(&state, &copy, LocalEditsPolicy::Protect).expect("offline open");
            assert_eq!(opened.offline, Some(true), "{lost:?}");
            assert!(state.open_project(copy).success, "{lost:?}");

            let refused = state
                .speleodb_start_editing()
                .expect_err("no reply, no edit session");
            assert_eq!(
                refused.action,
                SpeleoDbAction::Retry,
                "{lost:?}: {refused:?}"
            );
            assert_eq!(
                refused.transport,
                Some(TransportCause::Dropped),
                "{lost:?}: {refused:?}"
            );
            assert!(
                refused
                    .message
                    .starts_with("Network error talking to SpeleoDB"),
                "{lost:?}: the raw diagnostic stays for the log: {refused:?}"
            );
            assert!(!state.edit_enabled(), "{lost:?}: still read-only");
        });
    }
}

/// A loopback SpeleoDB that keeps the project's lock: an acquire takes it for
/// [`ME`], a release frees it, and the project read names its holder. While
/// `lose_gets` is set it loses the reply to every GET under `/api/v2/projects/`
/// (closed before any reply) and still answers every POST: the fault under which
/// an acquire lands while every read around it is lost.
struct LockServer {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
    held: Arc<std::sync::atomic::AtomicBool>,
    lose_gets: Arc<std::sync::atomic::AtomicBool>,
}

impl LockServer {
    fn start() -> Self {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::new(AtomicBool::new(false));
        let lose_gets = Arc::new(AtomicBool::new(true));
        let (log, lock, lossy) = (Arc::clone(&seen), Arc::clone(&held), Arc::clone(&lose_gets));
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let line = read_request(&mut stream, false);
                log.lock().expect("log").push(line.clone());
                let project = |held: bool| {
                    let mutex = if held {
                        format!(r#"{{"user":"{ME}"}}"#)
                    } else {
                        "null".to_owned()
                    };
                    super::tests_support::sdk_fixture(&format!(
                        r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE",
                            "active_mutex":{mutex}}}"#
                    ))
                };
                if line.starts_with("GET /api/v2/projects/") && lossy.load(SeqCst) {
                    lose_reply(stream, Lost::Closed, &line);
                } else if line.contains("/acquire/") {
                    lock.store(true, SeqCst);
                    answer(&mut stream, 200, &project(true));
                } else if line.contains("/release/") {
                    lock.store(false, SeqCst);
                    answer(&mut stream, 200, &project(false));
                } else if line.contains("/user/") {
                    let me = super::tests_support::sdk_fixture(&format!(r#"{{"email":"{ME}"}}"#));
                    answer(&mut stream, 200, &me);
                } else if line.contains("/revisions/") {
                    let history = super::tests_support::sdk_fixture(&format!(
                        r#"{{"commits":[{{"id":"{BASE}","parent_ids":[]}}]}}"#
                    ));
                    answer(&mut stream, 200, &history);
                } else {
                    answer(&mut stream, 200, &project(lock.load(SeqCst)));
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            seen,
            held,
            lose_gets,
        }
    }
    fn held(&self) -> bool {
        self.held.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn answer_again(&self) {
        self.lose_gets
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
    fn releases(&self) -> usize {
        let seen = self.seen.lock().expect("log");
        seen.iter()
            .filter(|line| line.contains("/release/"))
            .count()
    }
}

#[test]
fn a_start_editing_whose_acquire_landed_leaves_a_pointer_to_the_lock() {
    // F1-transport-1. Start Editing over a clean copy while every GET loses its
    // reply and POSTs are answered: the holder probe fails, the acquire lands,
    // the HEAD read fails, and the unwind's holder-checked release cannot read
    // the holder, so it sends nothing. The lock was then held on SpeleoDB with
    // nothing in this process pointing at it: no close or cleanup could ever
    // hand it back, and collaborators were locked out until the next sign-in
    // scan or a release on the website. Now the unwind leaves an unbound claim,
    // and the close hands the lock back once SpeleoDB answers again.
    // Injection: the unwind leaving no claim when its release cannot tell
    // (`speleodb_unwind_release`) fails the claim and the release assertions.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let server = LockServer::start();
        let copy = seed(temp.path(), &server.url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &server.url);
        state.speleodb_record_cache_owner(&copy, &server.url, ME, "Remote");
        let opened = open(&state, &copy, LocalEditsPolicy::Protect).expect("offline open");
        assert_eq!(opened.offline, Some(true));
        assert!(state.open_project(copy).success);

        let refused = state
            .speleodb_start_editing()
            .expect_err("no reply, no edit session");
        assert_eq!(
            refused.transport,
            Some(TransportCause::Dropped),
            "{refused:?}"
        );
        assert!(server.held(), "the acquire landed");
        assert_eq!(
            server.releases(),
            0,
            "the unwind could not tell, so sent nothing"
        );
        assert!(!state.edit_enabled(), "still read-only");
        assert!(
            state.speleodb_holds_claim(ID),
            "the lock this call took has a local pointer"
        );
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "a pointer, not an edit session"
        );
        assert_eq!(
            refused.lock_may_be_held,
            Some(true),
            "the refusal says the lock may be held, for the read-only notice (demo item 3)"
        );

        server.answer_again();
        let closed = state.speleodb_release_held_lock_on_close();
        assert!(closed.is_ok(), "{closed:?}");
        assert!(!server.held(), "the close handed the lock back");
        assert_eq!(server.releases(), 1);
        assert!(!state.speleodb_holds_claim(ID), "and ended the claim");
    });
}

/// A loopback SpeleoDB whose history read answers with `tip` on top of [`BASE`]
/// (or `tip` alone when it is BASE) and that cuts every download short: a 200
/// promising 4 MB, a few bytes, then a close.
fn server_with_tip_and_cut_downloads(tip: &'static str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let line = read_request(&mut stream, false);
            if line.contains("/revisions/") {
                let commits = if tip == BASE {
                    format!(r#"[{{"id":"{BASE}","parent_ids":[]}}]"#)
                } else {
                    format!(
                        r#"[{{"id":"{tip}","parent_ids":["{BASE}"]}},{{"id":"{BASE}","parent_ids":[]}}]"#
                    )
                };
                let history =
                    super::tests_support::sdk_fixture(&format!(r#"{{"commits":{commits}}}"#));
                answer(&mut stream, 200, &history);
            } else if line.contains("/download/") {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: 4000000\r\nConnection: close\r\n\r\nPK\x03\x04partial"
                );
                let _ = stream.flush();
                std::thread::sleep(std::time::Duration::from_millis(30));
            } else if line.contains("/user/") {
                let me = super::tests_support::sdk_fixture(&format!(r#"{{"email":"{ME}"}}"#));
                answer(&mut stream, 200, &me);
            } else {
                answer(&mut stream, 404, r#"{"detail":"Not found."}"#);
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn an_open_whose_download_alone_was_lost_says_what_the_history_told_it() {
    // F1-transport-5. The history read answered, so SpeleoDB was reached and
    // the app knows its latest revision; only the download was cut. The open
    // still falls back to the copy, but says whether SpeleoDB has a newer
    // revision than the copy, instead of calling SpeleoDB unreachable. When
    // every read lost its reply the answer stays unknown (the tests above).
    // Injection: `offline_open_result` given no tip fails both cases.
    const NEWER: &str = "def5678";
    for (tip, newer) in [(NEWER, true), (BASE, false)] {
        crate::test_helpers::with_isolated_config(|| {
            let url = server_with_tip_and_cut_downloads(tip);
            let temp = tempfile::tempdir().expect("temp");
            let copy = seed(temp.path(), &url, false);
            let state = AppState::new().expect("state");
            sign_in(&state, &url);
            state.speleodb_record_cache_owner(&copy, &url, ME, "Remote");
            let opened = open(&state, &copy, LocalEditsPolicy::Protect).expect("offline open");
            assert_eq!(opened.offline, Some(true), "{tip}");
            assert_eq!(opened.newer_on_server, Some(newer), "{tip}");
            let link = link_on_disk(&copy).expect("linked");
            assert_eq!(
                link.base_commit_sha.as_deref(),
                Some(BASE),
                "{tip}: the copy is untouched"
            );
        });
    }
    crate::test_helpers::with_isolated_config(|| {
        let (url, _) = server(Lost::Closed, project_reads);
        let temp = tempfile::tempdir().expect("temp");
        let copy = seed(temp.path(), &url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &url);
        let opened = open(&state, &copy, LocalEditsPolicy::Protect).expect("offline open");
        assert_eq!(opened.newer_on_server, None, "nothing answered");
    });
}

#[test]
fn a_start_editing_that_never_connected_leaves_no_pointer_to_a_lock() {
    // Demo item 3. A refused connection sends nothing, so the lock request cannot
    // have reached SpeleoDB. Every transport failure of the acquire used to read as
    // "may have landed": the holder read was refused the same way, an unbound
    // claim was left, and the next Close asked "Couldn't hand back the lock" about
    // a lock request that never left this computer, after a notice that said only
    // "Could not connect". Now an acquire that never connected leaves nothing.
    // Injection: `acquire_outcome_unknown` matching every `Transport` again fails
    // the claim and the close assertions.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let url = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().expect("addr").port()
        );
        drop(listener);
        let copy = seed(temp.path(), &url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &url);
        state.speleodb_record_cache_owner(&copy, &url, ME, "Remote");
        let opened = open(&state, &copy, LocalEditsPolicy::Protect).expect("offline open");
        assert_eq!(opened.offline, Some(true));
        assert!(state.open_project(copy).success);

        let refused = state
            .speleodb_start_editing()
            .expect_err("nothing listens, no edit session");
        assert_eq!(
            refused.transport,
            Some(TransportCause::Refused),
            "{refused:?}"
        );
        assert!(
            !state.speleodb_holds_claim(ID),
            "nothing was sent, so there is no lock to point at"
        );
        assert_eq!(
            refused.lock_may_be_held, None,
            "and nothing to say about one"
        );
        let closed = state.speleodb_release_held_lock_on_close();
        assert!(
            matches!(closed, Ok(None)),
            "the close has nothing to hand back, so it asks nothing: {closed:?}"
        );
    });
}

/// A loopback SpeleoDB on which nobody holds the lock and every acquire loses its
/// reply without landing: the acquire never takes the lock, and the project read
/// says so.
fn server_losing_unlanded_acquires() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let line = read_request(&mut stream, false);
            if line.contains("/acquire/") {
                lose_reply(stream, Lost::Closed, &line);
            } else if line.contains("/user/") {
                let me = super::tests_support::sdk_fixture(&format!(r#"{{"email":"{ME}"}}"#));
                answer(&mut stream, 200, &me);
            } else if line.contains("/revisions/") {
                let history = super::tests_support::sdk_fixture(&format!(
                    r#"{{"commits":[{{"id":"{BASE}","parent_ids":[]}}]}}"#
                ));
                answer(&mut stream, 200, &history);
            } else {
                let project = super::tests_support::sdk_fixture(&format!(
                    r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE","active_mutex":null}}"#
                ));
                answer(&mut stream, 200, &project);
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn a_lost_lock_request_says_the_lock_may_be_held_only_while_nothing_can_tell() {
    // Demo item 3. The read-only notice after a failed Start Editing says the
    // lock may be held exactly when this process keeps a pointer to a lock that
    // may be ours, which the next close will hand back or ask about. An acquire
    // whose reply was lost and whose holder nobody could read leaves one; an
    // acquire whose reply was lost but which SpeleoDB says did not land (nobody
    // holds the lock) leaves none, and the refusal says nothing about a lock.
    // Injection: the flag set from the transport cause alone (`Dropped`) fails
    // the second case; the flag never set fails the first.
    crate::test_helpers::with_isolated_config(|| {
        let (url, _) = server(Lost::Closed, |line| {
            line.contains("/acquire/") || project_reads(line)
        });
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let copy = seed(temp.path(), &url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &url);
        state.speleodb_record_cache_owner(&copy, &url, ME, "Remote");
        open(&state, &copy, LocalEditsPolicy::Protect).expect("offline open");
        assert!(state.open_project(copy).success);
        let refused = state.speleodb_start_editing().expect_err("no reply");
        assert_eq!(
            refused.transport,
            Some(TransportCause::Dropped),
            "{refused:?}"
        );
        assert!(
            state.speleodb_holds_claim(ID),
            "a pointer to a lock that may be ours"
        );
        assert_eq!(refused.lock_may_be_held, Some(true), "{refused:?}");
    });
    crate::test_helpers::with_isolated_config(|| {
        let url = server_losing_unlanded_acquires();
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let copy = seed(temp.path(), &url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &url);
        state.speleodb_record_cache_owner(&copy, &url, ME, "Remote");
        assert!(state.open_project(copy).success);
        let refused = state.speleodb_start_editing().expect_err("no reply");
        assert_eq!(
            refused.transport,
            Some(TransportCause::Dropped),
            "{refused:?}"
        );
        assert!(
            !state.speleodb_holds_claim(ID),
            "SpeleoDB says nobody holds it"
        );
        assert_eq!(refused.lock_may_be_held, None, "{refused:?}");
        assert!(matches!(
            state.speleodb_release_held_lock_on_close(),
            Ok(None)
        ));
    });
}
