//! [#576 A7] The undo classification is exhaustive, and unchanged.
//!
//! `is_undoable`, `generate_undo_data`, `finalize_undo_data`, the redo
//! dispatch and the history preview used to end in `_` arms, so a new command
//! (or undo-data variant) compiled straight into "no undo data" - silently
//! un-undoable, and still clearing redo. They now name every variant; the
//! compiler is the test for that. What a test CAN pin is that making them
//! exhaustive changed no classification: every variant below is classified
//! exactly as the negative list classified it.

use std::path::PathBuf;

use super::AppState;
use crate::core::{Command, CommentHolder};
use crate::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, SectionUpdates, ShotData, ShotKind, ShotUpdates,
    StationRef,
};
use crate::test_fixtures::state_with_project;

/// One instance of every `Command` variant, with the classification the base
/// (3e450812) negative list gave it. The `match` in `variant_name` is
/// exhaustive, so a new variant fails to compile here until it is added.
fn every_command() -> Vec<(Command, bool)> {
    let at = StationRef::new(1, 0);
    let to = StationRef::new(2, 0);
    let offset = Coordinates::new(1.0, 2.0, 0.0);
    vec![
        (
            Command::CreateProject {
                name: "CaveA".to_string(),
                location: None,
            },
            false,
        ),
        (
            Command::OpenProject {
                path: PathBuf::from("CaveA.stalx"),
            },
            false,
        ),
        (Command::SaveProject { path: None }, false),
        (Command::CloseProject, false),
        (
            Command::CreateSection {
                name: None,
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: Vec::new(),
            },
            true,
        ),
        (Command::DeleteSection { section_id: 1 }, true),
        (
            Command::UpdateSectionMetadata {
                section_id: 1,
                updates: SectionUpdates::default(),
            },
            true,
        ),
        (
            Command::ReverseSection {
                section_id: 1,
                swap_left_right: false,
            },
            true,
        ),
        (
            Command::CreateShot {
                section_id: 1,
                shot: ShotData::default(),
                after_shot_id: None,
                before_shot_id: None,
            },
            true,
        ),
        (
            Command::UpdateShot {
                section_id: 1,
                shot_id: 0,
                updates: ShotUpdates::default(),
            },
            true,
        ),
        (
            Command::DeleteShot {
                section_id: 1,
                shot_id: 0,
            },
            true,
        ),
        (
            Command::SetShotKind {
                section_id: 1,
                shot_id: 0,
                kind: ShotKind::default(),
            },
            true,
        ),
        (
            Command::MergeStations {
                station_a: at,
                station_b: to,
            },
            true,
        ),
        (Command::UnmergeStations { station_ref: at }, true),
        (
            Command::UnmergeStationPair {
                station_a: at,
                station_b: to,
            },
            true,
        ),
        (
            Command::CreateLoopClosure {
                station_a: at,
                station_b: to,
                closure_strategy: ClosureStrategy::ProportionalByLength,
            },
            true,
        ),
        (Command::RemoveLoopClosure { closure_id: 1 }, true),
        (
            Command::SetZeroDatum {
                station_ref: at,
                coordinates: GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
            },
            true,
        ),
        (Command::RemoveZeroDatum { station_ref: at }, true),
        (
            Command::SetStationComment {
                station_ref: at,
                comment: None,
            },
            true,
        ),
        (
            Command::MoveSection {
                section_id: 1,
                offset,
            },
            true,
        ),
        (
            Command::MoveSegment {
                section_id: 1,
                offset,
            },
            true,
        ),
        (
            Command::MoveStation {
                station_ref: at,
                offset,
            },
            true,
        ),
        (
            Command::MoveComment {
                source: CommentHolder::Station { station_ref: at },
                target: CommentHolder::Station { station_ref: to },
                swap: false,
            },
            true,
        ),
        (
            Command::UntrackImport {
                file_hashes: Vec::new(),
                section_hashes: Vec::new(),
            },
            true,
        ),
        (
            Command::Batch {
                commands: Vec::new(),
            },
            true,
        ),
        (
            Command::MergeProjects {
                source_paths: Vec::new(),
                output_path: PathBuf::from("CaveB.stalx"),
                output_name: "CaveB".to_string(),
                archive_originals: false,
            },
            false,
        ),
        (Command::RepairIntegrity, false),
        (Command::UpdateProjectSettings, true),
        (Command::SetSurfaceElevation, true),
    ]
}

