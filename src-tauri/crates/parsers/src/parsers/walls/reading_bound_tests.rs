//! #576 FZ-PARSE-5 / FZ-PARSE-8 (decision FZ-PARSE-5-D1): a Walls reading
//! that is finite but beyond the 10,000 km limit no real survey reaches is
//! read as missing, and the preview names it with its line. It used to be
//! accepted: two 1e308 m legs computed a station at infinity, and a RECT
//! vector past ~1.3e154 overflowed on squaring, so the preview accepted a
//! file the import then refused. Synthetic surveys only.

use std::collections::HashMap;

use super::{
    build_preview, convert_walls_to_import_configs, parse_srv_contents, ParsedWalls,
    WallsImportOptions, WallsUnits,
};

/// `srv` parsed as the bare `survey.srv` a user opened.
fn parse(srv: &str) -> ParsedWalls {
    let mut warnings = Vec::new();
    let survey = parse_srv_contents(
        srv,
        "survey",
        "survey",
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        None,
        true,
        false,
        &mut warnings,
    );
    ParsedWalls {
        project_name: "survey".to_string(),
        book_count: 0,
        surveys: vec![survey],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings,
    }
}

/// The imported shot lengths under the default options.
fn imported_lengths(parsed: &ParsedWalls) -> Vec<f64> {
    let (configs, _, _) =
        convert_walls_to_import_configs(parsed, &WallsImportOptions::default()).unwrap();
    configs
        .iter()
        .flat_map(|c| c.shots.iter().map(|s| s.length))
        .collect()
}

fn limit_note(line: usize, what: &str, value: &str) -> String {
    format!(
        "survey:{line}: {what} {value} m is beyond the 10,000 km limit no real survey \
         reaches; read as missing."
    )
}

#[test]
fn a_huge_distance_is_read_as_missing_and_named_on_the_preview() {
    let parsed = parse("A1 A2 1e308 0 0\nA2 A3 1e308 0 0\nA3 A4 10 0 0\n");
    let preview = build_preview(&parsed);
    let lengths = imported_lengths(&parsed);
    println!("preview warnings: {:?}", preview.warnings);
    println!("imported lengths: {lengths:?}");
    assert_eq!(
        preview.warnings,
        [
            limit_note(1, "distance", "1e308"),
            limit_note(2, "distance", "1e308")
        ]
    );
    // Each huge leg reads as a missing distance: with its bearing it stays
    // a coincident-station tie, so the stations stay connected.
    assert_eq!(lengths.len(), 3);
    assert!(lengths.iter().all(|l| *l <= 10.0), "{lengths:?}");
}

#[test]
fn a_huge_rect_vector_is_named_on_the_preview_and_never_imported() {
    let parsed = parse("#Units RECT\nA1 A2 1e308 1e308 1e308\nA2 A3 3 4 0\n");
    let preview = build_preview(&parsed);
    let lengths = imported_lengths(&parsed);
    println!("preview warnings: {:?}", preview.warnings);
    println!(
        "preview shots: {} imported lengths: {lengths:?}",
        preview.shot_count
    );
    assert_eq!(
        preview.warnings,
        [
            limit_note(2, "east", "1e308"),
            limit_note(2, "north", "1e308"),
            limit_note(2, "up", "1e308"),
        ]
    );
    // The preview and the import agree: one shot each.
    assert_eq!(preview.shot_count, 1);
    assert_eq!(lengths.len(), 1);
    assert!((lengths[0] - 5.0).abs() < 1e-9, "{lengths:?}");
}

#[test]
fn a_huge_correction_passage_dimension_or_fix_elevation_is_named() {
    let parsed =
        parse("#Units IncH=1e300\nA1 A2 10 0 0 <1e308,1,1,1>\n#Units IncH=0\n#Fix A1 0 0 -2e7\n");
    let warnings = &parsed.warnings;
    println!("warnings: {warnings:?}");
    assert!(warnings.contains(&limit_note(4, "#Fix elevation", "-2e7")));
    assert!(warnings.contains(&limit_note(2, "passage dimension", "1e308")));
    assert!(warnings.contains(&limit_note(2, "IncH= correction", "1e300")));
    assert_eq!(parsed.surveys[0].fixes[0].up, None);
    let shot = &parsed.surveys[0].vectors[0];
    assert_eq!((shot.left, shot.inch), (None, 0.0));
}

#[test]
fn a_reading_at_the_limit_is_kept() {
    let parsed = parse("A1 A2 1e7 0 0\n");
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(imported_lengths(&parsed), [1e7]);
}

