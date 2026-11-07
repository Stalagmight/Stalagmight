//! Refusing `.stalx` files whose schema could run SQL the file chose (#608).
//!
//! A `.stalx` is a SQLite database anyone can send, so its schema is input.
//! The app never creates views, triggers or virtual tables, so a file that has
//! one was edited by something else, and opening it would let the file run its
//! own SQL or a virtual-table module's code. [`check_schema`] refuses such a
//! file before anything else reads it; the `stalx-reader` crate runs the same
//! check on server uploads.

use rusqlite::config::DbConfig;
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
/// in any case is still refused. Every other open keeps the exact spelling.
pub(super) fn check_schema_for_salvage(conn: &Connection) -> anyhow::Result<()> {
    guard(conn, TypeSpelling::AnyCase)
}

/// How [`refused_by_master`] reads a row's `type`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TypeSpelling {
    /// Only exactly `table` or `index` is accepted.
    Exact,
    /// `table` or `index` in any ASCII case is accepted as that type.
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
    let mut stmt = conn.prepare("SELECT type, name, rootpage, sql FROM sqlite_master")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        // Read loosely: a crafted row may store any column as any type, and that
        // must come back as this refusal, not as a conversion error.
        let (Some(kind), Some(name)) = (text(row.get_ref(0)?), text(row.get_ref(1)?)) else {
            return Ok(Some(RefusedSchemaObject {
                what: "a schema object of an unexpected type",
                name: String::new(),
            }));
        };
        let rootpage = match row.get_ref(2)? {
            ValueRef::Integer(page) => Some(page),
            _ => None,
        };
        let sql = text(row.get_ref(3)?);
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
fn refused_by_expression(conn: &Connection) -> rusqlite::Result<Option<RefusedSchemaObject>> {
    let mut stmt = conn.prepare(
        "SELECT m.name, c.hidden, c.dflt_value \
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
         SELECT m.name FROM sqlite_master AS m, pragma_index_xinfo(m.name) AS x \
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
