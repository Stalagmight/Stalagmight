//! Contract tests for only the operations Stalagmight calls.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test setup and assertions"
)]
use super::*;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const ID: &str = "11111111-1111-1111-1111-111111111111";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn project_json() -> Value {
    json!({"id": ID, "name":"Synthetic cave", "description":"SDK test", "country":"US",
        "type":"STALAGMIGHT", "visibility":"PRIVATE", "permission":"ADMIN",
        "color":"#123456", "commit_count":0, "created_by":"test@example.com",
        "creation_date":"2026-01-01T00:00:00Z", "modified_date":"2026-01-01T00:00:00Z",
        "exclude_geojson":false, "fork_from":null, "is_active":true,
        "active_mutex":null, "latest_commit":null})
}
fn user_json() -> Value {
    json!({"email":"test@example.com", "name":"Test", "country":"US",
        "email_on_projects_updates":false, "email_on_speleodb_updates":false})
}

struct Server {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn replies(replies: Vec<(u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        listener.set_nonblocking(true).expect("nonblocking");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            for (status, body) in replies {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(_) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => return,
                    }
                };
                stream
                    .set_nonblocking(false)
                    .expect("blocking accepted stream");
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .expect("timeout");
                let mut request = Vec::new();
                let mut buf = [0_u8; 4096];
                loop {
                    let n = stream.read(&mut buf).expect("request bytes");
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|h| {
                                h.strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                captured
                    .lock()
                    .expect("requests")
                    .push(String::from_utf8_lossy(&request).into_owned());
                let content_type = if body.starts_with(b"SQLite") || body.first() == Some(&0) {
                    "application/octet-stream"
                } else {
                    "application/json"
                };
                let header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n",body.len());
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Self {
            base,
            requests,
            worker: Some(worker),
        }
    }
    fn json(value: Value) -> Self {
        Self::replies(vec![(200, value.to_string().into_bytes())])
    }
    fn finish(mut self) -> Vec<String> {
        self.worker.take().expect("worker").join().expect("server");
        self.requests.lock().expect("requests").clone()
    }
}

#[test]
fn both_authentication_paths_use_the_sdk_contract() {
    let s = Server::json(json!({"token":"test-token","user":"test@example.com"}));
    assert_eq!(
        login(&s.base, false, "test@example.com", "test-password").unwrap(),
        "test-token"
    );
    let requests = s.finish();
    assert!(requests[0].starts_with("POST /api/v2/user/auth-token/ "));
    let body: Value = serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"email":"test@example.com","password":"test-password"})
    );
    let s = Server::json(user_json());
    assert_eq!(
        fetch_user_email(&s.base, "test-token", false).unwrap(),
        "test@example.com"
    );
    let r = s.finish();
    assert!(r[0].starts_with("GET /api/v2/user/ "));
    assert!(r[0]
        .to_lowercase()
        .contains("authorization: bearer test-token"));
}

#[test]
fn project_operations_preserve_permissions_locks_and_patch_nulls() {
    let mut projects: Vec<Value> = [
        "ARIANE",
        "COMPASS",
        "STICKMAPS",
        "THERION",
        "WALLS",
        "OTHER",
    ]
    .into_iter()
    .map(|kind| {
        let mut p = project_json();
        p["type"] = json!(kind);
        p
    })
    .collect();
    projects.insert(2, project_json());
    let s = Server::json(json!(projects));
    let listed = list_projects(&s.base, "t", false).unwrap();
    assert_eq!(listed.len(), 1);
    let summary = super::super::models::SpeleoProjectSummary::from_project(listed[0].clone());
    assert_eq!(
        summary.modified_date.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
    assert_eq!(listed[0].kind, "stalagmight");
    assert_eq!(listed[0].permission, Some(Permission::Admin));
    assert!(s.finish()[0].starts_with("GET /api/v2/projects/ "));
    let mut p = project_json();
    p["permission"] = json!("READ_ONLY");
    p["active_mutex"] = json!("other@example.com");
    for probe in [false, true] {
        let s = Server::json(p.clone());
        let result = if probe {
            get_project_probe(&s.base, "t", false, ID)
        } else {
            get_project(&s.base, "t", false, ID)
        }
        .unwrap();
        assert_eq!(
            result.modified_date.as_deref(),
            Some("2026-01-01T00:00:00+00:00")
        );
        assert_eq!(result.permission, Some(Permission::ReadOnly));
        assert_eq!(result.active_mutex.unwrap().user, "other@example.com");
        assert!(s.finish()[0].starts_with(&format!("GET /api/v2/projects/{ID}/ ")));
    }
    let s = Server::replies(vec![(201, project_json().to_string().into_bytes())]);
    create_project_with(
        &s.base,
        "t",
        false,
        &json!({"name":"Synthetic","description":"Test","country":"US","type":"STALAGMIGHT"})
            .to_string(),
    )
    .unwrap();
    assert!(s.finish()[0].starts_with("POST /api/v2/projects/ "));
    let s = Server::json(project_json());
    patch_project(
        &s.base,
        "t",
        false,
        ID,
        r#"{"name":"Renamed","latitude":null,"longitude":null}"#,
    )
    .unwrap();
    let r = s.finish();
    let body: Value = serde_json::from_str(r[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"name":"Renamed","latitude":null,"longitude":null})
    );
    for (acquire, route) in [(true, "acquire"), (false, "release")] {
        let s = Server::json(project_json());
        if acquire {
            acquire_lock(&s.base, "t", false, ID)
        } else {
            release_lock(&s.base, "t", false, ID)
        }
        .unwrap();
        assert!(s.finish()[0].starts_with(&format!("POST /api/v2/projects/{ID}/{route}/ ")));
    }
}

