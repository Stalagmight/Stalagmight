//! Station references on the wire: the derived merge pairs, datums and
//! closures on their slots, and slot resolution at the command boundary.
//!
//! A stored reference is a station (plus a `via` line hint); the wire names
//! a slot. The slot of a datum is [`Project::datum_slot`] (its `via` line,
//! else its station's first line), of a closure end
//! [`Project::closure_end_slot`] (likewise); every other slot of a station is
//! in [`Project::slots_of`]. A migrated reference keeps the `via` of the
//! legacy member that held it, so it comes back on the slot it had.

use crate::{
    ClosurePair, ClosureStrategy as WireStrategy, DatumRole, LineId, LoopClosureWire, StationId,
    StationMergeWire, StationRef, ZeroDatumWire,
};

use super::super::{ClosureStrategy, LoopClosure, OpError, Project, RawPositions};

/// A slot resolved at the command boundary: the station it addresses and
/// the line it is on (the `via` hint a command records).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotTarget {
    pub station: StationId,
    pub line: LineId,
}

/// The wire strategy of a closure. `Tie` (a migrated legacy rigid tie) has
/// no variant of its own on the wire: it is shown as an aligned `Batch`
/// closure, which is what a legacy aligned closure looked like and what
/// the frontend's eligibility table already treats like a `Tie` (eligible).
pub fn wire_strategy(strategy: ClosureStrategy) -> WireStrategy {
    strategy.to_shared().unwrap_or(WireStrategy::Batch)
}

impl Project {
    /// Resolve a slot from the IPC: the station at it and its line, or
    /// [`OpError::UnknownSlot`] for a line that does not exist or an index
    /// past the line's last station.
    pub fn resolve_slot(&self, slot: StationRef) -> Result<SlotTarget, OpError> {
        Ok(SlotTarget {
            station: self.station_of_slot(slot)?,
            line: slot.line_id,
        })
    }

    /// Resolve the two slots of a pair command (merge, closure, pair
    /// detach), `a` first.
    pub fn resolve_slot_pair(
        &self,
        a: StationRef,
        b: StationRef,
    ) -> Result<(SlotTarget, SlotTarget), OpError> {
        Ok((self.resolve_slot(a)?, self.resolve_slot(b)?))
    }

    /// The wire's `station_merges` (plan 9.2): for each junction, ascending
    /// by station id, its first slot paired with each of its other slots.
    /// A derived pair has no merge time; it carries the project's
    /// `created_at`, which no edit changes (nothing in the app reads it).
    pub fn station_merges_wire(&self) -> Vec<StationMergeWire> {
        self.merge_pairs()
            .into_iter()
            .map(|(station_a, station_b)| StationMergeWire {
                station_a,
                station_b,
                merged_at: self.created_at,
            })
            .collect()
    }

    /// The wire's `zero_datums`: every datum in list order on its slot, with
    /// its derived role, entrance flag and, for a GPS fix, its gap (today's
    /// `zero_datums_wire`). A datum whose station is on no line is left out.
    pub fn zero_datums_wire(&self) -> Vec<ZeroDatumWire> {
        self.zero_datums_wire_with(&self.raw_positions())
    }

    /// [`Self::zero_datums_wire`] from this state's raw layout `raw`.
    pub fn zero_datums_wire_with(&self, raw: &RawPositions) -> Vec<ZeroDatumWire> {
        let gaps = self.gps_fix_gaps_from(raw);
        self.datums
            .iter()
            .enumerate()
            .zip(self.datum_roles())
            .filter_map(|((index, datum), role)| {
                let role = role?;
                let gap = match role {
                    DatumRole::GpsFix => gaps
                        .iter()
                        .find(|(fix, _)| *fix == index)
                        .map(|(_, gap)| *gap),
                    DatumRole::Placement => None,
                };
                Some(ZeroDatumWire {
                    station_ref: self.datum_slot(datum)?,
                    gps: datum.gps,
                    accuracy_m: datum.accuracy_m,
                    role,
                    is_entrance: self.datum_is_entrance(datum, role),
                    gap,
                })
            })
            .collect()
    }

    /// The wire's `loop_closures`: every closure in list order with its ends
    /// on their slots, the metrics of the raw layout `raw` and the alignment
    /// `aligned` (parallel to `closures`). A closure with an end on no line
    /// is left out.
    pub fn loop_closures_wire(&self, raw: &RawPositions, aligned: &[bool]) -> Vec<LoopClosureWire> {
        self.closures
            .iter()
            .zip(self.closures_metrics(raw))
            .enumerate()
            .filter_map(|(index, (closure, metrics))| {
                let metrics = metrics?;
                let pair = self.closure_pair(closure)?;
                Some(LoopClosureWire {
                    id: closure.id,
                    station_a: pair.station_a,
                    station_b: pair.station_b,
                    path_length: metrics.path_length,
                    misclosure_distance: metrics.misclosure_distance,
                    error_percentage: metrics.error_percentage,
                    horizontal_misclosure: metrics.horizontal_misclosure,
                    vertical_misclosure: metrics.vertical_misclosure,
                    strategy: wire_strategy(closure.strategy),
                    is_aligned: aligned.get(index).copied().unwrap_or(false),
                    archived: self.closure_archive_wire(closure),
                })
            })
            .collect()
    }

    /// A closure by its id and its two end slots, in this state.
    pub(super) fn closure_pair(&self, closure: &LoopClosure) -> Option<ClosurePair> {
        Some(ClosurePair {
            closure_id: closure.id,
            station_a: self.closure_end_slot(closure.a, closure.a_via)?,
            station_b: self.closure_end_slot(closure.b, closure.b_via)?,
        })
    }
}
