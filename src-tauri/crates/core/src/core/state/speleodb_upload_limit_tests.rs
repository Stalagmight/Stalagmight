//! sdk#3 and R2 H8: the push snapshot is fitted under SpeleoDB's upload limit
//! BEFORE a byte is sent.
//!
//! The server refuses a file over 5 MiB (`client::SERVER_UPLOAD_LIMIT_BYTES`)
//! only after the whole body has arrived. The push serializes the project in
//! the version 2 "for push" mode (plan 6.5): no `local`, no redo, the undo
//! history only in Full mode, cut at the inherited watermark and then from the
//! OLDEST end so the whole file fits. A survey that does not fit even without
//! its history is refused with a clear error and no request.
//!
//! Every file here is synthetic: a small survey plus an undo history of
//! incompressible entries.

use chrono::{DateTime, Utc};

use super::speleodb_push::push_snapshot;
use super::stalx_fixtures::{extras_for, fixture_link, line_survey, raw_entries};
use crate::models::survey::Project;
use crate::models::UndoMode;
use crate::persistence::stalx_v2::{decode, DecodeOptions, Decoded, Extras, History, RawJson};
use crate::speleodb::client::SERVER_UPLOAD_LIMIT_BYTES;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{SpeleoDbAction, SpeleoDbError};

fn first_entry_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
        .expect("a fixture time")
        .with_timezone(&Utc)
}

/// A linked Full-mode project with `count` undo entries of about `pad` bytes
/// each, and one redo entry (which a push never carries).
fn with_history(count: usize, pad: usize) -> (Project, Extras) {
    let link = fixture_link("https://speleodb.example", true);
    let mut extras = extras_for(Some(&link));
    extras.history = History {
        undo: raw_entries(first_entry_time(), count, pad),
        redo: raw_entries(first_entry_time() + chrono::Duration::days(1), 1, pad),
    };
    (line_survey("Synthetic cave"), extras)
}

fn decoded(bytes: &[u8]) -> Decoded {
    decode(bytes, &DecodeOptions::default()).expect("the push snapshot is a valid file")
}

/// The size of the same snapshot with no history at all.
fn size_without_history(project: &Project, extras: &Extras) -> usize {
    push_snapshot(project, extras, false, u64::MAX)
        .expect("serialize")
        .bytes
        .len()
}

fn full_size(project: &Project, extras: &Extras) -> usize {
    push_snapshot(project, extras, true, u64::MAX)
        .expect("serialize")
        .bytes
        .len()
}

fn limit(bytes: usize) -> u64 {
    u64::try_from(bytes).expect("a size")
}

#[test]
fn a_snapshot_within_the_limit_uploads_its_whole_undo_history_and_no_redo() {
    let (project, extras) = with_history(40, 256);
    let size = full_size(&project, &extras);
    let snapshot =
        push_snapshot(&project, &extras, true, limit(size)).expect("a file at the limit fits");
    assert!(!snapshot.undo_history_omitted, "nothing was left out");
    assert_eq!(snapshot.bytes.len(), size);
    let pushed = decoded(&snapshot.bytes);
    assert_eq!(
        pushed.extras.history.undo, extras.history.undo,
        "every undo entry rides"
    );
    assert!(
        pushed.extras.history.redo.is_empty(),
        "a push never carries redo"
    );
    assert!(pushed.extras.local.is_empty(), "nor any per-machine state");
}

#[test]
fn an_oversized_snapshot_keeps_its_newest_history_entries() {
    // R2 H8: the cut is by bytes, from the oldest end, so a busy Full-mode project
    // still ships the history of its latest work instead of none at all.
    let (project, extras) = with_history(60, 1_024);
    let with_history = full_size(&project, &extras);
    let without_history = size_without_history(&project, &extras);
    assert!(
        without_history < with_history,
        "precondition: the history takes space"
    );
    let cap = without_history + (with_history - without_history) / 2;

    let snapshot =
        push_snapshot(&project, &extras, true, limit(cap)).expect("fits with part of the history");

    assert!(snapshot.bytes.len() <= cap);
    assert!(
        !snapshot.undo_history_omitted,
        "part of the history rides, so the commit is not one without history"
    );
    let kept = decoded(&snapshot.bytes).extras.history.undo;
    assert!(!kept.is_empty() && kept.len() < extras.history.undo.len());
    let newest: Vec<RawJson> =
        extras.history.undo[extras.history.undo.len() - kept.len()..].to_vec();
    assert_eq!(
        kept, newest,
        "the kept entries are the newest ones, in order"
    );
}

#[test]
fn a_snapshot_that_fits_only_without_history_says_so() {
    let (project, extras) = with_history(30, 2_048);
    let without_history = size_without_history(&project, &extras);
    // Room for the survey and less than one entry.
    let snapshot = push_snapshot(&project, &extras, true, limit(without_history + 100))
        .expect("fits once the history is left out");
    assert!(
        snapshot.undo_history_omitted,
        "the caller is told the history was left out"
    );
    let pushed = decoded(&snapshot.bytes);
    assert!(
        pushed.extras.history.undo.is_empty(),
        "no history in the uploaded copy"
    );
    assert_eq!(
        pushed.project.name, "Synthetic cave",
        "the survey data survives"
    );
}

