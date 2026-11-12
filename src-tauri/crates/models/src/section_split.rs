//! Arguments for Split Section Here (#335): where to cut a section, and what
//! each half gets of the station the cut duplicates.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ShotId;

/// Where Split Section Here cuts a section.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SplitAt {
    /// At an existing mid-section station (not the first or last).
    Station { station_index: u32 },
    /// At a point `distance` metres along a shot. A station is inserted there
    /// first; within `MIN_SPLIT_CHILD_LENGTH_M` of either end the cut snaps to
    /// that end's station instead.
    Shot { shot_id: ShotId, distance: f64 },
}

/// Which half of the split keeps something that sat on the split station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SplitSide {
    /// The first half, which keeps the section's id.
    A,
    /// The new second half.
    B,
    /// Both halves get a copy. Not allowed for a zero datum.
    Both,
}

/// The choices the Split Section Here dialog sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SplitSectionOptions {
    pub name_a: String,
    pub name_b: String,
    /// `#rrggbb`.
    pub color_a: String,
    /// `#rrggbb`.
    pub color_b: String,
    /// Which half keeps a comment on the split station.
    pub comment_side: SplitSide,
    /// Which half keeps a zero datum on the split station: `A` or `B`.
    pub datum_side: SplitSide,
}
