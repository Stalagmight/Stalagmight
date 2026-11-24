//! Per-shot survey credit through the command path on the station graph:
//! `EditShotCredit`, its one undo step (the pipeline's changeset, which keys
//! shots by uuid and interns each credit once), the legacy line-level
//! surveyor/date, and inheritance by new shots.
//!
//! History labels and preview rows are the history-labels package's
//! (`history_labels_tests.rs`); these tests pin what the edit does.

use std::sync::Arc;

use chrono::NaiveDate;

use super::commands::Record;
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{Project, ShotPlace};
use crate::models::{
    CreditEdit, CreditTargets, ImportLineConfig, ImportPlacement, LineId, LineUpdates, Person,
    ShotData, ShotId, SurveyCredit,
};

fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 30.0,
        azimuth_out: 30.0,
        length,
        ..Default::default()
    }
}

fn person(name: &str) -> Person {
    Person::named(name).unwrap()
}

fn credit(surveyors: &[&str], explorers: &[&str], date: Option<NaiveDate>) -> SurveyCredit {
    SurveyCredit {
        surveyors: surveyors.iter().map(|n| person(n)).collect(),
        explorers: explorers.iter().map(|n| person(n)).collect(),
        date,
    }
}

fn day(d: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(2024, 5, d)
}

fn state_with_project() -> AppState {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Credit".to_string(),
                location: None,
            })
            .success
    );
    state
}

