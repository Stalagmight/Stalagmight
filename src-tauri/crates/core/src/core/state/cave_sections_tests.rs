//! Sections - named groupings of lines (#327) - through the command path on
//! the station graph: each command and its undo/redo, name uniqueness, the
//! memberships through split / combine / delete / import / Add, the counter
//! never handing an id out twice, and the Sections and their history
//! surviving a save and a reopen.
//!
//! History labels are the history-labels package's
//! (`history_labels_tests.rs`). Split, delete-and-split and combine run their
//! model operation through the same pipeline (`run_recorded`), so the
//! membership rules are pinned whichever package ports those recipes.

use std::collections::HashSet;
use std::path::Path;

use super::commands::Record;
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{Project, RangeDeleteMode, SplitOptions};
use crate::models::{
    CombineLinesOptions, CombinePick, ImportLineConfig, ImportPlacement, LineId, SectionId,
    ShotData, SplitAt, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
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
        name: "Sections".to_string(),
        location: None,
    }));
    state
}

fn saved_state(path: &Path) -> AppState {
    let state = new_state();
    ok(state.execute(Command::SaveProject {
        path: Some(path.to_path_buf()),
    }));
    state
}

fn reopen(state: &AppState, path: &Path) {
    ok(state.execute(Command::CloseProject));
    ok(state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    }));
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    ok(state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: format!("#{:06x}", name.len() * 0x11_1111 % 0xff_ffff),
        direction: None,
        shots,
        cave_section_id: None,
    }))
    .line_id
    .unwrap()
}

fn three_lines(state: &AppState) -> (LineId, LineId, LineId) {
    (
        create(state, "A", vec![shot(90.0, 10.0)]),
        create(state, "B", vec![shot(0.0, 5.0)]),
        create(state, "C", vec![shot(180.0, 7.0)]),
    )
}

fn create_section(state: &AppState, name: &str, lines: &[LineId]) -> SectionId {
    ok(state.execute(Command::CreateCaveSection {
        name: name.to_string(),
        color: "#336699".to_string(),
        line_ids: lines.to_vec(),
    }))
    .cave_section_id
    .unwrap()
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn section_of(state: &AppState, line: LineId) -> Option<SectionId> {
    let p = project(state);
    assert!(p.line(line).is_some(), "line {line} is gone");
    p.line(line).and_then(|l| l.section_id)
}

fn section_named(state: &AppState, name: &str) -> Option<SectionId> {
    project(state)
        .sections
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.id)
}

fn members(state: &AppState, id: SectionId) -> usize {
    project(state)
        .lines()
        .iter()
        .filter(|l| l.section_id == Some(id))
        .count()
}

