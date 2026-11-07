//! DETERMINISM: every engine in this directory, replayed from the same seed,
//! has to produce the same bytes.
//!
//! # Why this exists
//!
//! Three defects were found late and by hand, each a `HashMap`'s iteration
//! order leaking into something the app hands out: the order of a station's
//! closures (#576 HFI-15), which of two colliding station comments survived
//! (#576 NF-W13BE-2) and the row order of a save (#576 NF-W13BE-3). The other
//! engines here compare two STATES or two ROUTES; none of them compares a run
//! with itself, so none of them could see any of the three. This module does,
//! for every engine at once.
//!
//! # How the hasher is varied
//!
//! std's `RandomState` draws its SipHash keys from the OS once per THREAD, and
//! bumps one of them for every map built on that thread. Each replay below runs
//! on a fresh thread, so every `HashMap`/`HashSet` the app builds in it has
//! keys the other replay never saw - and any output that depends on iteration
//! order comes out in another order. No product code is touched for this, and
//! nothing is assumed: [`hasher_order`] records each replay's iteration order
//! of one fixed 64-key set, and a run whose replays did not differ there fails
//! (the chance of two random keyings agreeing on 64 keys is about 1/64!).
//!
//! # What is compared
//!
//! A replay records a STREAM of stages. The engines call [`probe_move`] after
//! every move (a fuzz mutation, an undo/redo, a save, a reload, a metamorphic
//! route) and [`probe_full`] at the end of a run; each records a list of named
//! channels, each a byte string:
//!
//! * the live project as the snapshot oracle serializes it, and the IPC answers
//!   of the read commands the frontend issues after every edit (`get_project_state`
//!   in both its JSON and binary forms, adjusted stations, warnings, integrity,
//!   the undo and redo stacks);
//! * at the end of a run, also the read commands that take an argument (per
//!   section, per station), the gas reach field, every export (CSV, JSON, the
//!   TML document, SVG and PNG), a SAVE through the app's own pipeline - every
//!   table of the file row by row in rowid order, undo/redo history included, and
//!   the page bytes of every table that carries no wall-clock or random value -
//!   and the raw-data view of the saved file.
//!
//! Two replays must agree stage for stage, channel for channel, byte for byte.
//! Array order is compared, float bits are compared, JSON object key order is
//! compared. Three things are masked, and nothing else:
//!
//! * the WALL CLOCK: timestamp-shaped text becomes `<ts>`, and the binary
//!   channels are encoded from a copy of the project whose stamps are pinned
//!   ([`pinned_clock`]), because a length-changing mask would misalign them;
//! * RANDOM IDENTITIES: a v4 uuid is renamed to its order of first appearance in
//!   the replay's stream (`00000000-0000-0000-0000-00000000000k`, same length),
//!   so two replays that mint uuids for the same shots in the same order agree;
//! * TEMP PATHS: a path under the system temp dir becomes `<tmp>`.
//!
//! # Running it
//!
//! The gates below run a short fixed seed set in the normal `cargo test` job.
//! The sweep (`determinism_sweep`, ignored) takes `STALAGMIGHT_RUST_DET_SEEDS`,
//! `STALAGMIGHT_RUST_DET_ITERATIONS` and `STALAGMIGHT_RUST_DET_REPLAYS`; see
//! docs/testing-recipes.md, "Determinism".

#![expect(
    clippy::print_stdout,
    reason = "a gate's per-scenario summary IS its output, and `cargo test -- --nocapture` is the \
              only channel a test has"
)]
#![expect(
    clippy::panic,
    reason = "a determinism failure's payload (scenario, stage, channel, first difference) is the \
              repro, and a panic IS how a test fails"
)]
#![expect(
    clippy::expect_used,
    reason = "test module: a failed setup step fails the calling test"
)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde::Serialize;
use tempfile::TempDir;

use super::format_property_tests::{build_state, mutate, open_fresh};
use super::fuzz_undo_tests::{panic_message, random_shot, run_fuzz, Plan, Rng, COLORS};
use super::metamorphic_undo_tests::run_pairs_summary;
use super::AppState;
use crate::config::GasConfig;
use crate::core::Command;
use crate::export::{ExportImageOptions, ImageFormat};
use crate::models::{
    ClosureStrategy, CreationStamp, CylRole, Cylinder, Diver, Project, RulerEndpoint, Station,
    StationRef, TankMaterial,
};
use crate::test_helpers::isolated_config;

// ===========================================================================
// The probe: what one replay records
// ===========================================================================

/// One named output, as recorded.
struct Channel {
    name: String,
    bytes: Vec<u8>,
}

/// Everything one probe point recorded.
struct Stage {
    label: String,
    channels: Vec<Channel>,
}

/// A replay in progress, on its own thread.
struct Probe {
    stages: Vec<Stage>,
    masker: Masker,
    /// Exports and saves made by [`probe_full`] go here, so they never land
    /// next to a file a run is using.
    scratch: TempDir,
}

thread_local! {
    static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

fn armed() -> bool {
    PROBE.with(|probe| probe.borrow().is_some())
}

/// A channel before masking: text channels take every mask, binary channels
/// only the length-preserving uuid rename.
struct Raw {
    name: String,
    bytes: Vec<u8>,
    text: bool,
}

impl Raw {
    fn text(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bytes: text.into().into_bytes(),
            text: true,
        }
    }

    fn json<T: Serialize + ?Sized>(name: impl Into<String>, value: &T) -> Self {
        let text = serde_json::to_string(value)
            .unwrap_or_else(|error| format!("<does not serialize: {error}>"));
        Self::text(name, text)
    }

    fn binary(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            bytes,
            text: false,
        }
    }
}

fn record(label: &str, raws: Vec<Raw>) {
    PROBE.with(|probe| {
        let mut probe = probe.borrow_mut();
        let Some(probe) = probe.as_mut() else {
            return;
        };
        let channels = raws
            .into_iter()
            .map(|raw| Channel {
                bytes: probe.masker.mask(&raw.bytes, raw.text),
                name: raw.name,
            })
            .collect();
        probe.stages.push(Stage {
            label: label.to_string(),
            channels,
        });
    });
}

/// Record the per-move channels of `state`. A no-op unless a determinism replay
/// is running on this thread, so the engines call it unconditionally.
///
/// A `save` move also records the file it just wrote: the fuzz engine's
/// crossings save through the app's pipeline, and that file is a channel.
pub(super) fn probe_move(label: Option<&str>, state: &AppState) {
    if !armed() {
        return;
    }
    let label = label.unwrap_or("<unlabelled move>");
    let mut raws = light_channels(state);
    if label == "save" {
        if let Some(path) = state
            .get_project_state()
            .and_then(|project| project.file_path)
        {
            raws.extend(file_channels("file", &path));
        }
    }
    record(label, raws);
}

