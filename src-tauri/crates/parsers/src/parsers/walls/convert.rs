use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;
use chrono::{Datelike, NaiveDate};

use crate::models::azimuth_reference::{AzimuthReference, CorrectionSource, NorthReference};
use crate::models::section::{ImportClosure, ImportPlacement, ImportSectionConfig};
use crate::models::shot::{ShotData, ShotKind};
use crate::models::station::GpsCoordinates;

use super::{
    ParsedSurvey, ParsedWalls, WallsFile, WallsImportOptions, WallsRef, WallsSurveySummary,
    WallsVector, SECTION_COLORS,
};

// ---------------------------------------------------------------------------
// Coordinate projection
// ---------------------------------------------------------------------------

pub(crate) fn utm_to_wgs84(
    easting: f64,
    northing: f64,
    zone: i32,
    datum: &str,
) -> Option<(f64, f64)> {
    let token = datum_proj_token(datum);
    // Walls's `.REF` encodes a negative zone for the southern hemisphere.
    // `project_utm` returns (lon, lat); this importer carries (lat, lon).
    let (lon, lat) =
        crate::parsers::geo::project_utm(easting, northing, zone.unsigned_abs(), zone < 0, &token)?;
    Some((lat, lon))
}

/// Geodetic family of a Walls datum name that this importer can project with a
/// dedicated PROJ token. Walls stores datum *display* names (the first column
/// of its 28-entry datum table - e.g. `WGS 1984`, `NAD83`, `NAD27 CONUS`); the
/// long NIMA names (`North American 1983`/`1927`) also appear in the wild. Any
/// datum outside these four families is an obscure regional datum that PROJ's
/// simple `datum=`/`ellps=` tokens don't cover, so it is approximated as WGS84.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DatumFamily {
    Wgs84,
    Wgs72,
    Nad83,
    Nad27,
    /// Unrecognized - projected as WGS84 and may be off by tens of meters.
    OtherApproxWgs84,
}

/// Classify a Walls `.REF`/`#Fix` datum name. Whitespace is stripped and case
/// folded so `WGS 1984` == `WGS1984` and `North American 1983` matches, then
/// the canonical names plus their common abbreviations are recognized. This is
/// the single source of truth for both [`datum_proj_token`] and
/// [`datum_recognized`], so the projection used can never disagree with whether
/// we consider the datum "recognized".
pub(crate) fn datum_family(datum: &str) -> DatumFamily {
    let key = datum
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if key.contains("NAD83") || key.contains("NORTHAMERICAN1983") {
        DatumFamily::Nad83
    } else if key.contains("NAD27") || key.contains("NORTHAMERICAN1927") {
        DatumFamily::Nad27
    } else if key.contains("WGS1984") || key.contains("WGS84") {
        DatumFamily::Wgs84
    } else if key.contains("WGS1972") || key.contains("WGS72") {
        DatumFamily::Wgs72
    } else {
        DatumFamily::OtherApproxWgs84
    }
}

pub(crate) fn datum_proj_token(datum: &str) -> String {
    match datum_family(datum) {
        DatumFamily::Nad83 => "datum=NAD83",
        DatumFamily::Nad27 => "datum=NAD27",
        DatumFamily::Wgs72 => "ellps=WGS72",
        DatumFamily::Wgs84 | DatumFamily::OtherApproxWgs84 => "datum=WGS84",
    }
    .to_string()
}

/// Whether [`datum_proj_token`] maps this datum to a specific projection rather
/// than the WGS84 fallback. Used to warn when an unusual datum is silently
/// approximated as WGS84.
pub(crate) fn datum_recognized(datum: &str) -> bool {
    datum_family(datum) != DatumFamily::OtherApproxWgs84
}

// ---------------------------------------------------------------------------
// Preview
// ---------------------------------------------------------------------------

/// Whether a parsed vector becomes an imported shot. This is the single source
/// of truth for the keep/skip filter in `convert_walls_to_import_configs` and the
/// shot counts in `build_preview`, so the preview can never overstate the import.
/// `include_splays` controls whether anonymous wall-point (`-`) splay legs count.
pub(crate) fn is_importable_shot(v: &WallsVector, include_splays: bool) -> bool {
    if v.is_reference && !include_splays {
        return false;
    }
    // Dive legs are valid with a real depth change even at zero tape; a
    // zero-length non-dive leg that still carries a bearing is a coincident-
    // station tie worth keeping; every other zero-length leg is a degenerate
    // equate and is dropped.
    let depth_delta = match (v.from_depth, v.to_depth) {
        (Some(fd), Some(td)) => (td - fd).abs(),
        _ => 0.0,
    };
    let bearing_tie = !v.is_depth_gauge && v.azimuth_in.is_some();
    !(v.length_m <= 0.0 && !(v.is_depth_gauge && depth_delta > 0.0) && !bearing_tie)
}

/// Build the import-dialog preview from parsed data.
/// Survey count and whether any survey used feet, without building the full
/// [`WallsFile`] preview. `execute_walls_import` needs only these two
/// scalars to fill in the receipt (#393 item 1); the full preview also runs
/// `resolve_anchors` and builds a `WallsSurveySummary` per survey, work an
/// import that already parsed and converted the file has no use for.
pub fn preview_scalars(parsed: &ParsedWalls) -> (u32, bool) {
    (
        parsed.surveys.len() as u32,
        parsed.surveys.iter().any(|s| s.used_feet),
    )
}

