use std::path::PathBuf;

use super::speleodb_cache::LocalEditsPolicy;
// [#275] The push half (fence, snapshot, upload, Done/Push/Update) lives in
// `speleodb_push`; the lock + edit gate here still consults its outcome type and
// its empty-tip-tolerant head read.
use super::speleodb_push::head_with_empty_retry;
use super::AppState;
use crate::speleodb::auth::{normalize_endpoint, TokenStore};
use crate::speleodb::client;
// [Track A] #186/D-B2: typed Endpoint + map_status + SpeleoDbError→SpeleoErrorDto
// conversion. [Integration] map_status/SpeleoDbAction also drive Track C's C5
// 401 auto-drop (auth_error_drops_token).
use crate::speleodb::contract::{dto_for_error, map_status, Endpoint};
use crate::speleodb::sync::{self, ReconcileVerdict};
use crate::speleodb::SpeleoDbAction;
use crate::speleodb::{
    RemoteState, SpeleoDbError, SpeleoErrorDto, SpeleoLinkStatus, SpeleoSessionStatus,
    StartEditingResult,
};

/// Per-session SpeleoDB connection state held in [`AppState`]. The token lives
/// inside the backend token cache and is never
/// exposed to the webview.
#[derive(Debug, Default)]
pub(crate) struct SpeleoDbSession {
    pub(super) endpoint: Option<String>,
    pub(super) email: Option<String>,
    pub(super) token_store: TokenStore,
    /// [Track C] (#188, C4): set once per process after the lazy silent startup
    /// rehydration runs (whether it found a token or not), so an empty/failed
    /// the saved token is restored at most once.
    pub(super) rehydrate_attempted: bool,
    /// Every write lock THIS process holds a claim on and has not handed back: the
    /// pointer every release follows, the acquisition provenance the authorship
    /// shortcut in [`AppState::speleodb_remote_state`] hangs on (an AUTHORED claim),
    /// and - bound to the session - the edit gate itself (`speleodb_edit_gate`).
    /// Deliberately process-local and never persisted - a lock left over from an
    /// earlier run, or one held by a sibling session on the same account, is no
    /// evidence that THIS process pushed anything.
    ///
    /// A LIST, not a slot, and the difference is a stranded lock. A claim can
    /// outlive the session that made it (logout declines to release over un-pushed
    /// edits; a teardown leaves one behind for the janitor) and can only be
    /// released by the account that took it, so it has to survive until that
    /// account signs in again. With one slot, the next acquisition - possibly by a
    /// different user, on a different project - overwrote the only pointer to that
    /// still-held mutex, and nothing could ever hand it back. Claims are therefore
    /// keyed by (account, project): each entry knows whose it is
    /// ([`super::speleodb_session::LockClaim`]), and a write for one account never
    /// disturbs another's.
    ///
    /// Read-modify-written only through named sites, never by a field assignment
    /// anywhere else, because each such pair has to happen under ONE session-lock
    /// acquisition - two acquisitions leave a gap a fresh acquisition can land in and
    /// be erased from (each site's doc says which gap it is closing): the edit gate
    /// (`speleodb_edit_gate`: bind, carry, shut, the provisional claim of an
    /// ambiguous acquire), [`AppState::speleodb_forget_lock_acquired_for`] (clear
    /// the one that names a given project), the restore of a claim whose release
    /// failed, and the stranded-claim janitor
    /// (`AppState::speleodb_janitor_stranded_claim`, which additionally holds the
    /// `project` mutex over its decision and the lock-ops mutex over its
    /// restore-on-failure, so its writes cannot cross a live session's).
    ///
    /// Every one of those writes is scoped to the LIVE account. Dropping another
    /// account's claim is the one irreversible mistake available here: a claim kept
    /// too long costs at most a holder read before a release, while a claim dropped
    /// early strands a server mutex with nothing local pointing at it - and
    /// SpeleoDB never expires a lock on its own.
    ///
    /// `pub(super)` rather than private because those sites live in several
    /// `core::state` modules, and a sibling module cannot see a private field.
    /// Nothing else in `core::state` may touch it.
    pub(super) locks_acquired_here: Vec<super::speleodb_session::LockClaim>,
    /// The claim the most recent Start Editing MINTED, kept apart from the set
    /// above so the stale-context unwind can hand back the lock that operation
    /// took rather than an arbitrary one of this account's.
    ///
    /// Written only by `AppState::speleodb_note_acquisition_by_this_flow`, in
    /// the same session acquisition that records the claim itself, and TAKEN by
    /// `AppState::speleodb_take_acquisition_by_this_flow` - so it answers once,
    /// for the flow that set it. An acquire that inherited a lock the account
    /// already held mints nothing and leaves this alone, which is what makes the
    /// unwind's promised no-op on an inherited lock real.
    pub(super) acquired_by_this_flow: Option<super::speleodb_session::LockClaim>,
}

/// Pure endpoint-precedence resolver (extracted so it is unit-testable without
/// process env or an `AppState`): the first tier that is present and non-blank,
/// in order override → env → config, trimmed. Returns `None` if all are blank.
fn resolve_endpoint(
    override_endpoint: Option<&str>,
    env_endpoint: Option<&str>,
    config_endpoint: Option<&str>,
) -> Option<String> {
    // Same cleaning as a settings save (#675), so the override and
    // `DEV_SPELEODB_URL` accept a pasted value too; a value that cleans to
    // nothing falls through to the next source.
    [override_endpoint, env_endpoint, config_endpoint]
        .into_iter()
        .flatten()
        .find_map(crate::speleodb::clean_endpoint_input)
}

/// Reject any `project_id` that is not a plain SpeleoDB project UUID before it is
/// used as a filesystem path component or interpolated into a request URL. The
/// server keys projects by UUID, so anything else is invalid; refusing it closes
/// the path-traversal / route-manipulation vector at the IPC trust boundary (a
/// `..`, an absolute path, or an embedded separator could otherwise redirect the
/// cached `.stalx` write or the download URL). A parsed UUID has no separators,
/// `..`, or absolute prefix, so it is always a safe single path component.
///
/// Only the canonical spelling passes: lowercase, hyphenated, the form SpeleoDB
/// sends (and the SDK prints). `Uuid::parse_str` also takes `{braced}`,
/// `urn:uuid:`, hyphenless and uppercase forms, each naming the same project as
/// another cache directory, request path or link row than the canonical one: two
/// spellings of one project would be two cache entries, and an id from IPC could
/// name a directory the listing never showed.
pub(super) fn validate_project_id(project_id: &str) -> Result<(), SpeleoDbError> {
    match uuid::Uuid::parse_str(project_id) {
        Ok(id) if id.hyphenated().to_string() == project_id => Ok(()),
        _ => Err(SpeleoDbError::InvalidProjectId),
    }
}

/// #202: how long a lock stays "verified" before a lock-held edit re-verifies it.
/// A continuously-active editor therefore issues at most ~3 re-verify requests/hr;
/// an idle/AFK session issues zero (the check is edit-triggered, never a timer).
const REVERIFY_INTERVAL_SECS: i64 = 20 * 60;

/// #202 throttle: is a lock re-verify due? Due when we have never verified since
/// pull (`None`) OR more than [`REVERIFY_INTERVAL_SECS`] has elapsed since
/// `last_verify_at`. An unparseable timestamp fails toward re-verifying (safer to
/// issue one extra GET than to skip a stale-lock check). Pure for unit tests.
fn reverify_due(last_verify_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(stamp) = last_verify_at else {
        return true;
    };
    match chrono::DateTime::parse_from_rfc3339(stamp) {
        Ok(t) => {
            now.signed_duration_since(t.with_timezone(&chrono::Utc))
                .num_seconds()
                >= REVERIFY_INTERVAL_SECS
        }
        Err(_) => true,
    }
}

/// #202: does `holder` (the project's `active_mutex.user`, if any) match our
/// authenticated email? Case-insensitive, mirroring the ancestry-recovery email
/// comparison in `sync.rs` (SpeleoDB may normalise case). A missing holder (lock
/// cleared) or a missing self-email never matches - i.e. the lock is not ours.
pub(super) fn lock_held_by_me(holder: Option<&str>, me: Option<&str>) -> bool {
    matches!((holder, me), (Some(h), Some(m)) if h.eq_ignore_ascii_case(m))
}

/// Is this failure the local-edits refusal - i.e. does it say there is un-pushed
/// work behind the lock an unwind is about to hand back?
///
/// [`AppState::guard_local_edits`] is the sole producer, and it raises this for BOTH
/// of its refusals: a copy it read and found dirty, and a copy present-but-unreadable
/// (where a read-write open might still recover a hot journal, so it fails closed).
/// Both mean the same thing to a lock decision - work that exists only here may be
/// behind that mutex - so both keep it. See `speleodb_start_editing`'s unwind, whose
/// pre-acquire flag cannot see a copy other than the open project's.
pub(super) fn is_local_edits_refusal(err: &SpeleoDbError) -> bool {
    matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. })
}

/// Could an acquire that failed this way still have LANDED (sdk#4)? A transport
/// failure, a 5xx or an unreadable reply can all arrive after the server committed
/// the mutex to us - a proxy giving up on a slow reply, a reset after the write, a
/// body the SDK's strict schema refuses. A 4xx is the server's own verdict about
/// the request and is never ambiguous.
fn acquire_outcome_unknown(err: &SpeleoDbError) -> bool {
    matches!(
        err,
        SpeleoDbError::Transport { .. }
            | SpeleoDbError::Parse(_)
            | SpeleoDbError::Status {
                status: 500..=599,
                ..
            }
    )
}

/// [Track A] #186/D-B2: map a non-endpoint (local / session) `SpeleoDbError` to
/// the IPC `SpeleoErrorDto`. Local errors always fail closed to `Fatal`
/// regardless of endpoint, so the endpoint passed to [`dto_for_error`] is
/// irrelevant for them (`User` is an arbitrary, non-load-bearing choice).
pub(super) fn local_dto(err: SpeleoDbError) -> SpeleoErrorDto {
    dto_for_error(Endpoint::User, &err)
}

/// [Track C] (#188, C5): the pure 401 auto-drop predicate. A genuine 401 on an
/// established-session call drops the token; **everything else is kept** - 403 is
/// never a drop (D-B4, robust to a DRF 401→403 downgrade), and 2xx/5xx/transport/
/// local errors pass through. Extracted so the drop decision is unit-testable
/// without a full `AppState`.
///
/// [Integration] The drop decision now consumes Track A's #186 contract: the
/// token is dropped **iff** `map_status(endpoint, status) == SpeleoDbAction::ReLogin`
/// (the single authoritative status→action mapping). 401 on an authenticated
/// endpoint maps to `ReLogin`; 403/2xx/5xx/unmapped never do, so this is
/// behaviorally identical to the prior bare-401 guard while removing the
/// duplicate status logic.
fn auth_error_drops_token<T>(endpoint: Endpoint, result: &Result<T, SpeleoDbError>) -> bool {
    matches!(
        result,
        Err(SpeleoDbError::Status { status, .. })
            if map_status(endpoint, *status) == SpeleoDbAction::ReLogin
    )
}

/// Decide, from a whoami (`GET /user/`) probe result, whether the session token
/// is dead. The live box returns 200 for any valid token and 403 for a dead one
/// (the DRF 401→403 downgrade), so a probe whose status maps to
/// [`SpeleoDbAction::ReLogin`] (a 403 or 401 on the `User` endpoint) means the
/// token is dead. A success means it is alive - so a resource 403 was a genuine
/// permission denial, keep the token (D-B4). A transport error is inconclusive
/// and is treated as not-dead (never drop on a flaky probe). Pure + unit-tested;
/// the network call lives in [`AppState::speleodb_token_is_dead`].
fn whoami_indicates_dead_token(probe: &Result<String, SpeleoDbError>) -> bool {
    matches!(
        probe,
        Err(SpeleoDbError::Status { status, .. })
            if map_status(Endpoint::User, *status) == SpeleoDbAction::ReLogin
    )
}

impl SpeleoDbSession {
    /// Resolve the active `(endpoint, token)` for this session, or
    /// [`SpeleoDbError::NotAuthenticated`] if endpoint, email, or token is
    /// missing. Extracted from [`AppState::speleodb_active_token`] so the branch
    /// logic is testable without a full `AppState`.
    ///
    /// `&mut self` because [`TokenStore::load`] caches its result + updates the
    /// cached session credential.
    fn active_token(&mut self) -> Result<(String, String), SpeleoDbError> {
        let endpoint = self
            .endpoint
            .clone()
            .ok_or(SpeleoDbError::NotAuthenticated)?;
        let email = self.email.clone().ok_or(SpeleoDbError::NotAuthenticated)?;
        let token = self
            .token_store
            .load(&endpoint, &email)
            .ok_or(SpeleoDbError::NotAuthenticated)?;
        Ok((endpoint, token))
    }

    /// Local session status. The token never crosses IPC.
    fn status(&mut self) -> SpeleoSessionStatus {
        let logged_in = self.active_token().is_ok();
        SpeleoSessionStatus {
            user_email: logged_in.then(|| self.email.clone()).flatten(),
            endpoint: logged_in.then(|| self.endpoint.clone()).flatten(),
            logged_in,
        }
    }

    pub(super) fn forget(&mut self) {
        self.token_store.clear();
        self.endpoint = None;
        self.email = None;
        self.rehydrate_attempted = true;
    }

    pub(super) fn reload_credentials(&mut self) {
        self.forget();
        self.rehydrate_attempted = false;
    }

    /// Restore only a token saved for this server. Environment overrides must
    /// never send a production token to a different instance.
    fn rehydrate(&mut self, endpoint: Option<String>, config: &crate::config::SpeleoDbConfig) {
        if self.rehydrate_attempted || self.endpoint.is_some() {
            return;
        }
        self.rehydrate_attempted = true;
        let (Some(endpoint), Some(email), Some(token), Some(saved_endpoint)) = (
            endpoint,
            config.email.as_deref(),
            config.token.0.as_deref(),
            config.endpoint.as_deref(),
        ) else {
            return;
        };
        if normalize_endpoint(&endpoint) != normalize_endpoint(saved_endpoint)
            || email.trim().is_empty()
            || token.trim().is_empty()
        {
            return;
        }
        self.token_store.store(&endpoint, email, token);
        self.endpoint = Some(endpoint);
        self.email = Some(email.to_owned());
    }
}

/// The server `type` a project Stalagmight manages carries - the state adoption
/// exists to leave behind. Compared case-insensitively for the same reason
/// the Ariane project type is.
///
/// SpeleoDB generates a project's map only for types it has an exporter for, and
/// `stalagmight` is the one it maps from the `.stalx` (via `stalx-reader`, #593).
pub(super) const MANAGED_PROJECT_TYPE: &str = "STALAGMIGHT";

/// The managed type before SpeleoDB had a Stalagmight type. SpeleoDB never maps
/// an `other` project, so it is NOT treated as managed: the next landed push
/// (`type_needs_managed_label`) and an adoption's finalize both relabel it.
const LEGACY_MANAGED_PROJECT_TYPE: &str = "other";

/// The PATCH body that labels a project as Stalagmight-managed.
pub(super) fn managed_type_patch() -> String {
    serde_json::json!({ "type": MANAGED_PROJECT_TYPE }).to_string()
}

/// Should a push that just landed rewrite the server type to
/// [`MANAGED_PROJECT_TYPE`]? Yes for an adoption that never finished (`ariane`)
/// and for the legacy managed type (`other`); no for anything else, including an
/// unreadable type (the next push reads it again).
pub(super) fn type_needs_managed_label(server_kind: Option<&str>) -> bool {
    super::speleodb_push::type_still_says_ariane(server_kind)
        || server_kind.is_some_and(|kind| {
            kind.trim()
                .eq_ignore_ascii_case(LEGACY_MANAGED_PROJECT_TYPE)
        })
}
impl AppState {
    /// Resolve the effective endpoint. Precedence: explicit dialog override →
    /// `DEV_SPELEODB_URL` env (dev default) → persisted `config.speleodb.endpoint`.
    pub(super) fn speleodb_resolve_endpoint(
        &self,
        override_endpoint: Option<&str>,
    ) -> Option<String> {
        let env = std::env::var("DEV_SPELEODB_URL").ok();
        let config_endpoint = self
            .config
            .lock()
            .ok()
            .and_then(|c| c.speleodb.endpoint.clone());
        resolve_endpoint(
            override_endpoint,
            env.as_deref(),
            config_endpoint.as_deref(),
        )
    }

    /// [Track A] #187/D-B10: read the persisted `insecure_http` override under the
    /// config lock. It is threaded into every client call ->
    /// `tls::validate_endpoint`, where it relaxes ONLY the plain-`http://` rule
    /// (the automatic exceptions apply without it) - in a development build only:
    /// the policy ignores it in a release build, so a stored `true` from an old
    /// config opens nothing there. The one place every call reads the stored
    /// value. Defaults `false` (the secure default) when the config lock is
    /// unavailable.
    pub(super) fn speleodb_insecure_http(&self) -> bool {
        self.config
            .lock()
            .ok()
            .is_some_and(|c| c.speleodb.insecure_http)
    }

    pub(super) fn speleodb_session_lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, SpeleoDbSession>, SpeleoDbError> {
        self.speleodb
            .lock()
            .map_err(|_| SpeleoDbError::Io("SpeleoDB session lock poisoned".to_owned()))
    }

    /// Restore a locally saved token once, without network access.
    pub(super) fn speleodb_ensure_rehydrated(&self) {
        let resolved_endpoint = self.speleodb_resolve_endpoint(None);
        let config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(mut session) = self.speleodb_session_lock() {
            session.rehydrate(resolved_endpoint, &config.speleodb);
        }
    }

    /// Resolve the active `(endpoint, token)` for an authenticated call, or
    /// [`SpeleoDbError::NotAuthenticated`] if there is no usable token.
    pub(super) fn speleodb_active_token(&self) -> Result<(String, String), SpeleoDbError> {
        self.speleodb_ensure_rehydrated();
        self.speleodb_session_lock()?.active_token()
    }

    /// [`Self::speleodb_active_token`] plus the EMAIL, resolved under ONE
    /// session acquisition - `(endpoint, token, email)` that provably describe
    /// the same account.
    ///
    /// For every caller that has to decide "is this lock MINE to release?", the
    /// two halves must come from one read. A login lands as a single write to
    /// this session, but it does not take the `project` or `speleodb_lock_ops`
    /// guards, so it can land BETWEEN two separate reads and hand a caller one
    /// account's endpoint and token with another account's email. The
    /// stranded-claim janitor is the one that pays for it: it would then find
    /// account B's claim with B's email, take it out of the retry list, and send
    /// the release with A's token - an idempotent success for a lock A never
    /// held, and B's only pointer to its still-held mutex gone.
    pub(super) fn speleodb_active_account(
        &self,
    ) -> Result<(String, String, String), SpeleoDbError> {
        self.speleodb_ensure_rehydrated();
        let mut session = self.speleodb_session_lock()?;
        let (endpoint, token) = session.active_token()?;
        // `active_token` already refuses without an email, so this cannot fail
        // after it succeeded - it is spelled out rather than unwrapped.
        let email = session
            .email
            .clone()
            .ok_or(SpeleoDbError::NotAuthenticated)?;
        Ok((endpoint, token, email))
    }

    /// Report whether a usable token is held (and for whom), so the dialog can
    /// restore the project list on reopen instead of forcing a re-login. The
    /// token itself never leaves the backend.
    pub fn speleodb_session_status(&self) -> Result<SpeleoSessionStatus, SpeleoErrorDto> {
        self.speleodb_ensure_rehydrated();
        // No network call - only the (local) session-lock error can surface here.
        Ok(self.speleodb_session_lock().map_err(local_dto)?.status())
    }

    /// Report the open project's SpeleoDB link/edit status for the action shell
    /// (#204): whether it is remote, whether editing is enabled (the write lock is
    /// held), the un-pushed-edits dirty bit, and - best-effort over one
    /// `get_project` - the user's permission tier and the current lock holder.
    ///
    /// A local project (or none open) reports `is_remote:false` with the network
    /// fields `None`. The network enrichment is deliberately fail-open: a logged
    /// out / offline / failed lookup leaves `permission`/`lock_holder` `None`
    /// rather than failing the whole status, because the local truth
    /// (`is_remote` + `edit_enabled`) is what the read-only gate hangs on. Not
    /// routed through [`Self::speleodb_handle_auth`] - a transient 401 on this
    /// passive probe must not auto-drop the session (mirrors
    /// [`Self::speleodb_lock_holder`]).
    pub fn speleodb_link_status(&self) -> Result<SpeleoLinkStatus, SpeleoErrorDto> {
        let (status_generation, link_result) = {
            let _project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                self.speleodb_session_generation(),
                self.speleodb_open_link(),
            )
        };
        let link = match link_result {
            Ok(link) => link,
            // No remote project open (no database, or a local project): not-remote.
            Err(SpeleoDbError::NoRemoteProject) => {
                // The stranded-claim janitor (F7 leftovers): with no project open,
                // a surviving acquisition record describes a dead session - clear
                // it and hand its lock back. This branch used to return before any
                // cleanup could run, which is what made a stranded record
                // permanent (footgun 5.4).
                self.speleodb_janitor_stranded_claim();
                return Ok(SpeleoLinkStatus {
                    is_remote: false,
                    edit_enabled: self.edit_enabled(),
                    has_unpushed_edits: false,
                    // Not remote: there is no chain to be mid-walk on.
                    unwound: false,
                    permission: None,
                    lock_holder: None,
                    remote_state: RemoteState::Unknown,
                    access_denied: false,
                    server_error: None,
                    // [#303] A local project has no undo POLICY (the fixed window is
                    // unconditional there), so report the global default and leave the
                    // control - which only renders for a remote project - unbothered.
                    undo_mode: self.undo_mode_status(),
                    project_id: None,
                });
            }
            Err(e) => return Err(local_dto(e)),
        };
        // No claim bookkeeping here any more. A shut gate used to mean "drop the
        // open project's claim" (a catch-all for the release paths that ran no
        // release verb), because a claim on its own licensed the authorship
        // shortcut. Now the gate IS a bound claim (`speleodb_edit_gate`) and the
        // shortcut reads it, so an unbound claim licenses nothing, and it stays as
        // the pointer a later holder-checked release needs.
        let has_unpushed_edits = link.has_unpushed_edits;

        // Best-effort network enrichment: the user's permission tier + lock holder
        // (get_project), and the remote-state reconcile verdict (revision topology,
        // the #202 freshness fold). Guard the request URL with the same project-id
        // validation as the edit verbs; on any miss (invalid id, no token,
        // transport/HTTP error) the fields degrade to None / Unknown - the local
        // truth still returns.
        let (permission, lock_holder, remote_state, access_denied, server_error) =
            if validate_project_id(&link.project_id).is_ok() {
                match self.speleodb_active_token() {
                    Ok((endpoint, token)) => {
                        let insecure_http = self.speleodb_insecure_http();
                        // Serialize the short holder probe with acquisition/release.
                        // Otherwise a response saying "unlocked" can close a new
                        // edit session acquired while this request was in flight.
                        let status = {
                            let _lock_ops = self.speleodb_lock_ops_guard();
                            if self.speleodb_session_generation() != status_generation {
                                return Err(local_dto(SpeleoDbError::SessionMoved));
                            }
                            if normalize_endpoint(&endpoint) != normalize_endpoint(&link.endpoint) {
                                self.speleodb_disable_editing_if_current(status_generation);
                                return Err(local_dto(SpeleoDbError::SessionMoved));
                            }
                            let result = client::get_project_probe(
                                &endpoint,
                                &token,
                                insecure_http,
                                &link.project_id,
                            );
                            let lost = match &result {
                                Ok(remote) => {
                                    !lock_held_by_me(
                                        remote
                                            .active_mutex
                                            .as_ref()
                                            .map(|mutex| mutex.user.as_str()),
                                        self.speleodb_my_email().as_deref(),
                                    ) || !remote.permission.is_some_and(|permission| {
                                        permission >= crate::speleodb::Permission::ReadAndWrite
                                    })
                                }
                                Err(SpeleoDbError::Status {
                                    status: 401 | 403, ..
                                }) => true,
                                Err(_) => false,
                            };
                            if lost {
                                self.speleodb_disable_editing_if_current(status_generation);
                            }
                            result
                        };
                        match status {
                            Ok(p) => {
                                // Only now (the project is reachable) is the revision-
                                // topology fold worth a SECOND round trip. On every
                                // error arm below the remote state is Unknown anyway
                                // (get_revisions would fail the same way get_project
                                // just did), so skip the doomed extra GET (N5).
                                let lock_holder = p.active_mutex.map(|m| m.user);
                                // The lock holder is what licenses the refresh's
                                // authorship recovery, so it has to be read from the
                                // SAME get_project response the verdict is folded
                                // from - not re-fetched or inferred.
                                let remote_state = self.speleodb_remote_state(
                                    &endpoint,
                                    &token,
                                    insecure_http,
                                    &link,
                                    lock_holder.as_deref(),
                                );
                                (p.permission, lock_holder, remote_state, false, None)
                            }
                            // A definitive 403 means we are logged in but not a
                            // collaborator on this managed project - flag it so the
                            // UI can offer converting to a local file (fix 10).
                            Err(SpeleoDbError::Status { status: 403, .. }) => {
                                (None, None, RemoteState::Unknown, true, None)
                            }
                            // A 401 is a dead token (re-login) handled by the edit
                            // verbs; this passive probe must not nag about it.
                            Err(SpeleoDbError::Status { status: 401, .. }) => {
                                (None, None, RemoteState::Unknown, false, None)
                            }
                            // Any OTHER status (a 404 for a deleted/renamed project,
                            // a 422, a 5xx, ...) is unexpected - surface the humanized
                            // detail so the button can show a "?" state and explain
                            // what SpeleoDB said (fix 12), rather than silently
                            // degrading to Unknown.
                            Err(e @ SpeleoDbError::Status { .. }) => (
                                None,
                                None,
                                RemoteState::Unknown,
                                false,
                                Some(dto_for_error(Endpoint::GetProject, &e)),
                            ),
                            // Transport / local error is offline - stay quiet.
                            Err(_) => (None, None, RemoteState::Unknown, false, None),
                        }
                    }
                    Err(_) => (None, None, RemoteState::Unknown, false, None),
                }
            } else {
                (None, None, RemoteState::Unknown, false, None)
            };

        if self.speleodb_session_generation() != status_generation {
            return Err(local_dto(SpeleoDbError::Io(
                "project session changed during status refresh".to_owned(),
            )));
        }
        let edit_enabled = self.edit_enabled();
        // Status may close an existing session, but only Start Editing may open one.

