//! Read-only reader for Stalagmight `.stalx` project files (#593).
//!
//! Built for servers that store `.stalx` uploads and need the survey inside
//! them, e.g. to generate a map. It never writes to the source: the bytes are
//! copied to a private temporary file, the app's own load path runs on that
//! copy (integrity check, forward migrations, foreign-key check), and the
//! result is converted into this crate's [`Survey`].
//!
//! Running the app's migrations is what makes old files read correctly: some
//! migrations change stored meaning (v12 flips the pitch sign convention), so a
//! raw read of an old file would be silently wrong.
//!
//! The input is treated as hostile. Before the app's load path runs, a file is
//! refused if it defines a view, trigger, virtual table, generated column,
//! expression or partial index, a non-literal column default, or any CHECK
//! constraint other than the app's own `CHECK (id = 1)`: each would evaluate an
//! expression the file chose, and the forward migrations write to the copy. It
//! is also refused when a table holds more rows than [`CountLimits`] allows,
//! counted before anything is loaded. Every connection to the copy runs in
//! defensive mode with `trusted_schema` off, a cap on any value SQLite produces,
//! no memory-mapping, cell-size checks, and a deadline that interrupts any
//! statement still running past it.
//!
//! **Bounds.** One deadline covers the read from the first SQLite statement to
//! the result, at two granularities: inside SQLite a statement is interrupted
//! mid-run; the steps after the load (the app's post-load normalisation, the
//! conversion, optional station positions) are plain Rust, and the deadline is
//! checked between them, so a step that has started runs to completion. How
//! long such a step can run is set by the app code it calls; the known
//! super-linear one left is the offset cascade in the normalisation (#602).
//! Station positions come from the app's positions-only path, near-linear in
//! the station count whatever the merge-group sizes (#596). Memory is bounded
//! only by the input size and [`CountLimits`]. SQLite's own temporary files go
//! to the process temp dir (`SQLITE_TMPDIR`), not [`ReadOptions::temp_dir`].
//! A server should run reads in a worker it can kill: stack overflow and
//! allocation failure abort the process, which no error type can contain.
//!
//! Files written by a newer schema than this crate knows are refused with
//! [`ReadError::NewerSchema`]; upgrade the crate to read them.
//!
//! ```no_run
//! let survey = stalx_reader::read_file(std::path::Path::new("project.stalx"))?;
//! for section in &survey.sections {
//!     println!("{:?}: {} shots", section.name, section.shots.len());
//! }
//! # Ok::<(), stalx_reader::ReadError>(())
//! ```

mod survey;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use stalagmight_models::ClassifyScope;
use stalagmight_persistence::persistence::{refused_schema_object, Database, SCHEMA_VERSION};

pub use survey::{
    CorrectionSource, Direction, LoopClosure, North, Person, Section, Shot, ShotKind,
    StationComment, StationId, StationMerge, StationName, StationPosition, Survey, ZeroDatum,
};

/// Default cap on the input size, in bytes.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Default deadline for the whole read.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);

/// The newest `.stalx` schema version this crate reads.
pub const SUPPORTED_SCHEMA_VERSION: i32 = SCHEMA_VERSION;

/// Options for [`read_file_with`] and [`read_bytes`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ReadOptions {
    /// Inputs larger than this are refused before anything is copied. Also caps
    /// any single string or blob SQLite produces while loading.
    pub max_file_bytes: u64,
    /// Time allowed for the whole read, from the start of the load. A SQLite
    /// statement still running at the deadline is interrupted (checked every
    /// 1,000 VM steps); after the load, it is checked between steps (see the
    /// crate docs). Either way the read fails with [`InvalidKind::Deadline`].
    pub deadline: Duration,
    /// Also compute measured (unadjusted) station positions into [`Survey::stations`].
    pub with_stations: bool,
    /// Caps on the counts the post-load steps scale with, checked right after the
    /// load and before any of them run.
    pub count_limits: CountLimits,
    /// Directory the private copy goes in; `None` is the system temp directory.
    pub temp_dir: Option<PathBuf>,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            deadline: DEFAULT_DEADLINE,
            with_stations: false,
            count_limits: CountLimits::default(),
            temp_dir: None,
        }
    }
}

