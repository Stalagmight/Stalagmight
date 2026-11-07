//! [#576 RV5-5] Renaming the open project by path while a save is in flight keeps
//! the new name in the file.
//!
//! `set_project_name_for_path` (the recent-projects panel's rename) writes the name
//! through a SECOND connection to the file, then the in-memory name. Every command
//! runs on the blocking pool now, so it can run while an autosave holds
//! `command_lock`, and it opened the file the save was about to replace:
//!
//! - when its name write landed before the save's rename, the rename swapped in the
//!   temp file written from the old in-memory name: the rename reported success,
//!   the session showed the new name, and the file kept the old one until the next
//!   save (code reading; there is no seam to stop the rename between its check and
//!   its write);
//! - when the save's rename landed first, the write went to the replaced file and
//!   SQLite refused it, so the rename failed with "Failed to update project name"
//!   for no reason the user could act on (this test).
//!
//! It now takes `command_lock` like every other mutation of the open project, so it
//! runs strictly before or after the save. The save is held deterministically just
//! before its rename: it takes the `database` lock there (to close the live
//! handle), so a helper thread that holds that lock parks it with the temp file
//! written. All data is synthetic, written to a temp dir.

use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;

/// A saved project with enough shots that its save takes a while to write.
fn saved_project(path: &std::path::Path) {
    let mut project = Project::new("CaveA".to_string(), None);
    for s in 0..20 {
        project.add_section(SectionData {
            name: Some(format!("Section{s}")),
            color: "#123456".to_string(),
            shots: (0..50)
                .map(|i| ShotData {
                    length: 5.0 + f64::from(i) * 0.01,
                    azimuth_in: 10.0,
                    azimuth_out: 190.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
    }
    let mut db = Database::open(path).unwrap();
    db.save_project(&project).unwrap();
}

#[test]
fn a_rename_by_path_during_a_save_reaches_the_saved_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CaveA.stalx");
    saved_project(&path);
    let state = Arc::new(AppState::new().unwrap());
    let opened = state.open_project(path.clone());
    assert!(opened.success, "{:?}", opened.error);

    let tmp = path.with_extension("stalx.tmp");
    let (parked_tx, parked_rx) = mpsc::channel::<()>();
    let (renaming_tx, renaming_rx) = mpsc::channel::<()>();
    // Holds `database` from the moment the save is writing its temp file: the save
    // then waits just before its rename, with the old file still in place.
    let parker = {
        let state = Arc::clone(&state);
        let tmp = tmp.clone();
        thread::spawn(move || {
            let start = Instant::now();
            while !tmp.exists() {
                assert!(
                    start.elapsed() < Duration::from_secs(20),
                    "save never staged"
                );
                thread::sleep(Duration::from_micros(50));
            }
            let held = state.database.lock();
            parked_tx.send(()).unwrap();
            // Let the rename start (and, unfixed, open the file about to be replaced).
            renaming_rx.recv().unwrap();
            thread::sleep(Duration::from_millis(300));
            drop(held);
        })
    };
    let saver = {
        let state = Arc::clone(&state);
        thread::spawn(move || state.execute(Command::SaveProject { path: None }))
    };
    parked_rx.recv().unwrap();
    let tmp_staged = tmp.exists();
    renaming_tx.send(()).unwrap();
    let renamed = state.set_project_name_for_path(path.clone(), "CaveB".to_string());
    let saved = saver.join().unwrap();
    parker.join().unwrap();
    assert!(saved.success, "{:?}", saved.error);

    let live = state.project_snapshot().map(|p| p.name.clone());
    drop(state);
    let on_disk = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap()
        .name;
    println!(
        "temp file staged when the rename started: {tmp_staged}; rename success: {} \
         (error {:?}); live session name: {live:?}; name in the saved file: {on_disk:?}",
        renamed.success, renamed.error
    );
    assert!(renamed.success, "{:?}", renamed.error);
    assert_eq!(live.as_deref(), Some("CaveB"));
    assert_eq!(
        on_disk, "CaveB",
        "the rename the user made during the save is gone from the file"
    );
}
