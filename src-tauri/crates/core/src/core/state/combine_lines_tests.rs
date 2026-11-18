//! Combine Lines through `AppState`: one undo entry, refusals that change
//! nothing, the frame, persistence and ignored warnings.

use super::AppState;
use crate::core::Command;
use crate::models::{
    CombineLinesOptions, CombinePick, GpsCoordinates, LineId, ShotData, StationRef,
};

/// A shot credited to its own surveyor, so the snapshot comparisons below
/// cover per-shot survey credit through the combine, its undo and its redo.
fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        credit: Some(std::sync::Arc::new(crate::models::SurveyCredit {
            surveyors: crate::models::Person::named(&format!("Surveyor {azimuth}/{length}"))
                .into_iter()
                .collect(),
            explorers: Vec::new(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 4, 1),
        })),
        ..Default::default()
    }
}

fn options() -> CombineLinesOptions {
    CombineLinesOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    let created = state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots,
    });
    assert!(created.success, "{:?}", created.error);
    created.line_id.unwrap()
}

/// K (two shots east) with O (two shots north, then a short one) merged to
/// K's end.
fn state() -> (AppState, LineId, LineId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Combine".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 10.0), shot(90.0, 5.0)]);
    let o = create(
        &state,
        "O",
        vec![shot(0.0, 5.0), shot(0.0, 5.0), shot(0.0, 0.1)],
    );
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 2),
        station_b: StationRef::new(o, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    (state, k, o)
}

fn snapshot(state: &AppState) -> String {
    let project = state.get_project_state().unwrap();
    AppState::serialize_project_snapshot(&project).unwrap()
}

fn shot_counts(state: &AppState) -> Vec<(LineId, usize)> {
    state
        .get_project_state()
        .unwrap()
        .lines
        .iter()
        .map(|s| (s.id, s.shots.len()))
        .collect()
}

#[test]
fn a_combine_is_one_undo_step_and_redoes() {
    let (state, k, o) = state();
    let before = snapshot(&state);

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.line_id, Some(k));
    assert_eq!(shot_counts(&state), vec![(k, 5)]);
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before, "one undo restores it all");
    assert!(state.redo().success);
    assert_eq!(snapshot(&state), after);
}

/// A blocked combine is refused before anything happens: nothing changes and
/// no undo entry is pushed, so the next undo reverts the edit before it.
/// Both lines placed by their own zero datum is the one refusal left
/// besides lines that do not meet. (Merged at the join, the second datum
/// would be a GPS fix of one component, which no longer blocks, #741 - so the
/// join merge is dropped here, leaving the ends coincident and unmerged.)
#[test]
fn a_refused_combine_changes_nothing_and_leaves_no_undo_entry() {
    let (state, k, o) = state();
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        let (unmerged, _) = project.unmerge_stations(StationRef::new(k, 2), StationRef::new(o, 0));
        assert!(unmerged);
        for at in [StationRef::new(k, 0), StationRef::new(o, 3)] {
            let station_ref = project.key_for_ref(at).unwrap();
            project.zero_datums.push(crate::models::ZeroDatum {
                station_ref,
                gps: GpsCoordinates {
                    latitude: 20.0,
                    longitude: -87.0,
                    depth_reference: 0.0,
                },
                accuracy_m: None,
                entrance: None,
            });
        }
    }
    let before = snapshot(&state);
    let depth = state.get_undo_stack().len();

    let result = state.execute_combine_lines(k, o, options());
    assert!(!result.success);
    assert!(result.error.unwrap().contains("zero datum"));
    assert_eq!(snapshot(&state), before);
    assert_eq!(state.get_undo_stack().len(), depth);

    for (kept, other) in [(k, k), (k, 999)] {
        assert!(!state.execute_combine_lines(kept, other, options()).success);
        assert_eq!(snapshot(&state), before);
        assert_eq!(state.get_undo_stack().len(), depth);
    }
}

/// The frame datum on the other line moves to the kept one: the station
/// that defines the georeference is re-keyed, and the survey is re-framed so
/// that station still sits at local (0, 0).
#[test]
fn a_datum_on_the_other_line_keeps_the_frame() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Frame".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 10.0)]);
    let o = create(&state, "O", vec![shot(0.0, 5.0), shot(0.0, 5.0)]);
    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(o, 2),
        coordinates: GpsCoordinates {
            latitude: 20.0,
            longitude: -87.0,
            depth_reference: 0.0,
        },
        accuracy_m: None,
        entrance: None,
    });
    assert!(datum.success, "{:?}", datum.error);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 1),
        station_b: StationRef::new(o, 0),
    });
    assert!(merged.success, "{:?}", merged.error);

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);

    let project = state.get_project_state().unwrap();
    let frame = project.frame_datum_key().unwrap();
    assert_eq!(frame.line_id, k, "the frame datum is re-keyed onto K");
    let at = project.resolve_key(&frame).unwrap();
    assert_eq!(at, StationRef::new(k, 3));
    let c = project.compute_station_raw_coords(at).unwrap();
    assert!(c.x.hypot(c.y) < 1e-9, "the frame datum is the origin");
}

#[test]
fn a_combine_survives_a_save_and_a_reopen() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("combine.stalx");
    let (state, k, o) = state();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    assert!(state.execute_combine_lines(k, o, options()).success);
    let before = snapshot_lines(&state);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");

    assert_eq!(snapshot_lines(&state), before);
    let project = state.get_project_state().unwrap();
    assert!(project.get_line(o).is_none());
    assert!(project.check_structural_integrity().is_empty());
}

/// The lines as plain data: id, name, and every shot's uuid and length.
fn snapshot_lines(state: &AppState) -> Vec<(LineId, Option<String>, Vec<(String, f64)>)> {
    state
        .get_project_state()
        .unwrap()
        .lines
        .iter()
        .map(|s| {
            (
                s.id,
                s.name.clone(),
                s.shots
                    .iter()
                    .map(|shot| (shot.uuid.clone().unwrap(), shot.length))
                    .collect(),
            )
        })
        .collect()
}

/// `SHORT_SHOT` verdicts (is it ignored?) by line.
fn short_shot_verdicts(state: &AppState) -> Vec<(LineId, bool)> {
    let project = state.get_project_state().unwrap();
    state
        .compute_project_warnings(&project, None)
        .into_iter()
        .filter(|w| w.code == "SHORT_SHOT")
        .map(|w| (w.line_id, w.is_resolved))
        .collect()
}

/// An ignore on one of the other line's shots follows the shot into the
/// kept line, in the session and through a save and a reopen.
#[test]
fn an_ignored_warning_follows_its_shot_into_the_kept_line() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("combine.stalx");
    let (state, k, o) = state();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let (short_id, short_uuid) = {
        let project = state.get_project_state().unwrap();
        let shot = &project.get_line(o).unwrap().shots[2];
        (shot.id, shot.uuid.clone().unwrap())
    };
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            line_id: o,
            shot_id: short_id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: Some(short_uuid),
        })
        .unwrap();
    assert_eq!(short_shot_verdicts(&state), vec![(o, true)]);

    assert!(state.execute_combine_lines(k, o, options()).success);
    assert_eq!(short_shot_verdicts(&state), vec![(k, true)]);

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(short_shot_verdicts(&state), vec![(k, true)]);
}

