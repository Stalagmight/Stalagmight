//! Every zero datum is an entrance in the Ariane export (#741 decision 5).
//!
//! Ariane keeps GPS on START rows only and reads every START's lat/lon as a
//! hard fix. A GPS fix part-way through the survey has no START of its own, so
//! the exporter writes one at the fix's GPS and ties it to the fix station with
//! a zero-length CLOSURE. Synthetic data only, built on the station graph with
//! the model's own operations (`Survey`).

use super::datums::{stamp_starts_at, start_shot, DatumNodes};
use super::survey::{build_rows, export_tml};
use super::*;
use crate::export::survey::test_helpers::{meta, Survey};
use crate::models::survey::{ClosureStrategy, Project, ZeroDatum};
use crate::models::{DatumRole, GpsCoordinates, LineId, ShotData, StationId, StationRef};

const PLACEMENT: (f64, f64) = (20.0, -87.5);
const FIX_AT_LINE_START: (f64, f64) = (20.0003, -87.4995);
const FIX_MID_LINE: (f64, f64) = (20.0006, -87.4985);

fn gps((lat, lon): (f64, f64)) -> GpsCoordinates {
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

fn shots(count: usize, azimuth_in: f64) -> Vec<ShotData> {
    (0..count)
        .map(|i| ShotData {
            length: 50.0,
            azimuth_in,
            azimuth_out: azimuth_in,
            depth_in: i as f64,
            depth_out: (i + 1) as f64,
            ..Default::default()
        })
        .collect()
}

fn station(project: &Project, line: LineId, index: u32) -> StationId {
    project.station_at(StationRef::new(line, index)).unwrap()
}

fn gps_fixes(project: &Project) -> usize {
    project.datums_with_role(DatumRole::GpsFix).len()
}

/// The lines of [`three_entrance_cave`].
struct Cave {
    project: Project,
    main: LineId,
    side: LineId,
    lp: LineId,
}

/// Main runs 4 x 50 m east (M0..M4). Side runs 3 x 50 m north from M2 (it
/// starts on that station), and Loop runs 4 x 50 m north from M4. One
/// connected cave with a placement datum on M0, a GPS fix on S0 (a line
/// start, which is the junction M2) and a GPS fix on L2 (the middle of a
/// line).
fn three_entrance_cave(with_fixes: bool) -> Cave {
    let mut s = Survey::new("Three entrances");
    let main = s.named("Main", "#3366ff", shots(4, 90.0));
    let side = s.branch(meta("Side", "#3366ff"), (main, 2), shots(3, 0.0));
    let lp = s.branch(meta("Loop", "#3366ff"), (main, 4), shots(4, 0.0));
    s.datum((main, 0), gps(PLACEMENT));
    if with_fixes {
        s.datum((side, 0), gps(FIX_AT_LINE_START));
        s.datum((lp, 2), gps(FIX_MID_LINE));
    }
    let project = s.build();
    if with_fixes {
        assert_eq!(
            gps_fixes(&project),
            2,
            "fixture rot: both later datums must be GPS fixes, or this proves nothing"
        );
    }
    Cave {
        project,
        main,
        side,
        lp,
    }
}

fn export(project: &Project, profile: TmlProfile) -> (Vec<ExportShot>, usize) {
    let decision = AzimuthExport::decide_from(
        project
            .lines()
            .iter()
            .filter(|line| !line.shots.is_empty())
            .map(|line| project.azimuth_reference_for(line)),
    );
    let (rows, left_out) = build_rows(project, profile, decision);
    (rows, left_out.datums)
}

fn uuid_of(project: &Project, line: LineId, shot: usize) -> Option<String> {
    project.line(line).map(|l| l.shots[shot].uuid.clone())
}

fn start_at(shots: &[ExportShot], (lat, lon): (f64, f64)) -> Vec<&ExportShot> {
    shots
        .iter()
        .filter(|s| {
            s.shot_type == "START"
                && (s.latitude - lat).abs() < 1e-12
                && (s.longitude - lon).abs() < 1e-12
        })
        .collect()
}

/// The fix START at `gps`, the CLOSURE tying it in, and the row it is tied to.
fn fix_shape(shots: &[ExportShot], gps: (f64, f64)) -> (&ExportShot, &ExportShot, &ExportShot) {
    let starts = start_at(shots, gps);
    assert_eq!(starts.len(), 1, "exactly one START carries {gps:?}");
    let start = starts[0];
    let ties: Vec<&ExportShot> = shots
        .iter()
        .filter(|s| s.shot_type == "CLOSURE" && s.closure_to_id == start.id)
        .collect();
    assert_eq!(ties.len(), 1, "exactly one CLOSURE ties the fix START in");
    let tie = ties[0];
    let row = shots.iter().find(|s| s.id == tie.from_id).unwrap();
    (start, tie, row)
}

#[test]
fn every_datum_gets_a_start_carrying_its_own_gps() {
    let cave = three_entrance_cave(true);
    let (shots, unwritten) = export(&cave.project, TmlProfile::User);

    assert_eq!(unwritten, 0, "every datum is written");
    for gps in [PLACEMENT, FIX_AT_LINE_START, FIX_MID_LINE] {
        assert_eq!(start_at(&shots, gps).len(), 1, "one START at {gps:?}");
    }
    assert_eq!(
        shots.iter().filter(|s| s.shot_type == "START").count(),
        3,
        "one START per datum and no other"
    );
    // Only the placement START roots a tree; the fix STARTs are leaves.
    let placement = start_at(&shots, PLACEMENT)[0];
    assert!(shots.iter().any(|s| s.from_id == placement.id));
}

#[test]
fn a_fix_start_is_tied_to_its_station_by_a_zero_length_closure() {
    let cave = three_entrance_cave(true);
    let (shots, _) = export(&cave.project, TmlProfile::User);

    // S0 is the junction M2, so its node is the Main row that ends at M2.
    let (start, tie, row) = fix_shape(&shots, FIX_AT_LINE_START);
    assert_eq!(row.uuid, uuid_of(&cave.project, cave.main, 1));
    // L2 is the end of Loop's second shot.
    let (mid_start, mid_tie, mid_row) = fix_shape(&shots, FIX_MID_LINE);
    assert_eq!(mid_row.uuid, uuid_of(&cave.project, cave.lp, 1));

    for (start, tie, row) in [(start, tie, row), (mid_start, mid_tie, mid_row)] {
        assert_eq!(row.shot_type, "REAL");
        assert_eq!((start.from_id, start.length), (-1, 0.0));
        assert!(start.locked);
        assert!(start.uuid.is_none() && tie.uuid.is_none());
        assert_eq!(
            start.depth, row.depth,
            "the START sits at its station's depth"
        );
        assert_eq!(
            (tie.length, tie.depth_in, tie.depth),
            (0.0, row.depth, row.depth)
        );
        assert_eq!(tie.line, row.line);
        // Both are minted after every other row, so nothing hangs off them.
        assert!(shots
            .iter()
            .all(|s| s.from_id != start.id && s.from_id != tie.id));
    }
}

/// 0.6.0's orphan shape - A, then a shot-less line E bridging a merge from A1
/// to B0, so E had no row and B's first shot was left parentless - cannot
/// happen on the station graph: E, A and B meet at one station, and B hangs
/// off A's row for it. The fix on A1 is written.
#[test]
fn a_shotless_line_at_a_junction_leaves_no_orphan_and_the_fix_is_written() {
    let mut s = Survey::new("Bridged");
    let a = s.named("A", "#3366ff", shots(2, 90.0));
    s.branch(meta("E", "#3366ff"), (a, 1), Vec::new());
    s.branch(meta("B", "#3366ff"), (a, 1), shots(2, 90.0));
    s.datum((a, 0), gps(PLACEMENT));
    s.datum((a, 1), gps(FIX_MID_LINE));
    let project = s.build();
    assert_eq!(gps_fixes(&project), 1, "fixture rot: A1 is a fix");

    let (shots, unwritten) = export(&project, TmlProfile::User);
    assert!(
        !shots.iter().any(is_orphan_root),
        "B hangs off the junction"
    );
    assert_eq!(unwritten, 0);
    fix_shape(&shots, FIX_MID_LINE);
    let dir = tempfile::TempDir::new().unwrap();
    assert!(export_tml(&project, &dir.path().join("cave.tml"))
        .unwrap()
        .is_empty());
}

/// A survey row: `id`, hanging off `from_id`, 50 m on 90 degrees at depth 0.
fn plain_row(id: i64, from_id: i64) -> ExportShot {
    let mut row = start_shot(
        id,
        "A".to_string(),
        0.0,
        None,
        None,
        "0x3366ffff".to_string(),
    );
    if from_id != -1 {
        row.shot_type = "REAL";
        row.from_id = from_id;
        row.length = 50.0;
        row.azimuth = 90.0;
        row.locked = false;
        row.comment = None;
    }
    row
}

#[test]
fn an_orphan_with_a_fix_and_no_closures_still_exports() {
    // Ariane blanks its map for an orphan root alongside ANY CLOSURE row, and
    // a fix's tie is one. So the fix is left out and counted, rather than the
    // export failing over "closures" the user never made. The station-graph
    // walk produces no orphan (see above), so the rows are built by hand: A's
    // START (0), A1 (1), and a parentless B (3, 4).
    let mut rows = vec![
        plain_row(0, -1),
        plain_row(1, 0),
        plain_row(3, -1),
        plain_row(4, 3),
    ];
    rows[2].shot_type = "REAL";
    rows[2].locked = false;
    assert!(
        rows.iter().any(is_orphan_root),
        "fixture rot: B is orphaned"
    );
    let nodes = DatumNodes {
        gps: vec![gps(PLACEMENT), gps(FIX_MID_LINE)],
        nodes: vec![Some(0), Some(1)],
        frame: Some(0),
        frame_xy: Some((0.0, 0.0)),
    };
    let stamped = stamp_starts_at(nodes, |_| Some((0.0, 0.0)), &mut rows, TmlProfile::User);
    let unwritten = stamped.append_fix_starts(&mut rows, 10);

    assert!(rows.iter().all(|s| s.shot_type != "CLOSURE"));
    assert!(start_at(&rows, FIX_MID_LINE).is_empty());
    assert_eq!(unwritten, 1, "the left-out fix is reported");
    assert!(
        ensure_rooted_chain(&rows, TmlProfile::User).is_ok(),
        "an orphan with no closure must still export, as it did before fixes"
    );

    // A real closure still takes the refusal, and it names the user's closures.
    let mut closure = plain_row(20, 4);
    closure.shot_type = "CLOSURE";
    closure.closure_to_id = 1;
    rows.push(closure);
    let refused = ensure_rooted_chain(&rows, TmlProfile::User);
    assert!(refused.unwrap_err().to_string().contains("loop closures"));
}

#[test]
fn a_fix_start_sits_at_its_rows_repaired_depth() {
    // The geometry repair re-hangs Loop onto M4 (depth 4) because L0 was
    // surveyed at depth 0, so the L2 row is written 4 m deeper than the
    // survey says. The fix rows are appended AFTER the repair, so the START
    // takes the written depth; appended before it, the START would keep the
    // survey's depth and the zero-length tie would measure a 4 m drop.
    let cave = three_entrance_cave(true);
    let surveyed = cave.project.line(cave.lp).unwrap().shots[1].depth_out;
    let (shots, _) = export(&cave.project, TmlProfile::User);
    let (start, tie, row) = fix_shape(&shots, FIX_MID_LINE);
    assert_eq!(row.uuid, uuid_of(&cave.project, cave.lp, 1));
    assert_eq!(
        (surveyed, row.depth),
        (2.0, 6.0),
        "fixture rot: the repair must move the fix row, or this proves nothing"
    );
    assert_eq!(
        start.depth, row.depth,
        "the START sits at the repaired depth"
    );
    assert_eq!((tie.depth_in, tie.depth, tie.length), (6.0, 6.0, 0.0));
}

#[test]
fn the_sidecar_leaves_the_fixes_out_and_counts_them() {
    let cave = three_entrance_cave(true);
    let (shots, unwritten) = export(&cave.project, TmlProfile::SpeleoDbSidecar);
    assert_eq!(shots.iter().filter(|s| s.shot_type == "START").count(), 1);
    assert!(shots.iter().all(|s| s.shot_type != "CLOSURE"));
    assert_eq!(unwritten, 2);
}

#[test]
fn two_datums_on_one_station_still_count_one_as_unwritten() {
    // M2 is S0: two GPS positions for one point is a conflict the file
    // cannot carry, so the later datum is reported, not written as a START
    // pinned somewhere the survey disagrees with.
    let mut cave = three_entrance_cave(true);
    let m2 = station(&cave.project, cave.main, 2);
    cave.project.datums.push(ZeroDatum {
        via: Some(cave.main),
        ..ZeroDatum::new(m2, gps((20.1, -87.1)))
    });
    assert!(cave.project.validate().is_empty());
    let (shots, unwritten) = export(&cave.project, TmlProfile::User);
    assert_eq!(unwritten, 1);
    assert!(start_at(&shots, (20.1, -87.1)).is_empty());
    assert_eq!(start_at(&shots, FIX_AT_LINE_START).len(), 1);
}

#[test]
fn the_fix_shape_round_trips_to_the_same_stations() {
    let read_back = |project: &Project| {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("cave.tml");
        let notes = export_tml(project, &path).unwrap();
        assert!(
            !notes.iter().any(|n| n == "gps-anchors-not-written"),
            "every datum is written: {notes:?}"
        );
        let parsed = crate::parsers::tml::parse_tml(&path).unwrap();
        let (configs, closures, warnings) =
            crate::parsers::tml::convert_tml_to_import_configs(&parsed, false, true).unwrap();
        let lines: Vec<(Option<String>, Vec<(f64, f64)>)> = configs
            .iter()
            .map(|c| {
                let legs = c.shots.iter().map(|s| (s.length, s.azimuth_in)).collect();
                (c.name.clone(), legs)
            })
            .collect();
        ((lines, closures.len()), warnings)
    };

    let (with_fixes, fix_warnings) = read_back(&three_entrance_cave(true).project);
    let (without, plain_warnings) = read_back(&three_entrance_cave(false).project);
    assert_eq!(
        with_fixes, without,
        "the fix rows add no station and no leg"
    );
    assert_eq!(
        with_fixes
            .0
            .iter()
            .map(|(_, legs)| legs.len())
            .sum::<usize>(),
        11
    );
    // What the receipt says about the two fixes. Each tie hangs off a
    // mid-chain row, so the parser splits it into a one-row branch that holds
    // only the CLOSURE and is dropped empty (true of any mid-chain closure),
    // and the tie's target, a START nothing hangs off, lands in the
    // excluded-stations bucket. Both blame filtering that never happened;
    // see docs/ariane-export.md.
    assert!(plain_warnings.is_empty(), "{plain_warnings:?}");
    assert_eq!(
        fix_warnings,
        [
            "2 lines had no shots after filtering and were dropped",
            "2 loop closures referenced excluded stations and were skipped",
        ]
    );
}

/// Where `start` is, in local metres from the placement datum's GPS.
fn local_of(start: &ExportShot) -> (f64, f64) {
    let at = GpsCoordinates::new(start.latitude, start.longitude, 0.0).unwrap();
    crate::models::gps_to_local(&at, &gps(PLACEMENT))
}

fn start_of<'a>(shots: &'a [ExportShot], line: &str) -> &'a ExportShot {
    shots
        .iter()
        .find(|s| s.shot_type == "START" && s.line == line)
        .unwrap()
}

