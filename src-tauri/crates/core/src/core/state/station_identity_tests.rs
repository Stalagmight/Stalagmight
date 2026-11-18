//! Station identity at the state layer: every command leaves every station
//! named and numbered, undo and redo give the exact ids and names back, the
//! user edits are validated, Add .stalx offsets the incoming ids, the file
//! keeps them, and the history speaks in names.

use std::collections::BTreeMap;

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{
    CombineJoinEnds, CombineLinesOptions, CombinePick, LineData, LineEnd, LineId, LineUpdates,
    Project, ShotData, SplitAt, SplitLineOptions, SplitSide, StationId, StationRef,
};
use crate::persistence::Database;

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn create_line(state: &AppState, name: &str, shots: usize) -> LineId {
    let result = state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: (0..shots).map(|_| shot(90.0, 10.0)).collect(),
        cave_section_id: None,
    });
    assert!(result.success, "{:?}", result.error);
    result.line_id.unwrap()
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Identity".to_string(),
                location: None,
            })
            .success
    );
    state
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// Every station of every line: `(line, index) -> (id, name)`.
fn identities(project: &Project) -> BTreeMap<(LineId, u32), (Option<StationId>, Option<String>)> {
    let mut out = BTreeMap::new();
    for line in &project.lines {
        for index in 0..=line.shots.len() as u32 {
            let station = StationRef::new(line.id, index);
            out.insert(
                (line.id, index),
                (
                    project.station_id_at(station),
                    project.station_name_at(station).map(str::to_string),
                ),
            );
        }
    }
    out
}

fn names(project: &Project, line: LineId) -> Vec<String> {
    project
        .line_station_names(line)
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect()
}

fn shot_id(project: &Project, line: LineId, index: usize) -> crate::models::ShotId {
    project.get_line(line).unwrap().shots[index].id
}

/// Undo, check `before`; redo, check `after`.
fn assert_undo_redo_exact(state: &AppState, before: &str, after: &str, what: &str) {
    let snapshot = |state: &AppState| -> String {
        let p = project(state);
        format!("{:?} next={}", identities(&p), p.next_station_id)
    };
    assert_eq!(snapshot(state), after, "{what}: forward");
    assert!(state.undo().success, "{what}: undo");
    let undone = project(state);
    assert_eq!(
        format!("{:?}", identities(&undone)),
        before.split(" next=").next().unwrap(),
        "{what}: undo gives every station its id and name back"
    );
    assert!(state.redo().success, "{what}: redo");
    let redone = project(state);
    assert_eq!(
        format!("{:?}", identities(&redone)),
        after.split(" next=").next().unwrap(),
        "{what}: redo gives the created stations the same ids and names"
    );
}

fn identity_snapshot(state: &AppState) -> String {
    let p = project(state);
    format!("{:?} next={}", identities(&p), p.next_station_id)
}

#[test]
fn every_command_leaves_every_station_named_and_numbered() {
    let state = new_state();
    let main = create_line(&state, "Main", 3);
    let p = project(&state);
    assert_eq!(names(&p, main), ["0", "1", "2", "3"]);
    let ids: Vec<_> = (0..=3)
        .map(|i| p.station_id_at(StationRef::new(main, i)).unwrap())
        .collect();
    assert_eq!(ids, [0, 1, 2, 3]);

    // Append at the end: the next number, the next id.
    assert!(
        state
            .execute(Command::CreateShot {
                line_id: main,
                shot: shot(0.0, 5.0),
                after_shot_id: None,
                before_shot_id: None,
            })
            .success
    );
    let p = project(&state);
    assert_eq!(names(&p, main), ["0", "1", "2", "3", "4"]);
    assert_eq!(p.station_id_at(StationRef::new(main, 4)), Some(4));
}

#[test]
fn undo_redo_of_an_insert_gives_the_station_back_exactly() {
    let state = new_state();
    let main = create_line(&state, "Main", 3);
    let before = identity_snapshot(&state);
    let after_shot = shot_id(&project(&state), main, 1);
    assert!(
        state
            .execute(Command::CreateShot {
                line_id: main,
                shot: shot(0.0, 5.0),
                after_shot_id: Some(after_shot),
                before_shot_id: None,
            })
            .success
    );
    let p = project(&state);
    assert_eq!(names(&p, main), ["0", "1", "2", "2a", "3"]);
    let after = identity_snapshot(&state);
    assert_undo_redo_exact(&state, &before, &after, "insert after");

    // A prepend names the new start after the old start's family.
    let before = identity_snapshot(&state);
    let first = shot_id(&project(&state), main, 0);
    assert!(
        state
            .execute(Command::CreateShot {
                line_id: main,
                shot: shot(0.0, 5.0),
                after_shot_id: None,
                before_shot_id: Some(first),
            })
            .success
    );
    assert_eq!(
        names(&project(&state), main),
        ["4", "0", "1", "2", "2a", "3"]
    );
    let after = identity_snapshot(&state);
    assert_undo_redo_exact(&state, &before, &after, "prepend");
}

