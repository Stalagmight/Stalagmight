//! Cached SpeleoDB copies: owner-aware listing, removal and purge
//! (`speleodb_cached`), and opening a cached copy read-only without a session
//! (`speleodb_pull`).
//!
//! All data is synthetic: every copy is a `.stalx` built here, every server a
//! loopback listener.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::speleodb_cache::LocalEditsPolicy;
use super::speleodb_cached::SpeleoCacheOwnership;
use super::speleodb_local_state::{cache_path_in, TestCacheRoot};
use super::AppState;
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;
use crate::speleodb::{SpeleoDbAction, SpeleoReadOnlyOpen};

const MINE: &str = "11111111-1111-1111-1111-111111111111";
const THEIRS: &str = "22222222-2222-2222-2222-222222222222";
const LEGACY: &str = "33333333-3333-3333-3333-333333333333";
const ELSEWHERE: &str = "44444444-4444-4444-4444-444444444444";
const THEIRS_CLEAN: &str = "55555555-5555-5555-5555-555555555555";

const SERVER: &str = "https://sdb.example";
const OTHER_SERVER: &str = "https://other.example";
const ME: &str = "diver@example.com";

/// A cached copy of `id` under `root`, linked to `endpoint`.
fn seed(root: &Path, id: &str, endpoint: &str, dirty: bool) -> PathBuf {
    let path = cache_path_in(root, id);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("cache dir");
    let mut db = Database::open(&path).expect("open db");
    db.save_project(&Project::new(format!("Cave {id}"), None))
        .expect("save project");
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: id.to_owned(),
        project_name: format!("Remote {}", id.get(..4).unwrap_or(id)),
        base_commit_sha: Some("abc1234".to_owned()),
        has_unpushed_edits: dirty,
        last_edit_at: dirty.then(|| "2026-11-09T10:00:00Z".to_owned()),
        last_verify_at: None,
        undo_mode: None,
    }))
    .expect("save link");
    path
}

fn sign_in(state: &AppState, endpoint: &str, email: &str) {
    let mut session = state.speleodb_session_lock().expect("session lock");
    session.endpoint = Some(endpoint.to_owned());
    session.email = Some(email.to_owned());
    session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token("tok");
    session.rehydrate_attempted = true;
}

/// A signed-out state that never rehydrates a saved token from the test config.
fn signed_out() -> AppState {
    let state = AppState::new().expect("state");
    state.speleodb_session_lock().expect("session").forget();
    state
}

fn ownership(state: &AppState, root: &Path, id: &str) -> Option<SpeleoCacheOwnership> {
    state
        .speleodb_cached_projects_in(root)
        .into_iter()
        .find(|copy| copy.project_id == id)
        .map(|copy| copy.ownership)
}

/// Four copies: mine, another account's, a pre-upgrade one from this instance,
/// and a pre-upgrade one from another instance.
fn four_copies(root: &Path) {
    let state = AppState::new().expect("state");
    let mine = seed(root, MINE, SERVER, false);
    state.speleodb_record_cache_owner(&mine, SERVER, ME, "Mine");
    let theirs = seed(root, THEIRS, SERVER, false);
    state.speleodb_record_cache_owner(&theirs, SERVER, "someone@example.com", "Theirs");
    seed(root, LEGACY, SERVER, false);
    seed(root, ELSEWHERE, OTHER_SERVER, false);
}

#[test]
fn signed_out_lists_every_cached_copy() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        four_copies(temp.path());
        let state = signed_out();
        let copies = state.speleodb_cached_projects_in(temp.path());
        assert_eq!(copies.len(), 4);
        assert!(copies.iter().all(
            |copy| copy.ownership == SpeleoCacheOwnership::SignedOut && copy.ownership.listed()
        ));
    });
}

#[test]
fn signed_in_lists_only_this_accounts_copies_and_unknown_ones_from_this_instance() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        four_copies(root);
        let state = AppState::new().expect("state");
        sign_in(&state, SERVER, ME);
        assert_eq!(
            ownership(&state, root, MINE),
            Some(SpeleoCacheOwnership::Mine)
        );
        assert_eq!(
            ownership(&state, root, THEIRS),
            Some(SpeleoCacheOwnership::OtherAccount)
        );
        assert_eq!(
            ownership(&state, root, LEGACY),
            Some(SpeleoCacheOwnership::Unknown)
        );
        assert_eq!(
            ownership(&state, root, ELSEWHERE),
            Some(SpeleoCacheOwnership::OtherAccount)
        );
        let listed: Vec<String> = state
            .speleodb_cached_projects_in(root)
            .into_iter()
            .filter(|copy| copy.ownership.listed())
            .map(|copy| copy.project_id)
            .collect();
        assert_eq!(listed, vec![MINE.to_owned(), LEGACY.to_owned()]);
    });
}

#[test]
fn the_same_account_on_another_instance_is_another_account() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        four_copies(temp.path());
        let state = AppState::new().expect("state");
        sign_in(&state, OTHER_SERVER, ME);
        assert_eq!(
            ownership(&state, temp.path(), MINE),
            Some(SpeleoCacheOwnership::OtherAccount)
        );
        assert_eq!(
            ownership(&state, temp.path(), ELSEWHERE),
            Some(SpeleoCacheOwnership::Unknown)
        );
    });
}

#[test]
fn owner_matching_ignores_email_case_and_endpoint_spelling() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        four_copies(temp.path());
        let state = AppState::new().expect("state");
        sign_in(&state, "HTTPS://SDB.example:443/", "Diver@Example.COM");
        assert_eq!(
            ownership(&state, temp.path(), MINE),
            Some(SpeleoCacheOwnership::Mine)
        );
    });
}