#[test]
fn revisions_and_native_file_routes_are_exact() {
    let commit = json!({"id":SHA,"parent_ids":[],"author_email":"test@example.com","author_name":"Test","authored_date":"2026-01-01T00:00:00Z","dt_since":"now","message":"Test","formats":[],"tree":[],"url":null});
    let s = Server::json(json!({"project":project_json(),"commits":[commit]}));
    assert_eq!(
        get_revisions(&s.base, "t", false, ID).unwrap().commits[0].id,
        SHA
    );
    assert!(s.finish()[0].starts_with(&format!("GET /api/v2/projects/{ID}/revisions/ ")));
    let bytes = b"SQLite format 3\0synthetic bytes";
    for revision in [None, Some(SHA)] {
        let s = Server::replies(vec![(200, bytes.to_vec())]);
        let native = if let Some(sha) = revision {
            download_stalx_at_hash(&s.base, "t", false, ID, sha)
        } else {
            download_stalx(&s.base, "t", false, ID)
        }
        .unwrap();
        assert_eq!(std::fs::read(native.path()).unwrap(), bytes);
        let path = native.path().to_owned();
        drop(native);
        assert!(!path.exists());
        let suffix = revision.map(|sha| format!("/{sha}")).unwrap_or_default();
        assert!(s.finish()[0].starts_with(&format!(
            "GET /api/v2/projects/{ID}/download/stalagmight{suffix}/ "
        )));
    }
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), bytes).unwrap();
    let s = Server::json(
        json!({"project":project_json(),"hexsha":SHA,"files":["project.stalx"],"message":"Saved","browser_url":null}),
    );
    assert_eq!(
        upload_stalx(&s.base, "t", false, ID, f.path(), "My commit")
            .unwrap()
            .latest_commit
            .unwrap()
            .id,
        SHA
    );
    let r = s.finish();
    assert!(r[0].starts_with(&format!("PUT /api/v2/projects/{ID}/upload/stalagmight/ ")));
    assert_eq!(r[0].matches("filename=").count(), 1);
    assert!(r[0].contains("filename=\"project.stalx\""));
    assert!(r[0].contains("My commit"));
    assert!(r[0].contains("SQLite format 3\0synthetic bytes"));
}

#[test]
fn upload_refuses_a_description_the_server_cannot_store_before_sending_anything() {
    // sdk#2: SpeleoDB would commit it to git and then fail to store it, breaking
    // every later upload. Refused before the file is read or a request is made:
    // the path does not exist and nothing listens on the endpoint.
    let long = format!("{}\n{}", "a".repeat(500), "b".repeat(499));
    let missing = Path::new("/nonexistent/synthetic.stalx");
    let err = upload_stalx("http://127.0.0.1:9", "t", true, ID, missing, &long).unwrap_err();
    assert!(
        matches!(&err, SpeleoDbError::Parse(m) if m.contains("too long") && m.contains("1001")),
        "{err:?}"
    );
    let err = upload_stalx("http://127.0.0.1:9", "t", true, ID, missing, " \n ").unwrap_err();
    assert!(
        matches!(&err, SpeleoDbError::Parse(m) if m.contains("empty")),
        "{err:?}"
    );
}

