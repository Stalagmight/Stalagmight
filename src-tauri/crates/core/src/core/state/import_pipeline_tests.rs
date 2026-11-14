//! End-to-end import tests relocated from `parsers::{tml,walls}` ahead of the
//! crate split: they drive the REAL import path through [`AppState`], so they
//! live with the state they construct. `core` depends on `parsers` and
//! `export` non-test, which keeps every import below downward.

use crate::parsers::tml::test_helpers::{
    parse_synthetic_tmlu, tml_shot_xml_depth, tml_shot_xml_gps, wrap_xml_with,
};
use crate::parsers::tml::{convert_tml_to_import_configs, parse_tml};
use crate::parsers::walls::test_support::{convert_one, parse_srv};

/// #431: a round elevation in metres (100.0) stated in feet (328.084)
/// lands in `Project.surface_elevation` correctly converted, not just on
/// the intermediate `TmlFile`/`TmlGeoCoding` structs.
#[test]
fn imperial_elevation_328_084_ft_lands_as_100_m_in_the_project() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;

    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml_with("ft", 328.084, &shots));
    assert!((tml.first_start_absolute_elevation - 100.0).abs() < 1e-3);

    let (sections, closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Elevation Units".to_string(),
        location: None,
    });
    let elevation = crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation);
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        elevation,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    let got = project.surface_elevation.expect("elevation must be set");
    assert!((got - 100.0).abs() < 1e-3, "got {got}, want 100.0 +/- 1e-3");
}

/// #431 round trip: a synthetic `.tmlu` carrying
/// `firstStartAbsoluteElevation = 123.5` survives import into a fresh
/// project and back out through export unchanged. Red before #431 (import
/// dropped it on the floor, the TODO(#411) this test replaces); red again
/// if the export side reverts to reading a zero datum's `depth_reference`.
#[test]
fn entrance_elevation_round_trips_through_import_and_export() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;

    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 123.5, &shots));

    let (sections, closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Round Trip".to_string(),
        location: None,
    });
    let elevation = crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation);
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        elevation,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.surface_elevation, Some(123.5));

    let out_dir = tempfile::tempdir().unwrap();
    let out_path = out_dir.path().join("roundtrip.tmlu");
    crate::export::tml::export_tml(&project, &out_path).unwrap();
    let round_tripped = parse_tml(&out_path).unwrap();
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
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;

    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
    ]
    .join("\n");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "No Clobber".to_string(),
        location: None,
    });

    let first_tml = parse_synthetic_tmlu(&wrap_xml_with("m", 50.0, &shots));
    let (sections, closures, _) = convert_tml_to_import_configs(&first_tml, false, false).unwrap();
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        Some(first_tml.first_start_absolute_elevation),
    );
    assert!(result.success, "first import failed: {:?}", result.error);
    assert_eq!(
        state.get_project_state().unwrap().surface_elevation,
        Some(50.0)
    );

    let second_tml = parse_synthetic_tmlu(&wrap_xml_with("m", 75.0, &shots));
    let (sections, closures, _) = convert_tml_to_import_configs(&second_tml, false, false).unwrap();
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        Some(second_tml.first_start_absolute_elevation),
    );
    assert!(result.success, "second import failed: {:?}", result.error);
    assert_eq!(
        state.get_project_state().unwrap().surface_elevation,
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
        "the clobber must be surfaced naming the KEPT value explicitly \
         (not just both numbers, which a reversed message would also \
         contain), got {warnings:?}"
    );
}

