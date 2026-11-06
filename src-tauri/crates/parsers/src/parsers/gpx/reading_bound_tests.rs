//! #576 FZ-PARSE-5 (decision FZ-PARSE-5-D1), GPX case: an elevation that is
//! finite but beyond the 10,000 km limit no real survey reaches is read as
//! missing and named on the preview. Next to an ordinary one it made a slope
//! length of infinity, so the dialog accepted a file the core then refused.
//! Synthetic tracks only.

use super::*;

const GPX: &str = r#"<?xml version="1.0"?><gpx version="1.1">
<wpt lat="0" lon="0"><name>W</name><ele>-1e300</ele></wpt>
<trk><name>T</name><trkseg>
<trkpt lat="0" lon="0"><ele>10</ele></trkpt>
<trkpt lat="0" lon="0.0001"><ele>1.7976931348623157e308</ele></trkpt>
<trkpt lat="0" lon="0.0002"><ele>12</ele></trkpt>
</trkseg></trk>
<rte><rtept lat="0" lon="0"><ele>1</ele></rtept><rtept lat="0" lon="0.0001"><ele>2e7</ele></rtept></rte>
</gpx>"#;

fn limit_note(at: &str, value: &str) -> String {
    format!(
        "{at}: elevation {value} m is beyond the 10,000 km limit no real survey reaches; \
         read as missing."
    )
}

#[test]
fn a_huge_elevation_is_read_as_missing_and_named_on_the_preview() {
    let gpx = parse_gpx_str(GPX).unwrap();
    let warnings = gpx.warnings.clone().unwrap();
    println!("preview warnings: {warnings:?}");
    assert_eq!(
        warnings,
        [
            limit_note("Waypoint 'W'", "-1e300"),
            limit_note("Track 'T' point 2", "1.7977e308"),
            limit_note("Route 1 point 2", "2e7"),
        ]
    );
    assert_eq!(gpx.tracks[0].segments[0].points[1].elevation, None);

    let (configs, receipt) =
        convert_gpx_to_import_configs(&gpx, true, true, true, &[], false).unwrap();
    let lengths: Vec<f64> = configs
        .iter()
        .flat_map(|c| c.shots.iter().map(|s| s.length))
        .collect();
    println!("imported lengths: {lengths:?}");
    println!("receipt lines: {receipt:?}");
    assert!(
        lengths.iter().all(|l| l.is_finite() && *l < 20.0),
        "{lengths:?}"
    );
    // The receipt repeats the notes, then says the gaps were filled in.
    assert_eq!(receipt[..3], warnings[..]);
    assert!(receipt[3].starts_with("2 GPS points had no elevation"));
}
