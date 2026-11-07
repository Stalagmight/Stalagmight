//! Section cleanup (fn-47-ok1.7) and section relationships.
//!
//! Lines 4658-4858 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// === Section cleanup tests (fn-47-ok1.7) ===

/// Helper: build a project with 3 sections (a, b, c) where:
/// - a is merged to b (station a:1 -> b:0)
/// - b has a loop closure to c (b:1 -> c:0)
/// - a has a section offset
/// - a has a station offset on station 0
/// - a has a zero datum on station 0
fn build_project_with_relationships() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("Cleanup Test".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 2.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge a -> b
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Loop closure b -> c
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 5.0,
        misclosure_distance: 0.1,
        error_percentage: 2.0,
        horizontal_misclosure: 0.08,
        vertical_misclosure: 0.06,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id = 2;

    // Section offset for a
    project.section_offsets.insert(a, (10.0, 20.0));

    // Station offset for a:0
    project
        .station_offsets
        .insert(StationRef::new(a, 0).into(), (1.0, 2.0, 3.0));

    // Zero datum for a:0
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });

    (project, a, b, c)
}

#[test]
fn test_cleanup_removes_merges() {
    let (mut project, a, _b, _c) = build_project_with_relationships();
    assert_eq!(project.station_merges.len(), 1);

    project.cleanup_section_references(a);

    assert_eq!(
        project.station_merges.len(),
        0,
        "Merge referencing section A should be removed"
    );
}

#[test]
fn test_cleanup_removes_closures() {
    let (mut project, _a, b, _c) = build_project_with_relationships();
    assert_eq!(project.loop_closures.len(), 1);

    project.cleanup_section_references(b);

    assert_eq!(
        project.loop_closures.len(),
        0,
        "Closure referencing section B should be removed"
    );
}

#[test]
fn test_cleanup_removes_offsets() {
    let (mut project, a, _b, _c) = build_project_with_relationships();
    assert!(project.section_offsets.contains_key(&a));
    assert_eq!(project.station_offsets.len(), 1);

    project.cleanup_section_references(a);

    assert!(
        !project.section_offsets.contains_key(&a),
        "Section offset for A should be removed"
    );
    assert_eq!(
        project.station_offsets.len(),
        0,
        "Station offset for A:0 should be removed"
    );
}

#[test]
fn test_cleanup_removes_zero_datums() {
    let (mut project, a, _b, _c) = build_project_with_relationships();
    assert_eq!(project.zero_datums.len(), 1);

    project.cleanup_section_references(a);

    assert_eq!(
        project.zero_datums.len(),
        0,
        "Zero datum for A:0 should be removed"
    );
}

#[test]
fn test_cleanup_guarded_by_existence() {
    // Cleanup on a non-existent section should not mutate anything
    let (mut project, _a, _b, _c) = build_project_with_relationships();
    let orig_merges = project.station_merges.len();
    let orig_closures = project.loop_closures.len();
    let orig_offsets = project.section_offsets.len();
    let orig_station_offsets = project.station_offsets.len();
    let orig_datums = project.zero_datums.len();

    // Section 999 doesn't exist - cleanup should be a no-op
    project.cleanup_section_references(999);

    assert_eq!(project.station_merges.len(), orig_merges);
    assert_eq!(project.loop_closures.len(), orig_closures);
    assert_eq!(project.section_offsets.len(), orig_offsets);
    assert_eq!(project.station_offsets.len(), orig_station_offsets);
    assert_eq!(project.zero_datums.len(), orig_datums);
}

#[test]
fn test_cleanup_preserves_unrelated_relationships() {
    let (mut project, a, b, c) = build_project_with_relationships();

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    assert_eq!(project.station_merges.len(), 2);
    assert_eq!(project.loop_closures.len(), 1);

    project.cleanup_section_references(a);

    assert_eq!(project.station_merges.len(), 1);
    assert!(project.station_merges.iter().any(|m| {
        (m.station_a.section_id == b && m.station_b.section_id == c)
            || (m.station_a.section_id == c && m.station_b.section_id == b)
    }));

    assert_eq!(project.loop_closures.len(), 1);
    let closure = &project.loop_closures[0];
    assert_eq!(closure.station_a.section_id, b);
    assert_eq!(closure.station_b.section_id, c);
}

#[test]
fn test_get_section_relationships() {
    let (project, a, b, _c) = build_project_with_relationships();

    let rel_a = project.get_section_relationships(a);
    assert_eq!(rel_a.merges.len(), 1, "Section A has one merge");
    assert_eq!(rel_a.closures.len(), 0, "Section A has no closures");
    assert!(rel_a.has_section_offset, "Section A has a section offset");
    assert_eq!(
        rel_a.station_offset_count, 1,
        "Section A has one station offset"
    );
    assert_eq!(rel_a.zero_datum_count, 1, "Section A has one zero datum");

    let rel_b = project.get_section_relationships(b);
    assert_eq!(rel_b.merges.len(), 1, "Section B has one merge");
    assert_eq!(rel_b.closures.len(), 1, "Section B has one closure");
    assert!(!rel_b.has_section_offset, "Section B has no section offset");
    assert_eq!(
        rel_b.station_offset_count, 0,
        "Section B has no station offsets"
    );
    assert_eq!(rel_b.zero_datum_count, 0, "Section B has no zero datums");
}
