//! "Add .stalx to current project" (#674) on the station-graph model, end to
//! end through `AppState`: the recorded step, its undo and redo (in the
//! session and after a reopen), the carried ignores, import hashes and
//! identity, the receipt, the refusals (each with nothing changed), the
//! placement of a georeferenced source, legacy sources (WAL, hot journal,
//! never written) and the mid-command project swap.
//!
//! Every project here is synthetic, built in code and written to a temp dir;
//! the legacy cases copy the tracked e2e fixture into a temp dir. No fixture
//! is read from outside the tree or written into it.
//!
//! The first half is the kit `merge_projects_tests.rs` shares.
//!
//! Ported from the 0.6.0 suite (same scenarios, new model): undo exactness,
//! redo ids, save and reload, the edit gate, ignores and their rename, the
//! linked source, adding twice, the refusals, the line-id budget and its
//! exact fit, orphan-placement ids, placement by datum (framed and
//! unframed), the path privacy of the command, the mid-read swap, non-regular
//! and oversized sources, invalid GPS, the WAL and hot-journal sources.
//! Retired (0.6.0 state the station graph does not have, plan 1.3): the
//! `AddedProjectContent` record and redo-record checks (the generic changeset
//! replaces it; the engine's preconditions are stream U's tests), the
//! integrity-guard rollback through an in-memory source (a source is
//! validated before the append; `append_tests.rs` covers the refusal), the
//! pending kind classification (the frozen legacy reader finishes it), the
//! record parity, reverse parity, gauge anchors and held gauges (gone with
//! the cascade). The placement-suggestion case moves to the import package
//! (the station-graph suggester is its to port).

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;

use super::project_add::{set_add_window_hook, AddWindow};
use super::AppState;
use crate::core::{Command, CommandResult, EDIT_DISABLED_CODE, READ_ONLY_FILE_CODE};
use crate::models::survey::{ClosureStrategy, Counters, LineMeta, Project, StartAt, ZeroDatum};
use crate::models::{
    gps_to_local, AzimuthReference, CorrectionSource, GpsCoordinates, LineId, ShotData, ShotId,
    ShotUpdates, SpeleoLink, StationId, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID, MAX_STATION_ID,
};
use crate::persistence::stalx_v2::{self, DecodeOptions, Extras, IgnoredWarning, Mode};

// ===========================================================================
// Kit (shared with merge_projects_tests.rs)
// ===========================================================================

/// The open project's frame in the georeferenced targets.
pub(super) const TARGET_FRAME: (f64, f64) = (20.55, -87.35);

pub(super) fn fixed_time(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + seconds, 0)
        .single()
        .expect("a valid instant")
}

/// A shot with a FIXED uuid, so every output that carries shot identity is
/// deterministic across runs.
pub(super) fn shot(uuid: &str, azimuth: f64, length: f64, depth: f64) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: depth,
        depth_out: depth + 0.5,
        ..ShotData::default()
    }
}

pub(super) fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#4ecdc4".to_string(),
        ..LineMeta::default()
    }
}

pub(super) fn gps(latitude: f64, longitude: f64, depth: f64) -> GpsCoordinates {
    GpsCoordinates::new(latitude, longitude, depth).unwrap()
}

/// Station `index` of `line`.
pub(super) fn at(project: &Project, line: LineId, index: usize) -> StationId {
    project.line(line).unwrap().station(index).unwrap()
}

