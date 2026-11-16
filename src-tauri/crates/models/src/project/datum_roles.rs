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

/// Part of the gap warning threshold that grows with the survey distance from
/// the datum's placement datum: 2% (#741 decision 4).
pub const DATUM_GAP_WARN_FRACTION: f64 = 0.02;

/// The gap above which a datum's GPS and its surveyed position disagree enough
/// to warn: 15 m plus 2% of the survey distance from the placement datum of
/// the datum's piece. An infinite or NaN distance (none reachable) yields the
/// base.
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
    /// Survey distance, along shots and merges, from the placement datum of
    /// this station's merge component (the datum its raw position is carried
    /// from), or `None` when that component has no placement datum.
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
    /// Whether the datum would count as an entrance: the stored answer for a
    /// station that already holds a datum, else the default rule for `role`
    /// (`Project::datum_is_entrance`), so the dialog's Entrance checkbox
    /// starts from the backend's answer.
    pub is_entrance: bool,
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
        let stored = self
            .zero_datums
            .iter()
            .zip(self.datum_roles())
            .find(|(zd, _)| self.resolve_key(&zd.station_ref) == Some(station_ref));
        let existing = stored.and_then(|(_, role)| role);
        let override_entrance = stored.and_then(|(zd, _)| zd.entrance);
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
        let is_entrance = self.datum_is_entrance(station_ref, role, override_entrance);
        Some(ZeroDatumPreview {
            role,
            gap,
            is_entrance,
        })
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
    /// list order. Entrance readers use [`Self::entrance_datum_refs`] instead.
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
    /// The survey distance is measured from the PLACEMENT datum of the
    /// station's merge component: the raw position is carried from that datum,
    /// so that is the survey whose accumulated error the threshold allows for.
    /// It works both for a datum that is already stored and for one that is
    /// only being previewed.
    ///
    /// One station's worth of work (one graph build and one Dijkstra), for the
    /// Edit Station preview. Every fix at once goes through
    /// [`Self::gps_fix_gaps`], which shares that one pass between them.
    pub fn datum_gap(&self, station_ref: StationRef, gps: &GpsCoordinates) -> Option<DatumGap> {
        let frame_gps = self.frame_datum()?.gps;
        let surveyed = self.compute_station_raw_coords(station_ref)?;
        let distances = self.survey_distances_from_placements();
        Some(DatumGap::between(
            (surveyed.x, surveyed.y),
            gps,
            &frame_gps,
            distances.to(station_ref),
        ))
    }

    /// Gaps for every GPS-fix datum, keyed by its resolved station.
    ///
    /// Runs on every project open, so its cost must not multiply with the
    /// number of fixes: the graph is built once, and one multi-source Dijkstra
    /// from every placement datum answers every fix at once.
    pub fn gps_fix_gaps(&self) -> HashMap<StationRef, DatumGap> {
        let Some(frame_gps) = self.frame_datum().map(|zd| zd.gps) else {
            return HashMap::new();
        };
        let fixes: Vec<(StationRef, GpsCoordinates)> = self
            .gps_fix_datums()
            .into_iter()
            .filter_map(|zd| Some((self.resolve_key(&zd.station_ref)?, zd.gps)))
            .collect();
        if fixes.is_empty() {
            return HashMap::new();
        }
        let distances = self.survey_distances_from_placements();
        fixes
            .into_iter()
            .filter_map(|(sr, gps)| {
                let surveyed = self.compute_station_raw_coords(sr)?;
                Some((
                    sr,
                    DatumGap::between((surveyed.x, surveyed.y), &gps, &frame_gps, distances.to(sr)),
                ))
            })
            .collect()
    }

    /// Every station's survey distance from the placement datum of its merge
    /// component, in one multi-source Dijkstra.
    ///
    /// The graph holds shots and merges only, the edges the raw position is
    /// carried along: no edge leaves a merge component, so each station is
    /// reached from its own component's placement datum and from no other, and
    /// a loop closure (a measured misclosure, not a carried position) is no
    /// shortcut.
    fn survey_distances_from_placements(&self) -> PlacementDistances {
        let stations = self.compute_station_coordinates();
        let index = Self::station_index_map(&stations);
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); stations.len()];
        let mut link = |a: StationRef, b: StationRef, w: f64| {
            if let (Some(&i), Some(&j)) = (index.get(&a), index.get(&b)) {
                adj[i].push((j, w));
                adj[j].push((i, w));
            }
        };
        for section in &self.sections {
            for (i, shot) in (0u32..).zip(&section.shots) {
                // A non-finite length is never crossed (`dijkstra`).
                let w = if shot.length.is_finite() {
                    shot.length.max(0.0)
                } else {
                    f64::INFINITY
                };
                link(
                    StationRef::new(section.id, i),
                    StationRef::new(section.id, i + 1),
                    w,
                );
            }
        }
        for merge in &self.station_merges {
            if let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                link(a, b, 0.0);
            }
        }
        let roots: Vec<usize> = self
            .placement_datum_refs()
            .into_iter()
            .filter_map(|sr| index.get(&sr).copied())
            .collect();
        let distance = Self::dijkstra(&adj, &roots);
        PlacementDistances { index, distance }
    }
}

/// What [`Project::survey_distances_from_placements`] found.
struct PlacementDistances {
    index: HashMap<StationRef, usize>,
    distance: Vec<f64>,
}

impl PlacementDistances {
    /// The distance to `station_ref`, or `None` when its piece has no placement
    /// datum (or the station does not exist).
    fn to(&self, station_ref: StationRef) -> Option<f64> {
        let d = *self.distance.get(*self.index.get(&station_ref)?)?;
        d.is_finite().then_some(d)
    }
}

impl DatumGap {
    /// The gap from a station `surveyed` at local (x, y) to `gps`, in the frame
    /// whose origin is `frame_gps`.
    fn between(
        surveyed: (f64, f64),
        gps: &GpsCoordinates,
        frame_gps: &GpsCoordinates,
        survey_distance_m: Option<f64>,
    ) -> Self {
        let (gx, gy) = gps_to_local(gps, frame_gps);
        let (dx, dy) = (gx - surveyed.0, gy - surveyed.1);
        let gap_m = dx.hypot(dy);
        let threshold_m = datum_gap_threshold_m(survey_distance_m.unwrap_or(f64::INFINITY));
        Self {
            gap_m,
            bearing_deg: dx.atan2(dy).to_degrees().rem_euclid(360.0),
            survey_distance_m,
            threshold_m,
            exceeds_threshold: gap_m > threshold_m,
        }
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
