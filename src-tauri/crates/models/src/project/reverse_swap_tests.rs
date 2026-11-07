//! [#576 RV8b-E3-1] Reversing a shot reciprocates its readings AND swaps which end each belongs to.
//!
//! Every reverse test helper builds a shot with `azimuth_in == azimuth_out`, where a transform that
//! only reciprocates (no swap) and one that only swaps (no reciprocal) cannot be told from the right
//! one in the azimuth half; the circular mean the geometry tests read is order-independent too. A
//! shot with DISTINCT readings at each end - and distinct pitches and depths - pins the whole rule:
//! the reversed shot's IN reading is the reciprocal of the old OUT one and vice versa, its pitches
//! are the other end's negated (pitch is a signed slope, +down), and its depths change ends.

use super::reverse::reverse_shots;
use crate::ShotData;

fn near(what: &str, got: f64, want: f64) {
    assert!((got - want).abs() < 1e-9, "{what}: {got}, want {want}");
}

#[test]
fn reverse_reciprocates_and_swaps_distinct_in_and_out_readings() {
    let mut shots = vec![ShotData {
        azimuth_in: 10.0,
        azimuth_out: 20.0,
        pitch_in: Some(5.0),
        pitch_out: Some(7.0),
        depth_in: 3.0,
        depth_out: 9.0,
        length: 12.0,
        ..Default::default()
    }];
    reverse_shots(&mut shots, false);
    assert_eq!(shots.len(), 1);
    let shot = &shots[0];
    // Reciprocate-only would read 190 / 200; swap-only 20 / 10.
    near("azimuth in", shot.azimuth_in, 200.0);
    near("azimuth out", shot.azimuth_out, 190.0);
    // Negate-only would read -5 / -7; swap-only 7 / 5.
    assert_eq!((shot.pitch_in, shot.pitch_out), (Some(-7.0), Some(-5.0)));
    near("depth in", shot.depth_in, 9.0);
    near("depth out", shot.depth_out, 3.0);
    near("length", shot.length, 12.0);

    // An involution: reversing again restores every reading.
    reverse_shots(&mut shots, false);
    let back = &shots[0];
    near("azimuth in, twice", back.azimuth_in, 10.0);
    near("azimuth out, twice", back.azimuth_out, 20.0);
    assert_eq!((back.pitch_in, back.pitch_out), (Some(5.0), Some(7.0)));
}