#[test]
fn undo_of_a_delete_gives_the_station_back_exactly() {
    let state = new_state();
    let main = create_line(&state, "Main", 4);
    let before = identity_snapshot(&state);
    let middle = shot_id(&project(&state), main, 1);
    assert!(
        state
            .execute(Command::DeleteShot {
                line_id: main,
                shot_id: middle,
            })
            .success
    );
    let after = identity_snapshot(&state);
    assert_undo_redo_exact(&state, &before, &after, "delete");
}

#[test]
fn undo_redo_of_insert_station_split_and_combine_are_exact() {
    let state = new_state();
    let main = create_line(&state, "Main", 4);

    // Insert Station on shot 1 -> 2.
    let before = identity_snapshot(&state);
    let target = shot_id(&project(&state), main, 1);
    assert!(
        state
            .execute_split_shot_at_distance(main, target, 4.0)
            .success
    );
    assert_eq!(
        names(&project(&state), main),
        ["0", "1", "1a", "2", "3", "4"]
    );
    let after = identity_snapshot(&state);
    assert_undo_redo_exact(&state, &before, &after, "insert station");

    // Split Line Here at station 3: the copy gets a new id, the same name.
    let before = identity_snapshot(&state);
    let options = SplitLineOptions {
        name_a: "Main".to_string(),
        name_b: "Main B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    };
    let result = state.execute_split_line(main, SplitAt::Station { station_index: 3 }, options);
    assert!(result.success, "{:?}", result.error);
    let second = result.line_id.unwrap();
    let p = project(&state);
    assert_eq!(names(&p, second)[0], names(&p, main)[3]);
    assert_ne!(
        p.station_id_at(StationRef::new(second, 0)),
        p.station_id_at(StationRef::new(main, 3))
    );
    let after = identity_snapshot(&state);
    assert_undo_redo_exact(&state, &before, &after, "split");

    // Combine them back: the names collide (the copy "2"), so both letter.
    let before = identity_snapshot(&state);
    let options = CombineLinesOptions {
        name: "Main".to_string(),
        color: "#111111".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: Some(CombineJoinEnds {
            kept: LineEnd::End,
            other: LineEnd::Start,
        }),
    };
    let result = state.execute_combine_lines(main, second, options);
    assert!(result.success, "{:?}", result.error);
    let after = identity_snapshot(&state);
    assert_undo_redo_exact(&state, &before, &after, "combine");
}

#[test]
fn undo_redo_of_create_line_keeps_its_ids() {
    let state = new_state();
    create_line(&state, "Main", 2);
    let before = identity_snapshot(&state);
    create_line(&state, "Side", 2);
    let after = identity_snapshot(&state);
    assert_undo_redo_exact(&state, &before, &after, "create line");
}

#[test]
fn a_station_name_is_validated_and_undoable() {
    let state = new_state();
    let main = create_line(&state, "Main Line", 3);
    let station = StationRef::new(main, 2);

    let taken = state.execute(Command::SetStationName {
        station_ref: station,
        name: "1".to_string(),
    });
    assert_eq!(
        taken.error.as_deref(),
        Some("Line \"Main Line\" already has a station named \"1\"")
    );
    let empty = state.execute(Command::SetStationName {
        station_ref: station,
        name: "  ".to_string(),
    });
    assert_eq!(
        empty.error.as_deref(),
        Some("A station name cannot be empty")
    );

    let ok = state.execute(Command::SetStationName {
        station_ref: station,
        name: " sump ".to_string(),
    });
    assert!(ok.success, "{:?}", ok.error);
    assert_eq!(project(&state).station_name_at(station), Some("sump"));
    let history = state.get_undo_stack();
    assert_eq!(
        history[0].description,
        "Rename station Main Line / 2 to \"sump\""
    );

    assert!(state.undo().success);
    assert_eq!(project(&state).station_name_at(station), Some("2"));
    assert!(state.redo().success);
    assert_eq!(project(&state).station_name_at(station), Some("sump"));
}

#[test]
fn a_user_line_rename_must_be_unique_and_non_empty() {
    let state = new_state();
    let main = create_line(&state, "Main", 1);
    create_line(&state, "Side", 1);

    let rename = |name: Option<&str>| {
        state.execute(Command::UpdateLineMetadata {
            line_id: main,
            updates: LineUpdates {
                name: Some(name.map(str::to_string)),
                ..Default::default()
            },
        })
    };
    assert_eq!(
        rename(Some(" Side ")).error.as_deref(),
        Some("A line named \"Side\" already exists")
    );
    assert_eq!(
        rename(None).error.as_deref(),
        Some("A line name cannot be empty")
    );
    assert_eq!(
        rename(Some("")).error.as_deref(),
        Some("A line name cannot be empty")
    );
    assert!(rename(Some("Upstream")).success);
    assert!(state.undo().success);
    assert_eq!(
        project(&state).get_line(main).unwrap().name.as_deref(),
        Some("Main")
    );

    // The same rename inside a batch (Edit Line's Save) is checked too.
    let batch = state.execute(Command::Batch {
        commands: vec![Command::UpdateLineMetadata {
            line_id: main,
            updates: LineUpdates {
                name: Some(Some("Side".to_string())),
                ..Default::default()
            },
        }],
    });
    assert_eq!(
        batch.error.as_deref(),
        Some("A line named \"Side\" already exists")
    );

    // The IPC create path refuses a taken or missing name; the automatic
    // paths de-duplicate instead.
    assert!(state.refuse_new_line_name(Some("Main")).is_some());
    assert!(state.refuse_new_line_name(None).is_some());
    assert!(state.refuse_new_line_name(Some("New")).is_none());
    create_line(&state, "Main", 1);
    let p = project(&state);
    assert!(p
        .lines
        .iter()
        .any(|l| l.name.as_deref() == Some("Main (2)")));
}

#[test]
fn the_history_names_lines_stations_and_shots() {
    let state = new_state();
    let main = create_line(&state, "Main Line", 6);
    let side = create_line(&state, "Side", 2);
    let p = project(&state);
    let fifth = shot_id(&p, main, 4);
    assert!(
        state
            .execute(Command::UpdateShot {
                line_id: main,
                shot_id: fifth,
                updates: crate::models::ShotUpdates {
                    comment: Some(Some("tight".to_string())),
                    ..Default::default()
                },
            })
            .success
    );
    let side_end = StationRef::new(side, 1);
    // Bring Side's station 1 onto Main's station 0, then merge them.
    let p = project(&state);
    let a = p
        .compute_station_raw_coords(StationRef::new(main, 0))
        .unwrap();
    let b = p.compute_station_raw_coords(side_end).unwrap();
    assert!(
        state
            .execute(Command::MoveLine {
                line_id: side,
                offset: crate::models::Coordinates::new(a.x - b.x, a.y - b.y, 0.0),
            })
            .success
    );
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(main, 0),
        station_b: side_end,
    });
    assert!(merged.success, "{:?}", merged.error);
    assert!(state.execute(Command::DeleteLine { line_id: side }).success);

    let labels: Vec<String> = state
        .get_undo_stack()
        .into_iter()
        .map(|item| item.description)
        .collect();
    assert_eq!(labels[0], "Delete line \"Side\"", "{labels:?}");
    // Side is gone, so the merge falls back to its wording without handles.
    assert_eq!(labels[1], "Merge stations", "{labels:?}");
    assert_eq!(
        labels[3], "Update shot Main Line: 4 \u{2192} 5",
        "{labels:?}"
    );
    assert!(labels
        .iter()
        .all(|l| !l.contains(':') || l.contains("Main Line:")));

    // Undo the delete: Side is back, and the merge reads by name.
    assert!(state.undo().success);
    let labels: Vec<String> = state
        .get_undo_stack()
        .into_iter()
        .map(|item| item.description)
        .collect();
    assert_eq!(
        labels[0], "Merge stations Main Line / 0 and Side / 1",
        "{labels:?}"
    );
}

