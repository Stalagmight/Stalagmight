//! The name rules at the command layer: a rename to the name a station
//! already has is a no-op, control and invisible formatting characters are
//! refused, names are stored and compared trimmed, a junction's name is
//! unique on every one of its lines (plan 5.3), Split and Combine refuse a
//! line name a rename would refuse (and never rename an unrelated line), and
//! the Combine candidate tells the dialog which stations it renames.
//!
//! Lines are built with the model's own `create_line` (what Create Line
//! does). Every case runs: the commands they drive (Split Line Here and
//! Combine, delete-and-split, line renames and Sections) are all on the
//! station graph, and so is their setup.
//!
//! Retired from 0.6.0: the half of `names_are_stored_and_compared_trimmed`
//! that ran `Project::ensure_station_identities` over padded and repeated
//! names a file could hold. There is no identity epilogue on the station
//! graph (identities are structural, a v2 load is pure, and `validate`
//! refuses repeated names); a legacy file's names are settled by the
//! migration. `a_split_given_a_taken_name_by_a_direct_caller_renames_its_own_half`
//! is adapted: the model's Split refuses the taken name itself, so there is
//! no unvalidated path that could rename either line.

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{LineMeta, Project, SplitOptions, StartAt};
use crate::models::{
    CombineJoinEnds, CombineLinesOptions, CombinePick, CombineRenameReason, Coordinates, LineEnd,
    LineId, LineUpdates, ShotData, SplitAt, SplitLineOptions, StationRef, LINE_GONE,
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

fn refused(result: CommandResult) -> String {
    assert!(!result.success, "the command was expected to be refused");
    result.error.unwrap_or_default()
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    ok(state.execute(Command::CreateProject {
        name: "Names".to_string(),
        location: None,
    }));
    state
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// Change the open project directly: a fixture outside the history.
fn edit<R>(state: &AppState, f: impl FnOnce(&mut Project) -> R) -> R {
    let mut p = project(state);
    let out = f(&mut p);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    state.set_project_for_tests(p);
    out
}

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#4ecdc4".to_string(),
        ..LineMeta::default()
    }
}

/// A new free line of `shots` shots heading east (what Create Line makes).
fn create_line(state: &AppState, name: &str, shots: usize) -> LineId {
    edit(state, |p| {
        let shots = (0..shots).map(|_| shot(90.0, 10.0)).collect();
        p.create_line(meta(name), StartAt::New, shots, None)
            .unwrap()
            .value
    })
}

/// A new line starting at `at`'s station: a junction there.
fn create_branch(state: &AppState, name: &str, at: StationRef, shots: usize) -> LineId {
    edit(state, |p| {
        let start = p.station_at(at).unwrap();
        let shots = (0..shots).map(|_| shot(0.0, 5.0)).collect();
        p.create_line(meta(name), StartAt::Station(start), shots, None)
            .unwrap()
            .value
    })
}

fn line_name(state: &AppState, line: LineId) -> Option<String> {
    project(state).line(line).and_then(|l| l.name.clone())
}

/// The station names along `line`.
fn names_on(state: &AppState, line: LineId) -> Vec<String> {
    let p = project(state);
    p.line(line)
        .unwrap()
        .stations()
        .map(|s| p.stations[&s].name.clone())
        .collect()
}

fn name_at(state: &AppState, slot: StationRef) -> String {
    let p = project(state);
    p.stations[&p.station_at(slot).unwrap()].name.clone()
}

fn rename(state: &AppState, slot: StationRef, name: &str) -> CommandResult {
    state.execute(Command::SetStationName {
        station_ref: slot,
        name: name.to_string(),
    })
}