pub fn build_preview(parsed: &ParsedWalls) -> WallsFile {
    // Count shots the import would ACTUALLY produce under the default options
    // (splays excluded), not the raw vector count - otherwise the preview
    // overstates a file of splays/equates that converts to nothing.
    let shot_count = parsed
        .surveys
        .iter()
        .flat_map(|s| s.vectors.iter())
        .filter(|v| is_importable_shot(v, false))
        .count() as u32;
    // Splay/anonymous-wall shots not counted above but importable once the user
    // enables "Include splays" - so a splay-only file isn't reported as empty.
    let splay_count = parsed
        .surveys
        .iter()
        .flat_map(|s| s.vectors.iter())
        .filter(|v| is_importable_shot(v, true) && !is_importable_shot(v, false))
        .count() as u32;
    let resolved_survey_count = parsed.surveys.iter().filter(|s| s.resolved).count() as u32;
    // DISTINCT geographic `#Fix` stations the import would actually anchor on.
    // Reuse `resolve_anchors` (the importer's own logic) as the single source of
    // truth so the preview count can never drift from the anchors created.
    let fixed_station_count =
        resolve_anchors(parsed, &WallsImportOptions::default(), &mut Vec::new()).len() as u32;
    let surveys = parsed
        .surveys
        .iter()
        .map(|s| WallsSurveySummary {
            name: s.name.clone(),
            title: s.title.clone(),
            date: s.date.map(|d| d.to_string()).unwrap_or_default(),
            segment: s.segment.clone(),
            shot_count: s
                .vectors
                .iter()
                .filter(|v| is_importable_shot(v, false))
                .count() as u32,
            splay_count: s
                .vectors
                .iter()
                .filter(|v| is_importable_shot(v, true) && !is_importable_shot(v, false))
                .count() as u32,
            fixed_station_count: survey_fixed_station_count(s),
            resolved: s.resolved,
        })
        .collect();
    WallsFile {
        project_name: parsed.project_name.clone(),
        book_count: parsed.book_count,
        survey_count: parsed.surveys.len() as u32,
        resolved_survey_count,
        shot_count,
        splay_count,
        has_georeference: parsed.georef.is_some(),
        datum: parsed.georef.as_ref().map(|r| r.datum.clone()),
        utm_zone: parsed.georef.as_ref().map(|r| r.zone),
        fixed_station_count,
        missing_file_count: parsed.missing_files.len() as u32,
        skipped_attachment_count: parsed.skipped_attachments,
        any_feet_units: parsed.surveys.iter().any(|s| s.used_feet),
        surveys,
        warnings: parsed.warnings.clone(),
    }
}

// ---------------------------------------------------------------------------
// Conversion to import configs
// ---------------------------------------------------------------------------

pub(crate) struct AnchorGeo {
    pub(crate) station: String,
    pub(crate) coordinates: GpsCoordinates,
    pub(crate) depth: f64,
}

