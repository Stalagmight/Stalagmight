//! Synthetic projects for the legacy tests, written against the 0.6.0 model
//! API through the alias `m`, which the including test crate binds to either
//! the frozen copy (`stalagmight_legacy::models`) or the live 0.6.0 crate
//! (`stalagmight_models`, in `stalagmight-legacy-fidelity`). The two APIs are
//! the same code, so one builder makes the same project in both.
//!
//! Every value here is invented; no real survey data.

use super::m::{
    ClosureStrategy, GpsCoordinates, LineData, LineId, Project, ShotData, StationRef,
    SurveyDirection,
};

/// One shot of `length` metres on `azimuth` (both readings are forward
/// azimuths), descending `dz`.
fn shot(azimuth: f64, length: f64, depth_in: f64, dz: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out: depth_in + dz,
        left: Some(0.5),
        right: Some(0.75),
        ..Default::default()
    }
}

fn line(name: &str, legs: &[(f64, f64)], depth: f64) -> LineData {
    let mut shots = Vec::new();
    let mut z = depth;
    for (i, &(azimuth, length)) in legs.iter().enumerate() {
        let dz = if i % 2 == 0 { 0.5 } else { -0.25 };
        shots.push(shot(azimuth, length, z, dz));
        z += dz;
    }
    LineData {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#336699".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots,
    }
}

