//! Salvage (plan 6.6): the recovery dialog's "Attempt Repair" and "Export
//! Salvageable Data" on a file the load refused, and the open project's
//! integrity Repair.
//!
//! A version 2 file that parses but breaks the survey's invariants is refused
//! by the load ("cannot be opened normally") and the recovery dialog offers
//! salvage. [`stalx_v2::decode_salvage`] repairs a copy of the model in memory
//! (drop dangling references, give an anchorless piece a placement, break a
//! cycle into a `Batch` closure, ...), and every repair is reported:
//!
//! - "Attempt Repair" keeps the damaged file as `<name>.stalx.backup` (the
//!   next free `.backup.N`; "Restore from Backup" lists it) and writes the
//!   repaired project in its place, guarded against a change since the read,
//!   so the dialog's "Open" opens the repaired project at the path Recents,
//!   a SpeleoDB link and the user already know (0.6.0 repaired in place after
//!   the same numbered backup). The history is dropped: its entries describe
//!   the survey before the repair. A SpeleoDB-managed copy is marked as having
//!   unpushed edits: it no longer matches the revision it was based on.
//! - "Export Salvageable Data" writes nothing: it returns the repaired project
//!   as canonical version 2 JSON (a file that opens once renamed to `.stalx`),
//!   with the repairs listed as warnings.
//!
//! A legacy SQLite file keeps 0.6.0's SQLite repair and table-by-table export
//! (the persistence crate's `Database`), until the SQLite layer is retired.
//!
//! The open project needs neither: the load refuses an invalid file and every
//! edit validates before it commits, so [`AppState::repair_open_project_integrity`]
//! has nothing to repair.

use std::path::{Path, PathBuf};

use super::project_io::{generator, same_file};
use super::project_restore::name;
use super::AppState;
use crate::core::IntegrityRepairResult;
use crate::models::survey::Project;
use crate::persistence::stalx_v2::{
    self, decode_salvage, encode, write_atomic, DecodeOptions, Extras, Fingerprint, Format, Guard,
    History, Mode, SalvageReport,
};
use crate::persistence::{Database, RepairResult, SalvagedData};

/// How many repairs a message lists before it says how many more there are.
const LISTED: usize = 5;

impl AppState {
    /// Repair the open project's integrity (the integrity banner's Repair).
    /// The open project always holds the invariants (see the module docs), so
    /// this reports what is left (nothing, unless a bug let an invalid model
    /// in) and changes nothing.
    pub fn repair_open_project_integrity(&self) -> IntegrityRepairResult {
        let remaining =
            u32::try_from(self.integrity_violation_messages().len()).unwrap_or(u32::MAX);
        IntegrityRepairResult {
            fixed: 0,
            remaining,
            changed: false,
        }
    }

    /// The recovery dialog's "Attempt Repair" on the file at `path` (see the
    /// module docs). `Err` means nothing was written and says why; an `Ok`
    /// with `success: false` means the file could not be rebuilt (nothing
    /// written either, unless the legacy SQLite repair ran).
    pub fn repair_project_file(&self, path: &Path) -> Result<RepairResult, String> {
        if self
            .open_file_path()
            .is_some_and(|open| same_file(&open, path))
        {
            return Err(format!(
                "Not repaired: {} is open in Stalagmight. Close it, then repair.",
                name(path)
            ));
        }
        let (bytes, fingerprint) = Fingerprint::read(path, DecodeOptions::default().max_file_bytes)
            .map_err(|e| format!("Repair failed: {e}"))?;
        match stalx_v2::sniff(&bytes) {
            Format::V2 => repair_v2(path, &bytes, &fingerprint),
            Format::Legacy => {
                drop(bytes);
                repair_legacy(path)
            }
            Format::Unknown => {
                Err("Repair failed: this is not a Stalagmight project file".to_string())
            }
        }
    }

