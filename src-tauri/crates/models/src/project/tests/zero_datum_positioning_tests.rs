//! Zero datums: positioning on set, remove and update, and datum propagation through merges.
//!
//! Lines 484-1385 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn set_zero_datum_first_datum_positions_station_at_origin() {
    let mut project = Project::new("FirstDatum".to_string(), None);
    // Create a section with a shot going north (azimuth 0)
    let shot_a = shot(0.0, 0.0, 10.0, 0.0, 1.0);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot_a.clone()],
            ..Default::default()
        })
        .id;

    // Station 1 is at (0, 10, 1) before datum is set
    let station_ref = StationRef::new(section_id, 1);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();

    // Set first datum on station 1
    project.set_zero_datum(station_ref, gps).unwrap();

    // Check that offset was applied to move station to origin
    let offset = project.section_offsets.get(&section_id).unwrap();
    // Station was at (0, horizontal), needs offset (-0, -horizontal) to be at origin
    assert!(
        (offset.0 - 0.0).abs() < 1e-6,
        "Expected x offset ~0, got {}",
        offset.0
    );
    let expected_y_offset = -shot_a.horizontal_length();
    assert!(
        (offset.1 - expected_y_offset).abs() < 1e-6,
        "Expected y offset ~{}, got {}",
        expected_y_offset,
        offset.1
    );

    // Verify station coordinates after applying offset
    let coords = project.compute_station_raw_coords(station_ref).unwrap();
    assert!((coords.x).abs() < 1e-6, "Expected x ~0, got {}", coords.x);
    assert!((coords.y).abs() < 1e-6, "Expected y ~0, got {}", coords.y);
    // Z should be preserved
    assert!(
        (coords.z - 1.0).abs() < 1e-6,
        "Expected z ~1, got {}",
        coords.z
    );
}

/// Adding the survey's FIRST datum translates the WHOLE survey as one rigid
/// body, not just the datum's own merge group.
///
/// Before the frame exists, local `(0, 0)` means nothing and nothing has been
/// placed against it, so the only real content of the project is the RELATIVE
/// arrangement of its components. Landing the datum on the origin by moving its
/// own merge group is the one answer that destroys exactly that: every other
/// component stays where it was and the whole survey silently re-arranges
/// itself. `renormalize_frame` gives the other answer - one translation applied
/// to every section - which lands the datum on the origin AND leaves every
/// inter-component vector bit-for-bit intact.
///
/// Non-vacuous by construction, on both halves. The two sections are separate
/// components (no merge, no closure, so `adjust_rigid_group_offsets` on `a`
/// cannot reach `b`), and the datum's station sits 10 m north of its section's
/// start, so establishing the frame is a real (0, -10) translation rather than
/// nothing at all. Under the pre-fix rule `b` keeps its coordinates and the
/// vector between the components changes by the full 10 m.
#[test]
fn set_zero_datum_first_datum_translates_every_component() {
    let mut project = Project::new("FirstDatumFrame".to_string(), None);
    // Component A: one shot due north, so station 1 sits at (0, 10) - away from
    // the origin, which is what gives the establishing translation something to
    // do.
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // Component B: unconnected to A and deliberately parked away from the
    // origin, so "B did not move" and "B moved with A" are different answers.
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 4.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.set_section_offset(b, (25.0, -13.0));
    assert!(
        project.station_merges.is_empty() && project.loop_closures.is_empty(),
        "setup: the two sections have to be independent components, or the datum's rigid group \
         would reach B and the test would prove nothing"
    );

    let datum_station = StationRef::new(a, 1);
    let a1_before = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        a1_before.x.abs() < 1e-9 && (a1_before.y - 10.0).abs() < 1e-9,
        "setup: the datum station must start AWAY from the origin, got {a1_before:?}"
    );
    let b0_before = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    let b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();

    project
        .set_zero_datum(
            datum_station,
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Half one: the datum station IS the origin now. That is what establishing
    // the frame means, and every reader of local metres depends on it.
    let a1_after = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        a1_after.x.abs() < 1e-9 && a1_after.y.abs() < 1e-9,
        "the first datum's station has to sit at local (0, 0), got {a1_after:?}"
    );

    // Half two: the OTHER component moved by the same delta, so nothing moved
    // relative to anything else.
    let delta = (-a1_before.x, -a1_before.y);
    let b0_after = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    let b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        (b0_after.x - (b0_before.x + delta.0)).abs() < 1e-9
            && (b0_after.y - (b0_before.y + delta.1)).abs() < 1e-9,
        "the unconnected component has to ride the establishing translation {delta:?}: B:0 went \
         {b0_before:?} -> {b0_after:?}"
    );
    assert!(
        (b1_after.x - (b1_before.x + delta.0)).abs() < 1e-9
            && (b1_after.y - (b1_before.y + delta.1)).abs() < 1e-9,
        "the unconnected component has to ride the establishing translation {delta:?}: B:1 went \
         {b1_before:?} -> {b1_after:?}"
    );

    // Said the other way round, which is the property the user actually owns:
    // the vector between the two components is unchanged.
    let separation_before = (b0_before.x - a1_before.x, b0_before.y - a1_before.y);
    let separation_after = (b0_after.x - a1_after.x, b0_after.y - a1_after.y);
    assert!(
        (separation_after.0 - separation_before.0).abs() < 1e-9
            && (separation_after.1 - separation_before.1).abs() < 1e-9,
        "adding the survey's first georeference moved one component relative to another: A:1 -> \
         B:0 was {separation_before:?}, is {separation_after:?}"
    );
}

