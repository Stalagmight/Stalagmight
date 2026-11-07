//! along_line_between (the #26 ruler): the direct and the along-cave-line distance.
//!
//! Lines 3273-3818 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// ---------------------------------------------------------------------------
// along_line_between (#26 ruler: direct + along-cave-line distance + path shots)
// ---------------------------------------------------------------------------

fn ep_station(section_id: SectionId, station_index: u32) -> RulerEndpoint {
    RulerEndpoint {
        section_id,
        station_index: Some(station_index),
        shot_id: None,
        t: None,
    }
}

fn ep_shot(section_id: SectionId, shot_id: ShotId, t: f64) -> RulerEndpoint {
    RulerEndpoint {
        section_id,
        station_index: None,
        shot_id: Some(shot_id),
        t: Some(t),
    }
}

#[test]
fn along_line_simple_linear_section() {
    let mut project = Project::new("Linear".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let shots: Vec<ShotId> = project
        .get_section(a)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 3));
    assert!(
        (r.along_line.unwrap() - 30.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![
            PathShot {
                section_id: a,
                shot_id: shots[0]
            },
            PathShot {
                section_id: a,
                shot_id: shots[1]
            },
            PathShot {
                section_id: a,
                shot_id: shots[2]
            },
        ],
    );
    assert_eq!(
        r.path_stations,
        vec![
            StationRef::new(a, 0),
            StationRef::new(a, 1),
            StationRef::new(a, 2),
            StationRef::new(a, 3),
        ],
    );
}

#[test]
fn along_line_cross_section_merge_skips_merge_edge() {
    let mut project = Project::new("Merge".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    let a_shot = project.get_section(a).unwrap().shots[0].id;
    let b_shot = project.get_section(b).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(b, 1));
    assert!(
        (r.along_line.unwrap() - 10.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    // Two shots; the zero-cost merge edge is NOT emitted as a shot.
    assert_eq!(
        r.path_shots,
        vec![
            PathShot {
                section_id: a,
                shot_id: a_shot
            },
            PathShot {
                section_id: b,
                shot_id: b_shot
            },
        ],
    );
}

#[test]
fn along_line_routes_through_loop_closure() {
    // Parity with the distance-gradient view (#34): loop closures are zero-cost
    // edges, so a closed loop gives a shorter route than the direct long shot.
    let mut project = Project::new("Closure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // Tie A.0<->B.0 and A.1<->B.2 with loop closures (not merges).
    for (sa, sb) in [
        (StationRef::new(a, 0), StationRef::new(b, 0)),
        (StationRef::new(a, 1), StationRef::new(b, 2)),
    ] {
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: sa.into(),
            station_b: sb.into(),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }
    let b_shots: Vec<ShotId> = project
        .get_section(b)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();
    let a_shot = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 1));
    assert!(
        (r.along_line.unwrap() - 2.0).abs() < 1e-6,
        "should route via closure, got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![
            PathShot {
                section_id: b,
                shot_id: b_shots[0]
            },
            PathShot {
                section_id: b,
                shot_id: b_shots[1]
            },
        ],
    );
    assert!(
        !r.path_shots.iter().any(|s| s.shot_id == a_shot),
        "the long direct shot must not be on the path"
    );
}

