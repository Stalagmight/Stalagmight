//! Which zero datums are entrances (#741): placement datums always, a GPS fix
//! by the user's answer or, by default, when it sits on a section's first or
//! last station. Synthetic surveys only.

use crate::{local_to_gps, GpsCoordinates, Project, SectionData, SectionId, ShotData, StationRef};

const ORIGIN_LAT: f64 = 20.5;
const ORIGIN_LON: f64 = -87.25;

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(ORIGIN_LAT, ORIGIN_LON, 0.0).unwrap()
}

/// The GPS position `north` metres north of the origin.
fn gps_north(north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(0.0, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

/// One section of six 50 m shots due north (stations 0..=6, 300 m), placed by
/// a datum on station 0.
fn line() -> (Project, SectionId) {
    let mut project = Project::new("Line".to_string(), None);
    let shots = (0..6)
        .map(|_| ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 50.0,
            ..Default::default()
        })
        .collect();
    let a = project
        .add_section(SectionData {
            shots,
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project
        .set_zero_datum(StationRef::new(a, 0), origin())
        .unwrap();
    (project, a)
}

fn fix(project: &mut Project, at: StationRef, entrance: Option<bool>) {
    project
        .set_zero_datum(at, gps_north(50.0 * f64::from(at.station_index)))
        .unwrap();
    if entrance.is_some() {
        assert!(project.set_zero_datum_entrance(at, entrance));
    }
}

fn wire_entrances(project: &Project) -> Vec<(u32, bool)> {
    project
        .zero_datums_wire()
        .iter()
        .map(|w| (w.station_ref.station_index, w.is_entrance))
        .collect()
}

#[test]
fn a_mid_passage_fix_is_not_an_entrance_and_a_fix_at_a_section_end_is() {
    let (mut project, a) = line();
    fix(&mut project, StationRef::new(a, 3), None);
    fix(&mut project, StationRef::new(a, 6), None);

    assert_eq!(
        wire_entrances(&project),
        vec![(0, true), (3, false), (6, true)]
    );
    assert_eq!(
        project.entrance_datum_refs(),
        vec![StationRef::new(a, 0), StationRef::new(a, 6)]
    );
}

#[test]
fn the_user_answer_overrides_the_default_both_ways() {
    let (mut project, a) = line();
    fix(&mut project, StationRef::new(a, 3), Some(true));
    fix(&mut project, StationRef::new(a, 6), Some(false));

    assert_eq!(
        wire_entrances(&project),
        vec![(0, true), (3, true), (6, false)]
    );

    // Back to the default rule.
    assert!(project.set_zero_datum_entrance(StationRef::new(a, 3), None));
    assert!(project.set_zero_datum_entrance(StationRef::new(a, 6), None));
    assert_eq!(
        wire_entrances(&project),
        vec![(0, true), (3, false), (6, true)]
    );
}

#[test]
fn a_placement_datum_is_an_entrance_whatever_its_stored_answer() {
    let (mut project, a) = line();
    assert!(project.set_zero_datum_entrance(StationRef::new(a, 0), Some(false)));
    assert_eq!(wire_entrances(&project), vec![(0, true)]);
    assert_eq!(project.entrance_datum_refs(), vec![StationRef::new(a, 0)]);
}

#[test]
fn setting_the_answer_on_a_station_with_no_datum_reports_false() {
    let (mut project, a) = line();
    assert!(!project.set_zero_datum_entrance(StationRef::new(a, 2), Some(true)));
    assert!(project.zero_datums[0].entrance.is_none());
}

#[test]
fn re_setting_a_fix_gps_keeps_its_answer() {
    let (mut project, a) = line();
    fix(&mut project, StationRef::new(a, 3), Some(true));
    project
        .set_zero_datum(StationRef::new(a, 3), gps_north(160.0))
        .unwrap();
    assert_eq!(wire_entrances(&project), vec![(0, true), (3, true)]);
}

#[test]
fn the_preview_reports_the_entrance_a_new_or_stored_datum_would_have() {
    let (mut project, a) = line();
    let preview = |p: &Project, i: u32| {
        p.preview_zero_datum(StationRef::new(a, i), &gps_north(50.0 * f64::from(i)))
            .unwrap()
            .is_entrance
    };
    assert!(preview(&project, 0), "the placement datum");
    assert!(!preview(&project, 3), "a new mid-passage fix");
    assert!(preview(&project, 6), "a new fix at the section's end");

    fix(&mut project, StationRef::new(a, 3), Some(true));
    assert!(preview(&project, 3), "a stored fix keeps its answer");
}

#[test]
fn the_entrance_answer_round_trips_through_serde_and_is_absent_by_default() {
    let (mut project, a) = line();
    fix(&mut project, StationRef::new(a, 3), Some(true));
    let json = serde_json::to_value(&project.zero_datums).unwrap();
    assert!(json[0].get("entrance").is_none(), "None is not written");
    assert_eq!(json[1]["entrance"], serde_json::Value::Bool(true));
    let back: Vec<crate::ZeroDatum> = serde_json::from_value(json).unwrap();
    assert_eq!(back[1].entrance, Some(true));
    assert_eq!(back[0].entrance, None);
}
