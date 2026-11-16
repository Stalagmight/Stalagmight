//! Crafted `zero_datums` / `zero_datum_extras` tables (#741), fuzzed.
//!
//! A random synthetic project with placement datums, GPS fixes and stored
//! accuracies is saved, then the two datum tables are edited at random
//! through a plain SQLite connection, as another tool or a hand edit could:
//! non-finite, huge, negative and out-of-range coordinates and accuracies
//! (`1e999` is infinity to SQLite), text and blobs where numbers belong,
//! rows on missing sections and stations, stale shot uuids, flipped
//! `in_zero_datums` flags, rows reordered so a fix comes before its
//! placement, duplicated and extra rows, a table of another shape. Then the
//! file is opened as the app opens it (`Database::open_existing`,
//! `load_project`, then the model's post-load `ensure_shot_uuids` and
//! `redrive_offsets_after_load`). Properties, each failure starting with its
//! class:
//!
//! - `panic`: nothing panics; the open and the load succeed or fail cleanly.
//! - `extras-failed-load`: an edit of `zero_datum_extras` alone never fails
//!   the load (the module doc of `datum_extras.rs`: nothing the table holds
//!   fails the load).
//! - `roundtrip`: with neither table edited, the datum list comes back
//!   exactly (stations, GPS bits, accuracies, order).
//! - `datum-list`: the datums are either the `zero_datums` rows alone, in row
//!   order with the default accuracy (the fallback), or the table's: a
//!   subsequence of its rows in `position` order, each placement row a
//!   `zero_datums` datum at its station with its accuracy or the default,
//!   each fix row its own GPS with its accuracy or the default, and every
//!   `zero_datums` datum present.
//! - `nonfinite`: raw coordinates are finite, and so is the Closures Computed
//!   View. `zero-datums-nonfinite-gps` is a non-finite computed view in a
//!   file whose `zero_datums` holds a non-finite GPS (fixed: the solve leaves
//!   such a fix out).
//! - `integrity`: `check_structural_integrity` reports nothing, except a
//!   dangling datum when a `zero_datums` row was pointed at a station its
//!   section does not have (the legacy table is trusted as the newer truth).
//! - `resave-drift`: saving the loaded project and loading that file again
//!   gives the same datums. `invalid-gps-resave` is the same failure for a
//!   datum whose GPS no write path accepts (known open, muted by default).
//!
//! `STALAGMIGHT_DATUM_FILE_FUZZ_SEEDS` (default 120) files from
//! `STALAGMIGHT_DATUM_FILE_FUZZ_FIRST_SEED` (default 0);
//! `STALAGMIGHT_DATUM_FILE_FUZZ_MUTE` names classes to mute (unset: the known
//! open ones; empty: none); `STALAGMIGHT_DATUM_FILE_FUZZ_KEEP_GOING=1` reports
//! every failing seed. A failure prints the seed and the SQL it ran.
//! Synthetic surveys only; the files live in a temporary directory.

use rusqlite::types::ValueRef;
use tempfile::TempDir;

use super::datum_extras::CREATE_DATUM_EXTRAS;
use super::*;
use crate::models::config::ClosureAdjustmentConfig;
use crate::models::{
    local_to_gps, SectionId, ShotData, StationAnchor, StationRef, MAX_GPS_ACCURACY_M,
};

/// Failure classes that are open bugs, muted unless the mute variable is set.
///
/// - `invalid-gps-resave`: a datum `zero_datums` held with a GPS no write
///   path accepts (out of range, or non-finite) that the loaded list makes a
///   fix (a reordered `zero_datum_extras`, or a later merge) is saved into
///   `zero_datum_extras`, whose reader validates, and dropped on the next
///   open. The file reader keeps `zero_datums` GPS as stored on purpose (Add
///   refuses such a source, the outside reader reports it), so this is a
///   damaged-file case. Seed 17 reaches it first
///   (`STALAGMIGHT_DATUM_FILE_FUZZ_FIRST_SEED` with `_SEEDS=1` and `_MUTE=`
///   set empty).
///
/// No longer muted: `zero-datums-nonfinite-gps` (seed 279), a non-finite
/// `zero_datums` GPS reaching the solve as a fix: the solve leaves a fix whose
/// position is not finite out.
const KNOWN_OPEN: &[&str] = &["invalid-gps-resave"];