pub(super) fn line_named(project: &Project, name: &str) -> LineId {
    project
        .lines()
        .iter()
        .find(|l| l.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no line named {name}"))
        .id
}

/// A placement datum on `station`, attached via `line`.
pub(super) fn add_datum(
    project: &mut Project,
    station: StationId,
    line: LineId,
    at: GpsCoordinates,
) {
    let mut datum = ZeroDatum::new(station, at);
    datum.via = Some(line);
    project.push_datum(datum);
}

pub(super) fn assert_valid(project: &Project) {
    let violations = project.validate();
    assert!(violations.is_empty(), "{violations:?}");
}

/// Raw position of a station.
pub(super) fn xy(project: &Project, station: StationId) -> [f64; 2] {
    project.raw_positions().xy(station).unwrap()
}

/// Source A: a datum, a junction (a merge in 0.6.0), a free piece, a project
/// default north reference AND a line that declares its own, a hand move,
/// comments, a closure, an entrance elevation and a location.
pub(super) fn source_alpha() -> Project {
    let mut p = Project::new_at("Alpha", Some("Yucatan".to_string()), fixed_time(1));
    p.settings.surface_elevation = Some(12.0);
    p.settings.default_azimuth_reference = Some(AzimuthReference::magnetic(
        -4.0,
        CorrectionSource::FileDeclared,
    ));
    let a0 = p
        .add_line(
            meta("A0"),
            StartAt::New,
            vec![
                shot("a0-0", 10.0, 5.0, 3.0),
                shot("a0-1", 40.0, 6.5, 3.5),
                shot("a0-2", 95.0, 4.25, 4.0),
            ],
        )
        .unwrap();
    let a1 = p
        .add_line(
            meta("A1"),
            StartAt::Station(at(&p, a0, 3)),
            vec![shot("a1-0", 200.0, 7.0, 4.5), shot("a1-1", 230.0, 3.0, 5.0)],
        )
        .unwrap();
    let a2 = p
        .add_line(
            LineMeta {
                azimuth_reference: Some(AzimuthReference::magnetic(10.0, CorrectionSource::Manual)),
                ..meta("A2")
            },
            StartAt::New,
            vec![shot("a2-0", 300.0, 2.0, 6.0)],
        )
        .unwrap();
    let _ = a1;
    let a2_start = at(&p, a2, 0);
    p.set_placement(a2_start, -30.0, 12.0).unwrap();
    let hand = at(&p, a0, 1);
    p.line_mut(a0)
        .unwrap()
        .hand_moves
        .insert(hand, [0.5, -0.5, 1.0]);
    for (index, comment) in [(2, "squeeze"), (3, "junction")] {
        let station = at(&p, a0, index);
        p.station_mut(station).unwrap().comment = Some(comment.to_string());
    }
    let start = at(&p, a0, 0);
    add_datum(&mut p, start, a0, gps(20.5, -87.3, 0.0));
    let (a, b) = (at(&p, a0, 1), at(&p, a0, 3));
    p.push_closure(a, Some(a0), b, Some(a0), ClosureStrategy::Batch);
    assert_valid(&p);
    p
}

/// Source A with a named station.
pub(super) fn source_alpha_named() -> Project {
    let mut p = source_alpha();
    let a0 = line_named(&p, "A0");
    let gate = at(&p, a0, 1);
    p.station_mut(gate).unwrap().name = "Gate".to_string();
    assert_valid(&p);
    p
}

/// Source B: a second datum, a junction, a cross-line closure, a DISAGREEING
/// entrance elevation (the first source's must win, with a warning) and a
/// new location.
pub(super) fn source_bravo() -> Project {
    let mut p = Project::new_at("Bravo", Some("Quintana Roo".to_string()), fixed_time(2));
    p.settings.surface_elevation = Some(15.0);
    let b0 = p
        .add_line(
            meta("B0"),
            StartAt::New,
            vec![shot("b0-0", 120.0, 8.0, 2.0), shot("b0-1", 150.0, 9.0, 2.5)],
        )
        .unwrap();
    let b1 = p
        .add_line(
            meta("B1"),
            StartAt::Station(at(&p, b0, 2)),
            vec![shot("b1-0", 60.0, 4.0, 3.0), shot("b1-1", 20.0, 5.5, 3.25)],
        )
        .unwrap();
    let sump = at(&p, b1, 1);
    p.station_mut(sump).unwrap().comment = Some("sump".to_string());
    let start = at(&p, b0, 0);
    add_datum(&mut p, start, b0, gps(20.6, -87.4, 1.5));
    let end = at(&p, b1, 2);
    p.push_closure(start, Some(b0), end, Some(b1), ClosureStrategy::Batch);
    assert_valid(&p);
    p
}

/// Source C: no datum (a free piece placed at (100, 50)) and a location the
/// merge already holds.
pub(super) fn source_charlie() -> Project {
    let mut p = Project::new_at("Charlie", Some("Yucatan".to_string()), fixed_time(3));
    let c0 = p
        .add_line(
            meta("C0"),
            StartAt::New,
            vec![shot("c0-0", 270.0, 11.0, 7.0)],
        )
        .unwrap();
    let start = at(&p, c0, 0);
    p.set_placement(start, 100.0, 50.0).unwrap();
    assert_valid(&p);
    p
}

/// A georeferenced one-line source: its datum at `latitude`, its shots
/// `prefix-0`, `prefix-1`, its line named `name`.
pub(super) fn georeferenced(name: &str, prefix: &str, latitude: f64) -> Project {
    let mut p = Project::new_at(name, None, fixed_time(10));
    let line = p
        .add_line(
            meta(name),
            StartAt::New,
            vec![
                shot(&format!("{prefix}-0"), 30.0, 12.0, 2.0),
                shot(&format!("{prefix}-1"), 60.0, 9.0, 2.5),
            ],
        )
        .unwrap();
    let start = at(&p, line, 0);
    add_datum(&mut p, start, line, gps(latitude, -87.3, 0.0));
    assert_valid(&p);
    p
}

pub(super) fn write_v2_with(dir: &Path, file: &str, project: &Project, extras: &Extras) -> PathBuf {
    let path = dir.join(file);
    let bytes = stalx_v2::encode(project, extras, Mode::Normal).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

pub(super) fn write_v2(dir: &Path, file: &str, project: &Project) -> PathBuf {
    write_v2_with(dir, file, project, &Extras::default())
}

/// The decoded file at `path`.
pub(super) fn read_v2(path: &Path) -> stalx_v2::Decoded {
    let bytes = std::fs::read(path).unwrap();
    stalx_v2::decode(&bytes, &DecodeOptions::default())
        .unwrap_or_else(|e| panic!("{} does not decode: {e}", path.display()))
}

/// The tracked legacy fixture (e2e/fixtures), copied into `dir` as `name`.
pub(super) fn legacy_copy(dir: &Path, name: &str) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures/fractal_plant_iter3.stalx");
    let path = dir.join(name);
    std::fs::copy(&fixture, &path).unwrap();
    path
}

/// `path` with a SQLite sidecar suffix.
pub(super) fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

/// Every shot length of a project.
pub(super) fn lengths(project: &Project) -> Vec<f64> {
    project
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.length))
        .collect()
}

/// A legacy source a crash left with committed pages in its `-wal`: the
/// first shot's length is 77 m in the WAL only.
pub(super) fn legacy_with_wal(dir: &Path) -> PathBuf {
    let live = legacy_copy(dir, "live.stalx");
    let crashed = dir.join("crashed.stalx");
    let conn = rusqlite::Connection::open(&live).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    let changed = conn
        .execute("UPDATE shots SET length = 77.0 WHERE id = 1", [])
        .unwrap();
    assert_eq!(changed, 1);
    std::fs::copy(&live, &crashed).unwrap();
    std::fs::copy(sidecar(&live, "-wal"), sidecar(&crashed, "-wal")).unwrap();
    drop(conn);
    crashed
}

/// A legacy source a crash caught mid-write: half-applied pages in the
/// database file, the originals in a hot `-journal` beside it. Every shot is
/// 1000 m longer in the interrupted write.
pub(super) fn legacy_with_hot_journal(dir: &Path) -> PathBuf {
    let clean = legacy_copy(dir, "clean.stalx");
    let crashed = dir.join("crashed.stalx");
    let conn = rusqlite::Connection::open(&clean).unwrap();
    // One page of cache makes the transaction spill into the database file,
    // which is the state a kill has to be recoverable from.
    conn.execute_batch("PRAGMA cache_size = 1;").unwrap();
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         UPDATE shots SET length = length + 1000.0;
         UPDATE sections SET name = 'interrupted-write';",
    )
    .unwrap();
    std::fs::copy(&clean, &crashed).unwrap();
    std::fs::copy(sidecar(&clean, "-journal"), sidecar(&crashed, "-journal")).unwrap();
    conn.execute_batch("ROLLBACK;").unwrap();
    drop(conn);
    assert_ne!(
        std::fs::read(&crashed).unwrap(),
        std::fs::read(&clean).unwrap(),
        "the interrupted write really reached the database file"
    );
    crashed
}