fn split_options(name_a: &str, name_b: &str) -> SplitLineOptions {
    SplitLineOptions {
        name_a: name_a.to_string(),
        name_b: name_b.to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
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

// === Station names ===

#[test]
fn renaming_a_station_to_its_own_name_leaves_no_history() {
    let state = new_state();
    let main = create_line(&state, "Main", 2);
    let station = StationRef::new(main, 1);
    ok(rename(&state, station, "sump"));
    ok(state.undo());
    assert!(state.can_redo());
    let history = state.get_undo_stack().len();
    let modified_at = project(&state).modified_at;

    // " 1 " is "1" once trimmed: the station's name already.
    ok(rename(&state, station, " 1 "));
    assert_eq!(state.get_undo_stack().len(), history, "no undo entry");
    assert!(state.can_redo(), "the redo branch survives");
    assert_eq!(project(&state).modified_at, modified_at, "not touched");
    assert_eq!(name_at(&state, station), "1");
}

/// The `command` field of every event `run` emits on this thread, as a
/// formatter renders it (`%` values raw, `?` values escaped).
fn commands_logged_during(run: impl FnOnce()) -> Vec<String> {
    use std::sync::{Arc, Mutex};

    struct Capture(Arc<Mutex<Vec<String>>>);
    struct CommandField(Option<String>);
    impl tracing::field::Visit for CommandField {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "command" {
                self.0 = Some(format!("{value:?}"));
            }
        }
    }
    impl tracing::Subscriber for Capture {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut field = CommandField(None);
            event.record(&mut field);
            if let Some(command) = field.0 {
                self.0.lock().unwrap().push(command);
            }
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    let logged = Arc::new(Mutex::new(Vec::new()));
    // With a single dispatcher registered, tracing works out a callsite's
    // interest from whichever thread hits it first - another test's, with no
    // subscriber - and caches "never". A second one, kept alive throughout,
    // makes it ask every registered dispatcher, this one included.
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    tracing::subscriber::with_default(Capture(Arc::clone(&logged)), run);
    let logged = logged.lock().unwrap().clone();
    logged
}

/// Command descriptions quote station and Section names, which a file can
/// fill with anything: they are logged escaped, so a name holding a newline
/// cannot forge a log line.
#[test]
fn a_name_with_a_newline_is_logged_escaped() {
    let state = new_state();
    let main = create_line(&state, "Main", 2);
    let station = StationRef::new(main, 1);
    let forged = "1\nERROR forged line";
    edit(&state, |p| {
        let id = p.station_at(station).unwrap();
        p.stations.get_mut(&id).unwrap().name = forged.to_string();
    });
    // The rename to the name the station has is the no-op path's log.
    let logged = commands_logged_during(|| {
        ok(rename(&state, station, forged));
    });
    assert!(!logged.is_empty(), "the command is logged");
    assert!(
        logged.iter().all(|command| !command.contains('\n')),
        "a raw newline reached the log: {logged:?}"
    );
}

#[test]
fn control_and_invisible_characters_are_refused_in_station_names() {
    let state = new_state();
    let main = create_line(&state, "Main", 2);
    assert_eq!(
        refused(rename(&state, StationRef::new(main, 1), "A\u{202E}B")),
        "A station name cannot contain control or invisible formatting characters (U+202E)"
    );
    assert!(state
        .refuse_new_line_name(Some("New\u{200D}Line"))
        .is_some());
    // Visible separators stay allowed.
    ok(rename(&state, StationRef::new(main, 1), "1 / sump"));
    assert_eq!(name_at(&state, StationRef::new(main, 1)), "1 / sump");
}

#[test]
fn control_and_invisible_characters_are_refused_in_line_and_section_names() {
    let state = new_state();
    let main = create_line(&state, "Main", 2);
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
    ok(state.execute(Command::UpdateLineMetadata {
        line_id: main,
        updates: LineUpdates {
            name: Some(Some("Main / upstream".to_string())),
            ..Default::default()
        },
    }));
}

#[test]
fn station_names_are_stored_trimmed_and_line_names_compared_trimmed() {
    let state = new_state();
    let main = create_line(&state, "Main", 1);
    ok(rename(&state, StationRef::new(main, 1), "  sump\u{20}"));
    assert_eq!(name_at(&state, StationRef::new(main, 1)), "sump");
    assert!(
        state.refuse_new_line_name(Some("Main ")).is_some(),
        "a padded copy of a taken name is the taken name"
    );
    assert!(state.refuse_new_line_name(Some(" Side ")).is_none());
    assert_eq!(
        state
            .refuse_new_line_name(None)
            .and_then(|r| r.error)
            .as_deref(),
        Some("A line name cannot be empty")
    );
}

#[test]
fn line_names_are_stored_trimmed() {
    let state = new_state();
    let main = ok(state.execute(Command::CreateLine {
        name: Some(" Main ".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot(90.0, 10.0)],
        cave_section_id: None,
    }))
    .line_id
    .unwrap();
    assert_eq!(line_name(&state, main).as_deref(), Some("Main"));
    ok(state.execute(Command::UpdateLineMetadata {
        line_id: main,
        updates: LineUpdates {
            name: Some(Some("  Upstream\u{20}".to_string())),
            ..Default::default()
        },
    }));
    assert_eq!(line_name(&state, main).as_deref(), Some("Upstream"));
}

#[test]
fn a_rename_is_refused_in_0_6_0_words() {
    let state = new_state();
    let main = create_line(&state, "Main", 2);
    ok(rename(&state, StationRef::new(main, 2), "sump"));
    assert_eq!(
        refused(rename(&state, StationRef::new(main, 1), "sump")),
        "Line \"Main\" already has a station named \"sump\""
    );
    assert_eq!(
        refused(rename(&state, StationRef::new(main, 1), "   ")),
        "A station name cannot be empty"
    );
    assert_eq!(
        refused(rename(&state, StationRef::new(main, 9), "x")),
        "Station not found"
    );
    assert_eq!(
        refused(rename(&state, StationRef::new(77, 0), "x")),
        LINE_GONE
    );
    assert_eq!(state.get_undo_stack().len(), 1, "only the first rename");
}

#[test]
fn a_junction_name_is_unique_on_every_one_of_its_lines() {
    // Main 0-1-2-3, Side starts at Main's 1 (a junction): its stations are
    // the junction, then Side's own.
    let state = new_state();
    let main = create_line(&state, "Main", 3);
    let side = create_branch(&state, "Side", StationRef::new(main, 1), 2);
    ok(rename(&state, StationRef::new(side, 2), "far"));
    let junction = StationRef::new(side, 0);

    // A name Main holds, asked on Side's slot: Main is named.
    assert_eq!(
        refused(rename(&state, junction, "3")),
        "Line \"Main\" already has a station named \"3\""
    );
    // A name only Side holds, asked on Main's slot: Side is named.
    assert_eq!(
        refused(rename(&state, StationRef::new(main, 1), "far")),
        "Line \"Side\" already has a station named \"far\""
    );
    // Free on both lines: one name on every slot.
    ok(rename(&state, junction, "J"));
    assert_eq!(name_at(&state, StationRef::new(main, 1)), "J");
    assert_eq!(names_on(&state, side)[0], "J");
    // And it now blocks the name on both lines.
    assert!(refused(rename(&state, StationRef::new(main, 3), "J")).contains("\"Main\""));
    assert!(refused(rename(&state, StationRef::new(side, 1), "J")).contains("\"Side\""));
}

#[test]
fn a_pasted_column_of_names_is_one_step_and_all_or_nothing() {
    let state = new_state();
    let main = create_line(&state, "Main", 3);
    let paste = |names: &[&str]| Command::Batch {
        commands: names
            .iter()
            .enumerate()
            .map(|(i, name)| Command::SetStationName {
                station_ref: StationRef::new(main, u32::try_from(i).unwrap()),
                name: (*name).to_string(),
            })
            .collect(),
        label: None,
    };
    ok(state.execute(paste(&["E0", "E1", "E2", "E3"])));
    assert_eq!(names_on(&state, main), ["E0", "E1", "E2", "E3"]);
    assert_eq!(state.get_undo_stack().len(), 1, "one entry for the paste");

    // The third name repeats the first: the whole paste is refused.
    let error = refused(state.execute(paste(&["x", "y", "x", "z"])));
    assert_eq!(error, "Line \"Main\" already has a station named \"x\"");
    assert_eq!(names_on(&state, main), ["E0", "E1", "E2", "E3"]);
    assert_eq!(state.get_undo_stack().len(), 1);

    ok(state.undo());
    assert_eq!(names_on(&state, main), ["0", "1", "2", "3"]);
    ok(state.redo());
    assert_eq!(names_on(&state, main), ["E0", "E1", "E2", "E3"]);
}

#[test]
fn a_rename_survives_save_and_reopen_and_undoes_there() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("names.stalx");
    let state = new_state();
    let main = create_line(&state, "Main", 2);
    ok(rename(&state, StationRef::new(main, 1), "sump"));
    ok(state.save_project(Some(path.clone())));

    let reopened = AppState::new().unwrap();
    ok(reopened.open_project(path));
    assert_eq!(name_at(&reopened, StationRef::new(main, 1)), "sump");
    ok(reopened.undo());
    assert_eq!(name_at(&reopened, StationRef::new(main, 1)), "1");
}

// === Split and Combine names ===

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
    assert_eq!(project(&state).lines().len(), 2, "nothing changed");
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
    let shot_at = |i: usize| project(&state).line(main).unwrap().shots[i].id;
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
    assert_eq!(project(&state).lines().len(), 2, "nothing changed");
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
        .lines()
        .iter()
        .map(|l| l.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["Main B", "Side"], "no unrelated line renamed");
}

