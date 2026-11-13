//! Per-shot survey credit: `credit_people` and `shot_credits`.
//!
//! - `credit_people (id, name, email)` holds each distinct person once.
//! - `shot_credits (section_id, shot_uuid, date, surveyors, explorers)`, keyed
//!   by `(section_id, shot_uuid)`, holds a shot's credit, the people columns as
//!   comma-joined `credit_people` ids.
//!
//! Like `station_names`, the tables arrived WITHOUT a schema bump: SpeleoDB's
//! Python reader (see `station_texts`) accepts exactly v18, ignores tables it
//! does not know, and validates `sections.surveyor` as one string and
//! `sections.date` as one ISO date. Those two legacy columns are still written,
//! as the section aggregate: `sections.date` is the earliest shot date, and
//! `sections.surveyor` is [`crate::models::legacy_credit_text`] - the distinct
//! surveyor names joined with `", "` (the text it always held), or, when anyone
//! explored the section, the Ariane `"explorers / surveyors"` form that older
//! Ariane imports already stored there. A `" / "` inside a name is written
//! `" - "` there, so the two halves stay unambiguous.
//!
//! Rows are keyed by `(section_id, shot_uuid)`: `sections.id` is persisted,
//! and a file written before shot uuids were project-unique can hold one uuid
//! in several sections, each with its own credit.
//!
//! A released build (0.5.3 and older) does NOT keep these tables: it saves by
//! writing a fresh database beside the project and renaming it over the file,
//! so after one save there only the legacy columns are left, and every shot
//! falls back to them. That is why the surveyor column carries both roles
//! (#718): each section still lists who explored and who surveyed it. What
//! collapses is expected: per-shot differences become the section aggregate,
//! the date becomes the earliest one, emails are gone, and two one-word names
//! in one role read back as one `Last, First` person (the splitter's rule).
//!
//! The tables can still end up next to a legacy pair this build did not write
//! (a hand edit, another tool editing the file in place). So
//! `credit_sections (section_id, surveyor, date)` records the pair THIS build
//! wrote, and a section's rows apply only while its `sections` pair still
//! matches that record: once something else has retyped it, the typed text is
//! the newer truth and every shot of the section falls back to it.
//!
//! A shot WITHOUT a row takes its credit from those legacy columns through
//! [`crate::models::SurveyCredit::from_legacy`]. That is the migration for every
//! file written before this feature or saved since by an older build. The
//! writer relies on it in the other direction too: it writes a row only for a
//! shot whose credit the fallback would NOT reproduce, so an uncredited project
//! (and a section whose shots all agree with its legacy columns) costs nothing.
//! A row is explicit even when its lists are empty.
//!
//! Rows are keyed by the shot uuid `write.rs` mints or keeps. Nothing on read
//! is fatal (#717):
//! - A credit table that cannot be read as a whole - a view or other non-table
//!   object, a missing column, a read that fails part way, or a `credit_people`
//!   missing beside `shot_credits` - gives no rows at all, with a warning: every
//!   shot falls back to its section's legacy columns.
//! - Inside a readable table, an unreadable row or person is skipped (counted
//!   in a warning, never named), a dangling person id is dropped, and a row
//!   naming no shot of the project is ignored.

use std::collections::HashMap;

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::{params, Connection, Transaction};

use crate::models::{dedupe_people, Person, SurveyCredit, MAX_PEOPLE_PER_ROLE};

/// DDL for both tables, shared by `init_schema` and `save_project` (which runs
/// it inside its transaction, as it does for `station_names`, so a v18 file
/// opened through the fast path cannot fail the DELETE).
pub(super) const CREATE_CREDIT_TABLES: &str = "
    CREATE TABLE IF NOT EXISTS credit_people (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        email TEXT
    );
    CREATE TABLE IF NOT EXISTS credit_sections (
        section_id INTEGER PRIMARY KEY,
        surveyor TEXT,
        date TEXT
    );
    CREATE TABLE IF NOT EXISTS shot_credits (
        section_id INTEGER NOT NULL,
        shot_uuid TEXT NOT NULL,
        date TEXT,
        surveyors TEXT NOT NULL DEFAULT '',
        explorers TEXT NOT NULL DEFAULT '',
        PRIMARY KEY (section_id, shot_uuid)
    );
