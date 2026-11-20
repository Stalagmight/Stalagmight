//! Solver v1 raw positions on hand-built surveys, and determinism.

use crate::{gps_to_local, AzimuthReference, CorrectionSource, LineId, StationId, StationRef};

use super::test_helpers::{
    add, add_on, assert_xy_near, at, flat, gps, project, shot, three_line_junction, xy_of, z_of,
};
use super::{Anchor, ClosureStrategy, PositionIssue, Project, RawPositions, StartAt, ZeroDatum};

const TOL: f64 = 1e-9;

#[test]
fn a_junction_of_three_lines_walks_every_line_from_the_placement() {
    let (p, [l0, l1, l2]) = three_line_junction();
    let positions = p.raw_positions();
    let s = |line: LineId, i: usize| at(&p, line, i);
    let h96 = 96.0_f64.sqrt();
    let h24 = 24.0_f64.sqrt();
    assert_xy_near(xy_of(&positions, s(l0, 0)), [100.0, 200.0], TOL, "s0");
    assert_xy_near(xy_of(&positions, s(l0, 1)), [110.0, 200.0], TOL, "s1");
    assert_xy_near(xy_of(&positions, s(l0, 2)), [110.0 + h96, 200.0], TOL, "s2");
    assert_xy_near(xy_of(&positions, s(l0, 3)), [120.0 + h96, 200.0], TOL, "s3");
    assert_xy_near(xy_of(&positions, s(l1, 1)), [110.0, 200.0 + h24], TOL, "a");
    assert_xy_near(xy_of(&positions, s(l1, 2)), [110.0, 205.0 + h24], TOL, "b");
    assert_xy_near(xy_of(&positions, s(l2, 0)), [127.0 + h96, 200.0], TOL, "x0");
    // The placement station sits exactly where the placement says.
    assert_eq!(xy_of(&positions, s(l0, 0)), [100.0, 200.0]);
    // Depth: absolute, the mean of the readings at each station.
    assert_eq!(z_of(&positions, s(l0, 2)), 1.0);
    assert_eq!(z_of(&positions, s(l1, 1)), 0.5);
    assert_eq!(z_of(&positions, s(l0, 1)), 0.0);
    assert!(positions.issues().is_empty());
    assert_eq!(
        positions.anchors(),
        &[Anchor::Placement {
            placement: 0,
            station: s(l0, 0),
            at: [100.0, 200.0],
        }]
    );
}

#[test]
fn a_free_piece_sits_where_its_placement_puts_a_non_start_station() {
    let mut p = project();
    let line = add(&mut p, StartAt::New, vec![flat(0.0, 10.0), flat(0.0, 10.0)]);
    let last = at(&p, line, 2);
    p.set_placement(last, 5.0, 5.0).unwrap();
    let positions = p.raw_positions();
    assert_xy_near(
        xy_of(&positions, at(&p, line, 0)),
        [5.0, -15.0],
        TOL,
        "start",
    );
    assert_xy_near(
        xy_of(&positions, at(&p, line, 1)),
        [5.0, -5.0],
        TOL,
        "middle",
    );
    assert_eq!(xy_of(&positions, last), [5.0, 5.0]);
}

/// Two pieces, each placed by a datum; the frame is the first.
fn two_datum_pieces() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = add(&mut p, StartAt::New, vec![flat(90.0, 10.0)]);
    let b = add(&mut p, StartAt::New, vec![flat(0.0, 10.0)]);
    p.push_datum(ZeroDatum::new(at(&p, a, 0), gps(20.0, -87.0)));
    p.push_datum(ZeroDatum::new(at(&p, b, 1), gps(20.001, -87.001)));
    (p, a, b)
}

#[test]
fn the_frame_datum_station_is_the_origin() {
    let (p, a, _) = two_datum_pieces();
    let positions = p.raw_positions();
    assert_eq!(positions.frame(), Some(0));
    let origin = xy_of(&positions, at(&p, a, 0));
    assert_eq!(origin[0].to_bits(), 0.0_f64.to_bits());
    assert_eq!(origin[1].to_bits(), 0.0_f64.to_bits());
    assert_xy_near(xy_of(&positions, at(&p, a, 1)), [10.0, 0.0], TOL, "a1");
}

