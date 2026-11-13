//! Split Section Here (#335) through `AppState`: one undo entry covers the
//! whole operation, a cut along a shot inserts a station first, and a cut near
//! a shot's end snaps to that end's station.

use super::AppState;
use crate::core::Command;
use crate::models::{
    ClosureStrategy, SectionId, ShotData, SplitAt, SplitSectionOptions, SplitSide, StationRef,
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

/// Run a split that must be refused before anything mutates: it fails, every
/// shot keeps its length, and it leaves no undo entry, so the next undo reverts
/// the most recent section creation.
fn assert_refused(
    state: &AppState,
    section: SectionId,
    at: SplitAt,
    options: SplitSectionOptions,
    what: &str,
) {
    let before = lengths(state);
    let result = state.execute_split_section(section, at, options);
    assert!(!result.success, "{what}: the split was accepted");
    assert_eq!(
        lengths(state),
        before,
        "{what}: a refused split changed shots"
    );
    assert!(state.undo().success, "{what}");
    assert_eq!(
        lengths(state),
        before[..before.len() - 1],
        "{what}: the refused split left an undo entry"
    );
}

#[test]
fn a_distance_off_the_shot_is_refused_before_it_snaps() {
    for (distance, what) in [
        (f64::NAN, "NaN"),
        (-1.0, "negative"),
        (7.5, "past the shot's end"),
    ] {
        let (state, main) = state();
        let middle = shot_id(&state, main, 1);
        assert_refused(
            &state,
            main,
            SplitAt::Shot {
                shot_id: middle,
                distance,
            },
            options(),
            what,
        );
    }
}

#[test]
fn an_unknown_section_is_refused() {
    let (state, main) = state();
    assert_refused(
        &state,
        main + 99,
        SplitAt::Station { station_index: 1 },
        options(),
        "unknown section",
    );
}

#[test]
fn an_out_of_range_station_is_refused() {
    let (state, main) = state();
    assert_refused(
        &state,
        main,
        SplitAt::Station {
            station_index: u32::MAX,
        },
        options(),
        "station index u32::MAX",
    );
}

#[test]
fn a_shot_from_another_section_is_refused() {
    let (state, main) = state();
    let other = state
        .execute(Command::CreateSection {
            name: Some("Other".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(4.0)],
        })
        .section_id
        .unwrap();
    assert_refused(
        &state,
        main,
        SplitAt::Shot {
            shot_id: shot_id(&state, other, 0),
            distance: 2.0,
        },
        options(),
        "a shot from another section",
    );
}

#[test]
fn a_datum_on_both_halves_is_refused_before_the_shot_splits() {
    let (state, main) = state();
    let middle = shot_id(&state, main, 1);
    assert_refused(
        &state,
        main,
        SplitAt::Shot {
            shot_id: middle,
            distance: 2.0,
        },
        SplitSectionOptions {
            datum_side: SplitSide::Both,
            ..options()
        },
        "datum on both halves",
    );
}

fn flat(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    let created = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots,
    });
    assert!(created.success, "{:?}", created.error);
    created.section_id.unwrap()
}

fn raw_xy(state: &AppState, station: StationRef) -> (f64, f64) {
    let c = state
        .get_project_state()
        .unwrap()
        .compute_station_raw_coords(station)
        .unwrap();
    ((c.x * 1e6).round() / 1e6, (c.y * 1e6).round() / 1e6)
}