";

/// Run by `save_project` in place of a DELETE: every save rewrites both tables
/// whole, and recreating them also replaces the shape an unreleased build of
/// this feature wrote (rows keyed by uuid alone).
pub(super) const RECREATE_CREDIT_TABLES: &str = "
    DROP TABLE IF EXISTS shot_credits;
    DROP TABLE IF EXISTS credit_people;
    DROP TABLE IF EXISTS credit_sections;
";

/// A section's legacy `(sections.surveyor, sections.date)`, as stored text.
pub(super) type LegacyPair = (Option<String>, Option<String>);

/// A shot's address in `shot_credits`: its section and its uuid. A uuid alone
/// is not enough - a file written before uuids were project-unique can hold
/// one uuid in several sections, and each of those shots keeps its own credit.
pub(super) type CreditKey = (u32, String);

/// Write one row per `((section, uuid), credit)`, interning people. The caller
/// passes only the shots that need a row, each key once.
pub(super) fn write_credit_rows(
    tx: &Transaction<'_>,
    rows: &[(CreditKey, SurveyCredit)],
) -> Result<()> {
    let mut ids: HashMap<(String, Option<String>), i64> = HashMap::new();
    let mut insert_person =
        tx.prepare("INSERT INTO credit_people (id, name, email) VALUES (?1, ?2, ?3)")?;
    let mut insert_row = tx.prepare(
        "INSERT INTO shot_credits (section_id, shot_uuid, date, surveyors, explorers) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    let mut intern = |person: &Person| -> Result<i64> {
        let key = (person.name.clone(), person.email.clone());
        if let Some(id) = ids.get(&key) {
            return Ok(*id);
        }
        let id = i64::try_from(ids.len())
            .unwrap_or(i64::MAX)
            .saturating_add(1);
        insert_person.execute(params![id, person.name, person.email])?;
        ids.insert(key, id);
        Ok(id)
    };
    for ((section_id, uuid), credit) in rows {
        let mut join = |people: &[Person]| -> Result<String> {
            let mut out = Vec::with_capacity(people.len());
            for person in people {
                out.push(intern(person)?.to_string());
            }
            Ok(out.join(","))
        };
        let surveyors = join(&credit.surveyors)?;
        let explorers = join(&credit.explorers)?;
        insert_row.execute(params![
            section_id,
            uuid,
            credit.date.map(|d| d.to_string()),
            surveyors,
            explorers
        ])?;
    }
    Ok(())
}

/// Record, per section, the legacy pair this save wrote into `sections`.
pub(super) fn write_recorded_legacy(
    tx: &Transaction<'_>,
    section_id: u32,
    pair: (Option<&str>, Option<&str>),
) -> Result<()> {
    tx.execute(
        "INSERT INTO credit_sections (section_id, surveyor, date) VALUES (?1, ?2, ?3)",
        params![section_id, pair.0, pair.1],
    )?;
    Ok(())
}

/// The legacy pair recorded for each section in `stored` (rows naming no
/// section of the file are not kept). A missing table means "nothing
/// recorded", so no section's credit rows apply; so does an unreadable one,
/// with a warning. An unreadable row records nothing for its section.
pub(super) fn load_recorded_legacy(
    conn: &Connection,
    stored: &HashMap<u32, LegacyPair>,
) -> HashMap<u32, LegacyPair> {
    read_recorded_legacy(conn, stored).unwrap_or_else(|unreadable| {
        unreadable.warn();
        HashMap::new()
    })
}

