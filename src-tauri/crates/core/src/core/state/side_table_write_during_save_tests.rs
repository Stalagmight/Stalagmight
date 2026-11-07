//! [#576 RV5-1] A side-table write that lands while a save is writing its temp
//! file is not lost.
//!
//! The save snapshots the side tables (display settings, ignored warnings, ...)
//! from the open file, builds a fresh temp file and renames it over the open
//! one. A display-setting change or an Ignore/Un-ignore written through the open
//! handle between the snapshot and the rename went into the file the rename then
//! replaced: gone from disk and from the session, with `Ok` returned. They used to
//! be main-thread commands that could not interleave with a save; on the pool
//! they can (an autosave and a display toggle). They now take `command_lock`,
//! which the save holds throughout, so they land before or after it.
//!
//! The interleaving is forced with a test seam in the save (`AFTER_TEMP_WRITTEN`,
//! run on the saving thread with the temp file complete and not yet renamed). All
//! data is synthetic.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use super::project_io::AFTER_TEMP_WRITTEN;
use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::{Database, ResolvedWarning};

/// A saved project `CaveA.stalx` with one section and `gridSpacing = old`.
fn saved_project(dir: &Path) -> PathBuf {
    let path = dir.join("CaveA.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        color: "#123456".to_string(),
        shots: (0..20)
            .map(|i| ShotData {
                length: 5.0 + f64::from(i) * 0.1,
                azimuth_in: 10.0,
                azimuth_out: 190.0,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    db.save_display_setting("gridSpacing", "old").unwrap();
    path
}

/// Run `write` on its own thread while a save on another thread is parked with
/// its temp file complete and not yet renamed. Returns whether the write had
/// already returned before the save was let go, and the save's success.
fn write_during_save<T: Send + 'static>(
    state: &Arc<AppState>,
    write: impl FnOnce(&AppState) -> T + Send + 'static,
) -> (bool, T) {
    let (reached_tx, reached_rx) = mpsc::channel::<()>();
    let (resume_tx, resume_rx) = mpsc::channel::<()>();
    let saver = {
        let state = Arc::clone(state);
        thread::spawn(move || {
            AFTER_TEMP_WRITTEN.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    reached_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                }));
            });
            state.execute(Command::SaveProject { path: None })
        })
    };
    reached_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the save staged its temp file");
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let writer = {
        let state = Arc::clone(state);
        thread::spawn(move || {
            let out = write(&state);
            let _ = done_tx.send(());
            out
        })
    };
    // Before the fix the write returns at once (into the file about to be
    // replaced); after it, it waits for the save.
    let landed_during_save = done_rx.recv_timeout(Duration::from_millis(500)).is_ok();
    resume_tx.send(()).unwrap();
    let saved = saver.join().unwrap();
    assert!(saved.success, "{:?}", saved.error);
    (landed_during_save, writer.join().unwrap())
}

#[test]
fn a_display_setting_changed_during_a_save_is_kept() {
    let dir = TempDir::new().unwrap();
    let path = saved_project(dir.path());
    let state = Arc::new(AppState::new().unwrap());
    let opened = state.open_project(path.clone());
    assert!(opened.success, "{:?}", opened.error);

    let (during, wrote) =
        write_during_save(&state, |s| s.save_display_setting("gridSpacing", "new"));
    wrote.expect("the setting write reports success");
    let live = state.get_display_setting("gridSpacing").unwrap();
    drop(state);
    let on_disk = Database::open_existing(&path)
        .unwrap()
        .load_display_settings()
        .unwrap()
        .get("gridSpacing")
        .cloned();
    println!(
        "display setting written during a save: returned before the save finished: {during}; \
         live gridSpacing = {live:?}; on disk gridSpacing = {on_disk:?}"
    );
    assert_eq!(live.as_deref(), Some("new"), "the session lost the change");
    assert_eq!(on_disk.as_deref(), Some("new"), "the file lost the change");
}

#[test]
fn an_ignore_and_an_unignore_during_a_save_are_kept() {
    let dir = TempDir::new().unwrap();
    let path = saved_project(dir.path());
    let state = Arc::new(AppState::new().unwrap());
    let opened = state.open_project(path.clone());
    assert!(opened.success, "{:?}", opened.error);
    let section_id = state.get_project_state().unwrap().sections[0].id;
    let ignore = move |code: &str| ResolvedWarning {
        section_id,
        shot_id: 0,
        warning_code: code.to_string(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    };
    // One ignore already on disk, to be un-ignored during the save.
    state
        .save_resolved_warning_gated(&ignore("LONG_SHOT"))
        .unwrap();
    assert!(state.execute(Command::SaveProject { path: None }).success);

    let new_ignore = ignore("STEEP_SHOT");
    let (ignored_during, ignored) =
        write_during_save(&state, move |s| s.save_resolved_warning_gated(&new_ignore));
    ignored.expect("the ignore reports success");
    let (unignored_during, unignored) = write_during_save(&state, move |s| {
        s.delete_resolved_warning_gated(section_id, 0, "LONG_SHOT", None)
    });
    unignored.expect("the un-ignore reports success");
    drop(state);
    let mut on_disk: Vec<String> = Database::open_existing(&path)
        .unwrap()
        .load_resolved_warnings_verbatim()
        .unwrap()
        .into_iter()
        .map(|w| w.warning_code)
        .collect();
    on_disk.sort();
    println!(
        "ignore during a save returned early: {ignored_during}; un-ignore during a save \
         returned early: {unignored_during}; ignored codes on disk: {on_disk:?}"
    );
    assert_eq!(
        on_disk,
        vec!["STEEP_SHOT".to_string()],
        "the file keeps the new ignore and drops the un-ignored one"
    );
}
