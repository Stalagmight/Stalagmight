//! [#576 FZ-PERSIST B] The old-schema fuzzer.
//!
//! Every schema this build upgrades on open - 2 through 17 - is rebuilt in code from
//! the DDL that version's `init_schema` really created, copied verbatim from git
//! history ([`ERAS`]; where a version's DDL changed during its lifetime, its first
//! and its last build are both reproduced). Each file is filled with random content
//! that version could hold, stamped the way that version stamped (`PRAGMA
//! user_version` and `project.schema_version`, both written by every build from
//! schema 2 on), then opened, upgraded, saved and reopened through the real core
//! load and save paths. Alongside them: files a NEWER build stamped (with a table
//! and a column this build has never heard of), unstamped files, pre-release files,
//! and all of those again in a folder where nothing may be written.
//!
//! The data-loss oracle is DIFFERENTIAL. The same random content is also written,
//! with only the documented migration transforms applied ([`upgraded`]), into a
//! file at this build's own schema: the file a perfect migration produces. Both
//! files are opened through the same load path and saved through the same save
//! path, and they must agree table by table and row by row after the open, in the
//! loaded project, and after the save. Everything the load path DERIVES (section
//! offsets, closure alignment, the one-shot Jump/Reference backfill, the UUIDs it
//! mints for a pre-v6 file's shots) is derived on both sides, so the oracle needs
//! no copy of any of it. A direct check of the loaded survey against the generated
//! rows backs it up.
//!
//! The documented transforms (`ensure_schema_version_with`, docs/saving.md):
//! v<4 renames the `showVirtualShots` display key; v<8 backfills
//! `resolved_warnings.shot_uuid` from the shot at the legacy position; v<10
//! backfills `speleo_link.last_edit_at` for a dirty link; v<12 negates the pitch
//! of every section whose shots vote for the +up convention (the vote rule of
//! `migrate_pitch_to_down_convention`, applied to votes the generator DESIGNED,
//! not recomputed) and clears those sections' `INCLINATION_DEPTH_MISMATCH`
//! dismissals; v<13 seeds every legacy reference shot as Jump, sets
//! `pending_kind_classification` and materialises `showJumps`; v<14 wraps every
//! azimuth into `[0, 360)`; v<15 drops `collab_state`; v<17 drops the persisted
//! undo and redo history.
//!
//! All data is synthetic, built in code. No fixture is read or written; every file
//! lives in a `TempDir`.

#![expect(
    clippy::print_stdout,
    reason = "a fuzz sweep's per-seed FAIL lines and its SUMMARY line ARE its output, and \
              `cargo test -- --nocapture` is the only channel a test has"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use rusqlite::types::{Value, ValueRef};
use rusqlite::Connection;
use tempfile::TempDir;

use super::format_property_tests::dump_database;
use super::fuzz_undo_tests::{canonical_snapshot, describe_diffs, diff_snapshots, Rng};
use super::AppState;
use crate::models::{normalize_azimuth, CommandErrorCode, Project, FEATURE_ID_STRIDE};
use crate::persistence::{compress_history_json, Database, UpgradeRefusal};
use crate::test_fixtures::state_with_null_sink;
use crate::test_helpers::isolated_config;

// ===========================================================================
// The eras
// ===========================================================================

/// One generation of `.stalx` writers: the schema version they stamped, which of
/// that version's builds, the commit(s) whose `init_schema` is reproduced, and the
/// statements it ran, in its order.
struct Era {
    version: i32,
    builds: &'static str,
    commits: &'static str,
    ddl: &'static [&'static str],
}

impl Era {
    fn label(&self) -> String {
        format!("v{} ({}, {})", self.version, self.builds, self.commits)
    }
}

/// The file name every case uses, so a backup's name is predictable.
const FILE: &str = "Cave.stalx";

/// What the load commands hand the load for a protected folder (as
/// `protected_folder_load_tests` builds it).
const OLDER_REFUSAL: &str = "Cave.stalx was saved by an older version of Stalagmight (fuzz \
                             refusal). Copy the project to a folder of your own and open the copy.";
const UNSTAMPED_REFUSAL: &str = "Cave.stalx has no file-format version stamp (fuzz refusal). \
                                 Copy the project to a folder of your own and open the copy.";

fn refusal() -> UpgradeRefusal {
    UpgradeRefusal {
        older_format: OLDER_REFUSAL.to_owned(),
        unstamped: UNSTAMPED_REFUSAL.to_owned(),
    }
}

/// The schema this build stamps on a file it creates, read back from a real file
/// rather than restated, so the fuzzer follows a bump (and the era guard below
/// fails until the bumped-from version gets its era).
fn current_version() -> Result<i32, String> {
    let dir = TempDir::new().map_err(text_of)?;
    let probe = dir.path().join("probe.stalx");
    drop(Database::open(&probe).map_err(text_of)?);
    user_version(&probe)
}

fn text_of(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// ===========================================================================
// Reading a database
// ===========================================================================

/// Every table of a database and its columns: table -> [(column, declared type)].
type Columns = BTreeMap<String, Vec<(String, String)>>;

fn columns_of(conn: &Connection) -> Result<Columns, String> {
    let tables: Vec<String> = {
        let mut statement = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .map_err(text_of)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(text_of)?;
        rows.collect::<Result<_, _>>().map_err(text_of)?
    };
    let mut out = Columns::new();
    for table in tables {
        // The name comes from `sqlite_master`, not from a caller.
        let mut statement = conn
            .prepare(&format!(
                "SELECT name, upper(type) FROM pragma_table_info('{table}')"
            ))
            .map_err(text_of)?;
        let columns = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(text_of)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(text_of)?;
        out.insert(table, columns);
    }
    Ok(out)
}

fn has(columns: &Columns, table: &str, column: &str) -> bool {
    columns
        .get(table)
        .is_some_and(|present| present.iter().any(|(name, _)| name == column))
}

fn era_columns(era: &Era) -> Result<Columns, String> {
    let conn = Connection::open_in_memory().map_err(text_of)?;
    for statement in era.ddl {
        conn.execute_batch(statement)
            .map_err(|e| format!("{}: the historical DDL does not run: {e}", era.label()))?;
    }
    columns_of(&conn)
}

fn read_only(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(text_of)
}

fn user_version(path: &Path) -> Result<i32, String> {
    read_only(path)?
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(text_of)
}

fn row_version(path: &Path) -> Result<i64, String> {
    read_only(path)?
        .query_row("SELECT schema_version FROM project LIMIT 1", [], |row| {
            row.get(0)
        })
        .map_err(text_of)
}

fn count_rows(path: &Path, table: &str) -> Result<i64, String> {
    read_only(path)?
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .map_err(text_of)
}

/// Every user table, every row, each row's cells sorted by column name so two
/// files whose tables list the same columns in a different ORDER (an `ALTER TABLE
/// ADD COLUMN` appends) still compare equal. Nothing is masked: this compares two
/// files nothing has re-saved yet, so even the timestamps must agree.
fn table_rows(path: &Path) -> Result<BTreeMap<String, Vec<String>>, String> {
    let conn = read_only(path)?;
    let mut out = BTreeMap::new();
    for table in columns_of(&conn)?.keys() {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .map_err(text_of)?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(ToString::to_string)
            .collect();
        let mut rows: Vec<String> = statement
            .query_map([], |row| {
                let mut cells: Vec<String> = Vec::with_capacity(names.len());
                for (index, name) in names.iter().enumerate() {
                    cells.push(format!("{name}={}", render(row.get_ref(index)?)));
                }
                cells.sort();
                Ok(cells.join(" | "))
            })
            .map_err(text_of)?
            .collect::<Result<_, _>>()
            .map_err(text_of)?;
        rows.sort();
        out.insert(table.clone(), rows);
    }
    Ok(out)
}

fn render(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => "null".to_owned(),
        ValueRef::Integer(v) => v.to_string(),
        ValueRef::Real(v) => format!("{v:?}"),
        ValueRef::Text(bytes) => format!("{:?}", String::from_utf8_lossy(bytes)),
        ValueRef::Blob(bytes) => format!("blob[{}]:{:016x}", bytes.len(), fnv1a(bytes)),
    }
}

/// FNV-1a, so a blob can be compared without being printed.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Where two [`table_rows`] dumps disagree, a few lines per table.
fn row_differences(
    left: &BTreeMap<String, Vec<String>>,
    right: &BTreeMap<String, Vec<String>>,
    labels: (&str, &str),
) -> Vec<String> {
    let mut out = Vec::new();
    let tables: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    for table in tables {
        let (Some(a), Some(b)) = (left.get(table), right.get(table)) else {
            out.push(format!(
                "table {table}: in {} only",
                if left.contains_key(table) {
                    labels.0
                } else {
                    labels.1
                }
            ));
            continue;
        };
        if a == b {
            continue;
        }
        let only_a: Vec<&String> = a.iter().filter(|row| !b.contains(row)).collect();
        let only_b: Vec<&String> = b.iter().filter(|row| !a.contains(row)).collect();
        out.push(format!(
            "table {table}: {} row(s) vs {}; only in {}: {:?}; only in {}: {:?}",
            a.len(),
            b.len(),
            labels.0,
            only_a.iter().take(2).collect::<Vec<_>>(),
            labels.1,
            only_b.iter().take(2).collect::<Vec<_>>()
        ));
    }
    out
}

/// The tables and columns two files carry, where they differ.
fn schema_differences(left: &Columns, right: &Columns, labels: (&str, &str)) -> Vec<String> {
    let mut out = Vec::new();
    let tables: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    for table in tables {
        let a: BTreeSet<&(String, String)> = left.get(table).into_iter().flatten().collect();
        let b: BTreeSet<&(String, String)> = right.get(table).into_iter().flatten().collect();
        if a != b {
            out.push(format!(
                "table {table}: columns only in {}: {:?}; only in {}: {:?}",
                labels.0,
                a.difference(&b).collect::<Vec<_>>(),
                labels.1,
                b.difference(&a).collect::<Vec<_>>()
            ));
        }
    }
    out
}