#[test]
fn a_split_given_a_taken_name_by_a_direct_caller_is_refused_and_renames_nothing() {
    // 0.6.0's model could be called without the command's validation and
    // then renamed its own half. The station graph's Split validates the
    // names itself, so no caller can make it rename either line.
    let state = new_state();
    let main = create_line(&state, "Main", 4);
    let side = create_line(&state, "Side", 1);
    let mut p = project(&state);
    let before = p.clone();
    let refusal = p
        .split_line(
            main,
            SplitAt::Station { station_index: 2 },
            &SplitOptions {
                name_a: "Main".to_string(),
                name_b: "Side".to_string(),
                color_a: "#111111".to_string(),
                color_b: "#222222".to_string(),
            },
        )
        .err()
        .map(|e| e.to_string());
    assert_eq!(
        refusal.as_deref(),
        Some("A line named \"Side\" already exists")
    );
    assert_eq!(p, before, "nothing changed");
    assert_eq!(p.line(side).unwrap().name.as_deref(), Some("Side"));
}

/// Put `side`'s start on `main`'s last station (`main_end`) and merge them,
/// so the two lines touch there and can be combined. The merge moves
/// `side` onto `main` (`station_a`'s piece moves), so the junction keeps
/// Main's name and Side's own names are made unique around it; merged the
/// other way, the junction would take Side's `0` and letter Main's.
fn merge_start_onto_end(state: &AppState, main: LineId, main_end: u32, side: LineId) {
    let position = |line: LineId, index: u32| {
        let p = project(state);
        p.raw_positions()
            .slot(&p, StationRef::new(line, index))
            .unwrap()
    };
    let (end, start) = (position(main, main_end), position(side, 0));
    ok(state.execute(Command::MoveLine {
        line_id: side,
        offset: Coordinates {
            x: end[0] - start[0],
            y: end[1] - start[1],
            z: 0.0,
        },
    }));
    ok(state.execute(Command::MergeStations {
        station_a: StationRef::new(side, 0),
        station_b: StationRef::new(main, main_end),
        keep: None,
        renames: None,
    }));
}

