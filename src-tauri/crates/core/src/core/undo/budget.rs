use std::collections::VecDeque;

use super::{
    DepthLimit, StackSlot, UndoData, UndoEntry, UndoStack, COUNT_WINDOW_MEMORY_BYTES,
    ENTRY_OVERHEAD_BYTES,
};

impl UndoStack {
    /// [#303] Estimated serialized size of one entry, in bytes.
    ///
    /// An ESTIMATE by design: it exists to bound growth, and measuring exactly would
    /// mean serializing the whole stack on every push - the cost the byte budget is
    /// there to avoid. The dominant terms are exact (the snapshot variants hold their
    /// JSON as `String` already, so their length is the real payload); the structured
    /// variants are approximated from their element counts, and everything else is
    /// small enough that the flat overhead covers it.
    ///
    /// EVERY variant that owns a `previous_offsets` map charges `BYTES_PER_OFFSET`
    /// per entry on top of whatever else its arm counts. The map is the one field
    /// that grows with the survey rather than with the edit - the rigid connected
    /// set of the edited section - and a keystroke-scale shot edit records one, so
    /// leaving an arm's map uncounted lets a Full-mode history grow past its budget
    /// while the estimate says it has not.
    pub fn estimated_size(entry: &UndoEntry) -> usize {
        /// Roughly the JSON size of one serialized shot, from the measured
        /// project-shaped payload (~420 bytes/shot).
        const BYTES_PER_SHOT: usize = 420;
        /// Roughly the JSON size of one `previous_offsets` entry: a section id
        /// keyed to an `Option<(f64, f64)>`. Charged by EVERY variant that owns
        /// such a map, so a long merge chain's offsets count against the budget
        /// wherever they were recorded.
        const BYTES_PER_OFFSET: usize = 64;

        let payload = match &entry.undo_data {
            // Exact: these hold the serialized project already.
            UndoData::ProjectSnapshot(json) => json.len(),
            UndoData::ProjectSnapshotPair { before, after } => before.len() + after.len(),
            UndoData::PreviousValues {
                values_json,
                previous_offsets,
                ..
            } => values_json.len() + previous_offsets.len() * BYTES_PER_OFFSET,
            // Approximated from element counts - a section snapshot of a big section is
            // the only other variant that can reach megabytes.
            UndoData::SectionSnapshot { section_data, .. }
            | UndoData::CreatedSectionSnapshot { section_data, .. } => {
                section_data.shots.len() * BYTES_PER_SHOT
            }
            UndoData::ShotSnapshot { .. } => BYTES_PER_SHOT,
            UndoData::CreatedShotSnapshot {
                previous_offsets,
                legacy_comment_rows,
                ..
            } => {
                BYTES_PER_SHOT
                    + previous_offsets.len() * BYTES_PER_OFFSET
                    + legacy_comment_rows
                        .iter()
                        .map(|(_, text)| 128 + text.len())
                        .sum::<usize>()
            }
            UndoData::MergeInfo {
                removed_closures,
                previous_offsets,
                ..
            } => removed_closures.len() * 256 + previous_offsets.len() * BYTES_PER_OFFSET,
            UndoData::UnmergeInfo {
                removed_closures, ..
            } => removed_closures.len() * 256,
            UndoData::ClosureInfo { .. } => 256,
            UndoData::ZeroDatumInfo {
                previous_offsets, ..
            } => previous_offsets.len() * BYTES_PER_OFFSET,
            UndoData::SegmentMoveInfo {
                affected_sections,
                previous_offsets,
                ..
            } => affected_sections.len() * 16 + previous_offsets.len() * BYTES_PER_OFFSET,
            UndoData::StationMoveInfo { .. } => 2 * BYTES_PER_SHOT,
            UndoData::StationCommentRows { previous, .. } => {
                previous.iter().map(|(_, text)| 128 + text.len()).sum()
            }
            UndoData::InverseCommand(_) | UndoData::None => 0,
        };
        // Restored rows deliberately do not reserialize their command just to rebuild
        // this cache. They retain the historical 512-byte estimate; snapshot and other
        // undo payloads remain the dominant, separately-counted part of the budget.
        // That floor is what `MAX_PERSISTED_HISTORY_ROWS` is derived from, so a restored
        // entry must keep costing at least `ENTRY_OVERHEAD_BYTES`.
        payload + entry.command_bytes.unwrap_or(ENTRY_OVERHEAD_BYTES)
    }

