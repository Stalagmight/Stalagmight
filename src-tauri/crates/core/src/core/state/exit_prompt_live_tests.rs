//! LIVE check of the SpeleoDB exit flows against a PRIVATE test server.
//!
//! `#[ignore]` AND env-gated: it never runs in CI or in a plain `cargo test`. It
//! drives the same backend verbs the exit prompt and the sign-out guard call -
//! open (pull), Start Editing, a real edit, save, the three closes (keep /
//! discard / release), Finish editing (upload + release), the explicit release
//! and logout - against throwaway projects it creates and deletes itself.
//!
//! Run (on the private test host only):
//!
//! ```text
//! STALAGMIGHT_EXIT_LIVE=1 SDB_URL=http://127.0.0.1:19000 \
//!   SDB_EMAIL=... SDB_PASSWORD=... \
//!   cargo test -p stalagmight-core --lib exit_prompt_live -- --ignored --nocapture
//! ```
//!
//! What it proves, each as its own step:
//! 1. KEEP: close keeping the lock -> the lock is still ours on the server ->
//!    reopen opens the local copy (no refusal, no download) -> Start Editing
//!    resumes with no divergence -> upload advances BASE and releases the lock.
//! 2. DISCARD: close discarding -> the local copy is gone and the lock released ->
//!    reopen downloads the server copy with no refusal; the discarded edit is gone.
//! 3. STRANDED (what earlier builds left behind: un-uploaded edits, lock already
//!    released, base == tip): reopen opens locally, Start Editing re-acquires, and
//!    the upload succeeds.
//! 4. SIGN-OUT: two locks held (one kept across a close, one on the open project)
//!    are both handed back explicitly before logout, as the sign-out guard does.
//! 5. SOMEONE ELSE'S LOCK (with `SDB_EMAIL2` / `SDB_PASSWORD2`): our un-uploaded
//!    work on the server tip still opens locally with no prompt, read-only behind
//!    the other account's lock, with the work kept.

use std::path::{Path, PathBuf};

use super::AppState;
use crate::core::Command;
use crate::speleodb::{client, sync, DoneResult, SpeleoOpenResult, StartEditingResult};

#[derive(Clone)]
struct Live {
    url: String,
    email: String,
    password: String,
    /// A second account for step 5 (optional): `SDB_EMAIL2` / `SDB_PASSWORD2`.
    other: Option<(String, String)>,
}

fn live() -> Option<Live> {
    if std::env::var("STALAGMIGHT_EXIT_LIVE").ok().as_deref() != Some("1") {
        return None;
    }
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    Some(Live {
        url: var("SDB_URL")?,
        email: var("SDB_EMAIL")?,
        password: var("SDB_PASSWORD")?,
        other: var("SDB_EMAIL2").zip(var("SDB_PASSWORD2")),
    })
}

/// `curl` with the bearer token on STDIN (`-H @-`), so the credential never
/// appears in an argument list or in the output.
fn curl(method: &str, url: &str, token: &str, body: Option<&str>) -> bool {
    use std::io::Write as _;
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "-s",
        "-o",
        "/dev/null",
        "-w",
        "%{http_code}",
        "-X",
        method,
        "-H",
        "@-",
    ]);
    if let Some(body) = body {
        cmd.args(["-H", "Content-Type: application/json", "--data", body]);
    }
    cmd.arg(url)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "Authorization: Token {token}");
    }
    child
        .wait_with_output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim().starts_with('2'))
}

/// A throwaway project with one native revision, created as the test account.
fn seed_project(live: &Live, token: &str, label: &str) -> String {
    let created = client::create_project_with(
        &live.url,
        token,
        false,
        &serde_json::json!({
            "name": format!("Stalagmight exit-prompt live {label} {}", uuid::Uuid::new_v4()),
            "description": "Synthetic exit-prompt live check; deleted after the run",
            "country": "US", "type": "STALAGMIGHT", "visibility": "PRIVATE",
        })
        .to_string(),
    )
    .expect("create a throwaway project");
    let id = created.id;
    eprintln!("LIVE: created throwaway project {label} = {id}");
    client::acquire_lock(&live.url, token, false, &id).expect("seed lock");
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("project.stalx");
    {
        let mut db = crate::persistence::Database::open(&path).expect("seed db");
        db.save_project(&crate::models::Project::new(
            format!("Synthetic {label}"),
            None,
        ))
        .expect("seed project");
    }
    client::upload_stalx(
        &live.url,
        token,
        false,
        &id,
        &path,
        "First synthetic revision",
    )
    .expect("seed revision");
    client::release_lock(&live.url, token, false, &id).expect("seed release");
    id
}