    /// The recovery dialog's "Export Salvageable Data" for the file at `path`:
    /// whatever can be read, never written back.
    pub fn salvage_project_file(path: &Path) -> Result<SalvagedData, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Salvage failed: {e}"))?;
        match stalx_v2::sniff(&bytes) {
            Format::V2 => Ok(salvage_v2(&bytes)),
            Format::Legacy => {
                drop(bytes);
                Database::export_salvageable(path).map_err(|e| format!("Salvage failed: {e}"))
            }
            Format::Unknown => {
                Err("Salvage failed: this is not a Stalagmight project file".to_string())
            }
        }
    }
}

/// The repaired project's file content: the file's own extras, this build as
/// the generator, no history (it describes the survey before the repair).
fn repaired_bytes(project: &Project, report: &SalvageReport) -> Result<Vec<u8>, String> {
    let mut extras: Extras = report.extras.clone().unwrap_or_default();
    extras.generator = generator();
    extras.history = History::default();
    extras.read_only = None;
    if extras.speleodb.is_some() {
        extras.local.speleodb.unpushed_edits = true;
        extras.local.speleodb.last_edit_at = Some(chrono::Utc::now());
    }
    encode(project, &extras, Mode::Normal).map_err(|e| e.to_string())
}

/// The repairs, in words, `LISTED` at most.
fn repairs_text(report: &SalvageReport) -> String {
    let mut text = report
        .repairs
        .iter()
        .take(LISTED)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    if let Some(more) = report.repairs.len().checked_sub(LISTED).filter(|n| *n > 0) {
        text.push_str(" (and ");
        text.push_str(&more.to_string());
        text.push_str(" more)");
    }
    text
}

/// Why salvage could not rebuild the project.
fn unrepairable(report: &SalvageReport) -> String {
    if let Some(error) = &report.error {
        return error.to_string();
    }
    let mut text = format!(
        "{} problem(s) could not be repaired",
        report.remaining.len()
    );
    for (i, violation) in report.remaining.iter().take(LISTED).enumerate() {
        text.push_str(if i == 0 { ": " } else { "; " });
        text.push_str(&violation.to_string());
    }
    text
}

fn repair_v2(path: &Path, bytes: &[u8], fingerprint: &Fingerprint) -> Result<RepairResult, String> {
    let (project, report) = decode_salvage(bytes);
    if let Some(stalx_v2::DecodeError::NewerVersion { found, supported }) = &report.error {
        // A newer format is not this build's to rewrite (#649).
        return Err(format!(
            "Repair failed: {}",
            crate::models::NewerSchema {
                file: name(path),
                found: i32::try_from(*found).unwrap_or(i32::MAX),
                supported: i32::try_from(*supported).unwrap_or(i32::MAX),
            }
        ));
    }
    let Some(mut project) = project else {
        tracing::warn!(
            repairs = report.repairs.len(),
            remaining = report.remaining.len(),
            "Salvage could not rebuild the project"
        );
        return Ok(RepairResult {
            success: false,
            backup_path: String::new(),
            message: format!(
                "Repair could not rebuild the project ({}). Nothing was changed. Try Restore \
                 from Backup or Export Salvageable Data instead.",
                unrepairable(&report)
            ),
        });
    };
    if report
        .extras
        .as_ref()
        .is_some_and(|extras| extras.read_only.is_some())
    {
        return Err(
            "Repair failed: the file was made with a newer Stalagmight, which can repair it \
             without losing what this version does not understand"
                .to_string(),
        );
    }
    if report.repairs.is_empty() {
        return Ok(RepairResult {
            success: true,
            backup_path: String::new(),
            message: "The project opens normally: nothing needed repairing.".to_string(),
        });
    }
    project.file_path = None;
    project.touch();
    let repaired = repaired_bytes(&project, &report).map_err(|e| format!("Repair failed: {e}"))?;
    let backup = numbered_backup(path).map_err(|e| {
        format!(
            "Repair failed: could not keep a copy of {} before repairing it ({e})",
            name(path)
        )
    })?;
    if let Err(e) = write_atomic(path, &repaired, Guard::Expect(fingerprint)) {
        if let Err(cleanup) = std::fs::remove_file(&backup) {
            tracing::warn!(error = %cleanup, path = ?backup, "could not remove the unused repair backup");
        }
        return Err(format!("Repair failed: {e}"));
    }
    tracing::info!(
        path = ?path,
        repairs = report.repairs.len(),
        purpose = "repair a project the load refused",
        "Repaired the project in place; the damaged original is kept"
    );
    Ok(RepairResult {
        success: true,
        backup_path: backup.display().to_string(),
        message: format!(
            "Repair succeeded: {} fix(es): {}. The damaged original is kept as {}.",
            report.repairs.len(),
            repairs_text(&report),
            name(&backup)
        ),
    })
}

