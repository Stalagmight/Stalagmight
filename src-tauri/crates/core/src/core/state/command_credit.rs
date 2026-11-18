//! `Command::EditShotCredit`: who surveyed and explored a set of shots, and
//! when - plus the per-shot undo record it and the legacy line-level
//! surveyor/date edit share (`UndoData::ShotCredits`).

use super::AppState;
use crate::core::{Command, CommandResult, ShotCreditChange, UndoData};
use crate::models::{CreditEdit, CreditTargets, LineId, LineUpdates, Project, ShotId};

/// The most shots one explicit-target edit may name. A whole line is
/// bounded by the per-line shot cap instead.
const MAX_EXPLICIT_TARGETS: usize = 100_000;

/// The `(line, shot)` pairs `targets` names, in order, deduplicated - or why
/// they cannot be resolved. All-or-nothing: one unknown shot fails the edit.
fn resolve_targets(
    project: &Project,
    targets: &CreditTargets,
) -> Result<Vec<(LineId, ShotId)>, String> {
    match targets {
        CreditTargets::Line { line_id } => {
            let line = project
                .get_line(*line_id)
                .ok_or_else(|| crate::models::LINE_GONE.to_string())?;
            Ok(line.shots.iter().map(|s| (*line_id, s.id)).collect())
        }
        CreditTargets::Shots { shots } => {
            if shots.len() > MAX_EXPLICIT_TARGETS {
                return Err(format!(
                    "A credit edit can name at most {MAX_EXPLICIT_TARGETS} shots"
                ));
            }
            let mut seen = std::collections::HashSet::with_capacity(shots.len());
            let mut out = Vec::with_capacity(shots.len());
            for &(line_id, shot_id) in shots {
                let line = project
                    .get_line(line_id)
                    .ok_or_else(|| crate::models::LINE_GONE.to_string())?;
                if line.get_shot(shot_id).is_none() {
                    return Err(crate::models::SHOT_GONE.to_string());
                }
                if seen.insert((line_id, shot_id)) {
                    out.push((line_id, shot_id));
                }
            }
            Ok(out)
        }
    }
}

