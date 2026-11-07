//! [#576 NF-A8R-GAPS] The two fast-forward arms driven end to end: Start editing
//! and Update, each over a clean linked copy whose BASE the server tip moved past.
//!
//! Their ORDERS are typed (`start_order`, `update_restore`, `PinnedDump`) and their
//! decisions unit-tested, but neither arm was ever run past the download: both
//! resolve the cache path through the process-wide app dirs, a `OnceLock` other
//! tests in this binary rely on staying unset. So, as in `take_remote_success_tests`
//! (whose loopback stub and synthetic projects this reuses), each scenario runs in a
//! process of its own: the tests below re-run this test binary on the ignored,
//! env-gated body, killed at the deadline if it hangs.
//!
//! The stub answers like SpeleoDB: the revision history (a tip another account
//! pushed on top of our BASE), the lock (unheld before Start editing takes it; our
//! own account's under Update), and the dump at the tip. After either verb the open project is the server's copy, BASE is the
//! tip, the copy is clean, the session is editable and its claim is recorded; the
//! dump was fetched once, at the tip's hash.
//!
//! All data is synthetic, built in code.

use std::process::Command as ChildCommand;

use tempfile::TempDir;

use super::speleodb::{authed_session_at, REMOTE_ID};
use super::take_remote_success_tests::{
    dump_at_tip, output_within, project_file, project_json_held_by, routing_stub,
    routing_stub_answering, BASE_SHA, CHILD_DEADLINE, TIP_SHA,
};
use crate::models::{SpeleoLink, UndoMode};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// The environment variable that carries the child's scenario.
const CHILD_ENV: &str = "STALAGMIGHT_FF_ARM_CHILD";
/// The body's full test name, for the child's exact filter.
const CHILD_TEST: &str = "core::state::sdb_ff_arm_tests::ff_arm_body_in_a_process_of_its_own";

/// Re-run this test binary on the child body with `scenario`.
fn run_child(scenario: &str) {
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
            .env(CHILD_ENV, scenario),
        CHILD_DEADLINE,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{stdout}");
    assert!(
        output.status.success(),
        "the {scenario} body failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed"),
        "the body ran (and was not filtered out): {stdout}"
    );
}

#[test]
fn start_editing_fast_forwards_a_clean_copy_to_the_tip_and_opens_the_session() {
    run_child("start_editing");
}

#[test]
fn update_fast_forwards_an_edit_session_to_the_tip_and_keeps_it_editable() {
    run_child("update");
}

#[test]
#[ignore = "the child-process body of the fast-forward arm tests: it sets the process-wide \
            app dirs, so it runs only in a process of its own"]
fn ff_arm_body_in_a_process_of_its_own() {
    let Some(scenario) = std::env::var_os(CHILD_ENV) else {
        println!("not in the child process ({CHILD_ENV} unset): nothing to do");
        return;
    };
    let scenario = scenario.to_string_lossy().into_owned();
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

    // The canonical cache copy, linked at BASE and clean. Start editing finds the
    // lock unheld (so its acquire is this session's, and records a claim); Update
    // runs inside an edit session our account already holds it for.
    let (base, _listener, seen) = if scenario == "start_editing" {
        routing_stub_answering(dump_at_tip(), project_json_held_by(""))
    } else {
        routing_stub(dump_at_tip())
    };
    project_file(&dest, "Local Copy", "LocalSection");
    Database::open_existing(&dest)
        .expect("reopen")
        .save_speleo_link(Some(&SpeleoLink {
            endpoint: base.clone(),
            project_id: REMOTE_ID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some(BASE_SHA.to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: Some(chrono::Utc::now().to_rfc3339()),
            undo_mode: Some(UndoMode::Full),
        }))
        .expect("link");
    let state = state_with_null_sink();
    assert!(state.open_project(dest).success, "open the cache copy");
    *state.speleodb.lock() = authed_session_at(&base);

    let answer = match scenario.as_str() {
        // Read-only: Start editing takes the lock, finds the tip past BASE, and
        // fast-forwards before it opens the session.
        "start_editing" => state.speleodb_start_editing().map(|r| format!("{r:?}")),
        // Mid edit session under our lock: Update fast-forwards and gives the
        // session back.
        "update" => {
            state.set_edit_enabled(true);
            state.speleodb_note_lock_acquired(Some(REMOTE_ID));
            state.speleodb_update().map(|r| format!("{r:?}"))
        }
        other => panic!("unknown scenario {other}"),
    };

    let link = state.speleodb_open_link().ok();
    let project = state.get_project_state();
    let sections: Option<Vec<Option<String>>> = project
        .as_ref()
        .map(|p| p.sections.iter().map(|s| s.name.clone()).collect());
    let requests = seen.lock().expect("request log").clone();
    let downloads: Vec<&String> = requests
        .iter()
        .filter(|r| r.contains("/download/dump/"))
        .collect();
    let editable = state.edit_enabled();
    let claimed = state.speleodb_lock_acquired_here(REMOTE_ID);
    println!(
        "[NF-A8R-GAPS] {scenario} over a clean copy at BASE, tip past it -> {:?}; BASE {:?}, \
         dirty {:?}; project {:?} sections {sections:?}; editable {editable}, claim recorded \
         {claimed}; downloads {downloads:?}",
        answer.as_ref().map_err(|e| (e.code, e.message.clone())),
        link.as_ref().map(|l| l.base_commit_sha.clone()),
        link.as_ref().map(|l| l.has_unpushed_edits),
        project.as_ref().map(|p| p.name.clone()),
    );

    let answer = answer.expect("the fast-forward arm succeeds");
    if scenario == "update" {
        assert!(
            answer.starts_with("FastForwarded"),
            "update took the fast-forward arm: {answer}"
        );
    }
    assert_eq!(
        downloads.len(),
        1,
        "the dump was fetched once: {requests:?}"
    );
    assert!(
        downloads
            .first()
            .is_some_and(|line| line.contains(&format!("/download/dump/{TIP_SHA}/"))),
        "at the tip's hash: {downloads:?}"
    );
    let link = link.expect("the reloaded copy is linked");
    assert_eq!(
        link.base_commit_sha.as_deref(),
        Some(TIP_SHA),
        "BASE is the tip"
    );
    assert!(!link.has_unpushed_edits, "and the copy is clean");
    assert_eq!(
        sections,
        Some(vec![Some("ServerSection".to_owned())]),
        "the open project is the server's copy"
    );
    assert!(editable, "the edit session is open");
    assert!(claimed, "under a recorded claim on the lock");
}
