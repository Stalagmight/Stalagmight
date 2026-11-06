//! Status → action decision contract for the SpeleoDB client (#186).
//!
//! One pure function, [`map_status`], turns an `(endpoint, http_status)` pair
//! into a typed [`SpeleoDbAction`] the rest of the app (and the frontend, over
//! IPC) branches on - instead of re-parsing human-facing error strings. The
//! function **fails closed**: any status we do not explicitly model maps to
//! [`SpeleoDbAction::Fatal`] (surface it, keep the project dirty, never auto-drop
//! the token).
//!
//! INVARIANT (D-186): the action is a pure function of `(endpoint, status)`.
//! `client::parse_error_message` stays DISPLAY-only - **no** branch in
//! `map_status` reads the parsed message string. The `SpeleoDbError::Status`
//! variant stays `{status, message}` with no action field; the action is derived
//! here, at the IPC boundary, where the endpoint context is known, so the error
//! enum is not coupled to call-site context.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::models::{DumpCandidateKind, SpeleoErrorDto, UnpushedLocalEdits};
use super::SpeleoDbError;
use crate::models::CommandErrorCode;

/// The decision the caller should take after a SpeleoDB call, derived from the
/// HTTP status + the endpoint it came from. Crosses IPC as a ts-rs enum so the
/// frontend branches on `err.action` rather than on message text (D-B2). Order
/// is non-load-bearing (no `>=` comparisons - unlike [`super::Permission`]).
///
/// [#576 NF-AN3-4] Who reads which: the frontend branches on `ReLogin`, `Adopt`,
/// `UnpushedLocalEdits` and `Ambiguous` (`utils/speleoError.ts`) and shows the
/// message for the rest. `ReadOnly` / `Fence` and `StayDirty` are read by the
/// backend - the ambiguous-403 whoami probe (`speleodb_handle_auth`) and the
/// push's "did it land?" verdict (`speleodb_upload`) - so they are not dead
/// outputs; `Retry` is the transport/session-moved signal. Since the endpoint
/// table (`client::spec`) decides each row's category, these rows now hold by
/// construction, which is what a frontend that ever branches on them needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub enum SpeleoDbAction {
    /// The token is dead (revoked / invalid): a 401 on any authed endpoint, or a
    /// 403 on the `/user/` whoami chokepoint (the box's DRF 401→403 downgrade,
    /// verified live). Drives Track C's auto-drop (D-B4).
    ReLogin,
    /// 403 on a read-capable endpoint (list/get/download = read-only access).
    ReadOnly,
    /// 403 on a write endpoint (upload/acquire/release/PATCH/admin writes) - no
    /// write permission / lock fence.
    Fence,
    /// A run-once write surfaced without a confirmed server commit - keep dirty
    /// (g7).
    StayDirty,
    /// Upload-304 - the remote tree is already at this state (benign; the dirty
    /// bit is left unchanged, this is NOT an error).
    UpToDate,
    /// A retry-safe call's 5xx that the transport already retried + exhausted (or
    /// a transport failure); the FE may offer a manual retry.
    Retry,
    /// The opened project is an Ariane (`.tml`-only) project with no `.stalx`: the
    /// FE should offer the one-time C2 adoption flow (#214) instead of surfacing a
    /// hard error. NOT derived from an HTTP status - it is the local
    /// [`SpeleoDbError::AdoptAriane`] dump classification crossing the same typed
    /// action channel so the FE never branches on message text.
    Adopt,
    /// The re-pull was REFUSED because the local copy still holds edits the server
    /// has never seen. Like [`SpeleoDbAction::Adopt`] this is a routing signal, not
    /// a failure, and it is NOT derived from an HTTP status - it is the local
    /// [`SpeleoDbError::WouldDiscardLocalEdits`] guard crossing the same typed
    /// channel. The FE offers the binary choice described on
    /// [`super::models::UnpushedLocalEdits`], which the error carries.
    UnpushedLocalEdits,
    /// [#258] The dump held more than one openable file, so there is nothing to
    /// guess. A routing signal like [`SpeleoDbAction::Adopt`], not a failure, and
    /// not status-derived: the FE reads
    /// [`super::models::SpeleoErrorDto::dump_candidates`] and offers a picker, then
    /// re-calls the same command with the chosen `entryPath`.
    ///
    /// The paths that cannot resolve (the fast-forward verbs take no `entryPath`)
    /// still receive this action; having no candidates to act on, the FE falls back
    /// to showing the error's own message, which names the remedy.
    Ambiguous,
    /// Unmapped / 422 / 4xx we do not model - surface, fail closed.
    Fatal,
}