impl AppState {
    pub(super) fn edit_shot_credit(
        &self,
        targets: &CreditTargets,
        edit: &CreditEdit,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            let resolved = match resolve_targets(project, targets) {
                Ok(r) => r,
                Err(e) => return CommandResult::failure(e),
            };
            let mut lines: Vec<LineId> = Vec::new();
            for &(line_id, _) in &resolved {
                if !lines.contains(&line_id) {
                    lines.push(line_id);
                }
            }
            // Every touched shot gets an identity first, so the undo record can
            // address it by the uuid that survives a reload.
            for &line_id in &lines {
                project.ensure_line_shot_uuids(line_id);
                project.reindex_line_shot_uuids(line_id);
            }
            let mut changed = 0usize;
            for &(line_id, shot_id) in &resolved {
                let Some(shot) = project
                    .get_line_mut(line_id)
                    .and_then(|s| s.get_shot_mut(shot_id))
                else {
                    continue;
                };
                let after = edit.apply(shot.credit());
                if &after != shot.credit() {
                    shot.set_credit(after);
                    changed += 1;
                }
            }
            if changed > 0 {
                project.touch();
            }
            tracing::debug!(
                shots = resolved.len(),
                changed,
                purpose = "edit survey credit",
                "Shot credit edited"
            );
            CommandResult::success().with_line_ids(lines)
        })
    }

    /// The `generate_undo_data` half: every shot the command can change, with
    /// its credit BEFORE, addressed by in-session id. `None` when the command
    /// cannot run (an unknown target), so nothing is pushed.
    pub(super) fn credit_undo_stub(project: &Project, cmd: &Command) -> UndoData {
        let (resolved, line_values) = match cmd {
            Command::EditShotCredit { targets, .. } => match resolve_targets(project, targets) {
                Ok(r) => (r, None),
                Err(_) => return UndoData::None,
            },
            Command::UpdateLineMetadata { line_id, updates } => {
                let Some(line) = project.get_line(*line_id) else {
                    return UndoData::None;
                };
                // The other fields, previous values only for what the update
                // sets - the `PreviousValues` capture, minus surveyor/date. A
                // shot-less line keeps the legacy pair itself, so there its
                // previous surveyor/date are line values too.
                let shotless = line.shots.is_empty();
                let prev = LineUpdates {
                    name: updates.name.as_ref().map(|_| line.name.clone()),
                    surveyor: updates
                        .surveyor
                        .as_ref()
                        .filter(|_| shotless)
                        .map(|_| line.legacy_surveyor.clone()),
                    date: updates
                        .date
                        .as_ref()
                        .filter(|_| shotless)
                        .map(|_| line.legacy_date),
                    color: updates.color.as_ref().map(|_| line.color.clone()),
                    direction: updates.direction.map(|_| line.direction),
                    quality: updates.quality.map(|_| line.quality),
                };
                let values = serde_json::to_string(&prev)
                    .ok()
                    .filter(|json| json != "{}")
                    .map(|json| (*line_id, json));
                let shots = line.shots.iter().map(|s| (*line_id, s.id)).collect();
                (shots, values)
            }
            _ => return UndoData::None,
        };
        let changes = resolved
            .into_iter()
            .filter_map(|(line_id, shot_id)| {
                let shot = project.get_line(line_id)?.get_shot(shot_id)?;
                Some(ShotCreditChange {
                    line_id,
                    shot_uuid: shot.uuid.clone().unwrap_or_default(),
                    before: shot.credit().clone(),
                    after: shot.credit().clone(),
                    shot_id,
                })
            })
            .collect();
        UndoData::ShotCredits {
            changes,
            line_values,
        }
    }

    /// The `finalize_undo_data` half: read each stub shot back after the edit -
    /// its uuid (the executor minted any missing one) and its credit AFTER - and
    /// keep only the shots that changed. `None` when nothing changed at all.
    pub(super) fn finalize_credit_undo(project: &Project, undo_data: UndoData) -> UndoData {
        let UndoData::ShotCredits {
            changes,
            line_values,
        } = undo_data
        else {
            return undo_data;
        };
        // Both executors (`edit_shot_credit`, and `Project::update_line` for
        // the legacy surveyor/date) mint a uuid for every shot they touch, so a
        // changed shot without one is a bug - and dropping it here would make its
        // undo silently incomplete. Say so rather than hide it.
        let mut unaddressable = 0usize;
        let changes: Vec<ShotCreditChange> = changes
            .into_iter()
            .filter_map(|mut change| {
                let shot = project.get_line(change.line_id)?.get_shot(change.shot_id)?;
                change.after = shot.credit().clone();
                if change.after == change.before {
                    return None;
                }
                let Some(uuid) = shot.uuid.clone().filter(|u| !u.is_empty()) else {
                    unaddressable += 1;
                    return None;
                };
                change.shot_uuid = uuid;
                Some(change)
            })
            .collect();
        if unaddressable > 0 {
            tracing::warn!(
                unaddressable,
                purpose = "record a survey-credit undo",
                "Changed shots without a uuid cannot be addressed by the undo; their credit \
                 change will not be undone"
            );
        }
        if changes.is_empty() && line_values.is_none() {
            return UndoData::None;
        }
        UndoData::ShotCredits {
            changes,
            line_values,
        }
    }

    /// Write `before` (undo) or `after` (redo) of every recorded change back
    /// onto its shot, resolved by uuid. A change whose shot no longer resolves
    /// is skipped with a warning: a credit is metadata, and failing the step
    /// would wedge every entry beneath it.
    pub(super) fn write_recorded_credits(
        &self,
        changes: &[ShotCreditChange],
        use_after: bool,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            let mut missing = 0usize;
            for change in changes {
                let Some(shot_id) = project.shot_id_by_uuid(change.line_id, &change.shot_uuid)
                else {
                    missing += 1;
                    continue;
                };
                if let Some(shot) = project
                    .get_line_mut(change.line_id)
                    .and_then(|s| s.get_shot_mut(shot_id))
                {
                    let credit = if use_after {
                        &change.after
                    } else {
                        &change.before
                    };
                    shot.set_credit(credit.clone());
                }
            }
            if missing > 0 {
                tracing::warn!(
                    missing,
                    recorded = changes.len(),
                    purpose = "restore survey credit",
                    "Some shots a credit edit recorded no longer exist; restored the rest"
                );
            }
            project.touch();
            CommandResult::success()
        })
    }

    /// Undo of [`UndoData::ShotCredits`]: the credits, then (for a line
    /// metadata edit) the line's other previous values.
    pub(super) fn undo_shot_credits(
        &self,
        changes: &[ShotCreditChange],
        line_values: Option<&(LineId, String)>,
    ) -> CommandResult {
        let result = self.write_recorded_credits(changes, false);
        if !result.success {
            return result;
        }
        match line_values {
            None => result,
            Some((line_id, json)) => match serde_json::from_str::<LineUpdates>(json) {
                Ok(prev) => self.update_line(*line_id, prev),
                Err(_) => CommandResult::failure("Failed to parse undo data".to_string()),
            },
        }
    }
}
