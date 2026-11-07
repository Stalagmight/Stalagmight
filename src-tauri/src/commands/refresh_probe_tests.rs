//! [#576 A9 / A9-D2] Release-mode probe of one edit's refresh cycle on a large
//! synthetic project: the edit, then the three reads the frontend issues after
//! it (`get_project_state`, `get_project_warnings`, `check_project_integrity`).
//! `#[ignore]`d: it measures, it does not gate. Run with
//! `cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight --lib refresh_probe -- --ignored --nocapture --test-threads=1`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::Manager;

use super::command_tests::{h, read, run};
use super::{project, validation};
use crate::core::state::{AppState, VirtualShotMode};
use crate::core::Command;
use crate::models::{ImportPlacement, ImportSectionConfig, ShotData, ShotUpdates};
use stalagmight_core::test_fixtures::{state_with_project, straight_shot};

/// A synthetic passage of `len` shots.
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

/// An open project of `sections` sections of `shots` shots each.
fn synthetic(sections: u32, shots: usize) -> AppState {
    let state = state_with_project("Probe");
    let configs = (0..sections)
        .map(|i| ImportSectionConfig {
            section_index: i,
            name: Some(format!("CaveA{i}")),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: passage(i, shots),
            placement: ImportPlacement::Origin,
            station_comments: Vec::new(),
            reverse: false,
            azimuth_reference: None,
        })
        .collect();
    let result = state.execute_import_sections(
        configs,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    state
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
#[ignore = "timing probe; run in release with --ignored --nocapture"]
fn refresh_cycle_probe() {
    for (sections, shots) in [(100_u32, 100_usize), (750, 100)] {
        // In a mock app so the reads below are the registered commands (#576 C9).
        let app = tauri::test::mock_app();
        app.manage(synthetic(sections, shots));
        let state = app.state::<AppState>();
        let first = state.get_project_state().unwrap();
        let (section_id, shot_id) = (first.sections[0].id, first.sections[0].shots[0].id);
        drop(first);

        let mut clone = Vec::new();
        // Deep copies of the open project, counted by the slot's `Arc` identity
        // (#576 A9): an edit that had to copy moves the slot to a new allocation,
        // and a state read that did not copy hands back the slot's own `Arc`.
        let mut copies_on_edit = 0_usize;
        let mut copies_on_state_read = 0_usize;
        let mut edit = Vec::new();
        let mut read_state = Vec::new();
        let mut read_warnings = Vec::new();
        let mut read_integrity = Vec::new();
        for round in 0..7 {
            let t = Instant::now();
            let deep = state.get_project_state().unwrap();
            clone.push(ms(t.elapsed()));
            drop(deep);

            let slot_before = Arc::as_ptr(&state.project_snapshot().unwrap()) as usize;
            let t = Instant::now();
            let result = state.execute(Command::UpdateShot {
                section_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(4.0 + f64::from(round) * 0.01),
                    ..Default::default()
                },
            });
            edit.push(ms(t.elapsed()));
            assert!(result.success, "{:?}", result.error);
            if Arc::as_ptr(&state.project_snapshot().unwrap()) as usize != slot_before {
                copies_on_edit += 1;
            }

            let t = Instant::now();
            let refreshed = read(project::get_project_state(h(&app))).unwrap();
            read_state.push(ms(t.elapsed()));
            if !Arc::ptr_eq(&refreshed.project, &state.project_snapshot().unwrap()) {
                copies_on_state_read += 1;
            }
            drop(refreshed);

            let t = Instant::now();
            let warnings = read(validation::get_project_warnings(h(&app)));
            read_warnings.push(ms(t.elapsed()));
            drop(warnings);

            let t = Instant::now();
            let integrity = state.integrity_check();
            read_integrity.push(ms(t.elapsed()));
            drop(integrity);
        }
        println!(
            "{} shots / {} sections, medians of 7: whole-project clone {:.1} ms; edit {:.1} ms; \
             get_project_state {:.1} ms; get_project_warnings {:.1} ms; \
             check_project_integrity {:.1} ms",
            sections as usize * shots,
            sections,
            median(clone),
            median(edit),
            median(read_state),
            median(read_warnings),
            median(read_integrity),
        );
        println!(
            "  whole-project copies over the 7 cycles: by the edit {copies_on_edit}, by \
             get_project_state {copies_on_state_read}"
        );
    }
}

/// [#576 A9-D2] The post-edit refresh payload: the JSON `get_project_state` (the
/// command, then the `serde_json` serialization Tauri runs on its answer) against
/// `get_project_state_binary` (the command, encoding included). Bytes are what
/// crosses to the webview, where the JSON form is then `JSON.parse`d whole.
#[test]
#[ignore = "timing probe; run in release with --ignored --nocapture"]
fn refresh_payload_probe() {
    use tauri::ipc::{InvokeResponseBody, IpcResponse};
    for (sections, shots) in [(100_u32, 100_usize), (750, 100)] {
        let app = tauri::test::mock_app();
        app.manage(synthetic(sections, shots));
        let mut json_ms = Vec::new();
        let mut binary_ms = Vec::new();
        let (mut json_bytes, mut binary_bytes) = (0, 0);
        for _ in 0..7 {
            let t = Instant::now();
            let state = read(project::get_project_state(h(&app))).unwrap();
            let json = serde_json::to_string(&state).unwrap();
            json_ms.push(ms(t.elapsed()));
            json_bytes = json.len();
            drop((state, json));

            let t = Instant::now();
            let response = run(project::get_project_state_binary(h(&app))).unwrap();
            let body = response.body().unwrap();
            binary_ms.push(ms(t.elapsed()));
            binary_bytes = match body {
                InvokeResponseBody::Raw(bytes) => bytes.len(),
                InvokeResponseBody::Json(text) => text.len(),
            };
        }
        println!(
            "{} shots / {} sections, medians of 7: JSON get_project_state {:.1} ms, {:.1} MB; \
             binary get_project_state_binary {:.1} ms, {:.1} MB",
            sections as usize * shots,
            sections,
            median(json_ms),
            json_bytes as f64 / 1e6,
            median(binary_ms),
            binary_bytes as f64 / 1e6,
        );
    }
}
