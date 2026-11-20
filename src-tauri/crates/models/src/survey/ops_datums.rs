//! Zero datums (plan 5.7, anchors 4.1, frame 4.3): set, GPS and accuracy
//! edits, remove, entrance answers, and the Edit Station preview.
//!
//! A station may hold several datums (migrated merges, or two placed pieces
//! merged at stations that each had one). The datum a slot "has" is the first
//! datum on its station attached via the slot's line, else the first datum on
//! the station ([`Project::datum_index_for`]): that is the one Set edits and
//! Remove removes. Roles are derived (the first datum of a piece places it,
//! later ones are GPS fixes), so:
//!
//! - **Set on a station without a datum** appends one. On a free piece it is
//!   the placement datum: the piece goes onto its GPS (the first datum of the
//!   whole survey translates the survey onto it instead) and its placement is
//!   dropped. On a placed piece it is a GPS fix: nothing moves.
//! - **Set with the same GPS** is an accuracy-only edit: nothing moves,
//!   whatever the role, even for a piece an older file left off its GPS.
//! - **Set with a new GPS** edits in place (list order is the role). A
//!   placement datum's correction is cleared and its piece goes onto the new
//!   GPS; the frame stays at the origin and every other placed piece follows
//!   its kept correction (today's `place_pieces_on_frame`); a fix moves
//!   nothing.
//! - **Remove** re-derives the roles: a promoted fix puts its piece on its
//!   GPS, a piece that loses its last datum gets a placement where it is, and
//!   a frame change translates the survey so nothing moves relative to
//!   anything else ("measure, do not re-derive"). The orphan-closure sweep
//!   runs afterwards, since a piece that loses its datum can no longer hold a
//!   closure to another piece.

use crate::{
    validate_gps_accuracy, DatumRole, GpsCoordinates, LineId, StationId, StationRef,
    ZeroDatumPreview,
};

use super::m3_stubs::{datum_gap, datum_is_entrance, datum_roles};
use super::ops::{OpError, OpOutcome, Sweep};
use super::{EditContext, Project, ZeroDatum};

/// What a Set did to the datum list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatumChange {
    /// A new datum was appended.
    Added,
    /// An existing datum got a new GPS position (and accuracy).
    GpsEdited,
    /// An existing datum got the GPS it had: only its accuracy (and depth
    /// reference) changed. Nothing moved.
    AccuracyOnly,
}

/// What [`Project::set_datum`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatumSet {
    /// The datum's list index.
    pub index: usize,
    pub change: DatumChange,
    /// Its role afterwards.
    pub role: Option<DatumRole>,
}

impl Project {
    /// The datum a slot on `station` via `via` has: the first datum on the
    /// station attached via `via`, else the first datum on the station.
    pub fn datum_index_for(&self, station: StationId, via: Option<LineId>) -> Option<usize> {
        let on_station = || {
            self.datums
                .iter()
                .enumerate()
                .filter(move |(_, d)| d.station == station)
        };
        via.and_then(|line| on_station().find(|(_, d)| d.via == Some(line)))
            .or_else(|| on_station().next())
            .map(|(index, _)| index)
    }

    /// Set a zero datum on `station` (see the module docs), attached via
    /// `via` when it is new.
    pub fn set_datum(
        &mut self,
        station: StationId,
        via: Option<LineId>,
        gps: GpsCoordinates,
        accuracy_m: Option<f64>,
    ) -> Result<OpOutcome<DatumSet>, OpError> {
        self.existing_station(station)?;
        GpsCoordinates::validate(gps.latitude, gps.longitude, gps.depth_reference)
            .map_err(OpError::InvalidGps)?;
        validate_gps_accuracy(accuracy_m).map_err(OpError::InvalidAccuracy)?;
        if let Some(line) = via {
            if self.topology().slot_on(station, line).is_none() {
                return Err(OpError::StationNotOnLine { station, line });
            }
        }
        let outcome = match self.datum_index_for(station, via) {
            Some(index) if same_position(&self.datums[index].gps, &gps) => self.run_plain(|p| {
                let datum = &mut p.datums[index];
                datum.gps = gps;
                datum.accuracy_m = accuracy_m;
                (index, DatumChange::AccuracyOnly)
            })?,
            Some(index) => self.run_op(Sweep::Skip, |p| {
                let previous = p.datums[index].gps;
                let datum = &mut p.datums[index];
                datum.gps = gps;
                datum.accuracy_m = accuracy_m;
                Ok((
                    (index, DatumChange::GpsEdited),
                    EditContext::none().with_gps_edit(index, previous),
                ))
            })?,
            None => self.run_op(Sweep::Skip, |p| {
                let index = p.push_datum(ZeroDatum {
                    via,
                    accuracy_m,
                    ..ZeroDatum::new(station, gps)
                });
                Ok(((index, DatumChange::Added), EditContext::none()))
            })?,
        };
        let (index, change) = outcome.value;
        let role = datum_roles(self).get(index).copied().flatten();
        Ok(OpOutcome {
            value: DatumSet {
                index,
                change,
                role,
            },
            anchors: outcome.anchors,
            sweep: outcome.sweep,
        })
    }

