//! [#576 SDB-DEL-D1] Delete project on SpeleoDB leaves the local copy the way Unlink
//! does: out of the SpeleoDB cache at a path the user picks, or - when they cancel
//! that Save As - unlinked in the cache, where the download guard keeps it.
//!
//! The admin Delete soft-deletes the server project and drops the copy's link in
//! place (`speleodb_delete_project` -> `speleodb_unlink_in_place`). The frontend then
//! asks the Unlink Save As and runs the Unlink move (`speleodb_unlink`) on the now
//! link-less copy, and loads the moved copy. Driven end to end against a loopback
//! stub that answers like SpeleoDB:
//!
//! 1. Delete, then the move: the copy (survey and an unsaved edit) lands at the
//!    picked path, unlinked; the cache copy is gone; the session on it closed; the
//!    held lock was handed back; and once the copy is loaded, the session and the
//!    recent list name it, not the cache path.
//! 2. Delete, then Cancel: the copy stays at the cache path, unlinked, and a re-open
//!    from SpeleoDB is refused, flagged `unlinked`, with the copy untouched.
//!
//! The verbs resolve the cache path through the process-wide app dirs, so - as in
//! `sdb_unlink_move_tests`, whose helpers this reuses - the body runs in a process of
//! its own.
//!
//! All data is synthetic, built in code.

use std::path::Path;

use tempfile::TempDir;

use super::sdb_unlink_move_tests::{edit, linked_at, recents, releases, sections_at};
use super::speleodb::{authed_session_at, REMOTE_ID};
use super::take_remote_success_tests::{
    dump_at_tip, output_within, project_file, routing_stub, BASE_SHA, CHILD_DEADLINE, TIP_SHA,
};
use super::AppState;
use crate::core::Command;
use crate::models::{SpeleoLink, UndoMode};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// The environment variable that lets the child-process body run.
const CHILD_ENV: &str = "STALAGMIGHT_DELETE_MOVE_CHILD";
/// The body's full test name, for the child's exact filter.
const CHILD_TEST: &str =
    "core::state::sdb_delete_move_tests::delete_move_body_in_a_process_of_its_own";

/// How many DELETE requests the stub has seen for the project.
fn deletes(seen: &std::sync::Mutex<Vec<String>>) -> usize {
    seen.lock()
        .map(|lines| {
            lines
                .iter()
                .filter(|r| r.starts_with(&format!("DELETE /api/v2/projects/{REMOTE_ID}/")))
                .count()
        })
        .unwrap_or_default()
}

/// The managed copy at its cache path, linked at BASE to the stub's server, open mid
/// edit session under a claimed lock, with an in-memory edit nobody has saved.
fn open_managed_copy(state: &AppState, cache: &Path, base: &str) {
    // A move out of the cache removes the per-project folder with the copy.
    std::fs::create_dir_all(cache.parent().expect("cache dir")).expect("create cache dir");
    project_file(cache, "Local Copy", "LocalSection");
    Database::open_existing(cache)
        .expect("reopen")
        .save_speleo_link(Some(&SpeleoLink {
            endpoint: base.to_owned(),
            project_id: REMOTE_ID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some(BASE_SHA.to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: Some(chrono::Utc::now().to_rfc3339()),
            undo_mode: Some(UndoMode::Full),
        }))
        .expect("link");
    assert!(
        state.open_project(cache.to_path_buf()).success,
        "open the cache copy"
    );
    *state.speleodb.lock() = authed_session_at(base);
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(REMOTE_ID));
    edit(state, "LocalEdit");
}

#[test]
fn delete_project_then_the_unlink_move_keeps_the_copy_out_of_the_cache() {
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
        "the delete-move body failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the body ran (and was not filtered out): {stdout}"
    );
}

#[test]
#[ignore = "the child-process body of \
            delete_project_then_the_unlink_move_keeps_the_copy_out_of_the_cache: it sets \
            the process-wide app dirs, so it runs only in a process of its own"]
