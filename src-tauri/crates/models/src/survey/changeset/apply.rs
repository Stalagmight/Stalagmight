//! Applying a changeset either way.
//!
//! Two phases. `prepare` reads the live project only: it checks every
//! precondition (the live project is the recorded source state on
//! everything the changeset touches), builds the target lines and lists,
//! and checks the built lines against their recorded hashes. `commit` then
//! moves the prepared parts in and cannot fail. A refusal therefore leaves
//! the project untouched, without cloning it.

use std::collections::{HashMap, HashSet};

use crate::{LineId, Section, ShotId, FEATURE_ID_STRIDE};

use super::super::{Counters, Line, LoopClosure, Placement, Project, ZeroDatum};
use super::bits::line_hash;
use super::shape;
use super::shots::{build_shots, SessionIds};
use super::{Changeset, Direction, Refusal};

pub(super) fn apply(
    project: &mut Project,
    changes: &Changeset,
    direction: Direction,
) -> Result<(), Refusal> {
    shape::check(changes).map_err(Refusal::Shape)?;
    let plan = prepare(project, changes, direction)?;
    commit(project, changes, direction, plan);
    Ok(())
}

/// A rebuilt line and the session ids its shots keep.
struct BuiltLine {
    line: Line,
    kept: Vec<Option<ShotId>>,
    hint: Option<SessionIds>,
    /// The live line's allocator, when the line exists live.
    live_next: Option<ShotId>,
}

#[derive(Default)]
struct Plan {
    order: Option<Vec<LineId>>,
    built: HashMap<LineId, BuiltLine>,
    removed: HashSet<LineId>,
    datums: Option<Vec<ZeroDatum>>,
    placements: Option<Vec<Placement>>,
    closures: Option<Vec<LoopClosure>>,
    sections: Option<Vec<Section>>,
    file_hashes: Option<Vec<String>>,
    line_hashes: Option<Vec<String>>,
    counters: Option<Counters>,
}

fn from<T>(pair: &(T, T), direction: Direction) -> &T {
    match direction {
        Direction::Undo => &pair.1,
        Direction::Redo => &pair.0,
    }
}

fn to<T>(pair: &(T, T), direction: Direction) -> &T {
    match direction {
        Direction::Undo => &pair.0,
        Direction::Redo => &pair.1,
    }
}

fn prepare(project: &Project, changes: &Changeset, direction: Direction) -> Result<Plan, Refusal> {
    use super::bits::Same;
    let p = &changes.project;
    if p.name
        .as_ref()
        .is_some_and(|n| *from(n, direction) != project.name)
    {
        return Err(Refusal::ProjectField("name"));
    }
    if p.location
        .as_ref()
        .is_some_and(|l| *from(l, direction) != project.location)
    {
        return Err(Refusal::ProjectField("location"));
    }
    if p.created
        .as_ref()
        .is_some_and(|c| *from(c, direction) != project.created_at)
    {
        return Err(Refusal::ProjectField("creation time"));
    }
    if p.settings
        .as_ref()
        .is_some_and(|s| !from(s, direction).same(&project.settings))
    {
        return Err(Refusal::ProjectField("settings"));
    }
    changes.stations.check(&project.stations, direction)?;

    let mut plan = Plan::default();
    macro_rules! list {
        ($field:ident, $live:expr, $name:literal) => {
            if let Some(change) = &changes.$field {
                plan.$field = Some(change.step($live, direction).ok_or(Refusal::List($name))?);
            }
        };
    }
    list!(datums, &project.datums, "datums");
    list!(placements, &project.placements, "placements");
    list!(closures, &project.closures, "loop closures");
    list!(sections, &project.sections, "Sections");
    list!(
        file_hashes,
        &project.imported_file_hashes,
        "imported file hashes"
    );
    list!(
        line_hashes,
        &project.imported_line_hashes,
        "imported line hashes"
    );

    let live: HashMap<LineId, &Line> = project.lines().iter().map(|l| (l.id, l)).collect();
    if let Some(order) = &changes.order {
        let ids: Vec<LineId> = project.lines().iter().map(|l| l.id).collect();
        plan.order = Some(order.step(&ids, direction).ok_or(Refusal::LineOrder)?);
    }
    let (source_after, target_after) = match direction {
        Direction::Undo => (true, false),
        Direction::Redo => (false, true),
    };
    let mut created: HashSet<LineId> = HashSet::new();
    for change in &changes.lines {
        let id = change.id;
        let existing = live.get(&id).copied();
        if change.exists(source_after) != existing.is_some() {
            return Err(Refusal::Line(id));
        }
        if let (Some(side), Some(line)) = (change.source(direction), existing) {
            if side.n as usize != line.shots.len() || side.h != line_hash(line) {
                return Err(Refusal::Line(id));
            }
        }
        if !change.meta.is_empty() {
            match existing {
                Some(line) if change.meta.matches(line, direction) => {}
                _ => return Err(Refusal::Line(id)),
            }
        }
        if let Some(side) = change.target(direction) {
            let built = build_shots(side, id, &live, &changes.credits)?;
            let mut line = match (&side.line, existing) {
                (Some(image), _) => image.to_line(id),
                (None, Some(live_line)) => meta_only(live_line),
                (None, None) => return Err(Refusal::Line(id)),
            };
            line.shots = built.shots;
            change.meta.apply(&mut line, direction);
            if line_hash(&line) != side.h {
                return Err(Refusal::Line(id));
            }
            if existing.is_none() {
                created.insert(id);
            }
            plan.built.insert(
                id,
                BuiltLine {
                    line,
                    kept: built.kept,
                    hint: side.hint.as_deref().cloned(),
                    live_next: existing.map(|l| l.next_shot_id),
                },
            );
        } else if !change.exists(target_after) {
            plan.removed.insert(id);
        }
    }
    // The target line set: the live lines, less the removed, plus the
    // created, in the target order.
    match &plan.order {
        Some(order) => {
            let mut seen = HashSet::with_capacity(order.len());
            for id in order {
                let present =
                    (live.contains_key(id) && !plan.removed.contains(id)) || created.contains(id);
                if !present || !seen.insert(*id) {
                    return Err(Refusal::LineOrder);
                }
            }
            if seen.len() != live.len() - plan.removed.len() + created.len() {
                return Err(Refusal::LineOrder);
            }
        }
        None if !created.is_empty() || !plan.removed.is_empty() => {
            return Err(Refusal::Shape(
                "lines are created or removed without a line order change".to_string(),
            ));
        }
        None => {}
    }
    if let Some(counters) = &changes.counters {
        plan.counters = Some(counters.stepped(&project.counters, direction));
    }
    Ok(plan)
}