/// Convert parsed Walls data into Stalagmight import configs + loop closures.
pub fn convert_walls_to_import_configs(
    parsed: &ParsedWalls,
    opts: &WallsImportOptions,
) -> Result<(Vec<ImportSectionConfig>, Vec<ImportClosure>, Vec<String>)> {
    let mut warnings = parsed.warnings.clone();

    // Every `#Fix` with a geographic position is a candidate GPS anchor. Each
    // connected survey component gets at most ONE of them as a zero datum (the
    // model forbids multiple datums per merge component); extra fixes in the
    // same component are tied in by the normal merges + loop closures.
    let anchors = resolve_anchors(parsed, opts, &mut warnings);

    // Pass 1: finalize per-survey vectors, collect graph edges + known depths.
    struct KeptSurvey {
        label: String,
        surveyor: Option<String>,
        date: Option<NaiveDate>,
        vectors: Vec<KeptVector>,
    }
    struct KeptVector {
        from: String,
        to: String,
        length_m: f64,
        azimuth_in: f64,
        azimuth_out: f64,
        pitch_in: Option<f64>,
        pitch_out: Option<f64>,
        left: Option<f64>,
        right: Option<f64>,
        up: Option<f64>,
        down: Option<f64>,
        comment: Option<String>,
        is_reference: bool,
        lrud_is_to: bool,
        /// Degrees to ADD to this vector's stored (measured) azimuth to reach
        /// TRUE north, which is the frame geometry is drawn in: the declination
        /// ALONE (#438). A grid convergence is reported rather than folded in -
        /// see where this is computed for why.
        /// Kept per VECTOR because Walls declares both per entry:
        /// `Decl=`/`Grid=` can change mid-survey and a RECT vector takes no
        /// declination at all.
        azimuth_correction: f64,
        correction_source: CorrectionSource,
        /// What this vector's stored reading IS: a compass reading, or the
        /// already-true-north bearing a RECT vector's E/N components imply.
        north: NorthReference,
    }

    let mut kept: Vec<KeptSurvey> = Vec::new();
    let mut edges: Vec<(String, String, f64)> = Vec::new();
    let mut known_depth: HashMap<String, f64> = HashMap::new();

    for (si, survey) in parsed.surveys.iter().enumerate() {
        // Skip surveys the user deselected. `si` stays the ORIGINAL index so
        // synthetic splay names remain stable and match `resolve_anchors`.
        if !opts.includes_survey(si) {
            continue;
        }
        if survey.vectors.is_empty() {
            continue;
        }
        // Use THIS survey's inherited `.REF` (nearest ancestor) for grid
        // convergence + auto-declination, not the project's first reference.
        let survey_georef = &survey.georef;
        let mut kept_vectors = Vec::new();
        for (vi, v) in survey.vectors.iter().enumerate() {
            if !is_importable_shot(v, opts.include_splays) {
                continue;
            }
            // Give anonymous wall endpoints globally-unique names so unrelated
            // splays never collapse onto one shared station in the graph.
            let from = if v.from_anon {
                format!("\u{1}anon:{si}:{vi}:f")
            } else {
                v.from.clone()
            };
            let to = if v.to_anon {
                format!("\u{1}anon:{si}:{vi}:t")
            } else {
                v.to.clone()
            };
            // The grid convergence, computed ONLY so it can be reported below:
            // the `.REF` convergence (when grid-relative) plus any explicit
            // `#Units Grid=` the survey set. Both rotate true north to grid
            // north, which is exactly why they are not applied here - see the
            // correction below.
            let convergence = if v.grid_relative {
                survey_georef.as_ref().map(|r| r.convergence).unwrap_or(0.0)
            } else {
                0.0
            } + v.grid_convergence;
            // RECT vectors already carry an absolute grid azimuth; magnetic
            // declination must NOT rotate Cartesian E/N/U deltas (only
            // convergence, via grid_relative, may apply).
            let (declination, correction_source) = if v.is_rect {
                (0.0, CorrectionSource::FileDeclared)
            } else {
                resolve_declination(v, survey_georef, &mut warnings)
            };
            // #438: the stored azimuth is the MEASURED one, and the correction
            // recorded beside it is what reaches TRUE north - the declination
            // alone.
            //
            // The grid convergence is deliberately NOT part of it. `correction`
            // means "add this to reach true north", and the app's world frame is
            // true north: `gps_to_local` returns true-north ENU
            // (`models/geo.rs:79-81`) and every georeferencing path places datums
            // with it. Walls subtracts convergence because its own output is
            // grid-relative; drawing in that frame here would put the survey at
            // an angle to its own GPS. A RECT vector's bearing is already a
            // true-north one in the file's Cartesian frame, so it needs nothing.
            let azimuth_correction = declination;
            let north = if v.is_rect {
                NorthReference::True
            } else {
                NorthReference::Magnetic
            };
            if convergence.abs() > f64::EPSILON {
                warnings.push(format!(
                    "Shot {}->{}: the file applies a {convergence:.2} degree grid convergence. \
                     Stalagmight draws in true north, so the convergence was NOT applied - only \
                     the declination.",
                    v.from, v.to
                ));
            }
            let measured = |a: Option<f64>| a.map(|x| x.rem_euclid(360.0)).unwrap_or(0.0);
            let azimuth_in = measured(v.azimuth_in);
            let azimuth_out = measured(v.azimuth_out);

            // The model stores azimuth as a plain f64, so an omitted/unreadable
            // bearing collapses to 0° (due north). That is correct for a plumb
            // shot but silently wrong for anything else - surface it.
            if v.azimuth_in.is_none() && !v.is_depth_gauge {
                let plumb = v.inc_deg.map(|i| i.abs() >= 89.5).unwrap_or(false);
                if !plumb {
                    warnings.push(format!(
                        "Shot {}->{}: missing or unreadable azimuth; recorded as 0° (due north).",
                        v.from, v.to
                    ));
                }
            }

            // Edge elevation gain (+up).
            let gain = if v.is_depth_gauge {
                match (v.from_depth, v.to_depth) {
                    (Some(fd), Some(td)) => fd - td,
                    // Exactly one depth column was supplied (the other was `--` or
                    // absent). The leg's vertical change is unknown, so it is
                    // integrated as flat and the unmeasured station inherits its
                    // neighbour's depth - surface that rather than asserting a
                    // zero gain silently. (A genuine depth-gauge reading for that
                    // station from another leg still overrides the estimate.)
                    (Some(_), None) | (None, Some(_)) => {
                        warnings.push(format!(
                            "Shot {}->{}: dive leg is missing a depth reading; its \
                             vertical change was treated as zero.",
                            v.from, v.to
                        ));
                        0.0
                    }
                    (None, None) => 0.0,
                }
            } else {
                let inc = v.inc_deg.unwrap_or(0.0);
                let mut g = v.length_m * inc.to_radians().sin();
                // IncH adjusts the vertical of non-plumb CT shots. Require a real
                // measured distance: a zero-length coincident-station bearing tie
                // must not pick up a spurious IncH vertical offset.
                if inc.abs() < 89.5 && v.length_m > 0.0 {
                    g += v.inch;
                }
                g
            };
            edges.push((from.clone(), to.clone(), gain));

            if v.is_depth_gauge {
                if let Some(fd) = v.from_depth {
                    known_depth.entry(from.clone()).or_insert(fd);
                }
                if let Some(td) = v.to_depth {
                    known_depth.entry(to.clone()).or_insert(td);
                }
            }

            kept_vectors.push(KeptVector {
                from,
                to,
                length_m: v.length_m,
                azimuth_in,
                azimuth_out,
                pitch_in: v.pitch_in,
                pitch_out: v.pitch_out,
                left: v.left,
                right: v.right,
                up: v.up,
                down: v.down,
                comment: v.comment.clone(),
                is_reference: v.is_reference,
                lrud_is_to: v.lrud_is_to,
                azimuth_correction,
                correction_source,
                north,
            });
        }
        if kept_vectors.is_empty() {
            continue;
        }
        kept.push(KeptSurvey {
            label: survey_label(survey),
            surveyor: survey.surveyor.clone(),
            date: survey.date,
            vectors: kept_vectors,
        });
    }

    // Resolve absolute station depths. Fixed stations seed the depth of their
    // component only where there is no depth-gauge reading to defer to.
    let anchor_depths: HashMap<&str, f64> = anchors
        .iter()
        .map(|a| (a.station.as_str(), a.depth))
        .collect();
    let depths = build_station_depths(&edges, &known_depth, &anchor_depths, &mut warnings);
    let depth_of = |s: &str| depths.get(s).copied().unwrap_or(0.0);

    // Pass 2a: split each survey into head-to-tail chains -> sections.
    struct PendingChain {
        section_index: u32,
        names: Vec<String>,
        shots: Vec<ShotData>,
        station_comments: Vec<(u32, String)>,
        label: String,
        surveyor: Option<String>,
        date: Option<NaiveDate>,
        azimuth_reference: AzimuthReference,
    }
    // The model has no zero-length shot (it can't place two stations at one
    // point as a leg), so a Walls zero-length coincident-station tie is nudged to
    // a negligible length and counted here for a single aggregated warning.
    const COINCIDENT_TIE_EPSILON_M: f64 = 0.001;
    let mut coincident_ties = 0usize;
    let mut chains: Vec<PendingChain> = Vec::new();
    for survey in &kept {
        let split = split_into_chains(&survey.vectors, |v| (v.from.as_str(), v.to.as_str()));
        let multi = split.len() > 1;
        for (chain_idx, chain) in split.iter().enumerate() {
            if chain.is_empty() {
                continue;
            }
            let section_index = chains.len() as u32;
            let mut names = Vec::with_capacity(chain.len() + 1);
            names.push(survey.vectors[chain[0]].from.clone());
            for &idx in chain {
                names.push(survey.vectors[idx].to.clone());
            }
            let mut shots = Vec::with_capacity(chain.len());
            for (pos, &idx) in chain.iter().enumerate() {
                let v = &survey.vectors[idx];
                let depth_in = depth_of(&v.from);
                let depth_out = depth_of(&v.to);
                // Slope length must be >= |Δdepth| or the model can't form the
                // triangle; clamp up (with a warning if the gap is material).
                let dz = (depth_out - depth_in).abs();
                let length = if v.length_m < dz {
                    if dz - v.length_m > 0.05 {
                        warnings.push(format!(
                            "Shot {}->{}: slope length {:.2} m < depth change {:.2} m; clamped",
                            v.from, v.to, v.length_m, dz
                        ));
                    }
                    dz
                } else {
                    v.length_m
                };
                // A zero-length leg that survived the keep filter is a
                // coincident-station tie (e.g. `A B 0 0 0` equating two names, or
                // a bearing with no tape). The model rejects length == 0, so nudge
                // it to a negligible length: the two stations stay connected and
                // effectively coincident instead of failing the whole import.
                let length = if length <= 0.0 {
                    coincident_ties += 1;
                    COINCIDENT_TIE_EPSILON_M
                } else {
                    length
                };
                // Resolve this shot's FROM-station LRUD honoring `#Units LRUD`. A
                // From-typed leg contributes its own LRUD; a To-typed leg's LRUD
                // belongs to its TO station - i.e. the FROM of the NEXT shot - so
                // it is adopted from the previous leg in the chain. (A To survey
                // thus shifts each cross-section one shot forward; the final To
                // station's LRUD has no outgoing shot and is dropped.)
                let own_lrud = if v.lrud_is_to {
                    (None, None, None, None)
                } else {
                    (v.left, v.right, v.up, v.down)
                };
                let (left, right, up, down) = match pos
                    .checked_sub(1)
                    .map(|p| &survey.vectors[chain[p]])
                {
                    Some(prev) if prev.lrud_is_to => (prev.left, prev.right, prev.up, prev.down),
                    _ => own_lrud,
                };
                shots.push(ShotData {
                    credit: None,
                    // Walls files carry no shot identity; every shot gets a
                    // fresh mint.
                    uuid: None,
                    azimuth_in: v.azimuth_in,
                    azimuth_out: v.azimuth_out,
                    length,
                    depth_in,
                    depth_out,
                    pitch_in: v.pitch_in,
                    pitch_out: v.pitch_out,
                    left,
                    right,
                    up,
                    down,
                    comment: v.comment.clone(),
                    // A Walls splay (anonymous `-`/`--` endpoint) is a passage
                    // cross-section, not a swum leg, so it seeds as Reference and
                    // the chain classifier refines it at import finalize (#27 q10).
                    kind: if v.is_reference {
                        ShotKind::Reference
                    } else {
                        ShotKind::Normal
                    },
                    marker_index: None,
                    temperature: None,
                });
            }
            let mut station_comments = Vec::new();
            if opts.comments_on_stations {
                for (shot_idx, shot) in shots.iter_mut().enumerate() {
                    if let Some(comment) = shot.comment.take() {
                        if !comment.is_empty() {
                            station_comments.push((shot_idx as u32, comment));
                        }
                    }
                }
            }
            let label = if multi {
                format!("{} (branch {})", survey.label, chain_idx + 1)
            } else {
                survey.label.clone()
            };
            // #438: one reference per section, so the chain's per-vector
            // corrections have to agree. They do for every ordinary survey (one
            // `Decl=`, one datum). When they do not - a `Decl=` changed
            // mid-survey, or RECT vectors mixed with compass ones - no single
            // constant can express the file, so this chain folds each shot's own
            // correction into its readings and declares the result true north.
            //
            // That fallback is only honest because the per-vector correction is
            // the one that reaches TRUE north (see where it is computed): the
            // grid convergence is never folded in, so the rotated bearings are
            // true-north bearings and the label is the truth about them. It is
            // still lossy for the MEASURED reading, which is why it is
            // announced.
            //
            // `shots` was built by pushing exactly one `ShotData` per element of
            // `chain`, in order, and the zip below is only correct while that
            // holds - a `continue` added to that loop would silently pair each
            // shot with the wrong vector's correction from the skip onward.
            debug_assert_eq!(
                shots.len(),
                chain.len(),
                "one shot per chain vector: the correction zip below pairs them positionally"
            );
            let azimuth_reference = {
                let mut declared = chain
                    .iter()
                    .map(|&idx| &survey.vectors[idx])
                    .map(|v| (v.north, v.azimuth_correction, v.correction_source));
                let first = declared.next();
                let uniform = match first {
                    Some((north, correction, _)) => declared
                        .clone()
                        .all(|(n, c, _)| n == north && (c - correction).abs() < 1e-9),
                    None => true,
                };
                match first {
                    Some((north, correction, source)) if uniform => {
                        // The SOURCE has to agree too, and it is not part of the
                        // uniformity test above because it does not change the
                        // geometry - only what the UI may say about it and what
                        // the export may recompute. Taking `chain[0]`'s over a
                        // chain whose vectors disagree would attribute the whole
                        // section to whichever entry happened to come first.
                        let source = if declared.all(|(_, _, s)| s == source) {
                            source
                        } else {
                            CorrectionSource::Unknown
                        };
                        AzimuthReference::from_parts(north, correction, source)
                    }
                    Some(_) => {
                        for (shot, &idx) in shots.iter_mut().zip(chain.iter()) {
                            let correction = survey.vectors[idx].azimuth_correction;
                            shot.azimuth_in = (shot.azimuth_in + correction).rem_euclid(360.0);
                            shot.azimuth_out = (shot.azimuth_out + correction).rem_euclid(360.0);
                        }
                        warnings.push(format!(
                            "Section '{}': the shots declare more than one declination, which a \
                             single section-level north reference cannot express. Each shot's own \
                             correction was applied to its readings and the section is recorded \
                             as true north.",
                            label
                        ));
                        AzimuthReference::true_north(CorrectionSource::FileDeclared)
                    }
                    None => AzimuthReference::default(),
                }
            };
            chains.push(PendingChain {
                section_index,
                names,
                shots,
                station_comments,
                label,
                surveyor: survey.surveyor.clone(),
                date: survey.date,
                azimuth_reference,
            });
        }
    }

    if coincident_ties > 0 {
        warnings.push(format!(
            "{coincident_ties} zero-length coincident-station tie(s) were given a \
             negligible {COINCIDENT_TIE_EPSILON_M} m length so they import as valid shots \
             (the model has no zero-length shot)."
        ));
    }

    // Pass 2b: spanning-tree placement over chain connectivity. Each component
    // is rooted at a fixed station (ZeroDatum) when it contains one, else at
    // Origin; the rest of the component chains to its root via FromImporting.
    let placements = place_chains(
        &chains.iter().map(|c| c.names.clone()).collect::<Vec<_>>(),
        &anchors,
        &mut warnings,
        |idx| chains[idx].label.clone(),
    );

    // Pass 2c: loop closures for shared stations that aren't tree edges.
    let closures = build_closures(
        &chains
            .iter()
            .map(|c| (c.section_index, c.names.clone()))
            .collect::<Vec<_>>(),
        &placements,
    );

    // Emit configs in BFS creation order.
    let mut configs: Vec<ImportSectionConfig> = Vec::with_capacity(chains.len());
    for &ci in &placements.creation_order {
        let chain = &chains[ci];
        configs.push(ImportSectionConfig {
            reverse: false,
            section_index: chain.section_index,
            name: Some(chain.label.clone()),
            surveyor: chain.surveyor.clone(),
            date: chain.date,
            color: SECTION_COLORS[chain.section_index as usize % SECTION_COLORS.len()].to_string(),
            direction: None,
            quality: 1.0,
            shots: chain.shots.clone(),
            placement: placements.placement[ci]
                .clone()
                .unwrap_or(ImportPlacement::Origin),
            station_comments: chain.station_comments.clone(),
            station_names: Vec::new(),
            azimuth_reference: Some(chain.azimuth_reference),
        });
    }
    topological_sort_configs(&mut configs);

    // Collapse identical warnings (e.g. the one-per-shot declination / missing-
    // azimuth notes) to a single line each, preserving first-seen order.
    {
        let mut seen: HashSet<String> = HashSet::new();
        warnings.retain(|w| seen.insert(w.clone()));
    }

    Ok((configs, closures, warnings))
}

