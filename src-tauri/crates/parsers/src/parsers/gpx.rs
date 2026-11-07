//! GPX (GPS Exchange Format) file parser.
//!
//! Parses GPX 1.0 and 1.1 files containing waypoints, tracks, and routes.
//! Converts GPS data into import-ready section configurations.

use std::path::Path;

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::geo::{haversine_distance, initial_bearing};
use crate::models::{
    AzimuthReference, CorrectionSource, GpsCoordinates, ImportPlacement, ImportSectionConfig,
    ShotData, ShotKind,
};

/// GPX bearings are derived from coordinates (`initial_bearing`), so they are
/// TRUE north by construction - no instrument, no declination (#438). Declaring
/// it keeps a GPX track in the same frame as a corrected magnetic survey rather
/// than inheriting a project default that would rotate it.
const GPX_AZIMUTH_REFERENCE: AzimuthReference = AzimuthReference {
    north: crate::models::NorthReference::True,
    correction: 0.0,
    source: CorrectionSource::FileDeclared,
};
use crate::parsers::encoding::{read_utf8, FileFamily};

// ============================================================================
// IPC-visible types
// ============================================================================

/// Parsed GPX file.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct GpxFile {
    pub name: Option<String>,
    pub waypoints: Vec<GpxWaypoint>,
    pub tracks: Vec<GpxTrack>,
    pub routes: Vec<GpxRoute>,
}

/// A GPX waypoint.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct GpxWaypoint {
    pub name: Option<String>,
    pub comment: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub elevation: Option<f64>,
    pub time: Option<String>,
}

/// A GPX track containing one or more segments.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct GpxTrack {
    pub name: Option<String>,
    pub comment: Option<String>,
    pub segments: Vec<GpxTrackSegment>,
}

/// A single track segment (contiguous sequence of trackpoints).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct GpxTrackSegment {
    pub points: Vec<GpxTrackPoint>,
}

/// A trackpoint within a track segment.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct GpxTrackPoint {
    pub latitude: f64,
    pub longitude: f64,
    pub elevation: Option<f64>,
    pub time: Option<String>,
}

/// A GPX route.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct GpxRoute {
    pub name: Option<String>,
    pub comment: Option<String>,
    pub points: Vec<GpxRoutePoint>,
}

/// A routepoint within a route.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct GpxRoutePoint {
    pub name: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub elevation: Option<f64>,
    pub time: Option<String>,
}

/// Per-section placement for GPX import (tracks/routes from Step 2).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct GpxSectionPlacement {
    pub section_index: u32,
    pub placement: ImportPlacement,
    pub name: Option<String>,
    pub color: String,
}

// ============================================================================
// Parser
// ============================================================================

/// Strip XML namespace prefix from a tag name (e.g. "gpx:trk" → "trk").
fn strip_ns(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Get the local tag name from a quick-xml event, stripping any namespace prefix.
fn local_name(e: &quick_xml::events::BytesStart<'_>) -> Vec<u8> {
    let qname = e.name();
    strip_ns(qname.as_ref()).to_vec()
}

/// The XML reader every GPX loop reads through once `<gpx>` has opened, so the
/// document's end is judged in one place (#576 B14). quick-xml reports a
/// document that simply stops as a clean end of file, so a GPX cut off by a
/// dying GPS or a partial download read as a shorter, complete track, and a
/// number cut in half (352.4 cut to 35) became a real reading. Any end of
/// file while an element is still open fails the parse instead, as the
/// Ariane reader does (`tml::parse_xml_content`).
struct GpxReader<'a> {
    inner: Reader<&'a [u8]>,
    /// Elements opened and not yet closed, `<gpx>` included.
    open: u32,
}

impl GpxReader<'_> {
    /// The next event, or an error for malformed XML or a document that
    /// ends with elements still open.
    fn next_event<'b>(&mut self, buf: &'b mut Vec<u8>) -> Result<Event<'b>> {
        match self.inner.read_event_into(buf) {
            Ok(event @ Event::Start(_)) => {
                self.open += 1;
                Ok(event)
            }
            Ok(event @ Event::End(_)) => {
                self.open = self.open.saturating_sub(1);
                Ok(event)
            }
            Ok(Event::Eof) if self.open > 0 => bail!(
                "The GPX ends at byte {} with {} element(s) still open - the file appears \
                 truncated or damaged. Export the track from the device or app again.",
                self.inner.buffer_position(),
                self.open
            ),
            Ok(event) => Ok(event),
            Err(e) => bail!("XML parse error: {e}"),
        }
    }
}

/// Parse a GPX file from the given path.
pub fn parse_gpx(path: &Path) -> Result<GpxFile> {
    parse_gpx_str(&read_utf8(path, FileFamily::Xml)?)
}