const ACCURACIES: [Option<f64>; 6] = [
    None,
    Some(0.0),
    Some(0.5),
    Some(5.0),
    Some(50.0),
    Some(1000.0),
];

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0xD605_BBB5_8C8A_BBFD) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// A random project: sections merged into trees, a placement datum on most
/// pieces, GPS fixes on placed ones, accuracies here and there.
fn random_project(rng: &mut Rng) -> Project {
    let mut project = Project::new("DatumFileFuzz".to_string(), None);
    for _ in 0..2 + rng.below(4) {
        let mut depth = rng.range(0.0, 30.0);
        let shots = (0..=rng.below(5))
            .map(|_| {
                let length = rng.range(3.0, 20.0);
                let depth_out = (depth + rng.range(-0.5, 0.5) * length).max(0.0);
                let azimuth = rng.range(0.0, 360.0);
                let shot = ShotData {
                    azimuth_in: azimuth,
                    azimuth_out: azimuth,
                    length,
                    depth_in: depth,
                    depth_out,
                    ..Default::default()
                };
                depth = depth_out;
                shot
            })
            .collect();
        project.add_section(SectionData {
            shots,
            ..Default::default()
        });
    }
    project.ensure_shot_uuids();
    let ids: Vec<SectionId> = project.sections.iter().map(|s| s.id).collect();
    let station = |project: &Project, rng: &mut Rng, id: SectionId| {
        let n = project.get_section(id).map_or(0, |s| s.shots.len());
        StationRef::new(id, rng.below(n + 1) as u32)
    };
    for (i, &id) in ids.iter().enumerate().skip(1) {
        if rng.chance(60) {
            let other = ids[rng.below(i)];
            let (a, b) = (station(&project, rng, other), station(&project, rng, id));
            let _ = project.merge_stations(a, b);
        }
    }
    let origin = GpsCoordinates::new(
        rng.range(-60.0, 60.0),
        rng.range(-179.0, 179.0),
        rng.range(0.0, 3.0),
    )
    .unwrap();
    let mut frame: Option<GpsCoordinates> = None;
    for _ in 0..=rng.below(5) {
        let id = ids[rng.below(ids.len())];
        let at = station(&project, rng, id);
        let gps = match frame {
            None => origin,
            Some(f) => {
                let Some(c) = project.compute_station_raw_coords(at) else {
                    continue;
                };
                let (dx, dy) = (rng.range(-80.0, 80.0), rng.range(-80.0, 80.0));
                let Some((lat, lon)) = local_to_gps(c.x + dx, c.y + dy, &f) else {
                    continue;
                };
                GpsCoordinates::new(lat, lon, rng.range(0.0, 3.0)).unwrap()
            }
        };
        let accuracy = ACCURACIES[rng.below(ACCURACIES.len())];
        if project
            .set_zero_datum_with_accuracy(at, gps, accuracy)
            .is_ok()
            && frame.is_none()
        {
            frame = project.frame_datum().map(|zd| zd.gps);
        }
    }
    project
}

/// A datum as compared: station (resolved, or read off a positional key),
/// GPS bits and accuracy bits.
type Row = (Option<(u32, u32)>, [u64; 3], Option<u64>);

fn bits(g: &GpsCoordinates) -> [u64; 3] {
    [
        g.latitude.to_bits(),
        g.longitude.to_bits(),
        g.depth_reference.to_bits(),
    ]
}

fn rows_of(project: &Project) -> Vec<Row> {
    project
        .zero_datums
        .iter()
        .map(|zd| {
            let at = project
                .resolve_key(&zd.station_ref)
                .map(|sr| (sr.section_id, sr.station_index))
                .or(match zd.station_ref.anchor {
                    StationAnchor::SectionStart => Some((zd.station_ref.section_id, 0)),
                    StationAnchor::Index(i) => Some((zd.station_ref.section_id, i)),
                    StationAnchor::AfterShot(_) => None,
                });
            (at, bits(&zd.gps), zd.accuracy_m.map(f64::to_bits))
        })
        .collect()
}

