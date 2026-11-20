//! Reciprocating a run of shots so it reads the other way without moving the
//! cave (issue #24): the per-shot rule of Reverse Line, of an import's
//! direction toggle and of Combine Lines' reversed other line.
//!
//! A shot's bearing is the circular mean of its two *forward* readings
//! ([`Shot::average_azimuth`](crate::Shot::average_azimuth)) -
//! `azimuth_in`/`azimuth_out` are not a foresight/backsight pair - so merely
//! swapping them is a no-op and would leave every bearing pointing the
//! original way. Each shot is therefore *reciprocated*:
//!
//! | field                    | operation                        |
//! |--------------------------|----------------------------------|
//! | shot order               | reversed; ids stay with positions |
//! | `azimuth_in`             | `(old azimuth_out + 180) % 360`  |
//! | `azimuth_out`            | `(old azimuth_in + 180) % 360`   |
//! | `depth_in` / `depth_out` | swapped                          |
//! | `pitch_in` / `pitch_out` | swapped **and negated**          |
//! | `left` / `right`         | swapped only when `swap_lr`      |
//! | everything else          | travels with its record          |
//!
//! One degenerate case needs care: when the two readings are exactly 180
//! degrees apart the circular mean is undefined, and reciprocate-and-swap maps
//! the pair to itself - the shot would keep its old bearing while the rest of
//! the line turned around, moving the cave. Such a pair is swapped instead,
//! which preserves both recorded values and flips the arbitrary tie-break that
//! [`average_azimuth`](crate::Shot::average_azimuth) resolves it with.

use crate::{normalize_azimuth, AzimuthReference, ShotData};

/// Reciprocal of a forward azimuth, normalised to `[0, 360)`.
pub(crate) fn reciprocal(azimuth: f64) -> f64 {
    normalize_azimuth(azimuth + 180.0)
}

/// True when reciprocating and swapping a shot's two readings would give the
/// same pair back, i.e. they are exactly 180 degrees apart.
///
/// `reciprocal` is an involution, so for such a pair `reciprocal(azimuth_out) ==
/// azimuth_in` and vice versa - the transform below would be a no-op and the
/// shot would keep pointing the original way while the rest of the line
/// turned around.
///
/// The comparison is deliberately exact. A pair that misses antipodal by even
/// one ulp has a well-defined circular mean and reciprocates correctly on its
/// own; only the precise degenerate case needs the fix-up, and treating a
/// near-antipodal pair as degenerate would give it the *wrong* bearing (the
/// swap-only branch preserves the mean instead of turning it around). Compared
/// through `to_bits` so this reads as "bit-exact by intent" rather than a
/// sloppy float `==`.
pub(crate) fn readings_are_antipodal(azimuth_in: f64, azimuth_out: f64) -> bool {
    normalize_azimuth(azimuth_out - azimuth_in).to_bits() == 180.0_f64.to_bits()
}

/// Total local displacement of a run of shots: the local coordinate of the last
/// station, since station 0 sits at the local origin by definition.
///
/// This is what a reverse has to add to `line_offsets` to keep the cave
/// still - station 0 becomes the far end, so the anchor moves by the span the
/// line used to cover. Shared by the in-app reverse and the import-time
/// toggle so the two placements can never disagree.
///
/// `reference` is the line's north reference (#438): the displacement has to
/// be measured in the same rotated frame the coordinate walk uses, or a reverse
/// of a line carrying a declination compensates by a vector that is off by
/// exactly that rotation - and the line slides.
pub fn shot_run_displacement(shots: &[ShotData], reference: AzimuthReference) -> (f64, f64) {
    let (mut dx, mut dy) = (0.0_f64, 0.0_f64);
    for shot in shots {
        let azimuth_rad = shot.average_azimuth_true_rad(reference);
        let horizontal = shot.horizontal_length();
        dx += horizontal * azimuth_rad.sin();
        dy += horizontal * azimuth_rad.cos();
    }
    (dx, dy)
}