/// Parse GPX from a string (for testing).
pub fn parse_gpx_str(xml: &str) -> Result<GpxFile> {
    let mut xml_reader = Reader::from_str(xml);

    let mut gpx = GpxFile {
        name: None,
        waypoints: Vec::new(),
        tracks: Vec::new(),
        routes: Vec::new(),
    };

    let mut buf = Vec::new();

    // Find the <gpx> root element
    loop {
        match xml_reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = local_name(e);
                if tag == b"gpx" {
                    break;
                }
            }
            Ok(Event::Eof) => bail!("Not a valid GPX file: no <gpx> root element found"),
            Err(e) => bail!("XML parse error: {e}"),
            _ => {}
        }
        buf.clear();
    }

    buf.clear();
    // From here on every event goes through the reader that counts what is
    // open, starting with <gpx> itself (#576 B14).
    let mut reader = GpxReader {
        inner: xml_reader,
        open: 1,
    };

    // Parse children of <gpx>
    let mut depth = 1u32;
    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(ref e) => {
                let tag = local_name(e);
                // Extract lat/lon now while `e` is still alive
                let lat_lon = if tag == b"wpt" || tag == b"trkpt" || tag == b"rtept" {
                    Some(parse_lat_lon(e)?)
                } else {
                    None
                };
                depth += 1;
                match tag.as_slice() {
                    b"metadata" => {
                        gpx.name = parse_metadata_name(&mut reader)?;
                        depth -= 1;
                    }
                    b"wpt" => {
                        let Some((lat, lon)) = lat_lon else {
                            bail!("<wpt> element missing lat/lon attributes");
                        };
                        gpx.waypoints
                            .push(parse_waypoint_children(lat, lon, &mut reader)?);
                        depth -= 1;
                    }
                    b"trk" => {
                        gpx.tracks.push(parse_track(&mut reader)?);
                        depth -= 1;
                    }
                    b"rte" => {
                        gpx.routes.push(parse_route(&mut reader)?);
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            Event::Empty(ref e) => {
                let tag = local_name(e);
                if tag == b"wpt" {
                    let (lat, lon) = parse_lat_lon(e)?;
                    gpx.waypoints.push(GpxWaypoint {
                        name: None,
                        comment: None,
                        latitude: lat,
                        longitude: lon,
                        elevation: None,
                        time: None,
                    });
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(gpx)
}

/// Parse lat/lon attributes from a waypoint-like element.
fn parse_lat_lon(e: &quick_xml::events::BytesStart<'_>) -> Result<(f64, f64)> {
    let mut lat = None;
    let mut lon = None;

    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"lat" => {
                lat = Some(
                    std::str::from_utf8(&attr.value)
                        .context("Invalid UTF-8 in lat attribute")?
                        .parse::<f64>()
                        .context("Invalid lat value")?,
                );
            }
            b"lon" => {
                lon = Some(
                    std::str::from_utf8(&attr.value)
                        .context("Invalid UTF-8 in lon attribute")?
                        .parse::<f64>()
                        .context("Invalid lon value")?,
                );
            }
            _ => {}
        }
    }

    let lat = lat.context("Missing lat attribute")?;
    let lon = lon.context("Missing lon attribute")?;

    if !(-90.0..=90.0).contains(&lat) {
        bail!("Latitude {lat} out of range [-90, 90]");
    }
    if !(-180.0..=180.0).contains(&lon) {
        bail!("Longitude {lon} out of range [-180, 180]");
    }

    Ok((lat, lon))
}

/// Read the text content of the current element (between start and end tags).
fn read_text(reader: &mut GpxReader<'_>) -> Result<String> {
    let mut buf = Vec::new();
    let mut text = String::new();
    loop {
        match reader.next_event(&mut buf)? {
            Event::Text(ref e) => {
                text.push_str(&e.unescape().context("Invalid text content")?);
            }
            Event::End(_) | Event::Eof => break,
            Event::Start(_) => {
                // Nested element - skip it entirely
                skip_element(reader)?;
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(text)
}

/// Skip past the current element and all its children (after Start has been consumed).
fn skip_element(reader: &mut GpxReader<'_>) -> Result<()> {
    let mut buf = Vec::new();
    let mut depth = 1u32;
    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buf.clear();
    }
}

/// Parse <metadata> and extract the <name> child if present.
fn parse_metadata_name(reader: &mut GpxReader<'_>) -> Result<Option<String>> {
    let mut buf = Vec::new();
    let mut name = None;
    let mut depth = 1u32;

    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(ref e) => {
                depth += 1;
                let tag = local_name(e);
                if tag == b"name" && depth == 2 {
                    let text = read_text(reader)?;
                    depth -= 1; // read_text consumed the end tag
                    if !text.is_empty() {
                        name = Some(text);
                    }
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(name)
}

/// Parse children of a <wpt> element (lat/lon already extracted).
fn parse_waypoint_children(
    latitude: f64,
    longitude: f64,
    reader: &mut GpxReader<'_>,
) -> Result<GpxWaypoint> {
    let mut wpt = GpxWaypoint {
        name: None,
        comment: None,
        latitude,
        longitude,
        elevation: None,
        time: None,
    };

    let mut buf = Vec::new();
    let mut depth = 1u32;

    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(ref e) => {
                depth += 1;
                let tag = local_name(e);
                if depth == 2 {
                    match tag.as_slice() {
                        b"name" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            if !t.is_empty() {
                                wpt.name = Some(t);
                            }
                        }
                        b"cmt" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            if !t.is_empty() && wpt.comment.is_none() {
                                wpt.comment = Some(t);
                            }
                        }
                        b"desc" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            if !t.is_empty() && wpt.comment.is_none() {
                                wpt.comment = Some(t);
                            }
                        }
                        b"ele" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            wpt.elevation = t.parse::<f64>().ok().filter(|v| v.is_finite());
                        }
                        b"time" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            if !t.is_empty() {
                                wpt.time = Some(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(wpt)
}

/// Parse a <trk> element.
fn parse_track(reader: &mut GpxReader<'_>) -> Result<GpxTrack> {
    let mut trk = GpxTrack {
        name: None,
        comment: None,
        segments: Vec::new(),
    };

    let mut buf = Vec::new();
    let mut depth = 1u32;

    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(ref e) => {
                depth += 1;
                let tag = local_name(e);
                match tag.as_slice() {
                    b"name" if depth == 2 => {
                        let t = read_text(reader)?;
                        depth -= 1;
                        if !t.is_empty() {
                            trk.name = Some(t);
                        }
                    }
                    b"cmt" if depth == 2 => {
                        let t = read_text(reader)?;
                        depth -= 1;
                        if !t.is_empty() && trk.comment.is_none() {
                            trk.comment = Some(t);
                        }
                    }
                    b"desc" if depth == 2 => {
                        let t = read_text(reader)?;
                        depth -= 1;
                        if !t.is_empty() && trk.comment.is_none() {
                            trk.comment = Some(t);
                        }
                    }
                    b"trkseg" => {
                        trk.segments.push(parse_track_segment(reader)?);
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(trk)
}

/// Parse a <trkseg> element.
fn parse_track_segment(reader: &mut GpxReader<'_>) -> Result<GpxTrackSegment> {
    let mut seg = GpxTrackSegment { points: Vec::new() };
    let mut buf = Vec::new();
    let mut depth = 1u32;

    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(ref e) => {
                depth += 1;
                let tag = local_name(e);
                if tag == b"trkpt" {
                    let (lat, lon) = parse_lat_lon(e)?;
                    seg.points.push(parse_point_children(lat, lon, reader)?);
                    depth -= 1;
                }
            }
            Event::Empty(ref e) => {
                let tag = local_name(e);
                if tag == b"trkpt" {
                    let (lat, lon) = parse_lat_lon(e)?;
                    seg.points.push(GpxTrackPoint {
                        latitude: lat,
                        longitude: lon,
                        elevation: None,
                        time: None,
                    });
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(seg)
}

/// Parse children of a <trkpt> or <rtept> element, returning a `GpxTrackPoint`.
fn parse_point_children(
    latitude: f64,
    longitude: f64,
    reader: &mut GpxReader<'_>,
) -> Result<GpxTrackPoint> {
    let mut pt = GpxTrackPoint {
        latitude,
        longitude,
        elevation: None,
        time: None,
    };

    let mut buf = Vec::new();
    let mut depth = 1u32;

    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(ref e) => {
                depth += 1;
                let tag = local_name(e);
                if depth == 2 {
                    match tag.as_slice() {
                        b"ele" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            pt.elevation = t.parse::<f64>().ok().filter(|v| v.is_finite());
                        }
                        b"time" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            if !t.is_empty() {
                                pt.time = Some(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(pt)
}

/// Parse a <rte> element.
fn parse_route(reader: &mut GpxReader<'_>) -> Result<GpxRoute> {
    let mut rte = GpxRoute {
        name: None,
        comment: None,
        points: Vec::new(),
    };

    let mut buf = Vec::new();
    let mut depth = 1u32;

    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(ref e) => {
                depth += 1;
                let tag = local_name(e);
                match tag.as_slice() {
                    b"name" if depth == 2 => {
                        let t = read_text(reader)?;
                        depth -= 1;
                        if !t.is_empty() {
                            rte.name = Some(t);
                        }
                    }
                    b"cmt" if depth == 2 => {
                        let t = read_text(reader)?;
                        depth -= 1;
                        if !t.is_empty() && rte.comment.is_none() {
                            rte.comment = Some(t);
                        }
                    }
                    b"desc" if depth == 2 => {
                        let t = read_text(reader)?;
                        depth -= 1;
                        if !t.is_empty() && rte.comment.is_none() {
                            rte.comment = Some(t);
                        }
                    }
                    b"rtept" => {
                        let (lat, lon) = parse_lat_lon(e)?;
                        let pt = parse_route_point_children(lat, lon, reader)?;
                        rte.points.push(pt);
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            Event::Empty(ref e) => {
                let tag = local_name(e);
                if tag == b"rtept" {
                    let (lat, lon) = parse_lat_lon(e)?;
                    rte.points.push(GpxRoutePoint {
                        name: None,
                        latitude: lat,
                        longitude: lon,
                        elevation: None,
                        time: None,
                    });
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(rte)
}

/// Parse children of an <rtept> element, returning a `GpxRoutePoint`.
fn parse_route_point_children(
    latitude: f64,
    longitude: f64,
    reader: &mut GpxReader<'_>,
) -> Result<GpxRoutePoint> {
    let mut pt = GpxRoutePoint {
        name: None,
        latitude,
        longitude,
        elevation: None,
        time: None,
    };

    let mut buf = Vec::new();
    let mut depth = 1u32;

    loop {
        match reader.next_event(&mut buf)? {
            Event::Start(ref e) => {
                depth += 1;
                let tag = local_name(e);
                if depth == 2 {
                    match tag.as_slice() {
                        b"name" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            if !t.is_empty() {
                                pt.name = Some(t);
                            }
                        }
                        b"ele" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            pt.elevation = t.parse::<f64>().ok().filter(|v| v.is_finite());
                        }
                        b"time" => {
                            let t = read_text(reader)?;
                            depth -= 1;
                            if !t.is_empty() {
                                pt.time = Some(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(pt)
}

// ============================================================================
// Conversion to import configs
// ============================================================================

/// Section color palette for imported GPX sections.
const GPX_COLORS: &[&str] = &[
    "#2ecc71", "#3498db", "#9b59b6", "#e67e22", "#1abc9c", "#e74c3c", "#f39c12", "#2980b9",
    "#8e44ad", "#27ae60",
];

/// Every point's elevation with the gaps filled (#576 B34), and how many
/// points had none.
///
/// A missing elevation is interpolated along the track between the nearest
/// points before and after it that have one, by horizontal distance; a gap
/// at the start or end of the track takes the nearest known value. It used
/// to take the FIRST point's elevation, or 0 m when the first point had
/// none, which invented a vertical leg as tall as the track's altitude.
/// Points that coincide horizontally share the span by position instead. A
/// track with no elevation at all stays flat at 0 m and counts nothing: no
/// value was invented for it.
fn filled_elevations(points: &[(f64, f64, Option<f64>)]) -> (Vec<f64>, usize) {
    let known: Vec<(usize, f64)> = points
        .iter()
        .enumerate()
        .filter_map(|(i, (_, _, ele))| ele.map(|e| (i, e)))
        .collect();
    if known.is_empty() {
        return (vec![0.0; points.len()], 0);
    }
    // Horizontal distance from the first point, the interpolation axis.
    let mut along = Vec::with_capacity(points.len());
    let mut total = 0.0;
    for (i, &(lat, lon, _)) in points.iter().enumerate() {
        if let Some(&(prev_lat, prev_lon, _)) = i.checked_sub(1).and_then(|j| points.get(j)) {
            total += haversine_distance(prev_lat, prev_lon, lat, lon);
        }
        along.push(total);
    }
    let mut filled = 0;
    // `known[next]` is the first known point at or after the current one.
    let mut next = 0;
    let elevations = points
        .iter()
        .enumerate()
        .map(|(i, &(_, _, ele))| {
            if let Some(e) = ele {
                return e;
            }
            filled += 1;
            while known.get(next).is_some_and(|&(k, _)| k < i) {
                next += 1;
            }
            let before = next.checked_sub(1).and_then(|k| known.get(k));
            match (before, known.get(next)) {
                (Some(&(b, eb)), Some(&(a, ea))) => {
                    let span = along[a] - along[b];
                    let t = if span > 0.0 {
                        (along[i] - along[b]) / span
                    } else {
                        (i - b) as f64 / (a - b) as f64
                    };
                    eb + (ea - eb) * t
                }
                (Some(&(_, e)), None) | (None, Some(&(_, e))) => e,
                // `known` is not empty, so one side always exists.
                (None, None) => 0.0,
            }
        })
        .collect();
    (elevations, filled)
}

/// Convert GPS point pairs into virtual `ShotData`, and say how many points
/// had their elevation filled in ([`filled_elevations`]).
///
/// Elevation is converted to relative depth (first point = depth 0).
/// Shot length is slope distance incorporating elevation change.
///
/// When `depth_inverted` is false (default): lower elevation = deeper (surface GPS tracks).
/// When `depth_inverted` is true: higher elevation = deeper (elevation stores positive depth).
fn shots_from_points(
    points: &[(f64, f64, Option<f64>)],
    depth_inverted: bool,
) -> (Vec<ShotData>, usize) {
    if points.len() < 2 {
        return (Vec::new(), 0);
    }

    // Depths relative to the first point's (filled) elevation.
    let (elevations, filled) = filled_elevations(points);
    let ref_elev = elevations[0];
    let depths: Vec<f64> = elevations
        .iter()
        .map(|&e| {
            if depth_inverted {
                e - ref_elev
            } else {
                ref_elev - e
            }
        })
        .collect();

    let shots = points
        .windows(2)
        .enumerate()
        .map(|(i, pair)| {
            let (lat1, lon1, _) = pair[0];
            let (lat2, lon2, _) = pair[1];

            let horiz = haversine_distance(lat1, lon1, lat2, lon2);
            let dz = depths[i + 1] - depths[i];
            let length = (horiz * horiz + dz * dz).sqrt();
            let azimuth = initial_bearing(lat1, lon1, lat2, lon2);

            ShotData {
                azimuth_in: azimuth,
                azimuth_out: azimuth,
                length,
                depth_in: depths[i],
                depth_out: depths[i + 1],
                // A GPS track is a surface trace, never a swum cave leg, so it
                // seeds as Reference. The chain classifier can still promote it
                // to a Jump if it turns out to bridge real survey on both ends
                // (#27 q10).
                kind: ShotKind::Reference,
                ..Default::default()
            }
        })
        .collect();
    (shots, filled)
}

/// Convert waypoints into zero-datum import configs.
///
/// Each waypoint becomes a single-station section (0 shots) placed at its GPS coordinates.
pub fn waypoints_to_import_configs(
    waypoints: &[GpxWaypoint],
    start_index: u32,
) -> Result<Vec<ImportSectionConfig>> {
    let mut configs = Vec::new();

    for (i, wpt) in waypoints.iter().enumerate() {
        let idx = start_index + i as u32;
        let color_idx = idx as usize % GPX_COLORS.len();

        let gps = GpsCoordinates::new(wpt.latitude, wpt.longitude, 0.0)
            .map_err(|e| anyhow::anyhow!("Invalid waypoint GPS: {e}"))?;

        let mut station_comments = Vec::new();
        if let Some(ref c) = wpt.comment {
            station_comments.push((0u32, c.clone()));
        }
        if let Some(e) = wpt.elevation {
            let elev_comment = format!("Elev: {e:.1}m");
            if let Some((_idx, existing)) = station_comments.first_mut() {
                existing.push_str(&format!(" | {elev_comment}"));
            } else {
                station_comments.push((0u32, elev_comment));
            }
        }

        configs.push(ImportSectionConfig {
            reverse: false,
            section_index: idx,
            name: wpt.name.clone(),
            surveyor: None,
            date: None,
            color: GPX_COLORS[color_idx].to_string(),
            direction: None,
            quality: 1.0,
            shots: Vec::new(),
            placement: ImportPlacement::ZeroDatum {
                coordinates: gps,
                station_index: 0,
            },
            station_comments,
            azimuth_reference: Some(GPX_AZIMUTH_REFERENCE),
        });
    }

    Ok(configs)
}

/// Convert a track segment into an import config, with the number of points
/// whose elevation was filled in.
fn track_segment_to_config(
    seg: &GpxTrackSegment,
    section_index: u32,
    name: Option<String>,
    color: &str,
    placement: ImportPlacement,
    depth_inverted: bool,
) -> (ImportSectionConfig, usize) {
    let points: Vec<(f64, f64, Option<f64>)> = seg
        .points
        .iter()
        .map(|p| (p.latitude, p.longitude, p.elevation))
        .collect();
    let (shots, filled) = shots_from_points(&points, depth_inverted);

    let config = ImportSectionConfig {
        reverse: false,
        section_index,
        name,
        surveyor: None,
        date: None,
        color: color.to_string(),
        direction: None,
        quality: 1.0,
        shots,
        placement,
        station_comments: Vec::new(),
        azimuth_reference: Some(GPX_AZIMUTH_REFERENCE),
    };
    (config, filled)
}

/// Convert a route into an import config, with the number of points whose
/// elevation was filled in.
fn route_to_config(
    rte: &GpxRoute,
    section_index: u32,
    color: &str,
    placement: ImportPlacement,
    depth_inverted: bool,
) -> (ImportSectionConfig, usize) {
    let points: Vec<(f64, f64, Option<f64>)> = rte
        .points
        .iter()
        .map(|p| (p.latitude, p.longitude, p.elevation))
        .collect();
    let (shots, filled) = shots_from_points(&points, depth_inverted);

    let config = ImportSectionConfig {
        reverse: false,
        section_index,
        name: rte.name.clone(),
        surveyor: None,
        date: None,
        color: color.to_string(),
        direction: None,
        quality: 1.0,
        shots,
        placement,
        station_comments: Vec::new(),
        azimuth_reference: Some(GPX_AZIMUTH_REFERENCE),
    };
    (config, filled)
}

/// Convert a GPX file into import configs.
///
/// Waypoints are always placed as zero-datum sections. Track/route sections
/// use the provided placements (from Step 2 of the import dialog).
///
/// If `placements` is empty, track/route sections default to zero-datum
/// placement at their first station.
///
/// Returns the configs and the converter's warnings for the import receipt:
/// one line counting the track/route points whose missing elevation was
/// filled in (#576 B34), when there are any.
pub fn convert_gpx_to_import_configs(
    gpx: &GpxFile,
    import_waypoints: bool,
    import_tracks: bool,
    import_routes: bool,
    placements: &[GpxSectionPlacement],
    depth_inverted: bool,
) -> Result<(Vec<ImportSectionConfig>, Vec<String>)> {
    let mut configs = Vec::new();
    let mut section_index = 0u32;
    let mut filled_points = 0usize;

    // Waypoints → zero-datum sections
    if import_waypoints {
        let wpt_configs = waypoints_to_import_configs(&gpx.waypoints, section_index)?;
        section_index += wpt_configs.len() as u32;
        configs.extend(wpt_configs);
    }

    // Tracks → sections
    if import_tracks {
        for trk in &gpx.tracks {
            for (seg_i, seg) in trk.segments.iter().enumerate() {
                if seg.points.len() < 2 {
                    section_index += 1;
                    continue;
                }

                let first_pt = seg.points.first();
                let (placement, sec_name, color) =
                    resolve_placement(section_index, placements, first_pt, || {
                        if trk.segments.len() == 1 {
                            trk.name.clone()
                        } else {
                            Some(format!(
                                "{} seg {}",
                                trk.name.as_deref().unwrap_or("Track"),
                                seg_i + 1
                            ))
                        }
                    })?;

                let (config, filled) = track_segment_to_config(
                    seg,
                    section_index,
                    sec_name,
                    &color,
                    placement,
                    depth_inverted,
                );
                configs.push(config);
                filled_points += filled;
                section_index += 1;
            }
        }
    }

    // Routes → sections
    if import_routes {
        for rte in &gpx.routes {
            if rte.points.len() < 2 {
                section_index += 1;
                continue;
            }

            // Convert first route point to a GpxTrackPoint for resolve_placement
            let first_as_trkpt = rte.points.first().map(|p| GpxTrackPoint {
                latitude: p.latitude,
                longitude: p.longitude,
                elevation: p.elevation,
                time: p.time.clone(),
            });

            let (placement, _sec_name, color) =
                resolve_placement(section_index, placements, first_as_trkpt.as_ref(), || {
                    rte.name.clone()
                })?;

            let (config, filled) =
                route_to_config(rte, section_index, &color, placement, depth_inverted);
            configs.push(config);
            filled_points += filled;
            section_index += 1;
        }
    }

    let warnings = match filled_points {
        0 => Vec::new(),
        1 => vec![
            "1 GPS point had no elevation; it was filled in from the nearest points along \
             the track that have one."
                .to_string(),
        ],
        n => vec![format!(
            "{n} GPS points had no elevation; they were filled in from the nearest points \
             along the track that have one."
        )],
    };
    Ok((configs, warnings))
}

/// Resolve placement for a track/route section.
///
/// Uses user-provided placement from Step 2 if available, otherwise falls back
/// to zero-datum at the first point.
fn resolve_placement(
    section_index: u32,
    placements: &[GpxSectionPlacement],
    first_point: Option<&GpxTrackPoint>,
    default_name: impl FnOnce() -> Option<String>,
) -> Result<(ImportPlacement, Option<String>, String)> {
    if let Some(p) = placements.iter().find(|p| p.section_index == section_index) {
        return Ok((p.placement.clone(), p.name.clone(), p.color.clone()));
    }

    // Default: zero datum at first point
    let color_idx = section_index as usize % GPX_COLORS.len();
    let color = GPX_COLORS[color_idx].to_string();
    let name = default_name();

    let placement = if let Some(pt) = first_point {
        let gps = GpsCoordinates::new(pt.latitude, pt.longitude, 0.0)
            .map_err(|e| anyhow::anyhow!("Invalid GPS: {e}"))?;
        ImportPlacement::ZeroDatum {
            coordinates: gps,
            station_index: 0,
        }
    } else {
        ImportPlacement::Origin
    };

    Ok((placement, name, color))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]
mod tests {
    use super::*;

    const SIMPLE_GPX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test"
     xmlns="http://www.topografix.com/GPX/1/1">
  <metadata>
    <name>Test GPX File</name>
  </metadata>
  <wpt lat="45.5" lon="-90.5">
    <name>Cave Entrance</name>
    <cmt>Main entrance</cmt>
    <ele>350.2</ele>
    <time>2024-01-15T10:30:00Z</time>
  </wpt>
  <wpt lat="45.501" lon="-90.499">
    <name>Spring</name>
    <desc>Natural spring</desc>
    <ele>348.0</ele>
  </wpt>
  <trk>
    <name>Surface Path</name>
    <cmt>Path to cave</cmt>
    <trkseg>
      <trkpt lat="45.5" lon="-90.5">
        <ele>350.2</ele>
      </trkpt>
      <trkpt lat="45.5005" lon="-90.4995">
        <ele>351.0</ele>
      </trkpt>
      <trkpt lat="45.501" lon="-90.499">
        <ele>348.0</ele>
      </trkpt>
    </trkseg>
  </trk>
  <rte>
    <name>Planned Route</name>
    <rtept lat="45.5" lon="-90.5">
      <name>Start</name>
      <ele>350.2</ele>
    </rtept>
    <rtept lat="45.502" lon="-90.498">
      <name>End</name>
      <ele>345.0</ele>
    </rtept>
  </rte>
</gpx>"#;

    #[test]
    fn parse_basic_gpx() {
        let gpx = parse_gpx_str(SIMPLE_GPX).unwrap();

        assert_eq!(gpx.name.as_deref(), Some("Test GPX File"));
        assert_eq!(gpx.waypoints.len(), 2);
        assert_eq!(gpx.tracks.len(), 1);
        assert_eq!(gpx.routes.len(), 1);

        // Waypoints
        let wpt = &gpx.waypoints[0];
        assert_eq!(wpt.name.as_deref(), Some("Cave Entrance"));
        assert_eq!(wpt.comment.as_deref(), Some("Main entrance"));
        assert!((wpt.latitude - 45.5).abs() < 1e-9);
        assert!((wpt.longitude - (-90.5)).abs() < 1e-9);
        assert!((wpt.elevation.unwrap() - 350.2).abs() < 1e-9);
        assert_eq!(wpt.time.as_deref(), Some("2024-01-15T10:30:00Z"));

        // Second waypoint uses <desc> for comment
        let wpt2 = &gpx.waypoints[1];
        assert_eq!(wpt2.name.as_deref(), Some("Spring"));
        assert_eq!(wpt2.comment.as_deref(), Some("Natural spring"));

        // Track
        let trk = &gpx.tracks[0];
        assert_eq!(trk.name.as_deref(), Some("Surface Path"));
        assert_eq!(trk.comment.as_deref(), Some("Path to cave"));
        assert_eq!(trk.segments.len(), 1);
        assert_eq!(trk.segments[0].points.len(), 3);

        // Route
        let rte = &gpx.routes[0];
        assert_eq!(rte.name.as_deref(), Some("Planned Route"));
        assert_eq!(rte.points.len(), 2);
        assert_eq!(rte.points[0].name.as_deref(), Some("Start"));
    }

    #[test]
    fn parse_gpx_1_0_no_namespace() {
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.0" creator="test">
  <wpt lat="45.0" lon="-90.0">
    <name>Point A</name>
  </wpt>
</gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();
        assert_eq!(gpx.waypoints.len(), 1);
        assert_eq!(gpx.waypoints[0].name.as_deref(), Some("Point A"));
    }

    #[test]
    fn parse_gpx_with_namespace_prefix() {
        let xml = r#"<?xml version="1.0"?>
<gpx:gpx xmlns:gpx="http://www.topografix.com/GPX/1/1" version="1.1">
  <gpx:wpt lat="10.0" lon="20.0">
    <gpx:name>NS Point</gpx:name>
    <gpx:ele>100.5</gpx:ele>
  </gpx:wpt>
</gpx:gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();
        assert_eq!(gpx.waypoints.len(), 1);
        assert_eq!(gpx.waypoints[0].name.as_deref(), Some("NS Point"));
        assert!((gpx.waypoints[0].elevation.unwrap() - 100.5).abs() < 1e-9);
    }

    #[test]
    fn reject_invalid_latitude() {
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.1"><wpt lat="91.0" lon="0.0"></wpt></gpx>"#;

        let result = parse_gpx_str(xml);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("out of range"),
            "Should report lat out of range"
        );
    }

    #[test]
    fn empty_gpx_file() {
        let xml = r#"<?xml version="1.0"?><gpx version="1.1"></gpx>"#;
        let gpx = parse_gpx_str(xml).unwrap();
        assert!(gpx.waypoints.is_empty());
        assert!(gpx.tracks.is_empty());
        assert!(gpx.routes.is_empty());
        assert!(gpx.name.is_none());
    }

    #[test]
    fn waypoints_to_configs() {
        let gpx = parse_gpx_str(SIMPLE_GPX).unwrap();
        let configs = waypoints_to_import_configs(&gpx.waypoints, 0).unwrap();

        assert_eq!(configs.len(), 2);

        let c0 = &configs[0];
        assert_eq!(c0.section_index, 0);
        assert_eq!(c0.name.as_deref(), Some("Cave Entrance"));
        assert!(c0.shots.is_empty());
        assert!(matches!(&c0.placement, ImportPlacement::ZeroDatum { .. }));

        assert!(c0.station_comments.len() == 1);
        assert!(c0.station_comments[0].1.contains("Main entrance"));
        assert!(c0.station_comments[0].1.contains("Elev:"));
    }

    #[test]
    fn track_to_configs() {
        let gpx = parse_gpx_str(SIMPLE_GPX).unwrap();
        let configs = convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0;

        assert_eq!(configs.len(), 1);
        let c = &configs[0];
        assert_eq!(c.name.as_deref(), Some("Surface Path"));
        assert_eq!(c.shots.len(), 2);
        assert_eq!(c.shots[0].kind, ShotKind::Reference);
        assert!(c.shots[0].length > 0.0);
        assert!(c.shots[0].azimuth_in >= 0.0 && c.shots[0].azimuth_in < 360.0);
    }

    #[test]
    fn route_to_configs() {
        let gpx = parse_gpx_str(SIMPLE_GPX).unwrap();
        let configs = convert_gpx_to_import_configs(&gpx, false, false, true, &[], false)
            .unwrap()
            .0;

        assert_eq!(configs.len(), 1);
        let c = &configs[0];
        assert_eq!(c.name.as_deref(), Some("Planned Route"));
        assert_eq!(c.shots.len(), 1);
    }

    #[test]
    fn all_types_combined() {
        let gpx = parse_gpx_str(SIMPLE_GPX).unwrap();
        let configs = convert_gpx_to_import_configs(&gpx, true, true, true, &[], false)
            .unwrap()
            .0;

        // 2 waypoints + 1 track segment + 1 route = 4 sections
        assert_eq!(configs.len(), 4);

        for (i, c) in configs.iter().enumerate() {
            assert_eq!(c.section_index, i as u32);
        }
    }

    #[test]
    fn shot_computation_accuracy() {
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.1">
  <trk><name>Test</name><trkseg>
    <trkpt lat="0.0" lon="0.0"><ele>100.0</ele></trkpt>
    <trkpt lat="0.0" lon="0.0005"><ele>100.0</ele></trkpt>
  </trkseg></trk>
</gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();
        let configs = convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0;

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].shots.len(), 1);

        let shot = &configs[0].shots[0];
        assert!(
            (shot.length - 55.6).abs() < 1.0,
            "Expected ~55.6m, got {}",
            shot.length
        );
        assert!(
            (shot.azimuth_in - 90.0).abs() < 0.1,
            "Expected ~90°, got {}",
            shot.azimuth_in
        );
        assert!((shot.depth_in).abs() < 1e-9);
        assert!((shot.depth_out).abs() < 1e-9);
        assert_eq!(shot.kind, ShotKind::Reference);
    }

    #[test]
    fn elevation_converts_to_depth_and_slope_length() {
        // Two points at same lat/lon but different elevation:
        // horizontal distance = 0, vertical drop = 50m
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.1">
  <trk><name>Descent</name><trkseg>
    <trkpt lat="45.0" lon="7.0"><ele>200.0</ele></trkpt>
    <trkpt lat="45.0" lon="7.0"><ele>150.0</ele></trkpt>
    <trkpt lat="45.0" lon="7.0"><ele>180.0</ele></trkpt>
  </trkseg></trk>
</gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();
        let configs = convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0;

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].shots.len(), 2);

        let s0 = &configs[0].shots[0];
        // First station: depth 0 (reference elevation 200)
        // Second station: elevation dropped 50m -> depth = 50
        assert!(
            (s0.depth_in - 0.0).abs() < 1e-9,
            "First station depth should be 0"
        );
        assert!(
            (s0.depth_out - 50.0).abs() < 1e-9,
            "Second station depth should be 50"
        );
        // Pure vertical drop -> length = 50m
        assert!(
            (s0.length - 50.0).abs() < 0.1,
            "Shot length should be ~50m, got {}",
            s0.length
        );

        let s1 = &configs[0].shots[1];
        // Third station: elevation 180, depth = 200 - 180 = 20
        assert!((s1.depth_in - 50.0).abs() < 1e-9);
        assert!((s1.depth_out - 20.0).abs() < 1e-9);
        // Going up 30m -> length = 30m
        assert!(
            (s1.length - 30.0).abs() < 0.1,
            "Shot length should be ~30m, got {}",
            s1.length
        );
    }

    #[test]
    fn elevation_slope_distance_with_horizontal() {
        // Points separated horizontally AND vertically
        // Horizontal ~111m (0.001 deg lat at equator), vertical 50m
        // Slope = sqrt(111^2 + 50^2) ~ 121.7m
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.1">
  <trk><name>Slope</name><trkseg>
    <trkpt lat="0.0" lon="0.0"><ele>100.0</ele></trkpt>
    <trkpt lat="0.001" lon="0.0"><ele>50.0</ele></trkpt>
  </trkseg></trk>
</gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();
        let configs = convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0;

        let shot = &configs[0].shots[0];
        // Depth: 100 - 50 = 50m drop
        assert!((shot.depth_in - 0.0).abs() < 1e-9);
        assert!((shot.depth_out - 50.0).abs() < 1e-9);
        // Slope should be > horizontal distance (~111m)
        assert!(
            shot.length > 111.0,
            "Slope distance should exceed horizontal, got {}",
            shot.length
        );
        // And roughly sqrt(111^2 + 50^2) ~ 121.7
        assert!(
            (shot.length - 121.7).abs() < 2.0,
            "Expected ~121.7m, got {}",
            shot.length
        );
    }

    #[test]
    fn depth_inverted_reverses_sign() {
        // With depth_inverted, higher elevation = deeper
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.1">
  <trk><name>Inverted</name><trkseg>
    <trkpt lat="45.0" lon="7.0"><ele>0.0</ele></trkpt>
    <trkpt lat="45.0" lon="7.0"><ele>50.0</ele></trkpt>
    <trkpt lat="45.0" lon="7.0"><ele>30.0</ele></trkpt>
  </trkseg></trk>
</gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();

        // Normal: ele goes up -> depth decreases (negative)
        let normal = convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0;
        let s0 = &normal[0].shots[0];
        assert!((s0.depth_in - 0.0).abs() < 1e-9);
        assert!(
            (s0.depth_out - -50.0).abs() < 1e-9,
            "Normal: ele +50 -> depth -50"
        );

        // Inverted: ele goes up -> depth increases (positive)
        let inverted = convert_gpx_to_import_configs(&gpx, false, true, false, &[], true)
            .unwrap()
            .0;
        let s0 = &inverted[0].shots[0];
        assert!((s0.depth_in - 0.0).abs() < 1e-9);
        assert!(
            (s0.depth_out - 50.0).abs() < 1e-9,
            "Inverted: ele +50 -> depth +50"
        );

        // Slope length should be the same regardless of sign
        assert!((normal[0].shots[0].length - inverted[0].shots[0].length).abs() < 1e-9);
    }

    #[test]
    fn no_elevation_defaults_to_zero_depth() {
        // Points without elevation -> depth stays 0, length = horizontal only
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.1">
  <trk><name>NoElev</name><trkseg>
    <trkpt lat="0.0" lon="0.0"/>
    <trkpt lat="0.0" lon="0.0005"/>
  </trkseg></trk>
</gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();
        let configs = convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0;

        let shot = &configs[0].shots[0];
        assert!((shot.depth_in).abs() < 1e-9, "No elevation -> depth 0");
        assert!((shot.depth_out).abs() < 1e-9, "No elevation -> depth 0");
        // Should be ~55.6m horizontal
        assert!((shot.length - 55.6).abs() < 1.0);
    }

    #[test]
    fn multi_segment_track() {
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.1">
  <trk>
    <name>Multi Seg</name>
    <trkseg>
      <trkpt lat="0.0" lon="0.0"/>
      <trkpt lat="0.001" lon="0.0"/>
    </trkseg>
    <trkseg>
      <trkpt lat="1.0" lon="1.0"/>
      <trkpt lat="1.001" lon="1.0"/>
    </trkseg>
  </trk>
</gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();
        assert_eq!(gpx.tracks[0].segments.len(), 2);

        let configs = convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0;
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name.as_deref(), Some("Multi Seg seg 1"));
        assert_eq!(configs[1].name.as_deref(), Some("Multi Seg seg 2"));
    }

    #[test]
    fn skip_single_point_segment() {
        let xml = r#"<?xml version="1.0"?>
<gpx version="1.1">
  <trk><name>T</name>
    <trkseg>
      <trkpt lat="0.0" lon="0.0"/>
    </trkseg>
  </trk>
</gpx>"#;

        let gpx = parse_gpx_str(xml).unwrap();
        let configs = convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0;
        assert!(configs.is_empty());
    }

    #[test]
    fn custom_placement_override() {
        let gpx = parse_gpx_str(SIMPLE_GPX).unwrap();
        let placements = vec![GpxSectionPlacement {
            section_index: 0,
            placement: ImportPlacement::Origin,
            name: Some("Custom Name".to_string()),
            color: "#ff0000".to_string(),
        }];

        let configs = convert_gpx_to_import_configs(&gpx, false, true, false, &placements, false)
            .unwrap()
            .0;

        assert_eq!(configs[0].name.as_deref(), Some("Custom Name"));
        assert_eq!(configs[0].color, "#ff0000");
        assert!(matches!(configs[0].placement, ImportPlacement::Origin));
    }

    /// A GPX exported by a Windows tool can carry a UTF-8 BOM ahead of its XML
    /// declaration. `quick-xml` tolerates one (it skips text before the root),
    /// so this pins that tolerance rather than repairing a break: a marked file
    /// must keep importing exactly like the same file without the mark.
    #[test]
    fn a_utf8_bom_does_not_break_a_gpx_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom.gpx");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(SIMPLE_GPX.as_bytes());
        std::fs::write(&path, bytes).unwrap();

        let expected = parse_gpx_str(SIMPLE_GPX).unwrap();
        let parsed = parse_gpx(&path).unwrap();
        assert_eq!(parsed.name, expected.name);
        assert_eq!(parsed.waypoints.len(), expected.waypoints.len());
        assert_eq!(parsed.tracks.len(), expected.tracks.len());
        assert_eq!(parsed.routes.len(), expected.routes.len());
    }

    /// UTF-16 is not GPX in practice, and reading it as bytes produces a
    /// document made of NULs. Say what is wrong instead.
    #[test]
    fn a_utf16_gpx_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("u16.gpx");
        let mut bytes = vec![0xFF, 0xFE];
        for unit in SIMPLE_GPX.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();

        let err = parse_gpx(&path)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("UTF-16"),
            "the error must name the encoding, got {err:?}"
        );
    }

    /// #576 B27(g): a path that is not a regular file - here a FIFO named like
    /// a `.gpx` - blocked the import forever, and imports run on the app's
    /// main thread. It is refused at once, by the capped reader every
    /// importer shares.
    #[cfg(unix)]
    #[test]
    fn a_fifo_named_like_a_gpx_is_refused_at_once() {
        use crate::parsers::encoding::fifo_test_support::{make_fifo, within_five_seconds};
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.gpx");
        make_fifo(&fifo);
        let path = fifo.clone();
        let outcome = within_five_seconds(&fifo, move || {
            parse_gpx(&path)
                .map(|g| g.tracks.len())
                .map_err(|e| e.to_string())
        });
        println!("parse_gpx(pipe.gpx): {outcome:?}");
        let err = outcome
            .expect("the read must not block on a FIFO")
            .expect_err("a FIFO is not a GPX file");
        assert!(err.contains("not a regular file"), "{err}");
    }

    /// A four-point track whose elevations are none, 350, 349 and 352.4.
    const TRUNCATION_GPX: &str = r#"<?xml version="1.0"?>
<gpx version="1.1">
  <trk><name>Cut</name><trkseg>
    <trkpt lat="0.0" lon="0.0"></trkpt>
    <trkpt lat="0.0" lon="0.0005"><ele>350</ele></trkpt>
    <trkpt lat="0.0" lon="0.0010"><ele>349</ele></trkpt>
    <trkpt lat="0.0" lon="0.0015"><ele>352.4</ele></trkpt>
  </trkseg></trk>
</gpx>"#;

    /// `TRUNCATION_GPX` cut right after the first occurrence of `marker`.
    fn truncated_after(marker: &str) -> &'static str {
        let end = TRUNCATION_GPX.find(marker).unwrap() + marker.len();
        &TRUNCATION_GPX[..end]
    }

    /// The elevations a parse produced, or its error.
    fn parse_outcome(xml: &str) -> std::result::Result<Vec<Option<f64>>, String> {
        parse_gpx_str(xml)
            .map(|g| {
                g.tracks
                    .iter()
                    .flat_map(|t| &t.segments)
                    .flat_map(|s| &s.points)
                    .map(|p| p.elevation)
                    .collect()
            })
            .map_err(|e| e.to_string())
    }

    /// #576 B14 (decision: refuse, like Ariane): quick-xml reports a document
    /// that simply stops as a clean end of file, so a GPX cut off by a dying
    /// GPS or a partial download imported as a shorter track - and a number
    /// cut in half became a real reading (352.4 m imported as 35 m, a 300 m
    /// drop). Any end of file with elements still open now fails the parse,
    /// naming where it ends and how much is open.
    #[test]
    fn a_truncated_gpx_is_refused() {
        let cuts = [
            ("inside an <ele> number", "lon=\"0.0015\"><ele>35", Some(5)),
            ("between two points", "<ele>349</ele></trkpt>", Some(3)),
            ("after the track", "</trk>", Some(1)),
            // quick-xml's own syntax error, never a parse.
            ("inside a tag", "<trkpt lat=\"0.0\" lo", None),
        ];
        let outcomes: Vec<_> = cuts
            .iter()
            .map(|&(label, marker, open)| {
                let xml = truncated_after(marker);
                let outcome = parse_outcome(xml);
                println!("cut {label} (byte {}): {outcome:?}", xml.len());
                (label, xml.len(), open, outcome)
            })
            .collect();
        for (label, len, open, outcome) in outcomes {
            let err = outcome.expect_err("a truncated GPX must not parse");
            if let Some(open) = open {
                assert!(
                    err.contains(&format!(
                        "ends at byte {len} with {open} element(s) still open"
                    )),
                    "{label}: {err}"
                );
            }
        }
    }

    /// The whole document still parses, and so does one whose last point is
    /// self-closed - an empty element opens nothing.
    #[test]
    fn a_complete_gpx_still_parses_after_the_truncation_check() {
        assert_eq!(
            parse_outcome(TRUNCATION_GPX).unwrap(),
            vec![None, Some(350.0), Some(349.0), Some(352.4)]
        );
        let self_closed = r#"<gpx><trk><trkseg>
            <trkpt lat="0.0" lon="0.0"><ele>1</ele></trkpt>
            <trkpt lat="0.0" lon="0.0005"/>
        </trkseg></trk></gpx>"#;
        assert_eq!(parse_outcome(self_closed).unwrap(), vec![Some(1.0), None]);
    }

    /// The track configs `xml` converts to, tracks only.
    fn track_configs(xml: &str) -> Vec<ImportSectionConfig> {
        let gpx = parse_gpx_str(xml).unwrap();
        convert_gpx_to_import_configs(&gpx, false, true, false, &[], false)
            .unwrap()
            .0
    }

    /// A one-segment track along the equator: each `(lon, ele)` is a point.
    fn equator_track(points: &[(f64, Option<f64>)]) -> String {
        let pts: String = points
            .iter()
            .map(|(lon, ele)| match ele {
                Some(e) => format!(r#"<trkpt lat="0.0" lon="{lon}"><ele>{e}</ele></trkpt>"#),
                None => format!(r#"<trkpt lat="0.0" lon="{lon}"/>"#),
            })
            .collect();
        format!("<gpx><trk><trkseg>{pts}</trkseg></trk></gpx>")
    }

    /// Each station's depth along the first track, rounded to the millimetre.
    fn station_depths(config: &ImportSectionConfig) -> Vec<f64> {
        let round = |d: f64| (d * 1000.0).round() / 1000.0;
        let mut depths: Vec<f64> = config.shots.iter().map(|s| round(s.depth_in)).collect();
        depths.extend(config.shots.last().map(|s| round(s.depth_out)));
        depths
    }

    /// #576 B34 (decision: interpolate, warn): a missing elevation became the
    /// FIRST point's elevation, or 0 m when the first point had none - so a
    /// track starting without one climbed 350 m on its first leg, and that
    /// leg's slope length was 350 m. A gap is now interpolated along the
    /// track between the nearest points that have an elevation, by
    /// horizontal distance; a leading or trailing gap takes the nearest known
    /// value; depth 0 is the first point's filled elevation.
    #[test]
    fn a_missing_gpx_elevation_is_interpolated_along_the_track() {
        for (label, depths, expected) in [
            (
                "leading gap",
                vec![(0.0, None), (0.0005, Some(350.0)), (0.0010, Some(349.0))],
                vec![0.0, 0.0, 1.0],
            ),
            (
                "inner gap, even spacing",
                vec![(0.0, Some(100.0)), (0.0005, None), (0.0010, Some(98.0))],
                vec![0.0, 1.0, 2.0],
            ),
            (
                "inner gap, a quarter of the way",
                vec![(0.0, Some(100.0)), (0.0005, None), (0.0020, Some(98.0))],
                vec![0.0, 0.5, 2.0],
            ),
            (
                "trailing gap",
                vec![(0.0, Some(100.0)), (0.0005, Some(98.0)), (0.0010, None)],
                vec![0.0, 2.0, 2.0],
            ),
            (
                "no elevation at all",
                vec![(0.0, None), (0.0005, None)],
                vec![0.0, 0.0],
            ),
        ]
        .map(|(label, points, expected)| {
            let configs = track_configs(&equator_track(&points));
            let depths = station_depths(&configs[0]);
            let first_length = configs[0].shots[0].length;
            println!("{label}: depths {depths:?}, first leg {first_length:.2} m");
            (label, depths, expected)
        }) {
            assert_eq!(depths, expected, "{label}");
        }
        // The leading-gap leg is level now: its length is the horizontal
        // distance, not the 350 m climb.
        let configs = track_configs(&equator_track(&[(0.0, None), (0.0005, Some(350.0))]));
        let horizontal = haversine_distance(0.0, 0.0, 0.0, 0.0005);
        assert!(
            (configs[0].shots[0].length - horizontal).abs() < 1e-9,
            "{} vs {horizontal}",
            configs[0].shots[0].length
        );
    }

    /// The receipt says how many points were filled in - one line for the
    /// whole file, tracks and routes together - and says nothing when every
    /// point had an elevation or a track had none to fill from.
    #[test]
    fn the_receipt_counts_the_filled_gpx_points() {
        let warnings_of = |xml: &str| {
            let gpx = parse_gpx_str(xml).unwrap();
            convert_gpx_to_import_configs(&gpx, false, true, true, &[], false)
                .unwrap()
                .1
        };
        let two_tracks = format!(
            "<gpx><trk><trkseg>{a}</trkseg></trk><trk><trkseg>{b}</trkseg></trk>\
             <rte><rtept lat=\"0\" lon=\"0\"/><rtept lat=\"0\" lon=\"0.001\"><ele>5</ele></rtept></rte></gpx>",
            a = r#"<trkpt lat="0" lon="0"/><trkpt lat="0" lon="0.001"><ele>1</ele></trkpt>"#,
            b = r#"<trkpt lat="0" lon="0"><ele>2</ele></trkpt><trkpt lat="0" lon="0.001"/>"#,
        );
        let warnings = warnings_of(&two_tracks);
        println!("two tracks and a route, one gap each: {warnings:?}");
        assert_eq!(
            warnings,
            vec![
                "3 GPS points had no elevation; they were filled in from the nearest points \
                 along the track that have one."
                    .to_string()
            ]
        );
        let one = warnings_of(&equator_track(&[(0.0, Some(1.0)), (0.0005, None)]));
        assert_eq!(one.len(), 1);
        assert!(
            one[0].starts_with("1 GPS point had no elevation; it was"),
            "{one:?}"
        );
        for clean in [
            equator_track(&[(0.0, Some(1.0)), (0.0005, Some(2.0))]),
            equator_track(&[(0.0, None), (0.0005, None)]),
        ] {
            assert!(warnings_of(&clean).is_empty(), "{clean}");
        }
    }
}