/// A datum whose station no longer resolves anchors NOTHING, so the survey's
/// first RESOLVABLE datum is the one that establishes the frame - and a new
/// datum added while only a dangling one is stored lands on the origin, not at
/// `gps_to_local(its gps, the dangling one's gps)`.
///
/// A dangling datum is dropped by `Project::datum_to_wire` before the frontend
/// ever sees it, and `Project::frame_datum_key` skips it, so the map's
/// `gpsOrigin`, the data table's GPS column and the Ariane export all read the
/// first resolvable one. `set_zero_datum` used to read `zero_datums.first()`
/// unfiltered, which positioned the new group against an origin nothing else in
/// the app reads.
///
/// Non-vacuous by construction: the stored datum's `AfterShot` anchor names a
/// uuid no shot carries (asserted below, so the setup cannot rot into a
/// resolvable key), and its GPS sits over 2 km from the new one - so under the
/// unfiltered rule the new component was flung by that whole distance instead
/// of landing on the origin.
#[test]
fn set_zero_datum_ignores_a_dangling_datum_when_choosing_the_frame() {
    let mut project = Project::new("DanglingFrame".to_string(), None);
    // The section the dangling datum claims to be anchored in, and a second,
    // unconnected component that will carry the real datum.
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 7.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.set_section_offset(b, (12.0, 30.0));

    // The dangling datum, pushed straight onto the vector: no mutator will
    // create one, it is what a project reaches when the anchoring shot is
    // deleted out from under a stored datum.
    let dangling_gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("no-such-uuid".to_string())),
        gps: dangling_gps,
    });
    assert_eq!(
        project.resolve_key(&project.zero_datums[0].station_ref),
        None,
        "setup: the stored datum has to be DANGLING, or this test is about the ordinary \
         subsequent-datum path instead"
    );

    let datum_station = StationRef::new(b, 1);
    let b1_before = project.compute_station_raw_coords(datum_station).unwrap();
    let a0_before = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    // Over 2 km north of the dangling fix: the displacement the unfiltered rule
    // would have applied, and far past any tolerance here.
    let new_gps = GpsCoordinates::new(45.02, -90.0, 0.0).unwrap();
    let flung = crate::gps_to_local(&new_gps, &dangling_gps);
    assert!(
        flung.0.hypot(flung.1) > 1000.0,
        "setup: the two GPS fixes must be far enough apart to be unmistakable, got {flung:?}"
    );

    project.set_zero_datum(datum_station, new_gps).unwrap();

    let b1_after = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        b1_after.x.abs() < 1e-9 && b1_after.y.abs() < 1e-9,
        "the first RESOLVABLE datum defines the frame, so its station has to sit at local (0, 0); \
         got {b1_after:?} (the dangling datum's GPS would have put it near {flung:?})"
    );

    // And, as with any establishing add, nothing moved relative to anything
    // else - the dangling-datum section rode along.
    let a0_after = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    let separation_before = (a0_before.x - b1_before.x, a0_before.y - b1_before.y);
    let separation_after = (a0_after.x - b1_after.x, a0_after.y - b1_after.y);
    assert!(
        (separation_after.0 - separation_before.0).abs() < 1e-9
            && (separation_after.1 - separation_before.1).abs() < 1e-9,
        "the establishing add moved one component relative to another: B:1 -> A:0 was \
         {separation_before:?}, is {separation_after:?}"
    );
}

