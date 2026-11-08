//! Refusing `.stalx` files whose schema could run SQL the file chose (#608).
//!
//! A `.stalx` is a SQLite database anyone can send, so its schema is input.
//! The app never creates views, triggers or virtual tables, so a file that has
//! one was edited by something else, and opening it would let the file run its
//! own SQL or a virtual-table module's code. [`check_schema`] refuses such a
//! file before anything else reads it; the `stalx-reader` crate runs the same
//! check on server uploads.

use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::Connection;

/// A schema object the app never creates, found in a file it was asked to open.
/// Rides inside the open error, so callers can `downcast_ref` it to tell this
/// refusal apart from corruption or an I/O failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedSchemaObject {
    /// What it is, in words for a person ("a view", "a virtual table").
    pub what: &'static str,
    /// Its name as stored in the file: untrusted text, see [`Self::display_name`].
    pub name: String,
}

impl RefusedSchemaObject {
    /// The object's name made safe to show: ASCII letters, digits, space and
    /// `_ . -` kept, anything else (quotes, line or paragraph separators,
    /// zero-width and bidi characters, look-alikes) shown as `?`, at most 60
    /// characters. The name is the file's own text and must not be able to
    /// close the message's quotes or add a line to it.
    pub fn display_name(&self) -> String {
        let mut shown: String = self
            .name
            .chars()
            .take(60)
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-') {
                    c
                } else {
                    '?'
                }
            })
            .collect();
        if self.name.chars().count() > 60 {
            shown.push_str("...");
        }
        shown
    }
}

impl std::fmt::Display for RefusedSchemaObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the file defines {} named '{}'",
            self.what,
            self.display_name()
        )
    }
}

impl std::error::Error for RefusedSchemaObject {}

/// Harden `conn` for a file from outside the app, then refuse it if its schema
/// holds anything the app never creates.
///
/// Hardening first: defensive mode, and `trusted_schema` off so schema code may
/// call only innocuous functions. Then one pass over `sqlite_master`, which a
/// file cannot shadow (`sqlite_` names are reserved) and whose reading runs no
/// module code. It refuses, in order:
/// - any row whose `name`, `tbl_name` or `sql` is not valid UTF-8 (#619): SQLite
///   would hand such an identifier to the authorizer, whose rusqlite wrapper
///   panics on it;
/// - any row whose `type`, `name` or `tbl_name` is not stored as text, or whose
///   `sql` is neither text nor NULL (#619): SQLite parses a `sql` cell stored as
///   a BLOB like text, so a CHECK in one is enforced, but the checks below read
///   only text and would see no SQL there;
/// - any object named `pragma_*` (any case): the file's own table or view of
///   that name would be read in place of SQLite's table-valued pragma by every
///   later probe, and could lie to all of them;
/// - any type other than exactly `table` or `index`: SQLite reads `type` case-
///   insensitively when it loads the schema, so a row typed 'View' or 'TABLE'
///   is still a view or a table to it while slipping past a `type = 'view'`
///   filter. Views and triggers land here too;
/// - virtual tables: rootpage 0, or SQL opening with the `CREATE VIRTUAL`
///   keywords (see [`declares_virtual_table`]).
pub fn check_schema(conn: &Connection) -> anyhow::Result<()> {
    guard(conn, TypeSpelling::Exact)
}

/// [`check_schema`] for the salvage of a damaged file (#576 RV19a-12): the
/// same refusals, except that a `table` or `index` type spelled in another
/// case - one flipped bit, 'table' -> 'Table', which SQLite still reads (it
/// builds the schema from the `sql` text) - is checked as that type instead of
/// refused. Every expression check still runs on it, so a case-variant row
/// cannot hide a generated column or an expression index; a view or a trigger
/// in any case is still refused. Likewise (#619) a `type`, `name`, `tbl_name`
/// or `sql` cell stored as a BLOB of valid UTF-8 - one flipped bit in the
/// record's serial type turns TEXT into a BLOB of the same bytes, which SQLite
/// still reads as that text - is read as that text and checked as text is
/// (a type in any case), instead of refused. Only in a UTF-8 file: SQLite
/// reads a BLOB cell as text in the file's own encoding, so in a UTF-16 file
/// it would read other text than the guard. Every other open keeps the exact
/// spelling and holds all four cells to text.
pub(super) fn check_schema_for_salvage(conn: &Connection) -> anyhow::Result<()> {
    guard(conn, TypeSpelling::AnyCase)
}