/// #431 tolerance: re-importing the SAME cave via its metric and
/// imperial twins (100.0 m vs 328.084 ft, which converts to
/// 100.00003... m) must not fire the no-clobber "differs" warning - the
/// unit round-trip's residue is far below `f64::EPSILON` (2.2e-16) but
/// well inside the message's own `{:.2}` precision. Red on
/// `f64::EPSILON` used as the tolerance, which reads this pair as
/// different and prints a warning naming "100.00 m" against "100.00 m".
#[test]
fn reimporting_the_metric_and_imperial_twins_of_one_elevation_warns_never() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;

    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
    ]
    .join("\n");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Unit Twins".to_string(),
        location: None,
    });

    let metric_tml = parse_synthetic_tmlu(&wrap_xml_with("m", 100.0, &shots));
    let (sections, closures, _) = convert_tml_to_import_configs(&metric_tml, false, false).unwrap();
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        Some(metric_tml.first_start_absolute_elevation),
    );
    assert!(result.success, "metric import failed: {:?}", result.error);

    let imperial_tml = parse_synthetic_tmlu(&wrap_xml_with("ft", 328.084, &shots));
    let (sections, closures, _) =
        convert_tml_to_import_configs(&imperial_tml, false, false).unwrap();
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        Some(imperial_tml.first_start_absolute_elevation),
    );
    assert!(result.success, "imperial import failed: {:?}", result.error);
    assert_eq!(
        state.get_project_state().unwrap().surface_elevation,
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
    // Runs the REAL import (execute_import_sections enforces
    // `multiple_datums_per_merge_component`), so a successful result proves
    // the placement respects the model's one-datum-per-component invariant.
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;

    let run = |srv: &str| -> (bool, Option<String>, usize) {
        let (sections, closures, _w) = convert_one(parse_srv(srv));
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: "T".to_string(),
            location: None,
        });
        let result = state.execute_import_sections(
            sections,
            closures,
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
        );
        let datums = state
            .get_project_state()
            .map(|p| p.zero_datums.len())
            .unwrap_or(0);
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
    // succeeds (no multiple_datums_per_merge_component violation).
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
        "exactly one datum survives in a single merge component"
    );
}

/// #378 item (d): export -> import -> export must not let GPS anchors drift.
/// Since the exporter writes one START per entrance, an import of that file
/// creates one zero datum per GPS-tagged START - so a survey's anchor set has
/// to be STABLE across cycles, and each anchor has to keep its own
/// coordinates. Decision recorded here: several anchors are accepted as
/// legitimate (one per merge component - the model's own invariant), NOT
/// suppressed on import; the second entrance's datum is exactly what keeps
/// its component placed on re-import, so dropping it would collapse the
/// survey onto one anchor again.
#[test]
fn zero_datums_are_stable_across_export_import_cycles() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;
    use crate::models::{GpsCoordinates, Project, SectionData, ShotData, StationRef, ZeroDatum};

    fn two_entrance_project(datums: &[(u32, f64, f64)]) -> Project {
        let mut project = Project::new("Cycle".to_string(), None);
        for (name, shot_count) in [("North", 1), ("South", 2)] {
            project.add_section(SectionData {
                name: Some(name.to_string()),
                color: "#ff0000".to_string(),
                shots: (0..shot_count)
                    .map(|_| ShotData {
                        length: 5.0,
                        azimuth_in: 90.0,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            });
        }
        for &(section_id, latitude, longitude) in datums {
            project.zero_datums.push(ZeroDatum {
                station_ref: StationRef {
                    section_id,
                    station_index: 0,
                }
                .into(),
                gps: GpsCoordinates::new(latitude, longitude, 0.0).unwrap(),
                accuracy_m: None,
            });
        }
        project
    }

    fn import_cycle(project: &Project, dir: &std::path::Path, label: &str) -> Project {
        let path = dir.join(format!("{label}.tmlu"));
        crate::export::tml::export_tml(project, &path).unwrap();
        let tml = parse_tml(&path).unwrap();
        let (sections, closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: label.to_string(),
            location: None,
        });
        let result = state.execute_tml_import_sections(
            sections,
            closures,
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
            None,
        );
        assert!(result.success, "{label}: import failed: {:?}", result.error);
        state.get_project_state().unwrap()
    }

    fn anchor_set(project: &Project) -> Vec<(i64, i64)> {
        // Coordinates as written-precision integers so f64 noise and datum
        // order cannot flake the comparison.
        let mut set: Vec<(i64, i64)> = project
            .zero_datums
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
    // stays put on a second cycle. Red before #378(e): both STARTs carried the
    // first datum's GPS, so the second anchor's coordinates were lost.
    let original = two_entrance_project(&[(0, 20.25, -87.5), (1, 20.31, -87.42)]);
    let expected = anchor_set(&original);
    let once = import_cycle(&original, dir.path(), "two-anchors-once");
    assert_eq!(anchor_set(&once), expected, "anchors changed on import");
    let twice = import_cycle(&once, dir.path(), "two-anchors-twice");
    assert_eq!(anchor_set(&twice), expected, "anchors drifted on re-import");

    // One anchor, two entrances: the export stamps the frame datum's GPS onto
    // the unanchored entrance (better than null island), so the FIRST cycle
    // legitimately gains one datum for that component - and from then on the
    // set must hold still. This pins the drift to that single, understood step.
    let original = two_entrance_project(&[(0, 20.25, -87.5)]);
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

    // A mid-section datum on a detached entrance: the export SPLITS that
    // section at the datum (forward + pre-zero-datum legs), so the first cycle
    // legitimately turns 2 datums into 3 sections/3 datums - all at the two
    // original coordinates - and from then on the set holds still. Splitting is
    // structurally lossy (the root's datum split always was); what this pins is
    // that the COORDINATES never drift and the growth stops after one cycle.
    let mut original = two_entrance_project(&[(0, 20.25, -87.5)]);
    original.zero_datums.push(ZeroDatum {
        station_ref: StationRef {
            section_id: 1,
            station_index: 1,
        }
        .into(),
        gps: GpsCoordinates::new(20.31, -87.42, 0.0).unwrap(),
        accuracy_m: None,
    });
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
        "the mid-section-split shape must be stable from the first re-import onward"
    );
}