#[test]
fn a_placement_datum_sits_on_its_gps_plus_its_correction() {
    let (mut p, a, b) = two_datum_pieces();
    let (gx, gy) = gps_to_local(&gps(20.001, -87.001), &gps(20.0, -87.0));
    let positions = p.raw_positions();
    assert_eq!(xy_of(&positions, at(&p, b, 1)), [gx, gy]);
    assert_xy_near(xy_of(&positions, at(&p, b, 0)), [gx, gy - 10.0], TOL, "b0");

    p.datums[1].correction = Some([3.0, -4.0]);
    p.datums[0].correction = Some([1.0, 2.0]);
    let positions = p.raw_positions();
    assert_eq!(xy_of(&positions, at(&p, a, 0)), [1.0, 2.0]);
    assert_eq!(xy_of(&positions, at(&p, b, 1)), [gx + 3.0, gy - 4.0]);
    assert_eq!(
        p.datum_anchor_position(1),
        Some([gx + 3.0, gy - 4.0]),
        "the anchor the solver uses is the one the API reports"
    );
}

#[test]
fn a_gps_fix_does_not_move_raw_geometry() {
    let (mut p, a, _) = two_datum_pieces();
    let before = p.raw_positions();
    // A second datum in piece A: a fix, far from where A's station sits.
    p.push_datum(ZeroDatum::new(at(&p, a, 1), gps(21.0, -88.0)));
    let after = p.raw_positions();
    assert_eq!(
        before.iter().collect::<Vec<_>>(),
        after.iter().collect::<Vec<_>>()
    );
    assert_eq!(p.placement_datums(), vec![Some(0), Some(1)]);
}

#[test]
fn hand_moves_apply_per_line_at_a_junction() {
    let mut p = project();
    let l0 = add(
        &mut p,
        StartAt::New,
        vec![shot(90.0, 10.0, 4.0, 4.0), shot(90.0, 10.0, 4.0, 4.0)],
    );
    let j = at(&p, l0, 1);
    let l1 = add(&mut p, StartAt::Station(j), vec![shot(0.0, 5.0, 7.0, 7.0)]);
    p.set_placement(at(&p, l0, 0), 0.0, 0.0).unwrap();
    p.line_mut(l0)
        .unwrap()
        .hand_moves
        .insert(j, [1.0, 0.0, 0.5]);
    p.line_mut(l1)
        .unwrap()
        .hand_moves
        .insert(j, [0.0, 2.0, -1.0]);
    assert_eq!(p.validate(), Vec::new());

    let positions = p.raw_positions();
    // J is shown where L0's walk plus L0's hand move puts it.
    assert_xy_near(xy_of(&positions, j), [11.0, 0.0], TOL, "J");
    // L0's walk continues from J's chain position, not its shown one.
    assert_xy_near(xy_of(&positions, at(&p, l0, 2)), [20.0, 0.0], TOL, "s2");
    // L1's chain at J is the shown position minus L1's own hand move.
    assert_xy_near(xy_of(&positions, at(&p, l1, 1)), [11.0, 3.0], TOL, "t1");
    // Z: the mean of 4, 4 and 7, plus each line's hand z per slot.
    assert_eq!(z_of(&positions, j), 5.0);
    let [jx, jy] = xy_of(&positions, j);
    assert_eq!(
        positions.slot(&p, StationRef::new(l0, 1)),
        Some([jx, jy, 5.5])
    );
    let on_l1 = positions.slot(&p, StationRef::new(l1, 0)).unwrap();
    assert_eq!(on_l1[2], 4.0);
    assert_eq!([on_l1[0], on_l1[1]], xy_of(&positions, j));
}

#[test]
fn the_wire_list_has_one_entry_per_slot_and_junction_slots_share_xy() {
    let (p, [l0, l1, l2]) = three_line_junction();
    let positions = p.raw_positions();
    let slots = positions.slots(&p);
    assert_eq!(slots.len(), 4 + 3 + 2);
    assert_eq!(slots[0].slot, StationRef::new(l0, 0));
    assert_eq!(slots[4].slot, StationRef::new(l1, 0));
    assert_eq!(slots[8].slot, StationRef::new(l2, 1));
    assert_eq!(slots[1].station, slots[4].station);
    assert_eq!((slots[1].x, slots[1].y), (slots[4].x, slots[4].y));
    assert_eq!((slots[3].x, slots[3].y), (slots[8].x, slots[8].y));
}