/// How [`refused_by_master`] reads a row's `type`, and whether it reads its
/// `type`, `name`, `tbl_name` and `sql` cells stored as a BLOB.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TypeSpelling {
    /// Only exactly `table` or `index` is accepted, and all four cells only
    /// stored as text (`sql` may be NULL).
    Exact,
    /// `table` or `index` in any ASCII case is accepted as that type, and, in
    /// a UTF-8 file, any of the four cells stored as a BLOB of valid UTF-8 is
    /// read as text: the salvage's [`check_schema_for_salvage`].
    AnyCase,
}

fn guard(conn: &Connection, spelling: TypeSpelling) -> anyhow::Result<()> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    match refused_object(conn, spelling)? {
        Some(refused) => Err(anyhow::Error::new(refused)),
        None => Ok(()),
    }
}

/// [#619] Install SQLite's authorizer on `conn`, a connection to a file from
/// outside the app, before its first statement. It refuses creating or dropping
/// any virtual table, reading an eponymous virtual table of a module the repo's
/// build leaves out (`DENIED_TABLES` in this module), and calling a function
/// that extension loading or one of those modules registers
/// (`DENIED_FUNCTIONS`); everything else is allowed. A refused statement fails
/// to prepare, with `SQLITE_AUTH`, or with `SQLITE_ERROR` for a refused function
/// call, which is how SQLite reports that one. It replaces any authorizer
/// `conn` already had.
///
/// The repo-root `.cargo/config.toml` builds the bundled SQLite without
/// FTS3/4/5, R-tree, DBSTAT and extension loading (#608), but cargo reads that
/// file only inside the clone, so the crates.io build of this crate and of
/// `stalx-reader` has them all. The authorizer holds in either build: it is what
/// the published crates rely on, and the build flags stay as defence in depth.
/// The app creates no virtual table and calls none of these functions.
///
/// Every opener in this crate that runs the schema guard installs it. It is
/// public for a crate that opens its own connection to such a file:
/// `stalx-reader` installs it on the connection it inspects an upload with,
/// which parses the file's schema before the load does. Call it right after
/// the open, before any statement is prepared.
pub fn deny_stripped_sqlite_features(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.authorizer(Some(authorize_untrusted))
}

/// The eponymous virtual tables of the modules the repo's build leaves out: a
/// module whose `xCreate` is its `xConnect` can be read by name, with no
/// `CREATE VIRTUAL TABLE`. From the SQLite 3.54.1 amalgamation bundled in
/// libsqlite3-sys 0.39.1 (`sqlite3/sqlite3.c`): `dbstat_module` (dbstat.c),
/// `fts3tok_module` (fts3_tokenize_vtab.c), `fts3aux_module` (fts3_aux.c) and
/// `fts5structure_module` (fts5_index.c, only with SQLITE_TEST or
/// SQLITE_FTS5_DEBUG). Matched in any letter case and in any schema.
const DENIED_TABLES: &[&str] = &["dbstat", "fts3tokenize", "fts4aux", "fts5_structure"];

