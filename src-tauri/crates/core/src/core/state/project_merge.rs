//! "Merge projects" (#674; plan 5.8): N source files into one NEW version 2
//! project file, optionally archiving the originals. The open project is
//! never touched.
//!
//! A merge is "new empty project + append each source": the copy is the
//! model's `append_project`, the routine "Add .stalx to current project"
//! uses too, so the two cannot drift apart on what a copy carries. Each
//! source is read the way an add reads it (`read_source_project`: a version
//! 2 file decoded, a legacy one migrated through a private copy), so a
//! source is never written. The output is encoded canonically and written
//! with `write_atomic(Guard::Absent)`: a file that appears at the output
//! path meanwhile is never replaced.
//!
//! As in 0.6.0 the output does not carry the sources' ignored warnings nor
//! their import hashes: it is a fresh project, and its warnings are
//! evaluated from scratch.

use std::path::{Path, PathBuf};

use super::project_io::{generator, save_failure};
use super::project_source::{with_suffix, SourceProject, SQLITE_SIDECARS};
use super::AppState;
use crate::core::command::{sanitize_display_text, MAX_FILE_NAME_LEN};
use crate::core::{Command, CommandResult, ValidationWarning, WarningSeverity};
use crate::models::survey::Project;
use crate::models::{FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID, MAX_STATION_ID};
use crate::persistence::stalx_v2::{encode, write_atomic, Extras, Guard, Mode, WriteError};

/// How much of a source's path a merge refusal shows (a path, unlike a file
/// name, has no natural bound).
const MAX_MERGE_SOURCE_LABEL: usize = 4 * MAX_FILE_NAME_LEN;

/// A source's path as a merge refusal names it: the whole path, since two
/// sources can share a file name, cut from the FRONT when it is too long so
/// the file name - the part that says which source failed - always survives.
pub(super) fn merge_source_label(path: &Path) -> String {
    let full = sanitize_display_text(&path.to_string_lossy(), usize::MAX);
    let count = full.chars().count();
    if count <= MAX_MERGE_SOURCE_LABEL {
        return full;
    }
    let tail: String = full
        .chars()
        .skip(count - (MAX_MERGE_SOURCE_LABEL - 3))
        .collect();
    format!("...{tail}")
}

/// The id budgets of the whole merge, checked before a single line is
/// appended and worded for the user. Refused whole: a merge writes a NEW
/// file, and half a merge is not a thing the user asked for.
fn budget_refusal(sources: &[SourceProject]) -> Option<String> {
    let lines: usize = sources.iter().map(|s| s.project.lines().len()).sum();
    if lines > MAX_LINES_IN_ID_SPACE {
        return Some(format!(
            "Merge refused: the {} source projects hold {lines} lines \
             in total, which is past the limit of {MAX_LINES_IN_ID_SPACE} lines \
             for one project. Shot ids encode as line_id * {FEATURE_ID_STRIDE} + \
             position in 32 bits, so line ids past {MAX_LINE_ID} cannot hold \
             shots. No output file was written.",
            sources.len()
        ));
    }
    // E5: each source keeps its Station IDs, offset past the ones before it.
    let station_ids: u64 = sources
        .iter()
        .map(|s| u64::from(s.project.counters.next_station_id))
        .sum();
    if station_ids > u64::from(MAX_STATION_ID) {
        return Some(format!(
            "Merge refused: the {} source projects use {station_ids} Station IDs in \
             total, which is past the limit of {MAX_STATION_ID} for one project. No \
             output file was written.",
            sources.len()
        ));
    }
    None
}

impl AppState {
    /// `Command::MergeProjects` (reached through `execute`, which holds
    /// `command_lock`; no edit gate: the open project is not touched).
    pub(super) fn execute_merge_projects(&self, cmd: &Command) -> CommandResult {
        let Command::MergeProjects {
            source_paths,
            output_path,
            output_name,
            archive_originals,
        } = cmd
        else {
            return CommandResult::failure("Not a merge of projects".to_string());
        };
        self.merge_projects(
            source_paths,
            output_path.clone(),
            output_name,
            *archive_originals,
        )
    }

