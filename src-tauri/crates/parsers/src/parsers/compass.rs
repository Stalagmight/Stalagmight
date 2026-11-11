//! Compass (`.DAT` / `.MAK`) survey import.
//!
//! Parsing is delegated to the vendored [`compass_data`] fork (see
//! `vendor/compass_data`); this module converts Compass's inclination-based,
//! feet/magnetic-degree survey model into Stalagmight's depth-based import
//! configs:
//!
//! - declination + UTM convergence + per-survey correction factors are baked
//!   into the stored azimuth/length so the imported geometry matches what
//!   Compass plots;
//! - feet are converted to meters;
//! - station elevations are integrated from `length * sin(inclination)` along the
//!   station graph (anchored at the `.MAK` fixed station's UTM elevation, or 0)
//!   and stored as Stalagmight absolute depth (positive = down);
//! - the station-name graph is split into linear sections joined by
//!   `FromImporting` merges + loop closures;
//! - a `.MAK` fixed station becomes a GPS zero datum (UTM → WGS84 via `proj4rs`).

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::Datelike;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use compass_data::{
    BackSightCorrectionFactors, CorrectionFactors, Datum, DeclinationMode, InclinationUnits,
    LengthUnits, Project, Shot, Survey,
};

use crate::models::azimuth_reference::{AzimuthReference, CorrectionSource};
use crate::models::section::{ImportClosure, ImportPlacement, ImportSectionConfig};
use crate::models::shot::{ShotData, ShotKind};
use crate::models::station::GpsCoordinates;
use crate::parsers::encoding::{read_latin1, reject_bom};
use crate::parsers::geo::{non_empty, FEET_TO_METERS};

/// Distances/LRUD at or above this are Compass "missing data" sentinels.
const MISSING_VALUE_THRESHOLD: f64 = 990.0;
/// Angles at or below this are Compass "missing data" sentinels.
const MISSING_ANGLE_THRESHOLD: f64 = -900.0;

/// Section colors cycled across imported sections (matches the app palette feel).
const SECTION_COLORS: &[&str] = &[
    "#4ecdc4", "#ff6b6b", "#ffd93d", "#6bcB77", "#4d96ff", "#c780fa", "#ff9f45", "#9bdeac",
];

// ---------------------------------------------------------------------------
// IPC preview payload
// ---------------------------------------------------------------------------

/// Lightweight summary of a parsed Compass file, returned to the import dialog.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct CompassFile {
    pub cave_name: String,
    pub survey_count: u32,
    pub shot_count: u32,
    /// How many of `shot_count` are flagged `X` (total exclusion) and so are
    /// dropped by the converter under the default `include_excluded=false`
    /// toggle (#437). `shot_count - excluded_shot_count` is what a default
    /// import actually lands.
    pub excluded_shot_count: u32,
    /// How many shots the converter drops because their resolved length is
    /// non-positive - a missing-length sentinel, an explicit zero, or a
    /// non-finite reading (`convert_compass_to_import_configs`'s
    /// `length_m <= 0.0` check) - broken out by exclusion status and by the
    /// `apply_corrections` toggle. See `InvalidShotCounts` for the exact
    /// arithmetic; it is exact in all four `(include_excluded,
    /// apply_corrections)` toggle states, unlike a single count computed
    /// under one fixed pair of assumptions (#437 follow-up, review of #550).
    pub invalid_shot_counts: InvalidShotCounts,
    pub has_backsights: bool,
    pub datum: Option<String>,
    pub utm_zone: Option<i32>,
    pub has_georeference: bool,
    pub declination_mode: String,
    /// Length unit declared by the file's surveys: `"feet"`, `"meters"`,
    /// `"mixed"` (surveys disagree), or `null` (no surveys / unknown).
    /// Drives the unit blurb on the import dialog so we don't tell a
    /// metric survey we're converting feet.
    pub length_unit: Option<String>,
    pub surveys: Vec<CompassSurveySummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct CompassSurveySummary {
    pub name: String,
    pub date: String,
    pub team: Option<String>,
    pub shot_count: u32,
    /// See `CompassFile::excluded_shot_count`; same accounting, scoped to
    /// this survey.
    pub excluded_shot_count: u32,
    /// See `CompassFile::invalid_shot_counts`; same accounting, scoped to
    /// this survey.
    pub invalid_shot_counts: InvalidShotCounts,
    pub declination: f64,
}

/// Options controlling how a Compass file is converted into import configs.
#[derive(Debug, Clone)]
pub struct CompassImportOptions {
    /// Include shots flagged `X` (total exclusion); they import as reference shots.
    pub include_excluded: bool,
    /// Apply the per-survey CORRECTIONS / CORRECTIONS2 factors to readings.
    pub apply_corrections: bool,
    /// Promote each non-empty shot comment to its FROM station (Compass
    /// convention: surveyors annotate the station they were standing at).
    /// When false, comments stay on the shot.
    pub comments_on_stations: bool,
}

impl Default for CompassImportOptions {
    fn default() -> Self {
        Self {
            include_excluded: false,
            apply_corrections: true,
            comments_on_stations: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal parsed representation
// ---------------------------------------------------------------------------

/// A `.MAK` fixed station with a known UTM location (meters).
/// `datum` / `utm_zone` are captured from the `#file` entry that declared
/// the station, so `resolve_anchor` can project it in its own frame
/// rather than file 0's (some multi-`#file` projects mix zones / datums).
#[derive(Debug, Clone)]
struct FixedStation {
    name: String,
    easting: f64,
    northing: f64,
    elevation: f64,
    datum: Option<Datum>,
    utm_zone: Option<u8>,
}

/// Per-`#file` Compass state.  `.MAK` projects can change DATUM, UTM zone,
/// convergence, and declination-mode policy between successive `#file`
/// entries, so the state has to travel alongside each survey instead of
/// being collapsed to the first file's values.
#[derive(Debug, Clone)]
struct CompassFileState {
    datum: Option<Datum>,
    utm_zone: Option<u8>,
    convergence: f64,
    convergence_applied: bool,
    declination_mode: DeclinationMode,
}

impl Default for CompassFileState {
    fn default() -> Self {
        Self {
            datum: None,
            utm_zone: None,
            convergence: 0.0,
            convergence_applied: false,
            declination_mode: DeclinationMode::default(),
        }
    }
}

/// A survey paired with the per-file Compass state in effect when it was
/// declared.  Used downstream by `build_ctx` so each survey sees its own
/// declination-mode / convergence rather than the first file's.
struct SurveyWithState {
    survey: Survey,
    state: CompassFileState,
}

/// Fully parsed Compass data plus the project-level georeference state.
/// `datum` / `utm_zone` come from the FIRST `#file` entry and feed
/// `resolve_anchor` (UTM-zone overrides on later files would need to be
/// projected per-file).  `declination_mode` is reported in the preview;
/// per-file overrides for declination/convergence flow through the
/// corresponding `SurveyWithState.state` so each survey's geometry uses
/// the policy declared in its own `#file` entry.
pub struct ParsedCompass {
    cave_name: String,
    datum: Option<Datum>,
    utm_zone: Option<u8>,
    declination_mode: DeclinationMode,
    fixed_stations: Vec<FixedStation>,
    surveys: Vec<SurveyWithState>,
    warnings: Vec<String>,
}

/// Parse a Compass `.MAK` project (following its `#file` links) or a single
/// `.DAT` survey file.
pub fn parse_compass(path: &Path) -> Result<ParsedCompass> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("mak") => parse_mak(path),
        Some("dat") => parse_dat(path),
        _ => Err(anyhow!(
            "Unsupported Compass file '{}' (expected .mak or .dat)",
            path.display()
        )),
    }
}

fn parse_mak(path: &Path) -> Result<ParsedCompass> {
    // The `.MAK` and every `.DAT` it links are read inside the vendored crate,
    // from their paths - so unlike `parse_dat` there is no boundary here at
    // which a byte-order mark could be stripped. Name the mark instead of
    // letting the vendored lexer report an invisible character (#429). A `.DAT`
    // reached through a `.MAK` keeps the residue: a mark there lands in that
    // file's cave name, which is only reachable by fixing the vendored reader.
    reject_bom(path)?;
    let project =
        Project::read(path).map_err(|e| anyhow!("Failed to read Compass project: {e}"))?;
    let loaded = project
        .load_survey_files()
        .map_err(|e| anyhow!("Failed to load Compass survey files: {e}"))?;

    let mut datum = None;
    let mut utm_zone = None;
    let mut declination_mode = DeclinationMode::default();
    let mut fixed_stations = Vec::new();
    let mut surveys: Vec<SurveyWithState> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (i, dat) in loaded.survey_files.iter().enumerate() {
        // Each linked `.DAT` is parsed on its own, so each can stop part-way
        // through and drop the surveys behind the one it stopped on.  A `.MAK`
        // links many files, so the warning has to say WHICH one lost surveys.
        warnings.extend(dat_truncation_warning(
            Some(&dat.file_path.display().to_string()),
            dat.unread(),
            dat.unread_at(),
            dat.surveys().len(),
        ));
        let raw_state = &dat.file_state;
        // Reduce the vendor crate's FileState to the subset our pipeline
        // actually consults.  This is computed per file so subsequent
        // `#file` entries with different DATUM/UTM/CONVERGENCE/DECL
        // settings flow through to the surveys they introduced.
        let mut file_state = CompassFileState {
            datum: Some(raw_state.datum),
            utm_zone: raw_state.utm_zone.or(Some(raw_state.base_location.zone)),
            convergence: raw_state.base_location.convergence_angle,
            convergence_applied: false,
            declination_mode: DeclinationMode::default(),
        };
        if let Some(fc) = &raw_state.file_convergence {
            file_state.convergence = fc.angle;
            file_state.convergence_applied = fc.enabled;
        }
        if let Some(pp) = &raw_state.project_parameters {
            file_state.declination_mode = pp.declination_mode;
            file_state.convergence_applied = pp.utm_convergence_applied;
        }
        if i == 0 {
            // Mirror the first-file state into the project-wide fields
            // surfaced by `build_preview` (datum / utm_zone / declination_mode).
            // For geometry, each survey uses its own per-file state via
            // `SurveyWithState.state`; for the anchor projection,
            // `resolve_anchor` reads the per-FixedStation datum/zone
            // below (so multi-zone projects pick up the right frame).
            datum = file_state.datum;
            utm_zone = file_state.utm_zone;
            declination_mode = file_state.declination_mode;
        } else {
            // Surface divergence so the user knows the single-line
            // preview values reflect file 0 only.  Geometry is already
            // resolved per-file via `SurveyWithState.state`; this is
            // strictly a transparency warning for the import dialog.
            if file_state.datum != datum {
                warnings.push(format!(
                    "File #{} declares DATUM {:?} but the project preview shows {:?} (file 0). \
                     Surveys from this file are still imported in their own frame.",
                    i + 1,
                    file_state.datum,
                    datum,
                ));
            }
            if file_state.utm_zone != utm_zone {
                warnings.push(format!(
                    "File #{} declares UTM zone {:?} but the project preview shows {:?} (file 0). \
                     The anchor station's own zone is used for projection.",
                    i + 1,
                    file_state.utm_zone,
                    utm_zone,
                ));
            }
            if file_state.declination_mode != declination_mode {
                warnings.push(format!(
                    "File #{} declares declination mode {:?} but the project preview shows {:?} \
                     (file 0). Each survey still uses its own file's declination mode.",
                    i + 1,
                    file_state.declination_mode,
                    declination_mode,
                ));
            }
        }
        for station in &dat.project_stations {
            if let Some(loc) = station.location {
                // A fixed station becomes the GPS zero datum / elevation anchor;
                // a non-finite coordinate there would poison the UTM->WGS84
                // projection and every anchored elevation. Skip it (with a
                // warning) rather than anchoring the import to garbage.
                if !loc.easting.is_finite() || !loc.northing.is_finite() || !loc.up.is_finite() {
                    warnings.push(format!(
                        "Fixed station '{}' has a non-finite UTM location and was ignored \
                         as an anchor candidate.",
                        station.name
                    ));
                    continue;
                }
                fixed_stations.push(FixedStation {
                    name: station.name.clone(),
                    easting: loc.easting,
                    northing: loc.northing,
                    elevation: loc.up,
                    datum: file_state.datum,
                    utm_zone: file_state.utm_zone,
                });
            }
        }
        for survey in dat.surveys() {
            surveys.push(SurveyWithState {
                survey: survey.clone(),
                state: file_state.clone(),
            });
        }
    }

    let cave_name = surveys
        .first()
        .map_or_else(String::new, |s| s.survey.cave_name.clone());

    Ok(ParsedCompass {
        cave_name,
        datum,
        utm_zone,
        declination_mode,
        fixed_stations,
        surveys,
        warnings,
    })
}

/// What a `.DAT` legitimately leaves behind once every survey has been read:
/// Compass ends each survey with a form feed (0x0C) and a DOS editor may park
/// an end-of-file marker (0x1A) after the last one.  Anything else in the tail
/// is survey text the parser stopped on.
fn only_record_separators(tail: &str) -> bool {
    tail.chars().all(|c| c.is_whitespace() || c == '\u{1a}')
}

/// The warning for a `.DAT` whose parse gave up part-way through the file.
///
/// The survey-file parser is a `many0`: it stops at the first survey it cannot
/// read and returns the ones before it, leaving the rest of the file
/// unconsumed.  That comes back as a successful import of a shorter file, so
/// without this the surveys after the bad one vanish and the user is told
/// nothing.  #479 is what made the state reachable - an impossible
/// `SURVEY DATE:` used to panic the parser and now rejects the survey.
///
/// Importing the surveys that did read is more useful than refusing the whole
/// file, so this is a warning rather than an error: the import proceeds, and
/// the user learns which survey ended it and where.
///
/// `file` names the `.DAT` for a `.MAK` project, which can link many of them;
/// a bare `.DAT` import passes `None`, since the file is the one the user just
/// picked.  `offset` is where the unread tail starts, in bytes from the top of
/// that file.
fn dat_truncation_warning(
    file: Option<&str>,
    remainder: &str,
    offset: usize,
    parsed: usize,
) -> Option<String> {
    if only_record_separators(remainder) {
        return None;
    }
    let stopped_at = parsed + 1;
    let of_file = file.map_or_else(String::new, |name| format!(" of '{name}'"));
    let imported = match parsed {
        0 => "no surveys were imported from it".to_string(),
        1 => "only the first survey was imported from it".to_string(),
        n => format!("only the first {n} surveys were imported from it"),
    };
    Some(format!(
        "Parsing stopped at survey {stopped_at}{of_file} (byte {offset}) and the rest of the \
         file was not read; {imported}. Compass surveys are read in order, so the first header \
         this importer cannot parse (an impossible SURVEY DATE, a malformed FORMAT line) ends \
         the import there. Fix survey {stopped_at}{of_file} and import again to get the surveys \
         after it."
    ))
}

fn parse_dat(path: &Path) -> Result<ParsedCompass> {
    let contents = read_latin1(path)?;
    let (raw_surveys, remainder) = Survey::parse_dat_file_with_remainder(&contents)
        .map_err(|e| anyhow!("Failed to parse Compass .DAT file: {e}"))?;
    // `read_latin1` maps one file byte to one `char`, so the chars ahead of
    // the remainder count the bytes consumed - the offset into the file, past
    // any byte-order mark (stripped before parsing).
    let offset = contents.chars().count() - remainder.chars().count();
    let warnings: Vec<String> = dat_truncation_warning(None, remainder, offset, raw_surveys.len())
        .into_iter()
        .collect();
    let cave_name = raw_surveys
        .first()
        .map_or_else(String::new, |s| s.cave_name.clone());
    // A bare .DAT lacks #file scope, so every survey shares the same
    // default state.  parse_mak builds richer per-file state.
    let default_state = CompassFileState::default();
    let surveys = raw_surveys
        .into_iter()
        .map(|survey| SurveyWithState {
            survey,
            state: default_state.clone(),
        })
        .collect();
    Ok(ParsedCompass {
        cave_name,
        datum: None,
        utm_zone: None,
        declination_mode: DeclinationMode::default(),
        fixed_stations: Vec::new(),
        surveys,
        warnings,
    })
}

/// Build the import-dialog preview from parsed data.
/// Survey count and the collapsed length-unit label, without building the
/// full [`CompassFile`] preview. `execute_compass_import` needs only these
/// two scalars to fill in the receipt (#393 item 1); the full preview also
/// builds a `CompassSurveySummary` per survey and clones the parser's
/// warnings, work an import that already parsed and converted the file has
/// no use for.
pub fn preview_scalars(parsed: &ParsedCompass) -> (u32, Option<String>) {
    // Collapse per-survey length units to a single project-wide label.
    // Compass `D` (decimal feet) → "feet"; `M` → "meters".  Multi-survey
    // files where the unit isn't uniform get "mixed" so the dialog can
    // surface the discrepancy instead of silently picking one.
    let length_unit = {
        let mut iter = parsed
            .surveys
            .iter()
            .map(|s| s.survey.parameters.format_parameters.length_units());
        match iter.next() {
            None => None,
            Some(first) => {
                let uniform = iter.all(|u| u == first);
                if !uniform {
                    Some("mixed".to_string())
                } else {
                    Some(match first {
                        Some(LengthUnits::Meters) => "meters".to_string(),
                        _ => "feet".to_string(),
                    })
                }
            }
        }
    };
    (parsed.surveys.len() as u32, length_unit)
}

/// Number of a survey's shots flagged `X` (total exclusion), i.e. the shots
/// `convert` drops under the default `include_excluded=false` toggle. Mirrors
/// the `parse_flags` + `exclude_all` check `convert` itself runs.
fn excluded_shot_count(survey: &Survey) -> u32 {
    survey
        .shots
        .iter()
        .filter(|shot| parse_flags(shot.flags.as_ref()).exclude_all)
        .count() as u32
}

/// Shots the converter drops because their resolved length is non-positive
/// (`convert_compass_to_import_configs`'s `length_m <= 0.0` check, pass 1),
/// broken out by the two live dialog toggles that change which shots reach
/// that check and how their length resolves:
///
/// - `include_excluded` decides whether `X`-flagged shots even reach the
///   length check at all (excluded-and-dropped-by-exclusion never does, under
///   the default toggle) - hence separate `non_excluded_*` / `excluded_*`
///   counters, not one count filtered to non-excluded shots. An excluded
///   shot that is ALSO zero-length used to be silently missing from the
///   preview once `include_excluded` was checked: `excluded_shot_count`
///   stopped being subtracted, but nothing else named that the backend still
///   drops it at the length check (#437 follow-up, review of #550).
/// - `apply_corrections` decides whether `CORRECTIONS2`'s length term is
///   added before the length check - it can itself push an otherwise-
///   positive length to zero or below, or an otherwise-invalid one positive
///   (#437 follow-up). A bare `shot.length` read, or a count computed under
///   one fixed corrections setting, went stale the moment the user flipped
///   that checkbox.
///
/// `shot_count - (include_excluded ? 0 : excluded_shot_count) -
/// non_excluded_{corrected,uncorrected} - (include_excluded ?
/// excluded_{corrected,uncorrected} : 0)`, picking the pair matching the
/// live `apply_corrections` toggle, is exact in all four toggle states.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct InvalidShotCounts {
    /// Non-`X`-flagged shots with non-positive length, corrections applied.
    pub non_excluded_corrected: u32,
    /// Non-`X`-flagged shots with non-positive length, corrections NOT
    /// applied.
    pub non_excluded_uncorrected: u32,
    /// `X`-flagged shots that would ALSO be dropped by the length check if
    /// `include_excluded` let them reach it, corrections applied.
    pub excluded_corrected: u32,
    /// `X`-flagged shots that would ALSO be dropped by the length check if
    /// `include_excluded` let them reach it, corrections NOT applied.
    pub excluded_uncorrected: u32,
}

