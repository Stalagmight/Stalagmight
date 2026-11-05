//! `.stalx` open path: synthetic base projects, SQL-level semantic mutations,
//! and the load oracle.
//!
//! A `.stalx` is a SQLite database. Byte-level mutation of one mostly dies in
//! SQLite's own header and page checks (C code the Rust fuzzer gets no
//! coverage from), so the main target works one level up: it opens a
//! well-formed synthetic project with SQLite and corrupts it the way a damaged,
//! hand-edited, foreign or hostile file would - wrong-typed and out-of-range
//! cells, broken JSON payloads, dangling and duplicated ids, deleted rows,
//! dropped tables and columns, an unknown or older schema version - then opens
//! it through the app's full load path (`AppState::do_load_project_full`:
//! schema checks, migration, repair, load, coordinate computation).
//!
//! Base projects are built in code through the app's own commands (import,
//! merge through placement, save), never read from a fixture.

use std::cell::RefCell;
use std::path::Path;

use rusqlite::types::Value;
use rusqlite::Connection;
use stalagmight_core::core::{AppState, Command, VirtualShotMode};
use stalagmight_models::section::{ImportPlacement, ImportSectionConfig};
use stalagmight_models::shot::ShotData;
use stalagmight_models::station::{GpsCoordinates, StationRef};

use crate::fileset::with_scratch_dir;
use crate::oracle::{check_project, fresh_project, with_state, Outcome, Violation};
use crate::source::{seed_bytes, ByteSource};

/// Number of distinct synthetic base projects.
pub const BASE_VARIANTS: u64 = 4;

fn violation(msg: impl Into<String>) -> Violation {
    Violation(msg.into())
}

fn shot(src: &mut ByteSource<'_>) -> ShotData {
    let opt = |src: &mut ByteSource<'_>| src.bool().then(|| src.f64_in(0.0, 5.0));
    ShotData {
        azimuth_in: src.f64_in(0.0, 359.9),
        azimuth_out: src.f64_in(0.0, 359.9),
        length: src.f64_in(0.1, 30.0),
        depth_in: src.f64_in(0.0, 60.0),
        depth_out: src.f64_in(0.0, 60.0),
        left: opt(src),
        right: opt(src),
        up: opt(src),
        down: opt(src),
        comment: src.chance(64).then(|| src.text(12)),
        ..ShotData::default()
    }
}

fn section(
    src: &mut ByteSource<'_>,
    index: u32,
    placement: ImportPlacement,
) -> ImportSectionConfig {
    let shots = (0..=src.below(12)).map(|_| shot(src)).collect();
    ImportSectionConfig {
        section_index: index,
        name: Some(format!("Section{index}")),
        surveyor: src.bool().then(|| "SurveyorA".to_string()),
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots,
        placement,
        station_comments: if src.bool() {
            vec![(1, "StationA1 note".to_string())]
        } else {
            Vec::new()
        },
        reverse: src.chance(32),
        azimuth_reference: None,
    }
}

/// Build base project `variant` (deterministic) and return its file bytes.
fn build_base(state: &AppState, variant: u64, dir: &Path) -> Result<Vec<u8>, Violation> {
    let bytes = seed_bytes(0xBA5E_0000 + variant, 4096);
    let mut src = ByteSource::new(&bytes);
    let path = dir.join(format!("base{variant}.stalx"));
    fresh_project(state, &format!("Base{variant}"), &path)?;
    let first_placement = if variant.is_multiple_of(2) {
        ImportPlacement::ZeroDatum {
            coordinates: GpsCoordinates {
                latitude: 20.5,
                longitude: -87.3,
                depth_reference: 0.0,
            },
            station_index: 0,
        }
    } else {
        ImportPlacement::Origin
    };
    let first: Vec<ImportSectionConfig> = (0..=src.below(3) as u32)
        .map(|i| {
            section(
                &mut src,
                i,
                if i == 0 {
                    first_placement.clone()
                } else {
                    ImportPlacement::Origin
                },
            )
        })
        .collect();
    let imported = state.execute_import_sections(
        first,
        Vec::new(),
        Some("hash-a".to_string()),
        Vec::new(),
        Vec::new(),
        VirtualShotMode::default(),
    );
    if !imported.success {
        return Err(violation(format!(
            "harness: base import: {:?}",
            imported.error
        )));
    }
    // A second import tied onto the first section's entrance: a merge, a
    // second undo entry, a second file hash.
    let anchor = state
        .get_project_state()
        .and_then(|p| p.sections.first().map(|s| s.id))
        .ok_or_else(|| violation("harness: base project has no section"))?;
    let second = vec![section(
        &mut src,
        0,
        ImportPlacement::FromExisting {
            import_station: 0,
            existing_station: StationRef::new(anchor, 0),
        },
    )];
    let imported = state.execute_import_sections(
        second,
        Vec::new(),
        Some("hash-b".to_string()),
        Vec::new(),
        Vec::new(),
        VirtualShotMode::default(),
    );
    if !imported.success {
        return Err(violation(format!(
            "harness: base second import: {:?}",
            imported.error
        )));
    }
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    if !saved.success {
        return Err(violation(format!("harness: base save: {:?}", saved.error)));
    }
    std::fs::read(&path).map_err(|e| violation(format!("harness: read base: {e}")))
}

