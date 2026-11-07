//! `AppState` fixtures shared by the Rust tests (#576 E1), next to the
//! survey-data builders of `models::test_fixtures` (re-exported here, so a test
//! imports both from one place).
//!
//! Compiled for this crate's own tests and, through the `test-support` feature,
//! for the app crate's; production builds never see this module.
//!
//! | fixture | what it gives the test |
//! |---|---|
//! | [`state_with_project`]`(name)` | a fresh `AppState` with an empty project created through `Command::CreateProject` (no location): the prelude almost every state test starts from |
//! | [`state_holding`]`(project)` | a fresh `AppState` with `project` installed as it is (`set_project_for_tests`): no command runs, no history is recorded |
//! | [`state_with_null_sink`]`()` | a fresh `AppState`, no project, with a [`NullEventSink`] installed: what the progress-reporting load and create paths need to run without a host |
//! | [`section_with_shots`]`(state, name, shots)` | `Command::CreateSection` with fixed defaults (color `#3388FF`, no surveyor, date or direction); returns the new section's id |
//!
//! Each one fails the calling test (not this file) when its setup step fails.

use std::sync::Arc;

use crate::core::events::NullEventSink;
use crate::core::{AppState, Command};
use crate::models::{Project, SectionId, ShotData};

pub use crate::models::test_fixtures::{bare_shot, heading_shot, shot, straight_shot};

/// A fresh `AppState` with an empty project named `name`, created through
/// `Command::CreateProject` with no location.
///
/// # Panics
///
/// When the state cannot be built or the create fails.
#[track_caller]
pub fn state_with_project(name: &str) -> AppState {
    let state = AppState::new().expect("AppState::new");
    let created = state.execute(Command::CreateProject {
        name: name.to_string(),
        location: None,
    });
    assert!(created.success, "CreateProject failed: {:?}", created.error);
    state
}

/// A fresh `AppState` holding `project` exactly as given.
///
/// # Panics
///
/// When the state cannot be built.
#[track_caller]
pub fn state_holding(project: Project) -> AppState {
    let state = AppState::new().expect("AppState::new");
    state.set_project_for_tests(project);
    state
}

/// A fresh `AppState`, no project open, whose event sink delivers nothing.
///
/// # Panics
///
/// When the state cannot be built.
#[track_caller]
pub fn state_with_null_sink() -> AppState {
    let state = AppState::new().expect("AppState::new");
    state.set_event_sink(Arc::new(NullEventSink));
    state
}

/// Create a section named `name` holding `shots` and return its id.
///
/// # Panics
///
/// When the create fails.
#[track_caller]
pub fn section_with_shots(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    let created = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#3388FF".to_string(),
        direction: None,
        shots,
    });
    assert!(created.success, "CreateSection failed: {:?}", created.error);
    created.section_id.expect("CreateSection returned no id")
}