fn tip(live: &Live, token: &str, id: &str) -> String {
    let history = client::get_revisions(&live.url, token, false, id).expect("revisions");
    sync::tip_of(&history.commits).expect("a tip").to_owned()
}

fn holder(live: &Live, token: &str, id: &str) -> Option<String> {
    client::get_project(&live.url, token, false, id)
        .expect("project")
        .active_mutex
        .map(|m| m.user)
}

fn open_remote(state: &AppState, id: &str) -> SpeleoOpenResult {
    state
        .speleodb_open_project(id.to_owned(), "Synthetic".to_owned(), None)
        .unwrap_or_else(|e| {
            std::panic::panic_any(format!(
                "open {id} was refused: {} ({:?})",
                e.message, e.action
            ))
        })
}

fn load(state: &AppState, path: &str) {
    let result = state.open_project(PathBuf::from(path));
    assert!(result.success, "load {path}: {:?}", result.error);
}

fn start_editing(state: &AppState) -> StartEditingResult {
    state
        .speleodb_start_editing()
        .unwrap_or_else(|e| std::panic::panic_any(format!("start editing: {}", e.message)))
}

fn edit_and_save(state: &AppState, elevation: f64) {
    let edited = state.set_surface_elevation(Some(elevation));
    assert!(edited.success, "edit: {:?}", edited.error);
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save: {:?}", saved.error);
    assert!(state.speleodb_open_link().expect("link").has_unpushed_edits);
}

fn elevation(state: &AppState) -> Option<f64> {
    state.get_project_state().and_then(|p| p.surface_elevation)
}

/// The three closes, exactly as `close_project_impl` performs them.
fn close_keep(state: &AppState) {
    state.speleodb_keep_open_lock_deliberately();
    assert!(state.execute(Command::CloseProject).success);
}
fn close_release(state: &AppState) {
    assert!(state.speleodb_close_releasing_lock().success);
}

fn done(state: &AppState, message: &str) {
    match state.speleodb_done_acknowledged(Some(message.to_owned()), false) {
        Ok(DoneResult::Released { .. }) => {}
        other => std::panic::panic_any(format!("upload did not release: {other:?}")),
    }
}

