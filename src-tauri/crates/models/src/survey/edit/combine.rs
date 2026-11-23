//! Combine Lines on the station graph (plan 5.4): join two lines that meet
//! end to end into one; other lines there stay attached as branches. The
//! inverse of Split Line Here.
//!
//! Two line ends **meet** (today's three join kinds,
//! [`crate::CombineJoinKind`]) when their stations are
//! - **merged**: one station (a junction of the two lines);
//! - **co-located**: two stations whose chain positions (the shown position
//!   less the line's hand move there) are within
//!   [`crate::COMBINE_TOLERANCE_M`];
//! - **closure**: tied by a loop closure; one side moves by the misclosure.
//!
//! The algorithm (plan 5.4, the 6 steps; `combine/plan.rs` decides,
//! `combine/apply.rs` does):
//! 1. the moving side M is the other line's side, unless it is held by a
//!    placement datum and the kept side is not, or (no datum deciding, and the
//!    Closures Computed View solves the joined survey) it holds the
//!    adjusted-view gauge (the component's placement station) and the kept
//!    side does not;
//! 2. when the two join stations are already connected (side by side in one
//!    piece, or across a closure inside one piece), M's line is detached from
//!    the junction on the path between them nearest M (it gets a new station
//!    there) and a `Batch` closure ties the two stations of that torn
//!    junction; every tear is a restoring detach (RS 4.4: the torn line takes
//!    back the archived station naming it, or keeps the junction while the
//!    other lines take theirs), and the tear closure archives the end that
//!    does not hold the identity the torn slot showed, so nothing visible
//!    changes and Remove Closure gives the line its own station back;
//! 3. when both sides are held by placement datums, M's line is also detached
//!    from every junction whose far part holds one (refused only when both
//!    lines carry a placement datum on their own stations, as today);
//! 4. the other line's join station is folded into the kept line's (it is
//!    archived in it with its native lines but the absorbed one, RS 4.4; the
//!    comment follows the dialog's pick when it becomes history; a closure
//!    between the two that archives the kept line's join keeps the other's
//!    identity instead, RS 4.1 step 4), the other line is reversed when
//!    needed and its shots join the kept line;
//! 5. snaps the move tears, and snaps across the two sides (which would tie
//!    the combined line to itself), become `Batch`; the fixed side's hand move
//!    wins at the join (today's rule), the other line's hand moves move to the
//!    kept line;
//! 6. a station the two lines share besides the join would make the combined
//!    line pass it twice: M's line is detached there into a `Batch` closure.
//!
//! Then, as today: closures the combine leaves across one shot are removed
//! (#746); of two closures left on the same two stations the one the solve
//! takes stays (#749), taking the dropped one's archive when it has none;
//! station names are letter-prefixed when they collide
//! (`plan_combine_prefixes`, on the names the lines show); the other line's
//! bearings turn to the kept line's north reference when the two differ.
//! The epilogue is every op's (RS 3): the A4 restore, the closure-archive
//! normalisation and rule L for what still repeats (on the combined line
//! and on third lines through a renamed junction).
//!
//! Everything the dialog shows ([`CombineReport`]) is read off the combine
//! itself, run on a copy: the preview is the same code as the command.

use crate::{
    CombineJoinKind, CombineStationRenames, KeptMovesReason, LineEnd, LineId, StationId, StationRef,
};

use super::super::{ClosureId, IdentityReport, LoopClosure};

mod apply;
mod plan;
mod touch;

/// Where two lines are joined, from the kept line's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CombineJoin {
    pub kept: LineId,
    pub other: LineId,
    pub kept_end: LineEnd,
    pub other_end: LineEnd,
    /// The join station on each line (one station for a merged join).
    pub kept_station: StationId,
    pub other_station: StationId,
    pub kind: CombineJoinKind,
}

/// What a tie the combine tears was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TornTieKind {
    /// A junction: one line is detached from a station it shared, which
    /// becomes a `Batch` closure between the station and the line's new one.
    Junction,
    /// A snap (an aligned `Move` closure) made an ordinary `Batch` closure.
    SnappedClosure,
}

/// Why a tie is torn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TornTieReason {
    /// It would tie the combined line to itself: the loop the combine closes
    /// now closes here.
    Loop,
    /// A placement datum holds its far side, which stays while the moving
    /// side moves.
    ZeroDatum,
    /// The two lines share the station besides the join: the combined line
    /// would pass it twice.
    PassesTwice,
    /// The move takes the snap's two stations apart.
    Moved,
}

/// A tie the combine tears into an ordinary loop closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TornTie {
    pub kind: TornTieKind,
    pub reason: TornTieReason,
    /// The station: the junction's (it stays on its other lines), or the
    /// snap's end `a`.
    pub station: StationId,
    /// The slot of `station` on the line that leaves it (a junction), or the
    /// snap's end `a`'s first slot, before the combine.
    pub slot: StationRef,
    /// The other side: the junction's slot on the line it stays shared with
    /// (towards the far side), or the snap's end `b`'s first slot.
    pub other_slot: StationRef,
    /// The snap's id, or the id of the `Batch` closure the junction became
    /// (`None` in a preview of a blocked choice).
    pub closure: Option<ClosureId>,
}

