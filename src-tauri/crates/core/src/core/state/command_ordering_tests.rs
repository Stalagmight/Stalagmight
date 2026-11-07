//! [#576 C2] The ordering guarantees that hold now that commands run on the
//! blocking pool, concurrently (docs/architecture.md, "Where commands run"):
//!
//! 1. Commands are atomic with respect to each other: `command_lock` serializes
//!    every mutation, so concurrent edits all land, once each, one undo entry
//!    each, and undoing them all restores the start.
//! 2. A settled read (`settled_snapshot` / `settled_read`) is taken between
//!    commands: never of a project a running command mutated and has not yet
//!    settled. A plain `project_snapshot` can be - that is the hazard the
//!    settled reads exist for, shown here at the seam (`settle_gate`).
//! 3. A snapshot is the project as it was when taken: a later edit never
//!    changes it (copy-on-write, #576 A9).
//! 4. Cancel is not a command: `cancel_loading` takes no `command_lock`, so it
//!    is never queued behind the work it cancels.
//!
//! All data is synthetic.

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use super::settle_gate;
use super::AppState;
use crate::core::Command;
use crate::models::{Project, ShotData, ShotUpdates, StationRef};
use crate::test_fixtures::{state_with_project, straight_shot};

fn shots(n: usize, azimuth: f64) -> Vec<ShotData> {
    (0..n).map(|_| straight_shot(azimuth, 5.0)).collect()
}

/// An open project of two sections, SectionA's end merged onto SectionB's start,
/// with no zero datum - so a length edit in SectionA moves SectionB only through
/// the reconcile every geometry command settles with.
fn merged_pair() -> (AppState, u32, u32) {
    let state = state_with_project("CaveA");
    for (name, azimuth) in [("SectionA", 0.0), ("SectionB", 90.0)] {
        let created = state.execute(Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#336699".to_string(),
            direction: None,
            shots: shots(2, azimuth),
        });
        assert!(created.success, "{:?}", created.error);
    }
    let project = state.project_snapshot().unwrap();
    let (a, b) = (project.sections[0].id, project.sections[1].id);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef {
            section_id: a,
            station_index: 2,
        },
        station_b: StationRef {
            section_id: b,
            station_index: 0,
        },
    });
    assert!(merged.success, "{:?}", merged.error);
    assert!(state
        .project_snapshot()
        .unwrap()
        .check_merge_alignment()
        .is_empty());
    (state, a, b)
}

/// Set the length of SectionA's last shot.
fn lengthen(state: &AppState, section: u32, length: f64) -> crate::core::CommandResult {
    let shot_id = state.project_snapshot().unwrap().sections[0].shots[1].id;
    state.execute(Command::UpdateShot {
        section_id: section,
        shot_id,
        updates: ShotUpdates {
            length: Some(length),
            ..Default::default()
        },
    })
}

fn drift(project: &Project) -> usize {
    project.check_merge_alignment().len()
}

/// What a read sees of the edited shot and of the history: the edit's length,
/// and how many entries the undo stack holds.
fn edit_and_history(state: &AppState) -> (f64, usize) {
    (
        state.project_snapshot().unwrap().sections[0].shots[1].length,
        state.get_undo_stack().len(),
    )
}

