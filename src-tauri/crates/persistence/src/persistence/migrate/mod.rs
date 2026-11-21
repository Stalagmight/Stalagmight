//! Migration of old SQLite `.stalx` files into the station-graph model
//! (plan `docs/plans/2026-11-20_station-graph-model.md`, section 7).
//!
//! The legacy file is opened by the frozen 0.6.0 reader
//! ([`stalagmight_legacy`]): exactly what the 0.6.0 app did on open, load
//! heal included. [`migrate`] converts that settled state:
//!
//! 1. **Stations** (7.2.1): a union-find over the legacy merges, taken in the
//!    order the old load cascade aligned them, then list order. Each group is
//!    one station. Self-merges and repeats are dropped; same-line merges and
//!    merges that would close a cycle become `Tie` closures at the FRONT of
//!    the closure list. The survivor is the member the map labelled (not a
//!    line's first station, then not its last, then line order, then index):
//!    its Station ID and name are kept, the others become aliases with their
//!    names; distinct non-empty comments are joined in member order with a
//!    blank line; names are made unique on every line of a junction.
//! 2. **Lines** (7.2.2): shots in order with their uuids (missing, overlong
//!    or repeated ones minted deterministically from line id and index),
//!    chains through the stations; ids, metadata and measurements as loaded.
//! 3. **Hand moves** (7.2.3): every legacy station offset, on its own line.
//! 4. **Closures** (7.2.4): ids, ends and strategies kept; a legacy aligned
//!    non-`Move` closure becomes a `Tie`. Every closure end and datum gets
//!    the `via` hint of the legacy member that held it.
//! 5. **Anchors** (7.2.5): a component holding a datum is placed by its first
//!    one, with a correction holding its station at its legacy position;
//!    any other component gets a `Placement` at its legacy gauge station
//!    (held gauge, else default gauge) at its legacy position.
//! 6. **Entrances** (7.2.6): an explicit override wherever the new default
//!    would differ from what 0.6.0 derived.
//! 7. **Verification** (7.2.7): raw positions and the Closures Computed View
//!    (with the least-squares config the legacy open used), see [`verify`].
//! 8. **History** (7.2.8): never migrated; [`Migrated::had_history`] says
//!    whether the file held any (the app shows its "history discarded"
//!    banner).
//! 9. **Per-file state** (7.2.9): [`MigratedExtras`].
//!
//! Every rule that fires is counted in the [`MigrationReport`]. The result
//! is held in memory; the original file is never written.

mod assemble;
mod convert;
mod ignores;
mod merges;
mod report;
mod slots;
mod stations;
pub mod verify;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod migrate_tests;

#[cfg(test)]
#[expect(
    clippy::print_stderr,
    reason = "test module: a failing seed prints its replay line, a sweep its coverage counts"
)]
mod migrate_property_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test module: panics on a failed assumption are the point; the runners print counts only"
)]
mod migrate_fixture_tests;

#[cfg(test)]
#[expect(
    clippy::panic,
    clippy::print_stderr,
    reason = "fuzz test: a failing seed prints its replay line, a sweep its coverage counts"
)]
mod migrate_fuzz_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod migrate_v2_roundtrip_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test helpers: a refused legacy build is a broken test setup"
)]
mod test_support;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stderr,
    reason = "test module: panics on a failed assumption are the point; the sweep prints class counts only"
)]
mod wire_equivalence_tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use stalagmight_legacy::{LegacySurvey, OpenOptions};

/// The frozen reader's refusal (re-exported so the app can say why).
pub use stalagmight_legacy::LegacyError;

use crate::models::config::ClosureAdjustmentConfig;
use crate::models::survey::{Project, Violation};
use crate::models::{LineId, SpeleoLink};

use super::stalx_v2::IgnoredWarning;

pub use report::{
    AdjustedCheck, AdjustedVerification, Axis, ClassStats, ExpectedClass, MigrationReport,
    Mismatch, Verification,
};

