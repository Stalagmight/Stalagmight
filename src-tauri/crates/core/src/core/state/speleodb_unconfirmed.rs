//! [#576 SDB-POLL-D1] An acquire whose reply was lost and whose re-read failed too.
//!
//! Start Editing and Publish follow a lost acquire reply (a transport error, an
//! unreadable body, a 5xx) by re-reading the holder at once (FZ-SDB-6-D1). When that
//! re-read fails as well - offline, the server down - the acquire may still have
//! landed: the server then holds the lock for this account and nothing here records
//! it. The next status poll reopens the gate over it as an INHERITED lock, a later
//! Start Editing reads it as inherited, and the quit's silent release, which hands
//! back only a lock this session took (#576 RV4-R-S2), leaves it held.
//!
//! So the acquire is marked UNCONFIRMED (`SpeleoDbSession::unconfirmed_acquire`,
//! stamped with the account that sent it), and the first successful status poll
//! settles it:
//!
//! - our account holds that project's lock: the claim is recorded, so a quit hands
//!   the lock back. With the gate shut, the poll's inherited-lock reopen records it
//!   as it opens the gate, after its own re-read under the lock-ops span; with the
//!   gate already open (a second Start Editing took the lock as inherited), it is
//!   recorded after a re-read under the same span;
//! - another account holds it, or nobody does: the acquire did not land, or the lock
//!   has moved on, and the mark is dropped;
//! - the holder could not be read: the mark stays for the next poll.
//!
//! A close drops the mark (`close_open_project_locked`). A mark is only ever set
//! where an answered acquire would have recorded a claim, so settling it records
//! exactly the claim the lost reply cost.

use super::speleodb::lock_held_by_me;
use super::speleodb_session::{LockClaim, LockOpsHeld};
use super::AppState;

impl AppState {
    /// Mark the acquire just sent for `project_id` unconfirmed: its reply was lost
    /// and the holder re-read failed. Stamped with the signed-in account in the same
    /// session acquisition that stores it; with no account there is no mark, as
    /// there would be no claim.
    pub(super) fn speleodb_mark_acquire_unconfirmed(&self, project_id: &str) {
        let mut session = self.speleodb.lock();
        let (endpoint, email) = (session.endpoint.clone(), session.email.clone());
        session.unconfirmed_acquire = endpoint
            .zip(email)
            .map(|(endpoint, email)| LockClaim::taken(project_id, endpoint, email));
        tracing::info!(
            project_id,
            marked = session.unconfirmed_acquire.is_some(),
            purpose = "settle an acquire whose reply was lost (#576 SDB-POLL-D1)",
            "speleodb: the acquire reply was lost and the holder re-read failed; the \
             next status poll settles whether the lock is ours"
        );
    }

    /// Drop the mark: the project closed.
    pub(super) fn speleodb_drop_unconfirmed_acquire(&self) {
        self.speleodb.lock().unconfirmed_acquire = None;
    }

    /// Whether a mark names `project_id`. A mark that names it under ANOTHER account
    /// (the session was replaced since) can never be confirmed here and is dropped.
    fn speleodb_unconfirmed_acquire_names(&self, project_id: &str) -> bool {
        let mut session = self.speleodb.lock();
        let Some(mark) = session.unconfirmed_acquire.as_ref() else {
            return false;
        };
        if mark.project_id != project_id {
            return false;
        }
        if mark.taken_by(session.endpoint.as_deref(), session.email.as_deref()) {
            return true;
        }
        session.unconfirmed_acquire = None;
        false
    }

    /// The poll found the lock held by our account, re-read inside the caller's
    /// lock-ops span: record the claim the lost reply cost, and drop the mark. Called
    /// with the gate OPEN (a claim over a shut gate is forgotten by the next poll).
    pub(super) fn speleodb_claim_unconfirmed_acquire(
        &self,
        _lock_ops: &LockOpsHeld<'_>,
        project_id: &str,
    ) {
        if !self.speleodb_unconfirmed_acquire_names(project_id) {
            return;
        }
        self.speleodb.lock().unconfirmed_acquire = None;
        self.speleodb_note_lock_acquired(Some(project_id));
        tracing::info!(
            project_id,
            purpose = "settle an acquire whose reply was lost (#576 SDB-POLL-D1)",
            "speleodb: the status poll found our account holding the lock; the acquire \
             landed, and the claim is recorded"
        );
    }

    /// The status poll's settle, after a SUCCESSFUL holder read for `project_id`
    /// (`holds_lock`: our account holds it) and after its gate rule ran. A shut gate
    /// that the reopen did not open keeps the mark for the next poll.
    pub(super) fn speleodb_settle_unconfirmed_acquire(&self, project_id: &str, holds_lock: bool) {
        if !self.speleodb_unconfirmed_acquire_names(project_id) {
            return;
        }
        if !holds_lock {
            self.speleodb.lock().unconfirmed_acquire = None;
            tracing::info!(
                project_id,
                purpose = "settle an acquire whose reply was lost (#576 SDB-POLL-D1)",
                "speleodb: the status poll found the lock free or another account's; \
                 the unconfirmed acquire is dropped"
            );
            return;
        }
        // Ours. The poll's read is one of several, so the holder is read again inside
        // the lock-ops span before a claim is recorded (as the reopen does).
        let lock_ops = self.speleodb_lock_ops_guard();
        if !self.edit_enabled() {
            return;
        }
        let Ok((endpoint, token, me)) = self.speleodb_active_account() else {
            return;
        };
        let insecure_http = self.speleodb_insecure_http();
        match self.speleodb_lock_holder_checked(&endpoint, &token, insecure_http, project_id) {
            Ok(holder) if lock_held_by_me(holder.as_deref(), Some(me.as_str())) => {
                self.speleodb_claim_unconfirmed_acquire(&lock_ops, project_id);
            }
            Ok(_) => self.speleodb.lock().unconfirmed_acquire = None,
            // Cannot say: the next poll settles it.
            Err(_) => {}
        }
    }
}
