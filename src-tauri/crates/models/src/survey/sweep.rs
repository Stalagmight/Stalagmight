//! The orphan-closure sweep (plan 5.5), run after topology edits (merge,
//! detach, datum removal, every delete), never on load.
//!
//! One pass over the closures in list order (after a delete, the closures on
//! a junction its line left come first), so a later closure sees what an
//! earlier one did:
//!
//! - a closure whose two ends are one station closes nothing and is removed
//!   (#738);
//! - a closure whose ends are in different pieces is their sole connection,
//!   i.e. a merge, not a loop: it is **promoted**, the two stations merged
//!   (plan 5.1) with the movable piece moving onto the other (plan 4.2: the
//!   piece of end `a` moves unless `a`'s piece is placed and `b`'s is not;
//!   a delete names the end that moves instead when it left that end's
//!   junction, `edit/junction_leave.rs`). Between two placed pieces a
//!   promotion would double-anchor, so the closure is kept (invariant 7
//!   allows it there).
//!
//! A promotion is a merge (RS 5.4): geometry first, exactly as above, then
//! the identity. It keeps the closure's kept end when the closure archives
//! one (the other end becomes the merge record, RS 4.1 step 4); else the
//! kept end of another closure between the two that archives one; else the
//! survivor, unless the survivor's name would repeat on the retired end's
//! lines and the retired end's would not. When the identity kept is the
//! retired end's, the merged node is re-identified (RS 4.1 step 3, its
//! pre-edit id told to the anchor algorithm), so the layout is the same
//! either way. Names a promotion makes repeat are lettered by rule L.
//!
//! A promotion only joins pieces that were apart, so a closure the pass kept
//! as a loop can never become one station later in the pass, and one pass
//! settles everything (today's argument). Each promotion runs the anchor
//! algorithm on its own (snapshot, merge, settle), so the next closure is
//! judged on the geometry the previous promotion left. The sweep does not
//! validate: the command that runs it does.

use crate::StationId;

use super::{ClosureEnd, ClosureId, EditContext, LetteredStation, LoopClosure, Project};

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
    /// The names the promotions made repeat, lettered by rule L (RS 5.3: a
    /// path without a dialog).
    pub lettered: Vec<LetteredStation>,
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
    /// The station whose piece stayed (geometry, plan 4.2).
    pub survivor: StationId,
    /// The station whose piece moved onto it (geometry).
    pub retired: StationId,
    /// The station kept (RS 5.4): the identity the merged node shows, and
    /// the merged node's id after the promotion (`survivor` or `retired`).
    pub kept: StationId,
    /// The other one, archived in the merged node (a record).
    pub archived: StationId,
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
        self.sweep_orphan_closures_moving(&[])
    }

    /// [`Self::sweep_orphan_closures`] for a delete that took its line off
    /// junctions (`edit/junction_leave.rs`): the closures named in `moving`
    /// are judged first (in list order, then the rest), as today's delete
    /// promoted the closures on the removed member before its sweep, and the
    /// named end is the one whose piece moves on promotion (unless it is held
    /// by a datum and the other end's piece is not). A closure named by both
    /// ends moves end `a`'s, as an unnamed one does.
    pub(super) fn sweep_orphan_closures_moving(
        &mut self,
        moving: &[(ClosureId, ClosureEnd)],
    ) -> SweepReport {
        let mut report = SweepReport::default();
        let original = self.closures.clone();
        // The state before the first promotion, for rule L's "new on the
        // line" (cloned only when a promotion happens).
        let mut before_promotions: Option<Project> = None;
        let mut fates = vec![Fate::Keep; self.closures.len()];
        let named = |c: &LoopClosure| moving.iter().any(|(id, _)| *id == c.id);
        let (first, rest): (Vec<usize>, Vec<usize>) =
            (0..original.len()).partition(|&i| named(&original[i]));
        for position in first.into_iter().chain(rest) {
            let fate = &mut fates[position];
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
            let b_moves = moving.contains(&(closure.id, ClosureEnd::B))
                && !moving.contains(&(closure.id, ClosureEnd::A));
            let (survivor, retired) = match (b_moves, held_a, held_b) {
                (true, _, false) | (false, true, _) => (closure.a, closure.b),
                _ => (closure.b, closure.a),
            };
            let keep = self.promotion_keep(&closure, survivor, retired);
            let before = self.anchor_snapshot();
            let pre = before_promotions.is_none().then(|| self.clone());
            let survivor_native = self.native_lines(survivor);
            if self.merge_ids(survivor, retired).is_err() {
                continue;
            }
            if pre.is_some() {
                before_promotions = pre;
            }
            let mut ctx = EditContext::merge(survivor, retired).with_renamed(retired, survivor);
            if keep == retired {
                // Geometry first, then the identity (RS 4.1 step 3).
                self.reidentify_merged(survivor, retired, survivor_native);
                ctx = ctx.with_reidentified(&[(survivor, retired)]);
            }
            self.settle_anchors(&before, &ctx);
            *fate = Fate::Promoted;
            let archived = if keep == retired { survivor } else { retired };
            report.promoted.push(Promotion {
                closure: original[position],
                position,
                survivor,
                retired,
                kept: keep,
                archived,
            });
        }
        if let Some(before) = &before_promotions {
            report.lettered = self.letter_collisions(before);
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

    /// Which end a promotion of `closure` keeps (module docs): the closure's
    /// kept end, else the kept end of another closure between the two that
    /// archives one, else `survivor` unless its name would repeat on
    /// `retired`'s lines and `retired`'s would not repeat on `survivor`'s.
    fn promotion_keep(
        &self,
        closure: &LoopClosure,
        survivor: StationId,
        retired: StationId,
    ) -> StationId {
        if let Some((_, kept)) = closure.archive() {
            return kept;
        }
        if let Some((_, kept)) = self.forced_merge_keep(survivor, retired) {
            return kept;
        }
        if self.name_repeats_on(survivor, retired) && !self.name_repeats_on(retired, survivor) {
            retired
        } else {
            survivor
        }
    }

    /// Whether the name `kept` shows is shown by another identity (not the
    /// one `other` shows) on a line through `other`: what keeping `kept` in
    /// a merge of the two would show twice there.
    fn name_repeats_on(&self, kept: StationId, other: StationId) -> bool {
        let (identity, gone) = (self.shown(kept), self.shown(other));
        let Some(name) = self.shown_name(kept) else {
            return false;
        };
        let topology = self.topology();
        topology.lines_of(other).into_iter().any(|line| {
            topology.chain(line).unwrap_or(&[]).iter().any(|&node| {
                let shown = self.shown(node);
                shown != identity && shown != gone && self.shown_name(node) == Some(name)
            })
        })
    }
}
