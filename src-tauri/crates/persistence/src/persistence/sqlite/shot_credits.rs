//! Per-shot survey credit: `credit_people` and `shot_credits`.
//!
//! - `credit_people (id, name, email)` holds each distinct person once.
//! - `shot_credits (shot_uuid PK, date, surveyors, explorers)` holds a shot's
//!   credit, the people columns as comma-joined `credit_people` ids.
//!
//! Like `station_names`, the tables arrived WITHOUT a schema bump: SpeleoDB's
//! Python reader (see `station_texts`) accepts exactly v18, ignores tables it
//! does not know, and validates `sections.surveyor` as one string and
//! `sections.date` as one ISO date. Those two legacy columns are still written,
//! as the section aggregate (the distinct surveyor names joined with `", "`,
//! and the earliest shot date), so that reader - and any older build of this
//! app - sees what it always saw.
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
    CREATE TABLE IF NOT EXISTS shot_credits (
        shot_uuid TEXT PRIMARY KEY,
        date TEXT,
        surveyors TEXT NOT NULL DEFAULT '',
        explorers TEXT NOT NULL DEFAULT ''
    );
";

/// Write one row per `(uuid, credit)`, interning people. The caller passes
/// only the shots that need a row, each uuid once.
pub(super) fn write_credit_rows(
    tx: &Transaction<'_>,
    rows: &[(String, SurveyCredit)],
) -> Result<()> {
    let mut ids: HashMap<(String, Option<String>), i64> = HashMap::new();
    let mut insert_person =
        tx.prepare("INSERT INTO credit_people (id, name, email) VALUES (?1, ?2, ?3)")?;
    let mut insert_row = tx.prepare(
        "INSERT INTO shot_credits (shot_uuid, date, surveyors, explorers) VALUES (?1, ?2, ?3, ?4)",
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
    for (uuid, credit) in rows {
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
            uuid,
            credit.date.map(|d| d.to_string()),
            surveyors,
            explorers
        ])?;
    }
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .is_ok_and(|c| c > 0)
}

/// Every readable `shot_credits` row, by shot uuid. Missing tables read as no
/// rows; unreadable rows and people are skipped.
pub(super) fn load_credit_rows(conn: &Connection) -> Result<HashMap<String, SurveyCredit>> {
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
    let mut stmt =
        conn.prepare("SELECT shot_uuid, date, surveyors, explorers FROM shot_credits")?;
    let iter = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0),
            row.get::<_, Option<String>>(1),
            row.get::<_, Option<String>>(2),
            row.get::<_, Option<String>>(3),
        ))
    })?;
    for (uuid, date, surveyors, explorers) in iter.flatten() {
        let Ok(uuid) = uuid else {
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
            uuid,
            SurveyCredit {
                surveyors: resolve(&surveyors),
                explorers: resolve(&explorers),
                date,
            },
        );
    }
    Ok(rows)
}
