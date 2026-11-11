//! A revision View / Restore whose reply lands after the user moved to another
//! project leaves that project alone; an unraced one still swaps the tree.
//!
//! Synthetic throughout: a loopback SpeleoDB stub and databases built here. This
//! separate process owns its temporary app dirs (the cache path is global).
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "synthetic HTTP and SQLite test setup"
)]
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, OnceLock};

use serde_json::json;
use stalagmight_lib::core::events::EventSink;
use stalagmight_lib::models::{Project, SpeleoLink};
use stalagmight_lib::speleodb::SpeleoDbAction;
use stalagmight_lib::{core::AppState, persistence::Database};

/// The revision being viewed / restored.
const REV: &str = "1111111111111111111111111111111111111111";
/// The server tip a Restore stamps as BASE.
const TIP: &str = "2222222222222222222222222222222222222222";

/// The process-wide cache root: `init_paths` is first-call-wins, so every test
/// in this binary shares the one it set.
fn cache_root() -> PathBuf {
    static DIRS: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dirs = DIRS.get_or_init(|| {
        let temp = tempfile::tempdir().expect("temporary app directories");
        stalagmight_lib::models::app_dirs::init_paths(
            temp.path().join("data"),
            temp.path().join("config"),
            temp.path().join("cache"),
            None,
        );
        temp
    });
    let _ = dirs;
    stalagmight_lib::models::app_dirs::cache_dir()
        .expect("cache dir")
        .join("speleodb")
}

/// The load emits progress events; nothing listens here.
struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

/// Hold the revision download until the test says so.
struct Gate {
    seen: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = stream.read(&mut buffer).expect("request bytes");
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") {
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

/// A loopback SpeleoDB that serves `revision` for every download, a history
/// whose tip is [`TIP`], and a user for the sign-in. Stops when dropped.
struct Stub {
    base: String,
    stop: Arc<AtomicBool>,
    /// How many release requests reached the server.
    releases: Arc<AtomicUsize>,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve(project_id: &str, revision: Vec<u8>, gate: Option<Gate>) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let releases = Arc::new(AtomicUsize::new(0));
    let server_releases = Arc::clone(&releases);
    let project = json!({"id":project_id,"name":"P","description":"Synthetic","country":"US",
        "type":"STALAGMIGHT","visibility":"PRIVATE","permission":"READ_AND_WRITE","color":"#123456",
        "commit_count":2,"created_by":"test@example.com","creation_date":"2026-01-01T00:00:00Z",
        "modified_date":"2026-01-01T00:00:00Z","exclude_geojson":false,"fork_from":null,"is_active":true,
        "active_mutex":null,"latest_commit":null});
    let probe_route = format!("/{project_id}/ HTTP/1.1");
    std::thread::spawn(move || {
        // The server's mutex: taken by an acquire, freed by a release, and what
        // every project read (Start Editing's probe, a release's holder check)
        // reports.
        let mut held = false;
        let with_holder = |held: bool| {
            let mut body = project.clone();
            if held {
                body["active_mutex"] = json!("test@example.com");
            }
            body
        };
        while !server_stop.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            stream.set_nonblocking(false).expect("blocking stream");
            let text = read_request(&mut stream);
            let route = text.lines().next().unwrap_or_default().to_owned();
            if route.contains("/download/stalagmight") {
                if let Some(gate) = &gate {
                    gate.seen.send(()).expect("download seen");
                    gate.release.recv().expect("download released");
                }
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n", revision.len())
                    .expect("head");
                stream.write_all(&revision).expect("body");
                continue;
            }
            // Start Editing's probe and acquire: the lock is free until the acquire
            // takes it for the signed-in account.
            let (code, body) = if route.contains("/acquire/") {
                held = true;
                (200, with_holder(held))
            } else if route.contains("/release/") {
                held = false;
                server_releases.fetch_add(1, Ordering::SeqCst);
                (200, with_holder(held))
            } else if route.starts_with("GET") && route.ends_with(&probe_route) {
                (200, with_holder(held))
            } else if route.contains("/user/") {
                (
                    200,
                    json!({"email":"test@example.com","name":"Test","country":"US",
                        "email_on_projects_updates":false,"email_on_speleodb_updates":false}),
                )
            } else if route.contains("/revisions/") {
                let commit = |id: &str, parents: Vec<&str>| {
                    json!({"id":id,"parent_ids":parents,"author_email":"test@example.com",
                        "author_name":"Test","authored_date":"2026-01-01T00:00:00Z","dt_since":"now",
                        "message":"Synthetic","formats":[],"tree":[],"url":null})
                };
                (
                    200,
                    json!({"project":project,"commits":[commit(TIP, vec![REV]), commit(REV, vec![])]}),
                )
            } else {
                (404, json!({"detail":"Not found"}))
            };
            let body = body.to_string();
            write!(stream, "HTTP/1.1 {code} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len())
                .expect("reply");
        }
    });
    Stub {
        base,
        stop,
        releases,
    }
}

/// The bytes the server hands out for [`REV`].
fn revision_bytes(dir: &Path) -> Vec<u8> {
    let path = dir.join("revision.stalx");
    Database::open(&path)
        .expect("revision")
        .save_project(&Project::new("Revision X".into(), None))
        .expect("save");
    std::fs::read(&path).expect("bytes")
}

/// A signed-in state with linked project `P` open from its cache copy, edit gate
/// open (as after Start Editing). Returns the cache path.
fn open_p(state: &AppState, project_id: &str, base: &str) -> PathBuf {
    // Every case serves its own stub, and the config the cases share may still
    // name an earlier one. A connected session is bound to its server, so start
    // disconnected rather than switch servers under a session.
    state.speleodb_logout().expect("start disconnected");
    state
        .speleodb_login(
            Some(base.to_owned()),
            None,
            None,
            None,
            Some("synthetic-token".into()),
        )
        .expect("sign in");
    let cache = cache_root().join(project_id).join("project.stalx");
    std::fs::create_dir_all(cache.parent().expect("parent")).expect("mkdir");
    {
        let mut db = Database::open(&cache).expect("cache");
        db.save_project(&Project::new("P".into(), None))
            .expect("save");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: base.to_owned(),
            project_id: project_id.to_owned(),
            project_name: "P".into(),
            base_commit_sha: Some(TIP.into()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        }))
        .expect("link");
    }
    let mut p = Project::new("P".into(), None);
    p.file_path = Some(cache.clone());
    state.set_project_for_tests(p);
    state.set_database_for_tests(Database::open_existing(&cache).expect("open P"));
    // The edit gate opens only through a lock claim, so take the lock the way the
    // app does: BASE is the server tip, so the reconcile is UpToDate.
    assert!(
        matches!(
            state.speleodb_start_editing(),
            Ok(stalagmight_lib::speleodb::StartEditingResult::Editing)
        ),
        "the fixture starts editing P"
    );
    cache
}

