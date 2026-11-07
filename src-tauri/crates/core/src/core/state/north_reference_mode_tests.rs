//! [#576 NF-W3-2-PLAN] The default north reference is a survey edit: Survey only.
//!
//! It turns every section that declares no north reference of its own, so the
//! map, the closures and every exported bearing move with it - the same kind of
//! change as the entrance elevation, which Plan and Export refuse. The project
//! name and the validation thresholds stay on the read-only-workspace allowlist
//! (`update_project_settings`); the reference that rides the same save does not:
//! changing it there is refused like every other survey edit, and an unread one
//! is not set (lifted) from Plan or Export either.
//!
//! All data is synthetic, built in memory or in a temp dir.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{AppState, WorkspaceMode};
use crate::models::CommandErrorCode;
use crate::models::{AzimuthReference, CorrectionSource, Project, SectionData, ShotData};
use crate::persistence::Database;

fn magnetic(degrees: f64) -> AzimuthReference {
    AzimuthReference::magnetic(degrees, CorrectionSource::Manual)
}

/// A local project saved to a temp `.stalx` and opened (Survey mode).
fn open_local(dir: &Path, break_settings: bool) -> AppState {
    let path: PathBuf = dir.join("CaveN.stalx");
    let mut project = Project::new("CaveN".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionN".to_string()),
        shots: vec![ShotData {
            length: 4.0,
            azimuth_in: 30.0,
            azimuth_out: 210.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    if break_settings {
        // The open then reads none of the stored project settings.
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("ALTER TABLE project_settings RENAME COLUMN value TO broken_value;")
            .unwrap();
    }
    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success, "open the fixture");
    state
}

/// Changing the default north reference in Plan or Export is refused with the
/// workspace's read-only code, and NOTHING of that save lands - not the
/// reference, and not the name that rode along with it.
#[test]
fn a_changed_north_reference_is_refused_in_plan_and_export() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let state = open_local(dir.path(), false);
    for (mode, degrees) in [(WorkspaceMode::Plan, 3.0), (WorkspaceMode::Export, 4.0)] {
        state.set_workspace_mode(mode);
        let live = state.get_project_state().unwrap();
        let set = state.update_project_settings(
            format!("{}-renamed", live.name),
            live.validation,
            Some(magnetic(degrees)),
        );
        let after = state.get_project_state().unwrap();
        println!(
            "{mode:?}: success={} error={:?} reference after={:?} name after={}",
            set.success, set.error, after.default_azimuth_reference, after.name
        );
        assert!(
            !set.success,
            "{mode:?}: a changed north reference is a survey edit"
        );
        let error = set.error.unwrap_or_default();
        assert_eq!(
            set.code,
            Some(CommandErrorCode::WorkspaceReadOnly),
            "{mode:?}: refused as a read-only workspace: {error:?}"
        );
        assert!(
            !error.contains("WORKSPACE_READ_ONLY"),
            "{mode:?}: no marker in the text: {error:?}"
        );
        assert_eq!(after.default_azimuth_reference, None, "{mode:?}: unchanged");
        assert_eq!(after.name, live.name, "{mode:?}: the whole save is refused");
    }
}

/// The name and the thresholds stay editable in Plan and Export when the
/// reference rides along unchanged, and Survey can change the reference.
#[test]
fn the_name_stays_editable_in_plan_and_the_reference_in_survey() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let state = open_local(dir.path(), false);
    state.set_workspace_mode(WorkspaceMode::Plan);
    let live = state.get_project_state().unwrap();
    let renamed = state.update_project_settings("CaveRenamed".to_string(), live.validation, None);
    assert!(renamed.success, "{:?}", renamed.error);
    assert_eq!(state.get_project_state().unwrap().name, "CaveRenamed");

    state.set_workspace_mode(WorkspaceMode::Survey);
    let live = state.get_project_state().unwrap();
    let set = state.update_project_settings(live.name, live.validation, Some(magnetic(5.0)));
    assert!(set.success, "{:?}", set.error);
    assert_eq!(
        state.get_project_state().unwrap().default_azimuth_reference,
        Some(magnetic(5.0))
    );
}

/// An UNREAD north reference is not set from Plan or Export either: the save
/// there lifts the thresholds the user kept, but the reference stays unread (so
/// saves stay refused) until it is set in Survey - the entrance elevation's rule.
#[test]
fn an_unread_north_reference_is_only_set_in_survey() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let state = open_local(dir.path(), true);
    println!(
        "unreadable after open: {:?}",
        state.unreadable_project_settings()
    );
    state.set_workspace_mode(WorkspaceMode::Plan);
    let live = state.get_project_state().unwrap();
    let kept = state.update_project_settings(
        live.name.clone(),
        live.validation.clone(),
        live.default_azimuth_reference,
    );
    let left = state.unreadable_project_settings();
    println!(
        "Plan save: success={} error={:?} unreadable after={left:?}",
        kept.success, kept.error
    );
    assert!(kept.success, "{:?}", kept.error);
    assert!(
        !left.contains(&"validation".to_string()),
        "the thresholds are on the allowlist: {left:?}"
    );
    assert!(
        left.contains(&"default_azimuth_reference".to_string()),
        "the north reference is a survey edit, not set from Plan: {left:?}"
    );

    state.set_workspace_mode(WorkspaceMode::Survey);
    let set =
        state.update_project_settings(live.name, live.validation, live.default_azimuth_reference);
    assert!(set.success, "{:?}", set.error);
    let left = state.unreadable_project_settings();
    println!("Survey save: unreadable after={left:?}");
    assert!(!left.contains(&"default_azimuth_reference".to_string()));
}