#[test]
fn a_successful_online_open_by_another_account_takes_the_copy_over() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        four_copies(temp.path());
        let state = AppState::new().expect("state");
        state.speleodb_record_cache_owner(
            &cache_path_in(temp.path(), THEIRS),
            SERVER,
            ME,
            "Theirs",
        );
        sign_in(&state, SERVER, ME);
        assert_eq!(
            ownership(&state, temp.path(), THEIRS),
            Some(SpeleoCacheOwnership::Mine)
        );
    });
}

#[test]
fn listing_reports_un_uploaded_and_unreadable_copies() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        seed(root, MINE, SERVER, true);
        let unreadable = cache_path_in(root, THEIRS);
        std::fs::create_dir_all(unreadable.parent().expect("parent")).expect("dir");
        std::fs::write(&unreadable, b"not a database").expect("garbage");
        let state = signed_out();
        state.speleodb_record_cache_owner(&unreadable, SERVER, ME, "Named by the owner record");
        let copies = state.speleodb_cached_projects_in(root);
        let dirty = copies
            .iter()
            .find(|c| c.project_id == MINE)
            .expect("dirty row");
        assert!(dirty.has_unpushed_edits && !dirty.state_unknown);
        assert_eq!(dirty.project_name, "Remote 1111");
        let unknown = copies
            .iter()
            .find(|c| c.project_id == THEIRS)
            .expect("unknown row");
        assert!(
            unknown.state_unknown,
            "an unreadable copy is never reported clean"
        );
        assert_eq!(unknown.project_name, "Named by the owner record");
    });
}

#[test]
fn removing_a_copy_with_un_uploaded_edits_needs_explicit_consent() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let path = seed(root, MINE, SERVER, true);
        let state = signed_out();
        let refused = state.speleodb_remove_cached_copy_in(root, MINE, false);
        assert!(!refused.success);
        assert!(path.exists(), "un-uploaded work is never removed unasked");
        let removed = state.speleodb_remove_cached_copy_in(root, MINE, true);
        assert!(removed.success, "{:?}", removed.error);
        assert!(!root.join(MINE).exists(), "the whole copy directory goes");
    });
}

#[test]
fn an_unreadable_copy_is_treated_as_holding_un_uploaded_work() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let path = cache_path_in(root, MINE);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(&path, b"not a database").expect("garbage");
        let state = signed_out();
        assert!(
            !state
                .speleodb_remove_cached_copy_in(root, MINE, false)
                .success
        );
        assert!(path.exists());
    });
}

#[test]
fn removing_a_clean_copy_takes_its_owner_record_and_sidecars_with_it() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let path = seed(root, MINE, SERVER, false);
        let state = signed_out();
        state.speleodb_record_cache_owner(&path, SERVER, ME, "Mine");
        std::fs::write(path.with_file_name("project.stalx-wal"), b"").expect("stale wal");
        let removed = state.speleodb_remove_cached_copy_in(root, MINE, false);
        assert!(removed.success, "{:?}", removed.error);
        assert!(!root.join(MINE).exists());
        assert!(state.speleodb_cached_projects_in(root).is_empty());
    });
}

/// A removed copy goes to the trash under its project's name rather than as one
/// more `project.stalx`, so the user can tell which copy to restore. The name is
/// the copy's own project name, made a safe file name everywhere.
#[test]
fn a_removed_copy_is_trashed_under_its_project_name() {
    use super::speleodb_cached::{cached_copy_name, trash_file_name};

    let temp = tempfile::tempdir().expect("temp");
    let path = seed(temp.path(), MINE, SERVER, false);
    assert_eq!(cached_copy_name(&path).as_deref(), Some("Remote 1111"));
    assert_eq!(
        cached_copy_name(&path).as_deref().and_then(trash_file_name),
        Some("Remote 1111.stalx".to_owned())
    );

    for (name, expected) in [
        ("Synthetic Cave B", Some("Synthetic Cave B.stalx")),
        (
            "a/b\\c:d*e?f\"g<h>i|j\u{7}k",
            Some("a_b_c_d_e_f_g_h_i_j_k.stalx"),
        ),
        (" ..hidden cave. ", Some("hidden cave.stalx")),
        ("CON", Some("CON_.stalx")),
        ("com1", Some("com1_.stalx")),
        ("nul.cave", Some("nul.cave_.stalx")),
        ("COM10", Some("COM10.stalx")),
        ("Console", Some("Console.stalx")),
        ("", None),
        (" ... ", None),
        // Unicode format characters (Cf) are invisible, and a right-to-left
        // override would show this as "cave" + "xlsx..." reversed in a file manager.
        ("cave\u{202E}xslx.stalx", Some("cave_xslx.stalx.stalx")),
        ("\u{FEFF}Cave\u{200B}A", Some("_Cave_A.stalx")),
        ("soft\u{AD}hyphen\u{2066}", Some("soft_hyphen_.stalx")),
        ("tag\u{E0041}", Some("tag_.stalx")),
    ] {
        assert_eq!(trash_file_name(name).as_deref(), expected, "{name:?}");
    }
    let long = trash_file_name(&"\u{e9}".repeat(300)).expect("a long name is cut");
    assert!(long.len() <= 150 + ".stalx".len(), "{} bytes", long.len());
    assert!(long.ends_with(".stalx"));
}

#[test]
fn the_open_project_and_invalid_ids_are_never_removed() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let path = seed(root, MINE, SERVER, false);
        let state = signed_out();
        assert!(state.open_project(path.clone()).success, "open the copy");
        let refused = state.speleodb_remove_cached_copy_in(root, MINE, true);
        assert!(!refused.success);
        assert!(path.exists());
        let open = state
            .speleodb_cached_projects_in(root)
            .into_iter()
            .find(|copy| copy.project_id == MINE)
            .expect("row");
        assert!(open.is_open);
        assert!(
            !state
                .speleodb_remove_cached_copy_in(root, "../escape", true)
                .success
        );
    });
}

