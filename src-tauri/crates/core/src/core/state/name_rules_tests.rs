//! The name rules at the command layer: a rename to the name a station
//! already has is a no-op, control and invisible formatting characters are
//! refused, names are stored and compared trimmed, Split and Combine refuse
//! a line name a rename would refuse (and never rename an unrelated line),
//! and the Combine candidate tells the dialog which stations it renames.

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    CombineJoinEnds, CombineLinesOptions, CombinePick, CombineRenameReason, LineEnd, LineId,
    LineUpdates, Project, ShotData, SplitAt, SplitLineOptions, SplitSide, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn ok(result: CommandResult) -> CommandResult {
    assert!(result.success, "{:?}", result.error);
    result
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    ok(state.execute(Command::CreateProject {
        name: "Names".to_string(),
        location: None,
    }));
    state
}

fn create_line(state: &AppState, name: &str, shots: usize) -> LineId {
    ok(state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: (0..shots).map(|_| shot(90.0, 10.0)).collect(),
        cave_section_id: None,
    }))
    .line_id
    .unwrap()
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn line_name(state: &AppState, line: LineId) -> Option<String> {
    project(state).get_line(line).and_then(|l| l.name.clone())
}

fn split_options(name_a: &str, name_b: &str) -> SplitLineOptions {
    SplitLineOptions {
        name_a: name_a.to_string(),
        name_b: name_b.to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    }
}

fn combine_options(name: &str) -> CombineLinesOptions {
    CombineLinesOptions {
        name: name.to_string(),
        color: "#111111".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: Some(CombineJoinEnds {
            kept: LineEnd::End,
            other: LineEnd::Start,
        }),
    }
}

/// Split `line` at station `at`: the two halves.
fn split(state: &AppState, line: LineId, at: u32, name_a: &str, name_b: &str) -> (LineId, LineId) {
    let result = ok(state.execute_split_line(
        line,
        SplitAt::Station { station_index: at },
        split_options(name_a, name_b),
    ));
    (line, result.line_id.unwrap())
}

#[test]
fn renaming_a_station_to_its_own_name_leaves_no_history() {
    let state = new_state();
    let main = create_line(&state, "Main", 2);
    let station = StationRef::new(main, 1);
    ok(state.execute(Command::SetStationName {
        station_ref: station,
        name: "sump".to_string(),
    }));
    assert!(state.undo().success);
    assert!(state.can_redo());
    let history = state.get_undo_stack().len();
    let modified_at = project(&state).modified_at;

    // " 1 " is "1" once trimmed: the station's name already.
    ok(state.execute(Command::SetStationName {
        station_ref: station,
        name: " 1 ".to_string(),
    }));
    assert_eq!(state.get_undo_stack().len(), history, "no undo entry");
    assert!(state.can_redo(), "the redo branch survives");
    assert_eq!(project(&state).modified_at, modified_at, "not touched");
    assert_eq!(project(&state).station_name_at(station), Some("1"));
}

#[test]
fn control_and_invisible_characters_are_refused_in_names() {
    let state = new_state();
    let main = create_line(&state, "Main", 2);
    let refused = |result: CommandResult| result.error.unwrap_or_default();

    let station = refused(state.execute(Command::SetStationName {
        station_ref: StationRef::new(main, 1),
        name: "A\u{202E}B".to_string(),
    }));
    assert_eq!(
        station,
        "A station name cannot contain control or invisible formatting characters (U+202E)"
    );
    let line = refused(state.execute(Command::UpdateLineMetadata {
        line_id: main,
        updates: LineUpdates {
            name: Some(Some("Main\tLine".to_string())),
            ..Default::default()
        },
    }));
    assert_eq!(
        line,
        "A line name cannot contain control or invisible formatting characters (U+0009)"
    );
    for hidden in ['\u{200B}', '\u{200F}', '\u{2066}', '\u{FEFF}', '\u{0}'] {
        let section = refused(state.execute(Command::CreateCaveSection {
            name: format!("Upper{hidden}level"),
            color: "#abcdef".to_string(),
            line_ids: vec![],
        }));
        assert!(
            section.starts_with("A section name cannot contain"),
            "{hidden:?}: {section}"
        );
    }
    assert!(state
        .refuse_new_line_name(Some("New\u{200D}Line"))
        .is_some());

    // Visible separators stay allowed.
    ok(state.execute(Command::SetStationName {
        station_ref: StationRef::new(main, 1),
        name: "1 / sump".to_string(),
    }));
    ok(state.execute(Command::UpdateLineMetadata {
        line_id: main,
        updates: LineUpdates {
            name: Some(Some("Main / upstream".to_string())),
            ..Default::default()
        },
    }));
}

