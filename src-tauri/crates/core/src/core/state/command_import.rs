use super::command_epilogue::EpilogueSteps;
use super::project_slot::ProjectSlot;
use super::session_commit::CommandsHeld;
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    AzimuthReference, ClosureStrategy, Coordinates, Project, SectionData, SectionId, ShotData,
    ShotKind, StationRef, FEATURE_ID_STRIDE, MAX_SECTIONS_IN_ID_SPACE, MAX_SECTION_ID,
    MAX_SHOTS_PER_SECTION,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How a freshly-imported file's virtual shots should be classified (#27 q3).
///
/// A parser can only tell that a shot is virtual; which of the two virtual kinds
/// it is needs the project graph, which only exists once the import is placed
/// and merged. [`Auto`](VirtualShotMode::Auto) runs the chain rule at that
/// point. The two explicit modes let a user who knows the file override it in
/// one control, instead of the per-shot picker the issue originally proposed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum VirtualShotMode {
    /// Chain rule: a virtual run bounded by real survey on both ends is a Jump,
    /// anything else a Reference.
    #[default]
    Auto,
    /// Every virtual shot in this file is a swum shortcut.
    AllJumps,
    /// Every virtual shot in this file is a non-swimmable survey tie.
    AllReferences,
}
use crate::models::poison_policy::RecoveringMutex;

/// Tally closure-creation results from a `create_loop_closures_batch` call,
/// counting successes (and failures) directly from `results` rather than
/// deriving `created` by subtracting `failed` from the requested count.
///
/// That distinction matters for one specific case: when the project lock is
/// unavailable at the point `create_loop_closures_batch` would run, the
/// caller passes an empty `results` (it never ran the batch), so
/// `closure_pairs.iter().zip(results)` yields nothing at all - not one
/// success, not one failure. A subtraction-based count (`requested - skipped
/// - failed`) would then silently report every one of those pairs as
/// created, when in fact none were. Counting `Ok` arms directly reports the
/// correct zero.
fn tally_closure_results(
    closure_pairs: &[(StationRef, StationRef)],
    results: Vec<Result<crate::models::LoopClosure, String>>,
) -> (u32, u32) {
    let mut created = 0u32;
    let mut failed = 0u32;
    for ((station_a, station_b), result) in closure_pairs.iter().zip(results) {
        match result {
            Ok(lc) => {
                created += 1;
                tracing::debug!(
                    closure_id = lc.id,
                    ?station_a,
                    ?station_b,
                    purpose = "import loop closures",
                    "Created loop closure from TML import"
                );
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    ?station_a,
                    ?station_b,
                    error = %e,
                    purpose = "import loop closures",
                    "Failed to create loop closure from TML import, skipping"
                );
            }
        }
    }
    (created, failed)
}

/// RAII guard for [`Project::cascade_deferred`].
///
/// `execute_import_sections` flips the flag on so each per-section
/// `merge_stations` skips its O(N) cascade, then runs ONE cascade at the
/// end.  If the apply loop panics (poisoning the mutex) or any code path
/// returns early without clearing the flag, every subsequent command
/// silently skips its cascade and accumulates section-offset drift.  The
/// guard's `Drop` resets the flag to `false` even through a poisoned
/// mutex, closing that window.
///
/// Call [`Self::disarm`] before the final cascade so the cascade is
/// actually allowed to run (the cascade itself early-exits when the flag
/// is still `true`).  After `disarm`, `Drop` is a no-op.
struct CascadeDeferGuard<'a> {
    project: &'a RecoveringMutex<ProjectSlot>,
    armed: bool,
}

impl<'a> CascadeDeferGuard<'a> {
    /// Set `cascade_deferred = true` on the project (if present) and
    /// return a guard.  If the lock is already poisoned the flag is set
    /// anyway (the lock recovers it, #576 A11) - the import is about to run
    /// regardless, and we still want the Drop reset to fire.
    fn arm(project: &'a RecoveringMutex<ProjectSlot>) -> Self {
        Self::set_flag(project, true);
        Self {
            project,
            armed: true,
        }
    }

    /// Clear the flag now (allowing the final cascade to run) and skip
    /// the Drop reset.
    fn disarm(mut self) {
        Self::set_flag(self.project, false);
        self.armed = false;
    }

    fn set_flag(project: &RecoveringMutex<ProjectSlot>, value: bool) {
        let mut guard = project.lock();
        if let Some(p) = guard.as_mut() {
            p.cascade_deferred = value;
        }
    }
}

impl<'a> Drop for CascadeDeferGuard<'a> {
    fn drop(&mut self) {
        if self.armed {
            // Reset on the panic / early-return path.  `set_flag` tolerates
            // a poisoned mutex so the flag is cleared even if the apply
            // loop panicked while holding the lock.
            Self::set_flag(self.project, false);
        }
    }
}

impl AppState {
    /// Execute a create-section-and-merge operation as a single atomic undo/redo unit.
    /// This is used when importing a new section and immediately merging one of its stations
    /// with an existing station. Both operations are combined into a single undo entry.
    pub fn execute_create_and_merge(
        &self,
        name: Option<String>,
        surveyor: Option<String>,
        date: Option<chrono::NaiveDate>,
        color: String,
        direction: Option<crate::models::SurveyDirection>,
        shots: Vec<ShotData>,
        import_station_index: u32,
        target_station: StationRef,
        _closure_strategy: ClosureStrategy,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let _cmd_guard = self.command_lock.lock();
        // Read-only edit gate (#191, D-B6): a TML create+merge mutates the
        // project, so it is gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Import section and merge") {
            return CommandResult::refused(f);
        }
        tracing::info!(
            purpose = "keep undo atomic for import merge",
            "Executing create_section_and_merge as single undo unit"
        );

        // Take a project snapshot BEFORE any modifications
        let snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // [#576 A4] The frame-defining datum before the import, for the shared
        // epilogue's frame check.
        let frame_before = self.frame_datum_key();

        // Step 1: Create the section (using internal method)
        let section_data = SectionData {
            name: name.clone(),
            surveyor: surveyor.clone(),
            date,
            color: color.clone(),
            direction,
            quality: 1.0,
            shots: shots.clone(),
        };

        let create_result = self.create_section(section_data);
        if !create_result.success {
            return create_result;
        }

        let Some(new_section_id) = create_result.section_id else {
            return CommandResult::failure("Section created but no ID returned".to_string());
        };

        // Step 2: Create the station reference for the newly created section
        let import_station_ref = StationRef {
            section_id: new_section_id,
            station_index: import_station_index,
        };