#[test]
fn add_stalx_offsets_the_incoming_ids_and_dedupes_line_names() {
    let dir = TempDir::new().unwrap();
    // The source: one line named like the target's, ids 0..=2.
    let mut source = Project::new("Source".to_string(), None);
    source.add_line(LineData {
        name: Some("Main".to_string()),
        shots: vec![shot(0.0, 5.0), shot(0.0, 5.0)],
        ..Default::default()
    });
    source.ensure_station_identities();
    let path = dir.path().join("source.stalx");
    Database::open(&path)
        .unwrap()
        .save_project(&source)
        .unwrap();

    let state = new_state();
    let main = create_line(&state, "Main", 3);
    let base = project(&state).next_station_id;
    assert_eq!(base, 4);
    let result = state.execute(Command::add_project_file(path));
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    let added = p.lines.iter().find(|l| l.id != main).unwrap();
    assert_eq!(added.name.as_deref(), Some("Main (2)"));
    let ids: Vec<_> = (0..=2)
        .map(|i| p.station_id_at(StationRef::new(added.id, i)).unwrap())
        .collect();
    assert_eq!(ids, [base, base + 1, base + 2]);
    assert_eq!(p.next_station_id, base + 3);
    assert_eq!(names(&p, added.id), ["0", "1", "2"]);

    // Undo takes them away; redo puts back the same ids.
    let after = identity_snapshot(&state);
    assert!(state.undo().success);
    assert!(state.redo().success);
    assert_eq!(identity_snapshot(&state), after);
}

