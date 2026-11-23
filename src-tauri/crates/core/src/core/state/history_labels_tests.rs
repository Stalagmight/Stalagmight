//! The undo history as the user reads it: one case per command kind, its
//! label and preview after the edit, and the same item after an undo, a
//! redo, a save and a reopen (an entry is worded once, at record time, and
//! read back from the file alone).
//!
//! The commands the core switch has ported run through `execute`; the others
//! run through the same pipeline (`run_recorded` with `Record::of`, the
//! record every plain command gets) with the survey operation their family
//! applies, so the label and preview are exactly what the family's entry
//! will carry: both depend only on the record and the changeset.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

use super::commands::Record;
use super::labels::{self, ShotsDeleted};
use super::AppState;
use crate::core::{Command, CommentHolder, UndoHistoryItem};
use crate::models::survey::changeset::{decode_entry, encode_entry, EntryLimits};
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{
    ClosureStrategy, Coordinates, CreditEdit, CreditTargets, GpsCoordinates, LineId, LineUpdates,
    Person, Section, ShotData, ShotKind, ShotUpdates, StationRef, SurveyCredit, SurveyDirection,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

fn shot(length: f64, azimuth: f64) -> ShotData {
    ShotData {
        length,
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        depth_in: 1.0,
        depth_out: 1.5,
        ..ShotData::default()
    }
}

fn named(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#336699".to_string(),
        ..LineMeta::default()
    }
}

/// Lines (ids in creation order): `Main` (4 shots, placed at the origin,
/// stations `0`..`4`), `Branch` (2 shots off Main's station 2), `Side` (3
/// shots, a free piece at 100 m east, stations `0`..`3`).
fn survey() -> Project {
    let mut p = Project::new("Labels", None);
    let main = p
        .add_line(
            named("Main"),
            StartAt::New,
            vec![
                shot(10.0, 0.0),
                shot(12.0, 45.0),
                shot(8.0, 90.0),
                shot(6.0, 120.0),
            ],
        )
        .unwrap();
    let start = p.line(main).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    let junction = p.line(main).unwrap().stations().nth(2).unwrap();
    p.add_line(
        named("Branch"),
        StartAt::Station(junction),
        vec![shot(5.0, 180.0), shot(6.0, 200.0)],
    )
    .unwrap();
    let side = p
        .add_line(
            named("Side"),
            StartAt::New,
            vec![shot(4.0, 10.0), shot(4.5, 20.0), shot(5.0, 30.0)],
        )
        .unwrap();
    let side_start = p.line(side).unwrap().start;
    p.set_placement(side_start, 100.0, 0.0).unwrap();
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    p
}

const MAIN: LineId = 0;
const BRANCH: LineId = 1;
const SIDE: LineId = 2;

/// An open project on a version 2 file in its own folder.
struct Open {
    _dir: TempDir,
    path: PathBuf,
    state: AppState,
}

