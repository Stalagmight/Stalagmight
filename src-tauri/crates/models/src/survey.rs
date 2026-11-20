//! The station-graph survey model (`docs/plans/2026-11-20_station-graph-model.md`,
//! sections 1-5; module guide in `docs/survey-model.md`).
//!
//! Stations are entities with stable ids ([`StationId`], the user-visible
//! Station ID). A line is a simple chain of stations: `start`, then the `to` of
//! each shot. A station on two or more lines is a junction, which is what a
//! merge used to be. The model stores observations and user decisions only;
//! positions, depths, closure alignment and datum roles are derived
//! ([`Project::raw_positions`]).
//!
//! Layout:
//! - `types` - the entities ([`Station`], [`Line`], [`Shot`], [`ZeroDatum`],
//!   [`Placement`], [`LoopClosure`], ...).
//! - `topology` - the derived slot / chain / component index ([`Topology`]),
//!   cached on the project and dropped by every `&mut` access to the lines.
//! - `build` - the low-level construction API (not command-level).
//! - `validate` - the section 2 invariants ([`Violation`]).
//! - `positions` - solver v1 raw positions and depths ([`RawPositions`]).
//! - `anchors` - the post-edit anchor algorithm and the frame-change routine
//!   ([`AnchorSnapshot`], [`EditContext`]).
//! - `naming` - station-name uniqueness and shot-uuid checks.
//!
//! Commands (each one protocol: snapshot, mutate, settle, sweep, validate,
//! roll back on refusal):
//! - `edit` - line and shot operations, Combine Lines ([`EditError`],
//!   [`Edited`]).
//! - `ops`, `ops_*` - merge, detach, closures, datums, move tools
//!   ([`OpError`], [`OpOutcome`]); `sweep` - the orphan-closure sweep;
//!   `append` - Add .stalx.
//!
//! Derived views:
//! - `closures` - alignment, the across-one-shot skip set, tie groups;
//!   `metrics` - closure metrics; `adjust`, `lsq` - the Closures Computed
//!   View ([`AdjustedPositions`], [`SolveHold`]); `roles` - datum roles,
//!   entrances, GPS gaps; `rigid` - rigid groups, anchored lines, immovable
//!   stations, merge pairs; `frozen`; `stats`; `graph` - distances and the
//!   ruler; `gas` - the Plan-mode gas engine ([`GasReach`]).
//!
//! Old SQLite files are read by the frozen 0.6.0 model
//! (`stalagmight-legacy`) and migrated into this one.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{LineId, Section, StationId};

// The model (plan 1-4).
mod anchors;
mod build;
mod naming;
mod positions;
mod topology;
mod types;
mod validate;

// Undo and redo (plan 8).
pub mod changeset;

// Commands (plan 4-5).
mod append;
mod edit;
mod ops;
mod ops_closures;
mod ops_datums;
mod ops_detach;
mod ops_merge;
mod ops_move;
mod sweep;

// Derived views (plan 3).
mod adjust;
mod closures;
mod frozen;
mod gas;
mod graph;
mod lsq;
mod metrics;
mod rigid;
mod roles;
mod stats;

// The wire projection (plan 9.2).
pub mod wire;

#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "test helpers: a refused build is a broken test setup"
)]
mod test_helpers;

#[cfg(test)]
#[expect(
    clippy::panic,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod adjust_floor_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod adjust_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod anchors_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod build_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod derived_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod gama_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod graph_tests;
#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "test support: a refused conversion is a broken test setup"
)]
mod legacy_convert;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod legacy_crosscheck_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod positions_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod validate_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod append_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod ops_closures_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod ops_datums_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod ops_detach_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod ops_merge_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod ops_move_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod ops_property_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod ops_scenario_tests;
#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "test helpers: a refused setup step is a broken test"
)]
mod ops_test_helpers;

pub use adjust::{AdjustError, AdjustedPositions, AdjustmentFailure, ShotSigmas, SolveHold};
pub use anchors::{AnchorReport, AnchorSnapshot, EditContext, GpsEdit, MergeSides};
pub use build::{BuildError, StaleAttachments, StartAt};
pub use closures::{ClosureAcrossOneShot, ShotKey, TieGroups};
pub use gas::{GasProfilePoint, GasReach};
pub use graph::{AlongLine, RulerPoint, StationDistances, StationPositions};
pub use naming::NameConflict;
pub use positions::{
    Anchor, PositionIssue, RawPositions, SlotPosition, ALIGNMENT_TOLERANCE_M, CORRECTION_EPSILON_M,
};
pub use rigid::RigidGroups;
pub use roles::{FarFix, GpsGapWarning};
pub use stats::SurveyStats;
pub use topology::{Component, ShotLocation, ShotUuidIndex, Topology};
pub use types::{
    ClosureId, ClosureStrategy, Counters, Line, LineMeta, LoopClosure, Placement, ProjectSettings,
    Shot, Station, StationAlias, ZeroDatum, MAX_CLOSURE_ID, MAX_SHOTS_PER_LINE,
    MAX_SHOT_UUID_CHARS,
};
pub use validate::{ClosureEnd, CounterKind, NumberField, Violation};

pub use append::{AppendSummary, ShotUuidRename};
pub use changeset::{Changeset, HistoryEntry, Refusal};
pub use edit::{
    CombineCandidate, CombineJoin, CombineReport, JoinedMove, RemovedClosure, ReplacedClosure,
    TornTie, TornTieKind, TornTieReason,
};
pub use edit::{
    CreatedShot, DeleteRangePreview, DeleteShotPreview, DeletedLine, DeletedRange, DeletedShot,
    EditError, EditReport, Edited, RangeDeleteMode, ShotEditRun, ShotPlace, SplitLinePreview,
    SplitOptions, SplitShot,
};
pub use ops::{OpError, OpOutcome};
pub use ops_closures::ClosureCreated;
pub use ops_datums::{DatumChange, DatumSet};
pub use ops_detach::DetachReport;
pub use ops_merge::{MergeCheck, MergeDatumShift, MergeReport};
pub use ops_move::{SegmentMoved, StationMoved};
pub use sweep::{Promotion, SweepReport};

