//! The revision swaps a downloaded revision cannot honour, end to end against a
//! loopback SpeleoDB: a Restore of a revision a newer build saved (it opens
//! read-only, so the dirty copy it stamps could never be pushed; review
//! G-speleodb-8), and a history-keeping Unwind step onto a revision an earlier
//! build pushed (its SQLite history is never converted, plan 7.2.8; review
//! G-speleodb-5). Both are refused before the working copy is replaced.
//!
//! All data is synthetic: the files are built by the encoder and the legacy
//! fixture kit, the server is a loopback listener.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::speleodb_local_state::{cache_path_in, TestCacheRoot};
use super::stalx_fixtures::{
    extras_for, line_survey, link_on_disk, write_file, write_legacy_link_only,
};
use super::AppState;
use crate::models::{SpeleoLink, UndoMode};
use crate::speleodb::auth::TokenStore;

const ID: &str = "11111111-1111-1111-1111-111111111111";
const ME: &str = "diver@example.com";
/// The server tip, which the working copy sits on.
const BASE: &str = "abc1234";
/// The tip's parent: an older revision to restore.
const OLDER: &str = "0123456";

/// A loopback SpeleoDB whose history is `OLDER` then `BASE`, whose lock [`ME`]
/// holds, and whose every download answers `revision` (the bytes of a `.stalx`).
/// Returns the endpoint and the request lines it saw.
fn server(revision: Vec<u8>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let line = read_request_line(&mut stream);
            let (status, kind, body) = if line.contains("/user/") {
                (
                    200,
                    "application/json",
                    super::tests_support::sdk_fixture(&format!(r#"{{"email":"{ME}"}}"#))
                        .into_bytes(),
                )
            } else if line.contains("/revisions/") {
                let commits = format!(
                    r#"[{{"id":"{BASE}","parent_ids":["{OLDER}"],"author_email":"{ME}"}},{{"id":"{OLDER}","parent_ids":[],"author_email":"{ME}"}}]"#
                );
                (
                    200,
                    "application/json",
                    super::tests_support::sdk_fixture(&format!(r#"{{"commits":{commits}}}"#))
                        .into_bytes(),
                )
            } else if line.contains("/download/") {
                (200, "application/octet-stream", revision.clone())
            } else {
                (
                    200,
                    "application/json",
                    super::tests_support::sdk_fixture(&format!(
                        r#"{{"id":"{ID}","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{{"user":"{ME}"}}}}"#
                    ))
                    .into_bytes(),
                )
            };
            if let Ok(mut log) = log.lock() {
                log.push(line);
            }
            let head = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

/// The request line of one request (its headers and body are read and dropped).
fn read_request_line(stream: &mut std::net::TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    while let Ok(count) = stream.read(&mut chunk) {
        if count == 0 {
            break;
        }
        request.extend_from_slice(chunk.get(..count).unwrap_or_default());
        if request.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&request)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn link(endpoint: &str, has_unpushed_edits: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: ID.to_owned(),
        project_name: "Remote".to_owned(),
        base_commit_sha: Some(BASE.to_owned()),
        has_unpushed_edits,
        last_edit_at: None,
        last_verify_at: Some(chrono::Utc::now().to_rfc3339()),
        undo_mode: Some(UndoMode::Full),
    }
}

/// A clean Full-mode linked copy at its cache path under `root`, open in a
/// signed-in edit session against `endpoint`.
fn editing(root: &Path, endpoint: &str) -> (PathBuf, AppState) {
    let path = cache_path_in(root, ID);
    write_file(
        &path,
        &line_survey("Remote"),
        &extras_for(Some(&link(endpoint, false))),
    );
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

/// The bytes of a revision a newer Stalagmight saved: a version 2 file that
/// requires a feature this build does not know, so it opens read-only.
fn newer_revision(dir: &Path, endpoint: &str) -> Vec<u8> {
    let path = dir.join("newer.stalx");
    write_file(
        &path,
        &line_survey("Newer"),
        &extras_for(Some(&link(endpoint, false))),
    );
    let text = std::fs::read_to_string(&path).expect("read the revision");
    assert!(
        text.contains("\"requires\":[]"),
        "the fixture names its requires"
    );
    text.replacen("\"requires\":[]", "\"requires\":[\"x-from-the-future\"]", 1)
        .into_bytes()
}

#[expect(
    clippy::result_large_err,
    reason = "the verb's own structured IPC error, asserted on as it is"
)]
fn restore(
    state: &AppState,
    hexsha: &str,
    restore: bool,
    keep_undo_history: bool,
) -> Result<crate::speleodb::SpeleoOpenResult, crate::speleodb::SpeleoErrorDto> {
    state.speleodb_restore_revision(
        ID.to_owned(),
        "Remote".to_owned(),
        hexsha.to_owned(),
        restore,
        false,
        keep_undo_history,
        false,
        false,
    )
}

#[test]
fn a_restore_of_a_revision_a_newer_build_saved_is_refused_and_a_view_still_shows_it() {
    // Injection: drop the read-only refusal in `speleodb_swap_in_revision` and the
    // Restore lands a dirty, read-only copy holding the lock, which no push can
    // ever upload.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, _seen) = {
            // The endpoint is only known once the server listens, and the revision
            // carries it in its link; a placeholder there is fine (the install
            // stamps the session's own link over it).
            server(newer_revision(temp.path(), "https://placeholder.invalid"))
        };
        let (path, state) = editing(temp.path(), &endpoint);
        let before = std::fs::read(&path).expect("the working copy");

        let refused = restore(&state, OLDER, true, false).expect_err("the Restore is refused");
        assert!(
            refused.message.contains("newer version of Stalagmight")
                && refused.message.contains("not restore it"),
            "{refused:?}"
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), before);
        assert!(
            state.read_only_reason().is_none(),
            "the open copy is untouched"
        );
        assert!(!link_on_disk(&path).expect("linked").has_unpushed_edits);
        assert!(state.edit_enabled(), "the edit session goes on");

        // A View is clean, so it shows the revision read-only.
        restore(&state, OLDER, false, false).expect("a View shows it");
        assert!(state.read_only_reason().is_some(), "opened read-only");
        assert!(!link_on_disk(&path).expect("linked").has_unpushed_edits);
    });
}

