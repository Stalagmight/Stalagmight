use super::AppState;
#[cfg(test)]
use crate::core::CommentHolder;
use crate::core::{
    Command, CommandResult, IntegrityCheckResult, IntegrityRepairResult, ReversedSection, UndoData,
};
use crate::models::{Project, SectionData, SectionId, StationKey, ViolationSeverity};
#[cfg(test)]
use crate::models::{ShotData, ShotId, ShotUpdates};

/// The integrity-relevant capabilities of a [`Command`], declared in ONE place
/// ([`AppState::command_integrity`]). The `execute()` epilogue reads these flags
/// instead of three separate hand-maintained allow/deny lists, so a new command
/// variant can't silently skip an integrity step.
#[derive(Debug, Clone, Copy)]
struct CommandIntegrity {
    /// Can introduce a dangling/out-of-range cross-reference - by removing a
    /// referenced entity or renumbering station indices - and so needs the
    /// (expensive) whole-project rollback snapshot to reject corruption at the
    /// source. Geometry-only commands add valid refs or shift offsets but cannot
    /// dangle a reference, so they skip the serialize.
    can_dangle_refs: bool,
    /// Can REMOVE connectivity and strand a section so its only attachment is a
    /// loop closure - which must then be converted to a merge (issue #8). Commands
    /// that add or keep connectivity are excluded (a move must leave an inert
    /// closure independent).
    can_orphan_closures: bool,
}

