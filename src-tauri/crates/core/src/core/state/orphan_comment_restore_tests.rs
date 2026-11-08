//! [#576 Q-W30-3-D1, W57-NF1, RV20-6-NF1] An orphan station comment - a key
//! that names no station, a row past the end of its section - never comes back
//! through undo history.
//!
//! The open drops such a row and lists it on its receipt. Undo history a build
//! before that decision persisted can still hold one: that build's open kept
//! the row as a positional `Index` key, so every snapshot it recorded holds the
//! key, and an append or prepend it ran could make the key name a station.
//! Restoring that history put the key back in the open project, where the next
//! append or prepend gave it a station and the save joined it into that
//! station's row. Every restore now drops a comment whose key names no
//! station, as the open does.
//!
//! All data is synthetic: a file this build saved, then rows added with plain
//! SQL (`station_comment_display_tests::file_with_comment_rows`). The older
//! build's state is built in memory right after the open
//! (`seed_legacy_comment_rows`), as that build's open left it. No fixture is
//! read or written.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::station_comment_display_tests::{
    file_with_comment_rows, seed_legacy_comment_rows, shown_comment, written_comment,
};
use super::station_comment_legacy_key_tests::{append_shot, prepend_shot, redo, stored_rows, undo};
use super::AppState;
use crate::models::{StationKey, StationRef};
use crate::test_fixtures::state_with_null_sink;

/// The comment keys of the open project that name no station.
pub(super) fn orphan_keys(state: &AppState) -> Vec<StationKey> {
    let guard = state.project.lock();
    let project = guard.as_ref().unwrap();
    let mut keys: Vec<StationKey> = project
        .station_comments
        .keys()
        .filter(|key| project.resolve_key(key).is_none())
        .cloned()
        .collect();
    keys.sort();
    keys
}

fn opened(path: &Path) -> AppState {
    let state = state_with_null_sink();
    state.do_load_project_full(path.to_path_buf()).unwrap();
    state
}

/// A file whose persisted undo history holds an orphan comment key, as a build
/// before Q-W30-3-D1 saved one: its open kept the file's row past the end of
/// the section (station 3 of a section with stations 0 to 2) as an `Index` key,
/// which blocked every save until Repair. The Repair's undo entry, a snapshot
/// pair, recorded the key in its `before` half, and the save after the Repair
/// persisted that entry. Station 2, the end of the line, has a comment of its
/// own.
fn file_whose_history_holds_an_orphan(dir: &Path) -> (PathBuf, u32) {
    let (path, sid) = file_with_comment_rows(dir, &[(2, "end of the line")]);
    let state = opened(&path);
    seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
    assert!(
        state.integrity_check().save_blocked,
        "fixture: the orphan blocks the save, as it did in that build"
    );
    let repaired = state.repair_open_project_integrity().unwrap();
    assert!(repaired.changed, "fixture: the Repair drops the orphan");
    let saved = state.save_project(None);
    assert!(
        saved.success,
        "fixture: the save goes through: {:?}",
        saved.error
    );
    (path, sid)
}

/// [#576 W57-NF1] Undoing the older build's Repair restores the snapshot it
/// recorded, orphan key and all. The key names no station, so the restore drops
/// it: the undo brings back nothing the open dropped, nothing blocks the save,
/// redo and a second undo leave it out too, and an append gives the new
/// station (index 3, the orphan's position) no comment.
#[test]
fn undo_history_from_an_older_build_never_brings_an_orphan_comment_back() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_whose_history_holds_an_orphan(temp.path());
    let state = opened(&path);
    let at_open = stored_rows(&state);
    undo(&state);
    let after_undo = stored_rows(&state);
    let orphans_after_undo = orphan_keys(&state);
    let blocked_after_undo = state.integrity_check().save_blocked;
    redo(&state);
    undo(&state);
    let after_redo_and_undo = stored_rows(&state);
    append_shot(&state, sid);
    let station = StationRef::new(sid, 3);
    let appended = shown_comment(&state, station);
    let saved = temp.path().join("saved.stalx");
    let save = state.save_project(Some(saved.clone()));
    let written = written_comment(&saved, station);
    println!("at the open                {at_open:?}");
    println!("undo the older Repair      {after_undo:?}");
    println!("  keys naming no station   {orphans_after_undo:?}");
    println!("  save refused             {blocked_after_undo}");
    println!("redo, undo                 {after_redo_and_undo:?}");
    println!("append: station 3 shows    {appended:?}");
    println!(
        "save: ok={} error={:?}; it writes {written:?} for station 3",
        save.success, save.error
    );

    assert_eq!(
        after_undo, at_open,
        "the undo brings back no comment the open dropped"
    );
    assert!(
        orphans_after_undo.is_empty(),
        "no key that names no station is left: {orphans_after_undo:?}"
    );
    assert!(!blocked_after_undo, "nothing blocks the save");
    assert_eq!(
        after_redo_and_undo, at_open,
        "redo and undo leave it out too"
    );
    assert_eq!(appended, None, "the appended station gets no comment");
    assert!(save.success, "the save goes through: {:?}", save.error);
    assert_eq!(written, None, "and writes no comment for it");
}

