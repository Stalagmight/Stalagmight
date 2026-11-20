//! What the map image renderer reads, whichever model it comes from.
//!
//! `image::render_svg` draws a [`Sketch`]: the drawn lines (each shot's kind,
//! readings and comment), one wire `Station` per drawn slot (position,
//! Station ID, name, comment, datum star), and the slot pairs that are one
//! physical point (the colocation the segments, labels and junction trims
//! are built on). Distances it measures through a [`Measure`]. The legacy
//! adapter ([`Sketch::from_legacy`], [`LegacyMeasure`]) reads
//! `models::project`; the station-graph one is `survey::image`.

use std::collections::HashMap;

use crate::models::{LineId, Project, ShotKind, Station, StationRef};

use super::image_scope;

/// One drawn shot: what the renderer reads of it.
#[derive(Debug, Clone)]
pub(super) struct SketchShot {
    pub(super) kind: ShotKind,
    pub(super) length: f64,
    pub(super) azimuth_in: f64,
    pub(super) depth_in: f64,
    pub(super) depth_out: f64,
    pub(super) comment: Option<String>,
}

/// One drawn line, in list order; shot `i` runs from slot `i` to slot `i + 1`.
#[derive(Debug, Clone)]
pub(super) struct SketchLine {
    pub(super) id: LineId,
    pub(super) name: Option<String>,
    pub(super) color: String,
    pub(super) shots: Vec<SketchShot>,
}

/// Everything the renderer draws.
#[derive(Debug, Clone)]
pub(super) struct Sketch {
    pub(super) lines: Vec<SketchLine>,
    /// One entry per drawn slot.
    pub(super) stations: Vec<Station>,
    /// Slot pairs that are one physical point (a merge or a junction, then
    /// the two ends of an aligned loop closure).
    pub(super) colocated: Vec<(StationRef, StationRef)>,
}

/// The distances the renderer colours and labels by.
pub(super) trait Measure {
    /// The along-cave penetration distance of each of `drawn` (infinite where
    /// unreachable), measured along the whole survey.
    fn penetration_distances(&self, drawn: &[Station], include_reference: bool) -> Vec<f64>;
    /// The distance-ramp field by slot, and its farthest reachable distance.
    fn distance_field(&self, include_reference: bool) -> (HashMap<StationRef, Option<f64>>, f64);
    /// Whether there is a datum to measure from: `drawn_datums`, or one on a
    /// line the export does not draw.
    fn has_datum(&self, drawn_datums: bool) -> bool;
}

impl SketchShot {
    fn from_legacy(shot: &crate::models::Shot) -> Self {
        Self {
            kind: shot.kind,
            length: shot.length,
            azimuth_in: shot.azimuth_in,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
            comment: shot.comment.clone(),
        }
    }
}

impl Sketch {
    /// The legacy model's lines, the given stations, and its merges and
    /// aligned closures as the colocated pairs.
    pub(super) fn from_legacy(project: &Project, stations: Vec<Station>) -> Self {
        let lines = project
            .lines
            .iter()
            .map(|line| SketchLine {
                id: line.id,
                name: line.name.clone(),
                color: line.color.clone(),
                shots: line.shots.iter().map(SketchShot::from_legacy).collect(),
            })
            .collect();
        let merges = project.station_merges.iter().filter_map(|merge| {
            Some((
                project.resolve_key(&merge.station_a)?,
                project.resolve_key(&merge.station_b)?,
            ))
        });
        let aligned = project
            .loop_closures
            .iter()
            .filter(|closure| closure.is_aligned)
            .filter_map(|closure| {
                Some((
                    project.resolve_key(&closure.station_a)?,
                    project.resolve_key(&closure.station_b)?,
                ))
            });
        Self {
            lines,
            stations,
            colocated: merges.chain(aligned).collect(),
        }
    }
}

/// The legacy model's distances: `drawn` is the project the sketch draws,
/// `whole` the whole survey when that is more (hidden Sections).
pub(super) struct LegacyMeasure<'a> {
    pub(super) drawn: &'a Project,
    pub(super) whole: image_scope::Whole<'a>,
}

impl Measure for LegacyMeasure<'_> {
    fn penetration_distances(&self, drawn: &[Station], include_reference: bool) -> Vec<f64> {
        image_scope::penetration_distances(self.drawn, self.whole, drawn, include_reference)
    }

    fn distance_field(&self, include_reference: bool) -> (HashMap<StationRef, Option<f64>>, f64) {
        image_scope::distance_field(self.drawn, self.whole, include_reference)
    }

    fn has_datum(&self, drawn_datums: bool) -> bool {
        image_scope::has_datum(drawn_datums, self.whole)
    }
}
