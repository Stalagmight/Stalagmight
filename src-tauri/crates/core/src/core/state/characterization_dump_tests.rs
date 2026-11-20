//! Characterisation goldens (stream K of
//! `docs/plans/2026-11-20_station-graph-model.md`, sections 11 and 12).
//!
//! Recorded on the 0.6.0 model (`models::project`) before the switch, this
//! dump captures what the build does for a fixed set of synthetic scenarios;
//! run on the station-graph build (`models::survey`), it gives the output to
//! compare with those goldens, so every difference can be shown to be a
//! declared change (plan 12.2 and the Deviations sections).
//!
//! Each scenario starts a fresh `AppState`, runs its steps through the same
//! `Command` / `AppState` entry points the IPC handlers call, and after every
//! step writes what the frontend would read:
//!
//! - `NN-<step>-result.json`: the `CommandResult`, what the step expected
//!   (success or refusal) and how the undo depth changed;
//! - `NN-<step>-history.json`: `get_undo_stack` / `get_redo_stack` items
//!   (description, commandType, previewData; timestamps masked);
//! - `NN-<step>-stations.json`: the `get_project_state` station list (slot,
//!   coordinates, merged_with, closures_with, id, name, comment, datum
//!   fields), stats and frozen shots;
//! - `NN-<step>-adjusted.json`: `get_adjusted_stations` and the adjustment
//!   report;
//! - `NN-<step>-closures.json`: loop closures (metrics, `is_aligned`), merges,
//!   zero datums and their derived roles, in wire form;
//! - `NN-<step>-project.json`: the whole wire `Project` (the `project` member
//!   of `get_project_state`);
//! - `NN-<step>-warnings.json`: `get_project_warnings` and the integrity check.
//!
//! A preview step writes one `NN-<step>-<preview>.json` (merge connectivity,
//! delete shot, delete shot range, combine candidates and their plans, split
//! line, zero datum, line relationships). After the last step every scenario
//! runs a full undo/redo round trip (`NN-roundtrip-result.json`) and writes
//! its exports (`final-export.csv`, `.json`, `.tmlu`, `.tml.xml` - the
//! `Data.xml` inside the `.tml` zip - and two `.svg`, one labelled with
//! Station IDs and one with station names). `steps.json` lists the steps of
//! a scenario, `manifest.json` the scenarios.
//!
//! Determinism: object keys are sorted; derived floats (coordinates, closure
//! metrics, previews, stats) are rounded to 1e-9 and `-0` is written as `0`;
//! timestamps (`created_at`, `modified_at`, `merged_at`, `exported_at`,
//! `resolved_at`, `timestamp`) are masked, and so is any RFC 3339 date-time
//! inside a string (`<time>`); shot UUIDs are replaced by
//! `uuid#N`, numbered in the order the scenario first holds them (line
//! order, then shot order, after each step); the temp directory is written
//! as `<tmp>`; `merged_with` / `closures_with` and station lists are sorted.
//! Raw survey values in `project.json` and the exports are not rounded.
//!
//! Synthetic data only: every survey is built here, no fixture is read. The
//! outputs are NOT committed. `#[ignore]`d, run on demand:
//!
//! ```text
//! STALAGMIGHT_CHAR_OUT=/some/dir cargo test -p stalagmight-core --lib \
//!     characterization_dump -- --ignored --nocapture
//! ```
//!
//! `STALAGMIGHT_CHAR_ONLY=name,name` runs only the named scenarios. A step
//! whose outcome differs from its expectation, a scenario that panics, or a
//! full undo/redo round trip that does not come back identical is listed in
//! `mismatches.json` and fails the test once everything is written. Compare
//! two output directories with
//! `node scripts/dev/compare-characterization.mjs <old> <new>` (`--canon`
//! across the station-graph switch). The same scenarios also run in the
//! normal test run, into a temporary directory, as a gate on those three
//! failure kinds (`every_characterisation_scenario_does_what_it_expects`).
//!
//! The scenario functions only speak `Command`, the `AppState` recipes and
//! positional handles (`StationRef` slots, line ids, shots by position); they
//! are the 0.6.0 scripts unchanged. The query half reads what the IPC
//! handlers send on the station graph: `get_project_state` is
//! `AppState::project_state_wire` (the wire `Project` without
//! `line_offsets`, the per-slot stations, stats, frozen shots), the previews
//! go through the same wire mappings as their handlers
//! (`delete_shot_impact`, `delete_range_impact`, `line_relationships`, the
//! `AppState` previews), and the exports through `export::survey` (the
//! image from `AppState::export_views`, as `export_image` draws it).

use std::collections::BTreeMap;
use std::io::Read as _;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Serialize;
use serde_json::{json, Map, Value};
use tempfile::TempDir;

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::export::{ColorMode, ExportImageOptions, ImageFormat};
use crate::models::survey::wire::ProjectStateWire;
use crate::models::survey::Project;
use crate::models::{
    ClosureStrategy, CombineLinesOptions, CombinePick, Coordinates, CreditEdit, CreditTargets,
    GpsCoordinates, LineId, LineUpdates, Person, ShotData, ShotId, ShotUpdates, SplitAt,
    SplitLineOptions, SplitSide, Station, StationRef, SurveyCredit, SurveyDirection,
};

// ---------------------------------------------------------------------------
// Synthetic survey data
// ---------------------------------------------------------------------------

/// Where the synthetic cave sits: the first entrance's GPS position.
const BASE_LAT: f64 = 20.5;
const BASE_LON: f64 = -87.25;
/// Metres per degree of latitude, for placing synthetic GPS fixes near a
/// surveyed station. Only needs to be deterministic, not geodetically exact.
const M_PER_DEG_LAT: f64 = 111_320.0;

fn person(name: &str) -> Person {
    Person::named(name).expect("a non-empty name is a person")
}

fn day(d: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(2025, 3, d)
}

/// A shot: the two compass readings differ by one degree (so the mean is
/// exercised), LRUD set (so reversing with a swap shows), no credit.
fn leg(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: (azimuth + 1.0) % 360.0,
        length,
        depth_in,
        depth_out,
        left: Some(1.0),
        right: Some(2.0),
        up: Some(0.5),
        down: Some(1.5),
        ..Default::default()
    }
}

/// `n` shots on one bearing, `length` long, starting at `depth` and dropping
/// `step` metres per shot.
fn run(n: usize, azimuth: f64, length: f64, depth: f64, step: f64) -> Vec<ShotData> {
    (0..n)
        .map(|i| {
            let d0 = depth + step * i as f64;
            leg(azimuth, length, d0, d0 + step)
        })
        .collect()
}

/// `shots` with every shot credited to `surveyor` on `date`.
fn credited(mut shots: Vec<ShotData>, surveyor: &str, date: u32) -> Vec<ShotData> {
    let credit = std::sync::Arc::new(SurveyCredit {
        surveyors: vec![person(surveyor)],
        explorers: Vec::new(),
        date: day(date),
    });
    for shot in &mut shots {
        shot.credit = Some(std::sync::Arc::clone(&credit));
    }
    shots
}

/// A GPS position `east_m` / `north_m` metres from the base entrance (flat
/// approximation; deterministic).
fn gps_at(east_m: f64, north_m: f64) -> GpsCoordinates {
    let lat = BASE_LAT + north_m / M_PER_DEG_LAT;
    let lon = BASE_LON + east_m / (M_PER_DEG_LAT * BASE_LAT.to_radians().cos());
    GpsCoordinates::new(lat, lon, 0.0).expect("valid synthetic gps")
}

fn split_options(
    name_a: &str,
    name_b: &str,
    comment: SplitSide,
    datum: SplitSide,
) -> SplitLineOptions {
    SplitLineOptions {
        name_a: name_a.to_string(),
        name_b: name_b.to_string(),
        color_a: "#aa3300".to_string(),
        color_b: "#0033aa".to_string(),
        comment_side: comment,
        datum_side: datum,
    }
}

