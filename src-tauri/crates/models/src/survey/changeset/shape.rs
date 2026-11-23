//! The structural rules every changeset keeps (`check_shape`, the #808
//! precedent): a persisted entry is untrusted input, so these run when an
//! entry is decoded and again before every step.
//!
//! Together they bound a step: it never builds more shots than the live
//! lines it reads hold plus the literal rows the entry carries (each source
//! shot is copied at most once per direction, every run fits the recorded
//! length of the side it reads), never expands a count the entry does not
//! materialise, and every id is inside its id space. Each rule is linear or
//! `n log n` in the entry's size.

use std::collections::{BTreeMap, HashMap};

use crate::{LineId, MAX_LINE_ID};

use super::super::{MAX_SHOTS_PER_LINE, MAX_SHOT_UUID_CHARS};
use super::lines::LineChange;
use super::lists::ListChange;
use super::shots::{Seg, ShotSide};
use super::stations::{FieldCol, StationChanges, StationRow};
use super::values::{Cols, ShotField, Span};
use super::Changeset;

/// Station and Section ids (and their counters) stay below this (2^31).
const ID_SPACE: u64 = 1 << 31;

/// The largest integer a JSON reader keeps exactly.
const MAX_INTEGER: u64 = (1 << 53) - 1;

pub(crate) fn check(cs: &Changeset) -> Result<(), String> {
    for (name, ok) in [
        (
            "datums",
            cs.datums.as_ref().is_none_or(ListChange::shape_ok),
        ),
        (
            "placements",
            cs.placements.as_ref().is_none_or(ListChange::shape_ok),
        ),
        (
            "closures",
            cs.closures.as_ref().is_none_or(ListChange::shape_ok),
        ),
        (
            "sections",
            cs.sections.as_ref().is_none_or(ListChange::shape_ok),
        ),
        (
            "imported file hashes",
            cs.file_hashes.as_ref().is_none_or(ListChange::shape_ok),
        ),
        (
            "imported line hashes",
            cs.line_hashes.as_ref().is_none_or(ListChange::shape_ok),
        ),
        (
            "line order",
            cs.order.as_ref().is_none_or(ListChange::shape_ok),
        ),
    ] {
        if !ok {
            return Err(format!("the {name} change is malformed"));
        }
    }
    if let Some(counters) = &cs.counters {
        for side in [counters.0, counters.1] {
            if side[0] > ID_SPACE || side[2] > ID_SPACE || side[1] > MAX_INTEGER {
                return Err("a counter is out of its id space".to_string());
            }
        }
    }
    stations(&cs.stations)?;
    lines(cs)?;
    Ok(())
}

fn ascending(ids: impl Iterator<Item = u64>) -> bool {
    let mut last: Option<u64> = None;
    for id in ids {
        if last.is_some_and(|l| id <= l) {
            return false;
        }
        last = Some(id);
    }
    true
}

fn stations(s: &StationChanges) -> Result<(), String> {
    for (what, rows) in [("created", &s.add), ("deleted", &s.del)] {
        let ids = rows.iter().map(|StationRow(st)| u64::from(st.id));
        if !ascending(ids) {
            return Err(format!("the {what} stations are not in ascending id order"));
        }
        if rows
            .iter()
            .any(|StationRow(st)| u64::from(st.id) >= ID_SPACE)
        {
            return Err(format!("a {what} station id is out of its id space"));
        }
    }
    fn col<T>(name: &str, col: Option<&FieldCol<T>>) -> Result<(), String> {
        let Some(col) = col else { return Ok(()) };
        let runs_ok = col
            .0
             .0
            .iter()
            .all(|&(start, len)| len >= 1 && u64::from(start) + u64::from(len) <= ID_SPACE);
        let ascending = col
            .0
             .0
            .windows(2)
            .all(|w| u64::from(w[0].0) + u64::from(w[0].1) <= u64::from(w[1].0));
        if !runs_ok || !ascending || !col.shape_ok() {
            return Err(format!("the station {name} column is malformed"));
        }
        Ok(())
    }
    col("name", s.name.as_ref())?;
    col("comment", s.comment.as_ref())?;
    col("archived", s.archived.as_ref())?;
    col("external", s.external.as_ref())?;
    Ok(())
}

