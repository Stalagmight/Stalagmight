//! Arguments and answers for Combine Lines: join two lines that meet end
//! to end into one line; other lines there stay attached as branches.
//! The inverse of Split Line Here, junctions included. Two ends meet when
//! they are merged, co-located, or the two ends of a loop closure; what has to
//! change for the combine (a shift, ties that become loop closures, converted
//! bearings) is part of the answer, so the dialog can say so first.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{LineId, StationRef};

/// Which end of a line a join sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LineEnd {
    /// Station 0.
    Start,
    /// The last station (`shots.len()`).
    End,
}

/// Which line's value the joined station keeps when both join stations
/// carry a different one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CombinePick {
    /// The kept line's value.
    Kept,
    /// The other line's value.
    Other,
    /// Both, joined into one text.
    Both,
}

/// The choices the Combine Lines dialog sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombineLinesOptions {
    /// Name of the combined line.
    pub name: String,
    /// `#rrggbb` colour of the combined line.
    pub color: String,
    /// When the other line has to be reversed to line up, also swap its
    /// left and right wall distances (the surveyor now faces the other way).
    pub swap_lr: bool,
    /// What the joined station keeps when both join stations have a comment.
    pub comment_pick: CombinePick,
    /// IGNORED: the joined station keeps the kept line's name (station
    /// names are unique per line, so there is no choice to make). Kept, with
    /// its shape, so a caller that still sends it and an older build reading
    /// the options both keep working.
    pub name_pick: CombinePick,
    /// Which ends to join, as the candidate the dialog showed named them.
    /// Needed when the two lines meet at more than one place (a loop of
    /// two lines): the station menu offers the end that was clicked. When
    /// absent, the join is the one the two-line query prefers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub join: Option<CombineJoinEnds>,
}

/// The two ends a combine joins, named from the kept line's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CombineJoinEnds {
    /// The kept line's end at the join.
    pub kept: LineEnd,
    /// The other line's end at the join.
    pub other: LineEnd,
}

/// How the two join stations are the same point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CombineJoinKind {
    /// The two stations are merged (one merge group).
    Merged,
    /// Unmerged, but within 1 mm of each other (the state a split leaves).
    CoLocated,
    /// A loop closure ties the two stations (or their merge groups): the
    /// survey says they are one point, and the misclosure is how far apart
    /// the two survey lines put them. One side moves by it.
    Closure,
}

/// What a rigid tie was before the combine turned it into a loop closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TornTieKind {
    /// A station merge.
    Merge,
    /// A loop closure made by snapping one station onto another (a `Move`
    /// closure, aligned and so rigid).
    SnappedClosure,
    /// A loop closure left from a snap whose two stations had since moved
    /// apart (a `Move` closure no longer aligned), which the combine brings
    /// back together and would otherwise snap again (#736).
    FormerSnappedClosure,
}

/// Why a rigid tie becomes a loop closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TornTieReason {
    /// It would tie the combined line to itself: the loop the combine
    /// closes now closes here.
    Loop,
    /// A zero datum holds the line on its far side, which stays while the
    /// moving side moves.
    ZeroDatum,
    /// A snapped closure at a join station: the joined station keeps the
    /// other join station's position as shown (its hand move), which takes
    /// the snap's two stations more than 1 cm apart (#736).
    JoinedStationMoves,
}

/// A rigid tie the combine turns into an ordinary loop closure, because it
/// would otherwise tie the combined line to itself or hold the moving side
/// in place. Named by its stations as they are before the combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TornTie {
    pub station_a: StationRef,
    pub station_b: StationRef,
    pub kind: TornTieKind,
    pub reason: TornTieReason,
}

/// What a tie the combine removes (#746), or the loop closure that replaces
/// one the combine drops (#749), was before the combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CombineRemovedKind {
    /// A station merge the combine would tear into a loop closure.
    Merge,
    /// A snapped (`Move`) closure, which the combine would make an ordinary
    /// one or leave snapped (#736).
    SnappedClosure,
    /// A loop closure left from a snap whose two stations had since moved
    /// apart, which the combine brings back together (#736).
    FormerSnappedClosure,
    /// A loop closure already there.
    LoopClosure,
}

/// A tie the combine removes because, once combined, the loop closure it is
/// or becomes would join the two ends of one shot and squeeze that shot to
/// zero length (#746, the Combine half of #737). The Closures Computed View
/// would only skip it, so the combine removes it in the same undo step, as
/// Delete with shift does. Named by its stations as they are BEFORE the
/// combine, the ids the user knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CombineRemovedClosure {
    pub station_a: StationRef,
    pub station_b: StationRef,
    pub kind: CombineRemovedKind,
}

/// What a loop closure the combine drops for another on the same two
/// stations was (#749): one the Closures Computed View does not adjust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CombineReplacedKind {
    /// An older kind of closure (proportional or equal distribution), which
    /// the Closures Computed View does not adjust.
    LegacyClosure,
    /// A snapped (aligned) closure: its two stations moved as one and shared
    /// one depth.
    SnappedClosure,
    /// A closure a snap made whose two stations have moved apart since (no
    /// longer aligned): neither snapped nor adjusted.
    FormerSnap,
}