/// A survey project in the station-graph model (plan 1.2).
///
/// Every field is a stored observation or user decision. `lines` is the one
/// private field: the derived [`Topology`] (slots, chains, components) and the
/// shot-uuid index are built from it, so it is reached through
/// [`Self::lines`] / [`Self::line`] and mutated only through
/// [`Self::lines_mut`] / [`Self::line_mut`] (or the construction API), which
/// drop those caches. Everything else is a plain public field; none of it
/// feeds a cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub location: Option<String>,
    /// Creation instant. The nanoseconds must round-trip (`.plnx` ownership).
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    /// Every station, by id. A station is on at least one line (invariant 4).
    pub stations: BTreeMap<StationId, Station>,
    /// Lines in display order (list order is semantic: slot order, traversal
    /// tie-breaks, the per-component line lists).
    lines: Vec<Line>,
    /// Zero datums. List order is semantic: the first resolvable one is the
    /// frame, and the first one of each component places it.
    pub datums: Vec<ZeroDatum>,
    /// Where each datum-less component sits: exactly one per such component.
    pub placements: Vec<Placement>,
    /// Loop closures, in list order (sweeps and the across-one-shot skip set
    /// read it in that order).
    pub closures: Vec<LoopClosure>,
    /// Cave Sections (#327), in panel order.
    pub sections: Vec<Section>,
    pub settings: ProjectSettings,
    pub imported_file_hashes: Vec<String>,
    pub imported_line_hashes: Vec<String>,
    /// Id allocators. `next_line_id` is not stored: it is derived
    /// ([`Self::next_line_id`]).
    pub counters: Counters,
    /// Session only: where the project was opened from or saved to.
    #[serde(skip)]
    pub file_path: Option<PathBuf>,
    /// Session only: derived indexes, rebuilt on demand.
    #[serde(skip)]
    caches: Caches,
}

/// The derived indexes, built lazily from `lines` and dropped whenever the
/// lines are borrowed mutably. Never persisted, never part of equality.
#[derive(Debug, Clone, Default)]
struct Caches {
    topology: OnceLock<Arc<Topology>>,
    shot_uuids: OnceLock<Arc<ShotUuidIndex>>,
}

impl PartialEq for Caches {
    /// Caches are a pure function of the lines, which `Project`'s equality
    /// already compares.
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Project {
    /// The lines, in list order.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// Mutable access to the line list. Drops the derived caches; they are
    /// rebuilt on the next read.
    pub fn lines_mut(&mut self) -> &mut Vec<Line> {
        self.drop_caches();
        &mut self.lines
    }

    /// The line with id `id`, if any (the first one when an invalid model
    /// repeats an id).
    pub fn line(&self, id: LineId) -> Option<&Line> {
        match self.caches.topology.get() {
            Some(topology) => topology.line_position(id).and_then(|i| self.lines.get(i)),
            None => self.lines.iter().find(|line| line.id == id),
        }
    }

    /// Mutable access to one line. Drops the derived caches.
    pub fn line_mut(&mut self, id: LineId) -> Option<&mut Line> {
        let position = self.line_position(id)?;
        self.drop_caches();
        self.lines.get_mut(position)
    }

    /// The list position of line `id`.
    pub fn line_position(&self, id: LineId) -> Option<usize> {
        match self.caches.topology.get() {
            Some(topology) => topology.line_position(id),
            None => self.lines.iter().position(|line| line.id == id),
        }
    }

    /// The derived topology (slots, chains, components), built on first use
    /// after any change to the lines.
    pub fn topology(&self) -> &Topology {
        self.caches
            .topology
            .get_or_init(|| Arc::new(Topology::build(&self.lines)))
    }

    /// The topology behind an `Arc`, for code that must hold it while it
    /// mutates the project.
    pub fn topology_arc(&self) -> Arc<Topology> {
        self.topology();
        self.caches
            .topology
            .get()
            .map_or_else(|| Arc::new(Topology::build(&self.lines)), Arc::clone)
    }

    /// The shot-uuid index, built on first use after any change to the lines.
    pub fn shot_uuids(&self) -> &ShotUuidIndex {
        self.caches
            .shot_uuids
            .get_or_init(|| Arc::new(ShotUuidIndex::build(&self.lines)))
    }

    /// Edit the values of shot `index` of the line at list position
    /// `position` (measurements and annotations) without dropping the
    /// derived caches: the topology and the shot-uuid index read only a
    /// shot's `to` and `uuid`. Should `edit` change either (or the session
    /// id), the caches are dropped after all. `false` when there is no such
    /// shot.
    pub(super) fn edit_shot_values(
        &mut self,
        position: usize,
        index: usize,
        edit: impl FnOnce(&mut Shot),
    ) -> bool {
        let Some(shot) = self
            .lines
            .get_mut(position)
            .and_then(|line| line.shots.get_mut(index))
        else {
            return false;
        };
        let (to, id, uuid) = (shot.to, shot.id, shot.uuid.clone());
        edit(shot);
        if shot.to != to || shot.id != id || shot.uuid != uuid {
            self.drop_caches();
        }
        true
    }

    fn drop_caches(&mut self) {
        self.caches = Caches::default();
    }
}
