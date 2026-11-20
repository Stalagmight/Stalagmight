//! Station identity at the state layer, on the station-graph model: every
//! station has an id and a name by construction (invariants 1 and 8), a
//! junction is ONE station whose id, name and comment every slot shows
//! (plan 1.2, W identity), a merge keeps the retired station as an alias, a
//! detach mints a new station named as the junction, undo and redo give the
//! exact ids and names back, the file keeps them, and the history speaks in
//! names worded when the edit was made.
//!
//! Fates of the legacy tests:
//! - `the_history_names_lines_stations_and_shots`: labels are worded at
//!   record time (plan 8.2), so the merge keeps naming the line the delete
//!   later removed; the Move Line that brought the two stations together
//!   is gone (the merge moves the piece itself);
//! - `ids_and_names_survive_a_save_and_a_file_without_them_re_mints_the_same_ids`:
//!   the save half is kept; a v2 file cannot lack ids (the decoder refuses
//!   it), and the legacy re-mint is the migration's (`persistence::migrate`
//!   tests), so the second half is retired;
//! - `metadata_commands_skip_the_identity_pass_and_structural_ones_run_it`:
//!   there is no identity pass (ids are structural); retired;
//! - `a_legacy_reverse_undo_adopts_ids_by_physical_station` and
//!   `a_legacy_first_shot_delete_undo_never_moves_an_id_to_another_station`
//!   drove the legacy snapshot-pair restore; histories older than the
//!   station-graph format are dropped on open (plan 7.2.8); retired;
//! - the tests of Create Shot, Delete Shot, Insert Station, Split Line Here,
//!   Combine Lines, Create Line, line renames, station renames and Add .stalx
//!   are ported onto the station graph's expectations (the halves of a split
//!   share their station) and ignored until the packages that port those
//!   commands land (C2, C3, C4, C6, C8).

use std::collections::BTreeMap;

use super::topology_kit::{
    assert_round_trip, empty, history, line_at, line_from, merge, open, project, run, shot, slot,
    snapshot, st,
};
use super::AppState;
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{
    CombineJoinEnds, CombineLinesOptions, CombinePick, LineEnd, LineId, LineUpdates, SplitAt,
    SplitLineOptions, SplitSide, StationId,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

fn shots(count: usize) -> Vec<crate::models::ShotData> {
    (0..count).map(|_| shot(90.0, 10.0)).collect()
}

/// Every slot of every line: `(line, index) -> (id, name)`.
fn identities(p: &Project) -> BTreeMap<(LineId, u32), (StationId, String)> {
    let mut out = BTreeMap::new();
    for line in p.lines() {
        for (index, station) in line.stations().enumerate() {
            let index = u32::try_from(index).unwrap();
            out.insert(
                (line.id, index),
                (station, p.stations[&station].name.clone()),
            );
        }
    }
    out
}

fn names(p: &Project, line: LineId) -> Vec<String> {
    p.line(line)
        .unwrap()
        .stations()
        .map(|s| p.stations[&s].name.clone())
        .collect()
}

fn shot_id(p: &Project, line: LineId, index: usize) -> crate::models::ShotId {
    p.line(line).unwrap().shots[index].id
}

fn identity_snapshot(state: &AppState) -> String {
    let p = project(state);
    format!("{:?} next={}", identities(&p), p.counters.next_station_id)
}

/// Undo, check `before`; redo, check `after`. The counter may stand higher
/// after the undo (plan 8.1: an undo never hands an id out twice).
fn assert_undo_redo_exact(state: &AppState, before: &str, after: &str, what: &str) {
    let ids = |s: &str| s.split(" next=").next().unwrap().to_string();
    assert_eq!(identity_snapshot(state), after, "{what}: forward");
    assert!(state.undo().success, "{what}: undo");
    assert_eq!(
        ids(&identity_snapshot(state)),
        ids(before),
        "{what}: undo gives every station its id and name back"
    );
    assert!(state.redo().success, "{what}: redo");
    assert_eq!(
        identity_snapshot(state),
        after,
        "{what}: redo gives the created stations the same ids and names"
    );
}

/// One line `Main` of `count` shots, opened.
fn main_line(count: usize) -> (super::topology_kit::Opened, LineId) {
    let mut p = empty("Identity");
    let main = line_at(&mut p, "Main", [0.0, 0.0], shots(count));
    (open(&p), main)
}

#[test]
fn a_new_line_numbers_and_names_its_stations() {
    let (opened, main) = main_line(3);
    let p = project(&opened.state);
    assert_eq!(names(&p, main), ["0", "1", "2", "3"]);
    let ids: Vec<_> = (0..=3).map(|i| st(&p, main, i)).collect();
    assert_eq!(ids, [0, 1, 2, 3]);
    assert_eq!(p.counters.next_station_id, 4);
}

#[test]
fn an_append_takes_the_next_number_and_id() {
    let (opened, main) = main_line(3);
    let state = &opened.state;
    run(
        state,
        Command::CreateShot {
            line_id: main,
            shot: shot(0.0, 5.0),
            after_shot_id: None,
            before_shot_id: None,
        },
    );
    let p = project(state);
    assert_eq!(names(&p, main), ["0", "1", "2", "3", "4"]);
    assert_eq!(st(&p, main, 4), 4);
}

#[test]
fn undo_redo_of_an_insert_gives_the_station_back_exactly() {
    let (opened, main) = main_line(3);
    let state = &opened.state;
    let before = identity_snapshot(state);
    let after_shot = shot_id(&project(state), main, 1);
    run(
        state,
        Command::CreateShot {
            line_id: main,
            shot: shot(0.0, 5.0),
            after_shot_id: Some(after_shot),
            before_shot_id: None,
        },
    );
    assert_eq!(names(&project(state), main), ["0", "1", "2", "2a", "3"]);
    let after = identity_snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "insert after");

    // A prepend names the new start after the old start's family.
    let before = identity_snapshot(state);
    let first = shot_id(&project(state), main, 0);
    run(
        state,
        Command::CreateShot {
            line_id: main,
            shot: shot(0.0, 5.0),
            after_shot_id: None,
            before_shot_id: Some(first),
        },
    );
    assert_eq!(
        names(&project(state), main),
        ["4", "0", "1", "2", "2a", "3"]
    );
    let after = identity_snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "prepend");
}