/// [`load_recorded_legacy`] without the fallback: why the table is unusable.
pub(super) fn read_recorded_legacy(
    conn: &Connection,
    stored: &HashMap<u32, LegacyPair>,
) -> Result<HashMap<u32, LegacyPair>, Unreadable> {
    const TABLE: &str = "credit_sections";
    let mut recorded = HashMap::new();
    if !is_table(conn, TABLE)? {
        return Ok(recorded);
    }
    let mut stmt = conn
        .prepare("SELECT section_id, surveyor, date FROM credit_sections")
        .map_err(|e| Unreadable::new(TABLE, "prepare", e))?;
    let iter = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0),
                row.get::<_, Option<String>>(1),
                row.get::<_, Option<String>>(2),
            ))
        })
        .map_err(|e| Unreadable::new(TABLE, "query", e))?;
    let mut skipped = 0_usize;
    for row in iter {
        // The closure decodes nothing fallibly, so an error here is the
        // read itself failing part way: the rest of the table is unknown.
        let (section_id, surveyor, date) = row.map_err(|e| Unreadable::new(TABLE, "read", e))?;
        let (Ok(section_id), Ok(surveyor), Ok(date)) = (section_id, surveyor, date) else {
            skipped += 1;
            continue;
        };
        if stored.contains_key(&section_id) {
            recorded.insert(section_id, (surveyor, date));
        }
    }
    warn_skipped(TABLE, skipped, 0);
    Ok(recorded)
}

/// Every readable `shot_credits` row, by `(section, shot uuid)`.
///
/// Missing `shot_credits` reads as no rows. A credit table that cannot be read
/// as a whole - not a table, a column missing (another tool, a hand repair or
/// a future build wrote it), a read failing part way - also gives no rows,
/// with a warning, so every shot falls back to its section's legacy columns.
/// The rows need their people: a missing or unreadable `credit_people` beside
/// `shot_credits` voids them the same way. Inside readable tables, an
/// unreadable row or person is skipped and counted (see the module doc).
pub(super) fn load_credit_rows(conn: &Connection) -> HashMap<CreditKey, SurveyCredit> {
    read_credit_rows(conn).unwrap_or_else(|unreadable| {
        unreadable.warn();
        HashMap::new()
    })
}

/// [`load_credit_rows`] without the fallback: why a table is unusable.
pub(super) fn read_credit_rows(
    conn: &Connection,
) -> Result<HashMap<CreditKey, SurveyCredit>, Unreadable> {
    const TABLE: &str = "shot_credits";
    let mut rows = HashMap::new();
    if !is_table(conn, TABLE)? {
        return Ok(rows);
    }
    let people = read_people(conn)?;
    let resolve = |ids: &str| -> Vec<Person> {
        let list: Vec<Person> = ids
            .split(',')
            .filter_map(|token| token.trim().parse::<i64>().ok())
            .filter_map(|id| people.get(&id).cloned())
            .take(MAX_PEOPLE_PER_ROLE)
            .collect();
        dedupe_people(list)
    };
    // A table of another shape (an unreleased build of this feature keyed
    // rows by uuid alone) fails here.
    let mut stmt = conn
        .prepare("SELECT section_id, shot_uuid, date, surveyors, explorers FROM shot_credits")
        .map_err(|e| Unreadable::new(TABLE, "prepare", e))?;
    let iter = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0),
                row.get::<_, String>(1),
                row.get::<_, Option<String>>(2),
                row.get::<_, Option<String>>(3),
                row.get::<_, Option<String>>(4),
            ))
        })
        .map_err(|e| Unreadable::new(TABLE, "query", e))?;
    let (mut skipped, mut unreadable_values) = (0_usize, 0_usize);
    for row in iter {
        // As in `read_recorded_legacy`: only the read itself can fail here.
        let (section_id, uuid, date, surveyors, explorers) =
            row.map_err(|e| Unreadable::new(TABLE, "read", e))?;
        let (Ok(section_id), Ok(uuid)) = (section_id, uuid) else {
            skipped += 1;
            continue;
        };
        if uuid.is_empty() {
            skipped += 1;
            continue;
        }
        let mut readable = |value: rusqlite::Result<Option<String>>| {
            value.unwrap_or_else(|_| {
                unreadable_values += 1;
                None
            })
        };
        let date = readable(date).and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());
        let surveyors = readable(surveyors).unwrap_or_default();
        let explorers = readable(explorers).unwrap_or_default();
        rows.insert(
            (section_id, uuid),
            SurveyCredit {
                surveyors: resolve(&surveyors),
                explorers: resolve(&explorers),
                date,
            },
        );
    }
    warn_skipped(TABLE, skipped, unreadable_values);
    Ok(rows)
}

