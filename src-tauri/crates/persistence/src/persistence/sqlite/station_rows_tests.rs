//! [#576 RV13-6] A save writes one station-offset row and one station-comment
//! row per station, whatever keys name it.
//!
//! Both tables have the primary key `(section_id, station_index)`. Two keys
//! naming one station - a legacy `Index` anchor beside the canonical key, the
//! state the re-anchor on open settles (#576 NF-W13BE-2) - each wrote a row at
//! that key, and the whole save failed on the primary key. The save now settles
//! them as the re-anchor does: the canonical key's offset, both comments.

use tempfile::TempDir;

use super::Database;
use crate::models::{
    Project, SectionData, SectionId, ShotData, StationAnchor, StationKey, StationRef,
};

/// One section of three shots, the shot UUIDs minted.
fn project() -> (Project, SectionId) {
    let mut project = Project::new("CaveA".to_string(), None);
    let id = project
        .add_section(SectionData {
            name: Some("SectionA".to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: None,
            quality: 1.0,
            shots: (0..3)
                .map(|i| ShotData {
                    length: 2.0 + f64::from(i),
                    azimuth_in: f64::from(i * 30),
                    azimuth_out: f64::from(i * 30),
                    ..Default::default()
                })
                .collect(),
        })
        .id;
    project.ensure_shot_uuids();
    (project, id)
}

/// Station 2 under its canonical key (`AfterShot`) and under a legacy `Index`
/// key, and station 0 under `SectionStart` and `Index(0)`; each key with a
/// comment and an offset of its own. `canonical_first` inserts in either order,
/// so the outcome cannot ride on the order the map was filled in.
fn with_two_keys_per_station(canonical_first: bool) -> (Project, SectionId) {
    let (mut project, id) = project();
    let mut entries = Vec::new();
    for (station, legacy) in [(2_u32, 2_u32), (0, 0)] {
        let canonical = project.key_for_ref(StationRef::new(id, station)).unwrap();
        let legacy = StationKey::new(id, StationAnchor::Index(legacy));
        let mut pair = vec![
            (canonical, format!("StationA{station} canonical"), 1.0),
            (legacy, format!("StationA{station} legacy"), 7.0),
        ];
        if !canonical_first {
            pair.reverse();
        }
        entries.extend(pair);
    }
    for (key, comment, offset) in entries {
        project.station_comments.insert(key.clone(), comment);
        project
            .station_offsets
            .insert(key, (offset, offset + 1.0, offset + 2.0));
    }
    (project, id)
}

/// The comment and offset a project holds for each of `stations`, by the
/// station's canonical key.
fn held(project: &Project, id: SectionId, stations: &[u32]) -> Vec<String> {
    stations
        .iter()
        .map(|&station| {
            let key = project.key_for_ref(StationRef::new(id, station)).unwrap();
            format!(
                "station {station}: comment {:?}, offset {:?}",
                project.station_comments.get(&key),
                project.station_offsets.get(&key)
            )
        })
        .collect()
}

#[test]
fn two_keys_naming_one_station_save_as_the_one_row_the_reanchor_keeps() {
    for canonical_first in [true, false] {
        let (project, id) = with_two_keys_per_station(canonical_first);
        // What an open of the project (its re-anchor) holds for these stations.
        let mut reanchored = project.clone();
        reanchored.reanchor_station_refs();
        let expected = held(&reanchored, id, &[0, 2]);

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CaveA.stalx");
        let saved = Database::open(&path).unwrap().save_project(&project);
        println!("RV13-6 save with two keys on stations 0 and 2: {saved:?}");
        assert!(saved.is_ok(), "the save failed: {saved:?}");

        let mut loaded = Database::open_existing(&path)
            .unwrap()
            .load_project()
            .unwrap();
        // The rows come back under positional keys; an open re-anchors them
        // (`redrive_offsets_after_load`).
        loaded.reanchor_station_refs();
        let got = held(&loaded, id, &[0, 2]);
        for line in &got {
            println!("RV13-6 saved and opened again: {line}");
        }
        assert_eq!(got, expected, "the save keeps what the re-anchor keeps");
        assert_eq!(loaded.station_comments.len(), 2, "one comment per station");
        assert_eq!(loaded.station_offsets.len(), 2, "one offset per station");
        let comment = |station: u32| {
            let key = loaded.key_for_ref(StationRef::new(id, station)).unwrap();
            loaded.station_comments.get(&key).cloned()
        };
        assert_eq!(
            comment(2).as_deref(),
            Some("StationA2 canonical\nStationA2 legacy"),
            "both comments, the canonical key's first"
        );
        let key = loaded.key_for_ref(StationRef::new(id, 2)).unwrap();
        assert_eq!(
            loaded.station_offsets.get(&key),
            Some(&(1.0, 2.0, 3.0)),
            "the canonical key's offset"
        );
    }
}
