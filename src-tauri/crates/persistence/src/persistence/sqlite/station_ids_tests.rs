//! `station_ids` and the `next_station_id` setting: they round-trip, a file
//! without them (or with a table of another shape) still opens, and they
//! change nothing an outside reader of the v18 tables sees.

use std::collections::BTreeMap;

use tempfile::TempDir;

use super::station_names_tests::{dump, table_exists, READ_BY_STALAGMIGHT_LIB};
use super::*;
use crate::models::{LineData, ShotData, StationId, StationRef};

/// Two lines with every station named and numbered (as the app holds them),
/// with a gap in the ids (a station deleted since) and the counter past it.
fn project_with_ids() -> Project {
    let mut project = Project::new("Ids".to_string(), None);
    let shot = |length: f64| ShotData {
        length,
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        ..Default::default()
    };
    for (name, count) in [("Main", 3), ("Side", 1)] {
        project.add_line(LineData {
            name: Some(name.to_string()),
            shots: (0..count).map(|i| shot(f64::from(i) + 5.0)).collect(),
            ..Default::default()
        });
    }
    project.ensure_station_identities();
    let main = project.lines[0].id;
    let last = project.key_for_ref(StationRef::new(main, 3)).unwrap();
    project.station_ids.insert(last, 41);
    project.next_station_id = 42;
    project
}

/// `(line, index) -> id`, resolving each stored key.
fn ids_of(project: &Project) -> BTreeMap<(u32, u32), StationId> {
    project
        .station_ids
        .iter()
        .filter_map(|(key, id)| {
            let sr = project.resolve_key(key)?;
            Some(((sr.line_id, sr.station_index), *id))
        })
        .collect()
}

#[test]
fn ids_and_the_counter_round_trip_through_a_saved_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ids.stalx");
    let project = project_with_ids();
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();

    let loaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();

    assert_eq!(ids_of(&loaded), ids_of(&project));
    assert_eq!(loaded.next_station_id, 42);
    assert_eq!(
        loaded.station_names.len(),
        6,
        "every station's name is written"
    );
}

/// A v18 file from before Station IDs: no table, no key. It opens with no
/// ids (the open re-mints them), and a save into the same live connection
/// creates the table.
#[test]
fn a_file_without_the_table_or_the_key_opens_and_saves() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("before-ids.stalx");
    let project = project_with_ids();
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.conn
            .execute_batch(
                "DROP TABLE station_ids; \
                 DELETE FROM project_settings WHERE key = 'next_station_id';",
            )
            .unwrap();
    }

    let readonly = Database::open_readonly(&path).unwrap();
    let read = readonly.load_project().unwrap();
    assert!(read.station_ids.is_empty());
    assert_eq!(read.next_station_id, 0);
    drop(readonly);

    let mut live = Database::open_existing(&path).unwrap();
    assert!(!table_exists(&live, "station_ids"));
    live.save_project(&project)
        .expect("a save into a file without the table must create it");
    drop(live);
    let reloaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(ids_of(&reloaded), ids_of(&project));
}

/// A `station_ids` of another shape (another tool, a hand repair) gives no
/// ids, with a warning - never a failed open.
#[test]
fn a_table_of_another_shape_reads_as_no_ids() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("odd-ids.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project_with_ids()).unwrap();
        db.conn
            .execute_batch(
                "DROP TABLE station_ids; \
                 CREATE TABLE station_ids (something_else TEXT);",
            )
            .unwrap();
    }
    let loaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert!(loaded.station_ids.is_empty());
    assert_eq!(loaded.station_names.len(), 6);
}

/// An unreadable row (an id that is not a u32) is skipped and its station
/// re-minted, not the whole table lost.
#[test]
fn an_unreadable_row_is_skipped() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bad-row.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project_with_ids()).unwrap();
        db.conn
            .execute(
                "UPDATE station_ids SET station_id = -5 WHERE station_index = 1 \
                 AND section_id = (SELECT min(id) FROM sections)",
                [],
            )
            .unwrap();
    }
    let loaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(loaded.station_ids.len(), 5);
}

