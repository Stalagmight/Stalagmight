//! Delete section cascade.
//!
//! Lines 20071-20662 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// ============================================================================
// Delete section cascade tests
// ============================================================================

#[test]
fn test_delete_section_cascades_merges() {
    let state = state_with_project("Test");

    // Create two sections
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(3.0)],
        })
        .section_id
        .unwrap();

    // Merge them
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_result.success);
    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 1);

    // Delete section A - should cascade-remove the merge
    let delete_result = state.execute(Command::DeleteSection { section_id: sec_a });
    assert!(delete_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(
        project.station_merges.len(),
        0,
        "merge should be cascaded on delete"
    );
}

#[test]
fn test_delete_section_cascades_closures_and_offsets() {
    let state = state_with_project("Test");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(3.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(4.0)],
        })
        .section_id
        .unwrap();

    // Set a zero datum on section A. Station 1, NOT station 0: the first datum
    // pins its station to the origin, and station 0 is already there, so a datum
    // on it displaces nothing - and `section_offsets` now means "sections that
    // are actually displaced" (#413 finding 2: an exactly-(0, 0) entry is
    // dropped rather than stored). Anchoring station 1 gives A a REAL offset, so
    // the delete-cascade assertion below has something to cascade. Note that the
    // add displaces EVERY section, not just A's merge group: establishing the
    // frame re-frames the whole survey rigidly (`Project::set_zero_datum` ->
    // `renormalize_frame`), so B picks up the same translation. Only A's offset
    // is asserted here because only A's is the one the delete has to cascade.
    let datum_result = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 1),
        coordinates: GpsCoordinates::new(45.0, 10.0, 0.0).unwrap(),
    });
    assert!(datum_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.zero_datums.len(), 1);
    // set_zero_datum displaced A, so it has an offset (stronger than the
    // "the map is non-empty" this used to assert).
    assert!(
        project.section_offsets().contains_key(&sec_a),
        "set_zero_datum should give the displaced section an offset"
    );

    // Delete section A - should cascade-remove the zero datum and section offsets
    let delete_result = state.execute(Command::DeleteSection { section_id: sec_a });
    assert!(delete_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].id, sec_b);
    assert_eq!(
        project.zero_datums.len(),
        0,
        "zero datum should be cascaded"
    );
    assert!(
        !project.section_offsets().contains_key(&sec_a),
        "section A offset should be cascaded"
    );
}

#[test]
fn test_delete_section_undo_restores_merges_and_datums() {
    let state = state_with_project("Test");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(3.0)],
        })
        .section_id
        .unwrap();

    // Merge + set datum
    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 0),
        coordinates: GpsCoordinates::new(45.0, 10.0, 0.0).unwrap(),
    });

    // Delete section A
    state.execute(Command::DeleteSection { section_id: sec_a });
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.station_merges.len(), 0);
    assert_eq!(project.zero_datums.len(), 0);

    // Undo delete - should restore section A, merge, and datum
    let undo = state.undo();
    assert!(undo.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        2,
        "undo should restore deleted section"
    );
    assert_eq!(project.station_merges.len(), 1, "undo should restore merge");
    assert_eq!(
        project.zero_datums.len(),
        1,
        "undo should restore zero datum"
    );
}

#[test]
fn test_unmerge_removes_only_specific_merge() {
    let state = state_with_project("Test");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(3.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(4.0)],
        })
        .section_id
        .unwrap();

    // Merge A↔B and B↔C
    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_b, 1),
        station_b: StationRef::new(sec_c, 0),
    });

    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 2);

    // Unmerge at station A:1 (which is part of the A↔B merge)
    let result = state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(sec_a, 1),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.station_merges.len(),
        1,
        "only A↔B should be removed"
    );
    assert_eq!(project.station_merges[0].station_a.section_id, sec_b);
    assert_eq!(project.station_merges[0].station_b.section_id, sec_c);
}

#[test]
fn test_unmerge_undo_restores_merge() {
    let state = state_with_project("Test");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(3.0)],
        })
        .section_id
        .unwrap();

    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });

    // Unmerge, then undo
    state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(sec_a, 1),
    });
    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 0);

    let undo = state.undo();
    assert!(undo.success);
    assert_eq!(
        state.get_project_state().unwrap().station_merges.len(),
        1,
        "undo should restore unmerged pair"
    );
}

