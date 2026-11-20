//! Datum roles, entrances and GPS gaps (plan 3.8).
//!
//! Ported from `models/src/project/datum_roles.rs` and `datum_entrance.rs`:
//!
//! - **Role**: the first datum (list order) of each component is its
//!   placement datum; every later datum in it is a GPS fix. A datum whose
//!   station is on no line has no role.
//! - **Entrance**: a placement datum always is one; a GPS fix follows its
//!   override, else it is one when its station is the first or the last
//!   station of its `via` line, or of its station's first line (list order)
//!   when `via` is unset.
//! - **GPS gap**: the horizontal distance from a fix's raw position to its GPS
//!   in the frame, against `15 m + 2%` of the survey distance from its
//!   component's placement datum, along shots only (junctions are free;
//!   closures are no shortcut).

use crate::station::DatumRole;
use crate::{
    datum_gap_threshold_m, gps_to_local, DatumGap, GpsCoordinates, LineId, StationId, StationRef,
    DATUM_GPS_GAP, DATUM_GPS_GAP_START,
};

use super::graph::{dijkstra, StationDistances};
use super::{Project, ZeroDatum};

/// A GPS fix whose gap exceeds its threshold (today's `FarGpsFix`, by datum).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FarFix {
    /// Index into `Project::datums`.
    pub datum: usize,
    pub station: StationId,
    /// The fix's slot (its `via` line's slot, else the station's first slot).
    pub slot: StationRef,
    pub gap_m: f64,
    pub threshold_m: f64,
}

impl FarFix {
    /// The wire form.
    pub fn to_wire(&self) -> crate::FarGpsFix {
        crate::FarGpsFix {
            station: self.slot,
            gap_m: self.gap_m,
            threshold_m: self.threshold_m,
        }
    }
}

/// What the `DATUM_GPS_GAP` / `DATUM_GPS_GAP_START` warning of one GPS fix is
/// built from (today's `warnings.rs` block): the fix's slot, the shot it hangs
/// on (station `i` -> shot `i - 1` under `DATUM_GPS_GAP`, station 0 -> shot 0
/// under `DATUM_GPS_GAP_START`) and the gap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsGapWarning {
    /// Index into `Project::datums`.
    pub datum: usize,
    pub slot: StationRef,
    /// The shot index on `slot.line_id` the warning hangs on; `None` when the
    /// line has no shot (today: logged, no row).
    pub shot_index: Option<usize>,
    pub code: &'static str,
    pub gap: DatumGap,
}

impl Project {
    /// Every datum's role, parallel to `datums`; `None` for a datum whose
    /// station is on no line.
    pub fn datum_roles(&self) -> Vec<Option<DatumRole>> {
        let topology = self.topology();
        let placement = self.placement_datums();
        self.datums
            .iter()
            .enumerate()
            .map(|(index, datum)| {
                let component = topology.component_of(datum.station)?;
                Some(if placement[component] == Some(index) {
                    DatumRole::Placement
                } else {
                    DatumRole::GpsFix
                })
            })
            .collect()
    }

    /// Indices of the datums with `role`, in list order.
    pub fn datums_with_role(&self, role: DatumRole) -> Vec<usize> {
        self.datum_roles()
            .into_iter()
            .enumerate()
            .filter(|(_, r)| *r == Some(role))
            .map(|(index, _)| index)
            .collect()
    }

    /// The line a datum belongs to: its `via` line when that line passes its
    /// station, else its station's first line (list order).
    pub fn datum_line(&self, datum: &ZeroDatum) -> Option<LineId> {
        let topology = self.topology();
        if let Some(via) = datum.via {
            if topology.slot_on(datum.station, via).is_some() {
                return Some(via);
            }
        }
        topology.first_slot(datum.station).map(|slot| slot.line_id)
    }

    /// The slot a datum is shown on: its station's slot on [`Self::datum_line`].
    pub fn datum_slot(&self, datum: &ZeroDatum) -> Option<StationRef> {
        let line = self.datum_line(datum)?;
        self.topology().slot_on(datum.station, line)
    }

    /// Whether `station` is the first or the last station of `line`.
    pub fn is_line_end_on(&self, station: StationId, line: LineId) -> bool {
        self.line(line).is_some_and(|l| {
            l.start == station || l.shots.last().map_or(false, |shot| shot.to == station)
        })
    }

    /// The entrance rule (plan 3.8): a placement datum always; a fix by its
    /// override, else when its station is an end of its line
    /// ([`Self::datum_line`]).
    pub fn datum_is_entrance(&self, datum: &ZeroDatum, role: DatumRole) -> bool {
        match role {
            DatumRole::Placement => true,
            DatumRole::GpsFix => datum.entrance.unwrap_or_else(|| {
                self.datum_line(datum)
                    .is_some_and(|line| self.is_line_end_on(datum.station, line))
            }),
        }
    }

    /// Every datum's entrance flag, parallel to `datums`; `None` for a datum
    /// with no role.
    pub fn datum_entrances(&self) -> Vec<Option<bool>> {
        self.datums
            .iter()
            .zip(self.datum_roles())
            .map(|(datum, role)| Some(self.datum_is_entrance(datum, role?)))
            .collect()
    }

    /// The stations of the entrance datums, in list order, each once: the
    /// roots of "Start from entrance", penetration and the distance gradient.
    pub fn entrance_stations(&self) -> Vec<StationId> {
        let mut out: Vec<StationId> = Vec::new();
        for (datum, entrance) in self.datums.iter().zip(self.datum_entrances()) {
            if entrance == Some(true) && !out.contains(&datum.station) {
                out.push(datum.station);
            }
        }
        out
    }

