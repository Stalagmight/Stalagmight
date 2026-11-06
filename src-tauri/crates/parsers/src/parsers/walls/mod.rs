//! Walls (`.WPJ` / `.SRV`) cave-survey import.
//!
//! [Walls](https://texasspeleologicalsurvey.org/software/walls/) (David
//! McKenzie / Texas Speleological Survey) stores a project as a `.WPJ` tree of
//! books (folders) and surveys (leaves); each survey leaf points at a `.SRV`
//! survey-data file.  This module parses both and converts Walls's
//! inclination-or-depth-gauge, foresight/backsight survey model into
//! Stalagmight's depth-based import configs, mirroring [`crate::parsers::compass`]:
//!
//! - `.WPJ` is a flat, indented, Windows-1252 line file: `.BOOK`/`.ENDBOOK`
//!   nest, `.SURVEY` is a leaf, `.NAME` is the base filename (`.SRV` default
//!   extension), `.STATUS` is a bitfield (skip `DETACHED`/`TYPE_OTHER`), and
//!   `.OPTIONS` (`#units` args) + `.REF` (UTM/lat-long reference) inherit
//!   root → leaf.
//! - `.SRV` lines are `;`-commented, `#`-directive (`#Units`, `#Date`,
//!   `#Fix`, `#Prefix`, `#Segment`, `#Flag`, `#[`/`#]` block comments), or
//!   survey vectors.  Foresight/backsight pairs are written `fs/bs`; a Normal
//!   backsight is reversed 180° (azimuth) / negated (inclination) and
//!   averaged.  Underwater "depth-gauge" surveys (`#Units Order=DA Tape=SS`)
//!   carry two trailing columns that are station depths below the surface -
//!   these map straight onto Stalagmight's native `depth_in`/`depth_out`.
//! - CT inclinations are integrated (`length·sin(inc)`) along the station
//!   graph into absolute depths; the graph is split into linear sections
//!   joined by `FromImporting` merges + loop closures.
//! - A `#Fix`ed station with geographic coordinates becomes a GPS zero datum.
//!
//! Format references cross-validated against the dewalls
//! (`jedwards1211/dewalls`) and survex (`ojwb/survex` `src/datain.c`) Walls
//! parsers and the Walls manual.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::parsers::geo::FEET_TO_METERS;

mod convert;
mod srv;
mod units;
mod wpj;

pub use convert::*;
pub(crate) use srv::*;
pub(crate) use units::*;
pub(crate) use wpj::*;

/// Section colors cycled across imported sections (matches the app palette).
pub(crate) const SECTION_COLORS: &[&str] = &[
    "#4ecdc4", "#ff6b6b", "#ffd93d", "#6bcB77", "#4d96ff", "#c780fa", "#ff9f45", "#9bdeac",
];

// `.STATUS` bitfield (see module docs / dewalls `wallsprojectparser.cpp`).
pub(crate) const STATUS_DETACHED: u32 = 1 << 1; // entry skipped during compilation
pub(crate) const STATUS_TYPE_OTHER: u32 = 1 << 16; // attached non-survey file (svg, etc.)
pub(crate) const STATUS_DONT_DERIVE_DECL: u32 = 1 << 8;
pub(crate) const STATUS_DERIVE_DECL: u32 = 1 << 9;
pub(crate) const STATUS_NOT_GRID_RELATIVE: u32 = 1 << 10;
pub(crate) const STATUS_GRID_RELATIVE: u32 = 1 << 11;

// ---------------------------------------------------------------------------
// IPC preview payload
// ---------------------------------------------------------------------------

