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
//! The digest holds only while the server holds nothing of this copy past
//! BASE. An upload may land with its answer lost (#193), and then SpeleoDB's
//! latest revision may be ours, past BASE: a survey taken back to BASE's is a
//! change to upload, not a clean copy. So every upload drops the digest in the
//! save it starts with (`speleodb_upload`), and only a confirmed landing (or a
//! new install) records one again.
//!
//! What it never does: count a copy as clean whose survey differs from BASE's,
//! or whose BASE digest is not known (a 0.6.0 copy, a version 2 copy installed
//! before the digest existed, a Restore, which marks the copy over a BASE it
//! does not hold). Those keep today's answer: the mark is taken at its word.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use super::open_file::FileState;
use super::AppState;
use crate::persistence::stalx_v2::{self, Extras, Format, Header, LocalSync, MAX_V2_FILE_BYTES};
use crate::speleodb::SpeleoDbError;

/// Whether `sync`'s digest still speaks for the copy: every edit recorded since
/// it was recorded was recorded by a build that keeps it, so `last_edit_at` is
/// the one that build vouched for (`base_survey_edit`). An edit by a writer that
/// does not know the digest moved `last_edit_at` alone; that writer may also have
/// uploaded (its upload cannot drop a digest it does not know), so the digest no
/// longer says what SpeleoDB holds past BASE (F2-digest-2).
pub(super) fn digest_vouched(sync: &LocalSync) -> bool {
    sync.base_survey.is_some()
        && sync.last_edit_at.is_some()
        && sync.base_survey_edit == sync.last_edit_at
}

/// Whether `sync` marks un-uploaded work over a survey that is exactly BASE's:
/// `bytes` (the whole file the sync state belongs to) hold the survey whose
/// digest was recorded for the BASE `sync` names, and the digest is still
/// vouched for ([`digest_vouched`]).
pub(super) fn holds_base_survey(bytes: &[u8], sync: &LocalSync) -> bool {
    if !sync.unpushed_edits || !digest_vouched(sync) {
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
    let sync = header
        .local
        .as_ref()
        .map(|local| &local.speleodb)
        .filter(|sync| sync.unpushed_edits && sync.base_commit.is_some() && digest_vouched(sync));
    let worth_reading = header.format == Format::V2 && header.complete && sync.is_some();
    let Some(key) = sync
        .filter(|_| worth_reading)
        .map(|sync| UnsettledKey::of(path, sync))
    else {
        return Ok(header);
    };
    if key.as_ref().is_some_and(|key| unsettled_still(path, key)) {
        return Ok(header);
    }
    let bytes = match read_whole(path) {
        Ok(bytes) => bytes,
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
            // Debug, not info: a listing reads every cached copy each time it
            // is fetched, and this copy answers the same until it is opened.
            tracing::debug!(
                path = %path.display(),
                purpose = "tell a stale un-uploaded mark from un-uploaded work",
                "speleodb: the copy holds exactly its BASE survey; its un-uploaded mark is stale"
            );
            local.speleodb.unpushed_edits = false;
        } else if let Some(key) = key {
            remember_unsettled(path, key);
        }
    }
    Ok(whole)
}

/// What [`probe_settled`] last found to be real un-uploaded work at a path: the
/// file as it was then (its length and modification time) and the sync state
/// that asked (the digest and the edit time it was vouched for).
#[derive(Debug, Clone, PartialEq, Eq)]
struct UnsettledKey {
    len: u64,
    modified: SystemTime,
    digest: Option<String>,
    last_edit_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl UnsettledKey {
    /// `None` when the file's metadata cannot be read: nothing is remembered.
    fn of(path: &Path, sync: &LocalSync) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok()?,
            digest: sync.base_survey.clone(),
            last_edit_at: sync.last_edit_at,
        })
    }
}

/// The copies [`probe_settled`] found to hold real un-uploaded work (F2-digest-5).
/// Such a copy keeps its mark and its digest for as long as the work stays
/// un-uploaded, and every link reader of it (each listing fetch behind the Home
/// view and the sign-out and held-lock prompts, the local-copy state, logout's
/// forget-recents) read it whole and hashed it again. Only that answer is
/// remembered, and only while the file is unchanged: a stale entry can only keep
/// a mark, never clear one.
static UNSETTLED: Mutex<Option<HashMap<PathBuf, UnsettledKey>>> = Mutex::new(None);

/// More copies than anyone keeps; past it the memory starts over.
const UNSETTLED_KEPT: usize = 256;

/// Whether `path` is still the file [`probe_settled`] found real work in.
fn unsettled_still(path: &Path, key: &UnsettledKey) -> bool {
    UNSETTLED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|known| known.get(path))
        .is_some_and(|known| known == key)
}

fn remember_unsettled(path: &Path, key: UnsettledKey) {
    let mut known = UNSETTLED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let known = known.get_or_insert_with(HashMap::new);
    if known.len() >= UNSETTLED_KEPT {
        known.clear();
    }
    known.insert(path.to_path_buf(), key);
}

/// The file at `path`, at most [`MAX_V2_FILE_BYTES`] (the decoder's cap): a
/// larger one is no version 2 file this build reads, so its mark stands.
fn read_whole(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    #[cfg(test)]
    WHOLE_READS.with(|reads| reads.set(reads.get() + 1));
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_V2_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_V2_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "larger than a version 2 file this build reads",
        ));
    }
    Ok(bytes)
}

/// An edit is being recorded at `at` (`mark_remote_dirty`): the digest goes on
/// speaking for the copy only when it was still vouched for before this edit,
/// or no edit came since it was recorded. Then this build vouches for `at` too;
/// otherwise a writer that does not know the digest edited since, and the
/// digest goes (F2-digest-2). Call before `last_edit_at` is moved to `at`.
pub(super) fn vouch_for_edit(sync: &mut LocalSync, at: Option<chrono::DateTime<chrono::Utc>>) {
    if sync.base_survey.is_none() {
        sync.base_survey_edit = None;
        return;
    }
    if sync.base_survey_edit == sync.last_edit_at {
        sync.base_survey_edit = at;
    } else {
        sync.base_survey = None;
        sync.base_survey_edit = None;
    }
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
                let sync = &mut file.extras.local.speleodb;
                // A late edit kept marked was this session's, so it is vouched
                // for; with none, there is no edit since the digest.
                sync.base_survey_edit = digest.as_ref().and(sync.last_edit_at);
                sync.base_survey = digest;
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

#[cfg(test)]
thread_local! {
    /// How many times this thread read a marked copy whole ([`read_whole`]).
    static WHOLE_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// [`WHOLE_READS`] on this thread.
#[cfg(test)]
pub(super) fn whole_reads() -> usize {
    WHOLE_READS.with(std::cell::Cell::get)
}
