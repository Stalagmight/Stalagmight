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
//! [#576 Q-W30-3-D1] An open now drops a row past the end of its section, so
//! the legacy key is built in memory right after the open
//! (`station_comment_display_tests::seed_legacy_comment_rows`), exactly as
//! the open used to leave it; it can still come from undo history an older
//! build persisted.
//!
//! All data is synthetic: a file this build saved, then rows added with plain
//! SQL (`station_comment_display_tests::file_with_comment_rows`). No fixture is
//! read or written.

use tempfile::TempDir;

use super::station_comment_display_tests::{
    file_with_comment_rows, save_and_reopen, seed_legacy_comment_rows, shot_ids, shown_comment,
};
use super::AppState;
use crate::core::{Command, CommentHolder};
use crate::models::{ShotData, StationRef};
use crate::test_fixtures::state_with_null_sink;

/// Append a shot to the section, so a comment row at index 3 (past the end of
/// the file's two shots) names the new end station.
pub(super) fn append_shot(state: &AppState, sid: u32) {
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
pub(super) fn prepend_shot(state: &AppState, sid: u32) {
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

pub(super) fn undo(state: &AppState) {
    let undone = state.undo();
    assert!(undone.success, "undo: {:?}", undone.error);
}

pub(super) fn redo(state: &AppState) {
    let redone = state.redo();
    assert!(redone.success, "redo: {:?}", redone.error);
}

pub(super) fn station_holder(sr: StationRef) -> CommentHolder {
    CommentHolder::Station { station_ref: sr }
}

/// The comment sits under the legacy key alone. Clearing it clears it - shown,
/// saved and reopened - and undo brings it back, redo clears it again.
#[test]
fn clearing_a_comment_held_under_a_legacy_key_clears_it() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[]);
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
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
    let (path, sid) = file_with_comment_rows(temp.path(), &[]);
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
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
    let (path, sid) = file_with_comment_rows(temp.path(), &[]);
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
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
        &[(1, "comment on station 1"), (2, "comment at the end")],
    );
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
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

/// Every stored comment row as `key=text`, sorted: the keys themselves, not
/// what a station shows, so a comment that changed key is visible.
pub(super) fn stored_rows(state: &AppState) -> Vec<String> {
    let mut rows: Vec<String> = state
        .get_project_state()
        .unwrap()
        .station_comments
        .iter()
        .map(|(key, text)| format!("{key:?}={text}"))
        .collect();
    rows.sort();
    rows
}

/// [#576 RV19a-2] Undo of a set or a clear puts back the exact keys it changed.
/// Its undo was the inverse `SetStationComment` with the old joined text, which
/// stored that text under the station's CANONICAL key, not the legacy `Index`
/// key it came from. Undoing the append that made the legacy key name the
/// station then removed the canonical key with the shot, and the comment the
/// file held was gone. [#576 W57-NF1] Undoing the append as well leaves the
/// key past the end, which the undo now drops as an orphan.
#[test]
fn undoing_a_comment_change_restores_the_legacy_key_it_replaced() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[]);
    for (label, comment) in [
        ("clear", None),
        ("set", Some("comment set in the app".to_string())),
    ] {
        let state = state_with_null_sink();
        state.do_load_project_full(path.clone()).unwrap();
        seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
        let loaded = stored_rows(&state);
        append_shot(&state, sid);
        let station = StationRef::new(sid, 3);
        let changed = state.execute(Command::SetStationComment {
            station_ref: station,
            comment: comment.clone(),
        });
        assert!(changed.success, "{label}: {:?}", changed.error);
        let after_change = stored_rows(&state);
        undo(&state);
        let after_undo = stored_rows(&state);
        undo(&state);
        let after_undo_append = stored_rows(&state);
        redo(&state);
        redo(&state);
        let after_redo = stored_rows(&state);
        println!("{label}: loaded              {loaded:?}");
        println!("{label}: after the {label:<5}      {after_change:?}");
        println!("{label}: undo                {after_undo:?}");
        println!("{label}: undo the append     {after_undo_append:?}");
        println!("{label}: redo, redo          {after_redo:?}");

        assert_eq!(
            after_undo, loaded,
            "{label}: undo restores the comment under the key it was stored under"
        );
        // [#576 W57-NF1] Once the append is undone the legacy key names no
        // station (it is past the end again): an orphan, which the undo drops as
        // the open does (`orphan_comment_restore_tests`).
        assert_eq!(
            after_undo_append,
            Vec::<String>::new(),
            "{label}: undoing the append as well drops the key it leaves past the end"
        );
        assert_eq!(
            after_redo, after_change,
            "{label}: redoing both gives the changed comment again"
        );
    }
}