fn combine_options(name: &str, comment_pick: CombinePick) -> CombineLinesOptions {
    CombineLinesOptions {
        name: name.to_string(),
        color: "#227744".to_string(),
        swap_lr: true,
        comment_pick,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

// ---------------------------------------------------------------------------
// Canonical, masked JSON
// ---------------------------------------------------------------------------

/// Keys whose values are wall-clock times: masked wherever they appear.
const MASKED_KEYS: &[&str] = &[
    "created_at",
    "modified_at",
    "merged_at",
    "exported_at",
    "resolved_at",
    "timestamp",
];
const MASK: &str = "<masked>";

/// Round a derived float to 1e-9 so last-bit noise does not show as a
/// difference, and write `-0` as `0`.
fn round9(x: f64) -> f64 {
    let rounded = (x * 1e9).round() / 1e9;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// `8-4-4-4-12` hex digits.
fn looks_like_uuid(bytes: &[u8]) -> bool {
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(k, b)| {
            if matches!(k, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

/// The length of the RFC 3339 date-time starting at `bytes[0]`
/// (`YYYY-MM-DDTHH:MM:SS`, optional fraction, optional `Z` or `+HH:MM`), or
/// 0. Survey dates (`YYYY-MM-DD`) do not match.
fn datetime_len(bytes: &[u8]) -> usize {
    const SHAPE: &[u8] = b"dddd-dd-ddTdd:dd:dd";
    if bytes.len() < SHAPE.len()
        || !SHAPE.iter().zip(bytes).all(|(want, b)| match want {
            b'd' => b.is_ascii_digit(),
            other => b == other,
        })
    {
        return 0;
    }
    let mut n = SHAPE.len();
    if bytes.get(n) == Some(&b'.') {
        n += 1;
        while bytes.get(n).is_some_and(u8::is_ascii_digit) {
            n += 1;
        }
    }
    match bytes.get(n) {
        Some(b'Z') => n + 1,
        Some(b'+' | b'-')
            if bytes.len() >= n + 6
                && bytes[n + 1..n + 3].iter().all(u8::is_ascii_digit)
                && bytes[n + 3] == b':'
                && bytes[n + 4..n + 6].iter().all(u8::is_ascii_digit) =>
        {
            n + 6
        }
        _ => n,
    }
}

/// Wall-clock times inside strings (an Add .stalx import hash carries the
/// source's creation time) written as `<time>`.
fn mask_datetimes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut last) = (0, 0);
    while i < bytes.len() {
        let n = datetime_len(&bytes[i..]);
        if n > 0 {
            // A match is ASCII throughout, so both ends are char boundaries.
            out.push_str(&text[last..i]);
            out.push_str("<time>");
            i += n;
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Shot UUIDs are minted at random; outputs name them `uuid#N` instead,
/// numbered in the order the scenario first holds them.
#[derive(Default)]
struct UuidBook {
    ids: BTreeMap<String, usize>,
}

impl UuidBook {
    fn token(&mut self, uuid: &str) -> String {
        let next = self.ids.len() + 1;
        let n = *self.ids.entry(uuid.to_ascii_lowercase()).or_insert(next);
        format!("uuid#{n}")
    }

    /// Number every shot UUID of `project` not seen yet, in line order then
    /// shot order.
    fn register(&mut self, project: &Project) {
        for line in project.lines() {
            for shot in &line.shots {
                self.token(&shot.uuid);
            }
        }
    }

    fn mask_text(&mut self, text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let (mut i, mut last) = (0, 0);
        while i + 36 <= bytes.len() {
            if looks_like_uuid(&bytes[i..i + 36]) {
                // Every byte of a match is ASCII, so both ends are char
                // boundaries.
                out.push_str(&text[last..i]);
                out.push_str(&self.token(&text[i..i + 36]));
                i += 36;
                last = i;
            } else {
                i += 1;
            }
        }
        out.push_str(&text[last..]);
        out
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// What a step expected of the build. A step that does not get it is listed
/// in `mismatches.json`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    /// Succeeds and pushes exactly one undo entry.
    Ok,
    /// Is refused and changes nothing (no undo entry).
    Refused,
    /// An undo or redo step: succeeds; the stacks move by one.
    Step,
}

/// Writes one scenario's files and keeps the masking state.
struct Recorder {
    dir: PathBuf,
    tmp: PathBuf,
    tmp_text: String,
    scenario: String,
    step: usize,
    uuids: UuidBook,
    steps: Vec<Value>,
    mismatches: Vec<String>,
}

impl Recorder {
    fn new(root: &Path, tmp: &Path, scenario: &str) -> Self {
        let dir = root.join(scenario);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("clear the scenario directory");
        }
        std::fs::create_dir_all(&dir).expect("create the scenario directory");
        let tmp = tmp.join(scenario);
        std::fs::create_dir_all(&tmp).expect("create the scenario temp directory");
        Self {
            dir,
            tmp_text: tmp.display().to_string(),
            tmp,
            scenario: scenario.to_string(),
            step: 0,
            uuids: UuidBook::default(),
            steps: Vec::new(),
            mismatches: Vec::new(),
        }
    }

    fn mask_string(&mut self, text: &str) -> String {
        let text = if self.tmp_text.is_empty() {
            text.to_string()
        } else {
            text.replace(&self.tmp_text, "<tmp>")
        };
        self.uuids.mask_text(&mask_datetimes(&text))
    }

    /// Sorted keys, masked times / UUIDs / paths, and (when `round`) floats
    /// rounded to 1e-9.
    fn finish(&mut self, value: Value, round: bool) -> Value {
        match value {
            Value::Object(map) => {
                let mut entries: Vec<(String, Value)> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                let mut out = Map::new();
                for (key, item) in entries {
                    let item = if MASKED_KEYS.contains(&key.as_str()) && !item.is_null() {
                        Value::String(MASK.to_string())
                    } else {
                        self.finish(item, round)
                    };
                    out.insert(key, item);
                }
                Value::Object(out)
            }
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| self.finish(v, round)).collect())
            }
            Value::String(text) => Value::String(self.mask_string(&text)),
            Value::Number(n) if round && n.is_f64() => {
                n.as_f64().map_or(Value::Number(n), |x| json!(round9(x)))
            }
            other => other,
        }
    }

    fn name(&self, slug: &str, what: &str) -> String {
        format!("{:02}-{slug}-{what}", self.step)
    }

    fn write_value(&mut self, file: &str, value: Value, round: bool) {
        let value = self.finish(value, round);
        let mut text = serde_json::to_string_pretty(&value).expect("serialize output");
        text.push('\n');
        std::fs::write(self.dir.join(file), text).expect("write output");
    }

    fn write_text(&mut self, file: &str, text: &str) {
        let text = self.mask_string(text);
        std::fs::write(self.dir.join(file), text).expect("write output");
    }

    fn mismatch(&mut self, what: String) {
        self.mismatches.push(format!("{}: {what}", self.scenario));
    }
}

fn to_value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).expect("serialize to JSON")
}

fn result_value<T: Serialize>(result: Result<T, String>) -> Value {
    match result {
        Ok(value) => json!({ "ok": to_value(&value) }),
        Err(error) => json!({ "error": error }),
    }
}

// ---------------------------------------------------------------------------
// Queries: everything read from the build, as the IPC handlers read it.
// ---------------------------------------------------------------------------

/// Stations in slot order, with their relation lists sorted, so the
/// comparison does not depend on map iteration order.
fn sorted_stations(mut stations: Vec<Station>) -> Vec<Station> {
    for station in &mut stations {
        station.merged_with.sort();
        if let Some(closures) = station.closures_with.as_mut() {
            closures.sort_by_key(|c| (c.closure_id, c.station_ref, c.via_station));
        }
    }
    stations.sort_by_key(|s| s.station_ref);
    stations
}

/// The `get_project_state` payload (`get_project_state_impl` in the app
/// crate is `AppState::project_state_wire`).
fn project_state(state: &AppState) -> ProjectStateWire {
    state.project_state_wire().expect("a project is open")
}

/// The derived half of `get_project_state`: per-slot stations, stats (with
/// penetration and the depth range), frozen shots.
fn query_stations(wire: &ProjectStateWire) -> Value {
    json!({
        "stations": to_value(&sorted_stations(wire.stations.clone())),
        "stats": to_value(&wire.stats),
        "frozen_shots": to_value(&wire.frozen_shots),
    })
}

fn query_adjusted(state: &AppState) -> Value {
    json!({
        "stations": to_value(&state.get_adjusted_stations().map(sorted_stations)),
        "report": to_value(&state.get_adjustment_report()),
    })
}

/// Closures (with their metrics and `is_aligned`), merges and datums in wire
/// form, plus each datum's derived role.
fn query_closures(project: &Project, wire: &Value) -> Value {
    json!({
        "loop_closures": wire.get("loop_closures").cloned().unwrap_or(Value::Null),
        "station_merges": wire.get("station_merges").cloned().unwrap_or(Value::Null),
        "zero_datums": wire.get("zero_datums").cloned().unwrap_or(Value::Null),
        "datum_roles": to_value(&project.datum_roles()),
    })
}

/// `get_project_warnings` (`open_project_warnings`) and the integrity check.
fn query_warnings(state: &AppState) -> Value {
    json!({
        "warnings": to_value(&state.open_project_warnings()),
        "integrity": to_value(&state.integrity_check()),
    })
}

fn query_history(state: &AppState) -> Value {
    json!({
        "undo": to_value(&state.get_undo_stack()),
        "redo": to_value(&state.get_redo_stack()),
        "can_undo": state.can_undo(),
        "can_redo": state.can_redo(),
    })
}

fn undo_depth(state: &AppState) -> usize {
    state.get_undo_stack().len()
}

fn redo_depth(state: &AppState) -> usize {
    state.get_redo_stack().len()
}

/// Every per-step document, unmasked.
struct Snapshot {
    history: Value,
    stations: Value,
    adjusted: Value,
    closures: Value,
    project: Value,
    warnings: Value,
}

fn snapshot(state: &AppState) -> Snapshot {
    let project = state.get_project_state().expect("a project is open");
    let wire_state = project_state(state);
    let wire = to_value(&wire_state.project);
    Snapshot {
        history: query_history(state),
        stations: query_stations(&wire_state),
        adjusted: query_adjusted(state),
        closures: query_closures(&project, &wire),
        project: wire,
        warnings: query_warnings(state),
    }
}

/// One read-only preview, as its IPC handler answers it.
enum Preview {
    MergeConnectivity(StationRef, StationRef),
    DeleteShot(LineId, ShotId),
    DeleteShotRange(LineId, ShotId, ShotId),
    CombineCandidates(StationRef),
    CombineForLines(LineId, LineId),
    SplitLine(LineId, SplitAt, SplitLineOptions),
    ZeroDatum(StationRef, GpsCoordinates),
    LineRelationships(LineId),
}

impl Preview {
    fn kind(&self) -> &'static str {
        match self {
            Self::MergeConnectivity(..) => "merge-connectivity",
            Self::DeleteShot(..) => "delete-shot",
            Self::DeleteShotRange(..) => "delete-shot-range",
            Self::CombineCandidates(..) => "combine-candidates",
            Self::CombineForLines(..) => "combine-for-lines",
            Self::SplitLine(..) => "split-line",
            Self::ZeroDatum(..) => "zero-datum",
            Self::LineRelationships(..) => "line-relationships",
        }
    }

    fn query(&self, state: &AppState) -> Value {
        let project = || {
            state
                .get_project_state()
                .ok_or_else(|| "No project open".to_string())
        };
        // Each arm answers as its IPC handler does (`preview_delete_shot_impl`,
        // `preview_delete_shot_range_impl`, `get_line_relationships_impl` in
        // the app crate; the rest are `AppState` methods).
        match self {
            Self::MergeConnectivity(a, b) => result_value(state.check_merge_connectivity(*a, *b)),
            Self::DeleteShot(line, shot) => result_value(project().and_then(|p| {
                let preview = p
                    .preview_delete_shot(*line, *shot)
                    .map_err(|e| e.to_string())?;
                Ok(p.delete_shot_impact(&preview))
            })),
            Self::DeleteShotRange(line, from, to) => result_value(project().and_then(|p| {
                let preview = p
                    .preview_delete_shot_range(*line, *from, *to)
                    .map_err(|e| e.to_string())?;
                Ok(p.delete_range_impact(&preview))
            })),
            Self::CombineCandidates(station) => result_value(state.combine_candidates(*station)),
            Self::CombineForLines(a, b) => result_value(state.combine_candidate_for_lines(*a, *b)),
            Self::SplitLine(line, at, options) => {
                result_value(state.split_line_preview(*line, *at, options))
            }
            Self::ZeroDatum(station, gps) => result_value(state.preview_zero_datum(*station, *gps)),
            Self::LineRelationships(line) => result_value(project().and_then(|p| {
                if p.line(*line).is_none() {
                    return Err(crate::models::LINE_GONE.to_string());
                }
                Ok(p.line_relationships(*line))
            })),
        }
    }

    /// The arguments, for the record.
    fn args(&self) -> Value {
        match self {
            Self::MergeConnectivity(a, b) => json!({ "station_a": a, "station_b": b }),
            Self::DeleteShot(line, shot) => json!({ "line_id": line, "shot_id": shot }),
            Self::DeleteShotRange(line, from, to) => {
                json!({ "line_id": line, "start_shot_id": from, "end_shot_id": to })
            }
            Self::CombineCandidates(station) => json!({ "station": station }),
            Self::CombineForLines(a, b) => json!({ "a": a, "b": b }),
            Self::SplitLine(line, at, options) => {
                json!({ "line_id": line, "at": at, "options": options })
            }
            Self::ZeroDatum(station, gps) => json!({ "station_ref": station, "coordinates": gps }),
            Self::LineRelationships(line) => json!({ "line_id": line }),
        }
    }
}

/// The exports of the current project, as the export commands write them
/// (`export::survey`; the map image from the memoised Computed View and raw
/// layout of `AppState::export_views`, as `export_image` draws it).
fn query_exports(state: &AppState, rec: &mut Recorder) {
    let project = state.get_project_state().expect("a project is open");
    rec.uuids.register(&project);
    let tmp = rec.tmp.join("exports");
    std::fs::create_dir_all(&tmp).expect("create the export temp directory");

    let csv = tmp.join("export.csv");
    match crate::export::survey::export_csv(&project, &csv) {
        Ok(()) => {
            let text = std::fs::read_to_string(&csv).expect("read the CSV export");
            rec.write_text("final-export.csv", &text);
        }
        Err(e) => rec.write_value(
            "final-export-csv-error.json",
            json!(format!("{e:#}")),
            false,
        ),
    }

    let json_path = tmp.join("export.json");
    match crate::export::survey::export_json(&project, &json_path) {
        Ok(()) => {
            let text = std::fs::read_to_string(&json_path).expect("read the JSON export");
            let value: Value = serde_json::from_str(&text).expect("the JSON export parses");
            rec.write_value("final-export.json", value, false);
        }
        Err(e) => rec.write_value(
            "final-export-json-error.json",
            json!(format!("{e:#}")),
            false,
        ),
    }

    let tmlu = tmp.join("export.tmlu");
    match crate::export::survey::export_tml(&project, &tmlu) {
        Ok(notes) => {
            let text = std::fs::read_to_string(&tmlu).expect("read the TMLU export");
            rec.write_text("final-export.tmlu", &text);
            rec.write_value("final-export-tml-notes.json", to_value(&notes), false);
        }
        Err(e) => rec.write_value(
            "final-export-tmlu-error.json",
            json!(format!("{e:#}")),
            false,
        ),
    }

    let tml = tmp.join("export.tml");
    match crate::export::survey::export_tml(&project, &tml) {
        Ok(_) => {
            let file = std::fs::File::open(&tml).expect("open the TML export");
            let mut zip = zip::ZipArchive::new(file).expect("the TML export is a zip");
            let mut entries = Vec::new();
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i).expect("read a TML zip entry");
                let mut text = String::new();
                entry
                    .read_to_string(&mut text)
                    .expect("a TML zip entry is text");
                entries.push((entry.name().to_string(), text));
            }
            entries.sort();
            let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
            rec.write_value("final-export-tml-entries.json", to_value(&names), false);
            for (name, text) in &entries {
                let file = if name == "Data.xml" {
                    "final-export.tml.xml".to_string()
                } else {
                    format!("final-export.tml.{}", name.replace('/', "_"))
                };
                rec.write_text(&file, text);
            }
        }
        Err(e) => rec.write_value(
            "final-export-tml-error.json",
            json!(format!("{e:#}")),
            false,
        ),
    }

    let views = state.export_views().expect("a project is open");
    for (file, ids) in [
        ("final-export-station-ids.svg", true),
        ("final-export-station-names.svg", false),
    ] {
        let options = ExportImageOptions {
            format: ImageFormat::Svg,
            dpi: 96,
            color_mode: ColorMode::Line,
            show_station_labels: ids,
            show_station_names: !ids,
            show_line_names: true,
            show_station_comments: true,
            show_shot_comments: true,
            show_shot_data: false,
            include_reference_legs: state.get_config().gas.include_reference_legs,
            ..Default::default()
        };
        match crate::export::survey::generate_svg_string_with_positions(
            &views.project,
            &options,
            views.adjusted.as_ref(),
            &views.raw,
        ) {
            Ok(svg) => rec.write_text(file, &svg),
            Err(e) => rec.write_value(
                &format!("{file}-error.json"),
                json!(format!("{e:#}")),
                false,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------

/// Runs one scenario's steps and records after each. Scenario functions use
/// only its action methods and lookups.
struct Script {
    state: AppState,
    rec: Recorder,
}

impl Script {
    fn new(root: &Path, tmp: &Path, scenario: &str) -> Self {
        let state = AppState::new().expect("app state");
        let created = state.execute(Command::CreateProject {
            name: format!("Characterisation {scenario}"),
            location: Some("Synthetic".to_string()),
        });
        assert!(created.success, "create project: {:?}", created.error);
        Self {
            state,
            rec: Recorder::new(root, tmp, scenario),
        }
    }

    // --- Lookups ------------------------------------------------------------

    fn project(&self) -> Project {
        self.state.get_project_state().expect("a project is open")
    }

    fn line_named(&self, name: &str) -> LineId {
        self.project()
            .lines()
            .iter()
            .find(|l| l.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no line named {name}"))
            .id
    }

    fn shots(&self, line: LineId) -> Vec<ShotId> {
        self.project()
            .line(line)
            .unwrap_or_else(|| panic!("line {line} exists"))
            .shots
            .iter()
            .map(|s| s.id)
            .collect()
    }

    /// The shot at `index` of `line`.
    fn shot(&self, line: LineId, index: usize) -> ShotId {
        *self
            .shots(line)
            .get(index)
            .unwrap_or_else(|| panic!("line {line} has a shot {index}"))
    }

    fn last_shot(&self, line: LineId) -> ShotId {
        *self
            .shots(line)
            .last()
            .unwrap_or_else(|| panic!("line {line} has shots"))
    }

    /// The station at the far end of `line`.
    fn end(&self, line: LineId) -> StationRef {
        StationRef::new(line, self.shots(line).len() as u32)
    }

    /// The measured (raw) position of a slot, as `get_project_state` sends it.
    fn coords(&self, station: StationRef) -> Coordinates {
        project_state(&self.state)
            .stations
            .into_iter()
            .find(|s| s.station_ref == station)
            .unwrap_or_else(|| panic!("station {station} exists"))
            .coordinates
    }

    /// The horizontal offset that lands `from` on `to` (what a snap drag
    /// sends).
    fn offset_onto(&self, from: StationRef, to: StationRef) -> Coordinates {
        let (a, b) = (self.coords(from), self.coords(to));
        Coordinates::new(b.x - a.x, b.y - a.y, 0.0)
    }

    // --- Actions ------------------------------------------------------------

    /// Run one user action, check it against `expect`, record everything.
    fn act(
        &mut self,
        slug: &str,
        expect: Expect,
        action: impl FnOnce(&AppState) -> CommandResult,
    ) -> CommandResult {
        self.rec.step += 1;
        let (undo_before, redo_before) = (undo_depth(&self.state), redo_depth(&self.state));
        let result = action(&self.state);
        let (undo_after, redo_after) = (undo_depth(&self.state), redo_depth(&self.state));
        let undo_change = undo_after as i64 - undo_before as i64;
        let redo_change = redo_after as i64 - redo_before as i64;
        let met = match expect {
            Expect::Ok => result.success && undo_change == 1,
            Expect::Refused => !result.success && undo_change == 0 && redo_change == 0,
            Expect::Step => result.success && undo_change.abs() == 1 && redo_change.abs() == 1,
        };
        if !met {
            let what = format!(
                "step {:02}-{slug}: expected {expect:?}, got success={} undo {undo_change:+} \
                 redo {redo_change:+} error={:?}",
                self.rec.step, result.success, result.error
            );
            self.rec.mismatch(what);
        }
        let snap = snapshot(&self.state);
        self.rec.uuids.register(&self.project());
        let record = json!({
            "step": slug,
            "expect": format!("{expect:?}"),
            "expectation_met": met,
            "undo_depth_change": undo_change,
            "redo_depth_change": redo_change,
            "result": to_value(&result),
        });
        let mut files = Vec::new();
        for (what, value, round) in [
            ("result", record, true),
            ("history", snap.history, false),
            ("stations", snap.stations, true),
            ("adjusted", snap.adjusted, true),
            ("closures", snap.closures, true),
            ("project", snap.project, false),
            ("warnings", snap.warnings, true),
        ] {
            let file = format!("{}.json", self.rec.name(slug, what));
            self.rec.write_value(&file, value, round);
            files.push(file);
        }
        let index_entry = json!({
            "step": format!("{:02}-{slug}", self.rec.step),
            "kind": "action",
            "expect": format!("{expect:?}"),
            "success": result.success,
            "files": files,
        });
        self.rec.steps.push(index_entry);
        result
    }

    fn exec(&mut self, slug: &str, cmd: Command) -> CommandResult {
        self.act(slug, Expect::Ok, |state| state.execute(cmd))
    }

    fn refused(&mut self, slug: &str, cmd: Command) -> CommandResult {
        self.act(slug, Expect::Refused, |state| state.execute(cmd))
    }

    fn undo(&mut self, slug: &str) {
        self.act(slug, Expect::Step, AppState::undo);
    }

    fn redo(&mut self, slug: &str) {
        self.act(slug, Expect::Step, AppState::redo);
    }

    fn preview(&mut self, slug: &str, preview: &Preview) {
        self.rec.step += 1;
        self.rec.uuids.register(&self.project());
        let value = json!({
            "preview": preview.kind(),
            "args": preview.args(),
            "answer": preview.query(&self.state),
        });
        let file = format!("{}.json", self.rec.name(slug, preview.kind()));
        self.rec.write_value(&file, value, true);
        let index_entry = json!({
            "step": format!("{:02}-{slug}", self.rec.step),
            "kind": "preview",
            "files": [file],
        });
        self.rec.steps.push(index_entry);
    }

    fn create_line(&mut self, name: &str, shots: Vec<ShotData>) -> LineId {
        let slug = format!("create-{}", name.to_ascii_lowercase().replace(' ', "-"));
        let result = self.exec(
            &slug,
            Command::CreateLine {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#336699".to_string(),
                direction: Some(SurveyDirection::In),
                shots,
                cave_section_id: None,
            },
        );
        result.line_id.expect("CreateLine returns the new line id")
    }

    /// Merge `a` onto `b` (the piece of `a` moves, as in the merge dialog),
    /// with the connectivity check the map runs first.
    fn merge(&mut self, slug: &str, a: StationRef, b: StationRef) -> CommandResult {
        self.preview(&format!("check-{slug}"), &Preview::MergeConnectivity(a, b));
        self.exec(
            slug,
            Command::MergeStations {
                station_a: a,
                station_b: b,
            },
        )
    }

    fn datum(
        &mut self,
        slug: &str,
        at: StationRef,
        gps: GpsCoordinates,
        accuracy: Option<f64>,
        entrance: Option<bool>,
    ) {
        self.preview(&format!("preview-{slug}"), &Preview::ZeroDatum(at, gps));
        self.exec(
            slug,
            Command::SetZeroDatum {
                station_ref: at,
                coordinates: gps,
                accuracy_m: accuracy,
                entrance,
            },
        );
    }

    /// Undo everything, redo everything, and record whether the project came
    /// back as it was.
    fn roundtrip(&mut self) {
        self.rec.step += 1;
        let before = snapshot(&self.state);
        let mut undone = 0_usize;
        let mut undo_errors = Vec::new();
        while self.state.can_undo() && undone < 500 {
            let r = self.state.undo();
            if !r.success {
                undo_errors.push(r.error);
                break;
            }
            undone += 1;
        }
        let bottom_lines = self.project().lines().len();
        let mut redone = 0_usize;
        let mut redo_errors = Vec::new();
        while self.state.can_redo() && redone < 500 {
            let r = self.state.redo();
            if !r.success {
                redo_errors.push(r.error);
                break;
            }
            redone += 1;
        }
        let after = snapshot(&self.state);
        let mut differing = Vec::new();
        for (what, a, b) in [
            ("history", &before.history, &after.history),
            ("stations", &before.stations, &after.stations),
            ("adjusted", &before.adjusted, &after.adjusted),
            ("closures", &before.closures, &after.closures),
            ("project", &before.project, &after.project),
            ("warnings", &before.warnings, &after.warnings),
        ] {
            let mut x = self.rec.finish(a.clone(), true);
            let mut y = self.rec.finish(b.clone(), true);
            if what == "history" {
                // Redo keeps each entry's label but the stack is rebuilt.
                x = x.get("undo").cloned().unwrap_or(Value::Null);
                y = y.get("undo").cloned().unwrap_or(Value::Null);
            }
            if x != y {
                differing.push(what);
            }
        }
        if !undo_errors.is_empty() || !redo_errors.is_empty() || undone != redone {
            self.rec.mismatch(format!(
                "roundtrip: undone {undone}, redone {redone}, undo errors {undo_errors:?}, \
                 redo errors {redo_errors:?}"
            ));
        }
        if !differing.is_empty() {
            self.rec.mismatch(format!(
                "roundtrip: {differing:?} differ after undoing and redoing everything"
            ));
        }
        let value = json!({
            "undone": undone,
            "redone": redone,
            "lines_at_bottom": bottom_lines,
            "undo_errors": undo_errors,
            "redo_errors": redo_errors,
            "differs_after_roundtrip": differing,
        });
        let file = format!("{}.json", self.rec.name("roundtrip", "result"));
        self.rec.write_value(&file, value, false);
        let index_entry = json!({
            "step": format!("{:02}-roundtrip", self.rec.step),
            "kind": "roundtrip",
            "files": [file],
        });
        self.rec.steps.push(index_entry);
    }

    fn finish(mut self) -> Vec<String> {
        self.roundtrip();
        query_exports(&self.state, &mut self.rec);
        // Stream X's hook: the final state as a version 2 `.stalx` plus the
        // uuid book and the export settings, so a scenario's end state can be
        // opened, or exported and masked alike, by hand. The 0.6.0 build wrote
        // a SQLite file here for X's `golden_exports` runner, retired once
        // this dump exported through `export::survey` itself.
        if let Ok(dir) = std::env::var("STALAGMIGHT_CHAR_SAVE") {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).expect("create the save directory");
            let saved = self.state.execute(Command::SaveProject {
                path: Some(dir.join(format!("{}.stalx", self.rec.scenario))),
            });
            assert!(saved.success, "save the final state: {:?}", saved.error);
            let book: BTreeMap<String, usize> = self.rec.uuids.ids.clone();
            let config = self.state.get_config();
            let meta = json!({
                "uuids": book,
                "include_reference_legs": config.gas.include_reference_legs,
                "closure_adjustment": to_value(&config.closure_adjustment),
            });
            std::fs::write(
                dir.join(format!("{}.uuids.json", self.rec.scenario)),
                serde_json::to_string_pretty(&meta).expect("serialize the uuid book"),
            )
            .expect("write the uuid book");
        }
        let steps = Value::Array(std::mem::take(&mut self.rec.steps));
        self.rec.write_value("steps.json", steps, false);
        self.rec.mismatches
    }
}

// ---------------------------------------------------------------------------
// Scenarios: commands only (plus positional lookups)
// ---------------------------------------------------------------------------

/// Three lines meeting at one station (a hub), a branch off a mid-line
/// station (with two shots the validator flags), a merge refused because the
/// two stations are already connected.
fn junction_merges(s: &mut Script) {
    let trunk = s.create_line("Trunk", run(4, 90.0, 10.0, 5.0, 1.0));
    let north = s.create_line("North", run(3, 0.0, 8.0, 9.0, 0.5));
    let south = s.create_line("South", run(3, 180.0, 6.0, 9.0, 0.5));
    let hub = s.end(trunk);
    s.merge("merge-north-onto-hub", StationRef::new(north, 0), hub);
    s.merge("merge-south-onto-hub", StationRef::new(south, 0), hub);
    let (north_end, south_end) = (s.end(north), s.end(south));
    s.preview(
        "check-connected-ends",
        &Preview::MergeConnectivity(north_end, south_end),
    );
    s.refused(
        "merge-connected-ends",
        Command::MergeStations {
            station_a: north_end,
            station_b: south_end,
        },
    );
    // Two shots the validator flags: compass readings 35 degrees apart, and
    // a shot past the length limit.
    let spur = s.create_line(
        "Spur",
        vec![
            ShotData {
                azimuth_out: 80.0,
                ..leg(45.0, 5.0, 6.0, 6.5)
            },
            leg(45.0, 55.0, 6.5, 7.0),
        ],
    );
    s.merge(
        "merge-spur-onto-trunk-2",
        StationRef::new(spur, 0),
        StationRef::new(trunk, 2),
    );
    s.exec(
        "comment-hub",
        Command::SetStationComment {
            station_ref: hub,
            comment: Some("Hub".to_string()),
        },
    );
    s.preview("combine-at-hub", &Preview::CombineCandidates(hub));
    s.preview(
        "combine-north-south",
        &Preview::CombineForLines(north, south),
    );
    let last = s.last_shot(trunk);
    s.preview("delete-trunk-last", &Preview::DeleteShot(trunk, last));
    s.preview("relationships-trunk", &Preview::LineRelationships(trunk));
    s.undo("undo");
    s.redo("redo");
}

/// A loop closed by a Batch closure; the closure's stored metrics after a
/// later edit; delete-shot preview of the closure's station.
fn loop_batch_closure(s: &mut Script) {
    let east = s.create_line("Loop East", run(3, 90.0, 10.0, 5.0, 0.5));
    let north = s.create_line(
        "Loop North",
        vec![
            leg(0.0, 10.0, 5.0, 6.0),
            leg(90.0, 10.0, 6.0, 6.5),
            leg(180.0, 9.5, 6.5, 5.5),
        ],
    );
    s.merge(
        "merge-north-start",
        StationRef::new(north, 0),
        StationRef::new(east, 0),
    );
    let (north_end, east_1) = (s.end(north), StationRef::new(east, 1));
    s.preview(
        "check-loop-ends",
        &Preview::MergeConnectivity(north_end, east_1),
    );
    s.exec(
        "close-loop",
        Command::CreateLoopClosure {
            station_a: north_end,
            station_b: east_1,
            closure_strategy: ClosureStrategy::Batch,
        },
    );
    let edited = s.shot(north, 2);
    s.exec(
        "lengthen-closing-shot",
        Command::UpdateShot {
            line_id: north,
            shot_id: edited,
            updates: ShotUpdates {
                length: Some(9.8),
                ..Default::default()
            },
        },
    );
    s.refused(
        "close-loop-again",
        Command::CreateLoopClosure {
            station_a: north_end,
            station_b: east_1,
            closure_strategy: ClosureStrategy::Batch,
        },
    );
    let last = s.last_shot(north);
    s.preview("delete-closing-shot", &Preview::DeleteShot(north, last));
    let closure = s
        .project()
        .closures
        .first()
        .expect("the loop has a closure")
        .id;
    s.exec(
        "remove-closure",
        Command::RemoveLoopClosure {
            closure_id: closure,
        },
    );
    s.undo("undo");
    s.redo("redo");
}

/// A station snapped onto another (Move closure, aligned), then knocked out
/// of alignment by a shot edit; a free line snapped and merged (snap-to-merge
/// of a segment drag).
fn snap_move_closure(s: &mut Script) {
    let east = s.create_line("Snap East", run(3, 90.0, 10.0, 5.0, 0.5));
    let north = s.create_line(
        "Snap North",
        vec![
            leg(0.0, 10.0, 5.0, 6.0),
            leg(90.0, 10.0, 6.0, 6.5),
            leg(180.0, 9.5, 6.5, 5.5),
        ],
    );
    s.merge(
        "merge-north-start",
        StationRef::new(north, 0),
        StationRef::new(east, 0),
    );
    let (north_end, east_1) = (s.end(north), StationRef::new(east, 1));
    s.preview("check-snap", &Preview::MergeConnectivity(north_end, east_1));
    let offset = s.offset_onto(north_end, east_1);
    s.exec(
        "snap-station-move-closure",
        Command::Batch {
            commands: vec![
                Command::MoveStation {
                    station_ref: north_end,
                    offset,
                },
                Command::CreateLoopClosure {
                    station_a: north_end,
                    station_b: east_1,
                    closure_strategy: ClosureStrategy::Move,
                },
            ],
            label: None,
        },
    );
    let first = s.shot(north, 0);
    s.exec(
        "edit-shot-under-snap",
        Command::UpdateShot {
            line_id: north,
            shot_id: first,
            updates: ShotUpdates {
                length: Some(10.3),
                ..Default::default()
            },
        },
    );
    let free = s.create_line("Snap Free", run(2, 45.0, 6.0, 7.0, 0.5));
    let (free_0, east_end) = (StationRef::new(free, 0), s.end(east));
    s.preview(
        "check-segment-snap",
        &Preview::MergeConnectivity(free_0, east_end),
    );
    let offset = s.offset_onto(free_0, east_end);
    s.exec(
        "snap-segment-merge",
        Command::Batch {
            commands: vec![
                Command::MoveSegment {
                    line_id: free,
                    offset,
                },
                Command::MergeStations {
                    station_a: free_0,
                    station_b: east_end,
                },
            ],
            label: None,
        },
    );
    s.undo("undo");
    s.redo("redo");
}

/// One survey with two entrances: the first datum places it, the second is
/// a GPS fix; removing the first promotes the fix.
fn two_entrances(s: &mut Script) {
    let first = s.create_line("Entrance Line", run(3, 90.0, 12.0, 0.0, 2.0));
    let second = s.create_line("Second Way", run(3, 0.0, 10.0, 6.0, -2.0));
    s.merge("merge-second-way", StationRef::new(second, 0), s.end(first));
    s.datum(
        "datum-first-entrance",
        StationRef::new(first, 0),
        gps_at(0.0, 0.0),
        Some(3.0),
        Some(true),
    );
    let second_end = s.end(second);
    let fix = {
        let c = s.coords(second_end);
        gps_at(c.x + 1.5, c.y + 2.0)
    };
    s.datum(
        "gps-fix-second-entrance",
        second_end,
        fix,
        Some(5.0),
        Some(true),
    );
    s.exec(
        "fix-accuracy-edit",
        Command::SetZeroDatum {
            station_ref: second_end,
            coordinates: fix,
            accuracy_m: Some(1.0),
            entrance: None,
        },
    );
    s.exec(
        "remove-first-datum",
        Command::RemoveZeroDatum {
            station_ref: StationRef::new(first, 0),
        },
    );
    s.undo("undo");
    s.redo("redo");
}

/// A free piece moved with Move Segment and Move Line, a refused move of a
/// datum piece, then the free piece snapped onto the anchored one.
fn free_piece_move_segment(s: &mut Script) {
    let anchored = s.create_line("Anchored", run(3, 90.0, 10.0, 3.0, 1.0));
    s.datum(
        "datum-anchored",
        StationRef::new(anchored, 0),
        gps_at(0.0, 0.0),
        None,
        None,
    );
    let one = s.create_line("Free One", run(2, 30.0, 8.0, 4.0, 1.0));
    let two = s.create_line("Free Two", run(2, 120.0, 6.0, 6.0, 0.5));
    s.merge("merge-free-pair", StationRef::new(two, 0), s.end(one));
    s.exec(
        "move-segment-free",
        Command::MoveSegment {
            line_id: one,
            offset: Coordinates::new(15.0, -20.0, 0.0),
        },
    );
    s.exec(
        "move-line-free",
        Command::MoveLine {
            line_id: two,
            offset: Coordinates::new(1.0, 2.0, 0.0),
        },
    );
    s.refused(
        "move-segment-anchored",
        Command::MoveSegment {
            line_id: anchored,
            offset: Coordinates::new(5.0, 5.0, 0.0),
        },
    );
    let (one_0, anchored_end) = (StationRef::new(one, 0), s.end(anchored));
    s.preview(
        "check-snap",
        &Preview::MergeConnectivity(one_0, anchored_end),
    );
    let offset = s.offset_onto(one_0, anchored_end);
    s.exec(
        "snap-free-piece-onto-anchored",
        Command::Batch {
            commands: vec![
                Command::MoveSegment {
                    line_id: one,
                    offset,
                },
                Command::MergeStations {
                    station_a: one_0,
                    station_b: anchored_end,
                },
            ],
            label: None,
        },
    );
    s.refused(
        "move-segment-now-anchored",
        Command::MoveSegment {
            line_id: one,
            offset: Coordinates::new(2.0, 0.0, 0.0),
        },
    );
    s.undo("undo");
    s.redo("redo");
}

/// Reverse a branch (swapping L/R), the datum line, then both at once.
fn reverse_line(s: &mut Script) {
    let forward = s.create_line(
        "Forward",
        vec![
            leg(80.0, 9.0, 2.0, 4.0),
            leg(95.0, 11.0, 4.0, 5.5),
            leg(110.0, 7.5, 5.5, 5.0),
            leg(60.0, 12.0, 5.0, 8.0),
        ],
    );
    s.datum(
        "datum-forward",
        StationRef::new(forward, 0),
        gps_at(0.0, 0.0),
        None,
        Some(true),
    );
    s.exec(
        "comment-forward-1",
        Command::SetStationComment {
            station_ref: StationRef::new(forward, 1),
            comment: Some("Low bedding".to_string()),
        },
    );
    s.exec(
        "name-forward-2",
        Command::SetStationName {
            station_ref: StationRef::new(forward, 2),
            name: "Pillar".to_string(),
        },
    );
    let side = s.create_line("Side", run(2, 10.0, 6.0, 5.5, 0.5));
    s.merge(
        "merge-side",
        StationRef::new(side, 0),
        StationRef::new(forward, 2),
    );
    let (side_end, forward_end) = (s.end(side), s.end(forward));
    s.exec(
        "close-side-to-forward-end",
        Command::CreateLoopClosure {
            station_a: side_end,
            station_b: forward_end,
            closure_strategy: ClosureStrategy::Batch,
        },
    );
    s.exec(
        "reverse-side-swap-lr",
        Command::ReverseLine {
            line_id: side,
            swap_left_right: true,
        },
    );
    s.exec(
        "reverse-forward",
        Command::ReverseLine {
            line_id: forward,
            swap_left_right: false,
        },
    );
    s.exec(
        "reverse-both-batch",
        Command::Batch {
            commands: vec![
                Command::ReverseLine {
                    line_id: forward,
                    swap_left_right: true,
                },
                Command::ReverseLine {
                    line_id: side,
                    swap_left_right: false,
                },
            ],
            label: None,
        },
    );
    s.undo("undo");
    s.redo("redo");
}

/// Split Line Here at a station carrying a branch, a comment and a name,
/// with a closure across the cut (the branch still links the halves); then a
/// cut along a shot of the second half, after which the closure is the only
/// link between the pieces.
fn split_line_here(s: &mut Script) {
    let main = s.create_line("Main", run(5, 90.0, 10.0, 3.0, 1.0));
    s.datum(
        "datum-main",
        StationRef::new(main, 0),
        gps_at(0.0, 0.0),
        None,
        None,
    );
    let at = StationRef::new(main, 2);
    s.exec(
        "comment-split-station",
        Command::SetStationComment {
            station_ref: at,
            comment: Some("Split here".to_string()),
        },
    );
    s.exec(
        "name-split-station",
        Command::SetStationName {
            station_ref: at,
            name: "SP".to_string(),
        },
    );
    let branch = s.create_line("Branch", run(2, 0.0, 6.0, 5.0, 0.5));
    s.merge("merge-branch-at-split", StationRef::new(branch, 0), at);
    let parallel = s.create_line(
        "Parallel",
        vec![
            leg(0.0, 8.0, 4.0, 5.0),
            leg(90.0, 30.0, 5.0, 6.0),
            leg(180.0, 7.6, 6.0, 7.0),
        ],
    );
    s.merge(
        "merge-parallel-start",
        StationRef::new(parallel, 0),
        StationRef::new(main, 1),
    );
    let (parallel_end, main_4) = (s.end(parallel), StationRef::new(main, 4));
    s.exec(
        "close-parallel",
        Command::CreateLoopClosure {
            station_a: parallel_end,
            station_b: main_4,
            closure_strategy: ClosureStrategy::Batch,
        },
    );
    let options = split_options("Main A", "Main B", SplitSide::Both, SplitSide::A);
    let cut = SplitAt::Station { station_index: 2 };
    s.preview(
        "preview-split-at-station",
        &Preview::SplitLine(main, cut, options.clone()),
    );
    s.act("split-at-station", Expect::Ok, |state| {
        state.execute_split_line(main, cut, options)
    });
    let second = s.line_named("Main B");
    let cut = SplitAt::Shot {
        shot_id: s.shot(second, 1),
        distance: 4.0,
    };
    let options = split_options("Main B1", "Main B2", SplitSide::B, SplitSide::B);
    s.preview(
        "preview-split-along-shot",
        &Preview::SplitLine(second, cut, options.clone()),
    );
    s.act("split-along-shot", Expect::Ok, |state| {
        state.execute_split_line(second, cut, options)
    });
    s.undo("undo");
    s.redo("redo");
}

/// Combine across a merged join (a branch stays at the join, the comments
/// differ), then a combine that has to reverse the other line.
fn combine_merged_join(s: &mut Script) {
    let up = s.create_line("Upstream", run(3, 90.0, 10.0, 4.0, 1.0));
    let down = s.create_line("Downstream", run(3, 100.0, 8.0, 7.0, 0.5));
    let join = s.end(up);
    s.merge("merge-downstream", StationRef::new(down, 0), join);
    s.exec(
        "comment-up-end",
        Command::SetStationComment {
            station_ref: join,
            comment: Some("Upstream end".to_string()),
        },
    );
    s.exec(
        "comment-down-start",
        Command::SetStationComment {
            station_ref: StationRef::new(down, 0),
            comment: Some("Downstream start".to_string()),
        },
    );
    let tributary = s.create_line("Tributary", run(2, 0.0, 5.0, 7.0, 0.5));
    s.merge("merge-tributary", StationRef::new(tributary, 0), join);
    s.preview("combine-at-join", &Preview::CombineCandidates(join));
    s.preview("combine-up-down", &Preview::CombineForLines(up, down));
    s.act("combine-up-down", Expect::Ok, |state| {
        state.execute_combine_lines(up, down, combine_options("Combined", CombinePick::Both))
    });
    let back = s.create_line("Reverse Me", run(2, 270.0, 6.0, 9.0, -0.5));
    let combined_end = s.end(up);
    s.merge("merge-reverse-me-end", s.end(back), combined_end);
    s.preview(
        "combine-combined-reverse-me",
        &Preview::CombineForLines(up, back),
    );
    s.act("combine-with-reverse", Expect::Ok, |state| {
        state.execute_combine_lines(
            up,
            back,
            combine_options("Combined Again", CombinePick::Kept),
        )
    });
    s.undo("undo");
    s.redo("redo");
}

/// Combine across a side-by-side (co-located, unmerged) join: two lines laid
/// end to start with Move Line, and the two halves a split leaves.
fn combine_side_by_side(s: &mut Script) {
    let left = s.create_line("Left Half", run(3, 90.0, 10.0, 4.0, 1.0));
    let right = s.create_line("Right Half", run(3, 80.0, 9.0, 7.0, 0.5));
    let (left_end, right_0) = (s.end(left), StationRef::new(right, 0));
    let offset = s.offset_onto(right_0, left_end);
    s.exec(
        "lay-right-at-left-end",
        Command::MoveLine {
            line_id: right,
            offset,
        },
    );
    s.preview("combine-at-left-end", &Preview::CombineCandidates(left_end));
    s.preview("combine-left-right", &Preview::CombineForLines(left, right));
    s.act("combine-left-right", Expect::Ok, |state| {
        state.execute_combine_lines(
            left,
            right,
            combine_options("Laid Together", CombinePick::Both),
        )
    });
    let whole = s.create_line("Whole", run(4, 45.0, 7.0, 3.0, 0.5));
    s.act("split-whole", Expect::Ok, |state| {
        state.execute_split_line(
            whole,
            SplitAt::Station { station_index: 2 },
            split_options("Whole A", "Whole B", SplitSide::A, SplitSide::A),
        )
    });
    let (a, b) = (s.line_named("Whole A"), s.line_named("Whole B"));
    s.preview("combine-split-halves", &Preview::CombineForLines(a, b));
    s.act("combine-split-halves", Expect::Ok, |state| {
        state.execute_combine_lines(a, b, combine_options("Whole Again", CombinePick::Kept))
    });
    s.undo("undo");
    s.redo("redo");
}

/// Combine two lines whose ends are tied by a loop closure (the two lines
/// also meet through a third, so the combined line closes a loop).
fn combine_across_closure(s: &mut Script) {
    let base = s.create_line("Base", run(2, 0.0, 10.0, 5.0, 0.0));
    let lower = s.create_line("East Arm", run(3, 90.0, 10.0, 5.0, 0.5));
    let upper = s.create_line(
        "Upper Arm",
        vec![
            leg(90.0, 10.0, 5.0, 5.5),
            leg(90.0, 10.0, 5.5, 6.0),
            leg(90.0, 10.0, 6.0, 6.5),
            leg(180.0, 19.5, 6.5, 6.5),
        ],
    );
    s.merge(
        "merge-east-arm",
        StationRef::new(lower, 0),
        StationRef::new(base, 0),
    );
    s.merge("merge-upper-arm", StationRef::new(upper, 0), s.end(base));
    let (lower_end, upper_end) = (s.end(lower), s.end(upper));
    s.exec(
        "close-arms",
        Command::CreateLoopClosure {
            station_a: upper_end,
            station_b: lower_end,
            closure_strategy: ClosureStrategy::Batch,
        },
    );
    s.preview(
        "combine-at-lower-end",
        &Preview::CombineCandidates(lower_end),
    );
    s.preview("combine-arms", &Preview::CombineForLines(lower, upper));
    s.act("combine-arms", Expect::Ok, |state| {
        state.execute_combine_lines(lower, upper, combine_options("Arms", CombinePick::Both))
    });
    s.undo("undo");
    s.redo("redo");
}

/// Delete shots whose station is a junction (a hub of three lines, a branch
/// start), delete-and-split across a branch, and delete the last shot of a
/// one-shot line at the hub.
fn delete_shot_junction(s: &mut Script) {
    let trunk = s.create_line("Trunk", run(4, 90.0, 10.0, 4.0, 1.0));
    let one = s.create_line("Spoke One", run(2, 0.0, 7.0, 8.0, 0.5));
    let two = s.create_line("Spoke Two", run(2, 180.0, 7.0, 8.0, 0.5));
    let stub = s.create_line("Stub", run(1, 135.0, 4.0, 8.0, 0.5));
    let hub = s.end(trunk);
    s.merge("merge-spoke-one", StationRef::new(one, 0), hub);
    s.merge("merge-spoke-two", StationRef::new(two, 0), hub);
    s.merge("merge-stub", StationRef::new(stub, 0), hub);
    let branch = s.create_line("Branch", run(2, 45.0, 6.0, 5.0, 0.5));
    s.merge(
        "merge-branch-at-trunk-1",
        StationRef::new(branch, 0),
        StationRef::new(trunk, 1),
    );
    let last = s.last_shot(trunk);
    s.preview("preview-delete-hub-shot", &Preview::DeleteShot(trunk, last));
    s.exec(
        "delete-hub-shot",
        Command::DeleteShot {
            line_id: trunk,
            shot_id: last,
        },
    );
    let first = s.shot(one, 0);
    s.preview(
        "preview-delete-spoke-start",
        &Preview::DeleteShot(one, first),
    );
    s.exec(
        "delete-spoke-start",
        Command::DeleteShot {
            line_id: one,
            shot_id: first,
        },
    );
    let first = s.shot(branch, 0);
    s.preview(
        "preview-delete-branch-start",
        &Preview::DeleteShot(branch, first),
    );
    s.exec(
        "delete-branch-start",
        Command::DeleteShot {
            line_id: branch,
            shot_id: first,
        },
    );
    let long = s.create_line("Long", run(4, 270.0, 6.0, 4.0, 0.5));
    let side = s.create_line("Long Side", run(2, 200.0, 5.0, 5.0, 0.5));
    s.merge(
        "merge-long-side",
        StationRef::new(side, 0),
        StationRef::new(long, 2),
    );
    let cut = s.shot(long, 1);
    s.preview("preview-delete-and-split", &Preview::DeleteShot(long, cut));
    s.act("delete-and-split", Expect::Ok, |state| {
        state.execute_delete_shot_and_split(long, cut, "Long A".to_string(), "Long B".to_string())
    });
    let only = s.shot(stub, 0);
    s.preview("preview-delete-stub", &Preview::DeleteShot(stub, only));
    s.act("delete-last-shot-and-line", Expect::Ok, |state| {
        state.execute_delete_last_shot_and_line(stub, only)
    });
    s.undo("undo");
    s.redo("redo");
}

/// Range deletes: an interior gap-close across a branch and a closure end,
/// an interior split, a prefix and a suffix.
fn delete_shot_range(s: &mut Script) {
    let line = s.create_line("Range Line", run(6, 90.0, 8.0, 3.0, 0.5));
    let branch = s.create_line("Range Branch", run(2, 0.0, 5.0, 4.5, 0.5));
    s.merge(
        "merge-branch-at-3",
        StationRef::new(branch, 0),
        StationRef::new(line, 3),
    );
    let looped = s.create_line(
        "Range Loop",
        vec![
            leg(180.0, 6.0, 3.5, 4.0),
            leg(90.0, 32.0, 4.0, 5.0),
            leg(0.0, 5.7, 5.0, 6.0),
        ],
    );
    s.merge(
        "merge-loop-start",
        StationRef::new(looped, 0),
        StationRef::new(line, 1),
    );
    let (loop_end, line_5) = (s.end(looped), StationRef::new(line, 5));
    s.exec(
        "close-loop",
        Command::CreateLoopClosure {
            station_a: loop_end,
            station_b: line_5,
            closure_strategy: ClosureStrategy::Batch,
        },
    );
    let (from, to) = (s.shot(line, 2), s.shot(line, 3));
    s.preview(
        "preview-interior-range",
        &Preview::DeleteShotRange(line, from, to),
    );
    s.act("delete-interior-gap-close", Expect::Ok, |state| {
        state.execute_delete_shot_range_shift(line, from, to)
    });
    let split = s.create_line("Split Range", run(7, 300.0, 6.0, 4.0, 0.5));
    let side = s.create_line("Split Side", run(2, 250.0, 4.0, 5.0, 0.5));
    s.merge(
        "merge-split-side",
        StationRef::new(side, 0),
        StationRef::new(split, 2),
    );
    let (from, to) = (s.shot(split, 1), s.shot(split, 2));
    s.preview(
        "preview-interior-split",
        &Preview::DeleteShotRange(split, from, to),
    );
    s.act("delete-interior-split", Expect::Ok, |state| {
        state.execute_delete_shot_range_split(
            split,
            from,
            to,
            "Split Range A".to_string(),
            "Split Range B".to_string(),
        )
    });
    let (from, to) = (s.shot(line, 0), s.shot(line, 1));
    s.preview("preview-prefix", &Preview::DeleteShotRange(line, from, to));
    s.act("delete-prefix", Expect::Ok, |state| {
        state.execute_delete_shot_range_shift(line, from, to)
    });
    let tail = s.line_named("Split Range B");
    let n = s.shots(tail).len();
    let (from, to) = (s.shot(tail, n - 2), s.shot(tail, n - 1));
    s.preview("preview-suffix", &Preview::DeleteShotRange(tail, from, to));
    s.act("delete-suffix", Expect::Ok, |state| {
        state.execute_delete_shot_range_shift(tail, from, to)
    });
    s.undo("undo");
    s.redo("redo");
}

/// Unmerge a pair, a whole two-member station, one spoke of a hub of three
/// (pair and whole station), and the hub's centre.
fn unmerge(s: &mut Script) {
    let pair = s.create_line("Pair", run(2, 90.0, 6.0, 3.0, 0.5));
    let partner = s.create_line("Pair Partner", run(2, 30.0, 6.0, 4.0, 0.5));
    let pair_end = s.end(pair);
    let partner_0 = StationRef::new(partner, 0);
    s.merge("merge-pair", partner_0, pair_end);
    s.exec(
        "unmerge-pair",
        Command::UnmergeStationPair {
            station_a: pair_end,
            station_b: partner_0,
        },
    );
    s.merge("merge-pair-again", partner_0, pair_end);
    s.exec(
        "unmerge-whole-two-member-station",
        Command::UnmergeStations {
            station_ref: partner_0,
        },
    );
    let hub_line = s.create_line("Hub Line", run(3, 180.0, 8.0, 3.0, 1.0));
    let one = s.create_line("Spoke One", run(2, 90.0, 6.0, 6.0, 0.5));
    let two = s.create_line("Spoke Two", run(2, 270.0, 6.0, 6.0, 0.5));
    let hub = s.end(hub_line);
    let (one_0, two_0) = (StationRef::new(one, 0), StationRef::new(two, 0));
    s.merge("merge-spoke-one", one_0, hub);
    s.merge("merge-spoke-two", two_0, hub);
    s.exec(
        "unmerge-hub-pair",
        Command::UnmergeStationPair {
            station_a: hub,
            station_b: one_0,
        },
    );
    s.merge("merge-spoke-one-again", one_0, hub);
    s.exec(
        "unmerge-hub-spoke-station",
        Command::UnmergeStations { station_ref: two_0 },
    );
    s.merge("merge-spoke-two-again", two_0, hub);
    s.exec(
        "unmerge-hub-centre",
        Command::UnmergeStations { station_ref: hub },
    );
    s.undo("undo");
    s.redo("redo");
}

/// Two pieces each placed by a datum; the frame (first datum) is removed so
/// the frame changes, set again, the new frame's GPS edited, and the two
/// placed pieces merged.
fn datum_frame_change(s: &mut Script) {
    let frame = s.create_line("Frame Line", run(3, 90.0, 10.0, 2.0, 1.0));
    let far = s.create_line("Far Piece", run(3, 0.0, 8.0, 3.0, 0.5));
    let (frame_0, far_0) = (StationRef::new(frame, 0), StationRef::new(far, 0));
    s.datum("datum-frame", frame_0, gps_at(0.0, 0.0), None, Some(true));
    s.datum(
        "datum-far",
        far_0,
        gps_at(200.0, 50.0),
        Some(4.0),
        Some(true),
    );
    s.preview(
        "preview-remove-frame",
        &Preview::ZeroDatum(frame_0, gps_at(0.0, 0.0)),
    );
    s.exec(
        "remove-frame-datum",
        Command::RemoveZeroDatum {
            station_ref: frame_0,
        },
    );
    s.datum("datum-frame-again", frame_0, gps_at(3.0, -2.0), None, None);
    s.datum("edit-far-gps", far_0, gps_at(205.0, 48.0), Some(4.0), None);
    let frame_end = s.end(frame);
    s.merge("merge-two-placed-pieces", frame_end, far_0);
    s.exec(
        "remove-far-datum",
        Command::RemoveZeroDatum { station_ref: far_0 },
    );
    s.undo("undo");
    s.redo("redo");
}

/// Add a second synthetic `.stalx` (built and saved through its own
/// `AppState`): a datum piece with a merge, a comment and a closure, and a
/// free line. Added twice, so the second copy's names are made unique.
fn add_stalx(s: &mut Script) {
    let home = s.create_line("Home Line", run(3, 90.0, 10.0, 2.0, 1.0));
    s.datum(
        "datum-home",
        StationRef::new(home, 0),
        gps_at(0.0, 0.0),
        None,
        Some(true),
    );
    let branch = s.create_line("Home Branch", run(2, 0.0, 6.0, 5.0, 0.5));
    s.merge("merge-home-branch", StationRef::new(branch, 0), s.end(home));

    let source = s.rec.tmp.join("second-project.stalx");
    {
        let other = AppState::new().expect("app state");
        let ok = |r: CommandResult, what: &str| {
            assert!(r.success, "second project, {what}: {:?}", r.error);
            r
        };
        ok(
            other.execute(Command::CreateProject {
                name: "Second Survey".to_string(),
                location: Some("Elsewhere".to_string()),
            }),
            "create",
        );
        let create = |name: &str, shots: Vec<ShotData>| {
            ok(
                other.execute(Command::CreateLine {
                    name: Some(name.to_string()),
                    surveyor: None,
                    date: None,
                    color: "#aa5500".to_string(),
                    direction: Some(SurveyDirection::Out),
                    shots,
                    cave_section_id: None,
                }),
                name,
            )
            .line_id
            .expect("line id")
        };
        let one = create(
            "Added One",
            credited(run(3, 120.0, 7.0, 3.0, 1.0), "Adder", 7),
        );
        let two = create("Added Two", run(2, 200.0, 5.0, 6.0, 0.5));
        let free = create("Added Free", run(2, 45.0, 4.0, 2.0, 0.5));
        ok(
            other.execute(Command::MergeStations {
                station_a: StationRef::new(two, 0),
                station_b: StationRef::new(one, 3),
            }),
            "merge",
        );
        ok(
            other.execute(Command::SetStationComment {
                station_ref: StationRef::new(one, 1),
                comment: Some("added comment".to_string()),
            }),
            "comment",
        );
        ok(
            other.execute(Command::SetZeroDatum {
                station_ref: StationRef::new(one, 0),
                coordinates: gps_at(60.0, 40.0),
                accuracy_m: None,
                entrance: Some(true),
            }),
            "datum",
        );
        ok(
            other.execute(Command::CreateLoopClosure {
                station_a: StationRef::new(two, 2),
                station_b: StationRef::new(one, 1),
                closure_strategy: ClosureStrategy::Batch,
            }),
            "closure",
        );
        ok(
            other.execute(Command::MoveSegment {
                line_id: free,
                offset: Coordinates::new(-10.0, 12.0, 0.0),
            }),
            "move free",
        );
        ok(
            other.execute(Command::SaveProject {
                path: Some(source.clone()),
            }),
            "save",
        );
    }
    let add = Command::add_project_file(source.clone());
    s.exec("add-stalx", add);
    s.exec("add-stalx-again", Command::add_project_file(source));
    s.undo("undo");
    s.redo("redo");
}

/// A data-table paste of 1,000 cells (LRUD of 250 shots) as one batch.
fn paste_batch(s: &mut Script) {
    let commands = (0..10)
        .map(|i| Command::CreateLine {
            name: Some(format!("Grid {:02}", i + 1)),
            surveyor: None,
            date: None,
            color: "#556677".to_string(),
            direction: Some(SurveyDirection::In),
            shots: run(25, 10.0 * f64::from(i), 4.0, 2.0, 0.1),
            cave_section_id: None,
        })
        .collect();
    s.exec(
        "create-ten-lines",
        Command::Batch {
            commands,
            label: None,
        },
    );
    let mut commands = Vec::new();
    for line in s.project().lines().iter().map(|l| l.id) {
        for (k, shot) in s.shots(line).into_iter().enumerate() {
            let v = 0.25 + (commands.len() % 7) as f64 * 0.5 + k as f64 * 0.01;
            commands.push(Command::UpdateShot {
                line_id: line,
                shot_id: shot,
                updates: ShotUpdates {
                    left: Some(Some(v)),
                    right: Some(Some(v + 1.0)),
                    up: Some(Some(v / 2.0)),
                    down: Some(Some(v + 2.0)),
                    ..Default::default()
                },
            });
        }
    }
    assert_eq!(commands.len() * 4, 1000, "1,000 pasted cells");
    s.exec(
        "paste-1000-cells",
        Command::Batch {
            commands,
            label: None,
        },
    );
    s.undo("undo");
    s.redo("redo");
}

/// Station renames: a plain station, a merged station, a refused duplicate,
/// a whitespace-trimmed name, and a batch rename.
fn station_renames(s: &mut Script) {
    let named = s.create_line("Named Line", run(4, 90.0, 8.0, 3.0, 0.5));
    let other = s.create_line("Other Named", run(2, 0.0, 6.0, 5.0, 0.5));
    s.merge(
        "merge-other",
        StationRef::new(other, 0),
        StationRef::new(named, 2),
    );
    let rename = |station: StationRef, name: &str| Command::SetStationName {
        station_ref: station,
        name: name.to_string(),
    };
    s.exec(
        "rename-1",
        rename(StationRef::new(named, 1), "Entrance Pool"),
    );
    s.exec(
        "rename-junction",
        rename(StationRef::new(named, 2), "Junction"),
    );
    s.exec(
        "rename-junction-partner",
        rename(StationRef::new(other, 0), "Junction B"),
    );
    s.refused(
        "rename-duplicate",
        rename(StationRef::new(named, 3), "Entrance Pool"),
    );
    s.exec(
        "rename-trimmed",
        rename(StationRef::new(named, 3), "  Spaced Out  "),
    );
    s.exec(
        "rename-batch",
        Command::Batch {
            commands: vec![
                rename(StationRef::new(named, 4), "T4"),
                rename(StationRef::new(other, 1), "Side 1"),
                rename(StationRef::new(other, 2), "Side 2"),
            ],
            label: None,
        },
    );
    s.undo("undo");
    s.redo("redo");
}

/// Credit edits on a line and on chosen shots, line metadata, and the Edit
/// Line dialog's batch (metadata and credit together).
fn credit_edits(s: &mut Script) {
    let first = s.create_line(
        "Credit Line",
        credited(run(3, 90.0, 8.0, 3.0, 0.5), "Alice", 1),
    );
    let second = s.create_line("Credit Two", run(2, 180.0, 6.0, 4.0, 0.5));
    s.exec(
        "credit-line-add-surveyor",
        Command::EditShotCredit {
            targets: CreditTargets::Line { line_id: first },
            edit: CreditEdit {
                add_surveyors: Some(vec![person("Bob")]),
                set_date: Some(day(4)),
                ..Default::default()
            },
        },
    );
    let shots = vec![(first, s.shot(first, 0)), (second, s.shot(second, 1))];
    s.exec(
        "credit-shots",
        Command::EditShotCredit {
            targets: CreditTargets::Shots { shots },
            edit: CreditEdit {
                set_surveyors: Some(vec![person("Carol")]),
                add_explorers: Some(vec![person("Dan")]),
                ..Default::default()
            },
        },
    );
    s.exec(
        "credit-line-explorers",
        Command::EditShotCredit {
            targets: CreditTargets::Line { line_id: second },
            edit: CreditEdit {
                set_explorers: Some(vec![person("Eve"), person("Finn")]),
                remove_surveyors: Some(vec![person("Carol")]),
                ..Default::default()
            },
        },
    );
    s.exec(
        "line-metadata",
        Command::UpdateLineMetadata {
            line_id: first,
            updates: LineUpdates {
                name: Some(Some("Credit Line Renamed".to_string())),
                color: Some("#123456".to_string()),
                direction: Some(SurveyDirection::Out),
                quality: Some(0.8),
                ..Default::default()
            },
        },
    );
    s.exec(
        "edit-line-save-batch",
        Command::Batch {
            commands: vec![
                Command::UpdateLineMetadata {
                    line_id: second,
                    updates: LineUpdates {
                        name: Some(Some("Credit Two Renamed".to_string())),
                        ..Default::default()
                    },
                },
                Command::EditShotCredit {
                    targets: CreditTargets::Line { line_id: second },
                    edit: CreditEdit {
                        set_date: Some(None),
                        ..Default::default()
                    },
                },
            ],
            label: None,
        },
    );
    s.undo("undo");
    s.redo("redo");
}

type Scenario = fn(&mut Script);

const SCENARIOS: &[(&str, Scenario)] = &[
    ("junction-merges", junction_merges),
    ("loop-batch-closure", loop_batch_closure),
    ("snap-move-closure", snap_move_closure),
    ("two-entrances", two_entrances),
    ("free-piece-move-segment", free_piece_move_segment),
    ("reverse-line", reverse_line),
    ("split-line-here", split_line_here),
    ("combine-merged-join", combine_merged_join),
    ("combine-side-by-side", combine_side_by_side),
    ("combine-across-closure", combine_across_closure),
    ("delete-shot-junction", delete_shot_junction),
    ("delete-shot-range", delete_shot_range),
    ("unmerge", unmerge),
    ("datum-frame-change", datum_frame_change),
    ("add-stalx", add_stalx),
    ("paste-batch", paste_batch),
    ("station-renames", station_renames),
    ("credit-edits", credit_edits),
];

/// Run the scenarios (all, or the ones named in `only`) into `out`, write
/// `manifest.json` and `mismatches.json` there, and return the scenarios run
/// and the mismatches.
fn dump_scenarios(out: &Path, only: Option<&[String]>) -> (Vec<&'static str>, Vec<String>) {
    std::fs::create_dir_all(out).expect("create the output directory");
    let temp = TempDir::new().expect("temp dir");

    let mut ran = Vec::new();
    let mut mismatches = Vec::new();
    for (name, scenario) in SCENARIOS {
        if only.is_some_and(|only| !only.iter().any(|o| o == name)) {
            continue;
        }
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let mut script = Script::new(out, temp.path(), name);
            scenario(&mut script);
            script.finish()
        }));
        match outcome {
            Ok(found) => mismatches.extend(found),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_default();
                mismatches.push(format!("{name}: aborted: {message}"));
            }
        }
        ran.push(*name);
    }

    let manifest = json!({
        "format": 1,
        "scenarios": ran,
        "rounding": "derived floats rounded to 1e-9",
        "masked_keys": MASKED_KEYS,
        "datetimes": "RFC 3339 date-times inside strings written as <time>",
        "paths": "the run's temp directory written as <tmp>",
        "uuids": "shot UUIDs written as uuid#N in first-held order",
    });
    let mut text = serde_json::to_string_pretty(&manifest).expect("serialize manifest");
    text.push('\n');
    std::fs::write(out.join("manifest.json"), text).expect("write manifest");
    let mut text = serde_json::to_string_pretty(&mismatches).expect("serialize mismatches");
    text.push('\n');
    std::fs::write(out.join("mismatches.json"), text).expect("write mismatches");
    (ran, mismatches)
}

