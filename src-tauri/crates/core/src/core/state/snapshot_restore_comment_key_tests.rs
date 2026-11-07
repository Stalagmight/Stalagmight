//! [#576 W52-1] A whole-project snapshot restore keeps the comment keys the
//! snapshot holds.
//!
//! `restore_snapshot_into` canonicalized a snapshot (UUID mint and dedup, then
//! re-anchor) whenever it held a positional `Index` anchor, to bring a blob a
//! pre-anchor build persisted up to the live model. This build's own snapshots
//! hold one too: a comment row past the end of its section stays `Index` on
//! the open (`station_comment_display_tests`), and every snapshot taken while
//! it is there records it. Once an append had made that key name the new end
//! station, undoing any snapshot command (a comment move, a shot delete)
//! re-anchored the key onto the appended shot, so undoing the append then
//! deleted the comment together with the shot.
//!
//! All data is synthetic: a file this build saved, then rows added with plain
//! SQL (`station_comment_display_tests::file_with_comment_rows`). No fixture is
//! read or written.

use tempfile::TempDir;

use super::station_comment_display_tests::{
    file_with_comment_rows, save_and_reopen, shot_ids, shown_comment,
};
use super::station_comment_legacy_key_tests::{
    append_shot, redo, station_holder, stored_rows, undo,
};
use super::AppState;
use crate::core::Command;
use crate::models::StationRef;
use crate::test_fixtures::state_with_null_sink;

/// The edit a run makes once the append has made the file's past-the-end
/// comment row name station 3.
#[derive(Clone, Copy, Debug)]
enum Edit {
    /// Move the comment from station 3 to station 1 (a snapshot pair).
    MoveComment,
    /// Set a comment on station 1 (an inverse command, no snapshot).
    SetStationComment,
    /// Append another shot (a created-shot record, no snapshot).
    Append,
    /// Delete the section's first shot (a snapshot pair).
    Delete,
}

fn apply(state: &AppState, sid: u32, edit: Edit) {
    let station = |index| StationRef::new(sid, index);
    let result = match edit {
        Edit::MoveComment => state.execute(Command::MoveComment {
            source: station_holder(station(3)),
            target: station_holder(station(1)),
            swap: false,
        }),
        Edit::SetStationComment => state.execute(Command::SetStationComment {
            station_ref: station(1),
            comment: Some("comment set in the app".to_string()),
        }),
        Edit::Append => {
            append_shot(state, sid);
            return;
        }
        Edit::Delete => state.execute(Command::DeleteShot {
            section_id: sid,
            shot_id: shot_ids(state)[0],
        }),
    };
    assert!(result.success, "{edit:?}: {:?}", result.error);
}

/// The stored comment keys are exactly the ones each history step recorded:
/// undo and redo of the edit, then of the append under it too. Last, what a
/// save writes and the reopen shows agree with what the stations show.
#[test]
fn a_snapshot_restore_keeps_the_comment_keys_it_recorded() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[(3, "comment past the end")]);
    for edit in [
        Edit::MoveComment,
        Edit::SetStationComment,
        Edit::Append,
        Edit::Delete,
    ] {
        let state = state_with_null_sink();
        state.do_load_project_full(path.clone()).unwrap();
        let loaded = stored_rows(&state);
        let loaded_blocked = state.integrity_check().save_blocked;
        append_shot(&state, sid);
        let before_edit = stored_rows(&state);
        apply(&state, sid, edit);
        let after_edit = stored_rows(&state);
        undo(&state);
        let after_undo = stored_rows(&state);
        redo(&state);
        let after_redo = stored_rows(&state);
        undo(&state);
        undo(&state);
        let after_undo_both = stored_rows(&state);
        let undone_blocked = state.integrity_check().save_blocked;
        redo(&state);
        redo(&state);
        let after_redo_both = stored_rows(&state);
        println!("{edit:?}: loaded            {loaded:?}");
        println!("{edit:?}: appended          {before_edit:?}");
        println!("{edit:?}: edited            {after_edit:?}");
        println!("{edit:?}: undo              {after_undo:?}");
        println!("{edit:?}: redo              {after_redo:?}");
        println!("{edit:?}: undo, undo        {after_undo_both:?}");
        println!("{edit:?}: redo, redo        {after_redo_both:?}");
        println!(
            "{edit:?}: save refused at the open: {loaded_blocked}, \
             after undoing both: {undone_blocked}"
        );

        assert_eq!(
            after_undo, before_edit,
            "{edit:?}: undo restores the keys the edit replaced"
        );
        assert_eq!(
            after_redo, after_edit,
            "{edit:?}: redo restores the edit's keys"
        );
        assert_eq!(
            after_undo_both, loaded,
            "{edit:?}: undoing the append as well keeps the file's comment"
        );
        assert_eq!(
            undone_blocked, loaded_blocked,
            "{edit:?}: the save is refused as it was at the open, not let through \
             without the comment"
        );
        assert_eq!(
            after_redo_both, after_edit,
            "{edit:?}: redoing both gives the edit's keys again"
        );

        // A save of the edited project, where one is allowed (the delete leaves
        // the past-the-end row an orphan again, which the save refuses).
        if state.integrity_check().save_blocked {
            println!("{edit:?}: the edited project is save-blocked; no save");
            continue;
        }
        let saved = temp.path().join(format!("saved-{edit:?}.stalx"));
        for index in [1, 3] {
            let station = StationRef::new(sid, index);
            let shown = shown_comment(&state, station);
            let (written, reopened) = save_and_reopen(&state, &saved, station);
            println!(
                "{edit:?}: station {index} shows {shown:?}, the save writes {written:?}, \
                 the reopen shows {reopened:?}"
            );
            assert_eq!(
                written, shown,
                "{edit:?}: the save writes what station {index} shows"
            );
            assert_eq!(reopened, shown, "{edit:?}: and the reopen shows it");
        }
    }
}
