//! Test-support: read every page of the database through THIS connection.
//!
//! The #706 fuzzer (`commands::mmap_fault_fuzz_tests` in the app crate) needs a
//! probe that goes through the LIVE connection the open project holds, not a
//! fresh one: the hazard it hunts is a mapping that connection set up over an
//! inode some other write has since truncated. A fresh connection would map the
//! file as it is now and could never fault.

use rusqlite::types::ValueRef;

use super::Database;

/// The TEMP table [`Database::mark_live_connection_for_tests`] plants. TEMP
/// objects belong to one connection and never reach the main database file.
const LIVE_MARKER: &str = "fz706_live_marker";

/// What one [`Database::scan_live_for_tests`] saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveScan {
    /// `PRAGMA mmap_size` on this connection: how much of the file it maps.
    pub mmap_size: i64,
    /// `PRAGMA page_count`.
    pub page_count: i64,
    /// The first line `PRAGMA quick_check` returned (`"ok"` when healthy).
    pub quick_check: String,
    /// Tables in the main `sqlite_schema`.
    pub tables: usize,
    /// Rows read across every table.
    pub rows: u64,
    /// Rows in the `project` table; `None` when the table is gone.
    pub project_rows: Option<i64>,
    /// FNV-1a over every table name and every value read, in table-name and
    /// rowid order: two connections that see the same content agree on it.
    pub digest: u64,
}

/// FNV-1a, folded in place.
fn fold(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
}

impl Database {
    /// Walk the whole database through this connection: `PRAGMA quick_check`
    /// (every b-tree page, overflow chains included) and then a `SELECT *` over
    /// every table, reading every column of every row so no page that holds
    /// data is skipped.
    ///
    /// Read-only. An `Err` is the connection reporting damage (or a file that
    /// is no longer a database); a fault in the mapping is not an `Err` at all,
    /// it kills the process, which is what the caller is watching for.
    pub fn scan_live_for_tests(&self) -> rusqlite::Result<LiveScan> {
        let mmap_size: i64 = self.conn.query_row("PRAGMA mmap_size", [], |r| r.get(0))?;
        let page_count: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let quick_check: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        let names: Vec<String> = self
            .conn
            .prepare("SELECT name FROM main.sqlite_schema WHERE type = 'table' ORDER BY name")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut digest = 0xcbf2_9ce4_8422_2325_u64;
        let mut rows = 0u64;
        let mut project_rows = None;
        for name in &names {
            fold(&mut digest, name.as_bytes());
            let quoted = name.replace('"', "\"\"");
            // `rowid` order where there is one, so the digest is a function of
            // the content and not of the plan; WITHOUT ROWID tables (none in the
            // schema today) fall back to the natural order.
            let ordered = format!("SELECT * FROM main.\"{quoted}\" ORDER BY rowid");
            let mut stmt = match self.conn.prepare(&ordered) {
                Ok(stmt) => stmt,
                Err(_) => self
                    .conn
                    .prepare(&format!("SELECT * FROM main.\"{quoted}\""))?,
            };
            let columns = stmt.column_count();
            let mut cursor = stmt.query([])?;
            let mut here = 0i64;
            while let Some(row) = cursor.next()? {
                for i in 0..columns {
                    // Materialise every value, so an overflow chain is followed.
                    match row.get_ref(i)? {
                        ValueRef::Null => fold(&mut digest, b"n"),
                        ValueRef::Integer(v) => {
                            fold(&mut digest, b"i");
                            fold(&mut digest, &v.to_le_bytes());
                        }
                        ValueRef::Real(v) => {
                            fold(&mut digest, b"r");
                            fold(&mut digest, &v.to_bits().to_le_bytes());
                        }
                        ValueRef::Text(v) => {
                            fold(&mut digest, b"t");
                            fold(&mut digest, v);
                        }
                        ValueRef::Blob(v) => {
                            fold(&mut digest, b"b");
                            fold(&mut digest, v);
                        }
                    }
                }
                here += 1;
            }
            rows += here as u64;
            if name == "project" {
                project_rows = Some(here);
            }
        }
        Ok(LiveScan {
            mmap_size,
            page_count,
            quick_check,
            tables: names.len(),
            rows,
            project_rows,
            digest,
        })
    }

    /// Tell this connection apart from any other: `Ok(true)` when a previous
    /// call already marked THIS connection, `Ok(false)` when it is one the
    /// caller has not seen (it is marked now). The mark is a TEMP table, so it
    /// lives only as long as the connection and never touches the file.
    pub fn mark_live_connection_for_tests(&self) -> rusqlite::Result<bool> {
        let seen: i64 = self.conn.query_row(
            "SELECT count(*) FROM temp.sqlite_schema WHERE name = ?1",
            [LIVE_MARKER],
            |r| r.get(0),
        )?;
        if seen > 0 {
            return Ok(true);
        }
        self.conn
            .execute_batch(&format!("CREATE TEMP TABLE {LIVE_MARKER}(x)"))?;
        Ok(false)
    }
}
