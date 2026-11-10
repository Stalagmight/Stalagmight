//! Minimal black-on-white stick-map SVG export.
//!
//! Renders a survey as a stick map with two stroke widths chosen per-segment
//! by a user-controlled branching threshold. Reference/temporary shots are
//! the segmentation boundary and are not drawn. Zero-datum stations are
//! rendered as filled 5-pointed stars. Thick (trunk) segments are annotated
//! at their deepest reachable station with a penetration-distance label
//! (`p. 560`), where distance is the **along-cave** shortest path from
//! the nearest zero datum (sum of shot lengths through the survey graph,
//! with `station_merges` / aligned closures as zero-cost edges) - the
//! same metric the Survey overview panel reports via
//! [`Project::calculate_penetration`], so the two agree.
//!
//! ## Segment / branching model
//!
//! A **segment** is a maximal chain of non-reference shots whose endpoint
//! stations are connected directly, via station merges, or via aligned loop
//! closures.
//!
//! A **tie** links two segments through a sequence of reference shots,
//! possibly passing through reference-only intermediate stations. A chain
//! `seg₁ -ref- · -ref- · -ref- seg₂` (with the dots in stations that
//! belong to no real segment) counts as one tie, because reference-shot
//! chains are an artifact of how the survey was taken - they have nothing
//! to do with the cave's topology. Implementation: every station whose
//! segment owns ≥1 non-reference shot is an *anchor*; everything else is
//! *free*. We compute connected components in the ref-shot subgraph of
//! free stations; each such *free cluster*, together with the anchors
//! linked to it by a single ref shot, forms one tie hub - every pair of
//! distinct segments attached to that hub becomes mutual neighbors. Direct
//! anchor↔anchor reference shots add a tie of length one.
//!
//! A segment's **branching factor** is `|neighbors|` - the number of
//! *distinct other segments* tied to it.
//!
//! With `branching_threshold = T`, segments with `branching_factor >= T` are
//! drawn at `THICK_PX`, all others at `THIN_PX`.

use anyhow::{Context, Result};
use resvg::tiny_skia;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use ts_rs::TS;
use usvg::{Options, Tree};

use super::atomic_write::write_atomic;
use crate::models::{Project, StationRef};

/// Output image format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Svg,
    Pdf,
}

/// Serde default for [`ExportImageOptions::show_penetration_labels`]: labels
/// are drawn unless a caller explicitly opts out, so an options blob that
/// predates the field keeps the original behavior.
fn default_true() -> bool {
    true
}

/// Returns `color` when it is a safe CSS hex color (`#rgb` or `#rrggbb`),
/// else `#000000`. Section colors are user data written verbatim into the
/// SVG attribute, so they must be validated to avoid attribute injection.
fn sanitize_hex_color(color: &str) -> &str {
    let bytes = color.as_bytes();
    if matches!(bytes.len(), 4 | 7)
        && bytes[0] == b'#'
        && bytes[1..].iter().all(u8::is_ascii_hexdigit)
    {
        color
    } else {
        "#000000"
    }
}

/// Escapes the five XML metacharacters so user free-text (section names,
/// station/shot comments) is safe to write into SVG `<text>` content. Without
/// this, a comment containing `&`, `<`, or `>` (all legal, common free text)
/// produces invalid XML that fails resvg/svg2pdf parsing on the PNG/PDF paths,
/// or injects sibling markup into the raw `.svg` (which has no parse gate). The
/// numeric labels (penetration, station index, shot data) need no escaping.
/// A char that the XML 1.0 `Char` production forbids, so it cannot appear in an
/// SVG (not even as a numeric char ref) and would make resvg/svg2pdf reject the
/// whole document (and corrupt a raw `.svg`). For a Rust `char` this is exactly:
/// the C0 controls `0x00-0x1F` except tab/LF/CR, plus `U+FFFE`/`U+FFFF` (both
/// excluded from `[#xE000-#xFFFD]`). Surrogates and `> U+10FFFF` cannot occur in
/// a `char`. Note the Unicode noncharacters `U+FDD0..=U+FDEF` are NOT dropped:
/// XML 1.0 permits them (they sit inside `[#xE000-#xFFFD]`).
fn is_xml_illegal_char(c: char) -> bool {
    let u = c as u32;
    (u < 0x20 && c != '\t' && c != '\n' && c != '\r') || u == 0xFFFE || u == 0xFFFF
}

/// The longest station name the export draws whole; longer names are cut to
/// one char less and end in an ellipsis. Matches the live map's cap.
const STATION_NAME_LABEL_MAX_CHARS: usize = 24;

/// A station name as the export draws it: control characters and bidi
/// overrides dropped (names come from the file, so they are untrusted text),
/// trimmed, and capped at [`STATION_NAME_LABEL_MAX_CHARS`]. `None` when nothing
/// is left, so an unnamed station gets no label rather than its index.
///
/// Mirrors the live map's `formatStationNameLabel` (`labelText.ts`), so the
/// preview and the file say the same thing: it also drops the Arabic letter
/// mark (U+061C) that `sanitize_display_text` keeps, and trims a byte-order
/// mark (U+FEFF) the way JavaScript's `trim` does.
fn station_name_label(name: Option<&str>) -> Option<String> {
    let clean = crate::models::sanitize_display_text(name?, usize::MAX).replace('\u{061c}', "");
    let clean = clean.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if clean.is_empty() {
        return None;
    }
    if clean.chars().count() <= STATION_NAME_LABEL_MAX_CHARS {
        return Some(clean.to_string());
    }
    let mut out: String = clean
        .chars()
        .take(STATION_NAME_LABEL_MAX_CHARS - 1)
        .collect();
    out.truncate(out.trim_end().len());
    out.push('\u{2026}');
    Some(out)
}

fn escape_xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if is_xml_illegal_char(c) => {}
            _ => out.push(c),
        }
    }
    out
}

// --- Shot color gradients (mirror the live survey map) ----------------------
// The map computes these colors in the FRONTEND (src/utils/colors.ts +
// distanceGradient.ts). To match on screen, port those exact endpoints + math
// here (the HSL lerp lives in depth_colors.rs).

/// Fallback depth-gradient endpoints: a teal -> coral pair.
///
/// These do NOT mirror any current theme, and must not be read as if they did.
/// They were `dark`'s `--cc-depth-shallow` / `--cc-depth-deep` when they were
/// written; `dark` has been retuned twice since and no longer matches. Syncing
/// them is deliberately NOT done - they are a last-resort default, several Rust
/// tests pin these exact values, and a constant that silently tracks a token it
/// cannot see is the drift this comment exists to warn about.
///
/// These are only a FALLBACK. The endpoints that actually reach the page are the
/// ones the frontend resolved from the live theme and sent on
/// [`ExportImageOptions`] (#360); the constants apply when a caller omits them
/// (an older frontend, a Rust test, a hand-built options struct).
///
/// They used to be the whole story, and correctly so: `getMapThemeColors()` in
/// src/utils/themeColors.ts forced the dark pair for every theme, so one
/// hardcoded pair really was theme-independent. #332 made it resolve the live
/// token per theme, which left the export matching `dark` alone - on the light
/// theme the screen drew navy → brick while the export drew teal → coral onto a
/// white page, the pairing src/theme/__tests__/tokens.test.ts measures at 1.13:1.
const DEPTH_SHALLOW: &str = "#4ecdc4";
const DEPTH_DEEP: &str = "#ff6b6b";

/// The depth-gradient endpoints for one export: whatever the frontend resolved
/// from the live theme, else the dark-theme constants.
#[derive(Clone, Copy)]
struct DepthRamp<'a> {
    shallow: &'a str,
    deep: &'a str,
}

/// A `#rrggbb` literal, or `None`. Deliberately strict: `hex_to_rgb` answers
/// black for anything it cannot parse, so an unrecognised CSS colour would paint
/// a black survey rather than fall back to the readable dark ramp. Only a shape
/// we know that helper reads correctly is allowed through.
fn parse_hex_endpoint(value: Option<&String>) -> Option<&str> {
    let s = value?.trim();
    let rest = s.strip_prefix('#')?;
    (rest.len() == 6 && rest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

impl ExportImageOptions {
    /// Resolve this export's depth ramp, falling back per-endpoint.
    fn depth_ramp(&self) -> DepthRamp<'_> {
        DepthRamp {
            shallow: parse_hex_endpoint(self.depth_shallow.as_ref()).unwrap_or(DEPTH_SHALLOW),
            deep: parse_hex_endpoint(self.depth_deep.as_ref()).unwrap_or(DEPTH_DEEP),
        }
    }
}

/// Live-map distance-gradient endpoints (near gold → far purple) + the grey the
/// map paints unreachable features. From `src/utils/distanceGradient.ts`.
const DIST_NEAR: (u8, u8, u8) = (0xf4, 0xd0, 0x3f);
const DIST_FAR: (u8, u8, u8) = (0x7d, 0x3c, 0x98);
const DIST_UNREACHABLE: &str = "#666666";

/// Depth-gradient color for `depth` normalized over `[min_d, max_d]`, matching
/// the survey map: a 2-stop HSL shortest-hue-path lerp shallow→deep. Non-finite
/// input falls back to black; a flat range collapses to the shallow color.
fn depth_gradient_color(depth: f64, min_d: f64, max_d: f64, ramp: DepthRamp<'_>) -> String {
    if !depth.is_finite() || !min_d.is_finite() || !max_d.is_finite() {
        return "#000000".to_string();
    }
    if max_d == min_d {
        return ramp.shallow.to_string();
    }
    let t = ((depth.clamp(min_d, max_d) - min_d) / (max_d - min_d)).clamp(0.0, 1.0);
    crate::export::depth_colors::interpolate_hsl(ramp.shallow, ramp.deep, t)
}

/// The global depth range the survey map normalizes over: min/max across every
/// station's z AND every shot's `depth_in`/`depth_out` (reference shots included,
/// to match `MapCanvas` even though only non-reference shots are drawn). Falls
/// back to `(0, 50)` when non-finite.
fn depth_range(project: &Project, stations: &[crate::models::Station]) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for s in stations {
        min = min.min(s.coordinates.z);
        max = max.max(s.coordinates.z);
    }
    for section in &project.sections {
        for shot in &section.shots {
            min = min.min(shot.depth_in).min(shot.depth_out);
            max = max.max(shot.depth_in).max(shot.depth_out);
        }
    }
    if min.is_finite() && max.is_finite() {
        (min.min(max), max.max(min))
    } else {
        (0.0, 50.0)
    }
}

/// Distance-gradient color for `value` normalized over `[0, max]`, matching the
/// survey map: `None`/negative render grey; otherwise a near→far lerp **in plain
/// (gamma) sRGB channels**. MapLibre's `['interpolate', ['linear'], …]` on colors
/// uses the default `rgb` space, which lerps the parsed gamma channels directly
/// (NOT linear light) - so a naive per-channel lerp is what actually matches the
/// on-screen distance ramp. (The GPU renders float colors, so the export's 8-bit
/// rounding lands within ±1 level of the map.)
fn distance_gradient_color(value: Option<f64>, max: f64) -> String {
    let Some(v) = value.filter(|v| v.is_finite() && *v >= 0.0) else {
        return DIST_UNREACHABLE.to_string();
    };
    let safe_max = if max > 0.0 { max } else { 1.0 };
    let t = (v / safe_max).clamp(0.0, 1.0);
    let lerp =
        |a: u8, b: u8| -> u8 { (f64::from(a) + t * (f64::from(b) - f64::from(a))).round() as u8 };
    format!(
        "#{:02x}{:02x}{:02x}",
        lerp(DIST_NEAR.0, DIST_FAR.0),
        lerp(DIST_NEAR.1, DIST_FAR.1),
        lerp(DIST_NEAR.2, DIST_FAR.2),
    )
}

/// How the exported map colors its shots. Mirrors the live survey map's shot
/// coloring so a PNG/PDF export matches what's on screen. Raster-only: SVG
/// export is always monochrome regardless of this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum ColorMode {
    /// Monochrome black-on-white (the classic minimal stick map).
    #[default]
    None,
    /// Each shot in its section's color.
    Section,
    /// Depth gradient (shallow teal → deep coral), matching the survey map.
    Depth,
    /// Along-cave distance-from-entrance gradient (gold → purple), matching the map.
    Distance,
}

/// Options for SVG / PNG / PDF map image export.
///
/// The export is a black-on-white stick map by default; the knobs are the output
/// format, the PNG DPI, the branching-factor threshold that decides where to
/// split between the two stroke widths, whether to draw the penetration-distance
/// labels, and how to color the shots.
// No longer `Copy`: the theme's depth endpoints arrive as owned `String`s (#360).
// Every consumer already takes `&ExportImageOptions`, so this costs nothing.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ExportImageOptions {
    /// Image format (png, svg, pdf).
    pub format: ImageFormat,
    /// DPI for PNG rasterization. Ignored for SVG and PDF.
    pub dpi: u32,
    /// Branching-factor threshold. Segments whose branching factor is
    /// `>= branching_threshold` are drawn thick; the rest are drawn thin.
    pub branching_threshold: u32,
    /// Whether to draw the penetration-distance labels at each thick segment's
    /// deepest reachable station. Defaults to `true` (the historical behavior).
    #[serde(default = "default_true")]
    pub show_penetration_labels: bool,
    /// How to color the shots (section / depth / distance gradient), mirroring
    /// the live survey map. Opt-in (default [`ColorMode::None`] = monochrome).
    /// **Raster-only**: SVG export is always monochrome regardless of this.
    #[serde(default)]
    pub color_mode: ColorMode,
    /// Whether to draw station-index labels at every station touched by at
    /// least one drawn (non-reference) shot - i.e. all points on the drawn
    /// passage, deduped by colocation (station merges / aligned closures).
    /// Opt-in (default `false`); black text + white halo, same placement
    /// machinery as the penetration labels, so labels stay monochrome
    /// regardless of `color_mode`.
    #[serde(default)]
    pub show_station_labels: bool,
    /// Whether to draw each station's name (e.g. the mNemo station number a
    /// DMP import recorded) instead of its index, with the same points and
    /// placement as `show_station_labels`. A point with no named station gets
    /// no label - there is no fallback to the index. Opt-in (default `false`).
    /// The UI keeps it exclusive with `show_station_labels`; when both arrive
    /// `true`, the index labels win, so a caller that predates this option
    /// keeps the meaning it always had.
    #[serde(default)]
    pub show_station_names: bool,
    /// Whether to draw section-name labels at the centroid of each section's
    /// drawn stations. Opt-in (default `false`); black text + white halo.
    #[serde(default)]
    pub show_section_names: bool,
    /// Whether to draw a station's comment text at the station point, for
    /// every station with a comment that is on drawn geometry. Opt-in
    /// (default `false`); black text + white halo.
    #[serde(default)]
    pub show_station_comments: bool,
    /// Whether to draw a shot's comment text at the shot midpoint, for every
    /// non-reference shot with a comment. Opt-in (default `false`); black
    /// text + white halo.
    #[serde(default)]
    pub show_shot_comments: bool,
    /// Whether to draw azimuth/length/depth data at every non-reference
    /// shot's midpoint, mirroring the live map's shot label. Opt-in (default
    /// `false`); black text + white halo.
    #[serde(default)]
    pub show_shot_data: bool,
    /// Mirror of `gas.include_reference_legs` (#27), so the exported distance
    /// ramp and penetration labels use the same graph the app is showing.
    ///
    /// **Server-populated, not a user-facing export option.** The commands
    /// overwrite it with the live config value, so it is `ts(skip)`-ed out of
    /// the generated TS: the frontend has no business setting it, and
    /// `serde(default)` means it need not send the field at all. It lives here
    /// rather than as a fourth parameter because it is one more thing the
    /// renderer needs to know, exactly like the other flags.
    #[serde(default)]
    #[ts(skip)]
    pub include_reference_legs: bool,
    /// The live theme's `--cc-depth-shallow` / `--cc-depth-deep`, as `#rrggbb`.
    ///
    /// The depth ramp is the one colour rule the export cannot own: #332 made
    /// each theme declare its own endpoints, so the only way the file can match
    /// the screen is for the frontend to send what it resolved (#360). Threaded
    /// rather than tabulated here on purpose - a second copy of the per-theme
    /// table, in a second language, is exactly what drifted the first time.
    ///
    /// Omitted (or not `#rrggbb`) falls back to the dark-theme constants, which
    /// is what every caller got before this existed.
    /// `ts(optional)`, not `ts(skip)`: unlike `include_reference_legs` the
    /// frontend is exactly who must supply these, but they are plumbing rather
    /// than a user-facing export option, so the UI's options literals need not
    /// carry them - `projectStore.exportImage` fills them in on the way out.
    #[serde(default)]
    #[ts(optional)]
    pub depth_shallow: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub depth_deep: Option<String>,
}

impl Default for ExportImageOptions {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            dpi: 300,
            branching_threshold: 2,
            show_penetration_labels: true,
            color_mode: ColorMode::None,
            show_station_labels: false,
            show_station_names: false,
            show_section_names: false,
            show_station_comments: false,
            show_shot_comments: false,
            show_shot_data: false,
            include_reference_legs: false,
            depth_shallow: None,
            depth_deep: None,
        }
    }
}

/// Top-level entry point. Dispatches on `format`. Renders from the measured
/// station coordinates; use [`export_image_with_stations`] to render from a
/// caller-supplied set (e.g. the closure-adjusted solve).
pub fn export_image(project: &Project, path: &Path, options: &ExportImageOptions) -> Result<()> {
    export_image_with_stations(
        project,
        path,
        options,
        project.compute_station_coordinates(),
    )
}

/// Like [`export_image`] but renders from a caller-supplied set of station
/// coordinates instead of the measured ones - the commands pass the memoized
/// closure-adjusted solve so the export matches the on-screen (adjusted) map.
pub fn export_image_with_stations(
    project: &Project,
    path: &Path,
    options: &ExportImageOptions,
    stations: Vec<crate::models::Station>,
) -> Result<()> {
    match options.format {
        ImageFormat::Svg => export_svg(project, path, options, stations),
        ImageFormat::Png => export_png(project, path, options, stations),
        ImageFormat::Pdf => export_pdf(project, path, options, stations),
    }
}

/// Export project as SVG (writes file).
fn export_svg(
    project: &Project,
    path: &Path,
    options: &ExportImageOptions,
    stations: Vec<crate::models::Station>,
) -> Result<()> {
    let svg = generate_svg_string_with_stations(project, options, stations)?;
    write_atomic(path, |w| Ok(w.write_all(svg.as_bytes())?)).context("Failed to write SVG file")?;
    tracing::info!(path = ?path, "Exported SVG");
    Ok(())
}

/// Maximum dimension for PNG export to prevent out-of-memory errors.
const MAX_PNG_DIMENSION: u32 = 16_384;

