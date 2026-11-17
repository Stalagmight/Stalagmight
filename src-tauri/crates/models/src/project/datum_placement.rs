//! Settling the datums after a command (#469 item 1, #741).
//!
//! A command can change WHICH datum places a merge component, or which one
//! defines the frame, without asking for it: removing a placement datum, an
//! unmerge or a delete that splits a component all re-derive the roles
//! ([`Project::datum_roles`]), so a GPS fix can become its piece's placement
//! datum. Two things then have to follow, in this order:
//!
//! 1. A datum that has just become a placement datum places its piece, exactly
//!    as [`Project::set_zero_datum`] places a subsequent datum: its rigid group
//!    is translated so its station sits at `gps_to_local(datum.gps, frame.gps)`.
//!    Without this the piece stays where the old placement put it, off its own
//!    GPS by the fix's gap, and when the removed datum was the frame every
//!    other separately placed piece ends up off its GPS by that gap too.
//! 2. When the frame changed (another station, or the same key on another
//!    datum's GPS), the whole survey is translated onto the new frame
//!    ([`Project::renormalize_frame`]).
//!
//! Step 1 measures against the frame that was in effect BEFORE the command,
//! because until step 2 runs every stored coordinate is still read against it.
//!
//! Both steps live in [`Project::settle_datum_layout`], which every command
//! epilogue calls with the [`DatumLayout`] it read before dispatch. Derived,
//! not hooked: the epilogue asks the state what changed instead of each
//! command remembering to say so.

use tracing::{info, warn};

use super::Project;
use crate::{gps_to_local, DatumRole, GpsCoordinates, StationKey, StationRef, ZeroDatum};

/// The datums as they were before a command: read with
/// [`Project::datum_layout`] before dispatch, handed to
/// [`Project::settle_datum_layout`] after it.
#[derive(Debug, Clone, Default)]
pub struct DatumLayout {
    /// The frame datum's station and GPS.
    frame: Option<(StationKey, GpsCoordinates)>,
    /// Every datum in list order.
    datums: Vec<DatumWas>,
}

/// One datum as it was before a command.
#[derive(Debug, Clone)]
struct DatumWas {
    key: StationKey,
    gps: GpsCoordinates,
    /// The role it had (`None`: dangling).
    role: Option<DatumRole>,
    /// Its station's horizontal position (`None`: dangling).
    at: Option<(f64, f64)>,
    /// Its stored accuracy (bits) and Entrance answer: what else tells two
    /// datums with one GPS apart.
    accuracy: Option<u64>,
    entrance: Option<bool>,
}

/// Within the offset dust of each other: the same place.
fn same_place(a: Option<(f64, f64)>, b: Option<(f64, f64)>) -> bool {
    matches!((a, b), (Some(a), Some(b))
        if (a.0 - b.0).abs() <= Project::OFFSET_DUST_M && (a.1 - b.1).abs() <= Project::OFFSET_DUST_M)
}

impl DatumLayout {
    /// Whether `zd`, whose station is now `at`, was a GPS fix or a dangling
    /// datum before, and not a placement datum. `after` is every datum now.
    ///
    /// Datums have no identity of their own, so one is recognised by the
    /// first of these that finds an earlier datum:
    /// 1. the same station key, GPS and position: untouched;
    /// 2. the same GPS and position, against an earlier datum that is gone (no
    ///    datum now has its key and its stored GPS, accuracy and Entrance
    ///    answer): re-keyed where it stands (a split, or a shot delete);
    /// 3. the same station key and GPS: moved (a merge, a combine);
    /// 4. the same GPS, against an earlier datum that is gone: re-keyed and
    ///    moved (a combine);
    /// 5. the same station key: its GPS was edited in place.
    ///
    /// Tiers 1 to 4 also ask for the same stored accuracy and Entrance answer.
    /// A command that moves or re-keys a datum never edits it, and one that
    /// edits it never moves it, so an earlier datum with another stored value
    /// is a different datum, and only tier 5 (an edit) may match across it.
    /// That is what tells twins apart: two datums with one GPS (copied
    /// coordinates) and different accuracies, one re-keyed by a combine onto
    /// the key the other had.
    ///
    /// Key and GPS alone are not enough. Deleting a section's first shot drops
    /// the datum on its first station and re-anchors the next station's datum
    /// onto that key, and a GPS copied from another datum matches that datum
    /// too: either way the promoted datum would pass for the placement datum
    /// it replaced. The position settles it, since neither moves a station.
    ///
    /// Gone compares the whole stored value, not the GPS alone: a combine can
    /// re-key a datum onto the key of another with the very same GPS (copied
    /// coordinates), and that one would otherwise pass for still there.
    ///
    /// When one tier still finds earlier datums of both roles (twins with
    /// the same stored value, both re-keyed and moved by a combine), the datum
    /// counts as unplaced: placing it moves nothing when its piece already
    /// sits on that GPS. A datum that matches no earlier one is new (its
    /// command placed it already).
    fn was_unplaced(&self, zd: &ZeroDatum, at: Option<(f64, f64)>, after: &[ZeroDatum]) -> bool {
        let gone = |was: &DatumWas| {
            !after.iter().any(|now| {
                now.station_ref == was.key
                    && same_gps(&now.gps, &was.gps)
                    && now.accuracy_m.map(f64::to_bits) == was.accuracy
                    && now.entrance == was.entrance
            })
        };
        let same_key = |was: &DatumWas| was.key == zd.station_ref;
        let same_fix = |was: &DatumWas| {
            same_gps(&was.gps, &zd.gps)
                && was.accuracy == zd.accuracy_m.map(f64::to_bits)
                && was.entrance == zd.entrance
        };
        let same_at = |was: &DatumWas| same_place(was.at, at);
        let tiers: [&dyn Fn(&DatumWas) -> bool; 5] = [
            &|was| same_key(was) && same_fix(was) && same_at(was),
            &|was| same_fix(was) && same_at(was) && gone(was),
            &|was| same_key(was) && same_fix(was),
            &|was| same_fix(was) && gone(was),
            &|was| same_key(was),
        ];
        tiers
            .iter()
            .find_map(|tier| {
                let found: Vec<&DatumWas> = self.datums.iter().filter(|was| tier(was)).collect();
                if found.is_empty() {
                    return None;
                }
                Some(
                    !found
                        .iter()
                        .all(|was| was.role == Some(DatumRole::Placement)),
                )
            })
            .unwrap_or(false)
    }