#[test]
fn a_junction_depth_averages_every_incident_reading_in_slot_order() {
    let mut p = project();
    let l0 = add(
        &mut p,
        StartAt::New,
        vec![shot(90.0, 10.0, 0.1, 0.7), shot(90.0, 10.0, 0.3, 0.9)],
    );
    let j = at(&p, l0, 1);
    let _l1 = add(&mut p, StartAt::Station(j), vec![shot(0.0, 5.0, 0.2, 0.2)]);
    let s4 = p.mint_station("e").unwrap();
    let _l2 = add_on(&mut p, &[s4, j], vec![shot(180.0, 5.0, 0.6, 0.4)]);
    p.set_placement(at(&p, l0, 0), 0.0, 0.0).unwrap();
    let positions = p.raw_positions();
    // Slot order by (line id, index): L0:1 (in 0.7, out 0.3), L1:0 (out 0.2),
    // L2:1 (in 0.4).
    let expected = [0.7, 0.3, 0.2, 0.4].iter().sum::<f64>() / 4.0;
    assert_eq!(z_of(&positions, j).to_bits(), expected.to_bits());
}

/// A loop: L0 a0 -> a1 east 10, L1 a0 -> b1 east `length`, closed by `strategy`.
fn closed_loop(length: f64, strategy: ClosureStrategy) -> (Project, StationId, StationId) {
    let mut p = project();
    // Slope sqrt(104) over a 2 m depth change: 10 m horizontally.
    let l0 = add(
        &mut p,
        StartAt::New,
        vec![shot(90.0, 104.0_f64.sqrt(), 0.0, 2.0)],
    );
    let a0 = at(&p, l0, 0);
    let l1 = add(
        &mut p,
        StartAt::Station(a0),
        vec![shot(90.0, length, 0.0, 6.0)],
    );
    p.set_placement(a0, 0.0, 0.0).unwrap();
    let (a1, b1) = (at(&p, l0, 1), at(&p, l1, 1));
    p.push_closure(a1, None, b1, None, strategy);
    assert_eq!(p.validate(), Vec::new());
    (p, a1, b1)
}

#[test]
fn a_tie_and_an_aligned_snap_share_depth_and_a_torn_snap_does_not() {
    let (p, a1, b1) = closed_loop(50.0, ClosureStrategy::Tie);
    let positions = p.raw_positions();
    assert_eq!(z_of(&positions, a1), 4.0);
    assert_eq!(z_of(&positions, b1), 4.0);

    // A snap within 1 cm horizontally (horizontal 10.005 m vs 10 m).
    let aligned_length = (10.005_f64 * 10.005 + 36.0).sqrt();
    let (p, a1, b1) = closed_loop(aligned_length, ClosureStrategy::Move);
    let positions = p.raw_positions();
    assert!(p.closure_is_aligned(&positions, &p.closures[0]));
    assert_eq!(z_of(&positions, a1), z_of(&positions, b1));

    let torn_length = (10.02_f64 * 10.02 + 36.0).sqrt();
    let (p, a1, b1) = closed_loop(torn_length, ClosureStrategy::Move);
    let positions = p.raw_positions();
    assert!(!p.closure_is_aligned(&positions, &p.closures[0]));
    assert_eq!(z_of(&positions, a1), 2.0);
    assert_eq!(z_of(&positions, b1), 6.0);

    // A batch closure never ties.
    let (p, a1, _) = closed_loop(aligned_length, ClosureStrategy::Batch);
    assert_eq!(z_of(&p.raw_positions(), a1), 2.0);
}

#[test]
fn ties_are_transitive_for_depth() {
    let (mut p, a1, b1) = closed_loop(50.0, ClosureStrategy::Tie);
    let l2 = add(
        &mut p,
        StartAt::Station(a1),
        vec![shot(0.0, 10.0, 0.0, 10.0)],
    );
    let c1 = at(&p, l2, 1);
    p.push_closure(b1, None, c1, None, ClosureStrategy::Tie);
    let positions = p.raw_positions();
    // a1: 2 (L0 out), 0 (L2 in); b1: 6; c1: 10.
    let expected = [2.0, 0.0, 6.0, 10.0].iter().sum::<f64>() / 4.0;
    for station in [a1, b1, c1] {
        assert_eq!(z_of(&positions, station), expected);
    }
}

#[test]
fn a_snap_between_two_datum_pieces_is_never_aligned() {
    let (mut p, a, b) = two_datum_pieces();
    // Put b's end 1 mm from a's end with a correction.
    let (gx, gy) = gps_to_local(&gps(20.001, -87.001), &gps(20.0, -87.0));
    p.datums[1].correction = Some([10.001 - gx, -gy]);
    let (a1, b1) = (at(&p, a, 1), at(&p, b, 1));
    p.push_closure(a1, None, b1, None, ClosureStrategy::Move);
    assert_eq!(p.validate(), Vec::new());
    let positions = p.raw_positions();
    assert_xy_near(xy_of(&positions, b1), [10.001, 0.0], 1e-6, "b1");
    assert!(!p.closure_is_aligned(&positions, &p.closures[0]));
}

