//! What a merge does to depths and positions, said before and after it
//! (demo issue I-4). The Merge dialog's preview (`check_merge_connectivity`)
//! names the piece that moves and how far, and, when the two stations' depths
//! differ by more than `JUNCTION_DEPTH_SPREAD_M`, the depths and the joined
//! station's; that joined depth is what the merge then makes and what 0.6.0
//! made (the frozen crate: the mean of every reading, not of the two
//! stations). A junction whose readings disagree is listed in the Warnings
//! tab, advisory and ignorable. Synthetic data only.

use std::path::{Path, PathBuf};

use chrono::Utc;
use tempfile::TempDir;

use super::legacy_fixture_kit::{frozen, old, slot, write_legacy_file, Legacy};
use super::AppState;
use crate::core::Command;
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{
    LineId, ProjectWarning, ShotData, StationRef, JUNCTION_DEPTH_SPREAD, JUNCTION_DEPTH_SPREAD_M,
    JUNCTION_DEPTH_SPREAD_START,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};
use crate::persistence::ResolvedWarning;

const TOL: f64 = 1e-9;

fn shot(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..ShotData::default()
    }
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().expect("state");
    let opened = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(opened.success, "{:?}", opened.error);
    state
}

/// `p` written as a version 2 file in `dir` and opened.
fn open_v2(dir: &Path, p: &Project) -> AppState {
    assert!(p.validate().is_empty(), "setup: {:?}", p.validate());
    let path: PathBuf = dir.join("survey.stalx");
    let bytes = stalx_v2::encode(p, &Extras::default(), Mode::Normal).expect("encode");
    std::fs::write(&path, bytes).expect("write");
    open(&path)
}

/// The depth the open project shows at `slot`.
fn depth_at(state: &AppState, slot: StationRef) -> f64 {
    let project = state.get_project_state().expect("a project");
    project
        .stations_wire(&project.raw_positions())
        .iter()
        .find(|s| s.station_ref == slot)
        .map(|s| s.coordinates.z)
        .expect("the slot is shown")
}

fn merge(state: &AppState, a: StationRef, b: StationRef) {
    let merged = state.execute(Command::MergeStations {
        station_a: a,
        station_b: b,
    });
    assert!(merged.success, "{:?}", merged.error);
}

/// B: two shots north whose middle station B:1 reads 15.0 m on both; C: one
/// shot east from a free start reading 4.0 m. Merging C:0 onto B:1 joins
/// three readings: 0.6.0 took their mean (11.333 m), not the mean of the two
/// stations' depths (9.5 m). The preview says what the merge then makes, and
/// both are 0.6.0's figure. Injection: make the preview average the two
/// stations' depths and it reads 9.5 m against 0.6.0's 11.333 m.
#[test]
fn the_previewed_merged_depth_is_the_one_the_merge_and_0_6_0_make() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().expect("dir");
    let mut legacy = Legacy::new("Depths".to_owned(), None);
    let mut add = |shots: Vec<ShotData>| {
        legacy
            .add_line(old::LineData {
                shots: shots.iter().map(frozen).collect(),
                quality: 1.0,
                ..Default::default()
            })
            .id
    };
    let b = add(vec![
        shot(0.0, 10.0, 0.0, 15.0),
        shot(0.0, 10.0, 15.0, 20.0),
    ]);
    let c = add(vec![shot(90.0, 10.0, 4.0, 6.0)]);
    let path = dir.path().join("depths.stalx");
    write_legacy_file(&path, &legacy);

    // 0.6.0's merge of the same pair, in the frozen model.
    let mut merged_060 = legacy.clone();
    merged_060
        .merge_stations(slot(c, 0), slot(b, 1))
        .expect("0.6.0 merges");
    let z_060 = merged_060
        .compute_station_coordinates()
        .iter()
        .find(|s| s.station_ref == slot(b, 1))
        .map(|s| s.coordinates.z)
        .expect("0.6.0 shows B:1");
    assert!((z_060 - 34.0 / 3.0).abs() < TOL, "{z_060}");

    let state = open(&path);
    let (c0, b1) = (StationRef::new(c, 0), StationRef::new(b, 1));
    let check = state.check_merge_connectivity(c0, b1).expect("a check");
    assert!(check.can_merge);
    let preview = check.merge_preview.expect("a merge preview");
    let depths = preview.depths.expect("4.0 m and 15.0 m differ");
    assert!((depths.a_m - 4.0).abs() < TOL, "{depths:?}");
    assert!((depths.b_m - 15.0).abs() < TOL, "{depths:?}");
    assert!(
        (depths.joined_m - z_060).abs() < TOL,
        "{depths:?} vs {z_060}"
    );

    merge(&state, c0, b1);
    assert!((depth_at(&state, b1) - z_060).abs() < TOL);
}