/// A migrated legacy project.
#[derive(Debug, Clone)]
pub struct Migrated {
    pub project: Project,
    pub extras: MigratedExtras,
    pub report: MigrationReport,
    /// Whether the legacy file held undo or redo history (dropped).
    pub had_history: bool,
}

/// The per-file state a legacy `.stalx` carries outside the survey model,
/// converted for the new file's `speleodb`, `local` and `ignored_warnings`
/// members. Import hashes, validation thresholds, surface elevation, the
/// default azimuth reference and Sections are on [`Project`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MigratedExtras {
    /// The SpeleoDB link (shared identity, dirty bit, undo mode).
    pub speleo_link: Option<SpeleoLink>,
    /// The Unwind chain position (#303).
    pub speleo_undo_chain: Option<String>,
    /// The inherited-history watermark (#303).
    pub speleo_undo_watermark: Option<String>,
    /// Display settings (per-machine view state), key -> value, verbatim.
    pub display_settings: BTreeMap<String, String>,
    /// Ignored warnings, keyed by the migrated shot uuids.
    pub ignored_warnings: Vec<IgnoredWarning>,
    /// Shots whose stored kind 0.6.0 could not read (#27, loaded as
    /// `Reference`), with the migrated survey's shot ids, for the app's
    /// "unreadable shot kind" notice.
    pub unrecognised_shot_kinds: Vec<crate::models::UnrecognisedShotKind>,
    /// Whether the file stored its own validation thresholds. When it did
    /// not, `project.settings.validation` is the 0.6.0 default and the app
    /// should seed the user's global thresholds instead (as 0.6.0 did).
    pub validation_stored: bool,
    /// Every slot whose station now goes by another name than 0.6.0 gave it
    /// there, in slot order: a junction has one name on all its lines (its
    /// survivor's, kept wherever a line knows it by an alias), and a true
    /// clash is lettered. Survey content for the conversion notice only,
    /// never logged (review findings D-migration-upgrade-8, UX-3, UX-1).
    pub names_changed: Vec<NameChange>,
    /// Record-orientation parity the legacy model kept for `.plnx` dive-plan
    /// pins (a single-neighbour pin reads which end of its shot it sits on).
    /// The station-graph model has no such field; carried so the dive-plan
    /// resolver can still read old pins.
    pub legacy_parity: LegacyParity,
}

/// One slot whose station's name changed in the conversion
/// ([`MigratedExtras::names_changed`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameChange {
    /// The slot's line, by the name the converted project gives it.
    pub line: String,
    /// The name 0.6.0 showed at that slot.
    pub from: String,
    /// The station's name now.
    pub to: String,
}

/// See [`MigratedExtras::legacy_parity`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyParity {
    /// Lines whose `reverse_parity` was set (reversed an odd number of times).
    pub reversed_lines: BTreeSet<LineId>,
    /// Shots (by migrated uuid) whose record parity runs against their line's.
    pub counter_parity_shots: BTreeSet<String>,
}

/// Why a legacy project could not be migrated.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// The legacy reader refused the file.
    #[error(transparent)]
    Legacy(#[from] LegacyError),
    /// A line id above the model's id space.
    #[error("line id {0} is out of range")]
    LineIdOutOfRange(LineId),
    /// Two lines share an id.
    #[error("line id {0} is used twice")]
    DuplicateLineId(LineId),
    /// No Station ID left to give a station without one.
    #[error("no station id is left")]
    StationIdsExhausted,
    /// The migrated model breaks an invariant (a migration bug).
    #[error("the migrated project is invalid: {}", .0.first().map(ToString::to_string).unwrap_or_default())]
    Invalid(Vec<Violation>),
}

