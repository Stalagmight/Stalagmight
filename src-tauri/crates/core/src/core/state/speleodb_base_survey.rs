//! S2 (F2-phantom-unpushed): whether a SpeleoDB copy holds un-uploaded work is
//! decided by the survey it holds, not by its mark alone.
//!
//! `mark_remote_dirty` makes the mark durable at the edit (a local-only save),
//! so that a crash after an edit still knows there was work; the edit itself
//! reaches the disk only with the next save. A crash, or a close with "Don't
//! save", in between leaves the mark over a file that still holds its BASE
//! revision, and every reader took the mark at its word: the next open refused
//! "this project has local edits that were never pushed", the exit and
//! sign-out prompts asked about work that did not exist. 0.6.0 did the same
//! (one UPDATE per edit).
//!
//! The rule: the digest of BASE's survey (`local.speleodb.base_survey`,
//! [`stalx_v2::survey_digest`]) is recorded wherever the copy is known to hold
//! BASE: every install of a revision under a clean link
//! (`PreparedDownload::cache_bytes`), every confirmed upload (the landing, from
//! the bytes that went up). A marked copy whose survey has that digest, and
//! that has no unsaved changes, holds no un-uploaded work:
//!
//! - a link reader of a file on disk ([`probe_settled`], behind `probe_link`
//!   and `read_cache_file`) reads the mark as cleared and writes nothing;
//! - an open clears it ([`settle_on_open`]) and the open's local-only save
//!   writes that;
//! - a save whose survey is BASE's again clears it in the file it writes
//!   ([`clear_stale_mark`]): an edit taken back, by hand or by Undo.
//!
//! Only the mark goes: `last_edit_at` stays, so a push in flight still sees
//! that an edit came after it started, and its landing keeps the copy marked.
//!
//! What it never does: count a copy as clean whose survey differs from BASE's,
//! or whose BASE digest is not known (a 0.6.0 copy, a version 2 copy installed
//! before the digest existed, a Restore, which marks the copy over a BASE it
//! does not hold). Those keep today's answer: the mark is taken at its word.

use std::path::Path;

use super::open_file::FileState;
use super::AppState;
use crate::persistence::stalx_v2::{
    self, Extras, Fingerprint, Format, Header, LocalSync, MAX_V2_FILE_BYTES,
};
use crate::speleodb::SpeleoDbError;

/// Whether `sync` marks un-uploaded work over a survey that is exactly BASE's:
/// `bytes` (the whole file the sync state belongs to) hold the survey whose
/// digest was recorded for the BASE `sync` names.
pub(super) fn holds_base_survey(bytes: &[u8], sync: &LocalSync) -> bool {
    if !sync.unpushed_edits {
        return false;
    }
    let (Some(base), Some(recorded)) = (sync.base_commit.as_deref(), sync.base_survey.as_deref())
    else {
        return false;
    };
    stalx_v2::survey_digest(bytes, base).is_ok_and(|digest| digest == recorded)
}

/// The digest to record for `bytes` as the survey of the BASE revision `base`;
/// `None` for an empty repository (no revision) or bytes with no digest.
pub(super) fn base_digest(bytes: &[u8], base: Option<&str>) -> Option<String> {
    let base = base?;
    match stalx_v2::survey_digest(bytes, base) {
        Ok(digest) => Some(digest),
        Err(e) => {
            tracing::warn!(
                error = %e,
                purpose = "record the survey digest of a SpeleoDB copy's BASE",
                "speleodb: no survey digest for BASE; its un-uploaded mark is taken at its word"
            );
            None
        }
    }
}