/// Main (with the placement datum on M0) and a separate Detached piece,
/// each two shots east; Detached's start is placed at `detached_at`.
fn main_and_detached(detached_at: (f64, f64)) -> (Survey, LineId, LineId) {
    let mut s = Survey::new("Datumless entrance");
    let main = s.named("Main", "#3366ff", shots(2, 90.0));
    let detached = s.named("Detached", "#3366ff", shots(2, 90.0));
    let start = s.station(detached, 0);
    s.project
        .set_placement(start, detached_at.0, detached_at.1)
        .unwrap();
    s.datum((main, 0), gps(PLACEMENT));
    (s, main, detached)
}

#[test]
fn a_datumless_entrance_start_sits_where_the_survey_puts_it() {
    // Two STARTs with the same lat/lon are pinned onto one point, and Ariane
    // collapses any loop between them. A datum-less entrance used to repeat
    // the frame datum's coordinates. (A closure back from a datum-less piece,
    // the shape 0.6.0 pinned this with, is refused on the station graph:
    // closures across pieces join datum-held pieces only.)
    let (s, _, _) = main_and_detached((100.0, 50.0));
    let project = s.build();

    for profile in [TmlProfile::User, TmlProfile::SpeleoDbSidecar] {
        let (shots, unwritten) = export(&project, profile);
        assert_eq!(unwritten, 0);
        let (east, north) = local_of(start_of(&shots, "Detached"));
        assert!(
            (east - 100.0).abs() < 1e-3 && (north - 50.0).abs() < 1e-3,
            "{profile:?}: the detached START must sit at its surveyed offset, got {east},{north}"
        );
        let (east, north) = local_of(start_of(&shots, "Main"));
        assert!(east.abs() < 1e-3 && north.abs() < 1e-3);
    }
}

