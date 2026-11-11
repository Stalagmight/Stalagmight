//! Synchronous application boundary around the published async SpeleoDB SDK.
//! Only native Stalagmight artifacts cross this boundary.

use std::future::Future;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use speleodb_api::{types, ApiOpError, HttpClient, HttpError};
use tokio::io::AsyncWriteExt;

use super::metrics::{Endpoint, Outcome};
use super::models::{
    ActiveMutex, Commit, Permission, Project, RevisionCommit, RevisionHistory, Visibility,
};
use super::native::NativeFile;
use super::{SpeleoDbError, TransportCause};

mod commit_dates;

const READ_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn upload_timeout(bytes: usize) -> Duration {
    TRANSFER_TIMEOUT + Duration::from_secs((bytes as u64).div_ceil(16 * 1024))
}

/// The slowest average rate a download may keep up once its grace
/// ([`TRANSFER_TIMEOUT`]) has run, in bytes per second: 32 kbit/s, a quarter of
/// the upload budget's rate. Generous on purpose - a download only has a read
/// timeout otherwise, so a server trickling a byte now and then held the transfer
/// (and, through the quit-after-work wait, the app's exit) for ever.
const DOWNLOAD_MIN_RATE: u64 = 4 * 1024;

/// The longest any download may run, whatever it has received: the deadline of
/// the largest file a SpeleoDB server may hold ([`LARGEST_STORED_BYTES`]) at
/// [`DOWNLOAD_MIN_RATE`], 120 + 6,020 s (about 1 h 42 min). Without it the
/// per-byte deadline alone let a server trickling just above the minimum rate
/// hold a download - and the app's exit waiting for it - for some 36 hours
/// within the native size limit. Derived rather than picked: every project a
/// server can have stored still downloads at the minimum rate, so only a server
/// sending more than it could have stored, or slower than the minimum, is cut.
const DOWNLOAD_CEILING: Duration = Duration::from_secs(
    TRANSFER_TIMEOUT.as_secs() + LARGEST_STORED_BYTES.div_ceil(DOWNLOAD_MIN_RATE),
);

/// How long a download may have run once it has received `bytes`: `grace` plus
/// `bytes` at `min_rate`, the shape of [`upload_timeout`], and never more than
/// `ceiling` ([`DOWNLOAD_CEILING`]). The SDK's stream does not expose the reply's
/// `Content-Length`, so the size is what has arrived so far: a download of N
/// bytes must end within `grace + N / min_rate` of its start, and none outlasts
/// the ceiling.
fn download_deadline(grace: Duration, min_rate: u64, ceiling: Duration, bytes: u64) -> Duration {
    (grace + Duration::from_millis(bytes.saturating_mul(1000) / min_rate.max(1))).min(ceiling)
}
const JSON_LIMIT: usize = 10 * 1024 * 1024;
/// The largest native project file (`.stalx`) a download accepts, in bytes.
/// Public so a local read of a `.stalx` (adding one into the open project) can
/// hold a file to the same ceiling the app already holds a pulled one to.
pub const NATIVE_LIMIT: u64 = 512 * 1024 * 1024;

/// The largest project file SpeleoDB accepts in one upload, in bytes: 5 MiB. A
/// larger file is refused with a 400 only AFTER its whole body has been sent, so
/// the push path checks against this first (sdk#3).
///
/// SpeleoDB e094fd501 (2026-11-10, shipped with the ISO revision dates and SDK
/// 0.6.2) limits each uploaded file to `DJANGO_GIT_UPLOAD_INDIVIDUAL_FILESIZE_MB_LIMIT`
/// = 5 MiB, inclusive, and a commit to 7 MiB; an upload is one `.stalx`, so the
/// per-file limit is the one that binds. An older server accepted up to
/// [`LARGEST_STORED_BYTES`] (about 23.5 MiB), and uploads to it are held to the
/// newer limit too: a project between the two now leaves out its undo history,
/// and one still over 5 MiB without it is refused, where that server would have
/// taken it. The app cannot tell the server's limit from its replies, and the
/// servers that matter are moving to the newer one.
pub const SERVER_UPLOAD_LIMIT_BYTES: u64 = 5 * 1024 * 1024;

