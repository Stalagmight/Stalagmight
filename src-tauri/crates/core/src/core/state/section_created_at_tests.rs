//! [#576 NF-W14BE-1] Every save path keeps a section's creation time.
//!
//! `sections.created_at` was overwritten with the save's own time by every save,
//! so it held the last save time. A section's first save now stamps it and every
//! later save writes the stamp back: an implicit save (autosave, the SpeleoDB
//! pre-push flush), a Save As (and the SpeleoDB relocation, the same call), a
//! merge into a new file, and an undo that brings a deleted section back -
//! within the session and from the history persisted in the file.
//!
//! The "first save an hour ago" is injected by backdating the column of a saved
//! file and reopening it, so no test needs a clock. All data is synthetic.

use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use rusqlite::Connection;
use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{CreationStamp, ShotData};
use crate::test_fixtures::state_with_project;

/// `(section name, created_at)` of every row, in id order.
fn created_at(path: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(path).expect("open the saved file");
    let mut stmt = conn
        .prepare("SELECT name, created_at FROM sections ORDER BY id")
        .expect("prepare");
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}

/// Rewrite every section's `created_at` to `at`.
fn backdate(path: &Path, at: &str) {
    let conn = Connection::open(path).expect("open the saved file");
    conn.execute("UPDATE sections SET created_at = ?1", [at])
        .expect("backdate");
}

fn create_section(state: &AppState, name: &str) -> u32 {
    let created = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#123456".to_string(),
        direction: None,
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
    });
    assert!(created.success, "create_section: {:?}", created.error);
    created.section_id.expect("a section id")
}

/// A project with one section, saved to `<dir>/<name>.stalx` and its section
/// stamped an hour ago, then opened in a fresh state. Returns the state, the
/// file and the stamp.
fn opened_an_hour_after_its_first_save(dir: &TempDir, name: &str) -> (AppState, PathBuf, String) {
    let path = dir.path().join(format!("{name}.stalx"));
    let state = state_with_project(name);
    create_section(&state, "SectionA");
    assert!(state.save_project(Some(path.clone())).success);
    drop(state);
    let first_save = (Utc::now() - Duration::hours(1)).to_rfc3339();
    backdate(&path, &first_save);

    let state = AppState::new().expect("state");
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open: {:?}", opened.error);
    (state, path, first_save)
}

#[test]
fn two_saves_in_one_session_keep_a_new_sections_first_stamp() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("CaveA.stalx");
    let state = state_with_project("CaveA");
    create_section(&state, "SectionA");

    assert!(state.save_project(Some(path.clone())).success);
    let first = created_at(&path);
    // The implicit save: autosave and the SpeleoDB pre-push flush.
    let again = state.execute(Command::SaveProject { path: None });
    assert!(again.success, "implicit save: {:?}", again.error);
    let second = created_at(&path);

    println!("NF-W14BE-1 first save:  {first:?}");
    println!("NF-W14BE-1 second save: {second:?}");
    assert_eq!(second, first);
}

#[test]
fn an_implicit_save_of_a_reopened_project_keeps_the_stored_created_at() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let (state, path, first_save) = opened_an_hour_after_its_first_save(&dir, "CaveA");
    create_section(&state, "SectionB");

    let before = Utc::now();
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "implicit save: {:?}", saved.error);
    let after = Utc::now();

    let rows = created_at(&path);
    println!("NF-W14BE-1 first save stamped SectionA {first_save}");
    println!("NF-W14BE-1 after adding SectionB and saving: {rows:?}");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("SectionA".to_string(), first_save), "SectionA");
    let stamped_b = chrono::DateTime::parse_from_rfc3339(&rows[1].1)
        .expect("an RFC 3339 stamp")
        .with_timezone(&Utc);
    assert!(
        before <= stamped_b && stamped_b <= after,
        "SectionB is stamped by its first save: {stamped_b} not in [{before}, {after}]"
    );
}

