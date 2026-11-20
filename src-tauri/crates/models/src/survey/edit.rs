//! User-level line and shot operations on the station-graph model (plan 4.4,
//! 5.3, 5.4): create, delete and edit lines and shots, delete shot ranges,
//! reverse, split and combine lines, with today's refusals and the facts the
//! dialogs show before the user confirms.
//!
//! Every operation runs one protocol ([`Project::run_edit`]):
//!
//! 1. take an [`super::AnchorSnapshot`] (and, when the operation removes
//!    closures that end up across one shot, the closures the solve skips
//!    now);
//! 2. mutate the model on a working copy;
//! 3. epilogue: [`Project::drop_stale_attachments`], then
//!    [`Project::settle_anchors`] with the [`super::EditContext`] the
//!    operation declares, then (topology edits) the orphan-closure sweep
//!    ([`Project::sweep_orphan_closures`]) and the removal of closures the
//!    edit newly left across one shot (#737);
//! 4. [`Project::validate`]: any violation refuses the operation and the
//!    project is left exactly as it was ([`EditError::Invalid`]).
//!
//! The working copy is committed only when everything succeeded, so a refusal
//! never leaves a half-applied edit, and a preview is the same operation run
//! on a copy that is then dropped ("a dry run on a clone", plan 5).
//!
//! Operations never touch `modified_at`: the command layer stamps it, so a
//! model operation is a pure function of its inputs (the property test checks
//! that the same sequence gives the same bits).
//!
//! Files:
//! - `edit/shots.rs` - create, update, set kind, delete, split at distance;
//!   the delete-shot preview.
//! - `edit/lines.rs` - create, delete, metadata, reorder.
//! - `edit/ranges.rs` - delete a shot range (gap-close, split, prefix,
//!   suffix) and its preview.
//! - `edit/junction_leave.rs` - what a delete does with the closures its line
//!   kept at a junction it leaves (0.6.0's member semantics, S13).
//! - `edit/reverse.rs` - reverse a line (plan 4.4).
//! - `edit/split.rs` - Split Line Here (the halves share the split station).
//! - `edit/combine/` - Combine Lines (candidates, plan preview, the join).
//!
//! Shared with the topology commands (`ops_*.rs`): the merge and detach
//! mutations Combine Lines joins and tears with (`Project::merge_ids`,
//! `Project::detach_ids`), the orphan-closure sweep (`sweep.rs`), and the
//! across-one-shot skip set closures are removed by, which is the solve's
//! own ([`Project::closures_across_one_shot`], `closures.rs`).

use std::fmt;

use crate::LineId;

use super::{
    AnchorReport, AnchorSnapshot, ClosureEnd, ClosureId, EditContext, LoopClosure, Project,
    RawPositions, StaleAttachments, SweepReport, Violation,
};

mod combine;
mod junction_leave;
mod lines;
mod ranges;
mod reverse;
mod shot_run;
mod shots;
mod split;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod combine_port_closure_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod combine_port_lines_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod combine_port_misc_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod combine_tests;
#[cfg(test)]
mod edit_fuzz_tests;
#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "test helpers: a refused edit is a broken test setup"
)]
mod edit_test_helpers;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod lines_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod ranges_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod reverse_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod shot_run_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod shots_closure_promotion_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod shots_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod split_tests;

pub use combine::{
    CombineCandidate, CombineJoin, CombineReport, JoinedMove, RemovedClosure, ReplacedClosure,
    TornTie, TornTieKind, TornTieReason,
};
pub use lines::DeletedLine;
pub use ranges::{DeleteRangePreview, DeletedRange, RangeDeleteMode};
pub use shot_run::ShotEditRun;
pub use shots::{CreatedShot, DeleteShotPreview, DeletedShot, ShotPlace, SplitShot};

use super::Promotion;
pub use split::{SplitLinePreview, SplitOptions};

/// Why an operation was refused. Nothing changed.
#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    /// The user-facing refusal, worded as today's command words it.
    Refused(String),
    /// The edit would break an invariant of the model (plan section 2); it
    /// was rolled back.
    Invalid(Vec<Violation>),
}

impl EditError {
    /// A refusal with `text`.
    pub fn refused(text: impl Into<String>) -> Self {
        Self::Refused(text.into())
    }

    /// Today's refusal for a line that is gone.
    pub(super) fn line_gone() -> Self {
        Self::refused(crate::LINE_GONE)
    }