/// The declination for one vector, and where the number came from (#438 - the
/// provenance is recorded on the section so the UI can explain it).
pub(crate) fn resolve_declination(
    v: &WallsVector,
    georef: &Option<WallsRef>,
    warnings: &mut Vec<String>,
) -> (f64, CorrectionSource) {
    // An explicit `Decl=` always wins - including an explicit `Decl=0`, which
    // declares the azimuths already true/grid and suppresses auto-derivation.
    if let Some(decl) = v.units_decl {
        return (decl, CorrectionSource::FileDeclared);
    }
    // Otherwise auto-derive from the IGRF model ONLY when the entry's
    // derive-declination flag is on AND a georeference + date are available.
    if !v.derive_decl {
        return (0.0, CorrectionSource::FileDeclared);
    }
    match (georef, v.date) {
        (Some(r), Some(date)) => {
            match crate::parsers::geo::igrf_declination(r.latitude, r.longitude, date) {
                Ok(d) => (
                    d,
                    CorrectionSource::IgrfAuto {
                        epoch_year: date.year(),
                    },
                ),
                // The bundled IGRF model only spans 1900-2025; a survey dated
                // outside that range (e.g. a current-year dive) would otherwise
                // import with a silent 0° correction. Surface it instead.
                // Identical per-shot lines collapse via the dedup at the end of
                // conversion.
                Err(_) => {
                    warnings.push(format!(
                        "Could not derive magnetic declination for {date} at {:.3}, {:.3} \
                         (date/location outside the IGRF model range 1900-2025); using 0°.",
                        r.latitude, r.longitude
                    ));
                    (0.0, CorrectionSource::Unknown)
                }
            }
        }
        _ => (0.0, CorrectionSource::Unknown),
    }
}

