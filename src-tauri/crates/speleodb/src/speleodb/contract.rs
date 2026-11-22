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
//! enum is not coupled to call-site context. The one reader of the message is
//! [`upload_refusal`], which tells a server that does not take version 2 from
//! a refusal with the server's own reason (review finding O3); it picks the
//! error, not the action.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::models::{SpeleoErrorDto, UnpushedLocalEdits};
use super::SpeleoDbError;

/// The decision the caller should take after a SpeleoDB call, derived from the
/// HTTP status + the endpoint it came from. Crosses IPC as a ts-rs enum so the
/// frontend branches on `err.action` rather than on message text (D-B2). Order
/// is non-load-bearing (no `>=` comparisons - unlike [`super::Permission`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub enum SpeleoDbAction {
    /// The token is dead (revoked / invalid): a 401 on any authed endpoint, or a
    /// 403 on the `/user/` whoami chokepoint (the box's DRF 401→403 downgrade,
    /// verified live). Drives Track C's auto-drop (D-B4).
    ReLogin,
    /// 403 on a read-capable endpoint (list/get/download = read-only access).
    ReadOnly,
    /// 403 on upload/acquire/release - no write permission / lock fence.
    Fence,
    /// A write surfaced without a confirmed server commit - keep dirty (g7).
    StayDirty,
    /// Upload-304 - the remote tree is already at this state (benign; the dirty
    /// bit is left unchanged, this is NOT an error).
    UpToDate,
    /// An idempotent 5xx that the transport already retried + exhausted; the FE
    /// may offer a manual retry.
    Retry,
    /// The re-pull was REFUSED because the local copy still holds edits the server
    /// has never seen. Like [`SpeleoDbAction::Adopt`] this is a routing signal, not
    /// a failure, and it is NOT derived from an HTTP status - it is the local
    /// [`SpeleoDbError::WouldDiscardLocalEdits`] guard crossing the same typed
    /// channel. The FE offers the binary choice described on
    /// [`super::models::UnpushedLocalEdits`], which the error carries.
    UnpushedLocalEdits,
    /// Unmapped / 422 / 4xx we do not model - surface, fail closed.
    Fatal,
}

/// Why a transport attempt never reached (or never finished with) the server.
///
/// [#270] Derived from the TYPED error tree at the point of failure
/// (`client::transport_cause`, the one classifier of the SDK's send and body
/// failures: `reqwest::Error`, `hyper::Error`, `io::ErrorKind`, `rustls::Error`;
/// and `tls::enforce` for an unresolvable `http://` host), never from the
/// error's Display text - `io::Error`'s message comes from `strerror`, so it is
/// OS- and locale-dependent and unusable as a discriminant. Crosses IPC on
/// [`SpeleoErrorDto::transport`] so the frontend renders a translated,
/// actionable line ("could not reach SpeleoDB at <endpoint> …") instead of
/// surfacing a bare errno like "Invalid argument". The raw cause is kept in the
/// error's `detail` (and logged) so bug reports stay actionable.
///
/// What each cause says about the request (F1): after [`Self::Refused`] or
/// [`Self::Dns`] nothing reached the server; after [`Self::Timeout`],
/// [`Self::Dropped`], [`Self::Tls`] (a TLS error after the handshake can follow
/// a request already sent) or [`Self::Unknown`] the request may have reached it.
/// So
/// every WRITE (upload, lock acquire and release, publish, project create)
/// treats every transport failure as an outcome it does not know, whatever the
/// cause: an upload stays unconfirmed and dirty, an acquire asks the server who
/// holds the lock. A READ that failed with a cause [`Self::got_no_reply`] names
/// got no complete reply, so it learnt nothing from the server, and an open then
/// opens the cached copy offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub enum TransportCause {
    /// The connection could not be made: the connector failed, refused, the
    /// network or host unreachable, or any other failure to connect (a
    /// blackholed address's EINVAL, the case #270 was found through). Nothing
    /// was sent.
    Refused,
    /// The connection was made, then closed, reset or reported unreachable
    /// before the whole reply arrived: closed before any reply, a reply head cut
    /// short, or a body shorter than its length (F1). The request may have
    /// reached the server. A JSON reply whose body was cut short is this whatever
    /// its status said, since the SDK reads the body before the status (a
    /// download's refusal cut short is [`Self::Unknown`]: the SDK reads that
    /// body only for a status that is not 200).
    Dropped,
    /// A configured timeout elapsed (dial, response, or body read).
    Timeout,
    /// The hostname could not be resolved.
    Dns,
    /// The TLS handshake / certificate validation failed, or a TLS error ended
    /// the connection after it.
    Tls,
    /// Anything else: an unclassified transport failure, such as a reply that is
    /// not HTTP at all. Fails to the generic "could not reach the server" line
    /// rather than showing the raw error.
    Unknown,
}

