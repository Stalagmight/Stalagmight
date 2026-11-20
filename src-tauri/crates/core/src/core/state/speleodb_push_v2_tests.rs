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

/// How the loopback server answers an upload.
#[derive(Clone, Copy)]
enum Upload {
    /// 200 with `PUSHED`, which lands on the history.
    Accept,
    /// `status` with nothing landed: a server that does not take version 2 yet
    /// (the real server's answer is not recorded; the SDK declares 400, 406, 415
    /// and 500 for the upload), or a transient failure.
    Refuse(u16),
    /// `status` AFTER the commit landed: the ambiguous server error.
    FailAfterLanding(u16),
}

/// A loopback SpeleoDB: the lock is held by [`ME`], the history is `BASE` (and
/// `PUSHED` on top once an upload landed), an upload is answered as `upload`
/// says, a release frees the lock and a create (`POST /projects/`) answers the
/// project.
fn server(upload: Upload) -> (String, Arc<Mutex<Seen>>) {
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
                let refusal = r#"{"detail":"Synthetic refusal of the uploaded file."}"#;
                match upload {
                    Upload::Refuse(status) => (status, refusal.to_owned()),
                    Upload::FailAfterLanding(status) => {
                        landed = true;
                        (status, refusal.to_owned())
                    }
                    Upload::Accept => {
                        landed = true;
                        (
                            200,
                            format!(
                                r#"{{"project":{project},"hexsha":"{PUSHED}","files":["project.stalx"],"message":"Saved","browser_url":null}}"#
                            ),
                        )
                    }
                }
            } else if line.starts_with("POST /api/v2/projects/ ") {
                // A create answers 201 with the new project.
                (201, project)
            } else if line.contains("/release/") {
                (
                    200,
                    super::tests_support::sdk_fixture(&format!(
                        r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE"}}"#
                    )),
                )
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
        let (endpoint, seen) = server(Upload::Accept);
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

/// The one error every push surface shows for a server without version 2
/// support (plan 12.1), checked on the DTO the frontend reads.
fn assert_format_refused(refused: &crate::speleodb::SpeleoErrorDto, status: u16) {
    assert_eq!(refused.format_refused, Some(true), "{refused:?}");
    assert_eq!(refused.status, Some(status), "{refused:?}");
    assert_eq!(refused.action, crate::speleodb::SpeleoDbAction::Fatal);
    assert!(
        refused.message.starts_with(
            "This SpeleoDB server does not accept projects saved by this version yet."
        ),
        "{refused:?}"
    );
}

/// What a failed push must leave: dirty at BASE on disk, the lock and the history
/// kept, and no release sent (the lock lifecycle of any failed push in 0.6.0).
fn assert_kept_dirty_and_locked(path: &Path, state: &AppState, seen: &Mutex<Seen>) {
    let on_disk = link_on_disk(path).expect("linked");
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
}

#[test]
fn every_refusal_of_the_content_is_the_one_format_error_on_every_push() {
    // Plan 12.1 and the rollout decision: a server that refuses the version 2
    // upload gets ONE clear error, from Save Changes and from Done alike (the
    // close and quit prompts run Done), never "the upload could not be
    // confirmed". Injection: drop the format check in `speleodb_upload` and the
    // DTO carries no `format_refused` (and a 500 below turns unconfirmed).
    for status in [400, 406, 415, 422] {
        crate::test_helpers::with_isolated_config(|| {
            let temp = tempfile::tempdir().expect("temp");
            let _cache = TestCacheRoot::set(temp.path());
            let (endpoint, seen) = server(Upload::Refuse(status));
            let (path, state) = editing(temp.path(), &endpoint);
            reverse_the_line(&state);

            let pushed = state
                .speleodb_push_only(Some("Synthetic push".to_owned()))
                .expect_err("Save Changes is refused");
            assert_format_refused(&pushed, status);
            let done = state
                .speleodb_done(Some("Synthetic push".to_owned()))
                .expect_err("Done is refused");
            assert_format_refused(&done, status);
            assert_eq!(seen.lock().expect("log").uploads.len(), 2);
            assert_kept_dirty_and_locked(&path, &state, &seen);
        });
    }
}

#[test]
fn a_server_error_that_landed_nothing_is_the_same_refusal() {
    // A server whose upload processor raises on the version 2 file answers 500
    // and commits nothing: the history read right after shows the tip still at
    // BASE, so it is the format error, not an ambiguous upload retried forever.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, seen) = server(Upload::Refuse(500));
        let (path, state) = editing(temp.path(), &endpoint);
        reverse_the_line(&state);

        let refused = state
            .speleodb_done(Some("Synthetic push".to_owned()))
            .expect_err("Done is refused");
        assert_format_refused(&refused, 500);
        {
            let log = seen.lock().expect("log");
            let upload = log
                .lines
                .iter()
                .position(|l| l.contains("/upload/"))
                .expect("uploaded");
            assert!(
                log.lines[upload..]
                    .iter()
                    .any(|l| l.contains("/revisions/")),
                "the history was read after the error: {:?}",
                log.lines
            );
        }
        assert_kept_dirty_and_locked(&path, &state, &seen);
    });
}