fn run_steps(live: &Live, state: &AppState, token: &str, p1: &str, p2: &str) {
    // 1. KEEP the lock, reopen, resume, upload.
    let opened = open_remote(state, p1);
    load(state, &opened.local_path);
    assert!(matches!(start_editing(state), StartEditingResult::Editing));
    let base = state.speleodb_open_link().expect("link").base_commit_sha;
    assert_eq!(base.as_deref(), Some(tip(live, token, p1).as_str()));
    edit_and_save(state, 12.5);
    close_keep(state);
    let me = Some(live.email.to_ascii_lowercase());
    assert_eq!(
        holder(live, token, p1).map(|h| h.to_ascii_lowercase()),
        me,
        "the kept lock is still ours"
    );
    let cache = PathBuf::from(&opened.local_path);
    let before = std::fs::metadata(&cache).expect("cache").modified().ok();
    let reopened = open_remote(state, p1);
    assert_eq!(
        reopened.has_unpushed_edits,
        Some(true),
        "reopened the LOCAL copy"
    );
    assert_eq!(reopened.offline, None);
    assert_eq!(
        std::fs::metadata(&cache).expect("cache").modified().ok(),
        before,
        "nothing was downloaded over it"
    );
    load(state, &reopened.local_path);
    assert!(
        matches!(start_editing(state), StartEditingResult::Editing),
        "resumed editing with no divergence / conflict"
    );
    assert_eq!(elevation(state), Some(12.5), "the kept edit is there");
    assert_eq!(
        state.speleodb_open_link().expect("link").base_commit_sha,
        base
    );
    done(
        state,
        "Exit-prompt live check: kept the lock, then uploaded",
    );
    let new_tip = tip(live, token, p1);
    assert_ne!(
        Some(new_tip.as_str()),
        base.as_deref(),
        "the upload advanced the server"
    );
    let link = state.speleodb_open_link().expect("link");
    assert_eq!(
        link.base_commit_sha.as_deref(),
        Some(new_tip.as_str()),
        "BASE advanced to the new tip"
    );
    assert!(!link.has_unpushed_edits);
    assert_eq!(
        holder(live, token, p1),
        None,
        "the upload released the lock"
    );
    close_release(state);
    eprintln!("LIVE 1 KEEP: kept lock held by us after close; reopen opened locally (no download, no refusal); \
        start editing -> Editing; upload advanced BASE {base:?} -> {new_tip}; lock released");

    // 2. DISCARD, reopen gets the server copy.
    let opened = open_remote(state, p2);
    load(state, &opened.local_path);
    assert!(matches!(start_editing(state), StartEditingResult::Editing));
    let server_tip = tip(live, token, p2);
    edit_and_save(state, 33.0);
    let discarded = state.speleodb_close_discarding_local();
    assert!(discarded.success, "discard: {:?}", discarded.error);
    assert!(discarded.warnings.is_empty(), "{:?}", discarded.warnings);
    assert!(
        !Path::new(&opened.local_path).exists(),
        "the local copy is gone"
    );
    assert_eq!(
        holder(live, token, p2),
        None,
        "the discard released the lock"
    );
    let reopened = open_remote(state, p2);
    assert_ne!(
        reopened.has_unpushed_edits,
        Some(true),
        "a fresh server copy"
    );
    load(state, &reopened.local_path);
    assert_eq!(elevation(state), None, "the discarded edit is gone");
    assert!(matches!(start_editing(state), StartEditingResult::Editing));
    assert_eq!(
        state
            .speleodb_open_link()
            .expect("link")
            .base_commit_sha
            .as_deref(),
        Some(server_tip.as_str())
    );
    close_release(state);
    assert_eq!(holder(live, token, p2), None);
    eprintln!("LIVE 2 DISCARD: copy removed + lock released; reopen downloaded the server copy at {server_tip} with no refusal");

    // 3. STRANDED by an earlier build: un-uploaded edits, lock released, base == tip.
    let opened = open_remote(state, p1);
    load(state, &opened.local_path);
    assert!(matches!(start_editing(state), StartEditingResult::Editing));
    edit_and_save(state, 7.0);
    state
        .speleodb_release_lock(p1.to_owned())
        .expect("explicit release, as the old exit did");
    assert!(state.execute(Command::CloseProject).success);
    assert_eq!(holder(live, token, p1), None, "stranded: lock released");
    let reopened = open_remote(state, p1);
    assert_eq!(
        reopened.has_unpushed_edits,
        Some(true),
        "opened locally, no prompt"
    );
    load(state, &reopened.local_path);
    assert!(
        matches!(start_editing(state), StartEditingResult::Editing),
        "re-acquired the free lock"
    );
    assert_eq!(elevation(state), Some(7.0));
    let before = tip(live, token, p1);
    done(state, "Exit-prompt live check: stranded copy uploaded");
    assert_ne!(tip(live, token, p1), before);
    assert_eq!(holder(live, token, p1), None);
    close_release(state);
    eprintln!("LIVE 3 STRANDED: base == tip opened locally with no prompt, re-acquired, uploaded, released");

    // 4. SIGN-OUT hands back every lock: one kept across a close, one open.
    let opened = open_remote(state, p1);
    load(state, &opened.local_path);
    assert!(matches!(start_editing(state), StartEditingResult::Editing));
    close_keep(state);
    let opened = open_remote(state, p2);
    load(state, &opened.local_path);
    assert!(matches!(start_editing(state), StartEditingResult::Editing));
    let listed = state.speleodb_list_projects().expect("list");
    let mine: Vec<String> = listed
        .iter()
        .filter(|p| {
            p.lock_holder
                .as_deref()
                .is_some_and(|h| h.eq_ignore_ascii_case(&live.email))
        })
        .map(|p| p.id.clone())
        .collect();
    assert!(
        mine.contains(&p1.to_owned()) && mine.contains(&p2.to_owned()),
        "both locks are ours: {mine:?}"
    );
    for id in [p1, p2] {
        state
            .speleodb_release_lock(id.to_owned())
            .expect("the sign-out releases each lock explicitly");
    }
    state.speleodb_logout().expect("logout");
    assert_eq!(holder(live, token, p1), None);
    assert_eq!(holder(live, token, p2), None);
    eprintln!("LIVE 4 SIGN-OUT: both locks ({p1}, {p2}) released before logout");
}

