//! Compact undo entries cross the disk boundary in the pre-rename vocabulary
//! and come back exactly.
//!
//! A row's whole `{"v2": ...}` envelope goes through `models::disk_names`
//! (`undo/persistence.rs`), so a delta's member names, the keys of the literal
//! values it carries and its preview facts are written as a snapshot half's
//! are (`lines` as `sections`, `line_id` as `section_id`, ...) and translated
//! back before anything reads them. These tests build a history with every
//! kind of compact entry the app records, check that every payload's disk form
//! holds no code-vocabulary key and reads back to the same payload, then save
//! it, reopen it and walk it end to end against the states it was recorded
//! from.

#![expect(
    clippy::unwrap_used,
    reason = "test module: a failed setup step is a failed test"
)]

use std::path::Path;

use serde_json::Value;
use tempfile::TempDir;

use super::fuzz_undo_tests::canonical_snapshot;
use super::AppState;
use crate::core::{BeforePreview, Command, CommandResult, UndoData, UndoEntry, UndoStack};
use crate::models::{Coordinates, GpsCoordinates, LineId, ShotData, StationRef};

/// Keys this build serializes a project and its undo payloads with that the
/// disk vocabulary names otherwise: none may appear in a written row.
const CODE_ONLY_KEYS: &[&str] = &[
    "lines",
    "line_id",
    "next_line_id",
    "line_offsets",
    "imported_line_hashes",
    "Line",
    "CreatedLineSnapshot",
];

fn ok(result: CommandResult) -> CommandResult {
    assert!(result.success, "{:?}", result.error);
    result
}

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 10.0,
        depth_out: 10.5,
        left: Some(1.0),
        ..Default::default()
    }
}

fn create(state: &AppState, name: &str, shots: usize, azimuth: f64) -> LineId {
    ok(state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: (0..shots)
            .map(|i| shot(azimuth + i as f64, 4.0 + i as f64 * 0.25))
            .collect(),
        cave_section_id: None,
    }))
    .line_id
    .unwrap()
}

/// A saved project whose history holds every compact entry kind the app
/// records - a whole-line delete (preview: the line), a batch that deletes a
/// named line (preview: its name), a datum removal (preview: the GPS), a
/// reverse, a shot delete, a merge and a station move - with typed entries of
/// #804 (a Section, a station rename) between them and one entry on the redo
/// stack. Line ids, Section membership and station names all ride in the
/// recorded states.
fn history(dir: &Path) -> AppState {
    let state = AppState::new().unwrap();
    ok(state.execute(Command::CreateProject {
        name: "Disk names".to_string(),
        location: None,
    }));
    ok(state.execute(Command::SaveProject {
        path: Some(dir.join("history.stalx")),
    }));
    let main = create(&state, "Main", 6, 90.0);
    let side = create(&state, "Side", 4, 10.0);
    let branch = create(&state, "Branch", 3, 200.0);
    let spur = create(&state, "Spur", 2, 300.0);
    ok(state.execute(Command::CreateCaveSection {
        name: "Upper level".to_string(),
        color: "#336699".to_string(),
        line_ids: vec![main, side, spur],
    }));
    ok(state.execute(Command::SetStationName {
        station_ref: StationRef::new(main, 2),
        name: "2 junction".to_string(),
    }));
    ok(state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(main, 0),
        coordinates: GpsCoordinates {
            latitude: 20.5,
            longitude: -87.25,
            depth_reference: 0.0,
        },
        accuracy_m: None,
        entrance: None,
    }));
    ok(state.execute(Command::MergeStations {
        station_a: StationRef::new(side, 0),
        station_b: StationRef::new(main, 3),
    }));
    ok(state.execute(Command::ReverseLine {
        line_id: branch,
        swap_left_right: true,
    }));
    let shot_id = state
        .get_project_state()
        .unwrap()
        .get_line(main)
        .unwrap()
        .shots[4]
        .id;
    ok(state.execute(Command::DeleteShot {
        line_id: main,
        shot_id,
    }));
    ok(state.execute(Command::MoveStation {
        station_ref: StationRef::new(side, 2),
        offset: Coordinates::new(0.5, -0.25, 0.0),
    }));
    ok(state.execute(Command::Batch {
        commands: vec![Command::DeleteLine { line_id: spur }],
        label: None,
    }));
    ok(state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(main, 0),
    }));
    ok(state.execute(Command::DeleteLine { line_id: branch }));
    // One on the redo stack too: its delta runs the other way.
    ok(state.undo());
    state
}

fn entries(state: &AppState) -> Vec<UndoEntry> {
    let stack = state.undo_stack.lock().unwrap();
    stack
        .undo_entries()
        .chain(stack.redo_entries())
        .cloned()
        .collect()
}

fn keys(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                out.push(key.clone());
                keys(inner, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keys(item, out)),
        _ => {}
    }
}

