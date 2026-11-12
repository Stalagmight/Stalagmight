//! Per-shot survey credit through the command path: `EditShotCredit`, its one
//! undo step, the legacy section-level surveyor/date, inheritance by new shots,
//! and old undo payloads that still carry a section surveyor.

use chrono::NaiveDate;

use super::AppState;
use crate::core::{Command, UndoData};
use crate::models::{
    CreditEdit, CreditTargets, ImportPlacement, ImportSectionConfig, Person, SectionId,
    SectionUpdates, ShotData, ShotId, SurveyCredit,
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

fn create_section(state: &AppState, surveyor: Option<&str>, shots: Vec<ShotData>) -> SectionId {
    let r = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: surveyor.map(str::to_string),
        date: None,
        color: "#123456".to_string(),
        direction: None,
        shots,
    });
    assert!(r.success, "{:?}", r.error);
    r.section_id.unwrap()
}

fn credits(state: &AppState, section_id: SectionId) -> Vec<SurveyCredit> {
    let project = state.get_project_state().unwrap();
    project
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.credit().clone())
        .collect()
}

fn shot_ids(state: &AppState, section_id: SectionId) -> Vec<ShotId> {
    let project = state.get_project_state().unwrap();
    project
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

fn undo_len(state: &AppState) -> usize {
    state.get_undo_stack().len()
}

#[test]
fn a_section_wide_edit_is_one_undo_step_and_round_trips() {
    let state = state_with_project();
    let sid = create_section(&state, None, vec![shot(1.0), shot(2.0), shot(3.0)]);
    let before = credits(&state, sid);
    let steps = undo_len(&state);

    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Section { section_id: sid },
        edit: CreditEdit {
            add_surveyors: Some(vec![person("Ann"), person("Bo")]),
            set_date: Some(day(3)),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.section_ids, vec![sid]);
    let after = credits(&state, sid);
    assert!(after
        .iter()
        .all(|c| c == &credit(&["Ann", "Bo"], &[], day(3))));
    assert_eq!(undo_len(&state), steps + 1, "one step for three shots");

    let top = &state.get_undo_stack()[0];
    assert_eq!(top.description, "Edit survey credit (3 shots)");
    assert_eq!(top.command_type, "edit_shot_credit");
    let rows: Vec<(&str, &str, &str)> = top
        .preview_data
        .as_ref()
        .unwrap()
        .changes
        .iter()
        .map(|c| (c.field.as_str(), c.old_value.as_str(), c.new_value.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("Surveyors", "", "Ann, Bo"),
            ("Date", "", "2024-05-03"),
            ("Shots", "", "3"),
        ]
    );

    assert!(state.undo().success);
    assert_eq!(credits(&state, sid), before);
    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), after);
}

/// A data-table paste of credit cells: one `EditShotCredit` per shot inside
/// ONE batch. Each sub-command applies, and one undo takes them all back.
#[test]
fn credit_edits_run_inside_a_batch_as_one_undo_step() {
    let state = state_with_project();
    let sid = create_section(&state, None, vec![shot(1.0), shot(2.0)]);
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

/// Edit Section's Save: a rename and a credit edit in ONE batch are one undo
/// step - a single undo restores both, and nothing else is left to undo.
#[test]
fn a_rename_and_a_credit_edit_batch_into_one_undo_step() {
    let state = state_with_project();
    let sid = create_section(&state, None, vec![shot(1.0), shot(2.0)]);
    let before = credits(&state, sid);
    let steps = undo_len(&state);

    let r = state.execute(Command::Batch {
        commands: vec![
            Command::UpdateSectionMetadata {
                section_id: sid,
                updates: SectionUpdates {
                    name: Some(Some("Renamed".to_string())),
                    ..Default::default()
                },
            },
            Command::EditShotCredit {
                targets: CreditTargets::Section { section_id: sid },
                edit: CreditEdit {
                    add_surveyors: Some(vec![person("Ann")]),
                    ..Default::default()
                },
            },
        ],
    });
    assert!(r.success, "{:?}", r.error);
    let name = |state: &AppState| {
        state
            .get_project_state()
            .unwrap()
            .get_section(sid)
            .unwrap()
            .name
            .clone()
    };
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
fn explicit_targets_touch_only_those_shots_and_label_the_role() {
    let state = state_with_project();
    let sid = create_section(&state, Some("Old"), vec![shot(1.0), shot(2.0), shot(3.0)]);
    let ids = shot_ids(&state, sid);
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Shots {
            shots: vec![(sid, ids[0]), (sid, ids[2]), (sid, ids[0])],
        },
        edit: CreditEdit {
            remove_surveyors: Some(vec![person("old")]),
            add_surveyors: Some(vec![person("New")]),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    let now = credits(&state, sid);
    assert_eq!(now[0], credit(&["New"], &[], None));
    assert_eq!(now[1], credit(&["Old"], &[], None));
    assert_eq!(now[2], credit(&["New"], &[], None));
    assert_eq!(
        state.get_undo_stack()[0].description,
        "Edit surveyors (2 shots)"
    );
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
    let sid = create_section(&state, None, vec![shot(1.0)]);
    let steps = undo_len(&state);
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Shots {
            shots: vec![(sid, 999_999)],
        },
        edit: CreditEdit {
            set_date: Some(day(1)),
            ..Default::default()
        },
    });
    assert!(!r.success);
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Section { section_id: 77 },
        edit: CreditEdit::default(),
    });
    assert!(!r.success);
    assert_eq!(undo_len(&state), steps);
}