#[test]
fn a_snapshot_too_large_even_without_history_is_refused_before_upload() {
    let (project, extras) = with_history(10, 256);
    let without_history = size_without_history(&project, &extras);
    let cap = limit(without_history - 1);

    let err = push_snapshot(&project, &extras, true, cap)
        .expect_err("nothing fits, so nothing may be sent");

    assert!(
        matches!(err, SpeleoDbError::UploadTooLarge { size, limit: l } if size == limit(without_history) && l == cap),
        "got {err:?}"
    );
}

#[test]
fn without_history_to_drop_an_oversized_snapshot_is_refused() {
    // Every undo mode but Full ships no history, so there is nothing left to drop:
    // refuse at once.
    let (project, extras) = with_history(10, 256);
    let without_history = size_without_history(&project, &extras);
    let err = push_snapshot(&project, &extras, false, limit(without_history - 1))
        .expect_err("over the limit");
    assert!(matches!(err, SpeleoDbError::UploadTooLarge { .. }));
}

#[test]
fn a_disabled_project_policy_never_ships_history() {
    // The link's own undo mode backs the caller's flag up: a Disabled project
    // pushes no history even if the flag says keep.
    let (project, mut extras) = with_history(10, 64);
    if let Some(link) = extras.speleodb.as_mut() {
        link.undo_mode = Some(UndoMode::Disabled);
    }
    let snapshot =
        push_snapshot(&project, &extras, true, SERVER_UPLOAD_LIMIT_BYTES).expect("serialize");
    assert!(decoded(&snapshot.bytes).extras.history.undo.is_empty());
}

#[test]
fn history_at_or_before_the_watermark_is_not_pushed_again() {
    // [#303] A chain step loaded a past commit's history: what it loaded is that
    // commit's, already on the server, and is cut from the new one.
    let (project, mut extras) = with_history(10, 64);
    extras.local.speleodb.inherited_through =
        Some(first_entry_time() + chrono::Duration::seconds(5));
    let snapshot =
        push_snapshot(&project, &extras, true, SERVER_UPLOAD_LIMIT_BYTES).expect("serialize");
    let kept = decoded(&snapshot.bytes).extras.history.undo;
    assert_eq!(
        kept,
        extras.history.undo[6..].to_vec(),
        "entries 0..=5 are inherited"
    );
    assert!(
        !snapshot.undo_history_omitted,
        "an inherited cut is not a size cut"
    );
}

#[test]
fn a_full_mode_history_over_five_mib_still_pushes_under_the_limit_with_its_newest_entries() {
    // R2 H8 at the real limit: a Full-mode project whose history alone is over the
    // server's 5 MiB uploads in full survey, with as much of its newest history as
    // fits, and never over the limit.
    let (project, extras) = with_history(1_400, 4_096);
    assert!(
        full_size(&project, &extras) > usize::try_from(SERVER_UPLOAD_LIMIT_BYTES).expect("limit"),
        "precondition: over the limit with all of its history"
    );
    let snapshot = push_snapshot(&project, &extras, true, SERVER_UPLOAD_LIMIT_BYTES).expect("fits");
    assert!(limit(snapshot.bytes.len()) <= SERVER_UPLOAD_LIMIT_BYTES);
    assert!(!snapshot.undo_history_omitted);
    let kept = decoded(&snapshot.bytes).extras.history.undo;
    assert!(
        kept.len() > 1_000,
        "most of the history still rides: {}",
        kept.len()
    );
    assert_eq!(
        kept.last(),
        extras.history.undo.last(),
        "the newest entry is kept"
    );
}

#[test]
fn the_too_large_refusal_reads_clearly_and_fails_closed() {
    let dto = dto_for_error(
        Endpoint::UploadStalx,
        &SpeleoDbError::UploadTooLarge {
            size: 31_200_000,
            limit: SERVER_UPLOAD_LIMIT_BYTES,
        },
    );
    assert_eq!(dto.action, SpeleoDbAction::Fatal);
    assert_eq!(dto.status, None);
    assert!(
        dto.message.contains("too large to upload"),
        "{}",
        dto.message
    );
    assert!(dto.message.contains("31.2 MB"), "{}", dto.message);
    assert!(dto.message.contains("5.2 MB"), "{}", dto.message);
    assert!(
        dto.message.contains("Nothing was uploaded"),
        "{}",
        dto.message
    );
}

#[test]
fn the_push_fits_the_snapshot_before_uploading() {
    // The push needs an app cache directory, which the unit-test build does not
    // set up, so its ORDER is pinned structurally (as the #195 flush tripwire
    // does): save, then serialize and fit, then stream.
    let src = include_str!("speleodb_push.rs")
        .split("\n#[cfg(test)]")
        .next()
        .unwrap_or_default();
    let upload = src
        .split_once("fn speleodb_upload")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split("\n    fn ").next())
        .expect("speleodb_upload body precedes the next method");
    let save = upload
        .find("save_project_at_generation(")
        .expect("the push saves first");
    let fit = upload
        .find("push_snapshot(")
        .expect("the push serializes and fits the snapshot");
    let send = upload
        .find("upload_stalx(")
        .expect("the push streams the snapshot");
    assert!(save < fit, "the snapshot is what was saved");
    assert!(fit < send, "the limit is checked before anything is sent");
}
