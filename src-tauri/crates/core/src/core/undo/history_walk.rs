//! Which line holds which shot, in every state a loaded history records.
//!
//! The load's uuid remap (`uuid_remap.rs`) has to judge each entry against the
//! project that entry replays into: a shot a later split or combine moved is
//! held by one line in an older entry and by another live, and that is a
//! move, not a repeat (#732). A whole-project snapshot pair recorded that
//! state outright. A compact delta does not, so the remap walks each stack
//! from the live project outwards with a [`HolderWalk`] - shot uuids by
//! line, nothing else - stepping it through every entry the way undo (or
//! redo) would step the project.

use std::collections::HashMap;
use std::rc::Rc;

use serde::Deserialize;

use super::delta::HolderLine;
use super::{UndoData, UndoEntry};
use crate::models::{LineId, Project};

/// The shot ownership of one state, and an index of it.
pub(super) struct HolderWalk {
    view: Vec<HolderLine>,
    /// Every uuid the view holds, with the line of each shot holding it
    /// (repeated for a uuid held twice).
    index: HashMap<Rc<str>, Vec<LineId>>,
    /// False once an entry could not be followed; the remap then judges the
    /// rest against the live project, as it did before there were walls.
    pub(super) known: bool,
    /// Set when the entry that could not be followed was a compact delta that
    /// does not fit the state the entries above it leave - a history the
    /// steps would refuse at that entry, for good.
    pub(super) unfit_delta: bool,
}

/// What a step changed: the lines that are new or rebuilt in the state it
/// reached, with their shots.
pub(super) type Changed = Vec<HolderLine>;

impl HolderWalk {
    pub(super) fn of(project: &Project) -> Self {
        let view = project
            .lines
            .iter()
            .map(|s| {
                let shots = s
                    .shots
                    .iter()
                    .map(|shot| shot.uuid.as_deref().map(Rc::from))
                    .collect();
                (s.id, Rc::new(shots))
            })
            .collect();
        let mut walk = Self {
            view: Vec::new(),
            index: HashMap::new(),
            known: true,
            unfit_delta: false,
        };
        walk.replace(view);
        walk
    }

    /// The state reached so far.
    pub(super) fn view(&self) -> &[HolderLine] {
        &self.view
    }

    /// Does a line other than `line` hold `uuid` in this state?
    pub(super) fn held_elsewhere(&self, line: LineId, uuid: &str) -> bool {
        self.index
            .get(uuid)
            .is_some_and(|holders| holders.iter().any(|s| *s != line))
    }

    /// The lines holding `uuid`, once each.
    pub(super) fn holders_of(&self, uuid: &str) -> Vec<LineId> {
        let mut holders = self.index.get(uuid).cloned().unwrap_or_default();
        holders.sort_unstable();
        holders.dedup();
        holders
    }

    /// Step through `entry`, undoing it (`undoing`) or redoing it. Returns
    /// the lines the step rebuilt when it reached a RECORDED state (a
    /// delta's other side, a snapshot half), `None` otherwise.
    pub(super) fn step(&mut self, entry: &UndoEntry, undoing: bool) -> Option<Changed> {
        if !self.known {
            return None;
        }
        match &entry.undo_data {
            UndoData::Delta(delta) => {
                let delta = delta.delta().ok();
                // A delta that leaves every line as it is (a settings edit, a
                // merge or a move of offsets) leaves the view as it is: no
                // pass over every line for an entry that touches none.
                if delta
                    .as_ref()
                    .is_some_and(|delta| delta.check_shape().is_ok() && !delta.touches_lines())
                {
                    return Some(Vec::new());
                }
                if let Some(next) = delta.and_then(|delta| delta.step_holders(&self.view)) {
                    Some(self.replace(next))
                } else {
                    self.known = false;
                    self.unfit_delta = true;
                    None
                }
            }
            UndoData::ProjectSnapshotPair { before, after } => {
                let half = if undoing { before } else { after };
                self.reset_to_snapshot(half);
                None
            }
            UndoData::CreatedShotSnapshot {
                line_id,
                uuid,
                position,
                after_shot_uuid,
                before_shot_uuid,
                ..
            } => {
                let uuid = uuid.as_deref();
                self.edit_line(*line_id, |shots| {
                    if undoing {
                        let at = uuid
                            .and_then(|u| shots.iter().position(|s| s.as_deref() == Some(u)))
                            .or_else(|| (*position < shots.len()).then_some(*position))?;
                        shots.remove(at);
                    } else {
                        let find = |u: &Option<String>| {
                            let u = u.as_deref()?;
                            shots.iter().position(|s| s.as_deref() == Some(u))
                        };
                        let at = find(before_shot_uuid)
                            .or_else(|| find(after_shot_uuid).map(|i| i + 1))
                            .unwrap_or(*position)
                            .min(shots.len());
                        shots.insert(at, uuid.map(Rc::from));
                    }
                    Some(())
                });
                None
            }
            UndoData::CreatedLineSnapshot {
                line_id,
                line_data,
                identity,
            } => {
                if undoing {
                    self.remove_lines(&[*line_id]);
                } else {
                    let recorded = identity.as_ref().map(|identity| &identity.shot_uuids);
                    let shots = if recorded.is_none_or(Vec::is_empty) {
                        line_data
                            .iter()
                            .flat_map(|data| &data.shots)
                            .map(|s| s.uuid.as_deref().map(Rc::from))
                            .collect()
                    } else {
                        recorded
                            .into_iter()
                            .flatten()
                            .map(|u| u.as_deref().map(Rc::from))
                            .collect()
                    };
                    self.append(vec![(*line_id, Rc::new(shots))]);
                }
                None
            }
            UndoData::AddedProjectContent(content) => {
                if undoing {
                    let ids: Vec<LineId> = content.line_ids().into_iter().collect();
                    self.remove_lines(&ids);
                } else {
                    let added = content
                        .lines
                        .iter()
                        .map(|s| {
                            let shots = s
                                .shots
                                .iter()
                                .map(|shot| shot.uuid.as_deref().map(Rc::from))
                                .collect();
                            (s.id, Rc::new(shots))
                        })
                        .collect();
                    self.append(added);
                }
                None
            }
            _ => None,
        }
    }

