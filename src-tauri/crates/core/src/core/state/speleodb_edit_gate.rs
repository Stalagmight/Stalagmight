//! The edit gate (#191), DERIVED from the write-lock claim (data#3).
//!
//! There is no edit-gate flag. Whether the open project may be edited is a
//! function of facts that already have to be right for the lock to be handled
//! right, read together under the SpeleoDB session lock:
//!
//! 1. What is open ([`OpenProjectGate`]). Nothing, or a project with no SpeleoDB
//!    link, is always editable: editing it never waits on a lock.
//! 2. For a SpeleoDB-managed project, a claim
//!    ([`super::speleodb_session::LockClaim`]) on THAT project, of the account
//!    signed in NOW, BOUND to the current session generation.
//!
//! So "editing a managed project without holding its lock" is not a state this
//! code can represent. It used to be: the gate was a free boolean, set by a dozen
//! sites, and Take remote reopened the project editable after releasing its lock
//! because the reload copied the old boolean across.
//!
//! A binding names a generation, and the generation names one open project: the
//! bind checks the open link's project AND deployment against the signed-in
//! account, and every open, close or reload moves the generation on. That is why
//! the gate itself does not have to re-read the link (a database read on every
//! edit) to know the claim is about the project in front of it.
//!
//! One way in, and the ways out are the ones that remove the claim:
//!
//! - OPEN: [`AppState::speleodb_bind_edit_claim`] - Start Editing's commit (and
//!   Switch to Read Only's rollback of its own unbind after a failed release).
//!   An in-place reload that keeps the session (a fast-forward, Reload, Restore)
//!   carries an EXISTING binding into the generation it bumps to, in the same step
//!   ([`AppState::speleodb_commit_open_gate`]); it cannot conjure one.
//! - SHUT: [`AppState::speleodb_shut_edit_gate`] (unbind, keep the claim as the
//!   pointer a later release needs), or removing the claim
//!   (`speleodb_forget_lock_acquired_for` and every release built on it). Every
//!   session boundary - close, open, create, logout, login, a 401 drop - moves the
//!   generation on, so a binding never outlives the session it was made in and
//!   none of them has to remember to shut anything.
//!
//! The frontend mirror (`speleoStore` `editState`) is seeded from
//! `SpeleoLinkStatus::edit_enabled`, i.e. from this function, and only otherwise
//! moves to `editing` on a Start Editing result that bound a claim.

use super::speleodb::SpeleoDbSession;
use super::speleodb_session::LockClaim;
use super::AppState;
use crate::models::SpeleoLink;
use crate::speleodb::auth::normalize_endpoint;
use crate::speleodb::SpeleoDbError;

/// What the open project is, as far as the edit gate is concerned. Recorded by
/// every site that commits a project into the slot (open, create, close) or
/// changes its link in place (publish, unlink).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum OpenProjectGate {
    /// Nothing open, or a project without a SpeleoDB link: always editable.
    #[default]
    Unmanaged,
    /// A SpeleoDB-managed project: editable only through a bound claim.
    Managed {
        /// The SpeleoDB project the open copy is linked to.
        project_id: String,
    },
}

impl OpenProjectGate {
    /// The gate for a project whose persisted link is `link`.
    pub(super) fn for_link(link: Option<&SpeleoLink>) -> Self {
        link.map_or(Self::Unmanaged, |link| Self::Managed {
            project_id: link.project_id.clone(),
        })
    }

    /// Whether `claim` is about the open project at all.
    fn names(&self, claim: &LockClaim) -> bool {
        matches!(self, Self::Managed { project_id } if *project_id == claim.project_id)
    }
}

impl SpeleoDbSession {
    /// THE gate predicate (see the module doc). Pure, so the session lock and the
    /// generation it is compared with are read by the caller in one acquisition.
    fn edit_gate_open(&self, open: &OpenProjectGate, generation: u64) -> bool {
        match open {
            OpenProjectGate::Unmanaged => true,
            OpenProjectGate::Managed { .. } => self.locks_acquired_here.iter().any(|claim| {
                claim.editing() == Some(generation)
                    && open.names(claim)
                    && claim.taken_by(self.endpoint.as_deref(), self.email.as_deref())
            }),
        }
    }