/// The SQL functions extension loading and the left-out modules register, from
/// the same amalgamation, matched in any letter case. DBSTAT registers none.
/// - func.c `aBuiltinFunc`, unless SQLITE_OMIT_LOAD_EXTENSION: `load_extension`.
/// - fts3.c `sqlite3Fts3Init` and fts3_tokenizer.c `sqlite3Fts3InitHashTable`:
///   `fts3_tokenizer` and the overloads `snippet`, `offsets`, `matchinfo`,
///   `optimize`; with SQLITE_TEST also `fts3_tokenizer_test`,
///   `fts3_tokenizer_internal_test`, and `fts3_exprtest` and
///   `fts3_exprtest_rebalance` (fts3_expr.c).
/// - fts5.c `fts5Init`: `fts5`, `fts5_source_id`, `fts5_locale`,
///   `fts5_insttoken`, and through `sqlite3Fts5AuxInit` the overloads `snippet`,
///   `highlight`, `bm25`, `fts5_get_locale`; with SQLITE_TEST or
///   SQLITE_FTS5_DEBUG also `fts5_expr`, `fts5_expr_tcl`, `fts5_isalnum`,
///   `fts5_fold` (`sqlite3Fts5ExprInit`) and `fts5_decode`, `fts5_decode_none`,
///   `fts5_rowid` (`sqlite3Fts5IndexInit`).
/// - rtree.c `sqlite3RtreeInit`: `rtreenode`, `rtreedepth`, `rtreecheck`; with
///   SQLITE_DEBUG also `rtree_round`; with SQLITE_ENABLE_GEOPOLY the `geopoly_*`
///   functions of geopoly.c `sqlite3_geopoly_init`.
const DENIED_FUNCTIONS: &[&str] = &[
    "load_extension",
    "fts3_tokenizer",
    "snippet",
    "offsets",
    "matchinfo",
    "optimize",
    "fts3_tokenizer_test",
    "fts3_tokenizer_internal_test",
    "fts3_exprtest",
    "fts3_exprtest_rebalance",
    "fts5",
    "fts5_source_id",
    "fts5_locale",
    "fts5_insttoken",
    "highlight",
    "bm25",
    "fts5_get_locale",
    "fts5_expr",
    "fts5_expr_tcl",
    "fts5_isalnum",
    "fts5_fold",
    "fts5_decode",
    "fts5_decode_none",
    "fts5_rowid",
    "rtreenode",
    "rtreedepth",
    "rtreecheck",
    "rtree_round",
    "geopoly_area",
    "geopoly_blob",
    "geopoly_json",
    "geopoly_svg",
    "geopoly_within",
    "geopoly_contains_point",
    "geopoly_overlap",
    "geopoly_debug",
    "geopoly_bbox",
    "geopoly_xform",
    "geopoly_regular",
    "geopoly_ccw",
    "geopoly_group_bbox",
];

