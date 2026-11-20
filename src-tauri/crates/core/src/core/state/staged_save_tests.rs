//! The save's one sync (`sync_staged_save` in `project_io.rs`): the staging
//! file is written without SQLite's per-commit syncs (`Database::open_staging`)
//! and synced once, complete and closed, before the rename puts it in place.
//! These pin when that sync runs and what a failed one leaves behind. They
//! cannot see the `sync_all` itself (a unit test cannot observe a sync), only
//! the hook at its start; `docs/saving.md` has the syscall count.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use tempfile::TempDir;

use super::project_io::set_staged_sync_hook;
use super::AppState;
use crate::core::command::Command;
use crate::models::{LineData, Project, ShotData, SpeleoLink};
use crate::persistence::Database;
use crate::test_helpers::isolated_config;

fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 45.0,
        length,
        depth_in: 4.0,
        depth_out: 5.0,
        ..Default::default()
    }
}

fn one_line_project() -> Project {
    let mut project = Project::new("Staged save".to_string(), None);
    project.add_line(LineData {
        name: Some("Main".to_string()),
        color: "#336699".to_string(),
        shots: vec![shot(3.0), shot(4.5)],
        ..Default::default()
    });
    project
}

/// An open project saved once at `<dir>/cave.stalx`, plus an unsaved edit (a
/// second line), so the next save has something new to write.
fn saved_then_edited(dir: &Path) -> (AppState, PathBuf, Vec<u8>) {
    let state = AppState::new().unwrap();
    state.set_project_for_tests(one_line_project());
    let path = dir.join("cave.stalx");
    assert!(state.save_project(Some(path.clone())).success, "first save");
    let saved = std::fs::read(&path).unwrap();
    let result = state.execute(Command::CreateLine {
        name: Some("Side".to_string()),
        surveyor: None,
        date: None,
        color: "#993366".to_string(),
        direction: None,
        shots: vec![shot(2.0)],
        cave_section_id: None,
    });
    assert!(result.success, "edit: {:?}", result.error);
    (state, path, saved)
}

/// How many lines the file at `path` holds, read from a copy so the probe
/// never opens (or leaves a journal beside) the file under test.
fn lines_in_copy_of(path: &Path, scratch: &Path) -> usize {
    std::fs::copy(path, scratch).unwrap();
    Database::open_existing(scratch)
        .unwrap()
        .load_project()
        .unwrap()
        .lines
        .len()
}

/// Whether this process holds a file descriptor on `path` (Linux: read from
/// `/proc/self/fd`; `None` elsewhere). An open SQLite connection keeps one on
/// its database for as long as it lives.
fn held_open_by_this_process(path: &Path) -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let target = std::fs::canonicalize(path).ok()?;
    let held = std::fs::read_dir("/proc/self/fd")
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|fd| std::fs::read_link(fd.path()).ok())
        .any(|linked| linked == target);
    Some(held)
}

/// What the sync saw when it ran.
struct AtSync {
    staged: PathBuf,
    destination: Vec<u8>,
    staged_lines: usize,
    /// A `-journal` beside the staged file: a write still in flight.
    staged_journal: bool,
    /// The staging connection still open (Linux only).
    staged_held_open: Option<bool>,
}

/// Install a sync hook that records [`AtSync`] for a save to `destination`.
fn observe_sync(destination: &Path, scratch: &Path) -> Rc<RefCell<Option<AtSync>>> {
    let seen = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&seen);
    let destination = destination.to_path_buf();
    let scratch = scratch.to_path_buf();
    set_staged_sync_hook(move |staged| {
        let mut journal = staged.as_os_str().to_os_string();
        journal.push("-journal");
        // Before the copy below, which opens the staged file itself.
        let staged_held_open = held_open_by_this_process(staged);
        *slot.borrow_mut() = Some(AtSync {
            staged: staged.to_path_buf(),
            destination: std::fs::read(&destination).unwrap_or_default(),
            staged_lines: lines_in_copy_of(staged, &scratch),
            staged_journal: Path::new(&journal).exists(),
            staged_held_open,
        });
        Ok(())
    });
    seen
}