thread_local! {
    static BASES: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

/// The bytes of base project `variant % BASE_VARIANTS` (built once per thread).
pub fn base_bytes(variant: u64) -> Result<Vec<u8>, Violation> {
    let index = (variant % BASE_VARIANTS) as usize;
    let cached = BASES.with(|b| b.borrow().get(index).cloned());
    if let Some(bytes) = cached {
        return Ok(bytes);
    }
    let dir = tempfile::tempdir().map_err(|e| violation(format!("harness: tempdir: {e}")))?;
    let built: Vec<Vec<u8>> = with_state(|state| {
        (0..BASE_VARIANTS)
            .map(|v| build_base(state, v, dir.path()))
            .collect()
    })?;
    let bytes = built
        .get(index)
        .cloned()
        .ok_or_else(|| violation("harness: base index"))?;
    BASES.with(|b| *b.borrow_mut() = built);
    Ok(bytes)
}

/// A cell value chosen to break assumptions: wrong type, out of range,
/// non-finite, empty, huge, or broken JSON.
fn hostile_value(src: &mut ByteSource<'_>, current: &Value) -> Value {
    match src.below(14) {
        0 => Value::Null,
        1 => Value::Integer(src.pick_or(
            &[
                0,
                -1,
                1,
                i64::MAX,
                i64::MIN,
                i64::from(i32::MAX) + 1,
                4_294_967_296,
            ],
            0,
        )),
        2 => Value::Real(src.pick_or(
            &[
                f64::INFINITY,
                f64::NEG_INFINITY,
                -0.0,
                f64::MAX / 4.0,
                -f64::MAX / 4.0,
                f64::MIN_POSITIVE,
                -1.0,
                1e-300,
            ],
            0.0,
        )),
        3 => Value::Text(String::new()),
        4 => Value::Text(src.text(40)),
        5 => Value::Text("x".repeat(1 + src.below(1 << 16))),
        6 => Value::Blob(src.bytes(64).to_vec()),
        7 => Value::Text(
            src.pick_or(
                &[
                    "{}",
                    "[]",
                    "null",
                    "{\"a\":",
                    "[1,2",
                    "\"\"",
                    "1e999",
                    "-1",
                    "NaN",
                    "{\"type\":\"unknown\"}",
                ],
                "{}",
            )
            .to_string(),
        ),
        8 | 9 => mutate_json_or_number(src, current),
        10 => match current {
            Value::Integer(i) => Value::Integer(i.wrapping_add(src.range(-3, 3))),
            Value::Real(r) => Value::Real(-r),
            Value::Text(t) => Value::Text(t.chars().take(src.below(t.len() + 1)).collect()),
            Value::Blob(b) => Value::Blob(b.iter().take(src.below(b.len() + 1)).copied().collect()),
            Value::Null => Value::Integer(0),
        },
        11 => match current {
            // Same value, other storage class.
            Value::Integer(i) => Value::Text(i.to_string()),
            Value::Real(r) => Value::Text(r.to_string()),
            Value::Text(t) => Value::Blob(t.as_bytes().to_vec()),
            Value::Blob(b) => Value::Text(String::from_utf8_lossy(b).into_owned()),
            Value::Null => Value::Text("NULL".to_string()),
        },
        12 => match current {
            Value::Blob(b) => {
                let mut b = b.clone();
                for _ in 0..=src.below(4) {
                    let i = src.below(b.len());
                    if let Some(x) = b.get_mut(i) {
                        *x ^= 1 << src.below(8);
                    }
                }
                Value::Blob(b)
            }
            other => mutate_json_or_number(src, other),
        },
        _ => Value::Real(src.number(-1e6, 1e6)),
    }
}

/// A JSON text cell with one value inside it changed (numbers made huge,
/// negative or strings; strings made numbers; keys dropped), or a numeric
/// cell nudged to an edge.
fn mutate_json_or_number(src: &mut ByteSource<'_>, current: &Value) -> Value {
    match current {
        Value::Text(t) => match serde_json::from_str::<serde_json::Value>(t) {
            Ok(mut json) => {
                mutate_json(src, &mut json, 0);
                Value::Text(json.to_string())
            }
            Err(_) => Value::Text(format!("{t}{}", src.text(4))),
        },
        Value::Integer(i) => {
            Value::Integer(src.pick_or(&[-*i, i.saturating_mul(1_000_000), i64::MAX, -1], 0))
        }
        Value::Real(r) => {
            Value::Real(src.pick_or(&[-*r, r * 1e12, f64::INFINITY, r / 0.3048, r * 0.3048], 0.0))
        }
        _ => Value::Null,
    }
}

fn mutate_json(src: &mut ByteSource<'_>, json: &mut serde_json::Value, depth: usize) {
    use serde_json::Value as J;
    match json {
        J::Object(map) if !map.is_empty() && depth < 8 => {
            let keys: Vec<String> = map.keys().cloned().collect();
            let key = keys.get(src.below(keys.len())).cloned().unwrap_or_default();
            if src.chance(24) {
                map.remove(&key);
            } else if let Some(v) = map.get_mut(&key) {
                mutate_json(src, v, depth + 1);
            }
        }
        J::Array(items) if !items.is_empty() && depth < 8 => {
            let i = src.below(items.len());
            match src.below(8) {
                0 => {
                    items.remove(i);
                }
                1 => {
                    let dup = items.get(i).cloned().unwrap_or(J::Null);
                    items.insert(i, dup);
                }
                _ => {
                    if let Some(v) = items.get_mut(i) {
                        mutate_json(src, v, depth + 1);
                    }
                }
            }
        }
        J::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            *json = match src.below(6) {
                0 => J::from(-f),
                1 => J::from(f * 1e15),
                2 => J::from(u64::MAX),
                3 => J::from(i64::MIN),
                4 => J::String(f.to_string()),
                _ => J::from(f + 0.5),
            };
        }
        J::String(s) => {
            *json = match src.below(4) {
                0 => J::from(1),
                1 => J::String(String::new()),
                2 => J::Null,
                _ => J::String(format!("{s}{}", src.text(6))),
            };
        }
        J::Bool(b) => *json = J::from(i32::from(!*b)),
        _ => *json = J::Null,
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn tables(conn: &Connection) -> Vec<String> {
    conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0)).and_then(Iterator::collect))
        .unwrap_or_default()
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    conn.prepare(&format!("PRAGMA table_info({})", quote_ident(table)))
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(1))
                .and_then(Iterator::collect)
        })
        .unwrap_or_default()
}