#[test]
fn names_are_stored_and_compared_trimmed() {
    let state = new_state();
    let main = create_line(&state, " Main ", 1);
    assert_eq!(line_name(&state, main).as_deref(), Some("Main"));
    assert!(
        state.refuse_new_line_name(Some("Main ")).is_some(),
        "a padded copy of a taken name is the taken name"
    );
    ok(state.execute(Command::UpdateLineMetadata {
        line_id: main,
        updates: LineUpdates {
            name: Some(Some("  Upstream\u{20}".to_string())),
            ..Default::default()
        },
    }));
    assert_eq!(line_name(&state, main).as_deref(), Some("Upstream"));

    // A file (or an older build) can hold padded names: the normalization
    // trims them, and a repeat found on the trimmed key gets ` (2)`.
    let mut p = Project::new("Padded".to_string(), None);
    for name in ["Main", "Main ", " Side"] {
        p.add_line(crate::models::LineData {
            name: Some(name.to_string()),
            color: "#123456".to_string(),
            quality: 1.0,
            shots: vec![shot(0.0, 1.0)],
            ..Default::default()
        });
    }
    // `add_line` trims; put the padding back as a file would hold it.
    p.lines[1].name = Some("Main ".to_string());
    p.lines[2].name = Some(" Side".to_string());
    let (first, second) = (p.lines[0].id, p.lines[1].id);
    let key = |p: &Project, line: LineId, i: u32| p.key_for_ref(StationRef::new(line, i)).unwrap();
    let (k0, k1) = (key(&p, first, 0), key(&p, first, 1));
    p.station_names.insert(k0, "5 ".to_string());
    p.station_names.insert(k1, "5".to_string());
    p.ensure_station_identities();
    let names: Vec<_> = p.lines.iter().map(|l| l.name.clone().unwrap()).collect();
    assert_eq!(names, ["Main", "Main (2)", "Side"]);
    assert_eq!(
        p.line_station_names(first),
        [Some("5".to_string()), Some("5a".to_string())]
    );
    assert!(p.get_line(second).is_some());
    // Settled: a second pass changes nothing.
    assert!(p.ensure_station_identities().is_empty());
}

#[test]
fn split_refuses_a_taken_or_empty_half_name_like_a_rename() {
    let state = new_state();
    let main = create_line(&state, "Main", 4);
    create_line(&state, "Side", 1);
    let refusal = |name_a: &str, name_b: &str| {
        state
            .execute_split_line(
                main,
                SplitAt::Station { station_index: 2 },
                split_options(name_a, name_b),
            )
            .error
            .unwrap_or_default()
    };
    assert_eq!(
        refusal("Main", "Side"),
        "A line named \"Side\" already exists"
    );
    assert_eq!(
        refusal("Side", "Main B"),
        "A line named \"Side\" already exists"
    );
    assert_eq!(
        refusal("Twin", " Twin "),
        "A line named \"Twin\" already exists"
    );
    assert_eq!(
        refusal("Main", "Main\u{202E}"),
        "A line name cannot contain control or invisible formatting characters (U+202E)"
    );
    // The preview refuses exactly what the split would.
    assert_eq!(
        state
            .split_line_preview(
                main,
                SplitAt::Station { station_index: 2 },
                &split_options("Main", "Side"),
            )
            .err()
            .as_deref(),
        Some("A line named \"Side\" already exists")
    );
    assert_eq!(project(&state).lines.len(), 2, "nothing changed");
    // The line's own name is no collision: it is the line being replaced.
    let (_, second) = split(&state, main, 2, " Main ", "Main B");
    assert_eq!(line_name(&state, main).as_deref(), Some("Main"));
    assert_eq!(line_name(&state, second).as_deref(), Some("Main B"));
}

#[test]
fn delete_and_split_refuse_a_taken_or_empty_half_name() {
    let state = new_state();
    let main = create_line(&state, "Main", 5);
    create_line(&state, "Side", 1);
    let shot_at = |i: usize| project(&state).get_line(main).unwrap().shots[i].id;
    let single = state.execute_delete_shot_and_split(
        main,
        shot_at(2),
        "Main".to_string(),
        "Side".to_string(),
    );
    assert_eq!(
        single.error.as_deref(),
        Some("A line named \"Side\" already exists")
    );
    let range = state.execute_delete_shot_range_split(
        main,
        shot_at(1),
        shot_at(2),
        " ".to_string(),
        "Main B".to_string(),
    );
    assert_eq!(range.error.as_deref(), Some("A line name cannot be empty"));
    assert_eq!(project(&state).lines.len(), 2, "nothing changed");
    let done = ok(state.execute_delete_shot_and_split(
        main,
        shot_at(2),
        " Main ".to_string(),
        "Main B ".to_string(),
    ));
    let second = done.line_id.unwrap();
    assert_eq!(line_name(&state, main).as_deref(), Some("Main"));
    assert_eq!(line_name(&state, second).as_deref(), Some("Main B"));
}

