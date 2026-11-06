//! #576 FZ-PARSE-7 (decision FZ-PARSE-7-D1): any negative Compass LRUD reads
//! as missing. The vendored `compass_data` parser hands every LRUD column over
//! as the raw number (`-1.00` and `-0.50` alike), and the converter used to
//! treat only `<= -1` as the missing marker, so a unit-scaled marker such as
//! `-0.30` (-1 ft written in metres) imported as a negative passage
//! dimension that the validator then flagged. Synthetic surveys only.

use super::tests::{parse_dat_text, synthetic_survey_text};
use super::{convert_compass_to_import_configs, CompassImportOptions};

/// The imported (left, right, up, down) of each shot of a one-survey file.
fn imported_lruds(rows: &[&str]) -> Vec<[Option<f64>; 4]> {
    let parsed = parse_dat_text(&synthetic_survey_text("A", "DDDDLRUDLADN", rows));
    let (configs, _, _) =
        convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
    configs
        .iter()
        .flat_map(|c| c.shots.iter())
        .map(|s| [s.left, s.right, s.up, s.down])
        .collect()
}

#[test]
fn every_negative_lrud_reads_as_missing() {
    // Columns are LEFT UP DOWN RIGHT. One shot per marker spelling: the
    // classic -1, a half, the -1 ft marker written in metres, and a hair
    // below zero; the last row keeps one positive dimension so the test
    // also shows a real reading survives beside a negative one.
    let lruds = imported_lruds(&[
        "A1 A2 10.00 90.00 0.00 -1.00 -1.00 -1.00 -1.00",
        "A2 A3 10.00 90.00 0.00 -0.50 -0.50 -0.50 -0.50",
        "A3 A4 10.00 90.00 0.00 -0.30 -0.30 -0.30 -0.30",
        "A4 A5 10.00 90.00 0.00 -0.01 -0.01 -0.01 -0.01",
        "A5 A6 10.00 90.00 0.00 -0.50 3.28 -0.50 -0.50",
    ]);
    for (i, lrud) in lruds.iter().enumerate() {
        println!("shot {i}: left/right/up/down = {lrud:?}");
    }
    assert_eq!(lruds.len(), 5);
    for lrud in &lruds[..4] {
        assert_eq!(*lrud, [None; 4], "a negative LRUD must read as missing");
    }
    let [left, right, up, down] = lruds[4];
    assert_eq!((left, right, down), (None, None, None));
    let up = up.unwrap();
    assert!(
        (up - 3.28 * 0.3048).abs() < 1e-9,
        "the positive Up survives: {up}"
    );
}