/// [#576 PR #594 review] `StationCommentRows` names its station by position
/// (`station_ref`), and the undo history is saved with the project. Here a
/// set records one (a legacy `Index` key names the station), a shot is then
/// prepended or appended, and the project is saved and reopened: undoing the
/// shot and then the set, from the reopened history, puts the replaced rows
/// back under their own keys, and the comment back on the station the set
/// changed, not on one the added shot moved there.
#[test]
fn recorded_comment_rows_land_on_their_station_after_a_shot_a_save_and_a_reopen() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[(1, "comment on station 1")]);
    let station = StationRef::new(sid, 3);
    let shows = |state: &AppState| -> Vec<Option<String>> {
        (0..5)
            .map(|i| shown_comment(state, StationRef::new(sid, i)))
            .collect()
    };
    let adds: [(&str, fn(&AppState, u32)); 2] =
        [("prepend", prepend_shot), ("append", append_shot)];
    for (label, add_shot) in adds {
        let state = state_with_null_sink();
        state.do_load_project_full(path.clone()).unwrap();
        seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
        append_shot(&state, sid);
        let before_set = (stored_rows(&state), shows(&state));
        let set = state.execute(Command::SetStationComment {
            station_ref: station,
            comment: Some("comment set in the app".to_string()),
        });
        assert!(set.success, "{label}: set: {:?}", set.error);
        let recorded = state
            .undo_stack
            .lock()
            .peek_undo_data()
            .map(crate::core::UndoData::variant_name);
        add_shot(&state, sid);
        let saved = temp.path().join(format!("{label}.stalx"));
        let save = state.save_project(Some(saved.clone()));
        assert!(save.success, "{label}: save: {:?}", save.error);

        let reopened = state_with_null_sink();
        reopened.do_load_project_full(saved).unwrap();
        let at_reopen = shows(&reopened);
        undo(&reopened);
        undo(&reopened);
        let after_undo = (stored_rows(&reopened), shows(&reopened));
        println!("{label}: the set recorded {recorded:?}");
        println!("{label}: before the set   {before_set:?}");
        println!("{label}: at the reopen    {at_reopen:?}");
        println!("{label}: undo the {label}, undo the set {after_undo:?}");

        assert_eq!(
            recorded,
            Some("StationCommentRows"),
            "{label}: fixture: the set records the rows it replaced"
        );
        assert_eq!(
            after_undo, before_set,
            "{label}: the reopened undo puts every row back under its own key, on its station"
        );
    }
}

