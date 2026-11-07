//! [#576 RVF-c-C1] The IPCs that write the OPEN project without answering a
//! `MutationOutcome` name the project they were made for, like the survey edits
//! and the save (`switch_identity_tests`): the dive-plan save and delete (its
//! `.plnx`), the SpeleoDB undo-mode setting (its link row), the web-rendering flag
//! (its server side, through the link), Unlink (a Save As of it, then a close and
//! the removal of its cache copy) and Publish (whose later checks must still name
//! the project once it has moved into the cache). Each runs here inside `for_project` made for
//! CaveA while CaveB is open - the order a switch leaves behind - and is refused
//! with `PROJECT_CHANGED`, nothing written and nothing sent; made for CaveB, it
//! goes through.
//!
//! Unlink resolves the SpeleoDB cache through the process-wide app dirs, a
//! `OnceLock` other tests in this binary rely on staying unset, so its body runs in
//! a process of its own (as `sdb_unlink_move_tests`). The network is a loopback
//! stub. All data is synthetic, built in code.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::speleodb::{authed_session_at, REMOTE_ID};
use super::switch_identity_tests::{identity_of, saved};
use super::take_remote_success_tests::{
    output_within, project_file, routing_stub, BASE_SHA, CHILD_DEADLINE,
};
use super::{for_project, AppState, ProjectIdentity};
use crate::core::CommandErrorCode;
use crate::models::{DivePlanSnapshot, SpeleoLink, UndoMode};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// The environment variable that lets the Unlink child-process body run.
const CHILD_ENV: &str = "STALAGMIGHT_UNLINK_IDENTITY_CHILD";
/// The body's full test name, for the child's exact filter.
const CHILD_TEST: &str =
    "core::state::open_project_writer_identity_tests::unlink_identity_body_in_a_process_of_its_own";

/// A plan with nothing in it: what the dive-plan verbs store does not matter here.
fn empty_plan() -> DivePlanSnapshot {
    serde_json::from_str("{}").expect("an empty snapshot")
}

/// `path`'s SpeleoDB link row as it is on disk (`None` too when there is no file).
fn link_on_disk(path: &Path) -> Option<SpeleoLink> {
    Database::open_for_metadata(path)
        .and_then(|db| db.load_speleo_link())
        .ok()
        .flatten()
}

