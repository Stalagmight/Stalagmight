//! The structural rules every delta obeys, checked before one is applied.
//!
//! A delta is read back from the `.stalx`, so it can be hostile. Each rule
//! below is one the encoder keeps by construction, and together they bound
//! what an apply can build: never more than the source holds plus the literals
//! the delta itself carries. Without them a row of a few kilobytes that keeps
//! the same lines or shots over and over grows the project it is applied
//! to, step after step, until the allocation aborts the app.

use serde_json::Value;

use super::{DeltaError, Edit, LineRun, ProjectDelta, Run, ShotRun, ID};
use crate::models::MAX_LINE_ID;

/// A half-open range `[start, end)` of source positions.
type Span = (usize, usize);

impl ProjectDelta {
    /// Refuse a delta that breaks a rule the encoder keeps:
    /// - the line runs read each source line at most once (a kept range
    ///   or an edited position), all within the source count the delta records;
    /// - the shot copies take each source shot at most once, and never out of a
    ///   line whose shots stay where they are (kept, or edited without its
    ///   shots rebuilt: `ShotPool` in `shots.rs`);
    /// - every positional array edit walks its source forwards, each element
    ///   at most once, within the length it records;
    /// - a new line's id fits the line-id bound.
    pub fn check_shape(&self) -> Result<(), DeltaError> {
        for edit in self.fields.values() {
            check_edit(edit)?;
        }
        let Some((count, runs)) = &self.lines else {
            return Ok(());
        };
        let count = *count;
        let mut read: Vec<Span> = Vec::new();
        let mut in_place: Vec<Span> = Vec::new();
        let mut copies: Vec<(usize, Span)> = Vec::new();
        for run in runs {
            match run {
                LineRun::Keep(at, len) => {
                    let span = span(*at, *len, count, "a line run keeps lines")?;
                    read.push(span);
                    in_place.push(span);
                }
                LineRun::Edit {
                    at, fields, shots, ..
                } => {
                    let span = span(*at, 1, count, "a line run edits a line")?;
                    read.push(span);
                    for edit in fields.values() {
                        check_edit(edit)?;
                    }
                    match shots {
                        None => in_place.push(span),
                        Some(shots) => collect_copies(shots, count, &mut copies)?,
                    }
                }
                LineRun::New { fields, shots } => {
                    if let Some(id) = fields.get(ID).and_then(Value::as_u64) {
                        if id > u64::from(MAX_LINE_ID) {
                            return Err(format!(
                                "a new line's id {id} is past the bound {MAX_LINE_ID}"
                            ));
                        }
                    }
                    if let Some(shots) = shots {
                        collect_copies(shots, count, &mut copies)?;
                    }
                }
            }
        }
        read.sort_unstable();
        if read.windows(2).any(|pair| pair[1].0 < pair[0].1) {
            return Err("a line run reads a source line twice".to_string());
        }
        in_place.sort_unstable();
        copies.sort_unstable();
        if copies
            .windows(2)
            .any(|pair| pair[0].0 == pair[1].0 && pair[1].1 .0 < pair[0].1 .1)
        {
            return Err("a shot run copies a source shot twice".to_string());
        }
        if copies.iter().any(|(sec, _)| covers(&in_place, *sec)) {
            return Err(
                "a shot run copies out of a line whose shots stay where they are".to_string(),
            );
        }
        Ok(())
    }
}

/// `[at, at + len)`, refused past `count`.
fn span(at: usize, len: usize, count: usize, what: &str) -> Result<Span, DeltaError> {
    at.checked_add(len)
        .filter(|end| *end <= count)
        .map(|end| (at, end))
        .ok_or_else(|| format!("{what} from {at} ({len}), past the source's {count}"))
}

/// Whether one of the disjoint, sorted `spans` holds `position`.
fn covers(spans: &[Span], position: usize) -> bool {
    let after = spans.partition_point(|(start, _)| *start <= position);
    after > 0 && position < spans[after - 1].1
}

fn collect_copies(
    shots: &[ShotRun],
    count: usize,
    copies: &mut Vec<(usize, Span)>,
) -> Result<(), DeltaError> {
    for run in shots {
        let ShotRun::Copy {
            sec, at, n, rev, m, ..
        } = run
        else {
            continue;
        };
        if *sec >= count {
            return Err(format!(
                "a shot run copies from line {sec}, past the source's {count}"
            ));
        }
        for (_, edit) in m {
            check_edit(edit)?;
        }
        if *n == 0 {
            continue;
        }
        let range = if *rev {
            at.checked_sub(n - 1).zip(at.checked_add(1))
        } else {
            at.checked_add(*n).map(|end| (*at, end))
        };
        let Some(range) = range else {
            return Err(format!(
                "a shot run copies {n} shots from {at} of line {sec}, past any line"
            ));
        };
        copies.push((*sec, range));
    }
    Ok(())
}

fn check_edit(edit: &Edit) -> Result<(), DeltaError> {
    match edit {
        Edit::Set(_) | Edit::Del | Edit::Bag { .. } => Ok(()),
        Edit::Obj(members) => members.values().try_for_each(check_edit),
        Edit::Arr(len, runs) => {
            let mut next = 0;
            for run in runs {
                match run {
                    Run::Keep(at, keep) => {
                        if *at < next {
                            return Err("an array run reads an element twice".to_string());
                        }
                        next = span(*at, *keep, *len, "an array run keeps elements")?.1;
                    }
                    Run::Mod(at, inner) => {
                        if *at < next {
                            return Err("an array run reads an element twice".to_string());
                        }
                        next = span(*at, 1, *len, "an array run edits an element")?.1;
                        check_edit(inner)?;
                    }
                    Run::Put(_) => {}
                }
            }
            Ok(())
        }
    }
}
