//! [#576 E10] A Compass import lands on the open project, from a real file.
//!
//! `execute_compass_import_impl` is the command's body: parse the `.DAT`, convert
//! it, apply the sections. Driven here on a synthetic two-survey file written to a
//! temp dir - the sections land with their shots, the lengths are converted from
//! the file's decimal feet, and the receipt names the source format, the surveys
//! found and the unit conversion.
//!
//! All data is synthetic, built in code; the file lives in a temp dir only.

use std::sync::Arc;

use tempfile::TempDir;

use super::command_tests::with_config_lock;
use super::import::execute_compass_import_impl;
use crate::core::events::NullEventSink;
use crate::parsers::compass::CompassImportOptions;
use stalagmight_core::test_fixtures::state_with_project;

/// One Compass survey record of `CaveA`: `shots` legs of 10 ft heading east.
fn survey(name: &str, shots: usize) -> String {
    let mut record = format!(
        "CaveA\r\n\
         SURVEY NAME: {name}\r\n\
         SURVEY DATE: 7 10 1979\r\n\
         SURVEY TEAM:\r\n\
         Nobody\r\n\
         DECLINATION: 0.00  FORMAT: DMMDLRUDLADN  CORRECTIONS: 0.00 0.00 0.00\r\n\
         \r\n\
         FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\
         \r\n"
    );
    record.extend((1..=shots).map(|i| {
        format!(
            "{name}{i} {name}{} 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n",
            i + 1
        )
    }));
    record.push_str("\x0c\r\n");
    record
}

#[test]
fn a_compass_file_imports_its_surveys_into_the_open_project() {
    with_config_lock(|| {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("CaveA.dat");
        std::fs::write(&path, format!("{}{}", survey("A", 2), survey("B", 1)))
            .expect("write the synthetic file");
        let state = state_with_project("CaveA");

        let result = execute_compass_import_impl(
            &state,
            Arc::new(NullEventSink),
            None,
            &path,
            Some("synthetic-hash".to_owned()),
            &CompassImportOptions {
                include_excluded: false,
                apply_corrections: false,
                comments_on_stations: false,
            },
        );
        let project = state.get_project_state().expect("the project is open");
        let sections: Vec<(Option<String>, Vec<f64>)> = project
            .sections
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    s.shots.iter().map(|shot| shot.length).collect(),
                )
            })
            .collect();
        let receipt = result.import_receipt.clone().expect("a receipt");
        println!(
            "compass import: success={} error={:?}; sections {sections:?}; receipt: format {:?}, \
             found {}, imported {}, shots {}, unit conversion {:?}; tracked: {}",
            result.success,
            result.error,
            receipt.source_format,
            receipt.sections_found,
            receipt.sections_imported,
            receipt.shot_count,
            receipt.unit_conversion,
            project
                .imported_file_hashes
                .contains(&"synthetic-hash".to_owned()),
        );
        assert!(result.success, "{:?}", result.error);
        assert_eq!(sections.len(), 2, "one section per survey: {sections:?}");
        let lengths: Vec<usize> = sections.iter().map(|(_, l)| l.len()).collect();
        assert_eq!(lengths, [2, 1], "each survey's shots: {sections:?}");
        for (_, legs) in &sections {
            for length in legs {
                assert!(
                    (length - 3.048).abs() < 1e-9,
                    "10 ft is 3.048 m, got {length}"
                );
            }
        }
        assert_eq!(receipt.source_format, "Compass");
        assert_eq!(receipt.file_name, "CaveA.dat");
        assert_eq!(receipt.sections_found, 2);
        assert_eq!(receipt.sections_imported, 2);
        assert_eq!(receipt.shot_count, 3);
        assert_eq!(
            receipt.unit_conversion.as_deref(),
            Some("Lengths converted from feet to meters")
        );
        assert!(
            project
                .imported_file_hashes
                .contains(&"synthetic-hash".to_owned()),
            "the file is tracked for duplicate detection"
        );
        assert!(state.can_undo(), "the import is undoable");
    });
}
