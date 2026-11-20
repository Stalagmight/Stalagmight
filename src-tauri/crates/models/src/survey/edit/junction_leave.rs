//! What a delete does with the closures a line kept at a junction it leaves
//! (plan deviation S13).
//!
//! Today (0.6.0) every line had its own member at a merged station, and a
//! delete removed the line's member there: its merges went with it (the
//! other members were re-stitched to each other by a replacement merge only
//! when they were co-located and not already connected), and each of its
//! loop closures was re-anchored onto a remaining member, so that when the
//! delete broke the loop the closure closed, its promotion moved the
//! junction's side onto the closure's far end. The line's new end station was
//! never merged to the junction (no "slide": the line simply left it).
//!
//! On the station graph a junction is one station, which stays for the other
//! lines, and its closure ends keep the line they belonged to as `via`. So
//! for each closure end on a station the edited line left, attached via that
//! line:
//!
//! - a `Tie` (a 0.6.0 merge the migration demoted: a cycle or a same-line
//!   merge) leaves with the line, as the merge did, unless 0.6.0 would have
//!   re-stitched its two ends: they were co-located before the edit (within
//!   [`ALIGNMENT_TOLERANCE_M`]) and the edit leaves them in different pieces.
//!   Kept, it is the orphan-closure sweep's to promote, which moves nothing
//!   (its far end is the one retired, so the junction keeps its id);
//! - any other closure stays, and that end is the one whose piece moves if
//!   the sweep promotes the closure (unless its piece is held by a datum and
//!   the other is not).
//!
//! Without this, a demoted merge left as the only bridge was promoted and its
//! piece moved by the 0.6.0 misclosure, and a promoted closure moved its far
//! piece instead of the junction's (wire equivalence seeds 5 and 23).

use crate::{LineId, StationId};

use super::super::{
    ClosureEnd, ClosureId, ClosureStrategy, LoopClosure, Project, RawPositions,
    ALIGNMENT_TOLERANCE_M,
};

/// What [`Project::leave_junctions`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct LeftJunctions {
    /// `Tie`s removed with the line's slot, as they were, in list order.
    pub(super) ties: Vec<LoopClosure>,
    /// The ends of the remaining closures that were on the line's slot: the
    /// end whose piece moves if the sweep promotes the closure.
    pub(super) moving_ends: Vec<(ClosureId, ClosureEnd)>,
}

impl Project {
    /// The raw layout before the edit, when a `Tie` on one of `stations` is
    /// attached via `line` (what [`Self::leave_junctions`] measures); `None`
    /// otherwise, so an ordinary delete lays nothing out. Call it before the
    /// mutation.
    pub(super) fn member_tie_layout(
        &self,
        line: LineId,
        stations: &[StationId],
    ) -> Option<RawPositions> {
        let on = |station: StationId, via: Option<LineId>| {
            via == Some(line) && stations.contains(&station)
        };
        self.closures
            .iter()
            .any(|c| c.strategy == ClosureStrategy::Tie && (on(c.a, c.a_via) || on(c.b, c.b_via)))
            .then(|| self.raw_positions())
    }

    /// After `line` left `stations` (the delete's mutation, the stations on
    /// no line already deleted, the `via` hints not yet cleared): the module
    /// rules for every closure end on one of them attached via `line`.
    /// `layout` is [`Self::member_tie_layout`]'s.
    pub(super) fn leave_junctions(
        &mut self,
        line: LineId,
        stations: &[StationId],
        layout: Option<&RawPositions>,
    ) -> LeftJunctions {
        let topology = self.topology_arc();
        let left = |station: StationId, via: Option<LineId>| {
            via == Some(line)
                && stations.contains(&station)
                && topology.slot_on(station, line).is_none()
        };
        let co_located =
            |a: StationId, b: StationId| match layout.and_then(|l| Some((l.xy(a)?, l.xy(b)?))) {
                Some(([ax, ay], [bx, by])) => (ax - bx).hypot(ay - by) < ALIGNMENT_TOLERANCE_M,
                None => false,
            };
        let mut out = LeftJunctions::default();
        let mut kept = Vec::with_capacity(self.closures.len());
        for closure in std::mem::take(&mut self.closures) {
            let ends = [
                (ClosureEnd::A, left(closure.a, closure.a_via)),
                (ClosureEnd::B, left(closure.b, closure.b_via)),
            ];
            if ends.iter().all(|(_, on)| !on) {
                kept.push(closure);
                continue;
            }
            let tie = closure.strategy == ClosureStrategy::Tie;
            let re_stitched =
                co_located(closure.a, closure.b) && !topology.connected(closure.a, closure.b);
            if tie && !re_stitched {
                out.ties.push(closure);
                continue;
            }
            // The junction's end moves; a re-stitched `Tie` moves nothing, so
            // its far end goes and the junction keeps its id for its lines.
            let moving = match ends {
                [(_, true), (_, false)] => Some(if tie { ClosureEnd::B } else { ClosureEnd::A }),
                [(_, false), (_, true)] => Some(if tie { ClosureEnd::A } else { ClosureEnd::B }),
                _ => None,
            };
            if let Some(end) = moving {
                out.moving_ends.push((closure.id, end));
            }
            kept.push(closure);
        }
        self.closures = kept;
        out
    }
}
