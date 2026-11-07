//! [#576 RV4-R-S6] A Done or Push that fails after its pre-push save still tells
//! the user what that save could not keep.
//!
//! The pre-push save's warnings travelled only on the Ok answer. A push refused or
//! dropped after the save went through (or a render / snapshot failing after it)
//! returned the error alone, and the warnings - one-shot, the next save does not
//! repeat them - were lost. Done and Push now run through
//! `speleodb_with_pre_push_save`, which hands them to the frontend through the
//! event sink when the verb fails.
//!
//! The helper is driven directly: Done and Push reach the save only after the
//! cache-dir check that only the env-gated live tests can pass (see
//! `sdb_flush_*_tests.rs`). That both verbs go through it is the type's job since
//! #576 A8 (it used to be a source check here): their bodies collect into a
//! `PrePushSave`, which only the helper makes.

use std::sync::{Arc, Mutex};

use super::speleodb::local_dto;
use super::AppState;
use crate::core::{ValidationWarning, WarningSeverity};
use crate::speleodb::SpeleoDbError;

struct Recorder(Arc<Mutex<Vec<(String, serde_json::Value)>>>);

impl crate::core::events::EventSink for Recorder {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.0.lock().unwrap().push((event.to_owned(), payload));
    }
    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

fn lost_display_settings() -> ValidationWarning {
    ValidationWarning {
        code: "DISPLAY_SETTINGS_NOT_SAVED".to_owned(),
        message: "the display settings could not be saved".to_owned(),
        severity: WarningSeverity::Warning,
    }
}

fn recording_state() -> (AppState, Arc<Mutex<Vec<(String, serde_json::Value)>>>) {
    let state = AppState::new().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    state.set_event_sink(Arc::new(Recorder(Arc::clone(&events))));
    (state, events)
}

#[test]
fn a_push_failing_after_its_save_still_reports_the_save_warnings() {
    let _cfg = crate::test_helpers::isolated_config();
    let (state, events) = recording_state();

    let failed = state.speleodb_with_pre_push_save(|collected| -> Result<(), _> {
        // The save went through and lost the display settings ...
        collected.push(lost_display_settings());
        // ... then the upload was refused.
        Err(local_dto(SpeleoDbError::refused(
            "upload refused".to_owned(),
        )))
    });

    let sent = events.lock().unwrap().clone();
    println!(
        "failed push -> {:?}; events: {sent:?}",
        failed.as_ref().err().map(|e| &e.message)
    );
    assert!(failed.is_err(), "the verb's own error is returned");
    assert_eq!(sent.len(), 1, "one event carries the save's outcome");
    let (name, payload) = &sent[0];
    assert_eq!(name, "speleodb-pre-push-save");
    assert_eq!(
        payload["warnings"][0]["code"], "DISPLAY_SETTINGS_NOT_SAVED",
        "the event carries what the save could not keep"
    );
}

#[test]
fn a_successful_push_or_a_failure_with_nothing_lost_sends_no_event() {
    let _cfg = crate::test_helpers::isolated_config();
    let (state, events) = recording_state();

    let ok = state.speleodb_with_pre_push_save(|collected| {
        collected.push(lost_display_settings());
        Ok(7)
    });
    let failed_clean = state.speleodb_with_pre_push_save(|_| -> Result<(), _> {
        Err(local_dto(SpeleoDbError::refused(
            "upload refused".to_owned(),
        )))
    });

    let sent = events.lock().unwrap().len();
    println!(
        "ok -> {:?}; clean failure -> {:?}; events sent: {sent}",
        ok.as_ref().map(|(r, w)| (r, w.len())),
        failed_clean.as_ref().err().map(|e| &e.message)
    );
    assert_eq!(
        ok.map(|(r, w)| (r, w.len())).ok(),
        Some((7, 1)),
        "the Ok answer carries the warnings itself"
    );
    assert!(failed_clean.is_err());
    assert_eq!(
        sent, 0,
        "no event: the Ok answer carries them, or nothing was lost"
    );
}
