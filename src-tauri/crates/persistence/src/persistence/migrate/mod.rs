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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use stalagmight_legacy::{LegacyError, LegacySurvey, OpenOptions};

use crate::models::config::ClosureAdjustmentConfig;
use crate::models::survey::{Project, Violation};
use crate::models::{LineId, SpeleoLink};

pub use ignores::IgnoredWarning;
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
    /// Whether the file stored its own validation thresholds. When it did
    /// not, `project.settings.validation` is the 0.6.0 default and the app
    /// should seed the user's global thresholds instead (as 0.6.0 did).
    pub validation_stored: bool,
    /// Record-orientation parity the legacy model kept for `.plnx` dive-plan
    /// pins (a single-neighbour pin reads which end of its shot it sits on).
    /// The station-graph model has no such field; carried so the dive-plan
    /// resolver can still read old pins.
    pub legacy_parity: LegacyParity,
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
        validation_stored: legacy.load.validation_stored,
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