#[test]
fn a_detached_piece_keeps_its_own_start_position() {
    // A carries the datum and B starts on A's last station; then B is
    // detached from that junction. B leaves as a datum-less piece where it
    // was surveyed (placed there, nothing moves), and its START must not
    // land on A's entrance. (0.6.0 also pinned a closure back from B to A
    // here; on the station graph the detach's sweep promotes such a closure
    // to a junction, so B would not be separate.)
    let mut s = Survey::new("Unmerged");
    let a = s.named("A", "#3366ff", shots(2, 90.0));
    let b = s.branch(meta("B", "#3366ff"), (a, 2), shots(2, 0.0));
    s.datum((a, 0), gps(PLACEMENT));
    let mut project = s.build();
    let junction = station(&project, a, 2);
    project.detach_line(junction, b).unwrap();
    assert!(project.validate().is_empty());
    let b0 = station(&project, b, 0);
    assert_ne!(b0, junction, "fixture rot: the detach must happen");
    let surveyed = project.raw_positions().xy(b0).unwrap();
    assert!(
        surveyed[0].hypot(surveyed[1]) > 1.0,
        "fixture rot: B must not sit on A's entrance, or nothing can collapse"
    );

    let (shots, _) = export(&project, TmlProfile::User);
    let a = start_of(&shots, "A");
    let b = start_of(&shots, "B");
    assert!(
        (a.latitude, a.longitude) != (b.latitude, b.longitude),
        "two STARTs on one point collapse the loop between them"
    );
    let (east, north) = local_of(b);
    assert!(
        (east - surveyed[0]).abs() < 1e-3 && (north - surveyed[1]).abs() < 1e-3,
        "B's START differs from A's by B's surveyed offset: got {east},{north}, \
         surveyed {},{}",
        surveyed[0],
        surveyed[1]
    );
}

