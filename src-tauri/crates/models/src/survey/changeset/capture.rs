//! Generic capture: the changes from one model state to another.
//!
//! Cost on a large survey is a bitwise compare of every line (no allocation)
//! and a merge walk over the two station maps; only the lines that differ
//! are diffed shot by shot, and only their shots are indexed by uuid.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::LineId;

use super::super::{Line, Project};
use super::bits::{same_line_meta, same_shots, Same};
use super::lines::{LineChange, LineMetaEdit};
use super::lists::ListChange;
use super::shots::{capture_side, SourceState};
use super::stations::StationChanges;
use super::values::Credits;
use super::{Changeset, CounterChange, ProjectChanges};

pub(super) fn capture(before: &Project, after: &Project) -> Changeset {
    let mut out = Changeset {
        project: project_changes(before, after),
        stations: StationChanges::between(&before.stations, &after.stations),
        ..Changeset::default()
    };
    let order = |p: &Project| -> Vec<LineId> { p.lines().iter().map(|l| l.id).collect() };
    out.order = ListChange::between(&order(before), &order(after));
    let (lines, credits) = line_changes(before, after);
    out.lines = lines;
    out.credits = credits.table;
    out.datums = ListChange::between(&before.datums, &after.datums);
    out.placements = ListChange::between(&before.placements, &after.placements);
    out.closures = ListChange::between(&before.closures, &after.closures);
    out.sections = ListChange::between(&before.sections, &after.sections);
    out.file_hashes =
        ListChange::between(&before.imported_file_hashes, &after.imported_file_hashes);
    out.line_hashes =
        ListChange::between(&before.imported_line_hashes, &after.imported_line_hashes);
    if before.counters != after.counters {
        out.counters = Some(CounterChange(
            CounterChange::of(&before.counters),
            CounterChange::of(&after.counters),
        ));
    }
    out
}

fn project_changes(before: &Project, after: &Project) -> ProjectChanges {
    ProjectChanges {
        name: (before.name != after.name).then(|| (before.name.clone(), after.name.clone())),
        location: (before.location != after.location)
            .then(|| (before.location.clone(), after.location.clone())),
        created: (before.created_at != after.created_at)
            .then_some((before.created_at, after.created_at)),
        settings: (!before.settings.same(&after.settings))
            .then(|| Box::new((before.settings.clone(), after.settings.clone()))),
    }
}

/// Which lines changed, and how.
fn line_changes(before: &Project, after: &Project) -> (Vec<LineChange>, Credits) {
    let (b, a) = (lines_by_id(before), lines_by_id(after));
    let ids: BTreeSet<LineId> = b.keys().chain(a.keys()).copied().collect();
    // Per id: (meta edits, whether the shot lists are sides of the change).
    let mut touched: BTreeMap<LineId, (LineMetaEdit, bool)> = BTreeMap::new();
    for id in ids {
        match (b.get(&id), a.get(&id)) {
            (Some(lb), Some(la)) => {
                let meta_same = same_line_meta(lb, la);
                let shots_same = same_shots(&lb.shots, &la.shots);
                if meta_same && shots_same {
                    continue;
                }
                let meta = if meta_same {
                    LineMetaEdit::default()
                } else {
                    LineMetaEdit::between(lb, la)
                };
                touched.insert(id, (meta, !shots_same));
            }
            _ => {
                touched.insert(id, (LineMetaEdit::default(), true));
            }
        }
    }
    let (before_lines, after_lines) = (with_sides(&touched, &b), with_sides(&touched, &a));
    // The after sides read the before state, the before sides the after one.
    let mut from_before = SourceState::new(before_lines);
    let mut from_after = SourceState::new(after_lines);
    let mut credits = Credits::default();
    let mut out = Vec::with_capacity(touched.len());
    for (id, (meta, sides)) in touched {
        let (lb, la) = (b.get(&id).copied(), a.get(&id).copied());
        let only_one = lb.is_none() || la.is_none();
        let mut change = LineChange {
            id,
            meta,
            before: None,
            after: None,
        };
        if sides {
            change.before =
                lb.map(|line| capture_side(line, &mut from_after, &mut credits, only_one));
            change.after =
                la.map(|line| capture_side(line, &mut from_before, &mut credits, only_one));
        }
        out.push(change);
    }
    (out, credits)
}

fn lines_by_id(p: &Project) -> HashMap<LineId, &Line> {
    p.lines().iter().map(|l| (l.id, l)).collect()
}

/// The lines of one state whose shot lists are sides of the change.
fn with_sides<'a>(
    touched: &BTreeMap<LineId, (LineMetaEdit, bool)>,
    lines: &HashMap<LineId, &'a Line>,
) -> Vec<&'a Line> {
    touched
        .iter()
        .filter(|(_, (_, sides))| *sides)
        .filter_map(|(id, _)| lines.get(id).copied())
        .collect()
}
