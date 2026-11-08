//! Union-find over dense station slots.

/// Minimal union-find over station slots, shared by the Jump/Reference
/// classifier and the orphan-closure sweep.
pub(super) struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    pub(super) fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    pub(super) fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            // Path halving: keeps find near-constant without a second pass.
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    pub(super) fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[rb] = ra;
        }
    }
}
