//! Every zero datum is an entrance in the Ariane export (#741 decision 5).
//!
//! Ariane keeps GPS on START rows only and reads every START's lat/lon as a
//! hard fix. A GPS fix part-way through the survey has no START of its own, so
//! the exporter writes one at the fix's GPS and ties it to the fix station with
//! a zero-length CLOSURE. Synthetic data only.

use super::*;
use crate::models::{GpsCoordinates, SectionData, ShotData, StationMerge, ZeroDatum};

const PLACEMENT: (f64, f64) = (20.0, -87.5);
const FIX_AT_SECTION_START: (f64, f64) = (20.0003, -87.4995);
const FIX_MID_SECTION: (f64, f64) = (20.0006, -87.4985);

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

fn merge(project: &mut Project, a: StationRef, b: StationRef) {
    project.station_merges.push(StationMerge {
        station_a: a.into(),
        station_b: b.into(),
        merged_at: chrono::Utc::now(),
    });
}

fn datum(project: &mut Project, sr: StationRef, (lat, lon): (f64, f64)) {
    project.zero_datums.push(ZeroDatum::new(
        sr.into(),
        GpsCoordinates::new(lat, lon, 0.0).unwrap(),
    ));
}

/// Main runs 4 x 50 m east (M0..M4). Side runs 3 x 50 m north off M2 (S0 is
/// merged onto M2), and Loop runs 4 x 50 m north off M4 (L0 merged onto M4).
/// One merge-connected cave with a placement datum on M0, a GPS fix on S0 (a
/// section start, which the merge aliases onto M2) and a GPS fix on L2 (the
/// middle of a section).
fn three_entrance_cave(with_fixes: bool) -> Project {
    let mut project = Project::new("Three entrances".to_string(), None);
    for (name, count, azimuth) in [("Main", 4, 90.0), ("Side", 3, 0.0), ("Loop", 4, 0.0)] {
        project.add_section(SectionData {
            name: Some(name.to_string()),
            color: "#3366ff".to_string(),
            shots: shots(count, azimuth),
            ..Default::default()
        });
    }
    merge(&mut project, StationRef::new(0, 2), StationRef::new(1, 0));
    merge(&mut project, StationRef::new(0, 4), StationRef::new(2, 0));
    datum(&mut project, StationRef::new(0, 0), PLACEMENT);
    if with_fixes {
        datum(&mut project, StationRef::new(1, 0), FIX_AT_SECTION_START);
        datum(&mut project, StationRef::new(2, 2), FIX_MID_SECTION);
        assert_eq!(
            project.gps_fix_datums().len(),
            2,
            "fixture rot: both later datums must be GPS fixes, or this proves nothing"
        );
    }
    project
}

fn export(project: &Project, profile: TmlProfile) -> (Vec<ExportShot>, usize) {
    build_export_shots_with_report(project, profile, AzimuthExport::decide(project))
}

fn uuid_of(project: &Project, section: usize, shot: usize) -> Option<String> {
    project.sections[section].shots[shot].uuid.clone()
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
    let project = three_entrance_cave(true);
    let (shots, unwritten) = export(&project, TmlProfile::User);

    assert_eq!(unwritten, 0, "every datum is written");
    for gps in [PLACEMENT, FIX_AT_SECTION_START, FIX_MID_SECTION] {
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
    let project = three_entrance_cave(true);
    let (shots, _) = export(&project, TmlProfile::User);

    // S0 is merged onto M2, so its node is the Main row that ends at M2.
    let (start, tie, row) = fix_shape(&shots, FIX_AT_SECTION_START);
    assert_eq!(row.uuid, uuid_of(&project, 0, 1));
    // L2 is the end of Loop's second shot.
    let (mid_start, mid_tie, mid_row) = fix_shape(&shots, FIX_MID_SECTION);
    assert_eq!(mid_row.uuid, uuid_of(&project, 2, 1));

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
        assert_eq!(tie.section, row.section);
        // Both are minted after every other row, so nothing hangs off them.
        assert!(shots
            .iter()
            .all(|s| s.from_id != start.id && s.from_id != tie.id));
    }
}

