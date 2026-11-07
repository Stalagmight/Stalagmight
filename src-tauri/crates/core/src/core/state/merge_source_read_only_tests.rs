//! [#576 RV6b-R1-D1] A merge reads its sources and never writes them.
//!
//! Opening an older-format project upgrades it in place and keeps a
//! `.pre-v<N>.backup` copy beside it first. A merge used that same open for every
//! source, so merging an older project rewrote it and left a backup copy in its
//! folder - wherever that folder was (the app's own folders and system folders
//! included), and with "archive originals" off, which is when the user asked for
//! the sources to stay as they were. The source is now read in memory only: its
//! upgraded contents go into the merged project, and the source file and its
//! folder are left byte-for-byte as they were.
//!
//! All data is synthetic: projects built in memory, saved by this build, one of
//! them re-stamped as an older file. No fixture is read or written.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;

/// Re-stamps a current file as one an older build saved (the open upgrades it).
const OLDER_FORMAT: &str = "UPDATE project SET schema_version = 16; PRAGMA user_version = 16;";

/// A project file at `dir/name` with one section of `lengths`, re-stamped by
/// `surgery` (empty: left current).
fn write_project(dir: &Path, name: &str, section: &str, lengths: &[f64], surgery: &str) -> PathBuf {
    let path = dir.join(name);
    let mut project = Project::new(name.trim_end_matches(".stalx").to_owned(), None);
    project.add_section(SectionData {
        name: Some(section.to_owned()),
        color: "#123456".to_owned(),
        shots: lengths
            .iter()
            .map(|&length| ShotData {
                length,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    Database::open(&path)
        .expect("create")
        .save_project(&project)
        .expect("write");
    if !surgery.is_empty() {
        rusqlite::Connection::open(&path)
            .expect("reopen")
            .execute_batch(surgery)
            .expect("re-stamp");
    }
    path
}

/// The files in `dir`, by name (a backup copy or a journal would show up here).
fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn merging_an_older_project_leaves_the_source_and_its_folder_untouched() {
    let _cfg = crate::test_helpers::isolated_config();
    let older_dir = TempDir::new().expect("dir");
    let current_dir = TempDir::new().expect("dir");
    let out_dir = TempDir::new().expect("dir");
    let older = write_project(
        older_dir.path(),
        "CaveOld.stalx",
        "SectionOld",
        &[5.0, 7.0],
        OLDER_FORMAT,
    );
    let current = write_project(
        current_dir.path(),
        "CaveNew.stalx",
        "SectionNew",
        &[3.0],
        "",
    );
    let older_before = std::fs::read(&older).expect("read");
    let current_before = std::fs::read(&current).expect("read");
    let listing_before = (files_in(older_dir.path()), files_in(current_dir.path()));
    let output = out_dir.path().join("Merged.stalx");

    let state = AppState::new().expect("state");
    let merged = state.merge_projects(
        vec![older.clone(), current.clone()],
        output.clone(),
        "Merged".to_owned(),
        false,
    );
    let older_after = std::fs::read(&older).expect("read");
    let current_after = std::fs::read(&current).expect("read");
    let listing_after = (files_in(older_dir.path()), files_in(current_dir.path()));
    let sections: Vec<(Option<String>, usize)> = Database::open_existing(&output)
        .and_then(|db| db.load_project())
        .map(|p| {
            p.sections
                .iter()
                .map(|s| (s.name.clone(), s.shots.len()))
                .collect()
        })
        .unwrap_or_default();
    println!(
        "merge of an older + a current project: success={} error={:?}; merged sections: \
         {sections:?}; older source unchanged: {}; current source unchanged: {}; \
         source folders before: {listing_before:?} after: {listing_after:?}",
        merged.success,
        merged.error,
        older_after == older_before,
        current_after == current_before,
    );
    assert!(merged.success, "the merge goes through: {:?}", merged.error);
    assert_eq!(
        sections,
        [
            (Some("SectionOld".to_owned()), 2),
            (Some("SectionNew".to_owned()), 1)
        ],
        "both sources' sections are in the merged project"
    );
    assert_eq!(
        listing_after, listing_before,
        "no backup copy (or anything else) beside a source"
    );
    assert!(
        older_after == older_before,
        "the older source is not upgraded in place"
    );
    assert!(
        current_after == current_before,
        "the current source is not written"
    );
}
