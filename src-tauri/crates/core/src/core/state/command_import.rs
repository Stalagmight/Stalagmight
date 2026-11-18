use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    AzimuthReference, ClosureStrategy, Coordinates, LineData, LineId, Project, SectionId, ShotData,
    ShotKind, StationRef, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
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
use std::sync::Mutex;

/// Which per-station text map an import config entry lands in.
#[derive(Clone, Copy)]
enum StationText {
    Comment,
    Name,
}

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
/// `execute_import_lines` flips the flag on so each per-line
/// `merge_stations` skips its O(N) cascade, then runs ONE cascade at the
/// end.  If the apply loop panics (poisoning the mutex) or any code path
/// returns early without clearing the flag, every subsequent command
/// silently skips its cascade and accumulates line-offset drift.  The
/// guard's `Drop` resets the flag to `false` even through a poisoned
/// mutex, closing that window.
///
/// Call [`Self::disarm`] before the final cascade so the cascade is
/// actually allowed to run (the cascade itself early-exits when the flag
/// is still `true`).  After `disarm`, `Drop` is a no-op.
struct CascadeDeferGuard<'a> {
    project: &'a Mutex<Option<Project>>,
    armed: bool,
}

impl<'a> CascadeDeferGuard<'a> {
    /// Set `cascade_deferred = true` on the project (if present) and
    /// return a guard.  If the lock is already poisoned the flag is set
    /// anyway via `into_inner` - the import is about to run regardless,
    /// and we still want the Drop reset to fire.
    fn arm(project: &'a Mutex<Option<Project>>) -> Self {
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

    fn set_flag(project: &Mutex<Option<Project>>, value: bool) {
        let mut guard = match project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
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
    /// Rename the shot UUIDs of `new_lines` that another line of the open
    /// project already holds ([`crate::models::Project::reassign_repeated_shot_uuids`]):
    /// the project keeps its ids, the newly created lines yield.
    pub(super) fn reassign_repeated_shot_uuids(&self, new_lines: &[LineId]) {
        if new_lines.is_empty() {
            return;
        }
        // Recover a poisoned lock, as the import loop around this does: skipping
        // the rename would leave a repeat the integrity epilogue then reads as
        // corruption, refusing the whole import.
        let mut guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(project) = guard.as_mut() {
            project.reassign_repeated_shot_uuids(new_lines);
        }
    }

    /// Execute a create-line-and-merge operation as a single atomic undo/redo unit.
    /// This is used when importing a new line and immediately merging one of its stations
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
        closure_strategy: ClosureStrategy,
    ) -> CommandResult {
        self.execute_create_and_merge_in_cave_section(
            name,
            surveyor,
            date,
            color,
            direction,
            shots,
            import_station_index,
            target_station,
            closure_strategy,
            None,
        )
    }

    /// [`Self::execute_create_and_merge`] that also puts the new line in the
    /// Section `cave_section_id` (#327), inside the same undo step: a New
    /// Line placed by snapping onto a station is ONE undo, Section included.
    /// A Section that no longer exists refuses the whole step before anything
    /// changes, as `CreateLine` does (`create_line_command`).
    pub fn execute_create_and_merge_in_cave_section(
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
        cave_section_id: Option<SectionId>,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): a TML create+merge mutates the
        // project, so it is gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Import line and merge") {
            return f;
        }
        if let Some(section_id) = cave_section_id {
            let exists = self
                .project
                .lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(|p| p.section(section_id).is_some()));
            match exists {
                Some(true) => {}
                Some(false) => {
                    return CommandResult::failure(
                        "The section for the new line no longer exists; it may have been deleted"
                            .to_string(),
                    )
                }
                None => return CommandResult::failure("No project open".to_string()),
            }
        }
        tracing::info!(
            purpose = "keep undo atomic for import merge",
            "Executing create_line_and_merge as single undo unit"
        );

        // Take a project snapshot BEFORE any modifications
        let snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Step 1: Create the line (using internal method)
        let line_data = LineData {
            name: name.clone(),
            surveyor: surveyor.clone(),
            date,
            color: color.clone(),
            direction,
            quality: 1.0,
            shots: shots.clone(),
        };

        let create_result = self.create_line(line_data);
        if !create_result.success {
            return create_result;
        }

        let Some(new_line_id) = create_result.line_id else {
            return CommandResult::failure("Line created but no ID returned".to_string());
        };
        // The shots are the caller's (`ShotData::uuid` is not stripped at this
        // door), so an id the project already holds is renamed before the merge
        // below anchors on it.
        self.reassign_repeated_shot_uuids(&[new_line_id]);

        // Step 2: Create the station reference for the newly created line
        let import_station_ref = StationRef {
            line_id: new_line_id,
            station_index: import_station_index,
        };

        // Step 3: Merge the stations
        let merge_result = self.merge_stations(import_station_ref, target_station);
        if !merge_result.success {
            // Merge failed after line was created - rollback to pre-mutation snapshot
            // to avoid leaving a created line with no undo entry.
            tracing::error!(
                purpose = "keep project consistent after failed merge",
                "Merge failed after line creation; rolling back to pre-mutation snapshot"
            );
            self.rollback_project(&snapshot);
            return merge_result;
        }

        // The new line joins its Section before the after snapshot, so the
        // one undo entry covers it too.
        if let Some(section_id) = cave_section_id {
            if let Ok(mut guard) = self.project.lock() {
                if let Some(project) = guard.as_mut() {
                    project.set_line_section(new_line_id, Some(section_id));
                }
            }
        }

        // Step 4: Capture the after snapshot and push undo entry with before+after.
        let after_snapshot = match self.capture_after_snapshot_or_rollback(&snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Create a Batch command that represents both operations for the
        // description, named by the line and the station it was snapped onto.
        let label = self.batch_label(|project| {
            crate::core::undo::history_batch::create_line_at(project, new_line_id, target_station)
        });
        let batch_cmd = Command::Batch {
            commands: vec![
                Command::CreateLine {
                    name,
                    surveyor,
                    date,
                    color,
                    direction,
                    shots,
                    cave_section_id,
                },
                Command::MergeStations {
                    station_a: import_station_ref,
                    station_b: target_station,
                },
            ],
            label,
        };

        self.push_snapshot_undo(batch_cmd, snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): a create+merge import mutated a remote
        // project. Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        // Combine warnings from both operations
        let mut all_warnings = create_result.warnings;
        all_warnings.extend(merge_result.warnings);

        // Return success with the new line ID
        let result = CommandResult::success().with_line_id(new_line_id);
        if all_warnings.is_empty() {
            result
        } else {
            result.with_warnings(all_warnings)
        }
    }

    /// Execute a batch import of lines as a single atomic undo/redo unit.
    /// Creates all lines with their placements (merges, zero datums, moves)
    /// and optionally tracks the file hash. Uses `ProjectSnapshotPair` so a
    /// single undo reverts everything including the hash.
    ///
    /// `mode` picks how this file's virtual shots are split into Jump vs
    /// Reference (#27). The pass runs inside this command, before the `after`
    /// snapshot: doing it afterwards (the first cut) left the classified kinds
    /// out of the redo state, so undo-then-redo of an import silently reverted
    /// every shot to the parser's conservative `Reference` seed and dropped the
    /// far side of each jump out of gas and penetration.
    pub fn execute_import_lines(
        &self,
        lines: Vec<crate::models::ImportLineConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
    ) -> CommandResult {
        self.execute_import_lines_inner(
            lines,
            closures,
            file_hash,
            line_hashes,
            existing_shot_updates,
            mode,
            None,
        )
    }

    /// Same as [`Self::execute_import_lines`], but also merges an Ariane
    /// file's project-level entrance elevation (#431) into `Project::surface_elevation`
    /// atomically with the import, so undo/redo covers both. The only caller
    /// that has this value is the Ariane TML/TMLU importer.
    pub fn execute_tml_import_sections(
        &self,
        lines: Vec<crate::models::ImportLineConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        self.execute_import_lines_inner(
            lines,
            closures,
            file_hash,
            line_hashes,
            existing_shot_updates,
            mode,
            surface_elevation,
        )
    }

    fn execute_import_lines_inner(
        &self,
        lines: Vec<crate::models::ImportLineConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        use crate::models::{ImportPlacement, StationRef};
        use std::collections::{HashMap, HashSet};

        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): a TML import mutates the project, so
        // it is gated on a read-only remote project. (Empty-import early return
        // below is a no-op, but gating first keeps the chokepoint uniform.)
        if let Err(f) = self.guard_mutation("Import lines") {
            return f;
        }

        if lines.is_empty() {
            // An EMPTY receipt, not no receipt. Nothing was imported, and that
            // is exactly what has to be said: every line of the file was
            // dropped (all shots excluded, or malformed) and the converter's
            // warnings explaining why are attached to the receipt by
            // `decorate_receipt`, which has nothing to attach them to when the
            // receipt is absent. Without this the user gets a successful,
            // silent, empty import.
            //
            // NB this return also discards `existing_shot_updates` (the DMP
            // wizard's overlap interpolation) - pre-existing, and unreachable
            // from any shipped caller, since the wizard only sends updates
            // alongside the lines they belong to. A future caller that
            // sends updates with NO lines has to move that block above
            // this early return, not just add a warning here.
            return CommandResult::success()
                .with_import_receipt(crate::core::command::ImportReceipt::empty());
        }

        // Reject non-finite geometry in overlap-interpolation updates before
        // any mutation: these arrive from the DMP import wizard's overlap
        // resolution, i.e. from parsed file content, and must hold the same
        // invariant as update_shot (#275).
        for update in &existing_shot_updates {
            if !update.updates.all_finite() {
                return CommandResult::failure(format!(
                    "Import shot update for line {} shot {} has a non-finite (NaN/inf) value",
                    update.line_id, update.shot_id
                ));
            }
        }

        // Line-id budget (#555), checked for the WHOLE batch before any
        // mutation. Every importer (Ariane, Compass, Walls, GPX, the DMP
        // wizard) funnels through here, and the loop below mints one line
        // per config via `create_line` -> `Project::add_line`, whose
        // `Line::new` computes `next_shot_id = id * FEATURE_ID_STRIDE`
        // unchecked. `create_line` refuses past the bound too, but only per
        // line: reaching it mid-loop would roll back a large import after
        // the work, and the file's own declared line count is knowable up
        // front, so a file that cannot fit is refused whole - the same
        // batch-atomic rule the non-finite checks above apply.
        let ids_already_spent = match self.project.lock() {
            Ok(guard) => guard.as_ref().map(|p| p.next_line_id),
            Err(poisoned) => poisoned.into_inner().as_ref().map(|p| p.next_line_id),
        };
        if let Some(spent) = ids_already_spent {
            let spent_count = spent as usize;
            if lines.len().saturating_add(spent_count) > MAX_LINES_IN_ID_SPACE {
                return CommandResult::failure(format!(
                    "Import refused: this file would add {} lines and the project has \
                     already used {spent_count} line ids, which is past the limit of \
                     {MAX_LINES_IN_ID_SPACE} lines. Shot ids encode as \
                     line_id * {FEATURE_ID_STRIDE} + position in 32 bits, so line \
                     ids past {MAX_LINE_ID} cannot hold shots. Nothing was imported.",
                    lines.len()
                ));
            }
        }

        tracing::info!(
            count = lines.len(),
            purpose = "apply imported lines as single undo",
            "Executing import_lines as single undo unit"
        );

        // Take a project snapshot BEFORE any modifications
        let pre_had_critical = self.project_has_critical();
        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Apply shot updates to existing lines FIRST (overlap interpolation).
        // This must happen before placing new lines so that FromExisting placements
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
            let mut proj_guard = match self.project.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(project) = proj_guard.as_mut() {
                // Collect unique affected line IDs
                let affected_line_ids: HashSet<LineId> =
                    existing_shot_updates.iter().map(|u| u.line_id).collect();

                // Filter out updates for doubly-anchored lines.
                // If the modified shots sit between two positional constraints
                // (zero datums, aligned closures, station merges), modifying
                // the intermediate geometry would break one of them. We can only
                // compensate for a single anchor via offset adjustment, so when
                // two anchors straddle the modified range we must skip updates.
                let mut blocked_lines: HashSet<LineId> = HashSet::new();
                for &sid in &affected_line_ids {
                    let line = match project.lines.iter().find(|s| s.id == sid) {
                        Some(s) => s,
                        None => continue,
                    };

                    // Find shot indices being modified on this line
                    let mut modified_indices: Vec<u32> = Vec::new();
                    for update in &existing_shot_updates {
                        if update.line_id != sid {
                            continue;
                        }
                        if let Some(idx) = line.shots.iter().position(|s| s.id == update.shot_id) {
                            modified_indices.push(idx as u32);
                        }
                    }
                    let (Some(&min_shot), Some(&max_shot)) =
                        (modified_indices.iter().min(), modified_indices.iter().max())
                    else {
                        continue;
                    };

                    // Collect anchor station indices on this line.
                    // An anchor is any station whose absolute position is
                    // constrained by an immovable external relationship:
                    //   - placement zero datums on this line (always
                    //     anchors; a GPS fix pins nothing in raw geometry, #741)
                    //   - merges / aligned closures where the PARTNER can
                    //     independently reach a datum (without going through
                    //     this line).  A merge to a free-floating line
                    //     is NOT a positional constraint - the partner can
                    //     follow when this line's geometry changes.
                    let mut anchor_indices: Vec<u32> = Vec::new();

                    // Placement zero datums
                    for sr in project.placement_datum_refs() {
                        if sr.line_id == sid {
                            anchor_indices.push(sr.station_index);
                        }
                    }
                    // Station merges (only datum-backed partners)
                    for m in &project.station_merges {
                        if m.station_a.line_id == sid
                            && project.has_datum_path_excluding(m.station_b.line_id, sid)
                        {
                            if let Some(sr) = project.resolve_key(&m.station_a) {
                                anchor_indices.push(sr.station_index);
                            }
                        }
                        if m.station_b.line_id == sid
                            && project.has_datum_path_excluding(m.station_a.line_id, sid)
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
                        if c.station_a.line_id == sid
                            && project.has_datum_path_excluding(c.station_b.line_id, sid)
                        {
                            if let Some(sr) = project.resolve_key(&c.station_a) {
                                anchor_indices.push(sr.station_index);
                            }
                        }
                        if c.station_b.line_id == sid
                            && project.has_datum_path_excluding(c.station_a.line_id, sid)
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
                            line_id = sid,
                            min_shot,
                            max_shot,
                            anchors = ?anchor_indices,
                            purpose = "doubly-anchored guard",
                            "Skipping shot updates: modified range is between two anchored stations",
                        );
                        blocked_lines.insert(sid);
                        continue;
                    }

                    // (Connection-based constraints are handled post-update
                    //  by adjusting only the affected line's offset.)
                }

                doubly_anchored_shot_count = existing_shot_updates
                    .iter()
                    .filter(|u| blocked_lines.contains(&u.line_id))
                    .count() as u32;

                // Record anchor station positions BEFORE applying updates.
                // For each affected line that has a zero datum, we need to keep
                // the datum station fixed - otherwise modifying shots between station 0
                // and the datum shifts the datum's absolute position.
                let mut anchor_positions: HashMap<LineId, (StationRef, Coordinates)> =
                    HashMap::new();
                for &sid in &affected_line_ids {
                    if blocked_lines.contains(&sid) {
                        continue;
                    }
                    // The line's PLACEMENT datum: holding a GPS fix still
                    // would drag its whole placed group by the edit (#741).
                    let datum_sr = project
                        .placement_datum_refs()
                        .into_iter()
                        .find(|sr| sr.line_id == sid);
                    if let Some(sr) = datum_sr {
                        if let Some(coords) = project.compute_station_raw_coords(sr) {
                            tracing::debug!(
                                line_id = sid,
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

                // Apply the shot updates (skipping doubly-anchored lines)
                for update in &existing_shot_updates {
                    if blocked_lines.contains(&update.line_id) {
                        continue;
                    }
                    if let Some(line) = project.lines.iter_mut().find(|s| s.id == update.line_id) {
                        if let Some(shot) = line.shots.iter_mut().find(|s| s.id == update.shot_id) {
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
                                line_id = update.line_id,
                                shot_id = update.shot_id,
                                old_length = old_len,
                                new_length = shot.length,
                                purpose = "overlap interpolation",
                                "Updated existing shot",
                            );
                        } else {
                            tracing::warn!(
                                line_id = update.line_id,
                                shot_id = update.shot_id,
                                purpose = "overlap interpolation",
                                "Shot not found in line",
                            );
                        }
                    } else {
                        tracing::warn!(
                            line_id = update.line_id,
                            purpose = "overlap interpolation",
                            "Line not found in project",
                        );
                    }
                }

                // Compensate line offsets to keep zero datum stations fixed.
                // After modifying shots, the geometry between station 0 and the anchor
                // station has changed, so the anchor's computed position has drifted.
                // We adjust the line offset (and all connected lines in the rigid
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
                        // `line_offsets` key for a rigid group that has not
                        // moved - a key that is structural, saved, and makes this
                        // route differ from the route with nothing to compensate.
                        // The bound is measured at the largest coordinates the app
                        // can hold, on `Project::OFFSET_DUST_M` (#469).
                        if dx.abs() > Project::OFFSET_DUST_M || dy.abs() > Project::OFFSET_DUST_M {
                            tracing::info!(
                                line_id = sid,
                                anchor_station = anchor_ref.station_index,
                                dx,
                                dy,
                                purpose = "zero datum stability",
                                "Compensating line offsets to keep zero datum fixed",
                            );
                            project.adjust_rigid_group_offsets(*sid, dx, dy);
                        }
                    }
                }

                // Cascade offset adjustments through drifted merge/closure
                // connections.  After shot updates and zero datum compensation,
                // some connections may be misaligned - the modified line's
                // station shifted but its partner didn't follow.
                let cascade_seeds: Vec<LineId> = affected_line_ids
                    .iter()
                    .filter(|sid| !blocked_lines.contains(sid))
                    .copied()
                    .collect();
                project.cascade_connection_offsets(&cascade_seeds);
            }
        }

        // Mapping from import line_index to created LineId
        let mut created_ids: HashMap<u32, u32> = HashMap::new();
        let mut all_line_ids: Vec<u32> = Vec::new();
        let mut all_warnings = Vec::new();
        // Collect commands for the undo entry description
        let mut batch_commands: Vec<Command> = Vec::new();
        // Name of the first line placed via `ImportPlacement::ZeroDatum`
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
        // full O(N) `cascade_connection_offsets`, turning a 1 200-line
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
        // Eagerly populate the line-id lookup so the per-call `get_line`
        // stays O(1) instead of degrading into an O(N) linear scan inside
        // `merge_stations`. (The merge adjacency needs no priming: it builds
        // itself on first use and stays warm across the import's pushes.)
        if let Ok(mut proj_guard) = self.project.lock() {
            if let Some(project) = proj_guard.as_mut() {
                project.rebuild_line_index();
                // Make `resolve_key`'s AfterShot lookup O(1) for the hot
                // merge/closure connectivity loops below - without this, a large
                // import (e.g. a big Walls project) pins a core for minutes.
                project.rebuild_shot_uuid_index();
            }
        }

        // [perf-debug] Time the per-line apply loop. Used to confirm
        // whether the dominant cost in execute_import_lines scales
        // linearly with line count (good) or super-linearly (bad -
        // a per-line graph traversal that re-walks already-imported
        // lines would show up here as quadratic growth).
        let loop_start = std::time::Instant::now();
        let mut create_total = std::time::Duration::ZERO;
        let mut placement_total = std::time::Duration::ZERO;
        let report_every = 100usize;
        let mut last_report_idx = 0usize;
        let mut last_report_elapsed = std::time::Duration::ZERO;

        for (idx, config) in lines.iter().enumerate() {
            let t_line_start = std::time::Instant::now();
            // Build LineData from ImportLineConfig.
            //
            // The reverse (#24) is applied HERE, before the line exists, so
            // placement and station-comment indices below resolve against the
            // final order - the contract every importer wizard builds against.
            // Left/right is never swapped on this path (no dialog to ask).
            let mut shots = config.shots.clone();
            // A reverse makes station 0 the far end, so the line's anchor has
            // to gain the span it used to cover or the passage lands a full
            // end-to-end displacement away from where the wizard put it.  The
            // in-app reverse does this via `adjust_line_offset`; do the same
            // here, measured on the pre-reverse order, and apply it once the
            // line exists.  Merge- and datum-based placements below re-anchor
            // the line outright, so for those it is inert - it is `Origin` and
            // `Manual` (which position station 0 directly) that would otherwise
            // silently move.
            // The line does not exist yet, so its north reference is the one
            // the importer declared on the config, falling back to the project
            // default - the same value written onto the line below (#438).
            // The compensation has to be measured in that frame, because the
            // coordinate walk it cancels is.
            // `sanitised` because the config is DESERIALIZED: serde builds the
            // struct field by field and honours no constructor, so a payload can
            // assert `north: true` with a correction of 5 or a NaN correction,
            // and every geometry path below would apply it. Same rule the
            // `.stalx` read path imposes on the stored columns (#438).
            let declared_reference = config.azimuth_reference.map(AzimuthReference::sanitised);
            let azimuth_reference = declared_reference.unwrap_or_else(|| {
                self.get_project_state()
                    .and_then(|p| p.default_azimuth_reference)
                    .unwrap_or_default()
            });
            let reverse_offset = config.reverse.then(|| {
                let displacement = crate::models::shot_run_displacement(&shots, azimuth_reference);
                crate::models::reverse_shots(&mut shots, false);
                displacement
            });
            // The dialog's Surveyors / Explorers / date, when set, replace what
            // the file said on EVERY imported shot (the legacy `surveyor`/`date`
            // stay a template for the uncredited ones, via `Line::new`).
            let overrides = crate::models::CreditEdit {
                set_surveyors: config.surveyors.clone(),
                set_explorers: config.explorers.clone(),
                set_date: config.override_date.map(Some),
                ..Default::default()
            };
            if overrides.kind() != crate::models::CreditEditKind::Nothing {
                // Shots that share one credit (a Compass or Walls team line)
                // share its result too: one allocation, not one per shot.
                let mut applied: HashMap<
                    *const crate::models::SurveyCredit,
                    Option<std::sync::Arc<crate::models::SurveyCredit>>,
                > = HashMap::new();
                for shot in &mut shots {
                    let key = shot
                        .credit
                        .as_ref()
                        .map_or(std::ptr::null(), std::sync::Arc::as_ptr);
                    let next = applied
                        .entry(key)
                        .or_insert_with(|| {
                            let credit = overrides.apply(shot.credit());
                            (!credit.is_empty()).then(|| std::sync::Arc::new(credit))
                        })
                        .clone();
                    shot.credit = next;
                }
            }
            let line_data = LineData {
                name: config.name.clone(),
                surveyor: config.surveyor.clone(),
                date: config.date,
                color: config.color.clone(),
                direction: config.direction,
                quality: config.quality,
                shots,
            };

            // Step 1: Create the line
            let t_create_start = std::time::Instant::now();
            let create_result = self.create_line(line_data.clone());
            create_total += t_create_start.elapsed();
            if !create_result.success {
                // Rollback to before-snapshot
                self.rollback_project(&before_snapshot);
                return CommandResult::failure(format!(
                    "Failed to create line \"{}\": {}",
                    config.name.as_deref().unwrap_or("unnamed"),
                    create_result.error.unwrap_or_default()
                ));
            }

            let Some(line_id) = create_result.line_id else {
                self.rollback_project(&before_snapshot);
                return CommandResult::failure("Line created but no ID returned".to_string());
            };

            // The shots were reversed before the line existed, so record that on the line
            // itself (#24). `reverse_parity` is documented as flipping whenever a line's
            // shot order is reversed, and the plan-anchor resolution reads it to decide which
            // side of a shot a terminal station sits on - leaving it false here would make two
            // geometrically identical projects ("import then reverse" vs "import with the
            // toggle") disagree about that, and about which end the least-squares gauge pins.
            if config.reverse || declared_reference.is_some() {
                if let Ok(mut guard) = self.project.lock() {
                    if let Some(line) = guard.as_mut().and_then(|p| p.get_line_mut(line_id)) {
                        if config.reverse {
                            line.reverse_parity = !line.reverse_parity;
                        }
                        // #438: what the file said its azimuths are measured
                        // against. Written here because `LineData` cannot
                        // carry it through `create_line`, and the SANITISED
                        // value, so what the line holds is what the reverse
                        // compensation above was measured in.
                        if declared_reference.is_some() {
                            line.azimuth_reference = declared_reference;
                        }
                    }
                }
            }

            created_ids.insert(config.line_index, line_id);
            all_line_ids.push(line_id);
            all_warnings.extend(create_result.warnings);

            // Keep a reversed line where the un-reversed import would have put
            // it (see `reverse_offset` above).  Undo/redo for the whole import is
            // the before/after snapshot pair pushed at the end, so this needs no
            // command of its own.
            if let Some((dx, dy)) = reverse_offset {
                // Finiteness first - `NaN != 0.0` is true, and `line_offsets`
                // has no other finite guard (see `Project::reverse_line`).
                if dx.is_finite() && dy.is_finite() && (dx != 0.0 || dy != 0.0) {
                    let mut proj_guard = match self.project.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    if let Some(project) = proj_guard.as_mut() {
                        project.adjust_line_offset(line_id, dx, dy);
                    }
                }
            }

            // Apply station comments (promoted from shot comments during TML
            // import) and station names (the MNemo numbers of a DMP import).
            if !config.station_comments.is_empty() || !config.station_names.is_empty() {
                let mut proj_guard = match self.project.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if let Some(project) = proj_guard.as_mut() {
                    let shot_count = config.shots.len() as u32;
                    let entries = config
                        .station_comments
                        .iter()
                        .map(|entry| (StationText::Comment, entry))
                        .chain(
                            config
                                .station_names
                                .iter()
                                .map(|entry| (StationText::Name, entry)),
                        );
                    for (kind, (station_index, text)) in entries {
                        // Mirror here rather than trusting the caller to have done
                        // it. `reverse` renumbers station `i` to `shot_count - i`,
                        // and these indices were collected against the file's
                        // original order - the same rewrite the wizard applies to
                        // `placement` (src/utils/reversePlacement.ts). Doing it
                        // backend-side keeps every importer correct whether or not
                        // its wizard offers the toggle - TML sends comments, the
                        // DMP wizard (which has the toggle) sends names - instead
                        // of relying on a prose contract each has to re-derive.
                        // `checked_sub`, not `saturating_sub`: an index past the
                        // last station has no mirror, and saturating it would
                        // park the comment or name on station 0 - a station it
                        // was never about. Un-reversed the same index simply
                        // fails `key_for_ref` below and the entry is dropped, so
                        // skipping here keeps the two paths honest.
                        let index = if config.reverse {
                            match shot_count.checked_sub(*station_index) {
                                Some(mirrored) => mirrored,
                                None => continue,
                            }
                        } else {
                            *station_index
                        };
                        let station_ref = StationRef::new(line_id, index);
                        if let Some(key) = project.key_for_ref(station_ref) {
                            let map = match kind {
                                StationText::Comment => &mut project.station_comments,
                                StationText::Name => &mut project.station_names,
                            };
                            map.insert(key, text.clone());
                        }
                    }
                    // A line the file names in part (or rows the user added
                    // in the wizard) gets the rest by the creation rules; a
                    // line with no name at all is seeded from the station
                    // indexes by the station-identity epilogue.
                    if !config.station_names.is_empty() {
                        project.fill_station_names(line_id);
                    }
                }
            }

            // Record the shots as they were actually created (post-reverse), not
            // the raw config - otherwise a redo would recreate the line in the
            // file's original order.
            batch_commands.push(Command::CreateLine {
                name: config.name.clone(),
                surveyor: config.surveyor.clone(),
                date: config.date,
                color: config.color.clone(),
                direction: config.direction,
                shots: line_data.shots.clone(),
                cave_section_id: None,
            });

            // Step 2: Apply placement
            let t_placement_start = std::time::Instant::now();
            match &config.placement {
                ImportPlacement::Origin => {
                    // No-op - line starts at origin
                }

                ImportPlacement::ZeroDatum {
                    coordinates,
                    station_index,
                } => {
                    let station_ref = StationRef::new(line_id, *station_index);
                    let result = self.set_zero_datum(station_ref, *coordinates, None, None);
                    if !result.success {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Failed to set zero datum for line \"{}\": {}",
                            config.name.as_deref().unwrap_or("unnamed"),
                            result.error.unwrap_or_default()
                        ));
                    }
                    all_warnings.extend(result.warnings);
                    zero_datum_count += 1;
                    if zero_datum_created.is_none() {
                        // Sanitized HERE, where the raw file-controlled name is
                        // born, not only in `decorate_receipt`: the DMP
                        // wizard's `import_lines` IPC command never calls
                        // `decorate_receipt` (it overwrites source_format/
                        // file_name/lines_found directly), so that route's
                        // receipt would otherwise carry a raw
                        // `ImportLineConfig::name` straight from the parsed
                        // file end to end. `sanitize_display_text` is
                        // idempotent (filter then clamp), so decorate_receipt
                        // sanitizing this value again for the routes that DO
                        // call it is a harmless no-op, not a second transform.
                        zero_datum_created = Some(crate::models::sanitize_display_text(
                            &config
                                .name
                                .clone()
                                .unwrap_or_else(|| format!("Line {}", idx + 1)),
                            crate::core::command::MAX_FILE_NAME_LEN,
                        ));
                    }
                    batch_commands.push(Command::SetZeroDatum {
                        station_ref,
                        coordinates: *coordinates,
                        accuracy_m: None,
                        entrance: None,
                    });
                }

                ImportPlacement::FromExisting {
                    import_station,
                    existing_station,
                } => {
                    let import_ref = StationRef::new(line_id, *import_station);
                    let result = self.merge_stations(import_ref, *existing_station);
                    if !result.success {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Failed to merge line \"{}\": {}",
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
                    target_line_index,
                    target_station_index,
                } => {
                    let Some(&target_line_id) = created_ids.get(target_line_index) else {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Line \"{}\" references import line index {} which has not been created yet",
                            config.name.as_deref().unwrap_or("unnamed"),
                            target_line_index
                        ));
                    };
                    let import_ref = StationRef::new(line_id, *import_station);
                    let target_ref = StationRef::new(target_line_id, *target_station_index);
                    let result = self.merge_stations(import_ref, target_ref);
                    if !result.success {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Failed to merge line \"{}\" with imported line: {}",
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
                        let result = self.move_line(line_id, *offset);
                        if !result.success {
                            self.rollback_project(&before_snapshot);
                            return CommandResult::failure(format!(
                                "Failed to move line \"{}\": {}",
                                config.name.as_deref().unwrap_or("unnamed"),
                                result.error.unwrap_or_default()
                            ));
                        }
                        all_warnings.extend(result.warnings);
                        batch_commands.push(Command::MoveLine {
                            line_id,
                            offset: *offset,
                        });
                    }
                }

                ImportPlacement::ManualSnap {
                    align_station_index,
                    snapped_to_station,
                } => {
                    let snap_index = (*align_station_index).min(config.shots.len() as u32);
                    let import_ref = StationRef::new(line_id, snap_index);
                    let result = self.merge_stations(import_ref, *snapped_to_station);
                    if !result.success {
                        self.rollback_project(&before_snapshot);
                        return CommandResult::failure(format!(
                            "Failed to snap line \"{}\": {}",
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
            let _ = t_line_start;

            // [perf-debug] Periodic log: emit every `report_every` lines.
            // The "recent" delta highlights super-linear growth - if each
            // 100-line batch is taking longer than the last, a
            // per-line graph traversal is to blame.
            if (idx + 1) % report_every == 0 || idx + 1 == lines.len() {
                let now_elapsed = loop_start.elapsed();
                let batch = now_elapsed.saturating_sub(last_report_elapsed);
                let batch_n = (idx + 1) - last_report_idx;
                // Per-batch import progress is debug-only - it's
                // perf-investigation noise, not something a release
                // log should record.
                tracing::debug!(
                    lines_done = idx + 1,
                    total = lines.len(),
                    batch_n,
                    batch_ms = batch.as_millis() as u64,
                    total_elapsed_ms = now_elapsed.as_millis() as u64,
                    create_ms = create_total.as_millis() as u64,
                    placement_ms = placement_total.as_millis() as u64,
                    purpose = "TML import loop timing",
                    "line import progress"
                );
                last_report_idx = idx + 1;
                last_report_elapsed = now_elapsed;
            }
        }

        tracing::debug!(
            lines = lines.len(),
            total_ms = loop_start.elapsed().as_millis() as u64,
            create_ms = create_total.as_millis() as u64,
            placement_ms = placement_total.as_millis() as u64,
            purpose = "TML import loop summary",
            "execute_import_lines per-line loop done"
        );

        // Lift the deferral (clears the flag now so `cascade_connection_offsets`
        // is allowed to run) and execute ONE cascade across every newly
        // created line. This propagates any drift that would have
        // been caught by the per-call cascades - for a tree-shaped
        // import (no closures yet) this is a near-no-op single pass
        // that early-exits on the first iteration.  `disarm` consumes
        // the guard so its Drop becomes a no-op for the success path.
        // An Ariane file's `<UUID>`s are carried verbatim (#423), so importing a
        // file twice - or a file whose lines repeat one another - brings ids
        // the project already holds. Rename the newcomers' repeats now, once for
        // the whole import (per line would rescan the project every time):
        // before the integrity epilogue below, which treats an id held by two
        // lines as corruption, and before the after-snapshot the undo entry
        // records.
        self.reassign_repeated_shot_uuids(&all_line_ids);
        // Auto-grouping (#327): a config naming a Section joins the project's
        // Section of that name, created when there is none. Before the
        // after-snapshot, so one undo takes the Sections with the lines.
        let section_assignments: Vec<(u32, String)> = lines
            .iter()
            .filter_map(|config| {
                let name = config.cave_section.clone()?;
                Some((*created_ids.get(&config.line_index)?, name))
            })
            .collect();
        if !section_assignments.is_empty() {
            if let Ok(mut guard) = self.project.lock() {
                if let Some(project) = guard.as_mut() {
                    project.assign_sections_by_name(&section_assignments);
                }
            }
        }
        cascade_guard.disarm();
        let cascade_start = std::time::Instant::now();
        if let Ok(mut proj_guard) = self.project.lock() {
            if let Some(project) = proj_guard.as_mut() {
                project.cascade_connection_offsets(&all_line_ids);
            }
        }
        tracing::debug!(
            lines = all_line_ids.len(),
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
            let Some(&from_line_id) = created_ids.get(&closure.from_line_index) else {
                tracing::warn!(
                    from_line_index = closure.from_line_index,
                    purpose = "import loop closures",
                    "Closure source line not found in created lines, skipping"
                );
                closures_skipped += 1;
                continue;
            };
            let Some(&to_line_id) = created_ids.get(&closure.to_line_index) else {
                tracing::warn!(
                    to_line_index = closure.to_line_index,
                    purpose = "import loop closures",
                    "Closure target line not found in created lines, skipping"
                );
                closures_skipped += 1;
                continue;
            };
            closure_pairs.push((
                StationRef::new(from_line_id, closure.from_station_index),
                StationRef::new(to_line_id, closure.to_station_index),
            ));
        }
        if !closure_pairs.is_empty() {
            let results = {
                let mut proj_guard = match self.project.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
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

        // Track line content hashes (before after-snapshot so undo reverts them)
        if !line_hashes.is_empty() {
            let track_result = self.track_line_hashes(line_hashes);
            if !track_result.success {
                tracing::warn!(
                    purpose = "prevent duplicate line imports",
                    error = ?track_result.error,
                    "Failed to track imported line hashes"
                );
            }
        }

        // Split this file's virtual shots into Jump vs Reference (#27). Runs
        // here, inside the command, because the chain rule needs the graph the
        // loop above just built (lines placed, merges and closures created)
        // AND because everything it writes has to be inside the `after` snapshot
        // captured below, or redo would resurrect the unclassified state.
        //
        // Scoped to the lines this import created: the read stays
        // project-wide (a new run is usually only bounded *through* a merge into
        // existing survey), but a per-file setting must never rewrite a kind the
        // user hand-set in an earlier session.
        {
            let imported: HashSet<LineId> = all_line_ids.iter().copied().collect();
            let scope = crate::models::ClassifyScope::Lines(&imported);
            let mut proj_guard = match self.project.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
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

        // Block corruption at the source: reconcile derived
        // offsets, and reject the whole import if it produced critical
        // corruption (dangling refs / non-finite geometry) rather than committing
        // and later persisting it. This custom path bypasses execute()'s guard, so
        // it must run the same epilogue itself (#integrity Root 4 / catalog #19).
        // An import only ADDS lines/merges/closures (its closures are loops over
        // the spanning-tree merges, never sole bridges), so it can't strand a
        // line behind a closure - which is why the epilogue runs no
        // sole-bridge conversion at all.
        if self.finalize_structural_edit(pre_had_critical) {
            self.rollback_project(&before_snapshot);
            tracing::error!(
                purpose = "block corruption at the source",
                "TML import would corrupt survey structure; rolled back, nothing imported"
            );
            return CommandResult::failure(
                "Blocked: the imported data would corrupt the survey structure \
                 (dangling references or invalid geometry). Nothing was imported."
                    .to_string(),
            );
        }

        // Merge an Ariane import's project-level entrance elevation (#431)
        // before the after-snapshot, so it undoes/redoes atomically with the
        // rest of the import - the same reason `mode`'s virtual-shot pass
        // runs here rather than after (see the doc comment above). No-clobber:
        // an existing different value wins, and the mismatch becomes a receipt
        // warning naming both numbers.
        let surface_elevation_warning = surface_elevation.and_then(|incoming| {
            let mut proj_guard = match self.project.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
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
        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Push a single undo entry covering all lines + hash, named by the
        // lines it made.
        let mut imported: Vec<(u32, LineId)> =
            created_ids.iter().map(|(&i, &id)| (i, id)).collect();
        imported.sort_unstable();
        let imported: Vec<LineId> = imported.into_iter().map(|(_, id)| id).collect();
        let label = self.batch_label(|project| {
            crate::core::undo::history_batch::import_lines(project, &imported)
        });
        self.push_snapshot_undo(
            Command::Batch {
                commands: batch_commands,
                label,
            },
            before_snapshot,
            after_snapshot,
        );
        // Dirty-bit flip (#191, D-B16): a lines import mutated a remote
        // project. Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        let shot_count: u32 = lines.iter().map(|s| s.shots.len() as u32).sum();
        let closures_requested = closures.len() as u32;
        let mut receipt_warnings = Vec::new();
        if let Some(warning) = surface_elevation_warning {
            receipt_warnings.push(warning);
        }
        if doubly_anchored_shot_count > 0 {
            receipt_warnings.push(format!(
                "{doubly_anchored_shot_count} shot{} kept {} position{}: line anchored on both sides",
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
        // "and N more": naming only the FIRST zero-datum line without saying
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
            lines_found: 0,
            lines_imported: all_line_ids.len() as u32,
            shot_count,
            closures_requested,
            closures_created,
            unit_conversion: None,
            warnings: receipt_warnings,
            info: Vec::new(),
            zero_datum_created,
        };
        receipt.cap_warnings();

        let result = CommandResult::success()
            .with_line_ids(all_line_ids)
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
