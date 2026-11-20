//! Read-only browsing of the open project's file as saved (the data table's
//! "Raw data" view).
//!
//! The SQLite file had tables; a version 2 `.stalx` has members
//! (`docs/file-format.md`). Each member is shown as a table with one row per
//! record: one per station, line header, shot, datum, placement, closure,
//! Section, ignored warning, import hash and history entry, and one row for
//! each single-object member (`project`, `settings`, `counters`, `speleodb`,
//! `local`, `summary`) and for the file's own header (`file`: format, version,
//! solver, requires, conventions, generator). A shot row says which line it
//! is on and where. Values are shown as the file spells them: strings as
//! text, numbers in their shortest form, `null` as `NULL`, objects and arrays
//! as compact JSON (cut at [`MAX_CELL_CHARS`]).
//!
//! It always shows the LAST-SAVED state, read from disk (the frontend says
//! so while the project is dirty and refetches after a save). A legacy file is
//! still SQLite until its first save upgrades it, and a project never saved
//! has no file: both are refused with the reason.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use serde_json::{Map, Value};

use super::open_file::FileOrigin;
use super::AppState;
use crate::persistence::stalx_v2::{self, DecodeOptions, Format};
use crate::persistence::RawTableData;

/// Longest cell text shown; longer values end in an ellipsis and their size.
pub(super) const MAX_CELL_CHARS: usize = 500;

/// The file's own header members, shown as the `file` table.
const FILE_KEYS: [&str; 6] = [
    "format",
    "version",
    "solver",
    "requires",
    "conventions",
    "generator",
];

/// Table names in file order; `shots` follows `lines`, whose rows hold them.
const MEMBER_ORDER: [&str; 16] = [
    "summary",
    "project",
    "settings",
    "counters",
    "speleodb",
    "local",
    "sections",
    "stations",
    "lines",
    "shots",
    "datums",
    "placements",
    "closures",
    "ignored_warnings",
    "imports",
    "history",
];

/// The column order of each record kind as `docs/file-format.md` lists it;
/// keys it does not name follow, sorted.
fn preferred_columns(table: &str) -> &'static [&'static str] {
    match table {
        "summary" => &[
            "name", "lines", "stations", "shots", "length", "center", "bounds", "speleodb",
        ],
        "project" => &["name", "location", "created", "modified"],
        "settings" => &["validation", "surface_elevation", "azimuth"],
        "counters" => &["station", "closure", "section"],
        "speleodb" => &["endpoint", "project", "name", "undo_mode"],
        "local" => &["display", "speleodb"],
        "sections" => &["id", "name", "color"],
        "stations" => &["id", "name", "comment", "aliases", "external"],
        "lines" => &[
            "id",
            "name",
            "color",
            "direction",
            "quality",
            "section",
            "azimuth",
            "surveyor",
            "date",
            "credit",
            "start",
            "hand",
            "shots",
        ],
        "shots" => &[
            "line",
            "index",
            "uuid",
            "to",
            "length",
            "azimuth_in",
            "azimuth_out",
            "depth_in",
            "depth_out",
            "left",
            "right",
            "up",
            "down",
            "kind",
            "pitch_in",
            "pitch_out",
            "temperature",
            "marker",
            "comment",
            "credit",
        ],
        "datums" => &[
            "station",
            "via",
            "lat",
            "lon",
            "depth",
            "accuracy",
            "entrance",
            "correction",
        ],
        "placements" => &["station", "x", "y"],
        "closures" => &["id", "a", "a_via", "b", "b_via", "strategy"],
        "ignored_warnings" => &["shot", "code", "at", "comment"],
        "imports" => &["list", "position", "value"],
        "history" => &[
            "list", "position", "v", "t", "k", "label", "refs", "preview", "ops",
        ],
        "file" => &FILE_KEYS,
        _ => &[],
    }
}

/// The parsed file of the last read, keyed by what the file looked like then
/// (path, size, modification time), so paging through a large file parses it
/// once.
type Parsed = (
    PathBuf,
    u64,
    Option<std::time::SystemTime>,
    Arc<Map<String, Value>>,
);

static LAST_PARSED: Mutex<Option<Parsed>> = Mutex::new(None);