/// Why a transport attempt never reached (or never finished with) the server.
///
/// [#270] Derived from the TYPED `ureq::Error` / `io::ErrorKind` at the point of
/// failure (`client::classify_transport`), never from the error's Display text -
/// `io::Error`'s message comes from `strerror`, so it is OS- and locale-
/// dependent and unusable as a discriminant. Crosses IPC on
/// [`SpeleoErrorDto::transport`] so the frontend renders a translated,
/// actionable line ("could not reach SpeleoDB at <endpoint> …") instead of
/// surfacing a bare errno like "Invalid argument". The raw cause is kept in the
/// error's `detail` (and logged) so bug reports stay actionable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub enum TransportCause {
    /// The host answered the dial with a refusal / dropped the connection
    /// (nothing listening on that port, or the server closed on us).
    Refused,
    /// A configured timeout elapsed (dial, response, or body read).
    Timeout,
    /// The hostname could not be resolved.
    Dns,
    /// The TLS handshake / certificate validation failed.
    Tls,
    /// Anything else - no route, a blackholed address (the EINVAL this fix was
    /// found through), an unclassified transport failure. Fails to the generic
    /// "could not reach the server" line rather than showing the raw errno.
    Unknown,
}

/// The SpeleoDB endpoint a status came from, so [`map_status`] can apply the
/// per-endpoint 403 semantics (D-B4): a 403 on a read endpoint means read-only
/// access, a 403 on a write endpoint means a write fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Login,
    User,
    ListProjects,
    // GetProject/Acquire/Release/UploadAuto/CreateProject are constructed by the
    // not-yet-wired orchestration callers (#202/#194/#207) and by the unit tests;
    // they are dead in the non-test lib build until those callers land.
    // [P1] #197 maps get_project / get_revisions errors via GetProject.
    GetProject,
    DownloadDump,
    // [P1] #194 start_editing constructs Acquire (auth handler + dto_for_error).
    Acquire,
    // [P1] #197 done maps release errors via Release.
    Release,
    // [P1] #197 done maps upload errors via UploadAuto.
    UploadAuto,
    // [P1] #207 speleodb_publish constructs CreateProject (auth handler + dto_for_error).
    CreateProject,
    // [#215] Collaborator/team permission writes + the metadata PATCH (#576 B5:
    // the PATCH used to be mapped as GetProject, a read). A write (not read, not
    // idempotent): a 403 is a Fence (no admin/write grant, not read-only access),
    // a 5xx is StayDirty (never auto-retried - these are run-once POST/PUT/PATCH/
    // DELETE). Falls through the `403 => Fence` / `500..=599 => StayDirty` arms of
    // `map_status` by being absent from both `is_read` and `is_idempotent`.
    ManageProject,
    // [#576 B5] `DELETE user/release_all_locks/`: a write whose repetition is a
    // no-op, so it IS retried - and its exhausted 5xx says so (Retry), where the
    // `Release` row it used to be mapped through said "never retried".
    ReleaseAll,
    // [#576 B5-D2] `DELETE projects/{id}/`: idempotent and retried (a 404 is
    // "already deleted" in the client). A 403 is a Fence; an exhausted 5xx, Retry.
    DeleteProject,
}

