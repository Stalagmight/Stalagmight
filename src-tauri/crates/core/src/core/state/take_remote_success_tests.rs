//! [#576 E10] "Take remote" lands the server tip and discards the local edits.
//!
//! Driven end to end against a loopback stub that answers like SpeleoDB - the
//! revision history (a tip another account pushed on top of our BASE), the lock
//! release, and the dump at the tip, a ZIP built in this test around a synthetic
//! server project. The open project is the canonical cache copy, mid edit
//! session, with a local edit: after the call BASE is the tip, the dirty bit is
//! cleared, the project is read-only, the abandoning release went out, and the
//! local edit is gone (the open project is the server's).
//!
//! The verb resolves the cache path through the process-wide app dirs, a
//! `OnceLock` the other tests in this binary rely on staying unset (several
//! accept a linked file only because no cache root exists). So the body runs in a
//! process of its own: the test below re-runs this test binary on the ignored,
//! env-gated body, which points the app dirs at a private temp dir.
//!
//! All data is synthetic, built in code.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Command as ChildCommand, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::speleodb::{authed_session_at, REMOTE_ID};
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData, SpeleoLink, UndoMode};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// The environment variable that lets the child-process body run.
const CHILD_ENV: &str = "STALAGMIGHT_TAKE_REMOTE_CHILD";
/// The body's full test name, for the child's exact filter.
const CHILD_TEST: &str =
    "core::state::take_remote_success_tests::take_remote_body_in_a_process_of_its_own";

/// Our BASE, and the tip another account pushed on top of it (revision ids are
/// 40-hex shas: anything else is not pinned, and the dump comes from HEAD).
pub(super) const BASE_SHA: &str = "abc1230000000000000000000000000000000000";
pub(super) const TIP_SHA: &str = "def4560000000000000000000000000000000000";

/// Every JSON request: the project, its revision history and the release answer.
/// Our own account (`u@x`, `authed_session_at`) holds the lock.
fn project_json() -> String {
    project_json_held_by(r#""active_mutex":{"user":"u@x"},"#)
}

/// [`project_json`] with `mutex` (an `"active_mutex":{..},` member, or nothing for
/// an unheld lock) in place of our account's.
pub(super) fn project_json_held_by(mutex: &str) -> String {
    format!(
        r#"{{"id":"{REMOTE_ID}","name":"Remote",{mutex}
            "commits":[{{"id":"{TIP_SHA}","parent_ids":["{BASE_SHA}"],"author_email":"other@x"}},
                       {{"id":"{BASE_SHA}","parent_ids":[],"author_email":"u@x"}}]}}"#
    )
}

