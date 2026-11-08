//! [#576 FZ-SDB-6-D1] A lock verb whose reply was lost re-reads the holder at once.
//!
//! The server can apply a release or an acquire and lose only the answer: the
//! connection drops after the request went out, the body arrives unreadable, or a
//! gateway answers 5xx in its place. The app used to take the failed reply at its
//! word - a landed release kept the edit gate open over a lock that was gone, and a
//! landed acquire skipped the publish's initial upload while the new project stayed
//! locked by us. Now each lock verb re-reads who holds the lock when the reply may
//! have been lost, and follows what the server did:
//!
//! - the Admin tab's release (`speleodb_release_current_lock`): free, or another
//!   account's while we were editing under our lock - released (gate shut, Ok);
//!   anything else - the original error, gate kept;
//! - Done's release: not ours any more - Released (gate shut, claim gone); still
//!   ours or unreadable - the error, gate and claim kept;
//! - publish's acquire: ours - the claim is recorded and the initial upload and
//!   release go ahead; anything else - the failed-acquire path.
//! - Start Editing's acquire: ours - editing starts and the probe's verdict decides
//!   the claim, as on an answered acquire; anything else - the failed acquire.
//!
//! A failure the server ANSWERED (a 4xx) is not re-read: no second request.
//!
//! Driven against the loopback routing stub: a "lost" reply is a 200 with a
//! truncated JSON body (a parse failure) or a 504, a failed re-read is a 503. Done's
//! pre-release holder check must still see our lock, so its stubs flip the project
//! read once the release went out (a static per test, since the stub routes by a
//! plain `fn`). Publish resolves the cache path through the process-wide app dirs,
//! so - as in `sdb_publish_gate_tests` - its bodies run in a process of their own.
//!
//! All data is synthetic, built in code.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use super::speleodb::{
    authed_session_at, editing_fixture, link_open_copy_to, remote_readonly_fixture, routing_stub,
    FREE_MUTEX_AT_BASE_BODY, HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::take_remote_success_tests::{output_within, project_file, CHILD_DEADLINE};
use super::AppState;
use crate::speleodb::{SpeleoPublishParams, Visibility};
use crate::test_fixtures::state_with_null_sink;

// --- the server's answers ---------------------------------------------------------