impl Endpoint {
    /// Idempotent endpoints are retried on a transient 5xx (D-B3), so their
    /// exhausted 5xx reads `Retry`; the run-once writes never auto-retry
    /// (double-submit hazard) and surface for g7 as `StayDirty`. Each row of the
    /// client's endpoint table must agree with this (`client::spec`, pinned by
    /// `every_endpoint_row_agrees_with_its_contract_row`).
    pub(crate) fn is_idempotent(self) -> bool {
        matches!(
            self,
            Endpoint::Login
                | Endpoint::User
                | Endpoint::ListProjects
                | Endpoint::GetProject
                | Endpoint::DownloadDump
                | Endpoint::ReleaseAll
                | Endpoint::DeleteProject
        )
    }

    /// `true` for the read-capable endpoints whose 403 means "read-only access"
    /// rather than a write fence.
    pub(crate) fn is_read(self) -> bool {
        matches!(
            self,
            Endpoint::ListProjects | Endpoint::GetProject | Endpoint::DownloadDump
        )
    }
}

/// Map an `(endpoint, status)` pair to the action the caller should take.
///
/// Pure + fail-closed: an unmodelled status yields [`SpeleoDbAction::Fatal`].
/// Per the ledger (D-186): no first-class 423/409/412/429 rows (the server does
/// not use them - lock contention is acquire-**400**, handled by #194), so they
/// fall through to the Fatal default; 429/Retry-After is intentionally NOT a
/// `Wait` action (there is no `Wait` in this issue).
pub fn map_status(endpoint: Endpoint, status: u16) -> SpeleoDbAction {
    match status {
        // 401 = real token revocation on every authenticated endpoint. Login is
        // the exception: bad creds come back as 400 (verified), so a 401 cannot
        // arise from Login and the catch-all below would treat it as Fatal - but
        // we still special-case Login here so it is explicit and 401 never maps
        // to ReLogin for the unauthenticated login call.
        401 if !matches!(endpoint, Endpoint::Login) => SpeleoDbAction::ReLogin,
        // 403 on the whoami (User) chokepoint = the unambiguous dead-token signal:
        // the box returns 200 for any valid token and 403 for a dead one (the DRF
        // 401→403 downgrade, verified live), and `/user/` carries no per-resource
        // permission, so a 403 there can ONLY mean the token is bad → ReLogin.
        403 if matches!(endpoint, Endpoint::User) => SpeleoDbAction::ReLogin,
        // 403 on a resource endpoint is ambiguous (a real read-only/fence grant vs
        // a dead token) and NEVER drops the token here (D-B4); the orchestration
        // disambiguates it with a whoami probe (`speleodb_token_is_dead`).
        403 if endpoint.is_read() => SpeleoDbAction::ReadOnly,
        403 => SpeleoDbAction::Fence, // UploadAuto / Acquire / Release / CreateProject / Login
        // Upload-304: the remote tree already matches - benign, dirty bit
        // unchanged. Without this row 304 would fall to the Fatal default.
        304 if matches!(endpoint, Endpoint::UploadAuto) => SpeleoDbAction::UpToDate,
        // 422: defensive unprocessable fatal row. NOTE (verified live): the box
        // answers a malformed/missing-artifact upload with 400, NOT 422 - that 400
        // has no first-class row and correctly falls to the fail-closed default
        // below. 422 is kept as a defensive mapping in case the server ever uses it.
        422 => SpeleoDbAction::Fatal,
        // 5xx: idempotent reads were already retried → surface the exhausted
        // Retry state; writes never auto-retried → StayDirty (keep dirty, g7).
        500..=599 if endpoint.is_idempotent() => SpeleoDbAction::Retry,
        500..=599 => SpeleoDbAction::StayDirty,
        // Everything else (incl. acquire-400 lock-held, 409/412/423/429, unknown
        // 4xx, and any 2xx that wrongly reaches here) fails closed.
        _ => SpeleoDbAction::Fatal,
    }
}

