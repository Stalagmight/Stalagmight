//! Split Section Here (#335) through `AppState`: one undo entry covers the
//! whole operation, a cut along a shot inserts a station first, and a cut near
//! a shot's end snaps to that end's station.

use super::AppState;
use crate::core::Command;
use crate::models::{SectionId, ShotData, SplitAt, SplitSectionOptions, SplitSide};

fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 45.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

fn options() -> SplitSectionOptions {
    SplitSectionOptions {
        name_a: "Main-A".to_string(),
        name_b: "Main-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    }
}

/// A project holding one section "Main" of three shots: 5, 7 and 3 metres.
fn state() -> (AppState, SectionId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Split".to_string(),
        location: None,
    });
    let created = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0)],
    });
    (state, created.section_id.unwrap())
}

/// Shot lengths of every section, in section order.
fn lengths(state: &AppState) -> Vec<Vec<f64>> {
    let project = state.get_project_state().unwrap();
    project
        .sections
        .iter()
        .map(|s| s.shots.iter().map(|shot| shot.length).collect())
        .collect()
}

fn shot_id(state: &AppState, section: SectionId, index: usize) -> u32 {
    state
        .get_project_state()
        .unwrap()
        .get_section(section)
        .unwrap()
        .shots[index]
        .id
}

#[test]
fn a_station_split_is_one_undo_step_and_redoes() {
    let (state, main) = state();

    let result =
        state.execute_split_section(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.section_id.unwrap();
    assert_eq!(result.section_ids, vec![main, second]);
    assert_eq!(lengths(&state), vec![vec![5.0], vec![7.0, 3.0]]);

    assert!(state.undo().success);
    assert_eq!(
        lengths(&state),
        vec![vec![5.0, 7.0, 3.0]],
        "one undo restores it all"
    );
    assert!(state.redo().success);
    assert_eq!(lengths(&state), vec![vec![5.0], vec![7.0, 3.0]]);
}

#[test]
fn a_cut_along_a_shot_inserts_a_station_and_undoes_in_one_step() {
    let (state, main) = state();
    let middle = shot_id(&state, main, 1);

    let result = state.execute_split_section(
        main,
        SplitAt::Shot {
            shot_id: middle,
            distance: 2.0,
        },
        options(),
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(lengths(&state), vec![vec![5.0, 2.0], vec![5.0, 3.0]]);

    assert!(state.undo().success);
    assert_eq!(lengths(&state), vec![vec![5.0, 7.0, 3.0]]);
}

#[test]
fn a_cut_near_a_shot_end_snaps_to_that_station() {
    let (state, main) = state();
    let middle = shot_id(&state, main, 1);

    let result = state.execute_split_section(
        main,
        SplitAt::Shot {
            shot_id: middle,
            distance: 6.995,
        },
        options(),
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        lengths(&state),
        vec![vec![5.0, 7.0], vec![3.0]],
        "no sliver shot: the cut snapped to the shot's end station"
    );
}

#[test]
fn a_cut_that_snaps_to_the_sections_end_is_refused_and_changes_nothing() {
    let (state, main) = state();
    let first = shot_id(&state, main, 0);

    let result = state.execute_split_section(
        main,
        SplitAt::Shot {
            shot_id: first,
            distance: 0.001,
        },
        options(),
    );
    assert!(!result.success);
    assert_eq!(lengths(&state), vec![vec![5.0, 7.0, 3.0]]);
    // No undo entry was pushed: the next undo reverts the section's creation.
    assert!(state.undo().success);
    assert!(
        lengths(&state).is_empty(),
        "the refused split left an undo entry"
    );
}
