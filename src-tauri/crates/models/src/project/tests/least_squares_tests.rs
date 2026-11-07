//! adjust_least_squares.
//!
//! Lines 9746-10434 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// ── adjust_least_squares tests ──────────────────────────────────────

#[test]
fn lsq_no_closures_is_noop() {
    // With no closures, there are no constraints to adjust - should return Ok
    let mut project = Project::new("LSQ".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 5.0, 5.0),
                shot(90.0, 90.0, 10.0, 5.0, 5.0),
            ],
            ..Default::default()
        })
        .id;

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [sec].into_iter().collect();
    let result = project.adjust_least_squares(&component, &[], StationRef::new(sec, 0), &config);
    assert!(result.is_ok(), "No closures should succeed: {:?}", result);
}

#[test]
fn lsq_preserves_depth_values() {
    // Key test: LSQ should adjust XY but NOT touch depth_in/depth_out
    let mut project = Project::new("LSQDepth".to_string(), None);

    // Create a triangle: section A (north 10m), section B (east 10m), section C (south-west ~14m)
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 5.0, 8.0)], // north, depth 5→8
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 8.0, 12.0)], // east, depth 8→12
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(225.0, 225.0, 14.5, 12.0, 5.5)], // south-west back, depth 12→5.5
            ..Default::default()
        })
        .id;

    // Merge to form a loop: A:1 → B:0, B:1 → C:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    // Closure: C:1 should be at A:0 (closing the loop)
    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(c, 1).into(),
        station_b: StationRef::new(a, 0).into(),
        path_length: 34.5,
        misclosure_distance: 0.5,
        error_percentage: 1.4,
        horizontal_misclosure: 0.5,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    // Save original depths
    let orig_depths: Vec<(f64, f64)> = project
        .sections
        .iter()
        .flat_map(|s| s.shots.iter().map(|sh| (sh.depth_in, sh.depth_out)))
        .collect();

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b, c].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    // Verify depths are unchanged
    let new_depths: Vec<(f64, f64)> = project
        .sections
        .iter()
        .flat_map(|s| s.shots.iter().map(|sh| (sh.depth_in, sh.depth_out)))
        .collect();
    assert_eq!(
        orig_depths, new_depths,
        "depth_in/depth_out must not change after LSQ adjustment"
    );
}

#[test]
fn lsq_anchor_station_stays_fixed() {
    // The anchor station (or its group) should not move
    let mut project = Project::new("LSQAnchor".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 5.0, 5.0),
                shot(0.0, 0.0, 10.0, 5.0, 5.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 20.5, 5.0, 5.0)], // slight error for closure
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(a, 0).into(),
        path_length: 30.5,
        misclosure_distance: 0.5,
        error_percentage: 1.6,
        horizontal_misclosure: 0.5,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let anchor = StationRef::new(a, 0);
    let coords_before = project.compute_station_raw_coords(anchor).unwrap();

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let result = project.adjust_least_squares(&component, &[closure], anchor, &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    let coords_after = project.compute_station_raw_coords(anchor).unwrap();
    assert!(
        (coords_after.x - coords_before.x).abs() < 1e-6
            && (coords_after.y - coords_before.y).abs() < 1e-6,
        "anchor XY should not move: before=({}, {}), after=({}, {})",
        coords_before.x,
        coords_before.y,
        coords_after.x,
        coords_after.y
    );
}