#[test]
fn ids_and_names_survive_a_save_and_a_file_without_them_re_mints_the_same_ids() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ids.stalx");
    let state = new_state();
    let main = create_line(&state, "Main", 3);
    let second = shot_id(&project(&state), main, 1);
    // Delete a shot so the ids are not simply the positions any more.
    assert!(
        state
            .execute(Command::DeleteShot {
                line_id: main,
                shot_id: second,
            })
            .success
    );
    assert!(
        state
            .execute(Command::SetStationName {
                station_ref: StationRef::new(main, 1),
                name: "sump".to_string(),
            })
            .success
    );
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    let before = project(&state);

    let reopened = AppState::new().unwrap();
    assert!(
        reopened
            .execute(Command::OpenProject { path: path.clone() })
            .success
    );
    let after = project(&reopened);
    assert_eq!(identities(&after), identities(&before));
    assert_eq!(after.next_station_id, before.next_station_id);

    // An older build rewrites the file without the table and the key: every
    // open re-mints the same ids, in line order.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "DROP TABLE station_ids; DELETE FROM project_settings WHERE key = 'next_station_id';",
        )
        .unwrap();
    let mut minted = Vec::new();
    for _ in 0..2 {
        let state = AppState::new().unwrap();
        assert!(
            state
                .execute(Command::OpenProject { path: path.clone() })
                .success
        );
        minted.push(identities(&project(&state)));
    }
    assert_eq!(minted[0], minted[1]);
    let names_kept: Vec<_> = minted[0].values().map(|(_, n)| n.clone()).collect();
    assert_eq!(
        names_kept,
        [Some("0".into()), Some("sump".into()), Some("3".into())]
    );
}

/// The epilogue's fast path: a command that cannot change station identity
/// (`Command::can_change_station_identity`) skips the whole-project pass, a
/// structural one runs it. Observed through a station whose name was taken
/// away behind the command path's back: only the structural command seeds
/// it again.
#[test]
fn metadata_commands_skip_the_identity_pass_and_structural_ones_run_it() {
    let state = new_state();
    let main = create_line(&state, "Main", 2);
    let unname = |state: &AppState| {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        let key = project.key_for_ref(StationRef::new(main, 1)).unwrap();
        project.station_names.remove(&key);
    };
    let name_of_1 = |state: &AppState| {
        project(state)
            .station_name_at(StationRef::new(main, 1))
            .map(str::to_string)
    };
    unname(&state);
    let metadata = [
        Command::UpdateLineMetadata {
            line_id: main,
            updates: LineUpdates {
                color: Some("#123456".to_string()),
                ..Default::default()
            },
        },
        Command::CreateCaveSection {
            name: "Upper".to_string(),
            color: "#abcdef".to_string(),
            line_ids: vec![main],
        },
        Command::SetStationName {
            station_ref: StationRef::new(main, 2),
            name: "end".to_string(),
        },
        Command::Batch {
            commands: vec![Command::UpdateLineMetadata {
                line_id: main,
                updates: LineUpdates {
                    color: Some("#654321".to_string()),
                    ..Default::default()
                },
            }],
        },
    ];
    for cmd in metadata {
        assert!(!cmd.can_change_station_identity(), "{}", cmd.description());
        let result = state.execute(cmd);
        assert!(result.success, "{:?}", result.error);
        assert_eq!(name_of_1(&state), None, "the pass was skipped");
    }
    let append = Command::CreateShot {
        line_id: main,
        shot: shot(0.0, 5.0),
        after_shot_id: None,
        before_shot_id: None,
    };
    assert!(append.can_change_station_identity());
    assert!(state.execute(append).success);
    assert!(name_of_1(&state).is_some(), "the pass ran");
}