/// The comment rows a file holds, as `(section_id, station_index, comment)`.
fn file_rows(path: &std::path::Path) -> Vec<(u32, u32, String)> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT section_id, station_index, comment FROM station_comments ORDER BY 1, 2")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// [#576 RV20-6, Q-W30-3-D1] RV20-6's own file, read under the orphan rule. The
/// file holds one section of two shots - stations 0 to 2 - and a comment row at
/// station 3. RV20-6 pinned that the open keeps the row as a legacy `Index` key,
/// the append gives it the new station 3, the save writes it there, and an undo
/// after the reopen puts it back. But station 3 is past the end of the section
/// when the FILE IS OPENED: the row is an orphan, which the open drops and lists
/// on its receipt, and it must never attach to a station later (Q-W30-3-D1). So
/// no step holds it: the append gives station 3 no comment, the save writes no
/// row, the reopen lists nothing to drop, and the undo and redo of the append, in
/// session and after the reopen, leave it out. (The legacy key the old open left
/// still comes from older undo history; `orphan_comment_restore_tests` covers it.)
#[test]
fn rv20_6s_row_past_the_end_is_dropped_at_the_open_and_never_comes_back() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[(3, "comment past the end")]);
    let in_file = file_rows(&path);
    let state = state_with_null_sink();
    let opened = state.do_load_project_full(path).unwrap();
    let stations_at_open = shot_ids(&state).len() + 1;
    let dropped_at_open = opened.load_repairs.station_comments;
    let at_open = stored_rows(&state);
    append_shot(&state, sid);
    let station = StationRef::new(sid, 3);
    let appended = shown_comment(&state, station);
    let saved = temp.path().join("saved.stalx");
    let save = state.save_project(Some(saved.clone()));
    assert!(save.success, "{:?}", save.error);
    let saved_rows = file_rows(&saved);
    undo(&state);
    let in_session_undo = stored_rows(&state);
    redo(&state);
    let in_session_redo = shown_comment(&state, station);

    let reopened = state_with_null_sink();
    let dropped_at_reopen = reopened
        .do_load_project_full(saved)
        .unwrap()
        .load_repairs
        .station_comments;
    undo(&reopened);
    let reopened_undo = stored_rows(&reopened);
    let reopened_blocked = reopened.integrity_check().save_blocked;
    redo(&reopened);
    let reopened_redo = shown_comment(&reopened, station);
    println!(
        "the file holds             {in_file:?} (stations 0 to {})",
        stations_at_open - 1
    );
    println!("open: receipt drops {dropped_at_open}, the project holds {at_open:?}");
    println!("append: station 3 shows    {appended:?}");
    println!("the save writes            {saved_rows:?}");
    println!("in session: undo {in_session_undo:?}, redo: station 3 shows {in_session_redo:?}");
    println!("reopen: receipt drops {dropped_at_reopen}");
    println!(
        "reopened: undo {reopened_undo:?} (save refused: {reopened_blocked}), \
         redo: station 3 shows {reopened_redo:?}"
    );

    assert_eq!(
        (in_file, stations_at_open),
        (vec![(sid, 3, "comment past the end".to_string())], 3),
        "fixture: the row names station 3 of a section whose stations are 0 to 2"
    );
    assert_eq!(
        dropped_at_open, 1,
        "the open lists the orphan on its receipt"
    );
    assert_eq!(
        at_open,
        Vec::<String>::new(),
        "and the project does not hold it"
    );
    assert_eq!(appended, None, "the appended station 3 gets no comment");
    assert_eq!(saved_rows, Vec::new(), "the save writes no comment row");
    assert_eq!(dropped_at_reopen, 0, "so the reopen has nothing to drop");
    assert_eq!(
        (in_session_undo, reopened_undo),
        (Vec::new(), Vec::new()),
        "undoing the append brings no comment back, in session or after the reopen"
    );
    assert!(!reopened_blocked, "nothing blocks the save");
    assert_eq!(
        (in_session_redo, reopened_redo),
        (None, None),
        "redoing the append gives station 3 no comment either"
    );
}

/// [#576 RV19a-2] The rows the undo records survive the persisted form the
/// undo history is saved in, legacy `Index` key included.
#[test]
fn the_recorded_comment_rows_survive_the_persisted_undo_history() {
    use crate::core::{UndoData, UndoEntry, UndoStack};
    use crate::models::{StationAnchor, StationKey};

    let previous = vec![(
        StationKey::new(4, StationAnchor::Index(3)),
        "comment past the end".to_string(),
    )];
    let entry = UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::SetStationComment {
            station_ref: StationRef::new(4, 3),
            comment: None,
        },
        undo_data: UndoData::StationCommentRows {
            station_ref: StationRef::new(4, 3),
            previous: previous.clone(),
        },
        timestamp: None,
    };
    let (command, data, stamp) = UndoStack::serialize_entry_with_timestamp(&entry).unwrap();
    println!("persisted undo data: {data}");
    let back =
        UndoStack::deserialize_entry_with_timestamp(&command, &data, stamp.as_deref()).unwrap();
    let UndoData::StationCommentRows {
        station_ref,
        previous: restored,
    } = back.undo_data
    else {
        unreachable!("reloaded as {}", back.undo_data.variant_name());
    };
    assert_eq!(station_ref, StationRef::new(4, 3));
    assert_eq!(restored, previous);
}