/// `anchor_lat_lng` is deliberately `None` here rather than resolving the
/// real GPS anchor: it only feeds `Auto`-mode declination, which affects
/// azimuth, never length, so it cannot change these counts. Any warnings
/// `build_ctx` would emit are scratch - `build_preview` already carries the
/// parser's own warnings and must not duplicate conversion-time ones it
/// isn't otherwise running.
fn invalid_shot_counts(survey: &Survey, file_state: &CompassFileState) -> InvalidShotCounts {
    let opts_corrected = CompassImportOptions {
        include_excluded: false,
        apply_corrections: true,
        comments_on_stations: true,
    };
    let opts_uncorrected = CompassImportOptions {
        apply_corrections: false,
        ..opts_corrected
    };
    let mut scratch = Vec::new();
    let ctx_corrected = build_ctx(survey, file_state, None, &opts_corrected, &mut scratch);
    scratch.clear();
    let ctx_uncorrected = build_ctx(survey, file_state, None, &opts_uncorrected, &mut scratch);

    let mut counts = InvalidShotCounts::default();
    for shot in &survey.shots {
        let excluded = parse_flags(shot.flags.as_ref()).exclude_all;
        let (_, length_corrected, _) = shot_geometry(shot, &ctx_corrected);
        let (_, length_uncorrected, _) = shot_geometry(shot, &ctx_uncorrected);
        if excluded {
            if length_corrected <= 0.0 {
                counts.excluded_corrected += 1;
            }
            if length_uncorrected <= 0.0 {
                counts.excluded_uncorrected += 1;
            }
        } else {
            if length_corrected <= 0.0 {
                counts.non_excluded_corrected += 1;
            }
            if length_uncorrected <= 0.0 {
                counts.non_excluded_uncorrected += 1;
            }
        }
    }
    counts
}

fn add_invalid_shot_counts(a: InvalidShotCounts, b: InvalidShotCounts) -> InvalidShotCounts {
    InvalidShotCounts {
        non_excluded_corrected: a.non_excluded_corrected + b.non_excluded_corrected,
        non_excluded_uncorrected: a.non_excluded_uncorrected + b.non_excluded_uncorrected,
        excluded_corrected: a.excluded_corrected + b.excluded_corrected,
        excluded_uncorrected: a.excluded_uncorrected + b.excluded_uncorrected,
    }
}

