//! [#576 W25-NF1] A link planted at a save's or a merge's temp name after the
//! name was cleared and before the temp database was opened there is refused,
//! never followed; and a save or a merge into a folder reached through a link
//! still writes there.
//!
//! The clear (`clear_save_temp`) removes any link at the temp name, but SQLite
//! opened the name by itself afterwards, resolving a link at it - so one planted
//! in between took the whole project wherever it pointed: a new file for a link
//! that led nowhere yet, another project overwritten for a link to one. The link
//! goes in through `AFTER_TEMP_CLEARED`, exactly in that window.
//!
//! All data is synthetic, built in code; every file lives in a `TempDir`.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use super::fuzz_undo_tests::Rng;
use super::persist_roundtrip_fuzz_tests::Session;
use super::project_io::AFTER_TEMP_CLEARED;
use crate::core::CommandResult;
use crate::models::Project;
use crate::persistence::Database;
use crate::test_helpers::isolated_config;

/// A saved project named `name` at `path`.
fn project_at(path: &Path, name: &str) -> PathBuf {
    Database::open(path)
        .expect("create the project")
        .save_project(&Project::new(name.to_owned(), None))
        .expect("write the project");
    path.to_path_buf()
}

/// What a planted link leads to: a file not there yet, or another project, in a
/// folder of its own. Returns the target and its bytes before anything runs.
fn target_in(elsewhere: &Path, existing: bool) -> (PathBuf, Option<Vec<u8>>) {
    let target = if existing {
        project_at(&elsewhere.join("Other.stalx"), "Other")
    } else {
        elsewhere.join("planted.stalx")
    };
    let before = std::fs::read(&target).ok();
    (target, before)
}

/// Plant a link at the temp name to `target` once, right after it is cleared.
fn plant_after_clear(target: PathBuf) {
    AFTER_TEMP_CLEARED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |temp: &Path| {
            symlink(&target, temp).expect("plant the link");
        }));
    });
}

/// What is wrong after a write that had to refuse the link planted at its temp
/// name: the link's target was written (created, or its bytes changed), or the
/// write succeeded, or the seam never ran.
fn refused_over_the_link(
    what: &str,
    result: &CommandResult,
    target: &Path,
    before: Option<&[u8]>,
) -> Vec<String> {
    let after = std::fs::read(target).ok();
    println!(
        "{what}: success {} error {:?}; the link's target was {:?} bytes, now {:?}",
        result.success,
        result.error,
        before.map(<[u8]>::len),
        after.as_ref().map(Vec::len)
    );
    let mut problems = Vec::new();
    if AFTER_TEMP_CLEARED
        .with(|hook| hook.borrow_mut().take())
        .is_some()
    {
        problems.push(format!("{what}: the temp name was never cleared"));
    }
    if after.as_deref() != before {
        problems.push(format!(
            "{what}: written through the link at the temp name ({:?} bytes before, {:?} after)",
            before.map(<[u8]>::len),
            after.as_ref().map(Vec::len)
        ));
    }
    if result.success {
        problems.push(format!("{what}: succeeded over a link at its temp name"));
    }
    problems
}

#[test]
fn a_link_planted_at_the_temp_name_after_the_clear_is_not_followed_by_the_save() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    for existing in [false, true] {
        let session = Session::new(&mut Rng::new(3));
        let elsewhere = tempfile::TempDir::new().expect("a folder the save must not write");
        let (target, before) = target_in(elsewhere.path(), existing);
        plant_after_clear(target.clone());

        let saved = session
            .state
            .save_project(Some(session.dir.path().join("CaveA.stalx")));

        problems.extend(refused_over_the_link(
            &format!("save (existing {existing})"),
            &saved,
            &target,
            before.as_deref(),
        ));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn a_link_planted_at_the_merge_temp_name_after_the_clear_is_not_followed() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    for existing in [false, true] {
        let sources_dir = tempfile::TempDir::new().expect("the sources' folder");
        let sources = vec![
            project_at(&sources_dir.path().join("CaveA.stalx"), "CaveA"),
            project_at(&sources_dir.path().join("CaveB.stalx"), "CaveB"),
        ];
        let elsewhere = tempfile::TempDir::new().expect("a folder the merge must not write");
        let output_dir = tempfile::TempDir::new().expect("the output's folder");
        let (target, before) = target_in(elsewhere.path(), existing);
        let state = super::AppState::new().expect("state");
        plant_after_clear(target.clone());

        let merged = state.merge_projects(
            sources,
            output_dir.path().join("Merged.stalx"),
            "Merged".to_owned(),
            false,
        );

        problems.extend(refused_over_the_link(
            &format!("merge (existing {existing})"),
            &merged,
            &target,
            before.as_deref(),
        ));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The other half of the contract: a folder reached through a link is a place a
/// save and a merge may go, and both still write there.
#[test]
fn a_save_and_a_merge_into_a_folder_reached_through_a_link_still_write_there() {
    let _config = isolated_config();
    let session = Session::new(&mut Rng::new(3));
    let real = session.dir.path().join("real");
    std::fs::create_dir(&real).expect("the real folder");
    let linked = session.dir.path().join("linked");
    symlink(&real, &linked).expect("a link to the folder");

    let saved = session.state.save_project(Some(linked.join("CaveA.stalx")));
    let sources_dir = tempfile::TempDir::new().expect("the sources' folder");
    let merged = super::AppState::new().expect("state").merge_projects(
        vec![
            project_at(&sources_dir.path().join("CaveB.stalx"), "CaveB"),
            project_at(&sources_dir.path().join("CaveC.stalx"), "CaveC"),
        ],
        linked.join("Merged.stalx"),
        "Merged".to_owned(),
        false,
    );

    println!(
        "into a linked folder: save success {} error {:?}; merge success {} error {:?}",
        saved.success, saved.error, merged.success, merged.error
    );
    assert!(saved.success, "the save failed: {:?}", saved.error);
    assert!(merged.success, "the merge failed: {:?}", merged.error);
    for name in ["CaveA.stalx", "Merged.stalx"] {
        let file = real.join(name);
        assert!(
            std::fs::symlink_metadata(&file).is_ok_and(|meta| meta.file_type().is_file()),
            "{name} is not a file in the real folder"
        );
        Database::open_existing(&file)
            .expect("the written file opens")
            .load_project()
            .expect("the written file loads");
    }
    assert!(
        std::fs::symlink_metadata(&linked).is_ok_and(|meta| meta.file_type().is_symlink()),
        "the folder link is still a link"
    );
}
