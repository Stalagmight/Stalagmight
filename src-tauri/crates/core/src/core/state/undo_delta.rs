//! One step through a compact undo entry ([`UndoData::Delta`]).
//!
//! The entry holds the delta that turns the live project into the state on
//! its other side. A step serializes the live project (once - the cost every
//! snapshot restore has always paid), applies the delta to that text, installs
//! the result through the same restore every snapshot takes
//! (`restore_snapshot_into_with`: caches, the refusal bounds, the GPS-fix
//! merge, the heal carry-forward, the dust-offset invariant), and re-encodes
//! the entry as the delta from the project that restore installed (read again
//! when the restore healed it) back to the state it just left. So the entry always holds one direction, and the next
//! step is exact by construction - the delta form of the self-refreshing wall
//! a snapshot pair was (`refresh_snapshot_pair_counterpart`).
//!
//! The delta works on the text ([`Doc`]): a line it does not touch is
//! never parsed, so a step costs about what a snapshot restore costs.
//!
//! [`UndoData::Delta`]: crate::core::UndoData::Delta

use serde_json::Value;

use super::project_io::RecordedGauges;
use super::AppState;
use crate::core::{CommandResult, Doc, LineIds, ProjectDelta, SnapshotDelta};
use crate::models::{LineId, Project};

impl AppState {
    /// Step through `entry`: install the state on its other side and re-encode
    /// it as the way back. On failure the project and the entry are left as
    /// they were, so the caller can push the entry back where it came from.
    pub(super) fn step_through_delta(&self, entry: &mut SnapshotDelta) -> CommandResult {
        let Ok(mut guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };
        let Some(live) = guard.as_ref() else {
            return CommandResult::failure("No project open".to_string());
        };
        let live_text = match Self::serialize_project_snapshot(live) {
            Ok(text) => text,
            Err(e) => {
                return CommandResult::failure(format!("Failed to serialize project: {e}"));
            }
        };
        let live_doc = match Doc::parse(&live_text) {
            Ok(doc) => doc,
            Err(e) => return CommandResult::failure(format!("Failed to read project: {e}")),
        };
        let delta = match entry.delta() {
            Ok(delta) => delta,
            Err(e) => return CommandResult::failure(format!("Failed to read undo entry: {e}")),
        };
        let ids = LineIds::of_project(live);
        let mut applied = match delta.apply_doc(&live_doc, &ids) {
            Ok(applied) => applied,
            Err(e) => {
                tracing::error!(
                    purpose = "step through a compact undo entry",
                    reason = %e,
                    "The undo entry does not fit the open project; nothing was changed"
                );
                return CommandResult::failure(format!("Failed to apply undo entry: {e}"));
            }
        };
        // Only a rebuilt line can differ from the live project, so the
        // gauge rules only ever have those to look at.
        let mut built = applied.built_lines_mut();
        drop_unresolvable_carried_gauges(&mut built, live, &delta.ungauged_lines());
        let edited = gauges_the_step_removes(&built, live);
        drop(built);
        let other_doc = match applied.into_doc() {
            Ok(doc) => doc,
            Err(e) => return CommandResult::failure(format!("Failed to write undo state: {e}")),
        };
        let Some(healed) =
            Self::restore_snapshot_reporting(&mut guard, &other_doc.text(), Some(&edited))
        else {
            return CommandResult::failure("Failed to restore the undo state".to_string());
        };
        // The way back, from the state just installed to the one just left.
        // When the restore healed (a save's gauge carried forward, a uuid
        // minted, a dust offset dropped, ...) the installed project is not the
        // text it was handed, and the next step applies this delta to whatever
        // is live then - so the way back is read off the project the restore
        // installed. Otherwise the text in hand IS that project, and a second
        // serialization of a large survey is saved.
        let installed_text;
        let installed_doc;
        let from = if healed {
            installed_text = match guard.as_ref().map(Self::serialize_project_snapshot) {
                Some(Ok(text)) => text,
                Some(Err(e)) => {
                    return Self::roll_back_step(
                        &mut guard,
                        &live_text,
                        &format!("serialize: {e}"),
                    );
                }
                None => {
                    return Self::roll_back_step(&mut guard, &live_text, "no project installed");
                }
            };
            installed_doc = match Doc::parse(&installed_text) {
                Ok(doc) => doc,
                Err(e) => {
                    return Self::roll_back_step(&mut guard, &live_text, &format!("read: {e}"));
                }
            };
            &installed_doc
        } else {
            &other_doc
        };
        match ProjectDelta::between_docs(from, &live_doc).and_then(|back| entry.set_delta(&back)) {
            Ok(()) => CommandResult::success(),
            Err(e) => Self::roll_back_step(&mut guard, &live_text, &e),
        }
    }