/// One stored value, as SQLite holds it.
#[derive(Debug, Clone, PartialEq)]
enum Val {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob,
}

impl Val {
    fn of(v: ValueRef<'_>) -> Self {
        match v {
            ValueRef::Null => Val::Null,
            ValueRef::Integer(i) => Val::Int(i),
            ValueRef::Real(r) => Val::Real(r),
            ValueRef::Text(t) => Val::Text(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(_) => Val::Blob,
        }
    }

    fn int(&self) -> Option<i64> {
        match self {
            Val::Int(i) => Some(*i),
            _ => None,
        }
    }

    fn real(&self) -> Option<f64> {
        match self {
            Val::Int(i) => Some(*i as f64),
            Val::Real(r) => Some(*r),
            _ => None,
        }
    }
}

/// Every row of `table` (`columns`, in `order`), or `None` when the table or
/// a column is missing.
fn dump_rows(conn: &Connection, sql: &str) -> Option<Vec<Vec<Val>>> {
    let mut stmt = conn.prepare(sql).ok()?;
    let n = stmt.column_count();
    let rows = stmt
        .query_map([], |row| {
            (0..n)
                .map(|i| row.get_ref(i).map(Val::of))
                .collect::<rusqlite::Result<Vec<Val>>>()
        })
        .ok()?;
    rows.collect::<rusqlite::Result<Vec<_>>>().ok()
}

/// What the edits did, for the oracle.
#[derive(Default)]
struct Edits {
    sql: Vec<String>,
    zero_datums: bool,
    extras: bool,
    /// A `zero_datums` row was pointed at a station its section lacks.
    dangling: bool,
}

/// One to four random edits of the two tables, run on a plain connection
/// (no foreign keys): each kept only when SQLite accepts it.
fn corrupt(conn: &Connection, rng: &mut Rng, project: &Project) -> Edits {
    let mut edits = Edits::default();
    let zd_rows = dump_rows(conn, "SELECT id FROM zero_datums ORDER BY id").unwrap_or_default();
    let zd_ids: Vec<i64> = zd_rows.iter().filter_map(|r| r[0].int()).collect();
    if rng.chance(15) {
        // A file whose table is missing gets one, so its rows can be edited.
        let _ = conn.execute_batch(CREATE_DATUM_EXTRAS);
    }
    let positions: Vec<i64> = dump_rows(
        conn,
        "SELECT position FROM zero_datum_extras ORDER BY position",
    )
    .unwrap_or_default()
    .iter()
    .filter_map(|r| r[0].int())
    .collect();
    let numbers = [
        "1e999", "-1e999", "1e300", "-1e300", "-95.5", "200.0", "0.0", "-1.0", "5000.0", "'abc'",
        "'12.5'", "x'00ff'", "NULL", "3",
    ];
    let stations: Vec<StationRef> = project
        .sections
        .iter()
        .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| StationRef::new(s.id, i)))
        .collect();
    let uuid_of = |sr: StationRef| -> String {
        match sr.station_index.checked_sub(1) {
            None => "NULL".to_string(),
            Some(seq) => format!(
                "(SELECT uuid FROM shots WHERE section_id = {} AND sequence_order = {seq})",
                sr.section_id
            ),
        }
    };
    for _ in 0..=rng.below(4) {
        let mut dangles = false;
        let zd = zd_ids.get(rng.below(zd_ids.len().max(1))).copied();
        let pos = positions.get(rng.below(positions.len().max(1))).copied();
        let at = stations[rng.below(stations.len())];
        let (sql, table) = match rng.below(16) {
            0 | 1 => match zd {
                Some(id) => (
                    format!(
                        "UPDATE zero_datums SET {} = {} WHERE id = {id}",
                        rng.pick(&["latitude", "longitude", "depth_reference"]),
                        rng.pick(&numbers)
                    ),
                    1,
                ),
                None => continue,
            },
            2 => match zd {
                Some(id) => {
                    let (sec, idx) = match rng.below(4) {
                        0 => ("999".to_string(), "0".to_string()),
                        1 => (at.section_id.to_string(), "1000".to_string()),
                        2 => (at.section_id.to_string(), "-1".to_string()),
                        _ => (at.section_id.to_string(), at.station_index.to_string()),
                    };
                    dangles = idx == "1000";
                    (
                        format!(
                            "UPDATE zero_datums SET section_id = {sec}, station_index = {idx} \
                             WHERE id = {id}"
                        ),
                        1,
                    )
                }
                None => continue,
            },
            3 => match zd {
                Some(id) => (format!("DELETE FROM zero_datums WHERE id = {id}"), 1),
                None => continue,
            },
            4 => (
                format!(
                    "INSERT INTO zero_datums (section_id, station_index, latitude, longitude, \
                     depth_reference) VALUES ({}, {}, {}, {}, 0.0)",
                    at.section_id,
                    at.station_index,
                    rng.range(-60.0, 60.0),
                    rng.range(-179.0, 179.0)
                ),
                1,
            ),
            5 | 6 => match pos {
                Some(p) => (
                    format!(
                        "UPDATE zero_datum_extras SET {} = {} WHERE position = {p}",
                        rng.pick(&["accuracy_m", "latitude", "longitude", "depth_reference"]),
                        rng.pick(&numbers)
                    ),
                    2,
                ),
                None => continue,
            },
            7 => match pos {
                Some(p) => (
                    format!(
                        "UPDATE zero_datum_extras SET in_zero_datums = {} WHERE position = {p}",
                        rng.pick(&["1 - in_zero_datums", "2", "'one'", "1.0", "1.5", "NULL"])
                    ),
                    2,
                ),
                None => continue,
            },
            8 | 9 => {
                // Swap two rows' positions: a fix before its placement.
                let (Some(a), Some(b)) = (
                    pos,
                    positions.get(rng.below(positions.len().max(1))).copied(),
                ) else {
                    continue;
                };
                (
                    format!(
                        "UPDATE zero_datum_extras SET position = -1000 WHERE position = {a};
                         UPDATE zero_datum_extras SET position = {a} WHERE position = {b};
                         UPDATE zero_datum_extras SET position = {b} WHERE position = -1000;"
                    ),
                    2,
                )
            }
            10 => match pos {
                Some(p) => (
                    format!(
                        "UPDATE zero_datum_extras SET {} WHERE position = {p}",
                        rng.pick(&[
                            "section_id = 999",
                            "station_index = 1000",
                            "station_index = station_index + 1",
                            "shot_uuid = 'stale'",
                            "shot_uuid = NULL",
                            "section_id = 'abc'",
                        ])
                    ),
                    2,
                ),
                None => continue,
            },
            11 => match pos {
                // A copy of a row under a new position, before or after.
                Some(p) => (
                    format!(
                        "INSERT INTO zero_datum_extras SELECT {}, section_id, station_index, \
                         shot_uuid, in_zero_datums, latitude, longitude, depth_reference, \
                         accuracy_m FROM zero_datum_extras WHERE position = {p}",
                        if rng.chance(50) { -7 } else { 1000 }
                    ),
                    2,
                ),
                None => continue,
            },
            12 | 13 => (
                // A new fix row on a real station, valid or not.
                format!(
                    "INSERT INTO zero_datum_extras VALUES ({}, {}, {}, {}, {}, {}, {}, 0.0, {})",
                    500 + rng.below(1000),
                    at.section_id,
                    at.station_index,
                    uuid_of(at),
                    rng.pick(&["0", "1"]),
                    if rng.chance(70) {
                        format!("{}", rng.range(-60.0, 60.0))
                    } else {
                        rng.pick(&numbers).to_string()
                    },
                    rng.range(-179.0, 179.0),
                    rng.pick(&numbers)
                ),
                2,
            ),
            14 => (
                rng.pick(&[
                    "DROP TABLE zero_datum_extras",
                    "DROP TABLE zero_datum_extras; CREATE TABLE zero_datum_extras \
                     (position INTEGER PRIMARY KEY, note TEXT); \
                     INSERT INTO zero_datum_extras VALUES (0, 'x')",
                    "DELETE FROM zero_datum_extras",
                    "ALTER TABLE zero_datum_extras DROP COLUMN accuracy_m",
                ])
                .to_string(),
                2,
            ),
            _ => match pos {
                Some(p) => (
                    format!("DELETE FROM zero_datum_extras WHERE position = {p}"),
                    2,
                ),
                None => continue,
            },
        };
        // All of an edit or none of it.
        if conn
            .execute_batch(&format!("SAVEPOINT edit; {sql}; RELEASE edit;"))
            .is_ok()
        {
            edits.sql.push(sql);
            edits.dangling |= dangles;
            if table == 1 {
                edits.zero_datums = true;
            } else {
                edits.extras = true;
            }
        } else {
            let _ = conn.execute_batch("ROLLBACK TO edit; RELEASE edit;");
        }
    }
    edits
}

