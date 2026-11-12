//! Per-shot survey credit on disk: exact round trip, the legacy fallback for
//! files and shots without rows, tolerance of foreign rows, and the SpeleoDB
//! reader contract (every table it reads unchanged except the two legacy
//! values, at version 18).

use chrono::NaiveDate;
use rusqlite::types::ValueRef;
use tempfile::TempDir;

use super::*;
use crate::models::{Person, SectionData, SectionId, ShotData, SurveyCredit};

fn shot(length: f64) -> ShotData {
    ShotData {
        length,
        azimuth_in: 45.0,
        azimuth_out: 45.0,
        ..Default::default()
    }
}

fn person(name: &str, email: Option<&str>) -> Person {
    Person::new(name, email).unwrap()
}

fn credit(surveyors: &[&str], explorers: &[&str], date: Option<NaiveDate>) -> SurveyCredit {
    SurveyCredit {
        surveyors: surveyors.iter().map(|n| person(n, None)).collect(),
        explorers: explorers.iter().map(|n| person(n, None)).collect(),
        date,
    }
}

fn day(d: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(2023, 8, d)
}

fn section_with(project: &mut Project, credits: Vec<SurveyCredit>) -> SectionId {
    let id = project
        .add_section(SectionData {
            shots: credits.iter().map(|_| shot(3.0)).collect(),
            ..Default::default()
        })
        .id;
    let section = project.get_section_mut(id).unwrap();
    for (shot, c) in section.shots.iter_mut().zip(credits) {
        shot.set_credit(c);
    }
    id
}

fn save(dir: &TempDir, name: &str, project: &Project) -> std::path::PathBuf {
    let path = dir.path().join(name);
    Database::open(&path)
        .unwrap()
        .save_project(project)
        .unwrap();
    path
}

fn reload(path: &Path) -> Project {
    Database::open_existing(path)
        .unwrap()
        .load_project()
        .unwrap()
}

fn credits_of(project: &Project, id: SectionId) -> Vec<SurveyCredit> {
    project
        .get_section(id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.credit().clone())
        .collect()
}