/// Lines that move with the joined station because they sit at the join
/// station whose hand move the joined station does not keep (#755).
#[derive(Debug, Clone, PartialEq)]
pub struct JoinedMove {
    pub station: StationId,
    pub lines: Vec<LineId>,
    pub shift_m: f64,
}

/// A loop closure the combine removed because it would join the two ends of
/// one shot (#746).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemovedClosure {
    /// The closure as it was when removed.
    pub closure: LoopClosure,
    /// The tie it came from (an index into [`CombineReport::torn_ties`]), when
    /// the combine made it.
    pub torn_tie: Option<usize>,
}

/// A loop closure the combine dropped because another one the solve takes
/// ties the same two stations, while it is not one itself (#749).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplacedClosure {
    pub dropped: LoopClosure,
    pub by: LoopClosure,
}

/// What combining does beyond joining the shots, for one choice of the kept
/// line (today's `CombinePlan`, by station ids). The preview and the command
/// return the same facts: the preview runs the combine on a copy.
#[derive(Debug, Clone, PartialEq)]
pub struct CombineReport {
    pub kept: LineId,
    /// The line whose side moves; `None` when the join stations already
    /// coincide, to float dust.
    pub moving_line: Option<LineId>,
    pub kept_moves_because: Option<KeptMovesReason>,
    /// Every line moved by the shift, the moving line first.
    pub moved_lines: Vec<LineId>,
    /// How far the moving side moves, horizontally.
    pub shift_m: f64,
    pub torn_ties: Vec<TornTie>,
    /// Degrees added to the other line's bearings (north references differ).
    pub azimuth_rotation_deg: Option<f64>,
    /// How far apart the two join stations' hand moves are, when more than
    /// the tolerance (not merged).
    pub join_offset_change_m: Option<f64>,
    pub closures_removed_across_one_shot: Vec<RemovedClosure>,
    pub closures_replaced: Vec<ReplacedClosure>,
    pub moved_with_joined_station: Option<JoinedMove>,
    pub station_renames: Option<CombineStationRenames>,
    /// The closure between the two join stations that archived the kept
    /// line's: the joined station shows the other line's join station (RS
    /// 4.1 step 4), and the dialog says why.
    pub join_keep_forced_by: Option<ClosureId>,
    /// What the combine's epilogue did to identities (RS 3): restores,
    /// dropped closure archives, rule-L letters.
    pub identities: IdentityReport,
    /// Stations under another id after the combine, `(old, new)`: the
    /// forced join identity and the restores.
    pub reidentified: Vec<(StationId, StationId)>,
}

impl CombineReport {
    /// The report of a choice that cannot be made (a blocked candidate).
    fn empty(kept: LineId) -> Self {
        Self {
            kept,
            moving_line: None,
            kept_moves_because: None,
            moved_lines: Vec::new(),
            shift_m: 0.0,
            torn_ties: Vec::new(),
            azimuth_rotation_deg: None,
            join_offset_change_m: None,
            closures_removed_across_one_shot: Vec::new(),
            closures_replaced: Vec::new(),
            moved_with_joined_station: None,
            station_renames: None,
            join_keep_forced_by: None,
            identities: IdentityReport::default(),
            reidentified: Vec::new(),
        }
    }
}

/// Whether two lines can be combined, and where (today's
/// `CombineCandidate`, by station ids). `a` is the lower line id.
#[derive(Debug, Clone, PartialEq)]
pub struct CombineCandidate {
    pub line_a: LineId,
    pub line_b: LineId,
    pub end_a: LineEnd,
    pub end_b: LineEnd,
    /// The join station on each line (equal for a merged join).
    pub station_a: StationId,
    pub station_b: StationId,
    pub slot_a: StationRef,
    pub slot_b: StationRef,
    pub join_kind: CombineJoinKind,
    /// Both joins are the same kind of end: the line not kept is reversed.
    pub reverses: bool,
    pub blocked_reason: Option<String>,
    pub blocked_keeping_a: Option<String>,
    pub blocked_keeping_b: Option<String>,
    pub shot_count_a: u32,
    pub shot_count_b: u32,
    /// The join stations' comments, when both have one and they differ.
    pub comment_a: Option<String>,
    pub comment_b: Option<String>,
    /// The depth gap between two join stations that are not one station,
    /// when above `COMBINE_DEPTH_NOTICE_M`.
    pub depth_difference_m: Option<f64>,
    /// Other lines through a join station with an end there (they stay
    /// attached to the joined station).
    pub branches: Vec<LineId>,
    /// Other lines with an end at the join point that are not on a join
    /// station: left as they are.
    pub loose_ends: Vec<LineId>,
    /// Other lines through a join station at a station in their middle.
    pub passing_through: Vec<LineId>,
    pub plan_keeping_a: CombineReport,
    pub plan_keeping_b: CombineReport,
}
