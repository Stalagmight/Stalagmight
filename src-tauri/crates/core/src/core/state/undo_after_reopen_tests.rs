//! The history survives a save and a reopen, and a history the open cannot
//! read is dropped whole with the banner.
//!
//! An undo after a save and a reopen lands on exactly the state it recorded,
//! also when the undone command moved shots from one line to another (#732:
//! a combine or a split). 0.6.0 got there by translating the persisted
//! history through the load's shot UUID heal (`undo/uuid_remap.rs`); on the
//! station graph a history entry names shots by uuid and lines and stations
//! by their stable ids, the reopened entry has no session ids, and its step
//! is checked against the content hash of every line it rebuilds. Each test
//! records the project before the move, moves shots, saves, reopens, undoes
//! and compares the whole survey (the v2 text: uuids, station ids, every
//! number), then redoes and compares with the moved state.
//!
//! The drop rule (plan 8.3, `HistoryDropped`): a history with one entry this
//! build cannot read (a later build's, or damaged) is dropped whole, both
//! stacks, with the discarded-history banner; one written by an earlier
//! history format, a legacy file's included, gets the banner's
//! earlier-version words. The survey itself opens unchanged.
//!
//! All data is synthetic, except the tracked legacy fixture the C0 tests use
//! too (copied into a temporary directory, never written back).

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{
    ClosureStrategy, CombineJoinEnds, CombineLinesOptions, CombinePick, LineEnd, LineId, ShotData,
    SplitAt, SplitLineOptions, SplitSide, StationRef,
};
use crate::persistence::stalx_v2::{self, DecodeOptions, Extras, History, Mode, RawJson};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    let created = state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots,
        cave_section_id: None,
    });
    assert!(created.success, "{:?}", created.error);
    created.line_id.unwrap()
}

/// A new project saved at `path` (so the later saves have a file).
fn new_project(name: &str, path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: name.to_string(),
                location: None,
            })
            .success
    );
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.to_path_buf())
            })
            .success
    );
    state
}

fn save_close_reopen(state: &AppState, path: &Path) {
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "reopen: {:?}", r.error);
    assert!(!state.history_discarded(), "the history read back whole");
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// The whole survey as the file writes it, without the edit time a step
/// stamps afresh and the path. A reopened history has no session ids, so
/// they are not part of it (every other id is).
fn snapshot(state: &AppState) -> String {
    let mut p = project(state);
    p.modified_at = p.created_at;
    p.file_path = None;
    String::from_utf8(stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap()).unwrap()
}

