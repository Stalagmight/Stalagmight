//! [`Database::open_staging`]: the file a save builds before renaming it over
//! the project. It skips SQLite's per-commit syncs, and nothing else about it
//! may differ from a file written through [`Database::open`].

use tempfile::TempDir;

use super::*;
use crate::models::{LineData, ShotData, StationRef};

fn pragma_text(db: &Database, name: &str) -> String {
    db.conn
        .query_row(&format!("PRAGMA {name}"), [], |row| {
            row.get::<_, rusqlite::types::Value>(0)
        })
        .map(|value| match value {
            rusqlite::types::Value::Integer(i) => i.to_string(),
            rusqlite::types::Value::Text(t) => t,
            other => format!("{other:?}"),
        })
        .unwrap()
}

/// Two lines, a station name and a comment: enough that the file has rows in
/// the main tables and in the station side tables.
fn sample_project() -> Project {
    let mut project = Project::new("Staging".to_string(), Some("Nowhere".to_string()));
    let shot = |length: f64| ShotData {
        length,
        azimuth_in: 45.0,
        azimuth_out: 45.0,
        depth_in: 3.0,
        depth_out: 4.0,
        ..Default::default()
    };
    for (name, count) in [("Main", 4), ("Side", 2)] {
        project.add_line(LineData {
            name: Some(name.to_string()),
            color: "#336699".to_string(),
            shots: (0..count).map(|i| shot(f64::from(i) + 2.5)).collect(),
            ..Default::default()
        });
    }
    let main = project.lines[0].id;
    let key = project.key_for_ref(StationRef::new(main, 2)).unwrap();
    project.station_names.insert(key.clone(), "T2".to_string());
    project
        .station_comments
        .insert(key, "tie-off on a rock".to_string());
    project
}

/// Everything a save writes into its staging file, in the save's order.
fn write_everything(db: &mut Database, project: &Project) {
    db.save_project(project).unwrap();
    db.save_display_setting("map.zoom", "14.25").unwrap();
    db.save_display_setting("palette", "depth").unwrap();
    db.replace_history_stacks_with_timestamps(
        vec![(
            r#"{"type":"Edit"}"#.to_string(),
            r#"{"kind":"edit"}"#.to_string(),
            Some("2026-11-20T10:00:00+00:00".to_string()),
        )],
        Vec::new(),
    )
    .unwrap();
}

#[test]
fn a_staging_file_skips_the_syncs_and_a_project_file_keeps_them() {
    let dir = TempDir::new().unwrap();
    let staging = Database::open_staging(&dir.path().join("cave.stalx.tmp")).unwrap();
    let live = Database::open(&dir.path().join("cave.stalx")).unwrap();

    // 0 = OFF, 2 = FULL (SQLite's default in a rollback-journal mode).
    assert_eq!(pragma_text(&staging, "synchronous"), "0");
    assert_eq!(pragma_text(&live, "synchronous"), "2");
    // Both keep the rollback journal: the staging file needs ROLLBACK as much
    // as the project file does (see the next test).
    assert_eq!(pragma_text(&staging, "journal_mode"), "delete");
    assert_eq!(pragma_text(&live, "journal_mode"), "delete");
}