#[test]
fn an_unanchored_component_is_laid_out_from_its_lowest_station_and_reported() {
    let mut p = project();
    let line = add(&mut p, StartAt::New, vec![flat(90.0, 3.0)]);
    let positions = p.raw_positions();
    let s0 = at(&p, line, 0);
    assert_eq!(xy_of(&positions, s0), [0.0, 0.0]);
    assert_eq!(
        positions.issues(),
        &[PositionIssue::UnanchoredComponent { lowest_station: s0 }]
    );
}

#[test]
fn the_shot_vector_is_bit_for_bit_the_legacy_one() {
    let references = [
        AzimuthReference::default(),
        AzimuthReference::magnetic(3.25, CorrectionSource::Manual),
        AzimuthReference::grid(-1.5, CorrectionSource::Manual),
    ];
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    for _ in 0..500 {
        let mut data = shot(next() * 360.0, next() * 30.0, next() * 50.0, next() * 50.0);
        data.azimuth_out = data.azimuth_in + (next() - 0.5) * 4.0;
        let legacy = crate::Shot::with_uuid(0, data.clone(), Some("u".to_string()));
        let new = super::Shot::from_data(0, data, 0);
        assert_eq!(
            new.horizontal_length().to_bits(),
            legacy.horizontal_length().to_bits()
        );
        for reference in references {
            assert_eq!(
                new.average_azimuth_true_rad(reference).to_bits(),
                legacy.average_azimuth_true_rad(reference).to_bits()
            );
        }
    }
}

#[test]
fn a_line_walks_with_its_azimuth_reference_or_the_project_default() {
    let mut p = project();
    let line = add(&mut p, StartAt::New, vec![flat(0.0, 10.0)]);
    p.set_placement(at(&p, line, 0), 0.0, 0.0).unwrap();
    p.settings.default_azimuth_reference =
        Some(AzimuthReference::magnetic(90.0, CorrectionSource::Manual));
    let positions = p.raw_positions();
    assert_xy_near(
        xy_of(&positions, at(&p, line, 1)),
        [10.0, 0.0],
        TOL,
        "default east",
    );
    p.line_mut(line).unwrap().azimuth_reference =
        Some(AzimuthReference::magnetic(180.0, CorrectionSource::Manual));
    let positions = p.raw_positions();
    assert_xy_near(
        xy_of(&positions, at(&p, line, 1)),
        [0.0, -10.0],
        TOL,
        "own south",
    );
}

// ----- determinism ---------------------------------------------------------

/// A small deterministic generator (an LCG), so the "random" surveys are the
/// same on every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn unit(&mut self) -> f64 {
        self.next() as f64 / f64::from(u32::MAX)
    }
}

/// A random forest of lines: each line starts at an existing station (a
/// junction) or a new one, with hand moves, depth changes, closures of every
/// rigid kind, and datums on some pieces; every other piece has a placement.
fn random_survey(seed: u64) -> Project {
    let mut rng = Lcg(seed);
    let mut p = project();
    for _ in 0..25 {
        let stations: Vec<StationId> = p.stations.keys().copied().collect();
        let start = if stations.is_empty() || rng.below(5) == 0 {
            StartAt::New
        } else {
            StartAt::Station(stations[rng.below(stations.len())])
        };
        let count = 1 + rng.below(6);
        let shots = (0..count)
            .map(|_| {
                let depth_in = rng.unit() * 40.0;
                shot(
                    rng.unit() * 360.0,
                    1.0 + rng.unit() * 20.0,
                    depth_in,
                    depth_in + (rng.unit() - 0.5) * 2.0,
                )
            })
            .collect();
        let line = add(&mut p, start, shots);
        if rng.below(4) == 0 {
            let chain = p.line_stations(line).unwrap().to_vec();
            let station = chain[rng.below(chain.len())];
            let hand = [rng.unit() - 0.5, rng.unit() - 0.5, rng.unit() - 0.5];
            p.line_mut(line).unwrap().hand_moves.insert(station, hand);
        }
    }
    // Loop closures inside components, of every kind.
    let topology = p.topology_arc();
    for (c, component) in topology.components().iter().enumerate() {
        if component.stations.len() > 3 {
            let a = component.stations[rng.below(component.stations.len())];
            let b = component.stations[rng.below(component.stations.len())];
            if a != b {
                let strategy = [
                    ClosureStrategy::Tie,
                    ClosureStrategy::Move,
                    ClosureStrategy::Batch,
                ][c % 3];
                p.push_closure(a, None, b, None, strategy);
            }
        }
    }
    // Datums on every other piece (a fix too on some), placements elsewhere.
    for (c, component) in topology.components().iter().enumerate() {
        let station = component.stations[rng.below(component.stations.len())];
        if c % 2 == 0 {
            let lat = 20.0 + rng.unit() * 0.01;
            let lon = -87.0 + rng.unit() * 0.01;
            p.push_datum(ZeroDatum::new(station, gps(lat, lon)));
            if c % 4 == 0 {
                let fix = component.stations[rng.below(component.stations.len())];
                p.push_datum(ZeroDatum::new(fix, gps(lat + 0.0001, lon)));
            }
        } else {
            p.set_placement(station, rng.unit() * 100.0, rng.unit() * 100.0)
                .unwrap();
        }
    }
    p
}

