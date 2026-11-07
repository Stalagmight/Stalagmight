//! [#576 W30-NF1] Setting, clearing and moving a station comment act on every
//! stored key that names the station.
//!
//! A station's comment can sit under a legacy positional `Index` key: a row
//! past the end of its section stays `Index` on the open's re-anchor, and an
//! edit that gives the section that station (a shot appended or prepended)
//! makes it name a station that has, or then gets, a canonical key of its own
//! (see `station_comment_display_tests`). The station shows, and the save
//! writes, all its keys' comments joined (`Project::one_row_per_station`).
//! `SetStationComment` and `MoveComment` read and wrote only the canonical
//! key: clearing such a comment left it in place, setting one added the new
//! text beside the old instead of replacing it, and moving it failed with
//! "Source has no comment to move".
//!
//! All data is synthetic: a file this build saved, then rows added with plain
//! SQL (`station_comment_display_tests::file_with_comment_rows`). No fixture is
//! read or written.

use tempfile::TempDir;

use super::station_comment_display_tests::{
    file_with_comment_rows, save_and_reopen, shot_ids, shown_comment,
};
use super::AppState;
use crate::core::{Command, CommentHolder};
use crate::models::{ShotData, StationRef};
use crate::test_fixtures::state_with_null_sink;

/// Append a shot to the section, so a comment row at index 3 (past the end of
/// the file's two shots) names the new end station.
fn append_shot(state: &AppState, sid: u32) {
    let last = *shot_ids(state).last().unwrap();
    let appended = state.execute(Command::CreateShot {
        section_id: sid,
        shot: ShotData {
            length: 3.0,
            ..Default::default()
        },
        after_shot_id: Some(last),
        before_shot_id: None,
    });
    assert!(appended.success, "append: {:?}", appended.error);
}

/// Prepend a shot to the section: the end station moves from index 2 to 3,
/// where a row at index 3 now resolves - two keys, one station.
fn prepend_shot(state: &AppState, sid: u32) {
    let first = *shot_ids(state).first().unwrap();
    let prepended = state.execute(Command::CreateShot {
        section_id: sid,
        shot: ShotData {
            length: 3.0,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: Some(first),
    });
    assert!(prepended.success, "prepend: {:?}", prepended.error);
}

fn undo(state: &AppState) {
    let undone = state.undo();
    assert!(undone.success, "undo: {:?}", undone.error);
}

fn redo(state: &AppState) {
    let redone = state.redo();
    assert!(redone.success, "redo: {:?}", redone.error);
}

fn station_holder(sr: StationRef) -> CommentHolder {
    CommentHolder::Station { station_ref: sr }
}

/// The comment sits under the legacy key alone. Clearing it clears it - shown,
/// saved and reopened - and undo brings it back, redo clears it again.
#[test]
fn clearing_a_comment_held_under_a_legacy_key_clears_it() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[(3, "comment past the end")]);
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    append_shot(&state, sid);
    let station = StationRef::new(sid, 3);
    assert_eq!(
        shown_comment(&state, station).as_deref(),
        Some("comment past the end"),
        "fixture: the legacy key names the appended station"
    );

    let cleared = state.execute(Command::SetStationComment {
        station_ref: station,
        comment: None,
    });
    assert!(cleared.success, "clear: {:?}", cleared.error);
    let after_clear = shown_comment(&state, station);
    undo(&state);
    let after_undo = shown_comment(&state, station);
    redo(&state);
    let after_redo = shown_comment(&state, station);
    let (written, reopened) = save_and_reopen(&state, &temp.path().join("saved.stalx"), station);
    println!("clear  -> station {station:?} shows {after_clear:?}");
    println!("undo   -> {after_undo:?}");
    println!("redo   -> {after_redo:?}");
    println!("save writes {written:?}, the reopen shows {reopened:?}");

    assert_eq!(
        after_clear, None,
        "a cleared comment is gone from the station"
    );
    assert_eq!(
        after_undo.as_deref(),
        Some("comment past the end"),
        "undo brings the cleared comment back"
    );
    assert_eq!(after_redo, None, "redo clears it again");
    assert_eq!(written, None, "the save writes no comment for the station");
    assert_eq!(reopened, None, "and the reopen shows none");
}

