//! What this session uploaded of the open project, and what a landed upload
//! replaced (R3, the fuzzer's same-account-overwrite).
//!
//! SpeleoDB locks are per ACCOUNT. A second session of the same account
//! (another machine, another window) holds the project's lock too, and its
//! revisions carry our email, so neither the lock nor the author tells the push
//! which revisions are this session's own. Before an upload the push checks
//! that the server's HEAD is still the copy's BASE, and the one exception it
//! makes - the #193 recovery of our own upload whose answer was lost - may
//! only adopt a revision this record accounts for
//! ([`crate::speleodb::sync::OwnUploads`]). Anything else on top of BASE is
//! the divergence prompt, and nothing is uploaded.
//!
//! The server has no conditional upload, so the check can still lose a race:
//! the other session lands a revision between it and the upload, and the
//! server puts ours on top. The upload's answer cannot say so; the revision
//! history read right after it can ([`crate::speleodb::sync::replaced_by_upload`]).
//! What it finds waits here until the push's result reports it to the user, by
//! name, with where to get it back (it is still in the history).
//!
//! One record, for one project of one account on one deployment, in one
//! session generation: anything else reads as "nothing uploaded", which fails
//! closed to the divergence prompt. Process-local, never persisted - a restart
//! reopens through Start editing, whose reconcile has no authorship recovery.
//! Held only briefly, never across I/O.

use super::AppState;
use crate::speleodb::client;
use crate::speleodb::models::SpeleoRevision;
use crate::speleodb::sync::{self, OwnUploads, UnansweredUpload};

/// How many unanswered uploads the record remembers: retries sent while an
/// earlier upload's fate is unknown. A handful is plenty.
const UNANSWERED_KEPT: usize = 8;

/// How many landed revisions the record remembers. Only the ones a status
/// refresh can still meet in BASE..HEAD matter (the push advances BASE to each
/// as it lands), so a handful is plenty.
const LANDED_KEPT: usize = 8;

/// The record behind [`AppState::speleodb_own_uploads`].
#[derive(Debug)]
pub(super) struct OwnUploadsRecord {
    endpoint: String,
    email: String,
    project_id: String,
    generation: u64,
    uploads: OwnUploads,
    /// Revisions a landed upload replaced that no push result has reported yet.
    unreported: Vec<SpeleoRevision>,
}

impl OwnUploadsRecord {
    fn is_for(&self, endpoint: &str, email: &str, project_id: &str, generation: u64) -> bool {
        self.generation == generation
            && self.project_id == project_id
            && crate::speleodb::auth::normalize_endpoint(&self.endpoint)
                == crate::speleodb::auth::normalize_endpoint(endpoint)
            && self.email.eq_ignore_ascii_case(email)
    }
}

impl AppState {
    /// The live account (endpoint, email), read under one session acquisition.
    fn speleodb_own_uploads_account(&self) -> Option<(String, String)> {
        let session = self.speleodb_session_lock().ok()?;
        Some((session.endpoint.clone()?, session.email.clone()?))
    }

    /// Edit the record for `project_id` in `generation`, starting a fresh one
    /// when the one held is for anything else. A no-op once the session has
    /// moved past `generation`: nothing recorded for it could ever be read.
    fn speleodb_own_uploads_edit(
        &self,
        generation: u64,
        project_id: &str,
        edit: impl FnOnce(&mut OwnUploadsRecord),
    ) {
        let Some((endpoint, email)) = self.speleodb_own_uploads_account() else {
            return;
        };
        if self.speleodb_session_generation() != generation {
            return;
        }
        let mut slot = self
            .speleodb_own_uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let record = match slot.take() {
            Some(record) if record.is_for(&endpoint, &email, project_id, generation) => record,
            _ => OwnUploadsRecord {
                endpoint,
                email,
                project_id: project_id.to_owned(),
                generation,
                uploads: OwnUploads::default(),
                unreported: Vec::new(),
            },
        };
        let record = slot.insert(record);
        edit(record);
    }