#[test]
fn along_line_shot_point_partial_length() {
    let mut project = Project::new("ShotPoint".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    // Station 0 to a point 25% along shot 0 → 2.5 m; whole shot tinted.
    let r = project.along_line_between(&stations, ep_station(a, 0), ep_shot(a, s0, 0.25));
    assert!(
        (r.along_line.unwrap() - 2.5).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![PathShot {
            section_id: a,
            shot_id: s0
        }]
    );
    // Only station 0 is a real node on the path; the shot-point endpoint is virtual.
    assert_eq!(r.path_stations, vec![StationRef::new(a, 0)]);
}

#[test]
fn along_line_same_shot_endpoints() {
    let mut project = Project::new("SameShot".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_shot(a, s0, 0.2), ep_shot(a, s0, 0.7));
    assert!(
        (r.along_line.unwrap() - 5.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![PathShot {
            section_id: a,
            shot_id: s0
        }]
    );
    // Two mid-shot points cross no station.
    assert!(r.path_stations.is_empty());
}

#[test]
fn along_line_same_shot_prefers_shorter_route_around() {
    // Two points on one long shot whose endpoints are ALSO tied by a zero-cost loop
    // closure: routing around the closure (0.1·100 + 0 + 0.1·100 = 20) beats the
    // direct sub-segment (0.8·100 = 80), so the same-shot case must NOT short-circuit
    // to the direct segment - Dijkstra picks the genuine shortest path (#26, Codex).
    let mut project = Project::new("SameShotRoute".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(a, 1).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_shot(a, s0, 0.1), ep_shot(a, s0, 0.9));
    assert!(
        (r.along_line.unwrap() - 20.0).abs() < 1e-6,
        "should route around, got {:?}",
        r.along_line
    );
    // The end-legs of the shot are traversed (v→A.0 and A.1→v), but the middle
    // A.0→A.1 leg uses the closure, not the shot - still exactly one shot entry.
    assert_eq!(
        r.path_shots,
        vec![PathShot {
            section_id: a,
            shot_id: s0
        }]
    );
}

#[test]
fn along_line_parallel_closure_does_not_tint_shot() {
    // A loop closure parallel to a real shot (same station pair): measuring between
    // the two stations routes via the zero-cost closure, so the shot - which the
    // path did NOT traverse - must not be tinted (#26, Codex).
    let mut project = Project::new("ParallelClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(a, 1).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 1));
    assert_eq!(r.along_line, Some(0.0), "routes via the 0-cost closure");
    assert!(
        r.path_shots.is_empty(),
        "the parallel shot must not be tinted: {:?}",
        r.path_shots
    );
}

#[test]
fn along_line_zero_length_same_shot_has_empty_path() {
    // Both endpoints are the exact same point on a shot: distance 0 and NO tint,
    // matching identical station endpoints (#26, Codex).
    let mut project = Project::new("ZeroLenShot".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_shot(a, s0, 0.5), ep_shot(a, s0, 0.5));
    assert_eq!(r.along_line, Some(0.0), "got {:?}", r.along_line);
    assert!(
        r.path_shots.is_empty(),
        "zero-length same-shot must not tint: {:?}",
        r.path_shots
    );
}

#[test]
fn along_line_disconnected_returns_none() {
    let mut project = Project::new("Disconnected".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(b, 1));
    assert_eq!(r.along_line, None);
    assert!(r.path_shots.is_empty());
}

#[test]
fn along_line_identical_and_degenerate_t() {
    let mut project = Project::new("Degenerate".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    // Identical endpoints → 0, empty path.
    let same = project.along_line_between(&stations, ep_station(a, 1), ep_station(a, 1));
    assert_eq!(same.along_line, Some(0.0));
    assert!(same.path_shots.is_empty());

    // t≈0 on shot 0 collapses to station 0; t≈1 collapses to station 1.
    let t0 = project.along_line_between(&stations, ep_shot(a, s0, 0.0), ep_station(a, 2));
    let st = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 2));
    assert!((t0.along_line.unwrap() - st.along_line.unwrap()).abs() < 1e-9);
    assert_eq!(t0.path_shots, st.path_shots);
}

#[test]
fn along_line_skips_merge_and_closure_phantoms() {
    // A=merge=B=closure=C; A.0 -> C.1 should list exactly 3 shots, no phantom
    // entries for the zero-cost merge/closure ties.
    let mut project = Project::new("NoPhantom".to_string(), None);
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
    let c = project
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
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(c, 1));
    assert!(
        (r.along_line.unwrap() - 3.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots.len(),
        3,
        "exactly 3 shots, no merge/closure phantoms: {:?}",
        r.path_shots
    );
}

#[test]
fn along_line_reference_shot_is_traversed_and_tinted() {
    // Reference shots are real measured legs (inherited #34 behavior): they are
    // graph edges and get tinted like any shot.
    let mut project = Project::new("RefShot".to_string(), None);
    let mut ref_shot = shot(0.0, 0.0, 7.0, 0.0, 0.0);
    ref_shot.kind = ShotKind::Jump;
    let a = project
        .add_section(SectionData {
            shots: vec![ref_shot],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 1));
    assert!(
        (r.along_line.unwrap() - 7.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![PathShot {
            section_id: a,
            shot_id: s0
        }]
    );
}

#[test]
fn along_line_follows_passed_coordinates_not_measured_length() {
    // The along-line distance sums the euclidean distance between the coordinates
    // in `stations`, NOT the raw shot.length (#26): feeding the closure-ADJUSTED
    // coordinates yields the adjusted shot lengths the user asked for. Here we feed
    // coordinates that deliberately disagree with the 10 m measured lengths and
    // confirm the result follows the coordinates (3 + 25 = 28, not 10 + 10 = 20).
    let mut project = Project::new("AdjustedLengths".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let s1 = project.get_section(a).unwrap().shots[1].id;
    let mut stations = project.compute_station_coordinates();
    let set = |stations: &mut Vec<Station>, station_index: u32, y: f64| {
        let i = stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(a, station_index))
            .unwrap();
        stations[i].coordinates = Coordinates::new(0.0, y, 0.0);
    };
    set(&mut stations, 0, 0.0);
    set(&mut stations, 1, 3.0); // shot 0 → 3 m apart
    set(&mut stations, 2, 28.0); // shot 1 → 25 m apart

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 2));
    assert!(
        (r.along_line.unwrap() - 28.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );

    // A mid-shot endpoint's partial leg uses the same coordinate-derived length:
    // 40 % along shot 1 (25 m apart) = 10 m, plus shot 0's 3 m = 13 m.
    let r2 = project.along_line_between(&stations, ep_station(a, 0), ep_shot(a, s1, 0.4));
    assert!(
        (r2.along_line.unwrap() - 13.0).abs() < 1e-6,
        "got {:?}",
        r2.along_line
    );
}