fn count(db: &Database, table: &str) -> i64 {
    db.conn
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn sql(path: &Path, statements: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(statements).unwrap();
}

/// A project exercising every shape: emails, explorers, dates, an explicit
/// empty credit among credited shots, a section that agrees with its legacy
/// columns, and an uncredited section.
fn rich_project() -> (Project, [SectionId; 3]) {
    let mut project = Project::new("Credit".to_string(), None);
    let mut with_email = credit(&["Ann"], &["Ex One", "Ex Two"], day(2));
    with_email
        .surveyors
        .push(person("Bo", Some("bo@example.org")));
    let mixed = section_with(
        &mut project,
        vec![
            with_email,
            SurveyCredit::default(),
            credit(&["Doe, John"], &[], day(1)),
            credit(&["Ann"], &[], None),
        ],
    );
    let uniform = section_with(
        &mut project,
        vec![credit(&["Cy"], &[], day(5)), credit(&["Cy"], &[], day(5))],
    );
    let bare = section_with(&mut project, vec![SurveyCredit::default()]);
    project.ensure_shot_uuids();
    (project, [mixed, uniform, bare])
}

#[test]
fn credit_round_trips_exactly() {
    let dir = TempDir::new().unwrap();
    let (project, ids) = rich_project();
    let path = save(&dir, "rich.stalx", &project);
    let loaded = reload(&path);
    for id in ids {
        assert_eq!(
            credits_of(&loaded, id),
            credits_of(&project, id),
            "section {id}"
        );
    }
    // And a second save of what was loaded is the same file content.
    let again = save(&dir, "again.stalx", &loaded);
    let twice = reload(&again);
    for id in ids {
        assert_eq!(credits_of(&twice, id), credits_of(&project, id));
    }
}

#[test]
fn rows_are_written_only_where_the_legacy_fallback_would_differ() {
    let dir = TempDir::new().unwrap();
    let (project, _) = rich_project();
    let path = save(&dir, "rows.stalx", &project);
    let db = Database::open_readonly(&path).unwrap();
    // The mixed section needs all four rows (its legacy columns cannot say
    // "explorers", an email, or "no credit"); the uniform section matches its
    // own columns; the bare section has no credit to fall back from.
    assert_eq!(count(&db, "shot_credits"), 4);
    // Ann, Bo <email>, Ex One, Ex Two, Doe, John: each person once.
    assert_eq!(count(&db, "credit_people"), 5);

    let mut bare = Project::new("Bare".to_string(), None);
    section_with(&mut bare, vec![SurveyCredit::default(); 3]);
    let path = save(&dir, "bare.stalx", &bare);
    let db = Database::open_readonly(&path).unwrap();
    assert_eq!(count(&db, "shot_credits"), 0);
    assert_eq!(count(&db, "credit_people"), 0);
}

#[test]
fn the_legacy_columns_carry_the_section_aggregate() {
    let dir = TempDir::new().unwrap();
    let (project, ids) = rich_project();
    let path = save(&dir, "legacy.stalx", &project);
    let db = Database::open_readonly(&path).unwrap();
    let row = |id: SectionId| -> (Option<String>, Option<String>) {
        db.conn
            .query_row(
                "SELECT surveyor, date FROM sections WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    };
    assert_eq!(
        row(ids[0]),
        (
            Some("Ann, Bo, Doe, John".to_string()),
            Some("2023-08-01".to_string())
        )
    );
    assert_eq!(
        row(ids[1]),
        (Some("Cy".to_string()), Some("2023-08-05".to_string()))
    );
    assert_eq!(row(ids[2]), (None, None));
}

#[test]
fn an_old_file_without_the_tables_splits_the_legacy_surveyor() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Old".to_string(), None);
    let id = section_with(&mut project, vec![SurveyCredit::default(); 2]);
    let path = save(&dir, "old.stalx", &project);
    // What a build before per-shot credit wrote: section text, no tables.
    sql(
        &path,
        &format!(
            "DROP TABLE shot_credits; DROP TABLE credit_people;
             UPDATE sections SET surveyor = 'nh / ds', date = '2020-03-04' WHERE id = {id};"
        ),
    );
    let loaded = reload(&path);
    let expected = credit(&["ds"], &["nh"], NaiveDate::from_ymd_opt(2020, 3, 4));
    assert_eq!(credits_of(&loaded, id), vec![expected.clone(), expected]);

    // Saving it again keeps the meaning (and now writes the explorer as a row).
    let resaved = save(&dir, "resaved.stalx", &loaded);
    assert_eq!(credits_of(&reload(&resaved), id), credits_of(&loaded, id));
}

#[test]
fn an_older_builds_edits_keep_their_meaning() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Mixed".to_string(), None);
    let id = section_with(
        &mut project,
        vec![
            credit(&["Kept"], &["Ex"], day(1)),
            credit(&["Kept"], &["Ex"], day(1)),
        ],
    );
    project.ensure_shot_uuids();
    let path = save(&dir, "older.stalx", &project);
    let second_uuid = project.get_section(id).unwrap().shots[1]
        .uuid
        .clone()
        .unwrap();
    // An older build: deleted the second shot's row (it re-created that shot),
    // added a row for a shot it then deleted, and re-typed the section text.
    sql(
        &path,
        &format!(
            "DELETE FROM shot_credits WHERE shot_uuid = '{second_uuid}';
             INSERT INTO shot_credits (shot_uuid, date, surveyors, explorers)
                 VALUES ('no-such-shot', NULL, '1', '');
             UPDATE sections SET surveyor = 'Typed In Old Build', date = NULL WHERE id = {id};"
        ),
    );
    let loaded = reload(&path);
    assert_eq!(
        credits_of(&loaded, id),
        vec![
            credit(&["Kept"], &["Ex"], day(1)),
            credit(&["Typed In Old Build"], &[], None),
        ]
    );
}

#[test]
fn unreadable_and_dangling_rows_are_skipped_not_fatal() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Bad".to_string(), None);
    let id = section_with(
        &mut project,
        vec![
            credit(&["A"], &[], day(1)),
            credit(&["B"], &[], None),
            credit(&["C"], &[], None),
        ],
    );
    project.ensure_shot_uuids();
    let uuids: Vec<String> = project
        .get_section(id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect();
    let path = save(&dir, "bad.stalx", &project);
    sql(
        &path,
        &format!(
            "DELETE FROM shot_credits;
             DELETE FROM credit_people;
             INSERT INTO credit_people (id, name, email) VALUES (1, 'Good', NULL);
             INSERT INTO credit_people (id, name, email) VALUES (2, '   ', NULL);
             INSERT INTO credit_people (id, name, email) VALUES (3, X'00ff', NULL);
             INSERT INTO shot_credits (shot_uuid, date, surveyors, explorers)
                 VALUES ('{}', 'not a date', '1, 2,3, 99,x', '1');
             INSERT INTO shot_credits (shot_uuid, date, surveyors, explorers)
                 VALUES ('{}', X'01', X'02', '');
             UPDATE sections SET surveyor = 'Fallback', date = NULL WHERE id = {id};",
            uuids[0], uuids[1]
        ),
    );
    let loaded = reload(&path);
    let got = credits_of(&loaded, id);
    // Dangling ids, a blank name and a BLOB name are dropped; a bad date reads
    // as none.
    assert_eq!(got[0], credit(&["Good"], &["Good"], None));
    // A row with a BLOB date still says "explicit" for its readable parts.
    assert_eq!(got[1], SurveyCredit::default());
    // No row: the legacy column.
    assert_eq!(got[2], credit(&["Fallback"], &[], None));
}

#[test]
fn a_uuid_repeated_across_sections_gives_its_row_to_the_first_holder() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Repeat".to_string(), None);
    let a = section_with(&mut project, vec![credit(&["First"], &["E1"], None)]);
    let b = section_with(&mut project, vec![credit(&["Second"], &["E2"], None)]);
    project.get_section_mut(a).unwrap().shots[0].uuid = Some("same".to_string());
    project.get_section_mut(b).unwrap().shots[0].uuid = Some("same".to_string());
    let path = save(&dir, "repeat.stalx", &project);
    let loaded = reload(&path);
    assert_eq!(
        credits_of(&loaded, a),
        vec![credit(&["First"], &["E1"], None)]
    );
    // The second holder falls back to its own section's legacy columns.
    assert_eq!(credits_of(&loaded, b), vec![credit(&["Second"], &[], None)]);
}

