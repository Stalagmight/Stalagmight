//! The persisted JSON a build from before the Section -> Line rename wrote,
//! read and written back by this build.
//!
//! `undo/testdata/pre_line_rename_rows.json` holds rows captured with
//! `legacy_disk_vectors_dump` on the commit before the rename: the undo and
//! redo history of a project that ran every undoable `Command` and every
//! `UndoData` variant, its `held_gauges` setting and a dive-plan sidecar
//! snapshot. Whole-project snapshots are kept for the two smallest rows of each
//! snapshot variant only (the file would be ~2 MB otherwise); the other rows
//! keep their command.
//!
//! Each command must parse in this build, and serialize back to the same JSON
//! (compared as values, with the JSON documents nested in strings compared
//! parsed): what this build saves stays what an older build reads.
//!
//! The undo payloads are different by design: this build writes every one in
//! the compact history's `{"v2": ...}` envelope and drops a history written
//! before it (the undo history keeps no compatibility with older builds; see
//! `docs/undo-redo.md`, "Compact entries"). A captured row as it is is
//! therefore an earlier version's and is refused. Wrapped in the envelope, the
//! payload of every variant this build still has must read in the new names
//! and write back to the same disk JSON - the vocabulary of a payload did not
//! change with the envelope - and the payloads the compact history deleted,
//! or writes in another shape (`ShotCredits`), must not read at all.

use serde_json::Value;

use super::dive_plan::PersistedDivePlanSnapshot;
use crate::core::{Command, UndoStack};
use crate::models::{disk_names, HeldGauge};

const ROWS: &str = include_str!("../undo/testdata/pre_line_rename_rows.json");

/// Keys whose string value is a JSON document (see `disk_names`).
const NESTED: &[&str] = &["ProjectSnapshot", "before", "after", "values_json"];

/// The `ShotUpdates` readings that are a plain `Option`, where an explicit
/// `null` and an absent key read alike in every build (`serde(default)`).
/// This build leaves an unset one out (the compact history writes what a
/// command changed and nothing else); the build before the rename wrote
/// `null`. The double-option fields (`left`, `comment`, ...) are not here:
/// for them `null` means "clear" and is kept.
const PLAIN_OPTION_READINGS: &[&str] = &[
    "azimuth_in",
    "azimuth_out",
    "length",
    "depth_in",
    "depth_out",
];

/// Parse `text`, then parse the JSON documents nested in it as strings, so two
/// serializations compare by content rather than by key order; and drop an
/// explicit `null` of a [`PLAIN_OPTION_READINGS`] key, which reads exactly
/// like the key left out.
fn deep(text: &str) -> Value {
    let mut value: Value = serde_json::from_str(text).expect("a captured row is JSON");
    expand(&mut value);
    drop_unset_readings(&mut value);
    value
}

fn drop_unset_readings(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|key, inner| {
                !(inner.is_null() && PLAIN_OPTION_READINGS.contains(&key.as_str()))
            });
            map.values_mut().for_each(drop_unset_readings);
        }
        Value::Array(items) => items.iter_mut().for_each(drop_unset_readings),
        _ => {}
    }
}

fn expand(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                let nested_pair = key == "section_values";
                if NESTED.contains(&key.as_str()) {
                    expand_string(inner);
                }
                if nested_pair {
                    if let Value::Array(items) = inner {
                        items.iter_mut().for_each(expand_string);
                    }
                }
                expand(inner);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(expand),
        _ => {}
    }
}

fn expand_string(value: &mut Value) {
    if let Value::String(text) = value {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.is_object() || parsed.is_array() {
                *value = parsed;
            }
        }
    }
}

/// The payloads, by their disk tag, that this build cannot read even inside
/// its envelope: the variants the compact undo history deleted (only a
/// pre-compact history held them), and `ShotCredits`, whose changes it writes
/// as grouped runs (`grouped_credit_changes`) rather than one row per shot.
const UNREADABLE_PAYLOADS: &[&str] = &[
    "SectionSnapshot",
    "ShotSnapshot",
    "MergeInfo",
    "UnmergeInfo",
    "StationMoveInfo",
    "ProjectSnapshot",
    "ShotCredits",
];

