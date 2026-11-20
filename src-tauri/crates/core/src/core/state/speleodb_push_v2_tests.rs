//! The push on `.stalx` version 2, end to end against a loopback SpeleoDB:
//! what the upload carries (plan 6.5 mode 2, 10.1), that every state change a
//! landed push makes is durable at once (R2 B3), that a server still refusing
//! non-SQLite uploads leaves the project dirty and locked (plan 12.1), and the
//! clear-on-push of the pushed history (#303).
//!
//! All data is synthetic: the project is built by the encoder, the server is a
//! loopback listener.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::speleodb_local_state::{cache_path_in, TestCacheRoot};
use super::stalx_fixtures::{decoded, extras_for, line_survey, link_on_disk, write_file};
use super::AppState;
use crate::core::Command;
use crate::models::{SpeleoLink, UndoMode};
use crate::speleodb::auth::TokenStore;
use crate::speleodb::PushResult;

const ID: &str = "11111111-1111-1111-1111-111111111111";
const ME: &str = "diver@example.com";
const BASE: &str = "abc1234";
const PUSHED: &str = "def5678";

/// What the loopback server saw.
#[derive(Default)]
struct Seen {
    lines: Vec<String>,
    uploads: Vec<String>,
}

/// A loopback SpeleoDB: the lock is held by [`ME`], the history is `BASE` (and
/// `PUSHED` on top once an upload landed), an upload answers 200 with `PUSHED`
/// or, when `refuse_uploads`, the 400 a server that only takes SQLite sends.
fn server(refuse_uploads: bool) -> (String, Arc<Mutex<Seen>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut landed = false;
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_request(&mut stream);
            let line = request.lines().next().unwrap_or_default().to_owned();
            let project = super::tests_support::sdk_fixture(&format!(
                r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{{"user":"{ME}"}}}}"#
            ));
            let (status, body) = if line.contains("/user/") {
                (
                    200,
                    super::tests_support::sdk_fixture(&format!(r#"{{"email":"{ME}"}}"#)),
                )
            } else if line.contains("/revisions/") {
                let commits = if landed {
                    format!(
                        r#"[{{"id":"{PUSHED}","parent_ids":["{BASE}"],"author_email":"{ME}"}},{{"id":"{BASE}","parent_ids":[]}}]"#
                    )
                } else {
                    format!(r#"[{{"id":"{BASE}","parent_ids":[]}}]"#)
                };
                (
                    200,
                    super::tests_support::sdk_fixture(&format!(r#"{{"commits":{commits}}}"#)),
                )
            } else if line.contains("/upload/") {
                if let Ok(mut log) = log.lock() {
                    log.uploads.push(request.clone());
                }
                if refuse_uploads {
                    (
                        400,
                        r#"{"detail":"Only SQLite project files are accepted."}"#.to_owned(),
                    )
                } else {
                    landed = true;
                    (
                        200,
                        format!(
                            r#"{{"project":{project},"hexsha":"{PUSHED}","files":["project.stalx"],"message":"Saved","browser_url":null}}"#
                        ),
                    )
                }
            } else {
                (200, project)
            };
            if let Ok(mut log) = log.lock() {
                log.lines.push(line);
            }
            let reply = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
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

/// A Full-mode linked project at its cache path under `root`, open in a signed-in
/// edit session against `endpoint`.
fn editing(root: &Path, endpoint: &str) -> (PathBuf, AppState) {
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
    assert!(
        state.open_project(path.clone()).success,
        "open the cache copy"
    );
    {
        let mut session = state.speleodb.lock().expect("session");
        session.endpoint = Some(endpoint.to_owned());
        session.email = Some(ME.to_owned());
        session.token_store = TokenStore::from_in_memory_token("tok-1");
        session.rehydrate_attempted = true;
    }
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(ID));
    (path, state)
}

fn reverse_the_line(state: &AppState) {
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

#[test]
fn a_confirmed_push_uploads_v2_json_and_is_durable_at_once() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, seen) = server(false);
        let (path, state) = editing(temp.path(), &endpoint);
        reverse_the_line(&state);
        assert!(state.speleodb_open_link().expect("link").has_unpushed_edits);
        assert!(state.has_unsaved_changes());

        let pushed = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect("push");
        assert!(
            matches!(
                pushed,
                PushResult::Pushed {
                    undo_history_omitted: None
                }
            ),
            "{pushed:?}"
        );

        // What went up: the canonical v2 file as JSON, with the Full-mode history
        // and no per-machine state.
        let uploads = &seen.lock().expect("log").uploads;
        assert_eq!(uploads.len(), 1);
        let upload = &uploads[0];
        assert!(upload.contains("filename=\"project.stalx\""));
        assert!(upload.contains("application/json"), "the part says JSON");
        assert!(
            upload.contains("{\"format\":\"stalx\",\"version\":2"),
            "a v2 body"
        );
        assert!(
            upload.contains("\"history\":{\"undo\":["),
            "Full mode ships the undo history"
        );
        assert!(
            !upload.contains("\n\"local\":"),
            "no per-machine state rides to the server (the `local` member; the \
             `conventions` also name a `local` frame)"
        );

        // R2 B3: BASE, the dirty bit and the cut history are on DISK now, not only
        // in memory - a close right after the push never reopens to a stale BASE.
        let on_disk = link_on_disk(&path).expect("linked");
        assert_eq!(on_disk.base_commit_sha.as_deref(), Some(PUSHED));
        assert!(!on_disk.has_unpushed_edits);
        let file = decoded(&path);
        assert!(
            file.extras.history.undo.is_empty(),
            "the pushed history is cut on disk"
        );
        assert!(!state.can_undo(), "and in memory (clear-on-push)");
        assert!(!state.has_unsaved_changes(), "the push saved first");
        assert!(state.edit_enabled(), "Push keeps the lock");
        assert_eq!(file.project.lines().len(), 1);
    });
}

#[test]
fn a_server_that_refuses_v2_uploads_leaves_the_project_dirty_and_locked() {
    // Plan 12.1 (rollout prerequisite S0): until SpeleoDB accepts v2 uploads, a
    // push fails through the existing error path and nothing local is lost.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, seen) = server(true);
        let (path, state) = editing(temp.path(), &endpoint);
        reverse_the_line(&state);

        let refused = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect_err("the server refused the upload");
        assert!(
            refused.message.contains("Only SQLite") || refused.status == Some(400),
            "{refused:?}"
        );
        let on_disk = link_on_disk(&path).expect("linked");
        assert!(
            on_disk.has_unpushed_edits,
            "the work is still marked un-pushed"
        );
        assert_eq!(
            on_disk.base_commit_sha.as_deref(),
            Some(BASE),
            "BASE unchanged"
        );
        assert!(state.edit_enabled(), "the lock is still held");
        assert!(state.can_undo(), "and the history is untouched");
        assert!(
            !seen
                .lock()
                .expect("log")
                .lines
                .iter()
                .any(|l| l.contains("/release/")),
            "nothing released the lock"
        );
    });
}