fn open_name(state: &AppState) -> Option<String> {
    state.get_project_state().map(|p| p.name)
}

/// Whether the edit gate is open, probed through a no-op settings write (it is
/// refused while the gate is shut).
fn gate_open(state: &AppState) -> bool {
    let project = state.get_project_state().expect("open project");
    state
        .update_project_settings(project.name, project.validation)
        .success
}

fn swap(
    state: &AppState,
    project_id: &str,
    restore: bool,
) -> Result<String, (SpeleoDbAction, String)> {
    state
        .speleodb_restore_revision(
            project_id.to_owned(),
            "P".into(),
            REV.into(),
            restore,
            restore, // A Restore here is the confirmed discard; a View never waives.
            false,
            false,
            false,
        )
        .map(|result| result.local_path)
        .map_err(|e| (e.action, e.message))
}

/// Bug: the swap checked the open project only on entry, then closed WHATEVER
/// was open when its download finished - unsaved edits and all - and loaded
/// its revision in its place. Now the reply finds the session moved, refuses,
/// and leaves the project the user switched to (or reopened) exactly as it is.
#[test]
fn a_late_revision_reply_never_touches_the_project_open_now() {
    for (case, restore, reopen_p) in [
        ("view, switched to Q", false, false),
        ("restore, switched to Q", true, false),
        ("view, switched to Q and back to P", false, true),
    ] {
        let id = match case {
            "view, switched to Q" => "11111111-1111-1111-1111-111111111111",
            "restore, switched to Q" => "11111111-1111-1111-1111-111111111112",
            _ => "11111111-1111-1111-1111-111111111113",
        };
        let temp = tempfile::tempdir().expect("temp");
        let (seen_tx, seen) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let stub = serve(
            id,
            revision_bytes(temp.path()),
            Some(Gate {
                seen: seen_tx,
                release: release_rx,
            }),
        );
        let state = AppState::new().expect("state");
        state.set_event_sink(Arc::new(NullSink));
        let cache = open_p(&state, id, &stub.base);

        std::thread::scope(|scope| {
            let state = &state;
            let pending = scope.spawn(move || swap(state, id, restore));
            seen.recv().expect("download in flight");

            // The user opens a local project Q and edits it; the edit is only in
            // memory (autosave has not run).
            let q_path = temp.path().join("q.stalx");
            Database::open(&q_path)
                .expect("q")
                .save_project(&Project::new("Q saved".into(), None))
                .expect("save q");
            state.do_load_project_full(q_path.clone()).expect("open Q");
            let validation = state.get_project_state().expect("Q").validation;
            assert!(
                state
                    .update_project_settings("Q with unsaved edit".into(), validation)
                    .success,
                "{case}: edit Q"
            );
            let expected_open = if reopen_p {
                // ...and goes back to P, which is a NEW session of the same file.
                state.do_load_project_full(cache.clone()).expect("reopen P");
                "P".to_owned()
            } else {
                "Q with unsaved edit".to_owned()
            };

            // Taken after any reopen, whose own load-time writes are not the swap's.
            let cache_before = std::fs::read(&cache).expect("P bytes");
            release.send(()).expect("release the download");
            let (action, message) = pending
                .join()
                .expect("join")
                .expect_err("a late reply must be refused");
            assert_eq!(action, SpeleoDbAction::Retry, "{case}: {message}");
            assert!(
                message.contains("changed while this request was in flight"),
                "{case}: {message}"
            );
            assert_eq!(
                open_name(state).as_deref(),
                Some(expected_open.as_str()),
                "{case}: the project open now stays open, edits intact"
            );
            if !reopen_p {
                assert_eq!(
                    state.get_project_state().expect("Q").file_path.as_deref(),
                    Some(q_path.as_path())
                );
                assert!(gate_open(state), "{case}: Q's edit gate is untouched");
            }
            assert_eq!(
                Database::open_existing(&q_path)
                    .and_then(|db| db.load_project())
                    .expect("Q on disk")
                    .name,
                "Q saved",
                "{case}: nothing saved or replaced Q"
            );
            assert!(
                std::fs::read(&cache).expect("P bytes") == cache_before,
                "{case}: the refused swap replaced nothing"
            );
            let cached = Database::open_existing(&cache).expect("P on disk");
            assert_eq!(cached.load_project().expect("project").name, "P");
            assert_eq!(
                cached
                    .load_speleo_link()
                    .expect("link")
                    .expect("linked")
                    .base_commit_sha
                    .as_deref(),
                Some(TIP),
                "{case}: P's cache still holds P, not the revision"
            );
        });
        drop(stub);
    }
}