/// Two free pieces: A runs east 2 x 10 m from (0, 0) at 2.0 m, B north
/// 2 x 5 m from (100, 50). Merging A:2 with B:0 moves A (neither is placed by
/// a datum, so the first pick's piece moves) by (80, 50) onto B:0, and the
/// preview says so: A's lines, the distance, the station it joins. Depths
/// within the threshold are not mentioned; past it they are. Injection:
/// measure the shift from the stationary piece and it reads 0 m.
#[test]
fn the_merge_preview_says_which_piece_moves_how_far_and_onto_what() {
    crate::app_dirs::init_for_test();
    let mut p = Project::new("Pieces", None);
    let a = p
        .add_line(
            LineMeta::default(),
            StartAt::New,
            vec![shot(90.0, 10.0, 2.0, 2.0), shot(90.0, 10.0, 2.0, 2.0)],
        )
        .expect("A");
    let b = p
        .add_line(
            LineMeta::default(),
            StartAt::New,
            vec![shot(0.0, 5.0, 2.4, 2.4), shot(0.0, 5.0, 2.4, 2.4)],
        )
        .expect("B");
    for (line, xy) in [(a, [0.0, 0.0]), (b, [100.0, 50.0])] {
        let start = p.line(line).expect("line").start;
        p.set_placement(start, xy[0], xy[1]).expect("placement");
    }
    let dir = TempDir::new().expect("dir");
    let state = open_v2(dir.path(), &p);
    let (a2, b0) = (StationRef::new(a, 2), StationRef::new(b, 0));

    let preview = state
        .check_merge_connectivity(a2, b0)
        .expect("a check")
        .merge_preview
        .expect("a merge preview");
    assert_eq!(preview.moving_line_ids, vec![a]);
    assert_eq!(preview.joins, b0);
    assert!(
        (preview.shift_m - 80.0_f64.hypot(50.0)).abs() < 1e-6,
        "{preview:?}"
    );
    assert_eq!(preview.depths, None, "0.4 m apart: within the threshold");
    // Read-only: the check changed nothing.
    assert!(!state.has_unsaved_changes());

    // Past the threshold the depths are said.
    let deeper = state.execute(Command::UpdateShot {
        line_id: b,
        shot_id: state
            .get_project_state()
            .expect("project")
            .line(b)
            .expect("B")
            .shots[0]
            .id,
        updates: crate::models::ShotUpdates {
            depth_in: Some(2.0 + JUNCTION_DEPTH_SPREAD_M + 0.1),
            ..crate::models::ShotUpdates::default()
        },
    });
    assert!(deeper.success, "{:?}", deeper.error);
    let depths = state
        .check_merge_connectivity(a2, b0)
        .expect("a check")
        .merge_preview
        .and_then(|m| m.depths)
        .expect("0.6 m apart");
    assert!((depths.a_m - 2.0).abs() < TOL && (depths.b_m - 2.6).abs() < TOL);
    assert!((depths.joined_m - 2.3).abs() < TOL, "{depths:?}");
}