/// [#576 C4/A6] The IPC routing code for `action`: one `SPELEODB_*` code per
/// action, so a `CommandError` built from a [`SpeleoErrorDto`] says what the old
/// `action` field said (which is no longer sent).
pub fn code_for_action(action: SpeleoDbAction) -> CommandErrorCode {
    match action {
        SpeleoDbAction::ReLogin => CommandErrorCode::SpeleodbRelogin,
        SpeleoDbAction::ReadOnly => CommandErrorCode::SpeleodbReadOnly,
        SpeleoDbAction::Fence => CommandErrorCode::SpeleodbFence,
        SpeleoDbAction::StayDirty => CommandErrorCode::SpeleodbStayDirty,
        SpeleoDbAction::UpToDate => CommandErrorCode::SpeleodbUpToDate,
        SpeleoDbAction::Retry => CommandErrorCode::SpeleodbRetry,
        SpeleoDbAction::Adopt => CommandErrorCode::SpeleodbAdopt,
        SpeleoDbAction::UnpushedLocalEdits => CommandErrorCode::SpeleodbUnpushedLocalEdits,
        SpeleoDbAction::Ambiguous => CommandErrorCode::SpeleodbAmbiguous,
        SpeleoDbAction::Fatal => CommandErrorCode::SpeleodbFatal,
    }
}