/// [`probe_move`] for a pair of twins (the metamorphic engine's two routes).
pub(super) fn probe_twins(pair: &str, left: &AppState, right: &AppState) {
    probe_move(Some(&format!("{pair} left")), left);
    probe_move(Some(&format!("{pair} right")), right);
}

/// Record EVERYTHING `state` answers: the per-move channels, the read commands
/// that take an argument, the gas reach field, every export, and a save
/// through the app's pipeline with every table of the file it writes.
///
/// It saves, so it is for the END of a run: the state's file path and dirty
/// flag change.
pub(super) fn probe_full(label: &str, state: &AppState) {
    if !armed() {
        return;
    }
    let scratch = PROBE.with(|probe| {
        probe.borrow().as_ref().map(|probe| {
            probe
                .scratch
                .path()
                .join(format!("stage{}", probe.stages.len()))
        })
    });
    let Some(scratch) = scratch else {
        return;
    };
    let mut raws = light_channels(state);
    if std::fs::create_dir_all(&scratch).is_ok() {
        raws.extend(argument_channels(state));
        raws.extend(export_channels(state, &scratch));
        raws.extend(save_channels(state, &scratch));
    } else {
        raws.push(Raw::text("scratch", "<could not create the scratch dir>"));
    }
    record(label, raws);
}

/// Record one channel directly. For scenarios that are not an engine (the
/// harness's own self-tests).
fn probe_value(label: &str, raw: Raw) {
    record(label, vec![raw]);
}

// ===========================================================================
// Masking
// ===========================================================================

/// Per-replay masks. See the module header for the three, and why only three.
struct Masker {
    /// Every uuid seen so far, by order of first appearance. A lookup table,
    /// never iterated.
    uuids: HashMap<String, usize>,
    temp_root: Vec<u8>,
}

impl Masker {
    fn new() -> Self {
        let root = std::env::temp_dir();
        let root = root.to_string_lossy();
        Self {
            uuids: HashMap::new(),
            temp_root: root.trim_end_matches('/').as_bytes().to_vec(),
        }
    }

    fn mask(&mut self, bytes: &[u8], text: bool) -> Vec<u8> {
        let renamed = self.rename_uuids(bytes);
        if text {
            mask_temp_paths(&mask_timestamps(&renamed), &self.temp_root)
        } else {
            renamed
        }
    }

    fn rename_uuids(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            if let Some(uuid) = bytes.get(at..at + 36).filter(|run| is_uuid(run)) {
                let key = String::from_utf8_lossy(uuid).to_ascii_lowercase();
                let next = self.uuids.len();
                let ordinal = *self.uuids.entry(key).or_insert(next);
                out.extend_from_slice(format!("00000000-0000-0000-0000-{ordinal:012x}").as_bytes());
                at += 36;
            } else {
                out.extend_from_slice(bytes.get(at..=at).unwrap_or_default());
                at += 1;
            }
        }
        out
    }
}

fn is_uuid(run: &[u8]) -> bool {
    run.len() == 36
        && run.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

/// Length of the timestamp starting at `bytes[0]`, if one does:
/// `YYYY-MM-DD[T ]HH:MM:SS`, then an optional fraction and an optional `Z` or
/// numeric offset. A bare date is survey data (a section's `date`) and is not
/// a timestamp.
fn timestamp_len(bytes: &[u8]) -> Option<usize> {
    const SHAPE: &[u8] = b"dddd-dd-dd?dd:dd:dd";
    let head = bytes.get(..SHAPE.len())?;
    let fits = head.iter().zip(SHAPE).all(|(byte, want)| match want {
        b'd' => byte.is_ascii_digit(),
        b'?' => matches!(byte, b'T' | b' '),
        other => byte == other,
    });
    if !fits {
        return None;
    }
    let mut len = SHAPE.len();
    if bytes.get(len) == Some(&b'.') {
        let digits = bytes
            .get(len + 1..)
            .unwrap_or_default()
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits > 0 {
            len += 1 + digits;
        }
    }
    match bytes.get(len) {
        Some(b'Z') => len += 1,
        Some(b'+' | b'-') => {
            let offset = bytes.get(len + 1..len + 6).unwrap_or_default();
            let numeric = |index: usize| offset.get(index).is_some_and(u8::is_ascii_digit);
            if numeric(0) && numeric(1) && offset.get(2) == Some(&b':') && numeric(3) && numeric(4)
            {
                len += 6;
            }
        }
        _ => {}
    }
    Some(len)
}

fn mask_timestamps(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if let Some(len) = timestamp_len(bytes.get(at..).unwrap_or_default()) {
            out.extend_from_slice(b"<ts>");
            at += len;
        } else {
            out.extend_from_slice(bytes.get(at..=at).unwrap_or_default());
            at += 1;
        }
    }
    out
}

/// `<temp root>/<component>` becomes `<tmp>`: the scratch dirs a run saves into
/// are named at random, and a saved project carries its path.
fn mask_temp_paths(bytes: &[u8], root: &[u8]) -> Vec<u8> {
    if root.is_empty() {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let rest = bytes.get(at..).unwrap_or_default();
        if rest.starts_with(root) && rest.get(root.len()) == Some(&b'/') {
            let component = rest
                .get(root.len() + 1..)
                .unwrap_or_default()
                .iter()
                .take_while(|byte| !matches!(byte, b'/' | b'"' | b'\'' | b'\\' | b' ' | b'<'))
                .count();
            out.extend_from_slice(b"<tmp>");
            at += root.len() + 1 + component;
        } else {
            out.extend_from_slice(rest.get(..1).unwrap_or_default());
            at += 1;
        }
    }
    out
}

// ===========================================================================
// The channels
// ===========================================================================

/// `Project` in the positional wire form every IPC struct sends it in.
struct IpcProject<'a>(&'a Project);

impl Serialize for IpcProject<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        crate::models::serialize_project_ipc(self.0, serializer)
    }
}

/// A copy of `project` with every wall-clock stamp pinned and its path dropped,
/// for the BINARY channels: a length-changing mask would misalign them, so the
/// clock is taken out at the source instead.
fn pinned_clock(project: &Project) -> Project {
    let epoch = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;
    let mut pinned = project.clone();
    pinned.created_at = CreationStamp::at(epoch);
    pinned.modified_at = epoch;
    pinned.file_path = None;
    for merge in pinned.station_merges.iter_mut() {
        merge.merged_at = epoch;
    }
    pinned
}

