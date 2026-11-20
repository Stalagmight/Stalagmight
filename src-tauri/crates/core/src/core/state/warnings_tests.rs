//! Project warnings and their ignores on the station-graph model (package
//! C10): each warning kind with the refs the frontend reads, the ignore
//! round trip in the session and through a save and a reopen, the load
//! result, the datum GPS-gap ignore pinned to the datum's GPS, and the
//! command refusals ported from the 0.6.0 "Validation Tests" and "Edge Case
//! Tests for Validation Errors" of `tests.rs`.
//!
//! All data is synthetic: projects built in memory and written to a temp dir,
//! no fixture read or written.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use tempfile::TempDir;

use super::AppState;
use crate::core::events::EventSink;
use crate::core::{Command, WarningSeverity};
use crate::models::survey::{ClosureId, ClosureStrategy, LineMeta, Project, StartAt};
use crate::models::{
    datum_gap_ignore_code, local_to_gps, warning_is_ignorable, GpsCoordinates, LineId, LineUpdates,
    ProjectWarning, ShotData, ShotUpdates, StationId, StationRef, CLOSURE_ACROSS_ONE_SHOT,
    CLOSURE_ADJUSTMENT_FAILED, DATUM_GPS_GAP, DATUM_GPS_GAP_START,
};
use crate::persistence::stalx_v2::{self, Extras, IgnoredWarning, Mode};
use crate::persistence::ResolvedWarning;

/// The production load path reports progress through a sink; nothing listens.
struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

// ----- building surveys -------------------------------------------------------

/// A flat shot (no depth change).
fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..ShotData::default()
    }
}

fn north(count: usize, length: f64) -> Vec<ShotData> {
    (0..count).map(|_| shot(0.0, length)).collect()
}

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        ..LineMeta::default()
    }
}

/// A line named `name` on a piece of its own, its start placed at `at`.
fn free_line(p: &mut Project, name: &str, at: [f64; 2], shots: Vec<ShotData>) -> LineId {
    let line = p.add_line(meta(name), StartAt::New, shots).unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, at[0], at[1]).unwrap();
    line
}

/// A line named `name` starting on the existing station `from` (a junction).
fn line_from(p: &mut Project, name: &str, from: StationId, shots: Vec<ShotData>) -> LineId {
    p.add_line(meta(name), StartAt::Station(from), shots)
        .unwrap()
}

fn st(p: &Project, line: LineId, index: usize) -> StationId {
    p.line(line).unwrap().station(index).unwrap()
}

fn uuid_of(p: &Project, line: LineId, index: usize) -> String {
    p.line(line).unwrap().shots[index].uuid.clone()
}

/// The GPS frame's origin (synthetic).
fn origin() -> GpsCoordinates {
    GpsCoordinates::new(10.0, 20.0, 0.0).unwrap()
}

/// The GPS position `east` / `north` metres from [`origin`].
fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

/// One line, "Main", with a clean shot, a short one (advisory), one longer
/// than THIS project's 8 m maximum (the global default is 50 m) and one with
/// a negative left (an error).
fn checked_line() -> (Project, LineId) {
    let mut p = Project::new("Checks", None);
    p.settings.validation.warn_shot_length_max = 8.0;
    let main = free_line(
        &mut p,
        "Main",
        [0.0, 0.0],
        vec![
            shot(0.0, 5.0),
            shot(0.0, 0.2),
            shot(0.0, 9.5),
            ShotData {
                left: Some(-1.0),
                ..shot(0.0, 4.0)
            },
        ],
    );
    (p, main)
}

/// Two lines from the placement datum at A:0, joined at A:5 = B:0, 50 m
/// shots due north: 500 m of survey from the datum to B:5 (threshold 25 m).
fn two_entrances() -> (Project, LineId, LineId) {
    let mut p = Project::new("Gaps", None);
    let a = free_line(&mut p, "A", [0.0, 0.0], north(5, 50.0));
    let a5 = st(&p, a, 5);
    let b = line_from(&mut p, "B", a5, north(5, 50.0));
    let a0 = st(&p, a, 0);
    p.set_datum(a0, Some(a), origin(), None).unwrap();
    (p, a, b)
}

