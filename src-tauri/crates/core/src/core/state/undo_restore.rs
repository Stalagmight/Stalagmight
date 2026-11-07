use crate::core::{Command, CommandResult};
use crate::models::{Section, SectionData, SectionId, SectionIdentity, Shot, ShotData, ShotId};

use super::AppState;

impl AppState {
    /// Re-derive the geometry every command leaves behind: the section offsets
    /// that keep merged stations colocated, and the alignment claims the loop
    /// closures make about their own endpoints.
    ///
    /// Both are treated as *derived* state: rather than trust every forward /
    /// undo / redo path to maintain them perfectly by hand (the source of the
    /// redo-desync and cascade-propagation bugs), the command layer reconciles
    /// them once after each user action. Both are a no-op when nothing changed
    /// (the common case - `O(shots + merges + closures)` thanks to the coordinate
    /// precompute) and both are skipped during the bulk-import loop
    /// (`cascade_deferred`), so it doesn't reintroduce the per-mutation cost.
    ///
    /// The two are ORDERED, and the closure re-measure runs on BOTH sides of the
    /// realign, because each direction answers a different requirement:
    ///
    /// * BEFORE, because an aligned closure is an EDGE the offset cascade solves
    ///   through, so the flags are an INPUT to `realign_merges`. Left to whatever
    ///   the last command happened to leave, that input would depend on the route
    ///   taken to a state rather than on the state - a forward command's realign
    ///   would see the pre-command flags while the undo's realign saw the swept
    ///   ones, and the two cascades would settle on offsets that differ by float
    ///   dust (a `section_offsets` key that exists on one route and not the
    ///   other, which is #413 finding 2's shape and what the fuzz roundtrip
    ///   oracle reports). Sweeping first makes the solver's input a function of
    ///   the geometry alone.
    /// * AFTER, because the realign can MOVE a section, which changes the very
    ///   gaps the claims are measured from. This function must exit with the
    ///   claims true of the geometry it is leaving, or the NEXT command's sweep
    ///   would correct them - producing a difference against a snapshot recorded
    ///   one command earlier.
    ///
    /// Both are cheap and idempotent (`O(shots + closures)`, and a no-op when
    /// nothing changed), so running the sweep twice costs a scan and buys a
    /// property. See `Project::remeasure_closure_alignment` for why a sweep that
    /// runs on the inverse at all is still a restore rather than a re-derivation.
    pub(super) fn reconcile_geometry(&self) {
        let mut guard = self.project.lock();
        let Some(project) = guard.as_mut() else {
            return;
        };
        if project.cascade_deferred {
            return;
        }
        project.remeasure_closure_alignment();
        let fixed = project.realign_merges();
        if fixed > 0 {
            tracing::warn!(
                fixed,
                purpose = "self-heal derived geometry",
                "Re-aligned drifted station merges after an operation"
            );
        }
        project.remeasure_closure_alignment();
    }

    /// Execute a command without pushing to undo stack (used for undo operations and batch)
    pub(super) fn execute_without_undo(&self, cmd: Command) -> CommandResult {
        match &cmd {
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
                // Batch sub-commands arrive verbatim off IPC (#423): strip
                // uuids here like `execute`'s own CreateSection arm does -
                // never in `AppState::create_section`, which the importers
                // share with legitimately-adopted identities.
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
            // Reachable as a batch sub-command: reversing a multi-selection is
            // one atomic user action (#24).
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
            Command::MoveSection { section_id, offset } => self.move_section(*section_id, *offset),
            Command::MoveSegment { section_id, offset } => self.move_segment(*section_id, *offset),
            Command::SetStationComment {
                station_ref,
                comment,
            } => self.set_station_comment(*station_ref, comment.clone()),
            Command::MoveStation {
                station_ref,
                offset,
            } => self.move_station(*station_ref, *offset),
            Command::Batch { commands } => self.execute_batch(commands.clone(), false),
            // Reachable through a Batch, which runs its sub-commands here (#394):
            // the DMP wizard's withdrawal deletes its sections and forgets their
            // import tracking in ONE batch. Without this arm the catch-all below
            // would fail that sub-command and roll the whole withdrawal back.
            Command::UntrackImport {
                file_hashes,
                section_hashes,
            } => self.untrack_import(file_hashes, section_hashes),
            // [#576 RV4-R-U3] Named, not a wildcard: a NEW Command variant must
            // decide here, at compile time, whether it can run as a redo
            // re-execution, an `InverseCommand` undo or a Batch sub-command - a
            // wildcard failed it only at runtime, rolling back a whole batch.
            // The commands that are not undoable (lifecycle, merge, Repair) and
            // the edits that record their own snapshot pair (the comment move, the
            // project settings, the entrance elevation - undone and redone from
            // those snapshots, never re-executed) are not.
            Command::CreateProject { .. }
            | Command::OpenProject { .. }
            | Command::SaveProject { .. }
            | Command::CloseProject
            | Command::MoveComment { .. }
            | Command::MergeProjects { .. }
            | Command::RepairIntegrity
            | Command::UpdateProjectSettings
            | Command::SetSurfaceElevation => {
                CommandResult::failure("Command cannot be executed in undo context".to_string())
            }
        }
    }

