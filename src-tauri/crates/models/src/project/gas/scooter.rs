//! Plan segments and the DPV range cap (#317) - the pure, graph-free half of scooter support.
//!
//! The engine proper ([`super`]) owns the graph walk; everything here is arithmetic over the
//! plan's own inputs, which is what keeps it testable without building a project.

use crate::config::DpvRig;

use crate::gas::PlanSegment;

/// One route span with its speed already decided - what the engine actually walks.
///
/// Distinct from [`PlanSegment`] on purpose. A `PlanSegment` carries `Option<f64>`, where `None`
/// means "fly this span at the plan's speed"; by the time the graph is weighted that choice must
/// already be made, and a type that cannot represent "undecided" is what stops the resolution step
/// from being skipped again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ResolvedSegment {
    pub(super) speed_m_per_min: f64,
    pub(super) on_scooter: bool,
    /// Metres of the rig's usable RANGE one metre of line on this span costs - the engine's
    /// scootered-distance rider, so it is `0.0` on a swum span. See [`range_per_metre`] for why it
    /// is not simply `1.0` on a scootered one.
    pub(super) range_per_metre: f64,
}

pub use crate::gas::{MAX_SEGMENT_SPEED_M_PER_MIN, MIN_SEGMENT_SPEED_M_PER_MIN};

/// The cruise speed the range cap is built from: the slower of the two units.
///
/// [`DpvRig::usable_range_m`] picks the weaker unit by LINE COVERED (`burn x speed`), which need not
/// be the slower one - so this is deliberately the more conservative of the two questions. It is
/// only ever used to inflate the range a below-cruise span spends, and the slowest cruise gives the
/// largest inflation, so picking it can never over-state how far the rig will carry the diver (§6).
fn slower_cruise_m_per_min(rig: &DpvRig) -> f64 {
    rig.primary.speed_m_per_min.min(rig.backup.speed_m_per_min)
}

/// Metres of scooter RANGE one metre of line costs on a scootered span ridden at `speed_m_per_min`.
///
/// The cap is a distance (`burn x cruising speed`, less the reserve), but what actually runs out is
/// battery MINUTES, and the two only convert 1:1 at the unit's cruise speed. A span ridden at a
/// quarter of cruise takes four times the minutes the cap's own arithmetic assumed for those metres,
/// so charging a flat metre per metre EXTENDS reach the slower the diver rides - the unsafe
/// direction (§6), and reachable-overlay ground the battery cannot actually deliver.
///
/// So a below-cruise span spends the allowance time-equivalently: `slower_cruise / speed` metres of
/// range per metre of line. At or above cruise the factor floors at exactly `1.0`, which keeps the
/// plain line model - a span cannot stretch the battery by going FASTER (drag rises with speed, so
/// the unit's burn figure would not hold there either) - and that floor is what leaves every
/// at-cruise plan, the cap itself, the Settings preview and their pinned formula tests untouched.
///
/// A non-finite ratio yields `1.0`: `f64::max` returns the non-NaN operand.
fn range_per_metre(speed_m_per_min: f64, slower_cruise_m_per_min: f64) -> f64 {
    (slower_cruise_m_per_min / speed_m_per_min).max(1.0)
}

