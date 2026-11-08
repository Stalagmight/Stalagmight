//! [#576 W90] The read-only verbs ask nothing of the signed-in server about a copy
//! linked to another SpeleoDB server.
//!
//! The status poll, the revision history and the dump question a Revert asks first
//! each send the SESSION's token to the session's server and name the open copy's
//! project id. For a copy linked to server X, open while signed in to server Y, that
//! asks Y about a project of X: Y answers for whatever it holds under that id, or
//! with an error. W88 and W89 refuse the write verbs over such a copy; these reads
//! are skipped instead, quietly - no request, and the answer each one already gives
//! when there is nothing to show (the poll as signed out, an empty history, no
//! question). A project picked from the signed-in server's own list (another id) is
//! asked about as before, and so is the same server spelled another way.
//!
//! Driven against the loopback recording stub. All data is synthetic, built in code.

use super::speleodb::{
    authed_session_at, link_open_copy_to, recording_stub, remote_readonly_fixture,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::AppState;
use crate::speleodb::RemoteState;

/// The server the copy is linked to in the mismatch case (never reachable).
const LINKED_TO: &str = "http://x.invalid/";

/// A project of the signed-in server's own list, not the open copy's.
const LISTED_ID: &str = "22222222-2222-2222-2222-222222222222";

/// A revision to ask the dump question about.
const HEXSHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// `endpoint` spelled another way: the scheme upper-cased, a trailing slash added.
fn spelled_another_way(endpoint: &str) -> String {
    format!(
        "{}/",
        endpoint.trim_end_matches('/').replacen("http", "HTTP", 1)
    )
}

/// The remote fixture signed in to a fresh recording stub that answers like SpeleoDB
/// for a project of the fixture's id (held by this account, at BASE), with the copy
/// linked to `link(stub)`. Answers the state, the stub's URL and its request log; the
/// first two values keep the copy's folder and the stub's port alive.
fn signed_in_with_copy_linked_to(
    link: fn(&str) -> String,
) -> (
    tempfile::TempDir,
    std::net::TcpListener,
    AppState,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let (stub, listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&stub);
    link_open_copy_to(&state, &link(&stub));
    (temp, listener, state, seen)
}

/// The requests a recording stub saw.
fn requests(seen: &std::sync::Mutex<Vec<String>>) -> Vec<String> {
    seen.lock().map(|lines| lines.clone()).unwrap_or_default()
}

/// What the poll reported about the server: (permission known, lock holder, remote
/// state, access denied, server error).
type ServerFields = (bool, Option<String>, RemoteState, bool, bool);

fn poll(state: &AppState) -> Result<(bool, ServerFields), String> {
    state
        .speleodb_link_status()
        .map(|s| {
            (
                s.is_remote,
                (
                    s.permission.is_some(),
                    s.lock_holder,
                    s.remote_state,
                    s.access_denied,
                    s.server_error.is_some(),
                ),
            )
        })
        .map_err(|e| e.message)
}

#[test]
fn the_status_poll_sends_nothing_for_a_copy_of_another_server() {
    let (_temp, _listener, state, seen) = signed_in_with_copy_linked_to(|_| LINKED_TO.to_owned());
    let other = poll(&state);
    let other_sent = requests(&seen);

    let (_s_temp, _s_listener, same, same_seen) =
        signed_in_with_copy_linked_to(spelled_another_way);
    let same_answer = poll(&same);
    let same_sent = requests(&same_seen);

    println!(
        "[W90 item B, status poll] linked to {LINKED_TO}, signed in elsewhere: {other:?}, \
         requests to the signed-in server {other_sent:?}. Same server spelled another way: \
         {same_answer:?}, requests {same_sent:?}"
    );
    assert!(
        other_sent.is_empty(),
        "nothing reaches the signed-in server for a copy of another server ({other_sent:?})"
    );
    assert_eq!(
        other,
        Ok((true, (false, None, RemoteState::Unknown, false, false))),
        "the copy is still remote, and the server fields read as signed out: nothing to show"
    );
    assert!(
        same_sent.iter().any(|line| line.starts_with("GET ")),
        "the same server spelled another way is still asked ({same_sent:?})"
    );
    assert_eq!(
        same_answer.map(|(_, fields)| fields.1),
        Ok(Some("u@x".to_owned())),
        "and its answer is read"
    );
}

#[test]
fn the_revision_history_sends_nothing_for_a_copy_of_another_server() {
    let (_temp, _listener, state, seen) = signed_in_with_copy_linked_to(|_| LINKED_TO.to_owned());
    let other = state
        .speleodb_get_revisions(REMOTE_ID.to_owned())
        .map(|revisions| revisions.len())
        .map_err(|e| e.message);
    let other_sent = requests(&seen);
    // A project of the signed-in server's own list, while that copy is open.
    let listed = state
        .speleodb_get_revisions(LISTED_ID.to_owned())
        .map(|revisions| revisions.len())
        .map_err(|e| e.message);
    let listed_sent = requests(&seen);

    let (_s_temp, _s_listener, same, same_seen) =
        signed_in_with_copy_linked_to(spelled_another_way);
    let same_answer = same
        .speleodb_get_revisions(REMOTE_ID.to_owned())
        .map(|revisions| revisions.len())
        .map_err(|e| e.message);
    let same_sent = requests(&same_seen);

    println!(
        "[W90 item B, revision history] linked to {LINKED_TO}, signed in elsewhere: \
         {other:?} revisions, requests {other_sent:?}; then a listed project {LISTED_ID}: \
         {listed:?}, requests {listed_sent:?}. Same server spelled another way: \
         {same_answer:?}, requests {same_sent:?}"
    );
    assert!(
        other_sent.is_empty(),
        "nothing reaches the signed-in server for a copy of another server ({other_sent:?})"
    );
    assert_eq!(other, Ok(0), "an empty history: nothing to show");
    assert_eq!(
        listed_sent.len(),
        1,
        "a project of the signed-in server's own list is still asked about"
    );
    assert!(
        listed_sent
            .first()
            .is_some_and(|line| line.contains(LISTED_ID)),
        "under its own id ({listed_sent:?})"
    );
    assert_eq!(listed, Ok(1), "and its history read");
    assert_eq!(
        same_sent.len(),
        1,
        "the same server spelled another way is still asked"
    );
    assert_eq!(same_answer, Ok(1));
}

#[test]
fn the_dump_question_sends_nothing_for_a_copy_of_another_server() {
    let (_temp, _listener, state, seen) = signed_in_with_copy_linked_to(|_| LINKED_TO.to_owned());
    let other = state
        .speleodb_dump_candidates(REMOTE_ID.to_owned(), Some(HEXSHA.to_owned()))
        .map(|candidates| candidates.is_some())
        .map_err(|e| e.message);
    let other_sent = requests(&seen);
    // A project of the signed-in server's own list (the pull's question, at the tip).
    let listed = state
        .speleodb_dump_candidates(LISTED_ID.to_owned(), None)
        .map(|candidates| candidates.is_some())
        .map_err(|e| e.message);
    let listed_sent = requests(&seen);

    let (_s_temp, _s_listener, same, same_seen) =
        signed_in_with_copy_linked_to(spelled_another_way);
    let same_answer = same
        .speleodb_dump_candidates(REMOTE_ID.to_owned(), Some(HEXSHA.to_owned()))
        .map(|candidates| candidates.is_some())
        .map_err(|e| e.message);
    let same_sent = requests(&same_seen);

    println!(
        "[W90 item B, dump question] linked to {LINKED_TO}, signed in elsewhere: candidates \
         {other:?}, requests {other_sent:?}; then a listed project {LISTED_ID}: {listed:?}, \
         requests {listed_sent:?}. Same server spelled another way: {same_answer:?}, \
         requests {same_sent:?}"
    );
    assert!(
        other_sent.is_empty(),
        "nothing reaches the signed-in server for a copy of another server ({other_sent:?})"
    );
    assert_eq!(other, Ok(false), "no question: nothing to show");
    assert_eq!(
        listed_sent.len(),
        1,
        "a project of the signed-in server's own list is still asked about"
    );
    assert!(
        listed_sent
            .first()
            .is_some_and(|line| line.contains(LISTED_ID)),
        "under its own id ({listed_sent:?})"
    );
    assert_eq!(
        same_sent.len(),
        1,
        "the same server spelled another way is still asked ({same_sent:?})"
    );
    assert!(
        same_sent.first().is_some_and(|line| line.contains(HEXSHA)),
        "at the revision asked about"
    );
    // The stub's answer is no dump, so neither asks a question: the request is the
    // difference.
    assert_eq!((listed, same_answer), (Ok(false), Ok(false)));
}