/// The #737 loop: Line 1 two shots east, Line 2 north / east / south from
/// Line 1's start, closed Line 2:3 to Line 1:2.
fn issue_loop() -> (Project, LineId, LineId) {
    let mut p = Project::new("737", None);
    let a = free_line(
        &mut p,
        "Line 1",
        [0.0, 0.0],
        vec![shot(90.0, 10.0), shot(90.0, 10.0)],
    );
    let a0 = st(&p, a, 0);
    let b = line_from(
        &mut p,
        "Line 2",
        a0,
        vec![shot(0.0, 10.0), shot(90.0, 20.5), shot(180.0, 10.0)],
    );
    let (b3, a2) = (st(&p, b, 3), st(&p, a, 2));
    p.create_closure(b3, None, a2, None, ClosureStrategy::Batch)
        .unwrap();
    (p, a, b)
}

/// A closure planted past the creation-time refusal, as an old file, Add
/// .stalx or a promotion can carry it.
fn plant(p: &mut Project, a: StationId, b: StationId) -> ClosureId {
    p.push_closure(a, None, b, None, ClosureStrategy::Batch)
}

// ----- the open state -----------------------------------------------------------

struct Opened {
    state: AppState,
    path: PathBuf,
    _dir: TempDir,
}

fn write(dir: &Path, name: &str, p: &Project, extras: &Extras) -> PathBuf {
    assert!(p.validate().is_empty(), "setup: {:?}", p.validate());
    let path = dir.join(name);
    std::fs::write(&path, stalx_v2::encode(p, extras, Mode::Normal).unwrap()).unwrap();
    path
}

fn open_file(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "open: {:?}", r.error);
    state
}

fn open(p: &Project, extras: &Extras) -> Opened {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "survey.stalx", p, extras);
    let state = open_file(&path);
    Opened {
        state,
        path,
        _dir: dir,
    }
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn save(state: &AppState) {
    let r = state.execute(Command::SaveProject { path: None });
    assert!(r.success, "save: {:?}", r.error);
}

/// The open project's warnings, through both doors: the IPC's (under the
/// lock, memoised layout) and a copy's (`compute_project_warnings`), which
/// must agree.
fn warnings(state: &AppState) -> Vec<ProjectWarning> {
    let open = state.open_project_warnings();
    let copy = state.compute_project_warnings(&project(state));
    assert_eq!(
        serde_json::to_value(&open).unwrap(),
        serde_json::to_value(&copy).unwrap()
    );
    open
}

fn of_code(warnings: &[ProjectWarning], code: &str) -> Vec<ProjectWarning> {
    warnings
        .iter()
        .filter(|w| w.code == code)
        .cloned()
        .collect()
}

/// The one warning of `code`.
fn only(state: &AppState, code: &str) -> ProjectWarning {
    let all = warnings(state);
    let found = of_code(&all, code);
    assert_eq!(found.len(), 1, "{all:?}");
    found.into_iter().next().unwrap()
}

/// Ignore `w` the way the resolve IPC does, from the row as the frontend
/// has it.
fn ignore(state: &AppState, w: &ProjectWarning) -> Result<(), String> {
    state.save_resolved_warning_gated(&ResolvedWarning {
        line_id: w.line_id,
        shot_id: w.shot_id,
        warning_code: w.code.clone(),
        resolved_at: Utc::now(),
        resolved_comment: Some("known".to_string()),
        shot_uuid: w.shot_uuid.clone(),
    })
}

fn unignore(state: &AppState, w: &ProjectWarning) -> Result<(), String> {
    state.delete_resolved_warning_gated(w.line_id, w.shot_id, &w.code, w.shot_uuid.as_deref())
}

