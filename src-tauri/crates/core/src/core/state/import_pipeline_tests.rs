//! End-to-end import tests relocated from `parsers::{tml,walls}` ahead of the
//! crate split: they drive the REAL import path through [`AppState`], so they
//! live with the state they construct. `core` depends on `parsers` and
//! `export` non-test, which keeps every import below downward.
//!
//! Ported to the station graph (C7): the exports are `export::survey`'s, the
//! projects are `models::survey` ones, and the edits the tests make between
//! the import and the export go through the model where their command is
//! another package's (a credit edit, a loop closure made by the import).

use crate::core::state::{AppState, VirtualShotMode};
use crate::core::Command;
use crate::models::survey::{LineMeta, Project, StartAt, ZeroDatum};
use crate::models::{GpsCoordinates, ShotData, ShotKind, StationId};
use crate::parsers::tml::test_helpers::{
    parse_synthetic_tmlu, tml_shot_xml_depth, tml_shot_xml_gps, wrap_xml_with,
};
use crate::parsers::tml::{convert_tml_to_import_configs, parse_tml, TmlFile};
use crate::parsers::walls::test_support::{convert_one, parse_srv};

/// A fresh state with an empty project named `name`.
fn new_state(name: &str) -> AppState {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: name.to_string(),
                location: None,
            })
            .success
    );
    state
}

/// Import `tml` (comments on shots unless `comments_on_stations`) into
/// `state` with `elevation`; returns the result.
fn import_tml_into(
    state: &AppState,
    tml: &TmlFile,
    comments_on_stations: bool,
    elevation: Option<f64>,
) -> crate::core::CommandResult {
    let (lines, closures, _) =
        convert_tml_to_import_configs(tml, false, comments_on_stations).unwrap();
    state.execute_tml_import_sections(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        elevation,
    )
}

/// Import `tml` into a fresh project named `name` and return the state.
fn import_tml(name: &str, tml: &TmlFile) -> AppState {
    let state = new_state(name);
    let result = import_tml_into(&state, tml, false, None);
    assert!(result.success, "{name}: import failed: {:?}", result.error);
    state
}

/// Export `project` to `<dir>/<label>.tmlu` and parse it back.
fn round_trip(project: &Project, dir: &std::path::Path, label: &str) -> TmlFile {
    let path = dir.join(format!("{label}.tmlu"));
    crate::export::survey::export_tml(project, &path).unwrap();
    parse_tml(&path).unwrap()
}

/// #431: a round elevation in metres (100.0) stated in feet (328.084)
/// lands in the project's `surface_elevation` correctly converted, not just on
/// the intermediate `TmlFile`/`TmlGeoCoding` structs.
#[test]
fn imperial_elevation_328_084_ft_lands_as_100_m_in_the_project() {
    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml_with("ft", 328.084, &shots));
    assert!((tml.first_start_absolute_elevation - 100.0).abs() < 1e-3);

    let state = new_state("Elevation Units");
    let elevation = crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation);
    let result = import_tml_into(&state, &tml, false, elevation);
    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    let got = project
        .settings
        .surface_elevation
        .expect("elevation must be set");
    assert!((got - 100.0).abs() < 1e-3, "got {got}, want 100.0 +/- 1e-3");
}

/// #431 round trip: a synthetic `.tmlu` carrying
/// `firstStartAbsoluteElevation = 123.5` survives import into a fresh
/// project and back out through export unchanged.
#[test]
fn entrance_elevation_round_trips_through_import_and_export() {
    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 123.5, &shots));

    let state = new_state("Round Trip");
    let elevation = crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation);
    let result = import_tml_into(&state, &tml, false, elevation);
    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.settings.surface_elevation, Some(123.5));

    let out_dir = tempfile::tempdir().unwrap();
    let round_tripped = round_trip(&project, out_dir.path(), "roundtrip");
    assert!(
        (round_tripped.first_start_absolute_elevation - 123.5).abs() < 1e-6,
        "got {}, want 123.5",
        round_tripped.first_start_absolute_elevation
    );
}

/// #431 no-clobber: importing a second file with a DIFFERENT entrance
/// elevation into a project that already has one keeps the existing value
/// and surfaces the mismatch as a receipt warning naming both numbers,
/// rather than silently overwriting it.
#[test]
fn entrance_elevation_import_does_not_clobber_an_existing_value() {
    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
    ]
    .join("\n");
    let state = new_state("No Clobber");

    let first_tml = parse_synthetic_tmlu(&wrap_xml_with("m", 50.0, &shots));
    let result = import_tml_into(
        &state,
        &first_tml,
        false,
        Some(first_tml.first_start_absolute_elevation),
    );
    assert!(result.success, "first import failed: {:?}", result.error);
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .settings
            .surface_elevation,
        Some(50.0)
    );

    let second_tml = parse_synthetic_tmlu(&wrap_xml_with("m", 75.0, &shots));
    let result = import_tml_into(
        &state,
        &second_tml,
        false,
        Some(second_tml.first_start_absolute_elevation),
    );
    assert!(result.success, "second import failed: {:?}", result.error);
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .settings
            .surface_elevation,
        Some(50.0),
        "the existing elevation must survive a conflicting import"
    );
    let warnings = result
        .import_receipt
        .as_ref()
        .map(|r| r.warnings.clone())
        .unwrap_or_default();
    assert!(
        warnings.iter().any(|w| w.contains("kept 50.00")),
        "the clobber must be surfaced naming the KEPT value explicitly, got {warnings:?}"
    );
}