#[test]
fn purge_keeps_un_uploaded_and_open_copies_unless_told_otherwise() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        seed(root, MINE, SERVER, false);
        seed(root, THEIRS, SERVER, true);
        let open = seed(root, LEGACY, SERVER, false);
        let state = signed_out();
        assert!(state.open_project(open.clone()).success, "open one copy");

        let kept = state.speleodb_purge_cached_copies_in(root, false);
        assert_eq!(kept.removed, 1);
        assert_eq!(kept.kept_unpushed, vec!["Remote 2222".to_owned()]);
        assert_eq!(kept.kept_open, vec!["Remote 3333".to_owned()]);
        assert!(kept.failed.is_empty());
        assert!(!root.join(MINE).exists());
        assert!(cache_path_in(root, THEIRS).exists());

        let all = state.speleodb_purge_cached_copies_in(root, true);
        assert_eq!(all.removed, 1);
        assert!(all.kept_unpushed.is_empty());
        assert_eq!(all.kept_open, vec!["Remote 3333".to_owned()]);
        assert!(!root.join(THEIRS).exists());
        assert!(open.exists(), "the open project is never purged");
    });
}

/// The pre-PR finding: signed in, the purge reached other accounts' copies, their
/// un-uploaded work included, though the home screen hides them and the purge
/// confirmation never named them. It now reaches exactly what the home screen
/// lists; signed out, everything.
#[test]
fn signed_in_purge_reaches_only_the_copies_the_home_screen_lists() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = loopback(|_| held_by("someone@example.com"));
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let setup = AppState::new().expect("state");
        let mine = seed(root, MINE, &url, true);
        setup.speleodb_record_cache_owner(&mine, &url, ME, "Mine");
        let theirs = seed(root, THEIRS, &url, true);
        setup.speleodb_record_cache_owner(&theirs, &url, "someone@example.com", "Theirs");
        let theirs_clean = seed(root, THEIRS_CLEAN, &url, false);
        setup.speleodb_record_cache_owner(&theirs_clean, &url, "someone@example.com", "Theirs");
        seed(root, LEGACY, &url, false);
        let elsewhere = seed(root, ELSEWHERE, OTHER_SERVER, true);

        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        let report = state.speleodb_purge_cached_copies_in(root, true);
        assert_eq!(report.removed, 2, "{report:?}");
        assert!(report.kept_unpushed.is_empty() && report.kept_open.is_empty());
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(!root.join(MINE).exists() && !root.join(LEGACY).exists());
        assert!(
            theirs.exists(),
            "another account's un-uploaded work survives"
        );
        assert!(theirs_clean.exists(), "another account's clean copy too");
        assert!(elsewhere.exists(), "another instance's copy too");
        assert!(
            seen.lock()
                .expect("log")
                .iter()
                .all(|line| line.contains(MINE) || line.contains(LEGACY)),
            "nothing is asked about the other account's projects"
        );

        let all = signed_out().speleodb_purge_cached_copies_in(root, true);
        assert_eq!(all.removed, 3, "signed out, every copy: {all:?}");
        assert!(!theirs.exists() && !theirs_clean.exists() && !elsewhere.exists());
    });
}

// --- opening a cached copy without a session ---------------------------------

#[test]
fn signed_out_opens_the_cached_copy_read_only_without_any_network_call() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        // The copy names an unreachable endpoint: any call would fail loudly.
        let path = seed(temp.path(), MINE, "http://127.0.0.1:1", true);
        let state = signed_out();
        let opened = state
            .speleodb_pull_project_to(
                &path,
                MINE.to_owned(),
                "Remote".to_owned(),
                None,
                LocalEditsPolicy::Protect,
            )
            .expect("read-only open");
        assert_eq!(opened.read_only, Some(SpeleoReadOnlyOpen::SignedOut));
        assert_eq!(
            opened.has_unpushed_edits,
            Some(true),
            "un-uploaded work is kept"
        );
        assert_eq!(opened.local_path, path.to_string_lossy());
        assert_eq!(opened.offline, None);
    });
}

#[test]
fn signed_out_without_a_cached_copy_still_asks_to_sign_in() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let state = signed_out();
        let err = state
            .speleodb_pull_project_to(
                &cache_path_in(temp.path(), MINE),
                MINE.to_owned(),
                "Remote".to_owned(),
                None,
                LocalEditsPolicy::Protect,
            )
            .expect_err("nothing to open");
        assert_eq!(err.action, SpeleoDbAction::ReLogin);
        // A copy of a DIFFERENT project at that path is no fallback either.
        let foreign = seed(temp.path(), THEIRS, SERVER, false);
        assert!(state
            .speleodb_pull_project_to(
                &foreign,
                MINE.to_owned(),
                "Remote".to_owned(),
                None,
                LocalEditsPolicy::Protect,
            )
            .is_err());
    });
}

#[test]
fn signed_out_discard_never_falls_back_to_the_copy() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let path = seed(temp.path(), MINE, SERVER, true);
        let state = signed_out();
        assert!(state
            .speleodb_pull_project_to(
                &path,
                MINE.to_owned(),
                "Remote".to_owned(),
                None,
                LocalEditsPolicy::Discard,
            )
            .is_err());
    });
}

/// A loopback SpeleoDB answering every request with `reply(request_line)`, and
/// recording the request lines.
fn loopback(reply: fn(&str) -> (u16, String)) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(chunk.get(..n).unwrap_or_default()),
                }
            }
            let line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let (status, body) = reply(&line);
            log.lock().expect("log").push(line);
            let response = format!(
                "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn denied_detail() -> String {
    r#"{"detail":"denied"}"#.to_owned()
}

