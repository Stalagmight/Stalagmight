//! [#576 W4-UNDO] Release-mode timing probes for the undo/import/cache
//! performance fixes. Every probe is `#[ignore]`d: they measure, they do not
//! gate. Run with
//! `cargo test --release --manifest-path src-tauri/crates/core/Cargo.toml --lib perf_probe -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use crate::core::state::{AppState, VirtualShotMode};
use crate::core::Command;
use crate::models::{ImportPlacement, ImportSectionConfig, ShotData};
use crate::test_fixtures::{state_with_project, straight_shot};

/// A straight-ish synthetic passage of `len` shots.
fn passage(seed: u32, len: usize) -> Vec<ShotData> {
    (0..len)
        .map(|i| {
            straight_shot(
                f64::from((seed * 37 + i as u32 * 11) % 360),
                3.0 + f64::from((seed + i as u32) % 5),
            )
            .with_depths(10.0, 10.0)
        })
        .collect()
}

fn section_config(index: u32, shots: Vec<ShotData>) -> ImportSectionConfig {
    ImportSectionConfig {
        section_index: index,
        name: Some(format!("CaveA{index}")),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots,
        placement: ImportPlacement::Origin,
        station_comments: Vec::new(),
        reverse: false,
        azimuth_reference: None,
    }
}

/// An open project holding `sections` sections of `shots_per_section` shots.
pub(super) fn synthetic_state(sections: u32, shots_per_section: usize) -> AppState {
    let state = state_with_project("Probe");
    let configs = (0..sections)
        .map(|i| section_config(i, passage(i, shots_per_section)))
        .collect();
    let result = state.execute_import_sections(
        configs,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "seed import failed: {:?}", result.error);
    state
}

/// The history a save writes: rows and compressed bytes, per stack.
fn persisted_history(path: &std::path::Path) -> String {
    let conn = rusqlite::Connection::open(path).unwrap();
    ["undo_stack", "redo_stack"]
        .iter()
        .map(|table| {
            let (rows, bytes): (i64, i64) = conn
                .query_row(
                    &format!(
                        "SELECT count(*), coalesce(sum(length(command_json) + \
                         length(undo_data_json)), 0) FROM {table}"
                    ),
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            format!("{table} {rows} rows / {:.1} MB gzipped", bytes as f64 / 1e6)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// B20 / NF-AN6-2: saves and in-memory history of a 20,000-shot project after
/// whole-project steps (each `ReverseSection` records two project snapshots).
#[test]
#[ignore = "timing probe; run in release with --ignored --nocapture"]
fn save_after_whole_project_steps_probe() {
    let state = synthetic_state(200, 100);
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("probe.stalx");
    assert!(state.save_project(Some(path.clone())).success);

    let reverse = |state: &AppState, steps: std::ops::Range<u32>| {
        for section_id in steps {
            let result = state.execute(Command::ReverseSection {
                section_id: section_id % 200,
                swap_left_right: false,
            });
            assert!(result.success, "{:?}", result.error);
        }
    };
    let memory = |state: &AppState| {
        let stack = state.undo_stack.lock();
        format!(
            "{} undo entries, {:.1} MB estimated in memory",
            stack.len(),
            stack.estimated_undo_bytes() as f64 / 1e6
        )
    };
    let timed_save = |state: &AppState, label: &str| {
        let start = Instant::now();
        assert!(state.save_project(Some(path.clone())).success);
        println!(
            "{label}: {:.0} ms; {}",
            start.elapsed().as_secs_f64() * 1000.0,
            persisted_history(&path)
        );
    };

    reverse(&state, 0..20);
    println!("after 20 whole-project steps: {}", memory(&state));
    timed_save(&state, "save after 20 steps");
    timed_save(&state, "idle autosave (nothing changed)");
    reverse(&state, 20..21);
    timed_save(&state, "save after one more step");
    reverse(&state, 21..60);
    println!("after 60 whole-project steps: {}", memory(&state));
}

/// NF-AN6-5: an import whose sections declare no north reference used to
/// clone the whole project once per section to read the project default.
#[test]
#[ignore = "timing probe; run in release with --ignored --nocapture"]
fn import_without_declared_reference_probe() {
    for (sections, shots) in [(200_u32, 100_usize), (750, 100)] {
        let state = synthetic_state(sections, shots);
        let configs: Vec<ImportSectionConfig> = (0..200)
            .map(|i| section_config(i, passage(i + 7, 5)))
            .collect();
        let start = Instant::now();
        let result = state.execute_import_sections(
            configs,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
        );
        let elapsed = start.elapsed();
        assert!(result.success, "{:?}", result.error);
        println!(
            "import 200 x 5-shot sections (no declared reference) into a {}-shot project: {:.1} ms",
            sections as usize * shots,
            elapsed.as_secs_f64() * 1000.0
        );
    }
}
