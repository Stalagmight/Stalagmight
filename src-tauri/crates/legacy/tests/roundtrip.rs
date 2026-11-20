//! (a) Round trip through the 0.6.0 writer and the frozen open.
#![expect(clippy::unwrap_used, reason = "test setup and assertions")]

use std::collections::BTreeMap;

use stalagmight_legacy::models as m;
use stalagmight_legacy::models::config::ClosureAdjustmentConfig;
use stalagmight_legacy::models::{ClosureStrategy, ComponentHold, SpeleoLink, UndoMode};
use stalagmight_legacy::persistence::ResolvedWarning;
use stalagmight_legacy::{
    open_bytes, positions_of, write_v18, write_v18_with, FileShape, LegacyExtras, LegacySurvey,
    OpenOptions, Positions, Slot, WriteOptions, SUPPORTED_SCHEMA_VERSION,
};

#[path = "support/builder.rs"]
mod builder;

/// Positions as bit patterns, so equality is bit equality (and NaN-safe).
fn bits(positions: &Positions) -> Vec<(Slot, [u64; 3])> {
    positions
        .iter()
        .map(|(slot, p)| (*slot, p.map(f64::to_bits)))
        .collect()
}

fn extras(project: &m::Project) -> LegacyExtras {
    let shot = &project.lines[0].shots[1];
    LegacyExtras {
        display_settings: BTreeMap::from([
            ("map_view".to_string(), "{\"zoom\":3}".to_string()),
            ("show_labels".to_string(), "true".to_string()),
        ]),
        resolved_warnings: vec![ResolvedWarning {
            line_id: project.lines[0].id,
            shot_id: shot.id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            resolved_comment: Some("checked".to_string()),
            shot_uuid: shot.uuid.clone(),
        }],
        speleo_link: Some(SpeleoLink {
            endpoint: "https://example.invalid".to_string(),
            project_id: "00000000-0000-4000-8000-000000000001".to_string(),
            project_name: "Synthetic".to_string(),
            base_commit_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            has_unpushed_edits: true,
            last_edit_at: Some("2026-01-02T03:04:05Z".to_string()),
            last_verify_at: None,
            undo_mode: Some(UndoMode::Disabled),
        }),
        speleo_undo_chain: Some("89abcdef0123456789abcdef0123456789abcdef".to_string()),
        speleo_undo_watermark: Some("2026-01-01T00:00:00Z".to_string()),
    }
}

fn json(survey: &LegacySurvey) -> String {
    serde_json::to_string(&survey.project).unwrap()
}

fn assert_same(a: &LegacySurvey, b: &LegacySurvey) {
    assert_eq!(bits(&a.raw_positions), bits(&b.raw_positions));
    assert_eq!(
        bits(a.adjusted_positions.as_ref().unwrap()),
        bits(b.adjusted_positions.as_ref().unwrap())
    );
    assert_eq!(a.component_anchors, b.component_anchors);
    assert_eq!(a.adjustment_report, b.adjustment_report);
    assert_eq!(a.extras, b.extras);
    assert_eq!(json(a), json(b));
}

#[test]
fn a_written_project_opens_to_its_own_positions_bit_for_bit() {
    let project = builder::kitchen_sink();
    let extras = extras(&project);
    let bytes = write_v18(&project, &extras).unwrap();
    let survey = open_bytes(&bytes, &OpenOptions::default()).unwrap();

    // The reported positions are the frozen model's own, on the settled project.
    let raw = positions_of(&survey.project.compute_station_coordinates());
    assert_eq!(bits(&survey.raw_positions), bits(&raw));
    let adjusted = survey
        .project
        .compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    assert_eq!(
        bits(survey.adjusted_positions.as_ref().unwrap()),
        bits(&positions_of(&adjusted.stations))
    );
    assert!(survey.adjustment_failures.is_empty());
    let stations: usize = project.lines.iter().map(|l| l.shots.len() + 1).sum();
    assert_eq!(survey.raw_positions.len(), stations);

    // Everything written came back.
    assert_eq!(survey.schema_version, SUPPORTED_SCHEMA_VERSION);
    assert_eq!(survey.project.lines.len(), project.lines.len());
    assert_eq!(
        survey.project.station_merges.len(),
        project.station_merges.len()
    );
    let strategies: Vec<ClosureStrategy> = survey
        .project
        .loop_closures
        .iter()
        .map(|c| c.strategy)
        .collect();
    assert_eq!(strategies, [ClosureStrategy::Batch, ClosureStrategy::Move]);
    assert!(
        survey.project.loop_closures[1].is_aligned,
        "the Move tie is aligned"
    );
    assert_eq!(survey.project.zero_datums.len(), 3);
    assert_eq!(survey.project.station_offsets.len(), 1);
    assert_eq!(survey.project.sections.len(), 1);
    assert!(survey.project.station_names.values().any(|n| n == "A3"));
    assert!(survey
        .project
        .station_comments
        .values()
        .any(|c| c == "squeeze"));
    assert_eq!(survey.extras, extras);
    assert!(!survey.history.any());
    assert!(survey.load.validation_stored);
    assert!(survey.load.derived_shot_uuids.is_empty());
    assert!(
        survey.load.healed_offsets > 0,
        "the drifted offset is re-solved"
    );

    // Main's piece and Far are placed by datums; the two free pieces by gauges.
    let holds: Vec<ComponentHold> = survey.component_anchors.iter().map(|a| a.hold).collect();
    assert_eq!(
        holds,
        [
            ComponentHold::PlacementDatum,
            ComponentHold::PlacementDatum,
            ComponentHold::DefaultGauge,
            ComponentHold::DefaultGauge,
        ]
    );
    assert_eq!(survey.datumless_gauges().count(), 2);
    // The free pieces keep their stored offsets.
    let loose = survey.project.lines[5].id;
    assert_eq!(
        survey.raw_positions[&Slot {
            line_id: loose,
            station_index: 0
        }][..2],
        [123.25, -45.5]
    );

    // A second open of the same bytes is identical.
    let again = open_bytes(&bytes, &OpenOptions::default()).unwrap();
    assert_same(&survey, &again);
}