/// Convert a [`SpeleoDbError`] into the IPC [`SpeleoErrorDto`], deriving the
/// typed action from the endpoint the call came from (D-B2). This is the single
/// conversion the IPC boundary uses; it needs the `endpoint` context, so it is a
/// named fn called at the call site (not a blanket `From`).
///
/// - `Status { status, message }` → `map_status(endpoint, status)`, `status: Some`.
/// - `Transport` (network/timeout, post-retry) → `Retry`, `status: None`, and
///   the typed [`TransportCause`] on `transport` (#270) so the FE can render a
///   translated "could not reach <endpoint>" line instead of the raw errno.
/// - `NotAuthenticated` → `ReLogin` (no usable token).
/// - `InsecureEndpoint` and every other local error → `Fatal`, `status: None`.
pub fn dto_for_error(endpoint: Endpoint, err: &SpeleoDbError) -> SpeleoErrorDto {
    // Only a Transport error carries a cause; every other arm leaves it None,
    // which is what makes `transport` a sound discriminant on the FE side.
    let transport = match err {
        SpeleoDbError::Transport { cause, .. } => Some(*cause),
        _ => None,
    };
    // Likewise the refused-replace payload: only the guard's own error carries it,
    // so `unpushed_local` stays a sound discriminant on the FE side.
    let unpushed_local = match err {
        SpeleoDbError::WouldDiscardLocalEdits {
            project_id,
            local_path,
            last_edit_at,
            unlinked,
        } => Some(UnpushedLocalEdits {
            project_id: project_id.clone(),
            local_path: local_path.clone(),
            last_edit_at: last_edit_at.clone(),
            unlinked: *unlinked,
        }),
        _ => None,
    };
    // [#258] Same discipline as `unpushed_local`: only the two ambiguity errors
    // carry candidates, so the field stays a sound discriminant on the FE side.
    // Built from the SAME match so the pair cannot drift: a kind without candidates
    // (or the reverse) would be a picker with nothing to show, or one that has to
    // guess which verb it is offering.
    let (dump_candidates, dump_candidate_kind) = match err {
        SpeleoDbError::AmbiguousDump(candidates) => {
            (Some(candidates.clone()), Some(DumpCandidateKind::Stalx))
        }
        SpeleoDbError::AmbiguousAriane(candidates) => {
            (Some(candidates.clone()), Some(DumpCandidateKind::Ariane))
        }
        _ => (None, None),
    };
    // [#576 A8] Exhaustive, and kept so by the compiler: a wildcard arm here is what
    // silently routes the NEXT routing variant (like `AdoptAriane` -> Adopt or
    // `WouldDiscardLocalEdits` -> UnpushedLocalEdits) to Fatal, so the lint denies one.
    // A new `SpeleoDbError` variant is then a compile error until someone decides
    // its action.
    #[deny(clippy::wildcard_enum_match_arm)]
    let (action, status, message) = match err {
        // Status: carry the parsed human detail WITHOUT the "SpeleoDB returned HTTP
        // N:" wrapper - the frontend frames it per operation ("Login to SpeleoDB
        // failed: …", "Unable to open the project: …"). See utils/speleoError.ts.
        SpeleoDbError::Status { status, message } => (
            map_status(endpoint, *status),
            Some(*status),
            message.clone(),
        ),
        // Transport: the Display keeps the RAW cause ("Network error talking to
        // SpeleoDB: Invalid argument") - that string is the diagnostic/log
        // surface. The FE renders `transport` instead of it (#270).
        SpeleoDbError::Transport { .. } => (SpeleoDbAction::Retry, None, err.to_string()),
        SpeleoDbError::NotAuthenticated => (SpeleoDbAction::ReLogin, None, err.to_string()),
        // Ariane (`.tml`-only) project: not a failure, a routing signal. The FE
        // offers the one-time C2 adoption flow (#214) on `action == Adopt` rather
        // than toasting the message.
        SpeleoDbError::AdoptAriane => (SpeleoDbAction::Adopt, None, err.to_string()),
        // A refused re-pull: also a routing signal, not a failure. The FE reads
        // `unpushed_local` (built above) and offers keep-local vs take-remote.
        SpeleoDbError::WouldDiscardLocalEdits { .. } => {
            (SpeleoDbAction::UnpushedLocalEdits, None, err.to_string())
        }
        // [#258] More than one openable file: also a routing signal. The FE reads
        // `dump_candidates` (built above) and offers the choice. Where it cannot
        // resolve, the Display already names the remedy, so this is never worse
        // than the `Fatal` it replaces.
        SpeleoDbError::AmbiguousDump(_) | SpeleoDbError::AmbiguousAriane(_) => {
            (SpeleoDbAction::Ambiguous, None, err.to_string())
        }
        // The session-moved refusal from Start Editing's commit guard: the world it
        // was started in is gone (delete / close / switch / logout landed inside
        // its network window) and nothing was committed. Retryable by definition -
        // a retry runs in the successor world and does the right thing there.
        SpeleoDbError::SessionMoved => (SpeleoDbAction::Retry, None, err.to_string()),
        // [#576 B35(e)] Done found the write lock is not ours: the lock fence, decided
        // locally from the holder read rather than earned as a server 403.
        SpeleoDbError::LockNotHeld { .. } => (SpeleoDbAction::Fence, None, err.to_string()),
        // Everything below fails closed; each Display is already user-facing.
        //
        // ENUMERATED, not a `_` catch-all. Two of the arms above (`AdoptAriane`,
        // `WouldDiscardLocalEdits`) exist precisely because a ROUTING signal must
        // not reach the user as `Fatal` - and a wildcard here is what silently
        // routes the next one wrongly: a new variant compiles clean and lands in
        // `Fatal`, so the frontend toasts a dead end instead of offering the choice
        // the variant was added to offer. Listing them makes adding a variant fail
        // the build until someone decides which side of that line it is on.
        SpeleoDbError::NoEndpoint
        | SpeleoDbError::MissingCredentials
        | SpeleoDbError::Parse(_)
        | SpeleoDbError::EmptyDump
        | SpeleoDbError::Io(_)
        | SpeleoDbError::Refused { .. }
        | SpeleoDbError::InvalidProjectId
        | SpeleoDbError::InvalidRevision
        | SpeleoDbError::InsecureEndpoint { .. }
        | SpeleoDbError::NoRemoteProject => (SpeleoDbAction::Fatal, None, err.to_string()),
    };
    // [#576 C4] A local refusal carries the refusing gate's own code (the edit
    // gate, a refused save, ...); `action` stays `Fatal` for the backend's own
    // branches, and every other error's code is its action's.
    let code = match err {
        SpeleoDbError::Refused { code, .. } => *code,
        _ => code_for_action(action),
    };
    SpeleoErrorDto {
        code,
        message,
        action,
        status,
        transport,
        unpushed_local,
        dump_candidates,
        dump_candidate_kind,
    }
}