/// Plant an ignore below every gate, as a file or a bulk tool could hold it.
fn plant_ignore(state: &AppState, w: &ProjectWarning) {
    state
        .save_resolved_warning(&ResolvedWarning {
            line_id: w.line_id,
            shot_id: w.shot_id,
            warning_code: w.code.clone(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: w.shot_uuid.clone(),
        })
        .unwrap();
}

/// Set (or move) the datum of `station` via `line` in the open project.
fn set_datum(
    state: &AppState,
    station: StationId,
    line: LineId,
    gps: GpsCoordinates,
    accuracy: Option<f64>,
) {
    state
        .with_project_mut(|p| {
            p.set_datum(station, Some(line), gps, accuracy)
                .map(|_| ())
                .map_err(|e| format!("{e:?}"))
        })
        .unwrap();
}

// ----- per-shot checks ------------------------------------------------------------

#[test]
fn per_shot_checks_use_the_projects_thresholds_and_name_the_shot() {
    let (p, main) = checked_line();
    let o = open(&p, &Extras::default());
    let all = warnings(&o.state);
    // Errors first, then by line and shot.
    let codes: Vec<(&str, usize)> = all
        .iter()
        .map(|w| (w.code.as_str(), w.shot_index))
        .collect();
    assert_eq!(
        codes,
        vec![("NEGATIVE_LRUD", 3), ("SHORT_SHOT", 1), ("LONG_SHOT", 2)]
    );
    let line = project(&o.state).line(main).unwrap().clone();
    for w in &all {
        let shot = &line.shots[w.shot_index];
        assert_eq!(w.line_id, main);
        assert_eq!(w.shot_id, shot.id);
        assert_eq!(w.shot_uuid.as_deref(), Some(shot.uuid.as_str()));
        assert!(!w.is_resolved);
        assert!(w.datum_station_index.is_none() && w.datum_refs.is_none());
        assert!(w.closure_id.is_none());
        let label = format!("Shot Main: {} \u{2192} {} ", w.shot_index, w.shot_index + 1);
        assert!(w.message.starts_with(&label), "{}", w.message);
    }
    assert_eq!(all[0].severity, WarningSeverity::Error);
    assert_eq!(all[1].severity, WarningSeverity::Warning);
    assert!(
        all[2].message.ends_with("above maximum threshold 8.0m"),
        "the project's own threshold: {}",
        all[2].message
    );
}

#[test]
fn an_ignore_hides_an_advisory_warning_and_never_an_error() {
    let (p, _) = checked_line();
    let o = open(&p, &Extras::default());
    let state = &o.state;
    let short = only(state, "SHORT_SHOT");
    assert!(!state.has_unsaved_changes());

    ignore(state, &short).unwrap();
    assert!(state.has_unsaved_changes(), "an ignore is survey content");
    let all = warnings(state);
    let last = all.last().unwrap();
    assert_eq!(
        (last.code.as_str(), last.is_resolved),
        ("SHORT_SHOT", true),
        "an ignored warning sorts after the open ones: {all:?}"
    );

    // An error is never hidden, whatever the file holds.
    let lrud = only(state, "NEGATIVE_LRUD");
    assert!(!warning_is_ignorable(&lrud.code));
    plant_ignore(state, &lrud);
    assert!(!only(state, "NEGATIVE_LRUD").is_resolved);

    unignore(state, &short).unwrap();
    assert!(!only(state, "SHORT_SHOT").is_resolved);
}

#[test]
fn ignores_ride_the_save_and_come_back_on_reopen() {
    let (p, main) = checked_line();
    let short_uuid = uuid_of(&p, main, 1);
    let o = open(&p, &Extras::default());
    ignore(&o.state, &only(&o.state, "SHORT_SHOT")).unwrap();

    // "Don't save" discards it (declared change: 0.6.0 wrote it at once).
    drop(o.state);
    let discarded = open_file(&o.path);
    assert!(!only(&discarded, "SHORT_SHOT").is_resolved);
    assert!(discarded.load_resolved_warnings().unwrap().is_empty());

    ignore(&discarded, &only(&discarded, "SHORT_SHOT")).unwrap();
    save(&discarded);
    assert!(!discarded.has_unsaved_changes());
    drop(discarded);

    let reopened = open_file(&o.path);
    let short = only(&reopened, "SHORT_SHOT");
    assert!(short.is_resolved);
    let rows = reopened.load_resolved_warnings().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].shot_uuid.as_deref(), Some(short_uuid.as_str()));
    assert_eq!(rows[0].warning_code, "SHORT_SHOT");
    assert_eq!((rows[0].line_id, rows[0].shot_id), (main, short.shot_id));
    assert_eq!(rows[0].resolved_comment.as_deref(), Some("known"));

    unignore(&reopened, &short).unwrap();
    assert!(reopened.has_unsaved_changes());
    save(&reopened);
    drop(reopened);
    let again = open_file(&o.path);
    assert!(!only(&again, "SHORT_SHOT").is_resolved);
    assert!(again.load_resolved_warnings().unwrap().is_empty());
}