#[test]
fn an_expired_token_opens_the_cached_copy_read_only_and_signs_out() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = loopback(|_| (401, denied_detail()));
        let temp = tempfile::tempdir().expect("temp");
        let path = seed(temp.path(), MINE, &url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        let opened = state
            .speleodb_pull_project_to(
                &path,
                MINE.to_owned(),
                "Remote".to_owned(),
                None,
                LocalEditsPolicy::Protect,
            )
            .expect("read-only open");
        assert_eq!(opened.read_only, Some(SpeleoReadOnlyOpen::SessionExpired));
        assert!(
            state.speleodb_active_token().is_err(),
            "the rejected token is dropped"
        );
        assert_eq!(
            seen.lock().expect("log").len(),
            1,
            "nothing is asked after the rejection"
        );
    });
}

#[test]
fn a_dead_token_reported_as_403_is_an_expired_session_too() {
    crate::test_helpers::with_isolated_config(|| {
        // The DRF downgrade: a dead token gets 403 everywhere, whoami included.
        let (url, _) = loopback(|_| (403, denied_detail()));
        let temp = tempfile::tempdir().expect("temp");
        let path = seed(temp.path(), MINE, &url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        let opened = state
            .speleodb_pull_project_to(
                &path,
                MINE.to_owned(),
                "Remote".to_owned(),
                None,
                LocalEditsPolicy::Protect,
            )
            .expect("read-only open");
        assert_eq!(opened.read_only, Some(SpeleoReadOnlyOpen::SessionExpired));
    });
}

#[test]
fn a_server_denial_with_a_valid_session_never_falls_back_to_the_copy() {
    crate::test_helpers::with_isolated_config(|| {
        // whoami succeeds, so the 403 is a real "this account may not have it".
        let (url, _) = loopback(|line| {
            if line.contains("/api/v2/user/") {
                (
                    200,
                    super::tests_support::sdk_fixture(r#"{"email":"diver@example.com"}"#),
                )
            } else {
                (403, denied_detail())
            }
        });
        let temp = tempfile::tempdir().expect("temp");
        let path = seed(temp.path(), MINE, &url, true);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        let refused = state.speleodb_pull_project_to(
            &path,
            MINE.to_owned(),
            "Remote".to_owned(),
            None,
            LocalEditsPolicy::Protect,
        );
        assert!(
            refused.is_err(),
            "403 with a live session keeps the no-fallback rule"
        );
        assert!(state.speleodb_active_token().is_ok(), "the session is kept");
    });
}

#[test]
fn a_missing_project_never_falls_back_to_the_copy() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, _) = loopback(|_| (404, denied_detail()));
        let temp = tempfile::tempdir().expect("temp");
        let path = seed(temp.path(), MINE, &url, false);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        assert!(state
            .speleodb_pull_project_to(
                &path,
                MINE.to_owned(),
                "Remote".to_owned(),
                None,
                LocalEditsPolicy::Protect,
            )
            .is_err());
        assert!(state.speleodb_active_token().is_ok());
    });
}

// --- the fast-forward open only opens this account's own work -----------------

/// A loopback SpeleoDB whose history tip is `abc1234` (the [`seed`] base) and
/// whose download is a valid synthetic project, recording the request lines.
fn history_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let native = {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("server.stalx");
        let mut db = Database::open(&path).expect("server db");
        db.save_project(&Project::new("Server".to_owned(), None))
            .expect("save project");
        drop(db);
        std::fs::read(&path).expect("server bytes")
    };
    let history = super::tests_support::sdk_fixture(r#"{"commits":[{"id":"abc1234"}]}"#);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(chunk.get(..n).unwrap_or_default()),
                }
            }
            let line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let (kind, body) = if line.contains("/download/") {
                ("application/octet-stream", native.clone())
            } else {
                ("application/json", history.clone().into_bytes())
            };
            log.lock().expect("log").push(line);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
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

fn owner_bytes(copy: &Path) -> Option<Vec<u8>> {
    std::fs::read(copy.with_file_name("owner.json")).ok()
}

fn downloads(seen: &Arc<Mutex<Vec<String>>>) -> usize {
    seen.lock()
        .expect("log")
        .iter()
        .filter(|line| line.contains("/download/"))
        .count()
}

/// A refused open, reduced to what the frontend routes on: the action, and whose
/// work the un-uploaded-local prompt names (`None` = the signed-in account's own).
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    action: SpeleoDbAction,
    other_owner: Option<String>,
}

/// Open the cached copy of [`MINE`] at `copy` online, protecting local edits.
fn open_online(
    state: &AppState,
    copy: &Path,
) -> Result<crate::speleodb::SpeleoOpenResult, Refusal> {
    state
        .speleodb_pull_project_to(
            copy,
            MINE.to_owned(),
            "Remote".to_owned(),
            None,
            LocalEditsPolicy::Protect,
        )
        .map_err(|e| Refusal {
            action: e.action,
            other_owner: e.unpushed_local.and_then(|local| local.other_owner),
        })
}