    /// Merge `source_paths` into a NEW project file at `output_path` (the
    /// `.stalx` extension added when missing) named `output_name`.
    pub(super) fn merge_projects(
        &self,
        source_paths: &[PathBuf],
        output_path: PathBuf,
        output_name: &str,
        archive_originals: bool,
    ) -> CommandResult {
        if source_paths.is_empty() {
            return CommandResult::failure("No source projects provided".to_string());
        }
        if source_paths.len() < 2 {
            return CommandResult::failure(
                "At least two projects are required for merge".to_string(),
            );
        }
        // Normalize the `.stalx` extension FIRST so the existence check below
        // and the write both refer to the actual on-disk file: an
        // extension-less `output_path` (`foo`) would otherwise slip past an
        // existing `foo.stalx` (#169).
        let output_path = if output_path.extension().is_none_or(|ext| ext != "stalx") {
            output_path.with_extension("stalx")
        } else {
            output_path
        };
        if output_path.exists() {
            return CommandResult::failure(format!(
                "Output file already exists: {}",
                output_path.display()
            ));
        }
        for path in source_paths {
            if !path.exists() {
                return CommandResult::failure(format!(
                    "Source file not found: {}",
                    path.display()
                ));
            }
        }

        let mut sources = Vec::with_capacity(source_paths.len());
        for path in source_paths {
            match self.read_source_project(path, &merge_source_label(path), "merged") {
                Ok(source) => sources.push(source),
                Err(e) => return CommandResult::failure(e),
            }
        }
        if let Some(refusal) = budget_refusal(&sources) {
            return CommandResult::failure(refusal);
        }

        // Validation thresholds from the global config (the "defaults for
        // new projects", #57), as a new project gets them.
        let mut merged = Project::new(output_name, None);
        merged.settings.validation = self.get_config().validation;
        // The entrance elevation (#431, #448): the first source that has one
        // wins; the first later source disagreeing beyond the import
        // tolerance is reported on `CommandResult::warnings` (a merge has no
        // receipt).
        let mut elevation_warning: Option<String> = None;
        for source in &sources {
            let summary = match merged.append_project(&source.project) {
                Ok(outcome) => outcome.value,
                Err(e) => return CommandResult::failure(e.to_string()),
            };
            if let Some((incoming, existing)) = summary.elevation_conflict {
                elevation_warning.get_or_insert_with(|| {
                    format!(
                        "Source project's entrance elevation ({incoming:.2} m) differs from \
                         the merged project's value ({existing:.2} m); kept {existing:.2} m"
                    )
                });
            }
        }

        let extras = Extras {
            generator: generator(),
            ..Extras::default()
        };
        let bytes = match encode(&merged, &extras, Mode::Normal) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::error!(error = %e, "Encoding the merged project failed");
                return CommandResult::failure(format!("Failed to save merged project: {e}"));
            }
        };
        if let Err(e) = write_atomic(&output_path, &bytes, Guard::Absent) {
            return match e {
                WriteError::Exists { .. } | WriteError::ExternallyModified { .. } => {
                    CommandResult::failure(format!(
                        "Output file already exists: {}",
                        output_path.display()
                    ))
                }
                WriteError::Io { error, .. } => save_failure(
                    "Failed to save merged project",
                    &output_path,
                    &anyhow::Error::from(error),
                ),
            };
        }
        tracing::info!(
            output = ?output_path,
            line_count = merged.lines().len(),
            purpose = "combine projects into single file",
            "Projects merged successfully"
        );

        if archive_originals {
            for source_path in source_paths {
                if let Err(e) = archive_project_file(source_path) {
                    // The merge itself succeeded; the original stays where it is.
                    tracing::warn!(
                        path = ?source_path,
                        error = %e,
                        purpose = "archive source projects after merge",
                        "Failed to archive original project (merge completed)"
                    );
                }
            }
        }

        let mut result = CommandResult::success();
        if let Some(message) = elevation_warning {
            result.warnings.push(ValidationWarning {
                code: "SURFACE_ELEVATION_MISMATCH".to_string(),
                message,
                severity: WarningSeverity::Warning,
                parts: None,
            });
        }
        // A source converted as it was read (RS 8.2, 8.3) says so: the
        // merged file shows it converted, with no banner of its own.
        for (path, source) in source_paths.iter().zip(&sources) {
            if let Some(notice) = &source.converted {
                result.warnings.push(ValidationWarning {
                    code: "SOURCE_CONVERTED".to_string(),
                    message: super::project_add::converted_source_note(
                        &merge_source_label(path),
                        notice,
                    ),
                    severity: WarningSeverity::Warning,
                    parts: None,
                });
            }
        }
        result
    }
}

/// Archive a project file by renaming it with an `.archived` suffix
/// (`original.stalx.archived`, a timestamp appended when that exists).
///
/// A legacy source's `-wal` or hot `-journal` belongs to the database by
/// NAME: left behind, the archive loses the WAL's commits (which the merge
/// itself read) and a stale journal waits for whatever file next takes this
/// name. Each moves with the file.
fn archive_project_file(path: &Path) -> std::io::Result<()> {
    let mut name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    name.push_str(".archived");
    let mut archive_path = path.with_file_name(&name);
    if archive_path.exists() {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        name.push('.');
        name.push_str(&timestamp);
        archive_path = path.with_file_name(&name);
    }
    std::fs::rename(path, &archive_path)?;
    for suffix in SQLITE_SIDECARS {
        let sidecar = with_suffix(path, suffix);
        if sidecar.is_file() {
            std::fs::rename(&sidecar, with_suffix(&archive_path, suffix))?;
        }
    }
    tracing::info!(
        original = ?path,
        archived = ?archive_path,
        purpose = "archive source project after merge",
        "Project file archived"
    );
    Ok(())
}
