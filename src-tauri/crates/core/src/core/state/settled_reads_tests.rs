//! [#576 RV5-4] The distance field, the gas reach field and the along-line
//! distance read the project BETWEEN commands.
//!
//! docs/architecture.md says every IPC read of the project takes a settled
//! snapshot (under `command_lock`) or runs under `settled_read`. These three took
//! a plain `project_snapshot()`, so one landing while a command was between its
//! mutation and its epilogue (orphan conversion, reframe, reconcile, undo push)
//! computed on a project no user ever sees. Here an edit is parked in exactly that
//! window (the `park_after_mutation` seam) while each read runs on another thread:
//! a settled read waits for the edit, an unsettled one answers inside it. The
//! along-line read is probed against a held `command_lock` instead (RV6a-11, see
//! the test): through a parked edit its adjusted solve always waited, so that
//! probe could not tell a settled topology read from an unsettled one.
//!
//! All data is synthetic, built in memory.

use std::sync::mpsc;
use std::time::Duration;

use super::app_state::ParkAfterMutation;
use super::AppState;
use crate::config::GasConfig;
use crate::core::Command;
use crate::models::{Diver, RulerEndpoint, ShotData, ShotUpdates};
use crate::test_fixtures::state_with_project;

fn surveyed() -> (AppState, u32, u32) {
    let state = state_with_project("CaveA");
    let created = state.execute(Command::CreateSection {
        name: Some("SectionA".to_owned()),
        surveyor: None,
        date: None,
        color: "#336699".to_owned(),
        direction: None,
        shots: (0..3)
            .map(|_| ShotData {
                length: 5.0,
                ..Default::default()
            })
            .collect(),
    });
    assert!(created.success, "{:?}", created.error);
    let section_id = created.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[1].id;
    (state, section_id, shot_id)
}

/// Park an edit after its mutation, run `read` on another thread, and report
/// whether the read answered while the edit was still parked.
fn answered_inside_an_edit(read: impl Fn(&AppState) + Sync) -> bool {
    let (state, section_id, shot_id) = surveyed();
    let (reached_tx, reached_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    *state.park_after_mutation.lock() = Some(ParkAfterMutation {
        reached: reached_tx,
        resume: resume_rx,
    });
    let (state, read) = (&state, &read);
    std::thread::scope(|scope| {
        let edit = scope.spawn(|| {
            state.execute(Command::UpdateShot {
                section_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..ShotUpdates::default()
                },
            })
        });
        reached_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the edit reaches the window");
        let (done_tx, done_rx) = mpsc::channel();
        let reader = scope.spawn(move || {
            read(state);
            let _ = done_tx.send(());
        });
        let inside = done_rx.recv_timeout(Duration::from_millis(300)).is_ok();
        resume_tx.send(()).unwrap();
        assert!(edit.join().unwrap().success, "the edit itself succeeds");
        reader.join().unwrap();
        inside
    })
}

/// Hold `command_lock` on this thread - as a command in flight holds it from its
/// mutation through its epilogue - run `read` on another thread, and report
/// whether the read answered while the lock was held.
fn answered_while_a_command_holds_the_lock(
    state: &AppState,
    read: impl Fn(&AppState) + Sync,
) -> bool {
    let read = &read;
    std::thread::scope(|scope| {
        let in_flight = state.hold_commands();
        let (done_tx, done_rx) = mpsc::channel();
        let reader = scope.spawn(move || {
            read(state);
            let _ = done_tx.send(());
        });
        let inside = done_rx.recv_timeout(Duration::from_millis(300)).is_ok();
        drop(in_flight);
        reader.join().unwrap();
        inside
    })
}

#[test]
fn the_three_project_reads_wait_for_a_command_in_flight() {
    let distance = answered_inside_an_edit(|state| {
        assert!(state.distance_field_checked(None).unwrap().is_some());
    });
    // [#576 RV6a-11] The along-line read is probed against `command_lock` itself.
    // Through a parked edit it could not fail: every `execute()` drops the adjusted
    // cache before its mutation, so the read's adjusted-stations solve waited for
    // the lock even while its topology snapshot was taken mid-command (and asking
    // for the section id inside the read waited too). Here the cache is warm (no
    // command ran since it was filled) and the section id is known up front, so
    // only the topology read can make the along-line read wait for a command.
    let along_line = {
        let (state, section_id, _) = surveyed();
        assert!(
            state.adjusted_stations_checked().unwrap().is_some(),
            "warm the adjusted cache"
        );
        let station = |index| RulerEndpoint {
            section_id,
            station_index: Some(index),
            shot_id: None,
            t: None,
        };
        answered_while_a_command_holds_the_lock(&state, |state| {
            let (a, b) = (station(0), station(3));
            assert!(state.along_line_between_checked(a, b).unwrap().is_some());
        })
    };
    let gas = answered_inside_an_edit(|state| {
        let diver = Diver::new(20.0, None).unwrap();
        assert!(state
            .gas_reach_field(
                None,
                None,
                &[],
                &[],
                &diver,
                &[],
                &GasConfig::default(),
                &[]
            )
            .is_some());
    });
    println!(
        "answered while an edit was parked between its mutation and its epilogue: \
         distance field {distance}, gas reach {gas}; while a command held command_lock: \
         along-line {along_line}"
    );
    assert!(!distance, "the distance field read a mid-command project");
    assert!(
        !along_line,
        "the along-line distance read a mid-command project"
    );
    assert!(!gas, "the gas reach field read a mid-command project");
}
