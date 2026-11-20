//! A run of shot updates: a data-table paste of a column, or one edit (D2).
//!
//! Every update has exactly the semantics of [`Project::update_shot`] run
//! one after another (the same refusals in the same order, the same stored
//! model after each, bit for bit), at the cost of one survey pass for the
//! whole run instead of a working copy, two layouts and a validation per
//! update.
//!
//! What one update does to the stored model (plan 4.1, through the protocol
//! of `edit.rs`), on a valid model:
//! - **Geometry** (length, azimuths, depths, pitches): the shot changes; a
//!   datum-held piece stays on its datum, so nothing else stored changes; a
//!   free piece keeps the edited line's first station where it was before
//!   the edit, so its `Placement` is laid out again from that station (the
//!   placement station and its slot in the list stay); every datum that is
//!   not its piece's placement datum loses its correction (the epilogue's
//!   demotion; idempotent). The only invariant it can break is a placement
//!   that is no longer finite.
//! - **Kind** without geometry: the same epilogue with no line to hold, so
//!   only the demotion can change anything.
//! - **Annotation** (walls, comment, marker, temperature): no epilogue.
//!
//! The run keeps the raw layout of the current state, from the same walk as
//! [`Project::raw_positions`] with each line's entry index recorded, and
//! keeps it exact after each update: a datum-held piece by re-walking the
//! edited line from its entry and every line entered downstream of it (in a
//! forest a line is always entered at its station nearest the anchor, so the
//! re-walk adds the same vectors in the same order as a full walk does); a
//! free piece by walking it again from its new placement when it is next
//! read. The frozen-shot check reads the closures' alignment off that layout
//! and recomputes the frozen set only when an alignment changed.
//!
//! The project must be valid (plan 2) when the run starts; each update then
//! keeps it valid or is refused. On `Err` the project may hold the updates
//! before the refused one, so the caller rolls back (the execute pipeline
//! restores its snapshot).

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use crate::{LineId, ShotId, ShotUpdates, StationId};

use super::super::positions::{is_aligned, Layout};
use super::super::{Anchor, NumberField, Placement, Project, Topology, Violation};
use super::shots::apply_shot_updates;
use super::EditError;

/// A sequence of [`Project::update_shot`]s on one project (module docs).
pub struct ShotEditRun<'p> {
    project: &'p mut Project,
    layout: Option<RunLayout>,
    /// An update ran the edit epilogue, so the run validates at the end
    /// (an annotation alone validates nothing, as `update_shot`).
    settled: bool,
}

/// The raw layout of the run's current state, and what reads it.
struct RunLayout {
    topology: Arc<Topology>,
    /// Per component (indexed like `topology.components()`).
    anchors: Vec<Anchor>,
    /// Per dense station: its raw horizontal position.
    xy: Vec<[f64; 2]>,
    /// Per line list position: the chain index the walk entered it at.
    entry: Vec<usize>,
    /// Per component: `xy` is out of date (a free piece whose placement an
    /// update moved).
    stale: Vec<bool>,
    /// The frozen shots `(line, index)` and the closure alignment they were
    /// computed for.
    frozen: Option<(Vec<bool>, BTreeSet<(LineId, u32)>)>,
}

impl RunLayout {
    fn build(project: &Project) -> Self {
        let topology = project.topology_arc();
        let anchors = project.component_anchors();
        let mut layout = Layout::new(&topology, project.lines.len());
        for anchor in &anchors {
            project.walk_component(&topology, anchor.station(), anchor.at(), &mut layout);
        }
        Self {
            stale: vec![false; anchors.len()],
            topology,
            anchors,
            xy: layout.xy,
            entry: layout.entry,
            frozen: None,
        }
    }

    fn xy_of(&self, station: StationId) -> Option<[f64; 2]> {
        self.topology.dense(station).map(|d| self.xy[d])
    }

    /// Walk component `c` again from its anchor, if an update left it stale.
    fn refresh(&mut self, project: &Project, c: usize) {
        if !self.stale[c] {
            return;
        }
        let topology = Arc::clone(&self.topology);
        let anchor = self.anchors[c];
        let mut layout = Layout::new(&topology, project.lines.len());
        project.walk_component(&topology, anchor.station(), anchor.at(), &mut layout);
        if let Some(component) = topology.component(c) {
            for &station in &component.stations {
                if let Some(d) = topology.dense(station) {
                    self.xy[d] = layout.xy[d];
                }
            }
            for &line in &component.lines {
                if let Some(position) = topology.line_position(line) {
                    self.entry[position] = layout.entry[position];
                }
            }
        }
        self.stale[c] = false;
    }

