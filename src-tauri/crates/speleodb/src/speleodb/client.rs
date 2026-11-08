//! ureq HTTP client for the SpeleoDB v2 API (#183 read-only spike → #185
//! production transport).
//!
//! Reuses the tile-fetch transport idiom (`lib.rs`): a fresh `ureq::Agent` per
//! attempt with a global timeout, and a retry loop over transient failures. The
//! agent is configured with `http_status_as_error(false)` so non-2xx responses
//! are returned (not turned into errors) and this module branches on the status
//! itself - the shape #186's status→action table builds on.
//!
//! Auth uses `Authorization: Bearer <token>` (verified: `BearerAuthentication`,
//! `authentication.py:59`).
//!
//! Transport policy (#185/#187/D-A-retry/D-B3):
//! - [`with_retry`] enforces the http private-network policy ([`super::tls`]) on
//!   every request before the first attempt, then runs the closure with backoff.
//! - The closure returns `(status, body)` so a 5xx is visible to the retry loop.
//!   Idempotent (read-like) calls retry a transient 5xx; non-idempotent writes
//!   (`idempotent == false`) run **exactly once** - a mid-response failure must
//!   never double-submit a commit / lock / create.

use std::path::{Path, PathBuf};
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use super::metrics::{Endpoint, Outcome};
use super::models::{
    Commit, Permission, Project, RevisionHistory, Team, TeamPermission, TeamPermissionList,
    UserAutocomplete, UserPermission, UserPermissionList,
};
use super::{SpeleoDbError, TransportCause};

/// Global per-request timeout for reads. Larger than the tile timeout because a
/// project dump (whole working tree as a ZIP) can be bigger than a single tile.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Upload (multipart `PUT`) timeout - a full `.stalx` + `.tml` push is larger and
/// slower than a read, so it gets its own, longer budget (D-A-methods).
// (Referenced by `upload_auto`, so rustc does not flag it dead even while that
// method's orchestration caller is pending - no dead_code expectation needed.)
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Dump-download timeout (#270). The dump is the whole working tree as a ZIP - the
/// same payload the upload sends back - so it does NOT belong on the shared
/// small-GET budget: `timeout_global` is end-to-end *including the body read*, so a
/// big project on a slow link blows the 30 s read budget on every one of the three
/// idempotent attempts while a same-size upload (120 s) succeeds. Named separately
/// from [`UPLOAD_TIMEOUT`] so the two budgets can drift apart later.
const DUMP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// The wire filename every push gives the `.stalx`, and therefore the ONE entry a
/// Stalagmight-managed repo is supposed to contain.
///
/// Single-sourced because two distant places must agree on it: the push writes it
/// ([`build_upload_parts`]), and the #258 dump picker flags the candidate matching
/// it so a user resolving an ambiguous dump is not silently steered into replacing
/// the canonical file with a stray one.
pub(crate) const CANONICAL_STALX_WIRE_NAME: &str = "project.stalx";
/// Body-size ceiling for a dump download (#270).
///
/// `Body::read_to_vec()` silently applies ureq's own default ceiling of 10 MiB,
/// so raising [`DUMP_DOWNLOAD_TIMEOUT`] alone fixed nothing for exactly the
/// projects it was meant to help: a dump over 10 MiB could never be opened,
/// restored or fast-forwarded at ANY timeout. Worse, the refusal arrives as an
/// `io::Error` wrapping `BodyExceedsLimit`, which lands in
/// [`classify_transport`]'s catch-all - so a healthy `200` was reported to the
/// user as "Could not reach SpeleoDB at the configured endpoint", and retried
/// three times first.
///
/// The dump is the whole working tree as a ZIP - the same payload the (uncapped)
/// upload sends back - so the ceiling exists only as a sanity bound against a
/// runaway response, not as a project-size policy. It must stay far above any
/// plausible survey (the "a dump over 10 MiB must still be openable" property is
/// the whole point), but NOT arbitrarily far: the whole body is read into memory
/// (`read_to_vec` is `read_to_end` into a `Vec`), and `Dump::extract_entry_capped`
/// then builds a SECOND buffer of up to `dump::MAX_ENTRY_BYTES` + 1 on top of it,
/// so the ceiling is a peak-RSS budget as much as a sanity bound - and this app
/// already has documented WebKitGTK memory-ceiling trouble.
///
/// 640 MiB, chosen to be COHERENT with `MAX_ENTRY_BYTES` (512 MiB): the extractable
/// `.stalx` is what a dump is downloaded for, so a body whose every entry is over
/// that per-entry cap can never yield one. The earlier 2 GiB left 1.5 GiB of
/// allowed body that could only ever be downloaded, held, and then rejected - a
/// guaranteed-futile multi-gigabyte allocation. The remaining 128 MiB is headroom
/// for ZIP overhead plus the dump's other entries (`ariane.tml`, `misc/`).
///
/// Streaming the body to the staging temp file instead of buffering would retire
/// the budget question entirely, but `Dump` parses an in-memory `&[u8]` and all
/// four pull flows are built on that, so it is its own change.
const DUMP_MAX_BYTES: u64 = 640 * 1024 * 1024;
/// ureq's own default body ceiling, applied by a bare `Body::read_to_vec()` - i.e.
/// the limit that actually bites on every endpoint except the two dump downloads.
/// Mirrored here (ureq does not export it) so [`with_retry`] can name the limit
/// that was really exceeded instead of quoting the dump one at a JSON endpoint.
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Transient-transport retries (total attempts = 1 + MAX_RETRIES).
const MAX_RETRIES: u32 = 2;
/// Backoff delays between retries (mirrors the tile-fetch idiom).
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(500)];

/// Build a fresh agent with the read timeout. `http_status_as_error(false)` makes
/// non-2xx a normal `Response` so the caller branches on the status code itself.
fn agent() -> ureq::Agent {
    agent_with_timeout(REQUEST_TIMEOUT)
}

/// Build a fresh agent for a dump download, on the longer
/// [`DUMP_DOWNLOAD_TIMEOUT`] budget. Both dump endpoints go through here so
/// neither can silently fall back to the small-GET budget.
fn dump_agent() -> ureq::Agent {
    agent_with_timeout(DUMP_DOWNLOAD_TIMEOUT)
}

/// Build a fresh agent with an explicit global timeout (upload uses the longer
/// [`UPLOAD_TIMEOUT`], the dump download [`DUMP_DOWNLOAD_TIMEOUT`]).
/// `http_status_as_error(false)` is kept so the status branching shape #186 keys
/// off is preserved.
///
/// `max_redirects(0)` is a SECURITY setting, not a tidiness one. The http /
/// private-network policy ([`super::tls::enforce`]) runs once per request against
/// the CONFIGURED base URL, so it says nothing about where a redirect goes. With
/// ureq's default of 10 redirects, a `302` to `http://<anything>/` was followed in
/// plaintext straight past the policy that exists to forbid exactly that - the
/// endpoint the policy vetted is not the endpoint the credentials-bearing request
/// ends up at. Zero redirects means the 3xx comes back as an ordinary status and
/// is reported by [`status_error`] like any other unexpected response.
///
/// (Redirected credentials were never the exposure here: ureq defaults to
/// `RedirectAuthHeaders::Never`, so the `Authorization` header is dropped on a
/// cross-host hop. Verified against the pinned ureq; the plaintext-to-anywhere hole
/// is what this closes. The v2 API is not expected to redirect at all, so nothing
/// legitimate is lost.)
fn agent_with_timeout(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .new_agent()
}

