//! [#608] A `.stalx` can come from anywhere, so its own schema must never get
//! to run SQL: the raw-table viewer neither lists nor reads a virtual table,
//! and every open of an existing file refuses a hostile schema before anything
//! reads it (`schema_guard`). The build-flag check that the bundled SQLite has
//! no FTS/RTREE/DBSTAT modules or extension loading stays in `tests` (CI runs it
//! there by its exact path).
//!
//! All files are synthetic projects this build writes, then reshaped with raw
//! SQL. No fixture is read.

use super::*;
use tempfile::TempDir;

// === The raw-table viewer leaves virtual tables out (#608 p1) ===

#[test]
fn test_raw_tables_leave_out_virtual_tables_even_when_pragmas_are_shadowed() {
    // A crafted file's virtual tables would run their module's code over the
    // file's own data; the viewer must neither list nor read them. The file
    // also ships its own `pragma_table_list`, which a table-valued pragma
    // query would read instead of SQLite's and so list `docs` as ordinary.
    // The schema rows are written directly: release builds compile SQLite
    // without FTS5 or RTREE, so `CREATE VIRTUAL TABLE` would fail there.
    let mut db = Database::in_memory().unwrap();
    db.save_project(&Project::new("Hostile Cave".to_string(), None))
        .unwrap();
    db.conn
        .execute_batch(
            "PRAGMA writable_schema = ON;
             INSERT INTO sqlite_master (type, name, tbl_name, rootpage, sql) VALUES
               ('table', 'docs', 'docs', 0, 'CREATE VIRTUAL TABLE docs USING fts5(body)'),
               ('table', 'boxes', 'boxes', 0, 'CREATE VIRTUAL TABLE/**/boxes USING rtree(id, x0, x1)');
             PRAGMA writable_schema = RESET;
             CREATE TABLE pragma_table_list (name, schema, type);
             INSERT INTO pragma_table_list VALUES ('docs', 'main', 'table'),
                                                  ('boxes', 'main', 'table');",
        )
        .unwrap();

    let names = db.list_raw_table_names().unwrap();

    for hidden in ["docs", "boxes"] {
        assert!(
            !names.iter().any(|n| n == hidden),
            "{hidden} listed: {names:?}"
        );
        let err = db.read_raw_table(hidden, 100, 0).unwrap_err();
        assert!(err.to_string().contains("Unknown table"), "{hidden}: {err}");
    }
    assert!(names.iter().any(|n| n == "shots"));
}

#[test]
fn test_raw_tables_keep_a_legacy_table_with_an_is_virtual_column() {
    // Pre-v4 shots tables had an `is_virtual` column; it is still an ordinary table.
    let db = Database::in_memory().unwrap();
    db.conn
        .execute_batch("CREATE TABLE legacy_shots (id INTEGER, is_virtual INTEGER);")
        .unwrap();

    let names = db.list_raw_table_names().unwrap();

    assert!(names.iter().any(|n| n == "legacy_shots"), "{names:?}");
}

#[test]
fn test_declares_virtual_table_matches_keywords_not_substrings() {
    use super::declares_virtual_table as virt;
    assert!(virt("CREATE VIRTUAL TABLE t USING fts5(x)"));
    assert!(virt("create virtual table t using rtree(a, b, c)"));
    assert!(virt("CREATE/**/VIRTUAL  TABLE t USING fts5(x)"));
    assert!(virt("CREATE -- note\n VIRTUAL TABLE t USING fts5(x)"));
    assert!(virt("CREATE /* unterminated"));
    // A legacy (pre-v4) shots table has an `is_virtual` column.
    assert!(!virt("CREATE TABLE shots (id INTEGER, is_virtual INTEGER)"));
    assert!(!virt("CREATE TABLE \"virtual\" (x)"));
    assert!(!virt(""));
}

// === Hostile schema refused at open (#608 p2) ===

/// A saved project with `sql` run on it through a raw connection afterwards.
fn project_file_with(dir: &TempDir, sql: &str) -> std::path::PathBuf {
    let path = dir.path().join("crafted.stalx");
    project_at(&path, sql);
    path
}

/// [`project_file_with`] at `path`.
fn project_at(path: &Path, sql: &str) {
    Database::open(path)
        .unwrap()
        .save_project(&Project::new("Crafted".to_string(), None))
        .unwrap();
    Connection::open(path).unwrap().execute_batch(sql).unwrap();
}

