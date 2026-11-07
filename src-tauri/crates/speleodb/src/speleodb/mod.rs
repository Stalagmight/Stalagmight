//! SpeleoDB integration - the primitives behind the single-writer hand-off (#182).
//!
//! The full round trip is implemented: log in, list projects, download a project
//! "dump" and extract its `.stalx`, take/release the server write lock (or
//! force-edit without one), reconcile against the server history, push a new
//! revision, persist the `SpeleoLink` that marks a project remote and drives the
//! edit gate, browse/restore past revisions, and administer the project
//! (metadata, collaborators, teams, delete).
//!
//! Layout:
//! - [`client`]   - the ureq HTTP surface (Bearer auth, retry/timeout idiom).
//! - [`auth`]     - backend-only token storage (keychain / plaintext / memory tiers).
//! - [`config`]   - persisted connection settings (re-exported via `crate::config`).
//! - [`contract`] - the pure status→action table (#186) + `SpeleoErrorDto`.
//! - [`dump`]     - ZIP dump classification + `.stalx` extraction.
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
pub mod dump;
// [F1] #210 per-endpoint traffic instrumentation (count + latency + outcome).
pub mod metrics;
pub mod models;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod server_enum_tests;
// [P1] #192/#193 pure sync-engine logic: base-SHA fence + ancestry recovery.
pub mod sync;
// [Track A] #187 http/https private-network TLS policy (pure classify_addr + enforce).
pub(crate) mod tls;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod transport_tests;

// [Track A] #186/D-B2: SpeleoDbAction + the structured SpeleoErrorDto cross IPC.
pub use contract::{SpeleoDbAction, TransportCause};
pub use models::{
    AdoptUpload, DoneResult, DumpCandidateKind, KeychainReport, Permission, PushResult,
    RemoteState, SpeleoAdoptResult, SpeleoCollaborator, SpeleoCollaborators, SpeleoDumpCandidate,
    SpeleoErrorDto, SpeleoLinkStatus, SpeleoLocalProjectState, SpeleoLoginResult, SpeleoOpenResult,
    SpeleoProjectMetadata, SpeleoProjectMetadataPatch, SpeleoProjectSummary, SpeleoPublishParams,
    SpeleoPublishResult, SpeleoRevision, SpeleoRewindTarget, SpeleoSessionStatus, SpeleoTeamGrant,
    SpeleoTeamOption, SpeleoUnwindTarget, SpeleoUserSuggestion, StartEditingResult, TokenStorage,
    UnpushedLocalEdits, UpdateResult, Visibility,
};

/// Errors from the SpeleoDB client and dump handling.
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
    /// `cause` is the TYPED classification (from `ureq::Error` / `io::ErrorKind`,
    /// see `client::classify_transport`) that crosses IPC so the frontend can
    /// render a translated, actionable line naming the configured endpoint.
    /// `detail` is the raw cause, cleaned of `ureq`/io noise (`io: `,
    /// ` (os error N)`) - it is the DIAGNOSTIC surface (logs + bug reports), NOT
    /// what the user is shown: a bare errno like "Invalid argument" tells them
    /// nothing (#270).
    #[error("Network error talking to SpeleoDB: {detail}")]
    Transport {
        cause: TransportCause,
        detail: String,
    },
    /// The server returned a non-2xx status. `status` is the HTTP code; `message`
    /// is a best-effort detail parsed from the body.
    #[error("SpeleoDB returned HTTP {status}: {message}")]
    Status { status: u16, message: String },
    /// A response (or dump) could not be parsed into the expected shape.
    #[error("invalid response from SpeleoDB: {0}")]
    Parse(String),
    /// The dump contained no openable project and no Ariane sidecar.
    #[error("the project dump contained nothing to open")]
    EmptyDump,
    /// The dump held an `ariane.tml` but no `.stalx` - route to the one-time C2
    /// adoption flow (#214). Surfaced to the FE as `SpeleoDbAction::Adopt`, not a
    /// hard error.
    #[error("this is an Ariane (.tml) project; adopt it to edit it in Stalagmight")]
    AdoptAriane,
    /// The dump held more than one `.stalx` - ambiguous. [#258] Surfaced to the FE
    /// as `SpeleoDbAction::Ambiguous` with the candidates attached, so the user can
    /// pick one.
    ///
    /// A COUNT, not the names. Two reasons, and neither is brevity. The verbs this
    /// used to be the fallback for - the fast-forward family, which carries no
    /// `entryPath` - no longer reach it at all: they are refused earlier with
    /// `AMBIGUOUS_NEEDS_REOPEN`, which names the way out. And the names are
    /// server-supplied and unbounded in aggregate, so joining them built a second full
    /// copy of every candidate path inside the error, alongside the structured list the
    /// DTO already carries. A count says the same useful thing to anyone who does see
    /// it, for a fixed size.
    #[error(
        "SpeleoDB contains more than one Stalagmight file ({} found). Choose which to open, or remove the extra file(s) on SpeleoDB, leaving a single one.",
        .0.len()
    )]
    AmbiguousDump(Vec<SpeleoDumpCandidate>),
    /// The dump held no `.stalx` but more than one `ariane.tml` - ambiguous on the
    /// adoption path. [#258] A count rather than the names, for the reasons above.
    #[error(
        "SpeleoDB contains more than one Ariane file ({} found). Choose which to adopt, or remove the extra file(s) on SpeleoDB, leaving a single one.",
        .0.len()
    )]
    AmbiguousAriane(Vec<SpeleoDumpCandidate>),
    /// A local filesystem error while extracting/writing.
    #[error("filesystem error: {0}")]
    Io(String),
    /// [#576 NF-W4S2-2] A verb refused locally, before anything was attempted - the
    /// sentence says why and what to do ("reconnect to acquire the write lock before
    /// pushing"). Shown as written: wrapping these in [`Self::Io`] prefixed them with
    /// "filesystem error:", which they are not.
    ///
    /// [#576 C4] `code` is what the frontend routes the refusal on: the refusing
    /// gate's own code when the core refused it (the edit gate, the crash refusal,
    /// a refused save, a newer file), `SPELEODB_FATAL` when it is only a sentence
    /// ([`Self::refused`]). Build it with [`Self::refused`] / [`Self::refused_with`].
    #[error("{message}")]
    Refused {
        code: crate::models::CommandErrorCode,
        message: String,
    },
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
    /// [#576 B35(e)] Done asked the server who holds the open project's write lock
    /// and it is not this account: someone else took it (`holder` names them) or
    /// it is gone (`None` - released by an admin, or expired). Nothing was uploaded
    /// (the upload needs the lock) and nothing was released (a release by a project
    /// ADMIN would free somebody else's mutex); the edit gate was switched off and
    /// any un-pushed edits stay marked for the next Start editing.
    #[error("{}", lock_not_held_message(.holder.as_deref()))]
    LockNotHeld { holder: Option<String> },
    /// A re-pull would have overwritten a local copy that still holds edits the
    /// server has never seen. REFUSED rather than performed: un-pushed local work
    /// exists in no git anywhere, so replacing it is unrecoverable, and the user
    /// never asked for that - they asked to open / restore / refresh.
    ///
    /// Not a failure so much as a routing signal, like
    /// [`SpeleoDbError::AdoptAriane`]: it crosses the typed action channel as
    /// [`SpeleoDbAction::UnpushedLocalEdits`] so the frontend can offer the binary
    /// choice (keep the local copy, or take the server's and discard the local
    /// edits) instead of scraping the message. `local_path` is the copy that holds
    /// the edits - the "keep local" branch opens exactly that path through the
    /// normal open flow, no download.
    ///
    /// [#576 W53] `state_unknown` is the guard's OTHER refusal: the copy is there
    /// but could not be read (a hot journal a read-only open cannot roll back, a
    /// lock another writer holds, damage), so un-pushed edits cannot be ruled out.
    /// It routes exactly like the dirty one - the same choice, the same lock
    /// retention - but its message says what is actually known instead of claiming
    /// edits nobody made.
    #[error("{}", would_discard_local_edits_message(*.state_unknown))]
    WouldDiscardLocalEdits {
        project_id: String,
        local_path: String,
        last_edit_at: Option<String>,
        state_unknown: bool,
    },
}

impl SpeleoDbError {
    /// A verb refused locally with `message` and no classification of its own:
    /// it reaches the frontend as `SPELEODB_FATAL`, like every local failure.
    pub fn refused(message: impl Into<String>) -> Self {
        Self::refused_with(crate::models::CommandErrorCode::SpeleodbFatal, message)
    }

    /// A verb refused locally by a refusal that has its own `code` (#576 C4).
    pub fn refused_with(code: crate::models::CommandErrorCode, message: impl Into<String>) -> Self {
        Self::Refused {
            code,
            message: message.into(),
        }
    }
}

/// The user-facing line for [`SpeleoDbError::WouldDiscardLocalEdits`]: the copy
/// read as dirty, or (`state_unknown`) it could not be read at all, which the line
/// names together with how the two branches the refusal offers get past it.
fn would_discard_local_edits_message(state_unknown: bool) -> &'static str {
    if state_unknown {
        "the copy of this project on this computer could not be read, so it was not \
         replaced: a save may have been interrupted, or another program may be using it. \
         Open the local copy to let an interrupted save roll back to the last one that \
         finished (close any other program using it first), or take the server copy, \
         which discards the local one"
    } else {
        "this project has local edits that were never pushed to SpeleoDB"
    }
}

