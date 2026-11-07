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
//! - feet are converted to meters (a `.DAT` stores lengths and LRUDs in decimal
//!   feet and inclinations in decimal degrees whatever its FORMAT line says;
//!   FORMAT only describes the surveyor's notebook);
//! - station elevations are integrated from `length * sin(inclination)` along the
//!   station graph (anchored at the `.MAK` fixed station's UTM elevation, or 0)
//!   and stored as Stalagmight absolute depth (positive = down);
//! - the station-name graph is split into linear sections joined by
//!   `FromImporting` merges + loop closures;
//! - a `.MAK` fixed station becomes a GPS zero datum (UTM → WGS84 via `proj4rs`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use chrono::Datelike;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use compass_data::{
    BackSightCorrectionFactors, CorrectionFactors, Datum, DeclinationMode, LRUDAssociation,
    Project, Shot, Survey,
};

use crate::models::azimuth_reference::{AzimuthReference, CorrectionSource};
use crate::models::section::{ImportClosure, ImportPlacement, ImportSectionConfig};
use crate::models::shot::{ShotData, ShotKind};
use crate::models::station::GpsCoordinates;
use crate::parsers::encoding::{
    cp1252_replacement_warning, describe_byte_limit, read_cp1252, Cp1252Text,
};
use crate::parsers::geo::{non_empty, FEET_TO_METERS};
use crate::parsers::linked_files::{
    outside_root, resolve_link, Admission, LinkBudget, MAX_LINKED_BYTES,
};
use crate::parsers::reading_bounds::{beyond_limit, note, BoundNotes};
use crate::parsers::station_graph::{
    build_closures, place_chains, split_into_chains, topological_sort_configs, Anchor, ConfigOrder,
    PlacementPolicy,
};

/// LRUDs and angles at or above this are Compass "missing data" sentinels. Not
/// lengths: a length has no missing marker (see `length_missing`).
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
    /// Length unit the file's surveys STORE their numbers in: always
    /// `"feet"`, or `null` when there are no surveys. A `.DAT` stores decimal
    /// feet whatever unit its FORMAT says the notebook used (#576 B28), so
    /// every import converts from feet. Drives the unit blurb on the import
    /// dialog; the `"meters"` / `"mixed"` labels it also understands are no
    /// longer produced.
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
    /// The `.MAK`'s LRUD association override: `Some(true)` puts every
    /// survey's LRUDs at the To station (`!OT`), `Some(false)` at the From
    /// station (`!Ot`), `None` leaves each survey's FORMAT flag in charge
    /// (#576 B31).
    lrud_on_to_override: Option<bool>,
}