#[test]
fn another_accounts_un_uploaded_work_on_the_tip_takes_the_prompt_and_keeps_its_owner() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = history_server();
        let temp = tempfile::tempdir().expect("temp");
        // Base == tip, un-uploaded edits, recorded as another account's.
        let copy = seed(temp.path(), MINE, &url, true);
        let state = AppState::new().expect("state");
        state.speleodb_record_cache_owner(&copy, &url, "someone@example.com", "Theirs");
        let before = owner_bytes(&copy).expect("owner record");
        sign_in(&state, &url, ME);

        let refused = open_online(&state, &copy).expect_err("no fast-forward");
        assert_eq!(
            refused,
            Refusal {
                action: SpeleoDbAction::UnpushedLocalEdits,
                other_owner: Some("someone@example.com".to_owned()),
            },
            "the un-uploaded-local prompt, naming whose work it is"
        );
        assert_eq!(downloads(&seen), 1, "it went the guarded-download way");
        assert_eq!(owner_bytes(&copy), Some(before), "owner.json unchanged");
        let link = Database::open_for_metadata(&copy)
            .expect("open")
            .load_speleo_link()
            .expect("read")
            .expect("linked");
        assert!(
            link.has_unpushed_edits,
            "the other account's work is intact"
        );
        assert_eq!(
            ownership(&state, temp.path(), MINE),
            Some(SpeleoCacheOwnership::OtherAccount)
        );
    });
}

#[test]
fn an_owner_less_copy_with_un_uploaded_work_takes_the_prompt_and_stays_unclaimed() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, _seen) = history_server();
        let temp = tempfile::tempdir().expect("temp");
        let copy = seed(temp.path(), MINE, &url, true);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        let refused = open_online(&state, &copy).expect_err("no fast-forward");
        assert_eq!(
            refused,
            Refusal {
                action: SpeleoDbAction::UnpushedLocalEdits,
                other_owner: None,
            },
            "nothing names another account: the prompt as before"
        );
        assert_eq!(owner_bytes(&copy), None, "not silently claimed");
    });
}

#[test]
fn my_own_un_uploaded_work_on_the_tip_still_fast_forwards() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = history_server();
        let temp = tempfile::tempdir().expect("temp");
        let copy = seed(temp.path(), MINE, &url, true);
        let state = AppState::new().expect("state");
        state.speleodb_record_cache_owner(&copy, &url, ME, "Mine");
        sign_in(&state, &url, ME);
        let opened = open_online(&state, &copy).expect("fast-forward open");
        assert_eq!(opened.has_unpushed_edits, Some(true));
        assert_eq!(opened.local_path, copy.to_string_lossy());
        assert_eq!(downloads(&seen), 0, "nothing downloaded");
        assert_eq!(
            ownership(&state, temp.path(), MINE),
            Some(SpeleoCacheOwnership::Mine)
        );
    });
}

#[test]
fn offline_another_accounts_un_uploaded_work_opens_read_only_and_names_them() {
    crate::test_helpers::with_isolated_config(|| {
        // Never resolves (RFC 6761): a DNS failure, so the open falls back to the
        // cached copy (a refused connection is not offline-eligible today). A
        // hostname is outside the automatic HTTP exceptions, so it reaches DNS only
        // through the development-build insecure-HTTP override (tests are debug
        // builds); without it the policy refuses it, which is not offline.
        let url = "http://speleodb-offline.invalid:8000";
        let temp = tempfile::tempdir().expect("temp");
        let copy = seed(temp.path(), MINE, url, true);
        let state = AppState::new().expect("state");
        state.config.lock().expect("config").speleodb.insecure_http = true;
        state.speleodb_record_cache_owner(&copy, url, "someone@example.com", "Theirs");
        sign_in(&state, url, ME);
        let opened = open_online(&state, &copy).expect("offline open");
        assert_eq!(opened.offline, Some(true));
        assert_eq!(opened.read_only, Some(SpeleoReadOnlyOpen::OtherAccount));
        assert_eq!(opened.other_owner.as_deref(), Some("someone@example.com"));

        // This account's own work opens offline as before.
        state.speleodb_record_cache_owner(&copy, url, ME, "Mine");
        let mine = open_online(&state, &copy).expect("offline open");
        assert_eq!((mine.read_only, mine.other_owner), (None, None));
    });
}

// --- Start Editing never takes over another account's work --------------------

/// The cached copy of [`MINE`] on `url`, open, recorded as `owner`'s, with this
/// state signed in as [`ME`].
fn open_copy_owned_by(root: &Path, url: &str, owner: &str, dirty: bool) -> (PathBuf, AppState) {
    let copy = seed(root, MINE, url, dirty);
    let state = AppState::new().expect("state");
    state.speleodb_record_cache_owner(&copy, url, owner, "Remote 1111");
    assert!(state.open_project(copy.clone()).success, "open the copy");
    sign_in(&state, url, ME);
    (copy, state)
}

#[test]
fn start_editing_refuses_another_accounts_un_uploaded_work_before_any_request() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = super::tests_support::lock_server(MINE, None);
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (copy, state) = open_copy_owned_by(temp.path(), &url, "someone@example.com", true);
        let before = owner_bytes(&copy);
        let refused = state
            .speleodb_start_editing()
            .expect_err("never adopt another account's work");
        assert_eq!(refused.action, SpeleoDbAction::Fatal);
        assert!(
            refused.message.contains("someone@example.com"),
            "{}",
            refused.message
        );
        assert!(
            seen.lock().expect("log").is_empty(),
            "no lock was asked for"
        );
        assert!(!state.edit_enabled(), "still read-only");
        assert_eq!(owner_bytes(&copy), before, "owner.json unchanged");
    });
}

#[test]
fn start_editing_a_clean_copy_recorded_for_another_account_records_this_one() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = super::tests_support::lock_server(MINE, None);
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (_copy, state) = open_copy_owned_by(temp.path(), &url, "someone@example.com", false);
        // Whatever the stub's answers lead to, the acquire is attempted: a clean
        // copy holds only the server's content, nobody's work.
        let _ = state.speleodb_start_editing();
        assert!(
            !seen.lock().expect("log").is_empty(),
            "the lock was asked for"
        );
        assert_eq!(
            ownership(&state, temp.path(), MINE),
            Some(SpeleoCacheOwnership::Mine)
        );
    });
}