/// A copy of `line` without its shots (the metadata a rebuilt line starts
/// from).
fn meta_only(line: &Line) -> Line {
    Line {
        id: line.id,
        name: line.name.clone(),
        color: line.color.clone(),
        direction: line.direction,
        quality: line.quality,
        azimuth_reference: line.azimuth_reference,
        section_id: line.section_id,
        legacy_surveyor: line.legacy_surveyor.clone(),
        legacy_date: line.legacy_date,
        start: line.start,
        shots: Vec::new(),
        hand_moves: line.hand_moves.clone(),
        next_shot_id: line.next_shot_id,
    }
}

fn commit(project: &mut Project, changes: &Changeset, direction: Direction, plan: Plan) {
    let p = &changes.project;
    if let Some(name) = &p.name {
        project.name.clone_from(to(name, direction));
    }
    if let Some(location) = &p.location {
        project.location.clone_from(to(location, direction));
    }
    if let Some(created) = &p.created {
        project.created_at = *to(created, direction);
    }
    if let Some(settings) = &p.settings {
        project.settings = to(settings, direction).clone();
    }
    changes.stations.apply(&mut project.stations, direction);
    if let Some(v) = plan.datums {
        project.datums = v;
    }
    if let Some(v) = plan.placements {
        project.placements = v;
    }
    if let Some(v) = plan.closures {
        project.closures = v;
    }
    if let Some(v) = plan.sections {
        project.sections = v;
    }
    if let Some(v) = plan.file_hashes {
        project.imported_file_hashes = v;
    }
    if let Some(v) = plan.line_hashes {
        project.imported_line_hashes = v;
    }
    if let Some(counters) = plan.counters {
        project.counters = counters;
    }
    let touched_lines = !plan.built.is_empty()
        || !plan.removed.is_empty()
        || plan.order.is_some()
        || changes.lines.iter().any(|l| !l.meta.is_empty());
    if !touched_lines {
        return;
    }
    let lines = std::mem::take(project.lines_mut());
    let live_order: Vec<LineId> = lines.iter().map(|l| l.id).collect();
    let mut by_id: HashMap<LineId, Line> = lines.into_iter().map(|l| (l.id, l)).collect();
    for id in &plan.removed {
        by_id.remove(id);
    }
    for change in &changes.lines {
        if change.target(direction).is_none() && !change.meta.is_empty() {
            if let Some(line) = by_id.get_mut(&change.id) {
                change.meta.apply(line, direction);
            }
        }
    }
    for (id, built) in plan.built {
        by_id.insert(id, with_session_ids(built));
    }
    let order = plan.order.unwrap_or(live_order);
    let rebuilt: Vec<Line> = order.iter().filter_map(|id| by_id.remove(id)).collect();
    *project.lines_mut() = rebuilt;
}

/// Give a rebuilt line's shots their session ids: the recorded ones when the
/// changeset carries them for this side (a step in the session it was
/// captured in), else each copied shot keeps its live id and the rest are
/// minted from the line's allocator. The allocator never goes down.
fn with_session_ids(built: BuiltLine) -> Line {
    let BuiltLine {
        mut line,
        kept,
        hint,
        live_next,
    } = built;
    let base = line.id.saturating_mul(FEATURE_ID_STRIDE);
    let floor = live_next.unwrap_or(base).max(base);
    if let Some(hint) = hint.filter(|h| h.ids.len() == line.shots.len()) {
        for (shot, id) in line.shots.iter_mut().zip(hint.ids) {
            shot.id = id;
        }
        line.next_shot_id = hint.next.max(floor);
    } else {
        let mut next = floor;
        for (shot, keep) in line.shots.iter_mut().zip(kept) {
            shot.id = keep.unwrap_or_else(|| {
                let id = next;
                next = next.saturating_add(1);
                id
            });
        }
        line.next_shot_id = next;
    }
    let window_end = base.saturating_add(FEATURE_ID_STRIDE);
    let ids_fit = line.shots.iter().all(|s| s.id >= base && s.id < window_end)
        && line.next_shot_id < window_end;
    if !ids_fit {
        line.assign_session_shot_ids();
    }
    line
}