#[test]
fn set_zero_datum_connected_sections_move_together() {
    let mut project = Project::new("Connected".to_string(), None);
    // Create two sections
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Align b:0 to a:1 (a:1 is at (0, 10) since shot goes north 10m)
    project.set_section_offset(b, (0.0, 10.0));

    // Merge section b's station 0 to section a's station 1
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Record positions before setting datum
    // Section a station 1 is at (0, 10) before datum (shot goes north)
    // Section b station 0 is at (0, 10) before datum (aligned to a:1)
    // Section b station 1 is at (5, 10) before datum (shot goes east from b:0)
    let coords_b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        (coords_b1_before.x - 5.0).abs() < 1e-6,
        "Pre-datum b.1 x should be 5"
    );
    assert!(
        (coords_b1_before.y - 10.0).abs() < 1e-6,
        "Pre-datum b.1 y should be 10"
    );

    // Set datum on section a, station 1 (which is at y=10 before datum)
    // This should move all connected sections so a.1 lands at origin
    let station_ref = StationRef::new(a, 1);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps).unwrap();

    // Both sections should have the datum shift applied. Asserted on the
    // RESOLVED offset rather than on key presence: `b` was aligned to `a:1` by
    // an offset of (0, 10) and the datum shifts everything by (0, -10), which
    // cancels EXACTLY - and `section_offsets` means "sections that are actually
    // displaced", so a value of exactly (0, 0) is dropped rather than stored
    // (#413 finding 2). Comparing the values is strictly stronger than the
    // `contains_key` pair this replaced, which never checked what was in them.
    assert_eq!(project.section_offset(a), (0.0, -10.0));
    assert_eq!(project.section_offset(b), (0.0, 0.0));
    assert!(!project.section_offsets.contains_key(&b));

    // Section a station 1 (datum station) should be at origin
    let coords_a1 = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!(
        (coords_a1.x).abs() < 1e-6,
        "Datum station x should be 0, got {}",
        coords_a1.x
    );
    assert!(
        (coords_a1.y).abs() < 1e-6,
        "Datum station y should be 0, got {}",
        coords_a1.y
    );

    // Section a station 0 should be at (0, -10) since it's 10m south of the datum
    let coords_a0 = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        (coords_a0.x).abs() < 1e-6,
        "a.0 x should be 0, got {}",
        coords_a0.x
    );
    assert!(
        (coords_a0.y - (-10.0)).abs() < 1e-6,
        "a.0 y should be -10, got {}",
        coords_a0.y
    );

    // Section b should have moved by the same delta as section a
    // Before: b.1 was at (5, 10). After: b.1 should be at (5, 0)
    // (The offset delta was (0, -10) to move a.1 from (0, 10) to (0, 0))
    let coords_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        (coords_b1_after.x - 5.0).abs() < 1e-6,
        "b.1 x should be 5, got {}",
        coords_b1_after.x
    );
    assert!(
        (coords_b1_after.y - 0.0).abs() < 1e-6,
        "b.1 y should be 0 (moved with connected sections), got {}",
        coords_b1_after.y
    );
}