fn names(state: &AppState) -> Vec<String> {
    project(state)
        .sections
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

fn undo(state: &AppState) {
    ok(state.undo());
}

fn redo(state: &AppState) {
    ok(state.redo());
}

/// Run a model operation through the execute pipeline as one recorded edit.
fn run_model(
    state: &AppState,
    kind: &str,
    f: impl FnOnce(&mut Project) -> Result<(), String>,
) -> CommandResult {
    let recorded = state.run_recorded(
        |_| Record {
            kind: kind.to_string(),
            label: kind.to_string(),
            ..Record::default()
        },
        |p, _| f(p),
    );
    match recorded {
        Ok(()) => CommandResult::success(),
        Err(failure) => failure,
    }
}

#[test]
fn create_with_lines_is_one_step_that_undoes_and_redoes() {
    let state = new_state();
    let (a, b, c) = three_lines(&state);
    let result = ok(state.execute(Command::CreateCaveSection {
        name: " Upper level ".to_string(),
        color: " #336699 ".to_string(),
        line_ids: vec![a, b, b],
    }));
    let id = result.cave_section_id.unwrap();
    assert_eq!(names(&state), ["Upper level"]);
    assert_eq!(project(&state).sections[0].color, "#336699", "trimmed");
    assert_eq!(
        (
            section_of(&state, a),
            section_of(&state, b),
            section_of(&state, c)
        ),
        (Some(id), Some(id), None)
    );
    assert_eq!(
        state.get_undo_stack()[0].command_type,
        "create_cave_section"
    );

    undo(&state);
    assert!(project(&state).sections.is_empty());
    assert_eq!((section_of(&state, a), section_of(&state, b)), (None, None));

    redo(&state);
    assert_eq!(project(&state).sections[0].id, id, "the redo keeps the id");
    assert_eq!(
        (section_of(&state, a), section_of(&state, b)),
        (Some(id), Some(id))
    );
}

#[test]
fn a_user_name_must_be_unique_and_not_empty() {
    let state = new_state();
    let upper = create_section(&state, "Upper level", &[]);
    let lower = create_section(&state, "Lower level", &[]);
    let refused = state.execute(Command::CreateCaveSection {
        name: "Upper level".to_string(),
        color: "#336699".to_string(),
        line_ids: Vec::new(),
    });
    assert_eq!(
        refused.error.as_deref(),
        Some("There is already a section named \"Upper level\"")
    );
    let renamed_onto = state.execute(Command::UpdateCaveSection {
        section_id: lower,
        name: Some("Upper level".to_string()),
        color: None,
    });
    assert_eq!(
        renamed_onto.error.as_deref(),
        Some("There is already a section named \"Upper level\"")
    );
    for blank in ["", "   "] {
        let r = state.execute(Command::UpdateCaveSection {
            section_id: upper,
            name: Some(blank.to_string()),
            color: None,
        });
        assert_eq!(r.error.as_deref(), Some("A section needs a name"));
    }
    let long = state.execute(Command::CreateCaveSection {
        name: "x".repeat(201),
        color: "#336699".to_string(),
        line_ids: Vec::new(),
    });
    assert_eq!(
        long.error.as_deref(),
        Some("Section names are at most 200 characters")
    );
    let invisible = state.execute(Command::CreateCaveSection {
        name: "Upper\u{200B}".to_string(),
        color: "#336699".to_string(),
        line_ids: Vec::new(),
    });
    assert!(!invisible.success, "a forbidden character is refused");
    let colour = state.execute(Command::CreateCaveSection {
        name: "Fine".to_string(),
        color: "blue".to_string(),
        line_ids: Vec::new(),
    });
    assert_eq!(
        colour.error.as_deref(),
        Some("\"blue\" is not a section colour; use a hex colour like #4ecdc4")
    );
    let missing_line = state.execute(Command::CreateCaveSection {
        name: "Fine".to_string(),
        color: "#336699".to_string(),
        line_ids: vec![404],
    });
    assert_eq!(
        missing_line.error.as_deref(),
        Some(crate::models::LINE_GONE)
    );
    assert_eq!(names(&state), ["Upper level", "Lower level"]);
    assert_eq!(state.get_undo_stack().len(), 2, "a refusal records nothing");
}

#[test]
fn rename_and_recolour_undo_and_redo() {
    let state = new_state();
    let id = create_section(&state, "Upper level", &[]);
    ok(state.execute(Command::UpdateCaveSection {
        section_id: id,
        name: Some("Upper".to_string()),
        color: None,
    }));
    ok(state.execute(Command::UpdateCaveSection {
        section_id: id,
        name: None,
        color: Some("#abcdef".to_string()),
    }));
    let section = project(&state).sections[0].clone();
    assert_eq!(
        (section.name.as_str(), section.color.as_str()),
        ("Upper", "#abcdef")
    );

    undo(&state);
    undo(&state);
    let section = project(&state).sections[0].clone();
    assert_eq!(
        (section.name.as_str(), section.color.as_str()),
        ("Upper level", "#336699")
    );
    redo(&state);
    assert_eq!(names(&state), ["Upper"]);
    // Renaming a section to its own name changes nothing and records nothing.
    let depth = state.get_undo_stack().len();
    ok(state.execute(Command::UpdateCaveSection {
        section_id: id,
        name: Some("Upper".to_string()),
        color: None,
    }));
    assert_eq!(state.get_undo_stack().len(), depth);
    let gone = state.execute(Command::UpdateCaveSection {
        section_id: 99,
        name: Some("X".to_string()),
        color: None,
    });
    assert_eq!(
        gone.error.as_deref(),
        Some("Section 99 not found; it may have been deleted")
    );
}

#[test]
fn delete_ungroups_and_undo_restores_the_section_in_place_with_its_lines() {
    let state = new_state();
    let (a, b, c) = three_lines(&state);
    let first = create_section(&state, "First", &[c]);
    let doomed = create_section(&state, "Doomed", &[a, b]);
    let last = create_section(&state, "Last", &[]);
    ok(state.execute(Command::DeleteCaveSection { section_id: doomed }));
    assert_eq!(names(&state), ["First", "Last"]);
    assert_eq!((section_of(&state, a), section_of(&state, b)), (None, None));
    assert_eq!(section_of(&state, c), Some(first));

    undo(&state);
    assert_eq!(
        names(&state),
        ["First", "Doomed", "Last"],
        "back in its place"
    );
    assert_eq!(
        (section_of(&state, a), section_of(&state, b)),
        (Some(doomed), Some(doomed))
    );
    redo(&state);
    assert_eq!(names(&state), ["First", "Last"]);
    let fresh = create_section(&state, "Fresh", &[]);
    assert!(fresh > last, "a deleted id is not handed out again");
    assert!(
        !state
            .execute(Command::DeleteCaveSection { section_id: doomed })
            .success
    );
}

#[test]
fn moving_lines_in_and_out_is_undoable_and_refuses_what_is_missing() {
    let state = new_state();
    let (a, b, c) = three_lines(&state);
    let upper = create_section(&state, "Upper level", &[a]);
    let lower = create_section(&state, "Lower level", &[]);
    ok(state.execute(Command::MoveLinesToCaveSection {
        line_ids: vec![a, b, c],
        section_id: Some(lower),
    }));
    assert!([a, b, c]
        .iter()
        .all(|l| section_of(&state, *l) == Some(lower)));
    ok(state.execute(Command::MoveLinesToCaveSection {
        line_ids: vec![b],
        section_id: None,
    }));
    assert_eq!(section_of(&state, b), None);

    undo(&state);
    assert_eq!(section_of(&state, b), Some(lower));
    undo(&state);
    assert_eq!(
        (
            section_of(&state, a),
            section_of(&state, b),
            section_of(&state, c)
        ),
        (Some(upper), None, None),
        "each line back in the section it came from"
    );
    redo(&state);
    assert!([a, b, c]
        .iter()
        .all(|l| section_of(&state, *l) == Some(lower)));

    let depth = state.get_undo_stack().len();
    let missing_line = state.execute(Command::MoveLinesToCaveSection {
        line_ids: vec![a, 999],
        section_id: Some(upper),
    });
    assert_eq!(
        missing_line.error.as_deref(),
        Some(crate::models::LINE_GONE)
    );
    let missing_section = state.execute(Command::MoveLinesToCaveSection {
        line_ids: vec![a],
        section_id: Some(77),
    });
    assert_eq!(
        missing_section.error.as_deref(),
        Some("Section 77 not found; it may have been deleted")
    );
    assert_eq!(
        section_of(&state, a),
        Some(lower),
        "a refusal moves nothing"
    );
    // Moving lines where they already are records no step.
    ok(state.execute(Command::MoveLinesToCaveSection {
        line_ids: vec![a],
        section_id: Some(lower),
    }));
    assert_eq!(state.get_undo_stack().len(), depth);
}

#[test]
fn applying_the_colour_is_one_step_that_restores_every_line_colour() {
    let state = new_state();
    let (a, b, c) = three_lines(&state);
    let colours = |state: &AppState| -> Vec<String> {
        [a, b, c]
            .iter()
            .map(|l| project(state).line(*l).unwrap().color.clone())
            .collect()
    };
    let before = colours(&state);
    let id = create_section(&state, "Upper level", &[a, b]);
    let depth = state.get_undo_stack().len();
    ok(state.execute(Command::ApplyCaveSectionColor { section_id: id }));
    assert_eq!(state.get_undo_stack().len(), depth + 1, "one step");
    assert_eq!(colours(&state), ["#336699", "#336699", before[2].as_str()]);
    undo(&state);
    assert_eq!(colours(&state), before);
    redo(&state);
    assert_eq!(colours(&state)[0], "#336699");
    // Applying it again changes nothing and records nothing.
    ok(state.execute(Command::ApplyCaveSectionColor { section_id: id }));
    assert_eq!(state.get_undo_stack().len(), depth + 1);
}

#[test]
fn section_commands_batch_into_one_step() {
    let state = new_state();
    let (a, b, _) = three_lines(&state);
    let id = create_section(&state, "Upper level", &[]);
    let depth = state.get_undo_stack().len();
    let r = ok(state.execute(Command::Batch {
        commands: vec![
            Command::MoveLinesToCaveSection {
                line_ids: vec![a, b],
                section_id: Some(id),
            },
            Command::ApplyCaveSectionColor { section_id: id },
        ],
        label: None,
    }));
    assert_eq!(r.cave_section_id, None, "a batch names no Section");
    assert_eq!(state.get_undo_stack().len(), depth + 1);
    assert_eq!(members(&state, id), 2);
    undo(&state);
    assert_eq!(members(&state, id), 0);
}

#[test]
fn a_split_keeps_both_halves_in_the_section() {
    let state = new_state();
    let main = create(
        &state,
        "Main",
        vec![shot(45.0, 5.0), shot(45.0, 7.0), shot(45.0, 3.0)],
    );
    let id = create_section(&state, "Upper level", &[main]);
    let options = SplitOptions {
        name_a: "Main-A".to_string(),
        name_b: "Main-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
    };
    ok(run_model(&state, "split_line", |p| {
        p.split_line(main, SplitAt::Station { station_index: 1 }, &options)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }));
    let second = project(&state)
        .lines()
        .iter()
        .find(|l| l.id != main)
        .map(|l| l.id)
        .unwrap();
    assert_eq!(
        (section_of(&state, main), section_of(&state, second)),
        (Some(id), Some(id))
    );
    undo(&state);
    assert!(project(&state).line(second).is_none());
    assert_eq!(section_of(&state, main), Some(id));
}

#[test]
fn a_delete_and_split_keeps_the_tail_in_the_section() {
    let state = new_state();
    let main = create(
        &state,
        "Main",
        vec![
            shot(45.0, 5.0),
            shot(45.0, 7.0),
            shot(45.0, 3.0),
            shot(45.0, 4.0),
        ],
    );
    let id = create_section(&state, "Upper level", &[main]);
    let middle = project(&state).line(main).unwrap().shots[1].id;
    ok(run_model(&state, "delete_shot_and_split", |p| {
        p.delete_shot_range(
            main,
            middle,
            middle,
            &RangeDeleteMode::Split {
                name_a: "A".to_string(),
                name_b: "B".to_string(),
            },
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }));
    let tail = project(&state)
        .lines()
        .iter()
        .find(|l| l.id != main)
        .map(|l| l.id)
        .unwrap();
    assert_eq!(
        (section_of(&state, main), section_of(&state, tail)),
        (Some(id), Some(id))
    );
}

#[test]
fn a_combine_keeps_the_kept_lines_section() {
    let state = new_state();
    let k = create(&state, "K", vec![shot(90.0, 10.0), shot(90.0, 5.0)]);
    let o = create(&state, "O", vec![shot(0.0, 5.0), shot(0.0, 4.0)]);
    let kept_section = create_section(&state, "Kept", &[k]);
    let other_section = create_section(&state, "Other", &[o]);
    ok(state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 2),
        station_b: StationRef::new(o, 0),
        keep: None,
        renames: None,
    }));
    let options = CombineLinesOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    };
    ok(run_model(&state, "combine_lines", |p| {
        p.combine_lines(k, o, &options)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }));
    assert!(project(&state).line(o).is_none());
    assert_eq!(section_of(&state, k), Some(kept_section));
    undo(&state);
    assert_eq!(section_of(&state, o), Some(other_section));
}

