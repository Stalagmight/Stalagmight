use super::AppState;
#[cfg(test)]
use crate::core::CommentHolder;
use crate::core::{Command, CommandResult, IntegrityCheckResult, IntegrityRepairResult, UndoData};
use crate::models::{Project, SectionData, ViolationSeverity};
#[cfg(test)]
use crate::models::{SectionId, ShotData, ShotId, ShotUpdates};

/// The integrity-relevant capabilities of a [`Command`], declared in ONE place
/// ([`AppState::command_integrity`]). The `execute()` epilogue reads these flags
/// instead of three separate hand-maintained allow/deny lists, so a new command
/// variant can't silently skip an integrity step.
#[derive(Debug, Clone, Copy)]
struct CommandIntegrity {
    /// Can drift section offsets (move merged partners out of alignment) and so
    /// needs the post-command geometry reconcile. Pure metadata/lifecycle edits
    /// cannot, so they skip it.
    affects_geometry: bool,
    /// Can introduce a dangling/out-of-range cross-reference — by removing a
    /// referenced entity or renumbering station indices — and so needs the
    /// (expensive) whole-project rollback snapshot to reject corruption at the
    /// source. Geometry-only commands add valid refs or shift offsets but cannot
    /// dangle a reference, so they skip the serialize.
    can_dangle_refs: bool,
    /// Can REMOVE connectivity and strand a section so its only attachment is a
    /// loop closure — which must then be converted to a merge (issue #8). Commands
    /// that add or keep connectivity are excluded (a move must leave an inert
    /// closure independent).
    can_orphan_closures: bool,
}

impl AppState {
    /// Declare every command's integrity-relevant capabilities in one exhaustive
    /// match, so adding a [`Command`] variant FORCES a decision here (the compiler
    /// rejects a non-exhaustive match) rather than silently defaulting across three
    /// separate allow/deny lists — where a miss fails open: a skipped rollback
    /// snapshot commits corruption, a missing reconcile leaves merges drifted, and
    /// a missing orphan-closure conversion leaves a phantom loop closure.
    /// Whether a command is a lifecycle pass-through that the read-only edit gate
    /// (#191, D-B6/D-B16) must NOT block: Save/Open/Close/Create/MergeProjects.
    /// Autosave (a `SaveProject`) must keep working on a read-only remote project,
    /// and opening/closing/creating/merging are not "edits of the open project".
    /// An exhaustive `match` (like `command_integrity`) forces a deliberate choice
    /// for every new `Command` variant — anti-fail-open: a new mutating variant
    /// defaults to gated.
    fn is_lifecycle_passthrough(cmd: &Command) -> bool {
        use Command::*;
        match cmd {
            CreateProject { .. }
            | OpenProject { .. }
            | SaveProject { .. }
            | CloseProject
            | MergeProjects { .. } => true,
            CreateSection { .. }
            | DeleteSection { .. }
            | UpdateSectionMetadata { .. }
            | CreateShot { .. }
            | UpdateShot { .. }
            | DeleteShot { .. }
            | SetShotReference { .. }
            | MergeStations { .. }
            | UnmergeStations { .. }
            | UnmergeStationPair { .. }
            | CreateLoopClosure { .. }
            | RemoveLoopClosure { .. }
            | SetZeroDatum { .. }
            | RemoveZeroDatum { .. }
            | SetStationComment { .. }
            | MoveSection { .. }
            | MoveSegment { .. }
            | MoveStation { .. }
            | MoveComment { .. }
            | Batch { .. }
            | ApplyCollabChanges
            | RepairIntegrity
            | UpdateProjectSettings
            | AddMarker { .. }
            | UpdateMarker { .. }
            | DeleteMarker { .. } => false,
        }
    }