/// The copies of the undo history (and of the ignored warnings) into the
/// staging file each run in one transaction, and the save relies on a failed
/// one leaving nothing behind. With the rollback journal turned off a failed
/// transaction that had spilled pages into the file would keep them; this
/// forces the spill (a one-page cache) so that loss would show.
#[test]
fn a_failed_write_into_a_staging_file_still_rolls_back() {
    let dir = TempDir::new().unwrap();
    let db = Database::open_staging(&dir.path().join("cave.stalx.tmp")).unwrap();
    db.conn.execute_batch("PRAGMA cache_size = 1;").unwrap();
    let row = |n: usize| {
        (
            format!(r#"{{"type":"Edit","n":{n},"pad":"{}"}}"#, "x".repeat(3000)),
            format!(r#"{{"kind":"edit","n":{n},"pad":"{}"}}"#, "y".repeat(3000)),
            Some("2026-11-20T10:00:00+00:00".to_string()),
        )
    };
    db.replace_history_stacks_with_timestamps((0..2).map(row).collect(), Vec::new())
        .unwrap();
    db.conn
        .execute_batch(
            "CREATE TRIGGER reject_fifth BEFORE INSERT ON undo_stack \
             WHEN NEW.sequence = 4 BEGIN SELECT RAISE(FAIL, 'rejected'); END;",
        )
        .unwrap();

    let failed = db.replace_history_stacks_with_timestamps((10..20).map(row).collect(), Vec::new());

    assert!(failed.is_err(), "the trigger must fail the replacement");
    let kept = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(
        kept.len(),
        2,
        "the failed replacement must leave the two committed rows exactly as they were"
    );
    assert!(kept
        .iter()
        .all(|(_, command, _, _)| command.contains(r#""n":0"#) || command.contains(r#""n":1"#)));
}

/// The syncs are the only difference: a staging file holds the same bytes as
/// the same writes through a project-file connection. So the format, and what
/// every other build and reader makes of the file, is unchanged.
#[test]
fn a_staging_file_holds_the_same_bytes_as_a_synced_one() {
    let dir = TempDir::new().unwrap();
    let project = sample_project();
    let synced_path = dir.path().join("synced.stalx");
    let staged_path = dir.path().join("staged.stalx.tmp");

    let mut synced = Database::open(&synced_path).unwrap();
    write_everything(&mut synced, &project);
    drop(synced);
    let mut staged = Database::open_staging(&staged_path).unwrap();
    write_everything(&mut staged, &project);
    drop(staged);

    let synced = std::fs::read(&synced_path).unwrap();
    let staged = std::fs::read(&staged_path).unwrap();
    assert!(synced.len() > 4096, "the file must hold more than a header");
    assert!(
        synced == staged,
        "a staging file must hold exactly the bytes a synced write does \
         ({} vs {} bytes, first difference at byte {:?})",
        synced.len(),
        staged.len(),
        synced.iter().zip(&staged).position(|(a, b)| a != b)
    );
    assert!(
        !dir.path().join("staged.stalx.tmp-journal").exists(),
        "a closed staging connection leaves no journal behind"
    );
}

/// The frozen 0.6.0 reader (`stalagmight_legacy`, the switched app's only
/// reader of SQLite files) opens a staging file exactly as it opens a synced
/// one: what a one-sync (#823) build wrote migrates as what any 0.6.0 build
/// wrote, so the reader needs no change for it.
#[test]
fn the_frozen_reader_opens_a_staging_file_as_a_synced_one() {
    let dir = TempDir::new().unwrap();
    let project = sample_project();
    let synced_path = dir.path().join("synced.stalx");
    let staged_path = dir.path().join("staged.stalx");

    let mut synced = Database::open(&synced_path).unwrap();
    write_everything(&mut synced, &project);
    drop(synced);
    let mut staged = Database::open_staging(&staged_path).unwrap();
    write_everything(&mut staged, &project);
    drop(staged);

    let options = stalagmight_legacy::OpenOptions {
        with_adjusted: false,
        ..stalagmight_legacy::OpenOptions::default()
    };
    let synced = stalagmight_legacy::open_path(&synced_path, &options).unwrap();
    let staged = stalagmight_legacy::open_path(&staged_path, &options).unwrap();
    assert_eq!(synced.schema_version, SCHEMA_VERSION);
    assert_eq!(staged.schema_version, synced.schema_version);
    // `modified_at` and `file_path` are each file's own; everything else is
    // what the file holds.
    let read = |survey: &stalagmight_legacy::LegacySurvey| {
        let mut project = survey.project.clone();
        project.modified_at = project.created_at;
        project.file_path = None;
        serde_json::to_value(&project).unwrap()
    };
    assert_eq!(read(&staged), read(&synced));
    assert_eq!(read(&staged)["lines"].as_array().map(Vec::len), Some(2));
    assert_eq!(staged.raw_positions, synced.raw_positions);
}
