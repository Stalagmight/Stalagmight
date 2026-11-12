//! Per-shot survey credit (#707): every credit state survives save and reopen,
//! a file without the `shot_credits` table still opens and saves, and storing
//! credit changes nothing in the tables SpeleoDB's outside reader reads.

use chrono::NaiveDate;
use tempfile::TempDir;

use super::station_names_tests::dump;
use super::*;
use crate::models::{SectionData, ShotCredit, ShotData};

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// One section surveyed by Bob on 2026-03-02, holding a shot of each credit
/// state: inherited, explicitly unknown, and Ann's own on 2026-04-11.
fn project_with_credit() -> Project {
    let mut project = Project::new("Credit".to_string(), None);
    let shot = |credit: Option<ShotCredit>| ShotData {
        length: 5.0,
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        credit,
        ..Default::default()
    };
    project.add_section(SectionData {
        surveyor: Some("Bob".to_string()),
        date: Some(day(2026, 3, 2)),
        shots: vec![
            shot(None),
            shot(Some(ShotCredit {
                surveyor: Some(None),
                date: Some(None),
            })),
            shot(Some(ShotCredit {
                surveyor: Some(Some("Ann".to_string())),
                date: Some(Some(day(2026, 4, 11))),
            })),
        ],
        ..Default::default()
    });
    project
}

fn credits(project: &Project) -> Vec<Option<ShotCredit>> {
    project.sections[0]
        .shots
        .iter()
        .map(|shot| shot.credit.clone())
        .collect()
}

#[test]
fn every_credit_state_round_trips_through_save_and_reopen() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("credit.stalx");
    let project = project_with_credit();
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();

    let loaded = Database::open(&path).unwrap().load_project().unwrap();
    assert_eq!(credits(&loaded), credits(&project));

    let section = &loaded.sections[0];
    let effective: Vec<_> = section
        .shots
        .iter()
        .map(|shot| shot.credit().surveyor_or(section.surveyor.as_deref()))
        .collect();
    assert_eq!(effective, vec![Some("Bob"), None, Some("Ann")]);
}

#[test]
fn a_file_without_the_table_loads_with_every_shot_inheriting_and_still_saves() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("older.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project_with_credit()).unwrap();
    // The shape a file from a build before #707 has.
    db.conn.execute("DROP TABLE shot_credits", []).unwrap();

    let loaded = db.load_project().unwrap();
    assert!(
        credits(&loaded).iter().all(Option::is_none),
        "no table means every shot inherits; it must not fail the load"
    );

    // A v18 file takes the fast path on open, so nothing migrates it: the save
    // itself has to create the table.
    let project = project_with_credit();
    db.save_project(&project).unwrap();
    assert_eq!(credits(&db.load_project().unwrap()), credits(&project));
}

/// The SpeleoDB contract (see `station_names_tests`, whose test this mirrors):
/// OpenSpeleo's Python reader accepts exactly v18 and reads these tables by
/// explicit column lists, so credit must change none of their rows.
#[test]
fn credit_leaves_every_table_an_outside_reader_reads_unchanged() {
    const READ_BY_STALAGMIGHT_LIB: [&str; 7] = [
        "project",
        "sections",
        "shots",
        "station_comments",
        "project_settings",
        "zero_datums",
        "station_merges",
    ];
    let dir = TempDir::new().unwrap();
    let credited = project_with_credit();
    let mut plain = credited.clone();
    for shot in &mut plain.sections[0].shots {
        shot.credit = None;
    }

    let mut saved = Vec::new();
    for (label, project) in [("credited", &credited), ("plain", &plain)] {
        let path = dir.path().join(format!("{label}.stalx"));
        Database::open(&path)
            .unwrap()
            .save_project(project)
            .unwrap();
        saved.push(Database::open_readonly(&path).unwrap());
    }

    for db in &saved {
        let user_version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 18, "credit must not need a schema bump");
    }
    for table in READ_BY_STALAGMIGHT_LIB {
        let skip: &[&str] = if table == "sections" {
            &["created_at"]
        } else {
            &[]
        };
        assert_eq!(
            dump(&saved[0], table, skip),
            dump(&saved[1], table, skip),
            "{table} differs once credit is stored"
        );
    }
}
