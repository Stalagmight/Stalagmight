//! What the history machinery reads and rewrites in a delta without applying
//! it: the lines it touches, the shot uuids it carries, a holder view
//! stepped through it (the load's uuid remap), and the renames that remap
//! decides.

use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::Value;

use super::json::edit_values_mut;
use super::{
    line_id_of_map, Edit, LineRun, ProjectDelta, ShotRun, GAUGE_NEIGHBOUR, GAUGE_SHOT, SHOTS, UUID,
};
use crate::models::LineId;

impl ProjectDelta {
    /// Ids of the lines the TARGET holds whose shots this delta rebuilds
    /// and whose gauge pair it does not touch, where the SOURCE held no pair.
    ///
    /// A save captures a missing gauge pair with no undo entry of its own, so
    /// by the time this delta is applied the live project can hold a pair
    /// that neither recorded state had. A snapshot restore carries such a
    /// pair across only when the restored line can resolve it (see
    /// `reapply_heals_after_restore`); a delta, which keeps every member it
    /// does not edit, would carry it regardless. These are the lines where
    /// that can matter - the shots differ, so a pair naming a live shot can
    /// name one the target lacks - and the step applies the same resolve rule
    /// to them.
    pub fn ungauged_lines(&self) -> Vec<LineId> {
        self.lines
            .iter()
            .flat_map(|(_, runs)| runs)
            .filter_map(|run| match run {
                LineRun::Edit {
                    id, ungauged: true, ..
                } => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// Every shot uuid this delta carries literally, with the line the
    /// target holds it in (a shot it puts back whole, or a uuid an edit writes onto a copied
    /// shot; gauge and station-key uuids are not shots).
    pub fn shot_uuid_mentions(&self) -> Vec<(LineId, &str)> {
        let mut out = Vec::new();
        let Some((_, runs)) = &self.lines else {
            return out;
        };
        for run in runs {
            let (line, shots) = match run {
                LineRun::Keep(..) => continue,
                LineRun::Edit { id, shots, .. } => (Some(*id), shots),
                LineRun::New { fields, shots } => (line_id_of_map(fields), shots),
            };
            let (Some(line), Some(shots)) = (line, shots) else {
                continue;
            };
            for shot_run in shots {
                match shot_run {
                    ShotRun::Put(literal) => out.extend(
                        literal
                            .iter()
                            .filter_map(|s| s.get(UUID)?.as_str())
                            .map(|uuid| (line, uuid)),
                    ),
                    ShotRun::Copy { m, .. } => out.extend(m.iter().filter_map(|(_, edit)| {
                        let Edit::Obj(members) = edit else {
                            return None;
                        };
                        match members.get(UUID) {
                            Some(Edit::Set(Value::String(uuid))) => Some((line, uuid.as_str())),
                            _ => None,
                        }
                    })),
                }
            }
        }
        out
    }

    /// Apply shot-uuid renames (the load's uuid heal, `uuid_remap.rs`) to
    /// everything this delta carries literally: each shot's `uuid` and each
    /// line's gauge pair, keyed by the line the target holds them in,
    /// and every station key anywhere in its values. The delta's runs over the
    /// source are positional and carry no uuid, so they need nothing. Returns
    /// whether anything changed.
    pub fn remap(&mut self, rename: &dyn Fn(LineId, &str) -> Option<String>) -> bool {
        let mut changed = false;
        for edit in self.fields.values_mut() {
            changed |= edit_values_mut(edit, &mut |v| {
                crate::models::remap_station_keys_json(v, rename)
            });
        }
        let Some((_, runs)) = &mut self.lines else {
            return changed;
        };
        for run in runs {
            match run {
                LineRun::Keep(..) => {}
                LineRun::Edit {
                    id, fields, shots, ..
                } => {
                    let id = *id;
                    for (key, edit) in fields.iter_mut() {
                        if key == GAUGE_SHOT || key == GAUGE_NEIGHBOUR {
                            if let Edit::Set(value) = edit {
                                changed |= rename_string(value, id, rename);
                            }
                        }
                        changed |= edit_values_mut(edit, &mut |v| {
                            crate::models::remap_station_keys_json(v, rename)
                        });
                    }
                    if let Some(shots) = shots {
                        changed |= remap_shot_runs(shots, id, rename);
                    }
                }
                LineRun::New { fields, shots } => {
                    let Some(id) = line_id_of_map(fields) else {
                        continue;
                    };
                    for key in [GAUGE_SHOT, GAUGE_NEIGHBOUR] {
                        if let Some(value) = fields.get_mut(key) {
                            changed |= rename_string(value, id, rename);
                        }
                    }
                    if let Some(Value::Array(literal)) = fields.get_mut(SHOTS) {
                        for shot in literal {
                            if let Some(value) = shot.get_mut(UUID) {
                                changed |= rename_string(value, id, rename);
                            }
                        }
                    }
                    for value in fields.values_mut() {
                        changed |= crate::models::remap_station_keys_json(value, rename);
                    }
                    if let Some(shots) = shots {
                        changed |= remap_shot_runs(shots, id, rename);
                    }
                }
            }
        }
        changed
    }

    /// `view` - which shots, by uuid, each line of the SOURCE holds, in list
    /// order - stepped through this delta: the same view of the target. `None`
    /// when the delta does not fit the view (its recorded line count, or a
    /// run past the end).
    ///
    /// What the load's uuid remap walks a history with (`uuid_remap.rs`): it
    /// needs to know which line holds which shot in every recorded state,
    /// and a delta carries no state whole. A line the delta does not touch
    /// is shared with `view`, so a step costs the lines it touches.
    pub fn step_holders(&self, view: &[HolderLine]) -> Option<Vec<HolderLine>> {
        self.check_shape().ok()?;
        let Some((count, runs)) = &self.lines else {
            return Some(view.to_vec());
        };
        if *count != view.len() {
            return None;
        }
        let mut out = Vec::with_capacity(view.len());
        for run in runs {
            match run {
                LineRun::Keep(at, len) => {
                    out.extend_from_slice(view.get(*at..at.checked_add(*len)?)?);
                }
                LineRun::Edit { at, id, shots, .. } => {
                    let (held_id, held) = view.get(*at)?;
                    if held_id != id {
                        return None;
                    }
                    let shots = match shots {
                        None => Rc::clone(held),
                        Some(runs) => Rc::new(holder_shots(view, runs)?),
                    };
                    out.push((*id, shots));
                }
                LineRun::New { fields, shots } => {
                    let id = line_id_of_map(fields)?;
                    let held = match shots {
                        Some(runs) => holder_shots(view, runs)?,
                        None => fields
                            .get(SHOTS)
                            .and_then(Value::as_array)
                            .map(|literal| literal.iter().map(literal_uuid).collect())
                            .unwrap_or_default(),
                    };
                    out.push((id, Rc::new(held)));
                }
            }
        }
        Some(out)
    }

    /// Pin a renamed uuid onto every shot this delta COPIES into a line
    /// under an id that `rename` renames there. `view` is the holder view of
    /// the source this delta will be applied to.
    ///
    /// A copy carries no uuid - it takes the source shot's - so [`Self::remap`]
    /// cannot reach it, yet a copy across lines (a split, a combine, or a
    /// shot re-created from a same-uuid shot elsewhere) can land an id the
    /// load's heal renamed in that line. The pin is an ordinary uuid edit
    /// on the copy. Returns whether anything changed.
    pub fn rename_copies(
        &mut self,
        view: &[HolderLine],
        rename: &dyn Fn(LineId, &str) -> Option<String>,
    ) -> bool {
        let Some((_, runs)) = &mut self.lines else {
            return false;
        };
        let mut changed = false;
        for run in runs {
            let (line, shots) = match run {
                LineRun::Keep(..) => continue,
                LineRun::Edit { id, shots, .. } => (Some(*id), shots),
                LineRun::New { fields, shots } => (line_id_of_map(fields), shots),
            };
            let (Some(line), Some(shots)) = (line, shots) else {
                continue;
            };
            for shot_run in shots {
                let ShotRun::Copy {
                    sec, at, n, rev, m, ..
                } = shot_run
                else {
                    continue;
                };
                let Some((_, held)) = view.get(*sec) else {
                    continue;
                };
                for k in 0..*n {
                    let index = if *rev {
                        at.checked_sub(k)
                    } else {
                        at.checked_add(k)
                    };
                    let Some(source) = index.and_then(|i| held.get(i)) else {
                        break;
                    };
                    let slot = m.iter().position(|(offset, _)| *offset == k);
                    let pinned = slot.and_then(|i| match &m[i].1 {
                        Edit::Obj(members) => members.get(UUID),
                        _ => None,
                    });
                    let effective = match pinned {
                        Some(Edit::Set(value)) => value.as_str().map(str::to_string),
                        Some(Edit::Del) => None,
                        _ => source.as_deref().map(str::to_string),
                    };
                    let Some(new) = effective.as_deref().and_then(|u| rename(line, u)) else {
                        continue;
                    };
                    let edit = Edit::Set(Value::String(new));
                    if let Some(i) = slot {
                        // A copy edited whole is not a copy; leave it.
                        let Edit::Obj(members) = &mut m[i].1 else {
                            continue;
                        };
                        members.insert(UUID.to_string(), edit);
                    } else {
                        let mut members = BTreeMap::new();
                        members.insert(UUID.to_string(), edit);
                        let i = m.partition_point(|(offset, _)| *offset < k);
                        m.insert(i, (k, Edit::Obj(members)));
                    }
                    changed = true;
                }
            }
        }
        changed
    }

    /// The facts about one line of the TARGET that a history preview reads
    /// without the project: its name and shot count, when this delta carries
    /// the line whole (a line a delete removed, on the undo side).
    pub fn new_line(&self, id: LineId) -> Option<(Option<String>, usize)> {
        self.lines
            .iter()
            .flat_map(|(_, runs)| runs)
            .find_map(|run| match run {
                LineRun::New { fields, shots } if line_id_of_map(fields) == Some(id) => {
                    let name = fields
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let count = match shots {
                        Some(runs) => runs
                            .iter()
                            .map(|r| match r {
                                ShotRun::Copy { n, .. } => *n,
                                ShotRun::Put(v) => v.len(),
                            })
                            .sum(),
                        None => fields
                            .get(SHOTS)
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len),
                    };
                    Some((name, count))
                }
                _ => None,
            })
    }
}
/// One line of a holder view ([`ProjectDelta::step_holders`]): its id and
/// its shots' uuids in list order (`None` for a shot without one).
pub type HolderLine = (LineId, Rc<Vec<Option<Rc<str>>>>);

fn literal_uuid(shot: &Value) -> Option<Rc<str>> {
    shot.get(UUID).and_then(Value::as_str).map(Rc::from)
}

fn holder_shots(view: &[HolderLine], runs: &[ShotRun]) -> Option<Vec<Option<Rc<str>>>> {
    let mut out = Vec::new();
    for run in runs {
        match run {
            ShotRun::Copy {
                sec, at, n, rev, m, ..
            } => {
                let (_, shots) = view.get(*sec)?;
                let mut mods = m.iter().peekable();
                for k in 0..*n {
                    let index = if *rev {
                        at.checked_sub(k)?
                    } else {
                        at.checked_add(k)?
                    };
                    let mut uuid = shots.get(index)?.as_ref().map(Rc::clone);
                    if let Some((_, Edit::Obj(members))) = mods.next_if(|(offset, _)| *offset == k)
                    {
                        match members.get(UUID) {
                            Some(Edit::Set(value)) => {
                                uuid = value.as_str().map(Rc::from);
                            }
                            Some(Edit::Del) => uuid = None,
                            _ => {}
                        }
                    }
                    out.push(uuid);
                }
            }
            ShotRun::Put(literal) => out.extend(literal.iter().map(literal_uuid)),
        }
    }
    Some(out)
}

fn rename_string(
    value: &mut Value,
    line: LineId,
    rename: &dyn Fn(LineId, &str) -> Option<String>,
) -> bool {
    let Some(new) = value.as_str().and_then(|old| rename(line, old)) else {
        return false;
    };
    *value = Value::String(new);
    true
}

fn remap_shot_runs(
    runs: &mut [ShotRun],
    line: LineId,
    rename: &dyn Fn(LineId, &str) -> Option<String>,
) -> bool {
    let mut changed = false;
    for run in runs {
        match run {
            ShotRun::Put(literal) => {
                for shot in literal {
                    if let Some(value) = shot.get_mut(UUID) {
                        changed |= rename_string(value, line, rename);
                    }
                    changed |= crate::models::remap_station_keys_json(shot, rename);
                }
            }
            ShotRun::Copy { m, .. } => {
                for (_, edit) in m {
                    if let Edit::Obj(members) = edit {
                        if let Some(Edit::Set(value)) = members.get_mut(UUID) {
                            changed |= rename_string(value, line, rename);
                        }
                    }
                    changed |= edit_values_mut(edit, &mut |v| {
                        crate::models::remap_station_keys_json(v, rename)
                    });
                }
            }
        }
    }
    changed
}