#[test]
fn the_combine_candidate_names_the_renames_and_the_joined_station() {
    // The contract's example: `0 1 2 3` + `0 1 2` collide.
    let state = new_state();
    let main = create_line(&state, "Main", 3);
    let side = create_line(&state, "Side", 2);
    merge_start_onto_end(&state, main, 3, side);
    let candidate = state
        .combine_candidate_for_lines(main, side)
        .unwrap()
        .unwrap();
    let renames = candidate.plan_keeping_a.station_renames.unwrap();
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
    assert_eq!(kept.unchanged, 0);
    // The other line's join station is no rename: it becomes `A3`.
    let other = renames.other.unwrap();
    assert_eq!(
        (
            other.count,
            other.first_from.as_str(),
            other.first_to.as_str(),
            other.last_to.as_str()
        ),
        (2, "1", "B1", "B2")
    );
    assert_eq!(renames.suffixed, None);
    assert_eq!(renames.join_name, "A3");

    // The combine does what the candidate said.
    ok(state.execute_combine_lines(main, side, combine_options("Main")));
    assert_eq!(names_on(&state, main), ["A0", "A1", "A2", "A3", "B1", "B2"]);
}

#[test]
fn a_combine_keeps_custom_names_and_says_how_many_it_keeps() {
    // Main `0 1 Junction Sump 4 .. 8`, Side `0 1 Sump 3` joined at Main's
    // end: the digits collide and are lettered, Main's custom names stay, and
    // Side's `Sump` (Main has one) becomes `Sump-a`.
    let state = new_state();
    let main = create_line(&state, "Main", 8);
    let side = create_line(&state, "Side", 3);
    for (line, index, name) in [(main, 2, "Junction"), (main, 3, "Sump"), (side, 2, "Sump")] {
        ok(rename(&state, StationRef::new(line, index), name));
    }
    merge_start_onto_end(&state, main, 8, side);
    let candidate = state
        .combine_candidate_for_lines(main, side)
        .unwrap()
        .unwrap();
    let renames = candidate.plan_keeping_a.station_renames.unwrap();
    assert_eq!(renames.reason, CombineRenameReason::Collision);
    let kept = renames.kept.unwrap();
    assert_eq!((kept.count, kept.unchanged), (7, 2));
    assert_eq!(
        (kept.first_from.as_str(), kept.first_to.as_str()),
        ("0", "A0")
    );
    assert_eq!(
        (kept.last_from.as_str(), kept.last_to.as_str()),
        ("8", "A8")
    );
    // Side's lettering is `1` and `3`; its `Sump` is listed on its own.
    let other = renames.other.unwrap();
    assert_eq!((other.count, other.unchanged), (2, 0));
    assert_eq!(
        (
            other.first_from.as_str(),
            other.first_to.as_str(),
            other.last_from.as_str(),
            other.last_to.as_str()
        ),
        ("1", "B1", "3", "B3")
    );
    let suffixed = renames.suffixed.unwrap();
    assert_eq!(
        suffixed
            .iter()
            .map(|r| (r.from.as_str(), r.to.as_str()))
            .collect::<Vec<_>>(),
        [("Sump", "Sump-a")]
    );
    assert_eq!(renames.join_name, "A8");

    ok(state.execute_combine_lines(main, side, combine_options("Main")));
    assert_eq!(
        names_on(&state, main),
        ["A0", "A1", "Junction", "Sump", "A4", "A5", "A6", "A7", "A8", "B1", "Sump-a", "B3"]
    );
}

#[test]
fn a_combine_that_renames_nothing_says_nothing() {
    // A line split from one letter family and combined back keeps its names.
    let state = new_state();
    let main = create_line(&state, "Main", 4);
    for i in 0..=4 {
        ok(rename(&state, StationRef::new(main, i), &format!("E{i}")));
    }
    let (_, second) = split(&state, main, 2, "Main", "Main B");
    let candidate = state
        .combine_candidate_for_lines(main, second)
        .unwrap()
        .unwrap();
    assert_eq!(candidate.plan_keeping_a.station_renames, None);
    assert_eq!(candidate.plan_keeping_b.station_renames, None);
    ok(state.execute_combine_lines(main, second, combine_options("Main")));
    assert_eq!(names_on(&state, main), ["E0", "E1", "E2", "E3", "E4"]);
}
