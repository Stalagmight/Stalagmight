//! Legacy merges -> shared stations (plan 7.2.1).
//!
//! A union-find over slots, fed the legacy merges in the order the 0.6.0
//! load's offset cascade adopted them (`cascade_connection_offsets` as run by
//! `resolve_offsets`): the merges it walked to reach a line first, in its
//! BFS order, then every other merge in list order. A merge joins its two
//! slots into one station unless that would make the station graph cyclic:
//!
//! - both ends are one slot (a self-merge) or already one station (a
//!   duplicate): dropped, nothing to tie;
//! - both ends on one line (a same-line merge): demoted to a `Tie` closure;
//! - the ends already connected (a merge cycle): demoted to a `Tie` closure.
//!
//! The cascade order matters only inside merge cycles: the merges it walked
//! are the ones it aligned (the raw view puts their slots at one point), so
//! the merge it left torn is the one demoted, and every station stays where
//! the old build showed it.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use stalagmight_legacy::models::{LineId, StationKey};
use stalagmight_legacy::{LegacyProject, LegacySurvey, Slot};

use super::slots::Slots;

/// What became of one legacy merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fate {
    /// Its two slots are one station now.
    Collapsed,
    /// Kept as a `Tie` closure: it would have closed a cycle.
    DemotedCycle,
    /// Kept as a `Tie` closure: both ends are on one line.
    DemotedSameLine,
    /// Dropped: both ends are the same slot.
    DroppedSelf,
    /// Dropped: the ends were already one station (a repeated merge).
    DroppedRedundant,
    /// Dropped: an end does not resolve.
    DroppedDangling,
}

/// The merge plan of one legacy project.
pub(super) struct MergePlan {
    /// Per legacy merge (list order).
    pub(super) fates: Vec<Fate>,
    /// Per legacy merge: its resolved dense slots.
    pub(super) ends: Vec<Option<(usize, usize)>>,
    /// Per dense slot: the root of its station group.
    pub(super) group: Vec<usize>,
}

impl MergePlan {
    /// The demoted merges, in merge list order, as their two dense slots.
    pub(super) fn ties(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.fates
            .iter()
            .zip(&self.ends)
            .filter(|(fate, _)| matches!(fate, Fate::DemotedCycle | Fate::DemotedSameLine))
            .filter_map(|(_, ends)| *ends)
    }

    pub(super) fn count(&self, fate: Fate) -> usize {
        self.fates.iter().filter(|&&f| f == fate).count()
    }
}

/// Plan every legacy merge of `legacy` over `slots`.
pub(super) fn plan(legacy: &LegacySurvey, slots: &Slots) -> MergePlan {
    let project = &legacy.project;
    let resolve = |key: &StationKey| slots.resolve(key).and_then(|slot| slots.dense(slot));
    let ends: Vec<Option<(usize, usize)>> = project
        .station_merges
        .iter()
        .map(|m| Some((resolve(&m.station_a)?, resolve(&m.station_b)?)))
        .collect();

    let mut order = cascade_order(legacy, slots, &ends);
    let mut queued: Vec<bool> = vec![false; ends.len()];
    for &i in &order {
        queued[i] = true;
    }
    order.extend((0..ends.len()).filter(|&i| !queued[i] && ends[i].is_some()));

    let mut group = UnionFind::new(slots.len());
    let mut component = UnionFind::new(slots.line_count());
    let mut fates = vec![Fate::DroppedDangling; ends.len()];
    for i in order {
        let Some((a, b)) = ends[i] else {
            continue;
        };
        let (line_a, line_b) = (slots.line_position_of(a), slots.line_position_of(b));
        fates[i] = if a == b {
            Fate::DroppedSelf
        } else if group.find(a) == group.find(b) {
            Fate::DroppedRedundant
        } else if line_a == line_b {
            Fate::DemotedSameLine
        } else if component.find(line_a) == component.find(line_b) {
            Fate::DemotedCycle
        } else {
            group.union(a, b);
            component.union(line_a, line_b);
            Fate::Collapsed
        };
    }
    let group = (0..slots.len()).map(|d| group.find(d)).collect();
    MergePlan { fates, ends, group }
}

/// An edge of the cascade's coincidence graph.
#[derive(Clone, Copy)]
enum Edge {
    Merge(usize),
    Closure,
}