#[test]
fn set_zero_datum_subsequent_datum_uses_gps_offset() {
    let mut project = Project::new("SubsequentDatum".to_string(), None);
    // Create two unconnected sections
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum on section a, station 0
    let gps_a = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(a, 0), gps_a)
        .unwrap();

    // Section a station 0 should be at origin
    let coords_a0 = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!((coords_a0.x).abs() < 1e-6);
    assert!((coords_a0.y).abs() < 1e-6);

    // Set second datum on section b with GPS offset
    // At equator, 0.01 degree longitude ~ 1113 meters east
    let gps_b = GpsCoordinates::new(0.0, 0.01, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(b, 0), gps_b)
        .unwrap();

    // Section b station 0 should be at approximately (1113, 0)
    let coords_b0 = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!(
        (coords_b0.x - 1113.2).abs() < 10.0,
        "Expected x ~1113, got {}",
        coords_b0.x
    );
    assert!(
        coords_b0.y.abs() < 10.0,
        "Expected y ~0, got {}",
        coords_b0.y
    );
}

#[test]
fn set_zero_datum_z_coordinates_preserved() {
    let mut project = Project::new("PreserveZ".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 5.0),
                shot(0.0, 0.0, 10.0, 5.0, 12.0),
            ],
            ..Default::default()
        })
        .id;

    // Set datum on station 1 (which has z=5)
    let station_ref = StationRef::new(section_id, 1);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps).unwrap();

    // Check all stations have preserved z coordinates
    let coords_0 = project
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    let coords_1 = project
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    let coords_2 = project
        .compute_station_raw_coords(StationRef::new(section_id, 2))
        .unwrap();

    assert!((coords_0.z - 0.0).abs() < 1e-6, "Station 0 z should be 0");
    assert!((coords_1.z - 5.0).abs() < 1e-6, "Station 1 z should be 5");
    assert!((coords_2.z - 12.0).abs() < 1e-6, "Station 2 z should be 12");
}

#[test]
fn remove_zero_datum_keeps_positions_unchanged() {
    let mut project = Project::new("RemoveDatum".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum
    let station_ref = StationRef::new(section_id, 1);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps).unwrap();

    // Record offset after setting datum
    let offset_before = project.section_offsets.get(&section_id).copied();

    // Remove datum
    project.remove_zero_datum(station_ref);

    // Offset should be unchanged
    let offset_after = project.section_offsets.get(&section_id).copied();
    assert_eq!(
        offset_before, offset_after,
        "Offset should not change when removing datum"
    );
}

#[test]
fn update_first_datum_gps_keeps_station_at_origin() {
    // Issue: Re-setting the first datum should not move it away from origin
    let mut project = Project::new("UpdateFirstDatum".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum on station 1 (which is at y=10 before datum)
    let station_ref = StationRef::new(section_id, 1);
    let gps1 = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps1).unwrap();

    // Station 1 should be at origin
    let coords_after_first = project.compute_station_raw_coords(station_ref).unwrap();
    assert!(
        (coords_after_first.x).abs() < 1e-6,
        "First datum: x should be 0"
    );
    assert!(
        (coords_after_first.y).abs() < 1e-6,
        "First datum: y should be 0"
    );
    let offset_after_first = project.section_offsets.get(&section_id).copied().unwrap();

    // Update the first datum with different GPS coordinates
    let gps2 = GpsCoordinates::new(46.0, -91.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps2).unwrap();

    // Station 1 should STILL be at origin (updating first datum doesn't reposition)
    let coords_after_update = project.compute_station_raw_coords(station_ref).unwrap();
    assert!(
        (coords_after_update.x).abs() < 1e-6,
        "After update: x should still be 0, got {}",
        coords_after_update.x
    );
    assert!(
        (coords_after_update.y).abs() < 1e-6,
        "After update: y should still be 0, got {}",
        coords_after_update.y
    );

    // Offset should be unchanged (no repositioning happened)
    let offset_after_update = project.section_offsets.get(&section_id).copied().unwrap();
    assert_eq!(
        offset_after_first, offset_after_update,
        "Offset should not change when updating first datum GPS"
    );

    // GPS should be updated
    assert_eq!(project.zero_datums[0].gps.latitude, 46.0);
    assert_eq!(project.zero_datums[0].gps.longitude, -91.0);
}

