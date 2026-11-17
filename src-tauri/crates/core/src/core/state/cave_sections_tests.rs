//! Sections - named groupings of lines (#327) - through the command path:
//! each command and its undo/redo, the history labels, name uniqueness, the
//! memberships through split / combine / delete / import / Add, and the
//! history and the Sections surviving a save and a reopen.

use std::path::Path;

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    disk_names, CombineLinesOptions, CombinePick, ImportLineConfig, ImportPlacement, LineId,
    Project, Section, SectionId, ShotData, SplitAt, SplitLineOptions, SplitSide, StationRef,
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

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    ok(state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: format!("#{:06x}", name.len() * 0x11_1111 % 0xff_ffff),
        direction: None,
        shots,
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
    project(state).get_line(line).unwrap().section_id
}

fn names(state: &AppState) -> Vec<String> {
    project(state)
        .sections
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

fn last_label(state: &AppState) -> String {
    state.get_undo_stack()[0].description.clone()
}

fn undo(state: &AppState) {
    ok(state.undo());
}

fn redo(state: &AppState) {
    ok(state.redo());
}

#[test]
fn create_with_lines_is_one_step_that_undoes_and_redoes() {
    let state = new_state();
    let (a, b, c) = three_lines(&state);
    let id = create_section(&state, " Upper level ", &[a, b, b]);
    assert_eq!(names(&state), ["Upper level"]);
    assert_eq!(
        (
            section_of(&state, a),
            section_of(&state, b),
            section_of(&state, c)
        ),
        (Some(id), Some(id), None)
    );
    assert_eq!(
        last_label(&state),
        "Create section \"Upper level\" with 2 lines"
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
    assert!(!renamed_onto.success);
    for blank in ["", "   "] {
        assert!(
            !state
                .execute(Command::UpdateCaveSection {
                    section_id: upper,
                    name: Some(blank.to_string()),
                    color: None,
                })
                .success
        );
    }
    assert!(
        !state
            .execute(Command::CreateCaveSection {
                name: "Fine".to_string(),
                color: "blue".to_string(),
                line_ids: Vec::new(),
            })
            .success,
        "a colour must be hex"
    );
    assert_eq!(names(&state), ["Upper level", "Lower level"]);
    assert_eq!(state.get_undo_stack().len(), 2, "a refusal records nothing");
}

#[test]
fn rename_and_recolour_undo_and_redo_with_labels() {
    let state = new_state();
    let id = create_section(&state, "Upper level", &[]);
    ok(state.execute(Command::UpdateCaveSection {
        section_id: id,
        name: Some("Upper".to_string()),
        color: None,
    }));
    assert_eq!(
        last_label(&state),
        "Rename section \"Upper level\" to \"Upper\""
    );
    ok(state.execute(Command::UpdateCaveSection {
        section_id: id,
        name: None,
        color: Some("#abcdef".to_string()),
    }));
    assert_eq!(last_label(&state), "Change colour of section \"Upper\"");
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
}

#[test]
fn delete_ungroups_and_undo_restores_the_section_in_place_with_its_lines() {
    let state = new_state();
    let (a, b, c) = three_lines(&state);
    let first = create_section(&state, "First", &[c]);
    let doomed = create_section(&state, "Doomed", &[a, b]);
    let last = create_section(&state, "Last", &[]);
    ok(state.execute(Command::DeleteCaveSection { section_id: doomed }));
    assert_eq!(last_label(&state), "Delete section \"Doomed\"");
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
    assert_eq!(
        last_label(&state),
        "Move 3 lines to section \"Lower level\""
    );
    assert!([a, b, c]
        .iter()
        .all(|l| section_of(&state, *l) == Some(lower)));
    ok(state.execute(Command::MoveLinesToCaveSection {
        line_ids: vec![b],
        section_id: None,
    }));
    assert_eq!(last_label(&state), "Move 1 line out of their section");
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
    for refused in [
        Command::MoveLinesToCaveSection {
            line_ids: vec![a, 999],
            section_id: Some(upper),
        },
        Command::MoveLinesToCaveSection {
            line_ids: vec![a],
            section_id: Some(77),
        },
    ] {
        assert!(!state.execute(refused).success);
    }
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
            .map(|l| project(state).get_line(*l).unwrap().color.clone())
            .collect()
    };
    let before = colours(&state);
    let id = create_section(&state, "Upper level", &[a, b]);
    ok(state.execute(Command::ApplyCaveSectionColor { section_id: id }));
    assert_eq!(
        last_label(&state),
        "Apply colour of section \"Upper level\" to its lines"
    );
    assert_eq!(colours(&state), ["#336699", "#336699", before[2].as_str()]);
    undo(&state);
    assert_eq!(colours(&state), before);
    redo(&state);
    assert_eq!(colours(&state)[0], "#336699");
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
    let options = SplitLineOptions {
        name_a: "Main-A".to_string(),
        name_b: "Main-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    };
    let split = ok(state.execute_split_line(main, SplitAt::Station { station_index: 1 }, options));
    let second = split.line_id.unwrap();
    assert_eq!(
        (section_of(&state, main), section_of(&state, second)),
        (Some(id), Some(id))
    );
    undo(&state);
    assert!(project(&state).get_line(second).is_none());
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
    let middle = project(&state).get_line(main).unwrap().shots[1].id;
    let result =
        ok(state.execute_delete_shot_and_split(main, middle, "A".to_string(), "B".to_string()));
    let tail = project(&state)
        .lines
        .iter()
        .find(|l| l.id != main)
        .map(|l| l.id)
        .unwrap();
    assert!(result.success);
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
    create_section(&state, "Other", &[o]);
    ok(state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 2),
        station_b: StationRef::new(o, 0),
    }));
    ok(state.execute_combine_lines(
        k,
        o,
        CombineLinesOptions {
            name: "Joined".to_string(),
            color: "#abcdef".to_string(),
            swap_lr: false,
            comment_pick: CombinePick::Kept,
            name_pick: CombinePick::Kept,
            join: None,
        },
    ));
    assert!(project(&state).get_line(o).is_none());
    assert_eq!(section_of(&state, k), Some(kept_section));
    undo(&state);
    assert_eq!(
        section_of(&state, o),
        project(&state).section_named("Other").map(|s| s.id)
    );
}

