//! [#576 FZ-PARSE-2] A `project_settings` value stored as TEXT that is not valid
//! UTF-8 is malformed, like any other unparseable setting: it is warned about and
//! read as not set. Found by the `stalx_raw` fuzz target: reading the cell as a
//! `rusqlite::types::Value` panicked inside rusqlite (its `From<ValueRef>`
//! conversion expects valid UTF-8), so opening such a file crashed the load
//! instead of opening the project.

use rusqlite::Connection;
use tempfile::TempDir;

use super::upgrade_safety_tests::b6_build_file;
use super::Database;

#[test]
fn a_project_setting_that_is_not_utf8_reads_as_not_set() {
    let temp = TempDir::new().unwrap();
    let path = b6_build_file(temp.path(), "CaveA.stalx", "");
    // `{"` then a lone 0xC3 lead byte: TEXT that is not UTF-8. SQLite stores the
    // bytes as given; nothing on the write side checks them.
    let conn = Connection::open(&path).unwrap();
    for key in [
        "validation",
        "surface_elevation",
        "default_azimuth_reference",
    ] {
        conn.execute(
            "INSERT OR REPLACE INTO project_settings (key, value) \
             VALUES (?1, CAST(X'7B22C328' AS TEXT))",
            [key],
        )
        .unwrap();
    }
    drop(conn);

    let db = Database::open_existing(&path).unwrap();
    let validation = db.load_project_validation();
    println!("validation setting read back as: {validation:?}");
    assert!(
        matches!(validation, Ok(None)),
        "a setting that is not UTF-8 reads as not set, got {validation:?}"
    );
    let (project, _report) = db.load_project_with_report().unwrap();
    println!("project loaded: {} section(s)", project.sections.len());
    assert_eq!(
        project.sections.len(),
        1,
        "the rest of the file still loads"
    );
}
