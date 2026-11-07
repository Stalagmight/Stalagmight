//! Reversing a section end-to-end (issue #24).
//!
//! The operation flips which physical end of a section is station 0 **without
//! moving the cave**: after a reverse every station keeps the exact world
//! coordinate it had before, and only the numbering direction changes.
//!
//! Preserving geometry takes more than reversing the shot vec.  A shot's
//! bearing is the circular mean of its two *forward* readings
//! ([`Shot::average_azimuth`](crate::Shot::average_azimuth)) -
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
//! the section turned around, moving the cave.  Such a pair is swapped instead,
//! which preserves both recorded values and flips the arbitrary tie-break that
//! [`average_azimuth`](crate::Shot::average_azimuth) resolves it with.
//!
//! Two things outside the shot vec also move:
//!
//! * **The section anchor.** `section_offsets` positions station 0, which is
//!   now the opposite physical end, so the offset gains the section's old
//!   total displacement or the whole section teleports.
//! * **External station refs.** Merges, loop closures, zero datums, station
//!   offsets and station comments are stored as anchor-based
//!   [`StationKey`](crate::StationKey)s.
//!   They still *resolve* after a reverse (UUIDs travel with their records),
//!   but to a different physical station - a zero datum on station 0 would end
//!   up pinning the far end.  Each key in the reversed section is remapped to
//!   its mirrored station so every relationship stays where the surveyor put
//!   it.

use std::collections::{HashMap, HashSet};

