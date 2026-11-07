//! [#576 C7] The geometry queries behind the closure-adjusted view, the distance
//! gradient and the ruler tell "no project open" from "the project cannot be
//! read".
//!
//! The IPC commands for these three were infallible: a poisoned lock (an earlier
//! operation panicked while holding it) came back as `None`, exactly like "no
//! project open", and reached the frontend as an empty adjusted view, an absent
//! distance field or "no path". The checked variants the commands use keep the
//! fallible envelope; since the one poison policy (#576 A11) a poisoned lock is no
//! longer a failure there at all - it is recovered and the queries answer.
//!
//! All data is synthetic.

use super::AppState;
use crate::core::Command;
use crate::models::{RulerEndpoint, ShotData};
use crate::test_fixtures::state_with_project;

fn with_project() -> (AppState, u32) {
    let state = state_with_project("CaveA");
    let created = state.execute(Command::CreateSection {
        name: Some("SectionA".to_string()),
        surveyor: None,
        date: None,
        color: "#336699".to_string(),
        direction: None,
        shots: (0..3)
            .map(|i| ShotData {
                length: 5.0,
                azimuth_in: 40.0 * f64::from(i),
                azimuth_out: 40.0 * f64::from(i),
                ..Default::default()
            })
            .collect(),
    });
    (state, created.section_id.unwrap())
}

fn station(section_id: u32, station_index: u32) -> RulerEndpoint {
    RulerEndpoint {
        section_id,
        station_index: Some(station_index),
        shot_id: None,
        t: None,
    }
}

/// Poison the project mutex the way a panic inside a guarded section does.
fn poison_project(state: &AppState) {
    std::thread::scope(|scope| {
        let crashed = scope
            .spawn(|| {
                let _guard = state.project.lock();
                panic!("synthetic panic while holding the project lock (#576 C7 test)");
            })
            .join();
        assert!(crashed.is_err(), "the helper thread panicked on purpose");
    });
    assert!(state.project.is_poisoned(), "the project mutex is poisoned");
}

#[test]
fn no_project_is_an_empty_answer_and_a_poisoned_one_still_answers() {
    let _cfg = crate::test_helpers::isolated_config();

    // Healthy: every query answers.
    let (state, sid) = with_project();
    assert!(matches!(state.adjusted_stations_checked(), Ok(Some(ref s)) if !s.is_empty()));
    assert!(matches!(state.distance_field_checked(None), Ok(Some(_))));
    assert!(matches!(
        state.along_line_between_checked(station(sid, 0), station(sid, 3)),
        Ok(Some(_))
    ));

    // No project open: an honest empty answer, not an error.
    let empty = AppState::new().unwrap();
    assert!(matches!(empty.adjusted_stations_checked(), Ok(None)));
    assert!(matches!(empty.distance_field_checked(None), Ok(None)));
    assert!(matches!(
        empty.along_line_between_checked(station(1, 0), station(1, 1)),
        Ok(None)
    ));

    // [#576 A11] A crash poisoned the project lock. Under the one poison policy the
    // lock is RECOVERED (and the internal-error banner raised), so the queries keep
    // answering from the project the lock holds - neither an empty answer nor the
    // rejection C7 first gave this case.
    poison_project(&state);
    state.invalidate_adjusted_cache();
    let adjusted = state.adjusted_stations_checked();
    let distance = state.distance_field_checked(None);
    let along = state.along_line_between_checked(station(sid, 0), station(sid, 3));
    println!(
        "poisoned project lock: adjusted -> {:?}, distance -> {:?}, along-line -> {:?}",
        adjusted.as_ref().map(|s| s.as_ref().map(Vec::len)),
        distance.as_ref().map(Option::is_some),
        along.as_ref().map(Option::is_some),
    );
    assert!(
        matches!(adjusted, Ok(Some(ref s)) if !s.is_empty()),
        "the adjusted view draws from the recovered project"
    );
    assert!(
        matches!(distance, Ok(Some(_))),
        "the distance field answers"
    );
    assert!(matches!(along, Ok(Some(_))), "the ruler answers");
    assert_eq!(state.poison_monitor().first_poisoned(), Some("project"));
}