/// The largest project file a SpeleoDB server may hold, in bytes: what a server
/// from before e094fd501 accepted in one upload. Its check compared the upload
/// against `DJANGO_UPLOAD_TOTAL_FILES_LIMIT * 1024 * 1204`: the maximum file COUNT
/// (20) rather than a size, and `1204` a typo for `1024`, so 20 * 1024 * 1204 =
/// 24,657,920 bytes (about 23.5 MiB). Projects that large may still be stored, so
/// downloads are budgeted for them ([`DOWNLOAD_CEILING`]).
pub const LARGEST_STORED_BYTES: u64 = 24_657_920;

#[derive(Clone, Copy)]
struct Settings {
    timeout: Option<Duration>,
    retries: u32,
    native_limit: u64,
    upload_limit: u64,
    /// A download's total deadline ([`download_deadline`]): its grace, the
    /// minimum average rate after it, and the absolute ceiling.
    download_grace: Duration,
    download_min_rate: u64,
    download_ceiling: Duration,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout: None,
            retries: 2,
            native_limit: NATIVE_LIMIT,
            upload_limit: SERVER_UPLOAD_LIMIT_BYTES,
            download_grace: TRANSFER_TIMEOUT,
            download_min_rate: DOWNLOAD_MIN_RATE,
            download_ceiling: DOWNLOAD_CEILING,
        }
    }
}

fn runtime() -> Result<&'static tokio::runtime::Runtime, SpeleoDbError> {
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|_| SpeleoDbError::Io("could not initialize the SpeleoDB runtime".into()))
}

struct SdkClient {
    http: HttpClient,
    settings: Settings,
}
impl SdkClient {
    fn new(
        base: &str,
        token: Option<&str>,
        insecure: bool,
        timeout: Duration,
        settings: Settings,
    ) -> Result<Self, SpeleoDbError> {
        super::tls::enforce(base, insecure)?;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(if timeout.is_zero() {
                settings.timeout.unwrap_or(TRANSFER_TIMEOUT)
            } else {
                TRANSFER_TIMEOUT
            });
        if !timeout.is_zero() {
            builder = builder.timeout(settings.timeout.unwrap_or(timeout));
        }
        let transport = builder
            .build()
            .map_err(|_| SpeleoDbError::Io("could not initialize the SpeleoDB transport".into()))?;
        let mut http = HttpClient::with_client(transport)
            .with_base_url(base.trim_end_matches('/'))
            .with_max_response_body_bytes(JSON_LIMIT);
        if let Some(token) = token {
            http = http.with_api_key(token);
        }
        Ok(Self { http, settings })
    }

    fn call<T, F, Fut>(
        &self,
        endpoint: Endpoint,
        retry_safe: bool,
        mut operation: F,
    ) -> Result<T, SpeleoDbError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, SpeleoDbError>>,
    {
        runtime()?.block_on(async {
            let retries = if retry_safe { self.settings.retries } else { 0 };
            for attempt in 0..=retries {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_millis(if attempt == 1 { 200 } else { 500 }))
                        .await;
                }
                let started = Instant::now();
                let result = operation().await;
                // sdk#8: a contract mismatch is the failure most likely to need a
                // bug report (a new server enum value fails every call that
                // decodes it), so name the call and the field in the log.
                if let Err(SpeleoDbError::Parse(detail)) = &result {
                    tracing::warn!(
                        endpoint = endpoint.kind(),
                        detail = %detail,
                        "SpeleoDB reply does not match the expected contract"
                    );
                }
                super::metrics::record(endpoint, attempt_outcome(&result), started.elapsed());
                let transient = matches!(&result, Err(SpeleoDbError::Transport { .. }))
                    || matches!(
                        &result,
                        Err(SpeleoDbError::Status {
                            status: 500..=599,
                            ..
                        })
                    );
                if !transient || attempt == retries {
                    return result;
                }
            }
            unreachable_result()
        })
    }

    fn download(&self, id: &str, revision: Option<&str>) -> Result<NativeFile, SpeleoDbError> {
        self.call(Endpoint::DownloadStalx, true, || async {
            // The total deadline, on top of the read timeout: see `download_deadline`.
            let started = tokio::time::Instant::now();
            let deadline = |received: u64| {
                started
                    + download_deadline(
                        self.settings.download_grace,
                        self.settings.download_min_rate,
                        self.settings.download_ceiling,
                        received,
                    )
            };
            let file = NativeFile::new()?;
            let mut output = tokio::fs::File::create(file.path())
                .await
                .map_err(io_error)?;
            let request = async {
                let chunks: std::pin::Pin<
                    Box<dyn futures_util::Stream<Item = Result<_, reqwest::Error>>>,
                > = match revision {
                    Some(sha) => Box::pin(
                        self.http
                            .v2_projects_download_retrieve_by_format_at_hash_binary_stream(
                                "stalagmight",
                                sha,
                                id,
                                None,
                            )
                            .await
                            .map_err(api_error)?,
                    ),
                    None => Box::pin(
                        self.http
                            .v2_projects_download_retrieve_by_format_binary_stream(
                                "stalagmight",
                                id,
                                None,
                            )
                            .await
                            .map_err(api_error)?,
                    ),
                };
                Ok::<_, SpeleoDbError>(chunks)
            };
            let mut stream = tokio::time::timeout_at(deadline(0), request)
                .await
                .map_err(|_| download_too_slow())??;
            let mut received = 0_u64;
            loop {
                let Some(chunk) = tokio::time::timeout_at(deadline(received), stream.next())
                    .await
                    .map_err(|_| download_too_slow())?
                else {
                    break;
                };
                let chunk = chunk.map_err(network_error)?;
                received = received.saturating_add(chunk.len() as u64);
                if received > self.settings.native_limit {
                    return Err(SpeleoDbError::Parse(
                        "native project exceeds the download size limit".into(),
                    ));
                }
                output.write_all(&chunk).await.map_err(io_error)?;
            }
            output.flush().await.map_err(io_error)?;
            drop(output);
            Ok(file)
        })
    }
}

