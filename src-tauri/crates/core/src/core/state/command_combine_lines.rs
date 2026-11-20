//! Combine Lines as one atomic undo/redo unit: the inverse of Split Line
//! Here (`command_split_line.rs`), and built the same way.

use super::AppState;

use crate::core::{Command, CommandResult, ValidationWarning, WarningSeverity};

use crate::models::{
    CombineCandidate, CombineLinesOptions, CombineRemovedClosure, CombineRemovedKind, LineId,
    LineUpdates, StationRef, CLOSURE_REMOVED_ACROSS_ONE_SHOT,
};


impl AppState {
    /// Read-only: what can be combined at the point `station` ends at: the
    /// clicked line with each other line ending there first, then (at a
    /// junction) every pair of the others, each with its own reason when
    /// blocked (`Project::combine_candidates_at_point`). Empty when `station`
    /// is not a line end or nothing else touches it. Answered under the project
    /// lock without cloning the project: the context menu asks on every
    /// right-click of an end station.
    pub fn combine_candidates(&self, station: StationRef) -> Result<Vec<CombineCandidate>, String> {
        let _ = (&station,);
        crate::core::stub::not_ported("command_combine_lines::combine_candidates")
    }

    /// Read-only: can lines `a` and `b` be combined (the Sections panel's
    /// two-line selection)? `None` when either is missing or they are the
    /// same line.
    pub fn combine_candidate_for_lines(
        &self,
        a: LineId,
        b: LineId,
    ) -> Result<Option<CombineCandidate>, String> {
        let _ = (&a, &b);
        crate::core::stub::not_ported("command_combine_lines::combine_candidate_for_lines")
    }

    /// Combine `other` into `kept` (see `Project::combine_lines`). One undo
    /// entry (`ProjectSnapshotPair`) covers the optional reverse of `other` and
    /// the combine, and the closures across one shot the combine removes
    /// (#746), so one Undo restores them too. A refusal (the candidate's
    /// blocked reason) changes nothing and pushes no undo entry. Returns
    /// `kept` as `line_id`, and a `CLOSURE_REMOVED_ACROSS_ONE_SHOT` warning
    /// per removed tie.
    pub fn execute_combine_lines(
        &self,
        kept: LineId,
        other: LineId,
        options: CombineLinesOptions,
    ) -> CommandResult {
        let _ = (&kept, &other, &options);
        crate::core::stub::not_ported("command_combine_lines::execute_combine_lines")
    }
}