/// A datum as a decoded file can hold one: `validate` checks only that the
/// numbers are finite, so a GPS off the globe arrives as it was written.
/// Pushed after the build, so a non-finite one (no file can hold it) still
/// shows the export stays total.
fn raw_datum(project: &mut Project, line: LineId, index: u32, latitude: f64, longitude: f64) {
    let at = station(project, line, index);
    project.datums.push(ZeroDatum {
        via: Some(line),
        ..ZeroDatum::new(
            at,
            GpsCoordinates {
                latitude,
                longitude,
                depth_reference: 0.0,
            },
        )
    });
}

fn starts_are_on_the_globe(shots: &[ExportShot]) -> bool {
    shots
        .iter()
        .filter(|s| s.shot_type == "START")
        .all(|s| s.latitude.abs() <= 90.0 && s.longitude.abs() <= 180.0)
}

#[test]
fn a_fix_gps_off_the_globe_is_unwritten() {
    let mut cave = three_entrance_cave(false);
    raw_datum(&mut cave.project, cave.lp, 2, f64::INFINITY, -87.0);
    raw_datum(&mut cave.project, cave.lp, 1, 500.0, -87.0);
    raw_datum(&mut cave.project, cave.side, 2, 20.0, 1e300);
    assert_eq!(
        gps_fixes(&cave.project),
        3,
        "fixture rot: all three must be GPS fixes, or this proves nothing"
    );

    for profile in [TmlProfile::User, TmlProfile::SpeleoDbSidecar] {
        let (shots, unwritten) = export(&cave.project, profile);
        assert_eq!(unwritten, 3, "{profile:?}: every bad fix is counted");
        assert_eq!(
            shots.iter().filter(|s| s.shot_type == "START").count(),
            1,
            "{profile:?}: only the placement START"
        );
        assert!(shots.iter().all(|s| s.shot_type != "CLOSURE"));
        assert!(starts_are_on_the_globe(&shots));
    }
}