/// A loop closure on the split station stays on the first half's end. Main
/// runs east past the closure station into a dead end; Side leaves Main's
/// start and comes back 1 m short of station 2. A copy of the closure would be
/// the dead end's only tie, so the orphan sweep would promote it to a merge
/// and drag the dead end 1 m onto Side's end.
#[test]
fn a_loop_closure_on_the_split_station_stays_on_the_first_half() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Split".to_string(),
        location: None,
    });
    let main = create(
        &state,
        "Main",
        vec![
            flat(90.0, 10.0),
            flat(90.0, 10.0),
            flat(90.0, 5.0),
            flat(90.0, 5.0),
        ],
    );
    let side = create(
        &state,
        "Side",
        vec![flat(0.0, 10.0), flat(90.0, 21.0), flat(180.0, 10.0)],
    );
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(main, 0),
        station_b: StationRef::new(side, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(main, 2),
        station_b: StationRef::new(side, 3),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closed.success, "{:?}", closed.error);
    let closure = state.get_project_state().unwrap().loop_closures[0].clone();
    assert!(
        !closure.is_aligned && closure.misclosure_distance > 0.9,
        "setup: a 1 m misclosure, not aligned"
    );
    let dead_end: Vec<_> = (2..=4)
        .map(|i| raw_xy(&state, StationRef::new(main, i)))
        .collect();

    let result =
        state.execute_split_section(main, SplitAt::Station { station_index: 2 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.section_id.unwrap();

    let after: Vec<_> = (0..=2)
        .map(|i| raw_xy(&state, StationRef::new(second, i)))
        .collect();
    assert_eq!(after, dead_end, "the second half moved");
    let project = state.get_project_state().unwrap();
    assert_eq!(project.loop_closures.len(), 1);
    let ends = [
        project.resolve_key(&project.loop_closures[0].station_a),
        project.resolve_key(&project.loop_closures[0].station_b),
    ];
    assert!(
        ends.contains(&Some(StationRef::new(main, 2))),
        "the closure left the first half's end: {ends:?}"
    );
    assert!(ends.contains(&Some(StationRef::new(side, 3))));
}

/// `SHORT_SHOT` verdicts (is it ignored?) for every section, by section id.
fn short_shot_verdicts(state: &AppState) -> Vec<(SectionId, bool)> {
    let project = state.get_project_state().unwrap();
    state
        .compute_project_warnings(&project, None)
        .into_iter()
        .filter(|w| w.code == "SHORT_SHOT")
        .map(|w| (w.section_id, w.is_resolved))
        .collect()
}

/// An ignored warning is keyed by `(section_id, shot_uuid)`. The shot past the
/// cut moves to the second half with its uuid, so its ignore has to follow it:
/// in the session, through a save, and after a reopen.
#[test]
fn an_ignored_warning_follows_its_shot_into_the_second_half() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("split.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Split".to_string(),
        location: None,
    });
    let main = create(
        &state,
        "Main",
        vec![flat(90.0, 5.0), flat(90.0, 5.0), flat(90.0, 0.1)],
    );
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let (short_id, short_uuid) = {
        let project = state.get_project_state().unwrap();
        let shot = &project.get_section(main).unwrap().shots[2];
        (shot.id, shot.uuid.clone().unwrap())
    };
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: main,
            shot_id: short_id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: Some(short_uuid),
        })
        .unwrap();
    assert_eq!(short_shot_verdicts(&state), vec![(main, true)]);

    let result =
        state.execute_split_section(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.section_id.unwrap();
    assert_eq!(
        short_shot_verdicts(&state),
        vec![(second, true)],
        "the ignore follows its shot in the session"
    );

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(
        short_shot_verdicts(&state),
        vec![(second, true)],
        "and after a save and a reopen"
    );
    let rows = state.load_resolved_warnings().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].section_id, second, "the save re-keyed the row");
}

/// A split survives a save and a reopen: both halves, the moved references
/// and the copied split station.
#[test]
fn a_split_survives_a_save_and_a_reopen() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("split.stalx");
    let (state, main) = state();
    let comment = state.execute(Command::SetStationComment {
        station_ref: StationRef::new(main, 2),
        comment: Some("past the cut".to_string()),
    });
    assert!(comment.success, "{:?}", comment.error);
    let result =
        state.execute_split_section(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.section_id.unwrap();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let before = lengths(&state);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");

    assert_eq!(lengths(&state), before);
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project
            .station_comment_for_ref(StationRef::new(second, 1))
            .map(String::as_str),
        Some("past the cut")
    );
    assert!(project.check_structural_integrity().is_empty());
}