    /// Put the live project back after a step that installed the other side
    /// but could not record the way back: the entry can only describe the
    /// state it was recorded against.
    fn roll_back_step(guard: &mut Option<Project>, live_text: &str, reason: &str) -> CommandResult {
        let restored_back = Self::restore_snapshot_into(guard, live_text);
        tracing::error!(
            purpose = "step through a compact undo entry",
            reason = %reason,
            restored_back,
            "Could not re-encode the undo entry after the step; rolled the step back"
        );
        CommandResult::failure(format!("Failed to record the way back: {reason}"))
    }
}

fn line_id(line: &Value) -> Option<LineId> {
    LineId::try_from(line.get("id")?.as_u64()?).ok()
}

fn gauge_pair(line: &Value) -> Option<(String, Option<String>)> {
    let shot = line.get("gauge_shot_uuid")?.as_str()?.to_string();
    let neighbour = line
        .get("gauge_neighbour_uuid")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((shot, neighbour))
}

fn live_gauge_pair(live: &Project, id: LineId) -> Option<(String, Option<String>)> {
    let line = live.get_line(id)?;
    Some((
        line.gauge_shot_uuid.clone()?,
        line.gauge_neighbour_uuid.clone(),
    ))
}

fn holds_shot(line: &Value, uuid: &str) -> bool {
    line.get("shots")
        .and_then(Value::as_array)
        .is_some_and(|shots| {
            shots
                .iter()
                .any(|s| s.get("uuid").and_then(Value::as_str) == Some(uuid))
        })
}

/// Clear, on a rebuilt line the delta lists as UNGAUGED
/// ([`ProjectDelta::ungauged_lines`]), a gauge pair the step carried over
/// from the live project that the line cannot resolve.
///
/// Such a pair is a save's capture, made after the entry was recorded: the
/// side the delta was encoded from had no pair there, and the delta does not
/// touch it. A snapshot restore carries a save's pair across only when the
/// restored line holds the shots it names (`reapply_heals_after_restore`),
/// because a pair that does not resolve is no pin at all - and worse than none,
/// since it stops the next save capturing one that does. A delta keeps every
/// member it does not edit, so the same rule is applied here, to exactly the
/// lines where the shots differ and a save could have made a pair.
fn drop_unresolvable_carried_gauges(built: &mut [&mut Value], live: &Project, ungauged: &[LineId]) {
    if ungauged.is_empty() {
        return;
    }
    for line in built.iter_mut() {
        let Some(id) = line_id(line).filter(|id| ungauged.contains(id)) else {
            continue;
        };
        let Some((shot, neighbour)) = gauge_pair(line) else {
            continue;
        };
        let carried = live_gauge_pair(live, id) == Some((shot.clone(), neighbour.clone()));
        let resolves =
            holds_shot(line, &shot) && neighbour.as_deref().is_none_or(|n| holds_shot(line, n));
        if carried && !resolves {
            if let Value::Object(members) = &mut **line {
                members.insert("gauge_shot_uuid".to_string(), Value::Null);
                members.insert("gauge_neighbour_uuid".to_string(), Value::Null);
            }
        }
    }
}

/// The live project's gauge pairs on the rebuilt lines that hold NONE in
/// the target: the pairs this step takes away. They are the edit's own (the
/// delta writes the recorded absence back), so the restore's carry-forward
/// must not copy them across - the `edited` set a snapshot pair reads from the
/// half it leaves (`recorded_counterpart_gauges`).
fn gauges_the_step_removes(built: &[&mut Value], live: &Project) -> RecordedGauges {
    built
        .iter()
        .filter(|line| gauge_pair(line).is_none())
        .filter_map(|line| {
            let id = line_id(line)?;
            Some((id, live_gauge_pair(live, id)?))
        })
        .collect()
}