#[test]
fn an_edit_that_changes_nothing_pushes_no_undo_step() {
    let state = state_with_project();
    let sid = create_section(&state, Some("Ann"), vec![shot(1.0)]);
    let steps = undo_len(&state);
    let r = state.execute(Command::EditShotCredit {
        targets: CreditTargets::Section { section_id: sid },
        edit: CreditEdit {
            add_surveyors: Some(vec![person("ann")]),
            ..Default::default()
        },
    });
    assert!(r.success);
    assert_eq!(undo_len(&state), steps);
}

#[test]
fn the_undo_record_addresses_shots_by_uuid_and_survives_json() {
    let state = state_with_project();
    let sid = create_section(&state, None, vec![shot(1.0), shot(2.0)]);
    assert!(
        state
            .execute(Command::EditShotCredit {
                targets: CreditTargets::Section { section_id: sid },
                edit: CreditEdit {
                    set_explorers: Some(vec![person("Ex")]),
                    ..Default::default()
                },
            })
            .success
    );
    let undo_data = state
        .undo_stack
        .lock()
        .unwrap()
        .peek_undo_data()
        .cloned()
        .unwrap();
    let UndoData::ShotCredits { changes, .. } = &undo_data else {
        panic!("expected ShotCredits, got {}", undo_data.variant_name());
    };
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| !c.shot_uuid.is_empty()));
    let json = serde_json::to_string(&undo_data).unwrap();
    assert!(
        !json.contains("shot_id"),
        "in-session ids are not persisted: {json}"
    );
    let back: UndoData = serde_json::from_str(&json).unwrap();
    let UndoData::ShotCredits { changes: back, .. } = back else {
        panic!("round trip changed the variant");
    };
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].after, credit(&[], &["Ex"], None));
}

#[test]
fn a_legacy_section_surveyor_edit_sets_every_shot_and_undoes_exactly() {
    let state = state_with_project();
    let sid = create_section(&state, None, vec![shot(1.0), shot(2.0), shot(3.0)]);
    let ids = shot_ids(&state, sid);
    // Three different credits, so a section-level undo could not restore them.
    for (i, id) in ids.iter().enumerate() {
        let name = format!("P{i}");
        assert!(
            state
                .execute(Command::EditShotCredit {
                    targets: CreditTargets::Shots {
                        shots: vec![(sid, *id)],
                    },
                    edit: CreditEdit {
                        set_surveyors: Some(vec![person(&name)]),
                        set_date: Some(day(u32::try_from(i).unwrap() + 1)),
                        ..Default::default()
                    },
                })
                .success
        );
    }
    let before = credits(&state, sid);

    let r = state.execute(Command::UpdateSectionMetadata {
        section_id: sid,
        updates: SectionUpdates {
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

    // The history shows the legacy edit like any section edit, its old value
    // read off the per-shot record.
    let top = &state.get_undo_stack()[0];
    assert_eq!(top.command_type, "update_section_metadata");
    let preview = top.preview_data.as_ref().unwrap();
    let surveyor = preview
        .changes
        .iter()
        .find(|c| c.field == "Surveyor")
        .unwrap();
    assert_eq!(surveyor.old_value, "P0, P1, P2");
    assert_eq!(surveyor.new_value, "nh / ds");

    assert!(state.undo().success);
    assert_eq!(credits(&state, sid), before);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.get_section(sid).unwrap().name.as_deref(), Some("S"));

    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), after);
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.get_section(sid).unwrap().name.as_deref(),
        Some("Renamed")
    );
}