    fn command_integrity(cmd: &Command) -> CommandIntegrity {
        use Command::*;
        // Named profiles keep the table readable; every variant maps to exactly one.
        const INERT: CommandIntegrity = CommandIntegrity {
            affects_geometry: false,
            can_dangle_refs: false,
            can_orphan_closures: false,
        };
        const GEOMETRY_ONLY: CommandIntegrity = CommandIntegrity {
            affects_geometry: true,
            can_dangle_refs: false,
            can_orphan_closures: false,
        };
        const REMOVES: CommandIntegrity = CommandIntegrity {
            affects_geometry: true,
            can_dangle_refs: true,
            can_orphan_closures: true,
        };
        const CREATE_SHOT: CommandIntegrity = CommandIntegrity {
            affects_geometry: true,
            can_dangle_refs: true,
            can_orphan_closures: false,
        };
        const UNMERGE: CommandIntegrity = CommandIntegrity {
            affects_geometry: true,
            can_dangle_refs: false,
            can_orphan_closures: true,
        };
        match cmd {
            // Lifecycle + pure metadata edits: touch no geometry and no refs.
            // RepairIntegrity never runs through execute() (it records its own
            // snapshot-pair undo and returns failure here), so its flags are never
            // read; grouped here as it disturbs nothing.
            CreateProject { .. }
            | OpenProject { .. }
            | SaveProject { .. }
            | CloseProject
            | UpdateSectionMetadata { .. }
            | SetShotReference { .. }
            // Station comments are metadata keyed by station identity; they are
            // never geometry-derived and can never introduce merge drift, so they
            // must NOT run the reconcile epilogue. Classifying them GEOMETRY_ONLY
            // would heal an already-drifted project as a side effect of a comment
            // edit, while their undo (restore the previous comment) does not invert
            // that geometry change — an asymmetric undo.
            | SetStationComment { .. }
            | MoveComment { .. }
            | RepairIntegrity
            | UpdateProjectSettings => INERT,

            // Remove an entity / renumber stations: can dangle refs AND strand a
            // section behind a sole-bridge closure.
            DeleteSection { .. } | DeleteShot { .. } | Batch { .. } | MergeProjects { .. } => REMOVES,

            // Inserting a shot renumbers later stations (can dangle a positional
            // ref) but cannot strand a section.
            CreateShot { .. } => CREATE_SHOT,

            // Unmerge removes connectivity (can strand a section) but adds no
            // dangling reference.
            UnmergeStations { .. } | UnmergeStationPair { .. } => UNMERGE,

            // Markers are pure metadata: no geometry, no cross-references, no connectivity.
            AddMarker { .. } | UpdateMarker { .. } | DeleteMarker { .. } => INERT,

            // Everything else shifts geometry but keeps refs valid and connectivity
            // intact, so it needs only the reconcile. UpdateShot belongs here too: a
            // length/azimuth/depth edit moves stations, and its bespoke
            // realign_after_shot_edit no-ops on a merge group with no zero datum, so
            // without the reconcile epilogue a shot edit upstream of a datum-less
            // merge would leave the partners drifted until a later load/repair.
            CreateSection { .. }
            | UpdateShot { .. }
            | MergeStations { .. }
            | CreateLoopClosure { .. }
            | RemoveLoopClosure { .. }
            | SetZeroDatum { .. }
            | RemoveZeroDatum { .. }
            | MoveSection { .. }
            | MoveSegment { .. }
            | MoveStation { .. }
            | ApplyCollabChanges => GEOMETRY_ONLY,
        }
    }

    /// Whether a *specific* command instance warrants the geometry-reconcile
    /// epilogue. Refines the per-variant [`CommandIntegrity::affects_geometry`]
    /// flag with a per-instance check for [`Command::UpdateShot`]: the same
    /// command carries both geometry edits (length/azimuth/depth/pitch — must
    /// reconcile) and pure metadata edits (comment/LRUD/reference — must NOT).
    /// Running the reconcile on a metadata-only edit would heal an already-drifted
    /// project as a side effect, while its undo only restores the metadata — the
    /// same asymmetric undo that keeps `SetStationComment` INERT (see
    /// `command_integrity`). The variant flag stays `true` (it CAN move stations);
    /// only the epilogue decision is instance-aware.
    ///
    /// A [`Command::Batch`] inherits the union of its members, so a metadata-only
    /// batch (a multi-cell comment/LRUD paste) skips the reconcile at BOTH the
    /// inner `execute_batch` epilogue and the outer `execute` epilogue that re-runs
    /// it — otherwise the outer pass would heal the drift the inner pass left.
    fn cmd_affects_geometry(cmd: &Command) -> bool {
        match cmd {
            Command::UpdateShot { updates, .. } => updates.touches_geometry(),
            Command::Batch { commands } => commands.iter().any(Self::cmd_affects_geometry),
            other => Self::command_integrity(other).affects_geometry,
        }
    }

    /// Convert any sole-bridge loop closures in the open project to merges.
    fn convert_orphan_closures_in_open_project(&self) {
        let mut guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(project) = guard.as_mut() else {
            return;
        };
        let converted = project.convert_orphan_closures_to_merges();
        if converted > 0 {
            tracing::info!(
                converted,
                purpose = "enforce closures-are-loops invariant",
                "Converted sole-bridge loop closures to merges after a removal"
            );
        }
    }