/// [#576 RV20-6-NF1] W56's sequence - prepend, save, reopen, undo - with the
/// orphan an older build's history restores. The prepend moves the end station
/// to index 3, the orphan's position: the save joined the orphan into the end
/// station's row, and after the reopen the undo of the prepend left the joined
/// text on the end station. In session the undo left the key past the end
/// again, for the next append. The restore drops the orphan, so the end
/// station keeps its own comment alone, in session and after the reopen alike.
#[test]
fn a_prepend_save_reopen_undo_never_joins_an_orphan_comment_onto_the_end_station() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_whose_history_holds_an_orphan(temp.path());
    let state = opened(&path);
    undo(&state);
    prepend_shot(&state, sid);
    let saved = temp.path().join("saved.stalx");
    let save = state.save_project(Some(saved.clone()));
    assert!(save.success, "the save goes through: {:?}", save.error);
    let written = written_comment(&saved, StationRef::new(sid, 3));
    undo(&state);
    let end = StationRef::new(sid, 2);
    let in_session = stored_rows(&state);
    let in_session_end = shown_comment(&state, end);
    let in_session_orphans = orphan_keys(&state);
    let reopened = opened(&saved);
    undo(&reopened);
    let after_reopen = stored_rows(&reopened);
    let reopened_end = shown_comment(&reopened, end);
    println!("the save writes for the end station (3)  {written:?}");
    println!("in session, undo the prepend             {in_session:?}");
    println!("  the end station (2) shows              {in_session_end:?}");
    println!("  keys naming no station                 {in_session_orphans:?}");
    println!("reopened, undo the prepend               {after_reopen:?}");
    println!("  the end station (2) shows              {reopened_end:?}");

    let own = Some("end of the line");
    assert_eq!(
        written.as_deref(),
        own,
        "the save writes the end station's own comment, nothing joined in"
    );
    assert_eq!(in_session_end.as_deref(), own, "in session");
    assert!(
        in_session_orphans.is_empty(),
        "the undo leaves no key past the end: {in_session_orphans:?}"
    );
    assert_eq!(
        reopened_end.as_deref(),
        own,
        "after the reopen the end station shows its own comment alone"
    );
    assert_eq!(
        after_reopen, in_session,
        "in session and after the reopen alike"
    );
}

/// [#576 RV20-6, W57-NF1] A key an older session's append made resolve. That
/// build kept the file's row past the end as an `Index` key, an append gave it
/// the new end station, and the save, allowed once the key resolves, wrote it
/// there; that history can restore the key. Undoing the append - in session,
/// and after a save and a reopen - takes the station away, and the key names no
/// station again: the undo drops it, rather than leave it (in session) or put
/// it back (after the reopen) for the next append to give to its new station.
#[test]
fn undoing_the_append_that_made_a_legacy_key_resolve_drops_the_orphan_it_leaves() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[]);
    let state = opened(&path);
    seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
    append_shot(&state, sid);
    let station = StationRef::new(sid, 3);
    assert_eq!(
        shown_comment(&state, station).as_deref(),
        Some("comment past the end"),
        "fixture: the append gave the legacy key its station"
    );
    let saved = temp.path().join("saved.stalx");
    let save = state.save_project(Some(saved.clone()));
    assert!(save.success, "the save goes through: {:?}", save.error);

    undo(&state);
    let in_session = stored_rows(&state);
    let in_session_blocked = state.integrity_check().save_blocked;
    append_shot(&state, sid);
    let in_session_appended = shown_comment(&state, station);

    let reopened = opened(&saved);
    undo(&reopened);
    let after_reopen = stored_rows(&reopened);
    let reopened_blocked = reopened.integrity_check().save_blocked;
    append_shot(&reopened, sid);
    let reopened_appended = shown_comment(&reopened, station);
    println!("in session: undo the append  {in_session:?} (save refused: {in_session_blocked})");
    println!("  append again: station 3    {in_session_appended:?}");
    println!("reopened: undo the append    {after_reopen:?} (save refused: {reopened_blocked})");
    println!("  append again: station 3    {reopened_appended:?}");

    assert_eq!(
        in_session,
        Vec::<String>::new(),
        "in session the undo drops the orphan"
    );
    assert_eq!(
        after_reopen,
        Vec::<String>::new(),
        "after a save and a reopen the undo drops it too"
    );
    assert!(
        !in_session_blocked && !reopened_blocked,
        "nothing blocks the save"
    );
    assert_eq!(
        (in_session_appended, reopened_appended),
        (None, None),
        "a station appended afterwards gets no comment"
    );
}
