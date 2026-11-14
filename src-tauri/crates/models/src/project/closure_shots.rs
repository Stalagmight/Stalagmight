//! Loop closures that tie the two ends of ONE shot together (#737).
//!
//! The least-squares adjustment treats every merge and every eligible
//! (`Batch` / `LeastSquaresWeighted`) closure as "these stations are one
//! point" and builds constraint groups - knots - from them
//! (`lsq.rs`, `adjust_least_squares`). A shot whose two ends land in the same
//! knot is skipped by the solve and keeps its measured vector, so the
//! per-section translation check then finds two targets for one section and
//! the whole component falls back to measured geometry.
//!
//! A closure that does that has no loop to close: the two stations it ties
//! are joined by exactly one shot. This module is the ONE place that decides
//! which closures do, so creation (`create_loop_closure`, the batch importer
//! path, the connectivity check behind the closure dialog - each refuses ONLY
//! such a closure), the solve
//! (`adjust_loop_closures`), the warnings and the delete commands all agree.
//!
//! The rule, applied in `loop_closures` vector order on top of every merge:
//! a closure is "across one shot" when uniting its two ends' knots would put
//! both end stations of some shot into one knot that were not in one knot
//! before it. Such a closure is NOT united (it is skipped), so it never
//! changes what a later closure sees. Merges are united first, without the
//! check: a merge is a hard coincidence the user asserted, and the adjustment
//! unites them the same way.
//!
//! Cost: one union-find over every station plus the contracted shot graph
//! between knots ([`Knots`]): asking whether a closure collapses a shot is
//! `O(1)` (two `find`s and a map lookup), whatever the knot sizes and however
//! many closures ask about the same two knots. Unions merge adjacency maps
//! small-to-large, so a whole sweep is `O(n + m log m)` for `n` stations and
//! `m` shots touching united knots, and close to `O(n)` in the common case,
//! where knots are a handful of stations. That bound matters: the sweep runs
//! under the project lock on every warnings refresh, report, solve, delete and
//! closure-dialog check, and a file can carry any number of skipped closures.

use std::collections::{HashMap, HashSet};

use crate::{SectionId, StationRef};

use super::{ClosurePair, Project};

/// A loop closure the solve skips because it ties the two ends of one shot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureAcrossOneShot {
    /// `LoopClosure::id` of the skipped closure.
    pub closure_id: u64,
    /// The closure's own end stations (resolved, positional).
    pub station_a: StationRef,
    pub station_b: StationRef,
    /// The shot it collapses: `(section, shot index)`, the shot between
    /// stations `index` and `index + 1` of that section. When it would
    /// collapse several, the lowest `(section, index)` is reported.
    pub collapsed_shot: (SectionId, usize),
}

impl ClosureAcrossOneShot {
    /// The two end stations of the collapsed shot.
    pub fn shot_ends(&self) -> (StationRef, StationRef) {
        shot_ends(self.collapsed_shot)
    }
}

/// The user-facing refusal for a new closure that would tie the two ends of
/// the shot `(section, shot index)` together. Shared by every creation path
/// and the closure dialog's connectivity check, so the dialog shows exactly
/// what the backend says.
///
/// This is the ONLY creation-time refusal the helper makes. A closure between
/// two stations that other merges and closures already tie together is
/// accepted as before: uniting two stations already in one knot changes
/// nothing for the solve, and refusing it would drop closures from imported
/// files.
pub fn closure_across_one_shot_refusal(shot: (SectionId, usize)) -> String {
    let (from, to) = shot_ends(shot);
    format!(
        "This closure would tie together the two ends of one shot ({} to {}), so there is no loop to close. Pick stations joined by at least two separate paths.",
        station_id_label(from),
        station_id_label(to)
    )
}

/// `S:I`, the station id the UI shows (`formatStationId`).
pub fn station_id_label(station: StationRef) -> String {
    format!("{}:{}", station.section_id, station.station_index)
}

fn shot_ends((section_id, shot_index): (SectionId, usize)) -> (StationRef, StationRef) {
    let index = u32::try_from(shot_index).unwrap_or(u32::MAX);
    (
        StationRef::new(section_id, index),
        StationRef::new(section_id, index.saturating_add(1)),
    )
}

/// A shot, as `(section, shot index)`: the shot between stations `index` and
/// `index + 1`. Ordered, so "the lowest shot" is well defined.
type Shot = (SectionId, usize);

