//! [#576 NF-W6A-3, NF-W6A-4] Two reads that must be taken between commands.
//!
//! - `resolve_warning` / `unresolve_warning` look up the shot's UUID with
//!   `shot_uuid_for`. It read the project under its own lock only, so a lookup
//!   landing while a structural edit was between its mutation and its settle step
//!   resolved the id against a project no user ever sees. It now waits for the
//!   command, like every settled read.
//! - The image export (and its preview) took the project and its closure-adjusted
//!   stations as two reads, so a command between them paired a project with the
//!   stations of another version. `settled_project` + `adjusted_stations_of` take
//!   both from one settled version.
//!
//! All data is synthetic.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::settle_gate;
use super::AppState;
use crate::core::Command;
use crate::models::{ShotData, ShotUpdates};
use crate::test_fixtures::{state_with_project, straight_shot};

fn shots(n: usize, azimuth: f64) -> Vec<ShotData> {
    (0..n).map(|_| straight_shot(azimuth, 5.0)).collect()
}

/// An open project with one section of `n` shots; its id.
fn project_with_section(n: usize) -> (AppState, u32) {
    let state = state_with_project("CaveA");
    let section = state.execute(Command::CreateSection {
        name: Some("SectionA".to_owned()),
        surveyor: None,
        date: None,
        color: "#336699".to_owned(),
        direction: None,
        shots: shots(n, 0.0),
    });
    assert!(section.success, "{:?}", section.error);
    (state, section.section_id.expect("section id"))
}

#[test]
fn a_warning_lookup_waits_for_a_running_command() {
    let (state, section) = project_with_section(2);
    let state = Arc::new(state);
    let shot_id = state.project_snapshot().expect("open").sections[0].shots[1].id;
    let parked = settle_gate::park(&state);

    let editor = {
        let state = Arc::clone(&state);
        thread::spawn(move || {
            state.execute(Command::UpdateShot {
                section_id: section,
                shot_id,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
        })
    };
    assert!(
        parked.wait_entered(Duration::from_secs(10)),
        "the edit reached the gate"
    );
    let lookup = {
        let state = Arc::clone(&state);
        thread::spawn(move || {
            let started = Instant::now();
            let uuid = state.shot_uuid_for(section, shot_id);
            (uuid, started.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(300));
    let waiting = !lookup.is_finished();
    parked.release();

    let edited = editor.join().expect("editor");
    let (uuid, waited) = lookup.join().expect("lookup");
    println!(
        "shot_uuid_for during a command between its mutation and its settle step: \
         still waiting at 300 ms: {waiting}; answered {uuid:?} after {} ms",
        waited.as_millis()
    );
    assert!(edited.success, "{:?}", edited.error);
    assert!(waiting, "the lookup waits for the command to settle");
    assert!(uuid.is_some(), "and then finds the shot");
}

#[test]
fn the_export_pairs_a_project_with_the_adjusted_stations_of_that_same_version() {
    let (state, _) = project_with_section(2);
    let settled = state.settled_project().expect("open");
    let stations_then = settled.project.compute_station_coordinates().len();

    // A command lands after the snapshot - another section, more stations - and
    // the adjusted view's memo is filled for the NEW version.
    let added = state.execute(Command::CreateSection {
        name: Some("SectionB".to_owned()),
        surveyor: None,
        date: None,
        color: "#996633".to_owned(),
        direction: None,
        shots: shots(3, 90.0),
    });
    assert!(added.success, "{:?}", added.error);
    let memo_now = state.get_adjusted_stations().expect("open").len();

    let paired = state.adjusted_stations_of(&settled).len();
    println!(
        "snapshot: {stations_then} stations; after a later command the memo holds {memo_now}; \
         the second read the export used to take (get_adjusted_stations) -> {memo_now}; \
         adjusted_stations_of(snapshot) -> {paired}"
    );
    assert!(
        memo_now > stations_then,
        "premise: the later command changed the station set"
    );
    assert_eq!(
        paired, stations_then,
        "the stations are those of the snapshot the export renders"
    );
}

#[test]
fn with_no_command_in_between_the_pairing_is_the_adjusted_view() {
    let (state, _) = project_with_section(3);
    let settled = state.settled_project().expect("open");
    let paired = state.adjusted_stations_of(&settled);
    let view = state.get_adjusted_stations().expect("open");
    assert_eq!(paired.len(), view.len());
    assert!(paired
        .iter()
        .zip(&view)
        .all(
            |(a, b)| a.coordinates.x.to_bits() == b.coordinates.x.to_bits()
                && a.coordinates.y.to_bits() == b.coordinates.y.to_bits()
        ));
}
