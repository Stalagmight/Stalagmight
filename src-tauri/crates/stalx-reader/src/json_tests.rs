//! Pins the JSON shape behind the `serde` feature. Consumers outside Rust build
//! their models from these exact keys and spellings, so a change here is a
//! breaking change for them, not a test to update in passing.

#![expect(clippy::unwrap_used, reason = "test assertions")]

use serde_json::json;

use super::{
    CorrectionSource, Direction, LoopClosure, North, Section, Shot, ShotKind, StationComment,
    StationId, StationMerge, StationPosition, Survey, ZeroDatum,
};

const fn station(section_id: u32, index: u32) -> StationId {
    StationId { section_id, index }
}

fn shot(kind: ShotKind) -> Shot {
    Shot {
        uuid: "0b6c1f1e-6d1c-4a4e-9a51-2f1d5c3e7a90".to_string(),
        uuid_derived: false,
        kind,
        length: 5.0,
        azimuth_in: 90.0,
        azimuth_out: 270.0,
        depth_in: 10.0,
        depth_out: 12.5,
        pitch_in: Some(30.0),
        pitch_out: None,
        left: Some(1.5),
        right: None,
        up: None,
        down: None,
        temperature: None,
        marker_index: Some(3),
        comment: Some("sump".to_string()),
    }
}

fn survey() -> Survey {
    Survey {
        schema_version: 18,
        name: "Synthetic".to_string(),
        location: None,
        created_at: "2026-01-02T03:04:05Z".to_string(),
        modified_at: "2026-01-02T03:04:06Z".to_string(),
        sections: vec![Section {
            id: 1,
            name: Some("Main".to_string()),
            surveyor: None,
            date: Some("2026-01-02".to_string()),
            direction: Direction::In,
            quality: 1.0,
            color: "#ff5733".to_string(),
            north: North::Magnetic,
            azimuth_correction_deg: -2.5,
            azimuth_correction_source: CorrectionSource::Model { epoch_year: 2025 },
            shots: vec![shot(ShotKind::Jump)],
        }],
        station_merges: vec![StationMerge {
            a: station(1, 1),
            b: station(2, 0),
        }],
        zero_datums: vec![ZeroDatum {
            station: station(1, 0),
            latitude: 20.5,
            longitude: -87.4,
            depth: 0.0,
            residual_m: None,
        }],
        loop_closures: vec![LoopClosure {
            id: 7,
            a: station(1, 1),
            b: station(2, 2),
        }],
        station_comments: vec![StationComment {
            station: station(1, 1),
            text: "lead".to_string(),
        }],
        frame_datum: Some(station(1, 0)),
        stations: Some(vec![StationPosition {
            station: station(1, 0),
            x: 0.0,
            y: 0.0,
            z: 0.0,
            anchor: Some(station(1, 0)),
            latitude: Some(20.5),
            longitude: None,
        }]),
        unresolved_references: 0,
    }
}

#[test]
fn a_survey_serializes_to_the_documented_json() {
    let want = json!({
        "schema_version": 18,
        "name": "Synthetic",
        "location": null,
        "created_at": "2026-01-02T03:04:05Z",
        "modified_at": "2026-01-02T03:04:06Z",
        "sections": [{
            "id": 1,
            "name": "Main",
            "surveyor": null,
            "date": "2026-01-02",
            "direction": "in",
            "quality": 1.0,
            "color": "#ff5733",
            "north": "magnetic",
            "azimuth_correction_deg": -2.5,
            "azimuth_correction_source": { "kind": "model", "epoch_year": 2025 },
            "shots": [{
                "uuid": "0b6c1f1e-6d1c-4a4e-9a51-2f1d5c3e7a90",
                "uuid_derived": false,
                "kind": "jump",
                "length": 5.0,
                "azimuth_in": 90.0,
                "azimuth_out": 270.0,
                "depth_in": 10.0,
                "depth_out": 12.5,
                "pitch_in": 30.0,
                "pitch_out": null,
                "left": 1.5,
                "right": null,
                "up": null,
                "down": null,
                "temperature": null,
                "marker_index": 3,
                "comment": "sump"
            }]
        }],
        "station_merges": [{
            "a": { "section_id": 1, "index": 1 },
            "b": { "section_id": 2, "index": 0 }
        }],
        "zero_datums": [{
            "station": { "section_id": 1, "index": 0 },
            "latitude": 20.5,
            "longitude": -87.4,
            "depth": 0.0,
            "residual_m": null
        }],
        "loop_closures": [{
            "id": 7,
            "a": { "section_id": 1, "index": 1 },
            "b": { "section_id": 2, "index": 2 }
        }],
        "station_comments": [{
            "station": { "section_id": 1, "index": 1 },
            "text": "lead"
        }],
        "frame_datum": { "section_id": 1, "index": 0 },
        "stations": [{
            "station": { "section_id": 1, "index": 0 },
            "x": 0.0,
            "y": 0.0,
            "z": 0.0,
            "anchor": { "section_id": 1, "index": 0 },
            "latitude": 20.5,
            "longitude": null
        }],
        "unresolved_references": 0
    });
    assert_eq!(serde_json::to_value(survey()).unwrap(), want);
}

/// The documented spelling of every variant. Each `match` names every variant
/// with no wildcard arm, so adding a variant fails to compile here until its
/// spelling is pinned (and the variant joins the lists in the test below).
fn shot_kind_spelling(kind: ShotKind) -> serde_json::Value {
    match kind {
        ShotKind::Normal => json!("normal"),
        ShotKind::Jump => json!("jump"),
        ShotKind::Reference => json!("reference"),
    }
}

fn direction_spelling(direction: Direction) -> serde_json::Value {
    match direction {
        Direction::In => json!("in"),
        Direction::Out => json!("out"),
    }
}

fn north_spelling(north: North) -> serde_json::Value {
    match north {
        North::Magnetic => json!("magnetic"),
        North::True => json!("true"),
        North::Grid => json!("grid"),
    }
}

fn correction_source_spelling(source: CorrectionSource) -> serde_json::Value {
    match source {
        CorrectionSource::NotRecorded => json!({ "kind": "not_recorded" }),
        CorrectionSource::Unknown => json!({ "kind": "unknown" }),
        CorrectionSource::FileDeclared => json!({ "kind": "file_declared" }),
        CorrectionSource::Model { epoch_year } => {
            json!({ "kind": "model", "epoch_year": epoch_year })
        }
        CorrectionSource::Manual => json!({ "kind": "manual" }),
    }
}

#[test]
fn every_enum_variant_has_its_documented_spelling() {
    for kind in [ShotKind::Normal, ShotKind::Jump, ShotKind::Reference] {
        assert_eq!(json!(kind), shot_kind_spelling(kind));
    }
    for direction in [Direction::In, Direction::Out] {
        assert_eq!(json!(direction), direction_spelling(direction));
    }
    for north in [North::Magnetic, North::True, North::Grid] {
        assert_eq!(json!(north), north_spelling(north));
    }
    for source in [
        CorrectionSource::NotRecorded,
        CorrectionSource::Unknown,
        CorrectionSource::FileDeclared,
        CorrectionSource::Model { epoch_year: 2025 },
        CorrectionSource::Manual,
    ] {
        assert_eq!(json!(source), correction_source_spelling(source));
    }
}

#[test]
fn positions_left_out_serialize_as_null_not_a_missing_key() {
    let mut survey = survey();
    survey.stations = None;
    let value = serde_json::to_value(survey).unwrap();
    assert_eq!(value.get("stations"), Some(&serde_json::Value::Null));
}
