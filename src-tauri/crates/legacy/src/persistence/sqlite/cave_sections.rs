//! Sections - named groupings of lines (#327) - in `cave_sections` and
//! `cave_section_lines`.
//!
//! - `cave_sections (id, name, color, position)`: one row per Section,
//!   `position` its place in the Sections panel (0-based).
//! - `cave_section_lines (line_id, cave_section_id)`: one row per grouped line,
//!   `line_id` the line's id (`sections.id` - the lines table keeps its pre-
//!   rename name, see `disk_names`).
//!
//! Like `station_names`, the tables arrived WITHOUT a schema bump: SpeleoDB's
//! Python reader pins the schema version (exactly 18 before its 0.0.2) and
//! reads only the tables it knows, by explicit column lists, so new tables are invisible to it (see
//! `station_texts` for the version of it this was verified against). Every
//! save drops and recreates both, inside its transaction, so a v18 file opened
//! through the fast path (no migration) gets them on its first save. The price:
//! a build without them drops the Sections on its next save (each save
//! rebuilds the file), and the lines read back ungrouped.
//!
//! Reading tolerates their absence (a file older than them, a read-only open)
//! and their being of another shape - another tool, a future build - which
//! reads as no Sections with a warning, never a failed open. SQLite failing for
//! a reason of its own still fails the open, as for the credit tables, so a
//! save cannot then drop the rows for good. A row that cannot be read, or
//! that names a line or a Section the file does not hold, is skipped and
//! counted; [`crate::models::Project::normalize_sections`] then settles the
//! rest (repeated names, the id counter).
//!
//! The id counter (`Project::next_section_id`) is the `project_settings` row
//! `next_cave_section_id`, so a Section id deleted (or undone) before a save
//! is never handed out again after a reopen - a view setting such as
//! `hiddenSections` names Sections by id. Read tolerantly: a missing or
//! unreadable row, or one past `MAX_SECTION_ID`, is derived from the ids, and
//! one below the saved ids is raised above them. Each table's read is capped
//! ([`MAX_CAVE_SECTION_ROWS`], [`MAX_CAVE_SECTION_LINE_ROWS`]): a crafted file
//! cannot make the open unbounded.
//!
//! Plain DDL only: `schema_guard` refuses a file with CHECKs other than
//! `id = 1`, partial or expression indexes, `DEFAULT (expr)`, views and
//! triggers, and these tables must not be the reason a file is refused.

use std::collections::HashSet;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, Transaction};

use crate::models::{LineId, Project, Section, SectionId, MAX_SECTION_ID};

use super::shot_credits::{extended_code, is_schema_error};

/// The `project_settings` key of `Project::next_section_id`.
pub(super) const NEXT_CAVE_SECTION_ID_KEY: &str = "next_cave_section_id";

/// The most `cave_sections` rows a read takes, in panel order; the rest are
/// skipped with a warning. The tables are not bounded by anything the user did
/// (a crafted file can hold millions of rows), and the open must stay bounded:
/// the cap matches `stalx-reader`'s default `CountLimits::cave_sections`.
pub(super) const MAX_CAVE_SECTION_ROWS: usize = 100_000;

/// The most `cave_section_lines` rows a read takes beyond one per line (a
/// line belongs to at most one Section, so a file the app wrote never holds
/// more than the lines); as `stalx-reader`'s `CountLimits::cave_section_lines`.
pub(super) const MAX_CAVE_SECTION_LINE_ROWS: usize = 100_000;

/// Run by `save_project` inside its transaction, after the lines are written:
/// both tables are rewritten whole, so they are recreated rather than emptied
/// (which also replaces a table of another shape).
pub(super) const RECREATE_CAVE_SECTION_TABLES: &str = "
    DROP TABLE IF EXISTS cave_section_lines;
    DROP TABLE IF EXISTS cave_sections;
    CREATE TABLE cave_sections (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        color TEXT NOT NULL,
        position INTEGER NOT NULL
    );
    CREATE TABLE cave_section_lines (
        line_id INTEGER PRIMARY KEY REFERENCES sections(id) ON DELETE CASCADE,
        cave_section_id INTEGER NOT NULL REFERENCES cave_sections(id) ON DELETE CASCADE
    );
";