/// Which state a side is built from: the after side reads the before state.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Reads {
    Before,
    After,
}

fn lines(cs: &Changeset) -> Result<(), String> {
    if !ascending(cs.lines.iter().map(|l| u64::from(l.id))) {
        return Err("the lines are not in ascending id order".to_string());
    }
    // Recorded lengths per (state, line), for the runs that read them.
    let mut lengths: HashMap<(Reads, LineId), u32> = HashMap::new();
    let mut created: Vec<LineId> = Vec::new();
    let mut removed: Vec<LineId> = Vec::new();
    for change in &cs.lines {
        line_rules(change, cs.credits.len())?;
        if let Some(side) = &change.before {
            lengths.insert((Reads::Before, change.id), side.n);
        }
        if let Some(side) = &change.after {
            lengths.insert((Reads::After, change.id), side.n);
        }
        if change.after.as_ref().is_some_and(|s| s.line.is_some()) {
            created.push(change.id);
        }
        if change.before.as_ref().is_some_and(|s| s.line.is_some()) {
            removed.push(change.id);
        }
    }
    // Every run fits the side it reads, and takes each source shot once.
    let mut taken: BTreeMap<(Reads, LineId), Vec<(u32, u32)>> = BTreeMap::new();
    for change in &cs.lines {
        for (side, reads) in [
            (&change.after, Reads::Before),
            (&change.before, Reads::After),
        ] {
            let Some(side) = side else { continue };
            for seg in &side.s {
                let Seg::Copy(run) = seg else { continue };
                let Some(&len) = lengths.get(&(reads, run.line)) else {
                    return Err(format!(
                        "line {}: a run reads line {}, which the entry does not record",
                        change.id, run.line
                    ));
                };
                let (first, end) = if run.mode.reversed() {
                    (run.at.checked_sub(run.n - 1), run.at.checked_add(1))
                } else {
                    (Some(run.at), run.at.checked_add(run.n))
                };
                match (first, end) {
                    (Some(first), Some(end)) if end <= len => {
                        taken
                            .entry((reads, run.line))
                            .or_default()
                            .push((first, end));
                    }
                    _ => {
                        return Err(format!(
                            "line {}: a run reads past the end of line {}",
                            change.id, run.line
                        ))
                    }
                }
            }
        }
    }
    for ((_, line), mut ranges) in taken {
        ranges.sort_unstable();
        if ranges.windows(2).any(|w| w[1].0 < w[0].1) {
            return Err(format!("line {line}: two runs copy the same shot"));
        }
    }
    order_rules(cs, &mut created, &mut removed)
}

fn line_rules(change: &LineChange, credits: usize) -> Result<(), String> {
    let id = change.id;
    if id > MAX_LINE_ID {
        return Err(format!("line id {id} is out of its id space"));
    }
    let images = [&change.before, &change.after]
        .iter()
        .filter(|s| s.as_ref().is_some_and(|s| s.line.is_some()))
        .count();
    let sides = [&change.before, &change.after]
        .iter()
        .filter(|s| s.is_some())
        .count();
    let consistent = match (images, sides) {
        // Created or deleted: one side, with its image, no field edits.
        (1, 1) => change.meta.is_empty(),
        // Shots changed on a line on both sides.
        (0, 2) => true,
        // Metadata only.
        (0, 0) => !change.meta.is_empty(),
        _ => false,
    };
    if !consistent {
        return Err(format!("line {id}: its sides are inconsistent"));
    }
    for side in [&change.before, &change.after].into_iter().flatten() {
        side_rules(id, side, credits)?;
    }
    Ok(())
}

