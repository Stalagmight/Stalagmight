//! The map image renderer, driven through the station-graph exporter
//! (`survey::image`): output shape, bounds, the raster and PDF writers,
//! segmentation and branching, datum stars, penetration labels, colour
//! modes, station labels and names, line names, comments and shot data.
//!
//! Ported from the 0.6.0 suite that drove the renderer through
//! `models::project`. Each survey is built with the model's own operations
//! (`Survey`): a 0.6.0 merge is a junction here, and a merge that closed a
//! loop is an aligned (`Tie`) closure, which the renderer treats alike.

use super::*;
use crate::export::survey::test_helpers::{meta, At, Survey};
use crate::export::survey::{
    export_image, generate_svg_string, generate_svg_string_with_positions,
};
use crate::models::survey::{ClosureStrategy, Project, StationPositions};
use crate::models::{GpsCoordinates, ShotData, ShotKind, StationId};
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

fn gps() -> GpsCoordinates {
    GpsCoordinates::new(45.0, -122.0, 0.0).unwrap()
}

fn simple_project() -> Project {
    let mut s = Survey::new("Test");
    s.named("Main", "#000000", vec![make_shot(10.0, 0.0, false)]);
    s.build()
}

/// A one-shot reference `Stub` line from the station at `from`, with the
/// start of `leaf` moved onto its far end: the 0.6.0 tests' "stub merged
/// into the parent at one end and into the leaf at the other".
fn stub_between(s: &mut Survey, from: At, leaf: LineId, azimuth: f64) -> LineId {
    let stub = s.named("Stub", "#000000", vec![make_shot(1.0, azimuth, true)]);
    s.merge((stub, 0), from);
    s.merge((leaf, 0), (stub, 1));
    stub
}

/// Options for the branching tests: SVG at 72 DPI, labels on, no colour,
/// no station labels, `threshold` as the branching threshold.
fn branching(threshold: u32) -> ExportImageOptions {
    ExportImageOptions {
        format: ImageFormat::Svg,
        dpi: 72,
        branching_threshold: threshold,
        show_penetration_labels: true,
        color_mode: ColorMode::None,
        show_station_labels: false,
        ..Default::default()
    }
}

/// Write `project` to a temp file with `extension` in `options.format`.
fn write_to(
    project: &Project,
    extension: &str,
    options: &ExportImageOptions,
) -> (NamedTempFile, std::path::PathBuf, Result<()>) {
    let file = NamedTempFile::new().unwrap();
    let path = file.path().with_extension(extension);
    let result = export_image(project, &path, options);
    (file, path, result)
}

/// PNG options at `dpi` (the raster tests' shared shape).
fn png_at(dpi: u32) -> ExportImageOptions {
    ExportImageOptions {
        format: ImageFormat::Png,
        dpi,
        branching_threshold: 2,
        show_penetration_labels: true,
        color_mode: ColorMode::None,
        show_station_labels: false,
        ..Default::default()
    }
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
        &Survey::new("Empty").build(),
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
    // A single-station project (one line with no shots) - the
    // only station sits at the origin, so raw extent_x == extent_y
    // == 0. Without clamping the viewBox would be 0×0 and usvg
    // would refuse to rasterise it.
    let mut s = Survey::new("Single");
    s.named("Just one station", "#000000", vec![]);
    let project = s.build();
    let svg = generate_svg_string(&project, &ExportImageOptions::default()).unwrap();
    let (_, _, w, h) = parse_viewbox(&svg);
    assert!(w > 0.0, "width should be positive: viewBox w={w}");
    assert!(h > 0.0, "height should be positive: viewBox h={h}");

    // PNG rasterisation should succeed (used to fail on zero-size).
    let (_file, path, result) = write_to(&project, "png", &png_at(72));
    result.unwrap();
    std::fs::remove_file(&path).ok();
}

#[test]
fn reference_only_stations_do_not_inflate_bounds() {
    // A small drawn passage (10 m east) with a long invisible
    // reference shot (1000 m further east). The viewBox should hug
    // the drawn 10 m line, not the 1010 m total span - otherwise
    // the actual stick map shrinks to ~1 % of the rendered area.
    let mut s = Survey::new("RefBounds");
    let main = s.named("Main", "#000000", vec![make_shot(10.0, 90.0, false)]);
    // The long ref shot starts at main's end, in the same frame.
    s.branch(
        meta("LongRef", "#000000"),
        (main, 1),
        vec![make_shot(1000.0, 90.0, true)], // invisible
    );
    let project = s.build();

    let svg = generate_svg_string(&project, &ExportImageOptions::default()).unwrap();
    let (_, _, w, _) = parse_viewbox(&svg);
    // Width should reflect just the drawn 10 m extent + padding -
    // an order of magnitude tighter than the 1000 m raw span.
    assert!(
        w < 100.0,
        "viewBox width {w} should hug the drawn 10 m line, not the 1000 m ref shot",
    );
}

