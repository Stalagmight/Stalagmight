//! [#576 NF-W2SAVE-1] Done and Push carry the warnings of the save they run first.
//!
//! Done and Push save the live file in place before taking the upload snapshot
//! (`speleodb_flush_before_push`). A save that goes through can still have lost
//! something - the display settings, the undo history - and says so in its
//! warnings, which a normal save shows the user. The pre-push save only logged
//! them, and those warnings are one-shot: the next save does not repeat them. The
//! flush now hands them back so the Done / Push answer can carry them.
//!
//! The flush is driven directly: Done and Push reach it only after the cache-dir
//! check that only the env-gated live tests can pass. All data is synthetic.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;

/// A project whose display-settings table the save cannot read, so a save goes
/// through without it and says so (`DISPLAY_SETTINGS_NOT_SAVED`).
fn file_with_unreadable_display_settings(dir: &Path) -> PathBuf {
    let path = dir.join("CaveF.stalx");
    let mut project = Project::new("CaveF".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionF".to_string()),
        shots: vec![ShotData {
            length: 4.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = Database::open(&path).expect("create the fixture");
    db.save_project(&project).expect("save the fixture");
    drop(db);
    path
}

fn break_display_settings(path: &Path) {
    rusqlite::Connection::open(path)
        .expect("open the fixture directly")
        .execute_batch("ALTER TABLE display_settings RENAME COLUMN value TO broken_value;")
        .expect("break the display settings table");
}

#[test]
fn the_pre_push_save_hands_back_its_warnings() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let path = file_with_unreadable_display_settings(dir.path());
    let state = AppState::new().expect("state");
    assert!(state.open_project(path.clone()).success);
    break_display_settings(&path);

    let warnings = state
        .speleodb_flush_before_push()
        .expect("the save goes ahead without the display settings");

    let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();
    println!("the pre-push save's warnings: {codes:?}");
    assert!(
        codes.contains(&"DISPLAY_SETTINGS_NOT_SAVED"),
        "the lost display settings reach the push answer, got {codes:?}"
    );
}

/// [#576 RV9a-8] The upload's 200 arm reports a history the push cleanup had to
/// drop. The upload releases `command_lock` while it is on the network, so an edit
/// can crash in that window; the cleanup after the landing then clears the file's
/// history (`UNDO_HISTORY_DROPPED`) instead of writing the crashed session's, and
/// the push answer must say so - the pre-push save ran before the crash and said
/// nothing. Driven through `speleodb_upload` against a loopback stub that crashes
/// the session (poisons the undo stack) while it holds the upload, then answers 200.
///
/// The upload snapshots into the process-wide app cache dir, a `OnceLock` other
/// tests in this binary rely on staying unset, so the body runs in a process of its
/// own (as `take_remote_success_tests` does).
#[test]
fn a_crash_during_the_upload_reaches_the_push_answer_as_a_dropped_history() {
    run_crash_during_upload_body(200);
}

/// [#576 RV9a-8] The 304 arm (the tree already matched: the documented recovery
/// re-uploads a landing whose answer was lost) concludes the landing through the same
/// cleanup, so a crash in the upload's window drops the history there too, and the
/// push answer must say so just the same. The body of the 200 test, with the stub
/// answering the upload 304 Not Modified.
#[test]
fn a_crash_during_an_upload_answered_304_reaches_the_push_answer_as_a_dropped_history() {
    run_crash_during_upload_body(304);
}

/// Runs [`crash_during_upload_body`] in a process of its own, its stub answering the
/// upload with `put_status`.
fn run_crash_during_upload_body(put_status: u16) {
    const BODY: &str = "core::state::sdb_flush_warnings_tests::crash_during_upload_body";
    let exe = std::env::current_exe().expect("this test binary");
    let output = super::take_remote_success_tests::output_within(
        std::process::Command::new(exe)
            .args([
                BODY,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CRASH_CHILD_ENV, put_status.to_string()),
        super::take_remote_success_tests::CHILD_DEADLINE,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("{stdout}");
    assert!(
        output.status.success(),
        "the body failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("1 passed"), "the body ran: {stdout}");
}

/// The environment variable that lets [`crash_during_upload_body`] run: the status its
/// stub answers the upload with (`200` or `304`).
const CRASH_CHILD_ENV: &str = "STALAGMIGHT_CRASH_UPLOAD_CHILD";

#[test]
#[ignore = "the child-process body of a_crash_during_the_upload_reaches_the_push_answer_as_a_\
            dropped_history: it sets the process-wide app dirs"]
fn crash_during_upload_body() {
    use std::io::{Read as _, Write as _};
    use std::sync::atomic::{AtomicBool, Ordering};

    let Some(put_status) = std::env::var(CRASH_CHILD_ENV)
        .ok()
        .and_then(|status| status.parse::<u16>().ok())
    else {
        println!("not in the child process ({CRASH_CHILD_ENV} unset): nothing to do");
        return;
    };
    let _cfg = crate::test_helpers::isolated_config();
    let dirs = TempDir::new().expect("app dirs");
    crate::app_dirs::init_paths(
        dirs.path().join("data"),
        dirs.path().join("config"),
        dirs.path().join("cache"),
        None,
    );
    let cache = super::speleodb_local_state::cache_root().expect("a cache root in this process");
    let project_cache =
        super::speleodb_local_state::cache_path_in(&cache, super::speleodb::REMOTE_ID);
    std::fs::create_dir_all(project_cache.parent().expect("cache dir")).expect("create cache dir");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    listener.set_nonblocking(true).expect("nonblocking accept");
    let base = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("addr").port()
    );
    // The canonical cache copy (the upload snapshots it), linked to the stub and
    // open mid edit session under our lock.
    super::take_remote_success_tests::project_file(&project_cache, "Local Copy", "LocalSection");
    Database::open_existing(&project_cache)
        .expect("reopen")
        .save_speleo_link(Some(&crate::models::SpeleoLink {
            endpoint: base.clone(),
            project_id: super::speleodb::REMOTE_ID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some(super::take_remote_success_tests::BASE_SHA.to_owned()),
            has_unpushed_edits: true,
            last_edit_at: None,
            last_verify_at: Some(chrono::Utc::now().to_rfc3339()),
            undo_mode: Some(crate::models::UndoMode::Full),
        }))
        .expect("link");
    let state = crate::test_fixtures::state_with_null_sink();
    assert!(
        state.open_project(project_cache).success,
        "open the cache copy"
    );
    *state.speleodb.lock() = super::speleodb::authed_session_at(&base);
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(super::speleodb::REMOTE_ID));
    let link = state.speleodb_open_link().expect("the fixture is linked");
    let done = AtomicBool::new(false);
    let seen = std::sync::Mutex::new(Vec::<String>::new());

    let (answer, warnings) = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut request = Vec::new();
                let mut chunk = vec![0u8; 64 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(chunk.get(..n).unwrap_or_default());
                }
                let line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let (status, body) = if line.starts_with("PUT ") {
                    // The crash, in the window the upload holds no lock.
                    std::thread::scope(|crash| {
                        let _ = crash
                            .spawn(|| {
                                let _held = state.undo_stack.lock();
                                panic!("synthetic crash while the upload is on the network");
                            })
                            .join();
                    });
                    if put_status == 304 {
                        ("304 Not Modified", String::new())
                    } else {
                        (
                            "200 OK",
                            format!(
                                r#"{{"hexsha":"{}","project":{{"id":"{}","name":"Remote"}}}}"#,
                                "f00d".repeat(10),
                                super::speleodb::REMOTE_ID
                            ),
                        )
                    }
                } else {
                    (
                        "200 OK",
                        format!(
                            r#"{{"id":"{}","name":"Remote"}}"#,
                            super::speleodb::REMOTE_ID
                        ),
                    )
                };
                if let Ok(mut lines) = seen.lock() {
                    lines.push(line);
                }
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        let mut warnings = Vec::new();
        let answer = state.speleodb_upload(&base, "tok-1", true, &link, None, false, &mut warnings);
        done.store(true, Ordering::SeqCst);
        (answer, warnings)
    });

    let codes: Vec<&str> = warnings.iter().map(|w| w.code.as_str()).collect();
    let requests = seen.lock().expect("request log").clone();
    println!(
        "upload answered {put_status} after a crash in its window -> {:?}; push answer \
         warnings {codes:?}; requests {requests:?}",
        answer
            .as_ref()
            .map(|_| "landed")
            .map_err(|e| e.message.clone())
    );
    assert!(
        requests.iter().any(|r| r.starts_with("PUT ")),
        "the upload reached the stub: {requests:?}"
    );
    assert!(answer.is_ok(), "the upload landed");
    assert!(
        codes.contains(&super::save_sidecars::UNDO_HISTORY_DROPPED_CODE),
        "the dropped history reaches the push answer, got {codes:?}"
    );
}