impl MigrateError {
    /// A stable name for the error's class, safe to print for a private file
    /// (never the error text, which can quote the file).
    pub fn class(&self) -> &'static str {
        match self {
            Self::Legacy(LegacyError::NotSqlite) => "legacy-not-sqlite",
            Self::Legacy(LegacyError::NewerSchema { .. }) => "legacy-newer-schema",
            Self::Legacy(LegacyError::TooLarge { .. }) => "legacy-too-large",
            Self::Legacy(LegacyError::RefusedSchema(_)) => "legacy-refused-schema",
            Self::Legacy(LegacyError::Corrupt(_)) => "legacy-corrupt",
            Self::Legacy(LegacyError::Io(_)) => "legacy-io",
            Self::LineIdOutOfRange(_) => "line-id-out-of-range",
            Self::DuplicateLineId(_) => "duplicate-line-id",
            Self::StationIdsExhausted => "station-ids-exhausted",
            Self::Invalid(_) => "invalid",
        }
    }
}

/// Open a legacy `.stalx` at `path` (and any journal beside it, never
/// written) and migrate it. The project's `file_path` is `path`.
pub fn migrate_path(path: &Path) -> Result<Migrated, MigrateError> {
    migrate_path_with(path, &OpenOptions::default())
}

/// [`migrate_path`] with the legacy reader's options.
pub fn migrate_path_with(path: &Path, options: &OpenOptions) -> Result<Migrated, MigrateError> {
    let legacy = stalagmight_legacy::open_path(path, options)?;
    let mut migrated = migrate(
        &legacy,
        &convert::closure_adjustment(&options.closure_adjustment),
    )?;
    migrated.project.file_path = Some(path.to_path_buf());
    Ok(migrated)
}

/// [`migrate_path`] with the app's least-squares config: what the app's
/// open calls (the verification then compares the Computed View like for
/// like, G2).
pub fn migrate_path_for_app(
    path: &Path,
    adjustment: &ClosureAdjustmentConfig,
) -> Result<Migrated, MigrateError> {
    let options = OpenOptions {
        closure_adjustment: convert::legacy_closure_adjustment(adjustment),
        ..OpenOptions::default()
    };
    migrate_path_with(path, &options)
}

/// Migrate a legacy `.stalx` held in memory.
pub fn migrate_bytes(bytes: &[u8]) -> Result<Migrated, MigrateError> {
    migrate_bytes_with(bytes, &OpenOptions::default())
}

/// [`migrate_bytes`] with the legacy reader's options.
pub fn migrate_bytes_with(bytes: &[u8], options: &OpenOptions) -> Result<Migrated, MigrateError> {
    let legacy = stalagmight_legacy::open_bytes(bytes, options)?;
    migrate(
        &legacy,
        &convert::closure_adjustment(&options.closure_adjustment),
    )
}