/// A crafted file: an id past `MAX_STATION_ID` reads as no id, and a counter
/// past it as no counter, so the open re-mints instead of saturating.
#[test]
fn an_id_or_a_counter_past_the_bound_reads_as_missing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("huge.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project_with_ids()).unwrap();
        db.conn
            .execute(
                "UPDATE station_ids SET station_id = 4294967295 WHERE station_index = 1 \
                 AND section_id = (SELECT min(id) FROM sections)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "UPDATE project_settings SET value = '4294967295' WHERE key = 'next_station_id'",
                [],
            )
            .unwrap();
    }
    let mut loaded = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(loaded.station_ids.len(), 5);
    assert_eq!(loaded.next_station_id, 0, "derived from the ids instead");
    loaded.ensure_station_identities();
    assert!(loaded
        .station_ids
        .values()
        .all(|id| *id < crate::models::MAX_STATION_ID));
    assert!(loaded.ensure_station_identities().is_empty());
}

/// The SpeleoDB contract (see `station_names_tests`): the tables an outside
/// reader of v18 files reads are unchanged by the ids, but for the one
/// `project_settings` row `next_station_id` - a key that reader never asks for
/// (it reads that table by key, as it does past `held_gauges` and
/// `counter_parity_shots`). The versions stay 18.
#[test]
fn ids_leave_every_table_an_outside_reader_reads_unchanged() {
    assert_eq!(SCHEMA_VERSION, 18);
    let dir = TempDir::new().unwrap();
    let with_ids = project_with_ids();
    let mut without_ids = with_ids.clone();
    without_ids.station_ids.clear();
    without_ids.next_station_id = 0;

    let mut saved = Vec::new();
    for (label, project) in [("with", &with_ids), ("without", &without_ids)] {
        let path = dir.path().join(format!("{label}.stalx"));
        Database::open(&path)
            .unwrap()
            .save_project(project)
            .unwrap();
        saved.push(Database::open_readonly(&path).unwrap());
    }
    let (numbered, plain) = (&saved[0], &saved[1]);
    for db in [numbered, plain] {
        let user_version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 18);
    }
    let without_counter = |rows: Vec<Vec<String>>| -> Vec<Vec<String>> {
        rows.into_iter()
            .filter(|row| row.first().map(String::as_str) != Some("next_station_id"))
            .collect()
    };
    for table in READ_BY_STALAGMIGHT_LIB {
        let skip: &[&str] = if table == "sections" {
            &["created_at"]
        } else {
            &[]
        };
        let (a, b) = (dump(numbered, table, skip), dump(plain, table, skip));
        if table == "project_settings" {
            assert_eq!(without_counter(a), without_counter(b), "{table}");
        } else {
            assert_eq!(a, b, "{table} must not change when stations gain ids");
        }
    }
    assert_eq!(dump(numbered, "station_ids", &[]).len(), 6);
    assert!(dump(plain, "station_ids", &[]).is_empty());
}

/// `page number -> owning table or index`, found by walking each b-tree in
/// `sqlite_schema` down from its root page. Not `dbstat`: shipped and CI
/// builds compile SQLite without it (`LIBSQLITE3_FLAGS`, #608). Overflow and
/// free pages are not walked, so they compare as unowned pages.
fn page_owners(db: &Database, bytes: &[u8], page_size: usize) -> BTreeMap<u32, String> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, rootpage FROM sqlite_schema WHERE rootpage > 0")
        .unwrap();
    let mut roots: Vec<(String, u32)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    roots.push(("sqlite_schema".to_string(), 1));
    let be_u16 = |at: usize| usize::from(u16::from_be_bytes([bytes[at], bytes[at + 1]]));
    let be_u32 = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let mut owners = BTreeMap::new();
    for (name, root) in roots {
        let mut pending = vec![root];
        while let Some(number) = pending.pop() {
            owners.insert(number, name.clone());
            let start = (usize::try_from(number).unwrap() - 1) * page_size;
            // Page 1 starts with the 100-byte file header.
            let header = start + if number == 1 { 100 } else { 0 };
            // 0x02 and 0x05 are interior (index, table) pages: each cell
            // starts with a child page number, and the header holds the
            // rightmost one. Leaves have no children.
            if !matches!(bytes[header], 0x02 | 0x05) {
                continue;
            }
            pending.push(be_u32(header + 8));
            for cell in 0..be_u16(header + 3) {
                pending.push(be_u32(start + be_u16(header + 12 + 2 * cell)));
            }
        }
    }
    owners
}

