//! [#576 FZ-PARSE-5-D1] The core refuses an import whose computed station
//! positions are not finite. Every shot reading here is finite, so the shot
//! checks accept it, but three 1e308 m legs at 45 degrees walk a station to
//! infinity (each adds 7.1e307 m east and north): the backstop for a
//! reading no importer bounds. Synthetic configs only.

use super::VirtualShotMode;
use crate::models::{ImportPlacement, ImportSectionConfig};
use crate::test_fixtures::{shot, state_with_project};

fn config(name: &str, lengths: &[f64]) -> ImportSectionConfig {
    ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: lengths.iter().map(|&l| shot(l)).collect(),
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
    }
}

#[test]
fn an_import_that_computes_a_station_at_infinity_is_refused_whole() {
    let state = state_with_project("Finite");
    let result = state.execute_import_sections(
        vec![config("Huge legs", &[1e308, 1e308, 1e308])],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    let project = state.get_project_state().unwrap();
    println!(
        "success={} error={:?} sections after={}",
        result.success,
        result.error,
        project.sections.len()
    );
    assert!(!result.success, "the import must be refused");
    assert_eq!(
        result.error.as_deref(),
        Some(
            "Import refused: section \"Huge legs\" places a station at a position that is not \
             finite (a reading in the file is too large to compute with). Nothing was imported."
        )
    );
    assert!(project.sections.is_empty(), "nothing may land");
}

#[test]
fn an_import_with_finite_positions_still_lands() {
    let state = state_with_project("Finite");
    let result = state.execute_import_sections(
        vec![config("Ordinary", &[10.0, 1e7])],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
}

/// A Walls `.WPJ` or Compass `.MAK` project imports several sections in one
/// call: the refusal names the one that reaches infinity, not the first, and
/// none of them lands.
#[test]
fn a_multi_section_import_names_the_section_that_reaches_infinity() {
    let state = state_with_project("Finite");
    let mut huge = config("S2 huge", &[1e308, 1e308, 1e308]);
    huge.section_index = 1;
    let result = state.execute_import_sections(
        vec![config("S1 ordinary", &[10.0, 12.0]), huge],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    println!("success={} error={:?}", result.success, result.error);
    assert!(!result.success);
    assert!(
        result
            .error
            .as_deref()
            .unwrap()
            .contains("section \"S2 huge\""),
        "{:?}",
        result.error
    );
    assert!(state.get_project_state().unwrap().sections.is_empty());
}
