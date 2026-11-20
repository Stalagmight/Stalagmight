//! Plan segments and the DPV range cap (#317) - the pure, graph-free half of scooter support.
//!
//! The engine proper ([`super`]) owns the graph walk; everything here is arithmetic over the
//! plan's own inputs, which is what keeps it testable without building a project.

use crate::models::config::DpvRig;

use super::model::PlanSegment;

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

/// Slowest per-span speed (metres per minute) the engine will fly; below it the span inherits the
/// plan speed instead.
///
/// A floor is needed, not just `> 0.0`. A stored speed can be any positive `f64` a hand-edited
/// `.plnx` or a bad round-trip contains, and the engine divides by it twice - once for the gas
/// scalar `rmv / speed`, once for the runtime rider `1 / speed`. A denormal (1e-320) overflows both
/// to `INFINITY`, which is not merely "very expensive": a real edge is then priced at `INFINITY`, so
/// `nd < dist[v]` fails and it never relaxes, and a zero-weight merge / loop-closure tie is priced
/// at `0.0 * INFINITY = NaN`, which fails the same comparison. Nothing propagates and the whole
/// field collapses to the origin. 0.1 m/min is two orders of magnitude under any real survey pace,
/// so nothing legitimate is excluded.
pub const MIN_SEGMENT_SPEED_M_PER_MIN: f64 = 0.1;

/// Fastest per-span speed the engine will fly; above it the span inherits the plan speed. The
/// ceiling is the floor's mirror - it keeps the reciprocal off the denormal end - and 1000 m/min is
/// roughly ten times the fastest DPV, so it cannot exclude a real plan either.
pub const MAX_SEGMENT_SPEED_M_PER_MIN: f64 = 1000.0;

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