/// #564: a shot comment with no station comment beside it used to export
/// UNMARKED, and the default import routes unmarked lines to the station -
/// so the text silently changed side on every default round trip. Cover the
/// three shapes: shot comment alone, station comment alone, and both on one
/// shot (the shape that always worked, which is why the bug hid).
#[test]
fn comments_keep_their_side_across_a_default_reimport() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;
    use crate::models::{Project, SectionData, ShotData, StationRef};

    let mut original = Project::new("Comments".to_string(), None);
    original.add_section(SectionData {
        name: Some("Main".to_string()),
        color: "#ff0000".to_string(),
        shots: (0..3)
            .map(|i| ShotData {
                length: 5.0,
                azimuth_in: 90.0,
                comment: match i {
                    0 => Some("broken tape".to_string()),
                    2 => Some("silty here".to_string()),
                    _ => None,
                },
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    // Station comments away from station 0 (the START alias documentedly
    // drops a comment there): shot 1's to-station carries one alone, shot 2's
    // to-station rides beside that shot's own comment.
    original.station_comments.insert(
        StationRef {
            section_id: 0,
            station_index: 2,
        }
        .into(),
        "junction with main".to_string(),
    );
    original.station_comments.insert(
        StationRef {
            section_id: 0,
            station_index: 3,
        }
        .into(),
        "beware silt".to_string(),
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("comments.tmlu");
    crate::export::tml::export_tml(&original, &path).unwrap();
    let tml = parse_tml(&path).unwrap();
    // `true` = comments-on-stations, the import dialog's default - the exact
    // setting under which the missing marker moved shot comments.
    let (sections, closures, _) = convert_tml_to_import_configs(&tml, false, true).unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "comments-reimport".to_string(),
        location: None,
    });
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        None,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let reimported = state.get_project_state().unwrap();

    assert_eq!(reimported.sections.len(), 1, "one section must come back");
    let section = &reimported.sections[0];
    let shot_comments: Vec<Option<&str>> =
        section.shots.iter().map(|s| s.comment.as_deref()).collect();
    assert_eq!(
        shot_comments,
        vec![Some("broken tape"), None, Some("silty here")],
        "shot comments must come back on their shots, not move to stations"
    );

    let mut station_comments: Vec<(u32, &str)> = reimported
        .station_comments
        .iter()
        .filter_map(|(k, v)| {
            reimported
                .resolve_key(k)
                .map(|sr| (sr.station_index, v.as_str()))
        })
        .collect();
    station_comments.sort_unstable();
    assert_eq!(
        station_comments,
        vec![(2, "junction with main"), (3, "beware silt")],
        "station comments must come back on their stations and nowhere else"
    );
}

/// Every shot in the project, cross-checked with the real validator - the
/// same call `command_shot.rs` makes.
fn inclination_depth_mismatch_count(project: &crate::models::Project) -> usize {
    use crate::config::ValidationConfig;
    use crate::core::Validator;

    let config = ValidationConfig::default();
    project
        .sections
        .iter()
        .flat_map(|section| {
            section.shots.iter().enumerate().flat_map(|(i, shot)| {
                Validator::validate_shot(&shot.to_shot_data(), &config, section.id, i)
            })
        })
        .filter(|w| w.code == "INCLINATION_DEPTH_MISMATCH")
        .count()
}

/// #482: a clean TML export/import cycle must not fabricate
/// `INCLINATION_DEPTH_MISMATCH` warnings for depth-only surveys. Red on
/// master: export collapses `pitch_in: None` to a wire `0.0` inclination,
/// and import used to read every wire inclination back as `Some`, so the
/// re-imported shot's fabricated `Some(0.0)` disagreed with its real depth
/// change on every one of these depth-only shots.
#[test]
fn depth_only_tml_round_trip_fabricates_no_inclination_depth_mismatch() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;

    // Five depth-only shots (Inclination 0.0, i.e. never sighted), each with
    // a real depth change steep enough to fabricate a mismatch under the old
    // logic - the shape issue #482 reports. `tml_shot_xml_depth` hardcodes
    // `Length` at 10.0, so every |dz| below stays under that (never taking
    // the LENGTH_BELOW_DEPTH_CHANGE branch instead) while still exceeding
    // the default 15-degree threshold (dz > 10*sin(15deg) =~ 2.59m). Chain
    // starts from the START shot's hardcoded Depth=5.0.
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
    let (sections, closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Depth Only".to_string(),
        location: None,
    });
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        Some(tml.first_start_absolute_elevation),
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        inclination_depth_mismatch_count(&project),
        0,
        "first import must not fabricate a mismatch for a never-sighted inclination"
    );

    // Export and re-import to complete the round trip the issue describes.
    let out_dir = tempfile::tempdir().unwrap();
    let out_path = out_dir.path().join("depth_only.tmlu");
    crate::export::tml::export_tml(&project, &out_path).unwrap();
    let round_tripped = parse_tml(&out_path).unwrap();
    let (sections, closures, _) =
        convert_tml_to_import_configs(&round_tripped, false, false).unwrap();

    let state2 = AppState::new().unwrap();
    state2.execute(Command::CreateProject {
        name: "Depth Only Round Trip".to_string(),
        location: None,
    });
    let result2 = state2.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        Some(round_tripped.first_start_absolute_elevation),
    );
    assert!(result2.success, "re-import failed: {:?}", result2.error);

    let project2 = state2.get_project_state().unwrap();
    assert_eq!(
        inclination_depth_mismatch_count(&project2),
        0,
        "a clean export/import round trip must not fabricate inclination-depth \
         mismatch warnings for a depth-only survey"
    );
}