/// #431 tolerance: re-importing the SAME cave via its metric and imperial
/// twins (100.0 m vs 328.084 ft) must not fire the no-clobber "differs"
/// warning: the unit round trip's residue is inside the message's own
/// `{:.2}` precision.
#[test]
fn reimporting_the_metric_and_imperial_twins_of_one_elevation_warns_never() {
    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
    ]
    .join("\n");
    let state = new_state("Unit Twins");

    let metric_tml = parse_synthetic_tmlu(&wrap_xml_with("m", 100.0, &shots));
    let result = import_tml_into(
        &state,
        &metric_tml,
        false,
        Some(metric_tml.first_start_absolute_elevation),
    );
    assert!(result.success, "metric import failed: {:?}", result.error);

    let imperial_tml = parse_synthetic_tmlu(&wrap_xml_with("ft", 328.084, &shots));
    let result = import_tml_into(
        &state,
        &imperial_tml,
        false,
        Some(imperial_tml.first_start_absolute_elevation),
    );
    assert!(result.success, "imperial import failed: {:?}", result.error);
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .settings
            .surface_elevation,
        Some(100.0),
        "the original metric value must survive unchanged"
    );
    let warnings = result
        .import_receipt
        .as_ref()
        .map(|r| r.warnings.clone())
        .unwrap_or_default();
    assert!(
        warnings.iter().all(|w| !w.contains("differs from")),
        "a unit round-trip must not read as a conflicting elevation, got {warnings:?}"
    );
}

#[test]
fn end_to_end_import_respects_datum_invariants() {
    // Runs the REAL import (the pipeline validates the model), so a
    // successful result proves the placement respects the one-anchor-per-
    // piece invariant.
    let run = |srv: &str| -> (bool, Option<String>, usize) {
        let (lines, closures, _w) = convert_one(parse_srv(srv));
        let state = new_state("T");
        let result = state.execute_import_lines(
            lines,
            closures,
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
        );
        let datums = state.get_project_state().map_or(0, |p| p.datums.len());
        (result.success, result.error, datums)
    };

    // Two separate components, each with its own #Fix -> 2 datums, OK.
    let (ok2, err2, d2) = run("#UNITS meters order=DAV\n\
         #FIX A0 W90:00:00 N30:00:00 0\n\
         A0 A1 10 90 0\n\
         #FIX B0 W91:00:00 N31:00:00 0\n\
         B0 B1 10 90 0\n");
    assert!(ok2, "two-datum import should succeed: {err2:?}");
    assert_eq!(d2, 2);

    // Two #Fix in ONE connected component -> exactly 1 datum, import still
    // succeeds.
    let (ok1, err1, d1) = run("#UNITS meters order=DAV\n\
         #FIX A0 W90:00:00 N30:00:00 0\n\
         #FIX B0 W91:00:00 N31:00:00 0\n\
         A0 A1 10 90 0\n\
         A1 J 10 90 0\n\
         B0 J 10 270 0\n");
    assert!(
        ok1,
        "single-datum component import should succeed: {err1:?}"
    );
    assert_eq!(
        d1, 1,
        "exactly one datum survives in a single connected component"
    );
}

/// Shots of `length` metres east, `count` of them.
fn east(count: usize, length: f64) -> Vec<ShotData> {
    (0..count)
        .map(|_| ShotData {
            length,
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            ..Default::default()
        })
        .collect()
}

/// A survey of two lines, North (one shot) and South (two shots), as two
/// pieces; `datums` are `(line, station index, latitude, longitude)`; a piece
/// without a datum sits at the origin.
fn two_entrance_project(datums: &[(usize, usize, f64, f64)]) -> Project {
    let mut project = Project::new("Cycle", None);
    let mut ids = Vec::new();
    for (name, count) in [("North", 1), ("South", 2)] {
        let id = project
            .add_line(
                LineMeta {
                    name: Some(name.to_string()),
                    color: "#ff0000".to_string(),
                    ..LineMeta::default()
                },
                StartAt::New,
                east(count, 5.0),
            )
            .unwrap();
        ids.push(id);
    }
    for &(line, index, latitude, longitude) in datums {
        let station = project
            .line(ids[line])
            .unwrap()
            .stations()
            .nth(index)
            .unwrap();
        project.push_datum(ZeroDatum {
            via: Some(ids[line]),
            ..ZeroDatum::new(
                station,
                GpsCoordinates::new(latitude, longitude, 0.0).unwrap(),
            )
        });
    }
    for &id in &ids {
        let start = project.line(id).unwrap().start;
        if project.placement_datum_of(start).is_none() {
            project.set_placement(start, 0.0, 0.0).unwrap();
        }
    }
    assert!(project.validate().is_empty(), "{:?}", project.validate());
    project
}

