//! [#576 W88] The edit gate never opens over a copy linked to another SpeleoDB server
//! than the one signed in.
//!
//! Every lock verb sends the SESSION's token to the session's server and names the
//! LINK's project: the acquire, the push, Done's release and the release verbs. So a
//! copy linked to server X, open while signed in to server Y, must not start an edit
//! session: Start editing used to acquire on Y with X's project id and open the gate,
//! and the status poll's inherited-lock reopen opened it whenever Y named this account
//! as the holder of a project with that id. (W87b's item 6 stopped the silent releases
//! that followed; this stops the gate itself.) The same server spelled another way -
//! the case of the scheme or host, a trailing slash, the default port - is still the
//! same server and edits as before. Restore's reopen is driven in
//! `speleodb_revisions.rs`, beside the step it tests.
//!
//! Driven against the loopback recording stub. All data is synthetic, built in code.

use super::speleodb::{
    authed_session_at, link_open_copy_to, recording_stub, remote_readonly_fixture,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::AppState;

/// The server `remote_readonly_fixture` links its copy to.
const LINKED_TO: &str = "http://x/";

/// `endpoint` spelled another way: the scheme upper-cased, a trailing slash added.
fn spelled_another_way(endpoint: &str) -> String {
    format!(
        "{}/",
        endpoint.trim_end_matches('/').replacen("http", "HTTP", 1)
    )
}

/// The requests a recording stub saw.
fn requests(seen: &std::sync::Mutex<Vec<String>>) -> Vec<String> {
    seen.lock().map(|lines| lines.clone()).unwrap_or_default()
}

/// (edit gate, force edit, any claim on the fixture's project).
fn gate_and_claim(state: &AppState) -> (bool, bool, bool) {
    (
        state.edit_enabled(),
        state.force_editing(),
        state.speleodb_lock_acquired_here(REMOTE_ID),
    )
}

#[test]
fn start_editing_refuses_a_copy_linked_to_another_server() {
    // Server Y: the session signs in here; it answers for the fixture's project id as
    // a free-to-take lock held by this account at our BASE (an UpToDate reconcile).
    let (y, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&y);

    let started = state
        .speleodb_start_editing()
        .map(|_| ())
        .map_err(|e| e.message);
    let after_start = gate_and_claim(&state);
    let resumed = state
        .speleodb_resume_held_lock_editing()
        .map(|_| ())
        .map_err(|e| e.message);
    let after_resume = gate_and_claim(&state);
    let sent = requests(&seen);

    // Control: the same server spelled another way.
    let (z, _z_listener, z_seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_z_temp, same) = remote_readonly_fixture();
    *same.speleodb.lock() = authed_session_at(&z);
    let spelled = spelled_another_way(&z);
    link_open_copy_to(&same, &spelled);
    let same_started = same
        .speleodb_start_editing()
        .map(|_| ())
        .map_err(|e| e.message);
    let same_gate = gate_and_claim(&same);
    let same_acquires = requests(&z_seen)
        .iter()
        .filter(|line| line.starts_with("POST") && line.contains("/acquire/"))
        .count();

    println!(
        "[W88 item 1, Start editing] linked to {LINKED_TO}, signed in to {y}: start -> \
         {started:?}, (gate, force, claim) {after_start:?}; resume -> {resumed:?}, \
         {after_resume:?}; requests to the signed-in server {sent:?}. Same server as \
         {spelled} vs {z}: start -> {same_started:?}, {same_gate:?}, acquires {same_acquires}"
    );
    let message = started.expect_err("Start editing over a copy linked to another server");
    assert!(
        message.contains(LINKED_TO) && message.contains("sign in to that server"),
        "the refusal names the copy's server and says what to do: {message}"
    );
    assert_eq!(
        after_start,
        (false, false, false),
        "the gate stays shut, nothing is claimed"
    );
    let resume_message = resumed.expect_err("Continue editing is the same flow");
    assert_eq!(resume_message, message, "and refused the same way");
    assert_eq!(after_resume, (false, false, false));
    assert!(
        sent.is_empty(),
        "nothing reaches the signed-in server: no probe, no acquire ({sent:?})"
    );
    assert!(
        same_started.is_ok(),
        "the same server spelled another way still edits"
    );
    assert_eq!(same_gate, (true, false, true), "with its gate and claim");
    assert_eq!(same_acquires, 1, "after one acquire");
}

#[test]
fn the_poll_never_reopens_the_gate_over_a_copy_linked_to_another_server() {
    // Server Y names this account as the holder of a project with the fixture's id:
    // the shape the poll's inherited-lock reopen opens the gate for.
    let (y, _listener, _seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&y);

    let status = state.speleodb_link_status().map_err(|e| e.message);
    let reported = status
        .as_ref()
        .map(|s| (s.edit_enabled, s.lock_holder.clone()));
    let after = gate_and_claim(&state);
    let resumed = state.speleodb_lock_resumed_here(REMOTE_ID);

    // Control: the same server spelled another way.
    let (z, _z_listener, _z_seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_z_temp, same) = remote_readonly_fixture();
    *same.speleodb.lock() = authed_session_at(&z);
    let spelled = spelled_another_way(&z);
    link_open_copy_to(&same, &spelled);
    let same_reported = same
        .speleodb_link_status()
        .map(|s| s.edit_enabled)
        .map_err(|e| e.message);
    let same_after = gate_and_claim(&same);

    println!(
        "[W88 item 1, poll reopen] linked to {LINKED_TO}, signed in to {y}: status \
         (edit_enabled, holder) {reported:?}, (gate, force, claim) {after:?}, adopted \
         {resumed}. Same server as {spelled} vs {z}: {same_reported:?}, {same_after:?}"
    );
    // [#576 W90] The poll no longer asks the signed-in server about such a copy at
    // all (`sdb_endpoint_read_tests`), so it reads no holder either.
    assert_eq!(
        reported,
        Ok((false, None)),
        "the poll asks nothing and leaves the copy read-only"
    );
    assert_eq!(after, (false, false, false), "the gate stays shut");
    assert!(!resumed, "and nothing is adopted");
    assert_eq!(
        same_reported,
        Ok(true),
        "the same server spelled another way still reopens"
    );
    assert_eq!(same_after, (true, false, true));
}