/// Convert a legacy project, opened by the frozen reader, into the
/// station-graph model, and verify it against the legacy positions.
/// `adjustment` is the least-squares config the legacy open solved the
/// Closures Computed View with (`OpenOptions::closure_adjustment`), so the
/// two views are compared like for like.
///
/// Deterministic: the same legacy survey always gives the same project and
/// report. Fails only on states the model cannot hold (ids out of range, a
/// repeated line id) or on a migration bug (the result does not validate);
/// position differences are reported, not refused.
pub fn migrate(
    legacy: &LegacySurvey,
    adjustment: &ClosureAdjustmentConfig,
) -> Result<Migrated, MigrateError> {
    let mut report = MigrationReport {
        schema_version: legacy.schema_version,
        history_rows_dropped: legacy.history.undo_rows + legacy.history.redo_rows,
        legacy_derived_shot_uuids: legacy.load.derived_shot_uuids.len(),
        legacy_renamed_shot_uuids: legacy.load.renamed_shot_uuids.len(),
        legacy_healed_offsets: legacy.load.healed_offsets,
        legacy_merges_created: legacy.load.merges_created,
        legacy_integrity_findings: legacy.load.integrity.len(),
        ..MigrationReport::default()
    };
    let built = assemble::build(legacy, &mut report)?;
    let violations = built.project.validate();
    if !violations.is_empty() {
        tracing::error!(
            violations = violations.len(),
            "migration produced an invalid project"
        );
        return Err(MigrateError::Invalid(violations));
    }
    report.stations = built.project.stations.len();
    report.verification = verify::verify(&built, legacy, adjustment);

    let ignored_warnings = ignores::rekey(legacy, &built.final_uuid, &mut report);
    let lp = &legacy.project;
    let extras = MigratedExtras {
        speleo_link: legacy.extras.speleo_link.as_ref().map(convert::speleo_link),
        speleo_undo_chain: legacy.extras.speleo_undo_chain.clone(),
        speleo_undo_watermark: legacy.extras.speleo_undo_watermark.clone(),
        display_settings: legacy.extras.display_settings.clone(),
        ignored_warnings,
        unrecognised_shot_kinds: unrecognised_shot_kinds(legacy, &built.project),
        validation_stored: legacy.load.validation_stored,
        names_changed: built.names_changed.clone(),
        legacy_parity: LegacyParity {
            reversed_lines: lp
                .lines
                .iter()
                .filter(|l| l.reverse_parity)
                .map(|l| l.id)
                .collect(),
            counter_parity_shots: lp
                .lines
                .iter()
                .flat_map(|l| l.shots.iter().map(move |s| (l.id, s)))
                .filter_map(|(line, shot)| {
                    let uuid = shot.uuid.as_ref()?;
                    lp.counter_parity_shots
                        .contains(uuid)
                        .then(|| built.final_uuid.get(&(line, uuid.clone())).cloned())
                        .flatten()
                })
                .collect(),
        },
    };

    let v = &report.verification;
    if v.is_clean() {
        tracing::info!(
            stations = report.stations,
            merges_collapsed = report.merges_collapsed,
            ties = report.merges_demoted_cycle + report.merges_demoted_same_line,
            max_xy_m = v.max_xy_m,
            "migrated a legacy .stalx"
        );
    } else {
        tracing::warn!(
            mismatches = v.mismatches.len(),
            max_xy_m = v.max_xy_m,
            max_z_m = v.max_z_m,
            "migrated a legacy .stalx with unexplained position differences"
        );
    }
    Ok(Migrated {
        project: built.project,
        extras,
        report,
        had_history: legacy.history.any(),
    })
}

/// The legacy load's unreadable shot kinds on the migrated survey: the same
/// line (line ids are kept) and shot position, the migrated session shot id.
fn unrecognised_shot_kinds(
    legacy: &LegacySurvey,
    project: &Project,
) -> Vec<crate::models::UnrecognisedShotKind> {
    legacy
        .load
        .unrecognised_shot_kinds
        .iter()
        .filter_map(|found| {
            let line = project.line(found.line_id)?;
            let index = usize::try_from(found.shot_number.checked_sub(1)?).ok()?;
            Some(crate::models::UnrecognisedShotKind {
                line_id: found.line_id,
                shot_id: line.shots.get(index)?.id,
                shot_number: found.shot_number,
                line_name: line.name.clone(),
                raw_value: found.raw_value.clone(),
            })
        })
        .collect()
}