/// #378 item (d): export -> import -> export must not let GPS anchors drift.
/// Since the exporter writes one START per entrance, an import of that file
/// creates one zero datum per GPS-tagged START - so a survey's anchor set has
/// to be STABLE across cycles, and each anchor has to keep its own
/// coordinates. Several anchors are legitimate (one per piece), NOT
/// suppressed on import.
#[test]
fn zero_datums_are_stable_across_export_import_cycles() {
    fn import_cycle(project: &Project, dir: &std::path::Path, label: &str) -> Project {
        let tml = round_trip(project, dir, label);
        let state = new_state(label);
        let result = import_tml_into(&state, &tml, false, None);
        assert!(result.success, "{label}: import failed: {:?}", result.error);
        state.get_project_state().unwrap()
    }

    fn anchor_set(project: &Project) -> Vec<(i64, i64)> {
        // Coordinates as written-precision integers so f64 noise and datum
        // order cannot flake the comparison.
        let mut set: Vec<(i64, i64)> = project
            .datums
            .iter()
            .map(|d| {
                (
                    (d.gps.latitude * 1e6).round() as i64,
                    (d.gps.longitude * 1e6).round() as i64,
                )
            })
            .collect();
        set.sort_unstable();
        set
    }

    let dir = tempfile::tempdir().unwrap();

    // Two entrances, each with its own anchor: the set survives verbatim, and
    // stays put on a second cycle.
    let original = two_entrance_project(&[(0, 0, 20.25, -87.5), (1, 0, 20.31, -87.42)]);
    let expected = anchor_set(&original);
    let once = import_cycle(&original, dir.path(), "two-anchors-once");
    assert_eq!(anchor_set(&once), expected, "anchors changed on import");
    let twice = import_cycle(&once, dir.path(), "two-anchors-twice");
    assert_eq!(anchor_set(&twice), expected, "anchors drifted on re-import");

    // One anchor, two entrances: the export stamps the frame datum's GPS onto
    // the unanchored entrance (better than null island), so the FIRST cycle
    // legitimately gains one datum for that piece - and from then on the set
    // must hold still.
    let original = two_entrance_project(&[(0, 0, 20.25, -87.5)]);
    let once = import_cycle(&original, dir.path(), "one-anchor-once");
    let after_first = anchor_set(&once);
    assert_eq!(
        after_first,
        vec![(20_250_000, -87_500_000), (20_250_000, -87_500_000)],
        "the unanchored entrance gains its placement datum on the first cycle, \
         at the FRAME datum's coordinates - anything else (0,0 in particular) \
         is the null-island regression"
    );
    let twice = import_cycle(&once, dir.path(), "one-anchor-twice");
    assert_eq!(
        anchor_set(&twice),
        after_first,
        "the anchor set must be stable from the first re-import onward"
    );

    // A mid-line datum on a detached entrance: the coordinates never drift
    // and the shape is stable from the first re-import onward.
    let original = two_entrance_project(&[(0, 0, 20.25, -87.5), (1, 1, 20.31, -87.42)]);
    let once = import_cycle(&original, dir.path(), "mid-datum-once");
    let after_first = anchor_set(&once);
    for anchor in &after_first {
        assert!(
            anchor == &(20_250_000, -87_500_000) || anchor == &(20_310_000, -87_420_000),
            "a cycled anchor must carry one of the survey's own coordinates, got {anchor:?}"
        );
    }
    let twice = import_cycle(&once, dir.path(), "mid-datum-twice");
    assert_eq!(
        anchor_set(&twice),
        after_first,
        "the mid-line-split shape must be stable from the first re-import onward"
    );
}

/// The comment of each station of `line`'s chain that has one, by chain
/// index.
fn station_comments(project: &Project, line: crate::models::LineId) -> Vec<(usize, String)> {
    project
        .line(line)
        .unwrap()
        .stations()
        .enumerate()
        .filter_map(|(i, s)| {
            project
                .station(s)
                .and_then(|st| st.comment.clone())
                .map(|c| (i, c))
        })
        .collect()
}

