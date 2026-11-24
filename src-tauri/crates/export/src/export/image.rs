//! Minimal black-on-white stick-map SVG export.
//!
//! Renders a survey as a stick map with two stroke widths chosen per-segment
//! by a user-controlled branching threshold. Reference/temporary shots are
//! the segmentation boundary and are not drawn. Zero-datum stations are
//! rendered as filled 5-pointed stars. Thick (trunk) segments are annotated
//! at their deepest reachable station with a penetration-distance label
//! (`p. 560`), where distance is the **along-cave** shortest path from
//! the nearest entrance datum (sum of shot lengths through the survey graph,
//! a junction being one station and aligned closures zero-cost edges) - the
//! same metric the Survey overview panel reports (the station graph's
//! `Project::penetration`), so the two agree.
//!
//! The renderer draws a model-neutral [`Sketch`] (`image_sketch`); the
//! station-graph exporter (`survey::image`) builds it and supplies the
//! distances.
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
use super::color_scale::{depth_color, distance_color, ExportColorScale, ResolvedScale};
#[cfg(test)]
use super::image_labels::STATION_NAME_LABEL_MAX_CHARS;
use super::image_labels::{point_representatives, station_name_label};
use super::image_sketch::{Measure, Sketch, SketchLine};
use crate::models::{LineId, StationRef};

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
/// else `#000000`. Line colors are user data written verbatim into the
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

