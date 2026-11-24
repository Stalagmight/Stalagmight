//! The 0.6.0 open path, run on a private copy.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::models::config::ClosureAdjustmentConfig;
use crate::models::{
    AdjustmentFailureReport, AdjustmentReport, ClassifyScope, LineId, SkippedClosure,
};
use crate::persistence::{refused_schema_object, Database, SCHEMA_VERSION};
use crate::survey::{positions_of, HistoryPresence, LegacyExtras, LegacySurvey, LoadReport};
use crate::{LegacyError, LegacyProject};

/// The newest `.stalx` schema version the frozen reader knows.
pub const SUPPORTED_SCHEMA_VERSION: i32 = SCHEMA_VERSION;

/// Default cap on the input size (the file plus its journal), in bytes.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;

/// Options for [`open_path`] and [`open_bytes`].
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// Inputs larger than this are refused with [`LegacyError::TooLarge`]
    /// before anything is copied. Also caps any single value SQLite produces.
    pub max_file_bytes: u64,
    /// The closure-adjustment config the Closures Computed View solves with
    /// (a user setting in the app, `Config::closure_adjustment`).
    pub closure_adjustment: ClosureAdjustmentConfig,
    /// Compute [`LegacySurvey::adjusted_positions`] (the least-squares solve;
    /// the costly step on a large survey).
    pub with_adjusted: bool,
    /// Directory the private copy goes in; `None` is the system temp dir.
    pub temp_dir: Option<PathBuf>,
    /// Interrupts any SQLite statement still running this long after the
    /// load started (the frozen untrusted open's guard). The steps after the
    /// load are not bounded by it.
    pub sqlite_deadline: Duration,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            closure_adjustment: ClosureAdjustmentConfig::default(),
            with_adjusted: true,
            temp_dir: None,
            sqlite_deadline: Duration::from_hours(1),
        }
    }
}

/// The sidecars SQLite reads beside a database, by name. A hot `-journal`
/// (DELETE journal mode, the app's) is rolled back on the copy; a `-wal`
/// (a file some other tool left in WAL mode) is read through. `-shm` is
/// rebuilt by SQLite and never copied.
const SIDECARS: [&str; 2] = ["-journal", "-wal"];

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Open a legacy `.stalx` at `path`. The file and any `-journal` / `-wal`
/// beside it are copied to a private temp directory first; nothing is ever
/// written to the originals.
pub fn open_path(path: &Path, opts: &OpenOptions) -> Result<LegacySurvey, LegacyError> {
    let dir = private_dir(opts)?;
    let copy = dir.path().join("project.stalx");
    let mut budget = opts.max_file_bytes;
    budget = copy_capped(path, &copy, budget, opts.max_file_bytes)?;
    for suffix in SIDECARS {
        let sidecar = sidecar_of(path, suffix);
        if sidecar.is_file() {
            budget = copy_capped(
                &sidecar,
                &sidecar_of(&copy, suffix),
                budget,
                opts.max_file_bytes,
            )?;
        }
    }
    let mut survey = open_copy(&copy, opts)?;
    survey.project.file_path = Some(path.to_path_buf());
    Ok(survey)
}

/// Open a legacy `.stalx` held in memory (no sidecars).
pub fn open_bytes(bytes: &[u8], opts: &OpenOptions) -> Result<LegacySurvey, LegacyError> {
    if bytes.len() as u64 > opts.max_file_bytes {
        return Err(LegacyError::TooLarge {
            max: opts.max_file_bytes,
        });
    }
    let dir = private_dir(opts)?;
    let copy = dir.path().join("project.stalx");
    std::fs::File::create(&copy)?.write_all(bytes)?;
    open_copy(&copy, opts)
}

