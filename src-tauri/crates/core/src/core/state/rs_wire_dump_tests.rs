//! The project-state wire of the kept-and-archived-station shapes (plan
//! 2026-11-23, RS 6), written on demand for the agent harness's
//! `checkProjectInvariants` (e2e/agent/lib/fuzzOracleCore.ts) to be checked
//! against real payloads: a junction keeping either station (a foreign
//! slot), a loop end on another line and a same-line loop end. Synthetic.
//!
//! ```text
//! STALAGMIGHT_RS_WIRE_OUT=<dir> TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
//!   --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib rs_wire_dump -- --ignored
//! ```
//!
//! then call `checkProjectInvariants` on each `<dir>/*.json` (for example
//! from a `tsx` one-liner): every payload must give no violation.

use super::topology_kit::{line_at, line_from, open, project, run, shot, slot, st};
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::ClosureStrategy;

/// A (0, 1, 2 east) and B (0, 1 north) apart, B's 0 merged onto A's 2,
/// keeping A's 2 (`keep_a`) or B's 0.
fn junction(keep_a: bool) -> Project {
    let mut p = Project::new("Junction", None);
    let a = line_at(
        &mut p,
        "A",
        [0.0, 0.0],
        vec![shot(90.0, 5.0), shot(90.0, 5.0)],
    );
    let b = line_at(&mut p, "B", [50.0, 50.0], vec![shot(0.0, 4.0)]);
    let opened = open(&p);
    let keep = if keep_a { st(&p, a, 2) } else { st(&p, b, 0) };
    run(
        &opened.state,
        Command::MergeStations {
            station_a: slot(b, 0),
            station_b: slot(a, 2),
            keep: Some(keep),
            renames: None,
        },
    );
    project(&opened.state)
}

/// A loop (A east 4 x 10 m; B from A's 0 north, east and south onto A's 4's
/// spot) closed B 3 to A 4 keeping A's 4, and a line C walking a square
/// back to its start, closed C 0 to C 4 keeping C's 0 (a same-line loop
/// end).
fn loop_ends() -> Project {
    let mut p = Project::new("Loops", None);
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0); 4]);
    let a0 = st(&p, a, 0);
    let b = line_from(
        &mut p,
        "B",
        a0,
        vec![shot(0.0, 10.0), shot(90.0, 40.0), shot(180.0, 10.0)],
    );
    let c = line_at(
        &mut p,
        "C",
        [200.0, 0.0],
        vec![
            shot(0.0, 10.0),
            shot(90.0, 10.0),
            shot(180.0, 10.0),
            shot(270.0, 10.0),
        ],
    );
    let opened = open(&p);
    let state = &opened.state;
    for (x, y, keep) in [
        (slot(b, 3), slot(a, 4), st(&p, a, 4)),
        (slot(c, 4), slot(c, 0), st(&p, c, 0)),
    ] {
        run(
            state,
            Command::CreateLoopClosure {
                station_a: x,
                station_b: y,
                closure_strategy: ClosureStrategy::Batch,
                keep: Some(keep),
                renames: None,
            },
        );
    }
    let wire = state.project_state_wire().unwrap();
    assert!(!wire.project.loop_closures.is_empty());
    project(state)
}

#[test]
#[ignore = "on demand: writes RS wire payloads to $STALAGMIGHT_RS_WIRE_OUT"]
fn rs_wire_dump() {
    let _config = crate::test_helpers::isolated_config();
    let out = std::path::PathBuf::from(std::env::var("STALAGMIGHT_RS_WIRE_OUT").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    for (name, p) in [
        ("junction-keep-a", junction(true)),
        ("junction-keep-b", junction(false)),
        ("loop-ends", loop_ends()),
    ] {
        let opened = open(&p);
        let wire = opened.state.project_state_wire().unwrap();
        let json = serde_json::to_string_pretty(&wire).unwrap();
        std::fs::write(out.join(format!("{name}.json")), json).unwrap();
    }
}
