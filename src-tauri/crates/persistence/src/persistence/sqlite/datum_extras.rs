//! `zero_datum_extras`: the GPS fixes and accuracies of the zero datums (#741).
//!
//! A merge-connected survey can hold several zero datums. Their roles are
//! derived from list order (`Project::datum_roles`): the first resolvable datum
//! of a component PLACES it, every later one is a GPS FIX. The v18
//! `zero_datums` table cannot carry that, and must not change: SpeleoDB's
//! Python reader (see `station_texts`) accepts exactly v18 and reads
//! `zero_datums` by an explicit column list, and an older build assumes at
//! most one datum per component there. So, like `station_names` and the credit
//! tables, the extra data lives in a NEW table with no schema bump:
//!
//! - `zero_datums` keeps exactly what an older build would write: the
//!   placement datums, at most one per merge component, in list order. A datum
//!   whose station no longer resolves is dropped on save, as it always was.
//! - `zero_datum_extras` holds one row per saved datum, placement and fix
//!   alike, so the order of the WHOLE list survives:
//!   - `position` (primary key): the datum's place in the saved list, from 0;
//!   - `section_id`, `station_index`: its station;
//!   - `shot_uuid`: the uuid of the shot ending at that station, NULL for
//!     station 0 (see the checks below);
//!   - `in_zero_datums`: 1 when its GPS is the `zero_datums` row at that
//!     station (a placement datum), 0 for a GPS fix;
//!   - `latitude`, `longitude`, `depth_reference`: a fix's GPS, NULL for a
//!     placement datum;
//!   - `accuracy_m`: the stored accuracy in metres, NULL for the default.
//!
//! Rows are written only when they add something: a project with no fix and
//! no stored accuracy writes an empty table and reads back from `zero_datums`
//! alone, exactly as every file written before this table did.
//!
//! An older build drops the table on its next normal save (every save writes a
//! fresh database from `init_schema` and renames it over the file), so it loses
//! the fixes and accuracies and keeps its placements: the same price as the
//! other tables added since v18. But a save into the LIVE connection (the
//! SpeleoDB rename path), a repair or another tool can rewrite `zero_datums`
//! and the shots in place and leave this table beside them. So the table is
//! trusted only while it still describes the file:
//!
//! - its `in_zero_datums = 1` rows must name exactly the stations of the
//!   `zero_datums` rows. Otherwise `zero_datums` was rewritten by something
//!   else, it is the newer truth, and the whole table is ignored (with a
//!   warning): the project loads as an older build sees it;
//! - each row's `shot_uuid` must still be the uuid of the shot ending at its
//!   station. A fix whose station moved is skipped; a placement row that moved
//!   fails the first check, so the table is ignored.
//!
//! Nothing the table holds fails the load, as with the credit tables (#717): a
//! table of another shape (a column missing, more rows than the file has
//! stations) is ignored with a warning; an unreadable or invalid row is skipped
//! and counted, never named; an unreadable accuracy reads as the default.
//! SQLite failing for a reason of its own (busy, I/O, corrupt) still fails the
//! open, so a save cannot then drop the rows for good.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{params, Connection, Transaction};

use crate::models::{DatumRole, GpsCoordinates, Project, Section, StationKey, ZeroDatum};

use super::shot_credits::{extended_code, is_schema_error};

/// DDL shared by `init_schema` and `save_project`. No foreign key: every save
/// recreates the table whole, and the read path checks each row against the
/// file instead.
pub(super) const CREATE_DATUM_EXTRAS: &str = "
    CREATE TABLE IF NOT EXISTS zero_datum_extras (
        position INTEGER PRIMARY KEY,
        section_id INTEGER NOT NULL,
        station_index INTEGER NOT NULL,
        shot_uuid TEXT,
        in_zero_datums INTEGER NOT NULL,
        latitude REAL,
        longitude REAL,
        depth_reference REAL,
        accuracy_m REAL
    );
";

/// Run by `save_project` before [`CREATE_DATUM_EXTRAS`]: every save rewrites
/// the table whole, and recreating it also replaces a table of another shape.
pub(super) const RECREATE_DATUM_EXTRAS: &str = "DROP TABLE IF EXISTS zero_datum_extras;";

