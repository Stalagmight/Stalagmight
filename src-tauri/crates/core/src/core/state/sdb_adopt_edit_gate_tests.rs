//! [#576 NF-W4C-3-EDIT / NF-W5L-2] The edit gate stays shut from the Ariane
//! adoption until its hand-over push has landed.
//!
//! After the adoption's local span (build, promote, reopen, stamp) the adopted
//! project is open, and it reopened as an ordinary editable file - the gate was
//! derived before the link was stamped. Through the hand-over (Start editing, the
//! push, Done) and up to the adoption's final close, an edit could land and ride
//! the push, or be lost by the close. Decided: shut the gate from the adoption
//! until the hand-over lands; an edit there is refused as an edit-gate refusal
//! (`ADOPTION_IN_PROGRESS`, RV5-F4-5).
//!
//! Driven through `adopt_import_and_save`, the local half, with a synthetic remote
//! file under a temp dir; nothing reaches a server.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::{Command, CommandError};
use crate::models::CommandErrorCode;
use crate::models::{ShotUpdates, SpeleoLink};
use crate::parsers::tml::test_helpers::{wrap_xml, TripShot};
use crate::test_fixtures::state_with_null_sink;

const PROJECT_ID: &str = "55555555-5555-5555-5555-555555555555";

fn remote_file(dir: &Path) -> PathBuf {
    let shots: Vec<String> = (1..=3)
        .map(|id| {
            TripShot {
                id,
                from_id: if id == 1 { -1 } else { id - 1 },
                section: "SectionA",
                ..TripShot::default()
            }
            .xml()
        })
        .collect();
    let path = dir.join("adopt-source.tmlu");
    std::fs::write(&path, wrap_xml(&shots.join("\n"))).unwrap();
    path
}

/// An edit of the adopted project: the first shot's length.
fn try_edit(state: &AppState) -> Result<(), CommandError> {
    let project = state.get_project_state().unwrap();
    let section = &project.sections[0];
    let edited = state.execute(Command::UpdateShot {
        section_id: section.id,
        shot_id: section.shots[0].id,
        updates: ShotUpdates {
            length: Some(42.0),
            ..ShotUpdates::default()
        },
    });
    if edited.success {
        Ok(())
    } else {
        Err(edited.into_error())
    }
}

#[test]
fn an_edit_between_the_adoptions_local_span_and_its_hand_over_is_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let dest = temp.path().join("project.stalx");
    let remote = remote_file(temp.path());
    let state = state_with_null_sink();

    let adopted = state.adopt_import_and_save(
        Some(PROJECT_ID),
        &remote,
        "ariane.tml",
        &dest,
        "CaveA".to_string(),
        |undo_mode| SpeleoLink {
            endpoint: "http://speleodb.invalid/".to_string(),
            project_id: PROJECT_ID.to_string(),
            project_name: "CaveA".to_string(),
            base_commit_sha: Some("abc123".to_string()),
            has_unpushed_edits: true,
            last_edit_at: Some("2026-07-27T10:00:00Z".to_string()),
            last_verify_at: None,
            undo_mode: Some(undo_mode),
        },
    );
    assert!(
        matches!(adopted, Ok((_, Ok(())))),
        "the local half adopts and stamps"
    );

    // The window the hand-over runs in: the adopted project is open.
    let during = try_edit(&state);
    println!("edit after the local span, before the hand-over -> {during:?}");
    assert!(
        during
            .as_ref()
            .is_err_and(|e| e.code == CommandErrorCode::AdoptionInProgress
                && e.code.is_edit_gate()
                && !e.message.contains("EDIT_DISABLED")
                && !e.message.contains("ADOPTION_IN_PROGRESS")),
        "an edit there is refused with the read-only notice, got {during:?}"
    );

    // The adoption ends (its gate guard drops): the gate is the project's again.
    state.speleodb_open_gate_after_adoption();
    state.set_edit_enabled(true);
    let after = try_edit(&state);
    println!("edit once the adoption has finished -> {after:?}");
    assert!(after.is_ok(), "{after:?}");
}

#[test]
fn a_new_open_lifts_the_adoptions_gate() {
    // The gate is the ADOPTED project's: opening another project (a new session
    // generation) is not held by it.
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let state = state_with_null_sink();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "CaveB".to_string(),
                location: None,
            })
            .success
    );
    state.speleodb_shut_gate_for_adoption();
    let other = temp.path().join("other.stalx");
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(other.clone()),
            })
            .success
    );
    assert!(state.open_project(other).success);
    let created = state.execute(Command::CreateSection {
        name: Some("SectionB".to_string()),
        surveyor: None,
        date: None,
        color: "#336699".to_string(),
        direction: None,
        shots: Vec::new(),
    });
    println!(
        "edit in a project opened after the gate was shut -> {:?}",
        created.error
    );
    assert!(created.success, "{:?}", created.error);
}