fn angle_note(line: usize, what: &str, value: &str) -> String {
    format!(
        "survey:{line}: {what} {value} degrees is beyond the 1,000,000 degree limit no real \
         survey reaches; read as missing."
    )
}

/// Every number the import hands the core, per shot.
fn imported_numbers(parsed: &ParsedWalls) -> Vec<f64> {
    let (configs, _, _) =
        convert_walls_to_import_configs(parsed, &WallsImportOptions::default()).unwrap();
    configs
        .iter()
        .flat_map(|c| c.shots.iter())
        .flat_map(|s| {
            [
                s.azimuth_in,
                s.azimuth_out,
                s.length,
                s.depth_in,
                s.depth_out,
            ]
            .into_iter()
            .chain(s.pitch_in)
            .chain(s.pitch_out)
        })
        .collect()
}

/// Minimised from the 15-minute gen_srv cargo-fuzz run on this fix: an
/// inclination foresight of f64::MAX and backsight of -f64::MAX averaged to
/// infinity, the shot's pitch became NaN, and the preview accepted a file
/// the import refused ("Section has a shot with a non-finite value").
#[test]
fn a_huge_inclination_pair_is_read_as_missing_and_named_on_the_preview() {
    let max = format!("{:.2}", f64::MAX);
    let parsed = parse(&format!("A16 A17 2.26 222.95/-- {max}/-{max}\n"));
    let preview = build_preview(&parsed);
    let numbers = imported_numbers(&parsed);
    println!("preview warnings: {:?}", preview.warnings);
    println!("imported numbers: {numbers:?}");
    assert_eq!(
        preview.warnings,
        [
            angle_note(1, "inclination", "1.7977e308"),
            angle_note(1, "inclination backsight", "-1.7977e308"),
        ]
    );
    assert_eq!(preview.shot_count, 1);
    assert!(
        !numbers.is_empty() && numbers.iter().all(|v| v.is_finite()),
        "{numbers:?}"
    );
}

#[test]
fn a_huge_azimuth_or_angle_correction_is_read_as_missing() {
    let parsed = parse(
        "#Units IncA=1e308 IncV=1e308 IncVB=-1e308 Decl=1e308 Grid=1e308\n\
         A1 A2 10 1e308/-1e308 1/2\n",
    );
    let warnings = &parsed.warnings;
    println!("warnings: {warnings:?}");
    for what in [
        "IncA= correction",
        "IncV= correction",
        "IncVB= correction",
        "Grid= convergence",
        "Decl= declination",
        "azimuth",
    ] {
        assert!(
            warnings.contains(&angle_note(2, what, "1e308"))
                || warnings.contains(&angle_note(2, what, "-1e308")),
            "{what}: {warnings:?}"
        );
    }
    let shot = &parsed.surveys[0].vectors[0];
    assert_eq!((shot.units_decl, shot.grid_convergence), (None, 0.0));
    let numbers = imported_numbers(&parsed);
    assert!(
        !numbers.is_empty() && numbers.iter().all(|v| v.is_finite()),
        "{numbers:?}"
    );
}

#[test]
fn an_ordinary_angle_pair_is_kept() {
    let parsed = parse("A1 A2 10 90/270 5/-5\n");
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

/// Minimised from the 15-minute gen_wpj cargo-fuzz run on this fix: an
/// `IncS=` of f64::MAX added to a passage dimension of f64::MAX / 10
/// overflowed to infinity after the bound, and the shot's LRUD became
/// infinite. The correction and the dimension are now bounded first.
#[test]
fn a_huge_incs_correction_and_passage_dimension_are_read_as_missing() {
    let max = format!("{:.2}", f64::MAX);
    let tenth = format!("{:.2}", f64::MAX / 10.0);
    let parsed = parse(&format!(
        "#Units IncS={max}\nD50 G66 7.53 101.26/-- 32.65 <{tenth},5.03,5.56,1.13>\n"
    ));
    let warnings = &parsed.warnings;
    println!("warnings: {warnings:?}");
    assert!(warnings.contains(&limit_note(2, "IncS= correction", "1.7977e308")));
    assert!(warnings.contains(&limit_note(2, "passage dimension", "1.7977e307")));
    let shot = &parsed.surveys[0].vectors[0];
    assert_eq!(shot.left, None);
    assert!(
        (shot.right.unwrap() - 5.03).abs() < 1e-9,
        "{:?}",
        shot.right
    );
}

#[test]
fn an_ordinary_incs_correction_is_still_applied() {
    let parsed = parse("#Units IncS=0.5\nA1 A2 10 0 0 <1,2,3,4>\n");
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.surveys[0].vectors[0].left, Some(1.5));
}
