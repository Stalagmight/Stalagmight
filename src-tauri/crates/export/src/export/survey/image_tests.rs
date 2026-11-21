//! The map image on the station-graph model: one label, one comment and one
//! star per station, segments joined at junctions, the depth histogram
//! counting each station once, hidden lines, determinism, byte equality with
//! the legacy renderer where the models agree.

use super::super::test_helpers::{cave, gps_at, meta, project, run, Cave};
use super::{export_image, export_value_histogram, generate_svg_string};
use crate::export::color_scale::ExportValueKind;
use crate::export::{ColorMode, ExportImageOptions, ImageFormat};
use crate::models::survey::{StartAt, ZeroDatum};

fn labelled(ids: bool) -> ExportImageOptions {
    ExportImageOptions {
        format: ImageFormat::Svg,
        color_mode: ColorMode::Line,
        show_station_labels: ids,
        show_station_names: !ids,
        show_line_names: true,
        show_station_comments: true,
        show_shot_comments: true,
        ..ExportImageOptions::default()
    }
}

/// The text of every `<text>` inside the `<g class="{class}">` group.
fn texts(svg: &str, class: &str) -> Vec<String> {
    let Some(start) = svg.find(&format!("<g class=\"{class}\"")) else {
        return Vec::new();
    };
    let group = &svg[start..start + svg[start..].find("</g>").unwrap()];
    group
        .split("<text ")
        .skip(1)
        .map(|t| {
            let open = t.find('>').unwrap() + 1;
            t[open..t.find("</text>").unwrap()].to_string()
        })
        .collect()
}

/// A junction has one Station ID, so it gets one label (its representative
/// slot's name in names mode); every station on drawn passage is labelled
/// exactly once.
#[test]
fn a_junction_is_labelled_once() {
    let Cave { project, t, .. } = cave();
    let svg = generate_svg_string(&project, &labelled(true)).unwrap();
    let ids = texts(&svg, "station-labels");
    let hub = t[2].to_string();
    assert_eq!(ids.iter().filter(|t| **t == hub).count(), 1, "{ids:?}");
    let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "a station labelled twice: {ids:?}");
    let names = texts(
        &generate_svg_string(&project, &labelled(false)).unwrap(),
        "station-names",
    );
    assert_eq!(names.iter().filter(|n| *n == "Hub").count(), 1, "{names:?}");
}

/// A station comment is drawn once, at a junction too; a datum station gets
/// one star.
#[test]
fn comments_and_stars_are_drawn_once_per_station() {
    let Cave { project, .. } = cave();
    let options = ExportImageOptions {
        show_station_comments: true,
        ..ExportImageOptions::default()
    };
    let svg = generate_svg_string(&project, &options).unwrap();
    let comments = texts(&svg, "station-comments");
    assert_eq!(
        comments.iter().filter(|c| *c == "junction comment").count(),
        1,
        "{comments:?}"
    );
    let start = svg.find("<g class=\"zero-datums\"").unwrap();
    let group = &svg[start..start + svg[start..].find("</g>").unwrap()];
    assert_eq!(group.matches("<polygon").count(), project.datums.len());
}

/// The depth histogram counts each station once (a junction is one value),
/// over the same extent the render normalises across.
#[test]
fn the_depth_histogram_counts_each_station_once() {
    let Cave { project, .. } = cave();
    let raw = project.raw_positions();
    let h = export_value_histogram(&project, &raw, &raw, ExportValueKind::Depth, 8, false);
    let total: u32 = h.counts.iter().sum();
    assert_eq!(total as usize, project.stations.len());
    assert!(h.min <= 2.0 && h.max >= 9.0, "{h:?}");
    let d = export_value_histogram(&project, &raw, &raw, ExportValueKind::Distance, 8, false);
    let reachable: u32 = d.counts.iter().sum();
    let field = project.distance_field(&raw, None, false);
    let expected = field.iter().filter(|(_, d)| d.is_finite()).count();
    assert_eq!(reachable as usize, expected);
}

/// Hiding a line leaves it out of the picture.
#[test]
fn a_hidden_line_is_not_drawn() {
    let Cave { project, north, .. } = cave();
    let mut options = labelled(false);
    options.hidden_line_ids = Some(vec![north]);
    let names = texts(
        &generate_svg_string(&project, &options).unwrap(),
        "line-names",
    );
    assert!(!names.contains(&"North".to_string()), "{names:?}");
    assert!(names.contains(&"Trunk".to_string()), "{names:?}");
}