#[test]
fn an_entrance_datum_off_the_globe_is_unwritten_and_its_start_is_surveyed() {
    // Detached is held by a datum whose GPS is off the globe, its correction
    // keeping the piece where it was surveyed (100, 50) from the frame (the
    // "measure, do not re-derive" rule).
    let (s, _, detached) = main_and_detached((100.0, 50.0));
    let mut project = s.build();
    project.placements.clear();
    raw_datum(&mut project, detached, 0, 500.0, -87.0);
    let bad = project.datums.len() - 1;
    let derived = project.datum_anchor_position(bad).unwrap();
    project.datums[bad].correction = Some([100.0 - derived[0], 50.0 - derived[1]]);
    assert!(project.validate().is_empty(), "{:?}", project.validate());
    let start = station(&project, detached, 0);
    let at = project.raw_positions().xy(start).unwrap();
    assert!(
        (at[0] - 100.0).abs() < 1e-6 && (at[1] - 50.0).abs() < 1e-6,
        "fixture rot: Detached must sit at (100, 50), got {at:?}"
    );

    for profile in [TmlProfile::User, TmlProfile::SpeleoDbSidecar] {
        let (shots, unwritten) = export(&project, profile);
        assert_eq!(unwritten, 1, "{profile:?}");
        assert!(starts_are_on_the_globe(&shots));
        let (east, north) = local_of(start_of(&shots, "Detached"));
        assert!(
            (east - 100.0).abs() < 1e-3 && (north - 50.0).abs() < 1e-3,
            "{profile:?}: the START falls back to its surveyed spot, got {east},{north}"
        );
    }
}