/// A release that landed, answered with a body cut off mid-way: a parse failure.
const TRUNCATED_PROJECT: &str = r#"{"id":"11111111-1111-1111-1111-111111111111","na"#;
/// A gateway answering in the server's place after the request went out.
const GATEWAY_TIMEOUT: (u16, &str) = (504, r#"{"detail":"gateway timeout"}"#);
/// The holder re-read failing: the server is down.
const UNAVAILABLE: (u16, &str) = (503, r#"{"detail":"service unavailable"}"#);
/// A release the server answered: it did not happen.
const REFUSED: (u16, &str) = (400, r#"{"detail":"you do not hold this project's lock"}"#);

/// The request line's path.
fn path_of(line: &str) -> &str {
    line.split_whitespace().nth(1).unwrap_or_default()
}

/// Whether `line` is a release POST.
fn is_release(line: &str) -> bool {
    line.starts_with("POST ") && path_of(line).ends_with("/release/")
}

/// Whether `line` is the acquire POST.
fn is_acquire(line: &str) -> bool {
    line.starts_with("POST ") && path_of(line).ends_with("/acquire/")
}

/// Whether `line` reads project `id` - the holder re-read.
fn is_project_read(line: &str, id: &str) -> bool {
    line.starts_with("GET ") && path_of(line) == format!("/api/v2/projects/{id}/")
}

/// How many reads of project `id` came after the first request matching `verb`.
fn reads_after(requests: &[String], verb: fn(&str) -> bool, id: &str) -> usize {
    requests
        .iter()
        .skip_while(|r| !verb(r))
        .skip(1)
        .filter(|r| is_project_read(r, id))
        .count()
}

/// What one lock verb left behind.
#[derive(Debug)]
struct Observed {
    answer: String,
    gate: bool,
    force_edit: bool,
    claim_kept: bool,
    reads_after_release: usize,
    requests: Vec<String>,
}

fn observe(state: &AppState, answer: String, seen: &Arc<Mutex<Vec<String>>>) -> Observed {
    let requests = seen.lock().expect("request log").clone();
    Observed {
        answer,
        gate: state.edit_enabled(),
        force_edit: state.force_editing(),
        claim_kept: state.speleodb_lock_acquired_here(REMOTE_ID),
        reads_after_release: reads_after(&requests, is_release, REMOTE_ID),
        requests,
    }
}

// --- the Admin tab's release ------------------------------------------------------

/// The release landed and its reply was lost; the lock is free now.
fn admin_freed(line: &str) -> (u16, &'static str) {
    if is_release(line) {
        GATEWAY_TIMEOUT
    } else {
        (200, FREE_MUTEX_AT_BASE_BODY)
    }
}

/// The release landed, its reply was lost, and a colleague took the lock at once.
fn admin_taken_by_a_colleague(line: &str) -> (u16, &'static str) {
    if is_release(line) {
        (200, TRUNCATED_PROJECT)
    } else {
        (200, HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY)
    }
}

/// The reply was lost and the release did NOT land: the lock is still ours.
fn admin_still_ours(line: &str) -> (u16, &'static str) {
    if is_release(line) {
        GATEWAY_TIMEOUT
    } else {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    }
}

/// The reply was lost and the holder cannot be re-read.
fn admin_unreadable(line: &str) -> (u16, &'static str) {
    if is_release(line) {
        GATEWAY_TIMEOUT
    } else {
        UNAVAILABLE
    }
}

/// The server answered the release with a refusal.
fn refused(line: &str) -> (u16, &'static str) {
    if is_release(line) {
        REFUSED
    } else {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    }
}

/// The Admin tab's release, mid edit session, against a stub answering `route`.
fn admin_release_against(route: fn(&str) -> (u16, &'static str)) -> Observed {
    let (base, _listener, seen) = routing_stub(route);
    let (_temp, state) = editing_fixture(&base);
    let answer = format!(
        "{:?}",
        state.speleodb_release_current_lock().map_err(|e| e.message)
    );
    observe(&state, answer, &seen)
}

#[test]
fn release_lost_reply_over_a_freed_lock_shuts_the_gate_and_answers_ok() {
    let seen = admin_release_against(admin_freed);
    println!("[FZ-SDB-6] admin release, 504 then the lock is free -> {seen:?}");
    assert_eq!(
        seen.answer, "Ok(())",
        "the release landed, so it answers Ok"
    );
    assert!(
        !seen.gate && !seen.force_edit,
        "and the gate is shut as on a confirmed release"
    );
    assert_eq!(seen.reads_after_release, 1, "the holder is re-read at once");
}

#[test]
fn release_lost_reply_over_a_colleagues_lock_while_editing_is_a_landed_release() {
    let seen = admin_release_against(admin_taken_by_a_colleague);
    println!("[FZ-SDB-6] admin release, unreadable body then a colleague holds it -> {seen:?}");
    assert_eq!(
        seen.answer, "Ok(())",
        "we were editing under OUR lock and someone else holds it now: ours was released"
    );
    assert!(!seen.gate, "and the gate is shut");
    assert_eq!(seen.reads_after_release, 1, "the holder is re-read at once");
}

#[test]
fn release_lost_reply_still_ours_keeps_the_gate_and_fails() {
    let seen = admin_release_against(admin_still_ours);
    println!("[FZ-SDB-6] admin release, 504 then the lock is still ours -> {seen:?}");
    assert_eq!(seen.reads_after_release, 1, "the holder is re-read at once");
    assert!(
        seen.answer.starts_with("Err("),
        "the release did not land: {}",
        seen.answer
    );
    assert!(
        seen.gate && seen.claim_kept,
        "the gate and the claim stay for a retry"
    );
}

#[test]
fn release_lost_reply_unreadable_keeps_the_gate_and_fails() {
    let seen = admin_release_against(admin_unreadable);
    println!("[FZ-SDB-6] admin release, 504 then the re-read fails -> {seen:?}");
    assert!(
        seen.reads_after_release >= 1,
        "the holder re-read was attempted"
    );
    assert!(
        seen.answer.starts_with("Err("),
        "with nothing known the release is reported failed: {}",
        seen.answer
    );
    assert!(
        seen.gate && seen.claim_kept,
        "the gate and the claim stay for the next poll"
    );
}

#[test]
fn release_lost_reply_over_a_lock_we_were_not_editing_under_fails() {
    // An ADMIN force-releasing somebody else's lock, not editing: a lost reply and
    // the same holder still on the mutex is a release that did not land.
    let (base, _listener, seen) = routing_stub(admin_taken_by_a_colleague);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);
    // [#576 W89] The copy lives on the server signed in to: Release lock refuses a
    // copy of another server before sending anything.
    link_open_copy_to(&state, &base);
    let answer = format!(
        "{:?}",
        state.speleodb_release_current_lock().map_err(|e| e.message)
    );
    let seen = observe(&state, answer, &seen);
    println!("[FZ-SDB-6] admin release of a colleague's lock, not editing -> {seen:?}");
    assert_eq!(seen.reads_after_release, 1, "the holder is re-read at once");
    assert!(
        seen.answer.starts_with("Err("),
        "the colleague still holds it: the release did not land: {}",
        seen.answer
    );
    assert!(!seen.gate, "and a gate that was shut stays shut");
}

#[test]
fn release_refused_with_a_4xx_reads_nothing_back() {
    let seen = admin_release_against(refused);
    println!("[FZ-SDB-6] admin release refused with a 400 -> {seen:?}");
    assert_eq!(
        seen.reads_after_release, 0,
        "an answered failure is not re-read: {:?}",
        seen.requests
    );
    assert!(
        seen.answer.starts_with("Err("),
        "the refusal is reported: {}",
        seen.answer
    );
    assert!(seen.gate && seen.claim_kept, "and nothing changes locally");
}

// --- Done's release ---------------------------------------------------------------

static DONE_FREED_RELEASED: AtomicBool = AtomicBool::new(false);

/// Done's holder check sees our lock; the release lands with an unreadable reply,
/// and from then on the lock is free.
fn done_freed(line: &str) -> (u16, &'static str) {
    if is_release(line) {
        DONE_FREED_RELEASED.store(true, Ordering::SeqCst);
        (200, TRUNCATED_PROJECT)
    } else if DONE_FREED_RELEASED.load(Ordering::SeqCst) {
        (200, FREE_MUTEX_AT_BASE_BODY)
    } else {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    }
}

static DONE_UNREADABLE_RELEASED: AtomicBool = AtomicBool::new(false);

/// Done's holder check sees our lock; the release answers 504, and from then on
/// the project cannot be read.
fn done_unreadable(line: &str) -> (u16, &'static str) {
    if is_release(line) {
        DONE_UNREADABLE_RELEASED.store(true, Ordering::SeqCst);
        GATEWAY_TIMEOUT
    } else if DONE_UNREADABLE_RELEASED.load(Ordering::SeqCst) {
        UNAVAILABLE
    } else {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    }
}

/// A clean Done, mid edit session under a lock this process took, against a stub
/// answering `route`.
fn done_against(route: fn(&str) -> (u16, &'static str)) -> Observed {
    let (base, _listener, seen) = routing_stub(route);
    let (_temp, state) = editing_fixture(&base);
    let answer = format!("{:?}", state.speleodb_done(None).map_err(|e| e.message));
    observe(&state, answer, &seen)
}

#[test]
fn done_lost_release_reply_over_a_freed_lock_finishes_released() {
    let seen = done_against(done_freed);
    println!("[FZ-SDB-6] Done, unreadable release body then the lock is free -> {seen:?}");
    assert_eq!(
        seen.answer, "Ok(Released)",
        "the release landed: Done finishes"
    );
    assert!(!seen.gate, "the gate is shut");
    assert!(!seen.claim_kept, "and the claim is gone with the lock");
    assert_eq!(seen.reads_after_release, 1, "the holder is re-read at once");
}

#[test]
fn done_lost_release_reply_still_ours_keeps_editing() {
    let seen = done_against(admin_still_ours);
    println!("[FZ-SDB-6] Done, 504 then the lock is still ours -> {seen:?}");
    assert_eq!(seen.reads_after_release, 1, "the holder is re-read at once");
    assert!(
        seen.answer.starts_with("Err("),
        "the release did not land: {}",
        seen.answer
    );
    assert!(
        seen.gate && seen.claim_kept,
        "editing and the claim stay, so a retry of Done re-attempts the release"
    );
}

#[test]
fn done_lost_release_reply_unreadable_keeps_editing() {
    let seen = done_against(done_unreadable);
    println!("[FZ-SDB-6] Done, 504 then the re-read fails -> {seen:?}");
    assert!(
        seen.reads_after_release >= 1,
        "the holder re-read was attempted"
    );
    assert!(
        seen.answer.starts_with("Err("),
        "with nothing known the release is reported failed: {}",
        seen.answer
    );
    assert!(
        seen.gate && seen.claim_kept,
        "editing and the claim stay for the next poll"
    );
}

#[test]
fn done_refused_release_reads_nothing_back() {
    let seen = done_against(refused);
    println!("[FZ-SDB-6] Done, release refused with a 400 -> {seen:?}");
    assert_eq!(
        seen.reads_after_release, 0,
        "an answered failure is not re-read: {:?}",
        seen.requests
    );
    assert!(
        seen.answer.starts_with("Err("),
        "the refusal is reported: {}",
        seen.answer
    );
    assert!(seen.gate && seen.claim_kept, "and editing continues");
}

// --- Start Editing's acquire ------------------------------------------------------

static START_OURS_ACQUIRED: AtomicBool = AtomicBool::new(false);

/// The probe finds the lock free; the acquire lands with an unreadable reply, and
/// from then on the lock is our account's.
fn start_lost_then_ours(line: &str) -> (u16, &'static str) {
    if is_acquire(line) {
        START_OURS_ACQUIRED.store(true, Ordering::SeqCst);
        (200, TRUNCATED_PROJECT)
    } else if START_OURS_ACQUIRED.load(Ordering::SeqCst) {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    } else {
        (200, FREE_MUTEX_AT_BASE_BODY)
    }
}

/// The probe finds the lock free; the acquire answers 504 and did not land.
fn start_lost_then_unheld(line: &str) -> (u16, &'static str) {
    if is_acquire(line) {
        GATEWAY_TIMEOUT
    } else {
        (200, FREE_MUTEX_AT_BASE_BODY)
    }
}

/// Start Editing on an open, signed-in, not-editing copy against a stub answering
/// `route`. `reads_after_release` counts the reads after the ACQUIRE here.
fn start_against(route: fn(&str) -> (u16, &'static str)) -> Observed {
    let (base, _listener, seen) = routing_stub(route);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);
    link_open_copy_to(&state, &base);
    let answer = format!(
        "{:?}",
        state.speleodb_start_editing().map_err(|e| e.message)
    );
    let mut observed = observe(&state, answer, &seen);
    observed.reads_after_release = reads_after(&observed.requests, is_acquire, REMOTE_ID);
    observed
}

/// Whether the request right after the acquire re-reads the project's holder.
fn reread_at_once(requests: &[String]) -> bool {
    requests
        .iter()
        .skip_while(|r| !is_acquire(r))
        .nth(1)
        .is_some_and(|r| is_project_read(r, REMOTE_ID))
}

/// [#576 FZ-SDB-6-D1] The acquire landed and only its reply was lost: the re-read
/// says the lock is ours right after our own acquire, so Start Editing goes on and
/// the claim is recorded (the probe saw it free), and a later quit or logout hands
/// it back. It used to fail with the gate shut and no claim, while the server held
/// the lock for this account; the next Start Editing then took it as inherited, and
/// no silent release ever handed it back (farm seed 100026, INb).
#[test]
fn start_editing_lost_acquire_reply_ours_edits_and_claims() {
    let seen = start_against(start_lost_then_ours);
    println!("[FZ-SDB-6] Start Editing, unreadable acquire body then ours -> {seen:?}");
    assert!(
        reread_at_once(&seen.requests),
        "the holder is re-read at once: {:?}",
        seen.requests
    );
    assert!(
        seen.answer.starts_with("Ok(Editing"),
        "the acquire landed: editing starts: {}",
        seen.answer
    );
    assert!(seen.gate, "the gate is open");
    assert!(
        seen.claim_kept,
        "and the lock this call took is claimed, so it is handed back later"
    );
}

/// [#576 FZ-SDB-6-D1] The acquire answered 504 and did not land: the re-read finds
/// the lock free, and Start Editing fails as before, gate shut, no claim.
#[test]
fn start_editing_lost_acquire_reply_unheld_fails_and_claims_nothing() {
    let seen = start_against(start_lost_then_unheld);
    println!("[FZ-SDB-6] Start Editing, 504 then the lock is free -> {seen:?}");
    assert!(
        reread_at_once(&seen.requests),
        "the holder is re-read at once: {:?}",
        seen.requests
    );
    assert!(
        seen.answer.starts_with("Err("),
        "the acquire did not land: {}",
        seen.answer
    );
    assert!(!seen.gate && !seen.claim_kept, "nothing is taken");
}

// --- publish's acquire ------------------------------------------------------------

/// The environment variable that lets the child-process bodies run.
const CHILD_ENV: &str = "STALAGMIGHT_LOST_REPLY_CHILD";
/// The bodies' full test names, for the child's exact filter.
const OURS_CHILD: &str = "core::state::sdb_lost_reply_tests::publish_lost_acquire_ours_body";
const UNHELD_CHILD: &str = "core::state::sdb_lost_reply_tests::publish_lost_acquire_unheld_body";

/// The project the "ours" server creates, how it describes it (our account on the
/// lock), and the commit it answers the upload with.
const OURS_ID: &str = "66666666-6666-6666-6666-666666666666";
const OURS_PROJECT: &str = r#"{"id":"66666666-6666-6666-6666-666666666666","name":"LostOurs"}"#;
const OURS_HELD: &str = r#"{"id":"66666666-6666-6666-6666-666666666666","name":"LostOurs",
    "active_mutex":{"user":"u@x"}}"#;
const OURS_TRUNCATED: &str = r#"{"id":"66666666-6666-6666-6666-666666666666","na"#;
const OURS_SHA: &str = "0f1e2d0000000000000000000000000000000000";

/// The project the "unheld" server creates; nobody is on its lock.
const UNHELD_ID: &str = "77777777-7777-7777-7777-777777777777";
const UNHELD_PROJECT: &str = r#"{"id":"77777777-7777-7777-7777-777777777777","name":"LostUnheld"}"#;

/// The acquire landed with an unreadable reply; the re-read names our account.
fn acquire_lost_then_ours(line: &str) -> (u16, &'static str) {
    let path = path_of(line);
    if is_acquire(line) {
        (200, OURS_TRUNCATED)
    } else if line.starts_with("PUT ") && path.ends_with("/upload/auto/") {
        (
            200,
            r#"{"hexsha":"0f1e2d0000000000000000000000000000000000",
                "project":{"id":"66666666-6666-6666-6666-666666666666","name":"LostOurs"}}"#,
        )
    } else if path.ends_with("/revisions/") {
        (
            200,
            r#"{"commits":[{"id":"0f1e2d0000000000000000000000000000000000",
                "parent_ids":[],"author_email":"u@x"}]}"#,
        )
    } else if line.starts_with("POST /api/v2/projects/ ") {
        (201, OURS_PROJECT)
    } else if is_release(line) {
        (200, OURS_PROJECT)
    } else {
        (200, OURS_HELD)
    }
}

