//! The keep of a merge no dialog asks about where a station is placed onto
//! another (RS 4.1, U-P2-9): orphan placement merges a line an import left
//! unanchored onto a station of the anchored survey. The target (placed)
//! station is kept; rule K only decides when neither station is placed.
//!
//! Geometry is the merge's own (plan 4.2), whichever station is kept; every
//! name collision is lettered by rule L and reported, as on every path
//! without a dialog ([`Project::merge_stations_auto`]).

use crate::StationId;

use super::ops::{OpError, OpOutcome};
use super::{MergeReport, Project};

impl Project {
    /// The station a merge of `other` onto `target` keeps when no dialog
    /// asks (orphan placement): the closure-forced end when the two are the
    /// ends of an archiving closure (RS 4.1 step 4); else the station whose
    /// piece a datum places when only one of them is placed; `target` when
    /// both are; rule K over the two (node candidates) when neither is.
    pub fn placement_merge_keep(&self, other: StationId, target: StationId) -> StationId {
        if let Some((_, kept)) = self.forced_merge_keep(other, target) {
            return kept;
        }
        let placed = |s: StationId| self.placement_datum_of(s).is_some();
        match (placed(other), placed(target)) {
            (true, false) => other,
            (_, true) => target,
            (false, false) => self.rule_k_nodes(&[other, target]).unwrap_or(target),
        }
    }

    /// Merge `other` onto `target` without a dialog (orphan placement),
    /// keeping [`Self::placement_merge_keep`]'s station; every collision is
    /// lettered by rule L and reported (`OpOutcome::identities`).
    pub fn merge_by_placement(
        &mut self,
        other: StationId,
        target: StationId,
    ) -> Result<OpOutcome<MergeReport>, OpError> {
        let keep = self.placement_merge_keep(other, target);
        self.merge_stations_auto(other, target, keep)
    }
}