/// The combine reverses the other line inside the same undo step when they
/// meet end to end; the undo puts its original orientation back exactly.
#[test]
fn an_end_to_end_combine_undoes_the_reverse_too() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Reverse".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 10.0)]);
    let o = create(&state, "O", vec![shot(0.0, 5.0), shot(45.0, 2.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 1),
        station_b: StationRef::new(o, 2),
    });
    assert!(merged.success, "{:?}", merged.error);
    let before = snapshot(&state);

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    let shots = &project.get_line(k).unwrap().shots;
    assert_eq!(shots[1].azimuth_in, 225.0, "O's last shot, reciprocated");

    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before);
}

/// The station query answers `None` for an unknown line and for a station
/// that is not a line end, including one past the end (from #713).
#[test]
fn the_station_query_refuses_unknown_and_out_of_range_stations() {
    let (state, k, o) = state();
    for station in [
        StationRef::new(999, 0),
        StationRef::new(k, 1),
        StationRef::new(o, 4),
        StationRef::new(o, u32::MAX),
    ] {
        assert_eq!(
            state.combine_candidates(station).unwrap(),
            Vec::new(),
            "{station:?}"
        );
    }
    assert_eq!(
        state
            .combine_candidates(StationRef::new(k, 2))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(state.combine_candidate_for_lines(k, 999).unwrap(), None);
    assert_eq!(state.combine_candidate_for_lines(k, k).unwrap(), None);
}

/// Keeping the higher id keeps it: the lower id is the one retired (from
/// #713), and every station of both stays where it was.
#[test]
fn keeping_the_higher_id_keeps_it() {
    let (state, k, o) = state();
    let xy = |state: &AppState, at: StationRef| {
        let c = state
            .get_project_state()
            .unwrap()
            .compute_station_raw_coords(at)
            .unwrap();
        (c.x, c.y)
    };
    let far = xy(&state, StationRef::new(k, 0));

    let result = state.execute_combine_lines(o, k, options());

    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.line_id, Some(o));
    assert_eq!(shot_counts(&state), vec![(o, 5)]);
    let project = state.get_project_state().unwrap();
    assert!(
        project.get_line(k).is_none(),
        "the lower id is the one removed"
    );
    let moved = xy(&state, StationRef::new(o, 0));
    assert!((moved.0 - far.0).hypot(moved.1 - far.1) < 1e-9);
}

/// Out of shot ids on the kept side: refused before anything happens, no undo
/// entry; keeping the other line works.
#[test]
fn a_kept_line_without_shot_id_room_is_refused_with_no_undo_entry() {
    let (state, k, o) = state();
    let mut project = state.get_project_state().unwrap();
    project.get_line_mut(k).unwrap().next_shot_id = (k + 1) * crate::models::FEATURE_ID_STRIDE - 1;
    state.set_project_for_tests(project);
    let before = snapshot(&state);
    let depth = state.get_undo_stack().len();

    let refused = state.execute_combine_lines(k, o, options());
    assert!(!refused.success);
    assert!(refused.error.unwrap().contains("used up its shot numbers"));
    assert_eq!(snapshot(&state), before);
    assert_eq!(state.get_undo_stack().len(), depth);

    assert!(state.execute_combine_lines(o, k, options()).success);
}

/// A POSITION-keyed ignore on a shot of the other line, which the combine
/// reverses and moves into the kept one: pinned to the shot's persisted uuid
/// before the move, it follows the shot, in the session and through a save.
#[test]
fn a_position_keyed_ignore_follows_its_shot_through_a_reversing_combine() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("combine.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Combine".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 10.0)]);
    let o = create(&state, "O", vec![shot(0.0, 0.1), shot(0.0, 5.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 1),
        station_b: StationRef::new(o, 2),
    });
    assert!(merged.success, "{:?}", merged.error);
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
        .get_line(o)
        .unwrap()
        .shots[0]
        .id;
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            line_id: o,
            shot_id: short_id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        })
        .unwrap();
    assert_eq!(short_shot_verdicts(&state), vec![(o, true)]);

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(short_shot_verdicts(&state), vec![(k, true)]);

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(short_shot_verdicts(&state), vec![(k, true)]);
}

/// Shots that join a line of the other parity keep their own record
/// parity (`counter_parity_shots`), through a save and a reopen.
#[test]
fn the_record_parity_of_the_other_lines_shots_survives_a_save() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("parity.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Parity".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 10.0), shot(90.0, 5.0)]);
    let o = create(&state, "O", vec![shot(0.0, 5.0), shot(0.0, 3.0)]);
    assert!(
        state
            .execute(Command::ReverseLine {
                line_id: k,
                swap_left_right: false,
            })
            .success
    );
    // K is reversed (parity true): its end is now the old station 0.
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 2),
        station_b: StationRef::new(o, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    let o_uuids: std::collections::BTreeSet<String> = state
        .get_project_state()
        .unwrap()
        .get_line(o)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect();

    assert!(state.execute_combine_lines(k, o, options()).success);
    assert_eq!(
        state.get_project_state().unwrap().counter_parity_shots,
        o_uuids
    );

    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(
        state.get_project_state().unwrap().counter_parity_shots,
        o_uuids
    );

    assert!(
        state.undo().success,
        "the loaded history undoes the combine"
    );
    assert!(
        state
            .get_project_state()
            .unwrap()
            .counter_parity_shots
            .is_empty(),
        "the undo puts the orientation back with the shots"
    );
}

/// Every station's closure-adjusted position, keyed by its measured position
/// (rounded to a micrometre), which a combine keeps for every station.
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

fn assert_same_adjusted(
    a: &std::collections::BTreeMap<(i64, i64), (f64, f64)>,
    b: &std::collections::BTreeMap<(i64, i64), (f64, f64)>,
    what: &str,
) {
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "{what}"
    );
    for (key, p) in a {
        let q = b[key];
        assert!(
            (p.0 - q.0).hypot(p.1 - q.1) < 1e-6,
            "{what}: the station measured at {key:?} moved"
        );
    }
}