    /// Install `next`, keeping the index in step. Returns the lines of
    /// `next` that `self` did not already share.
    fn replace(&mut self, next: Vec<HolderLine>) -> Changed {
        let identity = |list: &[HolderLine]| -> HashMap<LineId, *const Vec<Option<Rc<str>>>> {
            list.iter()
                .map(|(id, shots)| (*id, Rc::as_ptr(shots)))
                .collect()
        };
        let before = identity(&self.view);
        let after = identity(&next);
        let shared = |map: &HashMap<LineId, *const Vec<Option<Rc<str>>>>,
                      (id, shots): &HolderLine| {
            map.get(id)
                .is_some_and(|ptr| std::ptr::eq(*ptr, Rc::as_ptr(shots)))
        };
        for (id, shots) in self.view.iter().filter(|line| !shared(&after, line)) {
            for uuid in shots.iter().flatten() {
                if let Some(holders) = self.index.get_mut(uuid) {
                    if let Some(k) = holders.iter().position(|s| s == id) {
                        holders.swap_remove(k);
                    }
                    if holders.is_empty() {
                        self.index.remove(uuid);
                    }
                }
            }
        }
        let changed: Changed = next
            .iter()
            .filter(|line| !shared(&before, line))
            .cloned()
            .collect();
        for (id, shots) in &changed {
            for uuid in shots.iter().flatten().filter(|u| !u.is_empty()) {
                self.index.entry(Rc::clone(uuid)).or_default().push(*id);
            }
        }
        self.view = next;
        changed
    }

    fn edit_line(
        &mut self,
        id: LineId,
        edit: impl FnOnce(&mut Vec<Option<Rc<str>>>) -> Option<()>,
    ) {
        let Some(at) = self.view.iter().position(|(s, _)| *s == id) else {
            self.known = false;
            return;
        };
        let mut shots: Vec<Option<Rc<str>>> = self.view[at].1.iter().cloned().collect();
        if edit(&mut shots).is_none() {
            self.known = false;
            return;
        }
        let mut next = self.view.clone();
        next[at] = (id, Rc::new(shots));
        self.replace(next);
    }

    fn remove_lines(&mut self, ids: &[LineId]) {
        let next = self
            .view
            .iter()
            .filter(|(id, _)| !ids.contains(id))
            .cloned()
            .collect();
        self.replace(next);
    }

    fn append(&mut self, lines: Vec<HolderLine>) {
        let mut next = self.view.clone();
        next.extend(lines);
        self.replace(next);
    }

    fn reset_to_snapshot(&mut self, json: &str) {
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            lines: Vec<ProbeLine>,
        }
        #[derive(Deserialize)]
        struct ProbeLine {
            id: LineId,
            #[serde(default)]
            shots: Vec<ProbeShot>,
        }
        #[derive(Deserialize)]
        struct ProbeShot {
            #[serde(default)]
            uuid: Option<String>,
        }
        let Ok(probe) = serde_json::from_str::<Probe>(json) else {
            self.known = false;
            return;
        };
        let next = probe
            .lines
            .into_iter()
            .map(|s| {
                let shots = s
                    .shots
                    .into_iter()
                    .map(|shot| shot.uuid.map(Rc::from))
                    .collect();
                (s.id, Rc::new(shots))
            })
            .collect();
        self.replace(next);
    }
}
