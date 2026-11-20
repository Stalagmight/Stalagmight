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
//! - `edit/reverse.rs` - reverse a line (plan 4.4).
//! - `edit/split.rs` - Split Line Here (the halves share the split station).
//! - `edit/combine/` - Combine Lines (candidates, plan preview, the join).
//! - `edit/fold.rs` - the station fold a merge, a promotion and a combine
//!   share (one station replaced by another everywhere).
//! - `edit/one_shot.rs` - the across-one-shot skip set (#737), a minimal
//!   local version for the removals these operations make.

use std::fmt;

use crate::LineId;

use super::{
    AnchorReport, AnchorSnapshot, ClosureId, EditContext, LoopClosure, Project, StaleAttachments,
    SweepReport, Violation,
};

mod combine;
mod fold;
mod lines;
mod one_shot;
mod ranges;
mod reverse;
mod shots;
mod split;

pub use combine::{
    CombineCandidate, CombineJoin, CombineReport, JoinedMove, RemovedClosure, ReplacedClosure,
    TornTie, TornTieKind, TornTieReason,
};
pub use fold::{CommentRule, Fold};
pub use lines::DeletedLine;
pub use ranges::{DeleteRangePreview, DeletedRange, RangeDeleteMode};
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
        let edited = work.run_edit(edit)?;
        *self = work;
        Ok(edited)
    }

    /// The operation protocol on `self` itself (a working copy): snapshot,
    /// mutate, epilogue, validate. On `Err` `self` is part-way and must be
    /// dropped.
    pub(super) fn run_edit<T>(
        &mut self,
        edit: impl FnOnce(&mut Project) -> Result<(T, Epilogue), EditError>,
    ) -> Result<Edited<T>, EditError> {
        let before = self.anchor_snapshot();
        let skipped_before = self.closures_across_one_shot_in(before.positions());
        let (value, epilogue) = edit(self)?;
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
            self.sweep_orphan_closures()
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
