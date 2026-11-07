//! [#576 RV2-R-7] A Save As away from a file that could not be reopened says what
//! the new file is missing.
//!
//! When the open file's database handle is gone and the file cannot be reopened,
//! a Save As to ANOTHER file is the user's way out: it goes ahead without the
//! handle. The side tables only the open file holds - the ignored warnings, the
//! SpeleoDB link, the undo-history markers - are then not in the new file, and
//! the save said nothing: the three tables that otherwise STOP a save were lost
//! without a word. It now succeeds with a `SIDE_TABLES_NOT_CARRIED` warning.
//!
//! All data is synthetic: a project this build saves, its handle dropped and the
//! file removed so the reopen fails.

use tempfile::TempDir;

use crate::core::Command;
use crate::models::ShotData;
use crate::persistence::ResolvedWarning;
use crate::test_fixtures::state_with_project;

#[test]
fn a_save_as_away_from_an_unreadable_file_warns_about_the_side_tables() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("CaveR.stalx");
    let state = state_with_project("CaveR");
    assert!(
        state
            .execute(Command::CreateSection {
                name: Some("SectionR".to_string()),
                surveyor: None,
                date: None,
                color: "#123456".to_string(),
                direction: None,
                shots: vec![ShotData {
                    length: 5.0,
                    ..Default::default()
                }],
            })
            .success
    );
    assert!(state.save_project(Some(path.clone())).success);
    let project = state.get_project_state().expect("project");
    let shot = &project.sections[0].shots[0];
    state
        .save_resolved_warning(&ResolvedWarning {
            section_id: project.sections[0].id,
            shot_id: shot.id,
            warning_code: "LONG_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: Some("checked".to_string()),
            shot_uuid: shot.uuid.clone(),
        })
        .expect("ignore a warning");

    // The handle is gone and the open file cannot be reopened.
    *state.database.lock() = None;
    std::fs::remove_file(&path).expect("remove the open file");

    let elsewhere = dir.path().join("CaveR-copy.stalx");
    let saved = state.save_project(Some(elsewhere.clone()));
    let codes: Vec<&str> = saved.warnings.iter().map(|w| w.code.as_str()).collect();
    println!(
        "Save As away from the unreadable file: success={} warnings={codes:?} written={}",
        saved.success,
        elsewhere.exists()
    );
    assert!(saved.success, "{:?}", saved.error);
    let warning = saved
        .warnings
        .iter()
        .find(|w| w.code == "SIDE_TABLES_NOT_CARRIED")
        .expect("the lost side tables are reported");
    println!("the warning: {}", warning.message);
    assert!(warning.message.contains("ignored warnings"));
}
