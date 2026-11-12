//! `Command::EditShotCredit`: who surveyed and explored a set of shots, and
//! when - plus the per-shot undo record it and the legacy section-level
//! surveyor/date edit share (`UndoData::ShotCredits`).

use super::AppState;
use crate::core::{Command, CommandResult, ShotCreditChange, UndoData};
use crate::models::{CreditEdit, CreditTargets, Project, SectionId, SectionUpdates, ShotId};

/// The most shots one explicit-target edit may name. A whole section is
/// bounded by the per-section shot cap instead.
const MAX_EXPLICIT_TARGETS: usize = 100_000;

/// The `(section, shot)` pairs `targets` names, in order, deduplicated - or why
/// they cannot be resolved. All-or-nothing: one unknown shot fails the edit.
fn resolve_targets(
    project: &Project,
    targets: &CreditTargets,
) -> Result<Vec<(SectionId, ShotId)>, String> {
    match targets {
        CreditTargets::Section { section_id } => {
            let section = project
                .get_section(*section_id)
                .ok_or_else(|| format!("Section {section_id} not found"))?;
            Ok(section.shots.iter().map(|s| (*section_id, s.id)).collect())
        }
        CreditTargets::Shots { shots } => {
            if shots.len() > MAX_EXPLICIT_TARGETS {
                return Err(format!(
                    "A credit edit can name at most {MAX_EXPLICIT_TARGETS} shots"
                ));
            }
            let mut seen = std::collections::HashSet::with_capacity(shots.len());
            let mut out = Vec::with_capacity(shots.len());
            for &(section_id, shot_id) in shots {
                let section = project
                    .get_section(section_id)
                    .ok_or_else(|| format!("Section {section_id} not found"))?;
                if section.get_shot(shot_id).is_none() {
                    return Err(format!("Shot {shot_id} not found in section {section_id}"));
                }
                if seen.insert((section_id, shot_id)) {
                    out.push((section_id, shot_id));
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
            let mut sections: Vec<SectionId> = Vec::new();
            for &(section_id, _) in &resolved {
                if !sections.contains(&section_id) {
                    sections.push(section_id);
                }
            }
            // Every touched shot gets an identity first, so the undo record can
            // address it by the uuid that survives a reload.
            for &section_id in &sections {
                project.ensure_section_shot_uuids(section_id);
                project.reindex_section_shot_uuids(section_id);
            }
            let mut changed = 0usize;
            for &(section_id, shot_id) in &resolved {
                let Some(shot) = project
                    .get_section_mut(section_id)
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
            CommandResult::success().with_section_ids(sections)
        })
    }

    /// The `generate_undo_data` half: every shot the command can change, with
    /// its credit BEFORE, addressed by in-session id. `None` when the command
    /// cannot run (an unknown target), so nothing is pushed.
    pub(super) fn credit_undo_stub(project: &Project, cmd: &Command) -> UndoData {
        let (resolved, section_values) = match cmd {
            Command::EditShotCredit { targets, .. } => match resolve_targets(project, targets) {
                Ok(r) => (r, None),
                Err(_) => return UndoData::None,
            },
            Command::UpdateSectionMetadata {
                section_id,
                updates,
            } => {
                let Some(section) = project.get_section(*section_id) else {
                    return UndoData::None;
                };
                // The other fields, previous values only for what the update
                // sets - the `PreviousValues` capture, minus surveyor/date.
                let prev = SectionUpdates {
                    name: updates.name.as_ref().map(|_| section.name.clone()),
                    surveyor: None,
                    date: None,
                    color: updates.color.as_ref().map(|_| section.color.clone()),
                    direction: updates.direction.map(|_| section.direction),
                    quality: updates.quality.map(|_| section.quality),
                };
                let values = serde_json::to_string(&prev)
                    .ok()
                    .filter(|json| json != "{}")
                    .map(|json| (*section_id, json));
                let shots = section.shots.iter().map(|s| (*section_id, s.id)).collect();
                (shots, values)
            }
            _ => return UndoData::None,
        };
        let changes = resolved
            .into_iter()
            .filter_map(|(section_id, shot_id)| {
                let shot = project.get_section(section_id)?.get_shot(shot_id)?;
                Some(ShotCreditChange {
                    section_id,
                    shot_uuid: shot.uuid.clone().unwrap_or_default(),
                    before: shot.credit().clone(),
                    after: shot.credit().clone(),
                    shot_id,
                })
            })
            .collect();
        UndoData::ShotCredits {
            changes,
            section_values,
        }
    }

    /// The `finalize_undo_data` half: read each stub shot back after the edit -
    /// its uuid (the executor minted any missing one) and its credit AFTER - and
    /// keep only the shots that changed. `None` when nothing changed at all.
    pub(super) fn finalize_credit_undo(project: &Project, undo_data: UndoData) -> UndoData {
        let UndoData::ShotCredits {
            changes,
            section_values,
        } = undo_data
        else {
            return undo_data;
        };
        let changes: Vec<ShotCreditChange> = changes
            .into_iter()
            .filter_map(|mut change| {
                let shot = project
                    .get_section(change.section_id)?
                    .get_shot(change.shot_id)?;
                change.after = shot.credit().clone();
                change.shot_uuid = shot.uuid.clone()?;
                (change.after != change.before).then_some(change)
            })
            .collect();
        if changes.is_empty() && section_values.is_none() {
            return UndoData::None;
        }
        UndoData::ShotCredits {
            changes,
            section_values,
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
                let Some(shot_id) = project.shot_id_by_uuid(change.section_id, &change.shot_uuid)
                else {
                    missing += 1;
                    continue;
                };
                if let Some(shot) = project
                    .get_section_mut(change.section_id)
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

    /// Undo of [`UndoData::ShotCredits`]: the credits, then (for a section
    /// metadata edit) the section's other previous values.
    pub(super) fn undo_shot_credits(
        &self,
        changes: &[ShotCreditChange],
        section_values: Option<&(SectionId, String)>,
    ) -> CommandResult {
        let result = self.write_recorded_credits(changes, false);
        if !result.success {
            return result;
        }
        match section_values {
            None => result,
            Some((section_id, json)) => match serde_json::from_str::<SectionUpdates>(json) {
                Ok(prev) => self.update_section(*section_id, prev),
                Err(_) => CommandResult::failure("Failed to parse undo data".to_string()),
            },
        }
    }
}