impl TransportCause {
    /// Whether a request that failed this way got no complete reply from the
    /// server: the connection could not be made, the name did not resolve, the
    /// time ran out, or the connection closed or reset before the whole reply
    /// arrived. A read without a reply learnt nothing about the project, so an
    /// open may fall back to the cached copy (`offline_open_result`).
    ///
    /// Not [`Self::Tls`]: the peer did not prove it is the server, which the user
    /// has to see and fix. Not [`Self::Unknown`]: an unclassified failure fails
    /// closed. A write never asks this: its outcome is unknown after every
    /// transport failure (see the type).
    #[must_use]
    pub const fn got_no_reply(self) -> bool {
        matches!(
            self,
            Self::Refused | Self::Dropped | Self::Timeout | Self::Dns
        )
    }
}

/// Why the endpoint policy (`tls::validate_endpoint`) refused a server address
/// before any request was made. Crosses IPC on
/// [`SpeleoErrorDto::endpoint_refusal`] so the frontend says what to change in a
/// translated line instead of the backend's English sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub enum EndpointRefusal {
    /// A plain `http://` server outside the automatic exceptions (`localhost`,
    /// `127.0.0.1`, `192.168.x.x`; see `tls`), without the development-build
    /// insecure-HTTP override.
    PlainHttp,
    /// Not a usable server address: no or an unknown scheme, no host, user info,
    /// or an invalid port.
    InvalidAddress,
}

impl EndpointRefusal {
    /// The rule, in English, for the error's Display text (logs, and callers that
    /// show the backend message as is).
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::PlainHttp => super::tls::PLAIN_HTTP_RULE,
            Self::InvalidAddress => {
                "Not a usable SpeleoDB address: it needs an http:// or https:// scheme, a host and a valid port."
            }
        }
    }
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
    DownloadStalx,
    // [P1] #194 start_editing constructs Acquire (auth handler + dto_for_error).
    Acquire,
    // [P1] #197 done maps release errors via Release.
    Release,
    // [P1] #197 done maps upload errors via UploadAuto.
    UploadStalx,
    // [P1] #207 speleodb_publish constructs CreateProject (auth handler + dto_for_error).
    CreateProject,
    // [#215] Collaborator/team permission writes + project delete. A write (not
    // read, not idempotent): a 403 is a Fence (no admin/write grant, not read-only
    // access), a 5xx is StayDirty (never auto-retried - these are non-idempotent
    // POST/PUT/DELETE). Falls through the `403 => Fence` / `500..=599 => StayDirty`
    // arms of `map_status` by being absent from both `is_read` and `is_idempotent`.
    ManageProject,
}

impl Endpoint {
    /// Idempotent (read-like) endpoints retry a transient 5xx (D-B3); the write
    /// endpoints never auto-retry (double-submit hazard) and surface for g7.
    fn is_idempotent(self) -> bool {
        matches!(
            self,
            Endpoint::Login
                | Endpoint::User
                | Endpoint::ListProjects
                | Endpoint::GetProject
                | Endpoint::DownloadStalx
        )
    }