/// `dump_database` (timestamps masked: a save stamps them) with every shot UUID
/// replaced by the shot's position, rows re-sorted. A pre-v6 file's shots get
/// their UUIDs minted at random by the load, on both sides, so the position is
/// the only identity the two files share.
fn positional_dump(path: &Path) -> Result<String, String> {
    let dump = dump_database(path)?;
    let mut statement_uuids: Vec<(String, String)> = Vec::new();
    {
        let conn = read_only(path)?;
        let mut statement = conn
            .prepare("SELECT uuid, section_id, sequence_order FROM shots WHERE uuid IS NOT NULL")
            .map_err(text_of)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    format!("U{}:{}", row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
                ))
            })
            .map_err(text_of)?;
        for row in rows {
            statement_uuids.push(row.map_err(text_of)?);
        }
    }
    let mut out = String::new();
    let mut block: Vec<String> = Vec::new();
    let flush = |out: &mut String, block: &mut Vec<String>| {
        block.sort();
        for line in block.drain(..) {
            out.push_str(&line);
            out.push('\n');
        }
    };
    for line in dump.lines() {
        if line.starts_with("== ") {
            flush(&mut out, &mut block);
            out.push_str(line);
            out.push('\n');
        } else {
            let mut line = line.to_owned();
            for (uuid, position) in &statement_uuids {
                line = line.replace(uuid.as_str(), position);
            }
            block.push(line);
        }
    }
    flush(&mut out, &mut block);
    Ok(out)
}

fn dump_line_differences(left: &str, right: &str, labels: (&str, &str)) -> Vec<String> {
    let a: Vec<&str> = left.lines().collect();
    let b: Vec<&str> = right.lines().collect();
    let only_a: Vec<&&str> = a.iter().filter(|line| !b.contains(line)).take(3).collect();
    let only_b: Vec<&&str> = b.iter().filter(|line| !a.contains(line)).take(3).collect();
    vec![format!(
        "only in {}: {:?}; only in {}: {:?}",
        labels.0, only_a, labels.1, only_b
    )]
}

/// The loaded project's canonical snapshot, made comparable across two loads of
/// two files: shot UUIDs replaced by positions (see [`positional_dump`]), and a
/// timestamp the LOAD minted (a sole-bridge closure becomes a merge stamped
/// now) replaced by `<now>`. The generator never writes a timestamp that recent.
/// The two set-like arrays the canonical form sorts by their text are sorted
/// again after the substitution, since the UUIDs they were sorted by differ.
fn positional_snapshot(state: &AppState) -> Result<String, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "no project is open".to_owned())?;
    let mut positions: BTreeMap<String, String> = BTreeMap::new();
    for section in &project.sections {
        for (position, shot) in section.shots.iter().enumerate() {
            if let Some(uuid) = &shot.uuid {
                positions.insert(uuid.clone(), format!("U{}:{position}", section.id));
            }
        }
    }
    let floor = chrono::Utc::now() - chrono::Duration::days(1);
    let mut value: serde_json::Value =
        serde_json::from_str(&canonical_snapshot(state, false)).map_err(text_of)?;
    substitute(&mut value, &positions, floor);
    if let serde_json::Value::Object(map) = &mut value {
        for key in ["station_offsets", "station_comments"] {
            if let Some(serde_json::Value::Array(items)) = map.get_mut(key) {
                items.sort_by_key(ToString::to_string);
            }
        }
    }
    Ok(value.to_string())
}

fn substitute(
    value: &mut serde_json::Value,
    positions: &BTreeMap<String, String>,
    floor: chrono::DateTime<chrono::Utc>,
) {
    match value {
        serde_json::Value::String(text) => {
            if let Some(position) = positions.get(text.as_str()) {
                *text = position.clone();
            } else if chrono::DateTime::parse_from_rfc3339(text)
                .is_ok_and(|at| at.with_timezone(&chrono::Utc) >= floor)
            {
                *text = "<now>".to_owned();
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                substitute(item, positions, floor);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                substitute(item, positions, floor);
            }
        }
        _ => {}
    }
}

fn snapshot_differences(left: &str, right: &str) -> Option<String> {
    let diffs = diff_snapshots(left, right);
    (!diffs.is_empty()).then(|| describe_diffs(&diffs).replace('\n', " / "))
}

/// The files in `dir`, by name (a backup copy, a partial copy, a journal or a
/// save's temp file would show up here).
fn files_in(dir: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(text_of)? {
        names.push(
            entry
                .map_err(text_of)?
                .file_name()
                .to_string_lossy()
                .into_owned(),
        );
    }
    names.sort();
    Ok(names)
}

// ===========================================================================
// The content
// ===========================================================================

#[derive(Clone)]
struct ShotRow {
    azimuth_in: f64,
    azimuth_out: f64,
    length: f64,
    depth_in: f64,
    depth_out: f64,
    pitch_in: Option<f64>,
    pitch_out: Option<f64>,
    lrud: [Option<f64>; 4],
    comment: Option<String>,
    /// 0 Normal, 1 Jump, 2 Reference. Before v13 a file can only say "virtual or
    /// not", which the generator writes as 0 or 1.
    kind: u8,
    marker_index: Option<i64>,
    temperature: Option<f64>,
    uuid: Option<String>,
}

#[derive(Clone)]
struct SectionRow {
    id: i64,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<String>,
    color: String,
    direction: &'static str,
    quality: f64,
    reverse_parity: bool,
    gauge: Option<(String, String)>,
    created_at: String,
    /// The verdict the v12 pitch migration owes this section: its shots were
    /// designed to cast exactly these votes.
    flips: bool,
    shots: Vec<ShotRow>,
}

#[derive(Clone)]
struct Closure {
    a: (i64, i64),
    b: (i64, i64),
    measures: [f64; 5],
    id: i64,
    strategy: &'static str,
    is_aligned: bool,
}

#[derive(Clone)]
struct Warning {
    /// Explicit, so a row the upgrade deletes leaves the same gap on both sides.
    id: i64,
    section_id: i64,
    position: i64,
    code: &'static str,
    resolved_at: String,
    comment: Option<String>,
    shot_uuid: Option<String>,
}

#[derive(Clone)]
struct Link {
    project_id: String,
    project_name: String,
    base_commit_sha: Option<String>,
    dirty: bool,
    last_edit_at: Option<String>,
    last_verify_at: Option<String>,
    undo_mode: Option<&'static str>,
}

#[derive(Clone)]
struct HistoryRow {
    sequence: i64,
    executed_at: String,
}

/// Everything one generated `.stalx` holds, independent of how a given schema
/// spells it. A value whose column an era lacks holds the default the upgrade's
/// `ALTER TABLE ... ADD COLUMN` gives it, so the same `World` written to the
/// current schema is what the upgrade must produce.
#[derive(Clone)]
struct World {
    name: String,
    location: Option<String>,
    created_at: String,
    modified_at: String,
    next_closure_id: i64,
    sections: Vec<SectionRow>,
    merges: Vec<((i64, i64), (i64, i64), String)>,
    datums: Vec<((i64, i64), [f64; 3])>,
    section_offsets: Vec<(i64, [f64; 2])>,
    station_offsets: Vec<((i64, i64), [f64; 3])>,
    station_comments: Vec<((i64, i64), String)>,
    closures: Vec<Closure>,
    display: BTreeMap<String, String>,
    settings: BTreeMap<String, String>,
    warnings: Vec<Warning>,
    file_hashes: Vec<String>,
    section_hashes: Vec<String>,
    collab: Option<String>,
    link: Option<Link>,
    undo_chain: Option<String>,
    undo_watermark: Option<String>,
    undo: Vec<HistoryRow>,
    redo: Vec<HistoryRow>,
}

/// A persisted history entry the current build can read back, the same one
/// `newer_schema_tests` stores.
const HISTORY_COMMAND: &str = r#"{"type":"close_project"}"#;
const HISTORY_UNDO: &str = r#"{"InverseCommand":{"type":"close_project"}}"#;

const WORDS: &[&str] = &[
    "Sump",
    "Main Passage",
    "Dome",
    "O'Neil's Restriction",
    "\"Quoted\" Hall",
    "Café Siphon",
    "Mañana",
    "Über-Gang",
    "  padded  ",
    "Line;DROP TABLE shots",
    "Gallery 7",
    "Pozo Azul",
];
const COLORS: &[&str] = &["#ff0000", "#00ff00", "#123456", "#abcdef", "#000000"];
const WARNING_CODES: &[&str] = &[
    "SHORT_SHOT",
    "LONG_SHOT",
    "INCLINATION_DEPTH_MISMATCH",
    "LENGTH_BELOW_DEPTH_CHANGE",
    "AZIMUTH_DEVIATION",
    "NEGATIVE_LRUD",
    "LARGE_DEPTH_CHANGE",
];
const DISPLAY_KEYS: &[&str] = &[
    "showStationLabels",
    "showLrud",
    "labelFontSize",
    "colorBy",
    "showGrid",
];
const DISPLAY_VALUES: &[&str] = &["true", "false", "12", "depth", "section"];
const STRATEGIES: &[&str] = &[
    "ProportionalByLength",
    "EqualDistribution",
    "Batch",
    "LeastSquaresWeighted",
    "Move",
];
/// Strategy names early builds wrote that the reader still maps
/// (`ClosureStrategy::from_str_lossy`).
const LEGACY_STRATEGIES: &[&str] = &["Sequential", "KeepRaw", "ManualAdjustment"];

fn hex(rng: &mut Rng, digits: usize) -> String {
    (0..digits)
        .map(|_| char::from_digit(rng.below(16) as u32, 16).unwrap_or('0'))
        .collect()
}

fn uuid(rng: &mut Rng) -> String {
    let (a, b, c, d, e) = (
        hex(rng, 8),
        hex(rng, 4),
        hex(rng, 3),
        hex(rng, 3),
        hex(rng, 12),
    );
    format!("{a}-{b}-4{c}-a{d}-{e}")
}

fn timestamp(rng: &mut Rng) -> String {
    let date = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        rng.range(2019, 2025),
        rng.range(1, 12),
        rng.range(1, 28),
        rng.range(0, 23),
        rng.range(0, 59),
        rng.range(0, 59)
    );
    if rng.chance(50) {
        format!("{date}+00:00")
    } else {
        format!("{date}.{:09}+00:00", rng.range(0, 999_999_999))
    }
}

fn word(rng: &mut Rng) -> String {
    (*rng.pick(WORDS)).to_owned()
}