/// #564: a shot comment with no station comment beside it used to export
/// UNMARKED, and the default import routes unmarked lines to the station -
/// so the text silently changed side on every default round trip. Cover the
/// three shapes: shot comment alone, station comment alone, and both on one
/// shot.
#[test]
fn comments_keep_their_side_across_a_default_reimport() {
    let mut original = Project::new("Comments", None);
    let shots: Vec<ShotData> = (0..3)
        .map(|i| ShotData {
            comment: match i {
                0 => Some("broken tape".to_string()),
                2 => Some("silty here".to_string()),
                _ => None,
            },
            ..east(1, 5.0).remove(0)
        })
        .collect();
    let line = original
        .add_line(
            LineMeta {
                name: Some("Main".to_string()),
                color: "#ff0000".to_string(),
                ..LineMeta::default()
            },
            StartAt::New,
            shots,
        )
        .unwrap();
    let chain: Vec<StationId> = original.line(line).unwrap().stations().collect();
    original.set_placement(chain[0], 0.0, 0.0).unwrap();
    // Station comments away from station 0 (the START alias documentedly
    // drops a comment there): shot 1's to-station carries one alone, shot 2's
    // to-station rides beside that shot's own comment.
    original.station_mut(chain[2]).unwrap().comment = Some("junction with main".to_string());
    original.station_mut(chain[3]).unwrap().comment = Some("beware silt".to_string());

    let dir = tempfile::tempdir().unwrap();
    let tml = round_trip(&original, dir.path(), "comments");
    // `true` = comments-on-stations, the import dialog's default - the exact
    // setting under which the missing marker moved shot comments.
    let state = new_state("comments-reimport");
    let result = import_tml_into(&state, &tml, true, None);
    assert!(result.success, "import failed: {:?}", result.error);
    let reimported = state.get_project_state().unwrap();

    assert_eq!(reimported.lines().len(), 1, "one line must come back");
    let line = &reimported.lines()[0];
    let shot_comments: Vec<Option<String>> = line.shots.iter().map(|s| s.comment.clone()).collect();
    assert_eq!(
        shot_comments,
        vec![
            Some("broken tape".to_string()),
            None,
            Some("silty here".to_string())
        ],
        "shot comments must come back on their shots, not move to stations"
    );
    assert_eq!(
        station_comments(&reimported, line.id),
        vec![
            (2, "junction with main".to_string()),
            (3, "beware silt".to_string())
        ],
        "station comments must come back on their stations and nowhere else"
    );
}

/// Every shot in the project, cross-checked with the real validator - the
/// same call the create path makes.
fn inclination_depth_mismatch_count(project: &Project) -> usize {
    use crate::config::ValidationConfig;
    use crate::core::Validator;

    let config = ValidationConfig::default();
    project
        .lines()
        .iter()
        .flat_map(|line| {
            line.shots.iter().flat_map(|shot| {
                Validator::validate_shot(&shot.to_shot_data(), &config, String::new)
            })
        })
        .filter(|w| w.code == "INCLINATION_DEPTH_MISMATCH")
        .count()
}

/// #482: a clean TML export/import cycle must not fabricate
/// `INCLINATION_DEPTH_MISMATCH` warnings for depth-only surveys (export
/// collapses `pitch_in: None` to a wire `0.0` inclination, which import reads
/// back as "not sighted").
#[test]
fn depth_only_tml_round_trip_fabricates_no_inclination_depth_mismatch() {
    // Five depth-only shots (Inclination 0.0, i.e. never sighted), each with
    // a real depth change steep enough to fabricate a mismatch under the old
    // logic. `tml_shot_xml_depth` hardcodes `Length` at 10.0. Chain starts
    // from the START shot's hardcoded Depth=5.0.
    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Main", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 13.0, -1.0), // dz +8
        tml_shot_xml_depth(2, 1, "REAL", "Main", 4.0, -1.0),  // dz -9
        tml_shot_xml_depth(3, 2, "REAL", "Main", 11.0, -1.0), // dz +7
        tml_shot_xml_depth(4, 3, "REAL", "Main", 3.0, -1.0),  // dz -8
        tml_shot_xml_depth(5, 4, "REAL", "Main", 12.0, -1.0), // dz +9
    ]
    .join("\n");

    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 100.0, &shots));
    let state = new_state("Depth Only");
    let result = import_tml_into(
        &state,
        &tml,
        false,
        Some(tml.first_start_absolute_elevation),
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        inclination_depth_mismatch_count(&project),
        0,
        "first import must not fabricate a mismatch for a never-sighted inclination"
    );

    let out_dir = tempfile::tempdir().unwrap();
    let round_tripped = round_trip(&project, out_dir.path(), "depth_only");
    let state2 = new_state("Depth Only Round Trip");
    let result2 = import_tml_into(
        &state2,
        &round_tripped,
        false,
        Some(round_tripped.first_start_absolute_elevation),
    );
    assert!(result2.success, "re-import failed: {:?}", result2.error);
    assert_eq!(
        inclination_depth_mismatch_count(&state2.get_project_state().unwrap()),
        0,
        "a clean export/import round trip must not fabricate inclination-depth \
         mismatch warnings for a depth-only survey"
    );
}

