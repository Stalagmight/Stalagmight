use super::AppState;
use crate::core::{CommandResult, ValidationWarning, Validator, WarningSeverity};
use crate::models::{
    ClosurePair, ClosuresBeforeEdit, ClosuresBeforeSweep, LineId, Project, RemovedClosure,
    ShotData, ShotId, ShotKind, ShotUpdates, SurveyCredit, CLOSURE_REMOVED_ACROSS_ONE_SHOT,
    CLOSURE_REMOVED_MERGED_ENDS, FEATURE_ID_STRIDE,
};

/// The warning a delete reports for a loop closure it removed because the
/// delete left it spanning a single shot (#737). Names the closure by its
/// stations as the user read them BEFORE the delete (`before`, from
/// `Project::closures_before_edit`): a gap-close renumbers stations, and the
/// user created the closure between `Main / 1` and `Main / 5`, not whatever
/// the delete made of them. A closure `before` cannot name (it had no
/// resolved ends then) is named by its stations now.
pub(super) fn closure_removed_message(
    project: &Project,
    before: &ClosuresBeforeEdit,
    closure: &ClosurePair,
) -> String {
    let (a, b) = before.labels(closure.closure_id).map_or_else(
        || {
            (
                project.station_text(closure.station_a),
                project.station_text(closure.station_b),
            )
        },
        |(a, b)| (a.to_string(), b.to_string()),
    );
    format!(
        "Removed loop closure {a} to {b}: closing the gap left its two stations at the two ends of one shot."
    )
}

/// What made the two stations of a closure the sweep removed one point, so
/// its warning can say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OnePointCause {
    /// The edit's own promotion of another closure to a merge (#738).
    Promotion,
    /// A user merge, which may join two placed pieces since #741.
    Merge,
}

impl OnePointCause {
    /// The cause for the sweep after `cmd`.
    pub(super) fn of(cmd: &crate::core::Command) -> Self {
        if matches!(cmd, crate::core::Command::MergeStations { .. }) {
            Self::Merge
        } else {
            Self::Promotion
        }
    }
}

/// The warnings an edit reports for the loop closures its orphan-closure sweep
/// removed because their two stations are one point (#738), each named by the
/// stations it had BEFORE the edit. The frontend shows them in one toast with
/// the closures-across-one-shot ones (`notifyRemovedClosures`).
pub(super) fn merged_ends_warnings(
    removed: &[RemovedClosure],
    cause: OnePointCause,
) -> Vec<ValidationWarning> {
    removed
        .iter()
        .map(|removed| ValidationWarning {
            code: CLOSURE_REMOVED_MERGED_ENDS.to_string(),
            message: format!(
                "Removed loop closure {} to {}: {}",
                removed.labels.0,
                removed.labels.1,
                match (removed.already_one_point, cause) {
                    (true, _) => "its two stations were already one point.",
                    (false, OnePointCause::Promotion) => {
                        "another loop closure became a merge, which made its two stations one point."
                    }
                    (false, OnePointCause::Merge) => {
                        "the merge made its two stations one point."
                    }
                }
            ),
            severity: WarningSeverity::Warning,
        })
        .collect()
}

/// After an edit that ran the whole-project sweep: log what it removed, and
/// turn it into [`merged_ends_warnings`] named against `before` (the capture
/// the edit took before it changed anything).
pub(super) fn report_swept_closures(
    project: &Project,
    before: &ClosuresBeforeSweep,
    sweep: &crate::models::OrphanClosureSweep,
    context: &str,
    cause: OnePointCause,
) -> Vec<ValidationWarning> {
    sweep.log_removed(context);
    merged_ends_warnings(&project.removed_closures_named(before, sweep), cause)
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
            message: closure_removed_message(project, before, &closure),
            severity: WarningSeverity::Warning,
        });
    }
    warnings
}

impl AppState {
    pub(super) fn create_shot(
        &self,
        line_id: LineId,
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
            let Some(line) = project.lines.iter().find(|s| s.id == line_id) else {
                return CommandResult::failure(format!("Line {line_id} not found"));
            };

            if line.shots.len() >= max_shots {
                return CommandResult::failure(format!(
                    "Line \"{}\" has reached the maximum of {max_shots} shots",
                    project.line_label(line_id).unwrap_or_default()
                ));
            }

            if let Some(before_id) = before_shot_id {
                line.shots
                    .iter()
                    .position(|shot| shot.id == before_id)
                    .unwrap_or(0)
            } else if let Some(after_id) = after_shot_id {
                line.shots
                    .iter()
                    .position(|shot| shot.id == after_id)
                    .map_or(0, |pos| pos + 1)
            } else {
                line.shots.len()
            }
        };

