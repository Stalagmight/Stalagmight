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
//! merge cannot silently flip a kind under the user.
//!
//! # Scope
//!
//! "Never re-runs" is enforced by [`ClassifyScope`], not just by call-site
//! discipline. At import finalize the pass reads the whole graph but may only
//! write to the sections that import created; without that the second import
//! into a project re-kinds the first one's shots, undoing by hand-set choices
//! with no undo entry. Only the post-migration backfill is project-wide.

use std::collections::{HashMap, HashSet};

use crate::{SectionId, ShotKind, StationRef};

use super::union_find::UnionFind;
use super::Project;

/// Which sections a classification pass is allowed to *write* to.
///
/// The distinction matters because the two halves of a pass have different
/// scopes. Deciding whether a virtual run is bounded is inherently
/// project-wide - a freshly imported run typically reaches real survey only
/// *through* a merge into a section that was already there - but applying the
/// verdict must not reach outside the file being imported, or a per-file
/// setting silently re-kinds shots the user hand-set in an earlier session.
pub enum ClassifyScope<'a> {
    /// Rewrite any virtual shot in the project. Only correct for the one-shot
    /// post-migration backfill, which is deliberately project-wide.
    WholeProject,
    /// Rewrite only shots in these sections - what an import uses. Everything
    /// else still informs the graph, it just never gets written.
    Sections(&'a HashSet<SectionId>),
}

impl ClassifyScope<'_> {
    fn allows(&self, section_id: SectionId) -> bool {
        match self {
            ClassifyScope::WholeProject => true,
            ClassifyScope::Sections(ids) => ids.contains(&section_id),
        }
    }
}