#[test]
fn undo_of_a_delete_gives_the_station_back_exactly() {
    let (opened, main) = main_line(4);
    let state = &opened.state;
    let before = identity_snapshot(state);
    let middle = shot_id(&project(state), main, 1);
    run(
        state,
        Command::DeleteShot {
            line_id: main,
            shot_id: middle,
        },
    );
    let after = identity_snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "delete");
}

#[test]
fn undo_redo_of_insert_station_split_and_combine_are_exact() {
    let (opened, main) = main_line(4);
    let state = &opened.state;

    // Insert Station on shot 1 -> 2.
    let before = identity_snapshot(state);
    let target = shot_id(&project(state), main, 1);
    assert!(
        state
            .execute_split_shot_at_distance(main, target, 4.0)
            .success
    );
    assert_eq!(
        names(&project(state), main),
        ["0", "1", "1a", "2", "3", "4"]
    );
    let after = identity_snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "insert station");

    // Split Line Here at station 3: the halves share that station (plan 5.4).
    let before = identity_snapshot(state);
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
    let p = project(state);
    assert_eq!(st(&p, second, 0), st(&p, main, 3), "one shared station");
    assert_eq!(names(&p, second)[0], names(&p, main)[3]);
    let after = identity_snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "split");

    // Combine them back at their shared station.
    let before = identity_snapshot(state);
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
    let after = identity_snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "combine");
}

#[test]
fn undo_redo_of_create_line_keeps_its_ids() {
    let (opened, _main) = main_line(2);
    let state = &opened.state;
    let before = identity_snapshot(state);
    run(
        state,
        Command::CreateLine {
            name: Some("Side".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: shots(2),
            cave_section_id: None,
        },
    );
    let after = identity_snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "create line");
}

#[test]
fn a_station_name_is_validated_and_undoable() {
    let mut p = empty("Identity");
    let main = line_at(&mut p, "Main Line", [0.0, 0.0], shots(3));
    let opened = open(&p);
    let state = &opened.state;
    let station = slot(main, 2);
    let name_at = |state: &AppState| {
        let p = project(state);
        p.stations[&st(&p, main, 2)].name.clone()
    };

    let taken = state.execute(Command::SetStationName {
        station_ref: station,
        name: "1".to_string(),
    });
    assert_eq!(
        taken.error.as_deref(),
        Some("Line \"Main Line\" already has a station named \"1\"")
    );
    let empty_name = state.execute(Command::SetStationName {
        station_ref: station,
        name: "  ".to_string(),
    });
    assert_eq!(
        empty_name.error.as_deref(),
        Some("A station name cannot be empty")
    );

    run(
        state,
        Command::SetStationName {
            station_ref: station,
            name: " sump ".to_string(),
        },
    );
    assert_eq!(name_at(state), "sump");
    assert_eq!(
        history(state)[0].1,
        "Rename station Main Line / 2 to \"sump\""
    );
    assert!(state.undo().success);
    assert_eq!(name_at(state), "2");
    assert!(state.redo().success);
    assert_eq!(name_at(state), "sump");
}

