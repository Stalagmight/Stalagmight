//! The survey graph, roles, entrances and GPS gaps against the legacy model:
//! equivalent surveys built with `models::project` give the same per-slot
//! penetration, distance field, ruler, datum roles, entrances and gaps.

use std::collections::BTreeMap;

use crate::{RulerEndpoint, StationRef};

use super::legacy_convert::{convert, random_legacy_survey, Converted, Mix, Old};
use super::{RulerPoint, StationDistances};

const TOL: f64 = 1e-9;

fn slots(old: &Old) -> Vec<StationRef> {
    old.compute_station_coordinates()
        .into_iter()
        .map(|s| s.station_ref)
        .collect()
}

fn assert_distances(
    what: &str,
    old: &Old,
    conv: &Converted,
    old_d: &[f64],
    new: &StationDistances,
) {
    for (slot, &expected) in slots(old).iter().zip(old_d) {
        let actual = new.get(conv.station(*slot));
        match (expected.is_finite(), actual) {
            (false, None) => {}
            (true, Some(a)) => assert!(
                (a - expected).abs() <= TOL * expected.abs().max(1.0),
                "{what} slot {slot}: {a} vs legacy {expected}"
            ),
            _ => panic!("{what} slot {slot}: {actual:?} vs legacy {expected}"),
        }
    }
}

fn surveys() -> Vec<(u64, Old)> {
    (0..30)
        .map(|seed| {
            let mix = Mix {
                across_one_shot: usize::from(seed % 4 == 0),
                ..Mix::default()
            };
            (seed, random_legacy_survey(seed, mix))
        })
        .collect()
}

#[test]
fn penetration_matches_the_legacy_model() {
    let mut reachable = 0;
    for (seed, old) in surveys() {
        let conv = convert(&old);
        let stations = old.compute_station_coordinates();
        for include_reference in [false, true] {
            let old_d = old.compute_penetration_distances_with(&stations, include_reference);
            let new = conv.project.penetration_distances(include_reference);
            assert_distances(&format!("seed {seed}"), &old, &conv, &old_d, &new);
            let old_max = old.calculate_penetration_with(&stations, include_reference);
            let new_max = conv.project.penetration(include_reference);
            assert!(
                (old_max - new_max).abs() <= TOL * old_max.max(1.0),
                "seed {seed}"
            );
            reachable += old_d.iter().filter(|d| d.is_finite()).count();
        }
    }
    assert!(
        reachable > 200,
        "the generator must reach stations ({reachable})"
    );
}

#[test]
fn distance_fields_match_the_legacy_model() {
    for (seed, old) in surveys() {
        let conv = convert(&old);
        let stations = old.compute_station_coordinates();
        let raw = conv.project.raw_positions();
        let all = slots(&old);
        let picks = [
            None,
            Some(all[seed as usize % all.len()]),
            Some(all[all.len() - 1]),
        ];
        for from in picks {
            for include_reference in [false, true] {
                let old_field =
                    old.distance_field_from_with(&stations, from.as_ref(), include_reference);
                let new = conv.project.distance_field(
                    &raw,
                    from.map(|slot| conv.station(slot)),
                    include_reference,
                );
                let wire = new.to_distance_field(&conv.project);
                assert_eq!(wire.station_refs, old_field.station_refs, "seed {seed}");
                for ((slot, a), e) in wire
                    .station_refs
                    .iter()
                    .zip(&wire.distances)
                    .zip(&old_field.distances)
                {
                    match (a, e) {
                        (None, None) => {}
                        (Some(a), Some(e)) => assert!(
                            (a - e).abs() <= 1e-6,
                            "seed {seed} from {from:?} slot {slot}: {a} vs legacy {e}"
                        ),
                        _ => panic!("seed {seed} from {from:?} slot {slot}: {a:?} vs legacy {e:?}"),
                    }
                }
                assert!((wire.max_reachable - old_field.max_reachable).abs() <= 1e-6);
            }
        }
    }
}