#[test]
fn status_errors_and_malformed_success_remain_distinct() {
    for status in [304, 400, 401, 403, 404, 409, 429, 500] {
        let s = Server::replies(vec![(status, br#"{"detail":"Test failure"}"#.to_vec())]);
        let err = release_lock(&s.base, "t", false, ID).unwrap_err();
        assert!(matches!(err,SpeleoDbError::Status {status:s,..} if s==status));
        assert_eq!(s.finish().len(), 1, "mutations must never retry");
    }
    let s = Server::json(json!({"unexpected":"sensitive response text"}));
    let err = get_project(&s.base, "t", false, ID).unwrap_err();
    assert!(matches!(err, SpeleoDbError::Parse(_)));
    assert!(!err.to_string().contains("sensitive"));
    assert_eq!(s.finish().len(), 1);
    for status in [400, 401] {
        let s = Server::replies(vec![(status, b"{}".to_vec())]);
        assert!(login(&s.base, false, "bad@example.com", "bad").is_err());
        assert_eq!(s.finish().len(), 1);
    }
}

/// An error reply's message is the server's text, and the reply may be as long
/// as the JSON limit: uncapped, a 10 MiB `detail` reached the log, the error
/// dialogs, Home's removal warning and the purge report. It is cut at the source,
/// to the same bound as a contract mismatch's detail, whichever field it came from.
#[test]
fn an_error_replys_message_is_bounded_whatever_field_it_comes_from() {
    let long = "\u{e9}".repeat(CONTRACT_DETAIL_LIMIT * 40);
    let cut = format!("{}...", "\u{e9}".repeat(CONTRACT_DETAIL_LIMIT));
    for body in [
        json!({"detail": long}),
        json!({"error": long}),
        json!({"errors": {"non_field_errors": [long]}}),
        json!({"errors": {"name": long}}),
    ] {
        let raw = serde_json::to_vec(&body).expect("json");
        assert_eq!(
            parse_error_message(&raw).as_deref(),
            Some(cut.as_str()),
            "{body}"
        );
    }
    // A short message is kept as it is.
    assert_eq!(
        parse_error_message(br#"{"detail":"Test failure"}"#).as_deref(),
        Some("Test failure")
    );

    // End to end: the status error a removal's lock release reports.
    let s = Server::replies(vec![(
        409,
        serde_json::to_vec(&json!({ "detail": long })).expect("json"),
    )]);
    let err = release_lock(&s.base, "t", false, ID).unwrap_err();
    s.finish();
    assert!(
        matches!(&err, SpeleoDbError::Status { status: 409, message } if *message == cut),
        "{err:?}"
    );
    assert!(err.to_string().chars().count() < CONTRACT_DETAIL_LIMIT + 100);
}

#[test]
fn reads_retry_transient_failures_and_native_streams_enforce_a_limit() {
    let s = Server::replies(vec![
        (503, b"{}".to_vec()),
        (200, user_json().to_string().into_bytes()),
    ]);
    assert!(fetch_user_email(&s.base, "t", false).is_ok());
    assert_eq!(s.finish().len(), 2);
    let s = Server::replies(vec![(200, vec![0; 1024])]);
    let c = SdkClient::new(
        &s.base,
        Some("t"),
        false,
        READ_TIMEOUT,
        Settings {
            native_limit: 10,
            ..Settings::default()
        },
    )
    .unwrap();
    assert!(matches!(c.download(ID, None), Err(SpeleoDbError::Parse(_))));
    assert_eq!(s.finish().len(), 1);
}

const LIVE_INPUTS: [&str; 4] = [
    "SPELEODB_INSTANCE_URL",
    "SPELEODB_OAUTH_TOKEN",
    "SPELEODB_EMAIL",
    "SPELEODB_PASSWORD",
];

fn live_value(
    name: &str,
    environment: &impl Fn(&str) -> Option<String>,
    file: &std::collections::HashMap<String, String>,
) -> Option<String> {
    environment(name).or_else(|| file.get(name).cloned())
}
fn live_credentials(
    value: &impl Fn(&str) -> Option<String>,
) -> Result<[String; 4], Vec<&'static str>> {
    let inputs = LIVE_INPUTS.map(|name| {
        value(name)
            .filter(|v| !v.trim().is_empty() && !v.to_ascii_lowercase().contains("placeholder"))
    });
    let missing: Vec<_> = LIVE_INPUTS
        .iter()
        .zip(&inputs)
        .filter_map(|(name, input)| input.is_none().then_some(*name))
        .collect();
    if missing.is_empty() {
        Ok(inputs.map(|v| v.expect("validated input")))
    } else {
        Err(missing)
    }
}

fn live_settings(value: &impl Fn(&str) -> Option<String>) -> Settings {
    Settings {
        timeout: Some(Duration::from_millis(
            value("SPELEODB_API_TIMEOUT_MS")
                .unwrap_or_else(|| "120000".into())
                .parse::<u64>()
                .expect("timeout milliseconds"),
        )),
        retries: value("SPELEODB_API_RETRY_COUNT")
            .unwrap_or_else(|| "3".into())
            .parse::<u32>()
            .expect("retry count"),
        ..Settings::default()
    }
}

#[test]
fn live_settings_default_to_120_seconds_and_allow_explicit_overrides() {
    let defaults = live_settings(&|_| None);
    assert_eq!(defaults.timeout, Some(Duration::from_secs(120)));
    assert_eq!(defaults.retries, 3);

    let file = std::collections::HashMap::from([
        ("SPELEODB_API_TIMEOUT_MS".into(), "180000".into()),
        ("SPELEODB_API_RETRY_COUNT".into(), "1".into()),
    ]);
    let from_file = live_settings(&|name| live_value(name, &|_| None, &file));
    assert_eq!(from_file.timeout, Some(Duration::from_secs(180)));
    assert_eq!(from_file.retries, 1);

    let overridden = live_settings(&|name| {
        live_value(
            name,
            &|key| (key == "SPELEODB_API_TIMEOUT_MS").then(|| "20000".into()),
            &file,
        )
    });
    assert_eq!(overridden.timeout, Some(Duration::from_secs(20)));
    assert_eq!(overridden.retries, 1);
}

fn cleanup_created<T, E>(
    release: impl FnOnce() -> Result<T, E>,
    delete: impl FnOnce() -> Result<T, E>,
) -> (Result<T, E>, Result<T, E>) {
    let released = release();
    let deleted = delete();
    (released, deleted)
}

struct TestProjectRun<T, E> {
    result: std::thread::Result<Result<T, E>>,
    released: Result<(), E>,
    deleted: Result<(), E>,
}
fn run_test_project<T, E>(
    operation: impl FnOnce() -> Result<T, E> + std::panic::UnwindSafe,
    release: impl FnOnce() -> Result<(), E>,
    delete: impl FnOnce() -> Result<(), E>,
) -> TestProjectRun<T, E> {
    let result = std::panic::catch_unwind(operation);
    let (released, deleted) = cleanup_created(release, delete);
    TestProjectRun {
        result,
        released,
        deleted,
    }
}

#[test]
#[expect(
    clippy::panic,
    reason = "controlled panic injection verifies live-test cleanup"
)]
fn test_project_cleanup_runs_after_success_api_error_and_assertion_panic() {
    for mode in ["success", "error", "panic"] {
        let calls = std::cell::RefCell::new(Vec::new());
        let TestProjectRun {
            result,
            released,
            deleted,
        } = run_test_project(
            || match mode {
                "error" => Err("original API failure"),
                "panic" => panic!("synthetic assertion failure"),
                _ => Ok(()),
            },
            || {
                calls.borrow_mut().push(("release", ID));
                Ok(())
            },
            || {
                calls.borrow_mut().push(("delete", ID));
                Ok(())
            },
        );
        assert_eq!(*calls.borrow(), vec![("release", ID), ("delete", ID)]);
        assert!(released.is_ok() && deleted.is_ok());
        match mode {
            "error" => assert_eq!(result.unwrap().unwrap_err(), "original API failure"),
            "panic" => assert!(result.is_err()),
            _ => assert!(result.unwrap().is_ok()),
        }
    }
}

#[test]
fn live_configuration_is_opt_in_and_validates_every_input_without_network() {
    let file =
        std::collections::HashMap::from([("SPELEODB_API_TEST_ENABLED".into(), "true".into())]);
    assert_eq!(
        live_value(
            "SPELEODB_API_TEST_ENABLED",
            &|_| Some("false".into()),
            &file
        )
        .as_deref(),
        Some("false")
    );
    assert_eq!(
        live_value("SPELEODB_API_TEST_ENABLED", &|_| None, &file).as_deref(),
        Some("true")
    );
    assert!(live_value("SPELEODB_API_TEST_ENABLED", &|_| None, &Default::default()).is_none());
    for name in LIVE_INPUTS {
        for invalid in [None, Some(""), Some("  "), Some("<placeholder>")] {
            assert_eq!(
                live_credentials(&|key| if key == name {
                    invalid.map(str::to_owned)
                } else {
                    Some("configured".into())
                })
                .unwrap_err(),
                vec![name]
            );
        }
    }
    assert!(live_credentials(&|_| Some("configured".into())).is_ok());
}

#[test]
fn cleanup_attempts_deletion_even_when_release_fails() {
    for failed_release in [false, true] {
        for failed_delete in [false, true] {
            let calls = std::cell::RefCell::new(Vec::new());
            let (release, delete) = cleanup_created(
                || {
                    calls.borrow_mut().push("release");
                    if failed_release {
                        Err("release")
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("delete");
                    if failed_delete {
                        Err("delete")
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(*calls.borrow(), vec!["release", "delete"]);
            assert_eq!(release.is_err(), failed_release);
            assert_eq!(delete.is_err(), failed_delete);
        }
    }
}

#[test]
#[expect(clippy::print_stderr, reason = "live-test diagnostics")]
fn live_used_api_calls_with_email_and_token() {
    // Read .env without mutating process-wide environment or making it an app config.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.env");
    let file: std::collections::HashMap<String, String> = match dotenvy::from_path_iter(path) {
        Ok(values) => values.filter_map(Result::ok).collect(),
        Err(_) => Default::default(),
    };
    let value = |name: &str| live_value(name, &|key| std::env::var(key).ok(), &file);
    if value("SPELEODB_API_TEST_ENABLED").unwrap_or_else(|| "false".into()) != "true" {
        eprintln!("live SpeleoDB API tests disabled; set SPELEODB_API_TEST_ENABLED=true");
        return;
    }
    let inputs = match live_credentials(&value) {
        Ok(inputs) => inputs,
        Err(missing) => {
            eprintln!(
                "live SpeleoDB API tests skipped: credentials incomplete: {}",
                missing.join(", ")
            );
            return;
        }
    };
    let [base, provided_token, email, password] = inputs;
    eprintln!("live SpeleoDB API tests executing EMAIL and TOKEN authentication paths");
    let _settings = TestSettings::install(live_settings(&value));
    let login_token = login(&base, false, &email, &password).expect("EMAIL authentication");
    assert!(fetch_user_email(&base, &login_token, false)
        .expect("EMAIL profile")
        .eq_ignore_ascii_case(&email));
    assert!(fetch_user_email(&base, &provided_token, false)
        .expect("TOKEN profile")
        .eq_ignore_ascii_case(&email));
    for (label, token) in [("EMAIL", login_token), ("TOKEN", provided_token)] {
        let created = create_project_with(
            &base,
            &token,
            false,
            &json!({
                "name": format!("Stalagmight SDK test {}", uuid::Uuid::new_v4()),
                "description": "Synthetic native-file API test; deleted after execution",
                "country": "US", "type": "STALAGMIGHT", "visibility": "PRIVATE",
            })
            .to_string(),
        );
        if created.is_err() {
            eprintln!("{label}: creation was not confirmed. If the server created a project but its response was lost, its unknown ID cannot safely be cleaned up automatically.");
        }
        let project = created.expect("create test-owned project");
        let id = &project.id;
        eprintln!("{label}: created test project {id}");
        let TestProjectRun {
            result,
            released,
            deleted,
        } = run_test_project(
            || {
                (|| -> Result<(), SpeleoDbError> {
                    assert!(list_projects(&base, &token, false)?
                        .iter()
                        .any(|p| p.id == *id));
                    get_project(&base, &token, false, id)?;
                    get_project_probe(&base, &token, false, id)?;
                    patch_project(
                        &base,
                        &token,
                        false,
                        id,
                        r#"{"description":"Native SQLite round trip"}"#,
                    )?;
                    acquire_lock(&base, &token, false, id)?;
                    let dir = tempfile::tempdir().unwrap();
                    let path = dir.path().join("project.stalx");
                    {
                        let mut db =
                            stalagmight_persistence::persistence::Database::open(&path).unwrap();
                        db.save_project(&crate::models::Project::new(
                            "Synthetic first revision".into(),
                            None,
                        ))
                        .unwrap();
                    }
                    let first_bytes = std::fs::read(&path).unwrap();
                    eprintln!("{label}: uploading first native file");
                    let first =
                        upload_stalx(&base, &token, false, id, &path, "First synthetic revision")?
                            .latest_commit
                            .expect("first SHA")
                            .id;
                    assert_eq!(
                        std::fs::read(download_stalx(&base, &token, false, id)?.path()).unwrap(),
                        first_bytes
                    );
                    assert!(matches!(
                        upload_stalx(&base, &token, false, id, &path, "No changes"),
                        Err(SpeleoDbError::Status { status: 304, .. })
                    ));
                    {
                        let mut db =
                            stalagmight_persistence::persistence::Database::open(&path).unwrap();
                        db.save_project(&crate::models::Project::new(
                            "Synthetic second revision".into(),
                            None,
                        ))
                        .unwrap();
                    }
                    eprintln!("{label}: uploading second native file");
                    let second =
                        upload_stalx(&base, &token, false, id, &path, "Second synthetic revision")?
                            .latest_commit
                            .expect("second SHA")
                            .id;
                    assert_ne!(first, second);
                    let history = get_revisions(&base, &token, false, id)?;
                    assert!(history.commits.iter().any(|c| c.id == first));
                    assert!(history.commits.iter().any(|c| c.id == second));
                    assert_eq!(
                        std::fs::read(
                            download_stalx_at_hash(&base, &token, false, id, &first)?.path()
                        )
                        .unwrap(),
                        first_bytes
                    );
                    Ok(())
                })()
            },
            // Only IDs returned by this invocation may be deleted, after every outcome.
            || release_lock(&base, &token, false, id).map(|_| ()),
            || {
                client(&base, Some(&token), false, READ_TIMEOUT).and_then(|c| {
                    runtime()?
                        .block_on(c.http.v2_projects_destroy(id, None, None::<&str>))
                        .map(|_| ())
                        .map_err(api_error)
                })
            },
        );
        if released.is_err() {
            eprintln!("{label}: test lock cleanup failed for {id}");
        }
        if deleted.is_err() {
            eprintln!("{label}: test project deletion failed for {id}");
        }
        result
            .expect("round-trip assertion")
            .expect("native-file round trip");
        released.expect("release test-owned lock");
        deleted.expect("delete test-owned project");
    }
    eprintln!("live SpeleoDB API tests executed successfully; both test-created projects deleted");
}

#[test]
fn profile_accepts_unset_country_but_rejects_other_invalid_profiles() {
    let mut profile = user_json();
    profile["country"] = json!("");
    let s = Server::json(profile.clone());
    assert_eq!(
        fetch_user_email(&s.base, "t", false).unwrap(),
        "test@example.com"
    );
    assert_eq!(s.finish().len(), 1);
    for country in ["invalid", ""] {
        profile["country"] = json!(country);
        profile["email"] = json!("");
        let s = Server::json(profile.clone());
        assert!(fetch_user_email(&s.base, "t", false).is_err());
        assert_eq!(s.finish().len(), 1);
    }
}

#[test]
fn list_only_projection_tolerates_unknown_enums_without_granting_permissions() {
    let mut future = project_json();
    future["permission"] = json!("FUTURE_PERMISSION");
    future["visibility"] = json!("FUTURE_VISIBILITY");
    future["country"] = json!("FUTURE_COUNTRY");
    let mut unknown_type = project_json();
    unknown_type["type"] = json!("FUTURE_TYPE");
    let s = Server::json(json!([project_json(), future.clone(), unknown_type]));
    let rows = list_projects(&s.base, "t", false).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].permission, Some(Permission::Admin));
    assert_eq!(rows[1].permission, None);
    s.finish();
    for invalid in [
        json!({"not":"an array"}),
        json!([{"type":"UNKNOWN", "name":"Missing ID"}]),
        json!([{"id":"invalid", "name":"Bad ID", "type":"STALAGMIGHT"}]),
    ] {
        let s = Server::json(invalid);
        assert!(list_projects(&s.base, "t", false).is_err());
        s.finish();
    }
    let s = Server::json(future);
    assert!(
        get_project(&s.base, "t", false, ID).is_err(),
        "detail and mutations stay strict"
    );
    s.finish();
}

#[test]
fn upload_budget_scales_for_slow_buffered_transfers() {
    assert_eq!(upload_timeout(0), Duration::from_secs(120));
    assert_eq!(upload_timeout(1), Duration::from_secs(121));
    assert_eq!(upload_timeout(16 * 1024), Duration::from_secs(121));
    assert_eq!(upload_timeout(16 * 1024 + 1), Duration::from_secs(122));
    assert_eq!(upload_timeout(64 * 1024 * 1024), Duration::from_secs(4216));
}

#[test]
fn downloads_allow_healthy_progress_beyond_the_idle_budget_but_reject_stalls() {
    for stall in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            let received = stream.read(&mut request).unwrap();
            assert!(received > 0, "the fake server must receive an HTTP request");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 12\r\nConnection: close\r\n\r\n")
                .unwrap();
            for _ in 0..12 {
                std::thread::sleep(Duration::from_millis(if stall { 800 } else { 50 }));
                if stream.write_all(b"x").is_err() {
                    break;
                }
            }
        });
        let c = SdkClient::new(
            &base,
            Some("synthetic"),
            false,
            Duration::ZERO,
            Settings {
                timeout: Some(Duration::from_millis(400)),
                retries: 0,
                ..Settings::default()
            },
        )
        .unwrap();
        let result = c.download(ID, None);
        if stall {
            assert!(matches!(
                result,
                Err(SpeleoDbError::Transport {
                    cause: TransportCause::Timeout,
                    ..
                })
            ));
        } else {
            assert_eq!(
                std::fs::read(result.unwrap().path()).unwrap(),
                b"xxxxxxxxxxxx"
            );
        }
        worker.join().unwrap();
    }
}

