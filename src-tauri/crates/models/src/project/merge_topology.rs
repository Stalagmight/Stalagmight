//! The project's station merges plus the merge-only line adjacency derived
//! from them.
//!
//! The adjacency map turns the per-BFS-step scan over every merge (O(N) inner)
//! into a HashMap lookup (O(1) inner), which is what keeps
//! [`Project::get_merged_line_ids`](crate::Project::get_merged_line_ids)
//! linear on a large import instead of quadratic. It used to live on `Project`
//! as a plain field under an informal "rebuild it after you mutate
//! `station_merges`" contract - and a path that rewrote merge endpoints in
//! place (`Project::map_station_refs`, run by the delete-shot line split)
//! quietly broke that contract, leaving the double-anchor guard reading a
//! pre-split merge graph.
//!
//! So the map is not a field anyone can forget to refresh: it is a memo owned
//! by this type, reachable only through [`MergeTopology::adjacency`], and every
//! mutator here either invalidates it or applies its own O(1) delta. There is
//! no `DerefMut` and no way to get a `&mut Vec<StationMerge>`, so a mutation
//! that bypasses the invalidation cannot be written.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::{LineId, StationMerge};

/// Station merges + a lazily-built, self-invalidating merge-only line
/// adjacency map. Reads like a `[StationMerge]` slice (via [`Deref`]); writes go
/// through the explicit mutators below.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MergeTopology {
    merges: Vec<StationMerge>,
    /// `line_id` -> neighbouring line ids reachable over one merge.
    /// Transient and NOT persisted: rebuilt on first [`Self::adjacency`] call
    /// after any invalidation (including deserialization).
    #[serde(skip)]
    adjacency: OnceLock<HashMap<LineId, Vec<LineId>>>,
}

impl MergeTopology {
    /// The merge-only line adjacency. THE only read path for the map; built
    /// on first use and reused until a mutator invalidates it.
    pub fn adjacency(&self) -> &HashMap<LineId, Vec<LineId>> {
        self.adjacency.get_or_init(|| Self::build(&self.merges))
    }

    /// Append a merge, keeping an already-built memo current with an O(1)
    /// delta rather than dropping it - `record_station_merge` runs once per
    /// merge on the import hot path, and rebuilding there would make the import
    /// quadratic again. This is the ONE hand-maintained delta in the type; every
    /// other mutator invalidates.
    pub fn push(&mut self, merge: StationMerge) {
        if let Some(adjacency) = self.adjacency.get_mut() {
            // The delta and `build` share ONE edge-insertion routine, so "same
            // insertion order as a fresh rebuild reaching the last merge" is now
            // structurally guaranteed rather than a comment two call sites have
            // to honor (still asserted by `debug_assert_memo_fresh`).
            Self::insert_edge(adjacency, &merge);
        }
        self.merges.push(merge);
    }

    /// Mutable iteration over the merges. Invalidates the memo up front: the
    /// caller can rewrite an endpoint's `line_id`, which is exactly the edit
    /// the memo describes.
    #[expect(
        clippy::iter_without_into_iter,
        reason = "an `IntoIterator for &mut MergeTopology` would make `for m in &mut merges` \
                  compile again - the named call is the point, so the invalidating path is \
                  visible at the call site"
    )]
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, StationMerge> {
        self.adjacency.take();
        self.merges.iter_mut()
    }

    /// Drop every merge for which `f` returns false.
    pub fn retain(&mut self, f: impl FnMut(&StationMerge) -> bool) {
        self.adjacency.take();
        self.merges.retain(f);
    }

    /// Remove and return the merge at `index`.
    ///
    /// # Panics
    /// Panics if `index` is out of bounds (same contract as `Vec::remove`).
    pub fn remove(&mut self, index: usize) -> StationMerge {
        self.adjacency.take();
        self.merges.remove(index)
    }

    /// Insert `merge` at `index`, shifting the rest right.
    ///
    /// # Panics
    /// Panics if `index > len` (same contract as `Vec::insert`).
    pub fn insert(&mut self, index: usize, merge: StationMerge) {
        self.adjacency.take();
        self.merges.insert(index, merge);
    }

    /// The one place a merge becomes two adjacency entries, shared by [`Self::build`]
    /// and [`Self::push`]'s O(1) delta so the two cannot describe an edge differently.
    fn insert_edge(adjacency: &mut HashMap<LineId, Vec<LineId>>, m: &StationMerge) {
        // Each merge contributes one entry in each direction.
        adjacency
            .entry(m.station_a.line_id)
            .or_default()
            .push(m.station_b.line_id);
        adjacency
            .entry(m.station_b.line_id)
            .or_default()
            .push(m.station_a.line_id);
    }

    fn build(merges: &[StationMerge]) -> HashMap<LineId, Vec<LineId>> {
        let mut adjacency: HashMap<LineId, Vec<LineId>> = HashMap::new();
        for m in merges {
            Self::insert_edge(&mut adjacency, m);
        }
        adjacency
    }

    /// Debug-only tripwire: a built memo must equal a fresh build. Catches a
    /// future hand-maintained delta drifting from `build`.
    pub fn debug_assert_memo_fresh(&self, context: &str) {
        #[cfg(debug_assertions)]
        if let Some(memo) = self.adjacency.get() {
            debug_assert_eq!(
                *memo,
                Self::build(&self.merges),
                "merge adjacency memo is stale after {context}"
            );
        }
        // Suppress unused warning in release.
        let _ = context;
    }
}

