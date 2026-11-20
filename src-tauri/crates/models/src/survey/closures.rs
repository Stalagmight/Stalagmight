//! Closure alignment, the across-one-shot skip set and co-located (tie)
//! groups (plan 3.6 and invariant 7).
//!
//! - **Alignment** ([`Project::closure_alignment`]): `Tie` always; `Move` while
//!   its ends are within 1 cm horizontally and the tie would not join two
//!   datum-held components; nothing else (M0's
//!   [`Project::closure_is_aligned`], once per closure).
//! - **Ties in the solve**: an eligible closure (`Batch`,
//!   `LeastSquaresWeighted`, and the legacy rigid `Tie`, which gives its
//!   component something to solve as a legacy aligned `Batch` closure does
//!   today) or an aligned snap.
//! - **Across one shot** (#737, ported from `models/src/project/closure_shots.rs`):
//!   knots are a union-find over stations (a junction is already one station,
//!   so today's "merges first" is built in); every closure that ties in the
//!   solve is taken in list order, and one whose union would put both ends of
//!   some shot into one knot is skipped (and never united, so it changes
//!   nothing a later closure sees). The lowest `(line id, shot index)` is the
//!   collapsed shot reported. Transitive through other closures and snaps.
//! - **Tie groups**: stations joined by aligned closures, transitively: what
//!   depth co-location, the Move tools' co-located sets and the immovable
//!   stations read.

use std::collections::BTreeMap;

use crate::{LineId, SkippedClosure, StationId, StationRef};

use super::topology::find;
use super::{ClosureId, ClosureStrategy, LoopClosure, Project, RawPositions};

/// A shot as `(line id, shot index)`: ordered, so "the lowest shot" is
/// well defined.
pub type ShotKey = (LineId, usize);

/// A loop closure the solve skips because it ties the two ends of one shot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosureAcrossOneShot {
    pub closure_id: ClosureId,
    /// Its list position.
    pub index: usize,
    pub a: StationId,
    pub b: StationId,
    /// The shot it collapses (the lowest when several).
    pub collapsed_shot: ShotKey,
}

impl ClosureAcrossOneShot {
    /// The wire form (today's `SkippedClosure`): the closure's ends as their
    /// `via` slots (else first slots), and the collapsed shot.
    pub fn to_wire(&self, project: &Project) -> Option<SkippedClosure> {
        let closure = project.closures.get(self.index)?;
        Some(SkippedClosure {
            closure_id: self.closure_id,
            station_a: project.closure_end_slot(closure.a, closure.a_via)?,
            station_b: project.closure_end_slot(closure.b, closure.b_via)?,
            shot_line_id: self.collapsed_shot.0,
            shot_index: u32::try_from(self.collapsed_shot.1).unwrap_or(u32::MAX),
        })
    }
}

/// Stations joined by aligned closures (transitively), per state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TieGroups {
    stations: Vec<StationId>,
    group_of: Vec<usize>,
    members: Vec<Vec<StationId>>,
}

impl TieGroups {
    /// The group of `station` (every member, ascending; the station alone
    /// when it has no aligned closure). Empty for a station on no line.
    pub fn group(&self, station: StationId) -> &[StationId] {
        match self.stations.binary_search(&station) {
            Ok(d) => &self.members[self.group_of[d]],
            Err(_) => &[],
        }
    }

    /// Every group of two or more stations, by lowest member.
    pub fn nontrivial(&self) -> impl Iterator<Item = &[StationId]> {
        self.members
            .iter()
            .filter(|m| m.len() > 1)
            .map(Vec::as_slice)
    }

    /// Whether two stations are in one group.
    pub fn tied(&self, a: StationId, b: StationId) -> bool {
        match (
            self.stations.binary_search(&a),
            self.stations.binary_search(&b),
        ) {
            (Ok(x), Ok(y)) => self.group_of[x] == self.group_of[y],
            _ => false,
        }
    }
}

impl Project {
    /// Whether a closure of this strategy gives its component something to
    /// solve in the Closures Computed View: `Batch`, `LeastSquaresWeighted`
    /// (today's `closure_is_eligible`), and `Tie` (a legacy aligned closure
    /// that today is an eligible one; plan 7.2.4).
    pub fn closure_is_eligible(strategy: ClosureStrategy) -> bool {
        matches!(
            strategy,
            ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted | ClosureStrategy::Tie
        )
    }