#[test]
fn a_datumless_start_is_measured_from_the_frame_station_not_the_origin() {
    // The frame datum's station need not sit on raw (0, 0): a correction
    // (kept by a frame change or an Add .stalx) puts it elsewhere.
    let (s, main, _) = main_and_detached((130.0, 30.0));
    let mut project = s.build();
    project.datums[0].correction = Some([30.0, -20.0]);
    assert!(project.validate().is_empty());
    let frame = station(&project, main, 0);
    assert_eq!(
        project.raw_positions().xy(frame),
        Some([30.0, -20.0]),
        "fixture rot: the frame station must sit off the origin"
    );

    let (shots, _) = export(&project, TmlProfile::User);
    let (east, north) = local_of(start_of(&shots, "Detached"));
    assert!(
        (east - 100.0).abs() < 1e-3 && (north - 50.0).abs() < 1e-3,
        "the START sits at its surveyed offset FROM THE FRAME STATION, got {east},{north}"
    );
}

/// Main and Side dated 2015-06-01 by Ann: a loop closure from Side's middle
/// back onto Main, and a GPS fix on Side's middle. Both CLOSURE rows hang off a
/// mid-line row, so each re-imports as a branch line of its own.
fn dated_cave_with_mid_line_closures() -> Project {
    let date = chrono::NaiveDate::from_ymd_opt(2015, 6, 1);
    let credit = crate::models::SurveyCredit::from_legacy(Some("Ann"), date)
        .stored()
        .map(std::sync::Arc::new);
    let dated = |count, azimuth| -> Vec<ShotData> {
        shots(count, azimuth)
            .into_iter()
            .map(|shot| ShotData {
                credit: credit.clone(),
                ..shot
            })
            .collect()
    };
    let mut s = Survey::new("Dated");
    let main = s.named("Main", "#3366ff", dated(4, 90.0));
    let side = s.branch(meta("Side", "#3366ff"), (main, 4), dated(3, 0.0));
    s.datum((main, 0), gps(PLACEMENT));
    s.closure((side, 2), (main, 2), ClosureStrategy::Batch);
    s.datum((side, 1), gps(FIX_MID_LINE));
    let project = s.build();
    assert_eq!(
        gps_fixes(&project),
        1,
        "fixture rot: the datum on Side must be a GPS fix"
    );
    project
}