pub(super) fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "open {}: {:?}", path.display(), r.error);
    state
}

pub(super) fn save(state: &AppState) {
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save failed: {:?}", saved.error);
}

/// The open project's survey as canonical text without the fields a step
/// stamps or a reopen changes by design (`modified_at`, `file_path`), with
/// its session shot ids and its id allocators beside it (an undo keeps the
/// allocators where they are: max(live, recorded), plan 8.1).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Snapshot {
    text: String,
    shot_ids: Vec<(LineId, Vec<ShotId>)>,
    counters: Counters,
    modified_at: DateTime<Utc>,
}

pub(super) fn snapshot(state: &AppState) -> Snapshot {
    let project = state.get_project_state().unwrap();
    let mut masked = project.clone();
    masked.modified_at = masked.created_at;
    masked.file_path = None;
    let encoded =
        String::from_utf8(stalx_v2::encode(&masked, &Extras::default(), Mode::Normal).unwrap())
            .unwrap();
    // The allocators are compared on their own (one record per line, plan
    // 6.2: the `counters` member is one physical line).
    let text: Vec<&str> = encoded
        .lines()
        .filter(|line| !line.starts_with("\"counters\":"))
        .collect();
    let text = text.join("\n");
    Snapshot {
        text,
        shot_ids: project
            .lines()
            .iter()
            .map(|l| (l.id, l.shots.iter().map(|s| s.id).collect()))
            .collect(),
        counters: project.counters,
        modified_at: project.modified_at,
    }
}

/// `now` is `then` restored: the same survey and session ids; allocators
/// never below.
pub(super) fn assert_restored(now: &Snapshot, then: &Snapshot, what: &str) {
    assert_eq!(now.text, then.text, "{what}: the survey differs");
    assert_eq!(now.shot_ids, then.shot_ids, "{what}: session shot ids");
    assert!(
        now.counters.next_station_id >= then.counters.next_station_id
            && now.counters.next_closure_id >= then.counters.next_closure_id
            && now.counters.next_section_id >= then.counters.next_section_id,
        "{what}: an allocator went back ({:?} < {:?})",
        now.counters,
        then.counters
    );
}

/// Nothing changed at all, allocators and modification time included.
pub(super) fn assert_unchanged(state: &AppState, then: &Snapshot, what: &str) {
    assert_eq!(&snapshot(state), then, "{what}: the project changed");
}

/// The open target every add test adds into, written as a version 2 file:
/// `T0` (two shots) on a datum at [`TARGET_FRAME`] when `georeferenced`,
/// else a free piece at the origin; opened, with one earlier undoable edit on
/// its stack, and saved.
pub(super) fn open_target(dir: &Path, georeferenced: bool) -> (AppState, PathBuf) {
    open_target_with(dir, georeferenced, |_| {})
}

/// [`open_target`] with `shape` applied to the target before it is written.
pub(super) fn open_target_with(
    dir: &Path,
    georeferenced: bool,
    shape: impl FnOnce(&mut Project),
) -> (AppState, PathBuf) {
    let mut target = Project::new_at("Target", Some("Mexico".to_string()), fixed_time(0));
    let t0 = target
        .add_line(
            meta("T0"),
            StartAt::New,
            vec![shot("t0-0", 0.0, 10.0, 1.0), shot("t0-1", 90.0, 8.0, 1.5)],
        )
        .unwrap();
    let start = at(&target, t0, 0);
    if georeferenced {
        add_datum(
            &mut target,
            start,
            t0,
            gps(TARGET_FRAME.0, TARGET_FRAME.1, 0.0),
        );
    } else {
        target.set_placement(start, 0.0, 0.0).unwrap();
    }
    shape(&mut target);
    assert_valid(&target);
    let path = write_v2(dir, "target.stalx", &target);
    let state = open(&path);
    let shot_id = target.line(t0).unwrap().shots[0].id;
    let edited = state.execute(Command::UpdateShot {
        line_id: t0,
        shot_id,
        updates: ShotUpdates {
            length: Some(10.5),
            ..ShotUpdates::default()
        },
    });
    assert!(
        edited.success,
        "an earlier edit for the stack: {:?}",
        edited.error
    );
    save(&state);
    (state, path)
}

pub(super) fn add(state: &AppState, path: &Path) -> CommandResult {
    state.execute(Command::add_project_file(path.to_path_buf()))
}

fn undo_depth(state: &AppState) -> usize {
    state.get_undo_stack().len()
}

fn receipt_warnings(result: &CommandResult) -> Vec<String> {
    result
        .import_receipt
        .as_ref()
        .expect("an add always carries a receipt")
        .warnings
        .clone()
}

fn ignores_of(state: &AppState) -> Vec<IgnoredWarning> {
    state.with_file(|file| file.extras.ignored_warnings.clone())
}

/// The uuids of a line's shots.
fn uuids(project: &Project, line: LineId) -> Vec<String> {
    project
        .line(line)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect()
}

// ===========================================================================
// The recorded step
// ===========================================================================