fn sidecar_of(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Stream at most `budget + 1` bytes of `from` into `to`; refuse when the
/// budget is exceeded. Returns the budget left.
fn copy_capped(from: &Path, to: &Path, budget: u64, max: u64) -> Result<u64, LegacyError> {
    let written = std::io::copy(
        &mut std::fs::File::open(from)?.take(budget.saturating_add(1)),
        &mut std::fs::File::create(to)?,
    )?;
    if written > budget {
        return Err(LegacyError::TooLarge { max });
    }
    Ok(budget - written)
}

fn private_dir(opts: &OpenOptions) -> std::io::Result<tempfile::TempDir> {
    let builder = {
        let mut b = tempfile::Builder::new();
        b.prefix("stalx-legacy-");
        b
    };
    match &opts.temp_dir {
        Some(dir) => builder.tempdir_in(dir),
        None => builder.tempdir(),
    }
}

/// The 0.6.0 production open (`AppState::do_load_project_full`), step for
/// step, on the private copy. The temp dir outlives every connection opened
/// here.
fn open_copy(copy: &Path, opts: &OpenOptions) -> Result<LegacySurvey, LegacyError> {
    sniff(copy)?;
    let max_value_bytes = i32::try_from(opts.max_file_bytes).unwrap_or(i32::MAX);
    let (schema_version, history) = inspect(copy, max_value_bytes)?;
    if schema_version > SCHEMA_VERSION {
        return Err(LegacyError::NewerSchema {
            found: schema_version,
            supported: SCHEMA_VERSION,
        });
    }

    let db = Database::open_existing_untrusted(copy, max_value_bytes, opts.sqlite_deadline)
        .map_err(|e| LegacyError::from_load(&e))?;
    let mut project = db.load_project().map_err(|e| LegacyError::from_load(&e))?;
    let loaded_modified_at = project.modified_at;
    let stored_merge_times: HashSet<DateTime<Utc>> = project
        .station_merges
        .iter()
        .map(|merge| merge.merged_at)
        .collect();
    let heal_started = Utc::now();
    let validation_stored = db
        .load_project_validation()
        .map_err(|e| LegacyError::from_load(&e))?
        .is_some();

    // The ignores' uuid backfill keys on the uuids the FILE holds, so the
    // map is taken before any uuid is filled in (as the app does).
    let stored_uuids = project.shot_uuid_by_position();
    // The app's `ensure_shot_uuids` mints random v4 ids here; derive them
    // instead, so every open of a file agrees (see the crate docs).
    let derived_shot_uuids = derive_missing_shot_uuids(&mut project);
    if let Err(e) = db.backfill_resolved_warning_uuids(&stored_uuids) {
        tracing::warn!(error = %e, "legacy open: resolved-warning uuid backfill failed");
    }
    project.assign_closure_ids();
    let (healed_offsets, renamed) = project.redrive_offsets_after_load_reporting();
    let unrecognised_shot_kinds = db.unrecognised_shot_kinds().unwrap_or_default();
    if !unrecognised_shot_kinds.is_empty() {
        // The app writes the fail-closed Reference substitution through; the
        // loaded project already holds it. Only the private copy changes.
        if let Err(e) = db.repair_unrecognised_shot_kinds() {
            tracing::warn!(error = %e, "legacy open: shot kind repair failed on the copy");
        }
    }
    let kind_classification_pending = db.is_kind_classification_pending();
    let reclassified_shots = if kind_classification_pending {
        project.classify_virtual_shots(&ClassifyScope::WholeProject)
    } else {
        0
    };

    let stations = project.compute_station_coordinates();
    let mut integrity = project.check_structural_integrity();
    integrity.extend(project.check_merge_alignment());

    let extras = LegacyExtras {
        display_settings: db
            .load_display_settings()
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        resolved_warnings: db
            .load_resolved_warnings_verbatim()
            .map_err(|e| LegacyError::from_load(&e))?,
        speleo_link: db
            .load_speleo_link()
            .map_err(|e| LegacyError::from_load(&e))?,
        speleo_undo_chain: db
            .load_speleo_undo_chain()
            .map_err(|e| LegacyError::from_load(&e))?,
        speleo_undo_watermark: db
            .load_speleo_undo_watermark()
            .map_err(|e| LegacyError::from_load(&e))?,
    };
    drop(db);

    let (adjusted_positions, adjustment_failures) = if opts.with_adjusted {
        // As `AppState::get_adjusted_stations`: the by-value solve on a clone.
        let adjusted = project
            .clone()
            .into_adjusted_stations(&opts.closure_adjustment);
        (Some(positions_of(&adjusted.stations)), adjusted.failures)
    } else {
        (None, Vec::new())
    };
    let adjustment_report = AdjustmentReport {
        skipped_closures: project
            .closures_across_one_shot()
            .iter()
            .map(SkippedClosure::from)
            .collect(),
        failures: adjustment_failures
            .iter()
            .map(AdjustmentFailureReport::from)
            .collect(),
        far_gps_fixes: project.far_gps_fixes(),
    };
    let component_anchors = project.component_anchors();

    // The wall clock the heal stamped: the edit time back to the file's, and
    // a merge the load created (an orphan closure promoted) dated by it too.
    let touched_modified_at = project.modified_at != loaded_modified_at;
    project.modified_at = loaded_modified_at;
    let mut merges_created = 0;
    for merge in project.station_merges.iter_mut() {
        if merge.merged_at >= heal_started && !stored_merge_times.contains(&merge.merged_at) {
            merge.merged_at = loaded_modified_at;
            merges_created += 1;
        }
    }
    Ok(LegacySurvey {
        raw_positions: positions_of(&stations),
        adjusted_positions,
        adjustment_failures,
        adjustment_report,
        component_anchors,
        extras,
        history,
        schema_version,
        load: LoadReport {
            validation_stored,
            derived_shot_uuids,
            renamed_shot_uuids: renamed
                .iter()
                .map(|(line, old, new)| (line, old.to_owned(), new.to_owned()))
                .collect(),
            healed_offsets,
            unrecognised_shot_kinds,
            kind_classification_pending,
            reclassified_shots,
            integrity,
            touched_modified_at,
            merges_created,
        },
        project,
    })
}

fn sniff(copy: &Path) -> Result<(), LegacyError> {
    let mut header = [0u8; 16];
    let mut file = std::fs::File::open(copy)?;
    let mut read = 0;
    while read < header.len() {
        let n = file.read(&mut header[read..])?;
        if n == 0 {
            return Err(LegacyError::NotSqlite);
        }
        read += n;
    }
    if &header == SQLITE_HEADER {
        Ok(())
    } else {
        Err(LegacyError::NotSqlite)
    }
}

/// Refuse schema objects that would run file-chosen SQL, and read the
/// stamped schema version and the history row counts, before anything
/// migrates the copy. Read-write, so a hot journal is rolled back first, as
/// the app's own open would.
fn inspect(copy: &Path, max_value_bytes: i32) -> Result<(i32, HistoryPresence), LegacyError> {
    use rusqlite::config::DbConfig;
    use rusqlite::limits::Limit;

    let sql = |e: rusqlite::Error| LegacyError::from_sqlite(&e);
    let conn = rusqlite::Connection::open_with_flags(
        copy,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sql)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(sql)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
        .map_err(sql)?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, max_value_bytes)
        .map_err(sql)?;
    conn.execute_batch("PRAGMA mmap_size = 0; PRAGMA cell_size_check = ON;")
        .map_err(sql)?;

    if let Some(refused) = refused_schema_object(&conn).map_err(sql)? {
        return Err(LegacyError::RefusedSchema(refused));
    }

    let history = HistoryPresence {
        undo_rows: count_rows(&conn, "undo_stack").map_err(sql)?,
        redo_rows: count_rows(&conn, "redo_stack").map_err(sql)?,
    };

    let stamped: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sql)?;
    if stamped < 0 {
        return Err(LegacyError::Corrupt(format!(
            "negative schema version {stamped}"
        )));
    }
    if stamped > 0 {
        return Ok((stamped, history));
    }
    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('project') WHERE name = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .map_err(sql)?;
    if has_column == 0 {
        return Ok((0, history));
    }
    let claimed: Option<i32> = conn
        .query_row("SELECT schema_version FROM project LIMIT 1", [], |row| {
            row.get(0)
        })
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
        .map_err(sql)?;
    Ok((claimed.unwrap_or(0).max(0), history))
}

