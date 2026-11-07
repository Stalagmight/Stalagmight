//! [#576 RV6a-5] A pull of the open project carries over the display settings it
//! holds when the file is replaced, not older ones.
//!
//! Replacing the open project's cache file (a pull, a take-remote) reads the
//! file's display settings to carry over, then closes the project and renames the
//! server copy over it. The read used to run before `command_lock` was taken, so a
//! display toggle landing between the read and the close (a toggle writes through
//! the open handle under `command_lock`) went into the file about to be replaced,
//! and the reopened project showed the older value. The read is now inside the
//! close's `command_lock` span.
//!
//! The toggle is simulated deterministically: this thread holds `command_lock`
//! (as `save_display_setting` does while it writes) while the replace starts on
//! another thread, then writes the setting through the open handle and lets go.
//!
//! All data is synthetic: a project built in memory and a one-entry ZIP dump.

use std::io::Write as _;
use std::time::Duration;

use tempfile::TempDir;

use super::speleodb_cache::InheritedUndo;
use super::AppState;
use crate::models::Project;
use crate::persistence::Database;
use crate::speleodb::dump::Dump;

const ENTRY: &str = "misc/project.stalx";

/// A one-entry ZIP standing in for a downloaded dump.
fn server_dump() -> Vec<u8> {
    let temp = TempDir::new().expect("temp");
    let project = temp.path().join("server.stalx");
    Database::open(&project)
        .expect("open server project")
        .save_project(&Project::new("Server Copy".to_owned(), None))
        .expect("save server project");
    let bytes = std::fs::read(project).expect("read server project");
    let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file(ENTRY, opts).expect("start_file");
    writer.write_all(&bytes).expect("write entry");
    writer.finish().expect("finish zip").into_inner()
}

#[test]
fn a_toggle_racing_the_replace_is_the_value_carried_over() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp");
    let dest = temp.path().join("project.stalx");
    Database::open(&dest)
        .expect("create local copy")
        .save_project(&Project::new("Local Copy".to_owned(), None))
        .expect("save local copy");
    let state = AppState::new().expect("state");
    let opened = state.open_project(dest.clone());
    assert!(opened.success, "{:?}", opened.error);
    let zip = server_dump();

    let carried = std::thread::scope(|scope| {
        // The toggle holds the lock while it writes.
        let toggle = state.hold_commands();
        let replace = scope.spawn(|| {
            let mut dump = Dump::open(&zip).expect("open dump");
            state.replace_cache_stalx_abandoning(&mut dump, ENTRY, &dest, InheritedUndo::Drop)
        });
        // Long enough for a replace that reads before taking the lock to read.
        std::thread::sleep(Duration::from_millis(300));
        state
            .database
            .lock()
            .as_ref()
            .expect("the project is still open")
            .save_display_setting("showGrid", "false")
            .expect("toggle");
        drop(toggle);
        replace.join().expect("replace thread")
    })
    .expect("the replace goes through");

    println!(
        "display settings carried over a replace racing a toggle: showGrid = {:?}",
        carried.get("showGrid")
    );
    assert_eq!(
        carried.get("showGrid").map(String::as_str),
        Some("false"),
        "the toggle that landed before the close is the value carried over"
    );
}
