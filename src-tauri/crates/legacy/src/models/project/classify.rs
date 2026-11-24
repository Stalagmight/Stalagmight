//! Jump vs Reference classification for virtual shots (#27).
//!
//! A parser can only tell that a shot is *virtual* (Ariane `VIRTUAL`, a Walls
//! splay, a Compass `X` leg, a GPX track). Deciding which of the two virtual
//! kinds it is needs the project graph, which does not exist until the import
//! is placed and merged. So the parsers seed every virtual shot as
//! [`ShotKind::Reference`] and this pass runs once at import finalize.
//!
//! # The rule
//!
//! A connected run of virtual shots that reaches real (non-virtual) survey at
//! **two or more distinct points** is a [`ShotKind::Jump`]: it bridges two
//! places a diver can already be, which is what a jump line does. A run that
//! reaches real survey at one point (a dangling tie into a lead) or nowhere at
//! all is a [`ShotKind::Reference`].
//!
//! "Reaches" is evaluated through station merges **and all loop closures**,
//! because a merged station and its partner are the same physical point. Note
//! all closures count, not only `is_aligned` ones: commit 88ccbaff established
//! that a closed loop is a real physical connection whatever reconciliation
//! strategy produced it.
//!
//! # Known limitation (deliberate, see #27)
//!
//! Boundedness cannot distinguish a swum jump from a non-swimmable tape tie:
//! both bridge two real points. `docs/SPEC.md` even documents the canonical
//! reference workflow as "mark the connecting shots, place one end, merge the
//! other", which lands bounded and therefore classifies as Jump. That is the
//! false-shortcut case #77 flagged. The mitigations are that the data table
//! cell reclassifies in one click (and fill-drags down a run), and that
//! `gas.include_reference_legs` can restore the old inclusive behaviour.
//!
//! There are exactly two call sites, and each is a point where a shot's
//! virtualness is *established* rather than edited: import finalize
//! (`command_import.rs`) and the one-shot post-migration backfill
//! (`project_load.rs`). It deliberately never runs anywhere else, so a later
//! merge cannot silently flip a kind under the user. (The backfill also runs
//! where a file is READ as the open path would read it - "Add .stalx"
//! (`project_add.rs`) and the stalx reader - on the read copy only.)
//!
//! # Scope
//!
//! "Never re-runs" is enforced by [`ClassifyScope`], not just by call-site
//! discipline. At import finalize the pass reads the whole graph but may only
//! write to the lines that import created; without that the second import
//! into a project re-kinds the first one's shots, undoing by hand-set choices
//! with no undo entry. Only the post-migration backfill is project-wide.

use std::collections::{HashMap, HashSet};

use crate::models::{LineId, ShotKind, StationRef};

use super::union_find::UnionFind;
use super::Project;

/// Which lines a classification pass is allowed to *write* to.
///
/// The distinction matters because the two halves of a pass have different
/// scopes. Deciding whether a virtual run is bounded is inherently
/// project-wide - a freshly imported run typically reaches real survey only
/// *through* a merge into a line that was already there - but applying the
/// verdict must not reach outside the file being imported, or a per-file
/// setting silently re-kinds shots the user hand-set in an earlier session.
pub enum ClassifyScope<'a> {
    /// Rewrite any virtual shot in the project. Only correct for the one-shot
    /// post-migration backfill, which is deliberately project-wide.
    WholeProject,
    /// Rewrite only shots in these lines - what an import uses. Everything
    /// else still informs the graph, it just never gets written.
    Lines(&'a HashSet<LineId>),
}

impl ClassifyScope<'_> {
    fn allows(&self, line_id: LineId) -> bool {
        match self {
            ClassifyScope::WholeProject => true,
            ClassifyScope::Lines(ids) => ids.contains(&line_id),
        }
    }
}