fn rowids(conn: &Connection, table: &str) -> Vec<i64> {
    conn.prepare(&format!(
        "SELECT rowid FROM {} LIMIT 256",
        quote_ident(table)
    ))
    .and_then(|mut s| {
        s.query_map([], |r| r.get::<_, i64>(0))
            .and_then(Iterator::collect)
    })
    .unwrap_or_default()
}

/// Apply one mutation; a statement SQLite refuses is simply skipped.
fn mutate_once(src: &mut ByteSource<'_>, conn: &Connection) {
    let names = tables(conn);
    let Some(table) = names.get(src.below(names.len())).cloned() else {
        return;
    };
    let t = quote_ident(&table);
    let cols = columns(conn, &table);
    let rows = rowids(conn, &table);
    let row = rows.get(src.below(rows.len())).copied().unwrap_or(1);
    let col = cols.get(src.below(cols.len())).cloned().unwrap_or_default();
    let c = quote_ident(&col);
    // Errors (constraint, type, missing table) are expected and ignored: the
    // mutation then does nothing and the next one gets its turn.
    let _ = match src.below(16) {
        0..=6 => {
            let current: Value = conn
                .query_row(
                    &format!("SELECT {c} FROM {t} WHERE rowid = ?1"),
                    [row],
                    |r| r.get(0),
                )
                .unwrap_or(Value::Null);
            let value = hostile_value(src, &current);
            conn.execute(
                &format!("UPDATE {t} SET {c} = ?1 WHERE rowid = ?2"),
                rusqlite::params![value, row],
            )
        }
        7 => conn.execute(&format!("DELETE FROM {t} WHERE rowid = ?1"), [row]),
        8 => conn.execute(
            &format!("INSERT INTO {t} SELECT * FROM {t} WHERE rowid = ?1"),
            [row],
        ),
        9 => {
            // Point a reference at another row's id, or at nothing.
            let other = rows.get(src.below(rows.len())).copied().unwrap_or(0) + src.range(-1, 1);
            conn.execute(
                &format!("UPDATE {t} SET {c} = ?1 WHERE rowid = ?2"),
                [other, row],
            )
        }
        10 => conn.execute(&format!("UPDATE {t} SET {c} = NULL"), []),
        11 if src.chance(64) => conn.execute(&format!("DELETE FROM {t}"), []),
        12 if src.chance(32) => conn.execute(&format!("DROP TABLE {t}"), []),
        13 if src.chance(64) => conn.execute(&format!("ALTER TABLE {t} DROP COLUMN {c}"), []),
        14 => {
            let v = src.pick_or(
                &[0, 1, -1, 2, 5, 10, 11, 12, 13, 14, 99, i64::from(i32::MAX)],
                0,
            );
            conn.execute_batch(&format!("PRAGMA user_version = {v}"))
                .map(|()| 0)
        }
        _ => conn.execute(
            &format!(
                "ALTER TABLE {t} ADD COLUMN {} TEXT DEFAULT 'x'",
                quote_ident(&format!("fz{}", src.u8()))
            ),
            [],
        ),
    };
}