/// The measured stations and stats `get_project_state` sends (mirrors the app
/// crate's `measured_state`, commands/project.rs).
fn measured(state: &AppState, project: &Project) -> (Vec<Station>, crate::models::ProjectStats) {
    let stations = project.compute_station_coordinates();
    let mut stats = project.stats();
    let include_reference = state.get_config().gas.include_reference_legs;
    stats.penetration_distance = project.calculate_penetration_with(&stations, include_reference);
    let (min_depth, max_depth) = Project::depth_range(&stations);
    stats.min_depth = min_depth;
    stats.max_depth = max_depth;
    (stations, stats)
}

/// What the frontend reads after every edit. Cheap enough per move.
fn light_channels(state: &AppState) -> Vec<Raw> {
    let mut out = vec![Raw::text(
        "snapshot",
        state
            .capture_project_snapshot()
            .unwrap_or_else(|failed| format!("<no snapshot: {:?}>", failed.error)),
    )];
    let Some(project) = state.settled_snapshot() else {
        out.push(Raw::text("ipc.project_state", "<no project>"));
        return out;
    };
    let (stations, stats) = measured(state, &project);
    out.push(Raw::json(
        "ipc.project_state.project",
        &IpcProject(&project),
    ));
    out.push(Raw::json("ipc.project_state.stations", &stations));
    out.push(Raw::json("ipc.project_state.stats", &stats));
    out.push(Raw::json(
        "ipc.project_state.frozen_shots",
        &project.compute_frozen_shots(),
    ));
    out.push(
        match crate::core::load_binary::encode_refresh(&pinned_clock(&project), stations, stats, 0)
        {
            Ok(bytes) => Raw::binary("ipc.project_state_binary", bytes),
            Err(error) => Raw::text("ipc.project_state_binary", format!("<error: {error}>")),
        },
    );
    out.push(Raw::json(
        "ipc.adjusted_stations",
        &state.adjusted_stations_checked(),
    ));
    out.push(Raw::json(
        "ipc.project_warnings",
        &state.compute_project_warnings(&project, None),
    ));
    out.push(Raw::json("ipc.integrity", &state.integrity_check()));
    out.push(Raw::json("ipc.undo_stack", &state.get_undo_stack()));
    out.push(Raw::json("ipc.redo_stack", &state.get_redo_stack()));
    out
}

const fn endpoint(station: &Station) -> RulerEndpoint {
    RulerEndpoint {
        section_id: station.station_ref.section_id,
        station_index: Some(station.station_ref.station_index),
        shot_id: None,
        t: None,
    }
}

/// The read commands that take an argument, each asked once per section or
/// for the first and last station.
fn argument_channels(state: &AppState) -> Vec<Raw> {
    let mut out = Vec::new();
    let Some(project) = state.settled_snapshot() else {
        return out;
    };
    let stations = project.compute_station_coordinates();
    out.push(Raw::json(
        "ipc.distance_field(none)",
        &state.distance_field_checked(None),
    ));
    if let (Some(first), Some(last)) = (stations.first(), stations.last()) {
        out.push(Raw::json(
            "ipc.distance_field(first)",
            &state.distance_field_checked(Some(&first.station_ref)),
        ));
        out.push(Raw::json(
            "ipc.along_line(first,last)",
            &state.along_line_between_checked(endpoint(first), endpoint(last)),
        ));
        let diver = Diver::new(20.0, None).expect("a valid diver");
        let cylinder = Cylinder::new(
            11.1,
            232.0,
            200.0,
            TankMaterial::Steel,
            CylRole::BackGas,
            None,
        )
        .expect("a valid cylinder");
        out.push(Raw::json(
            "ipc.gas_reach_field(first)",
            &state.gas_reach_field(
                Some(&first.station_ref),
                None,
                &[],
                &[],
                &diver,
                &[cylinder],
                &GasConfig::default(),
                &[],
            ),
        ));
    }
    for section in &project.sections {
        let id = section.id;
        out.push(Raw::json(
            format!("ipc.section_relationships({id})"),
            &project.get_section_relationships(id),
        ));
        out.push(Raw::json(
            format!("ipc.segment_geometry({id})"),
            &state.get_segment_geometry(id),
        ));
        out.push(Raw::json(
            format!("ipc.rigid_connected_section_ids({id})"),
            &state.get_rigid_connected_section_ids(id),
        ));
    }
    out.push(Raw::json(
        "ipc.anchored_section_ids",
        &state.get_anchored_section_ids(),
    ));
    out
}

fn read_or_error(written: anyhow::Result<()>, path: &Path) -> Vec<u8> {
    match written.and_then(|()| Ok(std::fs::read(path)?)) {
        Ok(bytes) => bytes,
        Err(error) => format!("<error: {error}>").into_bytes(),
    }
}

/// The TML document inside the export's zip. The zip itself is not compared:
/// its entry carries the export time in DOS form, by design.
fn tml_document(path: &Path) -> anyhow::Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        out.extend_from_slice(format!("== {}\n", entry.name()).as_bytes());
        entry.read_to_end(&mut out)?;
    }
    Ok(out)
}

/// Every export, from the closure-adjusted solve as the export commands use
/// it, of a project whose clock is pinned (the PNG is binary).
fn export_channels(state: &AppState, dir: &Path) -> Vec<Raw> {
    let mut out = Vec::new();
    let Some(project) = state.settled_snapshot() else {
        return out;
    };
    let project = pinned_clock(&project);
    let stations = state
        .get_adjusted_stations()
        .unwrap_or_else(|| project.compute_station_coordinates());

    let csv = dir.join("export.csv");
    out.push(Raw {
        name: "export.csv".to_string(),
        bytes: read_or_error(crate::export::export_csv(&project, &csv), &csv),
        text: true,
    });
    let json = dir.join("export.json");
    out.push(Raw {
        name: "export.json".to_string(),
        bytes: read_or_error(crate::export::export_json(&project, &json), &json),
        text: true,
    });
    let tml = dir.join("export.tml");
    out.push(match crate::export::export_tml(&project, &tml) {
        Ok(warnings) => Raw {
            name: "export.tml".to_string(),
            bytes: {
                let mut bytes = serde_json::to_vec(&warnings).unwrap_or_default();
                bytes.push(b'\n');
                bytes.extend(
                    tml_document(&tml)
                        .unwrap_or_else(|error| format!("<unreadable: {error}>").into_bytes()),
                );
                bytes
            },
            text: true,
        },
        Err(error) => Raw::text("export.tml", format!("<error: {error}>")),
    });
    // Labels on, so the label placement is in the drawing too.
    let labelled = ExportImageOptions {
        format: ImageFormat::Svg,
        dpi: 72,
        show_station_labels: true,
        show_section_names: true,
        show_station_comments: true,
        show_shot_data: true,
        ..ExportImageOptions::default()
    };
    out.push(
        match crate::export::generate_svg_string_with_stations(
            &project,
            &labelled,
            stations.clone(),
        ) {
            Ok(svg) => Raw::text("export.svg", svg),
            Err(error) => Raw::text("export.svg", format!("<error: {error}>")),
        },
    );
    let png = dir.join("export.png");
    let png_options = ExportImageOptions {
        format: ImageFormat::Png,
        ..labelled
    };
    out.push(Raw::binary(
        "export.png",
        read_or_error(
            crate::export::export_image_with_stations(&project, &png, &png_options, stations),
            &png,
        ),
    ));
    out
}

