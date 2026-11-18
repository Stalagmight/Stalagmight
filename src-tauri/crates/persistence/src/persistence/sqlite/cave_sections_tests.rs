//! `cave_sections` / `cave_section_lines` (#327): the Sections round-trip, a
//! file without the tables (or with tables of another shape) still opens and
//! saves, a push snapshot keeps them, and adding them changes nothing an
//! outside reader of the v18 tables sees.

use std::path::PathBuf;

use tempfile::TempDir;

use super::station_names_tests::{dump, table_exists, READ_BY_STALAGMIGHT_LIB};
use super::*;
use crate::models::{LineData, LineId, Section, SectionId, ShotData};

/// Three lines; the first two in "Upper level", the third ungrouped, and an
/// empty "Lower level" listed FIRST in the panel.
fn project_with_sections() -> (Project, Vec<LineId>) {
    let mut project = Project::new("Sections".to_string(), None);
    let shot = |length: f64| ShotData {
        length,
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        ..Default::default()
    };
    let ids: Vec<LineId> = (0..3)
        .map(|i| {
            project
                .add_line(LineData {
                    name: Some(format!("Line {i}")),
                    shots: vec![shot(5.0 + f64::from(i))],
                    ..Default::default()
                })
                .id
        })
        .collect();
    project.ensure_shot_uuids();
    let lower = project.add_section("Lower level".to_string(), "#112233".to_string());
    let upper = project.add_section("Upper level".to_string(), "#aabbcc".to_string());
    // Panel order is the list order, not the id order.
    project.sections.swap(0, 1);
    assert_eq!(project.sections[1].id, lower);
    project.set_line_section(ids[0], Some(upper));
    project.set_line_section(ids[1], Some(upper));
    (project, ids)
}

fn save_to(dir: &TempDir, name: &str, project: &Project) -> PathBuf {
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

fn memberships(project: &Project) -> Vec<Option<u32>> {
    project.lines.iter().map(|l| l.section_id).collect()
}

#[test]
fn sections_and_memberships_round_trip_through_a_saved_file() {
    let dir = TempDir::new().unwrap();
    let (project, _) = project_with_sections();
    let loaded = reload(&save_to(&dir, "sections.stalx", &project));

    assert_eq!(
        loaded.sections, project.sections,
        "names, colours and order"
    );
    assert_eq!(memberships(&loaded), memberships(&project));
    assert_eq!(loaded.next_section_id, 2, "the saved counter");
}

/// The counter is saved (`next_cave_section_id`), so a Section deleted
/// before the save keeps its id retired after a reopen; read tolerantly as
/// max(saved, highest id + 1), and derived when the row is missing.
#[test]
fn the_section_id_counter_survives_a_reopen_and_is_read_tolerantly() {
    let dir = TempDir::new().unwrap();
    let (mut project, _) = project_with_sections();
    project.next_section_id = 7;
    let path = save_to(&dir, "counter.stalx", &project);
    assert_eq!(
        reload(&path).next_section_id,
        7,
        "a deleted id stays retired"
    );

    let set = |value: Option<&str>| {
        let db = Database::open_existing(&path).unwrap();
        db.conn
            .execute(
                "DELETE FROM project_settings WHERE key = 'next_cave_section_id'",
                [],
            )
            .unwrap();
        if let Some(value) = value {
            db.conn
                .execute(
                    "INSERT INTO project_settings (key, value) VALUES ('next_cave_section_id', ?1)",
                    [value],
                )
                .unwrap();
        }
    };
    set(Some("1"));
    assert_eq!(
        reload(&path).next_section_id,
        2,
        "raised above the saved ids"
    );
    set(Some("not a number"));
    assert_eq!(reload(&path).next_section_id, 2);
    set(None);
    assert_eq!(reload(&path).next_section_id, 2, "derived from the ids");
}

/// The open refuses a file whose rows break a foreign key (as for every
/// table), so a dangling membership only reaches the reader through a
/// connection already open; it is skipped there rather than trusted.
#[test]
fn a_membership_of_a_missing_section_or_line_is_skipped_on_read() {
    let dir = TempDir::new().unwrap();
    let (project, ids) = project_with_sections();
    let path = save_to(&dir, "dangling.stalx", &project);
    let db = Database::open_existing(&path).unwrap();
    db.conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    db.conn
        .execute(
            "INSERT INTO cave_section_lines (line_id, cave_section_id) VALUES (?1, 99)",
            [ids[2]],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO cave_section_lines (line_id, cave_section_id) VALUES (999, 0)",
            [],
        )
        .unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(memberships(&loaded), memberships(&project));
    drop(db);
    assert!(
        Database::open_existing(&path).is_err(),
        "premise: a fresh open refuses the dangling rows"
    );
}

/// A v18 file written before the tables existed: nothing creates them on
/// open (no schema bump), so the load reads "no sections", and a save into
/// that same live connection creates them.
#[test]
fn a_file_without_the_tables_opens_reads_and_saves() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("before-sections.stalx");
    let (project, _) = project_with_sections();
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Before".to_string(), None))
            .unwrap();
        db.conn
            .execute_batch("DROP TABLE cave_section_lines; DROP TABLE cave_sections;")
            .unwrap();
    }

    let readonly = Database::open_readonly(&path).unwrap();
    assert!(!table_exists(&readonly, "cave_sections"));
    assert!(readonly.load_project().unwrap().sections.is_empty());
    drop(readonly);

    let mut live = Database::open_existing(&path).unwrap();
    assert!(live.load_project().unwrap().sections.is_empty());
    live.save_project(&project)
        .expect("a save into a file without the tables must create them");
    drop(live);

    let reloaded = reload(&path);
    assert_eq!(reloaded.sections, project.sections);
    assert_eq!(memberships(&reloaded), memberships(&project));
}