#[test]
fn a_settled_read_never_sees_a_command_between_its_mutation_and_its_settle_step() {
    let (state, a, _) = merged_pair();
    let state = Arc::new(state);
    let (_, depth_before) = edit_and_history(&state);
    let parked = settle_gate::park(&state);

    let editor = {
        let state = Arc::clone(&state);
        thread::spawn(move || lengthen(&state, a, 9.0))
    };
    assert!(
        parked.wait_entered(Duration::from_secs(10)),
        "the edit reached the gate"
    );

    // The window: the edit is applied, its settle step (reconcile, undo push,
    // dirty flip) has not run. An unlocked read lands in it.
    let unsettled = edit_and_history(&state);
    // Settled reads started now wait for the command to finish.
    let paired = {
        let state = Arc::clone(&state);
        thread::spawn(move || state.settled_read(|| edit_and_history(&state)))
    };
    let snapshot = {
        let state = Arc::clone(&state);
        thread::spawn(move || {
            let started = Instant::now();
            let snapshot = state.settled_snapshot().unwrap();
            (snapshot.sections[0].shots[1].length, started.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(300));
    let both_waiting = !paired.is_finished() && !snapshot.is_finished();
    parked.release();

    let edit = editor.join().unwrap();
    let settled = paired.join().unwrap();
    let (snapshot_length, waited) = snapshot.join().unwrap();
    println!(
        "undo depth before the edit: {depth_before}; unlocked read inside the window: \
         length {}, undo depth {} (the edit without its undo entry); settled_read: length {}, \
         undo depth {}; settled_snapshot: length {snapshot_length}, waited {} ms; both settled \
         reads still waiting when the command was released: {both_waiting}",
        unsettled.0,
        unsettled.1,
        settled.0,
        settled.1,
        waited.as_millis()
    );
    assert!(edit.success, "{:?}", edit.error);
    assert_eq!(
        unsettled,
        (9.0, depth_before),
        "premise: inside the window the edit is visible and its undo entry is not"
    );
    assert!(both_waiting, "the settled reads waited for the command");
    assert_eq!(
        settled,
        (9.0, depth_before + 1),
        "a settled read sees the edit together with its undo entry"
    );
    assert_eq!(
        snapshot_length, 9.0,
        "the settled snapshot is taken after it"
    );
    assert_eq!(
        drift(&state.settled_snapshot().unwrap()),
        0,
        "and the project it settled into is reconciled"
    );
}

#[test]
fn concurrent_edits_land_once_each_and_undo_back_to_the_start() {
    let (state, a, _) = merged_pair();
    let state = Arc::new(state);
    let before = state.settled_snapshot().unwrap();
    let undo_depth_before = state.get_undo_stack().len();

    const EDITS: usize = 24;
    let start = Arc::new(Barrier::new(EDITS));
    let workers: Vec<_> = (0..EDITS)
        .map(|i| {
            let state = Arc::clone(&state);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                lengthen(&state, a, 6.0 + i as f64 * 0.25)
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    let after = state.settled_snapshot().unwrap();
    let landed = state.get_undo_stack().len() - undo_depth_before;
    let final_length = after.sections[0].shots[1].length;
    println!(
        "{EDITS} concurrent edits: {} succeeded, {landed} undo entries, final length {final_length}, \
         drifted merges {}",
        results.iter().filter(|r| r.success).count(),
        drift(&after)
    );
    assert!(results.iter().all(|r| r.success));
    assert_eq!(
        landed, EDITS,
        "one undo entry per edit, none lost or merged"
    );
    assert!(
        (0..EDITS).any(|i| (6.0 + i as f64 * 0.25 - final_length).abs() < 1e-9),
        "the project holds exactly one edit's value"
    );
    assert_eq!(drift(&after), 0, "each edit settled before the next began");

    for _ in 0..EDITS {
        assert!(state.undo().success);
    }
    let restored = state.settled_snapshot().unwrap();
    assert_eq!(
        restored.sections[0].shots[1].length, before.sections[0].shots[1].length,
        "undoing every edit restores the start: they applied in one total order"
    );
    assert_eq!(drift(&restored), 0);
}

#[test]
fn a_snapshot_is_the_project_as_it_was_when_taken() {
    let (state, a, _) = merged_pair();
    let taken = state.settled_snapshot().unwrap();
    let length = taken.sections[0].shots[1].length;
    assert!(lengthen(&state, a, 12.5).success);
    assert!(state.undo().success);
    assert!(state.redo().success);
    assert_eq!(
        taken.sections[0].shots[1].length, length,
        "edits, undo and redo never reach a snapshot already handed out"
    );
    assert_eq!(
        state.settled_snapshot().unwrap().sections[0].shots[1].length,
        12.5
    );
}

#[test]
fn cancel_is_never_queued_behind_a_running_command() {
    let (state, a, _) = merged_pair();
    let state = Arc::new(state);
    let token = state.start_new_load();
    let parked = settle_gate::park(&state);
    let editor = {
        let state = Arc::clone(&state);
        thread::spawn(move || lengthen(&state, a, 7.0))
    };
    assert!(parked.wait_entered(Duration::from_secs(10)));
    // The edit holds `command_lock` at the gate; Cancel must not wait for it.
    let started = Instant::now();
    state.cancel_loading();
    let took = started.elapsed();
    let cancelled = token.load(std::sync::atomic::Ordering::SeqCst);
    parked.release();
    assert!(editor.join().unwrap().success);
    println!(
        "cancel_loading while an edit held command_lock: returned in {} us, token cancelled: \
         {cancelled}",
        took.as_micros()
    );
    assert!(cancelled);
    assert!(took < Duration::from_millis(200), "took {took:?}");
}