/// The traffic-metrics outcome of one attempt. A [`SpeleoDbError::Parse`] is its
/// own error kind, never `Ok` (sdk#8): the server answered, but not in a shape
/// the app can use.
fn attempt_outcome<T>(result: &Result<T, SpeleoDbError>) -> Outcome {
    match result {
        Ok(_) => Outcome::Ok,
        Err(SpeleoDbError::Parse(_)) => Outcome::ParseError,
        Err(SpeleoDbError::Status { status, .. }) => Outcome::from_status(*status),
        Err(_) => Outcome::NetworkError,
    }
}
fn unreachable_result<T>() -> Result<T, SpeleoDbError> {
    Err(SpeleoDbError::Io(
        "invalid SpeleoDB retry configuration".into(),
    ))
}
fn io_error(error: std::io::Error) -> SpeleoDbError {
    SpeleoDbError::Io(error.to_string())
}
/// A download that ran past its total deadline ([`download_deadline`]): a
/// timeout, like the read timeout that a trickling server stays inside.
fn download_too_slow() -> SpeleoDbError {
    SpeleoDbError::Transport {
        cause: TransportCause::Timeout,
        detail: "the download was too slow to finish in time".into(),
    }
}
/// `true` when a TLS handshake or certificate check is what failed: a typed
/// `rustls::Error` anywhere in the error tree. tokio-rustls reports it inside an
/// `io::Error`, which hyper-rustls wraps in a second one, and `io::Error::source`
/// skips the error it wraps - so every `io::Error` is also opened with
/// `get_ref`. Typed, never text-matched.
fn is_tls_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut pending = vec![error];
    while let Some(link) = pending.pop() {
        if link.is::<rustls::Error>() {
            return true;
        }
        if let Some(wrapped) = link
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
        {
            pending.push(wrapped);
        }
        pending.extend(link.source());
    }
    false
}
fn network_error(error: reqwest::Error) -> SpeleoDbError {
    use std::error::Error;
    let mut cause = if error.is_timeout() {
        TransportCause::Timeout
    } else if error.is_dns() {
        TransportCause::Dns
    } else if is_tls_failure(&error) {
        TransportCause::Tls
    } else {
        TransportCause::Unknown
    };
    let mut source = error.source();
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            match io.kind() {
                std::io::ErrorKind::TimedOut => cause = TransportCause::Timeout,
                std::io::ErrorKind::NetworkUnreachable
                | std::io::ErrorKind::HostUnreachable
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe => cause = TransportCause::Refused,
                _ => {}
            }
        }
        source = current.source();
    }
    SpeleoDbError::Transport {
        cause,
        detail: error.without_url().to_string(),
    }
}
/// The longest SDK decoder detail kept on a contract-mismatch error. serde's
/// "unknown variant" message lists every accepted value (about 250 for a
/// country), which would bury the one that failed.
const CONTRACT_DETAIL_LIMIT: usize = 240;

