//! Merge bookkeeping, alignment checks, the deep cascade and the section-offset dust gate.
//!
//! Lines 10435-11036 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn record_station_merge_after_a_structural_removal_keeps_surviving_merges() {
    // The half-populated-memo trap: a structural removal drops merges while
    // others survive, and the NEXT recorded merge must land on a whole base, not
    // on a map holding only its own edge (which `get_merged_section_ids` would
    // then treat as the entire merge graph, hiding every surviving merge).
    let mut project = Project::new("Cache".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let d = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // An extra, merge-free section: cleaning up ITS references is the structural
    // removal that invalidates the adjacency below.
    let e = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Establish two surviving merges A↔B and B↔C, then read the group so the
    // adjacency memo is warm.
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(b, 0).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });
    assert_eq!(project.get_merged_section_ids(a).len(), 3);

    // The `delete_section` cleanup: a merge retain that keeps A↔B and B↔C.
    project.cleanup_section_references(e);

    // Now record a NEW merge (e.g. the user merges D into the group). It must
    // land on the whole surviving graph, not on a map holding only {C, D}.
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(c, 0).into(),
        station_b: StationRef::new(d, 0).into(),
        merged_at: Utc::now(),
    });
    let merged = project.get_merged_section_ids(a);
    assert!(
        merged.contains(&a) && merged.contains(&b) && merged.contains(&c) && merged.contains(&d),
        "expected the surviving A-B-C merges to still propagate after a structural \
         removal + a new merge recorded; got {merged:?}"
    );
    assert_eq!(merged.len(), 4);
}

