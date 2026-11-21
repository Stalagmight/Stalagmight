//! Read-only reader for Stalagmight `.stalx` project files (#593).
//!
//! Built for servers that store `.stalx` uploads and need the survey inside
//! them, e.g. to generate a map. It never writes to the source: the bytes are
//! copied to a private temporary file and read from there.
//!
//! **Two formats.** The copy is sniffed ([`FileFormat`]):
//! - a JSON `.stalx` (format version 2, `{"format":"stalx","version":...`;
//!   whitespace between those tokens is tolerated) is decoded by the app's own
//!   decoder, which checks the format, version, conventions and the survey's
//!   invariants;
//! - an SQLite `.stalx` (the format before version 2) is opened by the frozen
//!   copy of the 0.6.0 app's reader and load pipeline (integrity check, the
//!   forward schema migrations, foreign-key check, the load's heals), then
//!   converted to the station-graph model exactly as the app converts an old
//!   file when it opens it.
//!
//! Anything else is refused as [`InvalidKind::NotSqlite`] ("not a `.stalx` in
//! either format"). Running the app's migrations is what makes old files read
//! correctly: some change stored meaning (v12 flips the pitch sign
//! convention), so a raw read of an old file would be silently wrong.
//!
//! The input is treated as hostile. Before the SQLite load runs, a file is
//! refused if it defines a view, trigger, virtual table, generated column,
//! expression or partial index, a non-literal column default, or any CHECK
//! constraint other than the app's own `CHECK (id = 1)`: each would evaluate an
//! expression the file chose, and the forward migrations write to the copy. It
//! is also refused when a table holds more rows than [`CountLimits`] allows,
//! counted before anything is loaded. Every connection to the copy runs in
//! defensive mode with `trusted_schema` off, a cap on any value SQLite produces,
//! no memory-mapping, cell-size checks, and a deadline that interrupts any
//! statement still running past it. A JSON file is parsed with the same
//! [`CountLimits`] (refused as soon as a list passes its cap), a nesting cap and
//! the size cap.
//!
//! **Bounds.** One deadline covers the read from the first statement to the
//! result, at two granularities: inside SQLite a statement is interrupted
//! mid-run; every other step (the SQLite load's post-load normalisation, the
//! conversion to the station-graph model, the JSON decode, optional station
//! positions) is plain Rust, and the deadline is checked between them, so a
//! step that has started runs to completion. How long such a step can run is
//! set by the app code it calls; the known super-linear one left is the
//! 0.6.0 offset cascade in an SQLite file's load normalisation (#602). Station
//! positions come from the station-graph solver, linear in the station count.
//! Memory is bounded only by the input size and [`CountLimits`]. SQLite's own
//! temporary files go to the process temp dir (`SQLITE_TMPDIR`), not
//! [`ReadOptions::temp_dir`]. A server should run reads in a worker it can
//! kill: stack overflow and allocation failure abort the process, which no
//! error type can contain.
//!
//! Files written by a newer SQLite schema than this crate knows are refused
//! with [`ReadError::NewerSchema`], JSON files of a newer major format version
//! with [`ReadError::NewerFormat`]; upgrade the crate to read them. A JSON file
//! of this major version that uses features or survey fields this crate does
//! not know is read, and says so in [`Survey::newer_content`]; one whose survey
//! read without those features breaks the format's rules is refused as
//! [`InvalidKind::NewerFeatures`], never as corrupt.
//!
//! ```no_run
//! let survey = stalx_reader::read_file(std::path::Path::new("project.stalx"))?;
//! for section in &survey.sections {
//!     println!("{:?}: {} shots", section.name, section.shots.len());
//! }
//! # Ok::<(), stalx_reader::ReadError>(())
//! ```

mod json;
mod sqlite;
mod survey;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use stalagmight_persistence::persistence::stalx_v2::{sniff, Format};

pub use survey::{
    CorrectionSource, Direction, FileFormat, LoopClosure, North, Person, Section, Shot, ShotKind,
    StationComment, StationId, StationMerge, StationName, StationPosition, Survey, ZeroDatum,
};

/// Default cap on the input size, in bytes.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Default deadline for the whole read.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);