/// Caps on what a file may contain, refused with [`ReadError::TooComplex`].
///
/// Rows are counted in the file before it is loaded, so the caps also bound the
/// memory the load uses. They do not make every step fast: see the crate docs'
/// **Bounds**. The defaults fit the largest real surveys; a server should set
/// caps that fit its own deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CountLimits {
    pub sections: usize,
    pub shots: usize,
    pub station_merges: usize,
    pub loop_closures: usize,
    /// Station names. Dense on a DMP-imported survey (one per station), and a
    /// row may name no station at all, so the table is capped on its own rather
    /// than through the shots it would resolve against.
    pub station_names: usize,
    /// Per-shot survey credit rows (`shot_credits`, at most one per shot). Keyed
    /// by shot uuid, so a row may name no shot: capped on its own.
    pub shot_credits: usize,
    /// Distinct credited people (`credit_people`).
    pub credit_people: usize,
}

impl Default for CountLimits {
    fn default() -> Self {
        Self {
            sections: 100_000,
            shots: 2_000_000,
            station_merges: 200_000,
            loop_closures: 50_000,
            // One per station: at most every shot's end plus every section start.
            station_names: 2_100_000,
            shot_credits: 2_000_000,
            credit_people: 2_000_000,
        }
    }
}

impl CountLimits {
    #[must_use]
    pub fn sections(mut self, max: usize) -> Self {
        self.sections = max;
        self
    }

    #[must_use]
    pub fn shots(mut self, max: usize) -> Self {
        self.shots = max;
        self
    }

    #[must_use]
    pub fn station_merges(mut self, max: usize) -> Self {
        self.station_merges = max;
        self
    }

    #[must_use]
    pub fn loop_closures(mut self, max: usize) -> Self {
        self.loop_closures = max;
        self
    }

    #[must_use]
    pub fn station_names(mut self, max: usize) -> Self {
        self.station_names = max;
        self
    }

    #[must_use]
    pub fn shot_credits(mut self, max: usize) -> Self {
        self.shot_credits = max;
        self
    }

    #[must_use]
    pub fn credit_people(mut self, max: usize) -> Self {
        self.credit_people = max;
        self
    }
}

/// Which [`CountLimits`] cap a file exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Limit {
    /// [`CountLimits::sections`].
    Sections,
    /// [`CountLimits::shots`].
    Shots,
    /// [`CountLimits::station_merges`].
    StationMerges,
    /// [`CountLimits::loop_closures`].
    LoopClosures,
    /// [`CountLimits::station_names`].
    StationNames,
    /// [`CountLimits::shot_credits`].
    ShotCredits,
    /// [`CountLimits::credit_people`].
    CreditPeople,
}

impl std::fmt::Display for Limit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Sections => "sections",
            Self::Shots => "shots",
            Self::StationMerges => "station merges",
            Self::LoopClosures => "loop closures",
            Self::StationNames => "station names",
            Self::ShotCredits => "shot credits",
            Self::CreditPeople => "credited people",
        })
    }
}

impl ReadOptions {
    #[must_use]
    pub fn max_file_bytes(mut self, bytes: u64) -> Self {
        self.max_file_bytes = bytes;
        self
    }

    #[must_use]
    pub fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    #[must_use]
    pub fn with_stations(mut self, on: bool) -> Self {
        self.with_stations = on;
        self
    }

    #[must_use]
    pub fn count_limits(mut self, limits: CountLimits) -> Self {
        self.count_limits = limits;
        self
    }

    /// Where the private copy is made (a fresh directory inside it, removed
    /// when the read ends). Default: the system temp directory.
    #[must_use]
    pub fn temp_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.temp_dir = Some(dir.into());
        self
    }
}

/// Why a read failed.
///
/// Every variant except one is deterministic for a given file and crate version,
/// so retrying as-is cannot help, apart from [`ReadError::Io`] (disk or temp-dir
/// trouble on the reading side) and [`ReadError::NewerSchema`] (retry after
/// upgrading the crate). The exception is [`InvalidKind::Deadline`], which
/// measures wall-clock time: a file near the limit can pass on a less-loaded
/// worker. Retry it at most once, or treat it as "too complex".
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReadError {
    #[error("input is over the {max}-byte limit")]
    TooLarge { max: u64 },
    /// One of the [`CountLimits`] was exceeded; `what` names it.
    #[error("file has {count} {what}, over the limit of {max}")]
    TooComplex {
        what: Limit,
        count: usize,
        max: usize,
    },
    #[error("file uses .stalx schema v{found}; this reader supports up to v{supported}")]
    NewerSchema { found: i32, supported: i32 },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Not a readable `.stalx`. Branch on `kind`; `detail` can quote content
    /// from the file (SQLite error context), so treat it as untrusted: log it,
    /// don't echo it to other users.
    #[error("not a readable .stalx file ({kind:?}): {detail}")]
    Invalid { kind: InvalidKind, detail: String },
    /// A bug in this crate or the app code it runs (a panic was contained). The
    /// file may be fine; report it, don't blame the uploader.
    #[error("internal error while reading: {0}")]
    Internal(String),
}