#[test]
fn add_then_undo_restores_the_project_exactly_and_keeps_older_history() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha_named());
    let before = snapshot(&state);
    let depth = undo_depth(&state);

    let result = add(&state, &alpha);
    assert!(result.success, "add failed: {:?}", result.error);
    assert_eq!(result.line_ids.len(), 3, "exactly the added lines");
    assert_eq!(undo_depth(&state), depth + 1, "the add is ONE undo step");
    let project = state.get_project_state().unwrap();
    assert_valid(&project);
    assert_eq!(project.lines().len(), 4);
    // What the add brought: the source's datum, closure, comments, hand move
    // and its named station, on the copy.
    assert_eq!(project.datums.len(), 2);
    assert_eq!(project.closures.len(), 1);
    let a0 = line_named(&project, "A0");
    assert_eq!(project.station(at(&project, a0, 1)).unwrap().name, "Gate");
    assert_eq!(
        project
            .station(at(&project, a0, 3))
            .unwrap()
            .comment
            .as_deref(),
        Some("junction")
    );
    assert_eq!(project.line(a0).unwrap().hand_moves.len(), 1);
    assert_eq!(
        project.location.as_deref(),
        Some("Mexico, Yucatan"),
        "the location is appended"
    );

    assert!(state.undo().success);
    assert_restored(&snapshot(&state), &before, "undo of an add");
    assert_eq!(undo_depth(&state), depth, "the earlier history is intact");
    assert!(
        state.undo().success,
        "the edit made before the add still undoes"
    );
}

#[test]
fn the_history_entry_names_the_file_and_counts_what_it_added() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha());
    assert!(add(&state, &alpha).success);

    let items = state.get_undo_stack();
    let item = &items[0];
    assert_eq!(item.description, "Add 'alpha.stalx' to project");
    assert_eq!(item.command_type, "add_project_file");
    let preview = item.preview_data.as_ref().expect("a preview");
    let rows: Vec<(&str, &str, &str)> = preview
        .changes
        .iter()
        .map(|c| (c.field.as_str(), c.old_value.as_str(), c.new_value.as_str()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("File", "", "alpha.stalx"),
            ("Lines added", "", "3"),
            ("Shots added", "", "6"),
        ]
    );
    assert_eq!(preview.additional_changes, 0);
}

#[test]
fn redo_of_an_add_reproduces_the_same_ids() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha_named());

    let result = add(&state, &alpha);
    assert!(result.success);
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert!(state.redo().success);
    // Line ids, station ids, session shot ids, uuids and closure ids match.
    assert_restored(
        &snapshot(&state),
        &after,
        "redo reinstalls the same content",
    );
    let project = state.get_project_state().unwrap();
    for id in &result.line_ids {
        assert!(project.line(*id).is_some(), "line {id} is back");
    }
}

/// The step is persisted with the project and still undoes, then redoes,
/// after a save and a reopen, where the command no longer names the file (and
/// the file is gone).
#[test]
fn add_undo_and_redo_survive_save_and_reload() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha_named());
    let before = snapshot(&state);

    assert!(add(&state, &alpha).success);
    save(&state);
    let after = snapshot(&state);
    assert!(
        after.text.contains("\"Gate\""),
        "the station name was added"
    );
    std::fs::remove_file(&alpha).unwrap();

    let reopened = open(&path);
    assert_restored(&snapshot(&reopened), &after, "reload changes nothing");
    assert!(reopened.undo().success, "undo after reload");
    assert_restored(&snapshot(&reopened), &before, "undo after reload restores");
    assert!(reopened.redo().success, "redo after reload");
    assert_restored(&snapshot(&reopened), &after, "redo after reload reinstalls");
}

/// Undo history rides to SpeleoDB in Full mode, so the command's serialized
/// form must not carry the local path, only the file's name.
#[test]
fn the_add_command_never_serializes_its_path() {
    let private = PathBuf::from("/home/user1/private/survey.stalx");
    let command = Command::add_project_file(private);
    let json = serde_json::to_string(&command).unwrap();
    assert!(!json.contains("/home/user1"), "{json}");
    assert!(json.contains("survey.stalx"));
    assert!(!AppState::is_batch_allowed(&command));
    assert_eq!(command.description(), "Add 'survey.stalx' to project");
}

// ===========================================================================
// What an add carries
// ===========================================================================

/// The source with ignores: one on an ignorable warning, one on an error
/// (never honoured, so never carried).
fn source_with_ignores(dir: &Path) -> PathBuf {
    let mut p = Project::new_at("Ignored", None, fixed_time(4));
    let line = p
        .add_line(
            meta("I0"),
            StartAt::New,
            vec![
                shot("i0-0", 10.0, 4.0, 1.0),
                shot("i0-1", 20.0, 250.0, 1.0),
                shot("i0-2", 30.0, 300.0, 1.0),
            ],
        )
        .unwrap();
    let start = at(&p, line, 0);
    p.set_placement(start, 0.0, 0.0).unwrap();
    let extras = Extras {
        ignored_warnings: vec![
            IgnoredWarning {
                shot: "i0-1".to_string(),
                code: "LONG_SHOT".to_string(),
                at: fixed_time(5),
                comment: Some("measured twice".to_string()),
            },
            IgnoredWarning {
                shot: "i0-2".to_string(),
                code: "ZERO_LENGTH".to_string(),
                at: fixed_time(5),
                comment: None,
            },
        ],
        ..Extras::default()
    };
    write_v2_with(dir, "ignored.stalx", &p, &extras)
}

#[test]
fn ignored_warnings_survive_the_add_a_save_and_a_reload() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let source = source_with_ignores(temp.path());

    let result = add(&state, &source);
    assert!(result.success, "{:?}", result.error);
    let carried = ignores_of(&state);
    assert_eq!(carried.len(), 1, "the error-severity ignore is not carried");
    assert_eq!(
        (carried[0].shot.as_str(), carried[0].code.as_str()),
        ("i0-1", "LONG_SHOT")
    );
    assert_eq!(carried[0].comment.as_deref(), Some("measured twice"));
    assert_eq!(carried[0].at, fixed_time(5));
    let shown = state.load_resolved_warnings().unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].line_id, result.line_ids[0], "on the copied line");

    save(&state);
    assert_eq!(read_v2(&path).extras.ignored_warnings, carried, "saved");
    let reopened = open(&path);
    assert_eq!(ignores_of(&reopened), carried, "reloaded");

    // An undo leaves the ignore matching nothing: the next save drops it.
    assert!(reopened.undo().success);
    save(&reopened);
    assert!(read_v2(&path).extras.ignored_warnings.is_empty());
    // A redo brings the shot back, and the ignore with it.
    assert!(reopened.redo().success);
    save(&reopened);
    assert_eq!(read_v2(&path).extras.ignored_warnings, carried);
}