/// The acquire answered 504 and did not land; the re-read finds the lock free.
fn acquire_lost_then_unheld(line: &str) -> (u16, &'static str) {
    if is_acquire(line) {
        GATEWAY_TIMEOUT
    } else if path_of(line).ends_with("/revisions/") {
        (200, r#"{"commits":[]}"#)
    } else if line.starts_with("POST /api/v2/projects/ ") {
        (201, UNHELD_PROJECT)
    } else {
        (200, UNHELD_PROJECT)
    }
}

/// What one publish left behind.
#[derive(Debug)]
struct Published {
    project_id: String,
    gate: bool,
    force_edit: bool,
    claim_kept: bool,
    /// The open project's link: (project id, BASE).
    link: Option<(String, Option<String>)>,
    reads_after_acquire: usize,
    requests: Vec<String>,
}

/// Open a fresh local project named `name` in `dir`, sign in to a stub answering
/// with `route`, and publish it.
fn publish_against(dir: &Path, name: &str, route: fn(&str) -> (u16, &'static str)) -> Published {
    let path = dir.join(format!("{name}.stalx"));
    project_file(&path, name, "SectionA");
    let state = state_with_null_sink();
    assert!(state.open_project(path).success, "open the local project");
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
    let project_id = report.result.project_id;
    Published {
        gate: state.edit_enabled(),
        force_edit: state.force_editing(),
        claim_kept: state.speleodb_lock_acquired_here(&project_id),
        link: state
            .speleodb_open_link()
            .ok()
            .map(|l| (l.project_id, l.base_commit_sha)),
        reads_after_acquire: reads_after(&requests, is_acquire, &project_id),
        requests,
        project_id,
    }
}

/// Re-run this test binary on the ignored child body `body`, and require it passed.
fn run_child(body: &str) {
    let exe = std::env::current_exe().expect("this test binary");
    let output = output_within(
        std::process::Command::new(exe)
            .args([
                body,
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

/// Publish `name` against `route` in a child process's private app dirs, or
/// `None` outside the child.
fn publish_in_child(name: &str, route: fn(&str) -> (u16, &'static str)) -> Option<Published> {
    if std::env::var_os(CHILD_ENV).is_none() {
        println!("not in the child process ({CHILD_ENV} unset): nothing to do");
        return None;
    }
    let _cfg = crate::test_helpers::isolated_config();
    let dirs = TempDir::new().expect("app dirs");
    crate::app_dirs::init_paths(
        dirs.path().join("data"),
        dirs.path().join("config"),
        dirs.path().join("cache"),
        None,
    );
    Some(publish_against(dirs.path(), name, route))
}

#[test]
fn publish_lost_acquire_reply_ours_uploads_and_releases() {
    run_child(OURS_CHILD);
}

#[test]
#[ignore = "the child-process body of publish_lost_acquire_reply_ours_uploads_and_releases: it \
            sets the process-wide app dirs, so it runs only in a process of its own"]
fn publish_lost_acquire_ours_body() {
    let Some(seen) = publish_in_child("LostOurs", acquire_lost_then_ours) else {
        return;
    };
    println!("[FZ-SDB-6] publish, unreadable acquire body then the lock is ours -> {seen:?}");
    assert_eq!(seen.project_id, OURS_ID, "the server's id is answered");
    let upload_at = seen.requests.iter().position(|r| r.starts_with("PUT "));
    let release_at = seen.requests.iter().position(|r| is_release(r));
    assert!(
        upload_at.is_some() && release_at > upload_at,
        "the acquire landed, so the initial upload went out and the lock was released \
         after it: {:?}",
        seen.requests
    );
    assert_eq!(
        seen.link,
        Some((OURS_ID.to_owned(), Some(OURS_SHA.to_owned()))),
        "the published project is linked at the pushed commit"
    );
    assert!(
        !seen.gate && !seen.force_edit,
        "linked without its lock held: read-only (FZ-SDB-2)"
    );
    assert!(!seen.claim_kept, "and no claim is left after the release");
    assert_eq!(
        seen.reads_after_acquire, 1,
        "the holder is re-read at once after the acquire"
    );
}

#[test]
fn publish_lost_acquire_reply_unheld_skips_the_upload() {
    run_child(UNHELD_CHILD);
}

#[test]
#[ignore = "the child-process body of publish_lost_acquire_reply_unheld_skips_the_upload: it \
            sets the process-wide app dirs, so it runs only in a process of its own"]
fn publish_lost_acquire_unheld_body() {
    let Some(seen) = publish_in_child("LostUnheld", acquire_lost_then_unheld) else {
        return;
    };
    println!("[FZ-SDB-6] publish, 504 acquire then the lock is free -> {seen:?}");
    assert_eq!(seen.project_id, UNHELD_ID, "the server's id is answered");
    assert_eq!(
        seen.reads_after_acquire, 1,
        "the holder is re-read at once after the acquire"
    );
    assert!(
        !seen
            .requests
            .iter()
            .any(|r| r.starts_with("PUT ") || is_release(r)),
        "the acquire did not land: no upload and nothing to release: {:?}",
        seen.requests
    );
    assert!(
        !seen.gate && !seen.force_edit,
        "linked without its lock held: read-only (FZ-SDB-2)"
    );
    assert!(!seen.claim_kept, "and no claim was recorded");
}