/// A project file holding one section named `section`.
pub(super) fn project_file(path: &Path, name: &str, section: &str) {
    let mut project = Project::new(name.to_owned(), None);
    project.add_section(SectionData {
        name: Some(section.to_owned()),
        color: "#123456".to_owned(),
        shots: vec![ShotData {
            length: 4.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    Database::open(path)
        .expect("create")
        .save_project(&project)
        .expect("write");
}

/// The server's working tree at the tip: one `.stalx` entry, as SpeleoDB zips it.
pub(super) fn dump_at_tip() -> Vec<u8> {
    let temp = TempDir::new().expect("temp");
    let server = temp.path().join("server.stalx");
    project_file(&server, "Server Copy", "ServerSection");
    let bytes = std::fs::read(&server).expect("read the server project");
    let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .start_file("misc/project.stalx", opts)
        .expect("start entry");
    writer.write_all(&bytes).expect("write entry");
    writer.finish().expect("finish zip").into_inner()
}

/// A loopback stub routing by path - the dump at a hash gets `zip`, everything
/// else [`project_json`] - and recording every request line. Keep the listener
/// bound for the whole test.
pub(super) fn routing_stub(
    zip: Vec<u8>,
) -> (String, std::net::TcpListener, Arc<Mutex<Vec<String>>>) {
    routing_stub_answering(zip, project_json())
}

/// [`routing_stub`] answering every JSON request with `json`.
pub(super) fn routing_stub_answering(
    zip: Vec<u8>,
    json: String,
) -> (String, std::net::TcpListener, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let accept_on = listener.try_clone().expect("clone listener");
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = accept_on.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
            let mut request = Vec::new();
            let mut chunk = vec![0u8; 8 * 1024];
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
            let (content_type, body): (&str, &[u8]) = if line.contains("/download/dump/") {
                ("application/zip", &zip)
            } else {
                ("application/json", json.as_bytes())
            };
            if let Ok(mut lines) = log.lock() {
                lines.push(line);
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), listener, seen)
}

/// How long a child-process test body may run before it is killed and its test
/// fails. Each takes well under a second; this is only ever a hang.
pub(super) const CHILD_DEADLINE: Duration = Duration::from_mins(2);

/// [#576 RV8a-F1] `command`'s output, as `Command::output` gives it, except that a
/// child still running after `limit` is killed and the test fails with what it
/// printed so far. `output()` waits forever, so a deadlock in the verb under test
/// hung this test - and the whole test run - until the CI job timeout, and left
/// the child orphaned if the harness then killed the parent.
pub(super) fn output_within(command: &mut ChildCommand, limit: Duration) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the child process");
    // Both pipes drain on their own threads, so a child that prints more than a
    // pipe holds cannot stall on a full one while this side waits for it to exit.
    let mut stdout = child.stdout.take().expect("the child's stdout");
    let mut stderr = child.stderr.take().expect("the child's stderr");
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the child process") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = out.join().unwrap_or_default();
            let stderr = err.join().unwrap_or_default();
            panic!(
                "the child process was still running after {limit:?} and was killed:\n{}\n{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Output {
        status,
        stdout: out.join().expect("the child's stdout"),
        stderr: err.join().expect("the child's stderr"),
    }
}

/// The environment variable that makes [`a_child_that_never_finishes`] hang.
const SLEEPER_ENV: &str = "STALAGMIGHT_CHILD_SLEEPER";
const SLEEPER_TEST: &str = "core::state::take_remote_success_tests::a_child_that_never_finishes";

#[test]
#[ignore = "the hung child of a_child_past_its_deadline_is_killed_and_fails_its_test: it \
            sleeps when its env var is set"]
fn a_child_that_never_finishes() {
    if std::env::var_os(SLEEPER_ENV).is_some() {
        std::thread::sleep(Duration::from_secs(20));
    }
}

#[test]
fn a_child_past_its_deadline_is_killed_and_fails_its_test() {
    let exe = std::env::current_exe().expect("this test binary");
    let mut command = ChildCommand::new(exe);
    command
        .args([SLEEPER_TEST, "--exact", "--ignored", "--nocapture"])
        .env(SLEEPER_ENV, "1");
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        output_within(&mut command, Duration::from_millis(500))
    }));
    let elapsed = started.elapsed();
    println!(
        "[RV8a-F1] a child that sleeps 20 s, waited for with a 500 ms deadline: failed the \
         test: {}; returned after {elapsed:?}",
        outcome.is_err()
    );
    assert!(outcome.is_err(), "a child past its deadline fails the test");
    assert!(
        elapsed < Duration::from_secs(10),
        "it is killed at the deadline, not waited for: {elapsed:?}"
    );
}

