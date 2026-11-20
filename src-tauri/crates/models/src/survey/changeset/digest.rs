//! What a history preview shows, read straight off a changeset (plan 8.2):
//! the per-field old -> new values, the entities created and deleted (a
//! deleted entity's name from its before-image), the lines reversed. No
//! `Command` payload is read, and no project: an entry deep in the stack is
//! described by its own images.

use std::sync::Arc;

use crate::{LineId, Section, StationId, SurveyCredit};

use super::super::{LoopClosure, Placement, Shot, Station, ZeroDatum};
use super::lines::{LineChange, LineImage, LineMetaEdit};
use super::lists::ListChange;
use super::shots::{Seg, ShotSide};
use super::stations::FieldCol;
use super::values::{blank_shot, ShotField, Val};
use super::Changeset;

/// The most created or deleted shots a digest lists (it counts them all).
pub const MAX_LISTED_SHOTS: usize = 16;

/// A project field a step changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectField {
    Name,
    Location,
    CreatedAt,
    SurfaceElevation,
    DefaultAzimuthReference,
    Validation,
}

#[derive(Debug, Clone)]
pub struct ProjectFieldChange {
    pub field: ProjectField,
    pub old: Val,
    pub new: Val,
}

/// A station field a step changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationField {
    Name,
    Comment,
    /// Values are the alias count.
    Aliases,
    External,
}

#[derive(Debug, Clone)]
pub struct StationFieldChange {
    pub station: StationId,
    pub field: StationField,
    pub old: Val,
    pub new: Val,
}

/// A line metadata field a step changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineField {
    Name,
    Color,
    /// `in` / `out`.
    Direction,
    Quality,
    /// Debug text of the reference, or null.
    AzimuthReference,
    Section,
    Surveyor,
    /// ISO date, or null.
    Date,
    Start,
    /// Values are the hand-move count.
    HandMoves,
}

#[derive(Debug, Clone)]
pub struct LineFieldChange {
    pub line: LineId,
    pub field: LineField,
    pub old: Val,
    pub new: Val,
}

/// A line created or deleted by the step, from its image.
#[derive(Debug, Clone)]
pub struct LineSummary {
    pub id: LineId,
    pub name: Option<String>,
    pub shots: u32,
    pub start: StationId,
}

/// One shot cell a step changed: line and index in the AFTER state, the
/// value before (when the record pairs it: a run read forwards) and after.
#[derive(Debug, Clone)]
pub struct ShotCellChange {
    pub line: LineId,
    pub index: u32,
    pub field: ShotField,
    pub old: Option<Val>,
    pub new: Val,
}

/// A shot only one side holds (created, or deleted), at its index there.
#[derive(Debug, Clone)]
pub struct AddedShot {
    pub line: LineId,
    pub index: u32,
    pub shot: Shot,
}

/// A list's elements only one side's hunks hold.
#[derive(Debug, Clone)]
pub struct ListDiff<T> {
    pub removed: Vec<T>,
    pub added: Vec<T>,
}

impl<T> Default for ListDiff<T> {
    fn default() -> Self {
        Self {
            removed: Vec::new(),
            added: Vec::new(),
        }
    }
}

/// See the module docs.
#[derive(Debug, Clone, Default)]
pub struct Digest {
    pub project: Vec<ProjectFieldChange>,
    pub stations_created: Vec<Station>,
    pub stations_deleted: Vec<Station>,
    pub station_fields: Vec<StationFieldChange>,
    pub lines_created: Vec<LineSummary>,
    pub lines_deleted: Vec<LineSummary>,
    pub line_fields: Vec<LineFieldChange>,
    /// Lines reversed end to end, with their shot count.
    pub reversed_lines: Vec<(LineId, u32)>,
    pub shot_cells: Vec<ShotCellChange>,
    /// Shots only the after state holds (the first [`MAX_LISTED_SHOTS`]).
    pub shots_created: Vec<AddedShot>,
    pub shots_created_total: usize,
    /// Shots only the before state holds (the first [`MAX_LISTED_SHOTS`]).
    pub shots_deleted: Vec<AddedShot>,
    pub shots_deleted_total: usize,
    pub datums: ListDiff<ZeroDatum>,
    pub placements: ListDiff<Placement>,
    pub closures: ListDiff<LoopClosure>,
    pub sections: ListDiff<Section>,
    pub file_hashes: ListDiff<String>,
    pub line_hashes: ListDiff<String>,
}

fn text(s: Option<&String>) -> Val {
    s.cloned().map_or(Val::Null, Val::Str)
}

impl Changeset {
    /// The credit a [`ShotField::Credit`] cell names (its `Val::Int` index).
    pub fn credit(&self, index: &Val) -> Option<&SurveyCredit> {
        match index {
            Val::Int(i) => self.credits.get(usize::try_from(*i).ok()?).map(|c| &**c),
            _ => None,
        }
    }

