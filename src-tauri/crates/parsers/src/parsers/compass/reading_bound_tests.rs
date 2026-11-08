//! #576 FZ-PARSE-5 (decision FZ-PARSE-5-D1), Compass case (minimised from a
//! gen_mak fuzz artifact): a survey's length correction near `f64::MAX` was
//! accepted and added to every shot, and the computed depths overflowed to
//! infinity. It is now read as 0 and the preview names the survey.
//!
//! The same, for a shot's own length (the fuzz farm's raw_dat, gen_dat and
//! gen_mak findings on 074ab799e): once a length had no missing marker (#576
//! RVF-d-2), a leg of about f64::MAX feet was accepted, the depths overflowed
//! and the import was refused whole, and a leg of 1e12 ft landed as measured.
//! A length beyond the limit now reads as missing: the preview names it and
//! lists the leg as not imported, and the import drops it.
//! Synthetic surveys only.

use std::fmt::Write as _;

use super::tests::synthetic_survey_text;
use super::{
    build_preview, convert_compass_to_import_configs, parse_compass, CompassImportOptions,
    InvalidShotCounts, ParsedCompass, FEET_TO_METERS,
};
use crate::parsers::reading_bounds::MAX_READING_M;

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

/// The legs of one synthetic survey `A`, written to a `.DAT` and read through
/// `parse_compass`, which bounds the readings.
fn parse_legs(rows: &[&str]) -> ParsedCompass {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("survey.dat");
    std::fs::write(&path, synthetic_survey_text("A", "DDDDLRUDLADN", rows)).unwrap();
    parse_compass(&path).unwrap()
}

/// Each imported section's shot lengths (m) and the import's warnings; every
/// imported depth must be finite.
fn imported(parsed: &ParsedCompass, opts: &CompassImportOptions) -> (Vec<Vec<f64>>, Vec<String>) {
    let (configs, _, warnings) = convert_compass_to_import_configs(parsed, opts).unwrap();
    for shot in configs.iter().flat_map(|c| &c.shots) {
        assert!(
            shot.depth_in.is_finite() && shot.depth_out.is_finite(),
            "{shot:?}"
        );
    }
    let lengths = configs
        .iter()
        .map(|c| c.shots.iter().map(|s| s.length).collect())
        .collect();
    (lengths, warnings)
}

fn assert_lengths_ft(section: &[f64], feet: &[f64]) {
    assert_eq!(section.len(), feet.len(), "{section:?} vs {feet:?} ft");
    for (m, ft) in section.iter().zip(feet) {
        assert!(
            (m - ft * FEET_TO_METERS).abs() < 1e-9,
            "{section:?} vs {feet:?} ft"
        );
    }
}

/// A leg of 1e12 ft (3.048e11 m) landed as measured. It now reads as missing:
/// the preview names it, counts it and lists it as not imported, and the rest
/// of the survey imports around it.
#[test]
fn a_trillion_foot_leg_reads_as_missing_and_the_rest_of_the_survey_imports() {
    let parsed = parse_legs(&[
        "L1 L2 10.00 0.00 -10.00 1.00 1.00 1.00 1.00",
        "L2 L3 20.00 90.00 -20.00 1.00 1.00 1.00 1.00",
        "L2 X1 1000000000000.00 45.00 -30.00 1.00 1.00 1.00 1.00",
        "L3 L4 30.00 180.00 -30.00 1.00 1.00 1.00 1.00",
    ]);
    let preview = build_preview(&parsed);
    let (lengths, warnings) = imported(&parsed, &CompassImportOptions::default());
    println!("preview warnings: {:#?}", preview.warnings);
    println!("sections and their shot lengths (m): {lengths:?}");
    println!("import warnings: {warnings:#?}");
    assert_eq!(
        preview.warnings,
        [
            "Survey 'A' shot L2 -> X1: length 3.048e11 m is beyond the 10,000 km limit no real \
             survey reaches; read as missing.",
            "Survey 'A': shot L2 -> X1 has no usable length, so it will not be imported.",
        ]
    );
    assert_eq!(
        preview.invalid_shot_counts,
        InvalidShotCounts {
            non_excluded_corrected: 1,
            non_excluded_uncorrected: 1,
            ..InvalidShotCounts::default()
        }
    );
    assert_eq!(lengths.len(), 1, "one connected section: {lengths:?}");
    assert_lengths_ft(&lengths[0], &[10.0, 20.0, 30.0]);
    assert!(
        warnings.contains(&"Shot L2 -> X1: dropped (missing or zero length).".to_string()),
        "{warnings:?}"
    );
}