/// Link the project at `path` to the SpeleoDB project `REMOTE_ID`, undo mode Full.
fn link(path: &Path, endpoint: &str) {
    Database::open_existing(path)
        .expect("reopen")
        .save_speleo_link(Some(&SpeleoLink {
            endpoint: endpoint.to_owned(),
            project_id: REMOTE_ID.to_owned(),
            project_name: "CaveB".to_owned(),
            base_commit_sha: Some(BASE_SHA.to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: Some(UndoMode::Full),
        }))
        .expect("link");
}

/// CaveA then CaveB loaded, in a temp dir: the identity made for CaveA (captured
/// while it was open) and the one for CaveB, which is open now. `linked` links
/// CaveB to SpeleoDB (at `endpoint`) before it is opened.
fn a_then_b(
    temp: &TempDir,
    linked: Option<&str>,
) -> (
    AppState,
    PathBuf,
    Option<ProjectIdentity>,
    Option<ProjectIdentity>,
) {
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let b = saved(temp.path(), "CaveB.stalx", "CaveB");
    if let Some(endpoint) = linked {
        link(&b, endpoint);
    }
    let state = state_with_null_sink();
    state.do_load_project_full(a).expect("open CaveA");
    let made_for_a = identity_of(&state);
    state.do_load_project_full(b.clone()).expect("open CaveB");
    let made_for_b = identity_of(&state);
    assert_ne!(made_for_a, made_for_b, "fixture: two projects");
    (state, b, made_for_a, made_for_b)
}

/// A dive plan saved, updated or deleted for CaveA after a switch to CaveB is
/// refused, and CaveB's `.plnx` keeps its bytes; CaveA gets no `.plnx` either.
#[test]
fn a_dive_plan_made_for_the_project_a_switch_replaced_is_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp");
    let (state, _b, made_for_a, made_for_b) = a_then_b(&temp, None);
    // CaveB's own plan, saved outside any scope (unchecked, as a core caller).
    let kept = state
        .save_dive_plan(None, "CaveB plan".to_owned(), empty_plan())
        .expect("CaveB's plan");
    let plnx_b = temp.path().join("CaveB.plnx");
    let before = std::fs::read(&plnx_b).expect("CaveB's .plnx");

    let insert = for_project(made_for_a.clone(), || {
        state.save_dive_plan_for(None, "Made for CaveA".to_owned(), empty_plan())
    });
    let update = for_project(made_for_a.clone(), || {
        state.save_dive_plan_for(Some(kept.id), "Made for CaveA".to_owned(), empty_plan())
    });
    let delete = for_project(made_for_a, || state.delete_dive_plan_for(kept.id));
    // Read before anything else opens the store (an open alone may touch the file).
    let unchanged = std::fs::read(&plnx_b).expect("CaveB's .plnx") == before;
    let codes = [
        ("insert", insert.as_ref().err().map(|e| e.code)),
        ("update", update.as_ref().err().map(|e| e.code)),
        ("delete", delete.as_ref().err().map(|e| e.code)),
    ];
    let plans: Vec<String> = state
        .list_dive_plans()
        .expect("list")
        .into_iter()
        .map(|p| p.name)
        .collect();
    println!(
        "[RVF-c-C1] dive plan made for CaveA with CaveB open -> {codes:?}; CaveB's plans \
         {plans:?}; CaveB.plnx unchanged: {unchanged}; CaveA.plnx exists: {}",
        temp.path().join("CaveA.plnx").exists()
    );
    for (what, code) in codes {
        assert_eq!(code, Some(CommandErrorCode::ProjectChanged), "{what}");
    }
    assert!(unchanged, "CaveB's .plnx is byte for byte as it was");
    assert_eq!(
        plans,
        ["CaveB plan"],
        "CaveB's .plnx gained or lost nothing"
    );
    assert!(!temp.path().join("CaveA.plnx").exists());

    // Made for CaveB, the open project: both go through.
    let saved_b = for_project(made_for_b.clone(), || {
        state.save_dive_plan_for(None, "Second".to_owned(), empty_plan())
    });
    let deleted_b = for_project(made_for_b, || state.delete_dive_plan_for(kept.id));
    println!("[RVF-c-C1] made for CaveB -> save {saved_b:?}, delete {deleted_b:?}");
    assert!(saved_b.is_ok() && deleted_b.is_ok());
    let names: Vec<String> = state
        .list_dive_plans()
        .expect("list")
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Second"]);
}