/// The disk tag of a captured payload: its one top-level key (`None` for a
/// unit variant, a bare string).
fn payload_tag(undo_data_json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(undo_data_json).ok()?;
    value.as_object()?.keys().next().cloned()
}

fn rows() -> Value {
    serde_json::from_str(ROWS).expect("the vector file is JSON")
}

#[test]
fn every_captured_history_row_reads_and_writes_back_unchanged() {
    let rows = rows();
    let mut checked_commands = 0;
    let mut checked_data = 0;
    let mut refused_legacy = 0;
    for stack in ["undo", "redo"] {
        for row in rows[stack].as_array().expect("rows") {
            let label = row["label"].as_str().unwrap_or("?");
            let command_json = row["command_json"].as_str().expect("command_json");
            let command: Command = disk_names::from_disk_str(command_json)
                .unwrap_or_else(|e| panic!("{stack} row {label:?}: command does not parse: {e}"));
            let written = disk_names::to_disk_string(&command).expect("serialize");
            assert_eq!(
                deep(&written),
                deep(command_json),
                "{stack} row {label:?}: the command does not write back as it was read"
            );
            checked_commands += 1;
            let Some(undo_data_json) = row["undo_data_json"].as_str() else {
                continue;
            };
            assert!(
                UndoStack::deserialize_entry(command_json, undo_data_json).is_none(),
                "{stack} row {label:?}: a row without this build's envelope is an earlier \
                 version's, and must not read"
            );
            let wrapped = format!("{{\"v2\":{undo_data_json}}}");
            let unreadable = payload_tag(undo_data_json)
                .is_some_and(|tag| UNREADABLE_PAYLOADS.contains(&tag.as_str()));
            let Some(entry) = UndoStack::deserialize_entry(command_json, &wrapped) else {
                assert!(
                    unreadable,
                    "{stack} row {label:?}: the payload does not parse in the envelope"
                );
                refused_legacy += 1;
                continue;
            };
            assert!(
                !unreadable,
                "{stack} row {label:?}: a payload this build no longer has still parses"
            );
            let (command_out, data_out) =
                UndoStack::serialize_entry(&entry).expect("an entry serializes");
            assert_eq!(
                deep(&command_out),
                deep(command_json),
                "{stack} row {label:?}"
            );
            assert_eq!(
                deep(&data_out),
                deep(&wrapped),
                "{stack} row {label:?}: the undo data does not write back as it was read"
            );
            checked_data += 1;
        }
    }
    assert_eq!(checked_commands, 79, "every captured row is checked");
    assert!(
        refused_legacy >= UNREADABLE_PAYLOADS.len(),
        "only {refused_legacy} rows of the payloads this build no longer reads"
    );
    assert!(
        checked_data >= 36,
        "only {checked_data} undo-data rows checked"
    );
}

#[test]
fn the_code_vocabulary_differs_from_the_disk_one() {
    // Guards the test above against passing vacuously: if the code still
    // spoke the disk names, the translation would be a no-op.
    let rows = rows();
    let first = rows["undo"][0]["command_json"]
        .as_str()
        .expect("command_json");
    let command: Command = disk_names::from_disk_str(first).expect("parses");
    let in_code = serde_json::to_string(&command).expect("serialize");
    assert!(first.contains("create_section"), "{first}");
    assert!(
        in_code.contains("create_line") && !in_code.contains("section"),
        "{in_code}"
    );
}

#[test]
fn held_gauges_read_and_write_back_unchanged() {
    let rows = rows();
    let raw = rows["held_gauges"].as_str().expect("held_gauges");
    let gauges: Vec<HeldGauge> = disk_names::from_disk_str(raw).expect("parses");
    assert!(!gauges.is_empty());
    let written = disk_names::to_disk_string(&gauges).expect("serialize");
    assert_eq!(deep(&written), deep(raw));
}

#[test]
fn dive_plan_snapshots_read_and_write_back_unchanged() {
    let rows = rows();
    let plans = rows["dive_plans"].as_array().expect("dive_plans");
    assert!(!plans.is_empty());
    for raw in plans {
        let raw = raw.as_str().expect("a plan snapshot is a string");
        let plan: PersistedDivePlanSnapshot = disk_names::from_disk_str(raw).expect("parses");
        let written = disk_names::to_disk_string(&plan).expect("serialize");
        assert_eq!(deep(&written), deep(raw));
    }
}