/// The open, the load and the post-load steps the app runs.
fn open_as_the_app(path: &Path) -> Result<Project> {
    let db = Database::open_existing(path)?;
    let mut project = db.load_project()?;
    Ok(project_after_load(&mut project))
}

fn project_after_load(project: &mut Project) -> Project {
    project.ensure_shot_uuids();
    project.redrive_offsets_after_load();
    project.clone()
}

/// What the sweep saw.
#[derive(Default)]
struct Stats {
    edited: usize,
    refused: usize,
    fallback: usize,
    table: usize,
    fixes_loaded: usize,
}

/// The datum list a load may produce from these tables (module doc).
fn check_datum_list(
    loaded: &[Row],
    base: &[Vec<Val>],
    extras: Option<&[Vec<Val>]>,
    stats: &mut Stats,
) -> Result<(), String> {
    let base_rows: Vec<Row> = base
        .iter()
        .map(|r| {
            let at = match (r[0].int(), r[1].int()) {
                (Some(s), Some(i)) => u32::try_from(s).ok().zip(u32::try_from(i).ok()),
                _ => None,
            };
            let gps = [
                r[2].real().unwrap_or(f64::NAN).to_bits(),
                r[3].real().unwrap_or(f64::NAN).to_bits(),
                r[4].real().unwrap_or(f64::NAN).to_bits(),
            ];
            (at, gps, None)
        })
        .collect();
    if loaded == base_rows.as_slice() {
        stats.fallback += 1;
        return Ok(());
    }
    stats.table += 1;
    let Some(extras) = extras else {
        return Err(format!(
            "no extras table, and the datums {loaded:?} are not zero_datums' {base_rows:?}"
        ));
    };
    // Greedy subsequence: each loaded datum matches the earliest row left.
    let matches = |d: &Row, r: &[Val]| -> bool {
        let at = match (r[0].int(), r[1].int()) {
            (Some(s), Some(i)) => u32::try_from(s).ok().zip(u32::try_from(i).ok()),
            _ => return false,
        };
        if d.0 != at {
            return false;
        }
        // A stored accuracy the app accepts is kept; anything else reads as
        // the default.
        let accuracy = r[7]
            .real()
            .filter(|a| a.is_finite() && (0.0..=MAX_GPS_ACCURACY_M).contains(a));
        let accuracy_ok = d.2 == accuracy.map(f64::to_bits);
        match r[3].int() {
            Some(1) => accuracy_ok && base_rows.iter().any(|b| b.0 == at && b.1 == d.1),
            Some(0) => {
                let gps = [r[4].real(), r[5].real(), r[6].real()];
                let valid = match gps {
                    [Some(lat), Some(lon), Some(depth)] => {
                        GpsCoordinates::validate(lat, lon, depth).is_ok()
                    }
                    _ => false,
                };
                accuracy_ok
                    && valid
                    && gps
                        .iter()
                        .zip(d.1)
                        .all(|(v, bits)| v.map(f64::to_bits) == Some(bits))
            }
            _ => false,
        }
    };
    let mut rows = extras.iter();
    for d in loaded {
        if !rows.any(|r| matches(d, r)) {
            return Err(format!(
                "datum {d:?} matches no zero_datum_extras row in order; datums {loaded:?}, \
                 zero_datums {base_rows:?}"
            ));
        }
    }
    for b in &base_rows {
        if !loaded.iter().any(|d| d.0 == b.0 && d.1 == b.1) {
            return Err(format!(
                "the zero_datums datum {b:?} is missing from {loaded:?}"
            ));
        }
    }
    Ok(())
}

