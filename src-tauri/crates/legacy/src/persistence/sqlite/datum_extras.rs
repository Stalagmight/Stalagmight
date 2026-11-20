//! `zero_datum_extras`: the GPS fixes and accuracies of the zero datums (#741).
//!
//! A merge-connected survey can hold several zero datums. Their roles are
//! derived from list order (`Project::datum_roles`): the first resolvable datum
//! of a component PLACES it, every later one is a GPS FIX. The v18
//! `zero_datums` table cannot carry that, and must not change: SpeleoDB's
//! Python reader (see `station_texts`) pins the schema version (exactly 18
//! before its 0.0.2) and reads
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
//!   - `line_id`, `station_index`: its station;
//!   - `shot_uuid`: the uuid of the shot ending at that station, NULL for
//!     station 0 (see the checks below);
//!   - `start_shot_uuid`: for station 0 only, the uuid of the shot STARTING
//!     there (the line's first shot); NULL for any other station, and for
//!     a line with no shots;
//!   - `in_zero_datums`: 1 when its GPS is the `zero_datums` row at that
//!     station (a placement datum), 0 for a GPS fix;
//!   - `latitude`, `longitude`, `depth_reference`: a fix's GPS, NULL for a
//!     placement datum;
//!   - `accuracy_m`: the stored accuracy in metres, NULL for the default;
//!   - `entrance`: the user's answer to "is this station an entrance?", 1 or
//!     0, NULL for the default rule (`Project::datum_is_entrance`).
//!
//! Rows are written only when they add something: a project with no fix, no
//! stored accuracy and no entrance answer writes an empty table and reads
//! back from `zero_datums` alone, exactly as every file written before this
//! table did.
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
//!   station, and a station 0 row's `start_shot_uuid` the uuid of the
//!   line's first shot. A fix whose station moved is skipped; a placement
//!   row that moved fails the first check, so the table is ignored. A station
//!   0 row with no `start_shot_uuid` (the column was added after the table, so
//!   a table without it, or a NULL, is a row written before it) is checked by
//!   its line alone, as it was then. A table without `entrance` (also
//!   added after the table) reads every row as having no answer.
//!
//! Nothing the table holds fails the load, as with the credit tables (#717): a
//! table of another shape (a column missing, more rows than the file has
//! stations) is ignored with a warning; an unreadable or invalid row is skipped
//! and counted, never named; an accuracy that is unreadable, or outside the
//! range the app accepts (`validate_gps_accuracy`: 0 to `MAX_GPS_ACCURACY_M`),
//! reads as the default, and so does an entrance answer other than 0 or 1.
//! SQLite failing for a reason of its own (busy, I/O, corrupt) still fails the
//! open, so a save cannot then drop the rows for good.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::models::{
    validate_gps_accuracy, DatumRole, GpsCoordinates, Line, Project, StationKey, ZeroDatum,
};

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
        accuracy_m REAL,
        start_shot_uuid TEXT,
        entrance INTEGER
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
            station.line_id,
            station.station_index,
            datum.gps.latitude,
            datum.gps.longitude,
            datum.gps.depth_reference
        ])?;
    }

    if !holds_datum_extras(project) {
        return Ok(());
    }
    let mut uuid_of =
        tx.prepare("SELECT uuid FROM shots WHERE section_id = ?1 AND sequence_order = ?2")?;
    let mut insert = tx.prepare(
        "INSERT INTO zero_datum_extras (position, section_id, station_index, shot_uuid, in_zero_datums, latitude, longitude, depth_reference, accuracy_m, start_shot_uuid, entrance) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for (position, (station, datum, is_fix)) in saved.iter().enumerate() {
        // Station 0 ends no shot, so it is anchored by the shot it starts.
        let (shot_uuid, start_shot_uuid): (Option<String>, Option<String>) =
            match station.station_index.checked_sub(1) {
                None => (
                    None,
                    uuid_of
                        .query_row(params![station.line_id, 0], |row| row.get(0))
                        .optional()?
                        .flatten(),
                ),
                Some(seq) => (
                    uuid_of.query_row(params![station.line_id, seq], |row| row.get(0))?,
                    None,
                ),
            };
        let gps = is_fix.then_some(&datum.gps);
        insert.execute(params![
            i64::try_from(position)?,
            station.line_id,
            station.station_index,
            shot_uuid,
            i64::from(!is_fix),
            gps.map(|g| g.latitude),
            gps.map(|g| g.longitude),
            gps.map(|g| g.depth_reference),
            datum.accuracy_m,
            start_shot_uuid,
            datum.entrance.map(i64::from),
        ])?;
    }
    Ok(())
}

/// Whether saving `project` writes `zero_datum_extras` rows: it holds a GPS
/// fix, or a datum with a stored accuracy or entrance answer (dangling datums are not saved, so
/// they do not count). That is exactly what an older build drops when it
/// saves the file, which is why the save reports it (#741).
pub fn holds_datum_extras(project: &Project) -> bool {
    project
        .zero_datums
        .iter()
        .zip(project.datum_roles())
        .any(|(datum, role)| match role {
            Some(DatumRole::GpsFix) => true,
            Some(DatumRole::Placement) => datum.accuracy_m.is_some() || datum.entrance.is_some(),
            None => false,
        })
}

