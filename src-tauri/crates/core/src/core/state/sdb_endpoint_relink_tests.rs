//! [#576 W89] Update, View, Restore, Take remote and Discard changes refuse a copy
//! linked to another SpeleoDB server than the one signed in.
//!
//! Each downloads the SIGNED-IN server's project under the copy's project id,
//! replaces the cached copy with it and re-stamps the link with the session's
//! server. So a copy of server X, open while signed in to server Y, became a copy
//! of Y's project of that id, and Take remote and Discard changes threw the local
//! edits away for it. Each is now refused before anything is sent, with the copy,
//! its link and its edits left as they were (the refusal Start editing gives, W88).
//! The same server spelled another way - the case of the scheme, a trailing slash -
//! is still the same server, and every verb runs as before.
//!
//! The verbs resolve the cache path through the process-wide app dirs, a `OnceLock`
//! other tests in this binary rely on staying unset. So, as in
//! `take_remote_success_tests` (whose loopback stub and synthetic projects this
//! reuses), each run is a process of its own: the tests below re-run this test
//! binary on the ignored, env-gated body, killed at the deadline if it hangs.
//!
//! All data is synthetic, built in code.

use std::process::Command as ChildCommand;

use tempfile::TempDir;

use super::speleodb::{authed_session_at, link_open_copy_to, REMOTE_ID};
use super::take_remote_success_tests::{
    dump_at_tip, output_within, project_file, project_json_held_by, routing_stub_answering,
    BASE_SHA, CHILD_DEADLINE,
};
use crate::core::Command;
use crate::models::{CommandErrorCode, ShotData, SpeleoLink, UndoMode};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// The environment variable that carries the child's `verb:linked` scenario.
const CHILD_ENV: &str = "STALAGMIGHT_W89_RELINK_CHILD";
/// The body's full test name, for the child's exact filter.
const CHILD_TEST: &str =
    "core::state::sdb_endpoint_relink_tests::relink_body_in_a_process_of_its_own";
/// The server the copy is linked to in the mismatch case (never reachable).
const LINKED_TO: &str = "http://x.invalid/";

/// `endpoint` spelled another way: the scheme upper-cased, a trailing slash added.
fn spelled_another_way(endpoint: &str) -> String {
    format!(
        "{}/",
        endpoint.trim_end_matches('/').replacen("http", "HTTP", 1)
    )
}

/// Re-run this test binary on the child body for `verb`, with the copy linked to
/// another server (`other`) or to the signed-in one spelled another way (`same`).
fn run_child(verb: &str, linked: &str) {
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
            .env(CHILD_ENV, format!("{verb}:{linked}")),
        CHILD_DEADLINE,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{stdout}");
    assert!(
        output.status.success(),
        "the {verb}:{linked} body failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed"),
        "the body ran (and was not filtered out): {stdout}"
    );
}

/// The verb over a copy of another server, then the same-server control.
fn refused_elsewhere_and_run_here(verb: &str) {
    run_child(verb, "other");
    run_child(verb, "same");
}

#[test]
fn update_refuses_a_copy_linked_to_another_server() {
    refused_elsewhere_and_run_here("update");
}

#[test]
fn view_refuses_a_copy_linked_to_another_server() {
    refused_elsewhere_and_run_here("view");
}

#[test]
fn restore_refuses_a_copy_linked_to_another_server() {
    refused_elsewhere_and_run_here("restore");
}

#[test]
fn take_remote_refuses_a_copy_linked_to_another_server() {
    refused_elsewhere_and_run_here("take_remote");
}

#[test]
fn discard_changes_refuses_a_copy_linked_to_another_server() {
    refused_elsewhere_and_run_here("discard");
}

/// The open project's section names.
fn sections(state: &super::AppState) -> Vec<Option<String>> {
    state
        .get_project_state()
        .expect("a project is open")
        .sections
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

#[test]
#[ignore = "the child-process body of the W89 re-link tests: it sets the process-wide app \
            dirs, so it runs only in a process of its own"]
fn relink_body_in_a_process_of_its_own() {
    let Some(scenario) = std::env::var_os(CHILD_ENV) else {
        println!("not in the child process ({CHILD_ENV} unset): nothing to do");
        return;
    };
    let scenario = scenario.to_string_lossy().into_owned();
    let (verb, linked) = scenario.split_once(':').expect("a verb:linked scenario");
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

    // The canonical cache copy, clean at BASE. The signed-in server answers like
    // SpeleoDB for a project of the same id: a tip past BASE, our account holding the
    // lock, and the dump of its own tree ("ServerSection").
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
    let (server, _listener, seen) = routing_stub_answering(
        dump_at_tip(),
        project_json_held_by(r#""active_mutex":{"user":"u@x"},"#),
    );
    let state = state_with_null_sink();
    assert!(state.open_project(dest).success, "open the cache copy");
    *state.speleodb.lock() = authed_session_at(&server);
    let copy_on = match linked {
        "other" => LINKED_TO.to_owned(),
        "same" => spelled_another_way(&server),
        other => panic!("unknown link {other}"),
    };
    link_open_copy_to(&state, &copy_on);

    // Restore, Take remote and Discard changes run in an edit session under our lock;
    // the last two over a local edit, which is what they throw away.
    if matches!(verb, "restore" | "take_remote" | "discard") {
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));
    }
    if matches!(verb, "take_remote" | "discard") {
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
    }
    let before = sections(&state);
    let revision = |restore: bool| {
        state.speleodb_restore_revision(
            REMOTE_ID.to_owned(),
            "Remote".to_owned(),
            BASE_SHA.to_owned(),
            restore,
            false,
            None,
            false,
            false,
            false,
        )
    };

    let answer = match verb {
        "update" => state.speleodb_update().map(|r| format!("{r:?}")),
        "view" => revision(false).map(|r| format!("{r:?}")),
        "restore" => revision(true).map(|r| format!("{r:?}")),
        "take_remote" => state.speleodb_take_remote(),
        "discard" => state
            .speleodb_discard_changes(false, true)
            .map(|r| format!("{r:?}")),
        other => panic!("unknown verb {other}"),
    }
    .map_err(|e| (e.code, e.message));

    let link = state
        .speleodb_open_link()
        .expect("the copy is still linked");
    let after = sections(&state);
    let requests = seen.lock().expect("request log").clone();
    println!(
        "[W89 item 2, {verb}] copy linked to {copy_on}, signed in to {server}: {answer:?}; \
         link now {} at BASE {:?}, dirty {}; sections {before:?} -> {after:?}; requests to \
         the signed-in server {requests:?}",
        link.endpoint, link.base_commit_sha, link.has_unpushed_edits
    );

    if linked == "same" {
        assert!(
            answer.is_ok(),
            "the same server spelled another way still runs {verb}: {answer:?}"
        );
        assert!(!requests.is_empty(), "and reaches that server");
        return;
    }
    assert!(
        requests.is_empty(),
        "{verb}: nothing reaches the signed-in server for a copy of another server \
         ({requests:?})"
    );
    let (code, message) = answer.expect_err("a copy linked to another server is refused");
    assert_eq!(code, CommandErrorCode::SpeleodbFatal);
    assert!(
        message.contains(LINKED_TO) && message.contains("sign in to that server"),
        "the refusal names the copy's server and says what to do: {message}"
    );
    assert_eq!(
        link.endpoint, LINKED_TO,
        "the copy stays linked to its own server"
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some(BASE_SHA));
    assert_eq!(
        after, before,
        "the local copy is untouched, local edits included"
    );
}
