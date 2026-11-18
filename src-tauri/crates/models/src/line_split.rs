//! Arguments for Split Line Here (#335): where to cut a line, and what
//! each half gets of the station the cut duplicates.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ClosurePair, ShotId};

/// Where Split Line Here cuts a line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SplitAt {
    /// At an existing mid-line station (not the first or last).
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
    /// The first half, which keeps the line's id.
    A,
    /// The new second half.
    B,
    /// Both halves get a copy. Not allowed for a zero datum.
    Both,
}

/// What Split Line Here would do beyond the cut, which the dialog shows
/// before the user confirms (`AppState::split_line_preview`).
///
/// A loop closure from before the cut to past it, with nothing else tying the
/// two halves, is left as their only tie. A closure must close a loop, so the
/// split turns it into a merge, which moves one half by the loop's misclosure.
/// A second closure that tied the same two points is then a closure between
/// merged stations, which closes nothing, and the split removes it (#738).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SplitPreview {
    /// How many loop closures the split turns into merges.
    pub closures_to_merges: u32,
    /// The half that moves to close them, if one moves.
    pub moved_side: Option<SplitSide>,
    /// How far it moves, in metres (0 when nothing moves).
    pub moved_m: f64,
    /// The loop closures the split removes because the merges it makes put
    /// their two stations on one point (one merge group), in the order the
    /// split removes them. Named by their stations BEFORE the split, the ids
    /// the user created them with. The preview runs the split's own sweep on a
    /// copy, so this is exactly what the split removes.
    pub removed_closures: Vec<ClosurePair>,
    /// The loop closures the split removes whose two stations were one point
    /// before it: the sweep the split ends in cleans up a closure an undo
    /// restored from a history recorded before #738, wherever it is. Named the
    /// same way. Empty in any project this version wrote.
    pub removed_already_one_point: Vec<ClosurePair>,
}

/// The choices the Split Line Here dialog sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SplitLineOptions {
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

impl SplitLineOptions {
    /// Why these choices cannot make a split, if they cannot: a half with no
    /// name (empty or only whitespace), or a zero datum asked to stay on both
    /// halves. The model, the command and the dialog's preview all refuse on
    /// it, so they agree.
    pub fn refusal(&self) -> Option<String> {
        if self.name_a.trim().is_empty() || self.name_b.trim().is_empty() {
            return Some("Each half of a split needs a name".to_string());
        }
        (self.datum_side == SplitSide::Both)
            .then(|| "A zero datum can stay on only one half of a split".to_string())
    }
}