        // Step 3: Merge the stations
        let merge_result = self.merge_stations(import_station_ref, target_station);
        if !merge_result.success {
            // Merge failed after section was created - rollback to pre-mutation snapshot
            // to avoid leaving a created section with no undo entry.
            tracing::error!(
                purpose = "keep project consistent after failed merge",
                "Merge failed after section creation; rolling back to pre-mutation snapshot"
            );
            self.rollback_project(&snapshot);
            return merge_result;
        }

        // Step 4: [#576 A4] The shared forward epilogue - frame check and reconcile,
        // as `execute()` runs for a CreateSection and a MergeStations (this path ran
        // neither); no orphan scan, since it only ADDS connectivity - then the after
        // snapshot and one undo entry with before+after, described as a Batch.
        let batch_cmd = Command::Batch {
            commands: vec![
                Command::CreateSection {
                    name,
                    surveyor,
                    date,
                    color,
                    direction,
                    shots,
                },
                Command::MergeStations {
                    station_a: import_station_ref,
                    station_b: target_station,
                },
            ],
        };

        if let Err(failure) =
            self.record_snapshot_edit(frame_before, EpilogueSteps::GEOMETRY, batch_cmd, snapshot)
        {
            return failure;
        }

        // Combine warnings from both operations
        let mut all_warnings = create_result.warnings;
        all_warnings.extend(merge_result.warnings);

