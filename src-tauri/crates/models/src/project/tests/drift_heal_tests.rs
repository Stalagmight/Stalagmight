//! Healing offset drift: the redrive after load, the dangling-datum guards and the realign-merges
//! self-heal.
//!
//! Lines 11037-11871 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn redrive_offsets_after_load_heals_persisted_drift() {
    // Simulate a file persisted with a drifted offset (older redo bug) plus
    // empty transient caches (as after deserialization).
    let mut project = Project::new("Loaded".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    // Persisted drift + an empty section index (as after deserialization).
    project.adjust_section_offset(b, 31.0, -9.0);
    project.section_index_cache.clear();
    assert!(!project.check_merge_alignment().is_empty());

    let healed = project.redrive_offsets_after_load();
    assert_eq!(healed, 1);
    assert!(
        project.check_merge_alignment().is_empty(),
        "load must open geometrically correct"
    );
}

#[test]
fn redrive_dedups_uuids_before_reanchor_preserving_second_duplicate_ref() {
    // Regression: when a loaded section already has two shots sharing a UUID, the
    // load-time canonicalization of positional Index anchors to AfterShot(uuid)
    // MUST run after de-duplicating UUIDs. Otherwise a ref at the station after the
    // SECOND duplicate canonicalizes to AfterShot(dup), which resolve_key resolves
    // to the FIRST duplicate - silently moving the ref off its endpoint and
    // defeating Repair.
    let mut project = Project::new("Dup".to_string(), None);
    let s = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Force a duplicate: shots[0] and shots[1] share a UUID, as a corrupt/legacy
    // file could persist. (ensure_unique keeps the first and re-mints the second.)
    let dup = project.get_section(s).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    project.get_section_mut(s).unwrap().shots[1].uuid = Some(dup.clone());

    // A station offset stored in the legacy positional form at station 2 - the
    // station immediately AFTER the second duplicate shot.
    project
        .station_offsets
        .insert(StationKey::new(s, StationAnchor::Index(2)), (1.0, 2.0, 3.0));

    project.redrive_offsets_after_load();

    // The offset must still resolve to station 2, not collapse onto station 1.
    let (key, _) = project
        .station_offsets
        .iter()
        .next()
        .expect("offset survived load");
    assert!(
        project.key_is_ref(key, StationRef::new(s, 2)),
        "offset ref must stay on station 2 after dedup+reanchor, resolved to {:?}",
        project.resolve_key(key)
    );
    // And the duplicate was actually repaired (no two shots share a UUID).
    let uuids: Vec<_> = project
        .get_section(s)
        .unwrap()
        .shots
        .iter()
        .filter_map(|sh| sh.uuid.clone())
        .collect();
    let mut uniq = uuids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(
        uuids.len(),
        uniq.len(),
        "duplicate shot UUID repaired on load"
    );
}

#[test]
fn resolve_offsets_does_not_touch_when_already_consistent() {
    // Opening a byte-clean project re-solves to identical offsets; resolve_offsets
    // must NOT bump modified_at then - otherwise a later no-edit save persists a
    // bogus timestamp and a no-op integrity Repair looks like a real change.
    let mut project = Project::new("Clean".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    // First solve makes the offsets consistent (and legitimately touches).
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty());
    let stamp = project.modified_at;

    // A second re-solve of the already-consistent project changes no offset, so it
    // must leave modified_at exactly as it was (no touch).
    let healed = project.resolve_offsets();
    assert_eq!(healed, 0, "nothing to heal");
    assert_eq!(
        project.modified_at, stamp,
        "resolve_offsets must not touch a clean project"
    );

    // And a genuine drift still touches (offsets change → modified_at advances).
    project.adjust_section_offset(b, 10.0, -4.0);
    let pre = project.modified_at;
    let healed2 = project.resolve_offsets();
    assert_eq!(healed2, 1, "the injected drift is healed");
    assert_ne!(project.modified_at, pre, "a real re-solve touches");
}