fn delete_move_body_in_a_process_of_its_own() {
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
    let root = super::speleodb_local_state::cache_root().expect("a cache root in this process");
    let cache = super::speleodb_local_state::cache_path_in(&root, REMOTE_ID);
    let projects = dirs.path().join("Documents/stalagmight");
    std::fs::create_dir_all(&projects).expect("projects folder");
    let (base, _listener, seen) = routing_stub(dump_at_tip());
    let state = state_with_null_sink();

    // 1. Delete, then the Save As answers a path: the Unlink move.
    open_managed_copy(&state, &cache, &base);
    let deleted = state.speleodb_delete_project();
    println!(
        "[SDB-DEL-D1] delete -> {:?}; DELETEs {}; releases {}; cache copy linked {}; open {:?}",
        deleted.as_ref().map_err(|e| &e.message),
        deletes(&seen),
        releases(&seen),
        linked_at(&cache),
        state.open_project_path(),
    );
    assert!(deleted.is_ok(), "the delete answers Ok");
    assert_eq!(deletes(&seen), 1, "the server project was deleted");
    assert_eq!(releases(&seen), 1, "the held lock was handed back");
    assert!(
        cache.exists() && !linked_at(&cache),
        "the copy is still in the cache, unlinked"
    );
    assert_eq!(state.open_project_path(), Some(cache.clone()), "still open");

    let kept = projects.join("Cenote_A.stalx");
    let moved = state
        .speleodb_unlink(kept.clone())
        .map_err(|e| e.message)
        .expect("the move answers Ok");
    println!(
        "[SDB-DEL-D1] move -> {moved:?}; cache copy left: {}; copy sections {:?}, linked {}; \
         open {:?}",
        cache.exists(),
        sections_at(&kept),
        linked_at(&kept),
        state.open_project_path(),
    );
    assert_eq!(
        moved.saved_path.as_deref(),
        kept.to_str(),
        "answers the copy"
    );
    assert!(!cache.exists(), "the copy left the cache");
    assert_eq!(
        sections_at(&kept),
        ["LocalSection", "LocalEdit"],
        "the copy holds the survey and the unsaved edit"
    );
    assert!(!linked_at(&kept), "and is not linked to SpeleoDB");
    assert_eq!(
        state.open_project_path(),
        None,
        "the session on the cache file closed"
    );
    // The frontend then loads the moved copy: the session and the recent list follow.
    assert!(state.open_project(kept.clone()).success, "load the copy");
    let recent = recents(&state);
    println!(
        "[SDB-DEL-D1] loaded -> open {:?}; recents {recent:?}",
        state.open_project_path()
    );
    assert_eq!(state.open_project_path(), Some(kept.clone()));
    assert!(
        recent.contains(&kept) && !recent.contains(&cache),
        "the recent entry follows the copy"
    );
    assert_eq!(releases(&seen), 1, "no second release");
    let closed = state.execute(Command::CloseProject);
    assert!(closed.success, "close the moved copy");

    // 2. Delete again, then the Save As is cancelled: nothing moves.
    open_managed_copy(&state, &cache, &base);
    assert!(state.speleodb_delete_project().is_ok(), "the delete");
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(
        saved.success,
        "the autosave at the cache path: {:?}",
        saved.error
    );
    let closed = state.execute(Command::CloseProject);
    assert!(closed.success, "close");
    let refused = state.speleodb_open_project(
        REMOTE_ID.to_owned(),
        "Remote".to_owned(),
        Some(TIP_SHA.to_owned()),
        None,
    );
    let payload = refused
        .as_ref()
        .err()
        .and_then(|e| e.unpushed_local.clone());
    println!(
        "[SDB-DEL-D1] cancelled; re-open -> {:?}; payload {payload:?}; cache sections {:?}, \
         linked {}",
        refused.as_ref().map(|_| ()).map_err(|e| &e.message),
        sections_at(&cache),
        linked_at(&cache),
    );
    assert!(
        payload.as_ref().is_some_and(|p| p.unlinked
            && p.project_id == REMOTE_ID
            && Path::new(&p.local_path) == cache),
        "the re-open is refused with the keep-local / take-remote prompt, flagged unlinked"
    );
    assert_eq!(
        sections_at(&cache),
        ["LocalSection", "LocalEdit"],
        "and the copy left in the cache is untouched"
    );
}