/// The second add of one file re-mints every shot uuid the first add already
/// brought in, so its carried ignores have to follow the re-mint onto the
/// copy, and the first copy keeps its own.
#[test]
fn a_carried_ignore_follows_its_shot_when_the_add_re_mints_it() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let source = source_with_ignores(temp.path());

    let first = add(&state, &source);
    assert!(first.success, "{:?}", first.error);
    let second = add(&state, &source);
    assert!(second.success, "{:?}", second.error);
    let project = state.get_project_state().unwrap();
    let copy = uuids(&project, second.line_ids[0]);
    assert!(
        !copy.iter().any(|u| u == "i0-1"),
        "precondition: the second add re-minted the shot its ignore names"
    );
    assert_eq!(uuids(&project, first.line_ids[0])[1], "i0-1");

    let expected: Vec<(String, String)> = {
        let mut keys = vec![
            ("i0-1".to_string(), "LONG_SHOT".to_string()),
            (copy[1].clone(), "LONG_SHOT".to_string()),
        ];
        keys.sort();
        keys
    };
    let keys = |ignores: Vec<IgnoredWarning>| {
        let mut keys: Vec<(String, String)> =
            ignores.into_iter().map(|i| (i.shot, i.code)).collect();
        keys.sort();
        keys
    };
    assert_eq!(keys(ignores_of(&state)), expected);
    save(&state);
    assert_eq!(keys(read_v2(&path).extras.ignored_warnings), expected);
}

fn linked(project_name: &str) -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: "22222222-2222-2222-2222-222222222222".to_string(),
        project_name: project_name.to_string(),
        base_commit_sha: Some("def456".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

/// Write `project` as a version 2 file linked to `link`.
fn write_linked(dir: &Path, file: &str, project: &Project, link: &SpeleoLink) -> PathBuf {
    let mut state = super::open_file::FileState::default();
    state.set_speleo_link(Some(link));
    write_v2_with(dir, file, project, &state.extras)
}

#[test]
fn linked_source_copies_data_but_never_the_link() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let source = write_linked(
        temp.path(),
        "charlie.stalx",
        &source_charlie(),
        &linked("Upstream Survey"),
    );
    let source_bytes = std::fs::read(&source).unwrap();

    let result = add(&state, &source);
    assert!(
        result.success,
        "a linked source is not refused: {:?}",
        result.error
    );
    assert_eq!(result.line_ids.len(), 1, "its survey data was copied");
    assert!(
        receipt_warnings(&result).iter().any(|w| w
            == "'charlie.stalx' is linked to SpeleoDB project 'Upstream Survey'; its data \
                was copied, that project is unchanged"),
        "{:?}",
        receipt_warnings(&result)
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        source_bytes,
        "source untouched"
    );
    assert!(
        state.speleo_link().is_none(),
        "the open project has no link"
    );
    save(&state);
    assert!(
        read_v2(&path).extras.speleodb.is_none(),
        "the open project did not acquire the link"
    );
}

#[test]
fn adding_the_same_stalx_twice_adds_again_with_a_warning() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha());

    let first = add(&state, &alpha);
    assert!(first.success);
    assert!(
        !receipt_warnings(&first)
            .iter()
            .any(|w| w.contains("already added")),
        "the first add is not a duplicate"
    );
    let identity = format!("stalx-project:{}", fixed_time(1).to_rfc3339());
    assert!(state
        .get_project_state()
        .unwrap()
        .imported_file_hashes
        .contains(&identity));

    // Across a save and a reopen: the identity rides the dedup set.
    save(&state);
    let state = open(&path);
    let second = add(&state, &alpha);
    assert!(second.success, "a duplicate is still added");
    assert_eq!(second.line_ids.len(), 3);
    assert!(
        receipt_warnings(&second)
            .iter()
            .any(|w| w
                == "'alpha.stalx' was already added to this project; its lines were added again"),
        "{:?}",
        receipt_warnings(&second)
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project
            .imported_file_hashes
            .iter()
            .filter(|h| **h == identity)
            .count(),
        1,
        "the identity is held once"
    );

    // A copy of the open project itself is the same survey a second time.
    let copy = temp.path().join("target-copy.stalx");
    std::fs::copy(&path, &copy).unwrap();
    let third = add(&state, &copy);
    assert!(third.success, "{:?}", third.error);
    assert!(receipt_warnings(&third)
        .iter()
        .any(|w| w.contains("is a copy of the open project")));

    // Three copies of Alpha and a copy of the target, yet every shot uuid is
    // held once and every repeated line name was made unique.
    let project = state.get_project_state().unwrap();
    assert_valid(&project);
    let mut names: Vec<&str> = project
        .lines()
        .iter()
        .filter_map(|l| l.name.as_deref())
        .collect();
    let all = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all, "line names are unique: {names:?}");
    assert!(names.contains(&"A0 (2)"), "{names:?}");
}

/// The source's import hashes come along, so re-importing the file it was
/// built from is still caught.
#[test]
fn the_sources_import_hashes_come_along() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let mut source = source_charlie();
    source.imported_file_hashes = vec!["9f86d081884c7d65".to_string()];
    source.imported_line_hashes = vec!["line-hash-1".to_string()];
    let path = write_v2(temp.path(), "charlie.stalx", &source);

    assert!(add(&state, &path).success);
    let project = state.get_project_state().unwrap();
    assert!(project
        .imported_file_hashes
        .contains(&"9f86d081884c7d65".to_string()));
    assert_eq!(
        project.imported_line_hashes,
        vec!["line-hash-1".to_string()]
    );
}