    /// Restore a deleted section with its original ID and its original identity.
    ///
    /// `identity` carries what `SectionData` cannot: the shots' uuids, the
    /// reverse parity and the gauge pair. A default `identity` (a legacy
    /// persisted entry, which has no such field) mints fresh uuids and resets
    /// the pair, which is the pre-fix behaviour (#413, finding 1).
    pub(super) fn restore_section(
        &self,
        original_id: SectionId,
        data: SectionData,
        identity: &SectionIdentity,
    ) -> CommandResult {
        let mut proj_guard = self.project.lock();

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        // #557: a PERSISTED undo entry carries a file-supplied id, and
        // `Section::restore_with_identity` -> `Section::new` computes
        // `id * FEATURE_ID_STRIDE` unchecked. Live undo cannot hold an
        // over-bound id (#556 bounds the mint), so this only fires on a
        // crafted or corrupted undo table - refuse the single entry.
        if original_id > crate::models::MAX_SECTION_ID {
            return CommandResult::failure(format!(
                "Cannot restore section {original_id}: section ids past {} cannot hold shots",
                crate::models::MAX_SECTION_ID
            ));
        }

        // Create section with original ID. `push_restored_section` keeps both
        // lookup caches in step (#576 A10): the restored shots keep the uuids
        // the entry recorded, and a missing cache entry, while never wrong,
        // would make every lookup into this section a linear scan.
        let section = Section::restore_with_identity(original_id, data, identity);
        project.push_restored_section(section);
        project.touch();

        // Update next_section_id if needed
        if original_id >= project.next_section_id {
            project.next_section_id = original_id + 1;
        }

        tracing::info!(section_id = original_id, "Section restored");
        CommandResult::success().with_section_id(original_id)
    }

    /// Restore a deleted shot at its original position with its original ID
    pub(super) fn restore_shot(
        &self,
        section_id: SectionId,
        original_shot_id: ShotId,
        data: ShotData,
        position: usize,
        uuid: Option<String>,
    ) -> CommandResult {
        let mut proj_guard = self.project.lock();

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        let Some(section) = project.get_section_mut(section_id) else {
            return CommandResult::failure(format!("Section {section_id} not found"));
        };

        // Recreate the shot with its original ID and UUID so the restored shot
        // keeps the identity station cross-references anchor on
        // (`StationKey::AfterShot`) and the Ariane export emits as `<UUID>`.
        //
        // Two places the identity can come from, and `Shot::with_uuid` prefers
        // the first: the payload's own `ShotData::uuid` (every entry written
        // since #423), or the `uuid` this entry recorded beside it (entries
        // written between the UUID column landing and #423). An entry older than
        // BOTH carries neither, and `with_uuid` does not mint - so this goes
        // through `uuid_or_mint`, or the restored shot would come back with no
        // identity at all: unindexed, and unnameable by any `AfterShot` anchor
        // until some later save noticed.
        //
        // The ID goes through the section's mint (A18(d), #576). It comes
        // verbatim off a persisted entry, so it is honoured only inside the
        // section's id window and while no shot holds it; otherwise the shot
        // comes back under a fresh id. `ShotId::MAX` used to go in as-is and
        // park the counter where the user's next append overflowed it.
        let shot_id = section.take_shot_id(Some(original_shot_id));
        let shot = Shot::with_uuid(shot_id, data, Some(Shot::uuid_or_mint(uuid)));

        // The one insert path that takes a payload-borne identity, so it is also
        // the one that could reintroduce a uuid the section acquired in the
        // meantime. Not reachable today: the only door a FOREIGN id enters
        // through is section creation (`Section::new`, which de-dupes), and
        // every other mint is a fresh v4 - so nothing can be holding this shot's
        // recorded uuid while the shot itself is deleted. Asserted rather than
        // argued, because the cheap repairs are both wrong here: clearing or
        // re-minting the restored shot would defeat the restore, whose entire
        // purpose is to bring that anchor back.
        let identity_already_present = section
            .shots
            .iter()
            .any(|s| s.uuid.is_some() && s.uuid == shot.uuid);
        debug_assert!(
            !identity_already_present,
            "restore_shot: section {section_id} already holds the identity being restored"
        );
        if identity_already_present {
            tracing::warn!(
                section_id,
                shot_id = original_shot_id,
                purpose = "shot identity uniqueness (#423)",
                "Restoring a shot whose UUID is already present in the section; the \
                 pre-save repair will re-mint one of them"
            );
        }

        if position <= section.shots.len() {
            section.shots.insert(position, shot);
        } else {
            section.shots.push(shot);
        }
        // The insert moved every later shot of the section (#576 A10).
        project.reindex_section_shot_uuids(section_id);

        project.touch();

        tracing::info!(
            section_id,
            shot_id,
            recorded_shot_id = original_shot_id,
            position,
            "Shot restored"
        );
        CommandResult::success()
    }
}