#[test]
fn deleting_a_line_drops_its_membership_and_undo_puts_it_back() {
    let state = new_state();
    let (a, b, _) = three_lines(&state);
    let id = create_section(&state, "Upper level", &[a, b]);
    ok(state.execute(Command::DeleteLine { line_id: a }));
    assert_eq!(members(&state, id), 1);
    undo(&state);
    assert_eq!(section_of(&state, a), Some(id));
    // Deleting the Section after the line came back still undoes cleanly.
    ok(state.execute(Command::DeleteCaveSection { section_id: id }));
    undo(&state);
    assert_eq!(
        (section_of(&state, a), section_of(&state, b)),
        (Some(id), Some(id))
    );
}

#[test]
fn a_line_created_into_a_section_undoes_and_redoes_with_its_membership() {
    let state = new_state();
    let id = create_section(&state, "Upper level", &[]);
    let create = |section| {
        state.execute(Command::CreateLine {
            name: Some("New".to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: None,
            shots: vec![shot(10.0, 4.0), shot(20.0, 3.0)],
            cave_section_id: section,
        })
    };
    let refused = create(Some(99));
    assert_eq!(
        refused.error.as_deref(),
        Some("Section 99 not found; it may have been deleted"),
        "a missing section is refused"
    );
    assert_eq!(state.get_undo_stack().len(), 1);
    assert!(project(&state).lines().is_empty());
    let line = ok(create(Some(id))).line_id.unwrap();
    assert_eq!(section_of(&state, line), Some(id));
    assert_eq!(state.get_undo_stack().len(), 2, "one step");
    // Every new station has its Station ID and its name straight away.
    let identity = |state: &AppState| -> Vec<(u32, String)> {
        let p = project(state);
        (0..3)
            .map(|i| {
                let station = p.station_at(StationRef::new(line, i)).unwrap();
                (station, p.stations[&station].name.clone())
            })
            .collect()
    };
    let created = identity(&state);
    let ids: HashSet<u32> = created.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids.len(), 3, "every station has its own id: {created:?}");
    assert!(
        created.iter().all(|(_, name)| !name.is_empty()),
        "every station has a name: {created:?}"
    );
    undo(&state);
    assert!(project(&state).line(line).is_none());
    redo(&state);
    assert_eq!(section_of(&state, line), Some(id));
    assert_eq!(
        identity(&state),
        created,
        "redo puts back the same ids and names"
    );
    undo(&state);
    redo(&state);
    assert_eq!(identity(&state), created);
    // Without a section it is a plain create.
    let plain = ok(create(None)).line_id.unwrap();
    assert_eq!(section_of(&state, plain), None);
}