        // The shot does not exist yet, so it has no station names to go by:
        // a refusal names its line (never a positional id).
        let new_shot = || {
            project.line_label(line_id).map_or_else(
                || "The new shot".to_string(),
                |l| format!("The new shot in line \"{l}\""),
            )
        };
        if data.length == 0.0 {
            return CommandResult::failure(format!("{} has zero length", new_shot()));
        }
        if data.length < 0.0 {
            return CommandResult::failure(format!(
                "{} has negative length: {:.1}m",
                new_shot(),
                data.length
            ));
        }

        // A new shot was surveyed by the same people, on the same day, as the
        // one it continues: with no credit of its own it takes its
        // neighbour's - the shot before the insert point, or the next one when
        // inserting at the start. Here, at the FORWARD executor only: redo and
        // restore replay the recorded shot, credit included, through the
        // `Line` methods.
        //
        // The FIRST shot of a shot-less line takes the legacy surveyor/date
        // the line kept (`Line::legacy_surveyor`) instead, and the
        // line lets it go below; the undo gives it back
        // (`CreatedShotSnapshot::consumed_legacy`).
        if data.credit().is_empty() {
            if let Some(line) = project.get_line(line_id) {
                if line.shots.is_empty() {
                    data.credit = SurveyCredit::from_legacy(
                        line.legacy_surveyor.as_deref(),
                        line.legacy_date,
                    )
                    .stored()
                    .map(std::sync::Arc::new);
                } else {
                    let neighbour = match shot_index.checked_sub(1) {
                        Some(previous) => line.shots.get(previous),
                        None => line.shots.first(),
                    };
                    data.credit = neighbour.and_then(|shot| shot.credit.clone());
                }
            }
        }

        // Validated after the insert below, which names the new station, so
        // the warnings name the shot as the user will read it.
        let validated = data.clone();

        let shot = if let Some(before_id) = before_shot_id {
            project.insert_shot_before(line_id, before_id, data, None, None)
        } else if let Some(after_id) = after_shot_id {
            project.insert_shot_after(line_id, after_id, data, None, None)
        } else {
            project.add_shot(line_id, data)
        };