/// #482 deliberate residual loss (a), pinned as chosen behaviour: a shot
/// with a GENUINELY recorded `Some(0.0)` pitch on a STEEP depth-only geometry
/// is erased to `None` by one export/import round trip (the wire `0.0` is
/// ambiguous, and keeping `Some(0.0)` would re-fabricate #482's warning).
#[test]
fn a_genuinely_recorded_zero_pitch_on_a_steep_shot_is_erased_by_one_round_trip() {
    let mut project = Project::new("Erasure Pin", None);
    let line = project
        .add_line(
            LineMeta {
                name: Some("Main".to_string()),
                color: "#abcdef".to_string(),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![ShotData {
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                length: 10.0,
                depth_in: 0.0,
                depth_out: 8.0, // implied ~53 deg, well past the 15 deg default
                pitch_in: Some(0.0),
                pitch_out: Some(0.0),
                ..Default::default()
            }],
        )
        .unwrap();
    let start = project.line(line).unwrap().start;
    project.set_placement(start, 0.0, 0.0).unwrap();
    assert_eq!(
        project.lines()[0].shots[0].pitch_in,
        Some(0.0),
        "the recorded reading survives until export"
    );

    let out_dir = tempfile::tempdir().unwrap();
    let round_tripped = round_trip(&project, out_dir.path(), "erasure");
    let state2 = new_state("Erasure Pin Round Trip");
    let result2 = import_tml_into(
        &state2,
        &round_tripped,
        false,
        Some(round_tripped.first_start_absolute_elevation),
    );
    assert!(result2.success, "re-import failed: {:?}", result2.error);

    let project2 = state2.get_project_state().unwrap();
    assert_eq!(
        project2.lines()[0].shots[0].pitch_in,
        None,
        "a genuinely-recorded 0deg pitch on a steep shot is erased, not \
         preserved - this is the chosen tradeoff, not a bug"
    );
    assert_eq!(
        inclination_depth_mismatch_count(&project2),
        0,
        "the erasure is exactly what keeps this shot from re-fabricating #482's warning"
    );
}

/// #482 closure interaction: a depth-only TML import (steep shots, `pitch_in:
/// None`) closed into a loop must not surface `INCLINATION_DEPTH_MISMATCH`
/// afterwards. On the station graph the Closures Computed View is derived and
/// never rewrites a shot, so the measured shots are what the validator sees;
/// the loop is made by an import (a bridge line on the chain's tail and a
/// closure back to its start), the funnel's own closure path.
#[test]
fn depth_only_shots_closed_into_a_loop_do_not_fabricate_a_mismatch() {
    use crate::models::{ImportClosure, ImportLineConfig, ImportPlacement};

    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Main", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 13.0, -1.0), // dz +8
        tml_shot_xml_depth(2, 1, "REAL", "Main", 4.0, -1.0),  // dz -9
        tml_shot_xml_depth(3, 2, "REAL", "Main", 11.0, -1.0), // dz +7
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 100.0, &shots));
    let (mut lines, _, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
    assert_eq!(lines.len(), 1);
    let main = &lines[0];
    assert!(
        main.shots.iter().any(|s| s.pitch_in.is_none()),
        "fixture must actually exercise the None-pitch shape #482 fixed"
    );
    let tail = main.shots.len() as u32;
    // A short bridging line on the chain's tail, left with pitch at its None
    // default so it cannot itself contribute a mismatch.
    lines.push(ImportLineConfig {
        line_index: 1,
        name: Some("Bridge".to_string()),
        color: "#222222".to_string(),
        shots: vec![ShotData {
            azimuth_in: 270.0,
            azimuth_out: 270.0,
            length: 5.0,
            depth_in: 100.0,
            depth_out: 100.0,
            ..Default::default()
        }],
        placement: ImportPlacement::FromImporting {
            import_station: 0,
            target_line_index: 0,
            target_station_index: tail,
        },
        ..main.clone()
    });
    let closures = vec![ImportClosure {
        from_line_index: 0,
        from_station_index: 0,
        to_line_index: 1,
        to_station_index: 1,
    }];
    let state = new_state("Closure Interaction");
    let result = state.execute_tml_import_sections(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        Some(tml.first_start_absolute_elevation),
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let receipt = result.import_receipt.unwrap();
    assert_eq!(
        (receipt.closures_requested, receipt.closures_created),
        (1, 1)
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(project.closures.len(), 1, "the loop is closed");
    assert_eq!(
        inclination_depth_mismatch_count(&project),
        0,
        "a closed depth-only loop must not surface INCLINATION_DEPTH_MISMATCH"
    );
}

/// The distinct surveyor names of every shot of `line`, joined.
fn surveyors_of(line: &crate::models::survey::Line) -> Option<String> {
    let summary = crate::models::LineCreditSummary::from_credits(
        line.shots
            .iter()
            .map(|s| s.credit.as_deref().unwrap_or(&crate::models::EMPTY_CREDIT)),
    );
    crate::models::join_names(&summary.surveyor_people())
}

/// #427 end to end: a chain that changes COLOUR mid-run imports as one line
/// per colour, and a full export/re-import round trip returns the SAME line
/// names and trips - no second cut, no second suffix.
#[test]
fn per_trip_lines_survive_an_export_and_re_import_unchanged() {
    use crate::parsers::tml::test_helpers::TripShot;

    let second = TripShot {
        line: "A",
        explorer: "Ada Lovelace",
        date: "2025-03-04",
        color: "0x11223344",
        ..TripShot::default()
    };
    let shots = [
        TripShot {
            id: 1,
            from_id: -1,
            line: "A",
            depth: 5.0,
            ..TripShot::default()
        }
        .xml(),
        TripShot {
            id: 2,
            from_id: 1,
            line: "A",
            depth: 9.0,
            ..TripShot::default()
        }
        .xml(),
        TripShot {
            id: 3,
            from_id: 2,
            depth: 11.0,
            ..second
        }
        .xml(),
        TripShot {
            id: 4,
            from_id: 3,
            depth: 12.0,
            ..second
        }
        .xml(),
    ]
    .join("\n");

    let trips = |project: &Project| -> Vec<(String, Option<String>)> {
        project
            .lines()
            .iter()
            .map(|s| (s.name.clone().unwrap_or_default(), surveyors_of(s)))
            .collect()
    };

    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &shots));
    let project = import_tml("Trips", &tml).get_project_state().unwrap();
    let expected = vec![
        ("A".to_string(), Some("Tester".to_string())),
        ("A (trip 2)".to_string(), Some("Ada Lovelace".to_string())),
    ];
    assert_eq!(trips(&project), expected, "the cut lands in the project");

    let out_dir = tempfile::tempdir().unwrap();
    let reparsed = round_trip(&project, out_dir.path(), "trips");
    assert_eq!(
        reparsed
            .lines
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "A (trip 2)"],
        "the round trip must not cut again, re-merge, or double the suffix"
    );

    let round_tripped = import_tml("Trips Again", &reparsed)
        .get_project_state()
        .unwrap();
    assert_eq!(
        trips(&round_tripped),
        expected,
        "and both trips come back with their own surveyor"
    );
}