/// A New Line placed by snapping onto a station goes in its Section inside
/// the create-and-merge step: ONE undo removes the line, its junction and
/// its membership; one redo brings all three back.
#[test]
fn a_snapped_new_line_joins_its_section_in_one_undo_step() {
    let state = new_state();
    let (a, _, _) = three_lines(&state);
    let id = create_section(&state, "Upper level", &[]);
    let merges_before = project(&state).merge_pairs().len();
    let steps_before = state.get_undo_stack().len();
    let create = |section| {
        state.execute_create_and_merge_in_cave_section(
            Some("Snapped".to_string()),
            None,
            None,
            "#123456".to_string(),
            None,
            vec![shot(45.0, 6.0)],
            0,
            StationRef::new(a, 1),
            crate::models::ClosureStrategy::Move,
            section,
        )
    };
    let refused = create(Some(99));
    assert!(!refused.success, "a missing section is refused");
    assert_eq!(state.get_undo_stack().len(), steps_before);
    assert_eq!(project(&state).lines().len(), 3);

    let line = ok(create(Some(id))).line_id.unwrap();
    assert_eq!(section_of(&state, line), Some(id));
    assert_eq!(project(&state).merge_pairs().len(), merges_before + 1);
    assert_eq!(state.get_undo_stack().len(), steps_before + 1, "one step");

    undo(&state);
    assert!(project(&state).line(line).is_none());
    assert_eq!(project(&state).merge_pairs().len(), merges_before);
    assert_eq!(members(&state, id), 0);
    redo(&state);
    assert_eq!(section_of(&state, line), Some(id));
    assert_eq!(project(&state).merge_pairs().len(), merges_before + 1);
}