        if self.speleodb_session_generation() != status_generation {
            return Err(local_dto(SpeleoDbError::SessionMoved));
        }

        Ok(SpeleoLinkStatus {
            is_remote: true,
            edit_enabled,
            has_unpushed_edits,
            // [#303] Mid-walk: a chain step has moved this copy off the tip. Reported RAW
            // so the frontend can combine it with its own optimistic dirty bit - a step onto
            // the tip leaves the copy clean, so a pre-combined flag was false at the only
            // moment the frontend refreshes.
            unwound: self.speleodb_unwound_from().is_some(),
            permission,
            lock_holder,
            remote_state,
            access_denied,
            server_error,
            // [#303] Resolved from the explicit project value. The global default is
            // consulted only while initializing a legacy NULL on open.
            undo_mode: self.undo_policy_for(Some(&link)).mode,
            project_id: Some(link.project_id.clone()),
        })
    }

    /// Best-effort remote-state (freshness fold, #202): resolve the server tip from
    /// the revision topology and project the BASE-vs-tip fence for the action
    /// button (T2c). Any failure → `Unknown` - the passive refresh must never surface
    /// an error, and it is NOT routed through `handle_auth` (a transient 401 here
    /// must not auto-drop the session, mirroring [`Self::speleodb_lock_holder`]).
    ///
    /// The policy itself lives in [`sync::status_verdict`] so every branch is
    /// unit-tested offline and the passive button provably agrees with what the
    /// write path ([`sync::plan_push`]) would do on click; this wrapper is just the
    /// I/O around it.
    fn speleodb_remote_state(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        link: &crate::models::SpeleoLink,
        lock_holder: Option<&str>,
    ) -> RemoteState {
        let my_email = self.speleodb_my_email().unwrap_or_default();
        let we_hold_the_lock = self.speleodb_lock_acquired_here(&link.project_id)
            && self.edit_enabled()
            && lock_holder.is_some_and(|holder| holder.eq_ignore_ascii_case(&my_email));
        match head_with_empty_retry(link.base_commit_sha.as_deref(), || {
            self.speleodb_server_head(endpoint, token, insecure_http, &link.project_id)
        }) {
            Ok((tip, commits)) => sync::status_verdict(
                link.base_commit_sha.as_deref(),
                tip.as_deref(),
                link.has_unpushed_edits,
                &commits,
                &my_email,
                we_hold_the_lock,
            ),
            Err(_) => RemoteState::Unknown,
        }
    }

    /// Open a Start Editing operation: forget whatever the PREVIOUS one left in
    /// the unwind slot.
    ///
    /// The slot answers "what did the acquire that just ran take?", and the
    /// honest answer for an acquire that INHERITED a lock the account already
    /// held - or one that aborted before committing - is nothing. Neither of
    /// those writes the slot, so without this the stale entry from an earlier
    /// operation stood in for them, and the revert flow's unwind released THAT
    /// lock: possibly one a logout kept back precisely because it guards
    /// un-pushed work, and certainly not the mutex this operation was about.
    ///
    /// Called inside the lock-ops span, so no other flow can be between its own
    /// clear and its own commit while this runs.
    pub(super) fn speleodb_begin_acquisition_flow(&self) {
        if let Ok(mut session) = self.speleodb_session_lock() {
            session.acquired_by_this_flow = None;
        }
    }

    /// Whether THIS process took `project_id`'s write lock (an AUTHORED claim, see
    /// [`super::speleodb_session::LockClaim`]) as the account currently signed in.
    /// The authorship licence; whether this account holds ANY claim on a project
    /// is [`Self::speleodb_holds_claim`].
    ///
    /// The account test is what keeps a claim from speaking for a session it has
    /// nothing to do with: signing out and in as somebody else does not make their
    /// lock ours, and it must not license the authorship shortcut over their push.
    pub(super) fn speleodb_lock_acquired_here(&self, project_id: &str) -> bool {
        self.speleodb_session_lock().is_ok_and(|session| {
            session.locks_acquired_here.iter().any(|claim| {
                claim.project_id == project_id
                    && claim.is_authored()
                    && claim.taken_by(session.endpoint.as_deref(), session.email.as_deref())
            })
        })
    }

    /// Whether a claim exists for `project_id` ON `endpoint`'s deployment,
    /// whoever holds it.
    ///
    /// The reclaim check's question, and deliberately NOT
    /// [`Self::speleodb_lock_acquired_here`]'s. A captured release names the
    /// server it is for and may outlive the session - and even the sign-in -
    /// that captured it, so "is this OUR account's?" is the wrong test: what
    /// matters is whether some session in this process has claimed the very
    /// mutex the release would free. Keyed on the deployment because a project
    /// UUID is unique only on one server; a clone holding the same id must not
    /// answer for it.
    pub(super) fn speleodb_claim_on_deployment(&self, endpoint: &str, project_id: &str) -> bool {
        self.speleodb_session_lock().is_ok_and(|session| {
            session
                .locks_acquired_here
                .iter()
                .any(|claim| claim.project_id == project_id && claim.on_deployment(endpoint))
        })
    }

    /// WHICH project the most recent Start Editing ACQUIRED a lock on in this
    /// process, if it acquired one at all - consumed, so it answers once.
    ///
    /// For the one caller that needs to unwind an acquisition WITHOUT knowing
    /// what got acquired ([`Self::speleodb_release_lock_acquired_here`]): the
    /// stale-context unwind, whose whole premise is that the open project moved
    /// under it. That caller cannot name the project, so this names it - and it
    /// has to name the acquisition THIS operation made, not merely one of this
    /// account's claims. Those are no longer the same thing: claims now survive
    /// per project, and an older one may be a lock a logout deliberately kept
    /// back because it guards un-pushed work. Releasing that instead was both a
    /// wrong release and a stranded mutex.
    ///
    /// Empty when the acquire INHERITED a lock the account already held (no
    /// acquisition, nothing of ours to hand back) or when the last one has
    /// already been unwound, which is exactly the no-op the caller documents.
    /// The account is re-checked at consumption: a sign-in as somebody else in
    /// between makes this session's credentials the wrong ones to release with.
    pub(super) fn speleodb_take_acquisition_by_this_flow(&self) -> Option<String> {
        let mut session = self.speleodb_session_lock().ok()?;
        let claim = session.acquired_by_this_flow.take()?;
        claim
            .taken_by(session.endpoint.as_deref(), session.email.as_deref())
            .then(|| claim.project_id.clone())
    }

    /// Hand back a write lock ONLY if this process is the one that took it, and only
    /// the one it took.
    ///
    /// For an unwind that has discovered its context went stale - the revert composite
    /// finding a different project open after its acquire. That caller cannot express
    /// this itself, and gets it wrong in two ways if it tries:
    ///
    /// - `speleodb_release_lock` targets an ARBITRARY project by design (#201's
    ///   held-lock prompt, where releasing a lock this process never took IS the
    ///   point). Handing it an id unwinds whatever holds that id.
    /// - the id the caller captured is not necessarily the id that got locked, because
    ///   `speleodb_start_editing` locks whatever is OPEN - which is the very thing the
    ///   stale-context check has just proved changed.
    ///
    /// And one of those ways is destructive rather than merely wrong. An acquire that
    /// met `Ok(Some(us))` INHERITED a lock a sibling session on this account is holding
    /// over its own un-pushed work, and `speleodb_start_editing` is explicit that no
    /// unwind may hand that back. `lock_acquired_here` is the durable form of the same
    /// verdict its `release_unless_kept` closure reads, so this releases exactly what
    /// this process took and no-ops on an inherited lock or none at all.
    ///
    /// Silent when there is nothing of ours to release: the caller is already reporting
    /// the staleness it found, and "no lock was ours" is not a second problem.
    pub fn speleodb_release_lock_acquired_here(&self) -> Result<(), SpeleoErrorDto> {
        let Some(project_id) = self.speleodb_take_acquisition_by_this_flow() else {
            return Ok(());
        };
        self.speleodb_release_lock(project_id)
    }

    /// [Track C] (#188, C5): centralized session-level 401 auto-drop handler.
    /// Every **established-session** authed call (list/open) routes its `Result`
    /// through this; a **genuine 401** clears the saved and cached token via `forget()`. A 403 is **never** a drop (D-B4:
    /// per-endpoint read-only / fence semantics, robust to a DRF 401→403
    /// downgrade); 2xx/5xx pass through untouched.
    ///
    /// The login path is deliberately NOT routed here: login's 401 is a
    /// pasted-dead-token, not the drop of an *established* session.
    pub(super) fn speleodb_handle_auth<T>(
        &self,
        endpoint: Endpoint,
        result: Result<T, SpeleoDbError>,
    ) -> Result<T, SpeleoDbError> {
        // Unambiguous dead token (a 401, or a 403 on the whoami `User` endpoint)
        // drops immediately. An ambiguous resource/write 403 (a real read-only /
        // fence grant vs a dead token) is disambiguated with a whoami probe.
        if auth_error_drops_token(endpoint, &result)
            || self.speleodb_ambiguous_403_is_dead_token(endpoint, &result)
        {
            self.speleodb_forget();
        }
        result
    }

    /// Disambiguate a resource/write **403** (D-B4 + the live-verified DRF 401→403
    /// downgrade): the box answers 403 for BOTH a real read-only / fence grant AND
    /// a dead token, so a 403 here is ambiguous. Probe the `/user/` whoami
    /// chokepoint (200 for any valid token, 403 for a dead one) and report the
    /// token dead **iff** the probe also indicates death - a whoami success means a
    /// genuine permission denial, so the token is kept (never log out a read-only
    /// user). Only the ambiguous 403s (`ReadOnly` / `Fence`) reach the probe; an
    /// unambiguous `ReLogin` (whoami 403 / 401) is handled by
    /// [`auth_error_drops_token`], and other statuses never indicate a dead token.
    fn speleodb_ambiguous_403_is_dead_token<T>(
        &self,
        endpoint: Endpoint,
        result: &Result<T, SpeleoDbError>,
    ) -> bool {
        let Err(SpeleoDbError::Status { status, .. }) = result else {
            return false;
        };
        if *status != 403
            || !matches!(
                map_status(endpoint, *status),
                SpeleoDbAction::ReadOnly | SpeleoDbAction::Fence
            )
        {
            return false;
        }
        self.speleodb_token_is_dead()
    }

    /// One whoami round-trip to test whether the current session token is still
    /// valid (see [`whoami_indicates_dead_token`]). Taken only on an ambiguous 403.
    /// Returns `false` when there is no established token (nothing to drop).
    fn speleodb_token_is_dead(&self) -> bool {
        let Ok((endpoint, token)) = self.speleodb_active_token() else {
            return false;
        };
        let insecure_http = self.speleodb_insecure_http();
        whoami_indicates_dead_token(&client::fetch_user_email(&endpoint, &token, insecure_http))
    }

    // `speleodb_logout` lives in `speleodb_logout`: it is three independently
    // fallible clears (gates + lock, credential, rehydrate seed) whose failure rules
    // are subtle enough to want their own module.

    /// Where a pulled `.stalx` is cached: `<cache>/speleodb/<project_id>/project.stalx`.
    /// The layout itself lives in `speleodb_local_state`, which also walks it to
    /// report per-project local state without opening anything read-write.
    ///
    /// Refuses an id that is not a project UUID ([`SpeleoDbError::InvalidProjectId`])
    /// before anything else, for every caller: the pull, the revision restore and
    /// the push snapshot all write under this path, and the push takes the id from
    /// the open file's `speleo_link` row. That row is validated on the way in today
    /// (the load refuses a linked file whose id is not a UUID, `speleodb_foreign`,
    /// and every verb reads the link through `speleodb_session_snapshot` /
    /// `speleodb_validated_link`); this keeps the path safe if a caller ever skips
    /// that.
    pub(super) fn speleodb_cache_path(&self, project_id: &str) -> Result<PathBuf, SpeleoDbError> {
        validate_project_id(project_id)?;
        let root = super::speleodb_local_state::cache_root()
            .ok_or_else(|| SpeleoDbError::Io("no app cache directory available".to_owned()))?;
        super::speleodb_local_state::validated_cache_path_in(&root, project_id)
    }

    pub fn speleodb_start_editing(&self) -> Result<StartEditingResult, SpeleoErrorDto> {
        // The lock-ops span opens FIRST, before a single fact is read: everything
        // below is decided from facts (the session generation, the link, the
        // token, the mutex holder) that another lock operation could invalidate,
        // and this guard is what stops one running while they are in flight. It
        // covers the token read specifically because logout takes the same guard -
        // a logout that slipped in after the token was read left this flow acquiring
        // and committing on behalf of a signed-out session, which the fast-forward
        // arm's identity-only commit cannot see (the slot never moves). Held across
        // the network span on purpose - see `speleodb_session`.
        let _lock_ops = self.speleodb_lock_ops_guard();
        // This operation's unwind slot starts empty. An acquire that inherits an
        // already-held lock, or one that aborts, records nothing - and must
        // leave the stale-context unwind with nothing to hand back, rather than
        // the previous operation's acquisition.
        self.speleodb_begin_acquisition_flow();

        // Generation + link read as ONE world under the `project` mutex (D6): the
        // commit far below re-checks the generation before opening the gate, so a
        // teardown landing inside the network window refuses instead of committing
        // onto a torn-down world (F7). See `speleodb_session`.
        let (entry_gen, link) = self.speleodb_session_snapshot()?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        if normalize_endpoint(&endpoint) != normalize_endpoint(&link.endpoint) {
            return Err(local_dto(SpeleoDbError::SessionMoved));
        }
        // Before anything is sent: another account's un-uploaded work in this copy
        // is never taken over by acquiring its lock (see `speleodb_cached`).
        self.speleodb_guard_open_copy_owner(&endpoint, &link)
            .map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        // Whether the acquire below is OURS to claim - probed BEFORE it, because the
        // acquire itself cannot tell us. Re-acquiring a lock the ACCOUNT already holds
        // is a successful server-side no-op (only ANOTHER holder produces the 400 that
        // reports contention), and `ActiveMutex` parses no timestamp, so a 200 here
        // says nothing about whether this process caused the transition. Without the
        // probe a restarted process, a second window, or the held-lock dialog's
        // "Continue editing" - which calls this verb precisely to resume an INHERITED
        // lock - would all mint an authorship claim they never earned.
        //
        // Only a mutex the server says is UNHELD proves the unheld -> ours transition
        // is ours. A FAILED probe is not "unheld": reading it as one would mint the
        // record on a network blip, so `Err` fails safe to false. The user then edits
        // exactly as before, just without an authorship claim - the shortcut is lost,
        // never wrongly granted.
        //
        // Residual, and irreducible without a server-side compare-and-set: between
        // this probe and the acquire another session on our account IN ANOTHER
        // PROCESS (a second window, the website) could take the lock, and we would
        // record an acquisition that was really theirs. That is the same fail
        // direction as today, over a window of milliseconds rather than a whole
        // process lifetime. In-process flows are no longer part of this residual:
        // the lock-ops span below starts before the probe precisely so a captured
        // or janitor release cannot land between the probe and the acquire.
        //
        // FOUR outcomes over three probe states, because a HELD mutex says nothing on
        // its own - only its holder does - and the two questions the probe answers
        // fail safe in opposite directions, of which only the FIRST is about
        // authorship:
        //
        // - `Ok(None)` (unheld) proves the unheld -> ours transition, so it may mint an
        //   authorship record. A lock this call demonstrably took is also ours to hand
        //   back on the way out - unless the unwind itself turns up local work behind
        //   it (the closure below).
        // - `Ok(Some(us))` is a lock OUR OWN ACCOUNT already held: the acquire below is
        //   a server-side no-op, we INHERITED it, and no unwind here may hand it back.
        // - `Ok(Some(someone_else))` followed by a SUCCESSFUL acquire is neither - it is
        //   `Ok(None)` arriving late. A holder who is still there answers the acquire
        //   with the 400 that returns `LockedBy` before either flag below is read, so
        //   reaching the arms at all means they let go in between and THIS call made the
        //   unheld -> ours transition. Reading it as inherited (what this used to do)
        //   costs the authorship shortcut over a push that really was ours AND, worse,
        //   skips the unwind release - stranding a mutex we took over an edit session
        //   that never started, which blocks every other writer until the next login's
        //   held-lock prompt or a release by hand (SpeleoDB has no lock timeout).
        // - `Err` is provenance we could not read at all. No record (as above), and the
        //   keep-or-release question is settled by what a release would COST rather
        //   than by who took the lock - see `keep_lock_on_unwind` below, which is
        //   that cost as the pre-acquire evidence can price it, and NOT a claim about
        //   inheritance. It is one of the two keep rules, not the whole decision: what
        //   a failure turns up afterwards is the other.
        //
        // Telling the middle two apart needs OUR email, which `speleodb_active_token`
        // above proved we have (it refuses without one) - but a concurrent logout can
        // still drop it under an in-flight call, so a holder we cannot compare against
        // is NOT "not us": it falls through to the `Err` answer. "Cannot say" must
        // never mint a claim; that is the same direction a failed probe fails in.
        //
        // Comparison is `lock_held_by_me`, the same case-insensitive predicate the #202
        // re-verify and the gate re-derivation use - SpeleoDB may normalise case, and
        // three hand-rolled spellings of "is this holder us?" is how they drift apart.
        //
        // The probe reads provenance INSIDE the lock-ops span opened at the top of
        // this function, and that placement is load-bearing: outside it, a captured
        // release can invalidate the answer before the acquire runs. The probe sees
        // the account still holding (the delete's deferred release has not POSTed
        // yet), answers "inherited, keep on unwind", the release then frees the
        // mutex, and the acquire takes a FRESH lock this call believes it merely
        // resumed - so an abort arm skips the unwind and strands a mutex we in fact
        // took. Inside the span the release either finished first (the probe reads
        // the freed world and the acquire is ours to record and to hand back) or
        // waits until our commit (the probe's "inherited" answer stays true for the
        // whole span, and the release's reclaim check then stands down for the live
        // session).
        let me = self.speleodb_my_email();
        let (acquired_here, keep_lock_on_unwind) = match self.speleodb_lock_holder_checked(
            &endpoint,
            &token,
            insecure_http,
            &link.project_id,
        ) {
            // Unheld -> this call took it: record the acquisition, hand it back if we
            // walk away.
            Ok(None) => (true, false),
            // Already ours -> inherited: no authorship, and never hand it back.
            Ok(Some(holder)) if lock_held_by_me(Some(holder.as_str()), me.as_deref()) => {
                (false, true)
            }
            // Someone else's, and the acquire below still succeeded -> they released in
            // between and this call took it: answer exactly as `Ok(None)` does.
            Ok(Some(_)) if me.is_some() => (true, false),
            // Unknown - an unreadable probe, or a holder we have no email to compare
            // against. No record either way, and keep the lock only when there is
            // un-pushed work behind it to protect - as far as the OPEN project's link
            // can tell, which is not the only copy there is (see the closure below).
            Ok(Some(_)) | Err(_) => (false, link.has_unpushed_edits),
        };

        // The unwind shared by every arm below that walks away WITHOUT an edit
        // session: hand the lock back, UNLESS one of TWO rules says the lock is worth
        // more here than the tidy-up is. `keep_lock_on_unwind` is the first - we know
        // we inherited it, or we do not know and there are un-pushed edits behind it -
        // and the failure being unwound is the second (`failure`, further down).
        //
        // Releasing a lock we merely INHERITED is the held-lock prompt's "Continue
        // editing" surrendering the very mutex it exists to protect: a stranded
        // session resumes, the reconcile says the server tip moved (a fork, or our own
        // earlier upload that landed without advancing BASE), and the un-pushed work
        // still only on this machine is left behind a project that is now free for the
        // next writer to lock and push. Start editing that fails to start editing must
        // leave the lock exactly as it found it; ending an inherited session is Done /
        // Release / close's job - verbs the user aims at the lock on purpose. Same
        // asymmetry `speleodb_lock_release_params` already encodes: a lock kept back is
        // recoverable (the next login's scan re-raises this very prompt, or the user
        // releases it on the website; SpeleoDB never expires a lock on its own),
        // un-pushed work behind a freed lock is not.
        //
        // A FAILED probe cannot decide this by provenance - it does not know any - so
        // it decides by consequence, and the two blanket answers are both wrong:
        //
        // - Blanket RELEASE (what this used to do) re-opens the very hole above. A
        //   `get_project` that blips while the acquire succeeds over a lock the
        //   ACCOUNT already held reads as "not inherited", and the first unwind then
        //   frees the project for the next writer while the un-pushed work is still
        //   only on this machine.
        // - Blanket KEEP strands a lock we may well have just taken over a copy with
        //   nothing to protect, for nothing - recoverable only via the next login's
        //   held-lock prompt or a release by hand: the server has no lock timeout.
        //
        // So the unknown case keys on whether there IS anything to protect
        // (`Err(_) => link.has_unpushed_edits`), which is the same asymmetry
        // `speleodb_lock_release_params` already encodes: never silently release a
        // lock whose project still holds edits the server has not seen.
        //
        // Nothing touches the acquisition record on the first rule's skip path,
        // deliberately: the record is dropped by RELEASING (in
        // `speleodb_release_best_effort`), and a release we declined to make ends no
        // claim. `acquired_here` is false there, so this call mints nothing either -
        // record and lock stay in step. (The second rule's skip leaves a kept
        // pointer, below.)
        //
        // `failure` is the SECOND keep rule, and the reason this takes the error at
        // all. `keep_lock_on_unwind` is decided BEFORE the acquire and reads the OPEN
        // project's link, so it cannot see un-pushed work in a copy that is not the
        // open one - and there can be: Save-As carries the `speleo_link` row forward,
        // so P can be open (and clean) from outside the cache while the CACHE copy of
        // P still holds edits, which is the file every re-pull here overwrites.
        // `is_local_edits_refusal` is that copy answering for itself: the guard has
        // just read it and found work (or found it unreadable, which fails closed the
        // same way), so the mutex we are about to hand back may be the one holding the
        // project for exactly that work. It is decided per-FAILURE rather than
        // per-probe because it is knowledge that only exists after the acquire.
        //
        // Only that one failure keeps. Everything else answers as before, so a corrupt
        // dump with nothing local at stake still hands back a lock we took. And when it
        // keeps a lock the FIRST rule would have released - one this call took, or
        // unknown provenance over a clean open copy - it leaves a KEPT claim behind:
        // without one that lock was held with nothing local pointing at it, so no
        // close, cleanup or held-lock decision of this process could ever find it
        // again. Kept, so no automatic release hands it back over the cache copy's
        // un-uploaded work; an explicit Release, or the next edit session, ends it.
        // Holder-checked against OUR email (lock#4): with no email to compare
        // against, the server's holder can never match and nothing is released.
        let unwind_params = super::speleodb_lock_release::LockReleaseParams {
            endpoint: endpoint.clone(),
            token: token.clone(),
            email: me.clone().unwrap_or_default(),
            insecure_http,
            project_id: link.project_id.clone(),
        };
        let release_unless_kept = |failure: Option<&SpeleoDbError>| {
            self.speleodb_unwind_unless_kept(
                keep_lock_on_unwind,
                failure,
                &unwind_params,
                entry_gen,
            );
        };

        // Acquire the lock. A 400 is the server's "someone else holds it" signal;
        // it has no first-class map_status row, so it is handled here (per #194).
        let acquired = self.speleodb_handle_auth(
            Endpoint::Acquire,
            client::acquire_lock(&endpoint, &token, insecure_http, &link.project_id),
        );
        // We only need the acquire to confirm we hold the lock; its `latest_commit`
        // is unreliable (see speleodb_server_head) so it is intentionally dropped.
        match acquired {
            Ok(_project) => {}
            Err(SpeleoDbError::Status { status: 400, .. }) => {
                return Ok(self.speleodb_locked_by(
                    &endpoint,
                    &token,
                    insecure_http,
                    &link.project_id,
                ));
            }
            // sdk#4: a failure that says nothing about whether the acquire LANDED.
            // The server may have committed the mutex to us before a proxy gave up
            // on the reply, the connection reset, or the SDK refused the body - and
            // SpeleoDB never expires a lock on its own, so walking away from one we
            // hold blocks every other writer until somebody releases it by hand.
            // The server's mutex is the only record of what happened: read it.
            Err(e) if acquire_outcome_unknown(&e) => {
                match self.speleodb_lock_holder_checked(
                    &endpoint,
                    &token,
                    insecure_http,
                    &link.project_id,
                ) {
                    // It landed; only the reply was lost. ADOPT it and carry on as
                    // if the acquire had answered: the user asked to edit, and the
                    // lock is ours either way. Provenance stays what the
                    // pre-acquire probe established (`acquired_here`), and every
                    // abort arm below unwinds it exactly as it would a clean acquire.
                    Ok(Some(holder)) if lock_held_by_me(Some(holder.as_str()), me.as_deref()) => {
                        tracing::info!(
                            error = %e,
                            project_id = %link.project_id,
                            purpose = "adopt an acquire whose reply was lost (sdk#4)",
                            "speleodb: the acquire failed ambiguously but the server says we \
                             hold the lock; continuing as if it had answered"
                        );
                    }
                    // Somebody else holds it: our acquire did not take it.
                    Ok(Some(holder)) if me.is_some() => {
                        return Ok(StartEditingResult::LockedBy {
                            email: Some(holder),
                        });
                    }
                    // Nobody holds it (the acquire did not land), or there is no
                    // email to compare the holder with: report the failure.
                    Ok(_) => return Err(dto_for_error(Endpoint::Acquire, &e)),
                    // Still cannot tell. Releasing blind is never an answer (lock#4),
                    // so leave a PROVISIONAL claim - unbound, it opens nothing - as
                    // the pointer the next close or the janitor needs: their
                    // holder-checked release hands the lock back only if it turns out
                    // to be ours. Only where the unwind would have released a lock
                    // this call took; a kept one (inherited, or unknown over un-pushed
                    // edits) is left exactly as it was found.
                    Err(_) => {
                        if !keep_lock_on_unwind {
                            self.speleodb_record_unbound_claim(&link.project_id);
                        }
                        return Err(dto_for_error(Endpoint::Acquire, &e));
                    }
                }
            }
            Err(e) => return Err(dto_for_error(Endpoint::Acquire, &e)),
        }

        // We hold the lock. Resolve the true HEAD from the revision topology and
        // fence our BASE against it (re-fetch once on a transient empty tip).
        let (tip, commits) = match head_with_empty_retry(link.base_commit_sha.as_deref(), || {
            self.speleodb_server_head(&endpoint, &token, insecure_http, &link.project_id)
        }) {
            Ok(v) => v,
            Err(e) => {
                // The likeliest place an unknown-provenance probe lands: whatever made
                // the probe GET fail tends to fail this GET too. So this is the site
                // that decides how much a blanket answer would cost - a dirty copy
                // keeps the lock that is guarding it, a clean one still hands it back.
                // The failure is handed over for the same reason the arm below hands
                // its own over, though nothing this GET can fail with is the local-edits
                // refusal: the unwind asks about the failure it is unwinding, always.
                release_unless_kept(Some(&e));
                return Err(dto_for_error(Endpoint::GetProject, &e));
            }
        };
        match sync::reconcile_verdict(
            link.base_commit_sha.as_deref(),
            tip.as_deref(),
            link.has_unpushed_edits,
            &commits,
        ) {
            ReconcileVerdict::UpToDate => {
                if let Err(e) =
                    self.speleodb_commit_editing(Some(entry_gen), &link.project_id, acquired_here)
                {
                    release_unless_kept(Some(&e));
                    return Err(local_dto(e));
                }
                // [#303] Clear-on-lock (clarification #2 / q14), gated on the copy really
                // being the file that is in git: `UpToDate` means BASE == the server tip,
                // and a clean link means we hold nothing the server has not seen. Under
                // both, whatever undo history is here describes operations that are
                // already committed - a stack inherited from the last pusher - and
                // starting an edit session one Ctrl-Z away from a stranger's operation is
                // the #194 b3 hazard.
                //
                // The guard is what makes the maintainer's case work: lock, edit, unlock
                // WITHOUT pushing, re-lock. That copy is dirty, so this does not fire and
                // the user's own history survives.
                //
                // AFTER the commit, not before: a session torn down in the window above
                // refuses with nothing committed, and clearing there would have thrown
                // away a stack the aborted session never went on to use.
                if !link.has_unpushed_edits {
                    self.speleodb_clear_inherited_undo(
                        "lock acquired over a copy that matches git",
                    );
                }
                Ok(StartEditingResult::Editing)
            }
            ReconcileVerdict::FastForward => {
                // A failure here (e.g. a corrupt re-pulled file) leaves us holding
                // the lock with a closed project - release it before surfacing the
                // error so it isn't stranded (mirrors the divergence arm), and on the
                // same terms: `release_unless_kept`, never a bare release. This arm
                // looks like the one safe place to release regardless, since the FF
                // verdict is reached only with `link.has_unpushed_edits == false` -
                // but the failure it is unwinding includes `Protect` refusing precisely
                // BECAUSE the copy turned out to hold un-pushed edits, which is the one
                // case where releasing an INHERITED lock does abandon them.
                //
                // So the FAILURE decides too, not just the pre-acquire probe: `Protect`
                // reads the copy at the CACHE path (and fails closed when it cannot be
                // read), which is not necessarily the open project's file, so it can
                // refuse over un-pushed work the open link does not carry - and the FF
                // verdict is reached only over a CLEAN open link, so on this arm the
                // pre-acquire flag keeps the lock for an inherited one and nothing
                // else. Handing the refusal to `release_unless_kept` is what lets the
                // cache copy speak for itself; every other failure releases as before.
                let local_path = match self.speleodb_fast_forward(
                    &endpoint,
                    &token,
                    insecure_http,
                    &link,
                    tip.as_deref(),
                    (LocalEditsPolicy::Protect, entry_gen, None),
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        release_unless_kept(Some(&e));
                        return Err(local_dto(e));
                    }
                };
                // The prepared replacement commits exactly one session boundary.
                // Refuse a later close/reopen even when it names the same project.
                if let Err(e) = self.speleodb_commit_editing(
                    Some(entry_gen.wrapping_add(1)),
                    &link.project_id,
                    acquired_here,
                ) {
                    release_unless_kept(Some(&e));
                    return Err(local_dto(e));
                }
                Ok(StartEditingResult::FastForwarded { local_path })
            }
            // A fork, a moved tip our BASE isn't an ancestor of, OR a server
            // inconsistency (non-linear / multi-root / a recorded BASE with no tip):
            // can't edit a diverged/anomalous copy. Release the lock we JUST TOOK so
            // it isn't stranded while the user reconciles (#198, next batch) and
            // surface the divergence prompt - never overwrite, never a hard error
            // (#193 b2). The edit gate stays disabled.
            //
            // A lock we INHERITED is kept instead - as is one whose provenance the
            // probe could not establish over a copy with un-pushed edits, which is the
            // shape this arm sees most (a dirty copy is what makes the fence say
            // Diverged in the first place); see `release_unless_kept`. The result is
            // the same either way: the user gets the divergence prompt over a read-only
            // project, now still holding the mutex that was guarding their un-pushed
            // work. `Take remote` / `Save as unmanaged` reconcile from there exactly as
            // before - neither needs the lock released first.
            ReconcileVerdict::Diverged => {
                // `None`: a verdict, not a failure - there is nothing here that has
                // read a local copy, so the pre-acquire flag is the whole answer.
                release_unless_kept(None);
                Ok(StartEditingResult::DivergenceNeeded)
            }
        }
    }

    /// The open project's hand-off link with its `project_id` validated (the
    /// route-traversal guard). The shared preamble for every SpeleoDB edit verb
    /// (start_editing / done / update) so no entry point can forget the validation.
    pub(super) fn speleodb_validated_link(
        &self,
    ) -> Result<crate::models::SpeleoLink, SpeleoErrorDto> {
        let link = self.speleodb_open_link().map_err(local_dto)?;
        validate_project_id(&link.project_id).map_err(local_dto)?;
        Ok(link)
    }

    /// Read the open project's persisted SpeleoDB hand-off link, or
    /// [`SpeleoDbError::NoRemoteProject`] when no remote project is open.
    pub(super) fn speleodb_open_link(&self) -> Result<crate::models::SpeleoLink, SpeleoDbError> {
        let db_guard = self
            .database
            .lock()
            .map_err(|_| SpeleoDbError::Io("database lock poisoned".to_owned()))?;
        let db = db_guard.as_ref().ok_or(SpeleoDbError::NoRemoteProject)?;
        db.load_speleo_link()
            .map_err(|e| SpeleoDbError::Io(e.to_string()))?
            .ok_or(SpeleoDbError::NoRemoteProject)
    }

    /// Convert the open managed project into a plain, unmanaged `.stalx` (T2/#204):
    /// drop its `speleo_link` row so it is no longer tied to SpeleoDB, then open the
    /// edit gate. Two callers reach this: (10) a project the server refused us access
    /// to (`access_denied`), and (11) keeping locally-edited work after a divergence
    /// by saving it elsewhere. Nothing is pushed - the SpeleoDB copy is abandoned (a
    /// fresh Open-from-SpeleoDB re-pulls the server tip), so the managed side stays
    /// clean. No-op-safe: a `None` delete over an absent row is fine.
    ///
    /// A held write lock IS released first (best-effort, mirroring logout - the lock
    /// needs the token, so before we drop the link): abandoning the project while
    /// still holding its server mutex would strand it locked for every other editor.
    /// Unlike logout the token is kept - the user stays signed in, they just walk
    /// away from this one project.
    pub fn speleodb_unlink(&self) -> Result<(), SpeleoErrorDto> {
        // Release the server lock if this is a held remote edit session, so we don't
        // leave it stranded. Best-effort: an offline / no-access release just no-ops.
        // The ABANDONING variant: un-pushed edits do not veto this release the way
        // they veto the silent close/quit/switch one, because unlink deletes the link
        // row - a lock kept back here could never be released from this project
        // again, and the user has said outright that the managed copy is abandoned.
        self.speleodb_release_held_lock_abandoning();
        {
            let db_guard = self
                .database
                .lock()
                .map_err(|_| local_dto(SpeleoDbError::Io("database lock poisoned".to_owned())))?;
            let db = db_guard
                .as_ref()
                .ok_or_else(|| local_dto(SpeleoDbError::NoRemoteProject))?;
            db.save_speleo_link(None)
                .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        }
        // No link, no lock to edit under: the project is unmanaged now, and an
        // unmanaged project is always editable (`speleodb_edit_gate`).
        self.speleodb_set_open_gate(None);
        Ok(())
    }

    /// Take the SERVER version, DISCARDING all local unpushed edits (#198 "Take
    /// remote"): re-pull the server tip and reset the local `.stalx` to it,
    /// abandoning the local work. This is the ONLY irreversible choice in the
    /// divergence prompt (local unpushed edits live in no git anywhere), so the
    /// frontend gates it behind a SECOND destroy-confirmation before calling here;
    /// there is no single-click path to this from the UI. Returns the reloaded
    /// `.stalx` path so the frontend can reload the view (as the fast-forward arms
    /// of Start-editing / Update do).
    ///
    /// Ends the edit session and leaves the project READ-ONLY: the held write lock
    /// is released (holder-checked and best-effort, like [`Self::speleodb_unlink`]),
    /// which takes the claim - and with it the edit gate (`speleodb_edit_gate`) -
    /// before the reload, and the reload can only carry an edit session that still
    /// has its claim. The user resumes via Start editing, which re-acquires the lock
    /// and now reconciles cleanly (BASE has advanced to the tip). (data#3: the reload
    /// used to copy the old gate across and reopen the project editable with no lock
    /// held.)
    pub fn speleodb_take_remote(&self) -> Result<String, SpeleoErrorDto> {
        let (generation, link) = self.speleodb_session_snapshot()?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        // Authoritative server tip from the revision topology, NOT the unreliable
        // latest_commit field (see speleodb_server_head).
        let (tip, commits) = head_with_empty_retry(link.base_commit_sha.as_deref(), || {
            self.speleodb_server_head(&endpoint, &token, insecure_http, &link.project_id)
        })
        .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;

        // Fail closed on an ambiguous fork: `tip_of` returns None with a NON-empty
        // history when there are multiple heads. We must not stamp BASE = None (the
        // empty-repo sentinel) over a real server tree, which would leave a BASE that
        // does not describe the downloaded working tree. Abort with the lock still
        // held and the local edits intact so the user can instead keep their work via
        // "Save as an unmanaged file". (An empty history yields tip == None too, but
        // then there is nothing to take - the fast-forward handles the empty case.)
        if tip.is_none() && !commits.is_empty() {
            return Err(local_dto(SpeleoDbError::Parse(
                "the SpeleoDB history has diverged into multiple heads; there is no single \
                 server version to take"
                    .to_owned(),
            )));
        }

        // Abandoning local edits ends the edit session. The session's claim is what
        // holds the gate open, so shut it first, outright: a release that is skipped
        // (no account claim to release, a token from another deployment) or that
        // fails (the claim comes back, unbound) must not leave it bound for the
        // reload below to carry.
        self.speleodb_shut_edit_gate();
        // Then release the held lock (if any) so it isn't left stranded, mirroring
        // speleodb_unlink. Best-effort and holder-checked - an offline release keeps
        // the claim as a pointer for a later retry. The ABANDONING variant,
        // necessarily: take-remote is only ever reached WITH un-pushed edits, and
        // discarding them is the whole point of the verb (behind the frontend's
        // second destroy-confirmation), so the veto that protects the silent
        // close/quit/switch release must not apply here.
        self.speleodb_release_held_lock_abandoning();

        // Re-pull + reset to the tip: BASE = tip, dirty cleared, last_edit_at reset,
        // and the in-place reload starts the undo/redo stacks empty (the server
        // snapshot carries no history). This IS a fast-forward, minus the dirty guard
        // - the whole point of take-remote is to discard the local edits. With no
        // bound claim there is no edit session for the reload to carry, so the
        // project comes back read-only.
        self.speleodb_fast_forward(
            &endpoint,
            &token,
            insecure_http,
            &link,
            tip.as_deref(),
            (LocalEditsPolicy::Discard, generation, None),
        )
        .map_err(local_dto)
    }

    pub fn speleodb_release_lock(&self, project_id: String) -> Result<(), SpeleoErrorDto> {
        // Validate the FE-supplied id BEFORE it is interpolated into the request URL
        // (it targets an arbitrary project, so unlike the open-project verbs it does
        // not pass through `speleodb_validated_link`): reject anything that is not a
        // plain UUID to close the path/route-traversal vector at the trust boundary,
        // exactly as `speleodb_open_project` does.
        validate_project_id(&project_id).map_err(local_dto)?;
        // ONE lock-ops span over the POST and the local cleanup below. This is a
        // lock operation like any other, and without the guard a Start Editing
        // for the same project could acquire the mutex this POST just freed and
        // record its claim BEFORE the cleanup runs - which then removes that new
        // claim and can put its live session back to read-only, leaving a
        // freshly acquired server mutex held and unaccounted for. Inside the
        // span the acquire runs wholly before this command or wholly after it.
        let _lock_ops = self.speleodb_lock_ops_guard();
        let (endpoint, token, email) = self.speleodb_active_account().map_err(local_dto)?;
        // Holder-checked like every release (lock#4): an explicit Release is aimed
        // at OUR lock, and the server would honour an admin's release of anybody's.
        // A lock somebody else holds now (or nobody) is not ours to free, so the
        // claim simply ends below; a holder that cannot be read is an error with
        // nothing sent and the claim kept, never a blind release.
        let released = self
            .speleodb_handle_auth(
                Endpoint::Release,
                super::speleodb_lock_release::release_if_held_by_us(
                    &super::speleodb_lock_release::LockReleaseParams {
                        endpoint,
                        token,
                        email,
                        insecure_http: self.speleodb_insecure_http(),
                        project_id: project_id.clone(),
                    },
                ),
            )
            .map_err(|e| dto_for_error(Endpoint::Release, &e))?;
        if released == super::speleodb_lock_release::HolderCheckedRelease::NotHeldByUs {
            tracing::info!(
                project_id = %project_id,
                purpose = "never release another account's lock (lock#4)",
                "speleodb: release requested for a lock this account no longer holds; \
                 ending the local claim only"
            );
        }
        // Whichever project's lock that was, this process no longer holds it, so an
        // acquisition recorded for it is over - and when it was the open project's
        // edit session, ending its claim shuts the gate with it
        // (`speleodb_edit_gate`). Keyed on the released id, so releasing some OTHER
        // project's lock leaves an open edit session alone - and keyed ATOMICALLY,
        // through the shared compare-and-clear. This was a
        // `speleodb_lock_acquired_here` read followed by a separate
        // `speleodb_note_lock_acquired(None)` write, i.e. two session-lock
        // acquisitions, which only NARROWED the window an acquisition landing between
        // them could be erased in rather than closing it.
        self.speleodb_forget_lock_acquired_for(&project_id);
        Ok(())
    }

    /// Best-effort: re-GET the project and return who holds its mutex (the active
    /// lock's user email), or `None` if unlocked / the lookup fails. Deliberately NOT
    /// routed through [`Self::speleodb_handle_auth`] - these are best-effort
    /// lock-holder probes (the concurrent-session notice on open, the contended-lock
    /// report), so a transient 401 must fall through rather than auto-drop the whole
    /// session (#197 review). Shared by both probe sites so they can't diverge.
    pub(super) fn speleodb_lock_holder(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
    ) -> Option<String> {
        self.speleodb_lock_holder_checked(endpoint, token, insecure_http, project_id)
            .ok()
            .flatten()
    }

    /// The same probe with all THREE outcomes kept apart: `Ok(None)` is a mutex the
    /// server says is UNHELD, `Ok(Some(user))` names its holder, and `Err` means we
    /// could not find out.
    ///
    /// [`Self::speleodb_lock_holder`] collapses the last two into `None`, which is
    /// right for the display probes that feed it (an unknown holder and no holder
    /// both render as "unknown") and wrong for
    /// [`Self::speleodb_start_editing`]'s acquisition provenance, where reading a
    /// failed GET as "unheld" would MINT a record on a network blip. Both views
    /// share this one implementation so they cannot drift.
    pub(super) fn speleodb_lock_holder_checked(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
    ) -> Result<Option<String>, SpeleoDbError> {
        client::get_project(endpoint, token, insecure_http, project_id)
            .map(|p| p.active_mutex.map(|m| m.user))
    }

    /// Re-GET the project to report who holds the contended lock. Best-effort: a
    /// failed GET (or a lock that has since cleared) reports the holder as unknown.
    fn speleodb_locked_by(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
    ) -> StartEditingResult {
        let email = self.speleodb_lock_holder(endpoint, token, insecure_http, project_id);
        StartEditingResult::LockedBy { email }
    }

    /// Best-effort lock release for when start-editing took the lock but can't
    /// proceed (divergence / anomaly). A failure is logged, never surfaced.
    ///
    /// "TOOK" is load-bearing: `speleodb_start_editing` routes its abort arms through
    /// `release_unless_kept`, which skips this entirely for a lock the account already
    /// held before that call, for one whose provenance its pre-acquire probe could not
    /// establish over a project with un-pushed edits, and for any lock whose unwind is
    /// the local-edits refusal - the pull cache answering that IT holds work the open
    /// project's link never carried (see there).
    ///
    /// Holder-checked like every automatic release (lock#4): the POST is sent only
    /// when SpeleoDB names this account as the holder
    /// ([`super::speleodb_lock_release::release_if_held_by_us`]).
    pub(super) fn speleodb_release_best_effort(
        &self,
        params: &super::speleodb_lock_release::LockReleaseParams,
    ) {
        // The shared sink for start-editing's abort arms, the publish push and -
        // through `speleodb_release_held_lock_*` - logout / unlink / take-remote /
        // the instance switch, so this project's claim ends here: a lock the server
        // confirms released, or confirms is not ours, is no longer this process's
        // to account for.
        //
        // Keyed on `project_id`, like `speleodb_release_lock`. It used to clear
        // whatever was recorded, on the grounds that clearing a record naming some
        // OTHER project could only cost the authorship shortcut - true when the
        // record was one slot holding at most a stale id, and false now that a
        // claim can be a live pointer to a mutex still held on another project
        // (the janitor's retry after a transient failure). This releases one
        // project's lock, so it may end one project's claim.
        let claim = self.speleodb_forget_claim_released_by(params);
        if let Err(e) = super::speleodb_lock_release::release_if_held_by_us(params) {
            tracing::warn!(
                error = %e,
                project_id = %params.project_id,
                purpose = "release a held SpeleoDB write lock",
                "speleodb: lock release failed; the claim is kept for a later holder-checked retry"
            );
            // Unknown outcome: the holder could not be read, or the POST failed.
            // The mutex may still be ours and the claim is the only local pointer
            // to it, so it goes back - as a pointer, never as an edit session - for
            // the next close or the no-project janitor to retry.
            self.speleodb_restore_lock_claim(claim);
        }
    }

    /// [#303] Drop the in-memory undo/redo stacks when an edit session starts over a
    /// copy whose history is not ours - the clear-on-lock rule (clarification #2).
    ///
    /// The download-replace chokepoint already empties the TABLES of every server copy
    /// it writes (see `InheritedUndo`), so in the ordinary flow this is a no-op. It is
    /// here for the copies that path never touched: a `.stalx` opened directly from disk
    /// (Save As'd, hand-copied, shared as a bare file), or one written by a build that
    /// predates the drop. Cheap, and the alternative is starting an edit session one
    /// Ctrl-Z away from a stranger's operation.
    ///
    /// `reason` goes to the log so the disappearance of a visible history is explicable.
    fn speleodb_clear_inherited_undo(&self, reason: &str) {
        // A fresh edit session starts at BASE, so any chain position from an earlier one
        // is stale: the next Unwind restarts from BASE rather than resuming a walk that
        // described a different working copy.
        self.speleodb_set_unwound_from(None);
        // ...and with the inherited history about to go with it, nothing in the stack is
        // inherited any more. A watermark left behind would cut this session's own work
        // out of its first push.
        self.speleodb_set_inherited_through(None);
        if let Ok(mut stack) = self.undo_stack.lock() {
            if stack.is_empty() && !stack.can_redo() {
                return;
            }
            let dropped = stack.len();
            stack.clear();
            tracing::info!(
                dropped,
                reason,
                purpose = "never inherit another editor's undo history (#194 b3 / #303)",
                "Cleared the undo history carried by the opened copy"
            );
        }
    }

    /// The authenticated user's email (for ancestry authorship checks), if any.
    pub(super) fn speleodb_my_email(&self) -> Option<String> {
        self.speleodb_session_lock()
            .ok()
            .and_then(|s| s.email.clone())
    }

    /// Load the open project's `speleo_link`, apply `f`, and persist it. Errors if
    /// no remote project is open.
    pub(super) fn speleodb_mutate_link(
        &self,
        f: impl FnOnce(&mut crate::models::SpeleoLink),
    ) -> Result<(), SpeleoDbError> {
        let db_guard = self
            .database
            .lock()
            .map_err(|_| SpeleoDbError::Io("database lock poisoned".to_owned()))?;
        let db = db_guard.as_ref().ok_or(SpeleoDbError::NoRemoteProject)?;
        let mut link = db
            .load_speleo_link()
            .map_err(|e| SpeleoDbError::Io(e.to_string()))?
            .ok_or(SpeleoDbError::NoRemoteProject)?;
        f(&mut link);
        db.save_speleo_link(Some(&link))
            .map_err(|e| SpeleoDbError::Io(e.to_string()))?;
        Ok(())
    }

    /// #202 lazy lock re-verify: when EDITING a remote project with a real held
    /// lock, at most once per [`REVERIFY_INTERVAL_SECS`], re-GET the project and
    /// confirm we STILL HOLD the write lock. Called from the `execute()` command
    /// chokepoint AFTER `guard_mutation` (so it runs only for an enabled edit session
    /// on that path - undo/redo and the other direct mutation entrypoints
    /// deliberately do NOT re-verify; they are covered by the same backstops). It
    /// bounds wasted work; it is NOT a correctness guard (the release-time fence + a
    /// 403 on push are).
    ///
    /// The GET runs on a background thread and NEVER on the caller's: `execute()`
    /// is reached from synchronous commands, which Tauri runs on the main thread,
    /// under `command_lock`. An inline probe froze the whole window for up to the
    /// probe budget once per interval, and held every other edit, undo and save
    /// behind it. This call stamps the attempt and returns at once; the edit that
    /// triggered it goes ahead, exactly as it does when the probe fails offline. The
    /// verdict is collected by the next edit ([`Self::speleodb_collect_reverify`]),
    /// so a lost lock still routes the user to the divergence prompt, one edit
    /// later than when the check ran inline.
    ///
    /// Only the lock-holder is checked, NOT the server tip: while we hold the lock
    /// no other writer can advance the tip, so a "tip moved" signal would mean our
    /// lock was already lost - which the holder check catches directly. (The server's
    /// `latest_commit` field is `max(authored_date)` and unreliable as a tip anyway,
    /// see [`sync::tip_of`]; comparing it to a topology-derived BASE false-positives.)
    ///
    /// - No lock held (read-only / local project), not yet due, or a probe already
    ///   pending for this session: no-op.
    /// - Otherwise: stamp `last_verify_at` (keeps the <=3/hr bound even offline) and
    ///   start the probe ([`client::get_project_probe`], short and non-retried).
    pub(super) fn speleodb_verify_lock_if_expired(&self) {
        if !self.edit_enabled() {
            return;
        }
        let Ok(link) = self.speleodb_open_link() else {
            return; // local-only: no remote lock to verify
        };
        let now = chrono::Utc::now();
        if !reverify_due(link.last_verify_at.as_deref(), now) {
            return;
        }
        let Ok((endpoint, token, me)) = self.speleodb_active_account() else {
            return; // logged out: the close/logout path already drops the gate
        };
        let generation = self.speleodb_session_generation();
        // `execute()` holds `command_lock`, so nothing else starts a probe between
        // this check and the store below.
        if self
            .speleodb_reverify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|probe| probe.generation == generation)
        {
            return;
        }
        let _ = self.speleodb_mutate_link(|l| l.last_verify_at = Some(now.to_rfc3339()));
        let insecure_http = self.speleodb_insecure_http();
        let project_id = link.project_id;
        let probe = std::thread::spawn(move || {
            match client::get_project_probe(&endpoint, &token, insecure_http, &project_id) {
                Ok(project) => !lock_held_by_me(
                    project.active_mutex.as_ref().map(|m| m.user.as_str()),
                    Some(&me),
                ),
                Err(e) => {
                    // Transient / offline: #202 is not a correctness guard, so the
                    // edit stands (the release-time fence is the backstop).
                    tracing::warn!(
                        error = %e,
                        purpose = "lazy lock re-verify (#202)",
                        "speleodb re-verify GET failed; allowing edits (release fence is the backstop)"
                    );
                    false
                }
            }
        });
        *self
            .speleodb_reverify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(PendingReverify { generation, probe });
    }

    /// Collect a finished background re-verify (see
    /// [`Self::speleodb_verify_lock_if_expired`]). Called from `execute()` BEFORE
    /// `guard_mutation`, under `command_lock`; never waits for an unfinished probe.
    ///
    /// A probe that found the write lock no longer ours (released, admin-cleared, or
    /// taken by another session) drops the edit gate - so further edits are cleanly
    /// read-only until resolved - and fails THIS edit with
    /// `EDIT_REVERIFY_DIVERGED`, which the frontend routes to the divergence prompt.
    /// Only for the session that started the probe, and only while its gate is still
    /// open: a verdict about an earlier session, or a gate something else has already
    /// shut, says nothing about the edit in hand.
    pub(super) fn speleodb_collect_reverify(&self) -> Result<(), crate::core::CommandResult> {
        let finished = {
            let mut pending = self
                .speleodb_reverify
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !pending
                .as_ref()
                .is_some_and(|pending| pending.probe.is_finished())
            {
                return Ok(());
            }
            pending.take()
        };
        let Some(PendingReverify { generation, probe }) = finished else {
            return Ok(());
        };
        // Finished, so `join` returns at once. A probe that panicked proved nothing.
        if !probe.join().unwrap_or(false) {
            return Ok(());
        }
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.speleodb_session_generation() != generation || !self.edit_enabled() {
            return Ok(());
        }
        // The lock is not ours any more: the session is over. Shutting the gate
        // keeps the claim as a pointer - the next close asks the server who holds
        // it (lock#4) and just drops it when that is somebody else.
        self.speleodb_shut_edit_gate();
        Err(crate::core::CommandResult::failure(format!(
            "{}: the SpeleoDB write lock is no longer held - reconcile before editing.",
            crate::core::command::EDIT_REVERIFY_DIVERGED_CODE
        )))
    }
}