/// 0.6.0's SQLite repair (backup, REINDEX, VACUUM, quick check).
fn repair_legacy(path: &Path) -> Result<RepairResult, String> {
    match Database::attempt_repair(path) {
        Ok((backup_path, integrity_ok)) => {
            let message = if integrity_ok {
                "Repair succeeded - database integrity restored.".to_string()
            } else {
                "Repair completed but integrity check still fails. \
                 Try exporting salvageable data instead."
                    .to_string()
            };
            Ok(RepairResult {
                success: integrity_ok,
                backup_path: backup_path.display().to_string(),
                message,
            })
        }
        Err(e) => {
            tracing::error!(error = %e, "Repair failed");
            Err(format!("Repair failed: {e}"))
        }
    }
}

/// `<file>.backup`, else the first free `<file>.backup.N`, holding a copy of
/// `path` (never overwrites a file).
fn numbered_backup(path: &Path) -> std::io::Result<PathBuf> {
    let mut source = std::fs::File::open(path)?;
    for i in 0..100 {
        let mut candidate = path.as_os_str().to_os_string();
        candidate.push(if i == 0 {
            ".backup".to_string()
        } else {
            format!(".backup.{i}")
        });
        let candidate = PathBuf::from(candidate);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut out) => {
                std::io::copy(&mut source, &mut out)?;
                out.sync_all()?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many existing backups (100)"))
}

fn salvage_v2(bytes: &[u8]) -> SalvagedData {
    let (project, report) = decode_salvage(bytes);
    let mut salvaged = SalvagedData::default();
    let Some(project) = project else {
        // Nothing rebuilt: say why, and what the header still tells.
        salvaged.errors.push(format!(
            "The project could not be rebuilt: {}",
            unrepairable(&report)
        ));
        let header = stalx_v2::probe_bytes(bytes);
        if let Some(info) = header.project {
            salvaged.project_name = Some(info.name);
            salvaged.project_location = info.location;
        }
        if let Some(summary) = header.summary {
            salvaged.line_count = usize::try_from(summary.lines).unwrap_or(usize::MAX);
            salvaged.shot_count = usize::try_from(summary.shots).unwrap_or(usize::MAX);
        }
        return salvaged;
    };
    salvaged.project_name = Some(project.name.clone());
    salvaged.project_location.clone_from(&project.location);
    salvaged.line_count = project.lines().len();
    salvaged.shot_count = project.lines().iter().map(|l| l.shots.len()).sum();
    let topology = project.topology();
    salvaged.merge_count = project
        .stations
        .keys()
        .filter(|id| topology.is_junction(**id))
        .count();
    let lines: Vec<serde_json::Value> = project
        .lines()
        .iter()
        .map(|line| {
            serde_json::json!({
                "id": line.id,
                "name": line.name,
                "color": line.color,
                "shots": line.shots.len(),
            })
        })
        .collect();
    salvaged.lines_json = serde_json::to_string(&lines).ok();
    match repaired_bytes(&project, &report).map(String::from_utf8) {
        Ok(Ok(text)) => salvaged.full_project_json = Some(text),
        Ok(Err(e)) => salvaged
            .errors
            .push(format!("Failed to write the project: {e}")),
        Err(e) => salvaged
            .errors
            .push(format!("Failed to write the project: {e}")),
    }
    salvaged.errors.extend(
        report
            .repairs
            .iter()
            .map(|repair| format!("Repaired: {repair}")),
    );
    salvaged
}