/// The bound is on the length in metres, after the feet are converted, and it
/// is inclusive: a leg of exactly 10,000 km is kept and the next foot value up
/// reads as missing.
#[test]
fn a_leg_just_past_the_limit_reads_as_missing_and_one_at_it_is_kept() {
    let at = MAX_READING_M / FEET_TO_METERS;
    assert_eq!(
        (at * FEET_TO_METERS).to_bits(),
        MAX_READING_M.to_bits(),
        "premise: {at} ft is exactly the limit"
    );
    let above = at.next_up();
    let rows = [
        format!("L1 L2 {at} 0.00 0.00 1.00 1.00 1.00 1.00"),
        format!("L2 L3 {above} 90.00 0.00 1.00 1.00 1.00 1.00"),
    ];
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let parsed = parse_legs(&rows);
    let preview = build_preview(&parsed);
    let (lengths, _) = imported(&parsed, &CompassImportOptions::default());
    println!("rows: {rows:?}");
    println!("preview warnings: {:#?}", preview.warnings);
    println!("sections and their shot lengths (m): {lengths:?}");
    assert_eq!(
        preview.warnings,
        [
            "Survey 'A' shot L2 -> L3: length 1e7 m is beyond the 10,000 km limit no real \
             survey reaches; read as missing.",
            "Survey 'A': shot L2 -> L3 has no usable length, so it will not be imported.",
        ]
    );
    assert_eq!(lengths.len(), 1, "{lengths:?}");
    assert_eq!(
        lengths[0].len(),
        1,
        "only the leg at the limit lands: {lengths:?}"
    );
    assert_eq!(
        lengths[0][0].to_bits(),
        MAX_READING_M.to_bits(),
        "{lengths:?}"
    );
}

/// The bound leaves real long legs alone: 990 ft and 1000 ft land as
/// measured (#576 RVF-d-2), and the preview says nothing about them.
#[test]
fn legs_of_990_and_1000_feet_are_kept() {
    let parsed = parse_legs(&[
        "L1 L2 990.00 0.00 0.00 1.00 1.00 1.00 1.00",
        "L2 L3 1000.00 90.00 0.00 1.00 1.00 1.00 1.00",
    ]);
    let preview = build_preview(&parsed);
    let (lengths, warnings) = imported(&parsed, &CompassImportOptions::default());
    println!("preview warnings: {:?}", preview.warnings);
    println!("sections and their shot lengths (m): {lengths:?}");
    println!("import warnings: {warnings:?}");
    assert!(preview.warnings.is_empty(), "{:?}", preview.warnings);
    assert_eq!(preview.invalid_shot_counts, InvalidShotCounts::default());
    assert_eq!(lengths.len(), 1, "{lengths:?}");
    assert_lengths_ft(&lengths[0], &[990.0, 1000.0]);
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// The preview and the import read the same bounded length under every
/// dialog toggle: the shots the preview counts out are the shots the import
/// does not land, the legs the import drops are the ones the preview names,
/// and the import's warnings lead with the preview's notes. One of the two
/// legs is `X`-flagged, so `include_excluded` decides whether it reaches the
/// length check at all.
#[test]
fn the_preview_and_the_import_agree_on_a_length_read_as_missing() {
    let rows = [
        "A1 A2 10.00 0.00 0.00 1.00 1.00 1.00 1.00".to_string(),
        "A2 A3 20.00 90.00 0.00 1.00 1.00 1.00 1.00".to_string(),
        "A2 X1 1000000000000.00 45.00 0.00 1.00 1.00 1.00 1.00".to_string(),
        format!("A3 X2 {:.2} 135.00 0.00 1.00 1.00 1.00 1.00 #|X#", f64::MAX),
    ];
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let parsed = parse_legs(&rows);
    let preview = build_preview(&parsed);
    println!("preview warnings: {:#?}", preview.warnings);
    let notes = [
        "Survey 'A' shot A2 -> X1: length 3.048e11 m is beyond the 10,000 km limit no real \
         survey reaches; read as missing.",
        "Survey 'A' shot A3 -> X2: length 5.4794e307 m is beyond the 10,000 km limit no real \
         survey reaches; read as missing.",
    ];
    assert_eq!(
        preview.warnings,
        [
            notes[0],
            notes[1],
            "Survey 'A': 2 shots have no usable length, so they will not be imported: \
             A2 -> X1, A3 -> X2.",
        ]
    );
    let counts = preview.invalid_shot_counts;
    assert_eq!(
        counts,
        InvalidShotCounts {
            non_excluded_corrected: 1,
            non_excluded_uncorrected: 1,
            excluded_corrected: 1,
            excluded_uncorrected: 1,
        }
    );
    for include_excluded in [false, true] {
        for apply_corrections in [false, true] {
            let opts = CompassImportOptions {
                include_excluded,
                apply_corrections,
                ..CompassImportOptions::default()
            };
            let (lengths, warnings) = imported(&parsed, &opts);
            let toggles =
                format!("include_excluded {include_excluded}, corrections {apply_corrections}");
            println!("{toggles}: lengths {lengths:?}, import warnings {warnings:#?}");
            // The dialog's arithmetic over the preview's fields.
            let (non_excluded, excluded) = if apply_corrections {
                (counts.non_excluded_corrected, counts.excluded_corrected)
            } else {
                (counts.non_excluded_uncorrected, counts.excluded_uncorrected)
            };
            let excluded_out = if include_excluded {
                excluded
            } else {
                preview.excluded_shot_count
            };
            let landed: usize = lengths.iter().map(Vec::len).sum();
            assert_eq!(
                landed as u32,
                preview.shot_count - excluded_out - non_excluded,
                "{toggles}: {lengths:?}"
            );
            assert_eq!(warnings[..2], notes, "{toggles}");
            let dropped: Vec<&str> = warnings
                .iter()
                .filter(|w| w.contains(": dropped ("))
                .map(String::as_str)
                .collect();
            let mut expected = vec!["Shot A2 -> X1: dropped (missing or zero length)."];
            if include_excluded {
                expected.push("Shot A3 -> X2: dropped (missing or zero length).");
            }
            assert_eq!(dropped, expected, "{toggles}");
        }
    }
}