/// Export project as PNG via resvg rasterization.
fn export_png(
    project: &Project,
    path: &Path,
    options: &ExportImageOptions,
    stations: Vec<crate::models::Station>,
) -> Result<()> {
    if options.dpi == 0 {
        anyhow::bail!("DPI must be at least 1");
    }

    let svg_content = generate_svg_string_with_stations(project, options, stations)?;

    let mut opt = Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree =
        Tree::from_str(&svg_content, &opt).context("Failed to parse SVG for PNG conversion")?;

    let size = tree.size();
    let scale = options.dpi as f32 / 72.0;
    let width = ((size.width() * scale).ceil() as u32).max(1);
    let height = ((size.height() * scale).ceil() as u32).max(1);

    if width > MAX_PNG_DIMENSION || height > MAX_PNG_DIMENSION {
        anyhow::bail!(
            "Export size {width}x{height} exceeds maximum {MAX_PNG_DIMENSION}px. Reduce DPI or use SVG/PDF format."
        );
    }

    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| anyhow::anyhow!("Failed to create pixmap for {width}x{height} image"))?;

    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let png = pixmap.encode_png().context("Failed to encode PNG")?;
    write_atomic(path, |w| Ok(w.write_all(&png)?)).context("Failed to save PNG file")?;

    tracing::info!(path = ?path, dpi = options.dpi, width = width, height = height, "Exported PNG");
    Ok(())
}

/// Export project as PDF via svg2pdf.
fn export_pdf(
    project: &Project,
    path: &Path,
    options: &ExportImageOptions,
    stations: Vec<crate::models::Station>,
) -> Result<()> {
    let svg_content = generate_svg_string_with_stations(project, options, stations)?;

    let mut opt = Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = Tree::from_str(&svg_content, &opt).context("Failed to parse SVG")?;

    let pdf = svg2pdf::to_pdf(
        &tree,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions::default(),
    )
    .map_err(|e| anyhow::anyhow!("Failed to convert SVG to PDF: {e:?}"))?;

    write_atomic(path, |w| Ok(w.write_all(&pdf)?)).context("Failed to write PDF file")?;

    tracing::info!(path = ?path, "Exported PDF");
    Ok(())
}

// ---------------------------------------------------------------------------
// SVG generation
// ---------------------------------------------------------------------------

/// Target longer-dimension of the output SVG in pixels. World coordinates are
/// scaled to fit within this size so that `viewBox` and pixel sizes line up
/// cleanly for downstream raster/PDF conversion.
const TARGET_SIZE: f64 = 800.0;

/// World-space padding around the survey bounds, expressed as a fraction
/// of the larger bounding-box dimension. 0.05 = 5 % margin on every side.
/// Using the *larger* extent for both axes keeps the absolute margin
/// uniform on all four sides even when the survey is long-and-skinny.
const PADDING_RATIO: f64 = 0.05;

/// Stroke widths, in *pixels* (then converted to viewBox units by `px()`).
const THIN_PX: f64 = 1.0;
const THICK_PX: f64 = 2.5;

/// Junction trim distance, in *pixels*. Applied uniformly to every child
/// polyline endpoint that ties (via reference shots) to a parent or peer
/// segment, regardless of either side's stroke width. Sized at 2 × the
/// thick stroke so the visible gap stays roughly one parent-stroke wide
/// even after the rounded line cap eats back ~half a stroke.
const JUNCTION_TRIM_PX: f64 = THICK_PX * 2.0;

/// Minimum visible polyline length, in *pixels*, that junction trimming
/// will never eat below. Keeps short tied passages from disappearing
/// entirely when both ends want a trim wider than the run itself.
const MIN_VISIBLE_SHOT_PX: f64 = THICK_PX;

/// Zero-datum star sizing, in *pixels*.
const STAR_OUTER_PX: f64 = 7.0;
const STAR_INNER_PX: f64 = 3.0;

/// Penetration-distance label font size, in *pixels*.
const PEN_LABEL_PX: f64 = 11.0;
/// Halo (white outline) thickness for labels, in *pixels*. Wide enough to
/// keep digits legible when they fall on top of a shot line.
const PEN_LABEL_HALO_PX: f64 = 3.0;
/// Distance from the labelled station to the label centre, in *pixels*.
const PEN_LABEL_OFFSET_PX: f64 = 9.0;
/// Approximate per-glyph width as a fraction of `font-size`. Used to derive
/// the bounding box for placement-collision tests - we don't need pixel
/// accuracy, just enough to avoid label overlap.
const PEN_LABEL_CHAR_WIDTH_RATIO: f64 = 0.55;

/// Smallest extent (in world meters) we'll accept in either axis before
/// clamping kicks in. A single-station or all-colocated project would
/// otherwise produce a zero-size viewBox that usvg rejects on PNG/PDF
/// rasterization.
const MIN_BOUNDS_EXTENT_M: f64 = 1.0;

/// Generate SVG content as a string from the measured station coordinates.
/// Pure: never touches the filesystem.
pub fn generate_svg_string(project: &Project, options: &ExportImageOptions) -> Result<String> {
    generate_svg_string_with_stations(project, options, project.compute_station_coordinates())
}