/// The station the adjustment holds a component at, kept by a combine whose
/// component is anchored afterwards by a third line that does not hold it
/// (O lowest, T between, keep K): recorded as a held gauge, it holds through
/// the undo and redo, a save and a reopen.
#[test]
fn a_held_component_gauge_survives_undo_redo_and_a_reopen() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("held.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Held".to_string(),
        location: None,
    });
    let o = create(&state, "O", vec![shot(90.0, 10.0)]);
    let t = create(
        &state,
        "T",
        vec![shot(0.0, 10.0), shot(270.0, 10.0), shot(180.0, 9.0)],
    );
    let k = create(&state, "K", vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    assert!(o < t && t < k);
    for (a, b) in [
        (StationRef::new(o, 1), StationRef::new(k, 0)),
        (StationRef::new(k, 2), StationRef::new(t, 0)),
    ] {
        let merged = state.execute(Command::MergeStations {
            station_a: a,
            station_b: b,
        });
        assert!(merged.success, "{:?}", merged.error);
    }
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(t, 3),
        station_b: StationRef::new(k, 1),
        closure_strategy: crate::models::ClosureStrategy::LeastSquaresWeighted,
    });
    assert!(closed.success, "{:?}", closed.error);
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let before = adjusted_by_position(&state);

    assert!(state.execute_combine_lines(k, o, options()).success);
    let after = adjusted_by_position(&state);
    assert_same_adjusted(&before, &after, "combine");
    assert_eq!(state.get_project_state().unwrap().held_gauges.len(), 1);

    assert!(state.undo().success);
    assert_same_adjusted(&before, &adjusted_by_position(&state), "undo");
    assert!(state.get_project_state().unwrap().held_gauges.is_empty());
    assert!(state.redo().success);
    assert_same_adjusted(&after, &adjusted_by_position(&state), "redo");

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(state.get_project_state().unwrap().held_gauges.len(), 1);
    assert_same_adjusted(&after, &adjusted_by_position(&state), "reopen");
}

/// `SHORT_SHOT` verdicts (is it ignored?) by shot uuid.
fn short_shot_verdicts_by_uuid(state: &AppState) -> std::collections::BTreeMap<String, bool> {
    let project = state.get_project_state().unwrap();
    let uuid_of = |line: LineId, index: usize| {
        project.get_line(line).unwrap().shots[index]
            .uuid
            .clone()
            .unwrap()
    };
    state
        .compute_project_warnings(&project, None)
        .into_iter()
        .filter(|w| w.code == "SHORT_SHOT")
        .map(|w| (uuid_of(w.line_id, w.shot_index), w.is_resolved))
        .collect()
}

/// A short shot added to O since the last save, its `SHORT_SHOT` ignored the
/// way the resolve command stores it (by position, since its uuid is not on
/// disk yet). Returns the state, both lines, the save path and the new
/// shot's uuid.
fn unsaved_ignored_shot(
    temp: &tempfile::TempDir,
) -> (AppState, LineId, LineId, std::path::PathBuf, String) {
    let path = temp.path().join("unsaved.stalx");
    let (state, k, o) = state();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let last = state
        .get_project_state()
        .unwrap()
        .get_line(o)
        .unwrap()
        .shots[2]
        .id;
    let created = state.execute(Command::CreateShot {
        line_id: o,
        shot: shot(0.0, 0.2),
        after_shot_id: Some(last),
        before_shot_id: None,
    });
    assert!(created.success, "{:?}", created.error);
    let (id, uuid) = {
        let project = state.get_project_state().unwrap();
        let shot = &project.get_line(o).unwrap().shots[3];
        (shot.id, shot.uuid.clone().unwrap())
    };
    let keyed = state.shot_uuid_is_persisted(o, &uuid).then(|| uuid.clone());
    assert_eq!(keyed, None, "setup: the new shot's uuid is not on disk");
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            line_id: o,
            shot_id: id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: keyed,
        })
        .unwrap();
    assert_eq!(short_shot_verdicts_by_uuid(&state).get(&uuid), Some(&true));
    (state, k, o, path, uuid)
}

/// An ignore on a shot added since the last save follows the shot through a
/// combine (as the other line's, which renumbers every one of its shots)
/// and a split past it, in the session and through a save and a reopen.
#[test]
fn an_ignore_on_an_unsaved_shot_follows_it_through_a_combine_and_a_split() {
    let temp = tempfile::TempDir::new().unwrap();
    let (state, k, o, path, uuid) = unsaved_ignored_shot(&temp);
    let ignored = |state: &AppState| short_shot_verdicts_by_uuid(state).get(&uuid).copied();

    assert!(state.execute_combine_lines(k, o, options()).success);
    assert_eq!(ignored(&state), Some(true), "after the combine");
    let split = state.execute_split_line(
        k,
        crate::models::SplitAt::Station { station_index: 3 },
        crate::models::SplitLineOptions {
            name_a: "K-A".to_string(),
            name_b: "K-B".to_string(),
            color_a: "#111111".to_string(),
            color_b: "#222222".to_string(),
            comment_side: crate::models::SplitSide::A,
            datum_side: crate::models::SplitSide::A,
        },
    );
    assert!(split.success, "{:?}", split.error);
    let second = split.line_id.unwrap();
    let holder = state
        .get_project_state()
        .unwrap()
        .lines
        .iter()
        .find(|s| {
            s.shots
                .iter()
                .any(|shot| shot.uuid.as_deref() == Some(&uuid))
        })
        .unwrap()
        .id;
    assert_eq!(holder, second, "setup: the split moved the shot");
    assert_eq!(ignored(&state), Some(true), "after the split");

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(ignored(&state), Some(true), "after a reopen");
}

/// Undone before any save, the combine leaves no dangling row: the ignore is
/// on its shot again in O, and stays there through a save and a reopen.
#[test]
fn an_ignore_on_an_unsaved_shot_survives_an_undone_combine() {
    let temp = tempfile::TempDir::new().unwrap();
    let (state, k, o, path, uuid) = unsaved_ignored_shot(&temp);
    let ignored = |state: &AppState| short_shot_verdicts_by_uuid(state).get(&uuid).copied();

    assert!(state.execute_combine_lines(k, o, options()).success);
    assert!(state.undo().success);
    assert_eq!(ignored(&state), Some(true), "after the undo");

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(ignored(&state), Some(true), "after a reopen");
    let rows = state.load_resolved_warnings().unwrap();
    assert!(
        rows.iter()
            .all(|row| row.shot_uuid.as_deref() == Some(uuid.as_str()) || row.shot_uuid.is_none()),
        "{rows:?}"
    );
}

