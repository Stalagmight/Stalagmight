//! The map image on the station-graph model: one label, one comment and one
//! star per station, segments joined at junctions, the depth histogram
//! counting each station once, hidden lines, determinism, byte equality with
//! the legacy renderer where the models agree.

use super::super::test_helpers::{cave, gps_at, meta, project, run, Cave};
use super::{export_image, export_value_histogram, generate_svg_string};
use crate::export::color_scale::ExportValueKind;
use crate::export::{ColorMode, ExportImageOptions, ImageFormat};
use crate::models::survey::{StartAt, ZeroDatum};
use crate::models::LineData;

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

/// A junction has one Station ID and one name, so it gets one label; every
/// station on drawn passage is labelled exactly once.
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
    let h = export_value_histogram(&project, &raw, ExportValueKind::Depth, 8, false);
    let total: u32 = h.counts.iter().sum();
    assert_eq!(total as usize, project.stations.len());
    assert!(h.min <= 2.0 && h.max >= 9.0, "{h:?}");
    let d = export_value_histogram(&project, &raw, ExportValueKind::Distance, 8, false);
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
    let solved = super::generate_svg_string_with_positions(&project, &options, &adjusted).unwrap();
    assert_ne!(
        measured, solved,
        "the Batch loop is solved in the Computed View"
    );
}

/// A plain survey (one line, a datum at its start) renders exactly what the
/// legacy renderer draws for the same survey.
#[test]
fn a_simple_survey_renders_the_legacy_bytes() {
    let mut p = project("Simple");
    let shots = run(0, 3, 60.0, 10.0, 2.0, 1.0);
    let main = p
        .add_line(meta("Main", "#336699"), StartAt::New, shots.clone())
        .unwrap();
    let start = p.line(main).unwrap().start;
    p.datums.push(ZeroDatum::new(start, gps_at(0.0, 0.0)));
    assert!(p.validate().is_empty());

    let mut legacy = crate::models::Project::new("Simple".to_string(), None);
    let line = legacy
        .add_line(LineData {
            name: Some("Main".to_string()),
            color: "#336699".to_string(),
            shots,
            ..LineData::default()
        })
        .id;
    legacy
        .set_zero_datum(crate::models::StationRef::new(line, 0), gps_at(0.0, 0.0))
        .unwrap();
    legacy.ensure_station_identities();
    for options in [labelled(true), labelled(false)] {
        assert_eq!(
            generate_svg_string(&p, &options).unwrap(),
            crate::export::generate_svg_string(&legacy, &options).unwrap()
        );
    }
}