#[test]
fn map_station_refs_invalidates_the_merge_adjacency_memo() {
    // `map_station_refs` can move a merge endpoint to a different SECTION - that
    // is what the delete-shot split does - so it rewrites the very graph the
    // merge-adjacency memo describes. Answering a later query from the pre-remap
    // memo reports the wrong rigid group, which is how a bridge closure got
    // promoted into a merge across two GPS-anchored components.
    let mut project = Project::new("Remap".to_string(), None);
    let mk = |project: &mut Project| {
        project
            .add_section(SectionData {
                shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id
    };
    let (a, b, c) = (mk(&mut project), mk(&mut project), mk(&mut project));

    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    // Warm the memo on the pre-remap shape.
    assert_eq!(project.get_merged_section_ids(a), HashSet::from([a, b]));

    // The split's remap: the B endpoint now lives in section C.
    project.map_station_refs(|key| {
        if key.section_id == b {
            StationRef::new(c, 0).into()
        } else {
            key
        }
    });

    assert_eq!(
        project.get_merged_section_ids(a),
        HashSet::from([a, c]),
        "the rigid group must follow the rewritten endpoint into section C"
    );
    assert_eq!(
        project.get_merged_section_ids(b),
        HashSet::from([b]),
        "section B no longer carries a merge endpoint, so it stands alone"
    );
}

#[test]
fn integrity_flags_non_finite_geometry() {
    let mut p = Project::new("NaN".to_string(), None);
    let a = p
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    assert!(p.check_structural_integrity().is_empty());
    // Poison a shot the way a corrupt/older file or unguarded import could.
    p.get_section_mut(a).unwrap().shots[0].length = f64::NAN;
    let v = p.check_structural_integrity();
    assert!(
        v.iter().any(|x| x.category == "non_finite_geometry"),
        "non-finite geometry must be flagged: {v:?}"
    );
    // And the live edit path rejects it.
    assert!(!p.get_section(a).unwrap().shots[0].all_finite());
}

#[test]
fn check_closure_alignment_sees_drift_that_merge_check_cannot() {
    // A: A0-A1-A2 north; B: B0-B1-B2 north; merge A2=B0 (chain). A closure
    // A0=B2 marked aligned, but geometry makes A0 and B2 20m apart - the merge
    // is fine, only the (redundant) closure is stale.
    let mut project = Project::new("StaleClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0), shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0), shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Merge A2 <-> B0 and align (B0 lands on A2).
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    assert!(
        project.check_merge_alignment().is_empty(),
        "merge should be aligned"
    );
    // A closure A0 <-> B2, marked aligned - but A0=(0,0) and B2 is 20m north.
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 2).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    // The merge-only check is blind to it; the closure check catches it.
    assert!(project.check_merge_alignment().is_empty());
    let closure_violations = project.check_closure_alignment();
    assert_eq!(closure_violations.len(), 1, "{closure_violations:?}");
    assert_eq!(closure_violations[0].category, "closure_misaligned");
    assert!(closure_violations[0].message.contains("20.00 m"));

    // An UNaligned closure is inert and not flagged.
    project.loop_closures[0].is_aligned = false;
    assert!(project.check_closure_alignment().is_empty());
}

#[test]
fn cascade_solves_deep_chain_past_old_iteration_cap() {
    // A 40-section chain merged end-to-end, anchored at the datum on the FAR
    // end so propagation must traverse all 40 hops in the worst (backward) edge
    // order. The old 20-iteration fixpoint could silently stop halfway; the
    // single-pass BFS solver must align every merge regardless of depth.
    let mut project = Project::new("DeepChain".to_string(), None);
    let mut ids = Vec::new();
    for _ in 0..40 {
        let id = project
            .add_section(SectionData {
                shots: vec![shot(90.0, 90.0, 3.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        ids.push(id);
    }
    // Chain: section[i] station 1 merged to section[i+1] station 0.
    for w in ids.windows(2) {
        project.record_station_merge(StationMerge {
            station_a: StationRef::new(w[0], 1).into(),
            station_b: StationRef::new(w[1], 0).into(),
            merged_at: Utc::now(),
        });
    }
    // Datum on the LAST section (far end from section[0]).
    let last = *ids.last().unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(last, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Knock every section out of alignment, then re-solve from scratch.
    for &id in &ids {
        project.adjust_section_offset(id, 7.0, -3.0);
    }
    project.realign_merges();
    assert!(
        project.check_merge_alignment().is_empty(),
        "single-pass solver must align all 40 hops regardless of depth/order"
    );
}

#[test]
fn an_adjustment_that_lands_on_dust_leaves_no_section_offset_key() {
    // #413 finding 2, dust half. An adjustment is not exactly invertible, so
    // "move it and move it back" lands a femtometre from where it started
    // rather than on it. On a section with NO offset that femtometre is enough
    // to CREATE a `section_offsets` key - and the key, unlike the value, is
    // structural: it is saved, and the route that never moved has no such key,
    // so two routes to the same geometry stop comparing equal.
    let mut project = Project::new("Dust".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // A round trip whose two halves do not cancel exactly: 0.1 + 0.2 - 0.3 is
    // 5.55e-17, not 0, in IEEE double.
    project.adjust_section_offset(a, 0.1, 0.0);
    project.adjust_section_offset(a, 0.2, 0.0);
    project.adjust_section_offset(a, -0.3, 0.0);
    assert!(
        !project.section_offsets.contains_key(&a),
        "an adjust-and-adjust-back must leave no key, not a dust one: {:?}",
        project.section_offsets.get(&a)
    );

    // Real geometry is untouched: a millimetre is three decades above the bound
    // (#469 re-measured it up from a picometre - see `Project::OFFSET_DUST_M`).
    project.adjust_section_offset(a, 0.001, 0.0);
    assert_eq!(project.section_offsets.get(&a).copied(), Some((0.001, 0.0)));

    // The bound is a normalization on WRITES, and it binds EVERY writer - a
    // restore included. It used to be the other way round (a restore reproduced
    // what it was handed, dust and all, so an undo stayed the exact inverse of
    // what it undid), and that exception is exactly what made a present all-zero
    // key representable: `MoveSection(+d)` then `MoveSection(-d)` then had to end
    // ABSENT from an absent key and PRESENT from an all-zero one, which no writer
    // can decide from its own arguments. The state is gone instead - see the
    // invariant on `Project::write_section_offset` - so a dust value handed to a
    // restore lands ABSENT, which is the only spelling of what it meant.
    project.set_section_offset(a, (1e-15, 0.0));
    assert_eq!(
        project.section_offsets.get(&a).copied(),
        None,
        "set_section_offset must hold the invariant: a dust value is not a position"
    );
    project.set_section_offset(a, (0.002, 0.0));
    let mut captured = HashMap::new();
    captured.insert(a, Some((3e-14, -2e-14)));
    project.restore_section_offsets(&captured);
    assert_eq!(
        project.section_offsets.get(&a).copied(),
        None,
        "restore_section_offsets must hold it too: an entry recorded by a build that predates \
         the invariant may not reinstate the state the invariant removed"
    );
    // And a restore of a REAL value is untouched by any of it.
    let mut captured = HashMap::new();
    captured.insert(a, Some((0.25, -0.5)));
    project.restore_section_offsets(&captured);
    assert_eq!(
        project.section_offsets.get(&a).copied(),
        Some((0.25, -0.5)),
        "a restore of a real displacement must still reproduce it verbatim"
    );
}

/// The worst residue #469's re-measurement found, over every fuzz corpus, at
/// coordinates PAST the largest a georeference can produce: `HORIZON=1` at 3
/// seeds x 5000 moves with the generator's datums scattered worldwide, 1.8e7 m
/// from the frame origin, five ULPs of that magnitude. The full table is on
/// [`Project::OFFSET_DUST_M`].
const WORST_MEASURED_RESIDUE_M: f64 = 4.66e-9;

#[test]
fn the_dust_gate_is_decades_clear_of_both_ends() {
    // `Project::OFFSET_DUST_M` is squeezed from two sides and both are numbers,
    // so both are asserted rather than described.
    //
    // From BELOW: it has to sit clear of the float residue two routes to the
    // same geometry differ by, or a residue lands on either side of it and one
    // route writes a `section_offsets` key the other does not (#469, the
    // `CLOSURE_SHAPES=1` seed 18 failure). Two decades of clearance over the
    // worst residue ever measured is the floor.
    assert!(
        WORST_MEASURED_RESIDUE_M * 100.0 <= Project::OFFSET_DUST_M,
        "the dust gate ({:e}) must stay at least two decades above the worst measured residue \
         ({WORST_MEASURED_RESIDUE_M:e}) - closing that gap is what makes two routes disagree \
         about whether a key exists",
        Project::OFFSET_DUST_M
    );

    // From ABOVE: it is a NORMALIZATION that silently discards what it gates, so
    // it must stay far below the smallest displacement the app treats as real.
    // `MERGE_ALIGNMENT_INVARIANT_M` is that floor - the gap at which a merge
    // stops being aligned as far as the code is concerned - and four decades is
    // the clearance the constant's own doc argues for.
    assert!(
        Project::OFFSET_DUST_M * 1e4 <= Project::MERGE_ALIGNMENT_INVARIANT_M,
        "the dust gate ({:e}) must stay at least four decades below the alignment invariant \
         ({:e}): everything under the gate is thrown away, so it may never approach a \
         displacement the app would call real",
        Project::OFFSET_DUST_M,
        Project::MERGE_ALIGNMENT_INVARIANT_M
    );

    // Those two are the bottom of the chain
    // OFFSET_DUST_M << MERGE_ALIGNMENT_INVARIANT_M <= MERGE_ALIGNMENT_TOLERANCE_M;
    // its top is pinned by
    // `the_self_heal_gate_is_never_looser_than_the_alignment_invariant`, which
    // is where the reason for that half lives (#413 finding 20).
}

#[test]
fn the_dust_gate_clears_the_residue_at_the_georeference_ceiling() {
    // THE measurement `Project::OFFSET_DUST_M` is argued from, re-runnable: how
    // far apart do two merged stations actually land once the survey sits at the
    // largest coordinates a georeference can put it at?
    //
    // That gap is exactly what `cascade_bfs` compares against the gate, and #469
    // item 1 is why it is not a metre-scale question any more: the frame
    // renormalisation translates the whole survey onto whichever datum defines
    // the frame, so the coordinates carry the INTER-DATUM distance. Built here
    // the way the app reaches it - two datums, the first one removed, the second
    // promoted - rather than by injecting a large offset.
    let mut project = Project::new("Ceiling".to_string(), None);

    // A chain of merged sections, deep enough that the cascade BFS sums a real
    // chain of adjustments rather than one hop.
    const CHAIN: usize = 12;
    let chain: Vec<SectionId> = (0..CHAIN)
        .map(|i| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "12 section indices, exact in f64"
            )]
            let turn = (i as f64) * 17.0;
            project
                .add_section(SectionData {
                    shots: vec![
                        shot(turn, turn + 3.0, 13.7, 0.0, 1.1),
                        shot(turn + 41.0, turn + 44.0, 7.3, 1.1, 2.9),
                        shot(turn + 122.0, turn + 125.0, 21.9, 2.9, 0.4),
                    ],
                    ..Default::default()
                })
                .id
        })
        .collect();
    project.ensure_shot_uuids();
    for pair in chain.windows(2) {
        project
            .merge_stations(StationRef::new(pair[0], 3), StationRef::new(pair[1], 0))
            .expect("a fresh chain link is mergeable");
    }

    // The far component, carrying the datum that will define the frame after the
    // first one goes. Nothing merges the two, so the translation below is the
    // only thing that moves the chain.
    let far = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 9.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();

    // Both fixes are valid GPS, so this is a survey the app must hold rather
    // than a corrupt one - and the pair is chosen to MAXIMISE the projection.
    // `gps_to_local`'s ENU turns an angular separation `theta` into a horizontal
    // displacement of `R * sin(theta)`, so a quarter turn of longitude at the
    // equator is the largest one any pair of fixes on this planet can produce:
    // 6.378137e6 m east, `WGS84_A` exactly. A more distant fix projects to LESS,
    // not more, because the separation goes into the discarded up component.
    project
        .set_zero_datum(
            StationRef::new(chain[0], 0),
            GpsCoordinates::new(0.0, -87.35, 0.0).expect("valid fix"),
        )
        .expect("first datum");
    project
        .set_zero_datum(
            StationRef::new(far, 0),
            GpsCoordinates::new(0.0, 2.65, 0.0).expect("valid fix"),
        )
        .expect("second datum");
    // Dropping the frame-defining datum promotes the far one and renormalises
    // onto it (#469 item 1), which is what puts the chain at ~1e7 m. The two
    // calls are what every command that can drop a datum does around the drop
    // (`AppState::renormalize_frame_if_changed`); the model has no epilogue of
    // its own.
    let frame_before = project.frame_datum_key();
    assert!(project.remove_zero_datum(StationRef::new(chain[0], 0)));
    assert!(
        project.renormalize_frame_if_changed(frame_before),
        "the promotion has to translate the survey - without it this test measures nothing"
    );

    // A zero-trust re-solve, the route the loader and Repair take.
    project.resolve_offsets();

    let local = project.section_local_coords();
    let station_offsets = project.resolved_station_offsets();
    let reached = chain
        .iter()
        .flat_map(|&id| {
            let (ox, oy) = project.section_offset(id);
            [ox.abs(), oy.abs()]
        })
        .fold(0.0_f64, f64::max);
    assert!(
        reached > 6.0e6,
        "the fixture has to REACH the coordinates it claims to measure at - the georeference \
         ceiling, ~6.38e6 m - and it only got to {reached:e} m, so a shrunken version of this \
         test would measure nothing"
    );

    let worst_gap = project
        .station_merges
        .iter()
        .filter_map(|merge| {
            project.endpoint_gap(&merge.station_a, &merge.station_b, &local, &station_offsets)
        })
        .map(|(_, _, distance)| distance)
        .fold(0.0_f64, f64::max);
    assert!(
        worst_gap * 100.0 < Project::OFFSET_DUST_M,
        "at {reached:e} m from the frame origin the solved chain left a {worst_gap:e} m gap \
         across a merge, which is within two decades of the dust gate ({:e}). That is the \
         straddle #469 fixed coming back: re-measure the residue and re-argue the constant \
         (its doc carries the table), do not widen an oracle",
        Project::OFFSET_DUST_M
    );

    // And the consequence, which is the defect itself rather than the number
    // behind it: a SECOND cascade over the same geometry, seeded from one
    // section of the chain instead of from every datum-bearing section, must
    // leave the map alone. Those two seed sets sum the same chains in different
    // orders and are entitled to differ in the last bits - what they are not
    // entitled to do is disagree about whether a section HAS a key, which is
    // structural, survives into the `.stalx`, and is exactly what #469's
    // `CLOSURE_SHAPES=1` seed 18 caught the picometre gate doing. Compared as a
    // whole map, so a key appearing on a section the first route left alone
    // fails even where the value is dust.
    let before = project.section_offsets.clone();
    project.cascade_connection_offsets(&[chain[CHAIN / 2]]);
    assert_eq!(
        before, project.section_offsets,
        "a second cascade over one solved geometry, seeded differently, rewrote the offset map \
         at {reached:e} m from the frame origin. Two routes to one geometry must agree on which \
         sections have a key (#469)"
    );
}

#[test]
fn reversing_a_section_twice_leaves_no_section_offset_key() {
    // The dust half's headline route (#413 finding 2, and the metamorphic
    // `reverse-twice-is-identity` pair): the reverse compensates the section
    // offset by the shot run's displacement, and the second reverse sums the
    // negated vectors in the OPPOSITE order, so the round trip lands near zero
    // rather than on it. An un-normalized write kept that femtometre as a key.
    let mut project = Project::new("ReverseTwice".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(37.0, 37.0, 13.7, 0.0, 1.1),
                shot(122.0, 122.0, 7.3, 1.1, 2.9),
                shot(281.0, 281.0, 21.9, 2.9, 0.4),
                shot(19.0, 19.0, 5.5, 0.4, 3.3),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    assert!(!project.section_offsets.contains_key(&a));

    assert!(project.reverse_section(a, false));
    assert!(project.reverse_section(a, false));
    assert!(
        !project.section_offsets.contains_key(&a),
        "reverse twice is the identity, so it must leave the map as it found it: {:?}",
        project.section_offsets.get(&a)
    );
}

#[test]
fn insert_before_and_after_agree_on_boundary_merge_ref() {
    // insert_before(s1) and insert_after(s0) are the SAME physical insertion
    // (a new shot in the array slot between s0 and s1), so a merge anchored
    // exactly on the boundary station must end up referencing the same station
    // index either way. A divergence is an off-by-one in one of the shifts.
    fn build() -> (Project, SectionId, ShotId, ShotId) {
        let mut p = Project::new("Boundary".to_string(), None);
        let a = p
            .add_section(SectionData {
                shots: vec![
                    shot(0.0, 0.0, 5.0, 0.0, 0.0),
                    shot(90.0, 90.0, 5.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        let b = p
            .add_section(SectionData {
                shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        // Merge B0 onto A's MIDDLE station (index 1) - the insertion boundary.
        p.record_station_merge(StationMerge {
            station_a: StationRef::new(a, 1).into(),
            station_b: StationRef::new(b, 0).into(),
            merged_at: Utc::now(),
        });
        let s0 = p.get_section(a).unwrap().shots[0].id;
        let s1 = p.get_section(a).unwrap().shots[1].id;
        (p, a, s0, s1)
    }

    let a_side_index = |p: &Project, a: SectionId| -> u32 {
        let m = p
            .station_merges
            .iter()
            .find(|m| m.station_a.section_id == a || m.station_b.section_id == a)
            .expect("merge");
        let key = if m.station_a.section_id == a {
            &m.station_a
        } else {
            &m.station_b
        };
        p.resolve_key(key)
            .expect("merge endpoint resolves")
            .station_index
    };

    let (mut p_after, a, s0, _) = build();
    p_after.insert_shot_after(a, s0, shot(10.0, 10.0, 3.0, 0.0, 0.0), None, None);
    let after_idx = a_side_index(&p_after, a);

    let (mut p_before, a2, _, s1) = build();
    p_before.insert_shot_before(a2, s1, shot(10.0, 10.0, 3.0, 0.0, 0.0), None, None);
    let before_idx = a_side_index(&p_before, a2);

    assert_eq!(
        after_idx, before_idx,
        "insert_after and insert_before disagree on the boundary merge ref index \
         (after={after_idx}, before={before_idx})"
    );
    // And the merge partner stays geometrically colocated in BOTH paths - the
    // off-by-one re-pointed the merge onto the new station, which still
    // "coincided" but at the wrong place; this checks the merge holds after the
    // insert + cascade.
    assert!(
        p_after.check_merge_alignment().is_empty(),
        "after-path partner displaced"
    );
    assert!(
        p_before.check_merge_alignment().is_empty(),
        "before-path partner displaced"
    );
}