/// Why a file is not a readable `.stalx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvalidKind {
    /// Not an SQLite database at all.
    NotSqlite,
    /// SQLite, but corrupt (failed the integrity check, a malformed schema, or
    /// an impossible schema version).
    Corrupt,
    /// The file defines a schema object that evaluates file-chosen SQL (see the
    /// crate docs).
    RefusedSchemaObject,
    /// A statement was still running at the deadline.
    Deadline,
    /// SQLite, but not a Stalagmight project (no project data).
    NoProjectData,
    /// A stored value no survey can have: a zero datum with a non-finite
    /// latitude/longitude or one outside [-90, 90] / [-180, 180].
    InvalidValue,
    /// Anything else the load path rejected.
    Other,
}

impl ReadError {
    fn invalid(kind: InvalidKind, detail: impl Into<String>) -> Self {
        Self::Invalid {
            kind,
            detail: detail.into(),
        }
    }
}

/// Read a `.stalx` file with default options.
pub fn read_file(path: &Path) -> Result<Survey, ReadError> {
    read_file_with(path, &ReadOptions::default())
}

/// Read a `.stalx` file. Streams at most `max_file_bytes + 1` bytes from `path`
/// straight into the private copy, so the input is never held in memory.
pub fn read_file_with(path: &Path, options: &ReadOptions) -> Result<Survey, ReadError> {
    contain_panics(|| {
        let dir = private_dir(options)?;
        let copy = dir.path().join("project.stalx");
        let written = std::io::copy(
            &mut std::fs::File::open(path)?.take(options.max_file_bytes.saturating_add(1)),
            &mut std::fs::File::create(&copy)?,
        )?;
        if written > options.max_file_bytes {
            return Err(ReadError::TooLarge {
                max: options.max_file_bytes,
            });
        }
        read_copy(&copy, options)
    })
}

/// Read a `.stalx` file held in memory (e.g. an upload).
pub fn read_bytes(bytes: &[u8], options: &ReadOptions) -> Result<Survey, ReadError> {
    contain_panics(|| {
        if bytes.len() as u64 > options.max_file_bytes {
            return Err(ReadError::TooLarge {
                max: options.max_file_bytes,
            });
        }
        let dir = private_dir(options)?;
        let copy = dir.path().join("project.stalx");
        std::fs::File::create(&copy)?.write_all(bytes)?;
        read_copy(&copy, options)
    })
}

/// A panic anywhere in the read becomes [`ReadError::Internal`] instead of
/// unwinding into the caller (through a Python binding it would surface as a
/// `BaseException` most task runners do not catch). Needs `panic = "unwind"`,
/// Rust's default; under `panic = "abort"` the process still dies.
fn contain_panics(read: impl FnOnce() -> Result<Survey, ReadError>) -> Result<Survey, ReadError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic with a non-string payload".to_owned());
        Err(ReadError::Internal(message))
    })
}

fn private_dir(options: &ReadOptions) -> std::io::Result<tempfile::TempDir> {
    match &options.temp_dir {
        Some(dir) => tempfile::Builder::new().prefix("stalx-").tempdir_in(dir),
        None => tempfile::Builder::new().prefix("stalx-").tempdir(),
    }
}