/// The comment sits under the legacy key alone. Setting a new one replaces it
/// (it used to be stored beside it, so the station showed both joined), and
/// undo brings the old one back, redo the new one.
#[test]
fn setting_a_comment_where_a_legacy_key_names_the_station_replaces_it() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[(3, "comment past the end")]);
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    append_shot(&state, sid);
    let station = StationRef::new(sid, 3);

    let set = state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("comment set in the app".to_string()),
    });
    assert!(set.success, "set: {:?}", set.error);
    let after_set = shown_comment(&state, station);
    undo(&state);
    let after_undo = shown_comment(&state, station);
    redo(&state);
    let after_redo = shown_comment(&state, station);
    let (written, reopened) = save_and_reopen(&state, &temp.path().join("saved.stalx"), station);
    println!("set    -> station {station:?} shows {after_set:?}");
    println!("undo   -> {after_undo:?}");
    println!("redo   -> {after_redo:?}");
    println!("save writes {written:?}, the reopen shows {reopened:?}");

    assert_eq!(
        after_set.as_deref(),
        Some("comment set in the app"),
        "the new comment replaces the one the station showed"
    );
    assert_eq!(
        after_undo.as_deref(),
        Some("comment past the end"),
        "undo brings the replaced comment back"
    );
    assert_eq!(after_redo, after_set, "redo sets the new one again");
    assert_eq!(written, after_set, "the save writes what the station shows");
    assert_eq!(reopened, after_set, "and the reopen shows it");
}

/// The comment sits under the legacy key alone. Moving it to another station
/// moves it - it used to fail with "Source has no comment to move" - and undo
/// and redo move it back and forth.
#[test]
fn moving_a_comment_held_under_a_legacy_key_moves_it() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[(3, "comment past the end")]);
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    append_shot(&state, sid);
    let source = StationRef::new(sid, 3);
    let target = StationRef::new(sid, 1);

    let moved = state.execute(Command::MoveComment {
        source: station_holder(source),
        target: station_holder(target),
        swap: false,
    });
    let shown = |state: &AppState| (shown_comment(state, source), shown_comment(state, target));
    let after_move = shown(&state);
    println!(
        "move {source:?} -> {target:?}: success={} error={:?}; (source, target) show {after_move:?}",
        moved.success, moved.error
    );
    assert!(moved.success, "move: {:?}", moved.error);
    undo(&state);
    let after_undo = shown(&state);
    redo(&state);
    let after_redo = shown(&state);
    let saved = temp.path().join("saved.stalx");
    let (written_source, _) = save_and_reopen(&state, &saved, source);
    let (written_target, reopened_target) = save_and_reopen(&state, &saved, target);
    println!("undo   -> {after_undo:?}");
    println!("redo   -> {after_redo:?}");
    println!(
        "save writes (source, target) = ({written_source:?}, {written_target:?}); \
         the reopen shows {reopened_target:?} on the target"
    );

    let moved_state = (None, Some("comment past the end".to_string()));
    assert_eq!(after_move, moved_state, "the comment moves to the target");
    assert_eq!(
        after_undo,
        (Some("comment past the end".to_string()), None),
        "undo moves it back"
    );
    assert_eq!(after_redo, moved_state, "redo moves it again");
    assert_eq!(
        (written_source, written_target),
        moved_state,
        "as the save writes"
    );
    assert_eq!(reopened_target, moved_state.1, "and the reopen shows");
}

/// The station holds two comments - its canonical key's and a legacy key's,
/// shown joined. Swapping it with another station's comment swaps the whole
/// joined text: the source used to keep the legacy key's line beside the
/// comment swapped in, and the target got only the canonical key's.
#[test]
fn swapping_a_comment_held_under_two_keys_swaps_the_joined_text() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(
        temp.path(),
        &[
            (1, "comment on station 1"),
            (2, "comment at the end"),
            (3, "comment past the end"),
        ],
    );
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    prepend_shot(&state, sid);
    let source = StationRef::new(sid, 3);
    let target = StationRef::new(sid, 2);
    let joined = "comment at the end\ncomment past the end".to_string();
    let shown = |state: &AppState| (shown_comment(state, source), shown_comment(state, target));
    assert_eq!(
        shown(&state),
        (
            Some(joined.clone()),
            Some("comment on station 1".to_string())
        ),
        "fixture: the end station shows both its comments, station 1 moved to index 2"
    );

    let swapped = state.execute(Command::MoveComment {
        source: station_holder(source),
        target: station_holder(target),
        swap: true,
    });
    assert!(swapped.success, "swap: {:?}", swapped.error);
    let after_swap = shown(&state);
    undo(&state);
    let after_undo = shown(&state);
    redo(&state);
    let after_redo = shown(&state);
    println!("swap {source:?} <-> {target:?}: (source, target) show {after_swap:?}");
    println!("undo   -> {after_undo:?}");
    println!("redo   -> {after_redo:?}");

    let swapped_state = (
        Some("comment on station 1".to_string()),
        Some(joined.clone()),
    );
    assert_eq!(
        after_swap, swapped_state,
        "the two stations' comments trade places"
    );
    assert_eq!(
        after_undo,
        (Some(joined), Some("comment on station 1".to_string())),
        "undo trades them back"
    );
    assert_eq!(after_redo, swapped_state, "redo trades them again");
}