/// A (placement on A0, a GPS fix on A1), then an empty section E bridging a
/// merge from A1 to B0. E has no row, so B's first shot is left parentless: an
/// orphan root the builder produces on its own, with no project closure.
fn orphaned_cave_with_a_fix() -> Project {
    let mut project = Project::new("Orphan".to_string(), None);
    for (name, count) in [("A", 2), ("E", 0), ("B", 2)] {
        project.add_section(SectionData {
            name: Some(name.to_string()),
            color: "#3366ff".to_string(),
            shots: shots(count, 90.0),
            ..Default::default()
        });
    }
    merge(&mut project, StationRef::new(0, 1), StationRef::new(1, 0));
    merge(&mut project, StationRef::new(1, 0), StationRef::new(2, 0));
    datum(&mut project, StationRef::new(0, 0), PLACEMENT);
    datum(&mut project, StationRef::new(0, 1), FIX_MID_SECTION);
    assert_eq!(
        project.gps_fix_datums().len(),
        1,
        "fixture rot: A1 is a fix"
    );
    assert!(project.loop_closures.is_empty());
    project
}

#[test]
fn an_orphan_with_a_fix_and_no_closures_still_exports() {
    // Ariane blanks its map for an orphan root alongside ANY CLOSURE row, and
    // a fix's tie is one. So the fix is left out and counted, rather than the
    // export failing over "closures" the user never made.
    let project = orphaned_cave_with_a_fix();
    let (shots, unwritten) = export(&project, TmlProfile::User);
    assert!(
        shots.iter().any(is_orphan_root),
        "fixture rot: the builder must leave B orphaned, or this proves nothing"
    );
    assert!(shots.iter().all(|s| s.shot_type != "CLOSURE"));
    assert!(start_at(&shots, FIX_MID_SECTION).is_empty());
    assert_eq!(unwritten, 1, "the left-out fix is reported");

    let dir = tempfile::TempDir::new().unwrap();
    let exported = export_tml(&project, &dir.path().join("cave.tml"));
    assert!(
        exported.is_ok(),
        "an orphan with no closure must still export, as it did before fixes: {exported:?}"
    );
    assert!(exported
        .unwrap()
        .iter()
        .any(|n| n == "gps-anchors-not-written"));

    // A real closure still takes the refusal, and it names the user's closures.
    let mut closed = project;
    closed.loop_closures.push(crate::models::LoopClosure {
        id: 1,
        station_a: StationRef::new(2, 2).into(),
        station_b: StationRef::new(0, 2).into(),
        path_length: 100.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: crate::models::ClosureStrategy::Batch,
        is_aligned: false,
    });
    let refused = export_tml(&closed, &dir.path().join("closed.tml"));
    assert!(refused.unwrap_err().to_string().contains("loop closures"));
}

#[test]
fn a_fix_start_sits_at_its_rows_repaired_depth() {
    // The geometry repair re-hangs Loop onto M4 (depth 4) because L0 was
    // surveyed at depth 0, so the L2 row is written 4 m deeper than the
    // survey says. The fix rows are appended AFTER the repair, so the START
    // takes the written depth; appended before it, the START would keep the
    // survey's depth and the zero-length tie would measure a 4 m drop.
    let project = three_entrance_cave(true);
    let surveyed = project.sections[2].shots[1].depth_out;
    let (shots, _) = export(&project, TmlProfile::User);
    let (start, tie, row) = fix_shape(&shots, FIX_MID_SECTION);
    assert_eq!(row.uuid, uuid_of(&project, 2, 1));
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
    let project = three_entrance_cave(true);
    let (shots, unwritten) = export(&project, TmlProfile::SpeleoDbSidecar);
    assert_eq!(shots.iter().filter(|s| s.shot_type == "START").count(), 1);
    assert!(shots.iter().all(|s| s.shot_type != "CLOSURE"));
    assert_eq!(unwritten, 2);
}