#[cfg(test)]
mod tests {
    use super::{dto_for_error, map_status, Endpoint, SpeleoDbAction, TransportCause};
    use crate::speleodb::SpeleoDbError;

    #[test]
    fn map_status_per_row() {
        // The load-bearing rows from the ledger's #186 table.
        assert_eq!(map_status(Endpoint::Login, 400), SpeleoDbAction::Fatal);
        assert_eq!(map_status(Endpoint::User, 401), SpeleoDbAction::ReLogin);
        // whoami 403 = dead-token signal (the live box's DRF 401→403 downgrade).
        assert_eq!(map_status(Endpoint::User, 403), SpeleoDbAction::ReLogin);
        assert_eq!(
            map_status(Endpoint::ListProjects, 403),
            SpeleoDbAction::ReadOnly
        );
        assert_eq!(
            map_status(Endpoint::GetProject, 403),
            SpeleoDbAction::ReadOnly
        );
        assert_eq!(
            map_status(Endpoint::DownloadDump, 403),
            SpeleoDbAction::ReadOnly
        );
        // acquire-400 (lock held) is NOT a first-class row → Fatal (deferred to #194).
        assert_eq!(map_status(Endpoint::Acquire, 400), SpeleoDbAction::Fatal);
        assert_eq!(map_status(Endpoint::Acquire, 403), SpeleoDbAction::Fence);
        assert_eq!(map_status(Endpoint::UploadAuto, 403), SpeleoDbAction::Fence);
        // [#215] The admin writes (collaborator/team perms + delete) are write-shaped:
        // 403 = Fence (no write/admin grant), 5xx = StayDirty (non-idempotent, never
        // auto-retried) - NOT ReadOnly/Retry (which GetProject, a read, would give).
        assert_eq!(
            map_status(Endpoint::ManageProject, 403),
            SpeleoDbAction::Fence
        );
        assert_eq!(
            map_status(Endpoint::ManageProject, 503),
            SpeleoDbAction::StayDirty
        );
        assert_eq!(
            map_status(Endpoint::UploadAuto, 304),
            SpeleoDbAction::UpToDate
        );
        assert_eq!(map_status(Endpoint::UploadAuto, 422), SpeleoDbAction::Fatal);
        // Verified live: a malformed / missing-artifact upload is 400, which has
        // no first-class row and must fail closed to Fatal.
        assert_eq!(map_status(Endpoint::UploadAuto, 400), SpeleoDbAction::Fatal);
        assert_eq!(
            map_status(Endpoint::UploadAuto, 503),
            SpeleoDbAction::StayDirty
        );
    }

    #[test]
    fn map_status_unmapped_fails_closed() {
        // Any status we don't model maps to Fatal, on any endpoint.
        for status in [418, 451, 409, 423, 412, 429] {
            assert_eq!(
                map_status(Endpoint::ListProjects, status),
                SpeleoDbAction::Fatal,
                "status {status} on a read endpoint should fail closed"
            );
        }
        // 304 is only meaningful on UploadAuto; on a read endpoint it is Fatal.
        assert_eq!(
            map_status(Endpoint::ListProjects, 304),
            SpeleoDbAction::Fatal
        );
    }

