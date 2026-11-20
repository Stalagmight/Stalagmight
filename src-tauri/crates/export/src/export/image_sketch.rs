//! What the map image renderer reads.
//!
//! `image::render_svg` draws a [`Sketch`]: the drawn lines (each shot's kind,
//! readings and comment), one wire `Station` per drawn slot (position,
//! Station ID, name, comment, datum star), and the slot pairs that are one
//! physical point (the colocation the segments, labels and junction trims
//! are built on). Distances it measures through a [`Measure`]. The
//! station-graph exporter (`survey::image`) builds both.

use std::collections::HashMap;

use crate::models::{LineId, ShotKind, Station, StationRef};

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