/// The project's zero datums: `base` (the `zero_datums` rows, in table order,
/// as positional keys) combined with `zero_datum_extras` as the module doc
/// describes. `lines` are the loaded lines, for the station checks.
pub(super) fn combine(
    conn: &Connection,
    lines: &[Line],
    base: Vec<ZeroDatum>,
) -> Result<Vec<ZeroDatum>> {
    let rows = match read_rows(conn, lines) {
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
    let lines: HashMap<u32, &Line> = lines.iter().map(|s| (s.id, s)).collect();

    // What each row becomes: a `base` datum (by index) or a fix of its own.
    enum Entry {
        Placement(usize, Option<f64>, Option<bool>),
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
        if !anchor_holds(&lines, &row) {
            counts.stale += 1;
            continue;
        }
        let key = StationKey::positional(row.line_id, row.station_index);
        let accuracy_m = row.accuracy_m.unwrap_or_else(|()| {
            counts.unreadable_values += 1;
            None
        });
        let entrance = row.entrance.unwrap_or_else(|()| {
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
                entries.push(Entry::Placement(i, accuracy_m, entrance));
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
                    entrance,
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
            Entry::Placement(i, accuracy_m, entrance) => base
                .get_mut(i)
                .and_then(Option::take)
                .map(|datum| ZeroDatum {
                    accuracy_m,
                    entrance,
                    ..datum
                }),
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
    line_id: u32,
    station_index: u32,
    shot_uuid: Option<String>,
    /// Station 0 only: the uuid of the shot starting there, when recorded.
    start_shot_uuid: Option<String>,
    /// `None` for a placement row, else a fix's valid GPS.
    gps: Option<GpsCoordinates>,
    /// `Err` when the stored accuracy is unreadable or one the app would refuse
    /// to store (`validate_gps_accuracy`).
    accuracy_m: Result<Option<f64>, ()>,
    /// `Err` when the stored entrance answer is neither NULL, 0 nor 1.
    entrance: Result<Option<bool>, ()>,
}

fn read_rows(conn: &Connection, lines: &[Line]) -> rusqlite::Result<Read> {
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
    let stations: usize = lines.iter().map(|s| s.shots.len() + 1).sum();
    if usize::try_from(rows).map_or(true, |rows| rows > stations) {
        return Ok(Read::TooMany);
    }
    // A table written before `start_shot_uuid` existed has no such column;
    // its rows read as having none.
    let has_start: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('zero_datum_extras') WHERE name = 'start_shot_uuid'",
        [],
        |row| row.get(0),
    )?;
    // Nor does one written before `entrance` existed: its rows read as having
    // no answer (the default rule).
    let has_entrance: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('zero_datum_extras') WHERE name = 'entrance'",
        [],
        |row| row.get(0),
    )?;
    let column = |present: i64, name: &'static str| if present > 0 { name } else { "NULL" };
    let (start, entrance) = (
        column(has_start, "start_shot_uuid"),
        column(has_entrance, "entrance"),
    );
    let mut stmt = conn.prepare(&format!(
        "SELECT section_id, station_index, shot_uuid, in_zero_datums, latitude, longitude, depth_reference, accuracy_m, {start}, {entrance} FROM zero_datum_extras ORDER BY position",
    ))?;
    let iter = stmt.query_map([], |row| {
        Ok(decode(
            row.get(0),
            row.get(1),
            [row.get(2), row.get(8)],
            row.get(3),
            [row.get(4), row.get(5), row.get(6)],
            row.get(7),
            row.get(9),
        ))
    })?;
    Ok(Read::Rows(iter.collect::<rusqlite::Result<_>>()?))
}

/// A row as this build writes it, or `None`.
fn decode(
    line_id: rusqlite::Result<u32>,
    station_index: rusqlite::Result<u32>,
    [shot_uuid, start_shot_uuid]: [rusqlite::Result<Option<String>>; 2],
    in_zero_datums: rusqlite::Result<i64>,
    gps: [rusqlite::Result<Option<f64>>; 3],
    accuracy_m: rusqlite::Result<Option<f64>>,
    entrance: rusqlite::Result<Option<i64>>,
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
    // The same rule as every write path, so a crafted file cannot hold a value
    // the Edit Station dialog could never have stored.
    let accuracy_m = match accuracy_m {
        Ok(a) if validate_gps_accuracy(a).is_ok() => Ok(a),
        _ => Err(()),
    };
    let entrance = match entrance {
        Ok(None) => Ok(None),
        Ok(Some(0)) => Ok(Some(false)),
        Ok(Some(1)) => Ok(Some(true)),
        _ => Err(()),
    };
    Some(Row {
        line_id: line_id.ok()?,
        station_index: station_index.ok()?,
        shot_uuid: shot_uuid.ok()?,
        start_shot_uuid: start_shot_uuid.ok()?,
        gps,
        accuracy_m,
        entrance,
    })
}

/// Whether the station still exists and the shot ending at it is still the
/// one the row recorded. Station 0 ends no shot: it holds while the line's
/// first shot is still the one the row recorded, or always for a row that
/// recorded none (one written before `start_shot_uuid`, or a line with no
/// shots then).
fn anchor_holds(lines: &HashMap<u32, &Line>, row: &Row) -> bool {
    let Some(line) = lines.get(&row.line_id) else {
        return false;
    };
    let shot_uuid = row.shot_uuid.as_deref();
    match row.station_index.checked_sub(1) {
        None => {
            shot_uuid.is_none()
                && row.start_shot_uuid.as_deref().is_none_or(|start| {
                    line.shots.first().and_then(|s| s.uuid.as_deref()) == Some(start)
                })
        }
        Some(seq) => {
            let Some(shot) = usize::try_from(seq)
                .ok()
                .and_then(|seq| line.shots.get(seq))
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