/// Generate SVG content from a caller-supplied set of station coordinates. The
/// commands pass the memoized closure-adjusted solve so the exported/previewed
/// map matches the on-screen (adjusted) geometry.
pub fn generate_svg_string_with_stations(
    project: &Project,
    options: &ExportImageOptions,
    stations: Vec<crate::models::Station>,
) -> Result<String> {
    // ---- Bounds and scale ---------------------------------------------------
    // Bounds come from *rendered* geometry only - endpoints of non-reference
    // shots plus zero-datum stations. Reference shots aren't drawn, so a
    // long ref connector or ref-only intermediate station should NOT
    // inflate the viewBox and shrink the visible map.
    let station_by_ref: HashMap<StationRef, &crate::models::Station> =
        stations.iter().map(|s| (s.station_ref, s)).collect();
    let mut min_x = f64::MAX;
    let mut max_x = f64::MIN;
    let mut min_y = f64::MAX;
    let mut max_y = f64::MIN;
    let extend = |c: &crate::models::Coordinates,
                  mnx: &mut f64,
                  mxx: &mut f64,
                  mny: &mut f64,
                  mxy: &mut f64| {
        *mnx = mnx.min(c.x);
        *mxx = mxx.max(c.x);
        *mny = mny.min(c.y);
        *mxy = mxy.max(c.y);
    };
    for section in &project.sections {
        for (i, shot) in section.shots.iter().enumerate() {
            if shot.kind.is_virtual() {
                continue;
            }
            let from = StationRef::new(section.id, i as u32);
            let to = StationRef::new(section.id, (i + 1) as u32);
            if let Some(s) = station_by_ref.get(&from) {
                extend(
                    &s.coordinates,
                    &mut min_x,
                    &mut max_x,
                    &mut min_y,
                    &mut max_y,
                );
            }
            if let Some(s) = station_by_ref.get(&to) {
                extend(
                    &s.coordinates,
                    &mut min_x,
                    &mut max_x,
                    &mut min_y,
                    &mut max_y,
                );
            }
        }
    }
    for s in &stations {
        if s.is_zero_datum {
            extend(
                &s.coordinates,
                &mut min_x,
                &mut max_x,
                &mut min_y,
                &mut max_y,
            );
        }
    }
    if min_x >= f64::MAX {
        // Nothing rendered (project has only reference shots and no zero
        // datums, or is genuinely empty). Fall back to every station's
        // coords so we still emit something locally meaningful; if even
        // that is empty, use a 100×100 placeholder box.
        for s in &stations {
            extend(
                &s.coordinates,
                &mut min_x,
                &mut max_x,
                &mut min_y,
                &mut max_y,
            );
        }
        if min_x >= f64::MAX {
            min_x = 0.0;
            max_x = 100.0;
            min_y = 0.0;
            max_y = 100.0;
        }
    }

    // Clamp each axis to a minimum extent and re-centre the bounds around
    // the original midpoint so a single point or tight cluster ends up in
    // the middle of the viewBox rather than at a corner. Only `min_x` and
    // `max_y` are read again downstream (in the viewBox origin); the
    // clamped extents are carried separately so we don't bother updating
    // `max_x` / `min_y`.
    let raw_extent_x = max_x - min_x;
    let raw_extent_y = max_y - min_y;
    let extent_x = raw_extent_x.max(MIN_BOUNDS_EXTENT_M);
    let extent_y = raw_extent_y.max(MIN_BOUNDS_EXTENT_M);
    if extent_x > raw_extent_x {
        let mid_x = (min_x + max_x) * 0.5;
        min_x = mid_x - extent_x * 0.5;
    }
    if extent_y > raw_extent_y {
        let mid_y = (min_y + max_y) * 0.5;
        max_y = mid_y + extent_y * 0.5;
    }
    let pad = extent_x.max(extent_y) * PADDING_RATIO;
    let width = extent_x + pad * 2.0;
    let height = extent_y + pad * 2.0;
    let scale = TARGET_SIZE / width.max(height);
    let svg_width = (width * scale).max(1.0);
    let svg_height = (height * scale).max(1.0);

    // Pixel-to-viewBox-units helper. Same trick as the legacy export: viewBox
    // units are world meters, so a `pixels`-worth of any element shrinks by
    // `1/scale` when emitted. The downstream raster preserves pixel sizes.
    let px = |pixels: f64| -> f64 { pixels / scale };

    // ---- Build segment graph -----------------------------------------------
    // Stations indexed; union-find over them.
    let mut station_index: HashMap<StationRef, usize> = HashMap::with_capacity(stations.len());
    for (i, s) in stations.iter().enumerate() {
        station_index.insert(s.station_ref, i);
    }

    let mut uf = UnionFind::new(stations.len());

    // Merges and aligned closures collapse colocated stations into one node.
    for merge in &project.station_merges {
        if let (Some(a_sr), Some(b_sr)) = (
            project.resolve_key(&merge.station_a),
            project.resolve_key(&merge.station_b),
        ) {
            if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                uf.union(a, b);
            }
        }
    }
    for closure in &project.loop_closures {
        if !closure.is_aligned {
            continue;
        }
        if let (Some(a_sr), Some(b_sr)) = (
            project.resolve_key(&closure.station_a),
            project.resolve_key(&closure.station_b),
        ) {
            if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                uf.union(a, b);
            }
        }
    }

    // Non-reference shots inside each section connect their two endpoint
    // stations into the same segment.
    let mut has_non_ref_shot: HashSet<usize> = HashSet::new();
    for section in &project.sections {
        for (i, shot) in section.shots.iter().enumerate() {
            let from = StationRef::new(section.id, i as u32);
            let to = StationRef::new(section.id, (i + 1) as u32);
            let (Some(&fi), Some(&ti)) = (station_index.get(&from), station_index.get(&to)) else {
                continue;
            };
            if !shot.kind.is_virtual() {
                uf.union(fi, ti);
                // Mark both roots as "has at least one real shot" after we
                // settle roots below. For now, store the indices.
                has_non_ref_shot.insert(fi);
                has_non_ref_shot.insert(ti);
            }
        }
    }

    // Collapse `has_non_ref_shot` to roots so we can distinguish real segments
    // (>=1 non-reference shot) from stray reference-only stations.
    let mut real_segments: HashSet<usize> = HashSet::with_capacity(has_non_ref_shot.len());
    for &i in &has_non_ref_shot {
        real_segments.insert(uf.find(i));
    }

    // ---- Branching factor per segment --------------------------------------
    // Tie semantics: reference-shot chains through reference-only stations
    // count as a single tie. See the module-level docs.
    //
    // 1. Mark each station as anchor (its segment is real) or free (otherwise).
    // 2. Compute free clusters: connected components in the ref-shot subgraph
    //    restricted to free stations.
    // 3. Walk the ref shots. anchor↔anchor refs add a direct tie. anchor↔free
    //    refs record "this segment is attached to that free cluster".
    // 4. Each free cluster's attached segments form a clique of mutual ties.

    let n = stations.len();
    let mut anchor_segment: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        let root = uf.find(i);
        if real_segments.contains(&root) {
            anchor_segment[i] = Some(root);
        }
    }

    // `free_uf` clusters free stations. Two free stations are in the same
    // cluster if either (a) they're the same physical point - i.e. their
    // segment_uf roots already match, which encodes station_merges and
    // aligned closures - or (b) they're connected by a reference shot. The
    // first rule keeps merged "hub" stations across separate stub sections
    // from being split into multiple sibling clusters.
    let mut free_uf = UnionFind::new(n);
    let mut first_free_per_seg: HashMap<usize, usize> = HashMap::new();
    for i in 0..n {
        if anchor_segment[i].is_some() {
            continue;
        }
        let seg_root = uf.find(i);
        match first_free_per_seg.entry(seg_root) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(i);
            }
            std::collections::hash_map::Entry::Occupied(e) => {
                free_uf.union(*e.get(), i);
            }
        }
    }
    for section in &project.sections {
        for (i, shot) in section.shots.iter().enumerate() {
            if !shot.kind.is_virtual() {
                continue;
            }
            let from = StationRef::new(section.id, i as u32);
            let to = StationRef::new(section.id, (i + 1) as u32);
            let (Some(&fi), Some(&ti)) = (station_index.get(&from), station_index.get(&to)) else {
                continue;
            };
            if anchor_segment[fi].is_none() && anchor_segment[ti].is_none() {
                free_uf.union(fi, ti);
            }
        }
    }

    // Walk ref shots; bucket attached segments per free cluster, and add
    // direct anchor-anchor ties straight away.
    let mut neighbors: HashMap<usize, HashSet<usize>> = HashMap::new();
    let mut cluster_segments: HashMap<usize, HashSet<usize>> = HashMap::new();
    for section in &project.sections {
        for (i, shot) in section.shots.iter().enumerate() {
            if !shot.kind.is_virtual() {
                continue;
            }
            let from = StationRef::new(section.id, i as u32);
            let to = StationRef::new(section.id, (i + 1) as u32);
            let (Some(&fi), Some(&ti)) = (station_index.get(&from), station_index.get(&to)) else {
                continue;
            };
            match (anchor_segment[fi], anchor_segment[ti]) {
                (Some(ra), Some(rb)) => {
                    // Direct tie; ignore self-loops within one segment.
                    if ra != rb {
                        neighbors.entry(ra).or_default().insert(rb);
                        neighbors.entry(rb).or_default().insert(ra);
                    }
                }
                (Some(ra), None) => {
                    let cid = free_uf.find(ti);
                    cluster_segments.entry(cid).or_default().insert(ra);
                }
                (None, Some(rb)) => {
                    let cid = free_uf.find(fi);
                    cluster_segments.entry(cid).or_default().insert(rb);
                }
                (None, None) => {
                    // Both free; cluster membership already in free_uf.
                }
            }
        }
    }

    // For every free cluster, the attached segments all meet at one tie
    // hub and become mutual neighbors.
    for segs in cluster_segments.values() {
        if segs.len() < 2 {
            continue;
        }
        let v: Vec<usize> = segs.iter().copied().collect();
        for a in 0..v.len() {
            for b in (a + 1)..v.len() {
                neighbors.entry(v[a]).or_default().insert(v[b]);
                neighbors.entry(v[b]).or_default().insert(v[a]);
            }
        }
    }

    // ---- Emit SVG ----------------------------------------------------------
    let mut svg = String::new();
    let _ = write!(
        svg,
        r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{svg_width:.2}" height="{svg_height:.2}" viewBox="{vb_x:.1} {vb_y:.1} {vb_w:.1} {vb_h:.1}">
<rect x="{vb_x:.1}" y="{vb_y:.1}" width="{vb_w:.1}" height="{vb_h:.1}" fill="#ffffff"/>
"##,
        svg_width = svg_width,
        svg_height = svg_height,
        vb_x = min_x - pad,
        vb_y = -(max_y + pad), // Flip Y (SVG y is down).
        vb_w = width,
        vb_h = height,
    );

    // `station_by_ref` was built earlier (bounds pass); reuse here.

    let thin_sw = px(THIN_PX);
    let thick_sw = px(THICK_PX);
    let threshold = options.branching_threshold;

    // ---- Junction trimming ----------------------------------------------
    // A "child" segment that ties into a "parent" segment via reference
    // shots (possibly a short stub section, possibly a direct anchor↔
    // anchor ref) ends visually right next to the parent line, blending
    // into it. The parent's polyline typically passes *through* the
    // connection as a middle vertex, so it shouldn't be touched - only
    // the child should retract.
    //
    // Rule: for each polyline endpoint, look at the segments tied to its
    // physical point via reference shots. If any tied segment has a
    // branching factor ≥ this endpoint's own segment's, pull the
    // endpoint back by `JUNCTION_TRIM_PX` (a single uniform distance,
    // independent of either side's stroke width). Strictly-larger
    // parents stay intact (their tied siblings have smaller branching,
    // never satisfy ≥); equal-width siblings both retract (each sees
    // the other as ≥), giving a visible gap.
    //
    // Build a "physical-point" UF using ONLY station_merges and aligned
    // closures so colocated stations share a root, then collect, per
    // physical point, the other-segment(s) reachable across each
    // incident reference shot.
    let mut physical_uf = UnionFind::new(stations.len());
    for merge in &project.station_merges {
        if let (Some(a_sr), Some(b_sr)) = (
            project.resolve_key(&merge.station_a),
            project.resolve_key(&merge.station_b),
        ) {
            if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                physical_uf.union(a, b);
            }
        }
    }
    for closure in &project.loop_closures {
        if !closure.is_aligned {
            continue;
        }
        if let (Some(a_sr), Some(b_sr)) = (
            project.resolve_key(&closure.station_a),
            project.resolve_key(&closure.station_b),
        ) {
            if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                physical_uf.union(a, b);
            }
        }
    }
    let mut ref_tied_segs_per_phys: HashMap<usize, HashSet<usize>> = HashMap::new();
    for section in &project.sections {
        for (i, shot) in section.shots.iter().enumerate() {
            if !shot.kind.is_virtual() {
                continue;
            }
            let from = StationRef::new(section.id, i as u32);
            let to = StationRef::new(section.id, (i + 1) as u32);
            let (Some(&fi), Some(&ti)) = (station_index.get(&from), station_index.get(&to)) else {
                continue;
            };
            let f_phys = physical_uf.find(fi);
            let t_phys = physical_uf.find(ti);
            if f_phys == t_phys {
                continue; // self-loop within one physical point
            }
            // Other-side segments are determined by the same rule we use
            // in the branching pass: an anchor contributes its own
            // segment; a free station contributes its free cluster's
            // attached anchor segments.
            let mut segs_from = |idx: usize| -> Vec<usize> {
                match anchor_segment[idx] {
                    Some(s) => vec![s],
                    None => cluster_segments
                        .get(&free_uf.find(idx))
                        .map(|set| set.iter().copied().collect())
                        .unwrap_or_default(),
                }
            };
            for s in segs_from(ti) {
                ref_tied_segs_per_phys.entry(f_phys).or_default().insert(s);
            }
            for s in segs_from(fi) {
                ref_tied_segs_per_phys.entry(t_phys).or_default().insert(s);
            }
        }
    }
    // station_trim_dist[i] = how far the polyline endpoint at station i
    // should retract; 0 if no qualifying ref-shot tie exists. The trim
    // amount itself is the same uniform `JUNCTION_TRIM_PX` for every
    // qualifying endpoint - what varies per-station is *whether* to
    // trim, governed by the parent-vs-child branching rule.
    let trim_dist = px(JUNCTION_TRIM_PX);
    let mut station_trim_dist = vec![0.0_f64; stations.len()];
    for i in 0..stations.len() {
        let phys = physical_uf.find(i);
        let Some(tied) = ref_tied_segs_per_phys.get(&phys) else {
            continue;
        };
        let own_seg = uf.find(i);
        let own_branching = neighbors.get(&own_seg).map_or(0u32, |s| s.len() as u32);
        // If any tied segment has branching ≥ this endpoint's own, the
        // endpoint is a child or peer (not the strict parent) - retract.
        let qualifies = tied
            .iter()
            .filter(|&&s| s != own_seg)
            .any(|&s| neighbors.get(&s).map_or(0u32, |n| n.len() as u32) >= own_branching);
        if qualifies {
            station_trim_dist[i] = trim_dist;
        }
    }

    // Pass 1: shots. One polyline per maximal contiguous run of
    // non-reference shots in a section. Drawing per-run rather than
    // per-shot lets `stroke-linejoin="round"` smooth interior corners and
    // `stroke-linecap="round"` cover the cross-section/cross-run joins
    // (e.g. at merged stations), eliminating the butt-cap notches that
    // showed up at angled joins between consecutive `<line>` elements.
    //
    // Every shot in a single run shares the same segment_uf root (the run
    // is unioned by its own non-ref shots), so the run's stroke width is
    // well-defined by looking at any one shot's segment.
    // Shots are monochrome by default. Raster export can color them to match the
    // live survey map (SVG export always stays monochrome). Depth/Distance are
    // per-shot gradients; Section/None use the smooth run-batched polylines below.
    let mode = if matches!(options.format, ImageFormat::Svg) {
        ColorMode::None
    } else {
        options.color_mode
    };
    let depth_rng = (mode == ColorMode::Depth).then(|| depth_range(project, &stations));
    let depth_ramp = options.depth_ramp();
    let dist_field = (mode == ColorMode::Distance).then(|| {
        // The map colors the distance ramp from MEASURED distances (it draws
        // adjusted geometry but deliberately computes the distance field from
        // measured coords - see `AppState::distance_field`). Match that: distances
        // key by station_ref, so this stays consistent with the adjusted geometry
        // we draw. (It also skips the adjusted solve the map was designed to avoid.)
        let field = project.distance_field_from_with(
            &project.compute_station_coordinates(),
            None,
            options.include_reference_legs,
        );
        let by_ref: HashMap<StationRef, Option<f64>> = field
            .station_refs
            .iter()
            .copied()
            .zip(field.distances.iter().copied())
            .collect();
        (by_ref, field.max_reachable)
    });
    let _ = writeln!(
        svg,
        r##"<g class="shots" stroke="#000000" fill="none" stroke-linejoin="round" stroke-linecap="round">"##,
    );
    if matches!(mode, ColorMode::Depth | ColorMode::Distance) {
        // Per-shot gradient: mirror the live map's per-shot coloring. No junction
        // trimming here - the color variation carries the visual, not line breaks.
        for section in &project.sections {
            for j in 0..section.shots.len() {
                if section.shots[j].kind.is_virtual() {
                    continue;
                }
                let from_ref = StationRef::new(section.id, j as u32);
                let to_ref = StationRef::new(section.id, (j + 1) as u32);
                let (Some(from), Some(to)) =
                    (station_by_ref.get(&from_ref), station_by_ref.get(&to_ref))
                else {
                    continue;
                };
                // Same thick/thin classification as the run-batched path.
                let width = station_index
                    .get(&from_ref)
                    .map(|&fi| neighbors.get(&uf.find(fi)).map_or(0u32, |s| s.len() as u32))
                    .map_or(thin_sw, |b| if b >= threshold { thick_sw } else { thin_sw });
                let (x0, y0) = (from.coordinates.x, -from.coordinates.y);
                let (x1, y1) = (to.coordinates.x, -to.coordinates.y);
                if mode == ColorMode::Distance {
                    let color = match dist_field.as_ref() {
                        Some((by_ref, max)) => {
                            let avg = match (
                                by_ref.get(&from_ref).copied().flatten(),
                                by_ref.get(&to_ref).copied().flatten(),
                            ) {
                                (Some(a), Some(b)) => Some((a + b) / 2.0),
                                _ => None,
                            };
                            distance_gradient_color(avg, *max)
                        }
                        None => DIST_UNREACHABLE.to_string(),
                    };
                    let _ = writeln!(
                        svg,
                        r#"<polyline points="{x0:.2},{y0:.2} {x1:.2},{y1:.2}" stroke="{color}" stroke-width="{width}"/>"#,
                    );
                } else {
                    let (min_d, max_d) = depth_rng.unwrap_or((0.0, 50.0));
                    let shot = &section.shots[j];
                    // Subdivide into 4-12 segments (the map's smoothness formula),
                    // each colored at its midpoint depth.
                    let segs = ((shot.length / 2.0).ceil() as i64).clamp(4, 12) as usize;
                    for s in 0..segs {
                        let t0 = s as f64 / segs as f64;
                        let t1 = (s + 1) as f64 / segs as f64;
                        let mid = shot.depth_in
                            + (shot.depth_out - shot.depth_in) * ((s as f64 + 0.5) / segs as f64);
                        let color = depth_gradient_color(mid, min_d, max_d, depth_ramp);
                        let (ax, ay) = (x0 + (x1 - x0) * t0, y0 + (y1 - y0) * t0);
                        let (bx, by) = (x0 + (x1 - x0) * t1, y0 + (y1 - y0) * t1);
                        let _ = writeln!(
                            svg,
                            r#"<polyline points="{ax:.2},{ay:.2} {bx:.2},{by:.2}" stroke="{color}" stroke-width="{width}"/>"#,
                        );
                    }
                }
            }
        }
    } else {
        for section in &project.sections {
            // Per-section stroke override (computed once); `None` inherits the
            // group's black stroke for the monochrome path.
            let section_stroke =
                (mode == ColorMode::Section).then(|| sanitize_hex_color(&section.color));
            let mut i = 0;
            while i < section.shots.len() {
                if section.shots[i].kind.is_virtual() {
                    i += 1;
                    continue;
                }
                // Walk the maximal contiguous run of non-ref shots from `i`,
                // collecting station positions into a single polyline.
                let mut points: Vec<(f64, f64)> = Vec::new();
                let mut stroke_width = thin_sw;
                let mut first_si: Option<usize> = None;
                let mut last_si: Option<usize> = None;
                let mut j = i;
                while j < section.shots.len() && !section.shots[j].kind.is_virtual() {
                    let from_ref = StationRef::new(section.id, j as u32);
                    let to_ref = StationRef::new(section.id, (j + 1) as u32);
                    let (Some(from), Some(to)) =
                        (station_by_ref.get(&from_ref), station_by_ref.get(&to_ref))
                    else {
                        break;
                    };
                    let Some(&fi) = station_index.get(&from_ref) else {
                        break;
                    };
                    let Some(&ti) = station_index.get(&to_ref) else {
                        break;
                    };
                    if points.is_empty() {
                        let root = uf.find(fi);
                        let branching = neighbors.get(&root).map_or(0u32, |s| s.len() as u32);
                        stroke_width = if branching >= threshold {
                            thick_sw
                        } else {
                            thin_sw
                        };
                        first_si = Some(fi);
                        points.push((from.coordinates.x, -from.coordinates.y));
                    }
                    last_si = Some(ti);
                    points.push((to.coordinates.x, -to.coordinates.y));
                    j += 1;
                }
                // Trim either end that lands on a multi-segment junction so
                // distinct segments don't blend into one continuous line.
                // Cap the combined trim at `total_len - MIN_VISIBLE_SHOT_PX`
                // so a short tied passage doesn't get its vertices eaten
                // entirely and silently disappear from the export - scale
                // both ends proportionally when the budget is tight.
                let want_start = first_si.map_or(0.0, |si| station_trim_dist[si]);
                let want_end = last_si.map_or(0.0, |si| station_trim_dist[si]);
                if want_start > 0.0 || want_end > 0.0 {
                    let total_len = polyline_length(&points);
                    let min_visible = px(MIN_VISIBLE_SHOT_PX);
                    let max_total = (total_len - min_visible).max(0.0);
                    let (use_start, use_end) = if want_start + want_end <= max_total {
                        (want_start, want_end)
                    } else if want_start + want_end > 0.0 {
                        let s = max_total / (want_start + want_end);
                        (want_start * s, want_end * s)
                    } else {
                        (0.0, 0.0)
                    };
                    if use_start > 0.0 {
                        trim_polyline_end(&mut points, true, use_start);
                    }
                    if use_end > 0.0 {
                        trim_polyline_end(&mut points, false, use_end);
                    }
                }
                if points.len() >= 2 {
                    let mut pts_str = String::with_capacity(points.len() * 14);
                    for (idx, (x, y)) in points.iter().enumerate() {
                        if idx > 0 {
                            pts_str.push(' ');
                        }
                        let _ = write!(pts_str, "{x:.2},{y:.2}");
                    }
                    match section_stroke {
                        Some(stroke) => {
                            let _ = writeln!(
                                svg,
                                r#"<polyline points="{pts_str}" stroke="{stroke}" stroke-width="{stroke_width}"/>"#,
                            );
                        }
                        None => {
                            let _ = writeln!(
                                svg,
                                r#"<polyline points="{pts_str}" stroke-width="{stroke_width}"/>"#,
                            );
                        }
                    }
                }
                // Advance past the run, but never less than one - protects
                // against a station-lookup failure on the run's first shot
                // (which would leave `j == i`).
                i = if j > i { j } else { i + 1 };
            }
        }
    }
    svg.push_str("</g>\n");

    // Pass 2: zero-datum stars.
    let mut had_zero_datum = false;
    let star_outer = px(STAR_OUTER_PX);
    let star_inner = px(STAR_INNER_PX);
    for station in &stations {
        if !station.is_zero_datum {
            continue;
        }
        if !had_zero_datum {
            svg.push_str("<g class=\"zero-datums\" fill=\"#000000\">\n");
            had_zero_datum = true;
        }
        let points = star_polygon_points(
            station.coordinates.x,
            -station.coordinates.y,
            star_outer,
            star_inner,
        );
        let _ = writeln!(svg, r#"<polygon points="{points}"/>"#);
    }
    if had_zero_datum {
        svg.push_str("</g>\n");
    }

    let zero_datum_indices: Vec<usize> = stations
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_zero_datum)
        .map(|(i, _)| i)
        .collect();
    // Shared label-occupancy boxes (viewBox units): seeded with zero-datum
    // stars, then appended to by pen-distance labels (Pass 3) and station
    // labels (Pass 4) so all three mutually dodge. Only seeded when at least
    // one label pass will read it (skip the filter + allocation otherwise).
    let mut placed: Vec<(f64, f64, f64, f64)> = Vec::new();
    if options.show_penetration_labels
        || options.show_station_labels
        || options.show_station_names
        || options.show_section_names
        || options.show_station_comments
        || options.show_shot_comments
        || options.show_shot_data
    {
        let star_r = px(STAR_OUTER_PX);
        for &i in &zero_datum_indices {
            let s = &stations[i];
            placed.push((
                s.coordinates.x - star_r,
                -s.coordinates.y - star_r,
                star_r * 2.0,
                star_r * 2.0,
            ));
        }
    }

    // Pass 3: penetration-distance labels at each thick segment's deepest
    // station. Only attempted when the project has at least one zero datum
    // (otherwise penetration distance is undefined). Labels carry a white
    // halo for readability and dodge collisions by trying eight placement
    // offsets before giving up.
    if options.show_penetration_labels && !zero_datum_indices.is_empty() {
        // Use the project's shared along-cave penetration computation -
        // same metric as `Project::calculate_penetration` (and therefore
        // the Survey overview panel), so the map labels and the panel
        // value agree.
        let pen_dist =
            project.compute_penetration_distances_with(&stations, options.include_reference_legs);

        // Group stations by their segment root so we can find each
        // segment's deepest member in one pass.
        let mut segment_stations: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..stations.len() {
            let r = uf.find(i);
            if real_segments.contains(&r) {
                segment_stations.entry(r).or_default().push(i);
            }
        }

        // For each thick segment, pick the reachable station with the
        // greatest pen distance. Skip segments with nothing reachable.
        let mut deepest_per_thick: Vec<(usize, f64)> = Vec::new(); // (station_idx, dist)
        for (&root, members) in &segment_stations {
            let branching = neighbors.get(&root).map_or(0u32, |s| s.len() as u32);
            if branching < threshold {
                continue;
            }
            let mut best: Option<(usize, f64)> = None;
            for &i in members {
                let d = pen_dist[i];
                if !d.is_finite() {
                    continue;
                }
                if best.map_or(true, |(_, bd)| d > bd) {
                    best = Some((i, d));
                }
            }
            if let Some(b) = best {
                deepest_per_thick.push(b);
            }
        }

        if !deepest_per_thick.is_empty() {
            // Deterministic emission order (by station index) so SVG diffs
            // cleanly.
            deepest_per_thick.sort_by_key(|(i, _)| *i);

            let label_size = px(PEN_LABEL_PX);
            let halo = px(PEN_LABEL_HALO_PX);
            let offset = px(PEN_LABEL_OFFSET_PX);
            let char_w = label_size * PEN_LABEL_CHAR_WIDTH_RATIO;

            let _ = writeln!(
                svg,
                r##"<g class="pen-distance" font-family="sans-serif" font-size="{size:.2}" fill="#000000" stroke="#ffffff" stroke-width="{halo:.2}" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">"##,
                size = label_size,
                halo = halo,
            );
            let quarter_turn = std::f64::consts::FRAC_PI_4;
            for (si, dist) in deepest_per_thick {
                let sx = stations[si].coordinates.x;
                let sy = -stations[si].coordinates.y;
                let text = format!("p. {}", dist.round() as i64);
                let text_w = char_w * text.chars().count() as f64;
                let text_h = label_size;

                // Primary placement: along the bisector of the largest
                // angular gap between drawn shot directions at this
                // station. That puts the label in the *empty* wedge -
                // guaranteed not to overlap the line at the station
                // itself. Fallback placements rotate around the primary
                // in 45° steps and are only reached when another label
                // (or the zero-datum star) is already occupying the spot.
                let primary = label_open_space_angle(si, &stations, project, &station_index);
                let mut chosen: Option<(f64, f64, (f64, f64, f64, f64))> = None;
                let mut candidate_angles: Vec<f64> = Vec::with_capacity(9);
                candidate_angles.push(primary);
                for k in 1..=4 {
                    candidate_angles.push(primary + (k as f64) * quarter_turn);
                    candidate_angles.push(primary - (k as f64) * quarter_turn);
                }
                for angle in candidate_angles {
                    let lx = sx + offset * angle.cos();
                    let ly = sy + offset * angle.sin();
                    let bb = (lx - text_w * 0.5, ly - text_h * 0.5, text_w, text_h);
                    let overlap = placed.iter().any(|p| {
                        bb.0 < p.0 + p.2
                            && bb.0 + bb.2 > p.0
                            && bb.1 < p.1 + p.3
                            && bb.1 + bb.3 > p.1
                    });
                    if !overlap {
                        chosen = Some((lx, ly, bb));
                        break;
                    }
                }
                if let Some((lx, ly, bb)) = chosen {
                    let _ = writeln!(svg, r#"<text x="{lx:.2}" y="{ly:.2}">{text}</text>"#,);
                    placed.push(bb);
                }
                // If every candidate collided, drop the label rather
                // than stack it on top of another - better to lose one
                // number than to render an unreadable smudge.
            }
            svg.push_str("</g>\n");
        }
    }

    // Pass 4: station-index labels (4a) and station comments (4b), both keyed
    // off the same colocation/degree computation. `point_uf` collapses ONLY
    // colocated stations (merges + aligned closures) into one physical point,
    // NOT shot-connected chains (the segment `uf` above does that and is
    // therefore unusable for per-station degree). Degree counts NON-reference
    // shots incident to the point (reference/jump shots are diver shortcuts,
    // not passage, and are excluded), so a point with no non-reference shot
    // (deg 0) is skipped by both sub-passes. Drop-on-collision is the
    // backstop, sharing `placed` with Passes 2-3.
    //
    // Index labels win when both label options arrive set: the UI keeps them
    // exclusive, and a caller that predates names keeps the meaning it had.
    let show_station_names = options.show_station_names && !options.show_station_labels;
    if options.show_station_labels || show_station_names || options.show_station_comments {
        let mut point_uf = UnionFind::new(n);
        for merge in &project.station_merges {
            if let (Some(a_sr), Some(b_sr)) = (
                project.resolve_key(&merge.station_a),
                project.resolve_key(&merge.station_b),
            ) {
                if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                    point_uf.union(a, b);
                }
            }
        }
        for closure in &project.loop_closures {
            if !closure.is_aligned {
                continue;
            }
            if let (Some(a_sr), Some(b_sr)) = (
                project.resolve_key(&closure.station_a),
                project.resolve_key(&closure.station_b),
            ) {
                if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                    point_uf.union(a, b);
                }
            }
        }

        // Degree per physical point (non-reference shot endpoints).
        let mut degree: HashMap<usize, u32> = HashMap::new();
        for section in &project.sections {
            for (i, shot) in section.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                let from = StationRef::new(section.id, i as u32);
                let to = StationRef::new(section.id, (i + 1) as u32);
                let (Some(&fi), Some(&ti)) = (station_index.get(&from), station_index.get(&to))
                else {
                    continue;
                };
                *degree.entry(point_uf.find(fi)).or_default() += 1;
                *degree.entry(point_uf.find(ti)).or_default() += 1;
            }
        }

        let label_size = px(PEN_LABEL_PX);
        let halo = px(PEN_LABEL_HALO_PX);
        let offset = px(PEN_LABEL_OFFSET_PX);
        let char_w = label_size * PEN_LABEL_CHAR_WIDTH_RATIO;
        let quarter_turn = std::f64::consts::FRAC_PI_4;

        // Pass 4a: one label at every point touched by at least one drawn
        // shot (deg >= 1) - i.e. all stations on drawn passage, deduped by
        // colocation. The label is the station index, or with names on, the
        // station name. Representative station per point = the smallest
        // station index that has a label (deterministic): every station for
        // index labels, only named ones for names, so a merge of an unnamed
        // and a named station still shows the name, and a point with no
        // named station gets no label.
        if options.show_station_labels || show_station_names {
            let label_for = |i: usize| -> Option<String> {
                if show_station_names {
                    station_name_label(stations[i].name.as_deref())
                } else {
                    Some(stations[i].station_ref.station_index.to_string())
                }
            };
            let mut rep: HashMap<usize, usize> = HashMap::new();
            for i in 0..n {
                if show_station_names && label_for(i).is_none() {
                    continue;
                }
                // Ascending `i`: the first entry per point is its smallest.
                rep.entry(point_uf.find(i)).or_insert(i);
            }
            // Only the chosen stations' text is built.
            let mut selected: Vec<(usize, String)> = degree
                .iter()
                .filter(|&(_, &deg)| deg >= 1)
                .filter_map(|(root, _)| rep.get(root).copied())
                .filter_map(|si| label_for(si).map(|text| (si, text)))
                .collect();
            selected.sort_unstable_by_key(|&(si, _)| si);

            if !selected.is_empty() {
                let _ = writeln!(
                    svg,
                    r##"<g class="{class}" font-family="sans-serif" font-size="{size:.2}" fill="#000000" stroke="#ffffff" stroke-width="{halo:.2}" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">"##,
                    class = if show_station_names {
                        "station-names"
                    } else {
                        "station-labels"
                    },
                    size = label_size,
                    halo = halo,
                );
                for (si, text) in selected {
                    let sx = stations[si].coordinates.x;
                    let sy = -stations[si].coordinates.y;
                    let text_w = char_w * text.chars().count() as f64;
                    let text_h = label_size;
                    let primary = label_open_space_angle(si, &stations, project, &station_index);
                    let offsets = radial_offsets(primary, quarter_turn, offset);
                    if let Some((lx, ly, bb)) =
                        place_near(sx, sy, &offsets, text_w, text_h, &placed)
                    {
                        let _ = writeln!(
                            svg,
                            r#"<text x="{lx:.2}" y="{ly:.2}">{t}</text>"#,
                            t = escape_xml_text(&text),
                        );
                        placed.push(bb);
                    }
                }
                svg.push_str("</g>\n");
            }
        }

        // Pass 4b: a station's comment at its own point, for every station
        // with a comment that sits on drawn geometry. Unlike station labels
        // this is per-STATION, not deduped by point - colocated stations may
        // each carry their own comment.
        if options.show_station_comments {
            let mut opened = false;
            for i in 0..n {
                let Some(text) = stations[i].comment.clone() else {
                    continue;
                };
                if degree.get(&point_uf.find(i)).copied().unwrap_or(0) == 0 {
                    continue;
                }
                let sx = stations[i].coordinates.x;
                let sy = -stations[i].coordinates.y;
                let text_w = char_w * text.chars().count() as f64;
                let text_h = label_size;
                let primary = label_open_space_angle(i, &stations, project, &station_index);
                let offsets = radial_offsets(primary, quarter_turn, offset);
                if let Some((lx, ly, bb)) = place_near(sx, sy, &offsets, text_w, text_h, &placed) {
                    if !opened {
                        let _ = writeln!(
                            svg,
                            r##"<g class="station-comments" font-family="sans-serif" font-size="{size:.2}" fill="#000000" stroke="#ffffff" stroke-width="{halo:.2}" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">"##,
                            size = label_size,
                            halo = halo,
                        );
                        opened = true;
                    }
                    let _ = writeln!(
                        svg,
                        r#"<text x="{lx:.2}" y="{ly:.2}">{t}</text>"#,
                        t = escape_xml_text(&text),
                    );
                    placed.push(bb);
                }
            }
            if opened {
                svg.push_str("</g>\n");
            }
        }
    }

    // Pass 5: section-name labels at the centroid of each section's drawn
    // (non-reference) stations. Sections with zero drawn stations (all
    // reference, or empty) are skipped. Falls back to "Section {id}" when
    // the section has no user-given name.
    if options.show_section_names {
        let label_size = px(PEN_LABEL_PX);
        let halo = px(PEN_LABEL_HALO_PX);
        let offset = px(PEN_LABEL_OFFSET_PX);
        let char_w = label_size * PEN_LABEL_CHAR_WIDTH_RATIO;
        let mut opened = false;
        for section in &project.sections {
            let mut seen: HashSet<StationRef> = HashSet::new();
            let mut sum_x = 0.0;
            let mut sum_y = 0.0;
            let mut summed = 0u32;
            for (i, shot) in section.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                for sr in [
                    StationRef::new(section.id, i as u32),
                    StationRef::new(section.id, (i + 1) as u32),
                ] {
                    // Average only the points we actually resolved, so a missing
                    // lookup can't dilute the centroid toward the world origin.
                    if seen.insert(sr) {
                        if let Some(s) = station_by_ref.get(&sr) {
                            sum_x += s.coordinates.x;
                            sum_y += -s.coordinates.y;
                            summed += 1;
                        }
                    }
                }
            }
            if summed == 0 {
                continue;
            }
            let count = f64::from(summed);
            let cx = sum_x / count;
            let cy = sum_y / count;
            let text = section
                .name
                .clone()
                .unwrap_or_else(|| format!("Section {}", section.id));
            let text_w = char_w * text.chars().count() as f64;
            let text_h = label_size;
            let offsets = compass_offsets(offset);
            if let Some((lx, ly, bb)) = place_near(cx, cy, &offsets, text_w, text_h, &placed) {
                if !opened {
                    let _ = writeln!(
                        svg,
                        r##"<g class="section-names" font-family="sans-serif" font-size="{size:.2}" fill="#000000" stroke="#ffffff" stroke-width="{halo:.2}" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">"##,
                        size = label_size,
                        halo = halo,
                    );
                    opened = true;
                }
                let _ = writeln!(
                    svg,
                    r#"<text x="{lx:.2}" y="{ly:.2}">{t}</text>"#,
                    t = escape_xml_text(&text),
                );
                placed.push(bb);
            }
        }
        if opened {
            svg.push_str("</g>\n");
        }
    }

    // Shot midpoint + perpendicular-offset candidates, shared by Passes 6-7
    // below (kept as a free fn rather than a closure so both self-contained
    // passes can call it without fighting the borrow checker over `placed`).
    fn shot_label_anchor(
        from: &crate::models::Station,
        to: &crate::models::Station,
    ) -> (f64, f64, f64) {
        let x0 = from.coordinates.x;
        let y0 = -from.coordinates.y;
        let x1 = to.coordinates.x;
        let y1 = -to.coordinates.y;
        ((x0 + x1) * 0.5, (y0 + y1) * 0.5, (y1 - y0).atan2(x1 - x0))
    }
    // Candidate offsets for a shot-midpoint label: the two perpendicular sides
    // first (closest, cleanest), then the same two farther out, then a compass
    // ring as a last resort. Richer than a bare 2-candidate perpendicular so a
    // dense passage drops far fewer labels before giving up (matches the
    // 9-candidate budget the station/section passes get).
    fn shot_offsets(dir: f64, offset: f64) -> Vec<(f64, f64)> {
        let a = dir + std::f64::consts::FRAC_PI_2;
        let b = dir - std::f64::consts::FRAC_PI_2;
        let mut out = Vec::with_capacity(12);
        out.push((offset * a.cos(), offset * a.sin()));
        out.push((offset * b.cos(), offset * b.sin()));
        out.push((1.8 * offset * a.cos(), 1.8 * offset * a.sin()));
        out.push((1.8 * offset * b.cos(), 1.8 * offset * b.sin()));
        for k in 0..8 {
            let g = (k as f64) * std::f64::consts::FRAC_PI_4;
            out.push((offset * g.cos(), offset * g.sin()));
        }
        out
    }

    // Pass 6: shot-data labels (azimuth/length/depth) at each non-reference
    // shot's screen midpoint, matching the live map's shot label
    // (see `features.ts`): `{azimuth_in} {length}m {depth}m`, where depth is the
    // mean of the two endpoint stations' z (no sign flip). Precision is fixed at
    // 1 decimal (the app's default decimal_precision for all three); the
    // per-project precision config is not threaded into the exporter yet.
    if options.show_shot_data {
        let label_size = px(PEN_LABEL_PX);
        let halo = px(PEN_LABEL_HALO_PX);
        let offset = px(PEN_LABEL_OFFSET_PX);
        let char_w = label_size * PEN_LABEL_CHAR_WIDTH_RATIO;
        let mut opened = false;
        for section in &project.sections {
            for (i, shot) in section.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                let from_ref = StationRef::new(section.id, i as u32);
                let to_ref = StationRef::new(section.id, (i + 1) as u32);
                let (Some(from), Some(to)) =
                    (station_by_ref.get(&from_ref), station_by_ref.get(&to_ref))
                else {
                    continue;
                };
                let (mid_x, mid_y, dir) = shot_label_anchor(from, to);
                let depth = (from.coordinates.z + to.coordinates.z) / 2.0;
                let text = format!(
                    "{:.1}\u{00b0} {:.1}m {:.1}m",
                    shot.azimuth_in, shot.length, depth
                );
                let text_w = char_w * text.chars().count() as f64;
                let text_h = label_size;
                let offsets = shot_offsets(dir, offset);
                if let Some((lx, ly, bb)) =
                    place_near(mid_x, mid_y, &offsets, text_w, text_h, &placed)
                {
                    if !opened {
                        let _ = writeln!(
                            svg,
                            r##"<g class="shot-data" font-family="sans-serif" font-size="{size:.2}" fill="#000000" stroke="#ffffff" stroke-width="{halo:.2}" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">"##,
                            size = label_size,
                            halo = halo,
                        );
                        opened = true;
                    }
                    let _ = writeln!(svg, r#"<text x="{lx:.2}" y="{ly:.2}">{text}</text>"#,);
                    placed.push(bb);
                }
            }
        }
        if opened {
            svg.push_str("</g>\n");
        }
    }

    // Pass 7: shot-comment labels at each non-reference shot's screen
    // midpoint, for shots that carry a comment. Same anchor/offset recipe as
    // Pass 6.
    if options.show_shot_comments {
        let label_size = px(PEN_LABEL_PX);
        let halo = px(PEN_LABEL_HALO_PX);
        let offset = px(PEN_LABEL_OFFSET_PX);
        let char_w = label_size * PEN_LABEL_CHAR_WIDTH_RATIO;
        let mut opened = false;
        for section in &project.sections {
            for (i, shot) in section.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                let Some(text) = shot.comment.clone() else {
                    continue;
                };
                let from_ref = StationRef::new(section.id, i as u32);
                let to_ref = StationRef::new(section.id, (i + 1) as u32);
                let (Some(from), Some(to)) =
                    (station_by_ref.get(&from_ref), station_by_ref.get(&to_ref))
                else {
                    continue;
                };
                let (mid_x, mid_y, dir) = shot_label_anchor(from, to);
                let text_w = char_w * text.chars().count() as f64;
                let text_h = label_size;
                let offsets = shot_offsets(dir, offset);
                if let Some((lx, ly, bb)) =
                    place_near(mid_x, mid_y, &offsets, text_w, text_h, &placed)
                {
                    if !opened {
                        let _ = writeln!(
                            svg,
                            r##"<g class="shot-comments" font-family="sans-serif" font-size="{size:.2}" fill="#000000" stroke="#ffffff" stroke-width="{halo:.2}" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">"##,
                            size = label_size,
                            halo = halo,
                        );
                        opened = true;
                    }
                    let _ = writeln!(
                        svg,
                        r#"<text x="{lx:.2}" y="{ly:.2}">{t}</text>"#,
                        t = escape_xml_text(&text),
                    );
                    placed.push(bb);
                }
            }
        }
        if opened {
            svg.push_str("</g>\n");
        }
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