    /// Unbind every claim: no edit session holds the gate any more.
    fn unbind_every_claim(&mut self) {
        for claim in &mut self.locks_acquired_here {
            claim.bind_editing(None);
        }
    }
}

impl AppState {
    /// Whether the open project may be edited (#191): the mutation chokepoint
    /// (`guard_mutation`) and every SpeleoDB verb that needs an edit session read
    /// this. Derived, never stored - see the module doc.
    pub(super) fn edit_enabled(&self) -> bool {
        let open = self.speleodb_open_gate();
        if open == OpenProjectGate::Unmanaged {
            return true;
        }
        let session = self
            .speleodb
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The generation is read UNDER the session lock: an in-place reload bumps
        // it and carries the binding in one acquisition of that same lock, so a
        // reader sees either the old generation with the old binding or the new
        // with the new - never a reload's open gate flickering shut.
        session.edit_gate_open(&open, self.speleodb_session_generation())
    }

    /// What is open now, as the gate sees it.
    pub(super) fn speleodb_open_gate(&self) -> OpenProjectGate {
        self.speleodb_open_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Record the project just committed into the slot, with `session` - the
    /// SpeleoDB session lock - held across the generation bump that committed it.
    ///
    /// `carry_from` is an in-place reload that keeps its edit session (a
    /// fast-forward, Reload, Restore): the claim bound to THAT generation, for the
    /// same project and account, is carried to the new one. Nothing else stays
    /// bound. A reload can therefore keep a session that exists, and can never
    /// start one - which is the whole of data#3: Take remote's reload carried
    /// "editing" across a lock it had just released.
    pub(super) fn speleodb_commit_open_gate(
        &self,
        session: &mut SpeleoDbSession,
        link: Option<&SpeleoLink>,
        carry_from: Option<u64>,
    ) {
        let open = OpenProjectGate::for_link(link);
        let now = self.speleodb_session_generation();
        let (endpoint, email) = (session.endpoint.clone(), session.email.clone());
        for claim in &mut session.locks_acquired_here {
            let carried = carry_from.is_some()
                && claim.editing() == carry_from
                && open.names(claim)
                && claim.taken_by(endpoint.as_deref(), email.as_deref());
            claim.bind_editing(carried.then_some(now));
        }
        *self
            .speleodb_open_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = open;
    }

    /// [`Self::speleodb_commit_open_gate`] for a change that carries no session: a
    /// create or close (whose generation bump already ended it), or a link
    /// appearing or vanishing in place (publish, unlink).
    pub(super) fn speleodb_set_open_gate(&self, link: Option<&SpeleoLink>) {
        let mut session = self
            .speleodb
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.speleodb_commit_open_gate(&mut session, link, None);
    }

    /// OPEN the edit gate: bind the signed-in account's claim on `project_id` to the
    /// current session, minting it if there is none. The one way in (module doc).
    ///
    /// The caller holds the `project` guard and has checked that `project_id` is
    /// the open project in the generation it acted on, linked on
    /// `link_endpoint`; this re-checks the parts it owns. `authored` is Start
    /// Editing's probe verdict (see [`super::speleodb_session::LockClaim`]);
    /// `by_this_flow` also records the claim as the flow's unwind target, in the
    /// same session acquisition.
    ///
    /// A fresh binding REPLACES the account's claim on the project: taking (or
    /// resuming) the lock is a new decision, so `kept` goes with it. Authorship
    /// does not: resuming a lock this process took earlier (an inherited bind over
    /// our own authored claim) is still a lock this process took. Refuses with
    /// `SessionMoved` when no account is signed in, or the signed-in deployment is
    /// not the project's - a claim nobody can release is not one an edit session
    /// may hang on.
    pub(super) fn speleodb_bind_edit_claim(
        &self,
        project_id: &str,
        link_endpoint: &str,
        authored: bool,
        by_this_flow: bool,
    ) -> Result<(), SpeleoDbError> {
        let open = self.speleodb_open_gate();
        if open
            != (OpenProjectGate::Managed {
                project_id: project_id.to_owned(),
            })
        {
            return Err(SpeleoDbError::SessionMoved);
        }
        let mut session = self.speleodb_session_lock()?;
        let (Some(endpoint), Some(email)) = (session.endpoint.clone(), session.email.clone())
        else {
            return Err(SpeleoDbError::SessionMoved);
        };
        if normalize_endpoint(&endpoint) != normalize_endpoint(link_endpoint) {
            return Err(SpeleoDbError::SessionMoved);
        }
        let generation = self.speleodb_session_generation();
        let mut was_authored = false;
        session.locks_acquired_here.retain(|claim| {
            let ours =
                claim.project_id == project_id && claim.taken_by(Some(&endpoint), Some(&email));
            was_authored |= ours && claim.is_authored();
            !ours
        });
        session.unbind_every_claim();
        let mut claim = LockClaim::taken(project_id, endpoint, email, authored || was_authored);
        claim.bind_editing(Some(generation));
        if by_this_flow {
            session.acquired_by_this_flow = Some(claim.clone());
        }
        session.locks_acquired_here.push(claim);
        drop(session);
        // Opening the gate also drops any pending #202 lock re-verify: it was
        // started under an edit session that has ended, and whatever it finds
        // describes that session, not this one (a re-acquire can reopen the gate
        // within the same generation, so the generation alone cannot tell).
        *self
            .speleodb_reverify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }

    /// SHUT the edit gate, keeping every claim as the pointer a later
    /// holder-checked release needs: lost or unverifiable ownership, a release
    /// that ends the session, Take remote.
    pub(super) fn speleodb_shut_edit_gate(&self) {
        self.speleodb
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unbind_every_claim();
    }

    /// Whether the signed-in account holds ANY claim on `project_id` - bound to
    /// the open session or not, taken or inherited: a lock this process can
    /// account for, and so one a lifecycle release may (holder-checked) hand back.
    pub(super) fn speleodb_holds_claim(&self, project_id: &str) -> bool {
        self.speleodb_session_lock().is_ok_and(|session| {
            session.locks_acquired_here.iter().any(|claim| {
                claim.project_id == project_id
                    && claim.taken_by(session.endpoint.as_deref(), session.email.as_deref())
            })
        })
    }

    /// Leave an UNBOUND claim on `project_id` for the signed-in account, unless it
    /// already has one: the provisional pointer an ambiguous acquire leaves when
    /// it cannot find out whether the lock is now ours (sdk#4). It opens nothing;
    /// the next close (or the janitor) asks the server and hands the lock back only
    /// if it turns out to be ours.
    pub(super) fn speleodb_record_unbound_claim(&self, project_id: &str) {
        let Ok(mut session) = self.speleodb_session_lock() else {
            return;
        };
        let (Some(endpoint), Some(email)) = (session.endpoint.clone(), session.email.clone())
        else {
            return;
        };
        if session.locks_acquired_here.iter().any(|claim| {
            claim.project_id == project_id && claim.taken_by(Some(&endpoint), Some(&email))
        }) {
            return;
        }
        session
            .locks_acquired_here
            .push(LockClaim::taken(project_id, endpoint, email, false));
    }

    /// [`Self::speleodb_record_unbound_claim`], marked KEPT: the pointer to a lock
    /// held back on purpose because un-uploaded work stands behind it, which no
    /// automatic release may hand back (the stranded-claim cleanup and the
    /// lifecycle releases skip kept claims). Start Editing's unwind leaves one
    /// when the cache copy refuses to be overwritten over its own un-uploaded
    /// edits - the lock this call just took is then the one guarding that work,
    /// and without a claim it was held with nothing local pointing at it. Only an
    /// explicit Release, an abandon, or a new edit session's binding ends it.
    pub(super) fn speleodb_record_kept_claim(&self, project_id: &str) {
        self.speleodb_record_unbound_claim(project_id);
        self.speleodb_keep_lock_deliberately(project_id);
    }

    /// Whether the signed-in account's claim on `project_id` is deliberately kept
    /// (see `LockClaim::kept`).
    pub(super) fn speleodb_claim_is_kept(&self, project_id: &str) -> bool {
        self.speleodb_session_lock().is_ok_and(|session| {
            session.locks_acquired_here.iter().any(|claim| {
                claim.project_id == project_id
                    && claim.is_kept()
                    && claim.taken_by(session.endpoint.as_deref(), session.email.as_deref())
            })
        })
    }
}

/// The account a fixture edit session is held by when the test signed nobody in.
#[cfg(test)]
pub(super) const FIXTURE_EDITOR: &str = "editor@fixture.invalid";

#[cfg(test)]
impl AppState {
    /// FIXTURE: record - or, with `None`, forget - an AUTHORED claim of the
    /// signed-in account (the acquisition provenance read by
    /// [`Self::speleodb_remote_state`]).
    ///
    /// Production records claims only through the edit gate
    /// (`speleodb_bind_edit_claim`, Start Editing's commit) and the provisional
    /// pointer of an ambiguous acquire (`speleodb_record_unbound_claim`); this is
    /// the test seam for the states those leave behind. `Some(id)` replaces the
    /// account's entry for `id` - keeping its edit binding, so it changes WHO the
    /// claim says took the lock, never whether the gate is open - and leaves its
    /// entries for other projects standing. `None` ends every claim of the
    /// account. Another account's claims are untouched in every direction.
    pub(super) fn speleodb_note_lock_acquired(&self, project_id: Option<&str>) {
        self.speleodb_record_claim(project_id, false);
    }