#[test]
fn download_deadline_scales_with_the_size_and_is_bounded_by_the_native_limit() {
    let grace = TRANSFER_TIMEOUT;
    let deadline = |bytes| download_deadline(grace, DOWNLOAD_MIN_RATE, DOWNLOAD_CEILING, bytes);
    assert_eq!(deadline(0), grace);
    assert_eq!(deadline(4 * 1024), grace + Duration::from_secs(1));
    assert_eq!(
        deadline(20 * 1024 * 1024),
        grace + Duration::from_secs(5120)
    );
    // Generous: a quarter of the upload budget's rate, so never shorter than it.
    assert!(deadline(20 * 1024 * 1024) >= upload_timeout(20 * 1024 * 1024));
    // Every file SpeleoDB can store still fits at the minimum rate...
    assert_eq!(
        deadline(SERVER_UPLOAD_LIMIT_BYTES),
        grace + Duration::from_secs(SERVER_UPLOAD_LIMIT_BYTES / DOWNLOAD_MIN_RATE)
    );
    assert!(deadline(SERVER_UPLOAD_LIMIT_BYTES) <= DOWNLOAD_CEILING);
    // ...while the most a download may receive (the native limit, about 36 h at
    // the minimum rate) is cut at the ceiling, Content-Length or not.
    assert_eq!(deadline(NATIVE_LIMIT), DOWNLOAD_CEILING);
    assert!(
        DOWNLOAD_CEILING < Duration::from_hours(2),
        "{DOWNLOAD_CEILING:?}"
    );
}