/// [`snapshot`] with the id allocators left out: a step never takes one
/// back (plan 8.1).
fn survey(state: &AppState) -> String {
    snapshot(state)
        .lines()
        .filter(|line| !line.starts_with("\"counters\""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn shot_uuids(state: &AppState) -> Vec<(LineId, Vec<String>)> {
    project(state)
        .lines()
        .iter()
        .map(|l| (l.id, l.shots.iter().map(|s| s.uuid.clone()).collect()))
        .collect()
}

/// Run `moving` (which moves shots between lines), save, reopen, and check
/// that undo and redo land exactly on the recorded states.
fn assert_exact_undo_and_redo_after_reopen(
    state: &AppState,
    path: &Path,
    what: &str,
    moving: impl FnOnce(&AppState),
) {
    let before = survey(state);
    let before_uuids = shot_uuids(state);
    moving(state);
    let after_uuids = shot_uuids(state);
    let moved = survey(state);

    save_close_reopen(state, path);
    assert_eq!(
        shot_uuids(state),
        after_uuids,
        "{what}: the reopen changed a shot uuid"
    );
    assert_eq!(
        survey(state),
        moved,
        "{what}: the reopen changed the survey"
    );

    let r = state.undo();
    assert!(r.success, "{what}: undo after the reopen: {:?}", r.error);
    assert_eq!(
        shot_uuids(state),
        before_uuids,
        "{what}: undo after a reopen gave moved shots new uuids"
    );
    assert_eq!(survey(state), before, "{what}: undo after a reopen");
    assert!(
        project(state).validate().is_empty(),
        "{what}: a valid survey"
    );

    let r = state.redo();
    assert!(r.success, "{what}: redo after the reopen: {:?}", r.error);
    assert_eq!(survey(state), moved, "{what}: redo after a reopen");
}

fn split_options() -> SplitLineOptions {
    SplitLineOptions {
        name_a: "Main-A".to_string(),
        name_b: "Main-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    }
}

fn combine_options(join: Option<CombineJoinEnds>) -> CombineLinesOptions {
    CombineLinesOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join,
    }
}

fn temp_path(dir: &TempDir, file: &str) -> PathBuf {
    dir.path().join(file)
}

#[test]
fn a_combine_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = TempDir::new().unwrap();
    let path = temp_path(&dir, "combine.stalx");
    let state = new_project("Combine", &path);
    let k = create(&state, "K", vec![shot(90.0, 10.0), shot(90.0, 5.0)]);
    let o = create(&state, "O", vec![shot(0.0, 5.0), shot(0.0, 4.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 2),
        station_b: StationRef::new(o, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    save_close_reopen(&state, &path);

    assert_exact_undo_and_redo_after_reopen(&state, &path, "combine", |state| {
        let result = state.execute_combine_lines(k, o, combine_options(None));
        assert!(result.success, "{:?}", result.error);
    });
}

#[test]
fn a_combine_across_a_closure_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = TempDir::new().unwrap();
    let path = temp_path(&dir, "closure.stalx");
    let state = new_project("Loop", &path);
    let k = create(&state, "K", vec![shot(90.0, 10.0)]);
    let o = create(&state, "O", vec![shot(270.0, 9.6)]);
    for command in [
        Command::MergeStations {
            station_a: StationRef::new(k, 0),
            station_b: StationRef::new(o, 1),
        },
        Command::CreateLoopClosure {
            station_a: StationRef::new(k, 1),
            station_b: StationRef::new(o, 0),
            closure_strategy: ClosureStrategy::Batch,
        },
    ] {
        let result = state.execute(command);
        assert!(result.success, "{:?}", result.error);
    }
    save_close_reopen(&state, &path);

    assert_exact_undo_and_redo_after_reopen(&state, &path, "closure combine", |state| {
        let join = Some(CombineJoinEnds {
            kept: LineEnd::End,
            other: LineEnd::Start,
        });
        let result = state.execute_combine_lines(k, o, combine_options(join));
        assert!(result.success, "{:?}", result.error);
    });
}

#[test]
fn a_split_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = TempDir::new().unwrap();
    let path = temp_path(&dir, "split.stalx");
    let state = new_project("Split", &path);
    let main = create(
        &state,
        "Main",
        vec![shot(45.0, 5.0), shot(45.0, 7.0), shot(45.0, 3.0)],
    );
    save_close_reopen(&state, &path);

    assert_exact_undo_and_redo_after_reopen(&state, &path, "split", |state| {
        let result =
            state.execute_split_line(main, SplitAt::Station { station_index: 1 }, split_options());
        assert!(result.success, "{:?}", result.error);
    });
}

#[test]
fn a_delete_and_split_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = TempDir::new().unwrap();
    let path = temp_path(&dir, "delete-split.stalx");
    let state = new_project("Delete split", &path);
    let main = create(
        &state,
        "Main",
        vec![
            shot(45.0, 5.0),
            shot(45.0, 7.0),
            shot(45.0, 3.0),
            shot(45.0, 4.0),
        ],
    );
    save_close_reopen(&state, &path);
    let middle = project(&state).line(main).unwrap().shots[1].id;

    assert_exact_undo_and_redo_after_reopen(&state, &path, "delete and split", |state| {
        let result =
            state.execute_delete_shot_and_split(main, middle, "A".to_string(), "B".to_string());
        assert!(result.success, "{:?}", result.error);
    });
}

/// The line a split moves shots out of came in through Add .stalx, whose
/// entry put the whole line in.
#[test]
fn a_split_of_an_added_line_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = TempDir::new().unwrap();
    let source_path = temp_path(&dir, "source.stalx");
    let source = new_project("Source", &source_path);
    create(
        &source,
        "Added",
        vec![shot(10.0, 5.0), shot(10.0, 7.0), shot(10.0, 3.0)],
    );
    assert!(source.execute(Command::SaveProject { path: None }).success);
    assert!(source.execute(Command::CloseProject).success);

    let path = temp_path(&dir, "target.stalx");
    let state = new_project("Target", &path);
    create(&state, "Own", vec![shot(200.0, 6.0)]);
    let added = state.execute(Command::add_project_file(source_path));
    assert!(added.success, "{:?}", added.error);
    let line = project(&state)
        .lines()
        .iter()
        .find(|s| s.name.as_deref() == Some("Added"))
        .unwrap()
        .id;
    save_close_reopen(&state, &path);

    assert_exact_undo_and_redo_after_reopen(&state, &path, "split of an added line", |state| {
        let result =
            state.execute_split_line(line, SplitAt::Station { station_index: 1 }, split_options());
        assert!(result.success, "{:?}", result.error);
    });

    // And the Add itself undoes after the reopen, below the split.
    assert!(state.undo().success, "undo the split again");
    let r = state.undo();
    assert!(r.success, "undo of the Add after a reopen: {:?}", r.error);
    assert!(
        project(&state)
            .lines()
            .iter()
            .all(|l| l.name.as_deref() != Some("Added")),
        "the added line is gone"
    );
}

/// 0.6.0 pinned that a created line's entry held only what its side needed
/// (`UndoData::CreatedLineSnapshot` without an identity list). One changeset
/// serves both stacks now (plan Deviation U1), so that shape is gone; what
/// stays is the round trip it protected: saved on the undo side and on the
/// redo side, each reopened, the undo and the redo land exactly.
#[test]
fn a_created_line_entry_round_trips_through_a_reopen_on_either_stack() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("created.stalx");
    let state = new_project("Created", &path);
    let before = survey(&state);
    let line = create(
        &state,
        "New",
        (0..12).map(|i| shot(f64::from(i) * 7.0, 3.5)).collect(),
    );
    let after = survey(&state);
    save_close_reopen(&state, &path);
    assert_eq!(state.get_undo_stack().len(), 1, "the undo side, reloaded");
    assert!(state.undo().success);
    assert_eq!(survey(&state), before);
    save_close_reopen(&state, &path);
    assert_eq!(state.get_redo_stack().len(), 1, "the redo side, reloaded");
    assert!(state.get_undo_stack().is_empty());
    assert!(state.redo().success);
    assert_eq!(survey(&state), after);
    assert_eq!(project(&state).line(line).unwrap().shots.len(), 12);
    assert!(state.undo().success);
    assert_eq!(survey(&state), before);
}