impl Project {
    /// Classify every virtual shot as [`ShotKind::Jump`] or
    /// [`ShotKind::Reference`] using the chain rule described in the module
    /// docs. Non-virtual shots are never touched.
    ///
    /// The graph is always read project-wide (boundedness usually depends on a
    /// merge into a pre-existing section), but only shots the `scope` admits are
    /// rewritten - see [`ClassifyScope`].
    ///
    /// Returns the number of shots whose kind actually changed, so callers can
    /// skip a redundant save.
    pub fn classify_virtual_shots(&mut self, scope: &ClassifyScope<'_>) -> usize {
        // Index every station slot: a section with N shots has N+1 stations.
        let mut station_slot: HashMap<StationRef, usize> = HashMap::new();
        for section in &self.sections {
            for i in 0..=section.shots.len() {
                let station = StationRef::new(section.id, i as u32);
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
        // (section index, shot index, canonical from-slot) for each virtual shot.
        let mut virtual_shots: Vec<(usize, usize, usize)> = Vec::new();

        for (sec_idx, section) in self.sections.iter().enumerate() {
            for (shot_idx, shot) in section.shots.iter().enumerate() {
                let from = StationRef::new(section.id, shot_idx as u32);
                let to = StationRef::new(section.id, (shot_idx + 1) as u32);
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
            let Some(section) = self.sections.get_mut(sec_idx) else {
                continue;
            };
            let section_id = section.id;
            if let Some(shot) = section.shots.get_mut(shot_idx) {
                if scope.allows(section_id) && shot.kind != kind {
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
        for section in &mut self.sections {
            // Section-level predicate since `ClassifyScope` lost its per-shot
            // variant, so it belongs outside the shot loop.
            if !scope.allows(section.id) {
                continue;
            }
            for shot in &mut section.shots {
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

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use std::collections::HashSet;

    use super::ClassifyScope;
    use crate::test_fixtures::straight_shot;
    use crate::{
        ClosureStrategy, LoopClosure, Project, SectionData, ShotData, ShotKind, StationMerge,
        StationRef,
    };

    fn new_project() -> Project {
        Project::new("t".to_string(), None)
    }

    fn merge(project: &mut Project, a: StationRef, b: StationRef) {
        project.station_merges.push(StationMerge {
            station_a: a.into(),
            station_b: b.into(),
            merged_at: Utc::now(),
        });
    }

    fn shot(kind: ShotKind) -> ShotData {
        straight_shot(0.0, 10.0).with_kind(kind)
    }

    fn project_with(shots: Vec<ShotData>) -> Project {
        let mut project = new_project();
        project.add_section(SectionData {
            shots,
            ..Default::default()
        });
        project
    }

    fn kinds(project: &Project) -> Vec<ShotKind> {
        project.sections[0].shots.iter().map(|s| s.kind).collect()
    }

    #[test]
    fn bounded_run_becomes_jump() {
        // R V V R -> the virtual run touches real survey on both sides.
        let mut project = project_with(vec![
            shot(ShotKind::Normal),
            shot(ShotKind::Reference),
            shot(ShotKind::Reference),
            shot(ShotKind::Normal),
        ]);

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            2
        );
        assert_eq!(
            kinds(&project),
            vec![
                ShotKind::Normal,
                ShotKind::Jump,
                ShotKind::Jump,
                ShotKind::Normal
            ]
        );
    }

    #[test]
    fn one_sided_run_stays_reference() {
        // R V V <end of section> -> only one real boundary.
        let mut project = project_with(vec![
            shot(ShotKind::Normal),
            shot(ShotKind::Reference),
            shot(ShotKind::Reference),
        ]);

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            0
        );
        assert_eq!(
            kinds(&project),
            vec![ShotKind::Normal, ShotKind::Reference, ShotKind::Reference]
        );
    }

    #[test]
    fn fully_detached_run_stays_reference() {
        let mut project = project_with(vec![shot(ShotKind::Reference), shot(ShotKind::Reference)]);

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            0
        );
        assert_eq!(
            kinds(&project),
            vec![ShotKind::Reference, ShotKind::Reference]
        );
    }

    #[test]
    fn a_jump_demoted_when_it_loses_a_boundary() {
        // Classification is idempotent in both directions: a Jump whose graph
        // does not support it comes back as Reference.
        let mut project = project_with(vec![shot(ShotKind::Normal), shot(ShotKind::Jump)]);

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            1
        );
        assert_eq!(kinds(&project), vec![ShotKind::Normal, ShotKind::Reference]);
    }

    #[test]
    fn bounded_only_through_a_merge_becomes_jump() {
        // Section 0: R V   (virtual tail dangles off real survey)
        // Section 1: R     (separate real survey)
        // Merge section 0's far station to section 1's near station: the
        // virtual shot now reaches real survey on both sides, THROUGH a merge.
        // This is the "even if connected through merge/closure" half of the rule.
        let mut project = new_project();
        project.add_section(SectionData {
            shots: vec![shot(ShotKind::Normal), shot(ShotKind::Reference)],
            ..Default::default()
        });
        project.add_section(SectionData {
            shots: vec![shot(ShotKind::Normal)],
            ..Default::default()
        });
        let sec0 = project.sections[0].id;
        let sec1 = project.sections[1].id;

        // Without the merge it is one-sided.
        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            0
        );
        assert_eq!(project.sections[0].shots[1].kind, ShotKind::Reference);

        merge(
            &mut project,
            StationRef::new(sec0, 2),
            StationRef::new(sec1, 0),
        );

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            1
        );
        assert_eq!(project.sections[0].shots[1].kind, ShotKind::Jump);
    }

    #[test]
    fn bounded_through_a_non_aligned_closure_becomes_jump() {
        // All closures count, not just aligned ones (commit 88ccbaff): a closed
        // loop is a real physical connection whatever strategy reconciled it.
        let mut project = new_project();
        project.add_section(SectionData {
            shots: vec![shot(ShotKind::Normal), shot(ShotKind::Reference)],
            ..Default::default()
        });
        project.add_section(SectionData {
            shots: vec![shot(ShotKind::Normal)],
            ..Default::default()
        });
        let sec0 = project.sections[0].id;
        let sec1 = project.sections[1].id;

        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: StationRef::new(sec0, 2).into(),
            station_b: StationRef::new(sec1, 0).into(),
            path_length: 1.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Batch,
            // Deliberately NOT aligned: 88ccbaff established that every
            // closure is a real physical connection.
            is_aligned: false,
        });

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            1
        );
        assert_eq!(project.sections[0].shots[1].kind, ShotKind::Jump);
    }

