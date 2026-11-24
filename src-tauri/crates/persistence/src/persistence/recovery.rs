//! What is left of the SQLite `.stalx` layer: the recovery dialog's legacy
//! half and the payloads the app shares across both formats.
//!
//! Old SQLite files are read only through the frozen 0.6.0 reader
//! (`stalagmight-legacy`) and the migration ([`super::migrate`]); version 2
//! files through [`super::stalx_v2`]. What stays here:
//!
//! - 0.6.0's "Attempt Repair" on a legacy file ([`attempt_repair`]: REINDEX
//!   and VACUUM behind a numbered backup, then a quick check) and "Export
//!   Salvageable Data" ([`export_salvageable`]: each table read on its own,
//!   then the whole project through the frozen reader, best effort);
//! - the dialog payloads ([`BackupInfo`], [`RepairResult`], [`SalvagedData`])
//!   and the raw-data view's page ([`RawTableData`]);
//! - [`ResolvedWarning`], an ignored warning as the app keeps it;
//! - [`same_file_on_disk`], what the backup list and a restore mean by "the
//!   project itself".

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OpenFlags};

use crate::models::NewerSchema;

/// The newest legacy SQLite schema: what the frozen 0.6.0 reader opens. A
/// legacy file stamped above it was written by a build this one cannot read.
pub const SCHEMA_VERSION: i32 = stalagmight_legacy::SUPPORTED_SCHEMA_VERSION;

pub use stalagmight_legacy::persistence::RefusedSchemaObject;

/// A stringified page of rows from a raw table.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct RawTableData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: u32,
}

/// A backup the recovery dialog offers.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub path: String,
    pub size_bytes: u64,
    /// When the file was last written, in UTC with milliseconds
    /// (`2026-11-21T02:50:05.334Z`, one fixed width).
    pub modified_at: String,
    /// What the file is, so the dialog can say it in words.
    pub kind: BackupKind,
}

/// What a backup the recovery dialog offers is (`list_project_backups`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    /// The 0.6.0 (SQLite) original the first version 2 write over it kept in
    /// the app-data backups folder.
    PreUpgrade,
    /// `<project>.stalx.pre-restore`: the file as it was before the last
    /// restore. Restoring it undoes that restore.
    PreRestore,
    /// `<project>.stalx.backup` or `.backup.N` beside the project: the copy
    /// Attempt Repair keeps of the damaged file (and 0.6.0 kept).
    Backup,
    /// `<project>.stalx.tmp`: a complete file a save wrote but never put in
    /// the project's place.
    InterruptedSave,
}

/// Result of attempting to repair a corrupt database.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "camelCase")]
pub struct RepairResult {
    pub success: bool,
    pub backup_path: String,
    pub message: String,
}

/// Data salvaged from a corrupt database.
#[derive(Debug, Clone, Default, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "camelCase")]
pub struct SalvagedData {
    pub project_name: Option<String>,
    pub project_location: Option<String>,
    pub line_count: usize,
    pub shot_count: usize,
    pub merge_count: usize,
    /// Lines as JSON (for display/re-import)
    pub lines_json: Option<String>,
    /// Full project as JSON (if loadable despite corruption)
    pub full_project_json: Option<String>,
    /// Errors encountered while reading individual tables
    pub errors: Vec<String>,
}

/// Minimal line info for salvage display.
#[derive(Debug, Clone, serde::Serialize)]
struct SalvagedLine {
    id: u32,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<String>,
    color: String,
}

/// An ignored ("resolved") warning: the shot it was raised against and when
/// the user ignored it. Matched by `(shot_uuid, warning_code)` when the shot
/// has a uuid (always, in a version 2 file), else by the legacy
/// `(line_id, shot_id, warning_code)` key. See
/// `AppState::compute_project_warnings`.
#[derive(Debug, Clone)]
pub struct ResolvedWarning {
    pub line_id: u32,
    pub shot_id: u32,
    pub warning_code: String,
    pub resolved_at: chrono::DateTime<Utc>,
    pub resolved_comment: Option<String>,
    /// Stable uuid of the suppressed shot (issue #57); `None` only for rows a
    /// pre-v8 legacy file stored without one.
    pub shot_uuid: Option<String>,
}

/// Attempt to repair a corrupt legacy SQLite file in place with REINDEX and
/// VACUUM (0.6.0's repair).
///
/// Refuses a file a newer schema wrote (#649), then always creates a backup
/// first (`path.stalx.backup`, with numeric suffixes so no backup is
/// overwritten). Returns `(backup_path, integrity_ok)`.
pub fn attempt_repair(path: &Path) -> Result<(PathBuf, bool)> {
    tracing::info!("Attempting repair of: {:?}", path);

    check_not_newer_at(path)?;

    let backup_path = create_numbered_backup(path)?;
    tracing::info!("Created backup at: {:?}", backup_path);

    let conn = Connection::open(path).context("Failed to open database for repair")?;
    if let Err(e) = conn.execute_batch("REINDEX;") {
        tracing::warn!(error = %e, "REINDEX failed, continuing with VACUUM");
    }
    // VACUUM rebuilds the entire database from the readable pages.
    conn.execute_batch("VACUUM;")
        .context("VACUUM failed - database may be too damaged to repair")?;

    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .unwrap_or_else(|_| "error".to_string());
    let ok = check == "ok";
    if ok {
        tracing::info!("Repair succeeded - integrity check passed");
    } else {
        tracing::error!(result = %check, "Repair completed but integrity check still fails");
    }
    Ok((backup_path, ok))
}