fn maybe<T>(rng: &mut Rng, percent: u32, make: impl FnOnce(&mut Rng) -> T) -> Option<T> {
    if rng.chance(percent) {
        Some(make(rng))
    } else {
        None
    }
}

/// The pitch convention a section's shots were recorded in, designed so the v12
/// migration's vote is known in advance (every sloped shot votes decisively).
#[derive(Clone, Copy)]
enum Convention {
    /// Every sloped shot +down: they all vote to keep.
    Down,
    /// Every sloped shot +up (a pre-#296 DMP import): they all vote to flip.
    Up,
    /// Each sloped shot +up with 60 %: the vote is decided by the counts.
    Mixed,
    /// No sloped shot: no vote at all.
    Flat,
}

fn generate_section(rng: &mut Rng, columns: &Columns, version: i32, id: i64) -> SectionRow {
    let conventions: &[Convention] = if version < 12 {
        &[
            Convention::Down,
            Convention::Up,
            Convention::Mixed,
            Convention::Flat,
        ]
    } else {
        &[Convention::Down, Convention::Flat]
    };
    let convention = *rng.pick(conventions);
    let shot_count = if rng.chance(5) {
        0
    } else {
        rng.range(1, 8) as usize
    };
    let wild_azimuths = version < 14 && rng.chance(40);
    let with_uuid = has(columns, "shots", "uuid");
    let (mut flip_votes, mut keep_votes) = (0_u32, 0_u32);
    let mut shots = Vec::with_capacity(shot_count);
    for _ in 0..shot_count {
        let length = rng.float(1.0, 30.0);
        let depth_in = rng.float(30.0, 80.0);
        let sloped = !matches!(convention, Convention::Flat) && rng.chance(85);
        let (depth_out, pitch_in, pitch_out) = if sloped {
            // `implied` is the angle the depth change gives; the recorded reading
            // agrees with it (+down) or with its negation (+up) to within 0.5
            // degrees, and |angle| >= 8, so the shot's vote is never close.
            let magnitude = rng.float(8.0, 70.0);
            let angle = if rng.chance(50) {
                magnitude
            } else {
                -magnitude
            };
            let up = match convention {
                Convention::Up => true,
                Convention::Mixed => rng.chance(60),
                Convention::Down | Convention::Flat => false,
            };
            if up {
                flip_votes += 1;
            } else {
                keep_votes += 1;
            }
            let recorded = if up { -angle } else { angle } + rng.float(-0.4, 0.4);
            let pitch_in = recorded + rng.float(-0.1, 0.1);
            let pitch_out = maybe(rng, 70, |rng| recorded + rng.float(-0.1, 0.1));
            (
                depth_in + length * angle.to_radians().sin(),
                Some(pitch_in),
                pitch_out,
            )
        } else {
            (depth_in, maybe(rng, 50, |rng| rng.float(-3.0, 3.0)), None)
        };
        let azimuth = |rng: &mut Rng| {
            if wild_azimuths && rng.chance(60) {
                rng.float(-720.0, 1080.0)
            } else {
                rng.float(0.0, 359.9)
            }
        };
        let azimuth_in = azimuth(rng);
        let azimuth_out = azimuth(rng);
        let kind = if version >= 13 {
            match rng.below(10) {
                0 => 1,
                1 => 2,
                _ => 0,
            }
        } else {
            u8::from(rng.chance(15))
        };
        shots.push(ShotRow {
            azimuth_in,
            azimuth_out,
            length,
            depth_in,
            depth_out,
            pitch_in,
            pitch_out,
            lrud: [
                maybe(rng, 60, |rng| rng.float(0.0, 5.0)),
                maybe(rng, 60, |rng| rng.float(0.0, 5.0)),
                maybe(rng, 60, |rng| rng.float(0.0, 5.0)),
                maybe(rng, 60, |rng| rng.float(0.0, 5.0)),
            ],
            comment: maybe(rng, 30, word),
            kind,
            marker_index: maybe(rng, 20, |rng| rng.range(0, 20)),
            temperature: maybe(rng, 30, |rng| rng.float(10.0, 30.0)),
            uuid: with_uuid.then(|| uuid(rng)),
        });
    }
    let gauge = if has(columns, "sections", "gauge_shot_uuid") && shots.len() >= 2 && rng.chance(25)
    {
        let at = rng.below(shots.len() - 1);
        match (shots.get(at), shots.get(at + 1)) {
            (Some(a), Some(b)) => a.uuid.clone().zip(b.uuid.clone()),
            _ => None,
        }
    } else {
        None
    };
    SectionRow {
        id,
        name: maybe(rng, 85, word),
        surveyor: maybe(rng, 50, word),
        date: maybe(rng, 60, |rng| {
            format!(
                "{:04}-{:02}-{:02}",
                rng.range(2019, 2026),
                rng.range(1, 12),
                rng.range(1, 28)
            )
        }),
        color: (*rng.pick(COLORS)).to_owned(),
        direction: if rng.chance(70) { "in" } else { "out" },
        quality: if rng.chance(70) {
            1.0
        } else {
            rng.float(0.5, 3.0)
        },
        reverse_parity: has(columns, "sections", "reverse_parity") && rng.chance(20),
        gauge,
        created_at: timestamp(rng),
        flips: version < 12 && flip_votes >= 3 && flip_votes > 2 * keep_votes,
        shots,
    }
}

/// A random station that exists: a section with shots, an index in `0..=shots`.
fn station(rng: &mut Rng, sections: &[SectionRow]) -> Option<(i64, i64)> {
    let populated: Vec<&SectionRow> = sections.iter().filter(|s| !s.shots.is_empty()).collect();
    if populated.is_empty() {
        return None;
    }
    let section = *rng.pick(&populated);
    Some((section.id, rng.range(0, section.shots.len() as i64)))
}

/// `count` distinct stations.
fn stations(rng: &mut Rng, sections: &[SectionRow], count: usize) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for _ in 0..count * 3 {
        if out.len() == count {
            break;
        }
        if let Some(at) = station(rng, sections) {
            if !out.contains(&at) {
                out.push(at);
            }
        }
    }
    out
}

