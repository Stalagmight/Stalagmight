//! [#576 B35(h), B35(g)] What opening a damaged file resets or drops, and how
//! the user is told.
//!
//! - Non-finite shot values, section offsets and station offsets are reset at
//!   the load boundary (station offsets used to load verbatim), and a save heals
//!   an in-memory non-finite station offset too.
//! - Every reset or dropped value is counted on the load result, so the
//!   frontend can show one notice after the open.
//! - A GPS tie point that is finite but impossible is kept and flagged by the
//!   integrity check (and carried by a merge); a non-finite one is dropped and
//!   counted.
//!
//! All data is synthetic, built in memory or written with SQL into a file this
//! build saved. No fixture is read or written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::models::{Project, SectionData, ShotData, StationKey};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

fn two_shot_project() -> Project {
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        color: "#123456".to_string(),
        shots: vec![
            ShotData {
                length: 5.0,
                ..Default::default()
            },
            ShotData {
                length: 7.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    project
}

/// A file this build saved, then `surgery` (with `{sid}` replaced by the
/// section's id) applied with plain SQL.
fn damaged_file(dir: &Path, surgery: &str) -> PathBuf {
    let path = dir.join("CaveA.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&two_shot_project()).unwrap();
    drop(db);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let sid: i64 = conn
        .query_row("SELECT id FROM sections LIMIT 1", [], |r| r.get(0))
        .unwrap();
    conn.execute_batch(&surgery.replace("{sid}", &sid.to_string()))
        .unwrap();
    path
}

fn stored_station_offsets(path: &Path) -> Vec<(f64, f64, f64)> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT offset_x, offset_y, offset_z FROM station_offsets")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// [#576 B35(h)] A save heals a non-finite station offset held in memory, as it
/// already did for shots and section offsets: an infinite one used to be written
/// to the file as it was, to poison the next open.
#[test]
fn a_save_heals_a_non_finite_station_offset() {
    let temp = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = two_shot_project();
    let sid = project.sections[0].id;
    let mut offsets: HashMap<StationKey, (f64, f64, f64)> = HashMap::new();
    offsets.insert(
        StationKey::positional(sid, 1),
        (f64::INFINITY, 1.5, f64::NEG_INFINITY),
    );
    project.station_offsets = offsets;
    state.set_project_for_tests(project);

    let path = temp.path().join("healed.stalx");
    let result = state.save_project(Some(path.clone()));
    assert!(result.success, "save: {:?}", result.error);
    let stored = stored_station_offsets(&path);
    println!("station_offsets rows written by the save: {stored:?}");
    assert_eq!(
        stored,
        vec![(0.0, 1.5, 0.0)],
        "the save must reset the non-finite components and keep the finite one"
    );
}

/// The envelope's `load_repairs` key, as the frontend decoder reads it.
#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
struct EnvelopeRepairs {
    shots: usize,
    section_offsets: usize,
    station_offsets: usize,
    gps_points: usize,
}

#[derive(serde::Deserialize)]
struct EnvelopeWithRepairs {
    load_repairs: EnvelopeRepairs,
}

/// [#576 B35(h)] The production load path hands the counts to the frontend:
/// on the load result, and as the `load_repairs` key of the binary envelope the
/// frontend actually decodes.
#[test]
fn the_load_result_and_the_binary_envelope_carry_what_the_load_reset() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = damaged_file(
        temp.path(),
        "UPDATE shots SET length = 9e999 WHERE section_id = {sid} AND sequence_order = 1;
         INSERT INTO station_offsets (section_id, station_index, offset_x, offset_y, offset_z)
         VALUES ({sid}, 1, 9e999, 0.0, 0.0), ({sid}, 2, 0.0, -9e999, 0.0);",
    );

    let state = state_with_null_sink();
    let loaded = state
        .do_load_project_full(path)
        .unwrap_or_else(|e| panic!("a damaged-but-readable file opens: {e:?}"));
    println!("load_repairs on the result: {:?}", loaded.load_repairs);
    assert_eq!(loaded.load_repairs.shots, 1);
    assert_eq!(loaded.load_repairs.station_offsets, 2);
    assert!(
        loaded
            .project
            .station_offsets
            .values()
            .all(|o| o.0.is_finite() && o.1.is_finite() && o.2.is_finite()),
        "no infinity reaches the open project"
    );

    let bytes = crate::core::load_binary::encode(loaded, 0).unwrap();
    let envelope: EnvelopeWithRepairs = rmp_serde::from_slice(&bytes).unwrap();
    println!("envelope load_repairs: {:?}", envelope.load_repairs);
    assert_eq!(
        envelope.load_repairs,
        EnvelopeRepairs {
            shots: 1,
            section_offsets: 0,
            station_offsets: 2,
            gps_points: 0,
        }
    );
}

/// The envelope's `load_repairs` key, read for the orphaned station comments.
#[derive(serde::Deserialize, Debug)]
struct EnvelopeOrphanComments {
    station_comments: usize,
}

#[derive(serde::Deserialize)]
struct EnvelopeWithOrphanComments {
    load_repairs: EnvelopeOrphanComments,
}

/// [#576 Q-W30-3-D1] A station comment row past the end of its section is
/// dropped by the production open and listed on its receipt: counted on the
/// load result and in the `load_repairs` key of the binary envelope the
/// frontend decodes (the notice after the open). The open project holds the
/// in-range comment only, and nothing blocks the save.
#[test]
fn an_orphan_station_comment_is_listed_on_the_load_receipt() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = damaged_file(
        temp.path(),
        "INSERT INTO station_comments (section_id, station_index, comment)
         VALUES ({sid}, 1, 'comment on station 1'), ({sid}, 3, 'comment past the end');",
    );

    let state = state_with_null_sink();
    let loaded = state
        .do_load_project_full(path)
        .unwrap_or_else(|e| panic!("the file opens: {e:?}"));
    let comments: Vec<String> = loaded.project.station_comments.values().cloned().collect();
    println!("load_repairs on the result: {:?}", loaded.load_repairs);
    println!("comments in the open project: {comments:?}");
    assert_eq!(loaded.load_repairs.station_comments, 1);
    assert_eq!(comments, ["comment on station 1"]);
    assert!(
        !state.integrity_check().save_blocked,
        "nothing dangles, so nothing blocks the save"
    );

    let bytes = crate::core::load_binary::encode(loaded, 0).unwrap();
    let envelope: EnvelopeWithOrphanComments = rmp_serde::from_slice(&bytes).unwrap();
    println!("envelope load_repairs: {:?}", envelope.load_repairs);
    assert_eq!(envelope.load_repairs.station_comments, 1);
}

/// [#576 B35(g)] A GPS tie point whose values are finite but impossible
/// (latitude past the pole, longitude past 180, or a depth reference beyond
/// 11 km, which builds before v0.5.0-rc5 allowed) opens AS IT IS - dropping it
/// would move that part of the cave, clamping would invent a position - and the
/// integrity check flags it, naming the station, so the user can correct it in
/// the zero-datum dialog. It used to open silently and position the cave with it.
#[test]
fn a_finite_out_of_range_gps_tie_point_opens_as_it_is_and_is_flagged() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = damaged_file(
        temp.path(),
        "INSERT INTO zero_datums (section_id, station_index, latitude, longitude, depth_reference)
         VALUES ({sid}, 0, 95.0, 190.0, 20000.0);",
    );

    let state = state_with_null_sink();
    let loaded = state
        .do_load_project_full(path)
        .unwrap_or_else(|e| panic!("a file with an out-of-range tie point opens: {e:?}"));
    let datums: Vec<_> = loaded
        .project
        .zero_datums
        .iter()
        .map(|zd| (zd.gps.latitude, zd.gps.longitude, zd.gps.depth_reference))
        .collect();
    println!("tie points after open: {datums:?}");
    assert_eq!(
        datums,
        vec![(95.0, 190.0, 20000.0)],
        "kept exactly as stored"
    );
    assert_eq!(
        loaded.load_repairs.gps_points, 0,
        "a finite point is not dropped"
    );

    let check = state.integrity_check();
    println!("integrity messages: {:?}", check.messages);
    let flagged: Vec<_> = check
        .messages
        .iter()
        .filter(|m| m.contains("GPS tie point"))
        .collect();
    assert_eq!(
        flagged.len(),
        1,
        "exactly one integrity message for the tie point"
    );
    assert!(
        flagged[0].contains("\"SectionA\" #0") && flagged[0].contains("Edit Station"),
        "the message names the station and where to fix it: {}",
        flagged[0]
    );
    assert!(
        !check.save_blocked,
        "a flag, not a save block: the user can keep working and fix it"
    );
}