#[test]
fn save_as_to_a_new_file_keeps_the_existing_sections_created_at() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let (state, _path, first_save) = opened_an_hour_after_its_first_save(&dir, "CaveA");

    let copy = dir.path().join("CaveA-copy.stalx");
    let saved = state.save_project(Some(copy.clone()));
    assert!(saved.success, "save as: {:?}", saved.error);

    let rows = created_at(&copy);
    println!("NF-W14BE-1 first save stamped SectionA {first_save}");
    println!("NF-W14BE-1 the Save As copy holds: {rows:?}");
    assert_eq!(rows, vec![("SectionA".to_string(), first_save)]);
}

#[test]
fn undoing_a_section_delete_across_saves_keeps_its_created_at() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let (state, path, first_save) = opened_an_hour_after_its_first_save(&dir, "CaveA");
    let section_id = state.get_project_state().expect("open").sections[0].id;

    assert!(state.execute(Command::DeleteSection { section_id }).success);
    assert!(state.save_project(None).success);
    assert!(created_at(&path).is_empty(), "the delete was saved");

    // In the same session.
    assert!(state.undo().success, "undo the delete");
    assert!(state.save_project(None).success);
    let in_session = created_at(&path);

    // And from the history persisted in the file: delete again, save, reopen.
    assert!(state.redo().success, "redo the delete");
    assert!(state.save_project(None).success);
    drop(state);
    let state = AppState::new().expect("state");
    assert!(state.open_project(path.clone()).success);
    assert!(state.undo().success, "undo the delete after a reopen");
    assert!(state.save_project(None).success);
    let after_reopen = created_at(&path);

    let expected = vec![("SectionA".to_string(), first_save.clone())];
    println!("NF-W14BE-1 first save stamped SectionA {first_save}");
    println!("NF-W14BE-1 undo of the delete, saved: {in_session:?}");
    println!("NF-W14BE-1 undo of the delete after a reopen, saved: {after_reopen:?}");
    assert_eq!(in_session, expected, "undo within the session");
    assert_eq!(after_reopen, expected, "undo from the persisted history");
}

#[test]
fn a_merge_keeps_each_source_sections_created_at() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let mut sources = Vec::new();
    let mut stamps = Vec::new();
    for (name, hours) in [("CaveA", 2), ("CaveB", 1)] {
        let path = dir.path().join(format!("{name}.stalx"));
        let state = state_with_project(name);
        create_section(&state, &format!("Section{name}"));
        assert!(state.save_project(Some(path.clone())).success);
        drop(state);
        let stamp = (Utc::now() - Duration::hours(hours)).to_rfc3339();
        backdate(&path, &stamp);
        stamps.push((format!("Section{name}"), stamp));
        sources.push(path);
    }

    let output = dir.path().join("Merged.stalx");
    let state = AppState::new().expect("state");
    let merged = state.merge_projects(sources, output.clone(), "Merged".to_string(), false);
    assert!(merged.success, "merge: {:?}", merged.error);

    let rows = created_at(&output);
    println!("NF-W14BE-1 the sources stamped {stamps:?}");
    println!("NF-W14BE-1 the merged file holds: {rows:?}");
    assert_eq!(rows, stamps);
}

/// A snapshot recorded before a section's first save holds no creation time; a
/// restore of it takes the one the save stamped on the live section, and never
/// replaces a time the restored section already has.
#[test]
fn a_restore_carries_the_live_sections_creation_time_forward() {
    let mut live = crate::models::Project::new("CaveA".to_string(), None);
    let first = live
        .add_section(crate::models::SectionData {
            name: Some("SectionA".to_string()),
            color: "#123456".to_string(),
            ..Default::default()
        })
        .id;
    let second = live
        .add_section(crate::models::SectionData {
            name: Some("SectionB".to_string()),
            color: "#123456".to_string(),
            ..Default::default()
        })
        .id;
    let mut restored = live.clone();
    let own = Utc::now() - Duration::hours(2);
    restored.sections[1].created_at = Some(CreationStamp::at(own));

    let stamped = Utc::now() - Duration::hours(1);
    super::project_io::stamp_section_creation_times(&mut live, stamped);
    super::project_io::reapply_heals_after_restore(&mut restored, Some(&live));

    let at = |id| {
        restored
            .get_section(id)
            .expect("restored")
            .created_at
            .clone()
    };
    assert_eq!(
        at(first),
        Some(CreationStamp::at(stamped)),
        "carried forward"
    );
    assert_eq!(
        at(second),
        Some(CreationStamp::at(own)),
        "its own time is kept"
    );
}