/// A server paying out just ABOVE the minimum rate keeps moving the per-byte
/// deadline ahead of the clock, so before the ceiling it held the download (and
/// the app's exit waiting for it) for as long as the native size limit allowed:
/// some 36 hours. The absolute ceiling stops it, as a timeout.
#[test]
fn a_download_paying_out_just_above_the_minimum_rate_is_stopped_by_the_ceiling() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0, "an HTTP request");
        // 16 KiB at 1,280 bytes a second: 25% above the 1,024 B/s minimum, so the
        // whole body would take about 13 s and arrive intact.
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 16384\r\nConnection: close\r\n\r\n")
            .unwrap();
        for _ in 0..256 {
            std::thread::sleep(Duration::from_millis(50));
            if stream.write_all(&[b'x'; 64]).is_err() {
                break;
            }
        }
    });
    let c = SdkClient::new(
        &base,
        Some("synthetic"),
        false,
        Duration::ZERO,
        Settings {
            retries: 0,
            download_grace: Duration::from_millis(300),
            download_min_rate: 1024,
            download_ceiling: Duration::from_millis(1500),
            ..Settings::default()
        },
    )
    .unwrap();
    let started = Instant::now();
    let result = c.download(ID, None);
    let took = started.elapsed();
    assert!(
        matches!(
            result,
            Err(SpeleoDbError::Transport {
                cause: TransportCause::Timeout,
                ..
            })
        ),
        "{:?}",
        result.err()
    );
    assert!(
        took < Duration::from_secs(5),
        "stopped by the ceiling, not by the end of the body: {took:?}"
    );
    worker.join().unwrap();
}