/// A #202 lock re-verify running in the background, started by one edit and
/// collected by a later one.
pub(super) struct PendingReverify {
    /// The session generation the probe was started in.
    pub(super) generation: u64,
    /// The probe thread. It returns `true` when it saw the write lock held by
    /// someone else (or by nobody), `false` when it is still ours or the probe failed
    /// offline. Polled with `is_finished`, so collecting it never blocks.
    pub(super) probe: std::thread::JoinHandle<bool>,
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
#[expect(
    clippy::panic,
    reason = "one test POISONS a mutex, which requires panicking while its guard is held"
)]
mod tests {
    use super::AppState;
    use super::{
        auth_error_drops_token, is_local_edits_refusal, local_dto, lock_held_by_me,
        managed_type_patch, resolve_endpoint, reverify_due, type_needs_managed_label,
        validate_project_id, whoami_indicates_dead_token, Endpoint, SpeleoDbSession,
        REVERIFY_INTERVAL_SECS,
    };
    use crate::speleodb::StartEditingResult;
    use std::path::Path;
    // [#275] The push half now lives in `speleodb_push`; these tests still exercise it
    // from here (they share this module's fixtures), so they reach it by its new path.
    use crate::core::state::speleodb_cache::{
        preserved_display_settings, restore_display_settings, InheritedUndo, LocalEditsPolicy,
    };
    use crate::core::state::speleodb_push::{
        clear_snapshot_dirty, head_with_empty_retry, type_still_says_ariane,
    };
    use crate::core::state::tests_support::speleodb_prod_matches;
    use crate::models::{Project, SpeleoLink};
    use crate::persistence::Database;
    use crate::speleodb::auth::TokenStore;
    use crate::speleodb::{DoneResult, PushResult, RemoteState, SpeleoDbError, TransportCause};
    use tempfile::TempDir;

