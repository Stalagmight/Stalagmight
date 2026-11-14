use super::AppState;
use crate::core::{CommandResult, ValidationWarning, Validator, WarningSeverity};
use crate::models::{
    shot_display_position, station_id_label, ClosurePair, ClosuresBeforeEdit, Project, SectionId,
    ShotData, ShotId, ShotKind, ShotUpdates, SurveyCredit, CLOSURE_REMOVED_ACROSS_ONE_SHOT,
    FEATURE_ID_STRIDE,
};

/// The warning a delete reports for a loop closure it removed because the
/// delete left it spanning a single shot (#737). Names the closure by its
/// stations BEFORE the delete (`ClosurePair` from
/// `Project::closures_newly_across_one_shot`): a gap-close renumbers stations,
/// and the user created closure 0:1-0:5, not the 0:1-0:2 it became.
pub(super) fn closure_removed_message(closure: &ClosurePair) -> String {
    format!(
        "Removed loop closure {} to {}: closing the gap left its two stations at the two ends of one shot.",
        station_id_label(closure.station_a),
        station_id_label(closure.station_b)
    )
}

/// After a delete with shift: remove every loop closure the delete left tying
/// the two ends of one shot together that did not before it (#737), and
/// report each one for the user, by the stations it had before the delete.
///
/// A gap-close shortens the path between a closure's two ends - closure
/// A:1-A:4 with shots 1..2 deleted becomes A:1-A:2, one shot - so the closure
/// that closed a real loop now closes none, and the solve would only skip it.
/// It goes with the delete, inside the same undo unit (both callers record a
/// project snapshot pair), so one Undo restores the shots AND the closure.
///
/// Deliberately NOT part of `remove_shot_with_rewire*`: that is also the
/// inverse of `CreateShot`, and an inverse must only invert (#413). A closure
/// that already spanned one shot before the delete is left alone (it carries
/// its own warning); only what this delete caused is undone here. Removing a
/// skipped closure never changes another closure's status, because a skipped
/// closure unites nothing. The range-delete preview lists the same closures
/// from the same `Project::closures_newly_across_one_shot`.
pub(super) fn remove_closures_newly_across_one_shot(
    project: &mut Project,
    before: &ClosuresBeforeEdit,
) -> Vec<ValidationWarning> {
    let newly = project.closures_newly_across_one_shot(before);
    let mut warnings = Vec::with_capacity(newly.len());
    for closure in newly {
        if project.remove_loop_closure(closure.closure_id).is_err() {
            continue;
        }
        tracing::info!(
            closure_id = closure.closure_id,
            station_a = ?closure.station_a,
            station_b = ?closure.station_b,
            purpose = "drop a closure a delete left across one shot",
            "Removed loop closure that now spans a single shot"
        );
        warnings.push(ValidationWarning {
            code: CLOSURE_REMOVED_ACROSS_ONE_SHOT.to_string(),
            message: closure_removed_message(&closure),
            severity: WarningSeverity::Warning,
        });
    }
    warnings
}