/// The round trip the trip cut broke (#329): ONE line whose shots carry
/// different people and dates - entered per shot in the app, one colour -
/// exported to Ariane and imported again comes back as one line with every
/// shot's credit exactly as it left. (The per-shot credit is set on the
/// model here: the credit command is another package's.)
#[test]
fn a_line_with_mixed_per_shot_credit_round_trips_through_ariane_as_one_line() {
    use crate::models::{Person, SurveyCredit};
    use crate::parsers::tml::test_helpers::TripShot;

    let shots: Vec<String> = (1..=4)
        .map(|id| {
            TripShot {
                id,
                from_id: id - 1,
                line: "A",
                depth: 5.0 + id as f64,
                ..TripShot::default()
            }
            .xml()
        })
        .collect();
    let state = import_tml(
        "Mixed",
        &parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &shots.join("\n"))),
    );
    let mut project = state.get_project_state().unwrap();
    assert_eq!(project.lines()[0].shots.len(), 4);

    // Three teams on three days over the one line, as the Detail panel does it.
    let person = |name: &str| Person::named(name).unwrap();
    let teams = [
        (person("Ann Diver"), "2026-03-02"),
        (person("Bo Lead"), "2026-04-11"),
        (person("Cy Cave"), "2026-05-20"),
    ];
    let line_id = project.lines()[0].id;
    for (index, shot) in project
        .line_mut(line_id)
        .unwrap()
        .shots
        .iter_mut()
        .enumerate()
        .skip(1)
    {
        let (who, day) = &teams[(index - 1).min(2)];
        let mut credit = shot.credit.as_deref().cloned().unwrap_or_default();
        credit = crate::models::CreditEdit {
            set_surveyors: Some(vec![who.clone()]),
            set_date: Some(day.parse().ok()),
            ..crate::models::CreditEdit::default()
        }
        .apply(&credit);
        shot.credit = credit.into_storable().stored().map(std::sync::Arc::new);
    }
    let credits = |project: &Project| -> Vec<Vec<SurveyCredit>> {
        project
            .lines()
            .iter()
            .map(|s| {
                s.shots
                    .iter()
                    .map(|shot| shot.credit.as_deref().cloned().unwrap_or_default())
                    .collect()
            })
            .collect()
    };
    let before = credits(&project);
    assert_eq!(before.len(), 1);

    let out_dir = tempfile::tempdir().unwrap();
    let reparsed = round_trip(&project, out_dir.path(), "mixed");
    assert_eq!(
        reparsed
            .lines
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A"],
        "credit changes must not cut the line"
    );

    let again = import_tml("Mixed Again", &reparsed);
    assert_eq!(
        credits(&again.get_project_state().unwrap()),
        before,
        "one line, every shot with exactly the credit it left with"
    );
}