/// The newest SQLite `.stalx` schema version this crate reads (the last one:
/// newer files are JSON).
pub const SUPPORTED_SCHEMA_VERSION: i32 = stalagmight_legacy::SUPPORTED_SCHEMA_VERSION;

/// The newest major version of the JSON `.stalx` format this crate reads.
pub const SUPPORTED_FORMAT_VERSION: u64 = stalagmight_persistence::persistence::stalx_v2::VERSION;

/// Options for [`read_file_with`] and [`read_bytes`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ReadOptions {
    /// Inputs larger than this are refused before anything is copied. Also caps
    /// any single string or blob SQLite produces while loading.
    pub max_file_bytes: u64,
    /// Time allowed for the whole read, from the start of the load. A SQLite
    /// statement still running at the deadline is interrupted (checked every
    /// 1,000 VM steps); otherwise it is checked between steps (see the crate
    /// docs). Either way the read fails with [`InvalidKind::Deadline`].
    pub deadline: Duration,
    /// Also compute measured (unadjusted) station positions into [`Survey::stations`].
    pub with_stations: bool,
    /// Caps on what a file may hold, checked before (SQLite) or while (JSON)
    /// it is loaded, and again right after.
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
/// For an SQLite file, rows are counted in the file before it is loaded, so
/// the caps also bound the memory the load uses. A JSON file is refused as
/// soon as one of its lists passes a cap; the caps map onto its records as
/// each field says (the ones marked "JSON only" have no SQLite table). They do
/// not make every step fast: see the crate docs' **Bounds**. The defaults fit
/// the largest real surveys; a server should set caps that fit its own
/// deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CountLimits {
    /// Sections (survey lines). JSON: `lines`.
    pub sections: usize,
    pub shots: usize,
    /// Station merges (SQLite rows; after the read, the derived junction
    /// pairs of [`Survey::station_merges`]).
    pub station_merges: usize,
    pub loop_closures: usize,
    /// Station names. Dense on a DMP-imported survey (one per station), and a
    /// row may name no station at all, so the table is capped on its own rather
    /// than through the shots it would resolve against.
    pub station_names: usize,
    /// Per-shot survey credit rows (`shot_credits`, at most one per shot). Keyed
    /// by shot uuid, so a row may name no shot: capped on its own.
    pub shot_credits: usize,
    /// Distinct credited people (`credit_people`). JSON: people over every
    /// credit object.
    pub credit_people: usize,
    /// Zero datum GPS fixes and accuracies (`zero_datum_extras`, one row per
    /// datum). A row may name no station, so the table is capped on its own.
    /// JSON: `datums`.
    pub zero_datum_extras: usize,
    /// Station IDs (`station_ids`, one row per station, like
    /// `station_names`). A row may name no station: capped on its own. JSON:
    /// `stations`.
    pub station_ids: usize,
    /// Sections, named groupings of lines (`cave_sections`). Nothing bounds
    /// them by the lines, so the table is capped on its own. JSON: `sections`.
    pub cave_sections: usize,
    /// Line memberships of Sections (`cave_section_lines`, at most one per
    /// line - `sections` holds the lines). JSON: a line's `section` field,
    /// bounded by `sections`.
    pub cave_section_lines: usize,
    /// JSON only: station aliases (ids merged into another station).
    pub aliases: usize,
    /// JSON only: legacy per-line hand moves.
    pub hand_moves: usize,
    /// JSON only: placements of datum-less pieces.
    pub placements: usize,
    /// JSON only: ignored validator warnings.
    pub ignored_warnings: usize,
    /// JSON only: file and line import hashes together.
    pub import_hashes: usize,
    /// JSON only: unknown members and keys a newer writer added.
    pub preserved_content: usize,
}

