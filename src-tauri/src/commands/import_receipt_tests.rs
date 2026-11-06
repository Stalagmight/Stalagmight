//! The import receipt the IPC import commands attach (#391, #411): provenance
//! decoration, sanitizing, the defaults, the GPX path, and the zero receipt of
//! an import that landed nothing.
//!
//! Moved out of `command_tests.rs` unchanged (#576 E8: the monolith is frozen).

use tempfile::NamedTempFile;

use super::command_tests::{
    app_with_state, assert_success, h, made_for, run, shot_data, state, with_config_lock,
};
use super::import;
use crate::core::Command;
use crate::models::GpsCoordinates;

/// #391: `import_sections`' additive provenance params (used by the mNemo DMP
/// wizard) decorate the base receipt when passed, and leave it at its
/// (empty-provenance) defaults when omitted - the shape every other caller
/// still uses.
#[test]
fn test_import_sections_provenance_params_decorate_receipt() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            placement: crate::models::ImportPlacement::Origin,
            azimuth_reference: None,
        }];

        let result = run(import::import_sections(
            h(&app),
            made_for(&app),
            sections,
            None,
            None,
            None,
            None,
            Some("mNemo DMP".to_string()),
            Some("dive.dmp".to_string()),
            Some(1), // #576 B13: the file's parse warnings ride the same call and lead.
            Some(vec![
                "Section \"BBB\" (#2 in the file) has no end-of-section marker".to_string(),
            ]),
        ));
        assert_success(&result);
        let receipt = result
            .as_ref()
            .expect("the import succeeds")
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        assert_eq!(receipt.source_format, "mNemo DMP");
        assert_eq!(receipt.file_name, "dive.dmp");
        assert_eq!(receipt.sections_found, 1);
        assert_eq!(
            receipt.warnings.first().map(String::as_str),
            Some("Section \"BBB\" (#2 in the file) has no end-of-section marker")
        );
    });
}

/// PR #529 Codex review: `import_sections` (the mNemo DMP wizard's actual IPC
/// entry point) never calls `decorate_receipt` - it only overwrites
/// source_format/file_name/sections_found directly (see above and
/// `import::import_sections`). A `zero_datum_created` sanitized ONLY in
/// `decorate_receipt` would therefore reach the frontend as a raw,
/// file-controlled `ImportSectionConfig::name` through this exact command.
/// Drives the real command with a hostile name (a bidi override + 10k chars)
/// and asserts the receipt field arrives clean - proof this route, not just
/// `decorate_receipt`'s callers, is sanitized.
#[test]
fn test_import_sections_sanitizes_hostile_zero_datum_section_name() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let hostile_name = format!("A\u{202e}B{}", "x".repeat(10_000));
        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some(hostile_name),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            placement: crate::models::ImportPlacement::ZeroDatum {
                coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
                station_index: 0,
            },
            azimuth_reference: None,
        }];

        let result = run(import::import_sections(
            h(&app),
            made_for(&app),
            sections,
            None,
            None,
            None,
            None,
            Some("mNemo DMP".to_string()),
            Some("dive.dmp".to_string()),
            Some(1),
            None,
        ));
        assert_success(&result);

        let receipt = result
            .as_ref()
            .expect("the import succeeds")
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        let name = receipt
            .zero_datum_created
            .as_ref()
            .expect("field must still be Some after sanitizing");
        assert!(
            !name.contains('\u{202e}'),
            "bidi override must be stripped: {name:?}"
        );
        assert!(
            name.chars().count() <= 120,
            "must clamp to MAX_FILE_NAME_LEN chars, got {}",
            name.chars().count()
        );
    });
}

#[test]
fn test_import_sections_without_provenance_params_leaves_receipt_defaults() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            placement: crate::models::ImportPlacement::Origin,
            azimuth_reference: None,
        }];

        let result = run(import::import_sections(
            h(&app),
            made_for(&app),
            sections,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ));
        assert_success(&result);
        let receipt = result
            .as_ref()
            .expect("the import succeeds")
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        assert_eq!(receipt.sections_imported, 1);
        assert_eq!(receipt.source_format, "");
        assert_eq!(receipt.file_name, "");
        assert_eq!(receipt.sections_found, 0);
    });
}

/// #391: `execute_gpx_import` decorates the import receipt with source format
/// "GPX", no unit conversion, and `sections_found` counting the tracks/routes/
/// waypoints actually selected for import.
#[test]
fn test_execute_gpx_import_decorates_receipt() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let gpx_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <trk>
    <name>Surface Path</name>
    <trkseg>
      <trkpt lat="45.5" lon="-90.5"><ele>350.2</ele></trkpt>
      <trkpt lat="45.5005" lon="-90.4995"><ele>351.0</ele></trkpt>
      <trkpt lat="45.501" lon="-90.499"><ele>348.0</ele></trkpt>
    </trkseg>
  </trk>
</gpx>"#;
        let mut file = NamedTempFile::with_suffix(".gpx").unwrap();
        use std::io::Write;
        file.write_all(gpx_xml.as_bytes()).unwrap();

        let result = tauri::async_runtime::block_on(import::execute_gpx_import(
            app.handle().clone(),
            made_for(&app),
            file.path().to_path_buf(),
            None,
            false,
            true,
            false,
            Vec::new(),
            false,
        ));
        assert_success(&result);
        let receipt = result
            .as_ref()
            .expect("the import succeeds")
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        assert_eq!(receipt.source_format, "GPX");
        assert_eq!(receipt.sections_found, 1);
        assert_eq!(receipt.unit_conversion, None);
    });
}

/// #411: an import whose sections all vanished before they reached the command
/// (every shot excluded, or every section malformed) lands NOTHING. It must
/// still return a receipt - a zero one - because `decorate_receipt` hangs the
/// converter's "why" on that receipt and had nothing to hang it on when the
/// command returned a bare success. A silent, successful, empty import is the
/// exact failure this closes.
#[test]
fn test_import_sections_with_nothing_to_import_still_reports_a_receipt() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let result = run(import::import_sections(
            h(&app),
            made_for(&app),
            Vec::new(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ));

        assert_success(&result);
        let receipt = result
            .as_ref()
            .expect("the import succeeds")
            .import_receipt
            .as_ref()
            .expect("an import that landed nothing must still report a receipt");
        assert_eq!(receipt.sections_imported, 0);
        assert_eq!(receipt.shot_count, 0);
        assert_eq!(receipt.closures_requested, 0);
        assert!(receipt.warnings.is_empty());
    });
}
