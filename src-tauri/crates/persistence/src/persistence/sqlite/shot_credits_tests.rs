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

/// A section's stored `sections.surveyor`.
fn legacy_surveyor(db: &Database, id: SectionId) -> Option<String> {
    db.conn
        .query_row("SELECT surveyor FROM sections WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
}

/// Kept (with an email, which the legacy text cannot carry, so the shot needs
/// a row) surveyed, Ex explored, on day 1.
fn kept_with_email() -> SurveyCredit {
    let mut kept = credit(&[], &["Ex"], day(1));
    kept.surveyors
        .push(person("Kept", Some("kept@example.org")));
    kept
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
    // The mixed section has explorers: the Ariane "explorers / surveyors"
    // form, so an older build's save keeps both roles (#718).
    assert_eq!(
        row(ids[0]),
        (
            Some("Ex One, Ex Two / Ann, Bo, Doe, John".to_string()),
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

/// One section per legacy-text case (#718): surveyors only, explorers only,
/// both roles with mixed per-shot credit, nobody, names holding `/`, and
/// same-named people in both roles.
fn roles_project() -> (Project, [SectionId; 6]) {
    let mut project = Project::new("Roles".to_string(), None);
    let surveyors_only = section_with(
        &mut project,
        vec![
            credit(&["Sue Diver", "Tom Tank"], &[], day(1)),
            credit(&["Tom Tank"], &[], day(2)),
        ],
    );
    let explorers_only = section_with(
        &mut project,
        vec![
            credit(&[], &["Ida Ice"], day(2)),
            credit(&[], &["Ida Ice", "Ex Plorer"], None),
        ],
    );
    let mixed = section_with(
        &mut project,
        vec![
            credit(&["Sue Diver"], &["Ex Plorer"], day(4)),
            credit(&["Sue Diver", "Tom Tank"], &[], day(3)),
            credit(&[], &["Ex Plorer", "Ida Ice"], None),
            SurveyCredit::default(),
        ],
    );
    let nobody = section_with(&mut project, vec![SurveyCredit::default(); 2]);
    let slashes = section_with(
        &mut project,
        vec![
            credit(&["Smith / Jones", "Lee /"], &["North / South"], day(5)),
            credit(&["/ Kim", "A/B"], &[], day(5)),
        ],
    );
    let same_named = section_with(
        &mut project,
        vec![SurveyCredit {
            surveyors: vec![person("Jo Diver", Some("jo@caves.example"))],
            explorers: vec![
                person("Jo Diver", Some("jo@caves.example")),
                person("jo  diver", Some("jo.diver@divers.example")),
            ],
            date: day(6),
        }],
    );
    project.ensure_shot_uuids();
    (
        project,
        [
            surveyors_only,
            explorers_only,
            mixed,
            nobody,
            slashes,
            same_named,
        ],
    )
}

/// A section's explorer and surveyor names, as its summary lists them.
fn role_names(project: &Project, id: SectionId) -> (Vec<String>, Vec<String>) {
    let summary = project.get_section(id).unwrap().credit_summary();
    let names = |people: Vec<Person>| people.into_iter().map(|p| p.name).collect();
    (
        names(summary.explorer_people()),
        names(summary.surveyor_people()),
    )
}

/// #718: the exact `sections.surveyor` per case. The plain surveyor list
/// (today's text) when nobody explored; the Ariane `"explorers / surveyors"`
/// form when anyone did, with an empty second half when nobody surveyed;
/// NULL for nobody. A `/` that could join the separator is written `-`, and
/// each display name appears once per half.
#[test]
fn the_legacy_surveyor_column_per_case() {
    let dir = TempDir::new().unwrap();
    let (project, ids) = roles_project();
    let path = save(&dir, "cases.stalx", &project);
    let db = Database::open_readonly(&path).unwrap();
    let texts: Vec<Option<String>> = ids.iter().map(|&id| legacy_surveyor(&db, id)).collect();
    assert_eq!(
        texts,
        [
            Some("Sue Diver, Tom Tank"),
            Some("Ida Ice, Ex Plorer /"),
            Some("Ex Plorer, Ida Ice / Sue Diver, Tom Tank"),
            None,
            Some("North - South / Smith - Jones, Lee -, - Kim, A/B"),
            Some("Jo Diver / Jo Diver"),
        ]
        .map(|t| t.map(str::to_string))
    );
    // The surveyors-only text is exactly what the writer wrote before #718.
    let before = crate::models::join_distinct_names(
        &project
            .get_section(ids[0])
            .unwrap()
            .credit_summary()
            .surveyor_people(),
    );
    assert_eq!(texts[0], before);
    // `credit_sections` records the new text, so an untouched section is not
    // mistaken for a pair retyped elsewhere: its rows apply on reopen.
    let mut stmt = db
        .conn
        .prepare(
            "SELECT count(*) FROM sections s JOIN credit_sections c ON c.section_id = s.id
             WHERE c.surveyor IS s.surveyor AND c.date IS s.date",
        )
        .unwrap();
    let matching: i64 = stmt.query_row([], |r| r.get(0)).unwrap();
    assert_eq!(matching, 6);
}

/// #718: a project saved and reopened by this build is exact, and rows are
/// written exactly where the new legacy text does not reproduce a shot.
#[test]
fn every_legacy_text_case_round_trips_exactly_with_rows_only_where_needed() {
    let dir = TempDir::new().unwrap();
    let (project, ids) = roles_project();
    let path = save(&dir, "round.stalx", &project);
    let loaded = reload(&path);
    for id in ids {
        assert_eq!(
            credits_of(&loaded, id),
            credits_of(&project, id),
            "section {id}"
        );
    }
    let twice = reload(&save(&dir, "twice.stalx", &loaded));
    for id in ids {
        assert_eq!(credits_of(&twice, id), credits_of(&project, id));
    }
    let db = Database::open_readonly(&path).unwrap();
    let rows_of = |id: SectionId| -> i64 {
        db.conn
            .query_row(
                "SELECT count(*) FROM shot_credits WHERE section_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    };
    // Surveyors only: the first shot is the aggregate, the second is not.
    // Explorers only: one explorer short, then no date. Mixed: every shot
    // differs from the aggregate. Slashes: the real names. Same-named: the
    // second explorer and the emails.
    assert_eq!(ids.map(rows_of), [1, 2, 4, 0, 2, 1]);
}

/// #718: what a save in a released build (0.5.3 and older) leaves - a fresh
/// file with only the tables it knows, so the three credit tables are gone -
/// and the same after that build's Edit Section saved too, which trims the
/// surveyor text (`surveyor.trim() || null`). Every section still lists who
/// explored and who surveyed it. What collapses is expected and pinned here:
/// every shot of a section now reads the section aggregate (earliest date),
/// names keep their legacy spelling (`" - "` for a `" / "`), and same-named
/// people with different emails become one name.
#[test]
fn an_older_builds_save_keeps_each_sections_roles() {
    const DROP: &str =
        "DROP TABLE shot_credits; DROP TABLE credit_people; DROP TABLE credit_sections;";
    const EDIT_SECTION_TRIM: &str = "UPDATE sections SET surveyor = NULLIF(trim(surveyor), '');";
    let (project, ids) = roles_project();
    let [surveyors_only, explorers_only, mixed, nobody, slashes, same_named] = ids;
    for (older_save, trimmed) in [
        (DROP.to_string(), false),
        (format!("{DROP} {EDIT_SECTION_TRIM}"), true),
    ] {
        let dir = TempDir::new().unwrap();
        let path = save(&dir, "older.stalx", &project);
        sql(&path, &older_save);
        let older = reload(&path);

        for id in [surveyors_only, explorers_only, mixed, nobody] {
            assert_eq!(
                role_names(&older, id),
                role_names(&project, id),
                "section {id} keeps its explorers and surveyors (trimmed: {trimmed})"
            );
        }
        assert_eq!(
            role_names(&older, slashes),
            (
                vec!["North - South".to_string()],
                ["Smith - Jones", "Lee -", "- Kim", "A/B"]
                    .map(str::to_string)
                    .to_vec()
            )
        );
        assert_eq!(
            role_names(&older, same_named),
            (vec!["Jo Diver".to_string()], vec!["Jo Diver".to_string()])
        );

        // Per shot: the section aggregate on every shot.
        let every_shot = |id: SectionId, expected: SurveyCredit| {
            let shots = credits_of(&older, id);
            assert_eq!(
                shots,
                vec![expected; shots.len()],
                "section {id} (trimmed: {trimmed})"
            );
        };
        every_shot(
            surveyors_only,
            credit(&["Sue Diver", "Tom Tank"], &[], day(1)),
        );
        every_shot(
            explorers_only,
            credit(&[], &["Ida Ice", "Ex Plorer"], day(2)),
        );
        every_shot(
            mixed,
            credit(
                &["Sue Diver", "Tom Tank"],
                &["Ex Plorer", "Ida Ice"],
                day(3),
            ),
        );
        every_shot(nobody, SurveyCredit::default());
        every_shot(same_named, credit(&["Jo Diver"], &["Jo Diver"], day(6)));
        // Shots that differed now agree: the per-shot detail is what is lost.
        assert_ne!(credits_of(&older, mixed), credits_of(&project, mixed));

        // And this build saves that state back unchanged.
        let resaved = reload(&save(&dir, "resaved.stalx", &older));
        for id in ids {
            assert_eq!(credits_of(&resaved, id), credits_of(&older, id));
        }
    }
}

/// #718: the explorers-only text this build wrote before it became trim-proof
/// (`"x / "`), trimmed by an older build's Edit Section to `"x /"`, still reads
/// as explorers - not as one surveyor named `"x /"`.
#[test]
fn a_trimmed_explorers_only_text_still_names_explorers() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Trimmed".to_string(), None);
    let id = section_with(&mut project, vec![SurveyCredit::default(); 2]);
    let path = save(&dir, "trimmed.stalx", &project);
    for text in ["Ida Ice, Ex Plorer / ", "Ida Ice, Ex Plorer /"] {
        sql(
            &path,
            &format!("UPDATE sections SET surveyor = '{text}', date = NULL WHERE id = {id};"),
        );
        let expected = credit(&[], &["Ida Ice", "Ex Plorer"], None);
        assert_eq!(
            credits_of(&reload(&path), id),
            vec![expected.clone(), expected],
            "{text:?}"
        );
    }
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

    // Saving it again keeps the meaning, in the same legacy text: the
    // "explorers / surveyors" form carries the explorer, no row needed (#718).
    let resaved = save(&dir, "resaved.stalx", &loaded);
    assert_eq!(credits_of(&reload(&resaved), id), credits_of(&loaded, id));
    let db = Database::open_readonly(&resaved).unwrap();
    assert_eq!(legacy_surveyor(&db, id).as_deref(), Some("nh / ds"));
    assert_eq!(count(&db, "shot_credits"), 0);
}

/// Something that edits the legacy columns in place (a hand edit, another
/// tool) keeps the credit tables and the shot uuids but retypes the section's
/// surveyor/date. The retyped text is the newer truth: every shot of the
/// section shows it, rows or not. (A released build does not get here: its
/// save drops the tables, see `an_older_builds_save_keeps_each_sections_roles`.)
#[test]
fn a_retyped_legacy_surveyor_wins_over_the_rows() {
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
        &format!("UPDATE sections SET surveyor = 'Typed Elsewhere' WHERE id = {id};"),
    );
    let loaded = reload(&path);
    assert_eq!(
        credits_of(&loaded, id),
        vec![
            credit(&["Typed Elsewhere"], &[], day(1)),
            credit(&["Typed Elsewhere"], &[], day(1)),
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
        .all(|c| c == &credit(&["Kept", "Other Person"], &["Ex"], date)));
}

/// Rows with no record of the pair they were written beside (the record table
/// lost, or a row for a section it never named) do not apply.
#[test]
fn rows_without_a_recorded_pair_do_not_apply() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Unrecorded".to_string(), None);
    let id = section_with(&mut project, vec![kept_with_email()]);
    let path = save(&dir, "unrecorded.stalx", &project);
    sql(&path, "DROP TABLE credit_sections;");
    // The row (it alone carries the email) does not apply; the pair does.
    assert_eq!(
        credits_of(&reload(&path), id),
        vec![credit(&["Kept"], &["Ex"], day(1))]
    );
}

/// Shots added without a row (new uuids, by a tool editing the file in place)
/// fall back to the pair it left untouched, while the shots with rows keep
/// theirs.
#[test]
fn shots_added_without_rows_fall_back_while_rows_hold() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Added".to_string(), None);
    let id = section_with(&mut project, vec![kept_with_email(), kept_with_email()]);
    project.ensure_shot_uuids();
    let path = save(&dir, "added.stalx", &project);
    let second = project.get_section(id).unwrap().shots[1]
        .uuid
        .clone()
        .unwrap();
    sql(
        &path,
        &format!(
            "UPDATE shots SET uuid = 'added-elsewhere' WHERE uuid = '{second}';
             INSERT INTO shot_credits (section_id, shot_uuid, date, surveyors, explorers)
                 VALUES ({id}, 'no-such-shot', NULL, '1', '');"
        ),
    );
    assert_eq!(
        credits_of(&reload(&path), id),
        vec![kept_with_email(), credit(&["Kept"], &["Ex"], day(1))]
    );
}

/// A row is used only when all of it reads back as this build writes it. A
/// row with an unreadable value, or naming a person id `credit_people` does
/// not hold readably, is skipped whole: its shot reads the legacy column
/// instead of losing what could not be read, which the next save would make
/// permanent. Valid rows beside them still apply, an empty list included.
#[test]
fn unreadable_and_dangling_rows_are_skipped_whole_not_fatal() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Bad".to_string(), None);
    let id = section_with(&mut project, vec![credit(&["A"], &[], day(1)); 10]);
    project.ensure_shot_uuids();
    let uuids: Vec<String> = project
        .get_section(id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect();
    let path = save(&dir, "bad.stalx", &project);
    let row = |shot: usize, date: &str, surveyors: &str, explorers: &str| {
        format!(
            "INSERT INTO shot_credits (section_id, shot_uuid, date, surveyors, explorers)
                 VALUES ({id}, '{}', {date}, {surveyors}, {explorers});",
            uuids[shot]
        )
    };
    sql(
        &path,
        &[
            "DELETE FROM shot_credits;
             DELETE FROM credit_people;
             INSERT INTO credit_people (id, name, email) VALUES (1, 'Good', NULL);
             INSERT INTO credit_people (id, name, email) VALUES (2, '   ', NULL);
             INSERT INTO credit_people (id, name, email) VALUES (3, X'00ff', NULL);
             INSERT INTO credit_people (id, name, email) VALUES (4, 'Mailless', X'05');"
                .to_string(),
            row(0, "NULL", "'1'", "'1'"),
            row(1, "NULL", "'1, 99'", "''"),
            row(2, "NULL", "'1,x'", "''"),
            row(3, "NULL", "'2'", "''"),
            row(4, "NULL", "'3'", "''"),
            row(5, "'not a date'", "'1'", "''"),
            row(6, "X'01'", "'1'", "''"),
            row(7, "NULL", "X'02'", "''"),
            // shot 8 has no row
            row(9, "'2023-08-04'", "''", "' 4 '"),
            format!(
                "UPDATE sections SET surveyor = 'Fallback', date = NULL WHERE id = {id};
                 UPDATE credit_sections SET surveyor = 'Fallback', date = NULL
                     WHERE section_id = {id};"
            ),
        ]
        .join("\n"),
    );
    let db = Database::open_existing(&path).unwrap();
    assert_eq!(shot_credits::read_credit_rows(&db.conn).unwrap().len(), 2);
    let got = credits_of(&db.load_project().unwrap(), id);
    let fallback = credit(&["Fallback"], &[], None);
    let mut expected = vec![fallback; 10];
    expected[0] = credit(&["Good"], &["Good"], None);
    // An unreadable email alone reads as none; an empty list is explicit.
    expected[9] = credit(&[], &["Mailless"], day(4));
    assert_eq!(got, expected);
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
    let id = section_with(&mut project, vec![kept_with_email()]);
    let path = save(&dir, "shape.stalx", &project);
    sql(
        &path,
        "DROP TABLE shot_credits;
         CREATE TABLE shot_credits (shot_uuid TEXT PRIMARY KEY, date TEXT,
             surveyors TEXT NOT NULL DEFAULT '', explorers TEXT NOT NULL DEFAULT '');",
    );
    let loaded = reload(&path);
    assert_eq!(
        credits_of(&loaded, id),
        vec![credit(&["Kept"], &["Ex"], day(1))]
    );
    let mut fixed = loaded;
    fixed.get_section_mut(id).unwrap().shots[0].set_credit(kept_with_email());
    let resaved = reload(&save(&dir, "resaved.stalx", &fixed));
    assert_eq!(credits_of(&resaved, id), vec![kept_with_email()]);
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
/// fatal, and only where they are: a row whose key or value cannot be read
/// is no row (its shot falls back, the other shot keeps its own row); a
/// record whose pair cannot be read records nothing (its section's rows do
/// not apply, the other section's still do); a person who cannot be read
/// makes the rows naming them unreadable (those shots fall back).
#[test]
fn credit_values_of_the_wrong_type_are_skipped_not_fatal() {
    let dir = TempDir::new().unwrap();
    let own = [
        credit(&["Ann Lee"], &[], day(1)),
        credit(&["Bo Ray"], &[], day(2)),
    ];
    let first_row = "rowid = (SELECT min(rowid) FROM shot_credits)";
    let second_row = "rowid = (SELECT max(rowid) FROM shot_credits)";
    for (broken, statement) in [
        (
            0,
            format!("UPDATE shot_credits SET section_id = 'not a number' WHERE {first_row};"),
        ),
        (
            1,
            format!("UPDATE shot_credits SET shot_uuid = X'0102' WHERE {second_row};"),
        ),
        (
            1,
            format!("UPDATE shot_credits SET date = X'01' WHERE {second_row};"),
        ),
        (
            0,
            "UPDATE credit_people SET name = X'41' WHERE name = 'Ann Lee';".to_string(),
        ),
    ] {
        let (path, id, fallback) = rows_needed(&dir, "one.stalx");
        sql(&path, &statement);
        let mut expected = own.to_vec();
        expected[broken] = fallback;
        assert_eq!(credits_of(&reload(&path), id), expected, "{statement}");
    }

    // A whole people table of the wrong types: every row names an unreadable
    // person, so every shot falls back - nobody's name is lost.
    let (path, id, fallback) = rows_needed(&dir, "people.stalx");
    sql(
        &path,
        "CREATE TABLE kept AS SELECT * FROM credit_people;
         DROP TABLE credit_people;
         CREATE TABLE credit_people (id TEXT, name BLOB, email INTEGER);
         INSERT INTO credit_people SELECT id, name, email FROM kept;
         DROP TABLE kept;",
    );
    assert_eq!(
        credits_of(&reload(&path), id),
        vec![fallback.clone(), fallback]
    );

    // Two sections with rows; one section's record unreadable.
    let mut project = Project::new("Records".to_string(), None);
    let a = section_with(&mut project, own.to_vec());
    let b_own = vec![
        credit(&["Cy Press"], &[], day(3)),
        credit(&["Di Vers"], &[], day(4)),
    ];
    let b = section_with(&mut project, b_own.clone());
    project.ensure_shot_uuids();
    let path = save(&dir, "records.stalx", &project);
    sql(
        &path,
        &format!("UPDATE credit_sections SET surveyor = X'41' WHERE section_id = {a};"),
    );
    let loaded = reload(&path);
    let a_fallback = credit(&["Ann Lee", "Bo Ray"], &[], day(1));
    assert_eq!(credits_of(&loaded, a), vec![a_fallback.clone(), a_fallback]);
    assert_eq!(credits_of(&loaded, b), b_own);
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
/// `sections.date` - which hold the aggregate, as one plain TEXT value and one
/// ISO date. The surveyor text is pinned: the surveyors, or the Ariane
/// `"explorers / surveyors"` form when the section has explorers (#718).
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
    assert_eq!(
        pairs,
        vec![
            (
                Some("Ex One, Ex Two / Ann, Bo, Doe, John".to_string()),
                Some("2023-08-01".to_string())
            ),
            (Some("Cy".to_string()), Some("2023-08-05".to_string())),
            (None, None),
        ]
    );
    let types: Vec<String> = {
        let mut stmt = with
            .conn
            .prepare("SELECT typeof(surveyor) FROM sections ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        types,
        ["text", "text", "null"],
        "a str | None, never a blob"
    );
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