/// The tables whose blobs are gzipped history payloads.
const HISTORY_TABLES: &[&str] = &["undo_stack", "redo_stack"];

/// The raw-data view shows a blob as `<blob N B>`. A history payload carries
/// the wall clock (the stamps inside the commands and snapshots it records),
/// and a stamp's length varies with its fraction digits, so its COMPRESSED
/// size is clock noise. Its CONTENT is not masked away: the `saved.table.*`
/// channels inflate every blob and compare it with only the stamps masked.
fn mask_blob_sizes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("<blob ") {
        out.push_str(rest.get(..at).unwrap_or_default());
        let tail = rest.get(at..).unwrap_or_default();
        let end = tail.find('>').map_or(tail.len(), |end| end + 1);
        out.push_str("<blob>");
        rest = tail.get(end..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// Save through the app's own pipeline, then every table of the file and the
/// raw-data view of it.
fn save_channels(state: &AppState, dir: &Path) -> Vec<Raw> {
    let path = dir.join("determinism.stalx");
    let saved = state.save_project(Some(path.clone()));
    let mut out = vec![Raw::text(
        "save",
        format!("success={} error={:?}", saved.success, saved.error),
    )];
    if !saved.success {
        return out;
    }
    out.extend(file_channels("saved", &path));
    match state.list_raw_table_names() {
        Ok(names) => {
            out.push(Raw::json("ipc.raw_table_names", &names));
            for name in names {
                let table = state
                    .read_raw_table(&name, 1000, 0)
                    .map_err(|error| error.to_string());
                let mut text = serde_json::to_string(&table)
                    .unwrap_or_else(|error| format!("<does not serialize: {error}>"));
                if HISTORY_TABLES.contains(&name.as_str()) {
                    text = mask_blob_sizes(&text);
                }
                out.push(Raw::text(format!("ipc.raw_table({name})"), text));
            }
        }
        Err(error) => out.push(Raw::text(
            "ipc.raw_table_names",
            format!("<error: {error}>"),
        )),
    }
    out
}

/// One cell as text. A gzip blob (the history payloads) is inflated so the
/// clock inside it can be masked; any other blob is shown as hex.
fn cell(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => "null".to_string(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => format!("{value:?}"),
        ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        ValueRef::Blob(bytes) => {
            if bytes.starts_with(&[0x1f, 0x8b]) {
                let mut inflated = String::new();
                if flate2::read::GzDecoder::new(bytes)
                    .read_to_string(&mut inflated)
                    .is_ok()
                {
                    return format!("gzip[{inflated}]");
                }
            }
            let mut hex = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                let _ = write!(hex, "{byte:02x}");
            }
            format!("blob[{hex}]")
        }
    }
}

/// Every row of `table`, in rowid order - the order it was written in.
fn table_rows(conn: &Connection, table: &str) -> String {
    let ordered = format!("SELECT rowid, * FROM \"{table}\" ORDER BY rowid");
    let natural = format!("SELECT * FROM \"{table}\"");
    let mut statement = match conn.prepare(&ordered).or_else(|_| conn.prepare(&natural)) {
        Ok(statement) => statement,
        Err(error) => return format!("<unreadable: {error}>"),
    };
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(ToString::to_string)
        .collect();
    let mut out = columns.join("|");
    out.push('\n');
    let Ok(mut rows) = statement.query([]) else {
        return out + "<query failed>";
    };
    while let Ok(Some(row)) = rows.next() {
        let cells: Vec<String> = (0..columns.len())
            .map(|index| row.get_ref(index).map_or_else(|_| "<?>".to_string(), cell))
            .collect();
        out.push_str(&cells.join("|"));
        out.push('\n');
    }
    out
}

/// Every table of the `.stalx` at `path`, row by row in rowid order, the schema,
/// and the page bytes of each table or index that fits one leaf page and holds
/// nothing a mask touches - where a different write ORDER shows even when the
/// rows read back equal (the technique of `save_order_tests.rs`).
fn file_channels(prefix: &str, path: &Path) -> Vec<Raw> {
    let mut out = Vec::new();
    let conn = match Connection::open(path) {
        Ok(conn) => conn,
        Err(error) => {
            out.push(Raw::text(prefix, format!("<cannot open: {error}>")));
            return out;
        }
    };
    let objects: Vec<(String, String, String, i64, String)> = conn
        .prepare(
            "SELECT type, name, tbl_name, rootpage, IFNULL(sql, '') FROM sqlite_master \
             ORDER BY type, name",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect()
        })
        .unwrap_or_default();
    out.push(Raw::text(
        format!("{prefix}.schema"),
        objects
            .iter()
            .map(|(kind, name, _, root, sql)| format!("{kind} {name} root={root} {sql}"))
            .collect::<Vec<_>>()
            .join("\n"),
    ));
    let mut clean_tables: HashSet<String> = HashSet::new();
    for (kind, name, _, _, _) in &objects {
        if kind != "table" {
            continue;
        }
        let rows = table_rows(&conn, name);
        let mut probe_mask = Masker::new();
        if probe_mask.mask(rows.as_bytes(), true) == rows.as_bytes() {
            clean_tables.insert(name.clone());
        }
        out.push(Raw::text(format!("{prefix}.table.{name}"), rows));
    }
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .unwrap_or(0);
    drop(conn);
    let (Ok(bytes), Ok(page_size)) = (std::fs::read(path), usize::try_from(page_size)) else {
        return out;
    };
    for (_, name, table, root, _) in &objects {
        let Ok(root) = usize::try_from(*root) else {
            continue;
        };
        // Page 1 opens with the file header, which carries the change counter.
        if root < 2 || page_size == 0 || !clean_tables.contains(table) {
            continue;
        }
        let start = (root - 1) * page_size;
        let Some(page) = bytes.get(start..start + page_size) else {
            continue;
        };
        // 0x0D: a table leaf page, 0x0A: an index leaf page.
        if matches!(page.first(), Some(0x0D | 0x0A)) {
            out.push(Raw::binary(format!("{prefix}.page.{name}"), page.to_vec()));
        }
    }
    out
}