/// A crafted `.stalx` whose `held_gauges` setting holds 100,000 copies of one
/// entry, 100,000 entries naming shots that do not exist and two valid
/// entries for one component: it opens fast, and keeps one entry per
/// component (the first valid one).
#[test]
fn a_huge_held_gauge_list_opens_fast_and_keeps_one_per_component() {
    use crate::models::{HeldGauge, LineData, Project};
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("crafted.stalx");
    let mut project = Project::new("Crafted".to_string(), None);
    let ids: Vec<LineId> = (0..3)
        .map(|_| {
            project
                .add_line(LineData {
                    shots: vec![shot(90.0, 10.0), shot(0.0, 10.0)],
                    ..Default::default()
                })
                .id
        })
        .collect();
    // Lines 0 and 1 are one component; 2 is its own.
    project
        .merge_stations(StationRef::new(ids[0], 2), StationRef::new(ids[1], 0))
        .unwrap_or_else(|_| {
            let to = project
                .compute_station_raw_coords(StationRef::new(ids[0], 2))
                .unwrap();
            let now = project
                .compute_station_raw_coords(StationRef::new(ids[1], 0))
                .unwrap();
            project.adjust_line_offset(ids[1], to.x - now.x, to.y - now.y);
            project
                .merge_stations(StationRef::new(ids[0], 2), StationRef::new(ids[1], 0))
                .unwrap()
        });
    let held = |project: &Project, line: LineId, index: u32| {
        project.held_gauge_at(StationRef::new(line, index)).unwrap()
    };
    let first = held(&project, ids[1], 1);
    let same_component = held(&project, ids[0], 1);
    let other_component = held(&project, ids[2], 2);
    let mut entries: Vec<HeldGauge> = std::iter::repeat_n(first.clone(), 100_000).collect();
    entries.extend((0..100_000).map(|i| HeldGauge {
        shot: format!("00000000-0000-4000-8000-{i:012}"),
        ..first.clone()
    }));
    entries.push(same_component);
    entries.push(other_component.clone());
    {
        let mut db = crate::persistence::Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
    }
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO project_settings (key, value) VALUES ('held_gauges', ?1)",
        [serde_json::to_string(&entries).unwrap()],
    )
    .unwrap();
    drop(conn);

    let state = AppState::new().unwrap();
    let started = std::time::Instant::now();
    assert!(state.open_project(path).success);
    let took = started.elapsed();
    assert!(
        took < std::time::Duration::from_secs(5),
        "open took {took:?}"
    );
    assert_eq!(
        state.get_project_state().unwrap().held_gauges,
        vec![first, other_component]
    );
}

/// A file saved before shot uuids were unique across the project holds one
/// uuid in two lines; the load renames the higher line's copy. A held
/// gauge on that copy names its line, so it follows the rename and holds
/// the station there, not the lower line's copy of the same uuid.
#[test]
fn a_held_gauge_on_the_renamed_copy_of_a_repeated_uuid_holds_its_station() {
    use crate::models::{LineData, Project};
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("repeats.stalx");
    let ids = [
        "59508181-468f-429e-9dac-7c8e82fe53a1",
        "473a69ed-22ac-449f-8fa1-11308801104a",
        "45fcbcd1-6d49-45fb-8504-7c5734fa7c9d",
    ];
    let copy = || LineData {
        shots: ids
            .iter()
            .map(|id| ShotData {
                uuid: Some((*id).to_string()),
                ..shot(90.0, 10.0)
            })
            .collect(),
        ..Default::default()
    };
    let mut project = Project::new("Repeats".to_string(), None);
    let low = project.add_line(copy()).id;
    let high = project.add_line(copy()).id;
    project.adjust_line_offset(high, 0.0, 50.0);
    let held = project.held_gauge_at(StationRef::new(high, 2)).unwrap();
    assert_eq!(held.line_id, high, "setup");
    project.held_gauges = vec![held];
    {
        let mut db = crate::persistence::Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
    }

    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success);
    let project = state.get_project_state().unwrap();
    assert_ne!(
        project.get_line(high).unwrap().shots[2].uuid.as_deref(),
        Some(ids[2]),
        "setup: the load renamed the higher copy"
    );
    assert_eq!(
        project.adjustment_anchor_for_tests(high),
        Some(StationRef::new(high, 2))
    );
    assert_eq!(
        project.adjustment_anchor_for_tests(low),
        Some(StationRef::new(low, 0)),
        "the lower copy keeps its own gauge"
    );
}

/// The stations `at` is merged to.
fn merged_to(state: &AppState, at: StationRef) -> Vec<StationRef> {
    let project = state.get_project_state().unwrap();
    let mut out: Vec<StationRef> = project
        .station_merges
        .iter()
        .filter_map(|m| {
            let (a, b) = (
                project.resolve_key(&m.station_a)?,
                project.resolve_key(&m.station_b)?,
            );
            (a == at).then_some(b).or((b == at).then_some(a))
        })
        .collect();
    out.sort_by_key(|r| (r.line_id, r.station_index));
    out
}

/// At a junction (C merged at K's end, where O starts), combining K and O
/// keeps C merged to the joined station, the combined line's station 2
/// whether O is appended to K or K prepended to O; an undo and a redo restore
/// the whole project exactly; and ignored warnings on C (the branch) and on O
/// (the moved line) stay ignored through it, a save and a reopen.
#[test]
fn a_combine_at_a_junction_keeps_the_branch_and_undoes_exactly() {
    for keep_k in [true, false] {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("junction.stalx");
        let (state, k, o) = state();
        let c = create(&state, "C", vec![shot(180.0, 5.0), shot(180.0, 0.1)]);
        let merged = state.execute(Command::MergeStations {
            station_a: StationRef::new(k, 2),
            station_b: StationRef::new(c, 0),
        });
        assert!(merged.success, "{:?}", merged.error);
        assert!(
            state
                .execute(Command::SaveProject {
                    path: Some(path.clone())
                })
                .success
        );
        let short = |line: LineId, index: usize| {
            let project = state.get_project_state().unwrap();
            let shot = &project.get_line(line).unwrap().shots[index];
            (shot.id, shot.uuid.clone().unwrap())
        };
        let ignored: Vec<String> = [(c, 1), (o, 2)]
            .into_iter()
            .map(|(line, index)| {
                let (id, uuid) = short(line, index);
                state
                    .save_resolved_warning(&crate::persistence::ResolvedWarning {
                        line_id: line,
                        shot_id: id,
                        warning_code: "SHORT_SHOT".to_string(),
                        resolved_at: chrono::Utc::now(),
                        resolved_comment: None,
                        shot_uuid: Some(uuid.clone()),
                    })
                    .unwrap();
                uuid
            })
            .collect();
        let all_ignored = |state: &AppState, what: &str| {
            let verdicts = short_shot_verdicts_by_uuid(state);
            for uuid in &ignored {
                assert_eq!(verdicts.get(uuid), Some(&true), "keep K {keep_k}, {what}");
            }
        };
        all_ignored(&state, "before");
        let candidate = state.combine_candidate_for_lines(k, o).unwrap().unwrap();
        assert_eq!(candidate.branches, vec![c]);
        let before = snapshot(&state);

        let (kept, other) = if keep_k { (k, o) } else { (o, k) };
        let result = state.execute_combine_lines(kept, other, options());
        assert!(result.success, "{:?}", result.error);
        let joined = StationRef::new(kept, 2);
        assert_eq!(merged_to(&state, joined), vec![StationRef::new(c, 0)]);
        all_ignored(&state, "after the combine");
        let after = snapshot(&state);

        assert!(state.undo().success);
        assert_eq!(snapshot(&state), before, "keep K {keep_k}: the undo");
        all_ignored(&state, "after the undo");
        assert!(state.redo().success);
        assert_eq!(snapshot(&state), after, "keep K {keep_k}: the redo");

        assert!(state.execute(Command::SaveProject { path: None }).success);
        assert!(state.execute(Command::CloseProject).success);
        assert!(state.open_project(path).success, "reopen");
        assert_eq!(merged_to(&state, joined), vec![StationRef::new(c, 0)]);
        all_ignored(&state, "after a reopen");
    }
}