    /// True iff the open project currently holds a Critical integrity violation.
    pub(super) fn project_has_critical(&self) -> bool {
        self.project
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(Project::has_critical_integrity_violations))
            .unwrap_or(false)
    }

    /// Integrity gate shared by [`Self::execute`] and [`Self::execute_batch`]: if
    /// the project was clean before (`!pre_had_critical`) but the command made it
    /// Critical, roll back to `snapshot` and return the standard "blocked"
    /// failure; otherwise return `None` and let the caller proceed. Corruption
    /// must be rejected BEFORE any undo entry is pushed, or a later undo+redo
    /// would reinstate the corrupt state the guard rejected (#1).
    fn guard_introduced_corruption(
        &self,
        pre_had_critical: bool,
        snapshot: &str,
        cmd_desc: &str,
    ) -> Option<CommandResult> {
        if pre_had_critical || !self.project_has_critical() {
            return None;
        }
        self.rollback_project(snapshot);
        tracing::error!(
            command = cmd_desc,
            purpose = "block corruption at the source",
            "Rejected command that would corrupt survey structure; rolled back"
        );
        Some(CommandResult::failure(
            "Blocked: this action would corrupt the survey structure \
             (it leaves dangling references). No changes were made."
                .to_string(),
        ))
    }

    /// Shared structural-edit epilogue for the custom `execute_*` paths that
    /// bypass [`Self::execute`]'s gated epilogue (collab apply, TML import).
    /// Converts sole-bridge closures to merges and reconciles derived offsets,
    /// then reports whether this edit INTRODUCED a CRITICAL integrity violation
    /// (dangling ref / out-of-range index / non-finite geometry). A caller that
    /// captured a pre-edit snapshot rolls back to it when this returns `true`,
    /// giving these paths the same "block corruption at the source" guarantee the
    /// standard command path gets (#integrity Root 4).
    ///
    /// `pre_had_critical` is the project's critical state BEFORE the edit. Matching
    /// `guard_introduced_corruption` / `execute`, an already-critical project (one
    /// opened with a dangling ref or non-finite shot — permitted; the banner offers
    /// Repair) does not block an otherwise-clean collab apply / import: only NEWLY
    /// introduced corruption rolls back.
    /// `can_orphan_closures` mirrors [`CommandIntegrity::can_orphan_closures`]:
    /// only an operation that can SEVER connectivity can strand a section behind a
    /// sole-bridge closure (issue #8). Pure-addition paths (a fresh import) never
    /// create one, so they pass `false` and skip the O(closures × connectivity)
    /// sole-bridge scan — which otherwise dominated a large import.
    pub(super) fn finalize_structural_edit(
        &self,
        pre_had_critical: bool,
        can_orphan_closures: bool,
    ) -> bool {
        if can_orphan_closures {
            self.convert_orphan_closures_in_open_project();
        }
        self.reconcile_geometry();
        if pre_had_critical {
            return false;
        }
        self.project
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(Project::has_critical_integrity_violations))
            .unwrap_or(false)
    }

    /// Structural-integrity violation messages for the open project, critical
    /// first. Empty when the project is sound or none is open. Used by the
    /// load-time "warn and offer repair" surface.
    pub fn integrity_violation_messages(&self) -> Vec<String> {
        self.integrity_check().messages
    }

    /// Full integrity check for the open project: the violation messages
    /// (critical first; empty = sound) plus whether the project is currently
    /// save-blocked (has unresolvable cross-references — the exact condition that
    /// refuses both manual and auto-save). The frontend uses `save_blocked` to keep
    /// the integrity banner non-dismissable while saving is blocked, so the Repair
    /// entry point can't be hidden on a project that can't be saved.
    pub fn integrity_check(&self) -> IntegrityCheckResult {
        let Some(project) = self.get_project_state() else {
            return IntegrityCheckResult::default();
        };
        // Structural (dangling refs) + geometric (drifted merges + stale,
        // still-marked-closed loop closures) + topological (a closure that is
        // really a sole-bridge merge).
        let mut violations = project.check_structural_integrity();
        violations.extend(project.check_merge_alignment());
        violations.extend(project.check_closure_alignment());
        violations.extend(project.check_orphan_closures());
        violations.sort_by_key(|v| match v.severity {
            ViolationSeverity::Critical => 0,
            ViolationSeverity::Warning => 1,
        });
        IntegrityCheckResult {
            save_blocked: project.has_unresolvable_cross_references(),
            messages: violations.into_iter().map(|v| v.message).collect(),
        }
    }

    /// Repair the open project's integrity: drop dangling refs/orphans, re-mint
    /// duplicate shot UUIDs, then re-cascade section offsets so drifted merges
    /// line up again. Returns `{ fixed, remaining }` measured as the change in
    /// the FULL violation set (the same set the banner counts), so the reported
    /// numbers match what the user sees. `remaining > 0` means some issues are
    /// detect-only and need manual attention (a drifted aligned loop closure to
    /// recompute/remove; non-finite shot geometry to correct).
    pub fn repair_open_project_integrity(&self) -> IntegrityRepairResult {
        // Serialize against execute()/undo()/redo() for the WHOLE operation. Repair
        // captures before_snapshot, mutates, and pushes a snapshot-pair undo across
        // several independent project locks; without command_lock a concurrent
        // command could commit a mutation in the window between before_snapshot and
        // repair's own mutation, so undoing the Repair would restore a stale `before`
        // and silently drop that command's changes. Deadlock-safe: none of the
        // helpers repair calls take command_lock (execute holds it while calling the
        // same capture/snapshot/undo helpers).
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return IntegrityRepairResult {
                fixed: 0,
                remaining: 0,
                changed: false,
            };
        };
        // Read-only edit gate (#191, D-B5): Repair mutates the project, so it is
        // gated on a read-only remote project. It returns IntegrityRepairResult
        // (not CommandResult), so it can't carry the EDIT_DISABLED code; instead
        // return `changed: false` and log — the FE treats changed:false as
        // "nothing happened" (it dirties only on changed). The coded-failure
        // surface for repair is #204's; #191 just refuses to mutate. (The
        // integrity banner stays dismissable for non-blocking corruption — see
        // OPEN-3/D-B5.)
        if self.guard_mutation("Repair integrity").is_err() {
            tracing::info!(
                purpose = "read-only remote project",
                "repair_open_project_integrity rejected: edit disabled (no-op)"
            );
            return IntegrityRepairResult {
                fixed: 0,
                remaining: 0,
                changed: false,
            };
        }
        // Count over the full violation set before repairing. Locks the project
        // internally, so it must run with no project guard held.
        let before = self.integrity_violation_messages().len();
        // Snapshot the project BEFORE mutating so the repair can be undone: it
        // drops dangling refs, re-mints duplicate UUIDs and re-cascades offsets,
        // all reversible by restoring this snapshot. (capture locks the project
        // internally, so take it with no guard held.)
        let before_snapshot = self.capture_project_snapshot().ok();
        {
            let mut guard = match self.project.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let Some(project) = guard.as_mut() else {
                return IntegrityRepairResult {
                    fixed: 0,
                    remaining: 0,
                    changed: false,
                };
            };
            // Drop dangling refs/orphans and re-mint duplicate shot UUIDs.
            project.repair_structural_integrity();
            // Dropping refs may have cleared the adjacency cache; rebuild before
            // the cascade walks the merge graph.
            project.rebuild_merge_adjacency();
            // Turn any sole-bridge closure into a merge (a closure is only a
            // loop), then re-derive offsets. Repair is an explicit "fix my
            // project" action, so use the zero-trust re-solve that never honors a
            // persisted offset.
            project.convert_orphan_closures_to_merges();
            project.resolve_offsets();
        } // release the guard before re-measuring (integrity_violation_messages re-locks)
          // Repair mutated measured geometry (and convert_orphan_closures_to_merges
          // doesn't bump modified_at), so drop any cached closure-adjusted solve.
        self.invalidate_adjusted_cache();
        let after = self.integrity_violation_messages().len();
        // Record an undo entry (before -> after) so the user can revert the repair.
        // Only when it actually changed the project, so a no-op repair doesn't add
        // a spurious history entry. Undo restores the pre-repair (corrupt) state —
        // which the save guard still refuses to persist — and redo re-applies the
        // fix; both go through the snapshot-pair restore, never re-executing.
        //
        // `changed` mirrors that same before != after condition: it is the honest
        // "the project was mutated" signal the frontend dirties on. It is broader
        // than `fixed > 0` because resolve_offsets() above heals sub-tolerance drift
        // that never registered as a counted violation (fixed == 0, yet changed).
        let mut changed = false;
        if let Some(before_json) = before_snapshot {
            if let Ok(after_json) = self.capture_project_snapshot() {
                if before_json != after_json {
                    changed = true;
                    self.push_snapshot_undo(Command::RepairIntegrity, before_json, after_json);
                    // Dirty-bit flip (#191, D-B16): repair mutated a remote project.
                    self.mark_remote_dirty();
                }
            }
        }
        IntegrityRepairResult {
            fixed: before.saturating_sub(after) as u32,
            remaining: after as u32,
            changed,
        }
    }

    /// Execute a command and return the result
    pub fn execute(&self, cmd: Command) -> CommandResult {
        // Serialize all command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): reject mutations on a remote project
        // opened read-only, BUT lifecycle commands (Save/Open/Close/Create/
        // MergeProjects) always pass through (autosave must not be blocked).
        // `execute_move_comment` and `apply_collab_changes` are reached via this
        // `execute()` (early `return self.execute_move_comment(..)` and the
        // `ApplyCollabChanges` arm), so they inherit this guard — see the
        // chokepoint enumeration test (`gate_rejects_every_mutation_entrypoint`).
        if !Self::is_lifecycle_passthrough(&cmd) {
            if let Err(f) = self.guard_mutation(&cmd.description()) {
                return f;
            }
            // Lazy 20-min lock re-verify (#202): on a lock-held remote edit through
            // this chokepoint, confirm (at most ~3x/hr) we still hold the write lock
            // before mutating. No-op for local / read-only projects and when recently
            // verified. Deliberately scoped to the `execute()` path (the direct
            // mutation entrypoints - undo/redo etc. - do not re-verify; the
            // release-time fence + a 403 on push are the correctness backstops).
            if let Err(f) = self.speleodb_verify_lock_if_expired() {
                return f;
            }
        }
        // Any mutation invalidates the derived closure-adjusted view.
        self.invalidate_adjusted_cache();
        tracing::info!(
            command = ?cmd.description(),
            purpose = "apply user action",
            "Executing command to update project state"
        );

        // Generate undo data before executing the command
        let undo_data = if Self::is_undoable(&cmd) {
            self.generate_undo_data(&cmd)
        } else {
            UndoData::None
        };

        // The command's integrity capabilities, computed once and read by the
        // epilogue below (snapshot / orphan-closure conversion / reconcile).
        let integrity = Self::command_integrity(&cmd);

        // Snapshot before a ref-removing/renumbering command so a result that
        // introduces dangling references can be rejected and rolled back —
        // corruption must not enter through the command path (#17). Only those
        // commands can dangle a ref, so geometry-only commands (moves, merges,
        // datums) skip the expensive whole-project serialize (#8).
        // Batch owns its own snapshot + integrity guard inside execute_batch
        // (which must reject corruption BEFORE pushing its undo entry, #1), so it
        // is excluded here to avoid a redundant whole-project serialize and a
        // double rollback.
        let integrity_snapshot =
            if integrity.can_dangle_refs && !matches!(cmd, Command::Batch { .. }) {
                self.capture_project_snapshot().ok()
            } else {
                None
            };
        // If the project is ALREADY corrupt (loaded that way), don't block — the
        // user repairs via the banner. We only reject commands that newly
        // introduce corruption into a clean project.
        let pre_had_critical = integrity_snapshot.is_some() && self.project_has_critical();

        let result = match &cmd {
            Command::CreateProject { name, location } => {
                self.create_project(name.clone(), location.clone())
            }
            Command::OpenProject { path } => self.open_project(path.clone()),
            Command::SaveProject { path, is_autosave } => {
                self.save_project(path.clone(), *is_autosave)
            }
            Command::CloseProject => self.close_project(),
            Command::CreateSection {
                name,
                surveyor,
                date,
                color,
                direction,
                shots,
            } => self.create_section(SectionData {
                name: name.clone(),
                surveyor: surveyor.clone(),
                date: *date,
                color: color.clone(),
                direction: *direction,
                quality: 1.0,
                shots: shots.clone(),
            }),
            Command::DeleteSection { section_id } => self.delete_section(*section_id),
            Command::UpdateSectionMetadata {
                section_id,
                updates,
            } => self.update_section(*section_id, updates.clone()),
            Command::CreateShot {
                section_id,
                shot,
                after_shot_id,
                before_shot_id,
            } => self.create_shot(*section_id, shot.clone(), *after_shot_id, *before_shot_id),
            Command::UpdateShot {
                section_id,
                shot_id,
                updates,
            } => self.update_shot(*section_id, *shot_id, updates.clone()),
            Command::DeleteShot {
                section_id,
                shot_id,
            } => self.delete_shot(*section_id, *shot_id),
            Command::SetShotReference {
                section_id,
                shot_id,
                is_reference,
            } => self.set_shot_reference(*section_id, *shot_id, *is_reference),
            Command::MergeStations {
                station_a,
                station_b,
            } => self.merge_stations(*station_a, *station_b),
            Command::UnmergeStations { station_ref } => self.unmerge_stations(*station_ref),
            Command::UnmergeStationPair {
                station_a,
                station_b,
            } => self.unmerge_station_pair(*station_a, *station_b),
            Command::CreateLoopClosure {
                station_a,
                station_b,
                closure_strategy,
            } => self.create_loop_closure(*station_a, *station_b, *closure_strategy),
            Command::RemoveLoopClosure { closure_id } => self.remove_loop_closure(*closure_id),
            Command::SetZeroDatum {
                station_ref,
                coordinates,
            } => self.set_zero_datum(*station_ref, *coordinates),
            Command::RemoveZeroDatum { station_ref } => self.remove_zero_datum(*station_ref),
            Command::SetStationComment {
                station_ref,
                comment,
            } => self.set_station_comment(*station_ref, comment.clone()),
            Command::MoveSection { section_id, offset } => self.move_section(*section_id, *offset),
            Command::MoveSegment { section_id, offset } => self.move_segment(*section_id, *offset),
            Command::MoveStation {
                station_ref,
                offset,
            } => self.move_station(*station_ref, *offset),
            Command::MoveComment {
                source,
                target,
                swap,
            } => return self.execute_move_comment(source.clone(), target.clone(), *swap, cmd),
            Command::Batch { commands } => self.execute_batch(commands.clone(), true),
            Command::MergeProjects {
                source_paths,
                output_path,
                output_name,
                archive_originals,
            } => self.merge_projects(
                source_paths.clone(),
                output_path.clone(),
                output_name.clone(),
                *archive_originals,
            ),
            Command::AddMarker {
                section_id,
                shot_id,
                position,
                marker_type,
                color,
                text,
                direction,
            } => self.add_marker(
                *section_id,
                *shot_id,
                *position,
                marker_type.clone(),
                color.clone(),
                text.clone(),
                direction.clone(),
            ),
            Command::UpdateMarker { marker_id, updates } => {
                self.update_marker(*marker_id, updates.clone())
            }
            Command::DeleteMarker { marker_id } => self.delete_marker(*marker_id),
            Command::ApplyCollabChanges => self.apply_collab_changes(),
            // Never routed here: repair is applied via `repair_project_integrity`,
            // which records its own snapshot-pair undo entry. Present only for match
            // exhaustiveness.
            Command::RepairIntegrity => CommandResult::failure(
                "RepairIntegrity is applied via repair_project_integrity, not execute()"
                    .to_string(),
            ),
            Command::UpdateProjectSettings => CommandResult::failure(
                "UpdateProjectSettings is applied via update_project_settings, not execute()"
                    .to_string(),
            ),
        };

        // Block corruption at the source: if a structural command left the
        // project with dangling references, roll back to the pre-command
        // snapshot and reject it rather than committing (and later persisting)
        // a corrupt state (#17).
        if result.success && !pre_had_critical {
            if let Some(snapshot) = &integrity_snapshot {
                if let Some(failure) =
                    self.guard_introduced_corruption(pre_had_critical, snapshot, &cmd.description())
                {
                    return failure;
                }
            }
        }

        // A command that REMOVES connectivity (delete shot/section, unmerge) can
        // strand a section so its only attachment is a loop closure — which is
        // then a merge, not a loop. Convert before reconciling offsets so the new
        // merge is solved immediately (#8). Scoped to removal commands so a move
        // still leaves an inert closure independent.
        if result.success && integrity.can_orphan_closures {
            self.convert_orphan_closures_in_open_project();
        }

        // Treat section offsets as derived: after any structural command,
        // reconcile geometry so no command path can leave merged stations
        // drifted (a no-op when nothing moved; skipped during bulk import). A
        // metadata-only UpdateShot (comment/LRUD/reference) is excluded — see
        // `cmd_affects_geometry` — so it can't silently heal drift with no undo.
        if result.success && Self::cmd_affects_geometry(&cmd) {
            self.reconcile_geometry();
        }

        // Push to undo stack if successful and undoable.
        // Batch commands handle their own undo push inside execute_batch()
        // (with before+after snapshots), so skip them here.
        if result.success && Self::is_undoable(&cmd) && !matches!(cmd, Command::Batch { .. }) {
            // For create commands, finalize undo data with snapshot-based ID preservation
            let final_undo_data = self.finalize_undo_data(&cmd, undo_data);

            if let Ok(mut undo_stack) = self.undo_stack.lock() {
                undo_stack.push_with_data(cmd.clone(), final_undo_data);
            }
            // Dirty-bit flip (#191, D-B16): a successful, undo-pushed edit of a
            // remote project marks it has_unpushed_edits. Set-only; no-op for
            // local-only (load_speleo_link → None). Lifecycle pass-throughs
            // (notably autosave/SaveProject) must NOT mark dirty. Batch pushes its
            // own undo inside execute_batch and flips there.
            if !Self::is_lifecycle_passthrough(&cmd) {
                self.mark_remote_dirty();
            }
        }

        result
    }

    // === Shot operations ===

    // === Station operations ===

    /// Check if a command is allowed in a batch (whitelist for IPC safety).
    ///
    /// `UpdateShot` is permitted so the data-table grid can commit a multi-cell
    /// edit (paste/fill) as a single undo unit: it keeps refs + connectivity
    /// intact, self-validates length/finiteness/frozen-geometry, and a geometry
    /// edit has the moved stations baked in by the batch epilogue's
    /// `reconcile_geometry()` — which the epilogue runs only when the batch
    /// actually touches geometry (see `cmd_affects_geometry`), so a metadata-only
    /// UpdateShot batch (comment/LRUD/reference) commits WITHOUT reconciling.
    /// `SetStationComment` is still NOT allowed: as a whole it is INERT metadata
    /// (see `command_integrity_profiles_are_stable`), so it would never carry the
    /// geometry that justifies a batch; comment edits stay one-command-per-undo.
    fn is_batch_allowed(cmd: &Command) -> bool {
        matches!(
            cmd,
            Command::CreateSection { .. }
                | Command::CreateLoopClosure { .. }
                | Command::MergeStations { .. }
                | Command::MoveSection { .. }
                | Command::MoveSegment { .. }
                | Command::MoveStation { .. }
                | Command::SetZeroDatum { .. }
                | Command::UpdateShot { .. }
        )
    }

    /// Execute a batch of commands atomically.
    /// When `push_undo` is true (called from `execute()`), pushes a ProjectSnapshotPair
    /// undo entry. When false (called from `execute_without_undo()`), no undo entry is pushed.
    /// Caller must already hold `command_lock`.
    pub(super) fn execute_batch(&self, commands: Vec<Command>, push_undo: bool) -> CommandResult {
        // No-op for empty batch: skip snapshot/undo overhead
        if commands.is_empty() {
            return CommandResult::success();
        }

        // Whitelist: reject commands not allowed in batch context
        for cmd in &commands {
            if !Self::is_batch_allowed(cmd) {
                return CommandResult::failure(format!(
                    "Command '{}' is not allowed in batch context",
                    cmd.description()
                ));
            }
        }

        // Whether the project was ALREADY corrupt before the batch (loaded that
        // way) — captured against the pre-batch state so the guard below only
        // rejects corruption the batch itself introduces.
        let pre_had_critical = self.project_has_critical();

        // Capture pre-batch snapshot for rollback on partial failure and undo
        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        let mut all_warnings = Vec::new();
        let mut section_ids = Vec::new();

        for cmd in &commands {
            // Use execute_without_undo to avoid pushing individual undo entries;
            // the outer Batch command handles undo as a single unit via ProjectSnapshotPair
            let result = self.execute_without_undo(cmd.clone());
            if !result.success {
                // Rollback: restore pre-batch state to avoid partial mutations
                self.rollback_project(&before_snapshot);
                return result;
            }
            all_warnings.extend(result.warnings);
            // Collect section IDs from CreateSection commands
            if let Some(id) = result.section_id {
                section_ids.push(id);
            }
            section_ids.extend(result.section_ids);
        }

        // Block corruption at the source — BEFORE pushing the undo entry. The
        // sub-commands ran via execute_without_undo (no per-command guard), so a
        // batch can strand a dangling reference; if we pushed the snapshot pair
        // first (as before) a later undo+redo would reinstate the corrupt state.
        // #1
        if let Some(failure) =
            self.guard_introduced_corruption(pre_had_critical, &before_snapshot, "Batch")
        {
            return failure;
        }

        // Finalize structural side-effects BEFORE snapshotting for redo, mirroring
        // the per-command epilogue in execute(). A batch that strands a sole-bridge
        // closure must have the closure→merge conversion baked into the after-
        // snapshot; otherwise undo→redo restores the pre-conversion topology — redo
        // only reconciles geometry, it does NOT re-run the conversion — and the
        // merge is silently lost. The conversion stays unconditional: a batched
        // CreateLoopClosure can itself create the sole-bridge closure that needs it.
        //
        // The reconcile, however, is gated exactly as execute() gates its own (see
        // cmd_affects_geometry): a batch that touches no geometry (a multi-cell
        // paste/fill of comments / LRUD / reference flags — all metadata-only
        // UpdateShots) must NOT reconcile, or it would heal an already-drifted
        // project as a side effect of a metadata edit whose undo only restores the
        // metadata (the asymmetric-undo hazard that keeps SetStationComment out of
        // batches entirely). The outer execute() epilogue applies the same gate to
        // the Batch command, so the two passes agree.
        self.convert_orphan_closures_in_open_project();
        if commands.iter().any(Self::cmd_affects_geometry) {
            self.reconcile_geometry();
        }

        if push_undo {
            // Capture after-snapshot for redo support; failure means rollback
            let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
                Ok(s) => s,
                Err(result) => return result,
            };

            // Push undo entry directly (bypasses finalize_undo_data which is a no-op for Batch)
            self.push_snapshot_undo(Command::Batch { commands }, before_snapshot, after_snapshot);
            // Dirty-bit flip (#191, D-B16): a batch is always a set of mutations
            // (never a lifecycle command), so mark the remote project dirty after
            // its undo entry is pushed. Set-only; no-op for local-only.
            self.mark_remote_dirty();
        }

        let result = if section_ids.is_empty() {
            CommandResult::success()
        } else {
            CommandResult::success().with_section_ids(section_ids)
        };
        if all_warnings.is_empty() {
            result
        } else {
            result.with_warnings(all_warnings)
        }
    }

    // === Station query operations ===

    // === Project merge operations ===

    // === Query operations ===
}