/// Write `project`'s Sections and memberships. A membership naming a
/// Section the project does not hold is not written (it reads as ungrouped).
pub(super) fn write_cave_sections(tx: &Transaction<'_>, project: &Project) -> Result<()> {
    tx.execute_batch(RECREATE_CAVE_SECTION_TABLES)?;
    let mut insert_section = tx.prepare_cached(
        "INSERT INTO cave_sections (id, name, color, position) VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut held: HashSet<SectionId> = HashSet::with_capacity(project.sections.len());
    for (position, section) in project.sections.iter().enumerate() {
        if !held.insert(section.id) {
            continue;
        }
        insert_section.execute(params![
            section.id,
            section.name,
            section.color,
            i64::try_from(position).unwrap_or(i64::MAX)
        ])?;
    }
    let mut insert_member = tx.prepare_cached(
        "INSERT INTO cave_section_lines (line_id, cave_section_id) VALUES (?1, ?2)",
    )?;
    for line in &project.lines {
        if let Some(section) = line.section_id.filter(|id| held.contains(id)) {
            insert_member.execute(params![line.id, section])?;
        }
    }
    tx.execute(
        "INSERT OR REPLACE INTO project_settings (key, value) VALUES (?1, ?2)",
        params![
            NEXT_CAVE_SECTION_ID_KEY,
            project.next_section_id.to_string()
        ],
    )?;
    Ok(())
}

/// The stored `next_cave_section_id`, or `None` when the row is missing,
/// does not read as an id, or is past [`MAX_SECTION_ID`] - no counter this
/// build keeps (`normalize_sections` then derives it).
fn load_next_cave_section_id(conn: &Connection) -> Option<SectionId> {
    conn.query_row(
        "SELECT value FROM project_settings WHERE key = ?1",
        [NEXT_CAVE_SECTION_ID_KEY],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|value| value.trim().parse::<SectionId>().ok())
    .filter(|next| *next <= MAX_SECTION_ID)
}

/// Whether `table` exists as a table.
fn has_table(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .with_context(|| format!("Failed to look for the {table} table"))?;
    Ok(count > 0)
}

/// What one read step came to: rows, a table of another shape (no rows, with
/// a warning), or SQLite failing for its own reason (the open fails).
fn shape_or_fail<T: Default>(table: &'static str, read: rusqlite::Result<T>) -> Result<T> {
    match read {
        Ok(rows) => Ok(rows),
        Err(error) if is_schema_error(&error) => {
            tracing::warn!(
                table,
                sqlite_code = ?extended_code(&error),
                purpose = "read sections",
                "A section table cannot be read; the project opens without sections"
            );
            Ok(T::default())
        }
        Err(error) => {
            Err(anyhow::Error::new(error).context(format!("Failed to read the {table} table")))
        }
    }
}

type SectionRow = (
    rusqlite::Result<SectionId>,
    rusqlite::Result<String>,
    rusqlite::Result<String>,
    rusqlite::Result<i64>,
);

type MemberRow = (rusqlite::Result<LineId>, rusqlite::Result<SectionId>);

/// At most `cap` rows, in panel order (the cap keeps the first Sections of
/// the panel), plus one more so the caller can tell the table was cut.
fn read_section_rows(conn: &Connection, cap: usize) -> rusqlite::Result<Vec<SectionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, color, position FROM cave_sections ORDER BY position, id LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([row_limit(cap)], |row| {
            Ok((row.get(0), row.get(1), row.get(2), row.get(3)))
        })?
        .collect::<rusqlite::Result<Vec<SectionRow>>>()?;
    Ok(rows)
}

/// At most `cap` rows, plus one more so the caller can tell the table was cut.
fn read_member_rows(conn: &Connection, cap: usize) -> rusqlite::Result<Vec<MemberRow>> {
    let mut stmt =
        conn.prepare("SELECT line_id, cave_section_id FROM cave_section_lines LIMIT ?1")?;
    let rows = stmt
        .query_map([row_limit(cap)], |row| Ok((row.get(0), row.get(1))))?
        .collect::<rusqlite::Result<Vec<MemberRow>>>()?;
    Ok(rows)
}

/// `cap + 1` as SQLite's `LIMIT`: one row past the cap tells it was reached.
fn row_limit(cap: usize) -> i64 {
    i64::try_from(cap.saturating_add(1)).unwrap_or(i64::MAX)
}

/// Cut `rows` to `cap`, warning when that dropped any.
fn capped<T>(mut rows: Vec<T>, cap: usize, table: &'static str) -> Vec<T> {
    if rows.len() > cap {
        rows.truncate(cap);
        tracing::warn!(
            table,
            cap,
            purpose = "read sections",
            "A section table holds more rows than any project; reading only the first"
        );
    }
    rows
}

/// Read the Sections and memberships into `project` (whose lines are
/// loaded), then normalize them. A file without the tables reads as no
/// Sections.
pub(super) fn load_cave_sections(conn: &Connection, project: &mut Project) -> Result<()> {
    project.sections.clear();
    // The saved counter, raised above the saved ids by the normalization
    // below: max(persisted, highest id + 1).
    project.next_section_id = load_next_cave_section_id(conn).unwrap_or(0);
    if !has_table(conn, "cave_sections")? {
        return Ok(());
    }
    let mut skipped = 0usize;
    let mut sections: Vec<(i64, Section)> = Vec::new();
    let rows = shape_or_fail(
        "cave_sections",
        read_section_rows(conn, MAX_CAVE_SECTION_ROWS),
    )?;
    for (id, name, color, position) in capped(rows, MAX_CAVE_SECTION_ROWS, "cave_sections") {
        let (Ok(id), Ok(name), Ok(color), Ok(position)) = (id, name, color, position) else {
            skipped += 1;
            continue;
        };
        sections.push((position, Section { id, name, color }));
    }
    // Panel order; a tie (another tool's rows) keeps id order.
    sections.sort_by_key(|(position, section)| (*position, section.id));
    project.sections = sections.into_iter().map(|(_, s)| s).collect();
    // Repeated ids keep their first row, before memberships are matched.
    let mut held: HashSet<SectionId> = HashSet::new();
    project.sections.retain(|s| held.insert(s.id));

    if has_table(conn, "cave_section_lines")? {
        let cap = MAX_CAVE_SECTION_LINE_ROWS.max(project.lines.len());
        let rows = shape_or_fail("cave_section_lines", read_member_rows(conn, cap))?;
        // Looked up here, not through `set_line_section`: a row naming no line
        // misses the line index and would scan every line.
        let lines: HashSet<LineId> = project.lines.iter().map(|line| line.id).collect();
        for (line_id, section) in capped(rows, cap, "cave_section_lines") {
            let (Ok(line_id), Ok(section)) = (line_id, section) else {
                skipped += 1;
                continue;
            };
            if !held.contains(&section)
                || !lines.contains(&line_id)
                || !project.set_line_section(line_id, Some(section))
            {
                skipped += 1;
            }
        }
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            purpose = "read sections",
            "Skipped unreadable or dangling section rows"
        );
    }
    project.normalize_sections();
    Ok(())
}