    /// What this session knows it uploaded of `project_id` (see the module doc);
    /// empty unless the record is the live account's, in the live session.
    pub(super) fn speleodb_own_uploads(&self, project_id: &str) -> OwnUploads {
        let Some((endpoint, email)) = self.speleodb_own_uploads_account() else {
            return OwnUploads::default();
        };
        let generation = self.speleodb_session_generation();
        self.speleodb_own_uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|record| record.is_for(&endpoint, &email, project_id, generation))
            .map(|record| record.uploads.clone())
            .unwrap_or_default()
    }

    /// An upload is about to be sent: until its answer says what it made, it is
    /// an unanswered upload. An earlier one still unanswered stays recorded too:
    /// the retry's HEAD check can read HEAD == BASE while the server has yet to
    /// land the earlier upload, which can then land under the retry, and it is
    /// still ours (F4R3-locks-push-5). The oldest go past [`UNANSWERED_KEPT`].
    pub(super) fn speleodb_note_upload_sent(
        &self,
        generation: u64,
        project_id: &str,
        base: Option<String>,
        message: &str,
    ) {
        self.speleodb_own_uploads_edit(generation, project_id, |record| {
            let unanswered = &mut record.uploads.unanswered;
            unanswered.push(UnansweredUpload {
                base,
                message: message.to_owned(),
            });
            let excess = unanswered.len().saturating_sub(UNANSWERED_KEPT);
            unanswered.drain(..excess);
        });
    }

    /// The outcome of the upload just sent is known: it made `landed` (a 200
    /// that named it, or a revision the push adopted as its landing), or nothing
    /// (a 304, a refusal). A landing settles every unanswered upload: BASE moves
    /// to it, and whatever an earlier one made is below it now. Nothing landed
    /// settles only the upload just sent (the last one recorded): an earlier
    /// one may still land, or may be what a 304 matched.
    pub(super) fn speleodb_note_upload_answered(
        &self,
        generation: u64,
        project_id: &str,
        landed: Option<&str>,
    ) {
        self.speleodb_own_uploads_edit(generation, project_id, |record| {
            if landed.is_some() {
                record.uploads.unanswered.clear();
            } else {
                record.uploads.unanswered.pop();
            }
            if let Some(landed) = landed {
                let kept = &mut record.uploads.landed;
                if !kept.iter().any(|id| id.eq_ignore_ascii_case(landed)) {
                    kept.push(landed.to_owned());
                }
                let excess = kept.len().saturating_sub(LANDED_KEPT);
                kept.drain(..excess);
            }
        });
    }

    /// The revisions an upload of this session replaced and no result has
    /// reported yet, taken: the push result that carries them is the report.
    /// `None` when there are none.
    pub(super) fn speleodb_take_replaced(&self, project_id: &str) -> Option<Vec<SpeleoRevision>> {
        let (endpoint, email) = self.speleodb_own_uploads_account()?;
        let generation = self.speleodb_session_generation();
        let mut slot = self
            .speleodb_own_uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let record = slot
            .as_mut()
            .filter(|record| record.is_for(&endpoint, &email, project_id, generation))?;
        let replaced = std::mem::take(&mut record.unreported);
        (!replaced.is_empty()).then_some(replaced)
    }

    /// After an upload landed as `landed` on the BASE `upload_base`: did it take
    /// HEAD from revisions another session landed meanwhile (R3, the lost
    /// race)? Reads the history once and records what it finds for the push
    /// result to report ([`Self::speleodb_take_replaced`]). `unanswered` is what
    /// the record held as unanswered when the upload was answered: a revision
    /// one of those made is our own upload landing late, not another session's
    /// ([`sync::not_our_own`], F4R3-locks-push-5).
    ///
    /// A direct GET, NOT through the auth handler, like the pre-push name fold:
    /// the upload already landed, and a spurious 401 on this follow-up must not
    /// drop the session over it. A failed read, or a history that does not yet
    /// list `landed`, cannot tell, and says so in the log only.
    pub(super) fn speleodb_check_replaced_by_upload(
        &self,
        (endpoint, token, insecure_http): (&str, &str, bool),
        generation: u64,
        project_id: &str,
        unanswered: &[UnansweredUpload],
        upload_base: Option<&str>,
        landed: &str,
    ) {
        let history = match client::get_revisions(endpoint, token, insecure_http, project_id) {
            Ok(history) => history,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id,
                    purpose = "tell whether a landed upload replaced another session's revision (R3)",
                    "speleodb push: could not read the history after the upload; \
                     a revision landed meanwhile would go unreported"
                );
                return;
            }
        };
        if !history
            .commits
            .iter()
            .any(|c| c.id.eq_ignore_ascii_case(landed))
        {
            tracing::warn!(
                project_id,
                landed,
                purpose = "tell whether a landed upload replaced another session's revision (R3)",
                "speleodb push: the history read after the upload does not list it yet; \
                 a revision landed meanwhile would go unreported"
            );
            return;
        }
        let my_email = self
            .speleodb_own_uploads_account()
            .map(|(_, email)| email)
            .unwrap_or_default();
        let replaced = sync::not_our_own(
            sync::replaced_by_upload(upload_base, landed, &history.commits),
            unanswered,
            &my_email,
        );
        if replaced.is_empty() {
            return;
        }
        tracing::warn!(
            project_id,
            landed,
            upload_base,
            replaced = ?replaced.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            purpose = "report an upload that replaced another session's revision (R3)",
            "speleodb push: revisions landed between the HEAD check and the upload; \
             ours is on top of them and they are no longer HEAD (still in the history)"
        );
        self.speleodb_own_uploads_edit(generation, project_id, |record| {
            record
                .unreported
                .extend(replaced.into_iter().map(SpeleoRevision::from));
        });
    }
}
