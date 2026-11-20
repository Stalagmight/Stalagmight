//! Rigid groups, anchored lines and immovable stations against the legacy
//! model, on equivalent surveys built with the frozen 0.6.0 model
//! (`legacy_convert`); frozen shots on a hand-checked fixture; and the
//! derived views' independence of the line order.
//!
//! Retired when `models::project` was deleted: the frozen-shot and stats
//! comparisons (`compute_frozen_shots`, `Project::stats`, `depth_range`,
//! `calculate_penetration_with` are not in the frozen crate, which carries
//! only what the 0.6.0 open needs). The station-graph frozen shots and stats
//! keep their unit tests and the K goldens (C18), which dump both per
//! scenario.

use std::collections::{BTreeSet, VecDeque};

use stalagmight_legacy::models as old;

use crate::{LineId, StationRef};

use super::legacy_convert::{convert, data, live, old_line, random_legacy_survey, Mix, Old};

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
        .map(|(i, s)| (live(s.station_ref), i))
        .collect();
    let mut aligned_ends: std::collections::BTreeMap<StationRef, Vec<StationRef>> =
        std::collections::BTreeMap::new();
    for c in old.loop_closures.iter().filter(|c| c.is_aligned) {
        if let (Some(a), Some(b)) = (
            old.resolve_key(&c.station_a).map(live),
            old.resolve_key(&c.station_b).map(live),
        ) {
            aligned_ends.entry(a).or_default().push(b);
            aligned_ends.entry(b).or_default().push(a);
        }
    }
    let mut out: BTreeSet<StationRef> = BTreeSet::new();
    let mut queue: VecDeque<StationRef> = stations
        .iter()
        .filter(|s| s.is_zero_datum)
        .map(|s| live(s.station_ref))
        .collect();
    while let Some(slot) = queue.pop_front() {
        if !out.insert(slot) {
            continue;
        }
        let Some(&i) = by_ref.get(&slot) else {
            continue;
        };
        queue.extend(stations[i].merged_with.iter().copied().map(live));
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
    let g = old::GpsCoordinates::new(20.5, -87.4, 0.0).unwrap();
    let at = old::StationRef::new;
    old.set_zero_datum(at(a, 0), g).unwrap();
    old.merge_stations(at(b, 0), at(a, 0)).unwrap();
    old.merge_stations(at(tail, 0), at(a, 1)).unwrap();
    old.create_loop_closure(at(a, 2), at(b, 2), old::ClosureStrategy::Move)
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
    // The loop's four shots are frozen (0.6.0 froze the same four); the tail
    // (a spur) is not.
    assert_eq!(frozen, vec![(a, 0), (a, 1), (b, 0), (b, 1)]);
}

/// The same survey with its line list shuffled.
fn shuffled(p: &super::Project, seed: u64) -> super::Project {
    let mut q = p.clone();
    let mut rng = super::legacy_convert::Lcg(seed);
    let lines = q.lines_mut();
    for i in (1..lines.len()).rev() {
        let j = rng.below(i + 1);
        lines.swap(i, j);
    }
    q
}

#[test]
fn derived_views_ignore_the_line_order_bit_for_bit() {
    for (what, old) in surveys().into_iter().take(30) {
        let conv = convert(&old);
        let p = &conv.project;
        let fingerprint = |q: &super::Project| {
            let raw = q.raw_positions();
            let aligned = q.closure_alignment(&raw);
            let mut frozen: Vec<(LineId, u32)> = q
                .frozen_shots(&aligned)
                .iter()
                .map(|f| (f.line_id, f.shot_index))
                .collect();
            frozen.sort_unstable();
            let metrics: Vec<[u64; 5]> = q
                .closures_metrics(&raw)
                .iter()
                .flatten()
                .map(|m| {
                    [
                        m.path_length.to_bits(),
                        m.misclosure_distance.to_bits(),
                        m.error_percentage.to_bits(),
                        m.horizontal_misclosure.to_bits(),
                        m.vertical_misclosure.to_bits(),
                    ]
                })
                .collect();
            let field: Vec<u64> = q
                .distance_field(&raw, None, false)
                .iter()
                .map(|(_, d)| d.to_bits())
                .collect();
            let pen: Vec<u64> = q
                .penetration_distances(true)
                .iter()
                .map(|(_, d)| d.to_bits())
                .collect();
            let gaps: Vec<(usize, u64)> = q
                .gps_fix_gaps()
                .iter()
                .map(|(d, g)| (*d, g.gap_m.to_bits()))
                .collect();
            let groups = q.rigid_groups(&aligned);
            let anchored = q.anchored_lines(&groups);
            let immovable = q.immovable_stations(&q.tie_groups(&aligned));
            let skipped: Vec<(u64, (LineId, usize))> = q
                .closures_across_one_shot_with(&aligned)
                .iter()
                .map(|c| (c.closure_id, c.collapsed_shot))
                .collect();
            let stats = q.survey_stats(&raw, false);
            (
                frozen,
                metrics,
                field,
                pen,
                gaps,
                anchored,
                immovable,
                skipped,
                stats.penetration_distance.to_bits(),
                stats.max_depth.to_bits(),
            )
        };
        let expected = fingerprint(p);
        for seed in 1..4 {
            assert_eq!(
                fingerprint(&shuffled(p, seed)),
                expected,
                "{what}: seed {seed}"
            );
        }
        // The merge pairs name each junction's slots.
        for (a, b) in p.merge_pairs() {
            assert_eq!(p.station_at(a), p.station_at(b));
            assert_ne!(a, b);
        }
    }
}