#[test]
fn a_compact_entry_round_trips_through_the_disk_vocabulary() {
    let dir = TempDir::new().unwrap();
    let state = history(dir.path());
    let entries = entries(&state);
    let mut deltas = 0;
    let mut previews = (0, 0, 0);
    let mut disk_keys_seen = false;
    for entry in &entries {
        let (command_json, data_json) = UndoStack::serialize_entry(entry).unwrap();
        assert!(data_json.starts_with("{\"v2\":"), "{data_json}");
        let disk: Value = serde_json::from_str(&data_json).unwrap();
        let mut found = Vec::new();
        keys(&disk, &mut found);
        for key in CODE_ONLY_KEYS {
            assert!(
                !found.iter().any(|k| k == key),
                "{}: the written payload holds the code key {key:?}: {data_json}",
                entry.command.description()
            );
        }
        disk_keys_seen |= found
            .iter()
            .any(|k| k == "sections" || k == "section_id" || k == "Section");

        let back = UndoStack::deserialize_entry(&command_json, &data_json);
        assert!(
            back.is_some(),
            "{}: the row does not read back",
            entry.command.description()
        );
        let back = back.unwrap();
        assert_eq!(
            serde_json::to_value(&back.undo_data).unwrap(),
            serde_json::to_value(&entry.undo_data).unwrap(),
            "{}: the payload does not read back as it was written",
            entry.command.description()
        );
        assert_eq!(
            serde_json::to_value(&back.command).unwrap(),
            serde_json::to_value(&entry.command).unwrap()
        );
        // Written again, the row is byte-identical: the pair is a bijection
        // on everything the payload holds.
        assert_eq!(
            UndoStack::serialize_entry(&back).unwrap().1,
            data_json,
            "{}",
            entry.command.description()
        );
        if let (UndoData::Delta(written), UndoData::Delta(read)) =
            (&entry.undo_data, &back.undo_data)
        {
            deltas += 1;
            assert_eq!(read.delta().unwrap(), written.delta().unwrap());
            assert!(read.delta().unwrap().check_shape().is_ok());
            match &written.preview {
                Some(BeforePreview::Line { .. }) => previews.0 += 1,
                Some(BeforePreview::DeletedLines(names)) => {
                    assert_eq!(
                        names.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
                        ["Spur"]
                    );
                    previews.1 += 1;
                }
                Some(BeforePreview::Datum(gps)) => {
                    assert!(gps.is_some(), "the removed datum's GPS");
                    previews.2 += 1;
                }
                None => {}
            }
        }
    }
    assert!(deltas >= 6, "only {deltas} compact entries in the history");
    assert_eq!(previews, (1, 1, 1), "one entry of each preview kind");
    assert!(
        disk_keys_seen,
        "no payload named a line in the disk vocabulary"
    );
}

#[test]
fn a_deleted_lines_label_and_preview_survive_compaction() {
    let dir = TempDir::new().unwrap();
    let state = history(dir.path());
    // The single Delete Line went to the redo stack; the batch that deleted
    // "Spur" is below the datum removal on the undo stack.
    let undo = state.get_undo_stack();
    let batch = undo.iter().find(|item| {
        item.preview_data
            .as_ref()
            .is_some_and(|p| p.changes.iter().any(|c| c.field == "Delete line \"Spur\""))
    });
    assert!(
        batch.is_some(),
        "the batch preview names its deleted line: {undo:?}"
    );
    let redo = state.get_redo_stack();
    assert!(
        redo.iter()
            .any(|item| item.description == "Delete line \"Branch\""),
        "the compact Delete Line entry names its line once it is gone: {redo:?}"
    );
}

#[test]
fn a_saved_history_of_compact_entries_reloads_and_walks_exactly() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("history.stalx");
    let state = history(dir.path());
    // A save heals the live project with no undo entry (it captures the
    // gauge pairs the lines lack), and every later restore carries that heal
    // onto the states it installs. Save first, so the states recorded below
    // are the healed ones the reopened file walks through.
    ok(state.execute(Command::SaveProject { path: None }));
    let (undo_len, redo_len) = {
        let stack = state.undo_stack.lock().unwrap();
        (stack.len(), stack.redo_len())
    };
    // Every state of the timeline, by depth, as the fuzzer compares them
    // (shot ids by position: the loader renumbers them).
    let mut states = vec![String::new(); undo_len + redo_len + 1];
    states[undo_len] = canonical_snapshot(&state, true);
    for state_at in states[..undo_len].iter_mut().rev() {
        ok(state.undo());
        *state_at = canonical_snapshot(&state, true);
    }
    for state_at in &mut states[1..] {
        ok(state.redo());
        *state_at = canonical_snapshot(&state, true);
    }
    for _ in 0..redo_len {
        ok(state.undo());
    }
    assert_eq!(canonical_snapshot(&state, true), states[undo_len]);

    ok(state.execute(Command::SaveProject { path: None }));
    ok(state.execute(Command::CloseProject));
    ok(state.execute(Command::OpenProject { path }));
    assert!(
        !state.history_discarded(),
        "the app's own file keeps its history"
    );
    {
        let stack = state.undo_stack.lock().unwrap();
        assert_eq!((stack.len(), stack.redo_len()), (undo_len, redo_len));
        assert!(
            stack
                .undo_entries()
                .chain(stack.redo_entries())
                .any(|e| matches!(e.undo_data, UndoData::Delta(_))),
            "the reloaded history holds compact entries"
        );
    }
    assert_eq!(
        canonical_snapshot(&state, true),
        states[undo_len],
        "the reopened project"
    );
    for (depth, expected) in states[..undo_len].iter().enumerate().rev() {
        ok(state.undo());
        assert_eq!(
            &canonical_snapshot(&state, true),
            expected,
            "undo to depth {depth} after the reopen"
        );
    }
    for (depth, expected) in states.iter().enumerate().skip(1) {
        ok(state.redo());
        assert_eq!(
            &canonical_snapshot(&state, true),
            expected,
            "redo to depth {depth} after the reopen"
        );
    }
}

