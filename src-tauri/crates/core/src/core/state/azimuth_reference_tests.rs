//! Cross-crate behaviour of the line north reference (#438) on the station
//! graph.
//!
//! The model-layer half is `stalagmight_models::azimuth_reference_tests`;
//! these are the assertions that need more than one crate to make: a station
//! move and a split through the execute pipeline (this crate), save / load
//! of the `.stalx` v2 file (persistence) and the Ariane export flag (export,
//! `export::survey::export_tml`).
//!
//! What each is trying to catch is a MISSING or MISDIRECTED correction - a
//! write-back that forgot to remove it, a persistence hop that dropped it, an
//! export that mislabelled it - so every assertion is written against a line
//! whose correction is large enough (10 degrees) that no tolerance hides it.

use stalagmight_models::survey::{LineMeta, Project, RangeDeleteMode, StartAt};
use stalagmight_models::{AzimuthReference, CorrectionSource, LineId, ShotData, StationRef};

use super::commands::Record;
use super::AppState;
use crate::core::Command;

/// 10 degrees east: big enough that a dropped correction cannot pass as noise.
const TEST_CORRECTION: f64 = 10.0;

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    }
}

/// Add a free line named `name` with `reference` to `project`.
fn add_line(
    project: &mut Project,
    name: &str,
    shots: Vec<ShotData>,
    reference: Option<AzimuthReference>,
) -> LineId {
    project
        .create_line(
            LineMeta {
                name: Some(name.to_string()),
                color: "#ffffff".to_string(),
                azimuth_reference: reference,
                ..LineMeta::default()
            },
            StartAt::New,
            shots,
            None,
        )
        .unwrap()
        .value
}

fn project_with(shots: Vec<ShotData>, reference: Option<AzimuthReference>) -> (Project, LineId) {
    let mut project = Project::new("declination", None);
    let id = add_line(&mut project, "S", shots, reference);
    (project, id)
}

fn station_xy(project: &Project, line_id: LineId, index: u32) -> (f64, f64) {
    let [x, y, _] = project
        .raw_positions()
        .slot(project, StationRef::new(line_id, index))
        .expect("station coords");
    (x, y)
}

/// Run a model operation through the execute pipeline as one recorded edit
/// (the move and split commands are other packages'; their `apply` calls
/// the same operation).
fn run_model(
    state: &AppState,
    f: impl FnOnce(&mut Project) -> Result<(), String>,
) -> crate::core::CommandResult {
    let recorded = state.run_recorded(
        |_| Record {
            kind: "edit".to_string(),
            label: "Edit".to_string(),
            ..Record::default()
        },
        |p, _| f(p),
    );
    match recorded {
        Ok(()) => crate::core::CommandResult::success(),
        Err(failure) => failure,
    }
}