pub(crate) fn survey_label(survey: &ParsedSurvey) -> String {
    let title = survey.title.trim();
    if !title.is_empty() {
        title.to_string()
    } else if !survey.name.trim().is_empty() {
        survey.name.trim().to_string()
    } else {
        "Unnamed section".to_string()
    }
}

/// Collect every `#Fix` station that resolves to a geographic position, deduped
/// by station name (first declaration wins).
pub(crate) fn resolve_anchors(
    parsed: &ParsedWalls,
    opts: &WallsImportOptions,
    warnings: &mut Vec<String>,
) -> Vec<AnchorGeo> {
    let mut anchors: Vec<AnchorGeo> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (si, survey) in parsed.surveys.iter().enumerate() {
        // A deselected survey's fixes must not anchor (or warn as "unused") -
        // skip them so deselecting a fix-heavy surface mesh stays silent.
        if !opts.includes_survey(si) {
            continue;
        }
        for fix in &survey.fixes {
            let Some((lat, lon)) = fix.lat_long else {
                continue;
            };
            if seen.contains(&fix.name) {
                continue;
            }
            match GpsCoordinates::new(lat, lon, 0.0) {
                Ok(coordinates) => {
                    seen.insert(fix.name.clone());
                    anchors.push(AnchorGeo {
                        station: fix.name.clone(),
                        coordinates,
                        // A fixed station's `up` (elevation) maps to depth below
                        // the surface datum; absent it sits at depth 0.
                        depth: fix.up.map(|u| -u).unwrap_or(0.0),
                    });
                }
                Err(e) => warnings.push(format!("Ignoring invalid #Fix '{}': {e}", fix.name)),
            }
        }
    }
    anchors
}

