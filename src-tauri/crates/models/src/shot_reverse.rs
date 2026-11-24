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
//! One degenerate case needs care: when the two readings are 180 degrees
//! apart the circular mean is undefined, and reciprocate-and-swap maps the
//! pair to itself - the shot would keep its old bearing while the rest of the
//! line turned around, moving the cave. Such a pair is swapped instead, which
//! preserves both recorded values and flips the arbitrary tie-break that
//! [`average_azimuth`](crate::Shot::average_azimuth) resolves it with.
//!
//! "180 degrees apart" is decided by the bearing itself, not by subtracting
//! the readings: a pair is degenerate exactly when
//! [`average_azimuth_of`](crate::average_azimuth_of) resolves it by that
//! tie-break, which is when swapping its two readings turns its bearing
//! around. Before this fix the test was a bit-exact
//! `(out - in) % 360 == 180`, and the two disagree on readings a surveyor
//! types: `338.1 - 158.1` is `180.00000000000003` in f64, so the test said
//! "not degenerate", while `average_azimuth_of`'s `+ 540` wrap rounds the
//! same difference to exactly 180 and resolves the pair by its tie-break.
//! Reciprocate-and-swap then gave `(158.10000000000002, 338.1)`, the same
//! tie with the same bearing, so that shot did not turn and every station
//! past it moved by twice the shot's horizontal length. That is about a
//! fifth of the pairs exactly opposite on a decimal grid: 832 of the 3600 on
//! a 0.1-degree grid. A second reverse then turned the shot the other way,
//! so reversing twice left the line moved as well. 0.6.0 has the same
//! bit-exact test; this is a declared fix (plan, Deviations T4-R1).
//!
//! The second guard is for f64 alone: reciprocating rounds, and on a pair a
//! few ulps short of opposite it can land the reciprocated pair on that tie
//! with the wrong bearing. The reverse checks the bearing it wrote and, when
//! it did not turn, swaps the reciprocated readings, which flips the tie.
//! Every reverse therefore turns the shot's bearing. Twice gives the
//! readings back (to the ulp of the 180-degree turn) for every pair of
//! readings on a decimal grid down to 0.001 degree; only a crafted pair
//! within a few ulps of opposite can come back with its two readings
//! exchanged, the bearing the original one.

use crate::{average_azimuth_of, normalize_azimuth, AzimuthReference, ShotData};

/// Reciprocal of a forward azimuth, normalised to `[0, 360)`.
pub(crate) fn reciprocal(azimuth: f64) -> f64 {
    normalize_azimuth(azimuth + 180.0)
}

/// How far apart two bearings are, in degrees in `[0, 180]`; NaN when either
/// is not finite, so every comparison with it is false.
fn degrees_apart(bearing: f64, other: f64) -> f64 {
    let apart = normalize_azimuth(bearing - other);
    if apart > 180.0 {
        360.0 - apart
    } else {
        apart
    }
}

/// True when the shot's bearing is [`average_azimuth_of`]'s tie-break: the
/// two readings are 180 degrees apart as the bearing computation sees them,
/// so reciprocate-and-swap would leave the bearing where it was while the
/// rest of the line turned around.
///
/// Decided by the bearing, not by the difference of the readings (module
/// docs): a pair is degenerate exactly when swapping its readings turns its
/// bearing. Away from the tie the circular mean is symmetric in its two
/// readings, so swapping them changes the bearing by rounding only; at the
/// tie it resolves to the first reading minus 90 in either order, so swapping
/// turns it by 180 degrees. The test is symmetric in its arguments, so a degenerate pair is
/// still degenerate swapped, which is what makes the swap branch of
/// [`reversed_azimuths`] an exact involution. Non-finite readings are never
/// degenerate.
pub(crate) fn readings_are_antipodal(azimuth_in: f64, azimuth_out: f64) -> bool {
    let bearing = average_azimuth_of(azimuth_in, azimuth_out);
    let swapped = average_azimuth_of(azimuth_out, azimuth_in);
    degrees_apart(bearing, swapped) > 90.0
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
/// load-bearing). A degenerate pair ([`readings_are_antipodal`]: the bearing
/// is `average_azimuth`'s tie-break, so reciprocate-and-swap is the identity)
/// is swapped instead: both values stay exactly as recorded while the reading
/// `average_azimuth` measures its tie-break from flips, which is what turns
/// the bearing around. Any other pair is reciprocated and swapped, and when
/// the rounding of the reciprocals left that pair on the tie with its bearing
/// unturned (a pair a few ulps short of opposite, module docs), the two
/// reciprocals are swapped back, which flips the tie. So the bearing this
/// returns always points opposite the shot's (non-finite readings aside,
/// which reciprocate as before). A second call returns the readings, the
/// swap exactly and the reciprocals to the ulp of the 180-degree turn, for
/// every pair on a decimal grid down to 0.001 degree; a crafted pair within a
/// few ulps of opposite can come back with its readings exchanged, its
/// bearing the original one (module docs).
pub fn reversed_azimuths(azimuth_in: f64, azimuth_out: f64) -> (f64, f64) {
    let (azimuth_in, azimuth_out) = (
        normalize_azimuth(azimuth_in),
        normalize_azimuth(azimuth_out),
    );
    if readings_are_antipodal(azimuth_in, azimuth_out) {
        return (azimuth_out, azimuth_in);
    }
    let (new_in, new_out) = (reciprocal(azimuth_out), reciprocal(azimuth_in));
    let unturned = degrees_apart(
        average_azimuth_of(new_in, new_out),
        average_azimuth_of(azimuth_in, azimuth_out),
    ) < 90.0;
    if unturned {
        (new_out, new_in)
    } else {
        (new_in, new_out)
    }
}

/// Re-associate one shot's readings with the opposite direction of travel.
///
/// Reverses the whole vec of shots as well, so the caller gets a line that
/// is fully reversed.  This is THE reverse transform: both the in-app action
/// ([`crate::survey::Project::reverse_line`]) and the import-time toggle route through it,
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
        // consumer; and the tie the antipodal test looks for is
        // `average_azimuth_of`'s on canonical readings. That verbatim swap
        // is exactly why a bare `rem_euclid` was not enough here: it can
        // return `360.0` (see `normalize_azimuth`), so the
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
