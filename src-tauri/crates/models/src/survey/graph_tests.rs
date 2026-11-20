//! Datum roles, entrances and GPS gaps against the legacy model: equivalent
//! surveys built with the frozen 0.6.0 model (`legacy_convert`) give the same
//! roles, entrances, gaps and far fixes.
//!
//! Retired when `models::project` was deleted: the per-slot penetration,
//! distance field and ruler comparisons (`compute_penetration_distances_with`,
//! `distance_field_from_with`, `along_line_between_with` are not in the frozen
//! crate, which carries only what the 0.6.0 open needs). The station-graph
//! views keep the line-order determinism test (`derived_tests`) and the K
//! goldens (C18), which dump the distance field and the ruler per scenario.

use std::collections::BTreeMap;

use crate::StationRef;

use super::legacy_convert::{convert, live, random_legacy_survey, relive, Mix, Old};

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
fn roles_entrances_and_gps_gaps_match_the_legacy_model() {
    let mut fixes = 0;
    for (seed, old) in surveys() {
        let conv = convert(&old);
        let p = &conv.project;
        let old_roles: Vec<Option<crate::DatumRole>> =
            old.datum_roles().iter().map(relive).collect();
        assert_eq!(p.datum_roles(), old_roles, "seed {seed}: roles");
        assert_eq!(
            p.datum_entrances(),
            old.datum_entrances(),
            "seed {seed}: entrances"
        );
        let mut old_entrances: Vec<_> = old
            .entrance_datum_refs()
            .into_iter()
            .map(|slot| conv.station(live(slot)))
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
            let actual = by_slot[&live(*slot)];
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
        let far: Vec<_> = p
            .far_gps_fixes()
            .iter()
            .map(super::FarFix::to_wire)
            .collect();
        let old_far = old.far_gps_fixes();
        assert_eq!(far.len(), old_far.len());
        for (a, e) in far.iter().zip(&old_far) {
            assert_eq!(a.station, live(e.station), "seed {seed}: far fix order");
        }
    }
    assert!(fixes > 10, "the generator must make GPS fixes ({fixes})");
}
