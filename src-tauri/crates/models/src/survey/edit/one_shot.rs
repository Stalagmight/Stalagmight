//! Loop closures that tie the two ends of one shot together (#737): today's
//! knot rule on the station graph. A minimal local version for the removals
//! the delete and combine operations make; stream M3 owns the skip set the
//! solve and the warnings read, and the two must be the same rule.
//!
//! The rule, in closure list order: every closure the solve ties (an
//! eligible `Batch` / `LeastSquaresWeighted` one, an aligned snap, a `Tie`)
//! unites its two ends' knots, unless uniting them would put the two ends of
//! some shot into one knot that were not in one knot before. Such a closure
//! is skipped and unites nothing, so it never changes what a later closure
//! sees. A junction is one station already (today's merges, united first
//! without the check).

use std::collections::{BTreeMap, BTreeSet};

use super::super::{ClosureId, ClosureStrategy, LoopClosure, Project, RawPositions};

/// Union-find over the dense stations plus the shot adjacency between knots
/// (keyed by current roots, symmetric).
struct Knots {
    parent: Vec<usize>,
    neighbours: BTreeMap<usize, BTreeSet<usize>>,
}

impl Knots {
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    /// Would uniting the knots of `a` and `b` collapse a shot? `None` when
    /// they are one knot already (uniting changes nothing).
    fn collapses(&mut self, a: usize, b: usize) -> Option<bool> {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return None;
        }
        Some(self.neighbours.get(&ra).is_some_and(|n| n.contains(&rb)))
    }

    /// Unite the knots of `a` and `b` (not adjacent by a shot).
    fn unite(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (keep, gone) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[gone] = keep;
        let moved = self.neighbours.remove(&gone).unwrap_or_default();
        for n in &moved {
            if let Some(set) = self.neighbours.get_mut(n) {
                set.remove(&gone);
                set.insert(keep);
            }
        }
        self.neighbours.entry(keep).or_default().extend(moved);
    }
}

impl Project {
    /// The closures the solve skips because they would tie the two ends of
    /// one shot together (#737), in list order, by id.
    pub(in crate::survey) fn closures_across_one_shot(&self) -> Vec<ClosureId> {
        if self.closures.is_empty() {
            return Vec::new();
        }
        self.closures_across_one_shot_in(&self.raw_positions())
    }

    /// [`Self::closures_across_one_shot`] with this state's positions already
    /// computed (snap alignment reads them).
    pub(in crate::survey) fn closures_across_one_shot_in(
        &self,
        positions: &RawPositions,
    ) -> Vec<ClosureId> {
        let tied: Vec<&LoopClosure> = self
            .closures
            .iter()
            .filter(|c| self.solve_ties(positions, c))
            .collect();
        if tied.is_empty() {
            return Vec::new();
        }
        let topology = self.topology();
        let n = topology.stations().len();
        let mut neighbours: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for line in self.lines() {
            let mut from = topology.dense(line.start);
            for shot in &line.shots {
                let to = topology.dense(shot.to);
                if let (Some(a), Some(b)) = (from, to) {
                    if a != b {
                        neighbours.entry(a).or_default().insert(b);
                        neighbours.entry(b).or_default().insert(a);
                    }
                }
                from = to;
            }
        }
        let mut knots = Knots {
            parent: (0..n).collect(),
            neighbours,
        };
        let mut skipped = Vec::new();
        for closure in tied {
            let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) else {
                continue;
            };
            match knots.collapses(a, b) {
                Some(true) => skipped.push(closure.id),
                Some(false) => knots.unite(a, b),
                None => {}
            }
        }
        skipped
    }

    /// Does the least-squares solve tie this closure (an eligible one, a
    /// snap, a legacy tie)?
    fn solve_ties(&self, positions: &RawPositions, closure: &LoopClosure) -> bool {
        match closure.strategy {
            ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted | ClosureStrategy::Tie => {
                true
            }
            ClosureStrategy::Move => self.closure_is_aligned(positions, closure),
            ClosureStrategy::ProportionalByLength | ClosureStrategy::EqualDistribution => false,
        }
    }

    /// Remove the closures the solve skips now that it did not skip in
    /// `before` (by id): what an edit newly left across one shot (#737).
    /// Returns them as they were, in list order.
    pub(in crate::survey) fn remove_closures_newly_across_one_shot(
        &mut self,
        before: &[ClosureId],
    ) -> Vec<LoopClosure> {
        let now = self.closures_across_one_shot();
        let newly: Vec<ClosureId> = now.into_iter().filter(|id| !before.contains(id)).collect();
        if newly.is_empty() {
            return Vec::new();
        }
        let mut removed = Vec::new();
        self.closures.retain(|c| {
            if newly.contains(&c.id) {
                removed.push(*c);
                false
            } else {
                true
            }
        });
        removed
    }
}
