//! Realigning merged and datum sections after an edit.
//!
//! Lines 8542-9065 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// === realign_after_shot_edit tests ===

#[test]
fn realign_datum_section_keeps_datum_pinned() {
    // Single section with datum on station 3. Edit shot before datum → datum stays at origin.
    let mut project = Project::new("DatumPin".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // north 10m
                shot(0.0, 0.0, 5.0, 0.0, 0.0),  // north 5m (will be edited)
                shot(0.0, 0.0, 3.0, 0.0, 0.0),  // north 3m
            ],
            ..Default::default()
        })
        .id;

    let datum_station = StationRef::new(sec, 3);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(datum_station, gps).unwrap();

    // Datum at origin
    let coords = project.compute_station_raw_coords(datum_station).unwrap();
    assert!((coords.y).abs() < 1e-6);

    // Edit shot[1]: 5m → 8m
    let shot_id = project.sections.iter().find(|s| s.id == sec).unwrap().shots[1].id;
    project.update_shot(
        sec,
        shot_id,
        &ShotUpdates {
            length: Some(8.0),
            ..Default::default()
        },
    );

    // Datum has drifted
    let drifted = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        (drifted.y).abs() > 1.0,
        "Expected drift, got y={}",
        drifted.y
    );

    // Realign
    project.realign_after_shot_edit(sec, None);

    // Datum back at origin
    let fixed = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        (fixed.x).abs() < 1e-6,
        "Datum x should be 0, got {}",
        fixed.x
    );
    assert!(
        (fixed.y).abs() < 1e-6,
        "Datum y should be 0, got {}",
        fixed.y
    );

    // Station 0 moved to compensate (absorbed the 3m length increase)
    let s0 = project
        .compute_station_raw_coords(StationRef::new(sec, 0))
        .unwrap();
    // Before edit: s0 was at y = -18. After: y = -21 (3m further south)
    assert!(
        (s0.y - (-21.0)).abs() < 1e-6,
        "s0 y should be -21, got {}",
        s0.y
    );
}

