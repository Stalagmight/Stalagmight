//! [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] Through the REGISTERED commands: a save or an
//! edit whose `expected_project` does not name the open project is refused with
//! `PROJECT_CHANGED` and changes nothing; with the open project's identity it lands.
//! The wire path the frontend uses (`expectedProject` on every save and survey
//! mutation), from the command's argument to the check under `command_lock`. All
//! data is synthetic.

use tempfile::TempDir;

use super::command_tests::{app_with_state, h, made_for, run, shot_data, state};
use super::{history, project, section};
use crate::core::state::ProjectIdentity;
use crate::core::{Command, CommandError, CommandErrorCode};

fn code<T>(answer: &Result<T, CommandError>) -> Option<CommandErrorCode> {
    answer.as_ref().err().map(|e| e.code)
}

/// The open project's identity with another `created_at`: another project.
fn another_project(app: &tauri::App<tauri::test::MockRuntime>) -> Option<ProjectIdentity> {
    made_for(app).map(|open| ProjectIdentity {
        created_at: "2020-01-01T00:00:00Z".to_owned(),
        ..open
    })
}

fn add_section(
    app: &tauri::App<tauri::test::MockRuntime>,
    made_for: Option<ProjectIdentity>,
    name: &str,
) -> Result<crate::core::SectionCreated, CommandError> {
    run(section::create_section(
        h(app),
        made_for,
        Some(name.to_owned()),
        None,
        None,
        "#336699".to_owned(),
        None,
        vec![shot_data(3.0), shot_data(4.0)],
    ))
}

fn section_names(app: &tauri::App<tauri::test::MockRuntime>) -> Vec<String> {
    state(app)
        .get_project_state()
        .map(|p| p.sections.iter().filter_map(|s| s.name.clone()).collect())
        .unwrap_or_default()
}

fn app_with_project() -> tauri::App<tauri::test::MockRuntime> {
    let app = app_with_state();
    assert!(
        state(&app)
            .execute(Command::CreateProject {
                name: "CaveA".to_owned(),
                location: None,
            })
            .success
    );
    app
}

#[test]
fn an_edit_for_another_project_or_none_is_refused_through_the_command() {
    let app = app_with_project();
    let other = add_section(&app, another_project(&app), "ForAnother");
    let none = add_section(&app, None, "ForNone");
    let own = add_section(&app, made_for(&app), "ForOpen");
    println!(
        "create_section: another project -> {:?}, none -> {:?}, the open one -> {:?}; sections {:?}",
        code(&other),
        code(&none),
        code(&own),
        section_names(&app)
    );
    assert_eq!(code(&other), Some(CommandErrorCode::ProjectChanged));
    assert_eq!(code(&none), Some(CommandErrorCode::ProjectChanged));
    assert!(own.is_ok(), "{:?}", own.err());
    assert_eq!(section_names(&app), ["ForOpen"]);
}

#[test]
fn an_undo_for_another_project_is_refused_through_the_command() {
    let app = app_with_project();
    add_section(&app, made_for(&app), "ToUndo").expect("the section lands");
    let undo = run(history::undo(h(&app), another_project(&app)));
    println!(
        "undo for another project -> {:?}; sections {:?}",
        code(&undo),
        section_names(&app)
    );
    assert_eq!(code(&undo), Some(CommandErrorCode::ProjectChanged));
    assert_eq!(section_names(&app), ["ToUndo"], "nothing was undone");
    assert!(run(history::undo(h(&app), made_for(&app))).is_ok());
    assert_eq!(section_names(&app), Vec::<String>::new(), "the undo ran");
}

#[test]
fn a_save_as_for_another_project_writes_nothing_through_the_command() {
    let app = app_with_project();
    let dir = TempDir::new().expect("a temp dir");
    let target = dir.path().join("Chosen.stalx");
    let refused = run(project::save_project(
        h(&app),
        another_project(&app),
        Some(target.clone()),
        None,
    ));
    println!(
        "Save As for another project -> {:?}; target exists: {}",
        code(&refused),
        target.exists()
    );
    assert_eq!(code(&refused), Some(CommandErrorCode::ProjectChanged));
    assert!(!target.exists(), "nothing written at the chosen path");
    let saved = run(project::save_project(
        h(&app),
        made_for(&app),
        Some(target.clone()),
        None,
    ));
    assert!(saved.is_ok(), "{:?}", saved.err());
    assert!(target.exists());
}