/// An undo-history mode set for CaveA after a switch to the linked CaveB is
/// refused, and CaveB's link row is as it was.
#[test]
fn an_undo_mode_made_for_the_project_a_switch_replaced_is_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp");
    let (state, b, made_for_a, made_for_b) = a_then_b(&temp, Some("http://placeholder.invalid/"));
    state.set_edit_enabled(true);
    let before = link_on_disk(&b);

    let refused = for_project(made_for_a, || {
        state.speleodb_set_undo_mode(UndoMode::Disabled)
    });
    let after = link_on_disk(&b);
    println!(
        "[RVF-c-C1] undo mode made for CaveA with CaveB open -> {:?}; link row unchanged: {}",
        refused.as_ref().err().map(|e| (&e.code, &e.message)),
        after == before
    );
    assert_eq!(
        refused.err().map(|e| e.code),
        Some(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(after, before, "CaveB's link row is untouched");

    let set = for_project(made_for_b, || {
        state.speleodb_set_undo_mode(UndoMode::Disabled)
    });
    assert!(set.is_ok(), "made for CaveB: {:?}", set.err());
    assert_eq!(
        link_on_disk(&b).and_then(|l| l.undo_mode),
        Some(UndoMode::Disabled)
    );
}

/// How many PATCHes of the project the stub has seen.
fn patches(seen: &std::sync::Mutex<Vec<String>>) -> usize {
    seen.lock().map_or(0, |lines| {
        lines
            .iter()
            .filter(|r| r.starts_with("PATCH") && r.contains(&format!("/projects/{REMOTE_ID}/")))
            .count()
    })
}

/// A network-only verb (the web-rendering flag) made for CaveA after a switch to
/// the linked CaveB is refused before it reads the link, so nothing is sent to
/// CaveB's server project.
#[test]
fn a_server_change_made_for_the_project_a_switch_replaced_sends_nothing() {
    let _cfg = crate::test_helpers::isolated_config();
    let (base, _listener, seen) = routing_stub(Vec::new());
    let temp = TempDir::new().expect("temp");
    let (state, _b, made_for_a, made_for_b) = a_then_b(&temp, Some(&base));
    *state.speleodb.lock() = authed_session_at(&base);

    let refused = for_project(made_for_a, || state.speleodb_set_exclude_geojson(false));
    let sent = patches(&seen);
    println!(
        "[RVF-c-C1] web rendering made for CaveA with CaveB open -> {:?}; PATCHes sent {sent}",
        refused.as_ref().err().map(|e| (&e.code, &e.message))
    );
    assert_eq!(
        refused.err().map(|e| e.code),
        Some(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(sent, 0, "no request went to CaveB's server project");

    let set = for_project(made_for_b, || state.speleodb_set_exclude_geojson(false));
    assert!(set.is_ok(), "made for CaveB: {:?}", set.err());
    assert_eq!(patches(&seen), 1, "made for CaveB, the PATCH goes out");
}

#[test]
fn an_unlink_made_for_the_project_a_switch_replaced_writes_nothing() {
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

/// An Unlink made for CaveA after a switch to CaveB (the cache copy of a SpeleoDB
/// project) is refused: no file at the destination, CaveB still open from its cache
/// copy, still linked. Made for CaveB it moves CaveB out. Then Publish, which needs
/// the cache too: refused for another project before any request, and made for the
/// open one it lands although its project moved into the cache on the way.
#[test]
#[ignore = "the child-process body of \
            an_unlink_made_for_the_project_a_switch_replaced_writes_nothing: it sets the \
            process-wide app dirs, so it runs only in a process of its own"]
fn unlink_identity_body_in_a_process_of_its_own() {
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
    let projects = dirs.path().join("Documents");
    std::fs::create_dir_all(&projects).expect("projects folder");

    // CaveA, a local project, then CaveB: the managed copy at its cache path.
    let a = saved(&projects, "CaveA.stalx", "CaveA");
    project_file(&cache, "CaveB", "CacheSection");
    link(&cache, "http://placeholder.invalid/");
    let state = state_with_null_sink();
    state.do_load_project_full(a).expect("open CaveA");
    let made_for_a = identity_of(&state);
    assert!(state.open_project(cache.clone()).success, "open CaveB");
    let made_for_b = identity_of(&state);

    let dest = projects.join("Chosen for CaveA.stalx");
    let refused = for_project(made_for_a, || state.speleodb_unlink(dest.clone()));
    println!(
        "[RVF-c-C1] unlink made for CaveA with CaveB open -> {:?}; destination exists: {}; \
         open {:?}; cache copy linked: {}",
        refused.as_ref().err().map(|e| (&e.code, &e.message)),
        dest.exists(),
        state.open_project_path(),
        link_on_disk(&cache).is_some()
    );
    assert_eq!(
        refused.err().map(|e| e.code),
        Some(CommandErrorCode::ProjectChanged)
    );
    assert!(!dest.exists(), "nothing written at the destination");
    assert_eq!(state.open_project_path(), Some(cache.clone()), "still open");
    assert!(
        link_on_disk(&cache).is_some(),
        "the cache copy is still there, linked"
    );

    let kept = projects.join("Kept.stalx");
    let moved = for_project(made_for_b.clone(), || state.speleodb_unlink(kept.clone()));
    println!(
        "[RVF-c-C1] unlink made for CaveB -> {:?}",
        moved.as_ref().map_err(|e| &e.message)
    );
    assert!(moved.is_ok(), "made for CaveB the move goes ahead");
    assert!(kept.exists() && !cache.exists(), "moved out of the cache");

    // Publish: made for another project it sends nothing (the create POST cannot be
    // taken back); made for the open one, the checks after its relocation into the
    // cache (the upload's local half, the link writes) still name it, so it lands.
    let local = projects.join("Local.stalx");
    project_file(&local, "Local", "SectionA");
    assert!(state.open_project(local).success, "open the local project");
    let made_for_local = identity_of(&state);
    let (base, _listener, seen) = super::speleodb::routing_stub(publish_route);
    *state.speleodb.lock() = authed_session_at(&base);
    let refused = for_project(made_for_b, || state.speleodb_publish(publish_params()));
    let sent = seen.lock().map_or(0, |lines| lines.len());
    println!(
        "[RVF-c-C1] publish made for CaveB with Local open -> {:?}; requests sent {sent}",
        refused.as_ref().err().map(|e| (&e.code, &e.message))
    );
    assert_eq!(
        refused.err().map(|e| e.code),
        Some(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(sent, 0, "no create POST went out");

    let published = for_project(made_for_local, || state.speleodb_publish(publish_params()));
    let link = state
        .speleodb_open_link()
        .ok()
        .map(|l| (l.project_id, l.base_commit_sha));
    println!(
        "[RVF-c-C1] publish made for Local -> {:?}; open link {link:?}",
        published
            .as_ref()
            .map(|r| &r.result.local_path)
            .map_err(|e| &e.message)
    );
    assert!(published.is_ok(), "made for the open project it publishes");
    assert_eq!(
        link,
        Some((PUBLISHED_ID.to_owned(), Some(PUBLISHED_SHA.to_owned()))),
        "the upload landed and its tip was recorded in the relocated project's link"
    );
}

/// The project the publish stub creates, and the commit its upload lands.
const PUBLISHED_ID: &str = "55555555-5555-5555-5555-555555555555";
const PUBLISHED_SHA: &str = "0123450000000000000000000000000000000000";

/// A SpeleoDB stub where a publish goes through: the create answers 201, the
/// acquire and release succeed, the upload lands [`PUBLISHED_SHA`].
fn publish_route(line: &str) -> (u16, &'static str) {
    const PROJECT: &str = r#"{"id":"55555555-5555-5555-5555-555555555555","name":"Local"}"#;
    let path = line.split_whitespace().nth(1).unwrap_or_default();
    if line.starts_with("PUT ") && path.ends_with("/upload/auto/") {
        (
            200,
            r#"{"hexsha":"0123450000000000000000000000000000000000",
                "project":{"id":"55555555-5555-5555-5555-555555555555","name":"Local"}}"#,
        )
    } else if path.ends_with("/revisions/") {
        (
            200,
            r#"{"commits":[{"id":"0123450000000000000000000000000000000000",
                "parent_ids":[],"author_email":"u@x"}]}"#,
        )
    } else if line.starts_with("POST /api/v2/projects/ ") {
        (201, PROJECT)
    } else {
        (200, PROJECT)
    }
}

fn publish_params() -> crate::speleodb::SpeleoPublishParams {
    crate::speleodb::SpeleoPublishParams {
        name: "Local".to_owned(),
        description: "a synthetic survey".to_owned(),
        country: "US".to_owned(),
        visibility: crate::speleodb::Visibility::Private,
        latitude: None,
        longitude: None,
    }
}