/// Append `text` to `out` with every quoted or backticked span replaced by
/// `"..."` / `` `...` ``. Returns whether `text` ended inside a span (the caller
/// then drops whatever follows, since the span may run into it). Only a quoted
/// string escapes (serde Debug-formats it); a backtick span is the value verbatim,
/// so it closes at the next backtick - which is why the server's value is not
/// scanned with this (see [`redact_server_values`]).
fn redact_spans(text: &str, out: &mut String) -> bool {
    let mut chars = text.chars();
    // The delimiter of the response value being skipped, while inside one.
    let mut open: Option<char> = None;
    while let Some(c) = chars.next() {
        match (open, c) {
            (None, '"' | '`') => {
                open = Some(c);
                out.push(c);
                out.push_str("...");
                out.push(c);
            }
            (Some(delimiter), c) if c == delimiter => open = None,
            (Some('"'), '\\') => {
                chars.next();
            }
            (Some(_), _) => {}
            (None, c) => out.push(c),
        }
    }
    open.is_some()
}

/// A serde decoder message with the server's value redacted.
///
/// serde puts at most one value from the reply in a message, ahead of what the
/// schema wanted: `` unknown variant `X`, expected `A` or `B` ``,
/// `` invalid type: string "X", expected u32 ``. A backticked value is printed
/// verbatim, so it can itself hold a backtick or the text `, expected`: closing
/// its span at the first backtick leaked the rest (`` `a`LEAK` ``). So the value
/// part is everything before the LAST `, expected` (the schema's side never
/// holds one), and a backtick span there runs from its first backtick to its
/// last. A quoted value is escaped, so its span ends where the quotes say. The
/// schema's side is redacted span by span as before. Whatever follows a span
/// that never closes is dropped.
fn redact_server_values(detail: &str) -> String {
    let (value, expected) = detail
        .rfind(", expected")
        .map_or((detail, ""), |at| detail.split_at(at));
    let mut out = String::with_capacity(detail.len());
    let first = value.find(['"', '`']);
    let open = match first {
        Some(first) if value[first..].starts_with('`') => {
            out.push_str(&value[..first]);
            out.push_str("`...`");
            match value.rfind('`').filter(|&last| last > first) {
                Some(last) => redact_spans(&value[last + 1..], &mut out),
                None => true,
            }
        }
        _ => redact_spans(value, &mut out),
    };
    if !open {
        redact_spans(expected, &mut out);
    }
    out
}

/// The [`SpeleoDbError::Parse`] text for a 2xx reply the SDK could not decode.
///
/// Keeps the SDK's decoder detail (what kind of mismatch, and where) so the log
/// and the error dialog can name it (sdk#8). Every response value in it is
/// redacted: serde quotes a string value (`string "..."`, which can be user text
/// such as a project description) and puts numbers, unknown variants and unknown
/// field names in backticks (`` floating point `20.48` ``, which can be a cave's
/// coordinates). The detail is capped at [`CONTRACT_DETAIL_LIMIT`] characters.
fn contract_mismatch(detail: Option<String>) -> String {
    const MISMATCH: &str = "response does not match the SpeleoDB SDK contract";
    let Some(detail) = detail else {
        return MISMATCH.into();
    };
    let detail = detail
        .trim()
        .trim_start_matches("failed to deserialize 2xx response body:")
        .trim();
    let redacted = redact_server_values(detail);
    let detail = redacted.trim();
    if detail.is_empty() {
        return MISMATCH.into();
    }
    format!("{MISMATCH}: {}", bounded(detail))
}