/// A hidden Section's lines (`hidden_line_ids`) are not drawn: no shot,
/// station, label, comment or datum star of theirs, and the picture's
/// extent hugs what is drawn. Distances still run along the whole
/// survey: the datum on the hidden line still measures the drawn one.
#[test]
fn hidden_lines_are_not_drawn_but_still_measure_distances() {
    let mut s = Survey::new("Hidden");
    // "Far" runs 100 m east from the entrance to the start of "Main",
    // which runs 10 m further east.
    let far = s.named(
        "Far",
        "#000000",
        vec![make_shot(50.0, 90.0, false), make_shot(50.0, 90.0, false)],
    );
    let main = s.named("Main", "#000000", vec![make_shot(10.0, 90.0, false)]);
    s.datum((far, 0), gps());
    s.merge((far, 2), (main, 0));
    s.comment((far, 1), "far-note");
    s.comment((main, 1), "main-note");
    let project = s.build();
    let options = |hidden: Option<Vec<LineId>>| ExportImageOptions {
        format: ImageFormat::Svg,
        show_penetration_labels: false,
        show_line_names: true,
        show_station_comments: true,
        hidden_line_ids: hidden,
        ..Default::default()
    };

    let all = generate_svg_string(&project, &options(None)).unwrap();
    assert!(all.contains(">Far</text>"), "{all}");
    assert!(all.contains("far-note"), "{all}");
    let (_, _, all_w, _) = parse_viewbox(&all);
    assert!(all_w > 100.0, "the whole survey spans 110 m: {all_w}");

    let svg = generate_svg_string(&project, &options(Some(vec![far]))).unwrap();
    assert!(svg.contains(">Main</text>"), "{svg}");
    assert!(svg.contains("main-note"), "{svg}");
    assert!(!svg.contains(">Far</text>"), "hidden line name: {svg}");
    assert!(!svg.contains("far-note"), "hidden station comment: {svg}");
    assert!(
        !svg.contains("<polygon"),
        "the datum star sits on the hidden line: {svg}"
    );
    let (_, _, w, _) = parse_viewbox(&svg);
    assert!(w < 60.0, "the extent hugs the drawn 10 m line: {w}");
    // Main's far end is 110 m from the entrance along the survey, the
    // hidden line included.
    let measured = generate_svg_string(
        &project,
        &ExportImageOptions {
            format: ImageFormat::Svg,
            branching_threshold: 0,
            show_penetration_labels: true,
            hidden_line_ids: Some(vec![far]),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(measured.contains(">p. 110</text>"), "{measured}");

    // An empty list, or ids of lines that do not exist, hide nothing.
    let none = generate_svg_string(&project, &options(Some(vec![]))).unwrap();
    assert_eq!(none, all);
    let stale = generate_svg_string(&project, &options(Some(vec![999]))).unwrap();
    assert_eq!(stale, all);
}

#[test]
fn export_svg_writes_file() {
    let (_file, path, result) = write_to(
        &simple_project(),
        "svg",
        &ExportImageOptions {
            format: ImageFormat::Svg,
            ..Default::default()
        },
    );
    result.unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.starts_with("<?xml"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn export_png_writes_valid_file() {
    let (_file, path, result) = write_to(&simple_project(), "png", &png_at(72));
    result.unwrap();
    let contents = std::fs::read(&path).unwrap();
    assert!(contents.len() > 8);
    assert_eq!(&contents[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    std::fs::remove_file(&path).ok();
}

/// Ported from the WDIO `export-scaling` spec (#120): PNG raster dimensions
/// must scale linearly with DPI. The PNG writer uses `scale = dpi / 72`, so
/// quadrupling the DPI must (modulo 1px ceil rounding per axis) ~4x each
/// dimension. This is the one export-scaling assertion not already covered
/// by `export_png_writes_valid_file` / the SVG structure tests.
#[test]
fn export_png_dimensions_scale_with_dpi() {
    let project = simple_project();

    let png_dims = |dpi: u32| -> (u32, u32) {
        let (_file, path, result) = write_to(&project, "png", &png_at(dpi));
        result.unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        // PNG layout: 8-byte signature, 4-byte IHDR length, 4-byte "IHDR",
        // then big-endian u32 width (bytes 16..20) and height (bytes 20..24).
        // Validate the magic + length first so a future empty/partial/non-PNG
        // export fails with a clear assertion instead of an out-of-bounds panic.
        const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
        assert!(
            bytes.len() >= 24 && bytes[..8] == PNG_SIGNATURE,
            "the PNG writer did not write a valid PNG at dpi {dpi} ({} bytes)",
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
        (i64::from(w4) - 4 * i64::from(w1)).abs() <= 4,
        "width should scale ~4x with dpi: 72->{w1}, 288->{w4}"
    );
    assert!(
        (i64::from(h4) - 4 * i64::from(h1)).abs() <= 4,
        "height should scale ~4x with dpi: 72->{h1}, 288->{h4}"
    );
}

#[test]
fn export_pdf_writes_nonempty_file() {
    let (_file, path, result) = write_to(
        &simple_project(),
        "pdf",
        &ExportImageOptions {
            format: ImageFormat::Pdf,
            ..Default::default()
        },
    );
    result.unwrap();
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len > 0);
    std::fs::remove_file(&path).ok();
}

#[test]
fn png_oversized_dpi_errors() {
    let (_file, _path, result) = write_to(&simple_project(), "png", &png_at(2000));
    let err = result.unwrap_err().to_string();
    assert!(err.contains("16384"), "{err}");
}

#[test]
fn png_dpi_zero_errors() {
    let (_file, _path, result) = write_to(&simple_project(), "png", &png_at(0));
    let err = result.unwrap_err().to_string();
    assert!(err.contains("DPI"), "{err}");
}

// ---- Segmentation / branching -----------------------------------------

/// Build a survey: one line with five shots; shot index 2 is a
/// reference shot. That splits the chain into two segments:
///   segment A = shots 0,1 (between stations 0..=2)
///   segment B = shots 3,4 (between stations 3..=5)
fn ref_separator_survey() -> (Survey, LineId) {
    let mut s = Survey::new("RefSplit");
    let line = s.named(
        "S",
        "#000000",
        vec![
            make_shot(10.0, 0.0, false),  // 0 -> 1
            make_shot(10.0, 90.0, false), // 1 -> 2
            make_shot(10.0, 90.0, true),  // 2 -> 3  (reference)
            make_shot(10.0, 0.0, false),  // 3 -> 4
            make_shot(10.0, 90.0, false), // 4 -> 5
        ],
    );
    (s, line)
}

fn project_with_one_ref_separator() -> Project {
    ref_separator_survey().0.build()
}

/// [`project_with_one_ref_separator`] with a zero datum at its station 0.
fn ref_separator_with_datum() -> Project {
    let (mut s, line) = ref_separator_survey();
    s.datum((line, 0), gps());
    s.build()
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
    // consume the entire side polyline. The second tie closes a
    // loop, so on the station graph it is an aligned closure.
    let mut s = Survey::new("ShortSide");
    let parent = s.named("Parent", "#000000", vec![make_shot(100.0, 90.0, false)]);
    let side = s.named("ShortSide", "#000000", vec![make_shot(1.0, 0.0, false)]); // 1 m only
    stub_between(&mut s, (parent, 0), side, 0.0);
    let stub = s.named("Stub", "#000000", vec![make_shot(1.0, 0.0, true)]);
    s.merge((stub, 0), (parent, 1));
    s.closure((stub, 1), (side, 1), ClosureStrategy::Tie);
    let project = s.build();

    let svg = generate_svg_string(&project, &branching(1)).unwrap();
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
    let mut s = Survey::new("ParentChild");
    let trunk = s.named(
        "Trunk",
        "#000000",
        vec![make_shot(50.0, 90.0, false), make_shot(50.0, 90.0, false)],
    );
    let child_x = s.named("ChildX", "#000000", vec![make_shot(40.0, 0.0, false)]); // north
    let child_y = s.named("ChildY", "#000000", vec![make_shot(40.0, 180.0, false)]); // south
    let child_z = s.named("ChildZ", "#000000", vec![make_shot(40.0, 45.0, false)]); // NE

    // Stub line per child: one ref shot from the trunk's middle station
    // to the child's start.
    for child in [child_x, child_y, child_z] {
        stub_between(&mut s, (trunk, 1), child, 0.0);
    }
    let project = s.build();

    let svg = generate_svg_string(&project, &branching(3)).unwrap(); // trunk thick, children thin
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

    // Each child polyline has 2 vertices: a "start" vertex (on the stub's
    // far end) and an "end" vertex 40 m away in the shot direction. The
    // right invariant to check is the polyline's straight-line *length*.
    // Each child's untrimmed length is 40 m; after trimming the start by
    // ~thin_sw the length must drop slightly below 40 m, but not by much.
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
    let mut s = Survey::new("Asymm");
    let trunk = s.named("Trunk", "#000000", vec![make_shot(100.0, 90.0, false)]);
    // Three single-shot children dangling off the trunk's east end.
    for az in [0.0, 90.0, 180.0] {
        let child = s.named("Child", "#000000", vec![make_shot(20.0, az, false)]);
        stub_between(&mut s, (trunk, 1), child, 0.0);
    }
    let project = s.build();

    let svg = generate_svg_string(&project, &branching(3)).unwrap();
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
    let svg_thick = generate_svg_string(&project, &branching(1)).unwrap();
    let unique_widths = extract_unique_stroke_widths(&svg_thick);
    assert_eq!(
        unique_widths.len(),
        1,
        "threshold=1 should give one width, got {unique_widths:?}: {svg_thick}"
    );

    // threshold = 2: both segments are thin (their branching=1 < 2).
    let svg_thin = generate_svg_string(&project, &branching(2)).unwrap();
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
    // stroke-width on its <text> wrapper - that's unrelated to stroke
    // weights.
    let mut found: HashSet<String> = HashSet::new();
    for svg_line in svg.lines() {
        let svg_line = svg_line.trim_start();
        if !svg_line.starts_with("<polyline ") {
            continue;
        }
        if let Some(start) = svg_line.find(r#"stroke-width=""#) {
            let after = &svg_line[start + r#"stroke-width=""#.len()..];
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
    // Layout: each piece is its own line. The three ref shots each
    // live in their own one-shot "stub" line, from the trunk's tip
    // station to the leaf's start station. Putting refs in separate
    // lines is the only way to make them genuinely fan out from a
    // single trunk station - within a line, shots form a linear chain.
    let mut s = Survey::new("Trunk");
    let trunk = s.named(
        "Trunk",
        "#000000",
        vec![make_shot(10.0, 0.0, false), make_shot(10.0, 0.0, false)],
    );
    let leaves: Vec<LineId> = ["LeafA", "LeafB", "LeafC"]
        .into_iter()
        .map(|name| s.named(name, "#000000", vec![make_shot(5.0, 90.0, false)]))
        .collect();
    for leaf in leaves {
        stub_between(&mut s, (trunk, 2), leaf, 90.0);
    }
    let project = s.build();

    // threshold = 3: only the trunk (branching=3) is thick; leaves
    // (branching=1) are thin. So we should see TWO distinct stroke widths.
    let svg = generate_svg_string(&project, &branching(3)).unwrap();
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

/// Two real segments joined through a two-hop reference chain (`Chain`,
/// whose middle station is on no real segment): from `left`'s end, through
/// the chain, to `right`'s end.
fn chain_survey(name: &str, left: (&str, f64), right: (&str, f64)) -> (Survey, LineId) {
    let mut s = Survey::new(name);
    let left_id = s.named(left.0, "#000000", vec![make_shot(left.1, 0.0, false)]);
    let right_id = s.named(right.0, "#000000", vec![make_shot(right.1, 0.0, false)]);
    let chain = s.named(
        "Chain",
        "#000000",
        vec![make_shot(1.0, 90.0, true), make_shot(1.0, 90.0, true)],
    );
    s.merge((chain, 0), (left_id, 1));
    s.merge((right_id, 1), (chain, 2));
    (s, left_id)
}

#[test]
fn ref_chain_through_free_stations_counts_as_one_tie() {
    // Two real segments connected by *two* reference shots in series,
    // routed through a reference-only intermediate line. Old behaviour
    // would have ignored the chain (neither hop touches two real
    // segments); the new tie semantics should give each segment
    // branching factor = 1.
    let project = chain_survey("Chain", ("Left", 10.0), ("Right", 10.0))
        .0
        .build();

    // threshold = 1 → both segments are thick (branching = 1 ≥ 1).
    let svg = generate_svg_string(&project, &branching(1)).unwrap();
    let widths = extract_unique_stroke_widths(&svg);
    assert_eq!(
        widths.len(),
        1,
        "chain should tie both segments at branching=1; got widths {widths:?}: {svg}"
    );

    // And threshold = 2 → both segments are thin (branching = 1 < 2). A
    // different single width than the previous run.
    let svg_thin = generate_svg_string(&project, &branching(2)).unwrap();
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
    let mut s = Survey::new("YAtAnchor");
    let trunk = s.named("Trunk", "#000000", vec![make_shot(10.0, 0.0, false)]);
    let left = s.named("Left", "#000000", vec![make_shot(5.0, 90.0, false)]);
    let right = s.named("Right", "#000000", vec![make_shot(5.0, 90.0, false)]);

    // Each stub is a one-ref-shot line from the trunk tip (anchor) to a
    // leaf start (anchor). Both endpoints are anchors, so the tie is the
    // direct anchor-anchor variety. No free station is shared, so the two
    // stubs do NOT cross-tie the leaves.
    for leaf in [left, right] {
        stub_between(&mut s, (trunk, 1), leaf, 90.0);
    }
    let project = s.build();

    // At threshold = 2 only the trunk (branching 2) is thick; the two
    // leaves (branching 1) are thin → two distinct stroke widths.
    let svg = generate_svg_string(&project, &branching(2)).unwrap();
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
    //   spokeA.0 ─ref─ spokeA.1 = A.0
    //   spokeB.0 ─ref─ spokeB.1 = B.0
    //   spokeC.0 ─ref─ spokeC.1 = C.0
    //   spokeA.0 = spokeB.0 = spokeC.0   (the hub, one station)
    // The hub is free (its lines own only ref shots), and the three
    // attached anchors form a 3-clique.
    let mut s = Survey::new("FreeHub");
    let segments: Vec<LineId> = ["A", "B", "C"]
        .into_iter()
        .map(|name| s.named(name, "#000000", vec![make_shot(5.0, 0.0, false)]))
        .collect();
    let mut spokes: Vec<LineId> = Vec::new();
    for (label, leaf) in ["SpokeA", "SpokeB", "SpokeC"].into_iter().zip(segments) {
        let spoke = s.named(label, "#000000", vec![make_shot(1.0, 0.0, true)]);
        s.merge((leaf, 0), (spoke, 1));
        spokes.push(spoke);
    }
    // Join the three spoke tails into the shared free hub.
    for win in spokes.windows(2) {
        s.merge((win[1], 0), (win[0], 0));
    }
    let project = s.build();

    // threshold = 2: every segment (branching = 2) ≥ threshold → all
    // shots thick. One stroke width.
    let svg = generate_svg_string(&project, &branching(2)).unwrap();
    let widths = extract_unique_stroke_widths(&svg);
    assert_eq!(widths.len(), 1, "all branching=2 → single width: {svg}");
}

// ---- Zero-datum stars --------------------------------------------------

#[test]
fn zero_datum_renders_as_star_polygon() {
    let mut s = Survey::new("Test");
    let line = s.named("Main", "#000000", vec![make_shot(10.0, 0.0, false)]);
    s.datum((line, 0), gps());
    let project = s.build();

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
    // threshold = 1 makes both segments thick.
    let svg = generate_svg_string(&project_with_one_ref_separator(), &branching(1)).unwrap();
    assert!(
        !svg.contains("pen-distance"),
        "no zero datum should mean no pen-distance group: {svg}"
    );
    assert!(!svg.contains("p. "), "{svg}");

    // A zero datum at station 0 → labels appear.
    let svg = generate_svg_string(&ref_separator_with_datum(), &branching(1)).unwrap();
    assert!(svg.contains(r#"class="pen-distance""#), "{svg}");
    assert!(svg.contains("p. "), "{svg}");
}

#[test]
fn pen_distance_label_uses_shot_length_sum_to_deepest_station() {
    // One trunk segment, two real shots: 100 m then 150 m. Zero datum at
    // station 0. Branching = 0, so threshold = 0 makes the trunk thick
    // and triggers a label at the deepest station (250 m).
    let mut s = Survey::new("PenDist");
    let trunk = s.named(
        "Trunk",
        "#000000",
        vec![make_shot(100.0, 0.0, false), make_shot(150.0, 0.0, false)],
    );
    s.datum((trunk, 0), gps());
    let project = s.build();

    let svg = generate_svg_string(&project, &branching(0)).unwrap();
    assert!(svg.contains("p. 250"), "expected p. 250 label: {svg}");
    // Exactly one label - the trunk has only one deepest station.
    let count = svg.matches(">p. ").count();
    assert_eq!(count, 1, "{svg}");
}

#[test]
fn pen_distance_label_skipped_for_thin_segments() {
    // Thin segments (below threshold) should NOT get labelled, even when
    // a zero datum exists.
    let mut s = Survey::new("Test");
    let line = s.named("Main", "#000000", vec![make_shot(10.0, 0.0, false)]);
    s.datum((line, 0), gps());
    let project = s.build();
    // Default threshold = 2; the lone segment has branching = 0 (no ref
    // shots), so it is thin → no label.
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
    let mut s = Survey::new("EastShot");
    let trunk = s.named("Trunk", "#000000", vec![make_shot(100.0, 90.0, false)]);
    s.datum((trunk, 0), gps());
    let project = s.build();
    let svg = generate_svg_string(&project, &branching(0)).unwrap();
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
    // Total path trunk.0 → trunk.1 → chain → far.1 → far.0 = 152.
    let (mut s, trunk) = chain_survey("Chain", ("Trunk", 100.0), ("Far", 50.0));
    s.datum((trunk, 0), gps());
    let project = s.build();

    // threshold = 1: both segments thick (branching = 1 via chain tie).
    let svg = generate_svg_string(&project, &branching(1)).unwrap();
    assert!(svg.contains("p. 100"), "expected p. 100 for trunk: {svg}");
    assert!(svg.contains("p. 152"), "expected p. 152 for far: {svg}");
}

// ---- Removed features ARE gone ----------------------------------------

/// One `Red` line, coloured `#ff0000`.
fn red_line_project() -> Project {
    let mut s = Survey::new("Colored");
    s.named("Red", "#ff0000", vec![make_shot(10.0, 0.0, false)]);
    s.build()
}

#[test]
fn no_line_color_in_default_output() {
    // Color is opt-in: with the default options (color_mode None) the
    // export stays a monochrome stick map.
    let svg = generate_svg_string(&red_line_project(), &ExportImageOptions::default()).unwrap();
    assert!(
        !svg.contains("#ff0000"),
        "line color must not bleed in by default: {svg}"
    );
}

#[test]
fn line_colors_only_for_raster_when_enabled() {
    let project = red_line_project();

    // Raster (PNG) + colors on → the shot is drawn in the line color.
    let png = generate_svg_string(
        &project,
        &ExportImageOptions {
            format: ImageFormat::Png,
            color_mode: ColorMode::Line,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        png.contains(r##"stroke="#ff0000""##),
        "raster export with colors on should use the line color: {png}"
    );

    // SVG stays monochrome even with colors on (the raster-only invariant).
    let svg = generate_svg_string(
        &project,
        &ExportImageOptions {
            format: ImageFormat::Svg,
            color_mode: ColorMode::Line,
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
    let project = ref_separator_with_datum();

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
    let mut s = Survey::new("RefRoute");
    let line = s.named(
        "S",
        "#000000",
        vec![
            make_shot(10.0, 90.0, false),
            ShotData {
                kind: ShotKind::Reference,
                ..make_shot(10.0, 90.0, true)
            },
            make_shot(10.0, 90.0, false),
        ],
    );
    s.datum((line, 0), gps());
    let project = s.build();

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

/// Positions that differ from a survey's measured layout: the `moved`
/// stations shifted by `by`.
struct Shifted<'a> {
    base: &'a crate::models::survey::RawPositions,
    moved: Vec<StationId>,
    by: [f64; 2],
}

impl StationPositions for Shifted<'_> {
    fn station_xy(&self, station: StationId) -> Option<[f64; 2]> {
        let [x, y] = self.base.xy(station)?;
        Some(if self.moved.contains(&station) {
            [x + self.by[0], y + self.by[1]]
        } else {
            [x, y]
        })
    }

    fn station_z(&self, station: StationId) -> Option<f64> {
        self.base.z(station)
    }
}

#[test]
fn generate_svg_string_with_positions_uses_supplied_coordinates() {
    // The commands pass the Closures Computed View here; the renderer must
    // draw from the supplied positions, not recompute the measured ones.
    let mut s = Survey::new("Coords");
    let line = s.named(
        "A",
        "#000000",
        vec![make_shot(10.0, 0.0, false), make_shot(10.0, 90.0, false)],
    );
    let last = s.station(line, 2);
    let project = s.build();
    let opts = ExportImageOptions::default();
    let measured = generate_svg_string(&project, &opts).unwrap();

    // Move one station off the measured line → a different rendered shape.
    let raw = project.raw_positions();
    let moved = Shifted {
        base: &raw,
        moved: vec![last],
        by: [25.0, -25.0],
    };
    let shifted = generate_svg_string_with_positions(&project, &opts, &moved, &raw).unwrap();

    assert_ne!(
        measured, shifted,
        "supplied station positions should change the rendered geometry"
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

/// A two-shot line `A`, north then east, flat.
fn two_shot_project(name: &str) -> (Survey, LineId) {
    let mut s = Survey::new(name);
    let line = s.named(
        "A",
        "#000000",
        vec![make_shot(10.0, 0.0, false), make_shot(10.0, 90.0, false)],
    );
    (s, line)
}

#[test]
fn depth_color_mode_colors_raster_but_svg_stays_mono() {
    let project = two_shot_project("Depth").0.build();
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
    let project = two_shot_project("Dist").0.build();
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
    let mut s = Survey::new("DepthGrad");
    s.named(
        "A",
        "#000000",
        vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 120.0, // >= the 100 m depth change so the geometry is valid
            depth_in: 0.0,
            depth_out: 100.0,
            kind: ShotKind::Normal,
            ..Default::default()
        }],
    );
    let project = s.build();
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
    let mut s = Survey::new("DistReach");
    let line = s.named(
        "A",
        "#000000",
        vec![make_shot(10.0, 0.0, false), make_shot(10.0, 0.0, false)],
    );
    s.datum((line, 0), gps());
    let project = s.build();
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

/// A straight 3-shot line `Main`: stations 0 (start), 1, 2 (interior),
/// 3 (end), named and numbered 0..=3.
fn three_shot_line_survey() -> (Survey, LineId) {
    let mut s = Survey::new("Line");
    let line = s.named(
        "Main",
        "#000000",
        vec![
            make_shot(10.0, 90.0, false),
            make_shot(10.0, 90.0, false),
            make_shot(10.0, 90.0, false),
        ],
    );
    (s, line)
}

fn three_shot_line_project() -> Project {
    three_shot_line_survey().0.build()
}

/// [`three_shot_line_project`] with a comment on its station 1.
fn commented_line_project() -> Project {
    let (mut s, line) = three_shot_line_survey();
    s.comment((line, 1), "wet crawl");
    s.build()
}

/// The group tag of `class` in `svg` (everything up to its `>`).
fn group_tag<'a>(svg: &'a str, class: &str) -> &'a str {
    let group_start = svg
        .find(&format!(r#"class="{class}""#))
        .unwrap_or_else(|| panic!("{class} group should be present: {svg}"));
    let group_tag_end = group_start + svg[group_start..].find('>').expect("unterminated tag");
    &svg[..group_tag_end]
}

#[test]
fn station_labels_mark_all_stations() {
    // CHANGED semantics (#240 follow-up): station labels now cover every
    // point touched by a drawn shot (deg >= 1), including interior
    // degree-2 stations 1 and 2 -- not just endpoints/junctions. The label
    // is the Station ID, 0..=3 along the line.
    let svg = generate_svg_string(
        &three_shot_line_project(),
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
    let svg = generate_svg_string(
        &three_shot_line_project(),
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

/// The three-shot line with stations 0 and 2 renamed `17` and `19` (1 and
/// 3 keep their seed names).
fn named_line_project() -> Project {
    let (mut s, line) = three_shot_line_survey();
    s.rename((line, 0), "17");
    s.rename((line, 2), "19");
    s.build()
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

/// Every station on the station graph has a name (a station the user never
/// named keeps its seed name), so the names label marks every drawn station,
/// by name, never by Station ID.
#[test]
fn station_names_label_every_station_by_its_name() {
    let svg = generate_svg_string(&named_line_project(), &names_only()).unwrap();
    assert!(svg.contains(r#"class="station-names""#), "{svg}");
    assert!(!svg.contains(r#"class="station-labels""#), "{svg}");
    assert_eq!(
        sorted_texts(&svg),
        ["1", "17", "19", "3"],
        "every station labelled by its name, not its Station ID (0..=3): {svg}"
    );
    let labels = parse_text_labels(&svg);
    let x_of = |name: &str| labels.iter().find(|(_, _, t)| t == name).unwrap().0;
    assert!(
        x_of("17") < x_of("19"),
        "each name sits at its own station (0 west of 2): {svg}"
    );
}

#[test]
fn station_names_are_opt_in() {
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
    // XML-illegal noncharacters would leave an empty <text> that still
    // reserves space: dropped, and a name of only those is no name.
    assert_eq!(station_name_label(Some("\u{fffe}\u{ffff}")), None);
    assert_eq!(
        station_name_label(Some("1\u{fffe}7")).as_deref(),
        Some("17")
    );
    // A byte-order mark right before the cut is trimmed there too.
    assert_eq!(
        station_name_label(Some(&format!(
            "{}\u{feff}{}",
            "c".repeat(22),
            "d".repeat(10)
        ))),
        Some(format!("{}\u{2026}", "c".repeat(22)))
    );

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
    let (mut s, line) = three_shot_line_survey();
    s.rename((line, 1), "a & b </text><script>x");
    let svg = generate_svg_string(&s.build(), &names_only()).unwrap();
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

/// Main (the three-shot line, `1` renamed `Junction`) and a two-shot
/// Branch north from Main's station `main_at` (its start IS that station,
/// one name and one Station ID); `branch_first` lists Branch first.
fn junction_project(main_at: u32, branch_first: bool) -> Project {
    let (mut s, main) = three_shot_line_survey();
    s.rename((main, 1), "Junction");
    s.branch(
        meta("Branch", "#000000"),
        (main, main_at),
        vec![make_shot(10.0, 0.0, false), make_shot(10.0, 0.0, false)],
    );
    let mut project = s.build();
    if branch_first {
        project.lines_mut().reverse();
    }
    project
}

/// A junction is one station: it is labelled once, with its one name, at
/// its one position, whatever the line order (0.6.0 had to pick which
/// merged member spoke for the point).
#[test]
fn a_junction_is_labelled_once_by_its_name() {
    for branch_first in [false, true] {
        let project = junction_project(1, branch_first);
        let svg = generate_svg_string(&project, &names_only()).unwrap();
        assert_eq!(
            sorted_texts(&svg),
            ["0", "1", "2", "2", "3", "Junction"],
            "Main 0 Junction 2 3 + Branch 1 2 (its start is the junction): {svg}"
        );
        let labels = parse_text_labels(&svg);
        let (x, _, _) = labels.iter().find(|(_, _, t)| t == "Junction").unwrap();
        assert!(
            (x - 10.0).abs() < 1.0,
            "the junction sits at Main:1 (x = 10): {svg}"
        );
        // The Station IDs label draws the junction's one id once.
        let svg = generate_svg_string(
            &project,
            &ExportImageOptions {
                show_penetration_labels: false,
                show_station_labels: true,
                ..Default::default()
            },
        )
        .unwrap();
        let texts = sorted_texts(&svg);
        assert_eq!(texts.len(), 6, "{svg}");
        assert_eq!(texts.iter().filter(|t| *t == "1").count(), 1, "{svg}");
    }
}

#[test]
fn a_line_ending_junction_is_labelled_once() {
    // Branch starts on Main's last station `3`, listed first: the point is
    // labelled once with its name.
    let project = junction_project(3, true);
    let svg = generate_svg_string(&project, &names_only()).unwrap();
    assert_eq!(
        sorted_texts(&svg),
        ["0", "1", "2", "2", "3", "Junction"],
        "{svg}"
    );
}

#[test]
fn station_names_render_from_supplied_positions() {
    // The export commands render the Computed View they pass in; the names
    // must come along with it.
    let project = named_line_project();
    let raw = project.raw_positions();
    let svg = generate_svg_string_with_positions(&project, &names_only(), &raw, &raw).unwrap();
    assert_eq!(sorted_texts(&svg), ["1", "17", "19", "3"], "{svg}");
}

#[test]
fn station_labels_mark_junctions() {
    // Main is the same 3-shot line as above; a two-shot Branch runs north
    // from Main's interior station 1, which is one station of both lines
    // (Station IDs: Main 0..=3, Branch 4..=5). The junction renders exactly
    // once, at Main:1's world position (10, 0).
    let (mut s, main) = three_shot_line_survey();
    s.branch(
        meta("Branch", "#000000"),
        (main, 1),
        vec![make_shot(10.0, 0.0, false), make_shot(10.0, 0.0, false)],
    );
    let project = s.build();

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
    let junction: Vec<_> = labels.iter().filter(|(_, _, text)| text == "1").collect();
    assert_eq!(junction.len(), 1, "the junction renders once: {svg}");
    assert!(
        (junction[0].0 - 10.0).abs() < 1.0,
        "the junction renders at Main:1's position (x≈10): {svg}"
    );
    // The Branch's own free endpoint (Branch:2, Station ID 5) is labeled
    // separately, at the end of its north-running column above the
    // junction (x≈10), not at Main's interior station 2 (x = 20).
    assert!(
        labels
            .iter()
            .any(|(x, _, text)| text == "5" && (x - 10.0).abs() < 1.0),
        "branch free endpoint (Branch:2, Station ID 5, x≈10) should be labeled: {svg}"
    );
}

#[test]
fn station_labels_stay_black_regardless_of_color_mode() {
    let svg = generate_svg_string(
        &three_shot_line_project(),
        &ExportImageOptions {
            show_station_labels: true,
            color_mode: ColorMode::Depth,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        group_tag(&svg, "station-labels").contains(r##"fill="#000000""##),
        "station labels should stay black regardless of color_mode: {svg}"
    );
}

/// A survey whose only line is one reference (jump) shot `Jump`, the
/// shot's comment `comment`, a station comment `station_comment` on its
/// start.
fn jump_only_project(comment: Option<&str>, station_comment: Option<&str>) -> Project {
    let mut s = Survey::new("Jump");
    let line = s.named(
        "Jump",
        "#000000",
        vec![ShotData {
            comment: comment.map(str::to_string),
            ..make_shot(10.0, 90.0, true)
        }],
    );
    if let Some(text) = station_comment {
        s.comment((line, 0), text);
    }
    s.build()
}

#[test]
fn station_labels_exclude_reference_only_shots() {
    // A survey whose only connection is a reference (jump) shot has no
    // physical-passage endpoints or junctions: reference shots are
    // excluded from the degree count, so every station stays degree 0 and
    // nothing is labeled. If that exclusion regressed, both ends of the
    // jump would read as degree-1 endpoints and a group would appear.
    let svg = generate_svg_string(
        &jump_only_project(None, None),
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
    let svg = generate_svg_string(
        &ref_separator_with_datum(),
        &ExportImageOptions {
            show_station_labels: true,
            ..branching(1)
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

// ---- Line names, station/shot comments, shot data (#240 follow-up) --

#[test]
fn line_names_present_with_custom_name_and_id_fallback() {
    let mut s = Survey::new("Test");
    s.named("Trunk", "#000000", vec![make_shot(10.0, 90.0, false)]);
    let unnamed_id = s.line(
        crate::models::survey::LineMeta {
            color: "#000000".to_string(),
            ..Default::default()
        },
        vec![make_shot(10.0, 0.0, false)],
    );
    let svg = generate_svg_string(
        &s.build(),
        &ExportImageOptions {
            show_line_names: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        svg.contains(r#"class="line-names""#),
        "line-names group should be present: {svg}"
    );
    assert!(svg.contains(">Trunk</text>"), "named line: {svg}");
    assert!(
        svg.contains(&format!(">Line {unnamed_id}</text>")),
        "unnamed line falls back to \"Line {{id}}\": {svg}"
    );
}

#[test]
fn line_names_absent_when_disabled() {
    let svg = generate_svg_string(
        &three_shot_line_project(),
        &ExportImageOptions {
            show_line_names: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !svg.contains(r#"class="line-names""#),
        "line-names group should be absent when opted out: {svg}"
    );
}

#[test]
fn line_names_skip_line_with_no_drawn_stations() {
    // A line whose only shot is a reference (jump) has zero drawn
    // stations and must be skipped entirely, not even via the
    // "Line {id}" fallback.
    let svg = generate_svg_string(
        &jump_only_project(None, None),
        &ExportImageOptions {
            show_line_names: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !svg.contains(r#"class="line-names""#),
        "reference-only line should produce no line-name label: {svg}"
    );
}

#[test]
fn line_names_stay_black_regardless_of_color_mode() {
    let svg = generate_svg_string(
        &three_shot_line_project(),
        &ExportImageOptions {
            show_line_names: true,
            color_mode: ColorMode::Depth,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        group_tag(&svg, "line-names").contains(r##"fill="#000000""##),
        "line names should stay black regardless of color_mode: {svg}"
    );
}

#[test]
fn line_names_share_collision_space_with_station_labels() {
    let svg = generate_svg_string(
        &three_shot_line_project(),
        &ExportImageOptions {
            show_station_labels: true,
            show_line_names: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        svg.contains(r#"class="station-labels""#),
        "station labels should render: {svg}"
    );
    assert!(
        svg.contains(r#"class="line-names""#),
        "line names should render alongside station labels: {svg}"
    );
}

#[test]
fn station_comments_present_when_seeded() {
    let svg = generate_svg_string(
        &commented_line_project(),
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

/// A one-shot `Trunk` east whose shot carries `comment`.
fn shot_comment_project(comment: &str) -> Project {
    let mut s = Survey::new("Test");
    s.named(
        "Trunk",
        "#000000",
        vec![ShotData {
            comment: Some(comment.to_string()),
            ..make_shot(10.0, 90.0, false)
        }],
    );
    s.build()
}

#[test]
fn user_text_labels_are_xml_escaped_and_parse() {
    // Line names, station comments, and shot comments are user free-text
    // written into SVG <text>. A raw '&' / '<' / '>' would produce invalid
    // XML (resvg/svg2pdf fail on the PNG/PDF paths) or inject sibling markup
    // into the raw .svg. Verify each sink in isolation (so the three labels
    // can't collision-drop each other) that the escaped forms appear, no
    // injected element survives, and the whole document still parses.
    let parses = |svg: &str| Tree::from_str(svg, &Options::default()).is_ok();

    // Line name.
    let mut s = Survey::new("Test");
    s.named(
        "Sump & <Gallery>",
        "#000000",
        vec![make_shot(10.0, 90.0, false)],
    );
    let svg = generate_svg_string(
        &s.build(),
        &ExportImageOptions {
            show_line_names: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        svg.contains("Sump &amp; &lt;Gallery&gt;"),
        "line name must be XML-escaped: {svg}"
    );
    assert!(parses(&svg), "line-name SVG must parse: {svg}");

    // Station comment carrying a full injection payload.
    let (mut s, line) = three_shot_line_survey();
    s.comment((line, 1), "a & b </text><script>x");
    let svg = generate_svg_string(
        &s.build(),
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
    let svg = generate_svg_string(
        &shot_comment_project("tight & silty"),
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
    let svg = generate_svg_string(
        &shot_comment_project("a\u{0}b\u{1}c\u{0B}\u{0C}\u{FFFE}\u{FFFF}\u{FDD0}\td"),
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
    let svg = generate_svg_string(
        &commented_line_project(),
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
    let svg = generate_svg_string(
        &three_shot_line_project(),
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
    let svg = generate_svg_string(
        &jump_only_project(None, Some("orphaned")),
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
    let svg = generate_svg_string(
        &commented_line_project(),
        &ExportImageOptions {
            show_station_comments: true,
            color_mode: ColorMode::Depth,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        group_tag(&svg, "station-comments").contains(r##"fill="#000000""##),
        "station comments should stay black regardless of color_mode: {svg}"
    );
}

#[test]
fn station_comments_share_collision_space_with_station_labels() {
    let svg = generate_svg_string(
        &commented_line_project(),
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
    let mut s = Survey::new("Test");
    // azimuth_in (42.5) differs from azimuth_out (55.0) so asserting "42.5"
    // pins the label to azimuth_IN specifically -- it can't pass on
    // azimuth_out or the wraparound average_azimuth() (~48.7) the geometry
    // solver uses elsewhere.
    s.named(
        "Trunk",
        "#000000",
        vec![ShotData {
            azimuth_in: 42.5,
            azimuth_out: 55.0,
            length: 12.3,
            depth_in: 3.0,
            depth_out: 5.0,
            kind: ShotKind::Normal,
            ..Default::default()
        }],
    );
    let svg = generate_svg_string(
        &s.build(),
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
    let svg = generate_svg_string(
        &three_shot_line_project(),
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
    let svg = generate_svg_string(
        &jump_only_project(None, None),
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
    let svg = generate_svg_string(
        &three_shot_line_project(),
        &ExportImageOptions {
            show_shot_data: true,
            color_mode: ColorMode::Depth,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        group_tag(&svg, "shot-data").contains(r##"fill="#000000""##),
        "shot data should stay black regardless of color_mode: {svg}"
    );
}

#[test]
fn shot_data_shares_collision_space_with_station_labels() {
    let svg = generate_svg_string(
        &three_shot_line_project(),
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
    let svg = generate_svg_string(
        &shot_comment_project("silty"),
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
    let svg = generate_svg_string(
        &shot_comment_project("silty"),
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
    let svg = generate_svg_string(
        &three_shot_line_project(),
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
    let svg = generate_svg_string(
        &jump_only_project(Some("orphaned"), None),
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
    let svg = generate_svg_string(
        &shot_comment_project("silty"),
        &ExportImageOptions {
            show_shot_comments: true,
            color_mode: ColorMode::Depth,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        group_tag(&svg, "shot-comments").contains(r##"fill="#000000""##),
        "shot comments should stay black regardless of color_mode: {svg}"
    );
}

#[test]
fn shot_comments_share_collision_space_with_shot_data() {
    let svg = generate_svg_string(
        &shot_comment_project("silty"),
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
