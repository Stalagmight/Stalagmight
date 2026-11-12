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
//! as the section aggregate (the distinct surveyor names joined with `", "`,
//! and the earliest shot date), so that reader - and any older build of this
//! app - sees what it always saw.
//!
//! Rows are keyed by `(section_id, shot_uuid)`: `sections.id` is persisted,
//! and a file written before shot uuids were project-unique can hold one uuid
//! in several sections, each with its own credit.
//!
//! An older build keeps both tables (it does not know them) and keeps the shot
//! uuids, but it edits the section's surveyor and date in those legacy
//! columns. So `credit_sections (section_id, surveyor, date)` records the pair
//! THIS build wrote, and a section's rows apply only while its `sections` pair
//! still matches that record: once an older build has retyped it, the typed
//! text is the newer truth and every shot of the section falls back to it.
//!
//! A shot WITHOUT a row takes its credit from those legacy columns through
//! [`crate::models::SurveyCredit::from_legacy`]. That is the migration for every
//! file written before this feature, and for shots an older build added. The
//! writer relies on it in the other direction too: it writes a row only for a
//! shot whose credit the fallback would NOT reproduce, so an uncredited project
//! (and a section whose shots all agree with its legacy columns) costs nothing.
//! A row is explicit even when its lists are empty.
//!
//! Rows are keyed by the shot uuid `write.rs` mints or keeps. On read, an
//! unreadable row or person is skipped, a dangling person id is dropped, and a
//! row naming no shot of the project is ignored - never fatal.

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
/// section of the file are not kept). Missing table, or an unreadable row,
/// means "nothing recorded", so that section's credit rows do not apply.
pub(super) fn load_recorded_legacy(
    conn: &Connection,
    stored: &HashMap<u32, LegacyPair>,
) -> HashMap<u32, LegacyPair> {
    let mut recorded = HashMap::new();
    if !table_exists(conn, "credit_sections") {
        return recorded;
    }
    let Ok(mut stmt) = conn.prepare("SELECT section_id, surveyor, date FROM credit_sections")
    else {
        return recorded;
    };
    let Ok(iter) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, u32>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    }) else {
        return recorded;
    };
    for (section_id, surveyor, date) in iter.flatten() {
        if stored.contains_key(&section_id) {
            recorded.insert(section_id, (surveyor, date));
        }
    }
    recorded
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .is_ok_and(|c| c > 0)
}

/// Every readable `shot_credits` row, by `(section, shot uuid)`. Missing tables
/// read as no rows; unreadable rows and people are skipped, and so is a table of
/// another shape (an unreleased build of this feature keyed rows by uuid alone).
pub(super) fn load_credit_rows(conn: &Connection) -> Result<HashMap<CreditKey, SurveyCredit>> {
    let mut rows = HashMap::new();
    if !table_exists(conn, "shot_credits") {
        return Ok(rows);
    }
    let mut people: HashMap<i64, Person> = HashMap::new();
    if table_exists(conn, "credit_people") {
        let mut stmt = conn.prepare("SELECT id, name, email FROM credit_people")?;
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0),
                row.get::<_, String>(1),
                row.get::<_, Option<String>>(2),
            ))
        })?;
        for (id, name, email) in iter.flatten() {
            let (Ok(id), Ok(name)) = (id, name) else {
                continue;
            };
            let email = email.ok().flatten();
            if let Some(person) = (Person { name, email }).sanitized() {
                people.insert(id, person);
            }
        }
    }
    let resolve = |ids: &str| -> Vec<Person> {
        let list: Vec<Person> = ids
            .split(',')
            .filter_map(|token| token.trim().parse::<i64>().ok())
            .filter_map(|id| people.get(&id).cloned())
            .take(MAX_PEOPLE_PER_ROLE)
            .collect();
        dedupe_people(list)
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT section_id, shot_uuid, date, surveyors, explorers FROM shot_credits")
    else {
        tracing::warn!(
            purpose = "read per-shot survey credit",
            "shot_credits has an unexpected shape; every shot falls back to its section's \
             legacy surveyor/date"
        );
        return Ok(rows);
    };
    let iter = stmt.query_map([], |row| {
        Ok((
            row.get::<_, u32>(0),
            row.get::<_, String>(1),
            row.get::<_, Option<String>>(2),
            row.get::<_, Option<String>>(3),
            row.get::<_, Option<String>>(4),
        ))
    })?;
    for (section_id, uuid, date, surveyors, explorers) in iter.flatten() {
        let (Ok(section_id), Ok(uuid)) = (section_id, uuid) else {
            continue;
        };
        if uuid.is_empty() {
            continue;
        }
        let date = date
            .ok()
            .flatten()
            .and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());
        let surveyors = surveyors.ok().flatten().unwrap_or_default();
        let explorers = explorers.ok().flatten().unwrap_or_default();
        rows.insert(
            (section_id, uuid),
            SurveyCredit {
                surveyors: resolve(&surveyors),
                explorers: resolve(&explorers),
                date,
            },
        );
    }
    Ok(rows)
}