/// The receipt counts what the add found and what it brought.
#[test]
fn the_receipt_counts_lines_shots_and_closures() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha());

    let result = add(&state, &alpha);
    let receipt = result.import_receipt.expect("a receipt");
    assert_eq!(receipt.source_format, "Stalagmight");
    assert_eq!(receipt.file_name, "alpha.stalx");
    assert_eq!(
        (
            receipt.lines_found,
            receipt.lines_imported,
            receipt.shot_count
        ),
        (3, 3, 6)
    );
    assert_eq!(
        (receipt.closures_requested, receipt.closures_created),
        (1, 1)
    );
    assert!(receipt.warnings.is_empty(), "{:?}", receipt.warnings);

    // An empty source says so; a disagreeing entrance elevation is reported.
    let mut empty = Project::new_at("Empty", None, fixed_time(6));
    empty.settings.surface_elevation = Some(30.0);
    let empty = write_v2(temp.path(), "empty.stalx", &empty);
    let result = add(&state, &empty);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        receipt_warnings(&result),
        vec![
            "'empty.stalx' has an entrance elevation of 30.00 m, which differs from this \
             project's 12.00 m; kept 12.00 m"
                .to_string(),
            "'empty.stalx' holds no survey lines".to_string(),
        ]
    );
}

/// The orphan-placement signal is `CommandResult::line_ids` holding exactly
/// the added lines.
#[test]
fn added_lines_are_reported_for_orphan_placement() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let charlie = write_v2(temp.path(), "charlie.stalx", &source_charlie());

    let island = add(&state, &charlie);
    assert!(island.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(island.line_ids, vec![line_named(&project, "C0")]);
}

// ===========================================================================
// Placement
// ===========================================================================

/// A georeferenced source lands at its GPS position in the open project's
/// frame, rigidly (its free piece too), and nothing that was there moves.
#[test]
fn a_georeferenced_source_is_placed_by_its_datum() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let source = source_alpha();
    let alpha = write_v2(temp.path(), "alpha.stalx", &source);
    let before = state.get_project_state().unwrap();

    let result = add(&state, &alpha);
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    let a0 = line_named(&project, "A0");
    let expected = gps_to_local(
        &gps(20.5, -87.3, 0.0),
        &gps(TARGET_FRAME.0, TARGET_FRAME.1, 0.0),
    );
    let datum = xy(&project, at(&project, a0, 0));
    assert!(
        (datum[0] - expected.0).abs() < 1e-6 && (datum[1] - expected.1).abs() < 1e-6,
        "datum station at {datum:?}, expected {expected:?}"
    );
    // The free piece moved by the same vector.
    let source_a0 = line_named(&source, "A0");
    let source_a2 = line_named(&source, "A2");
    let vector = {
        let s = xy(&source, at(&source, source_a0, 0));
        [datum[0] - s[0], datum[1] - s[1]]
    };
    let free = xy(&project, at(&project, line_named(&project, "A2"), 0));
    let free_source = xy(&source, at(&source, source_a2, 0));
    assert!(
        (free[0] - free_source[0] - vector[0]).abs() < 1e-6
            && (free[1] - free_source[1] - vector[1]).abs() < 1e-6,
        "the free piece kept its place relative to its cave: {free:?}"
    );
    for station in before.stations.keys() {
        assert_eq!(
            xy(&project, *station),
            xy(&before, *station),
            "pre-existing station {station} must not move"
        );
    }
}

/// Into a project with NO frame, the source's datum becomes the frame (its
/// station on the origin) and the open project's free piece stays put.
#[test]
fn a_source_datum_frames_an_unreferenced_project_without_moving_it() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), false);
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha());
    let before = state.get_project_state().unwrap();

    let result = add(&state, &alpha);
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    let datum = xy(&project, at(&project, line_named(&project, "A0"), 0));
    assert!(datum[0].abs() < 1e-6 && datum[1].abs() < 1e-6, "{datum:?}");
    for station in before.stations.keys() {
        assert_eq!(xy(&project, *station), xy(&before, *station));
    }
}

// ===========================================================================
// Refusals, each with nothing changed
// ===========================================================================

#[test]
fn an_add_is_refused_with_nothing_changed() {
    let temp = TempDir::new().unwrap();
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha());

    let nothing_open = AppState::new().unwrap();
    let refused = add(&nothing_open, &alpha);
    assert!(!refused.success);
    assert!(nothing_open.get_project_state().is_none());

    let (state, path) = open_target(temp.path(), true);
    let before = snapshot(&state);
    let depth = undo_depth(&state);
    let garbage = temp.path().join("garbage.stalx");
    std::fs::write(&garbage, b"this is not a project file").unwrap();
    for (what, target, expected) in [
        (
            "the open project's own file",
            path,
            "'target.stalx' is the open project itself, so it cannot be added to it",
        ),
        (
            "a file that is not a project",
            garbage,
            "Failed to load project 'garbage.stalx': This is not a Stalagmight project file.",
        ),
        (
            "a file that does not exist",
            temp.path().join("missing.stalx"),
            "File not found: missing.stalx",
        ),
    ] {
        let refused = add(&state, &target);
        assert!(!refused.success, "{what} must be refused");
        assert_eq!(refused.error.as_deref(), Some(expected), "{what}");
        assert!(refused.import_receipt.is_none());
        assert_unchanged(&state, &before, what);
        assert_eq!(undo_depth(&state), depth, "{what} pushed nothing");
        assert!(!state.has_unsaved_changes(), "{what} dirtied nothing");
    }
}

/// A SpeleoDB project opened without its lock is read-only: the add is
/// refused by the edit gate before the source is read.
#[test]
fn the_edit_gate_refuses_an_add_before_reading_the_file() {
    let temp = TempDir::new().unwrap();
    let target = write_linked(
        temp.path(),
        "remote.stalx",
        &source_bravo(),
        &linked("Remote"),
    );
    let state = open(&target);
    let before = snapshot(&state);

    // The source does not even exist: the refusal must be the gate's.
    let refused = add(&state, &temp.path().join("missing.stalx"));
    assert!(!refused.success);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with(EDIT_DISABLED_CODE)),
        "{:?}",
        refused.error
    );
    assert_unchanged(&state, &before, "a refused add changes nothing");
}

