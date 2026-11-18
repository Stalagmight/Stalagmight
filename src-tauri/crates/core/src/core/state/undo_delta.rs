//! One step through a compact undo entry ([`UndoData::Delta`]).
//!
//! The entry holds the delta that turns the live project into the state on
//! its other side. A step serializes the live project (once - the cost every
//! snapshot restore has always paid), applies the delta to that text, installs
//! the result through the same restore every snapshot takes
//! (`restore_snapshot_into_with`: caches, the refusal bounds, the GPS-fix
//! merge, the heal carry-forward, the dust-offset invariant), and re-encodes
//! the entry as the delta back to the state it just left. So the entry always
//! holds one direction, and the next step is exact by construction - the
//! delta form of the self-refreshing wall a snapshot pair was
//! (`refresh_snapshot_pair_counterpart`).
//!
//! The delta works on the text ([`Doc`]): a section it does not touch is
//! never parsed, so a step costs about what a snapshot restore costs.
//!
//! [`UndoData::Delta`]: crate::core::UndoData::Delta

use serde_json::Value;

use super::project_io::RecordedGauges;
use super::AppState;
use crate::core::{CommandResult, Doc, ProjectDelta, SectionIds, SnapshotDelta};
use crate::models::{Project, SectionId};

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
        let ids = SectionIds::of_project(live);
        let mut applied = match entry.delta.apply_doc(&live_doc, &ids) {
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
        // Only a rebuilt section can differ from the live project, so the
        // gauge rules only ever have those to look at.
        let mut built = applied.built_sections_mut();
        drop_unresolvable_carried_gauges(&mut built, live, &entry.delta.ungauged_sections());
        let edited = gauges_the_step_removes(&built, live);
        drop(built);
        let other_doc = match applied.into_doc() {
            Ok(doc) => doc,
            Err(e) => return CommandResult::failure(format!("Failed to write undo state: {e}")),
        };
        if !Self::restore_snapshot_into_with(&mut guard, &other_doc.text(), Some(&edited)) {
            return CommandResult::failure("Failed to restore the undo state".to_string());
        }
        // The way back, from the state just written to the one just left. The
        // restore's own heals (the uuid mint, the gauge carry-forward the
        // `edited` set blocks, the dust-offset drop) cannot change what this
        // delta installed, so the two documents in hand are the two states.
        match ProjectDelta::between_docs(&other_doc, &live_doc) {
            Ok(delta) => {
                entry.delta = delta;
                CommandResult::success()
            }
            Err(e) => {
                // Put the live project back: the entry can only describe the
                // state it was recorded against.
                let restored_back = Self::restore_snapshot_into(&mut guard, &live_text);
                tracing::error!(
                    purpose = "step through a compact undo entry",
                    reason = %e,
                    restored_back,
                    "Could not re-encode the undo entry after the step; rolled the step back"
                );
                CommandResult::failure(format!("Failed to record the way back: {e}"))
            }
        }
    }
}

fn section_id(section: &Value) -> Option<SectionId> {
    SectionId::try_from(section.get("id")?.as_u64()?).ok()
}

fn gauge_pair(section: &Value) -> Option<(String, Option<String>)> {
    let shot = section.get("gauge_shot_uuid")?.as_str()?.to_string();
    let neighbour = section
        .get("gauge_neighbour_uuid")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((shot, neighbour))
}

fn live_gauge_pair(live: &Project, id: SectionId) -> Option<(String, Option<String>)> {
    let section = live.get_section(id)?;
    Some((
        section.gauge_shot_uuid.clone()?,
        section.gauge_neighbour_uuid.clone(),
    ))
}

fn holds_shot(section: &Value, uuid: &str) -> bool {
    section
        .get("shots")
        .and_then(Value::as_array)
        .is_some_and(|shots| {
            shots
                .iter()
                .any(|s| s.get("uuid").and_then(Value::as_str) == Some(uuid))
        })
}

/// Clear, on a rebuilt section the delta lists as UNGAUGED
/// ([`ProjectDelta::ungauged_sections`]), a gauge pair the step carried over
/// from the live project that the section cannot resolve.
///
/// Such a pair is a save's capture, made after the entry was recorded: the
/// side the delta was encoded from had no pair there, and the delta does not
/// touch it. A snapshot restore carries a save's pair across only when the
/// restored section holds the shots it names (`reapply_heals_after_restore`),
/// because a pair that does not resolve is no pin at all - and worse than none,
/// since it stops the next save capturing one that does. A delta keeps every
/// member it does not edit, so the same rule is applied here, to exactly the
/// sections where the shots differ and a save could have made a pair.
fn drop_unresolvable_carried_gauges(
    built: &mut [&mut Value],
    live: &Project,
    ungauged: &[SectionId],
) {
    if ungauged.is_empty() {
        return;
    }
    for section in built.iter_mut() {
        let Some(id) = section_id(section).filter(|id| ungauged.contains(id)) else {
            continue;
        };
        let Some((shot, neighbour)) = gauge_pair(section) else {
            continue;
        };
        let carried = live_gauge_pair(live, id) == Some((shot.clone(), neighbour.clone()));
        let resolves = holds_shot(section, &shot)
            && neighbour.as_deref().is_none_or(|n| holds_shot(section, n));
        if carried && !resolves {
            if let Value::Object(members) = &mut **section {
                members.insert("gauge_shot_uuid".to_string(), Value::Null);
                members.insert("gauge_neighbour_uuid".to_string(), Value::Null);
            }
        }
    }
}

/// The live project's gauge pairs on the rebuilt sections that hold NONE in
/// the target: the pairs this step takes away. They are the edit's own (the
/// delta writes the recorded absence back), so the restore's carry-forward
/// must not copy them across - the `edited` set a snapshot pair reads from the
/// half it leaves (`recorded_counterpart_gauges`).
fn gauges_the_step_removes(built: &[&mut Value], live: &Project) -> RecordedGauges {
    built
        .iter()
        .filter(|section| gauge_pair(section).is_none())
        .filter_map(|section| {
            let id = section_id(section)?;
            Some((id, live_gauge_pair(live, id)?))
        })
        .collect()
}
