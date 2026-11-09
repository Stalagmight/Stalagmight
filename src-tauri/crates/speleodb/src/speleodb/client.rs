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

const READ_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn upload_timeout(bytes: usize) -> Duration {
    TRANSFER_TIMEOUT + Duration::from_secs((bytes as u64).div_ceil(16 * 1024))
}
const JSON_LIMIT: usize = 10 * 1024 * 1024;
pub(crate) const NATIVE_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Settings {
    timeout: Option<Duration>,
    retries: u32,
    native_limit: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout: None,
            retries: 2,
            native_limit: NATIVE_LIMIT,
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
                let outcome = match &result {
                    Ok(_) | Err(SpeleoDbError::Parse(_)) => Outcome::Ok,
                    Err(SpeleoDbError::Status { status, .. }) => Outcome::from_status(*status),
                    Err(_) => Outcome::NetworkError,
                };
                super::metrics::record(endpoint, outcome, started.elapsed());
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
            let file = NativeFile::new()?;
            let mut output = tokio::fs::File::create(file.path())
                .await
                .map_err(io_error)?;
            let stream: std::pin::Pin<
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
            let mut stream = stream;
            let mut received = 0_u64;
            while let Some(chunk) = stream.next().await {
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

fn unreachable_result<T>() -> Result<T, SpeleoDbError> {
    Err(SpeleoDbError::Io(
        "invalid SpeleoDB retry configuration".into(),
    ))
}
fn io_error(error: std::io::Error) -> SpeleoDbError {
    SpeleoDbError::Io(error.to_string())
}
fn network_error(error: reqwest::Error) -> SpeleoDbError {
    use std::error::Error;
    let mut cause = if error.is_timeout() {
        TransportCause::Timeout
    } else {
        TransportCause::Unknown
    };
    let mut source = error.source();
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            match io.kind() {
                std::io::ErrorKind::TimedOut => cause = TransportCause::Timeout,
                std::io::ErrorKind::ConnectionRefused
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
fn api_error<E: std::fmt::Debug>(error: ApiOpError<E>) -> SpeleoDbError {
    match error {
        ApiOpError::Api(error) if (200..300).contains(&error.status) => {
            SpeleoDbError::Parse("response does not match the SpeleoDB SDK contract".into())
        }
        ApiOpError::Api(error) => SpeleoDbError::Status {
            status: error.status,
            message: parse_error_message(&error.raw_body)
                .unwrap_or_else(|| format!("HTTP {}", error.status)),
        },
        ApiOpError::Transport(HttpError::Network(error)) => network_error(error),
        ApiOpError::Transport(HttpError::Middleware(error)) => SpeleoDbError::Transport {
            cause: if error.is_timeout() {
                TransportCause::Timeout
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
fn parse_error_message(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    for key in ["error", "detail"] {
        if let Some(message) = value.get(key).and_then(serde_json::Value::as_str) {
            return Some(message.into());
        }
    }
    let errors = value.get("errors")?.as_object()?;
    let first = |v: &serde_json::Value| {
        v.as_str()
            .or_else(|| v.as_array()?.first()?.as_str())
            .map(str::to_owned)
    };
    errors
        .get("non_field_errors")
        .and_then(first)
        .or_else(|| errors.values().find_map(first))
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
    let settings = Settings::default();
    #[cfg(test)]
    let settings = TEST_SETTINGS.with(|value| value.get()).unwrap_or(settings);
    SdkClient::new(base, token, insecure, timeout, settings)
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
                Err(SpeleoDbError::Parse(
                    "response does not match the SpeleoDB SDK contract".into(),
                ))
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
        project(
            c.http
                .v2_project_retrieve(id, None)
                .await
                .map_err(api_error)?,
        )
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
        let value = c
            .http
            .v2_projects_revisions_retrieve(id, None)
            .await
            .map_err(api_error)?;
        Ok(RevisionHistory {
            commits: value
                .commits
                .into_iter()
                .map(|v| RevisionCommit {
                    id: v.id,
                    parent_ids: v.parent_ids,
                    author_email: v.author_email,
                    authored_date: Some(v.authored_date),
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
            c.http
                .v2_projects_acquire_create(id, None, None::<&str>)
                .await
                .map_err(api_error)?,
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
            c.http
                .v2_projects_release_create_with_json(id, None, None::<&str>, None)
                .await
                .map_err(api_error)?,
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
            c.http
                .v2_projects_create_with_json(None, None::<&str>, request.clone())
                .await
                .map_err(api_error)?,
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
            c.http
                .v2_projects_partial_update_with_json(id, None, None::<&str>, Some(request.clone()))
                .await
                .map_err(api_error)?,
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
    if message.trim().is_empty() {
        return Err(SpeleoDbError::Parse(
            "commit message must not be empty".into(),
        ));
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
        let response = c
            .http
            .v2_projects_upload("stalagmight", id, None, None::<&str>, request)
            .await
            .map_err(api_error)?;
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