/// A loop closure the combine drops because, combined, another loop closure
/// ties the same two stations and only that one is adjusted by the Closures
/// Computed View (#749): the combine keeps the one it adjusts. Both are named
/// by their stations as they are BEFORE the combine, the replacing one with
/// what it was then (a merge or snapped closure the combine tears, or a loop
/// closure already there). Two closures the view treats alike keep the first
/// silently; nothing changes for the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CombineReplacedClosure {
    pub station_a: StationRef,
    pub station_b: StationRef,
    pub kind: CombineReplacedKind,
    pub by_station_a: StationRef,
    pub by_station_b: StationRef,
    pub by_kind: CombineRemovedKind,
}

/// Lines merged at the join station whose hand move (a manual station
/// offset, from an older or hand-edited file) the joined station does not
/// keep (#755): they stay merged to the joined station and move with it,
/// onto the joined station as shown, which is not the shift of the side
/// they are on. Named as they are before the combine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombineJoinedMove {
    /// The join station whose hand move the joined station does not keep.
    pub station: StationRef,
    /// The lines merged at it, and the lines that hang off them.
    pub lines: Vec<LineId>,
    /// How far (metres, horizontal) they move.
    pub shift_m: f64,
}

/// Why the kept line's side is the one that moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum KeptMovesReason {
    /// A zero datum holds the other line's side.
    ZeroDatum,
    /// The other line's side holds the station the closure-adjusted view
    /// (Plan, Export) holds the survey at: moving it would shift the whole
    /// cave there by the gap.
    AdjustmentAnchor,
}

/// What combining changes beyond joining the shots, for ONE choice of the
/// kept line (the candidate carries one for each).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombinePlan {
    /// The kept line this plan is for.
    pub kept: LineId,
    /// The line whose side moves to close the join (the other line,
    /// or the kept one for the reason in `kept_moves_because`); `None` when
    /// the two join stations already coincide, to float dust
    /// (`Project::OFFSET_DUST_M`). Side-by-side ends a fraction of a
    /// millimetre apart move a side too (#740): the combined line hangs
    /// the other line's stations off the kept join station.
    pub moving_line: Option<LineId>,
    /// Why the kept line's side moves, when it is the one that does.
    pub kept_moves_because: Option<KeptMovesReason>,
    /// Every line that moves, the moving line first, then the lines
    /// still rigidly attached to it (not those in `moved_with_joined_station`,
    /// which move by a different amount).
    pub moved_lines: Vec<LineId>,
    /// How far (metres, horizontal) the moving side moves.
    pub shift_m: f64,
    /// Rigid ties that become ordinary loop closures, so the combined line
    /// is not tied to itself and the moving side is free to move.
    pub torn_ties: Vec<TornTie>,
    /// Degrees added to the other line's stored bearings so they read
    /// against the kept line's north reference (the combined line has
    /// one); `None` when the two references agree.
    pub azimuth_rotation_deg: Option<f64>,
    /// How far (metres) the moving join station's manual offset differs from
    /// the one the joined station keeps (the fixed side's), when more than
    /// 1 mm; `None` otherwise.
    pub join_offset_change_m: Option<f64>,
    /// The ties the combine removes because the loop closure they are or
    /// become would join the two ends of one shot (#746): exactly what the
    /// combine removes, from the same plan. `torn_ties` leaves them out.
    /// Absent when there are none (and for a blocked choice).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub closures_removed_across_one_shot: Option<Vec<CombineRemovedClosure>>,
    /// The loop closures the combine drops because a closure the Closures
    /// Computed View adjusts ends up on the same two stations (#749): exactly
    /// what the combine replaces, from the same plan. Absent when there are
    /// none (and for a blocked choice).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub closures_replaced: Option<Vec<CombineReplacedClosure>>,
    /// The lines that move with the joined station because they are merged
    /// at the join station whose hand move it does not keep (#755). Absent
    /// when there are none (and for a blocked choice).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub moved_with_joined_station: Option<CombineJoinedMove>,
    /// The stations the combine renames with this line kept (their names
    /// collide, or the kept line continues an earlier lettered combine), for
    /// the dialog to say so before the user confirms. Absent when no station
    /// is renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub station_renames: Option<CombineStationRenames>,
}

/// Why a combine renames stations (`station_naming::plan_combine_prefixes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CombineRenameReason {
    /// A station name of the other line (its join station left out) is also
    /// one of the kept line's.
    Collision,
    /// The kept line is lettered by an earlier combine (two or more letter
    /// families) and the other line is not, so it takes the next letter.
    Lettered,
}

/// How a combine letters one line's stations: how many, how many keep their
/// names, and the first and the last lettered station in the line's own
/// station order, before and after (`0` -> `A0` .. `3` -> `A3`). The first
/// and last are of the lettered stations only: a custom name the combine
/// keeps (`Junction` between `1` and `5`) is counted in `unchanged`, and a
/// name made unique by the Mid rule is listed in
/// [`CombineStationRenames::suffixed`], never implied by the range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CombineLineRenames {
    pub count: u32,
    /// The line's stations that keep their names (the other line's join
    /// station, whose name the combine retires, is in no count).
    pub unchanged: u32,
    pub first_from: String,
    pub first_to: String,
    pub last_from: String,
    pub last_to: String,
}