/// A server that trickles a byte well inside the read timeout used to hold the
/// download (and the app's exit waiting for it) for ever: only a read timeout
/// applied. The total deadline stops it, as a timeout.
#[test]
fn a_download_trickling_below_the_minimum_rate_is_stopped_by_its_total_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0, "an HTTP request");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 1048576\r\nConnection: close\r\n\r\n")
            .unwrap();
        // 20 bytes a second, each well inside the read timeout.
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
            if stream.write_all(b"x").is_err() {
                break;
            }
        }
    });
    let c = SdkClient::new(
        &base,
        Some("synthetic"),
        false,
        Duration::ZERO,
        Settings {
            retries: 0,
            download_grace: Duration::from_millis(300),
            download_min_rate: 1024,
            ..Settings::default()
        },
    )
    .unwrap();
    let started = Instant::now();
    let result = c.download(ID, None);
    let took = started.elapsed();
    assert!(
        matches!(
            result,
            Err(SpeleoDbError::Transport {
                cause: TransportCause::Timeout,
                ..
            })
        ),
        "{:?}",
        result.err()
    );
    assert!(
        took < Duration::from_secs(5),
        "stopped by the total deadline, not the 120 s read timeout: {took:?}"
    );
    worker.join().unwrap();
}

#[test]
fn dns_failure_keeps_its_typed_cause_for_offline_open() {
    struct OfflineDns;
    impl reqwest::dns::Resolve for OfflineDns {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(async { Err(std::io::Error::other("synthetic DNS failure").into()) })
        }
    }
    let error = runtime().expect("runtime").block_on(async {
        reqwest::Client::builder()
            .no_proxy()
            .dns_resolver(Arc::new(OfflineDns))
            .build()
            .expect("client")
            .get("http://synthetic.invalid/")
            .send()
            .await
            .expect_err("synthetic resolver always fails")
    });
    assert!(matches!(
        network_error(error),
        SpeleoDbError::Transport {
            cause: TransportCause::Dns,
            ..
        }
    ));
}

#[test]
fn a_contract_mismatch_keeps_the_sdk_decoder_detail_without_response_text() {
    // sdk#8: a 2xx the SDK cannot decode used to collapse to one fixed string, so
    // a new server enum value was invisible in logs and diagnostics. The decoder
    // detail now survives - naming the field / value that failed - while any
    // quoted response string is redacted.
    let mut unknown_country = project_json();
    unknown_country["country"] = json!("ZZ");
    let s = Server::json(unknown_country);
    let err = acquire_lock(&s.base, "t", false, ID).unwrap_err();
    assert_eq!(s.finish().len(), 1);
    assert!(
        matches!(&err, SpeleoDbError::Parse(_)),
        "a contract mismatch is a Parse error, got {err:?}"
    );
    let detail = err.to_string();
    assert!(
        detail.contains("response does not match the SpeleoDB SDK contract: "),
        "{detail}"
    );
    // The kind of mismatch survives; the server's value does not.
    assert!(detail.contains("unknown variant `...`"), "{detail}");
    assert!(!detail.contains("ZZ"), "{detail}");
    assert!(
        detail.chars().count() <= 340,
        "the accepted-values list is capped: {detail}"
    );

    let mut wrong_type = project_json();
    wrong_type["commit_count"] = json!("sensitive response text");
    let s = Server::json(wrong_type);
    let err = acquire_lock(&s.base, "t", false, ID).unwrap_err();
    s.finish();
    assert!(
        matches!(&err, SpeleoDbError::Parse(_)),
        "a contract mismatch is a Parse error, got {err:?}"
    );
    let detail = err.to_string();
    assert!(detail.contains("invalid type: string \"...\""), "{detail}");
    assert!(!detail.contains("sensitive"), "{detail}");

    // serde prints a number in backticks, not quotes: a coordinate where a string
    // belongs must not reach the log or the error either.
    let mut number_for_text = project_json();
    number_for_text["name"] = json!(20.48);
    let s = Server::json(number_for_text);
    let err = acquire_lock(&s.base, "t", false, ID).unwrap_err();
    s.finish();
    let detail = err.to_string();
    assert!(detail.contains("floating point `...`"), "{detail}");
    assert!(!detail.contains("20.48"), "{detail}");
}

