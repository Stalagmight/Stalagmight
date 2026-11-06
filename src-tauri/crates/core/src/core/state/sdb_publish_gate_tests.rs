//! [#576 FZ-SDB-2] A publish leaves the now-linked project read-only.
//!
//! Publish starts from the open LOCAL project, whose edit gate is open (a local
//! project edits freely). It creates the server project, relocates the copy into
//! the SpeleoDB cache and links it, then takes the lock, uploads and releases -
//! best-effort, so a refused acquire is logged and the verb still answers Ok. It
//! used to return with the gate still open: a LINKED project editing without its
//! lock held. Every other path leaves a linked project read-only until Start
//! editing takes the lock (an opened linked project loads read-only; Done shuts the
//! gate after its release).
//!
//! Driven end to end against a loopback stub that answers like SpeleoDB, once per
//! arm of the initial push: the acquire refused (no upload goes out), and the
//! upload landed and the lock released. After each the gate is shut, no force-edit
//! is on, and no claim on the new project's lock is left.
//!
//! The verb resolves the cache path through the process-wide app dirs, a `OnceLock`
//! other tests in this binary rely on staying unset, so - as in
//! `take_remote_success_tests`, whose child-process, deadline and project-file
//! helpers this reuses - the body runs in a process of its own.
//!
//! All data is synthetic, built in code.

use std::path::Path;

use tempfile::TempDir;

use super::speleodb::{authed_session_at, routing_stub};
use super::take_remote_success_tests::{output_within, project_file, CHILD_DEADLINE};
use crate::speleodb::{SpeleoPublishParams, Visibility};
use crate::test_fixtures::state_with_null_sink;

/// The environment variable that lets the child-process body run.
const CHILD_ENV: &str = "STALAGMIGHT_PUBLISH_GATE_CHILD";
/// The body's full test name, for the child's exact filter.
const CHILD_TEST: &str =
    "core::state::sdb_publish_gate_tests::publish_gate_body_in_a_process_of_its_own";

/// The project the refused-acquire server creates, and how it describes it.
const REFUSED_ID: &str = "44444444-4444-4444-4444-444444444444";
const REFUSED_PROJECT: &str = r#"{"id":"44444444-4444-4444-4444-444444444444","name":"Refused"}"#;

/// The project the landed-upload server creates, how it describes it, and the
/// commit it answers the upload with.
const LANDED_ID: &str = "55555555-5555-5555-5555-555555555555";
const LANDED_PROJECT: &str = r#"{"id":"55555555-5555-5555-5555-555555555555","name":"Landed"}"#;
const LANDED_SHA: &str = "0123450000000000000000000000000000000000";

/// The request line's path.
fn path_of(line: &str) -> &str {
    line.split_whitespace().nth(1).unwrap_or_default()
}

/// A server where somebody took the new project's lock first: the create answers
/// 201, the acquire is refused (SpeleoDB's lock contention is a 400), the history
/// is empty, and any other read describes the project.
fn refused_acquire(line: &str) -> (u16, &'static str) {
    let path = path_of(line);
    if path.ends_with("/acquire/") {
        (400, r#"{"detail":"locked"}"#)
    } else if path.ends_with("/revisions/") {
        (200, r#"{"commits":[]}"#)
    } else if line.starts_with("POST /api/v2/projects/ ") {
        (201, REFUSED_PROJECT)
    } else {
        (200, REFUSED_PROJECT)
    }
}

/// A server where the initial push goes through: the create answers 201, the
/// acquire and release succeed, and the upload lands [`LANDED_SHA`].
fn landed_upload(line: &str) -> (u16, &'static str) {
    let path = path_of(line);
    if line.starts_with("PUT ") && path.ends_with("/upload/auto/") {
        (
            200,
            r#"{"hexsha":"0123450000000000000000000000000000000000",
                "project":{"id":"55555555-5555-5555-5555-555555555555","name":"Landed"}}"#,
        )
    } else if path.ends_with("/revisions/") {
        (
            200,
            r#"{"commits":[{"id":"0123450000000000000000000000000000000000",
                "parent_ids":[],"author_email":"u@x"}]}"#,
        )
    } else if line.starts_with("POST /api/v2/projects/ ") {
        (201, LANDED_PROJECT)
    } else {
        (200, LANDED_PROJECT)
    }
}