pub fn build_preview(parsed: &ParsedCompass) -> CompassFile {
    let shot_count = parsed
        .surveys
        .iter()
        .map(|s| s.survey.shots.len())
        .sum::<usize>() as u32;
    let excluded_shot_count_total = parsed
        .surveys
        .iter()
        .map(|s| excluded_shot_count(&s.survey))
        .sum::<u32>();
    let per_survey_invalid_counts: Vec<InvalidShotCounts> = parsed
        .surveys
        .iter()
        .map(|s| invalid_shot_counts(&s.survey, &s.state))
        .collect();
    let invalid_shot_counts_total = per_survey_invalid_counts
        .iter()
        .copied()
        .fold(InvalidShotCounts::default(), add_invalid_shot_counts);
    let has_backsights = parsed
        .surveys
        .iter()
        .any(|s| s.survey.parameters.format_parameters.has_backsights());
    let (survey_count, length_unit) = preview_scalars(parsed);
    let surveys = parsed
        .surveys
        .iter()
        .zip(per_survey_invalid_counts)
        .map(|(s, invalid_counts)| CompassSurveySummary {
            name: s.survey.name.clone(),
            date: s.survey.date.to_string(),
            team: non_empty(&s.survey.team),
            shot_count: s.survey.shots.len() as u32,
            excluded_shot_count: excluded_shot_count(&s.survey),
            invalid_shot_counts: invalid_counts,
            // Preview-only field, but it crosses IPC as a ts-rs `number`;
            // a NaN would serialize as JSON null (#275).
            declination: finite_or_zero(s.survey.parameters.declination),
        })
        .collect();
    CompassFile {
        cave_name: parsed.cave_name.clone(),
        survey_count,
        shot_count,
        excluded_shot_count: excluded_shot_count_total,
        invalid_shot_counts: invalid_shot_counts_total,
        has_backsights,
        datum: parsed.datum.map(|d| format!("{d:?}")),
        utm_zone: parsed.utm_zone.map(i32::from),
        has_georeference: !parsed.fixed_stations.is_empty() && parsed.utm_zone.is_some(),
        declination_mode: format!("{:?}", parsed.declination_mode),
        length_unit,
        surveys,
        warnings: parsed.warnings.clone(),
    }
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// Per-survey geometry context (corrections, units, declination, convergence).
struct SurveyCtx {
    /// What this survey's readings are measured against and what has to be
    /// ADDED to reach the frame geometry draws in (#438).
    ///
    /// Replaces the old `declination` / `convergence_angle` pair, which
    /// `shot_geometry` folded into every azimuth. The readings now land in the
    /// project exactly as the file recorded them and this rides on the section.
    /// The correction is the declination ALONE: the UTM grid convergence is
    /// reported as a warning instead of being folded in, because geometry is
    /// drawn in true north and Compass's own display frame is the grid.
    azimuth_reference: AzimuthReference,
    length_to_m: f64,
    apply_corrections: bool,
    corrections: Option<CorrectionFactors>,
    backsight_corrections: Option<BackSightCorrectionFactors>,
    /// Inclination unit used by this survey.  Compass supports five
    /// (Degrees, PercentGrade, DegreesAndMinutes, Grads, DepthGauge);
    /// `shot_geometry` converts each to plain degrees before feeding
    /// the trig in elevation_gain.  `None` (= unrecognized) is treated
    /// as Degrees with a warning at parse time.
    inclination_units: InclinationUnits,
}

/// Resolved georeference anchor for the project.
struct AnchorGeo {
    coordinates: GpsCoordinates,
    name: String,
    elevation: f64,
    lat_lng: (f64, f64),
}

#[derive(Default)]
struct ShotFlags {
    exclude_all: bool,
}

fn parse_flags(flags: Option<&String>) -> ShotFlags {
    let mut parsed = ShotFlags::default();
    if let Some(text) = flags {
        for c in text.chars() {
            if c.eq_ignore_ascii_case(&'X') {
                parsed.exclude_all = true;
            }
        }
    }
    parsed
}

/// Non-finite readings are treated as missing everywhere in this parser.
/// nom's `double` (used by the vendored compass_data crate) accepts literal
/// "nan"/"inf"/"infinity" tokens, and every sentinel guard below is a
/// comparison that NaN fails - so without an explicit is_finite() check a
/// corrupt or hostile .DAT would store NaN geometry that silently poisons
/// station coords, the closure solve, and the IPC payload (NaN serializes as
/// null where TS expects number). Mirrors the .tml parser's is_finite filter.
fn angle_missing(raw: f64) -> bool {
    !raw.is_finite() || raw <= MISSING_ANGLE_THRESHOLD || raw >= MISSING_VALUE_THRESHOLD
}

/// Length variant of the sentinel test: lengths (and DepthGauge depth
/// readings) use only the "+999" half of the sentinel range, plus the
/// shared non-finite guard.
fn length_missing(raw: f64) -> bool {
    !raw.is_finite() || raw >= MISSING_VALUE_THRESHOLD
}

fn lrud_meters(raw: f64, length_to_m: f64) -> Option<f64> {
    if !raw.is_finite() || raw <= -1.0 || raw >= MISSING_VALUE_THRESHOLD {
        return None;
    }
    let value = raw * length_to_m;
    if value.abs() < f64::EPSILON {
        None
    } else {
        Some(value)
    }
}

/// Corrected azimuth (grid, 0..360), slope length (m), inclination (deg, +up),
/// and elevation gain (m, +up) for a shot.
fn shot_geometry(shot: &Shot, ctx: &SurveyCtx) -> (f64, f64, f64) {
    // A missing or non-finite bearing collapses to 0.0 (due north); the shot
    // keeps its finite length and still participates in the closure solve as a
    // due-north leg rather than being dropped. The corruption is surfaced to the
    // user by the pass-1 per-shot warning (#275).
    let mut azimuth = if angle_missing(shot.azimuth) {
        0.0
    } else {
        shot.azimuth
    };
    // For Degrees / Grads / PercentGrade / DegreesAndMinutes the raw
    // value is sentinel-tested as an angle.  For DepthGauge the field
    // is a depth reading whose absolute value can equal the missing-
    // threshold, so only the "+ infinity" half of the sentinel applies
    // - pass the raw through.
    let mut inclination = match ctx.inclination_units {
        InclinationUnits::DepthGauge => {
            if length_missing(shot.inclination) {
                0.0
            } else {
                shot.inclination
            }
        }
        _ => {
            if angle_missing(shot.inclination) {
                0.0
            } else {
                shot.inclination
            }
        }
    };
    let mut length = if length_missing(shot.length) {
        0.0
    } else {
        shot.length
    };
    if ctx.apply_corrections {
        if let Some(cf) = &ctx.corrections {
            azimuth += cf.azimuth;
            // Corrections live in the survey's declared inclination
            // unit; they apply naturally for Degrees/Grads/Percent/DM.
            // For DepthGauge the field is a depth in length units;
            // adding the inclination-correction (also a length under
            // the DepthGauge convention per the Compass spec) is
            // dimensionally consistent.
            inclination += cf.inclination;
            length += cf.length;
        }
    }
    // #438: measured, not corrected. The survey's declination and convergence
    // live on `ctx.azimuth_reference` and are applied when geometry is computed.
    let azimuth_final = azimuth.rem_euclid(360.0);
    let length_m = length * ctx.length_to_m;
    // Single normalisation point: convert whatever Compass unit the
    // survey declared into degrees so the rest of the pipeline
    // (elevation_gain, shot_to_shot_data's pitch sign flip) stays
    // unit-agnostic.  DepthGauge needs the slope length to resolve
    // asin(depth / length) - pass the raw length (same units as the
    // depth reading) so the ratio is dimensionless.
    let inclination_deg = inclination_to_degrees(inclination, &ctx.inclination_units, length);
    // Backstop only: build_ctx sanitizes declination/convergence/corrections
    // and inclination_to_degrees clamps the DepthGauge asin ratio, so with
    // finite raw inputs these are already finite. 0.0 carries the same
    // missing semantics the raw sentinel guards above produce; the per-shot
    // corruption warning fires at the pass-1 scan, not here.
    (
        finite_or_zero(azimuth_final),
        finite_or_zero(length_m),
        finite_or_zero(inclination_deg),
    )
}

/// See `angle_missing`: the no-non-finite-output contract for file-controlled
/// float pipelines. 0.0 carries the same "missing" semantics the sentinel
/// guards produce.
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Process a back-sight pair (`back_azimuth`, `back_inclination`) and return
/// values comparable to the fore-sight after the same pipeline - i.e. in the
/// forward (FROM → TO) direction, Stalagmight's `+down` pitch convention.
///
/// Compass records back-sight as the raw instrument reading from the TO
/// station looking back at the FROM station, so for a well-calibrated survey
/// of a shot with fore-azimuth `a` and fore-inclination `i` (Compass +up):
///
///   * `back_azimuth`     ≈ a + 180°
///   * `back_inclination` ≈ -i   (uphill from FROM is downhill from TO)
///
/// We need to turn the back-sight around to FROM → TO:
///   * Azimuth: add 180° after declination/convergence.
///   * Inclination: convert through `inclination_to_degrees` first (the raw
///     reading is in the survey's declared unit - Grads / DDD.MM / Percent
///     Grade / DepthGauge - and only the Degrees case is pass-through).
///     Then flip sign to undo the TO-side reading direction (yielding
///     fore-direction in Compass's +up convention), and flip again to land
///     in Stalagmight's +down - the two flips cancel, so the final stored
///     `pitch_out` equals the degree-converted back-inclination.  Without
///     the unit conversion, Grads / DDD.MM / Percent surveys with
///     back-sights store `pitch_out` in the wrong units.
///
/// Returns `(azimuth_out_grid_deg, pitch_out_deg_positive_down)` if both
/// back-sight readings are present and non-sentinel, else `None`.
fn shot_backsight_geometry(shot: &Shot, ctx: &SurveyCtx) -> Option<(f64, f64)> {
    let back_az_raw = shot.back_azimuth?;
    let back_inc_raw = shot.back_inclination?;
    if angle_missing(back_az_raw) {
        return None;
    }
    let mut back_az = back_az_raw;
    let mut back_inc = if angle_missing(back_inc_raw) {
        0.0
    } else {
        back_inc_raw
    };
    // Same raw-length-with-fore-sight-corrections that `shot_geometry`
    // computes; needed by `inclination_to_degrees` for the DepthGauge
    // depth/length ratio.  Survey-level CORRECTIONS apply to the
    // physical slope distance regardless of sighting direction, so the
    // fore-sight `cf.length` is the right offset for the back-sight too.
    let mut slope_length_raw = if length_missing(shot.length) {
        0.0
    } else {
        shot.length
    };
    if ctx.apply_corrections {
        if let Some(bcf) = &ctx.backsight_corrections {
            back_az += bcf.azimuth;
            back_inc += bcf.inclination;
        }
        if let Some(cf) = &ctx.corrections {
            slope_length_raw += cf.length;
        }
    }
    // Measured, like the fore-sight (#438): no declination or convergence here.
    let back_az_measured = back_az.rem_euclid(360.0);
    // Flip 180° so the bearing points FROM → TO, matching azimuth_in.
    let azimuth_out_fore_direction = (back_az_measured + 180.0).rem_euclid(360.0);
    // Convert the back-inclination into degrees first - Grads / DDD.MM /
    // PercentGrade / DepthGauge are NOT pass-through.  After conversion,
    // back_inc_deg is the TO→FROM slope in Compass +up; the double sign
    // flip (TO→FROM ⇒ FROM→TO, then +up ⇒ +down) cancels, so the value
    // we store IS that degree-converted reading.
    let back_inc_deg = inclination_to_degrees(back_inc, &ctx.inclination_units, slope_length_raw);
    let pitch_out_down_positive = back_inc_deg;
    // Backstop: a computed non-finite backsight must become "no backsight"
    // (the caller mirrors the fore-sight, which is a no-op in the fore/back
    // circular mean), NEVER a fabricated due-north Some(0.0) that would
    // silently rotate every affected shot via average_azimuth (#275).
    // build_ctx sanitizes the header factors, so this should be unreachable.
    if !azimuth_out_fore_direction.is_finite() || !pitch_out_down_positive.is_finite() {
        return None;
    }
    Some((azimuth_out_fore_direction, pitch_out_down_positive))
}

fn elevation_gain(length_m: f64, inclination_deg: f64) -> f64 {
    length_m * inclination_deg.to_radians().sin()
}

/// Convert a single Compass shot to a Stalagmight `ShotData`, given the absolute
/// depths (meters, +down) already resolved for its endpoints.
fn shot_to_shot_data(
    shot: &Shot,
    ctx: &SurveyCtx,
    depth_in: f64,
    depth_out: f64,
    is_virtual: bool,
) -> ShotData {
    let (azimuth, length_m, inclination) = shot_geometry(shot, ctx);
    // If the survey took back-sight readings (separate from the fore-sight),
    // store them in the `_out` slots as forward-direction equivalents so
    // Stalagmight has both legs of the measurement available for averaging
    // and warnings.  When no back-sight is present, mirror the fore-sight
    // into the `_out` slot - matching the TML importer's convention.
    let (azimuth_out, pitch_out) = match shot_backsight_geometry(shot, ctx) {
        Some((back_az, back_pitch)) => (back_az, Some(back_pitch)),
        None => (azimuth, None),
    };
    ShotData {
        // Compass files carry no shot identity; every shot gets a fresh mint.
        uuid: None,
        azimuth_in: azimuth,
        azimuth_out,
        length: length_m,
        depth_in,
        depth_out,
        // Stalagmight pitch is positive downward; Compass inclination is positive up.
        pitch_in: Some(-inclination),
        pitch_out,
        left: lrud_meters(shot.left, ctx.length_to_m),
        right: lrud_meters(shot.right, ctx.length_to_m),
        up: lrud_meters(shot.up, ctx.length_to_m),
        down: lrud_meters(shot.down, ctx.length_to_m),
        comment: shot.comment.as_deref().and_then(non_empty),
        // Compass `X` (exclude-all) marks a leg the surveyor pulled out of
        // the computed cave. The chain classifier decides Jump vs Reference
        // at import finalize (#27 q10); Reference is the conservative seed.
        kind: if is_virtual {
            ShotKind::Reference
        } else {
            ShotKind::Normal
        },
        marker_index: None,
        temperature: None,
    }
}

fn build_ctx(
    survey: &Survey,
    file_state: &CompassFileState,
    anchor_lat_lng: Option<(f64, f64)>,
    opts: &CompassImportOptions,
    warnings: &mut Vec<String>,
) -> SurveyCtx {
    let format = &survey.parameters.format_parameters;
    let length_to_m = match format.length_units() {
        Some(LengthUnits::Meters) => 1.0,
        _ => FEET_TO_METERS,
    };
    let inclination_units = format
        .inclination_units()
        .unwrap_or(InclinationUnits::Degrees);
    if matches!(inclination_units, InclinationUnits::DepthGauge) {
        warnings.push(format!(
            "Survey '{}': inclination unit is DepthGauge (W); each shot's inclination \
             is interpreted as a vertical depth delta and converted to a slope angle \
             via asin(depth / length).  Depth-gauge surveys with zero-length shots \
             can't produce a slope and will be treated as horizontal.",
            survey.name
        ));
    }
    let (mut declination, declination_source) = survey_declination(
        file_state.declination_mode,
        survey,
        anchor_lat_lng,
        warnings,
    );
    let mut convergence_angle = if file_state.convergence_applied {
        file_state.convergence
    } else {
        0.0
    };
    let mut corrections = survey.parameters.correction_factors.clone();
    let mut backsight_corrections = survey.parameters.backsight_correction_factors.clone();
    // Header-level values are file-controlled floats too (nom's double
    // accepts nan/inf tokens). Sanitize them once per survey so every
    // downstream sum stays finite: a poisoned factor becomes 0.0 with a
    // warning instead of silently corrupting every shot in the survey.
    {
        let mut sanitize = |label: &str, v: &mut f64| {
            if !v.is_finite() {
                warnings.push(format!(
                    "Survey '{}': non-finite {label} treated as 0.",
                    survey.name
                ));
                *v = 0.0;
            }
        };
        sanitize("declination", &mut declination);
        sanitize("UTM convergence", &mut convergence_angle);
        if let Some(cf) = corrections.as_mut() {
            sanitize("azimuth correction", &mut cf.azimuth);
            sanitize("inclination correction", &mut cf.inclination);
            sanitize("length correction", &mut cf.length);
        }
        if let Some(bcf) = backsight_corrections.as_mut() {
            sanitize("backsight azimuth correction", &mut bcf.azimuth);
            sanitize("backsight inclination correction", &mut bcf.inclination);
        }
    }
    // #438: what the file said, recorded rather than baked in.
    //
    // The correction is the DECLINATION alone, not `declination - convergence`.
    // `correction` means "add this to reach TRUE north", and the app's world
    // frame is true north: `gps_to_local` returns true-north ENU
    // (`models/geo.rs:79-81`) and every georeferencing path - `set_zero_datum`,
    // `renormalize_frame`, `realign_after_shot_edit` - places datums with it and
    // compares them against the shot walk. A survey walked in GRID bearings
    // inside that frame is misaligned with its own GPS by the convergence, so
    // applying it here would be the same class of frame error #438 exists to
    // remove. Compass subtracts it because Compass's own display is the UTM
    // grid; ours is not.
    //
    // `north` is Magnetic because that is what the STORED numbers are: compass
    // readings. The convergence is reported below rather than applied.
    let azimuth_reference = AzimuthReference::magnetic(declination, declination_source);
    if convergence_angle.abs() > f64::EPSILON {
        warnings.push(format!(
            "Survey '{}': the file applies a {:.2} degree UTM grid convergence. Stalagmight \
             draws in true north, so the convergence was NOT applied - only the declination. \
             Bearings here differ from Compass's grid-relative display by that angle.",
            survey.name, convergence_angle
        ));
    }
    SurveyCtx {
        azimuth_reference,
        length_to_m,
        apply_corrections: opts.apply_corrections,
        corrections,
        backsight_corrections,
        inclination_units,
    }
}

/// Convert a Compass-format inclination value to plain degrees.
///
/// Compass stores inclinations in one of five units (declared in the
/// survey's format parameters).  The whole rest of `shot_geometry` /
/// `elevation_gain` operates in degrees, so this is the single
/// normalisation point.
///
/// * `Degrees` (D)              - pass through
/// * `Grads` (R)                - × 0.9 (400 grads / 360°)
/// * `PercentGrade` (G)         - atan(percent / 100)
/// * `DegreesAndMinutes` (M)    - DDD.MM packed, minutes in the
///                                fractional part (e.g. 12.34 → 12 + 34/60)
/// * `DepthGauge` (W)           - `raw` is a vertical depth reading
///                                (in the survey's length units), not an
///                                angle.  Convert to a slope via
///                                `asin(depth / slope_length)`.  The two
///                                values must be in the SAME units so
///                                the ratio is dimensionless; callers
///                                pass the raw (pre-meter-conversion)
///                                slope length to satisfy this.  If the
///                                length is zero we fall back to 0° (a
///                                parse-time warning has already fired
///                                for DepthGauge surveys).
fn inclination_to_degrees(raw: f64, units: &InclinationUnits, slope_length_raw: f64) -> f64 {
    match units {
        InclinationUnits::Degrees => raw,
        InclinationUnits::Grads => raw * 0.9,
        InclinationUnits::PercentGrade => (raw / 100.0).atan().to_degrees(),
        InclinationUnits::DegreesAndMinutes => {
            // DDD.MM format: integer part is degrees, fractional part *
            // 100 is minutes (so 12.34 means 12° 34').  Preserve sign.
            let sign = raw.signum();
            let abs = raw.abs();
            let degrees = abs.trunc();
            let minutes = (abs - degrees) * 100.0;
            sign * (degrees + minutes / 60.0)
        }
        InclinationUnits::DepthGauge => {
            if slope_length_raw.abs() < f64::EPSILON {
                0.0
            } else {
                // `raw` is a depth reading in the survey's length units
                // (feet for `D` length, meters for `M`).  `shot_geometry`
                // and `shot_backsight_geometry` deliberately pass the
                // pre-meter-conversion `length` so the ratio is
                // dimensionless regardless of which length unit the
                // survey declared.  Compass convention: positive depth =
                // down (target below origin); Compass inclination is +up;
                // hence a +5 ft depth reading corresponds to a negative
                // (downward) inclination.
                let depth = raw;
                let ratio = (depth / slope_length_raw).clamp(-1.0, 1.0);
                -ratio.asin().to_degrees()
            }
        }
    }
}

/// The survey's declination and where the number came from (#438: the
/// provenance is recorded on the section, so the UI can explain the value and a
/// later policy change knows which sections it may recompute).
fn survey_declination(
    mode: DeclinationMode,
    survey: &Survey,
    anchor_lat_lng: Option<(f64, f64)>,
    warnings: &mut Vec<String>,
) -> (f64, CorrectionSource) {
    match mode {
        // "Ignore" is an instruction not to correct, NOT a declaration that
        // zero is the true-north offset - the file is silent about what the
        // readings are off by. `Unknown` says exactly that, and the difference
        // is load-bearing at export: a declared correction votes in the
        // document's uniformity test, so calling this one FileDeclared dragged
        // any project mixing an Ignore survey with a real declination into the
        // rewrite-every-bearing path (#438).
        DeclinationMode::Ignore => (0.0, CorrectionSource::Unknown),
        DeclinationMode::Entered => (
            survey.parameters.declination,
            CorrectionSource::FileDeclared,
        ),
        DeclinationMode::Auto => match anchor_lat_lng {
            Some((lat, lng)) => {
                match crate::parsers::geo::igrf_declination(lat, lng, survey.date) {
                    Ok(value) => (
                        value,
                        CorrectionSource::IgrfAuto {
                            epoch_year: survey.date.year(),
                        },
                    ),
                    Err(reason) => {
                        warnings.push(format!(
                            "Survey '{}': auto declination failed ({reason}); using entered value",
                            survey.name
                        ));
                        (
                            survey.parameters.declination,
                            CorrectionSource::FileDeclared,
                        )
                    }
                }
            }
            None => {
                warnings.push(format!(
                    "Survey '{}': auto declination needs a georeference; using entered value",
                    survey.name
                ));
                (
                    survey.parameters.declination,
                    CorrectionSource::FileDeclared,
                )
            }
        },
    }
}

/// Map a Compass datum to a proj4rs datum/ellipsoid token, warning when only an
/// approximate (WGS84) fallback is available.
fn datum_proj_token(datum: Datum, warnings: &mut Vec<String>) -> String {
    match datum {
        Datum::WGS1984 => "datum=WGS84".to_string(),
        Datum::NorthAmerican1983 => "datum=NAD83".to_string(),
        Datum::NorthAmerican1927 => "datum=NAD27".to_string(),
        Datum::WGS1972 => "ellps=WGS72".to_string(),
        other => {
            warnings.push(format!(
                "Datum {other:?} is not directly supported; approximating as WGS84 (the GPS anchor may be offset)"
            ));
            "datum=WGS84".to_string()
        }
    }
}

/// Hemisphere inferred from the Compass datum, when the datum unambiguously
/// implies one.  Compass `.MAK` files don't store a hemisphere flag, so we
/// rely on the datum: most regional datums are bounded to a single
/// hemisphere.  WGS-84 / WGS-72 / Pulkovo (worldwide / cross-hemisphere)
/// fall through as `None` and the caller defaults to north.
fn hemisphere_from_datum(datum: Datum) -> Option<bool> {
    use Datum::*;
    match datum {
        // Unambiguously northern.
        NorthAmerican1927 | NorthAmerican1983 | European1950 | European1979
        | OrdinanceSurvey1936 | HongKong1963 | HuTzuShan | Tokyo | Oman | Adindan => Some(false),
        // Unambiguously southern.
        Australian1966 | Australian1984 | SouthAmerican1956 | SouthAmerican1969 | Arc1950
        | Cape | Geodetic1949 | CampAreaAstro => Some(true),
        // Worldwide / equatorial - caller defaults to north.
        WGS1984 | WGS1972 | Pulkovo1942 | Indian | Arc1960 => None,
    }
}

/// Convert a UTM easting/northing (meters) to WGS84 longitude/latitude in
/// degrees.
///
/// The Compass `.MAK` format records the UTM zone but not the hemisphere.
/// An earlier "project both, pick the smaller `|lat|`" heuristic was
/// pathological for northern caves at high latitude - a real anchor at
/// +60 °N projects to ~-30 °S under `+south`, whose absolute latitude is
/// smaller, so the heuristic silently flipped genuinely-northern surveys
/// onto a different continent.
///
/// Instead, we:
///   1. Try to infer the hemisphere from the Compass datum
///      (`hemisphere_from_datum`); regional datums like NAD83 / Australian
///      1984 lock the hemisphere with no ambiguity.
///   2. Default to **north** when the datum is worldwide (WGS-84 etc.).
///      Most Compass users survey in the northern hemisphere, and the
///      worst case for getting it wrong is a clearly-wrong latitude the
///      user can immediately see on the map.
///   3. If the north projection lands above 84 °N (outside UTM's design
///      range), fall back to the south projection - that's the one
///      ambiguous case where "default north" is statistically wrong.
fn utm_to_wgs84(
    easting: f64,
    northing: f64,
    zone: u8,
    datum: Datum,
    warnings: &mut Vec<String>,
) -> Option<(f64, f64)> {
    let token = datum_proj_token(datum, warnings);

    let project = |south: bool| {
        crate::parsers::geo::project_utm(easting, northing, zone as u32, south, &token)
    };

    // Strong signal: datum implies hemisphere.
    if let Some(use_south) = hemisphere_from_datum(datum) {
        return project(use_south);
    }

    // Worldwide datum: default to north.  Only switch to south when the
    // north projection produces a latitude outside UTM's normal range
    // (|lat| > 84°), which strongly implies the file actually encodes a
    // southern false-northing.  Warn either way so the user can verify
    // on the map.
    let north = project(false)?;
    if north.1.abs() <= 84.0 {
        if !(-80.0..=84.0).contains(&north.1) {
            warnings.push(format!(
                "UTM zone {zone} projected at latitude {:.1}° N (datum {datum:?} doesn't \
                 disambiguate hemisphere); verify on the map.",
                north.1,
            ));
        }
        return Some(north);
    }
    // North gave us an out-of-range latitude - try south.
    match project(true) {
        Some(south) => {
            warnings.push(format!(
                "UTM zone {zone} northing {northing:.0} projected as southern hemisphere \
                 (north projection out of range); verify on the map.",
            ));
            Some(south)
        }
        None => Some(north),
    }
}

fn resolve_anchor(parsed: &ParsedCompass, warnings: &mut Vec<String>) -> Option<AnchorGeo> {
    let station = parsed.fixed_stations.first()?;
    // Multi-entrance / split-survey MAK projects can declare more than
    // one fixed station.  We only use the first as the GPS anchor; the
    // rest are silently parsed but never projected.  Surface that
    // limitation so users with multiple `#project_station` entries
    // aren't surprised when only one zero-datum appears in the imported
    // project.
    if parsed.fixed_stations.len() > 1 {
        let extras: Vec<&str> = parsed
            .fixed_stations
            .iter()
            .skip(1)
            .map(|s| s.name.as_str())
            .collect();
        warnings.push(format!(
            "MAK declares {} fixed stations but only the first ('{}') is used as the GPS anchor; \
             the others are not imported as additional zero-datums: {}.",
            parsed.fixed_stations.len(),
            station.name,
            extras.join(", "),
        ));
    }
    // Prefer the anchor's own `#file` datum/zone, falling back to the
    // project-wide values (always populated from file 0).  This makes
    // multi-`#file` projects whose first fixed station lives outside
    // file 0 project in their own frame instead of file 0's.
    let zone = station.utm_zone.or(parsed.utm_zone)?;
    let datum = station.datum.or(parsed.datum).unwrap_or(Datum::WGS1984);
    let (lon, lat) = utm_to_wgs84(station.easting, station.northing, zone, datum, warnings)?;
    let coordinates = GpsCoordinates::new(lat, lon, 0.0)
        .map_err(|e| warnings.push(format!("Ignoring invalid GPS anchor: {e}")))
        .ok()?;
    Some(AnchorGeo {
        coordinates,
        name: station.name.clone(),
        elevation: station.elevation,
        lat_lng: (lat, lon),
    })
}

/// Kept shot with its survey context, retained for graph + section building.
struct KeptSurvey<'a> {
    survey: &'a Survey,
    ctx: SurveyCtx,
    shots: Vec<&'a Shot>,
}