#[test]
fn contract_mismatch_detail_is_redacted_and_bounded() {
    assert_eq!(
        contract_mismatch(None),
        "response does not match the SpeleoDB SDK contract"
    );
    assert_eq!(
        contract_mismatch(Some(
            r#"failed to deserialize 2xx response body: invalid value: string "a \"quoted\" name", expected x at line 1 column 9"#
                .into()
        )),
        r#"response does not match the SpeleoDB SDK contract: invalid value: string "...", expected x at line 1 column 9"#
    );
    // serde's backtick spans carry server values too: numbers, unknown variants
    // and unknown field names. Backslashes in them are verbatim, not escapes.
    for (message, expected) in [
        (
            "failed to deserialize 2xx response body: invalid type: floating point `20.48`, expected a string at line 1 column 42",
            "invalid type: floating point `...`, expected a string at line 1 column 42",
        ),
        (
            "invalid value: integer `-87`, expected u32",
            "invalid value: integer `...`, expected u32",
        ),
        (
            "unknown variant `Synthetic\\`, expected `PRIVATE` or `PUBLIC`",
            "unknown variant `...`, expected `...` or `...`",
        ),
        (
            "unknown field `coords`, then a string \"Cave \\\"A\\\"\"",
            "unknown field `...`, then a string \"...\"",
        ),
    ] {
        assert_eq!(
            contract_mismatch(Some(message.into())),
            format!("response does not match the SpeleoDB SDK contract: {expected}"),
            "{message}"
        );
    }
    // serde prints a backticked value verbatim, so the value can hold a backtick
    // or the text ", expected" itself. Closing its span at the first backtick
    // leaked everything after it.
    for (message, expected) in [
        (
            "unknown variant `a`LEAK`, expected `PRIVATE` or `PUBLIC`",
            "unknown variant `...`, expected `...` or `...`",
        ),
        (
            "unknown variant `x`, expected LEAK `y`, expected `PRIVATE` or `PUBLIC`",
            "unknown variant `...`, expected `...` or `...`",
        ),
        (
            "unknown field `a`LEAK`b`, expected one of `name`, `country` at line 1 column 7",
            "unknown field `...`, expected one of `...`, `...` at line 1 column 7",
        ),
        (
            "unknown variant `a\"LEAK`, expected `PUBLIC`",
            "unknown variant `...`, expected `...`",
        ),
        (
            "invalid type: string \"a`LEAK`, expected `b\", expected u32",
            "invalid type: string \"...\", expected u32",
        ),
        // A span that never closes may run into the rest, which is dropped.
        (
            "invalid value: string \"LEAK, expected x",
            "invalid value: string \"...\"",
        ),
        ("unknown variant `LEAK, expected x", "unknown variant `...`"),
    ] {
        let redacted = contract_mismatch(Some(message.into()));
        assert_eq!(
            redacted,
            format!("response does not match the SpeleoDB SDK contract: {expected}"),
            "{message}"
        );
        assert!(!redacted.contains("LEAK"), "{redacted}");
    }
    let long = format!(
        "unknown variant `ZZ`, expected one of {}",
        "`AA`, ".repeat(400)
    );
    let capped = contract_mismatch(Some(long));
    assert!(capped.ends_with("..."));
    assert!(capped.chars().count() < 320, "{capped}");
}

#[test]
fn a_parse_failure_is_its_own_metrics_outcome() {
    // sdk#8: Parse used to count as `ok`, so contract drift never showed in the
    // traffic metrics.
    use super::super::metrics::Outcome;
    let ok: Result<(), SpeleoDbError> = Ok(());
    assert_eq!(attempt_outcome(&ok), Outcome::Ok);
    let parse: Result<(), _> = Err(SpeleoDbError::Parse("drift".into()));
    assert_eq!(attempt_outcome(&parse), Outcome::ParseError);
    let status: Result<(), _> = Err(SpeleoDbError::Status {
        status: 404,
        message: "gone".into(),
    });
    assert_eq!(attempt_outcome(&status), Outcome::ClientError);
    let transport: Result<(), _> = Err(SpeleoDbError::Transport {
        cause: TransportCause::Tls,
        detail: "handshake".into(),
    });
    assert_eq!(attempt_outcome(&transport), Outcome::NetworkError);
}