#[test]
fn store_zero_datum_without_repositioning_does_not_modify_offsets() {
    let mut project = Project::new("StoreOnly".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Pre-set an offset manually (simulating merged project scenario)
    project.section_offsets.insert(section_id, (100.0, 200.0));

    // Store datum without repositioning
    let station_ref = StationRef::new(section_id, 0);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .store_zero_datum_without_repositioning(station_ref, gps)
        .unwrap();

    // Offset should remain unchanged
    let offset = project.section_offsets.get(&section_id).unwrap();
    assert_eq!(*offset, (100.0, 200.0), "Offset should not be modified");

    // But datum should be stored
    assert_eq!(project.zero_datums.len(), 1);
    assert!(project.key_is_ref(&project.zero_datums[0].station_ref, station_ref));
}

#[test]
fn coordinate_computation_no_global_datum_subtraction() {
    // Verify that compute_station_coordinates no longer subtracts primary datum
    let mut project = Project::new("NoGlobalSubtract".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Add a datum directly to zero_datums (bypassing set_zero_datum to avoid offset computation)
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(section_id, 1).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });

    // Compute coordinates
    let stations = project.compute_station_coordinates();

    // Station 0 should be at (0, 0) - no global subtraction
    let station_0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(section_id, 0))
        .unwrap();
    assert!((station_0.coordinates.x).abs() < 1e-6);
    assert!((station_0.coordinates.y).abs() < 1e-6);

    // Station 1 should be at (0, 10) - no global subtraction
    let station_1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(section_id, 1))
        .unwrap();
    assert!((station_1.coordinates.x).abs() < 1e-6);
    assert!((station_1.coordinates.y - 10.0).abs() < 1e-6);
}

#[test]
fn merge_stations_propagates_datum_positioning_a_into_b() {
    // Test: Section without datum merges into section group with datum
    // The merged section should be repositioned relative to the datum
    let mut project = Project::new("MergeDatumA".to_string(), None);

    // Section A: has datum, shot goes north 10m
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on A.0 (positions A.0 at origin)
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Verify A is positioned correctly before merge
    let a0_coords = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!((a0_coords.x).abs() < 1e-6, "A.0 x should be 0");
    assert!((a0_coords.y).abs() < 1e-6, "A.0 y should be 0");
    let a1_coords = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!((a1_coords.y - 10.0).abs() < 1e-6, "A.1 y should be 10");

    // Section B: no datum, shot goes east 5m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge B.0 into A.1 (B merges into datum group)
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // After merge, B should be positioned in datum coordinate system
    // B.0 should be at A.1's position (0, 10)
    let b0_coords = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!(
        (b0_coords.x).abs() < 1e-6,
        "B.0 x should be 0, got {}",
        b0_coords.x
    );
    assert!(
        (b0_coords.y - 10.0).abs() < 1e-6,
        "B.0 y should be 10, got {}",
        b0_coords.y
    );

    // B.1 should be at (5, 10) - 5m east of B.0
    let b1_coords = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        (b1_coords.x - 5.0).abs() < 1e-6,
        "B.1 x should be 5, got {}",
        b1_coords.x
    );
    assert!(
        (b1_coords.y - 10.0).abs() < 1e-6,
        "B.1 y should be 10, got {}",
        b1_coords.y
    );

    // A's positions should be unchanged
    let a0_after = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!((a0_after.x).abs() < 1e-6, "A.0 x should still be 0");
    assert!((a0_after.y).abs() < 1e-6, "A.0 y should still be 0");
}