/// Sum of edge lengths along the polyline.
fn polyline_length(points: &[(f64, f64)]) -> f64 {
    points
        .windows(2)
        .map(|w| {
            let dx = w[1].0 - w[0].0;
            let dy = w[1].1 - w[0].1;
            (dx * dx + dy * dy).sqrt()
        })
        .sum()
}

/// Trim a polyline at one end by `dist` along the existing edges, walking
/// inward until the budget runs out. If a full edge is shorter than the
/// remaining trim, the edge's outer vertex is dropped entirely and we
/// continue with the leftover distance on the next edge. The polyline is
/// left potentially shorter (fewer vertices) than the caller passed in,
/// possibly degenerate (< 2 vertices) - the caller must check before
/// emitting.
fn trim_polyline_end(points: &mut Vec<(f64, f64)>, from_start: bool, mut dist: f64) {
    while points.len() >= 2 && dist > 0.0 {
        let (outer, inner) = if from_start {
            (points[0], points[1])
        } else {
            let last = points.len() - 1;
            (points[last], points[last - 1])
        };
        let dx = inner.0 - outer.0;
        let dy = inner.1 - outer.1;
        let edge_len = (dx * dx + dy * dy).sqrt();
        if edge_len < 1e-9 {
            // Degenerate edge; drop the outer vertex and keep going.
            if from_start {
                points.remove(0);
            } else {
                points.pop();
            }
            continue;
        }
        if dist >= edge_len {
            if from_start {
                points.remove(0);
            } else {
                points.pop();
            }
            dist -= edge_len;
        } else {
            let t = dist / edge_len;
            let new_x = outer.0 + dx * t;
            let new_y = outer.1 + dy * t;
            if from_start {
                points[0] = (new_x, new_y);
            } else {
                let last = points.len() - 1;
                points[last] = (new_x, new_y);
            }
            dist = 0.0;
        }
    }
}

/// Return the angle (in screen coordinates - y axis flipped, same frame the
/// SVG itself uses) that points into the largest empty wedge between the
/// *drawn* (non-reference) shot directions incident to `station_idx`.
///
/// Used to pick a label-placement direction that is guaranteed not to
/// overlap any line at the labelled station, no matter how the
/// surrounding passages bend. Stations physically colocated with
/// `station_idx` (via station_merges or aligned closures, detected here
/// by coordinate equality) contribute their incident shots too - merged
/// stations are the same point, so a shot leaving the merged twin still
/// leaves *this* station.
///
/// If no drawn shot touches the station, returns `FRAC_PI_2` (screen
/// down) as an arbitrary but deterministic fallback.
fn label_open_space_angle(
    station_idx: usize,
    stations: &[crate::models::Station],
    project: &Project,
    station_index: &HashMap<StationRef, usize>,
) -> f64 {
    let si_pos = stations[station_idx].coordinates;
    let eps = 1e-6_f64;
    let mut dirs: Vec<f64> = Vec::new();
    let push_dir = |dirs: &mut Vec<f64>, sx: f64, sy: f64, ox: f64, oy: f64| {
        let dx = ox - sx;
        // Screen y is flipped relative to world y.
        let dy = -(oy - sy);
        if dx.abs() > eps || dy.abs() > eps {
            dirs.push(dy.atan2(dx));
        }
    };
    for section in &project.sections {
        for (i, shot) in section.shots.iter().enumerate() {
            if shot.kind.is_virtual() {
                continue;
            }
            let from_ref = StationRef::new(section.id, i as u32);
            let to_ref = StationRef::new(section.id, (i + 1) as u32);
            let (Some(&fi), Some(&ti)) = (station_index.get(&from_ref), station_index.get(&to_ref))
            else {
                continue;
            };
            let from_pos = stations[fi].coordinates;
            let to_pos = stations[ti].coordinates;
            if (from_pos.x - si_pos.x).abs() < eps && (from_pos.y - si_pos.y).abs() < eps {
                push_dir(&mut dirs, si_pos.x, si_pos.y, to_pos.x, to_pos.y);
            }
            if (to_pos.x - si_pos.x).abs() < eps && (to_pos.y - si_pos.y).abs() < eps {
                push_dir(&mut dirs, si_pos.x, si_pos.y, from_pos.x, from_pos.y);
            }
        }
    }
    if dirs.is_empty() {
        return std::f64::consts::FRAC_PI_2;
    }
    dirs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let two_pi = std::f64::consts::TAU;
    let n = dirs.len();
    let mut best_gap = -1.0_f64;
    let mut best_mid = 0.0_f64;
    for i in 0..n {
        let a = dirs[i];
        let b = if i + 1 < n {
            dirs[i + 1]
        } else {
            dirs[0] + two_pi
        };
        let gap = b - a;
        if gap > best_gap {
            best_gap = gap;
            best_mid = a + gap * 0.5;
        }
    }
    best_mid
}

/// Returns the first candidate `(x, y)` in `offsets` (each a `(dx, dy)`
/// relative to `(hub_x, hub_y)`) whose text bounding box doesn't collide
/// with `placed`, along with that box. `None` if every candidate collided -
/// callers drop the label rather than stack it on top of another. Shared by
/// every label pass that dodges collisions from a single anchor point
/// (station labels/comments, section names, shot data/comments); Pass 3
/// (penetration labels) predates this helper and keeps its own inline copy.
fn place_near(
    hub_x: f64,
    hub_y: f64,
    offsets: &[(f64, f64)],
    text_w: f64,
    text_h: f64,
    placed: &[(f64, f64, f64, f64)],
) -> Option<(f64, f64, (f64, f64, f64, f64))> {
    for &(dx, dy) in offsets {
        let lx = hub_x + dx;
        let ly = hub_y + dy;
        let bb = (lx - text_w * 0.5, ly - text_h * 0.5, text_w, text_h);
        let overlap = placed.iter().any(|p| {
            bb.0 < p.0 + p.2 && bb.0 + bb.2 > p.0 && bb.1 < p.1 + p.3 && bb.1 + bb.3 > p.1
        });
        if !overlap {
            return Some((lx, ly, bb));
        }
    }
    None
}

/// Nine candidate `(dx, dy)` offsets at distance `offset` from a station:
/// `primary` first, then ± `step` fallback directions rotating outward (the
/// mirror is dropped at the step where + and - coincide). 8 distinct candidates
/// for the callers' pi/4 step. Mirrors the order Pass 3/4 used before
/// [`place_near`] existed.
fn radial_offsets(primary: f64, step: f64, offset: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(8);
    out.push((offset * primary.cos(), offset * primary.sin()));
    for k in 1..=4 {
        let a = primary + (k as f64) * step;
        out.push((offset * a.cos(), offset * a.sin()));
        // With the callers' step (pi/4), k == 4 makes primary - k*step point the
        // same way as primary + k*step (both primary +/- pi), so add the mirror
        // only for k < 4 to drop that duplicate direction. 8 distinct candidates.
        if k < 4 {
            let b = primary - (k as f64) * step;
            out.push((offset * b.cos(), offset * b.sin()));
        }
    }
    out
}

