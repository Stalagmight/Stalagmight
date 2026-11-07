//! [#576 A8] A load refuses a second local copy of a SpeleoDB project BEFORE it
//! opens the database - driven here, where it used to be pinned by the order of
//! two calls in `do_load_project_full`'s source text.
//!
//! The order matters because `Database::open_existing` MIGRATES the file and
//! quick-checks every page: a copy the user is about to be told to leave alone
//! must not be rewritten first. It is observable through a copy that the probe
//! can read but the full open rejects - a linked file with no project row. The
//! probe-first load names the managed project (`SPELEODB_MANAGED`, the frontend
//! offers "open the canonical copy" / "drop the link"); an open-first load
//! would fail on the missing project data instead.
//!
//! Its own test binary, because the probe compares the file against the
//! SpeleoDB cache root, which lives in the process-wide app directories:
//! setting those in the library's unit-test binary would leak into every
//! other test there.

#![expect(
    clippy::expect_used,
    reason = "test setup failures should stop the test immediately"
)]

use std::sync::Arc;

use stalagmight_core::app_dirs;
use stalagmight_core::core::events::EventSink;
use stalagmight_core::core::state::AppState;
use stalagmight_core::models::{CommandErrorCode, SpeleoLink};
use stalagmight_core::persistence::Database;

/// The load refuses to run without a sink; nothing listens here.
struct NoListener;

impl EventSink for NoListener {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

#[test]
fn a_managed_copy_outside_the_cache_is_refused_before_its_database_is_opened() {
    let root = tempfile::TempDir::new().expect("temp root");
    app_dirs::init_paths(
        root.path().join("data"),
        root.path().join("config"),
        root.path().join("cache"),
        None,
    );

    // A copy of a managed project somewhere other than its cache path: linked,
    // but with no project row, so only the probe can make sense of it.
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("create folder");
    let copy = elsewhere.join("copy.stalx");
    {
        let db = Database::open(&copy).expect("create the copy");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "https://example.invalid".to_owned(),
            project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            project_name: "CaveA".to_owned(),
            base_commit_sha: None,
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        }))
        .expect("link the copy");
    }
    let bytes_before = std::fs::read(&copy).expect("read the copy");

    let state = AppState::new().expect("construct state");
    state.set_event_sink(Arc::new(NoListener));
    let refused = state
        .do_load_project_full(copy.clone())
        .err()
        .expect("a second local copy of a managed project must not open");

    assert_eq!(
        refused.code,
        CommandErrorCode::SpeleodbManaged,
        "the canonical-storage probe must answer before the database is opened (an open \
         first fails on the missing project data instead): {}",
        refused.message
    );
    assert_eq!(
        refused
            .speleodb_managed
            .as_ref()
            .map(|managed| managed.project_name.as_str()),
        Some("CaveA"),
        "the refusal names the project, so the frontend can offer its canonical copy"
    );
    assert!(
        std::fs::read(&copy).expect("re-read the copy") == bytes_before,
        "a refused copy is left exactly as it was - not migrated, not rewritten"
    );
}