#[test]
fn start_editing_my_own_or_an_owner_less_copy_is_not_refused() {
    crate::test_helpers::with_isolated_config(|| {
        for owner in [Some(ME), None] {
            let (url, seen) = super::tests_support::lock_server(MINE, None);
            let temp = tempfile::tempdir().expect("temp");
            let _cache = TestCacheRoot::set(temp.path());
            let copy = seed(temp.path(), MINE, &url, true);
            let state = AppState::new().expect("state");
            if let Some(owner) = owner {
                state.speleodb_record_cache_owner(&copy, &url, owner, "Remote 1111");
            }
            assert!(state.open_project(copy).success, "open the copy");
            sign_in(&state, &url, ME);
            if let Err(e) = state.speleodb_start_editing() {
                assert!(
                    !e.message.contains("can edit and upload"),
                    "{owner:?}: {}",
                    e.message
                );
            }
            assert!(
                !seen.lock().expect("log").is_empty(),
                "{owner:?}: the lock was asked for"
            );
        }
    });
}

/// Home's single "Remove local copy" follows the purge's rule: signed in, never
/// another account's (or another instance's) copy; signed out, any copy.
#[test]
fn signed_in_removal_never_reaches_another_accounts_copy() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let theirs = seed(root, THEIRS, SERVER, true);
        let setup = AppState::new().expect("state");
        setup.speleodb_record_cache_owner(&theirs, SERVER, "someone@example.com", "Theirs");
        let elsewhere = seed(root, ELSEWHERE, OTHER_SERVER, false);

        let state = AppState::new().expect("state");
        sign_in(&state, SERVER, ME);
        for (id, path) in [(THEIRS, &theirs), (ELSEWHERE, &elsewhere)] {
            let refused = state.speleodb_remove_cached_copy_in(root, id, true);
            assert!(!refused.success, "{id}");
            assert!(refused
                .error
                .as_deref()
                .is_some_and(|e| e.contains("another SpeleoDB account")));
            assert!(path.exists(), "{id} is kept");
        }

        let out = signed_out();
        assert!(
            out.speleodb_remove_cached_copy_in(root, THEIRS, true)
                .success
        );
        assert!(
            out.speleodb_remove_cached_copy_in(root, ELSEWHERE, false)
                .success
        );
        assert!(!theirs.exists() && !elsewhere.exists());
    });
}

// --- removing a copy hands back this account's lock ---------------------------

/// A project body whose lock `holder` holds, for every request (probe and release).
fn held_by(holder: &str) -> (u16, String) {
    (
        200,
        super::tests_support::sdk_fixture(&format!(
            r#"{{"id":"{MINE}","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{{"user":"{holder}"}}}}"#
        )),
    )
}

/// [`held_by`] `ME` for the holder probe, and for the release itself the project
/// with no holder - what SpeleoDB answers once it has let go. (A release reply that
/// still names this account released nothing, and is reported as a failure.)
fn held_by_me_until_released(line: &str) -> (u16, String) {
    if line.contains("/release") {
        (
            200,
            super::tests_support::sdk_fixture(&format!(
                r#"{{"id":"{MINE}","name":"Remote","permission":"READ_AND_WRITE"}}"#
            )),
        )
    } else {
        held_by(ME)
    }
}

fn releases(seen: &Arc<Mutex<Vec<String>>>) -> usize {
    seen.lock()
        .expect("log")
        .iter()
        .filter(|line| line.contains("/release"))
        .count()
}

/// A clean cached copy of [`MINE`] on `url`, recorded as `owner`'s.
fn owned_copy(root: &Path, url: &str, owner: &str) -> PathBuf {
    let path = seed(root, MINE, url, false);
    AppState::new()
        .expect("state")
        .speleodb_record_cache_owner(&path, url, owner, "Remote 1111");
    path
}

#[test]
fn removing_my_copy_hands_back_the_lock_the_server_says_i_hold() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = loopback(held_by_me_until_released);
        let temp = tempfile::tempdir().expect("temp");
        owned_copy(temp.path(), &url, ME);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        let removed = state.speleodb_remove_cached_copy_in(temp.path(), MINE, false);
        assert!(removed.success, "{:?}", removed.error);
        assert!(removed.warnings.is_empty(), "{:?}", removed.warnings);
        assert_eq!(releases(&seen), 1, "the lock is handed back");
        assert!(!temp.path().join(MINE).exists());
    });
}

#[test]
fn removing_my_copy_never_releases_someone_elses_lock() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = loopback(|_| held_by("someone@example.com"));
        let temp = tempfile::tempdir().expect("temp");
        owned_copy(temp.path(), &url, ME);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        assert!(
            state
                .speleodb_remove_cached_copy_in(temp.path(), MINE, false)
                .success
        );
        assert_eq!(seen.lock().expect("log").len(), 1, "only the holder probe");
        assert_eq!(releases(&seen), 0);
    });
}

#[test]
fn another_accounts_copy_or_no_session_asks_the_server_nothing() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = loopback(held_by_me_until_released);
        let temp = tempfile::tempdir().expect("temp");
        owned_copy(temp.path(), &url, "someone@example.com");
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        // Signed in, the purge does not reach another account's copy at all.
        let report = state.speleodb_purge_cached_copies_in(temp.path(), false);
        assert_eq!(report, super::speleodb_cached::SpeleoPurgeReport::default());
        assert!(cache_path_in(temp.path(), MINE).exists());

        owned_copy(temp.path(), &url, ME);
        let signed_out = signed_out();
        assert!(
            signed_out
                .speleodb_remove_cached_copy_in(temp.path(), MINE, false)
                .success
        );
        assert!(seen.lock().expect("log").is_empty(), "no request at all");
    });
}

