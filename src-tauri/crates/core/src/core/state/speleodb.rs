//! Orchestration tying the `crate::speleodb` primitives (HTTP client, token
//! store, dump handling) to `AppState`, config, and the app cache directory.
//!
//! The `commands::speleodb` IPC layer is a thin wrapper over these methods.
//!
//! What lives here (the whole single-writer hand-off, #182):
//! - **session**: login / logout / status, the lazy silent rehydration that
//!   revives a keychain token after a restart, and the centralized 401 auto-drop.
//! - **pull**: list projects, download + classify a dump, extract the `.stalx`
//!   into the app cache (see `speleodb_cache` for the replace chokepoint), adopt
//!   an Ariane-only project.
//! - **lock + edit gate**: acquire on Start editing (with the reconcile verdict),
//!   optimistic force-edit without a lock, periodic re-verify, and the
//!   release paths (Done / logout / close / app-exit / "release all").
//! - **take-remote**: abandoning local work for the server's copy.
//! - **persistence**: the `speleo_link` row that makes a project remote (and
//!   `speleodb_unlink`, which makes it plain again).
//! - **project admin**: metadata PATCH, publish (create-on-SDB), collaborators /
//!   teams, and project delete.
//!
//! Split out along this same axis, and no longer here: the **push** half - the
//! BASE-SHA fence, snapshot sanitize, upload, the divergence / unconfirmed
//! outcomes, and the Done / Push / Update verdicts - now lives in
//! [`super::speleodb_push`] (#275). Revision history and the View / Restore swap
//! live in [`super::speleodb_revisions`].

use std::path::PathBuf;

use super::speleodb_cache::LocalEditsPolicy;
// [#275] The push half (fence, snapshot, upload, Done/Push/Update) lives in
// `speleodb_push`; the lock + edit gate here still consults its outcome type and
// its empty-tip-tolerant head read.
use super::speleodb_push::{head_with_empty_retry, PushOutcome};

use super::AppState;
use crate::speleodb::auth::{account_key, SecretStoreError, TokenStore, TokenTier};
use crate::speleodb::client;
// [Track A] #186/D-B2: typed Endpoint + map_status + SpeleoDbError→SpeleoErrorDto
// conversion. [Integration] map_status/SpeleoDbAction also drive Track C's C5
// 401 auto-drop (auth_error_drops_token).
use crate::speleodb::contract::{dto_for_error, map_status, Endpoint};
use crate::speleodb::sync::{self, ReconcileVerdict};
use crate::speleodb::SpeleoDbAction;
use crate::speleodb::{
    KeychainReport, RemoteState, SpeleoDbError, SpeleoErrorDto, SpeleoLinkStatus,
    SpeleoLoginResult, SpeleoSessionStatus, StartEditingResult, TokenStorage,
};

/// Per-session SpeleoDB connection state held in [`AppState`]. The token lives
/// inside [`TokenStore`] (keychain / plaintext / in-memory tiers) and is never
/// exposed to the webview.
#[derive(Debug, Default)]
pub(crate) struct SpeleoDbSession {
    pub(super) endpoint: Option<String>,
    pub(super) email: Option<String>,
    pub(super) token_store: TokenStore,
    /// [Track C] (#188, C4): set once per process after the lazy silent startup
    /// rehydration runs (whether it found a token or not), so an empty/failed
    /// `load()` doesn't re-hit the keychain on every authed call.
    rehydrate_attempted: bool,
    /// Every write lock THIS process took and has not handed back: the acquisition
    /// provenance the authorship shortcut in [`AppState::speleodb_remote_state`]
    /// hangs on. Deliberately process-local and never persisted - a lock left over
    /// from an earlier run, or one held by a sibling session on the same account,
    /// is no evidence that THIS process pushed anything.
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
    /// Read-modify-written through exactly four sites, never by a field assignment
    /// anywhere else, because each such pair has to happen under ONE session-lock
    /// acquisition - two acquisitions leave a gap a fresh acquisition can land in and
    /// be erased from (each site's doc says which gap it is closing):
    /// [`AppState::speleodb_note_lock_acquired`] (record, or clear this account's),
    /// `AppState::speleodb_forget_lock_acquired_if_gate_shut` (clear if the edit gate
    /// is shut), [`AppState::speleodb_forget_lock_acquired_for`] (clear the one that
    /// names a given project), and the stranded-claim janitor
    /// (`AppState::speleodb_janitor_stranded_claim`, which additionally holds the
    /// `project` mutex over its decision and the lock-ops mutex over its
    /// restore-on-failure, so its writes cannot cross a live session's).
    ///
    /// Every one of those writes is scoped to the LIVE account. Dropping another
    /// account's claim is the one irreversible mistake available here: a claim kept
    /// too long costs at most a redundant release the server answers idempotently,
    /// while a claim dropped early strands a server mutex until its own timeout.
    ///
    /// `pub(super)` rather than private only because the last of those lives with the
    /// rest of the split delete release in [`super::speleodb_lock_release`], and a
    /// sibling module cannot see a private field. Nothing else in `core::state` may
    /// touch it.
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
    /// [#576 SDB-POLL-D1] An acquire this process sent whose reply was lost and whose
    /// holder re-read failed too: the server may hold the lock for this account with
    /// no claim here to say so. Stamped with the account like a claim, so it can only
    /// be settled for the account that sent the acquire. Written and settled only in
    /// `speleodb_unconfirmed.rs`: the first successful status poll records the claim
    /// (our account holds the lock) or drops the mark (another holder, or none); a
    /// close drops it too. Never persisted.
    pub(super) unconfirmed_acquire: Option<super::speleodb_session::LockClaim>,
    /// [#258] Which dump entry the user picked, per project, for THIS session.
    ///
    /// Deliberately process-local and never persisted. The chosen entry is a
    /// property of the SERVER TREE at a commit, not of the local project, so a
    /// stored name can vanish under you and the re-ask path is needed regardless -
    /// persisting it would add a schema surface that buys nothing.
    ///
    /// It exists so the fast-forward verbs (Start editing / Update / Take remote)
    /// can honour a choice already made. They take no `entryPath` of their own, so
    /// without this a user who resolved the picker at open time would meet a dead
    /// end the moment they tried to edit - the very thing the picker exists to
    /// remove. A miss falls back to failing closed, which is safe: it never guesses.
    chosen_dump_entries: std::collections::HashMap<(String, String), String>,
    /// [#258] Which revision this project's candidate list was last OFFERED for.
    ///
    /// The sibling of `chosen_dump_entries`, and it closes that field's blind spot.
    /// Dating the REMEMBERED answer stops a choice being reused across a revision it
    /// never described - but an EXPLICIT `entryPath` skipped the check entirely, and
    /// that is the one the user just made. The picker shows a per-candidate size and
    /// timestamp precisely so the choice can be informed; it is a question about one
    /// tree, and the confirmation replays the verb, which resolves the tip again. So
    /// the answer to A was being applied to B, with only path membership between them
    /// - and membership is exactly what a surviving backup copy passes.
    ///
    /// Recorded here rather than round-tripped through the DTO because it need not
    /// leave the process: the offer and the reply are the same session, so the client
    /// never has to be trusted to hand it back, and no IPC surface widens.
    offered_dump_at: std::collections::HashMap<String, Option<String>>,
}

/// [#258] Why the remembered dump choices are keyed by REVISION as well as project.
///
/// The revision is half the answer, not decoration. A push uploads under the server's
/// canonical wire name and leaves every other `.stalx` in the tree alone, so a choice
/// that named any OTHER entry describes pre-push bytes the moment a revision lands -
/// and it does not matter whose push landed it. Ours is handled by
/// [`AppState::speleodb_revision_landed`]; a collaborator's was not, and that is the
/// dangerous half, because their work is what the stale entry would replace.
///
/// It lived in the VALUE first, one slot per project, and that quietly broke the
/// documented "a View is a non-destructive peek" contract: answering a viewed
/// revision's picker evicted the tip's answer, so the next Update fast-forwarded,
/// found the recall dated elsewhere, and - having no picker of its own - dead-ended on
/// "re-open the project". In the key the two answers coexist, and a choice can no
/// longer be dated wrongly because there is nowhere to put an undated one.
///
/// Keyed by revision rather than rebound to the upload target, because rebinding is
/// not available: the upload response carries the commit sha and the project, never
/// the stored path. Re-asking costs one more question, and only for someone whose
/// project is ambiguous at all.

/// [#258] Which revision a dump being classified came from, as far as the caller can
/// establish - the yardstick a [`DumpChoice`] is measured against.
///
/// `Unknown` is a real answer and it fails CLOSED: a choice that cannot be dated
/// cannot be shown to still apply, so nothing is recalled and nothing is recorded.
/// The cost is a question; the alternative is honouring a name on the strength of
/// nothing, which is the bug this type exists to close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DumpAt<'a> {
    /// This dump is the tree at `sha`.
    Revision(&'a str),
    /// The revision could not be resolved (a best-effort history fetch that failed,
    /// or a caller with nothing authoritative to offer).
    Unknown,
}

impl<'a> DumpAt<'a> {
    /// The usual spelling at a call site: an optional sha straight from the caller.
    pub(super) fn from_sha(sha: Option<&'a str>) -> Self {
        sha.map_or(Self::Unknown, Self::Revision)
    }
}

/// Pure endpoint-precedence resolver (extracted so it is unit-testable without
/// process env or an `AppState`): the first tier that is present and non-blank,
/// in order override → env → config, trimmed. Returns `None` if all are blank.
fn resolve_endpoint(
    override_endpoint: Option<&str>,
    env_endpoint: Option<&str>,
    config_endpoint: Option<&str>,
) -> Option<String> {
    [override_endpoint, env_endpoint, config_endpoint]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|e| !e.is_empty())
        .map(str::to_owned)
}

/// Reject any `project_id` that is not a plain SpeleoDB project UUID before it is
/// used as a filesystem path component or interpolated into a request URL. The
/// server keys projects by UUID, so anything else is invalid; refusing it closes
/// the path-traversal / route-manipulation vector at the IPC trust boundary (a
/// `..`, an absolute path, or an embedded separator could otherwise redirect the
/// cached `.stalx` write or the download URL). A parsed UUID has no separators,
/// `..`, or absolute prefix, so it is always a safe single path component.
pub(super) fn validate_project_id(project_id: &str) -> Result<(), SpeleoDbError> {
    uuid::Uuid::parse_str(project_id)
        .map(|_| ())
        .map_err(|_| SpeleoDbError::InvalidProjectId)
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
fn is_local_edits_refusal(err: &SpeleoDbError) -> bool {
    matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. })
}

/// [Track A] #186/D-B2: map a non-endpoint (local / session) `SpeleoDbError` to
/// the IPC `SpeleoErrorDto`. Local errors always fail closed to `Fatal`
/// regardless of endpoint, so the endpoint passed to [`dto_for_error`] is
/// irrelevant for them (`User` is an arbitrary, non-load-bearing choice).
pub(super) fn local_dto(err: SpeleoDbError) -> SpeleoErrorDto {
    dto_for_error(Endpoint::User, &err)
}

/// [#576 C4] A SpeleoDB verb refused by a core refusal (the edit gate, the crash
/// refusal, a refused save): its message as written and its own code, so the
/// frontend routes it as that refusal rather than as `SPELEODB_FATAL`.
pub(super) fn refused_by(err: crate::core::CommandError) -> SpeleoDbError {
    SpeleoDbError::refused_with(err.code, err.message)
}

/// [#576 B6-D1 / C4] A SpeleoDB verb refused because a newer Stalagmight saved the
/// open project: `reason` (`AppState::newer_file_refusal`) under
/// `NEWER_FILE_READ_ONLY`, the code a refused save of that file carries.
pub(super) fn newer_file_refused(reason: String) -> SpeleoDbError {
    SpeleoDbError::refused_with(crate::models::CommandErrorCode::NewerFileReadOnly, reason)
}

/// [#576 C4] A SpeleoDB verb refused because the save it runs first was: the
/// save's message as written (`fallback` when it has none) under the save's own
/// code. An unclassified save failure stays the verb's local failure
/// (`SPELEODB_FATAL`), as before.
pub(super) fn refused_by_save(save: crate::core::CommandResult, fallback: &str) -> SpeleoDbError {
    let message = save.error.unwrap_or_else(|| fallback.to_owned());
    match save.code {
        Some(code) => SpeleoDbError::refused_with(code, message),
        None => SpeleoDbError::refused(message),
    }
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
    /// active tier ([Track C] #188: keychain-first read populates the cache).
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

    /// Current session status for the dialog. `logged_in` means a usable token is
    /// held *locally* - it is not re-validated against the server here (a token
    /// revoked server-side is caught on the next real call by the 401 auto-drop
    /// handler, [Track C] #188 C5). When not logged in, neither the email nor the
    /// keychain flag is surfaced, so the status can never claim a keychain-backed
    /// token while reporting logged-out.
    ///
    /// `&mut self` because resolving the token may rehydrate + cache it.
    fn status(&mut self) -> SpeleoSessionStatus {
        let logged_in = self.active_token().is_ok();
        SpeleoSessionStatus {
            user_email: logged_in.then(|| self.email.clone()).flatten(),
            endpoint: logged_in.then(|| self.endpoint.clone()).flatten(),
            keychain_backed: logged_in && self.token_store.keychain_backed(),
            // Reporting only - reading these never touches the keychain, which is
            // what lets Settings show where the key lives without re-raising a
            // desktop unlock prompt every time it is opened.
            storage: logged_in
                .then(|| self.token_store.active_tier())
                .flatten()
                .map(TokenStorage::from),
            keychain: self.token_store.keychain_state().into(),
            logged_in,
        }
    }

    /// Best-effort form of [`Self::forget_reporting`]: the 401-drop sink ([Track C]
    /// #188, C5). A dead token must leave the session even when the keychain is
    /// flaky, so a delete failure is logged (inside the token store) and dropped
    /// here rather than propagated (#270).
    fn forget(&mut self) {
        let _ = self.forget_reporting();
    }

    /// Forget the local token (every tier: keychain + plaintext + in-memory) and
    /// clear the session, **reporting** a persistent-tier delete failure. No
    /// server-side call - logout is purely local (the DRF token is not revoked).
    ///
    /// The session is dropped unconditionally, including on the error path: the
    /// user asked to log out, so an `Err` means "logged out, but a credential may
    /// still be stored on this machine" - never "still logged in" (#270).
    pub(super) fn forget_reporting(&mut self) -> Result<(), SecretStoreError> {
        let cleared = match (self.endpoint.clone(), self.email.clone()) {
            (Some(endpoint), Some(email)) => self.token_store.clear(&endpoint, &email),
            _ => Ok(()),
        };
        self.endpoint = None;
        self.email = None;
        // The recorded "the keyring holds a token for us" belief is about the account
        // that just left, so it leaves with it - see
        // [`TokenStore::forget_keychain_belief`]. After the sweep above, which needs
        // it. This is what stops a login as a DIFFERENT account inheriting it through
        // the store the switch reuses.
        self.token_store.forget_keychain_belief();
        // [#258] The remembered dump picks belong to the account that just left: a
        // different user may not even be able to see those entries, and the next
        // session must re-derive the choice from what the server actually holds.
        self.chosen_dump_entries.clear();
        self.offered_dump_at.clear();
        cleared
    }

    /// [Track C] (#188, C4): lazy **silent** startup rehydration, run once on the
    /// first authed/session call. Seeds `endpoint`/`email` from config (resolved
    /// through the same precedence as login so `DEV_SPELEODB_URL` still wins) and
    /// wires `TokenStore::load` for the **first time** so a prior-run keychain (or
    /// plaintext) token survives an app restart. **No network, no prompt.**
    ///
    /// Idempotent: marks `rehydrate_attempted` so a failed/empty load doesn't
    /// re-hit the keychain every call, and short-circuits if a session is already
    /// established this run.
    ///
    /// `keychain_has_token` is the persisted belief ALREADY SCOPED to
    /// `(resolved_endpoint, config_email)` by the caller
    /// (`SpeleoDbConfig::keychain_belief_applies_to`) - never the raw config flag,
    /// which says nothing about whose entry it is until it is paired with the stamp.
    fn rehydrate(
        &mut self,
        resolved_endpoint: Option<String>,
        config_email: Option<String>,
        keychain_declined: bool,
        keychain_disabled: bool,
        keychain_has_token: bool,
    ) {
        if self.rehydrate_attempted || self.endpoint.is_some() {
            return;
        }
        self.rehydrate_attempted = true;
        // Apply the persisted refusal BEFORE the load below. This is the launch-time
        // nag fix: startup rehydration is what re-raised the desktop unlock prompt on
        // every single launch for someone who had already dismissed it once.
        self.token_store.set_keychain_declined(keychain_declined);
        self.token_store.set_keychain_disabled(keychain_disabled);
        // Do not OPEN the keyring to find out whether it has anything for us. On
        // Linux that read is the desktop unlock prompt, so a signed-out launch was
        // raising one purely to discover there was nothing to find. What the
        // keychain holds is recorded when we put it there, so the answer is already
        // on disk. A stale `true` costs ONE prompt and self-corrects on the miss -
        // durably: the load below lowers the store's belief on an `Ok(None)`, and
        // `AppState::speleodb_ensure_rehydrated` writes that correction back to the
        // config afterwards. Without that write-back the same stale `true` would be
        // re-read and the same prompt raised on every launch.
        if !keychain_has_token {
            self.token_store.set_keychain_declined(true);
        }
        // The same flag, read the other way round and for a different question. A
        // `true` here is a PREVIOUS login's successful keychain store, which is the
        // one thing that separates a keyring that is merely locked this launch from
        // a box that has no secret service at all - both fail every op and both
        // record `Unavailable`. Without it, turning the keyring off over a locked
        // keyring reported the entry emptied and cleared this very flag, sending the
        // next launch past a live bearer token that is still in there. See
        // `TokenStore::unreadable_keychain_may_hold`.
        self.token_store.note_keychain_has_token(keychain_has_token);

        let (Some(endpoint), Some(email)) = (resolved_endpoint, config_email) else {
            // No persisted account → stay logged-out (no keychain probe).
            return;
        };
        let endpoint = endpoint.trim().to_owned();
        let email = email.trim().to_owned();
        if endpoint.is_empty() || email.is_empty() {
            return;
        }

        self.endpoint = Some(endpoint.clone());
        self.email = Some(email.clone());
        // First wiring of load(): keychain-first read (then plaintext). On a hit
        // the token is cached + the active tier set; on a miss the session stays
        // seeded-but-unauthenticated and the next call surfaces NotAuthenticated.
        let _ = self.token_store.load(&endpoint, &email);
    }
}

/// The server `type` a project Stalagmight manages carries - the state adoption
/// exists to leave behind. Compared case-insensitively for the same reason
/// the Ariane project type is.
///
/// SpeleoDB generates a project's map only for types it has an exporter for, and
/// `stalagmight` is the one it maps from the `.stalx` (via `stalx-reader`, #593).
pub(super) const MANAGED_PROJECT_TYPE: &str = "stalagmight";

/// The managed type before SpeleoDB had a Stalagmight type. SpeleoDB never maps
/// an `other` project, so it is NOT treated as managed: the next landed push
/// (`type_needs_managed_label`) and an adoption's finalize both relabel it.
const LEGACY_MANAGED_PROJECT_TYPE: &str = "other";

/// The PATCH body that labels a project as Stalagmight-managed.
pub(super) fn managed_type_patch() -> String {
    serde_json::json!({ "type": MANAGED_PROJECT_TYPE }).to_string()
}

