//! The Closures Computed View, closure alignment, the across-one-shot skip
//! set and closure metrics against the legacy model, on equivalent surveys
//! built with `models::project` (`legacy_convert`): every slot's adjusted
//! position within 1e-6 m of `compute_adjusted_stations`, the same skipped
//! closures and failures, the same misclosures; plus determinism.

use std::collections::BTreeMap;

use crate::config::ClosureAdjustmentConfig;
use crate::{ClosureStrategy as OldStrategy, StationRef};

use super::legacy_convert::{
    convert, data, old_adjusted, old_line, old_raw, random_legacy_survey, Converted, Mix, Old,
};
use super::{AdjustedPositions, ClosureStrategy, Project};

const ADJ_TOL: f64 = 1e-6;

fn mixes() -> Vec<Mix> {
    vec![
        Mix::default(),
        Mix {
            closures: 8,
            datums: 2,
            ..Mix::default()
        },
        Mix {
            closures: 3,
            datums: 7,
            snaps: 2,
            ..Mix::default()
        },
        Mix {
            lines: 18,
            closures: 6,
            datums: 5,
            snaps: 2,
            across_one_shot: 2,
            virtual_shots: true,
        },
        Mix {
            lines: 6,
            closures: 3,
            datums: 0,
            snaps: 1,
            across_one_shot: 1,
            virtual_shots: false,
        },
    ]
}

fn surveys(seeds: u64) -> Vec<(String, Old)> {
    let mut out = Vec::new();
    for (k, mix) in mixes().into_iter().enumerate() {
        for seed in 0..seeds {
            let seed = seed * 7919 + k as u64 * 104_729 + 11;
            out.push((
                format!("mix {k} seed {seed}"),
                random_legacy_survey(seed, mix),
            ));
        }
    }
    out
}

/// Every slot of `adjusted` is within `ADJ_TOL` of the legacy view.
fn assert_adjusted_match(
    what: &str,
    conv: &Converted,
    expected: &BTreeMap<StationRef, [f64; 3]>,
    adjusted: &AdjustedPositions,
) {
    let slots = adjusted.slots(&conv.project);
    assert_eq!(slots.len(), expected.len(), "{what}: slot count");
    for slot in slots {
        let [x, y, z] = expected[&slot.slot];
        let gap = ((slot.x - x).powi(2) + (slot.y - y).powi(2)).sqrt();
        assert!(
            gap <= ADJ_TOL,
            "{what} slot {}: adjusted ({}, {}) is {gap} m from legacy ({x}, {y})",
            slot.slot,
            slot.x,
            slot.y
        );
        assert!((slot.z - z).abs() <= 1e-9, "{what} slot {}: z", slot.slot);
    }
}

#[test]
fn adjusted_positions_match_the_legacy_view() {
    let config = ClosureAdjustmentConfig::default();
    let (mut solved, mut skipped, mut failed, mut fixes, mut snaps) = (0, 0, 0, 0, 0);
    for (what, old) in surveys(30) {
        let conv = convert(&old);
        let (expected, old_failures) = old_adjusted(&old, &config);
        let adjusted = conv.project.adjusted_positions(&config);
        assert_adjusted_match(&what, &conv, &expected, &adjusted);

        // Same failures (by anchor slot), same skipped closures.
        let new_failures: Vec<StationRef> =
            adjusted.failures().iter().map(|f| f.anchor_slot).collect();
        assert_eq!(new_failures, old_failures, "{what}: failures");
        let old_skipped: Vec<(u64, (u32, usize))> = old
            .closures_across_one_shot()
            .iter()
            .map(|c| (c.closure_id, c.collapsed_shot))
            .collect();
        let new_skipped: Vec<(u64, (u32, usize))> = adjusted
            .skipped()
            .iter()
            .map(|c| (c.closure_id, c.collapsed_shot))
            .collect();
        assert_eq!(new_skipped, old_skipped, "{what}: skip set");

        let raw = old_raw(&old);
        let moved = expected
            .iter()
            .any(|(slot, p)| ((p[0] - raw[slot][0]).abs() + (p[1] - raw[slot][1]).abs()) > 1e-6);
        solved += usize::from(moved);
        skipped += old_skipped.len();
        failed += old_failures.len();
        fixes += conv
            .project
            .datum_roles()
            .iter()
            .filter(|r| **r == Some(crate::station::DatumRole::GpsFix))
            .count();
        let aligned = conv
            .project
            .closure_alignment(&conv.project.raw_positions());
        snaps += conv
            .project
            .closures
            .iter()
            .zip(&aligned)
            .filter(|(c, a)| c.strategy == ClosureStrategy::Move && **a)
            .count();
    }
    assert!(
        solved > 60,
        "the generator must make solved views ({solved})"
    );
    assert!(
        skipped > 5,
        "the generator must make skipped closures ({skipped})"
    );
    assert!(fixes > 30, "the generator must make GPS fixes ({fixes})");
    assert!(
        snaps > 10,
        "the generator must make aligned snaps ({snaps})"
    );
    assert!(
        failed >= 1,
        "the generator must make a failing solve ({failed})"
    );
}

