const WORKSPACE_NEWER_FREE_BODY: &str = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote","permission":"READ_AND_WRITE","active_mutex":null,"commits":[{"id":"def4567","parent_ids":["abc123"],"author_email":"u@x"},{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;
const WORKSPACE_NEWER_BODY: &str = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{"user":"u@x"},"commits":[{"id":"def4567","parent_ids":["abc123"],"author_email":"u@x"},{"id":"abc123","parent_ids":[],"author_email":"u@x"}]}"#;
// Included in speleodb::tests to reuse its synthetic HTTP and project builders.
fn workspace_context(state: &AppState) -> crate::speleodb::SpeleoReleaseContext {
    let (generation, link) = state.speleodb_session_snapshot().expect("snapshot");
    crate::speleodb::SpeleoReleaseContext {
        session_generation: generation.to_string(),
        project_id: link.project_id,
        endpoint: link.endpoint,
        user_email: "u@x".into(),
        project_modified_at: state
            .project
            .lock()
            .expect("project")
            .as_ref()
            .expect("open")
            .modified_at
            .to_rfc3339(),
    }
}

#[test]
fn workspace_readonly_refuses_pending_work_and_stale_consent_before_network() {
    for pending in ["unsaved", "unuploaded", "newer edit", "account", "session"] {
        let (_temp, state) = editing_fixture("https://unreachable.invalid/");
        let mut expected = workspace_context(&state);
        if pending == "unsaved" || pending == "newer edit" {
            state
                .project
                .lock()
                .expect("project")
                .as_mut()
                .expect("open")
                .touch();
        }
        if pending == "unsaved" {
            expected = workspace_context(&state);
        }
        if pending == "unuploaded" {
            state.mark_remote_dirty();
        }
        if pending == "account" {
            expected.user_email = "another@x".into();
        }
        if pending == "session" {
            expected.session_generation = "stale".into();
        }
        assert!(
            state.speleodb_release_current_lock(Some(expected)).is_err(),
            "{pending}"
        );
        assert!(
            state.edit_enabled(),
            "refusal preserves the mode: {pending}"
        );
    }
}

#[test]
fn workspace_reload_requires_consent_and_rejects_stale_project_versions() {
    let (_temp, state) = editing_fixture("https://unreachable.invalid/");
    let expected = workspace_context(&state);
    state.mark_remote_dirty();
    let error = state
        .speleodb_reload(false, Some(expected.clone()))
        .expect_err("no discard acknowledgement");
    assert!(error.message.contains("Save Changes"));
    assert!(state.edit_enabled());
    state
        .project
        .lock()
        .expect("project")
        .as_mut()
        .expect("open")
        .touch();
    let error = state
        .speleodb_reload(true, Some(expected))
        .expect_err("newer work invalidates old consent");
    assert!(error.message.contains("session") || error.message.contains("changed"));
}

#[test]
fn workspace_acquisition_checks_live_revisions_after_acquire_without_equal_revision_download() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    state.set_edit_enabled(false);
    assert!(matches!(
        state.speleodb_start_editing(),
        Ok(StartEditingResult::Editing)
    ));
    let requests = seen.lock().expect("log");
    let acquire = requests
        .iter()
        .position(|r| r.contains("/acquire/"))
        .expect("acquire");
    let history = requests
        .iter()
        .position(|r| r.contains("/revisions/"))
        .expect("live history");
    assert!(
        history > acquire,
        "revision verification follows acquisition"
    );
    assert!(!requests.iter().any(|r| r.contains("/download/")));
}

#[test]
fn workspace_reload_failed_download_preserves_pending_work_and_mode_without_acquiring() {
    let (base, _listener, seen) = recording_stub(WORKSPACE_NEWER_BODY);
    let (_temp, state) = editing_fixture(&base);
    state.mark_remote_dirty();
    let before = state.capture_project_snapshot().expect("before");
    let expected = workspace_context(&state);
    assert!(
        state.speleodb_reload(true, Some(expected)).is_err(),
        "JSON is not a native project"
    );
    assert_eq!(state.capture_project_snapshot().expect("after"), before);
    assert!(state.speleodb_open_link().expect("link").has_unpushed_edits);
    assert!(state.edit_enabled());
    let requests = seen.lock().expect("requests");
    assert!(requests.iter().any(|r| r.contains("/revisions/")));
    assert!(requests.iter().any(|r| r.contains("/download/")));
    assert!(!requests.iter().any(|r| r.contains("/acquire/")));
}