/// The station side tables are filled from `HashMap`s, whose order changes on
/// every open, and so were their rows, rowids and page splits: an unchanged
/// project saved, opened and saved again gave a different file. Only
/// `sections` may differ now (it stamps each line's `created_at` with the
/// time of the save); every other page of the two files is the same.
#[test]
fn a_resave_with_no_edit_writes_the_same_pages() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Order".to_string(), None);
    for line in 0..12_u32 {
        project.add_line(LineData {
            name: Some(format!("L{line}")),
            shots: (0..8_u32)
                .map(|i| ShotData {
                    length: f64::from(i) + 3.0,
                    azimuth_in: 45.0,
                    azimuth_out: 45.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
    }
    project.ensure_station_identities();
    let line_ids: Vec<u32> = project.lines.iter().map(|line| line.id).collect();
    for line_id in line_ids {
        project
            .line_offsets
            .insert(line_id, (f64::from(line_id), -1.0));
        for index in (0..=8_u32).step_by(2) {
            let key = project
                .key_for_ref(StationRef::new(line_id, index))
                .unwrap();
            project
                .station_comments
                .insert(key.clone(), format!("c{line_id}-{index}"));
            project
                .station_offsets
                .insert(key, (f64::from(index), 1.0, -1.0));
        }
    }

    let first = dir.path().join("first.stalx");
    Database::open(&first)
        .unwrap()
        .save_project(&project)
        .unwrap();
    let reopened = Database::open_existing(&first)
        .unwrap()
        .load_project()
        .unwrap();
    let second = dir.path().join("second.stalx");
    Database::open(&second)
        .unwrap()
        .save_project(&reopened)
        .unwrap();

    let (a, b) = (
        Database::open_readonly(&first).unwrap(),
        Database::open_readonly(&second).unwrap(),
    );
    // `(table, rows, key columns)`: the station tables are keyed by line and
    // station index, `section_offsets` by line alone.
    for (table, count, key_columns) in [
        ("station_ids", 108, 2),
        ("station_names", 108, 2),
        ("station_comments", 60, 2),
        ("station_offsets", 60, 2),
        ("section_offsets", 12, 1),
    ] {
        let rows = dump(&a, table, &[]);
        assert_eq!(rows.len(), count, "{table}");
        let positions: Vec<Vec<u32>> = rows
            .iter()
            .map(|row| {
                row[..key_columns]
                    .iter()
                    .map(|v| v.parse().unwrap())
                    .collect()
            })
            .collect();
        assert!(
            positions.is_sorted(),
            "{table} rows are not in station order"
        );
    }

    let (bytes_a, bytes_b) = (
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
    );
    assert_eq!(bytes_a.len(), bytes_b.len(), "the files differ in size");
    let page_size: i64 = a
        .conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .unwrap();
    let page_size = usize::try_from(page_size).unwrap();
    let owners = page_owners(&a, &bytes_a, page_size);
    assert_eq!(
        owners,
        page_owners(&b, &bytes_b, page_size),
        "the two files lay out differently"
    );
    let changed: Vec<String> = bytes_a
        .chunks(page_size)
        .zip(bytes_b.chunks(page_size))
        .zip(1_u32..)
        .filter(|((page_a, page_b), _)| page_a != page_b)
        .map(|(_, number)| owners.get(&number).map_or("unowned", String::as_str))
        .filter(|owner| *owner != "sections")
        .map(str::to_string)
        .collect();
    assert!(
        changed.is_empty(),
        "pages changed on a re-save: {changed:?}"
    );
}