/// The authorizer [`deny_stripped_sqlite_features`] installs.
fn authorize_untrusted(ctx: AuthContext<'_>) -> Authorization {
    let listed = |names: &[&str], name: &str| names.iter().any(|n| n.eq_ignore_ascii_case(name));
    let denied = match ctx.action {
        AuthAction::CreateVtable { .. } | AuthAction::DropVtable { .. } => true,
        AuthAction::Read { table_name, .. } => listed(DENIED_TABLES, table_name),
        AuthAction::Function { function_name } => listed(DENIED_FUNCTIONS, function_name),
        _ => false,
    };
    if denied {
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}

/// The first object [`check_schema`] would refuse, if any.
///
/// Two passes. The first reads only `sqlite_master` and refuses pragma-shadowing
/// names, unexpected types, virtual tables and CHECKs the app never writes (see
/// [`check_schema`]). Only once none is left, so no pragma can be shadowed or
/// pointed at a virtual table, the second asks SQLite's pragmas about every
/// table and index and refuses anything else that evaluates an expression the
/// file chose whenever a table is read, checked or written - which the open
/// path's integrity check, migrations and saves all do: a generated column, an
/// expression or partial index, or a `DEFAULT (expr)`. trusted_schema off
/// still allows built-ins such as `zeroblob`, so one crafted CHECK could
/// otherwise stall an open for minutes and hundreds of megabytes per row. The
/// app has never written any of these.
pub fn refused_schema_object(conn: &Connection) -> rusqlite::Result<Option<RefusedSchemaObject>> {
    refused_object(conn, TypeSpelling::Exact)
}

fn refused_object(
    conn: &Connection,
    spelling: TypeSpelling,
) -> rusqlite::Result<Option<RefusedSchemaObject>> {
    if let Some(refused) = refused_by_master(conn, spelling)? {
        return Ok(Some(refused));
    }
    refused_by_expression(conn)
}

fn refused_by_master(
    conn: &Connection,
    spelling: TypeSpelling,
) -> rusqlite::Result<Option<RefusedSchemaObject>> {
    use rusqlite::types::ValueRef;
    let text = |v: ValueRef<'_>| match v {
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        _ => None,
    };
    // [#619] The text of a `type`, `name`, `tbl_name` or `sql` cell. The
    // salvage also reads a BLOB of valid UTF-8 as text, in a UTF-8 file only
    // (see [`check_schema_for_salvage`]): SQLite reads a BLOB cell as text in
    // the file's encoding. `PRAGMA encoding` loads the schema first, which
    // sets the encoding it reports.
    let blob_as_text = spelling == TypeSpelling::AnyCase
        && conn.query_row("PRAGMA encoding", [], |r| r.get::<_, String>(0))? == "UTF-8";
    let stored_text = |v: ValueRef<'_>| match v {
        ValueRef::Blob(b) if blob_as_text => std::str::from_utf8(b).ok().map(str::to_owned),
        v => text(v),
    };
    // [#619] Every identifier SQLite keeps for the file comes from the `name`
    // and `sql` cells, and SQLite hands identifiers to the authorizer
    // ([`deny_stripped_sqlite_features`]), whose safe rusqlite wrapper panics
    // on one that is not UTF-8. So a row whose bytes are not UTF-8, as text or
    // as a blob, is refused here, before a statement can pass one on.
    // `tbl_name` is held to UTF-8 too: SQLite reports a row whose `tbl_name`
    // differs from the table its SQL names as a malformed schema, but never
    // reads that cell on an autoindex row, which has no SQL.
    let utf8 = |v: ValueRef<'_>| match v {
        ValueRef::Text(b) | ValueRef::Blob(b) => std::str::from_utf8(b).is_ok(),
        _ => true,
    };
    let mut stmt = conn.prepare("SELECT type, name, rootpage, sql, tbl_name FROM sqlite_master")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if !(utf8(row.get_ref(1)?) && utf8(row.get_ref(3)?) && utf8(row.get_ref(4)?)) {
            return Ok(Some(RefusedSchemaObject {
                what: "a schema object whose name, table name or SQL is not valid UTF-8",
                name: text(row.get_ref(1)?).unwrap_or_default(),
            }));
        }
        // Read loosely: a crafted row may store any column as any type, and that
        // must come back as this refusal, not as a conversion error.
        let (Some(kind), Some(name)) = (stored_text(row.get_ref(0)?), stored_text(row.get_ref(1)?))
        else {
            return Ok(Some(RefusedSchemaObject {
                what: "a schema object of an unexpected type",
                name: String::new(),
            }));
        };
        // [#619] SQLite builds the schema from the `sql` cell's bytes whatever
        // its storage class, so a CHECK stored as a BLOB is parsed and enforced
        // like one stored as text, but a cell `stored_text` cannot read would
        // reach every check below as no SQL at all, and pass them. `tbl_name`
        // is held to text too.
        let sql_cell = row.get_ref(3)?;
        let sql = stored_text(sql_cell);
        if stored_text(row.get_ref(4)?).is_none()
            || (sql.is_none() && !matches!(sql_cell, ValueRef::Null))
        {
            return Ok(Some(RefusedSchemaObject {
                what: "a schema object whose table name or SQL is not stored as text",
                name,
            }));
        }
        let rootpage = match row.get_ref(2)? {
            ValueRef::Integer(page) => Some(page),
            _ => None,
        };
        let sql = sql.as_deref().unwrap_or("");
        let kind = match spelling {
            TypeSpelling::Exact => kind,
            TypeSpelling::AnyCase => kind.to_ascii_lowercase(),
        };
        let what = if name.to_ascii_lowercase().starts_with("pragma_") {
            Some("an object named like a SQLite pragma")
        } else if kind != "table" && kind != "index" {
            Some(match kind.to_ascii_lowercase().as_str() {
                "view" => "a view",
                "trigger" => "a trigger",
                _ => "a schema object of an unexpected type",
            })
        } else if rootpage == Some(0) || (kind == "table" && declares_virtual_table(sql)) {
            Some("a virtual table")
        } else if kind == "table" && !checks_are_the_apps(sql) {
            Some("a table with a CHECK constraint the app never writes")
        } else {
            None
        };
        if let Some(what) = what {
            return Ok(Some(RefusedSchemaObject { what, name }));
        }
    }
    Ok(None)
}