/// A file made with a newer Stalagmight is open read-only: the add is
/// refused before the source is read.
#[test]
fn a_read_only_target_refuses_an_add_before_reading_the_file() {
    let temp = TempDir::new().unwrap();
    let path = write_v2(temp.path(), "newer.stalx", &source_bravo());
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replacen("\"requires\":[]", "\"requires\":[\"x-from-the-future\"]", 1),
    )
    .unwrap();
    let state = open(&path);
    let before = snapshot(&state);

    let refused = add(&state, &temp.path().join("missing.stalx"));
    assert!(!refused.success);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with(READ_ONLY_FILE_CODE)),
        "{:?}",
        refused.error
    );
    assert_unchanged(&state, &before, "a refused add changes nothing");
}

/// A source this build cannot read whole is refused: one that would open
/// read-only (an unknown required feature), a newer format version, a
/// survey that breaks the model, a datum out of range.
#[test]
fn a_source_that_cannot_be_copied_whole_is_refused() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let before = snapshot(&state);
    let depth = undo_depth(&state);
    let patched = |file: &str, project: &Project, from: &str, to: &str| {
        let path = write_v2(temp.path(), file, project);
        let text = std::fs::read_to_string(&path).unwrap();
        let changed = text.replacen(from, to, 1);
        assert_ne!(changed, text, "{file}: the patch landed");
        std::fs::write(&path, changed).unwrap();
        path
    };
    let charlie = source_charlie();
    let mut polar = source_charlie();
    let c0 = line_named(&polar, "C0");
    polar.placements.clear();
    let start = at(&polar, c0, 0);
    add_datum(&mut polar, start, c0, gps(20.0, -87.0, 0.0));
    polar.datums[0].gps.latitude = 95.0;
    let polar = write_v2(temp.path(), "polar.stalx", &polar);

    for (path, expected) in [
        (
            patched(
                "future.stalx",
                &charlie,
                "\"requires\":[]",
                "\"requires\":[\"x-from-the-future\"]",
            ),
            "'future.stalx' was made with a newer Stalagmight, so it cannot be added",
        ),
        (
            patched("newer.stalx", &charlie, "\"version\":2", "\"version\":3"),
            "Failed to load project 'newer.stalx': ",
        ),
        (
            patched(
                "dangling.stalx",
                &charlie,
                "{\"station\":0,\"x\":100.0,\"y\":50.0}",
                "{\"station\":99,\"x\":100.0,\"y\":50.0}",
            ),
            "Failed to load project 'dangling.stalx': The project breaks an invariant",
        ),
        (
            polar,
            "'polar.stalx' was not added: a zero datum has invalid GPS coordinates",
        ),
    ] {
        let refused = add(&state, &path);
        assert!(!refused.success, "{} must be refused", path.display());
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with(expected)),
            "{}: {:?}",
            path.display(),
            refused.error
        );
        assert_unchanged(&state, &before, expected);
        assert_eq!(undo_depth(&state), depth);
    }
}

/// The source is read under `command_lock`, so it must be a regular file of
/// bounded size: a directory or a FIFO named `*.stalx` is refused from its
/// metadata alone (a FIFO would block the read), and so is a file past the
/// SpeleoDB download ceiling (sparse, so this costs no disk).
#[test]
fn a_non_regular_or_oversized_source_is_refused() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let before = snapshot(&state);
    let depth = undo_depth(&state);

    let directory = temp.path().join("folder.stalx");
    std::fs::create_dir(&directory).unwrap();
    let oversized = temp.path().join("huge.stalx");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(crate::speleodb::client::NATIVE_LIMIT + 1)
        .unwrap();
    let mut cases = vec![
        (directory, "'folder.stalx' is not a project file"),
        (oversized, "'huge.stalx' is larger than 512 MB"),
    ];
    // A FIFO (opening one for reading would block the add forever), where
    // the platform can make one without unsafe code.
    let fifo = temp.path().join("pipe.stalx");
    if std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|status| status.success())
    {
        cases.push((fifo, "'pipe.stalx' is not a project file"));
    }

    for (path, expected) in cases {
        let refused = add(&state, &path);
        assert!(!refused.success, "{expected}");
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with(expected)),
            "{:?}",
            refused.error
        );
        assert_unchanged(&state, &before, expected);
        assert_eq!(undo_depth(&state), depth);
    }
}

/// Make the target's highest line id `id`: a one-shot free line with that
/// id (`next_line_id` is derived, max + 1).
fn spend_line_ids(project: &mut Project, id: LineId) {
    let s0 = project.mint_station("0").unwrap();
    let s1 = project.mint_station("1").unwrap();
    project
        .insert_line(crate::models::survey::Line {
            id,
            name: Some("Spacer".to_string()),
            color: "#000000".to_string(),
            direction: crate::models::SurveyDirection::In,
            quality: 1.0,
            azimuth_reference: None,
            section_id: None,
            legacy_surveyor: None,
            legacy_date: None,
            start: s0,
            shots: vec![crate::models::survey::Shot::from_data(
                0,
                shot("spacer-0", 0.0, 3.0, 1.0),
                s1,
            )],
            hand_moves: std::collections::BTreeMap::new(),
            next_shot_id: 0,
        })
        .unwrap();
    project.set_placement(s0, -50.0, -50.0).unwrap();
}

/// The line-id budget (#555): an add that uses the id space up to its last
/// id exactly is accepted, one more line is refused with nothing changed.
#[test]
fn the_line_id_budget_admits_an_exact_fit_and_refuses_one_more() {
    let incoming = source_alpha().lines().len() as u32;
    for (highest, fits) in [
        (MAX_LINES_IN_ID_SPACE as u32 - incoming - 1, true),
        (MAX_LINES_IN_ID_SPACE as u32 - incoming, false),
    ] {
        let temp = TempDir::new().unwrap();
        let (state, _) = open_target_with(temp.path(), true, |p| spend_line_ids(p, highest));
        let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha());
        let before = snapshot(&state);

        let result = add(&state, &alpha);
        assert_eq!(
            result.success, fits,
            "highest {highest}: {:?}",
            result.error
        );
        if fits {
            assert_eq!(result.line_ids.iter().max(), Some(&MAX_LINE_ID));
        } else {
            assert!(result
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("Add refused: the file holds 3 lines")));
            assert_unchanged(&state, &before, "nothing was appended");
        }
    }
}