    /// What a preview shows of this step (module docs).
    pub fn digest(&self) -> Digest {
        let mut d = Digest::default();
        self.digest_project(&mut d);
        self.digest_stations(&mut d);
        for change in &self.lines {
            self.digest_line(change, &mut d);
        }
        d.datums = list(self.datums.as_ref());
        d.placements = list(self.placements.as_ref());
        d.closures = list(self.closures.as_ref());
        d.sections = list(self.sections.as_ref());
        d.file_hashes = list(self.file_hashes.as_ref());
        d.line_hashes = list(self.line_hashes.as_ref());
        d
    }

    fn digest_project(&self, d: &mut Digest) {
        let p = &self.project;
        let mut push = |field, old, new| d.project.push(ProjectFieldChange { field, old, new });
        if let Some((a, b)) = &p.name {
            push(ProjectField::Name, Val::Str(a.clone()), Val::Str(b.clone()));
        }
        if let Some((a, b)) = &p.location {
            push(ProjectField::Location, text(a.as_ref()), text(b.as_ref()));
        }
        if let Some((a, b)) = &p.created {
            push(
                ProjectField::CreatedAt,
                Val::Str(a.to_rfc3339()),
                Val::Str(b.to_rfc3339()),
            );
        }
        if let Some(pair) = &p.settings {
            let (a, b) = &**pair;
            let num = |v: Option<f64>| v.map_or(Val::Null, Val::Num);
            if a.surface_elevation.map(f64::to_bits) != b.surface_elevation.map(f64::to_bits) {
                push(
                    ProjectField::SurfaceElevation,
                    num(a.surface_elevation),
                    num(b.surface_elevation),
                );
            }
            let az = |r: &Option<crate::AzimuthReference>| {
                r.map_or(Val::Null, |r| Val::Str(format!("{r:?}")))
            };
            if a.default_azimuth_reference != b.default_azimuth_reference {
                push(
                    ProjectField::DefaultAzimuthReference,
                    az(&a.default_azimuth_reference),
                    az(&b.default_azimuth_reference),
                );
            }
            let json = |v: &crate::config::ValidationConfig| {
                Val::Str(serde_json::to_string(v).unwrap_or_default())
            };
            let (va, vb) = (json(&a.validation), json(&b.validation));
            if va.as_str() != vb.as_str() {
                push(ProjectField::Validation, va, vb);
            }
        }
    }

    fn digest_stations(&self, d: &mut Digest) {
        let s = &self.stations;
        d.stations_created = s.add.iter().map(|r| r.0.clone()).collect();
        d.stations_deleted = s.del.iter().map(|r| r.0.clone()).collect();
        let mut push = |station, field, old, new| {
            d.station_fields.push(StationFieldChange {
                station,
                field,
                old,
                new,
            });
        };
        for (id, a, b) in s.name.iter().flat_map(FieldCol::rows) {
            push(
                id,
                StationField::Name,
                Val::Str(a.clone()),
                Val::Str(b.clone()),
            );
        }
        for (id, a, b) in s.comment.iter().flat_map(FieldCol::rows) {
            push(
                id,
                StationField::Comment,
                text(a.as_ref()),
                text(b.as_ref()),
            );
        }
        for (id, a, b) in s.aliases.iter().flat_map(FieldCol::rows) {
            push(
                id,
                StationField::Aliases,
                Val::Int(a.len() as u64),
                Val::Int(b.len() as u64),
            );
        }
        for (id, a, b) in s.external.iter().flat_map(FieldCol::rows) {
            push(
                id,
                StationField::External,
                text(a.as_ref()),
                text(b.as_ref()),
            );
        }
        d.station_fields.sort_by_key(|c| c.station);
    }

    fn digest_line(&self, change: &LineChange, d: &mut Digest) {
        let summary = |side: &ShotSide, image: &LineImage| LineSummary {
            id: change.id,
            name: image.name.clone(),
            shots: side.n,
            start: image.start,
        };
        match (&change.before, &change.after) {
            (Some(side), None) => {
                if let Some(image) = &side.line {
                    d.lines_deleted.push(summary(side, image));
                }
            }
            (None, Some(side)) => {
                if let Some(image) = &side.line {
                    d.lines_created.push(summary(side, image));
                }
            }
            _ => {}
        }
        meta_fields(change.id, &change.meta, &mut d.line_fields);
        if let Some(after) = &change.after {
            if let [Seg::Copy(run)] = after.s.as_slice() {
                if run.line == change.id && run.mode.reversed() && run.n == after.n {
                    d.reversed_lines.push((change.id, after.n));
                }
            }
            self.cells(change.id, after, &mut d.shot_cells);
            rows(
                change.id,
                after,
                &self.credits,
                &mut d.shots_created,
                &mut d.shots_created_total,
            );
        }
        if let Some(before) = &change.before {
            rows(
                change.id,
                before,
                &self.credits,
                &mut d.shots_deleted,
                &mut d.shots_deleted_total,
            );
        }
    }