/// Moving a station solves in WORLD space and writes an azimuth back onto the
/// shot. Without the inverse conversion the stored bearing gains one correction,
/// and the station lands rotated away from where the user dropped it - by the
/// declination, every time, and the move is repeatable.
#[test]
fn a_station_move_lands_where_it_was_dropped_on_a_corrected_line() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (project, id) = project_with(vec![shot(0.0, 10.0), shot(90.0, 10.0)], Some(reference));
    let before = station_xy(&project, id, 1);
    let station = project.station_at(StationRef::new(id, 1)).unwrap();

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    let result = run_model(&state, |p| {
        p.move_station(station, [1.0, 2.0, 0.0])
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(result.success, "move failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let after = station_xy(&project, id, 1);
    assert!(
        (after.0 - (before.0 + 1.0)).abs() < 1e-9 && (after.1 - (before.1 + 2.0)).abs() < 1e-9,
        "the station did not land where it was dropped: wanted {:?}, got {after:?}",
        (before.0 + 1.0, before.1 + 2.0)
    );
}

/// Splitting a line produces a second line from the same survey. It must
/// inherit the reference, exactly as it inherits surveyor/date/colour.
#[test]
fn a_split_gives_both_halves_the_same_reference() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (project, id) = project_with(
        vec![shot(0.0, 10.0), shot(10.0, 10.0), shot(20.0, 10.0)],
        Some(reference),
    );
    let middle = project.line(id).unwrap().shots[1].id;
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = run_model(&state, |p| {
        p.delete_shot_range(
            id,
            middle,
            middle,
            &RangeDeleteMode::Split {
                name_a: "A".to_string(),
                name_b: "B".to_string(),
            },
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    assert!(result.success, "split failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.lines().len(), 2, "the split should make two lines");
    for line in project.lines() {
        assert_eq!(
            line.azimuth_reference,
            Some(reference),
            "line {} lost the reference across the split",
            line.id
        );
    }
}

/// Save `project` through the app to a fresh file and open it again.
fn round_trip(project: Project) -> Project {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("declination.stalx");
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path });
    assert!(opened.success, "{:?}", opened.error);
    reopened.get_project_state().unwrap()
}

/// Save/load has to preserve all three parts - the north, the number and where
/// the number came from - or the UI explains a value it no longer knows the
/// provenance of, and a policy change recomputes one it should not touch.
#[test]
fn the_reference_and_its_provenance_survive_save_and_load() {
    let recorded =
        AzimuthReference::magnetic(-2.13, CorrectionSource::IgrfAuto { epoch_year: 2024 });
    let (mut project, id) = project_with(vec![shot(12.5, 10.0)], Some(recorded));
    project.settings.default_azimuth_reference =
        Some(AzimuthReference::true_north(CorrectionSource::Manual));

    let loaded = round_trip(project);
    let line = loaded.line(id).expect("line survives the round trip");
    assert_eq!(line.azimuth_reference, Some(recorded));
    assert_eq!(
        loaded.settings.default_azimuth_reference,
        Some(AzimuthReference::true_north(CorrectionSource::Manual))
    );
    assert_eq!(
        line.shots[0].azimuth_in.to_bits(),
        12.5_f64.to_bits(),
        "the stored reading is still the measured one"
    );
}

/// The absent case is the one most files take. It must load as "no reference
/// recorded" - `None`, not `Some(default)` - so that a later project default
/// can still reach the line.
#[test]
fn a_project_that_declares_nothing_round_trips_as_nothing() {
    let (project, id) = project_with(vec![shot(12.5, 10.0)], None);
    assert_eq!(
        project.settings.default_azimuth_reference, None,
        "precondition"
    );
    let loaded = round_trip(project);
    assert_eq!(
        loaded.line(id).unwrap().azimuth_reference,
        None,
        "an unrecorded reference must not materialise as an explicit one"
    );
    assert_eq!(loaded.settings.default_azimuth_reference, None);
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

fn exported_xml(project: &Project) -> String {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("out.tmlu");
    stalagmight_export::export::survey::export_tml(project, &path).unwrap();
    std::fs::read_to_string(&path).unwrap()
}

fn azimuths_in(xml: &str) -> Vec<f64> {
    xml.split("<AZ>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</AZ>").next())
        .filter_map(|text| text.trim().parse::<f64>().ok())
        .collect()
}

fn has(azimuths: &[f64], wanted: f64) -> bool {
    azimuths.iter().any(|a| (a - wanted).abs() < 1e-9)
}

/// A magnetic project hands Ariane the readings and lets Ariane correct them.
#[test]
fn an_all_magnetic_project_exports_its_readings_and_declares_them_magnetic() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (project, _) = project_with(vec![shot(30.0, 10.0)], Some(reference));

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"),
        "magnetic project must declare magnetic"
    );
    assert!(
        has(&azimuths_in(&xml), 30.0),
        "the measured reading must be written verbatim: {:?}",
        azimuths_in(&xml)
    );
}

/// The bug this issue opened on: a true-north project used to be declared
/// magnetic, and Ariane rotated it again on the way in.
#[test]
fn an_all_true_north_project_is_no_longer_declared_magnetic() {
    let (project, _) = project_with(
        vec![shot(30.0, 10.0)],
        Some(AzimuthReference::true_north(CorrectionSource::FileDeclared)),
    );

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"),
        "a true-north project told Ariane its azimuths were magnetic"
    );
    assert!(has(&azimuths_in(&xml), 30.0));
}

/// Mixed: one flag cannot describe two conventions, so every bearing is rotated
/// into true north on the way out and the file says so. The PROJECT keeps its
/// measured readings.
#[test]
fn a_mixed_project_exports_true_north_bearings_and_leaves_the_project_alone() {
    let mut project = Project::new("mixed", None);
    let magnetic = add_line(
        &mut project,
        "Magnetic",
        vec![shot(30.0, 10.0)],
        Some(AzimuthReference::magnetic(
            TEST_CORRECTION,
            CorrectionSource::FileDeclared,
        )),
    );
    add_line(
        &mut project,
        "True",
        vec![shot(200.0, 10.0)],
        Some(AzimuthReference::true_north(CorrectionSource::FileDeclared)),
    );

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"),
        "a mixed project can only honestly declare true north"
    );
    let azimuths = azimuths_in(&xml);
    assert!(
        has(&azimuths, 40.0),
        "the magnetic line's 30 degrees should leave as 40: {azimuths:?}"
    );
    assert!(
        has(&azimuths, 200.0),
        "the true-north line must be written unchanged: {azimuths:?}"
    );
    assert_eq!(
        project.line(magnetic).unwrap().shots[0]
            .azimuth_in
            .to_bits(),
        30.0_f64.to_bits(),
        "export must not write the correction back into the project"
    );
}