    /// Whether the least squares ties a closure's two stations as one point:
    /// an eligible one or an aligned snap (today's `closure_ties_in_solve`).
    pub fn closure_ties_in_solve(closure: &LoopClosure, aligned: bool) -> bool {
        Self::closure_is_eligible(closure.strategy) || aligned
    }

    /// `is_aligned` of every closure, in list order (plan 3.6), for the raw
    /// layout `positions` of this state.
    pub fn closure_alignment(&self, positions: &RawPositions) -> Vec<bool> {
        self.closures
            .iter()
            .map(|closure| self.closure_is_aligned(positions, closure))
            .collect()
    }

    /// The slot a closure end is shown on: its station's slot on `via` when
    /// that line passes it, else its first slot.
    pub fn closure_end_slot(&self, station: StationId, via: Option<LineId>) -> Option<StationRef> {
        let topology = self.topology();
        via.and_then(|line| topology.slot_on(station, line))
            .or_else(|| topology.first_slot(station))
    }

    /// The knots and the skipped closures for the alignment `aligned`
    /// (parallel to `closures`). Decided in list order: the first closure
    /// whose union would collapse a shot is joined and a later one is
    /// skipped, so reordering closures can change the skip set.
    pub(crate) fn closure_knots(&self, aligned: &[bool]) -> (Knots, Vec<ClosureAcrossOneShot>) {
        let mut knots = Knots::new(self);
        let topology = self.topology();
        let mut skipped = Vec::new();
        for (index, closure) in self.closures.iter().enumerate() {
            let is_aligned = aligned.get(index).copied().unwrap_or(false);
            if !Self::closure_ties_in_solve(closure, is_aligned) {
                continue;
            }
            let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) else {
                continue;
            };
            match knots.collapsed_shot(a, b) {
                Some(collapsed_shot) => skipped.push(ClosureAcrossOneShot {
                    closure_id: closure.id,
                    index,
                    a: closure.a,
                    b: closure.b,
                    collapsed_shot,
                }),
                None => knots.join(a, b),
            }
        }
        (knots, skipped)
    }

    /// Every closure the solve would tie that ties the two ends of one shot,
    /// in list order (the skip set and the `CLOSURE_ACROSS_ONE_SHOT` data).
    pub fn closures_across_one_shot(&self) -> Vec<ClosureAcrossOneShot> {
        let aligned = self.closure_alignment(&self.raw_positions());
        self.closures_across_one_shot_with(&aligned)
    }

    /// [`Self::closures_across_one_shot`] for an alignment already computed.
    pub fn closures_across_one_shot_with(&self, aligned: &[bool]) -> Vec<ClosureAcrossOneShot> {
        self.closure_knots(aligned).1
    }

    /// Whether a NEW closure between `a` and `b` would tie the two ends of one
    /// shot, and which (today's `closure_spans_one_shot`), against the knots
    /// the solve uses now. `None` for a closure that closes a real loop, and
    /// for one whose stations are already one knot.
    pub fn closure_spans_one_shot(&self, a: StationId, b: StationId) -> Option<ShotKey> {
        let topology = self.topology();
        let (da, db) = (topology.dense(a)?, topology.dense(b)?);
        let aligned = self.closure_alignment(&self.raw_positions());
        let (mut knots, _) = self.closure_knots(&aligned);
        knots.collapsed_shot(da, db)
    }

    /// The tie groups of this state for the alignment `aligned`.
    pub fn tie_groups(&self, aligned: &[bool]) -> TieGroups {
        let topology = self.topology();
        let n = topology.stations().len();
        let mut parent: Vec<usize> = (0..n).collect();
        for (closure, &is_aligned) in self.closures.iter().zip(aligned) {
            if !is_aligned {
                continue;
            }
            if let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
        let mut group_of = vec![0; n];
        let mut members: Vec<Vec<StationId>> = Vec::new();
        let mut index_of_root: BTreeMap<usize, usize> = BTreeMap::new();
        for (d, slot) in group_of.iter_mut().enumerate() {
            let root = find(&mut parent, d);
            let g = *index_of_root.entry(root).or_insert_with(|| {
                members.push(Vec::new());
                members.len() - 1
            });
            *slot = g;
            members[g].push(topology.stations()[d]);
        }
        TieGroups {
            stations: topology.stations().to_vec(),
            group_of,
            members,
        }
    }
}

