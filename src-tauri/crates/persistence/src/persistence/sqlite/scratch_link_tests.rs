//! [#576 W25-NF1] A link planted at a scratch name after the name was cleared
//! and before SQLite opened it - a push snapshot, a push compaction's staging
//! copy, a repair's staging copy - is refused, never followed; and scratch files
//! in a folder reached through a link, of a project opened through a link, are
//! written as before.
//!
//! The link goes in through `AFTER_SCRATCH_CLEARED`, between
//! `clear_scratch_name` and the `VACUUM INTO` that writes the name: exactly where
//! a concurrent writer in the folder would put it. All data is synthetic, built in
//! code; every file lives in a `TempDir`.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::repair::{sibling, AFTER_SCRATCH_CLEARED, REPAIR_STAGING_SUFFIX};
use super::Database;
use crate::models::{Project, SectionData, ShotData};

/// A saved project of one section of two shots at `path`.
fn project_at(path: &Path) -> PathBuf {
    let mut project = Project::new("CaveA".to_owned(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_owned()),
        color: "#123456".to_owned(),
        shots: vec![
            ShotData {
                length: 5.0,
                ..Default::default()
            },
            ShotData {
                length: 3.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    Database::open(path)
        .expect("create the project")
        .save_project(&project)
        .expect("write the project");
    path.to_path_buf()
}

/// While alive, plants a link at `at` to `to` the first time `clear_scratch_name`
/// clears `at`; dropped, removes the seam.
struct PlantAfterClear;

impl PlantAfterClear {
    fn new(at: PathBuf, to: PathBuf) -> Self {
        let mut planted = false;
        AFTER_SCRATCH_CLEARED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |cleared: &Path| {
                if !planted && cleared == at {
                    symlink(&to, &at).expect("plant the link");
                    planted = true;
                }
            }));
        });
        Self
    }
}

impl Drop for PlantAfterClear {
    fn drop(&mut self) {
        AFTER_SCRATCH_CLEARED.with(|hook| hook.borrow_mut().take());
    }
}

/// What a planted link leads to: a file not there yet (`VACUUM INTO` creates it),
/// or an empty file (`VACUUM INTO` writes into an empty one), in a folder of its own.
fn targets(elsewhere: &Path) -> [(PathBuf, Option<u64>); 2] {
    let empty = elsewhere.join("empty.toml");
    std::fs::write(&empty, b"").expect("an empty file");
    [(elsewhere.join("planted.toml"), None), (empty, Some(0))]
}

/// The size of what is at `path`, following links, or `None`.
fn size_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|meta| meta.len())
}

/// What is wrong after a write that had to refuse the link planted at its name.
fn refused_over_the_link(
    what: &str,
    result: &anyhow::Result<impl std::fmt::Debug>,
    target: &Path,
    before: Option<u64>,
) -> Vec<String> {
    let after = size_of(target);
    println!(
        "{what}: result {result:?}; the link's target {:?} was {before:?} bytes, now {after:?}",
        target.file_name()
    );
    let mut problems = Vec::new();
    if after != before {
        problems.push(format!(
            "{what}: written through the link ({before:?} bytes before, {after:?} after)"
        ));
    }
    if result.is_ok() {
        problems.push(format!("{what}: succeeded over a link at its scratch name"));
    }
    problems
}

/// Whether `path` is a regular file, not a link.
fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}

