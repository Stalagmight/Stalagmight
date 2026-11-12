//! Combine Sections (#707) through `AppState`: one undo entry covers the
//! whole operation, and a refused combine leaves neither a change nor an undo
//! entry.

use super::AppState;
use crate::core::Command;
use crate::models::{
    CombineOptions, SectionId, ShotData, SplitAt, SplitSectionOptions, SplitSide, StationRef,
};

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

fn options() -> CombineOptions {
    CombineOptions {
        name: "Main".to_string(),
        color: "#ff0000".to_string(),
        comment_side: SplitSide::A,
        name_side: SplitSide::A,
        offset_side: SplitSide::A,
    }
}

/// "Main" of 5, 7 and 3 metres, split at station 1 into two co-located
/// halves, with the split's own undo entry cleared.
fn split_state() -> (AppState, SectionId, SectionId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Combine".to_string(),
        location: None,
    });
    let main = state
        .execute(Command::CreateSection {
            name: Some("Main".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(7.0), shot(3.0)],
        })
        .section_id
        .unwrap();
    let second = state
        .execute_split_section(
            main,
            SplitAt::Station { station_index: 1 },
            SplitSectionOptions {
                name_a: "Main-A".to_string(),
                name_b: "Main-B".to_string(),
                color_a: "#ff0000".to_string(),
                color_b: "#00ff00".to_string(),
                comment_side: SplitSide::A,
                datum_side: SplitSide::A,
            },
        )
        .section_id
        .unwrap();
    state.undo_stack.lock().unwrap().clear();
    (state, main, second)
}

fn lengths(state: &AppState) -> Vec<Vec<f64>> {
    state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.shots.iter().map(|shot| shot.length).collect())
        .collect()
}

#[test]
fn a_combine_is_one_undo_step_and_redoes() {
    let (state, main, second) = split_state();
    let candidate = state.combine_candidate(second, main).unwrap().unwrap();
    assert_eq!((candidate.kept, candidate.other), (main, second));
    assert_eq!(
        state
            .combine_candidate_at_station(StationRef::new(second, 0))
            .unwrap(),
        Some(candidate)
    );

    let result = state.execute_combine_sections(main, second, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.section_id, Some(main));
    assert_eq!(lengths(&state), vec![vec![5.0, 7.0, 3.0]]);
    assert_eq!(state.get_undo_stack().len(), 1);

    assert!(state.undo().success);
    assert_eq!(
        lengths(&state),
        vec![vec![5.0], vec![7.0, 3.0]],
        "one undo restores both sections"
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.get_section(second).unwrap().name.as_deref(),
        Some("Main-B")
    );
    assert!(state.redo().success);
    assert_eq!(lengths(&state), vec![vec![5.0, 7.0, 3.0]]);
}

#[test]
fn a_refused_combine_changes_nothing_and_leaves_no_undo_entry() {
    let (state, main, second) = split_state();
    if let Some(project) = state.project.lock().unwrap().as_mut() {
        project.get_section_mut(second).unwrap().quality = 2.0;
    }
    let before = state.capture_project_snapshot().unwrap();

    let result = state.execute_combine_sections(main, second, options());
    assert!(!result.success);
    assert!(result.error.unwrap().contains("quality"));
    let missing = state.execute_combine_sections(main, main, options());
    assert!(!missing.success);

    assert!(state.get_undo_stack().is_empty());
    assert_eq!(state.capture_project_snapshot().unwrap(), before);
}

#[test]
fn the_station_query_refuses_unknown_and_out_of_range_stations() {
    let (state, main, second) = split_state();
    for station in [
        StationRef::new(999, 0),
        StationRef::new(main, 2),
        StationRef::new(second, 3),
        StationRef::new(second, u32::MAX),
    ] {
        assert_eq!(
            state.combine_candidate_at_station(station).unwrap(),
            None,
            "{station:?}"
        );
    }
}

#[test]
fn keeping_the_higher_id_keeps_it() {
    let (state, main, second) = split_state();

    let result = state.execute_combine_sections(second, main, options());

    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.section_id, Some(second));
    let project = state.get_project_state().unwrap();
    let ids: Vec<SectionId> = project.sections.iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![second]);
    assert_eq!(lengths(&state), vec![vec![5.0, 7.0, 3.0]]);
}

#[test]
fn a_combine_of_missing_sections_changes_nothing_and_leaves_no_undo_entry() {
    let (state, main, _) = split_state();
    let extra = state.execute(Command::CreateSection {
        name: Some("Extra".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(2.0)],
    });
    assert!(extra.success);
    let before = state.capture_project_snapshot().unwrap();

    for (kept, other) in [(998, 999), (main, 999), (999, main)] {
        let result = state.execute_combine_sections(kept, other, options());
        assert!(!result.success, "{kept} {other}");
    }

    assert_eq!(state.capture_project_snapshot().unwrap(), before);
    assert_eq!(state.get_undo_stack().len(), 1);
    assert!(state.undo().success);
    assert_eq!(
        lengths(&state),
        vec![vec![5.0], vec![7.0, 3.0]],
        "the next undo reverts the step before"
    );
}
