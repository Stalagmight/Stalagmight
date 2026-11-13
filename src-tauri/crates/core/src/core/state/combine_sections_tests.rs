//! Combine Sections through `AppState`: one undo entry, refusals that change
//! nothing, the frame, persistence and ignored warnings.

use super::AppState;
use crate::core::Command;
use crate::models::{
    CombinePick, CombineSectionsOptions, GpsCoordinates, SectionId, ShotData, StationRef,
};

/// A shot credited to its own surveyor, so the snapshot comparisons below
/// cover per-shot survey credit through the combine, its undo and its redo.
fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        credit: Some(crate::models::SurveyCredit {
            surveyors: crate::models::Person::named(&format!("Surveyor {azimuth}/{length}"))
                .into_iter()
                .collect(),
            explorers: Vec::new(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 4, 1),
        }),
        ..Default::default()
    }
}

fn options() -> CombineSectionsOptions {
    CombineSectionsOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
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

/// K (two shots east) with O (two shots north, then a short one) merged to
/// K's end.
fn state() -> (AppState, SectionId, SectionId) {
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

fn shot_counts(state: &AppState) -> Vec<(SectionId, usize)> {
    state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| (s.id, s.shots.len()))
        .collect()
}

#[test]
fn a_combine_is_one_undo_step_and_redoes() {
    let (state, k, o) = state();
    let before = snapshot(&state);

    let result = state.execute_combine_sections(k, o, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.section_id, Some(k));
    assert_eq!(shot_counts(&state), vec![(k, 5)]);
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before, "one undo restores it all");
    assert!(state.redo().success);
    assert_eq!(snapshot(&state), after);
}

/// A blocked combine is refused before anything happens: nothing changes and
/// no undo entry is pushed, so the next undo reverts the merge before it.
#[test]
fn a_refused_combine_changes_nothing_and_leaves_no_undo_entry() {
    let (state, k, o) = state();
    // A third section at the join makes it a branch.
    let c = create(&state, "C", vec![shot(180.0, 3.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(c, 0),
        station_b: StationRef::new(k, 2),
    });
    assert!(merged.success, "{:?}", merged.error);
    let before = snapshot(&state);
    let depth = state.get_undo_stack().len();

    let result = state.execute_combine_sections(k, o, options());
    assert!(!result.success);
    assert!(result.error.unwrap().contains("branch"));
    assert_eq!(snapshot(&state), before);
    assert_eq!(state.get_undo_stack().len(), depth);

    for (kept, other) in [(k, k), (k, 999)] {
        assert!(
            !state
                .execute_combine_sections(kept, other, options())
                .success
        );
        assert_eq!(snapshot(&state), before);
        assert_eq!(state.get_undo_stack().len(), depth);
    }
}

/// The frame datum on the other section moves to the kept one: the station
/// that defines the georeference is re-keyed, and the survey is re-framed so
/// that station still sits at local (0, 0).
#[test]
fn a_datum_on_the_other_section_keeps_the_frame() {
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
    });
    assert!(datum.success, "{:?}", datum.error);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 1),
        station_b: StationRef::new(o, 0),
    });
    assert!(merged.success, "{:?}", merged.error);

    let result = state.execute_combine_sections(k, o, options());
    assert!(result.success, "{:?}", result.error);

    let project = state.get_project_state().unwrap();
    let frame = project.frame_datum_key().unwrap();
    assert_eq!(frame.section_id, k, "the frame datum is re-keyed onto K");
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
    assert!(state.execute_combine_sections(k, o, options()).success);
    let before = snapshot_sections(&state);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");

    assert_eq!(snapshot_sections(&state), before);
    let project = state.get_project_state().unwrap();
    assert!(project.get_section(o).is_none());
    assert!(project.check_structural_integrity().is_empty());
}

/// The sections as plain data: id, name, and every shot's uuid and length.
fn snapshot_sections(state: &AppState) -> Vec<(SectionId, Option<String>, Vec<(String, f64)>)> {
    state
        .get_project_state()
        .unwrap()
        .sections
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

/// `SHORT_SHOT` verdicts (is it ignored?) by section.
fn short_shot_verdicts(state: &AppState) -> Vec<(SectionId, bool)> {
    let project = state.get_project_state().unwrap();
    state
        .compute_project_warnings(&project, None)
        .into_iter()
        .filter(|w| w.code == "SHORT_SHOT")
        .map(|w| (w.section_id, w.is_resolved))
        .collect()
}

/// An ignore on one of the other section's shots follows the shot into the
/// kept section, in the session and through a save and a reopen.
#[test]
fn an_ignored_warning_follows_its_shot_into_the_kept_section() {
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
        let shot = &project.get_section(o).unwrap().shots[2];
        (shot.id, shot.uuid.clone().unwrap())
    };
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: o,
            shot_id: short_id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: Some(short_uuid),
        })
        .unwrap();
    assert_eq!(short_shot_verdicts(&state), vec![(o, true)]);

    assert!(state.execute_combine_sections(k, o, options()).success);
    assert_eq!(short_shot_verdicts(&state), vec![(k, true)]);

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(short_shot_verdicts(&state), vec![(k, true)]);
}

/// The combine reverses the other section inside the same undo step when they
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

    let result = state.execute_combine_sections(k, o, options());
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    let shots = &project.get_section(k).unwrap().shots;
    assert_eq!(shots[1].azimuth_in, 225.0, "O's last shot, reciprocated");

    assert!(state.undo().success);
    assert_eq!(snapshot(&state), before);
}

