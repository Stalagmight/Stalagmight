use super::project_append::append_project;
use super::project_io::save_failure;
use super::project_source::read_source_project;
use super::AppState;
use crate::core::command::{sanitize_display_text, MAX_FILE_NAME_LEN};
use crate::core::CommandResult;
use crate::models::{Project, FEATURE_ID_STRIDE, MAX_SECTIONS_IN_ID_SPACE, MAX_SECTION_ID};
use crate::persistence::Database;
use std::path::PathBuf;

impl AppState {
    /// Merge multiple projects into a new project file
    /// This creates a NEW output file, appends all sections with renumbered IDs,
    /// keeps all zero datums, and optionally archives the original files.
    pub(super) fn merge_projects(
        &self,
        source_paths: Vec<PathBuf>,
        output_path: PathBuf,
        output_name: String,
        archive_originals: bool,
    ) -> CommandResult {
        // Validate inputs
        if source_paths.is_empty() {
            return CommandResult::failure("No source projects provided".to_string());
        }

        if source_paths.len() < 2 {
            return CommandResult::failure(
                "At least two projects are required for merge".to_string(),
            );
        }

        // Normalize the `.stalx` extension FIRST so the existence check below and
        // the stored `file_path` both refer to the actual on-disk file. An
        // extension-less `output_path` (e.g. `foo`) otherwise slips past an
        // existing `foo.stalx` and leaves `file_path` pointing at a path that
        // doesn't match what's written (#169).
        let output_path = if output_path.extension().is_none_or(|ext| ext != "stalx") {
            output_path.with_extension("stalx")
        } else {
            output_path
        };

        // Check that output path doesn't already exist
        if output_path.exists() {
            return CommandResult::failure(format!(
                "Output file already exists: {}",
                output_path.display()
            ));
        }

        // Verify all source files exist
        for path in &source_paths {
            if !path.exists() {
                return CommandResult::failure(format!(
                    "Source file not found: {}",
                    path.display()
                ));
            }
        }

        // Read every source the way "Add .stalx" does (#680): through a
        // private copy, so opening an older file never migrates the user's own
        // file in place, and normalized as an open would - shot uuids, closure
        // ids, re-solved offsets, and the pending pre-v13 Jump/Reference
        // classification, which is per file and would otherwise be lost: the
        // merged output does not carry the source's flag.
        let mut source_projects: Vec<Project> = Vec::with_capacity(source_paths.len());
        for path in &source_paths {
            let label = path
                .file_name()
                .map(|n| sanitize_display_text(&n.to_string_lossy(), MAX_FILE_NAME_LEN))
                .unwrap_or_default();
            match read_source_project(path, &label, "merged") {
                Ok(source) => source_projects.push(source.project),
                Err(e) => return CommandResult::failure(e),
            }
        }

        // Create a new merged project; seed validation from the global config
        // (the "defaults for new projects", #57) rather than the hardcoded default.
        let mut merged_project = Project::new(output_name, None);
        merged_project.validation = self.get_config().validation;

        // Section-id budget (#555), checked before a single section is appended.
        // The append loop below calls `Project::add_section` once per source
        // section, and `Section::new` computes `next_shot_id =
        // id * FEATURE_ID_STRIDE` unchecked, so a combined count past the
        // encode lands sections on ids whose shot encode leaves u32: a debug
        // panic (outright at that multiply from id 42_950 on) and a wrap that
        // aliases shot ids in release. The merged project is fresh (ids start
        // at 0), but the sum is stated against its `next_section_id` all the
        // same so the check keeps holding if it ever stops being fresh. Refused
        // whole: a merge writes a NEW file, and half a merge is not a thing the
        // user asked for.
        let incoming_sections: usize = source_projects.iter().map(|p| p.sections.len()).sum();
        let spent_count = merged_project.next_section_id as usize;
        if incoming_sections.saturating_add(spent_count) > MAX_SECTIONS_IN_ID_SPACE {
            return CommandResult::failure(format!(
                "Merge refused: the {} source projects hold {incoming_sections} sections \
                 in total, which is past the limit of {MAX_SECTIONS_IN_ID_SPACE} sections \
                 for one project. Shot ids encode as section_id * {FEATURE_ID_STRIDE} + \
                 position in 32 bits, so section ids past {MAX_SECTION_ID} cannot hold \
                 shots. No output file was written.",
                source_projects.len()
            ));
        }

        // Carry the entrance/surface elevation (#431, #448) forward: the first
        // source that has one wins, and a later source disagreeing beyond the
        // same 0.005 m tolerance used on import (see `command_import.rs`'s
        // `surface_elevation_warning`) is reported instead of silently
        // overwriting or averaging. Unlike the import path, a merge has no
        // `ImportReceipt` to carry a receipt warning on - this rides the
        // returned `CommandResult.warnings` (`ValidationWarning`, a different
        // channel) instead. Only the first disagreement is kept: collecting
        // every later mismatch is not worth the machinery for a value that is
        // already pinned by the first source.
        let mut surface_elevation_warning: Option<String> = None;

        // A merge is "new empty project + append each source" (#674): the copy
        // itself is the routine "Add .stalx to current project" shares, so the
        // two cannot drift apart on what a copy carries.
        for source_project in &source_projects {
            let outcome = match append_project(&mut merged_project, source_project) {
                Ok(outcome) => outcome,
                Err(e) => return CommandResult::failure(e),
            };
            if let Some((incoming, existing)) = outcome.elevation_conflict {
                if surface_elevation_warning.is_none() {
                    surface_elevation_warning = Some(format!(
                        "Source project's entrance elevation ({incoming:.2} m) differs from \
                         the merged project's value ({existing:.2} m); kept {existing:.2} m"
                    ));
                }
            }
        }

        // Reassign closure IDs to ensure uniqueness across merged sources
        merged_project.assign_closure_ids();

        // Re-derive section offsets from the (now combined) merge/datum
        // constraints so the merged output is geometrically consistent and any
        // drift in a source isn't laundered into the new file.
        merged_project.redrive_offsets_after_load();

        // Set the output file path (already normalized to `.stalx` at the top).
        merged_project.file_path = Some(output_path.clone());

        // Save the merged project using safe save pattern (temp file + atomic rename)
        let temp_path = output_path.with_extension("stalx.tmp");

        // Remove any leftover temp file
        if temp_path.exists() {
            let _ = std::fs::remove_file(&temp_path);
        }

        // Save to temporary file
        let mut db = match Database::open(&temp_path) {
            Ok(db) => db,
            Err(e) => {
                let _ = std::fs::remove_file(&temp_path);
                return save_failure("Failed to create output file", &output_path, &e);
            }
        };

        if let Err(e) = db.save_project(&merged_project) {
            drop(db);
            let _ = std::fs::remove_file(&temp_path);
            return save_failure("Failed to save merged project", &output_path, &e);
        }

        // Resolved (ignored) warnings are intentionally NOT carried into the
        // merged output: it is a fresh artifact with re-derived offsets and
        // reassigned closure ids, so warnings are re-evaluated from scratch.

        // Close database before rename
        drop(db);

        // Atomically rename temp file to final destination
        if let Err(e) = std::fs::rename(&temp_path, &output_path) {
            let _ = std::fs::remove_file(&temp_path);
            let e = anyhow::Error::from(e);
            return save_failure("Failed to finalize save", &output_path, &e);
        }

        tracing::info!(
            output = ?output_path,
            section_count = merged_project.sections.len(),
            purpose = "combine projects into single file",
            "Projects merged successfully"
        );

        // Archive original files if requested
        if archive_originals {
            for source_path in &source_paths {
                if let Err(e) = Self::archive_project_file(source_path) {
                    tracing::warn!(
                        path = ?source_path,
                        error = %e,
                        purpose = "archive source projects after merge",
                        "Failed to archive original project (merge completed)"
                    );
                    // Don't fail the operation - merge was successful
                }
            }
        }

        let mut result = CommandResult::success();
        if let Some(message) = surface_elevation_warning {
            result.warnings.push(crate::core::ValidationWarning {
                code: "SURFACE_ELEVATION_MISMATCH".to_string(),
                message,
                severity: crate::core::WarningSeverity::Warning,
            });
        }
        result
    }

    /// Archive a project file by renaming it with .archived suffix
    fn archive_project_file(path: &PathBuf) -> std::io::Result<()> {
        // Create archive filename: original.stalx -> original.stalx.archived
        // If archived file exists, add timestamp: original.stalx.archived.20260117_123456
        let mut archive_path = path.clone();
        let mut new_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        new_name.push_str(".archived");
        archive_path.set_file_name(&new_name);

        // If archive already exists, add timestamp
        if archive_path.exists() {
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
            new_name.push('.');
            new_name.push_str(&timestamp);
            archive_path.set_file_name(&new_name);
        }

        std::fs::rename(path, &archive_path)?;
        tracing::info!(
            original = ?path,
            archived = ?archive_path,
            purpose = "archive source project after merge",
            "Project file archived"
        );
        Ok(())
    }
}