fn bits(positions: &RawPositions) -> Vec<(StationId, [u64; 3])> {
    positions
        .iter()
        .map(|(s, [x, y, z])| (s, [x.to_bits(), y.to_bits(), z.to_bits()]))
        .collect()
}

#[test]
fn the_random_surveys_are_valid_and_fully_positioned() {
    for seed in 0..20 {
        let p = random_survey(seed);
        assert_eq!(p.validate(), Vec::new(), "seed {seed}");
        let positions = p.raw_positions();
        assert!(positions.issues().is_empty());
        assert_eq!(positions.iter().count(), p.stations.len());
        assert!(positions
            .iter()
            .all(|(_, xyz)| xyz.iter().all(|v| v.is_finite())));
    }
}

#[test]
fn building_the_same_survey_twice_gives_identical_models_and_bits() {
    for seed in 0..10 {
        let (a, b) = (random_survey(seed), random_survey(seed));
        assert_eq!(a, b);
        assert_eq!(bits(&a.raw_positions()), bits(&b.raw_positions()));
        let json_a = serde_json::to_string(&a).unwrap();
        assert_eq!(json_a, serde_json::to_string(&b).unwrap());
        let mut back: Project = serde_json::from_str(&json_a).unwrap();
        back.assign_session_shot_ids();
        assert_eq!(back, a, "serde round trip, seed {seed}");
        assert_eq!(bits(&back.raw_positions()), bits(&a.raw_positions()));
    }
}

#[test]
fn reordering_lines_and_closures_changes_no_bit() {
    for seed in 0..10 {
        let p = random_survey(seed);
        let reference = bits(&p.raw_positions());
        let mut rng = Lcg(seed ^ 0xABCD);
        let mut shuffled = p.clone();
        let lines = shuffled.lines_mut();
        for i in (1..lines.len()).rev() {
            lines.swap(i, rng.below(i + 1));
        }
        for i in (1..shuffled.closures.len()).rev() {
            let j = rng.below(i + 1);
            shuffled.closures.swap(i, j);
        }
        assert_eq!(bits(&shuffled.raw_positions()), reference, "seed {seed}");
    }
}

#[test]
fn reordering_datums_where_roles_do_not_change_changes_no_bit() {
    for seed in 0..10 {
        let p = random_survey(seed);
        let reference = bits(&p.raw_positions());
        let topology = p.topology_arc();
        // Keep the frame first and each component's placement datum ahead of
        // its fixes; reverse everything else.
        let roles = p.placement_datums();
        let frame = p.frame_datum().unwrap();
        let mut order: Vec<usize> = vec![frame];
        let mut placement: Vec<usize> = roles
            .iter()
            .flatten()
            .copied()
            .filter(|&d| d != frame)
            .collect();
        placement.reverse();
        order.extend(placement);
        let mut fixes: Vec<usize> = (0..p.datums.len()).filter(|d| !order.contains(d)).collect();
        fixes.reverse();
        order.extend(fixes);
        let mut shuffled = p.clone();
        shuffled.datums = order.iter().map(|&d| p.datums[d]).collect();
        assert_eq!(shuffled.frame_datum(), Some(0));
        for (c, _) in topology.components().iter().enumerate() {
            let before = roles[c].map(|d| p.datums[d].station);
            let after = shuffled.placement_datums()[c].map(|d| shuffled.datums[d].station);
            assert_eq!(before, after);
        }
        assert_eq!(bits(&shuffled.raw_positions()), reference, "seed {seed}");
    }
}