    #[test]
    fn map_status_422_is_distinct_fatal() {
        // 422 is Fatal on every endpoint (no retry, no dirty change).
        for ep in [
            Endpoint::UploadAuto,
            Endpoint::Acquire,
            Endpoint::ListProjects,
            Endpoint::CreateProject,
        ] {
            assert_eq!(map_status(ep, 422), SpeleoDbAction::Fatal);
        }
    }

    #[test]
    fn map_status_upload_304_is_uptodate() {
        assert_eq!(
            map_status(Endpoint::UploadAuto, 304),
            SpeleoDbAction::UpToDate
        );
        // 304 elsewhere is NOT UpToDate.
        assert_ne!(
            map_status(Endpoint::DownloadDump, 304),
            SpeleoDbAction::UpToDate
        );
    }

    #[test]
    fn map_status_idempotent_5xx_retry() {
        for ep in [
            Endpoint::Login,
            Endpoint::User,
            Endpoint::ListProjects,
            Endpoint::GetProject,
            Endpoint::DownloadDump,
        ] {
            for status in [500, 502, 503, 599] {
                assert_eq!(
                    map_status(ep, status),
                    SpeleoDbAction::Retry,
                    "{ep:?} {status} should be Retry"
                );
            }
        }
    }

    #[test]
    fn map_status_write_5xx_stay_dirty() {
        for ep in [
            Endpoint::UploadAuto,
            Endpoint::Acquire,
            Endpoint::Release,
            Endpoint::CreateProject,
            Endpoint::ManageProject,
        ] {
            for status in [500, 502, 503] {
                assert_eq!(
                    map_status(ep, status),
                    SpeleoDbAction::StayDirty,
                    "{ep:?} {status} should be StayDirty"
                );
            }
        }
    }

    #[test]
    fn map_status_whoami_403_is_relogin_but_resource_403_is_not() {
        // Verified live: `/user/` returns 403 for a dead token, so a whoami 403 is
        // the unambiguous dead-token signal → ReLogin. The SAME 403 on a resource
        // endpoint stays ReadOnly (read) / Fence (write) - it must NOT map to
        // ReLogin, or a read-only user would be logged out (D-B4); the
        // orchestration disambiguates a resource 403 with a whoami probe.
        assert_eq!(map_status(Endpoint::User, 403), SpeleoDbAction::ReLogin);
        assert_eq!(
            map_status(Endpoint::ListProjects, 403),
            SpeleoDbAction::ReadOnly
        );
        assert_eq!(
            map_status(Endpoint::DownloadDump, 403),
            SpeleoDbAction::ReadOnly
        );
        assert_eq!(map_status(Endpoint::UploadAuto, 403), SpeleoDbAction::Fence);
        assert_eq!(map_status(Endpoint::Acquire, 403), SpeleoDbAction::Fence);
    }

    #[test]
    fn map_status_login_401_is_not_relogin() {
        // Login is unauthenticated - a 401 there (which the live box never sends,
        // it 400s on bad creds) must not be ReLogin.
        assert_ne!(map_status(Endpoint::Login, 401), SpeleoDbAction::ReLogin);
        assert_eq!(map_status(Endpoint::Login, 401), SpeleoDbAction::Fatal);
    }

