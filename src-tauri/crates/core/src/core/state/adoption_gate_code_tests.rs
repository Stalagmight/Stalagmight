//! [#576 RV5-F4-5] An edit refused because an Ariane adoption is handing the
//! project over to SpeleoDB says so in a code the frontend can read.
//!
//! The refusal carried only `EDIT_DISABLED`, so the frontend showed its generic
//! "open read-only - click Start editing to take the write lock" notice in the
//! middle of the hand-over, which invites a Start editing that races the
//! adoption's own lock operations. It now carries `ADOPTION_IN_PROGRESS`, a code
//! of the edit-gate family (`CommandErrorCode::is_edit_gate`) like
//! `NEWER_FILE_EDIT_DISABLED` and `WORKSPACE_READ_ONLY`, so every caller that
//! treats a gated refusal as "read-only, not a failure" keeps doing so (#576 C4:
//! the code, not a marker in the message). All data is synthetic.

use crate::core::Command;
use crate::models::{CommandErrorCode, ShotData};
use crate::test_fixtures::state_with_project;

#[test]
fn an_edit_during_an_adoption_hand_over_carries_the_adoption_code() {
    let state = state_with_project("CaveA");
    state.speleodb_shut_gate_for_adoption();
    let refused = state.execute(Command::CreateSection {
        name: Some("SectionA".to_owned()),
        surveyor: None,
        date: None,
        color: "#336699".to_owned(),
        direction: None,
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
    });
    let error = refused.error.clone().unwrap_or_default();
    println!(
        "edit during the hand-over: success {}, code {:?}, error {error:?}",
        refused.success, refused.code
    );
    assert!(!refused.success);
    assert_eq!(
        refused.code,
        Some(CommandErrorCode::AdoptionInProgress),
        "the refusal names the adoption in its code: {error:?}"
    );
    assert!(
        refused.code.is_some_and(CommandErrorCode::is_edit_gate),
        "the adoption refusal is an edit-gate refusal"
    );
    assert!(
        !error.contains("EDIT_DISABLED") && !error.contains("ADOPTION_IN_PROGRESS"),
        "the message carries no marker: {error:?}"
    );
    assert!(error.contains("handed over to SpeleoDB"));

    // Once the adoption returns, edits go through again.
    state.speleodb_open_gate_after_adoption();
    let renamed = state.update_project_settings(
        "CaveB".to_owned(),
        state.get_project_state().unwrap().validation,
        None,
    );
    assert!(renamed.success, "{:?}", renamed.error);
}
