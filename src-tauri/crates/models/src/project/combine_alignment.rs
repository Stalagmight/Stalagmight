//! What a combine does to the snaps' alignment (#736): which `Move`
//! closures the combined project will hold aligned, worked out by the plan
//! before anything changes.
//!
//! `is_aligned` is a measurement (`Project::closure_alignment_stamp`): a
//! `Move` closure is aligned while its two stations are within 1 cm as shown
//! and the pair would not tie two placed merge components together. The app
//! re-measures every flag after each command, and since #736 a snap's flag
//! decides what the Closures Computed solve ties, and so which closures are
//! across one shot. A combine changes both terms: it moves a side, drops one
//! join station's hand move, tears merges and makes two sections one. So the
//! plan asks the stamp's own two questions of the combined project, for every
//! `Move` closure it leaves as one:
//!
//! - **A torn snap the combine re-closes** (apart before, within 1 cm after)
//!   where a snap would tie the combined section to itself (across the two
//!   sides, or with both ends on the combined section) is torn, as such a snap
//!   is: it becomes an ordinary closure and the dialog lists it. Left as it
//!   was, it would snap again inside the combined section. Re-closed anywhere
//!   else it is a snap again, and the plan's model of the joined survey
//!   (`Project::joined_sweep`) ties it as one.
//! - **A snap the combine pulls apart** (past 1 cm after: a dropped hand move
//!   takes one of its stations away from the other) is torn, as a snap the
//!   moving side pulls apart is.
//! - **A snap the combine leaves between two placed groups** (still within
//!   1 cm, but a torn merge or the join puts a zero datum on each side) is a
//!   torn snap from then on, as the stamp would make it after any edit; the
//!   model no longer ties it. Nothing moves for it. No combine reaches this
//!   one: a snap has both ends in one merge component (a closure between two
//!   is made a merge unless both are placed, and then it is never aligned),
//!   and the only merges a combine tears that leave a component split (the
//!   moving section's merges to its datum sides) take every snap between the
//!   parts with them. It is kept so the plan asks exactly what the stamp
//!   asks.
//!
//! The combine then re-measures every flag itself, before its across-one-shot
//! backstop, and checks the flags it changed are the ones the plan predicted
//! (`TiePlan::realigned`): the command's epilogue finds nothing left to
//! change.
//!
//! Cost: the geometry as shown is read once per candidate query
//! (`ClosureFacts`, `O(total shots)`), and only when a plan leaves a `Move`
//! closure; each plan then reads it in `O(1)` per closure, with one
//! union-find over the merges (`O(merges)`) for the double-anchor term.

use std::collections::{HashMap, HashSet};

use crate::{ClosureStrategy, SectionId, StationRef};

use super::combine_plan::{ClosureFacts, JoinPoint, TieId};
use super::union_find::UnionFind;
use super::Project;

/// The project's geometry as shown, and where its zero datums sit, read once
/// per candidate query ([`ClosureFacts`]).
pub(crate) struct Shown {
    local: HashMap<SectionId, Vec<(f64, f64)>>,
    hands: HashMap<StationRef, (f64, f64, f64)>,
    /// The section each resolvable zero datum (placement or GPS fix) sits
    /// on: what makes a merge component placed for the stamp.
    datum_sections: Vec<SectionId>,
}

impl Shown {
    pub(super) fn of(project: &Project) -> Self {
        Self {
            local: project.section_local_coords(),
            hands: project.resolved_station_offsets(),
            datum_sections: project
                .zero_datums
                .iter()
                .filter_map(|datum| project.resolve_key(&datum.station_ref))
                .map(|at| at.section_id)
                .collect(),
        }
    }

    /// Where `at` is on its section's chain: shots and section offset, no
    /// hand move.
    fn chain(&self, project: &Project, at: StationRef) -> Option<(f64, f64)> {
        let &(x, y) = self
            .local
            .get(&at.section_id)?
            .get(at.station_index as usize)?;
        let (ox, oy) = project
            .section_offsets
            .get(&at.section_id)
            .copied()
            .unwrap_or((0.0, 0.0));
        Some((x + ox, y + oy))
    }

    /// `at`'s hand move (manual station offset), or none.
    fn hand(&self, at: StationRef) -> (f64, f64) {
        self.hands.get(&at).map_or((0.0, 0.0), |h| (h.0, h.1))
    }
}

/// Where a plan puts every station, as shown.
pub(crate) struct Landing<'a> {
    pub join: &'a JoinPoint,
    /// The chain gap: the kept join station's chain position minus the other
    /// one's.
    pub gap: (f64, f64),
    /// How far each section that moves goes: the moving side by the shift,
    /// what moves with the joined station by that station's move (#755).
    pub moves: HashMap<SectionId, (f64, f64)>,
    /// Whether the joined station keeps the kept join station's hand move
    /// (else the other one's).
    pub kept_offset_wins: bool,
}

impl Landing<'_> {
    /// Where `at` (a station before the combine) is shown after it. The two
    /// join stations are the joined station: the kept join station's chain
    /// where the kept side puts it, with the hand move it keeps. The other
    /// section's stations hang off that chain, so they move by the chain gap
    /// plus the kept side's move (the shift when the other side moves, nothing
    /// when the kept side does, float dust when nothing moves). Every other
    /// station moves with its section.
    fn lands(&self, project: &Project, shown: &Shown, at: StationRef) -> Option<(f64, f64)> {
        let join = self.join;
        let kept_move = self.moves.get(&join.kept).copied().unwrap_or((0.0, 0.0));
        if at == join.join_k || at == join.join_o {
            let (x, y) = shown.chain(project, join.join_k)?;
            let hand = shown.hand(if self.kept_offset_wins {
                join.join_k
            } else {
                join.join_o
            });
            return Some((x + kept_move.0 + hand.0, y + kept_move.1 + hand.1));
        }
        let (x, y) = shown.chain(project, at)?;
        let hand = shown.hand(at);
        let (dx, dy) = if at.section_id == join.other {
            (self.gap.0 + kept_move.0, self.gap.1 + kept_move.1)
        } else {
            self.moves
                .get(&at.section_id)
                .copied()
                .unwrap_or((0.0, 0.0))
        };
        Some((x + hand.0 + dx, y + hand.1 + dy))
    }
}