/// Tables of another shape (another tool, a future build) read as no
/// sections, never a failed open, and the next save replaces them.
#[test]
fn tables_of_another_shape_read_as_no_sections() {
    let dir = TempDir::new().unwrap();
    let (project, _) = project_with_sections();
    let path = save_to(&dir, "shape.stalx", &project);
    {
        let db = Database::open_existing(&path).unwrap();
        db.conn
            .execute_batch(
                "DROP TABLE cave_section_lines; DROP TABLE cave_sections;
                 CREATE TABLE cave_sections (id INTEGER PRIMARY KEY, label TEXT);
                 INSERT INTO cave_sections (id, label) VALUES (1, 'x');",
            )
            .unwrap();
    }
    let loaded = reload(&path);
    assert!(loaded.sections.is_empty());
    assert!(loaded.lines.iter().all(|l| l.section_id.is_none()));

    let mut db = Database::open_existing(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    assert_eq!(reload(&path).sections, project.sections);
}

/// Sections are project data: a SpeleoDB push snapshot keeps both tables
/// (`PUSH_SNAPSHOT_LOCAL_TABLES` strips only machine-local ones).
#[test]
fn a_push_snapshot_keeps_the_section_tables() {
    let dir = TempDir::new().unwrap();
    let (project, _) = project_with_sections();
    let path = save_to(&dir, "push.stalx", &project);
    Database::sanitize_push_snapshot(&path, false, None).unwrap();
    let pulled = reload(&path);
    assert_eq!(pulled.sections, project.sections);
    assert_eq!(memberships(&pulled), memberships(&project));
}

/// The SpeleoDB contract (see `names_leave_every_table_an_outside_reader_reads_unchanged`):
/// the same project saved with and without Sections must give identical rows
/// in every table OpenSpeleo's Python `stalagmight_lib` reads, at v18.
#[test]
fn sections_leave_every_table_an_outside_reader_reads_unchanged() {
    assert_eq!(SCHEMA_VERSION, 18);
    let dir = TempDir::new().unwrap();
    let (grouped, _) = project_with_sections();
    let mut plain = grouped.clone();
    plain.sections.clear();
    for line in &mut plain.lines {
        line.section_id = None;
    }
    let with = Database::open_readonly(&save_to(&dir, "with.stalx", &grouped)).unwrap();
    let without = Database::open_readonly(&save_to(&dir, "without.stalx", &plain)).unwrap();
    for table in READ_BY_STALAGMIGHT_LIB {
        let skip: &[&str] = if table == "sections" {
            &["created_at"]
        } else {
            &[]
        };
        assert_eq!(
            dump(&with, table, skip),
            dump(&without, table, skip),
            "{table} must not change when a project gains sections"
        );
    }
    assert_eq!(dump(&with, "cave_sections", &[]).len(), 2);
    assert_eq!(dump(&with, "cave_section_lines", &[]).len(), 2);
    assert!(dump(&without, "cave_sections", &[]).is_empty());
}

/// A list with a repeated name (another tool) is made unique on read, the
/// later Section renamed.
#[test]
fn repeated_names_are_made_unique_on_read() {
    let dir = TempDir::new().unwrap();
    let (mut project, _) = project_with_sections();
    project.sections.push(Section {
        id: 7,
        name: "Upper level".to_string(),
        color: "#000000".to_string(),
    });
    let loaded = reload(&save_to(&dir, "dupe.stalx", &project));
    let names: Vec<&str> = loaded.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Upper level", "Lower level", "Upper level (2)"]);
    assert_eq!(loaded.next_section_id, 8);
}

/// A crafted file: more Section rows than the read takes, all sharing one
/// name, one with an id past `MAX_SECTION_ID` and a colour that is no colour,
/// and a counter past the bound. The open stays prompt (the rows are capped
/// and the dedupe is linear), keeps the first rows of the panel, re-mints the
/// id with its line following it, and settles the counter and the colour.
#[test]
fn a_crafted_flood_of_same_named_sections_opens_promptly_and_settled() {
    use super::cave_sections::MAX_CAVE_SECTION_ROWS;
    use crate::models::{MAX_SECTION_ID, SECTION_PALETTE};

    let dir = TempDir::new().unwrap();
    let (project, ids) = project_with_sections();
    let path = save_to(&dir, "flood.stalx", &project);
    let cap = MAX_CAVE_SECTION_ROWS;
    let high: i64 = 3_000_000_000;
    {
        let db = Database::open_existing(&path).unwrap();
        db.conn
            .execute_batch(&format!(
                "DELETE FROM cave_section_lines;
                 DELETE FROM cave_sections;
                 WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n LIMIT {rows})
                 INSERT INTO cave_sections (id, name, color, position)
                     SELECT i, 'X', '#123456', i FROM n;
                 INSERT INTO cave_sections (id, name, color, position)
                     VALUES ({high}, 'High', 'url(x)', -1);
                 INSERT INTO cave_section_lines (line_id, cave_section_id) VALUES ({line}, {high});
                 INSERT OR REPLACE INTO project_settings (key, value)
                     VALUES ('next_cave_section_id', '4294967295');",
                rows = cap + 5,
                line = ids[0],
            ))
            .unwrap();
    }
    let started = std::time::Instant::now();
    let loaded = reload(&path);
    let elapsed = started.elapsed();

    assert_eq!(
        loaded.sections.len(),
        cap,
        "the rows past the cap are skipped"
    );
    let first = &loaded.sections[0];
    assert_eq!(first.name, "High", "the first rows of the panel are kept");
    let reminted = SectionId::try_from(cap - 1).unwrap();
    assert_eq!(first.id, reminted, "re-minted above the valid ids");
    assert_eq!(first.color, SECTION_PALETTE[0]);
    assert_eq!(
        loaded.get_line(ids[0]).unwrap().section_id,
        Some(reminted),
        "the line follows its Section"
    );
    assert_eq!(loaded.sections[1].name, "X");
    assert_eq!(loaded.sections[2].name, "X (2)");
    let names: std::collections::HashSet<&str> =
        loaded.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names.len(), cap, "every name unique");
    assert!(loaded.sections.iter().all(|s| s.id < MAX_SECTION_ID));
    assert_eq!(loaded.next_section_id, reminted + 1);
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the open took {elapsed:?}"
    );
}