/// Count the `#Fix` stations in ONE survey that would actually anchor: unique by
/// station name (first declaration wins) AND passing the same coordinate-validity
/// check `resolve_anchors` applies (`GpsCoordinates::new`). Without this the
/// per-survey preview stat counted raw `lat_long.is_some()` fixes - duplicates and
/// out-of-range projections included - so it could exceed the deduped, validated
/// project total and display nonsense like "3 / 2".
pub(crate) fn survey_fixed_station_count(survey: &ParsedSurvey) -> u32 {
    let mut seen: HashSet<&str> = HashSet::new();
    survey
        .fixes
        .iter()
        .filter(|f| {
            let Some((lat, lon)) = f.lat_long else {
                return false;
            };
            if !seen.insert(f.name.as_str()) {
                return false;
            }
            GpsCoordinates::new(lat, lon, 0.0).is_ok()
        })
        .count() as u32
}

/// Generic head-to-tail chain splitter.
pub(crate) fn split_into_chains<T>(
    items: &[T],
    ends: impl Fn(&T) -> (&str, &str),
) -> Vec<Vec<usize>> {
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut prev_to: Option<String> = None;
    for (i, item) in items.iter().enumerate() {
        let (from, to) = ends(item);
        let continues = prev_to.as_deref() == Some(from);
        if !continues && !current.is_empty() {
            chains.push(std::mem::take(&mut current));
        }
        current.push(i);
        prev_to = Some(to.to_string());
    }
    if !current.is_empty() {
        chains.push(current);
    }
    chains
}

/// Walk the station graph assigning each station an absolute depth (+down),
/// seeding each component from a known depth-gauge reading, else a fixed-station
/// anchor depth, else 0. Gauge readings win because they are the authoritative
/// vertical for dive data.
pub(crate) fn build_station_depths(
    edges: &[(String, String, f64)],
    known: &HashMap<String, f64>,
    anchors: &HashMap<&str, f64>,
    warnings: &mut Vec<String>,
) -> HashMap<String, f64> {
    // adjacency carries depth deltas (Δdepth = -Δup).
    let mut adjacency: HashMap<&str, Vec<(&str, f64)>> = HashMap::new();
    for (from, to, gain) in edges {
        adjacency
            .entry(from)
            .or_default()
            .push((to.as_str(), -gain));
        adjacency
            .entry(to)
            .or_default()
            .push((from.as_str(), *gain));
    }
    let mut nodes: Vec<&str> = adjacency.keys().copied().collect();
    nodes.sort_unstable();

    let mut depth: HashMap<String, f64> = HashMap::new();
    for &start in &nodes {
        if depth.contains_key(start) {
            continue;
        }
        // Discover the component (BFS) to choose the best seed.
        let mut component: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack = vec![start];
        seen.insert(start);
        while let Some(n) = stack.pop() {
            component.push(n);
            if let Some(nbrs) = adjacency.get(n) {
                for (m, _) in nbrs {
                    if seen.insert(*m) {
                        stack.push(*m);
                    }
                }
            }
        }
        // Choose seed station + absolute depth: gauge reading first (dive data
        // is authoritative), then a fixed-station anchor depth, else 0.
        let (seed, seed_depth) = if let Some(k) = component
            .iter()
            .filter(|n| known.contains_key(**n))
            .min()
            .copied()
        {
            (k, known[k])
        } else if let Some(a) = component
            .iter()
            .filter(|n| anchors.contains_key(**n))
            .min()
            .copied()
        {
            (a, anchors[a])
        } else {
            // `component` always contains `start`, so the fallback never fires;
            // expressed without `unwrap()` to satisfy the no-panic lint gate.
            (component.iter().min().copied().unwrap_or(start), 0.0)
        };

        // BFS from the seed propagating depth.
        let mut queue: VecDeque<&str> = VecDeque::new();
        depth.insert(seed.to_string(), seed_depth);
        queue.push_back(seed);
        while let Some(n) = queue.pop_front() {
            let current = depth[n];
            if let Some(nbrs) = adjacency.get(n) {
                for (m, delta) in nbrs {
                    if !depth.contains_key(*m) {
                        // A station's own depth-gauge reading is authoritative
                        // for dive data; only fall back to the integrated depth
                        // (seed + accumulated deltas) when it has none. When both
                        // exist and disagree materially, the gauge still wins but
                        // the discontinuity is surfaced rather than absorbed.
                        let integrated = current + delta;
                        let d = match known.get(*m) {
                            // Warn only on a gross discontinuity (well beyond
                            // normal depth-gauge noise), not a sub-meter wobble.
                            Some(&gauge) => {
                                if (gauge - integrated).abs() > 2.0 {
                                    warnings.push(format!(
                                        "Station {m}: depth-gauge reading {gauge:.2} m disagrees \
                                         with the integrated depth {integrated:.2} m; using the gauge."
                                    ));
                                }
                                gauge
                            }
                            // Only the seed anchor sets a component's depth datum
                            // (like the single horizontal zero-datum); a SECOND
                            // #Fix in the same component is a consistency check,
                            // not a second datum. Surface a material disagreement
                            // as a vertical misclosure but KEEP the integrated
                            // (measured-geometry) depth - overriding it would
                            // inject a fake vertical change and corrupt shot
                            // lengths, which Walls/dewalls never do.
                            None => {
                                if let Some(&anchor) = anchors.get(*m) {
                                    if (anchor - integrated).abs() > 2.0 {
                                        warnings.push(format!(
                                            "Station {m}: fixed-station depth {anchor:.2} m \
                                             disagrees with the integrated depth {integrated:.2} m; \
                                             keeping the integrated depth (vertical misclosure)."
                                        ));
                                    }
                                }
                                integrated
                            }
                        };
                        depth.insert((*m).to_string(), d);
                        queue.push_back(*m);
                    }
                }
            }
        }
    }
    depth
}

