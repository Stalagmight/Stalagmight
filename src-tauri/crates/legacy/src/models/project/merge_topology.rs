//! The project's station merges plus the merge-only line adjacency derived
//! from them.
//!
//! The adjacency map turns the per-BFS-step scan over every merge (O(N) inner)
//! into a HashMap lookup (O(1) inner), which is what keeps
//! [`Project::get_merged_line_ids`](crate::models::Project::get_merged_line_ids)
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

use crate::models::{LineId, StationMerge};

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
