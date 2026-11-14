//! The gap-close of a contiguous shot range (Delete shots with shift), shared
//! by the command that runs it and the preview that predicts what it does to
//! loop closures (#737), so the two run the same code and cannot drift.

use std::collections::HashSet;

use crate::{SectionId, ShotId, StationKey, StationRef};

use super::{ClosurePair, Project};

impl Project {
    /// Remove the shots at positions `start_pos..=end_pos` of `section_id` by
    /// gap-closing: in forward order, each a single-shot
    /// [`Self::remove_shot_with_rewire_excluding_anchors`] with the per-shot
    /// cascade deferred. The caller runs the one cascade afterwards
    /// (`execute_delete_shot_range_shift`); the preview does not need it, as a
    /// cascade moves offsets, never merges or closures.
    ///
    /// The stations the WHOLE range removes are captured up front as canonical
    /// (position-independent) keys, so renumbering during the per-shot deletes
    /// cannot invalidate them. Each per-shot gap-close ignores anchors on these
    /// when deciding which side keeps its position - otherwise an anchor on a
    /// later, also-doomed range shot would displace the survivors (a suffix
    /// shift would keep the doomed tail fixed and shift the head). The caller
    /// has refused the whole section, so `start_pos == 0` means a prefix.
    ///
    /// On `Err` the project is part-way through the range: the command restores
    /// its snapshot, the preview drops its clone.
    pub fn gap_close_shot_range(
        &mut self,
        section_id: SectionId,
        start_pos: usize,
        end_pos: usize,
    ) -> Result<(), String> {
        let section = self
            .get_section(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        let n = section.shots.len();
        if start_pos > end_pos || end_pos >= n {
            return Err(format!(
                "Shot range {start_pos}..={end_pos} is not in section {section_id}"
            ));
        }
        let range_shot_ids: Vec<ShotId> = section.shots[start_pos..=end_pos]
            .iter()
            .map(|s| s.id)
            .collect();
        let (start, end, count) = (start_pos as u32, end_pos as u32, n as u32);
        let doomed_indices: Vec<u32> = if start_pos == 0 {
            (0..=end).collect() // prefix
        } else if end_pos == n - 1 {
            ((start + 1)..=count).collect() // suffix
        } else {
            ((start + 1)..=(end + 1)).collect() // interior
        };
        let doomed_anchors: HashSet<StationKey> = doomed_indices
            .iter()
            .filter_map(|&i| self.key_for_ref(StationRef::new(section_id, i)))
            .collect();
        for &sid in &range_shot_ids {
            self.remove_shot_with_rewire_excluding_anchors(
                section_id,
                sid,
                &doomed_anchors,
                /* defer_cascade */ true,
            )?;
        }
        Ok(())
    }

    /// The loop closures gap-closing `start_pos..=end_pos` of `section_id`
    /// would leave tying the two ends of one shot together (#737), named by
    /// their stations before the delete - exactly the closures
    /// `execute_delete_shot_range_shift` removes, because it is the same
    /// [`Self::gap_close_shot_range`] followed by the same
    /// [`Self::closures_newly_across_one_shot`], here on a clone.
    ///
    /// Skips the clone when no closure takes part in the solve (none can be
    /// across one shot then).
    pub fn preview_gap_close_closure_removals(
        &self,
        section_id: SectionId,
        start_pos: usize,
        end_pos: usize,
    ) -> Result<Vec<ClosurePair>, String> {
        if !self.has_eligible_closure() {
            return Ok(Vec::new());
        }
        self.clone()
            .into_gap_close_closure_removals(section_id, start_pos, end_pos)
    }

    /// [`Self::preview_gap_close_closure_removals`] on a copy the caller gives
    /// up, so it is not cloned again.
    pub fn into_gap_close_closure_removals(
        mut self,
        section_id: SectionId,
        start_pos: usize,
        end_pos: usize,
    ) -> Result<Vec<ClosurePair>, String> {
        if !self.has_eligible_closure() {
            return Ok(Vec::new());
        }
        let before = self.closures_before_edit();
        self.gap_close_shot_range(section_id, start_pos, end_pos)?;
        Ok(self.closures_newly_across_one_shot(&before))
    }

    fn has_eligible_closure(&self) -> bool {
        self.loop_closures
            .iter()
            .any(|lc| Self::closure_is_eligible(lc.strategy))
    }
}