/// [#576 B35(g)] A tie point with a NON-finite coordinate cannot be kept: it
/// would turn every coordinate it positions into NaN. It is dropped at the load
/// boundary and counted, so the open's notice says so.
#[test]
fn a_non_finite_gps_tie_point_is_dropped_and_reported() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = damaged_file(
        temp.path(),
        "INSERT INTO zero_datums (section_id, station_index, latitude, longitude, depth_reference)
         VALUES ({sid}, 0, 9e999, 10.0, 0.0);",
    );

    let state = state_with_null_sink();
    let loaded = state
        .do_load_project_full(path)
        .unwrap_or_else(|e| panic!("a file with a non-finite tie point opens: {e:?}"));
    println!(
        "tie points after open: {}, reported dropped: {}",
        loaded.project.zero_datums.len(),
        loaded.load_repairs.gps_points
    );
    assert!(
        loaded.project.zero_datums.is_empty(),
        "a non-finite tie point must not reach the open project"
    );
    assert_eq!(loaded.load_repairs.gps_points, 1);
}

/// [#576 B35(g)] Merge copies a flagged (finite, out-of-range) tie point into the
/// merged project instead of refusing the whole merge over it; the merged
/// project's integrity check then flags it the same way.
#[test]
fn merging_a_project_with_a_flagged_tie_point_copies_it() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let flagged = damaged_file(
        temp.path(),
        "INSERT INTO zero_datums (section_id, station_index, latitude, longitude, depth_reference)
         VALUES ({sid}, 0, 95.0, 10.0, 0.0);",
    );
    let other = temp.path().join("CaveB.stalx");
    let mut db = Database::open(&other).unwrap();
    db.save_project(&two_shot_project()).unwrap();
    drop(db);

    let state = AppState::new().unwrap();
    let output = temp.path().join("Merged.stalx");
    let result = state.merge_projects(
        vec![flagged, other],
        output.clone(),
        "Merged".to_string(),
        false,
    );
    println!("merge: success={} error={:?}", result.success, result.error);
    assert!(result.success, "merge must not refuse: {:?}", result.error);

    let merged = Database::open_existing(&output)
        .unwrap()
        .load_project()
        .unwrap();
    let latitudes: Vec<f64> = merged
        .zero_datums
        .iter()
        .map(|zd| zd.gps.latitude)
        .collect();
    println!("merged tie point latitudes: {latitudes:?}");
    assert_eq!(
        latitudes,
        vec![95.0],
        "the flagged point is carried as it is"
    );
}