#[test]
fn workspace_local_settings_refuses_linked_rename_through_direct_calls() {
    let (_temp, state) = editing_fixture("https://unreachable.invalid/");
    let project = state
        .project
        .lock()
        .expect("project")
        .as_ref()
        .expect("open")
        .clone();
    assert!(
        !state
            .update_project_settings("Other name".into(), project.validation.clone())
            .success
    );
    assert_eq!(
        state
            .project
            .lock()
            .expect("project")
            .as_ref()
            .expect("open")
            .name,
        project.name
    );
}

#[test]
fn workspace_readonly_releases_only_a_clean_own_lock_and_closes_the_mutation_gate() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    state
        .speleodb_release_current_lock(Some(workspace_context(&state)))
        .expect("clean release");
    assert!(!state.edit_enabled());
    assert!(!state.set_surface_elevation(Some(10.0)).success);
    let requests = seen.lock().expect("requests");
    assert_eq!(
        requests.iter().filter(|r| r.contains("/release/")).count(),
        1
    );
    assert!(!requests.iter().any(|r| r.contains("/upload/")));
}

#[test]
fn workspace_ambiguous_history_never_starts_an_editing_session_or_discards_work() {
    const FORK: &str = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{"user":"u@x"},"commits":[{"id":"abc123","parent_ids":[]},{"id":"def456","parent_ids":["abc123"]},{"id":"fed321","parent_ids":["abc123"]}]}"#;
    let (base, _listener, seen) = recording_stub(FORK);
    let (_temp, state) = editing_fixture(&base);
    state.set_edit_enabled(false);
    assert!(matches!(
        state.speleodb_start_editing(),
        Ok(StartEditingResult::DivergenceNeeded)
    ));
    assert!(!state.edit_enabled());
    let requests = seen.lock().expect("requests");
    assert!(requests.iter().any(|r| r.contains("/revisions/")));
    assert!(!requests.iter().any(|r| r.contains("/download/")));
}

#[test]
fn workspace_historical_checkout_at_equal_base_still_downloads_on_acknowledged_reload() {
    let (base, _listener, seen) = recording_stub(
        r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Remote","permission":"READ_AND_WRITE","active_mutex":{"user":"u@x"},"commits":[{"id":"abc1234","parent_ids":[],"author_email":"u@x"}]}"#,
    );
    let (_temp, state) = editing_fixture(&base);
    state.speleodb_set_unwound_from(Some("def456"));
    state.mark_remote_dirty();
    assert!(state
        .speleodb_reload(true, Some(workspace_context(&state)))
        .is_err());
    assert!(
        state.speleodb_unwound_from().is_some(),
        "failed download preserves checkout"
    );
    let requests = seen.lock().expect("requests");
    assert!(
        requests
            .iter()
            .any(|r| r.contains("/download/stalagmight/abc1234/")),
        "return to the live revision even when BASE equals tip: {requests:?}"
    );
    assert!(!requests.iter().any(|r| r.contains("/acquire/")));
}

#[test]
fn workspace_prepared_reload_commits_validated_bytes_and_preserves_each_editing_mode() {
    for editing in [false, true] {
        let (temp, state) = editing_fixture("https://synthetic.example/");
        state.set_edit_enabled(editing);
        state.mark_remote_dirty();
        let source = temp.path().join("replacement.tmp");
        let dest = state
            .project
            .lock()
            .expect("project")
            .as_ref()
            .expect("open")
            .file_path
            .clone()
            .expect("path");
        let mut remote = Database::open(&source).expect("source");
        remote
            .save_project(&Project::new("New server version".into(), None))
            .expect("project");
        let mut link = state.speleodb_open_link().expect("link");
        link.has_unpushed_edits = false;
        link.base_commit_sha = Some("def4567".into());
        remote.save_speleo_link(Some(&link)).expect("linked");
        drop(remote);
        let expected = (
            state.speleodb_session_generation(),
            serde_json::to_vec(&*state.project.lock().expect("project")).expect("snapshot"),
        );
        assert!(
            state
                .open_project_download(source, dest.clone(), expected, editing)
                .expect("install")
                .success
        );
        assert_eq!(
            state.get_project_state().expect("open").name,
            "New server version"
        );
        assert_eq!(state.edit_enabled(), editing);
        assert!(!state.speleodb_open_link().expect("link").has_unpushed_edits);
        assert_eq!(
            Database::open_existing(&dest)
                .expect("saved")
                .load_project()
                .expect("project")
                .name,
            "New server version"
        );
    }
}

