//! Reversing a line end-to-end (issue #24).
//!
//! The operation flips which physical end of a line is station 0 **without
//! moving the cave**: after a reverse every station keeps the exact world
//! coordinate it had before, and only the numbering direction changes.
//!
//! Preserving geometry takes more than reversing the shot vec.  A shot's
//! bearing is the circular mean of its two *forward* readings
//! ([`Shot::average_azimuth`](crate::models::Shot::average_azimuth)) -
//! `azimuth_in`/`azimuth_out` are not a
//! foresight/backsight pair - so merely swapping them is a no-op and would
//! leave every bearing pointing the original way.  Walking the reversed chain
//! then produces the same two endpoints (vector addition commutes) but a
//! different polyline in between.  Each shot must therefore be *reciprocated*:
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
//! the line turned around, moving the cave.  Such a pair is swapped instead,
//! which preserves both recorded values and flips the arbitrary tie-break that
//! [`average_azimuth`](crate::models::Shot::average_azimuth) resolves it with.
//!
//! Two things outside the shot vec also move:
//!
//! * **The line anchor.** `line_offsets` positions station 0, which is
//!   now the opposite physical end, so the offset gains the line's old
//!   total displacement or the whole line teleports.
//! * **External station refs.** Merges, loop closures, zero datums, station
//!   offsets and station comments are stored as anchor-based
//!   [`StationKey`](crate::models::StationKey)s.
//!   They still *resolve* after a reverse (UUIDs travel with their records),
//!   but to a different physical station - a zero datum on station 0 would end
//!   up pinning the far end.  Each key in the reversed line is remapped to
//!   its mirrored station so every relationship stays where the surveyor put
//!   it.

use std::collections::{HashMap, HashSet};

use crate::models::{
    normalize_azimuth, AzimuthReference, LineId, Shot, ShotData, StationKey, StationRef,
};

use super::Project;