/// Nine candidate `(dx, dy)` offsets for a section-name label: the centroid
/// itself (no offset), then eight compass directions at distance `offset`.
fn compass_offsets(offset: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(9);
    out.push((0.0, 0.0));
    for k in 0..8 {
        let a = (k as f64) * std::f64::consts::FRAC_PI_4;
        out.push((offset * a.cos(), offset * a.sin()));
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the "points" attribute value for a filled 5-pointed star centered at
/// `(cx, cy)`. `r_outer` is the radius to the tips; `r_inner` is the radius
/// to the inner concave vertices.
///
/// The first point sits straight "up" in SVG coordinates (i.e. at angle
/// `-π/2`). Subsequent points alternate outer/inner and rotate clockwise.
fn star_polygon_points(cx: f64, cy: f64, r_outer: f64, r_inner: f64) -> String {
    let mut out = String::with_capacity(10 * 12);
    let start_angle = -std::f64::consts::FRAC_PI_2;
    let step = std::f64::consts::TAU / 10.0;
    for i in 0..10 {
        let r = if i % 2 == 0 { r_outer } else { r_inner };
        let angle = start_angle + (i as f64) * step;
        let x = cx + r * angle.cos();
        let y = cy + r * angle.sin();
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{x:.2},{y:.2}");
    }
    out
}

/// Tiny union-find with path compression + union by rank.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        // Path compression.
        let mut cur = x;
        while self.parent[cur] != root {
            let next = self.parent[cur];
            self.parent[cur] = root;
            cur = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::models::{GpsCoordinates, SectionData, ShotData, ShotKind, StationMerge};
    use tempfile::NamedTempFile;

    fn make_shot(length: f64, azimuth: f64, is_virtual: bool) -> ShotData {
        ShotData {
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            length,
            depth_in: 0.0,
            depth_out: 0.0,
            // Image export only cares about virtual-vs-not, so either virtual
            // kind exercises the same paths; Jump is the migration default.
            kind: if is_virtual {
                ShotKind::Jump
            } else {
                ShotKind::Normal
            },
            ..Default::default()
        }
    }

    fn simple_project() -> Project {
        let mut project = Project::new("Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 0.0, false)],
            ..Default::default()
        });
        project
    }

    // ---- Output shape ------------------------------------------------------

    #[test]
    fn svg_has_white_background() {
        let svg = generate_svg_string(&simple_project(), &ExportImageOptions::default()).unwrap();
        assert!(
            svg.contains(r##"fill="#ffffff""##),
            "background should be white: {svg}"
        );
    }

    #[test]
    fn empty_project_still_valid_svg() {
        let svg = generate_svg_string(
            &Project::new("Empty".to_string(), None),
            &ExportImageOptions::default(),
        )
        .unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("viewBox"));
        assert!(svg.contains("</svg>"));
    }

    /// Helper: parse `viewBox="x y w h"` out of the SVG header so bounds
    /// regression tests can assert on the rasterised area.
    fn parse_viewbox(svg: &str) -> (f64, f64, f64, f64) {
        let start = svg.find("viewBox=\"").expect("no viewBox") + 9;
        let end = start + svg[start..].find('"').expect("unterminated viewBox");
        let parts: Vec<f64> = svg[start..end]
            .split_whitespace()
            .map(|s| s.parse().expect("non-numeric viewBox part"))
            .collect();
        assert_eq!(parts.len(), 4, "viewBox should have 4 numbers");
        (parts[0], parts[1], parts[2], parts[3])
    }

    #[test]
    fn zero_size_bounds_are_clamped_to_non_degenerate_viewbox() {
        // A single-station project (one section with no shots) - the
        // only station sits at the origin, so raw extent_x == extent_y
        // == 0. Without clamping the viewBox would be 0×0 and usvg
        // would refuse to rasterise it.
        let mut project = Project::new("Single".to_string(), None);
        project.add_section(SectionData {
            name: Some("Just one station".to_string()),
            color: "#000000".to_string(),
            shots: vec![],
            ..Default::default()
        });
        let svg = generate_svg_string(&project, &ExportImageOptions::default()).unwrap();
        let (_, _, w, h) = parse_viewbox(&svg);
        assert!(w > 0.0, "width should be positive: viewBox w={w}");
        assert!(h > 0.0, "height should be positive: viewBox h={h}");

        // PNG rasterisation should succeed (used to fail on zero-size).
        let file = NamedTempFile::new().unwrap();
        let path = file.path().with_extension("png");
        export_png(
            &project,
            &path,
            &ExportImageOptions {
                format: ImageFormat::Png,
                dpi: 72,
                branching_threshold: 2,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
            project.compute_station_coordinates(),
        )
        .unwrap();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reference_only_stations_do_not_inflate_bounds() {
        // A small drawn passage (10 m east) with a long invisible
        // reference shot (1000 m further east). The viewBox should hug
        // the drawn 10 m line, not the 1010 m total span - otherwise
        // the actual stick map shrinks to ~1 % of the rendered area.
        let mut project = Project::new("RefBounds".to_string(), None);
        let main_id = project
            .add_section(SectionData {
                name: Some("Main".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 90.0, false)],
                ..Default::default()
            })
            .id;
        let ref_id = project
            .add_section(SectionData {
                name: Some("LongRef".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(1000.0, 90.0, true)], // invisible
                ..Default::default()
            })
            .id;
        // Connect the long ref shot's start to main's end so it lives in
        // the same coordinate frame.
        let merged_at = chrono::Utc::now();
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(ref_id, 0).into(),
            station_b: StationRef::new(main_id, 1).into(),
            merged_at,
        });

        let svg = generate_svg_string(&project, &ExportImageOptions::default()).unwrap();
        let (_, _, w, _) = parse_viewbox(&svg);
        // Width should reflect just the drawn 10 m extent + padding -
        // an order of magnitude tighter than the 1000 m raw span.
        assert!(
            w < 100.0,
            "viewBox width {w} should hug the drawn 10 m line, not the 1000 m ref shot",
        );
    }

    #[test]
    fn export_svg_writes_file() {
        let project = simple_project();
        let file = NamedTempFile::new().unwrap();
        let path = file.path().with_extension("svg");
        export_svg(
            &project,
            &path,
            &ExportImageOptions::default(),
            project.compute_station_coordinates(),
        )
        .unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("<?xml"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn export_png_writes_valid_file() {
        let project = simple_project();
        let file = NamedTempFile::new().unwrap();
        let path = file.path().with_extension("png");
        let options = ExportImageOptions {
            format: ImageFormat::Png,
            dpi: 72,
            branching_threshold: 2,
            show_penetration_labels: true,
            color_mode: ColorMode::None,
            show_station_labels: false,
            ..Default::default()
        };
        export_png(
            &project,
            &path,
            &options,
            project.compute_station_coordinates(),
        )
        .unwrap();
        let contents = std::fs::read(&path).unwrap();
        assert!(contents.len() > 8);
        assert_eq!(&contents[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        std::fs::remove_file(&path).ok();
    }

    /// Ported from the WDIO `export-scaling` spec (#120): PNG raster dimensions
    /// must scale linearly with DPI. `export_png` uses `scale = dpi / 72`, so
    /// quadrupling the DPI must (modulo 1px ceil rounding per axis) ~4x each
    /// dimension. This is the one export-scaling assertion not already covered
    /// by `export_png_writes_valid_file` / the SVG structure tests.
    #[test]
    fn export_png_dimensions_scale_with_dpi() {
        let project = simple_project();

        let png_dims = |dpi: u32| -> (u32, u32) {
            let file = NamedTempFile::new().unwrap();
            let path = file.path().with_extension("png");
            export_png(
                &project,
                &path,
                &ExportImageOptions {
                    format: ImageFormat::Png,
                    dpi,
                    branching_threshold: 2,
                    show_penetration_labels: true,
                    color_mode: ColorMode::None,
                    show_station_labels: false,
                    ..Default::default()
                },
                project.compute_station_coordinates(),
            )
            .unwrap();
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).ok();
            // PNG layout: 8-byte signature, 4-byte IHDR length, 4-byte "IHDR",
            // then big-endian u32 width (bytes 16..20) and height (bytes 20..24).
            // Validate the magic + length first so a future empty/partial/non-PNG
            // export fails with a clear assertion instead of an out-of-bounds panic.
            const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
            assert!(
                bytes.len() >= 24 && bytes[..8] == PNG_SIGNATURE,
                "export_png did not write a valid PNG at dpi {dpi} ({} bytes)",
                bytes.len()
            );
            let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
            let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
            (w, h)
        };

        let (w1, h1) = png_dims(72); // scale 1.0
        let (w4, h4) = png_dims(288); // scale 4.0
        assert!(w1 > 0 && h1 > 0, "base dimensions must be positive");
        assert!(
            (w4 as i64 - 4 * w1 as i64).abs() <= 4,
            "width should scale ~4x with dpi: 72->{w1}, 288->{w4}"
        );
        assert!(
            (h4 as i64 - 4 * h1 as i64).abs() <= 4,
            "height should scale ~4x with dpi: 72->{h1}, 288->{h4}"
        );
    }

    #[test]
    fn export_pdf_writes_nonempty_file() {
        let project = simple_project();
        let file = NamedTempFile::new().unwrap();
        let path = file.path().with_extension("pdf");
        let options = ExportImageOptions {
            format: ImageFormat::Pdf,
            ..Default::default()
        };
        export_image(&project, &path, &options).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len > 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn png_oversized_dpi_errors() {
        let project = simple_project();
        let file = NamedTempFile::new().unwrap();
        let path = file.path().with_extension("png");
        let options = ExportImageOptions {
            format: ImageFormat::Png,
            dpi: 2000,
            branching_threshold: 2,
            show_penetration_labels: true,
            color_mode: ColorMode::None,
            show_station_labels: false,
            ..Default::default()
        };
        let err = export_png(
            &project,
            &path,
            &options,
            project.compute_station_coordinates(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("16384"), "{err}");
    }

    #[test]
    fn png_dpi_zero_errors() {
        let project = simple_project();
        let file = NamedTempFile::new().unwrap();
        let path = file.path().with_extension("png");
        let options = ExportImageOptions {
            format: ImageFormat::Png,
            dpi: 0,
            branching_threshold: 2,
            show_penetration_labels: true,
            color_mode: ColorMode::None,
            show_station_labels: false,
            ..Default::default()
        };
        let err = export_png(
            &project,
            &path,
            &options,
            project.compute_station_coordinates(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("DPI"), "{err}");
    }

    // ---- Segmentation / branching -----------------------------------------

    /// Build a project: one section with five shots; shot index 2 is a
    /// reference shot. That splits the chain into two segments:
    ///   segment A = shots 0,1 (between stations 0..=2)
    ///   segment B = shots 3,4 (between stations 3..=5)
    fn project_with_one_ref_separator() -> Project {
        let mut project = Project::new("RefSplit".to_string(), None);
        project.add_section(SectionData {
            name: Some("S".to_string()),
            color: "#000000".to_string(),
            shots: vec![
                make_shot(10.0, 0.0, false),  // 0 -> 1
                make_shot(10.0, 90.0, false), // 1 -> 2
                make_shot(10.0, 90.0, true),  // 2 -> 3  (reference)
                make_shot(10.0, 0.0, false),  // 3 -> 4
                make_shot(10.0, 90.0, false), // 4 -> 5
            ],
            ..Default::default()
        });
        project
    }

    #[test]
    fn short_passage_survives_when_both_ends_qualify_for_trim() {
        // Pathological case codex flagged: a tied non-reference run
        // shorter than 2 × JUNCTION_TRIM_PX at the current scale would
        // have its outer vertices both eaten by the trim and the
        // polyline silently dropped from the export. The clamp now
        // keeps at least one stroke-width of visible polyline.
        //
        // Build a parent (100 m east) and a 1 m side that's tied to
        // the parent at *both* ends via short stubs - so both side
        // endpoints qualify for trim and would, without the clamp,
        // consume the entire side polyline.
        let mut project = Project::new("ShortSide".to_string(), None);
        let parent_id = project
            .add_section(SectionData {
                name: Some("Parent".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(100.0, 90.0, false)],
                ..Default::default()
            })
            .id;
        let side_id = project
            .add_section(SectionData {
                name: Some("ShortSide".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(1.0, 0.0, false)], // 1 m only
                ..Default::default()
            })
            .id;
        let merged_at = chrono::Utc::now();
        for (parent_station, side_station) in [(0u32, 0u32), (1, 1)] {
            let stub = project
                .add_section(SectionData {
                    name: Some("Stub".to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(1.0, 0.0, true)],
                    ..Default::default()
                })
                .id;
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 0).into(),
                station_b: StationRef::new(parent_id, parent_station).into(),
                merged_at,
            });
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 1).into(),
                station_b: StationRef::new(side_id, side_station).into(),
                merged_at,
            });
        }

        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 1,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let polylines = parse_polyline_points(&svg);
        // Two polylines must survive: the parent and the (clamped) side.
        assert_eq!(polylines.len(), 2, "{svg}");
        // The side polyline is the short one. Its remaining length must
        // be positive (at least the minimum visible budget) - proves
        // the clamp kept it alive.
        let side_poly = polylines
            .iter()
            .min_by(|a, b| {
                let la = polyline_geom_len(a);
                let lb = polyline_geom_len(b);
                la.partial_cmp(&lb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();
        let side_len = polyline_geom_len(side_poly);
        assert!(
            side_len > 0.0,
            "short side polyline should retain a visible remainder; got len={side_len}: {svg}",
        );
    }

    fn polyline_geom_len(pts: &[(f64, f64)]) -> f64 {
        pts.windows(2)
            .map(|w| {
                let dx = w[1].0 - w[0].0;
                let dy = w[1].1 - w[0].1;
                (dx * dx + dy * dy).sqrt()
            })
            .sum()
    }

    #[test]
    fn child_segment_is_trimmed_at_ref_shot_tie_to_larger_parent() {
        // A "trunk" parent runs east and is tied at its middle station to
        // *three* child segments via short stub ref shots, giving the
        // parent branching factor 3 and each child branching factor 1.
        //
        // After trimming the children should retract from the parent (so
        // they don't visually merge into one big shape) while the parent
        // is left untouched.
        let mut project = Project::new("ParentChild".to_string(), None);
        let trunk_id = project
            .add_section(SectionData {
                name: Some("Trunk".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(50.0, 90.0, false), make_shot(50.0, 90.0, false)],
                ..Default::default()
            })
            .id;
        let child_x = project
            .add_section(SectionData {
                name: Some("ChildX".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(40.0, 0.0, false)], // north
                ..Default::default()
            })
            .id;
        let child_y = project
            .add_section(SectionData {
                name: Some("ChildY".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(40.0, 180.0, false)], // south
                ..Default::default()
            })
            .id;
        let child_z = project
            .add_section(SectionData {
                name: Some("ChildZ".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(40.0, 45.0, false)], // NE
                ..Default::default()
            })
            .id;

        // Stub section per child: one ref shot whose two ends merge into
        // the trunk's middle station and the child's start.
        let trunk_mid = StationRef::new(trunk_id, 1);
        let merged_at = chrono::Utc::now();
        for child in [child_x, child_y, child_z] {
            let stub = project
                .add_section(SectionData {
                    name: Some("Stub".to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(1.0, 0.0, true)], // short ref
                    ..Default::default()
                })
                .id;
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 0).into(),
                station_b: trunk_mid.into(),
                merged_at,
            });
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 1).into(),
                station_b: StationRef::new(child, 0).into(),
                merged_at,
            });
        }

        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 3, // trunk thick, children thin
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let polylines = parse_polyline_points(&svg);
        assert_eq!(polylines.len(), 4, "{svg}"); // trunk + 3 children

        // The trunk polyline: only one polyline has 3 vertices (the two
        // shots of the trunk). Find it and assert its endpoints are
        // exactly at the original station positions (no trim).
        let trunk_poly = polylines
            .iter()
            .find(|pts| pts.len() == 3)
            .unwrap_or_else(|| panic!("missing 3-vertex trunk polyline: {svg}"));
        assert!(
            (trunk_poly[0].0 - 0.0).abs() < 1e-3 && (trunk_poly[0].1).abs() < 1e-3,
            "trunk west endpoint should be untouched: {trunk_poly:?}",
        );
        assert!(
            (trunk_poly[2].0 - 100.0).abs() < 1e-3 && (trunk_poly[2].1).abs() < 1e-3,
            "trunk east endpoint should be untouched: {trunk_poly:?}",
        );

        // Each child polyline has 2 vertices: a "start" vertex (merged
        // onto the trunk via the stub) and an "end" vertex 40 m away in
        // the shot direction. The world coordinates the children live
        // in depend on how compute_station_coordinates resolves the
        // merge graph (and don't necessarily centre on the trunk-mid
        // position); the right invariant to check is the polyline's
        // straight-line *length*. Each child's untrimmed length is 40
        // m; after trimming the start by ~thin_sw the length must drop
        // slightly below 40 m, but not by much.
        let child_polys: Vec<_> = polylines.iter().filter(|pts| pts.len() == 2).collect();
        assert_eq!(child_polys.len(), 3, "{svg}");
        for pts in &child_polys {
            let dx = pts[1].0 - pts[0].0;
            let dy = pts[1].1 - pts[0].1;
            let length = (dx * dx + dy * dy).sqrt();
            assert!(
                length < 40.0 - 1e-3,
                "child polyline should be shorter than 40 after trim; got {length}: {svg}",
            );
            assert!(
                length > 38.0,
                "child should only be trimmed slightly; got {length}: {svg}",
            );
        }
    }

    #[test]
    fn parent_is_not_trimmed_when_branching_strictly_higher_than_child() {
        // Sanity-check the asymmetric trim rule: when a parent has
        // strictly greater branching than the tied child, the parent's
        // polyline endpoint (even if it lands at the connection) does
        // NOT get pulled back.
        //
        // Topology: trunk (3 children) with a fourth child that is itself
        // a tiny parent of two grandchildren - so it has branching > 1
        // (it's tied to the trunk AND to 2 grandchildren) but still less
        // than the trunk. The trunk should stay full length.
        let mut project = Project::new("Asymm".to_string(), None);
        let trunk_id = project
            .add_section(SectionData {
                name: Some("Trunk".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(100.0, 90.0, false)],
                ..Default::default()
            })
            .id;
        let merged_at = chrono::Utc::now();
        // Three single-shot children dangling off the trunk's east end.
        let trunk_east = StationRef::new(trunk_id, 1);
        for az in [0.0, 90.0, 180.0] {
            let child = project
                .add_section(SectionData {
                    name: Some("Child".to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(20.0, az, false)],
                    ..Default::default()
                })
                .id;
            let stub = project
                .add_section(SectionData {
                    name: Some("Stub".to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(1.0, 0.0, true)],
                    ..Default::default()
                })
                .id;
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 0).into(),
                station_b: trunk_east.into(),
                merged_at,
            });
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 1).into(),
                station_b: StationRef::new(child, 0).into(),
                merged_at,
            });
        }

        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 3,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        // Trunk has branching 3 (tied to three children); each child
        // branching 1. The trunk's polyline ends at the connection (it's
        // a one-shot trunk). With strict-larger rule the trunk endpoint
        // should NOT be trimmed.
        let polylines = parse_polyline_points(&svg);
        let trunk_poly = polylines
            .iter()
            .find(|pts| {
                // Horizontal: both y ≈ 0.
                pts.iter().all(|(_, y)| y.abs() < 1e-3)
            })
            .unwrap_or_else(|| panic!("missing horizontal trunk polyline: {svg}"));
        assert_eq!(trunk_poly.len(), 2, "{svg}");
        assert!(
            (trunk_poly[1].0 - 100.0).abs() < 1e-3,
            "trunk east end should not be trimmed; got x={}: {svg}",
            trunk_poly[1].0,
        );
    }

    /// Helper: parse every `<polyline points="x,y x,y ..."/>` into a list
    /// of vertex lists.
    fn parse_polyline_points(svg: &str) -> Vec<Vec<(f64, f64)>> {
        let mut out = Vec::new();
        let mut rest = svg;
        let needle = "<polyline ";
        while let Some(start) = rest.find(needle) {
            let after = &rest[start + needle.len()..];
            let Some(close) = after.find('>') else { break };
            let attrs = &after[..close];
            rest = &after[close..];
            let Some(p_start) = attrs.find(r#"points=""#) else {
                continue;
            };
            let body = &attrs[p_start + 8..];
            let Some(p_end) = body.find('"') else {
                continue;
            };
            let pts: Vec<(f64, f64)> = body[..p_end]
                .split_whitespace()
                .filter_map(|tok| {
                    let (x, y) = tok.split_once(',')?;
                    Some((x.parse().ok()?, y.parse().ok()?))
                })
                .collect();
            if !pts.is_empty() {
                out.push(pts);
            }
        }
        out
    }

    #[test]
    fn reference_shots_not_drawn() {
        let project = project_with_one_ref_separator();
        let svg = generate_svg_string(&project, &ExportImageOptions::default()).unwrap();
        // Layout: real, real, REF, real, real → two runs of two shots
        // each, one polyline per run.
        let polyline_count = svg.matches("<polyline").count();
        assert_eq!(
            polyline_count, 2,
            "expected one polyline per non-ref run: {svg}"
        );
        // 5 shots total minus 1 ref = 4 non-ref shots → 4 "segment edges",
        // i.e. (3 vertices) + (3 vertices) = 6 commas in points strings.
        let comma_count = svg.matches(',').count();
        assert_eq!(
            comma_count, 6,
            "expected 6 polyline vertices in total: {svg}"
        );
        // And the ref shot itself MUST NOT appear as its own polyline or
        // line element.
        assert!(
            !svg.contains("<line "),
            "no <line> elements expected: {svg}"
        );
    }

    #[test]
    fn threshold_yields_two_distinct_widths_when_both_classes_present() {
        // Two segments separated by one ref shot. Each segment has branching
        // factor 1 (each "knows about" the other via the ref shot).
        let project = project_with_one_ref_separator();

        // threshold = 1: both segments are thick. Only one stroke-width should
        // appear in the output.
        let svg_thick = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 1,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let unique_widths = extract_unique_stroke_widths(&svg_thick);
        assert_eq!(
            unique_widths.len(),
            1,
            "threshold=1 should give one width, got {unique_widths:?}: {svg_thick}"
        );

        // threshold = 2: both segments are thin (their branching=1 < 2).
        let svg_thin = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 2,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let unique_widths_thin = extract_unique_stroke_widths(&svg_thin);
        assert_eq!(
            unique_widths_thin.len(),
            1,
            "threshold=2 should give one width, got {unique_widths_thin:?}"
        );
        // And the two outputs should use *different* widths.
        assert_ne!(unique_widths, unique_widths_thin);
    }

    fn extract_unique_stroke_widths(svg: &str) -> Vec<String> {
        // Only consider stroke widths emitted on shot <polyline> elements.
        // The pen-distance label group also carries its own (halo)
        // stroke-width on its <text> wrapper - that's unrelated to line
        // weights.
        let mut found: HashSet<String> = HashSet::new();
        for line in svg.lines() {
            let line = line.trim_start();
            if !line.starts_with("<polyline ") {
                continue;
            }
            if let Some(start) = line.find(r#"stroke-width=""#) {
                let after = &line[start + r#"stroke-width=""#.len()..];
                if let Some(end) = after.find('"') {
                    found.insert(after[..end].to_string());
                }
            }
        }
        let mut v: Vec<String> = found.into_iter().collect();
        v.sort();
        v
    }

    #[test]
    fn branching_thick_and_thin_mix() {
        // Trunk segment connected by reference shots to three leaf segments.
        // Trunk's branching factor = 3, each leaf's branching factor = 1.
        //
        // Layout: each piece is its own section. The three ref shots each
        // live in their own one-shot "stub" section, with station 0 merged
        // into the trunk's tip station and station 1 merged into the
        // leaf's start station. Putting refs in separate sections is the
        // only way to make them genuinely fan out from a single trunk
        // station - within a section, shots form a linear chain.
        let mut project = Project::new("Trunk".to_string(), None);

        let trunk_id = project
            .add_section(SectionData {
                name: Some("Trunk".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 0.0, false), make_shot(10.0, 0.0, false)],
                ..Default::default()
            })
            .id;

        let mut leaf_id = |name: &str| -> crate::models::SectionId {
            project
                .add_section(SectionData {
                    name: Some(name.to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(5.0, 90.0, false)],
                    ..Default::default()
                })
                .id
        };
        let a_id = leaf_id("LeafA");
        let b_id = leaf_id("LeafB");
        let c_id = leaf_id("LeafC");

        let mut stub_id = || -> crate::models::SectionId {
            project
                .add_section(SectionData {
                    name: Some("Stub".to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(1.0, 90.0, true)],
                    ..Default::default()
                })
                .id
        };
        let stub_a = stub_id();
        let stub_b = stub_id();
        let stub_c = stub_id();

        // Wire each stub: station 0 → trunk tip (station 2), station 1 → leaf start.
        let merged_at = chrono::Utc::now();
        let trunk_tip = StationRef::new(trunk_id, 2);
        for (stub, leaf) in [(stub_a, a_id), (stub_b, b_id), (stub_c, c_id)] {
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 0).into(),
                station_b: trunk_tip.into(),
                merged_at,
            });
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 1).into(),
                station_b: StationRef::new(leaf, 0).into(),
                merged_at,
            });
        }

        // threshold = 3: only the trunk (branching=3) is thick; leaves
        // (branching=1) are thin. So we should see TWO distinct stroke widths.
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 3,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let widths = extract_unique_stroke_widths(&svg);
        assert_eq!(
            widths.len(),
            2,
            "expected thick+thin, got {widths:?}: {svg}"
        );
    }

    #[test]
    fn isolated_segment_has_zero_branching() {
        // No reference shots at all → branching = 0 everywhere → only one
        // stroke width is emitted (thin, for default threshold 2).
        let svg = generate_svg_string(&simple_project(), &ExportImageOptions::default()).unwrap();
        let widths = extract_unique_stroke_widths(&svg);
        assert_eq!(widths.len(), 1, "{widths:?}");
    }

    #[test]
    fn ref_chain_through_free_stations_counts_as_one_tie() {
        // Two real segments connected by *two* reference shots in series,
        // routed through a reference-only intermediate section. Old behaviour
        // would have ignored the chain (neither hop touches two real
        // segments); the new tie semantics should give each segment
        // branching factor = 1.
        let mut project = Project::new("Chain".to_string(), None);

        let left_id = project
            .add_section(SectionData {
                name: Some("Left".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        let right_id = project
            .add_section(SectionData {
                name: Some("Right".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        // Two-hop reference chain. Stations 0,1,2 - only ref shots in this
        // section, so all three are "free" w.r.t. real segments.
        let chain_id = project
            .add_section(SectionData {
                name: Some("Chain".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(1.0, 90.0, true), make_shot(1.0, 90.0, true)],
                ..Default::default()
            })
            .id;
        // Hook the chain's two outer stations onto the real segments via
        // merges. Chain station 1 stays free in the middle.
        let merged_at = chrono::Utc::now();
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(chain_id, 0).into(),
            station_b: StationRef::new(left_id, 1).into(),
            merged_at,
        });
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(chain_id, 2).into(),
            station_b: StationRef::new(right_id, 1).into(),
            merged_at,
        });

        // threshold = 1 → both segments are thick (branching = 1 ≥ 1).
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 1,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let widths = extract_unique_stroke_widths(&svg);
        assert_eq!(
            widths.len(),
            1,
            "chain should tie both segments at branching=1; got widths {widths:?}: {svg}"
        );

        // And threshold = 2 → both segments are thin (branching = 1 < 2). A
        // different single width than the previous run.
        let svg_thin = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 2,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let widths_thin = extract_unique_stroke_widths(&svg_thin);
        assert_eq!(widths_thin.len(), 1, "{widths_thin:?}");
        assert_ne!(widths, widths_thin);
    }

    #[test]
    fn anchor_y_junction_does_not_cross_tie_siblings() {
        // Trunk segment with two independent direct ref shots, each leading
        // out to a different leaf segment. Topologically the two leaves are
        // *not* directly tied to one another - they only share the trunk.
        // Branching: trunk = 2, each leaf = 1. The trunk's role as the
        // shared station must not promote leaves into mutual neighbors.
        let mut project = Project::new("YAtAnchor".to_string(), None);

        let trunk_id = project
            .add_section(SectionData {
                name: Some("Trunk".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        let mut leaf_id = |name: &str| -> crate::models::SectionId {
            project
                .add_section(SectionData {
                    name: Some(name.to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(5.0, 90.0, false)],
                    ..Default::default()
                })
                .id
        };
        let left_id = leaf_id("Left");
        let right_id = leaf_id("Right");

        // Each stub is a one-ref-shot section. Station 0 → trunk tip
        // (anchor), station 1 → leaf start (anchor). Both endpoints are
        // anchors, so the tie is the direct anchor-anchor variety. No
        // free station is shared, so the two stubs do NOT cross-tie the
        // leaves.
        let merged_at = chrono::Utc::now();
        let trunk_tip = StationRef::new(trunk_id, 1);
        for leaf in [left_id, right_id] {
            let stub = project
                .add_section(SectionData {
                    name: Some("Stub".to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(1.0, 90.0, true)],
                    ..Default::default()
                })
                .id;
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 0).into(),
                station_b: trunk_tip.into(),
                merged_at,
            });
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(stub, 1).into(),
                station_b: StationRef::new(leaf, 0).into(),
                merged_at,
            });
        }

        // At threshold = 2 only the trunk (branching 2) is thick; the two
        // leaves (branching 1) are thin → two distinct stroke widths.
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 2,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let widths = extract_unique_stroke_widths(&svg);
        assert_eq!(
            widths.len(),
            2,
            "leaves must not become mutual neighbors via the shared trunk anchor; widths {widths:?}: {svg}"
        );
    }

    #[test]
    fn free_cluster_with_three_anchors_cross_ties_all_pairs() {
        // One physical free station shared by three independent spoke ref
        // shots, each terminating at a different real segment. The shared
        // free hub should turn the three segments into a clique of mutual
        // ties → branching factor 2 for every segment.
        //
        // Topology:
        //   spokeA.0 ─ref─ spokeA.1 ── merged with ──> A.0
        //   spokeB.0 ─ref─ spokeB.1 ── merged with ──> B.0
        //   spokeC.0 ─ref─ spokeC.1 ── merged with ──> C.0
        //   spokeA.0 ── merged ── spokeB.0 ── merged ── spokeC.0   (the hub)
        // Each spokeX.0 is free (its segment owns only ref shots). They
        // share a segment_uf root via merges, so free_uf groups them into
        // one cluster, and the three attached anchors form a 3-clique.
        let mut project = Project::new("FreeHub".to_string(), None);
        let mut real_seg = |name: &str| -> crate::models::SectionId {
            project
                .add_section(SectionData {
                    name: Some(name.to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(5.0, 0.0, false)],
                    ..Default::default()
                })
                .id
        };
        let a_id = real_seg("A");
        let b_id = real_seg("B");
        let c_id = real_seg("C");

        let merged_at = chrono::Utc::now();
        let mut spoke_ids: Vec<crate::models::SectionId> = Vec::new();
        for (label, leaf) in [("SpokeA", a_id), ("SpokeB", b_id), ("SpokeC", c_id)] {
            let spoke = project
                .add_section(SectionData {
                    name: Some(label.to_string()),
                    color: "#000000".to_string(),
                    shots: vec![make_shot(1.0, 0.0, true)],
                    ..Default::default()
                })
                .id;
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(spoke, 1).into(),
                station_b: StationRef::new(leaf, 0).into(),
                merged_at,
            });
            spoke_ids.push(spoke);
        }
        // Tie all three spoke-tail stations together → the shared free hub.
        for win in spoke_ids.windows(2) {
            project.station_merges.push(StationMerge {
                station_a: StationRef::new(win[0], 0).into(),
                station_b: StationRef::new(win[1], 0).into(),
                merged_at,
            });
        }

        // threshold = 2: every segment (branching = 2) ≥ threshold → all
        // shots thick. One stroke width.
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 2,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let widths = extract_unique_stroke_widths(&svg);
        assert_eq!(widths.len(), 1, "all branching=2 → single width: {svg}");
    }

    // ---- Zero-datum stars --------------------------------------------------

    #[test]
    fn zero_datum_renders_as_star_polygon() {
        let mut project = simple_project();
        let section_id = project.sections[0].id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(section_id, 0), gps)
            .unwrap();

        let svg = generate_svg_string(&project, &ExportImageOptions::default()).unwrap();
        // Exactly one polygon (the zero datum).
        assert_eq!(svg.matches("<polygon").count(), 1, "{svg}");
        // Black fill on the star group.
        assert!(svg.contains(r##"fill="#000000""##));
        // No <circle> elements emitted at all.
        assert!(!svg.contains("<circle"));
    }

    #[test]
    fn non_zero_datum_stations_emit_no_marker() {
        let svg = generate_svg_string(&simple_project(), &ExportImageOptions::default()).unwrap();
        assert!(!svg.contains("<circle"), "should not draw station circles");
        assert!(!svg.contains("<polygon"), "no zero datum, no stars: {svg}");
    }

    #[test]
    fn star_polygon_has_ten_points() {
        let pts = star_polygon_points(0.0, 0.0, 10.0, 4.0);
        // Ten "x,y" tokens separated by single spaces.
        assert_eq!(pts.split(' ').count(), 10, "{pts}");
        // First point is straight up (angle -π/2 → y = -r_outer).
        let first = pts.split(' ').next().unwrap();
        let (x, y) = first.split_once(',').unwrap();
        assert!(x.parse::<f64>().unwrap().abs() < 1e-6);
        assert!((y.parse::<f64>().unwrap() + 10.0).abs() < 1e-6);
    }

    // ---- Penetration distance labels --------------------------------------

    #[test]
    fn pen_distance_label_only_when_zero_datum_present() {
        // Without a zero datum, penetration distance is undefined → no
        // labels even though a thick segment exists.
        let mut project = project_with_one_ref_separator();
        // threshold = 1 makes both segments thick.
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 1,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains("pen-distance"),
            "no zero datum should mean no pen-distance group: {svg}"
        );
        assert!(!svg.contains("p. "), "{svg}");

        // Add a zero datum at station 0 → labels appear.
        let section_id = project.sections[0].id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(section_id, 0), gps)
            .unwrap();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 1,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(svg.contains(r#"class="pen-distance""#), "{svg}");
        assert!(svg.contains("p. "), "{svg}");
    }

    #[test]
    fn pen_distance_label_uses_shot_length_sum_to_deepest_station() {
        // One trunk segment, two real shots: 100 m then 150 m. Zero datum at
        // station 0. Branching = 0, so threshold = 0 makes the trunk thick
        // and triggers a label at the deepest station (250 m).
        let mut project = Project::new("PenDist".to_string(), None);
        let trunk_id = project
            .add_section(SectionData {
                name: Some("Trunk".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(100.0, 0.0, false), make_shot(150.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(trunk_id, 0), gps)
            .unwrap();

        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 0,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(svg.contains("p. 250"), "expected p. 250 label: {svg}");
        // Exactly one label - the trunk has only one deepest station.
        let count = svg.matches(">p. ").count();
        assert_eq!(count, 1, "{svg}");
    }

    #[test]
    fn pen_distance_label_skipped_for_thin_segments() {
        // Thin segments (below threshold) should NOT get labelled, even when
        // a zero datum exists.
        let mut project = simple_project();
        let section_id = project.sections[0].id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(section_id, 0), gps)
            .unwrap();
        // Default threshold = 2; simple_project has branching = 0 (no ref
        // shots), so the lone segment is thin → no label.
        let svg = generate_svg_string(&project, &ExportImageOptions::default()).unwrap();
        assert!(!svg.contains("pen-distance"), "{svg}");
        assert!(!svg.contains("p. "), "{svg}");
    }

    /// Helper: extract every `<text x="..." y="...">…</text>` from the SVG
    /// so directional-placement tests can inspect where labels landed.
    fn parse_text_labels(svg: &str) -> Vec<(f64, f64, String)> {
        let mut out = Vec::new();
        let mut rest = svg;
        while let Some(start) = rest.find("<text ") {
            let after = &rest[start + 6..];
            let Some(close_tag) = after.find('>') else {
                break;
            };
            let attrs = &after[..close_tag];
            let body_start = close_tag + 1;
            let Some(end) = after[body_start..].find("</text>") else {
                break;
            };
            let body = &after[body_start..body_start + end];
            let parse_attr = |name: &str| -> Option<f64> {
                let needle = format!("{name}=\"");
                let s = attrs.find(&needle)? + needle.len();
                let e = attrs[s..].find('"')? + s;
                attrs[s..e].parse().ok()
            };
            if let (Some(x), Some(y)) = (parse_attr("x"), parse_attr("y")) {
                out.push((x, y, body.to_string()));
            }
            rest = &after[body_start + end..];
        }
        out
    }

    #[test]
    fn pen_distance_label_placed_in_open_space_away_from_line() {
        // Single shot heading east (azimuth 90°, length 100). Zero datum at
        // station 0 - the deepest reachable station is station 1 at world
        // (≈100, 0). The only line direction *at station 1* points west,
        // so the open-space wedge is the entire half-plane to the east.
        // The label must therefore land east of station 1.
        let mut project = Project::new("EastShot".to_string(), None);
        let trunk_id = project
            .add_section(SectionData {
                name: Some("Trunk".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(100.0, 90.0, false)],
                ..Default::default()
            })
            .id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(trunk_id, 0), gps)
            .unwrap();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 0,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        let labels = parse_text_labels(&svg);
        let label = labels
            .iter()
            .find(|(_, _, t)| t == "p. 100")
            .unwrap_or_else(|| panic!("missing p. 100 label: {svg}"));
        // Station 1 sits at world x = 100 (azimuth 90° = east, length 100).
        // The label's x-centre must be strictly east of that, otherwise it
        // would overlap the shot line running back to station 0.
        assert!(
            label.0 > 100.0,
            "label should be east of station 1 (x=100); got x={}: {svg}",
            label.0,
        );
        // And vertically aligned with the station (|dy| < offset).
        assert!(
            label.1.abs() < 5.0,
            "label should sit on the horizontal line through the station; got y={}: {svg}",
            label.1,
        );
    }

    #[test]
    fn pen_distance_label_handles_ref_chain_to_deepest() {
        // Two real segments tied via a chain of refs through a free middle.
        // Labels measure along-cave distance (shot-length Dijkstra), so:
        //   trunk: 100 m (1 shot)         → trunk.1 deepest at "p. 100"
        //   chain: 2 × 1 m ref hops       → adds 2 m through the stub
        //   far: 50 m (1 shot)            → far.1 deepest at "p. 152"
        // Total path trunk.0 → trunk.1 → chain → far.0 → far.1 = 152.
        let mut project = Project::new("Chain".to_string(), None);
        let trunk_id = project
            .add_section(SectionData {
                name: Some("Trunk".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(100.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        let far_id = project
            .add_section(SectionData {
                name: Some("Far".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(50.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        let chain_id = project
            .add_section(SectionData {
                name: Some("Chain".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(1.0, 90.0, true), make_shot(1.0, 90.0, true)],
                ..Default::default()
            })
            .id;
        let merged_at = chrono::Utc::now();
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(chain_id, 0).into(),
            station_b: StationRef::new(trunk_id, 1).into(),
            merged_at,
        });
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(chain_id, 2).into(),
            station_b: StationRef::new(far_id, 0).into(),
            merged_at,
        });
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(trunk_id, 0), gps)
            .unwrap();

        // threshold = 1: both segments thick (branching = 1 via chain tie).
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 1,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(svg.contains("p. 100"), "expected p. 100 for trunk: {svg}");
        assert!(svg.contains("p. 152"), "expected p. 152 for far: {svg}");
    }

    // ---- Removed features ARE gone ----------------------------------------

    #[test]
    fn no_section_color_in_default_output() {
        // Color is opt-in: with the default options (color_mode None) the
        // export stays a monochrome stick map.
        let mut project = Project::new("Colored".to_string(), None);
        project.add_section(SectionData {
            name: Some("Red".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![make_shot(10.0, 0.0, false)],
            ..Default::default()
        });
        let svg = generate_svg_string(&project, &ExportImageOptions::default()).unwrap();
        assert!(
            !svg.contains("#ff0000"),
            "section color must not bleed in by default: {svg}"
        );
    }

    #[test]
    fn section_colors_only_for_raster_when_enabled() {
        let mut project = Project::new("Colored".to_string(), None);
        project.add_section(SectionData {
            name: Some("Red".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![make_shot(10.0, 0.0, false)],
            ..Default::default()
        });

        // Raster (PNG) + colors on → the shot is drawn in the section color.
        let png = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Png,
                color_mode: ColorMode::Section,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            png.contains(r##"stroke="#ff0000""##),
            "raster export with colors on should use the section color: {png}"
        );

        // SVG stays monochrome even with colors on (the raster-only invariant).
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                color_mode: ColorMode::Section,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains("#ff0000"),
            "SVG export must stay monochrome regardless of the colors toggle: {svg}"
        );
    }

    #[test]
    fn penetration_labels_toggle_off_suppresses_labels() {
        let mut project = project_with_one_ref_separator();
        let section_id = project.sections[0].id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(section_id, 0), gps)
            .unwrap();

        // Baseline: labels on → the pen-distance group is present.
        let on = generate_svg_string(
            &project,
            &ExportImageOptions {
                branching_threshold: 1,
                show_penetration_labels: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(on.contains(r#"class="pen-distance""#), "{on}");

        // Labels off → no pen-distance group and no text at all.
        let off = generate_svg_string(
            &project,
            &ExportImageOptions {
                branching_threshold: 1,
                show_penetration_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !off.contains("pen-distance"),
            "labels off should suppress the pen-distance group: {off}"
        );
        assert!(
            !off.contains("<text"),
            "labels off should emit no text: {off}"
        );
    }

    /// #27: the export has to route on the same graph the app does. Both
    /// distance call sites used the non-configurable wrappers, which hardcode
    /// reference exclusion, so with `gas.include_reference_legs` on the exported
    /// PNG/SVG showed a different penetration than the Survey panel - breaking
    /// the promise in this file's own header that the two agree.
    #[test]
    fn penetration_labels_follow_include_reference_legs() {
        // Normal - Reference - Normal, datum at station 0. Excluding the
        // reference tie strands everything past it; including it does not.
        let mut project = Project::new("RefRoute".to_string(), None);
        project.add_section(SectionData {
            name: Some("S".to_string()),
            color: "#000000".to_string(),
            shots: vec![
                make_shot(10.0, 90.0, false),
                ShotData {
                    kind: ShotKind::Reference,
                    ..make_shot(10.0, 90.0, true)
                },
                make_shot(10.0, 90.0, false),
            ],
            ..Default::default()
        });
        let section_id = project.sections[0].id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(section_id, 0), gps)
            .unwrap();

        let labels = |include: bool| {
            generate_svg_string(
                &project,
                &ExportImageOptions {
                    branching_threshold: 1,
                    show_penetration_labels: true,
                    include_reference_legs: include,
                    ..Default::default()
                },
            )
            .unwrap()
        };

        assert!(
            labels(false).contains("p. 10"),
            "excluded: the far side is unreachable, so penetration stops at the tie"
        );
        assert!(
            labels(true).contains("p. 30"),
            "included: the opt-in must restore the pre-#27 route through the tie"
        );
    }

    #[test]
    fn sanitize_hex_color_rejects_non_hex() {
        assert_eq!(sanitize_hex_color("#ff0000"), "#ff0000");
        assert_eq!(sanitize_hex_color("#abc"), "#abc");
        assert_eq!(sanitize_hex_color("red"), "#000000");
        assert_eq!(sanitize_hex_color(r#""/><script>"#), "#000000");
        assert_eq!(sanitize_hex_color("#ff00"), "#000000"); // wrong length
        assert_eq!(sanitize_hex_color("#gggggg"), "#000000"); // non-hex digits
    }

    #[test]
    fn generate_svg_string_with_stations_uses_supplied_coordinates() {
        // The commands pass the closure-adjusted solve here; the renderer must
        // draw from the supplied coordinates, not recompute the measured ones.
        let mut project = Project::new("Coords".to_string(), None);
        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 0.0, false), make_shot(10.0, 90.0, false)],
            ..Default::default()
        });
        let opts = ExportImageOptions::default();
        let measured = generate_svg_string(&project, &opts).unwrap();

        // Move one station off the measured line → a different rendered shape.
        let mut coords = project.compute_station_coordinates();
        let last = coords.len() - 1;
        coords[last].coordinates.x += 25.0;
        coords[last].coordinates.y -= 25.0;
        let shifted = generate_svg_string_with_stations(&project, &opts, coords).unwrap();

        assert_ne!(
            measured, shifted,
            "supplied station coordinates should change the rendered geometry"
        );
    }

    #[test]
    fn depth_gradient_color_matches_map_scheme() {
        let defaults = ExportImageOptions::default();
        let ramp = defaults.depth_ramp();
        // Flat range collapses to the shallow endpoint; non-finite falls back to black.
        assert_eq!(depth_gradient_color(5.0, 10.0, 10.0, ramp), "#4ecdc4");
        assert_eq!(depth_gradient_color(f64::NAN, 0.0, 1.0, ramp), "#000000");
        // Shallow (t=0) and deep (t=1) differ, and the midpoint differs from both.
        let shallow = depth_gradient_color(0.0, 0.0, 100.0, ramp);
        let deep = depth_gradient_color(100.0, 0.0, 100.0, ramp);
        let mid = depth_gradient_color(50.0, 0.0, 100.0, ramp);
        assert_ne!(shallow, deep);
        assert_ne!(mid, shallow);
        assert_ne!(mid, deep);
        // Out-of-range depths clamp to the endpoints.
        assert_eq!(depth_gradient_color(-10.0, 0.0, 100.0, ramp), shallow);
        assert_eq!(depth_gradient_color(999.0, 0.0, 100.0, ramp), deep);
    }

    #[test]
    fn depth_ramp_takes_the_themes_endpoints_and_rejects_junk() {
        let themed = ExportImageOptions {
            depth_shallow: Some("#14487F".to_string()),
            depth_deep: Some("#a52a1a".to_string()),
            ..Default::default()
        };
        let ramp = themed.depth_ramp();
        assert_eq!(ramp.shallow, "#14487F");
        assert_eq!(ramp.deep, "#a52a1a");
        // Uppercase must not merely pass the validator, it must RENDER the same:
        // passthrough alone would still look right here while `hex_to_rgb` read
        // it as black. Pin the rendered colour, not just the string.
        let lower = ExportImageOptions {
            depth_shallow: Some("#14487f".to_string()),
            ..themed.clone()
        };
        assert_eq!(
            depth_gradient_color(0.0, 0.0, 100.0, ramp),
            depth_gradient_color(0.0, 0.0, 100.0, lower.depth_ramp()),
        );
        // The whole point: a light-theme export must not paint the dark ramp.
        let defaults = ExportImageOptions::default();
        assert_ne!(
            depth_gradient_color(50.0, 0.0, 100.0, ramp),
            depth_gradient_color(50.0, 0.0, 100.0, defaults.depth_ramp()),
        );

        // Anything `hex_to_rgb` would read as black falls back per-endpoint
        // instead, so a colour this side cannot parse never blackens a survey.
        for junk in [
            "",
            "#fff",
            "rgb(1,2,3)",
            "oklch(0.7 0.1 200)",
            "4ecdc4",
            "#zzzzzz",
        ] {
            let bad = ExportImageOptions {
                depth_shallow: Some(junk.to_string()),
                depth_deep: Some("#a52a1a".to_string()),
                ..Default::default()
            };
            let ramp = bad.depth_ramp();
            assert_eq!(ramp.shallow, DEPTH_SHALLOW, "junk endpoint {junk:?} leaked");
            assert_eq!(ramp.deep, "#a52a1a", "the good endpoint was dropped too");
        }
    }

    #[test]
    fn distance_gradient_color_greys_unreachable_and_ramps() {
        // Unreachable (None) and the -1 sentinel both render grey.
        assert_eq!(distance_gradient_color(None, 100.0), "#666666");
        assert_eq!(distance_gradient_color(Some(-1.0), 100.0), "#666666");
        // Near and far are distinct, non-grey ramp colors.
        let near = distance_gradient_color(Some(0.0), 100.0);
        let far = distance_gradient_color(Some(100.0), 100.0);
        assert_ne!(near, far);
        assert_ne!(near, "#666666");
        assert_ne!(far, "#666666");
        // Degenerate max is guarded (no divide-by-zero / NaN).
        assert_eq!(distance_gradient_color(Some(0.0), 0.0), near);
    }

    #[test]
    fn depth_color_mode_colors_raster_but_svg_stays_mono() {
        let mut project = Project::new("Depth".to_string(), None);
        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 0.0, false), make_shot(10.0, 90.0, false)],
            ..Default::default()
        });
        // Raster + depth mode → shots carry the depth gradient (shallow color here,
        // since make_shot depths are flat).
        let png = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Png,
                color_mode: ColorMode::Depth,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            png.contains("#4ecdc4"),
            "depth mode should color raster: {png}"
        );
        // SVG stays monochrome regardless of the color mode.
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                color_mode: ColorMode::Depth,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains("#4ecdc4"),
            "SVG export must stay monochrome regardless of the color mode: {svg}"
        );
    }

    #[test]
    fn distance_color_mode_greys_unreachable_raster_but_svg_stays_mono() {
        // No zero datum → every station is unreachable → shots render grey.
        let mut project = Project::new("Dist".to_string(), None);
        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 0.0, false), make_shot(10.0, 90.0, false)],
            ..Default::default()
        });
        let png = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Png,
                color_mode: ColorMode::Distance,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            png.contains("#666666"),
            "distance mode should grey unreachable shots in raster: {png}"
        );
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                color_mode: ColorMode::Distance,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains("#666666"),
            "SVG export must stay monochrome regardless of the color mode: {svg}"
        );
    }

    #[test]
    fn distance_gradient_color_lerps_in_gamma_space() {
        // MapLibre's `['interpolate', ['linear'], …]` on colors uses the default
        // `rgb` space, which lerps the parsed GAMMA channels directly. The midpoint
        // of #f4d03f → #7d3c98 is ≈ #b8866c (184,134,108), NOT the linear-light
        // #c49d76 (196,157,118). Guards against regressing to a linearized lerp.
        let mid = distance_gradient_color(Some(50.0), 100.0);
        let n = u32::from_str_radix(&mid[1..7], 16).expect("valid hex");
        let (r, g, b) = ((n >> 16) as u8, (n >> 8) as u8, n as u8);
        assert!(
            (i32::from(r) - 184).abs() <= 1 && g == 134 && b == 108,
            "gamma-space midpoint expected ~#b8866c, got {mid}"
        );
        assert_ne!(mid, "#c49d76", "must not lerp in linear light");
    }

    #[test]
    fn depth_color_mode_paints_a_varying_gradient() {
        // A shot descending 0 → 100 must paint MULTIPLE distinct depth colors from
        // the per-shot subdivision - not one flat color. Guards the subdivision
        // count, the midpoint-depth formula, and the per-segment coloring.
        let mut project = Project::new("DepthGrad".to_string(), None);
        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                length: 120.0, // >= the 100 m depth change so the geometry is valid
                depth_in: 0.0,
                depth_out: 100.0,
                kind: ShotKind::Normal,
                ..Default::default()
            }],
            ..Default::default()
        });
        let png = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Png,
                color_mode: ColorMode::Depth,
                ..Default::default()
            },
        )
        .unwrap();
        // Distinct shot stroke colors, excluding the group's black default: the
        // per-shot subdivision paints a run of gradient colors, not one flat color.
        let colors: HashSet<&str> = png
            .split("stroke=\"")
            .skip(1)
            .filter_map(|s| s.get(0..7))
            .filter(|h| h.starts_with('#') && *h != "#000000")
            .collect();
        assert!(
            colors.len() >= 3,
            "depth subdivision should paint a gradient of distinct colors, got {colors:?}"
        );
    }

    #[test]
    fn distance_color_mode_ramps_reachable_shots() {
        // With a zero datum every station is reachable → shots get the gold→purple
        // ramp, none grey. Exercises the reachable `Some((a+b)/2)` avg + ramp path.
        let mut project = Project::new("DistReach".to_string(), None);
        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 0.0, false), make_shot(10.0, 0.0, false)],
            ..Default::default()
        });
        let sid = project.sections[0].id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(sid, 0), gps)
            .unwrap();
        let png = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Png,
                color_mode: ColorMode::Distance,
                ..Default::default()
            },
        )
        .unwrap();
        let has_ramp = png
            .split("stroke=\"")
            .skip(1)
            .filter_map(|s| s.get(0..7))
            .any(|h| h.starts_with('#') && h != "#000000" && h != "#666666");
        assert!(
            has_ramp,
            "reachable shots should paint a non-grey ramp color: {png}"
        );
        assert!(
            !png.contains("#666666"),
            "all stations reachable → no grey: {png}"
        );
    }

    #[test]
    fn no_labels_no_grid_no_legend() {
        // `simple_project` has no zero datum (so penetration distance is
        // undefined and no labels are emitted) and no thick segments. The
        // SVG should be pure shape geometry.
        let svg = generate_svg_string(&simple_project(), &ExportImageOptions::default()).unwrap();
        assert!(!svg.contains("<text"), "no text labels: {svg}");
        assert!(!svg.contains("class=\"grid\""));
        assert!(!svg.contains("depth-legend"));
        assert!(!svg.contains("linearGradient"));
    }

    // ---- Station labels (#240) ---------------------------------------------

    /// A straight 3-shot line: stations 0 (start), 1, 2 (interior), 3 (end).
    fn three_shot_line_project() -> Project {
        let mut project = Project::new("Line".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#000000".to_string(),
            shots: vec![
                make_shot(10.0, 90.0, false),
                make_shot(10.0, 90.0, false),
                make_shot(10.0, 90.0, false),
            ],
            ..Default::default()
        });
        project
    }

    #[test]
    fn station_labels_mark_all_stations() {
        // CHANGED semantics (#240 follow-up): station labels now cover every
        // point touched by a drawn shot (deg >= 1), including interior
        // degree-2 stations 1 and 2 -- not just endpoints/junctions.
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_labels: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="station-labels""#),
            "station-labels group should be present: {svg}"
        );
        assert!(svg.contains(">0</text>"), "start endpoint labeled: {svg}");
        assert!(
            svg.contains(">1</text>"),
            "interior station 1 labeled: {svg}"
        );
        assert!(
            svg.contains(">2</text>"),
            "interior station 2 labeled: {svg}"
        );
        assert!(svg.contains(">3</text>"), "end endpoint labeled: {svg}");
    }

    #[test]
    fn station_labels_absent_when_disabled() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="station-labels""#),
            "station-labels group should be absent when opted out: {svg}"
        );
    }

    // ---- Station names --------------------------------------------------

    /// The three-shot line with names on stations 0 and 2; 1 and 3 unnamed.
    fn named_line_project() -> Project {
        let mut project = three_shot_line_project();
        let sid = project.sections[0].id;
        project
            .station_names
            .insert(StationRef::new(sid, 0).into(), "17".to_string());
        project
            .station_names
            .insert(StationRef::new(sid, 2).into(), "19".to_string());
        project
    }

    /// Station names only (no penetration labels, so every `<text>` is a name).
    fn names_only() -> ExportImageOptions {
        ExportImageOptions {
            show_penetration_labels: false,
            show_station_names: true,
            ..Default::default()
        }
    }

    fn sorted_texts(svg: &str) -> Vec<String> {
        let mut texts: Vec<String> = parse_text_labels(svg)
            .into_iter()
            .map(|(_, _, text)| text)
            .collect();
        texts.sort();
        texts
    }

    #[test]
    fn station_names_label_named_stations_only() {
        let svg = generate_svg_string(&named_line_project(), &names_only()).unwrap();
        assert!(svg.contains(r#"class="station-names""#), "{svg}");
        assert!(!svg.contains(r#"class="station-labels""#), "{svg}");
        assert_eq!(
            sorted_texts(&svg),
            ["17", "19"],
            "only the named stations are labelled, by name, with no index fallback: {svg}"
        );
        let labels = parse_text_labels(&svg);
        let x_of = |name: &str| labels.iter().find(|(_, _, t)| t == name).unwrap().0;
        assert!(
            x_of("17") < x_of("19"),
            "each name sits at its own station (0 west of 2): {svg}"
        );
    }

    #[test]
    fn station_names_absent_when_no_station_is_named_or_disabled() {
        let svg = generate_svg_string(&three_shot_line_project(), &names_only()).unwrap();
        assert!(!svg.contains("station-names"), "{svg}");
        assert!(sorted_texts(&svg).is_empty(), "no index fallback: {svg}");

        let svg = generate_svg_string(
            &named_line_project(),
            &ExportImageOptions {
                show_penetration_labels: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!svg.contains("station-names"), "names are opt-in: {svg}");
        assert!(sorted_texts(&svg).is_empty(), "{svg}");
    }

    #[test]
    fn station_labels_win_when_both_label_options_are_set() {
        let svg = generate_svg_string(
            &named_line_project(),
            &ExportImageOptions {
                show_station_labels: true,
                ..names_only()
            },
        )
        .unwrap();
        assert!(svg.contains(r#"class="station-labels""#), "{svg}");
        assert!(!svg.contains("station-names"), "{svg}");
        assert_eq!(sorted_texts(&svg), ["0", "1", "2", "3"], "{svg}");
    }

    #[test]
    fn station_name_label_sanitizes_trims_and_caps() {
        assert_eq!(station_name_label(None), None);
        assert_eq!(station_name_label(Some("")), None);
        assert_eq!(station_name_label(Some("   ")), None);
        assert_eq!(
            station_name_label(Some("\u{202e}\n\0")),
            None,
            "a name of only controls and bidi overrides is no name"
        );
        assert_eq!(
            station_name_label(Some(" 1\u{202e}7\n ")).as_deref(),
            Some("17")
        );
        // The same rules as the live map's label: the Arabic letter mark goes,
        // and a byte-order mark is trimmed like whitespace.
        assert_eq!(
            station_name_label(Some("\u{feff}\u{061c}17\u{feff}")).as_deref(),
            Some("17")
        );
        assert_eq!(station_name_label(Some("\u{feff}")), None);

        let exact = "b".repeat(STATION_NAME_LABEL_MAX_CHARS);
        assert_eq!(
            station_name_label(Some(&exact)).as_deref(),
            Some(exact.as_str())
        );
        let capped = station_name_label(Some(&"a".repeat(40))).unwrap();
        assert_eq!(capped.chars().count(), STATION_NAME_LABEL_MAX_CHARS);
        assert!(capped.ends_with('\u{2026}'), "{capped}");
        // A cut that lands after a space does not keep the space.
        let spaced = format!("{} {}", "c".repeat(22), "d".repeat(10));
        assert_eq!(
            station_name_label(Some(&spaced)),
            Some(format!("{}\u{2026}", "c".repeat(22)))
        );
        // Multi-byte chars are counted and cut as chars, never mid-byte.
        let wide = station_name_label(Some(&"\u{17d}".repeat(30))).unwrap();
        assert_eq!(wide.chars().count(), STATION_NAME_LABEL_MAX_CHARS);
    }

    #[test]
    fn station_names_are_xml_escaped_and_parse() {
        let mut project = three_shot_line_project();
        let sid = project.sections[0].id;
        project.station_names.insert(
            StationRef::new(sid, 1).into(),
            "a & b </text><script>x".to_string(),
        );
        let svg = generate_svg_string(&project, &names_only()).unwrap();
        assert!(
            svg.contains("a &amp; b &lt;/text&gt;&lt;script&gt;x"),
            "station name must be XML-escaped: {svg}"
        );
        assert!(!svg.contains("<script>"), "{svg}");
        assert!(
            Tree::from_str(&svg, &Options::default()).is_ok(),
            "station-name SVG must parse: {svg}"
        );
    }

    #[test]
    fn station_names_use_the_named_member_of_a_merged_point() {
        // Branch:0 is merged onto Main:1. Main:1 has the lower global index but
        // no name, so the point is labelled - once - with Branch:0's name, at
        // Branch:0's own position (the representative's coordinates).
        let mut project = three_shot_line_project();
        let main_id = project.sections[0].id;
        let branch_id = project
            .add_section(SectionData {
                name: Some("Branch".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 0.0, false), make_shot(10.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(branch_id, 0).into(),
            station_b: StationRef::new(main_id, 1).into(),
            merged_at: chrono::Utc::now(),
        });
        project
            .station_names
            .insert(StationRef::new(branch_id, 0).into(), "J".to_string());

        let svg = generate_svg_string(&project, &names_only()).unwrap();
        let labels = parse_text_labels(&svg);
        assert_eq!(sorted_texts(&svg), ["J"], "{svg}");
        let (x, y, _) = &labels[0];
        assert!(
            x.abs() < 1.0 && y.abs() < 1.0,
            "the named member speaks for the point, at its own position: {svg}"
        );
    }

    #[test]
    fn station_names_render_from_supplied_stations() {
        // The export commands render the closure-adjusted stations they pass
        // in; the names must come along with them.
        let project = named_line_project();
        let stations = project.compute_station_coordinates();
        let svg = generate_svg_string_with_stations(&project, &names_only(), stations).unwrap();
        assert_eq!(sorted_texts(&svg), ["17", "19"], "{svg}");
    }

    #[test]
    fn station_labels_mark_junctions() {
        // Main is the same 3-shot line as above; a two-shot Branch is merged
        // onto Main's interior station 1, colocating that point with Branch's
        // station 0. Main is added first, so its stations occupy the lower
        // global indices; the merged point's representative (smallest global
        // index) is therefore Main's station 1 -- rendered at Main:1's world
        // position (10, 0), NOT Branch:0's own (unmerged-coordinate) position
        // (0, 0), even though station coordinates aren't geometrically
        // snapped in this (measured, non-adjusted) render. Since #240's
        // follow-up labels EVERY station (deg >= 1), asserting on the raw
        // ">1</text>" substring is no longer a clean junction proxy: Branch's
        // OWN unmerged interior station also happens to carry local index 1
        // (every section restarts numbering at 0) and renders its own,
        // separate ">1</text>" near (0, 10). So the dedup regression this
        // test guards is checked by POSITION: the merged point must render
        // exactly once, at Main:1's coordinates, and must NOT also render
        // a second time at Branch:0's own (0, 0).
        let mut project = three_shot_line_project();
        let main_id = project.sections[0].id;
        let branch_id = project
            .add_section(SectionData {
                name: Some("Branch".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 0.0, false), make_shot(10.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(branch_id, 0).into(),
            station_b: StationRef::new(main_id, 1).into(),
            merged_at: chrono::Utc::now(),
        });

        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_labels: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="station-labels""#),
            "station-labels group should be present: {svg}"
        );
        let labels = parse_text_labels(&svg);
        assert!(
            labels
                .iter()
                .any(|(x, _, text)| text == "1" && (*x - 10.0).abs() < 1.0),
            "merged point should render once, at Main:1's position (x≈10): {svg}"
        );
        assert!(
            !labels
                .iter()
                .any(|(x, y, text)| text == "1" && x.abs() < 1.0 && y.abs() < 1.0),
            "merged point must not ALSO render at Branch:0's own (unmerged) position \
             near the origin -- that would mean the point_uf dedup regressed: {svg}"
        );
        // The Branch's own free endpoint (Branch:2) is labeled separately,
        // confirming the fixture built a real second leg rather than a stub.
        // Assert by POSITION (x near 0, Branch's north-running column) so this
        // can't be satisfied by Main's own interior station 2 at x=20 -- that
        // substring-only check was vacuous once every station is labeled.
        assert!(
            labels
                .iter()
                .any(|(x, _, text)| text == "2" && x.abs() < 1.0),
            "branch free endpoint (Branch:2, x≈0) should be labeled: {svg}"
        );
    }

    #[test]
    fn station_labels_stay_black_regardless_of_color_mode() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_labels: true,
                color_mode: ColorMode::Depth,
                ..Default::default()
            },
        )
        .unwrap();
        let group_start = svg
            .find(r#"class="station-labels""#)
            .expect("station-labels group should be present");
        let group_tag_end = group_start + svg[group_start..].find('>').expect("unterminated tag");
        let group_tag = &svg[..group_tag_end];
        assert!(
            group_tag.contains(r##"fill="#000000""##),
            "station labels should stay black regardless of color_mode: {svg}"
        );
    }

    #[test]
    fn station_labels_exclude_reference_only_shots() {
        // A survey whose only connection is a reference (jump) shot has no
        // physical-passage endpoints or junctions: reference shots are
        // excluded from the degree count, so every station stays degree 0 and
        // nothing is labeled. If that exclusion regressed, both ends of the
        // jump would read as degree-1 endpoints and a group would appear.
        let mut project = Project::new("Jump".to_string(), None);
        project.add_section(SectionData {
            name: Some("Jump".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 90.0, true)],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_labels: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="station-labels""#),
            "reference-only survey should produce no station labels: {svg}"
        );
    }

    #[test]
    fn station_and_penetration_labels_share_collision_space() {
        // With a zero datum plus both label passes enabled, Pass 3
        // (penetration) and Pass 4 (station) write into the same shared
        // `placed` occupancy list that was hoisted above Pass 3. Exercise that
        // combined path: both groups render and the export does not panic on
        // the shared vector. (The other station-label tests have no zero datum,
        // so Pass 3 is inert and this interaction is otherwise untested.)
        let mut project = project_with_one_ref_separator();
        let section_id = project.sections[0].id;
        let gps = GpsCoordinates::new(45.0, -122.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(section_id, 0), gps)
            .unwrap();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                format: ImageFormat::Svg,
                dpi: 72,
                branching_threshold: 1,
                show_penetration_labels: true,
                color_mode: ColorMode::None,
                show_station_labels: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="pen-distance""#),
            "penetration labels should render: {svg}"
        );
        assert!(
            svg.contains(r#"class="station-labels""#),
            "station labels should render alongside penetration labels: {svg}"
        );
    }

    // ---- Section names, station/shot comments, shot data (#240 follow-up) --

    #[test]
    fn section_names_present_with_custom_name_and_id_fallback() {
        let mut project = Project::new("Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Trunk".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 90.0, false)],
            ..Default::default()
        });
        let unnamed_id = project
            .add_section(SectionData {
                name: None,
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 0.0, false)],
                ..Default::default()
            })
            .id;
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_section_names: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="section-names""#),
            "section-names group should be present: {svg}"
        );
        assert!(svg.contains(">Trunk</text>"), "named section: {svg}");
        assert!(
            svg.contains(&format!(">Section {unnamed_id}</text>")),
            "unnamed section falls back to \"Section {{id}}\": {svg}"
        );
    }

    #[test]
    fn section_names_absent_when_disabled() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_section_names: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="section-names""#),
            "section-names group should be absent when opted out: {svg}"
        );
    }

    #[test]
    fn section_names_skip_section_with_no_drawn_stations() {
        // A section whose only shot is a reference (jump) has zero drawn
        // stations and must be skipped entirely, not even via the
        // "Section {id}" fallback.
        let mut project = Project::new("Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Jump".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 90.0, true)],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_section_names: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="section-names""#),
            "reference-only section should produce no section-name label: {svg}"
        );
    }

    #[test]
    fn section_names_stay_black_regardless_of_color_mode() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_section_names: true,
                color_mode: ColorMode::Depth,
                ..Default::default()
            },
        )
        .unwrap();
        let group_start = svg
            .find(r#"class="section-names""#)
            .expect("section-names group should be present");
        let group_tag_end = group_start + svg[group_start..].find('>').expect("unterminated tag");
        let group_tag = &svg[..group_tag_end];
        assert!(
            group_tag.contains(r##"fill="#000000""##),
            "section names should stay black regardless of color_mode: {svg}"
        );
    }

    #[test]
    fn section_names_share_collision_space_with_station_labels() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_labels: true,
                show_section_names: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="station-labels""#),
            "station labels should render: {svg}"
        );
        assert!(
            svg.contains(r#"class="section-names""#),
            "section names should render alongside station labels: {svg}"
        );
    }

    #[test]
    fn station_comments_present_when_seeded() {
        let mut project = three_shot_line_project();
        let section_id = project.sections[0].id;
        project.station_comments.insert(
            StationRef::new(section_id, 1).into(),
            "wet crawl".to_string(),
        );
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="station-comments""#),
            "station-comments group should be present: {svg}"
        );
        assert!(svg.contains(">wet crawl</text>"), "comment text: {svg}");
    }

    #[test]
    fn user_text_labels_are_xml_escaped_and_parse() {
        // Section names, station comments, and shot comments are user free-text
        // written into SVG <text>. A raw '&' / '<' / '>' would produce invalid
        // XML (resvg/svg2pdf fail on the PNG/PDF paths) or inject sibling markup
        // into the raw .svg. Verify each sink in isolation (so the three labels
        // can't collision-drop each other) that the escaped forms appear, no
        // injected element survives, and the whole document still parses.
        let parses = |svg: &str| Tree::from_str(svg, &Options::default()).is_ok();

        // Section name.
        let mut p = Project::new("Test".to_string(), None);
        p.add_section(SectionData {
            name: Some("Sump & <Gallery>".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 90.0, false)],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &p,
            &ExportImageOptions {
                show_section_names: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains("Sump &amp; &lt;Gallery&gt;"),
            "section name must be XML-escaped: {svg}"
        );
        assert!(parses(&svg), "section-name SVG must parse: {svg}");

        // Station comment carrying a full injection payload.
        let mut p = three_shot_line_project();
        let sid = p.sections[0].id;
        p.station_comments.insert(
            StationRef::new(sid, 1).into(),
            "a & b </text><script>x".to_string(),
        );
        let svg = generate_svg_string(
            &p,
            &ExportImageOptions {
                show_station_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains("a &amp; b &lt;/text&gt;&lt;script&gt;x"),
            "station comment must be XML-escaped: {svg}"
        );
        assert!(
            !svg.contains("<script>"),
            "no injected <script> element may survive: {svg}"
        );
        assert!(parses(&svg), "station-comment SVG must parse: {svg}");

        // Shot comment.
        let mut p = Project::new("Test".to_string(), None);
        p.add_section(SectionData {
            name: Some("Trunk".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                comment: Some("tight & silty".to_string()),
                ..make_shot(10.0, 90.0, false)
            }],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &p,
            &ExportImageOptions {
                show_shot_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains("tight &amp; silty"),
            "shot comment must be XML-escaped: {svg}"
        );
        assert!(parses(&svg), "shot-comment SVG must parse: {svg}");

        // Control chars: XML-1.0-illegal C0 bytes (NUL, 0x01, VT, FF) plus the
        // non-C0 illegals U+FFFE / U+FFFF in user free-text must be dropped so the
        // document still parses; the legal tab (0x09), a Unicode noncharacter that
        // XML 1.0 DOES permit (U+FDD0), and the surrounding letters survive.
        let mut p = Project::new("Test".to_string(), None);
        p.add_section(SectionData {
            name: Some("Trunk".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                comment: Some("a\u{0}b\u{1}c\u{0B}\u{0C}\u{FFFE}\u{FFFF}\u{FDD0}\td".to_string()),
                ..make_shot(10.0, 90.0, false)
            }],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &p,
            &ExportImageOptions {
                show_shot_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains('\u{0}')
                && !svg.contains('\u{1}')
                && !svg.contains('\u{0B}')
                && !svg.contains('\u{0C}')
                && !svg.contains('\u{FFFE}')
                && !svg.contains('\u{FFFF}'),
            "XML-illegal chars (C0 + U+FFFE/U+FFFF) must be dropped: {svg:?}"
        );
        assert!(
            svg.contains("abc\u{FDD0}\td"),
            "legal chars (tab + the XML-permitted noncharacter U+FDD0) must survive: {svg:?}"
        );
        assert!(parses(&svg), "control-char SVG must parse: {svg}");
    }

    #[test]
    fn station_comments_absent_when_disabled() {
        let mut project = three_shot_line_project();
        let section_id = project.sections[0].id;
        project.station_comments.insert(
            StationRef::new(section_id, 1).into(),
            "wet crawl".to_string(),
        );
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_comments: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="station-comments""#),
            "station-comments group should be absent when opted out: {svg}"
        );
    }

    #[test]
    fn station_comments_absent_without_a_comment() {
        // No comment seeded anywhere -- the group must not appear even
        // though the toggle is on (the comment-present gate is not vacuous).
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="station-comments""#),
            "no station has a comment, group should be absent: {svg}"
        );
    }

    #[test]
    fn station_comments_exclude_reference_only_shots() {
        // A comment on a station whose only connection is a reference (jump)
        // shot must not render: the station never reaches deg >= 1 (the same
        // "on drawn geometry" guard as station labels).
        let mut project = Project::new("Jump".to_string(), None);
        let section_id = project
            .add_section(SectionData {
                name: Some("Jump".to_string()),
                color: "#000000".to_string(),
                shots: vec![make_shot(10.0, 90.0, true)],
                ..Default::default()
            })
            .id;
        project.station_comments.insert(
            StationRef::new(section_id, 0).into(),
            "orphaned".to_string(),
        );
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="station-comments""#),
            "reference-only station comment should not render: {svg}"
        );
    }

    #[test]
    fn station_comments_stay_black_regardless_of_color_mode() {
        let mut project = three_shot_line_project();
        let section_id = project.sections[0].id;
        project.station_comments.insert(
            StationRef::new(section_id, 1).into(),
            "wet crawl".to_string(),
        );
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_comments: true,
                color_mode: ColorMode::Depth,
                ..Default::default()
            },
        )
        .unwrap();
        let group_start = svg
            .find(r#"class="station-comments""#)
            .expect("station-comments group should be present");
        let group_tag_end = group_start + svg[group_start..].find('>').expect("unterminated tag");
        let group_tag = &svg[..group_tag_end];
        assert!(
            group_tag.contains(r##"fill="#000000""##),
            "station comments should stay black regardless of color_mode: {svg}"
        );
    }

    #[test]
    fn station_comments_share_collision_space_with_station_labels() {
        let mut project = three_shot_line_project();
        let section_id = project.sections[0].id;
        project.station_comments.insert(
            StationRef::new(section_id, 1).into(),
            "wet crawl".to_string(),
        );
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_labels: true,
                show_station_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="station-labels""#),
            "station labels should render: {svg}"
        );
        assert!(
            svg.contains(r#"class="station-comments""#),
            "station comments should render alongside station labels: {svg}"
        );
    }

    #[test]
    fn shot_data_exact_text_matches_live_map_format() {
        let mut project = Project::new("Test".to_string(), None);
        // azimuth_in (42.5) differs from azimuth_out (55.0) so asserting "42.5"
        // pins the label to azimuth_IN specifically -- it can't pass on
        // azimuth_out or the wraparound average_azimuth() (~48.7) the geometry
        // solver uses elsewhere.
        project.add_section(SectionData {
            name: Some("Trunk".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                azimuth_in: 42.5,
                azimuth_out: 55.0,
                length: 12.3,
                depth_in: 3.0,
                depth_out: 5.0,
                kind: ShotKind::Normal,
                ..Default::default()
            }],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_data: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="shot-data""#),
            "shot-data group should be present: {svg}"
        );
        assert!(
            svg.contains(">42.5\u{00b0} 12.3m 4.0m</text>"),
            "shot-data text should be azimuth_in/length/mean-depth to 1 decimal: {svg}"
        );
    }

    #[test]
    fn shot_data_absent_when_disabled() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_data: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="shot-data""#),
            "shot-data group should be absent when opted out: {svg}"
        );
    }

    #[test]
    fn shot_data_excludes_reference_shots() {
        let mut project = Project::new("Jump".to_string(), None);
        project.add_section(SectionData {
            name: Some("Jump".to_string()),
            color: "#000000".to_string(),
            shots: vec![make_shot(10.0, 90.0, true)],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_data: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="shot-data""#),
            "reference-only shot should produce no shot-data label: {svg}"
        );
    }

    #[test]
    fn shot_data_stays_black_regardless_of_color_mode() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_data: true,
                color_mode: ColorMode::Depth,
                ..Default::default()
            },
        )
        .unwrap();
        let group_start = svg
            .find(r#"class="shot-data""#)
            .expect("shot-data group should be present");
        let group_tag_end = group_start + svg[group_start..].find('>').expect("unterminated tag");
        let group_tag = &svg[..group_tag_end];
        assert!(
            group_tag.contains(r##"fill="#000000""##),
            "shot data should stay black regardless of color_mode: {svg}"
        );
    }

    #[test]
    fn shot_data_shares_collision_space_with_station_labels() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_station_labels: true,
                show_shot_data: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="station-labels""#),
            "station labels should render: {svg}"
        );
        assert!(
            svg.contains(r#"class="shot-data""#),
            "shot data should render alongside station labels: {svg}"
        );
    }

    #[test]
    fn shot_comments_present_when_seeded() {
        let mut project = Project::new("Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Trunk".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                comment: Some("silty".to_string()),
                ..make_shot(10.0, 90.0, false)
            }],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="shot-comments""#),
            "shot-comments group should be present: {svg}"
        );
        assert!(svg.contains(">silty</text>"), "comment text: {svg}");
    }

    #[test]
    fn shot_comments_absent_when_disabled() {
        let mut project = Project::new("Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Trunk".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                comment: Some("silty".to_string()),
                ..make_shot(10.0, 90.0, false)
            }],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_comments: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="shot-comments""#),
            "shot-comments group should be absent when opted out: {svg}"
        );
    }

    #[test]
    fn shot_comments_absent_without_a_comment() {
        let project = three_shot_line_project();
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="shot-comments""#),
            "no shot has a comment, group should be absent: {svg}"
        );
    }

    #[test]
    fn shot_comments_exclude_reference_shots() {
        let mut project = Project::new("Jump".to_string(), None);
        project.add_section(SectionData {
            name: Some("Jump".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                comment: Some("orphaned".to_string()),
                ..make_shot(10.0, 90.0, true)
            }],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !svg.contains(r#"class="shot-comments""#),
            "reference shot comment should not render: {svg}"
        );
    }

    #[test]
    fn shot_comments_stay_black_regardless_of_color_mode() {
        let mut project = Project::new("Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Trunk".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                comment: Some("silty".to_string()),
                ..make_shot(10.0, 90.0, false)
            }],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_comments: true,
                color_mode: ColorMode::Depth,
                ..Default::default()
            },
        )
        .unwrap();
        let group_start = svg
            .find(r#"class="shot-comments""#)
            .expect("shot-comments group should be present");
        let group_tag_end = group_start + svg[group_start..].find('>').expect("unterminated tag");
        let group_tag = &svg[..group_tag_end];
        assert!(
            group_tag.contains(r##"fill="#000000""##),
            "shot comments should stay black regardless of color_mode: {svg}"
        );
    }

    #[test]
    fn shot_comments_share_collision_space_with_shot_data() {
        let mut project = Project::new("Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Trunk".to_string()),
            color: "#000000".to_string(),
            shots: vec![ShotData {
                comment: Some("silty".to_string()),
                ..make_shot(10.0, 90.0, false)
            }],
            ..Default::default()
        });
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_shot_data: true,
                show_shot_comments: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            svg.contains(r#"class="shot-data""#),
            "shot data should render: {svg}"
        );
        assert!(
            svg.contains(r#"class="shot-comments""#),
            "shot comments should render alongside shot data: {svg}"
        );
    }
}
