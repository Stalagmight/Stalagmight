//! Golden on-disk JSON vectors, captured from the code as it stands BEFORE the
//! Section -> Line identifier rename.
//!
//! A rename of Rust identifiers (`section_id` -> `line_id`, `CreateSection` ->
//! `CreateLine`, ...) silently changes every serde name derived from them, and
//! several persisted shapes embed those names: the undo/redo history rows of a
//! `.stalx` (`command_json`, `undo_data_json`, and the whole-project snapshots
//! some `UndoData` variants carry as JSON strings), the `project_settings` and
//! `display_settings` tables, the dive-plan sidecar (`.plnx`) and the app
//! config. This test builds a project through the real `AppState` command
//! paths, saves it through the real save path, reads the saved file back and
//! writes every one of those shapes, decompressed and parsed, to the directory
//! named by `LEGACY_VECTORS_OUT`. The vectors are then compared after a round
//! trip through the renamed code, never regenerated.
//!
//! Synthetic data only: every survey is built in code, no fixture is read.
//! `#[ignore]`d - it writes files outside the tree, so it runs on demand:
//!
//! ```text
//! LEGACY_VECTORS_OUT=/some/dir cargo test -p stalagmight-core --lib \
//!     legacy_disk_vectors_dump -- --ignored --nocapture
//! ```

use std::path::Path;

use chrono::{NaiveDate, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

use super::{AppState, VirtualShotMode};
use crate::config::Config;
use crate::core::{Command, CommandResult, CommentHolder, UndoData, UndoEntry};
use crate::models::config::GasConfig;
use crate::models::{
    AzimuthReference, ClosureStrategy, CombinePick, CombineLinesOptions, Coordinates,
    CorrectionSource, CreditEdit, CreditTargets, DivePlanSnapshot, Diver, ExistingShotUpdate,
    GpsCoordinates, ImportClosure, ImportPlacement, ImportLineConfig, LoopClosure, MarkedPoint,
    Person, Project, LineData, LineId, LineUpdates, ShotData, ShotId, ShotKind,
    ShotUpdates, SplitAt, SplitLineOptions, SplitSide, StationAnchor, StationKey, StationMerge,
    StationRef, SurveyCredit, SurveyDirection,
};
use crate::persistence::Database;

/// The display toggles the frontend persists per project (`DisplayToggleKey`
/// minus the session-only debug overlays), plus the two string-valued keys.
const DISPLAY_SETTINGS: &[(&str, &str)] = &[
    ("showStations", "true"),
    ("showShots", "true"),
    ("showSectionNames", "true"),
    ("showStationLabels", "false"),
    ("showStationNames", "true"),
    ("showShotData", "false"),
    ("showShotErrors", "true"),
    ("showReferenceShots", "true"),
    ("showJumps", "true"),
    ("showZeroDatum", "true"),
    ("showGrid", "false"),
    ("showSatellite", "false"),
    ("showDepthGradient", "true"),
    ("showStationComments", "true"),
    ("showShotComments", "false"),
    ("gridSpacing", "25"),
    ("geometryMode", "raw"),
];

fn person(name: &str) -> Person {
    Person::named(name).expect("a non-empty name is a person")
}

fn day(d: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(2025, 3, d)
}

/// A shot with every optional field populated, so the snapshots carry each
/// per-shot key, credit included.
fn shot(azimuth: f64, length: f64, depth: f64, surveyor: &str) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: (azimuth + 180.0) % 360.0,
        length,
        depth_in: depth,
        depth_out: depth + 0.5,
        pitch_in: Some(-2.0),
        pitch_out: Some(2.0),
        left: Some(1.5),
        right: Some(2.5),
        up: Some(0.5),
        down: Some(1.0),
        comment: Some(format!("shot {azimuth}/{length}")),
        temperature: Some(24.5),
        credit: Some(std::sync::Arc::new(SurveyCredit {
            surveyors: vec![person(surveyor)],
            explorers: vec![person("Explorer One")],
            date: day(1),
        })),
        ..Default::default()
    }
}

fn shots(n: usize, azimuth: f64, length: f64, surveyor: &str) -> Vec<ShotData> {
    (0..n)
        .map(|i| shot(azimuth, length, 5.0 + i as f64, surveyor))
        .collect()
}

fn project(state: &AppState) -> Project {
    state.get_project_state().expect("a project is open")
}

fn shot_ids(state: &AppState, line_id: LineId) -> Vec<ShotId> {
    project(state)
        .get_line(line_id)
        .expect("line exists")
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

fn shot_id(state: &AppState, line_id: LineId, index: usize) -> ShotId {
    *shot_ids(state, line_id)
        .get(index)
        .expect("shot index in range")
}

/// The station at the far end of a line.
fn end(state: &AppState, line_id: LineId) -> StationRef {
    StationRef::new(line_id, shot_ids(state, line_id).len() as u32)
}

fn line_ids(state: &AppState) -> Vec<LineId> {
    project(state).lines.iter().map(|s| s.id).collect()
}

fn undo_len(state: &AppState) -> usize {
    state.undo_stack.lock().expect("undo stack lock").len()
}

/// Every undoable step, in order, so the dump can name each history row.
struct Steps<'a> {
    state: &'a AppState,
    labels: Vec<String>,
}