fn at(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

fn add(project: &mut Project, data: LineData) -> LineId {
    project.add_line(data).id
}

fn merge(project: &mut Project, a: StationRef, b: StationRef) {
    project.merge_stations(a, b).unwrap();
}

/// Merges, a Batch closure and an aligned Move closure, a placement datum and
/// a GPS fix in one component, a second datum-placed component, two free
/// components (one a merged pair) with stored line offsets, a hand move,
/// station names and comments, a Section, and a drifted line offset for the
/// load to heal.
pub(crate) fn kitchen_sink() -> Project {
    let mut p = sink_without_drift();
    // Drift a merged line's stored offset: the open re-solves it.
    let looped = p.lines[2].id;
    let (x, y) = p.line_offset(looped);
    p.line_offsets.insert(looped, (x + 0.5, y - 0.25));
    p
}

/// The kitchen sink with the merge into the GPS-fixed line torn instead: an
/// older build could save that, and the open demotes the merge to a closure
/// rather than move a datum-placed piece (#741).
pub(crate) fn torn_datum_merge() -> Project {
    let mut p = sink_without_drift();
    let side = p.lines[1].id;
    let (x, y) = p.line_offset(side);
    p.line_offsets.insert(side, (x + 0.5, y - 0.25));
    p
}

fn sink_without_drift() -> Project {
    let mut p = Project::new("Synthetic".to_string(), Some("Nowhere".to_string()));
    let main = add(
        &mut p,
        line(
            "Main",
            &[
                (90.0, 10.0),
                (0.0, 10.0),
                (90.0, 10.0),
                (0.0, 10.0),
                (45.0, 5.0),
                (135.0, 5.0),
            ],
            2.0,
        ),
    );
    p.set_zero_datum(at(main, 0), GpsCoordinates::new(20.5, -87.25, 0.0).unwrap())
        .unwrap();

    let side = add(
        &mut p,
        line("Side", &[(180.0, 7.0), (200.0, 6.5), (170.0, 4.0)], 3.0),
    );
    merge(&mut p, at(main, 2), at(side, 0));

    let looped = add(
        &mut p,
        line("Loop", &[(10.0, 6.0), (300.0, 9.0), (250.0, 8.0)], 4.0),
    );
    merge(&mut p, at(main, 4), at(looped, 0));
    // A real loop, not across one shot: Loop's end to Side's end.
    p.create_loop_closure(at(looped, 3), at(side, 3), ClosureStrategy::Batch)
        .unwrap();

    // A resurvey of Main's first two legs ends on Main:2: an aligned Move tie.
    let bypass = add(&mut p, line("Bypass", &[(90.0, 10.0), (0.0, 10.0)], 2.0));
    merge(&mut p, at(main, 0), at(bypass, 0));
    p.create_loop_closure(at(bypass, 2), at(main, 2), ClosureStrategy::Move)
        .unwrap();

    // A second datum in Main's component is a GPS fix (#741).
    p.set_zero_datum(
        at(side, 3),
        GpsCoordinates::new(20.5001, -87.2499, 0.0).unwrap(),
    )
    .unwrap();

    // A separate piece placed by its own datum.
    let far = add(&mut p, line("Far", &[(270.0, 12.0), (260.0, 11.0)], 6.0));
    p.set_zero_datum(
        at(far, 0),
        GpsCoordinates::new(20.501, -87.251, 0.0).unwrap(),
    )
    .unwrap();

    // Free components: one line, and a merged pair, each displaced.
    let loose = add(&mut p, line("Loose", &[(30.0, 3.0), (60.0, 4.0)], 1.0));
    p.set_line_offset(loose, (123.25, -45.5));
    let pair_a = add(&mut p, line("PairA", &[(120.0, 5.0), (110.0, 5.0)], 8.0));
    let pair_b = add(&mut p, line("PairB", &[(200.0, 6.0)], 9.0));
    merge(&mut p, at(pair_a, 2), at(pair_b, 0));
    for id in [pair_a, pair_b] {
        let (x, y) = p.line_offset(id);
        p.set_line_offset(id, (x - 310.0, y + 88.125));
    }

    // A hand move (`station_offsets`, which only old builds created).
    let key = p.key_for_ref(at(looped, 2)).unwrap();
    p.station_offsets.insert(key, (0.75, -0.5, 0.25));

    p.set_station_name(at(main, 3), "A3").unwrap();
    let key = p.key_for_ref(at(side, 1)).unwrap();
    p.station_comments.insert(key, "squeeze".to_string());
    let section = p.add_section("North".to_string(), "#aa3300".to_string());
    p.set_line_section(main, Some(section));
    p.set_line_section(side, Some(section));
    p
}

/// A hub station with many lines merged on it and a loose line: the merge
/// cascade and gauge rules over a wide group.
pub(crate) fn hub(spokes: usize) -> Project {
    let mut p = Project::new("Hub".to_string(), None);
    let trunk = add(&mut p, line("Trunk", &[(90.0, 10.0), (100.0, 9.0)], 1.0));
    for i in 0..spokes {
        let azimuth = (i as f64 * 37.0) % 360.0;
        let spoke = add(
            &mut p,
            line(
                &format!("S{i}"),
                &[(azimuth, 4.0), (azimuth + 10.0, 3.0)],
                2.0,
            ),
        );
        merge(&mut p, at(trunk, 1), at(spoke, 0));
    }
    let spoke_a = p.lines[1].id;
    let spoke_b = p.lines[2].id;
    p.create_loop_closure(at(spoke_a, 2), at(spoke_b, 2), ClosureStrategy::Batch)
        .unwrap();
    add(&mut p, line("Loose", &[(10.0, 2.0)], 0.0));
    p
}

/// The kitchen sink after the edits that move gauges and keys around: a
/// reversed line and a split one.
pub(crate) fn edited() -> Project {
    let mut p = kitchen_sink();
    let looped = p.lines[2].id;
    assert!(p.reverse_line(looped, true));
    let main = p.lines[0].id;
    p.split_line_at_station(main, 3, &split_options()).unwrap();
    p
}

fn split_options() -> super::m::SplitLineOptions {
    super::m::SplitLineOptions {
        name_a: "Main A".to_string(),
        name_b: "Main B".to_string(),
        color_a: "#112233".to_string(),
        color_b: "#445566".to_string(),
        comment_side: super::m::SplitSide::A,
        datum_side: super::m::SplitSide::A,
    }
}

/// No datum anywhere: every component is held by a gauge.
pub(crate) fn datumless() -> Project {
    let mut p = Project::new("Floating".to_string(), None);
    let a = add(
        &mut p,
        line("A", &[(0.0, 5.0), (45.0, 5.0), (90.0, 5.0)], 1.0),
    );
    let b = add(&mut p, line("B", &[(180.0, 5.0), (225.0, 5.0)], 1.0));
    merge(&mut p, at(a, 3), at(b, 0));
    p.create_loop_closure(at(a, 0), at(b, 2), ClosureStrategy::Batch)
        .unwrap();
    p.set_line_offset(a, (5.5, -2.25));
    p
}

/// A datum-less piece whose gauge an edit moved off its lowest-id line: the
/// reversed line's gauge (its far end) goes to the split's new half, so the
/// split records a held gauge.
pub(crate) fn held_gauge() -> Project {
    let mut p = datumless();
    let a = p.lines[0].id;
    assert!(p.reverse_line(a, false));
    p.split_line_at_station(a, 1, &split_options()).unwrap();
    p
}

/// Every builder, by name.
pub(crate) fn all() -> Vec<(&'static str, Project)> {
    vec![
        ("kitchen_sink", kitchen_sink()),
        ("torn_datum_merge", torn_datum_merge()),
        ("hub", hub(12)),
        ("edited", edited()),
        ("datumless", datumless()),
        ("held_gauge", held_gauge()),
    ]
}
