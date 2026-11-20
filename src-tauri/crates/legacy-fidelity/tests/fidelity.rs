//! The frozen legacy reader against the LIVE 0.6.0 app, bit for bit.
//!
//! Each synthetic project is built with the live model API (the same builder
//! the legacy crate's own tests use), saved by the live app's own save
//! (`Command::SaveProject`), then opened twice from separate copies: by the
//! live app's production open (`AppState::do_load_project_full`, then
//! `get_adjusted_stations` / `get_adjustment_report`) and by
//! `stalagmight_legacy::open_path`. Raw and Closures Computed View positions
//! must be the same `f64`s, and the settled projects the same model.
#![expect(clippy::unwrap_used, reason = "test setup and assertions")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use stalagmight_core::core::events::EventSink;
use stalagmight_core::core::{AppState, Command};
use stalagmight_legacy::{open_path, OpenOptions, Positions, Slot};
use stalagmight_models as m;

#[path = "../../legacy/tests/support/builder.rs"]
mod builder;

/// The production load reports progress through a sink; nothing listens.
struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

type Bits = Vec<(Slot, [u64; 3])>;

fn legacy_bits(positions: &Positions) -> Bits {
    positions
        .iter()
        .map(|(slot, p)| (*slot, p.map(f64::to_bits)))
        .collect()
}

fn live_bits(stations: &[m::Station]) -> Bits {
    let mut bits: Bits = stations
        .iter()
        .map(|s| {
            let c = s.coordinates;
            (
                Slot {
                    line_id: s.station_ref.line_id,
                    station_index: s.station_ref.station_index,
                },
                [c.x.to_bits(), c.y.to_bits(), c.z.to_bits()],
            )
        })
        .collect();
    bits.sort_by_key(|(slot, _)| *slot);
    bits
}

/// What the live app holds after opening `path`.
struct LiveOpen {
    project: m::Project,
    raw: Bits,
    adjusted: Bits,
    report: serde_json::Value,
}

fn live_open(path: &Path) -> LiveOpen {
    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(NullSink));
    state.do_load_project_full(path.to_path_buf()).unwrap();
    let project = state.get_project_state().unwrap();
    let raw = live_bits(&project.compute_station_coordinates());
    let adjusted = live_bits(&state.get_adjusted_stations().unwrap());
    let report = serde_json::to_value(state.get_adjustment_report().unwrap()).unwrap();
    LiveOpen {
        project,
        raw,
        adjusted,
        report,
    }
}

fn save_with_live_app(project: m::Project, path: &Path) {
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    let result = state.execute(Command::SaveProject {
        path: Some(path.to_path_buf()),
    });
    assert!(result.success, "live save failed: {:?}", result.error);
}

/// Two copies of `source`, one per reader (the live open migrates and
/// repairs the file it is given).
fn two_copies(source: &Path, dir: &Path) -> (PathBuf, PathBuf) {
    let live = dir.join("live.stalx");
    let legacy = dir.join("legacy.stalx");
    std::fs::copy(source, &live).unwrap();
    std::fs::copy(source, &legacy).unwrap();
    (live, legacy)
}

/// A project as JSON with the fields that legitimately differ blanked: the
/// path it was opened from, the edit time a heal may bump, and merge times
/// (a merge the load creates is stamped with the clock by the app, with the
/// file's edit time by the legacy reader).
fn comparable(mut value: serde_json::Value) -> serde_json::Value {
    value["file_path"] = serde_json::Value::Null;
    value["modified_at"] = serde_json::Value::Null;
    if let Some(merges) = value["station_merges"].as_array_mut() {
        for merge in merges {
            merge["merged_at"] = serde_json::Value::Null;
        }
    }
    value
}

#[test]
fn the_legacy_open_matches_the_live_apps_open_bit_for_bit() {
    for (name, project) in builder::all() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("saved.stalx");
        save_with_live_app(project, &saved);
        let (live_path, legacy_path) = two_copies(&saved, dir.path());

        let live = live_open(&live_path);
        let legacy = open_path(&legacy_path, &OpenOptions::default()).unwrap();

        assert_eq!(legacy_bits(&legacy.raw_positions), live.raw, "{name}: raw");
        assert_eq!(
            legacy_bits(legacy.adjusted_positions.as_ref().unwrap()),
            live.adjusted,
            "{name}: Closures Computed View"
        );
        assert_eq!(
            serde_json::to_value(&legacy.adjustment_report).unwrap(),
            live.report,
            "{name}: adjustment report"
        );
        assert_eq!(
            comparable(serde_json::to_value(&legacy.project).unwrap()),
            comparable(serde_json::to_value(&live.project).unwrap()),
            "{name}: settled project"
        );
        assert!(!live.raw.is_empty(), "{name}");
    }
}

#[test]
fn the_tracked_old_fixtures_open_to_the_live_apps_positions() {
    for name in ["fractal_plant_iter3.stalx", "naharon.stalx"] {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../e2e/fixtures")
            .join(name);
        let dir = tempfile::tempdir().unwrap();
        let (live_path, legacy_path) = two_copies(&source, dir.path());

        let live = live_open(&live_path);
        let legacy = open_path(&legacy_path, &OpenOptions::default()).unwrap();

        // Positions only: a pre-v6 file's missing shot uuids are random in
        // the app and derived in the legacy reader, and positions do not
        // depend on them.
        assert_eq!(legacy_bits(&legacy.raw_positions), live.raw, "{name}: raw");
        assert_eq!(
            legacy_bits(legacy.adjusted_positions.as_ref().unwrap()),
            live.adjusted,
            "{name}: Closures Computed View"
        );
        assert_eq!(
            legacy.project.lines.len(),
            live.project.lines.len(),
            "{name}"
        );
    }
}