/// Normalise the diver's segment list to exactly one entry per route span, with a concrete speed.
///
/// A plan has `waypoint_count + 1` spans (origin → W0 → … → open-field tail). The stored list can
/// disagree - a hand-edited `.plnx`, or a stale plan loaded after way points were added or removed
/// in Survey - so this is the single place that reconciles it.
///
/// `default_speed` is the plan's own speed (`survey_speed` falling back to `swim_speed`). A span
/// takes it whenever the diver has not set one: a missing entry, an explicit `None`, or a stored
/// number outside `[MIN_SEGMENT_SPEED_M_PER_MIN, MAX_SEGMENT_SPEED_M_PER_MIN]` (which subsumes
/// non-finite, zero and negative). The last case falls back rather than being rejected because it
/// would otherwise divide into the edge cost and poison the field with `+Inf`/`NaN`.
///
/// ⚠ An all-`None` list must therefore reproduce the pre-#317 field - arithmetically equal, within
/// float re-association (the engine now scales a shared edge basis at relax time rather than folding
/// the speed into the edge weight, so the multiplications associate differently). That is not a nice
/// property but the contract: the frontend sends a full-length list from the moment Plan mode opens,
/// so if `None` did not inherit, the Swim-speed field and Settings > Gas would silently stop
/// affecting anything (`the_plan_speed_still_reaches_the_engine_through_a_full_length_segment_list`
/// pins it).
///
/// `dpv` gates `on_scooter` - and is also what the span's range cost is measured against. Without a
/// rig there is no range to spend and no drop to derive, so a leftover flag would report a scootered
/// distance the plan cannot be held to. The segment's *speed* still applies - variable pace is
/// useful with or without a scooter.
pub(super) fn resolve_segments(
    segments: &[PlanSegment],
    waypoint_count: usize,
    default_speed: f64,
    dpv: Option<&DpvRig>,
) -> Vec<ResolvedSegment> {
    let cruise = dpv.map_or(f64::INFINITY, slower_cruise_m_per_min);
    (0..=waypoint_count)
        .map(|k| {
            let seg = segments.get(k);
            let speed_m_per_min = seg
                .and_then(|s| s.speed_m_per_min)
                .filter(|s| (MIN_SEGMENT_SPEED_M_PER_MIN..=MAX_SEGMENT_SPEED_M_PER_MIN).contains(s))
                .unwrap_or(default_speed);
            let on_scooter = dpv.is_some() && seg.is_some_and(|s| s.on_scooter);
            ResolvedSegment {
                speed_m_per_min,
                on_scooter,
                // `cruise` is only ever infinite when there is no rig, which is exactly when
                // `on_scooter` is false - so the rider can never be infinite.
                range_per_metre: if on_scooter {
                    range_per_metre(speed_m_per_min, cruise)
                } else {
                    0.0
                },
            }
        })
        .collect()
}

/// Which way point the scooters are parked at, if any.
///
/// The drop is *derived*, never placed: it is the boundary after the last scootered segment. Two
/// cases yield `None` - nothing is flown on a scooter, and the units are carried all the way to the
/// turnaround (the last segment is scootered, so its boundary is the turn, not a way point).
///
/// The returned index is both the segment index and the way-point index, since segment `k` ends at
/// way point `k` for every `k` but the tail.
pub(super) fn scooter_drop_index(segments: &[ResolvedSegment]) -> Option<usize> {
    let last = segments.iter().rposition(|s| s.on_scooter)?;
    (last + 1 < segments.len()).then_some(last)
}

/// The scootered-penetration cap in metres, or `f64::INFINITY` with no rig (nothing to cap, and
/// `resolve_segments` has already forced every span off the scooter in that case).
pub(super) fn scooter_range_limit(dpv: Option<&DpvRig>) -> f64 {
    dpv.map_or(f64::INFINITY, DpvRig::usable_range_m)
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "exact arithmetic on hand-picked values; a drift of one ULP is a real regression here"
)]
mod tests {
    use super::*;
    use crate::config::DpvUnit;

    fn unit(speed: f64, burn: f64) -> DpvUnit {
        DpvUnit {
            speed_m_per_min: speed,
            burn_minutes: burn,
        }
    }

    fn seg(speed: f64, on_scooter: bool) -> PlanSegment {
        PlanSegment {
            speed_m_per_min: Some(speed),
            on_scooter,
        }
    }

    fn inherit(on_scooter: bool) -> PlanSegment {
        PlanSegment {
            speed_m_per_min: None,
            on_scooter,
        }
    }

    /// A rig whose two units share one cruise speed, so `slower_cruise_m_per_min` is unambiguous.
    fn rig(cruise: f64) -> DpvRig {
        DpvRig {
            primary: unit(cruise, 60.0),
            backup: unit(cruise, 60.0),
            range_reserve_fraction: 0.0,
        }
    }

    /// A resolved span ridden AT CRUISE (or swum), where a metre of line costs exactly a metre of
    /// range - the only case with a range factor of 1.0.
    fn resolved(speed: f64, on_scooter: bool) -> ResolvedSegment {
        ResolvedSegment {
            speed_m_per_min: speed,
            on_scooter,
            range_per_metre: if on_scooter { 1.0 } else { 0.0 },
        }
    }