/// Lightweight summary of a parsed Walls project, returned to the import dialog.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct WallsFile {
    pub project_name: String,
    pub book_count: u32,
    pub survey_count: u32,
    /// Surveys whose `.SRV` file was found on disk and parsed.
    pub resolved_survey_count: u32,
    pub shot_count: u32,
    /// Anonymous-wall / splay shots that are excluded from `shot_count` but
    /// WOULD be imported with "Include splays" enabled. Lets the dialog allow
    /// import of a splay-only file (where `shot_count == 0`) once the user opts
    /// in, instead of leaving the button permanently disabled.
    pub splay_count: u32,
    /// True when the project carries a `.REF` projection reference (UTM/datum).
    /// This is the coordinate system, NOT a station anchor - see
    /// `fixed_station_count` for actual GPS anchors.
    pub has_georeference: bool,
    pub datum: Option<String>,
    pub utm_zone: Option<i32>,
    /// Number of `#Fix` stations with a resolvable geographic position. Each
    /// connected component containing one gets a GPS zero datum on import.
    pub fixed_station_count: u32,
    /// Referenced `.SRV` files that could not be found on disk.
    pub missing_file_count: u32,
    /// `.WPJ` entries skipped because they are non-survey attachments.
    pub skipped_attachment_count: u32,
    /// Whether any survey in this project had a shot measured in feet. Drives
    /// the import receipt's unit-conversion note - Walls carries no
    /// project-wide unit label the way Compass does, so this is derived from
    /// the per-shot units actually parsed (#391).
    pub any_feet_units: bool,
    pub surveys: Vec<WallsSurveySummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct WallsSurveySummary {
    pub name: String,
    pub title: String,
    pub date: String,
    pub segment: String,
    pub shot_count: u32,
    /// Anonymous-wall / splay shots in THIS survey (excluded from `shot_count`,
    /// importable only with "Include splays"). Lets the dialog scope the splay
    /// gate to the user's survey selection instead of the whole-file total.
    pub splay_count: u32,
    /// Geographic `#Fix` anchor stations in THIS survey. Lets the dialog scope
    /// the "GPS anchor" stat/warning to the selection.
    pub fixed_station_count: u32,
    /// `false` when the referenced `.SRV` file was missing on disk.
    pub resolved: bool,
}

/// Options controlling how a Walls project is converted into import configs.
#[derive(Debug, Clone)]
pub struct WallsImportOptions {
    /// Promote each non-empty shot comment to its FROM station.
    pub comments_on_stations: bool,
    /// Include anonymous "wall" / splay shots (a `-` station) as reference shots.
    pub include_splays: bool,
    /// Survey indices (into `ParsedWalls::surveys`, matching the preview's
    /// `WallsFile::surveys` order 1:1) the user chose to import. `None` imports
    /// every survey; `Some(set)` imports only the listed indices (an empty set
    /// imports nothing). Filtering by the ORIGINAL index keeps synthetic splay
    /// names (`anon:{si}:…`) stable and lets a deselected survey's `#Fix`
    /// anchors drop out cleanly instead of becoming "unused anchor" warnings.
    pub selected_surveys: Option<HashSet<u32>>,
}

impl Default for WallsImportOptions {
    fn default() -> Self {
        Self {
            comments_on_stations: true,
            include_splays: false,
            selected_surveys: None,
        }
    }
}