    #[test]
    fn y_shaped_virtual_region_is_a_jump() {
        // Three real tails meeting a shared virtual region: 3 distinct
        // boundaries, comfortably >= 2, so the whole region is a jump.
        let mut project = new_project();
        for _ in 0..3 {
            project.add_section(SectionData {
                shots: vec![shot(ShotKind::Normal), shot(ShotKind::Reference)],
                ..Default::default()
            });
        }
        let ids: Vec<_> = project.sections.iter().map(|s| s.id).collect();

        // Merge all three virtual tips together into one hub.
        for pair in ids.windows(2) {
            merge(
                &mut project,
                StationRef::new(pair[0], 2),
                StationRef::new(pair[1], 2),
            );
        }

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            3
        );
        for section in &project.sections {
            assert_eq!(section.shots[1].kind, ShotKind::Jump);
        }
    }

    #[test]
    fn virtual_loop_touching_real_once_stays_reference() {
        // A virtual chain whose two ends merge back to the SAME real station
        // has one distinct boundary, not two, so it is not a jump.
        let mut project = new_project();
        project.add_section(SectionData {
            shots: vec![
                shot(ShotKind::Normal),
                shot(ShotKind::Reference),
                shot(ShotKind::Reference),
            ],
            ..Default::default()
        });
        let sec = project.sections[0].id;

        // Close the virtual tail back onto the real survey's own station 1.
        merge(
            &mut project,
            StationRef::new(sec, 3),
            StationRef::new(sec, 1),
        );

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            0
        );
        assert_eq!(
            kinds(&project),
            vec![ShotKind::Normal, ShotKind::Reference, ShotKind::Reference]
        );
    }

    #[test]
    fn non_virtual_shots_are_never_touched() {
        let mut project = project_with(vec![shot(ShotKind::Normal), shot(ShotKind::Normal)]);
        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            0
        );
        assert_eq!(kinds(&project), vec![ShotKind::Normal, ShotKind::Normal]);
    }

    #[test]
    fn classification_is_idempotent() {
        let mut project = project_with(vec![
            shot(ShotKind::Normal),
            shot(ShotKind::Reference),
            shot(ShotKind::Normal),
        ]);

        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            1
        );
        // Second run changes nothing, so a caller can use the count to skip a save.
        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            0
        );
        assert_eq!(project.sections[0].shots[1].kind, ShotKind::Jump);
    }

    #[test]
    fn set_all_virtual_shots_overrides_the_chain_rule_without_touching_real_shots() {
        let mut project = project_with(vec![
            shot(ShotKind::Normal),
            shot(ShotKind::Reference),
            shot(ShotKind::Normal),
        ]);

        // The chain rule would call this bounded run a Jump; the override wins.
        assert_eq!(
            project.set_all_virtual_shots(ShotKind::Reference, &ClassifyScope::WholeProject),
            0
        );
        assert_eq!(
            project.set_all_virtual_shots(ShotKind::Jump, &ClassifyScope::WholeProject),
            1
        );
        assert_eq!(
            kinds(&project),
            vec![ShotKind::Normal, ShotKind::Jump, ShotKind::Normal],
            "permanent shots must never be virtualised by the override"
        );
    }

    #[test]
    fn scoped_pass_reads_the_whole_graph_but_only_writes_its_own_sections() {
        // The regression this pins: importing a second file must not re-kind the
        // first one's shots. Section 0 is "already in the project" with a
        // hand-set Reference; section 1 is the import, and its virtual shot is
        // only bounded *through* a merge into section 0 - so the read has to stay
        // project-wide even though the write must not.
        let mut project = new_project();
        project.add_section(SectionData {
            shots: vec![shot(ShotKind::Normal), shot(ShotKind::Reference)],
            ..Default::default()
        });
        project.add_section(SectionData {
            shots: vec![shot(ShotKind::Reference), shot(ShotKind::Normal)],
            ..Default::default()
        });
        let sec0 = project.sections[0].id;
        let sec1 = project.sections[1].id;
        // Section 0's virtual tip meets section 1's virtual head, so the two
        // virtual shots form one run bounded by real survey at both ends.
        merge(
            &mut project,
            StationRef::new(sec0, 2),
            StationRef::new(sec1, 0),
        );

        let imported: HashSet<_> = [sec1].into_iter().collect();
        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::Sections(&imported)),
            1,
            "only the imported section's shot may be rewritten"
        );
        assert_eq!(
            project.sections[0].shots[1].kind,
            ShotKind::Reference,
            "the pre-existing hand-set kind must survive a later import"
        );
        assert_eq!(
            project.sections[1].shots[0].kind,
            ShotKind::Jump,
            "boundedness still resolves through the pre-existing section"
        );
    }

    #[test]
    fn the_all_x_override_is_scoped_too() {
        let mut project = new_project();
        project.add_section(SectionData {
            shots: vec![shot(ShotKind::Reference)],
            ..Default::default()
        });
        project.add_section(SectionData {
            shots: vec![shot(ShotKind::Reference)],
            ..Default::default()
        });
        let sec1 = project.sections[1].id;

        let imported: HashSet<_> = [sec1].into_iter().collect();
        assert_eq!(
            project.set_all_virtual_shots(ShotKind::Jump, &ClassifyScope::Sections(&imported)),
            1
        );
        assert_eq!(project.sections[0].shots[0].kind, ShotKind::Reference);
        assert_eq!(project.sections[1].shots[0].kind, ShotKind::Jump);
    }

    #[test]
    fn empty_project_is_a_no_op() {
        let mut project = new_project();
        assert_eq!(
            project.classify_virtual_shots(&ClassifyScope::WholeProject),
            0
        );
    }
}