/// [#576 RV20-6] The rows a create records survive the persisted undo history.
/// Only an entry with rows to record carries the field, so every other entry is
/// written as before; and a build that predates the field skips it, as this
/// build skips a field it does not know (`UndoData` denies no unknown field).
#[test]
fn the_rows_a_create_records_survive_the_persisted_undo_history() {
    use crate::core::{UndoData, UndoEntry, UndoStack};
    use crate::models::{StationAnchor, StationKey};

    let rows = vec![(
        StationKey::new(4, StationAnchor::Index(3)),
        "comment past the end".to_string(),
    )];
    let entry_with = |legacy_comment_rows: Vec<(StationKey, String)>| UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CreateShot {
            section_id: 4,
            shot: ShotData::default(),
            after_shot_id: None,
            before_shot_id: None,
        },
        undo_data: UndoData::CreatedShotSnapshot {
            section_id: 4,
            shot_id: 400_002,
            shot_data: ShotData::default(),
            position: 2,
            station_shift_from: 3,
            after_shot_id: None,
            before_shot_id: None,
            after_shot_uuid: None,
            before_shot_uuid: None,
            uuid: Some("uuid-of-the-appended-shot".to_string()),
            previous_offsets: std::collections::HashMap::new(),
            legacy_comment_rows,
        },
        timestamp: None,
    };
    let read_rows = |command: &str, data: &str| {
        let back = UndoStack::deserialize_entry_with_timestamp(command, data, None);
        match back.map(|entry| entry.undo_data) {
            Some(UndoData::CreatedShotSnapshot {
                legacy_comment_rows,
                ..
            }) => legacy_comment_rows,
            other => unreachable!("reloaded as {other:?}"),
        }
    };

    let (command, data, _) =
        UndoStack::serialize_entry_with_timestamp(&entry_with(rows.clone())).unwrap();
    let (_, without, _) =
        UndoStack::serialize_entry_with_timestamp(&entry_with(Vec::new())).unwrap();
    let unknown = data.replace("legacy_comment_rows", "a_field_this_build_does_not_know");
    println!("persisted undo data: {data}");
    println!("with nothing to record: {without}");
    assert_eq!(read_rows(&command, &data), rows);
    assert!(
        !without.contains("legacy_comment_rows"),
        "an entry with nothing to record is written as before: {without}"
    );
    assert_eq!(
        read_rows(&command, &unknown),
        Vec::new(),
        "an entry with a field the reader does not know still loads"
    );
}

/// [#576 RV19a-19] Deleting a station deletes its comment under every key that
/// names it. The delete removed only the station's canonical key, so a comment
/// held under a legacy `Index` key stayed behind and, once the shot was gone,
/// named the next station along - a comment the user never put there. Both
/// the single delete and the range delete.
#[test]
fn deleting_a_station_deletes_a_comment_held_under_a_legacy_key() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[]);
    for how in ["delete shot", "delete range"] {
        let state = state_with_null_sink();
        state.do_load_project_full(path.clone()).unwrap();
        seed_legacy_comment_rows(&state, sid, &[(3, "comment past the end")]);
        append_shot(&state, sid);
        append_shot(&state, sid);
        let station = StationRef::new(sid, 3);
        let next = StationRef::new(sid, 4);
        assert_eq!(
            (shown_comment(&state, station), shown_comment(&state, next)),
            (Some("comment past the end".to_string()), None),
            "fixture: the legacy key names station 3, station 4 has no comment"
        );
        let before = stored_rows(&state);

        // Shot 2 runs from station 2 to station 3; deleting it removes station
        // 3 and rewires shot 3 to start from station 2, so station 4 becomes 3.
        let doomed = shot_ids(&state)[2];
        let deleted = if how == "delete shot" {
            state.execute(Command::DeleteShot {
                section_id: sid,
                shot_id: doomed,
            })
        } else {
            state.execute_delete_shot_range_shift(sid, doomed, doomed)
        };
        assert!(deleted.success, "{how}: {:?}", deleted.error);
        let after_delete = shown_comment(&state, station);
        let rows = stored_rows(&state);
        undo(&state);
        let after_undo = shown_comment(&state, station);
        let rows_after_undo = stored_rows(&state);
        println!("{how}: station 3 removed -> index 3 now shows {after_delete:?}");
        println!("{how}: stored rows after the delete: {rows:?}");
        println!("{how}: undo -> station 3 shows {after_undo:?}, rows {rows_after_undo:?}");

        assert_eq!(
            after_delete, None,
            "{how}: the comment of the deleted station must not move to the next one"
        );
        assert!(
            rows.is_empty(),
            "{how}: no row of the deleted station is left: {rows:?}"
        );
        assert_eq!(
            after_undo.as_deref(),
            Some("comment past the end"),
            "{how}: undo brings the station and its comment back"
        );
        // [#576 RV20-18] Under the key it was stored under, not only shown: a
        // comment the undo moved to the station's canonical key shows the same.
        assert_eq!(
            rows_after_undo, before,
            "{how}: undo restores the rows the delete removed, keys included"
        );
    }
}
