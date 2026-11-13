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

/// Two DIFFERENT people who share a display name: the legacy column names
/// them once (older builds and SpeleoDB show it as-is, and "Jo Diver, Jo
/// Diver" reads as a duplicate), while the shot keeps both, emails included,
/// because the writer compares each shot against what the legacy text reads
/// back as and writes the rows that differ.
#[test]
fn same_named_people_are_one_name_in_the_legacy_column_and_both_on_reopen() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Jo".to_string(), None);
    let both = SurveyCredit {
        surveyors: vec![
            person("Jo Diver", Some("jo@caves.example")),
            person("jo  diver", Some("jo.diver@divers.example")),
        ],
        explorers: Vec::new(),
        date: day(3),
    };
    let only_one = SurveyCredit {
        surveyors: vec![person("Jo Diver", None)],
        explorers: Vec::new(),
        date: day(3),
    };
    let id = section_with(&mut project, vec![both.clone(), only_one.clone()]);
    let path = save(&dir, "jo.stalx", &project);

    let db = Database::open_readonly(&path).unwrap();
    let surveyor: Option<String> = db
        .conn
        .query_row("SELECT surveyor FROM sections WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(surveyor.as_deref(), Some("Jo Diver"));
    // The shot with both people needs its row; the one the legacy text
    // reproduces exactly does not.
    assert_eq!(count(&db, "shot_credits"), 1);
    drop(db);

    assert_eq!(credits_of(&reload(&path), id), vec![both, only_one]);
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

/// An older build keeps the credit tables (it does not know them) and the
/// shot uuids, but edits the section's surveyor/date in the legacy columns.
/// Its retyped text is the newer truth: every shot of the section shows it,
/// rows or not.
#[test]
fn an_older_builds_retyped_surveyor_wins_over_the_rows() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Mixed".to_string(), None);
    let id = section_with(
        &mut project,
        vec![
            credit(&["Kept"], &["Ex"], day(1)),
            credit(&["Other Person"], &[], day(2)),
        ],
    );
    let untouched = section_with(&mut project, vec![credit(&["Stays"], &["Too"], day(3))]);
    project.ensure_shot_uuids();
    let path = save(&dir, "older.stalx", &project);
    sql(
        &path,
        &format!("UPDATE sections SET surveyor = 'Typed In Old Build' WHERE id = {id};"),
    );
    let loaded = reload(&path);
    assert_eq!(
        credits_of(&loaded, id),
        vec![
            credit(&["Typed In Old Build"], &[], day(1)),
            credit(&["Typed In Old Build"], &[], day(1)),
        ]
    );
    assert_eq!(
        credits_of(&loaded, untouched),
        vec![credit(&["Stays"], &["Too"], day(3))],
        "a section whose pair is untouched keeps its rows"
    );

    // A retyped date alone counts too.
    let path = save(&dir, "older-date.stalx", &project);
    sql(
        &path,
        &format!("UPDATE sections SET date = '2001-02-03' WHERE id = {id};"),
    );
    let loaded = reload(&path);
    let date = NaiveDate::from_ymd_opt(2001, 2, 3);
    assert!(credits_of(&loaded, id)
        .iter()
        .all(|c| c == &credit(&["Kept", "Other Person"], &[], date)));
}

/// Rows with no record of the pair they were written beside (the record table
/// lost, or a row for a section it never named) do not apply.
#[test]
fn rows_without_a_recorded_pair_do_not_apply() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Unrecorded".to_string(), None);
    let id = section_with(&mut project, vec![credit(&["Kept"], &["Ex"], day(1))]);
    let path = save(&dir, "unrecorded.stalx", &project);
    sql(&path, "DROP TABLE credit_sections;");
    assert_eq!(
        credits_of(&reload(&path), id),
        vec![credit(&["Kept"], &[], day(1))]
    );
}