/// One file: build, save, edit, open, check. `Err` carries the failure, its
/// class first.
fn run_seed(seed: u64, dir: &TempDir, mute: &[String], stats: &mut Stats) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let project = random_project(&mut rng);
    let path = dir.path().join(format!("datums-{seed}.stalx"));
    Database::open(&path)
        .and_then(|mut db| db.save_project(&project))
        .map_err(|e| format!("setup: save failed: {e}"))?;
    let original = rows_of(&project);
    let conn = Connection::open(&path).map_err(|e| format!("setup: {e}"))?;
    let edits = corrupt(&conn, &mut rng, &project);
    stats.edited += usize::from(!edits.sql.is_empty());
    let base = dump_rows(
        &conn,
        "SELECT section_id, station_index, latitude, longitude, depth_reference FROM zero_datums",
    )
    .unwrap_or_default();
    let extras = dump_rows(
        &conn,
        "SELECT section_id, station_index, shot_uuid, in_zero_datums, latitude, longitude, \
         depth_reference, accuracy_m FROM zero_datum_extras ORDER BY position",
    );
    drop(conn);
    let fail = |class: &str, detail: String| -> Result<(), String> {
        if mute.iter().any(|m| m == class) {
            return Ok(());
        }
        Err(format!("{class}: {detail} [edits: {:?}]", edits.sql))
    };

    let loaded = std::panic::catch_unwind(|| {
        let db = Database::open_existing(&path)?;
        db.load_project()
    });
    let mut loaded = match loaded {
        Err(_) => return fail("panic", "the open or the load panicked".to_string()),
        Ok(Err(e)) => {
            stats.refused += 1;
            if edits.extras && !edits.zero_datums {
                return fail("extras-failed-load", format!("{e:#}"));
            }
            return Ok(());
        }
        Ok(Ok(p)) => p,
    };
    let list = rows_of(&loaded);
    if !edits.zero_datums && !edits.extras && list != original {
        return fail("roundtrip", format!("{original:?} came back as {list:?}"));
    }
    if let Err(e) = check_datum_list(&list, &base, extras.as_deref(), stats) {
        return fail("datum-list", e);
    }
    stats.fixes_loaded += loaded
        .datum_roles()
        .iter()
        .filter(|r| **r == Some(crate::models::DatumRole::GpsFix))
        .count();
    let Ok(project) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        project_after_load(&mut loaded)
    })) else {
        return fail("panic", "the post-load steps panicked".to_string());
    };
    for s in project.compute_station_coordinates() {
        let c = s.coordinates;
        if !(c.x.is_finite() && c.y.is_finite() && c.z.is_finite()) {
            return fail("nonfinite", format!("raw {:?} at {c:?}", s.station_ref));
        }
    }
    let violations: Vec<_> = project
        .check_structural_integrity()
        .into_iter()
        .filter(|v| !(edits.dangling && v.category == "dangling_zero_datum_ref"))
        .collect();
    if !violations.is_empty() {
        return fail("integrity", format!("{violations:?}"));
    }
    let solved = std::panic::catch_unwind(|| {
        project.compute_adjusted_stations(&ClosureAdjustmentConfig::default())
    });
    let Ok(solved) = solved else {
        return fail("panic", "the computed view panicked".to_string());
    };
    if let Some(s) = solved.stations.iter().find(|s| {
        let c = s.coordinates;
        !(c.x.is_finite() && c.y.is_finite() && c.z.is_finite())
    }) {
        let bad_gps = project
            .zero_datums
            .iter()
            .any(|zd| !(zd.gps.latitude.is_finite() && zd.gps.longitude.is_finite()));
        let class = if bad_gps {
            "zero-datums-nonfinite-gps"
        } else {
            "nonfinite"
        };
        return fail(
            class,
            format!("computed {:?} at {:?}", s.station_ref, s.coordinates),
        );
    }
    // Save what loaded, open that: the same datums.
    let resaved = dir.path().join(format!("datums-{seed}-resaved.stalx"));
    if Database::open(&resaved)
        .and_then(|mut db| db.save_project(&project))
        .is_err()
    {
        return fail(
            "resave-drift",
            "the loaded project does not save".to_string(),
        );
    }
    let dangling = project
        .zero_datums
        .iter()
        .any(|zd| project.resolve_key(&zd.station_ref).is_none());
    match open_as_the_app(&resaved) {
        Ok(again) if !dangling && rows_of(&again) != rows_of(&project) => {
            // A datum `zero_datums` held with a GPS no write path accepts is
            // saved as a fix (a reorder or a merge made it one) into the table
            // whose reader validates, and dropped there.
            let now = rows_of(&again);
            let invalid_lost =
                project
                    .zero_datums
                    .iter()
                    .zip(rows_of(&project))
                    .any(|(zd, row)| {
                        !now.iter().any(|r| r.0 == row.0 && r.1 == row.1)
                            && GpsCoordinates::validate(
                                zd.gps.latitude,
                                zd.gps.longitude,
                                zd.gps.depth_reference,
                            )
                            .is_err()
                    });
            let class = if invalid_lost {
                "invalid-gps-resave"
            } else {
                "resave-drift"
            };
            fail(
                class,
                format!("{:?} reloaded as {now:?}", rows_of(&project)),
            )
        }
        Ok(_) => Ok(()),
        Err(e) => fail(
            "resave-drift",
            format!("the resaved file does not load: {e:#}"),
        ),
    }
}