#[test]
fn a_server_error_after_a_landing_or_a_busy_server_stays_unconfirmed() {
    // The ambiguity the ancestry recovery exists for is kept: a 5xx after the
    // commit landed, and 503 (try later) or 504 (still working), are not a
    // refusal of the file.
    for upload in [
        Upload::FailAfterLanding(500),
        Upload::Refuse(503),
        Upload::Refuse(504),
    ] {
        crate::test_helpers::with_isolated_config(|| {
            let temp = tempfile::tempdir().expect("temp");
            let _cache = TestCacheRoot::set(temp.path());
            let (endpoint, _seen) = server(upload);
            let (path, state) = editing(temp.path(), &endpoint);
            reverse_the_line(&state);
            let pushed = state
                .speleodb_push_only(Some("Synthetic push".to_owned()))
                .expect("an ambiguous upload is not an error");
            assert!(matches!(pushed, PushResult::PushUnconfirmed), "{pushed:?}");
            let on_disk = link_on_disk(&path).expect("linked");
            assert!(on_disk.has_unpushed_edits);
            assert!(state.edit_enabled());
        });
    }
}

#[test]
fn a_server_that_refuses_v2_uploads_leaves_the_project_dirty_and_locked() {
    // Plan 12.1 (rollout prerequisite S0): until SpeleoDB accepts v2 uploads, a
    // push fails with the format error and nothing local is lost.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, seen) = server(Upload::Refuse(400));
        let (path, state) = editing(temp.path(), &endpoint);
        reverse_the_line(&state);

        let refused = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect_err("the server refused the upload");
        assert_format_refused(&refused, 400);
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
        let (endpoint, _seen) = server(Upload::Accept);
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

#[test]
fn publish_to_a_server_that_refuses_the_format_says_so_and_never_creates_twice() {
    // The rollout decision: Publish reports a refused first upload clearly (the
    // server project is empty, the local copy unchanged) instead of success, and
    // a retry never POSTs a second server project. Injection: drop
    // `upload_format_refused` from the result and the first assertion fails.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(&temp.path().join("cache"));
        let (endpoint, seen) = server(Upload::Refuse(415));
        let local = temp.path().join("local.stalx");
        write_file(&local, &line_survey("Local"), &extras_for(None));
        let state = AppState::new().expect("state");
        assert!(
            state.open_project(local.clone()).success,
            "open the local file"
        );
        {
            let mut session = state.speleodb.lock().expect("session");
            session.endpoint = Some(endpoint.clone());
            session.email = Some(ME.to_owned());
            session.token_store = TokenStore::from_in_memory_token("tok-1");
            session.rehydrate_attempted = true;
        }
        let params = || crate::speleodb::SpeleoPublishParams {
            message: Some("Synthetic publish".to_owned()),
            name: "Remote".to_owned(),
            description: String::new(),
            country: "US".to_owned(),
            visibility: crate::speleodb::Visibility::Private,
            latitude: None,
            longitude: None,
        };
        let creates = || {
            seen.lock()
                .expect("log")
                .lines
                .iter()
                .filter(|l| l.starts_with("POST /api/v2/projects/ "))
                .count()
        };

        let published = state.speleodb_publish(params()).expect("created");
        assert_eq!(published.upload_format_refused, Some(true));
        assert_eq!(published.undo_history_omitted, None);
        assert_eq!(creates(), 1);
        assert_eq!(seen.lock().expect("log").uploads.len(), 1);
        // The local copy is unchanged and linked, its work still un-uploaded.
        let cache = PathBuf::from(&published.local_path);
        let link = link_on_disk(&cache).expect("the cache copy is linked");
        assert!(link.has_unpushed_edits);
        assert_eq!(decoded(&cache).project.lines().len(), 1);
        assert!(std::fs::metadata(&local).is_ok(), "the original stays");

        let again = state
            .speleodb_publish(params())
            .expect_err("a linked copy is never published twice");
        assert!(again.message.contains("already linked"), "{again:?}");
        assert_eq!(creates(), 1, "no second server project");
    });
}