/// Exhaustive on purpose: a new `Command` variant stops this compiling, so it
/// cannot be left out of the table above unnoticed.
fn variant_name(command: &Command) -> &'static str {
    match command {
        Command::CreateProject { .. } => "CreateProject",
        Command::OpenProject { .. } => "OpenProject",
        Command::SaveProject { .. } => "SaveProject",
        Command::CloseProject => "CloseProject",
        Command::CreateSection { .. } => "CreateSection",
        Command::DeleteSection { .. } => "DeleteSection",
        Command::UpdateSectionMetadata { .. } => "UpdateSectionMetadata",
        Command::ReverseSection { .. } => "ReverseSection",
        Command::CreateShot { .. } => "CreateShot",
        Command::UpdateShot { .. } => "UpdateShot",
        Command::DeleteShot { .. } => "DeleteShot",
        Command::SetShotKind { .. } => "SetShotKind",
        Command::MergeStations { .. } => "MergeStations",
        Command::UnmergeStations { .. } => "UnmergeStations",
        Command::UnmergeStationPair { .. } => "UnmergeStationPair",
        Command::CreateLoopClosure { .. } => "CreateLoopClosure",
        Command::RemoveLoopClosure { .. } => "RemoveLoopClosure",
        Command::SetZeroDatum { .. } => "SetZeroDatum",
        Command::RemoveZeroDatum { .. } => "RemoveZeroDatum",
        Command::SetStationComment { .. } => "SetStationComment",
        Command::MoveSection { .. } => "MoveSection",
        Command::MoveSegment { .. } => "MoveSegment",
        Command::MoveStation { .. } => "MoveStation",
        Command::MoveComment { .. } => "MoveComment",
        Command::UntrackImport { .. } => "UntrackImport",
        Command::Batch { .. } => "Batch",
        Command::MergeProjects { .. } => "MergeProjects",
        Command::RepairIntegrity => "RepairIntegrity",
        Command::UpdateProjectSettings => "UpdateProjectSettings",
        Command::SetSurfaceElevation => "SetSurfaceElevation",
    }
}

#[test]
fn every_command_keeps_its_undo_classification() {
    let table = every_command();
    let mut names: Vec<&str> = table.iter().map(|(c, _)| variant_name(c)).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 30, "one row per Command variant: {names:?}");
    for (command, undoable) in &table {
        assert_eq!(
            AppState::is_undoable(command),
            *undoable,
            "{} changed classification",
            variant_name(command)
        );
    }
}

/// With no project open every arm of `generate_undo_data` is unreachable (it
/// returns `None` before matching); with one open, the variants that record
/// nothing before execution still record nothing. Pins that the arms that
/// replaced `_ => UndoData::None` kept it.
#[test]
fn commands_that_record_nothing_up_front_still_record_nothing() {
    let state = state_with_project("CaveA");
    for (command, _) in every_command() {
        let none_up_front = matches!(
            command,
            Command::CreateProject { .. }
                | Command::OpenProject { .. }
                | Command::SaveProject { .. }
                | Command::CloseProject
                | Command::MergeProjects { .. }
                | Command::RepairIntegrity
                | Command::UpdateProjectSettings
                | Command::SetSurfaceElevation
                | Command::UntrackImport { .. }
                | Command::CreateSection { .. }
                | Command::MoveComment { .. }
                | Command::Batch { .. }
        );
        if none_up_front {
            assert!(
                matches!(
                    state.generate_undo_data(&command),
                    crate::core::UndoData::None
                ),
                "{} now records undo data before execution",
                variant_name(&command)
            );
        }
    }
}