/// Write `zero_datums` (placement datums only) and `zero_datum_extras`. Runs
/// after the shots are written, so each row's `shot_uuid` is the uuid the file
/// holds, including one this save minted.
pub(super) fn write_zero_datums(tx: &Transaction<'_>, project: &Project) -> Result<()> {
    // (station, datum, is a fix), in list order, dangling datums left out.
    let mut saved = Vec::with_capacity(project.zero_datums.len());
    for (datum, role) in project.zero_datums.iter().zip(project.datum_roles()) {
        let (Some(role), Some(station)) = (role, project.resolve_key(&datum.station_ref)) else {
            tracing::warn!(
                purpose = "skip dangling zero datum on save",
                "Dropping a zero datum whose station no longer resolves"
            );
            continue;
        };
        saved.push((station, datum, role == DatumRole::GpsFix));
    }

    let mut insert = tx.prepare(
        "INSERT INTO zero_datums (section_id, station_index, latitude, longitude, depth_reference) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (station, datum, _) in saved.iter().filter(|(_, _, is_fix)| !is_fix) {
        insert.execute(params![
            station.section_id,
            station.station_index,
            datum.gps.latitude,
            datum.gps.longitude,
            datum.gps.depth_reference
        ])?;
    }

    let needed = saved
        .iter()
        .any(|(_, datum, is_fix)| *is_fix || datum.accuracy_m.is_some());
    if !needed {
        return Ok(());
    }
    let mut uuid_of =
        tx.prepare("SELECT uuid FROM shots WHERE section_id = ?1 AND sequence_order = ?2")?;
    let mut insert = tx.prepare(
        "INSERT INTO zero_datum_extras (position, section_id, station_index, shot_uuid, in_zero_datums, latitude, longitude, depth_reference, accuracy_m) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    for (position, (station, datum, is_fix)) in saved.iter().enumerate() {
        let shot_uuid: Option<String> = match station.station_index.checked_sub(1) {
            None => None,
            Some(seq) => uuid_of.query_row(params![station.section_id, seq], |row| row.get(0))?,
        };
        let gps = is_fix.then_some(&datum.gps);
        insert.execute(params![
            i64::try_from(position)?,
            station.section_id,
            station.station_index,
            shot_uuid,
            i64::from(!is_fix),
            gps.map(|g| g.latitude),
            gps.map(|g| g.longitude),
            gps.map(|g| g.depth_reference),
            datum.accuracy_m,
        ])?;
    }
    Ok(())
}

/// The project's zero datums: `base` (the `zero_datums` rows, in table order,
/// as positional keys) combined with `zero_datum_extras` as the module doc
/// describes. `sections` are the loaded sections, for the station checks.
pub(super) fn combine(
    conn: &Connection,
    sections: &[Section],
    base: Vec<ZeroDatum>,
) -> Result<Vec<ZeroDatum>> {
    let rows = match read_rows(conn, sections) {
        Ok(Read::Rows(rows)) => rows,
        Ok(Read::Absent) => return Ok(base),
        Ok(Read::TooMany) => {
            warn_ignored("more rows than the file has stations", None);
            return Ok(base);
        }
        Err(error) if is_schema_error(&error) => {
            warn_ignored("another shape", extended_code(&error));
            return Ok(base);
        }
        Err(error) => {
            return Err(anyhow::Error::new(error)
                .context("Failed to read the zero datum table zero_datum_extras"));
        }
    };
    if rows.is_empty() {
        return Ok(base);
    }

    let index_of: HashMap<&StationKey, usize> = base
        .iter()
        .enumerate()
        .map(|(i, datum)| (&datum.station_ref, i))
        .collect();
    let sections: HashMap<u32, &Section> = sections.iter().map(|s| (s.id, s)).collect();

    // What each row becomes: a `base` datum (by index) or a fix of its own.
    enum Entry {
        Placement(usize, Option<f64>),
        Fix(ZeroDatum),
    }
    let mut entries = Vec::with_capacity(rows.len());
    let mut placed: HashSet<usize> = HashSet::new();
    let mut seen: HashSet<StationKey> = HashSet::new();
    let mut counts = Counts::default();
    for row in rows {
        let Some(row) = row else {
            counts.skipped += 1;
            continue;
        };
        if !anchor_holds(
            &sections,
            row.section_id,
            row.station_index,
            row.shot_uuid.as_deref(),
        ) {
            counts.stale += 1;
            continue;
        }
        let key = StationKey::positional(row.section_id, row.station_index);
        let accuracy_m = row.accuracy_m.unwrap_or_else(|()| {
            counts.unreadable_values += 1;
            None
        });
        match row.gps {
            None => {
                let Some(&i) = index_of.get(&key) else {
                    warn_ignored("no longer matches zero_datums", None);
                    return Ok(base);
                };
                if !placed.insert(i) {
                    counts.skipped += 1;
                    continue;
                }
                seen.insert(key);
                entries.push(Entry::Placement(i, accuracy_m));
            }
            Some(gps) => {
                // Two datums on one station is not a shape any build writes.
                if index_of.contains_key(&key) || !seen.insert(key.clone()) {
                    counts.skipped += 1;
                    continue;
                }
                entries.push(Entry::Fix(ZeroDatum {
                    station_ref: key,
                    gps,
                    accuracy_m,
                }));
            }
        }
    }
    counts.warn();
    if placed.len() != base.len() {
        warn_ignored("no longer matches zero_datums", None);
        return Ok(base);
    }

    let mut base: Vec<Option<ZeroDatum>> = base.into_iter().map(Some).collect();
    Ok(entries
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Placement(i, accuracy_m) => {
                base.get_mut(i)
                    .and_then(Option::take)
                    .map(|datum| ZeroDatum {
                        accuracy_m,
                        ..datum
                    })
            }
            Entry::Fix(datum) => Some(datum),
        })
        .collect())
}