/// Read the private copy. The caller keeps the temp dir alive across this call;
/// every connection to the copy is dropped before it returns.
fn read_copy(copy: &Path, options: &ReadOptions) -> Result<Survey, ReadError> {
    // ONE clock for the whole read: each SQLite connection gets only what is left,
    // and every post-load phase checks it before starting.
    let expires = Instant::now() + options.deadline;
    let max_value_bytes = i32::try_from(options.max_file_bytes).unwrap_or(i32::MAX);
    let schema_version = inspect(
        copy,
        max_value_bytes,
        options.count_limits,
        remaining(expires)?,
    )?;
    if schema_version > SCHEMA_VERSION {
        return Err(ReadError::NewerSchema {
            found: schema_version,
            supported: SCHEMA_VERSION,
        });
    }

    let db = Database::open_existing_untrusted(copy, max_value_bytes, remaining(expires)?)
        .map_err(invalid)?;
    let mut project = db.load_project().map_err(invalid)?;
    // A probe interrupted by the deadline can swallow the error and report a
    // table absent, so check the clock before trusting what was loaded.
    remaining(expires)?;
    #[cfg(test)]
    tests::AFTER_LOAD.with(|hook| hook.borrow()());
    check_counts(&project, options.count_limits)?;
    validate_values(&project)?;
    // Before the app's steps below: the app repairs a missing or in-section
    // duplicate shot UUID with a RANDOM one, which would make the same file read
    // differently every time. Derive them instead, so re-reads agree.
    let derived_uuids = derive_missing_shot_uuids(&mut project);
    // The post-load steps of the app's own open (`project_load.rs`) that change
    // what this crate reports: ids for legacy closures, re-derived section
    // offsets (which also re-anchors refs and turns orphan closures into
    // merges), and the one-shot Jump/Reference classification an old file's
    // migration left pending. All in-memory; the copy is discarded.
    remaining(expires)?;
    project.assign_closure_ids();
    remaining(expires)?;
    // The re-drive also renames shot UUIDs held by more than one section (a
    // file saved before they were unique across the project), with the same
    // derived ids the app's own open gives them - so they are flagged derived.
    let (_, renamed) = project.redrive_offsets_after_load_reporting();
    let mut derived_uuids = derived_uuids;
    derived_uuids.extend(
        renamed
            .iter()
            .map(|(section, _, new)| (section, new.to_string())),
    );
    remaining(expires)?;
    if db.is_kind_classification_pending() {
        project.classify_virtual_shots(&ClassifyScope::WholeProject);
    }
    remaining(expires)?;
    Ok(Survey::from_project(
        &project,
        schema_version,
        options.with_stations,
        &derived_uuids,
    ))
}

/// Time left before `expires`, or [`InvalidKind::Deadline`] once it has passed.
/// Checked between phases, so a phase that has started runs to completion: the
/// read is bounded to the deadline plus one phase.
fn remaining(expires: Instant) -> Result<Duration, ReadError> {
    expires
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| ReadError::invalid(InvalidKind::Deadline, "deadline passed"))
}

/// Refuse a file whose tables hold more rows than `limits` allow, before the
/// load builds them in memory. A table the file lacks counts as empty.
fn check_row_counts(conn: &rusqlite::Connection, limits: CountLimits) -> Result<(), ReadError> {
    for (table, what, max) in [
        ("sections", Limit::Sections, limits.sections),
        ("shots", Limit::Shots, limits.shots),
        (
            "station_merges",
            Limit::StationMerges,
            limits.station_merges,
        ),
        ("loop_closures", Limit::LoopClosures, limits.loop_closures),
        ("station_names", Limit::StationNames, limits.station_names),
        ("shot_credits", Limit::ShotCredits, limits.shot_credits),
        ("credit_people", Limit::CreditPeople, limits.credit_people),
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(sql_invalid)?;
        if exists == 0 {
            continue;
        }
        // `table` is one of the fixed names above, never file content.
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .map_err(sql_invalid)?;
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        if count > max {
            return Err(ReadError::TooComplex { what, count, max });
        }
    }
    Ok(())
}

/// Refuse zero datums no survey can have, before any geometry uses them.
fn validate_values(project: &stalagmight_models::Project) -> Result<(), ReadError> {
    // Shot measurements need no check here: the loader already replaces a
    // non-finite one with 0, as the app shows it (`Shot::sanitize_non_finite`).
    let bad = |what: String| Err(ReadError::invalid(InvalidKind::InvalidValue, what));
    for datum in &project.zero_datums {
        let (lat, lon) = (datum.gps.latitude, datum.gps.longitude);
        // A range never contains NaN or an infinity, so this also refuses those.
        if !((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
            return bad(format!(
                "zero datum on section {}: latitude/longitude out of range",
                datum.station_ref.section_id
            ));
        }
    }
    Ok(())
}

/// An arbitrary fixed UUID; changing it changes every derived shot id.
const DERIVED_UUID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6f1d_2c3e_8b4a_4e57_9c1d_5a7e_0b3f_2d91);

/// Fill a missing shot UUID, and replace the later of two equal UUIDs in one
/// section, with a UUID derived from the shot's place and content, so the same
/// file always reads to the same ids. Returns the shots it filled, as
/// `(section id, derived uuid)`.
fn derive_missing_shot_uuids(
    project: &mut stalagmight_models::Project,
) -> std::collections::HashSet<(u32, String)> {
    let mut derived = std::collections::HashSet::new();
    for section in &mut project.sections {
        let mut seen = std::collections::HashSet::new();
        for (i, shot) in section.shots.iter_mut().enumerate() {
            let keep = shot.uuid.as_ref().is_some_and(|u| seen.insert(u.clone()));
            if keep {
                continue;
            }
            let name = format!("{}/{i}/{}", section.id, shot.uuid.as_deref().unwrap_or(""));
            // The namespace is public, so a file can hold a UUID equal to the one
            // derived here; step past it rather than reuse it.
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
            derived.insert((section.id, id));
        }
    }
    derived
}