/// #482 deliberate residual loss (a), pinned as chosen behavior rather than
/// left as an unexamined side effect: a shot with a GENUINELY recorded
/// `Some(0.0)` pitch on a STEEP depth-only geometry is erased to `None` by
/// one export/import round trip. `export::tml::pitch_to_inclination` writes
/// both `None` and `Some(0.0)` as the same wire `0.0`
/// (`export/tml.rs::pitch_to_inclination`), so the importer cannot recover
/// which one it was; it resolves the ambiguity toward `None` specifically
/// because the alternative - keeping `Some(0.0)` - would have re-fabricated
/// #482's warning on every ordinary depth-only survey. The warning this
/// erased `Some(0.0)` WOULD have produced (mismatch against a steep implied
/// pitch) was never earnable back from a lossy wire `0.0` in the first
/// place, so nothing observable is lost beyond the raw pitch value itself.
#[test]
fn a_genuinely_recorded_zero_pitch_on_a_steep_shot_is_erased_by_one_round_trip() {
    use crate::core::state::AppState;
    use crate::core::Command;
    use crate::models::ShotData;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Erasure Pin".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#abcdef".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 8.0, // implied ~53 deg, well past the 15 deg default threshold
            pitch_in: Some(0.0),
            pitch_out: Some(0.0),
            ..Default::default()
        }],
    });
    assert!(create.success, "section create failed: {:?}", create.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections[0].shots[0].pitch_in,
        Some(0.0),
        "the recorded reading survives until export"
    );

    let out_dir = tempfile::tempdir().unwrap();
    let out_path = out_dir.path().join("erasure.tmlu");
    crate::export::tml::export_tml(&project, &out_path).unwrap();
    let round_tripped = parse_tml(&out_path).unwrap();
    let (sections, closures, _) =
        convert_tml_to_import_configs(&round_tripped, false, false).unwrap();

    let state2 = AppState::new().unwrap();
    state2.execute(Command::CreateProject {
        name: "Erasure Pin Round Trip".to_string(),
        location: None,
    });
    let result2 = state2.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        crate::core::state::VirtualShotMode::Auto,
        Some(round_tripped.first_start_absolute_elevation),
    );
    assert!(result2.success, "re-import failed: {:?}", result2.error);

    let project2 = state2.get_project_state().unwrap();
    assert_eq!(
        project2.sections[0].shots[0].pitch_in, None,
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
/// afterward. The closure solve (`Project::apply_station_move_internal`,
/// `models/project/closure.rs:265,290`) overwrites `pitch_in`/`pitch_out`
/// with `Some(pitch)` computed from the shot's OWN post-adjust geometry on
/// every shot it touches, so a touched shot's recorded and implied pitch
/// agree exactly regardless of what `pitch_in` held before (`None` or
/// otherwise); an untouched shot keeps `None`, which the validator already
/// treats as nothing to cross-check. If this assertion fails, that is a
/// genuine design question (a closure-adjusted shot disagreeing with its own
/// freshly-written pitch) - report it, do not tune the threshold to hide it.
#[test]
fn depth_only_shots_closed_into_a_loop_do_not_fabricate_a_mismatch() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;
    use crate::models::{ClosureStrategy, ShotData, StationRef};

    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Main", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 13.0, -1.0), // dz +8
        tml_shot_xml_depth(2, 1, "REAL", "Main", 4.0, -1.0),  // dz -9
        tml_shot_xml_depth(3, 2, "REAL", "Main", 11.0, -1.0), // dz +7
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 100.0, &shots));
    let (sections, closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Closure Interaction".to_string(),
        location: None,
    });
    let import_result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        Some(tml.first_start_absolute_elevation),
    );
    assert!(
        import_result.success,
        "import failed: {:?}",
        import_result.error
    );
    let main_id = state.get_project_state().unwrap().sections[0].id;
    assert!(
        state.get_project_state().unwrap().sections[0]
            .shots
            .iter()
            .any(|s| s.pitch_in.is_none()),
        "fixture must actually exercise the None-pitch shape #482 fixed"
    );

    // A short bridging section, left with pitch_in/out at their None default
    // so it cannot itself contribute a mismatch independent of the closure.
    let bridge_id = state
        .execute(Command::CreateSection {
            name: Some("Bridge".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: None,
            shots: vec![ShotData {
                azimuth_in: 270.0,
                azimuth_out: 270.0,
                length: 5.0,
                depth_in: 100.0,
                depth_out: 100.0,
                ..Default::default()
            }],
        })
        .section_id
        .unwrap();

    // Merge one end of the bridge to the tail of the imported chain, leaving
    // the other end - and the imported chain's own start - as the loop's two
    // loose ends for CreateLoopClosure to tie together.
    state.execute(Command::MergeStations {
        station_a: StationRef::new(main_id, 3),
        station_b: StationRef::new(bridge_id, 0),
    });
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(main_id, 0),
        station_b: StationRef::new(bridge_id, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closure.success, "loop closure failed: {:?}", closure.error);

    let project = state.get_project_state().unwrap();
    let count = inclination_depth_mismatch_count(&project);
    assert_eq!(
        count, 0,
        "a closure-adjusted depth-only loop must not surface \
         INCLINATION_DEPTH_MISMATCH - if this fires, it is a real finding, \
         not something to paper over"
    );
}