#[test]
fn lsq_produces_finite_coordinates() {
    // After adjustment, all station coordinates should be finite
    let mut project = Project::new("LSQFinite".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 5.0, 5.0),
                shot(90.0, 90.0, 10.0, 5.0, 5.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(225.0, 225.0, 14.0, 5.0, 5.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(a, 0).into(),
        path_length: 34.0,
        misclosure_distance: 0.2,
        error_percentage: 0.6,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    for section in &project.sections {
        for idx in 0..=section.shots.len() {
            let coords = project
                .compute_station_raw_coords(StationRef::new(section.id, idx as u32))
                .unwrap();
            assert!(
                coords.x.is_finite(),
                "x should be finite for {}:{}",
                section.id,
                idx
            );
            assert!(
                coords.y.is_finite(),
                "y should be finite for {}:{}",
                section.id,
                idx
            );
            assert!(
                coords.z.is_finite(),
                "z should be finite for {}:{}",
                section.id,
                idx
            );
        }
    }
}

#[test]
fn lsq_with_zero_datum_fixes_that_group() {
    // When a zero datum exists, its group should be fixed and not move
    let mut project = Project::new("LSQDatum".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 5.0, 5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 5.0, 5.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(225.0, 225.0, 14.5, 5.0, 5.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, 10.0, 100.0).unwrap(),
    });

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(c, 1).into(),
        station_b: StationRef::new(a, 0).into(),
        path_length: 34.5,
        misclosure_distance: 0.5,
        error_percentage: 1.4,
        horizontal_misclosure: 0.5,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let datum_coords_before = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b, c].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    let datum_coords_after = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        (datum_coords_after.x - datum_coords_before.x).abs() < 1e-6
            && (datum_coords_after.y - datum_coords_before.y).abs() < 1e-6,
        "zero datum station should stay fixed"
    );
}

#[test]
fn lsq_update_shot_horizontal_only_preserves_depth() {
    // Direct test of update_shot_horizontal_only
    let mut s = Shot::new(
        1,
        ShotData {
            azimuth_in: 45.0,
            azimuth_out: 45.0,
            length: 10.0,
            depth_in: 3.0,
            depth_out: 7.0,
            pitch_in: Some(23.5),
            pitch_out: Some(23.5),
            ..Default::default()
        },
    );

    // Update horizontal displacement to pure east (dx=5, dy=0)
    Project::update_shot_horizontal_only(&mut s, 5.0, 0.0, AzimuthReference::default());

    assert!((s.depth_in - 3.0).abs() < 1e-9, "depth_in must not change");
    assert!(
        (s.depth_out - 7.0).abs() < 1e-9,
        "depth_out must not change"
    );
    assert_eq!(s.pitch_in, Some(23.5), "pitch_in must not change");
    assert_eq!(s.pitch_out, Some(23.5), "pitch_out must not change");
    // Azimuth should now point east (90 degrees)
    assert!(
        (s.azimuth_in - 90.0).abs() < 1e-6,
        "azimuth should be 90, got {}",
        s.azimuth_in
    );
    // Length should account for horizontal + dz: sqrt(5^2 + 4^2) = sqrt(41)
    let expected_len = (25.0_f64 + 16.0).sqrt();
    assert!(
        (s.length - expected_len).abs() < 1e-6,
        "length should be sqrt(41), got {}",
        s.length
    );
}

#[test]
fn lsq_update_shot_horizontal_only_zero_displacement() {
    // When dx=dy=0, azimuth should be preserved, length = |dz|
    let mut s = Shot::new(
        1,
        ShotData {
            azimuth_in: 135.0,
            azimuth_out: 135.0,
            length: 5.0,
            depth_in: 2.0,
            depth_out: 6.0,
            ..Default::default()
        },
    );

    Project::update_shot_horizontal_only(&mut s, 0.0, 0.0, AzimuthReference::default());

    assert!(
        (s.azimuth_in - 135.0).abs() < 1e-6,
        "azimuth should be preserved when displacement is zero"
    );
    // length = sqrt(0 + dz^2) = |4| = 4
    assert!(
        (s.length - 4.0).abs() < 1e-6,
        "length should be |dz|=4, got {}",
        s.length
    );
}