/// The unraced swap still does its job: a View opens the revision read-only and
/// clean, a Restore opens it editable and un-pushed on top of the server tip.
#[test]
fn an_unraced_view_and_restore_still_swap_the_open_tree() {
    for (id, restore) in [
        ("22222222-2222-2222-2222-222222222221", false),
        ("22222222-2222-2222-2222-222222222222", true),
    ] {
        let temp = tempfile::tempdir().expect("temp");
        let stub = serve(id, revision_bytes(temp.path()), None);
        let state = AppState::new().expect("state");
        state.set_event_sink(Arc::new(NullSink));
        let cache = open_p(&state, id, &stub.base);

        let local_path = swap(&state, id, restore)
            .unwrap_or_else(|(_, message)| panic!("restore={restore}: {message}"));

        assert_eq!(Path::new(&local_path), cache.as_path());
        let open = state.get_project_state().expect("open");
        assert_eq!(open.name, "Revision X", "restore={restore}");
        assert_eq!(open.file_path.as_deref(), Some(cache.as_path()));
        assert_eq!(
            gate_open(&state),
            restore,
            "a View is read-only, a Restore keeps editing"
        );
        let db = Database::open_existing(&cache).expect("installed");
        assert_eq!(db.load_project().expect("project").name, "Revision X");
        let link = db.load_speleo_link().expect("link").expect("linked");
        assert_eq!(link.project_id, id);
        if restore {
            assert_eq!(link.base_commit_sha.as_deref(), Some(TIP));
            assert!(link.has_unpushed_edits, "a restore is an un-pushed edit");
        } else {
            assert_eq!(link.base_commit_sha.as_deref(), Some(REV));
            assert!(!link.has_unpushed_edits, "a View is clean");
        }
        drop(stub);
    }
}

/// data#3 regression: Take remote releases the lock and reloads the server tip.
/// The reload used to copy the old edit gate across, so the project came back
/// "editing" with no lock held - edits dirtied the link while anyone else could
/// lock and push. The gate is the lock claim now: Take remote hands the lock back
/// (the server still says it is ours, so the release is sent) and the project it
/// reopens is read-only.
#[test]
fn take_remote_reopens_the_server_version_read_only() {
    let id = "33333333-3333-3333-3333-333333333331";
    let temp = tempfile::tempdir().expect("temp");
    let stub = serve(id, revision_bytes(temp.path()), None);
    let state = AppState::new().expect("state");
    state.set_event_sink(Arc::new(NullSink));
    let cache = open_p(&state, id, &stub.base);
    assert!(gate_open(&state), "precondition: an edit session on P");

    let local_path = state.speleodb_take_remote().expect("take remote");

    assert_eq!(Path::new(&local_path), cache.as_path());
    assert_eq!(
        open_name(&state).as_deref(),
        Some("Revision X"),
        "the server version replaced the local copy"
    );
    assert_eq!(
        stub.releases.load(Ordering::SeqCst),
        1,
        "the held lock was handed back"
    );
    assert!(
        !gate_open(&state),
        "with the lock released, the reloaded project must be read-only"
    );
    drop(stub);
}