#[test]
#[ignore = "on demand: writes characterisation goldens to $STALAGMIGHT_CHAR_OUT"]
fn characterization_dump() {
    let out = PathBuf::from(
        std::env::var("STALAGMIGHT_CHAR_OUT")
            .expect("set STALAGMIGHT_CHAR_OUT to an output directory"),
    );
    let only: Option<Vec<String>> = std::env::var("STALAGMIGHT_CHAR_ONLY")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());
    let (ran, mismatches) = dump_scenarios(&out, only.as_deref());
    println!(
        "characterisation: {} scenarios -> {}",
        ran.len(),
        out.display()
    );
    assert!(
        mismatches.is_empty(),
        "steps that did not do what they expected:\n{}",
        mismatches.join("\n")
    );
}

/// The gate the dump leaves in the normal test run: every scenario's steps
/// do what they expect (each command succeeds or is refused as in 0.6.0 and
/// moves the undo stack by one, or not at all), no scenario panics, and
/// undoing and redoing everything gives back the same history, stations,
/// Computed View, closures, wire project and warnings (all 18 did on
/// 0.6.0). The output is compared with the goldens on demand
/// (`characterization_dump`, `scripts/dev/compare-characterization.mjs`).
#[test]
fn every_characterisation_scenario_does_what_it_expects() {
    let out = TempDir::new().expect("temp dir");
    let (ran, mismatches) = dump_scenarios(out.path(), None);
    assert_eq!(ran.len(), SCENARIOS.len());
    assert!(
        mismatches.is_empty(),
        "steps that did not do what they expected:\n{}",
        mismatches.join("\n")
    );
}