/// Both stacks survive a save and a reopen in order, with their labels and
/// times, and every entry steps (undo all, then redo all) to the states the
/// session recorded.
#[test]
fn both_stacks_survive_a_reopen_in_order_and_step_exactly() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("stacks.stalx");
    let state = new_project("Stacks", &path);
    let mut states = vec![survey(&state)];
    let main = create(&state, "Main", vec![shot(0.0, 5.0), shot(30.0, 6.0)]);
    states.push(survey(&state));
    let side = create(&state, "Side", vec![shot(90.0, 4.0)]);
    states.push(survey(&state));
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(side, 0),
        station_b: StationRef::new(main, 2),
    });
    assert!(r.success, "{:?}", r.error);
    states.push(survey(&state));
    let r = state.execute(Command::ReverseLine {
        line_id: main,
        swap_left_right: true,
    });
    assert!(r.success, "{:?}", r.error);
    states.push(survey(&state));
    assert!(state.undo().success, "one entry on the redo stack");
    let listed = |s: &AppState| {
        let undo: Vec<_> = s
            .get_undo_stack()
            .into_iter()
            .map(|i| (i.command_type, i.description, i.timestamp))
            .collect();
        let redo: Vec<_> = s
            .get_redo_stack()
            .into_iter()
            .map(|i| (i.command_type, i.description, i.timestamp))
            .collect();
        (undo, redo)
    };
    let history = listed(&state);
    save_close_reopen(&state, &path);
    assert_eq!(listed(&state), history, "the stacks read back in order");
    assert!(state.redo().success);
    assert_eq!(survey(&state), states[4]);
    for depth in (0..4).rev() {
        let r = state.undo();
        assert!(r.success, "undo to depth {depth}: {:?}", r.error);
        assert_eq!(survey(&state), states[depth], "depth {depth}");
    }
    for (depth, expected) in states.iter().enumerate().skip(1) {
        let r = state.redo();
        assert!(r.success, "redo to depth {depth}: {:?}", r.error);
        assert_eq!(&survey(&state), expected, "depth {depth}");
    }
}

