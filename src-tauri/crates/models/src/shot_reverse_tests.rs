//! The bearing half of the reverse (`shot_reverse`): every reverse turns the
//! shot's bearing as [`average_azimuth_of`] computes it, and a second reverse
//! gives the readings back.
//!
//! The farm's metamorphic fuzzer found the case these pin: two readings
//! typed exactly opposite (`158.1` / `338.1`) whose f64 difference is one ulp
//! off 180. The bit-exact antipodal test said "not degenerate" while the
//! bearing computation resolved the pair by its tie-break, so the reverse
//! left that shot's bearing where it was, the stations past it moved by twice
//! its horizontal length, and a second reverse turned it the other way
//! (module docs of `shot_reverse`). Everything here is synthetic.

use crate::shot_reverse::readings_are_antipodal;
use crate::{average_azimuth_of, normalize_azimuth, reversed_azimuths};

/// How far apart two bearings are, in degrees in `[0, 180]`.
fn apart(bearing: f64, other: f64) -> f64 {
    let d = normalize_azimuth(bearing - other);
    if d > 180.0 {
        360.0 - d
    } else {
        d
    }
}

/// One reverse turns the bearing (to within `1e-9` degrees of opposite), and
/// a second gives both readings back within `1e-9` degrees. `label` names the
/// pair in a failure.
fn assert_reverse_turns_and_returns(azimuth_in: f64, azimuth_out: f64, label: &str) {
    let bearing = average_azimuth_of(azimuth_in, azimuth_out);
    let once = reversed_azimuths(azimuth_in, azimuth_out);
    let turned = average_azimuth_of(once.0, once.1);
    assert!(
        (apart(turned, bearing) - 180.0).abs() < 1e-9,
        "{label}: ({azimuth_in}, {azimuth_out}) at {bearing} reversed to {once:?} at {turned}, \
         not turned"
    );
    let twice = reversed_azimuths(once.0, once.1);
    assert!(
        apart(twice.0, normalize_azimuth(azimuth_in)) < 1e-9
            && apart(twice.1, normalize_azimuth(azimuth_out)) < 1e-9,
        "{label}: ({azimuth_in}, {azimuth_out}) reversed twice to {twice:?}"
    );
}

/// The precondition, pinned: the difference of the two readings is not
/// exactly 180 in f64, while `average_azimuth_of` resolves the pair by its
/// tie-break (the first reading minus 90). The bit-exact test the reverse
/// used before missed it.
#[test]
fn a_typed_opposite_pair_is_a_tie_the_difference_misses() {
    for (azimuth_in, azimuth_out) in [(158.1, 338.1), (267.1, 87.1)] {
        let difference = normalize_azimuth(azimuth_out - azimuth_in);
        assert_ne!(
            difference.to_bits(),
            180.0_f64.to_bits(),
            "({azimuth_in}, {azimuth_out}): the difference is one ulp off 180"
        );
        let tie_break = normalize_azimuth(azimuth_in - 90.0);
        assert!(
            apart(average_azimuth_of(azimuth_in, azimuth_out), tie_break) < 1e-9,
            "({azimuth_in}, {azimuth_out}): the bearing is the tie-break"
        );
        assert!(
            readings_are_antipodal(azimuth_in, azimuth_out),
            "({azimuth_in}, {azimuth_out}) is degenerate for the reverse"
        );
    }
}

/// The farm's shot: reversed, its readings are swapped exactly (both values
/// as typed) and its bearing turns; reversed again, it is the shot it was, to
/// the bit.
#[test]
fn a_typed_opposite_pair_is_swapped_and_comes_back_exactly() {
    for (azimuth_in, azimuth_out) in [(158.1, 338.1), (267.1, 87.1), (10.0, 190.0)] {
        let once = reversed_azimuths(azimuth_in, azimuth_out);
        assert_eq!(
            (once.0.to_bits(), once.1.to_bits()),
            (azimuth_out.to_bits(), azimuth_in.to_bits()),
            "({azimuth_in}, {azimuth_out}) reversed to {once:?}"
        );
        let twice = reversed_azimuths(once.0, once.1);
        assert_eq!(
            (twice.0.to_bits(), twice.1.to_bits()),
            (azimuth_in.to_bits(), azimuth_out.to_bits()),
            "({azimuth_in}, {azimuth_out}) reversed twice to {twice:?}"
        );
        assert_reverse_turns_and_returns(azimuth_in, azimuth_out, "farm shot");
    }
}

/// Every pair a surveyor can type on a 0.1- or 0.01-degree grid that is
/// exactly opposite, or one grid step either side of it, in both orders:
/// the reverse turns it and a second reverse gives it back. On the 0.1 grid
/// 832 of the 3600 opposite pairs did not turn before the fix.
#[test]
fn every_decimal_pair_near_opposite_turns_and_comes_back() {
    for scale in [10_u32, 100] {
        let steps = 360 * scale;
        for i in 0..steps {
            for offset in [steps / 2 - 1, steps / 2, steps / 2 + 1] {
                let azimuth_in = f64::from(i) / f64::from(scale);
                let azimuth_out = f64::from((i + offset) % steps) / f64::from(scale);
                assert_reverse_turns_and_returns(azimuth_in, azimuth_out, "grid");
                assert_reverse_turns_and_returns(azimuth_out, azimuth_in, "grid, swapped");
            }
        }
    }
}

/// Readings a few ulps from exactly opposite (no typed value lands there,
/// but a computed one could): the reverse still turns every bearing, and
/// reversing twice gives the bearing back. The readings themselves may come
/// back exchanged (module docs), so only the bearing is asserted here.
#[test]
fn a_pair_within_ulps_of_opposite_always_turns() {
    // A fixed linear congruential stream: the cases are the same every run.
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next_unit = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1_u64 << 53) as f64
    };
    for _ in 0..4000 {
        let azimuth_in = next_unit() * 360.0;
        let opposite = normalize_azimuth(azimuth_in + 180.0);
        for ulps in -6_i64..=6 {
            let bits = opposite.to_bits() as i64 + ulps;
            let azimuth_out = normalize_azimuth(f64::from_bits(bits as u64));
            for (a, b) in [(azimuth_in, azimuth_out), (azimuth_out, azimuth_in)] {
                let bearing = average_azimuth_of(a, b);
                let once = reversed_azimuths(a, b);
                let turned = average_azimuth_of(once.0, once.1);
                assert!(
                    (apart(turned, bearing) - 180.0).abs() < 1e-9,
                    "({a}, {b}) at {bearing} reversed to {once:?} at {turned}, not turned"
                );
                let twice = reversed_azimuths(once.0, once.1);
                assert!(
                    apart(average_azimuth_of(twice.0, twice.1), bearing) < 1e-9,
                    "({a}, {b}) at {bearing} reversed twice to {twice:?}"
                );
            }
        }
    }
}

/// A non-finite reading has no bearing to turn; it reciprocates and swaps as
/// before the fix (the integrity banner is what reports it).
#[test]
fn a_non_finite_reading_reciprocates_as_before() {
    let once = reversed_azimuths(f64::NAN, 10.0);
    assert_eq!(once.0.to_bits(), 190.0_f64.to_bits());
    assert!(once.1.is_nan());
    let once = reversed_azimuths(10.0, f64::INFINITY);
    assert_eq!(once.0.to_bits(), f64::INFINITY.to_bits());
    assert_eq!(once.1.to_bits(), 190.0_f64.to_bits());
    assert!(!readings_are_antipodal(f64::NAN, 10.0));
}