/// One station a combine renames, before and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CombineStationRename {
    pub from: String,
    pub to: String,
}

/// The station renames a combine makes, for one choice of the kept line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CombineStationRenames {
    pub reason: CombineRenameReason,
    /// The kept line's lettering; absent when it letters no station.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kept: Option<CombineLineRenames>,
    /// The other line's lettering; absent when it letters no station.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub other: Option<CombineLineRenames>,
    /// The other line's stations whose name the kept line also holds, made
    /// unique by the Mid rule (`Sump` -> `Sump-a`), in station order; absent
    /// when there are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub suffixed: Option<Vec<CombineStationRename>>,
    /// The joined station's name after the combine.
    pub join_name: String,
}

/// Whether two lines can be combined, and where. Read-only: answered by
/// `Project::combine_candidate` (from an end station) and
/// `Project::combine_candidate_for_lines` (from two selected lines).
///
/// `a` is the lower line id, which is the default kept line. What
/// depends on which line is kept (what moves, which ties tear) comes as one
/// plan per choice, so the dialog's swap control needs no second query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombineCandidate {
    pub line_a: LineId,
    pub line_b: LineId,
    pub end_a: LineEnd,
    pub end_b: LineEnd,
    /// The join station on each line.
    pub station_a: StationRef,
    pub station_b: StationRef,
    /// The join is a station merge (otherwise the two ends are co-located,
    /// or tied by a loop closure; see `join_kind`).
    pub merged: bool,
    /// How the two join stations are the same point.
    pub join_kind: CombineJoinKind,
    /// Both joins are at the same kind of end, so the line that is not kept
    /// is reversed to line up.
    pub reverses: bool,
    /// Why the two cannot be combined; `None` when they can.
    pub blocked_reason: Option<String>,
    /// Why `a` cannot be the KEPT line (the shot ids it would hand out),
    /// though keeping `b` may still work.
    pub blocked_keeping_a: Option<String>,
    /// Why `b` cannot be the kept line.
    pub blocked_keeping_b: Option<String>,
    pub shot_count_a: u32,
    pub shot_count_b: u32,
    /// The join stations' comments, set only when both have one and they
    /// differ (the dialog then asks which to keep).
    pub comment_a: Option<String>,
    pub comment_b: Option<String>,
    /// ALWAYS `None` now: the joined station keeps the kept line's name, so
    /// there is no name choice for the dialog to offer. Kept on the wire so
    /// the dialog's type stays stable until it drops the field.
    pub station_name_a: Option<String>,
    pub station_name_b: Option<String>,
    /// How far apart in depth (metres) the two join stations are, when they
    /// are not merged (co-located, or tied by a closure) and the gap is more
    /// than `COMBINE_DEPTH_NOTICE_M`. The joined station reads the average of
    /// the shot before and the shot after it, so each end moves by half the
    /// gap; the dialog says so. `None` for a merged join, which already
    /// averages.
    pub depth_difference_m: Option<f64>,
    /// Other lines merged at the join point. They stay attached to the
    /// joined station, which becomes a mid-line station of the combined
    /// line (a branch).
    pub branches: Vec<LineId>,
    /// Other lines with an end at the join point that is NOT merged to
    /// it. Combining leaves them as they are: a separate line end there.
    pub loose_ends: Vec<LineId>,
    /// Lines merged at the join point at a station in their middle: they
    /// pass through it, and stay merged to the joined station (a crossing).
    pub passing_through: Vec<LineId>,
    /// How many of the other line's shots share an identity (a shot uuid)
    /// with a shot elsewhere; the combine gives them new ones.
    pub repeated_shot_ids: u32,
    /// What changes when `a` is kept.
    pub plan_keeping_a: CombinePlan,
    /// What changes when `b` is kept.
    pub plan_keeping_b: CombinePlan,
}

/// How far apart (metres) the two join stations may be and still count as the
/// same point (side by side); also the bound on how far a co-located join
/// moves a side, which it does by the residual (#740, `combine_plan.rs`).
pub const COMBINE_TOLERANCE_M: f64 = 0.001;

/// Above this depth gap (metres) between two co-located, unmerged join
/// stations, the candidate reports it and the dialog shows it: the joined
/// station takes their average, so each end moves by half the gap. Half the
/// 0.1 m resolution depth gauges and dive computers record; a smaller gap is
/// below what the survey can tell apart.
pub const COMBINE_DEPTH_NOTICE_M: f64 = 0.05;

/// The most other line ends the station query lists at one point. Past it
/// the point is a pile (unplaced lines all start at the origin), not a
/// junction to choose from in a menu, and every partner costs a pair query;
/// the Sections panel still combines any two of them.
pub const MAX_COMBINE_PARTNERS: usize = 16;