// ===========================================================================
// An unreadable history drops whole, with the banner
// ===========================================================================

/// A saved file whose history holds two real entries (one undone, so one
/// per stack), and its path.
fn file_with_history(dir: &TempDir) -> (PathBuf, String) {
    let path = dir.path().join("history.stalx");
    let state = new_project("History", &path);
    let line = create(&state, "Main", vec![shot(0.0, 5.0)]);
    let r = state.execute(Command::CreateShot {
        line_id: line,
        shot: shot(10.0, 6.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(r.success);
    assert!(state.undo().success);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let survey_text = survey(&state);
    (path, survey_text)
}

/// Rewrite the file at `path` with its history edited by `edit` (the survey
/// and every other member as they were).
fn rewrite_history(path: &Path, edit: impl FnOnce(&mut History)) {
    let decoded =
        stalx_v2::decode(&std::fs::read(path).unwrap(), &DecodeOptions::default()).unwrap();
    let mut extras = decoded.extras;
    edit(&mut extras.history);
    std::fs::write(
        path,
        stalx_v2::encode(&decoded.project, &extras, Mode::Normal).unwrap(),
    )
    .unwrap();
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "open: {:?}", r.error);
    state
}

/// The entry text with its `"v":1` version swapped for `version`.
fn with_version(entry: &RawJson, version: u32) -> RawJson {
    let text = entry
        .as_str()
        .replacen("{\"v\":1,", &format!("{{\"v\":{version},"), 1);
    assert_ne!(
        text,
        entry.as_str(),
        "setup: the entry starts with its version"
    );
    RawJson::parse(&text).unwrap()
}

#[test]
fn a_readable_history_opens_without_the_banner() {
    let dir = TempDir::new().unwrap();
    let (path, _) = file_with_history(&dir);
    let state = open(&path);
    assert!(!state.history_discarded());
    assert!(!state.history_discarded_from_earlier_version());
    assert_eq!(state.get_undo_stack().len(), 1);
    assert_eq!(state.get_redo_stack().len(), 1);
}

/// One entry of an earlier history format (the redo entry here) drops BOTH
/// stacks whole, with the earlier-version words; the survey is untouched.
#[test]
fn an_earlier_format_entry_drops_the_whole_history_with_the_earlier_version_banner() {
    let dir = TempDir::new().unwrap();
    let (path, survey_text) = file_with_history(&dir);
    rewrite_history(&path, |history| {
        let redo = history.redo.pop().unwrap();
        history.redo.push(with_version(&redo, 0));
    });
    let state = open(&path);
    assert!(state.history_discarded(), "the banner is raised");
    assert!(
        state.history_discarded_from_earlier_version(),
        "with the earlier-version words"
    );
    assert!(state.get_undo_stack().is_empty(), "the undo stack went too");
    assert!(state.get_redo_stack().is_empty());
    assert_eq!(survey(&state), survey_text, "the survey opens as saved");
    assert!(!state.undo().success, "nothing to undo");
}

/// An entry of a LATER history format, and one that is damaged, are
/// unreadable: the history drops whole with the banner, not the
/// earlier-version words.
#[test]
fn an_unreadable_entry_drops_the_whole_history_with_the_banner() {
    for (what, damage) in [
        ("a later format", 0usize),
        ("a damaged entry", 1),
        ("an unknown member", 2),
    ] {
        let dir = TempDir::new().unwrap();
        let (path, survey_text) = file_with_history(&dir);
        rewrite_history(&path, |history| {
            let undo = history.undo.pop().unwrap();
            let replaced = match damage {
                0 => with_version(&undo, 2),
                1 => {
                    let text = undo
                        .as_str()
                        .replacen("\"label\":", "\"label\":5,\"was\":", 1);
                    RawJson::parse(&text).unwrap()
                }
                _ => {
                    let text = undo.as_str().replacen("{\"v\":1,", "{\"v\":1,\"zz\":0,", 1);
                    RawJson::parse(&text).unwrap()
                }
            };
            assert_ne!(replaced.as_str(), undo.as_str(), "setup: {what}");
            history.undo.push(replaced);
        });
        let state = open(&path);
        assert!(state.history_discarded(), "{what}: the banner is raised");
        assert!(
            !state.history_discarded_from_earlier_version(),
            "{what}: not the earlier-version words"
        );
        assert!(state.get_undo_stack().is_empty(), "{what}: undo dropped");
        assert!(state.get_redo_stack().is_empty(), "{what}: redo dropped");
        assert_eq!(survey(&state), survey_text, "{what}: the survey as saved");
    }
}

/// The banner describes the open project: the next clean open clears it,
/// and a save of a dropped history writes an empty one (the next open is
/// clean).
#[test]
fn the_banner_clears_on_the_next_clean_open() {
    let dir = TempDir::new().unwrap();
    let (path, _) = file_with_history(&dir);
    let clean = dir.path().join("clean.stalx");
    std::fs::copy(&path, &clean).unwrap();
    rewrite_history(&path, |history| {
        let undo = history.undo.pop().unwrap();
        history.undo.push(with_version(&undo, 0));
    });
    let state = open(&path);
    assert!(state.history_discarded());
    let r = state.execute(Command::OpenProject {
        path: clean.clone(),
    });
    assert!(r.success);
    assert!(!state.history_discarded(), "a clean open clears the banner");
    assert!(!state.history_discarded_from_earlier_version());
    assert_eq!(state.get_undo_stack().len(), 1);

    // The dropped one, saved after an edit, reopens clean with that edit.
    let state = open(&path);
    let line = project(&state).lines()[0].id;
    let r = state.execute(Command::ReverseLine {
        line_id: line,
        swap_left_right: false,
    });
    assert!(r.success);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let reopened = open(&path);
    assert!(!reopened.history_discarded());
    assert_eq!(reopened.get_undo_stack().len(), 1, "only the new edit");
    assert!(reopened.get_redo_stack().is_empty());
    assert!(reopened.undo().success);
}

/// A legacy (SQLite) file never carries its history over (plan 7.2.8): it
/// opens with the banner's earlier-version words when it had one, and the
/// upgraded file then keeps the history made after the open.
#[test]
fn a_legacy_history_is_dropped_with_the_earlier_version_banner() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures/fractal_plant_iter3.stalx");
    let path = dir.path().join("legacy.stalx");
    std::fs::copy(&fixture, &path).unwrap();
    let rows: i64 =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM undo_stack", [], |row| row.get(0))
            .unwrap();
    assert!(rows > 0, "setup: the legacy file holds a history");
    let state = open(&path);
    assert!(state.history_discarded());
    assert!(state.history_discarded_from_earlier_version());
    assert!(state.get_undo_stack().is_empty() && state.get_redo_stack().is_empty());

    let before = survey(&state);
    let line = project(&state).lines()[0].id;
    let r = state.execute(Command::ReverseLine {
        line_id: line,
        swap_left_right: false,
    });
    assert!(r.success, "{:?}", r.error);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let reopened = open(&path);
    assert!(!reopened.history_discarded(), "the upgraded file is clean");
    assert_eq!(reopened.get_undo_stack().len(), 1);
    assert!(reopened.undo().success);
    assert_eq!(
        survey(&reopened),
        before,
        "the edit made after the open undoes"
    );
}
