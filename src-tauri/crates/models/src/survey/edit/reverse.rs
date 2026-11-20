//! Reverse a line end-to-end (plan 4.4, #24): the far end becomes station 0
//! and no station moves.
//!
//! The chain is reversed (the old last station is the new start) and every
//! shot is flipped by today's transform (`crate::reverse_shots`: bearings
//! reciprocated and swapped, the degenerate antipodal pair swapped only,
//! depths swapped, pitches swapped and negated, left and right swapped when
//! asked). Uuids and credits travel with their records; session shot ids
//! stay with their positions, as today. Stations, their names, comments,
//! datums, closures and hand moves are keyed by station id, so nothing needs
//! re-keying: they stay on the physical station.
//!
//! Positions do not change, up to the documented ulp rounding of the
//! `(az + 180) % 360` turn.

use crate::reverse_shots;
use crate::{LineId, ShotData};

use super::super::{EditContext, Project, Shot};
use super::{EditError, Edited, Epilogue};

impl Project {
    /// Reverse `line` (module docs). `swap_lr` also swaps every shot's left
    /// and right. A line with no shots reverses to itself.
    pub fn reverse_line(&mut self, line: LineId, swap_lr: bool) -> Result<Edited<()>, EditError> {
        self.line_or_refuse(line)?;
        self.apply_edit(|p| {
            p.reverse_line_in_place(line, swap_lr)?;
            Ok(((), Epilogue::settle(EditContext::line(line))))
        })
    }

    /// The mutation of [`Self::reverse_line`] (also used by Combine Lines).
    pub(super) fn reverse_line_in_place(
        &mut self,
        line: LineId,
        swap_lr: bool,
    ) -> Result<(), EditError> {
        let Some(l) = self.line_mut(line) else {
            return Err(EditError::line_gone());
        };
        if l.shots.is_empty() {
            return Ok(());
        }
        let chain: Vec<_> = l.stations().collect();
        let ids: Vec<_> = l.shots.iter().map(|s| s.id).collect();
        let mut data: Vec<ShotData> = l.shots.iter().map(Shot::to_shot_data).collect();
        reverse_shots(&mut data, swap_lr);
        let n = chain.len();
        // Reversed shot i runs from old station n-1-i to old station n-2-i.
        l.start = chain[n - 1];
        l.shots = data
            .into_iter()
            .zip(ids)
            .enumerate()
            .map(|(i, (d, id))| Shot::from_data(id, d, chain[n - 2 - i]))
            .collect();
        Ok(())
    }
}
