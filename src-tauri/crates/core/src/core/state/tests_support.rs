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
        {
            let mut guard = self.project.lock();
            guard.set(Some(project));
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_database_for_tests(&self, db: Database) {
        {
            let mut guard = self.database.lock();
            *guard = Some(db);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_initial_save_deferred_for_tests(&self, deferred: bool) {
        self.initial_save_deferred
            .store(deferred, std::sync::atomic::Ordering::Relaxed);
    }

    /// The edit gate as a Start editing that took the lock leaves it (#191), with
    /// no server round trip: the app crate's tests of what the IPC answers on a
    /// remote project in an edit session (#576 D22).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_edit_enabled_for_tests(&self, enabled: bool) {
        self.set_edit_enabled(enabled);
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
        ("speleodb_adopt.rs", include_str!("speleodb_adopt.rs")),
        ("speleodb_cache.rs", include_str!("speleodb_cache.rs")),
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
        // [#576 RV10a-3] The lock-ops span module lives in its own file (a doctest
        // `include!`s it) but is `speleodb_session.rs`'s child: scanned as part of it,
        // ahead of it, so the session file's test module still ends the region.
        (
            "speleodb_session.rs",
            concat!(
                include_str!("speleodb_session/lock_ops_span.rs"),
                include_str!("speleodb_session.rs")
            ),
        ),
    ]
    .map(|(name, src)| {
        // [#576 A8 / E9(b)] Only the top-level test MODULE ends production: a gated
        // `use`, the test-only method in `speleodb_push.rs`, or the attribute's text
        // in a comment must not truncate every scan built on this before the real
        // orchestration (`crate::models::source_scan::prod_region`).
        (
            name,
            crate::models::source_scan::prod_region(src, "impl AppState {"),
        )
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
    /// It matters because of what rides on it. One of the inventories it feeds is an
    /// ABSENCE assertion - no production region may call a test-only proof
    /// constructor (`::for_tests(`, `no_production_code_reaches_a_test_only_proof_
    /// constructor`) - and an absence assertion over a scan that has stopped seeing a
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
