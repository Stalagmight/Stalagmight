//! Reading project settings: unreadable and wrongly typed values.
//!
//! Lines 8754-8808 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

/// [#576 B15] A stored project setting that cannot be READ is an error, not "not
/// set". The three readers used `.optional().unwrap_or(None)`, which turned a
/// broken table into "no value", and the next save wrote the defaults over the
/// stored values. `load_project` still loads - with the defaults - and the report
/// names every key it could not read.
#[test]
fn an_unreadable_project_setting_is_an_error_not_unset() {
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("CaveB".to_string(), None);
    project.validation.warn_shot_length_max = 77.0;
    project.surface_elevation = Some(123.0);
    project.default_azimuth_reference = Some(crate::models::AzimuthReference::magnetic(
        4.5,
        crate::models::CorrectionSource::FileDeclared,
    ));
    db.save_project(&project).unwrap();
    db.conn
        .execute_batch("ALTER TABLE project_settings RENAME COLUMN value TO broken_value;")
        .unwrap();

    assert!(db.load_project_validation().is_err());
    assert!(db.load_project_surface_elevation().is_err());
    assert!(db.load_project_default_azimuth_reference().is_err());
    let (loaded, report) = db.load_project_with_report().unwrap();
    assert_eq!(
        report.unreadable_settings,
        vec![
            "validation",
            "surface_elevation",
            "default_azimuth_reference"
        ]
    );
    assert_eq!(loaded.surface_elevation, None, "the default stands in");
    assert_eq!(loaded.default_azimuth_reference, None);
    assert!(
        db.load_project().is_ok(),
        "the plain load stays tolerant for its other callers"
    );
}

/// [#576 B15] ...while a value that is merely the wrong SQLite type is
/// unparseable, like a malformed one, and reads as not set (the analysis's
/// "unparseable values keep today's warn-and-default").
#[test]
fn a_project_setting_of_the_wrong_type_reads_as_not_set() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO project_settings (key, value) VALUES ('surface_elevation', X'00FF')",
            [],
        )
        .unwrap();
    assert_eq!(db.load_project_surface_elevation().unwrap(), None);
}