impl Default for CompassFileState {
    fn default() -> Self {
        Self {
            datum: None,
            utm_zone: None,
            convergence: 0.0,
            convergence_applied: false,
            declination_mode: DeclinationMode::default(),
            lrud_on_to_override: None,
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
    let mut parsed = match ext.as_deref() {
        Some("mak") => parse_mak(path)?,
        Some("dat") => parse_dat(path)?,
        _ => {
            return Err(anyhow!(
                "Unsupported Compass file '{}' (expected .mak or .dat)",
                path.display()
            ))
        }
    };
    bound_length_corrections(&mut parsed);
    Ok(parsed)
}

/// A survey's length correction beyond the limit no real survey reaches is
/// read as 0 (#576 FZ-PARSE-5-D1): added to every shot, one near `f64::MAX`
/// pushed the computed depths to infinity. Done on the parsed surveys, so
/// the preview's counts, the warning it shows and the import all see the
/// same value. A shot's own length and LRUDs need no bound: Compass reads
/// 990 ft and above as its missing marker.
fn bound_length_corrections(parsed: &mut ParsedCompass) {
    let mut notes = BoundNotes::default();
    for entry in &mut parsed.surveys {
        let survey = &mut entry.survey;
        if let Some(cf) = survey.parameters.correction_factors.as_mut() {
            // CORRECTIONS are feet, like the lengths they adjust.
            let metres = cf.length * FEET_TO_METERS;
            if beyond_limit(metres) {
                let at = format!("Survey '{}'", survey.name);
                notes.push(note(&at, "length correction", metres, "read as 0"));
                cf.length = 0.0;
            }
        }
    }
    notes.finish_into(&mut parsed.warnings);
}

fn parse_mak(path: &Path) -> Result<ParsedCompass> {
    // Stalagmight reads the `.MAK` and every `.DAT` it links itself and hands
    // the vendored parser text (#576 B35i-3, B8): the same Windows-1252 decode,
    // byte-order-mark strip and replacement warning as a bare `.DAT`, plus the
    // Walls importer's rules for which linked paths may be read.
    let Cp1252Text { text, replaced } = read_cp1252(path)?;
    // A project reads many files, so each warning about one names it.
    let mut warnings: Vec<String> = cp1252_replacement_warning(Some(path), "Compass", replaced)
        .into_iter()
        .collect();
    let project = Project::from_text(path, &text)
        .map_err(|e| anyhow!("Failed to read Compass project: {e}"))?;
    let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let linked = project.survey_files.len();
    let mut skipped: Vec<String> = Vec::new();
    let mut budget = LinkBudget::new(MAX_LINKED_BYTES);
    let loaded = project
        .load_survey_files_with(|file| {
            read_linked_dat(&folder, file, &mut budget, &mut warnings, &mut skipped)
        })
        .map_err(|e| anyhow!("Failed to load Compass survey files: {e}"))?;
    // A skipped file costs only its own surveys; with nothing read at all
    // there is nothing to import.
    if linked > 0 && loaded.survey_files.is_empty() {
        bail!(
            "None of the survey files this project lists could be read. {}",
            skipped.join(" ")
        );
    }

    let mut datum = None;
    let mut utm_zone = None;
    let mut declination_mode = DeclinationMode::default();
    let mut fixed_stations = Vec::new();
    let mut surveys: Vec<SurveyWithState> = Vec::new();

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
            lrud_on_to_override: None,
        };
        if let Some(fc) = &raw_state.file_convergence {
            file_state.convergence = fc.angle;
            file_state.convergence_applied = fc.enabled;
        }
        if let Some(pp) = &raw_state.project_parameters {
            file_state.declination_mode = pp.declination_mode;
            file_state.convergence_applied = pp.utm_convergence_applied;
            // "If the flag is set to capital "O." all other methods of
            // controlling the LRUD associations are overridden" (Compass
            // Project Manager help, "Parameters").
            file_state.lrud_on_to_override =
                pp.override_lrud_association.then_some(pp.lrud_to_station);
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
                // #576 FZ-PARSE-5-D1: the elevation seeds every station depth.
                if beyond_limit(loc.up) {
                    let at = format!("Fixed station '{}'", station.name);
                    let outcome = "it was ignored as an anchor candidate";
                    warnings.push(note(&at, "elevation", loc.up, outcome));
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

/// Read one `.DAT` a `.MAK` links, for `Project::load_survey_files_with`:
/// its text, or `None` - with the reason in `warnings` - when it is refused,
/// missing, unreadable, already read, or over the project's `budget`. The
/// rest of the project still imports (#576 B8, decision B8-D2: the Walls
/// importer's rule); a refused, missing or unreadable one is also listed in
/// `skipped`.
///
/// Where it lives follows the Walls `.PATH` rules ([`resolve_link`], #576
/// B8-D1 and RV3-R-18): backslashes are separators, a relative link resolves
/// inside the `.MAK`'s `folder` with `..` clamped there, an absolute local
/// path is honoured, and network, device-namespace, DOS device and
/// unresolvable drive-letter links are refused from their text before
/// anything touches them. A file read from outside the folder - by an
/// absolute link or through a symlink - is said out loud.
fn read_linked_dat(
    folder: &Path,
    link: &Path,
    budget: &mut LinkBudget,
    warnings: &mut Vec<String>,
    skipped: &mut Vec<String>,
) -> Option<String> {
    let shown = link.display();
    let reason = match resolve_link(folder, folder, &link.to_string_lossy()) {
        Err(reason) => format!("'{shown}' was skipped: {reason}. Its surveys were not imported."),
        Ok(resolved) if !resolved.exists() => format!(
            "'{shown}' was not found (looked for {}); it was skipped and its surveys were not \
             imported.",
            resolved.display()
        ),
        Ok(resolved) => match budget.admit(&resolved) {
            Admission::AlreadyRead => {
                warnings.push(format!(
                    "'{shown}' is listed more than once in the .MAK; the file was read once, and \
                     this listing (with any fixed stations it declares) was skipped so its \
                     surveys are not imported twice."
                ));
                return None;
            }
            Admission::OverLimit { limit } => format!(
                "'{shown}' was skipped: with it, the files this .MAK links would add up to \
                 more than the {} limit for one project. Its surveys were not imported.",
                describe_byte_limit(limit)
            ),
            Admission::Fits(ticket) => match read_cp1252(&resolved) {
                Ok(Cp1252Text { text, replaced }) => {
                    if let Some(real) = outside_root(&resolved, folder) {
                        warnings.push(format!(
                            "'{shown}' is outside the .MAK's folder (it is {}); it was read \
                             from there.",
                            real.display()
                        ));
                    }
                    warnings.extend(cp1252_replacement_warning(Some(link), "Compass", replaced));
                    budget.record(ticket);
                    return Some(text);
                }
                Err(e) => format!(
                    "'{shown}' could not be read ({e:#}); it was skipped and its surveys \
                     were not imported."
                ),
            },
        },
    };
    warnings.push(reason.clone());
    skipped.push(reason);
    None
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
    let Cp1252Text {
        text: contents,
        replaced,
    } = read_cp1252(path)?;
    let (raw_surveys, remainder) = Survey::parse_dat_file_with_remainder(&contents)
        .map_err(|e| anyhow!("Failed to parse Compass .DAT file: {e}"))?;
    // `read_cp1252` maps one file byte to one `char`, so the chars ahead of
    // the remainder count the bytes consumed - the offset into the file, past
    // any byte-order mark (stripped before parsing).
    let offset = contents.chars().count() - remainder.chars().count();
    // A bare `.DAT` is the one file the user picked, so neither warning names it.
    let warnings: Vec<String> = cp1252_replacement_warning(None, "Compass", replaced)
        .into_iter()
        .chain(dat_truncation_warning(
            None,
            remainder,
            offset,
            raw_surveys.len(),
        ))
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
    // The unit the numbers are STORED in, which is what the import converts
    // from: a `.DAT` keeps decimal feet even for a survey whose FORMAT says the
    // notebook was metric (#576 B28), so there is no "meters" or "mixed" case
    // to report. Reporting the notebook unit here would tell the user a metric
    // survey's lengths were "kept as-is" while they are converted from feet.
    let length_unit = (!parsed.surveys.is_empty()).then(|| "feet".to_string());
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
///
/// The shots it counts are also named in `dropped`, a line per corrections
/// case (#576 RVF-d-2), so the preview says which legs will not land, not only
/// how many.
fn invalid_shot_counts(
    survey: &Survey,
    file_state: &CompassFileState,
    dropped: &mut Vec<String>,
) -> InvalidShotCounts {
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
    let mut dropped_either_way = Vec::new();
    let mut dropped_corrected_only = Vec::new();
    let mut dropped_uncorrected_only = Vec::new();
    for shot in &survey.shots {
        let excluded = parse_flags(shot.flags.as_ref()).exclude_all;
        let length_corrected = shot_length_m(shot, &ctx_corrected);
        let length_uncorrected = shot_length_m(shot, &ctx_uncorrected);
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
        match (length_corrected <= 0.0, length_uncorrected <= 0.0) {
            (true, true) => dropped_either_way.push(shot_label(shot)),
            (true, false) => dropped_corrected_only.push(shot_label(shot)),
            (false, true) => dropped_uncorrected_only.push(shot_label(shot)),
            (false, false) => {}
        }
    }
    let name = &survey.name;
    dropped.extend(dropped_length_line(
        name,
        &dropped_either_way,
        "",
        "will not be imported",
    ));
    dropped.extend(dropped_length_line(
        name,
        &dropped_corrected_only,
        " once the survey's length correction is applied",
        "will not be imported while corrections are applied",
    ));
    dropped.extend(dropped_length_line(
        name,
        &dropped_uncorrected_only,
        " without the survey's length correction",
        "will not be imported while corrections are not applied",
    ));
    counts
}

/// How many shots one preview line names before it only counts the rest: a
/// project ties its surveys together with zero-length connector shots, so one
/// survey can drop many (#576 RVF-d-2).
const MAX_NAMED_DROPPED_SHOTS: usize = 10;

fn shot_label(shot: &Shot) -> String {
    format!("{} -> {}", shot.from, shot.to)
}

/// The preview line naming a survey's shots the import drops for their length
/// (#576 RVF-d-2), or `None` when there are none. True under every toggle: a
/// shot dropped either way is not imported whatever the dialog says (an
/// `X`-flagged one is not imported unless excluded shots are included, and then
/// it is dropped for its length), and one that only the survey's length
/// correction decides says under which corrections setting it is dropped.
fn dropped_length_line(
    survey: &str,
    shots: &[String],
    cause: &str,
    outcome: &str,
) -> Option<String> {
    match shots {
        [] => None,
        [shot] => Some(format!(
            "Survey '{survey}': shot {shot} has no usable length{cause}, so it {outcome}."
        )),
        _ => {
            let named = shots
                .iter()
                .take(MAX_NAMED_DROPPED_SHOTS)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            let more = match shots.len().saturating_sub(MAX_NAMED_DROPPED_SHOTS) {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            Some(format!(
                "Survey '{survey}': {} shots have no usable length{cause}, so they {outcome}: \
                 {named}{more}.",
                shots.len()
            ))
        }
    }
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
    // The parser's own warnings, then one line per shot dropped for its length.
    let mut warnings = parsed.warnings.clone();
    let per_survey_invalid_counts: Vec<InvalidShotCounts> = parsed
        .surveys
        .iter()
        .map(|s| invalid_shot_counts(&s.survey, &s.state, &mut warnings))
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
        warnings,
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
    apply_corrections: bool,
    corrections: Option<CorrectionFactors>,
    backsight_corrections: Option<BackSightCorrectionFactors>,
    /// Each row's LRUDs were measured at its TO station, not its FROM (#576
    /// B31): the FORMAT's last character is `T`, or the `.MAK` overrides it.
    lrud_on_to: bool,
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

/// A length is missing only when it is not a number: the shared non-finite
/// guard, and no sentinel. Compass's "Survey Data File Format" page gives a
/// missing marker for a bearing or inclination with backsights (-999) and for
/// an LRUD (negative), but none for a length, "a single floating point number"
/// in decimal feet. Reading 990 ft and above as missing dropped real long legs
/// (#576 RVF-d-2).
fn length_missing(raw: f64) -> bool {
    !raw.is_finite()
}

/// A stored LRUD in meters. A `.DAT` stores LRUDs in decimal feet whatever the
/// FORMAT's passage unit says (#576 B28).
///
/// Any negative value is the missing marker (#576 FZ-PARSE-7-D1), not only
/// `-1` and below: the vendored parser hands the column over raw, and a marker
/// that went through a unit conversion (`-0.30`, -1 ft written in metres)
/// would otherwise import as a negative passage dimension. Walls drops
/// negative LRUDs the same way.
fn lrud_meters(raw: f64) -> Option<f64> {
    // Not finite, negative, or at the +990 sentinel: not measured.
    if !(0.0..MISSING_VALUE_THRESHOLD).contains(&raw) {
        return None;
    }
    let value = raw * FEET_TO_METERS;
    if value.abs() < f64::EPSILON {
        None
    } else {
        Some(value)
    }
}

/// A cross-section in meters; `None` for a dimension that was not measured.
#[derive(Clone, Copy, Default)]
struct Lrud {
    left: Option<f64>,
    right: Option<f64>,
    up: Option<f64>,
    down: Option<f64>,
}

impl Lrud {
    fn of(shot: &Shot) -> Self {
        Self {
            left: lrud_meters(shot.left),
            right: lrud_meters(shot.right),
            up: lrud_meters(shot.up),
            down: lrud_meters(shot.down),
        }
    }

    fn is_empty(&self) -> bool {
        self.left.is_none() && self.right.is_none() && self.up.is_none() && self.down.is_none()
    }
}

/// A To-associated survey's cross-sections, by the station they were measured
/// at (#576 B31).
///
/// A shot stores the cross-section of its FROM station. When a survey records
/// each row's LRUDs at the row's TO station, that section belongs on the shot
/// that LEAVES the station: the next shot in the chain, or - for a chain that
/// starts there, such as a branch - the first row of the survey that ends
/// there. A section measured at a station no shot of the survey leaves has
/// nowhere to go and is counted for the receipt; so is one measured where a
/// shot does leave, when every such shot took another row's section at that
/// station instead (#576 RV2-R-NITS).
struct ToStationLruds<'a> {
    /// The first kept row ending at each station that measured a section, as
    /// an index into the survey's kept shots.
    by_station: HashMap<&'a str, usize>,
    /// Sections measured at a station no kept shot leaves.
    stranded: usize,
    /// Sections measured at a station a kept shot leaves that no shot
    /// carries.
    superseded: usize,
}

impl<'a> ToStationLruds<'a> {
    /// `chains` is how the survey's kept `shots` split into sections.
    fn new(shots: &[&'a Shot], chains: &[Vec<usize>]) -> Self {
        let starts: HashSet<&str> = shots.iter().map(|s| s.from.as_str()).collect();
        let mut by_station: HashMap<&'a str, usize> = HashMap::new();
        let mut stranded = 0;
        // Rows whose section some shot could carry.
        let mut placeable: Vec<usize> = Vec::new();
        for (row, shot) in shots.iter().enumerate() {
            if Lrud::of(shot).is_empty() {
                continue;
            }
            if starts.contains(shot.to.as_str()) {
                by_station.entry(shot.to.as_str()).or_insert(row);
                placeable.push(row);
            } else {
                stranded += 1;
            }
        }
        let mut lruds = Self {
            by_station,
            stranded,
            superseded: 0,
        };
        let mut carried = vec![false; shots.len()];
        for chain in chains {
            for pos in 0..chain.len() {
                if let Some(row) = lruds.source(shots, chain, pos) {
                    carried[row] = true;
                }
            }
        }
        lruds.superseded = placeable.iter().filter(|&&row| !carried[row]).count();
        lruds
    }

    /// The kept row whose section the shot at `chain[pos]` carries: the row
    /// ahead of it in its chain (which ends at the shot's From station) when
    /// that row measured one, else the first row ending at that station.
    fn source(&self, shots: &[&Shot], chain: &[usize], pos: usize) -> Option<usize> {
        pos.checked_sub(1)
            .map(|p| chain[p])
            .filter(|&row| !Lrud::of(shots[row]).is_empty())
            .or_else(|| {
                self.by_station
                    .get(shots[chain[pos]].from.as_str())
                    .copied()
            })
    }

    /// The section the shot at `chain[pos]` carries.
    fn at(&self, shots: &[&Shot], chain: &[usize], pos: usize) -> Lrud {
        self.source(shots, chain, pos)
            .map_or_else(Lrud::default, |row| Lrud::of(shots[row]))
    }
}

/// The receipt line for `stranded` To-station cross-sections of one survey
/// that no shot could carry (#576 B31), or `None` when there were none. The
/// Walls importer words its own line the same way.
fn end_of_chain_lrud_note(survey: &str, stranded: usize) -> Option<String> {
    let counted = match stranded {
        0 => return None,
        1 => "1 end-of-chain cross-section had no shot to attach to and was dropped".to_string(),
        n => format!("{n} end-of-chain cross-sections had no shot to attach to and were dropped"),
    };
    Some(to_station_lrud_line(survey, &counted))
}

/// The receipt line for `superseded` To-station cross-sections of one survey
/// that lost to another row's section at the same station (#576
/// RV2-R-NITS), or `None` when there were none.
fn superseded_lrud_note(survey: &str, superseded: usize) -> Option<String> {
    let counted = match superseded {
        0 => return None,
        1 => "1 cross-section was superseded by another measured at the same station and was \
              dropped"
            .to_string(),
        n => format!(
            "{n} cross-sections were superseded by others measured at the same stations and \
             were dropped"
        ),
    };
    Some(to_station_lrud_line(survey, &counted))
}

fn to_station_lrud_line(survey: &str, counted: &str) -> String {
    format!("Survey '{survey}': {counted} (the survey records LRUDs at each shot's To station).")
}

/// Slope length (m) of a shot with the survey's length correction applied; 0.0
/// for a missing or non-finite length, which the converter drops as a
/// zero-length connector.
fn shot_length_m(shot: &Shot, ctx: &SurveyCtx) -> f64 {
    let mut length = if length_missing(shot.length) {
        0.0
    } else {
        shot.length
    };
    if ctx.apply_corrections {
        if let Some(cf) = &ctx.corrections {
            // CORRECTIONS are feet, like the lengths they adjust.
            length += cf.length;
        }
    }
    // A `.DAT` stores lengths in decimal feet whatever the FORMAT's length
    // unit says (#576 B28). Backstop only: build_ctx sanitizes the
    // corrections, so with a finite raw length this is already finite.
    finite_or_zero(length * FEET_TO_METERS)
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

/// An angle reading, or `None` when it was not taken: a -999 sentinel, or a
/// non-finite value (see `angle_missing`).
fn angle_reading(raw: f64) -> Option<f64> {
    (!angle_missing(raw)).then_some(raw)
}

/// One sight of a shot, turned to point FROM → TO: the bearing in degrees
/// [0, 360) and the inclination in Compass's `+up` degrees, each `None` when
/// that reading was not taken. Measured, not corrected (#438): the survey's
/// declination rides on `SurveyCtx::azimuth_reference`; only the survey's
/// instrument CORRECTIONS are applied. Every inclination is stored in decimal
/// degrees whatever unit the FORMAT says the notebook used - depth gauge
/// (`W`) included (#576 B28).
#[derive(Clone, Copy)]
struct Sight {
    bearing: Option<f64>,
    inclination: Option<f64>,
}

fn foresight(shot: &Shot, ctx: &SurveyCtx) -> Sight {
    let (bearing_correction, inclination_correction) =
        match (ctx.apply_corrections, &ctx.corrections) {
            (true, Some(cf)) => (cf.azimuth, cf.inclination),
            _ => (0.0, 0.0),
        };
    Sight {
        bearing: angle_reading(shot.azimuth)
            .map(|a| crate::models::normalize_azimuth(a + bearing_correction))
            .filter(|a| a.is_finite()),
        inclination: angle_reading(shot.inclination)
            .map(|i| i + inclination_correction)
            .filter(|i| i.is_finite()),
    }
}

/// The redundant backsight, turned around to point FROM → TO.
///
/// Compass records it as the instrument reading from the TO station looking
/// back at the FROM station, so for a shot with fore-azimuth `a` and
/// fore-inclination `i` (Compass +up) a well-calibrated backsight reads
/// `a + 180°` and `-i` (uphill from FROM is downhill from TO). Turning it
/// around adds 180° to the bearing and negates the inclination. Each half is
/// its own reading: a missing back bearing says nothing about the back
/// inclination, and vice versa.
fn backsight(shot: &Shot, ctx: &SurveyCtx) -> Sight {
    let (bearing_correction, inclination_correction) =
        match (ctx.apply_corrections, &ctx.backsight_corrections) {
            (true, Some(bcf)) => (bcf.azimuth, bcf.inclination),
            _ => (0.0, 0.0),
        };
    Sight {
        bearing: shot
            .back_azimuth
            .and_then(angle_reading)
            .map(|a| {
                let measured = crate::models::normalize_azimuth(a + bearing_correction);
                crate::models::normalize_azimuth(measured + 180.0)
            })
            .filter(|a| a.is_finite()),
        inclination: shot
            .back_inclination
            .and_then(angle_reading)
            .map(|i| -(i + inclination_correction))
            .filter(|i| i.is_finite()),
    }
}

/// A shot's readings in Stalagmight's slots, after Compass's missing-data
/// rule: a shot is processed "as long as there is at least one compass reading
/// and one inclinometer reading" (Compass Editor help, "Backsights and
/// Redundant Backsights"), so a foresight reading that was not taken is the
/// reversed backsight. The slots follow the Walls importer's convention
/// (`walls/srv.rs` `combine_azimuth` / `combine_inclination`): with one
/// bearing both azimuth slots carry it; with one inclination it sits in
/// `pitch_in` and `pitch_out` stays empty.
struct Readings {
    azimuth_in: f64,
    azimuth_out: f64,
    /// `+down` degrees.
    pitch_in: Option<f64>,
    /// `+down` degrees; only a backsight inclination that was taken.
    pitch_out: Option<f64>,
    /// The inclination the leg's depth change follows, Compass `+up` degrees:
    /// the mean of the two sights when both were taken.
    inclination: f64,
    /// Foresight readings that were not taken and came from the backsight.
    reconstructed: u32,
    /// No sight has a bearing: `azimuth_in` / `azimuth_out` are a 0.0
    /// placeholder (due north).
    bearing_missing: bool,
    /// No sight has an inclination: the leg is kept level.
    inclination_missing: bool,
}

fn resolve_readings(shot: &Shot, ctx: &SurveyCtx) -> Readings {
    let fore = foresight(shot, ctx);
    let back = backsight(shot, ctx);
    let mut reconstructed = 0;
    let (azimuth_in, azimuth_out) = match (fore.bearing, back.bearing) {
        (Some(f), Some(b)) => (f, b),
        (Some(f), None) => (f, f),
        (None, Some(b)) => {
            reconstructed += 1;
            (b, b)
        }
        // Compass's editor refuses this and its help does not say what
        // processing does with it, so the leg keeps its length and points due
        // north, as it always has; the caller names the shot (#576 B30).
        (None, None) => (0.0, 0.0),
    };
    let (inclination, pitch_in, pitch_out) = match (fore.inclination, back.inclination) {
        // Both taken: the depth follows their mean, as Compass and Survex
        // compute it and as the bearings already are (`average_azimuth_of` on
        // the two azimuth slots); the slots keep what each sight read
        // (#576 NF-AN4-1).
        (Some(f), Some(b)) => (f64::midpoint(f, b), Some(-f), Some(-b)),
        (Some(f), None) => (f, Some(-f), None),
        (None, Some(b)) => {
            reconstructed += 1;
            (b, Some(-b), None)
        }
        // As for the bearing: kept level, named by the caller.
        (None, None) => (0.0, Some(0.0), None),
    };
    Readings {
        azimuth_in,
        azimuth_out,
        pitch_in,
        pitch_out,
        inclination,
        reconstructed,
        bearing_missing: fore.bearing.is_none() && back.bearing.is_none(),
        inclination_missing: fore.inclination.is_none() && back.inclination.is_none(),
    }
}

/// The receipt line for a shot that has a field in no sight at all, kept
/// with a placeholder (#576 B30). A plumb leg's bearing does not matter, so
/// only its inclination can earn one.
fn missing_reading_warning(
    shot: &Shot,
    readings: &Readings,
    has_backsights: bool,
) -> Option<String> {
    let plumb = readings.inclination.abs() >= 89.5;
    let bearing = readings.bearing_missing && !plumb;
    let (fields, are, recorded, them) = match (bearing, readings.inclination_missing) {
        (true, true) => (
            "bearing and inclination",
            "are",
            "0 (due north) and level",
            "them",
        ),
        (true, false) => ("bearing", "is", "0 (due north)", "it"),
        (false, true) => ("inclination", "is", "level", "it"),
        (false, false) => return None,
    };
    let source = if has_backsights {
        format!("{are} missing from both the foresight and the backsight")
    } else {
        format!("{are} missing and the survey has no backsight to take {them} from")
    };
    Some(format!(
        "Shot {} -> {}: {fields} {source}; recorded as {recorded}.",
        shot.from, shot.to
    ))
}

/// The receipt line counting a survey's foresight readings that came from the
/// reversed backsight (#576 B30), or `None` when there were none.
fn reconstructed_readings_note(survey: &Survey, reconstructed: u32) -> Option<String> {
    let counted = match reconstructed {
        0 => return None,
        1 => "1 missing foresight reading was".to_string(),
        n => format!("{n} missing foresight readings were"),
    };
    Some(format!(
        "Survey '{}': {counted} taken from the backsight (bearing + 180, inclination \
         negated), as Compass does.",
        survey.name
    ))
}

fn elevation_gain(length_m: f64, inclination_deg: f64) -> f64 {
    length_m * inclination_deg.to_radians().sin()
}

/// Convert a single Compass shot to a Stalagmight `ShotData`, given the absolute
/// depths (meters, +down) already resolved for its endpoints.
fn shot_to_shot_data(
    shot: &Shot,
    ctx: &SurveyCtx,
    lrud: Lrud,
    depth_in: f64,
    depth_out: f64,
    is_virtual: bool,
) -> ShotData {
    let readings = resolve_readings(shot, ctx);
    ShotData {
        // Compass files carry no shot identity; every shot gets a fresh mint.
        uuid: None,
        azimuth_in: readings.azimuth_in,
        azimuth_out: readings.azimuth_out,
        length: shot_length_m(shot, ctx),
        depth_in,
        depth_out,
        pitch_in: readings.pitch_in,
        pitch_out: readings.pitch_out,
        left: lrud.left,
        right: lrud.right,
        up: lrud.up,
        down: lrud.down,
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
    // The FORMAT's length and inclination units describe the surveyor's
    // NOTEBOOK, not the file: Compass stores every length and LRUD in decimal
    // feet and every inclination in decimal degrees ("Survey Data File Format",
    // Compass Editor help), and Survex reads them that way - for a depth-gauge
    // (`W`) survey too, which Survex only marks as diving style. So nothing
    // here depends on those units (#576 B28).
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
    // FORMAT `T` = "To Station" (Compass "Survey Data File Format"); the
    // 11/12-character formats carry no flag and mean From.
    let lrud_on_to = file_state.lrud_on_to_override.unwrap_or_else(|| {
        matches!(
            survey.parameters.format_parameters.lrud_association(),
            Some(LRUDAssociation::ToStation)
        )
    });
    SurveyCtx {
        azimuth_reference,
        apply_corrections: opts.apply_corrections,
        corrections,
        backsight_corrections,
        lrud_on_to,
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
        let has_backsights = survey.parameters.format_parameters.has_backsights();
        let mut reconstructed = 0;
        let mut shots = Vec::new();
        for shot in &survey.shots {
            let flags = parse_flags(shot.flags.as_ref());
            if flags.exclude_all && !opts.include_excluded {
                continue;
            }
            // One warning per shot naming every corrupt (non-finite) raw
            // reading. Sentinel-coded missing values (-999/+999) are not
            // corruption but the format's "no reading" marker; what becomes
            // of them is reported below, once the readings are resolved
            // (#576 B30). Absent backsights read as finite 0.0 here.
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
            let length_m = shot_length_m(shot, &ctx);
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
            let readings = resolve_readings(shot, &ctx);
            reconstructed += readings.reconstructed;
            warnings.extend(missing_reading_warning(shot, &readings, has_backsights));
            edges.push((
                shot.from.clone(),
                shot.to.clone(),
                elevation_gain(length_m, readings.inclination),
            ));
            shots.push(shot);
        }
        warnings.extend(reconstructed_readings_note(survey, reconstructed));
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
    // later-processed survey, leaving those chains' shots at the origin).
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
        let split = split_into_chains(&kept_survey.shots, |s| (s.from.as_str(), s.to.as_str()));
        let to_station_lruds = kept_survey
            .ctx
            .lrud_on_to
            .then(|| ToStationLruds::new(&kept_survey.shots, &split));
        if let Some(lruds) = &to_station_lruds {
            let survey = &kept_survey.survey.name;
            warnings.extend(end_of_chain_lrud_note(survey, lruds.stranded));
            warnings.extend(superseded_lrud_note(survey, lruds.superseded));
        }
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
            for (pos, &idx) in chain.iter().enumerate() {
                let shot = kept_survey.shots[idx];
                let flags = parse_flags(shot.flags.as_ref());
                let lrud = match &to_station_lruds {
                    Some(lruds) => lruds.at(&kept_survey.shots, chain, pos),
                    None => Lrud::of(shot),
                };
                shots_data.push(shot_to_shot_data(
                    shot,
                    &kept_survey.ctx,
                    lrud,
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

    // Pass 2b: a spanning-tree placement over chain connectivity (anchor ->
    // ZeroDatum, each reached chain -> FromImporting onto its discoverer,
    // every other component seeded at Origin once), then 2c: loop closures
    // for the shared stations the tree did not merge. Compass's own
    // connection rule keeps every re-import where it was (#576 B3).
    let names: Vec<&[String]> = chains.iter().map(|c| c.names.as_slice()).collect();
    let policy = PlacementPolicy::Compass {
        anchor: anchor.as_ref().map(|a| Anchor {
            station: &a.name,
            coordinates: a.coordinates,
        }),
    };
    let placements = place_chains(&names, &policy, &mut warnings, |i| chains[i].label.clone());
    let closures = build_closures(&names, &placements);

    // Emit configs in BFS creation order so FromImporting targets always
    // precede their dependents at create time.
    let mut configs: Vec<ImportSectionConfig> = Vec::with_capacity(chains.len());
    for &ci in &placements.creation_order {
        let chain = &chains[ci];
        let p = placements.placement[ci]
            .clone()
            .unwrap_or(ImportPlacement::Origin);
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
            azimuth_reference: Some(chain.azimuth_reference),
        });
    }

    topological_sort_configs(&mut configs, ConfigOrder::Rebuild);

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

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]
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
    // not committed; point `COMPASS_PRIVATE_MAK` at a `.MAK` (its .DAT files
    // beside it), or place it as `PrivateCave.MAK` under
    // `src-tauri/tests/fixtures/compass/`, and re-run with
    // `cargo test -- --ignored` to enable them. Run without it, each fails
    // naming the missing file (#576 E13). [#576 HFI-11] They hold for ANY
    // Compass project - no dataset's numbers or names belong in them.
    fn private_cave_mak() -> PathBuf {
        match std::env::var_os("COMPASS_PRIVATE_MAK") {
            Some(_) => crate::models::test_support::require_env_file("COMPASS_PRIVATE_MAK"),
            None => crate::models::test_support::require_file(fixture("PrivateCave.MAK")),
        }
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_mak_reads_georeference() {
        let parsed = parse_compass(&private_cave_mak()).unwrap();
        assert!(!parsed.surveys.is_empty());
        assert!(parsed.datum.is_some());
        assert!(parsed.utm_zone.is_some());
        assert!(
            !parsed.fixed_stations.is_empty(),
            "expected at least one fixed station to anchor the project"
        );
    }

    /// The geometry is inclination-based, +down (#576 B28: the inclination
    /// column is an ordinary angle, negative = down, whatever the survey FORMAT
    /// says about how the notebook was kept - a `W` format once read it as a
    /// depth difference with the sign reversed). [#576 HFI-11] Was one dataset's
    /// first shot, to the centimetre; now what holds for any project:
    /// - depth is a function of the station: consecutive shots of a section
    ///   agree on the station they share, and the anchor station is the datum;
    /// - each station's depth comes from the shot that first reached it from
    ///   the anchor, as `length x sin(mean pitch)`, so every shot except those
    ///   closing a loop carries exactly that change. A survey network with as
    ///   many loop-closing shots as the rest would be a grid, not a cave: MORE
    ///   than half of the shots must match.
    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_first_shot_geometry_is_depth_based() {
        let parsed = parse_compass(&private_cave_mak()).unwrap();
        let (configs, _closures, _warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        assert!(!configs.is_empty());

        for c in &configs {
            for pair in c.shots.windows(2) {
                assert!(
                    (pair[0].depth_out - pair[1].depth_in).abs() < 1e-9,
                    "section {:?}: consecutive shots disagree on their station's depth",
                    c.name
                );
            }
        }
        for c in &configs {
            if let ImportPlacement::ZeroDatum { station_index, .. } = c.placement {
                let k = station_index as usize;
                let depth = match c.shots.get(k) {
                    Some(shot) => shot.depth_in,
                    None => c.shots[k - 1].depth_out,
                };
                assert!(depth.abs() < 1e-9, "the anchor station sits at {depth}");
            }
        }

        let shots: Vec<&ShotData> = configs.iter().flat_map(|c| &c.shots).collect();
        let own_pitch = shots
            .iter()
            .filter(|s| {
                let pitch_in = s.pitch_in.unwrap_or(0.0);
                let mean = s
                    .pitch_out
                    .map_or(pitch_in, |out| f64::midpoint(pitch_in, out));
                let change = s.length * mean.to_radians().sin();
                (s.depth_out - s.depth_in - change).abs() < 1e-6
            })
            .count();
        println!(
            "{own_pitch} of {} shots carry the depth change of their own pitch",
            shots.len()
        );
        assert!(
            own_pitch * 2 > shots.len(),
            "{own_pitch} of {} shots carry the depth change of their own +down pitch",
            shots.len()
        );
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_zero_datum_projects_to_plausible_coords() {
        let parsed = parse_compass(&private_cave_mak()).unwrap();
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

    /// `X`-flagged (excluded) shots land only when asked for, and then as
    /// reference shots - by the preview's own accounting (#437), exact for
    /// both toggles. [#576 HFI-11] Was "the fixture has an all-excluded
    /// survey", one dataset's shape; this holds for any project, including one
    /// with no excluded shot at all.
    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_excluded_shots_are_filtered_by_default() {
        let parsed = parse_compass(&private_cave_mak()).unwrap();
        let convert = |include_excluded| {
            convert_compass_to_import_configs(
                &parsed,
                &CompassImportOptions {
                    include_excluded,
                    apply_corrections: true,
                    comments_on_stations: true,
                },
            )
            .unwrap()
            .0
        };
        let (default_configs, included_configs) = (convert(false), convert(true));
        let preview = build_preview(&parsed);
        let invalid = &preview.invalid_shot_counts;
        println!(
            "shots={} excluded={} default import={} with excluded={}",
            preview.shot_count,
            preview.excluded_shot_count,
            total_shots(&default_configs),
            total_shots(&included_configs)
        );
        assert_eq!(
            total_shots(&default_configs) as u32,
            preview.shot_count - preview.excluded_shot_count - invalid.non_excluded_corrected
        );
        assert_eq!(
            total_shots(&included_configs) as u32,
            preview.shot_count - invalid.non_excluded_corrected - invalid.excluded_corrected
        );
        let virtual_shots = |configs: &[ImportSectionConfig]| {
            configs
                .iter()
                .flat_map(|c| &c.shots)
                .filter(|s| s.kind.is_virtual())
                .count() as u32
        };
        assert_eq!(virtual_shots(&default_configs), 0);
        assert_eq!(
            virtual_shots(&included_configs),
            preview.excluded_shot_count - invalid.excluded_corrected
        );
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

    /// Compass convention: a shot comment describes the FROM station (where the
    /// surveyor stood). With comments_on_stations on (the default) every
    /// non-empty shot comment becomes a station comment at station_index = shot
    /// index and leaves the shot; off, nothing moves. [#576 HFI-11] Was "the
    /// anchor section's first shot carries a comment", one dataset's shape; this
    /// compares the two imports section by section, for any project.
    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_comments_promoted_to_from_station_by_default() {
        let parsed = parse_compass(&private_cave_mak()).unwrap();
        let (promoted, _, _) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        let (kept_on_shots, _, _) = convert_compass_to_import_configs(
            &parsed,
            &CompassImportOptions {
                comments_on_stations: false,
                ..CompassImportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(promoted.len(), kept_on_shots.len());
        let mut moved = 0;
        for kept in &kept_on_shots {
            let on = promoted
                .iter()
                .find(|c| c.section_index == kept.section_index)
                .expect("the same sections either way");
            assert!(kept.station_comments.is_empty(), "off: nothing moves");
            assert!(
                on.shots.iter().all(|s| s.comment.is_none()),
                "on: every comment leaves its shot"
            );
            let expected: Vec<(u32, String)> = kept
                .shots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.comment.clone().map(|c| (i as u32, c)))
                .collect();
            assert!(expected.iter().all(|(_, c)| !c.is_empty()));
            assert_eq!(on.station_comments, expected, "section {:?}", kept.name);
            moved += expected.len();
        }
        println!("{moved} shot comments promoted to their FROM station");
    }

    #[test]
    #[ignore = "uses private cave survey data; fixtures kept outside the repo"]
    fn private_cave_every_section_connects_to_the_zero_datum() {
        // Regression guard for the spanning-tree placement: every Compass
        // section in the private corpus must attach to the network through a
        // station-name path back to the zero datum (1 ZeroDatum + the rest
        // FromImporting), with no chains stranded at origin.  A one-pass
        // first-occurrence placement previously left sections detached
        // because their connection station was first defined in a
        // later-processed survey.
        let parsed = parse_compass(&private_cave_mak()).unwrap();
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
        let parsed = parse_compass(&private_cave_mak()).unwrap();
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
        // A `.MAK` with two `#file` entries, built below: file 0 uses
        // NAD-1983 / UTM 13 / declination=Entered (default), file 1
        // switches to WGS 1984 / UTM 14 / declination=Auto.  parse_mak
        // mirrors file 0's values into the project-wide preview fields
        // and emits one warning per divergence so the user knows the
        // preview only reflects file 0.  Geometry is resolved per-file
        // through `SurveyWithState.state` regardless of these warnings.
        //
        // The two files are two DISTINCT synthetic `.DAT`s: a second listing
        // of one file is skipped (#576 RV2-R-12). This test used to read the
        // tracked fixture `tests/fixtures/compass/multi_zone.mak`, which listed
        // one `.DAT` twice; nothing reads that fixture any more (it is unused,
        // left for the maintainer to delete by hand - #576 RV4-R-P3).
        let dir = tempfile::tempdir().unwrap();
        for (name, survey) in [("a.dat", "A"), ("b.dat", "B")] {
            std::fs::write(
                dir.path().join(name),
                synthetic_survey_record("Synthetic Cave", survey, "7 10 1979"),
            )
            .unwrap();
        }
        let mak = "&North American 1983;\r\n$13;\r\n@500000.000,4000000.000,1000.000,13,0.000;\r\n\
                   !E;\r\n#a.dat;\r\n\r\n\
                   &WGS 1984;\r\n$14;\r\n@500000.000,4000000.000,1000.000,14,0.000;\r\n\
                   !A;\r\n#b.dat;\r\n";
        let parsed = parse_mak_text(dir.path(), mak.as_bytes()).unwrap();
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

    /// #576 (B26): `shot_geometry` wrapped the bearing with a bare
    /// `rem_euclid(360.0)`, which returns EXACTLY `360.0` for a tiny negative
    /// reading. `360.0` trips `INVALID_AZIMUTH`, an error the user cannot
    /// dismiss.
    #[test]
    fn a_tiny_negative_bearing_imports_as_north_not_360() {
        let dir = tempfile::tempdir().unwrap();
        let dat = dir.path().join("tiny-negative-bearing.dat");
        let text = synthetic_dat("Synthetic Cave")
            .replace("A1 A2 10.00 90.00 ", "A1 A2 10.00 -0.0000000000000001 ");
        assert!(
            text.contains("-0.0000000000000001"),
            "premise: bearing replaced"
        );
        std::fs::write(&dat, text).unwrap();

        let parsed = parse_compass(&dat).unwrap();
        let (configs, _closures, _warnings) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();

        let shot = &configs[0].shots[0];
        for (name, az) in [
            ("azimuth_in", shot.azimuth_in),
            ("azimuth_out", shot.azimuth_out),
        ] {
            assert!(
                (0.0..360.0).contains(&az) && az.abs() < 1e-9,
                "{name} = {az:?}: expected north in [0, 360), not a value that trips \
                 INVALID_AZIMUTH"
            );
        }
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

    /// Compass is a Windows program and writes its `.DAT` text in
    /// Windows-1252, where `0x92` is the right single quote a word processor
    /// types for an apostrophe. Read as ISO-8859-1 it is U+0092, an invisible
    /// C1 control, so the comment the user sees reads "dont" with a hole in it.
    #[test]
    fn a_windows_1252_apostrophe_in_a_dat_comment_survives_the_import() {
        let dir = tempfile::tempdir().unwrap();
        let record = synthetic_survey_record("Synthetic Cave", "A", "7 10 1979");
        // The record ends on its one shot line; give that shot a comment.
        let mut bytes = record.trim_end().as_bytes().to_vec();
        bytes.extend_from_slice(b" don\x92t\r\n");
        let path = dir.path().join("apostrophe.dat");
        std::fs::write(&path, &bytes).unwrap();

        let parsed = parse_compass(&path).unwrap();
        let comment = parsed.surveys[0].survey.shots[0]
            .comment
            .clone()
            .unwrap_or_default();
        let code_points: Vec<String> = comment
            .chars()
            .map(|c| format!("U+{:04X}", u32::from(c)))
            .collect();
        println!("shot comment: {comment:?} = {}", code_points.join(" "));
        assert_eq!(
            comment,
            "don\u{2019}t",
            "0x92 is U+2019 in Windows-1252, got {}",
            code_points.join(" ")
        );
    }

    /// A one-survey `.DAT` whose shot line carries `comment` as raw bytes,
    /// written into `dir`.
    fn dat_with_comment_bytes(dir: &Path, comment: &[u8]) -> PathBuf {
        let record = synthetic_survey_record("Synthetic Cave", "A", "7 10 1979");
        let mut bytes = record.trim_end().as_bytes().to_vec();
        bytes.push(b' ');
        bytes.extend_from_slice(comment);
        bytes.extend_from_slice(b"\r\n");
        let path = dir.join("synthetic.dat");
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    /// The five bytes Windows-1252 leaves undefined (0x81, 0x8D, 0x8F, 0x90,
    /// 0x9D) decode to U+FFFD, which loses whatever the surveyor typed. The
    /// TML reader counts that loss on the receipt; a bare `.DAT` must too, on
    /// the preview and on the receipt, rather than hand back a comment with
    /// holes in it and no word about them.
    ///
    /// Without the fix the decode is silent and neither list carries it.
    #[test]
    fn undefined_windows_1252_bytes_in_a_dat_are_counted_in_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        // Two undefined bytes, and one defined one (0x92) that costs nothing.
        let path = dat_with_comment_bytes(dir.path(), b"C\x81v\x9de don\x92t");

        let parsed = parse_compass(&path).unwrap();
        let comment = parsed.surveys[0].survey.shots[0]
            .comment
            .clone()
            .unwrap_or_default();
        assert_eq!(
            comment, "C\u{fffd}v\u{fffd}e don\u{2019}t",
            "premise: the bytes reached the comment"
        );
        let preview = build_preview(&parsed).warnings;
        let (_configs, _closures, receipt) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        println!("preview warnings: {preview:?}");
        println!("receipt warnings: {receipt:?}");

        let expected =
            "The file was read as windows-1252, the encoding Compass writes; 2 characters were \
             replaced";
        assert_eq!(preview, [expected], "one warning, for the one file read");
        assert_eq!(
            receipt
                .iter()
                .filter(|w| w.contains("windows-1252"))
                .collect::<Vec<_>>(),
            [expected],
            "the warning must reach the import receipt too"
        );
    }

    /// The warning must stay rare enough to mean something: a `.DAT` whose
    /// high bytes are all defined in Windows-1252 lost nothing and gets no
    /// warning at all.
    #[test]
    fn a_dat_with_no_undefined_byte_gets_no_encoding_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dat_with_comment_bytes(dir.path(), b"\x80 don\x92t caf\xe9 \x85");

        let parsed = parse_compass(&path).unwrap();
        let comment = parsed.surveys[0].survey.shots[0]
            .comment
            .clone()
            .unwrap_or_default();
        assert_eq!(
            comment, "\u{20ac} don\u{2019}t caf\u{e9} \u{2026}",
            "premise: the high bytes reached the comment"
        );
        let preview = build_preview(&parsed).warnings;
        let (_configs, _closures, receipt) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        println!("preview warnings: {preview:?}");
        println!("receipt warnings: {receipt:?}");
        assert!(preview.is_empty(), "got {preview:?}");
        assert!(
            receipt.iter().all(|w| !w.contains("windows-1252")),
            "got {receipt:?}"
        );
    }

    // -----------------------------------------------------------------------
    // #576 W2-COMPASS: in-memory synthetic surveys (no file on disk).
    // -----------------------------------------------------------------------

    /// One survey's header lines for `format`, with or without the backsight
    /// columns. Synthetic - no fixture data.
    fn synthetic_header(survey_name: &str, format: &str, backsights: bool) -> String {
        let columns = if backsights {
            "FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT AZM2 INC2 FLAGS COMMENTS"
        } else {
            "FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS"
        };
        format!(
            "Synthetic Cave\r\n\
             SURVEY NAME: {survey_name}\r\n\
             SURVEY DATE: 7 10 1979\r\n\
             SURVEY TEAM:\r\n\
             Nobody\r\n\
             DECLINATION: 0.00  FORMAT: {format}  CORRECTIONS: 0.00 0.00 0.00\r\n\
             \r\n\
             {columns}\r\n\
             \r\n"
        )
    }

    /// One survey record: the header for `format` plus `rows`, ended with the
    /// form feed Compass writes after every survey.
    pub(super) fn synthetic_survey_text(survey_name: &str, format: &str, rows: &[&str]) -> String {
        // The backsight flag is the 12th FORMAT character in the 12- and
        // 13-character forms and the 14th in the 15-character form.
        let flag = match format.len() {
            12 | 13 => format.chars().nth(11),
            15 => format.chars().nth(13),
            _ => None,
        };
        let mut text = synthetic_header(survey_name, format, flag == Some('B'));
        for row in rows {
            text.push_str(row);
            text.push_str("\r\n");
        }
        text.push_str("\x0c\r\n");
        text
    }

    /// Parse `.DAT` text the way `parse_dat` does once it has read the file,
    /// so a test can build its surveys in memory.
    pub(super) fn parse_dat_text(text: &str) -> ParsedCompass {
        let (surveys, remainder) = Survey::parse_dat_file_with_remainder(text).unwrap();
        assert!(
            only_record_separators(remainder),
            "premise: the synthetic text parses to the end, left {remainder:?}"
        );
        ParsedCompass {
            cave_name: String::new(),
            datum: None,
            utm_zone: None,
            declination_mode: DeclinationMode::default(),
            fixed_stations: Vec::new(),
            surveys: surveys
                .into_iter()
                .map(|survey| SurveyWithState {
                    survey,
                    state: CompassFileState::default(),
                })
                .collect(),
            warnings: Vec::new(),
        }
    }

    /// Convert one synthetic survey named `A` with the default options.
    fn convert_text(
        format: &str,
        rows: &[&str],
    ) -> (Vec<ImportSectionConfig>, Vec<ImportClosure>, Vec<String>) {
        let parsed = parse_dat_text(&synthetic_survey_text("A", format, rows));
        convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap()
    }

    fn all_shots(configs: &[ImportSectionConfig]) -> Vec<&ShotData> {
        configs.iter().flat_map(|c| c.shots.iter()).collect()
    }

    /// #576 B28: a `.DAT` stores every length and LRUD in decimal feet - the
    /// FORMAT line only records how the surveyor wrote them in the notebook
    /// (Compass's "Survey Data File Format" page; Survex reads them the same
    /// way). A metric-notebook survey storing `32.81` is a 10 m leg, not a
    /// 32.81 m one, and its `3.28` LRUD is 1 m.
    #[test]
    fn a_metric_format_survey_still_stores_feet() {
        let rows = ["A1 A2 32.81 90.00 -10.00 3.28 3.28 3.28 3.28"];
        let (configs, _, _) = convert_text("DMMDLRUDLADN", &rows);
        let shot = all_shots(&configs)[0];
        println!(
            "FORMAT DMMDLRUDLADN, stored 32.81 / LRUD 3.28 -> length {:.3} m, left {:?} m",
            shot.length, shot.left
        );
        assert!(
            (shot.length - 32.81 * FEET_TO_METERS).abs() < 1e-9,
            "the stored 32.81 is decimal feet, got {} m",
            shot.length
        );
        for (name, v) in [
            ("left", shot.left),
            ("up", shot.up),
            ("down", shot.down),
            ("right", shot.right),
        ] {
            let v = v.unwrap();
            assert!(
                (v - 3.28 * FEET_TO_METERS).abs() < 1e-9,
                "{name}: the stored 3.28 is decimal feet, got {v} m"
            );
        }

        // The import dialog and the receipt say what was done to the numbers:
        // they were converted from feet, whatever the notebook used.
        let parsed = parse_dat_text(&synthetic_survey_text("A", "DMMDLRUDLADN", &rows));
        assert_eq!(preview_scalars(&parsed).1.as_deref(), Some("feet"));
        assert_eq!(build_preview(&parsed).length_unit.as_deref(), Some("feet"));
    }

    /// #576 B28: a `.DAT` stores every inclination in decimal degrees, whatever
    /// unit the FORMAT says the notebook used (degrees `D`, percent grade `G`,
    /// degrees-and-minutes `M`, grads `R`, depth gauge `W`). Each stored value
    /// below is a downward angle, so it imports as a `+down` pitch of the same
    /// size and the leg descends by `length * sin(angle)`.
    #[test]
    fn every_inclination_unit_stores_decimal_degrees() {
        let mut wrong: Vec<String> = Vec::new();
        for (unit, stored) in [
            ('D', -45.0_f64),
            ('G', -45.0),
            ('M', -12.5),
            ('R', -45.0),
            ('W', -30.0),
        ] {
            let format = format!("DDD{unit}LRUDLADN");
            let row = format!("A1 A2 32.81 90.00 {stored:.2} 1.00 1.00 1.00 1.00");
            let (configs, _, _) = convert_text(&format, &[&row]);
            let shot = all_shots(&configs)[0];
            let pitch = shot.pitch_in.unwrap();
            let drop = shot.depth_out - shot.depth_in;
            let expected_drop = 32.81 * FEET_TO_METERS * (-stored).to_radians().sin();
            println!(
                "FORMAT {format}: stored {stored:.2} -> pitch_in {pitch:.3} (+down), \
                 depth change {drop:+.3} m (expected {:.3}, {expected_drop:+.3} m)",
                -stored
            );
            if (pitch + stored).abs() > 1e-9 || (drop - expected_drop).abs() > 1e-6 {
                wrong.push(format);
            }
        }
        assert!(
            wrong.is_empty(),
            "every stored inclination is decimal degrees; read otherwise for {wrong:?}"
        );
    }

    /// #576 B28 (decision D-8): a depth-gauge (`W`) survey's inclination field
    /// is an ordinary angle in degrees, negative = down, exactly as Survex
    /// reads it (it only marks the survey as diving style). A stored `-5.86`
    /// on a 49 ft leg is a 1.525 m DESCENT; 19b3cb53 read it as a depth
    /// difference with the sign backwards and made the leg climb 1.79 m.
    #[test]
    fn a_depth_gauge_survey_descends_on_a_negative_inclination() {
        let (configs, _, warnings) = convert_text(
            "DDDWLRUDLADN",
            &[
                "A1 A2 49.00 0.00 -5.86 1.00 1.00 1.00 1.00",
                "A2 A3 20.00 0.00 -30.00 1.00 1.00 1.00 1.00",
            ],
        );
        let shots = all_shots(&configs);
        for (i, s) in shots.iter().enumerate() {
            println!(
                "W shot {i}: pitch_in {:?}, depth {:.3} -> {:.3}",
                s.pitch_in, s.depth_in, s.depth_out
            );
        }
        let first_drop = shots[0].depth_out - shots[0].depth_in;
        assert!(
            (first_drop - 1.5249).abs() < 1e-3,
            "-5.86 deg over 49 ft descends 1.525 m, got {first_drop:+.3} m"
        );
        let second_drop = shots[1].depth_out - shots[1].depth_in;
        assert!(
            (second_drop - 20.0 * FEET_TO_METERS * 0.5).abs() < 1e-6,
            "-30 deg over 20 ft descends 3.048 m, got {second_drop:+.3} m"
        );
        assert!((shots[0].pitch_in.unwrap() - 5.86).abs() < 1e-9);
        assert!(
            warnings.iter().all(|w| !w.contains("DepthGauge")),
            "no warning describes a depth-difference reading any more: {warnings:?}"
        );
    }

    /// #576 B28: the backsight inclination is decimal degrees too, so a grads
    /// FORMAT no longer scales it (this replaces the dababa5c test that pinned
    /// the x0.9 conversion on `backsight_grads.dat`).
    #[test]
    fn a_grads_format_backsight_inclination_stores_degrees() {
        let (configs, _, _) = convert_text(
            "DDDRLRUDLADBF",
            &["A1 A2 32.81 90.00 -45.00 1.00 1.00 1.00 1.00 270.00 45.00"],
        );
        let shot = all_shots(&configs)[0];
        println!(
            "FORMAT DDDRLRUDLADBF, fore -45.00 back 45.00 -> pitch_in {:?} pitch_out {:?}",
            shot.pitch_in, shot.pitch_out
        );
        assert!((shot.pitch_in.unwrap() - 45.0).abs() < 1e-9);
        assert!((shot.pitch_out.unwrap() - 45.0).abs() < 1e-9);
    }

    /// Print a converted survey's shots, one line each, for test captures.
    fn print_shots(label: &str, configs: &[ImportSectionConfig], warnings: &[String]) {
        println!("{label}");
        for (i, s) in all_shots(configs).iter().enumerate() {
            println!(
                "  shot {i}: az_in {:.2} az_out {:.2} pitch_in {:?} pitch_out {:?} depth {:.3} -> {:.3}",
                s.azimuth_in, s.azimuth_out, s.pitch_in, s.pitch_out, s.depth_in, s.depth_out
            );
        }
        for w in warnings {
            println!("  warning: {w}");
        }
    }

    /// #576 B30: with backsights, -999 marks a reading that was not taken, and
    /// Compass processes the shot from the reading that WAS taken ("Compass can
    /// still process the shot as long as there is at least one compass reading
    /// and one inclinometer reading" - Compass Editor help, "Backsights and
    /// Redundant Backsights"). A 10 m leg at 90 deg, 30 deg down, whose
    /// foresight is missing, used to import pointing north and level, with
    /// nothing said.
    #[test]
    fn a_missing_foresight_is_taken_from_the_reversed_backsight() {
        let (configs, _, warnings) = convert_text(
            "DDDDLRUDLADBF",
            &["A1 A2 32.81 -999.00 -999.00 1.00 1.00 1.00 1.00 270.00 30.00"],
        );
        print_shots("fore -999/-999, back 270/+30", &configs, &warnings);
        let shot = all_shots(&configs)[0];
        assert!(
            (shot.azimuth_in - 90.0).abs() < 1e-9 && (shot.azimuth_out - 90.0).abs() < 1e-9,
            "bearing = back bearing + 180 in both slots, got {} / {}",
            shot.azimuth_in,
            shot.azimuth_out
        );
        assert_eq!(
            shot.pitch_in,
            Some(30.0),
            "inclination = the negated backsight (+30 back is 30 down)"
        );
        assert_eq!(shot.pitch_out, None, "only one inclination was taken");
        let drop = shot.depth_out - shot.depth_in;
        assert!(
            (drop - 32.81 * FEET_TO_METERS * 0.5).abs() < 1e-6,
            "the leg descends 5 m, got {drop:+.3} m"
        );
        let notes: Vec<&String> = warnings
            .iter()
            .filter(|w| w.contains("taken from the backsight"))
            .collect();
        assert_eq!(
            notes,
            [
                "Survey 'A': 2 missing foresight readings were taken from the backsight \
              (bearing + 180, inclination negated), as Compass does."
            ],
            "one receipt line per survey counts the reconstructed readings"
        );
    }

    /// #576 B30, mirror case: a missing BACK inclination is no reading, not a
    /// level one. `Some(0.0)` was averaged with the 30 deg foresight into a
    /// 15 deg "recorded" inclination that disagrees with the shot's own depth.
    #[test]
    fn a_missing_back_inclination_is_absent_not_level() {
        let (configs, _, warnings) = convert_text(
            "DDDDLRUDLADBF",
            &["A1 A2 32.81 90.00 -30.00 1.00 1.00 1.00 1.00 270.00 -999.00"],
        );
        print_shots("fore 90/-30, back 270/-999", &configs, &warnings);
        let shot = all_shots(&configs)[0];
        assert_eq!(shot.pitch_in, Some(30.0));
        assert_eq!(shot.pitch_out, None, "no back inclination was taken");
        assert!(
            (shot.azimuth_out - 90.0).abs() < 1e-9,
            "the back bearing stays"
        );
        assert!(
            warnings.iter().all(|w| !w.contains("backsight")),
            "nothing was reconstructed: {warnings:?}"
        );
    }

    /// #576 B30: the two halves of a backsight are independent readings. A
    /// missing back BEARING used to throw away a valid back inclination.
    #[test]
    fn a_missing_back_bearing_keeps_the_back_inclination() {
        let (configs, _, warnings) = convert_text(
            "DDDDLRUDLADBF",
            &["A1 A2 32.81 90.00 -30.00 1.00 1.00 1.00 1.00 -999.00 30.00"],
        );
        print_shots("fore 90/-30, back -999/+30", &configs, &warnings);
        let shot = all_shots(&configs)[0];
        assert!(
            (shot.azimuth_out - 90.0).abs() < 1e-9,
            "mirrors the foresight"
        );
        assert_eq!(shot.pitch_out, Some(30.0), "the back inclination was taken");
    }

    /// #576 B30: a field missing from BOTH sights has nothing to reconstruct
    /// from. Compass's editor refuses to let it be entered ("The program will
    /// flag an error if you try enter missing data for both the fore and
    /// backsight item"), and its help does not say what processing does with
    /// one, so the leg keeps today's placeholder (north, level) and the
    /// receipt names it. A plumb leg's bearing does not matter, so it is not
    /// named for that.
    #[test]
    fn a_reading_missing_from_both_sights_keeps_the_leg_and_names_it() {
        let (configs, _, warnings) = convert_text(
            "DDDDLRUDLADBF",
            &[
                "A1 A2 32.81 -999.00 -999.00 1.00 1.00 1.00 1.00 -999.00 -999.00",
                "A2 A3 10.00 -999.00 -90.00 1.00 1.00 1.00 1.00 -999.00 90.00",
            ],
        );
        print_shots("both sights -999, then a plumb leg", &configs, &warnings);
        let shots = all_shots(&configs);
        assert_eq!(shots.len(), 2, "both legs are kept");
        assert!(shots[0].azimuth_in.abs() < 1e-12, "{}", shots[0].azimuth_in);
        assert_eq!(shots[0].pitch_in, Some(0.0));
        assert!(
            warnings.iter().any(|w| w
                == "Shot A1 -> A2: bearing and inclination are missing from both the \
                    foresight and the backsight; recorded as 0 (due north) and level."),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().all(|w| !w.contains("A2 -> A3")),
            "a plumb leg needs no bearing: {warnings:?}"
        );

        // Without backsight columns the foresight is the only sight.
        let (_, _, warnings) = convert_text(
            "DDDDLRUDLADN",
            &["A1 A2 10.00 -999.00 0.00 1.00 1.00 1.00 1.00"],
        );
        println!("no backsight columns, bearing -999: {warnings:?}");
        assert!(
            warnings.iter().any(|w| w
                == "Shot A1 -> A2: bearing is missing and the survey has no backsight to \
                    take it from; recorded as 0 (due north)."),
            "{warnings:?}"
        );
    }

    /// #576 NF-AN4-1: with both inclinations taken, Compass averages them ("When
    /// you process the data, the two numbers are averaged" - Compass Editor
    /// help, "Backsights and Redundant Backsights"; Survex does the same), and
    /// the bearings already were. Depth used the foresight alone, so a 28 deg
    /// foresight with a 32 deg backsight dropped 4.70 m instead of the 5.00 m
    /// of the 30 deg mean - and disagreed with the shot's own pitch pair.
    #[test]
    fn depth_follows_the_mean_of_fore_and_back_inclination() {
        let (configs, _, warnings) = convert_text(
            "DDDDLRUDLADBF",
            &["A1 A2 32.81 90.00 -28.00 1.00 1.00 1.00 1.00 270.00 32.00"],
        );
        print_shots("fore 90/-28, back 270/+32", &configs, &warnings);
        let shot = all_shots(&configs)[0];
        assert_eq!(shot.pitch_in, Some(28.0), "the foresight is kept as read");
        assert_eq!(shot.pitch_out, Some(32.0), "the backsight is kept as read");
        let drop = shot.depth_out - shot.depth_in;
        let mean = 32.81 * FEET_TO_METERS * 30.0_f64.to_radians().sin();
        assert!(
            (drop - mean).abs() < 1e-6,
            "the leg descends by the 30 deg mean, {mean:.3} m, got {drop:+.3} m"
        );
    }

    /// The four LRUDs of a shot, in meters, rounded for printing.
    fn lrud_of(shot: &ShotData) -> [Option<f64>; 4] {
        [shot.left, shot.right, shot.up, shot.down]
            .map(|v| v.map(|m| (m * 10_000.0).round() / 10_000.0))
    }

    /// The receipt line for `n` LRUDs a To-associated survey `A` could not place.
    fn end_of_chain_line(n: usize) -> String {
        if n == 1 {
            "Survey 'A': 1 end-of-chain cross-section had no shot to attach to and was dropped \
             (the survey records LRUDs at each shot's To station)."
                .to_string()
        } else {
            format!(
                "Survey 'A': {n} end-of-chain cross-sections had no shot to attach to and were \
                 dropped (the survey records LRUDs at each shot's To station)."
            )
        }
    }

    /// #576 B31: a FORMAT ending in `T` records each row's LRUD at the shot's
    /// TO station (Compass "Survey Data File Format": "LRUD Association:
    /// F=From Station, T=To Station"; Survex `compass_lrud_on_to`). A shot
    /// carries the LRUD of its FROM station, so a row's cross-section belongs
    /// on the NEXT shot - it used to land one station early.
    #[test]
    fn a_to_associated_survey_puts_each_lrud_one_station_later() {
        let (configs, _, warnings) = convert_text(
            "DDDDLRUDLADNT",
            &[
                "A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00",
                "A2 A3 10.00 90.00 0.00 5.00 5.00 5.00 5.00",
            ],
        );
        let shots = all_shots(&configs);
        for (i, s) in shots.iter().enumerate() {
            println!("To survey shot {i}: LRUD (L, R, U, D) {:?}", lrud_of(s));
        }
        for w in &warnings {
            println!("  warning: {w}");
        }
        let one_foot = Some(0.3048);
        assert_eq!(lrud_of(shots[0]), [None; 4], "A1 has no measured section");
        assert_eq!(
            lrud_of(shots[1]),
            [one_foot; 4],
            "the 1 ft section measured at A2 rides on A2 -> A3"
        );
        assert!(
            warnings.contains(&end_of_chain_line(1)),
            "the 5 ft section at A3 has no shot from A3: {warnings:?}"
        );

        // The default (From) association is untouched.
        let (configs, _, warnings) = convert_text(
            "DDDDLRUDLADNF",
            &[
                "A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00",
                "A2 A3 10.00 90.00 0.00 5.00 5.00 5.00 5.00",
            ],
        );
        let shots = all_shots(&configs);
        assert_eq!(lrud_of(shots[0]), [one_foot; 4]);
        assert_eq!(lrud_of(shots[1]), [Some(1.524); 4]);
        assert!(warnings.iter().all(|w| !w.contains("cross-section")));
    }

    /// #576 B31: a To LRUD belongs to a STATION, so a chain that starts at a
    /// station another row ended at (a branch) picks up that station's
    /// section; only a section whose station starts no shot at all is lost.
    #[test]
    fn a_to_associated_lrud_reaches_a_branch_that_starts_at_its_station() {
        let (configs, _, warnings) = convert_text(
            "DDDDLRUDLADNT",
            &[
                "A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00",
                "A2 A3 10.00 90.00 0.00 2.00 2.00 2.00 2.00",
                "A2 B1 10.00 0.00 0.00 3.00 3.00 3.00 3.00",
                "B1 B2 10.00 0.00 0.00 4.00 4.00 4.00 4.00",
            ],
        );
        assert_eq!(configs.len(), 2, "premise: the branch is its own section");
        let shots = all_shots(&configs);
        for (i, s) in shots.iter().enumerate() {
            println!("To survey shot {i}: LRUD {:?}", lrud_of(s));
        }
        let ft = |n: f64| [Some((n * FEET_TO_METERS * 10_000.0).round() / 10_000.0); 4];
        // Chain 1: A1 -> A2 -> A3; chain 2 (branch): A2 -> B1 -> B2.
        assert_eq!(lrud_of(shots[0]), [None; 4], "A1 -> A2");
        assert_eq!(lrud_of(shots[1]), ft(1.0), "A2 -> A3 carries A2's section");
        assert_eq!(
            lrud_of(shots[2]),
            ft(1.0),
            "A2 -> B1 carries A2's section too"
        );
        assert_eq!(lrud_of(shots[3]), ft(3.0), "B1 -> B2 carries B1's section");
        assert!(
            warnings.contains(&end_of_chain_line(2)),
            "A3's and B2's sections have no shot: {warnings:?}"
        );
    }

    /// #576 B31: the `.MAK` can override every survey's association (`!O` with
    /// `T` or `t` - Compass Project Manager help, "Parameters": "If the flag is
    /// set to capital "O." all other methods of controlling the LRUD
    /// associations are overridden").
    #[test]
    fn a_mak_lrud_override_beats_the_survey_format() {
        let dir = tempfile::tempdir().unwrap();
        let rows = [
            "A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00",
            "A2 A3 10.00 90.00 0.00 5.00 5.00 5.00 5.00",
        ];
        std::fs::write(
            dir.path().join("from.dat"),
            synthetic_survey_text("A", "DDDDLRUDLADNF", &rows),
        )
        .unwrap();
        let shots_for = |flags: &str| {
            let mak = dir.path().join(format!("p{flags}.mak"));
            std::fs::write(
                &mak,
                format!(
                    "@100.0,200.0,300.0,13,0.5;\r\n&North American 1983;\r\n!{flags};\r\n\
                     #from.dat;\r\n"
                ),
            )
            .unwrap();
            let parsed = parse_compass(&mak).unwrap();
            let (configs, _, _) =
                convert_compass_to_import_configs(&parsed, &CompassImportOptions::default())
                    .unwrap();
            all_shots(&configs)
                .iter()
                .map(|s| lrud_of(s))
                .collect::<Vec<_>>()
        };
        let overridden = shots_for("OT");
        println!("!OT on an F survey: {overridden:?}");
        assert_eq!(
            overridden[0], [None; 4],
            "!OT puts every LRUD at the To station"
        );
        assert_eq!(overridden[1], [Some(0.3048); 4]);
        let not_overridden = shots_for("oT");
        println!("!oT on an F survey: {not_overridden:?}");
        assert_eq!(
            not_overridden[0],
            [Some(0.3048); 4],
            "without O the survey's own F applies"
        );
    }

    /// #576 NF-W2C-1: three surveys share J and E3 - two independent loops, so
    /// exactly two closures. The spanning tree merges Main's J onto Side's J
    /// and Loop's E3 onto Side's E3. Closures were decided per occurrence
    /// against each chain's OWN merge edge, so Side's J (merged onto the
    /// canonical J by Main's edge) got a closure to the point it already is
    /// (zero misclosure), and Side's E3 and Loop's E3 - one merged point - got
    /// one closure each, counting that loop twice in the least-squares solve.
    #[test]
    fn each_loop_gets_one_closure_and_merged_points_none() {
        let text = [
            synthetic_survey_text(
                "Main",
                "DDDDLRUDLADN",
                &[
                    "E1 E2 10.00 90.00 0.00 1 1 1 1",
                    "E2 J 10.00 90.00 0.00 1 1 1 1",
                    "J E3 10.00 90.00 0.00 1 1 1 1",
                ],
            ),
            synthetic_survey_text(
                "Side",
                "DDDDLRUDLADN",
                &[
                    "J S1 10.00 0.00 0.00 1 1 1 1",
                    "S1 S2 10.00 0.00 0.00 1 1 1 1",
                    "S2 E3 14.50 135.00 0.00 1 1 1 1",
                ],
            ),
            synthetic_survey_text(
                "Loop",
                "DDDDLRUDLADN",
                &[
                    "E3 L1 10.00 180.00 0.00 1 1 1 1",
                    "L1 J 14.00 315.00 0.00 1 1 1 1",
                    "J X1 10.00 200.00 0.00 1 1 1 1",
                ],
            ),
        ]
        .concat();
        let mut parsed = parse_dat_text(&text);
        parsed.fixed_stations.push(FixedStation {
            name: "S1".to_string(),
            easting: 500_000.0,
            northing: 2_200_000.0,
            elevation: 0.0,
            datum: Some(Datum::WGS1984),
            utm_zone: Some(16),
        });
        parsed.utm_zone = Some(16);
        parsed.datum = Some(Datum::WGS1984);
        let (configs, closures, _) =
            convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
        for config in &configs {
            println!(
                "section {} {:?}: {:?}",
                config.section_index, config.name, config.placement
            );
        }
        let mut pairs: Vec<String> = closures
            .iter()
            .map(|c| {
                format!(
                    "{}:{}->{}:{}",
                    c.from_section_index,
                    c.from_station_index,
                    c.to_section_index,
                    c.to_station_index
                )
            })
            .collect();
        pairs.sort();
        println!("closures: {pairs:?}");
        assert_eq!(pairs, ["1:3->0:3", "2:2->0:2"]);
    }

    /// #576 B3: a survey that closes a loop on itself (A1 -> A2 -> A3 -> A1)
    /// is one chain whose last station repeats its first. That repeat is the
    /// same physical station, so it needs a closure; the Compass converter
    /// skipped every station of the chain owning the canonical occurrence and
    /// emitted none, so the misclosure was never reported or distributed. The
    /// Walls converter had the same bug, fixed and pinned by
    /// `contiguous_self_loop_emits_closure`.
    #[test]
    fn a_loop_inside_one_survey_gets_a_closure() {
        let (configs, closures, warnings) = convert_text(
            "DDDDLRUDLADN",
            &[
                "A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00",
                "A2 A3 10.00 0.00 0.00 1.00 1.00 1.00 1.00",
                "A3 A1 14.14 225.00 0.00 1.00 1.00 1.00 1.00",
            ],
        );
        println!(
            "sections {}, shots {}, closures {closures:?}, warnings {warnings:?}",
            configs.len(),
            all_shots(&configs).len()
        );
        assert_eq!(configs.len(), 1, "premise: one chain, one section");
        let ties: Vec<(u32, u32, u32, u32)> = closures
            .iter()
            .map(|c| {
                (
                    c.from_section_index,
                    c.from_station_index,
                    c.to_section_index,
                    c.to_station_index,
                )
            })
            .collect();
        assert_eq!(
            ties,
            [(0, 3, 0, 0)],
            "the revisited A1 (station 3) ties to its first occurrence (station 0)"
        );
    }

    /// The `@` base location and `&` datum every `#file` entry needs ahead of
    /// it, then one `#` line per linked file.
    fn synthetic_mak(links: &[&str]) -> String {
        let files: String = links.iter().flat_map(|link| ["#", link, ";\r\n"]).collect();
        format!("@100.0,200.0,300.0,13,0.5;\r\n&North American 1983;\r\n{files}")
    }

    /// Write `mak` as `project.mak` in `dir` and import it.
    fn parse_mak_text(dir: &Path, mak: &[u8]) -> Result<ParsedCompass> {
        let path = dir.join("project.mak");
        std::fs::write(&path, mak)?;
        parse_compass(&path)
    }

    fn survey_names(parsed: &ParsedCompass) -> Vec<&str> {
        parsed
            .surveys
            .iter()
            .map(|s| s.survey.name.as_str())
            .collect()
    }

    /// #576 B35i-3 / B35(i): a `.DAT` opened through its `.MAK` was decoded
    /// byte == code point inside the vendored reader, so the same comment
    /// imported differently from the same file opened directly: 0x92 became
    /// the invisible C1 control U+0092 instead of U+2019, and an undefined
    /// byte was lost without a word. Stalagmight now reads every linked file
    /// itself, like a bare `.DAT`, and names the file in the warning.
    #[test]
    fn a_linked_dat_decodes_as_windows_1252_and_names_itself() {
        let dir = tempfile::tempdir().unwrap();
        let record = synthetic_survey_record("Synthetic Cave", "A", "7 10 1979");
        let mut bytes = record.trim_end().as_bytes().to_vec();
        bytes.extend_from_slice(b" C\x81ve don\x92t\r\n");
        std::fs::write(dir.path().join("cave.dat"), &bytes).unwrap();

        let parsed = parse_mak_text(dir.path(), synthetic_mak(&["cave.dat"]).as_bytes()).unwrap();
        let comment = parsed.surveys[0].survey.shots[0]
            .comment
            .clone()
            .unwrap_or_default();
        let code_points: Vec<String> = comment
            .chars()
            .map(|c| format!("U+{:04X}", u32::from(c)))
            .collect();
        println!(
            "linked .DAT comment: {comment:?} = {}",
            code_points.join(" ")
        );
        println!("warnings: {:?}", parsed.warnings);
        assert_eq!(
            comment,
            "C\u{fffd}ve don\u{2019}t",
            "{}",
            code_points.join(" ")
        );
        assert!(
            parsed.warnings.iter().any(|w| w
                == "'cave.dat' was read as windows-1252, the encoding Compass writes; 1 \
                    character was replaced"),
            "{:?}",
            parsed.warnings
        );
    }

    /// #576 B35i-3: a byte-order mark on a linked `.DAT` landed in its cave
    /// name (and so in the imported project's name).
    #[test]
    fn a_linked_dat_with_a_utf8_bom_keeps_its_cave_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(synthetic_dat("Synthetic Cave").as_bytes());
        std::fs::write(dir.path().join("cave.dat"), &bytes).unwrap();
        let parsed = parse_mak_text(dir.path(), synthetic_mak(&["cave.dat"]).as_bytes()).unwrap();
        println!("cave name: {:?}", parsed.cave_name);
        assert_eq!(parsed.cave_name, "Synthetic Cave");
    }

    /// #576 NF-AN4-2: the `.MAK` itself was read as strict UTF-8, so one
    /// Windows-1252 byte anywhere in it - an accented word in a comment, a file
    /// name - refused the whole project.
    #[test]
    fn a_mak_with_a_windows_1252_byte_opens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cave.dat"), synthetic_dat("Synthetic Cave")).unwrap();
        let mut mak = b"/ Caf\xe9 survey notes\r\n".to_vec();
        mak.extend_from_slice(synthetic_mak(&["cave.dat"]).as_bytes());
        let result = parse_mak_text(dir.path(), &mak);
        println!(
            "MAK with 0xE9 in a comment: {:?}",
            result
                .as_ref()
                .map(|p| survey_names(p).join(","))
                .map_err(ToString::to_string)
        );
        assert_eq!(survey_names(&result.unwrap()), ["A"]);
    }

    /// #576 B35i-3: a "UTF-8 with BOM" `.MAK` used to be refused by name; now
    /// that Stalagmight reads the file itself, the mark is simply dropped, as
    /// it is for every other text format.
    #[test]
    fn a_utf8_bom_on_a_mak_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s.dat"), synthetic_dat("Synthetic Cave")).unwrap();
        let mut marked = vec![0xEF, 0xBB, 0xBF];
        marked.extend_from_slice(synthetic_mak(&["s.dat"]).as_bytes());
        let parsed = parse_mak_text(dir.path(), &marked).unwrap();
        assert_eq!(parsed.cave_name, "Synthetic Cave");
    }

    /// #576 B8 (decision B8-D2): one missing `.DAT` used to fail the whole
    /// project. It is skipped and named; the rest imports.
    #[test]
    fn a_missing_linked_dat_is_skipped_and_named() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("here.dat"), synthetic_dat("Synthetic Cave")).unwrap();
        let result = parse_mak_text(
            dir.path(),
            synthetic_mak(&["here.dat", "absent.dat"]).as_bytes(),
        );
        println!(
            "MAK linking here.dat + absent.dat: {:?}",
            result
                .as_ref()
                .map(|p| (survey_names(p).join(","), p.warnings.clone()))
                .map_err(ToString::to_string)
        );
        let parsed = result.unwrap();
        assert_eq!(survey_names(&parsed), ["A"], "here.dat still imports");
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.starts_with("'absent.dat' was not found") && w.contains("skipped")),
            "{:?}",
            parsed.warnings
        );
    }

    /// #576 B8: with no readable file at all there is nothing to import, so
    /// the project is refused - naming every file and why.
    #[test]
    fn a_mak_whose_every_link_fails_is_refused_with_the_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let err = parse_mak_text(dir.path(), synthetic_mak(&["a.dat", "b.dat"]).as_bytes())
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        println!("every link missing: {err}");
        assert!(
            err.contains("'a.dat' was not found") && err.contains("'b.dat' was not found"),
            "{err}"
        );
    }

    /// #576 B8 (decision B8-D1, the Walls rules): a relative link resolves
    /// inside the `.MAK`'s folder, with `..` clamped there. A link climbing out
    /// of the folder used to read whatever it named.
    #[test]
    fn a_dotdot_link_cannot_leave_the_mak_folder() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        // Outside the project folder: must not be reachable through `..`.
        std::fs::write(
            dir.path().join("outside.dat"),
            synthetic_survey_record("Synthetic Cave", "OUT", "7 10 1979"),
        )
        .unwrap();
        std::fs::write(
            project.join("inner.dat"),
            synthetic_survey_record("Synthetic Cave", "IN", "7 10 1979"),
        )
        .unwrap();
        let result = parse_mak_text(
            &project,
            synthetic_mak(&["../outside.dat", "inner.dat"]).as_bytes(),
        );
        println!(
            "links ../outside.dat + inner.dat: {:?}",
            result
                .as_ref()
                .map(|p| (survey_names(p).join(","), p.warnings.clone()))
                .map_err(ToString::to_string)
        );
        let parsed = result.unwrap();
        assert_eq!(
            survey_names(&parsed),
            ["IN"],
            "the .. link resolves inside the folder, where there is no such file"
        );
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.starts_with("'../outside.dat' was not found")),
            "{:?}",
            parsed.warnings
        );
    }

    /// #576 B8: Compass is a Windows program and writes a backslash between
    /// folders; on macOS and Linux that was read as part of the file name.
    #[test]
    fn a_backslash_link_resolves_on_every_platform() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(
            dir.path().join("sub").join("inner.dat"),
            synthetic_survey_record("Synthetic Cave", "IN", "7 10 1979"),
        )
        .unwrap();
        let link = "sub\\inner.dat";
        let result = parse_mak_text(dir.path(), synthetic_mak(&[link]).as_bytes());
        println!(
            "link {link}: {:?}",
            result
                .as_ref()
                .map(|p| survey_names(p).join(","))
                .map_err(ToString::to_string)
        );
        assert_eq!(survey_names(&result.unwrap()), ["IN"]);
    }

    /// #576 B8: an absolute local path is honoured, as the Walls importer
    /// honours an absolute `.PATH`, but said out loud.
    #[test]
    fn an_absolute_link_outside_the_folder_is_read_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let target = elsewhere.join("far.dat");
        std::fs::write(&target, synthetic_dat("Synthetic Cave")).unwrap();
        let link = target.display().to_string();
        let parsed = parse_mak_text(&project, synthetic_mak(&[&link]).as_bytes()).unwrap();
        println!("absolute link warnings: {:?}", parsed.warnings);
        assert_eq!(survey_names(&parsed), ["A"]);
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.contains(&link) && w.contains("outside the .MAK's folder")),
            "{:?}",
            parsed.warnings
        );
    }

    /// #576 B8: a network (UNC) or device-namespace path is refused without
    /// being touched: on Windows even checking whether `\\server\share\x`
    /// exists hands the user's credentials to that server.
    #[test]
    fn network_and_device_links_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("here.dat"), synthetic_dat("Synthetic Cave")).unwrap();
        let refused = [
            "\\\\server\\share\\cave.dat",
            "//server/share/cave.dat",
            "\\\\?\\C:\\caves\\cave.dat",
            "\\\\.\\COM1",
        ];
        let mut links = vec!["here.dat"];
        links.extend(refused);
        let result = parse_mak_text(dir.path(), synthetic_mak(&links).as_bytes());
        println!(
            "network/device links: {:?}",
            result
                .as_ref()
                .map(|p| (survey_names(p).join(","), p.warnings.clone()))
                .map_err(ToString::to_string)
        );
        let parsed = result.unwrap();
        assert_eq!(survey_names(&parsed), ["A"]);
        for link in refused {
            assert!(
                parsed
                    .warnings
                    .iter()
                    .any(|w| w.starts_with(&format!("'{link}' was skipped"))
                        && w.contains("network")),
                "{link}: {:?}",
                parsed.warnings
            );
        }
    }