/// Random content the era's schema can hold, in the shapes that era's builds wrote.
fn generate(rng: &mut Rng, columns: &Columns, version: i32) -> World {
    let mut ids: Vec<i64> = Vec::new();
    for _ in 0..rng.range(1, 5) {
        let id = rng.range(1, 40);
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let sections: Vec<SectionRow> = ids
        .iter()
        .map(|id| generate_section(rng, columns, version, *id))
        .collect();

    let mut merges = Vec::new();
    for _ in 0..rng.below(4) {
        let (Some(a), Some(b)) = (station(rng, &sections), station(rng, &sections)) else {
            break;
        };
        let duplicate = merges
            .iter()
            .any(|(x, y, _)| (*x, *y) == (a, b) || (*x, *y) == (b, a));
        if a.0 != b.0 && !duplicate {
            merges.push((a, b, timestamp(rng)));
        }
    }
    let count = rng.below(3);
    let datums = stations(rng, &sections, count)
        .into_iter()
        .map(|at| {
            (
                at,
                [
                    rng.float(-60.0, 60.0),
                    rng.float(-170.0, 170.0),
                    rng.float(0.0, 5.0),
                ],
            )
        })
        .collect();
    let mut section_offsets = Vec::new();
    for section in &sections {
        if rng.chance(25) {
            section_offsets.push((
                section.id,
                [rng.float(-500.0, 500.0), rng.float(-500.0, 500.0)],
            ));
        }
    }
    let count = rng.below(4);
    let station_offsets = stations(rng, &sections, count)
        .into_iter()
        .map(|at| {
            (
                at,
                [
                    rng.float(-5.0, 5.0),
                    rng.float(-5.0, 5.0),
                    rng.float(-5.0, 5.0),
                ],
            )
        })
        .collect();
    let station_comments = if columns.contains_key("station_comments") {
        let count = rng.below(4);
        stations(rng, &sections, count)
            .into_iter()
            .map(|at| (at, word(rng)))
            .collect()
    } else {
        Vec::new()
    };

    let mut closures: Vec<Closure> = Vec::new();
    for _ in 0..rng.below(3) {
        let ends = stations(rng, &sections, 2);
        let (Some(a), Some(b)) = (ends.first(), ends.get(1)) else {
            break;
        };
        let strategy = if version <= 6 && rng.chance(25) {
            *rng.pick(LEGACY_STRATEGIES)
        } else {
            *rng.pick(STRATEGIES)
        };
        let closure_id = if rng.chance(10) {
            0
        } else {
            closures.len() as i64 + 1 + rng.range(0, 3) * 10
        };
        closures.push(Closure {
            a: *a,
            b: *b,
            measures: [
                rng.float(10.0, 500.0),
                rng.float(0.0, 5.0),
                rng.float(0.0, 3.0),
                rng.float(0.0, 4.0),
                rng.float(0.0, 2.0),
            ],
            id: closure_id,
            strategy,
            is_aligned: has(columns, "loop_closures", "is_aligned") && rng.chance(20),
        });
    }
    let next_closure_id = closures.iter().map(|c| c.id).max().unwrap_or(0) + 1 + rng.range(0, 3);

    let mut display = BTreeMap::new();
    for _ in 0..rng.below(4) {
        display.insert(
            (*rng.pick(DISPLAY_KEYS)).to_owned(),
            (*rng.pick(DISPLAY_VALUES)).to_owned(),
        );
    }
    let toggle = if version < 4 {
        "showVirtualShots"
    } else {
        "showReferenceShots"
    };
    if rng.chance(50) {
        display.insert(
            toggle.to_owned(),
            (*rng.pick(&["true", "false"])).to_owned(),
        );
    }
    if version >= 13 && rng.chance(50) {
        display.insert(
            "showJumps".to_owned(),
            (*rng.pick(&["true", "false"])).to_owned(),
        );
    }

    let mut settings = BTreeMap::new();
    if columns.contains_key("project_settings") {
        if rng.chance(50) {
            let mut json = format!(
                "{{\"warn_shot_length_min\":{},\"warn_shot_length_max\":{},\
                 \"warn_depth_change_per_shot_max\":{},\"warn_azimuth_deviation_max\":{}",
                rng.float(0.1, 1.0),
                rng.float(20.0, 80.0),
                rng.float(2.0, 20.0),
                rng.float(5.0, 60.0)
            );
            for key in [
                "azimuth_diff_threshold",
                "pitch_diff_threshold",
                "inclination_depth_diff_threshold",
            ] {
                if rng.chance(50) {
                    let _ = write!(json, ",\"{key}\":{}", rng.float(1.0, 30.0));
                }
            }
            json.push('}');
            settings.insert("validation".to_owned(), json);
        }
        // #431 stored the entrance elevation from f25ba7d24, an ancestor of the
        // first v17 build and of no earlier era reproduced here.
        if version >= 17 && rng.chance(30) {
            settings.insert(
                "surface_elevation".to_owned(),
                format!("{}", rng.float(-50.0, 500.0)),
            );
        }
    }

    let mut warnings: Vec<Warning> = Vec::new();
    if columns.contains_key("resolved_warnings") {
        for _ in 0..rng.below(5) {
            let populated: Vec<&SectionRow> =
                sections.iter().filter(|s| !s.shots.is_empty()).collect();
            if populated.is_empty() {
                break;
            }
            let section = *rng.pick(&populated);
            let position = rng.below(section.shots.len());
            // The pitch flip clears exactly this code, so favour it.
            let code = if rng.chance(30) {
                "INCLINATION_DEPTH_MISMATCH"
            } else {
                *rng.pick(WARNING_CODES)
            };
            if warnings
                .iter()
                .any(|w| (w.section_id, w.position, w.code) == (section.id, position as i64, code))
            {
                continue;
            }
            let shot_uuid = if has(columns, "resolved_warnings", "shot_uuid") && rng.chance(90) {
                section.shots.get(position).and_then(|s| s.uuid.clone())
            } else {
                None
            };
            warnings.push(Warning {
                id: warnings.len() as i64 + 1,
                section_id: section.id,
                position: position as i64,
                code,
                resolved_at: timestamp(rng),
                comment: maybe(rng, 50, word),
                shot_uuid,
            });
        }
    }

    let hashes = |rng: &mut Rng| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for _ in 0..rng.below(3) {
            let hash = hex(rng, 16);
            if !out.contains(&hash) {
                out.push(hash);
            }
        }
        out
    };
    let file_hashes = hashes(rng);
    let section_hashes = if columns.contains_key("imported_section_hashes") {
        hashes(rng)
    } else {
        Vec::new()
    };
    let collab = if columns.contains_key("collab_state") && rng.chance(50) {
        Some(format!(
            "{{\"tml_path\":\"/surveys/{}.tml\",\"autosave\":{}}}",
            hex(rng, 6),
            rng.chance(50)
        ))
    } else {
        None
    };
    let link = if columns.contains_key("speleo_link") && rng.chance(35) {
        let dirty = rng.chance(50);
        Some(Link {
            project_id: uuid(rng),
            project_name: word(rng),
            base_commit_sha: maybe(rng, 60, |rng| hex(rng, 40)),
            dirty,
            last_edit_at: if has(columns, "speleo_link", "last_edit_at") && dirty && rng.chance(70)
            {
                Some(timestamp(rng))
            } else {
                None
            },
            last_verify_at: if has(columns, "speleo_link", "last_verify_at") {
                maybe(rng, 50, timestamp)
            } else {
                None
            },
            undo_mode: if has(columns, "speleo_link", "undo_mode") {
                *rng.pick(&[None, Some("full"), Some("disabled")])
            } else {
                None
            },
        })
    } else {
        None
    };
    let linked = link.is_some();
    let undo_chain = if linked && columns.contains_key("speleo_undo_chain") && rng.chance(30) {
        Some(hex(rng, 40))
    } else {
        None
    };
    let undo_watermark =
        if linked && columns.contains_key("speleo_undo_watermark") && rng.chance(30) {
            Some(timestamp(rng))
        } else {
            None
        };
    let history = |rng: &mut Rng| -> Vec<HistoryRow> {
        (0..rng.below(3))
            .map(|index| HistoryRow {
                sequence: index as i64 + 1,
                executed_at: timestamp(rng),
            })
            .collect()
    };
    let undo = history(rng);
    let redo = history(rng);

    World {
        name: word(rng),
        location: maybe(rng, 50, word),
        created_at: timestamp(rng),
        modified_at: timestamp(rng),
        next_closure_id,
        sections,
        merges,
        datums,
        section_offsets,
        station_offsets,
        station_comments,
        closures,
        display,
        settings,
        warnings,
        file_hashes,
        section_hashes,
        collab,
        link,
        undo_chain,
        undo_watermark,
        undo,
        redo,
    }
}

/// The content a perfect upgrade from `from` produces: `world` with exactly the
/// documented migration transforms applied (see the module doc).
fn upgraded(world: &World, from: i32) -> World {
    let mut out = world.clone();
    if from < 4 {
        if let Some(value) = out.display.remove("showVirtualShots") {
            out.display.insert("showReferenceShots".to_owned(), value);
        }
    }
    if from < 8 {
        for warning in &mut out.warnings {
            warning.shot_uuid = out
                .sections
                .iter()
                .find(|s| s.id == warning.section_id)
                .and_then(|s| s.shots.get(warning.position as usize))
                .and_then(|shot| shot.uuid.clone());
        }
    }
    if from < 10 {
        if let Some(link) = &mut out.link {
            if link.dirty && link.last_edit_at.is_none() {
                link.last_edit_at = Some("1970-01-01T00:00:00+00:00".to_owned());
            }
        }
    }
    if from < 12 {
        let flipped: Vec<i64> = out
            .sections
            .iter()
            .filter(|s| s.flips)
            .map(|s| s.id)
            .collect();
        // `0.0 - p`, not `-p`: the same negation for every non-zero pitch, but a
        // level reading stays +0.0. A REAL column holds no signed zero (SQLite
        // reads 0.0 back for a stored -0.0), so -0.0 is a value no file can hold.
        for section in out.sections.iter_mut().filter(|s| s.flips) {
            for shot in &mut section.shots {
                shot.pitch_in = shot.pitch_in.map(|p| 0.0 - p);
                shot.pitch_out = shot.pitch_out.map(|p| 0.0 - p);
            }
        }
        out.warnings.retain(|w| {
            !(w.code == "INCLINATION_DEPTH_MISMATCH" && flipped.contains(&w.section_id))
        });
    }
    if from < 13 {
        out.settings
            .insert("pending_kind_classification".to_owned(), "1".to_owned());
        if !out.display.contains_key("showJumps") {
            let value = out
                .display
                .get("showReferenceShots")
                .cloned()
                .unwrap_or_else(|| "true".to_owned());
            out.display.insert("showJumps".to_owned(), value);
        }
    }
    if from < 14 {
        for shot in out.sections.iter_mut().flat_map(|s| s.shots.iter_mut()) {
            shot.azimuth_in = normalize_azimuth(shot.azimuth_in);
            shot.azimuth_out = normalize_azimuth(shot.azimuth_out);
        }
    }
    if from < 15 {
        out.collab = None;
    }
    if from < 17 {
        out.undo.clear();
        out.redo.clear();
    }
    out
}

// ===========================================================================
// Writing a file
// ===========================================================================

fn int(value: i64) -> Value {
    Value::Integer(value)
}

fn real(value: f64) -> Value {
    Value::Real(value)
}

fn opt_real(value: Option<f64>) -> Value {
    value.map_or(Value::Null, Value::Real)
}

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

fn opt_text(value: Option<&String>) -> Value {
    value.map_or(Value::Null, |v| Value::Text(v.clone()))
}

/// `INSERT` the cells of `row` whose columns `table` has; a table the file lacks
/// takes nothing.
fn insert(
    conn: &Connection,
    columns: &Columns,
    table: &str,
    row: Vec<(&str, Value)>,
) -> Result<(), String> {
    let Some(present) = columns.get(table) else {
        return Ok(());
    };
    let kept: Vec<(&str, Value)> = row
        .into_iter()
        .filter(|(column, _)| present.iter().any(|(name, _)| name == column))
        .collect();
    let names: Vec<&str> = kept.iter().map(|(column, _)| *column).collect();
    let marks: Vec<String> = (1..=kept.len()).map(|i| format!("?{i}")).collect();
    let values: Vec<Value> = kept.into_iter().map(|(_, value)| value).collect();
    conn.execute(
        &format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            names.join(", "),
            marks.join(", ")
        ),
        rusqlite::params_from_iter(values),
    )
    .map(|_| ())
    .map_err(|e| format!("INSERT INTO {table}: {e}"))
}

/// A persisted history payload as the file's undo table stores it: TEXT before
/// v3, the gzipped BLOB every later build writes.
fn history_payload(columns: &Columns, table: &str, json: &str) -> Value {
    let text_column = columns.get(table).is_some_and(|present| {
        present
            .iter()
            .any(|(name, kind)| name == "command_json" && kind == "TEXT")
    });
    if text_column {
        text(json)
    } else {
        Value::Blob(compress_history_json(json))
    }
}

