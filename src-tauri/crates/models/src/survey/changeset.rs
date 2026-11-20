//! The undo/redo engine of the station-graph model: id-keyed changesets
//! (plan 8; guide in `docs/survey-model.md`, "Undo engine").
//!
//! A [`Changeset`] records what one history step changed, as before and
//! after images keyed by stable ids: stations by [`crate::StationId`], lines
//! by [`crate::LineId`], shots by uuid through per-line runs, closures,
//! datums, placements, Sections and the import hashes as list hunks, the
//! project's name, location and settings, the line order and the id
//! counters. It is captured generically ([`capture`]: no per-command code),
//! applied either way with preconditions ([`undo`], [`redo`]: a step refuses,
//! and changes nothing, unless the live project is exactly the recorded
//! state it starts from), and lands bit for bit on the recorded state.
//!
//! The persisted history entry ([`HistoryEntry`], [`encode_entry`],
//! [`decode_entry`]) wraps a changeset with its time, kind, record-time
//! label, references and preview rows; the forward `Command` is never
//! persisted. [`Digest`] reads what a history preview shows straight off the
//! changeset.

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{LineId, Section, StationId, SurveyCredit};

use super::{Counters, LoopClosure, Placement, Project, ProjectSettings, ZeroDatum};

mod apply;
mod bits;
mod capture;
mod digest;
mod entry;
mod lines;
mod lists;
mod shape;
mod shots;
mod stations;
mod values;

#[cfg(any(test, feature = "test-support"))]
#[expect(
    clippy::panic,
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "test support: a refused setup step is a broken test"
)]
pub mod testkit;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod changeset_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod entry_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod property_tests;

pub use digest::{
    AddedShot, Digest, LineField, LineFieldChange, LineSummary, ListDiff, ProjectField,
    ProjectFieldChange, ShotCellChange, StationField, StationFieldChange, MAX_LISTED_SHOTS,
};
pub use entry::{
    decode_entry, encode_entry, EntryError, EntryLimits, HistoryEntry, HistoryRefs, PreviewRow,
    RecordedPreview, HISTORY_FORMAT_VERSION,
};
pub use values::{ShotField, Val};

use lines::LineChange;
use lists::ListChange;
use stations::StationChanges;

/// Which way a step goes through a changeset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the recorded AFTER state back to the BEFORE state.
    Undo,
    /// From the recorded BEFORE state to the AFTER state.
    Redo,
}

/// Why a step was refused. The project is left exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The changeset breaks a structural rule (a damaged or hostile entry).
    Shape(String),
    /// A project field (name, location, creation time, settings) is not the
    /// recorded one.
    ProjectField(&'static str),
    /// A station is not the recorded one (or is there when it should not be).
    Station(StationId),
    /// The line order is not the recorded one.
    LineOrder,
    /// A line is not the recorded one: its content hash, length or a recorded
    /// field differs, or it is missing or unexpectedly present.
    Line(LineId),
    /// A copy run's anchor shot is not where the record has it.
    Anchor(LineId),
    /// A list (datums, placements, closures, Sections, import hashes) is not
    /// the recorded one.
    List(&'static str),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lead = "The project is not in the state this history step expects";
        match self {
            Self::Shape(why) => write!(f, "This history step is damaged: {why}"),
            Self::ProjectField(field) => write!(f, "{lead}: the project {field} differs"),
            Self::Station(id) => write!(f, "{lead}: station {id} differs"),
            Self::LineOrder => write!(f, "{lead}: the line order differs"),
            Self::Line(id) => write!(f, "{lead}: line {id} differs"),
            Self::Anchor(id) => write!(f, "{lead}: a shot of line {id} is not where expected"),
            Self::List(which) => write!(f, "{lead}: the {which} differ"),
        }
    }
}

impl std::error::Error for Refusal {}

/// One undo/redo step's changes (plan 8.1): images of the touched entities
/// only, keyed by stable ids. See the module docs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changeset {
    #[serde(default, skip_serializing_if = "ProjectChanges::is_empty")]
    pub(crate) project: ProjectChanges,
    #[serde(default, skip_serializing_if = "StationChanges::is_empty")]
    pub(crate) stations: StationChanges,
    /// The line list's order (ids), when lines were created, deleted or
    /// reordered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) order: Option<ListChange<LineId>>,
    /// Every line whose metadata or shots changed, ascending id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) lines: Vec<LineChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) datums: Option<ListChange<ZeroDatum>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) placements: Option<ListChange<Placement>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) closures: Option<ListChange<LoopClosure>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sections: Option<ListChange<Section>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file_hashes: Option<ListChange<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) line_hashes: Option<ListChange<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) counters: Option<CounterChange>,
    /// The credits the shot rows and edits name, each once (plan 8.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) credits: Vec<Arc<SurveyCredit>>,
}

impl Changeset {
    /// Whether the step changed nothing (a capture of two equal states).
    pub fn is_empty(&self) -> bool {
        self.project.is_empty()
            && self.stations.is_empty()
            && self.order.is_none()
            && self.lines.is_empty()
            && self.datums.is_none()
            && self.placements.is_none()
            && self.closures.is_none()
            && self.sections.is_none()
            && self.file_hashes.is_none()
            && self.line_hashes.is_none()
            && self.counters.is_none()
    }