impl AppState {
    pub(super) fn create_shot(
        &self,
        section_id: SectionId,
        mut data: ShotData,
        after_shot_id: Option<ShotId>,
        before_shot_id: Option<ShotId>,
    ) -> CommandResult {
        // Identity is not the caller's to name (#423) - see create_shot_impl
        // (commands/shot.rs) for the full rationale. Cleared HERE, at the
        // executor, and not only at the IPC wrapper, so the rule holds for any
        // caller that constructs `Command::CreateShot` itself rather than by
        // grace of the batch allowlist (which refuses CreateShot today - a
        // separate mechanism maintained for a different reason). Undo/redo
        // does not lose identity to this line - it replays through the
        // `UndoData::CreatedShotSnapshot` arms, never through this command.
        data.uuid = None;
        // Reject ambiguous input: both after and before cannot be specified
        if after_shot_id.is_some() && before_shot_id.is_some() {
            return CommandResult::failure(
                "Cannot specify both after_shot_id and before_shot_id".to_string(),
            );
        }

        // Reject non-finite geometry before it reaches the coordinate path
        // (NaN/inf slips past zero/negative-length checks and poisons offsets).
        if !data.all_finite() {
            return CommandResult::failure("Shot has a non-finite (NaN/inf) value".to_string());
        }

        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        let max_shots = FEATURE_ID_STRIDE.saturating_sub(1) as usize;

        // Compute the real shot index for validation messages
        let shot_index = {
            let Some(section) = project.sections.iter().find(|s| s.id == section_id) else {
                return CommandResult::failure(format!("Section {section_id} not found"));
            };

            if section.shots.len() >= max_shots {
                return CommandResult::failure(format!(
                    "Section {section_id} has reached the maximum of {max_shots} shots"
                ));
            }

            if let Some(before_id) = before_shot_id {
                section
                    .shots
                    .iter()
                    .position(|shot| shot.id == before_id)
                    .unwrap_or(0)
            } else if let Some(after_id) = after_shot_id {
                section
                    .shots
                    .iter()
                    .position(|shot| shot.id == after_id)
                    .map_or(0, |pos| pos + 1)
            } else {
                section.shots.len()
            }
        };

        if data.length == 0.0 {
            return CommandResult::failure(format!(
                "Shot {}:{} has zero length",
                section_id,
                shot_index + 1
            ));
        }
        if data.length < 0.0 {
            return CommandResult::failure(format!(
                "Shot {}:{} has negative length: {:.1}m",
                section_id,
                shot_index + 1,
                data.length
            ));
        }

        // A new shot was surveyed by the same people, on the same day, as the
        // one it continues: with no credit of its own it takes its
        // neighbour's - the shot before the insert point, or the next one when
        // inserting at the start. Here, at the FORWARD executor only: redo and
        // restore replay the recorded shot, credit included, through the
        // `Section` methods.
        //
        // The FIRST shot of a shot-less section takes the legacy surveyor/date
        // the section kept (`Section::legacy_surveyor`) instead, and the
        // section lets it go below; the undo gives it back
        // (`CreatedShotSnapshot::consumed_legacy`).
        if data.credit().is_empty() {
            if let Some(section) = project.get_section(section_id) {
                if section.shots.is_empty() {
                    data.credit = SurveyCredit::from_legacy(
                        section.legacy_surveyor.as_deref(),
                        section.legacy_date,
                    )
                    .stored()
                    .map(std::sync::Arc::new);
                } else {
                    let neighbour = match shot_index.checked_sub(1) {
                        Some(previous) => section.shots.get(previous),
                        None => section.shots.first(),
                    };
                    data.credit = neighbour.and_then(|shot| shot.credit.clone());
                }
            }
        }

        let mut warnings = Vec::new();
        // Per-project thresholds (#57) so create-time warnings match the panel.
        warnings.extend(Validator::validate_shot(
            &data,
            &project.validation,
            section_id,
            shot_index,
        ));

        let shot = if let Some(before_id) = before_shot_id {
            project.insert_shot_before(section_id, before_id, data, None, None)
        } else if let Some(after_id) = after_shot_id {
            project.insert_shot_after(section_id, after_id, data, None, None)
        } else {
            project.add_shot(section_id, data)
        };

        if let Some(new_shot) = shot {
            let new_shot_id = new_shot.id;
            if let Some(section) = project.get_section_mut(section_id) {
                section.legacy_surveyor = None;
                section.legacy_date = None;
            }
            tracing::debug!(
                section_id,
                shot_id = new_shot_id,
                purpose = "add shot to section",
                "Shot created"
            );
            let result = CommandResult::success().with_shot_id(new_shot_id);
            if warnings.is_empty() {
                result
            } else {
                result.with_warnings(warnings)
            }
        } else {
            CommandResult::failure("Failed to create shot".to_string())
        }
    }

    pub(super) fn update_shot(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
        updates: ShotUpdates,
    ) -> CommandResult {
        self.write_shot_values(section_id, shot_id, updates, true)
    }

    /// [`Self::update_shot`] for an INVERSE: the same write, with the
    /// frozen-geometry refusal skipped.
    ///
    /// **If a forward command was permitted, its inverse must be too** (#413,
    /// findings 3 and 6; docs/undo-redo.md). The frozen check says what a USER
    /// may do - do not edit geometry the survey network is solved against - and
    /// it is a question about the state the edit STARTS from, which for an undo
    /// is the state the forward command PRODUCED, not the one it was judged
    /// against. Those differ whenever the edit itself changed what is frozen,
    /// and one route does it in a single command: a shot edit that pulls a
    /// snap-to-merge closure's endpoints into coincidence promotes the closure to
    /// a rigid edge (#469 item 2), which closes an anchored loop and freezes the
    /// very shot that was just edited. `undo_inner`'s epilogue pushes a refused
    /// entry back, so that would wedge the history permanently, with every older
    /// entry sealed off beneath it.
    ///
    /// Only the intent guard is skipped. The non-finite and length checks are
    /// about the VALUES being written, not about permission, and an inverse
    /// writes values a forward command already wrote - so they can only fire on a
    /// payload that was invalid when it was recorded, which is a different bug
    /// and should still be refused.
    pub(super) fn restore_shot_values(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
        updates: ShotUpdates,
    ) -> CommandResult {
        self.write_shot_values(section_id, shot_id, updates, false)
    }