#[test]
fn two_datums_on_one_station_still_count_one_as_unwritten() {
    // M2 and S0 are one Ariane node: two GPS positions for one point is a
    // conflict the file cannot carry, so the later datum is reported, not
    // written as a START pinned somewhere the survey disagrees with.
    let mut project = three_entrance_cave(true);
    datum(&mut project, StationRef::new(0, 2), (20.1, -87.1));
    let (shots, unwritten) = export(&project, TmlProfile::User);
    assert_eq!(unwritten, 1);
    assert!(start_at(&shots, (20.1, -87.1)).is_empty());
    assert_eq!(start_at(&shots, FIX_AT_SECTION_START).len(), 1);
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
        let sections: Vec<(Option<String>, Vec<(f64, f64)>)> = configs
            .iter()
            .map(|c| {
                let legs = c.shots.iter().map(|s| (s.length, s.azimuth_in)).collect();
                (c.name.clone(), legs)
            })
            .collect();
        ((sections, closures.len()), warnings)
    };

    let (with_fixes, fix_warnings) = read_back(&three_entrance_cave(true));
    let (without, plain_warnings) = read_back(&three_entrance_cave(false));
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
            "2 sections had no shots after filtering and were dropped",
            "2 loop closures referenced excluded stations and were skipped",
        ]
    );
}

/// Where `start` is, in local metres from the placement datum's GPS.
fn local_of(start: &ExportShot) -> (f64, f64) {
    let origin = GpsCoordinates::new(PLACEMENT.0, PLACEMENT.1, 0.0).unwrap();
    let at = GpsCoordinates::new(start.latitude, start.longitude, 0.0).unwrap();
    crate::models::gps_to_local(&at, &origin)
}

fn start_of<'a>(shots: &'a [ExportShot], section: &str) -> &'a ExportShot {
    shots
        .iter()
        .find(|s| s.shot_type == "START" && s.section == section)
        .unwrap()
}

