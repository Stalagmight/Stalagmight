//! `Command::EditShotCredit`: who surveyed and explored a set of shots, and
//! when - plus the per-shot undo record it and the legacy line-level
//! surveyor/date edit share (`UndoData::ShotCredits`).

use super::AppState;
use crate::core::{Command, CommandResult};
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
        let _ = (&targets, &edit);
        crate::core::stub::not_ported("command_credit::edit_shot_credit")
    }
}

/// The execute pipeline's mutation for the credit family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit), for rows only the result can say.
pub(super) fn apply(
    project: &mut crate::models::survey::Project,
    record: &mut super::commands::Record,
    cmd: &crate::core::Command,
) -> Result<super::commands::Applied, String> {
    let _ = (&mut *project, &mut *record);
    match cmd {
        crate::core::Command::EditShotCredit { .. } => {
            Err(super::commands::not_ported("execute::edit_shot_credit"))
        }
        _ => Err(format!(
            "{} is not a credit command",
            crate::core::command_kind(cmd)
        )),
    }
}