    /// Today's refusal for a shot that is gone.
    pub(super) fn shot_gone() -> Self {
        Self::refused(crate::SHOT_GONE)
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(text) => f.write_str(text),
            Self::Invalid(violations) => {
                f.write_str("the edit would leave the survey invalid")?;
                for violation in violations {
                    write!(f, "; {violation}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for EditError {}

impl From<super::OpError> for EditError {
    fn from(error: super::OpError) -> Self {
        match error {
            super::OpError::Invalid(violations) => Self::Invalid(violations),
            other => Self::Refused(other.to_string()),
        }
    }
}

impl From<super::BuildError> for EditError {
    fn from(error: super::BuildError) -> Self {
        Self::Refused(error.to_string())
    }
}

/// What an operation's epilogue did besides the operation itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditReport {
    /// What the anchor algorithm did (frame change, promoted datums, dropped
    /// placements).
    pub anchors: AnchorReport,
    /// `via` hints and hand moves cleared because their station left a line.
    pub stale: StaleAttachments,
    /// What the orphan-closure sweep did (topology edits only).
    pub sweep: SweepReport,
    /// Closures removed because the edit left them across one shot (#737),
    /// as they were before the edit, in list order.
    pub removed_across_one_shot: Vec<LoopClosure>,
}

/// An operation's result with its epilogue report.
#[derive(Debug, Clone, PartialEq)]
pub struct Edited<T> {
    pub value: T,
    pub report: EditReport,
}

/// What an operation asks of the epilogue.
#[derive(Debug, Clone, Default)]
pub(super) struct Epilogue {
    /// What kind of edit ran, for the anchor algorithm.
    pub(super) ctx: EditContext,
    /// Run the orphan-closure sweep (topology edits).
    pub(super) sweep: bool,
    /// Remove the closures the edit newly left across one shot (#737).
    pub(super) remove_across_one_shot: bool,
    /// Closure ends on a junction the edited line left: the sweep moves
    /// their piece on promotion (`edit/junction_leave.rs`).
    pub(super) moving_ends: Vec<(ClosureId, ClosureEnd)>,
}

impl Epilogue {
    /// Settle anchors only.
    pub(super) fn settle(ctx: EditContext) -> Self {
        Self {
            ctx,
            ..Self::default()
        }
    }

    /// A topology edit: settle, sweep, and remove closures newly across one
    /// shot.
    pub(super) fn topology(ctx: EditContext) -> Self {
        Self {
            ctx,
            sweep: true,
            remove_across_one_shot: true,
            moving_ends: Vec::new(),
        }
    }

    /// A delete that took its line off junctions: [`Self::topology`] with
    /// the closure ends the line left there (`edit/junction_leave.rs`).
    pub(super) fn delete(ctx: EditContext, moving_ends: Vec<(ClosureId, ClosureEnd)>) -> Self {
        Self {
            moving_ends,
            ..Self::topology(ctx)
        }
    }
}

impl Project {
    /// Run `edit` with the operation protocol (module docs) and commit it only
    /// when it succeeds and the result validates; otherwise the project is
    /// left exactly as it was.
    pub(super) fn apply_edit<T>(
        &mut self,
        edit: impl FnOnce(&mut Project) -> Result<(T, Epilogue), EditError>,
    ) -> Result<Edited<T>, EditError> {
        let mut work = self.clone();
        let edited = work.run_edit(self, edit)?;
        *self = work;
        Ok(edited)
    }

    /// The operation protocol on `self`, a working copy of `original`:
    /// snapshot, mutate, epilogue, validate. On `Err` `self` is part-way and
    /// must be dropped. `original` (the state before the edit) is read only
    /// when the epilogue removes closures newly across one shot.
    pub(super) fn run_edit<T>(
        &mut self,
        original: &Project,
        edit: impl FnOnce(&mut Project) -> Result<(T, Epilogue), EditError>,
    ) -> Result<Edited<T>, EditError> {
        let before = self.anchor_snapshot();
        let (value, epilogue) = edit(self)?;
        let skipped_before = if epilogue.remove_across_one_shot {
            original.skipped_closure_ids(before.positions())
        } else {
            Vec::new()
        };
        let report = self.finish_edit(&before, &skipped_before, &epilogue);
        let violations = self.validate();
        if !violations.is_empty() {
            return Err(EditError::Invalid(violations));
        }
        Ok(Edited { value, report })
    }

    /// The epilogue of [`Self::run_edit`] (steps 3 of the module docs).
    fn finish_edit(
        &mut self,
        before: &AnchorSnapshot,
        skipped_before: &[ClosureId],
        epilogue: &Epilogue,
    ) -> EditReport {
        let stale = self.drop_stale_attachments();
        let anchors = self.settle_anchors(before, &epilogue.ctx);
        let sweep = if epilogue.sweep {
            self.sweep_orphan_closures_moving(&epilogue.moving_ends)
        } else {
            SweepReport::default()
        };
        let removed_across_one_shot = if epilogue.remove_across_one_shot {
            self.remove_closures_newly_across_one_shot(skipped_before)
        } else {
            Vec::new()
        };
        EditReport {
            anchors,
            stale,
            sweep,
            removed_across_one_shot,
        }
    }

    /// The ids of the closures the solve skips as across one shot (#737), in
    /// list order, for this state's raw layout `positions`: the solve's own
    /// skip set ([`Project::closures_across_one_shot_with`]).
    pub(super) fn skipped_closure_ids(&self, positions: &RawPositions) -> Vec<ClosureId> {
        if self.closures.is_empty() {
            return Vec::new();
        }
        let aligned = self.closure_alignment(positions);
        self.closures_across_one_shot_with(&aligned)
            .into_iter()
            .map(|c| c.closure_id)
            .collect()
    }

    /// Remove the closures the solve skips now that it did not skip in
    /// `before` (by id): what an edit newly left across one shot (#737).
    /// Returns them as they were, in list order.
    pub(super) fn remove_closures_newly_across_one_shot(
        &mut self,
        before: &[ClosureId],
    ) -> Vec<LoopClosure> {
        let now = self.skipped_closure_ids(&self.raw_positions());
        let newly: Vec<ClosureId> = now.into_iter().filter(|id| !before.contains(id)).collect();
        if newly.is_empty() {
            return Vec::new();
        }
        let mut removed = Vec::new();
        self.closures.retain(|c| {
            if newly.contains(&c.id) {
                removed.push(*c);
                false
            } else {
                true
            }
        });
        removed
    }

    /// The line, or today's "That line no longer exists." refusal.
    pub(super) fn line_or_refuse(&self, line: LineId) -> Result<&super::Line, EditError> {
        self.line(line).ok_or_else(EditError::line_gone)
    }

    /// A line as the user knows it: its name, or `unnamed line` when it has
    /// none (today's `line_label`).
    pub fn line_label_of(&self, line: LineId) -> String {
        self.line(line)
            .and_then(|l| l.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| crate::UNNAMED_LINE.to_string())
    }
}