/// Union-find over the dense stations plus the CONTRACTED shot graph between
/// knots: for each united knot (by root), the knots its stations share a
/// shot with, each with the lowest such shot. Asking which shot uniting two
/// knots would collapse is two `find`s and one map lookup (today's `Knots`,
/// `models/src/project/closure_shots.rs`, on stations instead of slots, with
/// `BTreeMap`s).
///
/// A station never united keeps no map: its neighbours are read off its
/// incident shots. Unions merge maps small-to-large and re-key the absorbed
/// root's materialised neighbours onto the survivor.
pub(crate) struct Knots {
    /// CSR of incident shots per dense station: `(neighbour, shot)`.
    offsets: Vec<usize>,
    incident: Vec<(usize, ShotKey)>,
    parent: Vec<usize>,
    adj: Vec<Option<BTreeMap<usize, ShotKey>>>,
}

impl Knots {
    pub(crate) fn new(project: &Project) -> Self {
        let n = project.topology().stations().len();
        let mut pairs: Vec<(usize, usize, ShotKey)> = Vec::new();
        project.for_each_shot_edge(|edge| {
            let shot = (edge.line.id, edge.index);
            pairs.push((edge.from, edge.to, shot));
            pairs.push((edge.to, edge.from, shot));
        });
        pairs.sort_unstable();
        let mut offsets = vec![0; n + 1];
        for &(from, _, _) in &pairs {
            offsets[from + 1] += 1;
        }
        for d in 0..n {
            offsets[d + 1] += offsets[d];
        }
        let incident = pairs.into_iter().map(|(_, to, shot)| (to, shot)).collect();
        Self {
            offsets,
            incident,
            parent: (0..n).collect(),
            adj: vec![None; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        find(&mut self.parent, x)
    }

    fn implicit(&self, station: usize) -> &[(usize, ShotKey)] {
        &self.incident[self.offsets[station]..self.offsets[station + 1]]
    }

    fn materialise(&mut self, root: usize) {
        if self.adj[root].is_some() {
            return;
        }
        let neighbours: Vec<(usize, ShotKey)> = self.implicit(root).to_vec();
        let mut map: BTreeMap<usize, ShotKey> = BTreeMap::new();
        for (station, shot) in neighbours {
            let r = self.find(station);
            if r != root {
                let e = map.entry(r).or_insert(shot);
                *e = (*e).min(shot);
            }
        }
        self.adj[root] = Some(map);
    }

    fn map_len(&self, root: usize) -> usize {
        self.adj[root].as_ref().map_or(0, BTreeMap::len)
    }

    fn unite_roots(&mut self, ra: usize, rb: usize) {
        if ra == rb {
            return;
        }
        self.materialise(ra);
        self.materialise(rb);
        // Larger map survives; equal sizes keep the lower root (determinism).
        let (keep, gone) = match self.map_len(ra).cmp(&self.map_len(rb)) {
            std::cmp::Ordering::Greater => (ra, rb),
            std::cmp::Ordering::Less => (rb, ra),
            std::cmp::Ordering::Equal => (ra.min(rb), ra.max(rb)),
        };
        self.parent[gone] = keep;
        let gone_map = self.adj[gone].take().unwrap_or_default();
        let mut keep_map = self.adj[keep].take().unwrap_or_default();
        keep_map.remove(&gone);
        for (x, shot) in gone_map {
            if x == keep {
                continue;
            }
            let e = keep_map.entry(x).or_insert(shot);
            *e = (*e).min(shot);
            if let Some(x_map) = self.adj[x].as_mut() {
                if let Some(old) = x_map.remove(&gone) {
                    let e = x_map.entry(keep).or_insert(old);
                    *e = (*e).min(old);
                }
            }
        }
        self.adj[keep] = Some(keep_map);
    }

    fn shot_between(&mut self, ra: usize, rb: usize) -> Option<ShotKey> {
        if let Some(map) = self.adj[ra].as_ref() {
            return map.get(&rb).copied();
        }
        let neighbours: Vec<(usize, ShotKey)> = self.implicit(ra).to_vec();
        let mut best: Option<ShotKey> = None;
        for (station, shot) in neighbours {
            if self.find(station) == rb {
                best = Some(best.map_or(shot, |b| b.min(shot)));
            }
        }
        best
    }

    /// The shot uniting `a`'s and `b`'s knots would collapse, without uniting
    /// them. `None` when they are one knot already or no shot joins them.
    pub(crate) fn collapsed_shot(&mut self, a: usize, b: usize) -> Option<ShotKey> {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return None;
        }
        self.shot_between(ra, rb)
    }

    /// Unite `a`'s and `b`'s knots unconditionally.
    pub(crate) fn join(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        self.unite_roots(ra, rb);
    }
}