    /// After a geometry update of the line at list position `position` in a
    /// datum-held piece: walk that line again from its entry, then every line
    /// entered at a station that walk moved, and so on downstream.
    fn rewalk_downstream(&mut self, project: &Project, position: usize) {
        let topology = Arc::clone(&self.topology);
        let index = self.entry[position];
        let Some(&from) = topology
            .chain_at(position)
            .and_then(|chain| chain.get(index))
        else {
            return;
        };
        let Some(p) = self.xy_of(from) else {
            return;
        };
        let mut queue = VecDeque::new();
        let xy = &mut self.xy;
        let mut place = |station: StationId, shown: [f64; 2]| {
            if let Some(d) = topology.dense(station) {
                xy[d] = shown;
                queue.push_back(station);
            }
        };
        project.fold_line(&topology, position, index, p, &mut place);
        while let Some(station) = queue.pop_front() {
            let Some(p) = topology.dense(station).map(|d| self.xy[d]) else {
                continue;
            };
            for slot in topology.slots_of(station) {
                let Some(line) = topology.line_position(slot.line_id) else {
                    continue;
                };
                // Entered at this station: downstream of the edit. The line
                // that placed the station was entered elsewhere.
                if self.entry[line] != slot.station_index as usize {
                    continue;
                }
                let xy = &mut self.xy;
                project.fold_line(
                    &topology,
                    line,
                    slot.station_index as usize,
                    p,
                    |station, shown| {
                        if let Some(d) = topology.dense(station) {
                            xy[d] = shown;
                            queue.push_back(station);
                        }
                    },
                );
            }
        }
    }

    /// The frozen shots of the current state ([`Project::frozen_shots`] for
    /// [`Project::closure_alignment`] of its raw layout), recomputed only
    /// when the alignment changed.
    fn frozen(&mut self, project: &Project) -> &BTreeSet<(LineId, u32)> {
        for c in 0..self.stale.len() {
            self.refresh(project, c);
        }
        let held: Vec<bool> = self.anchors.iter().map(|a| a.datum().is_some()).collect();
        let aligned: Vec<bool> = project
            .closures
            .iter()
            .map(|closure| is_aligned(&self.topology, &held, closure, |s| self.xy_of(s)))
            .collect();
        let current = self.frozen.take().filter(|(was, _)| *was == aligned);
        let (aligned, frozen) = current.unwrap_or_else(|| {
            let frozen = project
                .frozen_shots(&aligned)
                .into_iter()
                .map(|f| (f.line_id, f.shot_index))
                .collect();
            (aligned, frozen)
        });
        &self.frozen.insert((aligned, frozen)).1
    }
}

impl<'p> ShotEditRun<'p> {
    /// A run on `project`, which must be valid.
    pub fn new(project: &'p mut Project) -> Self {
        Self {
            project,
            layout: None,
            settled: false,
        }
    }

    /// The project as the updates so far left it.
    pub fn project(&self) -> &Project {
        self.project
    }

    /// Whether shot `index` of `line` is frozen in the current state: part
    /// of an adjusted survey network (S12; the command refuses its geometry
    /// edits). Without a datum nothing is frozen.
    pub fn is_frozen(&mut self, line: LineId, index: usize) -> bool {
        if self.project.datums.is_empty() {
            return false;
        }
        let project: &Project = self.project;
        let layout = self.layout.get_or_insert_with(|| RunLayout::build(project));
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        layout.frozen(project).contains(&(line, index))
    }

