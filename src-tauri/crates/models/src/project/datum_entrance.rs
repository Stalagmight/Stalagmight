//! Which zero datums are cave entrances (#741).
//!
//! A survey can hold several datums, and not every one is a way in: a GPS fix
//! can be a radiolocation point in the middle of a passage. Plan mode's
//! "Start from entrance", penetration and the distance gradient root their
//! walks at the ENTRANCE datums only ([`Project::entrance_datum_refs`]).
//!
//! The rule, resolved here and nowhere else (the wire carries the answer as
//! `ZeroDatumWire::is_entrance`):
//!
//! - a placement datum is always an entrance: it is where the survey is tied
//!   to the surface, and a piece with no entrance could not be planned from;
//! - a GPS fix follows the user's override (`ZeroDatum::entrance`) when there
//!   is one, and otherwise is an entrance when its station is the first or the
//!   last station of its section ([`Project::is_section_end`]): surveys start
//!   at an entrance and tie a second entrance in at the end of a line, while a
//!   fix in the middle of a line is a point inside the cave.

use super::{Project, StationRef};
use crate::station::DatumRole;

impl Project {
    /// Whether `station_ref` is the first or the last station of its section:
    /// the default rule for a GPS fix. `false` for a station that does not
    /// exist.
    pub fn is_section_end(&self, station_ref: StationRef) -> bool {
        let Some(section) = self.get_section(station_ref.section_id) else {
            return false;
        };
        let index = station_ref.station_index as usize;
        index == 0 || index == section.shots.len()
    }

    /// Whether a datum with `role` and `override_entrance` on `station_ref` is
    /// an entrance (see the module doc).
    pub fn datum_is_entrance(
        &self,
        station_ref: StationRef,
        role: DatumRole,
        override_entrance: Option<bool>,
    ) -> bool {
        match role {
            DatumRole::Placement => true,
            DatumRole::GpsFix => {
                override_entrance.unwrap_or_else(|| self.is_section_end(station_ref))
            }
        }
    }

    /// Every datum's entrance flag, parallel to `zero_datums`; `None` for a
    /// datum that does not resolve.
    pub fn datum_entrances(&self) -> Vec<Option<bool>> {
        self.zero_datums
            .iter()
            .zip(self.datum_roles())
            .map(|(zd, role)| {
                let sr = self.resolve_key(&zd.station_ref)?;
                Some(self.datum_is_entrance(sr, role?, zd.entrance))
            })
            .collect()
    }

    /// Resolved stations of the datums that are entrances, in list order: the
    /// roots of "Start from entrance", penetration and the distance gradient.
    pub fn entrance_datum_refs(&self) -> Vec<StationRef> {
        self.zero_datums
            .iter()
            .zip(self.datum_entrances())
            .filter(|(_, entrance)| *entrance == Some(true))
            .filter_map(|(zd, _)| self.resolve_key(&zd.station_ref))
            .collect()
    }

    /// Store the user's entrance answer on the datum at `station_ref`
    /// (`None` returns it to the default rule). Returns whether a datum was
    /// there. Nothing moves: the flag feeds only the entrance readers.
    pub fn set_zero_datum_entrance(
        &mut self,
        station_ref: StationRef,
        entrance: Option<bool>,
    ) -> bool {
        let Some(key) = self.key_for_ref(station_ref) else {
            return false;
        };
        let Some(datum) = self.zero_datums.iter_mut().find(|zd| zd.station_ref == key) else {
            return false;
        };
        if datum.entrance != entrance {
            datum.entrance = entrance;
            self.touch();
        }
        true
    }
}