#[test]
fn an_unwind_step_onto_a_revision_an_earlier_build_pushed_is_refused_before_the_swap() {
    // Injection: drop the legacy refusal in `speleodb_restore_revision` and the step
    // replaces the copy with a history-less one and records a chain position.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let legacy = temp.path().join("legacy.stalx");
        write_legacy_link_only(&legacy, &link("https://placeholder.invalid", false));
        let (endpoint, seen) = server(std::fs::read(&legacy).expect("legacy bytes"));
        let (path, state) = editing(temp.path(), &endpoint);
        let before = std::fs::read(&path).expect("the working copy");

        // The Unwind's first step loads the history of the commit the copy sits on.
        let refused = restore(&state, BASE, true, true).expect_err("the step is refused");
        assert!(
            refused.message.contains("earlier version of Stalagmight")
                && refused.message.contains("without its history"),
            "{refused:?}"
        );
        assert!(
            seen.lock()
                .expect("log")
                .iter()
                .any(|l| l.contains("/download/")),
            "refused on what it downloaded"
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), before);
        assert_eq!(
            state.speleodb_unwound_from(),
            None,
            "no chain step recorded"
        );
        assert!(state.edit_enabled());
    });
}

#[test]
fn a_push_whose_save_is_refused_says_why() {
    // Review G-speleodb-8: a dirty copy that opens read-only (a newer build's
    // file) cannot be saved, so it cannot be pushed; the push used to say only
    // "failed to flush project before push". Injection: drop the save's error
    // from that message and READ_ONLY_FILE is lost again.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, seen) = server(Vec::new());
        let path = cache_path_in(temp.path(), ID);
        write_file(
            &path,
            &line_survey("Remote"),
            &extras_for(Some(&link(&endpoint, true))),
        );
        let text = std::fs::read_to_string(&path).expect("read");
        std::fs::write(
            &path,
            text.replacen("\"requires\":[]", "\"requires\":[\"x-from-the-future\"]", 1),
        )
        .expect("write");
        let state = AppState::new().expect("state");
        assert!(state.open_project(path.clone()).success, "open the copy");
        assert!(state.read_only_reason().is_some());
        {
            let mut session = state.speleodb.lock().expect("session");
            session.endpoint = Some(endpoint);
            session.email = Some(ME.to_owned());
            session.token_store = TokenStore::from_in_memory_token("tok-1");
            session.rehydrate_attempted = true;
        }
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(ID));

        let refused = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect_err("a read-only copy is not pushed");
        assert!(
            refused.message.contains(super::READ_ONLY_FILE_CODE),
            "{refused:?}"
        );
        assert!(
            !seen
                .lock()
                .expect("log")
                .iter()
                .any(|l| l.contains("/upload/")),
            "nothing was uploaded"
        );
        assert!(link_on_disk(&path).expect("linked").has_unpushed_edits);
    });
}