impl WallsImportOptions {
    /// Whether survey index `si` (into `ParsedWalls::surveys`) is included by the
    /// current selection. `None` selection ⇒ all surveys.
    pub(crate) fn includes_survey(&self, si: usize) -> bool {
        match &self.selected_surveys {
            Some(sel) => sel.contains(&(si as u32)),
            None => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Units state machine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LengthUnit {
    Meters,
    Feet,
}

impl LengthUnit {
    pub(crate) fn factor(self) -> f64 {
        match self {
            LengthUnit::Meters => 1.0,
            LengthUnit::Feet => FEET_TO_METERS,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AngleUnit {
    Degrees,
    Grads,
    Mils,
    Percent,
}

impl AngleUnit {
    /// Convert a raw reading in this unit to degrees.
    pub(crate) fn to_degrees(self, value: f64) -> f64 {
        match self {
            AngleUnit::Degrees => value,
            AngleUnit::Grads => value * 0.9, // 400 grad == 360°
            AngleUnit::Mils => value * (360.0 / 6400.0), // 6400 mil == 360°
            AngleUnit::Percent => (value / 100.0).atan().to_degrees(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum OrderElem {
    Dist,
    Azm,
    Inc,
    East,
    North,
    Up,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VectorType {
    Ct,
    Rect,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Case {
    Upper,
    Lower,
    Mixed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LrudType {
    From,
    To,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TapeEnd {
    Instrument,
    Station,
    Target,
}

/// The full `#Units` state, copied on `Save`/restored on `Restore`, reset by
/// `Reset`.  Defaults match the Walls manual / dewalls `WallsUnitsData`.
#[derive(Clone, Debug)]
pub(crate) struct WallsUnits {
    pub(crate) vector_type: VectorType,
    /// Measurement column order for compass-and-tape (CT) shots, e.g. D/A/V.
    pub(crate) ct_order: Vec<OrderElem>,
    /// Measurement column order for rectangular (RECT) shots, e.g. E/N/U. Kept
    /// independent of `ct_order` (matching Walls/dewalls' separate ctOrder /
    /// rectOrder) so a CT↔RECT switch never clobbers the other type's order.
    pub(crate) rect_order: Vec<OrderElem>,
    pub(crate) d_unit: LengthUnit,
    pub(crate) s_unit: LengthUnit,
    pub(crate) a_unit: AngleUnit,
    pub(crate) ab_unit: AngleUnit,
    pub(crate) v_unit: AngleUnit,
    pub(crate) vb_unit: AngleUnit,
    /// Explicit `Decl=` declination, degrees. `None` means unset - an explicit
    /// `Decl=0` (`Some(0.0)`) suppresses auto-derivation, a bare `Decl=` clears
    /// back to `None`.
    pub(crate) decl: Option<f64>,
    /// `#Units Grid=` grid convergence (degrees), subtracted from CT azimuths.
    pub(crate) grid: f64,
    /// `#Units RECT=<azimuth>`: bearing of the rectangular frame's +N axis
    /// (degrees), added to RECT vector bearings. Kept separate from `grid` so a
    /// `RECT=<angle>` → `CT` switch can't leak the frame angle into CT
    /// convergence (they are consumed with opposite signs).
    pub(crate) rect: f64,
    pub(crate) incd: f64,
    pub(crate) inca: f64,
    pub(crate) incab: f64,
    pub(crate) incv: f64,
    pub(crate) incvb: f64,
    pub(crate) incs: f64,
    pub(crate) inch: f64,
    pub(crate) type_ab_corrected: bool,
    pub(crate) type_vb_corrected: bool,
    pub(crate) type_ab_no_avg: bool,
    pub(crate) type_vb_no_avg: bool,
    pub(crate) case: Case,
    pub(crate) lrud_type: LrudType,
    pub(crate) lrud_order: [char; 4],
    pub(crate) tape_from: TapeEnd,
    pub(crate) tape_to: TapeEnd,
    pub(crate) prefix: [Option<String>; 3],
    pub(crate) flag: Option<String>,
}

impl Default for WallsUnits {
    fn default() -> Self {
        Self {
            vector_type: VectorType::Ct,
            ct_order: vec![OrderElem::Dist, OrderElem::Azm, OrderElem::Inc],
            rect_order: vec![OrderElem::East, OrderElem::North, OrderElem::Up],
            d_unit: LengthUnit::Meters,
            s_unit: LengthUnit::Meters,
            a_unit: AngleUnit::Degrees,
            ab_unit: AngleUnit::Degrees,
            v_unit: AngleUnit::Degrees,
            vb_unit: AngleUnit::Degrees,
            decl: None,
            grid: 0.0,
            rect: 0.0,
            incd: 0.0,
            inca: 0.0,
            incab: 0.0,
            incv: 0.0,
            incvb: 0.0,
            incs: 0.0,
            inch: 0.0,
            type_ab_corrected: false,
            type_vb_corrected: false,
            type_ab_no_avg: false,
            type_vb_no_avg: false,
            case: Case::Mixed,
            lrud_type: LrudType::From,
            lrud_order: ['L', 'R', 'U', 'D'],
            tape_from: TapeEnd::Instrument,
            tape_to: TapeEnd::Target,
            prefix: [None, None, None],
            flag: None,
        }
    }
}

impl WallsUnits {
    /// The measurement column order active for the current vector type.
    pub(crate) fn active_order(&self) -> &[OrderElem] {
        match self.vector_type {
            VectorType::Ct => &self.ct_order,
            VectorType::Rect => &self.rect_order,
        }
    }

    /// True when this survey records dive depth-gauge data: compass-and-tape,
    /// station-to-station taping, distance+azimuth order with no inclination
    /// column (`#Units Order=DA Tape=SS`).
    pub(crate) fn is_depth_gauge(&self) -> bool {
        self.vector_type == VectorType::Ct
            && self.tape_from == TapeEnd::Station
            && self.tape_to == TapeEnd::Station
            && self.ct_order.contains(&OrderElem::Dist)
            && self.ct_order.contains(&OrderElem::Azm)
            && !self.ct_order.contains(&OrderElem::Inc)
    }
}

// ---------------------------------------------------------------------------
// Georeference (.REF / #Fix)
// ---------------------------------------------------------------------------

/// A `.WPJ` `.REF` geographic reference (UTM zone/datum + lat-long origin).
///
/// The importer anchors on `#Fix` stations / the lat-long fields rather than the
/// UTM origin offset, so the `.REF` northing/easting columns are validated but
/// not retained.
#[derive(Clone, Debug)]
pub(crate) struct WallsRef {
    pub(crate) zone: i32,
    pub(crate) convergence: f64,
    pub(crate) latitude: f64,
    pub(crate) longitude: f64,
    pub(crate) datum: String,
}

// ---------------------------------------------------------------------------
// Parsed representation
// ---------------------------------------------------------------------------

/// A single normalized survey vector (post units / fs-bs averaging).  Azimuths
/// are forward-direction magnetic degrees (declination/convergence applied at
/// conversion time); inclinations are `+up` degrees; depths are `+down` meters.
#[derive(Clone, Debug)]
pub(crate) struct WallsVector {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) length_m: f64,
    /// Forward azimuth from the foresight (or reversed backsight), degrees.
    pub(crate) azimuth_in: Option<f64>,
    /// Forward azimuth from the reversed backsight (or foresight), degrees.
    pub(crate) azimuth_out: Option<f64>,
    /// Averaged inclination, `+up` degrees (`None` for depth-gauge / plumb-omitted).
    pub(crate) inc_deg: Option<f64>,
    /// Foresight inclination as `+down` pitch (model convention).
    pub(crate) pitch_in: Option<f64>,
    /// Backsight inclination as `+down` pitch.
    pub(crate) pitch_out: Option<f64>,
    pub(crate) from_depth: Option<f64>,
    pub(crate) to_depth: Option<f64>,
    pub(crate) is_depth_gauge: bool,
    pub(crate) left: Option<f64>,
    pub(crate) right: Option<f64>,
    pub(crate) up: Option<f64>,
    pub(crate) down: Option<f64>,
    pub(crate) comment: Option<String>,
    pub(crate) is_reference: bool,
    /// FROM / TO stations were the anonymous wall marker (`-`); these endpoints
    /// must be given unique synthetic names so unrelated splays never merge.
    pub(crate) from_anon: bool,
    pub(crate) to_anon: bool,
    pub(crate) date: Option<NaiveDate>,
    /// Explicit declination (`Decl=`) in effect for this vector, degrees.
    /// `None` when no explicit `Decl=` was set (auto-derive may apply).
    pub(crate) units_decl: Option<f64>,
    /// `IncH=` vertical adjustment (meters) added to the vertical of non-plumb
    /// CT shots.
    pub(crate) inch: f64,
    pub(crate) grid_relative: bool,
    /// `#Units Grid=` grid convergence (degrees) to subtract from the vector's
    /// azimuth (true → grid north). Applies to CT and RECT vectors alike - it is
    /// `units.grid` regardless of vector type. What a RECT vector excludes is the
    /// `RECT=<angle>` FRAME rotation (already baked into the computed bearing) and
    /// magnetic declination (forced to 0 for RECT); `Grid=` convergence is NOT
    /// dropped, so do not "fix" RECT to skip it.
    pub(crate) grid_convergence: f64,
    /// Auto-derive declination from `#Date` (`.STATUS` derive-declination flag).
    pub(crate) derive_decl: bool,
    /// True for `#Units RECT` E/N/U vectors: the azimuth is already an absolute
    /// grid bearing, so magnetic declination must NOT be applied at conversion
    /// (only convergence, governed by `grid_relative`, may rotate it).
    pub(crate) is_rect: bool,
    /// `#Units LRUD=To`: the LRUD on this leg was measured at the TO station, not
    /// the FROM station. In this shot-based model a station's passage rides on the
    /// shot that leaves it, so a To-typed leg's LRUD is shifted onto the next shot
    /// in the chain at conversion.
    pub(crate) lrud_is_to: bool,
}

/// A fixed station with known coordinates (`#Fix`).
#[derive(Clone, Debug)]
pub(crate) struct WallsFix {
    pub(crate) name: String,
    /// Resolved geographic position, when the `#Fix` gave lat/long (or could be
    /// projected via the inherited `.REF`).
    pub(crate) lat_long: Option<(f64, f64)>,
    /// Up coordinate (meters) if supplied - used to seed the station's depth.
    pub(crate) up: Option<f64>,
    /// True when `lat_long` came from projecting UTM E/N through the inherited
    /// `.REF` datum (rather than explicit lat/long). Lets the survey emit ONE
    /// aggregated "unrecognized datum" warning instead of one per fixed point
    /// (surface terrain files carry thousands of `#Fix` rows).
    pub(crate) projected_from_utm: bool,
}

/// A resolved + parsed survey leaf.
///
/// `pub` (fields deliberately not) because `test_support::parse_srv` hands it
/// to the root crate's tests as an opaque token to pass back into
/// `test_support::convert_one`.
pub struct ParsedSurvey {
    pub(crate) title: String,
    pub(crate) name: String,
    pub(crate) segment: String,
    pub(crate) date: Option<NaiveDate>,
    pub(crate) surveyor: Option<String>,
    pub(crate) vectors: Vec<WallsVector>,
    pub(crate) fixes: Vec<WallsFix>,
    /// The `.REF` inherited by this survey (nearest ancestor) - drives grid
    /// convergence + auto-declination for its shots.
    pub(crate) georef: Option<WallsRef>,
    pub(crate) resolved: bool,
    /// Whether any shot in this survey was measured in feet (#391).
    pub(crate) used_feet: bool,
}

/// Fully parsed Walls project.
pub struct ParsedWalls {
    pub(crate) project_name: String,
    pub(crate) book_count: u32,
    pub(crate) surveys: Vec<ParsedSurvey>,
    pub(crate) georef: Option<WallsRef>,
    pub(crate) missing_files: Vec<String>,
    pub(crate) skipped_attachments: u32,
    pub(crate) warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Parse a Walls `.WPJ` project (following its survey links) or a single
/// `.SRV` survey file.
pub fn parse_walls(path: &Path) -> Result<ParsedWalls> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("wpj") => parse_wpj(path),
        Some("srv") => parse_single_srv(path),
        _ => Err(anyhow!(
            "Unsupported Walls file '{}' (expected .wpj or .srv)",
            path.display()
        )),
    }
}

// `test-support` (never on in production builds) keeps the shared test entry
// points compiled for the root crate's tests, which can no longer see a
// dependency's `#[cfg(test)]` items now that the parsers live in their own
// crate. The test suite proper stays `cfg(test)`-only.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
pub(crate) mod tests;

// #576 NF-FIX3R-3: loop closures onto points the merges already joined.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod closure_tests;

// #576 NF-FIX3R-2: one read per .SRV, one byte budget per .WPJ.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod wpj_budget_tests;

// #576 RV4-R-P2: only a survey that was read is reported as read from outside.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod wpj_outside_rv4_tests;

// #576 FZ-PARSE-5 / FZ-PARSE-8: readings beyond the limit read as missing.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod reading_bound_tests;