/// The solver writes a recomputed bearing straight onto both legs, so its wrap is
/// the last thing standing between an `atan2` result and a stored reading.
///
/// A displacement a hair WEST of north gives a tiny negative angle, and a bare
/// `rem_euclid` maps that to exactly `360.0` - out of range, and `INVALID_AZIMUTH`
/// is error severity, so the user cannot even dismiss the warning the adjustment
/// just created for them.
#[test]
fn recomputed_azimuth_from_a_hair_west_of_north_stays_in_range() {
    assert_eq!(
        (-1e-16_f64).atan2(1.0).to_degrees().rem_euclid(360.0),
        360.0,
        "precondition: the bare rem_euclid wrap escapes [0, 360) for this displacement"
    );

    let mut s = Shot::new(
        1,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 1.0,
            ..Default::default()
        },
    );
    Project::update_shot_horizontal_only(&mut s, -1e-16, 1.0, AzimuthReference::default());

    assert!(
        (0.0..360.0).contains(&s.azimuth_in) && (0.0..360.0).contains(&s.azimuth_out),
        "solver wrote an out-of-range reading: ({}, {})",
        s.azimuth_in,
        s.azimuth_out
    );
    assert_eq!(s.azimuth_in, 0.0);
}

#[test]
fn lsq_long_chain_per_shot_changes_are_small() {
    // Regression test: for a long chain with a small closure error, the
    // per-shot geometry changes must be small (proportional to misclosure /
    // shot_count), not massive due to solver non-convergence.
    //
    // Topology: section A (200 shots north, each 5m) with zero datum at A:200.
    //           section B (1 shot, 10m east) with zero datum at B:0.
    //           Closure: A:100 ↔ B:1
    //
    // This creates an asymmetric loop where B:1 is 1 shot from a fixed point
    // and A:100 is 100-200 shots from fixed points. With proper convergence,
    // each shot should change by at most misclosure/num_shots ≈ 0.01m.
    let mut project = Project::new("LSQLongChain".to_string(), None);

    // Section A: 200 shots heading north, each 5m
    let shots_a: Vec<ShotData> = (0..200).map(|_| shot(0.0, 0.0, 5.0, 0.0, 0.0)).collect();
    let a = project
        .add_section(SectionData {
            shots: shots_a,
            ..Default::default()
        })
        .id;

    // Section B: 1 shot heading east, 10m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Zero datums
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 200).into(),
        gps: GpsCoordinates::new(45.0, 10.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.01, 10.01, 0.0).unwrap(),
    });

    // Large offsets stress-test the solver: absolute coordinates are 500-1800m
    // from origin, which triggers convergence issues if starting from x=0.
    project.section_offsets.insert(a, (500.0, 800.0));
    project.section_offsets.insert(b, (500.0, 1300.0));
    // A:0 at (500, 800), A:100 at (500, 1300), A:200 at (500, 1800)
    // B:0 at (500, 1300), B:1 at (510, 1300)
    // misclosure distance((500, 1300), (510, 1300)) = 10m
    // path_length ≈ 200*5 + 10 = 1010m

    // Save original shot data
    let orig_shots_a: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 100).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 1010.0,
        misclosure_distance: 10.0,
        error_percentage: 0.99,
        horizontal_misclosure: 10.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    // Check per-shot changes are small (bounded by misclosure / num_shots * safety_factor)
    // With 201 shots and 10m misclosure, max per-shot change ≈ 0.05m per shot.
    // We allow up to 0.5m per shot to be safe (10x margin).
    let new_shots_a: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let mut max_length_delta = 0.0_f64;
    for (i, ((orig_az, orig_len), (new_az, new_len))) in
        orig_shots_a.iter().zip(new_shots_a.iter()).enumerate()
    {
        let len_delta = (new_len - orig_len).abs();
        if len_delta > max_length_delta {
            max_length_delta = len_delta;
        }
        assert!(
            len_delta < 0.5,
            "shot {} length changed by {:.3}m (orig={:.3}, new={:.3}) - \
             suggests solver did not converge",
            i,
            len_delta,
            orig_len,
            new_len,
        );
        // Azimuth change: for a 5m shot, 0.5m displacement is ~5.7 degrees
        let az_delta = (new_az - orig_az).abs();
        let az_delta = if az_delta > 180.0 {
            360.0 - az_delta
        } else {
            az_delta
        };
        assert!(
            az_delta < 10.0,
            "shot {} azimuth changed by {:.3}° (orig={:.3}, new={:.3}) - \
             suggests solver did not converge",
            i,
            az_delta,
            orig_az,
            new_az,
        );
    }
}