// ===========================================================================
// Replays and their comparison
// ===========================================================================

/// What one replay of a scenario produced.
struct Replay {
    stages: Vec<Stage>,
    /// The scenario's own summary (an engine's outcome counters).
    verdict: String,
    /// Iteration order of one fixed set on this replay's thread: the evidence
    /// that the hasher really was keyed differently.
    hasher: Vec<u32>,
}

/// The iteration order of the set `0..64`, which depends only on the
/// thread's `RandomState` keys.
fn hasher_order() -> Vec<u32> {
    let set: HashSet<u32> = (0..64).collect();
    set.into_iter().collect()
}

/// The shot-uuid seed every replay mints from.
const UUID_SEED: u64 = 0x00DE_7E12;

/// Stack per replay thread, as the fuzz engine's workers (`WORKER_STACK_BYTES`).
const REPLAY_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Run `scenario` on a fresh thread with the probe armed.
fn replay(name: &str, index: usize, scenario: &(dyn Fn() -> String + Sync)) -> Replay {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name(format!("det-{name}-{index}"))
            .stack_size(REPLAY_STACK_BYTES)
            .spawn_scoped(scope, || {
                let _config = isolated_config();
                // Every replay mints the same shot uuids: an output ordered by
                // a uuid (a station key after a shot is) must not differ just
                // because v4 is random. See `uuid_mint`.
                let _uuids = crate::models::uuid_mint::seed_on_this_thread(UUID_SEED);
                let hasher = hasher_order();
                PROBE.with(|probe| {
                    *probe.borrow_mut() = Some(Probe {
                        stages: Vec::new(),
                        masker: Masker::new(),
                        scratch: TempDir::new().expect("a scratch dir"),
                    });
                });
                let verdict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(scenario))
                    .unwrap_or_else(|payload| format!("PANICKED: {}", panic_message(&*payload)));
                let stages = PROBE
                    .with(|probe| probe.borrow_mut().take())
                    .map(|probe| probe.stages)
                    .unwrap_or_default();
                Replay {
                    stages,
                    verdict,
                    hasher,
                }
            })
            .expect("spawn a replay thread")
            .join()
            .expect("a replay thread")
    })
}

/// Where two byte strings first differ, shown both ways.
fn show_difference(left: &[u8], right: &[u8]) -> String {
    let at = left
        .iter()
        .zip(right)
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| left.len().min(right.len()));
    let window = |bytes: &[u8]| -> String {
        let from = at.saturating_sub(120);
        let to = (at + 120).min(bytes.len());
        let slice = bytes.get(from..to).unwrap_or_default();
        if std::str::from_utf8(slice).is_ok() {
            String::from_utf8_lossy(slice).into_owned()
        } else {
            let from = at.saturating_sub(16);
            let to = (at + 16).min(bytes.len());
            bytes
                .get(from..to)
                .unwrap_or_default()
                .iter()
                .fold(String::new(), |mut hex, byte| {
                    let _ = write!(hex, "{byte:02x} ");
                    hex
                })
        }
    };
    let mut out = format!(
        "first difference at byte {at} (lengths {} and {})\n    first:  ...{}...\n    second: ...{}...",
        left.len(),
        right.len(),
        window(left),
        window(right)
    );
    if let (Ok(a), Ok(b)) = (
        serde_json::from_slice::<serde_json::Value>(left),
        serde_json::from_slice::<serde_json::Value>(right),
    ) {
        if a == b {
            out.push_str(
                "\n    (equal as JSON values: only the ORDER of an object's keys differs)",
            );
        } else if let Some(path) = first_json_difference(&a, &b, String::new()) {
            let _ = write!(out, "\n    first differing JSON path: {path}");
        }
    }
    out
}

fn first_json_difference(
    a: &serde_json::Value,
    b: &serde_json::Value,
    path: String,
) -> Option<String> {
    use serde_json::Value;
    match (a, b) {
        (Value::Array(left), Value::Array(right)) => {
            for (index, (x, y)) in left.iter().zip(right).enumerate() {
                if x != y {
                    return first_json_difference(x, y, format!("{path}[{index}]"));
                }
            }
            (left.len() != right.len())
                .then(|| format!("{path} (lengths {} and {})", left.len(), right.len()))
        }
        (Value::Object(left), Value::Object(right)) => {
            for (key, x) in left {
                match right.get(key) {
                    Some(y) if x == y => {}
                    Some(y) => return first_json_difference(x, y, format!("{path}.{key}")),
                    None => return Some(format!("{path}.{key} (only in the first)")),
                }
            }
            right
                .keys()
                .find(|key| !left.contains_key(*key))
                .map(|key| format!("{path}.{key} (only in the second)"))
        }
        _ => (a != b).then(|| format!("{path}: {a} vs {b}")),
    }
}

/// Channels a failure reports at most, each by its FIRST difference.
const MAX_REPORTED: usize = 12;

/// Every way two replays differ, empty when they agree everywhere: the first
/// difference of each channel (so one leak does not hide another), and the
/// stage where the runs stopped taking the same moves, if they did.
fn differences(left: &Replay, right: &Replay) -> Vec<String> {
    let mut out = Vec::new();
    let mut reported: HashSet<&str> = HashSet::new();
    for (index, (a, b)) in left.stages.iter().zip(&right.stages).enumerate() {
        if a.label != b.label {
            out.push(format!(
                "stage {index}: the replays took different moves ({:?} vs {:?})",
                a.label, b.label
            ));
            return out;
        }
        let names = |stage: &Stage| -> Vec<String> {
            stage.channels.iter().map(|c| c.name.clone()).collect()
        };
        if names(a) != names(b) {
            out.push(format!(
                "stage {index} ({}): different channels\n    first:  {:?}\n    second: {:?}",
                a.label,
                names(a),
                names(b)
            ));
            return out;
        }
        for (x, y) in a.channels.iter().zip(&b.channels) {
            if x.bytes != y.bytes && reported.insert(x.name.as_str()) {
                out.push(format!(
                    "stage {index} ({}), channel {}: {}",
                    a.label,
                    x.name,
                    show_difference(&x.bytes, &y.bytes)
                ));
                if out.len() >= MAX_REPORTED {
                    out.push("...and more channels".to_string());
                    return out;
                }
            }
        }
    }
    if left.stages.len() != right.stages.len() {
        out.push(format!(
            "the replays recorded {} and {} stages",
            left.stages.len(),
            right.stages.len()
        ));
    }
    if left.verdict != right.verdict {
        out.push(format!(
            "the scenario's own verdicts differ:\n    first:  {}\n    second: {}",
            left.verdict, right.verdict
        ));
    }
    out
}