fn import_config(index: u32, name: &str, section: Option<String>) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: section,
        line_index: index,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        surveyors: None,
        explorers: None,
        override_date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(90.0, 5.0 + f64::from(index))],
        placement: ImportPlacement::Origin,
        station_comments: Vec::new(),
        station_names: Vec::new(),
        station_external: Vec::new(),
        reverse: false,
        azimuth_reference: None,
    }
}

#[test]
fn an_import_groups_by_name_and_reuses_a_same_name_section() {
    let state = new_state();
    let own = create(&state, "Own", vec![shot(0.0, 3.0)]);
    let existing = create_section(&state, "Gallery", &[own]);
    let in_section = |name: &str| Some(name.to_string());
    let result = ok(state.execute_import_lines(
        vec![
            import_config(0, "Gallery", in_section("Gallery")),
            import_config(1, "Gallery (branch 2)", in_section("Gallery")),
            import_config(2, "Sump", in_section("Sump")),
            import_config(3, "Sump (trip 2)", in_section("Sump")),
            import_config(4, "Loose", None),
        ],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        super::VirtualShotMode::Auto,
    ));
    let lines = result.line_ids;
    assert_eq!(lines.len(), 5);
    let sump = section_named(&state, "Sump").unwrap();
    assert_eq!(
        lines
            .iter()
            .map(|l| section_of(&state, *l))
            .collect::<Vec<_>>(),
        [Some(existing), Some(existing), Some(sump), Some(sump), None]
    );
    assert_eq!(names(&state), ["Gallery", "Sump"]);
    undo(&state);
    assert_eq!(
        names(&state),
        ["Gallery"],
        "the import's own section goes with it"
    );
    assert_eq!(section_of(&state, own), Some(existing));
}