/// `text` cut to [`CONTRACT_DETAIL_LIMIT`] characters, with `...` when it was cut.
/// Every piece of server-chosen text an error carries goes through this: an error
/// reply can be as large as the JSON limit, and its message reaches the log, the
/// error dialogs, the removal warning and the purge report.
fn bounded(text: &str) -> String {
    match text.char_indices().nth(CONTRACT_DETAIL_LIMIT) {
        Some((cut, _)) => format!("{}...", &text[..cut]),
        None => text.to_owned(),
    }
}
fn api_error<E: std::fmt::Debug>(error: ApiOpError<E>) -> SpeleoDbError {
    match error {
        ApiOpError::Api(error) if (200..300).contains(&error.status) => {
            SpeleoDbError::Parse(contract_mismatch(error.parse_error))
        }
        ApiOpError::Api(error) => SpeleoDbError::Status {
            status: error.status,
            message: parse_error_message(&error.raw_body)
                .unwrap_or_else(|| format!("HTTP {}", error.status)),
        },
        ApiOpError::Transport(HttpError::Network(error)) => network_error(error),
        // A send failure. Only the timeout and the TLS failure are classified
        // here; refused / reset / DNS through this arm stay Unknown (tracked
        // separately as F1, deliberately not changed with the TLS mapping).
        ApiOpError::Transport(HttpError::Middleware(error)) => SpeleoDbError::Transport {
            cause: if error.is_timeout() {
                TransportCause::Timeout
            } else if is_tls_failure(&error) {
                TransportCause::Tls
            } else {
                TransportCause::Unknown
            },
            detail: error.without_url().to_string(),
        },
        ApiOpError::Transport(HttpError::Timeout) => SpeleoDbError::Transport {
            cause: TransportCause::Timeout,
            detail: "request timed out".into(),
        },
        ApiOpError::Transport(HttpError::ResponseTooLarge { .. }) => {
            SpeleoDbError::Parse("response exceeds the download size limit".into())
        }
        ApiOpError::Transport(_) => {
            SpeleoDbError::Parse("SpeleoDB request could not be processed".into())
        }
    }
}
/// The reply of an SDK call that decodes a project or a revision list. A 2xx reply
/// the SDK could not deserialize is decoded again with the commit dates a pre-ISO
/// server writes set aside ([`commit_dates::decode_reply`]); any other refusal,
/// an unexpected success status included, stays a contract mismatch.
fn sdk_reply<T: serde::de::DeserializeOwned, E: std::fmt::Debug>(
    result: Result<T, ApiOpError<E>>,
) -> Result<commit_dates::Reply<T>, SpeleoDbError> {
    match result {
        Ok(value) => Ok(commit_dates::Reply::typed(value)),
        Err(ApiOpError::Api(error))
            if (200..300).contains(&error.status)
                && error
                    .parse_error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("failed to deserialize 2xx response body")) =>
        {
            let reply = match commit_dates::decode_reply(&error.raw_body) {
                Some(Ok(reply)) => reply,
                Some(Err(detail)) => {
                    return Err(SpeleoDbError::Parse(contract_mismatch(Some(
                        detail.to_string(),
                    ))))
                }
                None => return Err(SpeleoDbError::Parse(contract_mismatch(error.parse_error))),
            };
            static NOTED: OnceLock<()> = OnceLock::new();
            NOTED.get_or_init(|| {
                tracing::info!(
                    "SpeleoDB sent commit dates the SDK cannot read (a server from before \
                     its ISO revision dates); revision times are read in the old format \
                     or not shown"
                );
            });
            Ok(reply)
        }
        Err(error) => Err(api_error(error)),
    }
}
/// The message of a SpeleoDB error reply, [`bounded`]: the server chooses it, and
/// it may be as long as the reply.
fn parse_error_message(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    for key in ["error", "detail"] {
        if let Some(message) = value.get(key).and_then(serde_json::Value::as_str) {
            return Some(bounded(message));
        }
    }
    fn first(v: &serde_json::Value) -> Option<&str> {
        v.as_str().or_else(|| v.as_array()?.first()?.as_str())
    }
    let errors = value.get("errors")?.as_object()?;
    errors
        .get("non_field_errors")
        .and_then(first)
        .or_else(|| errors.values().find_map(first))
        .map(bounded)
}
fn project(value: types::Project) -> Result<Project, SpeleoDbError> {
    let permission = value
        .permission
        .flatten()
        .map(|p| match p.as_str() {
            "WEB_VIEWER" => Ok(Permission::WebViewer),
            "READ_ONLY" => Ok(Permission::ReadOnly),
            "READ_AND_WRITE" => Ok(Permission::ReadAndWrite),
            "ADMIN" => Ok(Permission::Admin),
            _ => Err(SpeleoDbError::Parse("unknown project permission".into())),
        })
        .transpose()?;
    let active_mutex = value.active_mutex.map(|v| ActiveMutex {
        user: match v {
            types::ProjectActiveMutex::ProjectActiveMutexInlineVariant(m) => m.user,
            types::ProjectActiveMutex::String(user) => user,
        },
    });
    let latest_commit = value.latest_commit.map(|v| Commit {
        id: match v {
            types::ProjectLatestCommit::ProjectLatestCommitInlineVariant(c) => c.id,
            types::ProjectLatestCommit::String(id) => id,
        },
    });
    Ok(Project {
        modified_date: Some(value.modified_date.to_rfc3339()),
        id: value.id.to_string(),
        name: value.name,
        permission,
        active_mutex,
        latest_commit,
        kind: value.r#type.to_string().to_ascii_lowercase(),
        exclude_geojson: value.exclude_geojson,
        description: Some(value.description),
        country: Some(value.country.to_string()),
        visibility: Some(match value.visibility {
            types::ProjectVisibility::Private => Visibility::Private,
            types::ProjectVisibility::Public => Visibility::Public,
        }),
        latitude: value.latitude,
        longitude: value.longitude,
    })
}
fn client(
    base: &str,
    token: Option<&str>,
    insecure: bool,
    timeout: Duration,
) -> Result<SdkClient, SpeleoDbError> {
    SdkClient::new(base, token, insecure, timeout, settings())
}
/// The transport settings in force: the defaults, or a test's override.
fn settings() -> Settings {
    #[cfg(test)]
    if let Some(settings) = TEST_SETTINGS.with(std::cell::Cell::get) {
        return settings;
    }
    Settings::default()
}