#[test]
fn closure_rows_carry_the_date_and_people_of_their_row() {
    let project = dated_cave_with_mid_line_closures();
    let (shots, _) = export(&project, TmlProfile::User);
    let closures: Vec<&ExportShot> = shots.iter().filter(|s| s.shot_type == "CLOSURE").collect();
    assert_eq!(closures.len(), 2, "the loop closure and the fix's tie");
    for closure in closures {
        let row = shots.iter().find(|s| s.id == closure.from_id).unwrap();
        assert_eq!(
            (&closure.date, &closure.explorer),
            (&row.date, &row.explorer),
            "CLOSURE {} hangs off row {}",
            closure.id,
            row.id
        );
        assert_eq!(closure.date.as_deref(), Some("2015-06-01"));
    }
}

#[test]
fn a_mid_line_closure_does_not_move_the_reimport_declination_epoch() {
    // Each CLOSURE off a mid-line row re-imports as a branch line, and the
    // importer evaluates the declination at the earliest line date. Written
    // undated, the rows took the 2000-01-01 stand-in and turned the whole
    // re-imported survey by the 2000-vs-2015 declination difference.
    let project = dated_cave_with_mid_line_closures();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("dated.tml");
    export_tml(&project, &path).unwrap();
    let parsed = crate::parsers::tml::parse_tml(&path).unwrap();
    assert!(
        parsed.use_magnetic_azimuth,
        "fixture rot: needs a magnetic file"
    );
    assert!(
        parsed.lines.len() > 1,
        "fixture rot: the closures must re-import as branch lines"
    );
    let dates: Vec<_> = parsed.lines.iter().map(|s| s.date).collect();
    assert!(
        dates
            .iter()
            .all(|d| *d == chrono::NaiveDate::from_ymd_opt(2015, 6, 1)),
        "{dates:?}"
    );
    let (configs, _, _) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, false, true).unwrap();
    for config in &configs {
        assert_eq!(
            config.azimuth_reference.map(|r| r.source),
            Some(crate::models::CorrectionSource::IgrfAuto { epoch_year: 2015 }),
            "{:?}",
            config.name
        );
    }
}

#[test]
fn a_frame_gps_off_the_globe_leaves_the_fixes_out() {
    // The frame's entrance stays at 0,0. A fix START at its real GPS, tied into
    // the same survey, would have Ariane bend it across the globe.
    let mut cave = three_entrance_cave(false);
    cave.project.datums[0].gps.latitude = 95.0;
    let l2 = station(&cave.project, cave.lp, 2);
    cave.project.datums.push(ZeroDatum {
        via: Some(cave.lp),
        ..ZeroDatum::new(l2, gps(FIX_MID_LINE))
    });
    assert_eq!(
        gps_fixes(&cave.project),
        1,
        "fixture rot: the datum on L2 must be a GPS fix"
    );

    for profile in [TmlProfile::User, TmlProfile::SpeleoDbSidecar] {
        let (shots, unwritten) = export(&cave.project, profile);
        assert_eq!(unwritten, 2, "{profile:?}: the frame and the fix");
        assert!(
            shots.iter().all(|s| s.shot_type != "CLOSURE"),
            "{profile:?}"
        );
        let starts: Vec<&ExportShot> = shots.iter().filter(|s| s.shot_type == "START").collect();
        assert_eq!(starts.len(), 1, "{profile:?}: only the frame's entrance");
        assert_eq!((starts[0].latitude, starts[0].longitude), (0.0, 0.0));
    }
}