impl Steps<'_> {
    /// Run one user action and require that it succeeded and pushed exactly
    /// one undo entry.
    fn run(
        &mut self,
        label: &str,
        action: impl FnOnce(&AppState) -> CommandResult,
    ) -> CommandResult {
        let before = undo_len(self.state);
        let result = action(self.state);
        assert!(result.success, "{label}: {:?}", result.error);
        assert_eq!(
            undo_len(self.state),
            before + 1,
            "{label} pushes exactly one undo entry"
        );
        self.labels.push(label.to_string());
        result
    }

    fn exec(&mut self, label: &str, cmd: Command) -> CommandResult {
        self.run(label, |state| state.execute(cmd))
    }

    fn create(&mut self, name: &str, shots: Vec<ShotData>) -> LineId {
        let result = self.exec(
            &format!("CreateLine {name}"),
            Command::CreateLine {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#336699".to_string(),
                direction: Some(SurveyDirection::In),
                shots,
            },
        );
        result.line_id.expect("CreateLine returns the new id")
    }
}

/// A second `.stalx` for "Add .stalx to current project": two lines, a
/// merge, a comment and a manual offset, so the add records each kind.
fn write_add_source(dir: &Path) -> std::path::PathBuf {
    let mut source = Project::new("Added Source".to_string(), Some("Elsewhere".to_string()));
    let first = source
        .add_line(LineData {
            name: Some("Added One".to_string()),
            color: "#aa5500".to_string(),
            direction: Some(SurveyDirection::Out),
            shots: shots(2, 120.0, 7.0, "Adder"),
            ..Default::default()
        })
        .id;
    let second = source
        .add_line(LineData {
            name: Some("Added Two".to_string()),
            color: "#00aa55".to_string(),
            shots: shots(2, 200.0, 4.0, "Adder"),
            ..Default::default()
        })
        .id;
    source.ensure_shot_uuids();
    source.record_station_merge(StationMerge {
        station_a: StationRef::new(first, 2).into(),
        station_b: StationRef::new(second, 0).into(),
        merged_at: Utc::now(),
    });
    let comment_key = source
        .key_for_ref(StationRef::new(first, 1))
        .expect("station exists");
    source
        .station_comments
        .insert(comment_key, "added comment".to_string());
    source.line_offsets.insert(first, (40.0, -15.0));
    let path = dir.join("added-source.stalx");
    let mut db = Database::open(&path).expect("open add source");
    db.save_project(&source).expect("save add source");
    path
}

/// Undo entries no current command path produces any more, but that a file
/// written by an older build still carries: the vectors must cover their
/// serde shape too. Built from the live project so their payload is real.
fn legacy_entries(state: &AppState) -> Vec<(String, UndoEntry)> {
    let live = project(state);
    let line = live.lines.first().expect("a line exists");
    let sid = line.id;
    let first_shot = line.shots.first().expect("a shot exists");
    let line_data = LineData {
        name: line.name.clone(),
        surveyor: None,
        date: None,
        color: line.color.clone(),
        direction: Some(line.direction),
        quality: line.quality,
        shots: line.shots.iter().cloned().map(ShotData::from).collect(),
    };
    let shot_data = ShotData::from(first_shot.clone());
    let closure = LoopClosure {
        id: 7,
        station_a: StationRef::new(sid, 0).into(),
        station_b: StationRef::new(sid, 1).into(),
        path_length: 12.5,
        misclosure_distance: 0.25,
        error_percentage: 2.0,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.15,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    };
    let snapshot = AppState::serialize_project_snapshot(&live).expect("serialize project");
    let a = StationRef::new(sid, 0);
    let b = StationRef::new(sid, 1);
    let entry = |command: Command, undo_data: UndoData| UndoEntry {
        command,
        undo_data,
        timestamp: Some(Utc::now()),
        command_bytes: None,
    };
    vec![
        (
            "synthetic legacy LineSnapshot".to_string(),
            entry(
                Command::DeleteLine { line_id: sid },
                UndoData::LineSnapshot {
                    line_id: sid,
                    line_data,
                },
            ),
        ),
        (
            "synthetic legacy ShotSnapshot".to_string(),
            entry(
                Command::DeleteShot {
                    line_id: sid,
                    shot_id: first_shot.id,
                },
                UndoData::ShotSnapshot {
                    line_id: sid,
                    shot_id: first_shot.id,
                    shot_data: shot_data.clone(),
                    position: 0,
                    uuid: first_shot.uuid.clone(),
                },
            ),
        ),
        (
            "synthetic legacy MergeInfo".to_string(),
            entry(
                Command::MergeStations {
                    station_a: a,
                    station_b: b,
                },
                UndoData::MergeInfo {
                    station_a: a,
                    station_b: b,
                    removed_closures: vec![closure.clone()],
                    previous_offsets: [(sid, Some((1.0, 2.0)))].into_iter().collect(),
                },
            ),
        ),
        (
            "synthetic legacy UnmergeInfo".to_string(),
            entry(
                Command::UnmergeStationPair {
                    station_a: a,
                    station_b: b,
                },
                UndoData::UnmergeInfo {
                    station_a: a,
                    station_b: b,
                    removed_closures: vec![closure],
                },
            ),
        ),
        (
            "synthetic legacy StationMoveInfo".to_string(),
            entry(
                Command::MoveStation {
                    station_ref: b,
                    offset: Coordinates::new(0.1, 0.2, 0.0),
                },
                UndoData::StationMoveInfo {
                    station_ref: b,
                    prev_shot_before: Some((first_shot.id, shot_data.clone())),
                    prev_shot_after: Some((first_shot.id, shot_data)),
                    prev_offset: Some((0.1, 0.2, 0.3)),
                    prev_line_offset: Some((4.0, 5.0)),
                },
            ),
        ),
        (
            "synthetic legacy ProjectSnapshot".to_string(),
            entry(
                Command::Batch {
                    commands: vec![Command::MoveSegment {
                        line_id: sid,
                        offset: Coordinates::new(1.0, 1.0, 0.0),
                    }],
                },
                UndoData::ProjectSnapshot(snapshot),
            ),
        ),
        (
            "synthetic legacy InverseCommand(SetShotKind)".to_string(),
            entry(
                Command::SetShotKind {
                    line_id: sid,
                    shot_id: first_shot.id,
                    kind: ShotKind::Reference,
                },
                UndoData::InverseCommand(Command::SetShotKind {
                    line_id: sid,
                    shot_id: first_shot.id,
                    kind: ShotKind::Normal,
                }),
            ),
        ),
        (
            "synthetic UndoData::None".to_string(),
            entry(Command::SetSurfaceElevation, UndoData::None),
        ),
    ]
}