/// Every row of `table` as text, in rowid order, minus `skip` columns.
fn dump(db: &Database, table: &str, skip: &[&str]) -> Vec<Vec<String>> {
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

/// The SpeleoDB contract (see `station_texts` and `shot_credits`): its Python
/// reader accepts exactly v18 and reads these seven tables by explicit column
/// lists. The same project saved with and without per-shot credit must give
/// identical rows in every one of them, except `sections.surveyor` and
/// `sections.date` - which hold the aggregate, as one string and one ISO date.
#[test]
fn credit_leaves_every_table_an_outside_reader_reads_unchanged_but_the_legacy_pair() {
    const READ_BY_STALAGMIGHT_LIB: [&str; 7] = [
        "project",
        "sections",
        "shots",
        "station_comments",
        "project_settings",
        "zero_datums",
        "station_merges",
    ];
    assert_eq!(
        SCHEMA_VERSION, 18,
        "SpeleoDB's Python stalagmight_lib accepts exactly v18"
    );
    let dir = TempDir::new().unwrap();
    let (credited, ids) = rich_project();
    let mut plain = credited.clone();
    for section in &mut plain.sections {
        for shot in &mut section.shots {
            shot.set_credit(SurveyCredit::default());
        }
    }
    let with = Database::open_readonly(&save(&dir, "with.stalx", &credited)).unwrap();
    let without = Database::open_readonly(&save(&dir, "without.stalx", &plain)).unwrap();
    for db in [&with, &without] {
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
            // `created_at` is the save time; the pair is the documented change.
            &["created_at", "surveyor", "date"]
        } else {
            &[]
        };
        assert_eq!(
            dump(&with, table, skip),
            dump(&without, table, skip),
            "{table} must not change when shots gain survey credit"
        );
    }
    // The pair: one string (or NULL) and one ISO date (or NULL) per section.
    let pairs: Vec<(Option<String>, Option<String>)> = {
        let mut stmt = with
            .conn
            .prepare("SELECT surveyor, date FROM sections ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(pairs.len(), ids.len());
    for (_, date) in &pairs {
        if let Some(date) = date {
            assert!(
                NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok(),
                "sections.date must be ONE ISO date, got {date:?}"
            );
        }
    }
    let plain_pairs: Vec<(Option<String>, Option<String>)> = {
        let mut stmt = without
            .conn
            .prepare("SELECT surveyor, date FROM sections ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(plain_pairs.iter().all(|p| p == &(None, None)));
}