#[test]
fn an_unreachable_server_removes_the_copy_and_says_the_lock_may_stay() {
    crate::test_helpers::with_isolated_config(|| {
        let url = "http://127.0.0.1:1";
        let temp = tempfile::tempdir().expect("temp");
        owned_copy(temp.path(), url, ME);
        let state = AppState::new().expect("state");
        sign_in(&state, url, ME);
        let removed = state.speleodb_remove_cached_copy_in(temp.path(), MINE, false);
        assert!(removed.success, "{:?}", removed.error);
        assert!(!temp.path().join(MINE).exists(), "the copy is gone");
        assert!(removed
            .warnings
            .iter()
            .any(|w| w.code == "speleodb_lock_not_handed_back"));
        assert!(state.speleodb_active_token().is_ok(), "the session is kept");
    });
}

/// A purge with the server down probes once, not once per copy: each probe would
/// wait out its own timeout (5 s), so a cache of many copies held the purge for
/// minutes. Every copy is still removed, and every one of this account's locks is
/// reported as not checked.
#[test]
fn a_purge_with_the_server_down_probes_once_and_reports_every_lock_unchecked() {
    crate::test_helpers::with_isolated_config(|| {
        // Accepts each connection and drops it at once: a transport failure the
        // probe sees immediately, and a count of how many probes were made.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let probes = Arc::new(Mutex::new(0_usize));
        let counted = Arc::clone(&probes);
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                *counted.lock().expect("count") += 1;
                drop(stream);
            }
        });
        let temp = tempfile::tempdir().expect("temp");
        let recorder = AppState::new().expect("state");
        for id in [MINE, LEGACY, ELSEWHERE] {
            let path = seed(temp.path(), id, &url, false);
            recorder.speleodb_record_cache_owner(&path, &url, ME, "Remote");
        }
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);

        let report = state.speleodb_purge_cached_copies_in(temp.path(), false);

        assert_eq!(report.removed, 3, "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(state.speleodb_cached_projects_in(temp.path()).is_empty());
        let mut unchecked = report.locks_unchecked;
        unchecked.sort();
        assert_eq!(
            unchecked,
            vec![
                "Remote 1111".to_owned(),
                "Remote 3333".to_owned(),
                "Remote 4444".to_owned()
            ]
        );
        assert_eq!(*probes.lock().expect("count"), 1, "one probe for the run");
        assert!(state.speleodb_active_token().is_ok(), "the session is kept");
    });
}

#[test]
fn purge_reports_the_locks_it_handed_back() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = loopback(held_by_me_until_released);
        let temp = tempfile::tempdir().expect("temp");
        owned_copy(temp.path(), &url, ME);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        let report = state.speleodb_purge_cached_copies_in(temp.path(), false);
        assert_eq!(report.removed, 1);
        assert_eq!(report.locks_released, vec!["Remote 1111".to_owned()]);
        assert_eq!(releases(&seen), 1);
    });
}

#[test]
fn a_copy_that_could_not_be_removed_keeps_its_lock() {
    crate::test_helpers::with_isolated_config(|| {
        let (url, seen) = loopback(held_by_me_until_released);
        let temp = tempfile::tempdir().expect("temp");
        let path = owned_copy(temp.path(), &url, ME);
        let state = AppState::new().expect("state");
        sign_in(&state, &url, ME);
        assert!(state.open_project(path).success, "open the copy");
        assert!(
            !state
                .speleodb_remove_cached_copy_in(temp.path(), MINE, true)
                .success
        );
        assert!(seen.lock().expect("log").is_empty());
    });
}

// --- what a removal never reaches ---------------------------------------------

/// What `speleodb_unlink` leaves in the cache: the `.stalx`, with its link row
/// gone. From then on it is a plain project, which the user may have edited and
/// saved since.
fn unlink(path: &Path) {
    let db = Database::open(path).expect("open db");
    db.save_speleo_link(None).expect("unlink");
}

/// The pre-PR finding: the listing drops an unlinked `.stalx`, but the purge
/// walked every cache directory, read the unlinked file as "known clean" and
/// trashed it (and removed its folder), even with `discard_unpushed = false`. It
/// now reaches exactly the copies the listing shows; Home's single removal and
/// sign-out's Discard refuse such a file too, whatever the consent.
#[test]
fn an_unlinked_copy_is_never_purged_or_removed() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let path = seed(root, MINE, SERVER, false);
        unlink(&path);
        let signed_in = AppState::new().expect("state");
        sign_in(&signed_in, SERVER, ME);

        for state in [signed_out(), signed_in] {
            assert!(
                state.speleodb_cached_projects_in(root).is_empty(),
                "the listing never shows it"
            );
            for discard_unpushed in [false, true] {
                let report = state.speleodb_purge_cached_copies_in(root, discard_unpushed);
                assert_eq!(report.removed, 0, "{report:?}");
                assert_eq!(
                    report,
                    super::speleodb_cached::SpeleoPurgeReport::default(),
                    "nothing the confirmation never named is reported either"
                );
                assert!(path.exists(), "the purge leaves it");

                let refused = state.speleodb_remove_cached_copy_in(root, MINE, discard_unpushed);
                assert!(!refused.success);
                assert!(
                    refused
                        .error
                        .as_deref()
                        .is_some_and(|e| e.contains("no longer linked")),
                    "{:?}",
                    refused.error
                );
                assert!(path.exists(), "Home's removal leaves it");
            }
        }

        let state = AppState::new().expect("state");
        sign_in(&state, SERVER, ME);
        let refused = state.speleodb_discard_cached_copy_in(root, MINE, true);
        assert!(!refused.success, "sign-out's Discard leaves it");
        assert!(path.exists() && root.join(MINE).is_dir());
    });
}