#[test]
fn a_datumless_entrance_start_sits_where_the_survey_puts_it() {
    // Two STARTs with the same lat/lon are pinned onto one point, and Ariane
    // collapses any loop between them. A datum-less entrance used to repeat
    // the frame datum's coordinates, so a loop back to the frame's entrance
    // collapsed onto it.
    use crate::models::{ClosureStrategy, LoopClosure};
    let mut project = Project::new("Datumless entrance".to_string(), None);
    for name in ["Main", "Detached"] {
        project.add_section(SectionData {
            name: Some(name.to_string()),
            color: "#3366ff".to_string(),
            shots: shots(2, 90.0),
            ..Default::default()
        });
    }
    project.section_offsets.insert(1, (100.0, 50.0));
    datum(&mut project, StationRef::new(0, 0), PLACEMENT);
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(1, 2).into(),
        station_b: StationRef::new(0, 2).into(),
        path_length: 100.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

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
fn an_unmerged_piece_with_a_closure_back_keeps_its_own_start_position() {
    // The reachable path: A carries the datum, B is merged in, a closure ties
    // B back to A, then B is unmerged. Unmerging removes only the closures of
    // the unmerged pair, so B leaves as a datum-less piece with a loop back to
    // A's entrance.
    use crate::models::ClosureStrategy;
    let mut project = Project::new("Unmerged".to_string(), None);
    for (name, azimuth) in [("A", 90.0), ("B", 0.0)] {
        project.add_section(SectionData {
            name: Some(name.to_string()),
            color: "#3366ff".to_string(),
            shots: shots(2, azimuth),
            ..Default::default()
        });
    }
    let (a2, b0) = (StationRef::new(0, 2), StationRef::new(1, 0));
    project.merge_stations(a2, b0).unwrap();
    // Through the mutator, so the survey is laid out against the datum: A0 on
    // local (0, 0), which IS the datum's GPS.
    project
        .set_zero_datum(
            StationRef::new(0, 0),
            GpsCoordinates::new(PLACEMENT.0, PLACEMENT.1, 0.0).unwrap(),
        )
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(1, 2),
            StationRef::new(0, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();
    assert!(
        project.unmerge_stations(a2, b0).0,
        "fixture rot: the unmerge must happen"
    );
    assert_eq!(
        project.loop_closures.len(),
        1,
        "fixture rot: the closure B->A survives"
    );
    let surveyed = project.compute_station_raw_coords(b0).unwrap();
    assert!(
        surveyed.x.hypot(surveyed.y) > 1.0,
        "fixture rot: B must not sit on A's entrance, or nothing can collapse"
    );

    let (shots, _) = export(&project, TmlProfile::User);
    assert!(shots.iter().any(|s| s.shot_type == "CLOSURE"));
    let a = start_of(&shots, "A");
    let b = start_of(&shots, "B");
    assert!(
        (a.latitude, a.longitude) != (b.latitude, b.longitude),
        "two STARTs on one point collapse the loop between them"
    );
    let (east, north) = local_of(b);
    assert!(
        (east - surveyed.x).abs() < 1e-3 && (north - surveyed.y).abs() < 1e-3,
        "B's START differs from A's by B's surveyed offset: got {east},{north}, \
         surveyed {},{}",
        surveyed.x,
        surveyed.y
    );
}

/// A datum built the way persistence loads one: no `GpsCoordinates::new`
/// validation, so any f64 can arrive.
fn raw_datum(project: &mut Project, sr: StationRef, latitude: f64, longitude: f64) {
    project.zero_datums.push(ZeroDatum {
        station_ref: sr.into(),
        gps: GpsCoordinates {
            latitude,
            longitude,
            depth_reference: 0.0,
        },
        accuracy_m: None,
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
    let mut project = three_entrance_cave(false);
    raw_datum(&mut project, StationRef::new(2, 2), f64::INFINITY, -87.0);
    raw_datum(&mut project, StationRef::new(2, 1), 500.0, -87.0);
    raw_datum(&mut project, StationRef::new(1, 2), 20.0, 1e300);
    assert_eq!(
        project.gps_fix_datums().len(),
        3,
        "fixture rot: all three must be GPS fixes, or this proves nothing"
    );

    for profile in [TmlProfile::User, TmlProfile::SpeleoDbSidecar] {
        let (shots, unwritten) = export(&project, profile);
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
    let mut project = Project::new("Bad entrance".to_string(), None);
    for name in ["Main", "Detached"] {
        project.add_section(SectionData {
            name: Some(name.to_string()),
            color: "#3366ff".to_string(),
            shots: shots(2, 90.0),
            ..Default::default()
        });
    }
    project.section_offsets.insert(1, (100.0, 50.0));
    datum(&mut project, StationRef::new(0, 0), PLACEMENT);
    raw_datum(&mut project, StationRef::new(1, 0), 500.0, -87.0);

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
    // A legacy project need not keep the frame datum's station on raw (0, 0).
    let mut project = Project::new("Offset frame".to_string(), None);
    for name in ["Main", "Detached"] {
        project.add_section(SectionData {
            name: Some(name.to_string()),
            color: "#3366ff".to_string(),
            shots: shots(2, 90.0),
            ..Default::default()
        });
    }
    project.section_offsets.insert(0, (30.0, -20.0));
    project.section_offsets.insert(1, (130.0, 30.0));
    datum(&mut project, StationRef::new(0, 0), PLACEMENT);
    assert_eq!(
        project.compute_station_raw_xy(StationRef::new(0, 0)),
        Some((30.0, -20.0)),
        "fixture rot: the frame station must sit off the origin"
    );

    let (shots, _) = export(&project, TmlProfile::User);
    let (east, north) = local_of(start_of(&shots, "Detached"));
    assert!(
        (east - 100.0).abs() < 1e-3 && (north - 50.0).abs() < 1e-3,
        "the START sits at its surveyed offset FROM THE FRAME STATION, got {east},{north}"
    );
}
