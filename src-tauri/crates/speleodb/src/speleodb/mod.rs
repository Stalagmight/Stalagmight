//! SpeleoDB integration - the primitives behind the single-writer hand-off (#182).
//!
//! Log in, list native projects, transfer SQLite `project.stalx` files, take/release
//! the server write lock, reconcile history, push revisions, and edit metadata.
//! The persisted `SpeleoLink` drives the single-writer edit gate.
//!
//! Layout:
//! - [`client`]   - the published SpeleoDB SDK adapter (Bearer auth, retries, timeouts).
//! - [`auth`]     - backend-only token cache.
//! - [`config`]   - persisted connection settings (re-exported via `crate::config`).
//! - [`contract`] - the pure status→action table (#186) + `SpeleoErrorDto`.
//! - [`native`]   - temporary native SQLite downloads.
//! - [`metrics`]  - per-endpoint traffic instrumentation (#210).
//! - [`models`]   - serde DTOs + the IPC-facing types.
//! - [`sync`]     - the pure sync engine: BASE-SHA fence + ancestry recovery.
//! - [`tls`]      - the http/https private-network policy (#187).
//!
//! The orchestration that ties these to `AppState`/config/app-dirs lives in
//! `core::state::speleodb`; the IPC commands are in `commands::speleodb`.

pub mod auth;
pub mod client;
// Persisted connection settings ([`SpeleoDbConfig`]), re-exported via `crate::config`.
pub mod config;
// [Track A] #186 status→action contract (pure map_status + ts-rs SpeleoDbAction).
pub mod contract;
pub mod native;
// [F1] #210 per-endpoint traffic instrumentation (count + latency + outcome).
pub mod metrics;
pub mod models;
// [P1] #192/#193 pure sync-engine logic: base-SHA fence + ancestry recovery.
pub mod sync;
// [Track A] #187 http/https private-network TLS policy (pure classify_addr + enforce).
pub(crate) mod tls;

// [Track A] #186/D-B2: SpeleoDbAction + the structured SpeleoErrorDto cross IPC.
pub use contract::{SpeleoDbAction, TransportCause};
pub use models::{
    DoneResult, Permission, PushResult, RemoteState, SpeleoAccessVerification, SpeleoErrorDto,
    SpeleoFileAccess, SpeleoLinkStatus, SpeleoLocalProjectState, SpeleoLoginResult,
    SpeleoOpenResult, SpeleoProjectMetadata, SpeleoProjectMetadataPatch, SpeleoProjectSummary,
    SpeleoPublishParams, SpeleoPublishResult, SpeleoReleaseContext, SpeleoRevision,
    SpeleoRewindTarget, SpeleoSessionStatus, SpeleoUnwindTarget, StartEditingResult,
    UnpushedLocalEdits, UpdateResult, Visibility,
};

/// Errors from the SpeleoDB client and native-file handling.
///
/// Shaped so the status→action decision table (#186) can branch on
/// [`SpeleoDbError::Status`] without re-parsing message strings.
#[derive(Debug, thiserror::Error)]
pub enum SpeleoDbError {
    /// No token is available - the user is not logged in (or the token was dropped).
    #[error("not logged in to SpeleoDB")]
    NotAuthenticated,
    /// No endpoint is configured (no `DEV_SPELEODB_URL`, no saved config).
    #[error("SpeleoDB endpoint is not configured")]
    NoEndpoint,
    /// Password login was attempted without both an email and a password (and
    /// without a pasted token). Rejected at the boundary rather than POSTing
    /// blank credentials and relying on the server to 400.
    #[error("email and password are required to log in")]
    MissingCredentials,
    /// A transport-level failure (DNS, timeout, connection reset, body read).
    ///
    /// `cause` classifies the SDK transport error for translated UI messages.
    /// `detail` preserves diagnostics with request URLs removed.
    #[error("Network error talking to SpeleoDB: {detail}")]
    Transport {
        cause: TransportCause,
        detail: String,
    },
    /// The server returned a non-2xx status. `status` is the HTTP code; `message`
    /// is a best-effort detail parsed from the body.
    #[error("SpeleoDB returned HTTP {status}: {message}")]
    Status { status: u16, message: String },
    /// A response (or native file) could not be parsed into the expected shape.
    #[error("invalid response from SpeleoDB: {0}")]
    Parse(String),
    /// A downloaded native file requires a newer Stalagmight schema.
    #[error("{0}")]
    NewerSchema(crate::models::NewerSchema),
    /// A local filesystem error while reading/writing.
    #[error("filesystem error: {0}")]
    Io(String),
    /// The project id supplied over IPC was not a valid SpeleoDB project UUID.
    /// Guards the cache-path join and request-URL interpolation against path /
    /// route traversal at the trust boundary.
    #[error("invalid SpeleoDB project id")]
    InvalidProjectId,
    /// [#218] The revision SHA supplied over IPC (revision restore) was not a
    /// valid git hex object id. Guards the download-at-hash request-URL
    /// interpolation against route traversal at the trust boundary, exactly like
    /// [`SpeleoDbError::InvalidProjectId`].
    #[error("invalid SpeleoDB revision id")]
    InvalidRevision,
    // [Track A] #187: the endpoint scheme/host failed the http private-network
    // policy (public `http://`, a missing/unknown scheme, or an unresolvable
    // host). Non-retryable by construction - `tls::enforce` errors before any
    // request attempt, so `with_retry` never loops on it.
    #[error("refusing to use an insecure SpeleoDB endpoint: {endpoint}")]
    InsecureEndpoint { endpoint: String },
    /// [P1] No open remote SpeleoDB project to act on (start editing / done /
    /// update require one). Fails closed (Fatal).
    #[error("no open SpeleoDB project")]
    NoRemoteProject,
    /// The open project changed - or went away - between the moment Start Editing
    /// read it and the moment it was ready to open the edit gate (a delete, close,
    /// project switch or logout landed inside the network window). Nothing was
    /// committed: the gate stayed shut and no acquisition was recorded. Retrying in
    /// the new world does the right thing, whatever that is.
    #[error(
        "the project changed while Start Editing was in flight; nothing was changed - try again"
    )]
    SessionMoved,
    /// A re-pull would have overwritten a local copy that still holds edits the
    /// server has never seen. REFUSED rather than performed: un-pushed local work
    /// exists in no git anywhere, so replacing it is unrecoverable, and the user
    /// never asked for that - they asked to open / restore / refresh.
    ///
    /// A routing signal: it crosses the typed action channel as
    /// [`SpeleoDbAction::UnpushedLocalEdits`] so the frontend can offer the binary
    /// choice (keep the local copy, or take the server's and discard the local
    /// edits) instead of scraping the message. `local_path` is the copy that holds
    /// the edits - the "keep local" branch opens exactly that path through the
    /// normal open flow, no download.
    #[error("this project has local edits that were never pushed to SpeleoDB")]
    WouldDiscardLocalEdits {
        project_id: String,
        local_path: String,
        last_edit_at: Option<String>,
    },
}