/// Two lines closing a loop on each other, combined at the loop closure:
/// the other line moves by the misclosure, the merge at the far end
/// becomes the loop's closure, the epilogue's re-solve keeps all of it, and
/// one undo restores the two lines, the merge and the closure exactly.
#[test]
fn a_combine_at_a_loop_closure_is_one_undo_step() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Loop".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 10.0)]);
    let o = create(&state, "O", vec![shot(270.0, 9.6)]);
    for command in [
        Command::MergeStations {
            station_a: StationRef::new(k, 0),
            station_b: StationRef::new(o, 1),
        },
        Command::CreateLoopClosure {
            station_a: StationRef::new(k, 1),
            station_b: StationRef::new(o, 0),
            closure_strategy: crate::models::ClosureStrategy::Batch,
        },
    ] {
        let result = state.execute(command);
        assert!(result.success, "{:?}", result.error);
    }
    let xy = |at: StationRef| {
        let c = state
            .get_project_state()
            .unwrap()
            .compute_station_raw_coords(at)
            .unwrap();
        (c.x, c.y)
    };
    let (k0, k1, o1) = (
        xy(StationRef::new(k, 0)),
        xy(StationRef::new(k, 1)),
        xy(StationRef::new(o, 1)),
    );
    let before = snapshot(&state);

    let result = state.execute_combine_lines(
        k,
        o,
        CombineLinesOptions {
            join: Some(crate::models::CombineJoinEnds {
                kept: crate::models::LineEnd::End,
                other: crate::models::LineEnd::Start,
            }),
            ..options()
        },
    );
    assert!(result.success, "{:?}", result.error);
    let close = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1) < 1e-9;
    assert!(close(xy(StationRef::new(k, 0)), k0));
    assert!(close(xy(StationRef::new(k, 1)), k1));
    assert!(close(xy(StationRef::new(k, 2)), (o1.0 + 0.4, o1.1)));
    let project = state.get_project_state().unwrap();
    assert!(project.station_merges.is_empty());
    assert_eq!(project.loop_closures.len(), 1);
    assert!((project.loop_closures[0].misclosure_distance - 0.4).abs() < 1e-9);
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before, "one undo restores it all");
    assert!(state.redo().success);
    assert_eq!(snapshot(&state), after);
}

/// A prepend into a kept line with no gauge pair pins its gauge on its
/// old station 0 with a new pair (the index rule would move it to the
/// prepended start). Undo gives the line back exactly as it was, without
/// that pair: the restore's save-heal carry-forward must not copy an edit's
/// own pair onto the half it restores. Redo puts the pair back.
#[test]
fn undoing_a_prepend_does_not_keep_the_gauge_pair_it_made() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Prepend".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 10.0), shot(90.0, 5.0)]);
    let o = create(&state, "O", vec![shot(0.0, 5.0), shot(0.0, 5.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 0),
        station_b: StationRef::new(o, 2),
    });
    assert!(merged.success, "{:?}", merged.error);
    let gauge = |state: &AppState| {
        state
            .get_project_state()
            .unwrap()
            .get_line(k)
            .unwrap()
            .gauge_shot_uuid
            .clone()
    };
    assert_eq!(gauge(&state), None);
    let before = snapshot(&state);

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    assert!(gauge(&state).is_some(), "the prepend pins K's gauge");
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before);
    assert!(state.redo().success);
    assert_eq!(snapshot(&state), after);
}

/// Combine at K's end and O's start (`kept`/`other` with their join ends),
/// through the command, after checking the station menu's plan for that
/// choice names `listed` as the ties it removes across one shot (#746).
/// Returns the command's removed-closure warnings; asserts the combine left
/// no closure across one shot, and that one Undo restores the project
/// byte-for-byte and Redo re-applies it.
fn combine_removing(
    state: &AppState,
    at: StationRef,
    (kept, kept_end): (LineId, crate::models::LineEnd),
    (other, other_end): (LineId, crate::models::LineEnd),
    listed: &[(StationRef, StationRef, crate::models::CombineRemovedKind)],
) -> Vec<String> {
    let candidate = state
        .combine_candidates(at)
        .unwrap()
        .into_iter()
        .find(|c| {
            let ends = [(c.line_a, c.end_a), (c.line_b, c.end_b)];
            ends.contains(&(kept, kept_end)) && ends.contains(&(other, other_end))
        })
        .unwrap();
    let plan = if candidate.line_a == kept {
        candidate.plan_keeping_a
    } else {
        candidate.plan_keeping_b
    };
    let named: Vec<_> = plan
        .closures_removed_across_one_shot
        .unwrap_or_default()
        .iter()
        .map(|r| (r.station_a, r.station_b, r.kind))
        .collect();
    assert_eq!(
        named, listed,
        "the dialog lists the ties the combine removes"
    );
    let before = snapshot(state);

    let result = state.execute_combine_lines(
        kept,
        other,
        CombineLinesOptions {
            join: Some(crate::models::CombineJoinEnds {
                kept: kept_end,
                other: other_end,
            }),
            ..options()
        },
    );
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert!(
        project.closures_across_one_shot().is_empty(),
        "{:?}",
        project.closures_across_one_shot()
    );
    let after = snapshot(state);

    assert!(state.undo().success);
    assert_eq!(
        snapshot(state),
        before,
        "one undo restores the removed tie too"
    );
    assert!(state.redo().success);
    assert_eq!(snapshot(state), after, "redo removes it again");
    result
        .warnings
        .iter()
        .map(|w| {
            assert_eq!(w.code, crate::models::CLOSURE_REMOVED_ACROSS_ONE_SHOT);
            assert_eq!(w.severity, crate::core::WarningSeverity::Warning);
            w.message.clone()
        })
        .collect()
}