/// Split a survey's kept shots into head-to-tail connected chains, breaking
/// whenever a shot's `from` station doesn't continue the previous shot's `to`.
fn split_into_chains(shots: &[&Shot]) -> Vec<Vec<usize>> {
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut prev_to: Option<&str> = None;
    for (i, shot) in shots.iter().enumerate() {
        let continues = prev_to == Some(shot.from.as_str());
        if !continues && !current.is_empty() {
            chains.push(std::mem::take(&mut current));
        }
        current.push(i);
        prev_to = Some(shot.to.as_str());
    }
    if !current.is_empty() {
        chains.push(current);
    }
    chains
}

/// Walk the station graph (built from elevation gains) assigning each station an
/// absolute elevation. The anchor (if any) seeds its component; other components
/// seed at zero.
fn build_elevation(
    edges: &[(String, String, f64)],
    anchor_name: Option<&str>,
    anchor_elevation: f64,
) -> HashMap<String, f64> {
    let mut adjacency: HashMap<&str, Vec<(&str, f64)>> = HashMap::new();
    for (from, to, gain) in edges {
        adjacency
            .entry(from.as_str())
            .or_default()
            .push((to.as_str(), *gain));
        adjacency
            .entry(to.as_str())
            .or_default()
            .push((from.as_str(), -*gain));
    }

    let mut elevation: HashMap<String, f64> = HashMap::new();
    let mut queue: VecDeque<&str> = VecDeque::new();

    if let Some(anchor) = anchor_name {
        if adjacency.contains_key(anchor) {
            elevation.insert(anchor.to_string(), anchor_elevation);
            queue.push_back(anchor);
        }
    }

    // Collect node names for deterministic component seeding.
    let mut nodes: Vec<&str> = adjacency.keys().copied().collect();
    nodes.sort_unstable();

    let mut node_cursor = 0;
    loop {
        // Drain the current BFS frontier.
        while let Some(node) = queue.pop_front() {
            let current = elevation.get(node).copied().unwrap_or(0.0);
            if let Some(neighbors) = adjacency.get(node) {
                for (neighbor, gain) in neighbors {
                    if !elevation.contains_key(*neighbor) {
                        elevation.insert((*neighbor).to_string(), current + gain);
                        queue.push_back(neighbor);
                    }
                }
            }
        }
        // Seed the next unvisited component at zero.
        while node_cursor < nodes.len() && elevation.contains_key(nodes[node_cursor]) {
            node_cursor += 1;
        }
        if node_cursor >= nodes.len() {
            break;
        }
        let seed = nodes[node_cursor];
        elevation.insert(seed.to_string(), 0.0);
        queue.push_back(seed);
    }

    elevation
}