/// Same project, same bytes; PNG and PDF write through the shared writers.
#[test]
fn rendering_is_deterministic_and_every_format_writes() {
    let Cave { project, .. } = cave();
    let options = labelled(true);
    assert_eq!(
        generate_svg_string(&project, &options).unwrap(),
        generate_svg_string(&project, &options).unwrap()
    );
    let dir = tempfile::TempDir::new().unwrap();
    for format in [ImageFormat::Svg, ImageFormat::Png, ImageFormat::Pdf] {
        let path = dir.path().join(format!("map.{format:?}"));
        let options = ExportImageOptions {
            format,
            color_mode: ColorMode::Depth,
            ..ExportImageOptions::default()
        };
        export_image(&project, &path, &options).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }
}

/// The adjusted positions draw the solved loop: the rendering changes when
/// the Computed View moves stations, and is the measured one otherwise.
#[test]
fn the_rendering_follows_the_positions_it_is_given() {
    let Cave { project, .. } = cave();
    let options = labelled(true);
    let adjusted =
        project.adjusted_positions(&crate::models::config::ClosureAdjustmentConfig::default());
    let measured = generate_svg_string(&project, &options).unwrap();
    let solved = super::generate_svg_string_with_positions(
        &project,
        &options,
        &adjusted,
        &project.raw_positions(),
    )
    .unwrap();
    assert_ne!(
        measured, solved,
        "the Batch loop is solved in the Computed View"
    );
}

/// A plain survey (one line, a datum at its start) renders exactly what the
/// 0.6.0 renderer drew for the same survey: the pinned bytes below are the
/// legacy renderer's output, captured before it was deleted (with station
/// IDs, then with station names; this survey's names are its ids).
#[test]
fn a_simple_survey_renders_the_legacy_bytes() {
    let mut p = project("Simple");
    let shots = run(0, 3, 60.0, 10.0, 2.0, 1.0);
    let main = p
        .add_line(meta("Main", "#336699"), StartAt::New, shots)
        .unwrap();
    let start = p.line(main).unwrap().start;
    p.datums.push(ZeroDatum::new(start, gps_at(0.0, 0.0)));
    assert!(p.validate().is_empty());

    assert_eq!(
        generate_svg_string(&p, &labelled(true)).unwrap(),
        LEGACY_SIMPLE_IDS_SVG
    );
    assert_eq!(
        generate_svg_string(&p, &labelled(false)).unwrap(),
        LEGACY_SIMPLE_IDS_SVG.replace("station-labels", "station-names")
    );
}

/// The 0.6.0 renderer's SVG of [`a_simple_survey_renders_the_legacy_bytes`]'s
/// survey with station IDs labelled.
const LEGACY_SIMPLE_IDS_SVG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="800.00" height="492.62" viewBox="-1.3 -16.2 28.4 17.5">
<rect x="-1.3" y="-16.2" width="28.4" height="17.5" fill="#ffffff"/>
<g class="shots" stroke="#000000" fill="none" stroke-linejoin="round" stroke-linecap="round">
<polyline points="0.00,-0.00 8.62,-4.97 17.23,-9.95 25.85,-14.92" stroke-width="0.03554448137545405"/>
</g>
<g class="zero-datums" fill="#000000">
<polygon points="0.00,-0.25 0.06,-0.09 0.24,-0.08 0.10,0.03 0.15,0.20 0.00,0.11 -0.15,0.20 -0.10,0.03 -0.24,-0.08 -0.06,-0.09"/>
</g>
<g class="station-labels" font-family="sans-serif" font-size="0.39" fill="#000000" stroke="#ffffff" stroke-width="0.11" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">
<text x="8.78" y="-4.70">1</text>
<text x="17.39" y="-9.67">2</text>
<text x="26.13" y="-15.08">3</text>
</g>
<g class="line-names" font-family="sans-serif" font-size="0.39" fill="#000000" stroke="#ffffff" stroke-width="0.11" paint-order="stroke fill" text-anchor="middle" dominant-baseline="central">
<text x="12.93" y="-7.46">Main</text>
</g>
</svg>
"##;
