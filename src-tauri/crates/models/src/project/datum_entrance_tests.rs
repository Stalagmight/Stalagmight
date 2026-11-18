//! Which zero datums are entrances (#741): placement datums always, a GPS fix
//! by the user's answer or, by default, when it sits on a line's first or
//! last station. Synthetic surveys only.

use crate::{local_to_gps, GpsCoordinates, LineData, LineId, Project, ShotData, StationRef};

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

/// One line of six 50 m shots due north (stations 0..=6, 300 m), placed by
/// a datum on station 0.
fn line() -> (Project, LineId) {
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
        .add_line(LineData {
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
fn a_mid_passage_fix_is_not_an_entrance_and_a_fix_at_a_line_end_is() {
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
    assert!(preview(&project, 6), "a new fix at the line's end");

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

/// Distance from the entrances, by station index along the line, for the
/// gradient's entrance default.
fn gradient(project: &Project) -> Vec<Option<f64>> {
    let stations = project.compute_station_coordinates();
    project.distance_field_from(&stations, None).distances
}

#[test]
fn a_mid_passage_fix_is_not_a_root_of_penetration_or_the_distance_gradient() {
    let (mut project, a) = line();
    let stations = project.compute_station_coordinates();
    assert!((project.calculate_penetration(&stations) - 300.0).abs() < 1e-6);

    // A radiolocation fix at station 3 (150 m in) leaves both unchanged.
    fix(&mut project, StationRef::new(a, 3), None);
    let stations = project.compute_station_coordinates();
    assert!((project.calculate_penetration(&stations) - 300.0).abs() < 1e-6);
    let mid = gradient(&project)[3].unwrap();
    assert!((mid - 150.0).abs() < 1e-6, "{mid}");

    // Ticked as an entrance, it becomes a root: the far end is 150 m from it.
    assert!(project.set_zero_datum_entrance(StationRef::new(a, 3), Some(true)));
    let stations = project.compute_station_coordinates();
    assert!((project.calculate_penetration(&stations) - 150.0).abs() < 1e-6);
    assert!(gradient(&project)[3].unwrap().abs() < 1e-6);
}

#[test]
fn a_fix_at_the_far_end_is_a_root_unless_the_user_says_otherwise() {
    let (mut project, a) = line();
    fix(&mut project, StationRef::new(a, 6), None);
    let stations = project.compute_station_coordinates();
    assert!((project.calculate_penetration(&stations) - 150.0).abs() < 1e-6);

    assert!(project.set_zero_datum_entrance(StationRef::new(a, 6), Some(false)));
    let stations = project.compute_station_coordinates();
    assert!((project.calculate_penetration(&stations) - 300.0).abs() < 1e-6);
}

#[test]
fn start_from_entrance_does_not_start_at_a_mid_passage_fix() {
    use super::gas::{CylRole, Cylinder, Diver, TankMaterial};
    use crate::config::GasConfig;

    let (mut project, a) = line();
    fix(&mut project, StationRef::new(a, 3), None);
    let diver = Diver::new(20.0, None).unwrap();
    let tank = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let gas_at = |project: &Project, i: u32| {
        let field = project.compute_gas_reach_field(
            None,
            None,
            &diver,
            std::slice::from_ref(&tank),
            &GasConfig::default(),
        );
        let at = field
            .station_refs
            .iter()
            .position(|r| *r == StationRef::new(a, i))
            .unwrap();
        field.gas_to_reach[at].unwrap()
    };
    assert!(
        gas_at(&project, 0).abs() < 1e-9,
        "the placement datum is a root"
    );
    assert!(gas_at(&project, 3) > 0.0, "the mid-passage fix is not");

    assert!(project.set_zero_datum_entrance(StationRef::new(a, 3), Some(true)));
    assert!(gas_at(&project, 3).abs() < 1e-9, "ticked, it is");
}