#[test]
fn lsq_spur_shots_are_not_modified() {
    // Spur (branch) shots have no loop redundancy and must remain untouched.
    //
    // Topology:
    //   Section A: 100 shots north (5m each), zero datum at A:100.
    //   Section B: 1 shot east (10m), zero datum at B:0.
    //   Section C: 20 shots east (3m each), merged to A:50 via C:0↔A:50.
    //   Closure: A:30 ↔ B:1.
    //
    // Spur stations: A:0-A:29 (before closure), C:1-C:20 (branch beyond merge).
    // Loop stations: A:30-A:100, B:0-B:1.
    let mut project = Project::new("SpurTest".to_string(), None);

    let shots_a: Vec<ShotData> = (0..100).map(|_| shot(0.0, 0.0, 5.0, 0.0, 0.0)).collect();
    let a = project
        .add_section(SectionData {
            shots: shots_a,
            ..Default::default()
        })
        .id;

    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let shots_c: Vec<ShotData> = (0..20).map(|_| shot(90.0, 90.0, 3.0, 0.0, 0.0)).collect();
    let c = project
        .add_section(SectionData {
            shots: shots_c,
            ..Default::default()
        })
        .id;

    // Merge C:0 ↔ A:50
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(c, 0).into(),
        station_b: StationRef::new(a, 50).into(),
        merged_at: Utc::now(),
    });

    // Zero datums
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 100).into(),
        gps: GpsCoordinates::new(45.0, 10.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.01, 10.01, 0.0).unwrap(),
    });

    // Large offsets to stress the solver
    project.section_offsets.insert(a, (500.0, 800.0));
    project.section_offsets.insert(b, (500.0, 950.0));
    project.section_offsets.insert(c, (500.0, 1050.0));

    // Save original shot data for spur sections
    let orig_a_spur: Vec<(f64, f64)> = project.sections.iter().find(|s| s.id == a).unwrap().shots
        [..30] // A:0-A:29 are spur shots (before closure at A:30)
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let orig_c: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == c)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 30).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 360.0,
        misclosure_distance: 5.0,
        error_percentage: 1.4,
        horizontal_misclosure: 5.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b, c].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    // Verify spur shots in A (indices 0..29) are exactly unchanged.
    let new_a_spur: Vec<(f64, f64)> = project.sections.iter().find(|s| s.id == a).unwrap().shots
        [..30]
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();
    for (i, (orig, new)) in orig_a_spur.iter().zip(new_a_spur.iter()).enumerate() {
        assert!(
            (orig.0 - new.0).abs() < f64::EPSILON && (orig.1 - new.1).abs() < f64::EPSILON,
            "spur shot A:{i} was modified: az {:.6}→{:.6}, len {:.6}→{:.6}",
            orig.0,
            new.0,
            orig.1,
            new.1,
        );
    }

    // Verify all shots in section C are exactly unchanged.
    let new_c: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == c)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();
    for (i, (orig, new)) in orig_c.iter().zip(new_c.iter()).enumerate() {
        assert!(
            (orig.0 - new.0).abs() < f64::EPSILON && (orig.1 - new.1).abs() < f64::EPSILON,
            "spur shot C:{i} was modified: az {:.6}→{:.6}, len {:.6}→{:.6}",
            orig.0,
            new.0,
            orig.1,
            new.1,
        );
    }

    // Verify merged stations (C:0 ↔ A:50) still share the same position.
    let pos_c0 = project
        .compute_station_raw_coords(StationRef::new(c, 0))
        .unwrap();
    let pos_a50 = project
        .compute_station_raw_coords(StationRef::new(a, 50))
        .unwrap();
    let merge_gap = ((pos_c0.x - pos_a50.x).powi(2) + (pos_c0.y - pos_a50.y).powi(2)).sqrt();
    assert!(
        merge_gap < 1e-6,
        "merge C:0↔A:50 broken after adjustment: gap={merge_gap:.6}m, \
         C:0=({:.4},{:.4}), A:50=({:.4},{:.4})",
        pos_c0.x,
        pos_c0.y,
        pos_a50.x,
        pos_a50.y,
    );
}