/// Export whatever can be read from a (possibly corrupt) legacy SQLite file.
///
/// Each table is read on its own, read-only and without the integrity check,
/// so a failure in one table does not prevent salvaging the others. The full
/// project comes from the frozen 0.6.0 reader, which runs its own checks: on
/// a file it refuses, the error is listed instead.
pub fn export_salvageable(path: &Path) -> Result<SalvagedData> {
    tracing::info!("Opening database for recovery at: {:?}", path);
    if !path.exists() {
        anyhow::bail!("File does not exist");
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("Failed to open database for recovery")?;
    let mut salvaged = SalvagedData::default();

    match try_read_project_metadata(&conn) {
        Ok(Some((name, location))) => {
            salvaged.project_name = Some(name);
            salvaged.project_location = location;
        }
        Ok(None) => salvaged.errors.push("Project table is empty".into()),
        Err(e) => salvaged
            .errors
            .push(format!("Failed to read project metadata: {e}")),
    }
    match try_read_lines(&conn) {
        Ok(lines) => {
            salvaged.line_count = lines.len();
            salvaged.lines_json = serde_json::to_string(&lines).ok();
        }
        Err(e) => salvaged.errors.push(format!("Failed to read lines: {e}")),
    }
    match try_count(&conn, "shots") {
        Ok(count) => salvaged.shot_count = count,
        Err(e) => salvaged.errors.push(format!("Failed to read shots: {e}")),
    }
    match try_count(&conn, "station_merges") {
        Ok(count) => salvaged.merge_count = count,
        Err(e) => salvaged
            .errors
            .push(format!("Failed to read station merges: {e}")),
    }
    drop(conn);

    let opts = stalagmight_legacy::OpenOptions {
        with_adjusted: false,
        ..Default::default()
    };
    match stalagmight_legacy::open_path(path, &opts) {
        Ok(survey) => salvaged.full_project_json = serde_json::to_string(&survey.project).ok(),
        Err(e) => salvaged
            .errors
            .push(format!("Failed to load full project: {e}")),
    }
    Ok(salvaged)
}

/// The same file on disk: the same canonical path, or (on Unix) the same inode,
/// which also catches a hard link under another name. What the backup list and
/// a restore (#720) mean by "the project itself".
pub fn same_file_on_disk(a: &Path, b: &Path) -> bool {
    if let (Ok(ca), Ok(cb)) = (a.canonicalize(), b.canonicalize()) {
        if ca == cb {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return ma.dev() == mb.dev() && ma.ino() == mb.ino();
        }
    }
    false
}

/// Refuse a legacy file a newer schema than [`SCHEMA_VERSION`] wrote. A file
/// that does not open, or whose schema the frozen guard refuses, is left to
/// the repair itself to report.
fn check_not_newer_at(path: &Path) -> std::result::Result<(), NewerSchema> {
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Ok(());
    };
    if stalagmight_legacy::persistence::check_schema(&conn).is_err() {
        return Ok(());
    }
    let found = file_schema_version(&conn);
    if found > SCHEMA_VERSION {
        return Err(NewerSchema {
            file: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            found,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// The schema version a legacy file was written at: `PRAGMA user_version`,
/// or, when that is 0 (files from before the pragma), the
/// `project.schema_version` column. A value that cannot be read counts as 0.
/// Run only after the schema guard: it reads `pragma_table_info`, which a
/// hostile file could otherwise shadow.
fn file_schema_version(conn: &Connection) -> i32 {
    let stamped: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);
    if stamped != 0 {
        return stamped;
    }
    let has_column = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('project') WHERE name = 'schema_version'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .is_ok_and(|count| count > 0);
    if !has_column {
        return 0;
    }
    conn.query_row("SELECT schema_version FROM project LIMIT 1", [], |row| {
        Ok(match row.get_ref(0)? {
            ValueRef::Integer(v) => i32::try_from(v).unwrap_or(0),
            _ => 0,
        })
    })
    .unwrap_or(0)
}

/// Copy `path` to `path.stalx.backup`, else the first free
/// `path.stalx.backup.N` (never overwrites a backup).
fn create_numbered_backup(path: &Path) -> Result<PathBuf> {
    let base = path.with_extension("stalx.backup");
    if !base.exists() {
        std::fs::copy(path, &base)?;
        return Ok(base);
    }
    for i in 1..100 {
        let numbered = path.with_extension(format!("stalx.backup.{i}"));
        if !numbered.exists() {
            std::fs::copy(path, &numbered)?;
            return Ok(numbered);
        }
    }
    anyhow::bail!("Too many existing backups (max 100)")
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![name],
        |row| row.get(0),
    )?;
    Ok(exists > 0)
}

fn try_read_project_metadata(conn: &Connection) -> Result<Option<(String, Option<String>)>> {
    if !table_exists(conn, "project")? {
        return Ok(None);
    }
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))?;
    if count == 0 {
        return Ok(None);
    }
    let (name, location): (String, Option<String>) =
        conn.query_row("SELECT name, location FROM project LIMIT 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    Ok(Some((name, location)))
}

fn try_read_lines(conn: &Connection) -> Result<Vec<SalvagedLine>> {
    if !table_exists(conn, "sections")? {
        return Ok(Vec::new());
    }
    let mut stmt =
        conn.prepare("SELECT id, name, surveyor, date, color FROM sections ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        Ok(SalvagedLine {
            id: row.get(0)?,
            name: row.get(1)?,
            surveyor: row.get(2)?,
            date: row.get(3)?,
            color: row.get(4)?,
        })
    })?;
    let mut lines = Vec::new();
    for row in rows {
        lines.push(row?);
    }
    Ok(lines)
}

/// Rows of `table` (a fixed name, never user input), 0 when it is missing.
fn try_count(conn: &Connection, table: &str) -> Result<usize> {
    if !table_exists(conn, table)? {
        return Ok(0);
    }
    let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })?;
    Ok(usize::try_from(count).unwrap_or(0))
}
