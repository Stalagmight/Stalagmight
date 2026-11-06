use super::project_io::{save_failure, unreachable_folder_refusal};
use super::AppState;
use crate::core::CommandResult;
use crate::models::{
    Project, SectionId, SectionIdentity, StationKey, StationRef, FEATURE_ID_STRIDE,
    MAX_SECTIONS_IN_ID_SPACE, MAX_SECTION_ID,
};
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
        let output_path = crate::persistence::with_stalx_extension(output_path);

        // Check that output path doesn't already exist
        if output_path.exists() {
            return CommandResult::failure(format!(
                "Output file already exists: {}",
                output_path.display()
            ));
        }

        // [#576 W27] The output's folder has to be there before anything is
        // written, as for a save. Without this, an output spelled through a folder
        // that is not there (`missing/../cave.stalx`) passed the check above (the
        // kernel cannot walk it, so nothing "exists"), and SQLite - which resolves
        // `missing/..` by the name alone - created the temp database at
        // `cave.stalx.tmp` beside it, through a link there into the app's own
        // folders, before the rename failed.
        if let Some(refused) = unreachable_folder_refusal(&output_path) {
            return refused;
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

        // Load all source projects
        //
        // [#576 RV6b-R1-D1] Read, never written: a source an older build saved is
        // upgraded on a copy in memory, not in place, so neither the file nor a
        // `.pre-v<N>.backup` beside it is written - in a user's folder, the app's
        // own folders or a system folder alike, and whether or not the originals
        // are archived afterwards.
        let mut source_projects: Vec<Project> = Vec::new();
        for path in &source_paths {
            let db = match Database::open_existing_to_read(path) {
                Ok(db) => db,
                Err(e) => {
                    return CommandResult::failure(format!(
                        "Failed to open project '{}': {}",
                        path.display(),
                        e
                    ));
                }
            };
            // [#576 B6-D1] The merged project is written at this build's schema, so a
            // source a NEWER build saved would come out of it downgraded - what that
            // build stored and this one does not know is dropped. That is the same loss
            // a Save As of the project is refused for, so the merge is refused too,
            // before anything is written or archived.
            if let Some(version) = db.newer_schema_version() {
                tracing::warn!(
                    path = ?path,
                    purpose = "never downgrade a project saved by a newer version",
                    "Refused to merge a project saved by a newer version of Stalagmight"
                );
                return CommandResult::failure(format!(
                    "Merge refused: '{}' was saved by a newer version of Stalagmight \
                     (file format {version}). The merged project would be written in this \
                     version's older format and lose what the newer one stored. Update \
                     Stalagmight to merge it. No output file was written.",
                    path.display()
                ));
            }
            match db.load_project() {
                Ok(mut project) => {
                    project.file_path = Some(path.clone());
                    // Assign IDs to any legacy closures that have id=0
                    project.assign_closure_ids();
                    source_projects.push(project);
                }
                Err(e) => {
                    return CommandResult::failure(format!(
                        "Failed to load project '{}': {}",
                        path.display(),
                        e
                    ));
                }
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

        // Track section ID remapping: (old_project_idx, old_section_id) -> new_section_id
        let mut section_id_map: std::collections::HashMap<(usize, SectionId), SectionId> =
            std::collections::HashMap::new();

        // Import bookkeeping already in the merged project, so a hash two sources
        // share lands once - both tables are `UNIQUE(hash)`, and a repeat would
        // fail the whole save.
        let mut seen_file_hashes: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut seen_section_hashes: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // Append sections from all projects
        for (proj_idx, source_project) in source_projects.iter().enumerate() {
            for section in &source_project.sections {
                let old_section_id = section.id;
                let new_section_id = merged_project.next_section_id;

                // Shot identity rides along in `SectionData` (`ShotData::uuid`),
                // so `add_section` reproduces each source shot's UUID by itself.
                //
                // Uniqueness is PER-SECTION here, matching the invariant the app
                // actually enforces (`Project::ensure_unique_shot_uuids`, the
                // section-scoped `duplicate_shot_uuid` integrity check, and
                // `resolve_key`'s section-local lookup) - `Section::new` applies
                // it as the sections are built. A project-wide dedupe would be
                // stricter than the model AND than the format: two projects that
                // imported the same Ariane file legitimately hold the same
                // foreign ids (#423), and re-minting them here would throw away
                // exactly the identity the import was preserving. The one
                // consumer that needs document-wide uniqueness - the SpeleoDB
                // sidecar - already drops repeats itself
                // (`export::tml::harden_for_speleodb`).
                //
                // Everything else about the section that `SectionData` cannot
                // carry rides in a `SectionIdentity` (A17, #576). A plain
                // `add_section` resets `reverse_parity` and the captured gauge
                // pair, which decide the station the least-squares solve holds
                // for a datum-less component: losing them moved that gauge to the
                // parity default, translating the component's adjusted geometry
                // (Closures Computed, Plan, Export) by the closure residual, and
                // the merged file's first save then pinned the wrong station.
                // `shot_uuids` stays empty on purpose: each uuid already rides in
                // `ShotData`, and `Section::new` de-duplicates them per section
                // as described above, which restoring the list verbatim would
                // undo.
                //
                // #438: the same goes for the north reference - without it the
                // copy would land on the merged project's default, reverting a
                // corrected section to identity and rendering (and exporting) it
                // rotated by its own declination. The RESOLVED reference, from
                // the section's OWN project: a section that inherited its source
                // project's default has to keep that value here, where the
                // default is a different one. Merging the defaults themselves
                // would be the wrong question - two projects can hold different
                // ones legitimately, and only the per-section answer is
                // unambiguous - so every copied section carries its own and the
                // merged project declares no default at all.
                let identity = SectionIdentity {
                    shot_uuids: Vec::new(),
                    azimuth_reference: Some(source_project.azimuth_reference_for(section)),
                    ..SectionIdentity::of(section)
                };
                merged_project.add_section_with_identity(section.to_section_data(), &identity);

                // Record the mapping
                section_id_map.insert((proj_idx, old_section_id), new_section_id);
            }

            // Copy section offsets with remapped IDs. Through
            // `write_section_offset` so an all-zero entry a source project is
            // carrying (a file written before #413 finding 2 was fixed) is not
            // copied forward into the merged project and re-serialized: the map
            // means "sections that are actually displaced", and (0, 0) is the
            // same geometry as no entry at all.
            for (old_section_id, offset) in source_project.section_offsets() {
                if let Some(&new_section_id) = section_id_map.get(&(proj_idx, *old_section_id)) {
                    merged_project.set_section_offset(new_section_id, *offset);
                } else {
                    tracing::warn!(
                        project = %source_project.name,
                        section_id = old_section_id,
                        purpose = "merge section offsets",
                        "Section offset references unknown section ID; skipping"
                    );
                }
            }

            // Copy station offsets with remapped section IDs. Resolve the source
            // key to a positional index against the source, then store as a
            // transitional Index anchor under the new section; redrive's
            // reanchor pass canonicalizes it once merged shots are present.
            for (old_key, offset) in &source_project.station_offsets {
                if let Some(&new_section_id) = section_id_map.get(&(proj_idx, old_key.section_id)) {
                    if let Some(src_sr) = source_project.resolve_key(old_key) {
                        let new_key = StationKey::positional(new_section_id, src_sr.station_index);
                        merged_project.station_offsets.insert(new_key, *offset);
                    }
                } else {
                    tracing::warn!(
                        project = %source_project.name,
                        section_id = old_key.section_id,
                        purpose = "merge station offsets",
                        "Station offset references unknown section ID; skipping"
                    );
                }
            }

            // Copy station comments with remapped section IDs. Same transitional
            // Index-anchor approach as station_offsets above: resolve the source
            // key to a positional index, then store under the new section; the
            // redrive reanchor pass canonicalizes it once the merged shots exist.
            // (Without this, every per-station comment was silently dropped on
            // merge while every sibling collection was copied.)
            for (old_key, comment) in &source_project.station_comments {
                if let Some(&new_section_id) = section_id_map.get(&(proj_idx, old_key.section_id)) {
                    if let Some(src_sr) = source_project.resolve_key(old_key) {
                        let new_key = StationKey::positional(new_section_id, src_sr.station_index);
                        merged_project
                            .station_comments
                            .insert(new_key, comment.clone());
                    }
                } else {
                    tracing::warn!(
                        project = %source_project.name,
                        section_id = old_key.section_id,
                        purpose = "merge station comments",
                        "Station comment references unknown section ID; skipping"
                    );
                }
            }

            // Copy zero datums with remapped section IDs
            // Use store_zero_datum_without_repositioning to preserve original offsets
            // (section_offsets were already copied above with remapped IDs)
            for zd in &source_project.zero_datums {
                if let Some(&new_section_id) =
                    section_id_map.get(&(proj_idx, zd.station_ref.section_id))
                {
                    let Some(src_sr) = source_project.resolve_key(&zd.station_ref) else {
                        continue;
                    };
                    let new_station_ref = StationRef::new(new_section_id, src_sr.station_index);
                    // Use store-only helper to avoid triggering repositioning during merge
                    if let Err(e) = merged_project
                        .store_zero_datum_without_repositioning(new_station_ref, zd.gps)
                    {
                        return CommandResult::failure(format!(
                            "Failed to copy zero datum from source project {}: {}",
                            source_project.name, e
                        ));
                    }
                } else {
                    tracing::warn!(
                        project = %source_project.name,
                        section_id = zd.station_ref.section_id,
                        purpose = "merge zero datums",
                        "Zero datum references unknown section ID; skipping"
                    );
                }
            }

            // Copy station merges with remapped section IDs (only within same source project)
            for merge in &source_project.station_merges {
                if let (Some(&new_a_section), Some(&new_b_section)) = (
                    section_id_map.get(&(proj_idx, merge.station_a.section_id)),
                    section_id_map.get(&(proj_idx, merge.station_b.section_id)),
                ) {
                    let (Some(a_sr), Some(b_sr)) = (
                        source_project.resolve_key(&merge.station_a),
                        source_project.resolve_key(&merge.station_b),
                    ) else {
                        continue;
                    };
                    merged_project.record_station_merge(crate::models::StationMerge {
                        station_a: StationKey::positional(new_a_section, a_sr.station_index),
                        station_b: StationKey::positional(new_b_section, b_sr.station_index),
                        merged_at: merge.merged_at,
                    });
                } else {
                    tracing::warn!(
                        project = %source_project.name,
                        station_a_section = merge.station_a.section_id,
                        station_b_section = merge.station_b.section_id,
                        purpose = "merge station merges",
                        "Station merge references unknown section ID; skipping"
                    );
                }
            }

            // Copy loop closures with remapped section IDs (only within same source project)
            for closure in &source_project.loop_closures {
                if let (Some(&new_a_section), Some(&new_b_section)) = (
                    section_id_map.get(&(proj_idx, closure.station_a.section_id)),
                    section_id_map.get(&(proj_idx, closure.station_b.section_id)),
                ) {
                    let (Some(a_sr), Some(b_sr)) = (
                        source_project.resolve_key(&closure.station_a),
                        source_project.resolve_key(&closure.station_b),
                    ) else {
                        continue;
                    };
                    merged_project
                        .loop_closures
                        .push(crate::models::LoopClosure {
                            id: 0, // Reset to 0; assign_closure_ids() will reassign unique IDs
                            station_a: StationKey::positional(new_a_section, a_sr.station_index),
                            station_b: StationKey::positional(new_b_section, b_sr.station_index),
                            path_length: closure.path_length,
                            misclosure_distance: closure.misclosure_distance,
                            error_percentage: closure.error_percentage,
                            horizontal_misclosure: closure.horizontal_misclosure,
                            vertical_misclosure: closure.vertical_misclosure,
                            strategy: closure.strategy,
                            is_aligned: closure.is_aligned,
                        });
                } else {
                    tracing::warn!(
                        project = %source_project.name,
                        station_a_section = closure.station_a.section_id,
                        station_b_section = closure.station_b.section_id,
                        purpose = "merge loop closures",
                        "Loop closure references unknown section ID; skipping"
                    );
                }
            }

            // Combine surface elevation: first `Some` wins; a disagreeing later
            // source only produces a warning, it never overwrites. A NaN/infinite
            // incoming value (never produced by a real Ariane import, but not
            // provably impossible upstream) is skipped outright - adopting it as
            // the merged value would poison every later comparison and export,
            // and comparing it against `existing` would always read as
            // "differs" without ever explaining why.
            if let Some(incoming) = source_project.surface_elevation.filter(|v| v.is_finite()) {
                match merged_project.surface_elevation {
                    None => merged_project.surface_elevation = Some(incoming),
                    // Strictly greater than 0.005 m (exclusive): a difference AT
                    // the boundary is treated as agreement, matching the message's
                    // own `{:.2}` rounding (see command_import.rs's identical
                    // comparison, which this mirrors).
                    Some(existing) if (existing - incoming).abs() > 0.005 => {
                        if surface_elevation_warning.is_none() {
                            surface_elevation_warning = Some(format!(
                                "Source project's entrance elevation ({incoming:.2} m) differs from \
                                 the merged project's value ({existing:.2} m); kept {existing:.2} m"
                            ));
                        }
                    }
                    Some(_) => {}
                }
            }

            // Union the import hashes (A17, #576): they are how the import
            // wizards recognise a file or DMP section that is already in the
            // project, and a merge that dropped them let any source's survey be
            // imported again with no duplicate warning.
            for hash in &source_project.imported_file_hashes {
                if seen_file_hashes.insert(hash.clone()) {
                    merged_project.imported_file_hashes.push(hash.clone());
                }
            }
            for hash in &source_project.imported_section_hashes {
                if seen_section_hashes.insert(hash.clone()) {
                    merged_project.imported_section_hashes.push(hash.clone());
                }
            }

            // Combine location if present
            if let Some(ref loc) = source_project.location {
                if merged_project.location.is_none() {
                    merged_project.location = Some(loc.clone());
                } else if let Some(ref mut merged_loc) = merged_project.location {
                    if !merged_loc.contains(loc) {
                        merged_loc.push_str(", ");
                        merged_loc.push_str(loc);
                    }
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

        // Remove any leftover temp file - and a link at that name even when it
        // leads nowhere yet, which `exists()` misses and the temp database would
        // be created through (#576 FZ-PERSIST-fs-3, as in `save_project`); one that
        // cannot be removed refuses the merge (#576 RVF-b-3).
        if let Err(refused) = super::project_io::clear_save_temp(&temp_path, &output_path) {
            return *refused;
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