#[test]
fn combine_refuses_a_name_a_third_line_holds_and_takes_either_own_name() {
    let state = new_state();
    let main = create_line(&state, "Main", 4);
    let (_, second) = split(&state, main, 2, "Main", "Main B");
    create_line(&state, "Side", 1);
    let taken = state.execute_combine_lines(main, second, combine_options("Side"));
    assert_eq!(
        taken.error.as_deref(),
        Some("A line named \"Side\" already exists")
    );
    let empty = state.execute_combine_lines(main, second, combine_options("  "));
    assert_eq!(empty.error.as_deref(), Some("A line name cannot be empty"));
    // The other line's name is free once it is combined away.
    ok(state.execute_combine_lines(main, second, combine_options(" Main B ")));
    assert_eq!(line_name(&state, main).as_deref(), Some("Main B"));
    let names: Vec<_> = project(&state)
        .lines
        .iter()
        .map(|l| l.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["Main B", "Side"], "no unrelated line renamed");
}

#[test]
fn a_split_given_a_taken_name_by_a_direct_caller_renames_its_own_half() {
    // The model is reachable without the command's validation (tests, the
    // preview's copy): the half yields, never the unrelated line.
    let mut p = Project::new("Direct".to_string(), None);
    let data = |name: &str, shots: usize| crate::models::LineData {
        name: Some(name.to_string()),
        color: "#123456".to_string(),
        quality: 1.0,
        shots: (0..shots).map(|_| shot(0.0, 1.0)).collect(),
        ..Default::default()
    };
    let main = p.add_line(data("Main", 4)).id;
    let side = p.add_line(data("Side", 1)).id;
    let second = p
        .split_line_at_station(main, 2, &split_options("Main", "Side"))
        .unwrap();
    p.ensure_station_identities();
    assert_eq!(p.get_line(side).unwrap().name.as_deref(), Some("Side"));
    assert_eq!(
        p.get_line(second).unwrap().name.as_deref(),
        Some("Side (2)")
    );
}

#[test]
fn the_combine_candidate_names_the_renames_and_the_joined_station() {
    // The contract's example: `0 1 2 3` + `0 1 2` collide.
    let state = new_state();
    let main = create_line(&state, "Main", 3);
    let side = create_line(&state, "Side", 2);
    // Put Side's start on Main's end, so the two lines touch there.
    let end = project(&state)
        .compute_station_raw_coords(StationRef::new(main, 3))
        .unwrap();
    let start = project(&state)
        .compute_station_raw_coords(StationRef::new(side, 0))
        .unwrap();
    ok(state.execute(Command::MoveLine {
        line_id: side,
        offset: crate::models::Coordinates {
            x: end.x - start.x,
            y: end.y - start.y,
            z: 0.0,
        },
    }));
    ok(state.execute(Command::MergeStations {
        station_a: StationRef::new(main, 3),
        station_b: StationRef::new(side, 0),
    }));
    let candidate = state
        .combine_candidate_for_lines(main, side)
        .unwrap()
        .expect("the lines touch");
    let renames = candidate
        .plan_keeping_a
        .station_renames
        .clone()
        .expect("the names collide");
    assert_eq!(renames.reason, CombineRenameReason::Collision);
    let kept = renames.kept.unwrap();
    assert_eq!(
        (kept.count, kept.first_from.as_str(), kept.first_to.as_str()),
        (4, "0", "A0")
    );
    assert_eq!(
        (kept.last_from.as_str(), kept.last_to.as_str()),
        ("3", "A3")
    );
    let other = renames.other.unwrap();
    assert_eq!(
        (other.first_to.as_str(), other.last_to.as_str()),
        ("B0", "B2")
    );
    assert_eq!(renames.join_name, "A3");

    // The combine does what the candidate said.
    ok(state.execute_combine_lines(main, side, combine_options("Main")));
    let names: Vec<_> = project(&state)
        .line_station_names(main)
        .into_iter()
        .map(Option::unwrap)
        .collect();
    assert_eq!(names, ["A0", "A1", "A2", "A3", "B1", "B2"]);
}

#[test]
fn a_combine_that_renames_nothing_says_nothing() {
    // A line split from one letter family and combined back keeps its names.
    let state = new_state();
    let main = create_line(&state, "Main", 4);
    for i in 0..=4 {
        ok(state.execute(Command::SetStationName {
            station_ref: StationRef::new(main, i),
            name: format!("E{i}"),
        }));
    }
    let (_, second) = split(&state, main, 2, "Main", "Main B");
    let candidate = state
        .combine_candidate_for_lines(main, second)
        .unwrap()
        .expect("the halves touch");
    assert_eq!(candidate.plan_keeping_a.station_renames, None);
    assert_eq!(candidate.plan_keeping_b.station_renames, None);
    ok(state.execute_combine_lines(main, second, combine_options("Main")));
    let names: Vec<_> = project(&state)
        .line_station_names(main)
        .into_iter()
        .map(Option::unwrap)
        .collect();
    assert_eq!(names, ["E0", "E1", "E2", "E3", "E4"]);
}