    /// [#303] Total estimated size of the undo stack (the budget's subject).
    pub fn estimated_undo_bytes(&self) -> usize {
        self.undo_bytes
    }

    /// [#303] Total estimated size of the redo stack.
    pub fn estimated_redo_bytes(&self) -> usize {
        self.redo_bytes
    }

    /// Apply whichever bound is active to BOTH stacks, evicting oldest-first.
    ///
    /// Under `Bytes`, the newest entry is always kept even if it alone exceeds the
    /// budget: dropping it would silently discard the operation the user just did, and
    /// a single entry over the budget is a very large import, not runaway growth.
    ///
    /// [#576 B20] `Count` is bounded in bytes too, at [`COUNT_WINDOW_MEMORY_BYTES`]
    /// with the same newest-kept rule: a count alone let 100 whole-project snapshot
    /// pairs hold several GB on a large survey.
    pub(super) fn enforce_depth_limit(&mut self) {
        match self.depth_limit {
            DepthLimit::Count(limit) => {
                Self::trim_to_count(&mut self.undo_stack, &mut self.undo_bytes, limit);
                Self::trim_to_count(&mut self.redo_stack, &mut self.redo_bytes, limit);
                let ceiling = COUNT_WINDOW_MEMORY_BYTES;
                let undo_dropped =
                    Self::trim_to_bytes(&mut self.undo_stack, &mut self.undo_bytes, ceiling);
                let redo_dropped =
                    Self::trim_to_bytes(&mut self.redo_stack, &mut self.redo_bytes, ceiling);
                if undo_dropped + redo_dropped > 0 {
                    tracing::info!(
                        undo_dropped,
                        redo_dropped,
                        undo_remaining = self.undo_stack.len(),
                        estimated_undo_bytes = self.undo_bytes,
                        ceiling,
                        purpose = "bound the in-memory undo history in bytes (#576 B20)",
                        "Trimmed the oldest undo/redo entries past the in-memory byte ceiling"
                    );
                }
            }
            DepthLimit::Bytes(budget) => {
                let undo_dropped =
                    Self::trim_to_bytes(&mut self.undo_stack, &mut self.undo_bytes, budget);
                Self::trim_to_bytes(&mut self.redo_stack, &mut self.redo_bytes, budget);
                if undo_dropped > 0 {
                    tracing::info!(
                        dropped = undo_dropped,
                        remaining = self.undo_stack.len(),
                        estimated_bytes = self.undo_bytes,
                        budget,
                        purpose = "bound a Full-mode SpeleoDB project's undo history (#303)",
                        "Trimmed oldest undo entries to stay within the undo byte budget"
                    );
                }
            }
        }
    }

    fn trim_to_count(stack: &mut VecDeque<StackSlot>, total: &mut usize, limit: usize) {
        while stack.len() > limit {
            if let Some(front) = stack.pop_front() {
                *total = total.saturating_sub(Self::estimated_size(&front));
            }
        }
    }

    /// Evict from the front until the running total is at most `budget`, always
    /// keeping the newest entry. Returns the number evicted.
    fn trim_to_bytes(stack: &mut VecDeque<StackSlot>, total: &mut usize, budget: usize) -> u64 {
        let mut dropped = 0;
        while *total > budget && stack.len() > 1 {
            if let Some(front) = stack.pop_front() {
                *total = total.saturating_sub(Self::estimated_size(&front));
                dropped += 1;
            }
        }
        dropped
    }
}