    /// Whether the changeset carries session shot ids (it was captured in
    /// this session, not decoded from a file).
    pub fn has_session_ids(&self) -> bool {
        self.lines.iter().any(|l| {
            l.before.as_ref().is_some_and(|s| s.hint.is_some())
                || l.after.as_ref().is_some_and(|s| s.hint.is_some())
        })
    }

    /// Drop the session shot ids, as a persisted round trip does.
    pub fn forget_session_ids(&mut self) {
        for line in &mut self.lines {
            for side in [&mut line.before, &mut line.after].into_iter().flatten() {
                side.hint = None;
            }
        }
    }

    /// Every number finite (the encoder refuses a non-finite one; a valid
    /// model holds none).
    pub(crate) fn finite(&self) -> bool {
        let lists_ok = self.datums.as_ref().is_none_or(|l| {
            l.side(false).chain(l.side(true)).all(|d| {
                [d.gps.latitude, d.gps.longitude, d.gps.depth_reference]
                    .iter()
                    .all(|v| v.is_finite())
                    && d.accuracy_m.is_none_or(f64::is_finite)
                    && d.correction.is_none_or(|c| c.iter().all(|v| v.is_finite()))
            })
        }) && self.placements.as_ref().is_none_or(|l| {
            l.side(false)
                .chain(l.side(true))
                .all(|p| p.x.is_finite() && p.y.is_finite())
        });
        let lines_ok = self.lines.iter().all(|line| {
            line.meta.finite()
                && [&line.before, &line.after]
                    .into_iter()
                    .flatten()
                    .all(shape::side_finite)
        });
        lists_ok && lines_ok && self.project.finite()
    }
}

/// Capture the changes from `before` to `after` (plan 8.1): diff after the
/// command's full epilogue, field-level images for the touched entities
/// only. Generic: no per-command code. Lines are compared by cheap bitwise
/// equality first; only the lines that differ are diffed shot by shot.
pub fn capture(before: &Project, after: &Project) -> Changeset {
    capture::capture(before, after)
}

/// Step `project` back through `changes`: the live project must be the
/// recorded AFTER state (on everything the changeset touches); otherwise the
/// step is refused and the project is left untouched. Counters are left at
/// `max(live, recorded)`, so an id once handed out is not handed out again.
pub fn undo(project: &mut Project, changes: &Changeset) -> Result<(), Refusal> {
    apply::apply(project, changes, Direction::Undo)
}

/// Step `project` forward through `changes`: the mirror of [`undo`] (the
/// live project must be the recorded BEFORE state).
pub fn redo(project: &mut Project, changes: &Changeset) -> Result<(), Refusal> {
    apply::apply(project, changes, Direction::Redo)
}

/// [`undo`] or [`redo`] by `direction`.
pub fn step(
    project: &mut Project,
    changes: &Changeset,
    direction: Direction,
) -> Result<(), Refusal> {
    apply::apply(project, changes, direction)
}

/// The project fields a step can change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) location: Option<(Option<String>, Option<String>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) created: Option<(DateTime<Utc>, DateTime<Utc>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) settings: Option<Box<(ProjectSettings, ProjectSettings)>>,
}

impl ProjectChanges {
    pub(crate) fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.location.is_none()
            && self.created.is_none()
            && self.settings.is_none()
    }

    fn finite(&self) -> bool {
        self.settings.as_ref().is_none_or(|pair| {
            [&pair.0, &pair.1].iter().all(|s| {
                let v = &s.validation;
                [
                    v.warn_shot_length_min,
                    v.warn_shot_length_max,
                    v.warn_depth_change_per_shot_max,
                    v.warn_azimuth_deviation_max,
                    v.azimuth_diff_threshold,
                    v.pitch_diff_threshold,
                    v.inclination_depth_diff_threshold,
                ]
                .iter()
                .all(|x| x.is_finite())
                    && s.surface_elevation.is_none_or(f64::is_finite)
                    && s.default_azimuth_reference
                        .is_none_or(|r| r.correction.is_finite())
            })
        })
    }
}

/// The id counters on both sides, `[[station, closure, section], ...]`.
/// Applied as `max(live, recorded)`: ids are never handed out twice.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct CounterChange(pub(crate) [u64; 3], pub(crate) [u64; 3]);

impl CounterChange {
    pub(crate) fn of(counters: &Counters) -> [u64; 3] {
        [
            u64::from(counters.next_station_id),
            counters.next_closure_id,
            u64::from(counters.next_section_id),
        ]
    }

    /// The counters a step in `direction` leaves: each the larger of the live
    /// value and the recorded target.
    pub(crate) fn stepped(&self, live: &Counters, direction: Direction) -> Counters {
        let target = match direction {
            Direction::Undo => self.0,
            Direction::Redo => self.1,
        };
        let station = u32::try_from(target[0]).unwrap_or(u32::MAX);
        let section = u32::try_from(target[2]).unwrap_or(u32::MAX);
        Counters {
            next_station_id: live.next_station_id.max(station),
            next_closure_id: live.next_closure_id.max(target[1]),
            next_section_id: live.next_section_id.max(section),
        }
    }
}