/// Escapes the five XML metacharacters so user free-text (line names,
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
pub(super) fn is_xml_illegal_char(c: char) -> bool {
    let u = c as u32;
    (u < 0x20 && c != '\t' && c != '\n' && c != '\r') || u == 0xFFFE || u == 0xFFFF
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
pub(super) struct DepthRamp<'a> {
    pub(super) shallow: &'a str,
    pub(super) deep: &'a str,
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
pub(super) const DIST_UNREACHABLE: &str = "#666666";

/// Depth-gradient color for `depth` normalized over `[min_d, max_d]`, matching
/// the survey map: a 2-stop HSL shortest-hue-path lerp shallow→deep. Non-finite
/// input falls back to black; a flat range collapses to the shallow color.
pub(super) fn depth_gradient_color(
    depth: f64,
    min_d: f64,
    max_d: f64,
    ramp: DepthRamp<'_>,
) -> String {
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
pub(super) fn depth_extent(
    station_z: impl IntoIterator<Item = f64>,
    shot_depths: impl IntoIterator<Item = (f64, f64)>,
) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for z in station_z {
        min = min.min(z);
        max = max.max(z);
    }
    for (depth_in, depth_out) in shot_depths {
        min = min.min(depth_in).min(depth_out);
        max = max.max(depth_in).max(depth_out);
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
pub(super) fn distance_gradient_color(value: Option<f64>, max: f64) -> String {
    let Some(v) = value.filter(|v| v.is_finite() && *v >= 0.0) else {
        return DIST_UNREACHABLE.to_string();
    };
    let safe_max = if max > 0.0 { max } else { 1.0 };
    distance_ramp_at((v / safe_max).clamp(0.0, 1.0))
}

/// The fixed near→far distance ramp at position `t` (0..1).
pub(super) fn distance_ramp_at(t: f64) -> String {
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
    /// Each shot in its line's color.
    Line,
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
    /// How to color the shots (line / depth / distance gradient), mirroring
    /// the live survey map. Opt-in (default [`ColorMode::None`] = monochrome).
    /// **Raster-only**: SVG export is always monochrome regardless of this.
    #[serde(default)]
    pub color_mode: ColorMode,
    /// Whether to draw Station ID labels (the project-unique integer
    /// `Station::id`, never the positional index) at every station touched by
    /// at least one drawn (non-reference) shot - i.e. all points on the drawn
    /// passage, deduped by colocation (station merges / aligned closures).
    /// Opt-in (default `false`); black text + white halo, same placement
    /// machinery as the penetration labels, so labels stay monochrome
    /// regardless of `color_mode`.
    #[serde(default)]
    pub show_station_labels: bool,
    /// Whether to draw each station's name (unique within its line; every
    /// station of a normalized project has one) instead of its Station ID,
    /// with the same points and placement as `show_station_labels`. A point
    /// whose name cleans to nothing gets no label. Opt-in (default `false`).
    /// The UI keeps it exclusive with `show_station_labels`; when both arrive
    /// `true`, the Station ID labels win, so a caller that predates this
    /// option keeps the meaning it always had.
    #[serde(default)]
    pub show_station_names: bool,
    /// Whether to draw line-name labels at the centroid of each line's
    /// drawn stations. Opt-in (default `false`); black text + white halo.
    #[serde(default)]
    pub show_line_names: bool,
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
    /// The user's depth palette + Levels range (export panel). Absent = the
    /// theme ramp above over the data's full extent - the pre-palette export.
    #[serde(default)]
    #[ts(optional)]
    pub depth_scale: Option<ExportColorScale>,
    /// The user's distance palette + Levels range. Absent = the fixed
    /// near→far ramp over `0..max reachable`.
    #[serde(default)]
    #[ts(optional)]
    pub distance_scale: Option<ExportColorScale>,
    /// Lines the export leaves out: the lines of the Sections the map hides
    /// (display setting `hiddenSections`). Their shots, stations, labels,
    /// comments and closures are not drawn and do not count toward the
    /// picture's extent; distances (penetration labels, the distance ramp)
    /// are still measured along the whole survey. Absent or empty = every
    /// line, the export before Sections could be hidden.
    #[serde(default)]
    #[ts(optional)]
    pub hidden_line_ids: Option<Vec<LineId>>,
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
            show_line_names: false,
            show_station_comments: false,
            show_shot_comments: false,
            show_shot_data: false,
            include_reference_legs: false,
            depth_shallow: None,
            depth_deep: None,
            depth_scale: None,
            distance_scale: None,
            hidden_line_ids: None,
        }
    }
}

/// Maximum dimension for PNG export to prevent out-of-memory errors.
const MAX_PNG_DIMENSION: u32 = 16_384;

/// Write a rendered map to `path` in `options.format`.
pub(super) fn write_image(svg: &str, path: &Path, options: &ExportImageOptions) -> Result<()> {
    match options.format {
        ImageFormat::Svg => write_svg(svg, path),
        ImageFormat::Png => write_png(svg, path, options),
        ImageFormat::Pdf => write_pdf(svg, path),
    }
}

fn check_dpi(options: &ExportImageOptions) -> Result<()> {
    if options.dpi == 0 {
        anyhow::bail!("DPI must be at least 1");
    }
    Ok(())
}

fn write_svg(svg: &str, path: &Path) -> Result<()> {
    write_atomic(path, |w| Ok(w.write_all(svg.as_bytes())?)).context("Failed to write SVG file")?;
    tracing::info!(path = ?path, "Exported SVG");
    Ok(())
}

fn write_png(svg_content: &str, path: &Path, options: &ExportImageOptions) -> Result<()> {
    check_dpi(options)?;
    let mut opt = Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree =
        Tree::from_str(svg_content, &opt).context("Failed to parse SVG for PNG conversion")?;

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

fn write_pdf(svg_content: &str, path: &Path) -> Result<()> {
    let mut opt = Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = Tree::from_str(svg_content, &opt).context("Failed to parse SVG")?;

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

/// Draw `sketch` (what is drawn; `image_sketch`), with the
/// distances `measure` takes along the whole survey.
pub(super) fn render_svg(
    sketch: Sketch,
    measure: &dyn Measure,
    options: &ExportImageOptions,
) -> String {
    let Sketch {
        lines,
        stations,
        colocated,
        unlabelled,
    } = sketch;
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
    for line in &lines {
        for (i, shot) in line.shots.iter().enumerate() {
            if shot.kind.is_virtual() {
                continue;
            }
            let from = StationRef::new(line.id, i as u32);
            let to = StationRef::new(line.id, (i + 1) as u32);
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
    for &(a_sr, b_sr) in &colocated {
        if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
            uf.union(a, b);
        }
    }

    // Non-reference shots inside each line connect their two endpoint
    // stations into the same segment.
    let mut has_non_ref_shot: HashSet<usize> = HashSet::new();
    for line in &lines {
        for (i, shot) in line.shots.iter().enumerate() {
            let from = StationRef::new(line.id, i as u32);
            let to = StationRef::new(line.id, (i + 1) as u32);
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
    // first rule keeps merged "hub" stations across separate stub lines
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
    for line in &lines {
        for (i, shot) in line.shots.iter().enumerate() {
            if !shot.kind.is_virtual() {
                continue;
            }
            let from = StationRef::new(line.id, i as u32);
            let to = StationRef::new(line.id, (i + 1) as u32);
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
    for line in &lines {
        for (i, shot) in line.shots.iter().enumerate() {
            if !shot.kind.is_virtual() {
                continue;
            }
            let from = StationRef::new(line.id, i as u32);
            let to = StationRef::new(line.id, (i + 1) as u32);
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
    // shots (possibly a short stub line, possibly a direct anchor↔
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
    for &(a_sr, b_sr) in &colocated {
        if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
            physical_uf.union(a, b);
        }
    }
    let mut ref_tied_segs_per_phys: HashMap<usize, HashSet<usize>> = HashMap::new();
    for line in &lines {
        for (i, shot) in line.shots.iter().enumerate() {
            if !shot.kind.is_virtual() {
                continue;
            }
            let from = StationRef::new(line.id, i as u32);
            let to = StationRef::new(line.id, (i + 1) as u32);
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
    // non-reference shots in a line. Drawing per-run rather than
    // per-shot lets `stroke-linejoin="round"` smooth interior corners and
    // `stroke-linecap="round"` cover the cross-line/cross-run joins
    // (e.g. at merged stations), eliminating the butt-cap notches that
    // showed up at angled joins between consecutive `<line>` elements.
    //
    // Every shot in a single run shares the same segment_uf root (the run
    // is unioned by its own non-ref shots), so the run's stroke width is
    // well-defined by looking at any one shot's segment.
    // Shots are monochrome by default. Raster export can color them to match the
    // live survey map (SVG export always stays monochrome). Depth/Distance are
    // per-shot gradients; Line/None use the smooth run-batched polylines below.
    let mode = if matches!(options.format, ImageFormat::Svg) {
        ColorMode::None
    } else {
        options.color_mode
    };
    let depth_rng = (mode == ColorMode::Depth).then(|| {
        let shot_depths = lines
            .iter()
            .flat_map(|line| line.shots.iter().map(|s| (s.depth_in, s.depth_out)));
        depth_extent(stations.iter().map(|s| s.coordinates.z), shot_depths)
    });
    let depth_ramp = options.depth_ramp();
    let depth_scale = options.depth_scale.as_ref().map(ResolvedScale::new);
    let distance_scale = options.distance_scale.as_ref().map(ResolvedScale::new);
    let dist_field = (mode == ColorMode::Distance)
        .then(|| measure.distance_field(options.include_reference_legs));
    let _ = writeln!(
        svg,
        r##"<g class="shots" stroke="#000000" fill="none" stroke-linejoin="round" stroke-linecap="round">"##,
    );
    if matches!(mode, ColorMode::Depth | ColorMode::Distance) {
        // Per-shot gradient: mirror the live map's per-shot coloring. No junction
        // trimming here - the color variation carries the visual, not line breaks.
        for line in &lines {
            for j in 0..line.shots.len() {
                if line.shots[j].kind.is_virtual() {
                    continue;
                }
                let from_ref = StationRef::new(line.id, j as u32);
                let to_ref = StationRef::new(line.id, (j + 1) as u32);
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
                            distance_color(avg, *max, distance_scale.as_ref())
                        }
                        None => DIST_UNREACHABLE.to_string(),
                    };
                    let _ = writeln!(
                        svg,
                        r#"<polyline points="{x0:.2},{y0:.2} {x1:.2},{y1:.2}" stroke="{color}" stroke-width="{width}"/>"#,
                    );
                } else {
                    let (min_d, max_d) = depth_rng.unwrap_or((0.0, 50.0));
                    let shot = &line.shots[j];
                    // Subdivide into 4-12 segments (the map's smoothness formula),
                    // each colored at its midpoint depth.
                    let segs = ((shot.length / 2.0).ceil() as i64).clamp(4, 12) as usize;
                    for s in 0..segs {
                        let t0 = s as f64 / segs as f64;
                        let t1 = (s + 1) as f64 / segs as f64;
                        let mid = shot.depth_in
                            + (shot.depth_out - shot.depth_in) * ((s as f64 + 0.5) / segs as f64);
                        let color =
                            depth_color(mid, (min_d, max_d), depth_ramp, depth_scale.as_ref());
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
        for line in &lines {
            // Per-line stroke override (computed once); `None` inherits the
            // group's black stroke for the monochrome path.
            let line_stroke = (mode == ColorMode::Line).then(|| sanitize_hex_color(&line.color));
            let mut i = 0;
            while i < line.shots.len() {
                if line.shots[i].kind.is_virtual() {
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
                while j < line.shots.len() && !line.shots[j].kind.is_virtual() {
                    let from_ref = StationRef::new(line.id, j as u32);
                    let to_ref = StationRef::new(line.id, (j + 1) as u32);
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
                    match line_stroke {
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
        || options.show_line_names
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
    // offsets before giving up. A datum on a hidden line still measures the
    // drawn ones.
    let has_datum = measure.has_datum(!zero_datum_indices.is_empty());
    if options.show_penetration_labels && has_datum {
        // Use the project's shared along-cave penetration computation -
        // same metric as `Project::calculate_penetration` (and therefore
        // the Survey overview panel), so the map labels and the panel
        // value agree.
        let pen_dist = measure.penetration_distances(&stations, options.include_reference_legs);

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
                let primary = label_open_space_angle(si, &stations, &lines, &station_index);
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
        for &(a_sr, b_sr) in &colocated {
            if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                point_uf.union(a, b);
            }
        }

        // Degree per physical point (non-reference shot endpoints).
        let mut degree: HashMap<usize, u32> = HashMap::new();
        for line in &lines {
            for (i, shot) in line.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                let from = StationRef::new(line.id, i as u32);
                let to = StationRef::new(line.id, (i + 1) as u32);
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
        // colocation. The label is the Station ID, or with names on, the
        // station name. Representative station per point (docs/SPEC.md 4.2,
        // Labels; the map's `labelMergeGroups.ts` picks the same): among the
        // stations that have a label, one that is not its line's first
        // station, then one that is not its line's last, ties broken by the
        // line's order in the project and then the station index - so a
        // branch merged onto the main line shows the main line's name there,
        // not the branch's `0`. Every station has a label for index labels,
        // only named ones for names, so a merge of an unnamed and a named
        // station still shows the name, and a point with no named station
        // gets no label. A slot the sketch marks `unlabelled` (a loop end a
        // closure archives, RS 6) has no label in either mode, so it never
        // represents a point: an aligned closure is labelled by its kept
        // end, and an unaligned archived end's own point by nothing.
        if options.show_station_labels || show_station_names {
            // Names mode cleans each name once (it decides who represents a
            // point); index labels are formatted only for the chosen stations.
            let names: Vec<Option<String>> = if show_station_names {
                stations
                    .iter()
                    .map(|s| station_name_label(Some(s.name.as_str())))
                    .collect()
            } else {
                Vec::new()
            };
            let label_for = |i: usize| -> Option<String> {
                if show_station_names {
                    names[i].clone()
                } else {
                    // The project-unique Station ID, never the positional
                    // index (which is a session handle, not shown to users).
                    Some(stations[i].id.to_string())
                }
            };
            let rep = point_representatives(
                &lines,
                &stations,
                |i| point_uf.find(i),
                |i| {
                    !unlabelled.contains(&stations[i].station_ref)
                        && (!show_station_names || names[i].is_some())
                },
            );
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
                    let primary = label_open_space_angle(si, &stations, &lines, &station_index);
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
                let primary = label_open_space_angle(i, &stations, &lines, &station_index);
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

    // Pass 5: line-name labels at the centroid of each line's drawn
    // (non-reference) stations. Lines with zero drawn stations (all
    // reference, or empty) are skipped. Falls back to "Line {id}" when
    // the line has no user-given name.
    if options.show_line_names {
        let label_size = px(PEN_LABEL_PX);
        let halo = px(PEN_LABEL_HALO_PX);
        let offset = px(PEN_LABEL_OFFSET_PX);
        let char_w = label_size * PEN_LABEL_CHAR_WIDTH_RATIO;
        let mut opened = false;
        for line in &lines {
            let mut seen: HashSet<StationRef> = HashSet::new();
            let mut sum_x = 0.0;
            let mut sum_y = 0.0;
            let mut summed = 0u32;
            for (i, shot) in line.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                for sr in [
                    StationRef::new(line.id, i as u32),
                    StationRef::new(line.id, (i + 1) as u32),
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
            let text = line
                .name
                .clone()
                .unwrap_or_else(|| format!("Line {}", line.id));
            let text_w = char_w * text.chars().count() as f64;
            let text_h = label_size;
            let offsets = compass_offsets(offset);
            if let Some((lx, ly, bb)) = place_near(cx, cy, &offsets, text_w, text_h, &placed) {
                if !opened {
                    let _ = writeln!(
                        svg,
                        r##"<g class="line-names" font-family="sans-serif" font-size="{size:.2}" fill="#000000" stroke="#ffffff" stroke-width="{halo:.2}" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">"##,
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
    // 9-candidate budget the station/line passes get).
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
        for line in &lines {
            for (i, shot) in line.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                let from_ref = StationRef::new(line.id, i as u32);
                let to_ref = StationRef::new(line.id, (i + 1) as u32);
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
        for line in &lines {
            for (i, shot) in line.shots.iter().enumerate() {
                if shot.kind.is_virtual() {
                    continue;
                }
                let Some(text) = shot.comment.clone() else {
                    continue;
                };
                let from_ref = StationRef::new(line.id, i as u32);
                let to_ref = StationRef::new(line.id, (i + 1) as u32);
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
    svg
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
    lines: &[SketchLine],
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
    for line in lines {
        for (i, shot) in line.shots.iter().enumerate() {
            if shot.kind.is_virtual() {
                continue;
            }
            let from_ref = StationRef::new(line.id, i as u32);
            let to_ref = StationRef::new(line.id, (i + 1) as u32);
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
/// (station labels/comments, line names, shot data/comments); Pass 3
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

/// Nine candidate `(dx, dy)` offsets for a line-name label: the centroid
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

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
#[path = "image_tests.rs"]
mod tests;