/// The type tests read `lower(m.type)`: under [`TypeSpelling::Exact`] the
/// first pass has already refused any other spelling, and under
/// [`TypeSpelling::AnyCase`] a 'Table' row must be checked like a 'table' one.
/// Likewise `CAST(m.name AS TEXT)` (#619): the salvage's first pass lets a
/// `type` or `name` stored as a BLOB of UTF-8 through, which `lower` and the
/// pragmas read as text too.
fn refused_by_expression(conn: &Connection) -> rusqlite::Result<Option<RefusedSchemaObject>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(m.name AS TEXT), c.hidden, c.dflt_value \
         FROM sqlite_master AS m, pragma_table_xinfo(m.name) AS c WHERE lower(m.type) = 'table'",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let table: String = row.get(0)?;
        let hidden: i64 = row.get(1)?;
        let default: Option<String> = row.get(2)?;
        if hidden == 2 || hidden == 3 {
            return Ok(Some(RefusedSchemaObject {
                what: "a table with a generated column",
                name: table,
            }));
        }
        if default.as_deref().is_some_and(|d| !is_literal_default(d)) {
            return Ok(Some(RefusedSchemaObject {
                what: "a table with an expression default",
                name: table,
            }));
        }
    }
    let mut stmt = conn.prepare(
        "SELECT i.name FROM sqlite_master AS m, pragma_index_list(m.name) AS i \
         WHERE lower(m.type) = 'table' AND i.partial = 1 \
         UNION ALL \
         SELECT CAST(m.name AS TEXT) FROM sqlite_master AS m, pragma_index_xinfo(m.name) AS x \
         WHERE lower(m.type) = 'index' AND x.cid = -2",
    )?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(RefusedSchemaObject {
            what: "an expression or partial index",
            name: row.get(0)?,
        }));
    }
    Ok(None)
}

/// Whether a `sqlite_master.sql` text declares a virtual table: its first two
/// keywords, after any whitespace and comments, are `CREATE VIRTUAL`. A token
/// match, not a substring one, so a legacy column such as `is_virtual` does not
/// count. An unterminated comment counts as virtual, so a malformed text is
/// treated as the dangerous case.
pub fn declares_virtual_table(sql: &str) -> bool {
    let mut rest = sql;
    let mut words = Vec::with_capacity(2);
    while words.len() < 2 {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("/*") {
            match after.find("*/") {
                Some(end) => rest = &after[end + 2..],
                None => return true,
            }
        } else if let Some(after) = rest.strip_prefix("--") {
            rest = after.find('\n').map_or("", |end| &after[end..]);
        } else {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if end == 0 {
                break;
            }
            words.push(rest[..end].to_ascii_uppercase());
            rest = &rest[end..];
        }
    }
    words == ["CREATE", "VIRTUAL"]
}

/// Whether a column default, as `pragma_table_xinfo` reports it (an expression
/// default loses its parentheses there), is a plain literal: a number, a string
/// or blob literal, NULL, TRUE/FALSE or CURRENT_TIME/DATE/TIMESTAMP.
fn is_literal_default(default: &str) -> bool {
    let d = default.trim();
    let quoted = |body: &str| {
        body.len() >= 2
            && body.starts_with('\'')
            && body.ends_with('\'')
            && !body[1..body.len() - 1].replace("''", "").contains('\'')
    };
    let upper = d.to_ascii_uppercase();
    let unsigned = d.trim_start_matches(['+', '-']);
    matches!(
        upper.as_str(),
        "NULL" | "TRUE" | "FALSE" | "CURRENT_TIME" | "CURRENT_DATE" | "CURRENT_TIMESTAMP"
    ) || quoted(d)
        || (upper.starts_with("X'") && quoted(&d[1..]))
        || (!unsigned.is_empty()
            && unsigned
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
            && unsigned.parse::<f64>().is_ok())
}

/// Whether every CHECK constraint in a table's `CREATE TABLE` text is the app's
/// own singleton guard, `CHECK (id = 1)`. Scans for the keyword anywhere, so a
/// spelling this misreads is refused, never allowed.
fn checks_are_the_apps(sql: &str) -> bool {
    let upper = sql.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(at) = upper[from..].find("CHECK").map(|i| from + i) {
        from = at + "CHECK".len();
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(from).copied();
        if before.is_some_and(word) || after.is_some_and(word) {
            continue; // part of a longer identifier, e.g. `checked_at`
        }
        let rest = upper[from..].trim_start();
        let Some(body) = rest.strip_prefix('(') else {
            return false;
        };
        let Some(end) = body.find(')') else {
            return false;
        };
        let expr: String = body[..end].chars().filter(|c| !c.is_whitespace()).collect();
        if expr != "ID=1" {
            return false;
        }
    }
    true
}
