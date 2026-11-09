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

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_database_for_tests(&self, db: Database) {
        if let Ok(mut guard) = self.database.lock() {
            *guard = Some(db);
        }
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
fn speleodb_prod_sources() -> [(&'static str, &'static str); 14] {
    [
        ("speleodb.rs", include_str!("speleodb.rs")),
        ("speleodb_admin.rs", include_str!("speleodb_admin.rs")),
        ("speleodb_cache.rs", include_str!("speleodb_cache.rs")),
        (
            "speleodb_cache_install.rs",
            include_str!("speleodb_cache_install.rs"),
        ),
        (
            "speleodb_credentials.rs",
            include_str!("speleodb_credentials.rs"),
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
        ("speleodb_pull.rs", include_str!("speleodb_pull.rs")),
        ("speleodb_push.rs", include_str!("speleodb_push.rs")),
        (
            "speleodb_revisions.rs",
            include_str!("speleodb_revisions.rs"),
        ),
        ("speleodb_session.rs", include_str!("speleodb_session.rs")),
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
                (name.starts_with("speleodb") && name.ends_with(".rs")).then_some(name)
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
                    ("authored_date", json!("2026/01/01 00:00")),
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
