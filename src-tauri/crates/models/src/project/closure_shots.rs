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
//! Cost: one union-find over every station, with each knot's members on a
//! circular list. Uniting two knots walks only the SMALLER one, checking the
//! (at most two) shot neighbours of each member, so a whole sweep is
//! `O(n log n)` in the worst case and `O(n)` in the common case, where knots
//! are a handful of stations.

use std::collections::HashMap;

use crate::{SectionId, StationRef};

use super::Project;

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

/// Union-find over every station of the project, slots laid out section by
/// section so a station's shot neighbours are the adjacent slots.
pub(crate) struct Knots {
    /// First slot of each section.
    base: HashMap<SectionId, usize>,
    /// Per slot: (section, station index, section shot count).
    info: Vec<(SectionId, u32, u32)>,
    parent: Vec<usize>,
    /// Knot size, valid at roots.
    size: Vec<usize>,
    /// Circular member list: `next[s]` is the next member of `s`'s knot.
    next: Vec<usize>,
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
            size: vec![1; n],
            next: (0..n).collect(),
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

    fn unite_roots(&mut self, ra: usize, rb: usize) {
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra] >= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[small] = big;
        self.size[big] += self.size[small];
        // Splice the two circular member lists.
        self.next.swap(big, small);
    }

    /// The lowest shot with one end in knot `ra` and the other in knot `rb`,
    /// walking the smaller knot.
    fn shot_between(&mut self, ra: usize, rb: usize) -> Option<(SectionId, usize)> {
        let (walk, other) = if self.size[ra] <= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        let mut best: Option<(SectionId, usize)> = None;
        let mut s = walk;
        loop {
            let (section_id, index, shots) = self.info[s];
            let index = index as usize;
            if index > 0 && self.find(s - 1) == other {
                let c = (section_id, index - 1);
                best = Some(best.map_or(c, |b| b.min(c)));
            }
            if index < shots as usize && self.find(s + 1) == other {
                let c = (section_id, index);
                best = Some(best.map_or(c, |b| b.min(c)));
            }
            s = self.next[s];
            if s == walk {
                break;
            }
        }
        best
    }

    /// The shot that uniting `a`'s and `b`'s knots would collapse, without
    /// uniting them: the lowest shot with one end in each knot. `None` when the
    /// two are already in one knot (uniting changes nothing), when no shot runs
    /// between the knots, or when either station does not exist.
    pub(crate) fn collapsed_shot(
        &mut self,
        a: StationRef,
        b: StationRef,
    ) -> Option<(SectionId, usize)> {
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