#[test]
fn section_has_zero_datum_ignores_dangling_datum() {
    // A corrupt project can be opened with a DANGLING zero datum (anchor shot
    // deleted) so the user can repair it. It must NOT count as a real anchor - else
    // a merge picks the phantom-anchored section as the fixed group and shifts the
    // world coordinate system around a station that doesn't resolve.
    let mut project = Project::new("DanglingDatum".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // A dangling datum: anchored on a shot UUID that no shot carries.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });

    assert!(
        !project.section_has_zero_datum(a),
        "a dangling zero datum must not count as a merge anchor"
    );
    // Sanity: it is flagged as a dangling cross-reference.
    assert!(
        project
            .check_structural_integrity()
            .iter()
            .any(|v| v.category == "dangling_zero_datum_ref"),
        "dangling datum should be flagged by the integrity check"
    );
}

#[test]
fn has_datum_path_excluding_ignores_dangling_datum() {
    // A dangling datum (anchor key unresolvable) must NOT count as a reachable
    // anchor in has_datum_path_excluding - same resolve-aware invariant as
    // section_has_zero_datum (#174). Without the resolve guard the BFS reports a
    // datum path that doesn't actually anchor anything.
    let mut project = Project::new("DanglingPath".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A second, datum-less section to use as `excluded` (≠ a, so `a` is visited).
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(
        !project.has_datum_path_excluding(a, b),
        "a dangling datum must not count as a reachable anchor"
    );
}

#[test]
fn realign_after_shot_edit_ignores_dangling_datum() {
    // With only a dangling datum in the merge group, realign must treat the group
    // as UNANCHORED - it must not pin the group to the phantom-anchor section
    // (#174). Unanchored no longer means "do nothing" (#413 finding 20: a group
    // with no absolute anchor still has relative merge constraints, and skipping
    // them left the merge drifted by exactly the edit): it means anchor on the
    // EDITED section instead. So the edited section is the one that must not
    // move, and the merge must come out aligned around it.
    let mut project = Project::new("DanglingRealign".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Merge A1 = B0 so A and B share a rigid group, then drift B off the merge.
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project.adjust_section_offset(b, 3.0, 4.0);
    // Dangling datum on section A (anchor key unresolvable).
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    let before = project.section_offset(b);
    project.realign_after_shot_edit(b, None);
    let after = project.section_offset(b);
    assert_eq!(
        before, after,
        "the EDITED section must not move when the only datum in the group is \
         dangling - realigning around a phantom anchor would move it"
    );
    // ...and the group is realigned around it rather than left broken.
    let (ax, ay) = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .map(|c| (c.x, c.y))
        .expect("A:1 has no raw coordinates");
    let (bx, by) = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .map(|c| (c.x, c.y))
        .expect("B:0 has no raw coordinates");
    let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "an unanchored group must still be realigned against the edited section: \
         the merged pair is {gap:.6} m apart"
    );
}

#[test]
fn rigid_group_has_datum_ignores_dangling_datum() {
    // A dangling datum must not mark a merge group as anchored - otherwise a
    // move-drag is wrongly blocked on an actually-unanchored group (#174; the
    // guard sits at command_move's "merge group holds a zero datum" check).
    let mut project = Project::new("DanglingRigid".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(
        !project.rigid_group_has_datum(a),
        "a dangling datum must not count as a merge-group anchor"
    );
}

#[test]
fn cascade_connection_offsets_ignores_dangling_datum() {
    // B holds a real datum; A holds a DANGLING datum, is merged to B, and is
    // drifted off the merge. Without the resolve guard A is wrongly frozen as
    // authoritative, so the A=B merge (two "fixed" sections) can't be pulled back
    // into alignment. With the guard only B anchors and A cascades onto it (#174).
    let mut project = Project::new("DanglingCascade".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Merge A1 = B0 (direct push avoids merge_stations' auto-alignment).
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    // Real datum on B (the genuine anchor) + dangling datum on A.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Drift A off the merge; precondition: misaligned.
    project.adjust_section_offset(a, 7.0, -3.0);
    assert!(
        !project.check_merge_alignment().is_empty(),
        "precondition: the A=B merge starts misaligned"
    );
    // Cascade anchored at B; A must follow (not stay frozen on a phantom datum).
    project.cascade_connection_offsets(&[b]);
    assert!(
        project.check_merge_alignment().is_empty(),
        "cascade must realign A onto B; a dangling datum on A must not freeze it"
    );
}

#[test]
fn check_merge_alignment_detects_and_realign_fixes_drift() {
    // Two sections, A0-A1 and B0-B1, with A1 merged to B0.
    let mut project = Project::new("Drift".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // Anchor A at a zero datum so realign has an authority.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Merge A1 <-> B0 (merge_stations computes the aligning offset).
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    assert!(
        project.check_merge_alignment().is_empty(),
        "merge should start aligned"
    );

    // Corrupt section B's offset directly (simulating a missed cascade).
    project.adjust_section_offset(b, 12.0, -7.0);
    let drift = project.check_merge_alignment();
    assert_eq!(drift.len(), 1, "{drift:?}");
    assert_eq!(drift[0].category, "merge_misaligned");
    assert!(drift[0].message.contains(" m apart"));

    // Repair realigns it.
    assert_eq!(project.realign_merges(), 1);
    assert!(project.check_merge_alignment().is_empty());
}

#[test]
fn realign_corrects_sub_tolerance_offset_drift() {
    // Zero-trust offsets: a stale offset too small for the per-command self-heal
    // to act on reports NO merge-alignment violation (so the old
    // `if before == 0` early-return skipped the cascade and the wrong offset
    // survived load + every command), yet it IS wrong. realign must re-derive it
    // regardless of the reported drift count.
    //
    // The magnitude that demonstrates it is 3 MM rather than the 3 cm this test
    // used before #413 finding 20: the self-heal gate moved from the 5 cm
    // REPORTING tolerance down to `MERGE_ALIGNMENT_INVARIANT_M` (1 cm), because
    // the 1-5 cm band it used to skip is a band `debug_assert_merge_alignment`
    // panics on. The blind spot this test is about is smaller now, not gone.
    let mut project = Project::new("SubTol".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    let b_correct = project.section_offset(b);

    // Nudge B by 3 mm - under the 1 cm invariant the self-heal gates on.
    project.adjust_section_offset(b, 0.003, 0.0);
    assert!(
        project.check_merge_alignment().is_empty(),
        "3 mm drift is under the 5 cm report tolerance (the early-return's blind spot)"
    );
    assert!(
        (project.section_offset(b).0 - b_correct.0).abs() > 0.002,
        "precondition: B is actually drifted"
    );
    // The per-command self-heal still early-returns here (nothing past the
    // invariant)...
    assert_eq!(
        project.realign_merges(),
        0,
        "realign_merges keeps the no-drift early-return"
    );
    assert!(
        (project.section_offset(b).0 - b_correct.0).abs() > 0.002,
        "...so the sub-invariant drift survives the per-command path"
    );

    // ...but the zero-trust load/repair re-solve corrects it despite zero drifts.
    project.resolve_offsets();
    let b_after = project.section_offset(b);
    assert!(
        (b_after.0 - b_correct.0).abs() < 1e-9 && (b_after.1 - b_correct.1).abs() < 1e-9,
        "sub-tolerance drift must be corrected: correct={b_correct:?} after={b_after:?}"
    );
}

#[test]
fn the_self_heal_gate_is_never_looser_than_the_alignment_invariant() {
    // #413 finding 20 IS this inequality being violated. The self-heal
    // (`realign_merges`) gated on the 5 cm REPORTING tolerance while
    // `debug_assert_merge_alignment` asserted at 1 cm, so the whole band between
    // them was a state the app declares impossible and declines to repair: a
    // release build saved it silently, and the next command in a test build
    // panicked on a merge it had never touched.
    //
    // Pinned as an ORDERING rather than as two literals, because the numbers
    // answer different questions and are allowed to move - what is not allowed
    // is for the repair threshold to end up looser than the assert's again.
    assert!(
        Project::MERGE_ALIGNMENT_INVARIANT_M <= Project::MERGE_ALIGNMENT_TOLERANCE_M,
        "the invariant the app asserts ({}) must be at least as tight as the tolerance it \
         reports ({}) - and `realign_merges` must gate on the invariant",
        Project::MERGE_ALIGNMENT_INVARIANT_M,
        Project::MERGE_ALIGNMENT_TOLERANCE_M
    );

    // And the gate really is the invariant, not the tolerance: a drift inside
    // the old blind band must be healed by the per-command path.
    let mut project = Project::new("HealGate".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    // 3 cm: past the invariant, under the reporting tolerance - the band.
    project.adjust_section_offset(b, 0.03, 0.0);
    assert!(
        project.check_merge_alignment().is_empty(),
        "precondition: 3 cm is under the 5 cm the user is told about"
    );
    project.realign_merges();
    let gap = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .zip(project.compute_station_raw_coords(StationRef::new(b, 0)))
        .map(|(left, right)| ((left.x - right.x).powi(2) + (left.y - right.y).powi(2)).sqrt())
        .expect("merged stations have no raw coordinates");
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "the per-command self-heal left the pair {gap:.6} m apart - a drift the app's own \
         assert calls a violation and its repair declined to fix (#413 finding 20)"
    );
}

/// A merge component holding a gap the offset solver CANNOT close, plus one
/// bystander it can move.
///
/// A and B are merged and each carries its own zero datum - two GPS anchors in
/// one rigid group. `set_zero_datum`/`merge_stations` guard against building that
/// interactively, so it is assembled by pushing the datums directly, which is how
/// it actually arrives: a file written before the guards, or a repair. The
/// cascade marks every datum section authoritative and therefore reaches the A=B
/// merge from both sides and adjusts neither, so the 2 cm gap opened below
/// survives every re-solve, forever.
///
/// C hangs off the same component with no datum of its own, so it IS something
/// the cascade moves - the bystander that shows whether a re-solve ran.
fn over_anchored_irreducible_gap() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("Overanchored".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Both merges start exactly aligned - merge_stations computes the offset.
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(c, 0), StationRef::new(a, 0))
        .unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    // Pull B 2 cm off the merge: past the 1 cm invariant the self-heal gates on,
    // under the 5 cm the user is told about.
    project.adjust_section_offset(b, 0.0, -0.02);
    (project, a, b, c)
}

#[test]
fn realign_merges_skips_a_residual_the_solver_has_proven_irreducible() {
    // `reconcile_geometry` calls `realign_merges` after EVERY command and every
    // undo/redo. With an irreducible beyond-invariant gap in the project the gate
    // is non-empty forever, so before the fixpoint fingerprint every keystroke-scale
    // edit paid a full `resolve_offsets` cascade - which is also the one thing that
    // overrides the exact offsets an undo snapshot has just restored.
    let (mut project, _a, _b, c) = over_anchored_irreducible_gap();
    assert_eq!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len(),
        1,
        "precondition: exactly one merge is past the invariant"
    );
    assert!(
        project.check_merge_alignment().is_empty(),
        "precondition: and it is under the 5 cm reporting tolerance, so the healed \
         count stays 0 either way"
    );

    // Pass one runs the full solve, watches it move nothing, and records the
    // fixpoint.
    assert_eq!(project.realign_merges(), 0);
    assert!(
        project.realign_fixpoint.is_some(),
        "a solve that cleared nothing and moved nothing IS the fixpoint"
    );
    assert_eq!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len(),
        1,
        "the gap really is irreducible - the solver has now run on it"
    );

    // Drift the bystander by less than the invariant, so the residual set and every
    // gap bit stay exactly as fingerprinted (and nothing structural changes).
    project.adjust_section_offset(c, 0.003, 0.0);
    let drifted = project.section_offset(c);
    assert!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len()
            == 1,
        "3 mm keeps C out of the residual set"
    );

    assert_eq!(project.realign_merges(), 0);
    assert_eq!(
        project.section_offset(c),
        drifted,
        "the fingerprint matched, so the re-solve must have been skipped entirely"
    );

    // And the skipped work was real work: the same solve, run explicitly, pulls C
    // back. That is what makes this a skip rather than a coincidence.
    project.resolve_offsets();
    assert!(
        (project.section_offset(c).0 - drifted.0).abs() > 0.002,
        "the solve the skip declined would have moved C: before={drifted:?} after={:?}",
        project.section_offset(c)
    );
}

#[test]
fn realign_merges_re_solves_once_the_residual_becomes_healable() {
    // The other half of the contract: a structural edit can make an irreducible
    // residual healable WITHOUT moving a station or changing a single gap bit, so
    // the fingerprint carries the structure too. Dropping one of the two anchors is
    // exactly that edit - the component becomes solvable and the next realign must
    // do the work.
    let (mut project, _a, b, c) = over_anchored_irreducible_gap();
    assert_eq!(project.realign_merges(), 0);
    assert!(project.realign_fixpoint.is_some());
    project.adjust_section_offset(c, 0.003, 0.0);
    let drifted = project.section_offset(c);
    assert_eq!(project.realign_merges(), 0);
    assert_eq!(
        project.section_offset(c),
        drifted,
        "precondition: the skip is armed"
    );

    assert!(project.remove_zero_datum(StationRef::new(b, 0)));
    project.realign_merges();
    assert!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .is_empty(),
        "with one anchor left the cascade can pull the merge together - and must \
         not be skipped for a fingerprint recorded while it could not"
    );
    assert!(
        project.section_offset(c).0.abs() < 1e-9,
        "the re-solve ran in full, so the bystander drift is gone too: {:?}",
        project.section_offset(c)
    );
}

#[test]
fn realign_merges_re_solves_once_drift_crosses_the_invariant() {
    // The load-bearing half of the fixpoint's safety argument, asserted rather
    // than argued: the skip may leave SUB-invariant drift alone (the test two
    // above), but drift that CROSSES `MERGE_ALIGNMENT_INVARIANT_M` puts a new
    // pair in the residual set, and the set is compared by identity - so the
    // fingerprint misses and the full solve runs. That is what makes the state
    // `debug_assert_merge_alignment` panics on unreachable while the skip is
    // armed.
    let (mut project, _a, _b, c) = over_anchored_irreducible_gap();
    assert_eq!(project.realign_merges(), 0);
    assert!(project.realign_fixpoint.is_some(), "precondition: armed");
    let anchored_x = project.section_offset(c).0;
    project.adjust_section_offset(c, 0.003, 0.0);
    assert_eq!(project.realign_merges(), 0);
    assert!(
        (project.section_offset(c).0 - (anchored_x + 0.003)).abs() < 1e-12,
        "precondition: 3 mm is under the invariant, so the skip holds it - C is at {:?}",
        project.section_offset(c)
    );

    // Push the same bystander past the invariant - one edit, no structural
    // change, and every EXISTING residual gap still bit-identical.
    project.adjust_section_offset(c, 0.017, 0.0);
    assert_eq!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len(),
        2,
        "precondition: C's merge has now crossed, so there are two residuals"
    );

    project.realign_merges();
    assert!(
        (project.section_offset(c).0 - anchored_x).abs() < 1e-9,
        "a gap past the invariant must never be left to the skip: C is still at {:?}",
        project.section_offset(c)
    );
    assert_eq!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len(),
        1,
        "the solve ran and healed everything it could - only the irreducible gap is left"
    );
}