/// Review of #719, item 1: one name with no `" / "` in Ariane's `Explorer`
/// field - the common form, e.g. `Tester` - is a SURVEYOR: in the app after
/// the import, in the saved `.stalx`, and again after the file is reopened
/// and saved.
#[test]
fn a_single_ariane_name_is_a_surveyor_in_the_app_and_in_the_saved_file() {
    use crate::parsers::tml::test_helpers::TripShot;

    let shots: Vec<String> = (1..=3)
        .map(|id| {
            TripShot {
                id,
                from_id: id - 1,
                line: "A",
                explorer: "Tester",
                depth: 5.0 + id as f64,
                ..TripShot::default()
            }
            .xml()
        })
        .collect();
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &shots.join("\n")));
    let state = import_tml("Single", &tml);

    let roles = |project: &Project| -> Vec<(Vec<String>, Vec<String>)> {
        let names = |people: &[crate::models::Person]| {
            people.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
        };
        project.lines()[0]
            .shots
            .iter()
            .map(|shot| {
                let credit = shot.credit.as_deref().cloned().unwrap_or_default();
                (names(&credit.explorers), names(&credit.surveyors))
            })
            .collect()
    };
    let surveyor_only = (Vec::new(), vec!["Tester".to_string()]);
    let imported = state.get_project_state().unwrap();
    assert!(!imported.lines()[0].shots.is_empty());
    assert!(
        roles(&imported).iter().all(|r| r == &surveyor_only),
        "{:?}",
        roles(&imported)
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("single.stalx");
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    assert!(std::fs::read_to_string(&path).unwrap().contains("Tester"));

    let reopened = AppState::new().unwrap();
    assert!(
        reopened
            .execute(Command::OpenProject { path: path.clone() })
            .success
    );
    assert!(roles(&reopened.get_project_state().unwrap())
        .iter()
        .all(|r| r == &surveyor_only));
    let resaved = reopened.execute(Command::SaveProject { path: None });
    assert!(resaved.success, "{:?}", resaved.error);
    let again = AppState::new().unwrap();
    assert!(again.execute(Command::OpenProject { path }).success);
    assert!(roles(&again.get_project_state().unwrap())
        .iter()
        .all(|r| r == &surveyor_only));
}

/// Review of #719, item 5c: a virtual shot between two real shots of a run is
/// a Jump, one dangling off the end a Reference (the import's chain rule).
#[test]
fn a_bounded_virtual_shot_imports_as_a_jump_and_a_dangling_one_as_a_reference() {
    let shots = [
        tml_shot_xml_depth(1, 0, "REAL", "A", 1.0, -1.0),
        tml_shot_xml_depth(2, 1, "VIRTUAL", "A", 1.0, -1.0),
        tml_shot_xml_depth(3, 2, "REAL", "A", 1.0, -1.0),
        tml_shot_xml_depth(4, 3, "VIRTUAL", "A", 1.0, -1.0),
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &shots));
    let project = import_tml("Kinds", &tml).get_project_state().unwrap();
    let kinds: Vec<ShotKind> = project
        .lines()
        .iter()
        .flat_map(|s| s.shots.iter().map(|shot| shot.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ShotKind::Normal,
            ShotKind::Jump,
            ShotKind::Normal,
            ShotKind::Reference
        ]
    );
}

/// One Ariane row for the #812 tests, at a depth of 5 m, in the Ariane
/// section (an import line) `line`. `length` is the raw
/// `<Length>` text, so a test can write `NaN`; `excluded` is the `<Excluded>` flag.
fn tml_row(id: i64, from: i64, ty: &str, line: &str, length: &str, excluded: bool) -> String {
    tml_row_at(id, from, ty, line, length, excluded, 5.0)
}

/// [`tml_row`] with the row's `<Depth>` given.
fn tml_row_at(
    id: i64,
    from: i64,
    ty: &str,
    line: &str,
    length: &str,
    excluded: bool,
    depth: f64,
) -> String {
    format!(
        r"<SurveyData>
    <ID>{id}</ID><FromID>{from}</FromID><ClosureToID>-1</ClosureToID>
    <Type>{ty}</Type><Section>{line}</Section>
    <Azimut>90.0</Azimut><Length>{length}</Length><Depth>{depth}</Depth><DepthIn>-1.0</DepthIn>
    <Inclination>0.0</Inclination><Left>0.0</Left><Right>0.0</Right><Up>0.0</Up><Down>0.0</Down>
    <Comment></Comment><Explorer>Tester</Explorer><Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color><Excluded>{excluded}</Excluded>
    <Latitude>0.0</Latitude><Longitude>0.0</Longitude>
</SurveyData>"
    )
}