/// What one publish left behind.
#[derive(Debug)]
struct Published {
    project_id: String,
    edit_enabled: bool,
    force_editing: bool,
    claim_kept: bool,
    /// The open project's link: (project id, BASE).
    link: Option<(String, Option<String>)>,
    requests: Vec<String>,
}

/// Open a fresh local project named `name` in `dir`, sign in to a stub answering
/// with `route`, and publish it.
fn publish_against(dir: &Path, name: &str, route: fn(&str) -> (u16, &'static str)) -> Published {
    let path = dir.join(format!("{name}.stalx"));
    project_file(&path, name, "SectionA");
    let state = state_with_null_sink();
    assert!(state.open_project(path).success, "open the local project");
    assert!(
        state.edit_enabled() && !state.force_editing(),
        "an open local project edits freely before the publish"
    );
    let (base, _listener, seen) = routing_stub(route);
    *state.speleodb.lock() = authed_session_at(&base);

    let report = state
        .speleodb_publish(SpeleoPublishParams {
            name: name.to_owned(),
            description: "a synthetic survey".to_owned(),
            country: "US".to_owned(),
            visibility: Visibility::Private,
            latitude: None,
            longitude: None,
        })
        .map_err(|e| e.message)
        .expect("the publish answers Ok");

    let requests = seen.lock().expect("request log").clone();
    Published {
        edit_enabled: state.edit_enabled(),
        force_editing: state.force_editing(),
        claim_kept: state.speleodb_lock_acquired_here(&report.result.project_id),
        link: state
            .speleodb_open_link()
            .ok()
            .map(|l| (l.project_id, l.base_commit_sha)),
        requests,
        project_id: report.result.project_id,
    }
}

#[test]
fn a_published_project_comes_back_read_only() {
    let exe = std::env::current_exe().expect("this test binary");
    let output = output_within(
        std::process::Command::new(exe)
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
    println!("{stdout}");
    assert!(
        output.status.success(),
        "the publish body failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the body ran (and was not filtered out): {stdout}"
    );
}

#[test]
#[ignore = "the child-process body of a_published_project_comes_back_read_only: it sets the \
            process-wide app dirs, so it runs only in a process of its own"]
fn publish_gate_body_in_a_process_of_its_own() {
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

    let refused = publish_against(dirs.path(), "Refused", refused_acquire);
    let landed = publish_against(dirs.path(), "Landed", landed_upload);
    println!("[FZ-SDB-2] refused acquire -> {refused:?}");
    println!("[FZ-SDB-2] landed upload -> {landed:?}");

    // Each arm was reached: the project is created and linked either way; the
    // refused acquire sent no upload, the landed one uploaded, then released.
    assert_eq!(
        refused.project_id, REFUSED_ID,
        "the server's id is answered"
    );
    assert_eq!(
        refused.link,
        Some((REFUSED_ID.to_owned(), None)),
        "the refused-acquire publish is linked, over the empty history"
    );
    assert!(
        !refused.requests.iter().any(|r| r.starts_with("PUT ")),
        "no upload follows a refused acquire: {:?}",
        refused.requests
    );
    assert_eq!(landed.project_id, LANDED_ID, "the server's id is answered");
    assert_eq!(
        landed.link,
        Some((LANDED_ID.to_owned(), Some(LANDED_SHA.to_owned()))),
        "the landed publish is linked at the pushed commit"
    );
    let upload_at = landed.requests.iter().position(|r| r.starts_with("PUT "));
    let release_at = landed
        .requests
        .iter()
        .position(|r| r.starts_with("POST ") && r.contains("/release/"));
    assert!(
        upload_at.is_some() && release_at > upload_at,
        "the upload went out and the lock was released after it: {:?}",
        landed.requests
    );

    // The gate: a linked project whose lock is not held is read-only.
    assert_eq!(
        (refused.edit_enabled, landed.edit_enabled),
        (false, false),
        "a published project is linked without its lock held, so its edit gate is shut \
         (refused acquire, landed upload)"
    );
    assert_eq!(
        (refused.force_editing, landed.force_editing),
        (false, false),
        "and it is not force-editing"
    );
    assert_eq!(
        (refused.claim_kept, landed.claim_kept),
        (false, false),
        "and no claim on its lock is left"
    );
}