/// `STALAGMIGHT_RUST_DET_DUMP=<dir>`: every replay's stream as one text file
/// per replay (`<dir>/<scenario>/replay<i>.txt`, one channel per block), for
/// diffing a failure by hand. Binary channels are written as hex.
fn dump_replays(dir: &Path, name: &str, runs: &[Replay]) {
    let slug: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    let dir = dir.join(slug);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (index, run) in runs.iter().enumerate() {
        let mut text = format!("verdict: {}\n", run.verdict);
        for (stage_index, stage) in run.stages.iter().enumerate() {
            for channel in &stage.channels {
                let body = std::str::from_utf8(&channel.bytes).map_or_else(
                    |_| {
                        channel.bytes.iter().fold(String::new(), |mut hex, byte| {
                            let _ = write!(hex, "{byte:02x}");
                            hex
                        })
                    },
                    ToString::to_string,
                );
                let _ = writeln!(
                    text,
                    "=== stage {stage_index} ({}) {}\n{body}",
                    stage.label, channel.name
                );
            }
        }
        let _ = std::fs::write(dir.join(format!("replay{index}.txt")), text);
    }
}

/// What a determinism check of one scenario measured.
pub(super) struct Checked {
    stages: usize,
    channels: usize,
    bytes: usize,
}

/// Run `scenario` `replays` times, each on a fresh thread, and require every
/// replay to match the first. `Err` carries the repro.
pub(super) fn check(
    name: &str,
    replays: usize,
    scenario: &(dyn Fn() -> String + Sync),
) -> Result<Checked, String> {
    let runs: Vec<Replay> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..replays.max(2))
            .map(|index| scope.spawn(move || replay(name, index, scenario)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a replay"))
            .collect()
    });
    if let Ok(dir) = std::env::var("STALAGMIGHT_RUST_DET_DUMP") {
        dump_replays(Path::new(&dir), name, &runs);
    }
    let first = runs.first().expect("at least two replays");
    if first.verdict.starts_with("PANICKED") {
        return Err(format!("{name}: replay 0 {}", first.verdict));
    }
    for (index, other) in runs.iter().enumerate().skip(1) {
        if other.hasher == first.hasher {
            return Err(format!(
                "{name}: replays 0 and {index} iterated a 64-key set in the SAME order, so their \
                 hashers were not varied and this check proves nothing"
            ));
        }
        let found = differences(first, other);
        if !found.is_empty() {
            return Err(format!(
                "{name}: replays 0 and {index} of the same scenario differ\n  {}",
                found.join("\n  ")
            ));
        }
    }
    Ok(Checked {
        stages: first.stages.len(),
        channels: first.stages.iter().map(|stage| stage.channels.len()).sum(),
        bytes: first
            .stages
            .iter()
            .flat_map(|stage| &stage.channels)
            .map(|channel| channel.bytes.len())
            .sum(),
    })
}