#[test]
fn workspace_prepared_reload_rejects_newer_edits_and_invalid_files_without_replacement() {
    for invalid in [false, true] {
        let (temp, state) = editing_fixture("https://synthetic.example/");
        let source = temp.path().join("replacement.tmp");
        let dest = state
            .project
            .lock()
            .expect("project")
            .as_ref()
            .expect("open")
            .file_path
            .clone()
            .expect("path");
        let bytes = std::fs::read(&dest).expect("original bytes");
        std::fs::copy(&dest, &source).expect("stage");
        let expected = (
            state.speleodb_session_generation(),
            serde_json::to_vec(&*state.project.lock().expect("project")).expect("snapshot"),
        );
        if invalid {
            std::fs::write(&source, b"not SQLite").expect("invalid file");
        } else {
            state
                .project
                .lock()
                .expect("project")
                .as_mut()
                .expect("open")
                .name = "Newer edit".into();
        }
        let before = state.capture_project_snapshot().expect("before");
        let result = state.open_project_download(source, dest.clone(), expected, true);
        assert!(result.is_err() || result.is_ok_and(|result| !result.success));
        assert_eq!(state.capture_project_snapshot().expect("after"), before);
        assert_eq!(std::fs::read(&dest).expect("unchanged bytes"), bytes);
        assert!(state.edit_enabled());
    }
}

#[test]
fn workspace_acquisition_refuses_an_unusable_verified_revision_without_downloading_head() {
    let (base, _listener, seen) = recording_stub(SAME_ACCOUNT_HOLDER_BODY);
    let (_temp, state) = editing_fixture(&base);
    state.set_edit_enabled(false);
    assert!(state.speleodb_start_editing().is_err());
    assert!(!state.edit_enabled());
    assert!(!seen
        .lock()
        .expect("requests")
        .iter()
        .any(|r| r.contains("/download/")));
}

#[test]
fn workspace_reload_rejects_edits_made_during_live_revision_verification() {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let (_temp, state) = editing_fixture(&base);
    let expected = workspace_context(&state);
    let (arrived, started) = std::sync::mpsc::channel();
    let (finish, resume) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("revision request");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("timeout");
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let count = stream.read(&mut chunk).expect("request");
            if count == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                break;
            }
        }
        assert!(String::from_utf8_lossy(&request).contains("/revisions/"));
        arrived.send(()).expect("request arrived");
        let _ = resume.recv_timeout(std::time::Duration::from_secs(2));
        let body = crate::core::state::tests_support::sdk_fixture(WORKSPACE_NEWER_BODY);
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("reply");
    });
    std::thread::scope(|scope| {
        let pending = scope.spawn(|| {
            state
                .speleodb_reload(true, Some(expected))
                .map_err(|error| error.message)
        });
        started
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("verification started");
        assert!(
            state.set_surface_elevation(Some(12.5)).success,
            "edits remain possible during verification"
        );
        finish.send(()).expect("finish verification");
        let error = pending
            .join()
            .expect("reload thread")
            .expect_err("newer edits invalidate discard consent");
        assert!(error.contains("changed"));
    });
    server.join().expect("server");
    assert_eq!(
        state
            .get_project_state()
            .expect("project")
            .surface_elevation,
        Some(12.5)
    );
    assert!(state.speleodb_open_link().expect("link").has_unpushed_edits);
    assert!(state.edit_enabled());
}