impl Project {
    /// Classify every virtual shot as [`ShotKind::Jump`] or
    /// [`ShotKind::Reference`] using the chain rule described in the module
    /// docs. Non-virtual shots are never touched.
    ///
    /// The graph is always read project-wide (boundedness usually depends on a
    /// merge into a pre-existing line), but only shots the `scope` admits are
    /// rewritten - see [`ClassifyScope`].
    ///
    /// Returns the number of shots whose kind actually changed, so callers can
    /// skip a redundant save.
    pub fn classify_virtual_shots(&mut self, scope: &ClassifyScope<'_>) -> usize {
        // Index every station slot: a line with N shots has N+1 stations.
        let mut station_slot: HashMap<StationRef, usize> = HashMap::new();
        for line in &self.lines {
            for i in 0..=line.shots.len() {
                let station = StationRef::new(line.id, i as u32);
                let next = station_slot.len();
                station_slot.entry(station).or_insert(next);
            }
        }
        if station_slot.is_empty() {
            return 0;
        }

        // Phase 1: collapse merges and every loop closure, so two station refs
        // pinned to the same physical point become one canonical station.
        let mut canon = UnionFind::new(station_slot.len());
        let merge_pairs = self
            .station_merges
            .iter()
            .map(|m| (&m.station_a, &m.station_b));
        let closure_pairs = self
            .loop_closures
            .iter()
            .map(|c| (&c.station_a, &c.station_b));
        for (key_a, key_b) in merge_pairs.chain(closure_pairs) {
            if let (Some(ref_a), Some(ref_b)) = (self.resolve_key(key_a), self.resolve_key(key_b)) {
                if let (Some(&a), Some(&b)) = (station_slot.get(&ref_a), station_slot.get(&ref_b)) {
                    canon.union(a, b);
                }
            }
        }

        // Phase 2: which canonical stations does real survey touch, and which
        // virtual shots are connected to each other?
        let mut touches_real: HashSet<usize> = HashSet::new();
        let mut components = UnionFind::new(station_slot.len());
        // (line index, shot index, canonical from-slot) for each virtual shot.
        let mut virtual_shots: Vec<(usize, usize, usize)> = Vec::new();

        for (sec_idx, line) in self.lines.iter().enumerate() {
            for (shot_idx, shot) in line.shots.iter().enumerate() {
                let from = StationRef::new(line.id, shot_idx as u32);
                let to = StationRef::new(line.id, (shot_idx + 1) as u32);
                let (Some(&from_slot), Some(&to_slot)) =
                    (station_slot.get(&from), station_slot.get(&to))
                else {
                    continue;
                };
                let (from_canon, to_canon) = (canon.find(from_slot), canon.find(to_slot));

                if shot.kind.is_virtual() {
                    components.union(from_canon, to_canon);
                    virtual_shots.push((sec_idx, shot_idx, from_canon));
                } else {
                    // Both ends of a real shot are places a diver can be.
                    touches_real.insert(from_canon);
                    touches_real.insert(to_canon);
                }
            }
        }

        if virtual_shots.is_empty() {
            return 0;
        }

        // Phase 3: count the DISTINCT canonical stations at which each virtual
        // component meets real survey. Distinctness is what makes a Y-shaped
        // region and a virtual loop that touches real survey once both resolve
        // sensibly: 3 tails is a jump, a loop touching one point is not.
        let mut boundaries: HashMap<usize, HashSet<usize>> = HashMap::new();
        for &real_station in &touches_real {
            let component = components.find(real_station);
            // Only counts if this station is actually part of a virtual
            // component; an isolated real station forms its own singleton.
            boundaries
                .entry(component)
                .or_default()
                .insert(real_station);
        }

        // Phase 4: apply. Collect first so the immutable borrow above ends.
        let verdicts: Vec<(usize, usize, ShotKind)> = virtual_shots
            .into_iter()
            .map(|(sec_idx, shot_idx, from_canon)| {
                let component = components.find(from_canon);
                let bounded = boundaries
                    .get(&component)
                    .is_some_and(|stations| stations.len() >= 2);
                let kind = if bounded {
                    ShotKind::Jump
                } else {
                    ShotKind::Reference
                };
                (sec_idx, shot_idx, kind)
            })
            .collect();

        let mut changed = 0;
        for (sec_idx, shot_idx, kind) in verdicts {
            let Some(line) = self.lines.get_mut(sec_idx) else {
                continue;
            };
            let line_id = line.id;
            if let Some(shot) = line.shots.get_mut(shot_idx) {
                if scope.allows(line_id) && shot.kind != kind {
                    shot.kind = kind;
                    changed += 1;
                }
            }
        }

        if changed > 0 {
            self.touch();
        }
        changed
    }
}

impl Project {
    /// Force every virtual shot in `scope` to one kind, bypassing the chain rule
    /// (#27 q3's per-file import override). Non-virtual shots are untouched.
    /// Returns the number of shots whose kind actually changed.
    pub fn set_all_virtual_shots(&mut self, kind: ShotKind, scope: &ClassifyScope<'_>) -> usize {
        debug_assert!(kind.is_virtual(), "only the virtual kinds make sense here");
        let mut changed = 0;
        for line in &mut self.lines {
            // Line-level predicate since `ClassifyScope` lost its per-shot
            // variant, so it belongs outside the shot loop.
            if !scope.allows(line.id) {
                continue;
            }
            for shot in &mut line.shots {
                if shot.kind.is_virtual() && shot.kind != kind {
                    shot.kind = kind;
                    changed += 1;
                }
            }
        }
        if changed > 0 {
            self.touch();
        }
        changed
    }
}