/// The `(azimuth_in, azimuth_out)` a reverse writes for one shot read
/// `(azimuth_in, azimuth_out)` - the bearing half of [`reverse_shots`], shared
/// with the compact undo history, which replays a reverse's shot transform to
/// avoid recording every reversed reading.
///
/// Both readings are normalised first (see [`reverse_shots`] for why that is
/// load-bearing). A degenerate pair - the two readings disagree by a full 180
/// degrees, so the circular mean is undefined and reciprocate-and-swap is the
/// identity - is swapped instead: both values stay exactly as recorded while
/// the reading `average_azimuth` measures its ±180 tie-break from flips, which
/// is what turns the bearing around. An involution either way.
pub fn reversed_azimuths(azimuth_in: f64, azimuth_out: f64) -> (f64, f64) {
    let (azimuth_in, azimuth_out) = (
        normalize_azimuth(azimuth_in),
        normalize_azimuth(azimuth_out),
    );
    if readings_are_antipodal(azimuth_in, azimuth_out) {
        (azimuth_out, azimuth_in)
    } else {
        (reciprocal(azimuth_out), reciprocal(azimuth_in))
    }
}

/// Re-associate one shot's readings with the opposite direction of travel.
///
/// Reverses the whole vec of shots as well, so the caller gets a line that
/// is fully reversed.  This is THE reverse transform: both the in-app action
/// ([`Project::reverse_line`]) and the import-time toggle route through it,
/// so the two can never drift apart.
///
/// Order-independent per shot, so it is safe to apply before or after the vec
/// is reversed.  Applying it twice is the identity in exact arithmetic *for
/// readings already in `[0, 360)`*; an out-of-range reading is normalised on the
/// first application and comes back normalised (`-715` returns as `5`).  Even
/// for in-range readings f64 is not bit-exact: the `(az + 180) % 360` round trip
/// can land a couple of ulps off when the shifted value is not representable,
/// which is why undo snapshots instead of re-running the transform.
pub fn reverse_shots(shots: &mut [ShotData], swap_lr: bool) {
    shots.reverse();
    for shot in shots.iter_mut() {
        // Normalise first so the transform and `average_azimuth_of` work from
        // the same canonical readings. All three wrap through `normalize_azimuth`
        // now, so they agree on any input's mod-360 class - but the degenerate
        // branch of `reversed_azimuths` swaps its operands VERBATIM, so whatever this line leaves
        // on the record is what a degenerate pair keeps, for every later
        // consumer; and the exact `== 180` antipodal test only holds once the
        // magnitudes are small enough that `azimuth_out - azimuth_in` does not
        // round. That verbatim swap is exactly why a bare `rem_euclid` was not
        // enough here: it can return `360.0` (see `normalize_azimuth`), so the
        // pre-normalisation could itself hand the swap an OUT-OF-RANGE value and
        // violate the invariant it exists to establish - `(-1e-14, 180.0)` came
        // back as `(180.0, 360.0)`, a non-ignorable `INVALID_AZIMUTH` minted by
        // the very step meant to prevent one.
        //
        // The pre-normalisation was load-bearing for the geometry too while
        // `average_azimuth_of` still wrapped with Rust's truncated `%`: the two
        // disagreed for a pair whose difference falls outside `(-540, 540]`, the
        // degenerate-pair guard missed, and reciprocate-and-swap left the bearing
        // UNCHANGED while the rest of the line turned around. Out-of-range
        // readings are reachable: `validation.rs` flags an azimuth outside
        // `[0, 360)` as `INVALID_AZIMUTH`, which is `WarningSeverity::Error` and
        // so cannot be ignored - but it is a report, not a gate, and nothing
        // rejects the value. The DMP parser (models::dmp) also stored
        // `heading_in`/`heading_out` verbatim in older builds, where the Compass
        // and TML importers normalised theirs.
        (shot.azimuth_in, shot.azimuth_out) = reversed_azimuths(shot.azimuth_in, shot.azimuth_out);

        std::mem::swap(&mut shot.depth_in, &mut shot.depth_out);

        // Pitch is signed slope, so reversing the traverse flips it as well as
        // re-associating it with the other end.
        let pitch_in = shot.pitch_in.map(|p| -p);
        shot.pitch_in = shot.pitch_out.map(|p| -p);
        shot.pitch_out = pitch_in;

        if swap_lr {
            std::mem::swap(&mut shot.left, &mut shot.right);
        }
    }
}