    #[test]
    fn empty_segments_expand_to_the_default_speed_and_no_scooter() {
        let out = resolve_segments(&[], 2, 10.0, Some(&rig(55.0)));
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|s| s.speed_m_per_min == 10.0));
        assert!(out.iter().all(|s| !s.on_scooter));
    }

    #[test]
    fn a_full_length_list_of_unset_speeds_is_the_same_as_no_list_at_all() {
        // THE regression this file exists to prevent. The frontend always sends one entry per span,
        // so if these two disagreed the diver's configured speed would be unreachable in practice
        // while every test that passes `&[]` stayed green.
        let unset = resolve_segments(
            &[inherit(false), inherit(false), inherit(false)],
            2,
            17.0,
            Some(&rig(55.0)),
        );
        assert_eq!(unset, resolve_segments(&[], 2, 17.0, Some(&rig(55.0))));
        assert!(unset.iter().all(|s| s.speed_m_per_min == 17.0));
    }

    #[test]
    fn a_short_list_is_padded_and_a_long_one_truncated() {
        let padded = resolve_segments(&[seg(55.0, true)], 2, 10.0, Some(&rig(55.0)));
        assert_eq!(padded.len(), 3);
        assert_eq!(padded[0].speed_m_per_min, 55.0);
        assert!(padded[0].on_scooter);
        assert_eq!(padded[1].speed_m_per_min, 10.0);

        let truncated = resolve_segments(
            &[seg(55.0, true), seg(20.0, false), seg(30.0, true)],
            0,
            10.0,
            Some(&rig(55.0)),
        );
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].speed_m_per_min, 55.0);
    }

    #[test]
    fn a_bad_speed_falls_back_to_the_default() {
        // `1e-320` is the one that needs the bounds rather than a sign/finiteness check: it is a
        // positive, finite subnormal, so it passes `is_finite() && > 0.0` and then overflows the
        // engine's two reciprocals to INFINITY. `1e6` is its mirror at the top.
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY, 1e-320, 1e6] {
            let out = resolve_segments(&[seg(bad, false)], 0, 12.0, Some(&rig(55.0)));
            assert_eq!(out[0].speed_m_per_min, 12.0, "speed {bad} should fall back");
        }
        // The bounds are inclusive, and a plausible slow survey pace is well inside them.
        for good in [
            MIN_SEGMENT_SPEED_M_PER_MIN,
            3.0,
            MAX_SEGMENT_SPEED_M_PER_MIN,
        ] {
            let out = resolve_segments(&[seg(good, false)], 0, 12.0, Some(&rig(55.0)));
            assert_eq!(out[0].speed_m_per_min, good, "speed {good} should be kept");
        }
    }

    #[test]
    fn an_edited_span_keeps_its_speed_while_its_neighbours_inherit() {
        let out = resolve_segments(
            &[inherit(false), seg(55.0, false), inherit(false)],
            2,
            12.0,
            Some(&rig(55.0)),
        );
        assert_eq!(
            out,
            vec![
                resolved(12.0, false),
                resolved(55.0, false),
                resolved(12.0, false)
            ]
        );
    }

    #[test]
    fn without_a_rig_the_scooter_flag_is_dropped_but_the_speed_is_kept() {
        let out = resolve_segments(&[seg(55.0, true)], 0, 10.0, None);
        assert_eq!(out[0].speed_m_per_min, 55.0);
        assert!(!out[0].on_scooter);
        // And nothing is charged against a range that does not exist - never the INFINITY that a
        // rig-less "slower cruise" would otherwise divide into the rider.
        assert_eq!(out[0].range_per_metre, 0.0);
    }

    #[test]
    fn riding_below_cruise_spends_range_faster_than_it_covers_line() {
        // ⚠ THE OVER-EXTENDING BUG. The cap is a DISTANCE (`burn x cruise`), but what runs out is
        // battery MINUTES, and the two only convert 1:1 at cruise. A span ridden at a third of
        // cruise takes three times the minutes the cap assumed for those metres, so a flat metre per
        // metre lets the diver reach ground the battery cannot deliver - the unsafe direction.
        let cruise = rig(45.0);
        let out = resolve_segments(
            &[seg(15.0, true), seg(45.0, true), seg(90.0, true)],
            2,
            10.0,
            Some(&cruise),
        );
        // A third of cruise: three metres of allowance per metre of line.
        assert_eq!(out[0].range_per_metre, 3.0);
        // At cruise, exactly the old 1:1 - which is what keeps every at-cruise plan, the cap and the
        // Settings preview untouched.
        assert_eq!(out[1].range_per_metre, 1.0);
        // Above cruise the factor FLOORS rather than shrinking: going faster must not stretch the
        // battery either, since the unit's own burn figure is quoted at cruise.
        assert_eq!(out[2].range_per_metre, 1.0);
    }

    #[test]
    fn the_range_factor_is_measured_against_the_slower_unit() {
        // `usable_range_m` picks the weaker unit by LINE COVERED, which is not always the slower
        // one: here the 45 m/min primary covers 2700 m and the 20 m/min backup only 1200, but if the
        // burns were reversed the primary would be weaker while the backup stayed slower. The factor
        // takes the slower cruise either way, because that is the larger factor and so the
        // conservative answer.
        let mixed = DpvRig {
            primary: unit(45.0, 60.0),
            backup: unit(20.0, 60.0),
            range_reserve_fraction: 0.0,
        };
        let out = resolve_segments(&[seg(10.0, true)], 0, 10.0, Some(&mixed));
        assert_eq!(out[0].range_per_metre, 2.0, "20 / 10, not 45 / 10");
    }

    #[test]
    fn a_swum_span_spends_no_range_however_slowly_it_is_swum() {
        // The factor exists to price BATTERY time. A swum span spends none at any pace, so it must
        // stay at 0.0 rather than picking up the below-cruise multiplier of a scooter it is not on.
        let out = resolve_segments(&[seg(1.0, false)], 0, 10.0, Some(&rig(45.0)));
        assert_eq!(out[0].range_per_metre, 0.0);
    }

    #[test]
    fn the_drop_is_the_boundary_after_the_last_scootered_segment() {
        // scooter, scooter, swim -> parked at way point 1 (the end of segment 1).
        assert_eq!(
            scooter_drop_index(&[
                resolved(55.0, true),
                resolved(55.0, true),
                resolved(10.0, false)
            ]),
            Some(1)
        );
        // A tow: swimming in the middle, back on the scooter after, so the units travel on.
        assert_eq!(
            scooter_drop_index(&[
                resolved(55.0, true),
                resolved(10.0, false),
                resolved(55.0, true)
            ]),
            None
        );
        // Carried to the turnaround - the last span is scootered, so its boundary is the turn.
        assert_eq!(
            scooter_drop_index(&[resolved(55.0, true), resolved(55.0, true)]),
            None
        );
        // Never on a scooter at all.
        assert_eq!(scooter_drop_index(&[resolved(10.0, false)]), None);
    }

    #[test]
    fn the_cap_is_the_weaker_unit_by_range_not_by_burn() {
        // The backup burns for LESS time but the primary is the weaker unit by line covered:
        // 20 x 100 = 2000 m against 60 x 50 = 3000 m.
        let rig = DpvRig {
            primary: unit(20.0, 100.0),
            backup: unit(50.0, 60.0),
            range_reserve_fraction: 0.0,
        };
        assert_eq!(rig.usable_range_m(), 2000.0);
    }

    #[test]
    fn the_cap_is_not_halved_and_the_reserve_scales_it() {
        let rig = DpvRig {
            primary: unit(55.0, 90.0),
            backup: unit(50.0, 60.0),
            range_reserve_fraction: 0.2,
        };
        // min(4950, 3000) = 3000, less 20% = 2400. Not 1500: the primary flies in and the backup
        // flies out, so each is asked for one direction only.
        assert_eq!(rig.usable_range_m(), 2400.0);
    }

    #[test]
    fn no_rig_means_no_cap() {
        assert_eq!(scooter_range_limit(None), f64::INFINITY);
    }

    #[test]
    fn usable_range_matches_the_settings_preview_for_the_shared_case() {
        // ⚠ CROSS-LANGUAGE PIN. Settings > Gas shows the diver a usable-range preview before any
        // plan exists, so it cannot read `scooter_range_limit_m` off a computed field and has to
        // duplicate this formula in TypeScript (`dpvUsableRangeM`, GasSettings.tsx). A duplicated
        // formula drifts silently - the #233 gauge-divergence class - so both sides pin THIS SAME
        // case, and the twin is named in each so a change to either breaks a test that says where
        // its counterpart lives.
        //
        // The pair is chosen so several wrong rules are detectable: taking the primary
        // unconditionally gives 750, dropping the reserve gives 800, and halving gives 400.
        //
        // It does NOT discriminate burn from range - the backup is the weaker unit on both counts
        // here (20 min vs 100, and 800 m vs 1000), so either rule lands on 600. That distinction is
        // `the_cap_is_the_weaker_unit_by_range_not_by_burn`'s job, and this pair is chosen instead
        // to be the exact case the TypeScript preview pins.
        //
        // Twin: `the computed usable-range line matches the Rust formula for a hand-picked pair`
        // in `src/components/Dialogs/settings/GasSettings.test.tsx`.
        let rig = DpvRig {
            primary: unit(10.0, 100.0),
            backup: unit(40.0, 20.0),
            range_reserve_fraction: 0.25,
        };
        assert_eq!(rig.usable_range_m(), 600.0);
    }
}