/// The split mints the second half's id (#555): with no id left it is refused
/// through the shared `section_budget_refusal`, before anything changes.
#[test]
fn a_split_with_no_section_id_left_is_refused() {
    let (state, main) = state();
    let mut project = state.get_project_state().unwrap();
    project.next_section_id = crate::models::MAX_SECTIONS_IN_ID_SPACE as u32;
    state.set_project_for_tests(project);
    let before = lengths(&state);

    let result =
        state.execute_split_section(main, SplitAt::Station { station_index: 1 }, options());

    assert!(!result.success);
    assert!(result
        .error
        .unwrap_or_default()
        .contains("maximum number of sections"));
    assert_eq!(lengths(&state), before);
}

/// A POSITION-keyed ignore (no shot uuid: the resolve command stores one when
/// the uuid is not on disk yet, and pre-v8 files carry them) is keyed by
/// `(section_id, shot_id)`, both of which the split changes for the shots
/// past the cut. It is pinned to the shot's persisted uuid first, so it
/// follows the shot like a uuid-keyed one.
#[test]
fn a_position_keyed_ignore_follows_its_shot_into_the_second_half() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("split.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Split".to_string(),
        location: None,
    });
    let main = create(
        &state,
        "Main",
        vec![flat(90.0, 5.0), flat(90.0, 5.0), flat(90.0, 0.1)],
    );
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let short_id = state
        .get_project_state()
        .unwrap()
        .get_section(main)
        .unwrap()
        .shots[2]
        .id;
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: main,
            shot_id: short_id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        })
        .unwrap();
    assert_eq!(short_shot_verdicts(&state), vec![(main, true)]);

    let result =
        state.execute_split_section(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.section_id.unwrap();
    assert_eq!(short_shot_verdicts(&state), vec![(second, true)]);

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(short_shot_verdicts(&state), vec![(second, true)]);
}

/// The uuid each stored ignore is pinned to, in row order.
fn ignore_uuids(state: &AppState) -> Vec<Option<String>> {
    state
        .load_resolved_warnings()
        .unwrap()
        .into_iter()
        .map(|row| row.shot_uuid)
        .collect()
}

/// A refused split changes nothing, the database included: the pin of
/// position-keyed ignores (a database write) runs only once every refusal has
/// passed.
#[test]
fn a_refused_split_leaves_position_keyed_ignores_unpinned() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("split.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Split".to_string(),
        location: None,
    });
    let main = create(
        &state,
        "Main",
        vec![flat(90.0, 5.0), flat(90.0, 5.0), flat(90.0, 0.1)],
    );
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: main,
            shot_id: shot_id(&state, main, 2),
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        })
        .unwrap();

    for (at, what) in [
        (SplitAt::Station { station_index: 0 }, "an end station"),
        (
            SplitAt::Shot {
                shot_id: shot_id(&state, main, 0),
                distance: 0.001,
            },
            "a cut that snaps to an end",
        ),
        (
            SplitAt::Shot {
                shot_id: shot_id(&state, main, 1),
                distance: 9.0,
            },
            "a distance off the shot",
        ),
    ] {
        let result = state.execute_split_section(main, at, options());
        assert!(!result.success, "{what}");
        assert_eq!(ignore_uuids(&state), vec![None], "{what}");
    }

    // The same ignore IS pinned by a split that goes ahead.
    let result =
        state.execute_split_section(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
    assert!(ignore_uuids(&state)[0].is_some());
}