impl Default for CountLimits {
    fn default() -> Self {
        let json = stalagmight_persistence::persistence::stalx_v2::CountLimits::default();
        Self {
            sections: 100_000,
            shots: 2_000_000,
            station_merges: 200_000,
            loop_closures: 50_000,
            // One per station: at most every shot's end plus every section start.
            station_names: 2_100_000,
            shot_credits: 2_000_000,
            credit_people: 2_000_000,
            // At most one datum per station, like `station_names`.
            zero_datum_extras: 2_100_000,
            // One per station, like `station_names`.
            station_ids: 2_100_000,
            // No more groups than lines, and one membership per line.
            cave_sections: 100_000,
            cave_section_lines: 100_000,
            // The app's own decoder caps for the JSON-only records.
            aliases: json.aliases,
            hand_moves: json.hand_moves,
            placements: json.placements,
            ignored_warnings: json.ignored_warnings,
            import_hashes: json.import_hashes,
            preserved_content: json.preserved,
        }
    }
}

/// One builder per cap: `CountLimits::default().shots(1_000)`.
macro_rules! count_limit_setters {
    ($($field:ident),* $(,)?) => {
        impl CountLimits {
            $(
                #[must_use]
                pub fn $field(mut self, max: usize) -> Self {
                    self.$field = max;
                    self
                }
            )*
        }
    };
}

count_limit_setters!(
    sections,
    shots,
    station_merges,
    loop_closures,
    station_names,
    shot_credits,
    credit_people,
    zero_datum_extras,
    station_ids,
    cave_sections,
    cave_section_lines,
    aliases,
    hand_moves,
    placements,
    ignored_warnings,
    import_hashes,
    preserved_content,
);

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
    /// [`CountLimits::zero_datum_extras`].
    ZeroDatumExtras,
    /// [`CountLimits::station_ids`].
    StationIds,
    /// [`CountLimits::cave_sections`].
    CaveSections,
    /// [`CountLimits::cave_section_lines`].
    CaveSectionLines,
    /// [`CountLimits::aliases`].
    Aliases,
    /// [`CountLimits::hand_moves`].
    HandMoves,
    /// [`CountLimits::placements`].
    Placements,
    /// [`CountLimits::ignored_warnings`].
    IgnoredWarnings,
    /// [`CountLimits::import_hashes`].
    ImportHashes,
    /// [`CountLimits::preserved_content`].
    PreservedContent,
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
            Self::ZeroDatumExtras => "zero datum fixes",
            Self::StationIds => "station ids",
            Self::CaveSections => "line groups (sections)",
            Self::CaveSectionLines => "line group memberships",
            Self::Aliases => "station aliases",
            Self::HandMoves => "hand moves",
            Self::Placements => "placements",
            Self::IgnoredWarnings => "ignored warnings",
            Self::ImportHashes => "import hashes",
            Self::PreservedContent => "unknown members",
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
/// trouble on the reading side) and [`ReadError::NewerSchema`] /
/// [`ReadError::NewerFormat`] (retry after upgrading the crate). The exception
/// is [`InvalidKind::Deadline`], which measures wall-clock time: a file near the
/// limit can pass on a less-loaded worker. Retry it at most once, or treat it as
/// "too complex".
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReadError {
    #[error("input is over the {max}-byte limit")]
    TooLarge { max: u64 },
    /// One of the [`CountLimits`] was exceeded; `what` names it. For a JSON
    /// file `count` is `max + 1`: the decoder stops at the first record over
    /// the cap, so the file holds at least that many.
    #[error("file has {count} {what}, over the limit of {max}")]
    TooComplex {
        what: Limit,
        count: usize,
        max: usize,
    },
    /// An SQLite `.stalx` written by a newer schema than this crate knows.
    #[error("file uses .stalx schema v{found}; this reader supports up to v{supported}")]
    NewerSchema { found: i32, supported: i32 },
    /// A JSON `.stalx` of a newer major format version than this crate knows.
    #[error("file uses .stalx format version {found}; this reader supports up to {supported}")]
    NewerFormat { found: u64, supported: u64 },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Not a readable `.stalx`. Branch on `kind`; `detail` can quote content
    /// from the file (SQLite error context, a JSON path or value), so treat it
    /// as untrusted: log it, don't echo it to other users.
    #[error("not a readable .stalx file ({kind:?}): {detail}")]
    Invalid { kind: InvalidKind, detail: String },
    /// A bug in this crate or the app code it runs (a panic was contained, or
    /// the conversion of an old file produced an inconsistent survey). The
    /// file may be fine; report it, don't blame the uploader.
    #[error("internal error while reading: {0}")]
    Internal(String),
}