    fn write_shot_values(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
        updates: ShotUpdates,
        enforce_frozen: bool,
    ) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        // Reject non-finite geometry before it reaches the coordinate path.
        if !updates.all_finite() {
            return CommandResult::failure(
                "Shot update has a non-finite (NaN/inf) value".to_string(),
            );
        }

        // Validate the length this update SETS - and ONLY when it sets one.
        //
        // The guard used to fall back to the stored length
        // (`updates.length.unwrap_or(existing.length)`), which made a shot whose
        // STORED length is already zero or negative reject every update, kind and
        // comment included. That state is loadable: `Validator::validate_shot`
        // reports ZERO_LENGTH/NEGATIVE_LENGTH on a legacy or collaborator file as
        // a validation ERROR, it does not refuse the file - so the fallback locked
        // the invalid value out of the very edits that could correct it, and
        // wedged undo on top of that. `SetShotKind` records `PreviousValues`,
        // whose undo re-enters here with only `kind` set; the fallback failed it,
        // `undo_inner`'s epilogue pushed the entry straight back, and every older
        // entry was sealed off forever (#413).
        if let Some(section) = project.sections.iter().find(|s| s.id == section_id) {
            if section.shots.iter().any(|s| s.id == shot_id) {
                if let Some(new_length) = updates.length {
                    if new_length == 0.0 {
                        return CommandResult::failure(format!(
                            "Shot {}:{} has zero length",
                            section_id,
                            shot_display_position(shot_id, section_id)
                        ));
                    }
                    if new_length < 0.0 {
                        return CommandResult::failure(format!(
                            "Shot {}:{} has negative length: {:.1}m",
                            section_id,
                            shot_display_position(shot_id, section_id),
                            new_length
                        ));
                    }
                }

                // Reject geometry edits on frozen shots
                if enforce_frozen
                    && updates.touches_geometry()
                    && project.is_shot_frozen_by_id(section_id, shot_id)
                {
                    return CommandResult::failure(
                        "Cannot edit geometry of a frozen shot. This shot is part of an adjusted survey network.".to_string()
                    );
                }
            }
        }