fn write_world(conn: &Connection, world: &World, row_version: i64) -> Result<(), String> {
    let columns = columns_of(conn)?;
    let cols = &columns;
    insert(
        conn,
        cols,
        "project",
        vec![
            ("id", int(1)),
            ("name", text(&world.name)),
            ("location", opt_text(world.location.as_ref())),
            ("created_at", text(&world.created_at)),
            ("modified_at", text(&world.modified_at)),
            ("schema_version", int(row_version)),
            ("next_closure_id", int(world.next_closure_id)),
        ],
    )?;
    for section in &world.sections {
        insert(
            conn,
            cols,
            "sections",
            vec![
                ("id", int(section.id)),
                ("name", opt_text(section.name.as_ref())),
                ("surveyor", opt_text(section.surveyor.as_ref())),
                ("date", opt_text(section.date.as_ref())),
                ("color", text(&section.color)),
                ("direction", text(section.direction)),
                ("quality", real(section.quality)),
                ("reverse_parity", int(i64::from(section.reverse_parity))),
                (
                    "gauge_shot_uuid",
                    opt_text(section.gauge.as_ref().map(|g| &g.0)),
                ),
                (
                    "gauge_neighbour_uuid",
                    opt_text(section.gauge.as_ref().map(|g| &g.1)),
                ),
                ("created_at", text(&section.created_at)),
            ],
        )?;
    }
    for section in &world.sections {
        for (order, shot) in section.shots.iter().enumerate() {
            let virtual_flag = int(i64::from(shot.kind != 0));
            insert(
                conn,
                cols,
                "shots",
                vec![
                    ("section_id", int(section.id)),
                    ("sequence_order", int(order as i64)),
                    ("azimuth_in", real(shot.azimuth_in)),
                    ("azimuth_out", real(shot.azimuth_out)),
                    ("length", real(shot.length)),
                    ("depth_in", real(shot.depth_in)),
                    ("depth_out", real(shot.depth_out)),
                    ("pitch_in", opt_real(shot.pitch_in)),
                    ("pitch_out", opt_real(shot.pitch_out)),
                    ("left_distance", opt_real(shot.lrud[0])),
                    ("right_distance", opt_real(shot.lrud[1])),
                    ("up_distance", opt_real(shot.lrud[2])),
                    ("down_distance", opt_real(shot.lrud[3])),
                    ("comment", opt_text(shot.comment.as_ref())),
                    ("kind", int(i64::from(shot.kind))),
                    ("is_reference", virtual_flag.clone()),
                    ("is_virtual", virtual_flag),
                    ("marker_index", shot.marker_index.map_or(Value::Null, int)),
                    ("temperature", opt_real(shot.temperature)),
                    ("uuid", opt_text(shot.uuid.as_ref())),
                ],
            )?;
        }
    }
    for (a, b, merged_at) in &world.merges {
        insert(
            conn,
            cols,
            "station_merges",
            vec![
                ("station_a_section", int(a.0)),
                ("station_a_index", int(a.1)),
                ("station_b_section", int(b.0)),
                ("station_b_index", int(b.1)),
                ("merged_at", text(merged_at)),
            ],
        )?;
    }
    for (at, [latitude, longitude, depth]) in &world.datums {
        insert(
            conn,
            cols,
            "zero_datums",
            vec![
                ("section_id", int(at.0)),
                ("station_index", int(at.1)),
                ("latitude", real(*latitude)),
                ("longitude", real(*longitude)),
                ("depth_reference", real(*depth)),
            ],
        )?;
    }
    for (id, [x, y]) in &world.section_offsets {
        insert(
            conn,
            cols,
            "section_offsets",
            vec![
                ("section_id", int(*id)),
                ("offset_x", real(*x)),
                ("offset_y", real(*y)),
            ],
        )?;
    }
    for (at, [x, y, z]) in &world.station_offsets {
        insert(
            conn,
            cols,
            "station_offsets",
            vec![
                ("section_id", int(at.0)),
                ("station_index", int(at.1)),
                ("offset_x", real(*x)),
                ("offset_y", real(*y)),
                ("offset_z", real(*z)),
            ],
        )?;
    }
    for (at, comment) in &world.station_comments {
        insert(
            conn,
            cols,
            "station_comments",
            vec![
                ("section_id", int(at.0)),
                ("station_index", int(at.1)),
                ("comment", text(comment)),
            ],
        )?;
    }
    for closure in &world.closures {
        let [path, misclosure, percent, horizontal, vertical] = closure.measures;
        insert(
            conn,
            cols,
            "loop_closures",
            vec![
                ("station_a_section", int(closure.a.0)),
                ("station_a_index", int(closure.a.1)),
                ("station_b_section", int(closure.b.0)),
                ("station_b_index", int(closure.b.1)),
                ("path_length", real(path)),
                ("misclosure_distance", real(misclosure)),
                ("error_percentage", real(percent)),
                ("horizontal_misclosure", real(horizontal)),
                ("vertical_misclosure", real(vertical)),
                ("closure_id", int(closure.id)),
                ("strategy", text(closure.strategy)),
                ("is_aligned", int(i64::from(closure.is_aligned))),
            ],
        )?;
    }
    for (key, value) in &world.display {
        insert(
            conn,
            cols,
            "display_settings",
            vec![("key", text(key)), ("value", text(value))],
        )?;
    }
    for (key, value) in &world.settings {
        insert(
            conn,
            cols,
            "project_settings",
            vec![("key", text(key)), ("value", text(value))],
        )?;
    }
    for warning in &world.warnings {
        insert(
            conn,
            cols,
            "resolved_warnings",
            vec![
                ("id", int(warning.id)),
                ("section_id", int(warning.section_id)),
                (
                    "shot_id",
                    int(warning.section_id * i64::from(FEATURE_ID_STRIDE) + warning.position),
                ),
                ("warning_code", text(warning.code)),
                ("resolved_at", text(&warning.resolved_at)),
                ("resolved_comment", opt_text(warning.comment.as_ref())),
                ("shot_uuid", opt_text(warning.shot_uuid.as_ref())),
            ],
        )?;
    }
    for hash in &world.file_hashes {
        insert(
            conn,
            cols,
            "imported_file_hashes",
            vec![("hash", text(hash))],
        )?;
    }
    for hash in &world.section_hashes {
        insert(
            conn,
            cols,
            "imported_section_hashes",
            vec![("hash", text(hash))],
        )?;
    }
    if let Some(state) = &world.collab {
        insert(
            conn,
            cols,
            "collab_state",
            vec![("id", int(1)), ("state_json", text(state))],
        )?;
    }
    if let Some(link) = &world.link {
        insert(
            conn,
            cols,
            "speleo_link",
            vec![
                ("id", int(1)),
                ("endpoint", text("https://example.invalid")),
                ("project_id", text(&link.project_id)),
                ("project_name", text(&link.project_name)),
                ("base_commit_sha", opt_text(link.base_commit_sha.as_ref())),
                ("has_unpushed_edits", int(i64::from(link.dirty))),
                ("last_edit_at", opt_text(link.last_edit_at.as_ref())),
                ("last_verify_at", opt_text(link.last_verify_at.as_ref())),
                ("undo_mode", link.undo_mode.map_or(Value::Null, text)),
            ],
        )?;
    }
    if let Some(sha) = &world.undo_chain {
        insert(
            conn,
            cols,
            "speleo_undo_chain",
            vec![("id", int(1)), ("unwound_from_sha", text(sha))],
        )?;
    }
    if let Some(at) = &world.undo_watermark {
        insert(
            conn,
            cols,
            "speleo_undo_watermark",
            vec![("id", int(1)), ("inherited_through", text(at))],
        )?;
    }
    for (table, rows) in [("undo_stack", &world.undo), ("redo_stack", &world.redo)] {
        for row in rows {
            insert(
                conn,
                cols,
                table,
                vec![
                    ("sequence", int(row.sequence)),
                    (
                        "command_json",
                        history_payload(cols, table, HISTORY_COMMAND),
                    ),
                    ("undo_data_json", history_payload(cols, table, HISTORY_UNDO)),
                    ("executed_at", text(&row.executed_at)),
                ],
            )?;
        }
    }
    Ok(())
}

fn stamp(conn: &Connection, user_version: i32) -> Result<(), String> {
    conn.execute_batch(&format!("PRAGMA user_version = {user_version};"))
        .map_err(text_of)
}

/// A file as an `era` build wrote it: its `init_schema`, the content, its stamps.
fn build_era_file(
    path: &Path,
    era: &Era,
    world: &World,
    user_version: i32,
    row_version: i64,
) -> Result<(), String> {
    let conn = Connection::open(path).map_err(text_of)?;
    for statement in era.ddl {
        conn.execute_batch(statement).map_err(text_of)?;
    }
    write_world(&conn, world, row_version)?;
    stamp(&conn, user_version)
}

/// A table and a column this build has never heard of, as a newer build might add.
const FUTURE_EXTRAS: &[&str] = &[
    "CREATE TABLE future_feature (id INTEGER PRIMARY KEY, payload TEXT NOT NULL); \
     INSERT INTO future_feature (payload) VALUES ('from the future');",
    "ALTER TABLE shots ADD COLUMN future_column TEXT DEFAULT 'later';",
    "ALTER TABLE sections ADD COLUMN future_rank INTEGER NOT NULL DEFAULT 7;",
];

/// A file at this build's own schema (`Database::open` creates it), holding `world`.
fn build_current_file(
    path: &Path,
    world: &World,
    user_version: i32,
    row_version: i64,
    extras: &[&str],
) -> Result<(), String> {
    drop(Database::open(path).map_err(text_of)?);
    let conn = Connection::open(path).map_err(text_of)?;
    write_world(&conn, world, row_version)?;
    for extra in extras {
        conn.execute_batch(extra).map_err(text_of)?;
    }
    stamp(&conn, user_version)
}

// ===========================================================================
// The oracles
// ===========================================================================

fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