/// Why a file is not a readable `.stalx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvalidKind {
    /// Not a `.stalx` in either format: neither an SQLite database nor a JSON
    /// `.stalx` (an empty input included). The name predates the JSON format.
    NotSqlite,
    /// Corrupt: an SQLite file that failed the integrity check, has a
    /// malformed schema or an impossible schema version; a JSON file that is
    /// malformed, truncated, declares other conventions or a version no
    /// Stalagmight wrote, or holds an inconsistent survey (a dangling
    /// reference, a loop of shots, a repeated id...).
    Corrupt,
    /// The SQLite file defines a schema object that evaluates file-chosen SQL
    /// (see the crate docs).
    RefusedSchemaObject,
    /// A statement was still running at the deadline, or a step ended past it.
    Deadline,
    /// SQLite, but not a Stalagmight project (no project data).
    NoProjectData,
    /// A stored value no survey can have: a zero datum with a non-finite
    /// latitude/longitude or one outside [-90, 90] / [-180, 180], or (SQLite)
    /// ids the station-graph model cannot hold (a line id out of range or
    /// used twice, no Station ID left).
    InvalidValue,
    /// Anything else the SQLite load path rejected.
    Other,
    /// A JSON file that requires features this crate does not know, and read
    /// without them its survey breaks the format's rules: made with a newer
    /// Stalagmight, not damaged; upgrade the crate to read it.
    NewerFeatures,
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
/// straight into the private copy, so the input is never held in memory (a
/// JSON file is read into memory from the copy to decode it).
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

/// How many leading bytes the format sniff reads. The JSON prefix is 28 bytes
/// in canonical form; the rest is room for a pretty-printed file's whitespace.
const SNIFF_BYTES: u64 = 4096;

/// Read the private copy. The caller keeps the temp dir alive across this call;
/// every connection to the copy is dropped before it returns.
fn read_copy(copy: &Path, options: &ReadOptions) -> Result<Survey, ReadError> {
    // ONE clock for the whole read: each SQLite connection gets only what is left,
    // and every later phase checks it before starting.
    let expires = Instant::now() + options.deadline;
    let mut head = Vec::new();
    std::fs::File::open(copy)?
        .take(SNIFF_BYTES)
        .read_to_end(&mut head)?;
    match sniff(&head) {
        Format::Legacy => sqlite::read(copy, options, expires),
        Format::V2 => json::read(copy, options, expires),
        Format::Unknown => Err(ReadError::invalid(
            InvalidKind::NotSqlite,
            "neither an SQLite database nor a JSON .stalx",
        )),
    }
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

/// The test hook that stands in for a slow or panicking step right after the
/// load (both formats).
fn after_load() {
    #[cfg(test)]
    tests::AFTER_LOAD.with(|hook| hook.borrow()());
}

/// What [`check_counts`] compares against the caps once a file is loaded.
struct LoadedCounts {
    sections: usize,
    shots: usize,
    station_merges: usize,
    loop_closures: usize,
}

/// Refuse a loaded survey over the caps, before the steps that scale with it.
fn check_counts(counts: &LoadedCounts, limits: CountLimits) -> Result<(), ReadError> {
    for (what, count, max) in [
        (Limit::Sections, counts.sections, limits.sections),
        (Limit::Shots, counts.shots, limits.shots),
        (
            Limit::StationMerges,
            counts.station_merges,
            limits.station_merges,
        ),
        (
            Limit::LoopClosures,
            counts.loop_closures,
            limits.loop_closures,
        ),
    ] {
        if count > max {
            return Err(ReadError::TooComplex { what, count, max });
        }
    }
    Ok(())
}

/// Refuse a zero datum no survey can have, before any geometry uses it.
/// `datums` yields `(latitude, longitude, where)`; `where` names it in the
/// error.
fn validate_datums(datums: impl IntoIterator<Item = (f64, f64, String)>) -> Result<(), ReadError> {
    for (lat, lon, at) in datums {
        // A range never contains NaN or an infinity, so this also refuses those.
        if !((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
            return Err(ReadError::invalid(
                InvalidKind::InvalidValue,
                format!("zero datum on {at}: latitude/longitude out of range"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_v2;
