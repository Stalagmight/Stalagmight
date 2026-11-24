//! The input of the binary load envelope (`load_project_full_binary`,
//! `crates/core/src/core/load_binary.rs`, decoded by
//! `src/utils/loadProjectBinary.ts`) from the station-graph model.
//!
//! The encoder reads, today: the project header fields, each line's
//! metadata and shots (packed into the per-shot typed arrays), the wire
//! `station_merges` / `zero_datums` / `loop_closures`, `line_offsets`,
//! `station_offsets` and `station_comments_map` (both resolved to slots),
//! the per-slot station list, the stats and the frozen shots. Every one of
//! them is here with the same field names and element types, so the
//! encoder's packing loops are unchanged; the only layout change is that
//! `line_offsets` is gone (plan 9.2), which is a new envelope version (8 ->
//! 9, see docs/survey-model.md, "Wire API").

use crate::StationRef;

use super::super::{Project, RawPositions};
use super::ProjectStateWire;

/// Everything the load envelope packs (beside the load's own `loading_id`,
/// `is_remote`, warnings and display settings).
#[derive(Debug, Clone)]
pub struct LoadEnvelopeInput {
    /// The header fields and lines (`state.project`), the per-slot raw
    /// stations, the stats and the frozen shots: exactly
    /// `get_project_state`'s payload, so a binary load and a JSON refresh
    /// of one state agree.
    pub state: ProjectStateWire,
    /// `ProjectHeaderBinary.station_offsets`: every legacy hand move on its
    /// slot, lines in list order then chain index.
    pub station_offsets: Vec<(StationRef, (f64, f64, f64))>,
    /// `ProjectHeaderBinary.station_comments_map`: the comment of every slot
    /// whose station has one (a junction's comment on each of its slots),
    /// wire order. The decoder does not read it; the per-slot comment it
    /// reads is `Station::comment`.
    pub station_comments_map: Vec<(StationRef, String)>,
}

impl Project {
    /// The load envelope's input for this state; `include_reference_legs`
    /// is the penetration figure's opt-in, as for
    /// [`Self::project_state_wire`].
    pub fn load_envelope_input(&self, include_reference_legs: bool) -> LoadEnvelopeInput {
        self.load_envelope_input_with(&self.raw_positions(), include_reference_legs)
    }

    /// [`Self::load_envelope_input`] from this state's raw layout `raw` (the
    /// caller memoises it per revision).
    pub fn load_envelope_input_with(
        &self,
        raw: &RawPositions,
        include_reference_legs: bool,
    ) -> LoadEnvelopeInput {
        let mut station_offsets = Vec::new();
        let mut station_comments_map = Vec::new();
        for line in self.lines() {
            for (index, station) in line.stations().enumerate() {
                let slot = StationRef::new(line.id, index as u32);
                if let Some(&[dx, dy, dz]) = line.hand_moves.get(&station) {
                    station_offsets.push((slot, (dx, dy, dz)));
                }
                if let Some(comment) = self
                    .stations
                    .get(&station)
                    .and_then(|s| s.comment.as_ref())
                    .filter(|c| !c.is_empty())
                {
                    station_comments_map.push((slot, comment.clone()));
                }
            }
        }
        LoadEnvelopeInput {
            state: self.project_state_wire_with(raw, include_reference_legs),
            station_offsets,
            station_comments_map,
        }
    }
}
