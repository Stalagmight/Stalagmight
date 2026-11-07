//! #576 RV2-R-NITS (B31): a To-associated survey can measure a cross-section
//! at one station from two rows, and a shot leaving the station carries only
//! one of them. The other used to vanish without a word; it is counted on the
//! receipt like the end-of-chain sections. Synthetic surveys only.

use super::tests::{parse_dat_text, synthetic_survey_text};
use super::{convert_compass_to_import_configs, CompassImportOptions};

fn warnings_for(rows: &[&str]) -> Vec<String> {
    let parsed = parse_dat_text(&synthetic_survey_text("A", "DDDDLRUDLADNT", rows));
    let (_, _, warnings) =
        convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
    warnings
}

fn lrud_lines(warnings: &[String]) -> Vec<&String> {
    warnings
        .iter()
        .filter(|w| w.contains("cross-section"))
        .collect()
}

/// A1 -> A2 -> A3, then a second chain B1 -> A2 that also measured A2. The
/// shot A2 -> A3 carries the section its own chain brought (A1 -> A2's), so
/// B1 -> A2's section at A2 lands on no shot; A2 -> A3's at A3 has no shot
/// leaving A3 at all.
#[test]
fn a_second_section_at_one_station_is_counted_when_dropped() {
    let warnings = warnings_for(&[
        "A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00",
        "A2 A3 10.00 90.00 0.00 2.00 2.00 2.00 2.00",
        "B1 A2 10.00 0.00 0.00 3.00 3.00 3.00 3.00",
    ]);
    let lines = lrud_lines(&warnings);
    println!(
        "cross-section lines: {} {lines:?} (expected 2)",
        lines.len()
    );
    assert_eq!(
        lines,
        [
            "Survey 'A': 1 end-of-chain cross-section had no shot to attach to and was dropped \
             (the survey records LRUDs at each shot's To station).",
            "Survey 'A': 1 cross-section was superseded by another measured at the same station \
             and was dropped (the survey records LRUDs at each shot's To station).",
        ]
    );
}

/// The first row to reach a station can be the one superseded, too: B1 -> A2
/// comes first, but A2 -> A3 takes A1 -> A2's section from its own chain and
/// no other shot leaves A2; C1 -> A3 the same at A3. Two such stations make
/// the plural line (A3 -> A4 measured nothing, so nothing is stranded).
#[test]
fn every_superseded_section_of_a_survey_is_counted_on_one_line() {
    let warnings = warnings_for(&[
        "B1 A2 10.00 0.00 0.00 3.00 3.00 3.00 3.00",
        "C1 A3 10.00 0.00 0.00 4.00 4.00 4.00 4.00",
        "A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00",
        "A2 A3 10.00 90.00 0.00 2.00 2.00 2.00 2.00",
        "A3 A4 10.00 90.00 0.00 -9.90 -9.90 -9.90 -9.90",
    ]);
    let lines = lrud_lines(&warnings);
    println!(
        "cross-section lines: {} {lines:?} (expected 1)",
        lines.len()
    );
    assert_eq!(
        lines,
        [
            "Survey 'A': 2 cross-sections were superseded by others measured at the same \
             stations and were dropped (the survey records LRUDs at each shot's To station)."
        ]
    );
}

/// A section every shot leaving its station can take is not superseded: the
/// loop A1 -> A2 -> A3 -> A1 brings A1's section round to the first shot.
#[test]
fn a_section_a_shot_takes_is_not_counted() {
    let warnings = warnings_for(&[
        "A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00",
        "A2 A3 10.00 0.00 0.00 2.00 2.00 2.00 2.00",
        "A3 A1 14.14 225.00 0.00 3.00 3.00 3.00 3.00",
    ]);
    let lines = lrud_lines(&warnings);
    println!(
        "cross-section lines: {} {lines:?} (expected 0)",
        lines.len()
    );
    assert!(lines.is_empty(), "{lines:?}");
}