pub(crate) struct Placements {
    pub(crate) placement: Vec<Option<ImportPlacement>>,
    pub(crate) creation_order: Vec<usize>,
    /// For each chain: (parent chain index, merge station index) when joined.
    pub(crate) merge_station: Vec<Option<u32>>,
    pub(crate) parent: Vec<Option<usize>>,
}

/// Spanning-tree placement over chain connectivity. Each connected component is
/// rooted at a fixed station (ZeroDatum) when it contains one, else at Origin;
/// every other chain in the component merges to its discoverer via
/// FromImporting, and shared stations that aren't the tree edge become loop
/// closures (see [`build_closures`]). At most ONE zero datum is emitted per
/// component - the model forbids multiple datums per merge component - so extra
/// fixed stations in the same connected survey ride along via merges + closures
/// rather than becoming separate datums.
pub(crate) fn place_chains(
    chain_names: &[Vec<String>],
    anchors: &[AnchorGeo],
    warnings: &mut Vec<String>,
    label_of: impl Fn(usize) -> String,
) -> Placements {
    let n = chain_names.len();
    // Build chain adjacency: two chains neighbor iff they share a station name.
    let mut occurrences: HashMap<&str, Vec<usize>> = HashMap::new();
    for (ci, names) in chain_names.iter().enumerate() {
        for name in names {
            occurrences.entry(name.as_str()).or_default().push(ci);
        }
    }
    // Connect each station's occurrences as a star (hub = first chain) rather
    // than a full clique: O(k) edges instead of O(k²) for a station shared by k
    // chains. Every adjacency edge still corresponds to a shared station (so each
    // spanning-tree merge is valid) and the connected components are identical;
    // loop closures for the other shared occurrences are emitted separately in
    // `build_closures`.
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    for occ in occurrences.values() {
        let hub = occ[0];
        for &other in &occ[1..] {
            if other != hub {
                adjacency[hub].push(other);
                adjacency[other].push(hub);
            }
        }
    }
    for nbrs in adjacency.iter_mut() {
        nbrs.sort_unstable();
        nbrs.dedup();
    }

    // Per-chain station-name -> first index, so `shared_station` is O(child len)
    // instead of re-scanning the whole parent chain for every name.
    let chain_index: Vec<HashMap<&str, u32>> = chain_names
        .iter()
        .map(|names| {
            let mut m: HashMap<&str, u32> = HashMap::new();
            for (i, name) in names.iter().enumerate() {
                m.entry(name.as_str()).or_insert(i as u32);
            }
            m
        })
        .collect();
    let shared_station = |child: usize, parent: usize| -> Option<(u32, u32)> {
        for (ci, cname) in chain_names[child].iter().enumerate() {
            if let Some(&pi) = chain_index[parent].get(cname.as_str()) {
                return Some((ci as u32, pi));
            }
        }
        None
    };

    // Fixed-station name -> anchor index (first declaration wins per name).
    let anchor_by_name: HashMap<&str, usize> = {
        let mut m = HashMap::new();
        for (i, a) in anchors.iter().enumerate() {
            m.entry(a.station.as_str()).or_insert(i);
        }
        m
    };
    // A fixed station that no shot references can't be anchored to anything.
    for a in anchors {
        if !occurrences.contains_key(a.station.as_str()) {
            warnings.push(format!(
                "Fixed station '{}' isn't used by any shot; no zero datum created for it.",
                a.station
            ));
        }
    }
    // For each chain, the first station that is a fixed-station anchor.
    let chain_anchor: Vec<Option<(u32, usize)>> = chain_names
        .iter()
        .map(|names| {
            names
                .iter()
                .enumerate()
                .find_map(|(k, nm)| anchor_by_name.get(nm.as_str()).map(|&ai| (k as u32, ai)))
        })
        .collect();

    let mut placement: Vec<Option<ImportPlacement>> = vec![None; n];
    let mut merge_station: Vec<Option<u32>> = vec![None; n];
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut visited = vec![false; n];
    let mut creation_order: Vec<usize> = Vec::with_capacity(n);
    let have_any_anchor = !anchors.is_empty();

    for start in 0..n {
        if visited[start] {
            continue;
        }
        // Discover the whole component first so it can be rooted at an anchor.
        let mut component: Vec<usize> = Vec::new();
        {
            let mut seen: HashSet<usize> = HashSet::new();
            let mut stack = vec![start];
            seen.insert(start);
            while let Some(c) = stack.pop() {
                component.push(c);
                for &nb in &adjacency[c] {
                    if seen.insert(nb) {
                        stack.push(nb);
                    }
                }
            }
        }
        component.sort_unstable();

        // Root at a fixed station if the component has one (smallest chain idx).
        let anchored_chains: Vec<usize> = component
            .iter()
            .copied()
            .filter(|&ci| chain_anchor[ci].is_some())
            .collect();
        let root = anchored_chains.first().copied().unwrap_or(component[0]);

        if let Some((st_idx, ai)) = chain_anchor[root] {
            placement[root] = Some(ImportPlacement::ZeroDatum {
                coordinates: anchors[ai].coordinates,
                station_index: st_idx,
            });
            if anchored_chains.len() > 1 {
                let extras: Vec<&str> = anchored_chains
                    .iter()
                    .skip(1)
                    .filter_map(|&ci| chain_anchor[ci].map(|(_, j)| anchors[j].station.as_str()))
                    .collect();
                warnings.push(format!(
                    "Component anchored at '{}'; {} other fixed station(s) in the same connected \
                     survey are kept via merges + loop closures, not as separate datums: {}.",
                    anchors[ai].station,
                    extras.len(),
                    extras.join(", "),
                ));
            }
        } else {
            placement[root] = Some(ImportPlacement::Origin);
            if have_any_anchor {
                warnings.push(format!(
                    "Section '{}' is a separate component with no fixed station; placed at origin.",
                    label_of(root)
                ));
            }
        }
        visited[root] = true;
        creation_order.push(root);

        // BFS the component from its root, merging via FromImporting.
        let mut queue: VecDeque<usize> = VecDeque::new();
        queue.push_back(root);
        while let Some(p) = queue.pop_front() {
            for nb in adjacency[p].clone() {
                if visited[nb] {
                    continue;
                }
                if let Some((child_st, parent_st)) = shared_station(nb, p) {
                    placement[nb] = Some(ImportPlacement::FromImporting {
                        import_station: child_st,
                        target_section_index: p as u32,
                        target_station_index: parent_st,
                    });
                    merge_station[nb] = Some(child_st);
                    parent[nb] = Some(p);
                }
                visited[nb] = true;
                creation_order.push(nb);
                queue.push_back(nb);
            }
        }
    }

    Placements {
        placement,
        creation_order,
        merge_station,
        parent,
    }
}