    /// FIXTURE: [`Self::speleodb_note_lock_acquired`] that also REMEMBERS the
    /// claim as the last Start Editing's own acquisition (the stale-context
    /// unwind's target), as `speleodb_bind_edit_claim` does for an authored bind.
    pub(super) fn speleodb_note_acquisition_by_this_flow(&self, project_id: &str) {
        self.speleodb_record_claim(Some(project_id), true);
    }

    /// The body of both fixtures, so the two cannot record differently.
    fn speleodb_record_claim(&self, project_id: Option<&str>, by_this_flow: bool) {
        let Ok(mut session) = self.speleodb_session_lock() else {
            return;
        };
        let (endpoint, email) = (session.endpoint.clone(), session.email.clone());
        let mut binding = None;
        session.locks_acquired_here.retain(|claim| {
            if !claim.taken_by(endpoint.as_deref(), email.as_deref()) {
                return true;
            }
            // Ours: replaced when this call names its project, ended when the
            // call says the account holds nothing.
            let keep = project_id.is_some_and(|id| claim.project_id != id);
            if !keep && project_id.is_some() {
                binding = claim.editing();
            }
            keep
        });
        // No account to stamp it with means no claim: one that cannot say whose it
        // is can never be released and must never license the authorship shortcut.
        if let Some(((project_id, endpoint), email)) = project_id.zip(endpoint).zip(email) {
            let mut claim = LockClaim::taken(project_id, endpoint, email, true);
            claim.bind_editing(binding);
            if by_this_flow {
                session.acquired_by_this_flow = Some(claim.clone());
            }
            session.locks_acquired_here.push(claim);
        }
    }

