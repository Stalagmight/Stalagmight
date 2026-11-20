//! The persisted JSON names of the Line concept.
//!
//! A Line was called a Section until the rename that introduced this module,
//! and several shapes persist serde names derived from the Rust identifiers:
//! the undo/redo rows of a `.stalx` (`command_json`, `undo_data_json`, and the
//! whole-project snapshots and `values_json` documents some `UndoData` variants
//! carry as JSON strings), the `held_gauges` project setting and the dive-plan
//! sidecar. Renaming the identifiers would have changed those names, so an older
//! build reading a file this build wrote would lose its undo history, its held
//! gauges (and with them the adjusted geometry) and its dive plans.
//!
//! The files therefore keep the old vocabulary. Code works in the new names and
//! crosses the disk boundary through [`to_disk`] / [`from_disk`], which rename
//! the object keys and enum tags listed below and nothing else - never a user
//! value. The pair is a bijection over the names it knows: a key the new
//! vocabulary does not use is left alone, so a file an older build wrote reads
//! back unchanged apart from the rename.
//!
//! Adding a persisted field or tag that contains `line`/`Line` means adding its
//! disk name here, and a NEW concept must not reuse a disk name (`sections`,
//! `section_id`, ...) as its own key - both are checked by `debug_assert!`s that
//! the test suites (which persist and reload a great deal of undo history)
//! exercise.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

/// Object keys: (name in code, name on disk).
const KEYS: &[(&str, &str)] = &[
    ("line_id", "section_id"),
    ("lines", "sections"),
    ("next_line_id", "next_section_id"),
    ("line_offsets", "section_offsets"),
    ("imported_line_hashes", "imported_section_hashes"),
    ("line_data", "section_data"),
    ("line_hashes", "section_hashes"),
    ("affected_lines", "affected_sections"),
    ("line_values", "section_values"),
    ("prev_line_offset", "prev_section_offset"),
    ("CreatedLineSnapshot", "CreatedSectionSnapshot"),
    // No longer an `UndoData` variant (the compact undo history keeps no
    // payload only a pre-compact history held, and such a history is dropped
    // on open before anything reads it). Kept so the pair stays a bijection
    // over every name a file in the wild can hold.
    ("LineSnapshot", "SectionSnapshot"),
    // A compact undo entry's `BeforePreview::Line` (the facts a Delete Line
    // preview reads), written in the old vocabulary like the rest of the row.
    // Like every pair here it renames the KEY wherever it occurs in a
    // translated document, not only in the preview. Safe as long as no other
    // persisted shape uses `Line` or `Section` as an object key: no project
    // map is keyed by a user string, and the other `Line`/`Section` enums are
    // internally tagged (a `type` value, handled by TAGS) or never persisted
    // through here. A new externally tagged `Line` or `Section` variant, or a
    // map keyed by user text, must not go through this translation.
    ("Line", "Section"),
    // Born after the rename (the Section commands, #327; the compact entry's
    // `BeforePreview::DeletedLines`), so no older build wrote another name:
    // they persist as they are.
    ("line_ids", "line_ids"),
    ("DeletedLines", "DeletedLines"),
];

/// Enum tags written as string values: (key that holds the tag, name in code,
/// name on disk). `type` is the tag of the internally tagged `Command` and
/// `CreditTargets`; `anchor` holds a `StationAnchor`, whose unit variant is a
/// bare string.
const TAGS: &[(&str, &str, &str)] = &[
    ("type", "create_line", "create_section"),
    ("type", "delete_line", "delete_section"),
    ("type", "update_line_metadata", "update_section_metadata"),
    ("type", "reverse_line", "reverse_section"),
    ("type", "move_line", "move_section"),
    ("type", "line", "section"),
    ("anchor", "LineStart", "SectionStart"),
    // Born after the rename (#327): persists as it is.
    (
        "type",
        "move_lines_to_cave_section",
        "move_lines_to_cave_section",
    ),
];

/// Keys whose string value is itself a persisted JSON document (a project
/// snapshot, or the previous values of an update), translated in place.
const NESTED_DOCUMENT_KEYS: &[&str] = &["ProjectSnapshot", "before", "after", "values_json"];

/// The key whose value is a `(LineId, String)` pair with a JSON document second.
const NESTED_PAIR_KEYS: &[&str] = &["line_values", "section_values"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    ToDisk,
    FromDisk,
}

/// Rename a value written by this build into the on-disk vocabulary.
pub fn to_disk(value: &mut Value) {
    translate(value, Direction::ToDisk);
}