    /// [`Project::update_shot`] (module docs). On `Err` the caller rolls
    /// back.
    pub fn update_shot(
        &mut self,
        line: LineId,
        shot: ShotId,
        updates: &ShotUpdates,
    ) -> Result<(), EditError> {
        if !updates.all_finite() {
            return Err(EditError::refused(
                "Shot update has a non-finite (NaN/inf) value",
            ));
        }
        let index = self.project.shot_index(line, shot)?;
        if let Some(length) = updates.length {
            let label = self.project.shot_label_at(line, index);
            if length == 0.0 {
                return Err(EditError::refused(format!("Shot {label} has zero length")));
            }
            if length < 0.0 {
                return Err(EditError::refused(format!(
                    "Shot {label} has negative length: {length:.1}m"
                )));
            }
        }
        let position = self
            .project
            .line_position(line)
            .ok_or_else(EditError::line_gone)?;
        let apply = |p: &mut Project| {
            p.edit_shot_values(position, index, |s| apply_shot_updates(s, updates));
        };
        if !updates.touches_geometry() {
            apply(self.project);
            if updates.kind.is_some() {
                // The epilogue with no line to hold: only the demotion.
                self.settled = true;
                demote_fixes(self.project);
            }
            return Ok(());
        }
        self.settled = true;
        if self.layout.is_none() {
            self.layout = Some(RunLayout::build(self.project));
        }
        let Some(layout) = self.layout.as_mut() else {
            return Err(EditError::line_gone());
        };
        // Geometry edits keep the topology, so the run's indices hold.
        let Some(start) = self.project.line(line).map(|l| l.start) else {
            return Err(EditError::line_gone());
        };
        let anchor = layout
            .topology
            .component_of(start)
            .map(|c| (c, layout.anchors[c]));
        let Some((
            c,
            Anchor::Placement {
                placement, station, ..
            },
        )) = anchor
        else {
            // A datum-held piece stays on its datum: only the layout moves.
            apply(self.project);
            demote_fixes(self.project);
            layout.rewalk_downstream(self.project, position);
            return Ok(());
        };
        // A free piece: rule (a), the line's first station stays where it
        // was before the edit.
        layout.refresh(self.project, c);
        let Some(before) = layout.xy_of(start) else {
            return Err(EditError::line_gone());
        };
        let at = [before[0] + NO_SHIFT[0], before[1] + NO_SHIFT[1]];
        apply(self.project);
        demote_fixes(self.project);
        let topology = Arc::clone(&layout.topology);
        let mut walk = Layout::new(&topology, self.project.lines.len());
        self.project.walk_component(&topology, start, at, &mut walk);
        let laid = topology.dense(station).map_or([0.0, 0.0], |d| walk.xy[d]);
        self.project.placements[placement] = Placement {
            station,
            x: laid[0],
            y: laid[1],
        };
        layout.anchors[c] = Anchor::Placement {
            placement,
            station,
            at: laid,
        };
        layout.stale[c] = true;
        let violations = non_finite_placement(placement, laid);
        if violations.is_empty() {
            Ok(())
        } else {
            Err(EditError::Invalid(violations))
        }
    }

    /// End the run: validate once if any update ran the edit epilogue.
    pub fn finish(self) -> Result<(), EditError> {
        if self.settled {
            let violations = self.project.validate();
            if !violations.is_empty() {
                return Err(EditError::Invalid(violations));
            }
        }
        Ok(())
    }
}

/// The settle's translation when the frame does not change (plan 4.3):
/// added all the same, so a `-0.0` comes out exactly as the settle's does.
const NO_SHIFT: [f64; 2] = [0.0, 0.0];

/// The epilogue's demotion (`Project::settle_anchors`): a datum that is not
/// its component's placement datum carries no correction.
fn demote_fixes(project: &mut Project) {
    if project.datums.iter().all(|d| d.correction.is_none()) {
        return;
    }
    let mut is_placement = vec![false; project.datums.len()];
    for datum in project.placement_datums().into_iter().flatten() {
        is_placement[datum] = true;
    }
    let topology = project.topology_arc();
    for (index, datum) in project.datums.iter_mut().enumerate() {
        if !is_placement[index] && topology.contains(datum.station) {
            datum.correction = None;
        }
    }
}

/// What [`Project::validate`] reports for placement `placement` at `at`
/// when the run's state is otherwise valid: each axis that is not finite.
fn non_finite_placement(placement: usize, at: [f64; 2]) -> Vec<Violation> {
    at.into_iter()
        .enumerate()
        .filter(|(_, value)| !value.is_finite())
        .map(|(axis, value)| Violation::NonFinite {
            field: NumberField::Placement { placement, axis },
            value,
        })
        .collect()
}
