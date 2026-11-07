//! [#576 NF-W4S2-2] A local refusal reads as itself, not as a filesystem error.
//!
//! The SpeleoDB verbs refused locally ("reconnect to acquire the write lock before
//! pushing", "start editing before changing the undo-history setting", ...) by
//! wrapping the sentence in `SpeleoDbError::Io`, whose Display is "filesystem
//! error: {0}" - and the DTO message is that Display, so the banner or toast said
//! "filesystem error: reconnect to acquire the write lock before pushing". Those
//! refusals now carry their own wording.

use super::speleodb::{
    authed_session_at, editing_fixture, recording_stub, remote_readonly_fixture,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY,
};
use crate::models::UndoMode;

fn check(what: &str, message: &str, expected: &str) {
    println!("{what}: {message:?}");
    assert!(
        !message.starts_with("filesystem error"),
        "{what}: a refusal is not a filesystem error: {message:?}"
    );
    assert_eq!(message, expected, "{what}");
}

#[test]
fn done_while_force_editing_says_why_in_plain_words() {
    let (base, _listener, _seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    state.set_force_editing(true);

    let done = state
        .speleodb_done(None)
        .expect_err("refused while force-editing");
    let push = state
        .speleodb_push_only(None)
        .expect_err("refused while force-editing");

    check(
        "Done",
        &done.message,
        "reconnect to acquire the write lock before pushing",
    );
    check(
        "Push",
        &push.message,
        "reconnect to acquire the write lock before pushing",
    );
}

#[test]
fn the_undo_mode_refusal_says_why_in_plain_words() {
    let (base, _listener, _seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);

    let err = state
        .speleodb_set_undo_mode(UndoMode::Full)
        .expect_err("refused while not editing");

    check(
        "undo mode while read-only",
        &err.message,
        "start editing before changing the undo-history setting",
    );
}