    /// [`Self::set_datum`] on a slot: its station, attached via its line.
    pub fn set_datum_at(
        &mut self,
        slot: StationRef,
        gps: GpsCoordinates,
        accuracy_m: Option<f64>,
    ) -> Result<OpOutcome<DatumSet>, OpError> {
        let station = self.station_of_slot(slot)?;
        self.set_datum(station, Some(slot.line_id), gps, accuracy_m)
    }

    /// Remove the datum at list index `index`; returns it as it was stored.
    pub fn remove_datum(&mut self, index: usize) -> Result<OpOutcome<ZeroDatum>, OpError> {
        if index >= self.datums.len() {
            return Err(OpError::UnknownDatum(index));
        }
        self.run_op(Sweep::Run, |p| {
            Ok((p.datums.remove(index), EditContext::none()))
        })
    }

    /// Remove the datum a slot has ([`Self::datum_index_for`]); `None` (and
    /// nothing done) when it has none, as today's command answers `false`.
    pub fn remove_datum_at(
        &mut self,
        slot: StationRef,
    ) -> Result<Option<OpOutcome<ZeroDatum>>, OpError> {
        let station = self.station_of_slot(slot)?;
        match self.datum_index_for(station, Some(slot.line_id)) {
            Some(index) => self.remove_datum(index).map(Some),
            None => Ok(None),
        }
    }

    /// Store the user's entrance answer on datum `index` (`None` returns it to
    /// the default rule). Nothing moves.
    pub fn set_datum_entrance(
        &mut self,
        index: usize,
        entrance: Option<bool>,
    ) -> Result<(), OpError> {
        if index >= self.datums.len() {
            return Err(OpError::UnknownDatum(index));
        }
        self.run_plain(|p| p.datums[index].entrance = entrance)
            .map(|_| ())
    }

    /// Whether datum `index` is an entrance (its role, override and the
    /// default rule, plan 3.8). `None` for an unknown or unplaced datum.
    pub fn datum_entrance(&self, index: usize) -> Option<bool> {
        let datum = self.datums.get(index)?;
        let role = datum_roles(self).get(index).copied().flatten()?;
        Some(datum_is_entrance(
            self,
            datum.station,
            datum.via,
            role,
            datum.entrance,
        ))
    }

    /// What setting a datum with `gps` on `station` (via `via`) would do, for
    /// the Edit Station dialog (today's `preview_zero_datum`): the role it
    /// would have (a stored datum keeps its own), the gap to the surveyed
    /// position for a GPS fix, and whether it would count as an entrance.
    /// `None` for a station on no line. Stores nothing.
    pub fn preview_zero_datum(
        &self,
        station: StationId,
        via: Option<LineId>,
        gps: &GpsCoordinates,
    ) -> Option<ZeroDatumPreview> {
        self.existing_station(station).ok()?;
        let stored = self.datum_index_for(station, via);
        let role = stored
            .and_then(|index| datum_roles(self).get(index).copied().flatten())
            .unwrap_or_else(|| {
                if self.placement_datum_of(station).is_some() {
                    DatumRole::GpsFix
                } else {
                    DatumRole::Placement
                }
            });
        let gap = match role {
            DatumRole::GpsFix => datum_gap(self, station, gps),
            DatumRole::Placement => None,
        };
        let (via, entrance) = stored
            .and_then(|index| self.datums.get(index))
            .map_or((via, None), |d| (d.via.or(via), d.entrance));
        Some(ZeroDatumPreview {
            role,
            gap,
            is_entrance: datum_is_entrance(self, station, via, role, entrance),
        })
    }

    /// An edit that cannot move anything (accuracy, entrance): mutate, then
    /// validate (finite numbers); roll back on a violation. No anchors.
    pub(super) fn run_plain<T>(
        &mut self,
        op: impl FnOnce(&mut Project) -> T,
    ) -> Result<OpOutcome<T>, OpError> {
        let backup = self.clone();
        let value = op(self);
        let violations = self.validate();
        if violations.is_empty() {
            Ok(OpOutcome {
                value,
                anchors: super::AnchorReport::default(),
                sweep: super::SweepReport::default(),
            })
        } else {
            *self = backup;
            Err(OpError::Invalid(violations))
        }
    }
}

/// The same horizontal GPS position, bit for bit (today's `same_gps`: the
/// depth reference does not place anything).
fn same_position(a: &GpsCoordinates, b: &GpsCoordinates) -> bool {
    a.latitude.to_bits() == b.latitude.to_bits() && a.longitude.to_bits() == b.longitude.to_bits()
}