/// Refuse schema objects that would run file-chosen SQL, and return the file's
/// schema version, both read before anything migrates the copy.
///
/// The version follows the app's own rule: `PRAGMA user_version`, or when that
/// is 0 (files from before the pragma), the `project.schema_version` column.
fn inspect(
    path: &Path,
    max_value_bytes: i32,
    limits: CountLimits,
    deadline: Duration,
) -> Result<i32, ReadError> {
    use rusqlite::config::DbConfig;
    use rusqlite::limits::Limit;

    let sql_err = sql_invalid;
    let mut conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(sql_err)?;
    // Same hardening as the load connection: this one parses the file's schema too.
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(sql_err)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
        .map_err(sql_err)?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, max_value_bytes)
        .map_err(sql_err)?;
    let expires = Instant::now() + deadline;
    conn.progress_handler(1_000, Some(move || Instant::now() > expires))
        .map_err(sql_err)?;
    conn.execute_batch("PRAGMA mmap_size = 0; PRAGMA cell_size_check = ON;")
        .map_err(sql_err)?;

    // The app's own hostile-schema check (`persistence::refused_schema_object`):
    // pragma-shadowing names, unexpected types, virtual tables, and any schema
    // expression the file chose (generated columns, expression or partial
    // indexes, expression defaults, CHECKs other than the app's).
    if let Some(refused) = refused_schema_object(&conn).map_err(sql_err)? {
        return Err(ReadError::invalid(
            InvalidKind::RefusedSchemaObject,
            refused.to_string(),
        ));
    }
    check_row_counts(&conn, limits)?;

    let stamped: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sql_err)?;
    if stamped < 0 {
        return Err(ReadError::invalid(
            InvalidKind::Corrupt,
            format!("negative schema version {stamped}"),
        ));
    }
    if stamped > 0 {
        return Ok(stamped);
    }
    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('project') WHERE name = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if has_column == 0 {
        return Ok(0);
    }
    let claimed: Option<i32> = conn
        .query_row("SELECT schema_version FROM project LIMIT 1", [], |row| {
            row.get(0)
        })
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
        .map_err(sql_err)?;
    Ok(claimed.unwrap_or(0).max(0))
}

fn check_counts(
    project: &stalagmight_models::Project,
    limits: CountLimits,
) -> Result<(), ReadError> {
    let shots = project.sections.iter().map(|s| s.shots.len()).sum();
    for (what, count, max) in [
        (Limit::Sections, project.sections.len(), limits.sections),
        (Limit::Shots, shots, limits.shots),
        (
            Limit::StationMerges,
            project.station_merges.len(),
            limits.station_merges,
        ),
        (
            Limit::LoopClosures,
            project.loop_closures.len(),
            limits.loop_closures,
        ),
    ] {
        if count > max {
            return Err(ReadError::TooComplex { what, count, max });
        }
    }
    Ok(())
}

/// Classify a load-path failure. SQLite result codes are read from the error
/// chain; the two app-level refusals are recognised by the exact messages the
/// persistence crate attaches (pinned by tests here).
fn invalid(e: anyhow::Error) -> ReadError {
    let detail = format!("{e:#}");
    let kind = e
        .chain()
        .find_map(|cause| cause.downcast_ref::<rusqlite::Error>())
        .and_then(sqlite_kind)
        .unwrap_or_else(|| {
            if detail.contains("Database file may be corrupted") {
                InvalidKind::Corrupt
            } else if detail.contains("does not contain a valid Stalagmight project") {
                InvalidKind::NoProjectData
            } else {
                InvalidKind::Other
            }
        });
    ReadError::invalid(kind, detail)
}

fn sqlite_kind(e: &rusqlite::Error) -> Option<InvalidKind> {
    use rusqlite::ErrorCode;
    match e.sqlite_error_code()? {
        ErrorCode::NotADatabase => Some(InvalidKind::NotSqlite),
        ErrorCode::DatabaseCorrupt => Some(InvalidKind::Corrupt),
        ErrorCode::OperationInterrupted => Some(InvalidKind::Deadline),
        _ => None,
    }
}

fn sql_invalid(e: rusqlite::Error) -> ReadError {
    let kind = sqlite_kind(&e).unwrap_or(InvalidKind::Other);
    ReadError::invalid(kind, e.to_string())
}

#[cfg(test)]
mod tests;