#[test]
fn clear_on_push_cuts_the_pushed_history_and_the_chain_on_disk() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, _seen) = server(false);
        let (path, state) = editing(temp.path(), &endpoint);
        for _ in 0..3 {
            reverse_the_line(&state);
        }
        assert!(state.save_project(None).success, "save");
        state.speleodb_set_unwound_from(Some("older-revision"));
        let stamps: Vec<_> = state
            .undo_stack
            .lock()
            .expect("stack")
            .undo_entries()
            .map(crate::core::undo::UndoEntry::timestamp)
            .collect();
        assert_eq!(stamps.len(), 3);
        // Entries are listed newest first: everything up to the second-newest was
        // in the commit; the newest was recorded after the snapshot.
        let cutoff = stamps[1];
        {
            let project = state.project.lock().expect("project");
            state.speleodb_clear_undo_after_push(project.as_ref(), cutoff);
        }
        assert_eq!(
            state.undo_stack.lock().expect("stack").len(),
            1,
            "the newest is kept"
        );
        let file = decoded(&path);
        assert_eq!(
            file.extras.history.undo.len(),
            1,
            "and the file says the same"
        );
        assert!(file.extras.history.redo.is_empty());
        assert_eq!(
            file.extras.local.speleodb.unwound_from, None,
            "the chain is cleared"
        );
        assert_eq!(file.extras.local.speleodb.inherited_through, None);
    });
}
