//! #576 FZ-PARSE-5 (decision FZ-PARSE-5-D1), Ariane case: a length, depth or
//! passage dimension that is finite but beyond the 10,000 km limit no real
//! survey reaches is read as missing, as a non-finite one already was, and
//! the preview names the shot. Synthetic files only.

use super::test_helpers::{parse_synthetic_tmlu, wrap_xml_with};
use super::{convert_tml_with_options, TmlImportOptions};

fn shot(id: i64, from: i64, kind: &str, length: &str, depth: &str, left: &str) -> String {
    format!(
        r#"<SurveyData>
            <ID>{id}</ID><FromID>{from}</FromID><ClosureToID>-1</ClosureToID>
            <Type>{kind}</Type><Section>S1</Section>
            <Azimut>90</Azimut><Length>{length}</Length><Depth>{depth}</Depth><DepthIn>-1.0</DepthIn>
            <Inclination>0</Inclination><Left>{left}</Left><Right>1</Right><Up>1</Up><Down>1</Down>
            <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
        </SurveyData>"#
    )
}

fn limit_note(at: &str, what: &str, value: &str) -> String {
    format!(
        "{at}: {what} {value} m is beyond the 10,000 km limit no real survey reaches; read \
         as missing."
    )
}

#[test]
fn a_huge_reading_is_read_as_missing_and_named_on_the_preview() {
    let shots = [
        shot(0, -1, "START", "0", "0", "0"),
        shot(1, 0, "REAL", "1e308", "5", "1"),
        shot(2, 1, "REAL", "10", "1e308", "1"),
        shot(3, 2, "REAL", "10", "6", "1e308"),
    ]
    .concat();
    let tml = parse_synthetic_tmlu(&wrap_xml_with("m", "-3e7", &shots));
    let warnings = tml.warnings.clone().unwrap_or_default();
    println!("preview warnings: {warnings:?}");
    assert_eq!(
        warnings,
        [
            limit_note("The file", "entrance elevation", "-3e7"),
            limit_note("Shot 1", "length", "1e308"),
            limit_note("Shot 2", "depth", "1e308"),
            limit_note("Shot 3", "left", "1e308"),
        ]
    );
    assert_eq!(tml.first_start_absolute_elevation, 0.0);

    let (configs, _, receipt) =
        convert_tml_with_options(&tml, &TmlImportOptions::default()).unwrap();
    let readings: Vec<(f64, f64)> = configs
        .iter()
        .flat_map(|c| c.shots.iter().map(|s| (s.length, s.depth_out)))
        .collect();
    println!("imported (length, depth_out): {readings:?}");
    assert!(
        readings
            .iter()
            .all(|(l, d)| l.abs() <= 1e7 && d.abs() <= 1e7),
        "{readings:?}"
    );
    for line in &warnings {
        assert!(receipt.contains(line), "the receipt repeats {line:?}");
    }
}

#[test]
fn a_feet_file_is_bounded_in_metres() {
    // 3e7 ft is 9,144 km: inside the limit, so nothing is noted.
    let shots = [
        shot(0, -1, "START", "0", "0", "0"),
        shot(1, 0, "REAL", "3e7", "5", "1"),
    ]
    .concat();
    let tml = parse_synthetic_tmlu(&wrap_xml_with("ft", 0, &shots));
    assert_eq!(tml.warnings, None);
}
