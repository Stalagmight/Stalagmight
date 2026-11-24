//! Frozen shots (plan 3.8): a shot is frozen when both its ends reach a
//! placement datum's station without it, over shots and aligned closures
//! (snaps and ties). Editing it would have to move a datum-anchored rigid
//! loop, so geometry edits refuse it. GPS fixes freeze nothing.
//!
//! Ported from `models/src/project/distance.rs` (`compute_frozen_shots`,
//! Tarjan's bridges in `O(V + E)`) on stations: a junction is one node, so
//! today's zero-cost merge edges are gone (contracting an edge changes no
//! other edge's bridge status). A non-bridge shot in a component with a
//! placement datum is frozen; a bridge shot is frozen when both of its sides
//! hold one. The output is positional (`{line_id, shot_index}`), lines in
//! list order, for the wire.

use crate::FrozenShotRef;

use super::{LineId, Project};

impl Project {
    /// Every frozen shot, lines in list order, shots in chain order, for the
    /// alignment `aligned` (parallel to `closures`).
    pub fn frozen_shots(&self, aligned: &[bool]) -> Vec<FrozenShotRef> {
        let topology = self.topology();
        let n = topology.stations().len();
        if self.datums.is_empty() || n == 0 {
            return Vec::new();
        }
        let mut is_datum = vec![false; n];
        let mut any = false;
        for d in self.placement_datums().into_iter().flatten() {
            if let Some(i) = topology.dense(self.datums[d].station) {
                is_datum[i] = true;
                any = true;
            }
        }
        if !any {
            return Vec::new();
        }

        // Edges: shots (lines in list order) then aligned closures; each edge
        // has an id so the tree edge to a parent is skipped once (multigraph).
        let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
        let mut shot_edges: Vec<(LineId, usize, usize, usize)> = Vec::new();
        let mut edge_count = 0;
        for (position, line) in self.lines().iter().enumerate() {
            let Some(chain) = topology.chain_at(position) else {
                continue;
            };
            for index in 0..line.shots.len() {
                let (Some(a), Some(b)) = (
                    chain.get(index).and_then(|&s| topology.dense(s)),
                    chain.get(index + 1).and_then(|&s| topology.dense(s)),
                ) else {
                    continue;
                };
                adj[a].push((b, edge_count));
                adj[b].push((a, edge_count));
                shot_edges.push((line.id, index, a, b));
                edge_count += 1;
            }
        }
        for (closure, &is_aligned) in self.closures.iter().zip(aligned) {
            if !is_aligned {
                continue;
            }
            if let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) {
                adj[a].push((b, edge_count));
                adj[b].push((a, edge_count));
                edge_count += 1;
            }
        }

        // Iterative Tarjan: discovery time, low link, datums per DFS subtree.
        let mut disc = vec![0u32; n];
        let mut low = vec![0u32; n];
        let mut visited = vec![false; n];
        let mut subtree = vec![0u32; n];
        let mut parent = vec![usize::MAX; n];
        let mut parent_edge = vec![usize::MAX; n];
        let mut bridge_to = vec![false; n];
        let mut root_of = vec![usize::MAX; n];
        let mut total = vec![0u32; n];
        let mut timer: u32 = 1;
        for start in 0..n {
            if visited[start] {
                continue;
            }
            visited[start] = true;
            disc[start] = timer;
            low[start] = timer;
            timer += 1;
            subtree[start] = u32::from(is_datum[start]);
            root_of[start] = start;
            let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
            while let Some(&mut (u, ref mut next)) = stack.last_mut() {
                if *next < adj[u].len() {
                    let (w, edge) = adj[u][*next];
                    *next += 1;
                    if !visited[w] {
                        visited[w] = true;
                        disc[w] = timer;
                        low[w] = timer;
                        timer += 1;
                        subtree[w] = u32::from(is_datum[w]);
                        parent[w] = u;
                        parent_edge[w] = edge;
                        root_of[w] = start;
                        stack.push((w, 0));
                    } else if edge != parent_edge[u] {
                        low[u] = low[u].min(disc[w]);
                    }
                } else {
                    stack.pop();
                    if let Some(&(p, _)) = stack.last() {
                        low[p] = low[p].min(low[u]);
                        subtree[p] += subtree[u];
                        if low[u] > disc[p] {
                            bridge_to[u] = true;
                        }
                    }
                }
            }
            total[start] = subtree[start];
        }

        let mut frozen = Vec::new();
        for (line_id, index, a, b) in shot_edges {
            let all = total[root_of[a]];
            if all == 0 {
                continue;
            }
            let child = if parent[b] == a && bridge_to[b] {
                Some(b)
            } else if parent[a] == b && bridge_to[a] {
                Some(a)
            } else {
                None
            };
            let is_frozen = match child {
                Some(c) => subtree[c] > 0 && all - subtree[c] > 0,
                None => true,
            };
            if is_frozen {
                frozen.push(FrozenShotRef {
                    line_id,
                    shot_index: u32::try_from(index).unwrap_or(u32::MAX),
                });
            }
        }
        frozen
    }

    /// Whether shot `index` of line `line` is frozen (see [`Self::frozen_shots`]).
    pub fn is_shot_frozen(&self, line: LineId, index: usize) -> bool {
        let aligned = self.closure_alignment(&self.raw_positions());
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        self.frozen_shots(&aligned)
            .iter()
            .any(|f| f.line_id == line && f.shot_index == index)
    }
}
