//! sdk#3: the push snapshot is fitted under SpeleoDB's upload limit BEFORE a byte
//! is sent.
//!
//! The server refuses a file over 5 MiB (`client::SERVER_UPLOAD_LIMIT_BYTES`)
//! only after the whole body has arrived. The
//! push path now measures the sanitized snapshot first: within the limit it is
//! unchanged; over it, the undo history is left out of the COPY when that is
//! enough; otherwise the push is refused with a clear error and no request.
//!
//! Every file here is synthetic: a one-section project plus an undo history of
//! incompressible payloads, sized against a small test limit.

use std::path::{Path, PathBuf};

use super::speleodb_push::{clear_snapshot_dirty, fit_snapshot_to_upload_limit};
use crate::models::Project;
use crate::persistence::Database;
use crate::speleodb::client::SERVER_UPLOAD_LIMIT_BYTES;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{SpeleoDbAction, SpeleoDbError};

/// Deterministic, incompressible text (the history columns are compressed on save).
fn noise(seed: u64, len: usize) -> String {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(b'a' + ((state >> 33) % 26) as u8)
        })
        .collect()
}

/// A sanitized push snapshot that KEEPS its undo history (Full mode), as the push
/// path leaves it right before the size check.
fn snapshot_with_history(dir: &Path, name: &str) -> PathBuf {
    let live = dir.join(format!("{name}-live.stalx"));
    {
        let mut db = Database::open(&live).expect("open live");
        db.save_project(&Project::new("Synthetic cave".to_owned(), None))
            .expect("save project");
        let entries = (0..120)
            .map(|i| {
                (
                    format!("{{\"cmd\":\"edit-{i}\",\"pad\":\"{}\"}}", noise(i, 64)),
                    format!("{{\"before\":\"{}\"}}", noise(i + 1_000, 2_048)),
                    None,
                )
            })
            .collect();
        db.save_undo_stack_with_timestamps(entries)
            .expect("save history");
    }
    let snapshot = dir.join(format!("{name}-snapshot.stalx"));
    Database::vacuum_snapshot(&live, &snapshot).expect("snapshot");
    clear_snapshot_dirty(&snapshot, true, None).expect("sanitize keeping history");
    snapshot
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).expect("snapshot size").len()
}

fn undo_rows(path: &Path) -> usize {
    Database::open_existing(path)
        .expect("open snapshot")
        .load_undo_stack_with_timestamps()
        .expect("read history")
        .len()
}

/// The size of the same snapshot once its history is stripped.
fn size_without_history(dir: &Path) -> u64 {
    let copy = snapshot_with_history(dir, "measure");
    clear_snapshot_dirty(&copy, false, None).expect("strip history");
    size(&copy)
}

#[test]
fn a_snapshot_within_the_limit_uploads_unchanged() {
    let dir = tempfile::tempdir().expect("temp");
    let snapshot = snapshot_with_history(dir.path(), "within");
    let before = std::fs::read(&snapshot).expect("bytes");
    assert!(
        undo_rows(&snapshot) > 0,
        "precondition: history in the copy"
    );

    let omitted = fit_snapshot_to_upload_limit(&snapshot, true, size(&snapshot))
        .expect("a file at the limit fits");

    assert!(!omitted, "nothing was left out");
    assert_eq!(std::fs::read(&snapshot).expect("bytes"), before);
    assert!(undo_rows(&snapshot) > 0, "the history still rides");
}

#[test]
fn an_oversized_snapshot_drops_its_undo_history_when_that_is_enough() {
    let dir = tempfile::tempdir().expect("temp");
    let snapshot = snapshot_with_history(dir.path(), "over");
    let with_history = size(&snapshot);
    let without_history = size_without_history(dir.path());
    assert!(
        without_history < with_history,
        "precondition: the history takes space ({without_history} vs {with_history})"
    );
    let limit = without_history + (with_history - without_history) / 2;

    let omitted = fit_snapshot_to_upload_limit(&snapshot, true, limit)
        .expect("fits once the history is left out");

    assert!(omitted, "the caller is told the history was left out");
    assert!(size(&snapshot) <= limit);
    assert_eq!(undo_rows(&snapshot), 0, "no history in the uploaded copy");
    let survey = Database::open_existing(&snapshot)
        .expect("open")
        .load_project()
        .expect("survey data survives");
    assert_eq!(survey.name, "Synthetic cave");
}

#[test]
fn a_snapshot_too_large_even_without_history_is_refused_before_upload() {
    let dir = tempfile::tempdir().expect("temp");
    let snapshot = snapshot_with_history(dir.path(), "still-over");
    let without_history = size_without_history(dir.path());
    let limit = without_history - 1;

    let err = fit_snapshot_to_upload_limit(&snapshot, true, limit)
        .expect_err("nothing fits, so nothing may be sent");

    assert!(
        matches!(err, SpeleoDbError::UploadTooLarge { size, limit: l } if size == without_history && l == limit),
        "got {err:?}"
    );
}

#[test]
fn without_history_to_drop_an_oversized_snapshot_is_refused_untouched() {
    // Every undo mode but Full has already stripped the history, so there is
    // nothing left to drop: refuse at once and leave the copy as it is.
    let dir = tempfile::tempdir().expect("temp");
    let snapshot = snapshot_with_history(dir.path(), "no-history");
    clear_snapshot_dirty(&snapshot, false, None).expect("strip history");
    let before = std::fs::read(&snapshot).expect("bytes");

    let err = fit_snapshot_to_upload_limit(&snapshot, false, size(&snapshot) - 1)
        .expect_err("over the limit");

    assert!(matches!(err, SpeleoDbError::UploadTooLarge { .. }));
    assert_eq!(std::fs::read(&snapshot).expect("bytes"), before);
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
fn the_push_fits_the_snapshot_after_sanitizing_and_before_uploading() {
    // The push needs an app cache directory, which the unit-test build does not
    // set up, so its ORDER is pinned structurally (as the #195 flush tripwire
    // does): sanitize, then fit, then stream.
    let src = include_str!("speleodb_push.rs")
        .split("\n#[cfg(test)]")
        .next()
        .unwrap_or_default();
    let upload = src
        .split_once("fn speleodb_upload")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split("\n    fn ").next())
        .expect("speleodb_upload body precedes the next method");
    let sanitize = upload
        .find("clear_snapshot_dirty(")
        .expect("the push sanitizes the snapshot");
    let fit = upload
        .find("fit_snapshot_to_upload_limit(")
        .expect("the push fits the snapshot to the upload limit");
    let send = upload
        .find("upload_stalx(")
        .expect("the push streams the snapshot");
    assert!(sanitize < fit, "the size that counts is the sanitized one");
    assert!(fit < send, "the limit is checked before anything is sent");
}
