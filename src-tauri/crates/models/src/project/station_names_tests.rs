//! Station names (`Project::station_names`) stay on their PHYSICAL station.
//!
//! The field exists so an imported MNemo station number survives every edit
//! that shifts positional station indexes. Each test names every station of a
//! straight section after its original index, makes one edit, and checks which
//! name each surviving station carries.

use super::*;

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

/// One section of `count` 10 m shots, every station named after its index.
fn named_section(count: usize) -> (Project, SectionId) {
    let mut project = Project::new("Names".to_string(), None);
    let id = project
        .add_section(SectionData {
            shots: (0..count).map(|i| shot(i as f64 * 10.0, 10.0)).collect(),
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    for index in 0..=count as u32 {
        let key = project.key_for_ref(StationRef::new(id, index)).unwrap();
        project.station_names.insert(key, index.to_string());
    }
    (project, id)
}

/// The name of every station of `id`, in station order.
fn names(project: &Project, id: SectionId) -> Vec<Option<String>> {
    let count = project.get_section(id).unwrap().shots.len() as u32;
    (0..=count)
        .map(|i| {
            project
                .station_name_for_ref(StationRef::new(id, i))
                .cloned()
        })
        .collect()
}

fn some(values: &[&str]) -> Vec<Option<String>> {
    values.iter().map(|v| Some((*v).to_string())).collect()
}

fn shot_id(project: &Project, id: SectionId, position: usize) -> ShotId {
    project.get_section(id).unwrap().shots[position].id
}

#[test]
fn deleting_the_first_shot_keeps_the_new_start_on_its_old_name() {
    let (mut project, id) = named_section(3);
    let first = shot_id(&project, id, 0);
    project.remove_shot_with_rewire(id, first).unwrap();

    // Station 0 (named "0") is gone; old station 1 is the new start.
    assert_eq!(names(&project, id), some(&["1", "2", "3"]));
    assert_eq!(project.station_names.len(), 3);
}

#[test]
fn deleting_an_interior_shot_drops_only_the_station_it_removes() {
    let (mut project, id) = named_section(3);
    let middle = shot_id(&project, id, 1);
    project.remove_shot_with_rewire(id, middle).unwrap();

    // Deleting shot 1 (station 1 -> 2) removes station 2; the rest keep theirs.
    assert_eq!(names(&project, id), some(&["0", "1", "3"]));
    assert_eq!(project.station_names.len(), 3);
}

#[test]
fn inserting_shots_leaves_the_new_station_unnamed() {
    let (mut project, id) = named_section(2);

    let first = shot_id(&project, id, 0);
    project
        .insert_shot_before(id, first, shot(0.0, 5.0), None, None)
        .unwrap();
    // A prepend puts a new station in front; the old start keeps "0".
    assert_eq!(
        names(&project, id),
        vec![None, Some("0".into()), Some("1".into()), Some("2".into())]
    );

    // An interior insert after the shot ending at "1": the new station sits
    // between "1" and "2".
    let after = shot_id(&project, id, 1);
    project
        .insert_shot_after(id, after, shot(0.0, 5.0), None, None)
        .unwrap();
    assert_eq!(
        names(&project, id),
        vec![
            None,
            Some("0".into()),
            Some("1".into()),
            None,
            Some("2".into())
        ]
    );
}

#[test]
fn splitting_a_shot_adds_one_unnamed_station() {
    let (mut project, id) = named_section(2);
    let first = shot_id(&project, id, 0);
    project.split_shot_at_distance(id, first, 4.0).unwrap();

    assert_eq!(
        names(&project, id),
        vec![Some("0".into()), None, Some("1".into()), Some("2".into())]
    );
}

#[test]
fn reversing_a_section_keeps_each_name_on_its_physical_station() {
    let (mut project, id) = named_section(3);
    assert!(project.reverse_section(id, false));

    // Station i became station N - i.
    assert_eq!(names(&project, id), some(&["3", "2", "1", "0"]));
}

#[test]
fn deleting_a_section_drops_its_names_only() {
    let (mut project, a) = named_section(1);
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 10.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let b0 = project.key_for_ref(StationRef::new(b, 0)).unwrap();
    project.station_names.insert(b0, "0".to_string());

    project.cleanup_section_references(b);

    assert_eq!(names(&project, a), some(&["0", "1"]));
    assert!(project.station_names.keys().all(|k| k.section_id == a));
}

#[test]
fn repair_drops_a_name_whose_station_no_longer_exists() {
    let (mut project, id) = named_section(1);
    project.station_names.insert(
        StationKey::new(id, StationAnchor::AfterShot("no-such-shot".to_string())),
        "ghost".to_string(),
    );

    project.repair_structural_integrity();

    assert_eq!(names(&project, id), some(&["0", "1"]));
    assert_eq!(project.station_names.len(), 2);
}

#[test]
fn computed_stations_carry_their_name_beside_their_comment() {
    let (mut project, id) = named_section(2);
    let middle = project.key_for_ref(StationRef::new(id, 1)).unwrap();
    project
        .station_comments
        .insert(middle, "restriction".to_string());
    let start = project.key_for_ref(StationRef::new(id, 0)).unwrap();
    project.station_names.remove(&start);

    let stations = project.compute_station_coordinates();
    let station = |index: u32| {
        stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(id, index))
            .unwrap()
    };
    assert_eq!(station(0).name, None);
    assert_eq!(station(1).name.as_deref(), Some("1"));
    assert_eq!(station(1).comment.as_deref(), Some("restriction"));
    assert_eq!(station(2).name.as_deref(), Some("2"));
    assert_eq!(station(2).comment, None);

    // Off the wire when absent, so unnamed stations cost nothing per refresh.
    let wire = serde_json::to_value(station(0)).unwrap();
    assert!(!wire.as_object().unwrap().contains_key("name"), "{wire}");
}