#[test]
fn realign_merged_section_keeps_merge_point_aligned() {
    // Section 1 has datum at 1:0. Section 2 merged at 1:2 <-> 2:0.
    // Edit shot 2:1 → 2:0 should stay aligned with 1:2, station 2:0 shifts.
    // Wait, 2:0 IS the merge point. Let me restructure:
    //
    // Section 1: 3 shots north (10m each). Datum at 1:0.
    //   Stations: 1:0(datum, origin), 1:1(y=10), 1:2(y=20), 1:3(y=30)
    // Section 2: 3 shots east (5m each). Merged at 1:3 <-> 2:0.
    //   Before merge alignment: 2:0(0,0), 2:1(5,0), 2:2(10,0), 2:3(15,0)
    //   After merge alignment: 2:0(0,30), 2:1(5,30), 2:2(10,30), 2:3(15,30)
    //
    // Edit shot 2:0 (first shot in section 2): length 5m → 15m.
    //   Without realign: 2:0(0,30), 2:1(15,30), 2:2(20,30), 2:3(25,30)
    //   2:0 is the merge point and should stay at (0,30) matching 1:3.
    //   With realign: same, because 2:0 is station_index 0 and the edit is shot[0]
    //   which shifts 2:1+ but NOT 2:0. Station 2:0 stays put.
    //
    // Better test: edit shot that's BEFORE the merge point within section 2.
    // But 2:0 is the merge point (station_index 0) so all shots are after it.
    // Let's make the merge at a middle station instead.

    // Section 1: shots north. Datum at 1:0.
    // Section 2: 5 shots east (5m each). Merged at 1:3 <-> 2:3.
    //   Edit shot 2:1 → stations 2:2, 2:3 shift. 2:3 is the merge point.
    //   Realign should keep 2:3 aligned with 1:3.

    let mut project = Project::new("MergeAlign".to_string(), None);

    let sec1 = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // north 10m
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // north 10m
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // north 10m
            ],
            ..Default::default()
        })
        .id;

    let sec2 = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0), // east 5m
                shot(90.0, 90.0, 5.0, 0.0, 0.0), // east 5m (will be edited)
                shot(90.0, 90.0, 5.0, 0.0, 0.0), // east 5m
            ],
            ..Default::default()
        })
        .id;

    // Set datum at 1:0
    let datum_ref = StationRef::new(sec1, 0);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(datum_ref, gps).unwrap();

    // Merge 1:3 <-> 2:3
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec1, 3).into(),
        station_b: StationRef::new(sec2, 3).into(),
        merged_at: Utc::now(),
    });

    // Align section 2 offset so 2:3 matches 1:3
    // 1:3 is at (0, 30). 2:3 without offset is at (15, 0).
    // Need offset: (0-15, 30-0) = (-15, 30)
    let pos_1_3 = project
        .compute_station_raw_coords(StationRef::new(sec1, 3))
        .unwrap();
    let pos_2_3_raw = project
        .compute_station_raw_coords(StationRef::new(sec2, 3))
        .unwrap();
    project
        .section_offsets
        .insert(sec2, (pos_1_3.x - pos_2_3_raw.x, pos_1_3.y - pos_2_3_raw.y));

    // Verify merge alignment
    let pos_2_3 = project
        .compute_station_raw_coords(StationRef::new(sec2, 3))
        .unwrap();
    assert!(
        (pos_2_3.x - pos_1_3.x).abs() < 1e-6,
        "Merge should be aligned x"
    );
    assert!(
        (pos_2_3.y - pos_1_3.y).abs() < 1e-6,
        "Merge should be aligned y"
    );

    // Record positions of locked stations
    let pos_2_0_before = project
        .compute_station_raw_coords(StationRef::new(sec2, 0))
        .unwrap();

    // Edit shot 2:1 (between 2:1 and 2:2): length 5m → 15m (+10m)
    let shot_id = project
        .sections
        .iter()
        .find(|s| s.id == sec2)
        .unwrap()
        .shots[1]
        .id;
    project.update_shot(
        sec2,
        shot_id,
        &ShotUpdates {
            length: Some(15.0),
            ..Default::default()
        },
    );

    // Before realign: 2:3 has drifted 10m east from 1:3
    let drifted_2_3 = project
        .compute_station_raw_coords(StationRef::new(sec2, 3))
        .unwrap();
    assert!(
        (drifted_2_3.x - pos_1_3.x).abs() > 5.0,
        "2:3 should have drifted from 1:3"
    );

    // Realign
    project.realign_after_shot_edit(sec2, None);

    // After realign: 2:3 matches 1:3 again (merge point preserved)
    let fixed_2_3 = project
        .compute_station_raw_coords(StationRef::new(sec2, 3))
        .unwrap();
    assert!(
        (fixed_2_3.x - pos_1_3.x).abs() < 1e-6,
        "2:3 x should match 1:3 x, got {} vs {}",
        fixed_2_3.x,
        pos_1_3.x
    );
    assert!(
        (fixed_2_3.y - pos_1_3.y).abs() < 1e-6,
        "2:3 y should match 1:3 y, got {} vs {}",
        fixed_2_3.y,
        pos_1_3.y
    );

    // 1:0 (datum) still at origin
    let datum_coords = project.compute_station_raw_coords(datum_ref).unwrap();
    assert!(
        (datum_coords.x).abs() < 1e-6,
        "Datum should still be at origin x"
    );
    assert!(
        (datum_coords.y).abs() < 1e-6,
        "Datum should still be at origin y"
    );

    // 2:0 (unlocked end) moved to compensate
    let pos_2_0_after = project
        .compute_station_raw_coords(StationRef::new(sec2, 0))
        .unwrap();
    assert!(
        (pos_2_0_after.x - pos_2_0_before.x).abs() > 5.0,
        "2:0 should have shifted to compensate, before={} after={}",
        pos_2_0_before.x,
        pos_2_0_after.x
    );
}

