//! [#576 W57-NF2] A merge's answer names what reading each source reset or
//! dropped, as an open lists it on its receipt. Its own module: the command
//! test monolith (`command_tests.rs`) is frozen by the file-size ratchet.

use tempfile::TempDir;

use super::command_tests::{
    app_with_state, assert_success, h, project_with_section, run, shot_data,
};
use super::project;
use crate::test_helpers::TestDb;

/// [#576 W57-NF2] What reading a source reset or dropped reaches the merge's
/// answer, named by source, as an open lists it on its receipt. The merge read
/// each source with `load_project`, which drops that report, so a comment row
/// past the end of its section (dropped by the read since Q-W30-3-D1) was left
/// out of the merged file with nothing shown. Synthetic: two projects this build
/// saved, then one comment row past the end written into the second with SQL.
#[test]
fn merging_names_what_reading_a_source_dropped() {
    let app = app_with_state();
    let project_a = project_with_section(vec![shot_data(1.0)]);
    let mut project_b = project_with_section(vec![shot_data(2.0)]);
    project_b.name = "Cave B".to_string();
    let (db_a, _database_a) = TestDb::with_project(&project_a).unwrap();
    let (db_b, _database_b) = TestDb::with_project(&project_b).unwrap();
    {
        let conn = rusqlite::Connection::open(db_b.path()).unwrap();
        let sid: u32 = conn
            .query_row("SELECT id FROM sections LIMIT 1", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO station_comments (section_id, station_index, comment) \
             VALUES (?1, 99, 'comment past the end')",
            rusqlite::params![sid],
        )
        .unwrap();
    }
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.stalx");

    let result = run(project::merge_projects(
        h(&app),
        vec![db_a.path().to_path_buf(), db_b.path().to_path_buf()],
        output_path.clone(),
        "Merged".to_string(),
        false,
    ));

    assert_success(&result);
    let wire = serde_json::to_value(result.unwrap()).unwrap();
    eprintln!("merge_projects answers {wire}");
    assert_eq!(
        wire["source_repairs"],
        serde_json::json!([{
            "path": db_b.path().display().to_string(),
            "name": "Cave B",
            "repairs": {
                "shots": 0,
                "section_offsets": 0,
                "station_offsets": 0,
                "gps_points": 0,
                "station_comments": 1,
            },
        }]),
        "the source whose read dropped the comment is named with its counts; \
         the clean source is not listed"
    );
}