/// The cheap header of a legacy (SQLite) `.stalx` at `path`, in the shape
/// [`super::stalx_v2::probe_path`] gives a version 2 file (plan 6.1, F13):
/// the `project` member, a `summary` and the SpeleoDB link identity, so
/// Recents and the other header readers treat both formats alike.
///
/// Read-only and cheap: one read-only connection, the frozen reader's schema
/// guard (a file whose schema would run its own SQL is refused, as every
/// legacy open refuses it), then aggregate queries only. No migration, no
/// integrity check, no project load: a newer schema still lists (the full
/// open refuses it), and a file with a hot journal fails the read-only open
/// (its first read would have to roll the journal back).
///
/// `summary.stations` counts slots (shots plus one start per line), as 0.6.0
/// listed a file; once the file is converted it counts station entities (a
/// junction once). `summary.length` leaves Reference shots out (#689) and is
/// an upper bound for a file not yet opened since the shot-kind backfill.
/// `version` is the SQLite `user_version` stamp.
pub fn legacy_header(path: &Path) -> Result<super::stalx_v2::Header, LegacyError> {
    use std::io::Read as _;

    use rusqlite::{Connection, OpenFlags, OptionalExtension as _};

    use super::stalx_v2::{Format, Header, ProjectInfo, SpeleoDbLink, Summary, SQLITE_MAGIC};
    use crate::models::survey::ZeroDatum;
    use crate::models::{GpsCoordinates, UndoMode};

    let mut magic = [0u8; 16];
    std::fs::File::open(path)?
        .read_exact(&mut magic)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => LegacyError::NotSqlite,
            _ => LegacyError::Io(e),
        })?;
    if magic.as_slice() != SQLITE_MAGIC {
        return Err(LegacyError::NotSqlite);
    }
    let sql = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::NotADatabase) => LegacyError::NotSqlite,
        _ => LegacyError::Corrupt(e.to_string()),
    };
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sql)?;
    stalagmight_legacy::persistence::check_schema(&conn)
        .map_err(|e| LegacyError::Corrupt(format!("{e:#}")))?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sql)?;
    let (name, location, created, modified): (String, Option<String>, String, String) = conn
        .query_row(
            "SELECT name, location, created_at, modified_at FROM project LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(sql)?;
    let time = |text: &str| {
        chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|t| t.with_timezone(&chrono::Utc))
    };
    let count = |table: &str| -> u64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .map_or(0, |n| u64::try_from(n).unwrap_or(0))
    };
    let lines = count("sections");
    let shots = count("shots");
    // The 0.6.0 listing's rule (#689): only an INTEGER-stored Normal (0) or
    // Jump (1) counts; a file without the column predates the shot kinds.
    let length_sql = if conn.prepare("SELECT kind FROM shots LIMIT 0").is_ok() {
        "SELECT COALESCE(SUM(length), 0.0) FROM shots \
         WHERE typeof(kind) = 'integer' AND kind IN (0, 1)"
    } else {
        "SELECT COALESCE(SUM(length), 0.0) FROM shots"
    };
    let length: f64 = conn
        .query_row(length_sql, [], |row| row.get(0))
        .unwrap_or(0.0);
    // The datums' GPS only, for the summary's center and bounds (list order:
    // the first row is the center, as 0.6.0 listed it).
    let mut datums = Project::new(name.clone(), None);
    if let Ok(mut stmt) =
        conn.prepare("SELECT latitude, longitude, depth_reference FROM zero_datums")
    {
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, f64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
            ))
        });
        if let Ok(rows) = rows {
            for (latitude, longitude, depth) in rows.flatten() {
                if let Ok(gps) = GpsCoordinates::new(latitude, longitude, depth) {
                    datums.datums.push(ZeroDatum {
                        station: 0,
                        via: None,
                        gps,
                        accuracy_m: None,
                        entrance: None,
                        correction: None,
                    });
                }
            }
        }
    }
    // A pre-v9 file has no link table: no link.
    let speleodb = conn
        .query_row(
            "SELECT endpoint, project_id, project_name, undo_mode FROM speleo_link WHERE id = 1",
            [],
            |row| {
                Ok(SpeleoDbLink {
                    endpoint: row.get(0)?,
                    project_id: row.get(1)?,
                    project_name: row.get(2)?,
                    undo_mode: row
                        .get::<_, Option<String>>(3)?
                        .map(|mode| UndoMode::from_db_str(&mode)),
                })
            },
        )
        .optional()
        .ok()
        .flatten();
    let geo = Summary::of(&datums, speleodb.as_ref());
    let summary = Summary {
        name: name.clone(),
        lines,
        stations: shots.saturating_add(lines),
        shots,
        length: length.is_finite().then_some(length),
        center: geo.center,
        bounds: geo.bounds,
        speleodb: geo.speleodb,
    };
    Ok(Header {
        format: Format::Legacy,
        version: u64::try_from(version).ok().filter(|v| *v > 0),
        requires: Vec::new(),
        generator: None,
        summary: Some(summary),
        project: Some(ProjectInfo {
            name,
            location,
            created: time(&created),
            modified: time(&modified),
        }),
        speleodb,
        local: None,
        complete: true,
    })
}