    /// Every station's survey distance from its component's placement datum,
    /// along shots only (today's `survey_distances_from_placements`; a shot
    /// with a non-finite length is never crossed).
    pub fn survey_distances_from_placements(&self) -> StationDistances {
        let topology = self.topology();
        // Shots only: no closure edge.
        let mut shots_only: Vec<Vec<(usize, f64)>> = vec![Vec::new(); topology.stations().len()];
        self.for_each_shot_edge(|edge| {
            let length = edge.line.shots[edge.index].length;
            let w = if length.is_finite() {
                length.max(0.0)
            } else {
                f64::INFINITY
            };
            shots_only[edge.from].push((edge.to, w));
            shots_only[edge.to].push((edge.from, w));
        });
        let roots: Vec<usize> = self
            .placement_datums()
            .into_iter()
            .flatten()
            .filter_map(|d| topology.dense(self.datums[d].station))
            .collect();
        StationDistances::new(topology, dijkstra(&shots_only, &roots))
    }

    /// The gap between `gps` and where the raw survey puts `station`, in the
    /// current frame (today's `datum_gap`, for the datum preview). `None` with
    /// no frame or no such station.
    pub fn datum_gap(&self, station: StationId, gps: &GpsCoordinates) -> Option<DatumGap> {
        let frame = self.datums.get(self.frame_datum()?)?.gps;
        let surveyed = self.raw_positions().xy(station)?;
        let distances = self.survey_distances_from_placements();
        Some(gap_between(surveyed, gps, &frame, distances.get(station)))
    }

    /// The gap of every GPS fix, by datum index in list order (today's
    /// `gps_fix_gaps`): one raw layout and one multi-source Dijkstra.
    pub fn gps_fix_gaps(&self) -> Vec<(usize, DatumGap)> {
        let Some(frame) = self.frame_datum().and_then(|f| self.datums.get(f)) else {
            return Vec::new();
        };
        let frame = frame.gps;
        let fixes = self.datums_with_role(DatumRole::GpsFix);
        if fixes.is_empty() {
            return Vec::new();
        }
        let positions = self.raw_positions();
        let distances = self.survey_distances_from_placements();
        fixes
            .into_iter()
            .filter_map(|d| {
                let datum = &self.datums[d];
                let surveyed = positions.xy(datum.station)?;
                Some((
                    d,
                    gap_between(surveyed, &datum.gps, &frame, distances.get(datum.station)),
                ))
            })
            .collect()
    }

    /// Every GPS fix whose gap exceeds its threshold, largest gap first, ties
    /// by slot (today's `far_gps_fixes`).
    pub fn far_gps_fixes(&self) -> Vec<FarFix> {
        let mut far: Vec<FarFix> = self
            .gps_fix_gaps()
            .into_iter()
            .filter(|(_, gap)| gap.exceeds_threshold)
            .filter_map(|(d, gap)| {
                let datum = &self.datums[d];
                Some(FarFix {
                    datum: d,
                    station: datum.station,
                    slot: self.datum_slot(datum)?,
                    gap_m: gap.gap_m,
                    threshold_m: gap.threshold_m,
                })
            })
            .collect();
        far.sort_by(|a, b| {
            b.gap_m
                .total_cmp(&a.gap_m)
                .then_with(|| a.slot.line_id.cmp(&b.slot.line_id))
                .then_with(|| a.slot.station_index.cmp(&b.slot.station_index))
                .then_with(|| a.datum.cmp(&b.datum))
        });
        far
    }

    /// The data of every GPS-gap warning, sorted by slot (today's
    /// `warnings.rs` block): one per fix whose gap exceeds its threshold.
    pub fn gps_gap_warnings(&self) -> Vec<GpsGapWarning> {
        let mut out: Vec<GpsGapWarning> = self
            .gps_fix_gaps()
            .into_iter()
            .filter(|(_, gap)| gap.exceeds_threshold)
            .filter_map(|(d, gap)| {
                let slot = self.datum_slot(&self.datums[d])?;
                let (shot_index, code) = match slot.station_index {
                    0 => (0, DATUM_GPS_GAP_START),
                    i => (i as usize - 1, DATUM_GPS_GAP),
                };
                let has_shot = self
                    .line(slot.line_id)
                    .is_some_and(|line| shot_index < line.shots.len());
                Some(GpsGapWarning {
                    datum: d,
                    slot,
                    shot_index: has_shot.then_some(shot_index),
                    code,
                    gap,
                })
            })
            .collect();
        out.sort_by_key(|w| (w.slot.line_id, w.slot.station_index, w.datum));
        out
    }
}

/// [`DatumGap`] from a station surveyed at local `(x, y)` to `gps`, in the
/// frame at `frame_gps` (today's private `DatumGap::between`).
pub(crate) fn gap_between(
    surveyed: [f64; 2],
    gps: &GpsCoordinates,
    frame_gps: &GpsCoordinates,
    survey_distance_m: Option<f64>,
) -> DatumGap {
    let (gx, gy) = gps_to_local(gps, frame_gps);
    let (dx, dy) = (gx - surveyed[0], gy - surveyed[1]);
    let gap_m = dx.hypot(dy);
    let threshold_m = datum_gap_threshold_m(survey_distance_m.unwrap_or(f64::INFINITY));
    DatumGap {
        gap_m,
        bearing_deg: dx.atan2(dy).to_degrees().rem_euclid(360.0),
        survey_distance_m,
        threshold_m,
        exceeds_threshold: gap_m > threshold_m,
    }
}