#[test]
fn every_open_of_an_existing_file_refuses_a_hostile_schema() {
    for (sql, what) in [
        ("CREATE VIEW v AS SELECT 1;", "a view"),
        (
            "CREATE TRIGGER t AFTER UPDATE ON project BEGIN SELECT 1; END;",
            "a trigger",
        ),
        (
            "PRAGMA writable_schema = ON;
             INSERT INTO sqlite_master (type, name, tbl_name, rootpage, sql)
             VALUES ('table', 'docs', 'docs', 0, 'CREATE VIRTUAL TABLE docs USING fts5(body)');",
            "a virtual table",
        ),
        (
            "CREATE TABLE pragma_table_info (cid, name, type);",
            "an object named like a SQLite pragma",
        ),
        (
            "CREATE VIEW w AS SELECT 1;
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = 'View' WHERE name = 'w';",
            "a view",
        ),
        (
            "CREATE TABLE c (x CHECK (length(zeroblob(10)) > 0));",
            "a table with a CHECK constraint the app never writes",
        ),
        (
            "CREATE TABLE g (a, b AS (zeroblob(10)));",
            "a table with a generated column",
        ),
        (
            "CREATE TABLE d (x TEXT DEFAULT (hex(zeroblob(10))));",
            "a table with an expression default",
        ),
        (
            "CREATE INDEX e ON shots (length(hex(zeroblob(10))) + length);",
            "an expression or partial index",
        ),
        (
            "CREATE INDEX p ON shots (length) WHERE length > 0;",
            "an expression or partial index",
        ),
        (
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET name = CAST('project' AS BLOB) WHERE name = 'project';",
            "a schema object of an unexpected type",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let path = project_file_with(&dir, sql);
        let openers: [(&str, fn(&Path) -> Result<Database>); 6] = [
            ("open", Database::open),
            ("open_existing", Database::open_existing),
            ("open_existing_to_read", Database::open_existing_to_read),
            ("open_readonly", Database::open_readonly),
            ("open_for_metadata", Database::open_for_metadata),
            (
                "open_for_metadata_recovering",
                Database::open_for_metadata_recovering,
            ),
        ];
        for (name, opener) in openers {
            let Err(err) = opener(&path) else {
                unreachable!("{name} opened a file with {what}: {sql}");
            };
            let refused = err
                .chain()
                .find_map(|c| c.downcast_ref::<RefusedSchemaObject>())
                .unwrap_or_else(|| unreachable!("{name}: not a schema refusal: {err:#}"));
            assert_eq!(refused.what, what, "{name}: {sql}");
        }
    }
}

#[test]
fn every_opener_still_opens_what_sqlite_itself_adds_to_a_real_file() {
    // AUTOINCREMENT adds sqlite_sequence, UNIQUE an autoindex with NULL sql, and
    // ANALYZE sqlite_stat1: all ordinary, none written by the app's schema code.
    let dir = TempDir::new().unwrap();
    let path = project_file_with(
        &dir,
        "CREATE TABLE extra (id INTEGER PRIMARY KEY AUTOINCREMENT, tag TEXT UNIQUE,
                             n REAL DEFAULT -1.5, t TEXT DEFAULT CURRENT_TIMESTAMP);
         INSERT INTO extra (tag) VALUES ('a');
         CREATE INDEX extra_tag_n ON extra (tag, n);
         ANALYZE;",
    );

    Database::open(&path).unwrap();
    Database::open_existing(&path).unwrap();
    Database::open_existing_to_read(&path).unwrap();
    Database::open_readonly(&path).unwrap();
    Database::open_for_metadata(&path).unwrap();
    Database::open_for_metadata_recovering(&path).unwrap();
    Database::check_schema_at(&path).unwrap();
}

#[test]
fn a_legacy_is_virtual_column_still_opens() {
    let dir = TempDir::new().unwrap();
    let path = project_file_with(&dir, "ALTER TABLE shots ADD COLUMN is_virtual INTEGER;");

    Database::open_existing(&path).unwrap();
}

// === Salvage reads a type cell that lost its case (#576 RV19a-12) ===

/// The schema refusal behind a salvage's error, if that is why it failed.
fn salvage_refused(path: &Path) -> (Result<SalvagedData>, Option<&'static str>) {
    let salvaged = Database::export_salvageable(path);
    let refused = salvaged.as_ref().err().and_then(|err| {
        err.chain()
            .find_map(|c| c.downcast_ref::<RefusedSchemaObject>())
            .map(|refused| refused.what)
    });
    (salvaged, refused)
}