impl AppState {
    /// The raw tables of the open project's saved file (see the module docs),
    /// in file order.
    pub fn list_raw_table_names(&self) -> Result<Vec<String>> {
        let file = self.saved_file_members()?;
        let mut names = vec!["file".to_string()];
        for member in MEMBER_ORDER {
            let present = if member == "shots" {
                file.get("lines").is_some_and(|lines| {
                    lines
                        .as_array()
                        .is_some_and(|rows| rows.iter().any(Value::is_array))
                })
            } else {
                file.contains_key(member)
            };
            if present {
                names.push(member.to_string());
            }
        }
        // Members a newer writer added, after the known ones.
        names.extend(
            file.keys()
                .filter(|k| {
                    k.as_str() != "end"
                        && !FILE_KEYS.contains(&k.as_str())
                        && !MEMBER_ORDER.contains(&k.as_str())
                })
                .cloned(),
        );
        Ok(names)
    }

    /// A page of `table`: `limit` rows from `offset`, every value as text.
    pub fn read_raw_table(&self, table: &str, limit: u32, offset: u32) -> Result<RawTableData> {
        let file = self.saved_file_members()?;
        let rows = table_rows(&file, table)?;
        let mut seen = BTreeSet::new();
        for row in &rows {
            seen.extend(row.iter().map(|(k, _)| k.clone()));
        }
        let columns = ordered_columns(preferred_columns(table), seen);
        let total_rows = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let take = usize::try_from(limit).unwrap_or(usize::MAX);
        let page = rows
            .iter()
            .skip(start)
            .take(take)
            .map(|row| {
                columns
                    .iter()
                    .map(|column| {
                        row.iter()
                            .find(|(k, _)| k == column)
                            .map_or_else(String::new, |(_, v)| cell(v))
                    })
                    .collect()
            })
            .collect();
        Ok(RawTableData {
            columns,
            rows: page,
            total_rows,
        })
    }

    /// The top-level members of the open project's file as last saved.
    fn saved_file_members(&self) -> Result<Arc<Map<String, Value>>> {
        let path = self
            .open_file_path()
            .ok_or_else(|| anyhow!("No project file; save the project to browse raw data"))?;
        match self.with_file(|file| file.origin) {
            FileOrigin::V2 => {}
            FileOrigin::Legacy => bail!(
                "This project is still in the old file format until it is saved; save it to \
                 browse its raw data"
            ),
            FileOrigin::New => bail!("No project file yet; save the project to browse raw data"),
        }
        parsed_file(&path)
    }
}

/// The file at `path`, parsed (from the cache when it has not changed).
fn parsed_file(path: &Path) -> Result<Arc<Map<String, Value>>> {
    let meta =
        std::fs::metadata(path).map_err(|e| anyhow!("Could not read the project file: {e}"))?;
    let (len, modified) = (meta.len(), meta.modified().ok());
    {
        let cache = LAST_PARSED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((p, l, m, parsed)) = cache.as_ref() {
            if p == path && *l == len && *m == modified {
                return Ok(Arc::clone(parsed));
            }
        }
    }
    if len > DecodeOptions::default().max_file_bytes {
        bail!("The project file is too large to browse ({len} bytes)");
    }
    let bytes = std::fs::read(path).map_err(|e| anyhow!("Could not read the project file: {e}"))?;
    if stalx_v2::sniff(&bytes) != Format::V2 {
        bail!("The project file on disk is not a version 2 .stalx file");
    }
    let parsed: Map<String, Value> = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("The project file is not readable JSON: {e}"))?;
    let parsed = Arc::new(parsed);
    *LAST_PARSED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some((path.to_path_buf(), len, modified, Arc::clone(&parsed)));
    Ok(parsed)
}

/// One row: `(column, value)` in no particular order. Values are borrowed
/// from the parsed file, except the few the table adds (a line's shot count,
/// a shot's index, a history entry's list and position).
type Row<'a> = Vec<(String, Cow<'a, Value>)>;

fn borrowed<'a>(key: &str, value: &'a Value) -> (String, Cow<'a, Value>) {
    (key.to_string(), Cow::Borrowed(value))
}

fn owned<'a>(key: &str, value: impl Into<Value>) -> (String, Cow<'a, Value>) {
    (key.to_string(), Cow::Owned(value.into()))
}

/// The rows of `table` in the file.
fn table_rows<'a>(file: &'a Map<String, Value>, table: &str) -> Result<Vec<Row<'a>>> {
    let missing = || anyhow!("No such table: {table}");
    Ok(match table {
        "file" => vec![FILE_KEYS
            .iter()
            .filter_map(|k| file.get(*k).map(|v| borrowed(k, v)))
            .collect()],
        "stations" => array_of(file, table)?.iter().map(station_row).collect(),
        "lines" => line_rows(array_of(file, "lines")?),
        "shots" => shot_rows(array_of(file, "lines")?),
        "imports" | "history" => grouped_rows(file.get(table).ok_or_else(missing)?),
        _ => match file.get(table).ok_or_else(missing)? {
            Value::Array(records) => records.iter().map(record_row).collect(),
            other => vec![record_row(other)],
        },
    })
}