fn open_on(project: &Project) -> Open {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("labels.stalx");
    std::fs::write(
        &path,
        stalx_v2::encode(project, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject { path: path.clone() });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(survey_of(&state).lines()[0].id, MAIN);
    Open {
        _dir: dir,
        path,
        state,
    }
}

fn open() -> Open {
    open_on(&survey())
}

fn survey_of(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn shot_id(state: &AppState, line: LineId, index: usize) -> u32 {
    survey_of(state).line(line).unwrap().shots[index].id
}

fn exec(state: &AppState, cmd: Command) {
    let r = state.execute(cmd);
    assert!(r.success, "{:?}", r.error);
}

/// Record `cmd` through the pipeline with the survey operation `op` as its
/// mutation (see the module docs).
fn record(state: &AppState, cmd: &Command, op: impl FnOnce(&mut Project) -> Result<(), String>) {
    let r = state.run_recorded(|p| Record::of(p, cmd), |p, _| op(p));
    assert!(r.is_ok(), "{:?}", r.err().and_then(|f| f.error));
}

fn top(state: &AppState) -> UndoHistoryItem {
    let item = state.get_undo_stack().into_iter().next().unwrap();
    assert_parts_say_the_words(&item);
    item
}

/// RS-PROSE: an item's parts say exactly its description, name something,
/// and carry no slot (they are persisted).
fn assert_parts_say_the_words(item: &UndoHistoryItem) {
    if let Some(parts) = &item.description_parts {
        assert_eq!(
            crate::models::prose::plain(parts),
            item.description,
            "{parts:?}"
        );
        assert!(parts.iter().any(crate::models::ProsePart::is_name));
        assert!(parts.iter().all(|p| match p {
            crate::models::ProsePart::Station(s) => s.slot.is_none(),
            _ => true,
        }));
    }
}

/// The names an item's parts draw as pills, by kind and words.
fn pills(item: &UndoHistoryItem) -> Vec<(&'static str, String)> {
    item.description_parts
        .iter()
        .flatten()
        .filter_map(|p| match p {
            crate::models::ProsePart::Text { .. } => None,
            crate::models::ProsePart::Station(s) => Some(("station", s.text.clone())),
            crate::models::ProsePart::Shot(s) => Some(("shot", s.text.clone())),
            crate::models::ProsePart::Line(l) => Some(("line", l.text.clone())),
        })
        .collect()
}

/// A recipe label's words, after checking that its parts say exactly them
/// and keep no slot (RS-PROSE).
fn words(label: labels::Label) -> String {
    if !label.parts.is_empty() {
        assert_eq!(crate::models::prose::plain(&label.parts), label.text);
        assert!(label.parts.iter().any(crate::models::ProsePart::is_name));
        assert!(label.parts.iter().all(|p| match p {
            crate::models::ProsePart::Station(s) => s.slot.is_none(),
            _ => true,
        }));
    }
    label.text
}

/// `(field, old, new)` of every preview row.
fn rows(item: &UndoHistoryItem) -> Vec<(String, String, String)> {
    item.preview_data
        .as_ref()
        .unwrap()
        .changes
        .iter()
        .map(|c| (c.field.clone(), c.old_value.clone(), c.new_value.clone()))
        .collect()
}

fn more(item: &UndoHistoryItem) -> usize {
    item.preview_data.as_ref().unwrap().additional_changes
}

fn r(field: &str, old: &str, new: &str) -> (String, String, String) {
    (field.to_string(), old.to_string(), new.to_string())
}

/// An item as the frontend reads it, its stack position aside.
fn shown(item: &UndoHistoryItem) -> serde_json::Value {
    let mut value = serde_json::to_value(item).unwrap();
    value.as_object_mut().unwrap().remove("index");
    value
}

/// The newest entry reads the same on the redo stack after an undo, back on
/// the undo stack after the redo, and after a save and a reopen of the file.
fn assert_survives(open: &Open) -> UndoHistoryItem {
    let state = &open.state;
    let item = top(state);
    assert!(state.undo().success);
    assert_eq!(
        shown(&state.get_redo_stack()[0]),
        shown(&item),
        "after undo"
    );
    assert!(state.redo().success);
    assert_eq!(shown(&top(state)), shown(&item), "after redo");
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "{:?}", saved.error);
    let reopened = AppState::new().unwrap();
    let r = reopened.execute(Command::OpenProject {
        path: open.path.clone(),
    });
    assert!(r.success, "{:?}", r.error);
    assert!(!reopened.history_discarded());
    let back = reopened.get_undo_stack();
    assert_eq!(back.len(), state.get_undo_stack().len());
    assert_eq!(shown(&back[0]), shown(&item), "after a reopen");
    item
}

// ---- Shots ----------------------------------------------------------------

#[test]
fn an_update_shot_names_the_shot_and_lists_its_changed_cells() {
    let o = open();
    let shot_id = shot_id(&o.state, MAIN, 1);
    exec(
        &o.state,
        Command::UpdateShot {
            line_id: MAIN,
            shot_id,
            updates: ShotUpdates {
                length: Some(13.5),
                azimuth_in: Some(47.25),
                depth_out: Some(2.0),
                left: Some(Some(1.25)),
                comment: Some(Some("Low arch".to_string())),
                kind: Some(ShotKind::Jump),
                ..ShotUpdates::default()
            },
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Update shot Main: 1 \u{2192} 2");
    assert_eq!(item.command_type, "update_shot");
    assert_eq!(
        rows(&item),
        vec![
            r("Length", "12.00", "13.50"),
            r("Azimuth In", "45.0", "47.2"),
            r("Depth Out", "1.50", "2.00"),
            r("Left", "", "1.25"),
            r("Comment", "", "Low arch"),
        ]
    );
    assert_eq!(more(&item), 1, "the kind is the sixth cell");
}

#[test]
fn a_kind_change_names_the_shot_and_both_kinds() {
    let o = open();
    let shot_id = shot_id(&o.state, MAIN, 0);
    exec(
        &o.state,
        Command::SetShotKind {
            line_id: MAIN,
            shot_id,
            kind: ShotKind::Jump,
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Mark shot Main: 0 \u{2192} 1 as jump");
    assert_eq!(rows(&item), vec![r("Kind", "Permanent", "Jump")]);
}

#[test]
fn a_created_shot_shows_its_length_and_azimuth() {
    let o = open();
    let cmd = Command::CreateShot {
        line_id: MAIN,
        shot: shot(7.25, 33.0),
        after_shot_id: None,
        before_shot_id: None,
    };
    record(&o.state, &cmd, |p| {
        p.append_shot(MAIN, shot(7.25, 33.0))
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Create shot in line \"Main\"");
    assert_eq!(
        rows(&item),
        vec![r("Length", "", "7.25"), r("Azimuth", "", "33.0")]
    );
}

/// Named by its stations, like an update (demo finding I-9: "Delete shot in
/// line \"Side\"" did not say which shot went).
#[test]
fn a_deleted_shot_names_the_shot() {
    let o = open();
    let shot_id = shot_id(&o.state, SIDE, 1);
    let cmd = Command::DeleteShot {
        line_id: SIDE,
        shot_id,
    };
    record(&o.state, &cmd, |p| {
        p.delete_shot(SIDE, shot_id)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Delete shot Side: 1 \u{2192} 2");
    assert_eq!(
        rows(&item),
        vec![r("Action", "", "Delete shot Side: 1 \u{2192} 2")]
    );
}

// ---- Lines ----------------------------------------------------------------

#[test]
fn a_created_line_shows_its_name() {
    let o = open();
    let cmd = Command::CreateLine {
        name: Some("New".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(3.0, 0.0)],
        cave_section_id: None,
    };
    record(&o.state, &cmd, |p| {
        p.create_line(named("New"), StartAt::New, vec![shot(3.0, 0.0)], None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Create line \"New\"");
    assert_eq!(item.command_type, "create_line");
    assert_eq!(rows(&item), vec![r("Name", "", "New")]);
}

#[test]
fn a_deleted_line_is_named_by_the_name_it_had() {
    let o = open();
    exec(&o.state, Command::DeleteLine { line_id: SIDE });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Delete line \"Side\"");
    assert_eq!(
        rows(&item),
        vec![r("Name", "Side", ""), r("Shots", "3", "0")]
    );
}

#[test]
fn a_reverse_names_the_line() {
    let o = open();
    exec(
        &o.state,
        Command::ReverseLine {
            line_id: SIDE,
            swap_left_right: true,
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Reverse line \"Side\"");
    assert_eq!(rows(&item), vec![r("Action", "", "Reverse line \"Side\"")]);
}

#[test]
fn a_metadata_edit_lists_three_fields_and_counts_the_rest() {
    let o = open();
    let updates = LineUpdates {
        name: Some(Some("Main Renamed".to_string())),
        color: Some("#123456".to_string()),
        direction: Some(SurveyDirection::Out),
        quality: Some(2.5),
        ..LineUpdates::default()
    };
    let cmd = Command::UpdateLineMetadata {
        line_id: MAIN,
        updates: updates.clone(),
    };
    record(&o.state, &cmd, |p| {
        p.update_line_meta(MAIN, &updates)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    // Worded before the edit: the line is named by the name it had.
    assert_eq!(item.description, "Update line \"Main\" metadata");
    assert_eq!(
        rows(&item),
        vec![
            r("Name", "Main", "Main Renamed"),
            r("Color", "#336699", "#123456"),
            r("Direction", "in", "out"),
        ]
    );
    assert_eq!(more(&item), 1, "the closure weight is the fourth");
}

/// A survey whose Main shots 0..3 carry surveyors `P0`, `P1`, `P2` and
/// dates 2025-03-01..03.
fn credited() -> Project {
    let mut p = survey();
    let line = p.line_mut(MAIN).unwrap();
    for (i, shot) in line.shots.iter_mut().take(3).enumerate() {
        shot.credit = Some(Arc::new(SurveyCredit {
            surveyors: vec![Person::named(&format!("P{i}")).unwrap()],
            explorers: Vec::new(),
            date: chrono::NaiveDate::from_ymd_opt(2025, 3, 1 + i as u32),
        }));
    }
    p
}

#[test]
fn a_legacy_surveyor_and_date_edit_reads_the_shot_credits() {
    let o = open_on(&credited());
    let updates = LineUpdates {
        surveyor: Some(Some("nh / ds".to_string())),
        date: Some(chrono::NaiveDate::from_ymd_opt(2025, 4, 2)),
        ..LineUpdates::default()
    };
    let cmd = Command::UpdateLineMetadata {
        line_id: MAIN,
        updates: updates.clone(),
    };
    record(&o.state, &cmd, |p| {
        p.update_line_meta(MAIN, &updates)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(
        rows(&item),
        vec![
            r("Surveyor", "P0, P1, P2", "nh / ds"),
            r("Date", "2025-03-01", "2025-04-02"),
        ]
    );
}

/// Apply `edit` to the credit of every target (the credit family's rule).
fn edit_credit(p: &mut Project, targets: &[(LineId, usize)], edit: &CreditEdit) {
    for &(line_id, index) in targets {
        let shot = &mut p.line_mut(line_id).unwrap().shots[index];
        let before = shot.credit.as_deref().cloned().unwrap_or_default();
        let after = edit.apply(&before);
        if after != before {
            shot.credit = Some(Arc::new(after));
        }
    }
}

#[test]
fn a_credit_edit_counts_the_shots_it_changes_and_shows_the_names() {
    let o = open_on(&credited());
    let alice = Person::named("Alice").unwrap();
    let edit = CreditEdit {
        set_surveyors: Some(vec![alice]),
        set_date: Some(chrono::NaiveDate::from_ymd_opt(2025, 3, 4)),
        ..CreditEdit::default()
    };
    let cmd = Command::EditShotCredit {
        targets: CreditTargets::Line { line_id: MAIN },
        edit: edit.clone(),
    };
    record(&o.state, &cmd, |p| {
        edit_credit(p, &[(MAIN, 0), (MAIN, 1), (MAIN, 2), (MAIN, 3)], &edit);
        Ok(())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Edit survey credit (4 shots)");
    assert_eq!(item.command_type, "edit_shot_credit");
    assert_eq!(
        rows(&item),
        vec![
            r("Surveyors", "P0, P1, P2", "Alice"),
            r("Date", "2025-03-01 - 2025-03-03", "2025-03-04"),
            r("Shots", "", "4"),
        ]
    );

    // An explicit selection counts only the shots whose credit changes:
    // shot 0 already has Bob, shot 3 is not selected.
    let bob = Person::named("Bob").unwrap();
    let add = CreditEdit {
        add_surveyors: Some(vec![bob]),
        ..CreditEdit::default()
    };
    record(
        &o.state,
        &Command::EditShotCredit {
            targets: CreditTargets::Shots { shots: vec![] },
            edit: add.clone(),
        },
        |p| {
            edit_credit(p, &[(MAIN, 0)], &add);
            Ok(())
        },
    );
    let ids: Vec<(LineId, u32)> = (0..3).map(|i| (MAIN, shot_id(&o.state, MAIN, i))).collect();
    let cmd = Command::EditShotCredit {
        targets: CreditTargets::Shots { shots: ids },
        edit: add.clone(),
    };
    record(&o.state, &cmd, |p| {
        edit_credit(p, &[(MAIN, 0), (MAIN, 1), (MAIN, 2)], &add);
        Ok(())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Edit surveyors (2 shots)");
    assert_eq!(
        rows(&item),
        vec![r("Surveyors", "Alice", "Alice, Bob"), r("Shots", "", "2")]
    );
}

// ---- Stations, merges, closures, datums ------------------------------------

#[test]
fn a_merge_names_both_stations_as_they_were() {
    let o = open();
    exec(
        &o.state,
        Command::MergeStations {
            station_a: StationRef::new(SIDE, 0),
            station_b: StationRef::new(MAIN, 4),
            keep: None,
            renames: None,
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Merge Side / 0 into Main / 4 (keeps 4)");
    // RS-PROSE: both stations are parts, through a save and a reopen.
    assert_eq!(
        pills(&item),
        vec![
            ("station", "Side / 0".to_string()),
            ("station", "Main / 4".to_string())
        ]
    );
    assert_eq!(
        rows(&item),
        vec![
            r("Station A", "", "Side / 0"),
            r("Station B", "", "Main / 4")
        ]
    );
}

#[test]
fn unmerges_name_the_stations_they_detach() {
    let o = open();
    let cmd = Command::UnmergeStationPair {
        station_a: StationRef::new(MAIN, 2),
        station_b: StationRef::new(BRANCH, 0),
        renames: None,
        rest_keep: None,
    };
    record(&o.state, &cmd, |p| {
        p.unmerge_station_pair(
            StationRef::new(MAIN, 2),
            StationRef::new(BRANCH, 0),
            crate::models::survey::NameAnswers::RuleL,
            None,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    // The menu's words (Detach Line from Station): the pair form detaches
    // the second slot's line, naming the station on the first slot's line,
    // which stays on it (V-6).
    assert_eq!(item.description, "Detach line \"Branch\" from Main / 2");
    assert_eq!(
        rows(&item),
        vec![
            r("Station A", "", "Main / 2"),
            r("Station B", "", "Branch / 2")
        ]
    );

    assert!(o.state.undo().success);
    let cmd = Command::UnmergeStations {
        station_ref: StationRef::new(BRANCH, 0),
        renames: None,
        rest_keep: None,
    };
    record(&o.state, &cmd, |p| {
        p.unmerge_station(
            StationRef::new(BRANCH, 0),
            crate::models::survey::NameAnswers::RuleL,
            None,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Detach line \"Branch\" from Main / 2");
    assert_eq!(rows(&item), vec![r("Station", "", "Branch / 2")]);
}

/// Demo finding N-2: when the addressed line keeps the station (it shows it
/// by its own name and the other line was merged there), the line that comes
/// off is the other one, and the history names it, never the line that keeps
/// the station ("Detach line "Main Line"" read as if Main Line had left).
/// Both forms: the station form on the keeper's slot, and the pair form whose
/// partner is the keeper. The station reads `Main / 4`, on the line that
/// keeps it (demo finding V-6): `station 0` was Side's name for it, and Main
/// has its own station 0. Injection: naming the addressed slot's line again
/// (the old `detach`) gives `Detach line "Main" ...`; naming the station by
/// the detached line's name gives `... from station 0`.
#[test]
fn a_detach_that_keeps_the_station_names_the_line_that_comes_off() {
    let o = open();
    exec(
        &o.state,
        Command::MergeStations {
            station_a: StationRef::new(SIDE, 0),
            station_b: StationRef::new(MAIN, 4),
            keep: None,
            renames: None,
        },
    );
    let p = survey_of(&o.state);
    let junction = p.topology().station_at(StationRef::new(MAIN, 4)).unwrap();
    assert!(p.detach_keeps(junction, MAIN), "Main keeps the station");
    assert!(!p.detach_keeps(junction, SIDE));
    // Every line shows the kept station (Main's 4, RS 4.1); Side's own 0 is
    // archived in it.
    assert_eq!(p.shown_name(junction), Some("4"));
    assert_eq!(
        p.record_naming(junction, SIDE).map(|r| r.name.as_str()),
        Some("0")
    );

    exec(
        &o.state,
        Command::UnmergeStations {
            station_ref: StationRef::new(MAIN, 4),
            renames: None,
            rest_keep: None,
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Detach line \"Side\" from Main / 4");
    // The station form on the line that comes off reads the same.
    assert!(o.state.undo().success);
    exec(
        &o.state,
        Command::UnmergeStations {
            station_ref: StationRef::new(SIDE, 0),
            renames: None,
            rest_keep: None,
        },
    );
    assert_eq!(
        top(&o.state).description,
        "Detach line \"Side\" from Main / 4"
    );
    // The pair form whose partner (the line it detaches) keeps the station.
    assert!(o.state.undo().success);
    exec(
        &o.state,
        Command::UnmergeStationPair {
            station_a: StationRef::new(SIDE, 0),
            station_b: StationRef::new(MAIN, 4),
            renames: None,
            rest_keep: None,
        },
    );
    assert_eq!(
        top(&o.state).description,
        "Detach line \"Side\" from Main / 4"
    );
}

/// N-2 on a hub: when the addressed line keeps the station, every other line
/// comes off with the identity the first of them had, and the label lists
/// them by name, the station on the line that keeps it (V-6).
#[test]
fn a_detach_that_keeps_a_hub_names_every_line_that_comes_off() {
    let mut p = survey();
    let far = p
        .add_line(named("Far"), StartAt::New, vec![shot(3.0, 270.0)])
        .unwrap();
    let far_start = p.line(far).unwrap().start;
    p.set_placement(far_start, -100.0, 0.0).unwrap();
    let o = open_on(&p);
    for line in [SIDE, far] {
        exec(
            &o.state,
            Command::MergeStations {
                station_a: StationRef::new(line, 0),
                station_b: StationRef::new(MAIN, 4),
                keep: None,
                renames: None,
            },
        );
    }
    exec(
        &o.state,
        Command::UnmergeStations {
            station_ref: StationRef::new(MAIN, 4),
            renames: None,
            rest_keep: None,
        },
    );
    assert_eq!(
        top(&o.state).description,
        "Detach lines \"Side\" and \"Far\" from Main / 4"
    );
}

/// RS-FX4: Far merged into Side's start (Side kept), then that junction
/// into Main 4 (Main kept): Detach of Far gives back Side's station on Side
/// and Far together, and the label names both lines that come off.
#[test]
fn a_detach_of_a_junction_merged_away_names_every_line_that_comes_off() {
    let mut p = survey();
    let far = p
        .add_line(named("Far"), StartAt::New, vec![shot(3.0, 270.0)])
        .unwrap();
    let far_start = p.line(far).unwrap().start;
    p.set_placement(far_start, -100.0, 0.0).unwrap();
    let o = open_on(&p);
    for (a, b) in [
        (StationRef::new(far, 0), StationRef::new(SIDE, 0)),
        (StationRef::new(SIDE, 0), StationRef::new(MAIN, 4)),
    ] {
        exec(
            &o.state,
            Command::MergeStations {
                station_a: a,
                station_b: b,
                keep: Some(survey_of(&o.state).topology().station_at(b).unwrap()),
                renames: None,
            },
        );
    }
    exec(
        &o.state,
        Command::UnmergeStations {
            station_ref: StationRef::new(far, 0),
            renames: None,
            rest_keep: None,
        },
    );
    assert_eq!(
        top(&o.state).description,
        "Detach lines \"Side\" and \"Far\" from Main / 4"
    );
    let p = survey_of(&o.state);
    let side0 = p.topology().station_at(StationRef::new(SIDE, 0)).unwrap();
    assert_eq!(
        p.topology().station_at(StationRef::new(far, 0)),
        Some(side0)
    );
    assert_ne!(
        p.topology().station_at(StationRef::new(MAIN, 4)),
        Some(side0)
    );
}

#[test]
fn closures_name_their_ends_when_created_and_removed() {
    let o = open();
    let cmd = Command::CreateLoopClosure {
        station_a: StationRef::new(BRANCH, 2),
        station_b: StationRef::new(MAIN, 4),
        closure_strategy: ClosureStrategy::Batch,
        keep: None,
        renames: None,
    };
    let branch_end = labels::NameBook::of(&survey_of(&o.state))
        .station(StationRef::new(BRANCH, 2))
        .unwrap();
    record(&o.state, &cmd, |p| {
        p.create_closure_at(
            StationRef::new(BRANCH, 2),
            StationRef::new(MAIN, 4),
            crate::models::survey::ClosureStrategy::Batch,
            None,
            &[],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(
        item.description,
        format!("Create loop closure between {branch_end} and Main / 4")
    );
    assert_eq!(
        rows(&item),
        vec![
            r("Stations", "", &format!("{branch_end} \u{2194} Main / 4")),
            r("Strategy", "", "Batch"),
        ]
    );

    let closure_id = survey_of(&o.state).closures[0].id;
    let cmd = Command::RemoveLoopClosure {
        closure_id,
        renames: None,
    };
    record(&o.state, &cmd, |p| {
        p.remove_closure(closure_id, &[])
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Remove loop closure");
    assert_eq!(
        rows(&item),
        vec![
            r("Closure ID", "", &closure_id.to_string()),
            r("Stations", "", &format!("{branch_end} \u{2194} Main / 4")),
        ]
    );
}

#[test]
fn datums_show_the_gps_they_set_and_the_gps_they_remove() {
    let o = open();
    let gps = GpsCoordinates::new(20.5, -87.25, 0.0).unwrap();
    let cmd = Command::SetZeroDatum {
        station_ref: StationRef::new(MAIN, 0),
        coordinates: gps,
        accuracy_m: None,
        entrance: None,
    };
    record(&o.state, &cmd, |p| {
        p.set_datum_at(StationRef::new(MAIN, 0), gps, None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Set zero datum at Main / 0");
    assert_eq!(
        rows(&item),
        vec![
            r("Latitude", "", "20.500000"),
            r("Longitude", "", "-87.250000")
        ]
    );

    let cmd = Command::RemoveZeroDatum {
        station_ref: StationRef::new(MAIN, 0),
    };
    record(&o.state, &cmd, |p| {
        p.remove_datum_at(StationRef::new(MAIN, 0))
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Remove zero datum from Main / 0");
    assert_eq!(
        rows(&item),
        vec![
            r("Station", "", "Main / 0"),
            r("Latitude", "20.500000", ""),
            r("Longitude", "-87.250000", ""),
        ]
    );
}

fn set_comment(p: &mut Project, slot: StationRef, comment: Option<String>) -> Result<(), String> {
    let id = p.topology().station_at(slot).ok_or("no station")?;
    p.stations.get_mut(&id).ok_or("no station")?.comment = comment;
    Ok(())
}

#[test]
fn a_station_comment_shows_the_comment_before_and_after() {
    let o = open();
    let slot = StationRef::new(MAIN, 2);
    let hub = || Some("Hub".to_string());
    let cmd = Command::SetStationComment {
        station_ref: slot,
        comment: hub(),
    };
    record(&o.state, &cmd, |p| set_comment(p, slot, hub()));
    let item = assert_survives(&o);
    assert_eq!(item.description, "Set comment on station Main / 2");
    assert_eq!(rows(&item), vec![r("Comment", "", "Hub")]);

    let long = "x".repeat(81);
    let cut = format!("{}\u{2026}", "x".repeat(80));
    let cmd = Command::SetStationComment {
        station_ref: slot,
        comment: Some(long.clone()),
    };
    record(&o.state, &cmd, |p| set_comment(p, slot, Some(long.clone())));
    assert_eq!(rows(&top(&o.state)), vec![r("Comment", "Hub", &cut)]);

    let cmd = Command::SetStationComment {
        station_ref: slot,
        comment: None,
    };
    record(&o.state, &cmd, |p| set_comment(p, slot, None));
    let item = assert_survives(&o);
    assert_eq!(item.description, "Remove comment from station Main / 2");
    assert_eq!(rows(&item), vec![r("Comment", &cut, "(removed)")]);
}

#[test]
fn a_rename_names_the_station_by_the_name_it_had() {
    let o = open();
    let slot = StationRef::new(MAIN, 3);
    let cmd = Command::SetStationName {
        station_ref: slot,
        name: " Pillar ".to_string(),
    };
    record(&o.state, &cmd, |p| {
        let id = p.topology().station_at(slot).ok_or("no station")?;
        p.stations.get_mut(&id).ok_or("no station")?.name = "Pillar".to_string();
        Ok(())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Rename station Main / 3 to \"Pillar\"");
    assert_eq!(
        rows(&item),
        vec![r("Action", "", "Rename station Main / 3 to \"Pillar\"")]
    );
}

#[test]
fn a_comment_move_names_both_holders() {
    let o = open();
    let slot = StationRef::new(MAIN, 2);
    let hub = || Some("Hub".to_string());
    record(
        &o.state,
        &Command::SetStationComment {
            station_ref: slot,
            comment: hub(),
        },
        |p| set_comment(p, slot, hub()),
    );
    let shot_id = shot_id(&o.state, MAIN, 0);
    let cmd = Command::MoveComment {
        source: CommentHolder::Station { station_ref: slot },
        target: CommentHolder::Shot {
            line_id: MAIN,
            shot_id,
        },
        swap: false,
    };
    record(&o.state, &cmd, |p| {
        set_comment(p, slot, None)?;
        p.line_mut(MAIN).ok_or("no line")?.shots[0].comment = hub();
        Ok(())
    });
    let item = assert_survives(&o);
    let label = "Move comment from station Main / 2 to a shot in line \"Main\"";
    assert_eq!(item.description, label);
    assert_eq!(rows(&item), vec![r("Action", "", label)]);
}

// ---- Moves ----------------------------------------------------------------

#[test]
fn moves_show_their_offsets() {
    let o = open();
    let north = Coordinates::new(0.0, 5.0, 0.0);
    let cmd = Command::MoveLine {
        line_id: SIDE,
        offset: north,
    };
    record(&o.state, &cmd, |p| {
        p.move_line(SIDE, [0.0, 5.0, 0.0])
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Move line \"Side\"");
    assert_eq!(
        rows(&item),
        vec![r("Line", "", "Side"), r("Offset", "", "5.0m N")]
    );

    let cmd = Command::MoveSegment {
        line_id: SIDE,
        offset: north,
    };
    record(&o.state, &cmd, |p| {
        p.move_segment(SIDE, [0.0, 5.0, 0.0])
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Move segment (line \"Side\")");
    assert_eq!(
        rows(&item),
        vec![r("Offset", "", "5.0m N"), r("Affected lines", "", "1")]
    );

    let slot = StationRef::new(MAIN, 3);
    let cmd = Command::MoveStation {
        station_ref: slot,
        offset: Coordinates::new(5.0, 0.0, 2.0),
    };
    record(&o.state, &cmd, |p| {
        let id = p.topology().station_at(slot).ok_or("no station")?;
        p.move_station(id, [5.0, 0.0, 2.0])
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Move station Main / 3");
    assert_eq!(
        rows(&item),
        vec![r(
            "Offset",
            "",
            &Coordinates::new(5.0, 0.0, 2.0).display_offset()
        )]
    );
}

#[test]
fn a_segment_move_counts_every_line_of_its_piece() {
    let mut p = survey();
    let side_end = p.line(SIDE).unwrap().stations().nth(3).unwrap();
    p.add_line(
        named("Side Spur"),
        StartAt::Station(side_end),
        vec![shot(2.0, 0.0)],
    )
    .unwrap();
    let o = open_on(&p);
    let cmd = Command::MoveSegment {
        line_id: SIDE,
        offset: Coordinates::new(3.0, 0.0, 0.0),
    };
    record(&o.state, &cmd, |p| {
        p.move_segment(SIDE, [3.0, 0.0, 0.0])
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert_eq!(
        rows(&top(&o.state)),
        vec![r("Offset", "", "3.0m E"), r("Affected lines", "", "2")]
    );
}

// ---- Batches --------------------------------------------------------------

fn update_length(state: &AppState, line_id: LineId, index: usize, length: f64) -> Command {
    Command::UpdateShot {
        line_id,
        shot_id: shot_id(state, line_id, index),
        updates: ShotUpdates {
            length: Some(length),
            ..ShotUpdates::default()
        },
    }
}

#[test]
fn a_batch_lists_its_first_eight_commands_and_counts_the_rest() {
    let o = open();
    let commands: Vec<Command> = (0..4)
        .flat_map(|i| {
            [
                update_length(&o.state, MAIN, i, 20.0 + i as f64),
                Command::UpdateShot {
                    line_id: MAIN,
                    shot_id: shot_id(&o.state, MAIN, i),
                    updates: ShotUpdates {
                        up: Some(Some(1.0)),
                        ..ShotUpdates::default()
                    },
                },
            ]
        })
        .chain((0..3).map(|i| update_length(&o.state, SIDE, i, 9.0)))
        .collect();
    exec(
        &o.state,
        Command::Batch {
            commands,
            label: None,
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Update 7 shots in 2 lines");
    assert_eq!(item.command_type, "batch");
    let listed = rows(&item);
    assert_eq!(listed.len(), labels::BATCH_PREVIEW_ROWS);
    assert_eq!(more(&item), 11 - labels::BATCH_PREVIEW_ROWS);
    assert_eq!(
        listed[0],
        r("Update shot in line \"Main\"", "", "Length: 20.00")
    );
    assert_eq!(listed[1], r("Update shot in line \"Main\"", "", ""));
}

#[test]
fn a_batch_with_its_own_label_keeps_it() {
    let o = open();
    exec(
        &o.state,
        Command::Batch {
            commands: vec![
                update_length(&o.state, MAIN, 0, 11.0),
                Command::ReverseLine {
                    line_id: SIDE,
                    swap_left_right: false,
                },
            ],
            label: Some("Tidy up".to_string()),
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Tidy up");
    assert_eq!(item.description_parts, None, "its own words, read plain");
    assert_eq!(
        rows(&item),
        vec![
            r("Update shot in line \"Main\"", "", "Length: 11.00"),
            r("Reverse line \"Side\"", "", ""),
        ]
    );
}

#[test]
fn a_batch_names_a_line_it_deletes_by_the_name_it_had() {
    let o = open();
    exec(
        &o.state,
        Command::Batch {
            commands: vec![
                Command::MergeStations {
                    station_a: StationRef::new(SIDE, 0),
                    station_b: StationRef::new(MAIN, 4),
                    keep: None,
                    renames: None,
                },
                Command::DeleteLine { line_id: BRANCH },
            ],
            label: None,
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Merge Side / 0 into Main / 4 (keeps 4)");
    assert_eq!(
        rows(&item),
        vec![
            r(
                "Merge Side / 0 into Main / 4 (keeps 4)",
                "",
                "Side / 0 \u{2194} Main / 4"
            ),
            r("Delete line \"Branch\"", "", ""),
        ]
    );
}

// ---- Sections, imports, Add .stalx ------------------------------------------

#[test]
fn section_commands_name_their_section() {
    let mut p = survey();
    p.sections.push(Section {
        id: 1,
        name: "Upper level".to_string(),
        color: "#4ecdc4".to_string(),
    });
    p.counters.next_section_id = 2;
    let o = open_on(&p);
    let check = |cmd: Command, op: &dyn Fn(&mut Project), label: &str| {
        record(&o.state, &cmd, |p| {
            op(p);
            Ok(())
        });
        let item = assert_survives(&o);
        assert_eq!(item.description, label);
        assert_eq!(rows(&item), vec![r("Action", "", label)]);
    };
    check(
        Command::CreateCaveSection {
            name: " Lower level ".to_string(),
            color: "#ff0000".to_string(),
            line_ids: vec![MAIN, BRANCH, MAIN],
        },
        &|p| {
            p.sections.push(Section {
                id: 2,
                name: "Lower level".to_string(),
                color: "#ff0000".to_string(),
            });
            p.counters.next_section_id = 3;
            for id in [MAIN, BRANCH] {
                p.line_mut(id).unwrap().section_id = Some(2);
            }
        },
        "Create section \"Lower level\" with 2 lines",
    );
    check(
        Command::UpdateCaveSection {
            section_id: 1,
            name: Some("Upper".to_string()),
            color: None,
        },
        &|p| p.sections[0].name = "Upper".to_string(),
        "Rename section \"Upper level\" to \"Upper\"",
    );
    check(
        Command::UpdateCaveSection {
            section_id: 1,
            name: None,
            color: Some("#00ff00".to_string()),
        },
        &|p| p.sections[0].color = "#00ff00".to_string(),
        "Change colour of section \"Upper\"",
    );
    check(
        Command::MoveLinesToCaveSection {
            line_ids: vec![SIDE],
            section_id: Some(2),
        },
        &|p| p.line_mut(SIDE).unwrap().section_id = Some(2),
        "Move 1 line to section \"Lower level\"",
    );
    check(
        Command::ApplyCaveSectionColor { section_id: 2 },
        &|p| {
            for id in [MAIN, BRANCH, SIDE] {
                p.line_mut(id).unwrap().color = "#ff0000".to_string();
            }
        },
        "Apply colour of section \"Lower level\" to its lines",
    );
    check(
        Command::MoveLinesToCaveSection {
            line_ids: vec![MAIN, SIDE],
            section_id: None,
        },
        &|p| {
            for id in [MAIN, SIDE] {
                p.line_mut(id).unwrap().section_id = None;
            }
        },
        "Move 2 lines out of their section",
    );
    check(
        Command::DeleteCaveSection { section_id: 1 },
        &|p| {
            p.sections.remove(0);
        },
        "Delete section \"Upper\"",
    );
}

#[test]
fn forgetting_an_import_says_how_many_lines() {
    let mut p = survey();
    p.imported_line_hashes = vec!["a".to_string(), "b".to_string()];
    let o = open_on(&p);
    let cmd = Command::UntrackImport {
        file_hashes: vec![],
        line_hashes: vec!["a".to_string(), "b".to_string()],
    };
    record(&o.state, &cmd, |p| {
        p.imported_line_hashes.clear();
        Ok(())
    });
    let item = assert_survives(&o);
    assert_eq!(item.description, "Forget 2 imported lines");
    assert_eq!(item.command_type, "untrack_import");
}

#[test]
fn an_added_project_shows_the_file_and_what_it_brought() {
    let o = open();
    let added = |name: &str, shots: usize, at: [f64; 2]| {
        let name = name.to_string();
        move |p: &mut Project, _: &mut Record| {
            p.create_line(
                named(&name),
                StartAt::New,
                (0..shots).map(|i| shot(2.0, 5.0 * i as f64)).collect(),
                Some(at),
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        }
    };
    let recorded = |_: &Project| Record {
        kind: "add_project_file".to_string(),
        label: "Add 'second.stalx' to project".to_string(),
        preview: labels::add_project_file_preview("second.stalx", 1, 2),
        ..Record::default()
    };
    let done = o
        .state
        .run_recorded(recorded, added("Added", 2, [50.0, 50.0]));
    assert!(done.is_ok());
    let item = assert_survives(&o);
    assert_eq!(item.description, "Add 'second.stalx' to project");
    assert_eq!(
        rows(&item),
        vec![
            r("File", "", "second.stalx"),
            r("Lines added", "", "1"),
            r("Shots added", "", "2"),
        ]
    );

    // Without recorded rows the counts are read off the changeset.
    let bare = |_: &Project| Record {
        kind: "add_project_file".to_string(),
        label: "Add 'third.stalx' to project".to_string(),
        ..Record::default()
    };
    let done = o.state.run_recorded(bare, added("Third", 1, [80.0, 50.0]));
    assert!(done.is_ok());
    assert_eq!(
        rows(&top(&o.state)),
        vec![r("Lines added", "", "1"), r("Shots added", "", "1")]
    );
}

// ---- Stability, limits ----------------------------------------------------

#[test]
fn labels_never_change_after_a_reverse_an_insert_or_a_rename() {
    let o = open();
    let shot_id = shot_id(&o.state, MAIN, 1);
    exec(
        &o.state,
        Command::UpdateShot {
            line_id: MAIN,
            shot_id,
            updates: ShotUpdates {
                length: Some(12.5),
                ..ShotUpdates::default()
            },
        },
    );
    exec(
        &o.state,
        Command::MergeStations {
            station_a: StationRef::new(SIDE, 0),
            station_b: StationRef::new(MAIN, 4),
            keep: None,
            renames: None,
        },
    );
    let before: Vec<serde_json::Value> = o.state.get_undo_stack().iter().map(shown).collect();
    exec(
        &o.state,
        Command::ReverseLine {
            line_id: MAIN,
            swap_left_right: false,
        },
    );
    // An Insert Station on Main shifts every station index after it.
    let head = survey_of(&o.state).line(MAIN).unwrap().shots[0].id;
    let done = o.state.run_recorded(
        |p| Record {
            kind: "batch".to_string(),
            label: format!("Insert station in line \"{}\"", p.line_label_of(MAIN)),
            ..Record::default()
        },
        |p, _| {
            p.split_shot_at_distance(MAIN, head, 1.0, None)
                .map(|_| ())
                .map_err(|e| e.to_string())
        },
    );
    assert!(done.is_ok(), "{:?}", done.err().and_then(|f| f.error));
    // And a rename of the line the older entries name.
    let rename = LineUpdates {
        name: Some(Some("Trunk".to_string())),
        ..LineUpdates::default()
    };
    record(
        &o.state,
        &Command::UpdateLineMetadata {
            line_id: MAIN,
            updates: rename.clone(),
        },
        |p| {
            p.update_line_meta(MAIN, &rename)
                .map(|_| ())
                .map_err(|e| e.to_string())
        },
    );
    let now: Vec<serde_json::Value> = o.state.get_undo_stack().iter().map(shown).collect();
    assert_eq!(now.len(), 5);
    assert_eq!(now[3..], before[..], "older entries keep their words");
    // `undo_to(i)` steps back through the entry at index `i`.
    assert!(o.state.undo_to(4).success);
    // The redo stack lists the next redo (the oldest entry) first.
    let redo: Vec<serde_json::Value> = o.state.get_redo_stack().iter().rev().map(shown).collect();
    assert_eq!(redo, now);
    assert!(o.state.redo_to(4).success);
    assert_eq!(
        o.state
            .get_undo_stack()
            .iter()
            .map(shown)
            .collect::<Vec<_>>(),
        now
    );
}

#[test]
fn a_long_name_never_makes_an_entry_the_next_open_refuses() {
    let mut p = survey();
    p.line_mut(SIDE).unwrap().name = Some("N".repeat(5_000));
    let o = open_on(&p);
    exec(&o.state, Command::DeleteLine { line_id: SIDE });
    let item = assert_survives(&o);
    assert!(item.description.chars().count() <= EntryLimits::default().max_text_chars);
    assert!(item.description.starts_with("Delete line \"NNN"));
    // Cut to fit, it has no parts: they would say other words.
    assert!(item.description.ends_with('\u{2026}'));
    assert_eq!(item.description_parts, None);
}

#[test]
fn a_huge_batch_records_refs_the_next_open_reads() {
    let mut p = survey();
    let max = EntryLimits::default().max_refs;
    // More lines than an entry lists (shot-less spurs: cheap to build), and
    // one line with more stations than it lists.
    let side_end = p.line(SIDE).unwrap().stations().nth(3).unwrap();
    let spurs: Vec<LineId> = (0..max + 10)
        .map(|i| {
            p.add_line(
                named(&format!("Spur {i}")),
                StartAt::Station(side_end),
                Vec::new(),
            )
            .unwrap()
        })
        .collect();
    let long = p
        .add_line(
            named("Long"),
            StartAt::Station(side_end),
            (0..max + 10).map(|i| shot(1.0, (i % 360) as f64)).collect(),
        )
        .unwrap();
    let cmd = Command::Batch {
        commands: spurs
            .iter()
            .map(|&line_id| Command::ReverseLine {
                line_id,
                swap_left_right: false,
            })
            .chain((1..=max + 10).map(|i| Command::SetStationName {
                station_ref: StationRef::new(long, i as u32),
                name: format!("T{i}"),
            }))
            .collect(),
        label: None,
    };
    let record = Record::of(&p, &cmd);
    assert_eq!(record.refs.lines.len(), max);
    assert_eq!(record.refs.stations.len(), max);
    assert!(
        record
            .label
            .starts_with("Reverse lines \"Spur 0\", \"Spur 1\", \"Spur 2\" and "),
        "{}",
        record.label
    );
    assert_eq!(record.preview.rows.len(), labels::BATCH_PREVIEW_ROWS);
    assert_eq!(
        record.preview.more,
        (2 * (max + 10) - labels::BATCH_PREVIEW_ROWS) as u64
    );
    // The entry it makes is one the next open reads.
    let entry = crate::models::survey::HistoryEntry {
        timestamp: chrono::Utc::now(),
        kind: record.kind,
        label: record.label,
        label_parts: record.label_parts,
        refs: record.refs,
        preview: record.preview,
        changes: crate::models::survey::changeset::capture(&p, &p),
    };
    let text = encode_entry(&entry).unwrap();
    assert!(decode_entry(&text, &EntryLimits::default()).is_ok());
}

#[test]
fn an_entry_reads_the_same_from_its_persisted_text() {
    let o = open();
    exec(
        &o.state,
        Command::MergeStations {
            station_a: StationRef::new(SIDE, 0),
            station_b: StationRef::new(MAIN, 4),
            keep: None,
            renames: None,
        },
    );
    let stack = o.state.undo_stack.lock().unwrap();
    let held = stack.undo_entries().next().unwrap();
    let text = encode_entry(held.entry()).unwrap();
    let back = decode_entry(&text, &EntryLimits::default()).unwrap();
    let item = |e| crate::core::undo::history_item::history_item(0, e);
    assert_eq!(shown(&item(held.entry())), shown(&item(&back)));
    assert_eq!(back.label, "Merge Side / 0 into Main / 4 (keeps 4)");
    assert!(back.refs.lines.contains(&SIDE) && back.refs.lines.contains(&MAIN));
    assert_eq!(back.refs.stations.len(), 2);
}

// ---- Recipe labels (the helpers the recipe packages call) --------------------

#[test]
fn recipe_labels_read_as_0_6_0_worded_them() {
    let p = survey();
    assert_eq!(
        words(labels::combine_lines_label(&p, MAIN, SIDE, "Main")),
        "Combine lines \"Main\" and \"Side\""
    );
    assert_eq!(
        words(labels::combine_lines_label(&p, MAIN, SIDE, " Big Loop ")),
        "Combine lines \"Main\" and \"Side\" into \"Big Loop\""
    );
    assert_eq!(
        words(labels::create_line_at_label(
            &p,
            SIDE,
            StationRef::new(MAIN, 3)
        )),
        "Create line \"Side\" at Main / 3"
    );
    assert_eq!(
        words(labels::split_line_label(Some(MAIN), "Main", "2")),
        "Split line \"Main\" at 2"
    );
    assert_eq!(
        words(labels::delete_shots_label(
            &p,
            MAIN,
            2,
            2,
            ShotsDeleted::Split
        )),
        "Delete shot 2 \u{2192} 3 and split line \"Main\""
    );
    assert_eq!(
        words(labels::delete_shots_label(
            &p,
            MAIN,
            1,
            3,
            ShotsDeleted::Split
        )),
        "Delete 3 shots (1 \u{2192} 4) and split line \"Main\""
    );
    assert_eq!(
        words(labels::delete_shots_label(
            &p,
            MAIN,
            0,
            1,
            ShotsDeleted::CloseUp
        )),
        "Delete 2 shots (0 \u{2192} 2) from line \"Main\""
    );
    assert_eq!(
        words(labels::delete_shots_label(
            &p,
            SIDE,
            0,
            0,
            ShotsDeleted::LastOfLine
        )),
        "Delete line \"Side\" with its last shot"
    );
    assert_eq!(
        words(labels::import_lines_label(&p, &[MAIN])),
        "Import line \"Main\""
    );
    assert_eq!(
        words(labels::import_lines_label(&p, &[MAIN, SIDE])),
        "Import lines \"Main\" and \"Side\""
    );
    assert_eq!(
        words(labels::import_lines_label(
            &p,
            &[MAIN, BRANCH, SIDE, MAIN, SIDE]
        )),
        "Import 5 lines (\"Main\", \"Branch\", \"Side\" and 2 more)"
    );
    assert_eq!(words(labels::import_lines_label(&p, &[])), "Import");
    let station = p.topology().station_at(StationRef::new(MAIN, 3)).unwrap();
    assert_eq!(
        words(labels::insert_station_label(&p, MAIN, station)),
        "Insert station Main / 3"
    );
    assert_eq!(
        words(labels::insert_station_label(&p, 99, station)),
        "Insert station in line a line"
    );
    assert_eq!(
        labels::NameBook::of(&p).station(StationRef::new(MAIN, 9)),
        None
    );
}

#[test]
fn an_insert_station_is_named_after_the_station_the_split_made() {
    let mut p = survey();
    let shot_id = p.line(MAIN).unwrap().shots[1].id;
    let made = p
        .split_shot_at_distance(MAIN, shot_id, 2.0, None)
        .unwrap()
        .value;
    let name = p.stations[&made.station].name.clone();
    assert_eq!(
        words(labels::insert_station_label(&p, MAIN, made.station)),
        format!("Insert station Main / {name}")
    );
}

/// A comment is cut by character, never inside a multi-byte codepoint, and
/// marked cut only when characters were dropped (0.6.0's regressions).
#[test]
fn comments_are_cut_by_character() {
    let p = survey();
    let detail = |comment: String| {
        labels::batch_preview(
            &p,
            &[Command::SetStationComment {
                station_ref: StationRef::new(MAIN, 1),
                comment: Some(comment),
            }],
        )
        .rows[0]
            .new
            .clone()
    };
    // 46 + 5 = 51 characters (56 bytes), over the 50-character detail cap.
    let cut = detail(format!("{}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", "a".repeat(46)));
    assert_eq!(
        cut,
        format!(
            "Comment: {}\u{e9}\u{e9}\u{e9}\u{e9}\u{2026}",
            "a".repeat(46)
        )
    );
    // 13 characters, 52 bytes: nothing dropped, no marker.
    let emoji = "\u{1f300}".repeat(13);
    assert_eq!(detail(emoji.clone()), format!("Comment: {emoji}"));
    let removed = labels::batch_preview(
        &p,
        &[Command::SetStationComment {
            station_ref: StationRef::new(MAIN, 1),
            comment: None,
        }],
    );
    assert_eq!(
        removed.rows[0].field,
        "Remove comment from station Main / 1"
    );
    assert_eq!(removed.rows[0].new, "(removed)");
    assert_eq!(
        crate::core::undo::history_item::truncate_with_ellipsis("\u{e9}\u{e9}\u{e9}", 2),
        "\u{e9}\u{e9}\u{2026}"
    );
}

/// A batch's sub-command details, as 0.6.0's batch preview listed them.
#[test]
fn batch_rows_carry_each_sub_commands_detail() {
    let p = survey();
    let shot0 = p.line(MAIN).unwrap().shots[0].id;
    let preview = labels::batch_preview(
        &p,
        &[
            Command::MoveLine {
                line_id: SIDE,
                offset: Coordinates::new(0.0, 5.0, 0.0),
            },
            Command::CreateLoopClosure {
                station_a: StationRef::new(BRANCH, 2),
                station_b: StationRef::new(MAIN, 9),
                closure_strategy: ClosureStrategy::Move,
                keep: None,
                renames: None,
            },
            Command::SetShotKind {
                line_id: MAIN,
                shot_id: shot0,
                kind: ShotKind::Reference,
            },
            Command::CreateShot {
                line_id: MAIN,
                shot: shot(4.0, 30.0),
                after_shot_id: None,
                before_shot_id: None,
            },
            Command::SetZeroDatum {
                station_ref: StationRef::new(MAIN, 0),
                coordinates: GpsCoordinates::new(20.5, -87.25, 0.0).unwrap(),
                accuracy_m: None,
                entrance: None,
            },
            Command::DeleteShot {
                line_id: MAIN,
                shot_id: shot0,
            },
            Command::CloseProject,
        ],
    );
    let rows: Vec<(String, String)> = preview
        .rows
        .into_iter()
        .map(|row| (row.field, row.new))
        .collect();
    let branch_end = labels::NameBook::of(&p)
        .station(StationRef::new(BRANCH, 2))
        .unwrap();
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        rows,
        vec![
            pair("Move line \"Side\"", "Side, 5.0m N"),
            // An out-of-range slot is never shown by its handle.
            pair(
                "Create loop closure between",
                &format!("{branch_end} \u{2194} -, Move")
            ),
            pair("Mark shot in line \"Main\" as reference", "Kind: Reference"),
            pair(
                "Create shot in line \"Main\"",
                "Length: 4.00, Azimuth: 30.0"
            ),
            pair(
                "Set zero datum at Main / 0",
                "Lat: 20.500000, Lon: -87.250000"
            ),
            pair("Delete shot in line \"Main\"", ""),
            pair("Close project", ""),
        ]
    );
    assert_eq!(preview.more, 0);
}

// ---- Label parts (RS-PROSE) ---------------------------------------------------

#[test]
fn entry_labels_name_shots_stations_and_lines_as_parts() {
    let o = open();
    let id = shot_id(&o.state, MAIN, 1);
    exec(
        &o.state,
        Command::SetShotKind {
            line_id: MAIN,
            shot_id: id,
            kind: ShotKind::Jump,
        },
    );
    let item = top(&o.state);
    assert_eq!(
        pills(&item),
        vec![("shot", "Main: 1 \u{2192} 2".to_string())]
    );
    exec(
        &o.state,
        Command::ReverseLine {
            line_id: SIDE,
            swap_left_right: false,
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Reverse line \"Side\"");
    assert_eq!(pills(&item), vec![("line", "\"Side\"".to_string())]);
    assert!(
        item.description_parts.iter().flatten().any(|p| matches!(
            p,
            crate::models::ProsePart::Line(l) if l.line_id == Some(SIDE) && l.name == "Side"
        )),
        "{item:?}"
    );
    // A summary names its lines.
    exec(
        &o.state,
        Command::Batch {
            commands: vec![
                Command::ReverseLine {
                    line_id: MAIN,
                    swap_left_right: false,
                },
                Command::ReverseLine {
                    line_id: BRANCH,
                    swap_left_right: false,
                },
            ],
            label: None,
        },
    );
    let item = assert_survives(&o);
    assert_eq!(item.description, "Reverse lines \"Main\" and \"Branch\"");
    assert_eq!(
        pills(&item),
        vec![
            ("line", "\"Main\"".to_string()),
            ("line", "\"Branch\"".to_string())
        ]
    );
}

#[test]
fn a_label_that_names_nothing_has_no_parts() {
    let o = open();
    exec(
        &o.state,
        Command::CreateLine {
            name: Some("Annex".to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: None,
            shots: vec![shot(4.0, 10.0)],
            cave_section_id: None,
        },
    );
    let item = top(&o.state);
    assert_eq!(item.description_parts, None, "{}", item.description);
}

#[test]
fn recipe_labels_name_their_lines_and_stations_as_parts() {
    let p = survey();
    let kinds = |label: &labels::Label| -> Vec<(String, Option<LineId>)> {
        label
            .parts
            .iter()
            .filter_map(|part| match part {
                crate::models::ProsePart::Line(l) => Some((l.text.clone(), l.line_id)),
                crate::models::ProsePart::Station(s) => Some((s.text.clone(), s.line_id)),
                _ => None,
            })
            .collect()
    };
    let combine = labels::combine_lines_label(&p, MAIN, SIDE, " Big Loop ");
    assert_eq!(
        kinds(&combine),
        vec![
            ("\"Main\"".to_string(), Some(MAIN)),
            ("\"Side\"".to_string(), Some(SIDE))
        ],
        "the new name is no line yet: words"
    );
    let at = labels::create_line_at_label(&p, SIDE, StationRef::new(MAIN, 3));
    assert_eq!(
        kinds(&at),
        vec![
            ("\"Side\"".to_string(), Some(SIDE)),
            ("Main / 3".to_string(), Some(MAIN))
        ]
    );
    let split = labels::split_line_label(Some(MAIN), "Main", "2");
    assert_eq!(kinds(&split), vec![("\"Main\"".to_string(), Some(MAIN))]);
    let deleted = labels::delete_shots_label(&p, MAIN, 1, 3, ShotsDeleted::Split);
    assert_eq!(kinds(&deleted), vec![("\"Main\"".to_string(), Some(MAIN))]);
    let imported = labels::import_lines_label(&p, &[MAIN, BRANCH, SIDE, MAIN, SIDE]);
    assert_eq!(kinds(&imported).len(), 3, "the lines it lists");
    let station = p.topology().station_at(StationRef::new(MAIN, 3)).unwrap();
    let inserted = labels::insert_station_label(&p, MAIN, station);
    assert_eq!(kinds(&inserted), vec![("Main / 3".to_string(), Some(MAIN))]);
    // Every one says its words, without slots.
    for label in [combine, at, split, deleted, imported, inserted] {
        assert_eq!(
            crate::models::prose::plain(&label.parts),
            words(label.clone())
        );
    }
    // A line the project does not hold is words.
    assert!(labels::insert_station_label(&p, 99, station)
        .parts
        .is_empty());
}