#[test]
fn realign_noop_without_datums() {
    // Section without any datums → realign should not change anything.
    let mut project = Project::new("NoDatum".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let offsets_before = project.section_offsets.clone();
    project.realign_after_shot_edit(sec, None);
    assert_eq!(
        project.section_offsets, offsets_before,
        "Offsets should not change when no datums exist"
    );
}

/// A realign with nothing held must not re-georeference the survey against a
/// datum that anchors nothing.
///
/// `zero_datums[0]` here DANGLES - its anchor key no longer resolves, which the
/// app reaches by deleting the shot or section a datum was anchored to - so the
/// survey is laid out against `zero_datums[1]`. That is what
/// [`Project::frame_datum`] names, what `frame_datum_key` gates the
/// renormalisation on, and the only one of the two the wire hands the frontend.
/// The absolute fallback here used to read `zero_datums.first()` instead: the
/// group's own datum then failed the "is this the frame" test, and its target
/// was recomputed as `gps_to_local(its gps, dangling.gps)` - the whole
/// inter-datum vector, about 11 km in this fixture, applied to a rigid group
/// inside an `update_shot` whose undo data carries no offsets at all.
///
/// # Why this drives the model and not a command
///
/// The realign's only production caller is
/// `src-tauri/src/core/state/command_shot.rs:251`, which passes
/// `hold_anchor_at = project.datum_anchor_position(section_id)` - computed at
/// `:243`, BEFORE the edit - whenever `updates.touches_geometry()`, and does not
/// call the realign at all otherwise. `datum_anchor_position` and the realign's
/// own datum lookup use the SAME predicate (first datum in the merge group that
/// resolves), so a group holding a resolvable datum always arrives with
/// `hold_anchor_at = Some` and takes the held branch. Through that caller the
/// absolute fallback is therefore reachable only in the degenerate case where
/// the datum resolves but its coordinates do not compute - non-finite geometry.
/// So this pins the model-level no-hold path directly, which is also the path
/// every neighbouring `realign_after_shot_edit(sec, None)` test exercises.
///
/// Non-vacuous: the two datums are about 11 km apart while the tolerance is
/// [`Project::OFFSET_DUST_M`], and the fixture asserts up front that
/// `zero_datums[0]` really does fail to resolve.
#[test]
fn realign_without_a_hold_ignores_a_dangling_first_datum() {
    let mut project = Project::new("DanglingFrame".to_string(), None);
    let dangling_sec = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let framed_sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 7.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // No merges: the two sections are separate rigid groups, so the realign on
    // one of them can only be reasoning about that group's own datum.

    // zero_datums[0], anchored on a shot uuid that is not in this project. Pushed
    // straight onto the vector because no mutator will build one.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(
            dangling_sec,
            StationAnchor::AfterShot("no-such-uuid".to_string()),
        ),
        gps: GpsCoordinates::new(20.25, -87.5, 0.0).unwrap(),
    });
    assert!(
        project
            .resolve_key(&project.zero_datums[0].station_ref)
            .is_none(),
        "fixture rot: zero_datums[0] must NOT resolve, or this test proves nothing"
    );

    // zero_datums[1]: the real one, about 11 km away, and the frame the survey is
    // actually laid out against.
    project
        .set_zero_datum(
            StationRef::new(framed_sec, 0),
            GpsCoordinates::new(20.35, -87.4, 0.0).unwrap(),
        )
        .unwrap();
    assert_eq!(
        project.zero_datums.len(),
        2,
        "the real datum must be stored AFTER the dangling one"
    );

    let before: Vec<(StationRef, Coordinates)> = project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, s.coordinates))
        .collect();

    project.realign_after_shot_edit(framed_sec, None);

    let after: HashMap<StationRef, Coordinates> = project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, s.coordinates))
        .collect();
    for (station_ref, was) in before {
        let now = after
            .get(&station_ref)
            .expect("realign must not remove a station");
        assert!(
            (now.x - was.x).abs() <= Project::OFFSET_DUST_M
                && (now.y - was.y).abs() <= Project::OFFSET_DUST_M,
            "{station_ref:?} moved from ({}, {}) to ({}, {}): the no-hold realign \
             re-georeferenced the group against the dangling zero_datums[0]",
            was.x,
            was.y,
            now.x,
            now.y
        );
    }
}