#[test]
fn the_load_result_reads_the_opened_files_ignores() {
    // Two files of the same survey (same shot uuids), one holding an ignore:
    // the list the load returns is the opened file's, whatever was open
    // before it.
    let dir = TempDir::new().unwrap();
    let (p, main) = checked_line();
    let extras = Extras {
        ignored_warnings: vec![IgnoredWarning {
            shot: uuid_of(&p, main, 1),
            code: "SHORT_SHOT".to_string(),
            at: Utc::now(),
            comment: None,
        }],
        ..Extras::default()
    };
    let with = write(dir.path(), "with.stalx", &p, &extras);
    let without = write(dir.path(), "without.stalx", &p, &Extras::default());
    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(NullSink));
    let resolved = |result: &crate::core::LoadProjectFullResult| {
        let short = of_code(&result.warnings, "SHORT_SHOT");
        assert_eq!(short.len(), 1, "{:?}", result.warnings);
        short[0].is_resolved
    };

    let first = state.do_load_project_full(without.clone()).unwrap();
    assert!(!resolved(&first));
    let second = state.do_load_project_full(with).unwrap();
    assert!(resolved(&second), "the opened file's ignore");
    assert!(only(&state, "SHORT_SHOT").is_resolved);
    let third = state.do_load_project_full(without).unwrap();
    assert!(
        !resolved(&third),
        "no ignore leaks from the file open before"
    );
    assert!(!only(&state, "SHORT_SHOT").is_resolved);
}

#[test]
fn an_ignore_follows_its_shot_through_a_reverse() {
    let (p, main) = checked_line();
    let uuid = uuid_of(&p, main, 1);
    let o = open(&p, &Extras::default());
    let state = &o.state;
    ignore(state, &only(state, "SHORT_SHOT")).unwrap();

    let r = state.execute(Command::ReverseLine {
        line_id: main,
        swap_left_right: false,
    });
    assert!(r.success, "{:?}", r.error);
    let short = only(state, "SHORT_SHOT");
    assert_eq!(
        short.shot_index, 2,
        "the short shot is third after the reverse"
    );
    assert_eq!(short.shot_uuid.as_deref(), Some(uuid.as_str()));
    assert!(short.is_resolved);

    assert!(state.undo().success);
    let short = only(state, "SHORT_SHOT");
    assert_eq!(short.shot_index, 1);
    assert!(short.is_resolved);
}

#[test]
fn a_file_opened_read_only_refuses_an_ignore() {
    let dir = TempDir::new().unwrap();
    let (p, _) = checked_line();
    let path = write(dir.path(), "newer.stalx", &p, &Extras::default());
    let text = std::fs::read_to_string(&path).unwrap();
    let patched = text.replacen("\"requires\":[]", "\"requires\":[\"x-from-the-future\"]", 1);
    assert_ne!(patched, text);
    std::fs::write(&path, patched.as_bytes()).unwrap();
    let state = open_file(&path);

    let short = only(&state, "SHORT_SHOT");
    let refused = ignore(&state, &short).unwrap_err();
    assert!(refused.starts_with(super::READ_ONLY_FILE_CODE), "{refused}");
    assert!(!state.has_unsaved_changes());
    assert!(!only(&state, "SHORT_SHOT").is_resolved);
    assert!(unignore(&state, &short)
        .unwrap_err()
        .starts_with(super::READ_ONLY_FILE_CODE));
}

// ----- GPS gaps -------------------------------------------------------------------

#[test]
fn a_fix_past_the_threshold_gets_one_gap_warning_on_its_slot() {
    for (east, warned) in [(24.9, false), (25.1, true), (40.0, true)] {
        let (mut p, _a, b) = two_entrances();
        let b5 = st(&p, b, 5);
        p.set_datum(b5, Some(b), gps_at(east, 500.0), None).unwrap();
        let o = open(&p, &Extras::default());
        let gaps = of_code(&warnings(&o.state), DATUM_GPS_GAP);
        if !warned {
            assert!(gaps.is_empty(), "{east} m is under 25 m: {gaps:?}");
            continue;
        }
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let w = &gaps[0];
        assert_eq!(w.line_id, b);
        assert_eq!(w.datum_station_index, Some(5));
        assert_eq!(w.shot_index, 4, "station 5 is the end of shot 4");
        assert_eq!(w.shot_uuid.as_deref(), Some(uuid_of(&p, b, 4).as_str()));
        assert!(w.datum_refs.is_none() && w.closure_id.is_none());
        assert_eq!(w.severity, WarningSeverity::Warning);
        assert!(!w.is_resolved);
        assert_eq!(
            w.message,
            format!(
                "The GPS position of this zero datum is {east:.1} m from where the survey puts \
                 it (bearing 90 degrees), more than the 25.0 m expected. Check the GPS reading \
                 and the survey between the datums."
            )
        );
    }
}