/// Every readable person of `credit_people`, by id. The table must exist: the
/// rows it serves cannot be read without it. An unreadable or blank person is
/// skipped and counted; a row naming them drops them like a dangling id.
fn read_people(conn: &Connection) -> Result<HashMap<i64, Person>, Unreadable> {
    const TABLE: &str = "credit_people";
    if !is_table(conn, TABLE)? {
        return Err(Unreadable::shape(TABLE, "missing"));
    }
    let mut stmt = conn
        .prepare("SELECT id, name, email FROM credit_people")
        .map_err(|e| Unreadable::new(TABLE, "prepare", e))?;
    let iter = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0),
                row.get::<_, String>(1),
                row.get::<_, Option<String>>(2),
            ))
        })
        .map_err(|e| Unreadable::new(TABLE, "query", e))?;
    let mut people = HashMap::new();
    let (mut skipped, mut unreadable_values) = (0_usize, 0_usize);
    for row in iter {
        // As in `read_recorded_legacy`: only the read itself can fail here.
        let (id, name, email) = row.map_err(|e| Unreadable::new(TABLE, "read", e))?;
        let (Ok(id), Ok(name)) = (id, name) else {
            skipped += 1;
            continue;
        };
        let email = email.unwrap_or_else(|_| {
            unreadable_values += 1;
            None
        });
        match (Person { name, email }).sanitized() {
            Some(person) => {
                people.insert(id, person);
            }
            None => skipped += 1,
        }
    }
    warn_skipped(TABLE, skipped, unreadable_values);
    Ok(people)
}

/// A credit table that cannot be read as a whole, for the warning: which
/// table, at which step, and SQLite's error, which names the table and its
/// columns but never a stored value. None of the table's text is kept.
#[derive(Debug)]
pub(super) struct Unreadable {
    pub(super) table: &'static str,
    pub(super) step: &'static str,
    error: Option<rusqlite::Error>,
}

impl Unreadable {
    fn new(table: &'static str, step: &'static str, error: rusqlite::Error) -> Self {
        Self {
            table,
            step,
            error: Some(error),
        }
    }

    /// Unusable by its shape alone: missing, or not a table.
    fn shape(table: &'static str, step: &'static str) -> Self {
        Self {
            table,
            step,
            error: None,
        }
    }

    fn warn(&self) {
        let error = self.error.as_ref().map(ToString::to_string);
        tracing::warn!(
            table = self.table,
            step = self.step,
            error = error.as_deref().unwrap_or("none"),
            purpose = "read per-shot survey credit",
            "A survey-credit table cannot be read; every shot falls back to its section's \
             legacy surveyor/date"
        );
    }
}

/// Log how many entries of a readable credit table were skipped, and how many
/// values read as none. Counts only: the entries hold names and emails.
fn warn_skipped(table: &'static str, skipped: usize, unreadable_values: usize) {
    if skipped > 0 || unreadable_values > 0 {
        tracing::warn!(
            table,
            skipped,
            unreadable_values,
            purpose = "read per-shot survey credit",
            "Skipped unreadable survey-credit entries"
        );
    }
}

/// Whether `name` is a table of the file. `Ok(false)` when nothing has that
/// name; an error when the schema cannot be read or the name is another kind
/// of object (a view), which no build writes.
fn is_table(conn: &Connection, name: &'static str) -> Result<bool, Unreadable> {
    let kinds: Vec<String> = conn
        .prepare("SELECT type FROM sqlite_master WHERE name = ?1")
        .and_then(|mut stmt| {
            stmt.query_map([name], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()
        })
        .map_err(|e| Unreadable::new(name, "schema", e))?;
    match kinds.as_slice() {
        [] => Ok(false),
        [kind] if kind.eq_ignore_ascii_case("table") => Ok(true),
        _ => Err(Unreadable::shape(name, "not a table")),
    }
}