#[test]
fn a_legacy_date_edit_sets_every_shot() {
    let state = state_with_project();
    let sid = create_section(&state, Some("A"), vec![shot(1.0), shot(2.0)]);
    assert!(
        state
            .execute(Command::UpdateSectionMetadata {
                section_id: sid,
                updates: SectionUpdates {
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
    let sid = create_section(&state, None, vec![shot(1.0), shot(2.0)]);
    let ids = shot_ids(&state, sid);
    let first = credit(&["First"], &[], day(1));
    let second = credit(&["Second"], &["X"], day(2));
    for (id, c) in [(ids[0], &first), (ids[1], &second)] {
        assert!(
            state
                .execute(Command::EditShotCredit {
                    targets: CreditTargets::Shots {
                        shots: vec![(sid, id)],
                    },
                    edit: CreditEdit {
                        set_surveyors: Some(c.surveyors.clone()),
                        set_explorers: Some(c.explorers.clone()),
                        set_date: Some(c.date),
                        ..Default::default()
                    },
                })
                .success
        );
    }

    // Append: the previous (last) shot.
    let appended = state.execute(Command::CreateShot {
        section_id: sid,
        shot: shot(3.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(appended.success);
    // Insert after the first: the first.
    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sid,
                shot: shot(4.0),
                after_shot_id: Some(ids[0]),
                before_shot_id: None,
            })
            .success
    );
    // Insert before the first: the next one, i.e. the old first.
    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sid,
                shot: shot(5.0),
                after_shot_id: None,
                before_shot_id: Some(ids[0]),
            })
            .success
    );
    // An explicit credit is kept.
    let own = credit(&["Own"], &[], None);
    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sid,
                shot: ShotData {
                    credit: Some(own.clone()),
                    ..shot(6.0)
                },
                after_shot_id: None,
                before_shot_id: None,
            })
            .success
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
    let sid = create_section(&state, Some("Split Person"), vec![shot(4.0), shot(4.0)]);
    let ids = shot_ids(&state, sid);
    let r = state.execute_split_shot_at_distance(sid, ids[0], 1.5);
    assert!(r.success, "{:?}", r.error);
    let now = credits(&state, sid);
    assert_eq!(now.len(), 3);
    assert!(now
        .iter()
        .all(|c| c == &credit(&["Split Person"], &[], None)));
}

#[test]
fn an_old_snapshot_with_a_section_surveyor_restores_people_onto_shots() {
    let state = state_with_project();
    let sid = create_section(&state, None, vec![shot(1.0), shot(2.0)]);
    let snapshot = state.capture_project_snapshot().unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    // What a build before per-shot credit wrote: section-level text, shots bare.
    let section = json["sections"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|s| s["id"] == sid)
        .unwrap();
    section["surveyor"] = serde_json::json!("nh / ds");
    section["date"] = serde_json::json!("2019-07-01");
    let old = json.to_string();

    let mut slot = None;
    assert!(AppState::restore_snapshot_into(&mut slot, &old));
    let project = slot.unwrap();
    let section = project.get_section(sid).unwrap();
    assert!(section.legacy_surveyor.is_none());
    for shot in &section.shots {
        assert_eq!(
            shot.credit(),
            &credit(&["ds"], &["nh"], NaiveDate::from_ymd_opt(2019, 7, 1))
        );
    }
}

