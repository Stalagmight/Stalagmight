//! Cross-crate behaviour of the section north reference (#438).
//!
//! The model-layer half is `stalagmight_models::azimuth_reference_tests`; these
//! are the assertions that need more than one crate to make: a station move and
//! a split (this crate), save / load (persistence) and the Ariane export flag
//! (export).
//!
//! What each is trying to catch is a MISSING or MISDIRECTED correction - a
//! write-back that forgot to remove it, a persistence hop that dropped it, an
//! export that mislabelled it - so every assertion is written against a section
//! whose correction is large enough (10 degrees) that no tolerance hides it.

use crate::test_fixtures::{state_holding, straight_shot};
use stalagmight_models::{
    AzimuthReference, CorrectionSource, Project, SectionData, SectionId, ShotData, StationRef,
};

/// 10 degrees east: big enough that a dropped correction cannot pass as noise.
const TEST_CORRECTION: f64 = 10.0;

fn project_with(shots: Vec<ShotData>, reference: Option<AzimuthReference>) -> (Project, SectionId) {
    let mut project = Project::new("declination".to_string(), None);
    let id = project
        .add_section(SectionData {
            name: Some("S".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots,
        })
        .id;
    project.get_section_mut(id).unwrap().azimuth_reference = reference;
    project.ensure_shot_uuids();
    (project, id)
}

fn station_xy(project: &Project, section_id: SectionId, index: u32) -> (f64, f64) {
    let coords = project
        .compute_station_raw_coords(StationRef::new(section_id, index))
        .expect("station coords");
    (coords.x, coords.y)
}

/// Moving a station solves in WORLD space and writes an azimuth back onto the
/// shot. Without the inverse conversion the stored bearing gains one correction,
/// and the station lands rotated away from where the user dropped it - by the
/// declination, every time, and the move is repeatable.
#[test]
fn a_station_move_lands_where_it_was_dropped_on_a_corrected_section() {
    use crate::core::command::Command;

    use stalagmight_models::Coordinates;

    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (project, id) = project_with(
        vec![straight_shot(0.0, 10.0), straight_shot(90.0, 10.0)],
        Some(reference),
    );
    let before = station_xy(&project, id, 1);

    let state = state_holding(project);
    let result = state.execute(Command::MoveStation {
        station_ref: StationRef::new(id, 1),
        offset: Coordinates::new(1.0, 2.0, 0.0),
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

/// Splitting a section produces a second section from the same survey. It must
/// inherit the reference, exactly as it inherits surveyor/date/colour.
#[test]
fn a_split_gives_both_halves_the_same_reference() {
    use crate::core::state::AppState;

    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let state = AppState::new().unwrap();
    let mut project = Project::new("split".to_string(), None);
    let id = project
        .add_section(SectionData {
            name: Some("S".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![
                straight_shot(0.0, 10.0),
                straight_shot(10.0, 10.0),
                straight_shot(20.0, 10.0),
            ],
        })
        .id;
    project.get_section_mut(id).unwrap().azimuth_reference = Some(reference);
    project.ensure_shot_uuids();
    let middle = project.get_section(id).unwrap().shots[1].id;
    state.set_project_for_tests(project);

    let result = state.execute_delete_shot_and_split(id, middle, "A".to_string(), "B".to_string());
    assert!(result.success, "split failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        2,
        "the split should make two sections"
    );
    for section in &project.sections {
        assert_eq!(
            section.azimuth_reference,
            Some(reference),
            "section {} lost the reference across the split",
            section.id
        );
    }
}

/// Save/load has to preserve all three parts - the north, the number and where
/// the number came from - or the UI explains a value it no longer knows the
/// provenance of, and a policy change recomputes one it should not touch.
#[test]
fn the_reference_and_its_provenance_survive_save_and_load() {
    use stalagmight_persistence::persistence::sqlite::Database;
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("declination.stalx");
    let mut db = Database::open(&path).unwrap();

    let recorded =
        AzimuthReference::magnetic(-2.13, CorrectionSource::IgrfAuto { epoch_year: 2024 });
    let (mut project, id) = project_with(vec![straight_shot(12.5, 10.0)], Some(recorded));
    project.default_azimuth_reference =
        Some(AzimuthReference::true_north(CorrectionSource::Manual));
    db.save_project(&project).unwrap();

    let loaded = db.load_project().unwrap();
    let section = loaded
        .get_section(id)
        .expect("section survives the round trip");
    assert_eq!(section.azimuth_reference, Some(recorded));
    assert_eq!(
        loaded.default_azimuth_reference,
        Some(AzimuthReference::true_north(CorrectionSource::Manual))
    );
    assert_eq!(
        section.shots[0].azimuth_in, 12.5,
        "the stored reading is still the measured one"
    );
}

/// The absent case is the one every existing file takes. It must load as "no
/// reference recorded" - `None`, not `Some(default)` - so that a later project
/// default can still reach the section.
#[test]
fn a_project_that_declares_nothing_round_trips_as_nothing() {
    use stalagmight_persistence::persistence::sqlite::Database;
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("plain.stalx");
    let mut db = Database::open(&path).unwrap();

    let (project, id) = project_with(vec![straight_shot(12.5, 10.0)], None);
    assert_eq!(project.default_azimuth_reference, None, "precondition");
    db.save_project(&project).unwrap();

    let loaded = db.load_project().unwrap();
    assert_eq!(
        loaded.get_section(id).unwrap().azimuth_reference,
        None,
        "an unrecorded reference must not materialise as an explicit one"
    );
    assert_eq!(loaded.default_azimuth_reference, None);
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

fn exported_xml(project: &Project) -> String {
    use tempfile::TempDir;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("out.tmlu");
    stalagmight_export::export::export_tml(project, &path).unwrap();
    std::fs::read_to_string(&path).unwrap()
}

fn azimuths_in(xml: &str) -> Vec<f64> {
    xml.split("<AZ>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</AZ>").next())
        .filter_map(|text| text.trim().parse::<f64>().ok())
        .collect()
}

/// A magnetic project hands Ariane the readings and lets Ariane correct them.
#[test]
fn an_all_magnetic_project_exports_its_readings_and_declares_them_magnetic() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (project, _) = project_with(vec![straight_shot(30.0, 10.0)], Some(reference));

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"),
        "magnetic project must declare magnetic"
    );
    assert!(
        azimuths_in(&xml).contains(&30.0),
        "the measured reading must be written verbatim: {:?}",
        azimuths_in(&xml)
    );
}

/// The bug this issue opened on: a true-north project used to be declared
/// magnetic, and Ariane rotated it again on the way in.
#[test]
fn an_all_true_north_project_is_no_longer_declared_magnetic() {
    let (project, _) = project_with(
        vec![straight_shot(30.0, 10.0)],
        Some(AzimuthReference::true_north(CorrectionSource::FileDeclared)),
    );

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"),
        "a true-north project told Ariane its azimuths were magnetic"
    );
    assert!(azimuths_in(&xml).contains(&30.0));
}

/// Mixed: one flag cannot describe two conventions, so every bearing is rotated
/// into true north on the way out and the file says so. The PROJECT keeps its
/// measured readings.
#[test]
fn a_mixed_project_exports_true_north_bearings_and_leaves_the_project_alone() {
    let mut project = Project::new("mixed".to_string(), None);
    let magnetic = project
        .add_section(SectionData {
            name: Some("Magnetic".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![straight_shot(30.0, 10.0)],
        })
        .id;
    let true_north = project
        .add_section(SectionData {
            name: Some("True".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![straight_shot(200.0, 10.0)],
        })
        .id;
    project.get_section_mut(magnetic).unwrap().azimuth_reference = Some(
        AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared),
    );
    project
        .get_section_mut(true_north)
        .unwrap()
        .azimuth_reference = Some(AzimuthReference::true_north(CorrectionSource::FileDeclared));
    project.ensure_shot_uuids();

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"),
        "a mixed project can only honestly declare true north"
    );
    let azimuths = azimuths_in(&xml);
    assert!(
        azimuths.contains(&40.0),
        "the magnetic section's 30 degrees should leave as 40: {azimuths:?}"
    );
    assert!(
        azimuths.contains(&200.0),
        "the true-north section must be written unchanged: {azimuths:?}"
    );

    assert_eq!(
        project.get_section(magnetic).unwrap().shots[0].azimuth_in,
        30.0,
        "export must not write the correction back into the project"
    );
}

/// The case the fixture corpus is made of: one cave imported from a
/// georeference-carrying `.tml` (declared, IGRF) plus `.dmp` files from the same
/// cave (raw magnetic, nothing declared).
///
/// Counting the undeclared sections as "correction 0" made this MIXED, which
/// rewrote every bearing in the export and declared the whole file true north -
/// asserting about the `.dmp` sections the one thing known to be false. The
/// document stays magnetic, and the readings stay the readings.
#[test]
fn undeclared_magnetic_sections_do_not_make_a_declared_one_mixed() {
    let mut project = Project::new("naharon".to_string(), None);
    let declared = project
        .add_section(SectionData {
            name: Some("From the .tml".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![straight_shot(30.0, 10.0)],
        })
        .id;
    let undeclared = project
        .add_section(SectionData {
            name: Some("From a .dmp".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![straight_shot(200.0, 10.0)],
        })
        .id;
    project.get_section_mut(declared).unwrap().azimuth_reference = Some(
        AzimuthReference::magnetic(-2.13, CorrectionSource::IgrfAuto { epoch_year: 2025 }),
    );
    project
        .get_section_mut(undeclared)
        .unwrap()
        .azimuth_reference = Some(AzimuthReference::magnetic(0.0, CorrectionSource::Unknown));
    project.ensure_shot_uuids();

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"),
        "an undeclared magnetic section must not push the document off magnetic"
    );
    let azimuths = azimuths_in(&xml);
    assert!(
        azimuths.contains(&30.0) && azimuths.contains(&200.0),
        "both sections' readings must leave verbatim: {azimuths:?}"
    );
}

/// Two DECLARED corrections that disagree are the case the mixed path is for.
#[test]
fn two_different_declared_corrections_are_mixed() {
    let mut project = Project::new("two declinations".to_string(), None);
    let a = project
        .add_section(SectionData {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![straight_shot(30.0, 10.0)],
        })
        .id;
    let b = project
        .add_section(SectionData {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![straight_shot(30.0, 10.0)],
        })
        .id;
    project.get_section_mut(a).unwrap().azimuth_reference = Some(AzimuthReference::magnetic(
        5.0,
        CorrectionSource::FileDeclared,
    ));
    project.get_section_mut(b).unwrap().azimuth_reference = Some(AzimuthReference::magnetic(
        -3.0,
        CorrectionSource::FileDeclared,
    ));
    project.ensure_shot_uuids();

    let xml = exported_xml(&project);
    assert!(xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"));
    let azimuths = azimuths_in(&xml);
    assert!(
        azimuths.contains(&35.0) && azimuths.contains(&27.0),
        "each section's own correction should be applied: {azimuths:?}"
    );
}

/// #438 review item 4: the escape hatch used to emit a FALSE claim.
///
/// A declared-true section beside one with no correction on record took the
/// `MixedToTrue` arm, where the undeclared section's reference resolves to
/// identity - so its bearings went out UNROTATED under
/// `useMagneticAzimuth=false`, which is exactly the claim the branch exists to
/// prevent. A section whose offset from true north is unknown cannot be put
/// into true north, so the rewrite is refused and the note says why.
#[test]
fn a_true_declaration_beside_an_unknown_one_never_claims_true_north() {
    let mut project = Project::new("unresolvable".to_string(), None);
    let declared = project
        .add_section(SectionData {
            name: Some("True".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![straight_shot(30.0, 10.0)],
        })
        .id;
    let unknown = project
        .add_section(SectionData {
            name: Some("Raw magnetic".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![straight_shot(200.0, 10.0)],
        })
        .id;
    project.get_section_mut(declared).unwrap().azimuth_reference =
        Some(AzimuthReference::true_north(CorrectionSource::FileDeclared));
    project.get_section_mut(unknown).unwrap().azimuth_reference =
        Some(AzimuthReference::magnetic(0.0, CorrectionSource::Unknown));
    project.ensure_shot_uuids();

    let xml = exported_xml(&project);
    let azimuths = azimuths_in(&xml);
    // Both readings leave untouched - so the file must NOT say true north.
    assert!(
        azimuths.contains(&30.0) && azimuths.contains(&200.0),
        "premise: nothing was rotated, so an unrotated bearing is in the file: {azimuths:?}"
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
    let mut project = Project::new("unresolvable mix".to_string(), None);
    let mut add = |name: &str, azimuth: f64| {
        project
            .add_section(SectionData {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#ffffff".to_string(),
                direction: None,
                quality: 1.0,
                shots: vec![straight_shot(azimuth, 10.0)],
            })
            .id
    };
    let a = add("A", 30.0);
    let b = add("B", 30.0);
    let c = add("C", 30.0);
    project.get_section_mut(a).unwrap().azimuth_reference = Some(AzimuthReference::magnetic(
        5.0,
        CorrectionSource::FileDeclared,
    ));
    project.get_section_mut(b).unwrap().azimuth_reference = Some(AzimuthReference::magnetic(
        -3.0,
        CorrectionSource::FileDeclared,
    ));
    project.get_section_mut(c).unwrap().azimuth_reference =
        Some(AzimuthReference::magnetic(0.0, CorrectionSource::Unknown));
    project.ensure_shot_uuids();

    let xml = exported_xml(&project);
    assert!(
        xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"),
        "one section with no correction on record makes the rewrite impossible"
    );
    let azimuths = azimuths_in(&xml);
    assert!(
        azimuths
            .iter()
            .all(|a| (a - 30.0).abs() < 1e-9 || *a == 0.0),
        "refusing the rewrite means writing the readings as stored: {azimuths:?}"
    );
}

/// A project with no references at all is the historical case, and it has to
/// keep producing the historical document.
#[test]
fn an_unreferenced_project_still_exports_the_historical_declaration() {
    let (project, _) = project_with(vec![straight_shot(30.0, 10.0)], None);
    let xml = exported_xml(&project);
    assert!(xml.contains("<useMagneticAzimuth>true</useMagneticAzimuth>"));
    assert!(azimuths_in(&xml).contains(&30.0));
}

/// Grid bearings are neither magnetic nor true: declaring them either way hands
/// Ariane a rotation it will not undo, so they are converted out.
#[test]
fn a_grid_referenced_project_is_converted_rather_than_mislabelled() {
    let (project, _) = project_with(
        vec![straight_shot(30.0, 10.0)],
        Some(AzimuthReference::grid(2.0, CorrectionSource::FileDeclared)),
    );
    let xml = exported_xml(&project);
    assert!(xml.contains("<useMagneticAzimuth>false</useMagneticAzimuth>"));
    assert!(
        azimuths_in(&xml).contains(&32.0),
        "the grid correction should be applied on the way out: {:?}",
        azimuths_in(&xml)
    );
}
