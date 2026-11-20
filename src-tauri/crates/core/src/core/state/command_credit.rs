//! `Command::EditShotCredit`: who surveyed and explored a set of shots, and
//! when, as one undo step however many shots it touches.
//!
//! A credit edit is metadata only: no geometry, no references, nothing to
//! settle. The undo record is the pipeline's generic changeset, which interns
//! each distinct credit once and keys every shot by its uuid.

use std::collections::HashMap;
use std::sync::Arc;

use super::commands::{Applied, Record};
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{
    CreditEdit, CreditTargets, LineId, ShotId, SurveyCredit, LINE_GONE, SHOT_GONE,
};

/// The most shots one explicit-target edit may name. A whole line is
/// bounded by the per-line shot cap instead.
const MAX_EXPLICIT_TARGETS: usize = 100_000;

/// The `(line, shot index)` pairs `targets` names, in order, deduplicated -
/// or why they cannot be resolved. All-or-nothing: one unknown shot fails the
/// edit (0.6.0's words).
fn resolve_targets(
    project: &Project,
    targets: &CreditTargets,
) -> Result<Vec<(LineId, usize)>, String> {
    match targets {
        CreditTargets::Line { line_id } => {
            let line = project
                .line(*line_id)
                .ok_or_else(|| LINE_GONE.to_string())?;
            Ok((0..line.shots.len()).map(|i| (*line_id, i)).collect())
        }
        CreditTargets::Shots { shots } => {
            if shots.len() > MAX_EXPLICIT_TARGETS {
                return Err(format!(
                    "A credit edit can name at most {MAX_EXPLICIT_TARGETS} shots"
                ));
            }
            // Each line's positions by shot id, built once per line named: a
            // selection of a thousand shots must not search its line per shot.
            let mut positions: HashMap<LineId, HashMap<ShotId, usize>> = HashMap::new();
            let mut seen = std::collections::HashSet::with_capacity(shots.len());
            let mut out = Vec::with_capacity(shots.len());
            for &(line_id, shot_id) in shots {
                let line = project.line(line_id).ok_or_else(|| LINE_GONE.to_string())?;
                let index = *positions
                    .entry(line_id)
                    .or_insert_with(|| {
                        line.shots
                            .iter()
                            .enumerate()
                            .map(|(i, s)| (s.id, i))
                            .collect()
                    })
                    .get(&shot_id)
                    .ok_or_else(|| SHOT_GONE.to_string())?;
                if seen.insert((line_id, index)) {
                    out.push((line_id, index));
                }
            }
            Ok(out)
        }
    }
}

/// What [`edit_credits`] does to a shot holding one particular credit.
enum Outcome {
    /// The edit leaves it as it is.
    Unchanged,
    /// The edit stores this (an empty credit is stored as `None`).
    Store(Option<Arc<SurveyCredit>>),
}

/// Apply `edit` to the credit of every shot in `targets` (0.6.0's
/// `edit_shot_credit`: `CreditEdit::apply` per shot, stored in its storable
/// form). Shots that held equal credits before share one allocation after,
/// as an import's do. Returns the lines touched, in order, and how many shots
/// changed.
fn edit_credits(
    project: &mut Project,
    targets: &[(LineId, usize)],
    edit: &CreditEdit,
) -> (Vec<LineId>, usize) {
    let mut lines: Vec<LineId> = Vec::new();
    if targets.is_empty() {
        return (lines, 0);
    }
    for &(line_id, _) in targets {
        if !lines.contains(&line_id) {
            lines.push(line_id);
        }
    }
    // List positions read once, before the lines are borrowed mutably (which
    // drops the topology that makes a lookup by id cheap).
    let positions: HashMap<LineId, usize> = lines
        .iter()
        .filter_map(|&id| project.line_position(id).map(|at| (id, at)))
        .collect();
    let empty = SurveyCredit::default();
    let mut memo: Option<(SurveyCredit, Outcome)> = None;
    let mut changed = 0usize;
    let all = project.lines_mut();
    for &(line_id, index) in targets {
        let Some(shot) = positions
            .get(&line_id)
            .and_then(|&at| all.get_mut(at))
            .and_then(|l| l.shots.get_mut(index))
        else {
            continue;
        };
        let before = shot.credit.as_deref().unwrap_or(&empty);
        let reuse = memo.as_ref().is_some_and(|(seen, _)| seen == before);
        if !reuse {
            let after = edit.apply(before);
            let outcome = if after == *before {
                Outcome::Unchanged
            } else {
                Outcome::Store(after.into_storable().stored().map(Arc::new))
            };
            memo = Some((before.clone(), outcome));
        }
        if let Some((_, Outcome::Store(stored))) = &memo {
            shot.credit.clone_from(stored);
            changed += 1;
        }
    }
    (lines, changed)
}

/// The execute pipeline's mutation for the credit family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit).
pub(super) fn apply(
    project: &mut Project,
    _record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    match cmd {
        Command::EditShotCredit { targets, edit } => {
            let resolved = resolve_targets(project, targets)?;
            let (lines, changed) = edit_credits(project, &resolved, edit);
            tracing::debug!(
                shots = resolved.len(),
                changed,
                purpose = "edit survey credit",
                "Shot credit edited"
            );
            Ok(Applied {
                line_ids: lines,
                ..Applied::default()
            })
        }
        _ => Err(format!(
            "{} is not a credit command",
            crate::core::command_kind(cmd)
        )),
    }
}
