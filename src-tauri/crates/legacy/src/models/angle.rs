//! The canonical wrap for a bearing this crate COMPUTES (issue #24).
//!
//! Scope: every azimuth the backend *derives* - a circular mean, a reciprocal,
//! an `atan2` of a solved displacement, a parsed reading being canonicalised.
//! It is deliberately NOT "the single wrap for every stored bearing": the shot
//! table stores raw user input on purpose (`ShotTableEditor.tsx` passes
//! `{ normalize: false }`), and `src/utils/validation.ts` plus `ImportDialog.tsx`
//! keep their own mirrors. Those are separate contracts, not clients of this one.

/// Wrap an azimuth in degrees into `[0, 360)`.
///
/// `f64::rem_euclid` alone is NOT enough, and the gap is reachable with ordinary
/// data rather than a crafted ulp. `rem_euclid` computes `r = self - (self /
/// rhs).trunc() * rhs` and then adds `rhs` when `r` is negative, so for a tiny
/// negative input the addend ROUNDS and the result is exactly `360.0`:
/// `(-1e-14f64).rem_euclid(360.0) == 360.0`. The ordinary route in is
/// [`average_azimuth_of`](crate::models::average_azimuth_of): every fore/back
/// pair whose circular mean is exactly north lands there, and there are 360 such
/// pairs on a plain 0.1-degree grid - `(0.1, 359.9)`, `(0.3, 359.7)`, and so on.
/// `atan2` writers reach it too (`(-1e-16f64).atan2(1.0)` in degrees wraps to
/// `360.0`).
///
/// `360.0` is not a harmless representation of north. It is `>= 360.0`, so it
/// trips `INVALID_AZIMUTH`, which `warning_is_ignorable` refuses to suppress
/// because it is [`WarningSeverity::Error`](crate::models::WarningSeverity) -
/// the user cannot dismiss it, only normalise it away by hand.
///
/// A non-finite reading has no canonical form and passes through untouched
/// (`rem_euclid` would turn an infinity into NaN); `ShotData::all_finite` and
/// the integrity banner are what surface those.
///
/// The `+ 0.0` maps `-0.0` to `+0.0`, so the output never serialises as `-0`.
#[inline]
#[must_use]
pub fn normalize_azimuth(degrees: f64) -> f64 {
    if !degrees.is_finite() {
        return degrees;
    }
    let wrapped = degrees.rem_euclid(360.0);
    if wrapped >= 360.0 {
        0.0
    } else {
        wrapped + 0.0
    }
}