/// Reciprocal of a forward azimuth, normalised to `[0, 360)`.
fn reciprocal(azimuth: f64) -> f64 {
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
fn readings_are_antipodal(azimuth_in: f64, azimuth_out: f64) -> bool {
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

/// Stored keys whose mirrored remap must be SKIPPED so the rewrite stays
/// injective - i.e. so no two keys come out of it equal.
///
/// `map_station_refs` rebuilds `station_offsets`/`station_comments` by collecting
/// transformed keys into a HashMap, so any two keys that leave the write pass
/// equal silently drop one entry - a station comment or offset lost with no
/// error.  Two stored keys can name one station: a legacy
/// `StationAnchor::Index(i)` beside the canonical `AfterShot`, or two shots
/// sharing a UUID (`ensure_line_shot_uuids` mints, it does not dedupe).
/// `key_for_ref` only ever emits canonical anchors, so both mirror onto the same
/// new key.
///
/// Freezing only the repeats of a mirrored key is not enough.  The survivor of a
/// colliding group keeps its OLD key, and that old key can itself be a third
/// key's mirror target, so the clash just moves - and which member survives is
/// `HashMap` iteration order, making the drop appear about half the time.  A
/// three-shot example: `Index(0)` and `LineStart` both name station 0 and both
/// mirror to `AfterShot(uuid_2)`, while `AfterShot(uuid_2)` mirrors back to
/// `LineStart`; freeze `LineStart` and it now collides with that third key.
/// So this walks the FULL image - remapped and left-alone keys alike - and
/// freezes every member of every clash, repeating until nothing clashes.  It
/// terminates because `frozen` only grows and the all-frozen mapping is the
/// identity on a set of distinct keys.
///
/// A frozen key keeps naming its pre-reverse station: wrong, but present and
/// visible, which beats one of them vanishing.  No traced path reaches this today
/// (load, snapshot restore and the offset redrive all canonicalize `Index`
/// anchors first), so the ordinary reverse pays one pass that finds nothing.
pub(super) fn frozen_remaps(
    stored: &HashSet<StationKey>,
    rebuilt: &HashMap<StationKey, StationKey>,
) -> HashSet<StationKey> {
    let mut frozen: HashSet<StationKey> = HashSet::new();
    loop {
        let mut owners: HashMap<&StationKey, Vec<&StationKey>> =
            HashMap::with_capacity(stored.len());
        for key in stored {
            let image = if frozen.contains(key) {
                key
            } else {
                rebuilt.get(key).unwrap_or(key)
            };
            owners.entry(image).or_default().push(key);
        }
        let clashing: Vec<StationKey> = owners
            .into_values()
            .filter(|keys| keys.len() > 1)
            .flatten()
            .filter(|key| !frozen.contains(*key))
            .cloned()
            .collect();
        if clashing.is_empty() {
            return frozen;
        }
        frozen.extend(clashing);
    }
}

impl Project {
    /// Reverse `line_id` end-to-end, preserving every station's world
    /// coordinate.  `swap_lr` also mirrors the `left`/`right` passage
    /// dimensions (the user is now facing the other way); it is opt-in because
    /// whether LRUD was recorded facing along travel is a survey convention we
    /// cannot infer.
    ///
    /// Returns `false` if the line does not exist.  A line with no shots
    /// is a single station and reverses to itself.
    ///
    /// Involutive up to floating point: every step is a swap, a negation or the
    /// 180-degree turn, so `reverse_line(id, s)` twice restores the original
    /// line - but `(az + 180) % 360` is only *bit*-exact when both the shifted
    /// value and the wrap are representable (e.g. `17.5 -> 197.5 -> 17.5`; `0.1`
    /// comes back a couple of ulps out).  Undo therefore snapshots rather than
    /// re-running the transform - see `AppState::generate_undo_data`.
    pub fn reverse_line(&mut self, line_id: LineId, swap_lr: bool) -> bool {
        let Some(line) = self.get_line(line_id) else {
            return false;
        };
        let shot_count = line.shots.len();
        if shot_count == 0 {
            return true;
        }

        // Every mirrored station has to be re-keyable, and an `AfterShot` anchor
        // needs the shot's uuid.  A pre-v6 .stalx loaded without uuids would
        // leave `key_for_ref` returning None for the mirrored station, and the
        // write pass below would silently keep the pre-reverse key - parking a
        // merge, datum or comment on the wrong physical end.  Mint the missing
        // ones up front instead; load already does this, so this is belt-and-
        // braces for any path that didn't.  Line-scoped: only this line's
        // anchors are rebuilt below, and a multi-line reverse runs one
        // `ReverseLine` per line.
        self.ensure_line_shot_uuids(line_id);

        // ---- Read pass, before anything moves -------------------------------
        // Station `i` of this line becomes station `shot_count - i`.  Resolve
        // every stored key now, while the old order is still in place; the write
        // pass below rebuilds them from the mirrored indices.
        //
        // Keyed by the stored key rather than by traversal position: the read pass
        // walks `for_each_station_ref` and the write pass walks `map_station_refs`,
        // and the two rebuild the offset/comment HashMaps from independent
        // iterations, so pairing them by index would depend on `keys()` and
        // `into_iter()` agreeing on order.  A key visited twice (two merges can
        // share a station) mirrors to the same index either way.
        let mirrored: HashMap<StationKey, u32> = {
            let mut out: HashMap<StationKey, u32> = HashMap::new();
            self.for_each_station_ref(|_label, key| {
                if key.line_id != line_id || out.contains_key(&key) {
                    return;
                }
                if let Some(sr) = self.resolve_key(&key) {
                    out.insert(key, shot_count as u32 - sr.station_index);
                }
            });
            out
        };
        // The universe the write pass below has to stay injective over: every
        // stored key of this line, whether or not it turns out to be
        // re-keyable.  A key that drops out of `rebuilt` keeps its old value and
        // can still be collided with, so the collision check needs it too.
        let stored_keys: HashSet<StationKey> = mirrored.keys().cloned().collect();

        let Some(line) = self.get_line(line_id) else {
            return false;
        };

        // Ids belong to positions, not records: the shot that was 1 becomes N.
        // Identity (uuid) travels with the record instead, so the station
        // anchors keyed on it (undo/redo `AfterShot`, `shot_uuid_index`) follow
        // the shot to its mirrored position rather than pointing at whatever
        // record inherited its old id.
        let ids: Vec<_> = line.shots.iter().map(|s| s.id).collect();
        let uuids: Vec<_> = line.shots.iter().map(|s| s.uuid.clone()).collect();
        let mut data: Vec<ShotData> = line.shots.iter().map(Shot::to_shot_data).collect();

        // Measured on the PRE-reverse order, so it is the offset the anchor has
        // to gain below once station 0 becomes the far end.
        let (dx, dy) = shot_run_displacement(&data, self.azimuth_reference_for(line));

        // ---- Mutate the shots ----------------------------------------------
        reverse_shots(&mut data, swap_lr);
        let Some(line) = self.get_line_mut(line_id) else {
            return false;
        };

        // `uuids` is still in the OLD order, and so is `data` after the reverse
        // put record i at position count-1-i - zip them by walking the uuids
        // backwards alongside the reversed data.
        //
        // Since #423 this pairing is REDUNDANT: `to_shot_data` copies the uuid,
        // so each `ShotData` already carries its own identity and
        // `Shot::with_uuid` prefers it over the value zipped in here. It is kept
        // as a cross-check rather than deleted, because "the uuid vector is one
        // step out of sync with the shots" is precisely the bug this loop exists
        // to avoid, and a `debug_assert` turns a future regression into a loud
        // failure instead of every station anchor in the line moving quietly.
        line.shots = data
            .into_iter()
            .zip(uuids.into_iter().rev())
            .zip(ids)
            .map(|((d, uuid), id)| {
                debug_assert_eq!(
                    d.uuid, uuid,
                    "reverse: a shot's identity parted company with its record"
                );
                Shot::with_uuid(id, d, uuid)
            })
            .collect();

        // Every shot in this line moved, so every cached `(line_id, uuid) ->
        // index` entry for it is stale.  A stale entry is only a slow path (
        // `resolve_key` re-checks and rescans), but the reverse invalidated all of
        // them at once - re-index the one line rather than leave every later
        // merge/closure/datum/comment lookup on the O(shots) fallback.
        self.reindex_line_shot_uuids(line_id);

        // ---- Keep the line where it was ----------------------------------
        // Station 0 is now the far end, so the anchor moves by the displacement
        // the line used to span.
        //
        // Finiteness first: `NaN != 0.0` is TRUE, so a bare `!= 0.0` would admit
        // a non-finite displacement straight into `line_offsets`, which has no
        // other finite guard - `save_project` rewrites a non-finite offset to 0.0,
        // teleporting the line to the origin (the exact opposite of what this
        // compensation is for). A finite `length` near 1e200 is enough:
        // `horizontal_length_from_slope` squares it to `inf`, and `inf * sin(0)`
        // is NaN, while `ShotData::all_finite` still passes. `move_segment` and
        // `move_station` reject non-finite offsets for the same reason.
        if dx.is_finite() && dy.is_finite() && (dx != 0.0 || dy != 0.0) {
            self.adjust_line_offset(line_id, dx, dy);
        }

        // ---- Write pass: mirror the external refs ---------------------------
        let rebuilt: HashMap<StationKey, StationKey> = mirrored
            .into_iter()
            .filter_map(|(old, index)| {
                let next = self.key_for_ref(StationRef::new(line_id, index));
                // `ensure_line_shot_uuids` above guarantees every `AfterShot`
                // anchor can be built, so a ref we mirrored must be re-keyable.
                // If that ever stops holding, the key is left untouched below -
                // and it now names the OPPOSITE physical station - so trip loudly
                // in dev rather than silently misplacing the relationship.
                debug_assert!(
                    next.is_some(),
                    "reverse_line: station ref in line {line_id} mirrored \
                     to station {index} but could not be re-keyed - it would \
                     silently stay on its pre-reverse station"
                );
                next.map(|n| (old, n))
            })
            .collect();

        let frozen = frozen_remaps(&stored_keys, &rebuilt);
        // Warn rather than `debug_assert!`: the collision is handled, so tripping
        // would turn a recovered case into a dev-only crash.
        if !frozen.is_empty() {
            tracing::warn!(
                line_id,
                count = frozen.len(),
                "reverse: station refs collided on the mirrored key; left on their \
                 pre-reverse station to avoid dropping one"
            );
        }
        self.map_station_refs(|key| {
            if frozen.contains(&key) {
                return key;
            }
            rebuilt.get(&key).cloned().unwrap_or(key)
        });

        // Flip the orientation marker (#24). Recorded HERE, at the mutation, so no caller can
        // reverse a line without it: the least-squares gauge reads it to stay on the same
        // physical station instead of jumping to the far end of the component.
        if let Some(line) = self.get_line_mut(line_id) {
            line.reverse_parity = !line.reverse_parity;
        }

        self.touch();
        true
    }
}