#[test]
fn fixes_on_stations_0_and_1_of_one_line_are_ignored_independently() {
    // Both stations touch shot 0 of B; the codes keep their ignores apart.
    // B:0 is the junction A:5; its fix is attached via B, so its row is on
    // B's slot.
    let (mut p, a, b) = two_entrances();
    let (b0, b1) = (st(&p, b, 0), st(&p, b, 1));
    assert_eq!(b0, st(&p, a, 5));
    p.set_datum(b0, Some(b), gps_at(40.0, 250.0), None).unwrap();
    p.set_datum(b1, Some(b), gps_at(40.0, 300.0), None).unwrap();
    let o = open(&p, &Extras::default());
    let state = &o.state;
    let gap_rows = || {
        let mut rows: Vec<_> = warnings(state)
            .into_iter()
            .filter(|w| w.code == DATUM_GPS_GAP || w.code == DATUM_GPS_GAP_START)
            .map(|w| {
                (
                    w.line_id,
                    w.datum_station_index,
                    w.shot_index,
                    w.code,
                    w.is_resolved,
                )
            })
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(
        gap_rows(),
        vec![
            (b, Some(0), 0, DATUM_GPS_GAP_START.to_string(), false),
            (b, Some(1), 0, DATUM_GPS_GAP.to_string(), false),
        ]
    );
    ignore(state, &only(state, DATUM_GPS_GAP)).unwrap();
    assert_eq!(
        gap_rows(),
        vec![
            (b, Some(0), 0, DATUM_GPS_GAP_START.to_string(), false),
            (b, Some(1), 0, DATUM_GPS_GAP.to_string(), true),
        ]
    );
    ignore(state, &only(state, DATUM_GPS_GAP_START)).unwrap();
    assert!(gap_rows().iter().all(|row| row.4));
    // Each is pinned to its own datum's GPS.
    let mut codes: Vec<String> = state
        .load_resolved_warnings()
        .unwrap()
        .into_iter()
        .map(|r| r.warning_code)
        .collect();
    codes.sort();
    let mut expected = vec![
        datum_gap_ignore_code(DATUM_GPS_GAP_START, &gps_at(40.0, 250.0)),
        datum_gap_ignore_code(DATUM_GPS_GAP, &gps_at(40.0, 300.0)),
    ];
    expected.sort();
    assert_eq!(codes, expected);
}

#[test]
fn a_fix_on_a_line_without_shots_gets_no_row_but_stays_in_the_report() {
    // D starts on A:3 and has no shot, so its fix has no shot to hang on.
    let (mut p, a, _b) = two_entrances();
    let a3 = st(&p, a, 3);
    let d = line_from(&mut p, "D", a3, Vec::new());
    p.set_datum(a3, Some(d), gps_at(60.0, 150.0), None).unwrap();
    let o = open(&p, &Extras::default());
    let all = warnings(&o.state);
    assert!(
        all.iter()
            .all(|w| w.code != DATUM_GPS_GAP && w.code != DATUM_GPS_GAP_START),
        "{all:?}"
    );
    let report = o.state.get_adjustment_report().unwrap();
    assert_eq!(report.far_gps_fixes.len(), 1, "{report:?}");
    assert_eq!(report.far_gps_fixes[0].station, StationRef::new(d, 0));
}

#[test]
fn a_gap_ignore_is_pinned_to_the_datum_gps() {
    // An ignore says "this GPS reading is far off, and I know". A new reading
    // is not covered by it; the old reading again (an undo of the change) is
    // (#741 U2).
    let (mut p, _a, b) = two_entrances();
    let b5 = st(&p, b, 5);
    let far = gps_at(40.0, 500.0);
    p.set_datum(b5, Some(b), far, None).unwrap();
    let o = open(&p, &Extras::default());
    let state = &o.state;
    let gap = only(state, DATUM_GPS_GAP);
    ignore(state, &gap).unwrap();
    let rows = state.load_resolved_warnings().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].warning_code,
        datum_gap_ignore_code(DATUM_GPS_GAP, &far)
    );
    assert!(rows[0].warning_code.starts_with("datum_gps_gap@"));
    let ignored = || only(state, DATUM_GPS_GAP).is_resolved;
    assert!(ignored());

    // An accuracy-only edit keeps the reading: still ignored.
    set_datum(state, b5, b, far, Some(3.0));
    assert!(ignored());
    // A new reading, 150 m off: the warning is back.
    set_datum(state, b5, b, gps_at(150.0, 500.0), Some(3.0));
    assert!(!ignored());
    // The old reading again: ignored again.
    set_datum(state, b5, b, far, Some(3.0));
    assert!(ignored());

    // The pinned ignore rides the save.
    save(state);
    let reopened = open_file(&o.path);
    let gap = only(&reopened, DATUM_GPS_GAP);
    assert!(gap.is_resolved);
    // Un-ignoring the row (it carries the plain code) drops the pinned ignore.
    unignore(&reopened, &gap).unwrap();
    assert!(!only(&reopened, DATUM_GPS_GAP).is_resolved);
    assert!(reopened.load_resolved_warnings().unwrap().is_empty());
}