/// Convert parsed Compass data into Stalagmight import configs + loop closures.
/// Returns `(configs, closures, warnings)`.
pub fn convert_compass_to_import_configs(
    parsed: &ParsedCompass,
    opts: &CompassImportOptions,
) -> Result<(Vec<ImportSectionConfig>, Vec<ImportClosure>, Vec<String>)> {
    let mut warnings = parsed.warnings.clone();

    let anchor = resolve_anchor(parsed, &mut warnings);
    let anchor_name = anchor.as_ref().map(|a| a.name.clone());
    let anchor_elevation = anchor.as_ref().map_or(0.0, |a| a.elevation);
    let anchor_lat_lng = anchor.as_ref().map(|a| a.lat_lng);
    let anchor_coordinates = anchor.as_ref().map(|a| a.coordinates);

    // Pass 1: per-survey contexts, kept shots, and elevation graph edges.
    let mut kept: Vec<KeptSurvey> = Vec::new();
    let mut edges: Vec<(String, String, f64)> = Vec::new();
    for survey_with_state in &parsed.surveys {
        let survey = &survey_with_state.survey;
        let ctx = build_ctx(
            survey,
            &survey_with_state.state,
            anchor_lat_lng,
            opts,
            &mut warnings,
        );
        let mut shots = Vec::new();
        for shot in &survey.shots {
            let flags = parse_flags(shot.flags.as_ref());
            if flags.exclude_all && !opts.include_excluded {
                continue;
            }
            // One warning per shot naming every corrupt (non-finite) raw
            // reading. Sentinel-coded missing values (-999/+999) stay
            // silent: they are the format's intended "no reading" markers,
            // not corruption. Absent backsights read as finite 0.0 here.
            let mut corrupt: Vec<&str> = Vec::new();
            for (name, value) in [
                ("length", shot.length),
                ("bearing", shot.azimuth),
                ("dip", shot.inclination),
                ("left", shot.left),
                ("up", shot.up),
                ("down", shot.down),
                ("right", shot.right),
                ("back bearing", shot.back_azimuth.unwrap_or(0.0)),
                ("back dip", shot.back_inclination.unwrap_or(0.0)),
            ] {
                if !value.is_finite() {
                    corrupt.push(name);
                }
            }
            if !corrupt.is_empty() {
                warnings.push(format!(
                    "Shot {} -> {}: non-finite {} treated as missing.",
                    shot.from,
                    shot.to,
                    corrupt.join(", ")
                ));
            }
            let (_, length_m, inclination) = shot_geometry(shot, &ctx);
            if length_m <= 0.0 {
                // Zero-length connectors can't be stored as section shots.
                // Dropping a shot removes its connectivity, so never do it
                // silently (#275).
                warnings.push(format!(
                    "Shot {} -> {}: dropped ({} length).",
                    shot.from,
                    shot.to,
                    if shot.length.is_finite() {
                        "missing or zero"
                    } else {
                        "non-finite"
                    }
                ));
                continue;
            }
            edges.push((
                shot.from.clone(),
                shot.to.clone(),
                elevation_gain(length_m, inclination),
            ));
            shots.push(shot);
        }
        kept.push(KeptSurvey { survey, ctx, shots });
    }

    let elevation = build_elevation(&edges, anchor_name.as_deref(), anchor_elevation);
    let depth_ref = anchor_name
        .as_deref()
        .and_then(|a| elevation.get(a))
        .copied()
        .unwrap_or(0.0);
    let depth_of = |station: &str| depth_ref - elevation.get(station).copied().unwrap_or(0.0);

    // Pass 2a: split every kept survey into head-to-tail chains and build the
    // per-section data (names, shots, label). Placement is decided in pass 2b
    // by a spanning-tree walk over chain connectivity, so it doesn't depend on
    // survey/file order (the previous one-pass first-occurrence registration
    // mis-placed every chain whose shared station was first defined in a
    // later-processed survey, dumping ~280 shots at origin in the
    // private real-cave corpus the spanning-tree placement was first
    // validated against).
    struct PendingChain {
        section_index: u32,
        names: Vec<String>,
        shots: Vec<ShotData>,
        station_comments: Vec<(u32, String)>,
        label: String,
        surveyor: Option<String>,
        date: chrono::NaiveDate,
        /// The survey's north reference (#438). Per-SECTION because Compass
        /// declares the declination per survey and can change mode mid-file.
        azimuth_reference: AzimuthReference,
    }
    let mut chains: Vec<PendingChain> = Vec::new();
    for kept_survey in &kept {
        let split = split_into_chains(&kept_survey.shots);
        let multi = split.len() > 1;
        for (chain_idx, chain) in split.iter().enumerate() {
            if chain.is_empty() {
                continue;
            }
            let section_index = chains.len() as u32;
            let mut names: Vec<String> = Vec::with_capacity(chain.len() + 1);
            names.push(kept_survey.shots[chain[0]].from.clone());
            for &idx in chain {
                names.push(kept_survey.shots[idx].to.clone());
            }
            let mut shots_data = Vec::with_capacity(chain.len());
            for &idx in chain {
                let shot = kept_survey.shots[idx];
                let flags = parse_flags(shot.flags.as_ref());
                shots_data.push(shot_to_shot_data(
                    shot,
                    &kept_survey.ctx,
                    depth_of(&shot.from),
                    depth_of(&shot.to),
                    flags.exclude_all,
                ));
            }
            // Compass convention: comments describe the FROM station (where the
            // surveyor was standing). When the option is on, promote each
            // non-empty shot comment to its FROM station (station_index = shot
            // index in the chain) and clear it from the shot.
            let mut station_comments: Vec<(u32, String)> = Vec::new();
            if opts.comments_on_stations {
                for (shot_idx, shot) in shots_data.iter_mut().enumerate() {
                    if let Some(comment) = shot.comment.take() {
                        if !comment.is_empty() {
                            station_comments.push((shot_idx as u32, comment));
                        }
                    }
                }
            }
            chains.push(PendingChain {
                section_index,
                names,
                shots: shots_data,
                station_comments,
                label: section_label(kept_survey.survey, multi, chain_idx),
                surveyor: non_empty(&kept_survey.survey.team),
                date: kept_survey.survey.date,
                azimuth_reference: kept_survey.ctx.azimuth_reference,
            });
        }
    }

    // Pass 2b: index all station-name occurrences across every chain, then
    // build the chain adjacency (two chains are neighbors iff they share a
    // station name) and BFS from the anchor chain to assign a spanning-tree
    // placement: anchor -> ZeroDatum, each reached chain -> FromImporting to
    // its discovering neighbor. Unreachable components are seeded at Origin
    // exactly once each (their members chain into them via FromImporting too).
    let mut occurrences: HashMap<String, Vec<(usize, u32)>> = HashMap::new();
    for (ci, chain) in chains.iter().enumerate() {
        for (k, name) in chain.names.iter().enumerate() {
            occurrences
                .entry(name.clone())
                .or_default()
                .push((ci, k as u32));
        }
    }
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); chains.len()];
    for occs in occurrences.values() {
        for a in 0..occs.len() {
            for b in (a + 1)..occs.len() {
                let (ia, _) = occs[a];
                let (ib, _) = occs[b];
                if ia != ib {
                    adjacency[ia].push(ib);
                    adjacency[ib].push(ia);
                }
            }
        }
    }
    for nbrs in adjacency.iter_mut() {
        nbrs.sort_unstable();
        nbrs.dedup();
    }

    // Find a station shared between two chains, returning (child_idx, parent_idx).
    let shared_station = |child: &PendingChain, parent: &PendingChain| -> Option<(u32, u32)> {
        for (ci, cname) in child.names.iter().enumerate() {
            if let Some(pi) = parent.names.iter().position(|pn| pn == cname) {
                return Some((ci as u32, pi as u32));
            }
        }
        None
    };

    let anchor_chain_idx = anchor_name
        .as_deref()
        .and_then(|a| chains.iter().position(|ch| ch.names.iter().any(|n| n == a)));

    let mut placement: Vec<Option<ImportPlacement>> = vec![None; chains.len()];
    let mut placement_merge_station: Vec<Option<u32>> = vec![None; chains.len()];
    let mut placement_parent: Vec<Option<usize>> = vec![None; chains.len()];
    let mut visited = vec![false; chains.len()];
    let mut creation_order: Vec<usize> = Vec::with_capacity(chains.len());
    let mut queue: VecDeque<usize> = VecDeque::new();

    if let (Some(ac), Some(coords)) = (anchor_chain_idx, anchor_coordinates) {
        let aname = anchor_name.as_deref().unwrap_or("");
        let aidx = chains[ac]
            .names
            .iter()
            .position(|n| n == aname)
            .unwrap_or(0) as u32;
        placement[ac] = Some(ImportPlacement::ZeroDatum {
            coordinates: coords,
            station_index: aidx,
        });
        visited[ac] = true;
        creation_order.push(ac);
        queue.push_back(ac);
    }

    let mut next_root_warns = anchor_chain_idx.is_some();
    loop {
        while let Some(parent_idx) = queue.pop_front() {
            // Borrow-checker dance: copy the neighbor list before iterating.
            let neighbors = adjacency[parent_idx].clone();
            for nb in neighbors {
                if visited[nb] {
                    continue;
                }
                if let Some((child_st, parent_st)) =
                    shared_station(&chains[nb], &chains[parent_idx])
                {
                    placement[nb] = Some(ImportPlacement::FromImporting {
                        import_station: child_st,
                        target_section_index: chains[parent_idx].section_index,
                        target_station_index: parent_st,
                    });
                    placement_merge_station[nb] = Some(child_st);
                    placement_parent[nb] = Some(parent_idx);
                }
                visited[nb] = true;
                creation_order.push(nb);
                queue.push_back(nb);
            }
        }
        // Seed the next disconnected component (if any) at Origin.
        let next = (0..chains.len()).find(|&i| !visited[i]);
        match next {
            Some(seed) => {
                placement[seed] = Some(ImportPlacement::Origin);
                if next_root_warns {
                    warnings.push(format!(
                        "Section '{}' starts a separate component not connected to the georeferenced survey; placed at origin",
                        chains[seed].label
                    ));
                }
                next_root_warns = true;
                visited[seed] = true;
                creation_order.push(seed);
                queue.push_back(seed);
            }
            None => break,
        }
    }

    // Pass 2c: emit loop closures for shared stations that aren't the
    // spanning-tree merge edge - pin each chain's other shared station to the
    // canonical (earliest by section_index) occurrence.
    let mut canonical: HashMap<&str, (u32, u32)> = HashMap::new();
    for chain in &chains {
        for (k, name) in chain.names.iter().enumerate() {
            canonical
                .entry(name.as_str())
                .and_modify(|prev| {
                    if chain.section_index < prev.0 {
                        *prev = (chain.section_index, k as u32);
                    }
                })
                .or_insert((chain.section_index, k as u32));
        }
    }
    let mut closures: Vec<ImportClosure> = Vec::new();
    let mut closure_keys: std::collections::HashSet<(u32, u32, u32, u32)> =
        std::collections::HashSet::new();
    for (ci, chain) in chains.iter().enumerate() {
        let merge_st = placement_merge_station[ci];
        let parent_section = placement_parent[ci].map(|p| chains[p].section_index);
        for (k, name) in chain.names.iter().enumerate() {
            if let Some(&(canon_sec, canon_st)) = canonical.get(name.as_str()) {
                if canon_sec == chain.section_index {
                    continue; // This chain owns the canonical occurrence.
                }
                // Skip the spanning-tree merge edge for THIS chain.
                if Some(k as u32) == merge_st && parent_section == Some(canon_sec) {
                    continue;
                }
                let key = (chain.section_index, k as u32, canon_sec, canon_st);
                if closure_keys.insert(key) {
                    closures.push(ImportClosure {
                        from_section_index: chain.section_index,
                        from_station_index: k as u32,
                        to_section_index: canon_sec,
                        to_station_index: canon_st,
                    });
                }
            }
        }
    }

    // Emit configs in BFS creation order so FromImporting targets always
    // precede their dependents at create time.
    let mut configs: Vec<ImportSectionConfig> = Vec::with_capacity(chains.len());
    for ci in creation_order {
        let chain = &chains[ci];
        let p = placement[ci].clone().unwrap_or(ImportPlacement::Origin);
        configs.push(ImportSectionConfig {
            reverse: false,
            section_index: chain.section_index,
            name: Some(chain.label.clone()),
            surveyor: chain.surveyor.clone(),
            date: Some(chain.date),
            color: SECTION_COLORS[chain.section_index as usize % SECTION_COLORS.len()].to_string(),
            direction: None,
            quality: 1.0,
            shots: chain.shots.clone(),
            placement: p,
            station_comments: chain.station_comments.clone(),
            station_names: Vec::new(),
            azimuth_reference: Some(chain.azimuth_reference),
        });
    }

    topological_sort_configs(&mut configs);

    Ok((configs, closures, warnings))
}

fn section_label(survey: &Survey, multi: bool, chain_idx: usize) -> String {
    // Prefer the survey name; fall back to the cave name; and if both
    // are blank, label as "Unnamed section" so the SectionsPanel never
    // renders an empty row (Compass files with empty header lines do
    // happen - partial exports, hand-edited DATs, etc.).
    let trimmed_name = survey.name.trim();
    let base = if !trimmed_name.is_empty() {
        trimmed_name.to_string()
    } else {
        let trimmed_cave = survey.cave_name.trim();
        if !trimmed_cave.is_empty() {
            trimmed_cave.to_string()
        } else {
            "Unnamed section".to_string()
        }
    };
    if multi {
        format!("{base} (branch {})", chain_idx + 1)
    } else {
        base
    }
}

