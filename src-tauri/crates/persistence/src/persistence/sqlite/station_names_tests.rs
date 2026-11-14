//! `station_names`: the table round-trips, a file without it still opens and
//! saves, and adding it changes nothing an outside reader of the v18 tables sees.

use rusqlite::types::ValueRef;
use tempfile::TempDir;

use super::*;
use crate::models::{SectionData, SectionId, ShotData, StationAnchor, StationKey, StationRef};

/// A two-section project, every station of the first section named after its
/// index; the second section has a comment but no names.
fn project_with_names() -> Project {
    let mut project = Project::new("Names".to_string(), None);
    let shot = |length: f64| ShotData {
        length,
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        ..Default::default()
    };
    let a = project
        .add_section(SectionData {
            shots: vec![shot(5.0), shot(6.0), shot(7.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(3.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    for index in 0..=3 {
        let key = project.key_for_ref(StationRef::new(a, index)).unwrap();
        project.station_names.insert(key, index.to_string());
    }
    let b1 = project.key_for_ref(StationRef::new(b, 1)).unwrap();
    project.station_comments.insert(b1, "sump".to_string());
    project
}

fn first_section(project: &Project) -> SectionId {
    project.sections[0].id
}

fn names_of(project: &Project, id: SectionId) -> Vec<Option<String>> {
    (0..=3)
        .map(|i| {
            project
                .station_names
                .iter()
                .find(|(k, _)| project.resolve_key(k) == Some(StationRef::new(id, i)))
                .map(|(_, v)| v.clone())
        })
        .collect()
}

fn digits(values: &[&str]) -> Vec<Option<String>> {
    values.iter().map(|v| Some((*v).to_string())).collect()
}

pub(super) fn table_exists(db: &Database, table: &str) -> bool {
    db.conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

#[test]
fn names_round_trip_through_a_saved_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("names.stalx");
    let project = project_with_names();
    let a = first_section(&project);
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();

    let loaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();

    assert_eq!(names_of(&loaded, a), digits(&["0", "1", "2", "3"]));
    assert_eq!(loaded.station_names.len(), 4);
    assert_eq!(loaded.station_comments.len(), 1, "comments are untouched");
}

#[test]
fn a_dangling_name_is_skipped_on_save() {
    let mut db = Database::in_memory().unwrap();
    let mut project = project_with_names();
    let a = first_section(&project);
    project.station_names.insert(
        StationKey::new(a, StationAnchor::AfterShot("ghost".into())),
        "ghost".to_string(),
    );

    db.save_project(&project).unwrap();

    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.station_names.len(), 4);
    assert!(!loaded.station_names.values().any(|n| n == "ghost"));
}

/// A v18 file written before `station_names` existed has no such table, and
/// with no schema bump nothing creates it on open: `open_existing` takes the
/// fast path and a read-only open never migrates. Loading must read "no
/// names", and a save into that same live connection (the SpeleoDB rename
/// path does one) must create the table rather than fail on its DELETE.
#[test]
fn a_file_without_the_table_opens_reads_and_saves() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("before-names.stalx");
    let project = project_with_names();
    let a = first_section(&project);
    {
        // A file as a pre-names build left it: a saved project, no table.
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Before".to_string(), None))
            .unwrap();
        db.conn.execute("DROP TABLE station_names", []).unwrap();
    }

    let readonly = Database::open_readonly(&path).unwrap();
    assert!(!table_exists(&readonly, "station_names"));
    assert!(readonly.load_project().unwrap().station_names.is_empty());
    drop(readonly);

    let mut live = Database::open_existing(&path).unwrap();
    assert!(
        !table_exists(&live, "station_names"),
        "premise: the fast path does not create the table"
    );
    assert!(live.load_project().unwrap().station_names.is_empty());
    live.save_project(&project)
        .expect("a save into a file without the table must create it");
    drop(live);

    let reloaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(names_of(&reloaded, a), digits(&["0", "1", "2", "3"]));
}

/// The tables OpenSpeleo's Python `stalagmight_lib` reads, each by an explicit
/// column list (see the contract test below).
pub(super) const READ_BY_STALAGMIGHT_LIB: [&str; 7] = [
    "project",
    "sections",
    "shots",
    "station_comments",
    "project_settings",
    "zero_datums",
    "station_merges",
];

/// Every row of `table` as text, in rowid order, minus `skip` columns.
pub(super) fn dump(db: &Database, table: &str, skip: &[&str]) -> Vec<Vec<String>> {
    if !table_exists(db, table) {
        return Vec::new();
    }
    let mut stmt = db
        .conn
        .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid"))
        .unwrap();
    let keep: Vec<usize> = stmt
        .column_names()
        .enumerate()
        .filter(|(_, name)| !skip.contains(name))
        .map(|(i, _)| i)
        .collect();
    let rows = stmt
        .query_map([], |row| {
            Ok(keep
                .iter()
                .map(|&i| match row.get_ref(i).unwrap() {
                    ValueRef::Null => "NULL".to_string(),
                    ValueRef::Integer(v) => v.to_string(),
                    ValueRef::Real(v) => v.to_string(),
                    ValueRef::Text(v) => String::from_utf8_lossy(v).into_owned(),
                    ValueRef::Blob(v) => format!("{v:?}"),
                })
                .collect::<Vec<_>>())
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

/// The SpeleoDB contract. OpenSpeleo's Python package `stalagmight_lib` (which
/// SpeleoDB uses, through `openspeleo_lib`, to turn an uploaded `.stalx` into a
/// map; not this repo's Rust crate of the same name) reads these tables by
/// explicit column lists and refuses any file whose `PRAGMA user_version` or
/// `project.schema_version` is not exactly 18 - see `station_texts` for the
/// version of it this was verified against. Station names must therefore
/// live only in their own table: the same project saved with and without names
/// must produce identical rows everywhere that reader looks, at version 18.
///
/// If this fails because `SCHEMA_VERSION` moved, a bump is a breaking change
/// for that reader - coordinate a release of the Python package first.
#[test]
fn names_leave_every_table_an_outside_reader_reads_unchanged() {
    assert_eq!(
        SCHEMA_VERSION, 18,
        "SpeleoDB's Python stalagmight_lib accepts exactly v18; see this test's doc comment"
    );

    let dir = TempDir::new().unwrap();
    let with_names = project_with_names();
    let mut without_names = with_names.clone();
    without_names.station_names.clear();

    let mut saved = Vec::new();
    for (label, project) in [("with", &with_names), ("without", &without_names)] {
        let path = dir.path().join(format!("{label}.stalx"));
        Database::open(&path)
            .unwrap()
            .save_project(project)
            .unwrap();
        saved.push(Database::open_readonly(&path).unwrap());
    }
    let (named, plain) = (&saved[0], &saved[1]);

    for db in [named, plain] {
        let user_version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let stored: i32 = db
            .conn
            .query_row("SELECT schema_version FROM project", [], |row| row.get(0))
            .unwrap();
        assert_eq!((user_version, stored), (18, 18));
    }
    for table in READ_BY_STALAGMIGHT_LIB {
        // `sections.created_at` is the save time, not project content.
        let skip: &[&str] = if table == "sections" {
            &["created_at"]
        } else {
            &[]
        };
        assert_eq!(
            dump(named, table, skip),
            dump(plain, table, skip),
            "{table} must not change when a project gains station names"
        );
    }
    assert_eq!(dump(named, "station_names", &[]).len(), 4);
    assert!(dump(plain, "station_names", &[]).is_empty());
}