/// Turn the snapshot pair on top of the undo (or redo) stack into what a
/// build that predates Station IDs recorded: no ids, no counter, no names.
fn make_snapshot_pair_legacy(state: &AppState, redo: bool) {
    let mut stack = state.undo_stack.lock().unwrap();
    let mut entry = if redo {
        stack.pop_redo().unwrap()
    } else {
        stack.pop_undo().unwrap()
    };
    let crate::core::UndoData::ProjectSnapshotPair { before, after } = &mut entry.undo_data else {
        panic!("expected a snapshot pair, got {:?}", entry.undo_data);
    };
    for snapshot in [before, after] {
        let mut json: serde_json::Value = serde_json::from_str(snapshot).unwrap();
        let object = json.as_object_mut().unwrap();
        for key in ["station_ids", "next_station_id", "station_names"] {
            object.remove(key);
        }
        *snapshot = serde_json::to_string(&json).unwrap();
    }
    if redo {
        stack.push_redo(entry);
    } else {
        stack.push_undo(entry);
    }
}

/// Every station of `line` by position: `(id, name)`.
fn line_identities(state: &AppState, line: LineId) -> Vec<(Option<StationId>, Option<String>)> {
    let p = project(state);
    let count = p.get_line(line).unwrap().shots.len() as u32;
    (0..=count)
        .map(|i| {
            let at = StationRef::new(line, i);
            (
                p.station_id_at(at),
                p.station_name_at(at).map(str::to_string),
            )
        })
        .collect()
}

fn name_stations(state: &AppState, line: LineId, names: &[&str]) {
    for (i, name) in names.iter().enumerate() {
        assert!(
            state
                .execute(Command::SetStationName {
                    station_ref: StationRef::new(line, i as u32),
                    name: (*name).to_string(),
                })
                .success
        );
    }
}

/// A legacy snapshot pair around a Reverse: the restored state's stations
/// take the live ids and names of the SAME physical stations (matched by the
/// shots on either side), not of whatever station now holds the same key -
/// `LineStart` is the other end after a reverse.
#[test]
fn a_legacy_reverse_undo_adopts_ids_by_physical_station() {
    let state = new_state();
    let main = create_line(&state, "Main", 3);
    name_stations(&state, main, &["s0", "s1", "s2", "s3"]);
    let original = line_identities(&state, main);
    assert!(
        state
            .execute(Command::ReverseLine {
                line_id: main,
                swap_left_right: false,
            })
            .success
    );
    let reversed = line_identities(&state, main);
    let mut expected = original.clone();
    expected.reverse();
    assert_eq!(
        reversed, expected,
        "a reverse keeps every station's identity"
    );

    make_snapshot_pair_legacy(&state, false);
    assert!(state.undo().success);
    assert_eq!(
        line_identities(&state, main),
        original,
        "undo puts each id and name back on its own station"
    );
    make_snapshot_pair_legacy(&state, true);
    assert!(state.redo().success);
    assert_eq!(line_identities(&state, main), reversed);
}

/// A legacy snapshot pair around a first-shot delete: `LineStart` names the
/// old station 1 live but station 0 in the restored state. Stations whose
/// sides match keep their ids and names; the two whose sides changed are
/// minted and seeded fresh, never given another station's.
#[test]
fn a_legacy_first_shot_delete_undo_never_moves_an_id_to_another_station() {
    let state = new_state();
    let main = create_line(&state, "Main", 4);
    name_stations(&state, main, &["s0", "s1", "s2", "s3", "s4"]);
    let original = line_identities(&state, main);
    let first = shot_id(&project(&state), main, 0);
    assert!(
        state
            .execute(Command::DeleteShot {
                line_id: main,
                shot_id: first,
            })
            .success
    );
    let deleted = line_identities(&state, main);
    assert_eq!(deleted, original[1..].to_vec());

    make_snapshot_pair_legacy(&state, false);
    assert!(state.undo().success);
    let undone = line_identities(&state, main);
    assert_eq!(undone[2..], original[2..], "unchanged sides keep their ids");
    for (i, (id, name)) in undone[..2].iter().enumerate() {
        assert!(id.is_some() && name.is_some(), "station {i}: {undone:?}");
        assert!(
            !original[1..].iter().any(|(o, _)| o == id),
            "station {i} took another station's id: {undone:?}"
        );
        assert_ne!(name.as_deref(), Some("s1"), "station {i}: {undone:?}");
    }
    let ids: std::collections::HashSet<_> = undone.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids.len(), undone.len(), "ids stay unique: {undone:?}");

    make_snapshot_pair_legacy(&state, true);
    assert!(state.redo().success);
    let redone = line_identities(&state, main);
    assert_eq!(redone[1..], deleted[1..], "s2..s4 keep their ids and names");
    assert!(
        !original.iter().any(|(o, _)| *o == redone[0].0),
        "the new first station gets a fresh id: {redone:?}"
    );
}