/// A cut inside a shot inserts a station: a new shot of the section, with an
/// id from the section's window. With no id left there it would take the next
/// section's first id, so the cut is refused before anything changes; a cut at
/// an existing station needs no new shot id in the section and still works.
#[test]
fn a_cut_inside_a_shot_with_no_shot_id_left_is_refused() {
    let (state, main) = state();
    let mut project = state.get_project_state().unwrap();
    project.get_section_mut(main).unwrap().next_shot_id =
        (main + 1) * crate::models::FEATURE_ID_STRIDE;
    state.set_project_for_tests(project);
    let depth = state.get_undo_stack().len();

    let refused = state.execute_split_section(
        main,
        SplitAt::Shot {
            shot_id: shot_id(&state, main, 1),
            distance: 2.0,
        },
        options(),
    );
    assert!(!refused.success);
    let reason = refused.error.unwrap();
    assert!(reason.contains("used up its shot numbers"), "{reason}");
    assert_eq!(lengths(&state), vec![vec![5.0, 7.0, 3.0]]);
    assert_eq!(state.get_undo_stack().len(), depth);

    let result =
        state.execute_split_section(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
}

/// The shot cap, exactly at its edge: a section of 99,999 shots (the most a
/// section may hold) cannot take the station a cut inside a shot inserts, and
/// is refused before anything changes.
#[test]
fn a_cut_inside_a_shot_of_a_full_section_is_refused() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Full".to_string(),
        location: None,
    });
    let mut project = state.get_project_state().unwrap();
    let full = project
        .add_section(crate::models::SectionData {
            shots: (0..99_999).map(|_| flat(90.0, 1.0)).collect(),
            ..Default::default()
        })
        .id;
    state.set_project_for_tests(project);
    let depth = state.get_undo_stack().len();

    let refused = state.execute_split_section(
        full,
        SplitAt::Shot {
            shot_id: shot_id(&state, full, 10),
            distance: 0.5,
        },
        options(),
    );
    assert!(!refused.success);
    let reason = refused.error.unwrap();
    assert!(reason.contains("maximum of 99999 shots"), "{reason}");
    let project = state.get_project_state().unwrap();
    let section = project.get_section(full).unwrap();
    assert_eq!(section.shots.len(), 99_999);
    assert!(section.shots.iter().all(|shot| shot.length == 1.0));
    assert_eq!(state.get_undo_stack().len(), depth);
}

/// Delete-and-split builds its second section fresh too, and keeps the cut
/// section's orientation the same way Split Section Here does: the reverse
/// parity, and a captured gauge pair on the half that now holds its station.
/// Without that the second section's gauge reads as unreversed, one station
/// off the pair's physical station.
#[test]
fn a_delete_and_split_keeps_the_parity_and_the_gauge_on_the_second_section() {
    for range in [false, true] {
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: "Gauge".to_string(),
            location: None,
        });
        let main = create(
            &state,
            "Main",
            (0..5_u8).map(|i| flat(90.0, 5.0 + f64::from(i))).collect(),
        );
        let reversed = state.execute(Command::ReverseSection {
            section_id: main,
            swap_left_right: false,
        });
        assert!(reversed.success, "{:?}", reversed.error);
        let mut project = state.get_project_state().unwrap();
        let section = project.get_section_mut(main).unwrap();
        let pair = (section.shots[4].uuid.clone(), section.shots[3].uuid.clone());
        (section.gauge_shot_uuid, section.gauge_neighbour_uuid) = pair.clone();
        state.set_project_for_tests(project);
        let gauge_at = state
            .get_project_state()
            .unwrap()
            .compute_station_raw_coords(StationRef::new(main, 5))
            .unwrap();

        let result = if range {
            state.execute_delete_shot_range_split(
                main,
                shot_id(&state, main, 1),
                shot_id(&state, main, 2),
                "A".to_string(),
                "B".to_string(),
            )
        } else {
            state.execute_delete_shot_and_split(
                main,
                shot_id(&state, main, 2),
                "A".to_string(),
                "B".to_string(),
            )
        };
        assert!(result.success, "{:?}", result.error);
        let project = state.get_project_state().unwrap();
        let second = project.sections.iter().find(|s| s.id != main).unwrap();
        assert!(second.reverse_parity, "range {range}");
        assert_eq!(
            (
                second.gauge_shot_uuid.clone(),
                second.gauge_neighbour_uuid.clone()
            ),
            pair,
            "range {range}"
        );
        // The pair names shots[4] with shots[3] behind it: station 5 of the
        // cut section, which is station 2 of the second.
        let station = project
            .compute_station_raw_coords(StationRef::new(second.id, 2))
            .unwrap();
        assert!(
            (station.x - gauge_at.x).hypot(station.y - gauge_at.y) < 1e-9,
            "range {range}"
        );
        let first = project.get_section(main).unwrap();
        assert!(first.gauge_shot_uuid.is_none(), "range {range}");
    }
}