    /// A `.stalx` on the server means the project is Stalagmight-managed, so the type
    /// should say so - and the ONE state worth healing is the one adoption leaves when
    /// its own push never ran (lock held elsewhere, read-only, offline). Everything
    /// else is left alone.
    ///
    /// `None` is the case worth being deliberate about: a type we could not READ is not
    /// a type we may act on. Reading it as "still ariane" would issue a write against a
    /// project whose current state we just failed to establish - on every push, since
    /// the read that failed is the one that would have told us to stop.
    #[test]
    fn only_a_project_the_server_still_calls_ariane_gets_re_typed() {
        assert!(type_still_says_ariane(Some("ariane")));
        // The API answers `ARIANE` on a GET and takes `ariane` on a PATCH.
        assert!(type_still_says_ariane(Some("ARIANE")));
        assert!(type_still_says_ariane(Some(" Ariane ")));

        assert!(!type_still_says_ariane(Some("other")), "already adopted");
        assert!(
            !type_still_says_ariane(Some("compass")),
            "somebody else's format is not ours to re-type"
        );
        assert!(
            !type_still_says_ariane(Some("")),
            "an absent type field is not a claim that it is Ariane"
        );
        assert!(
            !type_still_says_ariane(None),
            "a type we could not read must not be acted on: the next push reads it again"
        );
    }

    /// What a landed push relabels to `stalagmight`: an unfinished adoption
    /// (`ariane`) and the legacy managed type (`other`, which SpeleoDB never maps).
    /// Already `stalagmight`, somebody else's format, or an unreadable type: no write.
    #[test]
    fn a_landed_push_relabels_ariane_and_legacy_other_only() {
        assert!(type_needs_managed_label(Some("ariane")));
        assert!(type_needs_managed_label(Some("ARIANE")));
        assert!(type_needs_managed_label(Some("other")));
        assert!(type_needs_managed_label(Some(" OTHER ")));

        assert!(
            !type_needs_managed_label(Some("stalagmight")),
            "already right: every push after the first costs nothing"
        );
        assert!(!type_needs_managed_label(Some("STALAGMIGHT")));
        assert!(
            !type_needs_managed_label(Some("compass")),
            "somebody else's format is not ours to re-type"
        );
        assert!(!type_needs_managed_label(Some("")));
        assert!(
            !type_needs_managed_label(None),
            "a type we could not read must not be acted on"
        );
    }

    /// The PATCH body names the managed type and nothing else.
    #[test]
    fn the_managed_type_patch_is_exactly_the_type_field() {
        assert_eq!(managed_type_patch(), r#"{"type":"STALAGMIGHT"}"#);
    }

    /// The predicate and body above only matter if the writers use them. The heal
    /// gates on `type_needs_managed_label` (so a legacy `other` is relabelled, not
    /// only an unfinished `ariane`), and no SpeleoDB source writes a hand-made type
    /// body that could drift back to `other`.
    #[test]
    fn the_type_writers_go_through_the_shared_helpers() {
        let src = push_prod_source();
        let heal = src
            .find("fn speleodb_heal_managed_type")
            .expect("the heal is defined");
        let heal_body = &src[heal..];
        let heal_body = &heal_body[..heal_body.find("\n    }\n").expect("heal body ends")];
        assert!(
            heal_body.contains("if !type_needs_managed_label(server_kind)"),
            "the heal must relabel the legacy type too, not only `ariane`"
        );
        assert!(heal_body.contains("&managed_type_patch()"));
        assert_eq!(
            speleodb_prod_matches("&managed_type_patch()", &[]),
            1,
            "the type repair PATCHes through the helper"
        );
        assert_eq!(
            speleodb_prod_matches(r#"{"type":"#, &[]),
            0,
            "no hand-written type PATCH body in the SpeleoDB sources"
        );
    }

    /// The heal has to run on EVERY arm that concludes an upload landed, and "landed"
    /// is not the same set as "returned successfully" - the 304 arm concludes it too
    /// (the content is already up there, put there by an attempt whose response we may
    /// have lost). An arm that stops healing leaves the half-state standing silently,
    /// which is the failure this whole mechanism exists to end.
    ///
    /// Structural, like the sibling tripwire on the recall invalidation: reaching the
    /// real thing needs a server, and what is being guarded is someone adding a landing
    /// arm without the finishing move.
    #[test]
    fn every_landing_upload_arm_finishes_the_adoption() {
        // One definition plus the two landing arms inside `speleodb_upload`: the 200
        // and the 304. Both are points at which the `.stalx` is provably on the server.
        assert_eq!(
            speleodb_prod_matches("speleodb_heal_managed_type(", &[]),
            3,
            "expected one definition plus the 200 and 304 landing arms"
        );
        // Never BEFORE a landing: flipping ahead of one recreates the state the flip
        // exists to avoid - a project typed `stalagmight` whose only file is the Ariane one
        // that type says to disregard. Checked arm by arm rather than once, so a heal
        // added ahead of its landing cannot hide behind an earlier well-formed pair.
        let src = push_prod_source();
        let upload = src
            .find("fn speleodb_upload")
            .expect("the upload is defined");
        let helper = src
            .find("fn speleodb_heal_managed_type")
            .expect("the helper is defined");
        // Asserted, not assumed: `find` answers presence, not position, so slicing
        // straight from one to the other turns "the helper moved above the upload"
        // into a start>end slice PANIC - a real failure, reported as the wrong thing.
        assert!(
            upload < helper,
            "this tripwire slices from the upload to the helper, so the helper has to \
             be defined after it"
        );
        let body = &src[upload..helper];
        let landings: Vec<_> = body
            .match_indices("let landed = PushOutcome::Confirmed")
            .map(|(at, _)| at)
            .collect();
        let heals: Vec<_> = body
            .match_indices("self.speleodb_heal_managed_type(")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(
            landings.len(),
            heals.len(),
            "every landing the upload concludes must be followed by the finishing move"
        );
        for (landed, heal) in landings.iter().zip(&heals) {
            assert!(
                landed < heal,
                "the heal must follow the landing it is evidence of, never precede it"
            );
        }
    }

    #[test]
    fn reverify_due_forces_a_check_when_never_verified_or_unparseable() {
        // #202: a never-verified link (None) always re-verifies on the first
        // lock-held edit; an unparseable stamp fails toward re-verifying.
        let now = chrono::Utc::now();
        assert!(reverify_due(None, now), "None must force a re-verify");
        assert!(
            reverify_due(Some("not-a-timestamp"), now),
            "an unparseable stamp must force a re-verify"
        );
    }

    #[test]
    fn reverify_due_respects_the_20_minute_window() {
        // #202: within the window → not due; at/after the window → due. This bounds
        // a continuously-active editor to ~3 re-verify requests/hour.
        let now = chrono::Utc::now();
        let fresh = (now - chrono::Duration::seconds(REVERIFY_INTERVAL_SECS - 1)).to_rfc3339();
        let exact = (now - chrono::Duration::seconds(REVERIFY_INTERVAL_SECS)).to_rfc3339();
        let stale = (now - chrono::Duration::seconds(REVERIFY_INTERVAL_SECS + 1)).to_rfc3339();
        assert!(
            !reverify_due(Some(&fresh), now),
            "just under 20 min → not due"
        );
        // The predicate is `>= REVERIFY_INTERVAL_SECS`, so EXACTLY at the window is
        // due (pins the boundary so a `>` vs `>=` regression can't slip through).
        assert!(
            reverify_due(Some(&exact), now),
            "exactly at 20 min → due (boundary is inclusive)"
        );
        assert!(reverify_due(Some(&stale), now), "just over 20 min → due");
    }

    #[test]
    fn lock_held_by_me_is_case_insensitive_and_none_safe() {
        // #202: matches the ancestry-recovery email comparison (case-insensitive).
        assert!(lock_held_by_me(
            Some("Me@Example.com"),
            Some("me@example.com")
        ));
        assert!(!lock_held_by_me(
            Some("other@example.com"),
            Some("me@example.com")
        ));
        // A cleared lock (no holder) or an unknown self-email is never "ours".
        assert!(!lock_held_by_me(None, Some("me@example.com")));
        assert!(!lock_held_by_me(Some("me@example.com"), None));
        assert!(!lock_held_by_me(None, None));
    }

    #[test]
    fn clear_snapshot_dirty_clears_the_flag_in_the_copy() {
        // #197 guardrail: the push snapshot must never carry has_unpushed_edits to
        // the server - clear_snapshot_dirty wipes the dirty bit (and last_edit_at)
        // in the COPY while preserving the hand-off identity.
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("snap.stalx");
        {
            let mut db = Database::open(&path).expect("open snapshot");
            db.save_project(&Project::new("Snap".to_owned(), None))
                .expect("save project");
            db.save_speleo_link(Some(&SpeleoLink {
                endpoint: "http://x".to_owned(),
                project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
                project_name: "Snap".to_owned(),
                base_commit_sha: Some("abc".to_owned()),
                has_unpushed_edits: true,
                last_edit_at: Some("2026-06-27T10:00:00+00:00".to_owned()),
                last_verify_at: None,
                undo_mode: None,
            }))
            .expect("stamp dirty link");
        }

        clear_snapshot_dirty(&path, false, None).expect("sanitize the snapshot");

        let db = Database::open_existing(&path).expect("reopen snapshot");
        let link = db
            .load_speleo_link()
            .expect("load link")
            .expect("link present");
        assert!(
            !link.has_unpushed_edits,
            "dirty bit cleared in the snapshot copy"
        );
        assert_eq!(link.last_edit_at, None, "last_edit_at cleared too");
        // Identity is preserved - only the dirty bit / timestamp change.
        assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
        assert_eq!(link.project_id, "11111111-1111-1111-1111-111111111111");
    }

    #[test]
    fn local_display_settings_survive_a_download_replace() {
        // #197: view toggles are stripped from the server copy, so a fast-forward /
        // re-open (which replaces the whole local .stalx with the server's) must
        // carry the user's local display_settings across the overwrite - otherwise
        // their view resets to defaults on every sync.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");

        // The user's local file with their view prefs (a project row is present, as
        // in any real pulled .stalx, so open_existing's validation passes).
        {
            let mut db = Database::open(&dest).expect("open local cache");
            db.save_project(&Project::new("Cache".to_owned(), None))
                .expect("save project");
            db.save_display_setting("grid", "off").expect("save grid");
            db.save_display_setting("color_mode", "depth")
                .expect("save color_mode");
        }

        // Snapshot the prefs, then simulate the server file landing (a fresh file
        // with NO display_settings, as stripped on push).
        let preserved = preserved_display_settings(&dest);
        assert_eq!(preserved.len(), 2, "both local prefs captured");
        std::fs::remove_file(&dest).expect("remove old file");
        {
            let mut db = Database::open(&dest).expect("open replaced file");
            db.save_project(&Project::new("Cache".to_owned(), None))
                .expect("save project");
            assert!(
                db.load_display_settings().expect("load").is_empty(),
                "server copy carries no view prefs"
            );
            restore_display_settings(&db, &preserved);
        }

        // The user's view survived the replace.
        let after = Database::open_existing(&dest)
            .expect("reopen")
            .load_display_settings()
            .expect("load restored");
        assert_eq!(after.get("grid").map(String::as_str), Some("off"));
        assert_eq!(after.get("color_mode").map(String::as_str), Some("depth"));

        // A first open (no prior local file) has nothing to preserve → defaults.
        let fresh = temp.path().join("fresh.stalx");
        assert!(
            preserved_display_settings(&fresh).is_empty(),
            "first open preserves nothing"
        );
    }

    // --- closing the open project around a cache download-replace ---

    /// Write a minimal openable `.stalx` at `path` and return it.
    fn seed_stalx(path: &Path, name: &str) {
        let mut db = Database::open(path).expect("open seed db");
        db.save_project(&Project::new(name.to_owned(), None))
            .expect("save seed project");
    }

    /// Whether a project (and its database handle) is currently open in `state`.
    fn project_is_open(state: &AppState) -> bool {
        let project = state.project.lock().expect("project lock").is_some();
        let database = state.database.lock().expect("database lock").is_some();
        assert_eq!(
            project, database,
            "project and database handles should open/close together"
        );
        project
    }

    /// Install `source` over `dest` through the session-bound replace, under the
    /// session generation that is current right now (nothing moves it here).
    fn install_over(
        state: &AppState,
        source: &Path,
        dest: &Path,
        policy: LocalEditsPolicy,
    ) -> Result<std::collections::HashMap<String, String>, SpeleoDbError> {
        state.replace_cache_for_session(
            source,
            dest,
            GUARD_ID,
            policy,
            InheritedUndo::Drop,
            state.speleodb_session_generation(),
        )
    }

    fn project_name_at(path: &Path) -> String {
        Database::open_existing(path)
            .expect("open replaced file")
            .load_project()
            .expect("load replaced project")
            .name
    }

    #[test]
    fn replacing_the_open_project_closes_it() {
        // The download-replace must not leave a live mmap-enabled SQLite connection
        // on a file it renamed over (SIGBUS / torn reads on Unix, a denied rename on
        // Windows), so the install drops the handle before the rename (pinned by
        // `the_open_projects_handle_is_dropped_before_the_rename_under_a_poisoned_lock`)
        // and closes the project once the new bytes are in place.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");
        seed_linked_stalx(&dest, GUARD_ID, false);
        let native = server_stalx();

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        assert!(project_is_open(&state));

        install_over(&state, native.path(), &dest, LocalEditsPolicy::Protect)
            .expect("a clean open copy is replaced");
        assert!(
            !project_is_open(&state),
            "the open project IS this file, so it must be closed"
        );
        assert_eq!(project_name_at(&dest), "Server Copy");
    }

    #[test]
    fn replacing_a_different_path_leaves_the_open_project_alone() {
        // Conditional on purpose: a pull can target a cached project the user does
        // not have open, and dropping THAT project would throw away its undo history
        // for nothing.
        let temp = TempDir::new().expect("temp dir");
        let open_path = temp.path().join("open.stalx");
        let other = temp.path().join("other.stalx");
        seed_stalx(&open_path, "Open");
        seed_stalx(&other, "Other");
        let native = server_stalx();

        let state = AppState::new().expect("state");
        assert!(state.open_project(open_path).success, "open a project");

        install_over(&state, native.path(), &other, LocalEditsPolicy::Protect)
            .expect("the other copy is replaced");
        assert!(
            project_is_open(&state),
            "the unrelated open project must stay open"
        );
        assert_eq!(project_name_at(&other), "Server Copy");
    }

    #[test]
    fn a_first_install_leaves_the_open_project_alone() {
        // A first pull writes into an empty cache: `dest` cannot be canonicalized
        // and cannot be the file a live connection holds.
        let temp = TempDir::new().expect("temp dir");
        let open_path = temp.path().join("open.stalx");
        seed_stalx(&open_path, "Open");
        let missing = temp.path().join("not-downloaded-yet.stalx");
        let native = server_stalx();

        let state = AppState::new().expect("state");
        assert!(state.open_project(open_path).success, "open a project");

        install_over(&state, native.path(), &missing, LocalEditsPolicy::Protect)
            .expect("a first install needs nothing to replace");
        assert!(project_is_open(&state), "open project untouched");
        assert_eq!(project_name_at(&missing), "Server Copy");
    }

    #[test]
    fn replacing_the_open_project_under_a_poisoned_lock_still_closes_it() {
        // The end state of a successful replace under a poisoned mutex: project and
        // handle both gone. Poisoning only means an earlier call panicked while
        // holding the lock; the guarded value is an `Option`, and emptying it is
        // exactly the recovery. Whether the handle went BEFORE the rename cannot be
        // seen from here (Linux renames over an open file); that is pinned by
        // `the_open_projects_handle_is_dropped_before_the_rename_under_a_poisoned_lock`.
        // The policy is beside the point, so the confirmed discard keeps the guard
        // out of the way.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");
        seed_stalx(&dest, "Cache");
        let native = server_stalx();

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        assert!(project_is_open(&state));

        // Poison the database lock the way a real panic would: panic while its
        // guard is held, and swallow the unwind so the test itself survives.
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.database.lock().expect("database lock");
            panic!("simulated panic while holding the database lock");
        }));
        assert!(poisoned.is_err(), "the closure must have panicked");
        assert!(
            state.database.is_poisoned(),
            "the database lock must now be poisoned"
        );