/// Shots an older build added (new uuids, no row) fall back to the pair it
/// left untouched, while the shots with rows keep theirs.
#[test]
fn an_older_builds_new_shots_fall_back_while_rows_hold() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Added".to_string(), None);
    let id = section_with(
        &mut project,
        vec![
            credit(&["Kept"], &["Ex"], day(1)),
            credit(&["Kept"], &["Ex"], day(1)),
        ],
    );
    project.ensure_shot_uuids();
    let path = save(&dir, "added.stalx", &project);
    let second = project.get_section(id).unwrap().shots[1]
        .uuid
        .clone()
        .unwrap();
    sql(
        &path,
        &format!(
            "UPDATE shots SET uuid = 'added-by-old-build' WHERE uuid = '{second}';
             INSERT INTO shot_credits (section_id, shot_uuid, date, surveyors, explorers)
                 VALUES ({id}, 'no-such-shot', NULL, '1', '');"
        ),
    );
    assert_eq!(
        credits_of(&reload(&path), id),
        vec![
            credit(&["Kept"], &["Ex"], day(1)),
            credit(&["Kept"], &[], day(1)),
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
             INSERT INTO shot_credits (section_id, shot_uuid, date, surveyors, explorers)
                 VALUES ({id}, '{}', 'not a date', '1, 2,3, 99,x', '1');
             INSERT INTO shot_credits (section_id, shot_uuid, date, surveyors, explorers)
                 VALUES ({id}, '{}', X'01', X'02', '');
             UPDATE sections SET surveyor = 'Fallback', date = NULL WHERE id = {id};
             UPDATE credit_sections SET surveyor = 'Fallback', date = NULL WHERE section_id = {id};",
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

/// A file saved before shot uuids were project-unique can hold one uuid in
/// several sections. Rows are keyed by (section, uuid), so each of those shots
/// keeps its own credit across a save and reopen.
#[test]
fn a_uuid_repeated_across_sections_keeps_each_sections_credit() {
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
    assert_eq!(
        credits_of(&loaded, b),
        vec![credit(&["Second"], &["E2"], None)]
    );
}

/// A table of the shape an unreleased build wrote (rows keyed by uuid alone)
/// is not an error: its rows are ignored, every shot falls back to the legacy
/// columns, and the next save rewrites the table in the current shape.
#[test]
fn a_credit_table_of_another_shape_is_ignored_then_replaced() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Shape".to_string(), None);
    let id = section_with(&mut project, vec![credit(&["Kept"], &["Ex"], None)]);
    let path = save(&dir, "shape.stalx", &project);
    sql(
        &path,
        "DROP TABLE shot_credits;
         CREATE TABLE shot_credits (shot_uuid TEXT PRIMARY KEY, date TEXT,
             surveyors TEXT NOT NULL DEFAULT '', explorers TEXT NOT NULL DEFAULT '');",
    );
    let loaded = reload(&path);
    assert_eq!(credits_of(&loaded, id), vec![credit(&["Kept"], &[], None)]);
    let mut fixed = loaded;
    fixed.get_section_mut(id).unwrap().shots[0].set_credit(credit(&["Kept"], &["Ex"], None));
    let resaved = reload(&save(&dir, "resaved.stalx", &fixed));
    assert_eq!(
        credits_of(&resaved, id),
        vec![credit(&["Kept"], &["Ex"], None)]
    );
}

/// A saved section of two shots that both need a row (their credits differ
/// from the section aggregate), and the credit every shot reads when the
/// rows cannot be used: the section's legacy columns, "Ann Lee, Bo Ray" and
/// day 1. (Two one-word names would read back as one `Last, First` person.)
fn rows_needed(dir: &TempDir, name: &str) -> (std::path::PathBuf, SectionId, SurveyCredit) {
    let mut project = Project::new("Rows".to_string(), None);
    let id = section_with(
        &mut project,
        vec![
            credit(&["Ann Lee"], &[], day(1)),
            credit(&["Bo Ray"], &[], day(2)),
        ],
    );
    project.ensure_shot_uuids();
    let path = save(dir, name, &project);
    let db = Database::open_readonly(&path).unwrap();
    assert_eq!(count(&db, "shot_credits"), 2, "both shots need a row");
    assert_eq!(credits_of(&reload(&path), id), credits_of(&project, id));
    (path, id, credit(&["Ann Lee", "Bo Ray"], &[], day(1)))
}

/// #717: a `credit_people` without one of the columns the reader selects (a
/// hand repair, another tool, a future change) next to valid `shot_credits`
/// rows. The project opens, and every shot reads its section's legacy credit:
/// rows whose people cannot be read are not used at all.
#[test]
fn a_credit_people_without_its_email_column_falls_back_to_the_legacy_credit() {
    let dir = TempDir::new().unwrap();
    let (path, id, fallback) = rows_needed(&dir, "no-email.stalx");
    sql(
        &path,
        "CREATE TABLE people_copy AS SELECT id, name FROM credit_people;
         DROP TABLE credit_people;
         CREATE TABLE credit_people (id INTEGER PRIMARY KEY, name TEXT);
         INSERT INTO credit_people SELECT id, name FROM people_copy;
         DROP TABLE people_copy;",
    );
    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        count(&db, "shot_credits"),
        2,
        "the rows themselves are intact"
    );
    let unreadable = shot_credits::read_credit_rows(&db.conn).unwrap_err();
    assert_eq!(
        (unreadable.table, unreadable.step),
        ("credit_people", "prepare")
    );
    let loaded = db.load_project().unwrap();
    assert_eq!(credits_of(&loaded, id), vec![fallback.clone(), fallback]);

    // The next save writes the tables in their own shape again.
    let resaved = save(&dir, "resaved.stalx", &loaded);
    assert_eq!(credits_of(&reload(&resaved), id), credits_of(&loaded, id));
}