/// A loop closure crossing the cut: Main loops back on itself, closing from
/// station 10 onto station 2 with a 0.5 m misclosure, and is split at station
/// 5, where nothing else is merged. The closure is then the halves' only tie,
/// and a closure must close a loop (every load and every later edit converts a
/// sole-tie closure), so the split turns it into a merge and one half moves
/// by the misclosure. The preview reports that move before anything changes,
/// and the split then makes exactly that move.
#[test]
fn a_loop_closure_across_the_cut_is_previewed_as_a_move() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Loop".to_string(),
        location: None,
    });
    let main = create(
        &state,
        "Main",
        vec![
            flat(90.0, 5.0),
            flat(90.0, 5.0),
            flat(0.0, 4.0),
            flat(0.0, 4.0),
            flat(270.0, 4.0),
            flat(270.0, 4.0),
            flat(180.0, 4.0),
            flat(180.0, 4.0),
            flat(90.0, 4.0),
            flat(90.0, 3.5),
            flat(180.0, 5.0),
        ],
    );
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(main, 2),
        station_b: StationRef::new(main, 10),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closed.success, "{:?}", closed.error);
    let before: Vec<(f64, f64)> = (0..=11)
        .map(|i| raw_xy(&state, StationRef::new(main, i)))
        .collect();
    let snapshot = serde_json::to_string(&state.get_project_state().unwrap()).unwrap();
    let at = SplitAt::Station { station_index: 5 };

    let preview = state.split_section_preview(main, at, &options()).unwrap();
    assert_eq!(
        serde_json::to_string(&state.get_project_state().unwrap()).unwrap(),
        snapshot,
        "the preview changed the project"
    );
    assert_eq!(preview.closures_to_merges, 1);
    assert!((preview.moved_m - 0.5).abs() < 1e-6, "{}", preview.moved_m);

    let result = state.execute_split_section(main, at, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    assert!(
        project.loop_closures.is_empty(),
        "the closure became a merge"
    );
    let moved = |station: StationRef, index: usize| {
        let (x, y) = raw_xy(&state, station);
        (x - before[index].0).hypot(y - before[index].1)
    };
    let (moved_a, moved_b) = (
        moved(StationRef::new(main, 0), 0),
        moved(StationRef::new(second, 6), 11),
    );
    let (side, distance) = if moved_b >= moved_a {
        (SplitSide::B, moved_b)
    } else {
        (SplitSide::A, moved_a)
    };
    assert_eq!(preview.moved_side, Some(side));
    assert!((distance - preview.moved_m).abs() < 1e-6, "{distance}");
}

/// A split that leaves every tie in place moves nothing, and says so.
#[test]
fn a_plain_split_previews_no_move() {
    let (state, main) = state();
    let preview = state
        .split_section_preview(main, SplitAt::Station { station_index: 1 }, &options())
        .unwrap();
    assert_eq!(preview.closures_to_merges, 0);
    assert_eq!(preview.moved_side, None);
    assert_eq!(preview.moved_m, 0.0);

    let refused =
        state.split_section_preview(main, SplitAt::Station { station_index: 0 }, &options());
    assert!(refused.unwrap_err().contains("first or last station"));
}

/// Every station's closure-adjusted position, keyed by its measured position
/// (rounded to a micrometre), which a delete-and-split keeps for every
/// station it does not delete.
fn adjusted_by_position(state: &AppState) -> std::collections::BTreeMap<(i64, i64), (f64, f64)> {
    let project = state.get_project_state().unwrap();
    project
        .compute_adjusted_station_coordinates(&crate::config::ClosureAdjustmentConfig::default())
        .into_iter()
        .map(|s| {
            let raw = project.compute_station_raw_coords(s.station_ref).unwrap();
            let key = ((raw.x * 1e6).round() as i64, (raw.y * 1e6).round() as i64);
            (key, (s.coordinates.x, s.coordinates.y))
        })
        .collect()
}