#[test]
fn ruler_matches_the_legacy_model() {
    let mut measured = 0;
    for (seed, old) in surveys() {
        let conv = convert(&old);
        let stations = old.compute_station_coordinates();
        let raw = conv.project.raw_positions();
        let all = slots(&old);
        let n = all.len();
        let ends = [
            (all[0], all[n - 1]),
            (all[seed as usize % n], all[(seed as usize * 7 + 3) % n]),
        ];
        for (a, b) in ends {
            let ea = RulerEndpoint {
                line_id: a.line_id,
                station_index: Some(a.station_index),
                shot_id: None,
                t: None,
            };
            // The second end is a point on the shot that leaves `b` (or
            // arrives at it, for a line's last station).
            let line = old.get_line(b.line_id).unwrap_or_else(|| panic!("line"));
            let eb = if line.shots.is_empty() {
                RulerEndpoint {
                    line_id: b.line_id,
                    station_index: Some(b.station_index),
                    shot_id: None,
                    t: None,
                }
            } else {
                let i = (b.station_index as usize).min(line.shots.len() - 1);
                RulerEndpoint {
                    line_id: b.line_id,
                    station_index: None,
                    shot_id: Some(line.shots[i].id),
                    t: Some(0.37),
                }
            };
            for include_reference in [false, true] {
                let expected = old.along_line_between_with(&stations, ea, eb, include_reference);
                let (Some(pa), Some(pb)) = (
                    RulerPoint::from_endpoint(&conv.project, &ea),
                    RulerPoint::from_endpoint(&conv.project, &eb),
                ) else {
                    panic!("seed {seed}: endpoints resolve");
                };
                let actual = conv
                    .project
                    .along_line_between(&raw, pa, pb, include_reference);
                match (actual.along_line, expected.along_line) {
                    (None, None) => {}
                    (Some(x), Some(y)) => {
                        assert!((x - y).abs() <= 1e-6, "seed {seed}: {x} vs legacy {y}");
                        // The tinted shots are the same set (a route through a
                        // junction is unique on a tree; loops may tie).
                        let mut s1 = actual.path_shots.clone();
                        let mut s2 = expected.path_shots.clone();
                        s1.sort_by_key(|p| (p.line_id, p.shot_id));
                        s2.sort_by_key(|p| (p.line_id, p.shot_id));
                        if old.loop_closures.is_empty() {
                            assert_eq!(s1, s2, "seed {seed}: tinted shots");
                        }
                        measured += 1;
                    }
                    other => panic!("seed {seed}: {other:?}"),
                }
            }
        }
    }
    assert!(
        measured > 30,
        "the generator must give connected pairs ({measured})"
    );
}

#[test]
fn roles_entrances_and_gps_gaps_match_the_legacy_model() {
    let mut fixes = 0;
    for (seed, old) in surveys() {
        let conv = convert(&old);
        let p = &conv.project;
        assert_eq!(p.datum_roles(), old.datum_roles(), "seed {seed}: roles");
        assert_eq!(
            p.datum_entrances(),
            old.datum_entrances(),
            "seed {seed}: entrances"
        );
        let mut old_entrances: Vec<_> = old
            .entrance_datum_refs()
            .into_iter()
            .map(|slot| conv.station(slot))
            .collect();
        old_entrances.dedup();
        let mut seen = Vec::new();
        old_entrances.retain(|s| {
            let fresh = !seen.contains(s);
            seen.push(*s);
            fresh
        });
        assert_eq!(
            p.entrance_stations(),
            old_entrances,
            "seed {seed}: entrance stations"
        );

        let old_gaps = old.gps_fix_gaps();
        let new_gaps = p.gps_fix_gaps();
        assert_eq!(old_gaps.len(), new_gaps.len(), "seed {seed}: gap count");
        let by_slot: BTreeMap<StationRef, _> = new_gaps
            .iter()
            .map(|(d, gap)| (p.datum_slot(&p.datums[*d]).unwrap(), *gap))
            .collect();
        for (slot, expected) in &old_gaps {
            let actual = by_slot[slot];
            assert!((actual.gap_m - expected.gap_m).abs() <= 1e-6, "seed {seed}");
            assert!(
                (actual.bearing_deg - expected.bearing_deg).abs() <= 1e-6 || expected.gap_m < 1e-6
            );
            assert!((actual.threshold_m - expected.threshold_m).abs() <= 1e-9);
            assert_eq!(
                actual.survey_distance_m.is_some(),
                expected.survey_distance_m.is_some()
            );
            assert_eq!(actual.exceeds_threshold, expected.exceeds_threshold);
            fixes += 1;
        }
        let far: Vec<_> = p.far_gps_fixes().iter().map(|f| f.to_wire()).collect();
        let old_far = old.far_gps_fixes();
        assert_eq!(far.len(), old_far.len());
        for (a, e) in far.iter().zip(&old_far) {
            assert_eq!(a.station, e.station, "seed {seed}: far fix order");
        }
    }
    assert!(fixes > 10, "the generator must make GPS fixes ({fixes})");
}