/// Union-find over every station of the project, plus the CONTRACTED shot
/// graph between its knots: for each knot (by root), the knots its stations
/// share a shot with, each with the lowest such shot. So "which shot would
/// uniting these two knots collapse" is two `find`s and one map lookup, however
/// big the knots are and however many closures ask about the same pair - a
/// skipped closure does not unite anything, so a project can ask the same
/// question about two large knots any number of times (#737).
///
/// Slots are laid out section by section, so a station's shot neighbours are
/// the adjacent slots. A knot that was never united keeps no map: its
/// neighbours are read off the adjacent slots (`implicit_neighbours`) and the
/// map is built only when it first takes part in a union. Most stations never
/// do, so the common case allocates a map per merge point and closure end, not
/// per station.
///
/// Invariant: the map of every materialised root is keyed by CURRENT roots and
/// is symmetric with its neighbours' maps. A union re-keys the absorbed root's
/// neighbours onto the survivor; an implicit neighbour needs nothing, because
/// its map is built through `find` when it is materialised.
pub(crate) struct Knots {
    /// First slot of each section.
    base: HashMap<SectionId, usize>,
    /// Per slot: (section, station index, section shot count).
    info: Vec<(SectionId, u32, u32)>,
    parent: Vec<usize>,
    /// Per root: neighbour root -> lowest shot between the two knots. `None`
    /// for a slot never united (see the type docs); stale for non-roots.
    adj: Vec<Option<HashMap<usize, Shot>>>,
}

impl Knots {
    fn new(project: &Project) -> Self {
        let total: usize = project.sections.iter().map(|s| s.shots.len() + 1).sum();
        let mut base = HashMap::with_capacity(project.sections.len());
        let mut info = Vec::with_capacity(total);
        for section in &project.sections {
            base.insert(section.id, info.len());
            let shots = u32::try_from(section.shots.len()).unwrap_or(u32::MAX);
            for index in 0..=shots {
                info.push((section.id, index, shots));
            }
        }
        let n = info.len();
        Self {
            base,
            info,
            parent: (0..n).collect(),
            adj: vec![None; n],
        }
    }

