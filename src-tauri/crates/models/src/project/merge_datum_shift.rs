//! What merging two separately placed pieces does to the later-placed one
//! (#741), for the Merge dialog to say before the user confirms.
//!
//! The rule it mirrors lives in [`Project::merge_stations`]: the piece whose
//! placement datum comes LATER in `zero_datums` moves rigidly (horizontally)
//! so the two picked stations meet, and that datum is a GPS fix from then on.
//! `datum_ui_tests` pins the preview against the real merge.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{Project, SectionId, StationRef};

/// The piece a merge of two placed pieces moves, and how far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MergeDatumShift.ts")]
pub struct MergeDatumShift {
    /// Every section of the piece that moves, ascending.
    pub moving_section_ids: Vec<SectionId>,
    /// How far it moves, horizontal metres.
    pub shift_m: f64,
    /// The station of its placement datum, which becomes a GPS fix.
    pub datum_station: StationRef,
}

impl Project {
    /// What merging `station_a` with `station_b` would do to a placed piece,
    /// or `None` unless both stations sit in two different pieces that are
    /// each placed by their own zero datum. Read-only.
    pub fn merge_datum_shift(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> Option<MergeDatumShift> {
        let index_a = self.placement_datum_index(station_a.section_id)?;
        let index_b = self.placement_datum_index(station_b.section_id)?;
        if index_a == index_b {
            return None;
        }
        let (anchor, moving, moving_index) = if index_a < index_b {
            (station_a, station_b, index_b)
        } else {
            (station_b, station_a, index_a)
        };
        let (anchor_x, anchor_y) = self.compute_station_raw_xy(anchor)?;
        let (moving_x, moving_y) = self.compute_station_raw_xy(moving)?;
        let datum_station = self.resolve_key(&self.zero_datums.get(moving_index)?.station_ref)?;
        let mut moving_section_ids: Vec<SectionId> = self
            .get_merged_section_ids(moving.section_id)
            .into_iter()
            .collect();
        moving_section_ids.sort_unstable();
        Some(MergeDatumShift {
            moving_section_ids,
            shift_m: (anchor_x - moving_x).hypot(anchor_y - moving_y),
            datum_station,
        })
    }
}