/// #746, the issue's shape (a): K 10 m east; a passage X merged at K's end,
/// one shot 0.4 m further east; O 5 m east from X's end, merged there; K's
/// end and O's start tied by a legacy closure, as a file from before the
/// deferred strategies carries it. Combining K and O there tears the merge of
/// X's end and O's start into a closure across X's one shot: the combine
/// removes it inside its one undo step and says so.
#[test]
fn a_combine_removes_the_merge_it_would_tear_across_one_shot() {
    use crate::models::{CombineRemovedKind, LineEnd};
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Passage".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 10.0)]);
    let x = create(&state, "X", vec![shot(90.0, 0.4)]);
    let o = create(&state, "O", vec![shot(90.0, 5.0)]);
    for (a, b) in [
        (StationRef::new(k, 1), StationRef::new(x, 0)),
        (StationRef::new(x, 1), StationRef::new(o, 0)),
    ] {
        let merged = state.execute(Command::MergeStations {
            station_a: a,
            station_b: b,
        });
        assert!(merged.success, "{:?}", merged.error);
    }
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        let id = project.next_closure_id;
        project.next_closure_id += 1;
        let (station_a, station_b) = (
            project.key_for_ref(StationRef::new(k, 1)).unwrap(),
            project.key_for_ref(StationRef::new(o, 0)).unwrap(),
        );
        project.loop_closures.push(crate::models::LoopClosure {
            id,
            station_a,
            station_b,
            path_length: 0.4,
            misclosure_distance: 0.4,
            error_percentage: 100.0,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.0,
            strategy: crate::models::ClosureStrategy::ProportionalByLength,
            is_aligned: false,
        });
    }
    let merge = state.get_project_state().unwrap().station_merges[1].clone();
    let project = state.get_project_state().unwrap();
    let (a, b) = (
        project.resolve_key(&merge.station_a).unwrap(),
        project.resolve_key(&merge.station_b).unwrap(),
    );
    assert_eq!(
        [a, b]
            .iter()
            .filter(|s| [x, o].contains(&s.line_id))
            .count(),
        2,
        "setup: the second merge is X's end to O's start"
    );

    let warnings = combine_removing(
        &state,
        StationRef::new(k, 1),
        (k, LineEnd::End),
        (o, LineEnd::Start),
        &[(a, b, CombineRemovedKind::Merge)],
    );
    // Each station by its line's and its own name before the combine (seeded
    // from its position): O is gone after the combine.
    let name = |at: StationRef| {
        let line = if at.line_id == x { "X" } else { "O" };
        format!("{line} / {}", at.station_index)
    };
    assert_eq!(
        warnings,
        vec![format!(
            "Removed the merge of {} and {}: combining the lines left its two stations at \
             the two ends of one shot.",
            name(a),
            name(b)
        )]
    );
    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 1);
}

/// #746, an ordinary closure the join alone puts across one shot. K runs two
/// shots east; O starts on K's end (co-located, not merged) and loops back
/// north, west and south onto K's start, merged there; a `Batch` closure
/// ties K's middle station to O's start. Combined at K's end, O's start is
/// K's end and the closure spans K's last shot: the combine removes it, and
/// the merge that would tie the combined line to itself tears into a
/// closure around the real loop.
#[test]
fn a_combine_removes_a_closure_its_join_puts_across_one_shot() {
    use crate::models::{CombineRemovedKind, LineEnd};
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Loop".to_string(),
        location: None,
    });
    let k = create(&state, "K", vec![shot(90.0, 5.0), shot(90.0, 5.0)]);
    let o = create(
        &state,
        "O",
        vec![shot(0.0, 4.0), shot(270.0, 10.0), shot(180.0, 4.0)],
    );
    for command in [
        Command::MergeStations {
            station_a: StationRef::new(k, 0),
            station_b: StationRef::new(o, 3),
        },
        Command::CreateLoopClosure {
            station_a: StationRef::new(k, 1),
            station_b: StationRef::new(o, 0),
            closure_strategy: crate::models::ClosureStrategy::Batch,
        },
    ] {
        let result = state.execute(command);
        assert!(result.success, "{:?}", result.error);
    }

    let warnings = combine_removing(
        &state,
        StationRef::new(k, 2),
        (k, LineEnd::End),
        (o, LineEnd::Start),
        &[(
            StationRef::new(k, 1),
            StationRef::new(o, 0),
            CombineRemovedKind::LoopClosure,
        )],
    );
    assert_eq!(
        warnings,
        // By the names before the combine: K is "Joined" now, O is gone.
        vec![
            "Removed loop closure K / 1 to O / 0: combining the lines left its two \
             stations at the two ends of one shot."
                .to_string()
        ]
    );
    let project = state.get_project_state().unwrap();
    assert!(project.station_merges.is_empty());
    assert_eq!(project.loop_closures.len(), 1, "the torn merge's closure");
}

/// A placement datum on K's end and a GPS fix on O's start, the merged join
/// stations (#741 D2): the combine is refused and changes nothing, so the two
/// datums never share one station, and Remove Zero Datum on either station
/// removes that one datum only.
#[test]
fn a_combine_that_would_put_two_datums_on_one_station_is_refused() {
    let (state, k, o) = state();
    let g1 = GpsCoordinates::new(20.0, -87.0, 0.0).unwrap();
    let g2 = GpsCoordinates::new(20.0001, -87.0, 0.0).unwrap();
    for (at, g) in [(StationRef::new(k, 2), g1), (StationRef::new(o, 0), g2)] {
        let r = state.execute(Command::SetZeroDatum {
            station_ref: at,
            coordinates: g,
            accuracy_m: None,
            entrance: None,
        });
        assert!(r.success, "{:?}", r.error);
    }
    let before = snapshot(&state);
    let result = state.execute_combine_lines(k, o, options());
    assert!(!result.success);
    let error = result.error.unwrap_or_default();
    assert!(error.contains("each hold a zero datum"), "{error}");
    assert_eq!(snapshot(&state), before, "a refusal changes nothing");

    let r = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(o, 0),
    });
    assert!(r.success, "{:?}", r.error);
    let p = state.get_project_state().unwrap();
    assert_eq!(p.zero_datums.len(), 1, "only the fix went");
    assert_eq!(
        p.resolve_key(&p.zero_datums[0].station_ref),
        Some(StationRef::new(k, 2))
    );
    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before, "undo restores the fix");
}