#[test]
fn alignment_skip_set_and_new_closure_check_match_the_legacy_model() {
    for (what, old) in surveys(12) {
        let conv = convert(&old);
        let p = &conv.project;
        let aligned = p.closure_alignment(&p.raw_positions());
        for (closure, (old_closure, &is_aligned)) in p
            .closures
            .iter()
            .zip(old.loop_closures.iter().zip(&aligned))
        {
            assert_eq!(closure.id, old_closure.id);
            if old_closure.strategy == OldStrategy::Move {
                assert_eq!(
                    is_aligned, old_closure.is_aligned,
                    "{what}: closure {}",
                    closure.id
                );
            }
            if closure.strategy == ClosureStrategy::Tie {
                assert!(is_aligned);
            }
        }
        // A new closure between any two slots: same verdict as today.
        let slots: Vec<StationRef> = old_raw(&old).keys().copied().collect();
        for (i, &a) in slots.iter().enumerate().step_by(3) {
            let b = slots[(i * 7 + 5) % slots.len()];
            let expected = old.closure_spans_one_shot(a, b);
            let actual = p.closure_spans_one_shot(conv.station(a), conv.station(b));
            if conv.station(a) == conv.station(b) {
                continue;
            }
            assert_eq!(actual, expected, "{what}: new closure {a} - {b}");
        }
    }
}

#[test]
fn misclosures_match_the_legacy_model_and_path_length_is_along_shots() {
    let mut checked = 0;
    for (what, old) in surveys(12) {
        let conv = convert(&old);
        let p = &conv.project;
        let metrics = p.closures_metrics(&p.raw_positions());
        for (closure, m) in old.loop_closures.iter().zip(&metrics) {
            let (Some(a), Some(b)) = (
                old.resolve_key(&closure.station_a),
                old.resolve_key(&closure.station_b),
            ) else {
                continue;
            };
            let Some(m) = m else {
                panic!("{what}: closure {} has metrics", closure.id);
            };
            let Ok(expected) = old.compute_closure_metrics(a, b) else {
                continue;
            };
            assert!(
                (m.misclosure_distance - expected.misclosure_distance).abs() <= 1e-9,
                "{what}"
            );
            assert!(
                (m.horizontal_misclosure - expected.horizontal_misclosure).abs() <= 1e-9,
                "{what}"
            );
            assert!(
                (m.vertical_misclosure - expected.vertical_misclosure).abs() <= 1e-9,
                "{what}"
            );
            // The new path length: shots (and merges, which are junctions
            // now) only, never a closure.
            let along_shots = old.calculate_merge_only_path_length(a, b);
            if p.topology().connected(conv.station(a), conv.station(b)) {
                assert!(
                    (m.path_length - along_shots).abs() <= 1e-9 * along_shots.max(1.0),
                    "{what}: closure {} path {} vs shots-only {along_shots}",
                    closure.id,
                    m.path_length
                );
                assert!(
                    m.path_length + 1e-9 >= expected.path_length,
                    "{what}: never shorter than through closures"
                );
            } else {
                assert_eq!(m.path_length, 0.0);
                assert_eq!(m.error_percentage, 0.0);
            }
            checked += 1;
        }
    }
    assert!(checked > 50, "closures checked ({checked})");
}

/// Today's `looped_project_with_misclosure`: two paths from a datum, 10 m
/// east then 10 m north, and 10 m north then 10.2 m east, closed at the ends.
fn looped_legacy() -> Old {
    let mut old = Old::new("loop".to_string(), None);
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
            data(90.0, 90.0, 10.2, 0.0, 0.0),
        ],
    );
    let origin = crate::GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
    old.set_zero_datum(StationRef::new(a, 0), origin).unwrap();
    old.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    old.create_loop_closure(
        StationRef::new(a, 2),
        StationRef::new(b, 2),
        OldStrategy::Batch,
    )
    .unwrap();
    old
}