/// Runs `rows` through the real TML import into a fresh project and returns
/// the project plus the importer's receipt warnings. Panics with the import
/// error, so a red test names the failure the user saw.
fn import_tml_rows(rows: &[String]) -> (Project, Vec<String>) {
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &rows.join("\n")));
    let (lines, closures, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
    let state = new_state("Zero Length");
    let result = state.execute_tml_import_sections(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        None,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    (state.get_project_state().unwrap(), warnings)
}

fn tie_warning(warnings: &[String]) -> Option<&String> {
    warnings.iter().find(|w| w.contains("had no usable length"))
}

/// #812: one zero-length row in the middle of a line used to fail the
/// whole import ("Shot N:2 has zero length") and leave the project empty. It
/// now imports as a coincident tie, the shots on either side stay one chain,
/// and the receipt says so once.
#[test]
fn a_zero_length_row_mid_line_imports_as_a_coincident_tie() {
    let (project, warnings) = import_tml_rows(&[
        tml_row(0, -1, "START", "Main", "0.0", false),
        tml_row(1, 0, "REAL", "Main", "10.0", false),
        tml_row(2, 1, "REAL", "Main", "0.0", false),
        tml_row(3, 2, "REAL", "Main", "10.0", false),
    ]);
    assert_eq!(project.lines().len(), 1, "the line must import whole");
    let lengths: Vec<f64> = project.lines()[0].shots.iter().map(|s| s.length).collect();
    assert_eq!(
        lengths,
        vec![10.0, crate::parsers::geo::COINCIDENT_TIE_EPSILON_M, 10.0]
    );
    let note = tie_warning(&warnings).expect("the receipt must name the nudged row");
    assert!(note.starts_with("1 shot in the file "), "{note}");
}

/// #812: the zero-length row is the FIRST row of a branch line, the shape
/// the user's error ("Shot 14:1") pointed at. Both lines import.
#[test]
fn a_zero_length_first_row_of_a_branch_imports_as_a_coincident_tie() {
    let (project, warnings) = import_tml_rows(&[
        tml_row(0, -1, "START", "Main", "0.0", false),
        tml_row(1, 0, "REAL", "Main", "10.0", false),
        tml_row(2, 1, "REAL", "Main", "10.0", false),
        tml_row(3, 1, "REAL", "Branch", "0.0", false),
        tml_row(4, 3, "REAL", "Branch", "10.0", false),
    ]);
    let shots: usize = project.lines().iter().map(|s| s.shots.len()).sum();
    assert_eq!(shots, 4, "every REAL row must land as a shot");
    let lengths: Vec<f64> = project
        .lines()
        .iter()
        .flat_map(|s| &s.shots)
        .map(|s| s.length)
        .collect();
    assert!(
        lengths.contains(&crate::parsers::geo::COINCIDENT_TIE_EPSILON_M),
        "{lengths:?}"
    );
    assert!(tie_warning(&warnings).is_some(), "{warnings:?}");
}

/// #812: an unreadable length (`NaN`, which the parser reads as 0) takes the
/// same path instead of failing the import; an EXCLUDED zero-length row is not
/// imported, so it is not counted either.
#[test]
fn an_unreadable_length_is_a_tie_and_an_excluded_zero_row_is_not_counted() {
    let (project, warnings) = import_tml_rows(&[
        tml_row(0, -1, "START", "Main", "0.0", false),
        tml_row(1, 0, "REAL", "Main", "NaN", false),
        tml_row(2, 1, "REAL", "Main", "10.0", false),
        tml_row(3, 2, "REAL", "Main", "0.0", true),
    ]);
    assert_eq!(project.lines()[0].shots.len(), 2);
    let note = tie_warning(&warnings).expect("the NaN row must be reported");
    assert!(note.starts_with("1 shot in the file "), "{note}");
}

/// A file with no bad rows says nothing about ties: the receipt line is not a
/// standing fixture of every import.
#[test]
fn a_clean_file_has_no_tie_warning() {
    let (_, warnings) = import_tml_rows(&[
        tml_row(0, -1, "START", "Main", "0.0", false),
        tml_row(1, 0, "REAL", "Main", "10.0", false),
    ]);
    assert!(tie_warning(&warnings).is_none(), "{warnings:?}");
}

/// #812: a NEGATIVE length takes the tie path too (`create_line` rejects a
/// negative shot just as it rejects a zero one), and a tie keeps the depth step
/// its two ends record: only the horizontal offset collapses.
#[test]
fn a_negative_length_with_a_depth_step_is_a_tie_that_keeps_its_depth() {
    let (project, warnings) = import_tml_rows(&[
        tml_row_at(0, -1, "START", "Main", "0.0", false, 5.0),
        tml_row_at(1, 0, "REAL", "Main", "10.0", false, 5.0),
        tml_row_at(2, 1, "REAL", "Main", "-3.0", false, 8.0),
        tml_row_at(3, 2, "REAL", "Main", "10.0", false, 8.0),
    ]);
    let shots = &project.lines()[0].shots;
    let lengths: Vec<f64> = shots.iter().map(|s| s.length).collect();
    assert_eq!(
        lengths,
        vec![10.0, crate::parsers::geo::COINCIDENT_TIE_EPSILON_M, 10.0]
    );
    let step = shots[1].depth_out - shots[1].depth_in;
    assert!(
        (step - 3.0).abs() < 1e-9,
        "the tie must keep its 3 m depth step, got {step}"
    );
    let note = tie_warning(&warnings).expect("the negative row must be reported");
    assert!(note.starts_with("1 shot in the file "), "{note}");
}
