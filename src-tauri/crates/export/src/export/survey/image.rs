//! The map image (SVG / PNG / PDF) and its Levels histogram on the
//! station-graph model: the renderer (`super::super::image`) fed a sketch
//! read off the station graph.
//!
//! - One wire station per drawn slot, with the shared Station ID and name, so
//!   a junction's slots label as one point. The comment sits on the
//!   station's first drawn slot only and the datum star on the datum's slot
//!   (its `via` line's, else the first drawn one), so each is drawn once per
//!   station.
//! - Colocated slots: every junction's drawn slots, then the two ends of each
//!   closure aligned in this state (`closure_alignment` on the raw layout).
//! - Distances along the whole survey (hidden Sections included), from the
//!   station graph.
//! - The depth histogram counts each station once.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::models::survey::{Project, RawPositions, StationPositions};
use crate::models::{Coordinates, LineId, Station, StationRef};

use super::super::color_scale::{bin_values, ExportValueKind, ValueHistogram};
use super::super::image::{depth_extent, render_svg, write_image, ExportImageOptions};
use super::super::image_sketch::{Measure, Sketch, SketchLine, SketchShot};

/// Render the map from the measured (raw) positions.
pub fn generate_svg_string(project: &Project, options: &ExportImageOptions) -> Result<String> {
    let raw = project.raw_positions();
    generate_svg_string_with_positions(project, options, &raw, &raw)
}

/// Render the map from `positions` (the commands pass the Closures Computed
/// View, `Project::adjusted_positions`, so the file matches the map). `raw`
/// is the measured layout of the same state, which the render's own
/// derivations read (the closures aligned in it, the distance ramp); the
/// commands pass the memoised one, so an export lays the survey out once.
pub fn generate_svg_string_with_positions(
    project: &Project,
    options: &ExportImageOptions,
    positions: &impl StationPositions,
    raw: &RawPositions,
) -> Result<String> {
    let sketch = sketch(project, options.hidden_line_ids.as_deref(), positions, raw);
    Ok(render_svg(sketch, &GraphMeasure { project, raw }, options))
}

/// Write the map from the measured positions to `path` in `options.format`.
pub fn export_image(project: &Project, path: &Path, options: &ExportImageOptions) -> Result<()> {
    let raw = project.raw_positions();
    export_image_with_positions(project, path, options, &raw, &raw)
}

/// Write the map from `positions` to `path` in `options.format` (`raw` as
/// for [`generate_svg_string_with_positions`]).
pub fn export_image_with_positions(
    project: &Project,
    path: &Path,
    options: &ExportImageOptions,
    positions: &impl StationPositions,
    raw: &RawPositions,
) -> Result<()> {
    write_image(
        &generate_svg_string_with_positions(project, options, positions, raw)?,
        path,
        options,
    )
}

/// Bin counts for the export's Levels slider: station depths in `positions`
/// (each station once) over the extent the render normalises across, or the
/// distance from the entrances over the measured network `raw` (each station
/// once).
pub fn export_value_histogram(
    project: &Project,
    positions: &impl StationPositions,
    raw: &RawPositions,
    kind: ExportValueKind,
    bins: usize,
    include_reference_legs: bool,
) -> ValueHistogram {
    let topology = project.topology();
    let ((min, max), counts) = match kind {
        ExportValueKind::Depth => {
            // The render's extent: every slot's depth (its line's hand z
            // included) and every shot reading.
            let slot_depths = project.lines().iter().flat_map(|line| {
                line.stations()
                    .filter_map(|s| Some(positions.station_z(s)? + line.hand(s)[2]))
            });
            let extent = depth_extent(
                slot_depths,
                project
                    .lines()
                    .iter()
                    .flat_map(|l| l.shots.iter().map(|s| (s.depth_in, s.depth_out))),
            );
            let depths = topology
                .stations()
                .iter()
                .filter_map(|&s| positions.station_z(s));
            (extent, bin_values(depths, extent, bins))
        }
        ExportValueKind::Distance => {
            let field = project.distance_field(raw, None, include_reference_legs);
            let max = field.max_reachable();
            let extent = (0.0, if max > 0.0 { max } else { 1.0 });
            let reachable = field.iter().map(|(_, d)| d).filter(|d| d.is_finite());
            (extent, bin_values(reachable, extent, bins))
        }
    };
    ValueHistogram { min, max, counts }
}