/// The warnings of the open project with `code`.
fn of_code(state: &AppState, code: &str) -> Vec<ProjectWarning> {
    state
        .open_project_warnings()
        .into_iter()
        .filter(|w| w.code == code)
        .collect()
}

/// After a merge of stations reading 15.0 m and 4.0 m the junction is listed
/// in the Warnings tab (advisory), naming it and its readings, on the shot
/// arriving at it; an Ignore hides it, and readings within the threshold
/// raise none. A junction every line starts at hangs it on a leaving shot,
/// under its own code. Injection: drop `junction_depth_spread_warnings` from
/// `project_warnings` and no row appears.
#[test]
fn a_junction_whose_readings_disagree_is_a_warning() {
    crate::app_dirs::init_for_test();
    let mut p = Project::new("Junction", None);
    let far: LineId = p
        .add_line(
            LineMeta {
                name: Some("Far Room".to_owned()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![shot(0.0, 10.0, 10.0, 14.0), shot(0.0, 10.0, 14.0, 15.0)],
        )
        .expect("Far Room");
    let up = p
        .add_line(
            LineMeta {
                name: Some("Upstream".to_owned()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![shot(90.0, 10.0, 4.0, 5.0)],
        )
        .expect("Upstream");
    for (line, xy) in [(far, [0.0, 0.0]), (up, [50.0, 50.0])] {
        let start = p.line(line).expect("line").start;
        p.set_placement(start, xy[0], xy[1]).expect("placement");
    }
    let dir = TempDir::new().expect("dir");
    let state = open_v2(dir.path(), &p);
    assert!(of_code(&state, JUNCTION_DEPTH_SPREAD).is_empty());

    merge(&state, StationRef::new(up, 0), StationRef::new(far, 2));
    let rows = of_code(&state, JUNCTION_DEPTH_SPREAD);
    assert_eq!(rows.len(), 1, "{:?}", state.open_project_warnings());
    let row = &rows[0];
    assert_eq!((row.line_id, row.shot_index), (far, 1), "the arriving shot");
    assert_eq!(row.datum_station_index, Some(2), "Go to station finds it");
    assert!(!row.is_resolved);
    assert_eq!(row.severity, crate::models::WarningSeverity::Warning);
    assert!(
        row.message.contains("Far Room / 2")
            && row.message.contains("differ by 11.0 m (4.0 m to 15.0 m)")
            && row.message.contains("their mean, 9.5 m"),
        "{}",
        row.message
    );

    state
        .save_resolved_warning_gated(&ResolvedWarning {
            line_id: row.line_id,
            shot_id: row.shot_id,
            warning_code: row.code.clone(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: row.shot_uuid.clone(),
        })
        .expect("an advisory warning is ignorable");
    assert!(of_code(&state, JUNCTION_DEPTH_SPREAD)[0].is_resolved);

    // Two lines that both START at one station, readings 1 m apart: no shot
    // arrives there, so the row is on a leaving shot under the `_START` code.
    let mut p = Project::new("Starts", None);
    let main = p
        .add_line(
            LineMeta::default(),
            StartAt::New,
            vec![shot(0.0, 10.0, 3.0, 3.0)],
        )
        .expect("main");
    let hub = p.line(main).expect("main").start;
    p.set_placement(hub, 0.0, 0.0).expect("placement");
    let side = p
        .add_line(
            LineMeta::default(),
            StartAt::Station(hub),
            vec![shot(90.0, 10.0, 4.0, 4.0)],
        )
        .expect("side");
    let dir = TempDir::new().expect("dir");
    let state = open_v2(dir.path(), &p);
    let rows = of_code(&state, JUNCTION_DEPTH_SPREAD_START);
    assert_eq!(rows.len(), 1, "{:?}", state.open_project_warnings());
    assert_eq!(
        (rows[0].shot_index, rows[0].datum_station_index),
        (0, Some(0))
    );
    assert!([main, side].contains(&rows[0].line_id));

    // Readings within the threshold: nothing.
    let mut p = Project::new("Close", None);
    let main = p
        .add_line(
            LineMeta::default(),
            StartAt::New,
            vec![shot(0.0, 10.0, 3.0, 3.0)],
        )
        .expect("main");
    let hub = p.line(main).expect("main").start;
    p.set_placement(hub, 0.0, 0.0).expect("placement");
    p.add_line(
        LineMeta::default(),
        StartAt::Station(hub),
        vec![shot(90.0, 10.0, 3.0 + JUNCTION_DEPTH_SPREAD_M, 3.0)],
    )
    .expect("side");
    let dir = TempDir::new().expect("dir");
    let state = open_v2(dir.path(), &p);
    assert!(of_code(&state, JUNCTION_DEPTH_SPREAD_START).is_empty());
    assert!(of_code(&state, JUNCTION_DEPTH_SPREAD).is_empty());
}

/// Demo finding V-2: merging onto a station that is already a junction. Main
/// ends at Main:2 reading 12.5 m, Far (12.5 m at Far:0) is already merged
/// there, and Up:0 reads 4.0 m. The joined station takes the mean of THREE
/// readings (9.667 m), so the preview lists every one by its line, Up's first
/// (station A), then Main's (station B) and Far's, and says Main:2 is already
/// where Main and Far meet. Injection: list only the two picked stations'
/// depths (4.0 m and 12.5 m, whose mean is 8.25 m) and the readings do not
/// add up to the joined depth.
#[test]
fn a_merge_onto_a_junction_lists_every_reading_and_says_it_is_one() {
    crate::app_dirs::init_for_test();
    let mut p = Project::new("Junction depths", None);
    let add = |p: &mut Project, shots: Vec<ShotData>, xy: [f64; 2]| {
        let line = p
            .add_line(LineMeta::default(), StartAt::New, shots)
            .expect("line");
        let start = p.line(line).expect("line").start;
        p.set_placement(start, xy[0], xy[1]).expect("placement");
        line
    };
    let main = add(
        &mut p,
        vec![shot(0.0, 10.0, 2.0, 8.0), shot(0.0, 10.0, 8.0, 12.5)],
        [0.0, 0.0],
    );
    let far = add(&mut p, vec![shot(90.0, 10.0, 12.5, 14.0)], [100.0, 0.0]);
    let up = add(&mut p, vec![shot(270.0, 10.0, 4.0, 3.0)], [-100.0, 0.0]);
    let dir = TempDir::new().expect("dir");
    let state = open_v2(dir.path(), &p);
    let (main2, far0, up0) = (
        StationRef::new(main, 2),
        StationRef::new(far, 0),
        StationRef::new(up, 0),
    );
    merge(&state, far0, main2);

    let preview = state
        .check_merge_connectivity(up0, main2)
        .expect("a check")
        .merge_preview
        .expect("a merge preview");
    let depths = preview.depths.expect("4.0 m and 12.5 m differ");
    let readings: Vec<(LineId, f64)> = depths
        .readings
        .iter()
        .map(|r| (r.line_id, r.depth_m))
        .collect();
    assert_eq!(readings, vec![(up, 4.0), (main, 12.5), (far, 12.5)]);
    let mean = readings.iter().map(|(_, d)| d).sum::<f64>() / readings.len() as f64;
    assert!((depths.joined_m - mean).abs() < TOL, "{depths:?}");
    assert!((depths.joined_m - 29.0 / 3.0).abs() < TOL, "{depths:?}");
    assert_eq!(
        preview.junctions,
        vec![crate::models::MergeJunction {
            station: main2,
            line_ids: vec![main, far],
        }]
    );

    merge(&state, up0, main2);
    assert!((depth_at(&state, main2) - depths.joined_m).abs() < TOL);

    // Two plain stations: no junction to mention.
    let preview = state
        .check_merge_connectivity(StationRef::new(main, 0), StationRef::new(far, 1))
        .ok()
        .and_then(|c| c.merge_preview);
    assert!(preview.is_none_or(|m| m.junctions.is_empty()));
}