/// [#576 RV14-2] A stamp in another offset (another tool's) is written back as it
/// was stored by every save path, an undo from the persisted history included:
/// it was read as a UTC time and printed as one, the same moment in another text.
#[test]
fn a_stamp_in_another_offset_is_written_back_as_stored() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let stored = "2020-01-02T03:04:05+05:00";
    let (state, path, _) = opened_an_hour_after_its_first_save(&dir, "CaveA");
    drop(state);
    backdate(&path, stored);
    let state = AppState::new().expect("state");
    assert!(state.open_project(path.clone()).success);
    let section_id = state.get_project_state().expect("open").sections[0].id;

    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "implicit save: {:?}", saved.error);
    let implicit = created_at(&path);

    // An undo that brings the section back, from the history persisted in the file.
    assert!(state.execute(Command::DeleteSection { section_id }).success);
    assert!(state.save_project(None).success);
    drop(state);
    let state = AppState::new().expect("state");
    assert!(state.open_project(path.clone()).success);
    assert!(state.undo().success, "undo the delete after a reopen");
    assert!(state.save_project(None).success);
    let after_undo = created_at(&path);

    let copy = dir.path().join("CaveA-copy.stalx");
    assert!(state.save_project(Some(copy.clone())).success, "save as");
    let save_as = created_at(&copy);

    let expected = vec![("SectionA".to_string(), stored.to_string())];
    println!("RV14-2 stored {stored:?}");
    println!("RV14-2 implicit save: {implicit:?}");
    println!("RV14-2 Save As: {save_as:?}");
    println!("RV14-2 undo of a delete after a reopen, saved: {after_undo:?}");
    assert_eq!(implicit, expected, "implicit save");
    assert_eq!(save_as, expected, "save as");
    assert_eq!(after_undo, expected, "undo from the persisted history");
}

/// [#576 NF-W18-1] A whole-project snapshot persisted before the project's own
/// creation time was kept as text holds it as `DateTime<Utc>` serialized it (a
/// `Z`). A restore keeps the text the live project holds - its file's - when the
/// snapshot names the same moment, so an undo and a save do not write it in that
/// other form; a snapshot naming another moment keeps its own.
#[test]
fn a_restore_keeps_the_live_projects_created_at_text() {
    let stored = "2020-01-02T03:04:05+05:00";
    let mut live = crate::models::Project::new("CaveA".to_string(), None);
    live.created_at = CreationStamp::parse(stored).expect("stamp");
    let mut restored = live.clone();
    restored.created_at = CreationStamp::parse("2020-01-01T22:04:05Z").expect("stamp");
    let other = CreationStamp::parse("2021-01-01T00:00:00Z").expect("stamp");
    let mut elsewhere = live.clone();
    elsewhere.created_at = other.clone();

    super::project_io::reapply_heals_after_restore(&mut restored, Some(&live));
    super::project_io::reapply_heals_after_restore(&mut elsewhere, Some(&live));
    println!(
        "NF-W18-1 live {stored:?}; restored from a snapshot holding \"2020-01-01T22:04:05Z\": \
         {:?}; from one holding another moment: {:?}",
        restored.created_at.as_str(),
        elsewhere.created_at.as_str()
    );
    assert_eq!(restored.created_at.as_str(), stored, "the live text");
    assert_eq!(
        elsewhere.created_at, other,
        "another moment is the snapshot's own"
    );
}
