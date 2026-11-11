use crate::models::Project;
use crate::persistence::Database;

use super::AppState;

// `any(test, feature = "test-support")`, not plain `cfg(test)`: the root
// crate's tests call these setters through `test_helpers` and directly, and a
// dependency's `cfg(test)` items are invisible to a dependent's test build.
// The source-scan machinery below stays `cfg(test)` - its only callers are
// this crate's own tests.
#[cfg(any(test, feature = "test-support"))]
impl AppState {
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_project_for_tests(&self, project: Project) {
        if let Ok(mut guard) = self.project.lock() {
            *guard = Some(project);
        }
    }

    /// Install `db` as the open project's database, and say what that makes the
    /// open project the way an open would: a linked database is a SpeleoDB
    /// project, read-only until an edit session binds a lock claim
    /// (`speleodb_edit_gate`) - for which a fixture runs Start Editing.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_database_for_tests(&self, db: Database) {
        let link = db.load_speleo_link().ok().flatten();
        if let Ok(mut guard) = self.database.lock() {
            *guard = Some(db);
        }
        self.speleodb_set_open_gate(link.as_ref());
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_initial_save_deferred_for_tests(&self, deferred: bool) {
        self.initial_save_deferred
            .store(deferred, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Every production region in the `core::state::speleodb*` family, paired with the
/// file it came from.
///
/// ADD A LINE HERE when a new `speleodb_*.rs` module is born. The tripwires built on
/// this count source-level facts about the orchestration, and the orchestration is
/// spread across the family - a scan that stops seeing a file stops guarding it, and
/// an absence assertion goes on passing while it does.
#[cfg(test)]
fn speleodb_prod_sources() -> [(&'static str, &'static str); 20] {
    [
        ("speleodb.rs", include_str!("speleodb.rs")),
        ("speleodb_admin.rs", include_str!("speleodb_admin.rs")),
        ("speleodb_cache.rs", include_str!("speleodb_cache.rs")),
        (
            "speleodb_cache_install.rs",
            include_str!("speleodb_cache_install.rs"),
        ),
        ("speleodb_cached.rs", include_str!("speleodb_cached.rs")),
        (
            "speleodb_credentials.rs",
            include_str!("speleodb_credentials.rs"),
        ),
        (
            "speleodb_edit_gate.rs",
            include_str!("speleodb_edit_gate.rs"),
        ),
        ("speleodb_foreign.rs", include_str!("speleodb_foreign.rs")),
        (
            "speleodb_local_state.rs",
            include_str!("speleodb_local_state.rs"),
        ),
        (
            "speleodb_lock_release.rs",
            include_str!("speleodb_lock_release.rs"),
        ),
        ("speleodb_logout.rs", include_str!("speleodb_logout.rs")),
        ("speleodb_project.rs", include_str!("speleodb_project.rs")),
        (
            "speleodb_write_guard.rs",
            include_str!("speleodb_write_guard.rs"),
        ),
        ("speleodb_pull.rs", include_str!("speleodb_pull.rs")),
        ("speleodb_reload.rs", include_str!("speleodb_reload.rs")),
        ("speleodb_push.rs", include_str!("speleodb_push.rs")),
        (
            "speleodb_revision_swap.rs",
            include_str!("speleodb_revision_swap.rs"),
        ),
        (
            "speleodb_revisions.rs",
            include_str!("speleodb_revisions.rs"),
        ),
        ("speleodb_session.rs", include_str!("speleodb_session.rs")),
        ("speleodb_sidecars.rs", include_str!("speleodb_sidecars.rs")),
    ]
    .map(|(name, src)| {
        // Only the top-level test module ends production. `speleodb_push.rs` also has
        // a test-only method, whose indented attribute must not truncate every source
        // scan before the real push implementation.
        (name, src.split("\n#[cfg(test)]").next().unwrap_or(src))
    })
}

/// How many times `needle` appears across the family's production regions.
///
/// `skip` names files to leave out: an absence assertion about what the
/// orchestration may *call* must not match the definition site itself.
#[cfg(test)]
pub(super) fn speleodb_prod_matches(needle: &str, skip: &[&str]) -> usize {
    speleodb_prod_sources()
        .iter()
        .filter(|(name, _)| !skip.contains(name))
        .map(|(_, src)| src.matches(needle).count())
        .sum()
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::speleodb_prod_sources;

    /// The array above must list EVERY `speleodb*.rs` in the family, and nothing
    /// enforced that but a doc comment.
    ///
    /// It matters because of what rides on it. One of the tripwires it feeds is an
    /// ABSENCE assertion - `replace_cache_stalx_abandoning` must have no caller outside
    /// its own file - and an absence assertion over a scan that has stopped seeing a
    /// file goes on passing while the thing it forbids walks in through that file. A
    /// forgotten line is loud for the counting tripwires and completely silent for that
    /// one, which is the wrong way round. Not hypothetical: every module the #275 plan
    /// scheduled has since landed - `speleodb_session.rs`, and now `speleodb_push.rs` -
    /// and each one arrived as a file this array had to grow a line for.
    ///
    /// `include_str!` needs literal paths, so the array itself cannot be derived. What
    /// CAN be derived is whether it is complete - read the directory and say which file
    /// is missing.
    #[test]
    fn every_speleodb_module_in_the_family_is_registered_for_the_source_scans() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/state");
        let registered: Vec<&str> = speleodb_prod_sources().iter().map(|(n, _)| *n).collect();

        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("the state module directory is readable")
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().to_string_lossy().into_owned();
                (name.starts_with("speleodb")
                    && name.ends_with(".rs")
                    && !name.ends_with("_tests.rs"))
                .then_some(name)
            })
            .collect();
        on_disk.sort();

        let missing: Vec<&String> = on_disk
            .iter()
            .filter(|name| !registered.contains(&name.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "these `speleodb*.rs` modules are not registered in \
             `speleodb_prod_sources`, so every source scan built on it silently stops \
             covering them: {missing:?}. Add them there."
        );

        // And nothing registered that no longer exists - a stale entry would not fail
        // to compile if the file were merely renamed alongside a new one.
        let stale: Vec<&&str> = registered
            .iter()
            .filter(|name| !on_disk.iter().any(|d| d == *name))
            .collect();
        assert!(stale.is_empty(), "registered but not on disk: {stale:?}");
    }
}

/// Complete the synthetic HTTP fixtures to the SDK's full response schema.
/// Older orchestration tests specify only fields relevant to the race they exercise.
#[cfg(test)]
pub(super) fn sdk_fixture(body: &str) -> String {
    use serde_json::{json, Value};
    let Ok(mut value) = serde_json::from_str::<Value>(body) else {
        return body.to_owned();
    };
    let Some(obj) = value.as_object_mut() else {
        return body.to_owned();
    };
    if obj.contains_key("id") {
        let defaults = json!({"color":"#123456", "commit_count":0,"country":"US", "created_by":"test@example.com", "creation_date":"2026-01-01T00:00:00Z", "modified_date":"2026-01-01T00:00:00Z", "description":"Synthetic test", "exclude_geojson":false, "fork_from":null, "is_active":true,"latest_commit":null,"active_mutex":null,"type":"STALAGMIGHT","visibility":"PRIVATE"});
        if let Some(defaults) = defaults.as_object() {
            for (key, value) in defaults {
                obj.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        for (field, key) in [("active_mutex", "user"), ("latest_commit", "id")] {
            if let Some(text) = obj
                .get(field)
                .and_then(|v| v.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                obj.insert(field.into(), Value::String(text));
            }
        }
    }
    if obj.contains_key("email") {
        for (key, value) in [
            ("country", json!("US")),
            ("name", json!("Test")),
            ("email_on_projects_updates", json!(false)),
            ("email_on_speleodb_updates", json!(false)),
        ] {
            obj.entry(key).or_insert(value);
        }
    }
    if let Some(Value::Array(commits)) = obj.get_mut("commits") {
        for commit in commits {
            if let Some(commit) = commit.as_object_mut() {
                for (key, value) in [
                    ("author_email", json!("test@example.com")),
                    ("author_name", json!("Test")),
                    ("authored_date", json!("2026-01-01T00:00:00Z")),
                    ("dt_since", json!("now")),
                    ("formats", json!([])),
                    ("message", json!("Test")),
                    ("parent_ids", json!([])),
                    ("tree", json!([])),
                    ("url", Value::Null),
                ] {
                    commit.entry(key).or_insert(value);
                }
            }
        }
        let project =
            sdk_fixture(r#"{"id":"11111111-1111-1111-1111-111111111111","name":"Synthetic"}"#);
        if let Ok(project) = serde_json::from_str::<Value>(&project) {
            obj.entry("project").or_insert(project);
        }
    }
    value.to_string()
}

/// A loopback SpeleoDB that answers EVERY request 200 with project `project_id`
/// whose lock is held by `holder` (or by nobody), and records each request line.
///
/// For the holder-checked releases (lock#4): a release first asks who holds the
/// lock and may send the release only when the answer is its own account, so a
/// one-shot stub that answers the release alone no longer describes the
/// protocol. Count the releases with [`release_posts`]. Never blocks a test that
/// expects no traffic.
#[cfg(test)]
pub(super) fn lock_server(
    project_id: &str,
    holder: Option<String>,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    lock_server_with(project_id, holder, false)
}

/// [`lock_server`] whose READS fail with a 503 (writes still answer 200): the
/// holder of the lock cannot be found out, which is what a release must never
/// treat as permission to release blind.
#[cfg(test)]
pub(super) fn lock_server_unreadable(
    project_id: &str,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    lock_server_with(project_id, None, true)
}

#[cfg(test)]
fn lock_server_with(
    project_id: &str,
    holder: Option<String>,
    reads_fail: bool,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read as _, Write as _};
    let Ok(listener) = std::net::TcpListener::bind("127.0.0.1:0") else {
        return ("http://127.0.0.1:1".to_owned(), std::sync::Arc::default());
    };
    let port = listener.local_addr().map_or(1, |a| a.port());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&seen);
    let mutex = holder.map_or_else(String::new, |user| {
        format!(r#","active_mutex":{{"user":"{user}"}}"#)
    });
    let body = sdk_fixture(&format!(
        r#"{{"id":"{project_id}","name":"Remote","permission":"READ_AND_WRITE"{mutex}}}"#
    ));
    // The release's reply is the project AFTER the release, as SpeleoDB sends it:
    // no holder. (A reply that still names the releasing account is a release
    // that did nothing, which the holder-checked release reports as a failure.)
    let released = sdk_fixture(&format!(
        r#"{{"id":"{project_id}","name":"Remote","permission":"READ_AND_WRITE"}}"#
    ));
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
            let mut req = Vec::new();
            let mut chunk = vec![0u8; 8 * 1024];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&chunk[..n]);
            }
            let line = String::from_utf8_lossy(&req)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let resp = if reads_fail && line.starts_with("GET") {
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\
                 Connection: close\r\n\r\n"
                    .to_owned()
            } else {
                let reply = if line.starts_with("POST") && line.contains("/release") {
                    &released
                } else {
                    &body
                };
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
            };
            if let Ok(mut log) = log.lock() {
                log.push(line);
            }
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

/// Model SpeleoDB's mutex across a multi-request stub that otherwise answers one
/// static `body`: an acquire makes `acquirer` the holder, a release frees it, and
/// every later project reply reports that holder instead of the fixture's.
///
/// The holder-checked release (lock#4) reads the holder AFTER the flow's own
/// acquire, so a stub frozen on its pre-acquire answer would say "not yours" about
/// a lock the flow has just taken. `held` is the stub's state, `None` until the
/// first acquire or release.
#[cfg(test)]
pub(super) fn with_tracked_mutex(
    request_line: &str,
    body: &str,
    held: &std::sync::Mutex<Option<serde_json::Value>>,
    acquirer: &str,
) -> String {
    use serde_json::{json, Value};
    let Ok(mut held) = held.lock() else {
        return body.to_owned();
    };
    if request_line.starts_with("POST") && request_line.contains("/acquire") {
        *held = Some(json!({ "user": acquirer }));
    } else if request_line.starts_with("POST") && request_line.contains("/release") {
        *held = Some(Value::Null);
    }
    let Some(holder) = held.clone() else {
        return body.to_owned();
    };
    let Ok(mut value) = serde_json::from_str::<Value>(body) else {
        return body.to_owned();
    };
    if let Some(obj) = value.as_object_mut() {
        if obj.contains_key("id") {
            obj.insert("active_mutex".to_owned(), holder);
        }
    }
    value.to_string()
}

/// How many release requests a [`lock_server`] has received.
#[cfg(test)]
pub(super) fn release_posts(seen: &std::sync::Mutex<Vec<String>>) -> usize {
    seen.lock().map_or(0, |log| {
        log.iter()
            .filter(|line| line.starts_with("POST") && line.contains("/release"))
            .count()
    })
}