#[test]
fn crafted_datum_tables_load_cleanly_or_not_at_all() {
    let seeds = env_u64("STALAGMIGHT_DATUM_FILE_FUZZ_SEEDS", 120);
    let first = env_u64("STALAGMIGHT_DATUM_FILE_FUZZ_FIRST_SEED", 0);
    let keep_going =
        std::env::var("STALAGMIGHT_DATUM_FILE_FUZZ_KEEP_GOING").is_ok_and(|v| v == "1");
    let mute: Vec<String> = match std::env::var("STALAGMIGHT_DATUM_FILE_FUZZ_MUTE") {
        Ok(list) => list
            .split(',')
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect(),
        Err(_) => KNOWN_OPEN.iter().map(|m| (*m).to_string()).collect(),
    };
    let dir = TempDir::new().unwrap();
    let mut failed: Vec<String> = Vec::new();
    let mut stats = Stats::default();
    for seed in first..first + seeds {
        if let Err(e) = run_seed(seed, &dir, &mute, &mut stats) {
            let line = format!("{e} (datum file fuzz seed {seed})");
            assert!(keep_going, "{line}");
            eprintln!("FAIL {line}");
            failed.push(line);
        }
        // One file at a time on disk.
        for name in [
            format!("datums-{seed}.stalx"),
            format!("datums-{seed}-resaved.stalx"),
        ] {
            let _ = std::fs::remove_file(dir.path().join(name));
        }
    }
    eprintln!(
        "datum file fuzz seeds {first}..{}: {} files edited, {} refused to open, {} loaded \
         from zero_datums alone, {} from zero_datum_extras, {} fixes loaded; {} failing (mute \
         list {mute:?})",
        first + seeds,
        stats.edited,
        stats.refused,
        stats.fallback,
        stats.table,
        stats.fixes_loaded,
        failed.len()
    );
    assert!(
        failed.is_empty(),
        "{} failing seeds, first: {}",
        failed.len(),
        failed[0]
    );
}