    /// The edited cells of `side` (an after side of `line`), each paired with
    /// its before value where a forward run records both.
    fn cells(&self, line: LineId, side: &ShotSide, out: &mut Vec<ShotCellChange>) {
        let mut offset = 0u32;
        for seg in &side.s {
            if let Seg::Copy(run) = seg {
                let back = self
                    .lines
                    .binary_search_by_key(&run.line, |l| l.id)
                    .ok()
                    .and_then(|i| self.lines[i].before.as_ref());
                for (field, t, new) in run.edits.cells() {
                    let old = (!run.mode.reversed())
                        .then(|| {
                            let source = run.at + t;
                            back.and_then(|b| value_at(b, line, offset + t, source, field))
                        })
                        .flatten();
                    out.push(ShotCellChange {
                        line,
                        index: offset + t,
                        field,
                        old,
                        new: new.clone(),
                    });
                }
            }
            offset += seg.len();
        }
        out.sort_by_key(|c| (c.line, c.index, c.field));
    }
}

/// The value `field` takes at index `at` of `side` (a before side), when a
/// forward run there reads `line` at `from` and records the field.
fn value_at(side: &ShotSide, line: LineId, from: u32, at: u32, field: ShotField) -> Option<Val> {
    let mut offset = 0u32;
    for seg in &side.s {
        let len = seg.len();
        if at < offset + len {
            return match seg {
                Seg::Copy(run) if run.line == line && !run.mode.reversed() => {
                    let t = at - offset;
                    (run.at + t == from)
                        .then(|| run.edits.value(field, t).cloned())
                        .flatten()
                }
                _ => None,
            };
        }
        offset += len;
    }
    None
}

/// The literal rows of `side`, built (the first [`MAX_LISTED_SHOTS`]).
fn rows(
    line: LineId,
    side: &ShotSide,
    credits: &[Arc<SurveyCredit>],
    out: &mut Vec<AddedShot>,
    total: &mut usize,
) {
    let mut offset = 0u32;
    for seg in &side.s {
        if let Seg::Rows(rows) = seg {
            *total += rows.uuids.len();
            let take = MAX_LISTED_SHOTS
                .saturating_sub(out.len())
                .min(rows.uuids.len());
            let mut shots: Vec<Shot> = rows.uuids[..take]
                .iter()
                .map(|uuid| blank_shot(uuid.clone()))
                .collect();
            for (field, at, value) in rows.cols.cells() {
                if let Some(shot) = shots.get_mut(at as usize) {
                    field.set(shot, value, credits);
                }
            }
            for (r, shot) in shots.into_iter().enumerate() {
                out.push(AddedShot {
                    line,
                    index: offset + r as u32,
                    shot,
                });
            }
        }
        offset += seg.len();
    }
}

fn meta_fields(line: LineId, meta: &LineMetaEdit, out: &mut Vec<LineFieldChange>) {
    let mut push = |field, old, new| {
        out.push(LineFieldChange {
            line,
            field,
            old,
            new,
        });
    };
    if let Some((a, b)) = &meta.name {
        push(LineField::Name, text(a.as_ref()), text(b.as_ref()));
    }
    if let Some((a, b)) = &meta.color {
        push(LineField::Color, Val::Str(a.clone()), Val::Str(b.clone()));
    }
    if let Some((a, b)) = &meta.dir {
        push(
            LineField::Direction,
            Val::Str(a.to_string()),
            Val::Str(b.to_string()),
        );
    }
    if let Some((a, b)) = &meta.q {
        push(LineField::Quality, Val::Num(*a), Val::Num(*b));
    }
    if let Some((a, b)) = &meta.az {
        let az = |r: &Option<crate::AzimuthReference>| {
            r.map_or(Val::Null, |r| Val::Str(format!("{r:?}")))
        };
        push(LineField::AzimuthReference, az(a), az(b));
    }
    if let Some((a, b)) = &meta.sec {
        let id = |s: &Option<u32>| s.map_or(Val::Null, |s| Val::Int(u64::from(s)));
        push(LineField::Section, id(a), id(b));
    }
    if let Some((a, b)) = &meta.surveyor {
        push(LineField::Surveyor, text(a.as_ref()), text(b.as_ref()));
    }
    if let Some((a, b)) = &meta.date {
        let day = |d: &Option<chrono::NaiveDate>| d.map_or(Val::Null, |d| Val::Str(d.to_string()));
        push(LineField::Date, day(a), day(b));
    }
    if let Some((a, b)) = &meta.start {
        push(
            LineField::Start,
            Val::Int(u64::from(*a)),
            Val::Int(u64::from(*b)),
        );
    }
    if let Some((a, b)) = &meta.hand {
        push(
            LineField::HandMoves,
            Val::Int(a.len() as u64),
            Val::Int(b.len() as u64),
        );
    }
}

fn list<T: Clone>(change: Option<&ListChange<T>>) -> ListDiff<T> {
    change.map_or_else(ListDiff::default, |c| ListDiff {
        removed: c.side(false).cloned().collect(),
        added: c.side(true).cloned().collect(),
    })
}