/// #427 end to end: a chain that changes COLOUR mid-run imports as one section
/// per colour, and a full export/re-import round trip returns the SAME section
/// names and trips - no second cut, no second suffix.
///
/// The export writes each section's name into every one of its shot rows, so a
/// re-import sees two DECLARED names rather than one run to cut; the join then
/// has to refuse to sew them back together, which it does on the colour.
/// Anything that broke either half would show up here as a name like
/// "A (trip 2) (trip 2)", a re-merged "A", or a lost surveyor.
#[test]
fn per_trip_sections_survive_an_export_and_re_import_unchanged() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;
    use crate::parsers::tml::test_helpers::TripShot;

    let second = TripShot {
        section: "A",
        explorer: "Ada Lovelace",
        date: "2025-03-04",
        color: "0x11223344",
        ..TripShot::default()
    };
    let shots = [
        TripShot {
            id: 1,
            from_id: -1,
            section: "A",
            depth: 5.0,
            ..TripShot::default()
        }
        .xml(),
        TripShot {
            id: 2,
            from_id: 1,
            section: "A",
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

    let import_into = |name: &str, tml: &crate::parsers::tml::TmlFile| {
        let (sections, closures, _) = convert_tml_to_import_configs(tml, false, false).unwrap();
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: name.to_string(),
            location: None,
        });
        let result = state.execute_tml_import_sections(
            sections,
            closures,
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
            None,
        );
        assert!(result.success, "import failed: {:?}", result.error);
        state.get_project_state().unwrap()
    };
    let trips = |project: &crate::models::Project| -> Vec<(String, Option<String>)> {
        project
            .sections
            .iter()
            .map(|s| {
                // A plain single Ariane value is the shot's SURVEYORS, as in
                // a .stalx legacy column (#719).
                (
                    s.name.clone().unwrap_or_default(),
                    crate::models::join_names(&s.credit_summary().surveyor_people()),
                )
            })
            .collect()
    };

    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &shots));
    let project = import_into("Trips", &tml);
    let expected = vec![
        ("A".to_string(), Some("Tester".to_string())),
        ("A (trip 2)".to_string(), Some("Ada Lovelace".to_string())),
    ];
    assert_eq!(trips(&project), expected, "the cut lands in the project");

    let out_dir = tempfile::tempdir().unwrap();
    let out_path = out_dir.path().join("trips.tmlu");
    crate::export::tml::export_tml(&project, &out_path).unwrap();
    let reparsed = parse_tml(&out_path).unwrap();
    assert_eq!(
        reparsed
            .sections
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "A (trip 2)"],
        "the round trip must not cut again, re-merge, or double the suffix"
    );

    let round_tripped = import_into("Trips Again", &reparsed);
    assert_eq!(
        trips(&round_tripped),
        expected,
        "and both trips come back with their own surveyor"
    );
}