/// Rebuild every object with its keys sorted, whatever map type serde_json
/// was compiled with, so the written vectors are byte-stable.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, item)| (key, canonical(item)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

fn parse(text: &str, what: &str) -> Value {
    canonical(
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{what} is not JSON ({e}): {text}")),
    )
}

/// Every string inside `value` that is itself a JSON document (a project
/// snapshot, a `values_json`, ...), parsed, with the path it sits at.
fn collect_embedded(value: &Value, path: &str, out: &mut Vec<Value>) {
    match value {
        Value::String(text) => {
            let trimmed = text.trim_start();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                if let Ok(inner) = serde_json::from_str::<Value>(text) {
                    let inner = canonical(inner);
                    collect_embedded(&inner, &format!("{path}#"), out);
                    out.push(json!({ "path": path, "value": inner }));
                }
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect_embedded(item, &format!("{path}/{i}"), out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                collect_embedded(item, &format!("{path}/{key}"), out);
            }
        }
        _ => {}
    }
}

/// The externally tagged `UndoData` variant name of a parsed row.
fn undo_variant(value: &Value) -> String {
    match value {
        Value::String(name) => name.clone(),
        Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

fn command_type(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn nested_types(value: &Value) -> Vec<String> {
    value
        .get("commands")
        .and_then(Value::as_array)
        .map(|commands| commands.iter().map(command_type).collect())
        .unwrap_or_default()
}

fn write_json(out: &Path, name: &str, value: &Value) {
    let mut text = serde_json::to_string_pretty(value).expect("serialize vector");
    text.push('\n');
    std::fs::write(out.join(name), text).expect("write vector");
}

/// One history table, decompressed through the persistence crate's own loader.
struct StackDump {
    commands: Vec<Value>,
    data: Vec<Value>,
    raw: Vec<Value>,
    embedded: Vec<Value>,
    manifest: Vec<Value>,
}

fn dump_stack(rows: Vec<(i64, String, String, Option<String>)>, labels: &[String]) -> StackDump {
    let mut dump = StackDump {
        commands: Vec::new(),
        data: Vec::new(),
        raw: Vec::new(),
        embedded: Vec::new(),
        manifest: Vec::new(),
    };
    for (row, (sequence, command_json, undo_data_json, executed_at)) in rows.into_iter().enumerate()
    {
        let command = parse(&command_json, "command_json");
        let data = parse(&undo_data_json, "undo_data_json");
        let mut embedded = Vec::new();
        collect_embedded(&command, "command", &mut embedded);
        collect_embedded(&data, "undo_data", &mut embedded);
        dump.manifest.push(json!({
            "row": row,
            "sequence": sequence,
            "label": labels.get(row),
            "command_type": command_type(&command),
            "nested_command_types": nested_types(&command),
            "undo_data_variant": undo_variant(&data),
            "embedded_json_paths": embedded
                .iter()
                .filter_map(|e| e.get("path").cloned())
                .collect::<Vec<_>>(),
        }));
        if !embedded.is_empty() {
            dump.embedded
                .push(json!({ "row": row, "documents": embedded }));
        }
        dump.raw.push(json!({
            "sequence": sequence,
            "executed_at": executed_at,
            "command_json": command_json,
            "undo_data_json": undo_data_json,
        }));
        dump.commands.push(command);
        dump.data.push(data);
    }
    dump
}

fn key_value_table(conn: &Connection, table: &str) -> Value {
    let mut stmt = conn
        .prepare(&format!("SELECT key, value FROM {table} ORDER BY key"))
        .expect("prepare settings query");
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query settings");
    let mut out = Map::new();
    for row in rows {
        let (key, raw) = row.expect("settings row");
        let parsed = serde_json::from_str::<Value>(&raw).ok().map(canonical);
        out.insert(key, json!({ "raw": raw, "parsed": parsed }));
    }
    Value::Object(out)
}

/// Every other TEXT cell of the saved file that holds a JSON object or array,
/// so a serde name persisted somewhere unexpected is not missed.
fn other_json_cells(conn: &Connection) -> Value {
    const COVERED: &[&str] = &[
        "undo_stack",
        "redo_stack",
        "project_settings",
        "display_settings",
    ];
    let tables: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare table list");
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("list tables");
        names.map(|n| n.expect("table name")).collect()
    };
    let mut found = Vec::new();
    for table in tables
        .iter()
        .filter(|t| !COVERED.contains(&t.as_str()) && !t.starts_with("sqlite_"))
    {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .expect("prepare table scan");
        let columns: Vec<String> = stmt.column_names().map(str::to_string).collect();
        let mut rows = stmt.query([]).expect("scan table");
        let mut row_index = 0_usize;
        while let Some(row) = rows.next().expect("table row") {
            for (i, column) in columns.iter().enumerate() {
                if let Ok(Some(text)) = row.get::<_, Option<String>>(i) {
                    let trimmed = text.trim_start();
                    if trimmed.starts_with('{') || trimmed.starts_with('[') {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            found.push(json!({
                                "table": table,
                                "column": column,
                                "row": row_index,
                                "value": canonical(value),
                            }));
                        }
                    }
                }
            }
            row_index += 1;
        }
    }
    Value::Array(found)
}

fn dive_plan_dump(path: &Path) -> Value {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open .plnx read-only");
    let mut stmt = conn
        .prepare("SELECT id, name, schema_version, snapshot_json FROM dive_plans ORDER BY id")
        .expect("prepare plan query");
    let plans: Vec<Value> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .expect("query plans")
        .map(|row| {
            let (id, name, schema_version, raw) = row.expect("plan row");
            json!({
                "id": id,
                "name": name,
                "schema_version": schema_version,
                "snapshot_json": raw,
                "snapshot": parse(&raw, "snapshot_json"),
            })
        })
        .collect();
    let meta = conn
        .query_row(
            "SELECT project_name, app_schema_version FROM plnx_meta WHERE id = 1",
            [],
            |row| {
                Ok(json!({
                    "project_name": row.get::<_, Option<String>>(0)?,
                    "app_schema_version": row.get::<_, Option<i64>>(1)?,
                }))
            },
        )
        .unwrap_or(Value::Null);
    json!({ "plans": plans, "meta": meta })
}

#[test]
#[ignore = "on demand: writes golden vectors to $LEGACY_VECTORS_OUT"]
fn legacy_disk_vectors_dump() {
    let out = std::path::PathBuf::from(
        std::env::var("LEGACY_VECTORS_OUT").expect("set LEGACY_VECTORS_OUT to an output directory"),
    );
    std::fs::create_dir_all(&out).expect("create output directory");
    let temp = TempDir::new().expect("temp dir");
    let stalx = temp.path().join("legacy-vectors.stalx");

    let state = AppState::new().expect("app state");
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Legacy Vectors".to_string(),
                location: Some("Synthetic".to_string()),
            })
            .success
    );
    // Persist (and keep in memory) every entry this test makes, not the
    // default 20-entry window.
    state
        .config
        .lock()
        .expect("config lock")
        .general
        .undo_persist_limit = 10_000;
    state
        .undo_stack
        .lock()
        .expect("undo stack lock")
        .set_history_limit(10_000);
    // Before the first save, so they ride the in-memory cache into the file
    // exactly as the frontend's writes do.
    for (key, value) in DISPLAY_SETTINGS {
        state
            .save_display_setting(key, value)
            .expect("save display setting");
    }
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(stalx.clone()),
            })
            .success,
        "first save"
    );

    let mut steps = Steps {
        state: &state,
        labels: Vec::new(),
    };

    // --- Lines -----------------------------------------------------------
    let a = steps.create("Main", shots(4, 90.0, 10.0, "Alice"));
    let b = steps.create("Branch", shots(3, 0.0, 8.0, "Bob"));
    let c = steps.create("Side", shots(3, 180.0, 6.0, "Carol"));
    let legacy = steps.run("CreateLine Legacy (legacy surveyor/date)", |state| {
        state.execute(Command::CreateLine {
            name: Some("Legacy".to_string()),
            surveyor: Some("Legacy Surveyor".to_string()),
            date: day(2),
            color: "#999999".to_string(),
            direction: None,
            shots: Vec::new(),
        })
    });
    let legacy = legacy.line_id.expect("legacy line id");
    let d = steps.create("Spur", shots(2, 45.0, 5.0, "Dan"));
    let k = steps.create("Keep", shots(2, 90.0, 5.0, "Kim"));
    let o = steps.create("Other", shots(3, 270.0, 4.0, "Olga"));
    let ds1 = steps.create("Delete Split", shots(4, 300.0, 3.0, "Eve"));
    let ds2 = steps.create("Range Split", shots(5, 310.0, 3.0, "Eve"));
    let ds3 = steps.create("Range Shift", shots(4, 320.0, 3.0, "Eve"));
    let ds4 = steps.create("Last Shot", shots(1, 330.0, 3.0, "Eve"));
    let ss = steps.create("Split Shot", shots(2, 340.0, 6.0, "Eve"));

    // --- Shots --------------------------------------------------------------
    steps.exec(
        "CreateShot append into the legacy-credited empty line",
        Command::CreateShot {
            line_id: legacy,
            shot: shot(10.0, 4.0, 3.0, "Leo"),
            after_shot_id: None,
            before_shot_id: None,
        },
    );
    steps.exec(
        "CreateShot after",
        Command::CreateShot {
            line_id: a,
            shot: shot(95.0, 3.0, 6.0, "Alice"),
            after_shot_id: Some(shot_id(&state, a, 1)),
            before_shot_id: None,
        },
    );
    steps.exec(
        "CreateShot before",
        Command::CreateShot {
            line_id: a,
            shot: shot(85.0, 2.0, 5.0, "Alice"),
            after_shot_id: None,
            before_shot_id: Some(shot_id(&state, a, 0)),
        },
    );
    steps.exec(
        "UpdateShot",
        Command::UpdateShot {
            line_id: a,
            shot_id: shot_id(&state, a, 2),
            updates: ShotUpdates {
                length: Some(11.0),
                comment: Some(Some("edited".to_string())),
                left: Some(None),
                ..Default::default()
            },
        },
    );
    let last_a = *shot_ids(&state, a).last().expect("a has shots");
    steps.exec(
        "SetShotKind",
        Command::SetShotKind {
            line_id: a,
            shot_id: last_a,
            kind: ShotKind::Jump,
        },
    );
    steps.exec(
        "UpdateLineMetadata name/color/direction/quality",
        Command::UpdateLineMetadata {
            line_id: a,
            updates: LineUpdates {
                name: Some(Some("Main Line".to_string())),
                color: Some("#123456".to_string()),
                direction: Some(SurveyDirection::Out),
                quality: Some(0.8),
                ..Default::default()
            },
        },
    );
    steps.exec(
        "UpdateLineMetadata legacy surveyor/date",
        Command::UpdateLineMetadata {
            line_id: b,
            updates: LineUpdates {
                name: Some(Some("Branch Line".to_string())),
                surveyor: Some(Some("Old Surveyor".to_string())),
                date: Some(day(3)),
                ..Default::default()
            },
        },
    );
    steps.exec(
        "EditShotCredit line",
        Command::EditShotCredit {
            targets: CreditTargets::Line { line_id: c },
            edit: CreditEdit {
                add_surveyors: Some(vec![person("Carl")]),
                set_date: Some(day(4)),
                ..Default::default()
            },
        },
    );
    steps.exec(
        "EditShotCredit shots",
        Command::EditShotCredit {
            targets: CreditTargets::Shots {
                shots: vec![(a, shot_id(&state, a, 0)), (a, shot_id(&state, a, 1))],
            },
            edit: CreditEdit {
                set_surveyors: Some(vec![person("Ann")]),
                add_explorers: Some(vec![person("Xavier")]),
                ..Default::default()
            },
        },
    );

    // --- Moves and merges ---------------------------------------------------
    steps.exec(
        "MoveLine",
        Command::MoveLine {
            line_id: b,
            offset: Coordinates::new(3.0, 4.0, 0.0),
        },
    );
    steps.exec(
        "MoveSegment",
        Command::MoveSegment {
            line_id: d,
            offset: Coordinates::new(-2.0, 1.0, 0.0),
        },
    );
    let a_end = end(&state, a);
    steps.exec(
        "MergeStations A end - B start",
        Command::MergeStations {
            station_a: a_end,
            station_b: StationRef::new(b, 0),
        },
    );
    steps.exec(
        "MergeStations A start - C start",
        Command::MergeStations {
            station_a: StationRef::new(a, 0),
            station_b: StationRef::new(c, 0),
        },
    );
    let b_end = end(&state, b);
    steps.exec(
        "MergeStations B end - D start",
        Command::MergeStations {
            station_a: b_end,
            station_b: StationRef::new(d, 0),
        },
    );
    steps.exec(
        "UpdateShot inside a rigid group",
        Command::UpdateShot {
            line_id: a,
            shot_id: shot_id(&state, a, 1),
            updates: ShotUpdates {
                length: Some(9.5),
                azimuth_in: Some(92.0),
                ..Default::default()
            },
        },
    );
    steps.exec(
        "MoveStation",
        Command::MoveStation {
            station_ref: StationRef::new(a, 2),
            offset: Coordinates::new(0.4, -0.3, 0.0),
        },
    );
    steps.exec(
        "UnmergeStationPair",
        Command::UnmergeStationPair {
            station_a: b_end,
            station_b: StationRef::new(d, 0),
        },
    );
    steps.exec(
        "MergeStations B end - D start again",
        Command::MergeStations {
            station_a: b_end,
            station_b: StationRef::new(d, 0),
        },
    );
    steps.exec(
        "UnmergeStations",
        Command::UnmergeStations {
            station_ref: StationRef::new(d, 0),
        },
    );

    // --- Loop closures ------------------------------------------------------
    let c_end = end(&state, c);
    steps.exec(
        "CreateLoopClosure Batch",
        Command::CreateLoopClosure {
            station_a: b_end,
            station_b: c_end,
            closure_strategy: ClosureStrategy::Batch,
        },
    );
    steps.exec(
        "CreateLoopClosure LeastSquaresWeighted",
        Command::CreateLoopClosure {
            station_a: StationRef::new(b, 1),
            station_b: StationRef::new(c, 2),
            closure_strategy: ClosureStrategy::LeastSquaresWeighted,
        },
    );
    let first_closure = project(&state)
        .loop_closures
        .first()
        .expect("a closure exists")
        .id;
    steps.exec(
        "RemoveLoopClosure",
        Command::RemoveLoopClosure {
            closure_id: first_closure,
        },
    );

    // --- Comments and datums ------------------------------------------------
    steps.exec(
        "SetStationComment",
        Command::SetStationComment {
            station_ref: StationRef::new(a, 1),
            comment: Some("Restriction".to_string()),
        },
    );
    steps.exec(
        "SetStationComment second",
        Command::SetStationComment {
            station_ref: StationRef::new(a, 3),
            comment: Some("Junction".to_string()),
        },
    );
    steps.exec(
        "MoveComment station -> shot",
        Command::MoveComment {
            source: CommentHolder::Station {
                station_ref: StationRef::new(a, 1),
            },
            target: CommentHolder::Shot {
                line_id: a,
                shot_id: shot_id(&state, a, 0),
            },
            swap: false,
        },
    );
    let gps = |lat: f64, lon: f64| GpsCoordinates::new(lat, lon, 0.0).expect("valid gps");
    steps.exec(
        "SetZeroDatum new (entrance, accuracy)",
        Command::SetZeroDatum {
            station_ref: StationRef::new(a, 0),
            coordinates: gps(20.5, -87.25),
            accuracy_m: Some(3.5),
            entrance: Some(true),
        },
    );
    steps.exec(
        "SetZeroDatum replace",
        Command::SetZeroDatum {
            station_ref: StationRef::new(a, 0),
            coordinates: gps(20.6, -87.3),
            accuracy_m: None,
            entrance: Some(false),
        },
    );
    steps.exec(
        "RemoveZeroDatum",
        Command::RemoveZeroDatum {
            station_ref: StationRef::new(a, 0),
        },
    );
    steps.exec(
        "SetZeroDatum again",
        Command::SetZeroDatum {
            station_ref: StationRef::new(a, 0),
            coordinates: gps(20.5, -87.25),
            accuracy_m: Some(2.0),
            entrance: None,
        },
    );

    // --- Reverse, delete, batch ---------------------------------------------
    steps.exec(
        "ReverseLine",
        Command::ReverseLine {
            line_id: c,
            swap_left_right: true,
        },
    );
    steps.exec(
        "DeleteShot",
        Command::DeleteShot {
            line_id: d,
            shot_id: shot_id(&state, d, 1),
        },
    );
    steps.exec(
        "Batch (create, update, credit, metadata, reverse)",
        Command::Batch {
            commands: vec![
                Command::CreateLine {
                    name: Some("Batched".to_string()),
                    surveyor: None,
                    date: None,
                    color: "#abcdef".to_string(),
                    direction: None,
                    shots: shots(2, 15.0, 3.0, "Bea"),
                },
                Command::UpdateShot {
                    line_id: b,
                    shot_id: shot_id(&state, b, 0),
                    updates: ShotUpdates {
                        depth_in: Some(7.5),
                        ..Default::default()
                    },
                },
                Command::EditShotCredit {
                    targets: CreditTargets::Line { line_id: b },
                    edit: CreditEdit {
                        add_surveyors: Some(vec![person("Batch Surveyor")]),
                        ..Default::default()
                    },
                },
                Command::UpdateLineMetadata {
                    line_id: d,
                    updates: LineUpdates {
                        name: Some(Some("Spur Line".to_string())),
                        ..Default::default()
                    },
                },
                Command::ReverseLine {
                    line_id: d,
                    swap_left_right: false,
                },
            ],
        },
    );

    // --- Import, withdraw, create-and-merge ---------------------------------
    let before_import = line_ids(&state);
    let import_line = |index: u32, name: &str, placement: ImportPlacement| ImportLineConfig {
        line_index: index,
        name: Some(name.to_string()),
        surveyor: Some("Importer".to_string()),
        date: day(5),
        surveyors: Some(vec![person("Importer")]),
        explorers: None,
        override_date: None,
        color: "#ff8800".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots: shots(3, 60.0 + f64::from(index) * 30.0, 5.0, "Importer"),
        placement,
        station_comments: vec![(1, format!("{name} comment"))],
        station_names: vec![(0, format!("{name}-0")), (2, format!("{name}-2"))],
        reverse: false,
        azimuth_reference: None,
    };
    let b_last = end(&state, b);
    steps.run("Import lines", |state| {
        state.execute_import_lines(
            vec![
                import_line(
                    0,
                    "Imported One",
                    ImportPlacement::FromExisting {
                        import_station: 0,
                        existing_station: b_last,
                    },
                ),
                import_line(
                    1,
                    "Imported Two",
                    ImportPlacement::FromImporting {
                        import_station: 0,
                        target_line_index: 0,
                        target_station_index: 2,
                    },
                ),
            ],
            vec![ImportClosure {
                from_line_index: 1,
                from_station_index: 3,
                to_line_index: 0,
                to_station_index: 3,
            }],
            Some("synthetic-file-hash".to_string()),
            vec![
                "synthetic-line-hash-0".to_string(),
                "synthetic-line-hash-1".to_string(),
            ],
            vec![ExistingShotUpdate {
                line_id: c,
                shot_id: shot_id(state, c, 0),
                updates: ShotUpdates {
                    comment: Some(Some("updated by import".to_string())),
                    ..Default::default()
                },
            }],
            VirtualShotMode::Auto,
        )
    });
    let imported: Vec<LineId> = line_ids(&state)
        .into_iter()
        .filter(|id| !before_import.contains(id))
        .collect();
    let withdrawn = *imported.last().expect("the import created lines");
    steps.exec(
        "Batch (DeleteLine + UntrackImport)",
        Command::Batch {
            commands: vec![
                Command::DeleteLine {
                    line_id: withdrawn,
                },
                Command::UntrackImport {
                    file_hashes: vec!["synthetic-file-hash".to_string()],
                    line_hashes: vec!["synthetic-line-hash-1".to_string()],
                },
            ],
        },
    );
    let c_last = end(&state, c);
    steps.run("Create and merge", |state| {
        state.execute_create_and_merge(
            Some("Created Merged".to_string()),
            Some("Merger".to_string()),
            day(6),
            "#00ffaa".to_string(),
            Some(SurveyDirection::Out),
            shots(2, 135.0, 4.0, "Merger"),
            0,
            c_last,
            ClosureStrategy::Batch,
        )
    });

    // --- Combine (parities differ -> counter-parity shots) ------------------
    steps.exec(
        "MergeStations K end - O end",
        Command::MergeStations {
            station_a: end(&state, k),
            station_b: end(&state, o),
        },
    );
    steps.exec(
        "ReverseLine O",
        Command::ReverseLine {
            line_id: o,
            swap_left_right: false,
        },
    );
    steps.run("Combine lines", |state| {
        state.execute_combine_lines(
            k,
            o,
            CombineLinesOptions {
                name: "Combined".to_string(),
                color: "#abcdef".to_string(),
                swap_lr: true,
                comment_pick: CombinePick::Both,
                name_pick: CombinePick::Kept,
                join: None,
            },
        )
    });

    // --- Combine that leaves a held gauge -----------------------------------
    // X (lowest id) holds its component's gauge; Y, a branch off X, has an id
    // between X and Z. Combining X into Z retires X, so the component's new
    // lowest line (Y) does not hold the gauge station and the combine
    // records it in `Project::held_gauges`.
    let gx = steps.create("Gauge X", shots(2, 30.0, 5.0, "Gus"));
    let gy = steps.create("Gauge Y", shots(2, 120.0, 4.0, "Gus"));
    let gz = steps.create("Gauge Z", shots(2, 30.0, 6.0, "Gus"));
    steps.exec(
        "MergeStations X end - Z start",
        Command::MergeStations {
            station_a: end(&state, gx),
            station_b: StationRef::new(gz, 0),
        },
    );
    steps.exec(
        "MergeStations X 1 - Y start",
        Command::MergeStations {
            station_a: StationRef::new(gx, 1),
            station_b: StationRef::new(gy, 0),
        },
    );
    steps.run("Combine lines (held gauge)", |state| {
        state.execute_combine_lines(
            gz,
            gx,
            CombineLinesOptions {
                name: "Gauge Combined".to_string(),
                color: "#445566".to_string(),
                swap_lr: false,
                comment_pick: CombinePick::Kept,
                name_pick: CombinePick::Kept,
                join: None,
            },
        )
    });
    assert!(
        !project(&state).held_gauges.is_empty(),
        "the combine recorded a held gauge"
    );

    // --- Splits and delete-and-split ----------------------------------------
    let split_options = |a_name: &str, b_name: &str| SplitLineOptions {
        name_a: a_name.to_string(),
        name_b: b_name.to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::Both,
        datum_side: SplitSide::A,
    };
    steps.run("Split line at a station", |state| {
        state.execute_split_line(
            a,
            SplitAt::Station { station_index: 2 },
            split_options("Main A", "Main B"),
        )
    });
    let b_shot = shot_id(&state, b, 1);
    steps.run("Split line inside a shot", |state| {
        state.execute_split_line(
            b,
            SplitAt::Shot {
                shot_id: b_shot,
                distance: 3.0,
            },
            split_options("Branch A", "Branch B"),
        )
    });
    let ds1_shot = shot_id(&state, ds1, 1);
    steps.run("Delete shot and split", |state| {
        state.execute_delete_shot_and_split(
            ds1,
            ds1_shot,
            "Delete Split A".to_string(),
            "Delete Split B".to_string(),
        )
    });
    let (ds2_from, ds2_to) = (shot_id(&state, ds2, 1), shot_id(&state, ds2, 2));
    steps.run("Delete shot range and split", |state| {
        state.execute_delete_shot_range_split(
            ds2,
            ds2_from,
            ds2_to,
            "Range Split A".to_string(),
            "Range Split B".to_string(),
        )
    });
    let (ds3_from, ds3_to) = (shot_id(&state, ds3, 1), shot_id(&state, ds3, 2));
    steps.run("Delete shot range and shift", |state| {
        state.execute_delete_shot_range_shift(ds3, ds3_from, ds3_to)
    });
    let ds4_shot = shot_id(&state, ds4, 0);
    steps.run("Delete last shot and line", |state| {
        state.execute_delete_last_shot_and_line(ds4, ds4_shot)
    });
    let ss_shot = shot_id(&state, ss, 0);
    steps.run("Split shot at distance", |state| {
        state.execute_split_shot_at_distance(ss, ss_shot, 2.5)
    });

    // --- Project-level snapshot-pair entries --------------------------------
    let mut validation = state.get_config().validation;
    validation.warn_shot_length_max = 75.0;
    steps.run("Update project settings", |state| {
        state.update_project_settings("Legacy Vectors Renamed".to_string(), validation)
    });
    steps.run("Set surface elevation", |state| {
        state.set_surface_elevation(Some(12.5))
    });
    steps.run("Repair integrity", |state| {
        {
            let mut guard = state.project.lock().expect("project lock");
            let live = guard.as_mut().expect("a project is open");
            live.station_offsets.insert(
                StationKey::new(ss, StationAnchor::AfterShot("ghost".to_string())),
                (1.0, 2.0, 3.0),
            );
        }
        let repaired = state.repair_open_project_integrity();
        assert!(repaired.changed, "repair changed the project");
        CommandResult::success()
    });
    let source = write_add_source(temp.path());
    steps.exec("Add .stalx", Command::add_project_file(source));
    steps.exec(
        "DeleteLine",
        Command::DeleteLine { line_id: legacy },
    );

    // --- Three cheap edits, undone below so the redo stack holds them -------
    steps.exec(
        "SetStationComment (to be undone)",
        Command::SetStationComment {
            station_ref: StationRef::new(ss, 1),
            comment: Some("Redo me".to_string()),
        },
    );
    steps.exec(
        "CreateShot append (to be undone)",
        Command::CreateShot {
            line_id: ss,
            shot: shot(345.0, 2.0, 7.0, "Eve"),
            after_shot_id: None,
            before_shot_id: None,
        },
    );
    steps.exec(
        "UpdateShot (to be undone)",
        Command::UpdateShot {
            line_id: ss,
            shot_id: shot_id(&state, ss, 0),
            updates: ShotUpdates {
                depth_out: Some(9.0),
                kind: Some(ShotKind::Reference),
                ..Default::default()
            },
        },
    );

    // --- Dive plan sidecar --------------------------------------------------
    let k_end = end(&state, k);
    let plan = state
        .save_dive_plan(
            None,
            "Synthetic plan".to_string(),
            DivePlanSnapshot {
                origin: Some(StationRef::new(k, 0)),
                endpoint: Some(k_end),
                waypoints: vec![StationRef::new(k, 2)],
                diver: Diver {
                    rmv_l_per_min: 18.0,
                    label: Some("Diver".to_string()),
                },
                cylinders: Vec::new(),
                gas_config: GasConfig::default(),
                stage_drop_overrides: vec![None, Some(StationRef::new(k, 3))],
                marked_points: vec![MarkedPoint {
                    station: StationRef::new(k, 1),
                    label: Some("photo".to_string()),
                }],
                segments: Vec::new(),
            },
        )
        .expect("save dive plan");
    assert!(plan.id > 0);

    // --- Leave five entries on the redo stack -------------------------------
    const UNDONE: usize = 5;
    for _ in 0..UNDONE {
        let result = state.undo();
        assert!(result.success, "undo: {:?}", result.error);
    }
    let mut undo_labels = steps.labels.clone();
    let mut redo_labels = undo_labels.split_off(undo_labels.len() - UNDONE);
    redo_labels.reverse();

    // --- Legacy UndoData shapes no live path produces any more ---------------
    for (label, entry) in legacy_entries(&state) {
        state
            .undo_stack
            .lock()
            .expect("undo stack lock")
            .push_undo(entry);
        undo_labels.push(label);
    }

    // A project-level north reference, so `project_settings` carries that key.
    {
        let mut guard = state.project.lock().expect("project lock");
        let live = guard.as_mut().expect("a project is open");
        live.default_azimuth_reference = Some(AzimuthReference::magnetic(
            -4.0,
            CorrectionSource::FileDeclared,
        ));
    }
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "final save: {:?}", saved.error);

    // --- Read the saved file back -------------------------------------------
    let (undo_rows, redo_rows) = {
        let db = Database::open_readonly(&stalx).expect("open saved stalx");
        (
            db.load_undo_stack_with_timestamps()
                .expect("load undo rows"),
            db.load_redo_stack_with_timestamps()
                .expect("load redo rows"),
        )
    };
    let conn = Connection::open_with_flags(&stalx, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open saved stalx with rusqlite");
    let count = |table: &str| -> usize {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("count rows") as usize
    };
    assert_eq!(
        count("undo_stack"),
        undo_rows.len(),
        "every undo row loaded"
    );
    assert_eq!(
        count("redo_stack"),
        redo_rows.len(),
        "every redo row loaded"
    );
    assert_eq!(undo_rows.len(), undo_labels.len(), "one undo row per step");
    assert_eq!(redo_rows.len(), redo_labels.len(), "one redo row per undo");

    let undo = dump_stack(undo_rows, &undo_labels);
    let redo = dump_stack(redo_rows, &redo_labels);
    write_json(&out, "undo_commands.json", &Value::Array(undo.commands));
    write_json(&out, "undo_data.json", &Value::Array(undo.data));
    write_json(&out, "undo_rows_raw.json", &Value::Array(undo.raw));
    write_json(
        &out,
        "undo_embedded_json.json",
        &Value::Array(undo.embedded),
    );
    write_json(&out, "redo_commands.json", &Value::Array(redo.commands));
    write_json(&out, "redo_data.json", &Value::Array(redo.data));
    write_json(&out, "redo_rows_raw.json", &Value::Array(redo.raw));
    write_json(
        &out,
        "redo_embedded_json.json",
        &Value::Array(redo.embedded),
    );
    write_json(
        &out,
        "manifest.json",
        &json!({ "undo": undo.manifest, "redo": redo.manifest }),
    );
    write_json(
        &out,
        "project_settings.json",
        &key_value_table(&conn, "project_settings"),
    );
    write_json(
        &out,
        "display_settings.json",
        &key_value_table(&conn, "display_settings"),
    );
    write_json(&out, "other_json_cells.json", &other_json_cells(&conn));
    let plnx = temp.path().join("legacy-vectors.plnx");
    write_json(&out, "dive_plan.json", &dive_plan_dump(&plnx));
    let live_snapshot =
        AppState::serialize_project_snapshot(&project(&state)).expect("serialize project");
    write_json(
        &out,
        "project_snapshot_final.json",
        &parse(&live_snapshot, "project snapshot"),
    );

    // App config: TOML on disk through the real writer, plus its JSON form.
    // Written in the temp dir and copied, because the writer also drops a
    // SpeleoDB side file next to it that is not a vector.
    let config = Config::default();
    let config_path = temp.path().join("config").join("config.toml");
    config
        .save_to_path(&config_path)
        .expect("write config.toml");
    std::fs::copy(&config_path, out.join("config.toml")).expect("copy config.toml");
    write_json(
        &out,
        "config.json",
        &canonical(serde_json::to_value(&config).expect("config to JSON")),
    );

    println!(
        "legacy vectors: {} undo rows, {} redo rows -> {}",
        undo.manifest.len(),
        redo.manifest.len(),
        out.display()
    );
}