#[test]
fn deleting_a_line_drops_its_membership_and_undo_puts_it_back() {
    let state = new_state();
    let (a, b, _) = three_lines(&state);
    let id = create_section(&state, "Upper level", &[a, b]);
    ok(state.execute(Command::DeleteLine { line_id: a }));
    assert_eq!(project(&state).lines_in_section(id).count(), 1);
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
        state.create_line_in_cave_section(
            Command::CreateLine {
                name: Some("New".to_string()),
                surveyor: None,
                date: None,
                color: "#123456".to_string(),
                direction: None,
                shots: vec![shot(10.0, 4.0)],
            },
            section,
        )
    };
    assert!(!create(Some(99)).success, "a missing section is refused");
    assert_eq!(state.get_undo_stack().len(), 1);
    let line = ok(create(Some(id))).line_id.unwrap();
    assert_eq!(section_of(&state, line), Some(id));
    assert_eq!(state.get_undo_stack().len(), 2, "one step");
    assert_eq!(last_label(&state), "Create line 'New'");
    undo(&state);
    assert!(project(&state).get_line(line).is_none());
    redo(&state);
    assert_eq!(section_of(&state, line), Some(id));
    // Without a section it is a plain create.
    let plain = ok(create(None)).line_id.unwrap();
    assert_eq!(section_of(&state, plain), None);
}

fn import_config(index: u32, name: &str, section: Option<&str>) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: section.map(str::to_string),
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
        reverse: false,
        azimuth_reference: None,
    }
}

#[test]
fn an_import_groups_by_name_and_reuses_a_same_name_section() {
    let state = new_state();
    let own = create(&state, "Own", vec![shot(0.0, 3.0)]);
    let existing = create_section(&state, "Gallery", &[own]);
    let result = ok(state.execute_import_lines(
        vec![
            import_config(0, "Gallery", Some("Gallery")),
            import_config(1, "Gallery (branch 2)", Some("Gallery")),
            import_config(2, "Sump", Some("Sump")),
            import_config(3, "Sump (trip 2)", Some("Sump")),
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
    let sump = project(&state).section_named("Sump").unwrap().id;
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
    let added = project(&state).section_named("Gallery (2)").unwrap().id;
    assert_ne!(added, target_gallery);
    let added_lines: Vec<LineId> = project(&state)
        .lines
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
    assert!(added_lines
        .iter()
        .all(|l| section_of(&state, *l) == Some(added)));
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

    // The rows are the new vocabulary: no disk name of the Line concept.
    let rows: Vec<String> = state
        .with_open_database_for_tests(|db| {
            db.load_undo_stack()
                .unwrap()
                .into_iter()
                .map(|(_, command, _)| command)
                .collect()
        })
        .unwrap();
    assert!(rows
        .iter()
        .any(|r| r.contains("\"type\":\"move_lines_to_cave_section\"")
            && r.contains("\"line_ids\"")
            && r.contains("\"cave_section_id\"")));

    ok(state.execute(Command::CloseProject));
    ok(state.open_project(path));
    assert_eq!(names(&state), ["Upper level"]);
    assert!([a, b, c]
        .iter()
        .all(|l| section_of(&state, *l) == Some(upper)));
    let labels: Vec<String> = state
        .get_undo_stack()
        .iter()
        .take(4)
        .map(|item| item.description.clone())
        .collect();
    assert_eq!(
        labels,
        [
            "Delete section \"Lower level\"",
            "Create section \"Lower level\"",
            "Move 2 lines to section \"Upper level\"",
            "Create section \"Upper level\" with 1 line",
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

#[test]
fn every_section_command_round_trips_through_the_disk_vocabulary() {
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
        let disk = disk_names::to_disk_string(&command).unwrap();
        assert!(!disk.contains("\"section_id\""), "{disk}");
        let back: Command = disk_names::from_disk_str(&disk).unwrap();
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            serde_json::to_string(&command).unwrap()
        );
    }
    // A project snapshot (what a structural undo records) keeps the sections
    // under their own keys through the translation.
    let mut project = Project::new("P".to_string(), None);
    project.sections.push(Section {
        id: 0,
        name: "S".to_string(),
        color: "#123456".to_string(),
    });
    project.next_section_id = 1;
    let disk = disk_names::to_disk_string(&project).unwrap();
    let back: Project = disk_names::from_disk_str(&disk).unwrap();
    assert_eq!(back.sections, project.sections);
    assert_eq!(back.next_section_id, 1);
}