#[test]
fn merge_stations_propagates_datum_positioning_b_into_a() {
    // Test: Section with datum merges with section without datum
    // The non-datum section should be repositioned
    let mut project = Project::new("MergeDatumB".to_string(), None);

    // Section A: no datum, shot goes north 10m
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section B: has datum, shot goes east 5m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on B.0 (positions B.0 at origin)
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Verify B is positioned correctly before merge
    let b0_coords = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!((b0_coords.x).abs() < 1e-6, "B.0 x should be 0");
    assert!((b0_coords.y).abs() < 1e-6, "B.0 y should be 0");

    // Merge A.0 into B.1 (A merges into datum group)
    // B.1 is at (5, 0)
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 1))
        .unwrap();

    // After merge, A should be positioned in datum coordinate system
    // A.0 should be at B.1's position (5, 0)
    let a0_coords = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        (a0_coords.x - 5.0).abs() < 1e-6,
        "A.0 x should be 5, got {}",
        a0_coords.x
    );
    assert!(
        (a0_coords.y).abs() < 1e-6,
        "A.0 y should be 0, got {}",
        a0_coords.y
    );

    // A.1 should be at (5, 10) - 10m north of A.0
    let a1_coords = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!(
        (a1_coords.x - 5.0).abs() < 1e-6,
        "A.1 x should be 5, got {}",
        a1_coords.x
    );
    assert!(
        (a1_coords.y - 10.0).abs() < 1e-6,
        "A.1 y should be 10, got {}",
        a1_coords.y
    );

    // B's positions should be unchanged
    let b0_after = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!((b0_after.x).abs() < 1e-6, "B.0 x should still be 0");
    assert!((b0_after.y).abs() < 1e-6, "B.0 y should still be 0");
}

#[test]
fn merge_stations_without_datum_aligns_first_section_to_second() {
    // Test: No datum present; the first section should move to align with the second.
    let mut project = Project::new("MergeNoDatum".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let a1_before = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    let b0_before = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!((a1_before.y - 10.0).abs() < 1e-6);
    assert!((b0_before.x).abs() < 1e-6);
    assert!((b0_before.y).abs() < 1e-6);

    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    let a1_after = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    let b0_after = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!((a1_after.x - b0_after.x).abs() < 1e-6);
    assert!((a1_after.y - b0_after.y).abs() < 1e-6);

    let offset_a = project
        .section_offsets
        .get(&a)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!((offset_a.0).abs() < 1e-6);
    assert!((offset_a.1 + 10.0).abs() < 1e-6);

    let offset_b = project
        .section_offsets
        .get(&b)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!((offset_b.0).abs() < 1e-6);
    assert!((offset_b.1).abs() < 1e-6);
}

#[test]
fn merge_stations_transitive_datum_propagation() {
    // Test: Merge into a section that's connected to a datum group through another merge
    // C merges into B, which is already merged with datum-holding A
    let mut project = Project::new("TransitiveDatum".to_string(), None);

    // Section A: has datum, shot goes north 10m
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on A.0
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Section B: shot goes east 5m (will be connected to A)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge B.0 into A.1 - B now connected to datum group
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Section C: shot goes south 3m
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Verify B.1 position before merging C
    // B.1 should be at (5, 10) - B.0 at (0, 10), B.1 5m east
    let b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!((b1_before.x - 5.0).abs() < 1e-6, "B.1 x should be 5");
    assert!((b1_before.y - 10.0).abs() < 1e-6, "B.1 y should be 10");

    // Merge C.0 into B.1 - C should be positioned via transitive datum
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // C.0 should be at B.1's position (5, 10)
    let c0_coords = project
        .compute_station_raw_coords(StationRef::new(c, 0))
        .unwrap();
    assert!(
        (c0_coords.x - 5.0).abs() < 1e-6,
        "C.0 x should be 5, got {}",
        c0_coords.x
    );
    assert!(
        (c0_coords.y - 10.0).abs() < 1e-6,
        "C.0 y should be 10, got {}",
        c0_coords.y
    );

    // C.1 should be at (5, 7) - 3m south of C.0
    let c1_coords = project
        .compute_station_raw_coords(StationRef::new(c, 1))
        .unwrap();
    assert!(
        (c1_coords.x - 5.0).abs() < 1e-6,
        "C.1 x should be 5, got {}",
        c1_coords.x
    );
    assert!(
        (c1_coords.y - 7.0).abs() < 1e-6,
        "C.1 y should be 7, got {}",
        c1_coords.y
    );
}