/// [#576 RV19a-12] Salvage is for damaged files, so one flipped bit in a type
/// cell - 'table' -> 'Table', which SQLite still reads and `quick_check`
/// passes - must not refuse it. The schema guard RV18-1 put on the salvage
/// compared the type exactly, so the in-app recovery route was gone for that
/// damage. Every other open keeps the exact spelling.
#[test]
fn salvage_reads_a_file_whose_table_type_lost_its_case() {
    let dir = TempDir::new().unwrap();
    let path = project_file_with(
        &dir,
        "PRAGMA writable_schema = ON;
         UPDATE sqlite_master SET type = 'Table' WHERE name = 'project';",
    );
    let (salvaged, refused) = salvage_refused(&path);
    let opened = refused_what(Database::open_existing(&path));
    println!(
        "type cell 'Table' on the project table: salvage -> {:?} (refused as {refused:?}); \
         open_existing refused as {opened:?}",
        salvaged
            .as_ref()
            .map(|s| (
                s.project_name.clone(),
                s.full_project_json
                    .as_ref()
                    .map(|json| json.contains("\"Crafted\"")),
                s.errors.clone()
            ))
            .map_err(|e| format!("{e:#}"))
    );

    let salvaged = salvaged.unwrap();
    assert_eq!(
        salvaged
            .full_project_json
            .as_ref()
            .map(|json| json.contains("\"Crafted\"")),
        Some(true),
        "the salvage reads the damaged file's project"
    );
    assert_eq!(
        opened,
        Some("a schema object of an unexpected type"),
        "every other open still refuses it"
    );
}

/// [#576 RV19a-12] The salvage's leniency is about spelling only: a table or
/// index typed in another case is checked as one, so it cannot hide what the
/// guard refuses, and a view or trigger is refused in any case.
#[test]
fn salvage_still_refuses_a_hostile_object_whatever_case_its_type_is_in() {
    for (sql, what) in [
        (
            "CREATE TABLE g (a, b AS (zeroblob(10)));
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = 'Table' WHERE name = 'g';",
            "a table with a generated column",
        ),
        (
            "CREATE TABLE d (x TEXT DEFAULT (hex(zeroblob(10))));
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = 'TABLE' WHERE name = 'd';",
            "a table with an expression default",
        ),
        (
            "CREATE INDEX e ON shots (length(hex(zeroblob(10))) + length);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = 'Index' WHERE name = 'e';",
            "an expression or partial index",
        ),
        (
            "CREATE VIEW w AS SELECT 1;
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = 'View' WHERE name = 'w';",
            "a view",
        ),
        (
            "CREATE TRIGGER t AFTER UPDATE ON project BEGIN SELECT 1; END;
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = 'TRIGGER' WHERE name = 't';",
            "a trigger",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let path = project_file_with(&dir, sql);
        let (_, refused) = salvage_refused(&path);
        println!("salvage of {what} behind a case-variant type: refused as {refused:?}");
        assert_eq!(refused, Some(what), "{sql}");
    }
}

// === The second open of an open sequence keeps its own guard (#576 RV18-2) ===

/// Stamps a saved project as an older format, so `open_existing_to_read` reads
/// it through its copy into memory.
const OLDER_FORMAT: &str = "UPDATE project SET schema_version = 16; PRAGMA user_version = 16;";

/// The schema refusal in `result`, if that is why it failed.
fn refused_what(result: Result<Database>) -> Option<&'static str> {
    result.err().and_then(|err| {
        err.chain()
            .find_map(|c| c.downcast_ref::<RefusedSchemaObject>())
            .map(|refused| refused.what)
    })
}

/// The `between_opens` hook the swap tests set: replaces the file being opened
/// with the hostile `swapped.stalx` beside it.
fn swap_in_the_hostile_file(path: &Path) {
    std::fs::rename(path.with_file_name("swapped.stalx"), path).unwrap();
}

/// [#576 RV18-2] `open_existing_to_read` opens an older file twice: once to find
/// it needs an upgrade (refused, so nothing is written), then again to copy it
/// into memory. The first open refuses a hostile schema itself, so the copy's
/// own guard only matters for a file swapped in between - which this does.
#[test]
fn the_copy_into_memory_refuses_a_file_swapped_in_after_the_first_open() {
    let dir = TempDir::new().unwrap();
    let path = project_file_with(&dir, OLDER_FORMAT);
    project_at(
        &dir.path().join("swapped.stalx"),
        &format!("CREATE VIEW v AS SELECT 1; {OLDER_FORMAT}"),
    );

    open::between_opens::set(swap_in_the_hostile_file);
    let refused = refused_what(Database::open_existing_to_read(&path));
    println!("older file swapped for a hostile one before the copy: refused as {refused:?}");

    assert!(
        !dir.path().join("swapped.stalx").exists(),
        "premise: the swap happened"
    );
    assert_eq!(refused, Some("a view"));
}