    /// Whether the frame datum is not the one it was: another station, or
    /// another GPS (a promoted datum re-keyed onto the old frame's key).
    ///
    /// No frame now (`Some -> None`, the last datum removed) is no change:
    /// there is nothing to re-frame onto. A first frame (`None -> Some`) is
    /// one, though [`Project::set_zero_datum`] has already put it on the
    /// origin, so the measured re-frame is a no-op there.
    fn frame_changed(&self, now: Option<&ZeroDatum>) -> bool {
        match (&self.frame, now) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some((key, gps)), Some(zd)) => zd.station_ref != *key || !same_gps(&zd.gps, gps),
        }
    }
}

/// Bit-for-bit the same stored GPS: identity, not proximity.
pub(super) fn same_gps(a: &GpsCoordinates, b: &GpsCoordinates) -> bool {
    a.latitude.to_bits() == b.latitude.to_bits() && a.longitude.to_bits() == b.longitude.to_bits()
}

impl Project {
    /// The datums as they are now, for [`Self::settle_datum_layout`] after a
    /// command: a walk of `zero_datums` with a `resolve_key` and a shot walk
    /// to the station each.
    pub fn datum_layout(&self) -> DatumLayout {
        DatumLayout {
            frame: self
                .frame_datum()
                .map(|zd| (zd.station_ref.clone(), zd.gps)),
            datums: self
                .zero_datums
                .iter()
                .zip(self.datum_roles())
                .map(|(zd, role)| DatumWas {
                    key: zd.station_ref.clone(),
                    gps: zd.gps,
                    role,
                    at: self.datum_position(zd),
                    accuracy: zd.accuracy_m.map(f64::to_bits),
                    entrance: zd.entrance,
                })
                .collect(),
        }
    }

    /// The horizontal position of `zd`'s station, `None` when it dangles.
    /// The shot walk only: no colocation search, which would cost a pass over
    /// every merge per datum for a depth this never reads.
    fn datum_position(&self, zd: &ZeroDatum) -> Option<(f64, f64)> {
        self.compute_station_raw_xy(self.resolve_key(&zd.station_ref)?)
    }

    /// Place every piece whose placement datum changed in a command, then
    /// re-frame when the frame changed (see the module doc). `before` is what
    /// [`Self::datum_layout`] returned before the command. Returns true when
    /// anything moved.
    ///
    /// Run it ONCE per command, on the forward path only: the placement step
    /// measures against `before`'s frame, so a second call with the same
    /// `before` after a re-frame would place against a frame that is no longer
    /// the origin. Undo restores; it never settles.
    pub fn settle_datum_layout(&mut self, before: &DatumLayout) -> bool {
        let mut moved = false;
        // With no frame before, nothing was placed relative to anything, and
        // the re-frame below is the whole answer.
        if let Some((_, frame_gps)) = before.frame {
            let promoted: Vec<(StationRef, GpsCoordinates)> = self
                .placement_datums()
                .into_iter()
                .filter(|zd| before.was_unplaced(zd, self.datum_position(zd), &self.zero_datums))
                .filter_map(|zd| Some((self.resolve_key(&zd.station_ref)?, zd.gps)))
                .collect();
            for (station_ref, gps) in promoted {
                moved |= self.place_group_at(station_ref, gps_to_local(&gps, &frame_gps));
            }
        }
        // Compared by key AND GPS: a promoted datum can inherit the old
        // frame's key (see `DatumLayout::was_unplaced`), and the frame has
        // still changed. Re-framing is measured, so an unchanged frame is a
        // no-op anyway.
        if before.frame_changed(self.frame_datum()) {
            moved |= self.renormalize_frame();
        }
        moved
    }

    /// Translate `station_ref`'s rigid group so the station sits at `target`,
    /// then cascade through aligned closures: the same placement
    /// [`Self::set_zero_datum`] applies to a subsequent datum. Returns true when
    /// anything moved.
    fn place_group_at(&mut self, station_ref: StationRef, target: (f64, f64)) -> bool {
        let Some(current) = self.compute_station_raw_coords(station_ref) else {
            return false;
        };
        let (dx, dy) = (target.0 - current.x, target.1 - current.y);
        if !dx.is_finite() || !dy.is_finite() {
            warn!(
                ?station_ref,
                purpose = "place a piece by its promoted datum",
                "Datum placement is not finite; leaving the piece where it is"
            );
            return false;
        }
        if dx.abs() <= Self::OFFSET_DUST_M && dy.abs() <= Self::OFFSET_DUST_M {
            return false;
        }
        self.adjust_rigid_group_offsets(station_ref.section_id, dx, dy);
        self.cascade_connection_offsets(&[station_ref.section_id]);
        info!(
            ?station_ref,
            delta_x = dx,
            delta_y = dy,
            purpose = "place a piece by its promoted datum",
            "A datum became its piece's placement datum; moved the piece onto its GPS"
        );
        true
    }
}