    /// FIXTURE: put the open managed project into (`true`) or out of (`false`) an
    /// edit session, through the claim - there is no production setter.
    ///
    /// `true` binds the signed-in account's claim on the open project (minting an
    /// inherited one if there is none), signing [`FIXTURE_EDITOR`] in - on the
    /// project's deployment and without a token, so nothing reaches a network -
    /// when nobody is. A local project is always editable, so `true` is a no-op
    /// there and `false` cannot shut it.
    pub(crate) fn set_edit_enabled(&self, editing: bool) {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !editing {
            self.speleodb_shut_edit_gate();
            return;
        }
        let OpenProjectGate::Managed { project_id } = self.speleodb_open_gate() else {
            return;
        };
        let link_endpoint = self.speleodb_open_link().ok().map(|link| link.endpoint);
        let mut session = self
            .speleodb
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if session.email.is_none() {
            session.endpoint = link_endpoint;
            session.email = Some(FIXTURE_EDITOR.to_owned());
            session.rehydrate_attempted = true;
        }
        let generation = self.speleodb_session_generation();
        let (account_endpoint, email) = (session.endpoint.clone(), session.email.clone());
        session.unbind_every_claim();
        if let Some(claim) = session.locks_acquired_here.iter_mut().find(|claim| {
            claim.project_id == project_id
                && claim.taken_by(account_endpoint.as_deref(), email.as_deref())
        }) {
            claim.bind_editing(Some(generation));
        } else if let (Some(account_endpoint), Some(email)) = (account_endpoint, email) {
            let mut claim = LockClaim::taken(&project_id, account_endpoint, email, false);
            claim.bind_editing(Some(generation));
            session.locks_acquired_here.push(claim);
        }
        drop(session);
        *self
            .speleodb_reverify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}