#[test]
fn a_tls_handshake_failure_is_classified_as_tls() {
    // sdk#8: TransportCause::Tls was never produced, so a certificate or
    // handshake failure read as "could not reach the server". An https request
    // to a peer that answers in plaintext fails inside rustls.
    let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
    let base = format!("https://{}", listener.local_addr().expect("address"));
    let worker = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    // Acquire never retries, so exactly one connection is made.
    let err = acquire_lock(&base, "t", false, ID).unwrap_err();
    worker.join().expect("server");
    assert!(
        matches!(
            err,
            SpeleoDbError::Transport {
                cause: TransportCause::Tls,
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn an_upload_over_the_server_limit_is_refused_before_any_request() {
    // sdk#3: the server reads the whole body before refusing an oversized file,
    // so the client refuses it first - no connection, no body. The limit is
    // lowered for the test; the production value is the server's 24,657,920.
    assert_eq!(SERVER_UPLOAD_LIMIT_BYTES, 24_657_920);
    let _settings = TestSettings::install(Settings {
        upload_limit: 16,
        ..Settings::default()
    });
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), b"SQLite format 3\0over the limit").unwrap();
    let s = Server::replies(Vec::new());
    let err = upload_stalx(&s.base, "t", false, ID, f.path(), "Too large").unwrap_err();
    assert!(
        matches!(
            err,
            SpeleoDbError::UploadTooLarge {
                size: 30,
                limit: 16
            }
        ),
        "got {err:?}"
    );
    assert!(s.finish().is_empty(), "nothing may reach the server");

    // At the limit the upload proceeds.
    std::fs::write(f.path(), b"SQLite format 3\0").unwrap();
    let s = Server::json(
        json!({"project":project_json(),"hexsha":SHA,"files":["project.stalx"],"geojson_status":"skipped","message":"Saved","browser_url":null}),
    );
    upload_stalx(&s.base, "t", false, ID, f.path(), "Fits").unwrap();
    assert_eq!(s.finish().len(), 1);
}

#[test]
fn a_profile_contract_mismatch_also_keeps_the_decoder_detail() {
    // The profile read has its own 2xx fallback (an unset country is accepted);
    // any other mismatch there now carries the SDK detail too.
    let mut user = user_json();
    user["country"] = json!("ZZ");
    let s = Server::json(user);
    let err = fetch_user_email(&s.base, "t", false).unwrap_err();
    s.finish();
    assert!(matches!(&err, SpeleoDbError::Parse(_)), "got {err:?}");
    assert!(err.to_string().contains("unknown variant `...`"), "{err}");
    assert!(!err.to_string().contains("ZZ"), "{err}");
}

/// A revision as a SpeleoDB from before the ISO revision dates (server a679010c6)
/// sends it: New York wall time with no zone.
fn pre_iso_commit(id: &str, date: &str) -> Value {
    json!({"id":id,"parent_ids":[],"author_email":"test@example.com","author_name":"Test",
        "authored_date":date,"dt_since":"now","message":"Synthetic","formats":[],"tree":[],"url":null})
}
/// A project as a pre-ISO server sends it: `latest_commit` embeds the whole newest
/// commit, wall-time date included.
fn pre_iso_project() -> Value {
    let mut p = project_json();
    p["commit_count"] = json!(1);
    p["latest_commit"] = pre_iso_commit(SHA, "2026/11/10 03:18");
    p
}

#[test]
fn iso_revision_dates_reach_the_history_as_utc_milliseconds() {
    let mut project = project_json();
    project["latest_commit"] = pre_iso_commit(SHA, "2026-11-10T08:18:23Z");
    let s = Server::json(json!({"project":project,"commits":[
        pre_iso_commit(SHA, "2026-11-10T09:18:23.123456+01:00"),
        pre_iso_commit(SHA, "2026-11-10T08:18:23Z"),
    ]}));
    let dates: Vec<_> = get_revisions(&s.base, "t", false, ID)
        .unwrap()
        .commits
        .into_iter()
        .map(|c| c.authored_date)
        .collect();
    s.finish();
    assert_eq!(
        dates,
        [
            Some("2026-11-10T08:18:23.123Z".to_owned()),
            Some("2026-11-10T08:18:23.000Z".to_owned())
        ]
    );
}

#[test]
fn pre_iso_revision_dates_reach_the_history_as_sent() {
    let parent = "1111111111111111111111111111111111111111";
    let mut child = pre_iso_commit(SHA, "2026/11/10 03:18");
    child["parent_ids"] = json!([parent]);
    let s = Server::json(json!({"project":pre_iso_project(),"commits":[
        child,
        pre_iso_commit(parent, "2026-11-09T08:00:00.5Z"),
        pre_iso_commit(parent, "not a date"),
    ]}));
    let history = get_revisions(&s.base, "t", false, ID).unwrap();
    s.finish();
    // The wall time goes to the frontend unchanged (it knows New York's rules), an
    // instant is normalised, and a date neither format reads shows no time.
    let dates: Vec<_> = history
        .commits
        .iter()
        .map(|c| c.authored_date.clone())
        .collect();
    assert_eq!(
        dates,
        [
            Some("2026/11/10 03:18".to_owned()),
            Some("2026-11-09T08:00:00.500Z".to_owned()),
            None
        ]
    );
    assert_eq!(history.commits[0].id, SHA);
    assert_eq!(history.commits[0].parent_ids, [parent]);
    assert_eq!(history.commits[0].author_email, "test@example.com");
    assert_eq!(history.commits[0].message.as_deref(), Some("Synthetic"));
}

#[test]
fn pre_iso_dates_do_not_excuse_any_other_contract_mismatch() {
    let mut missing_id = pre_iso_commit(SHA, "2026/11/10 03:18");
    missing_id.as_object_mut().unwrap().remove("id");
    let mut unknown_field = pre_iso_commit(SHA, "2026/11/10 03:18");
    unknown_field["surprise"] = json!(true);
    let mut drifted_project = pre_iso_project();
    drifted_project["visibility"] = json!("FUTURE_VISIBILITY");
    for reply in [
        json!({"project":pre_iso_project(),"commits":[missing_id]}),
        json!({"project":pre_iso_project(),"commits":[unknown_field]}),
        json!({"project":drifted_project,"commits":[pre_iso_commit(SHA, "2026/11/10 03:18")]}),
    ] {
        let s = Server::json(reply);
        let err = get_revisions(&s.base, "t", false, ID).unwrap_err();
        s.finish();
        assert!(matches!(&err, SpeleoDbError::Parse(_)), "got {err:?}");
    }
    let mut drifted_project = pre_iso_project();
    drifted_project["visibility"] = json!("FUTURE_VISIBILITY");
    let s = Server::json(drifted_project);
    let err = get_project(&s.base, "t", false, ID).unwrap_err();
    s.finish();
    assert!(err.to_string().contains("unknown variant `...`"), "{err}");
}

#[test]
fn every_project_reply_from_a_pre_iso_server_decodes() {
    let check = |result: Result<Project, SpeleoDbError>| {
        assert_eq!(result.unwrap().latest_commit.unwrap().id, SHA);
    };
    for probe in [false, true] {
        let s = Server::json(pre_iso_project());
        check(if probe {
            get_project_probe(&s.base, "t", false, ID)
        } else {
            get_project(&s.base, "t", false, ID)
        });
        s.finish();
    }
    for acquire in [true, false] {
        let s = Server::json(pre_iso_project());
        check(if acquire {
            acquire_lock(&s.base, "t", false, ID)
        } else {
            release_lock(&s.base, "t", false, ID)
        });
        s.finish();
    }
    let s = Server::replies(vec![(201, pre_iso_project().to_string().into_bytes())]);
    check(create_project_with(
        &s.base,
        "t",
        false,
        &json!({"name":"Synthetic","description":"Test","country":"US","type":"STALAGMIGHT"})
            .to_string(),
    ));
    s.finish();
    let s = Server::json(pre_iso_project());
    check(patch_project(
        &s.base,
        "t",
        false,
        ID,
        r#"{"name":"Renamed"}"#,
    ));
    s.finish();
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), b"SQLite format 3\0").unwrap();
    let s = Server::json(
        json!({"project":pre_iso_project(),"hexsha":SHA,"files":["project.stalx"],"message":"Saved","browser_url":null}),
    );
    check(upload_stalx(
        &s.base,
        "t",
        false,
        ID,
        f.path(),
        "Synthetic upload",
    ));
    s.finish();
}
