//! The `summary` member: a non-authoritative cache for Recents and other
//! cheap readers. The writer always recomputes it from the model; a reader may
//! use it without parsing the body, and the decoder never trusts it.

use std::cmp::Ordering;

use crate::models::survey::Project;
use crate::models::ShotKind;

use super::error::EncodeError;
use super::scan::{members, MAX_SMALL_MEMBERS};
use super::types::{RawJson, SpeleoDbLink};
use super::write::{float, small, string, Obj};

/// The summary of a project, as written.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// The project name (the same as `project.name`).
    pub name: String,
    pub lines: u64,
    /// Station entities (a junction counts once).
    pub stations: u64,
    pub shots: u64,
    /// Surveyed length in metres, Reference shots excluded; `None` when the
    /// sum is not a finite number.
    pub length: Option<f64>,
    /// `[longitude, latitude]` of the first datum in list order.
    pub center: Option<[f64; 2]>,
    /// `[west, south, east, north]` over every datum; `east < west` when the
    /// box crosses the antimeridian.
    pub bounds: Option<[f64; 4]>,
    /// The linked SpeleoDB project id.
    pub speleodb: Option<String>,
}

impl Summary {
    /// The summary the writer computes for `project` and its link.
    pub fn of(project: &Project, link: Option<&SpeleoDbLink>) -> Self {
        let shots = project
            .lines()
            .iter()
            .map(|line| line.shots.len() as u64)
            .sum();
        let length = project
            .lines()
            .iter()
            .flat_map(|line| &line.shots)
            .filter(|shot| shot.kind != ShotKind::Reference)
            .map(|shot| shot.length)
            .sum::<f64>();
        let center = project
            .datums
            .first()
            .map(|d| [d.gps.longitude, d.gps.latitude]);
        let bounds = (!project.datums.is_empty()).then(|| {
            let mut south = f64::INFINITY;
            let mut north = f64::NEG_INFINITY;
            let mut longitudes = Vec::with_capacity(project.datums.len());
            for datum in &project.datums {
                south = south.min(datum.gps.latitude);
                north = north.max(datum.gps.latitude);
                longitudes.push(datum.gps.longitude);
            }
            let (west, east) = longitude_bounds(&longitudes);
            [west, south, east, north]
        });
        Self {
            name: project.name.clone(),
            lines: project.lines().len() as u64,
            stations: project.stations.len() as u64,
            shots,
            length: length.is_finite().then_some(length),
            center,
            bounds,
            speleodb: link.map(|l| l.project_id.clone()),
        }
    }

    /// Write `"summary":{...}`, `preserved` keys after the known ones.
    pub(super) fn write(
        &self,
        out: &mut Vec<u8>,
        preserved: &[(String, RawJson)],
    ) -> Result<(), EncodeError> {
        string(out, "summary");
        out.push(b':');
        let mut o = Obj::open(out);
        string(o.key("name"), &self.name);
        small(o.key("lines"), self.lines);
        small(o.key("stations"), self.stations);
        small(o.key("shots"), self.shots);
        if let Some(length) = self.length {
            float(o.key("length"), length, || "summary length".to_string())?;
        }
        if let Some(center) = self.center {
            floats(o.key("center"), &center, "summary center")?;
        }
        if let Some(bounds) = self.bounds {
            floats(o.key("bounds"), &bounds, "summary bounds")?;
        }
        if let Some(id) = &self.speleodb {
            string(o.key("speleodb"), id);
        }
        o.preserved(preserved);
        o.close();
        Ok(())
    }

    /// Read a summary leniently (a cache: a malformed field is left unset).
    pub(super) fn read(text: &str) -> Option<Self> {
        let pairs = members(text, MAX_SMALL_MEMBERS).ok()?;
        let mut summary = Self {
            name: String::new(),
            lines: 0,
            stations: 0,
            shots: 0,
            length: None,
            center: None,
            bounds: None,
            speleodb: None,
        };
        for (key, value) in pairs {
            let raw = value.get();
            match key.as_ref() {
                "name" => summary.name = serde_json::from_str(raw).unwrap_or_default(),
                "lines" => summary.lines = serde_json::from_str(raw).unwrap_or_default(),
                "stations" => summary.stations = serde_json::from_str(raw).unwrap_or_default(),
                "shots" => summary.shots = serde_json::from_str(raw).unwrap_or_default(),
                "length" => summary.length = serde_json::from_str(raw).ok(),
                "center" => summary.center = serde_json::from_str(raw).ok(),
                "bounds" => summary.bounds = serde_json::from_str(raw).ok(),
                "speleodb" => summary.speleodb = serde_json::from_str(raw).ok(),
                _ => {}
            }
        }
        Some(summary)
    }
}

/// The keys [`Summary::write`] writes, in order.
pub(super) const SUMMARY_KEYS: [&str; 8] = [
    "name", "lines", "stations", "shots", "length", "center", "bounds", "speleodb",
];

fn floats(out: &mut Vec<u8>, values: &[f64], what: &str) -> Result<(), EncodeError> {
    out.push(b'[');
    for (i, &value) in values.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        float(out, value, || what.to_string())?;
    }
    out.push(b']');
    Ok(())
}

/// West and east bounds of a set of longitudes: the box opposite the largest
/// gap between neighbours (wrapping at the antimeridian), so a cave across it
/// gets `east < west`. The rule of the Recents list today.
fn longitude_bounds(longitudes: &[f64]) -> (f64, f64) {
    let mut sorted = longitudes.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let (Some(&first), Some(&last)) = (sorted.first(), sorted.last()) else {
        return (0.0, 0.0);
    };
    if sorted.len() == 1 {
        return (first, first);
    }
    let mut max_gap = 0.0f64;
    let mut gap_end = 0;
    for i in 0..sorted.len() {
        let next = (i + 1) % sorted.len();
        let gap = if next == 0 {
            (first + 360.0) - last
        } else {
            sorted[next] - sorted[i]
        };
        if gap > max_gap {
            max_gap = gap;
            gap_end = next;
        }
    }
    let west = sorted[gap_end];
    let east = sorted[(gap_end + sorted.len() - 1) % sorted.len()];
    (west, east)
}
