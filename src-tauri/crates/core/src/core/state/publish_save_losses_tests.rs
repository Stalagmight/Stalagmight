//! [#576 NF-W4M-2] Publish reports what its saves could not keep.
//!
//! Publish relocates the open project into the SpeleoDB cache with a save, then
//! its initial push runs the pre-push save. Done and Push carry their pre-push
//! save's warnings on their answer (NF-W2SAVE-1); publish dropped both saves'
//! warnings (`&mut Vec::new()` and an ignored `save.warnings`), so a lost table
//! went unreported. The relocation now returns its save's warnings, and the
//! answer (`SpeleoPublishReport`) carries them with the pre-push save's, each code
//! once. All data is synthetic, in a temp dir; nothing reaches a server.

use std::path::Path;

use tempfile::TempDir;

use super::save_sidecars::{fault, DISPLAY_SETTINGS_NOT_SAVED_CODE, UNDO_HISTORY_DROPPED_CODE};
use super::speleodb_project::publish_save_losses;
use super::AppState;
use crate::core::{ValidationWarning, WarningSeverity};
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;

fn local_file(path: &Path) {
    let mut db = Database::open(path).unwrap();
    db.save_project(&Project::new("CaveA".to_string(), None))
        .unwrap();
}

fn lost(code: &str) -> ValidationWarning {
    ValidationWarning {
        code: code.to_owned(),
        message: format!("{code} message"),
        severity: WarningSeverity::Warning,
    }
}

#[test]
fn the_relocation_returns_what_its_save_could_not_keep() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let local = temp.path().join("local.stalx");
    local_file(&local);
    let state = AppState::new().unwrap();
    assert!(state.open_project(local).success);
    let dest = temp.path().join("cache.stalx");

    let _failing = fault::fail_writes_to("display_settings");
    let (link, codes) = state
        .speleodb_publish_relocate(&dest, |undo_mode| SpeleoLink {
            endpoint: "http://speleodb.invalid/".to_owned(),
            project_id: "44444444-4444-4444-4444-444444444444".to_owned(),
            project_name: "CaveA".to_owned(),
            base_commit_sha: None,
            has_unpushed_edits: true,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: Some(undo_mode),
        })
        .map(|(link, lost)| (link, lost.into_iter().map(|w| w.code).collect::<Vec<_>>()))
        .unwrap();
    println!(
        "publish relocation with a failing display-settings write: linked {}, warnings {codes:?}",
        link.project_id
    );
    assert_eq!(codes, vec![DISPLAY_SETTINGS_NOT_SAVED_CODE.to_owned()]);
}

#[test]
fn the_answer_carries_both_saves_losses_each_code_once() {
    let merged = publish_save_losses(
        vec![lost(DISPLAY_SETTINGS_NOT_SAVED_CODE)],
        vec![
            lost(DISPLAY_SETTINGS_NOT_SAVED_CODE),
            lost(UNDO_HISTORY_DROPPED_CODE),
        ],
    );
    let codes: Vec<&str> = merged.iter().map(|w| w.code.as_str()).collect();
    println!("relocation + pre-push losses: {codes:?}");
    assert_eq!(
        codes,
        vec![DISPLAY_SETTINGS_NOT_SAVED_CODE, UNDO_HISTORY_DROPPED_CODE]
    );
}