#[test]
fn take_remote_lands_the_server_tip_and_discards_the_local_edits() {
    let exe = std::env::current_exe().expect("this test binary");
    let output = output_within(
        ChildCommand::new(exe)
            .args([
                CHILD_TEST,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1"),
        CHILD_DEADLINE,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{stdout}");
    assert!(
        output.status.success(),
        "the take-remote body failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed"),
        "the body ran (and was not filtered out): {stdout}"
    );
}

#[test]
#[ignore = "the child-process body of take_remote_lands_the_server_tip_and_discards_the_local_edits: \
            it sets the process-wide app dirs, so it runs only in a process of its own"]
fn take_remote_body_in_a_process_of_its_own() {
    if std::env::var_os(CHILD_ENV).is_none() {
        println!("not in the child process ({CHILD_ENV} unset): nothing to do");
        return;
    }
    let _cfg = crate::test_helpers::isolated_config();
    let dirs = TempDir::new().expect("app dirs");
    crate::app_dirs::init_paths(
        dirs.path().join("data"),
        dirs.path().join("config"),
        dirs.path().join("cache"),
        None,
    );
    let cache = super::speleodb_local_state::cache_root().expect("a cache root in this process");
    let dest = super::speleodb_local_state::cache_path_in(&cache, REMOTE_ID);
    std::fs::create_dir_all(dest.parent().expect("cache dir")).expect("create cache dir");

    // The canonical cache copy, linked at BASE, opened mid edit session.
    project_file(&dest, "Local Copy", "LocalSection");
    Database::open_existing(&dest)
        .expect("reopen")
        .save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://placeholder.invalid/".to_owned(),
            project_id: REMOTE_ID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some(BASE_SHA.to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: Some(chrono::Utc::now().to_rfc3339()),
            undo_mode: Some(UndoMode::Full),
        }))
        .expect("link");
    let (base, _listener, seen) = routing_stub(dump_at_tip());
    let state = state_with_null_sink();
    assert!(
        state.open_project(dest.clone()).success,
        "open the cache copy"
    );
    *state.speleodb.lock() = authed_session_at(&base);
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(REMOTE_ID));
    let edit = state.execute(Command::CreateSection {
        name: Some("LocalEdit".to_owned()),
        surveyor: None,
        date: None,
        color: "#abcdef".to_owned(),
        direction: None,
        shots: vec![ShotData {
            length: 2.0,
            ..Default::default()
        }],
    });
    assert!(edit.success, "the local edit: {:?}", edit.error);
    assert!(
        state.speleodb_open_link().expect("link").has_unpushed_edits,
        "precondition: the copy holds a local edit"
    );

    let taken = state.speleodb_take_remote();

    let link = state
        .speleodb_open_link()
        .expect("the reloaded copy is linked");
    let project = state.get_project_state().expect("a project is open");
    let sections: Vec<Option<String>> = project.sections.iter().map(|s| s.name.clone()).collect();
    let requests = seen.lock().expect("request log").clone();
    let released = requests
        .iter()
        .any(|r| r.starts_with("POST") && r.contains(&format!("/projects/{REMOTE_ID}/release/")));
    println!(
        "take remote -> {taken:?}; BASE {:?}, dirty {}, last edit {:?}; editable {}; project \
         {:?} sections {sections:?}; undo offered {}; release issued {released}; requests \
         {requests:?}",
        link.base_commit_sha,
        link.has_unpushed_edits,
        link.last_edit_at,
        state.edit_enabled(),
        project.name,
        state.can_undo(),
    );
    assert_eq!(
        taken.as_deref().ok(),
        dest.to_str(),
        "answers the reloaded cache path"
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some(TIP_SHA), "BASE = tip");
    assert!(!link.has_unpushed_edits, "the dirty bit is cleared");
    assert!(link.last_edit_at.is_none());
    assert!(!state.edit_enabled(), "the project is read-only");
    assert!(released, "the held lock was released: {requests:?}");
    assert!(
        requests
            .iter()
            .any(|r| r.contains(&format!("/download/dump/{TIP_SHA}/"))),
        "the dump was fetched at the tip: {requests:?}"
    );
    assert_eq!(
        sections,
        [Some("ServerSection".to_owned())],
        "the local edit is gone; the server's tree is open"
    );
    assert_eq!(project.name, "Server Copy");
    assert!(!state.can_undo(), "no history over the server tip");
}