/// The round trip the trip cut broke (#329): ONE section whose shots carry
/// different people and dates - entered per shot in the app, one colour -
/// exported to Ariane and imported again. Credit is per shot, so it comes back
/// as one section with every shot's credit exactly as it left, instead of one
/// "(trip N)" section per credit change.
#[test]
fn a_section_with_mixed_per_shot_credit_round_trips_through_ariane_as_one_section() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;
    use crate::models::{CreditEdit, CreditTargets, Person, SurveyCredit};
    use crate::parsers::tml::test_helpers::TripShot;

    let shots: Vec<String> = (1..=4)
        .map(|id| {
            TripShot {
                id,
                from_id: id - 1,
                section: "A",
                depth: 5.0 + id as f64,
                ..TripShot::default()
            }
            .xml()
        })
        .collect();
    let import_into = |name: &str, tml: &crate::parsers::tml::TmlFile| {
        let (sections, closures, _) = convert_tml_to_import_configs(tml, false, false).unwrap();
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: name.to_string(),
            location: None,
        });
        let result = state.execute_tml_import_sections(
            sections,
            closures,
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
            None,
        );
        assert!(result.success, "import failed: {:?}", result.error);
        state
    };
    let state = import_into(
        "Mixed",
        &parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &shots.join("\n"))),
    );
    let section = state.get_project_state().unwrap().sections[0].clone();
    assert_eq!(section.shots.len(), 4);

    // Three teams on three days over the one section, as the Detail panel does it.
    let person = |name: &str| Person::named(name).unwrap();
    let teams = [
        (person("Ann Diver"), "2026-03-02"),
        (person("Bo Lead"), "2026-04-11"),
        (person("Cy Cave"), "2026-05-20"),
    ];
    for (index, shot) in section.shots.iter().enumerate().skip(1) {
        let (who, day) = &teams[(index - 1).min(2)];
        let result = state.execute(Command::EditShotCredit {
            targets: CreditTargets::Shots {
                shots: vec![(section.id, shot.id)],
            },
            edit: CreditEdit {
                set_surveyors: Some(vec![who.clone()]),
                set_date: Some(day.parse().ok()),
                ..CreditEdit::default()
            },
        });
        assert!(result.success, "{:?}", result.error);
    }
    let credits = |project: &crate::models::Project| -> Vec<Vec<SurveyCredit>> {
        project
            .sections
            .iter()
            .map(|s| s.shots.iter().map(|shot| shot.credit().clone()).collect())
            .collect()
    };
    let before = credits(&state.get_project_state().unwrap());
    assert_eq!(before.len(), 1);

    let out_dir = tempfile::tempdir().unwrap();
    let out_path = out_dir.path().join("mixed.tmlu");
    crate::export::tml::export_tml(&state.get_project_state().unwrap(), &out_path).unwrap();
    let reparsed = parse_tml(&out_path).unwrap();
    assert_eq!(
        reparsed
            .sections
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["A"],
        "credit changes must not cut the section"
    );

    let again = import_into("Mixed Again", &reparsed);
    assert_eq!(
        credits(&again.get_project_state().unwrap()),
        before,
        "one section, every shot with exactly the credit it left with"
    );
}