#[cfg(test)]
mod integrity_capability_tests {
    use super::*;
    use crate::models::{Coordinates, ShotData, StationRef};

    /// Locks the integrity-capability table (one command per profile). The
    /// exhaustive match in `command_integrity` makes a new variant a *compile*
    /// error; this guards against an existing variant being moved to the wrong
    /// profile — especially the REMOVES group, where dropping `can_dangle_refs`
    /// would skip the rollback snapshot and let corruption be committed.
    #[test]
    fn command_integrity_profiles_are_stable() {
        let caps = |cmd: &Command| {
            let i = AppState::command_integrity(cmd);
            (i.affects_geometry, i.can_dangle_refs, i.can_orphan_closures)
        };

        // INERT (no geometry, no refs).
        assert_eq!(caps(&Command::CloseProject), (false, false, false));
        // Markers are INERT: no geometry, no cross-refs, no connectivity.
        assert_eq!(
            caps(&Command::AddMarker {
                section_id: 1,
                shot_id: 0,
                position: 50.0,
                marker_type: crate::models::MarkerType::Ndm,
                color: "white".to_string(),
                text: String::new(),
                direction: None,
            }),
            (false, false, false)
        );
        assert_eq!(
            caps(&Command::UpdateMarker {
                marker_id: 1,
                updates: crate::models::MarkerUpdates::default(),
            }),
            (false, false, false)
        );
        assert_eq!(
            caps(&Command::DeleteMarker { marker_id: 1 }),
            (false, false, false)
        );
        assert_eq!(
            caps(&Command::SaveProject {
                path: None,
                is_autosave: false
            }),
            (false, false, false)
        );
        // RepairIntegrity never runs through execute(); it is classified INERT.
        assert_eq!(caps(&Command::RepairIntegrity), (false, false, false));
        // Comment edits are metadata-only — INERT, so a comment edit can't run the
        // reconcile epilogue (which would heal a drifted project with no undo).
        assert_eq!(
            caps(&Command::SetStationComment {
                station_ref: StationRef::new(1, 0),
                comment: None
            }),
            (false, false, false)
        );
        assert_eq!(
            caps(&Command::MoveComment {
                source: CommentHolder::Station {
                    station_ref: StationRef::new(1, 0)
                },
                target: CommentHolder::Station {
                    station_ref: StationRef::new(1, 1)
                },
                swap: false,
            }),
            (false, false, false)
        );

        // REMOVES (can dangle refs AND orphan a closure → needs rollback snapshot
        // + reconcile + orphan-closure conversion).
        assert_eq!(
            caps(&Command::DeleteSection { section_id: 1 }),
            (true, true, true)
        );
        assert_eq!(
            caps(&Command::Batch { commands: vec![] }),
            (true, true, true)
        );

        // CREATE_SHOT (renumbers stations → can dangle, but cannot orphan).
        assert_eq!(
            caps(&Command::CreateShot {
                section_id: 1,
                shot: ShotData::default(),
                after_shot_id: None,
                before_shot_id: None,
            }),
            (true, true, false)
        );

        // UNMERGE (removes connectivity → can orphan, but adds no dangling ref).
        assert_eq!(
            caps(&Command::UnmergeStations {
                station_ref: StationRef::new(1, 0)
            }),
            (true, false, true)
        );

        // GEOMETRY_ONLY (shifts offsets only).
        assert_eq!(
            caps(&Command::MoveSection {
                section_id: 1,
                offset: Coordinates::new(1.0, 2.0, 0.0),
            }),
            (true, false, false)
        );
        // UpdateShot shifts geometry (a measurement edit moves stations) but keeps
        // refs valid and connectivity intact — it must reconcile, not be inert.
        assert_eq!(
            caps(&Command::UpdateShot {
                section_id: 1,
                shot_id: 0,
                updates: ShotUpdates::default(),
            }),
            (true, false, false)
        );
    }