fn api_url(base: &str, suffix: &str) -> String {
    format!(
        "{}/api/v2/{}",
        base.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    )
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Run `attempt`, enforcing the http private-network policy first and then
/// retrying transient failures with backoff.
///
/// `base` + `insecure_http` are threaded so [`super::tls::enforce`] runs once at
/// the top - the single chokepoint a new method cannot forget (#187 step 5). The
/// same chokepoint carries the #210 traffic instrumentation: `endpoint` labels
/// each attempt so [`super::metrics::record`] emits its count + latency + outcome
/// (a new method cannot forget that either - it must pass an [`Endpoint`]).
///
/// The closure returns `(status, body)`. With `http_status_as_error(false)` every
/// `Err` is transport-level (DNS, timeout, connection reset, mid-body read).
/// Retry rules (D-A-retry / D-B3):
/// - `idempotent` reads retry a transport `Err` **and** an idempotent 5xx (read
///   the body, treat a 500-599 like a transient error).
/// - non-idempotent writes (`idempotent == false`, `max_retries == 0`) run
///   exactly once and surface a 5xx straight through - retrying could
///   double-submit (the response failure can land after the server committed).
fn with_retry(
    base: &str,
    insecure_http: bool,
    idempotent: bool,
    endpoint: Endpoint,
    mut attempt: impl FnMut() -> Result<(u16, Vec<u8>), ureq::Error>,
) -> Result<(u16, Vec<u8>), SpeleoDbError> {
    // #187: block public `http://` / scheme-less endpoints before any dial. A
    // policy block is not server traffic, so it is intentionally not recorded.
    super::tls::enforce(base, insecure_http)?;

    let max_retries = if idempotent { MAX_RETRIES } else { 0 };
    let mut last_err: Option<(TransportCause, String)> = None;
    for i in 0..=max_retries {
        if i > 0 {
            let delay = RETRY_DELAYS
                .get((i - 1) as usize)
                .copied()
                .unwrap_or(REQUEST_TIMEOUT);
            std::thread::sleep(delay);
        }
        // #210: time each attempt (the backoff sleep above is excluded) and
        // record it as one server hit - a retried 5xx really re-hits the server,
        // so counting per attempt is what makes a retry storm legible.
        let started = std::time::Instant::now();
        let result = attempt();
        let elapsed = started.elapsed();
        let outcome = match &result {
            Ok((status, _)) => Outcome::from_status(*status),
            Err(_) => Outcome::NetworkError,
        };
        super::metrics::record(endpoint, outcome, elapsed);
        match result {
            Ok((status, body)) => {
                // D-B3: an idempotent 5xx is treated like a transient error so it
                // is retried with backoff; a write (idempotent == false) returns
                // the 5xx immediately (it already ran, do not re-run it).
                if idempotent && i < max_retries && (500..600).contains(&status) {
                    tracing::debug!(
                        status,
                        attempt = i + 1,
                        "SpeleoDB idempotent request returned 5xx; will retry"
                    );
                    last_err = Some((TransportCause::Unknown, format!("HTTP {status}")));
                    continue;
                }
                return Ok((status, body));
            }
            Err(e) if is_body_limit(&e) => {
                // Not transient and not a reachability problem: the server
                // answered fine, the response is just bigger than we will hold.
                // Retrying re-downloads the whole oversized body to fail again,
                // and reporting it as "cannot reach the endpoint" sends the user
                // looking at their network for a payload-size refusal.
                let limit = body_limit_for(endpoint);
                tracing::error!(
                    ?endpoint,
                    limit,
                    "SpeleoDB response body exceeded the download ceiling"
                );
                return Err(SpeleoDbError::Parse(format!(
                    "SpeleoDB's reply was larger than the {} this app will hold in memory. \
                     If this was a project download, the project is too large for this \
                     version to open - report it so the limit can be raised.",
                    human_bytes(limit)
                )));
            }
            Err(e) => {
                tracing::debug!(error = %e, attempt = i + 1, "SpeleoDB request failed; will retry if attempts remain");
                last_err = Some((classify_transport(&e), clean_transport(&e.to_string())));
            }
        }
    }
    let (cause, detail) = last_err.unwrap_or((
        TransportCause::Unknown,
        "unknown transport error".to_owned(),
    ));
    // [#270] The user is shown a translated "could not reach <endpoint>" line, so
    // this is where the RAW cause has to survive: log it (with the classification
    // and which endpoint failed) or a bug report has nothing to go on.
    tracing::warn!(
        ?endpoint,
        ?cause,
        detail = %detail,
        "SpeleoDB transport failure (attempts exhausted)"
    );
    Err(SpeleoDbError::Transport { cause, detail })
}

/// Classify a transport failure from the TYPED `ureq::Error` (and, for io
/// errors, [`std::io::ErrorKind`]) - never from its Display text: an
/// `io::Error`'s message comes from the platform's `strerror`, so it is OS- and
/// locale-dependent and cannot be matched on. The classification crosses IPC and
/// picks which translated line the frontend shows (#270).
fn classify_transport(err: &ureq::Error) -> TransportCause {
    match err {
        ureq::Error::Timeout(_) => TransportCause::Timeout,
        ureq::Error::HostNotFound => TransportCause::Dns,
        ureq::Error::Tls(_) => TransportCause::Tls,
        ureq::Error::ConnectionFailed => TransportCause::Refused,
        ureq::Error::Io(e) => match e.kind() {
            std::io::ErrorKind::TimedOut => TransportCause::Timeout,
            std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotConnected => TransportCause::Refused,
            // Everything else - EINVAL from a blackholed route, no-route,
            // network-down, an unexpected io kind - is a plain "cannot reach it".
            _ => TransportCause::Unknown,
        },
        _ => TransportCause::Unknown,
    }
}

/// Strip `ureq`/std-io noise from a transport error string so it reads as a
/// user-facing detail: drop a leading `io: ` and a trailing ` (os error N)`.
/// (e.g. `io: Connection refused (os error 111)` → `Connection refused`.)
fn clean_transport(raw: &str) -> String {
    let s = raw.strip_prefix("io: ").unwrap_or(raw);
    let s = match s.rfind(" (os error ") {
        Some(idx) if s.ends_with(')') => &s[..idx],
        _ => s,
    };
    s.trim().to_owned()
}

/// Read a response into `(status, body)` for [`with_retry`]'s closure. A
/// body-read failure is a transport `Err` (so an idempotent call retries it).
fn read_status_body(resp: http::Response<ureq::Body>) -> Result<(u16, Vec<u8>), ureq::Error> {
    let status = resp.status().as_u16();
    let body = resp.into_body().read_to_vec()?;
    Ok((status, body))
}

/// [`read_status_body`] on an explicit body ceiling.
///
/// The limit is a parameter purely so the over-limit path can be exercised for
/// real in a test - see `an_over_limit_read_is_reported_as_a_size_refusal`, which
/// is what pins [`is_body_limit`]'s Display match to ureq's actual wording. In
/// production there is exactly one caller and one limit.
fn read_body_limited(
    resp: http::Response<ureq::Body>,
    limit: u64,
) -> Result<(u16, Vec<u8>), ureq::Error> {
    let status = resp.status().as_u16();
    let body = resp.into_body().with_config().limit(limit).read_to_vec()?;
    Ok((status, body))
}

/// [`read_status_body`] for a dump download, on the [`DUMP_MAX_BYTES`] ceiling
/// instead of the 10 MiB one `read_to_vec()` applies by default.
fn read_dump_body(resp: http::Response<ureq::Body>) -> Result<(u16, Vec<u8>), ureq::Error> {
    read_body_limited(resp, DUMP_MAX_BYTES)
}

/// Whether a transport failure is the body-size ceiling being hit.
///
/// It is neither transient (retrying re-downloads the same oversized body) nor a
/// reachability problem, so it must not be classified as one.
///
/// Matched STRUCTURALLY on ureq's own variant. This used to test
/// `Error::Io(e) if e.to_string().contains("larger than request limit")`, on the
/// premise that the limit reader's `Error::BodyExceedsLimit(_).into_io()` meant
/// the failure surfaced as an `Io`. It does not: `From<io::Error> for ureq::Error`
/// detects its own boxed error and unwraps it, so what arrives here is
/// `BodyExceedsLimit` again - and the arm never matched. Every oversized response
/// therefore fell through to the retry-then-"could not reach SpeleoDB" path this
/// exists to prevent, after three full re-downloads of the payload. Nothing caught
/// it because nothing provoked a real over-limit read; the test below now does
/// (`an_over_limit_read_is_reported_as_a_size_refusal_not_a_retried_transport_failure`).
fn is_body_limit(err: &ureq::Error) -> bool {
    matches!(err, ureq::Error::BodyExceedsLimit(_))
}

/// Which body ceiling actually applied to `endpoint`.
///
/// [`with_retry`] is shared by all thirteen endpoints, but only the two dump
/// downloads read through [`read_dump_body`]; the rest use [`read_status_body`],
/// which takes ureq's own [`DEFAULT_MAX_BYTES`]. Quoting the dump ceiling at a JSON
/// endpoint told the user their reply had exceeded a limit six-and-a-half times the
/// one it actually hit.
fn body_limit_for(endpoint: Endpoint) -> u64 {
    match endpoint {
        Endpoint::DownloadDump => DUMP_MAX_BYTES,
        _ => DEFAULT_MAX_BYTES,
    }
}

/// A byte count as GiB / MiB / KiB for a user-facing message. Whole units only
/// (the ceilings are all round powers of two), falling back to bytes below 1 KiB.
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    match bytes {
        b if b >= GIB => format!("{} GiB", b / GIB),
        b if b >= MIB => format!("{} MiB", b / MIB),
        b if b >= KIB => format!("{} KiB", b / KIB),
        b => format!("{b} bytes"),
    }
}

/// Map a non-2xx response into a [`SpeleoDbError::Status`], extracting a
/// best-effort human message from the v2 error body (`{error}` / `{errors}` /
/// `{detail}`).
fn status_error(status: u16, body: &[u8]) -> SpeleoDbError {
    SpeleoDbError::Status {
        status,
        message: parse_error_message(body).unwrap_or_else(|| format!("HTTP {status}")),
    }
}

/// Extract a human-facing message from a v2 error body. DISPLAY-ONLY: the
/// status→action contract ([`super::contract::map_status`]) is a pure function of
/// `(endpoint, status)` and never reads this string (#186, D-186).
fn parse_error_message(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    if let Some(s) = value.get("error").and_then(serde_json::Value::as_str) {
        return Some(s.to_owned());
    }
    if let Some(s) = value.get("detail").and_then(serde_json::Value::as_str) {
        return Some(s.to_owned());
    }
    // DRF validation error: `{errors: {non_field_errors: ["..."], field: ["..."]}}`.
    // Surface the first human sentence (non_field_errors first, else any field's
    // first message) rather than dumping the raw JSON object at the user.
    if let Some(errors) = value.get("errors") {
        return first_error_sentence(errors).or_else(|| Some(errors.to_string()));
    }
    None
}

/// Pull the first human message out of a DRF `errors` object: prefer
/// `non_field_errors`, else the first string in the first field (by sorted key).
///
/// The SpeleoDB v2 endpoints raise `non_field_errors` (from a serializer's
/// `validate()`) for the real failure cases the client surfaces - bad
/// credentials, etc. - and per-field errors only for a missing/invalid single
/// field; DRF never mixes both in one 400 (it runs `validate()` only after every
/// per-field check passes). So `non_field_errors` wins whenever present. For a
/// multi-field 400 with no `non_field_errors`, the field pick is made over
/// EXPLICITLY sorted keys so the surfaced sentence is deterministic and can't
/// silently depend on `serde_json`'s object-map ordering (a `BTreeMap` today, but
/// a feature-unified `preserve_order` elsewhere would otherwise change which field
/// wins).
fn first_error_sentence(errors: &serde_json::Value) -> Option<String> {
    fn first_string(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Array(a) => a.iter().find_map(|x| x.as_str().map(str::to_owned)),
            _ => None,
        }
    }
    if let Some(m) = errors.get("non_field_errors").and_then(first_string) {
        return Some(m);
    }
    let obj = errors.as_object()?;
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    keys.into_iter()
        .find_map(|k| obj.get(k).and_then(first_string))
}

#[derive(Deserialize)]
struct AuthTokenResponse {
    token: String,
}

/// `POST /api/v2/user/auth-token/` - exchange email+password for a token.
///
/// Note (verified against the live box): bad credentials return **400** with
/// `{errors: {non_field_errors: [...]}}`, not 401. On an authenticated endpoint a
/// *missing* header yields 403 and an *invalid/expired* token yields 401.
pub fn login(
    base: &str,
    insecure_http: bool,
    email: &str,
    password: &str,
) -> Result<String, SpeleoDbError> {
    let url = api_url(base, "user/auth-token/");
    let payload = serde_json::json!({ "email": email, "password": password }).to_string();

    // Idempotent: the auth-token endpoint is get-or-create (non-rotating), so a
    // retried login returns the same token rather than minting a duplicate.
    let (status, body) = with_retry(base, insecure_http, true, Endpoint::Login, || {
        let resp = agent()
            .post(&url)
            .header("Content-Type", "application/json")
            .send(payload.as_str())?;
        read_status_body(resp)
    })?;

    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    let parsed: AuthTokenResponse = serde_json::from_slice(&body)
        .map_err(|e| SpeleoDbError::Parse(format!("auth-token response: {e}")))?;
    Ok(parsed.token)
}