#[test]
fn realign_merges_still_skips_for_an_edit_in_a_disjoint_component() {
    // The other side of the same contract: the skip exists so that ONE
    // irreducible gap does not make every later edit pay a whole-project
    // re-solve. An edit in a component that shares nothing with the residual
    // must still hit it.
    let (mut project, _a, b, _c) = over_anchored_irreducible_gap();
    // A second, healthy component: D anchored by its own datum, E merged onto
    // it. Built BEFORE the fixpoint is recorded - adding a section is a
    // structural change, and the structure hash would (correctly) reject a
    // fingerprint taken before it. Built while the A=B gap is CLOSED, too:
    // `merge_stations` runs `debug_assert_merge_alignment`, which a test build
    // panics on for the very residual this project is about, so the helper's
    // 2 cm is backed out and re-applied around the new component.
    project.adjust_section_offset(b, 0.0, 0.02);
    let d = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let e = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project
        .merge_stations(StationRef::new(d, 1), StationRef::new(e, 0))
        .unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(d, 0).into(),
        gps: GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
    });
    project.adjust_section_offset(b, 0.0, -0.02);

    assert_eq!(project.realign_merges(), 0);
    assert!(project.realign_fixpoint.is_some(), "precondition: armed");

    project.adjust_section_offset(e, 0.003, 0.0);
    let drifted = project.section_offset(e);
    assert_eq!(project.realign_merges(), 0);
    assert_eq!(
        project.section_offset(e),
        drifted,
        "an edit two components away must still take the skip - that is the whole \
         optimization"
    );
    // And the declined solve was real work here too, so the skip is doing
    // something rather than agreeing with a no-op.
    project.resolve_offsets();
    assert!(
        (project.section_offset(e).0 - drifted.0).abs() > 0.002,
        "the solve the skip declined would have moved E: before={drifted:?} after={:?}",
        project.section_offset(e)
    );
}