        if let Some(new_shot) = shot {
            let new_shot_id = new_shot.id;
            if let Some(line) = project.get_line_mut(line_id) {
                line.legacy_surveyor = None;
                line.legacy_date = None;
            }
            // Per-project thresholds (#57) so create-time warnings match the panel.
            let warnings = Validator::validate_shot(&validated, &project.validation, || {
                project.shot_label(line_id, new_shot_id).unwrap_or_default()
            });
            tracing::debug!(
                line_id,
                shot_id = new_shot_id,
                purpose = "add shot to line",
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
        line_id: LineId,
        shot_id: ShotId,
        updates: ShotUpdates,
    ) -> CommandResult {
        self.write_shot_values(line_id, shot_id, updates, true)
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
        line_id: LineId,
        shot_id: ShotId,
        updates: ShotUpdates,
    ) -> CommandResult {
        self.write_shot_values(line_id, shot_id, updates, false)
    }

    fn write_shot_values(
        &self,
        line_id: LineId,
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
        if let Some(line) = project.lines.iter().find(|s| s.id == line_id) {
            if line.shots.iter().any(|s| s.id == shot_id) {
                if let Some(new_length) = updates.length {
                    let shot = || project.shot_label(line_id, shot_id).unwrap_or_default();
                    if new_length == 0.0 {
                        return CommandResult::failure(format!("Shot {} has zero length", shot()));
                    }
                    if new_length < 0.0 {
                        return CommandResult::failure(format!(
                            "Shot {} has negative length: {:.1}m",
                            shot(),
                            new_length
                        ));
                    }
                }

                // Reject geometry edits on frozen shots
                if enforce_frozen
                    && updates.touches_geometry()
                    && project.is_shot_frozen_by_id(line_id, shot_id)
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
            project.datum_anchor_position(line_id)
        } else {
            None
        };
        if project.update_shot(line_id, shot_id, &updates).is_some() {
            // Realign merge group offsets if geometry changed (keeps locked
            // stations pinned: merge points, zero datums)
            if updates.touches_geometry() {
                project.realign_after_shot_edit(line_id, hold_anchor_at);
            }
            tracing::debug!(
                line_id,
                shot_id,
                purpose = "update shot measurements",
                "Shot updated"
            );
            CommandResult::success()
        } else {
            CommandResult::failure(format!("Shot {line_id}:{shot_id} not found"))
        }
    }

    pub(super) fn delete_shot(&self, line_id: LineId, shot_id: ShotId) -> CommandResult {
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
        if project.is_shot_frozen_by_id(line_id, shot_id) {
            return CommandResult::failure(
                "Cannot delete a frozen shot. This shot is part of an adjusted survey network."
                    .to_string(),
            );
        }

        if let Ok(impact) = project.preview_delete_shot(line_id, shot_id) {
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
        let closures_before_sweep = project.closures_before_sweep();
        match project.remove_shot_with_rewire_reporting(line_id, shot_id) {
            Ok((_, _, swept_away)) => {
                tracing::debug!(
                    line_id,
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
                let mut removed = remove_closures_newly_across_one_shot(project, &closures_before);
                // The closures the removal's own orphan sweep took because their
                // two stations became one point (#738); `execute()`'s epilogue
                // reports any its whole-project sweep takes after this.
                crate::models::log_removed_closures(&swept_away, "delete shot");
                removed.extend(merged_ends_warnings(
                    &project.removed_closures_named_by_id(&closures_before_sweep, &swept_away),
                    OnePointCause::Promotion,
                ));
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
    /// `appended_into_empty_line` is the recorded shape of the insert being
    /// undone (both anchors absent and position 0 - an `add_shot` append into a
    /// line that had no shots); it is re-checked against the live line
    /// before it is honored.
    /// Give a line that is shot-less again the legacy pair its first shot
    /// took (the undo of that shot's creation).
    pub(super) fn restore_consumed_legacy(
        &self,
        line_id: LineId,
        pair: &(Option<String>, Option<chrono::NaiveDate>),
    ) {
        let mut guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(line) = guard
            .as_mut()
            .and_then(|p| p.get_line_mut(line_id))
            .filter(|s| s.shots.is_empty())
        {
            line.legacy_surveyor.clone_from(&pair.0);
            line.legacy_date = pair.1;
        }
    }

    pub(super) fn remove_created_shot(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        appended_into_empty_line: bool,
    ) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        // The one insert `remove_shot_with_rewire` is not the inverse of: an
        // append into an EMPTY line, whose `pos == 0` compensation displaces
        // the line and drops its station-0 refs (#413, finding 3). Every
        // other shape it mirrors exactly - an append into a non-empty line
        // leaves no station beyond the deletion point, so `anchored_sides_of_edit`
        // sees no anchored tail and compensates nothing; a mid-chain insert and a
        // prepend are undone by the same offset arithmetic that made them.
        let sole_shot = project
            .get_line(line_id)
            .is_some_and(|line| line.shots.len() == 1 && line.shots[0].id == shot_id);
        let removed = if appended_into_empty_line && sole_shot {
            project
                .remove_appended_first_shot(line_id, shot_id)
                .map(|_| ())
        } else {
            project
                .remove_shot_with_rewire(line_id, shot_id)
                .map(|_| ())
        };

        match removed {
            Ok(()) => {
                tracing::debug!(
                    line_id,
                    shot_id,
                    appended_into_empty_line = appended_into_empty_line && sole_shot,
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
        line_id: LineId,
        shot_id: ShotId,
        kind: ShotKind,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            if project.set_shot_kind(line_id, shot_id, kind).is_some() {
                tracing::info!(
                    line_id,
                    shot_id,
                    kind = kind.label(),
                    purpose = "set shot kind (permanent/jump/reference)",
                    "Shot kind updated"
                );
                CommandResult::success()
            } else {
                CommandResult::failure(format!("Shot {line_id}:{shot_id} not found"))
            }
        })
    }
}