/// #717: the same for the two other tables, and for a `credit_people` missing
/// beside `shot_credits` (the rows name people nobody can resolve).
#[test]
fn any_credit_table_missing_a_column_falls_back_to_the_legacy_credit() {
    let cases: [(&str, &str, &str); 3] = [
        (
            "shot_credits",
            "CREATE TABLE kept AS SELECT section_id, shot_uuid, date, surveyors FROM shot_credits;
             DROP TABLE shot_credits;
             ALTER TABLE kept RENAME TO shot_credits;",
            "prepare",
        ),
        (
            "credit_sections",
            "CREATE TABLE kept AS SELECT section_id, surveyor FROM credit_sections;
             DROP TABLE credit_sections;
             ALTER TABLE kept RENAME TO credit_sections;",
            "prepare",
        ),
        ("credit_people", "DROP TABLE credit_people;", "missing"),
    ];
    for (table, statements, step) in cases {
        let dir = TempDir::new().unwrap();
        let (path, id, fallback) = rows_needed(&dir, "malformed.stalx");
        sql(&path, statements);
        let db = Database::open_existing(&path).unwrap();
        let unreadable = if table == "credit_sections" {
            shot_credits::read_recorded_legacy(&db.conn, &HashMap::new()).unwrap_err()
        } else {
            shot_credits::read_credit_rows(&db.conn).unwrap_err()
        };
        assert_eq!((unreadable.table, unreadable.step), (table, step));
        let loaded = db.load_project().unwrap();
        assert_eq!(
            credits_of(&loaded, id),
            vec![fallback.clone(), fallback],
            "{table}"
        );
    }
}

/// #717: a credit table that is a VIEW. The app's open refuses any view
/// before reading anything (#608); the credit read, reached on such a
/// connection anyway, treats it as unreadable - even a view showing valid
/// rows - and every shot falls back.
#[test]
fn a_credit_table_that_is_a_view_is_refused_at_open_and_never_fatal_to_read() {
    for table in ["credit_people", "shot_credits", "credit_sections"] {
        let dir = TempDir::new().unwrap();
        let (path, id, fallback) = rows_needed(&dir, "view.stalx");
        sql(
            &path,
            &format!(
                "CREATE TABLE kept AS SELECT * FROM {table};
                 DROP TABLE {table};
                 CREATE VIEW {table} AS SELECT * FROM kept;"
            ),
        );
        let refused = Database::open_existing(&path).err().unwrap();
        assert!(
            refused
                .chain()
                .any(|c| c.downcast_ref::<RefusedSchemaObject>().is_some()),
            "{table}: {refused:#}"
        );
        let db = Database {
            conn: Connection::open(&path).unwrap(),
        };
        let loaded = db.load_project().unwrap();
        assert_eq!(
            credits_of(&loaded, id),
            vec![fallback.clone(), fallback],
            "{table}"
        );
    }
}

