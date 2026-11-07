//! The structural integrity check and repair.
//!
//! Lines 12921-13209 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`). `integrity_fixture` (former lines
//! 11872-11912, in `orphan_closure_tests.rs`'s range) leads this file: only it uses that helper.

use super::*;

// === Structural integrity check ===

/// A two-section project with one valid merge, zero datum, and loop closure.
fn integrity_fixture() -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Integrity".to_string(), None);
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
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.2,
        error_percentage: 2.0,
        horizontal_misclosure: 0.15,
        vertical_misclosure: 0.05,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    (project, a, b)
}

#[test]
fn repair_structural_integrity_drops_dangling_refs_and_keeps_valid() {
    let (mut project, a, b) = integrity_fixture();
    // Corrupt: a dangling merge, closure, datum, plus orphan offset/comment.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(9999, 0).into(),
        merged_at: Utc::now(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(9999, 0).into(),
        gps: GpsCoordinates::new(1.0, 2.0, 3.0).unwrap(),
    });
    project.section_offsets.insert(9999, (1.0, 1.0));

    let valid_merges_before = project.station_merges.len();
    let repaired = project.repair_structural_integrity();

    // It reported the violations it fixed and the project is now sound.
    assert!(!repaired.is_empty());
    assert!(project.check_structural_integrity().is_empty());
    assert!(!project.has_critical_integrity_violations());
    // The originally-valid merge/datum/closure survive (only the bad ones dropped).
    assert_eq!(project.station_merges.len(), valid_merges_before - 1);
    assert_eq!(project.loop_closures.len(), 1);
    assert!(project.get_section(a).is_some() && project.get_section(b).is_some());
    // Repairing a clean project is a no-op.
    assert!(project.repair_structural_integrity().is_empty());
}

#[test]
fn integrity_clean_project_has_no_violations() {
    let (project, _, _) = integrity_fixture();
    assert!(project.check_structural_integrity().is_empty());
    assert!(!project.has_critical_integrity_violations());
}

#[test]
fn integrity_flags_merge_to_missing_section() {
    let (mut project, a, _) = integrity_fixture();
    // Point the merge at a section id that doesn't exist.
    forge_first_merge(&mut project, |m| {
        m.station_b = StationRef::new(9999, 0).into();
    });
    let v = project.check_structural_integrity();
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].severity, ViolationSeverity::Critical);
    assert_eq!(v[0].category, "dangling_merge_ref");
    assert!(project.has_critical_integrity_violations());
    // The valid endpoint (section a) is untouched.
    assert!(project.get_section(a).is_some());
}

#[test]
fn integrity_flags_station_index_out_of_range() {
    let (mut project, a, _) = integrity_fixture();
    // Section a has 1 shot → stations 0..=1; index 5 is out of range, so the
    // transitional Index anchor no longer resolves → flagged as a dangling ref.
    forge_first_merge(&mut project, |m| {
        m.station_a = StationRef::new(a, 5).into();
    });
    let v = project.check_structural_integrity();
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].severity, ViolationSeverity::Critical);
    assert!(v[0].message.contains("no longer resolves"), "{v:?}");
}

#[test]
fn integrity_flags_dangling_closure_and_zero_datum() {
    let (mut project, _, _) = integrity_fixture();
    project.loop_closures[0].station_b = StationRef::new(9999, 0).into();
    project.zero_datums[0].station_ref = StationRef::new(9999, 0).into();
    let v = project.check_structural_integrity();
    let categories: Vec<&str> = v.iter().map(|x| x.category.as_str()).collect();
    assert!(categories.contains(&"dangling_closure_ref"), "{v:?}");
    assert!(categories.contains(&"dangling_zero_datum_ref"), "{v:?}");
    assert!(v.iter().all(|x| x.severity == ViolationSeverity::Critical));
}

#[test]
fn integrity_orphan_offsets_and_comments_are_warnings_only() {
    let (mut project, _, _) = integrity_fixture();
    project.section_offsets.insert(9999, (1.0, 2.0));
    project
        .station_offsets
        .insert(StationRef::new(9999, 0).into(), (1.0, 2.0, 3.0));
    project
        .station_comments
        .insert(StationRef::new(9999, 0).into(), "x".to_string());
    let v = project.check_structural_integrity();
    assert_eq!(v.len(), 3, "{v:?}");
    assert!(v.iter().all(|x| x.severity == ViolationSeverity::Warning));
    // Warnings alone don't trip the critical gate.
    assert!(!project.has_critical_integrity_violations());
}