/// The merge components of the combined project, by section: every merge
/// the plan does not tear, and the two sections as one; and which hold a
/// zero datum. What the stamp's double-anchor term reads there
/// (`StationComponents`, `Project::merge_would_double_anchor`).
struct ComponentsAfter {
    slot: HashMap<SectionId, usize>,
    sets: UnionFind,
    anchored: Vec<bool>,
}

impl ComponentsAfter {
    fn new(project: &Project, join: &JoinPoint, torn: &HashSet<TieId>, shown: &Shown) -> Self {
        let mut slot: HashMap<SectionId, usize> = HashMap::new();
        let mut slot_of = |id: SectionId| {
            let next = slot.len();
            *slot.entry(id).or_insert(next)
        };
        let pairs: Vec<(usize, usize)> = project
            .station_merges
            .iter()
            .enumerate()
            .filter(|(i, _)| !torn.contains(&TieId::Merge(*i)))
            .map(|(_, m)| {
                (
                    slot_of(m.station_a.section_id),
                    slot_of(m.station_b.section_id),
                )
            })
            .collect();
        let joined = (slot_of(join.kept), slot_of(join.other));
        let datums: Vec<usize> = shown.datum_sections.iter().map(|s| slot_of(*s)).collect();
        let mut sets = UnionFind::new(slot.len());
        for (a, b) in pairs.into_iter().chain([joined]) {
            sets.union(a, b);
        }
        let mut anchored = vec![false; slot.len()];
        for datum in datums {
            let root = sets.find(datum);
            anchored[root] = true;
        }
        Self {
            slot,
            sets,
            anchored,
        }
    }

    /// Would a rigid tie between `a` and `b` hold two placed components
    /// together? A section in no merge and holding no datum is a component
    /// of its own, unplaced.
    fn would_double_anchor(&mut self, a: SectionId, b: SectionId) -> bool {
        let (Some(&a), Some(&b)) = (self.slot.get(&a), self.slot.get(&b)) else {
            return false;
        };
        let (ra, rb) = (self.sets.find(a), self.sets.find(b));
        ra != rb && self.anchored[ra] && self.anchored[rb]
    }
}

/// What the combine does to the `Move` closures it leaves as they are,
/// beyond the ties its other rules tear (see the module docs).
#[derive(Debug, Default)]
pub(crate) struct Realignment {
    /// The ones it tears into ordinary closures, by id: a torn snap it
    /// re-closes where it would tie the combined section to itself, and a
    /// snap it pulls apart.
    pub torn: Vec<u64>,
    /// The ones it keeps whose alignment changes, by id: a torn snap it
    /// re-closes elsewhere (a snap again), and a snap it leaves between two
    /// placed groups (a torn snap again).
    pub flipped: HashSet<u64>,
}

impl Project {
    /// The [`Realignment`] of a plan that puts the stations where `landing`
    /// says and tears `torn` (merges and closures), with `side` the sections
    /// that move with the moving section and `fixed_side` the ones that stay
    /// with the fixed one: a tie between the two would tie the combined
    /// section to itself. `O(closures + merges)` once the query has read the
    /// geometry; nothing at all when the plan leaves no `Move` closure.
    pub(crate) fn plan_realignment(
        &self,
        landing: &Landing,
        torn: &HashSet<TieId>,
        side: &HashSet<SectionId>,
        fixed_side: &HashSet<SectionId>,
        facts: &ClosureFacts,
    ) -> Realignment {
        let join = landing.join;
        let mut left = facts
            .closures(self)
            .iter()
            .filter(|c| {
                c.strategy == ClosureStrategy::Move
                    && !join.is_join_tie(c.a, c.b)
                    && !torn.contains(&TieId::Closure(c.id))
            })
            .peekable();
        let mut realignment = Realignment::default();
        if left.peek().is_none() {
            return realignment;
        }
        let shown = facts.shown(self);
        let mut components = ComponentsAfter::new(self, join, torn, shown);
        let combined = |s: SectionId| s == join.kept || s == join.other;
        for closure in left {
            let gap = match (
                landing.lands(self, shown, closure.a),
                landing.lands(self, shown, closure.b),
            ) {
                (Some(p), Some(q)) => Some((p.0 - q.0).hypot(p.1 - q.1)).filter(|g| g.is_finite()),
                _ => None,
            };
            let close = gap.is_some_and(|gap| gap < Self::MERGE_ALIGNMENT_INVARIANT_M);
            let (a, b) = (closure.a.section_id, closure.b.section_id);
            let aligned = close && !components.would_double_anchor(a, b);
            match (closure.aligned, aligned) {
                (false, true) => {
                    let ties_itself = (side.contains(&a) && fixed_side.contains(&b))
                        || (side.contains(&b) && fixed_side.contains(&a))
                        || (combined(a) && combined(b));
                    if ties_itself {
                        realignment.torn.push(closure.id);
                    } else {
                        realignment.flipped.insert(closure.id);
                    }
                }
                (true, false) if close => {
                    realignment.flipped.insert(closure.id);
                }
                (true, false) => realignment.torn.push(closure.id),
                _ => {}
            }
        }
        realignment
    }
}
