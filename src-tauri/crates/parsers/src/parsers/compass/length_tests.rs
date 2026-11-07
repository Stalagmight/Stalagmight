//! #576 RVF-d-2: a shot's LENGTH has no missing marker in a `.DAT`. Compass's
//! "Survey Data File Format" page names one for a bearing or inclination when
//! backsights are used (-999) and says a negative LRUD is missing, but gives a
//! length none: it is "a single floating point number" in decimal feet. A leg
//! of 990 ft or more was read as missing, dropped, and the survey split in two
//! with nothing said on the preview. A shot the import does drop for its
//! length is now named on the preview. Synthetic surveys only.

use super::tests::{parse_dat_text, synthetic_survey_text};
use super::{build_preview, convert_compass_to_import_configs, CompassImportOptions};

const LEGS: [&str; 3] = [
    "L1 L2 100.00 0.00 0.00 1.00 1.00 1.00 1.00",
    "L2 L3 1000.00 90.00 0.00 1.00 1.00 1.00 1.00",
    "L3 L4 50.00 180.00 0.00 1.00 1.00 1.00 1.00",
];

/// A 1000 ft leg (304.8 m) between two ordinary ones lands as one section of
/// three shots: kept, not dropped, and not counted as invalid.
#[test]
fn a_leg_of_a_thousand_feet_is_kept() {
    let parsed = parse_dat_text(&synthetic_survey_text("A", "DDDDLRUDLADN", &LEGS));
    let preview = build_preview(&parsed);
    let (configs, _, warnings) =
        convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
    let lengths: Vec<Vec<f64>> = configs
        .iter()
        .map(|c| c.shots.iter().map(|s| s.length).collect())
        .collect();
    println!(
        "preview: invalid {:?} warnings {:?}",
        preview.invalid_shot_counts, preview.warnings
    );
    println!("sections and their shot lengths (m): {lengths:?}");
    println!("convert warnings: {warnings:?}");
    assert_eq!(lengths.len(), 1, "one connected section: {lengths:?}");
    assert_eq!(lengths[0].len(), 3, "every leg is kept: {lengths:?}");
    assert!(
        (lengths[0][1] - 304.8).abs() < 1e-9,
        "the long leg is 1000 ft: {lengths:?}"
    );
    assert!(
        !warnings.iter().any(|w| w.contains("dropped")),
        "no leg is dropped: {warnings:?}"
    );
    assert_eq!(preview.invalid_shot_counts.non_excluded_corrected, 0);
    assert_eq!(preview.invalid_shot_counts.non_excluded_uncorrected, 0);
}

/// A shot the import drops for its length is named on the preview, not only on
/// the receipt after the import: a zero-length leg whatever the corrections
/// say, and a leg the survey's length correction alone takes to zero or below,
/// with when that happens.
#[test]
fn a_leg_dropped_for_its_length_is_named_on_the_preview() {
    let rows = [
        "L1 L2 10.00 0.00 0.00 1.00 1.00 1.00 1.00",
        "L2 L3 0.00 90.00 0.00 1.00 1.00 1.00 1.00",
        "L3 L4 3.00 180.00 0.00 1.00 1.00 1.00 1.00",
    ];
    let text = synthetic_survey_text("A", "DDDDLRUDLADN", &rows).replace(
        "CORRECTIONS: 0.00 0.00 0.00",
        "CORRECTIONS: 0.00 0.00 -4.00",
    );
    let parsed = parse_dat_text(&text);
    let preview = build_preview(&parsed);
    println!("preview warnings: {:#?}", preview.warnings);
    let naming = |shot: &str| {
        preview
            .warnings
            .iter()
            .filter(|w| w.contains(shot))
            .cloned()
            .collect::<Vec<_>>()
    };
    let zero = naming("L2 -> L3");
    assert_eq!(zero.len(), 1, "the zero-length leg is named once: {zero:?}");
    assert!(
        zero[0].contains("Survey 'A'") && zero[0].contains("will not be imported"),
        "it says the leg will not be imported: {zero:?}"
    );
    let corrected = naming("L3 -> L4");
    assert_eq!(
        corrected.len(),
        1,
        "the leg the correction empties is named once: {corrected:?}"
    );
    assert!(
        corrected[0].contains("length correction"),
        "it says the correction is why: {corrected:?}"
    );
    assert!(
        naming("L1 -> L2").is_empty(),
        "a leg that is imported is not named: {:?}",
        preview.warnings
    );
}

/// Surveys are tied together with zero-length connector shots, so one survey
/// can drop many: they share one line, which names the first ten and counts
/// the rest.
#[test]
fn many_dropped_legs_share_one_preview_line() {
    let rows: Vec<String> = (1..=15)
        .map(|i| format!("C{i} C{} 0.00 0.00 0.00 1.00 1.00 1.00 1.00", i + 1))
        .collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let parsed = parse_dat_text(&synthetic_survey_text("A", "DDDDLRUDLADN", &rows));
    let preview = build_preview(&parsed);
    println!("preview warnings: {:#?}", preview.warnings);
    assert_eq!(preview.warnings.len(), 1, "{:?}", preview.warnings);
    let line = &preview.warnings[0];
    assert!(
        line.contains("15 shots") && line.contains("C10 -> C11") && line.contains("and 5 more"),
        "{line}"
    );
    assert!(!line.contains("C11 -> C12"), "only ten are named: {line}");
}