    /// `true` for the read-capable endpoints whose 403 means "read-only access"
    /// rather than a write fence.
    fn is_read(self) -> bool {
        matches!(
            self,
            Endpoint::ListProjects | Endpoint::GetProject | Endpoint::DownloadStalx
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
        304 if matches!(endpoint, Endpoint::UploadStalx) => SpeleoDbAction::UpToDate,
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

/// What a refused upload of a version 2 file says about the format (plan 12.1).
///
/// A server that does not take version 2 yet refuses this build's every push,
/// and once servers take it, a refusal is the server's verdict on this file.
/// The two read the same from the status alone (both are a 400 from SpeleoDB),
/// so the push reads the status AND the server's reason (`message`, the
/// `error` / `detail` text of the reply), and only for a body it knows is a
/// version 2 file. SpeleoDB's upload view answers a refused file with a 400
/// whose `error` is "An error occurred processing the file: [...]" around
/// the file processor's reason (`speleodb/api/v2/views/file.py`):
///
/// - a server without version 2 refuses the JSON part's type ("Invalid file
///   type received: `application/json`, expected one of: [...]", the list
///   naming the SQLite types), or, for a body sent as another type, its
///   content ("Invalid STALAGMIGHT file:
///   expected a valid SQLite database."; SQLite itself says "file is not a
///   database");
/// - a server that reads version 2 refuses a file it finds invalid with its
///   own reason ("Invalid STALAGMIGHT version 2 file: `conventions` must be
///   exactly the version 2 conventions."), which the user is shown as it is.
///
/// Hence:
///
/// - [`UploadRefusal::Content`]: a 400, 406 or 422 whose reason names the file
///   type or SQLite ([`names_the_file_format`]), and any 415 (the media type
///   refused, by definition). Every other cause of those this app can produce
///   is checked before a byte is sent (the size limit, the commit message), so
///   for a version 2 body the format is what was refused. Nothing landed.
/// - [`UploadRefusal::IfNotLanded`]: a server error (5xx, except 503 and 504)
///   whose reason names the format. It refuses the file only when nothing
///   landed, which the push checks in the server's history before it says so;
///   a 5xx after a commit stays the ambiguous "could not be confirmed".
/// - [`UploadRefusal::Other`]: everything else keeps its own meaning: a
///   refusal with another reason shows that reason, a 5xx without one stays
///   unconfirmed, 401 is sign-in, 403 the fence, 404 gone, 304 up to date, 413
///   or 429 a proxy's limit, and 503 (try later) and 504 (still working: it
///   may yet land).
///
/// Pure, like [`map_status`]: it chooses between two errors, and the action of
/// each is unchanged by it (the format refusal is Fatal; the server's own
/// reason keeps the action `map_status` gives its status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadRefusal {
    Content,
    IfNotLanded,
    Other,
}

/// See [`UploadRefusal`]. `message` is the server's reason as the client
/// parsed it (`SpeleoDbError::Status::message`; "HTTP <status>" when the
/// reply had none).
#[must_use]
pub fn upload_refusal(status: u16, message: &str) -> UploadRefusal {
    match status {
        415 => UploadRefusal::Content,
        _ if !names_the_file_format(message) => UploadRefusal::Other,
        400 | 406 | 422 => UploadRefusal::Content,
        503 | 504 => UploadRefusal::Other,
        500..=599 => UploadRefusal::IfNotLanded,
        _ => UploadRefusal::Other,
    }
}

/// Whether a refusal's reason says the server does not take the file's format:
/// it names the file (or media) type it refused, or the SQLite database it
/// expected (see [`UploadRefusal`]). Case-insensitive. A server that reads
/// version 2 names neither for a version 2 body.
#[must_use]
pub fn names_the_file_format(message: &str) -> bool {
    const SIGNS: [&str; 4] = ["file type", "media type", "sqlite", "not a database"];
    let message = message.to_lowercase();
    SIGNS.iter().any(|sign| message.contains(sign))
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
        } => Some(UnpushedLocalEdits {
            project_id: project_id.clone(),
            local_path: local_path.clone(),
            last_edit_at: last_edit_at.clone(),
            // Whose work it is needs the cache's owner record, which only the core
            // can read; it fills this in where it knows (the online open).
            other_owner: None,
        }),
        _ => None,
    };
    let newer_schema = match err {
        SpeleoDbError::NewerSchema(newer) => Some(newer.clone()),
        _ => None,
    };
    let endpoint_refusal = match err {
        SpeleoDbError::InsecureEndpoint { refusal, .. } => Some(*refusal),
        _ => None,
    };
    let format_refused = matches!(err, SpeleoDbError::UploadFormatRefused { .. }).then_some(true);
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
        // A refused re-pull: also a routing signal, not a failure. The FE reads
        // `unpushed_local` (built above) and offers keep-local vs take-remote.
        SpeleoDbError::WouldDiscardLocalEdits { .. } => {
            (SpeleoDbAction::UnpushedLocalEdits, None, err.to_string())
        }
        // The session-moved refusal from Start Editing's commit guard: the world it
        // was started in is gone (delete / close / switch / logout landed inside
        // its network window) and nothing was committed. Retryable by definition -
        // a retry runs in the successor world and does the right thing there.
        SpeleoDbError::SessionMoved => (SpeleoDbAction::Retry, None, err.to_string()),
        // The server refused this build's file format (plan 12.1): surface it and
        // keep the copy dirty, with the status the server answered.
        SpeleoDbError::UploadFormatRefused { status, .. } => {
            (SpeleoDbAction::Fatal, Some(*status), err.to_string())
        }
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
        | SpeleoDbError::ServerChangeWhileConnected
        | SpeleoDbError::NewerSchema(_)
        | SpeleoDbError::Parse(_)
        | SpeleoDbError::Io(_)
        | SpeleoDbError::InvalidProjectId
        | SpeleoDbError::InvalidRevision
        | SpeleoDbError::InsecureEndpoint { .. }
        | SpeleoDbError::UploadTooLarge { .. }
        | SpeleoDbError::OtherAccountsCopy { .. }
        | SpeleoDbError::NoRemoteProject => (SpeleoDbAction::Fatal, None, err.to_string()),
    };
    SpeleoErrorDto {
        message,
        action,
        status,
        transport,
        unpushed_local,
        newer_schema,
        endpoint_refusal,
        format_refused,
        replaced_revisions: None,
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::{dto_for_error, map_status, Endpoint, SpeleoDbAction, SpeleoDbError};

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
            map_status(Endpoint::DownloadStalx, 403),
            SpeleoDbAction::ReadOnly
        );
        // acquire-400 (lock held) is NOT a first-class row → Fatal (deferred to #194).
        assert_eq!(map_status(Endpoint::Acquire, 400), SpeleoDbAction::Fatal);
        assert_eq!(map_status(Endpoint::Acquire, 403), SpeleoDbAction::Fence);
        assert_eq!(
            map_status(Endpoint::UploadStalx, 403),
            SpeleoDbAction::Fence
        );
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
            map_status(Endpoint::UploadStalx, 304),
            SpeleoDbAction::UpToDate
        );
        assert_eq!(
            map_status(Endpoint::UploadStalx, 422),
            SpeleoDbAction::Fatal
        );
        // Verified live: a malformed / missing-artifact upload is 400, which has
        // no first-class row and must fail closed to Fatal.
        assert_eq!(
            map_status(Endpoint::UploadStalx, 400),
            SpeleoDbAction::Fatal
        );
        assert_eq!(
            map_status(Endpoint::UploadStalx, 503),
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
            Endpoint::UploadStalx,
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
            map_status(Endpoint::UploadStalx, 304),
            SpeleoDbAction::UpToDate
        );
        // 304 elsewhere is NOT UpToDate.
        assert_ne!(
            map_status(Endpoint::DownloadStalx, 304),
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
            Endpoint::DownloadStalx,
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
            Endpoint::UploadStalx,
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
            map_status(Endpoint::DownloadStalx, 403),
            SpeleoDbAction::ReadOnly
        );
        assert_eq!(
            map_status(Endpoint::UploadStalx, 403),
            SpeleoDbAction::Fence
        );
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
    fn a_newer_schema_refusal_is_fatal_and_carries_its_payload() {
        let newer = crate::models::NewerSchema {
            file: "project.stalx".to_owned(),
            found: 19,
            supported: 18,
        };
        let dto = dto_for_error(Endpoint::User, &SpeleoDbError::NewerSchema(newer.clone()));
        assert_eq!(dto.action, SpeleoDbAction::Fatal);
        assert_eq!(dto.message, newer.to_string());
        assert_eq!(dto.newer_schema, Some(newer));
        assert_eq!(
            dto_for_error(Endpoint::User, &SpeleoDbError::NotAuthenticated).newer_schema,
            None
        );
    }

    #[test]
    fn an_endpoint_refusal_is_fatal_and_says_why() {
        use super::EndpointRefusal;
        for refusal in [EndpointRefusal::PlainHttp, EndpointRefusal::InvalidAddress] {
            let dto = dto_for_error(
                Endpoint::Login,
                &SpeleoDbError::InsecureEndpoint {
                    endpoint: "http://192.0.2.1/".to_owned(),
                    refusal,
                },
            );
            assert_eq!(dto.action, SpeleoDbAction::Fatal);
            assert_eq!(dto.endpoint_refusal, Some(refusal));
        }
        // Only the policy's refusal carries it, so it is a sound discriminant.
        let other = dto_for_error(Endpoint::Login, &SpeleoDbError::MissingCredentials);
        assert_eq!(other.endpoint_refusal, None);
        let json = serde_json::to_string(&other).expect("serialize dto");
        assert!(
            !json.contains("endpoint_refusal"),
            "absent unless refused: {json}"
        );
    }

    /// What SpeleoDB answers (see `UploadRefusal`): a server without version
    /// 2 names the type it refused or the SQLite it expected; one that reads
    /// version 2 gives its own reason.
    const NO_V2_TYPE: &str = "An error occurred processing the file: [\"Invalid file type \
        received: `application/json`, expected one of: ['application/octet-stream', \
        'application/vnd.sqlite3']\"]";
    const NO_V2_SQLITE: &str = "An error occurred processing the file: ['Invalid STALAGMIGHT \
        file: expected a valid SQLite database.']";
    const NO_V2_SERVER_ERROR: &str =
        "There has been a problem committing the files: file is not a database";
    const V2_REASON: &str = "An error occurred processing the file: ['Invalid STALAGMIGHT \
        version 2 file: `conventions` must be exactly the version 2 conventions.']";

    /// Review finding O3 (package U5): a refusal is the format refusal only
    /// when the server's reason says so; any other reason is the server's
    /// own, with any status, so a real validation error never reads as a
    /// version problem. Injection: answer `Content` for a 400 whatever the
    /// reason (the status-only rule) and the `V2_REASON` rows fail.
    #[test]
    fn upload_refusal_reads_the_status_and_the_servers_reason() {
        use super::{names_the_file_format, upload_refusal, UploadRefusal};
        for reason in [NO_V2_TYPE, NO_V2_SQLITE, NO_V2_SERVER_ERROR] {
            assert!(names_the_file_format(reason), "{reason}");
        }
        for status in [400, 406, 415, 422] {
            for reason in [NO_V2_TYPE, NO_V2_SQLITE] {
                assert_eq!(
                    upload_refusal(status, reason),
                    UploadRefusal::Content,
                    "{status} {reason}"
                );
            }
        }
        // The server's own reason, or none at all, is not a format refusal...
        for status in [400, 406, 422] {
            for reason in [
                V2_REASON,
                "HTTP 400",
                "Synthetic refusal of the uploaded file.",
            ] {
                assert_eq!(
                    upload_refusal(status, reason),
                    UploadRefusal::Other,
                    "{status} {reason}"
                );
            }
        }
        // ...except a 415, which refuses the media type by definition.
        assert_eq!(upload_refusal(415, V2_REASON), UploadRefusal::Content);
        assert_eq!(upload_refusal(415, "HTTP 415"), UploadRefusal::Content);
        for status in [500, 501, 502, 505, 599] {
            assert_eq!(
                upload_refusal(status, NO_V2_SERVER_ERROR),
                UploadRefusal::IfNotLanded,
                "{status}"
            );
            for reason in [V2_REASON, "HTTP 500"] {
                assert_eq!(
                    upload_refusal(status, reason),
                    UploadRefusal::Other,
                    "{status} {reason}"
                );
            }
        }
        // Their own meanings whatever the reason: sign-in, fence, gone, up to
        // date, a proxy's size or rate limit, try later, and a gateway timeout
        // that may still land.
        for status in [304, 401, 403, 404, 409, 413, 429, 503, 504] {
            assert_eq!(
                upload_refusal(status, NO_V2_TYPE),
                UploadRefusal::Other,
                "{status}"
            );
        }
    }

    #[test]
    fn a_format_refusal_is_fatal_says_one_thing_and_keeps_its_status() {
        let refused = SpeleoDbError::UploadFormatRefused {
            status: 415,
            detail: "Unsupported media type".to_owned(),
        };
        let dto = dto_for_error(Endpoint::UploadStalx, &refused);
        assert_eq!(dto.action, SpeleoDbAction::Fatal);
        assert_eq!(dto.status, Some(415));
        assert_eq!(dto.format_refused, Some(true));
        assert!(
            dto.message.starts_with(
                "This SpeleoDB server does not accept projects saved by this version yet."
            ),
            "{}",
            dto.message
        );
        assert!(
            !dto.message.contains("Unsupported media type"),
            "the server's text is for the log"
        );
        // Only the refusal carries it, so it is a sound discriminant.
        let other = dto_for_error(
            Endpoint::UploadStalx,
            &SpeleoDbError::Status {
                status: 400,
                message: "bad".to_owned(),
            },
        );
        assert_eq!(other.format_refused, None);
        let json = serde_json::to_string(&other).expect("serialize dto");
        assert!(
            !json.contains("format_refused"),
            "absent unless refused: {json}"
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