#[test]
fn realign_is_idempotent() {
    // Edit shot, realign twice → same result.
    let mut project = Project::new("Idempotent".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let datum_station = StationRef::new(sec, 2);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(datum_station, gps).unwrap();

    // Edit shot[0]
    let shot_id = project.sections.iter().find(|s| s.id == sec).unwrap().shots[0].id;
    project.update_shot(
        sec,
        shot_id,
        &ShotUpdates {
            length: Some(12.0),
            ..Default::default()
        },
    );

    project.realign_after_shot_edit(sec, None);
    let offsets_first = project.section_offsets.clone();

    project.realign_after_shot_edit(sec, None);
    let offsets_second = project.section_offsets.clone();

    assert_eq!(offsets_first, offsets_second, "Should be idempotent");

    // Datum at origin
    let coords = project.compute_station_raw_coords(datum_station).unwrap();
    assert!((coords.x).abs() < 1e-6, "Datum x should be 0");
    assert!((coords.y).abs() < 1e-6, "Datum y should be 0");
}

#[test]
fn realign_edit_in_datum_section_propagates_to_merged() {
    // Edit a shot in the datum section (after datum) → merged partner follows.
    // Section 1: 3 shots north (10m each). Datum at 1:0.
    // Section 2: merged at 1:3 <-> 2:0.
    // Edit shot 1:1 → 1:3 moves → section 2 must follow.
    let mut project = Project::new("DatumPropagate".to_string(), None);

    let sec1 = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // will be edited
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let sec2 = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Datum at 1:0
    project
        .set_zero_datum(
            StationRef::new(sec1, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge 1:3 <-> 2:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec1, 3).into(),
        station_b: StationRef::new(sec2, 0).into(),
        merged_at: Utc::now(),
    });

    // Align section 2 to merge point
    let pos_1_3 = project
        .compute_station_raw_coords(StationRef::new(sec1, 3))
        .unwrap();
    let pos_2_0 = project
        .compute_station_raw_coords(StationRef::new(sec2, 0))
        .unwrap();
    project
        .section_offsets
        .insert(sec2, (pos_1_3.x - pos_2_0.x, pos_1_3.y - pos_2_0.y));

    // Record 2:1's position before edit
    let pos_2_1_before = project
        .compute_station_raw_coords(StationRef::new(sec2, 1))
        .unwrap();

    // Edit shot 1:1 (between 1:1 and 1:2): 10m → 20m
    let shot_id = project
        .sections
        .iter()
        .find(|s| s.id == sec1)
        .unwrap()
        .shots[1]
        .id;
    project.update_shot(
        sec1,
        shot_id,
        &ShotUpdates {
            length: Some(20.0),
            ..Default::default()
        },
    );

    // 1:3 has shifted north by 10m. 1:0 (datum) is before the edit, unchanged.
    // Realign from sec1 (the edited section)
    project.realign_after_shot_edit(sec1, None);

    // 1:0 still at origin (datum, before edited shot)
    let datum = project
        .compute_station_raw_coords(StationRef::new(sec1, 0))
        .unwrap();
    assert!(
        (datum.x).abs() < 1e-6 && (datum.y).abs() < 1e-6,
        "Datum should be at origin"
    );

    // 2:0 should match new 1:3 position (30 + 10 = 40 north)
    let new_1_3 = project
        .compute_station_raw_coords(StationRef::new(sec1, 3))
        .unwrap();
    let new_2_0 = project
        .compute_station_raw_coords(StationRef::new(sec2, 0))
        .unwrap();
    assert!(
        (new_2_0.x - new_1_3.x).abs() < 1e-6 && (new_2_0.y - new_1_3.y).abs() < 1e-6,
        "2:0 should match 1:3 after realign: 2:0=({},{}) 1:3=({},{})",
        new_2_0.x,
        new_2_0.y,
        new_1_3.x,
        new_1_3.y
    );

    // 2:1 should have moved by same amount as 1:3 (10m north)
    let pos_2_1_after = project
        .compute_station_raw_coords(StationRef::new(sec2, 1))
        .unwrap();
    assert!(
        (pos_2_1_after.y - pos_2_1_before.y - 10.0).abs() < 1e-6,
        "2:1 should have moved 10m north, delta={}",
        pos_2_1_after.y - pos_2_1_before.y
    );
}