/// [`stalx_v2::probe_path`] with the stale mark settled: when the header marks
/// un-uploaded work over a BASE whose survey digest it recorded, the whole file
/// is read, and if it holds exactly that survey the mark reads as cleared.
/// Nothing is written (the link readers are read-only). The answer comes from
/// the one read of the whole file; a failure to read it keeps the header as
/// first read, mark and all.
pub(super) fn probe_settled(path: &Path) -> std::io::Result<Header> {
    let header = stalx_v2::probe_path(path)?;
    let worth_reading = header.format == Format::V2
        && header.complete
        && header.local.as_ref().is_some_and(|local| {
            let sync = &local.speleodb;
            sync.unpushed_edits && sync.base_commit.is_some() && sync.base_survey.is_some()
        });
    if !worth_reading {
        return Ok(header);
    }
    let bytes = match Fingerprint::read(path, MAX_V2_FILE_BYTES) {
        Ok((bytes, _)) => bytes,
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                error = %e,
                purpose = "tell a stale un-uploaded mark from un-uploaded work",
                "speleodb: the marked copy could not be read whole; its mark stands"
            );
            return Ok(header);
        }
    };
    let mut whole = stalx_v2::probe_bytes(&bytes);
    if whole.format != Format::V2 || !whole.complete {
        return Ok(header);
    }
    if let Some(local) = whole.local.as_mut() {
        if holds_base_survey(&bytes, &local.speleodb) {
            tracing::info!(
                path = %path.display(),
                purpose = "tell a stale un-uploaded mark from un-uploaded work",
                "speleodb: the copy holds exactly its BASE survey; its un-uploaded mark is stale"
            );
            local.speleodb.unpushed_edits = false;
        }
    }
    Ok(whole)
}

/// The open's half: a file just read (`bytes`) whose mark is stale opens clean.
/// The mark is cleared in memory and left for the open's local-only save
/// (`local_pending`, written by `persist_pending_local_state`). Returns whether
/// it was cleared.
pub(super) fn settle_on_open(file: &mut FileState, bytes: &[u8]) -> bool {
    if file.dirty || !holds_base_survey(bytes, &file.extras.local.speleodb) {
        return false;
    }
    tracing::info!(
        purpose = "tell a stale un-uploaded mark from un-uploaded work",
        "speleodb: the opened copy holds exactly its BASE survey; its un-uploaded mark is cleared"
    );
    file.extras.local.speleodb.unpushed_edits = false;
    file.local_pending = true;
    true
}

/// The save's half: `bytes` (the file a save is about to write, encoded from
/// `extras`) hold BASE's survey again under a mark, so the mark goes from the
/// file it writes. Returns the bytes to write instead, with the mark cleared in
/// `extras`; `None` leaves both as they are.
pub(super) fn clear_stale_mark(bytes: &[u8], extras: &mut Extras) -> Option<Vec<u8>> {
    if !holds_base_survey(bytes, &extras.local.speleodb) {
        return None;
    }
    extras.local.speleodb.unpushed_edits = false;
    match stalx_v2::rewrite_local(bytes, extras) {
        Ok(cleared) => {
            tracing::info!(
                purpose = "tell a stale un-uploaded mark from un-uploaded work",
                "speleodb: the saved copy holds exactly its BASE survey again; its un-uploaded mark is cleared"
            );
            Some(cleared)
        }
        Err(e) => {
            extras.local.speleodb.unpushed_edits = true;
            tracing::warn!(
                error = %e,
                purpose = "tell a stale un-uploaded mark from un-uploaded work",
                "speleodb: could not clear a stale un-uploaded mark; it stands"
            );
            None
        }
    }
}

impl AppState {
    /// `speleodb_mutate_link_held` for a confirmed upload: `record` changes the
    /// link, and when `pushed` (the bytes that went up) is BASE now, its survey
    /// digest is recorded for that BASE in the same durable save. The caller
    /// passes `pushed` only when the landing made those bytes the copy's BASE
    /// (a new revision with its id, or a 304 over BASE).
    pub(super) fn speleodb_record_landed_link(
        &self,
        project: Option<&crate::models::survey::Project>,
        pushed: Option<&[u8]>,
        record: impl FnOnce(&mut crate::models::SpeleoLink),
    ) -> Result<(), SpeleoDbError> {
        let changed = self.with_file(|file| {
            let mut link = file.speleo_link()?;
            record(&mut link);
            file.set_speleo_link(Some(&link));
            if let Some(bytes) = pushed {
                let digest = base_digest(bytes, link.base_commit_sha.as_deref());
                file.extras.local.speleodb.base_survey = digest;
            }
            Some(())
        });
        if changed.is_none() {
            return Err(SpeleoDbError::NoRemoteProject);
        }
        self.persist_local_state_at(project.and_then(|p| p.file_path.clone()))
            .map_err(|e| SpeleoDbError::Io(e.to_string()))
    }
}