#[test]
fn a_user_line_rename_must_be_unique_and_non_empty() {
    let mut p = empty("Identity");
    let main = line_at(&mut p, "Main", [0.0, 0.0], shots(1));
    line_at(&mut p, "Side", [0.0, 0.0], shots(1));
    let opened = open(&p);
    let state = &opened.state;
    let rename = |name: Option<&str>| {
        state.execute(Command::UpdateLineMetadata {
            line_id: main,
            updates: LineUpdates {
                name: Some(name.map(str::to_string)),
                ..LineUpdates::default()
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
        project(state).line(main).unwrap().name.as_deref(),
        Some("Main")
    );

    // The same rename inside a batch (Edit Line's Save) is checked too.
    let batch = state.execute(Command::Batch {
        commands: vec![Command::UpdateLineMetadata {
            line_id: main,
            updates: LineUpdates {
                name: Some(Some("Side".to_string())),
                ..LineUpdates::default()
            },
        }],
        label: None,
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
    run(
        state,
        Command::CreateLine {
            name: Some("Main".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: shots(1),
            cave_section_id: None,
        },
    );
    assert!(project(state)
        .lines()
        .iter()
        .any(|l| l.name.as_deref() == Some("Main (2)")));
}

#[test]
fn the_history_names_lines_and_stations_as_they_were_at_the_edit() {
    let mut p = empty("Identity");
    let main = line_at(&mut p, "Main Line", [0.0, 0.0], shots(6));
    let side = line_at(&mut p, "Side", [40.0, 40.0], shots(2));
    let opened = open(&p);
    let state = &opened.state;
    let fifth = shot_id(&project(state), main, 4);
    run(
        state,
        Command::UpdateShot {
            line_id: main,
            shot_id: fifth,
            updates: crate::models::ShotUpdates {
                comment: Some(Some("tight".to_string())),
                ..crate::models::ShotUpdates::default()
            },
        },
    );
    merge(state, slot(main, 0), slot(side, 1));
    run(state, Command::DeleteLine { line_id: side });

    let labels: Vec<String> = history(state).into_iter().map(|(_, d)| d).collect();
    assert_eq!(labels[0], "Delete line \"Side\"", "{labels:?}");
    // Worded when the merge was made (plan 8.2): Side is named although it
    // is gone now.
    assert_eq!(
        labels[1], "Merge stations Main Line / 0 and Side / 1",
        "{labels:?}"
    );
    assert!(labels[2].starts_with("Update shot"), "{labels:?}");

    // Undo the delete: Side is back, and the merge still reads by name.
    assert!(state.undo().success);
    let labels: Vec<String> = history(state).into_iter().map(|(_, d)| d).collect();
    assert_eq!(
        labels[0], "Merge stations Main Line / 0 and Side / 1",
        "{labels:?}"
    );
}

#[test]
fn add_stalx_offsets_the_incoming_ids_and_dedupes_line_names() {
    // The source: one line named like the target's, ids 0..=2.
    let mut source = empty("Source");
    line_at(
        &mut source,
        "Main",
        [0.0, 0.0],
        vec![shot(0.0, 5.0), shot(0.0, 5.0)],
    );
    let (opened, main) = main_line(3);
    let path = opened.dir.path().join("source.stalx");
    std::fs::write(
        &path,
        stalx_v2::encode(&source, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();

    let state = &opened.state;
    let base = project(state).counters.next_station_id;
    assert_eq!(base, 4);
    run(state, Command::add_project_file(path));
    let p = project(state);
    let added = p.lines().iter().find(|l| l.id != main).unwrap().id;
    assert_eq!(p.line(added).unwrap().name.as_deref(), Some("Main (2)"));
    let ids: Vec<_> = (0..=2).map(|i| st(&p, added, i)).collect();
    assert_eq!(ids, [base, base + 1, base + 2]);
    assert_eq!(p.counters.next_station_id, base + 3);
    assert_eq!(names(&p, added), ["0", "1", "2"]);

    // Undo takes them away; redo puts back the same ids.
    let after = identity_snapshot(state);
    assert!(state.undo().success);
    assert!(state.redo().success);
    assert_eq!(identity_snapshot(state), after);
}

#[test]
fn ids_and_names_survive_a_save_and_reopen() {
    // A deleted shot and a renamed station, so ids are not simply positions.
    let mut p = empty("Identity");
    let main = line_at(&mut p, "Main", [0.0, 0.0], shots(3));
    let side = line_at(&mut p, "Side", [40.0, 0.0], shots(1));
    let second = shot_id(&p, main, 1);
    p.delete_shot(main, second).unwrap();
    let renamed = st(&p, main, 1);
    p.station_mut(renamed).unwrap().name = "sump".to_string();
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(side, 0), slot(main, 2));
    let before = project(state);

    let path = opened.dir.path().join("ids.stalx");
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let reopened = AppState::new().unwrap();
    assert!(reopened.execute(Command::OpenProject { path }).success);
    let after = project(&reopened);
    assert_eq!(identities(&after), identities(&before));
    assert_eq!(after.counters, before.counters);
    assert_eq!(after.stations, before.stations, "aliases ride the file too");
    assert_eq!(names(&after, main), ["0", "sump", "3"]);
}

// === Junction identity (plan 1.2, W identity) ===

#[test]
fn a_merge_shares_one_id_and_name_and_keeps_the_retired_one_as_an_alias() {
    let mut p = empty("Junction");
    let a = line_at(&mut p, "A", [0.0, 0.0], shots(2));
    let b = line_at(&mut p, "B", [50.0, 0.0], shots(2));
    let (b0, a2) = (st(&p, b, 0), st(&p, a, 2));
    p.station_mut(b0).unwrap().comment = Some("sump".to_string());
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    // B's piece moves onto A:2: A:2 is the survivor.
    merge(state, slot(b, 0), slot(a, 2));
    let p = project(state);
    assert_eq!(st(&p, b, 0), a2, "one station, the survivor's id");
    assert!(!p.stations.contains_key(&b0), "the retired station is gone");
    let survivor = &p.stations[&a2];
    assert_eq!(survivor.name, "2", "the survivor keeps its name");
    assert_eq!(
        survivor.comment.as_deref(),
        Some("sump"),
        "and takes the comment"
    );
    assert_eq!(
        survivor
            .aliases
            .iter()
            .map(|alias| (alias.id, alias.name.clone()))
            .collect::<Vec<_>>(),
        [(b0, Some("0".to_string()))]
    );
    // Every slot of the junction shows the one id, name and comment.
    let stations = state.project_state_wire().unwrap().stations;
    let wire: Vec<_> = stations
        .iter()
        .filter(|s| s.station_ref == slot(a, 2) || s.station_ref == slot(b, 0))
        .map(|s| (s.id, s.name.clone(), s.comment.clone()))
        .collect();
    assert_eq!(
        wire,
        [
            (a2, "2".to_string(), Some("sump".to_string())),
            (a2, "2".to_string(), Some("sump".to_string()))
        ]
    );
    assert_round_trip(state, &before, "merge");
}

#[test]
fn a_detach_mints_a_station_named_as_the_junction_and_redo_gives_it_back() {
    let mut p = empty("Junction");
    let a = line_at(&mut p, "A", [0.0, 0.0], shots(2));
    let a1 = st(&p, a, 1);
    let b = line_from(&mut p, "B", a1, shots(2));
    let opened = open(&p);
    let state = &opened.state;
    let before = identity_snapshot(state);
    let next = project(state).counters.next_station_id;

    run(
        state,
        Command::UnmergeStations {
            station_ref: slot(b, 0),
        },
    );
    let p = project(state);
    assert_eq!(st(&p, a, 1), a1, "A keeps the junction's id");
    assert_eq!(st(&p, b, 0), next, "B's slot gets the next id");
    assert_eq!(p.stations[&next].name, p.stations[&a1].name);
    assert!(p.stations[&next].aliases.is_empty());
    assert!(p.stations[&next].comment.is_none());
    let after = identity_snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "detach");
}

#[test]
fn a_merge_re_letters_a_clashing_name_on_the_moving_line() {
    // B's own station 1 is named like A:1, the survivor B:0 becomes: the
    // junction's lines keep their names unique (plan 5.3, the letter-suffix
    // rule).
    let mut p = empty("Names");
    let a = line_at(&mut p, "A", [0.0, 0.0], shots(2));
    let b = line_at(&mut p, "B", [50.0, 0.0], shots(2));
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);
    merge(state, slot(b, 0), slot(a, 1));
    let p = project(state);
    assert_eq!(names(&p, b), ["1", "1a", "2"]);
    assert_eq!(
        names(&p, a),
        ["0", "1", "2"],
        "the stationary line is untouched"
    );
    assert_round_trip(state, &before, "merge with a clash");
}