    #[test]
    fn speleo_error_dto_maps_status_to_action() {
        // A 401 Status on an authed endpoint → ReLogin, status carried through.
        let dto = dto_for_error(
            Endpoint::User,
            &SpeleoDbError::Status {
                status: 401,
                message: "invalid".to_owned(),
            },
        );
        assert_eq!(dto.action, SpeleoDbAction::ReLogin);
        assert_eq!(dto.status, Some(401));

        // A 403 on a read endpoint → ReadOnly.
        let dto = dto_for_error(
            Endpoint::ListProjects,
            &SpeleoDbError::Status {
                status: 403,
                message: "forbidden".to_owned(),
            },
        );
        assert_eq!(dto.action, SpeleoDbAction::ReadOnly);
        assert_eq!(dto.status, Some(403));

        // Transport → Retry, no status.
        let dto = dto_for_error(
            Endpoint::DownloadDump,
            &SpeleoDbError::Transport {
                cause: TransportCause::Timeout,
                detail: "timed out".to_owned(),
            },
        );
        assert_eq!(dto.action, SpeleoDbAction::Retry);
        assert_eq!(dto.status, None);
        // [#270] and it carries the typed cause, so the FE renders a translated
        // "could not reach <endpoint>" line instead of the raw errno in `message`.
        assert_eq!(dto.transport, Some(TransportCause::Timeout));
        assert!(
            dto.message.contains("timed out"),
            "the raw cause stays on the message for the log: {}",
            dto.message
        );

        // Every non-transport error leaves `transport` unset - that is what makes
        // it a sound discriminant on the frontend side.
        for err in [
            SpeleoDbError::Status {
                status: 500,
                message: "boom".to_owned(),
            },
            SpeleoDbError::NotAuthenticated,
            SpeleoDbError::AdoptAriane,
            SpeleoDbError::NoRemoteProject,
        ] {
            assert_eq!(
                dto_for_error(Endpoint::UploadAuto, &err).transport,
                None,
                "{err:?} must not look like a transport failure"
            );
        }

        // NotAuthenticated → ReLogin; InsecureEndpoint / local → Fatal.
        assert_eq!(
            dto_for_error(Endpoint::User, &SpeleoDbError::NotAuthenticated).action,
            SpeleoDbAction::ReLogin
        );
        assert_eq!(
            dto_for_error(
                Endpoint::Login,
                &SpeleoDbError::InsecureEndpoint {
                    endpoint: "http://x".to_owned()
                }
            )
            .action,
            SpeleoDbAction::Fatal
        );
        assert_eq!(
            dto_for_error(Endpoint::User, &SpeleoDbError::InvalidProjectId).action,
            SpeleoDbAction::Fatal
        );
        // AdoptAriane is a routing signal, not a failure: it must map to the typed
        // `Adopt` action (never `Fatal`) so the FE offers the C2 adoption flow (#214)
        // instead of toasting the message. Carries no HTTP status.
        let adopt = dto_for_error(Endpoint::DownloadDump, &SpeleoDbError::AdoptAriane);
        assert_eq!(adopt.action, SpeleoDbAction::Adopt);
        assert_eq!(adopt.status, None);
    }

    /// [#576 C4] A verb refused by a core refusal reaches the frontend with that
    /// refusal's code (the edit gate's, a refused save's, ...) instead of
    /// `SPELEODB_FATAL`; a plain local refusal stays `SPELEODB_FATAL`. The backend's
    /// own branches still see a local failure (`action == Fatal`) either way.
    #[test]
    fn a_local_refusal_carries_the_refusing_gates_code() {
        use crate::models::CommandErrorCode;
        let plain = dto_for_error(Endpoint::User, &SpeleoDbError::refused("not linked"));
        assert_eq!(plain.action, SpeleoDbAction::Fatal);
        assert_eq!(plain.code, CommandErrorCode::SpeleodbFatal);
        assert_eq!(plain.message, "not linked");

        let gated = dto_for_error(
            Endpoint::User,
            &SpeleoDbError::refused_with(
                CommandErrorCode::WorkspaceReadOnly,
                "Discard changes is disabled",
            ),
        );
        assert_eq!(gated.action, SpeleoDbAction::Fatal);
        assert_eq!(gated.code, CommandErrorCode::WorkspaceReadOnly);
        assert_eq!(
            gated.message, "Discard changes is disabled",
            "shown as written"
        );
    }

    #[test]
    fn speleo_action_ts_export() {
        // ts-rs export must not panic and must name the enum the FE imports.
        use ts_rs::TS;
        let decl = SpeleoDbAction::decl();
        assert!(
            decl.contains("SpeleoDbAction"),
            "ts-rs decl should name the enum: {decl}"
        );
    }
}
