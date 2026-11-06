//! #576 FZ-PARSE-5 (decision FZ-PARSE-5-D1), Compass case (minimised from a
//! gen_mak fuzz artifact): a survey's length correction near `f64::MAX` was
//! accepted and added to every shot, and the computed depths overflowed to
//! infinity. It is now read as 0 and the preview names the survey.
//! Synthetic surveys only.

use std::fmt::Write as _;

use super::{
    build_preview, convert_compass_to_import_configs, parse_compass, CompassImportOptions,
};

fn survey_with_length_correction(correction: &str) -> String {
    let mut text = format!(
        "SYNTHETIC CAVE\r\nSURVEY NAME: B1\r\nSURVEY DATE: 2 20 1993\r\n\
         SURVEY TEAM:\r\nAnn\r\n\
         DECLINATION: 0.00  CORRECTIONS: 0.00 0.00 {correction}\r\n\r\n\
         FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT\r\n\r\n"
    );
    for i in 1..10 {
        let _ = write!(
            text,
            "B{i} B{} 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n",
            i + 1
        );
    }
    text.push_str("\x0c\r\n");
    text
}

#[test]
fn a_huge_length_correction_is_read_as_zero_and_named_on_the_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("survey.dat");
    std::fs::write(&path, survey_with_length_correction("5.9e307")).unwrap();
    let parsed = parse_compass(&path).unwrap();
    let preview = build_preview(&parsed);
    let opts = CompassImportOptions {
        apply_corrections: true,
        ..CompassImportOptions::default()
    };
    let (configs, _, _) = convert_compass_to_import_configs(&parsed, &opts).unwrap();
    let shots: Vec<(f64, f64)> = configs
        .iter()
        .flat_map(|c| c.shots.iter().map(|s| (s.length, s.depth_out)))
        .collect();
    println!("preview warnings: {:?}", preview.warnings);
    println!("imported (length, depth_out): {shots:?}");
    assert_eq!(
        preview.warnings,
        [
            "Survey 'B1': length correction 1.7983e307 m is beyond the 10,000 km limit no \
          real survey reaches; read as 0."
        ]
    );
    assert_eq!(shots.len(), 9);
    for (length, depth) in shots {
        assert!(
            (length - 0.3048).abs() < 1e-9,
            "the 1 ft leg is uncorrected"
        );
        assert!(depth.is_finite() && depth.abs() < 3.0, "depth {depth}");
    }
}

#[test]
fn an_ordinary_length_correction_is_still_applied() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("survey.dat");
    std::fs::write(&path, survey_with_length_correction("1.00")).unwrap();
    let parsed = parse_compass(&path).unwrap();
    assert!(build_preview(&parsed).warnings.is_empty());
    let (configs, _, _) =
        convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
    let length = configs[0].shots[0].length;
    assert!(
        (length - 2.0 * 0.3048).abs() < 1e-9,
        "1 ft + 1 ft: {length}"
    );
}

/// A `.MAK` fixed station whose elevation is beyond the limit seeded every
/// station depth; it is now ignored as an anchor candidate, as a non-finite
/// one already was, and the preview names it.
#[test]
fn a_mak_fixed_station_elevation_beyond_the_limit_is_ignored_as_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("survey.dat"),
        survey_with_length_correction("0.00"),
    )
    .unwrap();
    let mak = |up: &str| {
        format!(
            "@500000.000,2000000.000,0.000,16,0.000;\r\n&WGS 1984;\r\n\
                 #survey.dat,B1[m,500000,2000000,{up}];\r\n"
        )
    };
    let path = dir.path().join("project.mak");
    std::fs::write(&path, mak("20000000.0")).unwrap();
    let parsed = parse_compass(&path).unwrap();
    let preview = build_preview(&parsed);
    println!("preview warnings: {:?}", preview.warnings);
    assert!(parsed.fixed_stations.is_empty());
    assert!(preview.warnings.contains(
        &"Fixed station 'B1': elevation 2e7 m is beyond the 10,000 km limit no real survey \
          reaches; it was ignored as an anchor candidate."
            .to_string()
    ));

    std::fs::write(&path, mak("12")).unwrap();
    let parsed = parse_compass(&path).unwrap();
    assert_eq!(parsed.fixed_stations.len(), 1, "{:?}", parsed.warnings);
}
