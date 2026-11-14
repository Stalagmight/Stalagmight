//! Several zero datums per merge-connected component (#741).
//!
//! `zero_datums` is one ordered list. Each datum's ROLE is derived from that
//! order and from the merge graph, never stored:
//!
//! - the first datum (in list order) that RESOLVES in a merge component is that
//!   component's [`DatumRole::Placement`] datum. It places the rigid group in Raw
//!   Data View exactly as a datum always has, and the first placement datum of
//!   the project is the frame ([`Project::frame_datum`]);
//! - every later resolvable datum in the same component is a
//!   [`DatumRole::GpsFix`]. It never moves raw geometry. The Closures Computed
//!   View solve pulls the survey toward it, weighted by its accuracy.
//!
//! Merging, unmerging, deleting and undo therefore re-derive every role with no
//! promote or demote bookkeeping. Everything that used to assume "at most one
//! datum per merge component" reads placement datums through
//! [`Project::placement_datum_refs`]; only the solve, the gap check, export and
//! Plan mode read the fixes.

use std::collections::{HashMap, HashSet};

use super::{Project, SectionId, StationRef, ZeroDatum};
use crate::station::DatumRole;
use crate::{gps_to_local, GpsCoordinates};

/// Fixed part of the gap warning threshold, in metres (#741 decision 4).
pub const DATUM_GAP_WARN_BASE_M: f64 = 15.0;

/// Part of the gap warning threshold that grows with the survey distance to
/// the nearest other datum: 2% (#741 decision 4).
pub const DATUM_GAP_WARN_FRACTION: f64 = 0.02;

/// The gap above which a datum's GPS and its surveyed position disagree enough
/// to warn: 15 m plus 2% of the survey distance to the nearest other datum.
/// An infinite or NaN distance (no other datum reachable) yields the base.
pub fn datum_gap_threshold_m(survey_distance_m: f64) -> f64 {
    if survey_distance_m.is_finite() && survey_distance_m > 0.0 {
        DATUM_GAP_WARN_BASE_M + DATUM_GAP_WARN_FRACTION * survey_distance_m
    } else {
        DATUM_GAP_WARN_BASE_M
    }
}

/// How far a datum's GPS position is from where the survey puts its station,
/// in the frame's local metres. Backend-owned so every UI readout agrees.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "bindings/DatumGap.ts")]
pub struct DatumGap {
    /// Horizontal distance from the surveyed position to the GPS position.
    pub gap_m: f64,
    /// Bearing of that vector, degrees clockwise from grid north, in [0, 360).
    pub bearing_deg: f64,
    /// Survey distance from this station to the nearest OTHER resolvable datum
    /// in the same connected survey, or `None` when no other datum is reachable.
    pub survey_distance_m: Option<f64>,
    /// [`datum_gap_threshold_m`] for that distance.
    pub threshold_m: f64,
    /// `gap_m > threshold_m`.
    pub exceeds_threshold: bool,
}

/// What setting a zero datum on a station would do, for the Edit Station dialog
/// to show before the user commits (#741).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "bindings/ZeroDatumPreview.ts")]
pub struct ZeroDatumPreview {
    /// The role the datum would have once stored.
    pub role: DatumRole,
    /// For a [`DatumRole::GpsFix`], how far the GPS is from the surveyed
    /// position. `None` for a placement datum (it moves its group onto the GPS,
    /// so there is no disagreement to show) or when there is no frame yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub gap: Option<DatumGap>,
}

impl Project {
    /// What setting a zero datum with `gps` on `station_ref` would do. A station
    /// that already holds a datum keeps its current role; otherwise the new datum
    /// is a fix exactly when its merge component already has a placement datum.
    /// `None` when the station does not exist.
    pub fn preview_zero_datum(
        &self,
        station_ref: StationRef,
        gps: &GpsCoordinates,
    ) -> Option<ZeroDatumPreview> {
        if !self.station_exists(station_ref) {
            return None;
        }
        let existing = self
            .zero_datums
            .iter()
            .zip(self.datum_roles())
            .find(|(zd, _)| self.resolve_key(&zd.station_ref) == Some(station_ref))
            .and_then(|(_, role)| role);
        let role = existing.unwrap_or_else(|| {
            if self.component_datum_refs(station_ref.section_id).is_empty() {
                DatumRole::Placement
            } else {
                DatumRole::GpsFix
            }
        });
        let gap = match role {
            DatumRole::GpsFix => self.datum_gap(station_ref, gps),
            DatumRole::Placement => None,
        };
        Some(ZeroDatumPreview { role, gap })
    }

    /// Every datum's role, parallel to `zero_datums`; `None` for a datum that
    /// does not resolve (a dangling datum anchors nothing).
    pub fn datum_roles(&self) -> Vec<Option<DatumRole>> {
        let mut placed: HashSet<SectionId> = HashSet::new();
        self.zero_datums
            .iter()
            .map(|zd| {
                let sr = self.resolve_key(&zd.station_ref)?;
                if placed.contains(&sr.section_id) {
                    Some(DatumRole::GpsFix)
                } else {
                    placed.extend(self.get_merged_section_ids(sr.section_id));
                    Some(DatumRole::Placement)
                }
            })
            .collect()
    }