/// What the renderer draws of `project` at `positions`, without the lines in
/// `hidden` (`raw`: the measured layout, for the closures aligned in it).
fn sketch(
    project: &Project,
    hidden: Option<&[LineId]>,
    positions: &impl StationPositions,
    raw: &RawPositions,
) -> Sketch {
    let hidden: HashSet<LineId> = hidden.unwrap_or_default().iter().copied().collect();
    let drawn = |slot: &StationRef| !hidden.contains(&slot.line_id);
    let topology = project.topology();
    let first_drawn = |station| topology.slots_of(station).iter().copied().find(drawn);
    // The slot of a station on line `via` when drawn, else its first drawn one.
    let end_slot = |station, via: Option<LineId>| {
        via.and_then(|line| topology.slot_on(station, line))
            .filter(drawn)
            .or_else(|| first_drawn(station))
    };

    let mut stars: HashMap<StationRef, crate::models::GpsCoordinates> = HashMap::new();
    for datum in &project.datums {
        if let Some(slot) = end_slot(datum.station, datum.via) {
            stars.entry(slot).or_insert(datum.gps);
        }
    }

    let mut lines = Vec::new();
    let mut stations = Vec::new();
    for line in project.lines().iter().filter(|l| !hidden.contains(&l.id)) {
        lines.push(SketchLine {
            id: line.id,
            name: line.name.clone(),
            color: line.color.clone(),
            shots: line
                .shots
                .iter()
                .map(|shot| SketchShot {
                    kind: shot.kind,
                    length: shot.length,
                    azimuth_in: shot.azimuth_in,
                    depth_in: shot.depth_in,
                    depth_out: shot.depth_out,
                    comment: shot.comment.clone(),
                })
                .collect(),
        });
        for (index, id) in line.stations().enumerate() {
            let (Some([x, y]), Some(z)) = (positions.station_xy(id), positions.station_z(id))
            else {
                continue;
            };
            let slot = StationRef::new(line.id, index as u32);
            let z = z + line.hand(id)[2];
            let mut wire = Station::with_coordinates(slot, Coordinates::new(x, y, z));
            wire.id = id;
            if let Some(station) = project.station(id) {
                // The name this slot's line shows (I-3): the representative
                // slot then labels a junction as the map does.
                wire.name = project
                    .name_on(id, line.id)
                    .map_or_else(|| station.name.clone(), str::to_string);
                if first_drawn(id) == Some(slot) {
                    wire.comment = station.comment.clone().filter(|c| !c.is_empty());
                }
            }
            if let Some(gps) = stars.get(&slot) {
                wire.is_zero_datum = true;
                wire.gps_coordinates = Some(*gps);
            }
            stations.push(wire);
        }
    }

    let mut colocated = Vec::new();
    for &station in topology.stations() {
        let mut slots = topology.slots_of(station).iter().copied().filter(drawn);
        if let Some(first) = slots.next() {
            colocated.extend(slots.map(|other| (first, other)));
        }
    }
    let aligned = project.closure_alignment(raw);
    for (closure, _) in project.closures.iter().zip(aligned).filter(|(_, a)| *a) {
        if let (Some(a), Some(b)) = (
            end_slot(closure.a, closure.a_via),
            end_slot(closure.b, closure.b_via),
        ) {
            colocated.push((a, b));
        }
    }
    Sketch {
        lines,
        stations,
        colocated,
    }
}

/// Distances on the station graph, along the whole survey (the distance
/// ramp weighs shots by the measured layout `raw`, #34).
struct GraphMeasure<'a> {
    project: &'a Project,
    raw: &'a RawPositions,
}

impl Measure for GraphMeasure<'_> {
    fn penetration_distances(&self, drawn: &[Station], include_reference: bool) -> Vec<f64> {
        let distances = self.project.penetration_distances(include_reference);
        drawn
            .iter()
            .map(|s| distances.get(s.id).unwrap_or(f64::INFINITY))
            .collect()
    }

    fn distance_field(&self, include_reference: bool) -> (HashMap<StationRef, Option<f64>>, f64) {
        let field = self
            .project
            .distance_field(self.raw, None, include_reference);
        let mut by_slot = HashMap::new();
        for line in self.project.lines() {
            for (index, station) in line.stations().enumerate() {
                by_slot.insert(StationRef::new(line.id, index as u32), field.get(station));
            }
        }
        (by_slot, field.max_reachable())
    }

    fn has_datum(&self, drawn_datums: bool) -> bool {
        drawn_datums || !self.project.datums.is_empty()
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
#[path = "image_tests.rs"]
mod tests;