impl AppState {
    /// Declare every command's integrity-relevant capabilities in one exhaustive
    /// match, so adding a [`Command`] variant FORCES a decision here (the compiler
    /// rejects a non-exhaustive match) rather than silently defaulting across three
    /// separate allow/deny lists - where a miss fails open: a skipped rollback
    /// snapshot commits corruption, a missing reconcile leaves merges drifted, and
    /// a missing orphan-closure conversion leaves a phantom loop closure.
    /// Whether a command is a lifecycle pass-through that the read-only edit gate
    /// (#191, D-B6/D-B16) must NOT block: Save/Open/Close/Create/MergeProjects.
    /// Autosave (a `SaveProject`) must keep working on a read-only remote project,
    /// and opening/closing/creating/merging are not "edits of the open project".
    /// An exhaustive `match` (like `command_integrity`) forces a deliberate choice
    /// for every new `Command` variant - anti-fail-open: a new mutating variant
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
            | ReverseSection { .. }
            | CreateShot { .. }
            | UpdateShot { .. }
            | DeleteShot { .. }
            | SetShotKind { .. }
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
            | UntrackImport { .. }
            // Adding a file INTO the open project is an edit of it (#674),
            // unlike MergeProjects, which writes a new file.
            | AddProjectFile { .. }
            | RepairIntegrity
            | UpdateProjectSettings
            | SetSurfaceElevation => false,
        }
    }

    /// The sections a command reverses, WITHOUT the parity reduction
    /// `Command::reversed_section_ids` applies - `None` when the command is not purely
    /// reverses. Parity reduction is right for the settle step (which asks "what moved?") and
    /// wrong here, where a `[A, A]` batch must still be recognised as touching A.
    fn reverse_only_sections(cmd: &Command) -> Option<Vec<SectionId>> {
        match cmd {
            Command::ReverseSection { section_id, .. } => Some(vec![*section_id]),
            Command::Batch { commands } => {
                let mut out = Vec::with_capacity(commands.len());
                for c in commands {
                    match c {
                        Command::ReverseSection { section_id, .. } => out.push(*section_id),
                        // Anything else in the batch can change something.
                        _ => return None,
                    }
                }
                (!out.is_empty()).then_some(out)
            }
            _ => None,
        }
    }

    /// Whether this command is a reverse (or a batch of reverses) whose every named section has
    /// no shots - i.e. provably a no-op, so it must not consume the redo branch or dirty a
    /// remote project. A missing section counts as "nothing to change" for the same reason.
    fn is_provable_reverse_noop(&self, cmd: &Command) -> bool {
        let Some(sections) = Self::reverse_only_sections(cmd) else {
            return false;
        };
        let guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(project) = guard.as_ref() else {
            return false;
        };
        // A MISSING section is not a no-op - it is an error, and must still fall through so the
        // command fails. Only a section that exists and has no shots is provably inert.
        sections.iter().all(|sid| {
            project
                .get_section(*sid)
                .is_some_and(|s| s.shots.is_empty())
        })
    }

    fn command_integrity(cmd: &Command) -> CommandIntegrity {
        use Command::*;
        // Named profiles keep the table readable; every variant maps to exactly one.
        const SAFE_REFS: CommandIntegrity = CommandIntegrity {
            can_dangle_refs: false,
            can_orphan_closures: false,
        };
        const REMOVES: CommandIntegrity = CommandIntegrity {
            can_dangle_refs: true,
            can_orphan_closures: true,
        };
        const CREATE_SHOT: CommandIntegrity = CommandIntegrity {
            can_dangle_refs: true,
            can_orphan_closures: false,
        };
        const UNMERGE: CommandIntegrity = CommandIntegrity {
            can_dangle_refs: false,
            can_orphan_closures: true,
        };
        // Same flags as UNMERGE, kept apart because the reason is different and
        // less obvious: removing a GPS anchor changes no topology at all. What it
        // changes is whether the conversion is ALLOWED to run - see the arm below.
        const REMOVE_DATUM: CommandIntegrity = CommandIntegrity {
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
            | SetShotKind { .. }
            // Station comments are metadata keyed by station identity; they are
            // never geometry-derived and can never introduce merge drift, so they
            // must NOT run the reconcile epilogue. Classifying them GEOMETRY_ONLY
            // would heal an already-drifted project as a side effect of a comment
            // edit, while their undo (restore the previous comment) does not invert
            // that geometry change - an asymmetric undo.
            | SetStationComment { .. }
            | MoveComment { .. }
            | RepairIntegrity
            | UpdateProjectSettings
            // Import-dedup bookkeeping: a set of hashes, referenced by nothing.
            | UntrackImport { .. }
            | SetSurfaceElevation => SAFE_REFS,

            // Remove an entity / renumber stations: can dangle refs AND strand a
            // section behind a sole-bridge closure.
            DeleteSection { .. } | DeleteShot { .. } | MergeProjects { .. } => REMOVES,

            // A batch is exactly as dangerous as the worst thing in it. Taking
            // the union rather than a blanket REMOVES matters for the data
            // table's fill-drag and paste, which emit a Batch of UpdateShots: a
            // bulk Jump/Reference reclassify is metadata-only, and grading it
            // REMOVES would run the reconcile epilogue - healing an
            // already-drifted project as a side effect of an edit whose undo
            // does not invert that geometry change (the asymmetric-undo hazard
            // spelled out for SetStationComment above).
            Batch { commands } => commands.iter().fold(SAFE_REFS, |acc, c| {
                let f = Self::command_integrity(c);
                CommandIntegrity {
                    can_dangle_refs: acc.can_dangle_refs || f.can_dangle_refs,
                    can_orphan_closures: acc.can_orphan_closures || f.can_orphan_closures,
                }
            }),

            // Inserting a shot renumbers later stations (can dangle a positional
            // ref) but cannot strand a section.
            CreateShot { .. } => CREATE_SHOT,

            // Unmerge removes connectivity (can strand a section) but adds no
            // dangling reference.
            UnmergeStations { .. } | UnmergeStationPair { .. } => UNMERGE,

            // Removing a GPS anchor strands nothing - it changes no shot and no
            // merge, so the set of sole-bridge closures is exactly what it was.
            // What it changes is the conversion's own DOUBLE-ANCHOR GUARD: a
            // sole-bridge closure between two independently anchored groups is
            // left as a closure (converting it would put two GPS anchors in one
            // rigid merge component), and dropping one of the two anchors is
            // precisely what stops that refusal. The closure becomes convertible
            // at this instant, and `redrive_offsets_after_load` WILL convert it on
            // the next open - so unless this command converts it too, memory and
            // disk disagree from here on and a reload silently reclassifies
            // (#413, the orphan-closure observation; the same convergence
            // argument as finding 10, one command further out).
            RemoveZeroDatum { .. } => REMOVE_DATUM,

            // Everything else keeps references valid and cannot strand a closure.
            // Whether it also needs geometry reconciliation is the separate,
            // exhaustive Command::affects_geometry classification.
            ReverseSection { .. }
            | CreateSection { .. }
            | UpdateShot { .. }
            | MergeStations { .. }
            | CreateLoopClosure { .. }
            | RemoveLoopClosure { .. }
            // ADDING an anchor moves the guard the other way - it can only turn a
            // convertible orphan into a refused one, and a refused orphan is what
            // the loader leaves alone too, so session and disk already agree.
            | SetZeroDatum { .. }
            | MoveSection { .. }
            | MoveSegment { .. }
            | MoveStation { .. } => SAFE_REFS,

            // A pure addition (#674): it copies a source's own references
            // between the sections it creates and links nothing existing, so
            // it can neither dangle a ref nor strand a closure. No rollback
            // snapshot either - `add_project_file` blocks introduced corruption
            // itself by removing what it appended, which costs the size of the
            // file being added rather than of the open project.
            AddProjectFile { .. } => SAFE_REFS,
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

    /// The open project's frame-defining datum station, or `None` with no project
    /// and none with no resolvable datum. Read BEFORE a command runs; handed back
    /// to [`Self::renormalize_frame_if_changed`] after it.
    pub(super) fn frame_datum_key(&self) -> Option<StationKey> {
        let guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.as_ref().and_then(Project::frame_datum_key)
    }

    /// If the command that just ran changed WHICH station defines the
    /// georeference frame, translate the survey onto the new origin
    /// ([`Project::renormalize_frame`], #469 item 1).
    ///
    /// Derived, not hooked. At least six routes can promote a different datum to
    /// first - `remove_zero_datum`, `cleanup_section_references` on a
    /// `DeleteSection`, the delete/split entry points, `repair_structural_integrity`,
    /// `set_zero_datum`, and the `ZeroDatumInfo` undo arm - and a hook set that is
    /// one call site short does not degrade, it silently skips. Comparing the frame
    /// station across the command catches all of them, including the ones nobody
    /// thought of, and asks the state rather than the command what happened.
    ///
    /// # The AFTER side must be `Some`
    ///
    /// * `Some -> None` removed the last anchor: there is no frame left to
    ///   renormalise onto, and translating onto nothing would move the survey for
    ///   no reason. No-op.
    /// * `None -> Some` ESTABLISHED the survey's first georeference, which gives
    ///   local `(0, 0)` a meaning it did not have, so the survey moves onto it -
    ///   the whole survey, rigidly, exactly as any other promotion does.
    ///   `Project::set_zero_datum` already does that inline (its subsequent-datum
    ///   arithmetic reads the invariant, so it cannot wait for an epilogue that
    ///   the import loop and redo's re-execute never reach), which makes this a
    ///   measured no-op on that route and a safety net on any other.
    ///
    /// # Never on the inverse
    ///
    /// This is the FORWARD epilogue only, alongside the orphan-closure conversion,
    /// and deliberately not inside `reconcile_geometry` - which `undo()`/`redo()`
    /// share. An inverse must RESTORE, not re-derive. Nor on the LOAD path:
    /// `resolve_offsets`' cascade is translation-invariant, so save/reopen
    /// converges with the loader untouched, and a load-time renormalisation would
    /// silently translate a merged project whose datums legitimately do not sit at
    /// the origin.
    ///
    /// # What the undo side actually guarantees
    ///
    /// Stated as what holds rather than as a blanket claim, because the gate now
    /// admits `None -> Some` as well and that is a different set of routes:
    ///
    /// * The commands that PROMOTE a datum (`RemoveZeroDatum`, `DeleteSection`,
    ///   `DeleteShot`, the delete/split entry points, `RepairIntegrity`, any
    ///   `Batch`) each record a `ProjectSnapshotPair`, so their undo hands the old
    ///   map back verbatim.
    /// * `SetZeroDatum` - the command that ordinarily ESTABLISHES a frame -
    ///   records `UndoData::ZeroDatumInfo` with a whole-project
    ///   `previous_offsets` (`Project::capture_all_section_offsets`, taken before
    ///   the mutation), which is exactly the blast radius of the translation it
    ///   performs.
    /// * `AddProjectFile` (#674) can establish one too, when the copied file
    ///   carries a datum and the open project has none. It lands that datum on
    ///   the origin itself, moving only what it copied, so this gate measures
    ///   zero; a pre-existing offset that moves anyway is recorded in its
    ///   `AddedContent::moved_offsets` and handed back by the undo.
    ///
    /// KNOWN LIMITATION, pre-existing and NOT introduced here: a datum stored
    /// against a legacy POSITIONAL anchor (`StationAnchor::Index`, written by
    /// pre-anchor builds) can go from dangling to resolving when a command GROWS
    /// its section - `CreateShot` is the reachable one - which flips the frame
    /// key under a command whose undo captures only its rigid group
    /// (`CreatedShotSnapshot`). The resulting translation is then not restored
    /// exactly by that undo. It is reachable on master already in the
    /// `Some -> Some` form (an index datum un-dangling ahead of the datum that
    /// was the frame); widening the gate adds the `None -> Some` form of the same
    /// shape. Anchors written by any current build are `SectionStart`/`AfterShot`,
    /// which no forward command can un-dangle, so this stays a legacy-file
    /// question and is deliberately left to the anchor work rather than patched
    /// here.
    ///
    /// # Why `MergeProjects` cannot fire this
    ///
    /// It is classified `affects_geometry` and dispatched through `execute()`, so
    /// it reaches this call - but `merge_projects` builds a `Project` locally,
    /// writes it to a NEW file and returns; it never touches `self.project` (there
    /// is no reference to it anywhere in `project_merge.rs`). Both reads therefore
    /// see the same unchanged open project, `before == after`, and the gate
    /// returns without measuring anything. That matters because a merged project
    /// is laid out by its own sources' datums (#682), not by the open project's
    /// frame, so a fire here would translate one onto the other. Pinned by
    /// `merge_projects_does_not_renormalise_the_open_project`.
    pub(super) fn renormalize_frame_if_changed(&self, before: Option<StationKey>) {
        let mut guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(project) = guard.as_mut() else {
            return;
        };
        project.renormalize_frame_if_changed(before);
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
    /// bypass [`Self::execute`]'s gated epilogue (TML import).
    /// Reconciles derived offsets, then reports whether this edit INTRODUCED a
    /// CRITICAL integrity violation
    /// (dangling ref / out-of-range index / non-finite geometry). A caller that
    /// captured a pre-edit snapshot rolls back to it when this returns `true`,
    /// giving these paths the same "block corruption at the source" guarantee the
    /// standard command path gets (#integrity Root 4).
    ///
    /// `pre_had_critical` is the project's critical state BEFORE the edit. Matching
    /// `guard_introduced_corruption` / `execute`, an already-critical project (one
    /// opened with a dangling ref or non-finite shot - permitted; the banner offers
    /// Repair) does not block an otherwise-clean import: only NEWLY introduced
    /// corruption rolls back.
    ///
    /// No sole-bridge scan runs here. Only an operation that can SEVER
    /// connectivity can strand a section behind a sole-bridge closure (issue #8),
    /// and the one remaining caller is a pure-addition path (a fresh import) that
    /// never creates one - so it would only pay the O(closures × connectivity)
    /// cost that used to dominate a large import.
    pub(super) fn finalize_structural_edit(&self, pre_had_critical: bool) -> bool {
        self.reconcile_geometry();
        !pre_had_critical && self.project_has_critical()
    }

    /// Structural-integrity violation messages for the open project, critical
    /// first. Empty when the project is sound or none is open. Used by the
    /// load-time "warn and offer repair" surface.
    pub fn integrity_violation_messages(&self) -> Vec<String> {
        self.integrity_check().messages
    }

    /// Full integrity check for the open project: the violation messages
    /// (critical first; empty = sound) plus whether the project is currently
    /// save-blocked (has unresolvable cross-references - the exact condition that
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
        // return `changed: false` and log - the FE treats changed:false as
        // "nothing happened" (it dirties only on changed). The coded-failure
        // surface for repair is #204's; #191 just refuses to mutate. (The
        // integrity banner stays dismissable for non-blocking corruption - see
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
        let demoted;
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
            // The frame-defining datum station before the repair. Repair never runs
            // through `execute()`, so it carries its own frame check (#469, item 1).
            //
            // No route through today's repair is known to trip it, and that is worth
            // stating rather than leaving as an unexamined "it might":
            // `repair_structural_integrity` drops zero datums whose anchor does not
            // resolve, and a datum that does not resolve is ALREADY not the frame
            // (`Project::frame_datum_key` takes the first RESOLVABLE one, matching
            // the wire, which drops dangling datums before the frontend ever sees
            // them). The check is here because the alternative is asserting that no
            // future repair step can promote a datum - which is exactly the kind of
            // claim the derive-don't-hook rule exists to avoid having to make.
            let frame_before = project.frame_datum_key();
            // Drop dangling refs/orphans and re-mint duplicate shot UUIDs.
            let repaired = project.repair_structural_integrity();
            // A cross-section repeat is healed by renaming, exactly as a load
            // does, so ignores stored against the old ids need the same alias
            // lookup until the next save re-keys them.
            if repaired
                .iter()
                .any(|v| v.category == "duplicate_shot_uuid_across_sections")
            {
                self.load_renamed_shots
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            // Demote any merge that bridges two independently datum-anchored
            // groups back to a loop closure. It must run BEFORE the conversion
            // and the re-solve: it is the one repair `resolve_offsets` cannot do
            // (an over-constrained merge can never be satisfied, only removed),
            // and undoing the promotion first means the conversion below sees the
            // healed topology. Explicit-repair only - never on load.
            demoted = project.demote_double_anchoring_merges();
            // Turn any sole-bridge closure into a merge (a closure is only a
            // loop), then re-derive offsets. Repair is an explicit "fix my
            // project" action, so use the zero-trust re-solve that never honors a
            // persisted offset. The closure just demoted is NOT re-promoted here:
            // the converter's own double-anchor guard sees the same two anchored
            // halves and leaves it alone.
            project.convert_orphan_closures_to_merges();
            // Same order as execute()'s epilogue: renormalise onto the new frame
            // before the solve that lands on it.
            project.renormalize_frame_if_changed(frame_before);
            project.resolve_offsets();
            // Repair does not go through `reconcile_geometry`, so it carries the
            // closure re-measure itself, last and off the geometry the re-solve
            // settled on (#469 item 2). Without it Repair would be the one path
            // that leaves an alignment claim untrue of the state it just built,
            // and the NEXT command's reconcile would correct it - landing on a
            // state Repair's own undo snapshot never recorded.
            project.remeasure_closure_alignment();
        } // release the guard before re-measuring (integrity_violation_messages re-locks)
          // Repair mutated measured geometry (and convert_orphan_closures_to_merges
          // doesn't bump modified_at), so drop any cached closure-adjusted solve.
        self.invalidate_adjusted_cache();
        let after = self.integrity_violation_messages().len();
        // Record an undo entry (before -> after) so the user can revert the repair.
        // Only when it actually changed the project, so a no-op repair doesn't add
        // a spurious history entry. Undo restores the pre-repair (corrupt) state -
        // which the save guard still refuses to persist - and redo re-applies the
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
                    self.mark_remote_forward_edit();
                }
            }
        }
        IntegrityRepairResult {
            // Demotions are already inside the before→after delta: each demoted
            // merge was, by construction, a counted `merge_misaligned` violation
            // that the demotion clears. The floor only guarantees that an explicit
            // repair action can never be reported to the user as "0 fixed" if the
            // rest of the violation set churns in the same pass.
            fixed: before.saturating_sub(after).max(demoted) as u32,
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
        // `execute_move_comment` is reached via this `execute()` (an early
        // `return self.execute_move_comment(..)`), so it inherits this guard -
        // see the chokepoint enumeration test
        // (`gate_rejects_every_mutation_entrypoint`).
        if !Self::is_lifecycle_passthrough(&cmd) {
            // A background lock re-verify (#202) that found the write lock gone
            // shuts the gate and refuses this edit with the re-verify code, which
            // routes to the divergence prompt. Ahead of `guard_mutation`, which
            // would otherwise report the gate it shuts as a plain read-only refusal.
            if let Err(f) = self.speleodb_collect_reverify() {
                return f;
            }
            if let Err(f) = self.guard_mutation(&cmd.description()) {
                return f;
            }
            // Lazy 20-min lock re-verify (#202): on a lock-held remote edit through
            // this chokepoint, check (at most ~3x/hr) that we still hold the write
            // lock. The GET runs in the background - this is reached from sync
            // commands on the main thread - and its verdict is collected above by a
            // later edit. No-op for local / read-only projects and when recently
            // verified. Deliberately scoped to the `execute()` path (the direct
            // mutation entrypoints - undo/redo etc. - do not re-verify; the
            // release-time fence + a 403 on push are the correctness backstops).
            self.speleodb_verify_lock_if_expired();
        }
        // A reverse of a section that has no shots provably changes nothing (#24):
        // `Project::reverse_section` returns `true` on `shot_count == 0` without touching a
        // single field. Returning here - BEFORE the cache invalidation, the undo/redo epilogue
        // and the dirty flip - is what makes that a true no-op. Previously it fell through,
        // cleared the whole redo branch and stamped `has_unpushed_edits`, proliferating a
        // phantom edit to SpeleoDB for an operation with no effect.
        //
        // Guarded in the BACKEND rather than trusting the dialog's filter: the batch path, the
        // raw IPC command and the agent harness all reach this function without passing through
        // that filter, and shot-less sections are real (a GPX waypoint imports as one).
        if self.is_provable_reverse_noop(&cmd) {
            tracing::debug!(
                command = %cmd.description(),
                "Reverse of shot-less section(s) changes nothing; skipping undo, redo-clear and dirty flip"
            );
            return CommandResult::success();
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

        // The station that currently defines the georeference frame, read before
        // dispatch so the epilogue can tell whether this command changed it
        // (#469, item 1). Cheap: a walk of `zero_datums` (a handful) with a
        // `resolve_key` each.
        let frame_before = self.frame_datum_key();

        // Snapshot before a ref-removing/renumbering command so a result that
        // introduces dangling references can be rejected and rolled back -
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
        // If the project is ALREADY corrupt (loaded that way), don't block - the
        // user repairs via the banner. We only reject commands that newly
        // introduce corruption into a clean project.
        let pre_had_critical = integrity_snapshot.is_some() && self.project_has_critical();

        let mut result = match &cmd {
            Command::CreateProject { name, location } => {
                self.create_project(name.clone(), location.clone())
            }
            Command::OpenProject { path } => self.open_project(path.clone()),
            Command::SaveProject { path } => self.save_project(path.clone()),
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
                // Identity is not the caller's to name (#423): this Command
                // arrives verbatim off IPC (create_section, execute_batch), so
                // strip uuids HERE, not in `AppState::create_section` - the
                // importers funnel through that executor with identities our
                // own parser legitimately adopted.
                shots: shots
                    .iter()
                    .cloned()
                    .map(|mut s| {
                        s.uuid = None;
                        s
                    })
                    .collect(),
            }),
            Command::DeleteSection { section_id } => self.delete_section(*section_id),
            Command::UpdateSectionMetadata {
                section_id,
                updates,
            } => self.update_section(*section_id, updates.clone()),
            Command::ReverseSection {
                section_id,
                swap_left_right,
            } => self.reverse_section(*section_id, *swap_left_right),
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
            Command::SetShotKind {
                section_id,
                shot_id,
                kind,
            } => self.set_shot_kind(*section_id, *shot_id, *kind),
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
            // Never routed here: repair is applied via `repair_project_integrity`,
            // which records its own snapshot-pair undo entry. Present only for match
            // exhaustiveness.
            Command::UntrackImport {
                file_hashes,
                section_hashes,
            } => self.untrack_import(file_hashes, section_hashes),
            Command::AddProjectFile { path, .. } => {
                self.add_project_file(path, Self::add_slot_generation(&undo_data))
            }
            Command::RepairIntegrity => CommandResult::failure(
                "RepairIntegrity is applied via repair_project_integrity, not execute()"
                    .to_string(),
            ),
            Command::UpdateProjectSettings => CommandResult::failure(
                "UpdateProjectSettings is applied via update_project_settings, not execute()"
                    .to_string(),
            ),
            Command::SetSurfaceElevation => CommandResult::failure(
                "SetSurfaceElevation is applied via set_surface_elevation, not execute()"
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
        // strand a section so its only attachment is a loop closure - which is
        // then a merge, not a loop. Convert before reconciling offsets so the new
        // merge is solved immediately (#8). Scoped to removal commands so a move
        // still leaves an inert closure independent.
        if result.success && integrity.can_orphan_closures {
            self.convert_orphan_closures_in_open_project();
        }

        // Local (0, 0) MEANS `zero_datums[0]`'s GPS, so a command that promoted a
        // different datum to first just re-defined what every stored coordinate
        // means. Translate the survey onto the new origin before the reconcile, so
        // the geometry the reconcile solves - and the after-half of any snapshot
        // pair - is the renormalised one (#469, item 1). Gated on
        // `affects_geometry` exactly as the reconcile below is: that classification
        // is what excludes the lifecycle commands, and OpenProject in particular,
        // where "the frame station changed" means a DIFFERENT PROJECT is open and
        // renormalising would translate a file the user just loaded.
        if result.success && cmd.affects_geometry() {
            self.renormalize_frame_if_changed(frame_before);
        }

        // Treat section offsets as derived: after any structural command,
        // reconcile geometry so no command path can leave merged stations
        // drifted (a no-op when nothing moved; skipped during bulk import). A
        // metadata-only UpdateShot (comment/LRUD/reference) is excluded - see
        // `Command::affects_geometry` - so it can't silently heal drift with no undo.
        if result.success && cmd.affects_geometry() {
            self.reconcile_geometry();
        }

        // An add finalizes and pushes under ONE `project` guard, bound to the
        // slot generation its stub was captured against (#674): a project
        // committed into the slot since must neither be recorded as the add's
        // content nor receive its undo entry.
        if result.success && matches!(cmd, Command::AddProjectFile { .. }) {
            if let Some(failure) = self.push_add_undo(&cmd, undo_data) {
                return failure;
            }
            self.mark_remote_forward_edit();
        } else if result.success && Self::is_undoable(&cmd) && !matches!(cmd, Command::Batch { .. })
        {
            // Push to undo stack if successful and undoable.
            // Batch commands handle their own undo push inside execute_batch()
            // (with before+after snapshots), so skip them here.
            // For create commands, finalize undo data with snapshot-based ID preservation
            let final_undo_data = self.finalize_undo_data(&cmd, undo_data);

            // An entry `undo()` cannot apply must never enter the stack: `undo()`
            // pops it, maps `UndoData::None` to a failure, and its epilogue pushes
            // the entry straight BACK - so that pop is unproductive forever and
            // every older entry underneath it is permanently unreachable. The
            // stack is persisted, so the wedge survives close/reopen (stacks
            // written before this guard are healed at load - see
            // `UndoStack::is_applicable`). Reachable today by reversing a shot-less
            // section (a GPX waypoint section, and the context menu offers Reverse
            // with no shot-count guard): the command succeeds as a no-op while
            // `generate_undo_data` returns `None`. Guarded here rather than per
            // command so any future command with the same shape is covered.
            //
            // The redo stack is cleared REGARDLESS. `None` here does NOT imply the
            // command changed nothing: `UnmergeStations` on a station that is in no
            // merge returns `CommandResult::success()` unconditionally
            // (`command_station.rs`) while `generate_undo_data` yields `None`, and
            // it is classified as affecting geometry - so the epilogue above has
            // already reconciled derived state. What the entry loses is the ability
            // to be undone, not the fact that a forward command ran; the redo branch
            // replays onto a state the project has left either way. (Accepted cost:
            // a provably-no-op forward command, such as reversing a shot-less
            // section, also drops the redo branch. "A successful forward command
            // invalidates redo" outranks preserving redo across a no-op.)
            if let Ok(mut undo_stack) = self.undo_stack.lock() {
                if matches!(final_undo_data, UndoData::None) {
                    tracing::debug!(
                        command = %cmd.description(),
                        "Command produced no undo data; not pushing an unundoable entry, clearing redo"
                    );
                    undo_stack.clear_redo();
                } else {
                    undo_stack.push_with_data(cmd.clone(), final_undo_data);
                }
            }
            // Dirty-bit flip (#191, D-B16): a successful, undo-pushed edit of a
            // remote project marks it has_unpushed_edits. Set-only; no-op for
            // local-only (load_speleo_link → None). Lifecycle pass-throughs
            // (notably autosave/SaveProject) must NOT mark dirty. Batch pushes its
            // own undo inside execute_batch and flips there.
            if !Self::is_lifecycle_passthrough(&cmd) {
                self.mark_remote_forward_edit();
            }
        }

        if result.success {
            result = self.settle_reversed_sections(&cmd, result);
        }

        result
    }

    /// Finish a successful operation that REVERSED some sections' station numbering
    /// (a forward `ReverseSection`, a batch of them, or an undo/redo of either - all
    /// re-reverse the numbering).
    ///
    /// The warning re-pin is driven off the PARITY-REDUCED list
    /// `Command::reversed_section_ids` returns, and each named section is fixed up exactly
    /// once; it rebuilds its map from the CURRENT project, so what makes undo/redo safe is the
    /// intervening re-reverse, not any self-inverse property of the call.
    ///
    /// `resolved_warnings` rows still keyed by position are out of reach of
    /// `reverse_section`'s own cross-reference rewrite (which walks `for_each_station_ref`:
    /// merges, closures, zero datums, station offsets, station comments), because the reverse
    /// detached their `shot_id` from its record.
    ///
    /// The sibling `.plnx` needs nothing here (#24). Its plans identify a station by the shots
    /// it sits BETWEEN, and a reverse does not change what a station sits between - so there is
    /// no re-pointing to do, and no window in which the two files could disagree.
    ///
    /// It also reports the sections on the result, each with the shot count now in
    /// force. A reverse keeps every station's world position, so there is nothing
    /// in the geometry for a consumer to diff: without this signal the frontend had
    /// to INFER a reverse from station positions, which cannot tell a reversed
    /// section from one that was reversed and moved in the same action.
    ///
    /// A named section that is gone or has no shots is DROPPED from the report:
    /// `Project::reverse_section` returns early on a shot-less section, so nothing
    /// was renumbered and there is no divisor for the frontend's mirror.
    pub(super) fn settle_reversed_sections(
        &self,
        cmd: &Command,
        result: CommandResult,
    ) -> CommandResult {
        let reversed = cmd.reversed_section_ids();
        if reversed.is_empty() {
            return result;
        }
        self.pin_position_keyed_warnings_after_reverse(&reversed);
        // One project lock for the whole list; both fix-ups above take (and
        // release) it themselves, and the mutex is not reentrant.
        let entries: Vec<ReversedSection> = {
            let guard = match self.project.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let Some(project) = guard.as_ref() else {
                return result;
            };
            reversed
                .iter()
                .filter_map(|section_id| {
                    let shot_count = project.get_section(*section_id)?.shots.len() as u32;
                    (shot_count > 0).then_some(ReversedSection {
                        section_id: *section_id,
                        shot_count,
                    })
                })
                .collect()
        };
        if entries.is_empty() {
            return result;
        }
        result.with_reversed_sections(entries)
    }

    // === Shot operations ===

    // === Station operations ===

    /// Check if a command is allowed in a batch (whitelist for IPC safety).
    ///
    /// `UpdateShot` is permitted so the data-table grid can commit a multi-cell
    /// edit (paste/fill) as a single undo unit: it keeps refs + connectivity
    /// intact, self-validates length/finiteness/frozen-geometry, and a geometry
    /// edit has the moved stations baked in by the batch epilogue's
    /// `reconcile_geometry()` - which the epilogue runs only when the batch
    /// actually touches geometry (see `Command::affects_geometry`), so a metadata-only
    /// UpdateShot batch (comment/LRUD/reference) commits WITHOUT reconciling.
    /// `SetStationComment` is still NOT allowed: as a whole it is INERT metadata
    /// (see `command_integrity_profiles_are_stable`), so it would never carry the
    /// geometry that justifies a batch; comment edits stay one-command-per-undo.
    ///
    /// `DeleteSection` is permitted so an abandoned import can be withdrawn as ONE
    /// user action (#394): the DMP wizard imports eagerly (the zero-datum anchor on
    /// mount, then each reachable segment), so "Cancel, remove what was imported"
    /// names N sections that must come back together on a single undo, and N
    /// separate commands would leave N undo entries and N chances to stop half way.
    /// It does NOT weaken the parity reduction in `Command::reversed_section_ids`:
    /// deleting a whole section cannot change a SURVIVING section's shot count
    /// (`cleanup_section_references` touches merges/closures/offsets/comments/
    /// datums, never shots), and a batch that reverses a section it also deleted
    /// fails on the second command ("Section N not found") and rolls back whole.
    pub(super) fn is_batch_allowed(cmd: &Command) -> bool {
        // The allowlist also prevents a path-bearing lifecycle command from being
        // nested inside an otherwise undoable Batch and uploaded in Full mode.
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
                // Reversing a multi-selection is one user action: batching gives
                // it all-or-nothing rollback and a single undo entry, instead of
                // N commands that can strand the project half-reversed.
                | Command::ReverseSection { .. }
                // Withdrawing an abandoned import is one user action too (#394) -
                // the deletes and the dedup bookkeeping that describes them have
                // to come back together, so both are batchable.
                | Command::DeleteSection { .. }
                | Command::UntrackImport { .. }
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
        // way) - captured against the pre-batch state so the guard below only
        // rejects corruption the batch itself introduces.
        let pre_had_critical = self.project_has_critical();

        // The frame-defining datum station before the batch (#469, item 1). Read
        // here rather than left to the outer `execute()` epilogue for the same
        // reason the conversion below is: the after-snapshot is taken inside this
        // function, so a renormalisation applied afterwards would sit OUTSIDE the
        // pair and undo->redo would land on the pre-renormalisation geometry.
        //
        // A batch CAN change the frame, in one shape: `is_batch_allowed` admits
        // `SetZeroDatum`, and the batch whose first sub-command adds the survey's
        // first datum goes `None -> Some`. `Project::set_zero_datum` re-frames the
        // survey inline for that one, so what is left for this bookend is any
        // FUTURE shape - a promotion by some later member of an allowlist that is
        // the kind of thing that grows. This is the only placement that stays
        // correct when it does.
        let frame_before = self.frame_datum_key();

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

        // Block corruption at the source - BEFORE pushing the undo entry. The
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
        // snapshot; otherwise undo→redo restores the pre-conversion topology - redo
        // only reconciles geometry, it does NOT re-run the conversion - and the
        // merge is silently lost. The conversion stays unconditional: a batched
        // CreateLoopClosure can itself create the sole-bridge closure that needs it.
        //
        // The reconcile, however, is gated exactly as execute() gates its own (see
        // Command::affects_geometry): a batch that touches no geometry (a multi-cell
        // paste/fill of comments / LRUD / reference flags - all metadata-only
        // UpdateShots) must NOT reconcile, or it would heal an already-drifted
        // project as a side effect of a metadata edit whose undo only restores the
        // metadata (the asymmetric-undo hazard that keeps SetStationComment out of
        // batches entirely). The outer execute() epilogue applies the same gate to
        // the Batch command, so the two passes agree.
        self.convert_orphan_closures_in_open_project();
        if commands.iter().any(Command::affects_geometry) {
            // Same order as `execute()`'s epilogue: renormalise onto the new frame
            // first, then reconcile the geometry that lands on it (#469, item 1).
            self.renormalize_frame_if_changed(frame_before);
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
            self.mark_remote_forward_edit();
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
    use crate::models::{Coordinates, ShotData, ShotKind, StationRef};

    /// Locks the integrity-capability table (one command per profile). The
    /// exhaustive match in `command_integrity` makes a new variant a *compile*
    /// error; this guards against an existing variant being moved to the wrong
    /// profile - especially the REMOVES group, where dropping `can_dangle_refs`
    /// would skip the rollback snapshot and let corruption be committed.
    #[test]
    fn command_integrity_profiles_are_stable() {
        let caps = |cmd: &Command| {
            let i = AppState::command_integrity(cmd);
            (
                cmd.affects_geometry(),
                i.can_dangle_refs,
                i.can_orphan_closures,
            )
        };

        // INERT (no geometry, no refs).
        assert_eq!(caps(&Command::CloseProject), (false, false, false));
        assert_eq!(
            caps(&Command::SaveProject { path: None }),
            (false, false, false)
        );
        // RepairIntegrity never runs through execute(); it is classified INERT.
        assert_eq!(caps(&Command::RepairIntegrity), (false, false, false));
        // Comment edits are metadata-only - INERT, so a comment edit can't run the
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
        // A batch is the UNION of what it contains, not a blanket REMOVES.
        // Empty batches never reach execute() (it early-returns), so INERT here
        // is just the identity of the fold.
        assert_eq!(
            caps(&Command::Batch { commands: vec![] }),
            (false, false, false)
        );
        // Metadata-only bulk edit - the data table's fill-drag / paste over the
        // Jump-Reference column. Must NOT run the reconcile epilogue.
        assert_eq!(
            caps(&Command::Batch {
                commands: vec![Command::UpdateShot {
                    section_id: 1,
                    shot_id: 1,
                    updates: ShotUpdates {
                        kind: Some(ShotKind::Jump),
                        ..Default::default()
                    },
                }],
            }),
            (false, false, false)
        );
        // One geometry-touching member is enough to grade the whole batch.
        assert_eq!(
            caps(&Command::Batch {
                commands: vec![
                    Command::UpdateShot {
                        section_id: 1,
                        shot_id: 1,
                        updates: ShotUpdates {
                            kind: Some(ShotKind::Jump),
                            ..Default::default()
                        },
                    },
                    Command::UpdateShot {
                        section_id: 1,
                        shot_id: 2,
                        updates: ShotUpdates {
                            length: Some(5.0),
                            ..Default::default()
                        },
                    },
                ],
            }),
            (true, false, false)
        );

        // An UpdateShot is graded by what it actually changes.
        assert_eq!(
            caps(&Command::UpdateShot {
                section_id: 1,
                shot_id: 1,
                updates: ShotUpdates {
                    kind: Some(ShotKind::Reference),
                    ..Default::default()
                },
            }),
            (false, false, false),
            "a kind-only edit is metadata, like a comment edit"
        );
        assert_eq!(
            caps(&Command::UpdateShot {
                section_id: 1,
                shot_id: 1,
                updates: ShotUpdates {
                    length: Some(5.0),
                    ..Default::default()
                },
            }),
            (true, false, false),
            "a length edit still moves stations"
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
        // refs valid and connectivity intact - it must reconcile, not be inert.
        // The update has to actually carry a measurement: an all-`None`
        // `ShotUpdates::default()` changes nothing and is graded INERT (above),
        // so using it here asserted the profile of a no-op, not of an edit.
        assert_eq!(
            caps(&Command::UpdateShot {
                section_id: 1,
                shot_id: 0,
                updates: ShotUpdates {
                    azimuth_in: Some(90.0),
                    ..Default::default()
                },
            }),
            (true, false, false)
        );
    }

    /// The data-table grid commits multi-cell edits (paste/fill) as one batch →
    /// one undo. `UpdateShot` must be batch-allowed (the batch epilogue reconciles
    /// only when an edit in the batch actually touches geometry); `SetStationComment`
    /// must NOT be (INERT metadata - it could never carry the geometry that
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
    /// to empty - the captured previous value `Some(None)` has to survive the
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
