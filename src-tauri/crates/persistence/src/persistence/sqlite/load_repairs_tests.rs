//! [#576 B35(h)] Non-finite values are reset at the load boundary, and the load
//! report counts every value it reset.
//!
//! Moved out of `tests.rs` unchanged (#576 E8: the monolith is frozen).

use std::path::Path;

use rusqlite::Connection;
use tempfile::TempDir;

use super::upgrade_safety_tests::b6_build_file;
use super::{Database, LoadRepairs};

/// A current file with one section of two shots, then `surgery` with `{sid}`
/// replaced by that section's id.
fn b35_build_file(dir: &Path, surgery: &str) -> std::path::PathBuf {
    let path = b6_build_file(dir, "CaveA.stalx", "");
    let conn = Connection::open(&path).unwrap();
    let sid: i64 = conn
        .query_row("SELECT id FROM sections LIMIT 1", [], |r| r.get(0))
        .unwrap();
    conn.execute_batch(&surgery.replace("{sid}", &sid.to_string()))
        .unwrap();
    path
}

/// [#576 B35(h)] A station offset is loaded with the same finiteness guard as
/// shots and section offsets: a non-finite component (SQLite stores `9e999` as
/// +Inf; NaN cannot be stored) is reset to 0 at the load boundary. It used to be
/// inserted verbatim and added straight into world coordinates, so the whole
/// downstream - gap metrics, the loop-closure solve - ran on infinities.
#[test]
fn a_non_finite_station_offset_is_reset_on_load() {
    let temp = TempDir::new().unwrap();
    let path = b35_build_file(
        temp.path(),
        "INSERT INTO station_offsets (section_id, station_index, offset_x, offset_y, offset_z)
         VALUES ({sid}, 1, 9e999, 2.5, -9e999);",
    );

    let project = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    let offset = project.station_offsets.values().next().copied();
    println!("station offset after load: {offset:?}");
    assert_eq!(
        offset,
        Some((0.0, 2.5, 0.0)),
        "only the non-finite components are reset; the finite one is a real move"
    );
}

/// [#576 B35(g), RV7b-4] The project-list read keeps the zero datum the load keeps.
/// A depth reference beyond 11 km (written before that bound existed) with a valid
/// latitude and longitude opens with the datum active and flagged, but the
/// metadata read ran every datum through `GpsCoordinates::new` and dropped it - so
/// the recents card and the Global View lost the project's position and marker,
/// under a comment claiming it matched the load. The depth reference positions
/// nothing; only a non-finite coordinate, or a latitude no map can place, is left
/// out of the position.
#[test]
fn the_project_list_keeps_a_zero_datum_the_load_keeps() {
    let temp = TempDir::new().unwrap();
    let path = b35_build_file(
        temp.path(),
        "INSERT INTO zero_datums (section_id, station_index, latitude, longitude, depth_reference)
         VALUES ({sid}, 0, 20.5, -87.25, 20000.0);",
    );

    let project = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    let loaded: Vec<_> = project.zero_datums.iter().map(|zd| zd.gps).collect();
    let meta = Database::open_for_metadata(&path)
        .unwrap()
        .load_metadata(path.clone())
        .unwrap();
    println!(
        "zero datum (lat 20.5, lon -87.25, depth_reference 20000 m): load keeps {loaded:?}; \
         metadata gps_center = {:?}, gps_bounds = {:?}",
        meta.gps_center, meta.gps_bounds
    );
    assert_eq!(loaded.len(), 1, "the load keeps the finite datum");
    assert_eq!(
        meta.gps_center,
        Some((-87.25, 20.5)),
        "the recents card and the Global View marker are placed by the datum the load kept"
    );
    let bounds = meta.gps_bounds.unwrap();
    assert_eq!(
        (bounds.south, bounds.north, bounds.west, bounds.east),
        (20.5, 20.5, -87.25, -87.25)
    );
}