pub fn login(
    base: &str,
    insecure: bool,
    email: &str,
    password: &str,
) -> Result<String, SpeleoDbError> {
    let c = client(base, None, insecure, READ_TIMEOUT)?;
    login_with(&c, email, password)
}
fn login_with(c: &SdkClient, email: &str, password: &str) -> Result<String, SpeleoDbError> {
    c.call(Endpoint::Login, true, || async {
        let request = types::PatchedAuthTokenRequest {
            email: Some(email.into()),
            password: Some(password.into()),
        };
        c.http
            .v2_user_auth_token_create_with_json(None, None::<&str>, Some(request))
            .await
            .map(|v| v.token)
            .map_err(api_error)
    })
}
pub fn fetch_user_email(base: &str, token: &str, insecure: bool) -> Result<String, SpeleoDbError> {
    let c = client(base, Some(token), insecure, READ_TIMEOUT)?;
    c.call(Endpoint::FetchUser, true, || async {
        match c.http.v2_user_retrieve(None).await {
            Ok(user) => Ok(user.email),
            // Existing accounts may have an unset country. The SDK country enum
            // excludes the empty string; authentication only consumes the email.
            Err(ApiOpError::Api(error)) if error.status == 200 => {
                let value: serde_json::Value = serde_json::from_slice(&error.raw_body)
                    .map_err(|_| SpeleoDbError::Parse("invalid user profile".into()))?;
                if value.get("country").and_then(serde_json::Value::as_str) == Some("") {
                    if let Some(email) = value
                        .get("email")
                        .and_then(serde_json::Value::as_str)
                        .filter(|v| !v.trim().is_empty())
                    {
                        return Ok(email.to_owned());
                    }
                }
                Err(SpeleoDbError::Parse(contract_mismatch(error.parse_error)))
            }
            Err(error) => Err(api_error(error)),
        }
    })
}
pub fn list_projects(
    base: &str,
    token: &str,
    insecure: bool,
) -> Result<Vec<Project>, SpeleoDbError> {
    let c = client(base, Some(token), insecure, READ_TIMEOUT)?;
    c.call(Endpoint::ListProjects, true, || async {
        match c.http.v2_projects_list(None).await {
            Ok(values) => list_projection(
                serde_json::to_value(values)
                    .map_err(|_| SpeleoDbError::Parse("invalid project list".into()))?,
            ),
            // The SDK still owns transport, status handling and the bounded body.
            // List-only projection tolerates future enums without weakening mutations.
            Err(ApiOpError::Api(error)) if error.status == 200 => {
                let value = serde_json::from_slice(&error.raw_body)
                    .map_err(|_| SpeleoDbError::Parse("invalid project list".into()))?;
                list_projection(value)
            }
            Err(error) => Err(api_error(error)),
        }
    })
}
fn list_projection(value: serde_json::Value) -> Result<Vec<Project>, SpeleoDbError> {
    let invalid = || SpeleoDbError::Parse("invalid project list".into());
    let rows = value.as_array().ok_or_else(invalid)?;
    let mut projects = Vec::new();
    for row in rows {
        let id = row
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid)?;
        uuid::Uuid::parse_str(id).map_err(|_| invalid())?;
        let name = row
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid)?;
        let kind = row
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid)?;
        if kind != "STALAGMIGHT" {
            continue;
        }
        let permission = row
            .get("permission")
            .and_then(serde_json::Value::as_str)
            .and_then(|p| match p {
                "WEB_VIEWER" => Some(Permission::WebViewer),
                "READ_ONLY" => Some(Permission::ReadOnly),
                "READ_AND_WRITE" => Some(Permission::ReadAndWrite),
                "ADMIN" => Some(Permission::Admin),
                _ => None,
            });
        let optional = |key: &str| {
            row.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        let nested = |key: &str, field: &str| {
            row.get(key)
                .and_then(|v| v.as_str().or_else(|| v.get(field)?.as_str()))
                .map(str::to_owned)
        };
        projects.push(Project {
            modified_date: optional("modified_date"),
            id: id.into(),
            name: name.into(),
            kind: "stalagmight".into(),
            permission,
            active_mutex: nested("active_mutex", "user").map(|user| ActiveMutex { user }),
            latest_commit: nested("latest_commit", "id").map(|id| Commit { id }),
            exclude_geojson: row
                .get("exclude_geojson")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            description: optional("description"),
            country: optional("country"),
            visibility: optional("visibility").and_then(|v| match v.as_str() {
                "PRIVATE" => Some(Visibility::Private),
                "PUBLIC" => Some(Visibility::Public),
                _ => None,
            }),
            latitude: row.get("latitude").and_then(serde_json::Value::as_f64),
            longitude: row.get("longitude").and_then(serde_json::Value::as_f64),
        });
    }
    Ok(projects)
}