/// E5: the copy keeps its Station IDs offset past the open project's; when
/// they do not fit, the add is refused with nothing changed (0.6.0
/// renumbered every station instead), and a source that fits still fits.
#[test]
fn an_add_past_the_station_id_space_is_refused() {
    let temp = TempDir::new().unwrap();
    let mut target = Project::new_at("Crowded", None, fixed_time(0));
    // T0 takes two ids: MAX - 3 is next. Alpha needs 8 more, Charlie 2.
    target.counters.next_station_id = MAX_STATION_ID - 5;
    let t0 = target
        .add_line(meta("T0"), StartAt::New, vec![shot("t0-0", 0.0, 10.0, 1.0)])
        .unwrap();
    let start = at(&target, t0, 0);
    target.set_placement(start, 0.0, 0.0).unwrap();
    assert_valid(&target);
    let state = open(&write_v2(temp.path(), "crowded.stalx", &target));
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha());
    let charlie = write_v2(temp.path(), "charlie.stalx", &source_charlie());
    let before = snapshot(&state);

    let refused = add(&state, &alpha);
    assert!(!refused.success);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("Add refused: the file's Station IDs run to")),
        "{:?}",
        refused.error
    );
    assert_unchanged(&state, &before, "the refused add spent nothing");
    assert!(!state.can_undo());

    let fits = add(&state, &charlie);
    assert!(fits.success, "{:?}", fits.error);
    let project = state.get_project_state().unwrap();
    assert_valid(&project);
    assert_eq!(project.counters.next_station_id, MAX_STATION_ID - 1);
}

// ===========================================================================
// The mid-command swap
// ===========================================================================

/// A project load does not take `command_lock`, so another project can be
/// committed into the slot while the source is read. The add must then leave
/// the project that is open now untouched and record nothing.
#[test]
fn an_add_is_refused_when_another_project_is_opened_mid_command() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = write_v2(temp.path(), "alpha.stalx", &source_alpha());
    let bravo = write_v2(temp.path(), "bravo.stalx", &source_bravo());
    let expected = snapshot(&open(&bravo));
    set_add_window_hook(AddWindow::DuringRead, move |state| {
        assert!(
            state.open_project(bravo).success,
            "commit another project mid-add"
        );
    });

    let refused = add(&state, &alpha);
    assert!(!refused.success, "the add must not land in Bravo");
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|e| e.contains("changed while 'alpha.stalx' was being added")),
        "{:?}",
        refused.error
    );
    assert_unchanged(&state, &expected, "the new project is untouched");
    assert_eq!(undo_depth(&state), 0, "no undo entry was pushed");
    assert!(ignores_of(&state).is_empty());
}

// ===========================================================================
// Legacy sources
// ===========================================================================

/// A legacy SQLite source is migrated in memory, like an open, and the file
/// is never written: the copy holds what an open of it holds.
#[test]
fn a_legacy_source_is_added_as_an_open_would_read_it_and_left_untouched() {
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let legacy = legacy_copy(temp.path(), "legacy.stalx");
    let bytes = std::fs::read(&legacy).unwrap();
    let opened = open(&legacy).get_project_state().unwrap();
    let before = snapshot(&state);
    let target_stations = state.get_project_state().unwrap().stations.len();

    let result = add(&state, &legacy);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.line_ids.len(), opened.lines().len());
    let receipt = result.import_receipt.unwrap();
    assert_eq!(receipt.lines_found as usize, opened.lines().len());
    assert_eq!(
        receipt.shot_count as usize,
        opened.lines().iter().map(|l| l.shots.len()).sum::<usize>()
    );
    let project = state.get_project_state().unwrap();
    assert_valid(&project);
    assert_eq!(
        project.stations.len(),
        target_stations + opened.stations.len()
    );
    assert_eq!(std::fs::read(&legacy).unwrap(), bytes, "source untouched");
    assert!(state.undo().success);
    assert_restored(&snapshot(&state), &before, "undo of a legacy add");
}

/// A source a crash left with committed pages in its `-wal`: opening it
/// would apply them, so reading it as a source must too, and neither the
/// source nor its WAL is touched.
#[test]
fn a_sources_committed_wal_pages_are_read_by_the_add() {
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let crashed = legacy_with_wal(temp.path());
    let (db, wal) = (
        std::fs::read(&crashed).unwrap(),
        std::fs::read(sidecar(&crashed, "-wal")).unwrap(),
    );
    let (state, _) = open_target(temp.path(), true);

    let result = add(&state, &crashed);
    assert!(result.success, "{:?}", result.error);
    assert!(
        lengths(&state.get_project_state().unwrap()).contains(&77.0),
        "the add applies the WAL's commit"
    );
    assert_eq!(std::fs::read(&crashed).unwrap(), db, "source untouched");
    assert_eq!(
        std::fs::read(sidecar(&crashed, "-wal")).unwrap(),
        wal,
        "its WAL too"
    );
}

/// A source with a hot `-journal`: opening it would roll the interrupted
/// write back, so the add sees the survey as it was before it, and neither
/// the source nor its journal is touched.
#[test]
fn a_sources_hot_journal_is_rolled_back_by_the_add() {
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let crashed = legacy_with_hot_journal(temp.path());
    let (db, journal) = (
        std::fs::read(&crashed).unwrap(),
        std::fs::read(sidecar(&crashed, "-journal")).unwrap(),
    );
    let (state, _) = open_target(temp.path(), true);

    let result = add(&state, &crashed);
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert!(
        lengths(&project).iter().all(|l| *l < 1000.0),
        "the add rolls the journal back"
    );
    assert!(project
        .lines()
        .iter()
        .all(|l| l.name.as_deref() != Some("interrupted-write")));
    assert_eq!(std::fs::read(&crashed).unwrap(), db, "source untouched");
    assert_eq!(
        std::fs::read(sidecar(&crashed, "-journal")).unwrap(),
        journal,
        "its journal too"
    );
}
