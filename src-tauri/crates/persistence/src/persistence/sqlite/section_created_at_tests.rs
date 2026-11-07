//! [#576 NF-W14BE-1] `sections.created_at` holds when each section was CREATED.
//!
//! Every save overwrote the column with the save's own time, and the load never
//! read it back, so the column held the last save time. A section's first save
//! stamps it now; every later save keeps the stored value, and a section added
//! later gets its own.
//!
//! The "first save an hour ago" is injected by backdating the column of a saved
//! file, so the tests need no clock. All data is synthetic.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use tempfile::TempDir;

use super::Database;
use crate::models::{CreationStamp, Project, SectionData, ShotData};

fn section(name: &str) -> SectionData {
    SectionData {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#123456".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
    }
}

/// `(section name, created_at)` of every row, in id order.
fn created_at(path: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT name, created_at FROM sections ORDER BY id")
        .unwrap();
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Rewrite every section's `created_at` to `at`: the stamp a save made then.
fn backdate(path: &Path, at: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute("UPDATE sections SET created_at = ?1", [at])
        .unwrap();
}

/// A one-section project saved to `CaveA.stalx`, its section stamped an hour
/// ago. Returns the file and that stamp.
fn saved_an_hour_ago(dir: &TempDir) -> (std::path::PathBuf, String) {
    let path = dir.path().join("CaveA.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(section("SectionA"));
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();
    let first_save = (Utc::now() - Duration::hours(1)).to_rfc3339();
    backdate(&path, &first_save);
    (path, first_save)
}

#[test]
fn a_second_save_an_hour_later_keeps_the_first_saves_created_at() {
    let dir = TempDir::new().unwrap();
    let (path, first_save) = saved_an_hour_ago(&dir);
    let reopened = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();

    // What the app's save does: a fresh file, renamed over the destination.
    let fresh = dir.path().join("CaveA-fresh.stalx");
    Database::open(&fresh)
        .unwrap()
        .save_project(&reopened)
        .unwrap();
    // What the in-place writers do: the same file again.
    Database::open_existing(&path)
        .unwrap()
        .save_project(&reopened)
        .unwrap();

    let expected = vec![("SectionA".to_string(), first_save.clone())];
    println!("NF-W14BE-1 first save stamped SectionA {first_save}");
    println!(
        "NF-W14BE-1 after a second save to a fresh file: {:?}",
        created_at(&fresh)
    );
    println!(
        "NF-W14BE-1 after a second save in place: {:?}",
        created_at(&path)
    );
    assert_eq!(created_at(&fresh), expected, "save to a fresh file");
    assert_eq!(created_at(&path), expected, "save in place");
}

#[test]
fn a_section_added_after_the_first_save_gets_its_own_time() {
    let dir = TempDir::new().unwrap();
    let (path, first_save) = saved_an_hour_ago(&dir);
    let mut reopened = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    reopened.add_section(section("SectionB"));

    let again = dir.path().join("CaveA-again.stalx");
    let before = Utc::now();
    Database::open(&again)
        .unwrap()
        .save_project(&reopened)
        .unwrap();
    let after = Utc::now();

    let rows = created_at(&again);
    println!("NF-W14BE-1 first save stamped SectionA {first_save}");
    println!("NF-W14BE-1 after adding SectionB and saving: {rows:?}");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("SectionA".to_string(), first_save), "SectionA");
    let stamped_b = DateTime::parse_from_rfc3339(&rows[1].1)
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(rows[1].0, "SectionB");
    assert!(
        before <= stamped_b && stamped_b <= after,
        "SectionB is stamped by the save that first wrote it: {stamped_b} not in [{before}, {after}]"
    );
}

#[test]
fn a_stored_created_at_reads_back_into_the_model_and_an_unparseable_one_does_not() {
    let dir = TempDir::new().unwrap();
    let (path, first_save) = saved_an_hour_ago(&dir);
    let loaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(
        loaded.sections[0]
            .created_at
            .as_ref()
            .map(CreationStamp::as_str),
        Some(first_save.as_str())
    );

    // Not a value this app writes: it loads as "never stamped", so the next
    // save writes a time it can read rather than carrying text it cannot.
    backdate(&path, "yesterday");
    let loaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(loaded.sections[0].created_at, None);
}

/// [#576 RV14-2] A stored stamp this app can read is written back exactly as it
/// was stored. The load parsed it to UTC and the save printed that, so a stamp in
/// another offset (another tool's, `+05:00`) came back as the same moment in
/// another text: `2020-01-01T22:04:05+00:00`, in a column no edit had touched.
#[test]
fn a_readable_stamp_in_any_form_survives_a_save_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    let (path, _) = saved_an_hour_ago(&dir);
    let mut wrong = Vec::new();
    for (i, stored) in [
        "2020-01-02T03:04:05+05:00",
        "2020-01-02T03:04:05Z",
        "2020-01-02T03:04:05.120-03:30",
    ]
    .into_iter()
    .enumerate()
    {
        backdate(&path, stored);
        let reopened = Database::open_existing(&path)
            .unwrap()
            .load_project()
            .unwrap();
        let fresh = dir.path().join(format!("CaveA-{i}.stalx"));
        Database::open(&fresh)
            .unwrap()
            .save_project(&reopened)
            .unwrap();
        Database::open_existing(&path)
            .unwrap()
            .save_project(&reopened)
            .unwrap();
        for (how, file) in [("to a fresh file", &fresh), ("in place", &path)] {
            let written = created_at(file).remove(0).1;
            println!("RV14-2 stored {stored:?}; after a save {how}: {written:?}");
            if written != stored {
                wrong.push(format!("{stored} -> {written} ({how})"));
            }
        }
    }
    assert!(wrong.is_empty(), "rewritten: {wrong:?}");
}