#[test]
fn an_old_created_section_entry_with_a_surveyor_restores_people_onto_shots() {
    let state = state_with_project();
    let sid = create_section(&state, None, vec![shot(1.0)]);
    // The redo of a `CreateSection` entry persisted before per-shot credit:
    // its `SectionData` still carries the section-level surveyor.
    let r = state.restore_section(
        sid + 1,
        crate::models::SectionData {
            surveyor: Some("Doe, John".to_string()),
            shots: vec![shot(2.0)],
            ..Default::default()
        },
        &crate::models::SectionIdentity::default(),
    );
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        credits(&state, sid + 1),
        vec![credit(&["Doe, John"], &[], None)]
    );
}

#[test]
fn import_overrides_replace_every_shots_credit() {
    let state = state_with_project();
    let mut file_shot = shot(1.0);
    file_shot.credit = Some(credit(&["From File"], &["Ex"], day(1)));
    let config = ImportSectionConfig {
        section_index: 0,
        name: Some("Imported".to_string()),
        surveyor: None,
        date: None,
        surveyors: Some(vec![person("Dialog")]),
        explorers: None,
        override_date: day(20),
        color: "#ffffff".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![file_shot, shot(2.0)],
        placement: ImportPlacement::Origin,
        station_comments: Vec::new(),
        station_names: Vec::new(),
        reverse: false,
        azimuth_reference: None,
    };
    let r = state.execute_import_sections(
        vec![config],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        crate::core::VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);
    let sid = r.section_ids[0];
    assert_eq!(
        credits(&state, sid),
        vec![
            credit(&["Dialog"], &["Ex"], day(20)),
            credit(&["Dialog"], &[], day(20)),
        ]
    );
}

#[test]
fn a_credit_edit_undoes_after_save_and_reopen() {
    let _config = crate::test_helpers::isolated_config();
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("credit.stalx");
    let (sid, before, after) = {
        let state = state_with_project();
        let sid = create_section(&state, Some("nh / ds"), vec![shot(1.0), shot(2.0)]);
        let before = credits(&state, sid);
        let ids = shot_ids(&state, sid);
        assert!(
            state
                .execute(Command::EditShotCredit {
                    targets: CreditTargets::Shots {
                        shots: vec![(sid, ids[1])],
                    },
                    edit: CreditEdit {
                        add_explorers: Some(vec![Person::new("Mail", Some("m@x.org")).unwrap()]),
                        set_date: Some(day(7)),
                        ..Default::default()
                    },
                })
                .success
        );
        let after = credits(&state, sid);
        let saved = state.save_project(Some(path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
        (sid, before, after)
    };

    let state = AppState::new().unwrap();
    let opened = state.open_project(path);
    assert!(opened.success, "open failed: {:?}", opened.error);
    assert_eq!(credits(&state, sid), after, "the credit survives the file");
    // Shot ids were renumbered by the load; the entry resolves by uuid.
    assert!(state.undo().success);
    assert_eq!(credits(&state, sid), before);
    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), after);
}

#[test]
fn a_shotless_section_keeps_its_surveyor_until_its_first_shot_takes_it() {
    let state = state_with_project();
    let sid = create_section(&state, Some("nh / ds"), Vec::new());
    let legacy = |state: &AppState| {
        let project = state.get_project_state().unwrap();
        let section = project.get_section(sid).unwrap();
        (section.legacy_surveyor.clone(), section.legacy_date)
    };
    assert_eq!(legacy(&state), (Some("nh / ds".to_string()), None));

    // A legacy edit of a shot-less section edits the pair, undoably.
    assert!(
        state
            .execute(Command::UpdateSectionMetadata {
                section_id: sid,
                updates: SectionUpdates {
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

    // The first shot takes the pair as its credit; the section lets it go.
    let appended = state.execute(Command::CreateShot {
        section_id: sid,
        shot: shot(1.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(appended.success, "{:?}", appended.error);
    assert_eq!(credits(&state, sid), vec![credit(&["ds"], &["nh"], None)]);
    assert_eq!(legacy(&state), (None, None));

    // Undo: shot-less again, the pair is back; redo: the shot takes it again.
    assert!(state.undo().success);
    assert_eq!(legacy(&state), (Some("nh / ds".to_string()), None));
    assert!(state.redo().success);
    assert_eq!(credits(&state, sid), vec![credit(&["ds"], &["nh"], None)]);
    assert_eq!(legacy(&state), (None, None));
}