/// Rename a value read from disk into this build's vocabulary.
pub fn from_disk(value: &mut Value) {
    translate(value, Direction::FromDisk);
}

/// Serialize `value` in the on-disk vocabulary.
///
/// # Errors
/// Whatever `serde_json` reports for `value`.
pub fn to_disk_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut json = serde_json::to_value(value)?;
    to_disk(&mut json);
    serde_json::to_string(&json)
}

/// Deserialize `text` written in the on-disk vocabulary.
///
/// # Errors
/// Whatever `serde_json` reports for `text`, or for the translated value.
pub fn from_disk_str<T: DeserializeOwned>(text: &str) -> serde_json::Result<T> {
    let mut json: Value = serde_json::from_str(text)?;
    from_disk(&mut json);
    serde_json::from_value(json)
}

/// Translate a JSON document held in a string; `None` (leave it as it is) for
/// anything that is not a parseable JSON object or array.
fn translate_document(text: &str, direction: Direction) -> Option<String> {
    let trimmed = text.trim_start();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let mut json: Value = serde_json::from_str(text).ok()?;
    translate(&mut json, direction);
    serde_json::to_string(&json).ok()
}

fn translate(value: &mut Value, direction: Direction) {
    match value {
        Value::Object(map) => translate_object(map, direction),
        Value::Array(items) => {
            for item in items {
                translate(item, direction);
            }
        }
        _ => {}
    }
}

fn rename_key(key: &str, direction: Direction) -> Option<&'static str> {
    KEYS.iter().find_map(|(code, disk)| match direction {
        Direction::ToDisk if *code == key => Some(*disk),
        Direction::FromDisk if *disk == key => Some(*code),
        _ => None,
    })
}

fn rename_tag(holder: &str, tag: &str, direction: Direction) -> Option<&'static str> {
    TAGS.iter().find_map(|(key, code, disk)| {
        if *key != holder {
            return None;
        }
        match direction {
            Direction::ToDisk if *code == tag => Some(*disk),
            Direction::FromDisk if *disk == tag => Some(*code),
            _ => None,
        }
    })
}

fn translate_object(map: &mut Map<String, Value>, direction: Direction) {
    let entries = std::mem::take(map);
    for (key, mut value) in entries {
        let new_key = match rename_key(&key, direction) {
            Some(renamed) => renamed.to_owned(),
            None => {
                if direction == Direction::ToDisk {
                    debug_assert!(
                        !is_disk_vocabulary(&key),
                        "disk_names: the key {key:?} is a disk name of the Line concept; \
                         give the new field its own disk name in KEYS"
                    );
                    debug_assert!(
                        !has_line_token(&key),
                        "disk_names: the key {key:?} names the Line concept but has no disk \
                         name; add it to KEYS"
                    );
                }
                key
            }
        };
        if let Value::String(tag) = &mut value {
            if let Some(renamed) = rename_tag(&new_key, tag, direction) {
                *tag = renamed.to_owned();
            } else if direction == Direction::ToDisk && new_key == "type" {
                debug_assert!(
                    !has_line_token(tag),
                    "disk_names: the tag {tag:?} names the Line concept but has no disk name; \
                     add it to TAGS"
                );
            }
        }
        if NESTED_DOCUMENT_KEYS.contains(&new_key.as_str()) {
            if let Value::String(text) = &mut value {
                if let Some(translated) = translate_document(text, direction) {
                    *text = translated;
                }
            }
        }
        if NESTED_PAIR_KEYS.contains(&new_key.as_str()) {
            if let Value::Array(items) = &mut value {
                for item in items.iter_mut() {
                    if let Value::String(text) = item {
                        if let Some(translated) = translate_document(text, direction) {
                            *text = translated;
                        }
                    }
                }
            }
        }
        translate(&mut value, direction);
        map.insert(new_key, value);
    }
}

fn is_disk_vocabulary(key: &str) -> bool {
    KEYS.iter().any(|(_, disk)| *disk == key)
}

/// Does `key` contain the word `line` as one of its snake_case or camelCase
/// parts (`line_id`, `lineIds`, `CreatedLineSnapshot`), not merely as letters
/// (`polyline`, `baseline`)?
fn has_line_token(key: &str) -> bool {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in key.chars() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if ch.is_uppercase() && previous_lower {
            parts.push(std::mem::take(&mut current));
        }
        previous_lower = ch.is_lowercase() || ch.is_ascii_digit();
        current.push(ch);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
        .iter()
        .any(|part| matches!(part.to_ascii_lowercase().as_str(), "line" | "lines"))
}