/// Check every scenario, print one line each, and fail with every repro.
fn gate(scenarios: &[(String, Box<dyn Fn() -> String + Sync>)], replays: usize) {
    let mut failures = Vec::new();
    for (name, scenario) in scenarios {
        let started = std::time::Instant::now();
        match check(name, replays, scenario.as_ref()) {
            Ok(checked) => println!(
                "determinism {name}: {replays} replays agree on {} stages, {} channels, {} bytes \
                 ({} ms)",
                checked.stages,
                checked.channels,
                checked.bytes,
                started.elapsed().as_millis()
            ),
            Err(failure) => {
                println!("determinism {name}: DIFFERS");
                failures.push(failure);
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} scenario(s) are not deterministic:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

// ===========================================================================
// The scenarios: the existing engines, driven twice
// ===========================================================================

/// The fuzz engine's in-memory gate plan, with knobs.
const fn fuzz_plan(iterations: usize, persistence: bool, closures: bool) -> Plan {
    Plan {
        iterations,
        persistence,
        crossings: 1,
        prologue: None,
        jumps: persistence,
        closures,
        divergence: false,
        closure_shapes: closures,
        horizon: false,
        config_fuzz: false,
        combined: false,
    }
}

/// One `run_fuzz`, probed after every move (`Fuzzer::finished_move`) and in full
/// after its closing walk (`finish_run`).
fn fuzz_scenario(seed: u32, plan: Plan) -> (String, Box<dyn Fn() -> String + Sync>) {
    (
        format!(
            "undo-fuzz seed={seed} iterations={} persistence={} closures={}",
            plan.iterations, plan.persistence, plan.closures
        ),
        Box::new(move || format!("{:?}", run_fuzz(seed, plan).shape())),
    )
}

/// The metamorphic engine: both twins probed after each route and after the
/// unwind (`run_pairs`).
fn metamorphic_scenario(seed: u32, repeats: usize) -> (String, Box<dyn Fn() -> String + Sync>) {
    (
        format!("metamorphic seed={seed} repeats={repeats}"),
        Box::new(move || run_pairs_summary(seed, repeats, METAMORPHIC_PRELUDE)),
    )
}

/// Prelude commands before the metamorphic twins split (as its gate).
const METAMORPHIC_PRELUDE: usize = 12;

/// The file-format engine's project (`build_state`), saved, reopened and saved
/// again, three generations, each probed in full - the route NF-W13BE-3 took.
fn format_scenario(
    seed: u32,
    closures: bool,
    moves: usize,
) -> (String, Box<dyn Fn() -> String + Sync>) {
    (
        format!("format seed={seed} closures={closures} moves={moves}"),
        Box::new(move || {
            let Some(mut state) = build_state(seed, closures, moves) else {
                return "skipped: the project could not be built".to_string();
            };
            for generation in 0..3 {
                probe_full(&format!("generation {generation}"), &state);
                let Some(path) = state.get_project_state().and_then(|p| p.file_path) else {
                    return format!("generation {generation}: the save did not stick");
                };
                drop(state);
                let Some(reopened) = open_fresh(&path) else {
                    return format!("generation {generation}: the saved file would not open");
                };
                state = reopened;
            }
            "three generations".to_string()
        }),
    )
}

/// The shape HFI-15 needed, which the fuzz engine's random closures reach only
/// by luck: a CENTER station closing to three HUBS, each hub closing to its own
/// SPOKE. The center then sees every spoke through that spoke's hub - one
/// chain-transitive closure per hub, all landing on one station, so the order
/// they arrive in is the order the hubs are visited in.
///
/// Built through the app's commands: seven sections whose first stations are
/// merged into one (so every loop below closes a real loop), the six wheel
/// closures, then edits with the format engine's commands, the whole history
/// unwound and rewound, every step probed, and everything probed at the end.
///
/// Panics (so the check reports it) when the center does not see spokes
/// through three hubs: a scenario that silently lost its shape would pass
/// while proving nothing.
fn closure_hub_scenario(seed: u32, edits: usize) -> (String, Box<dyn Fn() -> String + Sync>) {
    (
        format!("closure-hub seed={seed} edits={edits}"),
        Box::new(move || {
            let mut rng = Rng::new(seed);
            let state = AppState::new().expect("an app state");
            let created = state.execute(Command::CreateProject {
                name: "CaveA".to_string(),
                location: None,
            });
            assert!(created.success, "CreateProject failed: {:?}", created.error);
            let mut ends = Vec::new();
            for index in 0..7 {
                let count = 1 + rng.below(3);
                let shots = (0..count).map(|_| random_shot(&mut rng)).collect();
                let section = state
                    .execute(Command::CreateSection {
                        name: Some(format!("Section{index}")),
                        surveyor: None,
                        date: None,
                        color: (*rng.pick(COLORS)).to_string(),
                        direction: None,
                        shots,
                    })
                    .section_id
                    .expect("CreateSection returned no id");
                let last = u32::try_from(count).unwrap_or(u32::MAX);
                ends.push((section, StationRef::new(section, last)));
            }
            let Some(&(root, _)) = ends.first() else {
                panic!("seed {seed}: no sections");
            };
            for &(section, _) in ends.iter().skip(1) {
                state.execute(Command::MergeStations {
                    station_a: StationRef::new(root, 0),
                    station_b: StationRef::new(section, 0),
                });
                probe_move(Some("merge the starts"), &state);
            }
            let end = |index: usize| ends.get(index).map(|&(_, end)| end);
            // Center 0; hubs 1, 2, 3; spokes 4, 5, 6.
            for (a, b) in [(0, 1), (0, 2), (0, 3), (1, 4), (2, 5), (3, 6)] {
                let (Some(station_a), Some(station_b)) = (end(a), end(b)) else {
                    continue;
                };
                state.execute(Command::CreateLoopClosure {
                    station_a,
                    station_b,
                    closure_strategy: *rng
                        .pick(&[ClosureStrategy::Move, ClosureStrategy::LeastSquaresWeighted]),
                });
                probe_move(Some("wheel closure"), &state);
            }
            let center = end(0).expect("a center");
            let chained = state
                .get_project_state()
                .and_then(|project| {
                    project
                        .compute_station_coordinates()
                        .into_iter()
                        .find(|station| station.station_ref == center)
                })
                .map(|station| {
                    station
                        .closures()
                        .iter()
                        .filter_map(|closure| closure.via_station)
                        .collect::<HashSet<StationRef>>()
                        .len()
                })
                .unwrap_or(0);
            assert!(
                chained >= 3,
                "seed {seed}: the center sees spokes through {chained} hub(s), not 3, so the \
                 HFI-15 shape was not built"
            );
            for _ in 0..edits {
                mutate(&state, &mut rng);
                probe_move(Some("edit"), &state);
            }
            let mut undone = 0;
            while state.undo().success {
                undone += 1;
                probe_move(Some("undo"), &state);
            }
            while state.redo().success {
                probe_move(Some("redo"), &state);
            }
            probe_full("final", &state);
            format!("center sees spokes through {chained} hubs; undone={undone}")
        }),
    )
}

/// The crash engine's reference pair (`crash_property_tests::build_reference`):
/// a save, more edits and an undo, then an implicit save over the same file.
fn crash_scenario(
    seed: u32,
    moves: usize,
    extra: usize,
) -> (String, Box<dyn Fn() -> String + Sync>) {
    (
        format!("crash-reference seed={seed} moves={moves} extra={extra}"),
        Box::new(move || {
            let Some(state) = build_state(seed, false, moves) else {
                return "skipped: the project could not be built".to_string();
            };
            let Ok(stash) = TempDir::new() else {
                return "skipped: no temp dir".to_string();
            };
            let live = stash.path().join("reference.stalx");
            if !state.save_project(Some(live.clone())).success {
                return "skipped: the first save was refused".to_string();
            }
            record("old file", file_channels("old", &live));
            let mut rng = Rng::new(seed.wrapping_add(0x9e37_79b9));
            for _ in 0..extra {
                mutate(&state, &mut rng);
                probe_move(Some("extra edit"), &state);
            }
            state.undo();
            if !state.save_project(None).success {
                return "skipped: the implicit save was refused".to_string();
            }
            record("new file", file_channels("new", &live));
            probe_full("after the implicit save", &state);
            "two saves".to_string()
        }),
    )
}

// ===========================================================================
// The gates (short, fixed seeds; every CI run)
// ===========================================================================

/// Replays per scenario in the gates. Two is the minimum a comparison needs;
/// the sweep raises it.
const GATE_REPLAYS: usize = 2;

/// THE UNDO-FUZZ DETERMINISM GATE: the roundtrip fuzzer's runs, in memory and
/// across a save/reload, with closures (the shape HFI-15 needed).
#[test]
fn determinism_gate_undo_fuzz() {
    let _config = isolated_config();
    let scenarios = vec![
        fuzz_scenario(1, fuzz_plan(40, false, false)),
        fuzz_scenario(2, fuzz_plan(40, false, true)),
        fuzz_scenario(3, fuzz_plan(40, true, true)),
        // Regression seed (sweep): a closure between two unconnected
        // components took its path length from a hash-ordered float sum
        // (models project/pathfinding.rs fallback), a last bit apart per run.
        fuzz_scenario(15, fuzz_plan(80, false, true)),
    ];
    gate(&scenarios, GATE_REPLAYS);
}

/// THE METAMORPHIC DETERMINISM GATE: every pair's twins, after each route and
/// after the unwind.
#[test]
fn determinism_gate_metamorphic() {
    let _config = isolated_config();
    let scenarios = vec![metamorphic_scenario(1, 1), metamorphic_scenario(2, 1)];
    gate(&scenarios, GATE_REPLAYS);
}

/// THE FILE DETERMINISM GATE: the format engine's save/reopen generations and
/// the crash engine's two saves, every table and the map-held pages.
#[test]
fn determinism_gate_files() {
    let _config = isolated_config();
    let scenarios = vec![
        format_scenario(1, false, 16),
        format_scenario(2, true, 16),
        crash_scenario(1, 12, 6),
    ];
    gate(&scenarios, GATE_REPLAYS);
}

/// THE CLOSURE-HUB DETERMINISM GATE: the shape HFI-15 was found on (a station
/// with several direct closures), built on purpose rather than left to the
/// fuzz engine's luck.
#[test]
fn determinism_gate_closure_hub() {
    let _config = isolated_config();
    let scenarios = vec![closure_hub_scenario(1, 8), closure_hub_scenario(2, 8)];
    gate(&scenarios, GATE_REPLAYS);
}

// ===========================================================================
// The harness's own premises
// ===========================================================================

/// The premise: two replays really do key their hashers differently. Without
/// this every gate above would be comparing a run with an exact copy of itself.
#[test]
fn the_replays_vary_the_hasher() {
    let scenario = || {
        probe_value("probe", Raw::json("order", &hasher_order()));
        String::new()
    };
    let runs: Vec<Replay> = (0..3)
        .map(|index| replay("hasher", index, &scenario))
        .collect();
    for pair in runs.windows(2) {
        let [a, b] = pair else { continue };
        assert_ne!(
            a.hasher, b.hasher,
            "two replay threads iterated a set identically"
        );
    }
}

/// The check BITES: a scenario that lets a `HashMap`'s order into its output is
/// reported, with the stage and channel that differ. This is the class of all
/// three historical defects, planted in the harness rather than in the app (the
/// app-level proof is in the FZ-DET report: HFI-15 re-planted, caught, reverted).
#[test]
fn the_check_catches_a_leaked_map_order() {
    let leaky = || {
        let comments: HashMap<String, String> = (0..12)
            .map(|index| (format!("StationA{index}"), format!("comment {index}")))
            .collect();
        let rows: Vec<(&String, &String)> = comments.iter().collect();
        probe_value("save", Raw::json("station_comments", &rows));
        String::new()
    };
    let failure = check("leaky", 2, &leaky).err();
    println!("leaky scenario: {failure:?}");
    let failure = failure.expect("a map-ordered output went unnoticed");
    assert!(
        failure.contains("stage 0 (save), channel station_comments"),
        "the report names the wrong place: {failure}"
    );

    let sorted = || {
        let comments: HashMap<String, String> = (0..12)
            .map(|index| (format!("StationA{index}"), format!("comment {index}")))
            .collect();
        let rows: BTreeMap<&String, &String> = comments.iter().collect();
        probe_value("save", Raw::json("station_comments", &rows));
        String::new()
    };
    assert!(
        check("sorted", 2, &sorted).is_ok(),
        "the same output sorted before it leaves must pass"
    );
}

/// The masks hide the clock, random identities and temp paths - and nothing
/// else: a changed digit next to a timestamp is still a difference.
#[test]
fn the_masks_hide_only_the_clock_ids_and_temp_paths() {
    let mut first = Masker::new();
    let mut second = Masker::new();
    let temp = std::env::temp_dir();
    let temp = temp.to_string_lossy();
    let a = format!(
        "{{\"at\":\"2026-11-05T22:58:39.123456789Z\",\"uuid\":\"3f2b8c1e-0a4d-4c7e-9b1a-2d3e4f5a6b7c\",\
         \"path\":\"{temp}/.tmpAbC123/x.stalx\",\"date\":\"2024-01-02\",\"n\":1}}"
    );
    let b = format!(
        "{{\"at\":\"2026-11-06 01:02:03+01:00\",\"uuid\":\"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee\",\
         \"path\":\"{temp}/.tmpXyZ789/x.stalx\",\"date\":\"2024-01-02\",\"n\":1}}"
    );
    assert_eq!(
        first.mask(a.as_bytes(), true),
        second.mask(b.as_bytes(), true)
    );
    let c = b.replace("\"n\":1", "\"n\":2");
    let mut third = Masker::new();
    assert_ne!(
        first.mask(a.as_bytes(), true),
        third.mask(c.as_bytes(), true)
    );
    let d = b.replace("2024-01-02", "2024-01-03");
    let mut fourth = Masker::new();
    assert_ne!(
        Masker::new().mask(a.as_bytes(), true),
        fourth.mask(d.as_bytes(), true),
        "a survey date is data, not the clock"
    );
}

// ===========================================================================
// The sweep (on demand)
// ===========================================================================

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// THE SWEEP: every engine over `1..=STALAGMIGHT_RUST_DET_SEEDS` seeds, each
/// replayed `STALAGMIGHT_RUST_DET_REPLAYS` times, fuzz runs of
/// `STALAGMIGHT_RUST_DET_ITERATIONS` moves.
///
/// ```text
/// STALAGMIGHT_RUST_DET_SEEDS=40 STALAGMIGHT_RUST_DET_REPLAYS=3 \
///   STALAGMIGHT_RUST_DET_ITERATIONS=120 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib determinism_sweep -- --ignored --nocapture
/// ```
///
/// It does not stop at the first failure: every differing scenario is listed.
#[test]
#[ignore = "sweep: long determinism run, on demand"]
fn determinism_sweep() {
    let _config = isolated_config();
    let seeds = u32::try_from(env_usize("STALAGMIGHT_RUST_DET_SEEDS", 10)).unwrap_or(10);
    let replays = env_usize("STALAGMIGHT_RUST_DET_REPLAYS", 3);
    let iterations = env_usize("STALAGMIGHT_RUST_DET_ITERATIONS", 80);
    let mut scenarios = Vec::new();
    for seed in 1..=seeds {
        scenarios.push(fuzz_scenario(seed, fuzz_plan(iterations, false, false)));
        scenarios.push(fuzz_scenario(seed, fuzz_plan(iterations, false, true)));
        scenarios.push(fuzz_scenario(seed, fuzz_plan(iterations, true, true)));
        scenarios.push(metamorphic_scenario(seed, 2));
        scenarios.push(format_scenario(seed, seed % 2 == 0, 16));
        scenarios.push(crash_scenario(seed, 12, 6));
        scenarios.push(closure_hub_scenario(seed, 8));
    }
    // `STALAGMIGHT_RUST_DET_ONLY=<text>`: only the scenarios whose name holds
    // it - the repro of one failing line, e.g. `undo-fuzz seed=15 `.
    if let Ok(only) = std::env::var("STALAGMIGHT_RUST_DET_ONLY") {
        scenarios.retain(|(name, _)| name.contains(&only));
    }
    gate(&scenarios, replays);
}
