//! Synthetic inputs with hand-computed outputs, independent of any prior solver.
use super::*;

fn eastward_loop(quality_b: f64, anchored: bool) -> (Project, SectionId, SectionId) {
    let mut p = Project::new("Known horizontal loop".into(), None);
    let a = p
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 12.0, 0.0, 0.0)],
            quality: quality_b,
            ..Default::default()
        })
        .id;
    if anchored {
        p.set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    }
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(a, 1),
        StationRef::new(b, 1),
        ClosureStrategy::Batch,
    )
    .unwrap();
    (p, a, b)
}

fn assert_station(stations: &[Station], id: SectionId, index: u32, expected: [f64; 3]) {
    let actual = station_coords(stations, StationRef::new(id, index));
    for (actual, expected) in [actual.x, actual.y, actual.z].into_iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 1e-8,
            "got {actual}, expected {expected}"
        );
    }
}

#[test]
fn parallel_paths_produce_known_weighted_coordinates() {
    // Eastward X variance is length sigma squared, scaled by quality squared:
    // equal quality gives (10 + 12)/2; quality 2 gives (10 + 12/4)/(1 + 1/4).
    for (quality, anchored, expected_x) in
        [(1.0, true, 11.0), (2.0, true, 10.4), (1.0, false, 11.0)]
    {
        let (p, a, b) = eastward_loop(quality, anchored);
        let original = serde_json::to_value(&p).unwrap();
        let adjusted = p.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
        for id in [a, b] {
            assert_station(&adjusted, id, 0, [0.0, 0.0, 0.0]);
            assert_station(&adjusted, id, 1, [expected_x, 0.0, 0.0]);
        }
        assert_eq!(serde_json::to_value(&p).unwrap(), original);
    }
}

#[test]
fn multishot_loop_and_spur_have_known_adjusted_positions() {
    let mut p = Project::new("Known loop with branch".into(), None);
    let a = p
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0); 2],
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 12.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let spur = p
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 2.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    p.set_zero_datum(
        StationRef::new(a, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    p.merge_stations(StationRef::new(a, 1), StationRef::new(spur, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(a, 2),
        StationRef::new(b, 1),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let original = serde_json::to_value(&p).unwrap();
    let adjusted = p.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
    // Three equal-variance shots absorb the 2 m gap: +2/3 on both A shots,
    // -2/3 on B. The spur keeps its 2 m displacement from the moved midpoint.
    assert_station(&adjusted, a, 0, [0.0, 0.0, 0.0]);
    assert_station(&adjusted, a, 1, [17.0 / 3.0, 0.0, 0.0]);
    assert_station(&adjusted, a, 2, [34.0 / 3.0, 0.0, 0.0]);
    assert_station(&adjusted, b, 1, [34.0 / 3.0, 0.0, 0.0]);
    assert_station(&adjusted, spur, 0, [17.0 / 3.0, 0.0, 0.0]);
    assert_station(&adjusted, spur, 1, [23.0 / 3.0, 0.0, 0.0]);
    assert_eq!(serde_json::to_value(&p).unwrap(), original);
}

#[test]
fn conflicting_datums_return_known_error_and_measured_fallback() {
    let (mut p, a, b) = eastward_loop(1.0, true);
    p.zero_datums.push(ZeroDatum {
        station_ref: p.key_for_ref(StationRef::new(b, 0)).unwrap(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });
    let config = ClosureAdjustmentConfig::default();
    assert_eq!(
        p.clone().adjust_loop_closures(
            StationRef::new(a, 0),
            ClosureStrategy::LeastSquaresWeighted,
            &config
        ),
        Err("Multiple zero datums in a single constraint group".into())
    );
    let adjusted = p.compute_adjusted_station_coordinates(&config);
    assert_station(&adjusted, a, 1, [10.0, 0.0, 0.0]);
    assert_station(&adjusted, b, 1, [12.0, 0.0, 0.0]);
}