    /// The data-table grid commits multi-cell edits (paste/fill) as one batch →
    /// one undo. `UpdateShot` must be batch-allowed (the batch epilogue reconciles
    /// only when an edit in the batch actually touches geometry); `SetStationComment`
    /// must NOT be (INERT metadata — it could never carry the geometry that
    /// justifies a batch, and batching it once ran the reconcile epilogue and could
    /// heal drift silently).
    #[test]
    fn update_shot_is_batch_allowed_but_station_comment_is_not() {
        assert!(AppState::is_batch_allowed(&Command::UpdateShot {
            section_id: 1,
            shot_id: 0,
            updates: ShotUpdates::default(),
        }));
        assert!(!AppState::is_batch_allowed(&Command::SetStationComment {
            station_ref: StationRef::new(1, 0),
            comment: None,
        }));
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod update_shot_clear_undo_tests {
    use super::*;

    fn shot_data(left: Option<f64>) -> ShotData {
        ShotData {
            azimuth_in: 10.0,
            azimuth_out: 10.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 1.0,
            left,
            ..Default::default()
        }
    }

    fn state_with_one_shot(left: Option<f64>) -> (AppState, SectionId, ShotId) {
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: "Clear".to_string(),
            location: None,
        });
        let section_id = state
            .execute(Command::CreateSection {
                name: None,
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![shot_data(left)],
            })
            .section_id
            .unwrap();
        let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;
        (state, section_id, shot_id)
    }