/// `GET /api/v2/projects/` - list the projects visible to the token holder.
pub fn list_projects(
    base: &str,
    token: &str,
    insecure_http: bool,
) -> Result<Vec<Project>, SpeleoDbError> {
    let url = api_url(base, "projects/");
    let auth = bearer(token);

    let (status, body) = with_retry(base, insecure_http, true, Endpoint::ListProjects, || {
        let resp = agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    // v2 returns a bare JSON array (no `{data: …}` envelope).
    // FRESHNESS FOLD (#189/#191): the authoritative Projects are available here;
    // caching the verdict + resetting last_verify_at lands later (D-B1, #202).
    serde_json::from_slice(&body).map_err(|e| SpeleoDbError::Parse(format!("projects list: {e}")))
}

/// `GET /api/v2/projects/{id}/` - fetch a single project (D-B11, #185). A ~10-line
/// mirror of [`list_projects`] parsing one bare [`Project`]. The caller validates
/// `project_id` (UUID) before calling. Consumed by #194 start_editing (the
/// lock-holder re-GET) and #202 (the freshness fold).
pub fn get_project(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<Project, SpeleoDbError> {
    get_project_inner(
        base,
        token,
        insecure_http,
        project_id,
        REQUEST_TIMEOUT,
        true,
    )
}

/// Short per-request budget for the best-effort #202 lazy lock re-verify probe. It
/// runs UNDER the command lock, so a slow/hung server must not stall the whole
/// command pipeline (autosave included) for the full 30s x (1 + retries) read
/// budget; a tight timeout keeps the worst-case block bounded.
const REVERIFY_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Like [`get_project`] but with a tight, NON-retried budget for the #202 lazy lock
/// re-verify: one attempt, [`REVERIFY_PROBE_TIMEOUT`]. #202 is not a correctness
/// guard (a probe miss allows the edit and the release fence backstops it), so a
/// single fast attempt is the right trade - it must never freeze the editor.
pub fn get_project_probe(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<Project, SpeleoDbError> {
    get_project_inner(
        base,
        token,
        insecure_http,
        project_id,
        REVERIFY_PROBE_TIMEOUT,
        false,
    )
}

fn get_project_inner(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    timeout: Duration,
    idempotent: bool,
) -> Result<Project, SpeleoDbError> {
    let url = api_url(base, &format!("projects/{project_id}/"));
    let auth = bearer(token);

    let (status, body) = with_retry(
        base,
        insecure_http,
        idempotent,
        Endpoint::GetProject,
        || {
            let resp = agent_with_timeout(timeout)
                .get(&url)
                .header("Authorization", auth.as_str())
                .call()?;
            read_status_body(resp)
        },
    )?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    // FRESHNESS FOLD (#189/#191): authoritative Project available here; cache +
    // reset last_verify_at lands later (D-B1, #202).
    serde_json::from_slice(&body).map_err(|e| SpeleoDbError::Parse(format!("project detail: {e}")))
}

/// `GET /api/v2/projects/{id}/revisions/` - the project's linear commit history
/// (#193). Used by guardrail-7 ancestry recovery to decide, after a push whose
/// confirmation was lost, whether our edit actually landed on the server. The
/// caller validates `project_id` (UUID) first. A GET, so it is retry-safe.
/// Consumed by #197 done (ancestry recovery).
pub fn get_revisions(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<RevisionHistory, SpeleoDbError> {
    let url = api_url(base, &format!("projects/{project_id}/revisions/"));
    let auth = bearer(token);

    let (status, body) = with_retry(base, insecure_http, true, Endpoint::GetRevisions, || {
        let resp = agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    serde_json::from_slice(&body).map_err(|e| SpeleoDbError::Parse(format!("revisions: {e}")))
}

/// The authenticated user's profile (`GET /api/v2/user/`), parsed to the one
/// field the spike needs. The server also returns `name`/`country`/notification
/// prefs, which serde ignores.
#[derive(Deserialize)]
struct UserInfo {
    email: String,
}

/// `GET /api/v2/user/` - return the authenticated user's email. The token-paste
/// login path uses this to derive the authoritative email from the token itself
/// (instead of trusting typed input) and, as a side effect, to validate the
/// token at login time: a bad/expired token 401s here rather than slipping
/// through to the first list call.
pub fn fetch_user_email(
    base: &str,
    token: &str,
    insecure_http: bool,
) -> Result<String, SpeleoDbError> {
    let url = api_url(base, "user/");
    let auth = bearer(token);

    let (status, body) = with_retry(base, insecure_http, true, Endpoint::FetchUser, || {
        let resp = agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    let info: UserInfo = serde_json::from_slice(&body)
        .map_err(|e| SpeleoDbError::Parse(format!("user info response: {e}")))?;
    Ok(info.email)
}

/// `GET /api/v2/projects/{id}/download/dump/` - download the project's working
/// tree (HEAD) as a ZIP. Returns the raw ZIP bytes for [`super::dump`] to handle.
pub fn download_dump(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<Vec<u8>, SpeleoDbError> {
    let url = api_url(base, &format!("projects/{project_id}/download/dump/"));
    let auth = bearer(token);

    let (status, body) = with_retry(base, insecure_http, true, Endpoint::DownloadDump, || {
        let resp = dump_agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_dump_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    Ok(body)
}

/// [#218] `GET /api/v2/projects/{id}/download/dump/{hexsha}/` - download the
/// project's tree AT a specific past commit (the server's `FileDownloadAtHashView`)
/// as a ZIP, for the revision-restore flow. Identical wire shape to
/// [`download_dump`] (same `Endpoint::DownloadDump` contract/metrics, same read
/// retry) but pinned to `hexsha` rather than HEAD. The caller validates `hexsha`
/// at the trust boundary before it reaches this URL interpolation.
pub fn download_dump_at_hash(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    hexsha: &str,
) -> Result<Vec<u8>, SpeleoDbError> {
    let url = api_url(
        base,
        &format!("projects/{project_id}/download/dump/{hexsha}/"),
    );
    let auth = bearer(token);

    let (status, body) = with_retry(base, insecure_http, true, Endpoint::DownloadDump, || {
        let resp = dump_agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_dump_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    Ok(body)
}

/// Parse a 2xx body into a [`Project`] (the shape every write endpoint returns).
fn parse_project(body: &[u8], context: &str) -> Result<Project, SpeleoDbError> {
    serde_json::from_slice(body).map_err(|e| SpeleoDbError::Parse(format!("{context}: {e}")))
}

/// MIME type for an artifact part, by extension (D-A-mime). The server selects
/// the processor by *extension* via the AUTO catch-all, but then validates the
/// part's `Content-Type` against that processor's allow-list - so for `.tml` the
/// explicit MIME is **REQUIRED, not just robustness**: `ArianeTMLFileProcessor`
/// allows only `application/octet-stream` / `application/zip` (no `text/plain`),
/// and a bare `.tml` part is rejected with HTTP 400 (OPEN-1, resolved live at
/// #184: `live_open1_bare_tml_mime_outcome`; see `docs/speleodb-dev-testing.md`).
fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("stalx") => "application/vnd.sqlite3",
        // .tml (and anything else co-pushed) → generic binary.
        _ => "application/octet-stream",
    }
}

/// One multipart `artifact` part, described *before* it is folded into an opaque
/// `ureq::Form` (D-B15 / OPEN-2: a built `Form` exposes no `parts()` accessor, so
/// the CI part-name/MIME assertions run against this descriptor). The
/// bare-`.file()`-impossible invariant is structural: there is no constructor
/// path that omits `mime`, and [`upload_auto`] always builds the form from here.
struct UploadPart {
    name: &'static str,
    file_name: String,
    mime: &'static str,
    src: PathBuf,
}

/// The full multipart upload shape: the repeated `artifact` parts + the commit
/// `message` text part.
struct UploadParts {
    parts: Vec<UploadPart>,
    message: String,
}

/// Build the multipart descriptor for an `upload/auto/` push: one `artifact` part
/// per file (never `artifact[]`), each with an explicit per-extension MIME, plus
/// the commit message. Rejects a blank message at the boundary (D-A-methods) - it
/// does NOT substitute a placeholder like the spike's seed helper did.
fn build_upload_parts(
    stalx: &Path,
    tml: &Path,
    message: &str,
) -> Result<UploadParts, SpeleoDbError> {
    if message.trim().is_empty() {
        return Err(SpeleoDbError::Parse(
            "a non-empty commit message is required to upload".to_owned(),
        ));
    }
    let part = |src: &Path, wire_name: &str| UploadPart {
        // Both parts are named "artifact" (repeated), NEVER "artifact[]" - the
        // server expects repeated same-named parts, not bracket-indexed ones.
        name: "artifact",
        // The multipart filename is the CANONICAL server-side name, decoupled from
        // the local source path's basename. The server's upload/auto/ is additive
        // (it `git add`s each uploaded file by its name onto ToT), so the `.stalx`
        // MUST always land as `project.stalx` - else a push from the scratch
        // `snapshot.stalx` would commit a SECOND `.stalx` beside the canonical one,
        // and the next download_dump would be an ambiguous multi-`.stalx` repo.
        file_name: wire_name.to_owned(),
        mime: mime_for(src),
        src: src.to_path_buf(),
    };
    Ok(UploadParts {
        parts: vec![
            part(stalx, CANONICAL_STALX_WIRE_NAME),
            part(tml, "ariane.tml"),
        ],
        message: message.to_owned(),
    })
}

/// `PUT /api/v2/projects/{id}/upload/auto/` - push the project's `.stalx` and its
/// co-derived `ariane.tml` as a single multipart commit, returning the parsed
/// [`Project`] (its `latest_commit.id` is the adopted hexsha for #186 step 5).
///
/// Two `artifact` parts (never `artifact[]`), each streamed via `Part::file` with
/// an explicit MIME (D-A-multipart / D-A-mime). The caller must hold the project
/// mutex. `idempotent = false` → runs at most once (a commit must not
/// double-submit on a mid-response failure).
pub fn upload_auto(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    stalx_path: &Path,
    tml_path: &Path,
    message: &str,
) -> Result<Project, SpeleoDbError> {
    use ureq::unversioned::multipart::{Form, Part};

    let descriptor = build_upload_parts(stalx_path, tml_path, message)?;
    let url = api_url(base, &format!("projects/{project_id}/upload/auto/"));
    let auth = bearer(token);

    // The Form/Part builders consume their inputs (`send(form)` takes the Form by
    // value), so the form is rebuilt inside the closure. `idempotent = false`
    // means the closure runs at most once regardless.
    let (status, body) = with_retry(base, insecure_http, false, Endpoint::Upload, || {
        let mut form = Form::new();
        for p in &descriptor.parts {
            let part = Part::file(&p.src)?
                .file_name(&p.file_name)
                .mime_str(p.mime)?;
            form = form.part(p.name, part);
        }
        let form = form.text("message", descriptor.message.as_str());
        let resp = agent_with_timeout(UPLOAD_TIMEOUT)
            .put(&url)
            .header("Authorization", auth.as_str())
            .send(form)?;
        read_status_body(resp)
    })?;

    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    // CONDITIONAL CLEAR (#191): the adopted hexsha is the edit-epoch the
    // comparison that clears has_unpushed_edits is owned by #191 - do NOT clear
    // the dirty bit unconditionally here.
    // FRESHNESS FOLD (#189/#191): authoritative Project available here; cache +
    // reset last_verify_at lands later (D-B1, #202).
    //
    // The upload/auto success body is an ENVELOPE - `{ files, message, hexsha,
    // browser_url, project: {…} }` - with the Project NESTED under `project`
    // (verified live; a bare `parse_project` here failed with "missing field
    // `id`"). The authoritative SHA of the commit THIS push created is the
    // TOP-LEVEL `hexsha`. The nested `project.latest_commit` is
    // eventually-consistent and was observed live still pointing at the PRIOR tip
    // (the service-account root creation commit) immediately after a push - so we
    // adopt `hexsha`, not the nested pointer, and overwrite `latest_commit` with
    // it so every caller's `latest_commit.id` is the commit just created (#197).
    parse_upload_auto_response(&body)
}

/// Parse the `upload/auto/` success envelope into the adopted [`Project`]. The
/// top-level `hexsha` is the SHA of the commit THIS push created; prefer it and
/// overwrite `latest_commit` so callers adopt the right BASE. If a deployment
/// omits the top-level `hexsha`, fall back to the nested `project.latest_commit`
/// rather than failing an ACCEPTED (200) push as a Parse/Fatal error - that would
/// strand the dirty bit forever (#197 review); a slightly-stale BASE self-heals via
/// the ancestry fence on the next Sync.
fn parse_upload_auto_response(body: &[u8]) -> Result<Project, SpeleoDbError> {
    #[derive(serde::Deserialize)]
    struct UploadAutoResponse {
        #[serde(default)]
        hexsha: Option<String>,
        project: Project,
    }
    let parsed: UploadAutoResponse = serde_json::from_slice(body)
        .map_err(|e| SpeleoDbError::Parse(format!("upload auto response: {e}")))?;
    let mut project = parsed.project;
    // The top-level `hexsha` is the ONLY trustworthy source for the commit this
    // push just created, and the nested `project.latest_commit` is deliberately NOT
    // used as a fallback for it. That field is eventually consistent - documented
    // in `sync::reconcile_verdict` and observed live still pointing at the PRIOR
    // tip (the service-account creation root) immediately after a push. Folding it
    // in here handed the caller a stale ancestor that passed its "is there a sha?"
    // guard and moved BASE BACKWARD past every commit in between; the next edit
    // then fenced that BASE against our own just-landed commit and raised the
    // divergence prompt, offering to discard un-pushed work to "take remote".
    //
    // `None` is the honest answer when the authoritative field is absent, and the
    // caller already handles it: it keeps the previous BASE, which costs at most one
    // redundant ancestry recovery on the next push - exactly what that machinery is
    // for.
    project.latest_commit = parsed
        .hexsha
        .filter(|h| !h.is_empty())
        .map(|id| Commit { id });
    Ok(project)
}

/// `POST /api/v2/projects/{id}/acquire/` - take the project mutex, returning the
/// updated [`Project`]. The upload endpoint requires the caller to hold the lock
/// (`UserOwnsProjectMutex`). `idempotent = false`. Consumed by #194
/// `speleodb_start_editing`.
pub fn acquire_lock(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<Project, SpeleoDbError> {
    post_no_body_project(
        base,
        token,
        insecure_http,
        Endpoint::Acquire,
        &format!("projects/{project_id}/acquire/"),
        "acquire lock response",
    )
}

/// `POST /api/v2/projects/{id}/release/` - release the project mutex, returning
/// the updated [`Project`]. `idempotent = false`. Consumed by #194/#197.
pub fn release_lock(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<Project, SpeleoDbError> {
    post_no_body_project(
        base,
        token,
        insecure_http,
        Endpoint::Release,
        &format!("projects/{project_id}/release/"),
        "release lock response",
    )
}

/// `DELETE /api/v2/user/release_all_locks/` - release EVERY write lock the token
/// holder currently holds, returning `()` on success. Idempotent (releasing when
/// none are held is a no-op), so a transient-transport retry is safe. Drives the
/// held-lock resume/release prompt's "Release all" choice (#201, B1).
pub fn release_all_locks(
    base: &str,
    token: &str,
    insecure_http: bool,
) -> Result<(), SpeleoDbError> {
    let url = api_url(base, "user/release_all_locks/");
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, true, Endpoint::ReleaseAll, || {
        let resp = agent()
            .delete(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    Ok(())
}

/// `PATCH /api/v2/projects/{id}/` - update project metadata, returning the updated
/// [`Project`]. Non-idempotent (`idempotent = false`) so a mid-response transport
/// failure can't double-apply. Consumed by #200's Enable-rendering action (PATCH
/// `exclude_geojson = false`); the server enforces permission (READ_AND_WRITE, no
/// lock) on the metadata PATCH.
pub fn patch_project(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    payload: &str,
) -> Result<Project, SpeleoDbError> {
    let url = api_url(base, &format!("projects/{project_id}/"));
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, false, Endpoint::PatchProject, || {
        let resp = agent()
            .patch(&url)
            .header("Authorization", auth.as_str())
            .header("Content-Type", "application/json")
            .send(payload)?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    parse_project(&body, "patch project response")
}

/// `POST /api/v2/projects/` with a caller-built JSON body - create a fresh project
/// and return the full parsed [`Project`]. `idempotent = false` (run once so a
/// mid-response failure can't double-submit a create). The #207 create-on-SDB
/// orchestration builds the payload (name/description/country/visibility/`type`/
/// lat-long/`exclude_geojson`); `created_by` is injected server-side.
pub fn create_project_with(
    base: &str,
    token: &str,
    insecure_http: bool,
    payload: &str,
) -> Result<Project, SpeleoDbError> {
    let url = api_url(base, "projects/");
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, false, Endpoint::CreateProject, || {
        let resp = agent()
            .post(&url)
            .header("Authorization", auth.as_str())
            .header("Content-Type", "application/json")
            .send(payload)?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    // FRESHNESS FOLD (#189/#191): authoritative Project available here; cache +
    // reset last_verify_at lands later (D-B1, #202).
    parse_project(&body, "create project response")
}

/// Convenience: create a minimal project by name, typed with the LEGACY `other`
/// on purpose so live tests exercise the relabel to `stalagmight`. Test/seed helper
/// (the #207 production path uses [`create_project_with`] with a full payload).
/// `pub` because the root crate's live tests drive it across the crate
/// boundary, which also retires the old non-test dead-code expectation.
pub fn create_project(
    base: &str,
    token: &str,
    insecure_http: bool,
    name: &str,
) -> Result<Project, SpeleoDbError> {
    let payload = serde_json::json!({
        "name": name,
        "description": "Stalagmight project (auto-created)",
        "country": "US",
        "type": "other",
    })
    .to_string();
    create_project_with(base, token, insecure_http, &payload)
}

/// Shared POST-with-no-body helper for the lock mutations (acquire/release):
/// non-idempotent (run once), parse the 2xx body into a [`Project`].
fn post_no_body_project(
    base: &str,
    token: &str,
    insecure_http: bool,
    endpoint: Endpoint,
    suffix: &str,
    context: &str,
) -> Result<Project, SpeleoDbError> {
    let url = api_url(base, suffix);
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, false, endpoint, || {
        let resp = agent()
            .post(&url)
            .header("Authorization", auth.as_str())
            .send("")?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    parse_project(&body, context)
}

/// `DELETE /api/v2/projects/{id}/` - soft-delete the project (the server
/// deactivates it and a cron purges after ~30 days), returning `()` on success.
/// DELETE is idempotent (deleting an already-deleted project is a no-op/404), so a
/// transient-transport retry is safe. Consumed by #215's Delete-project action and
/// the live tests' per-run cleanup. Does not require releasing the lock first.
pub fn delete_project(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<(), SpeleoDbError> {
    let url = api_url(base, &format!("projects/{project_id}/"));
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, true, Endpoint::DeleteProject, || {
        let resp = agent()
            .delete(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    Ok(())
}

/// Send `payload` as a JSON body with an arbitrary `method` to `suffix`, enforcing
/// the http private-network policy first ([`super::tls::enforce`], the #187
/// chokepoint); a 2xx maps to `Ok(())`. The collaborator permission-detail
/// endpoints (#215) are non-idempotent writes routed here: they run exactly once
/// (no retry, matching every other SDB write), AND the `remove` variants are a
/// DELETE-with-body - which ureq's typestate `.delete()` builder cannot express -
/// so all three verbs go through `Agent::run` with an explicit `http::Request`.
fn send_json_request(
    base: &str,
    token: &str,
    insecure_http: bool,
    endpoint: Endpoint,
    method: http::Method,
    suffix: &str,
    payload: &serde_json::Value,
) -> Result<(), SpeleoDbError> {
    // #187: block public `http://` / scheme-less endpoints before any dial (the
    // same gate `with_retry` runs; these writes bypass the retry loop, never it).
    // A policy block is not server traffic, so it is intentionally not recorded.
    super::tls::enforce(base, insecure_http)?;
    let url = api_url(base, suffix);
    let request = http::Request::builder()
        .method(method)
        .uri(url)
        .header("Authorization", bearer(token))
        .header("Content-Type", "application/json")
        .body(payload.to_string())
        .map_err(|e| SpeleoDbError::Parse(format!("build request: {e}")))?;
    // #210: this write bypasses `with_retry`, so record its one attempt here - the
    // same count + latency + outcome the retry-loop chokepoint emits. Time and
    // classify the FULL attempt (dial + status + body read), exactly as
    // `with_retry`'s timed closure does, so latency covers the body read and a
    // mid-body read failure counts as `network-error` on both paths (not a false
    // `ok`/`server-error` off the status line alone).
    let started = std::time::Instant::now();
    let result = agent().run(request).and_then(read_status_body);
    let outcome = match &result {
        Ok((status, _)) => Outcome::from_status(*status),
        Err(_) => Outcome::NetworkError,
    };
    super::metrics::record(endpoint, outcome, started.elapsed());
    let (status, body) = result.map_err(|e| {
        let cause = classify_transport(&e);
        let detail = clean_transport(&e.to_string());
        // Same #270 contract as `with_retry`: the raw cause lives in the log.
        tracing::warn!(?endpoint, ?cause, detail = %detail, "SpeleoDB transport failure");
        SpeleoDbError::Transport { cause, detail }
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    Ok(())
}

/// `GET /api/v2/projects/{id}/permissions/user/` - list the project's active
/// per-user permission grants (#215). A GET, so retry-safe; requires READ_ONLY+
/// server-side (403 below that). Only the `permissions[]` array is consumed.
pub fn list_user_permissions(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<Vec<UserPermission>, SpeleoDbError> {
    let url = api_url(base, &format!("projects/{project_id}/permissions/user/"));
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, true, Endpoint::ListPermissions, || {
        let resp = agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    let list: UserPermissionList = serde_json::from_slice(&body)
        .map_err(|e| SpeleoDbError::Parse(format!("user permissions: {e}")))?;
    Ok(list.permissions)
}

/// `POST /api/v2/projects/{id}/permission/user/detail/` `{user, level}` - grant a
/// collaborator a permission level (#215). ADMIN-gated server-side.
pub fn add_user_permission(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    email: &str,
    level: Permission,
) -> Result<(), SpeleoDbError> {
    send_json_request(
        base,
        token,
        insecure_http,
        Endpoint::MutatePermission,
        http::Method::POST,
        &format!("projects/{project_id}/permission/user/detail/"),
        &serde_json::json!({ "user": email, "level": level }),
    )
}

/// `PUT /api/v2/projects/{id}/permission/user/detail/` `{user, level}` - change an
/// existing collaborator's permission level (#215). ADMIN-gated server-side.
pub fn set_user_permission(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    email: &str,
    level: Permission,
) -> Result<(), SpeleoDbError> {
    send_json_request(
        base,
        token,
        insecure_http,
        Endpoint::MutatePermission,
        http::Method::PUT,
        &format!("projects/{project_id}/permission/user/detail/"),
        &serde_json::json!({ "user": email, "level": level }),
    )
}

/// `DELETE /api/v2/projects/{id}/permission/user/detail/` with body `{user}` -
/// remove a collaborator (soft, #215). ADMIN-gated server-side.
pub fn remove_user_permission(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    email: &str,
) -> Result<(), SpeleoDbError> {
    send_json_request(
        base,
        token,
        insecure_http,
        Endpoint::MutatePermission,
        http::Method::DELETE,
        &format!("projects/{project_id}/permission/user/detail/"),
        &serde_json::json!({ "user": email }),
    )
}

/// `GET /api/v2/projects/{id}/permissions/team/` - list the project's active
/// per-team permission grants (#215). A GET, so retry-safe.
pub fn list_team_permissions(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
) -> Result<Vec<TeamPermission>, SpeleoDbError> {
    let url = api_url(base, &format!("projects/{project_id}/permissions/team/"));
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, true, Endpoint::ListPermissions, || {
        let resp = agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    let list: TeamPermissionList = serde_json::from_slice(&body)
        .map_err(|e| SpeleoDbError::Parse(format!("team permissions: {e}")))?;
    Ok(list.permissions)
}

/// `POST /api/v2/projects/{id}/permission/team/detail/` `{team, level}` - grant a
/// team a permission level (#215). ADMIN-gated; the server rejects `ADMIN` for a
/// team (teams may only hold WEB_VIEWER / READ_ONLY / READ_AND_WRITE).
pub fn add_team_permission(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    team_id: &str,
    level: Permission,
) -> Result<(), SpeleoDbError> {
    send_json_request(
        base,
        token,
        insecure_http,
        Endpoint::MutatePermission,
        http::Method::POST,
        &format!("projects/{project_id}/permission/team/detail/"),
        &serde_json::json!({ "team": team_id, "level": level }),
    )
}

/// `PUT /api/v2/projects/{id}/permission/team/detail/` `{team, level}` - change a
/// team's permission level (#215). ADMIN-gated; no `ADMIN` for teams.
pub fn set_team_permission(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    team_id: &str,
    level: Permission,
) -> Result<(), SpeleoDbError> {
    send_json_request(
        base,
        token,
        insecure_http,
        Endpoint::MutatePermission,
        http::Method::PUT,
        &format!("projects/{project_id}/permission/team/detail/"),
        &serde_json::json!({ "team": team_id, "level": level }),
    )
}

/// `DELETE /api/v2/projects/{id}/permission/team/detail/` with body `{team}` -
/// revoke a team's access (soft, #215). ADMIN-gated server-side.
pub fn remove_team_permission(
    base: &str,
    token: &str,
    insecure_http: bool,
    project_id: &str,
    team_id: &str,
) -> Result<(), SpeleoDbError> {
    send_json_request(
        base,
        token,
        insecure_http,
        Endpoint::MutatePermission,
        http::Method::DELETE,
        &format!("projects/{project_id}/permission/team/detail/"),
        &serde_json::json!({ "team": team_id }),
    )
}

/// `GET /api/v2/teams/` - the authenticated user's teams (a bare JSON array), for
/// the add-team picker and to resolve a team-permission UUID to a display name
/// (#215). A GET, so retry-safe.
pub fn list_teams(
    base: &str,
    token: &str,
    insecure_http: bool,
) -> Result<Vec<Team>, SpeleoDbError> {
    let url = api_url(base, "teams/");
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, true, Endpoint::ListTeams, || {
        let resp = agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    serde_json::from_slice(&body).map_err(|e| SpeleoDbError::Parse(format!("teams: {e}")))
}

/// `GET /api/v2/user/autocomplete/?query=<substr>` - up to 10 users whose email or
/// name contains `query` (the server requires >= 3 chars; the caller guards that).
/// Backs the add-collaborator email typeahead (#215). A GET, so retry-safe. The
/// query is percent-encoded so spaces / `@` / `&` in a partial name or email cannot
/// break the URL.
pub fn list_user_autocomplete(
    base: &str,
    token: &str,
    insecure_http: bool,
    query: &str,
) -> Result<Vec<UserAutocomplete>, SpeleoDbError> {
    let encoded = utf8_percent_encode(query, NON_ALPHANUMERIC);
    let url = api_url(base, &format!("user/autocomplete/?query={encoded}"));
    let auth = bearer(token);
    let (status, body) = with_retry(base, insecure_http, true, Endpoint::Autocomplete, || {
        let resp = agent()
            .get(&url)
            .header("Authorization", auth.as_str())
            .call()?;
        read_status_body(resp)
    })?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    serde_json::from_slice(&body)
        .map_err(|e| SpeleoDbError::Parse(format!("user autocomplete: {e}")))
}

/// Test-only convenience: grant `email` the given `level` string on the project.
/// Drives the two-account live tests' collaborator setup; the production path is
/// [`add_user_permission`] (typed `Permission`). The dev box is a private address,
/// so `insecure_http = false` is fine here.
#[cfg(any(test, feature = "test-support"))]
pub fn grant_permission(
    base: &str,
    token: &str,
    project_id: &str,
    email: &str,
    level: &str,
) -> Result<(), SpeleoDbError> {
    send_json_request(
        base,
        token,
        false,
        Endpoint::MutatePermission,
        http::Method::POST,
        &format!("projects/{project_id}/permission/user/detail/"),
        &serde_json::json!({ "user": email, "level": level }),
    )
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit test fixtures / let-else assertion"
)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::{
        add_team_permission, add_user_permission, api_url, bearer, body_limit_for,
        build_upload_parts, classify_transport, clean_transport, delete_project, download_dump,
        download_dump_at_hash, human_bytes, list_teams, list_user_autocomplete,
        list_user_permissions, mime_for, parse_error_message, parse_upload_auto_response,
        remove_team_permission, remove_user_permission, set_user_permission, with_retry, Endpoint,
        RevisionHistory, UserInfo, DEFAULT_MAX_BYTES, DUMP_DOWNLOAD_TIMEOUT, DUMP_MAX_BYTES,
        REQUEST_TIMEOUT,
    };
    use crate::speleodb::{Permission, SpeleoDbError, TransportCause};

    #[test]
    fn revisions_payload_decodes_verified_v2_field_names() {
        // The whole sync engine keys off the /revisions/ field names `parent_ids` and
        // `author_email`, both #[serde(default)] - so a RENAMED server key would
        // SILENTLY default (parent_ids=[] → every commit reads as a root → tip None →
        // fail closed; author_email="" → stranger → Diverged) with no parse error. The
        // only other decode of a real payload is the #[ignore] live test, so this
        // recorded fixture (the verified v2 contract, incl. the extra fields serde
        // ignores) locks the names in CI. (#197 review)
        let payload = r#"{
            "project": {"id": "11111111-1111-1111-1111-111111111111", "name": "X"},
            "commits": [
                {"id": "root0", "parent_ids": [], "author_email": "svc@speleodb.org",
                 "author_name": "Service", "authored_date": "2026-06-01T00:00:00Z", "message": "init"},
                {"id": "childA", "parent_ids": ["root0"], "author_email": "Me@Example.COM",
                 "author_name": "Me", "authored_date": "2026-06-02T00:00:00Z", "message": "edit"}
            ]
        }"#;
        let history: RevisionHistory = serde_json::from_slice(payload.as_bytes())
            .expect("verified v2 revisions payload decodes");
        assert_eq!(history.commits.len(), 2, "both commits decode");
        let child = history
            .commits
            .iter()
            .find(|c| c.id == "childA")
            .expect("childA present");
        assert_eq!(
            child.parent_ids,
            vec!["root0".to_owned()],
            "parent_ids MUST populate from the `parent_ids` key (a rename would silently default to [])"
        );
        assert_eq!(
            child.author_email, "Me@Example.COM",
            "author_email MUST populate from the `author_email` key (a rename would default to \"\")"
        );
        // parent_ids actually drives the topology leaf the fence trusts.
        assert_eq!(
            crate::speleodb::sync::tip_of(&history.commits),
            Some("childA"),
            "tip_of resolves the leaf via parent_ids"
        );
    }

    #[test]
    fn upload_auto_response_adopts_only_the_authoritative_hexsha() {
        // #197 review: a top-level hexsha is the commit THIS push created → adopt it.
        // A 200 envelope WITHOUT one must NOT fail as Parse/Fatal (that would strand
        // the dirty bit) - but it must not fall back to the nested `latest_commit`
        // either. That field is eventually consistent and was observed live still
        // pointing at the PRIOR tip right after a push, so adopting it moves BASE
        // BACKWARD past every intervening commit; the caller then fences that BASE
        // against our own just-landed commit and raises the divergence prompt,
        // offering to discard un-pushed work to "take remote". `None` makes the
        // caller keep the previous BASE, which costs one redundant ancestry recovery.
        let proj = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Up","latest_commit":{"id":"nested-prior"}}"#;

        let with = format!("{{\"hexsha\":\"top-new\",\"project\":{proj}}}");
        let adopted = parse_upload_auto_response(with.as_bytes()).expect("parse with hexsha");
        assert_eq!(
            adopted.latest_commit.as_ref().map(|c| c.id.as_str()),
            Some("top-new"),
            "a present top-level hexsha is adopted as the new BASE"
        );

        let without = format!("{{\"project\":{proj}}}");
        let fallback =
            parse_upload_auto_response(without.as_bytes()).expect("missing hexsha must not error");
        assert!(
            fallback.latest_commit.is_none(),
            "the stale nested latest_commit must never be adopted as the new BASE"
        );

        // An empty-string hexsha is treated as absent (same refusal).
        let empty = format!("{{\"hexsha\":\"\",\"project\":{proj}}}");
        let empty_fb =
            parse_upload_auto_response(empty.as_bytes()).expect("empty hexsha must not error");
        assert!(empty_fb.latest_commit.is_none());
    }

    #[test]
    fn user_info_parses_email_and_ignores_extra_fields() {
        // Mirrors the real `GET /user/` body: only `email` is consumed; the rest
        // (name/country/notification prefs) must be ignored, not rejected.
        let info: UserInfo = serde_json::from_str(
            r#"{"email":"a@b.com","name":"A","country":"","email_on_speleodb_updates":true}"#,
        )
        .expect("user info JSON");
        assert_eq!(info.email, "a@b.com");
    }

    #[test]
    fn api_url_joins_without_double_slash() {
        assert_eq!(
            api_url("http://10.10.0.220:8000/", "projects/"),
            "http://10.10.0.220:8000/api/v2/projects/"
        );
        assert_eq!(
            api_url("http://10.10.0.220:8000", "user/auth-token/"),
            "http://10.10.0.220:8000/api/v2/user/auth-token/"
        );
    }

    #[test]
    fn bearer_header_format() {
        assert_eq!(bearer("abc123"), "Bearer abc123");
    }

    #[test]
    fn parse_error_message_handles_v2_shapes() {
        assert_eq!(
            parse_error_message(br#"{"error":"nope"}"#).as_deref(),
            Some("nope")
        );
        assert_eq!(
            parse_error_message(br#"{"detail":"Not found."}"#).as_deref(),
            Some("Not found.")
        );
        // The auth-token bad-credentials shape: a nested `errors` object - surface
        // the human sentence (non_field_errors first), never the raw JSON.
        assert_eq!(
            parse_error_message(br#"{"errors":{"non_field_errors":["bad"]}}"#).as_deref(),
            Some("bad")
        );
        // A field error with no non_field_errors falls back to the first field msg.
        assert_eq!(
            parse_error_message(br#"{"errors":{"email":["Enter a valid email."]}}"#).as_deref(),
            Some("Enter a valid email.")
        );
        // A multi-field error with no non_field_errors picks the first field by
        // SORTED key, deterministically, regardless of JSON key order on the wire
        // (both orderings below must yield the same "email" message).
        assert_eq!(
            parse_error_message(br#"{"errors":{"password":["Too short."],"email":["Required."]}}"#)
                .as_deref(),
            Some("Required.")
        );
        assert_eq!(
            parse_error_message(br#"{"errors":{"email":["Required."],"password":["Too short."]}}"#)
                .as_deref(),
            Some("Required.")
        );
        // non_field_errors still wins over any per-field entry.
        assert_eq!(
            parse_error_message(
                br#"{"errors":{"email":["Required."],"non_field_errors":["Bad login."]}}"#
            )
            .as_deref(),
            Some("Bad login.")
        );
        assert_eq!(parse_error_message(b"not json"), None);
    }

    #[test]
    fn clean_transport_strips_io_and_os_error_noise() {
        assert_eq!(
            clean_transport("io: Connection refused (os error 111)"),
            "Connection refused"
        );
        assert_eq!(clean_transport("io: Connection reset"), "Connection reset");
        // A message without the noise is returned as-is (trimmed).
        assert_eq!(clean_transport("timed out"), "timed out");
        // A parenthetical that isn't an os-error suffix is preserved.
        assert_eq!(
            clean_transport("dns error (lookup failed)"),
            "dns error (lookup failed)"
        );
    }

    #[test]
    fn classify_transport_reads_the_typed_error_not_its_message() {
        // [#270] The classification MUST come from the typed error / io::ErrorKind:
        // an io::Error's Display comes from the platform strerror, so it is OS- and
        // locale-dependent and unusable as a discriminant.
        assert_eq!(
            classify_transport(&ureq::Error::HostNotFound),
            TransportCause::Dns
        );
        assert_eq!(
            classify_transport(&ureq::Error::Timeout(ureq::Timeout::Global)),
            TransportCause::Timeout
        );
        assert_eq!(
            classify_transport(&ureq::Error::Tls("handshake failed")),
            TransportCause::Tls
        );
        assert_eq!(
            classify_transport(&ureq::Error::ConnectionFailed),
            TransportCause::Refused
        );
        assert_eq!(
            classify_transport(&ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "Connection refused"
            ))),
            TransportCause::Refused
        );
        assert_eq!(
            classify_transport(&ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out"
            ))),
            TransportCause::Timeout
        );
        // The errno that started #270: a blackholed host surfaces EINVAL. It has no
        // useful meaning for a user, so it lands in the catch-all (the FE shows the
        // generic "could not reach the server" line) - never as raw text.
        assert_eq!(
            classify_transport(&ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid argument"
            ))),
            TransportCause::Unknown
        );
    }

    #[test]
    fn transport_error_keeps_the_raw_cause_for_the_log() {
        // [#270] The user-facing line is rendered FE-side from the typed cause, but
        // the raw cause must survive on the error (it is what a bug report needs).
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener); // nothing is listening on this port now
        let err = with_retry(
            &format!("http://127.0.0.1:{port}"),
            false,
            false,
            Endpoint::ListProjects,
            || {
                Err(ureq::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "Connection refused (os error 111)",
                )))
            },
        )
        .expect_err("a transport failure surfaces");
        let SpeleoDbError::Transport { cause, detail } = err else {
            panic!("expected a Transport error, got: {err:?}");
        };
        assert_eq!(cause, TransportCause::Refused);
        assert_eq!(
            detail, "Connection refused",
            "the raw cause is kept (os-error noise stripped) so the log stays actionable"
        );
    }

    #[test]
    fn parse_error_message_is_display_only() {
        // Guards the #186 invariant at the unit level: parse_error_message only
        // ever yields a display STRING - it never returns or implies an action.
        // (The action is a pure fn of (endpoint, status); see contract.rs.)
        let m = parse_error_message(br#"{"detail":"forbidden"}"#);
        assert_eq!(m.as_deref(), Some("forbidden"));
    }

    #[test]
    fn upload_parts_both_named_artifact_never_bracketed() {
        // The `.stalx` source is the scratch SNAPSHOT path (its real basename in the
        // push pipeline) - the wire filename MUST still be the canonical
        // `project.stalx`, NOT the source basename. The server is additive, so a
        // `snapshot.stalx` wire name would commit a second `.stalx` and corrupt the
        // repo into an ambiguous multi-`.stalx` dump. (Regression: this test used to
        // pass a `project.stalx` source, masking exactly that bug.)
        let parts = build_upload_parts(
            Path::new("/cache/snapshot.stalx"),
            Path::new("/cache/ariane.tml"),
            "msg",
        )
        .expect("descriptor builds");
        assert_eq!(parts.parts.len(), 2);
        for p in &parts.parts {
            assert_eq!(
                p.name, "artifact",
                "every part is the repeated `artifact` name"
            );
            assert_ne!(p.name, "artifact[]", "never the bracketed form");
        }
        assert_eq!(parts.message, "msg");
        assert_eq!(
            parts.parts[0].file_name, "project.stalx",
            "the .stalx uploads under the canonical name, not the scratch basename"
        );
        assert_eq!(parts.parts[1].file_name, "ariane.tml");
        // The source path is still the snapshot (that's what gets read + streamed).
        assert_eq!(parts.parts[0].src, Path::new("/cache/snapshot.stalx"));
    }

    #[test]
    fn upload_parts_carry_explicit_mime() {
        let parts = build_upload_parts(
            Path::new("/x/project.stalx"),
            Path::new("/x/ariane.tml"),
            "msg",
        )
        .expect("descriptor builds");
        assert_eq!(parts.parts[0].mime, "application/vnd.sqlite3");
        assert_eq!(parts.parts[1].mime, "application/octet-stream");
        // The extension→MIME map itself.
        assert_eq!(mime_for(Path::new("a.stalx")), "application/vnd.sqlite3");
        assert_eq!(mime_for(Path::new("a.tml")), "application/octet-stream");
        assert_eq!(mime_for(Path::new("a.bin")), "application/octet-stream");
    }

    #[test]
    fn upload_auto_rejects_blank_message() {
        for blank in ["", "   ", "\t\n"] {
            assert!(
                matches!(
                    build_upload_parts(Path::new("a.stalx"), Path::new("a.tml"), blank),
                    Err(SpeleoDbError::Parse(_))
                ),
                "blank message {blank:?} must be rejected"
            );
        }
    }

    #[test]
    fn project_dto_defaults_exclude_geojson() {
        use crate::speleodb::models::Project;
        // exclude_geojson defaults false when omitted, and parses when present.
        let omitted: Project =
            serde_json::from_str(r#"{"id":"a","name":"n"}"#).expect("minimal project");
        assert!(!omitted.exclude_geojson);
        let present: Project =
            serde_json::from_str(r#"{"id":"a","name":"n","exclude_geojson":true}"#)
                .expect("project with field");
        assert!(present.exclude_geojson);
    }

    #[test]
    fn get_project_parses_single_project() {
        // The get_project body is a bare single Project object (not an array).
        use crate::speleodb::models::Project;
        let p: Project = serde_json::from_str(
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Solo","type":"other"}"#,
        )
        .expect("single project");
        assert_eq!(p.id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(p.name, "Solo");
    }

    #[test]
    fn with_retry_calls_tls_enforce_first() {
        // A public http base must be rejected by tls::enforce BEFORE the closure
        // runs even once (the closure increments a counter we assert stays 0).
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let result = with_retry(
            "http://8.8.8.8",
            false,
            true,
            Endpoint::ListProjects,
            move || {
                c.fetch_add(1, Ordering::SeqCst);
                Ok((200, Vec::new()))
            },
        );
        assert!(matches!(
            result,
            Err(SpeleoDbError::InsecureEndpoint { .. })
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "closure must never run");
    }

    #[test]
    fn with_retry_reads_retry_transient_then_succeed() {
        // An idempotent read whose closure fails transiently the first time then
        // succeeds returns Ok - over a loopback base so tls::enforce allows it.
        let n = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&n);
        let out = with_retry(
            "http://127.0.0.1:9",
            false,
            true,
            Endpoint::ListProjects,
            move || {
                let i = c.fetch_add(1, Ordering::SeqCst);
                if i == 0 {
                    Err(ureq::Error::HostNotFound)
                } else {
                    Ok((200, b"ok".to_vec()))
                }
            },
        )
        .expect("idempotent read should retry then succeed");
        assert_eq!(out, (200, b"ok".to_vec()));
        assert_eq!(n.load(Ordering::SeqCst), 2, "one failure + one success");
    }

    #[test]
    fn with_retry_idempotent_5xx_is_retried() {
        // A 503 on an idempotent call is retried (treated like a transient error)
        // and the eventual 200 is returned.
        let n = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&n);
        let out = with_retry(
            "http://127.0.0.1:9",
            false,
            true,
            Endpoint::ListProjects,
            move || {
                let i = c.fetch_add(1, Ordering::SeqCst);
                if i < 2 {
                    Ok((503, b"busy".to_vec()))
                } else {
                    Ok((200, b"ok".to_vec()))
                }
            },
        )
        .expect("idempotent 5xx should retry to success");
        assert_eq!(out.0, 200);
        assert_eq!(n.load(Ordering::SeqCst), 3, "503, 503, 200");
    }

    #[test]
    fn with_retry_idempotent_5xx_exhausted_returns_5xx() {
        // If every idempotent attempt 5xxs, the final 5xx is surfaced (NOT a
        // transport error) so map_status can classify it as Retry.
        let n = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&n);
        let out = with_retry(
            "http://127.0.0.1:9",
            false,
            true,
            Endpoint::ListProjects,
            move || {
                c.fetch_add(1, Ordering::SeqCst);
                Ok((500, b"down".to_vec()))
            },
        )
        .expect("exhausted 5xx is still an Ok((status, body)), not a transport Err");
        assert_eq!(out.0, 500);
        assert_eq!(n.load(Ordering::SeqCst), 3, "1 + MAX_RETRIES attempts");
    }

    #[test]
    fn with_retry_writes_run_exactly_once() {
        // A non-idempotent write whose closure 5xxs runs ONCE and returns the 5xx
        // straight through (no retry - double-submit hazard).
        let n = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&n);
        let out = with_retry(
            "http://127.0.0.1:9",
            false,
            false,
            Endpoint::ListProjects,
            move || {
                c.fetch_add(1, Ordering::SeqCst);
                Ok((500, b"down".to_vec()))
            },
        )
        .expect("write returns the 5xx body");
        assert_eq!(out.0, 500);
        assert_eq!(n.load(Ordering::SeqCst), 1, "write must run exactly once");

        // A write whose closure errors also runs exactly once (then surfaces the
        // transport error).
        let n2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::clone(&n2);
        let err = with_retry(
            "http://127.0.0.1:9",
            false,
            false,
            Endpoint::ListProjects,
            move || {
                c2.fetch_add(1, Ordering::SeqCst);
                Err(ureq::Error::HostNotFound)
            },
        )
        .expect_err("transport error surfaces");
        assert!(matches!(err, SpeleoDbError::Transport { .. }));
        assert_eq!(
            n2.load(Ordering::SeqCst),
            1,
            "write transport error runs once"
        );
    }

    /// A one-shot loopback HTTP server that replies to each accepted connection
    /// with the next status from `statuses`, recording the raw request bytes. The
    /// only socket I/O in the unit suite (D-B15); doubles as the #187 127.0.0.1
    /// fixture.
    struct LoopbackStub {
        base: String,
        captured: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl LoopbackStub {
        fn start(statuses: Vec<u16>) -> Self {
            Self::start_with_body_len(statuses, 2)
        }

        /// [`Self::start`] with a body of `body_len` bytes instead of the default
        /// `{}` - for the over-limit read, which needs a response bigger than the
        /// ceiling it is read under.
        fn start_with_body_len(statuses: Vec<u16>, body_len: usize) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let port = listener.local_addr().expect("addr").port();
            let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
            let cap = Arc::clone(&captured);
            let handle = std::thread::spawn(move || {
                for status in statuses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    // Drain the whole request (request line + headers + any
                    // multipart body) by reading with a short timeout until the
                    // socket goes quiet - the body can arrive in a TCP segment
                    // after the headers, so a single read would miss it.
                    use std::io::Read as _;
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                    let mut captured_bytes: Vec<u8> = Vec::new();
                    let mut chunk = vec![0u8; 16 * 1024];
                    loop {
                        match stream.read(&mut chunk) {
                            // Peer closed write half - request fully delivered.
                            Ok(0) => break,
                            Ok(n) => captured_bytes.extend_from_slice(&chunk[..n]),
                            // WouldBlock/TimedOut (read timeout): the client is now
                            // waiting on our response, so the request is complete.
                            Err(_) => break,
                        }
                    }
                    cap.lock().expect("capture lock").push(captured_bytes);
                    let body = if body_len == 2 {
                        b"{}".to_vec()
                    } else {
                        vec![b'x'; body_len]
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.write_all(&body);
                    let _ = stream.flush();
                }
            });
            Self {
                base: format!("http://127.0.0.1:{port}"),
                captured,
                handle: Some(handle),
            }
        }

        fn captured(&self) -> Vec<Vec<u8>> {
            self.captured.lock().expect("capture lock").clone()
        }

        fn join(&mut self) {
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    #[test]
    fn an_over_limit_read_is_reported_as_a_size_refusal_not_a_retried_transport_failure() {
        // #270's oversized-dump fix rests on `is_body_limit` recognising ureq's
        // BodyExceedsLimit, which ureq only exposes as Display text on an
        // `Error::Io` - so a ureq bump that rephrases it would silently un-fix the
        // bug. Nothing else in the suite provokes the condition, so this does: a
        // REAL over-limit read of a REAL socket response, asserted on the two
        // behaviours that flip together if the match stops working.
        //
        // The limit is passed in rather than using DUMP_MAX_BYTES so the test costs
        // 4 KiB instead of 640 MiB; it is the same `read_body_limited` the dump
        // download uses, and the same ureq error.
        let stub = LoopbackStub::start_with_body_len(vec![200, 200, 200], 4096);
        let url = format!("{}/api/v2/projects/x/dump/", stub.base);
        let base = stub.base.clone();
        let err = with_retry(&base, false, true, Endpoint::DownloadDump, || {
            let resp = super::agent().get(&url).call()?;
            super::read_body_limited(resp, 1024)
        })
        .expect_err("a body over the ceiling cannot be read");

        // 1. It is a size refusal, naming the ceiling - NOT a transport failure.
        //    Misclassified, this read "Could not reach SpeleoDB at the configured
        //    endpoint", pointing the user at their network for a payload-size
        //    problem the server answered perfectly.
        match &err {
            SpeleoDbError::Parse(message) => assert!(
                message.contains("larger than the 640 MiB"),
                "the refusal names the ceiling that applied: {message}"
            ),
            other => panic!("expected a Parse size refusal, got: {other:?}"),
        }

        // 2. It did NOT retry. An oversized body is not transient, so retrying only
        //    re-downloads the whole payload to fail again - three times over, on the
        //    one response big enough for that to hurt. One served connection is the
        //    observable proof the non-transient branch was taken, and the stub is
        //    armed for three so a regression can actually show them.
        //
        //    Read WITHOUT joining: the stub is still parked in `accept()` for the two
        //    connections that must never come, so joining would hang the suite. Every
        //    attempt `with_retry` makes is captured before it returns, so the count is
        //    settled by the time we read it.
        assert_eq!(
            stub.captured().len(),
            1,
            "an over-limit body must be refused on the first read, not re-downloaded"
        );
        drop(stub);
    }

    #[test]
    fn loopback_read_retries_503_then_200() {
        // A real socket: an idempotent GET that the stub answers 503,503,200 must
        // succeed after the backoff, and the stub must have served 3 connections.
        let mut stub = LoopbackStub::start(vec![503, 503, 200]);
        let url = format!("{}/api/v2/projects/", stub.base);
        let base = stub.base.clone();
        let (status, _body) = with_retry(&base, false, true, Endpoint::ListProjects, || {
            let resp = super::agent().get(&url).call()?;
            super::read_status_body(resp)
        })
        .expect("read retries to success");
        assert_eq!(status, 200);
        stub.join();
        assert_eq!(stub.captured().len(), 3, "served 503, 503, 200");
    }

    #[test]
    fn loopback_write_attempted_once() {
        // A non-idempotent write that the stub answers 503 must hit the socket
        // exactly once (no double-submit), surfacing the 503.
        let mut stub = LoopbackStub::start(vec![503]);
        let url = format!("{}/api/v2/projects/", stub.base);
        let base = stub.base.clone();
        let (status, _body) = with_retry(&base, false, false, Endpoint::ListProjects, || {
            let resp = super::agent().post(&url).send("")?;
            super::read_status_body(resp)
        })
        .expect("write returns the 503");
        assert_eq!(status, 503);
        stub.join();
        assert_eq!(
            stub.captured().len(),
            1,
            "write hit the socket exactly once"
        );
    }

    #[test]
    fn loopback_multipart_parts_on_the_wire() {
        // The real multipart PUT must carry two `name="artifact"` parts with their
        // explicit Content-Type headers - asserted against the captured bytes.
        let dir = tempfile::tempdir().expect("tempdir");
        let stalx = dir.path().join("project.stalx");
        let tml = dir.path().join("ariane.tml");
        let mut f = std::fs::File::create(&stalx).expect("create stalx");
        f.write_all(b"SQLITE_BYTES").expect("write stalx");
        let mut g = std::fs::File::create(&tml).expect("create tml");
        g.write_all(b"<tml/>").expect("write tml");

        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        let project = super::upload_auto(
            &base,
            "tok",
            false,
            "11111111-1111-1111-1111-111111111111",
            &stalx,
            &tml,
            "wire test",
        );
        stub.join();
        // The stub returns `{}` which lacks id/name, so parsing fails - that's
        // fine; we only care about the bytes on the wire.
        let _ = project;
        let captured = stub.captured();
        assert_eq!(captured.len(), 1, "one PUT attempt");
        let wire = String::from_utf8_lossy(&captured[0]);
        let artifact_parts = wire.matches("name=\"artifact\"").count();
        assert_eq!(
            artifact_parts, 2,
            "two repeated `artifact` parts on the wire"
        );
        assert!(
            !wire.contains("artifact[]"),
            "never the bracketed `artifact[]` form"
        );
        assert!(
            wire.contains("application/vnd.sqlite3"),
            "the .stalx part carries its explicit MIME"
        );
        assert!(
            wire.contains("application/octet-stream"),
            "the .tml part carries its explicit MIME"
        );
        assert!(wire.contains("PUT "), "it is a PUT");
    }

    #[test]
    fn upload_parts_pin_the_canonical_repo_filenames() {
        // #208 repo-layout contract: every push lands the survey as `project.stalx`
        // and its render as `ariane.tml` REGARDLESS of the local source basenames.
        // The push uploads from scratch paths (`snapshot.stalx`) beside the live
        // cache file, so the multipart filename must be the CANONICAL server name,
        // never the source's basename -- else a second `.stalx` (or a `snapshot.stalx`)
        // would be committed and the next download_dump would be an ambiguous
        // multi-`.stalx` repo. Assert build_upload_parts decouples the two.
        let dir = tempfile::tempdir().expect("tempdir");
        let stalx_src = dir.path().join("snapshot.stalx");
        let tml_src = dir.path().join("render-42.tml");
        std::fs::write(&stalx_src, b"x").expect("write stalx");
        std::fs::write(&tml_src, b"y").expect("write tml");

        let parts = build_upload_parts(&stalx_src, &tml_src, "msg").expect("build parts");
        let names: Vec<&str> = parts.parts.iter().map(|p| p.file_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["project.stalx", "ariane.tml"],
            "#208: canonical wire names, decoupled from the source basenames"
        );
        // Both parts are the repeated `artifact` field (never `artifact[]`).
        assert!(
            parts.parts.iter().all(|p| p.name == "artifact"),
            "each file is a repeated `artifact` part"
        );
        assert_eq!(parts.message, "msg");
    }

    // --- #215 collaborator/team permission + delete wire contract ---------------
    //
    // These pin the exact method + path + JSON body each new client fn puts on the
    // wire, against the same LoopbackStub the multipart test uses. The load-bearing
    // case is the DELETE-with-body: ureq's typestate `.delete()` builder cannot
    // carry a body, so the remove-* fns hand-build an `http::Request` and go through
    // `Agent::run`. If that body were ever dropped (a ureq bump, or a refactor back
    // to `.delete()`), the server would receive a target-less DELETE and 400 - so
    // asserting `{"user"}` / `{"team"}` actually reaches the socket is the point.
    // The stub replies `{}`; the fns that return `()` treat 200 as success, so we
    // assert the returned `Ok(())` too (the ones that parse a body are ignored - we
    // only care about the request bytes).
    const WIRE_PID: &str = "11111111-1111-1111-1111-111111111111";

    #[test]
    fn loopback_remove_user_permission_is_delete_with_user_body() {
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        remove_user_permission(&base, "tok", false, WIRE_PID, "bob@example.com")
            .expect("200 -> Ok(())");
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with(&format!(
                "DELETE /api/v2/projects/{WIRE_PID}/permission/user/detail/ "
            )),
            "singular `permission/user/detail/` DELETE, not the plural list path: {wire}"
        );
        assert!(
            wire.to_ascii_lowercase()
                .contains("content-type: application/json"),
            "carries a JSON content-type: {wire}"
        );
        assert!(
            wire.contains(r#""user":"bob@example.com""#),
            "the DELETE body identifies the target user on the wire: {wire}"
        );
    }

    #[test]
    fn loopback_remove_team_permission_is_delete_with_team_body() {
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        remove_team_permission(&base, "tok", false, WIRE_PID, "team-uuid-9")
            .expect("200 -> Ok(())");
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with(&format!(
                "DELETE /api/v2/projects/{WIRE_PID}/permission/team/detail/ "
            )),
            "singular `permission/team/detail/` DELETE: {wire}"
        );
        assert!(
            wire.contains(r#""team":"team-uuid-9""#),
            "the DELETE body identifies the target team on the wire: {wire}"
        );
    }

    #[test]
    fn loopback_add_user_permission_is_post_with_user_and_level() {
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        add_user_permission(
            &base,
            "tok",
            false,
            WIRE_PID,
            "bob@example.com",
            Permission::ReadOnly,
        )
        .expect("200 -> Ok(())");
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with(&format!(
                "POST /api/v2/projects/{WIRE_PID}/permission/user/detail/ "
            )),
            "POST to the singular detail path: {wire}"
        );
        assert!(
            wire.contains(r#""user":"bob@example.com""#),
            "user on wire: {wire}"
        );
        assert!(
            wire.contains(r#""level":"READ_ONLY""#),
            "the typed Permission serializes to its wire label: {wire}"
        );
    }

    #[test]
    fn loopback_set_user_permission_is_put() {
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        set_user_permission(
            &base,
            "tok",
            false,
            WIRE_PID,
            "bob@example.com",
            Permission::ReadAndWrite,
        )
        .expect("200 -> Ok(())");
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with(&format!(
                "PUT /api/v2/projects/{WIRE_PID}/permission/user/detail/ "
            )),
            "a level change is a PUT: {wire}"
        );
        assert!(
            wire.contains(r#""level":"READ_AND_WRITE""#),
            "new level on wire: {wire}"
        );
    }

    #[test]
    fn loopback_add_team_permission_is_post_with_team_and_level() {
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        add_team_permission(
            &base,
            "tok",
            false,
            WIRE_PID,
            "team-uuid-9",
            Permission::ReadOnly,
        )
        .expect("200 -> Ok(())");
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with(&format!(
                "POST /api/v2/projects/{WIRE_PID}/permission/team/detail/ "
            )),
            "team grant POST: {wire}"
        );
        assert!(
            wire.contains(r#""team":"team-uuid-9""#),
            "team on wire: {wire}"
        );
        assert!(
            wire.contains(r#""level":"READ_ONLY""#),
            "level on wire: {wire}"
        );
    }

    #[test]
    fn loopback_list_user_permissions_is_get_plural_path() {
        // The list path is PLURAL (`permissions/user/`), distinct from the singular
        // write path (`permission/user/detail/`) - a mismatch would 404 the roster.
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        // Stub replies `{}` -> `permissions` defaults to [] -> Ok(empty). We only
        // assert the request bytes.
        let _ = list_user_permissions(&base, "tok", false, WIRE_PID);
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with(&format!(
                "GET /api/v2/projects/{WIRE_PID}/permissions/user/ "
            )),
            "plural `permissions/user/` GET: {wire}"
        );
    }

    #[test]
    fn loopback_list_teams_is_get_teams() {
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        let _ = list_teams(&base, "tok", false);
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with("GET /api/v2/teams/ "),
            "the teams picker/name-map GET: {wire}"
        );
    }

    #[test]
    fn loopback_user_autocomplete_get_percent_encodes_query() {
        // A partial name with a space must be percent-encoded into the query string
        // (a raw space would produce an invalid request line / truncated query).
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        let _ = list_user_autocomplete(&base, "tok", false, "jo di");
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with("GET /api/v2/user/autocomplete/?query=jo%20di "),
            "GET the autocomplete endpoint with a percent-encoded query: {wire}"
        );
    }

    #[test]
    fn loopback_delete_project_is_bare_delete() {
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        delete_project(&base, "tok", false, WIRE_PID).expect("200 -> Ok(())");
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with(&format!("DELETE /api/v2/projects/{WIRE_PID}/ ")),
            "project delete is a bare DELETE on the project, no detail suffix: {wire}"
        );
    }

    #[test]
    fn loopback_download_at_hash_pins_the_revision_sha() {
        // #218 restore: the at-hash download interpolates the revision SHA into the
        // path so the server serves that commit's tree (FileDownloadAtHashView), not
        // HEAD. A missing/misplaced SHA would silently restore the wrong revision.
        let mut stub = LoopbackStub::start(vec![200]);
        let base = stub.base.clone();
        let _ = download_dump_at_hash(&base, "tok", false, WIRE_PID, "abc1234");
        stub.join();
        let wire = String::from_utf8_lossy(&stub.captured()[0]).into_owned();
        assert!(
            wire.starts_with(&format!(
                "GET /api/v2/projects/{WIRE_PID}/download/dump/abc1234/ "
            )),
            "at-hash download GETs the dump pinned to the revision SHA: {wire}"
        );
    }

    #[test]
    fn dump_download_gets_its_own_longer_budget() {
        // #270: `timeout_global` is end-to-end INCLUDING the body read, so a dump
        // (the whole working tree as a ZIP) on the shared 30 s small-GET budget
        // fails all three idempotent attempts while a same-size upload succeeds.
        assert_eq!(
            super::dump_agent().config().timeouts().global,
            Some(DUMP_DOWNLOAD_TIMEOUT),
            "the dump agent must carry the dump budget"
        );
        assert_eq!(
            super::agent().config().timeouts().global,
            Some(REQUEST_TIMEOUT),
            "the shared read agent keeps the small-GET budget"
        );
        assert!(
            DUMP_DOWNLOAD_TIMEOUT > REQUEST_TIMEOUT,
            "the dump budget must be longer than the shared read budget"
        );
    }

    #[test]
    fn both_dump_downloads_use_the_dump_agent() {
        // The agent a function picks is not observable from its return value, so
        // guard it at the source: BOTH dump endpoints must build `dump_agent()`. A
        // future edit that reaches for the bare `agent()` silently reinstates the
        // 30 s budget on one of them.
        let src = include_str!("client.rs");
        for name in ["pub fn download_dump(", "pub fn download_dump_at_hash("] {
            let start = src.find(name).expect("dump download fn present");
            let body = &src[start..];
            let end = body.find("\n}\n").expect("fn body terminates");
            let body = &body[..end];
            assert!(
                body.contains("dump_agent()"),
                "{name} must build its agent via dump_agent()"
            );
            // Blank out the legitimate calls first, then look for ANY `agent()` left
            // over. Matching on a leading-space needle instead let two spellings
            // through: a qualified `client::agent()`, and a rustfmt wrap that puts
            // `agent()` at the start of an indented continuation line - both of which
            // reinstate the 30 s budget while the test stays green.
            let remaining = body.replace("dump_agent()", "");
            assert!(
                !remaining.contains("agent()"),
                "{name} must NOT fall back to the shared 30 s agent()"
            );
            assert!(
                body.contains("read_dump_body(resp)"),
                "{name} must read its body on the dump ceiling, not the 10 MiB default \
                 `read_to_vec()` applies via read_status_body"
            );
        }
    }

    #[test]
    fn dump_download_accepts_a_body_over_ureqs_default_limit() {
        // The real defect behind the longer dump timeout: `Body::read_to_vec()`
        // silently caps at ureq's own 10 MiB default, so a dump larger than that
        // could never be opened, restored or fast-forwarded at ANY timeout - and
        // the refusal arrived as an io error that classified as "could not reach
        // the endpoint", for a perfectly healthy 200, after three attempts.
        //
        // Serve just over the default and require the whole body to arrive.
        const SIZE: usize = 10 * 1024 * 1024 + 4096;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Read the request headers, then answer. A GET has no body, so the
            // first read is the whole request.
            use std::io::Read as _;
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(250)));
            let mut chunk = vec![0u8; 16 * 1024];
            let _ = stream.read(&mut chunk);
            let resp =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {SIZE}\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&vec![b'z'; SIZE]);
            let _ = stream.flush();
        });

        let base = format!("http://127.0.0.1:{port}");
        let body = download_dump(&base, "tok", false, WIRE_PID).expect("an 10 MiB+ dump downloads");
        let _ = handle.join();
        assert_eq!(
            body.len(),
            SIZE,
            "the whole dump must arrive, not ureq's 10 MiB prefix"
        );
        assert!(
            DUMP_MAX_BYTES > 10 * 1024 * 1024,
            "the dump ceiling must be above the default it exists to replace"
        );
    }

    #[test]
    fn the_dump_ceiling_is_coherent_with_the_per_entry_ceiling() {
        // Both directions are load-bearing.
        //
        // ABOVE the per-entry cap: the extractable `.stalx` is the whole reason a dump
        // is downloaded, so the body must be able to carry one at its full allowed size
        // plus the rest of the archive.
        //
        // But NOT far above it: the earlier 2 GiB left ~1.5 GiB of allowed body that
        // could only ever be downloaded, held in memory and then rejected by the
        // per-entry cap - a guaranteed-futile multi-gigabyte allocation in an app with
        // documented WebKitGTK memory-ceiling trouble. The whole body is buffered
        // (`read_to_vec` is `read_to_end`) and the extract then builds a SECOND buffer
        // on top of it, so this constant is a peak-RSS budget.
        const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
        assert!(
            DUMP_MAX_BYTES > MAX_ENTRY_BYTES,
            "the body ceiling must admit a full-size extractable entry"
        );
        assert!(
            DUMP_MAX_BYTES <= 2 * MAX_ENTRY_BYTES,
            "and must not admit a body the per-entry cap can never yield anything from"
        );
    }

    #[test]
    fn the_body_limit_message_names_the_limit_that_actually_applied() {
        // `with_retry` is shared by all thirteen endpoints, but only the two dump
        // downloads read on the dump ceiling; the other eleven use `read_status_body`,
        // i.e. ureq's own 10 MiB default. Quoting the dump ceiling at a JSON endpoint
        // told the user their reply had exceeded a limit many times the one it hit.
        assert_eq!(body_limit_for(Endpoint::DownloadDump), DUMP_MAX_BYTES);
        assert_eq!(body_limit_for(Endpoint::ListProjects), DEFAULT_MAX_BYTES);
        assert_eq!(body_limit_for(Endpoint::Upload), DEFAULT_MAX_BYTES);

        // And it is rendered in units a human reads, not as a raw byte count
        // ("2147483648-byte" was the old message).
        assert_eq!(human_bytes(DEFAULT_MAX_BYTES), "10 MiB");
        assert_eq!(human_bytes(DUMP_MAX_BYTES), "640 MiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3 GiB");
        assert_eq!(human_bytes(2048), "2 KiB");
        assert_eq!(human_bytes(17), "17 bytes");
    }

    #[test]
    fn agents_never_follow_a_redirect() {
        // SECURITY. `tls::enforce` vets the CONFIGURED base URL once per request and
        // says nothing about where a redirect leads, so with ureq's default of 10
        // redirects a `302` to `http://<anything>/` was followed in plaintext straight
        // past the private-network policy that exists to forbid exactly that. Zero
        // redirects means the 3xx returns as an ordinary status and `status_error` maps
        // it like any other unexpected response.
        assert_eq!(
            super::agent().config().max_redirects(),
            0,
            "the shared agent must not follow redirects"
        );
        assert_eq!(
            super::dump_agent().config().max_redirects(),
            0,
            "nor the dump agent"
        );
        // Credentials were never the exposure here - ureq drops `Authorization` on a
        // cross-host hop - but pin that too so a config change cannot quietly turn a
        // redirect into a token leak on top.
        assert!(
            matches!(
                super::agent().config().redirect_auth_headers(),
                ureq::config::RedirectAuthHeaders::Never
            ),
            "auth headers must never ride a redirect"
        );
    }

    #[test]
    fn a_redirect_is_reported_as_a_status_not_followed() {
        // End-to-end over loopback: the server answers `302` pointing somewhere else.
        // With redirects disabled the caller sees the 302 itself (a `Status` error),
        // never a second request to the redirect target.
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::clone(&hits);
        let handle = std::thread::spawn(move || {
            use std::io::Read as _;
            // Two accepts on purpose: if the agent followed the redirect there would be
            // a SECOND connection, and the count would show it.
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                seen.fetch_add(1, Ordering::SeqCst);
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(250)));
                let mut chunk = vec![0u8; 8 * 1024];
                let _ = stream.read(&mut chunk);
                let _ = stream.write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: http://example.com/evil\r\n\
                      Content-Length: 0\r\nConnection: close\r\n\r\n",
                );
                let _ = stream.flush();
            }
        });

        let base = format!("http://127.0.0.1:{port}");
        let err = download_dump(&base, "tok", false, WIRE_PID)
            .expect_err("a 302 must not be followed, and is not a success");
        assert!(
            matches!(err, SpeleoDbError::Status { status: 302, .. }),
            "the redirect must surface as its own status, got {err:?}"
        );
        drop(handle);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "exactly one request - the redirect target was never dialed"
        );
    }
}
