//! Frozen shots, rigid groups, anchored lines, immovable stations and stats
//! against the legacy model, on equivalent surveys built with
//! `models::project` (`legacy_convert`).

use std::collections::{BTreeSet, VecDeque};

use crate::{LineId, StationRef};

use super::legacy_convert::{convert, data, old_line, random_legacy_survey, Mix, Old};

fn surveys() -> Vec<(String, Old)> {
    let mixes = [
        Mix::default(),
        Mix {
            closures: 7,
            datums: 6,
            snaps: 3,
            ..Mix::default()
        },
        Mix {
            lines: 16,
            closures: 3,
            datums: 3,
            snaps: 2,
            across_one_shot: 1,
            virtual_shots: true,
        },
    ];
    let mut out = Vec::new();
    for (k, mix) in mixes.into_iter().enumerate() {
        for seed in 0..25 {
            let seed = seed * 31 + k as u64 * 1_000_003 + 5;
            out.push((
                format!("mix {k} seed {seed}"),
                random_legacy_survey(seed, mix),
            ));
        }
    }
    out
}

#[test]
fn frozen_shots_match_the_legacy_model() {
    let mut frozen = 0;
    for (what, old) in surveys() {
        let conv = convert(&old);
        let p = &conv.project;
        let aligned = p.closure_alignment(&p.raw_positions());
        let expected = old.compute_frozen_shots();
        let actual = p.frozen_shots(&aligned);
        let key = |v: &[crate::FrozenShotRef]| -> Vec<(LineId, u32)> {
            v.iter().map(|f| (f.line_id, f.shot_index)).collect()
        };
        assert_eq!(key(&actual), key(&expected), "{what}: frozen shots");
        frozen += actual.len();
        for f in actual.iter().take(3) {
            assert!(p.is_shot_frozen(f.line_id, f.shot_index as usize));
        }
    }
    assert!(frozen > 50, "the generator must freeze shots ({frozen})");
}

#[test]
fn rigid_groups_and_anchored_lines_match_the_legacy_model() {
    for (what, old) in surveys() {
        let conv = convert(&old);
        let p = &conv.project;
        let aligned = p.closure_alignment(&p.raw_positions());
        let groups = p.rigid_groups(&aligned);
        for line in &old.lines {
            let mut expected: Vec<LineId> = old
                .get_rigid_connected_line_ids(line.id)
                .into_iter()
                .collect();
            expected.sort_unstable();
            assert_eq!(
                p.rigid_lines_of(line.id, &groups),
                expected,
                "{what}: line {}",
                line.id
            );
        }
        // Today's `AppState::get_anchored_line_ids`.
        let mut anchored: BTreeSet<LineId> = BTreeSet::new();
        for zd in &old.zero_datums {
            anchored.extend(old.get_rigid_connected_line_ids(zd.station_ref.line_id));
        }
        let expected: Vec<LineId> = anchored.into_iter().collect();
        assert_eq!(
            p.anchored_lines(&groups),
            expected,
            "{what}: anchored lines"
        );
    }
}

/// The documented rule of today's frontend `computeImmovableStationKeys` on
/// the legacy model: from every datum slot, across merges and ALIGNED
/// closures. (The frontend itself walks `closures_with`, whose chain-transitive
/// entries carry the FAR closure's id, so an unaligned closure followed by an
/// aligned one also counts there; the station graph keeps the documented rule.)
fn legacy_immovable(old: &Old) -> BTreeSet<StationRef> {
    let stations = old.compute_station_coordinates();
    let by_ref: std::collections::BTreeMap<StationRef, usize> = stations
        .iter()
        .enumerate()
        .map(|(i, s)| (s.station_ref, i))
        .collect();
    let mut aligned_ends: std::collections::BTreeMap<StationRef, Vec<StationRef>> =
        std::collections::BTreeMap::new();
    for c in old.loop_closures.iter().filter(|c| c.is_aligned) {
        if let (Some(a), Some(b)) = (old.resolve_key(&c.station_a), old.resolve_key(&c.station_b)) {
            aligned_ends.entry(a).or_default().push(b);
            aligned_ends.entry(b).or_default().push(a);
        }
    }
    let mut out: BTreeSet<StationRef> = BTreeSet::new();
    let mut queue: VecDeque<StationRef> = stations
        .iter()
        .filter(|s| s.is_zero_datum)
        .map(|s| s.station_ref)
        .collect();
    while let Some(slot) = queue.pop_front() {
        if !out.insert(slot) {
            continue;
        }
        let Some(&i) = by_ref.get(&slot) else {
            continue;
        };
        queue.extend(stations[i].merged_with.iter().copied());
        queue.extend(aligned_ends.get(&slot).into_iter().flatten().copied());
    }
    out
}