/// [#576 RV14-2] A stored value this app cannot read, or none, is still stamped
/// by the next save: the time of that save, in the form this app writes.
#[test]
fn an_unreadable_stamp_is_stamped_anew_by_the_next_save() {
    let dir = TempDir::new().unwrap();
    let (path, _) = saved_an_hour_ago(&dir);
    for stored in ["yesterday", "", "2020-01-02 03:04:05"] {
        backdate(&path, stored);
        let reopened = Database::open_existing(&path)
            .unwrap()
            .load_project()
            .unwrap();
        let before = Utc::now();
        Database::open_existing(&path)
            .unwrap()
            .save_project(&reopened)
            .unwrap();
        let after = Utc::now();
        let written = created_at(&path).remove(0).1;
        println!("RV14-2 stored {stored:?}; after a save: {written:?}");
        let stamped = DateTime::parse_from_rfc3339(&written)
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            before <= stamped && stamped <= after,
            "{stored:?} is stamped by the save: {written} not in [{before}, {after}]"
        );
    }
}

/// [#576 NF-W18-1] The project's own `created_at` follows the same rule. The load
/// parsed it to UTC and the save printed that, so a value another tool stored in
/// another offset (`+05:00`) came back as the same moment in another text.
#[test]
fn a_project_created_at_in_any_form_survives_a_save_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    let (path, _) = saved_an_hour_ago(&dir);
    let project_created_at = |file: &Path| -> String {
        Connection::open(file)
            .unwrap()
            .query_row("SELECT created_at FROM project", [], |row| row.get(0))
            .unwrap()
    };
    let mut wrong = Vec::new();
    for (i, stored) in [
        "2020-01-02T03:04:05+05:00",
        "2020-01-02T03:04:05Z",
        "2020-01-02T03:04:05.120-03:30",
    ]
    .into_iter()
    .enumerate()
    {
        Connection::open(&path)
            .unwrap()
            .execute("UPDATE project SET created_at = ?1", [stored])
            .unwrap();
        let reopened = Database::open_existing(&path)
            .unwrap()
            .load_project()
            .unwrap();
        let fresh = dir.path().join(format!("CaveA-project-{i}.stalx"));
        Database::open(&fresh)
            .unwrap()
            .save_project(&reopened)
            .unwrap();
        Database::open_existing(&path)
            .unwrap()
            .save_project(&reopened)
            .unwrap();
        for (how, file) in [("to a fresh file", &fresh), ("in place", &path)] {
            let written = project_created_at(file);
            println!(
                "NF-W18-1 project created_at stored {stored:?}; after a save {how}: {written:?}"
            );
            if written != stored {
                wrong.push(format!("{stored} -> {written} ({how})"));
            }
        }
    }
    assert!(wrong.is_empty(), "rewritten: {wrong:?}");
}