/// Is the server already calling this a project we manage?
///
/// Only [`MANAGED_PROJECT_TYPE`] counts: the legacy `other` still needs the
/// relabel. Deliberately NOT `!type_still_says_ariane`, which answers "yes" for
/// `compass`, `therion` and anything the API grows next. Adoption used that
/// inverse to decide it had nothing to do, so a project re-typed under it mid-run
/// was reported as flipped with the server left untouched.
pub(super) fn type_says_managed(server_kind: Option<&str>) -> bool {
    server_kind.is_some_and(|kind| kind.trim().eq_ignore_ascii_case(MANAGED_PROJECT_TYPE))
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
    fn speleodb_resolve_endpoint(&self, override_endpoint: Option<&str>) -> Option<String> {
        let env = std::env::var("DEV_SPELEODB_URL").ok();
        let config_endpoint = self.config.lock().speleodb.endpoint.clone();
        resolve_endpoint(
            override_endpoint,
            env.as_deref(),
            config_endpoint.as_deref(),
        )
    }

    /// [Track A] #187/D-B10: read the persisted `insecure_http` override under the
    /// config lock. It is threaded into every client call → `with_retry` →
    /// `tls::enforce`, where it relaxes ONLY the public-`http://` scheme block.
    pub(super) fn speleodb_insecure_http(&self) -> bool {
        self.config.lock().speleodb.insecure_http
    }

    /// [#213] Read the persisted `allow_force_edit` opt-in under the config lock.
    /// Gates `speleodb_force_edit`.
    pub(super) fn speleodb_allow_force_edit(&self) -> bool {
        self.config.lock().speleodb.allow_force_edit
    }

    /// [Track C] (#188, C4): lazy silent startup rehydration. Resolve the config
    /// endpoint (same precedence as login, sans a dialog override) + email
    /// **outside** the session lock to avoid a config↔session lock-order hazard,
    /// then let [`SpeleoDbSession::rehydrate`] seed + wire `load()` once. Called
    /// before locking in the authed/session entry points. No network, no prompt.
    ///
    /// It makes exactly ONE config write, and only in one direction: the
    /// keychain-belief write-back at the end, when the rehydrate read PROVED the
    /// keyring holds nothing for this account. See there for why it cannot be left
    /// to the live store alone, and why it is a compare-and-set rather than a write.
    pub(super) fn speleodb_ensure_rehydrated(&self) {
        // Cheap idempotency check first: if rehydration already ran (or a session
        // is live), skip the config read entirely.
        {
            let session = self.speleodb.lock();
            if session.rehydrate_attempted || session.endpoint.is_some() {
                return;
            }
        }

        let resolved_endpoint = self.speleodb_resolve_endpoint(None);
        // The whole block, not four fields: the write-back below compares what it is
        // about to overwrite against exactly what was read here, so the snapshot has
        // to BE the read.
        let persisted = self.config.lock().speleodb.clone();
        // Whether the recorded belief is a fact about the account this rehydrate is
        // going to key the keychain read by - the endpoint as well as the email, since
        // that is what the entry is keyed by. Signing in to a second server with the
        // same address used to inherit the first server's record here.
        let believed = match (resolved_endpoint.as_deref(), persisted.email.as_deref()) {
            (Some(endpoint), Some(email)) => {
                persisted.keychain_belief_applies_to(endpoint.trim(), email.trim())
            }
            // No account to key an entry by, so nothing recorded can be about one.
            _ => false,
        };

        let mut session = self.speleodb.lock();
        session.rehydrate(
            resolved_endpoint,
            persisted.email.clone(),
            persisted.keychain_declined,
            persisted.keychain_disabled,
            believed,
        );
        let disproved = !session.token_store.keychain_has_token();
        // Released before the write-back: the writer takes the config lock, and the
        // documented order is config-then-session.
        drop(session);

        // The read `rehydrate` just wired is the one keychain access that can PROVE
        // the entry is gone - `TokenStore::load`'s `Ok(None)`, which is what an entry
        // removed outside the app (seahorse, a reinstall) answers with. The store
        // lowers its own belief on it, but only the CONFIG survives the process, so
        // without this the next launch reads the stale `true` back and opens the
        // keyring again - on Linux that read IS the desktop unlock prompt - for the
        // same miss, every launch, forever. `rehydrate`'s "a stale `true` costs one
        // prompt and self-corrects on the miss" is true only because of this line.
        //
        // Written HERE rather than inside `TokenStore`, which is deliberately
        // config-agnostic (the 401 auto-drop reaches it with no config in hand). This
        // is the one place that already holds both halves: it read the persisted
        // belief to hand to `rehydrate`, and it owns the lock order.
        //
        // ONLY a lowering, and only from a belief actually read off the config and
        // found to be about THIS account. A failed config read hands `false` down, and
        // re-persisting that would record a removal nothing observed - the exact
        // mistake the flag was fixed for. And only under the compare-and-set the
        // writer makes against `persisted`: the lock was released above, so a login
        // can have landed a NEWER fact in between.
        if believed && disproved {
            self.speleodb_lower_keychain_belief_after_proof(&persisted);
        }
    }

    /// Resolve the active `(endpoint, token)` for an authenticated call, or
    /// [`SpeleoDbError::NotAuthenticated`] if there is no usable token.
    pub(super) fn speleodb_active_token(&self) -> Result<(String, String), SpeleoDbError> {
        self.speleodb_ensure_rehydrated();
        self.speleodb.lock().active_token()
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
        let mut session = self.speleodb.lock();
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
        // [Track C] (#188): rehydrate first so a restart with a stored keychain
        // token reports logged_in:true without a re-login (the headline fix).
        self.speleodb_ensure_rehydrated();
        // No network call, and nothing here fails: the `Result` is the IPC envelope.
        Ok(self.speleodb.lock().status())
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
        let link = match self.speleodb_open_link() {
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
                    exclude_geojson: false,
                    // [#303] A local project has no undo POLICY (the fixed window is
                    // unconditional there), so report the global default and leave the
                    // control - which only renders for a remote project - unbothered.
                    undo_mode: self.undo_mode_status(),
                    force_editing: self.force_editing(),
                    allow_force_edit: self.speleodb_allow_force_edit(),
                    project_id: None,
                });
            }
            Err(e) => return Err(local_dto(e)),
        };

        let edit_enabled = self.edit_enabled();
        // A shut gate means this process is not editing under a lock of its own, so
        // whatever acquisition it recorded is over: released, dropped by the 401
        // auto-drop or the lost-lock re-verify, or left behind by a close / project
        // switch / reopen. Drop the record HERE, before the verdict below reads it -
        // the re-derivation at the bottom of this function can reopen the gate from
        // nothing but the holder email, and it must never hand back an authorship
        // licence this process no longer holds. (All THREE in-place reload sites -
        // both fast-forward arms and the Restore half of `speleodb_restore_revision` -
        // re-record afterwards, because the reload shuts the gate for a moment while
        // the lock is still genuinely ours.)
        //
        // The clear re-checks the gate for itself rather than acting on the snapshot
        // above, which a fast-forward finishing underneath this poll makes stale - see
        // [`Self::speleodb_forget_lock_acquired_if_gate_shut`].
        if !edit_enabled {
            self.speleodb_forget_lock_acquired_if_gate_shut();
        }
        let has_unpushed_edits = link.has_unpushed_edits;

        // Best-effort network enrichment: the user's permission tier + lock holder
        // (get_project), and the remote-state reconcile verdict (revision topology,
        // the #202 freshness fold). Guard the request URL with the same project-id
        // validation as the edit verbs; on any miss (invalid id, no token,
        // transport/HTTP error) the fields degrade to None / Unknown - the local
        // truth still returns.
        //
        // `holder_read` separates "the server says nobody holds the lock" from "we
        // could not ask", which `lock_holder == None` alone conflates: only the
        // first may switch editing off (#576 B35(e), below).
        let mut holder_read = false;
        let (permission, lock_holder, remote_state, access_denied, server_error, exclude_geojson) =
            if validate_project_id(&link.project_id).is_ok() {
                match self.speleodb_active_token() {
                    Ok((endpoint, token)) => {
                        let insecure_http = self.speleodb_insecure_http();
                        match client::get_project(
                            &endpoint,
                            &token,
                            insecure_http,
                            &link.project_id,
                        ) {
                            Ok(p) => {
                                // Only now (the project is reachable) is the revision-
                                // topology fold worth a SECOND round trip. On every
                                // error arm below the remote state is Unknown anyway
                                // (get_revisions would fail the same way get_project
                                // just did), so skip the doomed extra GET (N5).
                                let lock_holder = p.active_mutex.map(|m| m.user);
                                holder_read = true;
                                // The lock holder is what licenses the poll's
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
                                (
                                    p.permission,
                                    lock_holder,
                                    remote_state,
                                    false,
                                    None,
                                    p.exclude_geojson,
                                )
                            }
                            // A definitive 403 means we are logged in but not a
                            // collaborator on this managed project - flag it so the
                            // UI can offer converting to a local file (fix 10).
                            Err(SpeleoDbError::Status { status: 403, .. }) => {
                                (None, None, RemoteState::Unknown, true, None, false)
                            }
                            // A 401 is a dead token (re-login) handled by the edit
                            // verbs; this passive probe must not nag about it.
                            Err(SpeleoDbError::Status { status: 401, .. }) => {
                                (None, None, RemoteState::Unknown, false, None, false)
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
                                false,
                            ),
                            // Transport / local error is offline - stay quiet.
                            Err(_) => (None, None, RemoteState::Unknown, false, None, false),
                        }
                    }
                    Err(_) => (None, None, RemoteState::Unknown, false, None, false),
                }
            } else {
                (None, None, RemoteState::Unknown, false, None, false)
            };

        // Invariant: holding the write lock means we may edit. If we hold it but the
        // gate is off - e.g. we reopened a project we'd locked but not released, and
        // the load re-derived the gate as read-only from is_remote - resume editing
        // now, so the project isn't stuck read-only under our own lock. (A same-
        // account concurrent session is warned about separately at open time.)
        //
        // The holder read above only decides whether to ASK: the gate itself opens
        // in `speleodb_reopen_gate_for_inherited_lock`, which re-reads the holder
        // and commits inside the lock-ops span. Opening it straight from the read
        // above let a captured release for this same project land in between and
        // free the mutex - leaving a session editing a remote project with no
        // server lock and no record of one.
        //
        // [#576 B35(e)-D2] And the converse: a lock-holding session whose lock the
        // server says is gone or someone else's stops editing. Same shape - the read
        // above only decides whether to ask; `speleodb_close_gate_for_lost_lock`
        // re-reads and closes inside the lock-ops span - and only on a SUCCESSFUL
        // read: offline, nothing is known about the lock and editing stays
        // first-class. A force-editor holds no lock on purpose and is left alone.
        let holds_lock =
            lock_held_by_me(lock_holder.as_deref(), self.speleodb_my_email().as_deref());
        let edit_enabled = if !edit_enabled && holds_lock {
            self.speleodb_reopen_gate_for_inherited_lock(&link)
        } else if edit_enabled && holder_read && !holds_lock && !self.force_editing() {
            self.speleodb_close_gate_for_lost_lock(&link);
            // Re-read rather than assume: the close declines when the re-read finds
            // the lock ours again, and something else may have shut the gate.
            self.edit_enabled()
        } else {
            edit_enabled
        };
        // [#576 SDB-POLL-D1] A successful holder read settles an acquire whose reply
        // was lost and whose re-read failed (`speleodb_unconfirmed`): ours - the claim
        // is recorded (the reopen above already did, when it opened the gate);
        // another holder or none - the mark is dropped.
        if holder_read {
            self.speleodb_settle_unconfirmed_acquire(&link.project_id, holds_lock);
        }

        Ok(SpeleoLinkStatus {
            is_remote: true,
            edit_enabled,
            has_unpushed_edits,
            // [#303] Mid-walk: a chain step has moved this copy off the tip. Reported RAW
            // so the frontend can combine it with its own optimistic dirty bit - a step onto
            // the tip leaves the copy clean, so a pre-combined flag was false at the only
            // moment the frontend polls.
            unwound: self.speleodb_unwound_from().is_some(),
            permission,
            lock_holder,
            remote_state,
            access_denied,
            server_error,
            exclude_geojson,
            // [#303] Resolved from the explicit project value. The global default is
            // consulted only while initializing a legacy NULL on open.
            undo_mode: self.undo_policy_for(Some(&link)).mode,
            // #213: report force-edit so the FE renders the big-red banner + offers
            // Reconnect. An online force-editor whose tip moved already surfaces via
            // `remote_state` (RemoteAhead/Conflict); offline it stays Unknown (skipped).
            force_editing: self.force_editing(),
            allow_force_edit: self.speleodb_allow_force_edit(),
            // #218: the open remote project's id, so the in-project menu can open its
            // revision history without a separate id lookup.
            project_id: Some(link.project_id.clone()),
        })
    }

    /// Best-effort remote-state (freshness fold, #202): resolve the server tip from
    /// the revision topology and project the BASE-vs-tip fence for the action
    /// button (T2c). Any failure → `Unknown` - the passive poll must never surface
    /// an error, and it is NOT routed through `handle_auth` (a transient 401 here
    /// must not auto-drop the session, mirroring [`Self::speleodb_lock_holder`]).
    ///
    /// The policy itself lives in [`sync::poll_verdict`] so every branch is
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
        // What licenses the authorship recovery in `poll_verdict`: a commit in the
        // BASE..tip gap can only be OUR unrecorded push if THIS process was the
        // single writer when it was made.
        //
        // The edit gate is NOT that evidence, which is why it is not read here.
        // `speleodb_link_status` re-derives the gate from the holder email itself, so
        // pairing the two only reads one signal twice: a second session on the same
        // account holds the real lock, the poll opens this process's gate because the
        // holder is "us", and the other session's commit gets explained away as our
        // own unrecorded push. What this asks instead is whether THIS process took
        // the lock on THIS project and is still editing under it - a claim only a
        // successful Start editing makes, cleared on every release and on the first
        // poll that finds the gate shut. A lock inherited from an earlier run, or one
        // a sibling session holds, licenses nothing: its claim (#576 SDB-ACCT-D1) is
        // an ADOPTED one, which licenses the release only (`speleodb_lock_taken_here`
        // skips it).
        //
        // A #213 force-edit session is excluded twice over: it records no acquisition
        // (it opens the gate precisely because it could NOT take a lock), and the flag
        // is re-checked here because force-edit can be entered after a release has
        // shut the gate but before the poll that clears the record has run.
        // (The frontend's `holdsWriteLock` is the same shape: `editing &&
        // !forceEditing`.)
        //
        // The gate is asked for TOO - not as evidence of authorship, but as a veto.
        // The record can outlive a gate that shut underneath an in-flight poll (a
        // concurrent same-path reopen re-derives the gate from `is_remote` and the
        // catch-all clear may not have run yet), and a session that may no longer
        // edit must not still be handed a licence to explain a gap commit away as
        // its own. Gate AND record AND holder, or no shortcut.
        let we_hold_the_lock = self.speleodb_authorship_licence(&link.project_id, lock_holder);
        match head_with_empty_retry(link.base_commit_sha.as_deref(), || {
            self.speleodb_server_head(endpoint, token, insecure_http, &link.project_id)
        }) {
            Ok((tip, commits)) => sync::poll_verdict(
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

    /// The authorship licence: whether a commit in the BASE..tip gap carrying our
    /// email may be explained away as THIS session's own unrecorded push. Record AND
    /// gate AND not force-editing AND the server naming our account as the holder -
    /// see [`Self::speleodb_remote_state`] for why each half is needed. The record
    /// must be an ACQUISITION: a lock "Continue editing" resumed licenses its
    /// release but not this (#576 RV4-R-S3).
    ///
    /// One helper for the poll and the push (#576 B32): `sync::poll_verdict` and
    /// `sync::plan_push` take this same bool, so the passive mark and the Done/Push
    /// click behind it decide a same-account gap the same way. The push used to pass
    /// nothing at all, and adopted a second session's commit the mark called a
    /// conflict.
    pub(super) fn speleodb_authorship_licence(
        &self,
        project_id: &str,
        lock_holder: Option<&str>,
    ) -> bool {
        self.speleodb_lock_taken_here(project_id)
            && self.edit_enabled()
            && !self.force_editing()
            && lock_held_by_me(lock_holder, self.speleodb_my_email().as_deref())
    }

    /// Record - or, with `None`, forget - the project whose write lock THIS process
    /// took and is editing under. The acquisition provenance read by
    /// [`Self::speleodb_remote_state`].
    ///
    /// Set in the two arms of [`Self::speleodb_start_editing`] that KEEP the lock
    /// they just acquired, right after the edit gate opens - and only where its
    /// pre-acquire probe found the mutex UNHELD, since re-acquiring a lock the
    /// account already holds is a server-side no-op that proves nothing (its other
    /// arms release the lock again where this call is what took it, and the publish
    /// push acquires and releases inside one call without ever opening an edit
    /// session, so neither records anything).
    /// Restored after the two other in-place reloads -
    /// [`Self::speleodb_update`]'s fast-forward arm and
    /// [`Self::speleodb_restore_revision`]'s Restore half - but only where a record
    /// already existed, never conjured from the gate alone (see there).
    /// Cleared by
    /// every release verb in this file, and - covering the release paths that live
    /// elsewhere, plus close / project switch / logout / take-remote - by the poll in
    /// [`Self::speleodb_link_status`], through
    /// [`Self::speleodb_forget_lock_acquired_if_gate_shut`].
    ///
    /// A poisoned session lock silently declines to record, which costs the shortcut
    /// rather than granting one: the same call also fails `speleodb_active_token`, so
    /// the poll has no lock holder to compare against either.
    ///
    /// Private, and deliberately so again. The split release in
    /// [`Self::speleodb_release_captured_lock`] used to reach in here for its clear,
    /// which was the whole of the bug [`Self::speleodb_forget_lock_acquired_for`] now
    /// exists to prevent: an UNCONDITIONAL clear run after a DEFERRED release erases
    /// whatever another project's acquisition wrote in the gap. Any caller that clears
    /// on behalf of one named project wants that helper, not this one; every caller of
    /// this one is in this file.
    /// The account is stamped on from the SAME session acquisition the claim is
    /// written into, so a claim can never name an account other than the one whose
    /// token the acquire used - see [`super::speleodb_session::LockClaim`] for why
    /// a bare project id was not enough.
    ///
    /// Scoped to THIS account, and - when recording - to THIS project. `Some(id)`
    /// replaces the account's entry for `id` and leaves its entries for other
    /// projects standing, because those are pointers to mutexes that may still be
    /// held: a janitor release that failed transiently leaves exactly such an
    /// entry, and taking a lock on a second project must not be what loses it.
    ///
    /// `None` means "this account holds nothing any more", which is true of
    /// exactly one caller - [`Self::speleodb_release_all_locks`], which frees
    /// every lock the account holds. The per-project releases use
    /// [`Self::speleodb_forget_lock_acquired_for`] instead: they hand back one
    /// mutex, so one claim is what they may end.
    ///
    /// Another account's claims are untouched in every direction: they are not
    /// this session's to drop, and each is the only local pointer to a mutex
    /// still held elsewhere.
    pub(super) fn speleodb_note_lock_acquired(&self, project_id: Option<&str>) {
        self.speleodb_record_claim(project_id, false, false);
    }

    /// [`Self::speleodb_note_lock_acquired`] for a lock the held-lock prompt's
    /// "Continue editing" RESUMED (#576 RV2-R-13-RESTART): a claim that licenses
    /// the release but not the authorship shortcut (#576 RV4-R-S3, see
    /// `LockClaim::resumed`).
    pub(super) fn speleodb_note_resumed_lock(&self, project_id: &str) {
        self.speleodb_record_claim(Some(project_id), false, true);
    }

    /// [`Self::speleodb_note_lock_acquired`] for the acquisition a Start Editing
    /// just made, which additionally REMEMBERS that claim as this flow's own.
    ///
    /// The stale-context unwind (`speleodb_release_lock_acquired_here`) has to
    /// hand back the lock THIS operation took and nothing else, and it cannot be
    /// told which that is - the caller's whole reason for calling is that the
    /// open project changed under it. Scanning the claims for "one of this
    /// account's" was near enough while a claim was one slot, and stopped being
    /// so the moment claims started surviving per project: an older stranded
    /// claim - one logout kept back precisely because it guards un-pushed work -
    /// is indistinguishable that way, and the unwind released THAT while leaving
    /// the mutex it had just taken held.
    pub(super) fn speleodb_note_acquisition_by_this_flow(&self, project_id: &str) {
        self.speleodb_record_claim(Some(project_id), true, false);
    }

    /// The body of both, so the two cannot record differently.
    fn speleodb_record_claim(&self, project_id: Option<&str>, by_this_flow: bool, resumed: bool) {
        let mut session = self.speleodb.lock();
        let (endpoint, email) = (session.endpoint.clone(), session.email.clone());
        session.locks_acquired_here.retain(|claim| {
            if !claim.taken_by(endpoint.as_deref(), email.as_deref()) {
                return true;
            }
            // Ours: replaced when this call names its project, ended when the
            // call says the account holds nothing.
            project_id.is_some_and(|id| claim.project_id != id)
        });
        // No account to stamp it with (a logout raced us, or a caller reached here
        // without a token) means no claim: one that cannot say whose it is can
        // never be released and must never license the authorship shortcut. Same
        // direction as the poisoned-lock case above - it costs the shortcut rather
        // than granting one.
        if let Some(((project_id, endpoint), email)) = project_id.zip(endpoint).zip(email) {
            let mut claim = super::speleodb_session::LockClaim::taken(project_id, endpoint, email);
            if resumed {
                claim = claim.into_resumed();
            }
            if by_this_flow {
                // Recorded in the SAME session acquisition as the claim itself,
                // so the two can never disagree about what was just acquired.
                session.acquired_by_this_flow = Some(claim.clone());
            }
            session.locks_acquired_here.push(claim);
        }
    }

    /// Whether THIS process took `project_id`'s write lock, is still editing under
    /// it, and did so as the account currently signed in (see
    /// [`Self::speleodb_note_lock_acquired`]).
    ///
    /// The account test is what keeps a claim from speaking for a session it has
    /// nothing to do with: signing out and in as somebody else does not make their
    /// lock ours, and it must not license the authorship shortcut over their push.
    pub(super) fn speleodb_lock_acquired_here(&self, project_id: &str) -> bool {
        let session = self.speleodb.lock();
        session.locks_acquired_here.iter().any(|claim| {
            claim.project_id == project_id
                && claim.taken_by(session.endpoint.as_deref(), session.email.as_deref())
        })
    }

    /// [`Self::speleodb_lock_acquired_here`] for a claim this process ACQUIRED, not
    /// one "Continue editing" resumed (#576 RV4-R-S3): the provenance the
    /// authorship licence needs. A resumed lock may be one a second session of
    /// the same account is still editing under.
    pub(super) fn speleodb_lock_taken_here(&self, project_id: &str) -> bool {
        let session = self.speleodb.lock();
        session.locks_acquired_here.iter().any(|claim| {
            claim.project_id == project_id
                && !claim.is_resumed()
                && claim.taken_by(session.endpoint.as_deref(), session.email.as_deref())
        })
    }

    /// Whether this account's claim on `project_id` records a RESUMED lock - read
    /// by the in-place reloads so they re-record a claim of the same kind.
    pub(super) fn speleodb_lock_resumed_here(&self, project_id: &str) -> bool {
        let session = self.speleodb.lock();
        session.locks_acquired_here.iter().any(|claim| {
            claim.project_id == project_id
                && claim.is_resumed()
                && claim.taken_by(session.endpoint.as_deref(), session.email.as_deref())
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
        self.speleodb
            .lock()
            .locks_acquired_here
            .iter()
            .any(|claim| claim.project_id == project_id && claim.on_deployment(endpoint))
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
        let mut session = self.speleodb.lock();
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

    /// [#258] Record which dump entry resolved this project's ambiguity, so the
    /// verbs that carry no `entryPath` can honour the choice for the rest of the
    /// session. Best-effort: a poisoned lock costs the memory, not the pull.
    ///
    /// `at` is stored with the entry and is what later decides whether the answer
    /// still describes anything (see [`DumpChoice`]). An undatable dump records
    /// NOTHING: a choice with no revision on it could never be shown to still apply,
    /// so keeping it would only be a name waiting to be trusted on no evidence.
    pub(super) fn speleodb_remember_dump_entry(
        &self,
        project_id: &str,
        entry: &str,
        at: DumpAt<'_>,
    ) {
        let DumpAt::Revision(sha) = at else { return };
        {
            let mut session = self.speleodb.lock();
            session
                .chosen_dump_entries
                .insert((project_id.to_owned(), sha.to_owned()), entry.to_owned());
        }
    }

    /// The entry [`Self::speleodb_remember_dump_entry`] recorded, if it was an answer
    /// about the revision now being classified.
    ///
    /// Two independent checks, and both are needed. THIS one asks whether the answer
    /// is about the right tree at all; a hit is then still validated for membership
    /// against the archive being extracted, because remembering a name is not
    /// trusting it. Membership alone was the whole guard and it is far too weak on
    /// its own - the entry a stale choice names is usually still present (a
    /// collaborator's push updates the canonical file and leaves a backup copy exactly
    /// where it was), so it sails through, and the pre-push bytes it points at get
    /// extracted and stamped with the new tip.
    ///
    /// A mismatch is left in place rather than evicted. It cannot be honoured while
    /// the revision differs, the next answer overwrites it, and both logout and a
    /// landing clear it outright - so evicting from a `&self` read would buy nothing
    /// but a surprising write on a path that only meant to ask a question.
    pub(super) fn speleodb_recalled_dump_entry(
        &self,
        project_id: &str,
        at: DumpAt<'_>,
    ) -> Option<String> {
        let DumpAt::Revision(sha) = at else {
            return None;
        };
        let session = self.speleodb.lock();
        session
            .chosen_dump_entries
            .get(&(project_id.to_owned(), sha.to_owned()))
            .cloned()
    }

    /// [#258] Record that this project's candidate list was offered for `at`.
    ///
    /// Called wherever an ambiguity is about to be REPORTED - never where it is
    /// refused, because a caller with no picker offers nothing. An undatable dump
    /// records nothing, which is what keeps [`Self::speleodb_dump_offer_answers`]
    /// permissive in that case rather than looping.
    pub(super) fn speleodb_note_dump_offer(&self, project_id: &str, at: DumpAt<'_>) {
        // Recorded even when the dump could not be DATED. "Nothing was asked" and "a
        // question was asked about a tree we could not name" are different states, and
        // collapsing them let a reply to the second be accepted against any revision -
        // so a collaborator pushing while the picker was open could have the user's
        // pick pass membership against a tree whose size and timestamp they never saw.
        let dated = match at {
            DumpAt::Revision(sha) => Some(sha.to_owned()),
            DumpAt::Unknown => None,
        };
        self.speleodb
            .lock()
            .offered_dump_at
            .insert(project_id.to_owned(), dated);
    }

    /// Whether an explicit pick arriving now is an answer to the question we asked.
    ///
    /// True unless we can positively show otherwise, and the two permissive cases are
    /// deliberate rather than lax:
    ///
    /// - **No offer recorded.** Nothing was asked, so nothing can be contradicted. The
    ///   pick is still membership-checked against the archive being extracted, which is
    ///   the check that was always doing the real work for a hand-authored `entryPath`.
    /// - **An undatable dump.** There is no revision to disagree with. Refusing here
    ///   would be worse than useless: the re-offer would be undatable too, so the
    ///   picker would ask the same unanswerable question forever.
    ///
    /// A mismatch, though, is positive evidence - the tree moved between the offer and
    /// the reply - and the answer is discarded so the flow re-offers what the dump
    /// holds NOW. That re-offer records the new revision, so the next reply matches and
    /// the loop terminates.
    pub(super) fn speleodb_dump_offer_answers(&self, project_id: &str, at: DumpAt<'_>) -> bool {
        let session = self.speleodb.lock();
        match (session.offered_dump_at.get(project_id), at) {
            // Nothing was asked, so nothing can be contradicted. The pick is still
            // membership-checked, which is what has always carried a hand-authored
            // `entryPath`.
            (None, _) => true,
            // Both dated: the ordinary case, and the one the whole mechanism is for.
            (Some(Some(offered)), DumpAt::Revision(sha)) => offered == sha,
            // The offer was dated and this dump is not. We cannot show the reply
            // answers that question, and undatable is the failing side here - refuse
            // and let the flow re-offer.
            (Some(Some(_)), DumpAt::Unknown) => false,
            // The OFFER was undatable and the reply is dated. Positive evidence they
            // are not about the same thing: the candidates described a tree we could
            // not name, and this one we can. Refuse; the re-offer will be datable too,
            // so this converges instead of looping.
            (Some(None), DumpAt::Revision(_)) => false,
            // Neither is datable. Nothing distinguishes them, and refusing would
            // re-offer an equally undatable question forever - the one case where
            // membership has to carry it alone.
            (Some(None), DumpAt::Unknown) => true,
        }
    }

    /// [#258] Forget this project's remembered dump entry.
    ///
    /// A push uploads under the canonical wire name and leaves every other `.stalx`
    /// in the tree untouched. The recall is per SESSION, so a user who chose, say,
    /// `backup/old.stalx` held a name that was correct when they answered and is
    /// PRE-PUSH data the moment their push lands - and the next open or fast-forward
    /// extracted that stale database, then stamped it with the new server revision.
    /// A silent rollback of their own work, published under a commit that claims to
    /// contain it.
    ///
    /// UNCONDITIONAL, and that is the whole point. Sparing a choice whose basename
    /// is canonical looks safe and is not: a basename can only say the path COULD be
    /// the upload target, never that it is - the folder is the server's
    /// `TARGET_FOLDER` and not part of the contract, so `backup/project.stalx` and
    /// `misc/project.stalx` can both exist and someone who picked the backup was
    /// spared on the strength of a name that proves nothing. The push then updates
    /// the real target and leaves their pick stale, which is the exact rollback this
    /// is here to prevent. `Dump::entry_details` reaches the same conclusion about
    /// the same predicate and declines to badge either entry as canonical.
    ///
    /// Rebinding to the target would be better than forgetting and is not available:
    /// the upload response carries the commit sha and the project, never the stored
    /// path. Forgetting costs one more question, and only for someone whose project
    /// is ambiguous at all.
    ///
    /// Still worth its own call even though [`DumpChoice`] now carries the revision
    /// it answered about, which already retires a choice the moment the tree moves
    /// under it - by whoever's hand. The two are not redundant: this one runs on OUR
    /// push, where the sha we would compare against is the one we just created, so
    /// the recall is stale and datable at the same instant. Dropping it outright is
    /// simply the earliest correct moment, and it keeps the invalidation legible at
    /// the point the staleness is caused rather than deferred to whoever reads next.
    pub(super) fn speleodb_forget_dump_entry(&self, project_id: &str) {
        {
            let mut session = self.speleodb.lock();
            // EVERY revision's answer for this project, not one slot: a landing makes
            // any non-canonical pick stale whichever tree it was about.
            session
                .chosen_dump_entries
                .retain(|(project, _), _| project != project_id);
        }
    }

    /// Conclude that a push landed a revision on the server, invalidating anything
    /// this session remembered about which file to read (#258).
    ///
    /// One helper rather than an invalidation call beside each arm, so producing the
    /// outcome and dropping the recall cannot be done separately. Every arm that
    /// concludes an upload landed goes through it - including the two that conclude
    /// it landed EARLIER rather than just now: the adopted prior push, and the 304
    /// that proves our content is already the server's.
    ///
    /// NOT every `PushOutcome::Confirmed`: the dirty-bit gate also reports Confirmed
    /// when there was nothing to push, and no upload means nothing has moved.
    ///
    /// CALL IT BEFORE THE LOCAL BOOKKEEPING, not after. Every arm records the landing
    /// into the link row, and every one of those writes can fail; a `?` between the
    /// upload and this call returns with a choice we already know is stale still
    /// standing, and the next verb that honours the recall extracts that pre-push
    /// entry and stamps it with the new tip. The landing is a fact about the SERVER
    /// the moment the upload returns - nothing local can un-land it, so nothing local
    /// belongs in front of it. Each arm therefore binds the outcome here and returns
    /// it unchanged after its write, which keeps concluding and invalidating together
    /// while putting both ahead of anything fallible.
    pub(super) fn speleodb_revision_landed(&self, project_id: &str) -> Landed {
        self.speleodb_note_prior_push_landed(project_id)
    }

    /// [Track C] (#188, C5): centralized session-level 401 auto-drop handler.
    /// Every **established-session** authed call (list/open) routes its `Result`
    /// through this; a **genuine 401** fully clears the token (keychain +
    /// plaintext + memory) via `forget()`. A 403 is **never** a drop (D-B4:
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

    /// Full local token clear (keychain + plaintext + memory) + session reset -
    /// the shared sink for the C5 auto-drop paths. Also drops the transient edit
    /// gate AND the #213 force-edit flag: `guard_mutation` reads only
    /// `edit_enabled`, so leaving it open after the token is gone would let the user
    /// keep editing a remote project that can no longer be pushed. The server lock
    /// is intentionally NOT released here - a 401 may be transient, and surrendering
    /// the single-writer claim on a flaky blip is worse than holding it; a re-login
    /// reconciles. (This is the AppState auto-drop sink; `Session::forget` is left
    /// untouched so the login switch-account path does not spuriously close the
    /// gate.)
    pub(super) fn speleodb_forget(&self) {
        // Drop the transient edit gate for a remote edit session. A dead token
        // surfaces as a 401 on ANY SpeleoDB call (e.g. a background list_projects /
        // refresh), and this is the auto-drop sink for all of them - so an
        // unconditional drop would silently turn an open LOCAL project read-only.
        // Mirror speleodb_logout's remote-gated, fail-SAFE drop exactly (#197
        // review): only a confirmed local-only project (NoRemoteProject) keeps the
        // gate; an undeterminable load (e.g. a poisoned database Mutex) fails safe to
        // read-only rather than leaving a token-less session able to edit.
        if self.edit_enabled() {
            match self.speleodb_open_link() {
                Ok(_) => self.set_edit_enabled(false),
                Err(SpeleoDbError::NoRemoteProject) => {}
                Err(_) => self.set_edit_enabled(false),
            }
        }
        // #213: and the force-edit flag with it. Unconditional, unlike the gate
        // above, because it can only ever be set on a remote force-edit session
        // (`speleodb_force_edit` requires an open remote link) - so there is no local
        // project for it to affect, and a dead token ends that session by definition.
        //
        // Left set, it OUTLIVED the gate it rides on: `speleodb_link_status` reports
        // it on every poll, so the frontend re-seeded `forceEditing: true` right after
        // clearing it, and its big-red force-edit banner then pre-empted the durable
        // "your changes are only on this computer" push-failure banner - hiding the
        // Retry and the typed commit message behind a banner whose only control
        // (Reconnect) 401s again, while `guard_mutation` refused every edit. It
        // self-healed only on close, or on a start_editing / unlink / restore that a
        // dead token cannot reach.
        self.set_force_editing(false);
        self.speleodb.lock().forget();
        // No belief at all, not the token store's, and one of the two places that is
        // deliberate (see `speleodb_persist_keychain_belief`). The sweep
        // above may well have failed against a locked keyring, so an entry can
        // survive - but the token in it is the one the server just rejected, and the
        // seed that would find it is still on disk. Opening the keyring on the next
        // launch to recover a dead token is a desktop unlock prompt spent on nothing;
        // the next successful login overwrites the entry anyway (this path sets no
        // refusal flag, so `store` still reaches the keyring).
        self.speleodb_persist_keychain_belief(None);
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

    // The account-record writer (`speleodb_write_account_record` - the email and the
    // keyring belief stamped with it, in one save) lives in `speleodb_logout`: the
    // email is the rehydrate SEED, so clearing it is a logout obligation and its
    // failure is part of logout's report. `speleodb_login` only writes it.

    /// Log in: exchange email+password for a token (or accept a pasted `token`),
    /// store it backend-side, and record the session. The token never crosses
    /// back to the webview - only the email + whether the keychain was used.
    pub fn speleodb_login(
        &self,
        override_endpoint: Option<String>,
        email: Option<String>,
        password: Option<String>,
        token: Option<String>,
    ) -> Result<SpeleoLoginResult, SpeleoErrorDto> {
        let endpoint = self
            .speleodb_resolve_endpoint(override_endpoint.as_deref())
            .ok_or(SpeleoDbError::NoEndpoint)
            .map_err(local_dto)?;
        // [Track A] #187: read the override and thread it into every client call.
        let insecure_http = self.speleodb_insecure_http();

        // Two login modes, each yielding both the token and the *authoritative*
        // account email used for the session/keychain key:
        // - Pasted token: the email is NOT typed (it can't be trusted), so derive
        //   it from `GET /user/`. That call also validates the token (a bad token
        //   401s here instead of silently slipping through to the first list call).
        // - Email + password: the auth-token exchange verifies the pair, so the
        //   typed email is authoritative by construction.
        let (token, email) =
            if let Some(t) = token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
                let email = client::fetch_user_email(&endpoint, &t, insecure_http)
                    .map_err(|e| dto_for_error(Endpoint::User, &e))?;
                (t, email)
            } else {
                let email = email.map(|e| e.trim().to_owned()).unwrap_or_default();
                let password = password.unwrap_or_default();
                // Reject blank credentials at the boundary rather than POSTing them
                // and relying on the server to 400.
                if email.is_empty() || password.is_empty() {
                    return Err(local_dto(SpeleoDbError::MissingCredentials));
                }
                let token = client::login(&endpoint, insecure_http, &email, &password)
                    .map_err(|e| dto_for_error(Endpoint::Login, &e))?;
                (token, email)
            };

        // [Track C] (#188, C6) Gate 1 of the two-gate plaintext opt-in: the
        // persisted `token_fallback == Plaintext` preference (read under the
        // config lock). Gate 2 (the keychain failed this session) is intrinsic to
        // TokenStore::store, which only reaches plaintext in the keychain-Err
        // branch.
        //
        // Gate 1 requires a SETTLED preference, not merely a recorded one. Reading
        // `token_fallback` alone wrote the key to a cleartext file BEFORE
        // `needs_storage_choice` below had asked the user anything - a login whose
        // keychain fails would land the token on disk and only then raise the
        // question, so answering "this session only" (or dismissing with "Decide
        // later") left a file the user never agreed to. An unsettled login lands in
        // memory instead, and the answer moves it - which costs nothing, because
        // the key is still in hand (`TokenStore::restore`).
        let (previously_declined, keychain_disabled, prefers_plaintext, config_has_token) = {
            let c = self.config.lock();
            (
                c.speleodb.keychain_declined,
                c.speleodb.keychain_disabled,
                c.speleodb.token_fallback == crate::config::TokenFallback::Plaintext,
                // Scoped to the account the belief is ABOUT - by the same
                // (endpoint, email) the keyring entry itself is keyed by, so a
                // different account's entry, on this server or another one, is not
                // even reachable under this login's key. See the re-assert below
                // for what carrying it across would claim.
                c.speleodb.keychain_belief_applies_to(&endpoint, &email),
            )
        };
        // A standing "never use the keychain" outranks the per-login question: it is
        // not asked, not retried, and signing in does not clear it.
        let previously_declined = previously_declined || keychain_disabled;
        let allow_plaintext = previously_declined && prefers_plaintext;

        // The ACCOUNT SWAP below is a lock operation in everything but name: every
        // decision about who may release or edit under a mutex is made from the
        // session this is about to replace. Taken here rather than at the top of
        // the verb, so the guard covers the swap and everything after it without
        // holding lock operations behind the two network round trips above.
        //
        // What it closes: the status poll's inherited-lock gate reopen reads the
        // account, re-reads the holder and opens the gate, all inside this same
        // guard. Without it here, a sign-in as somebody else landed mid-poll and
        // the gate opened for the NEW account on the strength of the OLD one's
        // lock - free to mutate a project whose mutex their credentials do not
        // hold. Logout takes the guard for the same reason (see
        // `speleodb_logout`); a login is the same event in the other direction.
        let lock_ops = self.speleodb_lock_ops_guard();
        // [#576 FZ-SDB-3-D1] A sign-in that REPLACES a live session (another account,
        // or the same one on another server) first runs logout's hand-back for it:
        // the old session's held lock released best-effort with ITS token, the edit
        // gate shut, the claim gone with the release. The UI used to be the only
        // thing standing between a re-login and a forgotten session still holding a
        // lock with its gate open; the backend must not depend on it. Decided under
        // a brief session guard, which the hand-back (it takes the session lock to
        // read the token and clear the claim) must not run under.
        //
        // [#576 RVF-c-B1] "Another account" is judged the way the lock-holder check
        // and the claim identity judge it (`lock_held_by_me`, `LockClaim::taken_by`):
        // the email case-insensitively, the server through `normalize_endpoint`. The
        // same account re-typed as `U@x`, or the same server with a trailing slash,
        // replaces nothing; an exact compare released its own lock and shut its gate.
        let replaced_session = {
            let session = self.speleodb.lock();
            match (session.endpoint.as_deref(), session.email.as_deref()) {
                (Some(old_endpoint), Some(old_email)) => {
                    crate::speleodb::auth::normalize_endpoint(old_endpoint)
                        != crate::speleodb::auth::normalize_endpoint(&endpoint)
                        || !old_email.eq_ignore_ascii_case(&email)
                }
                (None, Some(_)) => true,
                (_, None) => false,
            }
        };
        if replaced_session {
            tracing::info!(
                purpose = "hand back the session a sign-in replaces (#576 FZ-SDB-3)",
                "speleodb login: replacing a live session; handing its lock and edit \
                 gate back first"
            );
            self.speleodb_hand_back_replaced_session(&lock_ops);
        }
        let (tier, keychain_state, has_token) = {
            let mut session = self.speleodb.lock();
            // If a *different* account is already active, forget it first so its
            // keychain entry isn't orphaned; its lock and edit gate were handed back
            // above. A same-account re-login just overwrites the existing entry.
            // Exact here, unlike `replaced_session`: the keychain key keeps the
            // email's case, so `U@x` over `u@x` clears the old entry (its claim and
            // gate stay - nothing was handed back for the same account).
            let switching = session.endpoint.as_deref() != Some(endpoint.as_str())
                || session.email.as_deref() != Some(email.as_str());
            if switching {
                session.forget();
            }
            // Signing in is the one moment the user has opted back into being
            // asked, so a previous refusal is cleared here and the keychain gets
            // another go. Everywhere else the refusal is honoured silently.
            session.token_store.set_keychain_disabled(keychain_disabled);
            session.token_store.set_keychain_declined(keychain_disabled);
            // Re-assert what the config already knows about the keyring's contents,
            // the same way `speleodb_disable_keychain` does and for the same reason:
            // the live store learns it at rehydrate, but a session established by a
            // login this run never rehydrates, so without this the store would come
            // to the write below believing the keyring holds nothing - and the
            // write-back after it would then record that as fact. Evidence only, and
            // only ever raised.
            //
            // Evidence about THIS account only - see where `config_has_token` is
            // read. Signing in as somebody else, or as the same person on a second
            // server, used to import the previous account's record into a store about
            // to be keyed by the new one: the write below fails against a locked
            // keyring without lowering it, and the new email and the old account's
            // `true` were then persisted together, so every later launch opened the
            // keyring for the new account and the keyring switch reported a residue
            // for a credential that is not theirs and that no operation keyed by them
            // can find or remove. Worse on the warm path: the `switching` sweep above
            // may have just PROVED the old entry gone, and re-asserting raised the
            // belief back over that proof.
            //
            // What this does NOT do is rescue the old account's entry. Once the email
            // on disk moves, nothing can key an operation by the old one anyway (the
            // same reason logout records `false` after clearing the seed), so the
            // entry is left where the account switch left it - and the honest record
            // for the account that IS signed in is what this login can still write.
            session
                .token_store
                .note_keychain_has_token(config_has_token);
            let tier = session
                .token_store
                .store(&endpoint, &email, &token, allow_plaintext);
            session.endpoint = Some(endpoint.clone());
            session.email = Some(email.clone());
            // A fresh login established a session this run; don't let a later lazy
            // rehydration clobber it.
            session.rehydrate_attempted = true;
            (
                tier,
                session.token_store.keychain_state(),
                session.token_store.keychain_has_token(),
            )
        };

        // ONE save for the account and what the keyring is believed to hold FOR it.
        //
        // The belief is read off the token store, NOT computed as `tier ==
        // TokenTier::Keychain`. The landing tier answers "where did this login's key
        // go", which is a different question and the wrong one: when an earlier login
        // stored to the keyring and the keyring is merely LOCKED today, this login
        // falls back to memory or plaintext WITHOUT removing that entry, so recording
        // "nothing" here erased the only record that it exists. What that record buys
        // is the discrimination in `TokenStore::unreadable_keychain_may_hold` - a
        // locked keyring and a box with no secret service both fail every op and both
        // record `Unavailable` - so losing it meant the next launch skipped the
        // keyring, and turning the keyring off classified the failed eviction and its
        // failed probe as "nothing was in there" and reported success over a live
        // bearer token. The store's own field raises on a keychain write/read that
        // proved an entry is there and falls only on a delete or probe that proved one
        // is not, which is exactly the rule this needs. No belief here therefore still
        // means the keychain-less box (nothing ever landed) and still keeps its
        // startup skip.
        //
        // Stamped with the SAME `account_key` the entry is stored under, and written
        // in the same save as the email it is about: written apart, a crash - or just
        // a failed second save - left this account's email standing over the previous
        // account's `true`, which the next launch read as this one's.
        let _ = self.speleodb_write_account_record(
            Some(email.clone()),
            has_token.then(|| account_key(&endpoint, &email)),
        );
        // Clear the persisted refusal ONLY once the keychain has actually taken the
        // key. The in-session flag above lets this login retry a keychain the user
        // previously refused, but writing `false` to disk unconditionally REVOKED
        // that refusal before the retry had proved anything: when the keychain
        // failed again, the storage prompt's "Decide later" deliberately persists
        // nothing, so the config was left saying "not declined" and the next launch
        // rehydrated through the keychain - bringing back the very unlock prompt the
        // flag was introduced to stop (#259). On a failure the earlier answer stands
        // until the user replaces it through `speleodb_set_token_storage`.
        if tier == TokenTier::Keychain {
            self.speleodb_persist_keychain_declined(false);
        }

        let keychain = KeychainReport::from(keychain_state);
        Ok(SpeleoLoginResult {
            user_email: email,
            endpoint,
            keychain_backed: tier == TokenTier::Keychain,
            storage: tier.into(),
            keychain,
            // Ask ONLY when this login actually hit a broken keychain AND the
            // question is not already settled. A working keychain needs no
            // question, and a user who already answered it has settled where the
            // key goes - re-asking on every login would be the same nag in a new
            // costume. `previously_declined` is what carries that answer across a
            // restart; the in-session flag was cleared above to give the keychain
            // another go, so `Declined` cannot appear in `keychain` here and the
            // check is on the failure itself.
            needs_storage_choice: keychain == KeychainReport::Unavailable && !previously_declined,
            replaced_session,
        })
    }

    // `speleodb_logout` lives in `speleodb_logout`: it is three independently
    // fallible clears (gates + lock, credential, rehydrate seed) whose failure rules
    // are subtle enough to want their own module.

    /// Where a pulled `.stalx` is cached: `<cache>/speleodb/<project_id>/project.stalx`.
    /// The layout itself lives in `speleodb_local_state`, which also walks it to
    /// report per-project local state without opening anything read-write.
    pub(super) fn speleodb_cache_path(&self, project_id: &str) -> Result<PathBuf, SpeleoDbError> {
        let root = super::speleodb_local_state::cache_root()
            .ok_or_else(|| SpeleoDbError::Io("no app cache directory available".to_owned()))?;
        Ok(super::speleodb_local_state::cache_path_in(
            &root, project_id,
        ))
    }

    /// [#213] Optimistic force-edit (E2): enable editing on the open remote project
    /// WITHOUT acquiring the server write lock - for surveying offline, or when
    /// someone else holds the lock. Opt-in (gated on `allow_force_edit`); the FE
    /// shows a big-red "this `.stalx` cannot be merged" warning before calling this.
    ///
    /// Changes nothing else about the state model: no acquire, no network, BASE
    /// never moves; edits set the dirty bit through the normal gated-mutation path.
    /// The user later reconnects via `speleodb_start_editing` (bare acquire + the same
    /// BASE-vs-tip fence). A force-editor is exempt from the #202 lock re-verify and
    /// from every lock-release path (it holds no lock).
    pub fn speleodb_force_edit(&self) -> Result<(), SpeleoErrorDto> {
        // [#576 RVF-c-C1] Under `command_lock` to the end (nothing below takes it or
        // makes a network call): a force edit made for another project than the open
        // one is refused before the gate opens or the history is cleared.
        let _commands = self.hold_commands();
        self.speleodb_refuse_other_project("Force edit")?;
        // [#576 B6-D1] A project a newer build saved cannot be edited here, so
        // there is no edit session to force.
        if let Some(reason) = self.newer_file_refusal() {
            return Err(local_dto(newer_file_refused(reason)));
        }
        if !self.speleodb_allow_force_edit() {
            return Err(local_dto(SpeleoDbError::refused(
                "force-edit is disabled; enable it in SpeleoDB settings first".to_owned(),
            )));
        }
        // Must be a remote project (a local project is already freely editable).
        let _link = self.speleodb_validated_link()?;
        // Refuse when we are already editing WITH the lock held (nothing to force).
        if self.edit_enabled() && !self.force_editing() {
            return Err(local_dto(SpeleoDbError::refused(
                "already editing with the write lock held".to_owned(),
            )));
        }
        // [#303] Clear-on-lock, best-effort arm (q24). A force-edit takes NO lock and
        // runs no reconcile, so the "copy matches git" fence cannot be evaluated here -
        // but a force-editor is starting an edit session all the same, and the stack it
        // would start with is still whatever the last pusher shipped. Clearing
        // unconditionally is the safe direction: force-edit already carries the big-red
        // unmergeable-`.stalx` warning, so losing an inherited history is the least of
        // what that session is accepting.
        self.speleodb_clear_inherited_undo("force-edit session started without a lock");
        self.set_edit_enabled(true);
        self.set_force_editing(true);
        Ok(())
    }

    /// Acquire the write lock on the open remote project and reconcile it against
    /// the server (#194). On a clean reconcile the transient edit gate is enabled;
    /// on divergence the freshly-taken lock is released and the user is asked to
    /// reconcile; on contention the lock holder is reported. Errors (no
    /// permission, an anomalous server state, network) surface as `SpeleoErrorDto`.
    ///
    /// Uses the base-SHA fence (#192) - NOT ancestry recovery (#193), which is the
    /// push path's concern (#197): start-editing only reconciles, it never pushes.
    /// `set_edit_enabled(true)` happens ONLY on a clean reconcile (UpToDate /
    /// FastForward), and for FastForward it happens AFTER the in-place reload (the
    /// reload re-derives the gate from `is_remote` and would otherwise re-disable).
    ///
    /// [#213] Doubles as the force-edit RECONNECT: when called while force-editing, a
    /// clean reconcile (UpToDate / FastForward) clears the force-edit flag because we
    /// now genuinely hold the lock, while the contention (400 `LockedBy`) and
    /// `Diverged` arms leave both gates untouched - so a force-editor stays
    /// force-editing (keeps their edits) rather than reverting to read-only.
    ///
    /// [#576 SDB-ACCT-D1] A lock found held by this account - another Stalagmight
    /// session, another machine, the SpeleoDB website, or this app before a restart -
    /// is the user's, and editing starts under it: the commit records it as this
    /// session's ADOPTED claim (resumed: the release, not the authorship shortcut), so
    /// Done and the silent releases hand it back as usual. Plain Start editing and the
    /// held-lock prompt's "Continue editing" (#576 RV2-R-13-RESTART,
    /// [`Self::speleodb_resume_held_lock_editing`]) are the same flow now; only the
    /// latter used to claim such a lock.
    fn speleodb_start_editing_with(&self) -> Result<StartEditingResult, SpeleoErrorDto> {
        // [#576 RVF-c-C1] Made for another project than the open one: refused before
        // anything below reads or resets that project's state. The check that binds
        // is the one in the snapshot's span.
        {
            let _commands = self.hold_commands();
            self.speleodb_refuse_other_project("Start editing")?;
        }
        // [#576 B6-D1] Before any of it: a project a newer build saved cannot be
        // edited here, and taking the server lock for it would only keep every
        // collaborator out of a project this session can never change or push.
        if let Some(reason) = self.newer_file_refusal() {
            return Err(local_dto(newer_file_refused(reason)));
        }
        // The lock-ops span opens FIRST, before a single fact is read: everything
        // below is decided from facts (the session generation, the link, the
        // token, the mutex holder) that another lock operation could invalidate,
        // and this guard is what stops one running while they are in flight. It
        // covers the token read specifically because logout takes the same guard -
        // a logout that slipped in after the token was read left this flow acquiring
        // and committing on behalf of a signed-out session, which the fast-forward
        // arm's identity-only commit cannot see (the slot never moves). Held across
        // the network span on purpose - see `speleodb_session`.
        let lock_ops = self.speleodb_lock_ops_guard();
        // This operation's unwind slot starts empty. An acquire that inherits an
        // already-held lock, or one that aborts, records nothing - and must
        // leave the stale-context unwind with nothing to hand back, rather than
        // the previous operation's acquisition.
        //
        // [#576 A8] The order below is the types' (`start_order`): every step takes
        // the span's proof (`LockOpsHeld`) or a value read inside it (`InSpan`), and
        // the acquire takes the reset's `FlowBegun` and the probe's `HolderProbed`.
        let flow = self.speleodb_begin_acquisition_flow(&lock_ops);

        // Generation + link read as ONE world under the `project` mutex (D6): the
        // commit far below re-checks the generation before opening the gate, so a
        // teardown landing inside the network window refuses instead of committing
        // onto a torn-down world (F7). See `speleodb_session`.
        // [#576 RVF-c-C1] In one `command_lock` span with the identity check (that
        // lock sits below the lock-ops span and above `project`), so the acquire below
        // goes to the remote of the project the request was made for.
        let (entry_gen, link) = {
            let _commands = self.hold_commands();
            self.speleodb_refuse_other_project("Start editing")?;
            self.speleodb_session_snapshot(&lock_ops)?
        };
        let account = lock_ops.read_account().transpose().map_err(local_dto)?;
        let (endpoint, token) = account.get().clone();
        let insecure_http = self.speleodb_insecure_http();

        // Whether the acquire below is OURS to claim - probed BEFORE it, because the
        // acquire itself cannot tell us. Re-acquiring a lock the ACCOUNT already holds
        // is a successful server-side no-op (only ANOTHER holder produces the 400 that
        // reports contention), and `ActiveMutex` parses no timestamp, so a 200 here
        // says nothing about whether this process caused the transition. Without the
        // probe a restarted process, a second window, or the held-lock dialog's
        // "Continue editing" - which calls this verb precisely to resume an INHERITED
        // lock - would all mint an authorship claim they never earned. (Such a lock
        // is still claimed for the release once editing starts, as an ADOPTED lock -
        // #576 SDB-ACCT-D1, in `speleodb_commit_editing` - but never by this probe.)
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
        //   that never started, which blocks every other writer until the server-side
        //   timeout or the next login's held-lock prompt.
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
        // The four outcomes are `start_order::probe_verdict`'s (driven by its tests).
        // Whatever it says, a commit below claims the lock (#576 SDB-ACCT-D1): this
        // flow's acquisition when `acquired_here`, an ADOPTED lock otherwise - and an
        // adopted lock is still never handed back by an unwind.
        let probed = self.speleodb_probe_before_acquire(
            &lock_ops,
            &account,
            insecure_http,
            &link.project_id,
        );
        let start_order::ProbeVerdict {
            acquired_here,
            keep_lock_on_unwind,
        } = probed.verdict(me.as_deref(), link.has_unpushed_edits);

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
        // recoverable (the next login's scan re-raises this very prompt, and the server
        // mutex times out on its own), un-pushed work behind a freed lock is not.
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
        //   held-lock prompt or the server-side timeout.
        //
        // So the unknown case keys on whether there IS anything to protect
        // (`Err(_) => link.has_unpushed_edits`), which is the same asymmetry
        // `speleodb_lock_release_params` already encodes: never silently release a
        // lock whose project still holds edits the server has not seen.
        //
        // Nothing touches the acquisition record on the skip path, deliberately: the
        // record is dropped by RELEASING (in `speleodb_release_best_effort`), and a
        // release we declined to make ends no claim. `acquired_here` is false on both
        // skip paths, so this call mints nothing either - record and lock stay in step.
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
        // dump with nothing local at stake still hands back a lock we took.
        let release_unless_kept = |failure: Option<&SpeleoDbError>| {
            if unwind_keeps_lock(keep_lock_on_unwind, failure) {
                return;
            }
            // Session-aware since F7/F8: same session releases exactly as before;
            // a moved one never touches the record and stands down for a
            // successor's claim. See `speleodb_unwind_release`.
            self.speleodb_unwind_release(
                &lock_ops,
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                entry_gen,
            );
        };

        // Acquire the lock. A 400 is the server's "someone else holds it" signal;
        // it has no first-class map_status row, so it is handled here (per #194).
        let acquired = self.speleodb_acquire_after_probe(
            &probed,
            &flow,
            &account,
            insecure_http,
            &link.project_id,
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
            // [#576 FZ-SDB-6-D1] A lost reply: re-read the holder at once. Ours right
            // after our own acquire means the acquire landed, so Start Editing goes on
            // as if the reply had arrived, and the probe's verdict decides the claim
            // exactly as on an answered acquire (taken here: this flow's; otherwise
            // adopted). Anything else stays the failed acquire.
            Err(e) => {
                let after = self.speleodb_reread_after_lock_verb(
                    &endpoint,
                    &token,
                    insecure_http,
                    &link.project_id,
                    &e,
                );
                if after != super::speleodb_lock_release::AfterLockVerb::Ours {
                    // [#576 SDB-POLL-D1] The re-read failed too: the acquire may have
                    // landed, and the next successful poll settles it. [#576
                    // SDB-ACCT-D1] Marked in EVERY case, as an answered acquire is
                    // claimed in every case: of the kind it would have recorded (this
                    // session's when the probe saw the lock free or another account's,
                    // adopted when it saw ours or could not say).
                    if after == super::speleodb_lock_release::AfterLockVerb::Unknown {
                        self.speleodb_mark_acquire_unconfirmed(&link.project_id, acquired_here);
                    }
                    return Err(dto_for_error(Endpoint::Acquire, &e));
                }
                tracing::info!(
                    project_id = %link.project_id,
                    error = %e,
                    purpose = "follow an acquire whose reply was lost (#576 FZ-SDB-6)",
                    "speleodb start editing: the acquire reply was lost, but the lock is ours"
                );
            }
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
                // Gate + acquisition record + force-edit clear + the #202 verify
                // stamp, committed under the session guard (F7): a teardown inside
                // the network window above refuses HERE, with nothing committed,
                // and the unwind hands the acquired lock back unless a successor
                // session has reclaimed it. See `speleodb_commit_editing`.
                if let Err(e) = self.speleodb_commit_editing(
                    &lock_ops,
                    Some(entry_gen),
                    &link.project_id,
                    acquired_here,
                ) {
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
                // A failure here (e.g. a corrupt re-pulled dump) leaves us holding
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
                    tip,
                    LocalEditsPolicy::Protect,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        release_unless_kept(Some(&e));
                        return Err(local_dto(e));
                    }
                };
                // As in the UpToDate arm, but identity-only (`None`): the reload
                // just above IS a session boundary, so a generation comparison
                // would always refuse - the open project still being linked to P
                // is the check that matters here. See `speleodb_commit_editing`.
                if let Err(e) =
                    self.speleodb_commit_editing(&lock_ops, None, &link.project_id, acquired_here)
                {
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

    /// Start editing the open remote project: acquire its write lock and reconcile
    /// it (see [`Self::speleodb_start_editing_with`]). A lock the account already
    /// holds is adopted, and claimed for this session (#576 SDB-ACCT-D1).
    pub fn speleodb_start_editing(&self) -> Result<StartEditingResult, SpeleoErrorDto> {
        self.speleodb_start_editing_with()
    }

    /// The held-lock prompt's "Continue editing" (#576 RV2-R-13-RESTART): start
    /// editing exactly as [`Self::speleodb_start_editing`] does. It used to be the
    /// only way to claim a lock this account already held; since #576 SDB-ACCT-D1
    /// every Start editing adopts such a lock (a RESUMED claim: the release, not the
    /// authorship shortcut, #576 RV4-R-S3, since the lock may be a second session's).
    pub fn speleodb_resume_held_lock_editing(&self) -> Result<StartEditingResult, SpeleoErrorDto> {
        self.speleodb_start_editing_with()
    }

    /// [#576 RV2-R-15] The read-only workspace gate for the SpeleoDB verbs that
    /// replace the survey outside the command chokepoint (Restore revision, Unwind,
    /// Take remote, Discard changes): the refusal of [`Self::guard_workspace`],
    /// carrying its `WORKSPACE_READ_ONLY` code (#576 C4) so the frontend recognises
    /// it as the edit gate and shows its notice.
    #[expect(
        clippy::result_large_err,
        reason = "the SpeleoErrorDto every SpeleoDB verb returns; boxing the error type is a \
                  repo-wide change outside #576 RV4"
    )]
    pub(super) fn speleodb_guard_workspace(&self, what: &str) -> Result<(), SpeleoErrorDto> {
        self.guard_workspace(what)
            .map_err(|refusal| local_dto(refused_by(refusal)))
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
        let db_guard = self.database.lock();
        let db = db_guard.as_ref().ok_or(SpeleoDbError::NoRemoteProject)?;
        db.load_speleo_link()
            .map_err(|e| SpeleoDbError::Io(e.to_string()))?
            .ok_or(SpeleoDbError::NoRemoteProject)
    }

    /// Convert the open managed project into a plain, unmanaged `.stalx` IN PLACE:
    /// drop its `speleo_link` row so it is no longer tied to SpeleoDB, then open the
    /// edit gate. Nothing is pushed. No-op-safe: a `None` delete over an absent row
    /// is fine.
    ///
    /// [#576 FZ-SDB-5] Not the Unlink any more: Unlink MOVES the copy out of the
    /// cache (`speleodb_unlink`, in `speleodb_unlink.rs`), because a link-less file
    /// left at the cache path was replaced by the next Open from SpeleoDB, local
    /// edits and all. The one caller left is `speleodb_delete_project`, after the
    /// server project is gone; the copy it leaves in the cache is a link-less
    /// `.stalx` the download guard now refuses to replace (FZ-SDB-5-D3).
    ///
    /// A held write lock IS released first (best-effort, mirroring logout - the lock
    /// needs the token, so before we drop the link): abandoning the project while
    /// still holding its server mutex would strand it locked for every other editor.
    /// Unlike logout the token is kept - the user stays signed in, they just walk
    /// away from this one project.
    pub(super) fn speleodb_unlink_in_place(&self) -> Result<(), SpeleoErrorDto> {
        // Release the server lock if this is a held remote edit session, so we don't
        // leave it stranded. Best-effort: an offline / no-access release just no-ops.
        // The ABANDONING variant: un-pushed edits do not veto this release the way
        // they veto the silent close/quit/switch one, because unlink deletes the link
        // row - a lock kept back here could never be released from this project
        // again, and the user has said outright that the managed copy is abandoned.
        self.speleodb_release_held_lock_abandoning();
        {
            // [#576 NF-W4C-2] Under `command_lock`, like every link write: an autosave
            // landing in between copies the link at its start and would write it back.
            let _commands = self.hold_commands();
            // [#576 RVF-c-C1] Never the link of a project a switch opened meanwhile.
            self.speleodb_refuse_other_project("Unlinking from SpeleoDB")?;
            let db_guard = self.database.lock();
            let db = db_guard
                .as_ref()
                .ok_or_else(|| local_dto(SpeleoDbError::NoRemoteProject))?;
            db.save_speleo_link(None)
                .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        }
        // Now a local-only project: it edits freely (the read-only gate hung on the
        // remote link, which is gone). Clear force_editing too (a force-editor can reach
        // "save a local copy"): a local project holds no lock concept, and every
        // force_editing reader assumes an open remote link, so don't leave the flag set.
        self.set_edit_enabled(true);
        self.set_force_editing(false);
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
    /// A held write lock is released first (best-effort, like [`Self::speleodb_unlink`]):
    /// the user is abandoning their edit session, and the fast-forward reload drops
    /// the edit gate back to read-only. They resume via Start editing, which
    /// re-acquires the lock and now reconciles cleanly (BASE has advanced to the tip).
    pub fn speleodb_take_remote(&self) -> Result<String, SpeleoErrorDto> {
        // [#576 RV4-R-NITS S-8] Replaces the survey: refused in the read-only
        // Plan and Export workspaces, before any network call (as Restore, RV2-R-15).
        self.speleodb_guard_workspace("Take remote")?;
        let link = self.speleodb_link_made_for("Take remote")?;
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

        // Abandoning local edits: release the held lock (if any) so it isn't left
        // stranded, mirroring speleodb_unlink. Best-effort - an offline release no-ops.
        // The ABANDONING variant, necessarily: take-remote is only ever reached WITH
        // un-pushed edits, and discarding them is the whole point of the verb (behind
        // the frontend's second destroy-confirmation), so the veto that protects the
        // silent close/quit/switch release must not apply here.
        self.speleodb_release_held_lock_abandoning();

        // Re-pull + reset to the tip: BASE = tip, dirty cleared, last_edit_at reset,
        // and the in-place reload starts the undo/redo stacks empty (the server
        // snapshot carries no history). This IS a fast-forward, minus the dirty guard
        // - the whole point of take-remote is to discard the local edits. The reload
        // re-derives the edit gate from is_remote, leaving the project read-only.
        let result = self.speleodb_fast_forward(
            &endpoint,
            &token,
            insecure_http,
            &link,
            tip,
            LocalEditsPolicy::Discard,
        );
        if result.is_err() {
            // Unlike a lock release - where the gate drop is a side effect and a
            // force-editor is exempt (see [`Self::speleodb_release_all_locks`]) - here
            // the session ending IS what the user asked for: take-remote is them
            // abandoning this local session in favour of the server tip, and the lock,
            // if they held one, is already gone above. So drop to read-only even when
            // the reset failed part-way. Keeping them editing would honour neither
            // outcome: they would still be editing a project we have just told them we
            // could not reconcile, over a local copy whose fate is now unclear.
            // Read-only is also the recoverable direction - they re-enter deliberately
            // via Start editing (or Force edit), whereas silently continuing piles more
            // edits onto a tree the reset failed to replace. Their local edits are
            // untouched on disk until then.
            //
            // BOTH gates, never just the first: `edit_enabled == false &&
            // force_editing == true` must never be observable. `speleodb_link_status`
            // reports the flag on every poll, so a stranded one paints the big-red
            // force-edit banner over a project that is read-only, and pre-empts the
            // surface that would explain why.
            self.set_edit_enabled(false);
            self.set_force_editing(false);
        }
        result.map_err(local_dto)
    }

    /// Release EVERY write lock this user holds across all projects (#201, B1):
    /// the "Release all" choice on the held-lock resume prompt, resolving locks a
    /// crashed/abandoned session left behind (SpeleoDB has no server-side TTL).
    /// Idempotent server-side (releasing when none are held is a no-op).
    ///
    /// DANGER, and the reason this is documented rather than quietly kept around:
    /// it is the bluntest verb in the module. It BYPASSES the un-pushed-edits veto
    /// entirely - not per project, but for every project at once, including ones
    /// this machine holds a dirty local copy of and ones it has never seen - and it
    /// asks the local cache nothing at all. A single call can therefore free N
    /// projects for other editors to lock and push over, leaving N piles of local
    /// survey reachable only through a divergence. `speleodb_release_lock`
    /// (per-project, after the frontend has joined the held-lock scan against
    /// `speleodb_local_project_states`) is what a user-facing button should reach
    /// for.
    ///
    /// Kept registered (`lib.rs` + `telemetry::registry`) even as its in-app caller
    /// is retired: unregistering it would also mean rebalancing the registry's
    /// command count, and the verb is still the only recovery for an account whose
    /// locks are scattered across projects this machine has no copy of.
    pub fn speleodb_release_all_locks(&self) -> Result<(), SpeleoErrorDto> {
        // Guarded for the same reason as `speleodb_release_lock`, and more so:
        // this frees EVERY lock the account holds, so an acquire slipping
        // between the POST and the cleanup leaves a mutex this call already
        // released re-taken and then un-recorded.
        let _lock_ops = self.speleodb_lock_ops_guard();
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        // [#576 B5] Its own row: release-all IS retried, and the `Release` row it
        // used to borrow reported an exhausted 5xx as "never retried".
        let row = client::spec::RELEASE_ALL.category;
        self.speleodb_handle_auth(
            row,
            client::release_all_locks(&endpoint, &token, insecure_http),
        )
        .map_err(|e| dto_for_error(row, &e))?;
        // If the OPEN project is a remote one we were editing UNDER THE LOCK, its lock
        // is gone too: drop the edit gate so the shell falls back to read-only. A
        // local project's always-true gate is left untouched (open_link fails → no
        // reset). The FE refreshes link status afterwards to re-seed the indicator.
        //
        // A force-editor (#213) is exempt, matching the gate check in
        // `speleodb_release_current_lock`: they hold NO server lock, so this call
        // released a stale or someone else's claim, not theirs, and ending an editing
        // session the release had nothing to do with is both wrong and the way the
        // invalid `edit_enabled == false && force_editing == true` state was reached -
        // a read-only project still reporting the big-red force-edit banner. The
        // frontend already models the two as independent (`holdsWriteLock` is
        // `editing && !forceEditing`).
        if self.edit_enabled() && !self.force_editing() && self.speleodb_open_link().is_ok() {
            self.set_edit_enabled(false);
        }
        // Unconditional, unlike the gate above: this frees every lock the account
        // holds, so any acquisition this process recorded - for the open project or
        // one it has since navigated away from - is over.
        self.speleodb_note_lock_acquired(None);
        Ok(())
    }

    /// Release the write lock on ONE project by id (#201 per-project "Release" in
    /// the held-lock resume prompt). Unlike [`Self::speleodb_done`] this targets an
    /// arbitrary held project (not necessarily the open one) and neither pushes nor
    /// touches the open project's edit state - EXCEPT when the released lock is the
    /// open project's own AND we were editing under it, in which case the edit gate
    /// is dropped so the shell falls back to read-only (mirrors
    /// [`Self::speleodb_release_all_locks`]). A #213 force-edit session holds no lock
    /// and so survives untouched.
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
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        self.speleodb_handle_auth(
            Endpoint::Release,
            client::release_lock(&endpoint, &token, insecure_http, &project_id),
        )
        .map_err(|e| dto_for_error(Endpoint::Release, &e))?;
        // If we just released the OPEN project's own lock, drop the edit gate so the
        // shell falls back to read-only; releasing some other project's lock leaves
        // the open project untouched. Force-editors are exempt for the same reason as
        // in [`Self::speleodb_release_all_locks`] - they hold no lock, so nothing
        // released here was theirs to lose.
        if self.edit_enabled()
            && !self.force_editing()
            && self
                .speleodb_open_link()
                .is_ok_and(|l| l.project_id == project_id)
        {
            self.set_edit_enabled(false);
        }
        // Whichever project's lock that was, this process no longer holds it, so an
        // acquisition recorded for it is over. Keyed on the released id, so releasing
        // some OTHER project's lock leaves an open edit session's record alone - and
        // keyed ATOMICALLY, through the shared compare-and-clear. This was a
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
    pub(super) fn speleodb_release_best_effort(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
    ) {
        // The shared sink for start-editing's abort arms, the publish push and -
        // through `speleodb_release_held_lock_*` - close / logout / unlink /
        // take-remote, so the acquisition record is dropped here whether or not the
        // POST lands: a release we asked for ends the claim, and one that failed
        // leaves a lock this process can no longer account for.
        //
        // Keyed on `project_id`, like `speleodb_release_lock`. It used to clear
        // whatever was recorded, on the grounds that clearing a record naming some
        // OTHER project could only cost the authorship shortcut - true when the
        // record was one slot holding at most a stale id, and false now that a
        // claim can be a live pointer to a mutex still held on another project
        // (the janitor's retry after a transient failure). This releases one
        // project's lock, so it may end one project's claim.
        self.speleodb_forget_lock_acquired_for(project_id);
        if let Err(e) = client::release_lock(endpoint, token, insecure_http, project_id) {
            tracing::warn!(
                error = %e,
                project_id,
                purpose = "release a lock taken before a no-edit reconcile outcome",
                "speleodb_start_editing: failed to release lock"
            );
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
        {
            let mut stack = self.undo_stack.lock();
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
        self.speleodb.lock().email.clone()
    }

    /// Load the open project's `speleo_link`, apply `f`, and persist it. Errors if
    /// no remote project is open.
    ///
    /// [#576 B35(d) / NF-W4C-2] Call it holding `command_lock` (`hold_commands`, or
    /// from inside `execute`, as the re-verify stamps do): an autosave copies the link
    /// at its start, writes a fresh file and renames it over the live one, so a write
    /// landing inside it went into the replaced file or found no handle. It does not
    /// take the lock itself because its callers already hold it (it is not reentrant).
    pub(super) fn speleodb_mutate_link(
        &self,
        f: impl FnOnce(&mut crate::models::SpeleoLink),
    ) -> Result<(), SpeleoDbError> {
        let db_guard = self.database.lock();
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

    /// Persist `link` onto the OPEN project's database, creating the row when the
    /// project has never had one. The insert-side twin of
    /// [`Self::speleodb_mutate_link`], which can only edit a link that already
    /// exists.
    ///
    /// For adoption, which is the one flow that mints a link for a project it just
    /// built. It used to stamp through a SECOND `Database::open_existing` over the
    /// same file, which worked only because nothing then read the link back through
    /// the live handle - and the hand-over does exactly that (`speleodb_done` reads
    /// `has_unpushed_edits` to decide whether to push at all). Writing through the
    /// handle the rest of the flow reads from removes the question, and with it the
    /// "could not reopen the adopted .stalx" arm that had no recovery behind it.
    pub(super) fn speleodb_set_open_link(
        &self,
        link: &crate::models::SpeleoLink,
    ) -> Result<(), SpeleoDbError> {
        let db_guard = self.database.lock();
        let db = db_guard.as_ref().ok_or(SpeleoDbError::NoRemoteProject)?;
        db.save_speleo_link(Some(link))
            .map_err(|e| SpeleoDbError::Io(e.to_string()))
    }

    /// #202 lazy lock re-verify: when EDITING a remote project with a real held
    /// lock, at most once per [`REVERIFY_INTERVAL_SECS`], re-GET the project before
    /// applying an edit and confirm we STILL HOLD the write lock. Called from the
    /// `execute()` command chokepoint AFTER `guard_mutation` (so it runs only for an
    /// enabled edit session on that path - undo/redo and the other direct mutation
    /// entrypoints deliberately do NOT re-verify; they are covered by the same
    /// backstops). It bounds wasted work; it is NOT a correctness guard (the
    /// release-time fence + a 403 on push are), so a transient failure lets the edit
    /// through and the fence catches any real conflict.
    ///
    /// Only the lock-holder is checked, NOT the server tip: while we hold the lock
    /// no other writer can advance the tip, so a "tip moved" signal would mean our
    /// lock was already lost - which the holder check catches directly. (The server's
    /// `latest_commit` field is `max(authored_date)` and unreliable as a tip anyway,
    /// see [`sync::tip_of`]; comparing it to a topology-derived BASE false-positives.)
    ///
    /// The GET uses a short, non-retried probe budget ([`client::get_project_probe`]):
    /// it runs UNDER the command lock, so a hung server must not stall the whole
    /// command pipeline (autosave included).
    ///
    /// - No lock held (read-only / local project) or not yet due: no-op.
    /// - Lock lost (mutex holder is not us): drop the edit gate and return an
    ///   `EDIT_REVERIFY_DIVERGED` failure so the FE blocks this edit and routes to
    ///   the divergence prompt.
    /// - Still ours: stamp `last_verify_at` and allow.
    /// - Transient / offline GET failure: stamp the attempt (keeps the <=3/hr bound)
    ///   and ALLOW the edit - offline editing stays first-class.
    pub(super) fn speleodb_verify_lock_if_expired(&self) -> Result<(), crate::core::CommandError> {
        // Only an active LOCK-HELD edit session on a remote project re-verifies. A
        // read-only remote project (gate off) or a local project (no link row) never
        // does. #213: a force-editor holds no lock, so there is nothing to re-verify -
        // exempt it (the online tip-vs-BASE early warning is surfaced by
        // speleodb_link_status instead, and skipped offline).
        if !self.edit_enabled() || self.force_editing() {
            return Ok(());
        }
        let Ok(link) = self.speleodb_open_link() else {
            return Ok(()); // local-only: no remote lock to verify
        };
        let now = chrono::Utc::now();
        if !reverify_due(link.last_verify_at.as_deref(), now) {
            return Ok(());
        }
        let Ok((endpoint, token)) = self.speleodb_active_token() else {
            return Ok(()); // logged out: the close/logout path already drops the gate
        };
        let insecure_http = self.speleodb_insecure_http();

        match client::get_project_probe(&endpoint, &token, insecure_http, &link.project_id) {
            Ok(project) => {
                let me = self.speleodb_my_email();
                let holder = project.active_mutex.as_ref().map(|m| m.user.as_str());
                if !lock_held_by_me(holder, me.as_deref()) {
                    // The write lock is no longer ours (released, admin-cleared, or
                    // taken by another session). We can no longer safely edit: drop
                    // the gate (so further edits are cleanly read-only until resolved)
                    // and route the user to the divergence prompt.
                    self.set_edit_enabled(false);
                    return Err(crate::core::CommandError::new(
                        crate::models::CommandErrorCode::EditReverifyDiverged,
                        "the SpeleoDB write lock is no longer held - reconcile before editing.",
                    ));
                }
                // Still ours: stamp the verify and allow the edit.
                let _ = self.speleodb_mutate_link(|l| l.last_verify_at = Some(now.to_rfc3339()));
                Ok(())
            }
            Err(e) => {
                // Transient / offline: #202 is not a correctness guard, so allow the
                // edit (the release-time fence is the backstop). Stamp the attempt so
                // an offline editor doesn't re-issue a failing GET on every edit.
                tracing::warn!(
                    error = %e,
                    purpose = "lazy lock re-verify (#202)",
                    "speleodb re-verify GET failed; allowing the edit (release fence is the backstop)"
                );
                let _ = self.speleodb_mutate_link(|l| l.last_verify_at = Some(now.to_rfc3339()));
                Ok(())
            }
        }
    }
}

/// [#576 A8] The unwind's keep rule (see `speleodb_start_editing_with`): keep the lock
/// when the pre-acquire evidence says so, or when the failure being unwound is the
/// cache copy refusing over its own un-pushed work - knowledge that only exists after
/// the acquire. Every other failure, and a plain verdict (`None`), releases.
pub(super) fn unwind_keeps_lock(
    keep_lock_on_unwind: bool,
    failure: Option<&SpeleoDbError>,
) -> bool {
    keep_lock_on_unwind || failure.is_some_and(is_local_edits_refusal)
}

/// [#576 A8] Start Editing's acquire chain as types, every proof made only here: the
/// unwind slot's reset ([`FlowBegun`]) and the pre-acquire holder probe
/// ([`HolderProbed`]) both run inside the lock-ops span (they take its proof, the
/// probe with the account read inside it), and the acquire takes both - so it cannot
/// be written above either, and neither can leave the span.
///
/// [#576 NF-A8R-GAPS] Not typed, on purpose: the session snapshot
/// (`speleodb_session_snapshot`) before the token read. Both are reads inside the
/// same span (each takes its proof), nothing between them can change either - logout
/// and login wait on the span - and the commit re-checks the generation and the
/// identity anyway, so swapping them changes no outcome. A proof there would order
/// two reads, not a read and the step it guards.
mod start_order {
    use super::super::speleodb_session::{InSpan, LockOpsHeld};
    use super::{lock_held_by_me, AppState};
    use crate::speleodb::contract::Endpoint;
    use crate::speleodb::models::Project;
    use crate::speleodb::{client, SpeleoDbError};

    /// Proof this operation's unwind slot was reset, inside the span. (Nameable across
    /// `core::state` because the reset is - the session tests drive it; still made
    /// only here.)
    pub(in crate::core::state) struct FlowBegun {
        _reset: (),
    }

    /// The pre-acquire holder probe, read inside the span with the span's account.
    pub(super) struct HolderProbed {
        holder: Result<Option<String>, SpeleoDbError>,
    }

    /// What the probe decides for the acquire that follows it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct ProbeVerdict {
        /// The acquire is OURS to record (it made the unheld -> ours transition).
        pub(super) acquired_here: bool,
        /// The pre-acquire half of the unwind's keep rule ([`super::unwind_keeps_lock`]).
        pub(super) keep_lock_on_unwind: bool,
    }

    impl AppState {
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
        /// Called inside the lock-ops span (it takes the proof), so no other flow can
        /// be between its own clear and its own commit while this runs.
        pub(in crate::core::state) fn speleodb_begin_acquisition_flow(
            &self,
            _lock_ops: &LockOpsHeld<'_>,
        ) -> FlowBegun {
            self.speleodb.lock().acquired_by_this_flow = None;
            FlowBegun { _reset: () }
        }

        /// Who holds the mutex, asked BEFORE the acquire (which cannot tell us) and
        /// inside the span - outside it, a captured release can invalidate the
        /// answer before the acquire runs (round 5). The CHECKED probe: "unheld" and
        /// "could not find out" stay apart.
        pub(super) fn speleodb_probe_before_acquire(
            &self,
            _lock_ops: &LockOpsHeld<'_>,
            account: &InSpan<'_, (String, String)>,
            insecure_http: bool,
            project_id: &str,
        ) -> HolderProbed {
            let (endpoint, token) = account.get();
            HolderProbed {
                holder: self.speleodb_lock_holder_checked(
                    endpoint,
                    token,
                    insecure_http,
                    project_id,
                ),
            }
        }

        /// Acquire the write lock: after the probe and the flow reset, with the span's
        /// account. A 400 is the server's "someone else holds it"; the caller handles it.
        pub(super) fn speleodb_acquire_after_probe(
            &self,
            _probed: &HolderProbed,
            _flow: &FlowBegun,
            account: &InSpan<'_, (String, String)>,
            insecure_http: bool,
            project_id: &str,
        ) -> Result<Project, SpeleoDbError> {
            let (endpoint, token) = account.get();
            self.speleodb_handle_auth(
                Endpoint::Acquire,
                client::acquire_lock(endpoint, token, insecure_http, project_id),
            )
        }
    }

    impl HolderProbed {
        /// [`probe_verdict`] on this probe.
        pub(super) fn verdict(&self, me: Option<&str>, has_unpushed_edits: bool) -> ProbeVerdict {
            probe_verdict(&self.holder, me, has_unpushed_edits)
        }
    }

    /// FOUR outcomes over three probe states (the full reasoning is at the call site
    /// in `speleodb_start_editing_with`):
    /// - unheld: this call takes it - record it, hand it back if we walk away;
    /// - held by OUR account (`lock_held_by_me`, case-insensitive): inherited - no
    ///   acquisition record, never handed back by an unwind;
    /// - held by someone else, and the acquire still succeeds: they let go in
    ///   between, so this call took it - exactly as unheld;
    /// - unknown (an unreadable probe, or a holder we have no email to compare
    ///   against): no acquisition record, and keep the lock only over un-pushed
    ///   work. "Cannot say" never mints an acquisition.
    ///
    /// [#576 SDB-ACCT-D1] "No acquisition record" is not "no claim": a commit that
    /// starts editing under an inherited or unknown-provenance lock records it as an
    /// ADOPTED (resumed) claim (`speleodb_commit_editing`).
    pub(super) fn probe_verdict(
        holder: &Result<Option<String>, SpeleoDbError>,
        me: Option<&str>,
        has_unpushed_edits: bool,
    ) -> ProbeVerdict {
        let (acquired_here, keep_lock_on_unwind) = match holder {
            // Unheld -> this call took it: record the acquisition, hand it back if we
            // walk away.
            Ok(None) => (true, false),
            // Already ours -> inherited: no acquisition record, and never hand it back
            // on an unwind.
            Ok(Some(holder)) if lock_held_by_me(Some(holder.as_str()), me) => (false, true),
            // Someone else's, and the acquire still succeeded -> they released in
            // between and this call took it: answer exactly as `Ok(None)` does.
            Ok(Some(_)) if me.is_some() => (true, false),
            // Unknown - an unreadable probe, or a holder we have no email to compare
            // against. No record either way, and keep the lock only when there is
            // un-pushed work behind it to protect - as far as the OPEN project's link
            // can tell, which is not the only copy there is (see the unwind's second
            // keep rule, `unwind_keeps_lock`).
            Ok(Some(_)) | Err(_) => (false, has_unpushed_edits),
        };
        ProbeVerdict {
            acquired_here,
            keep_lock_on_unwind,
        }
    }
}

/// [#576 A8 RV8a-5] A concluded landing's proof and its one producer, apart: nothing
/// else in this file (or its tests) can build a [`Landed`].
mod landing {
    use super::{AppState, PushOutcome};

    /// [#576 A8] Proof that a push arm has CONCLUDED an upload landed, and dropped the
    /// remembered dump choice that landing made stale.
    ///
    /// Made only by [`AppState::speleodb_note_prior_push_landed`] (its field is private
    /// to this small module, #576 RV8a-5). Every step that may only follow a landing takes one - the
    /// managed-type heal and each arm's link write - so writing one of them above the
    /// landing it records is a compile error, not a source-text tripwire. That is also
    /// what puts the invalidation ahead of every fallible local write (#258).
    #[must_use = "a concluded landing is what the arm's later steps and its outcome are built on"]
    pub(in crate::core::state) struct Landed {
        _concluded: (),
    }

    /// [#576 NF-A8R-4] The managed-type heal's PATCH follows a concluded landing.
    impl crate::speleodb::client::PatchLicence for Landed {}

    impl Landed {
        /// What an arm that concluded a landing reports.
        pub(in crate::core::state) fn outcome(&self) -> PushOutcome {
            PushOutcome::Confirmed
        }
    }

    impl AppState {
        /// The invalidation on its own, for the arm that concludes a landing WITHOUT
        /// producing an outcome (#258).
        ///
        /// `AdoptThenUpload` is that arm, and it is the reachable one of the two adoption
        /// plans - `AdoptConfirmed` is documented as forward-compatible only while
        /// push-start is unpersisted. It establishes exactly what the others do, that a
        /// prior upload landed, and then keeps going to push on top of it, so it has no
        /// `PushOutcome` to hand back and cannot use the wrapper above. Routing it through
        /// the wrapper's `Confirmed` and discarding that would be worse than this split:
        /// it would read as if the arm had finished.
        ///
        /// Leaving it out was a real hole, not a stylistic one. Its own `mutate_link` can
        /// fail, and the upload it continues into can come back ambiguous - and on either
        /// path the function returns having proved a revision exists that our remembered
        /// choice predates, with the choice still standing.
        pub(in crate::core::state) fn speleodb_note_prior_push_landed(
            &self,
            project_id: &str,
        ) -> Landed {
            self.speleodb_forget_dump_entry(project_id);
            Landed { _concluded: () }
        }
    }
}

pub(super) use landing::Landed;

// The fixtures the sibling test modules (`sdb_*_tests.rs`) share with the tests
// below. Re-exported at the very end of the production code, so a source scan that
// cuts the file at its first top-level `#[cfg(test)]` still sees all of it.
#[cfg(test)]
pub(super) use tests::{
    authed_session_at, editing_fixture, logged_events, recording_stub, remote_readonly_fixture,
    routing_stub, FREE_MUTEX_AT_BASE_BODY, HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
#[expect(
    clippy::panic,
    reason = "one test POISONS a mutex, which requires panicking while its guard is held"
)]
#[expect(
    clippy::print_stdout,
    reason = "the driven A8 tests print the values they observed, so a failure shows them"
)]
mod tests {
    use super::{
        auth_error_drops_token, is_local_edits_refusal, local_dto, lock_held_by_me,
        managed_type_patch, resolve_endpoint, reverify_due, type_needs_managed_label,
        type_says_managed, validate_project_id, whoami_indicates_dead_token, Endpoint,
        SpeleoDbSession, REVERIFY_INTERVAL_SECS,
    };
    // [#275] The push half now lives in `speleodb_push`; these tests still exercise it
    // from here (they share this module's fixtures), so they reach it by its new path.
    use crate::core::state::speleodb_cache::{
        preserved_display_settings, restore_display_settings, InheritedUndo,
    };
    use crate::core::state::speleodb_pull::classify_dump_entry;
    use crate::core::state::speleodb_push::{
        clear_snapshot_dirty, head_with_empty_retry, type_still_says_ariane, PushOutcome,
    };
    use crate::core::state::tests_support::speleodb_prod_matches;
    use crate::models::{Project, SpeleoLink};
    use crate::persistence::Database;
    use crate::speleodb::auth::{account_key, InMemoryStore, SecretStore, TokenStore};
    use crate::speleodb::{
        DoneResult, KeychainReport, PushResult, RemoteState, SpeleoDbError, TransportCause,
    };
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

    /// The half `type_still_says_ariane` cannot answer: "is this project ALREADY the
    /// thing adoption is trying to make it?"
    ///
    /// Adoption asked that with `!type_still_says_ariane`, which is a different
    /// question - it says yes for every label that is not `ariane`. A project re-typed
    /// under a running adoption (someone editing metadata in SpeleoDB's own UI) then
    /// took the "the heal got there first" short-circuit: no PATCH issued, and
    /// `type_flipped: true` reported over a server left on the other type, so the UI
    /// showed the green finished toast for a `.stalx` still advertising itself as
    /// somebody else's format.
    #[test]
    fn only_the_managed_type_counts_as_already_adopted() {
        assert!(type_says_managed(Some("stalagmight")));
        // Same case-insensitivity the Ariane side needs, and for the same reason.
        assert!(type_says_managed(Some("STALAGMIGHT")));
        assert!(type_says_managed(Some(" Stalagmight ")));
        assert!(
            !type_says_managed(Some("other")),
            "the legacy type is never mapped, so adoption's finalize must relabel it"
        );

        assert!(
            !type_says_managed(Some("ariane")),
            "the state adoption exists to leave behind, not one it may skip"
        );
        assert!(
            !type_says_managed(Some("compass")),
            "NOT managed - this is the case the old `!still_ariane` test got wrong"
        );
        assert!(!type_says_managed(Some("therion")), "likewise");
        assert!(
            !type_says_managed(Some("")),
            "an absent type field is not a claim that it is managed"
        );
        assert!(
            !type_says_managed(None),
            "a type we could not read is not a type we may call finished"
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
        assert_eq!(managed_type_patch(), r#"{"type":"stalagmight"}"#);
    }

    /// The predicate and body above only matter if the writers use them. The heal
    /// gates on `type_needs_managed_label` (so a legacy `other` is relabelled, not
    /// only an unfinished `ariane`), and no SpeleoDB source writes a hand-made type
    /// body that could drift back to `other`.
    #[test]
    fn the_type_writers_go_through_the_shared_helpers() {
        let src = crate::models::source_scan::prod_region(
            include_str!("speleodb_push.rs"),
            "impl AppState {",
        );
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
            2,
            "the heal and adoption's finalize both PATCH through the helper"
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
    /// [#576 A8] That a heal can only FOLLOW its landing is the type's job now: the
    /// heal takes the [`super::Landed`] proof only the landing makes. What stays a
    /// sanctioned source scan (docs/testing-recipes.md, "Source scans") is the site
    /// inventory, which no type can say: exactly the 200 and 304 arms heal.
    #[test]
    fn every_landing_upload_arm_finishes_the_adoption() {
        assert_eq!(
            speleodb_prod_matches("speleodb_heal_managed_type(", &[]),
            3,
            "RULE (finish the adoption on every landing upload): one definition plus the \
             200 and 304 landing arms of `speleodb_upload` - a landing arm added without \
             the heal, or a heal added outside one, changes this count"
        );
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

        // Snapshot the prefs, then simulate the server dump landing (a fresh file
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

    // --- guarded close before a cache download-replace ---

    /// Write a minimal openable `.stalx` at `path` and return it.
    fn seed_stalx(path: &Path, name: &str) {
        let mut db = Database::open(path).expect("open seed db");
        db.save_project(&Project::new(name.to_owned(), None))
            .expect("save seed project");
    }

    /// Whether a project (and its database handle) is currently open in `state`.
    fn project_is_open(state: &AppState) -> bool {
        let project = state.project.lock().is_some();
        let database = state.database.lock().is_some();
        assert_eq!(
            project, database,
            "project and database handles should open/close together"
        );
        project
    }

    #[test]
    fn guarded_close_fires_when_dest_is_the_open_project() {
        // The download-replace must not truncate a file this process still has an
        // mmap-enabled SQLite connection to (SIGBUS / torn reads on Unix, a denied
        // rename on Windows), so the live handle is dropped first.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");
        seed_stalx(&dest, "Cache");

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        assert!(project_is_open(&state));

        assert!(
            state.close_project_if_open_at(&dest),
            "the open project IS this file, so it must be closed"
        );
        assert!(
            !project_is_open(&state),
            "the live handle must be gone before the replace"
        );
    }

    #[test]
    fn guarded_close_ignores_a_different_path() {
        // Conditional on purpose: a pull can target a cached project the user does
        // not have open, and dropping THAT project would throw away its undo history
        // for nothing.
        let temp = TempDir::new().expect("temp dir");
        let open_path = temp.path().join("open.stalx");
        let other = temp.path().join("other.stalx");
        seed_stalx(&open_path, "Open");
        seed_stalx(&other, "Other");

        let state = AppState::new().expect("state");
        assert!(state.open_project(open_path).success, "open a project");

        assert!(
            !state.close_project_if_open_at(&other),
            "a different file must not close the open project"
        );
        assert!(
            project_is_open(&state),
            "the unrelated open project must stay open"
        );
    }

    #[test]
    fn guarded_close_ignores_a_dest_that_does_not_exist_yet() {
        // A first pull writes into an empty cache: `dest` cannot be canonicalized
        // and cannot be the file a live connection holds.
        let temp = TempDir::new().expect("temp dir");
        let open_path = temp.path().join("open.stalx");
        seed_stalx(&open_path, "Open");
        let missing = temp.path().join("not-downloaded-yet.stalx");

        let state = AppState::new().expect("state");
        assert!(state.open_project(open_path).success, "open a project");

        assert!(
            !state.close_project_if_open_at(&missing),
            "a not-yet-existing dest is never the open project"
        );
        assert!(project_is_open(&state), "open project untouched");
    }

    #[test]
    fn guarded_close_still_drops_the_handle_when_a_lock_is_poisoned() {
        // The guard recovers a poisoned lock for the path READ, but the close it
        // then performs used to be written as `if let Ok(..)` per lock - so on a
        // poisoned mutex it dropped NOTHING and still reported `true`, and the
        // caller went on to rename a new file over a live SQLite handle and its
        // 256 MB mmap. Poisoning only means an earlier call panicked while
        // holding the lock; the guarded value is an `Option`, and emptying it is
        // exactly the recovery.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");
        seed_stalx(&dest, "Cache");

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        assert!(project_is_open(&state));

        // Poison the database lock the way a real panic would: panic while its
        // guard is held, and swallow the unwind so the test itself survives.
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.database.lock();
            panic!("simulated panic while holding the database lock");
        }));
        assert!(poisoned.is_err(), "the closure must have panicked");
        assert!(
            state.database.is_poisoned(),
            "the database lock must now be poisoned"
        );

        assert!(
            state.close_project_if_open_at(&dest),
            "the open project IS this file, so it must be closed"
        );
        assert!(
            state.database.lock().is_none(),
            "the live database handle must be gone before the replace, poisoned or not"
        );
        assert!(
            state.project.lock().is_none(),
            "the project must be dropped too"
        );
    }

    #[test]
    fn guarded_close_matches_through_a_non_canonical_spelling() {
        // The open path and `dest` are frequently derived differently (cache-path
        // join vs. the FE's opened path), so the comparison canonicalizes.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");
        seed_stalx(&dest, "Cache");
        let round_about = temp.path().join("sub").join("..").join("cache.stalx");
        std::fs::create_dir(temp.path().join("sub")).expect("mkdir sub");

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest).success, "open the cache");
        assert!(
            state.close_project_if_open_at(&round_about),
            "the same file spelled differently must still match"
        );
        assert!(!project_is_open(&state));
    }

    #[test]
    fn replace_cache_stalx_closes_preserves_and_swaps_atomically() {
        // The chokepoint every download-replace goes through: preserve the local
        // view toggles, drop the live handle to the destination, then swap in the
        // downloaded copy via temp+rename. The destination is a clean managed copy
        // of the project: a link-less file there is refused (#576 FZ-SDB-5-D3).
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("cache.stalx");
        seed_linked_stalx(&dest, GUARD_ID, false);
        {
            let db = Database::open(&dest).expect("open local cache");
            db.save_display_setting("grid", "off").expect("save grid");
        }

        let zip = server_dump_zip();

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");

        let mut dump = crate::speleodb::dump::Dump::open(&zip).expect("open dump");
        let preserved = state
            .replace_cache_stalx(
                &mut dump,
                "misc/project.stalx",
                &dest,
                GUARD_ID,
                InheritedUndo::Drop,
            )
            .expect("replace the cache file");

        assert_eq!(
            preserved.get("grid").map(String::as_str),
            Some("off"),
            "local view toggles are captured before the replace"
        );
        assert!(
            !project_is_open(&state),
            "the live handle is dropped before the file is replaced"
        );
        let replaced = Database::open_existing(&dest).expect("open replaced file");
        assert_eq!(
            replaced.load_project().expect("load replaced project").name,
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

    /// A one-entry server dump whose `.stalx` payload is recognisable on disk.
    fn server_dump_zip() -> Vec<u8> {
        use std::io::Write as _;
        let temp = TempDir::new().expect("server project temp dir");
        let project = temp.path().join("server.stalx");
        seed_stalx(&project, "Server Copy");
        let bytes = std::fs::read(project).expect("read server project");
        let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer
            .start_file("misc/project.stalx", opts)
            .expect("start_file");
        writer.write_all(&bytes).expect("write entry");
        writer.finish().expect("finish zip").into_inner()
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
                unlinked: false,
                state_unknown,
            } => Some((project_id, local_path, last_edit_at, state_unknown)),
            _ => None,
        };
        let (project_id, local_path, last_edit_at, state_unknown) =
            refusal.expect("expected the un-pushed-edits refusal");
        assert!(!state_unknown, "the copy was read, and read as dirty");
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
            unlinked: false,
            state_unknown: false,
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
        assert!(!payload.state_unknown, "a copy read as dirty");
        assert_eq!(
            dto.message, "this project has local edits that were never pushed to SpeleoDB",
            "the dirty refusal keeps its line"
        );
    }

    #[test]
    fn an_unreadable_copy_crosses_ipc_as_the_same_routing_signal_flagged_unknown() {
        // [#576 W53] The guard's other refusal routes exactly like the dirty one (the
        // frontend still offers keep-local or take-remote), but the payload says the
        // copy could not be read, and the message says so instead of claiming edits.
        let dto = local_dto(SpeleoDbError::WouldDiscardLocalEdits {
            project_id: GUARD_ID.to_owned(),
            local_path: "/cache/speleodb/p/project.stalx".to_owned(),
            last_edit_at: None,
            unlinked: false,
            state_unknown: true,
        });
        assert_eq!(
            dto.action,
            crate::speleodb::SpeleoDbAction::UnpushedLocalEdits
        );
        let payload = dto
            .unpushed_local
            .expect("the refusal carries what the choice needs");
        assert_eq!(payload.local_path, "/cache/speleodb/p/project.stalx");
        assert!(payload.state_unknown, "flagged as unreadable on the wire");
        assert!(
            dto.message.contains("could not be read") && !dto.message.contains("never pushed"),
            "the message names what is known, not edits: {:?}",
            dto.message
        );
    }

    #[test]
    fn guard_allows_a_clean_or_absent_local_copy() {
        // No behaviour change when there is nothing to lose: a clean cached copy and
        // a first pull into an empty cache replace exactly as before.
        let temp = TempDir::new().expect("temp dir");
        let state = AppState::new().expect("state");

        let clean = temp.path().join("clean.stalx");
        seed_linked_stalx(&clean, GUARD_ID, false);
        assert!(state.guard_local_edits(GUARD_ID, &clean).is_ok());

        let missing = temp.path().join("not-pulled-yet.stalx");
        assert!(
            state.guard_local_edits(GUARD_ID, &missing).is_ok(),
            "a first pull has nothing to protect"
        );
    }

    /// [#576 FZ-SDB-5-D3] A `.stalx` with no link row where the replace would land -
    /// the copy an Unlink left in the cache before Unlink moved it out - is
    /// possible un-pushed work: the guard refuses with the keep-local / take-remote
    /// refusal, flagged `unlinked` so keep-local moves it out of the cache. It used
    /// to read as clean and be replaced without a word, edits and all.
    #[test]
    fn guard_refuses_an_unlinked_copy_at_the_destination() {
        let temp = TempDir::new().expect("temp dir");
        let state = AppState::new().expect("state");
        let unlinked = temp.path().join("unlinked.stalx");
        seed_stalx(&unlinked, "Unlinked");

        let err = state
            .guard_local_edits(GUARD_ID, &unlinked)
            .expect_err("an unlinked copy must not be replaced without asking");
        let seen = match &err {
            SpeleoDbError::WouldDiscardLocalEdits {
                project_id,
                local_path,
                unlinked,
                ..
            } => Some((project_id.clone(), local_path.clone(), *unlinked)),
            _ => None,
        };
        println!("[FZ-SDB-5-D3] guard over an unlinked copy -> {err:?}");
        assert_eq!(
            seen,
            Some((
                GUARD_ID.to_owned(),
                unlinked.to_string_lossy().into_owned(),
                true
            )),
            "the keep-local / take-remote refusal, naming the copy and flagged unlinked"
        );
        let dto = local_dto(err);
        assert_eq!(
            dto.unpushed_local.map(|p| p.unlinked),
            Some(true),
            "the flag crosses IPC"
        );
        assert!(
            unlinked.exists(),
            "the guard writes nothing: the copy is still there"
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
            matches!(
                err,
                SpeleoDbError::WouldDiscardLocalEdits {
                    state_unknown: true,
                    ..
                }
            ),
            "expected the refusal for a copy that could not be read, got {err:?}"
        );
    }

    #[test]
    fn a_failed_extract_reopens_the_project_it_closed() {
        // The close has to come BEFORE the extract (a live mmap must not survive into
        // the rename), so a failed extract left the project closed backend-side while
        // the frontend still displayed it - and every later command, auto-save
        // included, then failed with "no project open". The extract stages to a temp
        // and renames, so on failure `dest` is untouched and re-opening restores the
        // pre-call state.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, GUARD_ID, false);
        let before = std::fs::read(&dest).expect("read seeded cache");
        let zip = server_dump_zip();

        let state = AppState::new().expect("state");
        assert!(state.open_project(dest.clone()).success, "open the cache");
        assert!(project_is_open(&state));

        let mut dump = crate::speleodb::dump::Dump::open(&zip).expect("open dump");
        let err = state
            .replace_cache_stalx_abandoning(
                &mut dump,
                "no/such/entry.stalx",
                &dest,
                InheritedUndo::Drop,
            )
            .expect_err("extracting a missing entry must fail");
        assert!(
            !matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }),
            "the failure here is the extract, not the guard: {err:?}"
        );

        assert_eq!(
            std::fs::read(&dest).expect("read cache after the failure"),
            before,
            "a failed extract leaves the old copy intact"
        );
        assert!(
            project_is_open(&state),
            "and the project it closed to make room is re-opened, so the shell is \
             not left showing a project the backend does not have open"
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
    fn protected_replace_refuses_and_abandoning_replace_still_overwrites() {
        // The two chokepoint verbs, on the same dirty destination: the plain one
        // leaves the file byte-identical, and the explicitly-named abandoning one
        // (take-remote, behind the frontend's destroy-confirmation) still works.
        let temp = TempDir::new().expect("temp dir");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, GUARD_ID, true);
        let before = std::fs::read(&dest).expect("read seeded cache");
        let zip = server_dump_zip();
        let state = AppState::new().expect("state");

        let mut dump = crate::speleodb::dump::Dump::open(&zip).expect("open dump");
        let refused = state
            .replace_cache_stalx(
                &mut dump,
                "misc/project.stalx",
                &dest,
                GUARD_ID,
                InheritedUndo::Drop,
            )
            .expect_err("the guarded verb must refuse");
        assert!(matches!(
            refused,
            SpeleoDbError::WouldDiscardLocalEdits { .. }
        ));
        assert_eq!(
            std::fs::read(&dest).expect("read cache after the refusal"),
            before,
            "a refused replace must not touch the file at all"
        );

        let mut dump = crate::speleodb::dump::Dump::open(&zip).expect("reopen dump");
        state
            .replace_cache_stalx_abandoning(
                &mut dump,
                "misc/project.stalx",
                &dest,
                InheritedUndo::Drop,
            )
            .expect("the abandoning verb still discards on purpose");
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
        //     2. push ancestry Landed - advance BASE to tip, for BOTH adoption plans
        //        (`AdoptThenUpload`, `AdoptConfirmed`) through their one write step,
        //        `speleodb_adopt_tip_after_landing` (#576 A8: it takes the landing)
        //     3. speleodb_publish recovery - adopt the server tip when the
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
        //
        // A sanctioned source scan (#576 A8-D2, docs/testing-recipes.md "Source scans"):
        // "BASE moves only at these triggers" is a site inventory no type expresses.
        assert_eq!(
            speleodb_prod_matches("base_commit_sha =", &[]),
            3,
            "RULE (BASE advances only at sanctioned triggers, #192/#193/#197): unexpected \
             BASE mutate-advance count (sanctioned: push-200, the ancestry-landed write \
             step, publish tip recovery)"
        );
        assert_eq!(
            speleodb_prod_matches("base_commit_sha:", &[]),
            5,
            "RULE (BASE is stamped only at sanctioned triggers, #192/#193/#197): unexpected \
             BASE stamp count (sanctioned: open-on-pull, fast-forward restamp, \
             publish-create, adopt-ariane, revision-restore)"
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

    #[test]
    fn forget_clears_force_editing_on_an_auto_dropped_force_edit_session() {
        // #213 + the 401 auto-drop, together: the flag has to go with the gate it
        // rides on. It did not, and `speleodb_link_status` reports it on every poll -
        // so the frontend cleared `forceEditing` on the token loss and the very next
        // poll re-seeded it true. Its big-red force-edit banner then pre-empted the
        // durable "your changes are only on this computer" push-failure banner,
        // hiding the Retry and the typed commit message behind a banner whose only
        // control (Reconnect) 401s again - while `guard_mutation` refused every edit.
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        enable_force_edit_optin(&state);
        state.speleodb_force_edit().expect("force-edit opted in");
        assert!(
            state.edit_enabled() && state.force_editing(),
            "precondition: a live force-edit session"
        );

        state.speleodb_forget();

        assert!(!state.edit_enabled(), "the auto-drop closes the edit gate");
        assert!(
            !state.force_editing(),
            "and clears force-edit with it, so the push-failure surface is reachable"
        );
        // The work itself is untouched - the dirty bit is what the banner is about.
        assert!(
            state.speleodb_open_link().expect("link").has_unpushed_edits,
            "the un-pushed edits survive the auto-drop"
        );
    }

    /// A read-only remote fixture (saved project + `speleo_link`) opened in a fresh
    /// `AppState`. Mirrors `forget_drops_gate_on_remote_edit_session`; the config
    /// opt-in is left OFF (a test enables it when needed). #213.
    pub(in crate::core::state) fn remote_readonly_fixture() -> (TempDir, AppState) {
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
        assert!(!state.force_editing(), "remote opens not force-editing");
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
            matches!(pushed, PushResult::Pushed),
            "an open gate with a clean link is a genuine no-op, got {pushed:?}"
        );
    }

    fn enable_force_edit_optin(state: &AppState) {
        state.config.lock().speleodb.allow_force_edit = true;
    }

    #[test]
    fn force_edit_refused_without_optin() {
        // Off by default: a remote read-only project stays read-only; force-edit is
        // refused and never flips either gate.
        let (_temp, state) = remote_readonly_fixture();
        assert!(state.speleodb_force_edit().is_err(), "opt-in is required");
        assert!(
            !state.edit_enabled(),
            "still read-only after a refused force-edit"
        );
        assert!(!state.force_editing(), "the flag is never set on refusal");
    }

    #[test]
    fn force_edit_flips_both_gates_when_opted_in() {
        // Opt-in: force-edit enables editing WITHOUT a lock (edit_enabled +
        // force_editing), no acquire, no network.
        let (_temp, state) = remote_readonly_fixture();
        enable_force_edit_optin(&state);
        state
            .speleodb_force_edit()
            .expect("force-edit allowed once opted in");
        assert!(state.edit_enabled(), "force-edit enables editing");
        assert!(
            state.force_editing(),
            "and marks it as force-editing (no lock)"
        );
        // BASE is untouched (no acquire): the link's base_commit_sha is unchanged.
        let link = state.speleodb_open_link().expect("link");
        assert_eq!(link.base_commit_sha.as_deref(), Some("abc123"));
    }

    #[test]
    fn force_edit_refused_when_already_lock_held() {
        // If we already hold the lock (normal editing), there is nothing to force.
        let (_temp, state) = remote_readonly_fixture();
        enable_force_edit_optin(&state);
        state.set_edit_enabled(true); // simulate Start editing acquiring the lock
        assert!(
            state.speleodb_force_edit().is_err(),
            "no force-edit while genuinely lock-held"
        );
        assert!(
            !state.force_editing(),
            "lock-held editing is not force-editing"
        );
    }

    #[test]
    fn force_editing_exempts_reverify_and_lock_release() {
        // A force-editor holds no lock: the #202 re-verify is a no-op and the
        // close/exit/switch lock-release snapshot yields nothing to release.
        let (_temp, state) = remote_readonly_fixture();
        state.set_edit_enabled(true);
        state.set_force_editing(true);
        assert!(
            state.speleodb_verify_lock_if_expired().is_ok(),
            "re-verify never fires for a force-editor"
        );
        assert!(
            state.speleodb_held_lock_release_params().is_none(),
            "a force-editor has no held lock to release"
        );
    }

    /// An authenticated session pointed at `endpoint` (the loopback stub below, or
    /// any unreachable host), so the release paths get past `speleodb_active_token`.
    pub(in crate::core::state) fn authed_session_at(endpoint: &str) -> SpeleoDbSession {
        let mut token_store = TokenStore::from_stores(Box::new(InMemoryStore::new()), None);
        token_store.store(endpoint, "u@x", "tok-1", false);
        SpeleoDbSession {
            endpoint: Some(endpoint.to_owned()),
            email: Some("u@x".to_owned()),
            token_store,
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
        }
    }

    /// The project id `remote_readonly_fixture` links to.
    pub(in crate::core::state) const REMOTE_ID: &str = "11111111-1111-1111-1111-111111111111";

    /// The remote fixture mid-edit-session: the write lock is held (edit gate on,
    /// not force-editing) and a token is available, so the silent release gate has
    /// everything it needs. Clean to start with - the dirty bit is the test's lever.
    ///
    /// The acquisition is recorded too, exactly as a successful Start editing does:
    /// this fixture stands for a session that TOOK the lock, and that provenance -
    /// not the edit gate - is what licenses the authorship shortcut in
    /// `speleodb_remote_state`. (The session assignment resets it, so it is recorded
    /// after.)
    pub(in crate::core::state) fn editing_fixture(endpoint: &str) -> (TempDir, AppState) {
        let (temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(endpoint);
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
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    #[test]
    fn silent_lock_release_is_skipped_when_the_project_has_unpushed_edits() {
        // The close / project-switch / app-exit release is SILENT, so it must never
        // free the server mutex while local edits are stranded on this machine:
        // the project would immediately be lockable by someone else and the edits
        // could then only come back through a divergence. Keeping the lock is
        // recoverable; stranding the edits is not.
        let (_temp, state) = editing_fixture("https://x/");
        assert!(
            state.speleodb_held_lock_release_params().is_some(),
            "a CLEAN edit session still releases its lock on close/exit/switch"
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
    fn silent_lock_release_still_honours_the_force_edit_and_read_only_gates() {
        // The dirty-bit veto is additive: the pre-existing gates behave as before.
        let (_temp, state) = editing_fixture("https://x/");
        state.set_force_editing(true);
        assert!(
            state.speleodb_held_lock_release_params().is_none(),
            "a force-editor holds no lock to release (#213)"
        );
        state.set_force_editing(false);
        state.set_edit_enabled(false);
        assert!(
            state.speleodb_held_lock_release_params().is_none(),
            "a read-only open holds no lock to release"
        );
        state.set_edit_enabled(true);
        assert!(
            state.speleodb_held_lock_release_params().is_some(),
            "and a clean, lock-held session is unaffected by the new veto"
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
    fn a_release_leaves_a_force_edit_session_alone() {
        // #213: a force-editor holds NO server lock - that is the definition of
        // force-edit - so neither release verb is surrendering anything of theirs.
        // Both used to drop the edit gate anyway (they tested `edit_enabled` alone),
        // which ended an editing session the release had nothing to do with AND left
        // the invalid `edit_enabled == false && force_editing == true` pair: a
        // read-only project still reporting the big-red force-edit banner through
        // `speleodb_link_status`. Reachable from the held-lock resume prompt, whose
        // scan lists a STALE lock from an earlier session on the very project the
        // user is now force-editing.
        //
        // All three release verbs now ask the same pair (`edit_enabled() &&
        // !force_editing()`), which `speleodb_release_current_lock` had right all
        // along; the frontend already models the two as independent
        // (`holdsWriteLock` is `editing && !forceEditing`).
        let link_id = "11111111-1111-1111-1111-111111111111";
        for per_project in [true, false] {
            let (base, handle) = release_stub(link_id);
            let (_temp, state) = editing_fixture(&base);
            state.set_force_editing(true);

            if per_project {
                state
                    .speleodb_release_lock(link_id.to_owned())
                    .expect("the release itself still goes through");
            } else {
                state
                    .speleodb_release_all_locks()
                    .expect("the release itself still goes through");
            }

            assert!(
                state.edit_enabled(),
                "a force-edit session survives the release (per_project={per_project})"
            );
            assert!(
                state.force_editing(),
                "and stays flagged as force-editing, so the two gates never disagree \
                 (per_project={per_project})"
            );
            let _ = handle.join();
        }
    }

    #[test]
    fn a_release_that_lands_after_an_unlink_leaves_the_gate_open() {
        // `speleodb_release_current_lock` used to sample "was this lock ours?" BEFORE
        // its blocking release and apply the answer after it, holding nothing across
        // the round trip. Dismiss the Settings dialog and unlink the same project in
        // that window and the two collide: the unlink makes it a plain local project
        // and reopens the edit gate, and this stale completion closes it again.
        // Nothing reopens it from there - status polling needs a link the project no
        // longer has - so an ordinary local project was stuck read-only until it was
        // closed and reopened.
        //
        // The unlink runs FROM the stub, between the request arriving and the response
        // going out, which is exactly the window and needs no timing luck. The
        // listener is dropped first so the unlink's own best-effort lock release is
        // refused instantly (`release_lock` is a non-idempotent POST - one attempt, no
        // backoff), which is just the offline no-op that path is written for.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let base = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().expect("addr").port()
        );
        let (_temp, state) = editing_fixture(&base);
        assert!(state.edit_enabled(), "precondition: we hold the lock");

        std::thread::scope(|scope| {
            scope.spawn(|| {
                use std::io::{Read as _, Write as _};
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                }
                drop(listener);
                state.speleodb_unlink_in_place().expect("unlink mid-flight");
                let body = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
            state
                .speleodb_release_current_lock()
                .expect("the release itself succeeds");
        });

        assert!(
            state.speleodb_open_link().is_err(),
            "precondition: the project is no longer linked"
        );
        assert!(
            state.edit_enabled(),
            "a release completing over a project that is no longer remote must not \
             close the gate the unlink just opened"
        );
    }

    #[test]
    fn a_force_editor_does_not_claim_another_sessions_commit_as_its_own() {
        // The authorship shortcut in `poll_verdict` explains a commit in the BASE..tip
        // gap as OUR unrecorded push, and it is licensed by holding the single-writer
        // lock. A #213 force-editor holds NO lock - it opens the edit gate precisely
        // because it could not take one - so `edit_enabled` alone was not the claim
        // the comment above `we_hold_the_lock` said it was.
        //
        // The setup is the concurrent-session case `poll_verdict` already refuses on
        // its own inputs: a second session on the SAME account holds the real lock and
        // has pushed, so the lock holder's email matches ours. Reported as UpToDate the
        // status button keeps its resting state, and the divergence the user will
        // actually hit on Reconnect goes unannounced.
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

        state.set_force_editing(true);
        assert_eq!(
            state.speleodb_remote_state(&base, "tok-1", false, &link, Some("u@x")),
            RemoteState::Conflict,
            "a force-editor holds no lock, so a gap commit carrying our email is the \
             other session's - fail closed to the divergence prompt"
        );

        state.set_force_editing(false);
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
        *state.speleodb.lock() = authed_session_at("http://x/");

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
        // The record answers "did I take this lock?", the gate answers "may I edit?",
        // and the shortcut needs BOTH. The poll's catch-all clear is what normally
        // keeps them in step, but it runs at the TOP of `speleodb_link_status` while
        // the verdict is folded further down, after a network round trip - so a path
        // that shuts the gate in between (a concurrent same-path reopen re-deriving it
        // from `is_remote`) leaves a live record over a project that is no longer
        // editable, and the shortcut was still granted. A session that may not edit
        // must not be licensed to claim a gap commit as its own push.
        //
        // Same fixture and body as the force-editor sibling above: BASE behind the tip
        // by one commit carrying our email, so the verdict flips on this input alone.
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
    /// other endpoint owns, so the single-body stub can serve a whole poll. The
    /// mutex holder is OUR OWN account - a second session signed in as the same
    /// user - and the tip has moved one commit past BASE, authored by that account.
    const SAME_ACCOUNT_HOLDER_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "active_mutex":{"user":"u@x"},
        "commits":[{"id":"def456","parent_ids":["abc123"],"author_email":"u@x"},
                   {"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn a_second_session_on_the_same_account_never_inherits_the_holders_authorship() {
        // The reported hole, and why the two halves of `we_hold_the_lock` were never
        // independent: `speleodb_link_status` RE-DERIVES the edit gate from the lock
        // holder's email (so a project you locked and never released reopens editable
        // rather than stuck read-only under your own lock). Pairing that gate with the
        // same holder email therefore read ONE signal twice.
        //
        // Here a second session on the same account holds the real lock and has
        // pushed. This session never acquired anything: poll 1 re-derives the gate
        // from the holder email, and poll 2 - reading the gate that poll 1 opened -
        // used to hand the shortcut to a process that had pushed nothing, explaining
        // the OTHER session's commit away as its own unrecorded push and reporting
        // UpToDate over a real conflict.
        let (base, _listener) = always_answering_stub(SAME_ACCOUNT_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock() = authed_session_at(&base);
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "precondition: this process has taken no lock - it opened the project \
             read-only, exactly as a restart or a second window does"
        );

        let first = state.speleodb_link_status().expect("poll 1");
        assert_eq!(
            first.lock_holder.as_deref(),
            Some("u@x"),
            "precondition: the server names our own account as the holder"
        );
        assert!(
            first.edit_enabled,
            "the re-derivation this defect rides on is deliberate and must survive: \
             a project held under our own lock reopens editable"
        );
        assert_eq!(
            first.remote_state,
            RemoteState::Conflict,
            "the other session's commit is not ours to explain away"
        );

        let second = state.speleodb_link_status().expect("poll 2");
        assert_eq!(
            second.remote_state,
            RemoteState::Conflict,
            "and it is still not ours on the next poll, when the gate the first poll \
             opened is what `edit_enabled` reports - the verdict must not be laundered \
             through a gate derived from the very holder email it is corroborating"
        );

        // The other side of the line: a process that really did take this lock keeps
        // the shortcut, so its own landed push is not painted as a conflict.
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));
        let third = state.speleodb_link_status().expect("poll 3");
        assert_eq!(
            third.remote_state,
            RemoteState::UpToDate,
            "an acquire made by THIS process still licenses the authorship recovery"
        );
    }

    #[test]
    fn done_does_not_adopt_a_same_account_commit_this_session_did_not_make() {
        // [#576 B32] The click half of the test above. A second session on the SAME
        // account holds the lock and has pushed `def456` on top of our BASE; this
        // process opened the project read-only and had its gate reopened by the
        // inherited-lock poll, so it never took the lock and holds no claim. The
        // poll calls that gap a Conflict. Done used to call it our own landed push:
        // `plan_push` matched the commit by email alone, adopted `def456` as BASE
        // and uploaded our older copy on top, so the other session's revision
        // silently dropped out of the tip. Without the licence it must open the
        // conflict dialog instead, exactly as the button already says.
        let (base, _listener, seen) = recording_stub(SAME_ACCOUNT_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock() = authed_session_at(&base);
        state.set_edit_enabled(true);
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "precondition: this process took no lock - its gate came from the \
             inherited-lock reopen, as in a second window on the same account"
        );

        let result = state.speleodb_done(None);

        let link = state.speleodb_open_link().expect("link");
        assert_eq!(
            link.base_commit_sha.as_deref(),
            Some("abc123"),
            "BASE adopted the other session's commit {:?} (expected abc123): Done \
             treated a commit this session did not make as its own; result {result:?}",
            link.base_commit_sha
        );
        assert!(
            matches!(result, Ok(DoneResult::DivergenceNeeded)),
            "an unlicensed same-account gap is a divergence, got {result:?}"
        );
        assert!(
            link.has_unpushed_edits,
            "the local edits stay marked un-pushed for the conflict dialog"
        );
        let seen = seen.lock().expect("request log").clone();
        assert!(
            !seen
                .iter()
                .any(|r| r.contains("/upload/") || r.contains("/release/")),
            "nothing may be uploaded over the other session's revision, and its lock \
             must not be released; saw {seen:?}"
        );
    }

    #[test]
    fn done_still_adopts_our_own_landed_push_under_the_lock_we_took() {
        // [#576 B32] DO-NOT-REGRESS: the recovery the licence gates. Same world as the
        // test above, but THIS process took the lock (the claim Start editing
        // records), so a commit in the gap carrying our email can only be our own
        // upload whose reply was lost. Done must keep adopting it rather than raise
        // a divergence against the user's own work.
        //
        // The upload that follows the adoption cannot run here (it needs the app
        // cache dir only the live tests initialise), so the adoption is observed on
        // BASE, which the adopt arm writes before it uploads.
        let (base, _listener) = always_answering_stub(SAME_ACCOUNT_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock() = authed_session_at(&base);
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));

        let result = state.speleodb_done(None);

        assert!(
            !matches!(result, Ok(DoneResult::DivergenceNeeded)),
            "our own landed push is not a divergence, got {result:?}"
        );
        assert_eq!(
            state
                .speleodb_open_link()
                .expect("link")
                .base_commit_sha
                .as_deref(),
            Some("def456"),
            "the licensed recovery adopts our own landed commit as BASE"
        );
    }

    #[test]
    fn only_an_acquire_made_by_this_process_records_the_authorship_licence() {
        // The provenance is written by the acquire itself and given back with the
        // lock. A history whose tip IS our BASE reconciles UpToDate, so Start editing
        // keeps the lock it just took without a fast-forward (which a unit test cannot
        // reach - the reload needs an app cache dir only the live tests initialise).
        let (base, _listener) = always_answering_stub(
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote",
                "commits":[{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#,
        );
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&base);
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
    pub(in crate::core::state) const HELD_BY_SAME_ACCOUNT_AT_BASE_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "active_mutex":{"user":"u@x"},
        "commits":[{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn a_successful_acquire_over_an_inherited_lock_records_no_authorship() {
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
        *state.speleodb.lock() = authed_session_at(&base);

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
            !state.speleodb_lock_taken_here(REMOTE_ID),
            "but a lock the account ALREADY held was not taken by this process, so \
             no acquisition may be recorded"
        );
        // [#576 SDB-ACCT-D1] It is the user's lock all the same, and editing started
        // under it: recorded as ADOPTED (a resumed claim), which licenses the release
        // and nothing else.
        assert!(
            state.speleodb_lock_resumed_here(REMOTE_ID),
            "the account's lock is adopted for this session, for the release only"
        );
    }

    #[test]
    fn resuming_an_inherited_lock_over_a_diverged_tip_never_hands_it_back() {
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
        *state.speleodb.lock() = authed_session_at(&base);

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
    fn a_freshly_taken_lock_is_still_released_on_divergence() {
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
        *state.speleodb.lock() = authed_session_at(&base);

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
    fn a_lock_another_holder_released_just_before_our_acquire_is_still_handed_back() {
        // The probe answers "held", which used to be read as INHERITED on the strength
        // of the mutex being non-empty alone - discarding WHO held it. But another
        // holder who is still holding never reaches these arms (the acquire 400s and
        // returns `LockedBy`), so a held mutex followed by a successful acquire means
        // that holder released in between and THIS call made the unheld -> ours
        // transition. Treating it as inherited skipped the unwind release and stranded
        // a mutex we had just taken over an edit session that never started, blocking
        // every other writer until the server-side timeout.
        //
        // Same diverged history and dirty fixture as the two tests above, so the only
        // input that differs is the mutex holder's identity.
        let (base, _listener, seen) = recording_stub(OTHER_HOLDER_DIVERGED_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock() = authed_session_at(&base);

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
    pub(in crate::core::state) const HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "active_mutex":{"user":"someone.else@x"},
        "commits":[{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn a_lock_another_holder_released_just_before_our_acquire_records_authorship() {
        // The other half of the same race. Classifying it as inherited also withheld
        // the authorship record from a lock this process demonstrably took, and the
        // shortcut in `speleodb_remote_state` hangs on that record: without it, our own
        // landed-but-unrecorded push later reads as a stranger's commit and the status
        // button paints `Conflict` against our own work.
        let (base, _listener, seen) = recording_stub(HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&base);

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
    fn a_holder_differing_only_in_case_is_still_our_own_inherited_lock() {
        // DO-NOT-REGRESS: passes before the who-holds-it split as well as after, and
        // exists to pin the direction the split must NOT be taken in. Now that the
        // held-mutex arm compares the holder against our email, a case-SENSITIVE
        // comparison would read our own account back in a different case as "someone
        // else", hand the resume an authorship claim it never earned, and release the
        // very mutex the held-lock prompt's "Continue editing" exists to keep - over
        // un-pushed work that is still only on this machine.
        let (base, _listener, seen) = recording_stub(SAME_ACCOUNT_UPPERCASE_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock() = authed_session_at(&base);

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
    fn a_poll_over_a_shut_gate_drops_a_stale_acquisition_record() {
        // The catch-all for every way a lock-held session can end without passing
        // through a release verb in this file: a close / project switch / reopen, the
        // detached switch-time release, an admin release, the 401 auto-drop. All of
        // them leave the edit gate shut, and a shut gate means this process is not
        // editing under a lock of its own - so the poll drops the record BEFORE its
        // own re-derivation can reopen the gate and hand the shortcut back.
        let (base, _listener) = always_answering_stub(SAME_ACCOUNT_HOLDER_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        *state.speleodb.lock() = authed_session_at(&base);
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));
        // The session ends somewhere this file cannot hook (here: the reopen after a
        // project switch, which re-derives the gate from is_remote).
        state.set_edit_enabled(false);

        let status = state.speleodb_link_status().expect("poll");
        assert!(
            status.edit_enabled,
            "the reopen re-derivation still reopens the gate under our own lock"
        );
        assert_eq!(
            status.remote_state,
            RemoteState::Conflict,
            "but the stale record no longer licenses the shortcut"
        );
        assert!(
            !state.speleodb_lock_taken_here(REMOTE_ID),
            "and the record itself is gone, so later polls cannot resurrect it"
        );
        // [#576 SDB-ACCT-D1] What the reopen records instead is the ADOPTED lock's
        // claim: the release, never the shortcut.
        assert!(
            state.speleodb_lock_resumed_here(REMOTE_ID),
            "the reopen adopts the account's lock for the release only"
        );
        let again = state.speleodb_link_status().expect("second poll");
        assert_eq!(
            again.remote_state,
            RemoteState::Conflict,
            "and the adopted claim licenses no shortcut on a later poll either"
        );
    }

    /// The production region of this file, split out of the test module so the
    /// source-level tripwires below cannot match their own assertions.
    fn prod_source() -> &'static str {
        crate::models::source_scan::prod_region(include_str!("speleodb.rs"), "impl AppState {")
    }

    #[test]
    fn the_catch_all_clear_defers_to_a_gate_that_reopened_underneath_it() {
        let (_temp, state) = remote_fixture_with_dirty_bit(true);
        // A claim is stamped with the account that took it, so recording one at
        // all needs a session to stamp it from (`LockClaim`).
        *state.speleodb.lock() = authed_session_at("http://x/");

        // This process took the lock and is editing under it - the state a
        // fast-forward lands back in once its in-place reload finishes.
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));
        state.speleodb_forget_lock_acquired_if_gate_shut();
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "a poll that observed the reload's momentarily-shut gate must find the \
             gate open again when it reaches the clear, and leave our own record alone"
        );

        // The other direction: the gate really is shut, so the catch-all still has to
        // fire - it is what covers close / project switch / logout / take-remote / the
        // 401 auto-drop / the lost-lock re-verify, none of which run a release verb in
        // this file.
        state.set_edit_enabled(false);
        state.speleodb_forget_lock_acquired_if_gate_shut();
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "a shut gate means the acquisition is over, whoever ended it"
        );
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

    #[test]
    fn a_failed_take_remote_drops_both_gates_from_a_force_edit_session() {
        // The one place a gate drop is NOT exempt for a force-editor, and why:
        // take-remote is the user abandoning this local session for the server tip, so
        // the session ending is the thing they asked for - not, as with a lock
        // release, a side effect of surrendering a claim they never held. When the
        // reset then fails part-way, dropping BOTH gates is the only outcome that
        // honours the request AND keeps `edit_enabled == false && force_editing ==
        // true` unobservable: `speleodb_link_status` reports the flag on every poll,
        // so a stranded one paints the big-red force-edit banner over a project that
        // is now read-only.
        //
        // The stub answers every call with an empty revision history: enough for the
        // head fetch (twice - a recorded BASE meeting an empty tip re-fetches once)
        // and for take-remote's multi-head check to pass, after which the fast-forward
        // receives that same JSON where it expects a ZIP and fails at `Dump::open`.
        // The force-editor holds no lock, so the abandoning release in between is a
        // no-op and makes no request of its own.
        let (base, _listener) = always_answering_stub(r#"{"commits":[]}"#);
        let (_temp, state) = editing_fixture(&base);
        state.set_force_editing(true);

        state
            .speleodb_take_remote()
            .expect_err("a JSON body cannot be opened as a dump");

        assert!(
            !state.edit_enabled(),
            "a failed take-remote still drops the edit gate"
        );
        assert!(
            !state.force_editing(),
            "and clears force-edit with it, so the two gates never disagree"
        );
    }

    /// [`always_answering_stub`] that also RECORDS each request's first line
    /// (`METHOD /path HTTP/1.1`), so a test can assert WHICH calls a flow made -
    /// and, more to the point, which it did not. [`counting_stub`] proves an absence
    /// too, but it answers nothing, so a multi-call flow cannot run against it at
    /// all; this one lets the whole verb complete and then reads the wire.
    ///
    /// The listener is returned for the same reason as the two stubs below: the
    /// caller must keep the port bound for the whole test.
    pub(in crate::core::state) fn recording_stub(
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
                if let Ok(mut lines) = log.lock() {
                    lines.push(text.lines().next().unwrap_or_default().to_owned());
                }
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

    /// [#576 A8] A loopback stub that answers each request by its first line
    /// (`METHOD /path HTTP/1.1`): `route` returns the status and the JSON body. Records
    /// every request line, like [`recording_stub`], for a flow whose calls must each
    /// get their own answer - a probe that succeeds, an acquire that is refused, a
    /// history read that 401s. Keep the listener bound for the whole test.
    pub(in crate::core::state) fn routing_stub(
        route: fn(&str) -> (u16, &'static str),
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
                let line = String::from_utf8_lossy(&req)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let (status, body) = route(&line);
                if let Ok(mut lines) = log.lock() {
                    lines.push(line);
                }
                let resp = format!(
                    "HTTP/1.1 {status} STUB\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), listener, seen)
    }

    /// [#576 A8] Every event `f` logs on this thread, one `LEVEL target: message
    /// fields` line each, through a scoped `tracing` subscriber - so a test can pin
    /// that a failure is LOGGED (a log line is not observable from a return value)
    /// instead of reading the source for a `warn!`.
    pub(in crate::core::state) fn logged_events(f: impl FnOnce()) -> Vec<String> {
        #[derive(Clone)]
        struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Captured {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .map_err(|_| std::io::Error::other("the capture buffer is poisoned"))?
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = Captured(std::sync::Arc::clone(&buffer));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buffer
            .lock()
            .expect("the capture buffer is not poisoned")
            .clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::to_owned)
            .collect()
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
                let is_probe = line.starts_with("GET ")
                    && line
                        .split_whitespace()
                        .nth(1)
                        .is_some_and(|path| path.ends_with(&probe_path));
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
    fn an_unreadable_probe_over_unpushed_edits_keeps_the_lock() {
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
        *state.speleodb.lock() = authed_session_at(&base);

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
    fn an_unreadable_probe_over_a_clean_copy_still_releases_the_lock() {
        // The other half of the unknown-probe rule, and the reason it is not a blanket
        // "keep on unknown": with NOTHING un-pushed there is no work to strand, and a
        // lock this call may well have just taken would be left held over an edit
        // session that never started - recoverable only through the next login's
        // held-lock prompt or the server's own timeout.
        //
        // Same failing probe as the test above, same divergence routing; only the dirty
        // bit differs, which is exactly the input the rule keys on.
        let (base, _listener, seen) = stub_failing_the_lock_probe(CLEAN_DIVERGED_BODY);
        let (_temp, state) = remote_fixture_with_dirty_bit(false);
        *state.speleodb.lock() = authed_session_at(&base);

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
    fn a_freshly_taken_lock_is_still_released_when_the_fast_forward_fails() {
        // DO-NOT-REGRESS (passes before the local-edits keep rule as well as after):
        // the fast-forward arm's OTHER unwind, and the line the keep rule must not
        // cross. A lock this call demonstrably took, over a copy with nothing local at
        // stake, is still handed back when the fast-forward blows up - leaving it held
        // would strand the project for every other editor over an edit session that
        // never started.
        //
        // Also the reach test for that arm: `FREE_MUTEX_DIVERGED_BODY`'s history over
        // a CLEAN fixture fences to FastForward rather than Diverged (dirty is the only
        // difference), and the stub answers the dump download with JSON, so the
        // fast-forward fails inside `Dump::open` - a corrupt-dump failure, not the
        // local-edits refusal. So the arm really does run to its unwind here, and the
        // release below is that unwind's doing.
        let (base, _listener, seen) = recording_stub(FREE_MUTEX_DIVERGED_BODY);
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&base);

        state
            .speleodb_start_editing()
            .expect_err("a JSON body cannot be opened as a dump");

        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/download/dump/"))),
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
        // and a corrupt dump must not.
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

        let corrupt = crate::speleodb::dump::Dump::open(b"{\"not\":\"a zip\"}")
            .err()
            .expect("a JSON body is not a ZIP");
        assert!(
            !is_local_edits_refusal(&corrupt),
            "a corrupt dump has nothing local at stake, so a lock we took is still \
             handed back, got {corrupt:?}"
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
            let (base, _listener, hits) = counting_stub();
            let (_temp, state) = editing_fixture(&base);
            assert!(
                !state.speleodb_open_link().expect("link").has_unpushed_edits,
                "fixture starts clean"
            );

            let report = state.speleodb_logout();

            // The release goes out first. [#576 Q-W61-3-D1] The stub never answers
            // it, so the logout re-reads the holder (a reply that may have been lost,
            // #576 FZ-SDB-6) - those are the hits after it - and, unable to say the
            // lock is gone, names it in its answer.
            let hits = hits.load(std::sync::atomic::Ordering::SeqCst);
            assert!(
                hits >= 1,
                "a clean held lock is still released on logout: {hits}"
            );
            assert!(
                report.is_ok_and(|report| report.locks_left_held.len() == 1),
                "and a release nobody answered is reported as still held"
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
        let body = format!(r#"{{"id":"{project_id}","name":"Remote"}}"#);
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let accept_on = listener.try_clone().expect("clone listener");
        let counter = std::sync::Arc::clone(&hits);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = accept_on.accept() {
                counter.fetch_add(1, Ordering::SeqCst);
                // Drain the request until the socket goes quiet, then answer.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    /// model the world changing between the status poll's own holder read and
    /// the re-read that actually opens the gate. Requests for the revision
    /// topology get an empty body (the fold degrades to `Unknown`, which is not
    /// what these tests are about).
    fn holder_changes_after_the_first_read(
        project_id: &str,
        later_holder: Option<&str>,
    ) -> (String, std::net::TcpListener) {
        holder_changes_between_reads(project_id, Some("u@x"), later_holder)
    }

    /// [`holder_changes_after_the_first_read`] with the FIRST holder chosen too, for
    /// the opposite race: the poll's own read names somebody else, and the re-read
    /// that would close the gate finds the lock back with us.
    fn holder_changes_between_reads(
        project_id: &str,
        first_holder: Option<&str>,
        later_holder: Option<&str>,
    ) -> (String, std::net::TcpListener) {
        use std::io::{Read as _, Write as _};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let accept_on = listener.try_clone().expect("clone listener");
        let body_for = |holder: Option<&str>| match holder {
            Some(user) => format!(
                r#"{{"id":"{project_id}","name":"Remote","active_mutex":{{"user":"{user}"}}}}"#
            ),
            None => format!(r#"{{"id":"{project_id}","name":"Remote"}}"#),
        };
        let first = body_for(first_holder);
        let later = body_for(later_holder);
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
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), listener)
    }

    #[test]
    fn the_poll_resumes_editing_when_the_lock_is_still_ours_as_it_opens_the_gate() {
        // The invariant the reopen exists for: a project reopened while we still
        // hold its mutex must not sit read-only under our own lock.
        let (base, _listener) = holder_changes_after_the_first_read(REMOTE_ID, Some("u@x"));
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&base);
        assert!(!state.edit_enabled(), "fixture opens read-only");

        let status = state.speleodb_link_status().expect("status");

        assert!(status.edit_enabled, "our own lock resumes the edit session");
        assert!(state.edit_enabled(), "and the gate really is open");
    }

    #[test]
    fn the_poll_leaves_the_gate_shut_when_the_mutex_went_away_mid_poll() {
        // Round 6 (P1): the poll's holder read is one of several requests, and a
        // captured or janitor release for this same project can complete before
        // the gate opens. That release's reclaim check finds no claim record (the
        // poll never minted one) and no live edit session (the gate is still shut
        // - which is why the poll is here at all), so it rightly frees the mutex.
        // Opening the gate from the earlier read then left a session editing a
        // remote project with no server lock behind it and nothing recording one.
        //
        // The stub is that release: the mutex is ours on the read that decides to
        // ask, and gone by the re-read that would open the gate.
        let (base, _listener) = holder_changes_after_the_first_read(REMOTE_ID, None);
        let (_temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&base);

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

    // --- #576 B35(e): Done and the poll ask who holds the lock -----------------------

    /// A mutex at our BASE with nobody on it: the lock this session was editing
    /// under is gone (admin-released, or expired on the server).
    pub(in crate::core::state) const FREE_MUTEX_AT_BASE_BODY: &str = r#"{
        "id":"11111111-1111-1111-1111-111111111111","name":"Remote",
        "commits":[{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;

    #[test]
    fn done_never_releases_a_lock_another_account_now_holds() {
        // [#576 B35(e)-D1] Our lock was freed elsewhere and a colleague took it. A
        // clean Done used to go straight to `release_lock`; for a project ADMIN that
        // release frees the colleague's mutex (speleodb_admin.rs), so their next push
        // fails or a third party takes the lock. Done must ask who holds it first,
        // release only its own, and otherwise switch editing off and say who does.
        let (base, _listener, seen) = recording_stub(HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);

        let result = state.speleodb_done(None);

        let seen = seen.lock().expect("request log").clone();
        let releases = seen.iter().filter(|r| r.contains("/release/")).count();
        assert_eq!(
            releases, 0,
            "Done sent {releases} release request(s) for a lock someone.else@x holds; \
             saw {seen:?}"
        );
        let err = result.expect_err("a lock that is not ours cannot be handed back");
        assert_eq!(err.action, crate::speleodb::SpeleoDbAction::Fence);
        assert!(
            err.message.contains("someone.else@x"),
            "the error names who holds the lock now: {}",
            err.message
        );
        assert!(
            !state.edit_enabled(),
            "editing is switched off - there is no lock behind it any more"
        );
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "and the claim on a lock we no longer hold is dropped"
        );
    }

    #[test]
    fn done_never_uploads_or_releases_when_the_lock_is_gone() {
        // [#576 B35(e)-D1] The same over un-pushed work, with the mutex free: the
        // upload needs the lock (the server 403s it) and the release would free
        // nothing of ours, so Done does neither, keeps the edits marked un-pushed and
        // drops the gate so Start editing can take the lock properly.
        let (base, _listener, seen) = recording_stub(FREE_MUTEX_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);
        state.mark_remote_dirty();

        let result = state.speleodb_done(None);

        let seen = seen.lock().expect("request log").clone();
        assert!(
            !seen
                .iter()
                .any(|r| r.contains("/upload/") || r.contains("/release/")),
            "no upload or release without the lock; saw {seen:?}"
        );
        let err = result.expect_err("there is no lock to finish under");
        assert_eq!(err.action, crate::speleodb::SpeleoDbAction::Fence);
        assert!(
            !state.edit_enabled(),
            "editing is switched off over a lock that is gone"
        );
        assert!(
            state.speleodb_open_link().expect("link").has_unpushed_edits,
            "the un-pushed edits stay marked for the next Start editing"
        );
    }

    #[test]
    fn done_does_not_release_when_the_holder_cannot_be_read() {
        // [#576 B35(e)-D1] "Could not say who holds it" is not "ours": nothing is
        // released and nothing changes, so a retry asks again.
        let (base, _listener, seen) =
            stub_failing_the_lock_probe(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);

        let result = state.speleodb_done(None);

        let seen = seen.lock().expect("request log").clone();
        assert!(
            !seen.iter().any(|r| r.contains("/release/")),
            "an unreadable holder must not license a release; saw {seen:?}"
        );
        assert!(
            result.is_err(),
            "Done reports the failed read, got {result:?}"
        );
        assert!(
            state.edit_enabled(),
            "nothing is decided on a read that failed - the session stays as it was"
        );
    }

    #[test]
    fn done_still_releases_the_lock_we_hold() {
        // [#576 B35(e)] DO-NOT-REGRESS: our own lock is still handed back.
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);

        let result = state.speleodb_done(None).expect("done");

        assert!(matches!(result, DoneResult::Released), "got {result:?}");
        let seen = seen.lock().expect("request log").clone();
        assert!(
            seen.iter()
                .any(|r| r.contains(&format!("/projects/{REMOTE_ID}/release/"))),
            "our own lock is released; saw {seen:?}"
        );
        assert!(!state.edit_enabled(), "read-only after Done");
    }

    #[test]
    fn the_poll_switches_editing_off_when_another_account_holds_the_lock() {
        // [#576 B35(e)-D2] The poll used to only ever OPEN the gate. A session whose
        // lock was freed and taken by a colleague kept editing for up to 20 minutes
        // (until the lazy re-verify) over a lock it no longer held, and none of those
        // edits could be uploaded.
        let (base, _listener) = always_answering_stub(HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);
        state.mark_remote_dirty();

        let status = state.speleodb_link_status().expect("poll");

        assert!(
            !status.edit_enabled,
            "poll reported edit_enabled={} while someone.else@x holds the lock",
            status.edit_enabled
        );
        assert!(!state.edit_enabled(), "and the gate really is shut");
        assert_eq!(status.lock_holder.as_deref(), Some("someone.else@x"));
        assert!(
            status.has_unpushed_edits,
            "the edits stay on this computer, marked un-pushed"
        );
        assert!(
            !state.speleodb_lock_acquired_here(REMOTE_ID),
            "the claim on the lost lock is dropped"
        );
    }

    #[test]
    fn the_poll_switches_editing_off_when_the_lock_is_gone() {
        // [#576 B35(e)-D2] Nobody holds it: still not ours to edit under.
        let (base, _listener) = always_answering_stub(FREE_MUTEX_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);

        let status = state.speleodb_link_status().expect("poll");

        assert!(
            !status.edit_enabled && !state.edit_enabled(),
            "a session whose lock is gone must not keep editing"
        );
        assert_eq!(status.lock_holder, None);
    }

    #[test]
    fn the_poll_keeps_editing_when_the_lock_holder_cannot_be_read() {
        // [#576 B35(e)-D2] Only a SUCCESSFUL read closes the gate: offline editing
        // stays first-class.
        let (base, _listener, _hits) = counting_stub();
        let (_temp, state) = editing_fixture(&base);

        let status = state.speleodb_link_status().expect("poll");

        assert!(
            status.edit_enabled && state.edit_enabled(),
            "an unreachable server says nothing about the lock"
        );
        assert!(state.speleodb_lock_acquired_here(REMOTE_ID));
    }

    #[test]
    fn the_poll_leaves_a_force_edit_session_alone() {
        // [#576 B35(e)-D2] A #213 force-editor edits WITHOUT the lock on purpose;
        // another holder is the reason it exists, not a reason to end it.
        let (base, _listener) = always_answering_stub(HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);
        state.set_force_editing(true);

        let status = state.speleodb_link_status().expect("poll");

        assert!(status.edit_enabled && state.edit_enabled());
        assert!(status.force_editing && state.force_editing());
    }

    #[test]
    fn the_poll_rereads_the_holder_before_switching_editing_off() {
        // [#576 B35(e)-D2] The close is decided like the inherited-lock reopen: the
        // holder is re-read inside the lock-ops span, so a stale first read (the
        // lock was ours again by the time the gate would close) cannot end a
        // session that still holds it.
        let (base, _listener) =
            holder_changes_between_reads(REMOTE_ID, Some("someone.else@x"), Some("u@x"));
        let (_temp, state) = editing_fixture(&base);

        let status = state.speleodb_link_status().expect("poll");

        assert!(
            status.edit_enabled && state.edit_enabled(),
            "the re-read found the lock ours, so editing continues"
        );
    }

    // --- #576 A18(a): a failed undo/redo still marks the SpeleoDB copy dirty ----------

    /// An editing remote copy with a CLEAN link, a stack built in code, and a
    /// section the stack's real entries can undo. `poison_below` puts an entry the
    /// undo cannot apply at the BOTTOM of the undo stack, under the real one.
    fn clean_editing_copy_with_history() -> (TempDir, AppState) {
        let (temp, state) = remote_readonly_fixture();
        state.set_edit_enabled(true);
        // An entry no undo can apply (`UndoData::None`), at the bottom of the stack.
        state
            .undo_stack
            .lock()
            .push_with_data(Command::CloseProject, crate::core::undo::UndoData::None);
        for name in ["A", "B"] {
            assert!(
                state
                    .execute(Command::CreateSection {
                        name: Some(name.to_owned()),
                        surveyor: None,
                        date: None,
                        color: "#abcdef".to_owned(),
                        direction: None,
                        shots: vec![ShotData {
                            length: 5.0,
                            ..Default::default()
                        }],
                    })
                    .success,
                "section {name} is created on the editing copy"
            );
        }
        // As a confirmed push leaves it: everything above is on the server.
        state
            .speleodb_mutate_link(|l| {
                l.has_unpushed_edits = false;
                l.last_edit_at = None;
            })
            .expect("clean the link");
        (temp, state)
    }

    fn remote_dirty(state: &AppState) -> bool {
        state.speleodb_open_link().expect("link").has_unpushed_edits
    }

    #[test]
    fn a_partial_undo_to_marks_the_remote_copy_dirty() {
        // [#576 A18(a)] undo_to(2): steps 1-2 undo B and A, step 3 fails on the entry
        // beneath them. The two undone sections are real changes, but the failure arm
        // returned before the only dirty flip, so Done skipped the upload of a "clean"
        // link and released, and a later re-pull replaced the copy without asking.
        let (_temp, state) = clean_editing_copy_with_history();

        let result = state.undo_to(2);

        assert!(!result.success, "step 3 cannot be undone");
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("Undo failed at step 3 of 3")),
            "got {:?}",
            result.error
        );
        assert_eq!(
            state.get_project_state().expect("project").sections.len(),
            0,
            "steps 1-2 really undid both sections"
        );
        assert!(
            remote_dirty(&state),
            "has_unpushed_edits={} after a partial undo_to that removed two sections",
            remote_dirty(&state)
        );
    }

    #[test]
    fn a_failed_single_undo_past_the_empty_check_marks_the_remote_copy_dirty() {
        // [#576 NF-AN3-1] The single-step arm had the same hole: a failing step can have
        // mutated before it reported, and nothing marked the copy.
        let (_temp, state) = clean_editing_copy_with_history();
        assert!(state.undo().success && state.undo().success);
        state
            .speleodb_mutate_link(|l| l.has_unpushed_edits = false)
            .expect("clean the link");

        assert!(!state.undo().success, "the bottom entry cannot be undone");

        assert!(
            remote_dirty(&state),
            "a failed undo that got past the empty-stack check marks the copy"
        );
    }

    #[test]
    fn an_undo_with_nothing_to_undo_leaves_the_remote_copy_clean() {
        // [#576 A18(a)] DO-NOT-REGRESS: the index check is the line - an empty stack
        // changed nothing and must not cost a no-op push or veto a lock release.
        let (_temp, state) = remote_readonly_fixture();
        state.set_edit_enabled(true);

        assert!(!state.undo().success);
        assert!(!state.redo().success);
        assert!(!state.undo_to(0).success);
        assert!(!state.redo_to(0).success);
        assert!(!remote_dirty(&state));
    }

    /// [`clean_editing_copy_with_history`] with its two sections undone and a redo
    /// entry that cannot replay (it deletes a section that does not exist) slipped
    /// UNDER them on the redo stack.
    fn clean_copy_with_a_failing_redo_beneath() -> (TempDir, AppState) {
        let (temp, state) = clean_editing_copy_with_history();
        assert!(state.undo().success && state.undo().success);
        {
            let mut stack = state.undo_stack.lock();
            let a = stack.pop_redo().expect("A on top of redo");
            let b = stack.pop_redo().expect("B under it");
            stack.push_redo(crate::core::undo::UndoEntry {
                replayed_at: None,
                command_bytes: None,
                command: Command::DeleteSection { section_id: 999 },
                undo_data: crate::core::undo::UndoData::InverseCommand(Command::CloseProject),
                timestamp: None,
            });
            stack.push_redo(b);
            stack.push_redo(a);
        }
        state
            .speleodb_mutate_link(|l| l.has_unpushed_edits = false)
            .expect("clean the link");
        (temp, state)
    }

    #[test]
    fn a_partial_redo_to_marks_the_remote_copy_dirty() {
        // [#576 A18(a)] The redo half: steps 1-2 re-create A and B, step 3 fails.
        let (_temp, state) = clean_copy_with_a_failing_redo_beneath();

        let result = state.redo_to(2);

        assert!(!result.success);
        assert_eq!(
            state.get_project_state().expect("project").sections.len(),
            2,
            "steps 1-2 really re-created both sections"
        );
        assert!(
            remote_dirty(&state),
            "has_unpushed_edits={} after a partial redo_to that re-created two sections",
            remote_dirty(&state)
        );
    }

    #[test]
    fn a_failed_single_redo_past_the_empty_check_marks_the_remote_copy_dirty() {
        // [#576 NF-AN3-1] The single-step redo arm.
        let (_temp, state) = clean_copy_with_a_failing_redo_beneath();
        assert!(state.redo().success && state.redo().success);
        state
            .speleodb_mutate_link(|l| l.has_unpushed_edits = false)
            .expect("clean the link");

        assert!(!state.redo().success, "the bottom redo cannot replay");

        assert!(remote_dirty(&state));
    }

    /// The `.stalx` of a project we hold the SpeleoDB write lock on can be DELETED, and
    /// that has to hand the lock back too.
    ///
    /// the `close_project` command releases before it issues `Command::CloseProject`, but
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
                state.edit_enabled() && !state.force_editing(),
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

            // (2) No lock held (remote, opened read-only): nothing to release, so
            // nothing is sent and nothing is warned about.
            let (base, _listener2, hits) = counting_stub();
            let (_temp, state) = editing_fixture(&base);
            state.set_edit_enabled(false);
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
    /// deleted project's mutex until the server timeout.
    #[test]
    fn the_delete_sequence_clears_its_own_claim_then_releases() {
        let (base, _listener, hits) = answering_counting_stub(REMOTE_ID);
        let (_temp, state) = editing_fixture(&base);
        // The real sequence: `delete_project` snapshots AND clears its own claim
        // under the `project` mutex, then releases once that mutex (and the
        // database handle) are out of the way.
        let params = state
            .speleodb_delete_lock_release_params()
            .expect("precondition: the fixture holds a lock, so there is a snapshot");
        assert!(
            state.speleodb_lock_acquired_here(REMOTE_ID),
            "precondition: the fixture recorded the acquisition it edits under"
        );
        // Clear + teardown under ONE `project` guard, exactly as `delete_project`
        // does. The teardown matters to the release below since round 5: a still
        // OPEN, still EDITING session on the same project is itself grounds to
        // stand down (`speleodb_live_session_accounts_for`), so a fixture that
        // skipped the close would model a world the real sequence never produces.
        let cleared = {
            let commands = state.hold_commands();
            let mut open = state.project.lock();
            let cleared = state.speleodb_forget_lock_acquired_under(&open, &params.project_id);
            state.close_open_project_locked(&commands, &mut open);
            cleared
        };

        state
            .speleodb_release_captured_lock(&cleared, &params)
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
    /// then, so no status poll will ever look at that mutex again, and this
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
        // Root ignores the permission bits, and then there is no denied unlink
        // to test with: fail rather than pass on nothing (#576 NF-ROOT-PERM).
        crate::models::test_support::assert_dir_refuses_new_files(&dir);
        let result = state.delete_project(path.clone());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore the directory");

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
            .speleodb_delete_lock_release_params()
            .expect("precondition: the fixture holds a lock, so there is a snapshot");
        let cleared = {
            let open = state.project.lock();
            state.speleodb_forget_lock_acquired_under(&open, &params.project_id)
        };
        // A successor Start Editing on the same remote project commits its claim
        // inside the gap between the snapshot and the deferred POST.
        state.speleodb_note_lock_acquired(Some(REMOTE_ID));

        state
            .speleodb_release_captured_lock(&cleared, &params)
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
    /// `speleodb_remote_state`: without `we_hold_the_lock`, `poll_verdict` refuses to
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
            .speleodb_delete_lock_release_params()
            .expect("precondition: the fixture holds a lock, so there is a snapshot");
        assert_eq!(
            params.project_id, REMOTE_ID,
            "precondition: the snapshot names the project being deleted"
        );
        // The delete's own clear + teardown, as in the positive test above: the
        // deleted project's session must be GONE before the deferred POST, or
        // the round-5 live-session eye would (correctly) stand down for it.
        let cleared = {
            let commands = state.hold_commands();
            let mut open = state.project.lock();
            let cleared = state.speleodb_forget_lock_acquired_under(&open, &params.project_id);
            state.close_open_project_locked(&commands, &mut open);
            cleared
        };
        // Exactly what a successful Start editing on the NEXT project does, in the
        // window `delete_project` opens by releasing the `project` mutex first.
        state.speleodb_note_lock_acquired(Some(GAP_ID));

        state
            .speleodb_release_captured_lock(&cleared, &params)
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
    fn force_editing_blocks_done_and_push() {
        // Done / push need the lock a force-editor doesn't hold; both are refused with
        // the reconnect hint (the FE offers Reconnect instead).
        let (_temp, state) = remote_readonly_fixture();
        state.set_edit_enabled(true);
        state.set_force_editing(true);
        assert!(
            state.speleodb_done(None).is_err(),
            "Done is blocked in force-edit"
        );
        assert!(
            state.speleodb_push_only(None).is_err(),
            "push is blocked in force-edit"
        );
    }

    #[test]
    fn logout_drops_both_gates_when_force_editing() {
        // Logging out of a force-edit session drops BOTH gates (no lock to release).
        //
        // Isolated because logout writes the config (clearing the rehydrate seed) and
        // now reports a failure to do so, so it needs a config file of its own.
        crate::test_helpers::with_isolated_config(|| {
            let (_temp, state) = remote_readonly_fixture();
            state.set_edit_enabled(true);
            state.set_force_editing(true);
            state.speleodb_logout().expect("logout");
            assert!(!state.edit_enabled(), "logout drops the edit gate");
            assert!(!state.force_editing(), "logout clears force-edit");
        });
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

    /// A session seeded with an in-memory token (no real keychain). Marked
    /// already-rehydrated so it behaves like a live, established session.
    fn authed_session() -> SpeleoDbSession {
        SpeleoDbSession {
            endpoint: Some("https://x/".to_owned()),
            email: Some("u@x".to_owned()),
            token_store: TokenStore::from_in_memory_token("tok-1"),
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn session_status_reflects_auth_and_gates_keychain_flag() {
        // Authenticated: email surfaced; keychain flag reflects the store (an
        // in-memory token is not keychain-backed).
        let status = authed_session().status();
        assert!(status.logged_in);
        assert_eq!(status.user_email.as_deref(), Some("u@x"));
        assert!(!status.keychain_backed);

        // Not authenticated: no email leaks and keychain_backed is forced false
        // (it is gated on logged_in, so it can't claim a keychain token while
        // reporting logged-out).
        let status = SpeleoDbSession::default().status();
        assert!(!status.logged_in);
        assert_eq!(status.user_email, None);
        assert!(!status.keychain_backed);
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

    /// A [`SecretStore`] that stores/loads fine but whose DELETE always fails -
    /// the flaky-keychain seam for the #270 logout paths.
    ///
    /// `store`/`load` are REAL (an inner `InMemoryStore`), not stubs: `TokenStore::clear`
    /// now decides whether a failed keychain delete is worth reporting by PROBING the
    /// keychain for an entry, so a fake that accepted `store` and then answered `load`
    /// with `Ok(None)` claimed to hold nothing while modelling a store that had just
    /// been written to - and the delete failure it exists to raise was correctly (given
    /// that claim) suppressed.
    struct FailingDeleteStore(InMemoryStore);

    impl FailingDeleteStore {
        fn new() -> Self {
            Self(InMemoryStore::new())
        }
    }

    impl SecretStore for FailingDeleteStore {
        fn kind(&self) -> &'static str {
            "failing-delete"
        }
        fn store(
            &self,
            account_key: &str,
            token: &str,
        ) -> Result<(), crate::speleodb::auth::SecretStoreError> {
            self.0.store(account_key, token)
        }
        fn load(
            &self,
            account_key: &str,
        ) -> Result<Option<String>, crate::speleodb::auth::SecretStoreError> {
            self.0.load(account_key)
        }
        fn delete(&self, _: &str) -> Result<(), crate::speleodb::auth::SecretStoreError> {
            Err(crate::speleodb::auth::SecretStoreError::for_test(
                "keychain unavailable",
            ))
        }
    }

    /// An authed session whose token was written through `keychain`, so a later
    /// clear really exercises that backend's `delete`.
    fn authed_session_with(keychain: Box<dyn SecretStore>) -> SpeleoDbSession {
        let mut token_store = TokenStore::from_stores(keychain, None);
        token_store.store("https://x/", "u@x", "tok-1", false);
        SpeleoDbSession {
            endpoint: Some("https://x/".to_owned()),
            email: Some("u@x".to_owned()),
            token_store,
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn forget_reporting_surfaces_a_failed_delete_but_still_drops_the_session() {
        // #270: logout's fallible form REPORTS a keychain delete failure - and the
        // session is gone regardless (the user asked to log out; the Err is a warning
        // that a credential may survive on this machine).
        let mut session = authed_session_with(Box::new(FailingDeleteStore::new()));
        assert!(session.status().logged_in);
        assert!(
            session.forget_reporting().is_err(),
            "a failed credential delete must be reported to the logout caller"
        );
        assert!(!session.status().logged_in);
    }

    #[test]
    fn forget_stays_best_effort_for_the_401_auto_drop() {
        // The 401 auto-drop sink must NOT fail closed: a dead token leaves the
        // session even when the keychain refuses the delete. `forget` returns `()`,
        // so no error can reach `speleodb_handle_auth`'s callers.
        let mut session = authed_session_with(Box::new(FailingDeleteStore::new()));
        session.forget();
        assert!(!session.status().logged_in);
        assert!(session.active_token().is_err());
    }

    #[test]
    fn auto_drop_path_does_not_propagate_a_keychain_failure() {
        // End-to-end through the AppState sink: `speleodb_handle_auth` routes the
        // 401 through `speleodb_forget`, which returns nothing - the original
        // result is handed back untouched, never replaced by a keychain error.
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            *state.speleodb.lock() = authed_session_with(Box::new(FailingDeleteStore::new()));

            let result: Result<(), SpeleoDbError> = Err(SpeleoDbError::Status {
                status: 401,
                message: "unauthorized".to_owned(),
            });
            let out = state.speleodb_handle_auth(Endpoint::ListProjects, result);
            assert!(
                matches!(out, Err(SpeleoDbError::Status { status: 401, .. })),
                "the caller still sees its own 401, not a keychain error"
            );
            assert!(
                !state.speleodb.lock().status().logged_in,
                "the dead token is dropped even though the keychain delete failed"
            );
        });
    }

    #[test]
    fn logout_clears_the_persisted_rehydrate_email() {
        // #270: the config email is the rehydrate SEED - leaving it behind lets a
        // surviving keychain entry silently re-authenticate on the next launch.
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            state
                .speleodb_write_account_record(Some("u@x".to_owned()), None)
                .expect("seed the persisted account");
            *state.speleodb.lock() = authed_session_with(Box::new(InMemoryStore::new()));

            state.speleodb_logout().expect("a clean logout succeeds");
            assert_eq!(
                state.config.lock().speleodb.email,
                None,
                "logout must clear the persisted account email"
            );
            // The endpoint is a Settings preference and is deliberately untouched.
        });
    }

    #[test]
    fn logout_reports_a_failed_delete_but_still_logs_out() {
        // Even when the credential delete fails, logout drops the session AND the
        // rehydrate seed first, THEN reports - a keychain blip must never leave the
        // user logged in.
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            state
                .speleodb_write_account_record(Some("u@x".to_owned()), None)
                .expect("seed the persisted account");
            *state.speleodb.lock() = authed_session_with(Box::new(FailingDeleteStore::new()));

            let err = state
                .speleodb_logout()
                .expect_err("a failed credential delete is surfaced");
            assert!(
                err.message.contains("Logged out"),
                "the message must read as a warning about a leftover credential: {}",
                err.message
            );
            assert_eq!(
                state.config.lock().speleodb.email,
                None,
                "the rehydrate seed is cleared even on the error path"
            );
            assert!(
                !state.speleodb.lock().status().logged_in,
                "the session is dropped even on the error path"
            );
        });
    }

    #[test]
    fn logout_clears_the_rehydrate_seed_through_a_poisoned_config_lock() {
        // `speleodb_write_config_email` used to bail on a poisoned config mutex
        // (`let Ok(..) = self.config.lock() else { return; }`) - silently, and while
        // logout went on to report success. The email is the REHYDRATE SEED
        // (`speleodb_ensure_rehydrated` keys the keychain lookup by it), so skipping the
        // clear left the session re-authenticating itself on the next launch after the
        // user had been told the logout worked.
        //
        // A poisoned lock means another thread panicked mid-mutation; that is a reason
        // to be careful with what is in there, not a reason to leave a live rehydrate
        // seed behind - so it is recovered with `into_inner`, exactly as the save
        // pipeline does with the project lock.
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            state
                .speleodb_write_account_record(Some("u@x".to_owned()), None)
                .expect("seed the persisted account");
            *state.speleodb.lock() = authed_session_with(Box::new(InMemoryStore::new()));

            let cfg = &state.config;
            let _ = std::thread::scope(|s| {
                s.spawn(|| {
                    let _guard = cfg.lock();
                    panic!("poison the config mutex on purpose");
                })
                .join()
            });
            assert!(
                state.config.is_poisoned(),
                "precondition: the config mutex is poisoned"
            );

            state
                .speleodb_logout()
                .expect("logout still succeeds through a poisoned config lock");
            assert_eq!(
                state.config.lock().speleodb.email,
                None,
                "the rehydrate seed must be cleared, not skipped, on a poisoned lock"
            );
        });
    }

    #[test]
    fn logout_reports_a_seed_that_could_not_be_cleared() {
        // The other half of #270's residue: a credential that outlived a failed delete
        // is only dangerous because the SEED finds it again, so a seed that could not be
        // cleared is its own warning - and a run that fails both must report both rather
        // than hiding one behind the first.
        //
        // Failure is injected by squatting the config FILE's path with a DIRECTORY, so
        // the write cannot land no matter how the save is spelled.
        crate::test_helpers::with_isolated_config(|| {
            let config_dir =
                crate::test_helpers::current_config_dir().expect("this thread is isolated");
            std::fs::create_dir_all(config_dir.join("config.toml"))
                .expect("squat the config file path with a directory");

            let state = AppState::new().expect("state");
            *state.speleodb.lock() = authed_session_with(Box::new(InMemoryStore::new()));

            let err = state
                .speleodb_logout()
                .expect_err("an unwritable config must be reported");
            assert!(
                err.message.contains("Logged out") && err.message.contains("sign in again"),
                "the warning must name the resurrection risk, not just fail: {}",
                err.message
            );
            assert!(
                !state.speleodb.lock().status().logged_in,
                "and the logout itself still happened"
            );
        });
    }

    /// A keyring that refuses EVERY op - the locked or absent secret service. The
    /// two are indistinguishable from here, which is the whole reason
    /// `config.speleodb.keychain_has_token` exists.
    struct LockedKeyringStore;

    impl SecretStore for LockedKeyringStore {
        fn kind(&self) -> &'static str {
            "locked"
        }
        fn store(&self, _: &str, _: &str) -> Result<(), crate::speleodb::auth::SecretStoreError> {
            Err(crate::speleodb::auth::SecretStoreError::for_test(
                "the keyring is locked",
            ))
        }
        fn load(&self, _: &str) -> Result<Option<String>, crate::speleodb::auth::SecretStoreError> {
            Err(crate::speleodb::auth::SecretStoreError::for_test(
                "the keyring is locked",
            ))
        }
        fn delete(&self, _: &str) -> Result<(), crate::speleodb::auth::SecretStoreError> {
            Err(crate::speleodb::auth::SecretStoreError::for_test(
                "the keyring is locked",
            ))
        }
    }

    /// Put a locked keyring under a state's COLD session, leaving
    /// `rehydrate_attempted` false so the next authed call still runs the startup
    /// rehydrate through it.
    fn lock_the_keyring(state: &AppState) {
        state.speleodb.lock().token_store =
            TokenStore::from_stores(Box::new(LockedKeyringStore), None);
    }

    #[test]
    fn a_login_the_keyring_refuses_does_not_erase_an_earlier_logins_entry() {
        // The persisted `keychain_has_token` answers "what is in the keyring", and it
        // used to be written as `tier == TokenTier::Keychain` - "where did THIS login's
        // key go". Those come apart on exactly one machine, and it is the interesting
        // one: a keyring that took a token on Monday and is merely LOCKED today. The
        // login falls back to memory WITHOUT removing Monday's entry, so `false` was
        // recorded over a live bearer token that is still in there.
        //
        // That flag is also the only thing separating a locked keyring from a box with
        // no secret service - both fail every op and both record `Unavailable` - so
        // losing it is not just a missed prompt. Turning the keyring off on the next
        // launch then classified the failed eviction AND its failed probe as "nothing
        // was in there", reported success, and sent every later launch past the entry.
        crate::test_helpers::with_isolated_config(|| {
            let (base, _listener) = always_answering_stub(r#"{"email":"u@x"}"#);
            let state = AppState::new().expect("state");
            {
                // What Monday's successful keychain login left on disk. The endpoint is
                // the Settings preference (login never writes it), and it has to be
                // here or the restart below has no account key to evict by.
                let mut config = state.config.lock();
                config.speleodb.endpoint = Some(base.clone());
                config.save().expect("seed the previous launch's endpoint");
            }
            // The account and the belief stamped with it, through the writer that
            // login uses.
            state
                .speleodb_write_account_record(
                    Some("u@x".to_owned()),
                    Some(account_key(&base, "u@x")),
                )
                .expect("seed the previous launch's record");
            lock_the_keyring(&state);
            // Today's startup: the rehydrate read fails, recording `Unavailable` - the
            // verdict a keychain-less box records too.
            state.speleodb_session_status().expect("status");

            let result = state
                .speleodb_login(Some(base), None, None, Some("tok-today".to_owned()))
                .expect("the paste login succeeds; only the keychain write fails");

            assert_eq!(
                result.storage,
                crate::speleodb::TokenStorage::Memory,
                "the locked keyring refused the write, so the key is session-only"
            );
            assert_eq!(result.keychain, KeychainReport::Unavailable);
            assert!(
                state.get_config().speleodb.keychain_has_token,
                "and Monday's entry is still in there - nothing removed it, so the record \
                 of it must not be overwritten with where TODAY's key happened to land"
            );

            // The next launch, reading the config this login just wrote.
            let restarted = AppState::new().expect("restarted state");
            lock_the_keyring(&restarted);
            let mut settings = restarted.get_config();
            settings.speleodb.keychain_disabled = true;
            let report = restarted
                .update_config(settings)
                .expect("the settings still save - the residue is reported, not refused");

            assert!(
                report.keychain_residue.is_some(),
                "turning the keyring off must still report the entry it could not remove"
            );
            assert!(
                restarted.get_config().speleodb.keychain_has_token,
                "and must not record a removal that did not happen"
            );
        });
    }

    /// A keyring that ANSWERS and holds nothing for anybody, under a state's COLD
    /// session with `rehydrate_attempted` still false. The opposite fixture to
    /// [`lock_the_keyring`]: this one can give the startup read the one answer that
    /// PROVES an entry is gone.
    fn empty_the_keyring(state: &AppState) {
        state.speleodb.lock().token_store =
            TokenStore::from_stores(Box::new(InMemoryStore::new()), None);
    }

    #[test]
    fn a_keyring_that_answered_empty_is_not_opened_again_after_a_restart() {
        // The self-correction the whole "a stale `true` costs ONE prompt" argument
        // rests on. An entry removed outside the app - seahorse, a reinstall - leaves
        // `config.speleodb.keychain_has_token = true` over a keyring that no longer
        // holds anything, and the startup read is what finds that out: `Ok(None)` is
        // the one proof of absence a read can give, and the live store lowers its
        // belief on it.
        //
        // The live store is not what the next launch reads. Lowering it there and
        // stopping meant the config kept saying `true`, so every launch re-opened the
        // keyring - which on Linux IS the desktop unlock prompt - to be told the same
        // thing again, forever. The correction has to reach disk to be a correction.
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            {
                // What a previous launch's successful keychain login left on disk,
                // and an account to key the startup read by.
                let mut config = state.config.lock();
                config.speleodb.endpoint = Some("https://sdb.example".to_owned());
                config.save().expect("seed the previous launch's endpoint");
            }
            state
                .speleodb_write_account_record(
                    Some("u@x".to_owned()),
                    Some(account_key("https://sdb.example", "u@x")),
                )
                .expect("seed the previous launch's record");
            empty_the_keyring(&state);

            // The startup rehydrate: no network, and the keyring answers empty.
            state.speleodb_session_status().expect("status");

            assert!(
                !state.speleodb.lock().token_store.keychain_has_token(),
                "premise: the read proved the keyring holds nothing for this account"
            );
            assert!(
                !state.get_config().speleodb.keychain_has_token,
                "and the correction has to reach the config, not just the live store"
            );

            // The next launch, reading the config off disk.
            let restarted = AppState::new().expect("restarted state");
            assert!(
                !restarted.get_config().speleodb.keychain_has_token,
                "or the same stale `true` opens the keyring again on every launch"
            );
        });
    }

    #[test]
    fn one_accounts_keyring_entry_is_not_reported_to_the_next_account() {
        // The account switch, on the machine the recorded belief exists for. The
        // config's `keychain_has_token` is written by the same login that writes
        // `speleodb.email`, so it is a fact ABOUT that account - and a keyring entry
        // is keyed by (endpoint, email), so the previous account's entry is not even
        // reachable under this one's key.
        //
        // Signing in as somebody else used to inherit it twice over: re-asserted from
        // the config, and carried in the live store the switch reuses. The new
        // login's keychain write fails against the locked keyring without lowering
        // it, so the NEW email and the OLD account's `true` were persisted together -
        // after which every launch opened the keyring for an account it holds nothing
        // for, and turning the keyring off reported a residue belonging to somebody
        // else, which no operation keyed by this account can find or remove.
        crate::test_helpers::with_isolated_config(|| {
            let (base, _listener) = always_answering_stub(r#"{"email":"new@x"}"#);
            let state = AppState::new().expect("state");
            {
                // The PREVIOUS account's launch: its entry is in the keyring and the
                // config records it.
                let mut config = state.config.lock();
                config.speleodb.endpoint = Some(base.clone());
                config.save().expect("seed the previous account's endpoint");
            }
            state
                .speleodb_write_account_record(
                    Some("old@x".to_owned()),
                    Some(account_key(&base, "old@x")),
                )
                .expect("seed the previous account's record");
            lock_the_keyring(&state);
            // Today's startup, as the previous account: the read fails, so the belief
            // it mirrored in stands - about `old@x`.
            state.speleodb_session_status().expect("status");

            let result = state
                .speleodb_login(Some(base), None, None, Some("tok-new".to_owned()))
                .expect("the paste login succeeds; only the keychain write fails");

            assert_eq!(
                result.storage,
                crate::speleodb::TokenStorage::Memory,
                "the locked keyring refused the write, so the key is session-only"
            );
            assert_eq!(
                state.get_config().speleodb.email.as_deref(),
                Some("new@x"),
                "premise: the persisted account is now somebody else"
            );
            assert!(
                !state.get_config().speleodb.keychain_has_token,
                "and what the keyring held for the PREVIOUS account is not a record \
                 about this one - the two are keyed apart"
            );

            // The next launch, reading the config this login just wrote, on the same
            // keyring that still will not answer.
            let restarted = AppState::new().expect("restarted state");
            lock_the_keyring(&restarted);
            let mut settings = restarted.get_config();
            settings.speleodb.keychain_disabled = true;
            let report = restarted
                .update_config(settings)
                .expect("the settings still save");

            assert_eq!(
                report.keychain_residue, None,
                "warning this account about a credential of somebody else's - one its \
                 own eviction could never have removed - is the false alarm"
            );
        });
    }

    #[test]
    fn the_same_address_on_a_second_server_does_not_inherit_the_first_ones_record() {
        // The account switch nobody thinks of as one: the SAME person signing in to a
        // second server (a dev box, a migrated host, a club's own instance). A keyring
        // entry is keyed by `account_key(endpoint, email)`, so the first server's entry
        // is not reachable under the second's key and there is usually none there at
        // all - but the recorded belief used to be scoped by the EMAIL alone, so it was
        // inherited whole.
        //
        // With the keyring locked during the switch - the case where it matters,
        // because then nothing can correct it - the clear and the store both fail, so
        // the first server's `true` was re-asserted and persisted under the second
        // server's account. Every later launch opened the keyring for an entry that is
        // not there, and turning the keyring off warned about a residue no operation
        // keyed by the new endpoint can find or remove.
        crate::test_helpers::with_isolated_config(|| {
            const FIRST: &str = "https://sdb.example";
            let (second, _listener) = always_answering_stub(r#"{"email":"u@x"}"#);
            let state = AppState::new().expect("state");
            {
                // The FIRST server is the persisted Settings endpoint, so the startup
                // below keys by it.
                let mut config = state.config.lock();
                config.speleodb.endpoint = Some(FIRST.to_owned());
                config.save().expect("seed the previous launch's endpoint");
            }
            // And what the login on that server left: the account, and the belief
            // stamped with the key its entry is stored under.
            state
                .speleodb_write_account_record(
                    Some("u@x".to_owned()),
                    Some(account_key(FIRST, "u@x")),
                )
                .expect("seed the first server's record");
            lock_the_keyring(&state);
            // Today's startup, still pointed at the first server: the read fails, so
            // the belief it mirrored in stands - about the entry on THAT server.
            state.speleodb_session_status().expect("status");

            // The same person, signing in to the second server.
            let result = state
                .speleodb_login(Some(second), None, None, Some("tok-2".to_owned()))
                .expect("the paste login succeeds; only the keychain write fails");

            assert_eq!(
                result.storage,
                crate::speleodb::TokenStorage::Memory,
                "the locked keyring refused the write, so the key is session-only"
            );
            assert_eq!(
                state.get_config().speleodb.email.as_deref(),
                Some("u@x"),
                "premise: the address is the same one - the EMAIL alone cannot tell \
                 these two accounts apart"
            );
            assert!(
                !state.get_config().speleodb.keychain_has_token,
                "and what the first server's keyring holds is not a record about the \
                 second one - the two are keyed apart by endpoint as well as email"
            );

            // The next launch, on the same keyring that still will not answer.
            let restarted = AppState::new().expect("restarted state");
            lock_the_keyring(&restarted);
            let mut settings = restarted.get_config();
            settings.speleodb.keychain_disabled = true;
            let report = restarted
                .update_config(settings)
                .expect("the settings still save");

            assert_eq!(
                report.keychain_residue, None,
                "warning about a credential on another server - one this account's own \
                 eviction could never have removed - is the false alarm"
            );
        });
    }

    #[test]
    fn a_belief_written_before_the_stamp_existed_is_kept_for_the_account_it_names() {
        // The upgrade path. Every config written before the belief carried an account
        // key has the flag and no stamp, and the reading rule for that pair decides
        // what happens on the first launch after the update.
        //
        // It is read as a belief about the persisted email, which is the strongest
        // scoping that config could express - the flag and the email were written by
        // the same login. Reading it as nobody's would be the strict answer and the
        // dangerous one: it would strand a real token on every upgraded user with a
        // keychain login (startup stops opening the keyring, `clear` stops sweeping it,
        // and turning the keyring off reports "nothing was in there" over it) to spare
        // the rare same-address-second-server case one unlock prompt. Being wrong the
        // other way costs that one prompt, and the read it pays for is what disproves
        // the belief and writes the correction back.
        crate::test_helpers::with_isolated_config(|| {
            const EP: &str = "https://sdb.example";
            let state = AppState::new().expect("state");
            {
                // A config from before the stamp: the flag, the account email, and
                // nothing naming the two together.
                let mut config = state.config.lock();
                config.speleodb.endpoint = Some(EP.to_owned());
                config.speleodb.email = Some("u@x".to_owned());
                config.speleodb.keychain_has_token = true;
                assert_eq!(config.speleodb.keychain_token_account, None);
                config.save().expect("seed the pre-upgrade config");
            }
            lock_the_keyring(&state);
            state.speleodb_session_status().expect("status");

            assert!(
                state.speleodb.lock().token_store.keychain_has_token(),
                "the unstamped belief still names this account, so it is mirrored in - \
                 dropping it here is what strands the token on the upgrade launch"
            );
            let mut settings = state.get_config();
            settings.speleodb.keychain_disabled = true;
            let report = state.update_config(settings).expect("the settings save");
            assert!(
                report.keychain_residue.is_some(),
                "and the user is still warned about the entry the locked keyring would \
                 not give up, which is the whole point of keeping it"
            );

            // Kept for the account it names, and only that one.
            let pre_upgrade = crate::config::SpeleoDbConfig {
                email: Some("u@x".to_owned()),
                keychain_has_token: true,
                keychain_token_account: None,
                ..Default::default()
            };
            assert!(pre_upgrade.keychain_belief_applies_to(EP, "u@x"));
            assert!(
                !pre_upgrade.keychain_belief_applies_to(EP, "someone@else"),
                "an unstamped belief is still scoped - it is the endpoint it cannot \
                 speak for, not the account entirely"
            );
        });
    }

    #[test]
    fn a_newer_logins_record_survives_the_rehydrate_write_back() {
        // DO NOT REGRESS. `speleodb_ensure_rehydrated` releases the SESSION lock before
        // it writes the proof back, deliberately: the writer takes the CONFIG lock and
        // the documented order is config-then-session. SpeleoDB commands run
        // concurrently on `spawn_blocking` threads, and the keychain read that produced
        // the proof can sit on a desktop unlock prompt for as long as the user leaves
        // it there, so a whole login fits in that window.
        //
        // Writing unconditionally then overwrote a fact newer than the one being
        // corrected, and the launch after that skipped a keyring entry the login had
        // genuinely just put there: signed out, with the token still in the keyring and
        // a `clear` that would no longer sweep it.
        //
        // The window is driven DIRECTLY here rather than raced: the interleaving needs
        // the login to land between the session-lock release and the config write, and
        // there is no hook to hold it there, so a genuine thread would be timing-
        // dependent. What is exercised is the compare-and-set itself, with the config
        // and the live store in exactly the state that login would have left them.
        crate::test_helpers::with_isolated_config(|| {
            const EP: &str = "https://sdb.example";
            let state = AppState::new().expect("state");
            state
                .speleodb_write_account_record(Some("a@x".to_owned()), Some(account_key(EP, "a@x")))
                .expect("what the previous launch recorded");
            // The snapshot the rehydrate read, and the proof it got back: the keyring
            // answered `Ok(None)` for a@x, so the live store lowered its belief and the
            // write-back is armed.
            let read = state.get_config().speleodb;
            empty_the_keyring(&state);

            // The login that lands in the window - as SOMEBODY ELSE, over a keyring
            // that takes the key: it records its own account and its own belief. The
            // live store is left believing nothing on purpose, which is the harder
            // half: `SpeleoDbSession::forget_reporting` drops the store's belief the
            // moment an account leaves the session, so between one login and the next
            // account switch the persisted record and the live store genuinely
            // disagree. What has to hold this write back there is the persisted pair,
            // not the store.
            state
                .speleodb_write_account_record(Some("b@x".to_owned()), Some(account_key(EP, "b@x")))
                .expect("the newer login's record");

            state.speleodb_lower_keychain_belief_after_proof(&read);

            let after = state.get_config().speleodb;
            assert_eq!(
                after.email.as_deref(),
                Some("b@x"),
                "premise: the newer login is the one on disk"
            );
            assert!(
                after.keychain_has_token,
                "a proof about the PREVIOUS account must not lower a belief the newer \
                 login raised - the next launch would skip the entry it just stored"
            );
            assert_eq!(
                after.keychain_token_account,
                Some(account_key(EP, "b@x")),
                "and the record left standing is the newer login's, whole"
            );
        });
    }

    #[test]
    fn a_relogin_that_refilled_the_keyring_survives_the_rehydrate_write_back() {
        // The half of that window a value comparison cannot see. When the login in the
        // window is the SAME account, it re-asserts the config's own belief, so a
        // successful keychain write leaves the persisted record byte-identical to the
        // snapshot the rehydrate read - while the keyring now really does hold an entry
        // that was not there when the read proved it absent.
        //
        // `TokenStore::keychain_has_token` is the authoritative copy and that write
        // raised it, so the write-back asks it before lowering. Driven directly for the
        // reason the sibling test above gives.
        crate::test_helpers::with_isolated_config(|| {
            const EP: &str = "https://sdb.example";
            let state = AppState::new().expect("state");
            state
                .speleodb_write_account_record(Some("a@x".to_owned()), Some(account_key(EP, "a@x")))
                .expect("what the previous launch recorded");
            let read = state.get_config().speleodb;
            empty_the_keyring(&state);

            // The same account, signing in again over a keyring that takes the key.
            // Nothing about the persisted record changes; the keyring's contents do.
            state
                .speleodb
                .lock()
                .token_store
                .note_keychain_has_token(true);
            assert_eq!(
                state.get_config().speleodb.keychain_token_account,
                read.keychain_token_account,
                "premise: the persisted record is untouched, so comparing it to the \
                 snapshot cannot tell that anything happened"
            );

            state.speleodb_lower_keychain_belief_after_proof(&read);

            assert!(
                state.get_config().speleodb.keychain_has_token,
                "the keyring holds an entry again, and a proof that predates it must \
                 not be written over the login that put it there"
            );
        });
    }

    #[test]
    fn the_account_and_the_belief_stamped_with_it_land_in_one_save() {
        // A login writes two facts that are only meaningful together: WHO is signed in,
        // and what the keyring is believed to hold FOR them. Written as two saves, the
        // email landed first, and a belief save that then failed - or a process that
        // exited between them - left the new account's email standing over the previous
        // account's `true`. Nothing ever repairs that pair: the next launch reads the
        // old account's evidence as the new one's, opens the keyring for an entry that
        // is not there, and warns about a residue nothing keyed by them can remove.
        //
        // Failure is injected by squatting the config FILE's path with a DIRECTORY, so
        // no save can land however it is spelled. The pair the process is left running
        // on is what this asserts: with one save it is coherent whether the save landed
        // or not; with two, the half that succeeded (or the half that rolled back)
        // leaves them describing different accounts.
        crate::test_helpers::with_isolated_config(|| {
            let squatted = crate::test_helpers::current_config_dir()
                .expect("this thread is isolated")
                .join("config.toml");
            std::fs::create_dir_all(&squatted).expect("squat the config file path");

            let (base, _listener) = always_answering_stub(r#"{"email":"new@x"}"#);
            let state = AppState::new().expect("state");
            {
                // The previous account's record, seeded coherently by hand because the
                // squat is already in place and no writer can reach the disk.
                let mut config = state.config.lock();
                config.speleodb.endpoint = Some(base.clone());
                config.speleodb.email = Some("old@x".to_owned());
                config.speleodb.keychain_has_token = true;
                config.speleodb.keychain_token_account = Some(account_key(&base, "old@x"));
            }
            lock_the_keyring(&state);
            state.speleodb_session_status().expect("status");

            state
                .speleodb_login(Some(base.clone()), None, None, Some("tok-new".to_owned()))
                .expect("the paste login succeeds; only the keychain write and the save fail");

            let pair = state.get_config().speleodb;
            assert_eq!(
                pair.email.as_deref(),
                Some("new@x"),
                "premise: the account this process is running as has moved"
            );
            assert!(
                !pair.keychain_has_token
                    || pair.keychain_belief_applies_to(&base, pair.email.as_deref().unwrap_or("")),
                "a recorded belief has to be about the recorded account, whichever half \
                 of the write failed: email {:?} against stamp {:?}",
                pair.email,
                pair.keychain_token_account
            );

            // And the record that did NOT move, because nothing could be written, is
            // the previous account's - coherent in its own right rather than half of
            // each.
            std::fs::remove_dir(&squatted).expect("unblock the config file");
            let on_disk = crate::config::Config::load()
                .expect("read the config back off disk")
                .speleodb;
            assert!(
                !on_disk.keychain_has_token
                    || on_disk
                        .keychain_belief_applies_to(&base, on_disk.email.as_deref().unwrap_or("")),
                "the pair on disk is coherent too: email {:?} against stamp {:?}",
                on_disk.email,
                on_disk.keychain_token_account
            );
        });
    }

    // ── [Track C] (#188) C4 rehydration + C5 401-drop unit tests ────────────

    /// A logged-out session whose token store has a keychain (and optional
    /// plaintext) backend pre-seeded, so `rehydrate` can find a token.
    fn fresh_session_with_stores(
        keychain: Box<dyn SecretStore>,
        plaintext: Option<Box<dyn SecretStore>>,
    ) -> SpeleoDbSession {
        SpeleoDbSession {
            endpoint: None,
            email: None,
            token_store: TokenStore::from_stores(keychain, plaintext),
            rehydrate_attempted: false,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
        }
    }

    /// A `SecretStore` that shares one `InMemoryStore` behind an `Arc`, so a test
    /// can keep a handle to seed/inspect the store *after* it has been boxed into
    /// a `TokenStore`. `InMemoryStore` is interior-mutable (`Mutex<HashMap>`), so
    /// every op delegates straight through the shared backend.
    struct SharedStore(std::sync::Arc<InMemoryStore>);

    impl SecretStore for SharedStore {
        fn kind(&self) -> &'static str {
            self.0.kind()
        }
        fn store(
            &self,
            account_key: &str,
            token: &str,
        ) -> Result<(), crate::speleodb::auth::SecretStoreError> {
            self.0.store(account_key, token)
        }
        fn load(
            &self,
            account_key: &str,
        ) -> Result<Option<String>, crate::speleodb::auth::SecretStoreError> {
            self.0.load(account_key)
        }
        fn delete(&self, account_key: &str) -> Result<(), crate::speleodb::auth::SecretStoreError> {
            self.0.delete(account_key)
        }
    }

    #[test]
    fn rehydration_seeds_session_from_config_and_keychain() {
        // A keychain holds a prior-run token; rehydrate seeds endpoint+email from
        // "config" and the first load() finds + caches it (the restart fix).
        let endpoint = "https://x/";
        let email = "u@x";
        let keychain = InMemoryStore::new();
        keychain
            .store(&account_key(endpoint, email), "kc-tok")
            .expect("seed keychain");

        let mut session = fresh_session_with_stores(Box::new(keychain), None);
        session.rehydrate(
            Some(endpoint.to_owned()),
            Some(email.to_owned()),
            false,
            false,
            true,
        );

        let status = session.status();
        assert!(status.logged_in, "a stored keychain token survives restart");
        assert!(status.keychain_backed);
        assert_eq!(status.user_email.as_deref(), Some(email));
        // The active token resolves to the keychain value.
        let (ep, tok) = session
            .active_token()
            .expect("authenticated after rehydrate");
        assert_eq!(ep, endpoint);
        assert_eq!(tok, "kc-tok");
    }

    #[test]
    fn rehydration_leaves_a_declined_keychain_alone() {
        // Item 12: the launch-time nag. A user who dismissed the desktop unlock
        // prompt once was asked again on EVERY launch, because rehydration read the
        // keychain unconditionally. With the refusal persisted, startup must not go
        // near it - even though a token is sitting right there.
        let endpoint = "https://x/";
        let email = "u@x";
        let keychain = InMemoryStore::new();
        keychain
            .store(&account_key(endpoint, email), "kc-tok")
            .expect("seed keychain");

        let mut session = fresh_session_with_stores(Box::new(keychain), None);
        session.rehydrate(
            Some(endpoint.to_owned()),
            Some(email.to_owned()),
            true,
            false,
            true,
        );

        let status = session.status();
        assert!(
            !status.logged_in,
            "a declined keychain must not be read on startup, even holding a token"
        );
        assert_eq!(
            status.keychain,
            KeychainReport::Declined,
            "and the reason is reported as a choice, not as a broken keychain"
        );
    }

    #[test]
    fn startup_does_not_open_the_keyring_when_we_recorded_no_token_in_it() {
        // The launch-time nag in its remaining form: we used to READ the keychain to
        // find out whether it had anything for us, and on Linux that read IS the
        // desktop unlock prompt - so a launch after a sign-out raised one purely to
        // discover there was nothing to find. What the keychain holds is recorded
        // when we put it there, so the question is answered from disk.
        let endpoint = "https://x/";
        let email = "u@x";
        // A keychain that DOES hold a usable token: if startup probed it, the
        // session would come back signed in, so staying signed out is the proof
        // that it was never opened.
        let keychain = InMemoryStore::new();
        keychain
            .store(&account_key(endpoint, email), "tok")
            .expect("seed keychain");

        let mut session = fresh_session_with_stores(Box::new(keychain), None);
        session.rehydrate(
            Some(endpoint.to_owned()),
            Some(email.to_owned()),
            false, // not declined
            false, // not disabled
            false, // ...but we have no record of a token being in there
        );

        assert!(
            !session.status().logged_in,
            "startup must not open the keyring to answer a question already on disk"
        );
    }

    #[test]
    fn rehydration_noop_without_config_account() {
        // No persisted account → stays logged-out, and the keychain is NOT probed
        // (no endpoint/email to key it). rehydrate_attempted is still latched so
        // it won't retry every call.
        let mut session = fresh_session_with_stores(Box::new(InMemoryStore::new()), None);
        session.rehydrate(None, None, false, false, true);
        assert!(!session.status().logged_in);
        assert!(session.rehydrate_attempted);

        // A blank endpoint/email also yields a logged-out no-op.
        let mut session = fresh_session_with_stores(Box::new(InMemoryStore::new()), None);
        session.rehydrate(
            Some("   ".to_owned()),
            Some(String::new()),
            false,
            false,
            true,
        );
        assert!(!session.status().logged_in);
    }

    #[test]
    fn rehydration_is_idempotent_and_skips_live_session() {
        // Once attempted, a second rehydrate (even with a now-seeded keychain) is
        // a no-op - it won't clobber or re-probe. The `Arc<InMemoryStore>` handle
        // (via the `SharedStore` delegating wrapper below) lets the test seed the
        // keychain *after* the latch is set, proving the latch - not an empty
        // store - is what keeps the second rehydrate logged-out.
        let endpoint = "https://x/";
        let email = "u@x";
        let keychain = std::sync::Arc::new(InMemoryStore::new());

        // First rehydrate with no account: latches `rehydrate_attempted`, probes
        // nothing, stays logged-out.
        let mut session = fresh_session_with_stores(
            Box::new(SharedStore(std::sync::Arc::clone(&keychain))),
            None,
        );
        session.rehydrate(None, None, false, false, true); // marks attempted, finds nothing
        assert!(session.rehydrate_attempted);
        assert!(!session.status().logged_in);

        // Now seed the SAME keychain store (shared via the Arc) with a usable
        // token for this account. A naive re-probe would find it and log in.
        keychain
            .store(&account_key(endpoint, email), "kc-tok")
            .expect("seed keychain after the latch is set");

        // Second rehydrate, this time WITH the account: the latch short-circuits
        // before any seed/probe, so the session stays logged-out and never adopts
        // endpoint/email despite the now-present token. This is the idempotency
        // invariant: `rehydrate_attempted` gates the keychain probe to once.
        session.rehydrate(
            Some(endpoint.to_owned()),
            Some(email.to_owned()),
            false,
            false,
            true,
        );
        assert!(
            !session.status().logged_in,
            "the latch must block the re-probe even with a now-seeded keychain"
        );
        assert_eq!(
            session.endpoint, None,
            "a latched rehydrate must not adopt the account or probe the token"
        );

        // A live session (endpoint set) is also skipped.
        let mut live = authed_session();
        live.rehydrate(
            Some("https://other/".to_owned()),
            Some("z@z".to_owned()),
            false,
            false,
            true,
        );
        assert_eq!(live.endpoint.as_deref(), Some("https://x/"));
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
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
        };
        let (endpoint, token) = session.active_token().expect("authenticated session");
        assert_eq!(endpoint, "https://x/");
        assert_eq!(token, "tok-1");

        // Each missing field independently yields NotAuthenticated. The empty
        // in-memory keychain (NOT TokenStore::default(), which would probe the
        // real OS keychain) keeps this hermetic.
        let mut no_token = SpeleoDbSession {
            endpoint: Some("https://x/".to_owned()),
            email: Some("u@x".to_owned()),
            token_store: TokenStore::from_stores(Box::new(InMemoryStore::new()), None),
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
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
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
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
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
        };
        assert!(matches!(
            no_endpoint.active_token(),
            Err(SpeleoDbError::NotAuthenticated)
        ));
    }

    // ===================== Live orchestration round-trips (#194/#197) =========
    // AppState driven against the REAL dev box: the full wiring (acquire ->
    // server_head -> fence -> edit gate / fast-forward / push / release) with real
    // server responses - what the offline LoopbackStub cannot reproduce. Gated on
    // DEV_SPELEODB_* + #[ignore]; run with `--ignored --test-threads=1`.

    use crate::core::{AppState, Command};
    use crate::models::test_support::require_env;
    use crate::models::{SectionData, ShotData, StationRef};
    use crate::speleodb::client;
    use crate::speleodb::{StartEditingResult, UpdateResult};
    use std::path::{Path, PathBuf};

    // Fixture paths are relative to THIS member's manifest dir
    // (src-tauri/crates/core), two levels below the old root crate's.
    const ORCH_TML: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/test_simple.tml"
    );

    /// The box and the first account. A test run on purpose without them fails
    /// naming the unset variable (#576 E13): `#[ignore]` is the skip, so a
    /// deliberate run must not report `ok` having tested nothing.
    fn orch_env() -> (String, String, String) {
        (
            require_env("DEV_SPELEODB_URL"),
            require_env("DEV_SPELEODB_EMAIL"),
            require_env("DEV_SPELEODB_PASSWORD"),
        )
    }

    /// The second pre-provisioned collaborator account (email + password); the
    /// two-account tests fail when `DEV_SPELEODB_EMAIL2`/`_PASSWORD2` are unset.
    /// (That account can't be created over HTTP; see
    /// docs/speleodb-dev-testing.md.)
    fn orch_env2() -> (String, String) {
        (
            require_env("DEV_SPELEODB_EMAIL2"),
            require_env("DEV_SPELEODB_PASSWORD2"),
        )
    }

    /// Deletes the per-test project on drop (even on panic), so live runs don't leak.
    struct OrchCleanup {
        url: String,
        token: String,
        id: String,
    }
    impl Drop for OrchCleanup {
        fn drop(&mut self) {
            let _ = client::delete_project(&self.url, &self.token, false, &self.id, &self.url);
        }
    }

    /// The `.stalx` the suite seeds the remote with, saved into `dir`: a synthetic
    /// project of three sections, the second branching off the first and the third
    /// off the second through station merges (#576 E13-D2). The suite used to read
    /// `e2e/fixtures/fractal_plant_iter2.stalx`, which the repo does not carry, so
    /// it could run on no clone but the maintainer's. `advanced` adds one shot to
    /// the last section, so a second upload is a distinct commit.
    fn orch_seed_stalx(dir: &Path, advanced: bool) -> PathBuf {
        let shot = |length: f64, azimuth: f64| ShotData {
            length,
            azimuth_in: azimuth,
            azimuth_out: (azimuth + 180.0) % 360.0,
            depth_in: 3.0,
            depth_out: 3.0,
            ..Default::default()
        };
        let mut tip = vec![shot(4.0, 200.0), shot(4.0, 210.0)];
        if advanced {
            tip.push(shot(2.5, 220.0));
        }
        let mut project = Project::new("CaveA".to_owned(), None);
        let mut ids = Vec::new();
        for (name, shots) in [
            ("A", vec![shot(5.0, 0.0), shot(5.0, 10.0), shot(5.0, 20.0)]),
            ("B", vec![shot(3.0, 90.0), shot(3.0, 100.0)]),
            ("C", tip),
        ] {
            let section = project.add_section(SectionData {
                name: Some(name.to_owned()),
                color: "#4ecdc4".to_owned(),
                shots,
                ..Default::default()
            });
            ids.push(section.id);
        }
        for (from, to) in [((ids[0], 2), (ids[1], 0)), ((ids[1], 1), (ids[2], 0))] {
            project
                .merge_stations(StationRef::new(from.0, from.1), StationRef::new(to.0, to.1))
                .expect("merge the seed's branches");
        }
        let path = dir.join(if advanced {
            "seed-v2.stalx"
        } else {
            "seed.stalx"
        });
        let mut db = Database::open(&path).expect("create the seed .stalx");
        db.save_project(&project).expect("save the seed project");
        path
    }

    /// Not ignored: the live suite's seed is built on every run, so a seed that no
    /// longer opens fails here, on any clone, rather than on the dev box.
    #[test]
    fn the_live_suite_seed_opens_with_its_branches_and_its_advanced_twin_differs() {
        let dir = TempDir::new().expect("seed tempdir");
        let seed = orch_seed_stalx(dir.path(), false);
        let advanced = orch_seed_stalx(dir.path(), true);
        let load = |path: &Path| {
            Database::open_existing(path)
                .expect("the seed opens")
                .load_project()
                .expect("the seed loads")
        };
        let (project, next) = (load(&seed), load(&advanced));
        let shots =
            |p: &Project| -> Vec<usize> { p.sections.iter().map(|s| s.shots.len()).collect() };
        assert_eq!(shots(&project), [3, 2, 2]);
        assert_eq!(project.station_merges.len(), 2, "both branches are merged");
        assert_eq!(
            shots(&next),
            [3, 2, 3],
            "the advanced seed has one more shot"
        );
        assert_ne!(
            std::fs::read(&seed).expect("read seed"),
            std::fs::read(&advanced).expect("read advanced seed"),
            "so its upload is a distinct commit"
        );
    }

    /// Create a fresh remote project seeded with the synthetic `.stalx` (lock
    /// released), returning its id. The token owner is ADMIN on the new project.
    fn orch_seed(url: &str, token: &str, name: &str) -> String {
        let seed_dir = TempDir::new().expect("seed tempdir");
        let stalx = orch_seed_stalx(seed_dir.path(), false);
        let id = client::create_project(url, token, false, name)
            .expect("create project")
            .id;
        client::acquire_lock(url, token, false, &id).expect("acquire seed lock");
        client::upload_auto(
            &client::TestWire,
            url,
            token,
            false,
            &id,
            &stalx,
            Path::new(ORCH_TML),
            "seed",
        )
        .expect("seed upload");
        client::release_lock(url, token, false, &id).expect("release seed lock");
        id
    }

    /// Advance the remote tip with a DISTINCT upload (the seed plus one shot → a
    /// new commit), simulating another writer moving HEAD forward after our open.
    fn orch_advance_tip(url: &str, token: &str, id: &str) {
        let seed_dir = TempDir::new().expect("seed tempdir");
        let stalx = orch_seed_stalx(seed_dir.path(), true);
        client::acquire_lock(url, token, false, id).expect("acquire advance lock");
        client::upload_auto(
            &client::TestWire,
            url,
            token,
            false,
            id,
            &stalx,
            Path::new(ORCH_TML),
            "advance tip",
        )
        .expect("advance upload");
        client::release_lock(url, token, false, id).expect("release advance lock");
    }

    /// A fresh `AppState` whose SpeleoDB session carries a REAL dev-box token.
    fn orch_state(url: &str, email: &str, token: &str) -> AppState {
        // The pull/open path needs a resolved cache dir; without a Tauri runtime
        // point it at a throwaway temp tree.
        crate::app_dirs::init_for_test();
        let state = AppState::new().expect("AppState::new");
        *state.speleodb.lock() = SpeleoDbSession {
            endpoint: Some(url.to_owned()),
            email: Some(email.to_owned()),
            token_store: TokenStore::from_in_memory_token(token),
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
            unconfirmed_acquire: None,
            chosen_dump_entries: std::collections::HashMap::new(),
            offered_dump_at: std::collections::HashMap::new(),
        };
        state
    }

    /// Read the speleo_link straight from the open project DB.
    fn orch_link(state: &AppState) -> SpeleoLink {
        state
            .database
            .lock()
            .as_ref()
            .expect("db open")
            .load_speleo_link()
            .expect("load link")
            .expect("link present")
    }

    /// Open a remote project INTO this AppState: run the REAL download + BASE-stamp
    /// pull (`speleodb_open_project`), then load the extracted `.stalx` into state.
    /// The production load (`do_load_project_full`) needs a Wry `AppHandle` we can't
    /// build headless, so we do its essential work directly - open the db + project
    /// and drop the edit gate (a freshly pulled remote project is read-only until
    /// Start editing). The push path reads the same `speleodb_cache_path`, so the
    /// `file_path` we stamp here matches the live `.stalx` the upload snapshots.
    fn orch_open(state: &AppState, id: &str, name: &str) {
        let res = state
            .speleodb_open_project(id.to_owned(), name.to_owned(), None, None)
            .expect("pull + stamp");
        let path = PathBuf::from(res.local_path);
        let db = Database::open_existing(&path).expect("open extracted .stalx");
        let mut project = db.load_project().expect("load pulled project");
        project.file_path = Some(path);
        *state.database.lock() = Some(db);
        *state.project.lock() = Some(project).into();
        state.set_edit_enabled(false);
    }

    /// A real survey edit (a new section with one shot), which both changes the
    /// pushed content and sets the dirty bit through the normal mutation path.
    fn orch_edit(state: &AppState, name: &str, length: f64) {
        assert!(
            state
                .execute(Command::CreateSection {
                    name: Some(name.to_owned()),
                    surveyor: None,
                    date: None,
                    color: "#abcdef".to_owned(),
                    direction: None,
                    shots: vec![ShotData {
                        length,
                        ..Default::default()
                    }],
                })
                .success,
            "edit succeeds while editing"
        );
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_start_editing_edit_done_advances_base() {
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-roundtrip");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-roundtrip");
        assert!(
            !state.edit_enabled(),
            "a freshly pulled remote project is read-only until Start editing"
        );
        let base_before = orch_link(&state).base_commit_sha;

        // Start editing: clean reconcile -> editable, lock held.
        let started = state.speleodb_start_editing().expect("start_editing");
        assert!(
            matches!(started, StartEditingResult::Editing),
            "clean reconcile should enable editing, got {started:?}"
        );
        assert!(
            state.edit_enabled(),
            "editing enabled after a clean Start editing"
        );
        // #194 b1: acquire takes the mutex only; a clean (UpToDate) reconcile must
        // NOT move BASE. Only a confirmed push (below) advances it.
        assert_eq!(
            orch_link(&state).base_commit_sha,
            base_before,
            "acquire + clean reconcile must not move BASE"
        );

        // A real survey edit (new section) -> dirty bit set + content changed.
        assert!(
            state
                .execute(Command::CreateSection {
                    name: Some("orch-edit".to_owned()),
                    surveyor: None,
                    date: None,
                    color: "#abcdef".to_owned(),
                    direction: None,
                    shots: vec![ShotData {
                        length: 5.0,
                        ..Default::default()
                    }],
                })
                .success,
            "edit succeeds while editing"
        );
        assert!(
            orch_link(&state).has_unpushed_edits,
            "edit marks the project dirty"
        );

        // Done: push (200, new content) + release -> base advances, dirty cleared,
        // read-only again.
        let done = state.speleodb_done(None).expect("done");
        assert!(
            matches!(done, DoneResult::Released),
            "a confirmed push then release should report Released, got {done:?}"
        );
        assert!(!state.edit_enabled(), "read-only again after Done");
        let after = orch_link(&state);
        assert!(
            !after.has_unpushed_edits,
            "dirty cleared after a confirmed push"
        );
        assert!(
            after.base_commit_sha.is_some(),
            "BASE is the adopted hexsha"
        );
        assert_ne!(
            after.base_commit_sha, base_before,
            "BASE advanced to the new commit after a 200 push"
        );
    }

    /// Push, edit again, then Done - WITHOUT releasing the lock in between.
    ///
    /// The user-reported self-conflict: a second push against your own first one
    /// raises the divergence ("merge conflict") prompt. Nobody else touched the
    /// project, so every step here must stay on the happy path. This test pins the
    /// BACKEND half of that, which is also what discriminates the candidate causes:
    /// if it goes red, the fault is in the push/fence machinery; if it stays green,
    /// the fault is in the frontend (a status poll that captured the pre-push link
    /// row landing after the push and repainting the button `Conflict`).
    ///
    /// The mid-flight `speleodb_link_status` assertion is the important one: it is
    /// exactly what the status button polls, and `speleodb_remote_state` fences BASE
    /// against the tip with NO ancestry recovery, unlike the push path's `plan_push`.
    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_push_then_edit_then_done_does_not_self_conflict() {
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-push-twice");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-push-twice");
        let base_at_open = orch_link(&state).base_commit_sha;

        assert!(
            matches!(
                state.speleodb_start_editing().expect("start_editing"),
                StartEditingResult::Editing
            ),
            "clean reconcile should enable editing"
        );

        // --- first edit, then Push WITHOUT releasing (the hamburger's "Push (keep
        // the lock)") -------------------------------------------------------------
        orch_edit(&state, "orch-edit-1", 5.0);
        let pushed = state.speleodb_push_only(None).expect("first push");
        assert!(
            matches!(pushed, PushResult::Pushed),
            "the first push should land cleanly, got {pushed:?}"
        );
        assert!(
            state.edit_enabled(),
            "push-without-release keeps the edit session open"
        );
        let after_push = orch_link(&state);
        assert!(
            !after_push.has_unpushed_edits,
            "a confirmed push clears the dirty bit"
        );
        assert_ne!(
            after_push.base_commit_sha, base_at_open,
            "BASE must advance to the commit the push created - leaving it behind (or \
             wiping it to None) is what makes the NEXT push read as a divergence"
        );
        assert!(
            after_push.base_commit_sha.is_some(),
            "BASE must never be wiped to the empty-repo sentinel by a successful push"
        );

        // The BASE we now hold must be the server's real tip, resolved from the
        // parent-id topology rather than the unreliable `latest_commit` field.
        let (tip_after_push, _) = state
            .speleodb_server_head(&url, &token, false, &id)
            .expect("server head after the first push");
        assert_eq!(
            after_push.base_commit_sha, tip_after_push,
            "BASE after a push must equal the topological server tip"
        );

        // --- second edit: the state the user is in when they press the button -----
        orch_edit(&state, "orch-edit-2", 7.0);
        assert!(
            orch_link(&state).has_unpushed_edits,
            "the second edit marks the project dirty again"
        );

        // What the status button polls. `Conflict` here is what flips the button red
        // and routes the click to the divergence dialog instead of push & release.
        let status = state.speleodb_link_status().expect("link status");
        assert!(
            !matches!(status.remote_state, RemoteState::Conflict),
            "our own just-pushed commit must not read as a conflict, got {:?}",
            status.remote_state
        );

        // --- Done: push & release, the step the user reported as conflicting ------
        let done = state.speleodb_done(None).expect("done");
        assert!(
            matches!(done, DoneResult::Released),
            "pushing again over our OWN previous push must release cleanly, not raise \
             a divergence - got {done:?}"
        );
        assert!(!state.edit_enabled(), "read-only again after Done");
        let after_done = orch_link(&state);
        assert!(
            !after_done.has_unpushed_edits,
            "dirty cleared after the second confirmed push"
        );
        assert_ne!(
            after_done.base_commit_sha, after_push.base_commit_sha,
            "BASE advanced again for the second commit"
        );
    }

    /// The self-conflict, reproduced deterministically at the layer it actually
    /// happens in: a status poll that read the link row BEFORE a push and resolves
    /// the server tip AFTER it.
    ///
    /// `speleodb_link_status` loads the link, releases the DB lock, and only then
    /// makes its two network round trips; `run_blocking` holds no command-level
    /// lock, so a push can land in that window. The poll then fences a BASE that is
    /// behind the tip by exactly one commit - OUR OWN - and `speleodb_remote_state`
    /// applies a bare `sync::fence` with none of the ancestry recovery `plan_push`
    /// has, so it answers `Conflict`. That verdict paints the status button red and
    /// routes the next click to the divergence dialog.
    ///
    /// Passing `link_before_push` here is not a contrivance: it is byte-for-byte the
    /// snapshot the in-flight poll is holding.
    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_stale_poll_snapshot_does_not_report_our_own_push_as_conflict() {
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-stale-poll");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-stale-poll");
        assert!(matches!(
            state.speleodb_start_editing().expect("start_editing"),
            StartEditingResult::Editing
        ));
        orch_edit(&state, "orch-stale-edit", 5.0);

        // What a poll issued a moment before the push is holding: BASE at the
        // pre-push commit, dirty still set.
        let link_before_push = orch_link(&state);
        assert!(link_before_push.has_unpushed_edits);

        // The push lands while that poll is still waiting on the network.
        assert!(matches!(
            state.speleodb_push_only(None).expect("push"),
            PushResult::Pushed
        ));

        // Now the poll's revisions GET returns - resolving a tip that has moved on
        // by our own commit. Nobody else has touched this project, and we are still
        // the lock holder (push_only keeps the session editing), which is what
        // licenses the authorship recovery.
        let verdict =
            state.speleodb_remote_state(&url, &token, false, &link_before_push, Some(&email));
        assert!(
            !matches!(verdict, RemoteState::Conflict),
            "a tip that moved ahead of a stale BASE by OUR OWN commit must not read as \
             a conflict - this is the self-conflict the user reported, got {verdict:?}"
        );
    }

    /// Releasing our own write lock from the admin surface must also close the local
    /// edit gate.
    ///
    /// The gate is what `guard_mutation` reads, and the status poll only ever OPENS
    /// it, so leaving it set left the shell editable over a lock that no longer
    /// exists - an unflagged lockless edit session. The revert flow (#8) hands a
    /// lock back through this path, so it has to be true.
    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_release_current_lock_closes_the_edit_gate() {
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-release-gate");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-release-gate");
        assert!(matches!(
            state.speleodb_start_editing().expect("start_editing"),
            StartEditingResult::Editing
        ));
        assert!(state.edit_enabled(), "editing after a clean Start editing");

        state
            .speleodb_release_current_lock()
            .expect("release our own lock");

        assert!(
            !state.edit_enabled(),
            "releasing our own write lock must close the edit gate - otherwise the shell \
             stays editable over a lock that is gone"
        );
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_update_up_to_date_on_fresh_open() {
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-update-utd");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-update-utd");
        // Freshly opened at the tip -> Update is a no-op.
        let r = state.speleodb_update().expect("update");
        assert!(
            matches!(r, UpdateResult::UpToDate),
            "a fresh open is up to date, got {r:?}"
        );
    }

    // NOTE on fast-forward coverage: the FF *decision* (clean local + moved tip ->
    // FastForward) is exhaustively unit-tested in `sync.rs` (fence/ancestry). Its
    // *execution* re-pulls and reloads via `do_load_project_full`, which hard-requires
    // a Wry `AppHandle` (progress events) that can't be built in a headless test -
    // so a live FF orchestration test is intentionally omitted here rather than
    // asserting a test-environment artifact.

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_update_diverges_when_dirty_and_tip_moved() {
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-diverge");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-diverge");
        // Local un-pushed edits...
        state.mark_remote_dirty();
        // ...AND the tip moved underneath us -> a genuine fork, never an overwrite.
        orch_advance_tip(&url, &token, &id);

        let r = state.speleodb_update().expect("update");
        assert!(
            matches!(r, UpdateResult::DivergenceNeeded),
            "dirty + moved tip must route to divergence, got {r:?}"
        );
        // Update never overwrites: the dirty bit is still set, awaiting resolution.
        assert!(
            orch_link(&state).has_unpushed_edits,
            "divergence keeps local edits (no blind overwrite)"
        );
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_open_flags_concurrent_same_account_lock() {
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-concurrent");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);

        // No lock held at open -> no concurrent-session notice (#193 b8).
        let unlocked = state
            .speleodb_open_project(id.clone(), "orch-concurrent".to_owned(), None, None)
            .expect("open (unlocked)");
        assert!(
            !unlocked.concurrent_session,
            "no lock held -> no concurrent-session notice"
        );

        // Our OWN account now holds the lock (a second same-account session): the
        // next open must warn, because the ancestry self-heal can't tell that
        // session's commits from ours.
        client::acquire_lock(&url, &token, false, &id).expect("acquire as same account");
        let locked = state
            .speleodb_open_project(id.clone(), "orch-concurrent".to_owned(), None, None)
            .expect("reopen (same-account lock held)");
        assert!(
            locked.concurrent_session,
            "a same-account lock-hold at open must raise the notice"
        );
        client::release_lock(&url, &token, false, &id).expect("release");
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_start_editing_diverges_on_unreconcilable_base() {
        // The dev server's history is structurally linear, so it can't produce a
        // merge / multi-root. A BASE that is not an ancestor of the tip drives the
        // SAME "can't reconcile -> divergence" routing (#193 b2): Start editing must
        // surface the divergence PROMPT (never a hard error) and release the lock it
        // just took, not strand it.
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-se-diverge");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-se-diverge");
        // Forge an un-reconcilable BASE: a real-shaped SHA absent from history, so
        // base_is_ancestor_of_tip() is false and the verdict is Diverged.
        state
            .speleodb_mutate_link(|l| l.base_commit_sha = Some("0".repeat(40)))
            .expect("forge stale base");

        let r = state.speleodb_start_editing().expect("start_editing");
        assert!(
            matches!(r, StartEditingResult::DivergenceNeeded),
            "an unreconcilable base must route to the divergence prompt, got {r:?}"
        );
        assert!(!state.edit_enabled(), "divergence must not enable editing");
        // The lock taken during Start editing must be released on divergence.
        let holder = client::get_project(&url, &token, false, &id)
            .ok()
            .and_then(|p| p.active_mutex.map(|m| m.user));
        assert!(
            holder.is_none(),
            "the freshly-taken lock must be released on divergence, still held by {holder:?}"
        );
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_pushed_snapshot_carries_the_undo_history_since_the_last_commit() {
        // [#303] The end-to-end shape of the hybrid stack, against the real server. This
        // test used to assert the OPPOSITE (`..._carries_no_undo_history`, #194 b3) and
        // is inverted deliberately - three assertions, all of which flipped:
        //
        // 1. the pushed snapshot DOES carry the undo history (Full mode is the default),
        //    which is what gives undo cross-commit reach;
        // 2. it carries NO redo, in any mode (clarification #1); and
        // 3. the LOCAL stack is EMPTY after the push (clear-on-push, q8): those
        //    operations are in the commit now, so keeping them would duplicate them in
        //    the next one.
        //
        // The #194 b3 hazard the first assertion relaxes is contained elsewhere now: a
        // pulled stack is cleared on every implicit adopt path, and is only handed back
        // through the explicit, confirmed, lock-held Unwind.
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-undo");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-undo");
        let live_path = state
            .project
            .lock()
            .as_ref()
            .and_then(|p| p.file_path.clone())
            .expect("open project has a file path");

        state.speleodb_start_editing().expect("start_editing");
        assert!(
            state
                .execute(Command::CreateSection {
                    name: Some("undo-edit".to_owned()),
                    surveyor: None,
                    date: None,
                    color: "#abcdef".to_owned(),
                    direction: None,
                    shots: vec![ShotData {
                        length: 5.0,
                        ..Default::default()
                    }],
                })
                .success,
            "edit succeeds while editing"
        );
        state.speleodb_done(None).expect("done");

        // [#303] Clear-on-push, in memory: the operation that push carried is gone, so
        // the next commit cannot duplicate it.
        assert_eq!(
            state.undo_stack.lock().len(),
            0,
            "a confirmed push clears the undo entries it uploaded (q8)"
        );

        // Re-download the SERVER's copy and read what the commit actually stored.
        let server = state
            .speleodb_open_project(id.clone(), "orch-undo".to_owned(), None, None)
            .expect("re-download server copy");
        let server_db =
            Database::open_existing(Path::new(&server.local_path)).expect("open server copy");
        assert!(
            !server_db
                .load_undo_stack_with_timestamps()
                .expect("load server undo")
                .is_empty(),
            "Full mode must ship the undo history since the last commit (#303) - this is \
             what lets a later Unwind fetch it back and keep undoing past the commit"
        );
        assert!(
            server_db
                .load_redo_stack_with_timestamps()
                .expect("load server redo")
                .is_empty(),
            "redo is never uploaded, in any mode (clarification #1)"
        );

        // And the second push carries ONLY the second edit - the point of the whole
        // exercise (no duplication across commits). Re-acquire, edit, Done again.
        state
            .speleodb_start_editing()
            .expect("start_editing for the second push");
        assert!(
            state
                .execute(Command::CreateSection {
                    name: Some("undo-edit-2".to_owned()),
                    surveyor: None,
                    date: None,
                    color: "#123456".to_owned(),
                    direction: None,
                    shots: vec![ShotData {
                        length: 7.0,
                        ..Default::default()
                    }],
                })
                .success,
            "second edit succeeds while editing"
        );
        state.speleodb_done(None).expect("second done");

        let server2 = state
            .speleodb_open_project(id.clone(), "orch-undo".to_owned(), None, None)
            .expect("re-download after the second push");
        let commit2 = Database::open_existing(Path::new(&server2.local_path))
            .expect("open second server copy")
            .load_undo_stack_with_timestamps()
            .expect("load second server undo");
        assert_eq!(
            commit2.len(),
            1,
            "commit 2 must carry ONLY the operations made after commit 1, not the whole \
             session again - got {commit2:?}"
        );
        assert!(
            commit2
                .iter()
                .all(|(_, command_json, ..)| command_json.contains("undo-edit-2")),
            "and specifically the SECOND edit, not the first: {commit2:?}"
        );

        // `live_path` is the same cache file throughout; naming it keeps the re-download
        // assertions above honestly about the SERVER's copy rather than this one.
        assert!(live_path.exists(), "the local cache file is still there");
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* + _EMAIL2/_PASSWORD2 (run with --ignored --test-threads=1)"]
    fn live_orch_start_editing_reports_locked_by_other_account() {
        // #194 b5: a contended acquire (HTTP 400 - lock held by someone else) maps
        // to StartEditingResult::LockedBy{email}, surfacing WHO holds it, not an
        // error. Needs the second collaborator account.
        let ((url, owner_email, owner_pw), (email2, pw2)) = (orch_env(), orch_env2());
        let owner = client::login(&url, false, &owner_email, &owner_pw).expect("owner login");
        let id = orch_seed(&url, &owner, "orch-lockedby");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: owner.clone(),
            id: id.clone(),
        };
        client::grant_permission(&url, &owner, &id, &email2, "READ_AND_WRITE")
            .expect("grant collaborator write access");

        // The owner opens the project read-only...
        let state = orch_state(&url, &owner_email, &owner);
        orch_open(&state, &id, "orch-lockedby");

        // ...meanwhile the OTHER account takes the lock.
        let collab = client::login(&url, false, &email2, &pw2).expect("collaborator login");
        client::acquire_lock(&url, &collab, false, &id).expect("collaborator acquires the mutex");

        // The owner's Start editing now contends -> LockedBy{the collaborator}.
        let r = state.speleodb_start_editing().expect("start_editing");
        assert!(
            matches!(&r, StartEditingResult::LockedBy { email } if email.as_deref() == Some(email2.as_str())),
            "expected LockedBy{{{email2}}}, got {r:?}"
        );
        assert!(
            !state.edit_enabled(),
            "a contended Start editing must not enable editing"
        );

        // Release the collaborator's lock so cleanup's delete isn't fighting it.
        let _ = client::release_lock(&url, &collab, false, &id);
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_pushed_dump_is_self_contained_without_sidecars() {
        // #195 end-to-end against the real server: after a Done push, the dump the
        // server STORED must be exactly the VACUUM snapshot - one self-contained
        // `.stalx` (opens via open_existing with the pushed edit) co-pushed with the
        // `ariane.tml`, and NO `-wal`/`-journal`/`-shm` sidecar ever rode along
        // (bullets 1/2/3). The cache `.stalx` is default DELETE journal mode, so this
        // also exercises the push pipeline under DELETE (bullet 4 DELETE); the WAL
        // case is a client-local vacuum concern covered by vacuum_snapshot_under_wal_mode.
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-dump");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-dump");
        state.speleodb_start_editing().expect("start_editing");
        assert!(
            state
                .execute(Command::CreateSection {
                    name: Some("dump-edit".to_owned()),
                    surveyor: None,
                    date: None,
                    color: "#abcdef".to_owned(),
                    direction: None,
                    shots: vec![ShotData {
                        length: 5.0,
                        ..Default::default()
                    }],
                })
                .success,
            "edit succeeds while editing"
        );
        state.speleodb_done(None).expect("done");

        // Inspect the archive the server now holds.
        let zip = client::download_dump(&client::TestWire, &url, &token, false, &id)
            .expect("download dump");
        let mut dump = crate::speleodb::dump::Dump::open(&zip).expect("open server dump");
        let entries = dump.entry_paths().expect("dump entry paths");

        // (3) no SQLite journal sidecar ever left the client.
        for entry in &entries {
            for suffix in ["-wal", "-journal", "-shm"] {
                assert!(
                    !entry.ends_with(suffix),
                    "no sqlite sidecar may ride to the server: {entry} in {entries:?}"
                );
            }
        }
        // Exactly one `.stalx` (the snapshot) - never the ambiguous multi-.stalx shape.
        let stalx_count = entries.iter().filter(|e| e.ends_with(".stalx")).count();
        assert_eq!(
            stalx_count, 1,
            "the server dump must hold exactly one .stalx, got {entries:?}"
        );
        // (#196 b2/b3/b5) the .tml is co-pushed, lands as `ariane.tml`, and is the
        // ONLY .tml - no Ariane-collab sibling rode along.
        assert!(
            entries.iter().any(|e| e.ends_with("ariane.tml")),
            "the render must be co-pushed as ariane.tml, got {entries:?}"
        );
        let tml_count = entries.iter().filter(|e| e.ends_with(".tml")).count();
        assert_eq!(
            tml_count, 1,
            "exactly one .tml (ariane.tml) - no collab sibling rode along, got {entries:?}"
        );

        // (1/2) it extracts + opens as a SELF-CONTAINED DB carrying the pushed edit.
        let entry = classify_dump_entry(&entries).expect("classify the dump");
        let temp = TempDir::new().expect("temp dir");
        let out = temp.path().join("server_snap.stalx");
        dump.extract_entry_to(&entry, &out).expect("extract .stalx");
        for suffix in ["-wal", "-journal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", out.to_string_lossy()));
            assert!(
                !sidecar.exists(),
                "the extracted snapshot must be self-contained: {sidecar:?} must not exist"
            );
        }
        let server_db =
            Database::open_existing(&out).expect("server snapshot opens self-contained");
        let project = server_db
            .load_project()
            .expect("load project from server snapshot");
        assert!(
            project
                .sections
                .iter()
                .any(|s| s.name.as_deref() == Some("dump-edit")),
            "the pushed edit must be present in the server's self-contained snapshot"
        );
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_pushed_tml_byte_stable_vs_rerender() {
        // #196 b4 end-to-end: the ariane.tml the server STORED must be byte-identical
        // to a fresh local re-render of the same (UUID-minted) project. The render is
        // deterministic - the one-time shot-UUID pass ran on push - so re-pushing
        // unchanged content never churns the .tml. (The offline byte-stability test
        // proves the render is deterministic; this proves the bytes the server holds
        // are exactly that deterministic render, intact through the round-trip.)
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-tmlstable");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-tmlstable");
        state.speleodb_start_editing().expect("start_editing");
        assert!(
            state
                .execute(Command::CreateSection {
                    name: Some("tml-edit".to_owned()),
                    surveyor: None,
                    date: None,
                    color: "#abcdef".to_owned(),
                    direction: None,
                    shots: vec![ShotData {
                        length: 5.0,
                        ..Default::default()
                    }],
                })
                .success,
            "edit succeeds while editing"
        );
        state.speleodb_done(None).expect("done");

        // Pull the ariane.tml the server now holds.
        let zip = client::download_dump(&client::TestWire, &url, &token, false, &id)
            .expect("download dump");
        let mut dump = crate::speleodb::dump::Dump::open(&zip).expect("open server dump");
        let entries = dump.entry_paths().expect("dump entry paths");
        let tml_entry = entries
            .iter()
            .find(|e| e.ends_with("ariane.tml"))
            .expect("ariane.tml present in the dump");
        let temp = TempDir::new().expect("temp dir");
        let server_tml = temp.path().join("server.tml");
        dump.extract_entry_to(tml_entry, &server_tml)
            .expect("extract server ariane.tml");

        // Re-render the same in-memory project (UUIDs were minted + persisted on push).
        let project = state.project.lock().as_ref().expect("open project").clone();
        let rerender = temp.path().join("rerender.tml");
        crate::export::tml::export_tml_for_speleodb(&project, &rerender).expect("re-render .tml");

        assert_eq!(
            std::fs::read(&server_tml).expect("read server ariane.tml"),
            std::fs::read(&rerender).expect("read re-rendered .tml"),
            "the stored ariane.tml must be byte-identical to a fresh re-render (#196 b4)"
        );
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_done_without_edits_skips_push_and_releases() {
        // #197 b1: a non-dirty Done must push NOTHING (the dirty-bit gate skips the
        // upload) yet still release the lock back to read-only - no empty commit.
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-noedit");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-noedit");
        let started = state.speleodb_start_editing().expect("start_editing");
        assert!(
            matches!(started, StartEditingResult::Editing),
            "clean reconcile should enable editing, got {started:?}"
        );
        assert!(
            !orch_link(&state).has_unpushed_edits,
            "no edits made yet → the session is not dirty"
        );
        let base_before = orch_link(&state).base_commit_sha;

        // Commit count BEFORE the no-op Done.
        let (_, before) = state
            .speleodb_server_head(&url, &token, false, &id)
            .expect("revisions before Done");

        let done = state.speleodb_done(None).expect("done");
        assert!(
            matches!(done, DoneResult::Released),
            "a non-dirty Done releases (push skipped), got {done:?}"
        );
        assert!(!state.edit_enabled(), "read-only again after Done");
        assert_eq!(
            orch_link(&state).base_commit_sha,
            base_before,
            "a skipped push must not advance BASE"
        );

        // No commit was pushed.
        let (_, after) = state
            .speleodb_server_head(&url, &token, false, &id)
            .expect("revisions after Done");
        assert_eq!(
            before.len(),
            after.len(),
            "a non-dirty Done must push no commit (got {} -> {})",
            before.len(),
            after.len()
        );
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored --test-threads=1)"]
    fn live_orch_ambiguous_upload_preserves_dirty_bit() {
        // #197 b5: an ambiguous push (transport drop / exhausted 5xx - we never learn
        // whether it landed) must NOT clear the dirty bit or advance BASE; the
        // un-pushed edits survive for ancestry recovery (g7) on the next Done. Forced
        // by pointing the UPLOAD at a dead port (connection refused -> Transport ->
        // StayDirty) while reusing a live-opened project for the real flush/snapshot.
        let (url, email, password) = orch_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let id = orch_seed(&url, &token, "orch-ambig");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: token.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &email, &token);
        orch_open(&state, &id, "orch-ambig");

        // Make the session dirty (simulate an un-pushed edit): mark_remote_dirty stamps
        // the bit + last_edit_at together, exactly as a real edit would.
        state
            .speleodb_mutate_link(|l| {
                l.has_unpushed_edits = true;
                l.last_edit_at = Some("2020-01-01T00:00:00+00:00".to_owned());
            })
            .expect("mark dirty");
        let link = orch_link(&state);
        let base_before = link.base_commit_sha.clone();
        assert!(
            link.has_unpushed_edits,
            "precondition: the session is dirty"
        );

        // Upload to a dead endpoint -> transport failure -> ambiguous outcome.
        // (fold_metadata=false keeps the test on the upload path, not the #216 GET.)
        let outcome = state
            .speleodb_upload(
                "http://127.0.0.1:1/",
                &token,
                false,
                &link,
                None,
                false,
                &mut Vec::new(),
            )
            .expect("an ambiguous upload returns an outcome, not a hard error");
        assert!(
            matches!(outcome, PushOutcome::Unconfirmed),
            "an ambiguous push must report Unconfirmed, got {outcome:?}"
        );

        // The dirty bit + BASE survive untouched, so the edits aren't dropped.
        let after = orch_link(&state);
        assert!(
            after.has_unpushed_edits,
            "an ambiguous push must leave the file dirty (#197 b5)"
        );
        assert_eq!(
            after.base_commit_sha, base_before,
            "an ambiguous push must not advance BASE (#197 b5)"
        );
    }

    #[test]
    #[ignore = "hits the live SpeleoDB box; needs DEV_SPELEODB_* + _EMAIL2/_PASSWORD2 (run with --ignored --test-threads=1)"]
    fn live_orch_admin_collaborators_teams_and_delete_unlink() {
        // [#215] The Project Settings admin surface end-to-end against the live box:
        // add/change/remove a collaborator (asserting the roster + is_self mapping),
        // add/remove a team (asserting the UUID -> display-name resolution), and
        // delete-the-project (asserting the backend unlinks locally). The offline
        // LoopbackStub can't reproduce a real project pull, so this is a live test,
        // like the rest of the orchestration suite. Needs the second account as the
        // collaborator target (the server rejects granting a non-existent user).
        use crate::speleodb::Permission;
        let ((url, owner_email, owner_pw), (email2, _pw2)) = (orch_env(), orch_env2());
        let owner = client::login(&url, false, &owner_email, &owner_pw).expect("owner login");
        let id = orch_seed(&url, &owner, "orch-admin");
        let _cleanup = OrchCleanup {
            url: url.clone(),
            token: owner.clone(),
            id: id.clone(),
        };

        let state = orch_state(&url, &owner_email, &owner);
        orch_open(&state, &id, "orch-admin");

        // Grant the collaborator READ_ONLY; the returned roster is server truth.
        let roster = state
            .speleodb_add_collaborator(email2.clone(), Permission::ReadOnly)
            .expect("add collaborator");
        let added = roster
            .users
            .iter()
            .find(|u| u.email.eq_ignore_ascii_case(&email2))
            .expect("the new collaborator appears in the roster");
        assert_eq!(added.permission, Permission::ReadOnly);
        assert!(!added.is_self, "the collaborator is not the signed-in user");
        // The owner's own row is flagged is_self (case-folded email match).
        let me = roster
            .users
            .iter()
            .find(|u| u.email.eq_ignore_ascii_case(&owner_email))
            .expect("the owner appears in the roster");
        assert!(me.is_self, "the owner's row is flagged is_self");

        // Change the level: PUT reflects in the refreshed roster.
        let roster = state
            .speleodb_set_collaborator(email2.clone(), Permission::ReadAndWrite)
            .expect("change collaborator level");
        assert_eq!(
            roster
                .users
                .iter()
                .find(|u| u.email.eq_ignore_ascii_case(&email2))
                .expect("collaborator still present")
                .permission,
            Permission::ReadAndWrite,
        );

        // Teams are optional (the owner may belong to none); when present, the
        // UUID -> name resolution must produce the picker's display name.
        if let Some(team) = state.speleodb_list_teams().expect("list teams").first() {
            let roster = state
                .speleodb_add_team(team.id.clone(), Permission::ReadOnly)
                .expect("add team");
            let grant = roster
                .teams
                .iter()
                .find(|g| g.team_id == team.id)
                .expect("the granted team appears in the roster");
            assert_eq!(
                grant.team_name, team.name,
                "the team UUID resolves to its display name, not the raw id"
            );
            let roster = state
                .speleodb_remove_team(team.id.clone())
                .expect("remove team");
            assert!(
                !roster.teams.iter().any(|g| g.team_id == team.id),
                "the team grant is gone after remove"
            );
        }

        // Remove the collaborator: the row is gone from the refreshed roster.
        let roster = state
            .speleodb_remove_collaborator(email2.clone())
            .expect("remove collaborator");
        assert!(
            !roster
                .users
                .iter()
                .any(|u| u.email.eq_ignore_ascii_case(&email2)),
            "the collaborator is gone after remove"
        );

        // Delete the project. The open project is still read-only (we never started
        // editing), so this also proves the unlink REOPENS the local edit gate.
        assert!(!state.edit_enabled(), "precondition: opened read-only");
        state
            .speleodb_delete_project()
            .expect("delete + local unlink");
        let link_after = state
            .database
            .lock()
            .as_ref()
            .expect("db still open")
            .load_speleo_link()
            .expect("load link");
        assert!(
            link_after.is_none(),
            "delete unlinks the project locally (no persisted speleo_link)"
        );
        assert!(
            state.edit_enabled(),
            "unlink reopens the local edit gate (now a plain local project)"
        );
    }

    // ---- [#576 A8] Start Editing's rules, driven or inventoried -------------------

    /// The fail-safe on the held-mutex split: a holder we cannot compare against (no
    /// email of our own - a concurrent logout dropped it under an in-flight call) must
    /// answer like a failed probe: no claim, keep only to protect work. `lock_held_by_me`
    /// answers FALSE for an unknown self-email, so without the `me.is_some()` guard that
    /// holder read as "someone else's lock we just took" and minted a record on it.
    ///
    /// [#576 A8] Was a source scan of the probe match inside Start editing; the four
    /// outcomes are `start_order::probe_verdict` now, driven here.
    #[test]
    fn the_probe_mints_a_record_only_for_a_transition_it_can_prove() {
        use super::start_order::{probe_verdict, ProbeVerdict};
        let verdict = |acquired_here, keep_lock_on_unwind| ProbeVerdict {
            acquired_here,
            keep_lock_on_unwind,
        };
        let held_by = |who: &str| Ok(Some(who.to_owned()));
        let unreadable = Err(SpeleoDbError::Status {
            status: 503,
            message: String::new(),
        });
        let cases = [
            (
                "unheld",
                probe_verdict(&Ok(None), Some("u@x"), true),
                verdict(true, false),
            ),
            (
                "our account (any case)",
                probe_verdict(&held_by("U@X"), Some("u@x"), false),
                verdict(false, true),
            ),
            (
                "someone else, then our acquire landed",
                probe_verdict(&held_by("other@x"), Some("u@x"), true),
                verdict(true, false),
            ),
            (
                "a holder and no email of our own, clean copy",
                probe_verdict(&held_by("u@x"), None, false),
                verdict(false, false),
            ),
            (
                "a holder and no email of our own, dirty copy",
                probe_verdict(&held_by("u@x"), None, true),
                verdict(false, true),
            ),
            (
                "an unreadable probe over un-pushed work",
                probe_verdict(&unreadable, Some("u@x"), true),
                verdict(false, true),
            ),
        ];
        for (case, got, want) in cases {
            println!("{case} -> {got:?}");
            assert_eq!(got, want, "{case}");
        }
    }

    /// The unwind's second keep rule: the failure being unwound decides too. The
    /// pre-acquire flag reads the OPEN project's link, but the fast-forward's `Protect`
    /// refuses over the CACHE copy - not necessarily the open file (Save-As carries the
    /// link forward), so P can be open and clean while the cache copy holds un-pushed
    /// work. Releasing there frees the mutex holding the project for edits that exist
    /// nowhere else; every other failure still hands a lock we took back.
    ///
    /// [#576 A8] The rule is `unwind_keeps_lock` now, driven here; which arms hand it
    /// their failure is the site inventory below.
    #[test]
    fn the_unwind_keeps_the_lock_for_a_refusal_over_local_work_and_nothing_else() {
        let refusal = SpeleoDbError::WouldDiscardLocalEdits {
            project_id: REMOTE_ID.to_owned(),
            local_path: "/cache/project.stalx".to_owned(),
            last_edit_at: None,
            unlinked: false,
            state_unknown: false,
        };
        // [#576 W53] The copy that could not be read may hold that work just as well.
        let unreadable = SpeleoDbError::WouldDiscardLocalEdits {
            project_id: REMOTE_ID.to_owned(),
            local_path: "/cache/project.stalx".to_owned(),
            last_edit_at: None,
            unlinked: false,
            state_unknown: true,
        };
        let corrupt = SpeleoDbError::Parse("not a zip".to_owned());
        let cases = [
            (
                "a verdict, nothing to keep",
                super::unwind_keeps_lock(false, None),
                false,
            ),
            (
                "a corrupt dump",
                super::unwind_keeps_lock(false, Some(&corrupt)),
                false,
            ),
            (
                "the cache copy refusing",
                super::unwind_keeps_lock(false, Some(&refusal)),
                true,
            ),
            (
                "the cache copy refusing as unreadable",
                super::unwind_keeps_lock(false, Some(&unreadable)),
                true,
            ),
            (
                "an inherited lock",
                super::unwind_keeps_lock(true, Some(&corrupt)),
                true,
            ),
        ];
        for (case, keeps, want) in cases {
            println!("{case} -> keeps the lock: {keeps}");
            assert_eq!(keeps, want, "{case}");
        }
    }

    /// A sanctioned site inventory (#576 A8-D2, docs/testing-recipes.md "Source scans"):
    /// every arm of Start editing that walks away on a FAILURE hands that failure to the
    /// unwind - the refusal is the only evidence that the copy being overwritten holds
    /// work the open project's link never did - and only the Diverged verdict, which
    /// read no local copy, unwinds with none. Which value an arm passes is a site rule
    /// no type states (both spellings are the same `Option`).
    #[test]
    fn every_failing_start_editing_arm_hands_its_failure_to_the_unwind() {
        use crate::models::source_scan::fn_body;
        let body = fn_body(prod_source(), "fn speleodb_start_editing_with(");
        assert_eq!(
            body.matches("release_unless_kept(Some(&e));").count(),
            4,
            "RULE (the unwind decides on the failure it unwinds): the server-head \
             failure, both commit refusals and the fast-forward's re-pull each hand their \
             error over"
        );
        assert_eq!(
            body.matches("release_unless_kept(None);").count(),
            1,
            "RULE (the unwind decides on the failure it unwinds): only the Diverged \
             verdict unwinds without one"
        );
        assert!(
            body.contains("self.speleodb_unwind_release(")
                && !body.contains("speleodb_release_best_effort("),
            "RULE (a Start editing unwind is session-aware, F7/F8): it releases only \
             through `speleodb_unwind_release`"
        );
    }

    /// A sanctioned site inventory (#576 A8-D2): a REMOTE edit session opens only
    /// through the identity-checked commit (`speleodb_commit_editing`, which takes the
    /// lock-ops span's proof). The other production sites that open the gate are
    /// exactly the ones that are not opening a lock-held session: force edit (no lock,
    /// by definition), unlink (the project stops being remote), and a revision
    /// restore's reopen (the user's own lock, re-enabled after the in-place reload).
    /// A new site - a bare reopen in Start editing's arms or the update's restore, say -
    /// fails here and has to justify itself.
    #[test]
    fn the_edit_gate_opens_only_through_the_guarded_commit() {
        assert_eq!(
            speleodb_prod_matches(".set_edit_enabled(true)", &[]),
            4,
            "RULE (a remote edit session opens only through the identity-checked commit): \
             expected the commit, force edit, unlink and the restore's reopen"
        );
    }

    /// [#576 RV8a-6] The proof types' `#[cfg(test)]` constructors (`for_tests`) exist
    /// for the tests of the steps a proof gates. Production reaching one would skip
    /// the step that earns the proof; a non-test build (cargo build, CI's clippy lib
    /// target) refuses it, but under `cargo test` production is compiled with
    /// `cfg(test)` too, so this site inventory is what fails there.
    #[test]
    fn no_production_code_reaches_a_test_only_proof_constructor() {
        assert_eq!(
            speleodb_prod_matches("::for_tests(", &[]),
            0,
            "RULE (a proof is earned, never minted, outside tests): a production region \
             calls a `for_tests` constructor"
        );
    }

    /// Every first-party Rust file of the workspace (`src-tauri/src` and every
    /// `src-tauri/crates/*/src`), as (path relative to `src-tauri`, contents).
    fn workspace_rust_files() -> Vec<(String, String)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, std::fs::read_to_string(&path).expect("read source")));
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut out = Vec::new();
        walk(&root.join("src"), &root, &mut out);
        let crates = std::fs::read_dir(root.join("crates")).expect("read crates/");
        for krate in crates.flatten() {
            walk(&krate.path().join("src"), &root, &mut out);
        }
        out.sort();
        out
    }

    /// The `(trait, type)` of every `impl <one of traits> for <type>` in `src`.
    ///
    /// [#576 RV10a-4] Read from the CODE (`source_scan::code_only`: comments and
    /// literals blanked), at every `impl` keyword wherever it stands on its line -
    /// after an attribute, say, where a line-start match never looked - up to the
    /// item's `{` or `;`, with the whitespace folded so a header broken over lines
    /// reads as one.
    fn licence_impls(src: &str, traits: &[&str]) -> Vec<(String, String)> {
        let code = crate::models::source_scan::code_only(src);
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';
        let mut out = Vec::new();
        for (at, keyword) in code.match_indices("impl") {
            let before = code[..at].chars().next_back();
            let after = code[at + keyword.len()..].chars().next();
            if before.is_some_and(is_ident) || !after.is_some_and(|c| c.is_whitespace() || c == '<')
            {
                continue;
            }
            let rest = &code[at..];
            let header = rest[..rest.find(['{', ';']).unwrap_or(rest.len())]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let Some((head, ty)) = header.split_once(" for ") else {
                continue;
            };
            // The trait is the head's last path segment: `impl<T> a::b::Trait`.
            let Some(licence) = traits.iter().find(|t| {
                head.strip_suffix(**t)
                    .is_some_and(|lead| lead.ends_with(|c: char| !is_ident(c)))
            }) else {
                continue;
            };
            let ty = ty.split_once(" where ").map_or(ty, |(ty, _)| ty).trim();
            out.push(((*licence).to_owned(), ty.to_owned()));
        }
        out
    }

    /// [#576 RV10a-4] The inventory's parser on the shapes an implementation can
    /// take in source - an attribute on the impl's own line (which hid
    /// `#[rustfmt::skip] impl client::PatchLicence for X {}` from a line-start
    /// match), a header broken over lines, no space before the trait, a `where`
    /// clause - and on text that is NOT one: a comment, a string, `impl Trait` in
    /// argument position, a bound, a longer trait name.
    ///
    /// (Indented: a column-0 `}` here would end `mod tests` for `source_scan`.)
    #[test]
    fn the_licence_inventory_parses_every_impl_shape() {
        let fixture = r#"
            impl client::PatchLicence for Plain {}
            #[rustfmt::skip] impl client::PatchLicence for SkipAttr {}
            #[cfg(any())] #[doc(hidden)] impl DumpLicence for TwoAttrs {}
            impl<'a> client::DumpLicence
                for Wrapped<'a>
            {
            }
            impl<T>UploadLicence for Tight<T> {}
            impl ReclaimRelease for Bounded where Bounded: Sized {}
            // impl client::PatchLicence for InAComment {}
            /* impl client::PatchLicence for InABlockComment {} */
            const S: &str = "impl client::PatchLicence for InAString {}";
            fn takes(_: impl client::DumpLicence) -> u8 { 0 }
            impl<T: client::DumpLicence> Unrelated for Generic<T> {}
            impl client::PatchLicenceExtra for NotALicence {}
        "#;
        let traits = [
            "DumpLicence",
            "UploadLicence",
            "PatchLicence",
            "ReclaimRelease",
        ];
        let rows: Vec<String> = licence_impls(fixture, &traits)
            .iter()
            .map(|(licence, ty)| format!("{licence} for {ty}"))
            .collect();
        println!("[RV10a-4] parsed: {rows:?}");
        assert_eq!(
            rows,
            [
                "PatchLicence for Plain",
                "PatchLicence for SkipAttr",
                "DumpLicence for TwoAttrs",
                "DumpLicence for Wrapped<'a>",
                "UploadLicence for Tight<T>",
                "ReclaimRelease for Bounded",
            ]
        );
    }

    /// [#576 RV9a-4] Who may implement a licence. A licence trait is a promise that
    /// only a proof (or a marker named for why its flow has no precondition) holds
    /// one, and `DumpLicence` / `UploadLicence` / `PatchLicence` are public traits of
    /// the speleodb crate: any crate that depends on it - the app crate does - could
    /// declare a two-line marker and call the guarded wire. A sealed trait cannot help
    /// here: it admits only its own crate, and the implementors are core's proof
    /// types. So every implementation in the workspace is inventoried, and a new one
    /// fails this until it is added - with its reason - to the list.
    ///
    /// [#576 RV10a-4] A tripwire, not a seal: it reads the source text
    /// ([`licence_impls`], pinned by `the_licence_inventory_parses_every_impl_shape`),
    /// so an impl a macro emits with the trait as a metavariable is not there to see.
    #[test]
    fn licence_implementations_are_exactly_the_sanctioned_set() {
        const TRAITS: [&str; 5] = [
            "DumpLicence",
            "UploadLicence",
            "PatchLicence",
            "ReplaceLicence",
            "ReclaimRelease",
        ];
        let mut found: Vec<String> = Vec::new();
        for (path, src) in workspace_rust_files() {
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            found.extend(
                licence_impls(&src, &TRAITS)
                    .into_iter()
                    .map(|(licence, ty)| format!("{name}: {licence} for {ty}")),
            );
        }
        found.sort();
        let mut sanctioned = vec![
            // The test licence (compiled into test builds only).
            "client.rs: DumpLicence for TestWire",
            "client.rs: UploadLicence for TestWire",
            "client.rs: PatchLicence for TestWire",
            "speleodb_cache.rs: ReplaceLicence for TestReplace",
            // Proofs: each made only by the step whose order it carries.
            "speleodb.rs: PatchLicence for Landed",
            "speleodb_adopt.rs: DumpLicence for AdoptLockGuard<'_>",
            "speleodb_adopt.rs: PatchLicence for AdoptLanded",
            "speleodb_pull.rs: DumpLicence for LocalEditsCleared",
            "speleodb_pull.rs: ReplaceLicence for LocalEditsCleared",
            "speleodb_pull.rs: ReplaceLicence for PinnedDump",
            "speleodb_push.rs: UploadLicence for PushSnapshot",
            "speleodb_revisions.rs: DumpLicence for UnwindCleared",
            "speleodb_revisions.rs: ReplaceLicence for ChildPolicyChecked",
            "speleodb_session.rs: ReclaimRelease for StoodDown",
            // Markers, each named for why its flow has no precondition.
            "speleodb_project.rs: PatchLicence for UserMetadataEdit",
            "speleodb_pull.rs: DumpLicence for FastForwardFetch",
            "speleodb_pull.rs: DumpLicence for LockFreeProbe",
            "speleodb_revisions.rs: DumpLicence for AffordanceProbe",
            "speleodb_session.rs: ReclaimRelease for CapturedRelease",
            "speleodb_session.rs: ReclaimRelease for SessionMovedUnwind",
            "speleodb_session.rs: ReclaimRelease for StrandedClaim",
        ];
        sanctioned.sort_unstable();
        // [#576 RV9a-4-D1] Name the rows that differ, and how to sanction one.
        let unsanctioned: Vec<&str> = found
            .iter()
            .map(String::as_str)
            .filter(|row| !sanctioned.contains(row))
            .collect();
        let gone: Vec<&str> = sanctioned
            .iter()
            .copied()
            .filter(|row| !found.iter().any(|f| f == row))
            .collect();
        assert!(
            found == sanctioned,
            "RULE (a licence is held only by a proof or a named no-precondition marker, \
             #576 RV9a-4): the licence implementations in the workspace changed.\n\
             Not sanctioned: {unsanctioned:#?}\nSanctioned but gone: {gone:#?}\n\
             To sanction a new licence, add its row exactly as printed above \
             (`<file>: <Trait> for <Type>`) to `sanctioned` in this test \
             (core/state/speleodb.rs, licence_implementations_are_exactly_the_sanctioned_set), \
             with a comment saying why it may skip the step the licence guards: under \
             \"Proofs\" when the type is made only by the step whose order the licence carries, \
             or under \"Markers\" when it is a marker named for why its flow has no \
             precondition. Anything else holding a licence - a convenience marker, a test \
             licence in production - is the bypass the trait exists to stop. Remove a row whose \
             implementation is gone. (A new licence TRAIT goes in TRAITS above.) The \
             inventory reads source text - a tripwire, not a seal: an impl generated by a \
             macro is not in it, so do not write one to get past this."
        );
    }

    /// [#576 RV9a-6] The test licences (`client::TestWire`, `TestReplace`) exist for the
    /// tests of the guarded calls. Under `cargo test` and `clippy --all-targets` feature
    /// unification compiles `TestWire` into core's library too, so a production call
    /// naming it compiled there (only a plain `cargo check` refused it). No production
    /// region in the workspace may name either, outside the line that defines it.
    #[test]
    fn no_production_code_names_a_test_licence() {
        // The protocol fuzzer (#576 fuzzers) is a directory of test-only files: this
        // module's parent declares it `#[cfg(test)] mod speleodb_fuzz;`. Its files are
        // skipped below only while that gate stands.
        let state_mod = include_str!("mod.rs");
        let fuzz_decl = state_mod
            .find("\nmod speleodb_fuzz;")
            .expect("state/mod.rs declares the fuzzer module");
        let above = state_mod.get(..fuzz_decl).unwrap_or_default();
        let gate = above.rfind("\n#[cfg(test)]");
        assert!(
            gate.is_some_and(|g| !above.get(g..).unwrap_or_default().contains("\nmod ")),
            "the fuzzer module is declared under #[cfg(test)], so its files are test-only"
        );
        let mut named: Vec<String> = Vec::new();
        for (path, src) in workspace_rust_files() {
            let name = path.rsplit('/').next().unwrap_or(&path);
            let test_only_file = path.contains("/tests/")
                || path.contains("/state/speleodb_fuzz/")
                || name == "tests.rs"
                || name.ends_with("_tests.rs")
                || name == "test_fixtures.rs"
                || name == "test_helpers.rs"
                || name == "tests_support.rs";
            if test_only_file {
                continue;
            }
            let end = crate::models::source_scan::test_module_start(&src).unwrap_or(src.len());
            let region = src.get(..end).unwrap_or(&src);
            for (n, line) in region.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                let defines = code.contains("struct TestWire")
                    || code.contains("for TestWire")
                    || code.contains("struct TestReplace");
                if !defines && (code.contains("TestWire") || code.contains("TestReplace")) {
                    named.push(format!("{path}:{}: {}", n + 1, line.trim()));
                }
            }
        }
        assert!(
            named.is_empty(),
            "RULE (a test licence never reaches production, #576 RV9a-6): {named:#?}"
        );
    }

    /// The poll's catch-all clear must not act on the `edit_enabled` snapshot taken at
    /// the top of `speleodb_link_status`: a fast-forward finishing underneath an
    /// in-flight poll reopens the gate and restores the record for a lock we still
    /// genuinely hold, and clearing on the stale snapshot erases it - re-raising the
    /// self-conflict this branch exists to fix. The helper's own contract is driven by
    /// `the_catch_all_clear_defers_to_a_gate_that_reopened_underneath_it`.
    ///
    /// A sanctioned site rule (#576 A8-D2): the window needs two threads interleaving
    /// at a point with no seam, and "the poll clears only through the gate-rechecking
    /// helper" is a statement about one function's calls no type expresses.
    #[test]
    fn the_poll_clears_the_record_through_the_gate_rechecking_helper() {
        use crate::models::source_scan::fn_body;
        let status = fn_body(prod_source(), "pub fn speleodb_link_status(");
        assert!(
            status.contains("self.speleodb_forget_lock_acquired_if_gate_shut();"),
            "RULE (the poll clears the record only through the gate-rechecking helper): \
             it re-reads the gate under the session lock"
        );
        assert!(
            !status.contains("self.speleodb_note_lock_acquired(None);"),
            "RULE (the poll clears the record only through the gate-rechecking helper): \
             an unconditional clear here erases a record a concurrent fast-forward just \
             restored"
        );
    }
}