use crate::{
    normalize_azimuth, AzimuthReference, SectionId, Shot, ShotData, StationKey, StationRef,
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
/// shot would keep pointing the original way while the rest of the section
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
/// This is what a reverse has to add to `section_offsets` to keep the cave
/// still - station 0 becomes the far end, so the anchor moves by the span the
/// section used to cover. Shared by the in-app reverse and the import-time
/// toggle so the two placements can never disagree.
///
/// `reference` is the section's north reference (#438): the displacement has to
/// be measured in the same rotated frame the coordinate walk uses, or a reverse
/// of a section carrying a declination compensates by a vector that is off by
/// exactly that rotation - and the section slides.
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

/// Re-associate one shot's readings with the opposite direction of travel.
///
/// Reverses the whole vec of shots as well, so the caller gets a section that
/// is fully reversed.  This is THE reverse transform: both the in-app action
/// ([`Project::reverse_section`]) and the import-time toggle route through it,
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
        // branch below swaps its operands VERBATIM, so whatever this line leaves
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
        // UNCHANGED while the rest of the section turned around. Out-of-range
        // readings are reachable: `validation.rs` flags an azimuth outside
        // `[0, 360)` as `INVALID_AZIMUTH`, which is `WarningSeverity::Error` and
        // so cannot be ignored - but it is a report, not a gate, and nothing
        // rejects the value. The DMP parser (models::dmp) also stored
        // `heading_in`/`heading_out` verbatim in older builds, where the Compass
        // and TML importers normalised theirs.
        shot.azimuth_in = normalize_azimuth(shot.azimuth_in);
        shot.azimuth_out = normalize_azimuth(shot.azimuth_out);

        if readings_are_antipodal(shot.azimuth_in, shot.azimuth_out) {
            // Degenerate pair: the two readings disagree by a full 180 degrees,
            // so the circular mean is undefined and reciprocate-and-swap is the
            // identity.  Swapping the readings instead keeps both values exactly
            // as recorded - they are the same two numbers either way - while
            // flipping which one `average_azimuth` measures its ±180 tie-break
            // from, which is what actually turns the bearing around.  Still an
            // involution, so the round-trip guarantee holds.
            std::mem::swap(&mut shot.azimuth_in, &mut shot.azimuth_out);
        } else {
            let azimuth_in = shot.azimuth_in;
            shot.azimuth_in = reciprocal(shot.azimuth_out);
            shot.azimuth_out = reciprocal(azimuth_in);
        }

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
/// sharing a UUID (`ensure_section_shot_uuids` mints, it does not dedupe).
/// `key_for_ref` only ever emits canonical anchors, so both mirror onto the same
/// new key.
///
/// Freezing only the repeats of a mirrored key is not enough.  The survivor of a
/// colliding group keeps its OLD key, and that old key can itself be a third
/// key's mirror target, so the clash just moves - and which member survives is
/// `HashMap` iteration order, making the drop appear about half the time.  A
/// three-shot example: `Index(0)` and `SectionStart` both name station 0 and both
/// mirror to `AfterShot(uuid_2)`, while `AfterShot(uuid_2)` mirrors back to
/// `SectionStart`; freeze `SectionStart` and it now collides with that third key.
/// So this walks the FULL image - remapped and left-alone keys alike - and
/// freezes every member of every clash, repeating until nothing clashes.  It
/// terminates because `frozen` only grows and the all-frozen mapping is the
/// identity on a set of distinct keys.
///
/// A frozen key keeps naming its pre-reverse station: wrong, but present and
/// visible, which beats one of them vanishing.  No traced path reaches this today
/// (load, snapshot restore and the offset redrive all canonicalize `Index`
/// anchors first), so the ordinary reverse pays one pass that finds nothing.
fn frozen_remaps(
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
    /// Reverse `section_id` end-to-end, preserving every station's world
    /// coordinate.  `swap_lr` also mirrors the `left`/`right` passage
    /// dimensions (the user is now facing the other way); it is opt-in because
    /// whether LRUD was recorded facing along travel is a survey convention we
    /// cannot infer.
    ///
    /// Returns `false` if the section does not exist.  A section with no shots
    /// is a single station and reverses to itself.
    ///
    /// Involutive up to floating point: every step is a swap, a negation or the
    /// 180-degree turn, so `reverse_section(id, s)` twice restores the original
    /// section - but `(az + 180) % 360` is only *bit*-exact when both the shifted
    /// value and the wrap are representable (e.g. `17.5 -> 197.5 -> 17.5`; `0.1`
    /// comes back a couple of ulps out).  Undo therefore snapshots rather than
    /// re-running the transform - see `AppState::generate_undo_data`.
    pub fn reverse_section(&mut self, section_id: SectionId, swap_lr: bool) -> bool {
        let Some(section) = self.get_section(section_id) else {
            return false;
        };
        let shot_count = section.shots.len();
        if shot_count == 0 {
            return true;
        }

        // Every mirrored station has to be re-keyable, and an `AfterShot` anchor
        // needs the shot's uuid.  A pre-v6 .stalx loaded without uuids would
        // leave `key_for_ref` returning None for the mirrored station, and the
        // write pass below would silently keep the pre-reverse key - parking a
        // merge, datum or comment on the wrong physical end.  Mint the missing
        // ones up front instead; load already does this, so this is belt-and-
        // braces for any path that didn't.  Section-scoped: only this section's
        // anchors are rebuilt below, and a multi-section reverse runs one
        // `ReverseSection` per section.
        self.ensure_section_shot_uuids(section_id);

        // ---- Read pass, before anything moves -------------------------------
        // Station `i` of this section becomes station `shot_count - i`.  Resolve
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
                if key.section_id != section_id || out.contains_key(&key) {
                    return;
                }
                if let Some(sr) = self.resolve_key(&key) {
                    out.insert(key, shot_count as u32 - sr.station_index);
                }
            });
            out
        };
        // The universe the write pass below has to stay injective over: every
        // stored key of this section, whether or not it turns out to be
        // re-keyable.  A key that drops out of `rebuilt` keeps its old value and
        // can still be collided with, so the collision check needs it too.
        let stored_keys: HashSet<StationKey> = mirrored.keys().cloned().collect();

        let Some(section) = self.get_section(section_id) else {
            return false;
        };

        // Ids belong to positions, not records: the shot that was 1 becomes N.
        // Identity (uuid) travels with the record instead, so the station
        // anchors keyed on it (undo/redo `AfterShot`, `shot_uuid_index`) follow
        // the shot to its mirrored position rather than pointing at whatever
        // record inherited its old id.
        let ids: Vec<_> = section.shots.iter().map(|s| s.id).collect();
        let uuids: Vec<_> = section.shots.iter().map(|s| s.uuid.clone()).collect();
        let mut data: Vec<ShotData> = section.shots.iter().map(Shot::to_shot_data).collect();

        // Measured on the PRE-reverse order, so it is the offset the anchor has
        // to gain below once station 0 becomes the far end.
        let (dx, dy) = shot_run_displacement(&data, self.azimuth_reference_for(section));

        // ---- Mutate the shots ----------------------------------------------
        reverse_shots(&mut data, swap_lr);
        let Some(section) = self.get_section_mut(section_id) else {
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
        // failure instead of every station anchor in the section moving quietly.
        section.shots = data
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

        // Every shot in this section moved, so every cached `(section_id, uuid) ->
        // index` entry for it is stale.  A stale entry is only a slow path (
        // `resolve_key` re-checks and rescans), but the reverse invalidated all of
        // them at once - re-index the one section rather than leave every later
        // merge/closure/datum/comment lookup on the O(shots) fallback.
        self.reindex_section_shot_uuids(section_id);

        // ---- Keep the section where it was ----------------------------------
        // Station 0 is now the far end, so the anchor moves by the displacement
        // the section used to span.
        //
        // Finiteness first: `NaN != 0.0` is TRUE, so a bare `!= 0.0` would admit
        // a non-finite displacement straight into `section_offsets`, which has no
        // other finite guard - `save_project` rewrites a non-finite offset to 0.0,
        // teleporting the section to the origin (the exact opposite of what this
        // compensation is for). Finite lengths are enough: two tapes near
        // `f64::MAX` sum past it to `inf`, while `ShotData::all_finite` still
        // passes. `move_segment` and `move_station` reject non-finite offsets for
        // the same reason.
        if dx.is_finite() && dy.is_finite() && (dx != 0.0 || dy != 0.0) {
            self.adjust_section_offset(section_id, dx, dy);
        }

        // ---- Write pass: mirror the external refs ---------------------------
        let rebuilt: HashMap<StationKey, StationKey> = mirrored
            .into_iter()
            .filter_map(|(old, index)| {
                let next = self.key_for_ref(StationRef::new(section_id, index));
                // `ensure_section_shot_uuids` above guarantees every `AfterShot`
                // anchor can be built, so a ref we mirrored must be re-keyable.
                // If that ever stops holding, the key is left untouched below -
                // and it now names the OPPOSITE physical station - so trip loudly
                // in dev rather than silently misplacing the relationship.
                debug_assert!(
                    next.is_some(),
                    "reverse_section: station ref in section {section_id} mirrored \
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
                section_id,
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
        // reverse a section without it: the least-squares gauge reads it to stay on the same
        // physical station instead of jumping to the far end of the component.
        if let Some(section) = self.get_section_mut(section_id) {
            section.reverse_parity = !section.reverse_parity;
        }

        self.touch();
        true
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::*;
    use crate::config::ClosureAdjustmentConfig;
    use crate::test_fixtures::straight_shot;
    use crate::{ClosureStrategy, LoopClosure, SectionData, ShotData, Station, StationMerge};
    use chrono::Utc;

    fn shot(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
        straight_shot(azimuth, length).with_depths(depth_in, depth_out)
    }

    fn project_with_shots(shots: Vec<ShotData>) -> (Project, SectionId) {
        let mut project = Project::new("t".to_string(), None);
        let section = project.add_section(SectionData {
            name: None,
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots,
        });
        let id = section.id;
        project.ensure_shot_uuids();
        (project, id)
    }

    /// World coordinates of every station in `section_id`, in station order.
    ///
    /// Deliberately routed through the production walk rather than a local copy
    /// of `offset + sum(horizontal * sin/cos(average_azimuth))`: these tests exist
    /// to catch the reverse and the coordinate pipeline disagreeing, and a private
    /// re-implementation would keep passing if both drifted the same way.
    fn world_coords(project: &Project, section_id: SectionId) -> Vec<(f64, f64)> {
        let station_count = project.get_section(section_id).unwrap().shots.len() + 1;
        (0..station_count)
            .map(|i| {
                project
                    .compute_station_raw_xy(StationRef::new(section_id, i as u32))
                    .unwrap()
            })
            .collect()
    }

    fn assert_coords_close(a: &[(f64, f64)], b: &[(f64, f64)]) {
        assert_eq!(a.len(), b.len(), "station count changed");
        for (i, (p, q)) in a.iter().zip(b).enumerate() {
            assert!(
                (p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9,
                "station {i} moved: {p:?} -> {q:?}"
            );
        }
    }

    #[test]
    fn reverse_preserves_every_station_world_coordinate() {
        let (mut project, id) = project_with_shots(vec![
            shot(0.0, 10.0, 0.0, 2.0),
            shot(90.0, 5.0, 2.0, 2.0),
            shot(210.0, 7.5, 2.0, 1.0),
        ]);

        let before = world_coords(&project, id);
        assert!(project.reverse_section(id, false));
        let after = world_coords(&project, id);

        // Same physical stations, walked from the other end.
        let mut reversed = after;
        reversed.reverse();
        assert_coords_close(&before, &reversed);
    }

    /// Bit-exact round trip, which holds only because every azimuth here survives
    /// `(az + 180) % 360` exactly (17.5 -> 197.5 -> 17.5, 359 -> 179 -> 359,
    /// 210.25 -> 30.25 -> 210.25). That is the common case, not a guarantee - see
    /// [`reverse_of_an_unrepresentable_azimuth_round_trips_to_within_an_ulp`] for
    /// the general one, and `AppState::generate_undo_data` for why undo snapshots
    /// instead of re-running the transform.
    #[test]
    fn reverse_is_an_involution() {
        let (mut project, id) = project_with_shots(vec![
            shot(17.5, 10.0, 0.0, 2.0),
            shot(359.0, 5.0, 2.0, 2.0),
            shot(210.25, 7.5, 2.0, 1.0),
        ]);
        let original = project.get_section(id).unwrap().clone();

        assert!(project.reverse_section(id, true));
        assert!(project.reverse_section(id, true));

        let round_tripped = project.get_section(id).unwrap();
        assert_eq!(round_tripped.shots.len(), original.shots.len());
        for (a, b) in round_tripped.shots.iter().zip(&original.shots) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.uuid, b.uuid);
            assert_eq!(a.azimuth_in, b.azimuth_in);
            assert_eq!(a.azimuth_out, b.azimuth_out);
            assert_eq!(a.depth_in, b.depth_in);
            assert_eq!(a.depth_out, b.depth_out);
            assert_eq!(a.pitch_in, b.pitch_in);
            assert_eq!(a.pitch_out, b.pitch_out);
            assert_eq!(a.left, b.left);
            assert_eq!(a.right, b.right);
        }
    }

    /// The involution is exact in real arithmetic but not in f64: `0.1 + 180`
    /// rounds, and the `% 360` wrap on the way back cannot undo that, so the
    /// azimuth returns 2e-14 off. Pinned because the codebase relies on it in two
    /// places - undo snapshots the project instead of re-running the reverse, and
    /// nothing may promote "involutive" to "bit-exact".
    #[test]
    fn reverse_of_an_unrepresentable_azimuth_round_trips_to_within_an_ulp() {
        let (mut project, id) = project_with_shots(vec![shot(0.1, 10.0, 0.0, 0.0)]);

        assert!(project.reverse_section(id, false));
        assert!(project.reverse_section(id, false));

        let azimuth = project.get_section(id).unwrap().shots[0].azimuth_in;
        assert!(
            (azimuth - 0.1).abs() < 1e-9,
            "round trip drifted more than a rounding error: {azimuth}"
        );
        assert_ne!(
            azimuth, 0.1,
            "if this is now bit-exact the reciprocal changed - revisit the doc \
             comments that say the round trip is only exact for representable \
             shifts, and undo_capture's reason for snapshotting"
        );
    }

    #[test]
    fn reverse_swaps_azimuth_reciprocally_not_positionally() {
        // The regression this guards: swapping azimuth_in/out alone leaves
        // average_azimuth untouched, so the section would keep its old bearings.
        let (mut project, id) = project_with_shots(vec![shot(90.0, 10.0, 0.0, 0.0)]);
        let before = project.get_section(id).unwrap().shots[0].average_azimuth_measured();

        assert!(project.reverse_section(id, false));

        let after = project.get_section(id).unwrap().shots[0].average_azimuth_measured();
        assert!(
            (after - 270.0).abs() < 1e-9,
            "expected reciprocal bearing 270, got {after}"
        );
        assert!((before - 90.0).abs() < 1e-9);
    }

    #[test]
    fn reverse_swaps_depths_and_negates_pitch() {
        let mut data = shot(0.0, 10.0, 1.0, 4.0);
        data.pitch_in = Some(30.0);
        data.pitch_out = Some(-10.0);
        let (mut project, id) = project_with_shots(vec![data]);

        assert!(project.reverse_section(id, false));

        let s = &project.get_section(id).unwrap().shots[0];
        assert_eq!(s.depth_in, 4.0);
        assert_eq!(s.depth_out, 1.0);
        assert_eq!(s.pitch_in, Some(10.0));
        assert_eq!(s.pitch_out, Some(-30.0));
    }

    #[test]
    fn reverse_swaps_left_right_only_when_opted_in() {
        let mut data = shot(0.0, 10.0, 0.0, 0.0);
        data.left = Some(1.0);
        data.right = Some(4.0);
        data.up = Some(2.0);
        data.down = Some(3.0);

        let (mut project, id) = project_with_shots(vec![data.clone()]);
        assert!(project.reverse_section(id, false));
        let kept = &project.get_section(id).unwrap().shots[0];
        assert_eq!(kept.left, Some(1.0));
        assert_eq!(kept.right, Some(4.0));

        let (mut project, id) = project_with_shots(vec![data]);
        assert!(project.reverse_section(id, true));
        let swapped = &project.get_section(id).unwrap().shots[0];
        assert_eq!(swapped.left, Some(4.0));
        assert_eq!(swapped.right, Some(1.0));
        // Up/down are absolute and never mirror.
        assert_eq!(swapped.up, Some(2.0));
        assert_eq!(swapped.down, Some(3.0));
    }

    #[test]
    fn reverse_moves_ids_to_mirrored_positions() {
        let (mut project, id) = project_with_shots(vec![
            shot(0.0, 1.0, 0.0, 0.0),
            shot(0.0, 2.0, 0.0, 0.0),
            shot(0.0, 3.0, 0.0, 0.0),
        ]);
        let ids: Vec<_> = project
            .get_section(id)
            .unwrap()
            .shots
            .iter()
            .map(|s| s.id)
            .collect();

        assert!(project.reverse_section(id, false));

        let section = project.get_section(id).unwrap();
        // Ids stay ascending by position, records moved under them.
        assert_eq!(
            section.shots.iter().map(|s| s.id).collect::<Vec<_>>(),
            ids,
            "shot ids should stay attached to positions"
        );
        // The 3m shot was last and is now first.
        assert_eq!(section.shots[0].length, 3.0);
    }

    #[test]
    fn reverse_keeps_external_refs_on_their_physical_station() {
        let (mut project, id) =
            project_with_shots(vec![shot(0.0, 10.0, 0.0, 0.0), shot(90.0, 10.0, 0.0, 0.0)]);

        // A comment on station 0 and one on station 1 (the middle).
        let start = project.key_for_ref(StationRef::new(id, 0)).unwrap();
        let middle = project.key_for_ref(StationRef::new(id, 1)).unwrap();
        project
            .station_comments
            .insert(start, "at the entrance".to_string());
        project
            .station_comments
            .insert(middle, "at the bend".to_string());

        let world_before = world_coords(&project, id);
        assert!(project.reverse_section(id, false));
        let world_after = world_coords(&project, id);

        // Station i became station N - i, and nothing physically moved.
        let mut reversed = world_after;
        reversed.reverse();
        assert_coords_close(&world_before, &reversed);

        // The entrance comment now lives on station 2, the bend still on 1 -
        // both are the same physical places as before.
        let at = |index: u32| {
            project
                .station_comments
                .iter()
                .find(|(k, _)| project.resolve_key(k) == Some(StationRef::new(id, index)))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(at(2), Some("at the entrance"));
        assert_eq!(at(1), Some("at the bend"));
        assert_eq!(at(0), None);
    }

    /// Readings exactly 180 degrees apart have an undefined circular mean, and
    /// reciprocate-and-swap maps such a pair to itself.  Without the swap
    /// fix-up the shot keeps its original bearing while every other shot turns
    /// around, so the offset compensation no longer cancels and the passage
    /// jumps - the one way a reverse can move the cave.
    #[test]
    fn reverse_turns_around_a_shot_whose_readings_are_exactly_opposite() {
        let mut data = shot(0.0, 10.0, 0.0, 0.0);
        data.azimuth_in = 30.0;
        data.azimuth_out = 210.0;
        let (mut project, id) = project_with_shots(vec![data]);

        let before = project.get_section(id).unwrap().shots[0].average_azimuth_measured();
        let world_before = world_coords(&project, id);

        assert!(project.reverse_section(id, false));

        let after = project.get_section(id).unwrap().shots[0].average_azimuth_measured();
        assert!(
            (after - reciprocal(before)).abs() < 1e-9,
            "bearing did not turn around: {before} -> {after}"
        );

        // Both readings are still on the record, just the other way round.
        let s = &project.get_section(id).unwrap().shots[0];
        assert_eq!(s.azimuth_in, 210.0);
        assert_eq!(s.azimuth_out, 30.0);

        // And the whole point: the cave stayed put.
        let mut reversed = world_coords(&project, id);
        reversed.reverse();
        assert_coords_close(&world_before, &reversed);
    }

    #[test]
    fn reverse_of_opposite_readings_is_still_an_involution() {
        let mut data = shot(0.0, 10.0, 0.0, 0.0);
        data.azimuth_in = 30.0;
        data.azimuth_out = 210.0;
        let (mut project, id) = project_with_shots(vec![data]);

        assert!(project.reverse_section(id, false));
        assert!(project.reverse_section(id, false));

        let s = &project.get_section(id).unwrap().shots[0];
        assert_eq!(s.azimuth_in, 30.0);
        assert_eq!(s.azimuth_out, 210.0);
    }

    /// The degenerate branch swaps its operands VERBATIM, so whatever the
    /// pre-normalisation above leaves on the record is what the pair keeps.  A
    /// bare `rem_euclid` maps a tiny negative reading to exactly `360.0`, so the
    /// step meant to establish "both readings are in `[0, 360)`" used to hand the
    /// swap an out-of-range value and mint a non-ignorable `INVALID_AZIMUTH`
    /// itself: `(-1e-14, 180.0)` came back `(180.0, 360.0)`.
    #[test]
    fn reverse_of_a_degenerate_pair_with_a_tiny_negative_reading_stays_in_range() {
        assert_eq!(
            (-1e-14_f64).rem_euclid(360.0),
            360.0,
            "precondition: the bare rem_euclid wrap escapes [0, 360) here"
        );

        let mut data = shot(0.0, 10.0, 0.0, 0.0);
        data.azimuth_in = -1e-14;
        data.azimuth_out = 180.0;
        let mut shots = [data];
        reverse_shots(&mut shots, false);

        let (azimuth_in, azimuth_out) = (shots[0].azimuth_in, shots[0].azimuth_out);
        assert!(
            (0.0..360.0).contains(&azimuth_in) && (0.0..360.0).contains(&azimuth_out),
            "reverse left a reading out of range: ({azimuth_in}, {azimuth_out})"
        );
        // The pair IS degenerate once normalised (0 and 180), so it is swapped.
        assert_eq!(azimuth_in, 180.0);
        assert_eq!(azimuth_out, 0.0);
    }

    /// A near-antipodal pair is NOT degenerate - its mean is well defined, so it
    /// must take the ordinary reciprocate-and-swap path.  Guards against
    /// widening the exact test above into an epsilon window, which would give
    /// these shots the wrong bearing.
    #[test]
    fn reverse_treats_near_opposite_readings_normally() {
        let mut data = shot(0.0, 10.0, 0.0, 0.0);
        data.azimuth_in = 30.0;
        data.azimuth_out = 209.5;
        let (mut project, id) = project_with_shots(vec![data]);

        let before = project.get_section(id).unwrap().shots[0].average_azimuth_measured();
        assert!(project.reverse_section(id, false));
        let after = project.get_section(id).unwrap().shots[0].average_azimuth_measured();

        assert!(
            (after - reciprocal(before)).abs() < 1e-9,
            "bearing did not turn around: {before} -> {after}"
        );
    }

    /// Pre-v6 sections can carry `uuid: None` shots.  Without minting uuids
    /// first, the mirrored station has no `AfterShot` anchor, `key_for_ref`
    /// returns None and the write pass keeps the pre-reverse key - leaving the
    /// comment on the opposite physical end.
    #[test]
    fn reverse_remaps_refs_even_when_shots_arrive_without_uuids() {
        let (mut project, id) =
            project_with_shots(vec![shot(0.0, 10.0, 0.0, 0.0), shot(90.0, 10.0, 0.0, 0.0)]);

        // A comment on station 0, then strip the uuids the way a pre-v6 load would.
        let start = project.key_for_ref(StationRef::new(id, 0)).unwrap();
        project
            .station_comments
            .insert(start, "at the entrance".to_string());
        for shot in &mut project.get_section_mut(id).unwrap().shots {
            shot.uuid = None;
        }

        assert!(project.reverse_section(id, false));

        // Station 0 became station 2, and the comment went with it.
        let at = |index: u32| {
            project
                .station_comments
                .iter()
                .find(|(k, _)| project.resolve_key(k) == Some(StationRef::new(id, index)))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(at(2), Some("at the entrance"));
        assert_eq!(at(0), None);
    }

    /// Every other mutating `Project` method stamps `modified_at`; without it a
    /// reverse saves a project that still reports the previous session's edit time,
    /// so the recent-projects list and any "changed since?" check read it as
    /// untouched.
    #[test]
    fn reverse_marks_the_project_modified() {
        let (mut project, id) = project_with_shots(vec![shot(0.0, 10.0, 0.0, 0.0)]);
        let before = project.modified_at;

        assert!(project.reverse_section(id, false));

        assert!(
            project.modified_at > before,
            "reverse must touch the project: {before} -> {}",
            project.modified_at
        );
    }

    /// A missing section and an empty section both leave the project alone, so
    /// neither may stamp `modified_at` - an empty section reverses to itself.
    #[test]
    fn reverse_of_a_noop_does_not_mark_the_project_modified() {
        let (mut project, id) = project_with_shots(vec![]);
        let before = project.modified_at;

        assert!(project.reverse_section(id, false));
        assert!(!project.reverse_section(9999, false));

        assert_eq!(project.modified_at, before);
    }

    /// The reverse permutes every shot index in the section, so every cached
    /// `(section_id, uuid) -> index` entry is wrong afterwards. Stale entries are
    /// only a slow path (`resolve_key` rescans), so this asserts the cache itself
    /// rather than a behaviour change.
    #[test]
    fn reverse_refreshes_the_sections_shot_uuid_index() {
        let (mut project, id) = project_with_shots(vec![
            shot(0.0, 1.0, 0.0, 0.0),
            shot(0.0, 2.0, 0.0, 0.0),
            shot(0.0, 3.0, 0.0, 0.0),
        ]);
        project.rebuild_shot_uuid_index();
        let first_uuid = project.get_section(id).unwrap().shots[0]
            .uuid
            .clone()
            .unwrap();

        assert!(project.reverse_section(id, false));

        // The record that was at index 0 is now last.
        assert_eq!(
            project
                .shot_uuid_index
                .get(&id)
                .and_then(|m| m.get(&first_uuid)),
            Some(&2),
            "the uuid index must follow the record to its mirrored position"
        );
        // And the cache still agrees with the cold scan for every shot.
        for (i, shot) in project.get_section(id).unwrap().shots.iter().enumerate() {
            let uuid = shot.uuid.clone().unwrap();
            assert_eq!(
                project.shot_uuid_index.get(&id).and_then(|m| m.get(&uuid)),
                Some(&(i as u32))
            );
        }
    }

    /// `section_offsets` has no finite guard of its own, and `save_project`
    /// rewrites a non-finite offset to 0.0 - so admitting one here teleports the
    /// section to the origin on the next save, the exact opposite of what the
    /// compensation exists for. Finite lengths are enough to produce it: two
    /// tapes near `f64::MAX` sum past it to `inf`, while `ShotData::all_finite`
    /// still passes. (One 1e200 tape no longer does: `horizontal_length_from_slope`
    /// stays finite since #576 NF-TML-1.)
    #[test]
    fn reverse_refuses_to_write_a_non_finite_offset() {
        let absurd = || vec![shot(0.0, f64::MAX, 0.0, 0.0), shot(0.0, f64::MAX, 0.0, 0.0)];
        let (mut project, id) = project_with_shots(absurd());
        let (dx, dy) = shot_run_displacement(&absurd(), AzimuthReference::default());
        assert!(
            !(dx.is_finite() && dy.is_finite()),
            "precondition: this fixture must actually overflow to a non-finite \
             displacement, or the test proves nothing"
        );

        assert!(project.reverse_section(id, false));

        let (ox, oy) = project.section_offset(id);
        assert!(
            ox.is_finite() && oy.is_finite(),
            "reverse wrote a non-finite section offset: ({ox}, {oy})"
        );
    }

    /// A far-out-of-range pair must survive a reverse with the cave still.
    /// Reachable: validation only WARNS on an azimuth outside `[0, 360)` and the
    /// DMP parser stores headings verbatim. This pinned a real bug - while
    /// `average_azimuth_of` wrapped with the truncated `%` and the reverse's own
    /// helpers with `rem_euclid`, the two disagreed for a pair this far out, the
    /// degenerate-pair guard missed, and the bearing survived the reverse
    /// unturned, so the offset compensation no longer cancelled and the passage
    /// jumped by its own span. Both wrap with `rem_euclid` now; the test stands
    /// as the guard against either one drifting back.
    #[test]
    fn reverse_keeps_the_cave_still_for_out_of_range_azimuths() {
        let mut data = shot(0.0, 10.0, 0.0, 0.0);
        data.azimuth_in = 5.0;
        data.azimuth_out = -715.0;
        let (mut project, id) = project_with_shots(vec![data]);

        let world_before = world_coords(&project, id);
        assert!(project.reverse_section(id, false));

        let mut reversed = world_coords(&project, id);
        reversed.reverse();
        assert_coords_close(&world_before, &reversed);
    }

    /// Two stored keys can name one station (a legacy `Index` anchor beside the
    /// canonical one). Both mirror to the same new key, and `map_station_refs`
    /// rebuilds the comment map by collecting - so remapping both would silently
    /// drop one. Leaving them put is wrong-but-visible; losing one is not.
    #[test]
    fn reverse_does_not_drop_a_comment_when_two_keys_name_one_station() {
        use crate::{StationAnchor, StationKey};

        let (mut project, id) =
            project_with_shots(vec![shot(0.0, 10.0, 0.0, 0.0), shot(90.0, 10.0, 0.0, 0.0)]);
        let canonical = project.key_for_ref(StationRef::new(id, 0)).unwrap();
        let legacy = StationKey {
            section_id: id,
            anchor: StationAnchor::Index(0),
        };
        assert_ne!(canonical, legacy, "precondition: two distinct keys");
        assert_eq!(
            project.resolve_key(&canonical),
            project.resolve_key(&legacy)
        );
        project.station_comments.insert(canonical, "a".to_string());
        project.station_comments.insert(legacy, "b".to_string());

        assert!(project.reverse_section(id, false));

        assert_eq!(
            project.station_comments.len(),
            2,
            "a colliding remap must not silently drop a comment"
        );
    }

    /// The three-key counterexample the two-key case above cannot reach.
    ///
    /// `Index(0)` and `SectionStart` both name station 0 and both mirror onto
    /// `AfterShot(uuid_2)` (station 3); that third key mirrors back onto
    /// `SectionStart`. Freezing only the *repeat* of a mirrored key leaves the
    /// survivor of the pair on its old key - and if the survivor is
    /// `SectionStart`, the third key now remaps straight onto it and
    /// `map_station_refs`' `collect()` drops one comment. Which member survives
    /// is `HashMap` iteration order, so the drop appeared about half the time;
    /// looping over fresh maps turns that into a certainty for the old guard
    /// while staying deterministic for the fixed one.
    #[test]
    fn reverse_does_not_drop_a_comment_when_a_frozen_key_is_another_keys_target() {
        use crate::{StationAnchor, StationKey};

        for _ in 0..64 {
            let (mut project, id) = project_with_shots(vec![
                shot(0.0, 10.0, 0.0, 0.0),
                shot(90.0, 10.0, 0.0, 0.0),
                shot(180.0, 10.0, 0.0, 0.0),
            ]);
            let start = project.key_for_ref(StationRef::new(id, 0)).unwrap();
            let far_end = project.key_for_ref(StationRef::new(id, 3)).unwrap();
            let legacy = StationKey {
                section_id: id,
                anchor: StationAnchor::Index(0),
            };
            assert_eq!(
                project.resolve_key(&legacy),
                project.resolve_key(&start),
                "precondition: the legacy key must name station 0 too"
            );
            project.station_comments.insert(start, "a".to_string());
            project.station_comments.insert(legacy, "b".to_string());
            project.station_comments.insert(far_end, "c".to_string());

            assert!(project.reverse_section(id, false));

            assert_eq!(
                project.station_comments.len(),
                3,
                "a colliding remap must not silently drop a comment"
            );
        }
    }

    /// `Project::shot_displacement` writes the persisted `section_offsets`
    /// compensation on insert/prepend/delete, so it has to agree with the walk
    /// that actually places the stations. It used to inline its own `%` copy of
    /// the mean; once `average_azimuth_of` moved to `rem_euclid` the two
    /// disagreed by 180 degrees for readings more than 540 apart (here: 120 vs
    /// 300), and an insert or delete moved the section by twice the shot vector
    /// and saved it. Out-of-range readings are reachable - validation only warns.
    #[test]
    fn shot_displacement_agrees_with_the_coordinate_walk_for_out_of_range_readings() {
        let mut data = shot(0.0, 10.0, 0.0, 0.0);
        data.azimuth_in = 600.0;
        data.azimuth_out = 0.0;

        let (project, id) = project_with_shots(vec![data.clone()]);
        let coords = world_coords(&project, id);
        let walked = (coords[1].0 - coords[0].0, coords[1].1 - coords[0].1);

        let (dx, dy) = Project::shot_displacement(&data, AzimuthReference::default());
        assert!(
            (dx - walked.0).abs() < 1e-9 && (dy - walked.1).abs() < 1e-9,
            "shot_displacement {:?} disagrees with the coordinate walk {walked:?}",
            (dx, dy)
        );
        // And with the reverse's own run-displacement helper, which the import
        // toggle and the in-app reverse both use to keep the cave still.
        let (rx, ry) = shot_run_displacement(&[data], AzimuthReference::default());
        assert!((dx - rx).abs() < 1e-9 && (dy - ry).abs() < 1e-9);
    }

    /// A datum-less component whose loop closure is eligible for the
    /// least-squares solve: two sections merged end to end and tied back with a
    /// closure that has a real misclosure. `plan_adjustment_anchors` has to
    /// invent a gauge anchor for exactly this shape.
    ///
    /// Returns the project plus both section ids; `a` is the lower id, so it is
    /// the one the default anchor is picked from.
    fn project_with_eligible_closure() -> (Project, SectionId, SectionId) {
        let mut project = Project::new("closure".to_string(), None);
        let a = project
            .add_section(SectionData {
                shots: vec![shot(0.0, 10.0, 0.0, 0.0), shot(0.0, 10.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        let b = project
            .add_section(SectionData {
                // 0.5m long, so the loop does not quite close.
                shots: vec![shot(180.0, 20.5, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        project.ensure_shot_uuids();
        project.station_merges.push(StationMerge {
            station_a: project.key_for_ref(StationRef::new(a, 2)).unwrap(),
            station_b: project.key_for_ref(StationRef::new(b, 0)).unwrap(),
            merged_at: Utc::now(),
        });
        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: project.key_for_ref(StationRef::new(b, 1)).unwrap(),
            station_b: project.key_for_ref(StationRef::new(a, 0)).unwrap(),
            path_length: 40.5,
            misclosure_distance: 0.5,
            error_percentage: 1.2,
            horizontal_misclosure: 0.5,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::LeastSquaresWeighted,
            is_aligned: false,
        });
        (project, a, b)
    }

    /// The reverse promises every station keeps its world position, and
    /// Plan/Export mode FORCES the closure-adjusted view - so the guarantee has
    /// to hold for ADJUSTED coordinates, not only the measured walk every other
    /// reverse test asserts on.
    ///
    /// It did not. A component with an eligible closure and no zero datum has its
    /// otherwise-free translation pinned by `default_anchor_for_component`, which
    /// picked the lex-min `(section_id, station_index)`. A reverse renumbers
    /// station `i` to `N - i`, so `(a, 0)` became the OTHER physical end and the
    /// entire adjusted component slid by the adjustment residual between the two
    /// ends of the section.
    #[test]
    fn reverse_preserves_adjusted_coordinates_of_a_datum_less_component() {
        let (project, a, b) = project_with_eligible_closure();
        let config = ClosureAdjustmentConfig::default();
        let shots_a = project.get_section(a).unwrap().shots.len() as u32;

        let before = project.clone().into_adjusted_station_coordinates(&config);
        let mut project = project;
        assert!(project.reverse_section(a, false));
        let after = project.into_adjusted_station_coordinates(&config);

        let at = |stations: &[Station], sr: StationRef| {
            stations
                .iter()
                .find(|s| s.station_ref == sr)
                .map(|s| (s.coordinates.x, s.coordinates.y))
                .unwrap()
        };

        // Sanity: without an anchor the solve would be free to translate at will,
        // so pin that the adjustment actually did something first.
        assert!(
            (at(&before, StationRef::new(b, 1)).1 - at(&before, StationRef::new(a, 0)).1).abs()
                < 0.5,
            "precondition: the closure should have been adjusted"
        );

        // Section `a`'s station `i` is now station `shots_a - i`; `b` is untouched.
        for i in 0..=shots_a {
            let p = at(&before, StationRef::new(a, i));
            let q = at(&after, StationRef::new(a, shots_a - i));
            assert!(
                (p.0 - q.0).abs() < 1e-6 && (p.1 - q.1).abs() < 1e-6,
                "adjusted station a:{i} moved: {p:?} -> {q:?}"
            );
        }
        for i in 0..=1 {
            let p = at(&before, StationRef::new(b, i));
            let q = at(&after, StationRef::new(b, i));
            assert!(
                (p.0 - q.0).abs() < 1e-6 && (p.1 - q.1).abs() < 1e-6,
                "adjusted station b:{i} moved: {p:?} -> {q:?}"
            );
        }
    }

    #[test]
    fn reverse_of_empty_section_is_a_noop() {
        let (mut project, id) = project_with_shots(vec![]);
        assert!(project.reverse_section(id, false));
        assert!(project.get_section(id).unwrap().shots.is_empty());
    }

    #[test]
    fn reverse_of_missing_section_fails() {
        let (mut project, _) = project_with_shots(vec![]);
        assert!(!project.reverse_section(9999, false));
    }
}