    /// #576 RV2-R-9 / RV3-R-10: an absolute link that climbs out of the
    /// `.MAK`'s folder with `..` is still outside it. The check compared the
    /// link as written (`<folder>/../outside.dat` "starts with" `<folder>`), so
    /// the file was read with no warning.
    #[test]
    fn an_absolute_link_climbing_out_with_dotdot_is_read_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            dir.path().join("outside.dat"),
            synthetic_survey_record("Synthetic Cave", "OUT", "7 10 1979"),
        )
        .unwrap();
        let link = format!("{}/../outside.dat", project.display());
        let parsed = parse_mak_text(&project, synthetic_mak(&[&link]).as_bytes()).unwrap();
        println!(
            "link {link}: surveys {:?}, warnings {:?}",
            survey_names(&parsed),
            parsed.warnings
        );
        assert_eq!(survey_names(&parsed), ["OUT"], "absolute links are read");
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.contains(&link) && w.contains("outside the .MAK's folder")),
            "{:?}",
            parsed.warnings
        );
    }

    /// #576 RV2-R-9: a link inside the folder that is a symlink to a file
    /// outside it is read from outside, so it is said out loud too.
    #[cfg(unix)]
    #[test]
    fn a_symlink_in_the_folder_pointing_out_is_read_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let outside = dir.path().join("outside.dat");
        std::fs::write(
            &outside,
            synthetic_survey_record("Synthetic Cave", "OUT", "7 10 1979"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, project.join("link.dat")).unwrap();
        let parsed = parse_mak_text(&project, synthetic_mak(&["link.dat"]).as_bytes()).unwrap();
        println!(
            "symlinked link: surveys {:?}, warnings {:?}",
            survey_names(&parsed),
            parsed.warnings
        );
        assert_eq!(survey_names(&parsed), ["OUT"]);
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.starts_with("'link.dat' is outside the .MAK's folder")),
            "{:?}",
            parsed.warnings
        );
    }

    /// #576 RV2-R-10: a Windows DOS device name - with or without an
    /// extension, in any case - is refused from the link's text, before
    /// anything asks whether it exists (on Windows that opens the device).
    /// Files of those names exist here, so only the text can refuse them; a
    /// name that merely starts like one is an ordinary file.
    #[test]
    fn dos_device_names_are_refused_from_the_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        let devices = [
            "NUL.dat",
            "con",
            "Com1.txt",
            "sub\\LPT9.dat",
            "aux .dat",
            "PRN.",
        ];
        for (i, name) in devices.iter().enumerate() {
            std::fs::write(
                dir.path().join(name.replace('\\', "/")),
                synthetic_survey_record("Synthetic Cave", &format!("D{i}"), "7 10 1979"),
            )
            .unwrap();
        }
        std::fs::write(
            dir.path().join("COM10.dat"),
            synthetic_survey_record("Synthetic Cave", "C10", "7 10 1979"),
        )
        .unwrap();
        let mut links = vec!["COM10.dat"];
        links.extend(devices);
        let parsed = parse_mak_text(dir.path(), synthetic_mak(&links).as_bytes()).unwrap();
        println!(
            "device-name links: surveys {:?}, warnings {:?}",
            survey_names(&parsed),
            parsed.warnings
        );
        assert_eq!(survey_names(&parsed), ["C10"]);
        for link in devices {
            assert!(
                parsed.warnings.iter().any(
                    |w| w.starts_with(&format!("'{link}' was skipped")) && w.contains("device")
                ),
                "{link}: {:?}",
                parsed.warnings
            );
        }
    }

    /// #576 RV2-R-12: a `.MAK` listing the same `.DAT` again (under any
    /// spelling) read and held it once per listing, importing its surveys
    /// twice. It is read once; each further listing is skipped with a warning.
    #[test]
    fn a_file_linked_twice_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.dat"), synthetic_dat("Synthetic Cave")).unwrap();
        let parsed = parse_mak_text(
            dir.path(),
            synthetic_mak(&["a.dat", "a.dat", "./sub/../a.dat"]).as_bytes(),
        )
        .unwrap();
        println!(
            "a.dat linked three times: surveys {:?}, warnings {:?}",
            survey_names(&parsed),
            parsed.warnings
        );
        assert_eq!(survey_names(&parsed), ["A"]);
        assert_eq!(
            parsed
                .warnings
                .iter()
                .filter(|w| w.contains("is listed more than once"))
                .count(),
            2,
            "{:?}",
            parsed.warnings
        );
    }

    /// #576 RV2-R-12: the files one `.MAK` links are capped in TOTAL, not only
    /// one by one. A link that would take the project past the limit is
    /// skipped and named; the ones before it are read. (The real limit is
    /// 512 MB; the budget here is two files' worth.)
    #[test]
    fn the_linked_files_share_one_byte_budget() {
        let dir = tempfile::tempdir().unwrap();
        let mut size = 0;
        for name in ["a.dat", "b.dat", "c.dat"] {
            let record = synthetic_survey_record("Synthetic Cave", name, "7 10 1979");
            size = record.len() as u64;
            std::fs::write(dir.path().join(name), record).unwrap();
        }
        assert_eq!(MAX_LINKED_BYTES, 512 * 1024 * 1024);
        let mut budget = LinkBudget::new(2 * size + 1);
        let mut warnings = Vec::new();
        let mut skipped = Vec::new();
        let read: Vec<bool> = ["a.dat", "b.dat", "c.dat"]
            .iter()
            .map(|link| {
                read_linked_dat(
                    dir.path(),
                    Path::new(link),
                    &mut budget,
                    &mut warnings,
                    &mut skipped,
                )
                .is_some()
            })
            .collect();
        println!(
            "read {read:?} with a {} byte budget: {warnings:?}",
            2 * size + 1
        );
        assert_eq!(read, [true, true, false]);
        assert!(
            skipped.len() == 1
                && skipped[0].starts_with("'c.dat' was skipped")
                && skipped[0].contains("limit for one project"),
            "{skipped:?}"
        );
    }

    /// A named pipe at `path`, made with the system's `mkfifo`.
    #[cfg(unix)]
    fn make_fifo(path: &Path) {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "premise: mkfifo made {}", path.display());
    }

    /// Run `f` on its own thread and give it five seconds. Reading a FIFO
    /// nobody writes blocks forever, so on a timeout the FIFO at `release` is
    /// opened for writing and closed again - the reader then sees an empty file
    /// and the thread can finish - and the result is `None`.
    #[cfg(unix)]
    fn within_five_seconds<T: Send + 'static>(
        release: &Path,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = sender.send(f());
        });
        let started = std::time::Instant::now();
        let outcome = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .ok();
        println!(
            "  returned after {:.1} s{}",
            started.elapsed().as_secs_f64(),
            if outcome.is_some() {
                ""
            } else {
                " (still blocked; released by the test)"
            }
        );
        if outcome.is_none() {
            drop(std::fs::OpenOptions::new().write(true).open(release));
        }
        let _ = worker.join();
        outcome
    }

    /// #576 B27(g): a path that is not a regular file - here a FIFO named like
    /// a `.DAT` - blocked the import forever, and imports run on the app's
    /// main thread. It is refused at once.
    #[cfg(unix)]
    #[test]
    fn a_fifo_named_like_a_dat_is_refused_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.dat");
        make_fifo(&fifo);
        let path = fifo.clone();
        let outcome = within_five_seconds(&fifo, move || {
            parse_compass(&path)
                .map(|p| p.surveys.len())
                .map_err(|e| e.to_string())
        });
        println!("parse_compass(pipe.dat): {outcome:?}");
        let err = outcome
            .expect("the read must not block on a FIFO")
            .expect_err("a FIFO is not a survey file");
        assert!(err.contains("not a regular file"), "{err}");
    }

    /// #576 B27(g): the same for a FIFO a `.MAK` links - it is skipped and
    /// named, and the rest of the project imports.
    #[cfg(unix)]
    #[test]
    fn a_mak_linking_a_fifo_skips_it_at_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("here.dat"), synthetic_dat("Synthetic Cave")).unwrap();
        let fifo = dir.path().join("pipe.dat");
        make_fifo(&fifo);
        let mak = dir.path().join("project.mak");
        std::fs::write(&mak, synthetic_mak(&["here.dat", "pipe.dat"])).unwrap();
        let outcome = within_five_seconds(&fifo, move || {
            parse_compass(&mak)
                .map(|p| (p.surveys.len(), p.warnings))
                .map_err(|e| e.to_string())
        });
        println!("parse_compass(project.mak): {outcome:?}");
        let (surveys, warnings) = outcome
            .expect("the read must not block on a FIFO")
            .expect("here.dat still imports");
        assert_eq!(surveys, 1);
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with("'pipe.dat' could not be read")
                    && w.contains("not a regular file")),
            "{warnings:?}"
        );
    }

    /// #576 B27(g): survey text files (`.DAT`, `.MAK` and every `.DAT` it
    /// links, `.SRV`, `.WPJ`) are read up to 128 MB; a larger one is refused
    /// before it is read, naming the limit. The file here is sparse, so it
    /// costs no disk.
    #[test]
    fn a_dat_over_the_survey_text_limit_is_refused_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.dat");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(128 * 1024 * 1024 + 1).unwrap();
        drop(file);
        let result = parse_compass(&path).map(|p| p.surveys.len());
        let shown = result.as_ref().map_err(ToString::to_string);
        println!("parse_compass(128 MB + 1 byte .dat): {shown:?}");
        let err = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            err.contains("128 MB"),
            "the refusal names the limit: {err:?}"
        );
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod graph_tests;

// #576 RV2-R-NITS: To-station cross-sections superseded at a station.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod lrud_tests;

// #576 FZ-PARSE-7: any negative LRUD reads as missing.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod negative_lrud_tests;

// #576 FZ-PARSE-5: a length correction beyond the limit reads as 0.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod reading_bound_tests;

// #576 RVF-d-2: a length has no missing marker; a dropped leg is on the preview.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod length_tests;