impl Deref for MergeTopology {
    type Target = [StationMerge];

    fn deref(&self) -> &Self::Target {
        &self.merges
    }
}

impl From<Vec<StationMerge>> for MergeTopology {
    fn from(merges: Vec<StationMerge>) -> Self {
        Self {
            merges,
            adjacency: OnceLock::new(),
        }
    }
}

impl FromIterator<StationMerge> for MergeTopology {
    fn from_iter<T: IntoIterator<Item = StationMerge>>(iter: T) -> Self {
        Self::from(iter.into_iter().collect::<Vec<_>>())
    }
}

impl<'a> IntoIterator for &'a MergeTopology {
    type Item = &'a StationMerge;
    type IntoIter = std::slice::Iter<'a, StationMerge>;

    fn into_iter(self) -> Self::IntoIter {
        self.merges.iter()
    }
}

impl IntoIterator for MergeTopology {
    type Item = StationMerge;
    type IntoIter = std::vec::IntoIter<StationMerge>;

    fn into_iter(self) -> Self::IntoIter {
        self.merges.into_iter()
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test-only: fixed-size fixtures, a panic is the failure signal"
)]
mod tests {
    use super::*;
    use crate::{StationKey, StationRef};
    use chrono::Utc;

    fn merge(a: LineId, b: LineId) -> StationMerge {
        StationMerge {
            station_a: StationKey::from(StationRef::new(a, 1)),
            station_b: StationKey::from(StationRef::new(b, 0)),
            merged_at: Utc::now(),
        }
    }

    fn warm(merges: &[StationMerge]) -> MergeTopology {
        let topology = MergeTopology::from(merges.to_vec());
        // Force the memo to exist so each mutator below is exercised against a
        // BUILT memo - an uninitialized one can never be stale.
        let _ = topology.adjacency();
        topology
    }

    fn assert_memo_matches_fresh_build(topology: &MergeTopology, case: &str) {
        let fresh = MergeTopology::from(topology.to_vec());
        assert_eq!(
            *topology.adjacency(),
            *fresh.adjacency(),
            "adjacency disagrees with a fresh build after {case}"
        );
        topology.debug_assert_memo_fresh(case);
    }

    #[test]
    fn every_mutator_leaves_the_memo_consistent() {
        // Table-driven: each case warms the memo, mutates through ONE mutator,
        // then requires the memo to equal a freshly built map. A mutator that
        // forgets to invalidate (or hand-maintains a wrong delta) fails here.
        let base = vec![merge(0, 1), merge(1, 2)];
        let cases: Vec<(&str, Box<dyn Fn(&mut MergeTopology)>)> = vec![
            (
                "push",
                Box::new(|t: &mut MergeTopology| t.push(merge(2, 3))),
            ),
            (
                "push onto a cold memo",
                Box::new(|t: &mut MergeTopology| {
                    // `From` (like `FromIterator`) starts cold, so this replaces
                    // the warm memo with an uninitialized one - the push below
                    // then has no memo to delta and must simply leave it cold.
                    *t = MergeTopology::from(vec![merge(0, 1), merge(1, 2)]);
                    t.push(merge(2, 3));
                }),
            ),
            (
                "iter_mut rewriting an endpoint's line",
                Box::new(|t: &mut MergeTopology| {
                    for m in t.iter_mut() {
                        m.station_b = StationKey::from(StationRef::new(9, 0));
                    }
                }),
            ),
            (
                "retain",
                Box::new(|t: &mut MergeTopology| {
                    t.retain(|m| m.station_a.line_id != 0);
                }),
            ),
            (
                "retain dropping everything",
                Box::new(|t: &mut MergeTopology| t.retain(|_| false)),
            ),
            (
                "remove",
                Box::new(|t: &mut MergeTopology| {
                    let _ = t.remove(0);
                }),
            ),
            (
                "insert at an interior index",
                Box::new(|t: &mut MergeTopology| t.insert(1, merge(4, 5))),
            ),
            (
                "insert at the end index",
                Box::new(|t: &mut MergeTopology| t.insert(t.len(), merge(4, 5))),
            ),
        ];

        for (case, mutate) in cases {
            let mut topology = warm(&base);
            mutate(&mut topology);
            assert_memo_matches_fresh_build(&topology, case);
        }

        // FromIterator starts cold; its first adjacency() call must still build
        // the right map.
        let collected: MergeTopology = base.iter().cloned().collect();
        assert_memo_matches_fresh_build(&collected, "FromIterator");
    }

    #[test]
    fn adjacency_is_bidirectional_and_reflects_every_merge() {
        let topology = warm(&[merge(0, 1), merge(1, 2)]);
        let adjacency = topology.adjacency();
        assert_eq!(adjacency[&0], vec![1]);
        assert_eq!(adjacency[&1], vec![0, 2]);
        assert_eq!(adjacency[&2], vec![1]);
        assert_eq!(topology.len(), 2, "Deref exposes the merge slice");
    }

    #[test]
    fn serde_round_trip_is_the_plain_merge_vec() {
        // The persisted/snapshot shape must stay byte-identical to the bare
        // Vec<StationMerge> this type replaced.
        let topology = warm(&[merge(0, 1), merge(1, 2)]);
        let as_topology = serde_json::to_string(&topology).unwrap();
        let as_vec = serde_json::to_string(&topology.to_vec()).unwrap();
        assert_eq!(as_topology, as_vec);

        let restored: MergeTopology = serde_json::from_str(&as_topology).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(*restored.adjacency(), *topology.adjacency());
    }
}