#[test]
fn immovable_stations_match_the_legacy_move_tools() {
    let mut immovable = 0;
    for (what, old) in surveys() {
        let conv = convert(&old);
        let p = &conv.project;
        let aligned = p.closure_alignment(&p.raw_positions());
        let ties = p.tie_groups(&aligned);
        let expected: BTreeSet<_> = legacy_immovable(&old)
            .into_iter()
            .map(|slot| conv.station(slot))
            .collect();
        let actual: BTreeSet<_> = p.immovable_stations(&ties).into_iter().collect();
        assert_eq!(actual, expected, "{what}: immovable stations");
        immovable += actual.len();
    }
    assert!(
        immovable > 100,
        "the generator must make immovable stations ({immovable})"
    );
}

#[test]
fn stats_match_the_legacy_model() {
    for (what, old) in surveys() {
        let conv = convert(&old);
        let p = &conv.project;
        let raw = p.raw_positions();
        let stations = old.compute_station_coordinates();
        for include_reference in [false, true] {
            let mut expected = old.stats();
            let (min, max) = Old::depth_range(&stations);
            expected.min_depth = min;
            expected.max_depth = max;
            expected.penetration_distance =
                old.calculate_penetration_with(&stations, include_reference);
            let stats = p.survey_stats(&raw, include_reference);
            let actual = stats.to_project_stats();
            assert_eq!(actual.line_count, expected.line_count, "{what}");
            assert_eq!(actual.shot_count, expected.shot_count, "{what}");
            assert_eq!(
                actual.station_count, expected.station_count,
                "{what}: slots"
            );
            assert_eq!(actual.loop_count, expected.loop_count, "{what}");
            assert_eq!(
                actual.total_length.to_bits(),
                expected.total_length.to_bits(),
                "{what}"
            );
            assert!(
                (actual.min_depth - expected.min_depth).abs() <= 1e-9,
                "{what}"
            );
            assert!(
                (actual.max_depth - expected.max_depth).abs() <= 1e-9,
                "{what}"
            );
            assert!(
                (actual.penetration_distance - expected.penetration_distance).abs()
                    <= 1e-9 * expected.penetration_distance.max(1.0),
                "{what}"
            );
            assert_eq!(actual.first_survey_date, expected.first_survey_date);
            assert_eq!(actual.last_survey_date, expected.last_survey_date);
            assert_eq!(stats.station_count, p.topology().stations().len());
            assert!(stats.station_count <= stats.slot_count);
        }
    }
}

#[test]
fn a_datum_anchored_loop_freezes_its_shots_and_a_bridge_between_datums() {
    // A: datum at A0, B from A0, closure snapped... here a plain loop closed
    // by a merge is impossible, so the loop is closed by an aligned snap.
    let mut old = Old::new("frozen".to_string(), None);
    let a = old_line(
        &mut old,
        vec![
            data(90.0, 90.0, 10.0, 0.0, 0.0),
            data(0.0, 0.0, 10.0, 0.0, 0.0),
        ],
    );
    let b = old_line(
        &mut old,
        vec![
            data(0.0, 0.0, 10.0, 0.0, 0.0),
            data(90.0, 90.0, 10.0, 0.0, 0.0),
        ],
    );
    let tail = old_line(&mut old, vec![data(180.0, 180.0, 5.0, 0.0, 0.0)]);
    let g = crate::GpsCoordinates::new(20.5, -87.4, 0.0).unwrap();
    old.set_zero_datum(StationRef::new(a, 0), g).unwrap();
    old.merge_stations(StationRef::new(b, 0), StationRef::new(a, 0))
        .unwrap();
    old.merge_stations(StationRef::new(tail, 0), StationRef::new(a, 1))
        .unwrap();
    old.create_loop_closure(
        StationRef::new(a, 2),
        StationRef::new(b, 2),
        crate::ClosureStrategy::Move,
    )
    .unwrap();
    let conv = convert(&old);
    let p = &conv.project;
    let aligned = p.closure_alignment(&p.raw_positions());
    assert_eq!(aligned, vec![true], "fixture: the snap is aligned");
    let frozen: Vec<(LineId, u32)> = p
        .frozen_shots(&aligned)
        .iter()
        .map(|f| (f.line_id, f.shot_index))
        .collect();
    // The loop's four shots are frozen; the tail (a spur) is not.
    assert_eq!(frozen, vec![(a, 0), (a, 1), (b, 0), (b, 1)]);
    let expected: Vec<(LineId, u32)> = old
        .compute_frozen_shots()
        .iter()
        .map(|f| (f.line_id, f.shot_index))
        .collect();
    assert_eq!(frozen, expected);
}