/// #717: values of the wrong type inside readable tables are skipped, never
/// fatal. A row whose key cannot be read is no row (its shot falls back); a
/// record whose pair cannot be read records nothing (its section's rows do
/// not apply); a person whose id or name cannot be read is dropped from the
/// rows naming them, like a dangling id.
#[test]
fn credit_values_of_the_wrong_type_are_skipped_not_fatal() {
    let dir = TempDir::new().unwrap();
    let (path, id, fallback) = rows_needed(&dir, "keys.stalx");
    sql(
        &path,
        "UPDATE shot_credits SET section_id = 'not a number'
             WHERE rowid = (SELECT min(rowid) FROM shot_credits);
         UPDATE shot_credits SET shot_uuid = X'0102'
             WHERE rowid = (SELECT max(rowid) FROM shot_credits);",
    );
    let loaded = reload(&path);
    assert_eq!(credits_of(&loaded, id), vec![fallback.clone(), fallback]);

    let (path, id, fallback) = rows_needed(&dir, "record.stalx");
    sql(&path, "UPDATE credit_sections SET surveyor = X'41';");
    assert_eq!(
        credits_of(&reload(&path), id),
        vec![fallback.clone(), fallback]
    );

    let (path, id, _) = rows_needed(&dir, "people.stalx");
    sql(
        &path,
        "CREATE TABLE kept AS SELECT * FROM credit_people;
         DROP TABLE credit_people;
         CREATE TABLE credit_people (id TEXT, name BLOB, email INTEGER);
         INSERT INTO credit_people SELECT id, name, email FROM kept;
         DROP TABLE kept;",
    );
    let db = Database::open_existing(&path).unwrap();
    let rows = shot_credits::read_credit_rows(&db.conn).unwrap();
    assert_eq!(rows.len(), 2, "the rows still read");
    assert_eq!(
        credits_of(&db.load_project().unwrap(), id),
        vec![credit(&[], &[], day(1)), credit(&[], &[], day(2))]
    );
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

/// A `sections.surveyor` holding a hundred thousand names (a crafted file:
/// nothing but the file size bounds the column) loads promptly, gives each
/// role at most `MAX_PEOPLE_PER_ROLE` people, and the section's shots SHARE the
/// one fallback credit instead of a copy each.
#[test]
fn a_huge_legacy_surveyor_loads_bounded_and_shared() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Huge".to_string(), None);
    let id = section_with(&mut project, vec![SurveyCredit::default(); 2_000]);
    let path = save(&dir, "huge.stalx", &project);
    let names: String = (0..100_000)
        .map(|i| format!("Person {i}"))
        .collect::<Vec<_>>()
        .join("; ");
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE sections SET surveyor = ?1 WHERE id = ?2",
        rusqlite::params![format!("{names} / {names}"), id],
    )
    .unwrap();
    drop(conn);

    let started = std::time::Instant::now();
    let loaded = reload(&path);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    let section = loaded.get_section(id).unwrap();
    let first = section.shots[0].credit();
    assert_eq!(first.surveyors.len(), crate::models::MAX_PEOPLE_PER_ROLE);
    assert_eq!(first.explorers.len(), crate::models::MAX_PEOPLE_PER_ROLE);
    let shared = section.shots[0].credit.as_ref().unwrap();
    assert!(section.shots.iter().all(|s| s
        .credit
        .as_ref()
        .is_some_and(|c| std::sync::Arc::ptr_eq(c, shared))));
}

/// A legacy or imported name longer than a stored person may be is clamped
/// when it is read, so a save and reopen gives back exactly what was shown.
#[test]
fn a_long_legacy_name_survives_a_save_unchanged() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Long".to_string(), None);
    let id = section_with(&mut project, vec![SurveyCredit::default(); 2]);
    let path = save(&dir, "long.stalx", &project);
    let long = "n".repeat(crate::models::MAX_PERSON_TEXT_CHARS + 30);
    sql(
        &path,
        &format!("UPDATE sections SET surveyor = '{long}' WHERE id = {id};"),
    );
    let once = reload(&path);
    let name = &credits_of(&once, id)[0].surveyors[0].name;
    assert_eq!(name.chars().count(), crate::models::MAX_PERSON_TEXT_CHARS);
    let again = reload(&save(&dir, "again.stalx", &once));
    assert_eq!(credits_of(&again, id), credits_of(&once, id));
}

/// The credit tables are project data: a SpeleoDB push snapshot keeps all
/// three (`PUSH_SNAPSHOT_LOCAL_TABLES` strips only machine-local tables), so a
/// collaborator who pulls it sees the same credit.
#[test]
fn a_push_snapshot_keeps_the_credit_tables() {
    let dir = TempDir::new().unwrap();
    let (project, ids) = rich_project();
    let path = save(&dir, "push.stalx", &project);
    Database::sanitize_push_snapshot(&path, false, None).unwrap();
    let pulled = reload(&path);
    for id in ids {
        assert_eq!(credits_of(&pulled, id), credits_of(&project, id));
    }
}

/// A section with no shots has no shot to carry its legacy surveyor/date: it
/// keeps them, and writes them back, save after save.
#[test]
fn a_shotless_section_keeps_its_legacy_pair_across_saves() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Empty".to_string(), None);
    let id = section_with(&mut project, Vec::new());
    let path = save(&dir, "empty.stalx", &project);
    sql(
        &path,
        &format!(
            "UPDATE sections SET surveyor = 'Empty Guy', date = '2020-01-02' WHERE id = {id};"
        ),
    );
    let once = reload(&path);
    let section = once.get_section(id).unwrap();
    assert_eq!(section.legacy_surveyor.as_deref(), Some("Empty Guy"));
    assert_eq!(section.legacy_date, NaiveDate::from_ymd_opt(2020, 1, 2));

    let again_path = save(&dir, "again.stalx", &once);
    let db = Database::open_readonly(&again_path).unwrap();
    let stored: (Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT surveyor, date FROM sections WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        stored,
        (
            Some("Empty Guy".to_string()),
            Some("2020-01-02".to_string())
        )
    );
    let twice = reload(&again_path);
    assert_eq!(
        twice.get_section(id).unwrap().legacy_surveyor.as_deref(),
        Some("Empty Guy")
    );
}