#[test]
fn an_added_project_brings_its_sections_with_fresh_ids_and_deduped_names() {
    let dir = tempfile::TempDir::new().unwrap();
    let source_path = dir.path().join("source.stalx");
    let source = saved_state(&source_path);
    let s1 = create(&source, "S1", vec![shot(10.0, 5.0)]);
    let s2 = create(&source, "S2", vec![shot(20.0, 5.0)]);
    create_section(&source, "Unused", &[]);
    create_section(&source, "Gallery", &[s1, s2]);
    ok(source.execute(Command::SaveProject { path: None }));
    ok(source.execute(Command::CloseProject));

    let state = saved_state(&dir.path().join("target.stalx"));
    let own = create(&state, "Own", vec![shot(0.0, 3.0)]);
    let target_gallery = create_section(&state, "Gallery", &[own]);
    ok(state.execute(Command::add_project_file(source_path)));
    assert_eq!(names(&state), ["Gallery", "Unused", "Gallery (2)"]);
    let added = section_named(&state, "Gallery (2)").unwrap();
    assert_ne!(added, target_gallery);
    let added_lines: Vec<LineId> = project(&state)
        .lines()
        .iter()
        .filter(|l| l.name.as_deref().is_some_and(|n| n.starts_with('S')))
        .map(|l| l.id)
        .collect();
    assert_eq!(added_lines.len(), 2);
    assert!(added_lines
        .iter()
        .all(|l| section_of(&state, *l) == Some(added)));
    assert_eq!(section_of(&state, own), Some(target_gallery));

    undo(&state);
    assert_eq!(names(&state), ["Gallery"]);
    redo(&state);
    assert_eq!(names(&state), ["Gallery", "Unused", "Gallery (2)"]);
}

#[test]
fn sections_and_their_history_survive_a_save_and_a_reopen() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("history.stalx");
    let state = saved_state(&path);
    let (a, b, c) = three_lines(&state);
    let upper = create_section(&state, "Upper level", &[a]);
    ok(state.execute(Command::MoveLinesToCaveSection {
        line_ids: vec![b, c],
        section_id: Some(upper),
    }));
    let lower = create_section(&state, "Lower level", &[]);
    ok(state.execute(Command::DeleteCaveSection { section_id: lower }));
    ok(state.execute(Command::SaveProject { path: None }));
    let kinds: Vec<String> = state
        .get_undo_stack()
        .iter()
        .take(4)
        .map(|item| item.command_type.clone())
        .collect();

    reopen(&state, &path);
    assert_eq!(names(&state), ["Upper level"]);
    assert!([a, b, c]
        .iter()
        .all(|l| section_of(&state, *l) == Some(upper)));
    let reopened: Vec<String> = state
        .get_undo_stack()
        .iter()
        .take(4)
        .map(|item| item.command_type.clone())
        .collect();
    assert_eq!(reopened, kinds);
    assert_eq!(
        kinds,
        [
            "delete_cave_section",
            "create_cave_section",
            "move_lines_to_cave_section",
            "create_cave_section",
        ]
    );

    undo(&state);
    assert_eq!(names(&state), ["Upper level", "Lower level"]);
    undo(&state);
    undo(&state);
    assert_eq!((section_of(&state, b), section_of(&state, c)), (None, None));
    undo(&state);
    assert!(project(&state).sections.is_empty());
    for _ in 0..4 {
        redo(&state);
    }
    assert_eq!(names(&state), ["Upper level"]);
    assert!([a, b, c]
        .iter()
        .all(|l| section_of(&state, *l) == Some(upper)));
}

