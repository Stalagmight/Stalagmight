//! A revision View / Restore whose reply lands after the user moved to another
//! project leaves that project alone; an unraced one still swaps the tree.
//!
//! Synthetic throughout: a loopback SpeleoDB stub and version 2 files written
//! here. This separate process owns its temporary app dirs (the cache path is
//! global).
//!
//! Every case installs the downloaded revision through the SpeleoDB sync
//! package's install path (`stamp_downloaded_link`, `stage_download`,
//! `open_project_download`), so each is ignored until that package lands.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "synthetic HTTP and file test setup"
)]
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, OnceLock};

use serde_json::json;
use stalagmight_lib::core::events::EventSink;
use stalagmight_lib::core::{AppState, Command};
use stalagmight_lib::models::survey::Project;
use stalagmight_lib::models::SpeleoLink;
use stalagmight_lib::persistence::stalx_v2::{self, Extras, LocalSync, Mode, SpeleoDbLink};
use stalagmight_lib::speleodb::SpeleoDbAction;

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

/// Hold one request until the test says so: the first request on `route` once
/// `armed` is set (it disarms itself there, so later ones pass).
struct Gate {
    route: &'static str,
    armed: Arc<AtomicBool>,
    seen: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl Gate {
    /// A gate on the revision download, armed from the start.
    fn download(seen: mpsc::Sender<()>, release: mpsc::Receiver<()>) -> Self {
        Self {
            route: "/download/stalagmight",
            armed: Arc::new(AtomicBool::new(true)),
            seen,
            release,
        }
    }

    /// Hold the request here if this is the one the gate waits for.
    fn hold(&self, route: &str) {
        if route.contains(self.route) && self.armed.swap(false, Ordering::SeqCst) {
            self.seen.send(()).expect("request seen");
            self.release.recv().expect("request released");
        }
    }
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
            if let Some(gate) = &gate {
                gate.hold(&route);
            }
            if route.contains("/download/stalagmight") {
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

/// A version 2 file of an empty project named `name`, carrying `link`.
fn v2_bytes(name: &str, link: Option<&SpeleoLink>) -> Vec<u8> {
    let mut extras = Extras::default();
    if let Some(link) = link {
        extras.speleodb = Some(SpeleoDbLink {
            endpoint: link.endpoint.clone(),
            project_id: link.project_id.clone(),
            project_name: link.project_name.clone(),
            undo_mode: link.undo_mode,
        });
        extras.local.speleodb = LocalSync {
            base_commit: link.base_commit_sha.clone(),
            unpushed_edits: link.has_unpushed_edits,
            ..LocalSync::default()
        };
    }
    stalx_v2::encode(&Project::new(name, None), &extras, Mode::Normal).expect("encode")
}

/// The project name and link a file on disk holds now.
fn on_disk(path: &Path) -> (String, Option<SpeleoDbLink>, LocalSync) {
    let bytes = std::fs::read(path).expect("bytes");
    let decoded =
        stalx_v2::decode(&bytes, &stalx_v2::DecodeOptions::default()).expect("a v2 project");
    (
        decoded.project.name,
        decoded.extras.speleodb,
        decoded.extras.local.speleodb,
    )
}

/// The bytes the server hands out for [`REV`] (a push carries no `local`).
fn revision_bytes(_dir: &Path) -> Vec<u8> {
    v2_bytes("Revision X", None)
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
    std::fs::write(
        &cache,
        v2_bytes(
            "P",
            Some(&SpeleoLink {
                endpoint: base.to_owned(),
                project_id: project_id.to_owned(),
                project_name: "P".into(),
                base_commit_sha: Some(TIP.into()),
                has_unpushed_edits: false,
                last_edit_at: None,
                last_verify_at: None,
                undo_mode: None,
            }),
        ),
    )
    .expect("write P");
    assert!(
        state
            .execute(Command::OpenProject {
                path: cache.clone()
            })
            .success,
        "open P"
    );
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
        .update_project_settings(project.name, project.settings.validation)
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
            Some(Gate::download(seen_tx, release_rx)),
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
            std::fs::write(&q_path, v2_bytes("Q saved", None)).expect("save q");
            state.do_load_project_full(q_path.clone()).expect("open Q");
            let validation = state.get_project_state().expect("Q").settings.validation;
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
                on_disk(&q_path).0,
                "Q saved",
                "{case}: nothing saved or replaced Q"
            );
            assert!(
                std::fs::read(&cache).expect("P bytes") == cache_before,
                "{case}: the refused swap replaced nothing"
            );
            let (name, link, sync) = on_disk(&cache);
            assert_eq!(name, "P");
            assert!(link.is_some(), "{case}: P's cache is still linked");
            assert_eq!(
                sync.base_commit.as_deref(),
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
        let (name, link, sync) = on_disk(&cache);
        assert_eq!(name, "Revision X");
        assert_eq!(link.expect("linked").project_id, id);
        if restore {
            assert_eq!(sync.base_commit.as_deref(), Some(TIP));
            assert!(sync.unpushed_edits, "a restore is an un-pushed edit");
        } else {
            assert_eq!(sync.base_commit.as_deref(), Some(REV));
            assert!(!sync.unpushed_edits, "a View is clean");
        }
        drop(stub);
    }
}

/// Take the SpeleoDB version keeps the lock and the edit session (demo item 2).
/// SpeleoDB's lock is the account's, so the same account signed in elsewhere
/// edits under it too: a release here used to leave that session editing with
/// no lock on the server. Taking the server's version needs no release, so the
/// server version replaces the copy and the session goes on editing it under
/// the lock it still holds. (data#3 still holds: the reload carries the edit
/// session only because its claim is still bound to it.)
/// Injection: put the release back into `speleodb_take_remote` and one release
/// is sent and the project comes back read-only.
#[test]
fn take_remote_keeps_the_lock_and_the_edit_session() {
    let id = "33333333-3333-3333-3333-333333333331";
    let temp = tempfile::tempdir().expect("temp");
    let stub = serve(id, revision_bytes(temp.path()), None);
    let state = AppState::new().expect("state");
    state.set_event_sink(Arc::new(NullSink));
    let cache = open_p(&state, id, &stub.base);
    assert!(gate_open(&state), "precondition: an edit session on P");

    let took = state.speleodb_take_remote().expect("take remote");

    assert_eq!(Path::new(&took.local_path), cache.as_path());
    assert_eq!(
        took.editing_stopped, None,
        "SpeleoDB records this account's lock"
    );
    assert_eq!(
        open_name(&state).as_deref(),
        Some("Revision X"),
        "the server version replaced the local copy"
    );
    assert_eq!(
        stub.releases.load(Ordering::SeqCst),
        0,
        "nothing was handed back"
    );
    assert!(gate_open(&state), "the session goes on editing");
    drop(stub);
}

/// Taking the server version no longer shuts the edit gate first, so an edit
/// can land while it reads SpeleoDB's history. That edit was made after the
/// user confirmed the discard, so it is not discarded with the rest: the
/// reload is refused and the edit stays. (Before this fix the gate was shut
/// only after the history read, and an edit landing during it was discarded
/// too.)
/// Injection: drop the revision `speleodb_take_remote` reads on entry (pass
/// `None` to the fast-forward) and the edit is discarded with the copy.
#[test]
fn an_edit_during_take_remotes_history_read_refuses_it() {
    let id = "33333333-3333-3333-3333-333333333332";
    let temp = tempfile::tempdir().expect("temp");
    let (seen_tx, seen) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let armed = Arc::new(AtomicBool::new(false));
    let stub = serve(
        id,
        revision_bytes(temp.path()),
        Some(Gate {
            route: "/revisions/",
            armed: Arc::clone(&armed),
            seen: seen_tx,
            release: release_rx,
        }),
    );
    let state = AppState::new().expect("state");
    state.set_event_sink(Arc::new(NullSink));
    let cache = open_p(&state, id, &stub.base);
    armed.store(true, Ordering::SeqCst);

    std::thread::scope(|scope| {
        let state = &state;
        let pending = scope.spawn(move || {
            state
                .speleodb_take_remote()
                .map_err(|e| (e.action, e.message))
        });
        seen.recv().expect("history read in flight");
        let edited = state.set_surface_elevation(Some(12.5));
        assert!(edited.success, "the session is still editing: {edited:?}");
        release.send(()).expect("release the history read");
        let (action, message) = pending
            .join()
            .expect("join")
            .expect_err("an edit made meanwhile refuses the take");
        assert_eq!(action, SpeleoDbAction::Retry, "{message}");
    });
    let open = state.get_project_state().expect("P");
    assert_eq!(
        open.name, "P",
        "the server version did not replace the copy"
    );
    assert_eq!(
        open.settings.surface_elevation,
        Some(12.5),
        "the edit made meanwhile is kept"
    );
    assert_eq!(on_disk(&cache).2.base_commit.as_deref(), Some(TIP));
    assert_eq!(stub.releases.load(Ordering::SeqCst), 0);
    assert!(gate_open(&state), "still editing");
    drop(stub);
}