/// #749, the smallest survey, through the command and a saved file. B
/// (created first, so its start is where the Closures Computed View holds
/// the survey) runs 10 m east, 9.6 m back west and 5 m north; a legacy
/// closure (one the solve does not take, as a file from before the deferred
/// strategies carries it) ties B:2, 0.4 m east of B's start, back to B's
/// start. A runs one shot 4 m south, its end merged to B:2. Combining A's end
/// with B's start keeps A, which moves 0.4 m onto B's start, and tears the
/// merge into a `Batch` closure on the legacy closure's two stations. That
/// closure is the one left there, so the view solves the joined survey as
/// the plan said it would, held where B's start was, and the plan lists the
/// legacy closure as replaced by the torn merge's; one Undo restores the
/// project byte for byte, Redo re-applies it, and a save and a reopen keep
/// it. No warning: the connection stays.
#[test]
fn a_combine_keeps_the_closure_the_solve_takes_over_a_legacy_one() {
    use crate::models::{ClosureStrategy, CombineJoinEnds, KeptMovesReason, LineEnd, LoopClosure};
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("legacy-loop.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Legacy loop".to_string(),
        location: None,
    });
    let b = create(
        &state,
        "B",
        vec![shot(90.0, 10.0), shot(270.0, 9.6), shot(0.0, 5.0)],
    );
    let a = create(&state, "A", vec![shot(180.0, 4.0)]);
    assert!(b < a);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 2),
    });
    assert!(merged.success, "{:?}", merged.error);
    let legacy = {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        let id = project.next_closure_id;
        project.next_closure_id += 1;
        let (station_a, station_b) = (
            project.key_for_ref(StationRef::new(b, 2)).unwrap(),
            project.key_for_ref(StationRef::new(b, 0)).unwrap(),
        );
        project.loop_closures.push(LoopClosure {
            id,
            station_a,
            station_b,
            path_length: 19.6,
            misclosure_distance: 0.4,
            error_percentage: 0.4 / 19.6 * 100.0,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::ProportionalByLength,
            is_aligned: false,
        });
        id
    };
    // As a file carries it: saved, reopened, still the legacy closure.
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|c| (c.id, c.strategy))
            .collect::<Vec<_>>(),
        vec![(legacy, ClosureStrategy::ProportionalByLength)]
    );
    let b0 = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();

    let candidate = state
        .combine_candidates(StationRef::new(a, 1))
        .unwrap()
        .into_iter()
        .find(|c| [c.line_a, c.line_b] == [b, a])
        .unwrap();
    let plan = candidate.plan_keeping_b;
    assert_eq!(candidate.line_b, a);
    assert_eq!(plan.moving_line, Some(a));
    assert_eq!(
        plan.kept_moves_because,
        Some(KeptMovesReason::AdjustmentAnchor)
    );
    assert_eq!(plan.torn_ties.len(), 1, "the merge becomes a loop closure");
    // The dialog lists the legacy closure the torn merge's closure replaces.
    assert_eq!(
        plan.closures_replaced,
        Some(vec![crate::models::CombineReplacedClosure {
            station_a: StationRef::new(b, 2),
            station_b: StationRef::new(b, 0),
            kind: crate::models::CombineReplacedKind::LegacyClosure,
            by_station_a: StationRef::new(a, 1),
            by_station_b: StationRef::new(b, 2),
            by_kind: crate::models::CombineRemovedKind::Merge,
        }])
    );
    let before = snapshot(&state);

    let result = state.execute_combine_lines(
        a,
        b,
        CombineLinesOptions {
            join: Some(CombineJoinEnds {
                kept: LineEnd::End,
                other: LineEnd::Start,
            }),
            ..options()
        },
    );
    assert!(result.success, "{:?}", result.error);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    // B's stations follow A's two: B:0 is the joined A:1, B:2 is A:3.
    let check = |state: &AppState, what: &str| {
        let project = state.get_project_state().unwrap();
        assert!(project.station_merges.is_empty(), "{what}");
        let left: Vec<_> = project
            .loop_closures
            .iter()
            .map(|c| {
                let mut ends = [
                    project.resolve_key(&c.station_a).unwrap(),
                    project.resolve_key(&c.station_b).unwrap(),
                ];
                ends.sort_by_key(|s| s.station_index);
                (c.id == legacy, c.strategy, ends)
            })
            .collect();
        assert_eq!(
            left,
            vec![(
                false,
                ClosureStrategy::Batch,
                [StationRef::new(a, 1), StationRef::new(a, 3)]
            )],
            "{what}: the torn merge's closure, not the legacy one"
        );
        let adjusted =
            project.compute_adjusted_stations(&crate::config::ClosureAdjustmentConfig::default());
        assert!(adjusted.failures.is_empty(), "{what}");
        let at = |index: u32| {
            let s = adjusted
                .stations
                .iter()
                .find(|s| s.station_ref == StationRef::new(a, index))
                .unwrap();
            (s.coordinates.x, s.coordinates.y)
        };
        let (joined, b2) = (at(1), at(3));
        assert!(
            (joined.0 - b0.x).hypot(joined.1 - b0.y) < 1e-6,
            "{what}: held where B's start was"
        );
        assert!(
            (joined.0 - b2.0).hypot(joined.1 - b2.1) < 1e-6,
            "{what}: the Closures Computed View closes the loop"
        );
    };
    check(&state, "combine");
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert_eq!(
        snapshot(&state),
        before,
        "one undo restores the legacy closure"
    );
    assert!(state.redo().success);
    assert_eq!(snapshot(&state), after);
    check(&state, "redo");

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(
        state
            .open_project(temp.path().join("legacy-loop.stalx"))
            .success,
        "reopen"
    );
    check(&state, "reopen");
}

/// Every station's measured position, in line and station order.
fn station_positions(state: &AppState) -> Vec<(StationRef, (f64, f64))> {
    let project = state.get_project_state().unwrap();
    project
        .lines
        .iter()
        .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| StationRef::new(s.id, i)))
        .map(|at| {
            let c = project.compute_station_raw_coords(at).unwrap();
            (at, (c.x, c.y))
        })
        .collect()
}

fn apart(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// #740 through the command, as the issue found it in the app: M runs north
/// from the origin with O (east) merged to its start by O's end, and K
/// (east), moved by -20.0006 m (Adjust Position), ends 0.6 mm short of O's
/// start: side by side, not merged. Combining there keeping K moves O by the
/// residual, with M merged to it, so O's far end (K's station 2 now) stays
/// on M's start and K stays put. One Undo restores the project byte for
/// byte, Redo re-applies it, and a save and a reopen move nothing. Before
/// #740 the plan moved nothing, the merged pair ended 0.6 mm apart, and the
/// reopen moved K by 0.6 mm.
#[test]
fn a_side_by_side_combine_keeps_merged_stations_together_through_a_reopen() {
    use crate::models::{CombineJoinEnds, CombineJoinKind, Coordinates, LineEnd};
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("side-by-side.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Side by side".to_string(),
        location: None,
    });
    let m = create(&state, "M", vec![shot(0.0, 10.0)]);
    let o = create(&state, "O", vec![shot(90.0, 10.0)]);
    let k = create(&state, "K", vec![shot(90.0, 10.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(o, 1),
        station_b: StationRef::new(m, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    let moved = state.execute(Command::MoveLine {
        line_id: k,
        offset: Coordinates::new(-20.0006, 0.0, 0.0),
    });
    assert!(moved.success, "{:?}", moved.error);
    let at = |state: &AppState, station: StationRef| {
        station_positions(state)
            .into_iter()
            .find(|(s, _)| *s == station)
            .unwrap()
            .1
    };
    let gap = apart(
        at(&state, StationRef::new(k, 1)),
        at(&state, StationRef::new(o, 0)),
    );
    assert!(
        (gap - 0.0006).abs() < 1e-9,
        "setup: the ends are {gap} m apart"
    );

    let candidate = state.combine_candidate_for_lines(k, o).unwrap().unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated);
    let plan = if candidate.line_a == k {
        candidate.plan_keeping_a
    } else {
        candidate.plan_keeping_b
    };
    assert_eq!(plan.moving_line, Some(o));
    assert_eq!(plan.moved_lines, vec![o, m]);
    assert!((plan.shift_m - 0.0006).abs() < 1e-9, "{}", plan.shift_m);
    let k0 = at(&state, StationRef::new(k, 0));
    let before = snapshot(&state);

    let result = state.execute_combine_lines(
        k,
        o,
        CombineLinesOptions {
            join: Some(CombineJoinEnds {
                kept: LineEnd::End,
                other: LineEnd::Start,
            }),
            ..options()
        },
    );
    assert!(result.success, "{:?}", result.error);
    let check = |state: &AppState, what: &str| {
        assert!(
            apart(
                at(state, StationRef::new(k, 2)),
                at(state, StationRef::new(m, 0))
            ) < 1e-9,
            "{what}: the merged pair is one point"
        );
        assert!(
            apart(at(state, StationRef::new(k, 0)), k0) < 1e-12,
            "{what}: K stays"
        );
    };
    check(&state, "combine");
    let combined = station_positions(&state);
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before, "one undo restores it all");
    assert!(state.redo().success);
    assert_eq!(snapshot(&state), after);
    check(&state, "redo");

    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    check(&state, "reopen");
    let reopened = station_positions(&state);
    assert_eq!(reopened.len(), combined.len());
    for ((a, p), (b, q)) in combined.iter().zip(&reopened) {
        assert_eq!(a, b);
        assert!(apart(*p, *q) < 1e-9, "the reopen moved {a:?}");
    }
}

/// Every station of every line, as shown.
fn shown_stations(state: &AppState) -> Vec<(StationRef, (f64, f64))> {
    let project = state.get_project_state().unwrap();
    project
        .lines
        .iter()
        .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| StationRef::new(s.id, i)))
        .map(|at| {
            let c = project.compute_station_raw_coords(at).unwrap();
            (at, (c.x, c.y))
        })
        .collect()
}

