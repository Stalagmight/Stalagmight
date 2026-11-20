//! The orphan-closure sweep (plan 5.5), run after topology edits (merge,
//! detach, datum removal, every delete), never on load.
//!
//! One pass over the closures in list order, so a later closure sees what an
//! earlier one did:
//!
//! - a closure whose two ends are one station closes nothing and is removed
//!   (#738);
//! - a closure whose ends are in different pieces is their sole connection,
//!   i.e. a merge, not a loop: it is **promoted**, the two stations merged
//!   (plan 5.1) with the movable piece moving onto the other (plan 4.2: the
//!   piece of end `a` moves unless `a`'s piece is placed and `b`'s is not).
//!   Between two placed pieces a promotion would double-anchor, so the
//!   closure is kept (invariant 7 allows it there).
//!
//! A promotion only joins pieces that were apart, so a closure the pass kept
//! as a loop can never become one station later in the pass, and one pass
//! settles everything (today's argument). Each promotion runs the anchor
//! algorithm on its own (snapshot, merge, settle), so the next closure is
//! judged on the geometry the previous promotion left. The sweep does not
//! validate: the command that runs it does.

use crate::StationId;

use super::ops_merge::CommentRule;
use super::{ClosureId, EditContext, LoopClosure, Project};

/// What the sweep did, as warning data for the command's result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SweepReport {
    /// Closures removed because their two ends are one station, as they were
    /// stored when the sweep began, in list order.
    pub removed: Vec<LoopClosure>,
    /// Where each of `removed` sat in the closure list when the sweep began.
    pub removed_positions: Vec<usize>,
    /// Closures promoted to merges, in promotion order.
    pub promoted: Vec<Promotion>,
    /// Sole-bridge closures kept because both pieces are placed.
    pub kept_between_placed: Vec<ClosureId>,
}

impl SweepReport {
    /// Whether the sweep changed nothing.
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.promoted.is_empty()
    }
}

/// One closure the sweep promoted to a merge.
#[derive(Debug, Clone, PartialEq)]
pub struct Promotion {
    /// The closure as it was stored when the sweep began.
    pub closure: LoopClosure,
    /// Where it sat in the closure list when the sweep began.
    pub position: usize,
    /// The station that stayed (the stationary piece's end).
    pub survivor: StationId,
    /// The moving piece's end, now an alias of the survivor.
    pub retired: StationId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    Keep,
    Removed,
    Promoted,
}

impl Project {
    /// Remove closures whose ends are one station and promote sole-bridge
    /// closures to merges (see the module docs). Owned by M2; M1's deletes
    /// call it after settling their own anchors.
    pub fn sweep_orphan_closures(&mut self) -> SweepReport {
        let mut report = SweepReport::default();
        let original = self.closures.clone();
        let mut fates = vec![Fate::Keep; self.closures.len()];
        for (position, fate) in fates.iter_mut().enumerate() {
            let closure = self.closures[position];
            if closure.a == closure.b {
                *fate = Fate::Removed;
                continue;
            }
            let topology = self.topology();
            let (Some(ca), Some(cb)) = (
                topology.component_of(closure.a),
                topology.component_of(closure.b),
            ) else {
                // A dangling end is the validator's to report, not ours to fix.
                continue;
            };
            if ca == cb {
                continue;
            }
            let placed = self.placement_datums();
            let (held_a, held_b) = (placed[ca].is_some(), placed[cb].is_some());
            if held_a && held_b {
                report.kept_between_placed.push(closure.id);
                continue;
            }
            let (survivor, retired) = if held_a {
                (closure.a, closure.b)
            } else {
                (closure.b, closure.a)
            };
            let before = self.anchor_snapshot();
            if self.merge_ids(survivor, retired, CommentRule::Join).is_err() {
                continue;
            }
            let ctx = EditContext::merge(survivor, retired).with_renamed(retired, survivor);
            self.settle_anchors(&before, &ctx);
            *fate = Fate::Promoted;
            report.promoted.push(Promotion {
                closure: original[position],
                position,
                survivor,
                retired,
            });
        }
        if fates.iter().any(|fate| *fate != Fate::Keep) {
            let closures = std::mem::take(&mut self.closures);
            for (position, (closure, fate)) in closures.into_iter().zip(fates).enumerate() {
                match fate {
                    Fate::Keep => self.closures.push(closure),
                    Fate::Removed => {
                        report.removed_positions.push(position);
                        report.removed.push(original[position]);
                    }
                    Fate::Promoted => {}
                }
            }
        }
        report
    }
}
