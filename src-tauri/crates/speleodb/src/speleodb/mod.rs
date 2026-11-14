//! SpeleoDB integration - the primitives behind the single-writer hand-off (#182).
//!
//! Log in, list native projects, transfer SQLite `project.stalx` files, take/release
//! the server write lock, reconcile history, push revisions, and edit metadata.
//! The persisted `SpeleoLink` drives the single-writer edit gate.
//!
//! Layout:
//! - [`client`]   - the published SpeleoDB SDK adapter (Bearer auth, retries, timeouts).
//! - [`commit_message`] - the upload-description limit, measured as the server stores it.
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
// sdk#2: the longest upload description the server can store.
pub mod commit_message;
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
// [Track A] #187 http/https transport policy (automatic http exceptions).
pub(crate) mod tls;
pub use tls::{clean_endpoint_input, validate_endpoint};

// [Track A] #186/D-B2: SpeleoDbAction + the structured SpeleoErrorDto cross IPC.
pub use contract::{EndpointRefusal, SpeleoDbAction, TransportCause};
pub use models::{
    DoneResult, Permission, PushResult, RemoteState, SpeleoAccessVerification, SpeleoCollaborator,
    SpeleoErrorDto, SpeleoFileAccess, SpeleoLinkStatus, SpeleoLocalProjectState, SpeleoLoginResult,
    SpeleoOpenResult, SpeleoProjectSummary, SpeleoPublishParams, SpeleoPublishResult,
    SpeleoReadOnlyOpen, SpeleoReleaseContext, SpeleoRevision, SpeleoSessionStatus,
    StartEditingResult, UnpushedLocalEdits, UpdateResult, Visibility,
};

/// Errors from the SpeleoDB client and native-file handling.
///
/// Shaped so the status→action decision table (#186) can branch on
/// [`SpeleoDbError::Status`] without re-parsing message strings.
#[derive(Debug, thiserror::Error)]
pub enum SpeleoDbError {
    /// No token is available - the user is not connected (or the token was dropped).
    #[error("not connected to SpeleoDB")]
    NotAuthenticated,
    /// No endpoint is configured (no `DEV_SPELEODB_URL`, no saved config).
    #[error("SpeleoDB endpoint is not configured")]
    NoEndpoint,
    /// Password login was attempted without both an email and a password (and
    /// without a pasted token). Rejected at the boundary rather than POSTing
    /// blank credentials and relying on the server to 400.
    #[error("an email and a password are required to connect")]
    MissingCredentials,
    /// A sign-in (or a settings write) would move a connected session to another
    /// SpeleoDB server. Changing servers is a Disconnect - with its sign-out flow,
    /// which protects un-uploaded work and hands locks back - and a fresh
    /// Connect, never a sign-in that replaces the session underneath open work.
    /// Refused before anything is sent or saved.
    #[error("Disconnect from SpeleoDB before changing servers")]
    ServerChangeWhileConnected,
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
    // [Track A] #187: the endpoint failed the transport policy
    // (`tls::validate_endpoint`): plain `http://` outside the automatic
    // exceptions, or an unusable address (no/unknown scheme, no host, user info,
    // bad port). Decided without DNS. Non-retryable by construction - the policy
    // errors before the SDK client exists, so `SdkClient::call` never loops on
    // it. `refusal` says which rule refused it, for the frontend's translated line.
    // The endpoint is printed with `{:?}` so it arrives quoted and escaped (#675):
    // a character the user cannot see shows up in the message instead of making
    // a correct-looking address look rejected.
    #[error("{} Rejected endpoint: {endpoint:?}", .refusal.reason())]
    InsecureEndpoint {
        endpoint: String,
        refusal: EndpointRefusal,
    },
    /// sdk#3: the project file is larger than SpeleoDB accepts in one upload
    /// ([`client::SERVER_UPLOAD_LIMIT_BYTES`]), even with its undo history left
    /// out. Refused BEFORE any byte is sent: nothing was uploaded, and the local
    /// copy and its dirty bit are untouched. Fails closed (Fatal).
    #[error(
        "this project is too large to upload to SpeleoDB: it is {} MB and the server \
         accepts at most {} MB, even without the undo history. Nothing was uploaded; \
         your changes are still on this computer",
        megabytes(.size),
        megabytes(.limit)
    )]
    UploadTooLarge { size: u64, limit: u64 },
    /// [P1] No open remote SpeleoDB project to act on (start editing / done /
    /// update require one). Fails closed (Fatal).
    #[error("no open SpeleoDB project")]
    NoRemoteProject,
    /// Start Editing on a cached copy whose owner record names another account and
    /// that holds un-uploaded edits: taking the lock would make that account's work
    /// this account's. Refused (Fatal); the copy stays open read-only.
    #[error(
        "this copy holds changes {owner} made that were never uploaded to SpeleoDB. Only \
         {owner} can edit and upload them, so it stays read-only for this account; you \
         can still save a copy of it"
    )]
    OtherAccountsCopy { owner: String },
    /// The open project changed - or went away - between the moment a SpeleoDB
    /// operation read it and the moment it was ready to commit (a delete, close,
    /// project switch or logout landed inside the network window): Start Editing
    /// opening the edit gate, or a revision View / Restore replacing the working
    /// copy. Nothing was committed: no gate opened, no acquisition was recorded,
    /// no file was replaced and the project open now was left alone. Retrying in
    /// the new world does the right thing, whatever that is.
    #[error(
        "the project changed while this request was in flight; nothing was changed - try again"
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

/// A byte count as decimal megabytes with one decimal, for user-facing text
/// (`5_242_880` -> `5.2`).
fn megabytes(bytes: &u64) -> String {
    format!("{:.1}", *bytes as f64 / 1_000_000.0)
}