#[test]
fn the_staged_file_is_synced_complete_and_closed_before_the_rename() {
    let _config = isolated_config();
    let dir = TempDir::new().unwrap();
    let (state, path, saved) = saved_then_edited(dir.path());
    let seen = observe_sync(&path, &dir.path().join("probe.stalx"));

    let result = state.save_project(None);

    assert!(result.success, "save: {:?}", result.error);
    let seen = seen
        .borrow_mut()
        .take()
        .expect("the save must sync its staged file");
    assert_eq!(seen.staged, path.with_extension("stalx.tmp"));
    assert!(
        seen.destination == saved,
        "the sync must run while the project file still holds the previous save"
    );
    assert_eq!(
        seen.staged_lines, 2,
        "the synced file must already hold the edit"
    );
    assert!(
        !seen.staged_journal,
        "no write may be in flight (a journal beside the file) when it is synced"
    );
    assert_ne!(
        seen.staged_held_open,
        Some(true),
        "the staging connection must be closed when the file is synced"
    );
    assert_eq!(lines_in_copy_of(&path, &dir.path().join("after.stalx")), 2);
    assert!(!path.with_extension("stalx.tmp").exists());
}

#[test]
fn a_staged_file_that_cannot_be_synced_leaves_the_project_file_untouched() {
    let _config = isolated_config();
    let dir = TempDir::new().unwrap();
    let (state, path, saved) = saved_then_edited(dir.path());
    set_staged_sync_hook(|_| Err(std::io::Error::other("injected: the disk went away")));

    let result = state.save_project(None);

    assert!(
        !result.success,
        "a save whose file is not on disk must fail"
    );
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("Failed to write the save to disk") && error.contains("injected"),
        "the failure must say what happened: {error}"
    );
    assert!(
        std::fs::read(&path).unwrap() == saved,
        "the project file must be untouched"
    );
    assert!(
        !path.with_extension("stalx.tmp").exists(),
        "the unsynced staging file must be removed"
    );
    // The failure came before the session's own handle was closed, so it is
    // still open, and the next save goes through with the edit.
    assert_eq!(state.open_project_path_for_tests(), Some(path.clone()));
    assert_eq!(
        state.with_open_database_for_tests(|db| db.load_project().map(|p| p.lines.len()).ok()),
        Some(Some(1))
    );
    assert!(state.save_project(None).success, "the retry");
    assert_eq!(lines_in_copy_of(&path, &dir.path().join("retry.stalx")), 2);
}

/// Save a Copy of a SpeleoDB-managed project writes a detached copy through
/// its own branch of the pipeline; it gets the same sync before its rename.
#[test]
fn a_detached_copy_is_synced_before_it_is_renamed_into_place() {
    let _config = isolated_config();
    let dir = TempDir::new().unwrap();
    let (state, path, _) = saved_then_edited(dir.path());
    state
        .with_open_database_for_tests(|db| {
            db.save_speleo_link(Some(&SpeleoLink {
                endpoint: "https://speleodb.invalid".to_string(),
                project_id: "11111111-1111-1111-1111-111111111111".to_string(),
                project_name: "Remote".to_string(),
                base_commit_sha: None,
                has_unpushed_edits: false,
                last_edit_at: None,
                last_verify_at: None,
                undo_mode: None,
            }))
        })
        .unwrap()
        .unwrap();
    let copy = dir.path().join("copy.stalx");
    let seen = observe_sync(&copy, &dir.path().join("probe.stalx"));

    let result = state.save_project(Some(copy.clone()));

    assert!(result.success, "save a copy: {:?}", result.error);
    assert!(
        result.unlinked_copy_path.is_some(),
        "this must be the detached-copy branch: {result:?}"
    );
    let seen = seen.borrow_mut().take().expect("the copy must be synced");
    assert_eq!(seen.staged, copy.with_extension("stalx.tmp"));
    assert!(
        seen.destination.is_empty(),
        "the copy must not exist yet when its staged file is synced"
    );
    assert_eq!(seen.staged_lines, 2);
    assert_eq!(lines_in_copy_of(&copy, &dir.path().join("after.stalx")), 2);
    assert_eq!(state.open_project_path_for_tests(), Some(path));
}

/// Nothing above can tell a staging connection from a synced one: both write
/// the same bytes, and the tests see only the hook. So the opener the save
/// stages through is pinned in its source, the way `speleodb_session` pins its
/// bump sites. A save through `Database::open` is correct, only slower: 0.7 s
/// instead of 15 ms for a small project on a fast SSD.
#[test]
fn the_save_stages_through_the_unsynced_opener() {
    let source = std::fs::read_to_string(format!(
        "{}/src/core/state/project_io.rs",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("read project_io.rs");
    assert!(
        source.contains("Database::open_staging(&temp_path)"),
        "the save must build its temp file through `Database::open_staging`"
    );
    assert!(
        !source.contains("Database::open(&temp_path)"),
        "the save's temp file must not be opened with per-commit syncs"
    );
}