#[test]
fn a_loop_with_a_misclosure_is_closed_and_the_datum_stays() {
    let old = looped_legacy();
    let conv = convert(&old);
    let config = ClosureAdjustmentConfig::default();
    let adjusted = conv.project.adjusted_positions(&config);
    let a2 = conv.station(StationRef::new(0, 2));
    let b2 = conv.station(StationRef::new(1, 2));
    // The closure ends meet, the datum station stays at the origin.
    assert_eq!(adjusted.xy(a2), adjusted.xy(b2));
    assert_eq!(
        adjusted.xy(conv.station(StationRef::new(0, 0))),
        Some([0.0, 0.0])
    );
    let raw = conv.project.raw_positions();
    assert!(adjusted.xy(a2).unwrap()[0] > raw.xy(a2).unwrap()[0]);
    assert!(adjusted.failures().is_empty());
    // Depths are never adjusted.
    for (station, [_, _, z]) in adjusted.iter() {
        assert_eq!(Some(z), raw.z(station));
    }
}

#[test]
fn a_spur_keeps_its_measured_shots() {
    let mut old = looped_legacy();
    let spur = old_line(
        &mut old,
        vec![
            data(45.0, 45.0, 6.0, 0.0, 0.0),
            data(135.0, 135.0, 4.0, 0.0, 1.0),
        ],
    );
    old.merge_stations(StationRef::new(spur, 0), StationRef::new(0, 1))
        .unwrap();
    let conv = convert(&old);
    let adjusted = conv
        .project
        .adjusted_positions(&ClosureAdjustmentConfig::default());
    let raw = conv.project.raw_positions();
    let s: Vec<_> = (0..3)
        .map(|i| conv.station(StationRef::new(spur, i)))
        .collect();
    for w in s.windows(2) {
        let (pa, pb) = (adjusted.xy(w[0]).unwrap(), adjusted.xy(w[1]).unwrap());
        let (ra, rb) = (raw.xy(w[0]).unwrap(), raw.xy(w[1]).unwrap());
        assert!(((pb[0] - pa[0]) - (rb[0] - ra[0])).abs() < 1e-12);
        assert!(((pb[1] - pa[1]) - (rb[1] - ra[1])).abs() < 1e-12);
    }
    let (expected, _) = old_adjusted(&old, &ClosureAdjustmentConfig::default());
    assert_adjusted_match("spur", &conv, &expected, &adjusted);
}

#[test]
fn a_free_component_is_held_at_its_placement_station() {
    // A datum-less loop: the placement station is the gauge and stays put.
    let mut old = looped_legacy();
    old.zero_datums.clear();
    old.line_offsets.insert(0, (30.0, -20.0));
    let conv = convert(&old);
    let p = &conv.project;
    assert_eq!(p.placements.len(), 1);
    let gauge = p.placements[0];
    let adjusted = p.adjusted_positions(&ClosureAdjustmentConfig::default());
    assert_eq!(adjusted.xy(gauge.station), Some([gauge.x, gauge.y]));
    let (expected, _) = old_adjusted(&old, &ClosureAdjustmentConfig::default());
    assert_adjusted_match("free loop", &conv, &expected, &adjusted);
}

#[test]
fn two_placement_datums_tied_by_a_closure_fail_and_stay_measured() {
    let mut old = Old::new("two datums".to_string(), None);
    let a = old_line(&mut old, vec![data(90.0, 90.0, 10.0, 0.0, 0.0)]);
    let b = old_line(&mut old, vec![data(0.0, 0.0, 10.0, 0.0, 0.0)]);
    let ga = crate::GpsCoordinates::new(20.5, -87.4, 0.0).unwrap();
    let (lat, lon) = crate::local_to_gps(30.0, 5.0, &ga).unwrap();
    old.set_zero_datum(StationRef::new(a, 1), ga).unwrap();
    old.set_zero_datum(
        StationRef::new(b, 1),
        crate::GpsCoordinates::new(lat, lon, 0.0).unwrap(),
    )
    .unwrap();
    old.create_loop_closure(
        StationRef::new(a, 1),
        StationRef::new(b, 1),
        OldStrategy::Batch,
    )
    .unwrap();
    let conv = convert(&old);
    let config = ClosureAdjustmentConfig::default();
    let adjusted = conv.project.adjusted_positions(&config);
    let (expected, old_failures) = old_adjusted(&old, &config);
    assert_eq!(old_failures.len(), 1, "fixture: the legacy solve fails");
    assert_eq!(adjusted.failures().len(), 1);
    assert_eq!(
        adjusted.failures()[0].error,
        super::AdjustError::MultiplePlacementDatums
    );
    assert_eq!(adjusted.failures()[0].anchor_slot, old_failures[0]);
    assert_eq!(adjusted.failures()[0].lines, vec![a, b]);
    assert_adjusted_match("two datums", &conv, &expected, &adjusted);
    let report = conv.project.adjustment_report(Some(&adjusted));
    let old_report_reason = old.compute_adjusted_stations(&config).failures[0].user_reason();
    assert_eq!(report.failures[0].reason, old_report_reason);
}