fn create_line(state: &AppState, surveyor: Option<String>, shots: Vec<ShotData>) -> LineId {
    let r = state.execute(Command::CreateLine {
        name: Some("S".to_string()),
        surveyor,
        date: None,
        color: "#123456".to_string(),
        direction: None,
        shots,
        cave_section_id: None,
    });
    assert!(r.success, "{:?}", r.error);
    r.line_id.unwrap()
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn credits(state: &AppState, line_id: LineId) -> Vec<SurveyCredit> {
    project(state)
        .line(line_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.credit.as_deref().cloned().unwrap_or_default())
        .collect()
}

fn shot_ids(state: &AppState, line_id: LineId) -> Vec<ShotId> {
    project(state)
        .line(line_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

fn undo_len(state: &AppState) -> usize {
    state.get_undo_stack().len()
}

fn edit_shots(state: &AppState, shots: Vec<(LineId, ShotId)>, edit: CreditEdit) -> CommandResult {
    state.execute(Command::EditShotCredit {
        targets: CreditTargets::Shots { shots },
        edit,
    })
}

/// Run a shot operation of the model through the execute pipeline as one
/// recorded edit (the shot family's commands are another package's).
fn run_model(
    state: &AppState,
    f: impl FnOnce(&mut Project) -> Result<(), String>,
) -> CommandResult {
    let recorded = state.run_recorded(
        |_| Record {
            kind: "create_shot".to_string(),
            label: "Create shot".to_string(),
            ..Record::default()
        },
        |p, _| f(p),
    );
    match recorded {
        Ok(()) => CommandResult::success(),
        Err(failure) => failure,
    }
}

fn add_shot(state: &AppState, line: LineId, data: ShotData, place: ShotPlace) {
    let r = run_model(state, |p| {
        p.create_shot(line, data, place)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
}

/// The canonical text of the newest undo entry (what a save writes).
fn newest_entry_text(state: &AppState) -> String {
    state
        .undo_stack
        .lock()
        .unwrap()
        .peek_undo()
        .unwrap()
        .raw()
        .as_str()
        .to_string()
}

#[test]
fn a_line_wide_edit_is_one_undo_step_and_round_trips() {
    let state = state_with_project();
    let sid = create_line(&state, None, vec![shot(1.0), shot(2.0), shot(3.0)]);
    let before = credits(&state, sid);
    let steps = undo_len(&state);

    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Line { line_id: sid },
        edit: CreditEdit {
            add_surveyors: Some(vec![person("Ann"), person("Bo")]),
            set_date: Some(day(3)),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.line_ids, vec![sid]);
    let after = credits(&state, sid);
    assert!(after
        .iter()
        .all(|c| c == &credit(&["Ann", "Bo"], &[], day(3))));
    assert_eq!(undo_len(&state), steps + 1, "one step for three shots");
    assert_eq!(state.get_undo_stack()[0].command_type, "edit_shot_credit");

    assert!(state.undo().success);
    assert_eq!(credits(&state, sid), before);
    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), after);
}

/// Shots that held one credit before share one allocation after, as an
/// import's do (#719 review, item 5a).
#[test]
fn a_line_wide_edit_stores_one_shared_credit() {
    let state = state_with_project();
    let sid = create_line(
        &state,
        Some("Ann".to_string()),
        vec![shot(1.0), shot(2.0), shot(3.0)],
    );
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Line { line_id: sid },
        edit: CreditEdit {
            set_date: Some(day(8)),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    let shots = &p.line(sid).unwrap().shots;
    let first = shots[0].credit.as_ref().unwrap();
    assert!(shots
        .iter()
        .all(|s| s.credit.as_ref().is_some_and(|c| Arc::ptr_eq(c, first))));
    assert_eq!(**first, credit(&["Ann"], &[], day(8)));
}

/// A data-table paste of credit cells: one `EditShotCredit` per shot inside
/// ONE batch. Each sub-command applies, and one undo takes them all back.
#[test]
fn credit_edits_run_inside_a_batch_as_one_undo_step() {
    let state = state_with_project();
    let sid = create_line(&state, None, vec![shot(1.0), shot(2.0)]);
    let ids = shot_ids(&state, sid);
    let before = credits(&state, sid);
    let steps = undo_len(&state);
    let set = |shot_id: ShotId, name: &str| Command::EditShotCredit {
        targets: CreditTargets::Shots {
            shots: vec![(sid, shot_id)],
        },
        edit: CreditEdit {
            set_surveyors: Some(vec![person(name)]),
            ..Default::default()
        },
    };

    let r = state.execute(Command::Batch {
        commands: vec![set(ids[0], "Ann"), set(ids[1], "Bo")],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        credits(&state, sid),
        vec![credit(&["Ann"], &[], None), credit(&["Bo"], &[], None)]
    );
    assert_eq!(undo_len(&state), steps + 1);
    assert!(state.undo().success);
    assert_eq!(credits(&state, sid), before);
}

/// Edit Line's Save: a rename and a credit edit in ONE batch are one undo
/// step - a single undo restores both, and nothing else is left to undo.
#[test]
fn a_rename_and_a_credit_edit_batch_into_one_undo_step() {
    let state = state_with_project();
    let sid = create_line(&state, None, vec![shot(1.0), shot(2.0)]);
    let before = credits(&state, sid);
    let steps = undo_len(&state);

    let r = state.execute(Command::Batch {
        commands: vec![
            Command::UpdateLineMetadata {
                line_id: sid,
                updates: LineUpdates {
                    name: Some(Some("Renamed".to_string())),
                    ..Default::default()
                },
            },
            Command::EditShotCredit {
                targets: CreditTargets::Line { line_id: sid },
                edit: CreditEdit {
                    add_surveyors: Some(vec![person("Ann")]),
                    ..Default::default()
                },
            },
        ],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    let name = |state: &AppState| project(state).line(sid).unwrap().name.clone();
    assert_eq!(name(&state).as_deref(), Some("Renamed"));
    assert!(credits(&state, sid)
        .iter()
        .all(|c| c == &credit(&["Ann"], &[], None)));
    assert_eq!(undo_len(&state), steps + 1, "one step for the whole Save");

    assert!(state.undo().success);
    assert_eq!(name(&state).as_deref(), Some("S"));
    assert_eq!(credits(&state, sid), before);
    assert_eq!(undo_len(&state), steps);
}

#[test]
fn explicit_targets_touch_only_those_shots() {
    let state = state_with_project();
    let sid = create_line(
        &state,
        Some("Old".to_string()),
        vec![shot(1.0), shot(2.0), shot(3.0)],
    );
    let ids = shot_ids(&state, sid);
    let r = edit_shots(
        &state,
        vec![(sid, ids[0]), (sid, ids[2]), (sid, ids[0])],
        CreditEdit {
            remove_surveyors: Some(vec![person("old")]),
            add_surveyors: Some(vec![person("New")]),
            ..Default::default()
        },
    );
    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.line_ids, vec![sid]);
    let now = credits(&state, sid);
    assert_eq!(now[0], credit(&["New"], &[], None));
    assert_eq!(now[1], credit(&["Old"], &[], None));
    assert_eq!(now[2], credit(&["New"], &[], None));
    assert!(state.undo().success);
    assert!(credits(&state, sid)
        .iter()
        .all(|c| c == &credit(&["Old"], &[], None)));
    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), now);
}

#[test]
fn an_unknown_target_fails_and_records_nothing() {
    let state = state_with_project();
    let sid = create_line(&state, None, vec![shot(1.0)]);
    let steps = undo_len(&state);
    let r = edit_shots(
        &state,
        vec![(sid, 999_999)],
        CreditEdit {
            set_date: Some(day(1)),
            ..Default::default()
        },
    );
    assert_eq!(r.error.as_deref(), Some(crate::models::SHOT_GONE));
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Line { line_id: 77 },
        edit: CreditEdit::default(),
    });
    assert_eq!(r.error.as_deref(), Some(crate::models::LINE_GONE));
    let r = edit_shots(&state, vec![(77, 0)], CreditEdit::default());
    assert_eq!(r.error.as_deref(), Some(crate::models::LINE_GONE));
    assert_eq!(undo_len(&state), steps);
}

#[test]
fn an_oversized_selection_is_refused() {
    let state = state_with_project();
    let sid = create_line(&state, None, vec![shot(1.0)]);
    let id = shot_ids(&state, sid)[0];
    let r = edit_shots(&state, vec![(sid, id); 100_001], CreditEdit::default());
    assert_eq!(
        r.error.as_deref(),
        Some("A credit edit can name at most 100000 shots")
    );
}

#[test]
fn an_edit_that_changes_nothing_pushes_no_undo_step() {
    let state = state_with_project();
    let sid = create_line(&state, Some("Ann".to_string()), vec![shot(1.0)]);
    let steps = undo_len(&state);
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Line { line_id: sid },
        edit: CreditEdit {
            add_surveyors: Some(vec![person("ann")]),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(undo_len(&state), steps);
}

#[test]
fn a_shotless_lines_credit_edit_succeeds_and_changes_nothing() {
    let state = state_with_project();
    let sid = create_line(&state, None, Vec::new());
    let steps = undo_len(&state);
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Line { line_id: sid },
        edit: CreditEdit {
            set_date: Some(day(2)),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    assert!(r.line_ids.is_empty(), "no shot was named");
    assert_eq!(undo_len(&state), steps);
}

/// The entry never names a session shot id (a reload mints new ones; the
/// changeset addresses shots by position under each line's content hash)
/// and writes each credit once however many shots it covers.
#[test]
fn the_undo_record_holds_no_session_ids_and_writes_each_credit_once() {
    let state = state_with_project();
    let sid = create_line(&state, None, vec![shot(1.0), shot(2.0)]);
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Line { line_id: sid },
        edit: CreditEdit {
            set_explorers: Some(vec![person("Explorer Ex")]),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    let text = newest_entry_text(&state);
    assert_eq!(
        text.matches("Explorer Ex").count(),
        1,
        "the credit is interned once: {text}"
    );
    assert!(!text.contains("shot_id"), "no session ids: {text}");
}

#[test]
fn a_legacy_line_surveyor_edit_sets_every_shot_and_undoes_exactly() {
    let state = state_with_project();
    let sid = create_line(&state, None, vec![shot(1.0), shot(2.0), shot(3.0)]);
    let ids = shot_ids(&state, sid);
    // Three different credits, so a line-level undo could not restore them.
    for (i, id) in ids.iter().enumerate() {
        let name = format!("P{i}");
        assert!(
            edit_shots(
                &state,
                vec![(sid, *id)],
                CreditEdit {
                    set_surveyors: Some(vec![person(&name)]),
                    set_date: Some(day(u32::try_from(i).unwrap() + 1)),
                    ..Default::default()
                },
            )
            .success
        );
    }
    let before = credits(&state, sid);

    let r = state.execute(Command::UpdateLineMetadata {
        line_id: sid,
        updates: LineUpdates {
            name: Some(Some("Renamed".to_string())),
            surveyor: Some(Some("nh / ds".to_string())),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    let after = credits(&state, sid);
    for (i, c) in after.iter().enumerate() {
        assert_eq!(
            c,
            &credit(&["ds"], &["nh"], day(u32::try_from(i).unwrap() + 1))
        );
    }
    assert_eq!(
        state.get_undo_stack()[0].command_type,
        "update_line_metadata"
    );

    assert!(state.undo().success);
    assert_eq!(credits(&state, sid), before);
    assert_eq!(
        project(&state).line(sid).unwrap().name.as_deref(),
        Some("S")
    );

    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), after);
    assert_eq!(
        project(&state).line(sid).unwrap().name.as_deref(),
        Some("Renamed")
    );
}

#[test]
fn a_legacy_date_edit_sets_every_shot() {
    let state = state_with_project();
    let sid = create_line(&state, Some("A".to_string()), vec![shot(1.0), shot(2.0)]);
    assert!(
        state
            .execute(Command::UpdateLineMetadata {
                line_id: sid,
                updates: LineUpdates {
                    date: Some(day(9)),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(credits(&state, sid)
        .iter()
        .all(|c| c == &credit(&["A"], &[], day(9))));
    assert!(state.undo().success);
    assert!(credits(&state, sid)
        .iter()
        .all(|c| c == &credit(&["A"], &[], None)));
}

#[test]
fn a_new_shot_inherits_its_neighbours_credit() {
    let state = state_with_project();
    let sid = create_line(&state, None, vec![shot(1.0), shot(2.0)]);
    let ids = shot_ids(&state, sid);
    let first = credit(&["First"], &[], day(1));
    let second = credit(&["Second"], &["X"], day(2));
    for (id, c) in [(ids[0], &first), (ids[1], &second)] {
        assert!(
            edit_shots(
                &state,
                vec![(sid, id)],
                CreditEdit {
                    set_surveyors: Some(c.surveyors.clone()),
                    set_explorers: Some(c.explorers.clone()),
                    set_date: Some(c.date),
                    ..Default::default()
                },
            )
            .success
        );
    }

    // Append: the previous (last) shot.
    add_shot(&state, sid, shot(3.0), ShotPlace::End);
    // Insert after the first: the first.
    add_shot(&state, sid, shot(4.0), ShotPlace::After(ids[0]));
    // Insert before the first: the next one, i.e. the old first.
    add_shot(&state, sid, shot(5.0), ShotPlace::Before(ids[0]));
    // An explicit credit is kept.
    let own = credit(&["Own"], &[], None);
    add_shot(
        &state,
        sid,
        ShotData {
            credit: Some(own.clone().into()),
            ..shot(6.0)
        },
        ShotPlace::End,
    );
    // Order now: 5.0(new), 1.0, 4.0(new), 2.0, 3.0(new), 6.0(own)
    let now = credits(&state, sid);
    assert_eq!(
        now,
        vec![
            first.clone(),
            first.clone(),
            first,
            second.clone(),
            second,
            own
        ]
    );

    // Redo replays the inherited credit, not a fresh inheritance.
    assert!(state.undo().success);
    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), now);
}

#[test]
fn a_split_gives_both_pieces_the_shots_credit() {
    let state = state_with_project();
    let sid = create_line(
        &state,
        Some("Split Person".to_string()),
        vec![shot(4.0), shot(4.0)],
    );
    let ids = shot_ids(&state, sid);
    let r = run_model(&state, |p| {
        p.split_shot_at_distance(sid, ids[0], 1.5, None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
    let now = credits(&state, sid);
    assert_eq!(now.len(), 3);
    assert!(now
        .iter()
        .all(|c| c == &credit(&["Split Person"], &[], None)));
}

fn import_config(shots: Vec<ShotData>, surveyors: Option<Vec<Person>>) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: None,
        line_index: 0,
        name: Some("Imported".to_string()),
        surveyor: None,
        date: None,
        surveyors,
        explorers: None,
        override_date: None,
        color: "#ffffff".to_string(),
        direction: None,
        quality: 1.0,
        shots,
        placement: ImportPlacement::Origin,
        station_comments: Vec::new(),
        station_names: Vec::new(),
        station_external: Vec::new(),
        reverse: false,
        azimuth_reference: None,
    }
}

#[test]
fn import_overrides_replace_every_shots_credit() {
    let state = state_with_project();
    let mut file_shot = shot(1.0);
    file_shot.credit = Some(credit(&["From File"], &["Ex"], day(1)).into());
    let config = ImportLineConfig {
        override_date: day(20),
        ..import_config(vec![file_shot, shot(2.0)], Some(vec![person("Dialog")]))
    };
    let r = state.execute_import_lines(
        vec![config],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        crate::core::VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);
    let sid = r.line_ids[0];
    assert_eq!(
        credits(&state, sid),
        vec![
            credit(&["Dialog"], &["Ex"], day(20)),
            credit(&["Dialog"], &[], day(20)),
        ]
    );
}

/// #719 review, item 5a: an import that gives every shot of a survey ONE
/// credit (a Compass or Walls team line) costs one allocation.
#[test]
fn an_imported_team_credit_stays_one_shared_allocation() {
    for overrides in [false, true] {
        let state = state_with_project();
        let team = Arc::new(credit(&["Ann Smith", "Bo"], &[], day(3)));
        let shots: Vec<ShotData> = (1..=4)
            .map(|i| ShotData {
                credit: Some(Arc::clone(&team)),
                ..shot(f64::from(i))
            })
            .collect();
        let config = import_config(shots, overrides.then(|| vec![person("Dialog")]));
        let r = state.execute_import_lines(
            vec![config],
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            crate::core::VirtualShotMode::Auto,
        );
        assert!(r.success, "{:?}", r.error);
        let p = project(&state);
        let line = p.line(r.line_ids[0]).unwrap();
        let first = line.shots[0].credit.as_ref().unwrap();
        assert!(
            line.shots
                .iter()
                .all(|s| s.credit.as_ref().is_some_and(|c| Arc::ptr_eq(c, first))),
            "overrides: {overrides}"
        );
        if !overrides {
            assert!(Arc::ptr_eq(first, &team));
        }
    }
}

#[test]
fn a_credit_edit_undoes_after_save_and_reopen() {
    let _config = crate::test_helpers::isolated_config();
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("credit.stalx");
    let (sid, before, after) = {
        let state = state_with_project();
        let sid = create_line(
            &state,
            Some("nh / ds".to_string()),
            vec![shot(1.0), shot(2.0)],
        );
        let before = credits(&state, sid);
        let ids = shot_ids(&state, sid);
        assert!(
            edit_shots(
                &state,
                vec![(sid, ids[1])],
                CreditEdit {
                    add_explorers: Some(vec![Person::new("Mail", Some("m@x.org")).unwrap()]),
                    set_date: Some(day(7)),
                    ..Default::default()
                },
            )
            .success
        );
        let after = credits(&state, sid);
        let saved = state.execute(Command::SaveProject {
            path: Some(path.clone()),
        });
        assert!(saved.success, "save failed: {:?}", saved.error);
        (sid, before, after)
    };

    let state = AppState::new().unwrap();
    let opened = state.execute(Command::OpenProject { path });
    assert!(opened.success, "open failed: {:?}", opened.error);
    assert_eq!(credits(&state, sid), after, "the credit survives the file");
    // Session shot ids are minted anew by the load; the entry keys by uuid.
    assert!(state.undo().success);
    assert_eq!(credits(&state, sid), before);
    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), after);
}

/// A selection across lines - out of order, with gaps, over shots with
/// different previous credits - undoes and redoes exactly after a save and
/// a reopen, where every session shot id is new.
#[test]
fn a_credit_edit_over_runs_undoes_exactly_after_save_and_reopen() {
    let _config = crate::test_helpers::isolated_config();
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("runs.stalx");
    let (lines, before, after) = {
        let state = state_with_project();
        let lines: Vec<LineId> = (0..3)
            .map(|_| create_line(&state, None, (1..=12).map(|i| shot(f64::from(i))).collect()))
            .collect();
        // Shots 4..8 of the middle line start from another credit.
        let middle = shot_ids(&state, lines[1]);
        assert!(
            edit_shots(
                &state,
                middle[4..8].iter().map(|id| (lines[1], *id)).collect(),
                CreditEdit {
                    set_surveyors: Some(vec![person("Earlier")]),
                    ..Default::default()
                },
            )
            .success
        );
        let mut targets: Vec<(LineId, ShotId)> = Vec::new();
        for line in lines.iter().rev() {
            let ids = shot_ids(&state, *line);
            // Two runs per line with a gap between, the second listed first.
            targets.extend(ids[7..11].iter().rev().map(|id| (*line, *id)));
            targets.extend(ids[1..6].iter().map(|id| (*line, *id)));
        }
        let before: Vec<Vec<SurveyCredit>> = lines.iter().map(|l| credits(&state, *l)).collect();
        let r = edit_shots(
            &state,
            targets,
            CreditEdit {
                add_surveyors: Some(vec![person("Runs")]),
                set_date: Some(day(9)),
                ..Default::default()
            },
        );
        assert!(r.success, "{:?}", r.error);
        let mut touched = r.line_ids;
        touched.sort_unstable();
        assert_eq!(touched, lines, "every line the selection names");
        let text = newest_entry_text(&state);
        assert_eq!(
            text.matches("\"Earlier\"").count(),
            2,
            "each distinct credit is written once per side: {text}"
        );
        let after: Vec<Vec<SurveyCredit>> = lines.iter().map(|l| credits(&state, *l)).collect();
        let saved = state.execute(Command::SaveProject {
            path: Some(path.clone()),
        });
        assert!(saved.success, "save failed: {:?}", saved.error);
        (lines, before, after)
    };

    let state = AppState::new().unwrap();
    assert!(state.execute(Command::OpenProject { path }).success);
    let now = |state: &AppState| -> Vec<Vec<SurveyCredit>> {
        lines.iter().map(|l| credits(state, *l)).collect()
    };
    assert_eq!(now(&state), after);
    assert!(state.undo().success);
    assert_eq!(now(&state), before);
    assert!(state.redo().success);
    assert_eq!(now(&state), after);
    assert!(state.undo().success);
    assert_eq!(now(&state), before);
}

#[test]
fn a_shotless_line_keeps_its_surveyor_until_its_first_shot_takes_it() {
    let state = state_with_project();
    let sid = create_line(&state, Some("nh / ds".to_string()), Vec::new());
    let legacy = |state: &AppState| {
        let p = project(state);
        let line = p.line(sid).unwrap();
        (line.legacy_surveyor.clone(), line.legacy_date)
    };
    assert_eq!(legacy(&state), (Some("nh / ds".to_string()), None));

    // A legacy edit of a shot-less line edits the pair, undoably.
    assert!(
        state
            .execute(Command::UpdateLineMetadata {
                line_id: sid,
                updates: LineUpdates {
                    surveyor: Some(Some("Solo".to_string())),
                    date: Some(day(4)),
                    ..Default::default()
                },
            })
            .success
    );
    assert_eq!(legacy(&state), (Some("Solo".to_string()), day(4)));
    assert!(state.undo().success);
    assert_eq!(legacy(&state), (Some("nh / ds".to_string()), None));

    // The first shot takes the pair as its credit; the line lets it go.
    add_shot(&state, sid, shot(1.0), ShotPlace::End);
    assert_eq!(credits(&state, sid), vec![credit(&["ds"], &["nh"], None)]);
    assert_eq!(legacy(&state), (None, None));

    // Undo: shot-less again, the pair is back; redo: the shot takes it again.
    assert!(state.undo().success);
    assert_eq!(legacy(&state), (Some("nh / ds".to_string()), None));
    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), vec![credit(&["ds"], &["nh"], None)]);
    assert_eq!(legacy(&state), (None, None));
}