    /// A grid cell clear arrives as JSON `null` (deserialized to `Some(None)`,
    /// see `double_option`); the command must apply it and undo must restore
    /// the cleared value through the serialized `values_json` round-trip.
    #[test]
    fn update_shot_null_clears_field_and_undo_restores_it() {
        let (state, section_id, shot_id) = state_with_one_shot(Some(0.5));

        let updates: ShotUpdates =
            serde_json::from_value(serde_json::json!({ "left": null })).unwrap();
        let result = state.execute(Command::UpdateShot {
            section_id,
            shot_id,
            updates,
        });
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            state.get_project_state().unwrap().sections[0].shots[0].left,
            None
        );

        assert!(state.undo().success);
        assert_eq!(
            state.get_project_state().unwrap().sections[0].shots[0].left,
            Some(0.5),
            "undo must restore the cleared value"
        );
    }

    /// The reverse direction: setting a previously-empty field must undo back
    /// to empty — the captured previous value `Some(None)` has to survive the
    /// `values_json` round-trip as `null` rather than fold into "no update".
    #[test]
    fn update_shot_set_on_empty_field_undoes_back_to_empty() {
        let (state, section_id, shot_id) = state_with_one_shot(None);

        let updates: ShotUpdates =
            serde_json::from_value(serde_json::json!({ "left": 1.2 })).unwrap();
        let result = state.execute(Command::UpdateShot {
            section_id,
            shot_id,
            updates,
        });
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            state.get_project_state().unwrap().sections[0].shots[0].left,
            Some(1.2)
        );

        assert!(state.undo().success);
        assert_eq!(
            state.get_project_state().unwrap().sections[0].shots[0].left,
            None,
            "undo must clear the field back to its previous empty state"
        );
    }
}