#[test]
fn a_whole_rigid_group_translation_is_a_solver_no_op() {
    // Why the fingerprint does NOT need the residual component's own offsets in
    // it, which looks like a hole until the cascade is traced: a whole-group
    // move leaves every residual gap and the structure hash bit-identical, so
    // the skip fires - and it hides nothing, because the solve it declines is
    // itself a no-op. `cascade_connection_offsets` derives each
    // non-authoritative section from an authoritative one's STORED offset and
    // never re-derives an authoritative section at all (a datum section is
    // pinned WHERE IT IS, not re-anchored onto its GPS), so translating every
    // section of a component by one delta lands exactly where the cascade would
    // put them.
    let (mut project, a, b, c) = over_anchored_irreducible_gap();
    assert_eq!(project.realign_merges(), 0);
    assert!(project.realign_fixpoint.is_some(), "precondition: armed");

    // The move tools' own path, not a hand-rolled loop over the ids.
    project.adjust_rigid_group_offsets(a, 3.0, -2.0);
    let moved: Vec<(f64, f64)> = [a, b, c]
        .iter()
        .map(|s| project.section_offset(*s))
        .collect();

    let mut solved = project.clone();
    solved.resolve_offsets();
    let after_solve: Vec<(f64, f64)> = [a, b, c]
        .iter()
        .map(|s| solved.section_offset(*s))
        .collect();
    assert_eq!(
        moved, after_solve,
        "a full solve after a whole-group translation must change nothing - if this \
         ever fails, the skip really would be hiding a re-anchor and the fingerprint \
         needs the component's offsets in it"
    );

    project.realign_merges();
    let after_skip: Vec<(f64, f64)> = [a, b, c]
        .iter()
        .map(|s| project.section_offset(*s))
        .collect();
    assert_eq!(
        after_skip, after_solve,
        "so the skipped state and the solved state are the same state"
    );
}

#[test]
fn realign_preserves_floating_section_offset() {
    // A datum-less, merge-less (floating) section's absolute position is
    // non-derivable (free placement). The always-on re-solve must NOT zero it -
    // it's a cascade seed/anchor, never seed-adjusted.
    let mut project = Project::new("Float".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.adjust_section_offset(a, 12.0, 7.0);
    project.resolve_offsets(); // the unconditional re-solve
    assert_eq!(
        project.section_offset(a),
        (12.0, 7.0),
        "a free-placed floating section keeps its offset across an unconditional re-solve"
    );
}