#[test]
fn every_builder_round_trips_deterministically() {
    for (name, project) in builder::all() {
        let bytes = write_v18(&project, &LegacyExtras::default()).unwrap();
        let first = open_bytes(&bytes, &OpenOptions::default()).unwrap();
        let second = open_bytes(&bytes, &OpenOptions::default()).unwrap();
        assert_same(&first, &second);
        assert_eq!(
            bits(&first.raw_positions),
            bits(&positions_of(&first.project.compute_station_coordinates())),
            "{name}"
        );
        // Re-saving the settled project and opening it again changes nothing.
        let resaved = write_v18(&first.project, &first.extras).unwrap();
        let third = open_bytes(&resaved, &OpenOptions::default()).unwrap();
        assert_eq!(
            bits(&first.raw_positions),
            bits(&third.raw_positions),
            "{name}"
        );
        assert_eq!(
            bits(first.adjusted_positions.as_ref().unwrap()),
            bits(third.adjusted_positions.as_ref().unwrap()),
            "{name}"
        );
    }
}

#[test]
fn the_release_055_shape_opens_to_the_same_geometry() {
    let project = builder::kitchen_sink();
    let current = write_v18(&project, &LegacyExtras::default()).unwrap();
    let old = write_v18_with(
        &project,
        &LegacyExtras::default(),
        &WriteOptions {
            shape: FileShape::Release055,
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let tables = |bytes: &[u8]| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.stalx");
        std::fs::write(&path, bytes).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        names
    };
    assert!(tables(&current).contains(&"station_ids".to_string()));
    assert!(!tables(&old).contains(&"station_ids".to_string()));
    assert!(!tables(&old).contains(&"cave_sections".to_string()));

    let a = open_bytes(&current, &OpenOptions::default()).unwrap();
    let b = open_bytes(&old, &OpenOptions::default()).unwrap();
    assert_eq!(bits(&a.raw_positions), bits(&b.raw_positions));
    assert_eq!(
        bits(a.adjusted_positions.as_ref().unwrap()),
        bits(b.adjusted_positions.as_ref().unwrap())
    );
    assert!(b.project.sections.is_empty());
    // Station IDs are re-minted in line order, so they come back the same.
    assert_eq!(a.project.station_ids, b.project.station_ids);
}

#[test]
fn history_is_counted_not_loaded() {
    let project = builder::datumless();
    let row = (
        "{}".to_string(),
        "{}".to_string(),
        Some("2026-01-01T00:00:00Z".to_string()),
    );
    let bytes = write_v18_with(
        &project,
        &LegacyExtras::default(),
        &WriteOptions {
            undo_rows: vec![row.clone(), row.clone()],
            redo_rows: vec![row],
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let survey = open_bytes(&bytes, &OpenOptions::default()).unwrap();
    assert_eq!(survey.history.undo_rows, 2);
    assert_eq!(survey.history.redo_rows, 1);
    assert!(survey.history.any());
    // One component, no datum: held by its default gauge.
    assert_eq!(survey.component_anchors.len(), 1);
    assert_eq!(
        survey.component_anchors[0].hold,
        ComponentHold::DefaultGauge
    );
}

#[test]
fn the_load_heals_are_reported() {
    let open = |project: &m::Project| {
        open_bytes(
            &write_v18(project, &LegacyExtras::default()).unwrap(),
            &OpenOptions::default(),
        )
        .unwrap()
    };
    // A merge torn between two datum-placed pieces is demoted to a closure.
    let torn = builder::torn_datum_merge();
    let survey = open(&torn);
    assert_eq!(
        survey.project.station_merges.len() + 1,
        torn.station_merges.len()
    );
    assert_eq!(
        survey.project.loop_closures.len(),
        torn.loop_closures.len() + 1
    );
    // A split left a closure as the sole bridge: the load promotes it to a
    // merge, dated by the file rather than the clock.
    let edited = open(&builder::edited());
    assert_eq!(edited.load.merges_created, 1);
    assert!(edited
        .project
        .station_merges
        .iter()
        .any(|merge| merge.merged_at == edited.project.modified_at));
    // A held gauge holds its piece.
    let held = open(&builder::held_gauge());
    let holds: Vec<ComponentHold> = held.component_anchors.iter().map(|a| a.hold).collect();
    assert_eq!(holds, [ComponentHold::HeldGauge]);
}