fn side_rules(id: LineId, side: &ShotSide, credits: usize) -> Result<(), String> {
    if side.n as usize > MAX_SHOTS_PER_LINE {
        return Err(format!("line {id}: more than {MAX_SHOTS_PER_LINE} shots"));
    }
    let mut total: u64 = 0;
    for seg in &side.s {
        match seg {
            Seg::Copy(run) => {
                if run.n == 0 || run.line > MAX_LINE_ID {
                    return Err(format!("line {id}: an empty or out-of-range run"));
                }
                if run
                    .anchor
                    .as_ref()
                    .is_some_and(|a| a.is_empty() || a.chars().count() > MAX_SHOT_UUID_CHARS)
                {
                    return Err(format!("line {id}: a malformed run anchor"));
                }
                cols_rules(id, &run.edits, run.n, credits)?;
            }
            Seg::Rows(rows) => {
                if rows.uuids.is_empty()
                    || rows
                        .uuids
                        .iter()
                        .any(|u| u.is_empty() || u.chars().count() > MAX_SHOT_UUID_CHARS)
                {
                    return Err(format!("line {id}: a malformed shot uuid"));
                }
                cols_rules(id, &rows.cols, rows.uuids.len() as u32, credits)?;
            }
        }
        total += u64::from(seg.len());
    }
    if total != u64::from(side.n) {
        return Err(format!(
            "line {id}: its runs hold {total} shots, {} recorded",
            side.n
        ));
    }
    Ok(())
}

fn cols_rules(id: LineId, cols: &Cols, len: u32, credits: usize) -> Result<(), String> {
    for (field, spans) in &cols.0 {
        let mut end = 0u64;
        for span in spans {
            let start = u64::from(span.start());
            let n = u64::from(span.len());
            if n == 0 || start < end || start + n > u64::from(len) {
                return Err(format!(
                    "line {id}: a {} column is out of order or out of range",
                    field.code()
                ));
            }
            end = start + n;
            let values_ok = match span {
                Span::Values(_, values) => values.iter().all(|v| value_ok(*field, v, credits)),
                Span::Repeat(_, _, v) => value_ok(*field, v, credits),
            };
            if !values_ok {
                return Err(format!(
                    "line {id}: a {} value is not one the field takes",
                    field.code()
                ));
            }
        }
    }
    Ok(())
}

fn value_ok(field: ShotField, value: &super::Val, credits: usize) -> bool {
    field.accepts(value)
        && match (field, value) {
            (ShotField::Credit, super::Val::Int(i)) => (*i as usize) < credits,
            (ShotField::To, super::Val::Int(v)) => *v < ID_SPACE,
            _ => true,
        }
}

/// The line order change and the created and removed lines agree: what the
/// order gains is exactly the created lines, what it loses the removed ones.
fn order_rules(
    cs: &Changeset,
    created: &mut Vec<LineId>,
    removed: &mut Vec<LineId>,
) -> Result<(), String> {
    let Some(order) = &cs.order else {
        return if created.is_empty() && removed.is_empty() {
            Ok(())
        } else {
            Err("lines are created or removed without a line order change".to_string())
        };
    };
    if order
        .side(false)
        .chain(order.side(true))
        .any(|&id| id > MAX_LINE_ID)
    {
        return Err("a line id in the order is out of its id space".to_string());
    }
    let mut counts: HashMap<LineId, i64> = HashMap::new();
    for &id in order.side(true) {
        *counts.entry(id).or_default() += 1;
    }
    for &id in order.side(false) {
        *counts.entry(id).or_default() -= 1;
    }
    let mut gained: Vec<LineId> = Vec::new();
    let mut lost: Vec<LineId> = Vec::new();
    for (id, count) in counts {
        match count {
            1 => gained.push(id),
            -1 => lost.push(id),
            0 => {}
            _ => return Err(format!("line {id} appears twice in the order change")),
        }
    }
    gained.sort_unstable();
    lost.sort_unstable();
    created.sort_unstable();
    removed.sort_unstable();
    if gained != *created || lost != *removed {
        return Err(
            "the line order change does not match the created and removed lines".to_string(),
        );
    }
    Ok(())
}

/// Every number of a side finite.
pub(crate) fn side_finite(side: &ShotSide) -> bool {
    side.line.as_ref().is_none_or(|image| image.finite())
        && side.s.iter().all(|seg| {
            let cols = match seg {
                Seg::Copy(run) => &run.edits,
                Seg::Rows(rows) => &rows.cols,
            };
            cols.cells().all(|(_, _, v)| v.finite())
        })
}