/// The IPC vocabulary: a Section id travels as `cave_section_id`, never
/// `section_id` (the persisted name of a LINE id in 0.6.0's files), and every
/// Section command reads back what it wrote.
#[test]
fn every_section_command_round_trips_through_its_wire_vocabulary() {
    for command in [
        Command::CreateCaveSection {
            name: "S".to_string(),
            color: "#123456".to_string(),
            line_ids: vec![1, 2],
        },
        Command::UpdateCaveSection {
            section_id: 3,
            name: Some("T".to_string()),
            color: Some("#654321".to_string()),
        },
        Command::DeleteCaveSection { section_id: 3 },
        Command::MoveLinesToCaveSection {
            line_ids: vec![4],
            section_id: None,
        },
        Command::ApplyCaveSectionColor { section_id: 3 },
    ] {
        let wire = serde_json::to_string(&command).unwrap();
        assert!(!wire.contains("\"section_id\""), "{wire}");
        let back: Command = serde_json::from_str(&wire).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), wire);
    }
}

/// Hiding a Section is a view setting: durable at once, read back on the
/// next open, and no undo step.
#[test]
fn hidden_sections_are_a_saved_view_setting_not_an_edit() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("hidden.stalx");
    let state = saved_state(&path);
    let upper = create_section(&state, "Upper level", &[]);
    let depth = state.get_undo_stack().len();
    state
        .save_display_setting(
            crate::core::loading::HIDDEN_SECTIONS_KEY,
            &format!("[{upper}]"),
        )
        .unwrap();
    assert_eq!(state.get_undo_stack().len(), depth, "no undo step");
    ok(state.execute(Command::SaveProject { path: None }));
    reopen(&state, &path);
    let stored = state.load_display_settings().unwrap();
    assert_eq!(
        crate::core::loading::DisplaySettings::from_stored(&stored).hidden_sections,
        [upper]
    );
}

/// Section ids are never handed out twice in a timeline: an undone or
/// deleted Section keeps its id retired through an undo and through a save
/// and reopen (`hiddenSections` names Sections by id).
#[test]
fn a_section_id_is_never_reused_after_undo_delete_or_reopen() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("ids.stalx");
    let state = saved_state(&path);
    let (a, _, _) = three_lines(&state);
    let first = create_section(&state, "First", &[]);
    let undone = create_section(&state, "Undone", &[]);
    undo(&state);
    let after_undo = create_section(&state, "After undo", &[]);
    assert!(after_undo > undone, "{after_undo} reuses {undone}");

    // An edit recorded before a Section was created: undoing it does not
    // lower the counter either.
    ok(state.execute(Command::DeleteLine { line_id: a }));
    let made = create_section(&state, "Made", &[]);
    undo(&state); // the Section
    undo(&state); // the delete
    let after_restore = create_section(&state, "After restore", &[]);
    assert!(after_restore > made, "{after_restore} reuses {made}");

    // Delete the highest, save, reopen: its id stays retired.
    ok(state.execute(Command::DeleteCaveSection {
        section_id: after_restore,
    }));
    ok(state.execute(Command::SaveProject { path: None }));
    reopen(&state, &path);
    assert!(project(&state).sections.iter().any(|s| s.id == first));
    let after_reopen = create_section(&state, "After reopen", &[]);
    assert!(
        after_reopen > after_restore,
        "{after_reopen} reuses {after_restore}"
    );
}