/// [#576 RV18-2] `into_read_only` drops the read-write handle and reopens the
/// file read-only. The open that made the handle refused a hostile schema, so
/// the reopen's own guard only matters for a file swapped in between - which
/// this does.
#[test]
fn the_read_only_reopen_refuses_a_file_swapped_in_after_the_first_open() {
    let dir = TempDir::new().unwrap();
    let path = project_file_with(&dir, "");
    project_at(
        &dir.path().join("swapped.stalx"),
        "CREATE VIEW v AS SELECT 1;",
    );
    let db = Database::open_existing(&path).unwrap();

    open::between_opens::set(swap_in_the_hostile_file);
    let refused = refused_what(db.into_read_only(&path));
    println!("file swapped for a hostile one before the read-only reopen: refused as {refused:?}");

    assert!(
        !dir.path().join("swapped.stalx").exists(),
        "premise: the swap happened"
    );
    assert_eq!(refused, Some("a view"));
}

/// [#576 RV19a-8] `attempt_repair` renames its repaired copy into place and
/// opens the file again to `quick_check` it. That second open ran no schema
/// guard - the pattern every other reopen now guards - so a file swapped in
/// between the rename and the reopen had its schema's SQL run by the check.
#[test]
fn the_repair_reopen_refuses_a_file_swapped_in_after_the_rename() {
    let dir = TempDir::new().unwrap();
    let path = project_file_with(&dir, "");
    project_at(
        &dir.path().join("swapped.stalx"),
        "CREATE VIEW v AS SELECT 1;",
    );

    open::between_opens::set(swap_in_the_hostile_file);
    let repaired = Database::attempt_repair(&path);
    let refused = repaired.as_ref().err().and_then(|err| {
        err.chain()
            .find_map(|c| c.downcast_ref::<RefusedSchemaObject>())
            .map(|refused| refused.what)
    });
    println!(
        "file swapped for a hostile one before the repair's verifying reopen: {:?}; refused as \
         {refused:?}",
        repaired.as_ref().map_err(|e| format!("{e:#}"))
    );

    assert!(
        !dir.path().join("swapped.stalx").exists(),
        "premise: the swap happened"
    );
    assert_eq!(refused, Some("a view"));
}

// === check_schema_at leaves nothing beside the file (#576 RV18-3) ===

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// [#576 RV18-3] `check_schema_at` runs on a SpeleoDB download before anything
/// else opens it, and must leave its folder as it found it. Opened read-only,
/// its connection created the WAL index of a file in WAL journal mode and could
/// not remove it on close, so `-wal` and `-shm` were left beside the file - and
/// beside a download no read-write open follows (a newer build's file, or one
/// whose history is kept) they outlived it. It must not create a missing file
/// either, and still refuses a hostile one.
#[test]
fn check_schema_at_leaves_nothing_beside_a_wal_mode_file() {
    let dir = TempDir::new().unwrap();
    let path = project_file_with(&dir, "PRAGMA journal_mode = WAL;");
    let before = files_in(dir.path());

    let checked = Database::check_schema_at(&path);
    let after = files_in(dir.path());
    let missing = dir.path().join("missing.stalx");
    let on_missing = Database::check_schema_at(&missing).is_err();
    let hostile = dir.path().join("hostile.stalx");
    project_at(
        &hostile,
        "PRAGMA journal_mode = WAL; CREATE VIEW v AS SELECT 1;",
    );
    let refused = Database::check_schema_at(&hostile)
        .err()
        .and_then(|e| e.downcast_ref::<RefusedSchemaObject>().map(|r| r.what));
    println!(
        "WAL-mode file: check_schema_at -> {:?}; folder before {before:?}, after {after:?}; \
         a missing file: error={on_missing}, created={}; a hostile one refused as {refused:?}",
        checked.as_ref().map_err(|e| format!("{e:#}")),
        missing.exists()
    );

    checked.unwrap();
    assert_eq!(before, ["crafted.stalx"], "fixture: nothing beside it");
    assert_eq!(after, before, "and still nothing beside it");
    assert!(
        on_missing && !missing.exists(),
        "a missing file is not created"
    );
    assert_eq!(refused, Some("a view"));
}