/// Integration test: import kb5.dmp sections with overlap treatment and verify
/// the actual shot values in the project state after import.
///
/// Flow:
/// 1. Import section 002 at origin (becomes the "existing" section)
/// 2. Import section 001 (105 shots, overlap removed) with existing_shot_updates
///    that interpolate 002's overlap shots - all in one atomic import call
/// 3. Read back from project state and assert actual interpolated values
#[test]
fn kb5_import_overlap_interpolation_integration() {
    use crate::models::ExistingShotUpdate;
    use crate::parsers::dmp::{parse_dmp, DmpShot, DmpShotType};

    // Built exactly as the deleted `DmpShot::to_shot_data` did (#576 B24), so
    // this test's input is unchanged.
    let to_shot_data = |s: &DmpShot| ShotData {
        uuid: None,
        azimuth_in: crate::models::normalize_azimuth(s.heading_in),
        azimuth_out: crate::models::normalize_azimuth(s.heading_out),
        length: s.length,
        depth_in: s.depth_in,
        depth_out: s.depth_out,
        pitch_in: Some(s.pitch_in),
        pitch_out: Some(s.pitch_out),
        left: Some(s.left),
        right: Some(s.right),
        up: Some(s.up),
        down: Some(s.down),
        comment: s.comment.clone(),
        kind: ShotKind::Normal,
        marker_index: Some(u32::from(s.marker_index)),
        temperature: Some(s.temperature),
    };

    let dmp_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/kb5.dmp");
    let dmp = parse_dmp(&dmp_path).expect("should parse kb5.dmp");

    // Extract standard shots as ShotData for sections 001 (idx 0) and 002 (idx 1)
    let shots_001: Vec<ShotData> = dmp.sections[0]
        .shots
        .iter()
        .filter(|s| s.shot_type == DmpShotType::Standard)
        .map(to_shot_data)
        .collect();
    let shots_002: Vec<ShotData> = dmp.sections[1]
        .shots
        .iter()
        .filter(|s| s.shot_type == DmpShotType::Standard)
        .map(to_shot_data)
        .collect();
    assert_eq!(shots_001.len(), 107);
    assert_eq!(shots_002.len(), 364);

    // ── Step 1: Create project and import section 002 at origin ──
    let state = state_with_project("KB5 Overlap Test");

    let result_002 = state.execute_import_sections(
        vec![ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("002".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots: shots_002.clone(),
            station_comments: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
        }],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(
        result_002.success,
        "import 002 failed: {:?}",
        result_002.error
    );

    let project = state.get_project_state().unwrap();
    let section_002_id = project.sections[0].id;
    assert_eq!(
        project.sections[0].shots.len(),
        364,
        "002 should have 364 shots"
    );

    // ── Step 2: Import section 001 with overlap shots removed + existing shot updates ──
    // 001→002 overlap: Back (impStn=107), Reverse, matched=2, exStn=182.
    // Target shot indices (Back+Reverse): (182-2)+k = 180, 181.
    // Import shots 105, 106 are deleted; 002 shots 180, 181 get interpolated values.

    fn circular_mean_deg(a: f64, b: f64) -> f64 {
        let (a_r, b_r) = (a.to_radians(), b.to_radians());
        let mean =
            f64::atan2((a_r.sin() + b_r.sin()) / 2.0, (a_r.cos() + b_r.cos()) / 2.0).to_degrees();
        if mean < 0.0 {
            mean + 360.0
        } else {
            mean
        }
    }

    fn avg_opt(a: Option<f64>, b: Option<f64>) -> Option<Option<f64>> {
        match (a, b) {
            (None, None) => None,
            (Some(v), None) | (None, Some(v)) => Some(Some(v)),
            (Some(a), Some(b)) => Some(Some((a + b) / 2.0)),
        }
    }

    fn interp_update(
        section_id: SectionId,
        shot_id: ShotId,
        imp: &ShotData,
        ext: &ShotData,
    ) -> ExistingShotUpdate {
        ExistingShotUpdate {
            section_id,
            shot_id,
            updates: ShotUpdates {
                azimuth_in: Some(circular_mean_deg(imp.azimuth_in, ext.azimuth_in)),
                azimuth_out: Some(circular_mean_deg(imp.azimuth_out, ext.azimuth_out)),
                length: Some((imp.length + ext.length) / 2.0),
                depth_in: Some((imp.depth_in + ext.depth_in) / 2.0),
                depth_out: Some((imp.depth_out + ext.depth_out) / 2.0),
                pitch_in: avg_opt(imp.pitch_in, ext.pitch_in),
                pitch_out: avg_opt(imp.pitch_out, ext.pitch_out),
                left: avg_opt(imp.left, ext.left),
                right: avg_opt(imp.right, ext.right),
                up: avg_opt(imp.up, ext.up),
                down: avg_opt(imp.down, ext.down),
                ..Default::default()
            },
        }
    }

    let shot_180_id = project.sections[0].shots[180].id;
    let shot_181_id = project.sections[0].shots[181].id;

    let existing_updates = vec![
        interp_update(
            section_002_id,
            shot_180_id,
            &shots_001[105],
            &shots_002[180],
        ),
        interp_update(
            section_002_id,
            shot_181_id,
            &shots_001[106],
            &shots_002[181],
        ),
    ];

    // Remove overlap shots 105, 106 from 001
    let mut shots_001_treated: Vec<ShotData> = shots_001.clone();
    shots_001_treated.remove(106); // remove higher index first
    shots_001_treated.remove(105);
    assert_eq!(
        shots_001_treated.len(),
        105,
        "001 should have 105 shots after overlap removal"
    );

    // Import 001 merged to 002 at station 182, with existing shot updates
    let result_001 = state.execute_import_sections(
        vec![ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("001".to_string()),
            surveyor: None,
            date: None,
            color: "#ff6b6b".to_string(),
            direction: None,
            quality: 1.0,
            shots: shots_001_treated,
            station_comments: Vec::new(),
            placement: ImportPlacement::FromExisting {
                import_station: 105,
                existing_station: StationRef::new(section_002_id, 182),
            },
            azimuth_reference: None,
        }],
        Vec::new(),
        None,
        Vec::new(),
        existing_updates,
        VirtualShotMode::Auto,
    );
    assert!(
        result_001.success,
        "import 001 failed: {:?}",
        result_001.error
    );

    // ── Step 3: Read back from project state and verify ──
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2, "should have 2 sections");

    let sec_002 = project
        .sections
        .iter()
        .find(|s| s.id == section_002_id)
        .expect("section 002 should exist");
    let sec_001 = project
        .sections
        .iter()
        .find(|s| s.id != section_002_id)
        .expect("section 001 should exist");

    // Section 001: 105 shots (overlap shots removed)
    assert_eq!(
        sec_001.shots.len(),
        105,
        "001 should have 105 shots after import"
    );

    // Section 002: 364 shots, with shots 180-181 now having interpolated values
    assert_eq!(sec_002.shots.len(), 364, "002 should still have 364 shots");

    let actual_180 = &sec_002.shots[180];
    let actual_181 = &sec_002.shots[181];

    // Verify shot 180 has interpolated values (not the original 6.52m/329.7°)
    let eps = 0.01;
    assert!(
        (actual_180.length - 5.665).abs() < eps,
        "002 shot 180 length after import: expected ~5.665, got {}",
        actual_180.length
    );
    assert!(
        (actual_180.azimuth_in - 322.4).abs() < 0.5,
        "002 shot 180 azimuth_in after import: expected ~322.4°, got {}°",
        actual_180.azimuth_in
    );

    // Verify shot 181 has interpolated values (not the original 12.71m/355.8°)
    assert!(
        (actual_181.length - 12.375).abs() < eps,
        "002 shot 181 length after import: expected ~12.375, got {}",
        actual_181.length
    );
    assert!(
        (actual_181.azimuth_in - 349.75).abs() < 0.5,
        "002 shot 181 azimuth_in after import: expected ~349.75°, got {}°",
        actual_181.azimuth_in
    );

    println!(
        "002 shot 180: length={}, azi_in={}",
        actual_180.length, actual_180.azimuth_in
    );
    println!(
        "002 shot 181: length={}, azi_in={}",
        actual_181.length, actual_181.azimuth_in
    );
}