/// The station query answers `None` for an unknown section and for a station
/// that is not a section end, including one past the end (from #713).
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
            state.combine_candidate(station).unwrap(),
            None,
            "{station:?}"
        );
    }
    assert!(state
        .combine_candidate(StationRef::new(k, 2))
        .unwrap()
        .is_some());
    assert_eq!(state.combine_candidate_for_sections(k, 999).unwrap(), None);
    assert_eq!(state.combine_candidate_for_sections(k, k).unwrap(), None);
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

    let result = state.execute_combine_sections(o, k, options());

    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.section_id, Some(o));
    assert_eq!(shot_counts(&state), vec![(o, 5)]);
    let project = state.get_project_state().unwrap();
    assert!(
        project.get_section(k).is_none(),
        "the lower id is the one removed"
    );
    let moved = xy(&state, StationRef::new(o, 0));
    assert!((moved.0 - far.0).hypot(moved.1 - far.1) < 1e-9);
}

/// Out of shot ids on the kept side: refused before anything happens, no undo
/// entry; keeping the other section works.
#[test]
fn a_kept_section_without_shot_id_room_is_refused_with_no_undo_entry() {
    let (state, k, o) = state();
    let mut project = state.get_project_state().unwrap();
    project.get_section_mut(k).unwrap().next_shot_id =
        (k + 1) * crate::models::FEATURE_ID_STRIDE - 1;
    state.set_project_for_tests(project);
    let before = snapshot(&state);
    let depth = state.get_undo_stack().len();

    let refused = state.execute_combine_sections(k, o, options());
    assert!(!refused.success);
    assert!(refused.error.unwrap().contains("used up its shot numbers"));
    assert_eq!(snapshot(&state), before);
    assert_eq!(state.get_undo_stack().len(), depth);

    assert!(state.execute_combine_sections(o, k, options()).success);
}

/// A POSITION-keyed ignore on a shot of the other section, which the combine
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
        .get_section(o)
        .unwrap()
        .shots[0]
        .id;
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: o,
            shot_id: short_id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        })
        .unwrap();
    assert_eq!(short_shot_verdicts(&state), vec![(o, true)]);

    let result = state.execute_combine_sections(k, o, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(short_shot_verdicts(&state), vec![(k, true)]);

    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(short_shot_verdicts(&state), vec![(k, true)]);
}

/// Shots that join a section of the other parity keep their own record
/// parity (`counter_parity_shots`), through a save and a reopen.
#[test]
fn the_record_parity_of_the_other_sections_shots_survives_a_save() {
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
            .execute(Command::ReverseSection {
                section_id: k,
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
        .get_section(o)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect();

    assert!(state.execute_combine_sections(k, o, options()).success);
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
/// component is anchored afterwards by a third section that does not hold it
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

    assert!(state.execute_combine_sections(k, o, options()).success);
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
    let uuid_of = |section: SectionId, index: usize| {
        project.get_section(section).unwrap().shots[index]
            .uuid
            .clone()
            .unwrap()
    };
    state
        .compute_project_warnings(&project, None)
        .into_iter()
        .filter(|w| w.code == "SHORT_SHOT")
        .map(|w| (uuid_of(w.section_id, w.shot_index), w.is_resolved))
        .collect()
}

/// A short shot added to O since the last save, its `SHORT_SHOT` ignored the
/// way the resolve command stores it (by position, since its uuid is not on
/// disk yet). Returns the state, both sections, the save path and the new
/// shot's uuid.
fn unsaved_ignored_shot(
    temp: &tempfile::TempDir,
) -> (AppState, SectionId, SectionId, std::path::PathBuf, String) {
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
        .get_section(o)
        .unwrap()
        .shots[2]
        .id;
    let created = state.execute(Command::CreateShot {
        section_id: o,
        shot: shot(0.0, 0.2),
        after_shot_id: Some(last),
        before_shot_id: None,
    });
    assert!(created.success, "{:?}", created.error);
    let (id, uuid) = {
        let project = state.get_project_state().unwrap();
        let shot = &project.get_section(o).unwrap().shots[3];
        (shot.id, shot.uuid.clone().unwrap())
    };
    let keyed = state.shot_uuid_is_persisted(o, &uuid).then(|| uuid.clone());
    assert_eq!(keyed, None, "setup: the new shot's uuid is not on disk");
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: o,
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
/// combine (as the other section's, which renumbers every one of its shots)
/// and a split past it, in the session and through a save and a reopen.
#[test]
fn an_ignore_on_an_unsaved_shot_follows_it_through_a_combine_and_a_split() {
    let temp = tempfile::TempDir::new().unwrap();
    let (state, k, o, path, uuid) = unsaved_ignored_shot(&temp);
    let ignored = |state: &AppState| short_shot_verdicts_by_uuid(state).get(&uuid).copied();

    assert!(state.execute_combine_sections(k, o, options()).success);
    assert_eq!(ignored(&state), Some(true), "after the combine");
    let split = state.execute_split_section(
        k,
        crate::models::SplitAt::Station { station_index: 3 },
        crate::models::SplitSectionOptions {
            name_a: "K-A".to_string(),
            name_b: "K-B".to_string(),
            color_a: "#111111".to_string(),
            color_b: "#222222".to_string(),
            comment_side: crate::models::SplitSide::A,
            datum_side: crate::models::SplitSide::A,
        },
    );
    assert!(split.success, "{:?}", split.error);
    let second = split.section_id.unwrap();
    let holder = state
        .get_project_state()
        .unwrap()
        .sections
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

    assert!(state.execute_combine_sections(k, o, options()).success);
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