/// Rows in `table`, or 0 when the file has no such table. `table` is one of
/// this crate's fixed names, never file content.
fn count_rows(conn: &rusqlite::Connection, table: &str) -> rusqlite::Result<u64> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Ok(0);
    }
    let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
        row.get(0)
    })?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// The namespace `stalx-reader` derives missing shot uuids in; shared so the
/// two readers agree on every derived id.
const DERIVED_UUID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6f1d_2c3e_8b4a_4e57_9c1d_5a7e_0b3f_2d91);

/// Fill a missing (or empty) shot uuid, and replace the later of two equal
/// uuids in one line, with a uuid derived from the shot's place and stored
/// id, so the same file always opens to the same ids. `stalx-reader`'s rule,
/// in place of the app's random `ensure_shot_uuids` (and of the random
/// re-mint `ensure_unique_shot_uuids` would make in the re-drive).
fn derive_missing_shot_uuids(project: &mut LegacyProject) -> BTreeSet<(LineId, String)> {
    let mut derived = BTreeSet::new();
    for line in &mut project.lines {
        let mut seen: HashSet<String> = HashSet::new();
        for (i, shot) in line.shots.iter_mut().enumerate() {
            let keep = shot
                .uuid
                .as_ref()
                .is_some_and(|u| !u.is_empty() && seen.insert(u.clone()));
            if keep {
                continue;
            }
            let name = format!("{}/{i}/{}", line.id, shot.uuid.as_deref().unwrap_or(""));
            // The namespace is public, so a file can hold a uuid equal to the
            // one derived here; step past it rather than reuse it.
            let id = (0..u32::MAX)
                .map(|n| match n {
                    0 => uuid::Uuid::new_v5(&DERIVED_UUID_NAMESPACE, name.as_bytes()),
                    n => uuid::Uuid::new_v5(
                        &DERIVED_UUID_NAMESPACE,
                        format!("{name}#{n}").as_bytes(),
                    ),
                })
                .map(|id| id.to_string())
                .find(|id| seen.insert(id.clone()))
                .unwrap_or_default();
            shot.uuid = Some(id.clone());
            derived.insert((line.id, id));
        }
    }
    derived
}