/// Mutate the database at `path` per `src` (up to 24 mutations).
pub fn mutate_file(src: &mut ByteSource<'_>, path: &Path) -> Result<(), Violation> {
    let conn = Connection::open(path).map_err(|e| violation(format!("harness: open base: {e}")))?;
    let _ = conn.execute_batch("PRAGMA foreign_keys = OFF;");
    let n = 1 + src.below(24);
    for _ in 0..n {
        if src.is_empty() {
            break;
        }
        mutate_once(src, &conn);
    }
    conn.close()
        .map_err(|(_, e)| violation(format!("harness: close mutated base: {e}")))
}

/// Open `path` through the app's load path and check what it produced.
pub fn load_and_check(path: &Path) -> Result<Outcome, Violation> {
    with_state(|state| {
        let mut outcome = Outcome::default();
        // The previous input's project is still open (and a load refuses the
        // path that is already open): close it the way File > Close does.
        let _ = state.execute(Command::CloseProject);
        let loaded = match state.do_load_project_full(path.to_path_buf()) {
            Ok(loaded) => loaded,
            Err(e) => {
                outcome.rejected = Some(format!("{e:?}"));
                return Ok(outcome);
            }
        };
        outcome.parsed = true;
        outcome.sections = loaded.project.sections.len();
        outcome.shots = loaded.project.sections.iter().map(|s| s.shots.len()).sum();
        check_project(&loaded.project)?;
        for station in &loaded.stations {
            let c = &station.coordinates;
            if !(c.x.is_finite() && c.y.is_finite() && c.z.is_finite()) {
                return Err(violation(format!(
                    "loaded project has a non-finite station position at {:?}",
                    station.station_ref
                )));
            }
        }
        Ok(outcome)
    })
}

/// Fuzz entry: a base project mutated at the SQL level, then opened.
pub fn run_sql(data: &[u8]) -> Result<Outcome, Violation> {
    let mut src = ByteSource::new(data);
    let base = base_bytes(u64::from(src.u8()))?;
    with_scratch_dir(|dir| {
        let path = dir.join("project.stalx");
        std::fs::write(&path, &base).map_err(|e| violation(format!("harness: write base: {e}")))?;
        mutate_file(&mut src, &path)?;
        load_and_check(&path)
    })
    .map_err(|e| violation(format!("harness: scratch: {e:#}")))?
}

/// Fuzz entry: the input is the whole `.stalx` file.
pub fn run_raw(data: &[u8]) -> Result<Outcome, Violation> {
    with_scratch_dir(|dir| {
        let path = dir.join("project.stalx");
        std::fs::write(&path, data).map_err(|e| violation(format!("harness: write: {e}")))?;
        load_and_check(&path)
    })
    .map_err(|e| violation(format!("harness: scratch: {e:#}")))?
}
