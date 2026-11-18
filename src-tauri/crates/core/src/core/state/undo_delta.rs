//! One step through a compact undo entry ([`UndoData::Delta`]).
//!
//! The entry holds the delta that turns the live project into the state on
//! its other side. A step serializes the live project, applies the delta,
//! installs the result through the same restore every snapshot takes
//! (`restore_snapshot_into_with`: caches, the refusal bounds, the GPS-fix
//! merge, the heal carry-forward, the dust-offset invariant), and re-encodes
//! the entry as the delta back to the state it just left. So the entry always
//! holds one direction, and the next step is exact by construction - the
//! delta form of the self-refreshing wall a snapshot pair was
//! (`refresh_snapshot_pair_counterpart`).
//!
//! [`UndoData::Delta`]: crate::core::UndoData::Delta

use std::collections::HashMap;

use serde_json::Value;

use super::project_io::RecordedGauges;
use super::AppState;
use crate::core::undo::snapshot_fixes;
use crate::core::{CommandResult, ProjectDelta, SnapshotDelta};
use crate::models::SectionId;

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
        let live_value = match snapshot_fixes::snapshot_value(live) {
            Ok(value) => value,
            Err(e) => {
                return CommandResult::failure(format!("Failed to serialize project: {e}"));
            }
        };
        let mut other = match entry.delta.apply(&live_value) {
            Ok(value) => value,
            Err(e) => {
                tracing::error!(
                    purpose = "step through a compact undo entry",
                    reason = %e,
                    "The undo entry does not fit the open project; nothing was changed"
                );
                return CommandResult::failure(format!("Failed to apply undo entry: {e}"));
            }
        };
        drop_unresolvable_carried_gauges(&mut other, &live_value, &entry.delta.ungauged_sections());
        let edited = gauges_the_step_removes(&live_value, &other);
        let other_text = match serde_json::to_string(&other) {
            Ok(text) => text,
            Err(e) => {
                return CommandResult::failure(format!("Failed to serialize undo state: {e}"));
            }
        };
        if !Self::restore_snapshot_into_with(&mut guard, &other_text, Some(&edited)) {
            return CommandResult::failure("Failed to restore the undo state".to_string());
        }
        // Re-encode from what the restore actually installed - its heals and
        // invariants included - so the delta back starts from exactly the
        // state the next step will find.
        let back = guard
            .as_ref()
            .ok_or_else(|| "No project after restore".to_string())
            .and_then(|restored| {
                snapshot_fixes::snapshot_value(restored).map_err(|e| e.to_string())
            })
            .and_then(|restored| ProjectDelta::between(&restored, &live_value));
        match back {
            Ok(delta) => {
                entry.delta = delta;
                CommandResult::success()
            }
            Err(e) => {
                // Put the live project back: the entry can only describe the
                // state it was recorded against.
                let restored_back = serde_json::to_string(&live_value)
                    .is_ok_and(|text| Self::restore_snapshot_into(&mut guard, &text));
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

/// A section's `(id, gauge shot, gauge neighbour, shot uuids)` in a
/// serialized project.
fn sections_by_id(project: &Value) -> HashMap<SectionId, &Value> {
    project
        .get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|section| {
            let id = SectionId::try_from(section.get("id")?.as_u64()?).ok()?;
            Some((id, section))
        })
        .collect()
}

fn gauge_pair(section: &Value) -> Option<(String, Option<String>)> {
    let shot = section.get("gauge_shot_uuid")?.as_str()?.to_string();
    let neighbour = section
        .get("gauge_neighbour_uuid")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((shot, neighbour))
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

/// Clear, in `other`, a gauge pair the step carried over from the live
/// project that `other` cannot resolve, on the sections the delta lists as
/// UNGAUGED ([`ProjectDelta::ungauged_sections`]).
///
/// Such a pair is a save's capture, made after the entry was recorded: the
/// side the delta was encoded from had no pair there, and the delta does not
/// touch it. A snapshot restore carries a save's pair across only when the
/// restored section holds the shots it names (`reapply_heals_after_restore`),
/// because a pair that does not resolve is no pin at all - and worse than none,
/// since it stops the next save capturing one that does. A delta keeps every
/// member it does not edit, so the same rule is applied here, to exactly the
/// sections where the shots differ and a save could have made a pair.
fn drop_unresolvable_carried_gauges(other: &mut Value, live: &Value, ungauged: &[SectionId]) {
    if ungauged.is_empty() {
        return;
    }
    let live_sections = sections_by_id(live);
    let Some(sections) = other.get_mut("sections").and_then(Value::as_array_mut) else {
        return;
    };
    for section in sections {
        let Some(id) = section
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|id| SectionId::try_from(id).ok())
        else {
            continue;
        };
        if !ungauged.contains(&id) {
            continue;
        }
        let Some((shot, neighbour)) = gauge_pair(section) else {
            continue;
        };
        let carried = live_sections
            .get(&id)
            .and_then(|s| gauge_pair(s))
            .is_some_and(|pair| pair == (shot.clone(), neighbour.clone()));
        let resolves =
            holds_shot(section, &shot) && neighbour.as_deref().is_none_or(|n| holds_shot(section, n));
        if carried && !resolves {
            if let Value::Object(members) = section {
                members.insert("gauge_shot_uuid".to_string(), Value::Null);
                members.insert("gauge_neighbour_uuid".to_string(), Value::Null);
            }
        }
    }
}

/// The live project's gauge pairs on the sections `other` holds WITHOUT one:
/// the pairs this step takes away. They are the edit's own (the delta writes
/// the recorded absence back), so the restore's carry-forward must not copy
/// them across - the `edited` set a snapshot pair reads from the half it
/// leaves (`recorded_counterpart_gauges`).
fn gauges_the_step_removes(live: &Value, other: &Value) -> RecordedGauges {
    let live_sections = sections_by_id(live);
    sections_by_id(other)
        .into_iter()
        .filter(|(_, section)| gauge_pair(section).is_none())
        .filter_map(|(id, _)| Some((id, gauge_pair(live_sections.get(&id)?)?)))
        .collect()
}