        // Return success with the new section ID
        let result = CommandResult::success().with_section_id(new_section_id);
        if all_warnings.is_empty() {
            result
        } else {
            result.with_warnings(all_warnings)
        }
    }

    /// Execute a batch import of sections as a single atomic undo/redo unit.
    /// Creates all sections with their placements (merges, zero datums, moves)
    /// and optionally tracks the file hash. Uses `ProjectSnapshotPair` so a
    /// single undo reverts everything including the hash.
    ///
    /// `mode` picks how this file's virtual shots are split into Jump vs
    /// Reference (#27). The pass runs inside this command, before the `after`
    /// snapshot: doing it afterwards (the first cut) left the classified kinds
    /// out of the redo state, so undo-then-redo of an import silently reverted
    /// every shot to the parser's conservative `Reference` seed and dropped the
    /// far side of each jump out of gas and penetration.
    pub fn execute_import_sections(
        &self,
        sections: Vec<crate::models::ImportSectionConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        section_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
    ) -> CommandResult {
        self.execute_import_sections_inner(
            sections,
            closures,
            file_hash,
            section_hashes,
            existing_shot_updates,
            mode,
            None,
        )
    }

    /// Same as [`Self::execute_import_sections`], but also merges an Ariane
    /// file's project-level entrance elevation (#431) into `Project::surface_elevation`
    /// atomically with the import, so undo/redo covers both. The only caller
    /// that has this value is the Ariane TML/TMLU importer.
    pub fn execute_tml_import_sections(
        &self,
        sections: Vec<crate::models::ImportSectionConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        section_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        self.execute_import_sections_inner(
            sections,
            closures,
            file_hash,
            section_hashes,
            existing_shot_updates,
            mode,
            surface_elevation,
        )
    }

    /// [`Self::execute_tml_import_sections`] inside a `command_lock` span the caller
    /// already holds [#576 NF-W4C-3]: the same gate, validation, import, epilogue and
    /// receipt, minus the lock (taking it again would self-deadlock). The Ariane
    /// adoption imports through it, so its create, import, save, reopen and link stamp
    /// are one span no command can land inside.
    pub(super) fn execute_tml_import_sections_held(
        &self,
        commands: &CommandsHeld<'_>,
        sections: Vec<crate::models::ImportSectionConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        section_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        self.execute_import_sections_locked(
            commands,
            sections,
            closures,
            file_hash,
            section_hashes,
            existing_shot_updates,
            mode,
            surface_elevation,
        )
    }

    fn execute_import_sections_inner(
        &self,
        sections: Vec<crate::models::ImportSectionConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        section_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let commands = self.hold_commands();
        self.execute_import_sections_locked(
            &commands,
            sections,
            closures,
            file_hash,
            section_hashes,
            existing_shot_updates,
            mode,
            surface_elevation,
        )
    }

    /// The import itself, inside the `command_lock` span `_commands` proves.
    fn execute_import_sections_locked(
        &self,
        _commands: &CommandsHeld<'_>,
        mut sections: Vec<crate::models::ImportSectionConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        section_hashes: Vec<String>,
        mut existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        use crate::models::{ImportPlacement, StationRef};
        use std::collections::{HashMap, HashSet};

        // Read-only edit gate (#191, D-B6): a TML import mutates the project, so
        // it is gated on a read-only remote project. (Empty-import early return
        // below is a no-op, but gating first keeps the chokepoint uniform.)
        if let Err(f) = self.guard_mutation("Import sections") {
            return CommandResult::refused(f);
        }

        if sections.is_empty() {
            // An EMPTY receipt, not no receipt. Nothing was imported, and that
            // is exactly what has to be said: every section of the file was
            // dropped (all shots excluded, or malformed) and the converter's
            // warnings explaining why are attached to the receipt by
            // `decorate_receipt`, which has nothing to attach them to when the
            // receipt is absent. Without this the user gets a successful,
            // silent, empty import.
            //
            // NB this return also discards `existing_shot_updates` (the DMP
            // wizard's overlap interpolation) - pre-existing, and unreachable
            // from any shipped caller, since the wizard only sends updates
            // alongside the sections they belong to. A future caller that
            // sends updates with NO sections has to move that block above
            // this early return, not just add a warning here.
            return CommandResult::success()
                .with_import_receipt(crate::core::command::ImportReceipt::empty());
        }

        // Azimuths are stored in `[0, 360)` (SPEC "Azimuths are stored in
        // `[0, 360)`"), and this is the one gate every importer passes, so the
        // wrap happens HERE (#576 B24). The DMP wizard builds its `ShotData` in
        // TypeScript and sends the device's headings verbatim, and a DMP heading
        // is a raw `i16 * 0.1` that a damaged dump can push to 400 or -30; the
        // overlap updates it sends for EXISTING shots carry the same readings.
        // Geometry is unchanged (400 and 40 are one bearing); what the wrap
        // prevents is the truncated-`%` antipode a pair more than 540 degrees
        // apart used to hit. A non-finite reading has no canonical form and is
        // left for the finiteness guards below.
        for shot in sections
            .iter_mut()
            .flat_map(|config| config.shots.iter_mut())
        {
            shot.azimuth_in = crate::models::normalize_azimuth(shot.azimuth_in);
            shot.azimuth_out = crate::models::normalize_azimuth(shot.azimuth_out);
        }
        for update in &mut existing_shot_updates {
            let updates = &mut update.updates;
            updates.azimuth_in = updates.azimuth_in.map(crate::models::normalize_azimuth);
            updates.azimuth_out = updates.azimuth_out.map(crate::models::normalize_azimuth);
        }

        // Reject non-finite geometry in overlap-interpolation updates before
        // any mutation: these arrive from the DMP import wizard's overlap
        // resolution, i.e. from parsed file content, and must hold the same
        // invariant as update_shot (#275).
        for update in &existing_shot_updates {
            if !update.updates.all_finite() {
                return CommandResult::failure(format!(
                    "Import shot update for section {} shot {} has a non-finite (NaN/inf) value",
                    update.section_id, update.shot_id
                ));
            }
        }

        // Per-section shot budget (#576 NF-AN2-8), the other half of the id
        // window the section-id budget below guards: a section's shot ids are
        // `section_id * FEATURE_ID_STRIDE + position`, so it holds at most
        // `MAX_SHOTS_PER_SECTION`. `create_section` refuses a longer one too,
        // but only once the loop reaches it - after every section before it
        // was built and validated, and then rolled back - so the batch is
        // checked whole, up front, like the section-id budget.
        if let Some(config) = sections
            .iter()
            .find(|config| config.shots.len() > MAX_SHOTS_PER_SECTION)
        {
            let name = config.name.as_deref().map_or_else(
                || "unnamed".to_string(),
                |n| crate::models::sanitize_display_text(n, 64),
            );
            return CommandResult::failure(format!(
                "Import refused: section \"{name}\" has {} shots, more than the \
                 {MAX_SHOTS_PER_SECTION} shots one section can hold (shot ids encode as \
                 section_id * {FEATURE_ID_STRIDE} + position). Nothing was imported.",
                config.shots.len()
            ));
        }

        // Section-id budget (#555), checked for the WHOLE batch before any
        // mutation. Every importer (Ariane, Compass, Walls, GPX, the DMP
        // wizard) funnels through here, and the loop below mints one section
        // per config via `create_section` -> `Project::add_section`, whose
        // `Section::new` computes `next_shot_id = id * FEATURE_ID_STRIDE`
        // unchecked. `create_section` refuses past the bound too, but only per
        // section: reaching it mid-loop would roll back a large import after
        // the work, and the file's own declared section count is knowable up
        // front, so a file that cannot fit is refused whole - the same
        // batch-atomic rule the non-finite checks above apply.
        let ids_already_spent = {
            let guard = self.project.lock();
            guard.as_ref().map(|p| p.next_section_id)
        };
        if let Some(spent) = ids_already_spent {
            let spent_count = spent as usize;
            if sections.len().saturating_add(spent_count) > MAX_SECTIONS_IN_ID_SPACE {
                return CommandResult::failure(format!(
                    "Import refused: this file would add {} sections and the project has \
                     already used {spent_count} section ids, which is past the limit of \
                     {MAX_SECTIONS_IN_ID_SPACE} sections. Shot ids encode as \
                     section_id * {FEATURE_ID_STRIDE} + position in 32 bits, so section \
                     ids past {MAX_SECTION_ID} cannot hold shots. Nothing was imported.",
                    sections.len()
                ));
            }
        }

        tracing::info!(
            count = sections.len(),
            purpose = "apply imported sections as single undo",
            "Executing import_sections as single undo unit"
        );

        // Take a project snapshot BEFORE any modifications
        let pre_had_critical = self.project_has_critical();
        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // [#576 A4] The frame-defining datum before the import, for the shared
        // epilogue's frame check. (An import's datum placement renormalises inline
        // in `Project::set_zero_datum`, so the check is a measured no-op today.)
        let frame_before = self.frame_datum_key();

        // Apply shot updates to existing sections FIRST (overlap interpolation).
        // This must happen before placing new sections so that FromExisting placements
        // compute station positions using the updated (interpolated) shot data.
        // Populated below when the doubly-anchored guard (F12b) skips shots, so
        // the receipt can surface what was silently kept in the trace logs.
        let mut doubly_anchored_shot_count: u32 = 0;
        if !existing_shot_updates.is_empty() {
            tracing::info!(
                count = existing_shot_updates.len(),
                purpose = "overlap interpolation",
                "Applying existing shot updates before placement",
            );
            let mut proj_guard = self.project.lock();
            if let Some(project) = proj_guard.as_mut() {
                // Collect unique affected section IDs
                let affected_section_ids: HashSet<SectionId> =
                    existing_shot_updates.iter().map(|u| u.section_id).collect();

                // Filter out updates for doubly-anchored sections.
                // If the modified shots sit between two positional constraints
                // (zero datums, aligned closures, station merges), modifying
                // the intermediate geometry would break one of them. We can only
                // compensate for a single anchor via offset adjustment, so when
                // two anchors straddle the modified range we must skip updates.
                let mut blocked_sections: HashSet<SectionId> = HashSet::new();
                for &sid in &affected_section_ids {
                    let section = match project.sections.iter().find(|s| s.id == sid) {
                        Some(s) => s,
                        None => continue,
                    };

                    // Find shot indices being modified on this section
                    let mut modified_indices: Vec<u32> = Vec::new();
                    for update in &existing_shot_updates {
                        if update.section_id != sid {
                            continue;
                        }
                        if let Some(idx) = section.shots.iter().position(|s| s.id == update.shot_id)
                        {
                            modified_indices.push(idx as u32);
                        }
                    }
                    let (Some(&min_shot), Some(&max_shot)) =
                        (modified_indices.iter().min(), modified_indices.iter().max())
                    else {
                        continue;
                    };

                    // Collect anchor station indices on this section.
                    // An anchor is any station whose absolute position is
                    // constrained by an immovable external relationship:
                    //   - zero datums on this section (always anchors)
                    //   - merges / aligned closures where the PARTNER can
                    //     independently reach a datum (without going through
                    //     this section).  A merge to a free-floating section
                    //     is NOT a positional constraint - the partner can
                    //     follow when this section's geometry changes.
                    let mut anchor_indices: Vec<u32> = Vec::new();

                    // Zero datums
                    for zd in &project.zero_datums {
                        if zd.station_ref.section_id == sid {
                            if let Some(sr) = project.resolve_key(&zd.station_ref) {
                                anchor_indices.push(sr.station_index);
                            }
                        }
                    }
                    // Station merges (only datum-backed partners)
                    for m in &project.station_merges {
                        if m.station_a.section_id == sid
                            && project.has_datum_path_excluding(m.station_b.section_id, sid)
                        {
                            if let Some(sr) = project.resolve_key(&m.station_a) {
                                anchor_indices.push(sr.station_index);
                            }
                        }
                        if m.station_b.section_id == sid
                            && project.has_datum_path_excluding(m.station_a.section_id, sid)
                        {
                            if let Some(sr) = project.resolve_key(&m.station_b) {
                                anchor_indices.push(sr.station_index);
                            }
                        }
                    }
                    // Aligned closures (only datum-backed partners)
                    for c in &project.loop_closures {
                        if !c.is_aligned {
                            continue;
                        }
                        if c.station_a.section_id == sid
                            && project.has_datum_path_excluding(c.station_b.section_id, sid)
                        {
                            if let Some(sr) = project.resolve_key(&c.station_a) {
                                anchor_indices.push(sr.station_index);
                            }
                        }
                        if c.station_b.section_id == sid
                            && project.has_datum_path_excluding(c.station_a.section_id, sid)
                        {
                            if let Some(sr) = project.resolve_key(&c.station_b) {
                                anchor_indices.push(sr.station_index);
                            }
                        }
                    }

                    // Check for anchors on both sides of the modified range.
                    // Shot i connects station i → station i+1, so stations
                    // 0..=min_shot are unaffected (front) and stations
                    // max_shot+1..end are on the back side.
                    let has_front = anchor_indices.iter().any(|&idx| idx <= min_shot);
                    let has_back = anchor_indices.iter().any(|&idx| idx > max_shot);

                    if has_front && has_back {
                        tracing::warn!(
                            section_id = sid,
                            min_shot,
                            max_shot,
                            anchors = ?anchor_indices,
                            purpose = "doubly-anchored guard",
                            "Skipping shot updates: modified range is between two anchored stations",
                        );
                        blocked_sections.insert(sid);
                        continue;
                    }

                    // (Connection-based constraints are handled post-update
                    //  by adjusting only the affected section's offset.)
                }

                doubly_anchored_shot_count = existing_shot_updates
                    .iter()
                    .filter(|u| blocked_sections.contains(&u.section_id))
                    .count() as u32;

                // Record anchor station positions BEFORE applying updates.
                // For each affected section that has a zero datum, we need to keep
                // the datum station fixed - otherwise modifying shots between station 0
                // and the datum shifts the datum's absolute position.
                let mut anchor_positions: HashMap<SectionId, (StationRef, Coordinates)> =
                    HashMap::new();
                for &sid in &affected_section_ids {
                    if blocked_sections.contains(&sid) {
                        continue;
                    }
                    let datum_sr = project
                        .zero_datums
                        .iter()
                        .find(|zd| zd.station_ref.section_id == sid)
                        .and_then(|zd| project.resolve_key(&zd.station_ref));
                    if let Some(sr) = datum_sr {
                        if let Some(coords) = project.compute_station_raw_coords(sr) {
                            tracing::debug!(
                                section_id = sid,
                                anchor_station = sr.station_index,
                                x = coords.x,
                                y = coords.y,
                                purpose = "zero datum stability",
                                "Recorded anchor position before shot updates",
                            );
                            anchor_positions.insert(sid, (sr, coords));
                        }
                    }
                }

                // Apply the shot updates (skipping doubly-anchored sections)
                for update in &existing_shot_updates {
                    if blocked_sections.contains(&update.section_id) {
                        continue;
                    }
                    if let Some(section) = project
                        .sections
                        .iter_mut()
                        .find(|s| s.id == update.section_id)
                    {
                        if let Some(shot) =
                            section.shots.iter_mut().find(|s| s.id == update.shot_id)
                        {
                            let old_len = shot.length;
                            if let Some(v) = update.updates.azimuth_in {
                                shot.azimuth_in = v;
                            }
                            if let Some(v) = update.updates.azimuth_out {
                                shot.azimuth_out = v;
                            }
                            if let Some(v) = update.updates.length {
                                shot.length = v;
                            }
                            if let Some(v) = update.updates.depth_in {
                                shot.depth_in = v;
                            }
                            if let Some(v) = update.updates.depth_out {
                                shot.depth_out = v;
                            }
                            if let Some(v) = update.updates.pitch_in {
                                shot.pitch_in = v;
                            }
                            if let Some(v) = update.updates.pitch_out {
                                shot.pitch_out = v;
                            }
                            if let Some(v) = update.updates.left {
                                shot.left = v;
                            }
                            if let Some(v) = update.updates.right {
                                shot.right = v;
                            }
                            if let Some(v) = update.updates.up {
                                shot.up = v;
                            }
                            if let Some(v) = update.updates.down {
                                shot.down = v;
                            }
                            tracing::debug!(
                                section_id = update.section_id,
                                shot_id = update.shot_id,
                                old_length = old_len,
                                new_length = shot.length,
                                purpose = "overlap interpolation",
                                "Updated existing shot",
                            );
                        } else {
                            tracing::warn!(
                                section_id = update.section_id,
                                shot_id = update.shot_id,
                                purpose = "overlap interpolation",
                                "Shot not found in section",
                            );
                        }
                    } else {
                        tracing::warn!(
                            section_id = update.section_id,
                            purpose = "overlap interpolation",
                            "Section not found in project",
                        );
                    }
                }

                // Compensate section offsets to keep zero datum stations fixed.
                // After modifying shots, the geometry between station 0 and the anchor
                // station has changed, so the anchor's computed position has drifted.
                // We adjust the section offset (and all connected sections in the rigid
                // group) by the inverse delta to pin the anchor back to its original spot.
                for (sid, (anchor_ref, old_coords)) in &anchor_positions {
                    if let Some(new_coords) = project.compute_station_raw_coords(*anchor_ref) {
                        let dx = old_coords.x - new_coords.x;
                        let dy = old_coords.y - new_coords.y;
                        // Decline a compensation that is float dust, on the same
                        // shared bound as the merge BFS and `cascade_bfs`
                        // (models/project/connectivity.rs). "The anchor drifted"
                        // is the same question here as there, so it gets the same
                        // answer: a gap of a few ULPs of the coordinates being
                        // summed is the residue of re-summing the survey chain,
                        // not a drift, and "compensating" it writes a
                        // `section_offsets` key for a rigid group that has not
                        // moved - a key that is structural, saved, and makes this
                        // route differ from the route with nothing to compensate.
                        // The bound is measured at the largest coordinates the app
                        // can hold, on `Project::OFFSET_DUST_M` (#469).
                        if dx.abs() > Project::OFFSET_DUST_M || dy.abs() > Project::OFFSET_DUST_M {
                            tracing::info!(
                                section_id = sid,
                                anchor_station = anchor_ref.station_index,
                                dx,
                                dy,
                                purpose = "zero datum stability",
                                "Compensating section offsets to keep zero datum fixed",
                            );
                            project.adjust_rigid_group_offsets(*sid, dx, dy);
                        }
                    }
                }

                // Cascade offset adjustments through drifted merge/closure
                // connections.  After shot updates and zero datum compensation,
                // some connections may be misaligned - the modified section's
                // station shifted but its partner didn't follow.
                let cascade_seeds: Vec<SectionId> = affected_section_ids
                    .iter()
                    .filter(|sid| !blocked_sections.contains(sid))
                    .copied()
                    .collect();
                project.cascade_connection_offsets(&cascade_seeds);
            }
        }

        // Mapping from import section_index to created SectionId
        let mut created_ids: HashMap<u32, u32> = HashMap::new();
        let mut all_section_ids: Vec<u32> = Vec::new();
        let mut all_warnings = Vec::new();
        // Collect commands for the undo entry description
        let mut batch_commands: Vec<Command> = Vec::new();
        // Name of the first section placed via `ImportPlacement::ZeroDatum`
        // (F12c): a GPS-anchored zero datum is a structural fact about the
        // import worth a receipt line, not just a trace log. `zero_datum_count`
        // tracks how many such placements this import made - a batch import
        // (e.g. a multi-fixed-station TML/Compass file) can create more than
        // one, and naming only the first without saying so would read as a
        // claim of completeness the receipt doesn't back up.
        let mut zero_datum_created: Option<String> = None;
        let mut zero_datum_count: u32 = 0;

        // Defer per-merge cascades during the placement loop. Each
        // `merge_stations` / `set_zero_datum` call would otherwise run a
        // full O(N) `cascade_connection_offsets`, turning a 1 200-section
        // tree-shaped import into ~O(N²) work. The cascade is purely
        // defensive against drift in pre-existing connections, and the
        // batch under us is creating its connections in dependency order
        // (topological sort happens during `convert_tml_to_import_configs`).
        // We arm the guard here, run the loop, then disarm + run one
        // final cascade after the loop - equivalent to the union of the
        // deferred per-call cascades.  The guard's Drop resets the flag
        // even on the panic-poisoned-mutex path so a partial import
        // can't leave `cascade_deferred = true` and silently skip
        // cascades on every subsequent command.
        let cascade_guard = CascadeDeferGuard::arm(&self.project);
        // Eagerly populate the section-id lookup so the per-call `get_section`
        // stays O(1) instead of degrading into an O(N) linear scan inside
        // `merge_stations`. (The merge adjacency needs no priming: it builds
        // itself on first use and stays warm across the import's pushes.)
        {
            let mut proj_guard = self.project.lock();
            if let Some(project) = proj_guard.as_mut() {
                project.rebuild_section_index();
                // Make `resolve_key`'s AfterShot lookup O(1) for the hot
                // merge/closure connectivity loops below - without this, a large
                // import (e.g. a big Walls project) pins a core for minutes.
                project.rebuild_shot_uuid_index();
            }
        }

        // [perf-debug] Time the per-section apply loop. Used to confirm
        // whether the dominant cost in execute_import_sections scales
        // linearly with section count (good) or super-linearly (bad -
        // a per-section graph traversal that re-walks already-imported
        // sections would show up here as quadratic growth).
        let loop_start = std::time::Instant::now();
        let mut create_total = std::time::Duration::ZERO;
        let mut placement_total = std::time::Duration::ZERO;
        let report_every = 100usize;
        let mut last_report_idx = 0usize;
        let mut last_report_elapsed = std::time::Duration::ZERO;

        for (idx, config) in sections.iter().enumerate() {
            let t_section_start = std::time::Instant::now();
            // Build SectionData from ImportSectionConfig.
            //
            // The reverse (#24) is applied HERE, before the section exists, so
            // placement and station-comment indices below resolve against the
            // final order - the contract every importer wizard builds against.
            // Left/right is never swapped on this path (no dialog to ask).
            let mut shots = config.shots.clone();
            // A reverse makes station 0 the far end, so the section's anchor has
            // to gain the span it used to cover or the passage lands a full
            // end-to-end displacement away from where the wizard put it.  The
            // in-app reverse does this via `adjust_section_offset`; do the same
            // here, measured on the pre-reverse order, and apply it once the
            // section exists.  Merge- and datum-based placements below re-anchor
            // the section outright, so for those it is inert - it is `Origin` and
            // `Manual` (which position station 0 directly) that would otherwise
            // silently move.
            // The section does not exist yet, so its north reference is the one
            // the importer declared on the config, falling back to the project
            // default - the same value written onto the section below (#438).
            // The compensation has to be measured in that frame, because the
            // coordinate walk it cancels is.
            // `sanitised` because the config is DESERIALIZED: serde builds the
            // struct field by field and honours no constructor, so a payload can
            // assert `north: true` with a correction of 5 or a NaN correction,
            // and every geometry path below would apply it. Same rule the
            // `.stalx` read path imposes on the stored columns (#438).
            let declared_reference = config.azimuth_reference.map(AzimuthReference::sanitised);
            // Read under the lock rather than through `get_project_state`, which
            // CLONES the whole project - once per section that declares no
            // reference, so an import of N such sections copied the project N
            // times (#576 NF-AN6-5). Same lock, same poisoned-lock fallback.
            let azimuth_reference = declared_reference.unwrap_or_else(|| {
                self.project
                    .lock()
                    .as_ref()
                    .and_then(|p| p.default_azimuth_reference)
                    .unwrap_or_default()
            });
            let reverse_offset = config.reverse.then(|| {
                let displacement = crate::models::shot_run_displacement(&shots, azimuth_reference);
                crate::models::reverse_shots(&mut shots, false);
                displacement
            });
            let section_data = SectionData {
                name: config.name.clone(),
                surveyor: config.surveyor.clone(),
                date: config.date,
                color: config.color.clone(),
                direction: config.direction,
                quality: config.quality,
                shots,
            };

            // Step 1: Create the section
            let t_create_start = std::time::Instant::now();
            let create_result = self.create_section(section_data.clone());
            create_total += t_create_start.elapsed();
            if !create_result.success {
                // Rollback to before-snapshot
                self.rollback_project(&before_snapshot);
                return CommandResult::failure(format!(
                    "Failed to create section \"{}\": {}",
                    config.name.as_deref().unwrap_or("unnamed"),
                    create_result.error.unwrap_or_default()
                ));
            }

            let Some(section_id) = create_result.section_id else {
                self.rollback_project(&before_snapshot);
                return CommandResult::failure("Section created but no ID returned".to_string());
            };

            // The shots were reversed before the section existed, so record that on the section
            // itself (#24). `reverse_parity` is documented as flipping whenever a section's
            // shot order is reversed, and the plan-anchor resolution reads it to decide which
            // side of a shot a terminal station sits on - leaving it false here would make two
            // geometrically identical projects ("import then reverse" vs "import with the
            // toggle") disagree about that, and about which end the least-squares gauge pins.
            if config.reverse || declared_reference.is_some() {
                {
                    let mut guard = self.project.lock();
                    if let Some(section) =
                        guard.as_mut().and_then(|p| p.get_section_mut(section_id))
                    {
                        if config.reverse {
                            section.reverse_parity = !section.reverse_parity;
                        }
                        // #438: what the file said its azimuths are measured
                        // against. Written here because `SectionData` cannot
                        // carry it through `create_section`, and the SANITISED
                        // value, so what the section holds is what the reverse
                        // compensation above was measured in.
                        if declared_reference.is_some() {
                            section.azimuth_reference = declared_reference;
                        }
                    }
                }
            }

            created_ids.insert(config.section_index, section_id);
            all_section_ids.push(section_id);
            all_warnings.extend(create_result.warnings);

            // Keep a reversed section where the un-reversed import would have put
            // it (see `reverse_offset` above).  Undo/redo for the whole import is
            // the before/after snapshot pair pushed at the end, so this needs no
            // command of its own.
            if let Some((dx, dy)) = reverse_offset {
                // Finiteness first - `NaN != 0.0` is true, and `section_offsets`
                // has no other finite guard (see `Project::reverse_section`).
                if dx.is_finite() && dy.is_finite() && (dx != 0.0 || dy != 0.0) {
                    let mut proj_guard = self.project.lock();
                    if let Some(project) = proj_guard.as_mut() {
                        project.adjust_section_offset(section_id, dx, dy);
                    }
                }
            }

            // Apply station comments (promoted from shot comments during TML import)
            if !config.station_comments.is_empty() {
                let mut proj_guard = self.project.lock();
                if let Some(project) = proj_guard.as_mut() {
                    let shot_count = config.shots.len() as u32;
                    for (station_index, comment) in &config.station_comments {
                        // Mirror here rather than trusting the caller to have done
                        // it. `reverse` renumbers station `i` to `shot_count - i`,
                        // and these indices were collected against the file's
                        // original order - the same rewrite the wizard applies to
                        // `placement` (src/utils/reversePlacement.ts). Doing it
                        // backend-side keeps the one importer that actually
                        // populates `station_comments` (TML) correct the day its
                        // wizard gains the toggle, instead of relying on a prose
                        // contract every future importer has to re-derive.
                        // `checked_sub`, not `saturating_sub`: an index past the
                        // last station has no mirror, and saturating it would
                        // park the comment on station 0 - a station it was never
                        // about. Un-reversed the same index simply fails
                        // `key_for_ref` below and the comment is dropped, so
                        // skipping here keeps the two paths honest.
                        let index = if config.reverse {
                            match shot_count.checked_sub(*station_index) {
                                Some(mirrored) => mirrored,
                                None => continue,
                            }
                        } else {
                            *station_index
                        };
                        let station_ref = StationRef::new(section_id, index);
                        if let Some(key) = project.key_for_ref(station_ref) {
                            project.station_comments.insert(key, comment.clone());
                        }
                    }
                }
            }

            // Record the shots as they were actually created (post-reverse), not
            // the raw config - otherwise a redo would recreate the section in the
            // file's original order.
            batch_commands.push(Command::CreateSection {
                name: config.name.clone(),
                surveyor: config.surveyor.clone(),
                date: config.date,
                color: config.color.clone(),
                direction: config.direction,
                shots: section_data.shots.clone(),
            });

            // Step 2: Apply placement
            let t_placement_start = std::time::Instant::now();
            match &config.placement {
                ImportPlacement::Origin => {
                    // No-op - section starts at origin
                }

                ImportPlacement::ZeroDatum {
                    coordinates,
                    station_index,
                } => {
                    let station_ref = StationRef::new(section_id, *station_index);
                    let result = self.set_zero_datum(station_ref, *coordinates);
                    if !result.success {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Failed to set zero datum for section \"{}\": {}",
                            config.name.as_deref().unwrap_or("unnamed"),
                            result.error.unwrap_or_default()
                        ));
                    }
                    all_warnings.extend(result.warnings);
                    zero_datum_count += 1;
                    if zero_datum_created.is_none() {
                        // Sanitized HERE, where the raw file-controlled name is
                        // born, not only in `decorate_receipt`: the DMP
                        // wizard's `import_sections` IPC command never calls
                        // `decorate_receipt` (it overwrites source_format/
                        // file_name/sections_found directly), so that route's
                        // receipt would otherwise carry a raw
                        // `ImportSectionConfig::name` straight from the parsed
                        // file end to end. `sanitize_display_text` is
                        // idempotent (filter then clamp), so decorate_receipt
                        // sanitizing this value again for the routes that DO
                        // call it is a harmless no-op, not a second transform.
                        zero_datum_created = Some(crate::models::sanitize_display_text(
                            &config
                                .name
                                .clone()
                                .unwrap_or_else(|| format!("Section {}", idx + 1)),
                            crate::core::command::MAX_FILE_NAME_LEN,
                        ));
                    }
                    batch_commands.push(Command::SetZeroDatum {
                        station_ref,
                        coordinates: *coordinates,
                    });
                }

                ImportPlacement::FromExisting {
                    import_station,
                    existing_station,
                } => {
                    let import_ref = StationRef::new(section_id, *import_station);
                    let result = self.merge_stations(import_ref, *existing_station);
                    if !result.success {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Failed to merge section \"{}\": {}",
                            config.name.as_deref().unwrap_or("unnamed"),
                            result.error.unwrap_or_default()
                        ));
                    }
                    all_warnings.extend(result.warnings);
                    batch_commands.push(Command::MergeStations {
                        station_a: import_ref,
                        station_b: *existing_station,
                    });
                }

                ImportPlacement::FromImporting {
                    import_station,
                    target_section_index,
                    target_station_index,
                } => {
                    let Some(&target_section_id) = created_ids.get(target_section_index) else {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Section \"{}\" references import section index {} which has not been created yet",
                            config.name.as_deref().unwrap_or("unnamed"),
                            target_section_index
                        ));
                    };
                    let import_ref = StationRef::new(section_id, *import_station);
                    let target_ref = StationRef::new(target_section_id, *target_station_index);
                    let result = self.merge_stations(import_ref, target_ref);
                    if !result.success {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Failed to merge section \"{}\" with imported section: {}",
                            config.name.as_deref().unwrap_or("unnamed"),
                            result.error.unwrap_or_default()
                        ));
                    }
                    all_warnings.extend(result.warnings);
                    batch_commands.push(Command::MergeStations {
                        station_a: import_ref,
                        station_b: target_ref,
                    });
                }

                ImportPlacement::Manual {
                    align_station_index: _,
                    offset,
                } => {
                    if let Some(offset) = offset {
                        let result = self.move_section(section_id, *offset);
                        if !result.success {
                            self.rollback_project(&before_snapshot);
                            return CommandResult::failure(format!(
                                "Failed to move section \"{}\": {}",
                                config.name.as_deref().unwrap_or("unnamed"),
                                result.error.unwrap_or_default()
                            ));
                        }
                        all_warnings.extend(result.warnings);
                        batch_commands.push(Command::MoveSection {
                            section_id,
                            offset: *offset,
                        });
                    }
                }

                ImportPlacement::ManualSnap {
                    align_station_index,
                    snapped_to_station,
                } => {
                    let snap_index = (*align_station_index).min(config.shots.len() as u32);
                    let import_ref = StationRef::new(section_id, snap_index);
                    let result = self.merge_stations(import_ref, *snapped_to_station);
                    if !result.success {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Failed to snap section \"{}\": {}",
                            config.name.as_deref().unwrap_or("unnamed"),
                            result.error.unwrap_or_default()
                        ));
                    }
                    all_warnings.extend(result.warnings);
                    batch_commands.push(Command::MergeStations {
                        station_a: import_ref,
                        station_b: *snapped_to_station,
                    });
                }
            }
            placement_total += t_placement_start.elapsed();
            let _ = t_section_start;

            // [perf-debug] Periodic log: emit every `report_every` sections.
            // The "recent" delta highlights super-linear growth - if each
            // 100-section batch is taking longer than the last, a
            // per-section graph traversal is to blame.
            if (idx + 1) % report_every == 0 || idx + 1 == sections.len() {
                let now_elapsed = loop_start.elapsed();
                let batch = now_elapsed.saturating_sub(last_report_elapsed);
                let batch_n = (idx + 1) - last_report_idx;
                // Per-batch import progress is debug-only - it's
                // perf-investigation noise, not something a release
                // log should record.
                tracing::debug!(
                    sections_done = idx + 1,
                    total = sections.len(),
                    batch_n,
                    batch_ms = batch.as_millis() as u64,
                    total_elapsed_ms = now_elapsed.as_millis() as u64,
                    create_ms = create_total.as_millis() as u64,
                    placement_ms = placement_total.as_millis() as u64,
                    purpose = "TML import loop timing",
                    "section import progress"
                );
                last_report_idx = idx + 1;
                last_report_elapsed = now_elapsed;
            }
        }

        tracing::debug!(
            sections = sections.len(),
            total_ms = loop_start.elapsed().as_millis() as u64,
            create_ms = create_total.as_millis() as u64,
            placement_ms = placement_total.as_millis() as u64,
            purpose = "TML import loop summary",
            "execute_import_sections per-section loop done"
        );

        // Lift the deferral (clears the flag now so `cascade_connection_offsets`
        // is allowed to run) and execute ONE cascade across every newly
        // created section. This propagates any drift that would have
        // been caught by the per-call cascades - for a tree-shaped
        // import (no closures yet) this is a near-no-op single pass
        // that early-exits on the first iteration.  `disarm` consumes
        // the guard so its Drop becomes a no-op for the success path.
        cascade_guard.disarm();
        let cascade_start = std::time::Instant::now();
        {
            let mut proj_guard = self.project.lock();
            if let Some(project) = proj_guard.as_mut() {
                project.cascade_connection_offsets(&all_section_ids);
            }
        }
        tracing::debug!(
            sections = all_section_ids.len(),
            cascade_ms = cascade_start.elapsed().as_millis() as u64,
            purpose = "TML import batch cascade",
            "final cascade after deferred placement loop"
        );

        // Create loop closures from TML CLOSURE shots (non-fatal). Resolve every
        // pair first (in order), then create them in ONE batch that builds the
        // closure-graph invariants once instead of rebuilding the whole graph per
        // closure - the dominant cost on a large import.
        let mut closure_pairs: Vec<(StationRef, StationRef)> = Vec::with_capacity(closures.len());
        let mut closures_skipped: u32 = 0;
        let mut closures_failed: u32 = 0;
        // Counted directly from the batch results (Ok arms), never derived by
        // subtracting `closures_failed` from the requested count: when the
        // project lock is unavailable, `create_loop_closures_batch` never runs,
        // `results` is empty, and `closure_pairs.iter().zip(results)` yields
        // nothing - so `closures_failed` would silently stay 0 and a
        // subtraction-based count would over-report every pair as created.
        let mut closures_created: u32 = 0;
        for closure in &closures {
            let Some(&from_section_id) = created_ids.get(&closure.from_section_index) else {
                tracing::warn!(
                    from_section_index = closure.from_section_index,
                    purpose = "import loop closures",
                    "Closure source section not found in created sections, skipping"
                );
                closures_skipped += 1;
                continue;
            };
            let Some(&to_section_id) = created_ids.get(&closure.to_section_index) else {
                tracing::warn!(
                    to_section_index = closure.to_section_index,
                    purpose = "import loop closures",
                    "Closure target section not found in created sections, skipping"
                );
                closures_skipped += 1;
                continue;
            };
            closure_pairs.push((
                StationRef::new(from_section_id, closure.from_station_index),
                StationRef::new(to_section_id, closure.to_station_index),
            ));
        }
        if !closure_pairs.is_empty() {
            let results = {
                let mut proj_guard = self.project.lock();
                match proj_guard.as_mut() {
                    Some(project) => project.create_loop_closures_batch(&closure_pairs),
                    None => Vec::new(),
                }
            };
            let (created, failed) = tally_closure_results(&closure_pairs, results);
            closures_created += created;
            closures_failed += failed;
        }

        // Track file hash if provided (before after-snapshot so it's part of the undo)
        if let Some(ref hash) = file_hash {
            let track_result = self.track_imported_file(hash.clone());
            if !track_result.success {
                tracing::warn!(
                    purpose = "prevent duplicate imports",
                    error = ?track_result.error,
                    "Failed to track imported file hash"
                );
                // Non-fatal: don't rollback the entire import for a hash tracking failure
            }
        }

        // Track section content hashes (before after-snapshot so undo reverts them)
        if !section_hashes.is_empty() {
            let track_result = self.track_section_hashes(section_hashes);
            if !track_result.success {
                tracing::warn!(
                    purpose = "prevent duplicate section imports",
                    error = ?track_result.error,
                    "Failed to track imported section hashes"
                );
            }
        }

        // Split this file's virtual shots into Jump vs Reference (#27). Runs
        // here, inside the command, because the chain rule needs the graph the
        // loop above just built (sections placed, merges and closures created)
        // AND because everything it writes has to be inside the `after` snapshot
        // captured below, or redo would resurrect the unclassified state.
        //
        // Scoped to the sections this import created: the read stays
        // project-wide (a new run is usually only bounded *through* a merge into
        // existing survey), but a per-file setting must never rewrite a kind the
        // user hand-set in an earlier session.
        {
            let imported: HashSet<SectionId> = all_section_ids.iter().copied().collect();
            let scope = crate::models::ClassifyScope::Sections(&imported);
            let mut proj_guard = self.project.lock();
            if let Some(project) = proj_guard.as_mut() {
                let changed = match mode {
                    VirtualShotMode::Auto => project.classify_virtual_shots(&scope),
                    VirtualShotMode::AllJumps => {
                        project.set_all_virtual_shots(ShotKind::Jump, &scope)
                    }
                    VirtualShotMode::AllReferences => {
                        project.set_all_virtual_shots(ShotKind::Reference, &scope)
                    }
                };
                if changed > 0 {
                    tracing::info!(
                        changed,
                        ?mode,
                        purpose = "assign Jump/Reference kinds at import finalize (#27)",
                        "Classified imported virtual shots"
                    );
                }
            }
        }

        // [#576 A4] The shared forward epilogue: the frame check and the reconcile of
        // derived offsets, then - inside `record` - the corruption check, the
        // entrance-elevation merge, the after-snapshot and one undo entry covering
        // all sections + hash, then the dirty flip. An import only ADDS
        // sections/merges/closures (its closures are loops over the spanning-tree
        // merges, never sole bridges), so it can't strand a section behind a
        // closure - which is why it runs no sole-bridge conversion at all.
        let mut surface_elevation_warning = None;
        let recorded = self.forward_epilogue(frame_before, EpilogueSteps::GEOMETRY, || {
            // Block corruption at the source: reject the whole import if it produced
            // critical corruption (dangling refs / non-finite geometry) rather than
            // committing and later persisting it. This custom path bypasses
            // execute()'s guard, so it runs the same check itself (#integrity Root 4
            // / catalog #19).
            if !pre_had_critical && self.project_has_critical() {
                self.rollback_project(&before_snapshot);
                tracing::error!(
                    purpose = "block corruption at the source",
                    "TML import would corrupt survey structure; rolled back, nothing imported"
                );
                return Err(CommandResult::failure(
                    "Blocked: the imported data would corrupt the survey structure \
                     (dangling references or invalid geometry). Nothing was imported."
                        .to_string(),
                ));
            }

            // Merge an Ariane import's project-level entrance elevation (#431)
            // before the after-snapshot, so it undoes/redoes atomically with the
            // rest of the import - the same reason `mode`'s virtual-shot pass
            // runs here rather than after (see the doc comment above). No-clobber:
            // an existing different value wins, and the mismatch becomes a receipt
            // warning naming both numbers.
            surface_elevation_warning = surface_elevation.and_then(|incoming| {
                let mut proj_guard = self.project.lock();
                let project = proj_guard.as_mut()?;
                match project.surface_elevation {
                    None => {
                        project.surface_elevation = Some(incoming);
                        None
                    }
                    // 0.005 m matches the message's own `{:.2}` precision - a
                    // smaller difference (e.g. the same elevation re-imported
                    // metric then imperial: 100.0 m vs 328.084 ft -> 100.00003 m)
                    // must not fire a "differs" warning that prints the same
                    // rounded number on both sides.
                    Some(existing) if (existing - incoming).abs() > 0.005 => Some(format!(
                        "Imported file's entrance elevation ({incoming:.2} m) differs from \
                         the project's existing value ({existing:.2} m); kept {existing:.2} m"
                    )),
                    Some(_) => None,
                }
            });

            // Capture after-snapshot for redo support
            let after_snapshot = self.capture_after_snapshot_or_rollback(&before_snapshot)?;
            // Push a single undo entry covering all sections + hash
            self.push_snapshot_undo(
                Command::Batch {
                    commands: batch_commands,
                },
                before_snapshot,
                after_snapshot,
            );
            Ok(true)
        });
        if let Err(failure) = recorded {
            return failure;
        }

        let shot_count: u32 = sections.iter().map(|s| s.shots.len() as u32).sum();
        let closures_requested = closures.len() as u32;
        let mut receipt_warnings = Vec::new();
        if let Some(warning) = surface_elevation_warning {
            receipt_warnings.push(warning);
        }
        if doubly_anchored_shot_count > 0 {
            receipt_warnings.push(format!(
                "{doubly_anchored_shot_count} shot{} kept {} position{}: section anchored on both sides",
                if doubly_anchored_shot_count == 1 { "" } else { "s" },
                if doubly_anchored_shot_count == 1 { "its" } else { "their" },
                if doubly_anchored_shot_count == 1 { "" } else { "s" },
            ));
        }
        let closures_dropped = closures_skipped + closures_failed;
        if closures_dropped > 0 {
            receipt_warnings.push(format!(
                "{closures_dropped} loop closure{} could not be created and {} skipped",
                if closures_dropped == 1 { "" } else { "s" },
                if closures_dropped == 1 { "was" } else { "were" }
            ));
        }
        // "and N more": naming only the FIRST zero-datum section without saying
        // so would read as a claim that it was the only one this import made.
        let zero_datum_created = zero_datum_created.map(|name| {
            if zero_datum_count > 1 {
                format!("{name} (and {} more)", zero_datum_count - 1)
            } else {
                name
            }
        });
        let mut receipt = crate::core::command::ImportReceipt {
            source_format: String::new(),
            file_name: String::new(),
            sections_found: 0,
            sections_imported: all_section_ids.len() as u32,
            shot_count,
            closures_requested,
            closures_created,
            unit_conversion: None,
            warnings: receipt_warnings,
            info: Vec::new(),
            zero_datum_created,
            hidden_warnings: 0,
        };
        receipt.cap_warnings();

        let result = CommandResult::success()
            .with_section_ids(all_section_ids)
            .with_import_receipt(receipt);
        if all_warnings.is_empty() {
            result
        } else {
            result.with_warnings(all_warnings)
        }
    }
}

#[cfg(test)]
mod tally_closure_results_tests {
    use super::tally_closure_results;
    use crate::models::StationRef;

    /// The empty-`results` branch: `create_loop_closures_batch` never ran (the
    /// project-lock-unavailable arm returns `Vec::new()`), so `results` is
    /// empty even though `closure_pairs` is not. Must report zero created and
    /// zero failed, never derive "all created" from a stale subtraction.
    #[test]
    fn empty_results_reports_zero_created_and_zero_failed() {
        let pairs = vec![
            (StationRef::new(1, 0), StationRef::new(2, 0)),
            (StationRef::new(3, 0), StationRef::new(4, 0)),
        ];
        let (created, failed) = tally_closure_results(&pairs, Vec::new());
        assert_eq!(created, 0);
        assert_eq!(failed, 0);
    }
}
