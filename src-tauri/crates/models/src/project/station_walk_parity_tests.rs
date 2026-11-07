//! The station walk, pinned to ONE table shared with the frontend (#576 D15).
//!
//! The import and placement previews cannot ask the backend where a section's
//! stations will land - the section is not in the project yet, and they redraw
//! on every edit - so they walk the shots themselves, in
//! `src/utils/stationWalk.ts`. That walker is only worth anything if it lands
//! where [`Project::compute_station_coordinates`] lands, so both sides assert
//! the same vectors: this module builds each one as a real `Project` and runs
//! the real public walkers over it, and `src/utils/__tests__/stationWalk.test.ts`
//! runs `walkStations` over the very same file.
//!
//! Change a number in the table and one side fails; change the Rust walk and
//! this side fails until the table, and with it the TS mirror, follows.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: a failed unwrap is the failure report"
)]

use serde_json::Value;

use crate::test_fixtures::bare_shot;
use crate::{AzimuthReference, Project, SectionData, SectionId, ShotData, StationRef};

/// The shared table. It lives with the TS tests because that is where the
/// frontend's own test tooling can import it; this crate only reads it.
const VECTORS: &str = include_str!("../../../../../src/utils/__tests__/stationWalkVectors.json");

/// A reference as every ingress builds it: deserialized, then sanitised, so a
/// `true` row carrying a stray correction behaves as the persisted one would.
fn reference(value: &Value) -> Option<AzimuthReference> {
    serde_json::from_value::<Option<AzimuthReference>>(value.clone())
        .expect("reference must deserialize")
        .map(AzimuthReference::sanitised)
}

fn number(shot: &Value, key: &str) -> f64 {
    shot[key]
        .as_f64()
        .expect("every vector shot field is a number")
}

fn shot_data(shot: &Value) -> ShotData {
    let azimuth_in = number(shot, "azimuth_in");
    // The model has no "missing" reading: a lone leg is committed on both legs,
    // and that is the walk the frontend has to reproduce for its own NaN leg.
    let azimuth_out = shot["azimuth_out"].as_f64().unwrap_or(azimuth_in);
    bare_shot(number(shot, "length"))
        .with_azimuths(azimuth_in, azimuth_out)
        .with_depths(number(shot, "depth_in"), number(shot, "depth_out"))
}

fn project_for(vector: &Value) -> (Project, SectionId) {
    let mut project = Project::new("station walk parity".to_string(), None);
    project.default_azimuth_reference = reference(&vector["project_default"]);
    let shots = vector["shots"]
        .as_array()
        .expect("vector has shots")
        .iter()
        .map(shot_data)
        .collect();
    let id = project
        .add_section(SectionData {
            name: Some("CaveA".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots,
        })
        .id;
    project.get_section_mut(id).unwrap().azimuth_reference = reference(&vector["reference"]);
    (project, id)
}

/// Absolute for ordinary metres, relative for the overflow-guard row.
fn close(got: f64, want: f64, tolerance: f64) -> bool {
    (got - want).abs() <= tolerance * want.abs().max(1.0)
}

fn expected_stations(vector: &Value) -> Vec<[f64; 3]> {
    vector["stations"]
        .as_array()
        .expect("vector has stations")
        .iter()
        .map(|s| {
            let s = s.as_array().expect("station is [x, y, z]");
            [0, 1, 2].map(|i| s[i].as_f64().expect("coordinate is a number"))
        })
        .collect()
}

#[test]
fn compute_station_coordinates_walks_the_shared_vectors() {
    let table: Value = serde_json::from_str(VECTORS).expect("vector table must parse");
    let tolerance = table["tolerance"].as_f64().expect("table has a tolerance");
    let vectors = table["vectors"].as_array().expect("table has vectors");
    assert!(vectors.len() >= 10, "the table lost its rows");

    for vector in vectors {
        let name = vector["name"].as_str().expect("vector has a name");
        let (project, id) = project_for(vector);
        let expected = expected_stations(vector);

        let mut walked: Vec<_> = project
            .compute_station_coordinates()
            .into_iter()
            .filter(|s| s.station_ref.section_id == id)
            .collect();
        walked.sort_by_key(|s| s.station_ref.station_index);
        assert_eq!(walked.len(), expected.len(), "{name}: station count");

        for (index, (station, want)) in walked.iter().zip(&expected).enumerate() {
            let got = [
                station.coordinates.x,
                station.coordinates.y,
                station.coordinates.z,
            ];
            // The single-station walk is a second public entry to the same
            // formula; the previews have to match both.
            let raw = project
                .compute_station_raw_coords(StationRef::new(id, index as u32))
                .expect("station resolves");
            for (axis, (g, w)) in got.iter().zip(want).enumerate() {
                assert!(
                    close(*g, *w, tolerance),
                    "{name}: station {index} axis {axis}: walked {g}, table {w}"
                );
            }
            for (axis, (g, w)) in [raw.x, raw.y, raw.z].iter().zip(want).enumerate() {
                assert!(
                    close(*g, *w, tolerance),
                    "{name}: station {index} axis {axis}: raw walk {g}, table {w}"
                );
            }
        }
    }
}