        install_over(&state, native.path(), &dest, LocalEditsPolicy::Discard)
            .expect("the open copy is replaced, poisoned lock or not");
        assert!(
            state
                .database
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none(),
            "the live database handle must be gone, poisoned or not"
        );
        assert!(
            state
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none(),
            "the project must be dropped too"
        );
        assert_eq!(project_name_at(&dest), "Server Copy");
    }

    #[test]
    fn replacing_the_open_project_matches_through_a_non_canonical_spelling() {
        // The open path and `dest` are frequently derived differently (cache-path
        // join vs. the FE's opened path), so the comparison canonicalizes.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");
        seed_linked_stalx(&dest, GUARD_ID, false);
        let round_about = temp.path().join("sub").join("..").join("cache.stalx");
        std::fs::create_dir(temp.path().join("sub")).expect("mkdir sub");
        let native = server_stalx();

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        install_over(
            &state,
            native.path(),
            &round_about,
            LocalEditsPolicy::Protect,
        )
        .expect("the same file spelled differently is replaced");
        assert!(
            !project_is_open(&state),
            "the same file spelled differently must still match"
        );
        assert_eq!(project_name_at(&dest), "Server Copy");
    }

    #[test]
    fn the_session_replace_closes_preserves_and_swaps_atomically() {
        // The chokepoint the pull goes through: preserve the local view toggles,
        // drop the live handle to the destination, then swap in the downloaded copy
        // via temp+rename.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");
        seed_linked_stalx(&dest, GUARD_ID, false);
        Database::open(&dest)
            .expect("open local cache")
            .save_display_setting("grid", "off")
            .expect("save grid");

        let native = server_stalx();

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");

        let preserved = install_over(&state, native.path(), &dest, LocalEditsPolicy::Protect)
            .expect("replace the cache file");

        assert_eq!(
            preserved.get("grid").map(String::as_str),
            Some("off"),
            "local view toggles are captured before the replace"
        );
        assert!(
            !project_is_open(&state),
            "the live handle is dropped when the file is replaced"
        );
        assert_eq!(
            project_name_at(&dest),
            "Server Copy",
            "the destination now holds the downloaded copy"
        );
        let leftovers: Vec<_> = std::fs::read_dir(temp.path())
            .expect("readdir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the atomic swap must leave no temp file behind, found {leftovers:?}"
        );
    }

    // --- the un-pushed-edits guard in front of every download-replace ---

    /// A valid project id for the guard fixtures (the guard never parses it, but
    /// the surrounding code validates ids, so keep it well-formed).
    const GUARD_ID: &str = "44444444-4444-4444-4444-444444444444";

    /// Write a cache `.stalx` at `path` carrying a SpeleoDB link for `project_id`
    /// with the given dirty bit.
    fn seed_linked_stalx(path: &Path, project_id: &str, dirty: bool) {
        let mut db = Database::open(path).expect("open seed db");
        db.save_project(&Project::new("Cached".to_owned(), None))
            .expect("save seed project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://speleo.test/".to_owned(),
            project_id: project_id.to_owned(),
            project_name: "Cached".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: dirty,
            last_edit_at: dirty.then(|| "2026-07-26T09:00:00Z".to_owned()),
            last_verify_at: None,
            // Current linked fixtures carry an explicit project policy. Legacy NULL
            // behavior is covered separately by the undo-policy migration tests.
            undo_mode: Some(crate::models::UndoMode::Full),
        }))
        .expect("save link");
    }

    /// A native server project whose `.stalx` payload is recognisable on disk.
    fn server_stalx() -> tempfile::NamedTempFile {
        let temp = TempDir::new().expect("server project temp dir");
        let project = temp.path().join("server.stalx");
        seed_stalx(&project, "Server Copy");
        let native = tempfile::NamedTempFile::new().expect("native file");
        std::fs::copy(&project, native.path()).expect("copy native file");
        native
    }
    /// [#303] The discard verb destroys un-pushed work with no way back, and until now it
    /// validated nothing but a link and a token - so the backend could not tell a
    /// confirmed discard from a stray call. `acknowledged` is the caller reporting that it
    /// showed the loss, the same shape as `speleodb_guard_unwind`'s `discard_unpushed`.
    ///
    /// It is NOT a lock or read-only gate, which was considered and rejected: a 401
    /// auto-drop leaves the session read-only while the copy still holds un-pushed work,
    /// and discard is the way out of exactly that state.
    #[test]
    fn discarding_the_local_copy_needs_the_callers_acknowledgement() {
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("discard.stalx");
        seed_linked_stalx(&dest, GUARD_ID, true);
        let state = AppState::new().expect("state");
        assert!(state.open_project(dest).success, "open the linked copy");

        let refused = state
            .speleodb_discard_changes(true, false)
            .expect_err("an unacknowledged discard must be refused");
        assert!(
            refused.message.contains("confirmed"),
            "the refusal must say what is missing, got {:?}",
            refused.message
        );

        // ...and the acknowledgement is the ONLY thing that refusal was about: with it
        // set, the same call gets as far as the session token (absent in this offline
        // fixture) instead. Without this half, deleting the check would leave the test
        // passing on whatever error came next.
        let acknowledged = state
            .speleodb_discard_changes(true, true)
            .expect_err("offline fixture: there is no session token to reach the server with");
        assert_ne!(
            acknowledged.message, refused.message,
            "an acknowledged discard must get past the acknowledgement gate"
        );
    }

    #[test]
    fn guard_refuses_a_cached_copy_that_has_unpushed_edits() {
        // The data-loss hole: re-opening a project from the SpeleoDB list re-pulls
        // over the cached copy. When that copy still holds edits the server has
        // never seen, the replace is refused - un-pushed work exists in no git
        // anywhere, so overwriting it cannot be undone.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, GUARD_ID, true);
        let state = AppState::new().expect("state");

        let err = state
            .guard_local_edits(GUARD_ID, &dest)
            .expect_err("a dirty local copy must not be overwritten");

        // Typed, and the project + the file that holds the work are identifiable -
        // the frontend needs both to offer keep-local vs take-remote.
        let refusal = match err {
            SpeleoDbError::WouldDiscardLocalEdits {
                project_id,
                local_path,
                last_edit_at,
            } => Some((project_id, local_path, last_edit_at)),
            _ => None,
        };
        let (project_id, local_path, last_edit_at) =
            refusal.expect("expected the un-pushed-edits refusal");
        assert_eq!(project_id, GUARD_ID, "the refusal names the project");
        assert_eq!(
            local_path,
            dest.to_string_lossy(),
            "and the copy 'keep local' would open"
        );
        assert_eq!(
            last_edit_at.as_deref(),
            Some("2026-07-26T09:00:00Z"),
            "and when the work was last touched"
        );
    }

    #[test]
    fn refusal_crosses_ipc_as_a_typed_routing_signal() {
        // Not a hard error but a routing signal (like AdoptAriane): the frontend
        // branches on `action` and reads the payload, never on message text.
        let dto = local_dto(SpeleoDbError::WouldDiscardLocalEdits {
            project_id: GUARD_ID.to_owned(),
            local_path: "/cache/speleodb/p/project.stalx".to_owned(),
            last_edit_at: Some("2026-07-26T09:00:00Z".to_owned()),
        });
        assert_eq!(
            dto.action,
            crate::speleodb::SpeleoDbAction::UnpushedLocalEdits
        );
        assert_eq!(dto.status, None, "a local guard has no HTTP status");
        let payload = dto
            .unpushed_local
            .expect("the refusal carries what the choice needs");
        assert_eq!(payload.project_id, GUARD_ID);
        assert_eq!(payload.local_path, "/cache/speleodb/p/project.stalx");
        assert_eq!(
            payload.last_edit_at.as_deref(),
            Some("2026-07-26T09:00:00Z")
        );
    }

    #[test]
    fn guard_allows_a_clean_or_absent_local_copy() {
        // No behaviour change when there is nothing to lose: a clean cached copy,
        // an unmanaged `.stalx` with no link row, and a first pull into an empty
        // cache all replace exactly as before.
        let temp = TempDir::new().expect("temp dir");
        let state = AppState::new().expect("state");

        let clean = temp.path().join("clean.stalx");
        seed_linked_stalx(&clean, GUARD_ID, false);
        assert!(state.guard_local_edits(GUARD_ID, &clean).is_ok());

        let unlinked = temp.path().join("unlinked.stalx");
        seed_stalx(&unlinked, "Unlinked");
        assert!(state.guard_local_edits(GUARD_ID, &unlinked).is_ok());

        let missing = temp.path().join("not-pulled-yet.stalx");
        assert!(
            state.guard_local_edits(GUARD_ID, &missing).is_ok(),
            "a first pull has nothing to protect"
        );
    }

    #[test]
    fn guard_refuses_a_local_copy_that_is_present_but_unreadable() {
        // The guard used to fail OPEN here: any read error mapped to
        // `has_unpushed_edits: false`, so "I cannot read this file" was recorded as
        // "this file is clean" and the server copy was pulled straight over it.
        //
        // The realistic causes are exactly the ones where work is recoverable: a hot
        // journal after a kill or power loss, which a READ-ONLY connection cannot roll
        // back (`SQLITE_READONLY_ROLLBACK`) though a read-write open would, and a
        // transient `SQLITE_BUSY` while another writer is mid-commit. Refusing is
        // recoverable (the prompt offers keep-local, or an explicit take-remote);
        // overwriting is not.
        let temp = TempDir::new().expect("temp dir");
        let unreadable = temp.path().join("unreadable.stalx");
        std::fs::write(&unreadable, b"not a sqlite database at all").expect("write junk");
        let state = AppState::new().expect("state");

        let err = state
            .guard_local_edits(GUARD_ID, &unreadable)
            .expect_err("an unreadable local copy must not be treated as clean");
        assert!(
            matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }),
            "expected the un-pushed-edits refusal, got {err:?}"
        );
    }

    #[test]
    fn a_failed_staging_leaves_the_open_project_open() {
        // A failed install must not leave the project closed backend-side while the
        // frontend still displays it - every later command, auto-save included, would
        // then fail with "no project open". The install stages to a temp before it
        // touches the open project, so a staging failure leaves `dest` untouched and
        // never closes the project. A failure AFTER the handle is dropped is pinned by
        // `a_failed_sidecar_move_reopens_the_open_projects_database`.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, GUARD_ID, false);
        let before = std::fs::read(&dest).expect("read seeded cache");

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        assert!(project_is_open(&state));

        let err = install_over(
            &state,
            &temp.path().join("missing.stalx"),
            &dest,
            LocalEditsPolicy::Discard,
        )
        .expect_err("staging a missing download must fail");
        assert!(
            !matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }),
            "the failure here is the staging, not the guard: {err:?}"
        );

        assert_eq!(
            std::fs::read(&dest).expect("read cache after the failure"),
            before,
            "a failed install leaves the old copy intact"
        );
        assert!(
            project_is_open(&state),
            "and the open project stays open, so the shell is not left showing a \
             project the backend does not have open"
        );
    }

    /// Make the install's sidecar step fail: an inert `-shm` beside `dest`, and a
    /// non-empty directory where it would be parked, so moving it aside cannot succeed.
    /// That step runs after the open project's database handle has been dropped and
    /// before the rename, which is exactly the window the tests below need to observe.
    fn block_the_sidecar_move(dest: &Path) {
        let shm = std::path::PathBuf::from(format!("{}-shm", dest.display()));
        std::fs::write(&shm, b"").expect("inert shm sidecar");
        let parked = std::path::PathBuf::from(format!("{}-shm.set-aside", dest.display()));
        std::fs::create_dir(&parked).expect("obstacle where the sidecar would be parked");
        std::fs::write(parked.join("keep"), b"x").expect("non-empty obstacle");
    }

    #[test]
    fn a_failed_sidecar_move_reopens_the_open_projects_database() {
        // The install drops the open project's database handle before it moves the
        // sidecars and renames (a live handle must not survive into the rename). A
        // failure in between must reopen the ORIGINAL file, or the project stays open
        // in memory with no database behind it.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, GUARD_ID, false);
        let native = server_stalx();

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        let before = std::fs::read(&dest).expect("read cache");
        block_the_sidecar_move(&dest);

        let err = install_over(&state, native.path(), &dest, LocalEditsPolicy::Discard)
            .expect_err("a sidecar that cannot be moved aside fails the install");
        assert!(
            err.to_string().contains("could not move"),
            "the failure is the sidecar move, got {err:?}"
        );
        assert_eq!(
            std::fs::read(&dest).expect("read cache after the failure"),
            before,
            "the original bytes are still in place"
        );
        assert!(
            project_is_open(&state),
            "the project AND its reopened database are both there again"
        );
        assert!(
            std::path::PathBuf::from(format!("{}-shm", dest.display())).exists(),
            "and the sidecar is back beside the original"
        );
    }

    /// Unix only: it makes the original unreadable to observe the handle, and only unix
    /// permissions make a file unopenable to its owner.
    #[cfg(unix)]
    #[test]
    #[expect(
        clippy::print_stderr,
        reason = "say so when a privileged run has to skip the check"
    )]
    fn the_open_projects_handle_is_dropped_before_the_rename_under_a_poisoned_lock() {
        // A drop written as `if let Ok(..)` does nothing on a poisoned mutex, and the
        // install then renames a new file over a live SQLite handle and its 256 MB mmap
        // (SIGBUS / torn reads on Unix, a denied rename on Windows). Linux renames over
        // an open file anyway, so the end state of a successful install cannot show
        // whether the handle went first. This stops the install between the drop and
        // the rename instead, with the original made unreopenable: the slot can only
        // be empty afterwards if the drop really happened.
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("project.stalx");
        seed_stalx(&dest, "Cache");
        let native = server_stalx();

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.database.lock().expect("database lock");
            panic!("simulated panic while holding the database lock");
        }));
        assert!(poisoned.is_err() && state.database.is_poisoned());

        block_the_sidecar_move(&dest);
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o000))
            .expect("make the original unreadable");
        if std::fs::File::open(&dest).is_ok() {
            // Privileged runs (root in a container) ignore the mode, so the reopen
            // would succeed and nothing could be observed.
            eprintln!("skipped: this user can open a mode-000 file");
            return;
        }

        let err = install_over(&state, native.path(), &dest, LocalEditsPolicy::Discard)
            .expect_err("the sidecar move fails, and so does the reopen");
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644))
            .expect("restore the mode for cleanup");
        assert!(
            err.to_string().contains("could not be reopened"),
            "the reopen of the original was attempted and refused, got {err:?}"
        );
        assert!(
            state
                .database
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none(),
            "the live handle was dropped before the rename, poisoned lock or not"
        );
    }

    #[test]
    fn the_live_open_link_vetoes_its_own_file_and_only_its_own_file() {
        // The live in-memory link is consulted because it can be fresher than a file
        // on disk - but it may only speak for the file it IS.
        //
        // UPDATED: this used to assert the opposite half, that a dirty OPEN copy
        // vetoes a replace of a DIFFERENT, clean copy of the same project. It cannot
        // legitimately: replacing `cached` destroys nothing that lives in `open_path`,
        // and the shape is reachable in normal use (Save-As carries the `speleo_link`
        // row forward, so a user can keep their own copy of P open while re-opening P
        // from the browser). The refusal it produced named the CACHE path and the
        // OTHER file's edit time - describing work living in a file it did not name -
        // and its "take the server copy and discard my local changes" route would have
        // destroyed the clean cache copy while leaving the dirty edits untouched. It
        // also blocked every pull, fast-forward and revision-restore on P for as long
        // as that unrelated copy stayed dirty.
        let temp = TempDir::new().expect("temp dir");
        // A clean copy in the cache...
        let cached = temp.path().join("cached.stalx");
        seed_linked_stalx(&cached, GUARD_ID, false);
        // ...and the OPEN copy of the same project, elsewhere, carrying un-pushed work.
        let open_path = temp.path().join("open.stalx");
        seed_linked_stalx(&open_path, GUARD_ID, true);

        let state = AppState::new().expect("state");
        assert!(
            state.open_project(open_path.clone()).success,
            "open the dirty copy"
        );

        let err = state
            .guard_local_edits(GUARD_ID, &open_path)
            .expect_err("the open copy's own un-pushed edits must veto a replace of it");
        assert!(
            matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }),
            "expected the un-pushed-edits refusal, got {err:?}"
        );

        assert!(
            state.guard_local_edits(GUARD_ID, &cached).is_ok(),
            "but a different, clean copy of the same project holds no work to lose, \
             so replacing IT must not be refused on another file's behalf"
        );
    }

    #[test]
    fn protected_replace_refuses_and_discarding_replace_still_overwrites() {
        // The two policies, on the same dirty destination: `Protect` leaves the file
        // byte-identical, and the explicitly-named `Discard` (take-remote, behind the
        // frontend's destroy-confirmation) still works.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, GUARD_ID, true);
        let before = std::fs::read(&dest).expect("read seeded cache");
        let native = server_stalx();
        let state = AppState::new().expect("state");

        let refused = install_over(&state, native.path(), &dest, LocalEditsPolicy::Protect)
            .expect_err("the protected replace must refuse");
        assert!(matches!(
            refused,
            SpeleoDbError::WouldDiscardLocalEdits { .. }
        ));
        assert_eq!(
            std::fs::read(&dest).expect("read cache after the refusal"),
            before,
            "a refused replace must not touch the file at all"
        );

        install_over(&state, native.path(), &dest, LocalEditsPolicy::Discard)
            .expect("the confirmed discard still overwrites on purpose");
        let replaced = Database::open_existing(&dest).expect("open replaced file");
        assert_eq!(
            replaced.load_project().expect("load replaced project").name,
            "Server Copy",
            "the confirmed-discard path keeps working"
        );
    }

    // --- head_with_empty_retry (#192 re-fetch-once on a transient empty tip) ---

    #[test]
    fn head_retries_once_then_succeeds_on_transient_empty() {
        let calls = std::cell::Cell::new(0u32);
        // The settled read returns a realistic shape: a resolved tip always rides a
        // non-empty history (tip_of(&[]) is None), so the success case carries a
        // commit - not the unreachable (Some, empty) pair (#197 review).
        let settled = vec![crate::speleodb::models::RevisionCommit {
            id: "tip-sha".to_owned(),
            parent_ids: vec!["base-sha".to_owned()],
            author_email: "me@x".to_owned(),
            authored_date: None,
            message: None,
        }];
        let (tip, _) = head_with_empty_retry(Some("base-sha"), || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Ok((None, Vec::new())) // transient empty read
            } else {
                Ok((Some("tip-sha".to_owned()), settled.clone()))
            }
        })
        .expect("ok");
        assert_eq!(
            calls.get(),
            2,
            "re-fetched exactly once on (base Some, empty history)"
        );
        assert_eq!(
            tip.as_deref(),
            Some("tip-sha"),
            "the second read's tip wins"
        );
    }

    #[test]
    fn head_fails_closed_after_single_retry_when_still_empty() {
        let calls = std::cell::Cell::new(0u32);
        let (tip, _) = head_with_empty_retry(Some("base-sha"), || {
            calls.set(calls.get() + 1);
            Ok((None, Vec::new()))
        })
        .expect("ok");
        assert_eq!(calls.get(), 2, "exactly one retry, never loops");
        assert!(
            tip.is_none(),
            "still empty -> the caller fences fail-closed"
        );
    }

    #[test]
    fn head_no_retry_for_empty_repo_when_base_none() {
        let calls = std::cell::Cell::new(0u32);
        let (tip, _) = head_with_empty_retry(None, || {
            calls.set(calls.get() + 1);
            Ok((None, Vec::new()))
        })
        .expect("ok");
        assert_eq!(
            calls.get(),
            1,
            "(base None, tip None) is the valid empty repo -> no retry"
        );
        assert!(tip.is_none());
    }

    #[test]
    fn head_no_retry_when_tip_present() {
        // Realistic shape: a resolved tip always rides a non-empty history. The retry
        // keys on the commit list, so a present history → no re-fetch.
        let calls = std::cell::Cell::new(0u32);
        let history = vec![crate::speleodb::models::RevisionCommit {
            id: "tip-sha".to_owned(),
            parent_ids: vec!["base-sha".to_owned()],
            author_email: "me@x".to_owned(),
            authored_date: None,
            message: None,
        }];
        let (tip, _) = head_with_empty_retry(Some("base-sha"), || {
            calls.set(calls.get() + 1);
            Ok((Some("tip-sha".to_owned()), history.clone()))
        })
        .expect("ok");
        assert_eq!(calls.get(), 1, "tip present -> no retry");
        assert_eq!(tip.as_deref(), Some("tip-sha"));
    }

    #[test]
    fn head_no_retry_on_fork() {
        // #197 review: a multi-leaf FORK also yields tip None, but it is NOT a
        // transient empty read - it must fail closed immediately, never burning a
        // second GET. Keyed on the (non-empty) commit list, not tip.is_none().
        let calls = std::cell::Cell::new(0u32);
        let fork = vec![
            crate::speleodb::models::RevisionCommit {
                id: "a".to_owned(),
                parent_ids: vec![],
                author_email: "me@x".to_owned(),
                authored_date: None,
                message: None,
            },
            crate::speleodb::models::RevisionCommit {
                id: "b".to_owned(),
                parent_ids: vec![],
                author_email: "me@x".to_owned(),
                authored_date: None,
                message: None,
            },
        ];
        let (tip, commits) = head_with_empty_retry(Some("base-sha"), || {
            calls.set(calls.get() + 1);
            Ok((None, fork.clone()))
        })
        .expect("ok");
        assert_eq!(calls.get(), 1, "a fork must fail closed without a re-fetch");
        assert!(tip.is_none());
        assert_eq!(commits.len(), 2);
    }

    #[test]
    fn base_commit_sha_advances_only_at_sanctioned_sites() {
        // Tripwire (mirrors the MUTATION_ENTRYPOINTS guard): BASE may change ONLY
        // via the sanctioned triggers. If a count below changes, a base write was
        // added/removed - confirm the new site is a legitimate trigger before
        // updating it. (#192/#193/#197)
        //
        //   mutate `l.base_commit_sha =` (inside speleodb_mutate_link closures):
        //     1. push 200 confirmed - advance to the adopted hexsha (never BACKWARD
        //        to the empty-repo sentinel: a None there would fence the next push
        //        to Diverged against our own landed commit)
        //     2. push ancestry Landed - advance BASE to tip
        //     3. push ancestry Landed - already-up-to-date sub-case
        //     4. speleodb_publish recovery - adopt the server tip when the
        //        best-effort initial push did not confirm, so a just-published
        //        project does not open the divergence prompt on its first edit
        //   struct-init `base_commit_sha:` (a fresh SpeleoLink):
        //     1. speleodb_open_project - stamp BASE on pull
        //     2. speleodb_fast_forward - re-stamp BASE = tip after a clean FF
        //     3. speleodb_publish - stamp BASE = the create response's latest_commit
        //        (None for a brand-new repo) when minting the link for a published
        //        local project (#207)
        //     4. speleodb_adopt_ariane - stamp BASE = the server tip when minting the
        //        link for an adopted Ariane project (#214); same trigger as pull.
        //     5. speleodb_restore_revision - stamp BASE = the CURRENT server tip (NOT
        //        the restored revision) when re-minting the link on a revision restore
        //        (#218); the restored tree rides as an unpushed edit on top of HEAD.
        //
        // Counted across the WHOLE speleodb family, not just this file: BASE writes
        // that move to a sibling module must keep being counted here, or the split
        // that moved them would silently retire the guard (#275).
        assert_eq!(
            speleodb_prod_matches("base_commit_sha =", &[]),
            4,
            "unexpected BASE mutate-advance count (sanctioned: push-200, ancestry-landed x2, publish tip recovery)"
        );
        assert_eq!(
            speleodb_prod_matches("base_commit_sha:", &[]),
            4,
            "unexpected BASE stamp count (sanctioned: open-on-pull, fast-forward restamp, publish-create, revision-restore)"
        );
    }

    #[test]
    fn push_flushes_to_disk_before_snapshotting() {
        // #195 quiescent-point invariant: the push must flush in-memory edits to the
        // live .stalx (save_project) BEFORE taking the VACUUM snapshot, or the
        // uploaded snapshot would miss the last edit (data loss). The snapshot - NOT
        // the live bytes - is then what upload_stalx streams. The ordering can't be
        // observed without a client seam, so pin it structurally (like the
        // BASE-advance tripwire); the live round-trips prove the behaviour itself.
        let src = push_prod_source();
        let upload = src
            .split_once("fn speleodb_upload")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n    fn ").next())
            .expect("speleodb_upload body precedes the next method");
        let save = upload
            .find(".save_project_at_generation(")
            .expect("push flushes via save_project");
        let vacuum = upload
            .find("vacuum_snapshot(")
            .expect("push snapshots via vacuum_snapshot");
        let upload_call = upload
            .find("upload_stalx(")
            .expect("push streams via upload_stalx");
        assert!(
            save < vacuum,
            "save_project (flush) must precede vacuum_snapshot - the snapshot must be quiescent (#195)"
        );
        assert!(
            vacuum < upload_call,
            "the VACUUM snapshot must be taken before upload_stalx streams it (#195)"
        );
    }

    #[test]
    fn forget_keeps_local_project_editable() {
        // #197 review: speleodb_forget is the dead-token auto-drop sink - it runs on a
        // 401 from ANY SpeleoDB call (e.g. a background list_projects/refresh). It must
        // NOT turn an open LOCAL project read-only; only a remote edit session loses
        // the gate. (No remote link open here → open_link Err → gate untouched.)
        let state = AppState::new().expect("state");
        assert!(state.edit_enabled(), "local default-true");
        state.speleodb_forget();
        assert!(
            state.edit_enabled(),
            "a 401 auto-drop must not disable editing on a local-only project"
        );
    }

    #[test]
    fn forget_drops_gate_on_remote_edit_session() {
        // ...but a remote edit session (gate on + an open hand-off link) DOES lose the
        // gate, so a token-less session can't keep mutating a project it can't push.
        let temp = TempDir::new().expect("temp");
        let path = temp.path().join("remote.stalx");
        let mut db = Database::open(&path).expect("open db");
        db.save_project(&Project::new("Remote".to_owned(), None))
            .expect("save project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://x/".to_owned(),
            project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        }))
        .expect("save link");
        drop(db);

        let state = AppState::new().expect("state");
        assert!(state.open_project(path).success, "open remote fixture");
        assert!(!state.edit_enabled(), "remote opens read-only");
        state.set_edit_enabled(true); // simulate Start editing acquiring the lock
        state.speleodb_forget();
        assert!(
            !state.edit_enabled(),
            "forget must drop the edit gate on a remote edit session"
        );
    }

    /// A read-only remote fixture (saved project + `speleo_link`) opened in a fresh
    /// `AppState`. Mirrors `forget_drops_gate_on_remote_edit_session`; the config
    /// opt-in is left OFF (a test enables it when needed). #213.
    fn remote_readonly_fixture() -> (TempDir, AppState) {
        remote_fixture_with_dirty_bit(false)
    }

    /// [`remote_readonly_fixture`] with the persisted dirty bit set either way, for
    /// the gate-closed-BUT-DIRTY case (a 401 auto-drop closes the edit gate and
    /// deliberately leaves `has_unpushed_edits` alone).
    fn remote_fixture_with_dirty_bit(dirty: bool) -> (TempDir, AppState) {
        let temp = TempDir::new().expect("temp");
        let path = temp.path().join("remote.stalx");
        let mut db = Database::open(&path).expect("open db");
        db.save_project(&Project::new("Remote".to_owned(), None))
            .expect("save project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://x/".to_owned(),
            project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: dirty,
            last_edit_at: dirty.then(|| "2026-07-27T10:00:00Z".to_owned()),
            last_verify_at: None,
            undo_mode: None,
        }))
        .expect("save link");
        drop(db);

        let state = AppState::new().expect("state");
        assert!(state.open_project(path).success, "open remote fixture");
        assert!(!state.edit_enabled(), "remote opens read-only");

        (temp, state)
    }

    // --- F1: a closed edit gate is NOT a successful push / release ---------------

    #[test]
    fn push_and_done_report_not_editing_when_the_gate_is_closed_over_dirty_work() {
        // The hole: `edit_enabled()` can be FALSE while `has_unpushed_edits` is still
        // TRUE - a 401 auto-drop runs `speleodb_forget`, which closes the gate and
        // deliberately keeps the dirty bit so the work is not forgotten. Both verbs
        // short-circuited to Pushed / Released there, which the frontend reads as "the
        // work is on the server": it cleared its own `hasUnpushedEdits` and thereby
        // disarmed the quit guard, over edits nothing had ever uploaded.
        //
        // The offline fixture has no endpoint, so an `Ok` also proves the answer was
        // reached with no server call at all.
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        assert!(
            !state.edit_enabled() && state.speleodb_open_link().expect("link").has_unpushed_edits,
            "precondition: gate closed, project still dirty"
        );

        let pushed = state
            .speleodb_push_only(None)
            .expect("push is a local no-op");
        assert!(
            matches!(pushed, PushResult::NotEditing),
            "a closed gate over dirty work must NOT report Pushed, got {pushed:?}"
        );
        let done = state.speleodb_done(None).expect("done is a local no-op");
        assert!(
            matches!(done, DoneResult::NotEditing),
            "a closed gate over dirty work must NOT report Released, got {done:?}"
        );

        // And nothing was quietly cleared while reporting it: the work is still here.
        assert!(
            state.speleodb_open_link().expect("link").has_unpushed_edits,
            "the dirty bit survives the refusal - it is the work"
        );
    }

    #[test]
    fn push_and_done_report_not_editing_when_the_gate_is_closed_over_clean_work() {
        // Same arm even with nothing to push: "there was no edit session" is a fact
        // about the session, not about the dirty bit, and the frontend needs it to
        // route the user back through Start editing rather than showing a success.
        let (_temp, state) = remote_readonly_fixture();
        assert!(matches!(
            state.speleodb_push_only(None).expect("push"),
            PushResult::NotEditing
        ));
        assert!(matches!(
            state.speleodb_done(None).expect("done"),
            DoneResult::NotEditing
        ));
    }

    #[test]
    fn push_with_the_gate_open_and_nothing_dirty_is_still_a_true_no_op() {
        // The case that legitimately stays `Pushed`: the edit session IS live (lock
        // held) and there is simply nothing to upload, so "your work is on the server"
        // is true. This is the distinction the fix rests on, so it is pinned from both
        // sides. Reached with no server call - a real push would need the endpoint the
        // offline fixture does not have.
        let (_temp, state) = remote_readonly_fixture();
        state.set_edit_enabled(true);
        let pushed = state
            .speleodb_push_only(None)
            .expect("nothing to push is a no-op");
        assert!(
            matches!(pushed, PushResult::Pushed { .. }),
            "an open gate with a clean link is a genuine no-op, got {pushed:?}"
        );
    }

    /// An authenticated session pointed at `endpoint` (the loopback stub below, or
    /// any unreachable host), so the release paths get past `speleodb_active_token`.
    fn authed_session_at(endpoint: &str) -> SpeleoDbSession {
        let mut token_store = TokenStore::default();
        token_store.store(endpoint, "u@x", "tok-1");
        SpeleoDbSession {
            endpoint: Some(endpoint.to_owned()),
            email: Some("u@x".to_owned()),
            token_store,
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
        }
    }

    /// The project id `remote_readonly_fixture` links to.
    const REMOTE_ID: &str = "11111111-1111-1111-1111-111111111111";

    fn editing_fixture(endpoint: &str) -> (TempDir, AppState) {
        let (temp, state) = remote_readonly_fixture();
        *state.speleodb.lock().expect("session lock") = authed_session_at(endpoint);
        let mut link = state.speleodb_open_link().expect("link");
        link.endpoint = endpoint.to_owned();
        state
            .database
            .lock()
            .expect("database")
            .as_ref()
            .expect("open")
            .save_speleo_link(Some(&link))
            .expect("save link");
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));
        (temp, state)
    }

    /// A one-shot loopback HTTP server that answers a single request with 200 and a
    /// minimal project body - enough for `release_lock` to parse a success.
    fn release_stub(project_id: &str) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let body = format!(r#"{{"id":"{project_id}","name":"Remote"}}"#);
        let handle = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Drain the request until the socket goes quiet, then answer.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
            let mut chunk = vec![0u8; 8 * 1024];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
            }
            let body = crate::core::state::tests_support::sdk_fixture(&body);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    #[test]
    fn lifecycle_release_of_a_clean_session_preserves_the_local_file() {
        // Over UN-PUSHED edits these keep the lock instead (the invariant in
        // `speleodb_lock_release`, pinned by its own tests); a clean session is
        // released and its file left exactly as it was.
        for release in [
            |state: &AppState| {
                let _ = state.speleodb_release_held_lock_on_close();
            },
            AppState::speleodb_release_held_lock_detached,
            |state: &AppState| {
                state.speleodb_release_held_lock_bounded(std::time::Duration::from_secs(5));
            },
        ] {
            let (base, seen) =
                crate::core::state::tests_support::lock_server(REMOTE_ID, Some("u@x".into()));
            let (_temp, state) = editing_fixture(&base);
            let path = state
                .get_project_state()
                .expect("project")
                .file_path
                .expect("path");
            let before = std::fs::read(&path).expect("saved bytes");
            release(&state);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while crate::core::state::tests_support::release_posts(&seen) == 0
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert_eq!(
                crate::core::state::tests_support::release_posts(&seen),
                1,
                "the release reached the server"
            );
            assert_eq!(std::fs::read(&path).expect("file preserved"), before);
            assert!(state
                .speleodb_open_link()
                .expect("link preserved")
                .base_commit_sha
                .is_some());
            assert!(!state.speleodb_lock_acquired_here(REMOTE_ID));
            assert!(!state.edit_enabled(), "the claim, and so the gate, is gone");
        }
    }

    #[test]
    fn lifecycle_release_can_finish_an_owned_claim_after_the_edit_gate_closes() {
        let (base, seen) =
            crate::core::state::tests_support::lock_server(REMOTE_ID, Some("u@x".into()));
        let (_temp, state) = editing_fixture(&base);
        state.set_edit_enabled(false);
        assert_eq!(
            state
                .speleodb_release_held_lock_on_close()
                .expect("released"),
            Some(crate::core::LockReleaseOutcome::Released)
        );
        assert_eq!(crate::core::state::tests_support::release_posts(&seen), 1);
        assert!(!state.speleodb_lock_acquired_here(REMOTE_ID));
    }

    #[test]
    fn lifecycle_release_does_not_release_unowned_or_foreign_locks() {
        let (_temp, state) = editing_fixture("https://x/");
        state.set_edit_enabled(false);
        state.speleodb_forget_lock_acquired_for(REMOTE_ID);
        assert!(
            matches!(state.speleodb_release_held_lock_on_close(), Ok(None)),
            "no claim: nothing to release, and nothing to report"
        );
        state.set_edit_enabled(true);
        *state.speleodb.lock().expect("session") = authed_session_at("https://another.example/");
        assert!(
            matches!(state.speleodb_release_held_lock_on_close(), Ok(None)),
            "another deployment's session accounts for no lock here"
        );
    }

    #[test]
    fn silent_lock_release_is_skipped_when_the_project_has_unpushed_edits() {
        // Logout keeps its unpublished-work veto - and so, now, do the lifecycle
        // transitions (see `no_lifecycle_release_hands_the_lock_back_over_unpushed_edits`).
        let (_temp, state) = editing_fixture("https://x/");
        assert!(
            state.speleodb_held_lock_release_params().is_some(),
            "a clean edit session still releases its lock on logout"
        );

        state.mark_remote_dirty();
        assert!(
            state.speleodb_open_link().expect("link").has_unpushed_edits,
            "fixture is now dirty"
        );
        assert!(
            state.speleodb_held_lock_release_params().is_none(),
            "un-pushed edits veto the silent release"
        );
        assert!(
            state.speleodb_abandon_lock_release_params().is_some(),
            "the abandon variant (unlink / take-remote) is NOT vetoed - both verbs \
             are only ever reached with un-pushed edits"
        );
    }

    #[test]
    fn a_release_is_decided_by_the_claim_never_by_the_gate() {
        // The gate is a function of the claim (`speleodb_edit_gate`), so a release
        // asks the claim directly. A session whose gate a lost-lock signal or a
        // logout's boundary has already shut still holds the claim a release has to
        // end; a read-only open without one has nothing to release at all.
        let (_temp, state) = editing_fixture("https://x/");
        state.set_edit_enabled(false);
        assert!(
            state.speleodb_held_lock_release_params().is_some(),
            "a shut gate over a claim still releases (holder-checked)"
        );
        state.speleodb_forget_lock_acquired_for(REMOTE_ID);
        assert!(
            state.speleodb_held_lock_release_params().is_none(),
            "a read-only open with no claim holds no lock to release"
        );
        state.set_edit_enabled(true);
        assert!(
            state.speleodb_held_lock_release_params().is_some(),
            "and a clean, lock-held session is unaffected by the veto"
        );
    }

    #[test]
    fn an_explicit_release_still_works_with_unpushed_edits() {
        // The veto is on the SILENT paths only. `speleodb_release_lock` is the
        // user's own "release anyway" (the held-lock prompt's per-project Release,
        // and the quit prompt's discard-and-release), so it must still go through
        // with the dirty bit set - it never consults the silent gate.
        let link_id = "11111111-1111-1111-1111-111111111111";
        let (base, handle) = release_stub(link_id);
        let (_temp, state) = editing_fixture(&base);
        state.mark_remote_dirty();
        assert!(
            state.speleodb_held_lock_release_params().is_none(),
            "the silent path is refusing at this point"
        );

        state
            .speleodb_release_lock(link_id.to_owned())
            .expect("an explicit release is not gated by the dirty bit");
        assert!(
            !state.edit_enabled(),
            "releasing the open project's own lock drops the edit gate"
        );
        assert!(
            state.speleodb_open_link().expect("link").has_unpushed_edits,
            "and the edits are still marked un-pushed for the next session"
        );
        let _ = handle.join();
    }

    #[test]
    fn successful_login_invalidates_pending_downloads_without_closing_the_project() {
        let (base, _listener, _seen) = recording_stub(r#"{"email":"u@x"}"#);
        let (_temp, state) = editing_fixture(&base);
        let generation = state.reserve_download_generation();
        state
            .speleodb_login(
                Some(base),
                None,
                None,
                None,
                Some("replacement-token".into()),
            )
            .expect("login");
        assert_ne!(generation, state.speleodb_session_generation());
        assert!(state.get_project_state().is_some());
    }

    #[test]
    fn prepared_discard_rejects_a_concurrent_project_edit() {
        let (_temp, state) = editing_fixture("https://x/");
        let path = state
            .get_project_state()
            .expect("project")
            .file_path
            .expect("path");
        let bytes = serde_json::to_vec(&*state.project.lock().expect("project")).expect("snapshot");
        let result = state.open_project_prepared(
            path,
            Some((state.speleodb_session_generation(), bytes)),
            || {
                state
                    .project
                    .lock()
                    .expect("project")
                    .as_mut()
                    .expect("open")
                    .name = "Concurrent edit".into();
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(
            state.get_project_state().expect("project kept").name,
            "Concurrent edit"
        );
    }

    #[test]
    fn local_only_access_inspection_never_contacts_the_current_server() {
        let (base, _listener, seen) = recording_stub("{}");
        let (temp, state) = editing_fixture(&base);
        let result = state
            .speleodb_project_access_at(
                temp.path()
                    .join("remote.stalx")
                    .to_string_lossy()
                    .into_owned(),
                true,
            )
            .expect("inspection")
            .expect("linked");
        assert!(matches!(
            result.verification,
            crate::speleodb::SpeleoAccessVerification::Unavailable
        ));
        assert!(result.project.permission.is_none());
        assert!(seen.lock().expect("requests").is_empty());
        assert!(state.edit_enabled());
    }

    #[test]
    fn cold_file_open_preflight_reads_link_and_remote_access_without_switching() {
        let (base, _listener, seen) = recording_stub(
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Restricted","permission":"READ_ONLY","active_mutex":{"user":"other@x"}}"#,
        );
        let (temp, state) = editing_fixture(&base);
        let access = state
            .speleodb_project_access_at(
                temp.path()
                    .join("remote.stalx")
                    .to_string_lossy()
                    .into_owned(),
                false,
            )
            .expect("preflight")
            .expect("linked");
        assert_eq!(
            access.project.permission,
            Some(crate::speleodb::Permission::ReadOnly)
        );
        assert_eq!(access.project.lock_holder.as_deref(), Some("other@x"));
        assert!(
            state.edit_enabled(),
            "preflight must not close the current project"
        );
        assert!(seen
            .lock()
            .expect("requests")
            .iter()
            .all(|line| line.starts_with("GET ")));
    }

    #[test]
    fn a_read_only_session_does_not_claim_another_sessions_commit_as_its_own() {
        let (base, _listener) = always_answering_stub(
            r#"{"commits":[{"id":"def456","parent_ids":["abc123"],"author_email":"u@x"},
                           {"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#,
        );
        let (_temp, state) = editing_fixture(&base);
        let link = SpeleoLink {
            endpoint: base.clone(),
            project_id: REMOTE_ID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: true,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        };

        state.set_edit_enabled(false);
        assert_eq!(
            state.speleodb_remote_state(&base, "tok-1", false, &link, Some("u@x")),
            RemoteState::Conflict,
            "a read-only session holds no lock, so a gap commit carrying our email is the \
             other session's - fail closed to the divergence prompt"
        );

        state.set_edit_enabled(true);
        assert_eq!(
            state.speleodb_remote_state(&base, "tok-1", false, &link, Some("u@x")),
            RemoteState::UpToDate,
            "a session that really does hold the lock still gets the shortcut - our \
             own landed push must not be painted as a conflict"
        );
    }

    /// [#258 review] The provenance-scoped unwind releases what THIS process took, and
    /// nothing else.
    ///
    /// An acquire that met a lock this ACCOUNT already held inherited it from a sibling
    /// session guarding its own un-pushed work, and `speleodb_start_editing` is explicit
    /// that no unwind may hand that back. It records that case by leaving
    /// `lock_acquired_here` unset, so an absent record is the inherited case and MUST
    /// no-op - not fall through to a release the way `speleodb_release_lock` would.
    ///
    /// The no-op half is asserted behaviourally, and it is the half that matters: with
    /// no record the call returns `Ok` having issued nothing, which is only reachable by
    /// returning BEFORE the token lookup and the request. The complement - that a
    /// RECORDED acquisition does reach the release - cannot be exercised here without a
    /// server (it gets as far as a real request and blocks), so it rests on the
    /// delegation to `speleodb_release_lock` being the function's only other branch,
    /// plus the id assertion below proving the gate reads what the acquire wrote.
    #[test]
    fn the_unwind_releases_only_a_lock_this_process_recorded_taking() {
        let state = AppState::new().expect("state");
        // A claim carries the account that took it, so there has to be one.
        *state.speleodb.lock().expect("session lock") = authed_session_at("http://x/");

        assert!(
            state.speleodb_take_acquisition_by_this_flow().is_none(),
            "precondition: a fresh session has acquired nothing"
        );
        assert!(
            state.speleodb_release_lock_acquired_here().is_ok(),
            "an inherited lock (no record) must be left alone - and silently, since the \
             caller is already reporting the staleness that sent it here"
        );

        // And the gate reads exactly what a fresh acquire records, so the release it
        // delegates to can only ever name the project this process locked - never the
        // id a caller captured before the context moved under it.
        // What a successful Start Editing commit records - the acquisition the
        // stale-context unwind exists to hand back, and the ONLY thing it will:
        // a bare claim (an inherited lock, or one an earlier flow left behind)
        // is deliberately not offered.
        state.speleodb_note_acquisition_by_this_flow(REMOTE_ID);
        assert_eq!(
            state.speleodb_take_acquisition_by_this_flow().as_deref(),
            Some(REMOTE_ID)
        );
        assert!(
            state.speleodb_take_acquisition_by_this_flow().is_none(),
            "and it answers once - a second unwind must not release it again"
        );
    }

    #[test]
    fn a_shut_edit_gate_withdraws_the_shortcut_even_with_the_record_intact() {
        let (base, _listener) = always_answering_stub(
            r#"{"commits":[{"id":"def456","parent_ids":["abc123"],"author_email":"u@x"},
                           {"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#,
        );
        let (_temp, state) = editing_fixture(&base);
        let link = SpeleoLink {
            endpoint: base.clone(),
            project_id: REMOTE_ID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: true,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        };
        assert_eq!(
            state.speleodb_remote_state(&base, "tok-1", false, &link, Some("u@x")),
            RemoteState::UpToDate,
            "precondition: gate + record + holder together DO license the shortcut"
        );

        state.set_edit_enabled(false);
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "precondition: nothing on this path clears the record, which is the whole \
             point - the two signals really can disagree"
        );
        assert_eq!(
            state.speleodb_remote_state(&base, "tok-1", false, &link, Some("u@x")),
            RemoteState::Conflict,
            "a shut gate withdraws the licence on its own - fail closed to the \
             divergence prompt rather than explaining away a commit we may not have \
             made"
        );
    }

    /// A get_project body and a revisions body in one: serde ignores the fields the
    /// other endpoint owns, so the single-body stub can serve a whole refresh. The
    /// mutex holder is OUR OWN account - a second session signed in as the same
    /// user - and the tip has moved one commit past BASE, authored by that account.
    const SAME_ACCOUNT_HOLDER_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "permission":"READ_AND_WRITE","active_mutex":{"user":"u@x"},
        "commits":[{"id":"def456","parent_ids":["abc123"],"author_email":"u@x"},
                   {"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn a_late_metadata_response_cannot_rename_a_different_open_project() {
        let (_temp, state) = editing_fixture("https://unused.example/");
        let original = state
            .project
            .lock()
            .expect("project")
            .as_ref()
            .expect("open")
            .name
            .clone();
        state.speleodb_fold_local_name(
            "Stale response",
            "22222222-2222-2222-2222-222222222222",
            state.speleodb_session_generation(),
        );
        assert_eq!(
            state
                .project
                .lock()
                .expect("project")
                .as_ref()
                .expect("open")
                .name,
            original
        );
        assert_eq!(
            state.speleodb_open_link().expect("link").project_name,
            "Remote"
        );
        state.speleodb_fold_local_name(
            "Current response",
            REMOTE_ID,
            state.speleodb_session_generation(),
        );
        assert_eq!(
            state
                .project
                .lock()
                .expect("project")
                .as_ref()
                .expect("open")
                .name,
            "Current response"
        );
        assert_eq!(
            state.speleodb_open_link().expect("link").project_name,
            "Current response"
        );
    }

    #[test]
    fn fresh_write_verification_closes_the_gate_on_confirmed_lock_or_permission_loss() {
        for body in [
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{"user":"other@x"}}"#,
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote","permission":"READ_ONLY","active_mutex":{"user":"u@x"}}"#,
        ] {
            let (base, _listener) = always_answering_stub(body);
            let (_temp, state) = editing_fixture(&base);
            state.mark_remote_dirty();
            let link = state.speleodb_open_link().expect("remote link");
            assert!(state.speleodb_require_owned_lock(&link).is_err());
            assert!(!state.edit_enabled());
            assert!(
                state
                    .speleodb_open_link()
                    .expect("preserved link")
                    .has_unpushed_edits
            );
        }
    }

    #[test]
    fn a_second_session_on_the_same_account_never_inherits_the_holders_authorship() {
        // Another session on our account has pushed. Refresh must neither enable
        // editing nor explain that session's commit away as our own. Only a verified
        // acquisition by this process can establish the authorship provenance.
        let (base, _listener) = always_answering_stub(SAME_ACCOUNT_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "precondition: this process has taken no lock - it opened the project \
             read-only, exactly as a restart or a second window does"
        );

        let first = state.speleodb_link_status().expect("refresh 1");
        assert_eq!(
            first.lock_holder.as_deref(),
            Some("u@x"),
            "precondition: the server names our own account as the holder"
        );
        assert!(
            !first.edit_enabled,
            "status refresh never enables editing, even under our account lock"
        );
        assert_eq!(
            first.remote_state,
            RemoteState::Conflict,
            "the other session's commit is not ours to explain away"
        );

        let second = state.speleodb_link_status().expect("refresh 2");
        assert_eq!(
            second.remote_state,
            RemoteState::Conflict,
            "a second refresh must preserve the conflict and the shut edit gate"
        );

        // The other side of the line: a process that really did take this lock keeps
        // the shortcut, so its own landed push is not painted as a conflict.
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));
        let third = state.speleodb_link_status().expect("refresh 3");
        assert_eq!(
            third.remote_state,
            RemoteState::UpToDate,
            "an acquire made by THIS process still licenses the authorship recovery"
        );
    }

    #[test]
    fn workspace_only_an_acquire_made_by_this_process_records_the_authorship_licence() {
        // The provenance is written by the acquire itself and given back with the
        // lock. A history whose tip IS our BASE reconciles UpToDate, so Start editing
        // keeps the lock it just took without a fast-forward (which a unit test cannot
        // reach - the reload needs an app cache dir only the live tests initialise).
        let (base, _listener) = always_answering_stub(
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote",
                "commits":[{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#,
        );
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "a read-only open records nothing"
        );

        assert!(
            matches!(
                state.speleodb_start_editing().expect("start editing"),
                StartEditingResult::Editing
            ),
            "the fixture reconciles cleanly, so the lock is taken and kept"
        );
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "the acquire THIS process made is what gets recorded"
        );

        state
            .speleodb_release_lock(REMOTE_ID.to_owned())
            .expect("release");
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "handing the lock back ends the claim it licensed"
        );
    }

    /// [`SAME_ACCOUNT_HOLDER_BODY`]'s mutex - our own account already holding the
    /// lock - over a history whose tip IS our BASE, so `speleodb_start_editing`
    /// reconciles UpToDate and KEEPS the lock. (The fast-forward arm needs an app
    /// cache dir only the env-gated live tests initialise, so UpToDate is the only
    /// keep-the-lock arm a unit test can reach.)
    const HELD_BY_SAME_ACCOUNT_AT_BASE_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "permission":"READ_AND_WRITE","active_mutex":{"user":"u@x"},
        "commits":[{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn workspace_a_successful_acquire_over_an_inherited_lock_records_no_authorship() {
        // A 200 from `acquire` is NOT proof that this process took the lock:
        // re-acquiring a lock the ACCOUNT already holds is a successful server-side
        // no-op (only ANOTHER holder produces the 400), and `ActiveMutex` carries no
        // timestamp to tell the two apart. So a restarted process, a second window, or
        // the held-lock dialog's "Continue editing" - which calls this verb precisely
        // to resume an INHERITED lock - used to walk out of Start editing holding an
        // authorship claim they never earned, and the shortcut in
        // `speleodb_remote_state` would then explain a sibling session's commit away
        // as our own unrecorded push. Exactly the defect this branch exists to remove.
        //
        // The stub names our own account as the holder BEFORE the acquire, which is
        // what the pre-acquire probe reads.
        let (base, _listener) = always_answering_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        assert!(
            matches!(
                state.speleodb_start_editing().expect("start editing"),
                StartEditingResult::Editing
            ),
            "the tip IS our BASE, so the reconcile is clean and the lock is kept"
        );
        assert!(
            state.edit_enabled(),
            "resuming an inherited lock still opens the edit gate - the user edits \
             exactly as before, they simply carry no authorship claim"
        );
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "but a lock the account ALREADY held was not taken by this process, so \
             nothing may be recorded"
        );
    }

    #[test]
    fn workspace_resuming_an_inherited_lock_over_a_diverged_tip_never_hands_it_back() {
        // The held-lock prompt's "Continue editing" is a RESUME: the mutex is already
        // ours (a crashed run, a second window, a session the app never got to end),
        // the acquire is a server-side no-op, and the un-pushed work on this machine
        // is exactly what that lock is still holding the project for.
        // `SAME_ACCOUNT_HOLDER_BODY` is that world - our own account on the mutex over
        // a tip one commit past our BASE, which is also the shape of an upload that
        // LANDED without the local BASE ever being advanced - and the fixture is
        // dirty, so the fence returns Diverged.
        //
        // The abort arm used to release unconditionally, so pressing Continue editing
        // surrendered a mutex this call never took: the divergence prompt came up over
        // a project now free for the next writer to lock and push, while the local
        // work was still un-pushed and only here. Exactly the outcome the whole
        // held-lock prompt exists to prevent.
        //
        // Asserted by CONTRADICTION on the wire: the stub records every request line,
        // so a release that still went out is visible as a POST to `/release/`. The
        // acquire is asserted POSITIVELY from the same log, so this cannot pass by the
        // flow simply never reaching the server.
        let (base, _listener, seen) = recording_stub(SAME_ACCOUNT_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        let result = state.speleodb_start_editing().expect("start editing");
        assert!(
            matches!(result, StartEditingResult::DivergenceNeeded),
            "a moved tip over un-pushed work still routes to the divergence prompt, \
             got {result:?}"
        );

        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/acquire/"))),
            "precondition: the resume really did run the acquire against the stub, so \
             the absence below is about the release and not about a flow that never \
             left the process; saw {seen:?}"
        );
        assert!(
            !seen
                .iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/release/"))),
            "Start editing must never hand back a lock it did not take: releasing here \
             frees the project for another writer while the edits are still only on \
             this machine; saw {seen:?}"
        );
        assert!(
            !state.edit_enabled(),
            "the divergence prompt still refuses the edit gate"
        );
        assert!(
            state.speleodb_open_link().expect("link").has_unpushed_edits,
            "and the un-pushed work the kept lock is protecting is untouched"
        );
    }

    /// [`SAME_ACCOUNT_HOLDER_BODY`]'s diverged history with the mutex FREE, so the
    /// pre-acquire probe reads "unheld" and the lock Start editing takes really is
    /// its own to hand back.
    ///
    /// "Diverged" is a property of the COPY, not of this history: the same tip one
    /// commit past BASE fences to FastForward over a clean fixture, which is how
    /// `a_freshly_taken_lock_is_still_released_when_the_fast_forward_fails` reaches
    /// the other arm.
    const FREE_MUTEX_DIVERGED_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "commits":[{"id":"def456","parent_ids":["abc123"],"author_email":"u@x"},
                   {"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn workspace_a_freshly_taken_lock_is_still_released_on_divergence() {
        // The other side of the line the test above draws, and the reason the fix is
        // "release what we took" rather than "never release": when the probe finds the
        // mutex UNHELD, the lock the acquire produced is this call's own, and walking
        // away from it without releasing would strand the project locked for every
        // other editor over an edit session that never started.
        //
        // Same diverged history, same dirty fixture - only the mutex differs - so the
        // two tests differ in exactly the input the fix keys on.
        let (base, _listener, seen) = recording_stub(FREE_MUTEX_DIVERGED_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        let result = state.speleodb_start_editing().expect("start editing");
        assert!(
            matches!(result, StartEditingResult::DivergenceNeeded),
            "same divergence routing regardless of who held the mutex, got {result:?}"
        );

        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/release/"))),
            "a lock taken by THIS call must be handed back when the reconcile refuses \
             the edit session; saw {seen:?}"
        );
    }

    /// [`FREE_MUTEX_DIVERGED_BODY`] with ANOTHER account on the mutex: the narrow race
    /// where the probe sees a holder who lets go before our acquire lands. A holder who
    /// is still there answers the acquire with a 400 instead, which returns `LockedBy`
    /// long before any unwind - so reaching the arms over this body means the mutex
    /// changed hands to US in between.
    const OTHER_HOLDER_DIVERGED_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "active_mutex":{"user":"someone.else@x"},
        "commits":[{"id":"def456","parent_ids":["abc123"],"author_email":"u@x"},
                   {"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn workspace_a_lock_another_holder_released_just_before_our_acquire_is_still_handed_back() {
        // The probe answers "held", which used to be read as INHERITED on the strength
        // of the mutex being non-empty alone - discarding WHO held it. But another
        // holder who is still holding never reaches these arms (the acquire 400s and
        // returns `LockedBy`), so a held mutex followed by a successful acquire means
        // that holder released in between and THIS call made the unheld -> ours
        // transition. Treating it as inherited skipped the unwind release and stranded
        // a mutex we had just taken over an edit session that never started, blocking
        // every other writer until somebody released it by hand (SpeleoDB has no
        // lock timeout).
        //
        // Same diverged history and dirty fixture as the two tests above, so the only
        // input that differs is the mutex holder's identity.
        let (base, _listener, seen) = recording_stub(OTHER_HOLDER_DIVERGED_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        let result = state.speleodb_start_editing().expect("start editing");
        assert!(
            matches!(result, StartEditingResult::DivergenceNeeded),
            "same divergence routing regardless of who held the mutex, got {result:?}"
        );

        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/acquire/"))),
            "precondition: the acquire really ran against the stub and succeeded, so \
             the mutex is ours now and the assertion below is about the unwind, not \
             about a flow that never left the process; saw {seen:?}"
        );
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/release/"))),
            "a lock the PREVIOUS holder let go of, taken by this very call, must be \
             handed back like any other freshly taken lock; saw {seen:?}"
        );
    }

    /// [`HELD_BY_SAME_ACCOUNT_AT_BASE_BODY`] with ANOTHER account on the mutex, over a
    /// history whose tip IS our BASE - the same released-in-between race reaching the
    /// UpToDate arm, which is the only arm that can be observed to record provenance
    /// (the divergence arm releases, and a release ends the claim by design).
    const HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "active_mutex":{"user":"someone.else@x"},
        "commits":[{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn workspace_a_lock_another_holder_released_just_before_our_acquire_records_authorship() {
        // The other half of the same race. Classifying it as inherited also withheld
        // the authorship record from a lock this process demonstrably took, and the
        // shortcut in `speleodb_remote_state` hangs on that record: without it, our own
        // landed-but-unrecorded push later reads as a stranger's commit and the status
        // button paints `Conflict` against our own work.
        let (base, _listener, seen) = recording_stub(HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        assert!(
            matches!(
                state.speleodb_start_editing().expect("start editing"),
                StartEditingResult::Editing
            ),
            "the tip IS our BASE, so the reconcile is clean and the lock is kept"
        );
        assert!(
            seen.lock()
                .expect("request log")
                .iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/acquire/"))),
            "precondition: the acquire ran and succeeded over the other account's \
             released mutex"
        );
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "the unheld -> ours transition was made by THIS call, so it is ours to \
             record - a holder who let go before we asked is not an inherited lock"
        );
    }

    /// [`SAME_ACCOUNT_HOLDER_BODY`]'s mutex spelled in a different case. SpeleoDB may
    /// normalise the address it echoes back, so "is this holder us?" is case-insensitive
    /// everywhere ([`lock_held_by_me`]).
    const SAME_ACCOUNT_UPPERCASE_HOLDER_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "active_mutex":{"user":"U@X"},
        "commits":[{"id":"def456","parent_ids":["abc123"],"author_email":"u@x"},
                   {"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn workspace_a_holder_differing_only_in_case_is_still_our_own_inherited_lock() {
        // DO-NOT-REGRESS: passes before the who-holds-it split as well as after, and
        // exists to pin the direction the split must NOT be taken in. Now that the
        // held-mutex arm compares the holder against our email, a case-SENSITIVE
        // comparison would read our own account back in a different case as "someone
        // else", hand the resume an authorship claim it never earned, and release the
        // very mutex the held-lock prompt's "Continue editing" exists to keep - over
        // un-pushed work that is still only on this machine.
        let (base, _listener, seen) = recording_stub(SAME_ACCOUNT_UPPERCASE_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        let result = state.speleodb_start_editing().expect("start editing");
        assert!(
            matches!(result, StartEditingResult::DivergenceNeeded),
            "a moved tip over un-pushed work still routes to the divergence prompt, \
             got {result:?}"
        );

        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/acquire/"))),
            "precondition: the resume really did run the acquire against the stub; \
             saw {seen:?}"
        );
        assert!(
            !seen
                .iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/release/"))),
            "U@X is u@x: the lock is ours already, so this is a resume and the mutex \
             stays put; saw {seen:?}"
        );
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "and an inherited lock still mints no authorship record"
        );
    }

    #[test]
    fn a_holder_we_cannot_compare_against_never_mints_an_authorship_record() {
        // The fail-safe on the held-mutex split, pinned structurally because no unit
        // test can reach it: `speleodb_active_token` refuses without an email, so by the
        // probe we always have one - unless a concurrent logout drops the session under
        // an in-flight call, which is a race no single-threaded test has a seam for.
        //
        // The guard is what keeps that race failing in the safe direction.
        // `lock_held_by_me` answers FALSE for an unknown self-email (it is "the lock is
        // not ours", not "we cannot say"), so without the `me.is_some()` guard the
        // holder arm would read a logged-out session as "someone else's lock we just
        // took" and mint a record on it. Unknown provenance must answer like a failed
        // probe: no claim.
        let start_editing = prod_source()
            .split_once("fn speleodb_start_editing")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n    fn ").next())
            .expect("speleodb_start_editing body precedes the next method");
        let probe = start_editing
            .split_once("match self.speleodb_lock_holder_checked(")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("};").next())
            .expect("the probe match precedes the unwind closure");

        assert!(
            probe.contains("lock_held_by_me("),
            "the held-mutex arm must ask the shared case-insensitive predicate who the \
             holder is, not re-spell the comparison"
        );
        assert!(
            probe.contains("Ok(Some(_)) if me.is_some() => (true, false)"),
            "claiming a held mutex as ours to record must be guarded on HAVING an email \
             to compare against"
        );
        assert!(
            probe.contains("Ok(Some(_)) | Err(_) => (false, link.has_unpushed_edits)"),
            "and a holder we cannot compare against must fall through to the same \
             answer an unreadable probe gets: no record, keep only to protect work"
        );
    }

    #[test]
    fn a_refresh_over_a_shut_gate_withholds_the_shortcut_and_keeps_the_pointer() {
        // Closing or switching can shut editing outside the explicit release path.
        // The refresh must leave editing shut and withhold the authorship shortcut;
        // it no longer drops the claim to do it, because the shortcut reads the gate
        // and the gate IS the bound claim (data#3). The claim stays as the pointer a
        // later holder-checked release needs.
        let (base, _listener) = always_answering_stub(SAME_ACCOUNT_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));
        // The session ends somewhere this file cannot hook (here: a lost-lock signal).
        state.set_edit_enabled(false);

        let status = state.speleodb_link_status().expect("refresh");
        assert!(
            !status.edit_enabled,
            "status refresh keeps editing shut under our own lock"
        );
        assert_eq!(
            status.remote_state,
            RemoteState::Conflict,
            "and an unbound claim no longer licenses the shortcut"
        );
        let status = state.speleodb_link_status().expect("a second refresh");
        assert!(
            !status.edit_enabled && status.remote_state == RemoteState::Conflict,
            "nor does any later refresh resurrect either"
        );
        assert!(
            state.speleodb_holds_claim(REMOTE_ID),
            "the claim survives as a pointer to the lock"
        );
    }

    /// The production region of this file, split out of the test module so the
    /// source-level tripwires below cannot match their own assertions.
    fn prod_source() -> &'static str {
        include_str!("speleodb.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production region precedes the test module")
    }

    /// [#275] The same, for the push half after it moved to `speleodb_push.rs`.
    ///
    /// The tripwires that pin the upload's internal ORDERING (flush before snapshot,
    /// heal only after a landing, the fast-forward's lock-pair capture) are structural
    /// assertions about that source, so they have to follow it. Left pointed at
    /// `speleodb.rs` they would not fail - they would stop finding the function at all
    /// and panic in `expect`, which reports the wrong thing entirely.
    fn push_prod_source() -> &'static str {
        include_str!("speleodb_push.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("production region precedes the test module")
    }

    #[test]
    fn the_refresh_does_no_claim_bookkeeping() {
        // data#3: the refresh used to drop the open project's claim whenever it saw
        // a shut gate - a catch-all for the release paths that ran no release verb,
        // needed because a claim on its own licensed the authorship shortcut. The
        // gate is now the claim bound to the session, so a shut gate already
        // withdraws the licence, and the claim is the pointer a later holder-checked
        // release needs. Nothing in the refresh may end it.
        let status = prod_source()
            .split_once("fn speleodb_link_status")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n    fn ").next())
            .expect("speleodb_link_status body precedes the next method");
        for clear in [
            "speleodb_forget_lock_acquired",
            "speleodb_note_lock_acquired",
            "locks_acquired_here",
        ] {
            assert!(
                !status.contains(clear),
                "the status refresh must not touch the claims (`{clear}`)"
            );
        }
    }

    #[test]
    fn a_shut_gate_keeps_its_claim_as_a_pointer_that_opens_nothing() {
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock().expect("session lock") = authed_session_at("http://x/");
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));
        assert!(state.edit_enabled(), "fixture: an authored edit session");

        // Lost or unverifiable ownership shuts the gate...
        state.speleodb_shut_edit_gate();
        assert!(!state.edit_enabled(), "the gate is shut");
        // ...and keeps the claim, which is what a later holder-checked release
        // (close, the janitor) needs to find the lock at all.
        assert!(state.speleodb_holds_claim(REMOTE_ID));
        // A status refresh does not reopen it, and does not drop the claim.
        let _ = state.speleodb_link_status();
        assert!(!state.edit_enabled(), "only Start Editing opens the gate");
        assert!(state.speleodb_holds_claim(REMOTE_ID));
    }

    /// A loopback stub that answers EVERY request with the same 200 + body, for a
    /// flow that makes several calls. `release_stub` is one-shot, which would fail
    /// the second request for the wrong reason.
    ///
    /// The listener is returned so the caller keeps the port bound for the whole
    /// test (as in [`counting_stub`]).
    fn always_answering_stub(body: &'static str) -> (String, std::net::TcpListener) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let accept_on = listener.try_clone().expect("clone listener");
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = accept_on.accept() {
                // Drain the request until the socket goes quiet, then answer.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                }
                let body = crate::core::state::tests_support::sdk_fixture(&body);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), listener)
    }

    /// [`always_answering_stub`] that also RECORDS each request's first line
    /// (`METHOD /path HTTP/1.1`), so a test can assert WHICH calls a flow made -
    /// and, more to the point, which it did not. [`counting_stub`] proves an absence
    /// too, but it answers nothing, so a multi-call flow cannot run against it at
    /// all; this one lets the whole verb complete and then reads the wire.
    ///
    /// The listener is returned for the same reason as the two stubs below: the
    /// caller must keep the port bound for the whole test.
    fn recording_stub(
        body: &'static str,
    ) -> (
        String,
        std::net::TcpListener,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let accept_on = listener.try_clone().expect("clone listener");
        let log = std::sync::Arc::clone(&seen);
        std::thread::spawn(move || {
            let held = std::sync::Mutex::new(None);
            while let Ok((mut stream, _)) = accept_on.accept() {
                // Drain the request until the socket goes quiet, then answer.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut req = Vec::new();
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&chunk[..n]);
                }
                let text = String::from_utf8_lossy(&req).into_owned();
                let line = text.lines().next().unwrap_or_default().to_owned();
                if let Ok(mut lines) = log.lock() {
                    lines.push(line.clone());
                }
                // The server's mutex follows this flow's own acquire and release,
                // which the holder-checked release reads back (lock#4).
                let body = crate::core::state::tests_support::sdk_fixture(
                    &crate::core::state::tests_support::with_tracked_mutex(
                        &line, body, &held, "u@x",
                    ),
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), listener, seen)
    }

    /// [`recording_stub`] that FAILS the bare project GET - `speleodb_start_editing`'s
    /// pre-acquire lock probe - with a 503, while answering every other call (acquire,
    /// revisions, release) normally. The unknown-provenance path, which no uniform stub
    /// can produce: `always_answering_stub` answers the probe as happily as the rest.
    ///
    /// Keyed on the PATH rather than on "the first request", because `get_project` is
    /// idempotent and retries: refusing only the first attempt would let attempt 2
    /// succeed and the probe would come back `Ok` after all. A 503 on every attempt is
    /// a transient server failure that outlives our retry budget, which is exactly the
    /// `Err` the probe reports. The probe is the only call in this flow that GETs
    /// `projects/{id}/` with no further suffix (`revisions/`, `acquire/` and `release/`
    /// all carry one), so the path pins it precisely.
    fn stub_failing_the_lock_probe(
        body: &'static str,
    ) -> (
        String,
        std::net::TcpListener,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let accept_on = listener.try_clone().expect("clone listener");
        let log = std::sync::Arc::clone(&seen);
        std::thread::spawn(move || {
            let probe_path = format!("/projects/{REMOTE_ID}/");
            let held = std::sync::Mutex::new(None);
            while let Ok((mut stream, _)) = accept_on.accept() {
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut req = Vec::new();
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&chunk[..n]);
                }
                let text = String::from_utf8_lossy(&req).into_owned();
                let line = text.lines().next().unwrap_or_default().to_owned();
                if let Ok(mut lines) = log.lock() {
                    lines.push(line.clone());
                }
                // Only the PRE-acquire probe fails: once this flow's acquire has
                // landed, the release's own holder check (lock#4) reads the
                // recovered server, which reports the mutex this flow took.
                let acquired = held.lock().is_ok_and(|held| held.is_some());
                let is_probe = !acquired
                    && line.starts_with("GET ")
                    && line
                        .split_whitespace()
                        .nth(1)
                        .is_some_and(|path| path.ends_with(&probe_path));
                let body = crate::core::state::tests_support::sdk_fixture(
                    &crate::core::state::tests_support::with_tracked_mutex(
                        &line, body, &held, "u@x",
                    ),
                );
                let resp = if is_probe {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\
                     Connection: close\r\n\r\n"
                        .to_owned()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), listener, seen)
    }

    #[test]
    fn workspace_an_unreadable_probe_over_unpushed_edits_keeps_the_lock() {
        // The third probe state, and the one neither test above covers: the pre-acquire
        // GET fails while the acquire itself succeeds. Provenance is then UNKNOWN - the
        // acquire's 200 is equally the no-op of re-taking a lock this account already
        // holds - so reading it as "not inherited" and releasing on the way out frees
        // the project for the next writer while the un-pushed work is still only on
        // this machine. That is the same loss `resuming_an_inherited_lock_over_a_
        // diverged_tip_never_hands_it_back` prevents, reached through a network blip
        // instead of a successful probe.
        //
        // So the unknown case is decided by what a release would COST: with un-pushed
        // edits behind the lock, it is not handed back. Asserted by CONTRADICTION on
        // the wire, with the acquire asserted POSITIVELY from the same log so this
        // cannot pass by a flow that never left the process.
        let (base, _listener, seen) = stub_failing_the_lock_probe(SAME_ACCOUNT_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        let result = state.speleodb_start_editing().expect("start editing");
        assert!(
            matches!(result, StartEditingResult::DivergenceNeeded),
            "a moved tip over un-pushed work still routes to the divergence prompt, \
             got {result:?}"
        );

        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/acquire/"))),
            "precondition: the probe failed but the acquire still ran against the stub, \
             so the absence below is about the release and not about a flow that never \
             left the process; saw {seen:?}"
        );
        assert!(
            !seen
                .iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/release/"))),
            "a lock whose provenance we could not read must not be handed back over \
             un-pushed work: it may be the very lock guarding it; saw {seen:?}"
        );
        assert!(
            state.speleodb_open_link().expect("link").has_unpushed_edits,
            "and the un-pushed work the kept lock is protecting is untouched"
        );
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "keeping the lock is NOT a claim to have taken it - a failed probe still \
             mints no authorship record"
        );
    }

    /// A history whose tip is unrelated to our recorded BASE (`abc123` is absent, so it
    /// is not an ancestor of the tip): [`sync::reconcile_verdict`] fails closed to
    /// Diverged even over a CLEAN copy, which is the one way a unit test can reach an
    /// unwind arm with nothing un-pushed to protect.
    const CLEAN_DIVERGED_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "commits":[{"id":"zzz999","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn workspace_an_unreadable_probe_over_a_clean_copy_still_releases_the_lock() {
        // The other half of the unknown-probe rule, and the reason it is not a blanket
        // "keep on unknown": with NOTHING un-pushed there is no work to strand, and a
        // lock this call may well have just taken would be left held over an edit
        // session that never started - recoverable only through the next login's
        // held-lock prompt or a release by hand (SpeleoDB has no lock timeout).
        //
        // Same failing probe as the test above, same divergence routing; only the dirty
        // bit differs, which is exactly the input the rule keys on.
        let (base, _listener, seen) = stub_failing_the_lock_probe(CLEAN_DIVERGED_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(false);
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        let result = state.speleodb_start_editing().expect("start editing");
        assert!(
            matches!(result, StartEditingResult::DivergenceNeeded),
            "an unrelated tip fails closed to the divergence prompt even when clean, \
             got {result:?}"
        );

        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/release/"))),
            "with nothing un-pushed behind it, a lock of unknown provenance is still \
             handed back rather than left lingering; saw {seen:?}"
        );
    }

    #[test]
    fn workspace_a_freshly_taken_lock_is_still_released_when_the_fast_forward_fails() {
        // DO-NOT-REGRESS (passes before the local-edits keep rule as well as after):
        // the fast-forward arm's OTHER unwind, and the line the keep rule must not
        // cross. A lock this call demonstrably took, over a copy with nothing local at
        // stake, is still handed back when the fast-forward blows up - leaving it held
        // would strand the project for every other editor over an edit session that
        // never started.
        //
        // Also the reach test for that arm: `FREE_MUTEX_DIVERGED_BODY`'s history over
        // a CLEAN fixture fences to FastForward rather than Diverged (dirty is the only
        // difference), and the stub answers the native-file download with JSON, so the
        // fast-forward fails inside native SQLite validation - a corrupt-file failure, not the
        // local-edits refusal. So the arm really does run to its unwind here, and the
        // release below is that unwind's doing.
        let (base, _listener, seen) = recording_stub(WORKSPACE_NEWER_FREE_BODY);
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        state
            .speleodb_start_editing()
            .expect_err("a JSON body cannot be opened as a dump");

        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/download/stalagmight/"))),
            "precondition: the flow reached the fast-forward arm and tried the download, \
             so the release below is that arm's unwind; saw {seen:?}"
        );
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/release/"))),
            "a lock taken by THIS call, unwound over a failure with no local work at \
             stake, must still be handed back; saw {seen:?}"
        );
    }

    #[test]
    fn only_the_local_edits_refusal_makes_an_unwind_keep_the_lock() {
        // What the fast-forward unwind keys on, fed the REAL errors rather than a
        // hand-built variant: both of `guard_local_edits`' refusals must keep the lock,
        // and a corrupt native file must not.
        //
        // The unreadable case matters as much as the dirty one. The guard cannot rule
        // out un-pushed work in a file it could not read (a hot journal a read-write
        // open would have rolled back, a transient `SQLITE_BUSY`), so it fails closed
        // with the same error - and a lock decision reading that as "nothing to
        // protect" would free the project for the next writer over exactly the work the
        // guard refused to overwrite.
        let temp = TempDir::new().expect("temp dir");
        let state = AppState::new().expect("state");

        let dirty = temp.path().join("dirty.stalx");
        seed_linked_stalx(&dirty, GUARD_ID, true);
        let refused = state
            .guard_local_edits(GUARD_ID, &dirty)
            .expect_err("un-pushed edits veto the replace");
        assert!(
            is_local_edits_refusal(&refused),
            "the refusal over un-pushed work is what keeps the lock, got {refused:?}"
        );

        let unreadable = temp.path().join("unreadable.stalx");
        std::fs::write(&unreadable, b"not a sqlite database at all").expect("write junk");
        let unknown = state
            .guard_local_edits(GUARD_ID, &unreadable)
            .expect_err("an unreadable copy is not a clean one");
        assert!(
            is_local_edits_refusal(&unknown),
            "a copy that could not be read fails closed to the same keep, got {unknown:?}"
        );

        let corrupt = SpeleoDbError::Parse("invalid SQLite project".to_owned());
        assert!(
            !is_local_edits_refusal(&corrupt),
            "a corrupt native file has nothing local at stake, so a lock we took is still \
             handed back, got {corrupt:?}"
        );
    }

    #[test]
    fn a_lock_kept_for_the_cache_copys_work_leaves_a_kept_claim() {
        // The stranded lock: Start Editing took the lock, the fast-forward's re-pull
        // was refused because the CACHE copy holds un-uploaded work the clean open
        // copy does not show, and the unwind rightly kept the lock - with no claim,
        // so nothing in this process could ever find it again. Fed the REAL refusal.
        let temp = TempDir::new().expect("temp dir");
        let (server, seen) =
            crate::core::state::tests_support::lock_server(GUARD_ID, Some("u@x".to_owned()));
        let state = AppState::new().expect("state");
        *state.speleodb.lock().expect("session lock") = authed_session_at(&server);
        let dirty = temp.path().join("dirty.stalx");
        seed_linked_stalx(&dirty, GUARD_ID, true);
        let refused = state
            .guard_local_edits(GUARD_ID, &dirty)
            .expect_err("un-pushed edits veto the replace");
        let params = super::super::speleodb_lock_release::LockReleaseParams {
            endpoint: server.clone(),
            token: "tok-1".to_owned(),
            email: "u@x".to_owned(),
            insecure_http: true,
            project_id: GUARD_ID.to_owned(),
        };

        // `false`: the probe found the mutex free, so this call took the lock.
        state.speleodb_unwind_unless_kept(
            false,
            Some(&refused),
            &params,
            state.speleodb_session_generation(),
        );

        let posts = || crate::core::state::tests_support::release_posts(&seen);
        assert_eq!(posts(), 0, "the lock guarding that work is kept");
        assert!(state.speleodb_holds_claim(GUARD_ID), "with a pointer to it");
        assert!(state.speleodb_claim_is_kept(GUARD_ID), "marked kept");
        assert!(
            !state.speleodb_lock_acquired_here(GUARD_ID),
            "and licensing no authorship"
        );

        // No automatic release hands it back over the cache copy's work: not the
        // stranded-claim cleanup (no project is open here)...
        state.speleodb_janitor_stranded_claim();
        assert_eq!(posts(), 0);
        assert!(state.speleodb_holds_claim(GUARD_ID));

        // ...while an explicit Release still can (holder-checked: it is ours).
        state
            .speleodb_release_lock(GUARD_ID.to_owned())
            .expect("explicit release");
        assert_eq!(posts(), 1);
        assert!(!state.speleodb_holds_claim(GUARD_ID));

        // Any other failure hands back a lock this call took, exactly as before.
        let corrupt = SpeleoDbError::Parse("invalid SQLite project".to_owned());
        state.speleodb_unwind_unless_kept(
            false,
            Some(&corrupt),
            &params,
            state.speleodb_session_generation(),
        );
        assert_eq!(posts(), 2, "a corrupt download has nothing local at stake");
        assert!(!state.speleodb_holds_claim(GUARD_ID));
    }

    #[test]
    fn the_fast_forward_unwind_decides_on_the_failure_it_is_unwinding() {
        // The site the two tests above meet at. `keep_lock_on_unwind` is computed
        // BEFORE the acquire from the OPEN project's link, and the fast-forward verdict
        // is only reached over a CLEAN one - so on this arm that flag keeps the lock for
        // an inherited one and nothing else, while `Protect` refuses over the copy at
        // the CACHE path, which need not be the open file at all (Save-As carries the
        // `speleo_link` row forward, so P can be open and clean while the cache copy of
        // P holds un-pushed work). Releasing there frees the server mutex that is
        // holding the project for edits that exist nowhere else.
        //
        // Structural because the arm cannot reach that refusal in a unit test: the
        // guard runs inside the re-pull, past `speleodb_cache_path`, which resolves an
        // app cache dir only the env-gated live tests initialise - and initialising it
        // here would latch the process-wide `OnceLock` for every other test in the
        // binary. The sibling above reaches the arm behaviourally and pins the release
        // half; this pins which failures skip it.
        let start_editing = prod_source()
            .split_once("fn speleodb_start_editing")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n    fn ").next())
            .expect("speleodb_start_editing body precedes the next method");
        let fast_forward = start_editing
            .split_once("ReconcileVerdict::FastForward => {")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("ReconcileVerdict::Diverged").next())
            .expect("the fast-forward arm precedes the divergence arm");

        assert!(
            fast_forward.contains("LocalEditsPolicy::Protect"),
            "precondition: this is the arm whose re-pull can refuse over local edits"
        );
        assert!(
            fast_forward.contains("release_unless_kept("),
            "the unwind must stay wired: a failed fast-forward that keeps the lock \
             unconditionally strands every project whose dump merely failed to parse"
        );
        assert!(
            fast_forward.contains("release_unless_kept(Some(&e));"),
            "and must hand the failure over - the refusal is the only evidence that \
             the copy being overwritten holds work the open project's link never did"
        );
        assert!(
            !fast_forward.contains("release_unless_kept();")
                && !fast_forward.contains("release_unless_kept(None)"),
            "releasing without consulting the failure is the defect itself"
        );
        assert!(
            start_editing
                .split_whitespace()
                .collect::<String>()
                .contains("self.speleodb_unwind_unless_kept(keep_lock_on_unwind,failure,"),
            "the unwind decides through the shared method, handed the failure"
        );
        let session = include_str!("speleodb_session.rs");
        let unwind = session
            .split_once("fn speleodb_unwind_unless_kept")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n    }\n").next())
            .expect("the unwind decision is present");
        assert!(
            unwind.contains("failure.is_some_and(super::speleodb::is_local_edits_refusal)"),
            "and the unwind must act on it: passing the failure in and ignoring it \
             releases just the same"
        );
    }

    /// A loopback stub that only COUNTS connections, for asserting that a request
    /// was never made. `release_stub` answers but records nothing, so a test built
    /// on it passes whether or not the release went out.
    ///
    /// The listener is returned so the caller keeps the port bound for the whole
    /// test: dropping it would free the port and a later connect would fail for the
    /// wrong reason.
    fn counting_stub() -> (
        String,
        std::net::TcpListener,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let accept_on = listener.try_clone().expect("clone listener");
        let counter = std::sync::Arc::clone(&hits);
        std::thread::spawn(move || {
            while let Ok((stream, _)) = accept_on.accept() {
                counter.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });
        (format!("http://127.0.0.1:{port}"), listener, hits)
    }

    #[test]
    fn logout_does_not_release_a_lock_over_unpushed_edits() {
        // Logging out is not the same act as surrendering the lock. Unlike unlink /
        // take-remote - the verbs that legitimately bypass the veto - a logout
        // neither deletes the link row nor discards the edits, so releasing here
        // leaves the project free for a colleague to lock and push while the work
        // is still only on this machine. The lock left held is recoverable: the
        // next login's scan raises the held-lock prompt over this very project.
        //
        // Asserted by CONTRADICTION: the endpoint is a live listener that counts
        // connections, so a release that still went out is observable as a hit.
        //
        // Under `with_isolated_config` because logout WRITES the config (it clears the
        // rehydrate seed), so the save needs a config file of its own rather than
        // whatever this thread would otherwise resolve to.
        crate::test_helpers::with_isolated_config(|| {
            let (base, _listener, hits) = counting_stub();
            let (_temp, state) = editing_fixture(&base);
            state.mark_remote_dirty();
            assert!(
                state.speleodb_open_link().expect("link").has_unpushed_edits,
                "fixture is dirty"
            );

            let _ = state.speleodb_logout();

            assert_eq!(
                hits.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "logout must not issue ANY request to release a lock over un-pushed edits"
            );
            assert!(
                !state.edit_enabled(),
                "a logged-out session must not keep editing a project it cannot push"
            );
            assert!(
                state.speleodb_open_link().expect("link").has_unpushed_edits,
                "the edits stay marked un-pushed for the next session"
            );
        });
    }

    #[test]
    fn logout_still_releases_a_clean_held_lock() {
        // The veto is about stranded work, not about logout: a CLEAN edit session
        // has nothing to protect, so logging out should still hand the server mutex
        // back rather than leave it held for every other editor.
        //
        // `with_isolated_config` for the same reason as its sibling above: logout
        // clears the rehydrate seed, which is a config WRITE.
        crate::test_helpers::with_isolated_config(|| {
            // Answers that the fixture account holds the lock, and counts only the
            // release itself (the holder read before it is not one).
            let (base, _listener, hits) = answering_counting_stub(REMOTE_ID);
            let (_temp, state) = editing_fixture(&base);
            assert!(
                !state.speleodb_open_link().expect("link").has_unpushed_edits,
                "fixture starts clean"
            );

            let _ = state.speleodb_logout();

            assert_eq!(
                hits.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "a clean held lock is still released on logout"
            );
            assert!(!state.edit_enabled(), "and the edit gate drops either way");
        });
    }

    /// [`release_stub`]'s answer plus [`counting_stub`]'s counter.
    ///
    /// The two existing stubs each do exactly one half - `release_stub` answers 200
    /// but records nothing, `counting_stub` records but never answers, so the client
    /// always sees a failure - and the "a SUCCESSFUL release leaves no warning" case
    /// needs both at once: without the answer the release fails, and without the count
    /// the test passes just as happily if no request was ever made.
    ///
    /// Every answer says the fixture account (`u@x`) holds the lock, and only the
    /// release requests are counted: a release asks who holds the lock before it
    /// may send one (lock#4), and that read is not the release.
    ///
    /// The count is incremented BEFORE the response goes out, so a test that asserts
    /// on it after a blocking release has no race to lose.
    fn answering_counting_stub(
        project_id: &str,
    ) -> (
        String,
        std::net::TcpListener,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::io::{Read as _, Write as _};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let body =
            format!(r#"{{"id":"{project_id}","name":"Remote","active_mutex":{{"user":"u@x"}}}}"#);
        // The release's reply: the project after the release, with no holder.
        let released = format!(r#"{{"id":"{project_id}","name":"Remote"}}"#);
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let accept_on = listener.try_clone().expect("clone listener");
        let counter = std::sync::Arc::clone(&hits);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = accept_on.accept() {
                // Drain the request until the socket goes quiet, then answer.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut req = Vec::new();
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&chunk[..n]);
                }
                let text = String::from_utf8_lossy(&req);
                let line = text.lines().next().unwrap_or_default();
                let release = line.starts_with("POST") && line.contains("/release");
                if release {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                let body = crate::core::state::tests_support::sdk_fixture(if release {
                    &released
                } else {
                    &body
                });
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), listener, hits)
    }

    /// A loopback server that answers the FIRST `projects/<id>/` read with the
    /// mutex held by us and every later one with `later_holder` - so a test can
    /// model the world changing between the status refresh's own holder read and
    /// the re-read that actually opens the gate. Requests for the revision
    /// topology get an empty body (the fold degrades to `Unknown`, which is not
    /// what these tests are about).
    fn holder_changes_after_the_first_read(
        project_id: &str,
        later_holder: Option<&str>,
    ) -> (String, std::net::TcpListener) {
        use std::io::{Read as _, Write as _};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let accept_on = listener.try_clone().expect("clone listener");
        let first = format!(
            r#"{{"id":"{project_id}","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{{"user":"u@x"}}}}"#
        );
        let later = match later_holder {
            Some(user) => format!(
                r#"{{"id":"{project_id}","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{{"user":"{user}"}}}}"#
            ),
            None => format!(r#"{{"id":"{project_id}","name":"Remote"}}"#),
        };
        std::thread::spawn(move || {
            let project_reads = AtomicUsize::new(0);
            while let Ok((mut stream, _)) = accept_on.accept() {
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut request = Vec::new();
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..n]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = if String::from_utf8_lossy(&request).contains("/revisions/") {
                    "{}".to_owned()
                } else if project_reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    first.clone()
                } else {
                    later.clone()
                };
                let body = crate::core::state::tests_support::sdk_fixture(&body);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), listener)
    }

    #[test]
    fn workspace_status_never_opens_editing_even_when_the_lock_is_ours() {
        // Ownership alone cannot prove that this copy matches server revisions.
        let (base, _listener) = holder_changes_after_the_first_read(REMOTE_ID, Some("u@x"));
        let (_temp, state) = editing_fixture(&base);
        state.set_edit_enabled(false);
        assert!(!state.edit_enabled(), "fixture opens read-only");

        let status = state.speleodb_link_status().expect("status");

        assert!(
            !status.edit_enabled,
            "status cannot resume an editing session"
        );
        assert!(!state.edit_enabled(), "the gate remains shut");
    }

    #[test]
    fn the_refresh_leaves_the_gate_shut_when_the_mutex_went_away_mid_refresh() {
        // Round 6 (P1): the refresh's holder read is one of several requests, and a
        // captured or janitor release for this same project can complete before
        // the gate opens. That release's reclaim check finds no claim record (the
        // refresh never minted one) and no live edit session (the gate is still shut
        // - which is why the refresh is here at all), so it rightly frees the mutex.
        // Opening the gate from the earlier read then left a session editing a
        // remote project with no server lock behind it and nothing recording one.
        //
        // The stub is that release: the mutex is ours on the read that decides to
        // ask, and gone by the re-read that would open the gate.
        let (base, _listener) = holder_changes_after_the_first_read(REMOTE_ID, None);
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock().expect("session lock") = authed_session_at(&base);
        state
            .speleodb_mutate_link(|link| link.endpoint = base.clone())
            .expect("bind synthetic endpoint");

        let status = state.speleodb_link_status().expect("status");

        assert!(
            !status.edit_enabled,
            "the mutex was freed before the gate opened, so there is no lock to \
             resume editing under"
        );
        assert!(
            !state.edit_enabled(),
            "and the report is not hiding a gate that was opened anyway"
        );
    }

    /// The `.stalx` of a project we hold the SpeleoDB write lock on can be DELETED, and
    /// that has to hand the lock back too.
    ///
    /// `close_project_impl` releases before it issues `Command::CloseProject`, but
    /// `delete_project` tears the session down through the state layer's
    /// `close_open_project_locked` and so never goes near that command. The lock was
    /// therefore left held while the `.stalx` - and the `speleo_link` row inside it that
    /// is the only local record of the lock - was destroyed, and the frontend cleared
    /// its link state. Nothing local could reach that mutex again: every other diver
    /// stayed locked out of the project until the server timed it out.
    ///
    /// A snapshot taken after the teardown cannot fix it either, which is what makes the
    /// ordering in `delete_project` load-bearing: the teardown drops the database
    /// handle, so `speleodb_open_link` would answer `NoRemoteProject` and the release
    /// would no-op exactly as before.
    ///
    /// Asserted by CONTRADICTION on a counting stub: a release that went out is a hit.
    /// That stub never answers, so the release also FAILS - which is the other half of
    /// the contract here, and free: the delete must still report `success` (the file is
    /// gone; failing would strand a zombie recents row) with the failure demoted to a
    /// warning, because a lock whose local record has just been destroyed is one only
    /// the held-lock prompt can still clear.
    ///
    /// Under `with_isolated_config` because the fixture's `open_project` and the delete
    /// itself both WRITE the config (the recent-projects list).
    #[test]
    fn deleting_the_open_remote_project_releases_its_held_lock() {
        crate::test_helpers::with_isolated_config(|| {
            let (base, _listener, hits) = counting_stub();
            let (_temp, state) = editing_fixture(&base);
            let path = state
                .open_project_path()
                .expect("precondition: the fixture is open and saved");
            assert!(
                state.edit_enabled(),
                "precondition: this session holds the write lock"
            );

            let result = state.delete_project(path.clone());

            assert_eq!(
                hits.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "deleting the project we hold the lock on must release it - otherwise \
                 the server mutex stays held with nothing local left to release it from"
            );
            assert!(!path.exists(), "the .stalx really was deleted");
            assert!(
                result.success,
                "the file is gone, so the delete succeeded overall: {:?}",
                result.error
            );
            assert_eq!(
                result.session_closed,
                Some(true),
                "and the session was torn down"
            );
            assert!(
                result
                    .warnings
                    .iter()
                    .any(|w| w.code == "delete_project_lock_release_failed"),
                "the stub never answers, so the release failed - which must surface as \
                 a non-fatal warning, not a failed delete, got {:?}",
                result.warnings
            );
        });
    }

    /// A release that LANDS leaves no lock warning behind, and a delete with no lock to
    /// release issues no request at all.
    ///
    /// The negative half of the test above, in the two directions that matter: the
    /// warning is a function of the release OUTCOME rather than something the delete
    /// emits unconditionally, and the delete does not go near the network for a project
    /// this session holds nothing on (the read-only fixture never Started editing).
    #[test]
    fn a_delete_only_warns_about_a_lock_release_that_failed() {
        crate::test_helpers::with_isolated_config(|| {
            // (1) Lock held, stub answers 200: the release goes out AND succeeds.
            let (base, _listener, hits) = answering_counting_stub(REMOTE_ID);
            let (_temp, state) = editing_fixture(&base);
            let path = state.open_project_path().expect("fixture is open");

            let result = state.delete_project(path.clone());

            assert_eq!(
                hits.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "the release request was made"
            );
            assert!(result.success, "delete succeeded: {:?}", result.error);
            assert!(
                !result
                    .warnings
                    .iter()
                    .any(|w| w.code == "delete_project_lock_release_failed"),
                "a release the server accepted must NOT be reported as a failed one, \
                 got {:?}",
                result.warnings
            );
            assert!(!path.exists(), "and the .stalx is gone");

            // (2) No lock held (remote, opened read-only, no claim): nothing to
            // release, so nothing is sent - not even the holder read - and nothing is
            // warned about.
            let (base, _listener2, hits) = counting_stub();
            let (_temp, state) = editing_fixture(&base);
            state.speleodb_forget_lock_acquired_for(REMOTE_ID);
            let path = state.open_project_path().expect("fixture is open");

            let result = state.delete_project(path);

            assert_eq!(
                hits.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "a read-only open holds no lock, so the delete must not POST a release"
            );
            assert!(result.success, "delete succeeded: {:?}", result.error);
            assert!(
                result.warnings.is_empty(),
                "and there is nothing to warn about, got {:?}",
                result.warnings
            );
        });
    }

    /// The delete sequence still ends the acquisition it captured AND still
    /// releases the server lock - with the clear now at SNAPSHOT time, under the
    /// same `project` guard that decided the file's identity (F8).
    ///
    /// The positive direction of the pair below: a stand-down that fired here
    /// too would pass the successor test on its own while stranding every
    /// deleted project's mutex until somebody released it by hand.
    #[test]
    fn the_delete_sequence_clears_its_own_claim_then_releases() {
        let (base, _listener, hits) = answering_counting_stub(REMOTE_ID);
        let (_temp, state) = editing_fixture(&base);
        // The real sequence: `delete_project` snapshots AND clears its own claim
        // under the `project` mutex, then releases once that mutex (and the
        // database handle) are out of the way.
        let params = state
            .speleodb_abandon_lock_release_params()
            .expect("precondition: the fixture holds a lock, so there is a snapshot");
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "precondition: the fixture recorded the acquisition it edits under"
        );
        // Clear + teardown under ONE `project` guard, exactly as `delete_project`
        // does. The teardown matters to the release below since round 5: a still
        // OPEN, still EDITING session on the same project is itself grounds to
        // stand down (its claim), so a fixture that
        // skipped the close would model a world the real sequence never produces.
        {
            let mut open = state.project.lock().unwrap_or_else(|e| e.into_inner());
            state.speleodb_forget_lock_acquired_for(&params.project_id);
            state.close_open_project_locked(&mut open);
        }

        state
            .speleodb_release_captured_lock(&params)
            .expect("the stub answers 200");

        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "with no claim in the slot and no live session on the project there \
             is no successor to stand down for, so the release request went out"
        );
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "and the claim the delete cleared at snapshot time stays gone"
        );
    }

    /// A delete that cannot destroy the file must not have destroyed the only
    /// pointer to the lock either.
    ///
    /// The claim is cleared at SNAPSHOT time, under the guard that decided the
    /// file's identity (F8), and only afterwards is the file handed to the OS.
    /// When that fails nothing has been destroyed: the lock is still ours, still
    /// guarding a project still on disk. But the session HAS been closed by
    /// then, so no status refresh will ever look at that mutex again, and this
    /// return does not reach the deferred release either - a claim dropped here
    /// leaves the mutex with nothing at all pointing at it.
    #[cfg(unix)]
    #[test]
    fn a_delete_that_cannot_trash_the_file_keeps_its_lock_claim() {
        use std::os::unix::fs::PermissionsExt as _;

        let (base, _listener, hits) = counting_stub();
        let (_temp, state) = editing_fixture(&base);
        let path = state.open_project_path().expect("fixture is open");
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "precondition: the fixture holds a lock it recorded taking"
        );

        // Deny writes on the PARENT so the unlink is refused while the file
        // itself still stats perfectly well.
        let dir = path.parent().expect("parent").to_path_buf();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
            .expect("make the directory read-only");
        let probe = dir.join(".writable");
        let running_as_root = std::fs::write(&probe, b"x").is_ok();
        let _ = std::fs::remove_file(&probe);
        let result = (!running_as_root).then(|| state.delete_project(path.clone()));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore the directory");

        let Some(result) = result else {
            // Root ignores the permission bits, so there is no denied unlink to
            // test with.
            return;
        };
        assert!(!result.success, "the delete failed");
        assert!(path.exists(), "and the file is still there");
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "so its lock claim must be back - it is the only local record of a \
             mutex this process still holds, over a project that still exists"
        );
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "and nothing was released: the project was never destroyed"
        );
    }

    /// A claim naming the SAME project at release time is a SUCCESSOR's - the
    /// deleting session's own claim was cleared at snapshot time - and the
    /// captured release must stand down for it: the successor inherited the very
    /// mutex a POST here would free (the server treats a same-account re-acquire
    /// as a no-op), which is F8's "stale release frees the successor's lock".
    #[test]
    fn a_deferred_release_stands_down_for_a_successor_claim_on_the_same_project() {
        let (base, _listener, hits) = answering_counting_stub(REMOTE_ID);
        let (_temp, state) = editing_fixture(&base);
        let params = state
            .speleodb_abandon_lock_release_params()
            .expect("precondition: the fixture holds a lock, so there is a snapshot");
        state.speleodb_forget_lock_acquired_for(&params.project_id);
        // A successor Start Editing on the same remote project commits its claim
        // inside the gap between the snapshot and the deferred POST.
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));

        state
            .speleodb_release_captured_lock(&params)
            .expect("standing down is a success - the lock is accounted for");

        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the release must NOT go out over a successor's claim"
        );
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "and the successor's claim is not the deleter's to clear"
        );
    }

    /// ...but it must not erase an acquisition that landed in the gap it left open.
    ///
    /// `speleodb_release_captured_lock` is the one release that clears the record at a
    /// different moment than it decided to release: `delete_project` drops the
    /// `project` mutex before the HTTP call, and inside that window another SpeleoDB
    /// project can be opened and Started editing - which records ITSELF in
    /// `lock_acquired_here`. The clear used to be unconditional, so it blanked the new
    /// project's record while releasing the OLD project's lock. Nothing looks broken
    /// (the new lock is still held, still editable), but its acquisition PROVENANCE is
    /// gone - and that is the whole licence for the authorship shortcut in
    /// `speleodb_remote_state`: without `we_hold_the_lock`, `status_verdict` refuses to
    /// explain a commit in the BASE..tip gap as our own landed push, so THIS process's
    /// own next push comes back as a divergence / self-conflict on a project nobody
    /// else touched, with the status mark routing the next click into the divergence
    /// dialog.
    ///
    /// The second assertion is the other half of the contract, and the reason the fix
    /// is a compare-and-clear rather than a skip: keeping the interloper's record is
    /// not a licence to strand the deleted project's server mutex.
    #[test]
    fn a_deferred_release_leaves_another_projects_acquisition_alone() {
        const GAP_ID: &str = "22222222-2222-2222-2222-222222222222";

        let (base, _listener, hits) = answering_counting_stub(REMOTE_ID);
        let (_temp, state) = editing_fixture(&base);
        let params = state
            .speleodb_abandon_lock_release_params()
            .expect("precondition: the fixture holds a lock, so there is a snapshot");
        assert_eq!(
            params.project_id, REMOTE_ID,
            "precondition: the snapshot names the project being deleted"
        );
        // The delete's own clear + teardown, as in the positive test above: the
        // deleted project's session must be GONE before the deferred POST, or
        // its claim would (correctly) make the release stand down for it.
        {
            let mut open = state.project.lock().unwrap_or_else(|e| e.into_inner());
            state.speleodb_forget_lock_acquired_for(&params.project_id);
            state.close_open_project_locked(&mut open);
        }
        // Exactly what a successful Start editing on the NEXT project does, in the
        // window `delete_project` opens by releasing the `project` mutex first.
        state.speleodb_note_lock_acquired(Some(GAP_ID));

        state
            .speleodb_release_captured_lock(&params)
            .expect("the stub answers 200");

        assert!(
            state.speleodb_lock_acquired_here(GAP_ID),
            "releasing the DELETED project's lock must leave the record the new \
             project's Start editing wrote: erasing it loses the authorship evidence \
             for a lock we still hold, and our own next landed push then reports as a \
             divergence / self-conflict"
        );
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "and the deleted project's lock is STILL released - the record is what is \
             conditional here, never the release"
        );
    }

    #[test]
    fn release_lock_rejects_a_non_uuid_id_before_any_network() {
        // Security (trust boundary): speleodb_release_lock takes an FE-supplied id
        // that is NOT routed through speleodb_validated_link, so it must reject a
        // path/route-traversal id itself, BEFORE touching the token or the URL. A
        // fresh (logged-out) state proves validation runs first: an invalid id fails
        // with InvalidProjectId, not the NotAuthenticated we'd get past validation.
        let state = AppState::new().expect("state");
        let err = state
            .speleodb_release_lock("../../etc/passwd".to_owned())
            .expect_err("a traversal id must be rejected");
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("invalid") && msg.contains("project id"),
            "expected an invalid-project-id rejection, got: {}",
            err.message
        );
    }

    /// A session seeded with an in-memory token (synthetic credentials). Marked
    /// already-rehydrated so it behaves like a live, established session.
    fn authed_session() -> SpeleoDbSession {
        SpeleoDbSession {
            endpoint: Some("https://x/".to_owned()),
            email: Some("u@x".to_owned()),
            token_store: TokenStore::from_in_memory_token("tok-1"),
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
        }
    }

    #[test]
    fn session_status_reflects_auth() {
        let status = authed_session().status();
        assert!(status.logged_in);
        assert_eq!(status.user_email.as_deref(), Some("u@x"));

        let status = SpeleoDbSession::default().status();
        assert!(!status.logged_in);
        assert_eq!(status.user_email, None);
    }

    #[test]
    fn forget_clears_session_and_token() {
        let mut session = authed_session();
        assert!(session.status().logged_in);
        session.forget();
        let status = session.status();
        assert!(!status.logged_in);
        assert_eq!(status.user_email, None);
        assert!(session.active_token().is_err());
    }

    /// Apply the 401 auto-drop decision to a session the way
    /// `speleodb_handle_auth` does (predicate → full clear), so the C5 effect is
    /// asserted without constructing a full `AppState`. Returns the session for
    /// post-condition checks. [Integration] the predicate now keys off
    /// `map_status(endpoint, status) == ReLogin`; these tests use a read endpoint
    /// (`ListProjects`) where 401→ReLogin, 403→ReadOnly, 5xx→Retry.
    fn apply_auth_drop(session: &mut SpeleoDbSession, result: &Result<(), SpeleoDbError>) {
        if auth_error_drops_token(Endpoint::ListProjects, result) {
            session.forget();
        }
    }

    #[test]
    fn whoami_probe_decision_distinguishes_dead_from_alive() {
        // The pure half of the ambiguous-403 disambiguation: a whoami 403/401
        // (→ ReLogin on the User endpoint) means the token is dead; a whoami
        // success means it is alive (so a resource 403 was a real permission
        // denial - keep it, D-B4); a 5xx / transport probe is inconclusive → not
        // dead (never drop on a flaky probe).
        assert!(whoami_indicates_dead_token(&Err(SpeleoDbError::Status {
            status: 403,
            message: "forbidden".to_owned(),
        })));
        assert!(whoami_indicates_dead_token(&Err(SpeleoDbError::Status {
            status: 401,
            message: "unauthorized".to_owned(),
        })));
        assert!(!whoami_indicates_dead_token(&Ok(
            "user@example.com".to_owned()
        )));
        assert!(!whoami_indicates_dead_token(&Err(SpeleoDbError::Status {
            status: 503,
            message: "server".to_owned(),
        })));
        assert!(!whoami_indicates_dead_token(&Err(
            SpeleoDbError::Transport {
                cause: TransportCause::Timeout,
                detail: "timed out".to_owned(),
            }
        )));
    }

    #[test]
    fn auth_handler_drops_token_on_401() {
        let mut session = authed_session();
        assert!(session.status().logged_in);
        let result: Result<(), SpeleoDbError> = Err(SpeleoDbError::Status {
            status: 401,
            message: "invalid token".to_owned(),
        });
        assert!(auth_error_drops_token(Endpoint::ListProjects, &result));
        apply_auth_drop(&mut session, &result);
        // Full clear: session is logged out and the token is gone from every tier.
        assert!(!session.status().logged_in);
        assert!(session.active_token().is_err());
    }

    #[test]
    fn auth_handler_keeps_token_on_403() {
        // 403 is NEVER a drop (D-B4: read-only/fence per-endpoint meaning,
        // robust to a DRF 401→403 downgrade).
        let mut session = authed_session();
        let result: Result<(), SpeleoDbError> = Err(SpeleoDbError::Status {
            status: 403,
            message: "forbidden".to_owned(),
        });
        assert!(!auth_error_drops_token(Endpoint::ListProjects, &result));
        apply_auth_drop(&mut session, &result);
        assert!(session.status().logged_in, "403 must not drop the session");
    }

    #[test]
    fn auth_handler_keeps_token_on_2xx_and_5xx() {
        // A successful call and a 5xx both leave the session intact (writes never
        // auto-drop; reads surface for manual retry).
        let ok: Result<(), SpeleoDbError> = Ok(());
        assert!(!auth_error_drops_token(Endpoint::ListProjects, &ok));

        for status in [500u16, 502, 503] {
            let mut session = authed_session();
            let result: Result<(), SpeleoDbError> = Err(SpeleoDbError::Status {
                status,
                message: "server error".to_owned(),
            });
            assert!(!auth_error_drops_token(Endpoint::ListProjects, &result));
            apply_auth_drop(&mut session, &result);
            assert!(
                session.status().logged_in,
                "HTTP {status} must not drop the session"
            );
        }
        // A transport error also never drops.
        let mut session = authed_session();
        let result: Result<(), SpeleoDbError> = Err(SpeleoDbError::Transport {
            cause: TransportCause::Timeout,
            detail: "timed out".to_owned(),
        });
        assert!(!auth_error_drops_token(Endpoint::ListProjects, &result));
        apply_auth_drop(&mut session, &result);
        assert!(session.status().logged_in);
    }

    #[test]
    fn validate_project_id_accepts_uuid_rejects_traversal() {
        // A real SpeleoDB project id (UUID) passes.
        assert!(validate_project_id("11111111-1111-1111-1111-111111111111").is_ok());
        // Anything that could escape the cache dir or reshape the request URL is
        // rejected before it reaches the path join / URL interpolation.
        for bad in [
            "",
            "..",
            "../../etc/passwd",
            "/etc/cron.d/evil",
            "a/b",
            "11111111-1111-1111-1111-111111111111/../../etc",
            "not-a-uuid",
        ] {
            assert!(
                matches!(
                    validate_project_id(bad),
                    Err(SpeleoDbError::InvalidProjectId)
                ),
                "expected {bad:?} to be rejected"
            );
        }
    }

    /// `Uuid::parse_str` takes four more spellings of one project than SpeleoDB
    /// sends; each would be a different cache directory, request path or link row
    /// for the same project. Only the canonical one passes, and that is what the
    /// SDK prints for an id the server sends.
    #[test]
    fn validate_project_id_accepts_only_the_canonical_spelling() {
        let canonical = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert!(validate_project_id(canonical).is_ok());
        let parsed = uuid::Uuid::parse_str(canonical).expect("a UUID");
        assert_eq!(parsed.to_string(), canonical, "the SDK's spelling");
        for other in [
            "0F8FAD5B-D9CB-469F-A165-70867728950E",
            "0f8fad5b-D9CB-469f-a165-70867728950e",
            "{0f8fad5b-d9cb-469f-a165-70867728950e}",
            "urn:uuid:0f8fad5b-d9cb-469f-a165-70867728950e",
            "0f8fad5bd9cb469fa16570867728950e",
        ] {
            assert!(
                uuid::Uuid::parse_str(other).is_ok(),
                "{other:?} is a UUID spelling"
            );
            assert!(
                matches!(
                    validate_project_id(other),
                    Err(SpeleoDbError::InvalidProjectId)
                ),
                "expected {other:?} to be rejected"
            );
        }
    }

    /// The ids a crafted `.stalx` could carry in its `speleo_link` row to steer a
    /// cache write out of the cache.
    const CRAFTED_IDS: [&str; 6] = [
        "../../x",
        "..",
        "/tmp/stalagmight-escape",
        "a/b",
        "",
        "11111111-1111-1111-1111-111111111111/../../x",
    ];

    /// The cache path validates its own id, for every caller (the pull, the
    /// revision restore, the push snapshot): it can never name a file outside the
    /// cache root, whatever the id says.
    #[test]
    fn the_cache_path_refuses_an_id_that_is_not_a_project_uuid() {
        use crate::core::state::speleodb_local_state::{cache_path_in, validated_cache_path_in};
        let state = AppState::new().expect("state");
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("speleodb");
        for bad in CRAFTED_IDS {
            assert!(
                matches!(
                    state.speleodb_cache_path(bad),
                    Err(SpeleoDbError::InvalidProjectId)
                ),
                "{bad:?}"
            );
            assert!(
                matches!(
                    validated_cache_path_in(&root, bad),
                    Err(SpeleoDbError::InvalidProjectId)
                ),
                "{bad:?}"
            );
        }
        let id = "11111111-1111-1111-1111-111111111111";
        let path = validated_cache_path_in(&root, id).expect("a project UUID");
        assert_eq!(path, cache_path_in(&root, id));
        assert!(path.starts_with(&root));
    }

    /// The push snapshot is written beside the cache path built from the OPEN
    /// file's link id. A crafted id is refused before that path exists, even when
    /// it reached the open project some way other than the load (which refuses it,
    /// see `speleodb_foreign`).
    #[test]
    fn the_push_snapshot_path_refuses_a_crafted_link_id() {
        let temp = TempDir::new().expect("temp");
        for bad in CRAFTED_IDS {
            let path = temp.path().join(format!("crafted-{}.stalx", bad.len()));
            let mut db = Database::open(&path).expect("open db");
            db.save_project(&Project::new("Crafted".to_owned(), None))
                .expect("save project");
            let link = SpeleoLink {
                endpoint: "https://sdb.invalid".to_owned(),
                project_id: bad.to_owned(),
                project_name: "Crafted".to_owned(),
                base_commit_sha: Some("abc123".to_owned()),
                has_unpushed_edits: true,
                last_edit_at: None,
                last_verify_at: None,
                undo_mode: None,
            };
            db.save_speleo_link(Some(&link)).expect("save link");
            let state = AppState::new().expect("state");
            state.set_project_for_tests(Project::new("Crafted".to_owned(), None));
            state.set_database_for_tests(db);
            let generation = state.speleodb_session_generation();

            // The verbs read the link through the validated snapshot first...
            let done = state
                .speleodb_done_acknowledged(Some("synthetic".to_owned()), false)
                .expect_err("Done refuses a crafted link id");
            assert_eq!(
                done.message,
                local_dto(SpeleoDbError::InvalidProjectId).message,
                "{bad:?}"
            );
            // ...and the upload itself refuses it where it builds the snapshot path.
            let upload = state
                .speleodb_upload(
                    "https://sdb.invalid",
                    "tok",
                    false,
                    &link,
                    None,
                    false,
                    generation,
                )
                .expect_err("the upload refuses a crafted link id");
            assert_eq!(
                upload.message,
                local_dto(SpeleoDbError::InvalidProjectId).message,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn resolve_endpoint_cleans_pasted_values() {
        // #675: invisible characters around a pasted override or env value are
        // stripped, and one that cleans to nothing falls through.
        assert_eq!(
            resolve_endpoint(Some("\u{200B} https://override/ \u{FEFF}"), None, None),
            Some("https://override/".to_owned())
        );
        assert_eq!(
            resolve_endpoint(Some("\u{200B}"), Some("\u{00A0}https://env\u{00A0}"), None),
            Some("https://env".to_owned())
        );
    }

    #[test]
    fn resolve_endpoint_precedence_and_trimming() {
        // Override wins over env and config (trailing slash is left intact -
        // normalization is account_key's job, not the resolver's).
        assert_eq!(
            resolve_endpoint(
                Some("https://override/"),
                Some("https://env/"),
                Some("https://config/")
            ),
            Some("https://override/".to_owned())
        );
        // A blank override falls through to env.
        assert_eq!(
            resolve_endpoint(Some("   "), Some("https://env/"), Some("https://config/")),
            Some("https://env/".to_owned())
        );
        // Blank override + env fall through to config, which is also trimmed.
        assert_eq!(
            resolve_endpoint(Some(""), Some("  "), Some("  https://config/  ")),
            Some("https://config/".to_owned())
        );
        // Nothing usable anywhere → None.
        assert_eq!(resolve_endpoint(None, None, None), None);
        assert_eq!(resolve_endpoint(Some(""), None, Some("   ")), None);
    }

    #[test]
    fn active_token_requires_endpoint_email_and_token() {
        // Fully populated → Ok((endpoint, token)).
        let mut session = SpeleoDbSession {
            endpoint: Some("https://x/".to_owned()),
            email: Some("u@x".to_owned()),
            token_store: TokenStore::from_in_memory_token("tok-1"),
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
        };
        let (endpoint, token) = session.active_token().expect("authenticated session");
        assert_eq!(endpoint, "https://x/");
        assert_eq!(token, "tok-1");

        let mut no_token = SpeleoDbSession {
            endpoint: Some("https://x/".to_owned()),
            email: Some("u@x".to_owned()),
            token_store: TokenStore::default(),
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
        };
        assert!(matches!(
            no_token.active_token(),
            Err(SpeleoDbError::NotAuthenticated)
        ));

        let mut no_email = SpeleoDbSession {
            endpoint: Some("https://x/".to_owned()),
            email: None,
            token_store: TokenStore::from_in_memory_token("tok-1"),
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
        };
        assert!(matches!(
            no_email.active_token(),
            Err(SpeleoDbError::NotAuthenticated)
        ));

        let mut no_endpoint = SpeleoDbSession {
            endpoint: None,
            email: Some("u@x".to_owned()),
            token_store: TokenStore::from_in_memory_token("tok-1"),
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
        };
        assert!(matches!(
            no_endpoint.active_token(),
            Err(SpeleoDbError::NotAuthenticated)
        ));
    }
    include!("speleodb_workspace_tests.rs");
}
