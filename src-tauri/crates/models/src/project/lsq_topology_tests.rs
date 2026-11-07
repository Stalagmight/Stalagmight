//! The closure least-squares adjustment on a survey-scale topology generated in
//! code (#576 RV13-3): a long zigzag section closed onto a one-shot anchor
//! section, with a merged branch, two zero datums and offsets kilometres from
//! the origin. Every count, index and value comes from a fixed seed.

use super::*;
use crate::test_fixtures::bare_shot;
use crate::ClosureStrategy;

fn shot(azimuth_in: f64, azimuth_out: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    bare_shot(length)
        .with_azimuths(azimuth_in, azimuth_out)
        .with_depths(depth_in, depth_out)
}

/// Deterministic xorshift in [0, 1).
fn xorshift01(state: &mut u64) -> f64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    ((x >> 11) as f64) / ((1u64 << 53) as f64)
}

#[test]
fn lsq_realistic_topology_preserves_geometry() {
    // A survey-scale topology generated in code from a fixed seed: every count,
    // station index, heading, length, depth and offset below is drawn from the
    // generator, none is taken from a real survey.
    // - Section A: a long zigzag section (legs alternate between two headings,
    //   with jitter, random lengths and a random depth walk)
    // - Section B: a one-shot anchor section
    // - Section C: a random branch whose far end is merged onto A's looped part
    // - Two zero datums, at A's far end and at B's start
    // - Closure: an early station of A onto B's end, misclosure under 2 m
    // - Large section offsets (kilometres from the origin)
    let mut rng: u64 = 0xC0FF_EE15_D00D_F00D;
    let mut draw = |lo: f64, hi: f64| lo + xorshift01(&mut rng) * (hi - lo);

    let a_shots = draw(230.0, 330.0) as usize;
    let close_at = draw(15.0, 70.0) as u32;
    let merge_at = draw(a_shots as f64 * 0.5, a_shots as f64 * 0.8) as u32;
    let c_shots = draw(40.0, 120.0) as usize;

    let mut project = Project::new("RealisticLSQ".to_string(), None);

    // Section A: legs alternate between two headings roughly a right angle apart.
    let heading_1 = draw(0.0, 360.0);
    let heading_2 = heading_1 + draw(70.0, 110.0);
    let mut depth = draw(2.0, 12.0);
    let shots_a: Vec<ShotData> = (0..a_shots)
        .map(|i| {
            let base = if i % 2 == 0 { heading_1 } else { heading_2 };
            let az = (base + draw(-20.0, 20.0)).rem_euclid(360.0);
            let len = draw(2.0, 8.0);
            let from = depth;
            depth = (depth + draw(-0.6, 0.6)).max(0.5);
            shot(az, az, len, from, depth)
        })
        .collect();
    let a = project
        .add_section(SectionData {
            shots: shots_a,
            ..Default::default()
        })
        .id;
    project
        .section_offsets
        .insert(a, (draw(-3000.0, -1500.0), draw(900.0, 2400.0)));

    // Section B: one shot (anchor), placed so B:1 lands a misclosure vector away
    // from A:close_at.
    let b_az = draw(0.0, 360.0);
    let b_len = draw(6.0, 14.0);
    let b = project
        .add_section(SectionData {
            shots: vec![shot(b_az, b_az, b_len, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let a_close = project
        .compute_station_raw_coords(StationRef::new(a, close_at))
        .unwrap();
    let b_end_local = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let mis_dir = draw(0.0, 360.0).to_radians();
    let mis_len = draw(0.8, 1.9);
    project.section_offsets.insert(
        b,
        (
            a_close.x - b_end_local.x + mis_len * mis_dir.sin(),
            a_close.y - b_end_local.y + mis_len * mis_dir.cos(),
        ),
    );

    // Section C: a random branch, placed so its far end sits on A:merge_at, and
    // merged there.
    let mut c_depth = draw(2.0, 12.0);
    let shots_c: Vec<ShotData> = (0..c_shots)
        .map(|_| {
            let az = draw(0.0, 360.0);
            let len = draw(1.5, 6.0);
            let from = c_depth;
            c_depth = (c_depth + draw(-0.5, 0.5)).max(0.5);
            shot(az, az, len, from, c_depth)
        })
        .collect();
    let c = project
        .add_section(SectionData {
            shots: shots_c,
            ..Default::default()
        })
        .id;
    let c_end = StationRef::new(c, c_shots as u32);
    let a_merge = project
        .compute_station_raw_coords(StationRef::new(a, merge_at))
        .unwrap();
    let c_end_local = project.compute_station_raw_coords(c_end).unwrap();
    project
        .section_offsets
        .insert(c, (a_merge.x - c_end_local.x, a_merge.y - c_end_local.y));
    project.station_merges.push(StationMerge {
        station_a: c_end.into(),
        station_b: StationRef::new(a, merge_at).into(),
        merged_at: Utc::now(),
    });

    // Zero datums
    let (lat, lon) = (draw(-60.0, 60.0), draw(-170.0, 170.0));
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, a_shots as u32).into(),
        gps: GpsCoordinates::new(lat, lon, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(lat + draw(-0.02, 0.02), lon + draw(-0.02, 0.02), 0.0).unwrap(),
    });

    let shots_of = |project: &Project, id: SectionId| -> Vec<(f64, f64)> {
        project
            .sections
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .shots
            .iter()
            .map(|s| (s.azimuth_in, s.length))
            .collect()
    };
    let orig_a = shots_of(&project, a);
    let orig_c = shots_of(&project, c);

    // Closure: A:close_at ↔ B:1, with the measured misclosure and loop length.
    let pos_close_before = project
        .compute_station_raw_coords(StationRef::new(a, close_at))
        .unwrap();
    let pos_b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let actual_misclosure = ((pos_close_before.x - pos_b1_before.x).powi(2)
        + (pos_close_before.y - pos_b1_before.y).powi(2))
    .sqrt();
    assert!(
        actual_misclosure > 0.5,
        "generated misclosure {actual_misclosure:.3}m is too small to test anything"
    );
    let path_length = project.sections.iter().find(|s| s.id == a).unwrap().shots
        [close_at as usize..]
        .iter()
        .map(Shot::horizontal_length)
        .sum::<f64>()
        + b_len;
    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(a, close_at).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length,
        misclosure_distance: actual_misclosure,
        error_percentage: 100.0 * actual_misclosure / path_length,
        horizontal_misclosure: actual_misclosure,
        vertical_misclosure: (pos_close_before.z - pos_b1_before.z).abs(),
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b, c].into_iter().collect();
    // [#576 RV15-4] This test checks the merge itself, a thousand times tighter
    // than the 0.01 m tripwire the command runs in test builds; held back, that
    // tripwire cannot pre-empt the check below and its measured gap.
    let result = {
        let _own_merge_check = offsets::merge_alignment_assert::suspend();
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config)
    };
    assert!(result.is_ok(), "LSQ should succeed: {result:?}");

    // Verify closure endpoints now coincide
    let pos_close_after = project
        .compute_station_raw_coords(StationRef::new(a, close_at))
        .unwrap();
    let pos_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual_misc = ((pos_close_after.x - pos_b1_after.x).powi(2)
        + (pos_close_after.y - pos_b1_after.y).powi(2))
    .sqrt();
    assert!(
        residual_misc < 0.01,
        "residual misclosure {residual_misc:.6}m should be near zero"
    );

    // Verify per-shot changes in section A are bounded.
    let new_a = shots_of(&project, a);
    let mut max_len_delta_a = 0.0_f64;
    let mut worst_shot = 0;
    for (i, ((_, orig_len), (_, new_len))) in orig_a.iter().zip(new_a.iter()).enumerate() {
        let delta = (new_len - orig_len).abs();
        if delta > max_len_delta_a {
            max_len_delta_a = delta;
            worst_shot = i;
        }
    }
    assert!(
        max_len_delta_a < 1.0,
        "shot {worst_shot} length changed by {max_len_delta_a:.4}m - too large for \
         {actual_misclosure:.2}m misclosure"
    );

    // The branch is a spur: none of its shots is touched, and its merged far end
    // still sits on A:merge_at after A moved.
    let new_c = shots_of(&project, c);
    let moved_c: Vec<usize> = (0..orig_c.len())
        .filter(|&i| new_c.get(i) != orig_c.get(i))
        .collect();
    assert!(
        new_c.len() == orig_c.len() && moved_c.is_empty(),
        "branch C untouched: {} of {} shots modified (first: {:?}), {} shots now",
        moved_c.len(),
        orig_c.len(),
        moved_c.first().map(|&i| (i, orig_c[i], new_c.get(i))),
        new_c.len()
    );
    let a_merge_after = project
        .compute_station_raw_coords(StationRef::new(a, merge_at))
        .unwrap();
    let c_end_after = project.compute_station_raw_coords(c_end).unwrap();
    let merge_gap = ((a_merge_after.x - c_end_after.x).powi(2)
        + (a_merge_after.y - c_end_after.y).powi(2))
    .sqrt();
    assert!(
        merge_gap < 1e-6,
        "merged stations drifted {merge_gap:.6}m apart"
    );
}
