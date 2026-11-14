//! `zero_datum_extras` (#741): GPS fixes and accuracies round-trip in list
//! order, `zero_datums` keeps at most one datum per merge component, a file
//! without the table (or with a stale or corrupt one) still loads, and the
//! tables an outside reader reads do not change.

use tempfile::TempDir;

use super::station_names_tests::{dump, table_exists, READ_BY_STALAGMIGHT_LIB};
use super::*;
use crate::models::{DatumRole, SectionId, ShotData, StationRef};

/// Three sections: A (3 shots) merged at its end to B (2 shots), and C (1
/// shot) on its own. Datums, in list order: A0 places A+B (accuracy 2 m), B2
/// is a fix (hard pin), C0 places C, A2 is a fix (default accuracy). The fix
/// B2 sits BETWEEN the two placements, so order is observable.
struct Fixture {
    project: Project,
    a: SectionId,
    b: SectionId,
    c: SectionId,
}

fn fixture() -> Fixture {
    let mut project = Project::new("Datums".to_string(), None);
    let shots = |n: usize| SectionData {
        shots: (0..n)
            .map(|i| ShotData {
                length: 4.0 + i as f64,
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let a = project.add_section(shots(3)).id;
    let b = project.add_section(shots(2)).id;
    let c = project.add_section(shots(1)).id;
    project.ensure_shot_uuids();
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 3).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    let datum = |section, index, lat: f64, accuracy_m| ZeroDatum {
        station_ref: StationRef::new(section, index).into(),
        gps: GpsCoordinates::new(lat, -87.5, 1.5).unwrap(),
        accuracy_m,
    };
    project.zero_datums = vec![
        datum(a, 0, 20.1, Some(2.0)),
        datum(b, 2, 20.2, Some(0.0)),
        datum(c, 0, 20.3, None),
        datum(a, 2, 20.4, None),
    ];
    Fixture { project, a, b, c }
}

/// Each datum as (station, latitude, accuracy, role), in list order.
type Summary = Vec<(StationRef, f64, Option<f64>, Option<DatumRole>)>;

fn summary(project: &Project) -> Summary {
    project
        .zero_datums
        .iter()
        .zip(project.datum_roles())
        .map(|(zd, role)| {
            (
                project.resolve_key(&zd.station_ref).unwrap(),
                zd.gps.latitude,
                zd.accuracy_m,
                role,
            )
        })
        .collect()
}

fn zero_datum_rows(db: &Database) -> Vec<StationRef> {
    let mut stmt = db
        .conn
        .prepare("SELECT section_id, station_index FROM zero_datums ORDER BY rowid")
        .unwrap();
    stmt.query_map([], |row| Ok(StationRef::new(row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn saved_file(dir: &TempDir, name: &str, project: &Project) -> std::path::PathBuf {
    let path = dir.path().join(name);
    Database::open(&path)
        .unwrap()
        .save_project(project)
        .unwrap();
    path
}

fn load(path: &Path) -> Project {
    Database::open_existing(path)
        .unwrap()
        .load_project()
        .unwrap()
}

#[test]
fn fixes_and_accuracies_round_trip_in_list_order() {
    let dir = TempDir::new().unwrap();
    let Fixture { project, a, b, c } = fixture();
    let path = saved_file(&dir, "fixes.stalx", &project);

    let loaded = load(&path);

    let expected: Summary = vec![
        (
            StationRef::new(a, 0),
            20.1,
            Some(2.0),
            Some(DatumRole::Placement),
        ),
        (
            StationRef::new(b, 2),
            20.2,
            Some(0.0),
            Some(DatumRole::GpsFix),
        ),
        (
            StationRef::new(c, 0),
            20.3,
            None,
            Some(DatumRole::Placement),
        ),
        (StationRef::new(a, 2), 20.4, None, Some(DatumRole::GpsFix)),
    ];
    assert_eq!(summary(&project), expected, "premise: the fixture's roles");
    assert_eq!(summary(&loaded), expected);
    let reopened = Database::open_existing(&path).unwrap();
    assert_eq!(
        zero_datum_rows(&reopened),
        vec![StationRef::new(a, 0), StationRef::new(c, 0)],
        "zero_datums holds the placement datums only, in list order"
    );
    assert_eq!(dump(&reopened, "zero_datum_extras", &[]).len(), 4);
}

/// Every permutation of the given items.
fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let first = rest.remove(i);
        for mut tail in permutations(&rest) {
            tail.insert(0, first.clone());
            out.push(tail);
        }
    }
    out
}

/// Whatever the list order, `zero_datums` never holds two datums of one merge
/// component, so an older build and the outside reader see at most one, and
/// the order and accuracies still come back exactly.
#[test]
fn zero_datums_never_holds_two_datums_of_one_component() {
    let Fixture { project, .. } = fixture();
    for order in permutations(&project.zero_datums) {
        let mut ordered = project.clone();
        ordered.zero_datums = order;
        let mut db = Database::in_memory().unwrap();
        db.save_project(&ordered).unwrap();

        let rows = zero_datum_rows(&db);
        let components: Vec<SectionId> = rows
            .iter()
            .map(|r| {
                *ordered
                    .get_merged_section_ids(r.section_id)
                    .iter()
                    .min()
                    .unwrap()
            })
            .collect();
        let distinct: std::collections::HashSet<_> = components.iter().collect();
        assert_eq!(
            distinct.len(),
            rows.len(),
            "two zero_datums rows share a merge component: {rows:?}"
        );
        assert_eq!(rows, ordered.placement_datum_refs());
        assert_eq!(summary(&db.load_project().unwrap()), summary(&ordered));
    }
}

/// A project with no fix and no stored accuracy writes no extras rows, so its
/// file reads exactly as before the table existed.
#[test]
fn a_project_without_fixes_or_accuracies_writes_no_rows() {
    let Fixture { mut project, .. } = fixture();
    project.zero_datums.retain(|zd| zd.accuracy_m.is_none());
    project.zero_datums.truncate(1);
    let mut db = Database::in_memory().unwrap();
    db.save_project(&project).unwrap();
    assert!(table_exists(&db, "zero_datum_extras"));
    assert!(dump(&db, "zero_datum_extras", &[]).is_empty());
    assert_eq!(summary(&db.load_project().unwrap()), summary(&project));
}

/// A file written before the table existed has no such table, and with no
/// schema bump nothing creates it on open. It must load exactly as before -
/// the `zero_datums` rows in order, default accuracy - and a save into that
/// live connection (the SpeleoDB rename path does one) must create the table.
#[test]
fn a_file_without_the_table_loads_unchanged_and_saves() {
    let dir = TempDir::new().unwrap();
    let Fixture { project, a, c, .. } = fixture();
    let path = saved_file(&dir, "before.stalx", &project);
    {
        let db = Database::open_existing(&path).unwrap();
        db.conn.execute("DROP TABLE zero_datum_extras", []).unwrap();
    }

    let readonly = Database::open_readonly(&path).unwrap();
    assert!(!table_exists(&readonly, "zero_datum_extras"));
    let before = vec![
        (
            StationRef::new(a, 0),
            20.1,
            None,
            Some(DatumRole::Placement),
        ),
        (
            StationRef::new(c, 0),
            20.3,
            None,
            Some(DatumRole::Placement),
        ),
    ];
    assert_eq!(summary(&readonly.load_project().unwrap()), before);
    drop(readonly);

    let mut live = Database::open_existing(&path).unwrap();
    assert!(
        !table_exists(&live, "zero_datum_extras"),
        "premise: the fast path does not create the table"
    );
    assert_eq!(summary(&live.load_project().unwrap()), before);
    live.save_project(&project)
        .expect("a save into a file without the table must create it");
    drop(live);
    assert_eq!(summary(&load(&path)), summary(&project));
}

/// The SpeleoDB contract (see `station_names_tests`): GPS fixes and accuracies
/// live only in their own table, so the same project saved with and without
/// them produces identical rows everywhere OpenSpeleo's reader looks, at v18.
#[test]
fn fixes_leave_every_table_an_outside_reader_reads_unchanged() {
    assert_eq!(SCHEMA_VERSION, 18, "see station_names_tests' contract test");
    let dir = TempDir::new().unwrap();
    let Fixture {
        project: with_fixes,
        ..
    } = fixture();
    let mut without = with_fixes.clone();
    let placements: Vec<ZeroDatum> = without.placement_datums().into_iter().cloned().collect();
    without.zero_datums = placements;
    for zd in &mut without.zero_datums {
        zd.accuracy_m = None;
    }

    let fixed = Database::open_readonly(&saved_file(&dir, "with.stalx", &with_fixes)).unwrap();
    let plain = Database::open_readonly(&saved_file(&dir, "without.stalx", &without)).unwrap();
    for db in [&fixed, &plain] {
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
        let skip: &[&str] = if table == "sections" {
            &["created_at"]
        } else {
            &[]
        };
        assert_eq!(
            dump(&fixed, table, skip),
            dump(&plain, table, skip),
            "{table} must not change when a project gains GPS fixes"
        );
    }
    assert_eq!(dump(&fixed, "zero_datum_extras", &[]).len(), 4);
    assert!(dump(&plain, "zero_datum_extras", &[]).is_empty());
}

/// Corrupt rows are skipped, never fatal; the rest of the table still holds.
#[test]
fn corrupt_rows_are_skipped_and_the_load_succeeds() {
    let dir = TempDir::new().unwrap();
    let Fixture { project, a, b, c } = fixture();
    let path = saved_file(&dir, "corrupt.stalx", &project);
    {
        let db = Database::open_existing(&path).unwrap();
        db.conn
            .execute_batch(&format!(
                "
                -- an unreadable station index
                INSERT INTO zero_datum_extras VALUES (10, {b}, 'two', NULL, 0, 20.5, -87.5, 0, NULL);
                -- a fix with a latitude no GPS has
                INSERT INTO zero_datum_extras VALUES (11, {c}, 1, (SELECT uuid FROM shots WHERE section_id = {c} AND sequence_order = 0), 0, 200.0, -87.5, 0, NULL);
                -- neither a placement nor a fix
                INSERT INTO zero_datum_extras VALUES (12, {b}, 1, NULL, 7, NULL, NULL, NULL, NULL);
                -- a fix on a section the file does not hold
                INSERT INTO zero_datum_extras VALUES (13, 999, 0, NULL, 0, 20.6, -87.5, 0, NULL);
                -- an accuracy no GPS has reads as the default
                UPDATE zero_datum_extras SET accuracy_m = -1.0 WHERE position = 0;
                "
            ))
            .unwrap();
    }

    let loaded = load(&path);

    assert_eq!(
        summary(&loaded),
        vec![
            (
                StationRef::new(a, 0),
                20.1,
                None,
                Some(DatumRole::Placement)
            ),
            (
                StationRef::new(b, 2),
                20.2,
                Some(0.0),
                Some(DatumRole::GpsFix)
            ),
            (
                StationRef::new(c, 0),
                20.3,
                None,
                Some(DatumRole::Placement)
            ),
            (StationRef::new(a, 2), 20.4, None, Some(DatumRole::GpsFix)),
        ]
    );
}

/// A table of another shape, or one holding more rows than the file has
/// stations, is ignored: the datums load from `zero_datums` alone.
#[test]
fn a_table_of_another_shape_or_size_is_ignored() {
    let dir = TempDir::new().unwrap();
    let Fixture { project, a, c, .. } = fixture();
    let base = vec![
        (
            StationRef::new(a, 0),
            20.1,
            None,
            Some(DatumRole::Placement),
        ),
        (
            StationRef::new(c, 0),
            20.3,
            None,
            Some(DatumRole::Placement),
        ),
    ];
    for (name, sql) in [
        (
            "shape",
            "DROP TABLE zero_datum_extras;
             CREATE TABLE zero_datum_extras (position INTEGER PRIMARY KEY, note TEXT);
             INSERT INTO zero_datum_extras VALUES (0, 'x');",
        ),
        (
            "size",
            "INSERT INTO zero_datum_extras (position, section_id, station_index, in_zero_datums)
             WITH RECURSIVE n(i) AS (SELECT 100 UNION ALL SELECT i + 1 FROM n WHERE i < 120)
             SELECT i, 999, i, 0 FROM n;",
        ),
    ] {
        let path = saved_file(&dir, &format!("{name}.stalx"), &project);
        Database::open_existing(&path)
            .unwrap()
            .conn
            .execute_batch(sql)
            .unwrap();
        assert_eq!(summary(&load(&path)), base, "{name}");
    }
}

/// Something that rewrites `zero_datums` in place (an older build saving into
/// its live connection, a repair, another tool) leaves the table beside rows
/// it no longer describes. It is then ignored as a whole: `zero_datums` is the
/// newer truth, and the project loads as that build saw it.
#[test]
fn a_table_beside_a_rewritten_zero_datums_is_ignored() {
    let dir = TempDir::new().unwrap();
    let Fixture { project, a, c, .. } = fixture();
    let path = saved_file(&dir, "rewritten.stalx", &project);
    Database::open_existing(&path)
        .unwrap()
        .conn
        .execute(
            "UPDATE zero_datums SET station_index = 1 WHERE section_id = ?1",
            [a],
        )
        .unwrap();

    assert_eq!(
        summary(&load(&path)),
        vec![
            (
                StationRef::new(a, 1),
                20.1,
                None,
                Some(DatumRole::Placement)
            ),
            (
                StationRef::new(c, 0),
                20.3,
                None,
                Some(DatumRole::Placement)
            ),
        ]
    );
}

/// A fix whose station now ends a different shot (the shots were rewritten in
/// place) no longer names the station it was set on, so it is skipped; the
/// others still hold.
#[test]
fn a_fix_whose_station_moved_is_skipped() {
    let dir = TempDir::new().unwrap();
    let Fixture { project, a, b, c } = fixture();
    let path = saved_file(&dir, "moved.stalx", &project);
    Database::open_existing(&path)
        .unwrap()
        .conn
        .execute(
            "UPDATE shots SET uuid = 'another-shot' WHERE section_id = ?1 AND sequence_order = 1",
            [b],
        )
        .unwrap();

    assert_eq!(
        summary(&load(&path)),
        vec![
            (
                StationRef::new(a, 0),
                20.1,
                Some(2.0),
                Some(DatumRole::Placement)
            ),
            (
                StationRef::new(c, 0),
                20.3,
                None,
                Some(DatumRole::Placement)
            ),
            (StationRef::new(a, 2), 20.4, None, Some(DatumRole::GpsFix)),
        ]
    );
}