/// Delete-and-split has the same hazard as Split Section Here: the first half
/// keeps the section's id and anchors the component, the gauge station goes
/// to the second half, and the halves stay connected. S (five shots east,
/// lowest id) has its gauge captured on its far end; X bypasses the deleted
/// shot, merged at S's stations 1 and 4; W closes a loop onto S's station 4,
/// 1 m short. Deleting S's middle shot and splitting must not move the
/// closure-adjusted survey, in either delete path.
#[test]
fn a_delete_and_split_keeps_the_component_anchor() {
    use crate::models::{LoopClosure, Project, SectionData, StationMerge};
    for range in [false, true] {
        let mut project = Project::new("Bypass".to_string(), None);
        let add = |project: &mut Project, shots: Vec<ShotData>| {
            project
                .add_section(SectionData {
                    shots,
                    ..Default::default()
                })
                .id
        };
        let s = add(&mut project, (0..5).map(|_| flat(90.0, 10.0)).collect());
        let x = add(
            &mut project,
            vec![flat(0.0, 10.0), flat(90.0, 30.0), flat(180.0, 10.0)],
        );
        let w = add(
            &mut project,
            vec![flat(0.0, 10.0), flat(270.0, 10.0), flat(180.0, 9.0)],
        );
        let raw = |project: &Project, at: StationRef| {
            let c = project.compute_station_raw_coords(at).unwrap();
            (c.x, c.y)
        };
        for (section, onto) in [(x, StationRef::new(s, 1)), (w, StationRef::new(s, 5))] {
            let (to, now) = (
                raw(&project, onto),
                raw(&project, StationRef::new(section, 0)),
            );
            project.adjust_section_offset(section, to.0 - now.0, to.1 - now.1);
            project
                .merge_stations(onto, StationRef::new(section, 0))
                .unwrap();
        }
        // The bypass closes exactly, so it is a second merge (a rigid loop the
        // merge command would refuse, but a file can hold).
        let (a, b) = (
            project.key_for_ref(StationRef::new(x, 3)).unwrap(),
            project.key_for_ref(StationRef::new(s, 4)).unwrap(),
        );
        project.record_station_merge(StationMerge {
            station_a: a,
            station_b: b,
            merged_at: chrono::Utc::now(),
        });
        let (a, b) = (
            project.key_for_ref(StationRef::new(w, 3)).unwrap(),
            project.key_for_ref(StationRef::new(s, 4)).unwrap(),
        );
        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: a,
            station_b: b,
            path_length: 29.0,
            misclosure_distance: 1.0,
            error_percentage: 3.0,
            horizontal_misclosure: 1.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::LeastSquaresWeighted,
            is_aligned: false,
        });
        project.next_closure_id = 2;
        let section = project.get_section_mut(s).unwrap();
        // The gauge on S's far end: the end of its last shot, facing away from
        // the one before it.
        section.gauge_shot_uuid = section.shots[4].uuid.clone();
        section.gauge_neighbour_uuid = section.shots[3].uuid.clone();
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: "Bypass".to_string(),
            location: None,
        });
        state.set_project_for_tests(project);
        let before = adjusted_by_position(&state);

        let result = if range {
            state.execute_delete_shot_range_split(
                s,
                shot_id(&state, s, 2),
                shot_id(&state, s, 2),
                "A".to_string(),
                "B".to_string(),
            )
        } else {
            state.execute_delete_shot_and_split(
                s,
                shot_id(&state, s, 2),
                "A".to_string(),
                "B".to_string(),
            )
        };
        assert!(result.success, "range {range}: {:?}", result.error);
        let after = adjusted_by_position(&state);
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>(),
            "range {range}"
        );
        for (key, p) in &before {
            let q = after[key];
            assert!(
                (p.0 - q.0).hypot(p.1 - q.1) < 1e-6,
                "range {range}: the station measured at {key:?} moved {} m",
                (p.0 - q.0).hypot(p.1 - q.1)
            );
        }
    }
}