fn array_of<'a>(file: &'a Map<String, Value>, member: &str) -> Result<&'a [Value]> {
    file.get(member)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow!("No such table: {member}"))
}

/// An object's members as a row; any other value as one `value` column.
fn record_row(value: &Value) -> Row<'_> {
    match value {
        Value::Object(map) => map.iter().map(|(k, v)| borrowed(k, v)).collect(),
        other => vec![borrowed("value", other)],
    }
}

/// `[id, "name", {extras}?]`.
fn station_row(value: &Value) -> Row<'_> {
    let Some(fields) = value.as_array() else {
        return record_row(value);
    };
    let mut row = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        match (i, field) {
            (0, v) => row.push(borrowed("id", v)),
            (1, v) => row.push(borrowed("name", v)),
            (_, Value::Object(extras)) => row.extend(extras.iter().map(|(k, v)| borrowed(k, v))),
            (i, v) => row.push(borrowed(&format!("field {i}"), v)),
        }
    }
    row
}

/// The line headers, each with its shot count.
fn line_rows(lines: &[Value]) -> Vec<Row<'_>> {
    let mut rows: Vec<(Row<'_>, usize)> = Vec::new();
    for value in lines {
        if value.is_object() {
            rows.push((record_row(value), 0));
        } else if let Some((_, count)) = rows.last_mut() {
            *count += 1;
        }
    }
    rows.into_iter()
        .map(|(mut row, count)| {
            row.push(owned("shots", count));
            row
        })
        .collect()
}

/// `[uuid, to, length, az_in, az_out, depth_in, depth_out, left?, right?,
/// up?, down?, {extras}?]` under each line header, with the line id and the
/// shot's index on it.
fn shot_rows(lines: &[Value]) -> Vec<Row<'_>> {
    const POSITIONS: [&str; 11] = [
        "uuid",
        "to",
        "length",
        "azimuth_in",
        "azimuth_out",
        "depth_in",
        "depth_out",
        "left",
        "right",
        "up",
        "down",
    ];
    let mut rows = Vec::new();
    let mut line: Option<&Value> = None;
    let mut index = 0usize;
    for value in lines {
        if let Some(header) = value.as_object() {
            line = header.get("id");
            index = 0;
            continue;
        }
        let Some(fields) = value.as_array() else {
            continue;
        };
        let mut row: Row<'_> = Vec::new();
        if let Some(id) = line {
            row.push(borrowed("line", id));
        }
        row.push(owned("index", index));
        for (i, field) in fields.iter().enumerate() {
            match (POSITIONS.get(i), field) {
                (_, Value::Object(extras)) => {
                    row.extend(extras.iter().map(|(k, v)| borrowed(k, v)));
                }
                (Some(name), v) => row.push(borrowed(name, v)),
                (None, v) => row.push(borrowed(&format!("field {i}"), v)),
            }
        }
        rows.push(row);
        index += 1;
    }
    rows
}

/// `{"<list>": [...], ...}` (imports, history): one row per element, with the
/// list it is in and its position there.
fn grouped_rows(value: &Value) -> Vec<Row<'_>> {
    let Some(lists) = value.as_object() else {
        return vec![record_row(value)];
    };
    let mut rows = Vec::new();
    for (list, items) in lists {
        let Some(items) = items.as_array() else {
            rows.push(vec![owned("list", list.as_str()), borrowed("value", items)]);
            continue;
        };
        for (position, item) in items.iter().enumerate() {
            let mut row: Row<'_> = vec![owned("list", list.as_str()), owned("position", position)];
            match item {
                Value::Object(map) => row.extend(map.iter().map(|(k, v)| borrowed(k, v))),
                other => row.push(borrowed("value", other)),
            }
            rows.push(row);
        }
    }
    rows
}

/// The preferred columns that occur, then the others, sorted.
fn ordered_columns(preferred: &[&str], mut seen: BTreeSet<String>) -> Vec<String> {
    let mut columns: Vec<String> = preferred
        .iter()
        .filter(|c| seen.remove(**c))
        .map(|c| (*c).to_string())
        .collect();
    columns.extend(seen);
    columns
}

/// A value as the table shows it.
fn cell(value: &Value) -> String {
    let text = match value {
        Value::Null => "NULL".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    };
    if text.chars().count() <= MAX_CELL_CHARS {
        return text;
    }
    let cut: String = text.chars().take(MAX_CELL_CHARS).collect();
    format!("{cut}... ({} bytes)", text.len())
}