/// What [`read_rows`] found.
enum Read {
    /// No such table: every file written before it.
    Absent,
    /// More rows than the file has stations: no build writes that.
    TooMany,
    /// Each row in `position` order; `None` for one that cannot be read.
    Rows(Vec<Option<Row>>),
}

/// One readable row.
struct Row {
    section_id: u32,
    station_index: u32,
    shot_uuid: Option<String>,
    /// `None` for a placement row, else a fix's valid GPS.
    gps: Option<GpsCoordinates>,
    /// `Err` when the stored accuracy is unreadable, negative or not finite.
    accuracy_m: Result<Option<f64>, ()>,
}

fn read_rows(conn: &Connection, sections: &[Section]) -> rusqlite::Result<Read> {
    let tables: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'zero_datum_extras'",
        [],
        |row| row.get(0),
    )?;
    if tables == 0 {
        return Ok(Read::Absent);
    }
    let rows: i64 = conn.query_row("SELECT count(*) FROM zero_datum_extras", [], |row| {
        row.get(0)
    })?;
    let stations: usize = sections.iter().map(|s| s.shots.len() + 1).sum();
    if usize::try_from(rows).map_or(true, |rows| rows > stations) {
        return Ok(Read::TooMany);
    }
    let mut stmt = conn.prepare(
        "SELECT section_id, station_index, shot_uuid, in_zero_datums, latitude, longitude, depth_reference, accuracy_m FROM zero_datum_extras ORDER BY position",
    )?;
    let iter = stmt.query_map([], |row| {
        Ok(decode(
            row.get(0),
            row.get(1),
            row.get(2),
            row.get(3),
            [row.get(4), row.get(5), row.get(6)],
            row.get(7),
        ))
    })?;
    Ok(Read::Rows(iter.collect::<rusqlite::Result<_>>()?))
}

/// A row as this build writes it, or `None`.
fn decode(
    section_id: rusqlite::Result<u32>,
    station_index: rusqlite::Result<u32>,
    shot_uuid: rusqlite::Result<Option<String>>,
    in_zero_datums: rusqlite::Result<i64>,
    gps: [rusqlite::Result<Option<f64>>; 3],
    accuracy_m: rusqlite::Result<Option<f64>>,
) -> Option<Row> {
    let [latitude, longitude, depth] = gps;
    let gps = match in_zero_datums.ok()? {
        1 => None,
        0 => {
            let (latitude, longitude, depth) = (latitude.ok()??, longitude.ok()??, depth.ok()??);
            GpsCoordinates::validate(latitude, longitude, depth).ok()?;
            // Kept as stored (no longitude normalisation), like `zero_datums`.
            Some(GpsCoordinates {
                latitude,
                longitude,
                depth_reference: depth,
            })
        }
        _ => return None,
    };
    let accuracy_m = match accuracy_m {
        Ok(None) => Ok(None),
        Ok(Some(a)) if a.is_finite() && a >= 0.0 => Ok(Some(a)),
        _ => Err(()),
    };
    Some(Row {
        section_id: section_id.ok()?,
        station_index: station_index.ok()?,
        shot_uuid: shot_uuid.ok()?,
        gps,
        accuracy_m,
    })
}

/// Whether the station still exists and the shot ending at it is still the
/// one the row recorded (no shot for station 0).
fn anchor_holds(
    sections: &HashMap<u32, &Section>,
    section_id: u32,
    station_index: u32,
    shot_uuid: Option<&str>,
) -> bool {
    let Some(section) = sections.get(&section_id) else {
        return false;
    };
    match station_index.checked_sub(1) {
        None => shot_uuid.is_none(),
        Some(seq) => {
            let Some(shot) = usize::try_from(seq)
                .ok()
                .and_then(|seq| section.shots.get(seq))
            else {
                return false;
            };
            shot_uuid.is_some() && shot.uuid.as_deref() == shot_uuid
        }
    }
}

/// Rows left out of an otherwise usable table. Counts only.
#[derive(Default)]
struct Counts {
    skipped: usize,
    stale: usize,
    unreadable_values: usize,
}

impl Counts {
    fn warn(&self) {
        if self.skipped > 0 || self.stale > 0 || self.unreadable_values > 0 {
            tracing::warn!(
                skipped = self.skipped,
                stale = self.stale,
                unreadable_values = self.unreadable_values,
                purpose = "read zero datum fixes and accuracies",
                "Skipped unusable zero_datum_extras entries"
            );
        }
    }
}

/// The table is ignored: every datum loads from `zero_datums`, with the
/// default accuracy, as an older build reads the file. Logged with SQLite's
/// extended result code, never its message.
fn warn_ignored(why: &'static str, sqlite_code: Option<i32>) {
    tracing::warn!(
        why,
        sqlite_code = ?sqlite_code,
        purpose = "read zero datum fixes and accuracies",
        "zero_datum_extras cannot be used; zero datums load from zero_datums alone"
    );
}