#[test]
fn a_snapshot_never_follows_a_link_planted_at_its_name_after_the_clear() {
    let mut problems = Vec::new();
    let dir = TempDir::new().expect("dir");
    let elsewhere = TempDir::new().expect("a folder the snapshot must not write");
    let live = project_at(&dir.path().join("CaveA.stalx"));
    for (target, before) in targets(elsewhere.path()) {
        let dest = dir.path().join("push").join("snapshot.stalx");
        let _seam = PlantAfterClear::new(dest.clone(), target.clone());
        let result = Database::vacuum_snapshot(&live, &dest);
        problems.extend(refused_over_the_link("snapshot", &result, &target, before));
        let _ = std::fs::remove_file(&dest);
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn a_compaction_never_follows_a_link_planted_at_its_staging_name_after_the_clear() {
    let mut problems = Vec::new();
    let dir = TempDir::new().expect("dir");
    let elsewhere = TempDir::new().expect("a folder the compaction must not write");
    let live = project_at(&dir.path().join("CaveA.stalx"));
    for (target, before) in targets(elsewhere.path()) {
        let snapshot = dir.path().join("snapshot.stalx");
        let _ = std::fs::remove_file(&snapshot);
        Database::vacuum_snapshot(&live, &snapshot).expect("snapshot");
        let staging = sibling(&snapshot, ".vacuum.tmp");
        let _seam = PlantAfterClear::new(staging.clone(), target.clone());
        let result = Database::sanitize_push_snapshot(&snapshot, false, None);
        problems.extend(refused_over_the_link(
            "compaction",
            &result,
            &target,
            before,
        ));
        if !is_regular_file(&snapshot) {
            problems.push(format!(
                "compaction: the snapshot is no longer a file of its own (the link was \
                 renamed over it): {:?}",
                std::fs::symlink_metadata(&snapshot).map(|meta| meta.file_type())
            ));
        }
        let _ = std::fs::remove_file(&staging);
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// [#576 W42-NF1] The compaction's refusal of that link says it refused a link,
/// worded as the write-path policy words a refusal; it said "most likely not
/// enough free disk space", the guess for every failed `VACUUM INTO`. Any other
/// failure there (here: a file put at the staging name since the clear) keeps
/// that guess. What a push shows is the outermost message (`{e}`).
#[test]
fn a_compaction_refused_over_a_link_says_so_not_that_the_disk_is_full() {
    let mut problems = Vec::new();
    let dir = TempDir::new().expect("dir");
    let elsewhere = TempDir::new().expect("a folder the compaction must not write");
    let live = project_at(&dir.path().join("CaveA.stalx"));
    let snapshot = dir.path().join("snapshot.stalx");
    let staging = sibling(&snapshot, ".vacuum.tmp");

    Database::vacuum_snapshot(&live, &snapshot).expect("snapshot");
    let over_link = {
        let _seam = PlantAfterClear::new(staging.clone(), elsewhere.path().join("planted.toml"));
        Database::sanitize_push_snapshot(&snapshot, false, None)
            .expect_err("the compaction refuses the link")
    };
    let shown = over_link.to_string();
    println!("[W42-NF1] over a link: {shown:?}\n  chain: {over_link:#}");
    if shown.contains("disk space") {
        problems.push(format!("the link refusal claims low disk space: {shown:?}"));
    }
    if !shown.starts_with("Refused to change snapshot.stalx: ") || !shown.contains("is a link") {
        problems.push(format!("the link refusal does not say so: {shown:?}"));
    }

    let _ = std::fs::remove_file(&staging);
    let _ = std::fs::remove_file(&snapshot);
    Database::vacuum_snapshot(&live, &snapshot).expect("snapshot");
    AFTER_SCRATCH_CLEARED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |cleared: &Path| {
            if cleared == staging {
                std::fs::write(cleared, b"not a database").expect("a file at the staging name");
            }
        }));
    });
    let other = Database::sanitize_push_snapshot(&snapshot, false, None);
    AFTER_SCRATCH_CLEARED.with(|hook| hook.borrow_mut().take());
    let other = other.expect_err("the compaction fails over a file at its staging name");
    let shown = other.to_string();
    println!("[W42-NF1] over a file: {shown:?}\n  chain: {other:#}");
    if !shown.contains("most likely not enough free disk space") {
        problems.push(format!(
            "another failure lost the disk-space wording: {shown:?}"
        ));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn a_repair_never_follows_a_link_planted_at_its_staging_name_after_the_clear() {
    let mut problems = Vec::new();
    let dir = TempDir::new().expect("dir");
    let elsewhere = TempDir::new().expect("a folder the repair must not write");
    for (target, before) in targets(elsewhere.path()) {
        let path = project_at(&dir.path().join(format!("CaveA-{}.stalx", before.is_some())));
        let staging = sibling(&path, REPAIR_STAGING_SUFFIX);
        let _seam = PlantAfterClear::new(staging.clone(), target.clone());
        let result = Database::attempt_repair(&path);
        problems.extend(refused_over_the_link("repair", &result, &target, before));
        if !is_regular_file(&path) {
            problems.push(format!(
                "repair: the project is no longer a file of its own (the link was renamed \
                 over it): {:?}",
                std::fs::symlink_metadata(&path).map(|meta| meta.file_type())
            ));
        }
        let _ = std::fs::remove_file(&staging);
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The other half of the contract: a folder reached through a link, and a project
/// file that is itself a link, are where these writes always went. Each scratch
/// write still lands in the real folder, and every link stays a link.
#[test]
fn scratch_files_in_a_folder_reached_through_a_link_are_written_as_before() {
    let root = TempDir::new().expect("dir");
    let real = root.path().join("real");
    std::fs::create_dir(&real).expect("the real folder");
    let linked = root.path().join("linked");
    symlink(&real, &linked).expect("a link to the folder");
    let live = project_at(&linked.join("CaveA.stalx"));
    let live_link = root.path().join("CaveA-link.stalx");
    symlink(&live, &live_link).expect("a link to the project");

    // A push snapshot of a project opened through a link, into a linked folder.
    let snapshot = linked.join("push").join("snapshot.stalx");
    Database::vacuum_snapshot(&live_link, &snapshot).expect("snapshot through links");
    // Its compaction, staged beside it.
    Database::sanitize_push_snapshot(&snapshot, false, None).expect("compaction through a link");
    // A repair of a project opened through a link, staged beside the file.
    let (_, ok) = Database::attempt_repair(&live_link).expect("repair through links");
    // A save's temp in a linked folder.
    let temp = linked.join("CaveB.stalx.tmp");
    Database::open_scratch(&temp)
        .expect("a temp in a linked folder")
        .save_project(&Project::new("CaveB".to_owned(), None))
        .expect("write the temp");

    let snapshot_project = Database::open_existing(&real.join("push").join("snapshot.stalx"))
        .expect("the snapshot opens")
        .load_project()
        .expect("the snapshot loads");
    println!(
        "through links: snapshot sections {}, repair integrity {ok}, temp {}",
        snapshot_project.sections.len(),
        is_regular_file(&real.join("CaveB.stalx.tmp"))
    );
    assert_eq!(
        snapshot_project.sections.len(),
        1,
        "the snapshot holds the project"
    );
    assert!(ok, "the repaired project passes its integrity check");
    assert!(
        is_regular_file(&real.join("CaveB.stalx.tmp")),
        "the temp is in the real folder"
    );
    assert!(
        std::fs::symlink_metadata(&linked).is_ok_and(|m| m.file_type().is_symlink()),
        "the folder link is still a link"
    );
}

/// [#576 RVF-d-10] The same through a link to a folder whose real name is not
/// UTF-8 (a legacy Latin-1 name, some USB or SMB mounts). The scratch spelling
/// resolves the link, so `VACUUM INTO` gets the real name, and has to get it
/// byte for byte: spelled with `to_string_lossy` it named a folder with U+FFFD
/// in it, which is not there, and the push snapshot, its compaction and the
/// repair were all refused. A save's temp (`open_scratch`) was never affected:
/// SQLite's open takes the path as bytes.
#[test]
fn scratch_files_in_a_linked_folder_whose_name_is_not_utf8_land_there() {
    use std::os::unix::ffi::OsStrExt as _;
    let root = TempDir::new().expect("dir");
    let real = root.path().join(std::ffi::OsStr::from_bytes(b"caves-\xff"));
    if let Err(e) = std::fs::create_dir(&real) {
        // A file system that only takes UTF-8 names (APFS) cannot hold the case.
        println!("[RVF-d-10] skipped: this file system refuses a non-UTF-8 name ({e})");
        return;
    }
    let linked = root.path().join("caves");
    symlink(&real, &linked).expect("a link to the folder");
    let live = project_at(&linked.join("CaveA.stalx"));

    let snapshot = linked.join("snapshot.stalx");
    let snapshotted = Database::vacuum_snapshot(&live, &snapshot);
    let compacted = Database::sanitize_push_snapshot(&snapshot, false, None);
    let repaired = Database::attempt_repair(&live);
    let in_real: Vec<String> = std::fs::read_dir(&real)
        .expect("list the real folder")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    println!(
        "[RVF-d-10] snapshot {snapshotted:?}\n  compaction {compacted:?}\n  repair \
         {repaired:?}\n  in the real folder: {in_real:?}"
    );
    snapshotted.expect("a push snapshot into the linked folder");
    compacted.expect("its compaction, staged beside it");
    let (_, ok) = repaired.expect("a repair staged beside the project");
    assert!(ok, "the repaired project passes its integrity check");
    let sections = Database::open_existing(&real.join("snapshot.stalx"))
        .expect("the snapshot opens")
        .load_project()
        .expect("the snapshot loads")
        .sections
        .len();
    assert_eq!(
        sections, 1,
        "the snapshot in the real folder holds the project"
    );
}
