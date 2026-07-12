use anyhow::Result;
use chrono::Utc;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::Connection;
use std::io::{Read as _, Write as _};

// Items below are used only by the sibling `tests` module via `use super::*`;
// production code in this residual file no longer references them after the
// `impl Database` split into child modules, so gate them to the test build.
#[cfg(test)]
use crate::models::{
    ClosureStrategy, GpsCoordinates, LoopClosure, Project, SectionData, StationKey, StationMerge,
    SurveyDirection, ZeroDatum,
};
#[cfg(test)]
use chrono::NaiveDate;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;

/// Schema version for migrations
///
/// v8 adds a nullable `shot_uuid` column to `resolved_warnings` (issue #57):
/// "Ignore" suppression is keyed by the shot's stable UUID so it survives shot
/// inserts/deletes/reorders. Backfilled lazily from the legacy
/// `(section_id, shot_id)` key — no rewrite of existing rows at migration time.
///
/// v9 adds the additive single-row `speleo_link` table (SpeleoDB hand-off
/// identity + dirty bit, #189). No existing column or row is touched;
/// forward-looking only — no backfill. The table is created in BOTH
/// `init_schema` and `migrate_schema`, so legacy v8 files gain it on
/// `open_existing`. `is_remote` is DERIVED from `EXISTS(speleo_link)`, never
/// stored. See `PRESERVED_SPELEOLINK_COLUMNS` (the carry-forward fence) for the
/// columns every future bump must preserve.
///
/// v10 adds the additive `speleo_link.last_edit_at` column (RFC-3339 UTC of the
/// most recent accepted edit; #193 guardrail-7 push-start comparison). Added in
/// BOTH `init_schema` and `migrate_schema` (a conditional `ALTER` for existing v9
/// tables, mirroring the shot-uuid pattern); carried forward by the fence.
///
/// v11 adds the `speleo_link.last_verify_at` nullable column (RFC-3339 UTC of
/// the last verification; `#193`). Added in both `init_schema` and
/// `migrate_schema` (conditional `ALTER`); carried forward by the fence.
///
/// v12 adds the `markers` table (survey markers placed along shots) and the
/// `next_marker_id` column on `project` (monotonic ID counter). Both appear in
/// `init_schema` and `migrate_schema`.
///
/// v13 adds `shot_uuid TEXT` to `markers` so markers survive shot deletions.
/// Shot IDs are reconstructed positionally on every load, so they cannot serve
/// as stable foreign keys; the shot UUID is the only identity that persists.
/// New markers always write `shot_uuid`; on load, UUID takes priority over the
/// stored `shot_id` (legacy markers without a UUID fall back to `shot_id`).
const SCHEMA_VERSION: i32 = 13;

/// Schema version at which persisted undo/redo payloads became safe to replay.
///
/// v7 changed the `ShotUpdates` JSON wire format: nullable fields now use a
/// `double_option` deserializer where an explicit `null` means "clear the
/// field" (`Some(None)`), while pre-v7 writers serialized every UNTOUCHED
/// nullable field as an explicit `null`. Replaying a pre-v7 undo/redo entry
/// under the new semantics would silently clear pitch/LRUD/comment/marker/
/// temperature on the target shot. Legacy JSON cannot distinguish "untouched"
/// from "restore-to-empty", so files below this version get their persisted
/// undo/redo stacks dropped on open (see `drop_legacy_undo_history`).
///
/// Deliberately a separate constant from `SCHEMA_VERSION`: future schema bumps
/// (v8+) must NOT re-drop undo history unless the undo payload format changes
/// again, in which case bump this constant too.
const UNDO_FORMAT_SAFE_VERSION: i32 = 7;

/// Maximum number of rows a single raw-table page request may return.
pub const MAX_RAW_PAGE_ROWS: u32 = 10_000;

/// Render a blob value as a size placeholder, e.g. `<blob 4.2 KB>`.
fn format_blob_placeholder(len: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let len = len as f64;
    if len < KB {
        format!("<blob {len:.0} B>")
    } else if len < MB {
        format!("<blob {:.1} KB>", len / KB)
    } else if len < GB {
        format!("<blob {:.1} MB>", len / MB)
    } else {
        format!("<blob {:.1} GB>", len / GB)
    }
}

fn compress_json(json: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // write_all and finish are infallible on Vec<u8>
    encoder
        .write_all(json.as_bytes())
        .and_then(|()| encoder.finish())
        .unwrap_or_else(|_| json.as_bytes().to_vec())
}

fn decompress_json(data: &[u8]) -> Result<String> {
    // Gzip magic bytes: 0x1f 0x8b
    if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
        let mut decoder = GzDecoder::new(data);
        let mut s = String::new();
        decoder.read_to_string(&mut s)?;
        Ok(s)
    } else {
        // Backward compat: old TEXT entries (or CAST-ed BLOB) are plain UTF-8
        Ok(String::from_utf8_lossy(data).into_owned())
    }
}

/// Database operations for projects
pub struct Database {
    conn: Connection,
}

mod integrity;
mod open;
mod read;
mod repair;
mod schema;
mod settings;
mod undo;
mod warnings;
mod write;

/// A stringified page of rows from a raw sqlite table.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct RawTableData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: u32,
}

/// Information about a backup file.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub path: String,
    pub size_bytes: u64,
    pub modified_at: String,
}

/// Result of attempting to repair a corrupt database.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RepairResult {
    pub success: bool,
    pub backup_path: String,
    pub message: String,
}

/// Data salvaged from a corrupt database.
#[derive(Debug, Clone, Default, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SalvagedData {
    pub project_name: Option<String>,
    pub project_location: Option<String>,
    pub section_count: usize,
    pub shot_count: usize,
    pub merge_count: usize,
    /// Sections as JSON (for display/re-import)
    pub sections_json: Option<String>,
    /// Full project as JSON (if loadable despite corruption)
    pub full_project_json: Option<String>,
    /// Errors encountered while reading individual tables
    pub errors: Vec<String>,
}

/// Minimal section info for salvage display.
#[derive(Debug, Clone, serde::Serialize)]
struct SalvagedSection {
    id: u32,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<String>,
    color: String,
}

/// Resolved warning record
#[derive(Debug, Clone)]
pub struct ResolvedWarning {
    pub section_id: u32,
    pub shot_id: u32,
    pub warning_code: String,
    pub resolved_at: chrono::DateTime<Utc>,
    pub resolved_comment: Option<String>,
    /// Stable UUID of the suppressed shot (issue #57). `Some` for all warnings
    /// raised against shots that have a UUID (the normal case); `None` only for
    /// pre-v8 rows or shots loaded from pre-v6 .stalx files without a UUID yet.
    /// The suppression is matched by `(shot_uuid, warning_code)` when present,
    /// falling back to the legacy `(section_id, shot_id, warning_code)` key
    /// otherwise. See `AppState::compute_project_warnings`.
    pub shot_uuid: Option<String>,
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::float_cmp,
    clippy::panic
)]
mod tests;