/// The user-facing line for [`SpeleoDbError::LockNotHeld`]: who holds the lock now
/// (when the server named someone), what did NOT happen, and that editing stopped.
fn lock_not_held_message(holder: Option<&str>) -> String {
    match holder {
        Some(holder) => format!(
            "{holder} now holds the SpeleoDB write lock on this project, so nothing was \
             uploaded and no lock was released. Editing has been switched off; changes you \
             have not uploaded stay on this computer"
        ),
        None => "you no longer hold the SpeleoDB write lock on this project (it was \
                 released or expired), so nothing was uploaded. Editing has been switched \
                 off; start editing again to upload changes that are still only on this \
                 computer"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{SpeleoDbError, SpeleoDumpCandidate, TransportCause};

    #[test]
    fn lock_not_held_names_the_holder_and_what_did_not_happen() {
        // [#576 B35(e)] The Done refusal is shown verbatim in the push-failure banner.
        let taken = SpeleoDbError::LockNotHeld {
            holder: Some("colleague@example.com".to_owned()),
        }
        .to_string();
        assert!(taken.starts_with("colleague@example.com now holds the SpeleoDB write lock"));
        assert!(taken.contains("nothing was uploaded and no lock was released"));
        let gone = SpeleoDbError::LockNotHeld { holder: None }.to_string();
        assert!(gone.starts_with("you no longer hold the SpeleoDB write lock"));
        assert!(gone.contains("start editing again"));
    }

    #[test]
    fn error_display_strings_are_stable() {
        // These messages are the human-facing surface (and the #186 status→action
        // table keys off `Status`); pin them so they can't drift silently.
        assert_eq!(
            SpeleoDbError::NotAuthenticated.to_string(),
            "not logged in to SpeleoDB"
        );
        assert_eq!(
            SpeleoDbError::NoEndpoint.to_string(),
            "SpeleoDB endpoint is not configured"
        );
        assert_eq!(
            SpeleoDbError::Status {
                status: 404,
                message: "not found".to_owned(),
            }
            .to_string(),
            "SpeleoDB returned HTTP 404: not found"
        );
        assert_eq!(
            SpeleoDbError::EmptyDump.to_string(),
            "the project dump contained nothing to open"
        );
        assert_eq!(
            SpeleoDbError::AdoptAriane.to_string(),
            "this is an Ariane (.tml) project; adopt it to edit it in Stalagmight"
        );
        assert_eq!(
            SpeleoDbError::InvalidProjectId.to_string(),
            "invalid SpeleoDB project id"
        );
        assert_eq!(
            SpeleoDbError::NoRemoteProject.to_string(),
            "no open SpeleoDB project"
        );
        assert_eq!(
            SpeleoDbError::MissingCredentials.to_string(),
            "email and password are required to log in"
        );
        // [#258] The ambiguity Display still has to name the remedy - every path that
        // can offer the picker routes on `action == Ambiguous` and never shows this
        // string, so whoever DOES see it has no dialog to act in.
        //
        // A COUNT rather than the names, and the names being server-supplied is why:
        // joining them built a second unbounded copy of every candidate path inside the
        // error, beside the structured list the DTO already carries. The count says the
        // same useful thing for a fixed size, and this assertion is what stops the join
        // creeping back.
        let candidate = |path: &str| SpeleoDumpCandidate {
            entry_path: path.to_owned(),
            size_bytes: 0,
            modified: None,
        };
        assert_eq!(
            SpeleoDbError::AmbiguousDump(vec![candidate("a.stalx"), candidate("b.stalx")])
                .to_string(),
            "SpeleoDB contains more than one Stalagmight file (2 found). \
             Choose which to open, or remove the extra file(s) on SpeleoDB, leaving a single one."
        );
        assert_eq!(
            SpeleoDbError::AmbiguousAriane(vec![
                candidate("ariane.tml"),
                candidate("misc/ariane.tml")
            ])
            .to_string(),
            "SpeleoDB contains more than one Ariane file (2 found). \
             Choose which to adopt, or remove the extra file(s) on SpeleoDB, leaving a single one."
        );
        // [#270] Transport gained a typed `cause`, but its Display is unchanged:
        // it stays the RAW diagnostic line (logs / bug reports). What changed is
        // that the FE no longer SHOWS this string - it renders a translated line
        // off `SpeleoErrorDto::transport` - so a useful cause like "Connection
        // refused" is still preserved here rather than flattened away.
        assert_eq!(
            SpeleoDbError::Transport {
                cause: TransportCause::Refused,
                detail: "Connection refused".to_owned(),
            }
            .to_string(),
            "Network error talking to SpeleoDB: Connection refused"
        );
        // The errno that started #270 is likewise kept verbatim for the log,
        // while its cause classifies as the catch-all (the FE shows the generic
        // "could not reach the server" line, never this text).
        assert_eq!(
            SpeleoDbError::Transport {
                cause: TransportCause::Unknown,
                detail: "Invalid argument".to_owned(),
            }
            .to_string(),
            "Network error talking to SpeleoDB: Invalid argument"
        );
        // [Track A] #187: the insecure-endpoint message is the human-facing
        // surface for a blocked public-http / scheme-less endpoint; pin it.
        assert_eq!(
            SpeleoDbError::InsecureEndpoint {
                endpoint: "http://example.com".to_owned(),
            }
            .to_string(),
            "refusing to use an insecure SpeleoDB endpoint: http://example.com"
        );
    }
}

#[cfg(test)]
mod live_tests {
    //! Env-gated live tests against a real SpeleoDB box. `#[ignore]` so they
    //! never run in normal `cargo test`/CI; run explicitly with:
    //!   DEV_SPELEODB_URL=… DEV_SPELEODB_EMAIL=… DEV_SPELEODB_PASSWORD=… \
    //!   cargo test -p stalagmight-speleodb --lib live_ -- --ignored
    //! Each test creates throwaway projects per run and deletes them on the way
    //! out (see `ProjectCleanup`), so it owns its own state. The two-account
    //! tests additionally need a pre-provisioned, email-verified collaborator via
    //! `DEV_SPELEODB_EMAIL2`/`_PASSWORD2` (that account can't be created over
    //! HTTP). See `docs/speleodb-dev-testing.md`.
    //!
    //! Env-var tiers. A test run without its tier's vars FAILS, naming the
    //! missing one (#576 E13): `#[ignore]` is the skip, so a deliberate run that
    //! forgot a variable must not report `ok` having tested nothing. Filter a
    //! run to the tier you have the vars for. They NEVER run in CI - there is no
    //! dev box there:
    //! - **URL-only** (needs `DEV_SPELEODB_URL`): `live_bad_credentials_returns_400`,
    //!   `live_missing_auth_header_returns_403`, `live_dead_token_returns_403`,
    //!   `live_http_to_public_blocked` (the public-host block needs no box at all).
    //! - **Single-account** (needs `DEV_SPELEODB_URL` + `_EMAIL` + `_PASSWORD`, via
    //!   `live_env()`): `live_round_trip_seed_pull_open`,
    //!   `live_fetch_user_email_matches_token_owner`,
    //!   `live_upload_200_returns_hexsha_and_project`, `live_upload_304_when_unchanged`,
    //!   `live_upload_malformed_returns_400`, `live_stalx_round_trips_through_auto`,
    //!   `live_auto_rejects_unknown_extension_canary`, `live_http_to_private_box_allowed`,
    //!   `live_render_compat_georef_generates_geojson` (#184 P0.5 gate),
    //!   `live_render_compat_no_anchor_records_geojson_state` (#184 / D2),
    //!   `live_open1_bare_tml_mime_outcome` (#184 / OPEN-1).
    //! - **Two-account** (additionally needs `DEV_SPELEODB_EMAIL2` + `_PASSWORD2`,
    //!   via `live_env2()`): `live_collaborator_sees_read_only_label`,
    //!   `live_web_viewer_cannot_download`, `live_lock_holder_reflects_other_user`,
    //!   `live_no_permission_field_absent`, `live_acquire_contended_returns_400`,
    //!   `live_acquire_no_perm_returns_403`.

    use super::dump::{self, DumpClassification};
    use super::{client, Permission, SpeleoDbError, SpeleoProjectSummary};
    use stalagmight_models::test_support::{missing_prerequisite, require_env};

    /// The box and the first account; fails the test naming any that is unset.
    fn live_env() -> (String, String, String) {
        (
            require_env("DEV_SPELEODB_URL"),
            require_env("DEV_SPELEODB_EMAIL"),
            require_env("DEV_SPELEODB_PASSWORD"),
        )
    }

    /// The SECOND account (a pre-provisioned, email-verified collaborator). It
    /// can't be created over HTTP - the box enforces mandatory email
    /// verification - so the two-account tests fail unless these are set.
    fn live_env2() -> (String, String, String) {
        (
            require_env("DEV_SPELEODB_URL"),
            require_env("DEV_SPELEODB_EMAIL2"),
            require_env("DEV_SPELEODB_PASSWORD2"),
        )
    }

    /// The `.stalx` the upload tests seed with: a small synthetic project saved
    /// into `dir` (#576 E13-D2). The suite used to read
    /// `e2e/fixtures/fractal_plant_iter2.stalx`, which the repo does not carry, so
    /// it could run on no clone but the maintainer's.
    #[expect(clippy::expect_used, reason = "ignored live integration test helper")]
    fn seed_stalx(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("seed.stalx");
        let mut db = stalagmight_persistence::persistence::Database::open(&path)
            .expect("create the seed .stalx");
        db.save_project(&build_render_project(None))
            .expect("save the seed project");
        path
    }

    /// Not ignored: the seed the live tests upload is built on every run, so a
    /// seed that no longer opens fails here, on any clone, not on the dev box.
    #[test]
    #[expect(
        clippy::expect_used,
        reason = "test: a failed setup step is the failure"
    )]
    fn the_live_seed_opens_as_a_project() {
        let dir = tempfile::tempdir().expect("seed tempdir");
        let project =
            stalagmight_persistence::persistence::Database::open_existing(&seed_stalx(dir.path()))
                .expect("the seed opens")
                .load_project()
                .expect("the seed loads");
        assert_eq!(project.sections.len(), 1);
        assert_eq!(project.sections.first().map(|s| s.shots.len()), Some(2));
    }

    /// A **real** Ariane `.tml` (a ZIP archive - verified live: the server's
    /// `ArianeTMLFileProcessor` 400s a non-zip with "File is not a zip file").
    /// Tracks A/B/C do not generate a `.tml` (that is the #184/#196 render path),
    /// so the upload live tests seed with this known-good Ariane fixture to
    /// exercise the #185 transport path independent of our exporter. Copied in as
    /// `ariane.tml` so the multipart part keeps the contract filename.
    const TML_FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/test_simple.tml"
    );

    /// Build a `base/api/v2/{suffix}` URL the same way `client::api_url` does
    /// (`api_url` is private to `client`). Used by the raw-`ureq` tests that need a
    /// request the typed client can't make (no auth header, a junk token, or a
    /// deliberately malformed multipart).
    fn api_url(base: &str, suffix: &str) -> String {
        format!(
            "{}/api/v2/{}",
            base.trim_end_matches('/'),
            suffix.trim_start_matches('/')
        )
    }

    /// A fresh agent with `http_status_as_error(false)` so a non-2xx response is a
    /// normal `Response` whose status the raw tests can read (mirrors the agent the
    /// production `client` builds). Test-local because `client::agent` is private.
    fn raw_agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .new_agent()
    }

    /// Deletes the per-run test project on drop, so the round-trip cleans up
    /// after itself even if an assertion panics partway through.
    struct ProjectCleanup<'a> {
        url: &'a str,
        token: &'a str,
        id: String,
    }

    impl Drop for ProjectCleanup<'_> {
        #[expect(clippy::print_stderr, reason = "ignored live integration test cleanup")]
        fn drop(&mut self) {
            if let Err(e) = client::delete_project(self.url, self.token, false, &self.id, self.url)
            {
                eprintln!("cleanup: failed to delete test project {}: {e:?}", self.id);
            }
        }
    }

    /// Credentials-free live check: junk credentials must come back as HTTP 400
    /// (not 401), which is the exact contract the client branches on. Needs only
    /// `DEV_SPELEODB_URL`, so it can run against the box without any account.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_URL (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_bad_credentials_returns_400() {
        let url = require_env("DEV_SPELEODB_URL");
        let err = client::login(&url, false, "nobody@example.com", "definitely-wrong")
            .expect_err("junk credentials must be rejected");
        assert!(
            matches!(&err, SpeleoDbError::Status { status: 400, .. }),
            "bad credentials should map to HTTP 400, got: {err:?}"
        );
    }

    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "ignored live integration test"
    )]
    fn live_round_trip_seed_pull_open() {
        let (url, email, password) = live_env();

        // 1. Auth.
        let token = client::login(&url, false, &email, &password).expect("login should succeed");

        // 2. Create a throwaway project so the test owns its own state, and
        //    register cleanup before doing anything else that could panic.
        let name = format!("stalagmight-spike-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &token, false, &name)
            .expect("create project")
            .id;
        let _cleanup = ProjectCleanup {
            url: &url,
            token: &token,
            id: project_id.clone(),
        };

        // It should now be visible to this account.
        let projects =
            client::list_projects(&url, &token, false).expect("list_projects should succeed");
        assert!(
            projects.iter().any(|p| p.id == project_id),
            "freshly created project {project_id} not visible in list"
        );

        // 3. Seed a known-good .stalx (+ a tiny .tml sidecar so the production
        //    two-`artifact` upload_auto path is exercised): acquire, upload,
        //    release. The returned Project's latest_commit.id is the new hexsha.
        let seed_dir = tempfile::tempdir().expect("seed tempdir");
        let fixture = &seed_stalx(seed_dir.path());
        let tml_dir = tempfile::tempdir().expect("tml tempdir");
        let tml_sidecar = tml_dir.path().join("ariane.tml");
        std::fs::copy(TML_FIXTURE, &tml_sidecar).expect("copy real Ariane .tml fixture");
        client::acquire_lock(&url, &token, false, &project_id).expect("acquire mutex");
        let uploaded = client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &project_id,
            fixture,
            &tml_sidecar,
            "spike seed",
        )
        .expect("seed upload");
        assert!(
            uploaded.latest_commit.is_some(),
            "upload should return a Project carrying the new commit"
        );
        client::release_lock(&url, &token, false, &project_id).expect("release mutex");

        // 4. Pull the dump, classify, extract, and open the .stalx self-contained.
        let zip = client::download_dump(&client::TestWire, &url, &token, false, &project_id)
            .expect("download dump");
        let mut dump = dump::Dump::open(&zip).expect("open dump zip");
        let names = dump.entry_paths().expect("read dump entries");
        let DumpClassification::Stalx(entry) = dump::classify(&names) else {
            panic!("expected exactly one .stalx in the dump, got entries: {names:?}");
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("project.stalx");
        dump.extract_entry_to(&entry, &dest)
            .expect("extract .stalx");

        let db = stalagmight_persistence::persistence::Database::open_existing(&dest)
            .expect("open extracted .stalx");
        let project = db
            .load_project()
            .expect("load project from extracted .stalx");
        assert!(
            !project.name.is_empty() || !project.sections.is_empty(),
            "extracted .stalx opened but looked empty"
        );
        // `_cleanup` drops here → soft-deletes the test project.
    }

    /// The `GET /user/` whoami returns the token owner's email - the basis for the
    /// token-paste login path deriving the email from the token instead of input.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_fetch_user_email_matches_token_owner() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login should succeed");
        let got =
            client::fetch_user_email(&url, &token, false).expect("fetch_user_email should succeed");
        assert_eq!(got, email, "whoami should return the token owner's email");
    }

    /// Two-account: a collaborator granted READ_ONLY sees the project with the
    /// parsed `Permission::ReadOnly` label - the end-to-end check that the wire
    /// format is the string label, not the integer the issues describe.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* + _EMAIL2/_PASSWORD2 (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_collaborator_sees_read_only_label() {
        let ((url, owner_email, owner_pw), (_, email2, pw2)) = (live_env(), live_env2());
        let owner = client::login(&url, false, &owner_email, &owner_pw).expect("owner login");
        let name = format!("stalagmight-perm-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &owner, false, &name)
            .expect("create project")
            .id;
        let _cleanup = ProjectCleanup {
            url: &url,
            token: &owner,
            id: project_id.clone(),
        };
        client::grant_permission(&url, &owner, &project_id, &email2, "READ_ONLY")
            .expect("grant READ_ONLY");

        let collab = client::login(&url, false, &email2, &pw2).expect("collaborator login");
        let projects = client::list_projects(&url, &collab, false).expect("collaborator list");
        let project = projects
            .iter()
            .find(|p| p.id == project_id)
            .expect("collaborator should see the granted project");
        assert_eq!(
            project.permission,
            Some(Permission::ReadOnly),
            "granted READ_ONLY should parse to Permission::ReadOnly"
        );
    }

    /// Two-account: a WEB_VIEWER cannot download the dump (needs ≥ READ_ONLY), so
    /// the pull surfaces HTTP 403 - the contract the #203 "disable Open for
    /// view-only rows" gating builds on.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* + _EMAIL2/_PASSWORD2 (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_web_viewer_cannot_download() {
        let ((url, owner_email, owner_pw), (_, email2, pw2)) = (live_env(), live_env2());
        let owner = client::login(&url, false, &owner_email, &owner_pw).expect("owner login");
        let name = format!("stalagmight-wv-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &owner, false, &name)
            .expect("create project")
            .id;
        let _cleanup = ProjectCleanup {
            url: &url,
            token: &owner,
            id: project_id.clone(),
        };
        // Seed a real .stalx (+ .tml sidecar) so the only thing stopping the
        // download is permission.
        let seed_dir = tempfile::tempdir().expect("seed tempdir");
        let fixture = &seed_stalx(seed_dir.path());
        let tml_dir = tempfile::tempdir().expect("tml tempdir");
        let tml_sidecar = tml_dir.path().join("ariane.tml");
        std::fs::copy(TML_FIXTURE, &tml_sidecar).expect("copy real Ariane .tml fixture");
        client::acquire_lock(&url, &owner, false, &project_id).expect("acquire mutex");
        client::upload_auto(
            &client::TestWire,
            &url,
            &owner,
            false,
            &project_id,
            fixture,
            &tml_sidecar,
            "seed",
        )
        .expect("seed upload");
        client::release_lock(&url, &owner, false, &project_id).expect("release mutex");
        client::grant_permission(&url, &owner, &project_id, &email2, "WEB_VIEWER")
            .expect("grant WEB_VIEWER");

        let viewer = client::login(&url, false, &email2, &pw2).expect("viewer login");
        let err = client::download_dump(&client::TestWire, &url, &viewer, false, &project_id)
            .expect_err("WEB_VIEWER must be denied the dump download");
        assert!(
            matches!(&err, SpeleoDbError::Status { status: 403, .. }),
            "WEB_VIEWER download should be HTTP 403, got: {err:?}"
        );

        // #188 C5 / D-B4: the download 403 above is AMBIGUOUS (a real read-only
        // grant vs a dead token). The whoami probe disambiguates it - and this
        // WEB_VIEWER token is *alive*, so `GET /user/` returns 200 (resolves the
        // viewer's own email). The probe therefore KEEPS the token: a read-only
        // user is never logged out by the ambiguous-403 auto-drop. (Contrast
        // `live_dead_token_returns_403`, where whoami 403s and the token is dropped.)
        let whoami = client::fetch_user_email(&url, &viewer, false)
            .expect("a live WEB_VIEWER token must still resolve whoami (probe keeps it)");
        assert_eq!(
            whoami, email2,
            "whoami should resolve the viewer's own email - proving the token is alive, not dead"
        );
    }

    /// Two-account: when a different user holds the mutex, the owner's project
    /// list shows that user as the lock holder - validating the
    /// `active_mutex.user` → `SpeleoProjectSummary::lock_holder` mapping.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* + _EMAIL2/_PASSWORD2 (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_lock_holder_reflects_other_user() {
        let ((url, owner_email, owner_pw), (_, email2, pw2)) = (live_env(), live_env2());
        let owner = client::login(&url, false, &owner_email, &owner_pw).expect("owner login");
        let name = format!("stalagmight-lock-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &owner, false, &name)
            .expect("create project")
            .id;
        let _cleanup = ProjectCleanup {
            url: &url,
            token: &owner,
            id: project_id.clone(),
        };
        client::grant_permission(&url, &owner, &project_id, &email2, "READ_AND_WRITE")
            .expect("grant READ_AND_WRITE");

        let collab = client::login(&url, false, &email2, &pw2).expect("collaborator login");
        client::acquire_lock(&url, &collab, false, &project_id)
            .expect("collaborator acquire mutex");

        let projects = client::list_projects(&url, &owner, false).expect("owner list");
        let project = projects
            .iter()
            .find(|p| p.id == project_id)
            .expect("owner should see the project");
        let summary = SpeleoProjectSummary::from_project(project.clone());
        assert_eq!(
            summary.lock_holder.as_deref(),
            Some(email2.as_str()),
            "owner should see the collaborator as the lock holder"
        );

        // Release the collaborator's lock so cleanup's delete isn't fighting it.
        if let Err(e) = client::release_lock(&url, &collab, false, &project_id) {
            eprintln!("cleanup: failed to release collaborator lock: {e:?}");
        }
    }

    // ── L4 / L5: header vs token distinction (URL-only tier) ────────────────

    /// L4: a `GET /projects/` with **no** `Authorization` header is rejected with
    /// HTTP **403** (DRF "not authenticated on a protected endpoint with no
    /// credentials"). NOTE (verified live): a present-but-dead token is **also**
    /// 403 (L5) - the status alone does not distinguish missing-vs-dead; only a
    /// whoami 200-vs-403 separates a valid token from a dead one. The typed client
    /// always sends a Bearer header, so this is a raw `ureq` call. Needs only
    /// `DEV_SPELEODB_URL`.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_URL (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_missing_auth_header_returns_403() {
        let url = require_env("DEV_SPELEODB_URL");
        let resp = raw_agent()
            .get(&api_url(&url, "projects/"))
            // Deliberately NO Authorization header.
            .call()
            .expect("request should reach the server (status branched below)");
        assert_eq!(
            resp.status().as_u16(),
            403,
            "a missing Authorization header should be HTTP 403 (vs dead token → 401)"
        );
    }

    /// L5: a `GET /user/` (whoami) with a **junk/dead** token is rejected with HTTP
    /// **403**, NOT 401 - verified against the live box. This is the DRF `401→403`
    /// downgrade the research flagged (D-B4): a `TokenAuthentication` that fails to
    /// resolve the token supplies no `WWW-Authenticate` challenge, so DRF answers
    /// 403. A *valid* token returns 200 (see `live_fetch_user_email…`), so a **403
    /// from the whoami endpoint is the unambiguous dead-token signal** - no
    /// per-resource permission applies to `/user/`. Sent with the production
    /// `Bearer` scheme so it exercises the real client's dead-token path. Needs
    /// only `DEV_SPELEODB_URL`.
    ///
    /// DESIGN NOTE (#188 C5 / D-B4): because the server never emits 401 for auth,
    /// the 401-based auto-drop (`map_status == ReLogin` only on 401) cannot fire on
    /// a revoked token. The dead-token trigger must instead key off a whoami-403.
    /// This test pins the server fact that fix depends on. See the design
    /// follow-up.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_URL (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_dead_token_returns_403() {
        let url = require_env("DEV_SPELEODB_URL");
        let resp = raw_agent()
            .get(&api_url(&url, "user/"))
            .header("Authorization", "Bearer this-token-is-not-real-deadbeef")
            .call()
            .expect("request should reach the server (status branched below)");
        assert_eq!(
            resp.status().as_u16(),
            403,
            "the live box returns 403 (not 401) for a dead token - the DRF \
             downgrade; a whoami-403 is the dead-token signal"
        );
    }

    // ── L9 / L10 / L11: two-account permission + contention ─────────────────

    /// L9: a project the second user has **no grant** on does not even appear in
    /// their `GET /projects/` list - and on any project they *can* see without a
    /// grant the `permission` field is **ABSENT** (not `null`), parsing to `None`.
    /// We assert the structural invariant the wire format guarantees: when a
    /// project carries no permission the parsed field is `None` (absent ≡ null ≡
    /// `None`), and the un-granted project is invisible to the collaborator.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* + _EMAIL2/_PASSWORD2 (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_no_permission_field_absent() {
        let ((url, owner_email, owner_pw), (_, email2, pw2)) = (live_env(), live_env2());
        let owner = client::login(&url, false, &owner_email, &owner_pw).expect("owner login");
        let name = format!("stalagmight-noperm-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &owner, false, &name)
            .expect("create project")
            .id;
        let _cleanup = ProjectCleanup {
            url: &url,
            token: &owner,
            id: project_id.clone(),
        };
        // No grant to user2 at all.

        let collab = client::login(&url, false, &email2, &pw2).expect("collaborator login");
        let projects = client::list_projects(&url, &collab, false).expect("collaborator list");
        assert!(
            projects.iter().all(|p| p.id != project_id),
            "an un-granted project must NOT appear in the collaborator's list"
        );
        // Every project the collaborator can see carries a parsed permission OR
        // None - never a parse error from a present-but-absent field. (The wire
        // format omits the field entirely when there is no access; serde maps the
        // absent field to None.)
        for p in &projects {
            // `permission` is Option<Permission>; the field being absent on the
            // wire deserializes to None without error. This asserts the
            // absent≡None contract holds across the live payload.
            let _ = p.permission;
        }
    }

    /// L10: when the collaborator **holds the mutex**, the owner's acquire is
    /// rejected with HTTP **400** (`mutex.py` contention) - explicitly NOT 423 /
    /// 409 / 412. Pins the "no first-class lock-status codes" contract `map_status`
    /// relies on (lock contention is a plain 400).
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* + _EMAIL2/_PASSWORD2 (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_acquire_contended_returns_400() {
        let ((url, owner_email, owner_pw), (_, email2, pw2)) = (live_env(), live_env2());
        let owner = client::login(&url, false, &owner_email, &owner_pw).expect("owner login");
        let name = format!("stalagmight-contend-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &owner, false, &name)
            .expect("create project")
            .id;
        let _cleanup = ProjectCleanup {
            url: &url,
            token: &owner,
            id: project_id.clone(),
        };
        client::grant_permission(&url, &owner, &project_id, &email2, "READ_AND_WRITE")
            .expect("grant READ_AND_WRITE");

        // Collaborator takes the mutex first.
        let collab = client::login(&url, false, &email2, &pw2).expect("collaborator login");
        client::acquire_lock(&url, &collab, false, &project_id)
            .expect("collaborator acquires the mutex");

        // Owner's acquire now contends → 400 (NOT 423/409/412).
        let err = client::acquire_lock(&url, &owner, false, &project_id)
            .expect_err("owner acquire must be rejected while the mutex is held");
        assert!(
            matches!(&err, SpeleoDbError::Status { status: 400, .. }),
            "lock contention should map to HTTP 400 (not 423/409/412), got: {err:?}"
        );

        // Release the collaborator's lock so cleanup's delete isn't fighting it.
        if let Err(e) = client::release_lock(&url, &collab, false, &project_id) {
            eprintln!("cleanup: failed to release collaborator lock: {e:?}");
        }
    }

    /// L11: a collaborator with only READ_ONLY (no write permission) attempting to
    /// acquire the mutex is rejected with HTTP **403** ("no write perm" fence) -
    /// the contrast to contention's 400 (L10). `map_status(Acquire, 403) == Fence`.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* + _EMAIL2/_PASSWORD2 (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_acquire_no_perm_returns_403() {
        let ((url, owner_email, owner_pw), (_, email2, pw2)) = (live_env(), live_env2());
        let owner = client::login(&url, false, &owner_email, &owner_pw).expect("owner login");
        let name = format!("stalagmight-noperm-acq-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &owner, false, &name)
            .expect("create project")
            .id;
        let _cleanup = ProjectCleanup {
            url: &url,
            token: &owner,
            id: project_id.clone(),
        };
        // READ_ONLY: visible + downloadable, but NOT write/lock-capable.
        client::grant_permission(&url, &owner, &project_id, &email2, "READ_ONLY")
            .expect("grant READ_ONLY");

        let collab = client::login(&url, false, &email2, &pw2).expect("collaborator login");
        let err = client::acquire_lock(&url, &collab, false, &project_id)
            .expect_err("a READ_ONLY collaborator must be denied the mutex");
        assert!(
            matches!(&err, SpeleoDbError::Status { status: 403, .. }),
            "a no-write-perm acquire should be HTTP 403 (vs contention 400), got: {err:?}"
        );
    }

    // ── L12 / L13 / L14: upload/auto status contract (single-account) ───────

    /// Seed a project + acquire its mutex, returning `(project_id, fixture paths,
    /// tml tempdir guard)`. The caller owns the returned `ProjectCleanup` and the
    /// `TempDir` so they outlive the test body. Used by the upload-status tests.
    #[expect(clippy::expect_used, reason = "ignored live integration test helper")]
    fn seed_and_acquire<'a>(
        url: &'a str,
        token: &'a str,
    ) -> (ProjectCleanup<'a>, std::path::PathBuf, tempfile::TempDir) {
        let name = format!("stalagmight-upload-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(url, token, false, &name)
            .expect("create project")
            .id;
        let cleanup = ProjectCleanup {
            url,
            token,
            id: project_id,
        };
        client::acquire_lock(url, token, false, &cleanup.id).expect("acquire mutex");
        let tml_dir = tempfile::tempdir().expect("tml tempdir");
        let seed = seed_stalx(tml_dir.path());
        (cleanup, seed, tml_dir)
    }

    /// L12: a real `upload/auto` of a fresh change returns **200** with a body
    /// carrying the new `latest_commit.id` (the adopted hexsha) and the project
    /// block. This is the success path #186's "adopt-hexsha + conditional
    /// clear-dirty" handoff keys off.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_upload_200_returns_hexsha_and_project() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let (cleanup, fixture, tml_dir) = seed_and_acquire(&url, &token);
        let tml_sidecar = tml_dir.path().join("ariane.tml");
        std::fs::copy(TML_FIXTURE, &tml_sidecar).expect("copy real Ariane .tml fixture");

        let project = client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            &fixture,
            &tml_sidecar,
            "live upload-200 test",
        )
        .expect("upload/auto of a fresh change should be 200");
        let commit = project
            .latest_commit
            .as_ref()
            .expect("200 body must carry a latest_commit (the adopted hexsha)");
        assert!(
            !commit.id.is_empty(),
            "the returned latest_commit.id should be a non-empty hexsha"
        );
        assert_eq!(project.id, cleanup.id, "200 body carries the project block");

        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }
    }

    /// #193: after an upload, `get_revisions` returns the project's LINEAR commit
    /// history - at least our commit, authored by us, with the tip matching the
    /// adopted hexsha and no merges. Validates the new `get_revisions` client + the
    /// linear-history contract the ancestry classifier (g7) assumes.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_get_revisions_returns_linear_authored_history() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let my_email = client::fetch_user_email(&url, &token, false).expect("whoami");
        let (cleanup, fixture, tml_dir) = seed_and_acquire(&url, &token);
        let tml_sidecar = tml_dir.path().join("ariane.tml");
        std::fs::copy(TML_FIXTURE, &tml_sidecar).expect("copy real Ariane .tml fixture");

        // upload_auto adopts the TOP-LEVEL `hexsha` (the commit this push created),
        // not the nested `project.latest_commit` (which is max-authored-date and
        // ties to the service-account root - see speleodb_server_head).
        let hexsha = client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            &fixture,
            &tml_sidecar,
            "live get_revisions test",
        )
        .expect("upload/auto should be 200")
        .latest_commit
        .expect("200 carries the adopted hexsha")
        .id;

        let history =
            client::get_revisions(&url, &token, false, &cleanup.id).expect("get_revisions");
        assert!(
            !history.commits.is_empty(),
            "history must carry at least our commit"
        );
        for c in &history.commits {
            assert!(
                c.parent_ids.len() <= 1,
                "history must be linear (no merges): {:?}",
                c.parent_ids
            );
        }
        // The TOPOLOGICAL leaf - the tip the fence trusts - must equal the commit
        // upload_auto just created (NOT necessarily the server's latest_commit).
        assert_eq!(
            super::sync::tip_of(&history.commits),
            Some(hexsha.as_str()),
            "the revision-topology HEAD must equal the just-pushed hexsha"
        );
        // That HEAD commit is authored by us - the property g7 ancestry keys off.
        let head = history
            .commits
            .iter()
            .find(|c| c.id == hexsha)
            .expect("the pushed commit must appear in the history");
        assert!(
            head.author_email.eq_ignore_ascii_case(&my_email),
            "the HEAD commit must be authored by us: {} vs {my_email}",
            head.author_email
        );

        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }
    }

    /// L13: re-uploading the **identical** tree (no change since the last commit)
    /// returns **304** - the benign "remote already at this state" outcome
    /// `map_status(UploadAuto, 304) == UpToDate` models. A 304 is a non-2xx, so the
    /// production `upload_auto` surfaces it as `Status { status: 304, .. }`.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_upload_304_when_unchanged() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let (cleanup, fixture, tml_dir) = seed_and_acquire(&url, &token);
        let tml_sidecar = tml_dir.path().join("ariane.tml");
        std::fs::copy(TML_FIXTURE, &tml_sidecar).expect("copy real Ariane .tml fixture");

        // First upload lands the tree (200).
        client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            &fixture,
            &tml_sidecar,
            "live upload-304 seed",
        )
        .expect("first upload should be 200");

        // Re-upload the identical tree → 304 (unchanged).
        let err = client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            &fixture,
            &tml_sidecar,
            "live upload-304 re-push",
        )
        .expect_err("an unchanged re-upload should not be a 2xx");
        assert!(
            matches!(&err, SpeleoDbError::Status { status: 304, .. }),
            "an unchanged re-upload should be HTTP 304 (UpToDate), got: {err:?}"
        );

        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }
    }

    /// L14: a malformed `upload/auto` payload (a multipart that omits the required
    /// `artifact` part, here a `message`-only form) is rejected with HTTP **400** -
    /// NOT 422 (verified live: this API answers 400 for malformed input across the
    /// board - bad creds, non-zip, missing artifact). A 400 on upload has no
    /// first-class `map_status` row, so it correctly falls to the fail-closed
    /// default (`map_status(UploadAuto, 400) == Fatal`). The 422 row is kept as a
    /// defensive fail-closed mapping but is not the missing-artifact status. Sent
    /// raw because the typed `upload_auto` always builds a well-formed two-part
    /// form, so the malformed case can only be produced with a hand-built
    /// `ureq::Form`.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_upload_malformed_returns_400() {
        use ureq::unversioned::multipart::Form;

        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let name = format!("stalagmight-upload400-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &token, false, &name)
            .expect("create project")
            .id;
        let cleanup = ProjectCleanup {
            url: &url,
            token: &token,
            id: project_id,
        };
        client::acquire_lock(&url, &token, false, &cleanup.id).expect("acquire mutex");

        // A deliberately malformed multipart: only the message text part, NO
        // `artifact` part - the server rejects it as a bad request → 400.
        let upload_url = api_url(&url, &format!("projects/{}/upload/auto/", cleanup.id));
        let resp = raw_agent()
            .put(&upload_url)
            .header("Authorization", &format!("Bearer {token}"))
            .send(Form::new().text("message", "deliberately malformed"))
            .expect("request should reach the server (status branched below)");
        assert_eq!(
            resp.status().as_u16(),
            400,
            "a malformed upload/auto (missing artifact) should be HTTP 400 (verified live; NOT 422)"
        );

        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }
    }

    /// F2 (#212) negative: a `PUT upload/auto/` by a writer who does **not** hold
    /// the project mutex is rejected with HTTP **403**. The upload view is gated by
    /// `[SDB_WriteAccess, UserOwnsProjectMutex]` (verified in the SpeleoDB server
    /// source, `api/v2/views/file.py` + `permissions.py`): the superuser has write access,
    /// so `SDB_WriteAccess` passes, but with no `active_mutex` held
    /// `UserOwnsProjectMutex.has_object_permission` returns `False` → DRF 403. This
    /// is the server half of the client's "hold the lock before you push" contract
    /// (`acquire_lock` first); the production push path never uploads unlocked.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_upload_without_mutex_returns_403() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let name = format!("stalagmight-nomutex-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &token, false, &name)
            .expect("create project")
            .id;
        let _cleanup = ProjectCleanup {
            url: &url,
            token: &token,
            id: project_id.clone(),
        };

        // Deliberately DO NOT acquire the mutex. Seed a real .stalx (+ .tml sidecar)
        // so the ONLY thing stopping the push is the missing lock.
        let seed_dir = tempfile::tempdir().expect("seed tempdir");
        let fixture = &seed_stalx(seed_dir.path());
        let tml_dir = tempfile::tempdir().expect("tml tempdir");
        let tml_sidecar = tml_dir.path().join("ariane.tml");
        std::fs::copy(TML_FIXTURE, &tml_sidecar).expect("copy real Ariane .tml fixture");

        let err = client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &project_id,
            fixture,
            &tml_sidecar,
            "push without holding the mutex",
        )
        .expect_err("an upload without the mutex must be rejected");
        assert!(
            matches!(&err, SpeleoDbError::Status { status: 403, .. }),
            "push without holding the mutex should be HTTP 403 (UserOwnsProjectMutex), got: {err:?}"
        );
    }

    // ── L15 / L16: the AUTO catch-all `.stalx` coupling + drift canary ──────

    /// L15: a `.stalx` pushed through `upload/auto` is stored **verbatim** by the
    /// server's AUTO catch-all and comes back in the dump - located by its `.stalx`
    /// **suffix**, never a hard-coded `misc/project.stalx` path (so it survives a
    /// server `TARGET_FOLDER` change). The bytes round-trip: the extracted `.stalx`
    /// opens as a real sqlite project.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        clippy::panic,
        reason = "ignored live integration test"
    )]
    fn live_stalx_round_trips_through_auto() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let (cleanup, fixture, tml_dir) = seed_and_acquire(&url, &token);
        let tml_sidecar = tml_dir.path().join("ariane.tml");
        std::fs::copy(TML_FIXTURE, &tml_sidecar).expect("copy real Ariane .tml fixture");
        client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            &fixture,
            &tml_sidecar,
            "live .stalx round-trip seed",
        )
        .expect("seed upload");
        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }

        // Pull the dump and locate the .stalx BY SUFFIX (the AUTO catch-all stored
        // it verbatim).
        let zip = client::download_dump(&client::TestWire, &url, &token, false, &cleanup.id)
            .expect("download dump");
        let mut d = dump::Dump::open(&zip).expect("open dump zip");
        let names = d.entry_paths().expect("read dump entries");
        let DumpClassification::Stalx(entry) = dump::classify(&names) else {
            panic!("expected exactly one .stalx (located by suffix) in the dump: {names:?}");
        };
        assert!(
            entry.ends_with(".stalx"),
            "the located entry must be found by its .stalx suffix, got: {entry}"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("project.stalx");
        d.extract_entry_to(&entry, &dest).expect("extract .stalx");
        let db = stalagmight_persistence::persistence::Database::open_existing(&dest)
            .expect("the round-tripped .stalx opens as sqlite");
        let project = db
            .load_project()
            .expect("load project from round-tripped .stalx");
        assert!(
            !project.name.is_empty() || !project.sections.is_empty(),
            "the round-tripped .stalx opened but looked empty"
        );
    }

    /// L16 - SERVER-DRIFT CANARY (kept `#[ignore]` like the rest). TODAY the
    /// server's AUTO catch-all (`BaseFileProcessor`) **accepts and stores** a file
    /// with an unknown extension. This test asserts that *current* behavior: the
    /// upload of an unknown-extension artifact succeeds. If the server ever starts
    /// **rejecting** unknown extensions, this test FAILS LOUDLY - that is the whole
    /// point. It is a tripwire on the `.stalx` catch-all coupling (D-T-stalx): the
    /// hand-off relies on the catch-all storing arbitrary extensions verbatim, so a
    /// behavior change here would silently break every push/pull. When this fails,
    /// investigate the server's processor allow-list before "fixing" the test.
    #[test]
    #[ignore = "SERVER-DRIFT CANARY; hits a live SpeleoDB box; needs DEV_SPELEODB_* (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_auto_rejects_unknown_extension_canary() {
        use ureq::unversioned::multipart::{Form, Part};

        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let name = format!("stalagmight-canary-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &token, false, &name)
            .expect("create project")
            .id;
        let cleanup = ProjectCleanup {
            url: &url,
            token: &token,
            id: project_id,
        };
        client::acquire_lock(&url, &token, false, &cleanup.id).expect("acquire mutex");

        // An artifact with an unknown extension - what the AUTO catch-all is
        // expected to accept TODAY.
        let dir = tempfile::tempdir().expect("tempdir");
        let blob = dir.path().join("mystery.zzz");
        std::fs::write(&blob, b"unknown-extension-bytes").expect("write blob");

        let upload_url = api_url(&url, &format!("projects/{}/upload/auto/", cleanup.id));
        let part = Part::file(&blob)
            .expect("part from file")
            .file_name("mystery.zzz")
            .mime_str("application/octet-stream")
            .expect("mime");
        let form = Form::new()
            .part("artifact", part)
            .text("message", "drift canary: unknown extension");
        let resp = raw_agent()
            .put(&upload_url)
            .header("Authorization", &format!("Bearer {token}"))
            .send(form)
            .expect("request should reach the server (status branched below)");
        let status = resp.status().as_u16();

        // CANARY ASSERTION: today the AUTO catch-all ACCEPTS the unknown extension.
        // A 2xx (200 stored / 304 unchanged) is the documented current behavior.
        // If the server starts rejecting unknown extensions (4xx), this fails
        // LOUDLY - the catch-all coupling has drifted; see the doc-comment.
        assert!(
            (200..400).contains(&status),
            "DRIFT: the AUTO catch-all rejected an unknown extension (HTTP {status}); \
             the .stalx hand-off relies on it storing arbitrary extensions verbatim \
             - investigate the server processor allow-list before editing this test"
        );

        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }
    }

    // ── L19 / L20: the #187 http private-network policy on the real wire ────

    /// L19: pointing the client at a **public** `http://` host is blocked **before
    /// any dial** with `SpeleoDbError::InsecureEndpoint` (the `tls::enforce`
    /// policy) - unless `insecure_http` is set, in which case the request is
    /// allowed to proceed (and fails for an unrelated reason, NOT InsecureEndpoint).
    /// Needs no box: the block happens client-side before the network. Uses a
    /// documentation/test public host so no real public SpeleoDB is contacted.
    #[test]
    #[ignore = "exercises the #187 http policy; gated with the live suite (run with --ignored)"]
    #[expect(clippy::print_stderr, reason = "ignored live integration test")]
    fn live_http_to_public_blocked() {
        // This one needs no env at all, but stays #[ignore] with the live suite so
        // it never runs in CI's plain `cargo test`. A public http host:
        let public = "http://example.com";

        // Blocked before dial when insecure_http is false.
        let blocked = client::list_projects(public, "tok", false);
        assert!(
            matches!(blocked, Err(SpeleoDbError::InsecureEndpoint { .. })),
            "public http must be blocked before dial, got: {blocked:?}"
        );

        // With insecure_http = true the scheme block is lifted, so enforce no
        // longer short-circuits: the call proceeds to the network and fails for
        // some OTHER reason (transport/status), never InsecureEndpoint.
        let allowed = client::list_projects(public, "tok", true);
        assert!(
            !matches!(allowed, Err(SpeleoDbError::InsecureEndpoint { .. })),
            "insecure_http must lift the public-http block, got: {allowed:?}"
        );
        eprintln!("live_http_to_public_blocked: insecure_http path resolved to {allowed:?}");
    }

    /// L20: a `GET /projects/` to the real **private** box over plain
    /// `http://10.10.0.220:8000/` (RFC-1918) is **allowed** by the #187 policy
    /// without `insecure_http` - `classify_addr` puts the box in the private range.
    /// The whole single-account round-trip already exercises this, so here we just
    /// assert the http private endpoint is not rejected by the TLS policy.
    #[test]
    #[ignore = "hits a live SpeleoDB box over http; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_http_to_private_box_allowed() {
        let (url, email, password) = live_env();
        // An https box has nothing to prove about the http private policy, so
        // this test needs the suite pointed at a plain-http one.
        if !url.starts_with("http://") {
            missing_prerequisite(&format!(
                "DEV_SPELEODB_URL naming an http:// box (it is {url})"
            ));
        }
        let token = client::login(&url, false, &email, &password).expect("login over private http");
        // A successful list proves enforce() did NOT block the private http box.
        let projects = client::list_projects(&url, &token, false)
            .expect("private http box must be allowed by the #187 policy");
        // `projects` may legitimately be empty; the point is no InsecureEndpoint.
        eprintln!(
            "live_http_to_private_box_allowed: listed {} project(s) over private http",
            projects.len()
        );
    }

    // ── #184: P0.5 render-compat gate (GATES P1 render/push) ────────────────
    //
    // Prove SpeleoDB's `openspeleo_lib` accepts OUR exporter's `.tml` and
    // regenerates a `ProjectGeoJSON`. A 200 upload does NOT prove a render: the
    // server generates the GeoJSON best-effort *after* the commit and silently
    // `continue`s on `NoKnownAnchorError` / `EmptySurveyError` / any `Exception`
    // (SpeleoDB `api/v2/views/file.py`), so the push still 200s with no render.
    // The only honest proof is to read the render back via the OGC API - Features
    // `items`:
    //   GET /api/v2/gis-ogc/user/<token>/collections/<sha>_{points,lines}/items
    // The user token authenticates via the URL path (`user_token` converter =
    // 40-hex), so no Authorization header is sent; the endpoint 404s when no
    // `ProjectGeoJSON` exists for that commit + geometry group.

    /// Build an in-memory project with one section of two real shots. With
    /// `anchor = Some((lat, lon))` it carries a georeferenced zero datum, so
    /// [`export_tml_for_speleodb`](stalagmight_export::export::tml::export_tml_for_speleodb)
    /// writes a real GPS anchor on the exported START shot - the thing `openspeleo_lib` needs to emit a
    /// `ProjectGeoJSON`. With `None` the exported `.tml` START is 0/0 (no anchor).
    #[expect(clippy::expect_used, reason = "ignored live integration test helper")]
    fn build_render_project(anchor: Option<(f64, f64)>) -> crate::models::Project {
        use crate::models::{
            GpsCoordinates, Project, SectionData, ShotData, StationRef, ZeroDatum,
        };
        let mut project = Project::new("stalagmight-render-compat".to_owned(), None);
        let section_id = project
            .add_section(SectionData {
                name: Some("Main".to_owned()),
                surveyor: Some("Spike".to_owned()),
                color: "#ff0000".to_owned(),
                shots: vec![
                    ShotData {
                        azimuth_in: 45.0,
                        azimuth_out: 225.0,
                        length: 10.0,
                        depth_in: 0.0,
                        depth_out: 3.0,
                        ..Default::default()
                    },
                    ShotData {
                        azimuth_in: 90.0,
                        azimuth_out: 270.0,
                        length: 8.0,
                        depth_in: 3.0,
                        depth_out: 5.0,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
            .id;
        if let Some((lat, lon)) = anchor {
            project.zero_datums.push(ZeroDatum {
                station_ref: StationRef::new(section_id, 0).into(),
                gps: GpsCoordinates::new(lat, lon, 0.0).expect("valid GPS anchor"),
            });
        }
        project
    }

    /// GET the OGC API - Features `items` for `<commit_sha>_<group>` (group =
    /// `"points"` | `"lines"`). Returns `(http_status, body_json)`; the body is a
    /// bare `FeatureCollection` on 200 and `Value::Null` otherwise (e.g. 404 when
    /// the server generated no render for that commit + geometry group).
    #[expect(clippy::expect_used, reason = "ignored live integration test helper")]
    fn fetch_ogc_items(
        url: &str,
        token: &str,
        commit_sha: &str,
        group: &str,
    ) -> (u16, serde_json::Value) {
        let items_url = api_url(
            url,
            &format!("gis-ogc/user/{token}/collections/{commit_sha}_{group}/items"),
        );
        let resp = raw_agent()
            .get(&items_url)
            .call()
            .expect("OGC items request should reach the server (status branched below)");
        let status = resp.status().as_u16();
        let body = resp.into_body().read_to_vec().unwrap_or_default();
        let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    /// The feature count of a bare OGC `items` FeatureCollection, or `None` when
    /// the body is not a `FeatureCollection` (non-200 / unexpected shape).
    fn feature_count(body: &serde_json::Value) -> Option<usize> {
        if body.get("type").and_then(serde_json::Value::as_str) != Some("FeatureCollection") {
            return None;
        }
        body.get("features")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
    }

    /// #184 (single-account): a Stalagmight-exported **georeferenced** `.tml`,
    /// pushed via the production two-`artifact` `upload/auto`, makes SpeleoDB
    /// regenerate a `ProjectGeoJSON` whose OGC layers carry features. THIS IS THE
    /// GATE: if it fails, our exporter output is not accepted by `openspeleo_lib`
    /// and the P1 render/push design must change (fix the exporter or fast-track
    /// the #219 GeoJSON-acceptance ask) before building on it.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_render_compat_georef_generates_geojson() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let name = format!("stalagmight-render-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &token, false, &name)
            .expect("create project")
            .id;
        let cleanup = ProjectCleanup {
            url: &url,
            token: &token,
            id: project_id,
        };

        // Export OUR georeferenced .tml (not a static fixture) - this is the
        // exporter output whose render-compat we are gating.
        let project = build_render_project(Some((20.25, -87.5)));
        let dir = tempfile::tempdir().expect("tempdir");
        let tml = dir.path().join("ariane.tml");
        stalagmight_export::export::tml::export_tml_for_speleodb(&project, &tml)
            .expect("export georeferenced .tml");

        // The .stalx part is a stand-in (the render is driven entirely by the
        // .tml); reuse the synthetic seed, as the other upload tests do.
        let stalx = &seed_stalx(dir.path());
        client::acquire_lock(&url, &token, false, &cleanup.id).expect("acquire mutex");
        let uploaded = client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            stalx,
            &tml,
            "render-compat: georeferenced push",
        )
        .expect("georeferenced upload should be 200");
        let hexsha = uploaded
            .latest_commit
            .as_ref()
            .expect("200 upload must carry the new commit")
            .id
            .clone();
        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }

        // Read the render back (a 200 push alone does not prove generation).
        let (points_status, points_body) = fetch_ogc_items(&url, &token, &hexsha, "points");
        let (lines_status, lines_body) = fetch_ogc_items(&url, &token, &hexsha, "lines");
        let points = feature_count(&points_body);
        let lines = feature_count(&lines_body);
        eprintln!(
            "render-compat georef: commit={hexsha} \
             points(status={points_status}, count={points:?}) \
             lines(status={lines_status}, count={lines:?})"
        );

        let points_ok = points_status == 200 && points.unwrap_or(0) > 0;
        let lines_ok = lines_status == 200 && lines.unwrap_or(0) > 0;
        assert!(
            points_ok || lines_ok,
            "RENDER-COMPAT GATE FAILED: our exported georeferenced .tml did not produce a \
             ProjectGeoJSON for commit {hexsha} (points status={points_status}, lines \
             status={lines_status}). SpeleoDB's openspeleo_lib did not accept our exporter \
             output - fix the exporter or fast-track the #219 GeoJSON-acceptance ask before \
             building P1 render/push on this path."
        );
    }

    /// #184 (single-account): a push with **no** georeferenced anchor still
    /// succeeds (the push is never blocked - the D2 "no-GPS-anchor" banner case)
    /// but generates no new render for that commit. OBSERVATIONAL: the exact
    /// no-anchor detection inside `openspeleo_lib` (does 0/0 count as "no known
    /// anchor"?) is what this spike records; it asserts only the load-bearing
    /// invariant (the push is not blocked) and prints the render statuses so the
    /// D2 behavior is captured in the run log.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_render_compat_no_anchor_records_geojson_state() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let name = format!("stalagmight-noanchor-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &token, false, &name)
            .expect("create project")
            .id;
        let cleanup = ProjectCleanup {
            url: &url,
            token: &token,
            id: project_id,
        };

        let project = build_render_project(None); // no zero datum → 0/0 START
        let dir = tempfile::tempdir().expect("tempdir");
        let tml = dir.path().join("ariane.tml");
        stalagmight_export::export::tml::export_tml_for_speleodb(&project, &tml)
            .expect("export un-georeferenced .tml");

        let stalx = &seed_stalx(dir.path());
        client::acquire_lock(&url, &token, false, &cleanup.id).expect("acquire mutex");
        let uploaded = client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            stalx,
            &tml,
            "render-compat: no-anchor push",
        )
        .expect("no-anchor upload must still succeed (push is never blocked, D2)");
        let hexsha = uploaded
            .latest_commit
            .as_ref()
            .expect("200 upload must carry the new commit")
            .id
            .clone();
        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }

        let (points_status, _) = fetch_ogc_items(&url, &token, &hexsha, "points");
        let (lines_status, _) = fetch_ogc_items(&url, &token, &hexsha, "lines");
        eprintln!(
            "render-compat no-anchor: commit={hexsha} points status={points_status} \
             lines status={lines_status} (expect non-200 / 404 → no render → D2 banner path)"
        );
    }

    /// #184 / OPEN-1 (single-account): does a bare `.tml` multipart part (no
    /// explicit `Content-Type`) still upload, or does SpeleoDB reject it? The
    /// server selects the processor by **extension** (`auto_selector.py`: `.tml`
    /// → `ArianeTMLFileProcessor`) then validates the part's `Content-Type`
    /// against `ALLOWED_MIMETYPES = ["application/octet-stream", "application/zip"]`
    /// (`processors/_impl/ariane.py`; enforced in `processors/artifact.py`
    /// `assert_valid`) - `text/plain` is NOT allowed. Our production `upload_auto`
    /// always sends `application/octet-stream`, so this probe pushes the `.tml`
    /// part WITHOUT an explicit MIME to discover what `ureq` defaults to and
    /// whether that default is accepted. OBSERVATIONAL: it prints the verdict that
    /// resolves OPEN-1 (whether the explicit per-part MIME in `client::mime_for`
    /// is REQUIRED, not just defensive). Asserts only that a real HTTP response
    /// came back.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(
        clippy::expect_used,
        clippy::print_stderr,
        reason = "ignored live integration test"
    )]
    fn live_open1_bare_tml_mime_outcome() {
        use ureq::unversioned::multipart::{Form, Part};

        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let name = format!("stalagmight-open1-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &token, false, &name)
            .expect("create project")
            .id;
        let cleanup = ProjectCleanup {
            url: &url,
            token: &token,
            id: project_id,
        };
        client::acquire_lock(&url, &token, false, &cleanup.id).expect("acquire mutex");

        let project = build_render_project(Some((20.25, -87.5)));
        let dir = tempfile::tempdir().expect("tempdir");
        let tml = dir.path().join("ariane.tml");
        stalagmight_export::export::tml::export_tml_for_speleodb(&project, &tml)
            .expect("export georeferenced .tml");

        // A single `.tml` artifact part with NO explicit `.mime_str(...)` - let
        // ureq pick the Content-Type, then see whether the server accepts it.
        let part = Part::file(&tml)
            .expect("part from .tml file")
            .file_name("ariane.tml");
        let form = Form::new()
            .part("artifact", part)
            .text("message", "OPEN-1: bare .tml mime probe");
        let upload_url = api_url(&url, &format!("projects/{}/upload/auto/", cleanup.id));
        let resp = raw_agent()
            .put(&upload_url)
            .header("Authorization", &format!("Bearer {token}"))
            .send(form)
            .expect("request should reach the server (status branched below)");
        let status = resp.status().as_u16();
        eprintln!(
            "OPEN-1 RESULT: bare .tml part (no explicit Content-Type) → upload HTTP {status}. \
             Server allows only Content-Type in {{application/octet-stream, application/zip}} for \
             .tml (ArianeTMLFileProcessor.ALLOWED_MIMETYPES). 400 => explicit per-part MIME is \
             REQUIRED (the client.rs `mime_for` 'robustness, not a hard requirement' comment is \
             wrong - correct it); 2xx => ureq's default part Content-Type is already accepted, so \
             the explicit MIME is belt-and-suspenders."
        );
        assert!(
            (200..500).contains(&status),
            "expected a real HTTP response from upload/auto, got {status}"
        );

        if let Err(e) = client::release_lock(&url, &token, false, &cleanup.id) {
            eprintln!("cleanup: failed to release lock: {e:?}");
        }
    }

    /// #202: the lazy lock re-verify (`speleodb_verify_lock_if_expired`) branches on
    /// exactly one field of the single-project GET - `active_mutex.user` (is the lock
    /// still ours? via `lock_held_by_me`). This pins that both `client::get_project`
    /// and the short-budget `client::get_project_probe` surface it against the live
    /// box: while we hold the mutex it names us as the holder; after we release,
    /// `active_mutex` goes ABSENT - the lost-lock signal that drops the edit gate and
    /// routes the user to the divergence prompt. (#202 no longer compares the server
    /// tip: `latest_commit` is `max(authored_date)` and documented unreliable as a
    /// tip, see `speleodb_server_head`, so the holder check is the whole signal.)
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_get_project_reflects_holder_and_tip() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let my_email = client::fetch_user_email(&url, &token, false).expect("whoami");
        let (cleanup, fixture, tml_dir) = seed_and_acquire(&url, &token);
        let tml_sidecar = tml_dir.path().join("ariane.tml");
        std::fs::copy(TML_FIXTURE, &tml_sidecar).expect("copy real Ariane .tml fixture");

        // Land a commit, then GET the single project while still locked.
        client::upload_auto(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            &fixture,
            &tml_sidecar,
            "live get_project reverify test",
        )
        .expect("seed upload");

        // Lock still ours: the short-budget probe the re-verify actually uses names
        // us as the holder. This is the "still ours" path the re-verify allows.
        let held = client::get_project_probe(&url, &token, false, &cleanup.id)
            .expect("get_project_probe while we hold the mutex");
        let holder = held
            .active_mutex
            .as_ref()
            .map(|m| m.user.as_str())
            .expect("active_mutex must name the lock holder while we hold it");
        assert!(
            holder.eq_ignore_ascii_case(&my_email),
            "get_project must report us as the lock holder: {holder} vs {my_email}"
        );

        // Release, then GET again: active_mutex goes ABSENT. `lock_held_by_me(None,
        // _)` is false - exactly the lost-lock signal #202 turns into a divergence
        // prompt.
        client::release_lock(&url, &token, false, &cleanup.id).expect("release mutex");
        let freed = client::get_project(&url, &token, false, &cleanup.id)
            .expect("get_project after releasing the mutex");
        assert!(
            freed.active_mutex.is_none(),
            "after release, get_project active_mutex must be absent (the lost-lock signal), got: {:?}",
            freed.active_mutex
        );
    }

    /// #201: the held-lock resume/release prompt's "Release all" choice calls
    /// `client::release_all_locks`, which must drop EVERY write lock this token
    /// holds in one request, and be safe to repeat (the client marks it idempotent
    /// so a transient-transport retry can't error). Hold the mutex on two throwaway
    /// projects, release all, and assert both are freed via get_project; then call
    /// again and assert the empty-handed no-op still succeeds.
    ///
    /// NOTE: this releases ALL locks held by the configured test account, so run it
    /// only against the throwaway dev box - it will also drop any unrelated lock the
    /// same account happens to hold (matching the one-session-per-account rule in
    /// docs/speleodb-dev-testing.md).
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_release_all_locks_releases_held() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");

        // Two throwaway projects, both locked by us, so "release ALL" is meaningful.
        let name_a = format!("stalagmight-relall-a-{}", uuid::Uuid::new_v4());
        let id_a = client::create_project(&url, &token, false, &name_a)
            .expect("create project A")
            .id;
        let cleanup_a = ProjectCleanup {
            url: &url,
            token: &token,
            id: id_a,
        };
        client::acquire_lock(&url, &token, false, &cleanup_a.id).expect("acquire mutex A");

        let name_b = format!("stalagmight-relall-b-{}", uuid::Uuid::new_v4());
        let id_b = client::create_project(&url, &token, false, &name_b)
            .expect("create project B")
            .id;
        let cleanup_b = ProjectCleanup {
            url: &url,
            token: &token,
            id: id_b,
        };
        client::acquire_lock(&url, &token, false, &cleanup_b.id).expect("acquire mutex B");

        // One request drops both locks.
        client::release_all_locks(&url, &token, false).expect("release_all_locks");

        for id in [&cleanup_a.id, &cleanup_b.id] {
            let project = client::get_project(&url, &token, false, id)
                .expect("get_project after release_all_locks");
            assert!(
                project.active_mutex.is_none(),
                "release_all_locks must clear the mutex on {id}, still held: {:?}",
                project.active_mutex
            );
        }

        // Idempotent: with nothing held, a second call is a no-op that still succeeds.
        client::release_all_locks(&url, &token, false)
            .expect("release_all_locks must be a safe no-op when no locks are held");
    }

    /// #200: the "Enable rendering" action PATCHes `exclude_geojson = false` via
    /// `client::patch_project`. Pin that the field is genuinely writable on the
    /// live box and round-trips: flip it to `true`, confirm both the PATCH response
    /// and a follow-up GET report `true`, then flip it back to `false` (the shipped
    /// action) and confirm the cleared state. No lock is taken - the server gates
    /// the metadata PATCH on write permission, not the mutex.
    #[test]
    #[ignore = "hits a live SpeleoDB box; needs DEV_SPELEODB_* env (run with --ignored)"]
    #[expect(clippy::expect_used, reason = "ignored live integration test")]
    fn live_patch_exclude_geojson_roundtrips() {
        let (url, email, password) = live_env();
        let token = client::login(&url, false, &email, &password).expect("login");
        let name = format!("stalagmight-excljson-{}", uuid::Uuid::new_v4());
        let project_id = client::create_project(&url, &token, false, &name)
            .expect("create project")
            .id;
        let cleanup = ProjectCleanup {
            url: &url,
            token: &token,
            id: project_id,
        };

        // Flip rendering OFF: the PATCH response reflects the new value...
        let patched = client::patch_project(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            r#"{"exclude_geojson": true}"#,
        )
        .expect("patch exclude_geojson = true");
        assert!(
            patched.exclude_geojson,
            "PATCH response must carry exclude_geojson = true"
        );
        // ...and it persists (a fresh GET sees it).
        let after = client::get_project(&url, &token, false, &cleanup.id)
            .expect("get_project after the exclude_geojson PATCH");
        assert!(
            after.exclude_geojson,
            "get_project must report the persisted exclude_geojson = true"
        );

        // The shipped Enable-rendering action: flip it back to false.
        let re_enabled = client::patch_project(
            &client::TestWire,
            &url,
            &token,
            false,
            &cleanup.id,
            r#"{"exclude_geojson": false}"#,
        )
        .expect("patch exclude_geojson = false (Enable rendering)");
        assert!(
            !re_enabled.exclude_geojson,
            "Enable-rendering PATCH must clear exclude_geojson"
        );
    }
}
