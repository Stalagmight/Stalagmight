//! Arguments and answers for Combine Sections: join two sections that meet at
//! one end station each into one section. The inverse of Split Section Here.

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
}

/// Whether two sections can be combined, and where. Read-only: answered by
/// `Project::combine_candidate` (from an end station) and
/// `Project::combine_candidate_for_sections` (from two selected sections).
///
/// `a` is the lower section id, which is the default kept section. Everything
/// here is symmetric in which section is kept, so the dialog's swap control
/// needs no second query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombineCandidate {
    pub section_a: SectionId,
    pub section_b: SectionId,
    pub end_a: SectionEnd,
    pub end_b: SectionEnd,
    /// The join station on each section.
    pub station_a: StationRef,
    pub station_b: StationRef,
    /// The join is a station merge (otherwise the two ends are co-located).
    pub merged: bool,
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
}

/// A section Combine Sections folded into another, kept so that dive-plan pins
/// stored against it still land on the same physical station.
///
/// A plan pin at a section end names one neighbouring shot and a side, and
/// that side is corrected by comparing the section's `reverse_parity` with the
/// parity recorded in the pin (`dive_plan::resolve_anchor`). Once the other
/// section's shots live in the kept section, that comparison reads the KEPT
/// section's parity, which is unrelated to the parity the pin recorded. The
/// pins live in the `.plnx` beside the project, outside the undo step, so they
/// cannot be re-keyed; instead the project records, per retired section, how
/// its shots' orientation relates to the section that absorbed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetiredSection {
    /// The section it was combined into.
    pub into: SectionId,
    /// True when its shots run against `into`'s parity: the retired section's
    /// effective parity is `into.reverse_parity ^ parity_flip`.
    pub parity_flip: bool,
}