#[test]
fn check_structural_integrity_flags_and_repairs_duplicate_shot_uuid() {
    // Two shots in one section sharing a UUID make AfterShot anchors mis-resolve
    // (resolve_key takes the first match). It must be a Critical violation and
    // repair must re-mint the duplicate while KEEPING the first (so anchors that
    // resolved to it stay valid).
    let mut project = Project::new("Dup".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let first_uuid = project.get_section(a).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    // Force shot 1 to collide with shot 0's UUID.
    project.get_section_mut(a).unwrap().shots[1].uuid = Some(first_uuid.clone());

    // A merge anchored on the first station (a:1 == AfterShot(first_uuid)).
    let anchor = StationKey::new(a, StationAnchor::AfterShot(first_uuid.clone()));
    project.station_merges.push(StationMerge {
        station_a: anchor.clone(),
        station_b: StationRef::new(a, 0).into(),
        merged_at: Utc::now(),
    });

    // Detect.
    let violations = project.check_structural_integrity();
    assert!(
        violations
            .iter()
            .any(|v| v.category == "duplicate_shot_uuid"),
        "duplicate UUID must be flagged: {violations:?}"
    );
    assert!(project.has_critical_integrity_violations());

    // Repair: re-mint the duplicate, keep the first.
    project.repair_structural_integrity();
    let shots = &project.get_section(a).unwrap().shots;
    assert_eq!(
        shots[0].uuid.as_deref(),
        Some(first_uuid.as_str()),
        "first shot must keep its UUID"
    );
    assert_ne!(shots[0].uuid, shots[1].uuid, "duplicate must be re-minted");
    assert!(
        !project
            .check_structural_integrity()
            .iter()
            .any(|v| v.category == "duplicate_shot_uuid"),
        "no duplicate UUID violation after repair"
    );
    // The anchor that resolved to the first station is still valid.
    assert!(
        project.resolve_key(&anchor).is_some(),
        "first-shot anchor survives repair"
    );
}

#[test]
fn warm_shot_uuid_index_resolves_duplicate_to_first_occurrence() {
    // Under a duplicated within-section UUID, the warm `shot_uuid_index` must
    // resolve to the FIRST occurrence, matching the authoritative cold-path
    // `resolve_key` (which uses `shots.iter().position(...)`, i.e. first match).
    // Reachable by undoing a `duplicate_shot_uuid` integrity Repair: it restores
    // the duplicate state and rebuilds the cache warm. If the index used
    // last-write-wins, the warm cache would resolve to station 2 while a cold
    // scan resolves to station 1 - a silent divergence.
    let mut project = Project::new("DupWarm".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let first_uuid = project.get_section(a).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    // Force shot 1 to collide with shot 0's UUID.
    project.get_section_mut(a).unwrap().shots[1].uuid = Some(first_uuid.clone());

    // The cold-path answer: position() returns the first match (station 1).
    let cold = {
        let section = project.get_section(a).unwrap();
        let pos = section
            .shots
            .iter()
            .position(|s| s.uuid.as_deref() == Some(first_uuid.as_str()))
            .unwrap();
        StationRef::new(a, (pos + 1) as u32)
    };
    assert_eq!(
        cold,
        StationRef::new(a, 1),
        "cold scan picks the first shot"
    );

    // Rebuild the cache warm, then resolve through it.
    project.rebuild_shot_uuid_index();
    let anchor = StationKey::new(a, StationAnchor::AfterShot(first_uuid.clone()));
    assert_eq!(
        project.resolve_key(&anchor),
        Some(cold),
        "warm cache must agree with the cold scan on first occurrence"
    );
}

#[test]
fn dangling_cross_references_are_omitted_from_wire_output() {
    // A dangling merge (endpoint anchors on a deleted shot) must be OMITTED from
    // the wire form, not collapsed to a phantom station-0 relationship. Valid
    // refs still round-trip to their positional index.
    let mut project = Project::new("Wire".to_string(), None);
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
    project.ensure_shot_uuids();

    // Valid merge a:0 <-> b:0.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    // Dangling merge: one endpoint anchors on a non-existent shot UUID.
    project.station_merges.push(StationMerge {
        station_a: StationKey::new(a, StationAnchor::AfterShot("does-not-exist".to_string())),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    assert!(
        project.merge_to_wire(&project.station_merges[0]).is_some(),
        "valid merge resolves to wire"
    );
    assert!(
        project.merge_to_wire(&project.station_merges[1]).is_none(),
        "dangling merge must NOT collapse to a phantom ref"
    );

    // get_section_relationships excludes the dangling merge entirely.
    let rel = project.get_section_relationships(a);
    assert_eq!(
        rel.merges.len(),
        1,
        "dangling merge omitted from wire output"
    );
    assert_eq!(
        rel.merges[0].station_a.station_index, 0,
        "valid ref round-trips positionally"
    );
}

#[test]
fn has_unresolvable_cross_references_detects_dangling_offset() {
    // The save gate blocks on this. A dangling station offset is only
    // Warning-severity, but it still collapses to (section, 0) on save and can
    // collide on the PRIMARY KEY - so it must count as unresolvable.
    let mut project = Project::new("Block".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    assert!(
        !project.has_unresolvable_cross_references(),
        "clean project resolves"
    );

    let dangling = StationKey::new(a, StationAnchor::AfterShot("ghost".to_string()));
    project.station_offsets.insert(dangling, (1.0, 2.0, 3.0));
    assert!(
        project.has_unresolvable_cross_references(),
        "a dangling (Warning-level) offset must still block the save"
    );
}
