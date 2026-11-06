//! [#576 FZ-SDB-5] Unlink moves the copy out of the SpeleoDB cache, and a link-less
//! copy an older Unlink left there is never replaced without asking.
//!
//! Unlink used to drop the copy's link row IN PLACE, leaving a plain `.stalx` at the
//! project's cache path - its download slot. The next Open of the same project from
//! SpeleoDB then replaced it, and every local edit in it was gone: the un-pushed
//! edits guard reads the link row, and there was none.
//!
//! Driven end to end against a loopback stub that answers like SpeleoDB:
//!
//! 1. A refused Unlink (destination inside the cache, destination folder missing)
//!    changes nothing: the session, the cache copy, its link, the gate, the claim
//!    and the server lock are all as they were.
//! 2. Unlink to a path outside the cache writes the copy there (unlinked, with the
//!    in-memory edit), closes the session on the cache copy, removes it, moves the
//!    recent entry and hands the lock back. A re-open from SpeleoDB then pulls into
//!    an empty slot and the moved copy keeps the work (decisions D1/D2).
//! 3. A link-less copy at the cache path holding work (what an Unlink made before
//!    this change left there) makes the re-open fail closed, flagged `unlinked`,
//!    with the file untouched; keep-local (open it, Unlink it) moves it out with the
//!    work, and the re-open then succeeds (decision D3).
//!
//! The verbs resolve the cache path through the process-wide app dirs, a `OnceLock`
//! other tests in this binary rely on staying unset, so the body runs in a process
//! of its own, as in `take_remote_success_tests`, whose helpers this reuses.
//!
//! All data is synthetic, built in code.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::speleodb::{authed_session_at, REMOTE_ID};
use super::take_remote_success_tests::{
    dump_at_tip, output_within, project_file, routing_stub, BASE_SHA, CHILD_DEADLINE, TIP_SHA,
};
use super::AppState;
use crate::core::Command;
use crate::models::{ShotData, SpeleoLink, UndoMode};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// The environment variable that lets the child-process body run.
const CHILD_ENV: &str = "STALAGMIGHT_UNLINK_MOVE_CHILD";
/// The body's full test name, for the child's exact filter.
const CHILD_TEST: &str =
    "core::state::sdb_unlink_move_tests::unlink_move_body_in_a_process_of_its_own";

/// The section names of the project stored at `path`.
fn sections_at(path: &Path) -> Vec<String> {
    Database::open_existing(path)
        .and_then(|db| db.load_project())
        .map(|p| p.sections.iter().filter_map(|s| s.name.clone()).collect())
        .unwrap_or_default()
}

/// Whether the file at `path` carries a SpeleoDB link row.
fn linked_at(path: &Path) -> bool {
    Database::open_for_metadata(path)
        .and_then(|db| db.load_speleo_link())
        .ok()
        .flatten()
        .is_some()
}

/// Add a section named `name` through the command pipeline (an in-memory edit).
fn edit(state: &AppState, name: &str) {
    let r = state.execute(Command::CreateSection {
        name: Some(name.to_owned()),
        surveyor: None,
        date: None,
        color: "#abcdef".to_owned(),
        direction: None,
        shots: vec![ShotData {
            length: 2.0,
            ..Default::default()
        }],
    });
    assert!(r.success, "the edit {name}: {:?}", r.error);
}

/// How many release POSTs the stub has seen for the project.
fn releases(seen: &std::sync::Mutex<Vec<String>>) -> usize {
    seen.lock()
        .map(|lines| {
            lines
                .iter()
                .filter(|r| {
                    r.starts_with("POST") && r.contains(&format!("/projects/{REMOTE_ID}/release/"))
                })
                .count()
        })
        .unwrap_or_default()
}

/// The recent-projects list.
fn recents(state: &AppState) -> Vec<PathBuf> {
    state.recent_projects.lock().clone()
}

#[test]
fn unlink_moves_the_copy_out_of_the_cache_and_a_left_copy_is_kept() {
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
        "the unlink body failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the body ran (and was not filtered out): {stdout}"
    );
}

#[test]
#[ignore = "the child-process body of \
            unlink_moves_the_copy_out_of_the_cache_and_a_left_copy_is_kept: it sets the \
            process-wide app dirs, so it runs only in a process of its own"]