/// Emit loop closures for shared stations that aren't the spanning-tree edge.
pub(crate) fn build_closures(
    chains: &[(u32, Vec<String>)],
    placements: &Placements,
) -> Vec<ImportClosure> {
    let mut canonical: HashMap<&str, (u32, u32)> = HashMap::new();
    for (section_index, names) in chains {
        for (k, name) in names.iter().enumerate() {
            canonical
                .entry(name.as_str())
                .and_modify(|prev| {
                    if *section_index < prev.0 {
                        *prev = (*section_index, k as u32);
                    }
                })
                .or_insert((*section_index, k as u32));
        }
    }
    let mut closures = Vec::new();
    let mut seen: HashSet<(u32, u32, u32, u32)> = HashSet::new();
    for (ci, (section_index, names)) in chains.iter().enumerate() {
        let merge_st = placements.merge_station[ci];
        let parent_section = placements.parent[ci].map(|p| chains[p].0);
        for (k, name) in names.iter().enumerate() {
            if let Some(&(canon_sec, canon_st)) = canonical.get(name.as_str()) {
                // The canonical occurrence itself needs no closure.
                if canon_sec == *section_index && canon_st == k as u32 {
                    continue;
                }
                // A station revisited later within the SAME chain (a contiguous
                // intra-chain loop, e.g. A->B->C->A) is the same physical station
                // as its canonical occurrence and must be tied by an
                // intra-section closure - fall through to emit it. The
                // already-joined-by-merge shortcut only applies to cross-section
                // ties.
                if canon_sec != *section_index
                    && Some(k as u32) == merge_st
                    && parent_section == Some(canon_sec)
                {
                    continue;
                }
                let key = (*section_index, k as u32, canon_sec, canon_st);
                if seen.insert(key) {
                    closures.push(ImportClosure {
                        from_section_index: *section_index,
                        from_station_index: k as u32,
                        to_section_index: canon_sec,
                        to_station_index: canon_st,
                    });
                }
            }
        }
    }
    closures
}

/// Reorder configs so any `FromImporting` target precedes its dependent.
pub(crate) fn topological_sort_configs(configs: &mut Vec<ImportSectionConfig>) {
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
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); configs.len()];
    let mut placed = vec![false; configs.len()];
    let mut order: Vec<usize> = Vec::with_capacity(configs.len());
    let mut ready: VecDeque<usize> = VecDeque::new();
    for (i, d) in dep.iter().enumerate() {
        match d {
            Some(target) if *target < configs.len() => children[*target].push(i),
            _ => ready.push_back(i),
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