/// One cave imported from a georeference-carrying `.tml` (declared, IGRF)
/// plus `.dmp` files from the same cave (raw magnetic, nothing declared):
/// the document stays magnetic, and the readings stay the readings.
#[test]
fn undeclared_magnetic_lines_do_not_make_a_declared_one_mixed() {
    let mut project = Project::new("one cave", None);
    add_line(
        &mut project,
        "From the .tml",
        vec![shot(30.0, 10.0)],
        Some(AzimuthReference::magnetic(
            -2.13,
            CorrectionSource::IgrfAuto { epoch_year: 2025 },
        )),
    );
    add_line(
        &mut project,
        "From a .dmp",
        vec![shot(200.0, 10.0)],
        Some(AzimuthReference::magnetic(0.0, CorrectionSource::Unknown)),
    );

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"),
        "an undeclared magnetic line must not push the document off magnetic"
    );
    let azimuths = azimuths_in(&xml);
    assert!(
        has(&azimuths, 30.0) && has(&azimuths, 200.0),
        "both lines' readings must leave verbatim: {azimuths:?}"
    );
}

/// Two DECLARED corrections that disagree are the case the mixed path is for.
#[test]
fn two_different_declared_corrections_are_mixed() {
    let mut project = Project::new("two declinations", None);
    add_line(
        &mut project,
        "A",
        vec![shot(30.0, 10.0)],
        Some(AzimuthReference::magnetic(
            5.0,
            CorrectionSource::FileDeclared,
        )),
    );
    add_line(
        &mut project,
        "B",
        vec![shot(30.0, 10.0)],
        Some(AzimuthReference::magnetic(
            -3.0,
            CorrectionSource::FileDeclared,
        )),
    );

    let xml = exported_xml(&project);
    assert!(xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"));
    let azimuths = azimuths_in(&xml);
    assert!(
        has(&azimuths, 35.0) && has(&azimuths, 27.0),
        "each line's own correction should be applied: {azimuths:?}"
    );
}

/// #438 review item 4: a line whose offset from true north is unknown cannot
/// be put into true north, so the rewrite is refused and the file never
/// claims true north over bearings it did not rotate.
#[test]
fn a_true_declaration_beside_an_unknown_one_never_claims_true_north() {
    let mut project = Project::new("unresolvable", None);
    add_line(
        &mut project,
        "True",
        vec![shot(30.0, 10.0)],
        Some(AzimuthReference::true_north(CorrectionSource::FileDeclared)),
    );
    add_line(
        &mut project,
        "Raw magnetic",
        vec![shot(200.0, 10.0)],
        Some(AzimuthReference::magnetic(0.0, CorrectionSource::Unknown)),
    );

    let xml = exported_xml(&project);
    let azimuths = azimuths_in(&xml);
    assert!(
        has(&azimuths, 30.0) && has(&azimuths, 200.0),
        "premise: nothing was rotated: {azimuths:?}"
    );
    assert!(
        !xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"),
        "the file declared true north over bearings it did not rotate"
    );
    assert!(xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"));
}

/// The same refusal on the non-uniform path: two disagreeing declared
/// corrections AND an unknown one cannot be rotated into one frame either.
#[test]
fn a_disagreeing_pair_with_an_unknown_third_is_refused_rather_than_half_rotated() {
    let mut project = Project::new("unresolvable mix", None);
    for (name, reference) in [
        (
            "A",
            AzimuthReference::magnetic(5.0, CorrectionSource::FileDeclared),
        ),
        (
            "B",
            AzimuthReference::magnetic(-3.0, CorrectionSource::FileDeclared),
        ),
        (
            "C",
            AzimuthReference::magnetic(0.0, CorrectionSource::Unknown),
        ),
    ] {
        add_line(&mut project, name, vec![shot(30.0, 10.0)], Some(reference));
    }

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"),
        "one line with no correction on record makes the rewrite impossible"
    );
    let azimuths = azimuths_in(&xml);
    assert!(
        azimuths
            .iter()
            .all(|a| (a - 30.0).abs() < 1e-9 || a.abs() < 1e-12),
        "refusing the rewrite means writing the readings as stored: {azimuths:?}"
    );
}

/// A project with no references at all is the historical case, and it has to
/// keep producing the historical document.
#[test]
fn an_unreferenced_project_still_exports_the_historical_declaration() {
    let (project, _) = project_with(vec![shot(30.0, 10.0)], None);
    let xml = exported_xml(&project);
    assert!(xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"));
    assert!(has(&azimuths_in(&xml), 30.0));
}

/// Grid bearings are neither magnetic nor true: declaring them either way hands
/// Ariane a rotation it will not undo, so they are converted out.
#[test]
fn a_grid_referenced_project_is_converted_rather_than_mislabelled() {
    let (project, _) = project_with(
        vec![shot(30.0, 10.0)],
        Some(AzimuthReference::grid(2.0, CorrectionSource::FileDeclared)),
    );
    let xml = exported_xml(&project);
    assert!(xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"));
    assert!(
        has(&azimuths_in(&xml), 32.0),
        "the grid correction should be applied on the way out: {:?}",
        azimuths_in(&xml)
    );
}