/// Reorder configs so that any section referenced by a `FromImporting` placement
/// is created before its dependent (Kahn's algorithm; cycles fall back to
/// original order).
fn topological_sort_configs(configs: &mut Vec<ImportSectionConfig>) {
    if configs.len() <= 1 {
        return;
    }
    let pos_of: HashMap<u32, usize> = configs
        .iter()
        .enumerate()
        .map(|(i, c)| (c.section_index, i))
        .collect();
    let dep: Vec<Option<usize>> = configs
        .iter()
        .map(|c| match &c.placement {
            ImportPlacement::FromImporting {
                target_section_index,
                ..
            } => pos_of.get(target_section_index).copied(),
            _ => None,
        })
        .collect();

    // Ready-queue Kahn's: build a reverse-dependency adjacency list so
    // each placed config can directly enqueue the configs that were
    // waiting on it.  Replaces an outer-loop-scan-everything version
    // that ran O(N²) for deeply-chained surveys (one section per pass
    // dependency).
    //
    // children[i] = indices of configs whose `dep` is Some(i).
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); configs.len()];
    let mut placed = vec![false; configs.len()];
    let mut order: Vec<usize> = Vec::with_capacity(configs.len());
    let mut ready: VecDeque<usize> = VecDeque::new();
    for (i, d) in dep.iter().enumerate() {
        match d {
            Some(target) if *target < configs.len() => children[*target].push(i),
            // Root: no dependency, immediately ready.
            None => ready.push_back(i),
            // Dependency points outside the slice - treat as ready (the
            // legacy fallback was the same).
            Some(_) => ready.push_back(i),
        }
    }
    while let Some(i) = ready.pop_front() {
        if placed[i] {
            continue;
        }
        placed[i] = true;
        order.push(i);
        for &child in &children[i] {
            if !placed[child] {
                ready.push_back(child);
            }
        }
    }
    // Append any remaining (cyclic) configs in original order.
    for i in 0..configs.len() {
        if !placed[i] {
            order.push(i);
        }
    }

    let mut reordered: Vec<Option<ImportSectionConfig>> = configs.drain(..).map(Some).collect();
    for idx in order {
        if let Some(config) = reordered[idx].take() {
            configs.push(config);
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/compass")
            .join(name)
    }

    fn total_shots(configs: &[ImportSectionConfig]) -> usize {
        configs.iter().map(|c| c.shots.len()).sum()
    }

    // The private_cave_* tests below exercise the importer end-to-end against
    // a real-cave Compass corpus.  That corpus is private survey data and is
    // not committed; place `PrivateCave.MAK` (and the .DAT files it
    // references) under `src-tauri/tests/fixtures/compass/` locally and
    // re-run with `cargo test -- --ignored` to enable them.

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_mak_reads_georeference() {
        let parsed = parse_compass(&fixture("PrivateCave.MAK")).unwrap();
        assert!(!parsed.surveys.is_empty());
        assert!(parsed.datum.is_some());
        assert!(parsed.utm_zone.is_some());
        assert!(
            !parsed.fixed_stations.is_empty(),
            "expected at least one fixed station to anchor the project"
        );
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_first_shot_geometry_is_depth_based() {
        let parsed = parse_compass(&fixture("PrivateCave.MAK")).unwrap();
        let (configs, _closures, _warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        assert!(!configs.is_empty());

        // The private corpus uses a survey FORMAT whose 4th character is `W`
        // (DepthGauge), so each shot's "inclination" column is actually a
        // vertical depth offset in the shot's length units, not a slope
        // angle in degrees.  The pipeline converts it via
        // `-asin(depth / length)` into a Compass +up slope which Stalagmight
        // then stores as +down pitch.
        let anchor_config = configs
            .iter()
            .find(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. }))
            .expect("a section anchored by zero datum");
        if let ImportPlacement::ZeroDatum { station_index, .. } = anchor_config.placement {
            assert_eq!(station_index, 0);
        }
        let shot = &anchor_config.shots[0];
        // DepthGauge surveys produce a finite, non-trivial pitch out of a
        // raw "inclination" column that would otherwise be near-zero.
        assert!(shot.depth_in.abs() < 0.01, "depth_in {}", shot.depth_in);
        assert!(
            shot.pitch_in.is_some_and(|p| p.abs() > 0.5),
            "expected a meaningful DepthGauge-derived pitch, got {:?}",
            shot.pitch_in
        );

        // Exactly one zero datum for the whole project.
        let zero_datums = configs
            .iter()
            .filter(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. }))
            .count();
        assert_eq!(zero_datums, 1);
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_zero_datum_projects_to_plausible_coords() {
        let parsed = parse_compass(&fixture("PrivateCave.MAK")).unwrap();
        let (configs, _closures, _warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        let coords = configs
            .iter()
            .find_map(|c| match c.placement {
                ImportPlacement::ZeroDatum { coordinates, .. } => Some(coordinates),
                _ => None,
            })
            .expect("zero datum coordinates");
        // The fixture's anchor must reach `GpsCoordinates::new` with a
        // finite, in-range result - the actual lat/lon is intentionally
        // unchecked here so this test doesn't pin a real location.
        assert!(
            coords.latitude.is_finite() && coords.latitude.abs() < 90.0,
            "lat {} out of range",
            coords.latitude
        );
        assert!(
            coords.longitude.is_finite() && coords.longitude.abs() < 180.0,
            "lng {} out of range",
            coords.longitude
        );
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_excluded_shots_are_filtered_by_default() {
        let parsed = parse_compass(&fixture("PrivateCave.MAK")).unwrap();
        let (default_configs, _, _) = convert_compass_to_import_configs(
            &parsed,
            &CompassImportOptions {
                include_excluded: false,
                apply_corrections: true,
                comments_on_stations: true,
            },
        )
        .unwrap();
        let (included_configs, _, _) = convert_compass_to_import_configs(
            &parsed,
            &CompassImportOptions {
                include_excluded: true,
                apply_corrections: true,
                comments_on_stations: true,
            },
        )
        .unwrap();
        // The fixture contains at least one survey that is entirely
        // `#|X#`-flagged; including excluded shots must add shots, which
        // are imported as reference shots.
        assert!(total_shots(&included_configs) > total_shots(&default_configs));
        assert!(included_configs
            .iter()
            .any(|c| c.shots.iter().any(|s| s.kind.is_virtual())));
    }

    /// #393 item 1: `execute_compass_import` now reads `(survey_count,
    /// length_unit)` from `preview_scalars` instead of running the full
    /// `build_preview` a second time. This pins that `preview_scalars` never
    /// drifts from what `build_preview` itself reports for the same parse -
    /// an edit to `build_preview`'s survey-count or length-unit logic that
    /// forgot to update `preview_scalars` (or vice versa) fails here, not
    /// silently in the receipt.
    #[test]
    fn preview_scalars_matches_build_preview_survey_count_and_length_unit() {
        for name in ["simple.dat", "backsight_grads.dat", "multi_trip.dat"] {
            let parsed = parse_compass(&fixture(name)).unwrap();
            let (survey_count, length_unit) = preview_scalars(&parsed);
            let preview = build_preview(&parsed);
            assert_eq!(
                survey_count, preview.survey_count,
                "{name}: survey_count must match build_preview"
            );
            assert_eq!(
                length_unit, preview.length_unit,
                "{name}: length_unit must match build_preview"
            );
        }
    }

    /// #437 (+ review of #550): `build_preview`'s `shot_count` was the raw
    /// shot count, but the converter has two drop paths, and the preview's
    /// accounting of the second one used to be exact for only ONE of the
    /// four `(include_excluded, apply_corrections)` toggle states:
    ///
    /// - exclusion drops `X`-flagged shots when `!include_excluded`;
    /// - the length check (pass 1, unconditional on whichever shots reach
    ///   it) drops any shot whose resolved length is non-positive, and that
    ///   resolution itself depends on `apply_corrections` (`CORRECTIONS`'s
    ///   length term can push a length across zero either direction) - so a
    ///   shot can be BOTH excluded AND length-invalid, or invalid under one
    ///   corrections setting and not the other.
    ///
    /// The fixture spans two surveys (Compass shares one `CORRECTIONS` line
    /// per survey, and a single additive term can only push a length across
    /// zero in one direction) so all four counters are independently
    /// exercised, none left at a coincidental zero a broken counter could
    /// hide behind:
    ///
    /// - survey A (`CORRECTIONS` length `+0.20`): shot 4 (`A4->A5`) is
    ///   non-excluded with `LENGTH 0.00`, invalid uncorrected but valid once
    ///   `+0.20` lands (-> `non_excluded_uncorrected`); shot 5 (`A5->A6`) is
    ///   `#|X#`-flagged AND `LENGTH -1.00`, invalid either way once included
    ///   (-> `excluded_corrected` AND `excluded_uncorrected`).
    /// - survey B (`CORRECTIONS` length `-0.20`): shot 1 (`B1->B2`) is
    ///   non-excluded with `LENGTH 0.15`, valid uncorrected but invalid once
    ///   `-0.20` lands (-> `non_excluded_corrected`).
    ///
    /// The landed count is asserted against the preview in all four toggle
    /// combinations.
    ///
    /// Injection: zero any ONE of the four `InvalidShotCounts` fields (or
    /// `excluded_shot_count`) and the toggle combination(s) that field feeds
    /// go red - each state's assertion isolates its own counter.
    #[test]
    fn preview_shot_counts_match_the_landed_count_in_all_four_toggle_states() {
        let dir = tempfile::tempdir().unwrap();
        let survey_a = "Synthetic Cave\r\n\
             SURVEY NAME: A\r\n\
             SURVEY DATE: 7 10 1979\r\n\
             SURVEY TEAM:\r\n\
             Nobody\r\n\
             DECLINATION: 0.00  FORMAT: DMMDLRUDLADN  CORRECTIONS: 0.00 0.00 0.20\r\n\
             \r\n\
             FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\
             \r\n\
             A1 A2 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n\
             A2 A3 10.00 90.00 0.00 1.00 2.00 3.00 4.00 #|X#\r\n\
             A3 A4 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n\
             A4 A5 0.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n\
             A5 A6 -1.00 90.00 0.00 1.00 2.00 3.00 4.00 #|X#\r\n";
        let survey_b = "Synthetic Cave\r\n\
             SURVEY NAME: B\r\n\
             SURVEY DATE: 7 10 1979\r\n\
             SURVEY TEAM:\r\n\
             Nobody\r\n\
             DECLINATION: 0.00  FORMAT: DMMDLRUDLADN  CORRECTIONS: 0.00 0.00 -0.20\r\n\
             \r\n\
             FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\
             \r\n\
             B1 B2 0.15 90.00 0.00 1.00 2.00 3.00 4.00\r\n";
        let dat = format!("{survey_a}\x0c\r\n{survey_b}\x0c\r\n");
        let path = dir.path().join("excluded.dat");
        std::fs::write(&path, &dat).unwrap();

        let parsed = parse_compass(&path).unwrap();
        let preview = build_preview(&parsed);
        assert_eq!(preview.shot_count, 6, "raw shot count across both surveys");
        assert_eq!(
            preview.excluded_shot_count, 2,
            "shots 2 and 5 in survey A are #|X#-flagged"
        );
        let counts = preview.invalid_shot_counts;
        assert_eq!(
            counts.non_excluded_corrected, 1,
            "survey B shot 1's 0.15 - 0.20 = -0.05, invalid with corrections"
        );
        assert_eq!(
            counts.non_excluded_uncorrected, 1,
            "survey A shot 4's raw LENGTH 0.00 is invalid without corrections"
        );
        assert_eq!(
            counts.excluded_corrected, 1,
            "survey A shot 5's -1.00 + 0.20 = -0.80 stays invalid with corrections"
        );
        assert_eq!(
            counts.excluded_uncorrected, 1,
            "survey A shot 5's raw -1.00 is invalid without corrections"
        );
        assert_eq!(preview.surveys.len(), 2);
        assert_eq!(preview.surveys[0].name, "A");
        assert_eq!(preview.surveys[0].excluded_shot_count, 2);
        assert_eq!(
            preview.surveys[0].invalid_shot_counts,
            InvalidShotCounts {
                non_excluded_corrected: 0,
                non_excluded_uncorrected: 1,
                excluded_corrected: 1,
                excluded_uncorrected: 1,
            }
        );
        assert_eq!(preview.surveys[1].name, "B");
        assert_eq!(preview.surveys[1].excluded_shot_count, 0);
        assert_eq!(
            preview.surveys[1].invalid_shot_counts,
            InvalidShotCounts {
                non_excluded_corrected: 1,
                non_excluded_uncorrected: 0,
                excluded_corrected: 0,
                excluded_uncorrected: 0,
            }
        );

        // Exact landing arithmetic for the given toggles, mirroring what the
        // dialog computes from these same fields.
        let landing = |include_excluded: bool, apply_corrections: bool| -> u32 {
            let non_excluded_invalid = if apply_corrections {
                counts.non_excluded_corrected
            } else {
                counts.non_excluded_uncorrected
            };
            let excluded_invalid = if apply_corrections {
                counts.excluded_corrected
            } else {
                counts.excluded_uncorrected
            };
            preview.shot_count
                - if include_excluded {
                    0
                } else {
                    preview.excluded_shot_count
                }
                - non_excluded_invalid
                - if include_excluded {
                    excluded_invalid
                } else {
                    0
                }
        };

        for include_excluded in [false, true] {
            for apply_corrections in [false, true] {
                let (configs, _, _) = convert_compass_to_import_configs(
                    &parsed,
                    &CompassImportOptions {
                        include_excluded,
                        apply_corrections,
                        comments_on_stations: true,
                    },
                )
                .unwrap();
                assert_eq!(
                    total_shots(&configs) as u32,
                    landing(include_excluded, apply_corrections),
                    "include_excluded={include_excluded} apply_corrections={apply_corrections}: \
                     landed count must match the honest preview count"
                );
            }
        }
    }

    #[test]
    fn parse_dat_captures_backsights_flags_and_comments() {
        // simple.dat has a backsight format and flag/comment columns.
        let parsed = parse_compass(&fixture("simple.dat")).unwrap();
        let survey = &parsed.surveys[0].survey;
        assert!(survey.parameters.format_parameters.has_backsights());
        assert!(survey.shots[0].back_azimuth.is_some());
        assert!(survey
            .shots
            .iter()
            .any(|s| s.flags.as_deref() == Some("PC")));
        assert!(survey.shots.iter().any(|s| s.comment.is_some()));
    }

    /// #275 B4: nom's `double` (vendored compass_data) accepts literal
    /// nan/inf/-inf tokens, and every sentinel guard is a comparison NaN
    /// fails - so a corrupt or hostile .DAT could store NaN geometry that
    /// poisons station coords, the closure solve, and the IPC payload.
    /// nonfinite.dat carries nan/inf in length, bearing, dip, all four LRUD,
    /// and the backsight pair; every stored value must come out finite, with
    /// the poisoned fields treated as missing (the same semantics the
    /// existing -999/+999 sentinels get).
    #[test]
    fn nonfinite_dat_values_are_coerced_to_missing_never_stored() {
        let parsed = parse_compass(&fixture("nonfinite.dat")).unwrap();
        // All six rows parse: nom's double accepts bare "nan"/"inf"/"infinity"
        // tokens (unsigned only - a signed "-inf" fails the vendor parser and
        // terminates the shot list; pinned by
        // signed_nonfinite_token_truncates_shot_list).
        assert_eq!(parsed.surveys[0].survey.shots.len(), 6);
        let (configs, _closures, warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();

        // Corruption is never silent: one warning per poisoned shot naming
        // the fields, and one per dropped shot naming the reason (#275).
        let has = |needle: &str| warnings.iter().any(|w| w.contains(needle));
        assert!(has("Shot N2 -> N3: non-finite length"), "{warnings:?}");
        assert!(
            has("Shot N3 -> N4: non-finite bearing, back bearing"),
            "{warnings:?}"
        );
        assert!(
            has("Shot N4 -> N5: non-finite dip, back dip"),
            "{warnings:?}"
        );
        assert!(has("Shot N5 -> N6: non-finite length"), "{warnings:?}");
        assert!(
            has("Shot N6 -> N7: non-finite left, up, down, right"),
            "{warnings:?}"
        );
        assert!(
            has("Shot N2 -> N3: dropped (non-finite length)"),
            "{warnings:?}"
        );
        assert!(
            has("Shot N5 -> N6: dropped (non-finite length)"),
            "{warnings:?}"
        );

        let shots: Vec<&ShotData> = configs.iter().flat_map(|c| c.shots.iter()).collect();
        assert!(!shots.is_empty(), "fixture should keep at least one shot");

        for (i, s) in shots.iter().enumerate() {
            assert!(s.length.is_finite(), "shot {i} length: {}", s.length);
            assert!(
                s.azimuth_in.is_finite(),
                "shot {i} azimuth_in: {}",
                s.azimuth_in
            );
            assert!(
                s.azimuth_out.is_finite(),
                "shot {i} azimuth_out: {}",
                s.azimuth_out
            );
            assert!(s.depth_in.is_finite(), "shot {i} depth_in: {}", s.depth_in);
            assert!(
                s.depth_out.is_finite(),
                "shot {i} depth_out: {}",
                s.depth_out
            );
            for (name, v) in [
                ("pitch_in", s.pitch_in),
                ("pitch_out", s.pitch_out),
                ("left", s.left),
                ("right", s.right),
                ("up", s.up),
                ("down", s.down),
            ] {
                assert!(
                    v.is_none_or(f64::is_finite),
                    "shot {i} {name}: {v:?} must be finite or dropped"
                );
            }
        }

        // The nan-LRUD row (N6->N7) must have dropped all four dimensions,
        // exactly as a -9.9/999 sentinel would.
        // FORMAT declares decimal feet, so the 9.00 ft row stores 2.7432 m.
        let n6 = shots
            .iter()
            .find(|s| (s.length - 9.0 * 0.3048).abs() < 0.01)
            .expect("the 9.00 ft (2.7432 m) shot should survive with finite length");
        assert!(
            n6.left.is_none() && n6.up.is_none() && n6.down.is_none() && n6.right.is_none(),
            "nan/inf LRUD must be treated as missing: {:?}",
            (n6.left, n6.up, n6.down, n6.right)
        );

        // nan/inf LENGTH rows collapse to 0 length and are dropped as
        // zero-length connectors, not stored.
        assert!(
            shots.iter().all(|s| s.length > 0.0),
            "no zero/negative-length shot may be stored"
        );
    }

    /// #275: pins how a .MAK with a non-finite fixed-station UTM coordinate
    /// behaves today. The vendored PROJECT lexer has its own digit-only
    /// parse_double (unlike the .DAT survey parser's nom `double`), so a
    /// literal "nan" in a station fix fails the whole .mak parse with a clear
    /// error - it cannot silently reach geometry. The is_finite guard in
    /// parse_mak (fixed-station skip + warning) is therefore defense-in-depth
    /// for a vendor bump; if this test ever sees the file PARSE, that guard
    /// becomes load-bearing and this pin must flip to asserting the skip.
    #[test]
    fn nonfinite_mak_station_fix_fails_parse_never_silent() {
        let err = parse_compass(&fixture("nonfinite.mak"))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("Invalid station fix"),
            "a nan station fix must fail the vendored grammar; got: {err:?}"
        );
    }

    /// #275: header-level DECLINATION / CORRECTIONS2 values are file-controlled
    /// floats too. A non-finite factor is sanitized to 0 once per survey (with
    /// a warning), so the shot geometry comes through unchanged instead of the
    /// whole survey collapsing to fabricated due-north bearings.
    #[test]
    fn nonfinite_header_declination_and_correction_sanitized_with_warning() {
        let parsed = parse_compass(&fixture("nonfinite_header.dat")).unwrap();
        let (configs, _closures, warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        let shots: Vec<&ShotData> = configs.iter().flat_map(|c| c.shots.iter()).collect();
        assert_eq!(shots.len(), 1);
        let s = shots[0];
        // nan DECLINATION sanitizes to 0: the 90 deg bearing is unchanged.
        assert!(
            (s.azimuth_in - 90.0).abs() < 1e-9,
            "azimuth_in: {}",
            s.azimuth_in
        );
        // The backsight survives with the nan CORRECTIONS2 azimuth factor
        // sanitized to 0: (270 + 180) % 360 = 90, never a fabricated 0.0.
        assert!(
            (s.azimuth_out - 90.0).abs() < 1e-9,
            "azimuth_out: {}",
            s.azimuth_out
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("non-finite declination")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("non-finite backsight azimuth correction")),
            "{warnings:?}"
        );
    }

    /// #275: pins the vendored parser's behavior on SIGNED non-finite tokens.
    /// nom's `double` exception branch accepts only unsigned nan/inf/infinity;
    /// a literal "-inf" fails the row parse and truncates the survey's shot
    /// list at that row (silent data loss at the vendor grammar layer). If a
    /// vendor bump changes this, this test fails and the non-finite handling
    /// must be re-audited.
    #[test]
    fn signed_nonfinite_token_truncates_shot_list() {
        let parsed = parse_compass(&fixture("nonfinite_signed.dat")).unwrap();
        assert_eq!(
            parsed.surveys[0].survey.shots.len(),
            1,
            "the -inf row and everything after it are dropped by the vendored parser"
        );
    }

    #[test]
    fn utm_to_wgs84_matches_pyproj_reference() {
        // Reference point with no real-world cave attached: UTM Zone 31N's
        // false-easting / false-northing origin (easting=500000, northing=0)
        // is by construction the central-meridian / equator intersection.
        // Zone 31's central meridian is +3°, so EPSG:32631 → EPSG:4326
        // must round-trip to exactly (lon=3, lat=0).
        let mut warnings = Vec::new();
        let (lon, lat) = utm_to_wgs84(500_000.0, 0.0, 31, Datum::WGS1984, &mut warnings).unwrap();
        assert!(lat.abs() < 1e-6, "lat {lat}");
        assert!((lon - 3.0).abs() < 1e-6, "lon {lon}");
    }

    #[test]
    fn utm_to_wgs84_high_northing_wgs84_picks_north() {
        // Regression for the "smaller |lat| wins" bug Codex flagged:
        // a northern cave at +60° lat in Zone 30 (false-easting=500000,
        // northing≈6_700_000) projects to ~+60° N when read as northern,
        // and to ~-30° S when wrongly read as southern.  The earlier
        // heuristic picked south because |-30| < |+60|; the new
        // datum-defaulting heuristic must pick NORTH for WGS-84.
        let mut warnings = Vec::new();
        let (_lon, lat) =
            utm_to_wgs84(500_000.0, 6_700_000.0, 30, Datum::WGS1984, &mut warnings).unwrap();
        assert!(lat > 55.0 && lat < 65.0, "expected lat ≈ +60°, got {lat}");
    }

    #[test]
    fn utm_to_wgs84_southern_datum_forces_south() {
        // Genuinely-southern cave with an unambiguous southern datum
        // (e.g. Australian 1984) must project as southern regardless of
        // the worldwide-datum fallback heuristic.  Zone 55 central
        // meridian is +147°; northing=6_300_000 under +south corresponds
        // to ≈ -33° S (close to Sydney's latitude).
        let mut warnings = Vec::new();
        let (lon, lat) = utm_to_wgs84(
            500_000.0,
            6_300_000.0,
            55,
            Datum::Australian1984,
            &mut warnings,
        )
        .unwrap();
        assert!(lat < -25.0 && lat > -40.0, "expected lat ≈ -33°, got {lat}");
        assert!((lon - 147.0).abs() < 1.0, "expected lon ≈ +147°, got {lon}");
    }

    #[test]
    fn utm_to_wgs84_north_american_datum_forces_north() {
        // NAD83 unambiguously implies northern hemisphere; the
        // hemisphere-from-datum signal must short-circuit the worldwide
        // fallback and project as north even for high northings.
        let mut warnings = Vec::new();
        let (_lon, lat) = utm_to_wgs84(
            500_000.0,
            6_700_000.0,
            12,
            Datum::NorthAmerican1983,
            &mut warnings,
        )
        .unwrap();
        assert!(lat > 55.0 && lat < 65.0, "expected lat ≈ +60°, got {lat}");
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_comments_promoted_to_from_station_by_default() {
        // Compass convention: a shot comment describes the FROM station (the
        // point the surveyor was standing at).  With comments_on_stations on
        // (the default), each non-empty shot comment should become a
        // station_comment at station_index = shot index, and the shot's own
        // comment should be cleared.  The private fixture's first shot
        // carries an entrance note on its FROM station; this test asserts
        // that note migrates to station 0 of the anchor section.
        let parsed = parse_compass(&fixture("PrivateCave.MAK")).unwrap();
        let (promoted, _, _) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        let anchor = promoted
            .iter()
            .find(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. }))
            .expect("zero datum section");
        assert!(
            anchor.shots[0].comment.is_none(),
            "first shot's comment should be promoted off the shot, got {:?}",
            anchor.shots[0].comment
        );
        let station_zero_comment = anchor
            .station_comments
            .iter()
            .find_map(|(idx, text)| (*idx == 0).then_some(text.as_str()));
        assert!(
            station_zero_comment.is_some_and(|s| !s.is_empty()),
            "station 0 should carry a non-empty promoted comment, got {:?}",
            station_zero_comment
        );

        // With the option off, comments stay on the shot and station_comments
        // is empty for the section.
        let (kept_on_shots, _, _) = convert_compass_to_import_configs(
            &parsed,
            &CompassImportOptions {
                include_excluded: false,
                apply_corrections: true,
                comments_on_stations: false,
            },
        )
        .unwrap();
        let anchor_raw = kept_on_shots
            .iter()
            .find(|c| matches!(c.placement, ImportPlacement::ZeroDatum { .. }))
            .expect("zero datum section");
        assert!(anchor_raw.station_comments.is_empty());
        assert!(
            anchor_raw.shots[0]
                .comment
                .as_deref()
                .is_some_and(|s| !s.is_empty()),
            "first shot should retain its non-empty comment when promotion is off"
        );
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_every_section_connects_to_the_zero_datum() {
        // Regression guard for the spanning-tree placement: every Compass
        // section in the private corpus must attach to the network through a
        // station-name path back to the zero datum (1 ZeroDatum + the rest
        // FromImporting), with no chains stranded at origin.  A one-pass
        // first-occurrence placement previously left ~20 sections detached
        // because their connection station was first defined in a
        // later-processed survey.
        let parsed = parse_compass(&fixture("PrivateCave.MAK")).unwrap();
        let (configs, _closures, warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        let mut zero = 0;
        let mut origin = 0;
        for c in &configs {
            match c.placement {
                ImportPlacement::ZeroDatum { .. } => zero += 1,
                ImportPlacement::Origin => origin += 1,
                _ => {}
            }
        }
        assert_eq!(zero, 1, "exactly one zero datum expected");
        assert_eq!(origin, 0, "no section should be stranded at origin");
        assert!(
            warnings.iter().all(|w| !w.contains("origin")),
            "no 'placed at origin' warnings expected, got: {warnings:?}"
        );
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_corpus_converts_without_error() {
        let parsed = parse_compass(&fixture("PrivateCave.MAK")).unwrap();
        let preview = build_preview(&parsed);
        assert!(preview.has_georeference);
        assert!(preview.survey_count >= 3);
        let (configs, _closures, _warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        assert!(!configs.is_empty());
        // Every section is linear and non-empty.
        assert!(configs.iter().all(|c| !c.shots.is_empty()));
    }

    #[test]
    fn multi_file_mak_warns_on_per_file_divergence() {
        // multi_zone.mak declares two `#file` entries; file 0 uses
        // NAD-1983 / UTM 13 / declination=Entered (default), file 1
        // switches to WGS 1984 / UTM 14 / declination=Auto.  parse_mak
        // mirrors file 0's values into the project-wide preview fields
        // and emits one warning per divergence so the user knows the
        // preview only reflects file 0.  Geometry is resolved per-file
        // through `SurveyWithState.state` regardless of these warnings.
        let parsed = parse_compass(&fixture("multi_zone.mak")).unwrap();
        // Project-wide fields take file 0 values for preview.
        assert_eq!(parsed.datum, Some(Datum::NorthAmerican1983));
        assert_eq!(parsed.utm_zone, Some(13));
        // One warning per divergence (datum, utm_zone, declination_mode).
        let divergence_warnings: Vec<_> = parsed
            .warnings
            .iter()
            .filter(|w| w.starts_with("File #2"))
            .collect();
        assert_eq!(
            divergence_warnings.len(),
            3,
            "expected 3 divergence warnings, got: {:?}",
            parsed.warnings
        );
        assert!(divergence_warnings.iter().any(|w| w.contains("DATUM")));
        assert!(divergence_warnings.iter().any(|w| w.contains("UTM zone")));
        assert!(divergence_warnings
            .iter()
            .any(|w| w.contains("declination mode")));
        // Each survey carries its own file's state; the surveys from
        // file 0 should differ in declination_mode from those of file 1.
        let modes: Vec<DeclinationMode> = parsed
            .surveys
            .iter()
            .map(|s| s.state.declination_mode)
            .collect();
        assert!(
            modes.iter().any(|m| matches!(m, DeclinationMode::Auto)),
            "at least one survey should use Auto (from file 1), got {modes:?}"
        );
        assert!(
            modes.iter().any(|m| !matches!(m, DeclinationMode::Auto)),
            "at least one survey should NOT use Auto (from file 0), got {modes:?}"
        );

        // resolve_anchor must use the first FixedStation's own zone/datum,
        // not file 0's, when they disagree.  All fixed_stations in this
        // fixture come from project_stations (none declared inline), so
        // none are projected here - exercise the path by faking via the
        // build_preview output: it must surface file 0's UTM zone (13).
        let preview = build_preview(&parsed);
        assert_eq!(preview.utm_zone, Some(13));
    }

    #[test]
    fn backsight_inclination_converted_to_degrees_for_grads() {
        // backsight_grads.dat declares FORMAT inclination unit `R` (Grads)
        // and carries one shot with a backsight column.  Compass Grads:
        // 400 grads = 360°, so `100 grads = 90°` and `-100 grads = -90°`.
        //
        // Forward: BEARING=90, INC=+100 grads (= +90° up).
        // Back   : BEARING=270 (= 90+180), INC=-100 grads (B looking back
        //          at A which is below - back-inc is the down-reading).
        //
        // The two-sign-flip in `shot_backsight_geometry` (TO→FROM ⇒ FROM→TO,
        // then Compass +up ⇒ Stalagmight +down) collapses to identity on
        // the VALUE - so pitch_out should equal the degree-converted
        // back_inc, i.e. -100 grads * 0.9 = -90°.  Before the unit-
        // conversion fix the function returned the raw -100 (a grads
        // reading stored as if it were degrees), which is the regression
        // this test pins.
        let parsed = parse_compass(&fixture("backsight_grads.dat")).unwrap();
        let (configs, _, _) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        let shot = configs
            .iter()
            .flat_map(|c| c.shots.iter())
            .next()
            .expect("at least one shot");
        let pitch_out = shot.pitch_out.expect("backsight should yield pitch_out");
        assert!(
            (pitch_out + 90.0).abs() < 0.01,
            "pitch_out should be -90° after Grads→degrees + back-to-fore + +down; got {pitch_out}"
        );
        // Sanity: pitch_in is also degree-converted on the fore-sight
        // path.  +100 grads → +90° up; Stalagmight stores as -90° (+down).
        let pitch_in = shot
            .pitch_in
            .expect("fore inclination should populate pitch_in");
        assert!(
            (pitch_in + 90.0).abs() < 0.01,
            "pitch_in should be -90° after Grads→degrees + +down; got {pitch_in}"
        );
    }

    /// One survey record - cave name first line - in the smallest shape the
    /// vendored parser accepts, with its own two stations so records can be
    /// concatenated into a multi-survey file. Synthetic - no fixture data.
    fn synthetic_survey_record(cave_name: &str, survey_name: &str, date: &str) -> String {
        format!(
            "{cave_name}\r\n\
             SURVEY NAME: {survey_name}\r\n\
             SURVEY DATE: {date}\r\n\
             SURVEY TEAM:\r\n\
             Nobody\r\n\
             DECLINATION: 0.00  FORMAT: DMMDLRUDLADN  CORRECTIONS: 0.00 0.00 0.00\r\n\
             \r\n\
             FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\
             \r\n\
             {survey_name}1 {survey_name}2 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n"
        )
    }

    /// A one-survey `.DAT`, in the smallest shape the vendored parser accepts.
    fn synthetic_dat(cave_name: &str) -> String {
        synthetic_survey_record(cave_name, "A", "7 10 1979")
    }

    /// Join survey records the way Compass writes a `.DAT`: a form feed
    /// (0x0C) on its own line ends each record.
    fn synthetic_multi_survey_dat(records: &[String]) -> String {
        let mut dat = String::new();
        for record in records {
            dat.push_str(record);
            dat.push_str("\x0c\r\n");
        }
        dat
    }

    /// `SURVEY DATE:` takes three unsigned integers and nothing checks that
    /// they name a real day, so a hand-edited or truncated `.DAT` can carry
    /// `2 29 1979`, `13 45 1979` or a field wider than a `u32`.
    ///
    /// Every one of those has to come back as a REJECTED survey. The date is
    /// parsed inside a `#[tauri::command]`, so a panic there does not surface
    /// as an error dialog - it unwinds out of the command and the caller's
    /// promise never settles.
    #[test]
    fn an_impossible_survey_date_is_rejected_rather_than_panicking() {
        let dir = tempfile::tempdir().unwrap();
        // Every one of these reaches `NaiveDate::from_ymd_opt(..).unwrap()` or
        // `digit1.parse::<u32>().unwrap()` in the vendored survey parser.
        for date in [
            "2 29 1979",        // February 29th of a common year
            "0 10 1979",        // month 0
            "7 0 1979",         // day 0
            "13 45 1979",       // month 13, day 45
            "256 10 1979",      // month 256, which truncates to 0 as a u8
            "7 10 99999999999", // wider than the u32 the field parses into
        ] {
            let path = dir.path().join("bad-date.dat");
            std::fs::write(
                &path,
                synthetic_dat("Synthetic Cave")
                    .replace("SURVEY DATE: 7 10 1979", &format!("SURVEY DATE: {date}")),
            )
            .unwrap();

            // `many0` stops at the first survey it cannot parse and returns
            // what it has, so a lone bad survey yields an empty file rather
            // than an `Err`. Refusing the file outright is a rejection too;
            // what is NOT allowed is the survey coming back imported, or the
            // file coming back empty with nothing said about why.
            match parse_compass(&path) {
                Err(_) => {}
                Ok(parsed) => {
                    assert!(
                        parsed.surveys.is_empty(),
                        "SURVEY DATE: {date} must be rejected, got {} survey(s)",
                        parsed.surveys.len()
                    );
                    assert!(
                        parsed
                            .warnings
                            .iter()
                            .any(|w| w.contains("Parsing stopped")),
                        "SURVEY DATE: {date} left the import empty; that has to be said out \
                         loud, warnings: {:?}",
                        parsed.warnings
                    );
                }
            }
        }
    }

    /// `many0` stops at the first survey it cannot read and returns the ones
    /// before it WITHOUT consuming the rest of the file, so a `.DAT` with an
    /// unparseable survey in the middle imports as a SHORTER file rather than
    /// as a failure - a preview that looks perfectly healthy while the tail of
    /// the cave is missing.
    ///
    /// Refusing the whole file would cost the user the surveys that did read,
    /// so the import stays partial and a warning carries the loss.
    #[test]
    fn a_survey_that_stops_the_parse_takes_the_rest_of_the_file_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let dat = synthetic_multi_survey_dat(&[
            synthetic_survey_record("Synthetic Cave", "A", "7 10 1979"),
            // February 29th of a common year: a parse error since #479.
            synthetic_survey_record("Synthetic Cave", "B", "2 29 1979"),
            synthetic_survey_record("Synthetic Cave", "C", "8 11 1981"),
        ]);
        let path = dir.path().join("stops-at-two.dat");
        std::fs::write(&path, &dat).unwrap();

        let parsed = parse_compass(&path).unwrap();

        // A is ahead of the bad date and imports. C is NOT recovered: `many0`
        // stops at B and never resumes, so C reaches the user only through the
        // warning. Pinning the exact list is deliberate - if the parser ever
        // learns to skip a bad survey and carry on, this is the test that says
        // the warning has to change with it.
        let names: Vec<&str> = parsed
            .surveys
            .iter()
            .map(|s| s.survey.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["A"],
            "A imports, B is rejected, and C is behind B so it is lost with it"
        );

        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.contains("Parsing stopped at survey 2")),
            "the dropped surveys must be warned about, and the warning must name the survey \
             the parse stopped at, warnings: {:?}",
            parsed.warnings
        );

        // And where it stopped, so the user can open the file at that byte.
        // The file is ASCII, so a byte offset is a string index into it.
        let warning = parsed
            .warnings
            .iter()
            .find(|w| w.contains("(byte "))
            .expect("the truncation warning carries a byte offset");
        let offset: usize = warning
            .split("(byte ")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .and_then(|digits| digits.parse().ok())
            .expect("the byte offset is a number");
        let last_shot_of_a = dat.find("A1 A2").unwrap();
        let header_of_b = dat.find("SURVEY NAME: B").unwrap();
        assert!(
            (last_shot_of_a..=header_of_b).contains(&offset),
            "the offset points at the record the parse stopped on (between {last_shot_of_a} \
             and {header_of_b}), got {offset}"
        );
    }

    /// The counterpart: three good surveys leave only the form feeds behind,
    /// and a clean import must not be decorated with a truncation warning.
    #[test]
    fn a_dat_that_parses_to_the_end_carries_no_truncation_warning() {
        let dir = tempfile::tempdir().unwrap();
        let dat = synthetic_multi_survey_dat(&[
            synthetic_survey_record("Synthetic Cave", "A", "7 10 1979"),
            synthetic_survey_record("Synthetic Cave", "B", "8 10 1979"),
            synthetic_survey_record("Synthetic Cave", "C", "8 11 1981"),
        ]);
        let path = dir.path().join("whole.dat");
        // Compass-era files often end with a DOS end-of-file marker, which no
        // survey parser consumes; it is not a truncated tail.
        std::fs::write(&path, format!("{dat}\x1a")).unwrap();

        let parsed = parse_compass(&path).unwrap();
        assert_eq!(parsed.surveys.len(), 3);
        assert!(
            parsed.warnings.is_empty(),
            "a whole file read to the end warns about nothing, got {:?}",
            parsed.warnings
        );
    }

    /// A `.MAK` links its `.DAT` files and each one is parsed on its own, so
    /// each can stop part-way through - and the surveys behind the stop are
    /// dropped from a project import exactly as they are from a direct one.
    /// This is the path most real projects are opened by, so the warning has
    /// to reach it too, and has to name WHICH linked file lost surveys.
    #[test]
    fn a_mak_names_the_dat_whose_parse_stopped_part_way_through() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("whole.dat"),
            synthetic_multi_survey_dat(&[
                synthetic_survey_record("Synthetic Cave", "A", "7 10 1979"),
                synthetic_survey_record("Synthetic Cave", "B", "8 10 1979"),
            ]),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("stops.dat"),
            synthetic_multi_survey_dat(&[
                synthetic_survey_record("Synthetic Cave", "C", "9 10 1979"),
                // February 29th of a common year: a parse error since #479.
                synthetic_survey_record("Synthetic Cave", "D", "2 29 1979"),
                synthetic_survey_record("Synthetic Cave", "E", "8 11 1981"),
            ]),
        )
        .unwrap();

        // A `#file` entry needs a base location and a datum ahead of it.
        let mak = "@100.0,200.0,300.0,13,0.5;\r\n\
                   &North American 1983;\r\n\
                   #whole.dat;\r\n\
                   #stops.dat;\r\n";
        let path = dir.path().join("project.mak");
        std::fs::write(&path, mak).unwrap();

        let parsed = parse_compass(&path).unwrap();

        // Both surveys of the whole file, then the one ahead of the bad date.
        // E is behind D, and `many0` never resumes, so it is lost with it.
        let names: Vec<&str> = parsed
            .surveys
            .iter()
            .map(|s| s.survey.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["A", "B", "C"],
            "the whole file imports, and the second file only up to its bad date"
        );

        let truncation: Vec<&String> = parsed
            .warnings
            .iter()
            .filter(|w| w.contains("Parsing stopped"))
            .collect();
        assert_eq!(
            truncation.len(),
            1,
            "one truncated file, one warning; got {:?}",
            parsed.warnings
        );
        let warning = truncation.first().expect("one truncation warning");
        assert!(
            warning.contains("stops.dat") && warning.contains("Parsing stopped at survey 2"),
            "the warning names the linked file that lost surveys and where it stopped: {warning}"
        );
        assert!(
            !warning.contains("whole.dat"),
            "the file that read to the end is not implicated: {warning}"
        );
    }

    /// [`synthetic_survey_record`] with a declared declination, so the #438
    /// contract has something to bite on. Kept as its own `format!` rather than
    /// a parameter there: every other caller wants the zero that helper writes.
    fn synthetic_dat_with_declination(declination: f64) -> String {
        format!(
            "Synthetic Cave\r\n\
             SURVEY NAME: A\r\n\
             SURVEY DATE: 7 10 1979\r\n\
             SURVEY TEAM:\r\n\
             Nobody\r\n\
             DECLINATION: {declination:.2}  FORMAT: DMMDLRUDLADN  CORRECTIONS: 0.00 0.00 0.00\r\n\
             \r\n\
             FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\
             \r\n\
             A1 A2 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n"
        )
    }

    /// #438: Compass used to fold `declination - convergence` into every shot at
    /// import (`azimuth + ctx.declination - ctx.convergence_angle`), destroying
    /// the measured reading. It now keeps the reading and declares the
    /// correction on the section.
    ///
    /// Both halves are asserted because either alone is satisfiable by a wrong
    /// implementation: keeping the reading without recording the correction
    /// silently drops the declination (the cave rotates), and recording it while
    /// still baking it in applies it twice.
    #[test]
    fn a_declared_declination_lands_on_the_section_not_on_the_shots() {
        let dir = tempfile::tempdir().unwrap();
        let dat = dir.path().join("declination.dat");
        std::fs::write(&dat, synthetic_dat_with_declination(5.0)).unwrap();

        let parsed = parse_compass(&dat).unwrap();
        let (configs, _closures, _warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();

        let shot = &configs[0].shots[0];
        assert!(
            (shot.azimuth_in - 90.0).abs() < 1e-9,
            "the measured bearing must survive the import, got {}",
            shot.azimuth_in
        );
        let reference = configs[0]
            .azimuth_reference
            .expect("Compass always declares a reference");
        assert!(
            (reference.applied_correction() - 5.0).abs() < 1e-9,
            "the declared declination must land on the section, got {}",
            reference.applied_correction()
        );
        assert_eq!(reference.north, crate::models::NorthReference::Magnetic);
        assert_eq!(reference.source, CorrectionSource::FileDeclared);
        assert!(
            (reference.to_true(90.0) - 95.0).abs() < 1e-9,
            "the corrected bearing is what the importer used to store"
        );
    }

    /// An ENTERED zero is a declaration: the surveyor wrote `DECLINATION: 0.00`
    /// in the book, which says the readings need no correction. It votes in the
    /// export's uniformity test like any other declared value.
    ///
    /// Contrast [`ignore_mode_declares_nothing_at_all`]: the `!I` project flag
    /// says "do not apply declinations", which is an instruction about
    /// PROCESSING and tells us nothing about the readings.
    #[test]
    fn a_zero_declination_is_still_a_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let dat = dir.path().join("plain.dat");
        std::fs::write(&dat, synthetic_dat("Synthetic Cave")).unwrap();

        let parsed = parse_compass(&dat).unwrap();
        let (configs, _closures, _warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        let reference = configs[0]
            .azimuth_reference
            .expect("Compass always declares a reference");
        assert!(reference.is_identity());
        assert_eq!(reference.source, CorrectionSource::FileDeclared);
    }

    /// #438 review item 2: the grid convergence is REPORTED, not applied.
    ///
    /// `shot_geometry` used to fold `declination - convergence` into every
    /// reading and the reference then recorded that same combined number under
    /// a `Magnetic` label - so the convergence was invisible to the export and
    /// the label was wrong about what the correction reached.
    ///
    /// The resolution is that the correction reaches TRUE north and nothing
    /// else: the app's world frame is true north (`models/geo.rs:79-81` -
    /// `gps_to_local` returns true-north ENU, and every datum path places
    /// against it), so a survey walked in grid bearings sits at an angle to its
    /// own GPS. Compass subtracts convergence because Compass's display is the
    /// UTM grid; ours is not.
    #[test]
    fn a_grid_convergence_is_reported_and_not_folded_into_the_correction() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("cave.dat"),
            synthetic_dat_with_declination(5.0),
        )
        .unwrap();
        // `!V` turns UTM convergence on; the `@` line's fifth field is the
        // convergence angle in degrees.
        let mak = dir.path().join("cave.mak");
        std::fs::write(
            &mak,
            "@100.0,200.0,300.0,13,2.0;\r\n&North American 1983;\r\n!V;\r\n#cave.dat;\r\n",
        )
        .unwrap();

        let parsed = parse_compass(&mak).unwrap();
        let (configs, _closures, warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();

        assert!(
            (configs[0].shots[0].azimuth_in - 90.0).abs() < 1e-9,
            "the measured reading is untouched, got {}",
            configs[0].shots[0].azimuth_in
        );
        let reference = configs[0]
            .azimuth_reference
            .expect("Compass always records a reference");
        assert!(
            (reference.applied_correction() - 5.0).abs() < 1e-9,
            "the correction is the DECLINATION alone - a convergence folded in \
             would make this 3.0 - got {}",
            reference.applied_correction()
        );
        assert_eq!(reference.north, crate::models::NorthReference::Magnetic);
        assert!(
            warnings.iter().any(|w| w.contains("convergence")),
            "a convergence the import drops has to be in the receipt: {warnings:?}"
        );
    }

    /// `!I` (ignore declinations) is not a declaration that zero is the offset
    /// to true north - the file is silent about what the readings are off by.
    ///
    /// The difference only shows at export, and it is the whole reason the
    /// distinction is kept: a DECLARED correction votes in the document's
    /// uniformity test, so recording this as `FileDeclared` dragged any project
    /// mixing an Ignore survey with a real declination onto the
    /// rewrite-every-bearing path (#438).
    #[test]
    fn ignore_mode_declares_nothing_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let dat = dir.path().join("cave.dat");
        std::fs::write(&dat, synthetic_dat_with_declination(5.0)).unwrap();
        // `!I` on the MAK's project parameters selects Ignore for every file.
        // A `#file` entry needs a base location and a datum ahead of it.
        let mak = dir.path().join("cave.mak");
        std::fs::write(
            &mak,
            "@100.0,200.0,300.0,13,0.5;\r\n&North American 1983;\r\n!I;\r\n#cave.dat;\r\n",
        )
        .unwrap();

        let parsed = parse_compass(&mak).unwrap();
        let (configs, _closures, _warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        let reference = configs[0]
            .azimuth_reference
            .expect("Compass always records a reference");
        assert!(
            reference.is_identity(),
            "Ignore applies no correction, got {}",
            reference.applied_correction()
        );
        assert_eq!(
            reference.source,
            CorrectionSource::Unknown,
            "Ignore is an instruction, not a declaration"
        );
    }

    /// Compass files are read one byte per code point, so a "UTF-8 with BOM"
    /// `.DAT` carries three Latin-1 characters into whatever line 1 means -
    /// here the cave name, which becomes the imported project's name.
    #[test]
    fn a_utf8_bom_does_not_corrupt_the_dat_cave_name() {
        let dir = tempfile::tempdir().unwrap();
        let dat = synthetic_dat("Synthetic Cave");

        let plain = dir.path().join("plain.dat");
        std::fs::write(&plain, &dat).unwrap();
        let marked = dir.path().join("bom.dat");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(dat.as_bytes());
        std::fs::write(&marked, bytes).unwrap();

        let expected = parse_compass(&plain).unwrap();
        assert_eq!(expected.cave_name, "Synthetic Cave");
        assert_eq!(expected.surveys.len(), 1);

        let parsed = parse_compass(&marked).unwrap();
        assert_eq!(parsed.cave_name, expected.cave_name);
        assert_eq!(parsed.surveys.len(), expected.surveys.len());
    }

    /// A `.MAK` is read from its path inside the vendored crate, so the mark
    /// cannot be stripped for it. What we can do is say so: the vendored lexer
    /// otherwise reports an unexpected character that renders as nothing.
    #[test]
    fn a_utf8_bom_on_a_mak_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s.dat"), synthetic_dat("Synthetic Cave")).unwrap();

        // A `#file` entry needs a base location and a datum ahead of it.
        let mak = "@100.0,200.0,300.0,13,0.5;\r\n&North American 1983;\r\n#s.dat;\r\n";
        let plain = dir.path().join("plain.mak");
        std::fs::write(&plain, mak).unwrap();
        let marked = dir.path().join("bom.mak");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(mak.as_bytes());
        std::fs::write(&marked, bytes).unwrap();

        assert_eq!(
            parse_compass(&plain).unwrap().cave_name,
            "Synthetic Cave",
            "premise: the same project without a mark imports"
        );

        // Premise for the pre-flight: left to the vendored reader, the mark is
        // an unexpected character whose name is an empty-looking glyph.
        let vendor = Project::read(&marked)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            vendor.contains("Unexpected character"),
            "the vendored lexer is expected to choke on the mark, got {vendor:?}"
        );

        let err = parse_compass(&marked)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("byte-order mark"),
            "the error must name the mark, got {err:?}"
        );
    }
}
