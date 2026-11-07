//! [#576 NF-W13BE-3] A save writes the same rows in the same order for the same
//! project.
//!
//! Station offsets, station comments and section offsets live in `HashMap`s,
//! and the save inserted their rows in map order. Every map built anew (an open
//! builds them from the file) has its own random seed, so opening a project and
//! saving it again wrote its rows in another order - other rowids, other bytes -
//! though nothing had changed.

use std::path::Path;

use rusqlite::types::Value;
use rusqlite::Connection;
use tempfile::TempDir;

use super::{Database, SAVE_PROJECT_TABLES};
use crate::models::{Project, SectionData, ShotData, StationRef};

/// Three sections of twelve shots, a comment and an offset on every station,
/// and an offset on every section.
fn project() -> Project {
    let mut project = Project::new("CaveA".to_string(), None);
    let mut ids = Vec::new();
    for s in 0..3 {
        let section = project.add_section(SectionData {
            name: Some(format!("Section{s}")),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: None,
            quality: 1.0,
            shots: (0..12)
                .map(|i| ShotData {
                    length: 2.0 + f64::from(i),
                    azimuth_in: f64::from(i * 30),
                    azimuth_out: f64::from(i * 30),
                    ..Default::default()
                })
                .collect(),
        });
        ids.push(section.id);
    }
    project.ensure_shot_uuids();
    for (s, &id) in ids.iter().enumerate() {
        project.set_section_offset(id, (10.0 * (s as f64 + 1.0), -5.0));
        for i in 0..=12_u32 {
            let key = project.key_for_ref(StationRef::new(id, i)).unwrap();
            project
                .station_comments
                .insert(key.clone(), format!("StationA{s}-{i}"));
            project
                .station_offsets
                .insert(key, (f64::from(i), f64::from(i) / 2.0, 0.25));
        }
    }
    project
}

/// Every row of every table the save writes, with its rowid, in rowid order -
/// the table as stored. Nothing is masked: `sections.created_at` is stamped by
/// a section's first save and read back on open, so a save of an opened
/// project writes the value it read (#576 NF-W14BE-1).
fn rows(path: &Path) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    let mut out = Vec::new();
    for table in SAVE_PROJECT_TABLES {
        let mut stmt = conn
            .prepare(&format!("SELECT rowid, * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = stmt.column_count();
        let mut query = stmt.query([]).unwrap();
        while let Some(row) = query.next().unwrap() {
            let values: Vec<Value> = (0..columns).map(|c| row.get(c).unwrap()).collect();
            out.push(format!("{table} {values:?}"));
        }
    }
    out
}

/// The tables the save fills from a `HashMap`.
const MAP_TABLES: [&str; 3] = ["section_offsets", "station_offsets", "station_comments"];

/// The bytes on disk of each of [`MAP_TABLES`] and their primary-key indexes.
/// Each fits one leaf page here (asserted), so its root page is all of it -
/// where the order rows were inserted in shows even when their keys do not.
fn map_table_pages(path: &Path) -> Vec<(String, Vec<u8>)> {
    let conn = Connection::open(path).unwrap();
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    let page_size = usize::try_from(page_size).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT name, rootpage FROM sqlite_master \
             WHERE tbl_name IN ('section_offsets', 'station_offsets', 'station_comments') \
             ORDER BY name",
        )
        .unwrap();
    let roots: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    drop(stmt);
    drop(conn);
    let file = std::fs::read(path).unwrap();
    roots
        .into_iter()
        .map(|(name, root)| {
            let start = (usize::try_from(root).unwrap() - 1) * page_size;
            let page = file.get(start..start + page_size).unwrap().to_vec();
            // 0x0D: a table leaf page, 0x0A: an index leaf page.
            assert!(
                matches!(page.first(), Some(0x0D | 0x0A)),
                "{name} fits one leaf page"
            );
            (name, page)
        })
        .collect()
}

/// The tables whose rows (`rows`) or, for the map-held ones, pages
/// (`map_table_pages`) differ between two saves.
fn differing(a: &Path, b: &Path) -> Vec<String> {
    let (rows_a, rows_b) = (rows(a), rows(b));
    let mut out: Vec<String> = SAVE_PROJECT_TABLES
        .iter()
        .filter(|table| {
            let of = |all: &[String]| -> Vec<String> {
                all.iter()
                    .filter(|r| r.starts_with(&format!("{table} ")))
                    .cloned()
                    .collect()
            };
            of(&rows_a) != of(&rows_b)
        })
        .map(|table| format!("{table} (rows)"))
        .collect();
    for ((name, page_a), (_, page_b)) in map_table_pages(a).iter().zip(map_table_pages(b)) {
        if *page_a != page_b {
            out.push(format!("{name} (page bytes)"));
        }
    }
    out
}

/// Open a saved project and save it again, several times over: every table the
/// save writes comes out identical, row for row and rowid for rowid, and the
/// map-held tables byte for byte.
#[test]
fn saving_an_opened_project_again_writes_identical_tables() {
    let dir = TempDir::new().unwrap();
    let first = dir.path().join("CaveA.stalx");
    Database::open(&first)
        .unwrap()
        .save_project(&project())
        .unwrap();
    assert_eq!(map_table_pages(&first).len(), 2 * MAP_TABLES.len() - 1);

    let mut failed = Vec::new();
    for round in 0..8 {
        let reopened = Database::open_existing(&first)
            .unwrap()
            .load_project()
            .unwrap();
        let again = dir.path().join(format!("CaveA-{round}.stalx"));
        Database::open(&again)
            .unwrap()
            .save_project(&reopened)
            .unwrap();
        let changed = differing(&first, &again);
        println!("NF-W13BE-3 save {round} after an open: differs from the first save: {changed:?}");
        if !changed.is_empty() {
            failed.push(round);
        }
    }
    println!(
        "NF-W13BE-3 {} tables compared row by row, {} pages byte by byte, per save",
        SAVE_PROJECT_TABLES.len(),
        map_table_pages(&first).len()
    );
    assert!(
        failed.is_empty(),
        "saves {failed:?} of the same project wrote different tables"
    );
}
