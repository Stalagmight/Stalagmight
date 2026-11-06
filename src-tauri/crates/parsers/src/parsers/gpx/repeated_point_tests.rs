//! #576 FZ-PARSE-1 (decision FZ-PARSE-1-D1): a GPS standing still logs the
//! same point again and again. Each repeat used to become a zero-length shot,
//! and the core refuses a section holding one, so the whole track was lost.
//! The converter now drops a point identical to the one before it and counts
//! it on the receipt. Synthetic tracks only.

use super::*;

/// A one-segment track on the equator: `(longitude, elevation)` per point.
fn track(points: &[(f64, Option<f64>)]) -> GpxFile {
    let trkpts: String = points
        .iter()
        .map(|(lon, ele)| match ele {
            Some(e) => format!(r#"<trkpt lat="0" lon="{lon}"><ele>{e}</ele></trkpt>"#),
            None => format!(r#"<trkpt lat="0" lon="{lon}"/>"#),
        })
        .collect();
    let xml = format!(
        r#"<?xml version="1.0"?><gpx version="1.1"><trk><name>T</name><trkseg>{trkpts}</trkseg></trk></gpx>"#
    );
    parse_gpx_str(&xml).unwrap()
}

fn convert(
    gpx: &GpxFile,
    placements: &[GpxSectionPlacement],
) -> (Vec<ImportSectionConfig>, Vec<String>) {
    convert_gpx_to_import_configs(gpx, false, true, false, placements, false).unwrap()
}

#[test]
fn a_repeated_point_is_dropped_and_counted_on_the_receipt() {
    // Points 2 and 3 are the same position and elevation; so are 5 and 6.
    let gpx = track(&[
        (0.0, Some(10.0)),
        (0.0001, Some(10.0)),
        (0.0001, Some(10.0)),
        (0.0002, Some(12.0)),
        (0.0003, Some(12.0)),
        (0.0003, Some(12.0)),
    ]);
    let (configs, warnings) = convert(&gpx, &[]);
    let lengths: Vec<f64> = configs[0].shots.iter().map(|s| s.length).collect();
    println!("shot lengths: {lengths:?}");
    println!("receipt lines: {warnings:?}");
    assert!(
        lengths.iter().all(|&l| l > 0.0),
        "no zero-length shot may reach the core: {lengths:?}"
    );
    assert_eq!(lengths.len(), 3, "6 points, 2 repeats: 3 shots");
    assert_eq!(
        warnings,
        [
            "2 repeated GPS points skipped: each was at the same position and elevation as the \
          point before it."
        ]
    );
    // The chain stays continuous: each shot starts where the last one ended.
    for pair in configs[0].shots.windows(2) {
        assert!((pair[0].depth_out - pair[1].depth_in).abs() < 1e-12);
    }
}

#[test]
fn one_repeated_point_reads_in_the_singular() {
    let gpx = track(&[(0.0, Some(1.0)), (0.0, Some(1.0)), (0.0001, Some(1.0))]);
    let (configs, warnings) = convert(&gpx, &[]);
    println!("receipt lines: {warnings:?}");
    assert_eq!(configs[0].shots.len(), 1);
    assert_eq!(
        warnings,
        [
            "1 repeated GPS point skipped: it was at the same position and elevation as the \
          point before it."
        ]
    );
}

#[test]
fn a_point_that_only_changes_elevation_is_kept() {
    // Same position, different elevation: a vertical leg, not a repeat.
    let gpx = track(&[(0.0, Some(1.0)), (0.0, Some(3.0)), (0.0001, Some(3.0))]);
    let (configs, warnings) = convert(&gpx, &[]);
    assert_eq!(configs[0].shots.len(), 2);
    assert!((configs[0].shots[0].length - 2.0).abs() < 1e-9);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_track_of_one_repeated_point_imports_nothing_and_says_so() {
    let gpx = track(&[(0.0, Some(1.0)), (0.0, Some(1.0)), (0.0, Some(1.0))]);
    let (configs, warnings) = convert(&gpx, &[]);
    assert!(configs.is_empty(), "{configs:?}");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].starts_with("2 repeated GPS points skipped"));
}

#[test]
fn a_placement_station_is_renumbered_past_the_dropped_points() {
    // The placement dialog names the zero datum by POINT index: point 3 is
    // the last point, which becomes station 2 once point 2 is dropped.
    let gpx = track(&[
        (0.0, Some(1.0)),
        (0.0001, Some(1.0)),
        (0.0001, Some(1.0)),
        (0.0002, Some(1.0)),
    ]);
    let at_point = |station_index| GpxSectionPlacement {
        section_index: 0,
        placement: ImportPlacement::ZeroDatum {
            coordinates: GpsCoordinates::new(0.0, 0.0002, 0.0).unwrap(),
            station_index,
        },
        name: None,
        color: "#000000".to_string(),
    };
    for (point, station) in [(0, 0), (1, 1), (2, 1), (3, 2)] {
        let (configs, _) = convert(&gpx, &[at_point(point)]);
        let ImportPlacement::ZeroDatum { station_index, .. } = configs[0].placement else {
            panic!("placement kept its kind: {:?}", configs[0].placement);
        };
        println!("point {point} -> station {station_index}");
        assert_eq!(station_index, station, "point {point}");
    }
}