        // Measured BEFORE the edit, so the realign below holds the group's datum
        // station where this edit found it instead of re-deriving an absolute
        // position from `zero_datums[0]`. The two are the same for a project
        // whose frame is self-consistent; for one whose first datum was removed
        // under it, the absolute form translates the whole rigid group by the
        // distance between the two GPS anchors - inside a command whose undo
        // data carries no offsets, so the translation is unundoable (#413
        // finding 26).
        let hold_anchor_at = if updates.touches_geometry() {
            project.datum_anchor_position(section_id)
        } else {
            None
        };
        if project.update_shot(section_id, shot_id, &updates).is_some() {
            // Realign merge group offsets if geometry changed (keeps locked
            // stations pinned: merge points, zero datums)
            if updates.touches_geometry() {
                project.realign_after_shot_edit(section_id, hold_anchor_at);
            }
            tracing::debug!(
                section_id,
                shot_id,
                purpose = "update shot measurements",
                "Shot updated"
            );
            CommandResult::success()
        } else {
            CommandResult::failure(format!("Shot {section_id}:{shot_id} not found"))
        }
    }

    pub(super) fn delete_shot(&self, section_id: SectionId, shot_id: ShotId) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        // Defense-in-depth: refuse to delete a frozen shot (collaborator-locked
        // geometry anchored to an adjusted survey network). Mirrors update_shot's
        // frozen-geometry guard; the resolve-flow UI also gates this option. Runs
        // BEFORE the shift-blocked check so a frozen shot reports the clear frozen
        // message rather than the misleading "Use split instead."
        if project.is_shot_frozen_by_id(section_id, shot_id) {
            return CommandResult::failure(
                "Cannot delete a frozen shot. This shot is part of an adjusted survey network."
                    .to_string(),
            );
        }

        if let Ok(impact) = project.preview_delete_shot(section_id, shot_id) {
            if impact.shift_blocked {
                let reasons = if impact.shift_blocked_reasons.is_empty() {
                    "Shift geometry is not allowed for this shot.".to_string()
                } else {
                    impact.shift_blocked_reasons.join(" ")
                };
                return CommandResult::failure(format!("{reasons} Use split instead."));
            }
        }

        let closures_before = project.closures_before_edit();
        match project.remove_shot_with_rewire(section_id, shot_id) {
            Ok(_) => {
                tracing::debug!(
                    section_id,
                    shot_id,
                    purpose = "remove shot and rewire geometry",
                    "Shot deleted"
                );
                // Still under the project lock and ahead of the epilogue's
                // reconcile, so the snapshot pair's after-half has it (#737).
                // No shape is known to reach it from a SINGLE delete - it
                // refuses to shift a shot that is part of any loop, and a
                // rewire onto a merge partner keeps every knot - so this is
                // the defensive half keeping both delete paths under one rule
                // (`no_delete_leaves_a_new_closure_across_one_shot`).
                let removed = remove_closures_newly_across_one_shot(project, &closures_before);
                CommandResult::success().with_warnings(removed)
            }
            Err(e) => CommandResult::failure(e),
        }
    }

    /// Remove the shot a `CreateShot` created, as the exact inverse of the
    /// insert that made it. Undo's path - NEVER a user's.
    ///
    /// This deliberately does not go through [`Self::delete_shot`]. That
    /// function's guards encode USER INTENT: refuse to tear a loop apart,
    /// refuse to touch collaborator-locked geometry. An inverse must not
    /// consult them, because **if a forward command was permitted, its inverse
    /// must be too** - and the shot being removed is one that did not exist
    /// before the command being undone, so removing it cannot break anything
    /// the insert did not already build. Routing undo through the user-facing
    /// delete produced an undo that FAILS outright, and `undo_inner` pushes a
    /// failed entry back onto the stack, so it fails identically forever and
    /// every entry beneath it becomes permanently unreachable - a wedged
    /// history (#413, finding 6). Both of `delete_shot`'s refusals fire in
    /// practice: the frozen-shot check and the shift-blocked loop/datum pair,
    /// so special-casing either one is not a fix.
    ///
    /// The guards keep applying to a USER delete: `Command::DeleteShot` still
    /// runs `delete_shot`, guards and all.
    ///
    /// `appended_into_empty_section` is the recorded shape of the insert being
    /// undone (both anchors absent and position 0 - an `add_shot` append into a
    /// section that had no shots); it is re-checked against the live section
    /// before it is honored.
    /// Give a section that is shot-less again the legacy pair its first shot
    /// took (the undo of that shot's creation).
    pub(super) fn restore_consumed_legacy(
        &self,
        section_id: SectionId,
        pair: &(Option<String>, Option<chrono::NaiveDate>),
    ) {
        let mut guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(section) = guard
            .as_mut()
            .and_then(|p| p.get_section_mut(section_id))
            .filter(|s| s.shots.is_empty())
        {
            section.legacy_surveyor.clone_from(&pair.0);
            section.legacy_date = pair.1;
        }
    }

    pub(super) fn remove_created_shot(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
        appended_into_empty_section: bool,
    ) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        // The one insert `remove_shot_with_rewire` is not the inverse of: an
        // append into an EMPTY section, whose `pos == 0` compensation displaces
        // the section and drops its station-0 refs (#413, finding 3). Every
        // other shape it mirrors exactly - an append into a non-empty section
        // leaves no station beyond the deletion point, so `anchored_sides_of_edit`
        // sees no anchored tail and compensates nothing; a mid-chain insert and a
        // prepend are undone by the same offset arithmetic that made them.
        let sole_shot = project
            .get_section(section_id)
            .is_some_and(|section| section.shots.len() == 1 && section.shots[0].id == shot_id);
        let removed = if appended_into_empty_section && sole_shot {
            project
                .remove_appended_first_shot(section_id, shot_id)
                .map(|_| ())
        } else {
            project
                .remove_shot_with_rewire(section_id, shot_id)
                .map(|_| ())
        };

        match removed {
            Ok(()) => {
                tracing::debug!(
                    section_id,
                    shot_id,
                    appended_into_empty_section = appended_into_empty_section && sole_shot,
                    purpose = "undo a shot creation",
                    "Created shot removed"
                );
                CommandResult::success()
            }
            Err(e) => CommandResult::failure(e),
        }
    }

    pub(super) fn set_shot_kind(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
        kind: ShotKind,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            if project.set_shot_kind(section_id, shot_id, kind).is_some() {
                tracing::info!(
                    section_id,
                    shot_id,
                    kind = kind.label(),
                    purpose = "set shot kind (permanent/jump/reference)",
                    "Shot kind updated"
                );
                CommandResult::success()
            } else {
                CommandResult::failure(format!("Shot {section_id}:{shot_id} not found"))
            }
        })
    }
}