fn unlink_move_body_in_a_process_of_its_own() {
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
    std::fs::create_dir_all(cache.parent().expect("cache dir")).expect("create cache dir");
    let projects = dirs.path().join("Documents/stalagmight");
    std::fs::create_dir_all(&projects).expect("projects folder");

    // The managed copy at its cache path, linked at BASE, open mid edit session
    // with an in-memory edit nobody has saved.
    project_file(&cache, "Local Copy", "LocalSection");
    Database::open_existing(&cache)
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
        state.open_project(cache.clone()).success,
        "open the cache copy"
    );
    *state.speleodb.lock() = authed_session_at(&base);
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(REMOTE_ID));
    edit(&state, "LocalEdit");

    // 1. Refusals change nothing.
    let inside = state.speleodb_unlink(root.join("elsewhere.stalx"));
    let missing = state.speleodb_unlink(dirs.path().join("no-such-folder/Kept.stalx"));
    println!(
        "[FZ-SDB-5] unlink into the cache -> {:?}",
        inside.as_ref().map_err(|e| &e.message)
    );
    println!(
        "[FZ-SDB-5] unlink into a missing folder -> {:?}",
        missing.as_ref().map_err(|e| (&e.code, &e.message))
    );
    assert!(inside.is_err() && missing.is_err(), "both are refused");
    assert_eq!(state.open_project_path(), Some(cache.clone()), "still open");
    assert!(
        cache.exists() && linked_at(&cache),
        "the cache copy is untouched"
    );
    assert!(
        state.edit_enabled() && state.speleodb_lock_acquired_here(REMOTE_ID),
        "still editing under the claimed lock"
    );
    assert_eq!(releases(&seen), 0, "and the lock was not handed back");

    // 2. The move.
    let kept = projects.join("Kept.stalx");
    let moved = state
        .speleodb_unlink(kept.clone())
        .map_err(|e| e.message)
        .expect("the unlink answers Ok");
    let recent = recents(&state);
    println!(
        "[FZ-SDB-5] unlink -> {moved:?}; cache copy left: {}; copy sections {:?}, linked {}; \
         open {:?}; recents {recent:?}; releases {}; claim {}",
        cache.exists(),
        sections_at(&kept),
        linked_at(&kept),
        state.open_project_path(),
        releases(&seen),
        state.speleodb_lock_acquired_here(REMOTE_ID),
    );
    assert_eq!(
        moved.saved_path.as_deref(),
        kept.to_str(),
        "answers the copy"
    );
    assert!(!cache.exists(), "the copy left the cache");
    assert!(
        !cache.parent().is_some_and(Path::exists),
        "and its empty per-project folder went with it"
    );
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
    assert!(
        recent.contains(&kept) && !recent.contains(&cache),
        "the recent entry follows the copy"
    );
    assert_eq!(releases(&seen), 1, "the held lock was handed back");
    assert!(
        !state.speleodb_lock_acquired_here(REMOTE_ID),
        "and its claim ended"
    );

    // The re-open from SpeleoDB pulls into an empty slot; the moved copy keeps the work.
    let reopened = state.speleodb_open_project(
        REMOTE_ID.to_owned(),
        "Remote".to_owned(),
        Some(TIP_SHA.to_owned()),
        None,
    );
    println!(
        "[FZ-SDB-5] re-open -> {:?}; cache sections {:?}; copy sections {:?}",
        reopened.as_ref().map(|_| ()).map_err(|e| &e.message),
        sections_at(&cache),
        sections_at(&kept),
    );
    assert!(reopened.is_ok(), "the re-open goes ahead");
    assert_eq!(sections_at(&cache), ["ServerSection"], "the server copy");
    assert_eq!(
        sections_at(&kept),
        ["LocalSection", "LocalEdit"],
        "work kept"
    );

    // 3. A copy an older Unlink left in place: link dropped at the cache path, then
    // edited and saved there as a plain local project.
    assert!(
        state.open_project(cache.clone()).success,
        "open the server copy"
    );
    state
        .speleodb_unlink_in_place()
        .map_err(|e| e.message)
        .expect("the old in-place unlink");
    edit(&state, "AfterUnlink");
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save at the cache path: {:?}", saved.error);
    let closed = state.execute(Command::CloseProject);
    assert!(closed.success, "close");
    assert!(
        !linked_at(&cache),
        "fixture: a link-less copy at the cache path"
    );

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
        "[FZ-SDB-5-D3] re-open over the left copy -> {:?}; payload {payload:?}; cache sections {:?}",
        refused.as_ref().map(|_| ()).map_err(|e| &e.message),
        sections_at(&cache),
    );
    assert!(
        payload.as_ref().is_some_and(|p| p.unlinked
            && p.project_id == REMOTE_ID
            && Path::new(&p.local_path) == cache),
        "refused with the keep-local / take-remote prompt, flagged unlinked"
    );
    assert_eq!(
        sections_at(&cache),
        ["ServerSection", "AfterUnlink"],
        "and the left copy is untouched"
    );

    // Keep local: open it, then Unlink moves it out.
    assert!(
        state.open_project(cache.clone()).success,
        "open the left copy"
    );
    let legacy = projects.join("Legacy.stalx");
    let moved = state
        .speleodb_unlink(legacy.clone())
        .map_err(|e| e.message)
        .expect("the move-out of a left copy answers Ok");
    println!(
        "[FZ-SDB-5-D3] keep local -> {moved:?}; cache copy left: {}; copy sections {:?}",
        cache.exists(),
        sections_at(&legacy),
    );
    assert!(!cache.exists(), "the left copy is out of the cache");
    assert_eq!(sections_at(&legacy), ["ServerSection", "AfterUnlink"]);
    assert!(
        state
            .speleodb_open_project(
                REMOTE_ID.to_owned(),
                "Remote".to_owned(),
                Some(TIP_SHA.to_owned()),
                None,
            )
            .is_ok(),
        "and the re-open then goes ahead"
    );
    assert_eq!(
        sections_at(&legacy),
        ["ServerSection", "AfterUnlink"],
        "work kept"
    );
}