/// The removal checks "is it open?" before it deletes, without the project lock,
/// and the delete used to tear down whatever was open at that path by then: a
/// copy opened in between was closed and trashed. The delete now asks again
/// under the project lock, and refuses instead. (The check has passed: this
/// drives the step after it, with the copy open by then.)
#[test]
fn a_copy_opened_after_the_check_is_refused_not_closed_and_trashed() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let path = seed(root, MINE, SERVER, false);
        let state = signed_out();
        assert!(state.open_project(path.clone()).success, "open the copy");
        let still_open = || {
            state
                .open_project_path()
                .is_some_and(|open| super::project_io::same_file(&open, &path))
        };
        let _ = super::recent_projects::take_trashed_for_tests();

        assert!(
            state.remove_cached_copy_files(root, MINE).is_none(),
            "Home's removal and the purge refuse"
        );
        assert!(path.exists() && still_open(), "nothing was done");

        let refused = state.discard_cached_copy_files(root, MINE);
        assert!(!refused.success, "sign-out's Discard refuses");
        assert_eq!(refused.session_closed, None, "the session was not closed");
        assert!(path.exists() && still_open());
        assert!(
            super::recent_projects::take_trashed_for_tests().is_empty(),
            "nothing reached the trash"
        );
    });
}

/// A project directory in the cache that is a symlink to one of the user's own
/// folders made a removal trash the user's `project.stalx` through it. Every
/// removal now refuses one; the purge's walk never followed links.
#[cfg(unix)]
#[test]
fn a_symlinked_project_directory_is_never_followed() {
    crate::test_helpers::with_isolated_config(|| {
        let cache = tempfile::tempdir().expect("cache");
        let user = tempfile::tempdir().expect("user folder");
        // The user's own file, shaped like a clean managed copy so nothing else
        // would stop the removal.
        let users_file = seed(user.path(), MINE, SERVER, false);
        std::os::unix::fs::symlink(user.path().join(MINE), cache.path().join(MINE))
            .expect("symlink");
        let _ = super::recent_projects::take_trashed_for_tests();

        let out = signed_out();
        let refused = out.speleodb_remove_cached_copy_in(cache.path(), MINE, true);
        assert!(!refused.success);
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|e| e.contains("links to another place")),
            "{:?}",
            refused.error
        );
        assert_eq!(
            out.speleodb_purge_cached_copies_in(cache.path(), true)
                .removed,
            0
        );

        let state = AppState::new().expect("state");
        sign_in(&state, SERVER, ME);
        assert!(
            !state
                .speleodb_discard_cached_copy_in(cache.path(), MINE, true)
                .success
        );
        assert!(users_file.exists(), "the user's file is untouched");
        assert!(
            std::fs::symlink_metadata(cache.path().join(MINE))
                .is_ok_and(|meta| meta.file_type().is_symlink()),
            "the link itself is left too"
        );
        assert!(super::recent_projects::take_trashed_for_tests().is_empty());
    });
}

/// The owner record went through `owner.json.tmp` with a plain write, which
/// follows a symlink planted under that name and writes the record over its
/// target. The temporary file is now one the write creates itself.
#[cfg(unix)]
#[test]
fn the_owner_record_never_writes_through_a_planted_link() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        let copy = seed(temp.path(), MINE, SERVER, false);
        let victim = elsewhere.path().join("victim.txt");
        std::fs::write(&victim, b"keep me").expect("victim");
        let tmp = copy.with_file_name("owner.json.tmp");
        std::os::unix::fs::symlink(&victim, &tmp).expect("planted link");

        let state = AppState::new().expect("state");
        state.speleodb_record_cache_owner(&copy, SERVER, ME, "Mine");

        assert_eq!(
            std::fs::read(&victim).expect("victim"),
            b"keep me",
            "the link's target is untouched"
        );
        let record = copy.with_file_name("owner.json");
        assert!(
            std::fs::symlink_metadata(&record).is_ok_and(|meta| meta.file_type().is_file()),
            "owner.json is a plain file"
        );
        assert!(std::fs::symlink_metadata(&tmp).is_err(), "no temp left");
        sign_in(&state, SERVER, ME);
        assert_eq!(
            ownership(&state, temp.path(), MINE),
            Some(SpeleoCacheOwnership::Mine)
        );
    });
}

/// Start Editing's owner guard spoke for any open file laid out like a cache copy
/// (`<anything>/<project_id>/project.stalx`) and wrote `owner.json` beside it.
/// Only the copy in the app's own cache has (or gets) an owner record now.
#[test]
fn start_editing_writes_no_owner_record_outside_the_cache() {
    crate::test_helpers::with_isolated_config(|| {
        for dirty in [false, true] {
            let (url, seen) = super::tests_support::lock_server(MINE, None);
            let elsewhere = tempfile::tempdir().expect("elsewhere");
            // A linked file outside the cache, opened while no cache root was
            // known (the foreign-copy guard refuses it otherwise), with an owner
            // record beside it naming another account.
            let (copy, state) =
                open_copy_owned_by(elsewhere.path(), &url, "someone@example.com", dirty);
            let before = owner_bytes(&copy);
            let cache = tempfile::tempdir().expect("cache");
            let _cache = TestCacheRoot::set(cache.path());

            if let Err(e) = state.speleodb_start_editing() {
                assert!(
                    !e.message.contains("someone@example.com"),
                    "dirty={dirty}: the record outside the cache speaks for nothing: {}",
                    e.message
                );
            }
            assert_eq!(
                owner_bytes(&copy),
                before,
                "dirty={dirty}: no owner record is written outside the cache"
            );
            assert!(
                !seen.lock().expect("log").is_empty(),
                "dirty={dirty}: not refused before the lock was asked for"
            );
        }
    });
}
