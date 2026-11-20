//! The orphan-closure sweep (plan 5.5), run after topology edits (never on
//! load).
//!
//! MINIMAL LOCAL VERSION written by stream M1 because stream M2's (which owns
//! the sweep) was not on this branch yet. The integration keeps ONE: M2's,
//! with this file's callers (`edit.rs`'s epilogue) switched to it.
//!
//! In closure list order, so a later closure sees an earlier promotion:
//! - a closure whose two ends are one station is removed (#738);
//! - a closure that is the only connection between its two ends' pieces (no
//!   path over shots and junctions) is promoted: its two stations are merged
//!   (plan 5.1, through [`Project::fold_station`]), the movable piece moving
//!   onto the other: `a`'s piece moves onto `b` unless `a`'s piece is placed
//!   by a datum and `b`'s is not (then `b`'s moves). A promotion that would
//!   join two datum-placed pieces is refused and the closure stays (a
//!   closure between two placed pieces is legitimate);
//! - every other closure stays.

use crate::StationId;

use super::edit::CommentRule;
use super::{ClosureId, EditContext, LoopClosure, Project};

/// What the orphan-closure sweep did (plan 5.5).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SweepReport {
    /// Closures removed because their two ends became one station (#738), as
    /// they were before the sweep, in the order removed.
    pub removed: Vec<LoopClosure>,
    /// Closures promoted to a merge because they were the only connection
    /// between their two pieces, in the order promoted.
    pub promoted: Vec<Promotion>,
}

impl SweepReport {
    /// Whether the sweep changed nothing.
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.promoted.is_empty()
    }
}

/// One closure the sweep promoted: its two stations became one.
#[derive(Debug, Clone, PartialEq)]
pub struct Promotion {
    /// The closure as it was.
    pub closure: LoopClosure,
    /// The station that stayed (the placed side's, else `b`).
    pub survivor: StationId,
    /// The station merged into it (now an alias of the survivor).
    pub retired: StationId,
    /// How far the moving piece moved, horizontally.
    pub moved_by: [f64; 2],
}

impl Project {
    /// Run the orphan-closure sweep (module docs). Anchors must be settled
    /// (every component anchored) before it runs; each promotion settles its
    /// own.
    pub fn sweep_orphan_closures(&mut self) -> SweepReport {
        let mut report = SweepReport::default();
        let order: Vec<ClosureId> = self.closures.iter().map(|c| c.id).collect();
        for id in order {
            let Some(index) = self.closures.iter().position(|c| c.id == id) else {
                continue; // removed by an earlier promotion's fold
            };
            let closure = self.closures[index];
            if closure.a == closure.b {
                self.closures.remove(index);
                report.removed.push(closure);
                continue;
            }
            let topology = self.topology_arc();
            let (Some(ca), Some(cb)) = (
                topology.component_of(closure.a),
                topology.component_of(closure.b),
            ) else {
                continue;
            };
            if ca == cb {
                continue;
            }
            let held = self.placement_datums();
            let (held_a, held_b) = (held[ca].is_some(), held[cb].is_some());
            if held_a && held_b {
                continue;
            }
            let (stationary, moving) = if held_a {
                (closure.a, closure.b)
            } else {
                (closure.b, closure.a)
            };
            let before = self.anchor_snapshot();
            let mut work = self.clone();
            work.closures.remove(index);
            let Ok(fold) = work.fold_station(moving, stationary, CommentRule::Join) else {
                continue;
            };
            let ctx = EditContext::merge(stationary, moving).with_renamed(moving, stationary);
            work.settle_anchors(&before, &ctx);
            let moved_by = match (before.position(moving), work.raw_positions().xy(stationary)) {
                (Some(from), Some(to)) => [to[0] - from[0], to[1] - from[1]],
                _ => [0.0, 0.0],
            };
            *self = work;
            report.removed.extend(fold.removed_closures);
            report.promoted.push(Promotion {
                closure,
                survivor: stationary,
                retired: moving,
                moved_by,
            });
        }
        report
    }
}
