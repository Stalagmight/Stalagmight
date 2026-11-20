//! The complete example in `docs/file-format.md` is exactly what the encoder
//! writes for the survey built here, and it decodes back to that survey.

use std::sync::Arc;

use chrono::{TimeZone, Utc};

use crate::models::survey::{
    ClosureStrategy, LineMeta, Placement, Project, StartAt, StationAlias, ZeroDatum,
};
use crate::models::{
    AzimuthReference, CorrectionSource, GpsCoordinates, NorthReference, Person, Section, ShotData,
    ShotKind, SurveyCredit, SurveyDirection, UndoMode,
};

use super::types::{History, IgnoredWarning, LocalSync, RawJson, SpeleoDbLink};
use super::{decode, encode, DecodeOptions, Extras, Mode};

const DOC: &str = include_str!("../../../../../../docs/file-format.md");

/// Two lines meeting at a junction (station 2), a datum, a second free piece
/// with a placement, a closure, a Section, credits, an alias.
pub(super) fn example() -> (Project, Extras) {
    let at = |s: i64, n: u32| Utc.timestamp_opt(s, n).single().unwrap();
    let mut p = Project::new_at(
        "Cenote Example",
        Some("Synthetic, nowhere".to_string()),
        at(1_763_640_000, 123_456_789),
    );
    p.modified_at = at(1_763_643_600, 0);
    let section = p.mint_section_id().unwrap();
    p.sections.push(Section {
        id: section,
        name: "Upstream".to_string(),
        color: "#4ecdc4".to_string(),
    });
    let team = Arc::new(SurveyCredit {
        surveyors: vec![Person {
            name: "A. Diver".to_string(),
            email: None,
        }],
        explorers: Vec::new(),
        date: chrono::NaiveDate::from_ymd_opt(2026, 3, 14),
    });
    let shot = |uuid: &str, length: f64, az: f64, d_in: f64, d_out: f64| ShotData {
        uuid: Some(uuid.to_string()),
        length,
        azimuth_in: az,
        azimuth_out: az,
        depth_in: d_in,
        depth_out: d_out,
        credit: Some(Arc::clone(&team)),
        ..ShotData::default()
    };
    let main = p
        .add_line(
            LineMeta {
                name: Some("Main".to_string()),
                color: "#ff6b6b".to_string(),
                section_id: Some(section),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![
                ShotData {
                    left: Some(1.5),
                    right: Some(2.0),
                    up: None,
                    down: Some(0.5),
                    ..shot("5f0c2a1e-0000-4000-8000-000000000001", 12.5, 90.0, 3.0, 4.5)
                },
                shot("5f0c2a1e-0000-4000-8000-000000000002", 8.25, 92.5, 4.5, 6.0),
                ShotData {
                    comment: Some("restriction, \"silt\"".to_string()),
                    kind: ShotKind::Jump,
                    credit: None,
                    ..shot(
                        "5f0c2a1e-0000-4000-8000-000000000003",
                        4.0,
                        180.0,
                        6.0,
                        -0.0,
                    )
                },
            ],
        )
        .unwrap();
    let junction = p.line_stations(main).unwrap()[2];
    let side = p
        .add_line(
            LineMeta {
                name: Some("Side".to_string()),
                color: "#95e1d3".to_string(),
                direction: SurveyDirection::Out,
                azimuth_reference: Some(AzimuthReference {
                    north: NorthReference::Magnetic,
                    correction: -1.5,
                    source: CorrectionSource::IgrfAuto { epoch_year: 2026 },
                }),
                ..LineMeta::default()
            },
            StartAt::Station(junction),
            vec![ShotData {
                pitch_in: Some(-10.0),
                temperature: Some(24.5),
                ..shot("5f0c2a1e-0000-4000-8000-000000000004", 6.0, 0.0, 6.0, 7.0)
            }],
        )
        .unwrap();
    let free = p
        .add_line(
            LineMeta {
                name: Some("Unconnected".to_string()),
                color: "#abc".to_string(),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![shot(
                "5f0c2a1e-0000-4000-8000-000000000005",
                3.0,
                45.0,
                2.0,
                2.0,
            )],
        )
        .unwrap();
    let names = ["A0", "A1", "A2", "A3", "B1", "C0", "C1"];
    let ids: Vec<_> = p.stations.keys().copied().collect();
    for (id, name) in ids.iter().zip(names) {
        p.station_mut(*id).unwrap().name = name.to_string();
    }
    let alias = p.mint_station_id().unwrap();
    {
        let s = p.station_mut(junction).unwrap();
        s.comment = Some("tie-off on a rock".to_string());
        s.aliases.push(StationAlias {
            id: alias,
            name: Some("B0".to_string()),
        });
    }
    let entrance = p.line(main).unwrap().start;
    p.push_datum(ZeroDatum {
        station: entrance,
        via: Some(main),
        gps: GpsCoordinates {
            latitude: 20.5,
            longitude: -87.25,
            depth_reference: 0.0,
        },
        accuracy_m: Some(3.0),
        entrance: Some(true),
        correction: Some([0.25, -0.125]),
    });
    let free_start = p.line(free).unwrap().start;
    p.placements.push(Placement {
        station: free_start,
        x: 40.0,
        y: -12.5,
    });
    let side_end = p.line_stations(side).unwrap()[1];
    let main_end = p.line_stations(main).unwrap()[3];
    p.push_closure(side_end, Some(side), main_end, None, ClosureStrategy::Batch);
    p.imported_file_hashes.push("9f86d081884c7d65".to_string());
    assert!(p.validate().is_empty(), "{:?}", p.validate());

    let extras = Extras {
        generator: "Stalagmight 0.6.0".to_string(),
        speleodb: Some(SpeleoDbLink {
            endpoint: "https://speleodb.example".to_string(),
            project_id: "0f6c1d2e-3b4a-4c5d-8e9f-a0b1c2d3e4f5".to_string(),
            project_name: "Cenote Example".to_string(),
            undo_mode: Some(UndoMode::Full),
        }),
        local: super::LocalState {
            display: [("grid".to_string(), "true".to_string())].into_iter().collect(),
            speleodb: LocalSync {
                base_commit: Some("3a7bd3e2360a3d29eea436fcfb7e44c735d117c4".to_string()),
                unpushed_edits: true,
                last_edit_at: Some(at(1_763_643_600, 0)),
                ..LocalSync::default()
            },
        },
        ignored_warnings: vec![IgnoredWarning {
            shot: "5f0c2a1e-0000-4000-8000-000000000003".to_string(),
            code: "SHORT_SHOT".to_string(),
            at: at(1_763_643_000, 0),
            comment: Some("checked on the dive".to_string()),
        }],
        history: History {
            undo: vec![RawJson::parse(
                r#"{"t":"2025-11-20T12:59:00.000000000Z","k":"update_shot","label":"Edit shot A2-A3","ops":[]}"#,
            )
            .unwrap()],
            redo: Vec::new(),
        },
        ..Extras::default()
    };
    (p, extras)
}

#[test]
fn the_documented_example_is_what_the_encoder_writes() {
    let (p, extras) = example();
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let begin = DOC.find("<!-- example:begin -->").unwrap();
    let block = &DOC[begin..];
    let start = block.find("```json\n").unwrap() + "```json\n".len();
    let end = block[start..].find("```").unwrap();
    let documented = &block[start..start + end];
    if documented != text {
        std::fs::write(std::env::temp_dir().join("stalx-v2-example.json"), text).unwrap();
    }
    assert_eq!(
        documented, text,
        "docs/file-format.md's example is stale (the encoder's output is in the temp dir)"
    );
    let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
    let mut expected = p;
    expected.assign_session_shot_ids();
    assert!(decoded.project == expected);
    assert_eq!(decoded.extras, extras);
}
