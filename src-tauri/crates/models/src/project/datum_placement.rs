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
//! 2. When the frame changed, the whole survey is translated onto the new
//!    frame ([`Project::renormalize_frame_if_changed`]).
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
    /// Every datum in list order, with the role it had (`None`: dangling).
    datums: Vec<(StationKey, GpsCoordinates, Option<DatumRole>)>,
}

impl DatumLayout {
    /// The station that defined the frame.
    pub fn frame_key(&self) -> Option<StationKey> {
        self.frame.as_ref().map(|(key, _)| key.clone())
    }

    /// Whether `zd` was a GPS fix or a dangling datum before, and not a
    /// placement datum. A datum is recognised by its station key, or by its GPS
    /// when a split or a combine re-keyed it onto another section; a datum
    /// that matches no earlier one is new (its command placed it already).
    fn was_unplaced(&self, zd: &ZeroDatum) -> bool {
        let matches = |(key, gps, _): &&(StationKey, GpsCoordinates, Option<DatumRole>)| {
            *key == zd.station_ref || same_gps(gps, &zd.gps)
        };
        let mut earlier = self.datums.iter().filter(matches).peekable();
        earlier.peek().is_some() && earlier.all(|(_, _, role)| *role != Some(DatumRole::Placement))
    }
}

/// Bit-for-bit the same stored GPS: identity, not proximity.
fn same_gps(a: &GpsCoordinates, b: &GpsCoordinates) -> bool {
    a.latitude.to_bits() == b.latitude.to_bits() && a.longitude.to_bits() == b.longitude.to_bits()
}

impl Project {
    /// The datums as they are now, for [`Self::settle_datum_layout`] after a
    /// command. Cheap: a walk of `zero_datums` with a `resolve_key` each.
    pub fn datum_layout(&self) -> DatumLayout {
        DatumLayout {
            frame: self
                .frame_datum()
                .map(|zd| (zd.station_ref.clone(), zd.gps)),
            datums: self
                .zero_datums
                .iter()
                .zip(self.datum_roles())
                .map(|(zd, role)| (zd.station_ref.clone(), zd.gps, role))
                .collect(),
        }
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
                .filter(|zd| before.was_unplaced(zd))
                .filter_map(|zd| Some((self.resolve_key(&zd.station_ref)?, zd.gps)))
                .collect();
            for (station_ref, gps) in promoted {
                moved |= self.place_group_at(station_ref, gps_to_local(&gps, &frame_gps));
            }
        }
        self.renormalize_frame_if_changed(before.frame_key()) || moved
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