/// Review of #719, item 1: one name with no `" / "` in Ariane's `Explorer`
/// field - the common form, e.g. `Tester` - is a SURVEYOR: in the app after
/// the import, in the saved file's legacy `sections.surveyor`, and again after
/// the file is reopened and saved. That is the role a `.stalx` legacy column
/// (and SpeleoDB's reader) has always given the same text, so a file imported
/// before and after #714 no longer disagrees.
#[test]
fn a_single_ariane_name_is_a_surveyor_in_the_app_and_in_the_saved_file() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;
    use crate::parsers::tml::test_helpers::TripShot;

    let shots: Vec<String> = (1..=3)
        .map(|id| {
            TripShot {
                id,
                from_id: id - 1,
                section: "A",
                explorer: "Tester",
                depth: 5.0 + id as f64,
                ..TripShot::default()
            }
            .xml()
        })
        .collect();
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &shots.join("\n")));
    let (sections, closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Single".to_string(),
        location: None,
    });
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        None,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let roles = |project: &crate::models::Project| -> Vec<(Vec<String>, Vec<String>)> {
        let names = |people: &[crate::models::Person]| {
            people.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
        };
        project.sections[0]
            .shots
            .iter()
            .map(|shot| {
                (
                    names(&shot.credit().explorers),
                    names(&shot.credit().surveyors),
                )
            })
            .collect()
    };
    let surveyor_only = (Vec::new(), vec!["Tester".to_string()]);
    let imported = state.get_project_state().unwrap();
    assert!(!imported.sections[0].shots.is_empty());
    assert!(
        roles(&imported).iter().all(|r| r == &surveyor_only),
        "{:?}",
        roles(&imported)
    );

    let column = |path: &std::path::Path| -> Option<String> {
        rusqlite::Connection::open(path)
            .unwrap()
            .query_row("SELECT surveyor FROM sections", [], |r| r.get(0))
            .unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("single.stalx");
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    assert_eq!(column(&path).as_deref(), Some("Tester"));

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
    assert_eq!(column(&path).as_deref(), Some("Tester"));
}

/// Review of #719, item 5c: the TML import preview's length leaves out the
/// virtual shots the import makes References, using the import's own chain
/// rule (`src/utils/tmlImportStats.ts` `jumpShots`). This pins the backend
/// side of the case its vitest pins: a virtual shot between two real shots of
/// a run is a Jump, one dangling off the end a Reference.
#[test]
fn a_bounded_virtual_shot_imports_as_a_jump_and_a_dangling_one_as_a_reference() {
    use crate::core::state::{AppState, VirtualShotMode};
    use crate::core::Command;
    use crate::models::ShotKind;

    let shots = [
        tml_shot_xml_depth(1, 0, "REAL", "A", 1.0, -1.0),
        tml_shot_xml_depth(2, 1, "VIRTUAL", "A", 1.0, -1.0),
        tml_shot_xml_depth(3, 2, "REAL", "A", 1.0, -1.0),
        tml_shot_xml_depth(4, 3, "VIRTUAL", "A", 1.0, -1.0),
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 0.0, &shots));
    let (sections, closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Kinds".to_string(),
        location: None,
    });
    let result = state.execute_tml_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
        None,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    let kinds: Vec<ShotKind> = project
        .sections
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