/// What a compact entry's payload was before it was held serialized: the
/// delta as a parsed struct. A row the history writes now must be the row this
/// wrote, byte for byte.
#[derive(serde::Serialize)]
struct ParsedPayload<'a> {
    #[serde(rename = "d")]
    delta: crate::core::ProjectDelta,
    #[serde(rename = "p", skip_serializing_if = "Option::is_none")]
    preview: &'a Option<BeforePreview>,
    #[serde(rename = "n", skip_serializing_if = "is_zero")]
    omitted: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A compact entry holds its delta as JSON text, but its row is written as it
/// always was: `d` the delta's object, translated to the disk names - not a
/// string - so the rows already written and the ones written now are one
/// format, and either reads back.
#[test]
fn a_held_delta_is_written_as_the_parsed_delta_was() {
    let dir = TempDir::new().unwrap();
    let state = history(dir.path());
    let mut deltas = 0;
    for entry in entries(&state) {
        let UndoData::Delta(held) = &entry.undo_data else {
            continue;
        };
        deltas += 1;
        let (_, data_json) = UndoStack::serialize_entry(&entry).unwrap();
        let mut parsed = serde_json::json!({
            "v2": {
                "Delta": ParsedPayload {
                    delta: held.delta().unwrap(),
                    preview: &held.preview,
                    omitted: held.omitted_commands,
                }
            }
        });
        crate::models::disk_names::to_disk(&mut parsed);
        assert_eq!(
            data_json,
            serde_json::to_string(&parsed).unwrap(),
            "{}",
            entry.command.description()
        );
    }
    assert!(deltas >= 6, "only {deltas} compact entries");
}

/// A row in the format the history writes, pinned as text: it reads back to
/// the delta it spells out and writes back byte for byte.
#[test]
fn a_pinned_compact_row_reads_and_writes_back_unchanged() {
    let row = concat!(
        r##"{"v2":{"Delta":{"d":{"f":{"next_section_id":{"s":3},"station_names":{"b":{"##,
        r##""sd":[[2,0,"G0"],[2,1,"G1"]]}}},"s":[2,[{"k":[0,1]},{"e":{"at":1,"f":{"name":"##,
        r##"{"s":"Gone"}},"id":1,"s":[{"c":{"at":0,"n":2,"sec":1}},{"p":[{"t":{"k":["id","##,
        r##""length","uuid"],"r":[[100002,4.5,"9d1c2f6a-2b0e-4e3a-9c41-1f0d5a7b3e21"]]}}]}]}},"##,
        r##"{"n":{"f":{"color":"#123456","id":2,"name":"Gone","next_shot_id":200001,"##,
        r##""shots":[]}}}]]},"n":7,"p":{"Section":{"name":"Gone","shots":0}}}}}"##
    );
    let command = r#"{"type":"batch","commands":[],"label":"Pinned"}"#;
    let entry = UndoStack::deserialize_entry(command, row).expect("the pinned row reads");
    let held = match &entry.undo_data {
        UndoData::Delta(held) => Some(held),
        _ => None,
    }
    .expect("a compact entry");
    assert_eq!(held.omitted_commands, 7);
    assert!(matches!(
        &held.preview,
        Some(BeforePreview::Line { name: Some(name), shots: 0 }) if name == "Gone"
    ));
    let delta = held.delta().unwrap();
    assert!(delta.check_shape().is_ok());
    let written = serde_json::from_str::<Value>(row).unwrap();
    let expected: crate::core::ProjectDelta = crate::models::disk_names::from_disk_str(
        &serde_json::to_string(written.pointer("/v2/Delta/d").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(delta, expected);
    assert_eq!(UndoStack::serialize_entry(&entry).unwrap().1, row);
}
