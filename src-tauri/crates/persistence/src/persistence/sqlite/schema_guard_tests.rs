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

/// [#619] A table with a CHECK the guard refuses when its `sql` cell is text,
/// with that cell rewritten as a BLOB of the same bytes.
const CHECK_STORED_AS_BLOB: &str = "CREATE TABLE c (x CHECK (x > 0));
     PRAGMA writable_schema = ON;
     UPDATE sqlite_master SET sql = CAST(sql AS BLOB) WHERE name = 'c';";

/// [#619] How the guard refuses a schema row whose `tbl_name` is not text or
/// whose `sql` is neither text nor NULL.
const NOT_STORED_AS_TEXT: &str = "a schema object whose table name or SQL is not stored as text";

/// [#619] How the guard refuses a schema row whose `name`, `tbl_name` or `sql`
/// bytes are not UTF-8.
const NOT_UTF8: &str = "a schema object whose name, table name or SQL is not valid UTF-8";

/// [#619] An autoindex row (no SQL) whose `tbl_name` holds a byte that is not
/// UTF-8.
const AUTOINDEX_TABLE_NAME_NOT_UTF8: &str = "CREATE TABLE extra (tag TEXT UNIQUE);
     PRAGMA writable_schema = ON;
     UPDATE sqlite_master SET tbl_name = 'extra' || CAST(x'ff' AS TEXT)
     WHERE name = 'sqlite_autoindex_extra_1';";

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
        // [#619] Names that are not UTF-8 would reach the authorizer, whose
        // rusqlite wrapper panics on them. An index so named opened before.
        (
            "CREATE INDEX zzi ON shots (length);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET name = 'zz' || CAST(x'ff' AS TEXT),
               sql = 'CREATE INDEX \"zz' || CAST(x'ff' AS TEXT) || '\" ON shots (length)'
             WHERE name = 'zzi';",
            NOT_UTF8,
        ),
        (
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET sql = CAST(replace(sql, 'name TEXT NOT NULL',
               'name TEXT NOT NULL, \"c' || CAST(x'ff' AS TEXT) || '\" TEXT') AS BLOB)
             WHERE name = 'project';",
            NOT_UTF8,
        ),
        // [#619] A `tbl_name` SQLite never reads: this opened before.
        (AUTOINDEX_TABLE_NAME_NOT_UTF8, NOT_UTF8),
        // [#619] SQLite parses a `sql` cell stored as a BLOB like text, so a
        // CHECK so stored is enforced, but the guard saw no SQL in it. Each of
        // these opened before.
        (CHECK_STORED_AS_BLOB, NOT_STORED_AS_TEXT),
        (
            "CREATE INDEX zzb ON shots (length);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET sql = CAST(sql AS BLOB) WHERE name = 'zzb';",
            NOT_STORED_AS_TEXT,
        ),
        (
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET tbl_name = CAST(tbl_name AS BLOB) WHERE name = 'shots';",
            NOT_STORED_AS_TEXT,
        ),
        // A type cell stored as a BLOB was refused already.
        (
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = CAST(type AS BLOB) WHERE name = 'shots';",
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

/// [#619] SQLite builds its schema from a `sql` cell's bytes whatever the
/// cell's storage class, so a CHECK stored as a BLOB is parsed and enforced
/// exactly like one stored as text. The guard read only a text cell as SQL,
/// so it saw none in the BLOB, and the file passed. Stored as text, the same
/// table is refused.
#[test]
fn a_check_whose_sql_is_stored_as_a_blob_is_still_enforced_and_now_refused() {
    let dir = TempDir::new().unwrap();
    let path = project_file_with(&dir, CHECK_STORED_AS_BLOB);
    let conn = Connection::open(&path).unwrap();
    let stored: String = conn
        .query_row(
            "SELECT typeof(sql) FROM sqlite_master WHERE name = 'c'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let violating = conn
        .execute("INSERT INTO c VALUES (0)", [])
        .map_err(|e| e.to_string());
    let passing = conn
        .execute("INSERT INTO c VALUES (1)", [])
        .map_err(|e| e.to_string());
    let refused = refused_schema_object(&conn).unwrap().map(|r| r.what);
    println!(
        "sql cell stored as {stored}; INSERT 0 -> {violating:?}; INSERT 1 -> {passing:?}; \
         refused_schema_object -> {refused:?}"
    );
    assert_eq!(stored, "blob");
    assert_eq!(
        violating,
        Err("CHECK constraint failed: x > 0".to_string()),
        "SQLite enforces the CHECK it parsed from the BLOB"
    );
    assert_eq!(passing, Ok(1));
    assert_eq!(refused, Some(NOT_STORED_AS_TEXT));

    let dir = TempDir::new().unwrap();
    let as_text = project_file_with(&dir, "CREATE TABLE c (x CHECK (x > 0));");
    let refused = refused_schema_object(&Connection::open(&as_text).unwrap())
        .unwrap()
        .map(|r| r.what);
    assert_eq!(
        refused,
        Some("a table with a CHECK constraint the app never writes"),
        "the same table stored as text"
    );
}

/// [#619] SQLite reports a malformed schema for a row whose `tbl_name` is not
/// the table its SQL names, so a `tbl_name` that is not UTF-8 never got that
/// far on a table or an index. On an autoindex row, which has no SQL, SQLite
/// never reads the cell: the file loads, passes `quick_check`, and every open
/// opened it, past the UTF-8 check that held only `name` and `sql`.
#[test]
fn a_table_name_that_is_not_utf8_is_refused_where_sqlite_never_reads_it() {
    let quick_check = |path: &Path| {
        Connection::open(path)
            .unwrap()
            .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())
    };
    let dir = TempDir::new().unwrap();
    let on_a_table = quick_check(&project_file_with(
        &dir,
        "PRAGMA writable_schema = ON;
         UPDATE sqlite_master SET tbl_name = 'project' || CAST(x'ff' AS TEXT)
         WHERE name = 'project';",
    ));
    let dir = TempDir::new().unwrap();
    let path = project_file_with(&dir, AUTOINDEX_TABLE_NAME_NOT_UTF8);
    let on_an_autoindex = quick_check(&path);
    let refused = refused_schema_object(&Connection::open(&path).unwrap())
        .unwrap()
        .map(|r| r.what);
    let opened = refused_what(Database::open_existing(&path));
    println!(
        "tbl_name not UTF-8: on the project table, quick_check -> {on_a_table:?}; on an \
         autoindex, quick_check -> {on_an_autoindex:?}, refused_schema_object -> {refused:?}, \
         open_existing refused as {opened:?}"
    );

    assert_eq!(
        on_an_autoindex,
        Ok("ok".to_string()),
        "premise: SQLite loads the schema"
    );
    assert_eq!(refused, Some(NOT_UTF8));
    assert_eq!(opened, Some(NOT_UTF8));
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

/// [#619] One flipped bit in a record header's serial type turns a TEXT cell
/// into a BLOB of the same bytes (2n+13 -> 2n+12), which SQLite still parses
/// and `quick_check` passes, like the type cell's 'table' -> 'Table'
/// (RV19a-12). The guard holds `sql` and `tbl_name` to text, so the salvage
/// refused that damage too; it now reads such a BLOB of valid UTF-8 as the
/// text it was. Every other open still refuses it.
#[test]
fn salvage_reads_a_file_whose_sql_or_table_name_lost_its_text_type() {
    for cell in ["sql", "tbl_name"] {
        let dir = TempDir::new().unwrap();
        let path = project_file_with(
            &dir,
            &format!(
                "PRAGMA writable_schema = ON;
                 UPDATE sqlite_master SET {cell} = CAST({cell} AS BLOB) WHERE name = 'project';"
            ),
        );
        let conn = Connection::open(&path).unwrap();
        let stored: String = conn
            .query_row(
                &format!("SELECT typeof({cell}) FROM sqlite_master WHERE name = 'project'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        let quick_check: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .unwrap();
        drop(conn);
        let (salvaged, refused) = salvage_refused(&path);
        let read_back = salvaged.as_ref().ok().map(|s| {
            (
                s.full_project_json
                    .as_ref()
                    .map(|json| json.contains("\"Crafted\"")),
                s.errors.clone(),
            )
        });
        let opened = refused_what(Database::open_existing(&path));
        println!(
            "{cell} cell of the project table stored as {stored}: quick_check -> {quick_check:?}; \
             salvage -> {read_back:?} (refused as {refused:?}); open_existing refused as {opened:?}"
        );

        assert_eq!(
            (stored.as_str(), quick_check.as_str()),
            ("blob", "ok"),
            "premise: a BLOB SQLite reads"
        );
        assert_eq!(
            read_back,
            Some((Some(true), Vec::new())),
            "{cell}: the salvage reads the damaged file's project"
        );
        assert_eq!(
            opened,
            Some(NOT_STORED_AS_TEXT),
            "{cell}: every other open still refuses it"
        );
    }
}

/// [#619] What the salvage reads from such a BLOB goes through every check text
/// gets, so a CHECK stored in one is refused as that CHECK, and a BLOB whose
/// bytes are not UTF-8 is still refused.
#[test]
fn salvage_still_refuses_what_a_blob_sql_or_table_name_holds() {
    for (sql, what) in [
        (
            CHECK_STORED_AS_BLOB,
            "a table with a CHECK constraint the app never writes",
        ),
        (
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET sql = CAST(replace(sql, 'name TEXT NOT NULL',
               'name TEXT NOT NULL, \"c' || CAST(x'ff' AS TEXT) || '\" TEXT') AS BLOB)
             WHERE name = 'project';",
            NOT_UTF8,
        ),
        // On an autoindex row: SQLite never reads its `tbl_name`.
        (
            "CREATE TABLE extra (tag TEXT UNIQUE);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET tbl_name = CAST('extra' || CAST(x'ff' AS TEXT) AS BLOB)
             WHERE name = 'sqlite_autoindex_extra_1';",
            NOT_UTF8,
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let path = project_file_with(&dir, sql);
        let (_, refused) = salvage_refused(&path);
        println!("salvage of a file with a BLOB cell, expecting {what:?}: refused as {refused:?}");
        assert_eq!(refused, Some(what), "{sql}");
    }
}

/// [#619] How the guard refuses a schema row whose `type` or `name` is not text.
const UNEXPECTED_TYPE: &str = "a schema object of an unexpected type";

/// [#619] A saved project with one bit flipped in the file itself: the serial
/// type of the `type` or `name` cell in the project table's schema record,
/// TEXT of n bytes (2n+13) -> a BLOB of the same bytes (2n+12).
fn project_with_one_schema_bit_flipped(dir: &TempDir, cell: &str) -> std::path::PathBuf {
    let path = project_file_with(dir, "VACUUM;");
    flip_one_schema_bit(&path, "project", cell);
    path
}

/// [#619] One bit flipped in `table`'s schema record in the file at `path`,
/// which VACUUM has just compacted: with `cell` "type" or "name", the serial
/// type of that cell, TEXT of n bytes (2n+13) -> a BLOB of the same bytes
/// (2n+12); with "type case", bit 5 of the type's first byte, 'table' ->
/// 'Table' (0x74 -> 0x54).
fn flip_one_schema_bit(path: &Path, table: &str, cell: &str) {
    // VACUUM leaves no free page, but a page split can leave a stale copy of a
    // record in a page's free space: the live one is where a cell pointer of a
    // table leaf page (type 13) leads, past the cell's payload-size and rowid
    // varints.
    let mut bytes = std::fs::read(path).unwrap();
    let be16 = |at: usize| usize::from(u16::from_be_bytes([bytes[at], bytes[at + 1]]));
    let varint_len = |at: usize| (0..9).find(|k| bytes[at + k] < 0x80).map_or(9, |k| k + 1);
    let page_size = be16(16);
    let in_a_cell = |record: usize| {
        let page = record / page_size * page_size;
        let head = page + if page == 0 { 100 } else { 0 };
        bytes[head] == 13
            && (0..be16(head + 3)).any(|c| {
                let cell = page + be16(head + 8 + 2 * c);
                let rowid = cell + varint_len(cell);
                rowid + varint_len(rowid) == record
            })
    };
    // The record's header is 7 bytes (checked below), and its body opens with
    // its type, name and tbl_name cells.
    let body = format!("table{table}{table}");
    let body = body.as_bytes();
    let found: Vec<usize> = bytes
        .windows(body.len())
        .enumerate()
        .filter(|&(i, w)| w == body && in_a_cell(i - 7))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "premise: one schema record of the {table} table"
    );
    // Its header: its own size, 7, then the serial types of 'table' (23), the
    // name twice (27 for 'project'), the rootpage (an 8-bit int, 1) and the
    // 2-byte sql.
    let header = found[0] - 7;
    let name = u8::try_from(2 * table.len() + 13).unwrap();
    assert_eq!(
        bytes[header..header + 5],
        [7, 23, name, name, 1],
        "premise: the {table} table's schema record header"
    );
    let (at, bit) = match cell {
        "type" => (header + 1, 1),
        "name" => (header + 2, 1),
        "type case" => (found[0], 0x20),
        other => unreachable!("no such damage: {other}"),
    };
    bytes[at] ^= bit;
    std::fs::write(path, &bytes).unwrap();
}

/// [#619] The salvage's tolerance of a `sql` or `tbl_name` BLOB, for the same
/// one-bit damage in the `type` or `name` cell, here flipped in the file:
/// SQLite still builds the schema (it compares those cells with the SQL as
/// text, in any case) and `quick_check` passes, but the salvage refused the
/// file as a schema object of an unexpected type. It now reads such a BLOB of
/// valid UTF-8 as the text it was. Every other open still refuses it.
#[test]
fn salvage_reads_a_file_whose_type_or_name_lost_its_text_type() {
    for (cell, was) in [("type", "table"), ("name", "project")] {
        let dir = TempDir::new().unwrap();
        let path = project_with_one_schema_bit_flipped(&dir, cell);
        let conn = Connection::open(&path).unwrap();
        let (stored, bytes): (String, Vec<u8>) = conn
            .query_row(
                &format!(
                    "SELECT typeof({cell}), CAST({cell} AS BLOB) FROM sqlite_master
                     WHERE CAST(name AS TEXT) = 'project'"
                ),
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let quick_check: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .unwrap();
        let strict = refused_schema_object(&conn).unwrap().map(|r| r.what);
        drop(conn);
        let (salvaged, refused) = salvage_refused(&path);
        let read_back = salvaged.as_ref().ok().map(|s| {
            (
                s.project_name.clone(),
                s.full_project_json
                    .as_ref()
                    .map(|json| json.contains("\"Crafted\"")),
                s.errors.clone(),
            )
        });
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
        let opened: Vec<_> = openers
            .iter()
            .map(|(name, open)| (*name, refused_what(open(&path))))
            .collect();
        println!(
            "{cell} cell of the project table flipped to {stored} {:?}: quick_check -> \
             {quick_check:?}; salvage -> {read_back:?} (refused as {refused:?}); \
             refused_schema_object -> {strict:?}; openers refused as {opened:?}",
            String::from_utf8_lossy(&bytes)
        );

        assert_eq!(
            (stored.as_str(), bytes.as_slice(), quick_check.as_str()),
            ("blob", was.as_bytes(), "ok"),
            "premise: a BLOB of the same bytes, which SQLite reads"
        );
        assert_eq!(
            read_back,
            Some((Some("Crafted".to_string()), Some(true), Vec::new())),
            "{cell}: the salvage reads the damaged file's project"
        );
        assert_eq!(
            strict,
            Some(UNEXPECTED_TYPE),
            "{cell}: refused_schema_object"
        );
        for (name, refused) in opened {
            assert_eq!(
                refused,
                Some(UNEXPECTED_TYPE),
                "{cell}: {name} still refuses it"
            );
        }
    }
}

/// [#619] `load_project` first looks up each optional table (one a file written
/// before it existed lacks) with an exact `type='table' AND name='..'` match.
/// The salvage's whole-project load used it too, so a table whose schema row
/// one flipped bit left typed 'Table', or with its `type` or `name` cell a BLOB
/// of the same bytes, was taken for absent and its rows were silently left out
/// of the salvaged project, although the salvage's guard accepts such a row and
/// SQLite still reads the table. The salvage now finds those tables as its
/// guard reads them. Every other open still refuses such a file.
#[test]
fn salvage_keeps_the_rows_of_an_optional_table_whose_schema_row_lost_its_text_type_or_case() {
    use crate::models::{ShotData, StationRef};
    // Each optional table `load_project` reads, and the project field its
    // rows land in.
    let tables = [
        ("station_offsets", "station_offsets"),
        ("station_comments", "station_comments"),
        ("loop_closures", "loop_closures"),
        ("imported_file_hashes", "imported_file_hashes"),
        ("imported_section_hashes", "imported_section_hashes"),
        ("project_settings", "surface_elevation"),
    ];
    let mut project = Project::new("Crafted".to_string(), None);
    let section = project
        .add_section(SectionData {
            name: Some("SectionA".to_string()),
            shots: vec![
                ShotData {
                    length: 5.0,
                    ..Default::default()
                };
                3
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project
        .station_offsets
        .insert(StationRef::new(section, 1).into(), (0.5, -0.25, 0.125));
    project.station_comments.insert(
        StationRef::new(section, 2).into(),
        "Squeeze past the boulder".to_string(),
    );
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(section, 0).into(),
        station_b: StationRef::new(section, 3).into(),
        strategy: ClosureStrategy::Move,
        is_aligned: false,
        path_length: 15.0,
        misclosure_distance: 0.5,
        horizontal_misclosure: 0.25,
        vertical_misclosure: 0.5,
        error_percentage: 2.0,
    });
    project.imported_file_hashes.push("file-hash-1".to_string());
    project
        .imported_section_hashes
        .push("section-hash-1".to_string());
    project.surface_elevation = Some(-12.5);
    let dir = TempDir::new().unwrap();
    // The same project in every file, VACUUMed for `flip_one_schema_bit`.
    let saved_at = |name: &str| {
        let path = dir.path().join(name);
        Database::open(&path)
            .unwrap()
            .save_project(&project)
            .unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch("VACUUM;")
            .unwrap();
        path
    };
    let salvaged_project = |path: &Path| {
        let salvaged = Database::export_salvageable(path).unwrap();
        let loaded = salvaged
            .full_project_json
            .as_deref()
            .map(|json| serde_json::from_str::<serde_json::Value>(json).unwrap());
        (loaded, salvaged.errors)
    };
    let (clean, clean_errors) = salvaged_project(&saved_at("clean.stalx"));
    let clean = clean.unwrap_or_default();
    let fields: Vec<_> = tables
        .iter()
        .map(|(_, field)| (*field, clean[*field].clone()))
        .collect();
    println!("clean file: salvage errors {clean_errors:?}; the fields it read: {fields:?}");
    let has_rows = |value: &serde_json::Value| match value {
        serde_json::Value::Array(rows) => !rows.is_empty(),
        value => !value.is_null(),
    };
    assert!(
        clean_errors.is_empty() && fields.iter().all(|(_, value)| has_rows(value)),
        "premise: the clean file's salvage reads a row of every optional table"
    );

    let mut dropped = Vec::new();
    for (table, field) in tables {
        for (cell, damage) in [
            ("type case", "type 'Table'"),
            ("type", "type a BLOB"),
            ("name", "name a BLOB"),
        ] {
            let path = saved_at(&format!("{table}-{}.stalx", cell.replace(' ', "-")));
            flip_one_schema_bit(&path, table, cell);
            let conn = Connection::open(&path).unwrap();
            let stored: (String, Vec<u8>, String, Vec<u8>) = conn
                .query_row(
                    "SELECT typeof(type), CAST(type AS BLOB), typeof(name), CAST(name AS BLOB)
                     FROM sqlite_master WHERE tbl_name = ?1 AND sql LIKE 'CREATE TABLE%'",
                    [table],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            let rows: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            let quick_check: String = conn
                .query_row("PRAGMA quick_check", [], |r| r.get(0))
                .unwrap();
            drop(conn);
            let (damaged, errors) = salvaged_project(&path);
            let read = damaged.as_ref().map(|loaded| loaded[field].clone());
            let opened: Vec<_> = [
                ("open", Database::open as fn(&Path) -> Result<Database>),
                ("open_existing", Database::open_existing),
                ("open_existing_to_read", Database::open_existing_to_read),
                ("open_readonly", Database::open_readonly),
                ("open_for_metadata", Database::open_for_metadata),
                (
                    "open_for_metadata_recovering",
                    Database::open_for_metadata_recovering,
                ),
            ]
            .iter()
            .map(|(name, open)| (*name, refused_what(open(&path))))
            .collect();
            println!(
                "{table}, {damage}: stored as ({}, {:?}, {}, {:?}), {rows} row(s) SQLite reads, \
                 quick_check -> {quick_check:?}; salvage errors {errors:?}, {field} read back as \
                 {read:?}; openers refused as {opened:?}",
                stored.0,
                String::from_utf8_lossy(&stored.1),
                stored.2,
                String::from_utf8_lossy(&stored.3)
            );

            let expected = match cell {
                "type case" => ("text", "Table", "text"),
                "type" => ("blob", "table", "text"),
                _ => ("text", "table", "blob"),
            };
            assert_eq!(
                (
                    (stored.0.as_str(), stored.1.as_slice(), stored.2.as_str()),
                    stored.3.as_slice(),
                    quick_check.as_str()
                ),
                (
                    (expected.0, expected.1.as_bytes(), expected.2),
                    table.as_bytes(),
                    "ok"
                ),
                "premise: {table}, {damage}, which SQLite reads"
            );
            assert!(rows > 0, "premise: {table} holds a row");
            for (name, refused) in opened {
                assert_eq!(
                    refused,
                    Some(UNEXPECTED_TYPE),
                    "{table}, {damage}: {name} still refuses it"
                );
            }
            let every_table: Option<Vec<_>> = damaged.as_ref().map(|loaded| {
                tables
                    .iter()
                    .map(|(_, field)| (*field, loaded[*field].clone()))
                    .collect()
            });
            if !errors.is_empty() || every_table.as_ref() != Some(&fields) {
                dropped.push(format!("{table}, {damage}: {field} read back as {read:?}"));
            }
        }
    }
    assert!(
        dropped.is_empty(),
        "the salvage reads every optional table's rows, the damaged one's included: {dropped:#?}"
    );
}

/// [#619] What the salvage reads from a `type` or `name` BLOB goes through
/// every check text gets, the type in any case: a view or a pragma-named
/// table so stored is refused as what it is, and a table or an index so
/// stored cannot hide a generated column or an expression index. A BLOB that
/// is not UTF-8 is still refused.
#[test]
fn salvage_still_refuses_what_a_blob_type_or_name_holds() {
    for (sql, what) in [
        (
            "CREATE VIEW w AS SELECT 1;
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = CAST('View' AS BLOB) WHERE name = 'w';",
            "a view",
        ),
        (
            "CREATE TABLE g (a, b AS (zeroblob(10)));
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = CAST('Table' AS BLOB) WHERE name = 'g';",
            "a table with a generated column",
        ),
        (
            "CREATE INDEX e ON shots (length(hex(zeroblob(10))) + length);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = CAST(type AS BLOB) WHERE name = 'e';",
            "an expression or partial index",
        ),
        (
            "CREATE TABLE pragma_table_info (cid, name, type);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET name = CAST(name AS BLOB)
             WHERE name = 'pragma_table_info';",
            "an object named like a SQLite pragma",
        ),
        (
            "CREATE TABLE g (a, b AS (zeroblob(10)));
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET name = CAST(name AS BLOB) WHERE name = 'g';",
            "a table with a generated column",
        ),
        (
            "CREATE INDEX e ON shots (length(hex(zeroblob(10))) + length);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET name = CAST(name AS BLOB) WHERE name = 'e';",
            "an expression or partial index",
        ),
        // Not UTF-8, on an autoindex row: SQLite never reads its type.
        (
            "CREATE TABLE extra (tag TEXT UNIQUE);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET type = CAST('index' || CAST(x'ff' AS TEXT) AS BLOB)
             WHERE name = 'sqlite_autoindex_extra_1';",
            UNEXPECTED_TYPE,
        ),
        (
            "CREATE INDEX zzi ON shots (length);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET name = CAST('zz' || CAST(x'ff' AS TEXT) AS BLOB),
               sql = 'CREATE INDEX \"zz' || CAST(x'ff' AS TEXT) || '\" ON shots (length)'
             WHERE name = 'zzi';",
            NOT_UTF8,
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let path = project_file_with(&dir, sql);
        let (salvaged, refused) = salvage_refused(&path);
        println!(
            "salvage of a file with a BLOB type or name, expecting {what:?}: refused as \
             {refused:?} ({:?})",
            salvaged.as_ref().err().map(|e| format!("{e:#}"))
        );
        assert_eq!(refused, Some(what), "{sql}");
    }
}

/// [#619] SQLite reads a BLOB schema cell as text in the file's encoding, the
/// salvage as UTF-8, so in a UTF-16 file the two read different text. The
/// BLOB of a CHECK's UTF-16 text is valid UTF-8 in which the guard finds no
/// CHECK, while SQLite parses and enforces it; the BLOB of a pragma-named
/// table's UTF-16 name does not start with `pragma_` to the guard, while
/// SQLite reads that name. The salvage reads a BLOB as text only in a UTF-8
/// file, the only kind the app writes.
#[test]
fn salvage_reads_a_blob_schema_cell_as_text_only_in_a_utf8_file() {
    for (cell, sql, what) in [
        (
            "sql",
            "CREATE TABLE c (x CHECK (x > 0));
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET sql = CAST(sql AS BLOB) WHERE name = 'c';",
            NOT_STORED_AS_TEXT,
        ),
        (
            "name",
            "CREATE TABLE pragma_table_info (cid, name, type);
             PRAGMA writable_schema = ON;
             UPDATE sqlite_master SET name = CAST(name AS BLOB)
             WHERE name = 'pragma_table_info';",
            UNEXPECTED_TYPE,
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("utf16.stalx");
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!("PRAGMA encoding = 'UTF-16le'; {sql}"))
            .unwrap();
        let conn = Connection::open(&path).unwrap();
        let encoding: String = conn.query_row("PRAGMA encoding", [], |r| r.get(0)).unwrap();
        let (stored, as_sqlite_reads_it, bytes): (String, String, Vec<u8>) = conn
            .query_row(
                &format!(
                    "SELECT typeof({cell}), CAST({cell} AS TEXT), {cell} FROM sqlite_master
                     WHERE tbl_name IN ('c', 'pragma_table_info')"
                ),
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        let as_utf8 = std::str::from_utf8(&bytes).map(str::to_owned);
        let enforced = conn
            .execute("INSERT INTO c VALUES (0)", [])
            .map_err(|e| e.to_string());
        drop(conn);
        let refused = refused_what(Database::open_for_recovery(&path));
        println!(
            "{encoding} file, {cell} cell stored as {stored}: SQLite reads {as_sqlite_reads_it:?}, \
             as UTF-8 {as_utf8:?}; INSERT INTO c VALUES (0) -> {enforced:?}; \
             open_for_recovery refused as {refused:?}"
        );

        assert_eq!(
            (encoding.as_str(), stored.as_str()),
            ("UTF-16le", "blob"),
            "premise"
        );
        let as_utf8 =
            as_utf8.unwrap_or_else(|e| unreachable!("premise: the BLOB is valid UTF-8: {e}"));
        if cell == "sql" {
            assert_eq!(
                enforced,
                Err("CHECK constraint failed: x > 0".to_string()),
                "premise: SQLite parses and enforces the CHECK"
            );
            assert!(!as_utf8.contains("CHECK"), "premise: {as_utf8:?}");
        } else {
            assert_eq!(as_sqlite_reads_it, "pragma_table_info", "premise");
            assert!(!as_utf8.starts_with("pragma_"), "premise: {as_utf8:?}");
        }
        assert_eq!(refused, Some(what), "{cell}: the salvage refuses it");
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