/// #755 through the command, with the hand move read from a file as an
/// older one carries it (no command makes a manual station offset): K's end
/// and O's start side by side, O's start moved 0.3 m east and 0.2 m down by
/// hand, T merged to it. The joined station keeps K's hand move (none), so
/// the plan moves T with it by the 0.3 m; the command's result is exactly
/// that, with T on the joined station (the repair after the edit has
/// nothing to move) and no shot changed, one Undo restores every byte and
/// Redo repeats it, and a save and a reopen move nothing.
#[test]
fn a_hand_moved_join_station_keeps_its_branch_through_undo_and_a_reopen() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("hand-moved.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Hand moved".to_string(),
        location: None,
    });
    // Level shots, each line at its own depth: K 2 m, O 3 m, T 5 m.
    let level = |azimuth: f64, length: f64, depth: f64| ShotData {
        depth_in: depth,
        depth_out: depth,
        ..shot(azimuth, length)
    };
    let k = create(&state, "K", vec![level(90.0, 10.0, 2.0)]);
    let o = create(&state, "O", vec![level(90.0, 10.0, 3.0)]);
    let t = create(&state, "T", vec![level(0.0, 5.0, 5.0)]);
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        project.set_line_offset(o, (10.0, 0.0));
        project.set_line_offset(t, (10.3, 0.0));
        let key = project.key_for_ref(StationRef::new(o, 0)).unwrap();
        project.station_offsets.insert(key, (0.3, 0.0, 0.2));
    }
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(o, 0),
        station_b: StationRef::new(t, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path.clone()).success, "reopen");

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.station_offset_for_ref(StationRef::new(o, 0)),
        Some((0.3, 0.0, 0.2)),
        "the hand move comes from the file"
    );
    let depth = |project: &crate::models::Project, at: StationRef| {
        project.compute_station_raw_coords(at).unwrap().z
    };
    // O's start and T's read the average of their two readings, O's start
    // plus its hand move in depth.
    assert!((depth(&project, StationRef::new(o, 0)) - 4.2).abs() < 1e-9);
    assert!((depth(&project, StationRef::new(t, 0)) - 4.0).abs() < 1e-9);
    let candidate = project.combine_candidate_for_lines(k, o).unwrap();
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moved_lines, vec![o]);
    assert!(plan.shift_m < 1e-9, "the chain gap is 0: {}", plan.shift_m);
    let joined = plan.moved_with_joined_station.clone().unwrap();
    assert_eq!(joined.lines, vec![t], "T moves with the joined station");
    assert!((joined.shift_m - 0.3).abs() < 1e-9, "{}", joined.shift_m);
    let before = snapshot(&state);
    let was = shown_stations(&state);
    // The combine alone, without the command's repair after it.
    let mut planned = project;
    planned.combine_lines(k, o, &options()).unwrap();

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        state.get_project_state().unwrap().line_offsets,
        planned.line_offsets,
        "the repair after the edit moved a line: the combine left a merge open"
    );
    let now: std::collections::HashMap<StationRef, (f64, f64)> =
        shown_stations(&state).into_iter().collect();
    let dist = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1);
    // K stays, O's start lands on K's end and its far end stays, T moves the
    // 0.3 m with the joined station and stays on it.
    for (at, want) in [
        (StationRef::new(k, 0), (0.0, 0.0)),
        (StationRef::new(k, 1), (10.0, 0.0)),
        (StationRef::new(k, 2), (20.0, 0.0)),
        (StationRef::new(t, 0), (10.0, 0.0)),
        (StationRef::new(t, 1), (10.0, 5.0)),
    ] {
        assert!(dist(now[&at], want) < 1e-9, "{at:?} at {:?}", now[&at]);
    }
    let was: std::collections::HashMap<StationRef, (f64, f64)> = was.into_iter().collect();
    let t0 = StationRef::new(t, 0);
    assert!(
        (dist(now[&t0], was[&t0]) - joined.shift_m).abs() < 1e-9,
        "T moved as planned, no more"
    );
    let combined = state.get_project_state().unwrap();
    assert!(
        combined.station_offsets.is_empty(),
        "the joined station keeps K's hand move: none"
    );
    let lengths: Vec<f64> = combined.lines[0].shots.iter().map(|s| s.length).collect();
    assert_eq!(lengths, vec![10.0, 10.0], "no shot changes");
    // The joined station and T read the average of the three readings there
    // (K's, O's, T's); the joined station keeps K's hand move in depth, none,
    // so O's 0.2 m goes with the rest of its hand move.
    let joined_depth = (2.0 + 3.0 + 5.0) / 3.0;
    for at in [StationRef::new(k, 1), t0] {
        let now = depth(&combined, at);
        assert!((now - joined_depth).abs() < 1e-9, "{at:?} at depth {now}");
    }
    assert!(combined.check_structural_integrity().is_empty());
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before, "one undo restores every byte");
    assert!(state.redo().success);
    assert_eq!(snapshot(&state), after, "redo repeats every byte");

    let shown = shown_stations(&state);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen after the combine");
    let reopened: std::collections::HashMap<StationRef, (f64, f64)> =
        shown_stations(&state).into_iter().collect();
    for (at, p) in shown {
        assert!(dist(reopened[&at], p) < 1e-9, "the reopen moved {at:?}");
    }
    let reopened = state.get_project_state().unwrap();
    for at in [StationRef::new(k, 1), t0] {
        let now = depth(&reopened, at);
        assert!(
            (now - joined_depth).abs() < 1e-9,
            "reopened, {at:?} at depth {now}"
        );
    }
}