#[test]
fn every_other_code_is_stored_as_it_is() {
    let (p, main) = checked_line();
    let o = open(&p, &Extras::default());
    let short = only(&o.state, "SHORT_SHOT");
    assert_eq!(
        o.state.ignore_code_for(main, short.shot_id, "SHORT_SHOT"),
        "SHORT_SHOT"
    );
    // A gap code on a station without a datum is not pinned either.
    assert_eq!(
        o.state.ignore_code_for(main, short.shot_id, DATUM_GPS_GAP),
        DATUM_GPS_GAP
    );
}

// ----- closures across one shot ---------------------------------------------------

#[test]
fn a_closure_across_one_shot_raises_one_unignorable_error_on_its_shot() {
    let (mut p, a, _) = issue_loop();
    let (a1, a2) = (st(&p, a, 1), st(&p, a, 2));
    let id = plant(&mut p, a1, a2);
    let o = open(&p, &Extras::default());
    let state = &o.state;
    let w = only(state, CLOSURE_ACROSS_ONE_SHOT);
    assert_eq!(w.closure_id, Some(id));
    assert_eq!((w.line_id, w.shot_index), (a, 1));
    assert_eq!(w.shot_id, project(state).line(a).unwrap().shots[1].id);
    assert_eq!(w.shot_uuid.as_deref(), Some(uuid_of(&p, a, 1).as_str()));
    assert_eq!(w.severity, WarningSeverity::Error);
    assert!(!w.is_resolved);
    assert!(!warning_is_ignorable(CLOSURE_ACROSS_ONE_SHOT));
    assert_eq!(
        w.message,
        "Shot Line 1: 1 \u{2192} 2 has both ends joined by loop closure Line 1 / 1 to Line 1 / 2, \
         which would squeeze the shot to zero length, so the Closures Computed View skips that \
         closure. Remove the closure, or close the loop at two stations that are not the ends of \
         a single shot."
    );
    // Never hidden, whatever the file holds.
    plant_ignore(state, &w);
    assert!(!only(state, CLOSURE_ACROSS_ONE_SHOT).is_resolved);
    // The report the map's notice reads agrees.
    let report = state.get_adjustment_report().unwrap();
    assert_eq!(report.skipped_closures.len(), 1);
    assert_eq!(report.skipped_closures[0].closure_id, id);
}

#[test]
fn two_closures_across_the_same_shot_are_two_warnings_told_apart_by_closure_id() {
    let (mut p, a, _) = issue_loop();
    let (a1, a2) = (st(&p, a, 1), st(&p, a, 2));
    let first = plant(&mut p, a1, a2);
    let second = plant(&mut p, a2, a1);
    let o = open(&p, &Extras::default());
    let all = warnings(&o.state);
    let mut ids: Vec<Option<ClosureId>> = of_code(&all, CLOSURE_ACROSS_ONE_SHOT)
        .iter()
        .map(|w| {
            assert_eq!((w.line_id, w.shot_index), (a, 1));
            w.closure_id
        })
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![Some(first), Some(second)]);
    assert!(all
        .iter()
        .filter(|w| w.code != CLOSURE_ACROSS_ONE_SHOT)
        .all(|w| w.closure_id.is_none()));
}

#[test]
fn removing_the_closure_clears_the_warning() {
    let (mut p, a, _) = issue_loop();
    let (a1, a2) = (st(&p, a, 1), st(&p, a, 2));
    let id = plant(&mut p, a1, a2);
    let o = open(&p, &Extras::default());
    let state = &o.state;
    assert_eq!(of_code(&warnings(state), CLOSURE_ACROSS_ONE_SHOT).len(), 1);
    state
        .with_project_mut(|p| {
            p.remove_closure(id)
                .map(|_| ())
                .map_err(|e| format!("{e:?}"))
        })
        .unwrap();
    assert!(of_code(&warnings(state), CLOSURE_ACROSS_ONE_SHOT).is_empty());
}

