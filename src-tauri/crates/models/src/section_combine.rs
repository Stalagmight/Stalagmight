//! Arguments and the eligibility report for Combine Sections (#707): which
//! ends of two sections meet, whether they may become one section, and what
//! the joined station keeps.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{SectionId, SplitSide};

/// Which end of the kept section meets which end of the other one. The kept
/// section keeps its direction; the other is reversed first when its matching
/// end faces the wrong way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CombinePairing {
    /// The other section is appended.
    KeptEndOtherStart,
    /// The other section is reversed, then appended.
    KeptEndOtherEnd,
    /// The other section is prepended.
    KeptStartOtherEnd,
    /// The other section is reversed, then prepended.
    KeptStartOtherStart,
}

impl CombinePairing {
    pub const ALL: [Self; 4] = [
        Self::KeptEndOtherStart,
        Self::KeptEndOtherEnd,
        Self::KeptStartOtherEnd,
        Self::KeptStartOtherStart,
    ];

    /// Whether the other section has to be reversed before it is joined.
    pub fn reverses_other(self) -> bool {
        matches!(self, Self::KeptEndOtherEnd | Self::KeptStartOtherStart)
    }

    /// Whether the other section's shots go in front of the kept section's.
    pub fn prepends(self) -> bool {
        matches!(self, Self::KeptStartOtherEnd | Self::KeptStartOtherStart)
    }

    /// The station indices that meet, as `(kept, other)`, for sections of
    /// `kept_shots` and `other_shots` shots, before any reverse.
    pub fn ends(self, kept_shots: u32, other_shots: u32) -> (u32, u32) {
        match self {
            Self::KeptEndOtherStart => (kept_shots, 0),
            Self::KeptEndOtherEnd => (kept_shots, other_shots),
            Self::KeptStartOtherEnd => (0, other_shots),
            Self::KeptStartOtherStart => (0, 0),
        }
    }
}

/// Metres of the combined section credited to one surveyor on one date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CreditRun {
    /// `None`: unknown.
    pub surveyor: Option<String>,
    /// `YYYY-MM-DD`, or `None` for unknown.
    pub date: Option<String>,
    pub length_m: f64,
}

/// Whether two sections can be combined, and what the dialog has to ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombineCandidate {
    /// The section that keeps its id and direction: the lower id.
    pub kept: SectionId,
    pub other: SectionId,
    pub pairing: CombinePairing,
    /// Whether the other section is reversed (left and right swapped) first.
    pub reverse_other: bool,
    /// Why the combine is refused, or `None` when it is allowed.
    pub blocked: Option<String>,
    /// Both joined stations carry a comment, and they differ.
    pub comment_conflict: bool,
    /// Both joined stations carry a name, and they differ.
    pub name_conflict: bool,
    /// Both joined stations carry a manual offset, and they differ.
    pub offset_conflict: bool,
    /// The combined section's credit, summed by surveyor and date in shot
    /// order.
    pub credit: Vec<CreditRun>,
}

/// The choices the Combine Sections dialog sends. For each side, `A` is the
/// kept section and `B` the other one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CombineOptions {
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    /// Which joined station's comment survives; `Both` keeps the kept one, a
    /// newline, then the other.
    pub comment_side: SplitSide,
    /// Which joined station's name survives: `A` or `B`.
    pub name_side: SplitSide,
    /// Which joined station's manual offset survives: `A` or `B`.
    pub offset_side: SplitSide,
}
