//! [#619] The SQL the authorizer tests run on a connection to a file from
//! outside the app: what [`deny_stripped_sqlite_features`] must refuse whether
//! or not the linked SQLite has it, and ordinary SQL it must still allow. One
//! definition for this crate's `untrusted_sql_tests` and `stalx-reader`'s
//! inspect-connection test, so a probe added here reaches every connection
//! both check.
//!
//! Compiled for this crate's tests and for a dependent that enables
//! `test-support` in its `[dev-dependencies]`; production builds never see it.
//!
//! [`deny_stripped_sqlite_features`]: super::deny_stripped_sqlite_features

use rusqlite::{Connection, ErrorCode};

/// What a probe needs from the linked SQLite to get past name resolution.
#[derive(Clone, Copy)]
pub enum Needs {
    /// Nothing: the authorizer refuses it before SQLite looks the module up.
    Nothing,
    Module(&'static str),
    Function(&'static str),
}

/// DBSTAT and fts3tokenize read by name, a virtual table of each left-out
/// module created, and functions of extension loading, FTS3, FTS5 and R-tree.
pub const PROBES: &[(&str, Needs)] = &[
    ("SELECT * FROM dbstat", Needs::Module("dbstat")),
    ("SELECT count(*) FROM main.DBSTAT", Needs::Module("dbstat")),
    ("SELECT name FROM dbstat('main')", Needs::Module("dbstat")),
    (
        "SELECT token FROM fts3tokenize WHERE input = 'a b'",
        Needs::Module("fts3tokenize"),
    ),
    ("CREATE VIRTUAL TABLE t5 USING fts5(x)", Needs::Nothing),
    ("CREATE VIRTUAL TABLE t4 USING fts4(x)", Needs::Nothing),
    ("CREATE VIRTUAL TABLE t3 USING fts3(x)", Needs::Nothing),
    (
        "CREATE VIRTUAL TABLE tr USING rtree(id, x0, x1)",
        Needs::Nothing,
    ),
    (
        "SELECT load_extension('x')",
        Needs::Function("load_extension"),
    ),
    (
        "SELECT LOAD_EXTENSION('x', 'y')",
        Needs::Function("load_extension"),
    ),
    (
        "SELECT fts3_tokenizer('simple')",
        Needs::Function("fts3_tokenizer"),
    ),
    ("SELECT rtreecheck('t')", Needs::Function("rtreecheck")),
    ("SELECT fts5_source_id()", Needs::Function("fts5_source_id")),
];

/// Ordinary SQL the authorizer must still allow, an eponymous virtual table and
/// a table-valued pragma among it. It reads the `project` table, so it runs on
/// a connection to a saved project.
pub const ALLOWED: &[&str] = &[
    "SELECT count(*) FROM project",
    "SELECT lower(name) FROM project",
    "SELECT count(*) FROM json_each('[1, 2]')",
    "SELECT count(*) FROM pragma_table_info('project')",
];

/// Print the linked SQLite's compile options for the stripped features. With
/// `STALAGMIGHT_EXPECT_UNSTRIPPED_SQLITE` set (CI's build without the repo's
/// flags), fail unless they are all compiled in: on a stripped build the probes
/// would prove nothing about a crates.io build.
///
/// # Panics
///
/// On that premise failing, or when an in-memory SQLite cannot be queried.
pub fn check_the_linked_sqlite() {
    let options: (i64, i64, i64, i64, i64) = Connection::open_in_memory()
        .unwrap()
        .query_row(
            "SELECT sqlite_compileoption_used('ENABLE_FTS5'), \
             sqlite_compileoption_used('ENABLE_FTS3'), \
             sqlite_compileoption_used('ENABLE_RTREE'), \
             sqlite_compileoption_used('ENABLE_DBSTAT_VTAB'), \
             sqlite_compileoption_used('OMIT_LOAD_EXTENSION')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    println!(
        "plain connection: ENABLE_FTS5={} ENABLE_FTS3={} ENABLE_RTREE={} \
         ENABLE_DBSTAT_VTAB={} OMIT_LOAD_EXTENSION={}",
        options.0, options.1, options.2, options.3, options.4
    );
    if std::env::var_os("STALAGMIGHT_EXPECT_UNSTRIPPED_SQLITE").is_some() {
        assert_eq!(
            options,
            (1, 1, 1, 1, 0),
            "STALAGMIGHT_EXPECT_UNSTRIPPED_SQLITE is set, but this SQLite was built \
             with the repo's flags: the test would prove nothing about a crates.io build"
        );
        for needs in [
            Needs::Module("fts5"),
            Needs::Module("dbstat"),
            Needs::Module("fts3tokenize"),
            Needs::Function("load_extension"),
            Needs::Function("rtreecheck"),
        ] {
            assert!(
                plain_has(needs),
                "a plain connection lacks a probed feature"
            );
        }
    }
}

/// Run [`ALLOWED`] and [`PROBES`] on `conn`, printing each probe's outcome
/// after `label`, and return what went wrong: an allowed statement that failed,
/// or a probe that was not refused when prepared.
///
/// # Panics
///
/// When an in-memory SQLite cannot be queried for what it has.
pub fn refusal_failures(conn: &Connection, label: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for sql in ALLOWED {
        if let Err(e) = run(conn, sql) {
            failures.push(format!("{label}: `{sql}` must run, failed: {e}"));
        }
    }
    for (sql, needs) in PROBES {
        let present = plain_has(*needs);
        // Refused means the prepare fails, so nothing of it runs. The
        // authorizer's code is SQLITE_AUTH, except for a function call: SQLite
        // reports that refusal as SQLITE_ERROR (resolve.c sets it over
        // SQLITE_AUTH when it writes the message). SQLITE_ERROR is also what a
        // table or function this SQLite lacks gets, and what a call that cannot
        // prepare anywhere gets (a wrong argument count, which SQLite reports
        // before it asks the authorizer). So where this SQLite has the
        // function, the same SQL must prepare on a plain connection: only then
        // is the failure here the authorizer's.
        let outcome = match conn.prepare(sql) {
            Ok(_) => Ok(run(conn, sql)),
            Err(e) => Err(e),
        };
        let refused = outcome
            .as_ref()
            .err()
            .and_then(code_of)
            .is_some_and(|code| {
                code == ErrorCode::AuthorizationForStatementDenied
                    || (code == ErrorCode::Unknown
                        && (!present
                            || (matches!(needs, Needs::Function(_)) && plain_prepares(sql))))
            });
        let what = if refused { "refused" } else { "NOT REFUSED" };
        let has = match needs {
            Needs::Nothing => "",
            _ if present => " (linked SQLite has it)",
            _ => " (linked SQLite lacks it)",
        };
        println!("{label}: `{sql}` -> {what}: {outcome:?}{has}");
        if !refused {
            failures.push(format!("{label}: `{sql}` -> {outcome:?}"));
        }
    }
    failures
}

/// Prepare `sql` and step it to the end.
fn run(conn: &Connection, sql: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// The SQLite result code of `e`, also for the `SqlInputError` a failed
/// prepare can be, which `Error::sqlite_error_code` does not read.
fn code_of(e: &rusqlite::Error) -> Option<ErrorCode> {
    match e {
        rusqlite::Error::SqliteFailure(error, _) | rusqlite::Error::SqlInputError { error, .. } => {
            Some(error.code)
        }
        _ => None,
    }
}

/// Whether `sql` prepares on a plain connection, with no authorizer.
fn plain_prepares(sql: &str) -> bool {
    Connection::open_in_memory().unwrap().prepare(sql).is_ok()
}

/// Whether a plain connection, with no authorizer, has `needs`.
fn plain_has(needs: Needs) -> bool {
    let (sql, name) = match needs {
        Needs::Nothing => return true,
        Needs::Module(name) => (
            "SELECT count(*) FROM pragma_module_list WHERE name = ?1",
            name,
        ),
        Needs::Function(name) => (
            "SELECT count(*) FROM pragma_function_list WHERE name = ?1",
            name,
        ),
    };
    Connection::open_in_memory()
        .unwrap()
        .query_row(sql, [name], |r| r.get::<_, i64>(0))
        .unwrap()
        > 0
}