pub fn get_project(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
) -> Result<Project, SpeleoDbError> {
    get_project_inner(base, token, insecure, id, READ_TIMEOUT, true)
}
pub fn get_project_probe(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
) -> Result<Project, SpeleoDbError> {
    get_project_inner(base, token, insecure, id, Duration::from_secs(5), false)
}
/// [`get_project_probe`] with a caller-chosen total budget, for a probe that has
/// to fit inside a larger deadline (the lifecycle release's holder check runs
/// inside the app-exit wait).
pub fn get_project_probe_within(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
    timeout: Duration,
) -> Result<Project, SpeleoDbError> {
    get_project_inner(base, token, insecure, id, timeout, false)
}
fn get_project_inner(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
    timeout: Duration,
    retry: bool,
) -> Result<Project, SpeleoDbError> {
    let c = client(base, Some(token), insecure, timeout)?;
    c.call(Endpoint::GetProject, retry, || async {
        project(sdk_reply(c.http.v2_project_retrieve(id, None).await)?.value)
    })
}
pub fn get_revisions(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
) -> Result<RevisionHistory, SpeleoDbError> {
    let c = client(base, Some(token), insecure, READ_TIMEOUT)?;
    c.call(Endpoint::GetRevisions, true, || async {
        let reply = sdk_reply(c.http.v2_projects_revisions_retrieve(id, None).await)?;
        Ok(RevisionHistory {
            commits: reply
                .value
                .commits
                .into_iter()
                .enumerate()
                .map(|(i, v)| RevisionCommit {
                    authored_date: match reply.dates.get(i) {
                        Some(date) => date.clone(),
                        None => Some(commit_dates::instant(v.authored_date)),
                    },
                    id: v.id,
                    parent_ids: v.parent_ids,
                    author_email: v.author_email,
                    message: Some(v.message),
                })
                .collect(),
        })
    })
}
pub fn acquire_lock(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
) -> Result<Project, SpeleoDbError> {
    let c = client(base, Some(token), insecure, READ_TIMEOUT)?;
    c.call(Endpoint::Acquire, false, || async {
        project(
            sdk_reply(
                c.http
                    .v2_projects_acquire_create(id, None, None::<&str>)
                    .await,
            )?
            .value,
        )
    })
}
pub fn release_lock(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
) -> Result<Project, SpeleoDbError> {
    let c = client(base, Some(token), insecure, READ_TIMEOUT)?;
    c.call(Endpoint::Release, false, || async {
        project(
            sdk_reply(
                c.http
                    .v2_projects_release_create_with_json(id, None, None::<&str>, None)
                    .await,
            )?
            .value,
        )
    })
}
pub fn create_project_with(
    base: &str,
    token: &str,
    insecure: bool,
    payload: &str,
) -> Result<Project, SpeleoDbError> {
    let request: types::V2ProjectsCreateWithJsonRequest =
        serde_json::from_str(payload).map_err(|e| SpeleoDbError::Parse(e.to_string()))?;
    let c = client(base, Some(token), insecure, READ_TIMEOUT)?;
    c.call(Endpoint::CreateProject, false, || async {
        project(
            sdk_reply(
                c.http
                    .v2_projects_create_with_json(None, None::<&str>, request.clone())
                    .await,
            )?
            .value,
        )
    })
}
pub fn patch_project(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
    payload: &str,
) -> Result<Project, SpeleoDbError> {
    let request: types::V2ProjectsPartialUpdateWithJsonRequest =
        serde_json::from_str(payload).map_err(|e| SpeleoDbError::Parse(e.to_string()))?;
    let c = client(base, Some(token), insecure, READ_TIMEOUT)?;
    c.call(Endpoint::PatchProject, false, || async {
        project(
            sdk_reply(
                c.http
                    .v2_projects_partial_update_with_json(
                        id,
                        None,
                        None::<&str>,
                        Some(request.clone()),
                    )
                    .await,
            )?
            .value,
        )
    })
}
pub fn upload_stalx(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
    path: &Path,
    message: &str,
) -> Result<Project, SpeleoDbError> {
    // Every upload goes through here, so no caller can send a description the
    // server would commit and then fail to store (sdk#2). Empty is refused too.
    super::commit_message::check_commit_message(message)
        .map_err(|e| SpeleoDbError::Parse(e.to_string()))?;
    // sdk#3: the server reads the whole body before it refuses an oversized file,
    // so refuse it here, before reading it or opening a connection. The push path
    // shrinks the snapshot first (`core::state::speleodb_upload_limit`); this is the
    // backstop that keeps every caller from sending a body that cannot land.
    let size = std::fs::metadata(path).map_err(io_error)?.len();
    let limit = settings().upload_limit;
    if size > limit {
        return Err(SpeleoDbError::UploadTooLarge { size, limit });
    }
    let content = std::fs::read(path).map_err(io_error)?;
    let c = client(base, Some(token), insecure, upload_timeout(content.len()))?;
    c.call(Endpoint::Upload, false, || async {
        let request = types::ProjectUploadRequestRequest {
            message: message.into(),
            artifact: vec![types::MultipartFile {
                content: content.clone().into(),
                file_name: Some("project.stalx".into()),
                content_type: Some("application/vnd.sqlite3".into()),
            }],
        };
        let response = sdk_reply(
            c.http
                .v2_projects_upload("stalagmight", id, None, None::<&str>, request)
                .await,
        )?
        .value;
        let mut value = project(response.project)?;
        value.latest_commit = if response.hexsha.is_empty() {
            None
        } else {
            Some(Commit {
                id: response.hexsha,
            })
        };
        Ok(value)
    })
}
pub fn download_stalx(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
) -> Result<NativeFile, SpeleoDbError> {
    client(base, Some(token), insecure, Duration::ZERO)?.download(id, None)
}
pub fn download_stalx_at_hash(
    base: &str,
    token: &str,
    insecure: bool,
    id: &str,
    sha: &str,
) -> Result<NativeFile, SpeleoDbError> {
    client(base, Some(token), insecure, Duration::ZERO)?.download(id, Some(sha))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
thread_local! {
    static TEST_SETTINGS: std::cell::Cell<Option<Settings>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
struct TestSettings(Option<Settings>);
#[cfg(test)]
impl TestSettings {
    fn install(settings: Settings) -> Self {
        Self(TEST_SETTINGS.replace(Some(settings)))
    }
}
#[cfg(test)]
impl Drop for TestSettings {
    fn drop(&mut self) {
        TEST_SETTINGS.set(self.0);
    }
}