/// The same survey with its line list shuffled: identical bits.
fn shuffled(p: &Project, seed: u64) -> Project {
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
fn shuffled_lines_and_repeated_solves_give_identical_bits() {
    let config = ClosureAdjustmentConfig::default();
    let mut compared = 0;
    for (what, old) in surveys(8) {
        let conv = convert(&old);
        let p = &conv.project;
        let first = p.adjusted_positions(&config);
        let again = p.adjusted_positions(&config);
        let bits = |a: &AdjustedPositions| -> Vec<(u32, [u64; 3])> {
            a.iter()
                .map(|(s, [x, y, z])| (s, [x.to_bits(), y.to_bits(), z.to_bits()]))
                .collect()
        };
        assert_eq!(bits(&first), bits(&again), "{what}: repeated solve");
        for seed in 1..4 {
            let q = shuffled(p, seed);
            assert_eq!(
                bits(&q.adjusted_positions(&config)),
                bits(&first),
                "{what}: shuffled lines"
            );
        }
        // Closures reversed: same bits when the order does not change the
        // skip set (no closure is skipped).
        if first.skipped().is_empty() {
            let mut q = p.clone();
            q.closures.reverse();
            assert_eq!(
                bits(&q.adjusted_positions(&config)),
                bits(&first),
                "{what}: closures reversed"
            );
        }
        // A JSON round trip changes nothing either.
        let json = serde_json::to_string(p).unwrap();
        let mut back: Project = serde_json::from_str(&json).unwrap();
        back.assign_session_shot_ids();
        assert_eq!(
            bits(&back.adjusted_positions(&config)),
            bits(&first),
            "{what}: round trip"
        );
        compared += 1;
    }
    assert!(compared > 30);
}

/// Scale: a 26 000-station survey (a long ladder of loops plus 150 datum
/// pieces, each with a GPS fix and a closure) solves in linear time (about
/// 0.1 s in a debug build).
#[test]
fn a_large_survey_solves_in_linear_time() {
    use super::test_helpers::{add, flat, gps};
    use super::{StartAt, ZeroDatum};
    let mut p = super::test_helpers::project();
    let rail = |n: usize, az: f64| -> Vec<crate::ShotData> {
        (0..n)
            .map(|i| flat(az + (i % 7) as f64, 5.0 + (i % 5) as f64 * 0.1))
            .collect()
    };
    let a = add(&mut p, StartAt::New, rail(10_000, 90.0));
    let o = super::test_helpers::at(&p, a, 0);
    let b = add(&mut p, StartAt::Station(o), rail(10_000, 91.0));
    p.push_datum(ZeroDatum::new(o, gps(20.5, -87.4)));
    for k in 1..=200 {
        let (sa, sb) = (
            super::test_helpers::at(&p, a, k * 50),
            super::test_helpers::at(&p, b, k * 50),
        );
        p.push_closure(sa, None, sb, None, ClosureStrategy::Batch);
    }
    for k in 0..150_i32 {
        let l = add(&mut p, StartAt::New, rail(40, f64::from(k % 360)));
        let chain: Vec<crate::StationId> = p.line(l).unwrap().stations().collect();
        let (s0, s9) = (chain[0], chain[39]);
        let mut d = ZeroDatum::new(s0, gps(20.5 + f64::from(k) * 1e-4, -87.39));
        d.via = Some(l);
        p.push_datum(d);
        let mut fix = ZeroDatum::new(s9, gps(20.5 + f64::from(k) * 1e-4, -87.388));
        fix.accuracy_m = Some(5.0);
        p.push_datum(fix);
        p.push_closure(chain[20], None, s9, None, ClosureStrategy::Batch);
    }
    assert!(p.validate().is_empty());
    let started = std::time::Instant::now();
    let adjusted = p.adjusted_positions(&ClosureAdjustmentConfig::default());
    let elapsed = started.elapsed();
    assert!(adjusted.failures().is_empty());
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "solve took {elapsed:?}"
    );
    let started = std::time::Instant::now();
    let raw = p.raw_positions();
    let metrics = p.closures_metrics(&raw);
    let aligned = p.closure_alignment(&raw);
    let frozen = p.frozen_shots(&aligned);
    let field = p.distance_field(&raw, None, false);
    let elapsed = started.elapsed();
    assert_eq!(metrics.len(), 350);
    // Batch closures are not rigid: nothing is frozen.
    assert!(frozen.is_empty());
    assert!(field.max_reachable() > 0.0);
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "derived views took {elapsed:?}"
    );
}