/// The merges the 0.6.0 load cascade adopted as alignment edges, in the
/// order it adopted them: phase 1 over merges from every authoritative line
/// (the per-component seeds of `realign_seeds` plus every placement-datum
/// line, sorted), then phase 2 over merges plus aligned closures. A merge is
/// adopted when it reaches a line not yet authoritative and the coincidence
/// is computable (both slots have finite positions).
///
/// Phase 2 reads the closures' `is_aligned` as the load left them, which is
/// re-measured after the cascade ran; it only orders merges inside a
/// component the merges alone could not reach, which a settled file does not
/// hold (a datum-less piece joined only by closures has them promoted).
fn cascade_order(
    legacy: &LegacySurvey,
    slots: &Slots,
    ends: &[Option<(usize, usize)>],
) -> Vec<usize> {
    let project = &legacy.project;
    let datum_lines: HashSet<LineId> = project
        .placement_datum_refs()
        .into_iter()
        .map(|sr| sr.line_id)
        .collect();
    let mut authoritative: HashSet<LineId> = realign_seeds(project, &datum_lines)
        .into_iter()
        .chain(datum_lines.iter().copied())
        .collect();

    type Adjacency = HashMap<LineId, Vec<(Slot, Slot, Edge)>>;
    let push = |adjacency: &mut Adjacency, a: Slot, b: Slot, edge: Edge| {
        adjacency.entry(a.line_id).or_default().push((a, b, edge));
        adjacency.entry(b.line_id).or_default().push((b, a, edge));
    };
    let mut merges: Adjacency = HashMap::new();
    for (i, pair) in ends.iter().enumerate() {
        if let Some((a, b)) = *pair {
            push(&mut merges, slots.slot(a), slots.slot(b), Edge::Merge(i));
        }
    }
    let mut closures: Adjacency = HashMap::new();
    for closure in &project.loop_closures {
        if !closure.is_aligned {
            continue;
        }
        let a = slots.resolve(&closure.station_a);
        let b = slots.resolve(&closure.station_b);
        if let (Some(a), Some(b)) = (a, b) {
            push(&mut closures, a, b, Edge::Closure);
        }
    }

    let computable = |a: Slot, b: Slot| {
        let (Some(pa), Some(pb)) = (legacy.raw_positions.get(&a), legacy.raw_positions.get(&b))
        else {
            return false;
        };
        (pa[0] - pb[0]).is_finite() && (pa[1] - pb[1]).is_finite()
    };
    let mut order = Vec::new();
    let mut bfs = |adjacency: &Adjacency, authoritative: &mut HashSet<LineId>| {
        let mut seeds: Vec<LineId> = authoritative.iter().copied().collect();
        seeds.sort_unstable();
        let mut queue: VecDeque<LineId> = seeds.into();
        while let Some(line) = queue.pop_front() {
            let Some(neighbours) = adjacency.get(&line) else {
                continue;
            };
            for &(mine, theirs, edge) in neighbours {
                if authoritative.contains(&theirs.line_id) || !computable(mine, theirs) {
                    continue;
                }
                authoritative.insert(theirs.line_id);
                queue.push_back(theirs.line_id);
                if let Edge::Merge(i) = edge {
                    order.push(i);
                }
            }
        }
    };
    bfs(&merges, &mut authoritative);
    if !closures.is_empty() {
        let mut combined = merges;
        for (line, edges) in closures {
            combined.entry(line).or_default().extend(edges);
        }
        bfs(&combined, &mut authoritative);
    }
    order
}

/// The legacy `realign_seeds`: per component of the line graph over every
/// merge and closure (by stored line id), its lowest placement-datum line,
/// else its lowest line id.
fn realign_seeds(project: &LegacyProject, datum_lines: &HashSet<LineId>) -> Vec<LineId> {
    let mut adjacency: HashMap<LineId, Vec<LineId>> = HashMap::new();
    let edges = project
        .station_merges
        .iter()
        .map(|m| (m.station_a.line_id, m.station_b.line_id))
        .chain(
            project
                .loop_closures
                .iter()
                .map(|c| (c.station_a.line_id, c.station_b.line_id)),
        );
    for (a, b) in edges {
        adjacency.entry(a).or_default().push(b);
        adjacency.entry(b).or_default().push(a);
    }
    let mut seeds = Vec::new();
    let mut visited: HashSet<LineId> = HashSet::new();
    for line in &project.lines {
        if visited.contains(&line.id) {
            continue;
        }
        let mut component: BTreeSet<LineId> = BTreeSet::new();
        let mut stack = vec![line.id];
        while let Some(current) = stack.pop() {
            if component.insert(current) {
                if let Some(next) = adjacency.get(&current) {
                    stack.extend(next.iter().filter(|n| !component.contains(n)));
                }
            }
        }
        let seed = component
            .iter()
            .copied()
            .find(|l| datum_lines.contains(l))
            .or_else(|| component.first().copied());
        seeds.extend(seed);
        visited.extend(component);
    }
    seeds
}

/// Union-find with path halving; unions keep the lower index as the root.
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
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    pub(super) fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}