fn same_opt(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// The loaded survey against the generated rows, field by field (a backup for the
/// differential oracle: both sides of that could lose a value the same way).
fn check_loaded(project: &Project, expected: &World, version: i32) -> Vec<String> {
    let mut out = Vec::new();
    if project.name != expected.name || project.location != expected.location {
        out.push(format!(
            "project name/location {:?}/{:?}, expected {:?}/{:?}",
            project.name, project.location, expected.name, expected.location
        ));
    }
    if project.created_at.as_str() != expected.created_at {
        out.push(format!(
            "project created_at {:?}, expected {:?} byte for byte",
            project.created_at.as_str(),
            expected.created_at
        ));
    }
    if project.sections.len() != expected.sections.len() {
        out.push(format!(
            "{} section(s) loaded, {} written",
            project.sections.len(),
            expected.sections.len()
        ));
    }
    for want in &expected.sections {
        let Some(got) = project.sections.iter().find(|s| i64::from(s.id) == want.id) else {
            out.push(format!("section {} is missing", want.id));
            continue;
        };
        let metadata = (
            got.name.as_ref(),
            got.surveyor.as_ref(),
            got.date.map(|d| d.to_string()),
            got.color.as_str(),
            got.direction.to_string(),
            got.reverse_parity,
            got.created_at.as_ref().map(|c| c.as_str().to_owned()),
        );
        let wanted = (
            want.name.as_ref(),
            want.surveyor.as_ref(),
            want.date.clone(),
            want.color.as_str(),
            want.direction.to_owned(),
            want.reverse_parity,
            Some(want.created_at.clone()),
        );
        if metadata != wanted || !same(got.quality, want.quality) {
            out.push(format!(
                "section {}: loaded {metadata:?} quality {}, written {wanted:?} quality {}",
                want.id, got.quality, want.quality
            ));
        }
        if got.shots.len() != want.shots.len() {
            out.push(format!(
                "section {}: {} shot(s) loaded, {} written",
                want.id,
                got.shots.len(),
                want.shots.len()
            ));
            continue;
        }
        for (position, (shot, row)) in got.shots.iter().zip(&want.shots).enumerate() {
            let survey_equal = same(shot.azimuth_in, row.azimuth_in)
                && same(shot.azimuth_out, row.azimuth_out)
                && same(shot.length, row.length)
                && same(shot.depth_in, row.depth_in)
                && same(shot.depth_out, row.depth_out)
                && same_opt(shot.pitch_in, row.pitch_in)
                && same_opt(shot.pitch_out, row.pitch_out)
                && same_opt(shot.left, row.lrud[0])
                && same_opt(shot.right, row.lrud[1])
                && same_opt(shot.up, row.lrud[2])
                && same_opt(shot.down, row.lrud[3])
                && same_opt(shot.temperature, row.temperature)
                && shot.comment == row.comment
                && shot.marker_index.map(i64::from) == row.marker_index;
            // Before v13 the file says only "virtual or not": the backfill picks
            // Jump or Reference, and either keeps the shot virtual.
            let kind_ok = if version < 13 {
                shot.kind.is_virtual() == (row.kind != 0)
            } else {
                shot.kind.as_u8() == row.kind
            };
            let uuid_ok = match &row.uuid {
                Some(uuid) => shot.uuid.as_ref() == Some(uuid),
                None => shot.uuid.is_some(),
            };
            if !(survey_equal && kind_ok && uuid_ok) {
                out.push(format!(
                    "section {} shot {position}: loaded az {}/{} len {} depth {}/{} pitch {:?}/{:?} \
                     kind {} uuid {:?}; expected az {}/{} len {} depth {}/{} pitch {:?}/{:?} kind {} \
                     uuid {:?} (or any other field differs)",
                    want.id,
                    shot.azimuth_in,
                    shot.azimuth_out,
                    shot.length,
                    shot.depth_in,
                    shot.depth_out,
                    shot.pitch_in,
                    shot.pitch_out,
                    shot.kind.as_u8(),
                    shot.uuid,
                    row.azimuth_in,
                    row.azimuth_out,
                    row.length,
                    row.depth_in,
                    row.depth_out,
                    row.pitch_in,
                    row.pitch_out,
                    row.kind,
                    row.uuid
                ));
            }
        }
    }
    out
}

/// Independent of [`upgraded`]'s use of `normalize_azimuth`: every stored azimuth
/// of a migrated file is in `[0, 360)` and a whole number of turns from what the
/// old file held.
fn check_azimuths(path: &Path, written: &World) -> Result<Vec<String>, String> {
    let conn = read_only(path)?;
    let mut out = Vec::new();
    for section in &written.sections {
        for (order, shot) in section.shots.iter().enumerate() {
            let (stored_in, stored_out): (f64, f64) = conn
                .query_row(
                    "SELECT azimuth_in, azimuth_out FROM shots WHERE section_id = ?1 AND sequence_order = ?2",
                    [section.id, order as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(text_of)?;
            for (stored, old) in [(stored_in, shot.azimuth_in), (stored_out, shot.azimuth_out)] {
                let turns = (old - stored) / 360.0;
                if !(0.0..360.0).contains(&stored) || (turns - turns.round()).abs() > 1e-9 {
                    out.push(format!(
                        "section {} shot {order}: azimuth {old} migrated to {stored}",
                        section.id
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// One open through the production load path, in a fresh state.
fn open(
    path: &Path,
    upgrade_refusal: Option<&UpgradeRefusal>,
) -> (
    AppState,
    Result<crate::core::loading::LoadProjectFullResult, crate::core::loading::LoadError>,
) {
    let state = state_with_null_sink();
    let loaded = state.do_load_project_full_with(path.to_path_buf(), upgrade_refusal, None);
    (state, loaded)
}

fn opened(path: &Path, what: &str) -> Result<AppState, String> {
    let (state, loaded) = open(path, None);
    match loaded {
        Ok(_) => Ok(state),
        Err(e) => Err(format!(
            "{what}: the open failed: code={} message={:?}",
            e.code, e.message
        )),
    }
}

fn saved(state: &AppState, what: &str) -> Result<(), String> {
    let result = state.save_project(None);
    if result.success {
        Ok(())
    } else {
        Err(format!(
            "{what}: the save failed: code={:?} error={:?}",
            result.code, result.error
        ))
    }
}

/// [(e)] Under an upgrade refusal the open is refused with `code` and `message`,
/// writes nothing and leaves nothing beside the file.
fn check_refused(
    path: &Path,
    upgrade_refusal: Option<&UpgradeRefusal>,
    code: CommandErrorCode,
    message: Option<&str>,
    what: &str,
) -> Result<(), String> {
    let dir = path.parent().ok_or("no parent")?;
    let before = std::fs::read(path).map_err(text_of)?;
    let listed = files_in(dir)?;
    let (state, loaded) = open(path, upgrade_refusal);
    let Err(error) = loaded else {
        return Err(format!("{what}: the open was not refused"));
    };
    if error.code != code {
        return Err(format!(
            "{what}: refused with code {} ({:?}), expected {code}",
            error.code, error.message
        ));
    }
    if let Some(message) = message {
        if error.message != message {
            return Err(format!(
                "{what}: refused with message {:?}, expected {message:?}",
                error.message
            ));
        }
    }
    if state.get_project_state().is_some() {
        return Err(format!("{what}: a refused open left a project open"));
    }
    drop(state);
    if std::fs::read(path).map_err(text_of)? != before {
        return Err(format!("{what}: the refused file was written"));
    }
    let after = files_in(dir)?;
    if after != listed {
        return Err(format!(
            "{what}: the refused open left files beside it: {listed:?} -> {after:?}"
        ));
    }
    Ok(())
}

/// [(e)] A file this build reads as it is opens under a refusal and is not
/// written: not a byte, not its mtime, nothing beside it.
fn check_opens_untouched(
    path: &Path,
    upgrade_refusal: Option<&UpgradeRefusal>,
    what: &str,
) -> Result<AppState, String> {
    let dir = path.parent().ok_or("no parent")?;
    let before = std::fs::read(path).map_err(text_of)?;
    let mtime = modified(path)?;
    let listed = files_in(dir)?;
    let (state, loaded) = open(path, upgrade_refusal);
    if let Err(e) = loaded {
        return Err(format!(
            "{what}: the open failed: code={} message={:?}",
            e.code, e.message
        ));
    }
    if std::fs::read(path).map_err(text_of)? != before || modified(path)? != mtime {
        return Err(format!("{what}: opening it wrote to it"));
    }
    if files_in(dir)? != listed {
        return Err(format!(
            "{what}: opening it left files beside it: {listed:?} -> {:?}",
            files_in(dir)?
        ));
    }
    Ok(state)
}

fn modified(path: &Path) -> Result<SystemTime, String> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(text_of)
}

fn fail(problems: Vec<String>, what: &str) -> Result<(), String> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{what}: {}", problems.join("; ")))
    }
}

// ===========================================================================
// The cases
// ===========================================================================

/// One fuzz case.
#[derive(Clone, Copy, Debug)]
enum Case {
    /// A file an era's build wrote: stamped (`user_version` = the era's version), or
    /// with the stamp lost and only the project row claiming the version.
    Older { era: usize, stamped: bool },
    /// Unstamped, the project row claiming this build's version.
    UnstampedCurrent,
    /// Unstamped, the project row claiming a version above this build's.
    UnstampedFuture(i32),
    /// Stamped by a newer build, with a table and a column this build does not know.
    Newer(i32),
    /// From before the first public release: both claims below schema 2.
    PreRelease { era: usize, stamp: i32, row: i32 },
}

fn era_at(index: usize) -> Result<&'static Era, String> {
    ERAS.get(index).ok_or_else(|| format!("no era {index}"))
}

/// What a seed of the long run exercises.
fn case_for(seed: u32, current: i32) -> Case {
    let mut rng = Rng::new(seed.wrapping_mul(0x9e37_79b9) ^ 0x05ee_d5c4);
    match rng.below(100) {
        0..=59 => Case::Older {
            era: rng.below(ERAS.len()),
            stamped: true,
        },
        60..=74 => Case::Older {
            era: rng.below(ERAS.len()),
            stamped: false,
        },
        75..=79 => Case::UnstampedCurrent,
        80..=84 => Case::UnstampedFuture(rng.range(i64::from(current) + 1, 99) as i32),
        85..=94 => Case::Newer(if rng.chance(80) {
            rng.range(i64::from(current) + 1, 99) as i32
        } else {
            rng.range(100, i64::from(i32::MAX)) as i32
        }),
        _ => Case::PreRelease {
            era: rng.below(ERAS.len()),
            stamp: rng.range(0, 1) as i32,
            row: rng.range(-3, 1) as i32,
        },
    }
}

/// The working directories of one case: the file under test and the reference
/// file each get their own folder, so each folder's listing is that file's alone.
struct Dirs {
    _root: TempDir,
    file: PathBuf,
    reference: PathBuf,
}

fn dirs() -> Result<Dirs, String> {
    let root = TempDir::new().map_err(text_of)?;
    let file_dir = root.path().join("file");
    let reference_dir = root.path().join("reference");
    std::fs::create_dir(&file_dir).map_err(text_of)?;
    std::fs::create_dir(&reference_dir).map_err(text_of)?;
    Ok(Dirs {
        file: file_dir.join(FILE),
        reference: reference_dir.join(FILE),
        _root: root,
    })
}

/// Run one case. `Ok` carries a one-line description of what was exercised.
fn run(seed: u32, case: Case, current: i32) -> Result<String, String> {
    match case {
        Case::Older { era, stamped } => run_older(seed, era_at(era)?, stamped, current),
        Case::UnstampedCurrent => run_unstamped_current(seed, current),
        Case::UnstampedFuture(claim) => run_unstamped_future(seed, claim, current),
        Case::Newer(version) => run_newer(seed, version, current),
        Case::PreRelease { era, stamp, row } => run_pre_release(seed, era_at(era)?, stamp, row),
    }
}

/// The comparisons every opened-then-saved case shares: after the open, the file
/// under test against the reference file (tables and rows) and the two loaded
/// projects; the loaded survey against `expected`; both saved; the saved files
/// against each other; then the save-reopen fixpoint of the file under test.
/// `fixups` rewrites the file-under-test's dump rows the case documents as
/// different (an unstamped file keeps its row's claim).
fn compare_with_reference(
    state: &AppState,
    path: &Path,
    reference_path: &Path,
    expected: &World,
    version: i32,
    fixups: &dyn Fn(&mut BTreeMap<String, Vec<String>>),
) -> Result<(usize, usize), String> {
    let reference = opened(reference_path, "the reference file")?;

    // (a) The upgraded file holds what a perfect upgrade holds, table by table.
    let columns = columns_of(&read_only(path)?)?;
    let reference_columns = columns_of(&read_only(reference_path)?)?;
    fail(
        schema_differences(&columns, &reference_columns, ("upgraded", "reference")),
        "the upgraded file's tables and columns differ from a current file's",
    )?;
    let mut rows = table_rows(path)?;
    fixups(&mut rows);
    let reference_rows = table_rows(reference_path)?;
    fail(
        row_differences(&rows, &reference_rows, ("upgraded", "reference")),
        "after the open, the upgraded file's rows differ from the reference",
    )?;

    // (a) The loaded projects agree, and the loaded survey is the generated one.
    let project = state
        .get_project_state()
        .ok_or("no project after the open")?;
    fail(
        check_loaded(&project, expected, version),
        "the loaded survey",
    )?;
    let loaded = positional_snapshot(state)?;
    let loaded_reference = positional_snapshot(&reference)?;
    if let Some(diff) = snapshot_differences(&loaded, &loaded_reference) {
        return Err(format!(
            "the loaded project differs from the reference's: {diff}"
        ));
    }
    let (undo, redo) = (state.get_undo_stack().len(), state.get_redo_stack().len());
    let reference_history = (
        reference.get_undo_stack().len(),
        reference.get_redo_stack().len(),
    );
    if (undo, redo) != reference_history {
        return Err(format!(
            "restored history undo={undo} redo={redo}, the reference's {reference_history:?}"
        ));
    }

    // (a) Saved, the two files still agree.
    saved(state, "the upgraded file")?;
    saved(&reference, "the reference file")?;
    let dump = positional_dump(path)?;
    let reference_dump = positional_dump(reference_path)?;
    if dump != reference_dump {
        return Err(format!(
            "after the save, the files differ: {}",
            dump_line_differences(&dump, &reference_dump, ("upgraded", "reference")).join(" ")
        ));
    }

    // (c) Open -> save -> reopen is a fixpoint.
    let first_snapshot = canonical_snapshot(state, false);
    let first_dump = dump_database(path)?;
    let again = opened(path, "the saved upgraded file")?;
    if let Some(diff) = snapshot_differences(&first_snapshot, &canonical_snapshot(&again, false)) {
        return Err(format!(
            "reopening the saved file loads a different project: {diff}"
        ));
    }
    saved(&again, "the reopened file")?;
    let second_dump = dump_database(path)?;
    if second_dump != first_dump {
        return Err(format!(
            "save -> reopen -> save is not a fixpoint: {}",
            dump_line_differences(&first_dump, &second_dump, ("first save", "second save"))
                .join(" ")
        ));
    }
    Ok((undo, redo))
}

fn run_older(seed: u32, era: &Era, stamped: bool, current: i32) -> Result<String, String> {
    let mut rng = Rng::new(seed);
    let version = era.version;
    let columns = era_columns(era)?;
    let world = generate(&mut rng, &columns, version);
    let dirs = dirs()?;
    let stamp_value = if stamped { version } else { 0 };
    build_era_file(&dirs.file, era, &world, stamp_value, i64::from(version))?;
    let expected = upgraded(&world, version);
    build_current_file(&dirs.reference, &expected, current, i64::from(current), &[])?;
    let before = std::fs::read(&dirs.file).map_err(text_of)?;
    let dir = dirs.file.parent().ok_or("no parent")?;

    // (e) Where nothing may be written: refused, untouched, nothing beside it. A
    // current file (the reference) opens there and is not written.
    if rng.chance(50) {
        check_refused(
            &dirs.file,
            Some(&refusal()),
            CommandErrorCode::UpgradeRefused,
            Some(OLDER_REFUSAL),
            "older file, protected folder",
        )?;
        drop(check_opens_untouched(
            &dirs.reference,
            Some(&refusal()),
            "current file, protected folder",
        )?);
    }

    let state = opened(&dirs.file, "the older file")?;

    // (b) Exactly one pre-upgrade copy, byte-identical; no partial copy.
    let backup = format!("{FILE}.pre-v{version}.backup");
    let expected_files = vec![FILE.to_owned(), backup.clone()];
    let listed = files_in(dir)?;
    if listed != expected_files {
        return Err(format!(
            "after the upgrade the folder holds {listed:?}, expected {expected_files:?}"
        ));
    }
    let backup_path = dir.join(&backup);
    if std::fs::read(&backup_path).map_err(text_of)? != before {
        return Err("the pre-upgrade copy is not the file as it was".to_owned());
    }
    let (stamp_after, row_after) = (user_version(&dirs.file)?, row_version(&dirs.file)?);
    if stamp_after != current || row_after != i64::from(current) {
        return Err(format!(
            "the upgraded file is stamped user_version={stamp_after} schema_version={row_after}, \
             expected {current}"
        ));
    }
    if columns.contains_key("collab_state")
        && read_only(&dirs.file)?
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'collab_state'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(text_of)?
            != 0
    {
        return Err("collab_state survived the upgrade".to_owned());
    }
    fail(
        check_azimuths(&dirs.file, &world)?,
        "an azimuth was not wrapped into [0, 360)",
    )?;
    let history = count_rows(&dirs.file, "undo_stack")? + count_rows(&dirs.file, "redo_stack")?;
    let kept_history = (expected.undo.len() + expected.redo.len()) as i64;
    if history != kept_history {
        return Err(format!(
            "the upgraded file holds {history} history row(s), expected {kept_history}"
        ));
    }

    let (undo, redo) = compare_with_reference(
        &state,
        &dirs.file,
        &dirs.reference,
        &expected,
        version,
        &|_| {},
    )?;
    drop(state);

    // (b) Still the one copy, untouched, after the saves and the reopen.
    let listed = files_in(dir)?;
    if listed != expected_files {
        return Err(format!(
            "after the saves the folder holds {listed:?}, expected {expected_files:?}"
        ));
    }
    if std::fs::read(&backup_path).map_err(text_of)? != before {
        return Err("a save changed the pre-upgrade copy".to_owned());
    }

    // (b) Another file from the same version at that path: its upgrade keeps the
    // FIRST copy rather than replacing it, and writes no second one.
    if rng.chance(40) {
        let second = generate(&mut rng, &columns, version);
        std::fs::remove_file(&dirs.file).map_err(text_of)?;
        build_era_file(&dirs.file, era, &second, stamp_value, i64::from(version))?;
        drop(opened(&dirs.file, "a second older file at the same path")?);
        if files_in(dir)? != expected_files {
            return Err(format!(
                "a second upgrade from v{version} left {:?}",
                files_in(dir)?
            ));
        }
        if std::fs::read(&backup_path).map_err(text_of)? != before {
            return Err("a second upgrade replaced the first pre-upgrade copy".to_owned());
        }
    }

    Ok(format!(
        "{} {}: {} section(s), {} shot(s), {} flipped, history restored {undo}/{redo}",
        era.label(),
        if stamped { "stamped" } else { "unstamped" },
        world.sections.len(),
        world.sections.iter().map(|s| s.shots.len()).sum::<usize>(),
        world.sections.iter().filter(|s| s.flips).count()
    ))
}

/// Content at this build's own schema.
fn current_world(rng: &mut Rng, current: i32) -> Result<World, String> {
    let dir = TempDir::new().map_err(text_of)?;
    let probe = dir.path().join("probe.stalx");
    drop(Database::open(&probe).map_err(text_of)?);
    let columns = columns_of(&read_only(&probe)?)?;
    Ok(generate(rng, &columns, current))
}

fn run_unstamped_current(seed: u32, current: i32) -> Result<String, String> {
    let mut rng = Rng::new(seed);
    let world = current_world(&mut rng, current)?;
    let dirs = dirs()?;
    build_current_file(&dirs.file, &world, 0, i64::from(current), &[])?;
    build_current_file(&dirs.reference, &world, current, i64::from(current), &[])?;
    let dir = dirs.file.parent().ok_or("no parent")?;
    // (e) Not completed in place there either, and not called older.
    check_refused(
        &dirs.file,
        Some(&refusal()),
        CommandErrorCode::UpgradeRefused,
        Some(UNSTAMPED_REFUSAL),
        "unstamped current file, protected folder",
    )?;
    let state = opened(&dirs.file, "the unstamped current file")?;
    // docs/saving.md: no copy (not an upgrade), and nothing is stamped.
    if files_in(dir)? != [FILE] {
        return Err(format!("the open left {:?}", files_in(dir)?));
    }
    if user_version(&dirs.file)? != 0 {
        return Err("an unstamped file claiming the current version was stamped".to_owned());
    }
    compare_with_reference(
        &state,
        &dirs.file,
        &dirs.reference,
        &world,
        current,
        &|_| {},
    )?;
    if files_in(dir)? != [FILE] {
        return Err(format!("the saves left {:?}", files_in(dir)?));
    }
    Ok(format!(
        "unstamped claiming v{current}: {} section(s)",
        world.sections.len()
    ))
}

fn run_unstamped_future(seed: u32, claim: i32, current: i32) -> Result<String, String> {
    let mut rng = Rng::new(seed);
    let world = current_world(&mut rng, current)?;
    let dirs = dirs()?;
    build_current_file(&dirs.file, &world, 0, i64::from(claim), &[])?;
    // `ensure_schema_version_with`: the row's word drops the history the arms
    // exist to sanitize, and nothing is stamped or copied.
    let mut expected = world;
    expected.undo.clear();
    expected.redo.clear();
    build_current_file(&dirs.reference, &expected, current, i64::from(current), &[])?;
    let dir = dirs.file.parent().ok_or("no parent")?;
    check_refused(
        &dirs.file,
        Some(&refusal()),
        CommandErrorCode::UpgradeRefused,
        Some(UNSTAMPED_REFUSAL),
        "unstamped file claiming a future version, protected folder",
    )?;
    let (state, loaded) = open(&dirs.file, None);
    let loaded = loaded.map_err(|e| {
        format!(
            "the unstamped future-claiming file did not open: code={} message={:?}",
            e.code, e.message
        )
    })?;
    if loaded.newer_schema_version.is_some() || state.newer_file_schema().is_some() {
        return Err(
            "an unstamped file was opened as a newer one (only the stamp decides)".to_owned(),
        );
    }
    if files_in(dir)? != [FILE] {
        return Err(format!("the open left {:?}", files_in(dir)?));
    }
    if user_version(&dirs.file)? != 0 || row_version(&dirs.file)? != i64::from(claim) {
        return Err("the unverified future claim was stamped or overwritten".to_owned());
    }
    let claim_cell = format!("schema_version={claim}");
    let current_cell = format!("schema_version={current}");
    compare_with_reference(
        &state,
        &dirs.file,
        &dirs.reference,
        &expected,
        current,
        &|rows| {
            if let Some(project) = rows.get_mut("project") {
                for row in project.iter_mut() {
                    *row = row.replace(&claim_cell, &current_cell);
                }
            }
        },
    )?;
    Ok(format!("unstamped claiming v{claim}"))
}

fn run_newer(seed: u32, version: i32, current: i32) -> Result<String, String> {
    let mut rng = Rng::new(seed);
    let world = current_world(&mut rng, current)?;
    let dirs = dirs()?;
    let extras: Vec<&str> = FUTURE_EXTRAS
        .iter()
        .copied()
        .filter(|_| rng.chance(70))
        .collect();
    build_current_file(&dirs.file, &world, version, i64::from(version), &extras)?;
    build_current_file(&dirs.reference, &world, current, i64::from(current), &[])?;
    let dir = dirs.file.parent().ok_or("no parent")?;
    let before = std::fs::read(&dirs.file).map_err(text_of)?;
    let mtime = modified(&dirs.file)?;

    // (e) A newer file in a protected folder still opens (read-only), untouched.
    if rng.chance(50) {
        drop(check_opens_untouched(
            &dirs.file,
            Some(&refusal()),
            "newer file, protected folder",
        )?);
    }
    let (state, loaded) = open(&dirs.file, None);
    let loaded = loaded.map_err(|e| {
        format!(
            "the v{version} file did not open: code={} message={:?}",
            e.code, e.message
        )
    })?;
    if loaded.newer_schema_version != Some(version) || state.newer_file_schema() != Some(version) {
        return Err(format!(
            "a v{version} file opened with newer_schema_version {:?} / {:?}",
            loaded.newer_schema_version,
            state.newer_file_schema()
        ));
    }
    let reference = opened(&dirs.reference, "the reference file")?;
    if let Some(diff) = snapshot_differences(
        &positional_snapshot(&state)?,
        &positional_snapshot(&reference)?,
    ) {
        return Err(format!(
            "the newer file loads differently from the same content at v{current}: {diff}"
        ));
    }
    let elsewhere = dir.with_file_name("elsewhere.stalx");
    for (label, target) in [
        ("in place", None),
        ("to its own path", Some(dirs.file.clone())),
        ("as another file", Some(elsewhere.clone())),
    ] {
        let result = state.save_project(target);
        if result.success || result.code != Some(CommandErrorCode::NewerFileReadOnly) {
            return Err(format!(
                "saving a v{version} file {label}: success={} code={:?}, expected the \
                 NEWER_FILE_READ_ONLY refusal",
                result.success, result.code
            ));
        }
    }
    drop(state);
    if elsewhere.exists() {
        return Err("a refused Save As wrote its target".to_owned());
    }
    if std::fs::read(&dirs.file).map_err(text_of)? != before || modified(&dirs.file)? != mtime {
        return Err(format!("the v{version} file was written"));
    }
    if files_in(dir)? != [FILE] {
        return Err(format!(
            "the v{version} file has {:?} beside it",
            files_in(dir)?
        ));
    }
    Ok(format!("newer v{version}, {} extra(s)", extras.len()))
}

fn run_pre_release(seed: u32, era: &Era, stamp_value: i32, row: i32) -> Result<String, String> {
    let mut rng = Rng::new(seed);
    let columns = era_columns(era)?;
    let world = generate(&mut rng, &columns, era.version);
    let dirs = dirs()?;
    build_era_file(&dirs.file, era, &world, stamp_value, i64::from(row))?;
    for (refusal_given, label) in [(false, "anywhere"), (true, "in a protected folder")] {
        let given = refusal();
        check_refused(
            &dirs.file,
            refusal_given.then_some(&given),
            CommandErrorCode::PreReleaseFile,
            None,
            &format!("pre-release file (stamp {stamp_value}, row {row}) {label}"),
        )?;
    }
    Ok(format!(
        "pre-release stamp {stamp_value} row {row} on {} DDL",
        era.label()
    ))
}

/// One case, a panic included, as a result.
fn run_caught(seed: u32, case: Case, current: i32) -> Result<String, String> {
    match catch_unwind(AssertUnwindSafe(|| run(seed, case, current))) {
        Ok(result) => result,
        Err(panic) => Err(format!(
            "panicked: {}",
            panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("<non-string panic>")
        )),
    }
}

fn one_line(text: &str) -> String {
    let line = text.replace('\n', " / ");
    if line.chars().count() > 900 {
        format!("{}...", line.chars().take(900).collect::<String>())
    } else {
        line
    }
}

// ===========================================================================
// The tests
// ===========================================================================

/// Seeds a long run found failing, kept as regressions (`case_for` derives each
/// one's case, as the run did).
const REGRESSION_SEEDS: &[u32] = &[
    // Oracle: a level shot (pitch exactly 0.0) in a section the v12 migration
    // flips; the model negated it to -0.0, which no file can hold.
    4_000_574,
];

/// The CI gate: every era, stamped; a few eras unstamped; every other shape.
fn gate_cases(current: i32) -> Vec<(u32, Case)> {
    let mut cases: Vec<(u32, Case)> = (0..ERAS.len())
        .map(|era| (1000 + era as u32, Case::Older { era, stamped: true }))
        .collect();
    for (seed, era) in [(2001, 0), (2002, 5), (2003, 12), (2004, ERAS.len() - 1)] {
        cases.push((
            seed,
            Case::Older {
                era,
                stamped: false,
            },
        ));
    }
    cases.push((3001, Case::UnstampedCurrent));
    cases.push((3002, Case::UnstampedFuture(current + 1)));
    cases.push((3003, Case::Newer(current + 1)));
    cases.push((3004, Case::Newer(99)));
    cases.push((3005, Case::Newer(i32::MAX)));
    cases.push((
        3006,
        Case::PreRelease {
            era: 0,
            stamp: 0,
            row: 1,
        },
    ));
    cases.push((
        3007,
        Case::PreRelease {
            era: 1,
            stamp: 1,
            row: 0,
        },
    ));
    for seed in REGRESSION_SEEDS {
        cases.push((*seed, case_for(*seed, current)));
    }
    cases
}

/// The eras cover every version this build upgrades, each with DDL that runs.
#[test]
fn the_eras_cover_every_older_schema() {
    let _config = isolated_config();
    let current = current_version().unwrap_or(0);
    let covered: BTreeSet<i32> = ERAS.iter().map(|era| era.version).collect();
    let wanted: BTreeSet<i32> = (2..current).collect();
    println!("eras cover {covered:?}; this build upgrades {wanted:?}");
    assert_eq!(
        covered, wanted,
        "every schema this build upgrades needs an era: add the bumped-from version's \
         init_schema to ERAS"
    );
    for era in ERAS {
        let columns = era_columns(era);
        assert!(columns.is_ok(), "{}: {columns:?}", era.label());
    }
}

#[test]
fn schema_history_fuzz_gate() {
    let _config = isolated_config();
    let current = current_version().unwrap_or(0);
    assert!(current > 2, "could not read this build's schema version");
    let started = Instant::now();
    let mut failures = Vec::new();
    for (seed, case) in gate_cases(current) {
        match run_caught(seed, case, current) {
            Ok(summary) => println!("seed {seed} ok: {summary}"),
            Err(reason) => failures.push(format!(
                "FZ-PERSIST FAIL schema seed={seed} {case:?}: {}",
                one_line(&reason)
            )),
        }
    }
    println!(
        "schema_history_fuzz_gate: {} case(s), {} failure(s), {:.1}s",
        gate_cases(current).len(),
        failures.len(),
        started.elapsed().as_secs_f64()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The long run. `STALAGMIGHT_RUST_PERSIST_SEEDS` seeds (default 200) from
/// `STALAGMIGHT_RUST_PERSIST_SEED_START` (default 1).
#[test]
#[ignore = "long run: STALAGMIGHT_RUST_PERSIST_SEEDS=<count> [STALAGMIGHT_RUST_PERSIST_SEED_START=<first>]"]
fn schema_history_fuzz_sweep() {
    let _config = isolated_config();
    let current = current_version().unwrap_or(0);
    assert!(current > 2, "could not read this build's schema version");
    let knob = |name: &str, default: u32| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(default)
    };
    let count = knob("STALAGMIGHT_RUST_PERSIST_SEEDS", 200);
    let start = knob("STALAGMIGHT_RUST_PERSIST_SEED_START", 1);
    let end = start.saturating_add(count.saturating_sub(1));
    let started = Instant::now();
    let mut failed = 0_u32;
    for seed in start..=end {
        let case = case_for(seed, current);
        if let Err(reason) = run_caught(seed, case, current) {
            failed += 1;
            println!(
                "FZ-PERSIST FAIL schema seed={seed} {case:?}: {}",
                one_line(&reason)
            );
        }
    }
    println!(
        "FZ-PERSIST SUMMARY schema seeds={start}..{end} failed={failed} secs={:.0}",
        started.elapsed().as_secs_f64()
    );
    assert!(failed == 0, "{failed} seed(s) failed; see the FAIL lines");
}

// ===========================================================================
// The historical DDL, verbatim (its own module: see its doc)
// ===========================================================================

#[cfg(test)]
mod historical_ddl_tests;
use historical_ddl_tests::ERAS;