/// [#576 RV7b-4] What the project-list read leaves out of the position: a datum
/// the load drops (a non-finite coordinate, which SQLite stores as +/-Inf) and one
/// whose latitude no map can place - MapLibre throws on a latitude outside
/// [-90, 90], which would take the whole Global View down. The other datum still
/// positions the project.
#[test]
fn the_project_list_positions_a_project_only_by_placeable_datums() {
    let temp = TempDir::new().unwrap();
    let path = b35_build_file(
        temp.path(),
        "INSERT INTO zero_datums (section_id, station_index, latitude, longitude, depth_reference)
         VALUES ({sid}, 0, 9e999, 10.0, 0.0), ({sid}, 1, 95.0, 11.0, 0.0),
                ({sid}, 2, 45.0, 12.0, 0.0);",
    );

    let meta = Database::open_for_metadata(&path)
        .unwrap()
        .load_metadata(path.clone())
        .unwrap();
    println!(
        "zero datums (+Inf lat; lat 95; lat 45): metadata gps_center = {:?}, gps_bounds = {:?}",
        meta.gps_center, meta.gps_bounds
    );
    assert_eq!(meta.gps_center, Some((12.0, 45.0)));
    let bounds = meta.gps_bounds.unwrap();
    assert_eq!((bounds.south, bounds.north), (45.0, 45.0));
}

/// [#576 B35(h)] The load report counts every value the load reset, per kind, so
/// the open can tell the user once (the next save makes the reset permanent).
/// A clean file reports nothing.
#[test]
fn the_load_report_counts_every_value_it_reset() {
    let temp = TempDir::new().unwrap();
    let clean = b6_build_file(temp.path(), "Clean.stalx", "");
    let (_, report) = Database::open_existing(&clean)
        .unwrap()
        .load_project_with_report()
        .unwrap();
    assert!(
        report.repairs.is_empty(),
        "a clean file: {:?}",
        report.repairs
    );

    let path = b35_build_file(
        temp.path(),
        "UPDATE shots SET azimuth_in = 9e999 WHERE section_id = {sid} AND sequence_order = 0;
         INSERT OR REPLACE INTO section_offsets (section_id, offset_x, offset_y)
         VALUES ({sid}, 9e999, 1.0);
         INSERT INTO station_offsets (section_id, station_index, offset_x, offset_y, offset_z)
         VALUES ({sid}, 1, 9e999, 0.0, 0.0), ({sid}, 2, 1.0, 2.0, 3.0);",
    );
    let (_, report) = Database::open_existing(&path)
        .unwrap()
        .load_project_with_report()
        .unwrap();
    println!("load repairs: {:?}", report.repairs);
    assert_eq!(
        report.repairs,
        LoadRepairs {
            shots: 1,
            section_offsets: 1,
            station_offsets: 1,
            gps_points: 0,
            station_comments: 0,
        }
    );
}

/// [#576 Q-W30-3-D1] A station comment row past the end of its section names no
/// station (an orphan). The load drops it and counts it for the open's notice;
/// a comment on a real station - the end of the line too - is kept. It used to
/// be kept as a positional `Index` key that no edit re-anchors, so a shot later
/// added to the section put it on the new station at that position.
#[test]
fn an_orphan_station_comment_is_dropped_on_load_and_counted() {
    let temp = TempDir::new().unwrap();
    // One section of two shots: stations 0, 1 and 2.
    let path = b35_build_file(
        temp.path(),
        "INSERT INTO station_comments (section_id, station_index, comment)
         VALUES ({sid}, 2, 'comment at the end'), ({sid}, 3, 'comment past the end'),
                ({sid}, 9, 'comment far past the end');",
    );
    let (project, report) = Database::open_existing(&path)
        .unwrap()
        .load_project_with_report()
        .unwrap();
    let kept: Vec<&str> = project
        .station_comments
        .values()
        .map(String::as_str)
        .collect();
    println!("load repairs: {:?}", report.repairs);
    println!("comments kept: {kept:?}");
    assert_eq!(
        kept,
        ["comment at the end"],
        "only the in-range comment is kept"
    );
    assert_eq!(
        report.repairs.station_comments, 2,
        "both orphans are counted"
    );
    assert_eq!(
        report.repairs,
        LoadRepairs {
            station_comments: 2,
            ..LoadRepairs::default()
        },
        "and nothing else is reported"
    );
}