    /// The role `zd` has in the current project, or `None` if it dangles or is
    /// not in the list.
    pub fn datum_role(&self, zd: &ZeroDatum) -> Option<DatumRole> {
        let i = self
            .zero_datums
            .iter()
            .position(|d| d.station_ref == zd.station_ref)?;
        self.datum_roles().get(i).copied().flatten()
    }

    /// The placement datums, in list order. At most one per merge component.
    pub fn placement_datums(&self) -> Vec<&ZeroDatum> {
        self.datums_with_role(DatumRole::Placement)
    }

    /// The GPS-fix datums, in list order.
    pub fn gps_fix_datums(&self) -> Vec<&ZeroDatum> {
        self.datums_with_role(DatumRole::GpsFix)
    }

    fn datums_with_role(&self, role: DatumRole) -> Vec<&ZeroDatum> {
        self.zero_datums
            .iter()
            .zip(self.datum_roles())
            .filter(|(_, r)| *r == Some(role))
            .map(|(zd, _)| zd)
            .collect()
    }

    /// Resolved stations of the placement datums, in list order.
    pub fn placement_datum_refs(&self) -> Vec<StationRef> {
        self.placement_datums()
            .into_iter()
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .collect()
    }

    /// Sections that hold a placement datum on one of their own stations: the
    /// sections the raw-geometry solvers treat as fixed. A section holding only
    /// GPS fixes is NOT in it, because a fix never moves raw geometry.
    pub(crate) fn placement_datum_sections(&self) -> HashSet<SectionId> {
        self.placement_datum_refs()
            .into_iter()
            .map(|sr| sr.section_id)
            .collect()
    }

    /// List index of the placement datum of `section_id`'s merge component (the
    /// first datum in list order that resolves into it), or `None` when the
    /// component holds no resolvable datum. Comparing two of these decides which
    /// of two placed groups keeps its placement when they are merged.
    pub(crate) fn placement_datum_index(&self, section_id: SectionId) -> Option<usize> {
        let merged = self.get_merged_section_ids(section_id);
        self.zero_datums.iter().position(|zd| {
            self.resolve_key(&zd.station_ref)
                .is_some_and(|sr| merged.contains(&sr.section_id))
        })
    }

    /// Resolved stations of every resolvable datum, placement and fix alike, in
    /// list order. For consumers that treat every datum as an entrance.
    pub fn all_datum_refs(&self) -> Vec<StationRef> {
        self.zero_datums
            .iter()
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .collect()
    }

    /// The gap between `gps` and where the RAW survey puts `station_ref`, in the
    /// current frame. `None` when the survey has no frame yet, or the station
    /// does not exist.
    ///
    /// The survey distance is to the nearest OTHER resolvable datum (placement
    /// or fix), along the survey graph, so it works both for a datum that is
    /// already stored and for one that is only being previewed.
    pub fn datum_gap(&self, station_ref: StationRef, gps: &GpsCoordinates) -> Option<DatumGap> {
        let frame = self.frame_datum()?;
        let surveyed = self.compute_station_raw_coords(station_ref)?;
        let (gx, gy) = gps_to_local(gps, &frame.gps);
        let (dx, dy) = (gx - surveyed.x, gy - surveyed.y);
        let gap_m = dx.hypot(dy);
        let bearing_deg = dx.atan2(dy).to_degrees().rem_euclid(360.0);

        let survey_distance_m = self.survey_distance_to_other_datum(station_ref);
        let threshold_m = datum_gap_threshold_m(survey_distance_m.unwrap_or(f64::INFINITY));
        Some(DatumGap {
            gap_m,
            bearing_deg,
            survey_distance_m,
            threshold_m,
            exceeds_threshold: gap_m > threshold_m,
        })
    }

    /// Gaps for every GPS-fix datum, keyed by its resolved station.
    pub fn gps_fix_gaps(&self) -> HashMap<StationRef, DatumGap> {
        self.gps_fix_datums()
            .into_iter()
            .filter_map(|zd| {
                let sr = self.resolve_key(&zd.station_ref)?;
                Some((sr, self.datum_gap(sr, &zd.gps)?))
            })
            .collect()
    }

    /// Survey distance from `station_ref` to the nearest other resolvable datum.
    fn survey_distance_to_other_datum(&self, station_ref: StationRef) -> Option<f64> {
        let stations = self.compute_station_coordinates();
        let index = Self::station_index_map(&stations);
        let roots: Vec<usize> = self
            .all_datum_refs()
            .into_iter()
            .filter(|sr| *sr != station_ref)
            .filter_map(|sr| index.get(&sr).copied())
            .collect();
        let at = *index.get(&station_ref)?;
        let d = *self.compute_distances_from(&stations, &roots).get(at)?;
        d.is_finite().then_some(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_15_m_plus_2_percent() {
        assert!((datum_gap_threshold_m(1400.0) - 43.0).abs() < 1e-9);
        assert!((datum_gap_threshold_m(0.0) - 15.0).abs() < 1e-9);
        assert!((datum_gap_threshold_m(f64::INFINITY) - 15.0).abs() < 1e-9);
        assert!((datum_gap_threshold_m(f64::NAN) - 15.0).abs() < 1e-9);
    }
}