/// Step 5: the other account holds the lock over our fast-forward copy.
fn run_someone_elses_lock(live: &Live, state: &AppState, token: &str, p1: &str) {
    let Some((email2, password2)) = live.other.clone() else {
        eprintln!("LIVE 5 skipped: no second account configured");
        return;
    };
    state
        .speleodb_login(
            Some(live.url.clone()),
            None,
            Some(live.email.clone()),
            Some(live.password.clone()),
            None,
        )
        .expect("sign back in");
    let granted = curl(
        "POST",
        &format!("{}/api/v2/projects/{p1}/permission/user/detail/", live.url),
        token,
        Some(&serde_json::json!({ "user": email2, "level": "READ_AND_WRITE" }).to_string()),
    );
    assert!(
        granted,
        "share the throwaway project with the second account"
    );
    let opened = open_remote(state, p1);
    load(state, &opened.local_path);
    assert!(matches!(start_editing(state), StartEditingResult::Editing));
    edit_and_save(state, 9.0);
    state
        .speleodb_release_lock(p1.to_owned())
        .expect("stranded, as an old build left it");
    assert!(state.execute(Command::CloseProject).success);
    let token2 = client::login(&live.url, false, &email2, &password2).expect("second account");
    client::acquire_lock(&live.url, &token2, false, p1).expect("the other account takes the lock");
    let reopened = open_remote(state, p1);
    assert_eq!(
        reopened.has_unpushed_edits,
        Some(true),
        "opened locally, no prompt"
    );
    load(state, &reopened.local_path);
    match start_editing(state) {
        StartEditingResult::LockedBy { email } => {
            assert!(
                email
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(&email2)),
                "{email:?}"
            );
        }
        other => std::panic::panic_any(format!(
            "expected read-only behind the other lock, got {other:?}"
        )),
    }
    assert!(!state.edit_enabled(), "read-only");
    assert!(
        state.speleodb_open_link().expect("link").has_unpushed_edits,
        "our work is kept"
    );
    assert_eq!(elevation(state), Some(9.0));
    client::release_lock(&live.url, &token2, false, p1).expect("the other account lets go");
    assert!(state.execute(Command::CloseProject).success);
    eprintln!("LIVE 5 OTHER LOCK: base == tip opened locally, read-only behind {email2}'s lock, local work kept");
}

#[test]
#[ignore = "live: needs a private test SpeleoDB (STALAGMIGHT_EXIT_LIVE=1, SDB_URL, SDB_EMAIL, SDB_PASSWORD)"]
fn exit_prompt_live_keep_discard_stranded_and_sign_out() {
    let Some(live) = live() else {
        eprintln!("exit-prompt live check skipped: STALAGMIGHT_EXIT_LIVE/SDB_* not set");
        return;
    };
    crate::test_helpers::with_isolated_config(|| {
        crate::app_dirs::init_for_test();
        let state = AppState::new().expect("state");
        state
            .speleodb_login(
                Some(live.url.clone()),
                None,
                Some(live.email.clone()),
                Some(live.password.clone()),
                None,
            )
            .expect("sign in");
        let token = client::login(&live.url, false, &live.email, &live.password).expect("token");
        let p1 = seed_project(&live, &token, "P1");
        let p2 = seed_project(&live, &token, "P2");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_steps(&live, &state, &token, &p1, &p2);
            run_someone_elses_lock(&live, &state, &token, &p1);
        }));
        // Clean up whatever happened: hand back any lock, delete both projects.
        for id in [&p1, &p2] {
            let _ = client::release_lock(&live.url, &token, false, id);
            let deleted = curl(
                "DELETE",
                &format!("{}/api/v2/projects/{id}/", live.url),
                &token,
                None,
            );
            eprintln!("LIVE cleanup: deleted {id}: {deleted}");
        }
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    });
}