/// The bytes of a HOT rollback journal for a database other than the one it
/// will be put beside: `old(x)`, caught mid-transaction after a cache spill
/// (so the journal header is synced and SQLite would play it back), as a
/// crash in the middle of a write leaves it.
fn a_stale_hot_journal(dir: &Path) -> Vec<u8> {
    let old = dir.join("old.db");
    let conn = Connection::open(&old).unwrap();
    conn.execute_batch(
        "PRAGMA page_size = 1024; PRAGMA journal_mode = DELETE;
         CREATE TABLE old(x); INSERT INTO old VALUES ('OLD-FILE');
         PRAGMA cache_size = 1; BEGIN;
         INSERT INTO old SELECT hex(randomblob(500)) FROM
           (WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50)
            SELECT i FROM n);",
    )
    .unwrap();
    let journal = std::fs::read(repair::sibling(&old, "-journal")).unwrap();
    conn.execute_batch("ROLLBACK").unwrap();
    journal
}

/// [#576 RV19a-3] `check_schema_at` never rolls a journal it finds beside the
/// file into it. Its one caller checks a SpeleoDB download it has just
/// extracted, so a journal there can only be a stale one (left by a crash
/// while an earlier download was being written); a read-write open treated it
/// as hot and played the OLD file's pages back into the fresh bytes, so the
/// check passed and the old file became the cache. The check now opens
/// read-only, where a hot journal makes the read fail instead.
#[test]
fn check_schema_at_never_rolls_a_stale_journal_into_the_file() {
    let dir = TempDir::new().unwrap();
    let journal = a_stale_hot_journal(dir.path());
    let path = dir.path().join("download.stalx");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA page_size = 1024; CREATE TABLE fresh(y);
             INSERT INTO fresh VALUES ('NEW-DOWNLOAD');",
        )
        .unwrap();
    }
    let fresh = std::fs::read(&path).unwrap();
    std::fs::write(repair::sibling(&path, "-journal"), &journal).unwrap();

    let checked = Database::check_schema_at(&path);
    let changed = std::fs::read(&path).unwrap() != fresh;
    let tables: Vec<String> =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|conn| {
                conn.prepare("SELECT name FROM sqlite_master")?
                    .query_map([], |row| row.get(0))?
                    .collect()
            })
            .unwrap_or_default();
    println!(
        "fresh file beside a stale hot journal: check_schema_at -> {:?}; \
         file changed: {changed}; tables read back: {tables:?}",
        checked.as_ref().map_err(|e| format!("{e:#}"))
    );

    assert!(
        !changed,
        "the check must not write the stale journal's pages into the file"
    );
    assert!(
        checked.is_err(),
        "a file with a hot journal beside it is refused, not read as if clean"
    );
}

/// [#576 W52-6] The salvage's summary reads a table whose type cell lost its
/// case, as the salvage itself does. Its existence probe matched the type
/// exactly, so a 'Table' project table was reported as "Project table is
/// empty" with no name - beside the full project read back from that very
/// table - and a 'Table' sections or shots table was counted as 0.
#[test]
fn the_salvage_summary_reads_a_table_whose_type_lost_its_case() {
    let dir = TempDir::new().unwrap();
    let surveyed_at = |name: &str, sql: &str| {
        let path = dir.path().join(name);
        let mut project = Project::new("Crafted".to_string(), None);
        project.add_section(SectionData {
            name: Some("SectionA".to_string()),
            shots: vec![
                crate::models::ShotData {
                    length: 5.0,
                    ..Default::default()
                },
                crate::models::ShotData {
                    length: 7.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project.ensure_shot_uuids();
        Database::open(&path)
            .unwrap()
            .save_project(&project)
            .unwrap();
        Connection::open(&path).unwrap().execute_batch(sql).unwrap();
        path
    };
    let summary = |path: &Path| {
        let salvaged = Database::export_salvageable(path).unwrap();
        (
            salvaged.project_name,
            salvaged.section_count,
            salvaged.shot_count,
            salvaged.errors,
        )
    };
    let clean = summary(&surveyed_at("clean.stalx", ""));
    println!("clean file: salvage summary (name, sections, shots, errors) = {clean:?}");
    assert_eq!(
        clean,
        (Some("Crafted".to_string()), 1, 2, Vec::<String>::new()),
        "premise: the clean file's summary"
    );
    for table in ["project", "sections", "shots"] {
        let path = surveyed_at(
            &format!("{table}.stalx"),
            &format!(
                "PRAGMA writable_schema = ON;
                 UPDATE sqlite_master SET type = 'Table' WHERE name = '{table}';"
            ),
        );
        let damaged = summary(&path);
        println!("type cell 'Table' on {table}: salvage summary = {damaged:?}");
        assert_eq!(
            damaged, clean,
            "{table}: the summary of what the salvage read"
        );
    }
}