    fn slot(&self, station: StationRef) -> Option<usize> {
        let base = *self.base.get(&station.section_id)?;
        let (_, _, shots) = *self.info.get(base)?;
        (station.station_index <= shots).then(|| base + station.station_index as usize)
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    /// The (at most two) shot neighbours of `slot`: the adjacent slots of its
    /// section, with the shot to each.
    fn implicit_neighbours(&self, slot: usize) -> impl Iterator<Item = (usize, Shot)> {
        let (section_id, index, shots) = self.info[slot];
        let index = index as usize;
        let before = (index > 0).then(|| (slot - 1, (section_id, index - 1)));
        let after = (index < shots as usize).then(|| (slot + 1, (section_id, index)));
        before.into_iter().chain(after)
    }

    /// Build `root`'s map if it has none. Only a never-united slot has none,
    /// so its knot is itself and its neighbours are its adjacent slots, keyed
    /// by their current roots.
    fn materialise(&mut self, root: usize) {
        if self.adj[root].is_some() {
            return;
        }
        let neighbours: Vec<(usize, Shot)> = self.implicit_neighbours(root).collect();
        let mut map: HashMap<usize, Shot> = HashMap::with_capacity(neighbours.len());
        for (slot, shot) in neighbours {
            let r = self.find(slot);
            if r != root {
                let e = map.entry(r).or_insert(shot);
                *e = (*e).min(shot);
            }
        }
        self.adj[root] = Some(map);
    }

    fn map_len(&self, root: usize) -> usize {
        self.adj[root].as_ref().map_or(0, HashMap::len)
    }

    /// Unite two roots. The one with the larger map survives (small-to-large,
    /// so each map entry moves `O(log)` times overall); the absorbed root's
    /// entries merge into the survivor's, keeping the lowest shot per
    /// neighbour, and each of its materialised neighbours is re-keyed onto the
    /// survivor. The edge between the two becomes internal and is dropped.
    fn unite_roots(&mut self, ra: usize, rb: usize) {
        if ra == rb {
            return;
        }
        self.materialise(ra);
        self.materialise(rb);
        let (keep, gone) = if self.map_len(ra) >= self.map_len(rb) {
            (ra, rb)
        } else {
            (rb, ra)
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

    /// The lowest shot with one end in knot `ra` and the other in knot `rb`
    /// (two distinct roots): one lookup, or, for a never-united `ra`, a check
    /// of its two adjacent slots.
    fn shot_between(&mut self, ra: usize, rb: usize) -> Option<Shot> {
        if let Some(map) = self.adj[ra].as_ref() {
            return map.get(&rb).copied();
        }
        let neighbours: Vec<(usize, Shot)> = self.implicit_neighbours(ra).collect();
        let mut best: Option<Shot> = None;
        for (slot, shot) in neighbours {
            if self.find(slot) == rb {
                best = Some(best.map_or(shot, |b| b.min(shot)));
            }
        }
        best
    }

    /// The shot that uniting `a`'s and `b`'s knots would collapse, without
    /// uniting them: the lowest shot with one end in each knot. `None` when the
    /// two are already in one knot (uniting changes nothing), when no shot runs
    /// between the knots, or when either station does not exist. `O(1)` up to
    /// the union-find.
    pub(crate) fn collapsed_shot(&mut self, a: StationRef, b: StationRef) -> Option<Shot> {
        let (sa, sb) = (self.slot(a)?, self.slot(b)?);
        let (ra, rb) = (self.find(sa), self.find(sb));
        if ra == rb {
            return None;
        }
        self.shot_between(ra, rb)
    }

    /// Unite `a`'s and `b`'s knots unconditionally (a merge, or a closure the
    /// caller has already checked with [`Self::collapsed_shot`]). A no-op when
    /// they are already one knot.
    pub(crate) fn join(&mut self, a: StationRef, b: StationRef) {
        if let (Some(sa), Some(sb)) = (self.slot(a), self.slot(b)) {
            let (ra, rb) = (self.find(sa), self.find(sb));
            self.unite_roots(ra, rb);
        }
    }
}

impl Project {
    /// The knots of the current project - every merge, then every eligible
    /// closure in vector order that does not collapse a shot - and the
    /// closures that were skipped because they would.
    pub(crate) fn closure_knots(&self) -> (Knots, Vec<ClosureAcrossOneShot>) {
        let mut knots = Knots::new(self);
        for merge in &self.station_merges {
            if let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                knots.join(a, b);
            }
        }
        let mut skipped = Vec::new();
        for closure in &self.loop_closures {
            if !Self::closure_is_eligible(closure.strategy) {
                continue;
            }
            let (Some(a), Some(b)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) else {
                continue;
            };
            match knots.collapsed_shot(a, b) {
                Some(collapsed_shot) => skipped.push(ClosureAcrossOneShot {
                    closure_id: closure.id,
                    station_a: a,
                    station_b: b,
                    collapsed_shot,
                }),
                // Joins two knots safely, or (a redundant tie) changes nothing.
                None => knots.join(a, b),
            }
        }
        (knots, skipped)
    }

    /// Every eligible loop closure that ties the two ends of one shot
    /// together, in `loop_closures` order. The closure-adjusted solve skips
    /// exactly these (see the module docs for the rule).
    pub fn closures_across_one_shot(&self) -> Vec<ClosureAcrossOneShot> {
        self.closure_knots().1
    }

    /// Whether a NEW closure between `a` and `b` would tie the two ends of one
    /// shot together, and which shot `(section, shot index)` - judged against
    /// the knots the solve would use now (current merges and the eligible
    /// closures it does not skip). Applies whatever the new closure's strategy:
    /// a tie across one shot means nothing for a `Move` closure either. `None`
    /// for a closure that closes a real loop, and for one whose stations are
    /// already tied together (redundant, but harmless: it is still created).
    pub fn closure_spans_one_shot(
        &self,
        a: StationRef,
        b: StationRef,
    ) -> Option<(SectionId, usize)> {
        let (mut knots, _) = self.closure_knots();
        knots.collapsed_shot(a, b)
    }
}

/// What an edit needs to know, BEFORE it runs, to name the closures it leaves
/// across one shot (#737): which eligible closures already were (an edit only
/// answers for what it causes), and every eligible closure's two stations as
/// they resolve now. A gap-close renumbers stations, so closure 0:1-0:5 can
/// come out as 0:1-0:2; the user created it as 0:1-0:5 and is told so.
#[derive(Debug, Clone, Default)]
pub struct ClosuresBeforeEdit {
    across: HashSet<u64>,
    ends: HashMap<u64, (StationRef, StationRef)>,
}

impl Project {
    /// Capture [`ClosuresBeforeEdit`] for the project as it is now.
    /// `O(stations + closures)`.
    pub fn closures_before_edit(&self) -> ClosuresBeforeEdit {
        let across = self
            .closures_across_one_shot()
            .into_iter()
            .map(|c| c.closure_id)
            .collect();
        let ends = self
            .loop_closures
            .iter()
            .filter(|lc| Self::closure_is_eligible(lc.strategy))
            .filter_map(|lc| {
                Some((
                    lc.id,
                    (
                        self.resolve_key(&lc.station_a)?,
                        self.resolve_key(&lc.station_b)?,
                    ),
                ))
            })
            .collect();
        ClosuresBeforeEdit { across, ends }
    }

    /// The closures that tie the two ends of one shot together now but did
    /// not before the edit `before` was taken for, in `loop_closures` order,
    /// each named by its stations BEFORE the edit (its current ones only if it
    /// had none then). Shared by the delete commands, which remove them, and
    /// the range-delete preview, which lists them, so the two cannot disagree.
    pub fn closures_newly_across_one_shot(&self, before: &ClosuresBeforeEdit) -> Vec<ClosurePair> {
        self.closures_across_one_shot()
            .into_iter()
            .filter(|c| !before.across.contains(&c.closure_id))
            .map(|c| {
                let (station_a, station_b) = before
                    .ends
                    .get(&c.closure_id)
                    .copied()
                    .unwrap_or((c.station_a, c.station_b));
                ClosurePair {
                    closure_id: c.closure_id,
                    station_a,
                    station_b,
                }
            })
            .collect()
    }
}
