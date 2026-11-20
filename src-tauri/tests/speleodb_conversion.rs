//! Synthetic conversion contract; this separate process owns its temporary app dirs.
//! Every project file is a `.stalx` version 2 file written by the encoder.
#![expect(
    clippy::expect_used,
    reason = "synthetic HTTP and project-file test setup"
)]
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde_json::json;
use stalagmight_lib::core::{AppState, Command};
use stalagmight_lib::models::survey::Project;
use stalagmight_lib::persistence::stalx_v2::{self, DecodeOptions, Extras, Mode};
use stalagmight_lib::speleodb::{SpeleoPublishParams, Visibility};

/// The link (identity and sync state) a version 2 file at `path` holds.
fn link_at(path: &std::path::Path) -> Option<(String, bool)> {
    let header = stalx_v2::probe_path(path).expect("probe");
    let link = header.speleodb?;
    let dirty = header
        .local
        .is_some_and(|local| local.speleodb.unpushed_edits);
    Some((link.project_id, dirty))
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn conversion_saves_unsaved_survey_uploads_once_and_requires_verified_editing_after_creation() {
    let temp = tempfile::tempdir().expect("temporary app directories");
    stalagmight_lib::models::app_dirs::init_paths(
        temp.path().join("data"),
        temp.path().join("config"),
        temp.path().join("cache"),
        None,
    );
    for (attempt, fail_upload) in [false, true].into_iter().enumerate() {
        let id = format!("11111111-1111-1111-1111-11111111111{attempt}");
        let listener = TcpListener::bind("127.0.0.1:0").expect("local server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_stop = Arc::clone(&stop);
        let server_requests = Arc::clone(&requests);
        let server_id = id.clone();
        let server = std::thread::spawn(move || {
            let mut uploaded = false;
            // The server's mutex: taken by the publish's acquire, freed by its
            // release, and reported by the release's holder check (lock#4).
            let mut held = false;
            while !server_stop.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                stream
                    .set_nonblocking(false)
                    .expect("blocking request stream");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("request timeout");
                let mut request = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let count = stream.read(&mut buffer).expect("request bytes");
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&request).into_owned();
                let route = text.lines().next().expect("request line");
                if route.contains("/acquire/") {
                    held = true;
                } else if route.contains("/release/") {
                    held = false;
                }
                let holder = if held {
                    json!("test@example.com")
                } else {
                    serde_json::Value::Null
                };
                let project = json!({"id":server_id,"name":"Latest local edits","description":"Synthetic conversion","country":"US",
                    "type":"STALAGMIGHT","visibility":"PRIVATE","permission":"ADMIN","color":"#123456",
                    "commit_count":u8::from(uploaded),"created_by":"test@example.com","creation_date":"2026-01-01T00:00:00Z",
                    "modified_date":"2026-01-01T00:00:00Z","exclude_geojson":false,"fork_from":null,"is_active":true,
                    "active_mutex":holder,"latest_commit":null});
                let (code, body) = if route.contains("/user/") {
                    (
                        200,
                        json!({"email":"test@example.com","name":"Test","country":"US","email_on_projects_updates":false,"email_on_speleodb_updates":false}),
                    )
                } else if route.contains("/upload/") {
                    if fail_upload {
                        (403, json!({"detail":"Synthetic upload refusal"}))
                    } else {
                        uploaded = true;
                        (
                            200,
                            json!({"project":project,"hexsha":SHA,"files":["project.stalx"],"message":"Saved","browser_url":null}),
                        )
                    }
                } else if route.contains("/revisions/") {
                    let commits = if uploaded {
                        vec![
                            json!({"id":SHA,"parent_ids":[],"author_email":"test@example.com","author_name":"Test","authored_date":"2026-01-01T00:00:00Z","dt_since":"now","message":"Project Creation","formats":[],"tree":[],"url":null}),
                        ]
                    } else {
                        Vec::new()
                    };
                    (200, json!({"project":project,"commits":commits}))
                } else {
                    (
                        if route.starts_with("POST /api/v2/projects/ HTTP") {
                            201
                        } else {
                            200
                        },
                        project,
                    )
                };
                server_requests.lock().expect("request log").push(text);
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {code} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).expect("HTTP reply");
            }
        });
        let original_path = temp.path().join(format!("local-{attempt}.stalx"));
        let original = Project::new("Local original", None);
        std::fs::write(
            &original_path,
            stalx_v2::encode(&original, &Extras::default(), Mode::Normal).expect("encode"),
        )
        .expect("original save");
        let state = AppState::new().expect("state");
        assert!(
            state
                .execute(Command::OpenProject {
                    path: original_path.clone()
                })
                .success,
            "open the local project"
        );
        // An unsaved survey edit the conversion has to save before it uploads.
        assert!(
            state
                .update_project_settings(
                    "Latest local edits".into(),
                    original.settings.validation.clone()
                )
                .success,
            "rename"
        );
        state
            .speleodb_login(Some(base), None, None, None, Some("synthetic-token".into()))
            .expect("sign in");
        let result = state
            .speleodb_publish(SpeleoPublishParams {
                name: "Latest local edits".into(),
                description: "Synthetic conversion".into(),
                country: "US".into(),
                visibility: Visibility::Private,
                latitude: None,
                longitude: None,
                message: Some("Project Creation".into()),
            })
            .expect("creation converges on a linked project even when upload fails");
        let cached_path = std::path::Path::new(&result.local_path);
        let cached = stalx_v2::decode(
            &std::fs::read(cached_path).expect("saved cache"),
            &DecodeOptions::default(),
        )
        .expect("a valid cache file");
        assert_eq!(cached.project.name, "Latest local edits");
        let (_, dirty) = link_at(cached_path).expect("managed project");
        assert_eq!(dirty, fail_upload);
        assert!(
            !state.set_surface_elevation(Some(10.0)).success,
            "editing requires a fresh verified acquisition"
        );
        assert!(
            link_at(&original_path).is_none(),
            "the original local file stays unlinked"
        );
        state
            .speleodb_logout()
            .expect("disconnect before next synthetic server");
        stop.store(true, Ordering::SeqCst);
        server.join().expect("server shutdown");
        let requests = requests.lock().expect("request log");
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.starts_with("POST /api/v2/projects/ HTTP"))
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.contains("/upload/stalagmight/"))
                .count(),
            1
        );
        assert!(requests.iter().any(|request| request.contains("/release/")));
        assert!(
            requests
                .iter()
                .any(|request| request.contains("{\"format\":\"stalx\",\"version\":2")),
            "upload carries the saved version 2 project"
        );
    }
}
