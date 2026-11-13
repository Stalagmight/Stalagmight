//! Arguments and answers for Combine Sections: join two sections that meet end
//! to end into one section; other sections there stay attached as branches.
//! The inverse of Split Section Here, junctions included. Two ends meet when
//! they are merged, co-located, or the two ends of a loop closure; what has to
//! change for the combine (a shift, ties that become loop closures, converted
//! bearings) is part of the answer, so the dialog can say so first.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{SectionId, StationRef};

/// Which end of a section a join sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SectionEnd {
    /// Station 0.
    Start,
    /// The last station (`shots.len()`).
    End,
}

/// Which section's value the joined station keeps when both join stations
/// carry a different one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CombinePick {
    /// The kept section's value.
    Kept,
    /// The other section's value.
    Other,
    /// Both, joined into one text.
    Both,
}

/// The choices the Combine Sections dialog sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombineSectionsOptions {
    /// Name of the combined section.
    pub name: String,
    /// `#rrggbb` colour of the combined section.
    pub color: String,
    /// When the other section has to be reversed to line up, also swap its
    /// left and right wall distances (the surveyor now faces the other way).
    pub swap_lr: bool,
    /// What the joined station keeps when both join stations have a comment.
    pub comment_pick: CombinePick,
    /// What the joined station keeps when both join stations have a name.
    pub name_pick: CombinePick,
    /// Which ends to join, as the candidate the dialog showed named them.
    /// Needed when the two sections meet at more than one place (a loop of
    /// two sections): the station menu offers the end that was clicked. When
    /// absent, the join is the one the two-section query prefers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub join: Option<CombineJoinEnds>,
}

/// The two ends a combine joins, named from the kept section's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CombineJoinEnds {
    /// The kept section's end at the join.
    pub kept: SectionEnd,
    /// The other section's end at the join.
    pub other: SectionEnd,
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
}

/// Why a rigid tie becomes a loop closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TornTieReason {
    /// It would tie the combined section to itself: the loop the combine
    /// closes now closes here.
    Loop,
    /// A zero datum holds the section on its far side, which stays while the
    /// moving side moves.
    ZeroDatum,
}

/// A rigid tie the combine turns into an ordinary loop closure, because it
/// would otherwise tie the combined section to itself or hold the moving side
/// in place. Named by its stations as they are before the combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TornTie {
    pub station_a: StationRef,
    pub station_b: StationRef,
    pub kind: TornTieKind,
    pub reason: TornTieReason,
}

/// What combining changes beyond joining the shots, for ONE choice of the
/// kept section (the candidate carries one for each).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombinePlan {
    /// The kept section this plan is for.
    pub kept: SectionId,
    /// The section whose side moves to close the join (the other section,
    /// or the kept one when the other is held by a zero datum); `None` when
    /// the two ends already coincide (within 1 mm).
    pub moving_section: Option<SectionId>,
    /// Every section that moves, the moving section first, then the sections
    /// still rigidly attached to it.
    pub moved_sections: Vec<SectionId>,
    /// How far (metres, horizontal) the moving side moves.
    pub shift_m: f64,
    /// Rigid ties that become ordinary loop closures, so the combined section
    /// is not tied to itself and the moving side is free to move.
    pub torn_ties: Vec<TornTie>,
    /// Degrees added to the other section's stored bearings so they read
    /// against the kept section's north reference (the combined section has
    /// one); `None` when the two references agree.
    pub azimuth_rotation_deg: Option<f64>,
    /// How far (metres) the moving join station's manual offset differs from
    /// the one the joined station keeps (the fixed side's), when more than
    /// 1 mm; `None` otherwise.
    pub join_offset_change_m: Option<f64>,
}

/// Whether two sections can be combined, and where. Read-only: answered by
/// `Project::combine_candidate` (from an end station) and
/// `Project::combine_candidate_for_sections` (from two selected sections).
///
/// `a` is the lower section id, which is the default kept section. What
/// depends on which section is kept (what moves, which ties tear) comes as one
/// plan per choice, so the dialog's swap control needs no second query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombineCandidate {
    pub section_a: SectionId,
    pub section_b: SectionId,
    pub end_a: SectionEnd,
    pub end_b: SectionEnd,
    /// The join station on each section.
    pub station_a: StationRef,
    pub station_b: StationRef,
    /// The join is a station merge (otherwise the two ends are co-located,
    /// or tied by a loop closure; see `join_kind`).
    pub merged: bool,
    /// How the two join stations are the same point.
    pub join_kind: CombineJoinKind,
    /// Both joins are at the same kind of end, so the section that is not kept
    /// is reversed to line up.
    pub reverses: bool,
    /// Why the two cannot be combined; `None` when they can.
    pub blocked_reason: Option<String>,
    /// Why `a` cannot be the KEPT section (the shot ids it would hand out),
    /// though keeping `b` may still work.
    pub blocked_keeping_a: Option<String>,
    /// Why `b` cannot be the kept section.
    pub blocked_keeping_b: Option<String>,
    pub shot_count_a: u32,
    pub shot_count_b: u32,
    /// The join stations' comments, set only when both have one and they
    /// differ (the dialog then asks which to keep).
    pub comment_a: Option<String>,
    pub comment_b: Option<String>,
    /// The join stations' names, set only when both have one and they differ.
    pub station_name_a: Option<String>,
    pub station_name_b: Option<String>,
    /// How far apart in depth (metres) the two join stations are, when they
    /// are not merged (co-located, or tied by a closure) and the gap is more
    /// than `COMBINE_DEPTH_NOTICE_M`. The joined station reads the average of
    /// the shot before and the shot after it, so each end moves by half the
    /// gap; the dialog says so. `None` for a merged join, which already
    /// averages.
    pub depth_difference_m: Option<f64>,
    /// Other sections merged at the join point. They stay attached to the
    /// joined station, which becomes a mid-section station of the combined
    /// section (a branch).
    pub branches: Vec<SectionId>,
    /// Other sections with an end at the join point that is NOT merged to
    /// it. Combining leaves them as they are: a separate section end there.
    pub loose_ends: Vec<SectionId>,
    /// Sections merged at the join point at a station in their middle: they
    /// pass through it, and stay merged to the joined station (a crossing).
    pub passing_through: Vec<SectionId>,
    /// How many of the other section's shots share an identity (a shot uuid)
    /// with a shot elsewhere; the combine gives them new ones.
    pub repeated_shot_ids: u32,
    /// What changes when `a` is kept.
    pub plan_keeping_a: CombinePlan,
    /// What changes when `b` is kept.
    pub plan_keeping_b: CombinePlan,
}