// ----- Computed View failures -----------------------------------------------------

/// Two datum-placed lines X and Y tied datum to datum by a closure (which
/// creation allows): the solve cannot adjust them.
fn unsolvable() -> (Project, LineId, LineId) {
    let mut p = Project::new("Unsolvable", None);
    let x = free_line(&mut p, "X", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let y = free_line(&mut p, "Y", [30.0, 30.0], vec![shot(0.0, 10.0)]);
    let (x0, y0) = (st(&p, x, 0), st(&p, y, 0));
    p.set_datum(x0, Some(x), origin(), None).unwrap();
    p.set_datum(y0, Some(y), gps_at(15.0, 15.0), None).unwrap();
    p.push_closure(x0, Some(x), y0, Some(y), ClosureStrategy::Batch);
    (p, x, y)
}

#[test]
fn a_computed_view_failure_is_reported_only_from_a_current_solve() {
    let (p, x, y) = unsolvable();
    let o = open(&p, &Extras::default());
    let state = &o.state;

    // Before any solve the failure is unknown: the warnings pass never solves.
    assert!(of_code(&warnings(state), CLOSURE_ADJUSTMENT_FAILED).is_empty());

    // The view solves; now it is reported, in plain words, on X's first shot
    // (the solve is held at X:0, the first placement datum).
    state.get_adjusted_stations().unwrap();
    let w = only(state, CLOSURE_ADJUSTMENT_FAILED);
    assert_eq!((w.line_id, w.shot_index), (x, 0));
    assert_eq!(w.shot_uuid.as_deref(), Some(uuid_of(&p, x, 0).as_str()));
    assert_eq!(w.severity, WarningSeverity::Warning);
    assert!(warning_is_ignorable(CLOSURE_ADJUSTMENT_FAILED));
    assert!(!w.is_resolved && w.closure_id.is_none());
    assert_eq!(
        w.message,
        "Shot X: 0 \u{2192} 1 is in a part of the survey (2 lines) the Closures Computed View \
         could not adjust, so that part is shown as measured: loop closures tie two zero \
         datums to the same point, and both positions are fixed. Remove one of those closures \
         or one of the datums."
    );
    let report = state.get_adjustment_report().unwrap();
    assert_eq!(report.failures.len(), 1);
    let mut lines = vec![x, y];
    lines.sort_unstable();
    assert_eq!(report.failures[0].line_ids, lines);

    // Advisory: an ignore on its shot is honoured.
    ignore(state, &w).unwrap();
    assert!(only(state, CLOSURE_ADJUSTMENT_FAILED).is_resolved);

    // Only the solve of this revision under this config counts.
    let key = state.adjusted_config_key();
    assert_eq!(state.memoised_adjustment_failures(key).len(), 1);
    assert!(state
        .memoised_adjustment_failures(key.wrapping_add(1))
        .is_empty());
    let mut older = project(state);
    older.touch();
    assert!(
        of_code(
            &state.compute_project_warnings(&older),
            CLOSURE_ADJUSTMENT_FAILED
        )
        .is_empty(),
        "another version of the project never borrows the cached failure"
    );

    // An edit drops the cached solve, and with it the failure, until the view
    // solves again; the ignore still holds then.
    let shot_id = project(state).line(y).unwrap().shots[0].id;
    let r = state.execute(Command::UpdateShot {
        line_id: y,
        shot_id,
        updates: ShotUpdates {
            length: Some(11.0),
            ..ShotUpdates::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    assert!(of_code(&warnings(state), CLOSURE_ADJUSTMENT_FAILED).is_empty());
    state.get_adjusted_stations().unwrap();
    assert!(only(state, CLOSURE_ADJUSTMENT_FAILED).is_resolved);
}

// ----- command refusals (0.6.0 "Validation Tests" / "Edge Case Tests") --------------

#[test]
#[ignore = "C3: Create Line is not ported on this branch"]
fn create_line_refuses_a_zero_length_shot() {
    let (p, _) = checked_line();
    let o = open(&p, &Extras::default());
    let result = o.state.execute(Command::CreateLine {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 0.0,
            depth_in: 10.0,
            depth_out: 10.0,
            ..ShotData::default()
        }],
        cave_section_id: None,
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("zero length"));
}

#[test]
fn every_command_fails_without_an_open_project() {
    let state = AppState::new().unwrap();
    let commands = vec![
        Command::CreateLine {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
            cave_section_id: None,
        },
        Command::DeleteLine { line_id: 0 },
        Command::UpdateLineMetadata {
            line_id: 0,
            updates: LineUpdates::default(),
        },
        Command::CreateShot {
            line_id: 0,
            shot: shot(0.0, 5.0),
            after_shot_id: None,
            before_shot_id: None,
        },
        Command::UpdateShot {
            line_id: 0,
            shot_id: 0,
            updates: ShotUpdates::default(),
        },
        Command::DeleteShot {
            line_id: 0,
            shot_id: 0,
        },
        Command::MergeStations {
            station_a: StationRef::new(0, 0),
            station_b: StationRef::new(1, 0),
        },
        Command::SetZeroDatum {
            station_ref: StationRef::new(0, 0),
            coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            accuracy_m: None,
            entrance: None,
        },
    ];
    for cmd in commands {
        let label = format!("{cmd:?}");
        let result = state.execute(cmd);
        assert!(!result.success, "{label} must fail with no project open");
        assert!(result.error.is_some(), "{label} must say why");
    }
    assert!(state.open_project_warnings().is_empty());
    assert!(state.load_resolved_warnings().is_err());
}

#[test]
fn commands_naming_a_missing_line_or_shot_fail() {
    let (p, main) = checked_line();
    let o = open(&p, &Extras::default());
    let state = &o.state;
    let before = project(state);
    let refused = [
        Command::DeleteLine { line_id: 999 },
        Command::UpdateLineMetadata {
            line_id: 999,
            updates: LineUpdates::default(),
        },
        Command::UpdateShot {
            line_id: main,
            shot_id: 999_999,
            updates: ShotUpdates::default(),
        },
        Command::DeleteShot {
            line_id: main,
            shot_id: 999_999,
        },
    ];
    for cmd in refused {
        let label = format!("{cmd:?}");
        let result = state.execute(cmd);
        assert!(!result.success, "{label} must fail");
        assert!(result.error.is_some(), "{label} must say why");
    }
    assert_eq!(project(state), before, "nothing changed");
    assert!(!state.has_unsaved_changes());
}

// ----- cost -----------------------------------------------------------------------

/// The warnings pass on 80,000 shots (200 lines of 400 joined end to end,
/// a datum, a far GPS fix, a closure and one short shot per line), cold and
/// warm (the memoised layout). Run by hand:
/// `cargo test -p stalagmight-core --lib warnings_pass_cost -- --ignored --nocapture`.
#[test]
#[ignore = "timing run: prints the cost of a whole-project warnings pass"]
#[expect(clippy::print_stdout, reason = "the timing run reports its numbers")]
fn warnings_pass_cost() {
    let mut p = Project::new("Cost", None);
    let mut shots = north(400, 5.0);
    shots[7].length = 0.2;
    let first = free_line(&mut p, "L0", [0.0, 0.0], shots.clone());
    let mut last = first;
    for i in 1..200 {
        let end = st(&p, last, 400);
        last = line_from(&mut p, &format!("L{i}"), end, shots.clone());
    }
    p.set_datum(st(&p, first, 0), Some(first), origin(), None)
        .unwrap();
    let far = st(&p, last, 400);
    p.set_datum(far, Some(last), gps_at(20_000.0, 400_000.0), None)
        .unwrap();
    let (a, b) = (st(&p, first, 3), st(&p, first, 9));
    plant(&mut p, a, b);
    let state = AppState::new().unwrap();
    state.set_project_for_tests(p);

    let started = std::time::Instant::now();
    let cold = state.open_project_warnings();
    let cold_ms = started.elapsed().as_secs_f64() * 1e3;
    let started = std::time::Instant::now();
    let warm = state.open_project_warnings();
    let warm_ms = started.elapsed().as_secs_f64() * 1e3;
    assert_eq!(cold.len(), warm.len());
    assert_eq!(of_code(&warm, "SHORT_SHOT").len(), 200);
    assert_eq!(of_code(&warm, DATUM_GPS_GAP).len(), 1);
    println!(
        "warnings pass, 80,000 shots, {} warnings: cold {cold_ms:.1} ms, warm {warm_ms:.1} ms",
        warm.len()
    );
}