/// The Split dialog's preview refuses exactly what the split refuses, with the
/// same reason, and accepts what it accepts: both run one pre-check. Each row
/// builds a fresh project, asks for the preview, then runs the split.
#[test]
fn the_preview_refuses_exactly_what_the_split_refuses() {
    type Setup = fn(&AppState, SectionId) -> (SectionId, SplitAt, SplitSectionOptions);
    let rows: Vec<(&str, Setup, bool)> = vec![
        (
            "a mid station",
            |_, m| (m, SplitAt::Station { station_index: 1 }, options()),
            true,
        ),
        (
            "inside a shot",
            |s, m| {
                (
                    m,
                    SplitAt::Shot {
                        shot_id: shot_id(s, m, 1),
                        distance: 2.0,
                    },
                    options(),
                )
            },
            true,
        ),
        (
            "an unknown section",
            |_, m| (m + 99, SplitAt::Station { station_index: 1 }, options()),
            false,
        ),
        (
            "the first station",
            |_, m| (m, SplitAt::Station { station_index: 0 }, options()),
            false,
        ),
        (
            "the last station",
            |_, m| (m, SplitAt::Station { station_index: 3 }, options()),
            false,
        ),
        (
            "station u32::MAX",
            |_, m| {
                (
                    m,
                    SplitAt::Station {
                        station_index: u32::MAX,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "a NaN distance",
            |s, m| {
                (
                    m,
                    SplitAt::Shot {
                        shot_id: shot_id(s, m, 1),
                        distance: f64::NAN,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "a negative distance",
            |s, m| {
                (
                    m,
                    SplitAt::Shot {
                        shot_id: shot_id(s, m, 1),
                        distance: -1.0,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "past the shot's end",
            |s, m| {
                (
                    m,
                    SplitAt::Shot {
                        shot_id: shot_id(s, m, 1),
                        distance: 7.5,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "a snap to the section's start",
            |s, m| {
                (
                    m,
                    SplitAt::Shot {
                        shot_id: shot_id(s, m, 0),
                        distance: 0.001,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "a datum on both halves, at a station",
            |_, m| {
                (
                    m,
                    SplitAt::Station { station_index: 1 },
                    SplitSectionOptions {
                        datum_side: SplitSide::Both,
                        ..options()
                    },
                )
            },
            false,
        ),
        (
            "a datum on both halves, inside a shot",
            |s, m| {
                (
                    m,
                    SplitAt::Shot {
                        shot_id: shot_id(s, m, 1),
                        distance: 2.0,
                    },
                    SplitSectionOptions {
                        datum_side: SplitSide::Both,
                        ..options()
                    },
                )
            },
            false,
        ),
        (
            "no shot id left, inside a shot",
            |s, m| {
                let mut project = s.get_project_state().unwrap();
                project.get_section_mut(m).unwrap().next_shot_id =
                    (m + 1) * crate::models::FEATURE_ID_STRIDE;
                s.set_project_for_tests(project);
                (
                    m,
                    SplitAt::Shot {
                        shot_id: shot_id(s, m, 1),
                        distance: 2.0,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "no section id left",
            |s, m| {
                let mut project = s.get_project_state().unwrap();
                project.next_section_id = crate::models::MAX_SECTION_ID + 1;
                s.set_project_for_tests(project);
                (m, SplitAt::Station { station_index: 1 }, options())
            },
            false,
        ),
    ];
    for (what, setup, accepted) in rows {
        let (state, main) = state();
        let (section, at, opts) = setup(&state, main);
        let preview = state.split_section_preview(section, at, &opts);
        let result = state.execute_split_section(section, at, opts);
        assert_eq!(preview.is_ok(), accepted, "{what}: preview {preview:?}");
        assert_eq!(result.success, accepted, "{what}: split {:?}", result.error);
        if let Err(reason) = preview {
            assert_eq!(Some(reason), result.error, "{what}");
        }
    }

    // A SpeleoDB project open without its lock: both refuse at the edit gate.
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("remote.stalx");
    let (state, main) = state();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    assert!(state.execute(Command::CloseProject).success);
    crate::persistence::Database::open_existing(&path)
        .unwrap()
        .save_speleo_link(Some(&crate::models::SpeleoLink {
            endpoint: "http://speleodb.invalid/".to_string(),
            project_id: "22222222-2222-2222-2222-222222222222".to_string(),
            project_name: "Remote".to_string(),
            base_commit_sha: Some("def456".to_string()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        }))
        .unwrap();
    assert!(state.open_project(path).success);
    let at = SplitAt::Station { station_index: 1 };
    let preview = state.split_section_preview(main, at, &options());
    let result = state.execute_split_section(main, at, options());
    assert!(!result.success, "read-only");
    assert_eq!(preview.err(), result.error, "read-only");
}
