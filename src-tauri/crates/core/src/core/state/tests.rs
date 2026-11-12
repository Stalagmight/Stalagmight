use super::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::core::undo::{UndoData, UndoEntry};
use crate::core::{Command, CommandResult, PersistStack, UndoStack};
use crate::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, ImportPlacement, ImportSectionConfig,
    LoopClosure, Project, SectionData, SectionId, SectionUpdates, ShotData, ShotId, ShotKind,
    ShotUpdates, StationAnchor, StationKey, StationMerge, StationRef, SurveyDirection,
    MIN_SPLIT_CHILD_LENGTH_M,
};
use crate::persistence::Database;
use tempfile::TempDir;

/// Helper to create a basic shot data for testing
fn test_shot_data(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

fn assert_coords_close(a: Coordinates, b: Coordinates) {
    let eps = 1e-6;
    assert!((a.x - b.x).abs() < eps, "x mismatch: {} vs {}", a.x, b.x);
    assert!((a.y - b.y).abs() < eps, "y mismatch: {} vs {}", a.y, b.y);
    assert!((a.z - b.z).abs() < eps, "z mismatch: {} vs {}", a.z, b.z);
}

/// Like [`assert_coords_close`] but only the horizontal (map) position. Section
/// offsets are 2D, so gap-close deletes preserve x/y but not section-relative z.
fn assert_xy_close(a: Coordinates, b: Coordinates) {
    let eps = 1e-6;
    assert!((a.x - b.x).abs() < eps, "x mismatch: {} vs {}", a.x, b.x);
    assert!((a.y - b.y).abs() < eps, "y mismatch: {} vs {}", a.y, b.y);
}

/// Absolute (section-offset-applied) coordinates of every station, keyed by ref.
/// Used to assert that an edit shifts only the affected geometry and leaves the
/// rest of the map put (#41 regression sentinels).
fn abs_coord_map(project: &Project) -> HashMap<StationRef, Coordinates> {
    project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, s.coordinates))
        .collect()
}

fn poison_mutex<T>(mutex: &Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock().unwrap();
        panic!("poison mutex for lock failure coverage");
    }));
}

// === Project Operations Tests ===

#[test]
fn creating_a_project_clears_a_previous_projects_discarded_history_flag() {
    let state = AppState::new().unwrap();
    state.set_history_discarded(true);

    let result = state.execute(Command::CreateProject {
        name: "Fresh Cave".to_string(),
        location: None,
    });

    assert!(result.success);
    assert!(
        !state.history_discarded(),
        "a project created from nothing has no persisted history, so it cannot have lost any"
    );
}

/// ...and closing one does too, so the banner cannot outlive the project it describes.
#[test]
fn closing_a_project_clears_the_discarded_history_flag() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Doomed".to_string(),
        location: None,
    });
    state.set_history_discarded(true);

    state.execute(Command::CloseProject);

    assert!(!state.history_discarded());
}

#[test]
fn test_create_project() {
    let state = AppState::new().unwrap();

    let result = state.execute(Command::CreateProject {
        name: "Test Cave".to_string(),
        location: Some("Test Location".to_string()),
    });

    assert!(result.success);
    assert!(result.error.is_none());

    let project = state.get_project_state();
    assert!(project.is_some());

    let project = project.unwrap();
    assert_eq!(project.name, "Test Cave");
    assert_eq!(project.location, Some("Test Location".to_string()));
}

#[test]
fn test_merge_projects_archives_sources_and_adds_extension() {
    let temp_dir = TempDir::new().unwrap();

    let mut project_a = Project::new("A".to_string(), None);
    project_a.add_section(SectionData {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots: vec![test_shot_data(1.0)],
    });

    let mut project_b = Project::new("B".to_string(), None);
    project_b.add_section(SectionData {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 1.0,
        shots: vec![test_shot_data(2.0)],
    });

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");

    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&project_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&project_b).unwrap();
    drop(db_a);
    drop(db_b);

    let existing_archive = temp_dir.path().join("a.stalx.archived");
    std::fs::write(&existing_archive, b"archived").unwrap();

    let state = AppState::new().unwrap();
    let output_path = temp_dir.path().join("merged-output");

    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a.clone(), source_b.clone()],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: true,
    });

    assert!(result.success);

    let output_with_ext = output_path.with_extension("stalx");
    assert!(output_with_ext.exists());

    assert!(!source_a.exists());
    assert!(!source_b.exists());

    let archived_entries: Vec<_> = std::fs::read_dir(temp_dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("a.stalx.archived"))
        })
        .collect();

    assert!(!archived_entries.is_empty());
    assert!(temp_dir.path().join("b.stalx.archived").exists());
}

/// Save two synthetic projects (with the given `surface_elevation`s) and merge
/// them, returning the merged project and the merge's `CommandResult`.
fn merge_two_surface_elevations(
    elev_a: Option<f64>,
    elev_b: Option<f64>,
) -> (Project, CommandResult) {
    let temp_dir = TempDir::new().unwrap();

    let mut project_a = Project::new("A".to_string(), None);
    project_a.surface_elevation = elev_a;
    project_a.add_section(SectionData {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots: vec![test_shot_data(1.0)],
    });

    let mut project_b = Project::new("B".to_string(), None);
    project_b.surface_elevation = elev_b;
    project_b.add_section(SectionData {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 1.0,
        shots: vec![test_shot_data(2.0)],
    });

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");

    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&project_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&project_b).unwrap();
    drop(db_a);
    drop(db_b);

    let state = AppState::new().unwrap();
    let output_path = temp_dir.path().join("merged-output.stalx");

    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success);

    let db = Database::open_existing(&output_path).unwrap();
    let merged = db.load_project().unwrap();
    (merged, result)
}

/// #438: a merged section keeps the north reference it was RESOLVED to in its
/// own project, whether it declared one itself or inherited its project's
/// default.
///
/// `SectionData` cannot carry the reference, so the copy in `project_merge`
/// used to land on the merged project's default - which for a section that had
/// a 10 degree declination means the passage is drawn, and exported, rotated by
/// 10 degrees the moment it is merged. Both sources here are deliberately
/// unlike the target: one declares per-section, the other relies on a project
/// default that the merged project does not share.
///
/// Asserted end-to-end through the coordinate WALK, not just the stored field:
/// the field being right is only interesting because the geometry follows it.
#[test]
fn test_merge_projects_carries_each_sections_resolved_azimuth_reference() {
    let temp_dir = TempDir::new().unwrap();

    // Due north, level, so the walked bearing IS the applied correction.
    // `test_shot_data`'s 45/225 pair is deliberately antipodal (its circular
    // mean resolves to 315) and would make the geometry assertion unreadable.
    let due_north = |length: f64| ShotData {
        azimuth_in: 0.0,
        azimuth_out: 0.0,
        length,
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };

    // A: declares the reference on the section itself.
    let mut project_a = Project::new("A".to_string(), None);
    let section_a = project_a
        .add_section(SectionData {
            name: Some("Declared".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: Some(SurveyDirection::In),
            quality: 1.0,
            shots: vec![due_north(10.0)],
        })
        .id;
    project_a
        .get_section_mut(section_a)
        .unwrap()
        .azimuth_reference = Some(crate::models::AzimuthReference::magnetic(
        10.0,
        crate::models::CorrectionSource::Manual,
    ));

    // B: says nothing per section and leans on its PROJECT default.
    let mut project_b = Project::new("B".to_string(), None);
    project_b.default_azimuth_reference = Some(crate::models::AzimuthReference::magnetic(
        -4.0,
        crate::models::CorrectionSource::FileDeclared,
    ));
    project_b.add_section(SectionData {
        name: Some("Inherited".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 1.0,
        shots: vec![due_north(10.0)],
    });

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&project_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&project_b).unwrap();
    drop(db_a);
    drop(db_b);

    let state = AppState::new().unwrap();
    let output_path = temp_dir.path().join("merged-output.stalx");
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "merge failed: {:?}", result.error);

    let db = Database::open_existing(&output_path).unwrap();
    let merged = db.load_project().unwrap();
    assert_eq!(merged.sections.len(), 2, "both sections must survive");

    let declared = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Declared"))
        .expect("section A survives the merge");
    let inherited = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Inherited"))
        .expect("section B survives the merge");

    assert!(
        (merged.azimuth_reference_for(declared).applied_correction() - 10.0).abs() < 1e-9,
        "a section that declared its own reference lost it: {:?}",
        declared.azimuth_reference
    );
    assert!(
        (merged.azimuth_reference_for(inherited).applied_correction() - (-4.0)).abs() < 1e-9,
        "a section that inherited its source project's default lost it: {:?}",
        inherited.azimuth_reference
    );

    // And the geometry follows: station 1 of each section sits on the CORRECTED
    // bearing. The shots walk due north as measured, so the corrected heading is
    // the correction itself.
    for (section, correction) in [(declared, 10.0_f64), (inherited, -4.0_f64)] {
        let coords = merged
            .compute_station_raw_coords(StationRef::new(section.id, 1))
            .expect("station coords");
        let origin = merged
            .compute_station_raw_coords(StationRef::new(section.id, 0))
            .expect("station coords");
        let bearing = (coords.x - origin.x)
            .atan2(coords.y - origin.y)
            .to_degrees();
        assert!(
            (bearing - correction).abs() < 1e-6,
            "section {:?} walks at {bearing} degrees, expected {correction}",
            section.name
        );
    }
}

#[test]
fn test_merge_projects_surface_elevation_some_and_none() {
    let (merged, result) = merge_two_surface_elevations(Some(123.5), None);
    assert_eq!(merged.surface_elevation, Some(123.5));
    assert!(result.warnings.is_empty());
}

#[test]
fn test_merge_projects_surface_elevation_none_and_some() {
    let (merged, result) = merge_two_surface_elevations(None, Some(-7.25));
    assert_eq!(merged.surface_elevation, Some(-7.25));
    assert!(result.warnings.is_empty());
}

#[test]
fn test_merge_projects_surface_elevation_agreeing() {
    let (merged, result) = merge_two_surface_elevations(Some(100.0), Some(100.002));
    assert_eq!(merged.surface_elevation, Some(100.0));
    assert!(result.warnings.is_empty());
}

#[test]
fn test_merge_projects_surface_elevation_disagreeing_first_wins_with_warning() {
    let (merged, result) = merge_two_surface_elevations(Some(100.0), Some(150.0));
    assert_eq!(merged.surface_elevation, Some(100.0));
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].code, "SURFACE_ELEVATION_MISMATCH");
    assert!(result.warnings[0].message.contains("100.00"));
    assert!(result.warnings[0].message.contains("150.00"));
}

/// The tolerance is strictly-greater-than, not greater-or-equal: a difference
/// of EXACTLY 0.005 m must read as agreement, matching the `{:.2}` rounding
/// the warning message itself uses (both would print "100.00 m" against
/// "100.00 m", so a warning at the boundary would name no visible disagreement).
#[test]
fn test_merge_projects_surface_elevation_tolerance_boundary_is_exclusive() {
    let (merged, result) = merge_two_surface_elevations(Some(100.0), Some(100.005));
    assert_eq!(merged.surface_elevation, Some(100.0));
    assert!(
        result.warnings.is_empty(),
        "a 0.005 m difference is AT the tolerance, not beyond it, and must not warn: {:?}",
        result.warnings
    );
}

/// A non-finite incoming elevation (never produced by a real Ariane import,
/// but not provably unreachable upstream) must not be adopted as the merged
/// value, and must not silently swallow the no-clobber warning either - it is
/// simply skipped, as if that source had no elevation at all.
#[test]
fn test_merge_projects_surface_elevation_non_finite_is_skipped() {
    let (merged, result) = merge_two_surface_elevations(Some(f64::NAN), Some(100.0));
    assert_eq!(
        merged.surface_elevation,
        Some(100.0),
        "a NaN first source must not poison the merged elevation"
    );
    assert!(result.warnings.is_empty());

    let (merged, result) = merge_two_surface_elevations(Some(100.0), Some(f64::NAN));
    assert_eq!(
        merged.surface_elevation,
        Some(100.0),
        "a NaN later source must not be adopted or compared"
    );
    assert!(
        result.warnings.is_empty(),
        "a skipped NaN source must not produce a mismatch warning: {:?}",
        result.warnings
    );
}

#[test]
fn test_start_new_load_cancels_previous_and_clear_is_identity_safe() {
    let state = AppState::new().unwrap();

    let first = state.start_new_load();
    assert!(!first.load(Ordering::SeqCst));

    let second = state.start_new_load();
    assert!(first.load(Ordering::SeqCst));
    assert!(!second.load(Ordering::SeqCst));

    state.clear_active_load(&first);
    let guard = state.active_loading.lock().unwrap();
    assert!(guard.as_ref().is_some());
    assert!(Arc::ptr_eq(guard.as_ref().unwrap(), &second));
    drop(guard);

    state.clear_active_load(&second);
    assert!(state.active_loading.lock().unwrap().is_none());
}

#[test]
fn test_close_project_succeeds_with_poisoned_database_lock() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    poison_mutex(&state.database);

    let result = state.close_project();
    assert!(result.success);
    assert!(state.get_project_state().is_none());
}

#[test]
fn test_open_project_restores_undo_redo_and_assigns_closure_ids() {
    let temp_dir = TempDir::new().unwrap();
    let project_path = temp_dir.path().join("restore.stalx");

    let mut project = Project::new("Restore".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![test_shot_data(2.0)],
            ..Default::default()
        })
        .id;

    project.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(section_id, 0).into(),
        station_b: StationRef::new(section_id, 1).into(),
        path_length: 2.0,
        misclosure_distance: 0.1,
        error_percentage: 5.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    let mut db = Database::open(&project_path).unwrap();
    db.save_project(&project).unwrap();

    // The UNDO entry needs real undo data: `load_from_persisted_with_timestamps`
    // drops `UndoData::None` from the undo stack, so `None` here would test that
    // filter instead of the restore. The REDO entry deliberately keeps `None` -
    // the redo stack is not filtered (a `None` redo entry re-executes its command
    // correctly), and `redo_len() == 1` below is the pin for that.
    let undo_entry = UndoEntry {
        command_bytes: None,
        command: Command::CreateProject {
            name: "Undo Project".to_string(),
            location: None,
        },
        undo_data: UndoData::InverseCommand(Command::CloseProject),
        timestamp: None,
    };
    let redo_entry = UndoEntry {
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::None,
        timestamp: None,
    };

    let undo_persist = UndoStack::serialize_entry_with_timestamp(&undo_entry).unwrap();
    let redo_persist = UndoStack::serialize_entry_with_timestamp(&redo_entry).unwrap();
    db.save_undo_stack_with_timestamps(vec![undo_persist])
        .unwrap();
    db.save_redo_stack_with_timestamps(vec![redo_persist])
        .unwrap();
    drop(db);

    let state = AppState::new().unwrap();
    let result = state.open_project(project_path);
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.loop_closures.len(), 1);
    assert_ne!(project.loop_closures[0].id, 0);

    let undo_stack = state.undo_stack.lock().unwrap();
    assert_eq!(undo_stack.len(), 1);
    assert_eq!(undo_stack.redo_len(), 1);

    let undo_entry = undo_stack.undo_entries().next().unwrap();
    match &undo_entry.command {
        Command::CreateProject { name, location } => {
            assert_eq!(name, "Undo Project");
            assert!(location.is_none());
        }
        other => panic!("Unexpected undo entry: {other:?}"),
    }

    let redo_entry = undo_stack.redo_entries().next().unwrap();
    match &redo_entry.command {
        Command::CloseProject => {}
        other => panic!("Unexpected redo entry: {other:?}"),
    }
}

/// [#303] A Full-mode linked project opens with ALL of its persisted history, however
/// far past the entry-count window it reaches.
///
/// The bound has to be armed BEFORE the restore, and that ordering is the whole test:
/// `load_from_persisted_with_timestamps` ends in `enforce_depth_limit`, so whatever bound
/// is in force at that instant is the one that trims. Arming afterwards meant the restore
/// ran under the startup default `Count(undo_history_limit)` and evicted everything older
/// than the window on the way in - which Count -> Bytes cannot undo. The next
/// full-rewrite save then wrote the truncation to disk and the next push shipped a commit
/// missing the history it was meant to carry.
#[test]
fn opening_a_full_mode_project_restores_history_deeper_than_the_count_window() {
    // `open_project` writes the recent-projects list into the global config, and the
    // entry-count window this test reasons about is read from it too.
    crate::test_helpers::with_isolated_config(|| {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("deep_history.stalx");

        // Half again over the 100-entry default window, so the shortfall is unambiguous.
        const PERSISTED: usize = 150;
        let now = chrono::Utc::now();
        let entries: Vec<_> = (0..PERSISTED)
            .map(|i| {
                let entry = UndoEntry {
                    command_bytes: None,
                    command: Command::CreateProject {
                        name: format!("Entry {i}"),
                        location: None,
                    },
                    // Real undo data: the loader drops `UndoData::None` from the undo
                    // stack, which would test that filter instead of the depth bound.
                    undo_data: UndoData::InverseCommand(Command::CloseProject),
                    timestamp: Some(now + chrono::Duration::seconds(i as i64)),
                };
                UndoStack::serialize_entry_with_timestamp(&entry).unwrap()
            })
            .collect();

        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Deep".to_string(), None))
            .unwrap();
        // Full mode SET ON THE LINK, not inherited from the global default: this test is
        // about the arming order, so the policy must not depend on a config value another
        // test could reasonably change.
        db.save_speleo_link(Some(&SpeleoLink {
            undo_mode: Some(crate::models::UndoMode::Full),
            ..fixture_speleo_link()
        }))
        .unwrap();
        db.save_undo_stack_with_timestamps(entries).unwrap();
        drop(db);

        let state = AppState::new().unwrap();
        assert!(state.open_project(path).success);

        let stack = state.undo_stack.lock().unwrap();
        assert_eq!(
            stack.len(),
            PERSISTED,
            "a Full-mode project is on the byte budget, so every persisted entry must \
             survive the open - a short stack means the count window was still armed \
             when the entries were restored"
        );
        // The oldest is the one a count trim evicts first, so name it explicitly.
        assert!(
            matches!(
                &stack.undo_entries().last().unwrap().command,
                Command::CreateProject { name, .. } if name == "Entry 0"
            ),
            "the OLDEST entry is what a front eviction takes, so it is the one worth pinning"
        );
    });
}

/// [#469] A count-window project opens the NEWEST window of a bloated undo table and
/// reads nothing beyond it - and what it reads is a WORKING history.
///
/// Nothing in the schema bounds `undo_stack`, and a `.stalx` is a file the app opens
/// rather than one it can trust: the loader used to `SELECT` and gunzip every row, and
/// only the restore that followed applied the entry window. A long-lived (or crafted)
/// file could therefore exhaust memory during open with the correct bound already armed
/// - it was applied one step too late to matter. The bound has to be in the QUERY, which
/// is what the row counts below pin.
///
/// The undo at the end is the other half: dropping the oldest rows must not corrupt the
/// sequence bookkeeping. It cannot, because the loaded sequences are discarded and every
/// save renumbers 0..n - which the final assertion checks rather than assumes.
#[test]
fn opening_a_count_window_project_reads_only_the_newest_window_of_a_bloated_table() {
    // `open_project`/`save_project` record the path in the global config, and the entry
    // window this test reasons about is read from it.
    crate::test_helpers::with_isolated_config(|| {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("bloated_history.stalx");

        let mut project = Project::new("Bloated".to_string(), None);
        project.add_section(SectionData {
            shots: vec![ShotData {
                comment: Some("edit 399".to_string()),
                ..test_shot_data(2.0)
            }],
            ..Default::default()
        });

        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // Ids as the loader assigns them, so the persisted entries address the shot the
        // reopened project actually has.
        let saved = db.load_project().unwrap();
        let section_id = saved.sections[0].id;
        let shot_id = saved.sections[0].shots[0].id;

        // Four times the 100-entry default window, so a load that read the whole table
        // and trimmed afterwards is distinguishable from one that never read it.
        const PERSISTED: usize = 400;
        let now = chrono::Utc::now();
        let rows: Vec<_> = (0..PERSISTED)
            .map(|i| {
                let entry = UndoEntry {
                    command_bytes: None,
                    command: Command::UpdateShot {
                        section_id,
                        shot_id,
                        updates: ShotUpdates {
                            comment: Some(Some(format!("edit {i}"))),
                            ..Default::default()
                        },
                    },
                    undo_data: UndoData::PreviousValues {
                        section_id,
                        shot_id: Some(shot_id),
                        shot_uuid: None,
                        values_json: serde_json::to_string(&ShotUpdates {
                            comment: Some(Some(format!("comment {i}"))),
                            ..Default::default()
                        })
                        .unwrap(),
                        previous_offsets: HashMap::new(),
                    },
                    timestamp: Some(now + chrono::Duration::seconds(i as i64)),
                };
                UndoStack::serialize_entry_with_timestamp(&entry).unwrap()
            })
            .collect();
        db.save_undo_stack_with_timestamps(rows.clone()).unwrap();
        db.save_redo_stack_with_timestamps(rows).unwrap();
        drop(db);

        // The row that makes this test decisive rather than vacuous: the OLDEST entry in
        // each table is given a blob that claims to be gzip and is not, so any read that
        // reaches it fails - and a failed history load restores NOTHING. Without a
        // windowed query the assertions below therefore see an empty stack, whatever the
        // in-memory trim would have done afterwards.
        let conn = rusqlite::Connection::open(&path).unwrap();
        for table in ["undo_stack", "redo_stack"] {
            conn.execute(
                &format!("UPDATE {table} SET command_json = X'1F8B0000FFFF' WHERE sequence = 0"),
                [],
            )
            .unwrap();
        }
        drop(conn);
        assert!(
            Database::open_existing(&path)
                .unwrap()
                .load_undo_stack_with_timestamps()
                .is_err(),
            "the poison row must really poison a read that reaches it, or the window \
             assertions below prove nothing"
        );

        let state = AppState::new().unwrap();
        let window = state.undo_stack.lock().unwrap().history_limit();
        assert!(
            window < PERSISTED,
            "the fixture must overflow the window it is testing: {window} vs {PERSISTED}"
        );
        assert!(state.open_project(path.clone()).success);

        {
            let stack = state.undo_stack.lock().unwrap();
            assert_eq!(
                stack.len(),
                window,
                "a count-window project keeps its window and no more"
            );
            assert_eq!(
                stack.redo_len(),
                window,
                "the redo table is bounded the same way - a `.stalx` persists both"
            );
            assert!(
                matches!(
                    &stack.undo_entries().next().unwrap().command,
                    Command::UpdateShot { updates, .. }
                        if updates.comment == Some(Some("edit 399".to_string()))
                ),
                "the window has to be the NEWEST rows: the newest entry is the next undo"
            );
            assert!(
                matches!(
                    &stack.undo_entries().last().unwrap().command,
                    Command::UpdateShot { updates, .. }
                        if updates.comment == Some(Some("edit 300".to_string()))
                ),
                "...and it must be contiguous down to exactly one window's depth"
            );
        }

        // A working history, not merely a short one.
        assert!(state.undo().success);
        assert_eq!(
            state.get_project_state().unwrap().sections[0].shots[0]
                .comment
                .as_deref(),
            Some("comment 399"),
            "the newest restored entry must still undo to the state it recorded"
        );

        // And the rows dropped with the oldest history leave no hole behind: the save
        // renumbers the table from 0, exactly as it does for a history that was never
        // truncated.
        assert!(state.save_project(Some(path.clone())).success);
        let persisted = Database::open_existing(&path)
            .unwrap()
            .load_undo_stack_with_timestamps()
            .unwrap();
        assert!(!persisted.is_empty());
        assert_eq!(
            persisted.iter().map(|(seq, ..)| *seq).collect::<Vec<_>>(),
            (0..persisted.len() as i64).collect::<Vec<_>>(),
            "sequences are contiguous from 0 after a truncated load, so nothing that \
             reads them can trip over the dropped rows"
        );
    });
}

/// Shot with every nullable field populated - the fields a replayed legacy
/// (pre-v7) undo entry would silently clear under the v7 `double_option`
/// null-means-clear semantics.
fn shot_with_nullable_fields() -> ShotData {
    ShotData {
        pitch_in: Some(5.0),
        pitch_out: Some(-5.0),
        left: Some(1.5),
        right: Some(2.5),
        up: Some(0.5),
        down: Some(3.5),
        comment: Some("original".to_string()),
        marker_index: Some(2),
        temperature: Some(14.5),
        ..test_shot_data(2.0)
    }
}

#[test]
fn test_open_project_drops_legacy_null_style_undo_history() {
    // open_project records the path in recent projects (stored in the global
    // config), so isolate the config from the real file.
    let _cfg = crate::test_helpers::isolated_config();
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("legacy_undo.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot_with_nullable_fields()],
        ..Default::default()
    });

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    let saved = db.load_project().unwrap();
    let section_id = saved.sections[0].id;
    let shot_id = saved.sections[0].shots[0].id;

    // Persist undo/redo entries exactly as a pre-v7 build wrote them: every
    // untouched nullable field serialized as an explicit null, in both the
    // command payload and the PreviousValues snapshot.
    let legacy_command = format!(
        r#"{{"type":"update_shot","section_id":{section_id},"shot_id":{shot_id},"updates":{{"azimuth_in":null,"azimuth_out":null,"length":4.0,"depth_in":null,"depth_out":null,"pitch_in":null,"pitch_out":null,"left":null,"right":null,"up":null,"down":null,"comment":null,"is_reference":null,"marker_index":null,"temperature":null}}}}"#
    );
    let legacy_values = r#"{"azimuth_in":null,"azimuth_out":null,"length":2.0,"depth_in":null,"depth_out":null,"pitch_in":null,"pitch_out":null,"left":null,"right":null,"up":null,"down":null,"comment":null,"is_reference":null,"marker_index":null,"temperature":null}"#;
    let legacy_undo_data = format!(
        r#"{{"PreviousValues":{{"section_id":{section_id},"shot_id":{shot_id},"values_json":{}}}}}"#,
        serde_json::to_string(legacy_values).unwrap()
    );
    db.save_undo_stack_with_timestamps(vec![(
        legacy_command.clone(),
        legacy_undo_data.clone(),
        None,
    )])
    .unwrap();
    db.save_redo_stack_with_timestamps(vec![(legacy_command, legacy_undo_data, None)])
        .unwrap();
    drop(db);

    // Rewind the version stamp so the file looks exactly like one written by
    // a pre-v7 build (Database::open stamped the current schema version).
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("PRAGMA user_version = 6", []).unwrap();
    drop(conn);

    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success);

    // The legacy stacks are dropped wholesale - nothing to undo or redo, so
    // nothing can replay the explicit nulls and clear fields.
    {
        let undo_stack = state.undo_stack.lock().unwrap();
        assert_eq!(undo_stack.len(), 0, "legacy undo entries must be dropped");
        assert_eq!(
            undo_stack.redo_len(),
            0,
            "legacy redo entries must be dropped"
        );
    }

    // The shot's nullable fields are intact.
    let project = state.get_project_state().unwrap();
    let shot = &project.sections[0].shots[0];
    assert_eq!(shot.pitch_in, Some(5.0));
    assert_eq!(shot.left, Some(1.5));
    assert_eq!(shot.comment.as_deref(), Some("original"));
    assert_eq!(shot.marker_index, Some(2));
    assert_eq!(shot.temperature, Some(14.5));
}

#[test]
fn a_second_save_with_no_new_edit_still_persists_the_undo_history() {
    // [#303] THE regression guard for the bug the first cut of the hybrid stack
    // shipped: consecutive saves with nothing changed in between.
    //
    // `save_project` writes a FRESH `.stalx.tmp` and renames it over the target, then
    // writes the undo tables into the reopened file. A persist that "optimized" by
    // skipping rows it believed were already on disk therefore wrote NOTHING into a file
    // that had NOTHING - and the history vanished. Live, that is what a SpeleoDB push
    // hits: its own flush is a save, and it very often follows another one (an autosave,
    // or the save the frontend runs after a reload), so the commit it produced carried
    // no undo history at all.
    //
    // Two saves, one edit. The second must leave the history on disk.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("twice.stalx");

    let state = AppState::new().unwrap();
    assert!(state.create_project("Twice".to_string(), None).success);
    assert!(
        state
            .execute(Command::CreateSection {
                name: Some("only-edit".to_string()),
                surveyor: None,
                date: None,
                color: "#abcdef".to_string(),
                direction: None,
                shots: vec![ShotData {
                    length: 5.0,
                    ..Default::default()
                }],
            })
            .success
    );

    assert!(state.save_project(Some(path.clone())).success);
    let after_first = Database::open_existing(&path)
        .unwrap()
        .load_undo_stack_with_timestamps()
        .unwrap()
        .len();
    assert_eq!(
        after_first, 1,
        "the first save persists the edit's undo entry"
    );

    // No edit in between - exactly the shape a push's flush after an autosave has.
    assert!(state.save_project(Some(path.clone())).success);
    let after_second = Database::open_existing(&path)
        .unwrap()
        .load_undo_stack_with_timestamps()
        .unwrap()
        .len();
    assert_eq!(
        after_second, 1,
        "a save with nothing changed must still write the history - the file it writes \
         into was just rebuilt, so anything it skips is gone for good"
    );
}

#[test]
fn test_undo_after_reload_restores_exactly_the_edited_field() {
    // open_project/save_project record the path in recent projects (stored in
    // the global config), so isolate the config from the real file.
    let _cfg = crate::test_helpers::isolated_config();
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("new_format_undo.stalx");

    let mut project = Project::new("New Format".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot_with_nullable_fields()],
        ..Default::default()
    });

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);

    // Session 1: open, edit ONLY the comment, save (persists the undo stack
    // in the new format).
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    let (section_id, shot_id) = {
        let project = state.get_project_state().unwrap();
        (project.sections[0].id, project.sections[0].shots[0].id)
    };
    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            comment: Some(Some("edited".to_string())),
            ..Default::default()
        },
    });
    assert!(result.success);
    assert!(state.save_project(Some(path.clone())).success);

    // Session 2: fresh app instance, reload, undo.
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(path).success);
    {
        let stack = state2.undo_stack.lock().unwrap();
        assert!(
            stack.can_undo(),
            "new-format undo history must survive reload"
        );
    }
    let project = state2.get_project_state().unwrap();
    assert_eq!(
        project.sections[0].shots[0].comment.as_deref(),
        Some("edited")
    );

    assert!(state2.undo().success);

    let project = state2.get_project_state().unwrap();
    let shot = &project.sections[0].shots[0];
    // Exactly the edited field is restored...
    assert_eq!(shot.comment.as_deref(), Some("original"));
    // ...and the untouched nullable fields were NOT cleared by the undo.
    assert_eq!(shot.pitch_in, Some(5.0));
    assert_eq!(shot.pitch_out, Some(-5.0));
    assert_eq!(shot.left, Some(1.5));
    assert_eq!(shot.right, Some(2.5));
    assert_eq!(shot.up, Some(0.5));
    assert_eq!(shot.down, Some(3.5));
    assert_eq!(shot.marker_index, Some(2));
    assert_eq!(shot.temperature, Some(14.5));
}

#[test]
fn test_merge_projects_places_by_datum_and_keeps_station_offsets_datums_and_closure_ids() {
    let temp_dir = TempDir::new().unwrap();

    let mut project_a = Project::new("A".to_string(), None);
    let section_a = project_a
        .add_section(SectionData {
            name: Some("Section A".to_string()),
            shots: vec![test_shot_data(1.0)],
            ..Default::default()
        })
        .id;
    project_a.section_offsets.insert(section_a, (1.0, 2.0));
    project_a
        .station_offsets
        .insert(StationRef::new(section_a, 0).into(), (0.5, -0.5, 1.0));
    let gps_a = GpsCoordinates::new(10.0, 20.0, 0.0).unwrap();
    project_a
        .store_zero_datum_without_repositioning(StationRef::new(section_a, 0), gps_a)
        .unwrap();
    project_a.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(section_a, 0).into(),
        station_b: StationRef::new(section_a, 1).into(),
        path_length: 1.0,
        misclosure_distance: 0.1,
        error_percentage: 10.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    let mut project_b = Project::new("B".to_string(), None);
    let section_b = project_b
        .add_section(SectionData {
            name: Some("Section B".to_string()),
            shots: vec![test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    project_b.section_offsets.insert(section_b, (3.0, 4.0));
    project_b
        .station_offsets
        .insert(StationRef::new(section_b, 0).into(), (-1.0, 1.5, 0.0));
    let gps_b = GpsCoordinates::new(-5.0, 40.0, 2.0).unwrap();
    project_b
        .store_zero_datum_without_repositioning(StationRef::new(section_b, 0), gps_b)
        .unwrap();
    project_b.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(section_b, 0).into(),
        station_b: StationRef::new(section_b, 1).into(),
        path_length: 2.0,
        misclosure_distance: 0.2,
        error_percentage: 10.0,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&project_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&project_b).unwrap();
    drop(db_a);
    drop(db_b);

    let state = AppState::new().unwrap();
    let output_path = temp_dir.path().join("merged-output.stalx");
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success);

    let db = Database::open_existing(&output_path).unwrap();
    let merged = db.load_project().unwrap();

    let merged_section_a = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Section A"))
        .unwrap()
        .id;
    let merged_section_b = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Section B"))
        .unwrap()
        .id;

    // #682: each georeferenced source is placed by its datum. The first
    // source's datum station becomes the frame origin, and the second's lands at
    // its GPS position in that frame - instead of both keeping their stored
    // offsets and the second survey being drawn on top of the first.
    let datum_station = |id| {
        merged
            .compute_station_raw_coords(StationRef::new(id, 0))
            .unwrap()
    };
    let a0 = datum_station(merged_section_a);
    assert!(
        a0.x.abs() < 1e-6 && a0.y.abs() < 1e-6,
        "the first datum station is the frame origin: {a0:?}"
    );
    let (expected_x, expected_y) = crate::models::gps_to_local(&gps_b, &gps_a);
    let b0 = datum_station(merged_section_b);
    assert!(
        (b0.x - expected_x).abs() < 1e-3 && (b0.y - expected_y).abs() < 1e-3,
        "the second datum station sits at its GPS position: {b0:?} vs ({expected_x}, {expected_y})"
    );

    assert_eq!(
        merged.station_offset_for_ref(StationRef::new(merged_section_a, 0)),
        Some((0.5, -0.5, 1.0))
    );
    assert_eq!(
        merged.station_offset_for_ref(StationRef::new(merged_section_b, 0)),
        Some((-1.0, 1.5, 0.0))
    );

    assert!(merged
        .zero_datums
        .iter()
        .any(|zd| zd.station_ref.section_id == merged_section_a && zd.gps.latitude == 10.0));
    assert!(merged
        .zero_datums
        .iter()
        .any(|zd| zd.station_ref.section_id == merged_section_b && zd.gps.latitude == -5.0));

    let mut closure_ids = HashSet::new();
    for closure in &merged.loop_closures {
        assert_ne!(closure.id, 0, "closure IDs should be reassigned");
        closure_ids.insert(closure.id);
    }
    assert_eq!(closure_ids.len(), merged.loop_closures.len());

    assert!(merged.loop_closures.iter().any(|c| {
        c.station_a.section_id == merged_section_a && c.station_b.section_id == merged_section_a
    }));
    assert!(merged.loop_closures.iter().any(|c| {
        c.station_a.section_id == merged_section_b && c.station_b.section_id == merged_section_b
    }));
}

/// `MergeProjects` is classified `affects_geometry`, so it reaches `execute`'s
/// renormalisation epilogue - and that gate now fires on `None -> Some` as well
/// as on `Some -> Some`. An open project with no datum reads `frame_datum_key()
/// == None` on the before side, so if the merge ever swapped its freshly built
/// project into the open slot the gate would find a frame where there had been
/// none and translate the whole survey onto it. It does not: `merge_projects`
/// builds a `Project` locally, writes it to a NEW file and returns, without ever
/// touching `self.project` (there is no reference to it anywhere in
/// `project_merge.rs`), so both halves of the gate read the same unchanged open
/// project and it returns without measuring anything.
///
/// That distinction has teeth: the merged project is laid out by its own
/// sources' datums (#682), not by the open project's frame, so a fire here would
/// silently translate one onto the other.
///
/// Non-vacuous on both sides. The open project carries section offsets that
/// nothing in the merge could produce and no datum at all, so a swap-in is
/// visible in the offsets, in every station coordinate and in `zero_datums`; and
/// the sources DO carry datums, so the `None -> Some` transition the widened
/// gate now admits is genuinely available for it to fire on. Cited by name from
/// `AppState::renormalize_frame_if_changed`'s doc comment.
#[test]
fn merge_projects_does_not_renormalise_the_open_project() {
    let temp_dir = TempDir::new().unwrap();

    // The OPEN project: two free-floating sections, no datum anywhere.
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Open".to_string(),
                location: None,
            })
            .success
    );
    let open_a = state
        .execute(Command::CreateSection {
            name: Some("Open A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(7.0)],
        })
        .section_id
        .unwrap();
    let open_b = state
        .execute(Command::CreateSection {
            name: Some("Open B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(9.0)],
        })
        .section_id
        .unwrap();

    // Offsets nothing in this merge could ever produce, so a swap-in is loud.
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        project.section_offsets.insert(open_a, (137.0, -42.0));
        project.section_offsets.insert(open_b, (-311.0, 63.0));
        assert!(
            project.zero_datums.is_empty() && project.frame_datum_key().is_none(),
            "the open project must have NO frame, so the gate sees None on the before side"
        );
    }
    let open_offsets_before = {
        let guard = state.project.lock().unwrap();
        guard.as_ref().unwrap().section_offsets.clone()
    };
    let open_coords_before = abs_coord_map(&state.get_project_state().unwrap());

    // Two sources on disk, both carrying a datum, so the merged project has a
    // frame the gate could latch onto if it ever saw it.
    let mut source_project_a = Project::new("Source A".to_string(), None);
    let source_section_a = source_project_a
        .add_section(SectionData {
            name: Some("Source Section A".to_string()),
            shots: vec![test_shot_data(1.0)],
            ..Default::default()
        })
        .id;
    source_project_a
        .section_offsets
        .insert(source_section_a, (5.0, 6.0));
    source_project_a
        .store_zero_datum_without_repositioning(
            StationRef::new(source_section_a, 0),
            GpsCoordinates::new(20.35, -87.4, 0.0).unwrap(),
        )
        .unwrap();

    let mut source_project_b = Project::new("Source B".to_string(), None);
    let source_section_b = source_project_b
        .add_section(SectionData {
            name: Some("Source Section B".to_string()),
            shots: vec![test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    source_project_b
        .section_offsets
        .insert(source_section_b, (-8.0, 3.0));

    let source_a = temp_dir.path().join("source-a.stalx");
    let source_b = temp_dir.path().join("source-b.stalx");
    Database::open(&source_a)
        .unwrap()
        .save_project(&source_project_a)
        .unwrap();
    Database::open(&source_b)
        .unwrap()
        .save_project(&source_project_b)
        .unwrap();

    let output_path = temp_dir.path().join("merged-output.stalx");
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "merge failed: {:?}", result.error);

    // The merged file really does carry a frame - otherwise there is nothing for
    // the epilogue to have renormalised onto and the test below is vacuous.
    let merged = Database::open_existing(&output_path)
        .unwrap()
        .load_project()
        .unwrap();
    assert!(
        merged.frame_datum_key().is_some(),
        "the merged output must have a frame datum for this test to mean anything"
    );

    // ...and the open project is untouched: same offsets, same coordinates, still
    // no datum.
    let open_after = state.get_project_state().unwrap();
    assert_eq!(
        open_after.section_offsets, open_offsets_before,
        "MergeProjects translated the open project's sections"
    );
    let open_coords_after = abs_coord_map(&open_after);
    assert_eq!(
        open_coords_after.len(),
        open_coords_before.len(),
        "the open project's station set changed"
    );
    for (station_ref, before) in &open_coords_before {
        let after = open_coords_after
            .get(station_ref)
            .unwrap_or_else(|| panic!("{station_ref:?} vanished from the open project"));
        assert_coords_close(*after, *before);
    }
    assert!(
        open_after.zero_datums.is_empty(),
        "the open project gained a datum from a merge that writes a different file"
    );
}

#[test]
fn test_merge_projects_preserves_station_comments() {
    // merge_projects remaps every sibling cross-reference collection; station
    // comments must be carried over too (they were previously dropped silently
    // while offsets/datums/merges/closures were all copied).
    let temp_dir = TempDir::new().unwrap();

    let mut project_a = Project::new("A".to_string(), None);
    let section_a = project_a
        .add_section(SectionData {
            name: Some("Section A".to_string()),
            shots: vec![test_shot_data(1.0)],
            ..Default::default()
        })
        .id;
    project_a.ensure_shot_uuids();
    let key_a = project_a
        .key_for_ref(StationRef::new(section_a, 0))
        .unwrap();
    project_a
        .station_comments
        .insert(key_a, "junction with main line".to_string());

    let mut project_b = Project::new("B".to_string(), None);
    let section_b = project_b
        .add_section(SectionData {
            name: Some("Section B".to_string()),
            shots: vec![test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    project_b.ensure_shot_uuids();
    let key_b = project_b
        .key_for_ref(StationRef::new(section_b, 1))
        .unwrap();
    project_b
        .station_comments
        .insert(key_b, "sump start".to_string());

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    Database::open(&source_a)
        .unwrap()
        .save_project(&project_a)
        .unwrap();
    Database::open(&source_b)
        .unwrap()
        .save_project(&project_b)
        .unwrap();

    let state = AppState::new().unwrap();
    let output_path = temp_dir.path().join("merged-output.stalx");
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "merge failed: {:?}", result.error);

    let merged = Database::open_existing(&output_path)
        .unwrap()
        .load_project()
        .unwrap();
    let merged_section_a = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Section A"))
        .unwrap()
        .id;
    let merged_section_b = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Section B"))
        .unwrap()
        .id;

    assert_eq!(
        merged
            .station_comment_for_ref(StationRef::new(merged_section_a, 0))
            .map(String::as_str),
        Some("junction with main line"),
        "section A's station comment must survive the merge"
    );
    assert_eq!(
        merged
            .station_comment_for_ref(StationRef::new(merged_section_b, 1))
            .map(String::as_str),
        Some("sump start"),
        "section B's station comment must survive the merge"
    );
}

#[test]
fn save_blocks_on_dangling_refs() {
    // A save with unresolvable cross-references is refused outright (no file
    // written), so corruption never reaches disk. The user is told it didn't run
    // and restores integrity via Repair, after which saves resume.
    //
    // This covers autosave too, and there is nothing extra to assert for it: the
    // backend save has no manual-vs-auto distinction. `Command::SaveProject`
    // carries only a path; the frontend's `isAutosave` reaches the backend for
    // the log line alone (`commands::project::save_project`) and never touches
    // the pipeline, so the debounced autosave reaches exactly the code exercised
    // below.
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();

    let mut project = Project::new("Dangle".to_string(), None);
    let a = project
        .add_section(SectionData {
            name: Some("Main".to_string()),
            shots: vec![test_shot_data(1.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Dangling station offset: anchors a shot UUID that doesn't exist.
    let dangling = StationKey::new(a, StationAnchor::AfterShot("ghost".to_string()));
    project.station_offsets.insert(dangling, (1.0, 2.0, 3.0));
    assert!(project.has_unresolvable_cross_references());
    state.set_project_for_tests(project);

    let stalx = temp_dir.path().join("dangle.stalx");

    let result = state.execute(Command::SaveProject {
        path: Some(stalx.clone()),
    });
    assert!(!result.success, "save must block on dangling refs");
    assert!(!stalx.exists(), "a blocked save must not write a file");
}

#[test]
fn repair_open_project_integrity_is_undoable_and_redoable() {
    // Repair drops dangling refs / re-cascades offsets; it must record an undo
    // entry so the user can revert it. Undo restores the pre-repair (corrupt)
    // state, redo re-applies the fix - both via the snapshot pair.
    let state = AppState::new().unwrap();

    let mut project = Project::new("Repairable".to_string(), None);
    let a = project
        .add_section(SectionData {
            name: Some("Main".to_string()),
            shots: vec![test_shot_data(1.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Dangling station offset → unresolvable cross-reference (corrupt project).
    let dangling = StationKey::new(a, StationAnchor::AfterShot("ghost".to_string()));
    project.station_offsets.insert(dangling, (1.0, 2.0, 3.0));
    assert!(project.has_unresolvable_cross_references());
    state.set_project_for_tests(project);

    // Repair fixes it AND records exactly one undo entry.
    let result = state.repair_open_project_integrity();
    assert!(result.fixed >= 1, "repair should report at least one fix");
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .has_unresolvable_cross_references(),
        "repair drops the dangling ref"
    );
    assert_eq!(
        state.get_undo_stack().len(),
        1,
        "repair records one undo entry"
    );

    // Undo restores the pre-repair (corrupt) state.
    assert!(state.undo().success, "repair must be undoable");
    assert!(
        state
            .get_project_state()
            .unwrap()
            .has_unresolvable_cross_references(),
        "undo brings the dangling ref back"
    );

    // Redo re-applies the repair.
    assert!(state.redo().success, "repair must be redoable");
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .has_unresolvable_cross_references(),
        "redo re-drops the dangling ref"
    );
}

#[test]
fn redo_batch_preserves_double_anchored_bridge_closure() {
    // A single user Batch can strand a sole-bridge closure: two otherwise
    // unconnected sections joined only by a freshly-created closure. Normally the
    // post-command epilogue converts that closure into a merge (#8 - closures are
    // only for true loops), and execute_batch captures its redo snapshot AFTER that
    // conversion so undo→redo preserves it.
    //
    // BUT here both sections are zero-datum-anchored (the only way to create a
    // closure between two disconnected sections). Converting the bridge to a rigid
    // merge would put two GPS anchors in one merge component and tear it - the
    // reported bug - so the conversion guard now LEAVES it as a closure. This test
    // pins that corrected behaviour AND that the batch snapshot still captures the
    // post-epilogue state across undo/redo: the closure (not a merge) is preserved,
    // and the two datums stay in separate merge components.
    let state = AppState::new().unwrap();

    let mut project = Project::new("Batch".to_string(), None);
    let a = project
        .add_section(SectionData {
            name: Some("A".to_string()),
            shots: vec![test_shot_data(1.0), test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            name: Some("B".to_string()),
            shots: vec![test_shot_data(3.0), test_shot_data(4.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    state.set_project_for_tests(project);

    // One atomic batch: give each section a datum (so the otherwise-unconnected
    // pair is allowed a closure), then close A1↔B1. With no other path between them
    // the closure is a sole bridge and the epilogue converts it to a merge.
    let result = state.execute(Command::Batch {
        commands: vec![
            Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            },
            Command::SetZeroDatum {
                station_ref: StationRef::new(b, 0),
                coordinates: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
            },
            Command::CreateLoopClosure {
                station_a: StationRef::new(a, 1),
                station_b: StationRef::new(b, 1),
                closure_strategy: ClosureStrategy::default(),
            },
        ],
    });
    assert!(result.success, "batch should succeed: {:?}", result.error);

    let after_execute = state.get_project_state().unwrap();
    assert_eq!(
        after_execute.loop_closures.len(),
        1,
        "a sole-bridge closure between two anchored groups must be LEFT as a closure, not merged"
    );
    assert_eq!(
        after_execute.station_merges.len(),
        0,
        "no merge: converting it would double-anchor one merge component"
    );

    // Undo returns to the pre-batch topology (no datums, no closure, no merge).
    assert!(state.undo().success);
    let after_undo = state.get_project_state().unwrap();
    assert_eq!(after_undo.loop_closures.len(), 0);
    assert_eq!(after_undo.station_merges.len(), 0);

    // Redo must restore the post-epilogue topology: the closure is preserved and it
    // is still NOT a merge.
    assert!(state.redo().success);
    let after_redo = state.get_project_state().unwrap();
    assert_eq!(
        after_redo.loop_closures.len(),
        1,
        "redo must preserve the unconverted bridge closure"
    );
    assert_eq!(
        after_redo.station_merges.len(),
        0,
        "redo must not create the over-constraining merge"
    );
}

#[test]
fn update_shot_reconciles_datum_less_merge() {
    // Editing a shot upstream of a merge in a group with NO zero datum used to
    // leave the merge drifted: UpdateShot was misclassified INERT (skipping the
    // reconcile epilogue) and the bespoke realign_after_shot_edit gave up without
    // a datum. Both are fixed - UpdateShot is geometry-affecting so the reconcile
    // runs, and the realign anchors an unanchored group on the edited section
    // (#413 finding 20) - so this heals in the same command by either route.
    let state = AppState::new().unwrap();
    let mut project = Project::new("NoDatum".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![test_shot_data(5.0)],
            ..Default::default()
        })
        .id;
    // Merge A:2 (after both shots) ↔ B:0, with NO zero datum anywhere.
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty(), "starts aligned");
    state.set_project_for_tests(project);

    // Edit shot 0 in A (upstream of the merge station) - a big length change moves
    // A:2 well beyond the 5 cm reporting tolerance.
    let shot0 = state
        .get_project_state()
        .unwrap()
        .get_section(a)
        .unwrap()
        .shots[0]
        .id;
    let updates = ShotUpdates {
        length: Some(9.0),
        ..Default::default()
    };
    let result = state.execute(Command::UpdateShot {
        section_id: a,
        shot_id: shot0,
        updates,
    });
    assert!(result.success, "{:?}", result.error);

    assert!(
        state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a shot edit must reconcile the datum-less merge, not leave it drifted"
    );
}

#[test]
fn metadata_only_update_shot_does_not_heal_drift() {
    // The converse of update_shot_reconciles_datum_less_merge: a comment/LRUD/
    // reference-only UpdateShot touches no geometry, so it must NOT run the
    // reconcile epilogue. Reconciling would silently re-align an already-drifted
    // project as a side effect of a metadata edit whose undo only restores the
    // metadata - the asymmetric-undo hazard that keeps SetStationComment INERT.
    let state = AppState::new().unwrap();
    let mut project = Project::new("Drift".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![test_shot_data(5.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty(), "starts aligned");

    // Introduce drift WITHOUT reconciling: lengthen a shot upstream of the merge
    // directly in the model, so A:2 moves out of alignment with B:0.
    let shot0 = project.get_section(a).unwrap().shots[0].id;
    project.get_section_mut(a).unwrap().shots[0].length = 9.0;
    assert!(
        !project.check_merge_alignment().is_empty(),
        "drift is present before the metadata edit"
    );
    state.set_project_for_tests(project);

    // A comment-only edit must leave the drift untouched.
    let updates = ShotUpdates {
        comment: Some(Some("note".to_string())),
        ..Default::default()
    };
    let result = state.execute(Command::UpdateShot {
        section_id: a,
        shot_id: shot0,
        updates,
    });
    assert!(result.success, "{:?}", result.error);
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a metadata-only shot edit must not reconcile (heal) a drifted merge"
    );

    // Sanity-check the gate isn't simply always-off: a geometry edit DOES heal it.
    let updates = ShotUpdates {
        length: Some(7.0),
        ..Default::default()
    };
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: a,
                shot_id: shot0,
                updates
            })
            .success
    );
    assert!(
        state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a geometry edit must reconcile the drift"
    );
}

#[test]
fn metadata_only_update_shot_batch_does_not_heal_drift() {
    // execute_batch gates its reconcile epilogue the same way execute() does: a
    // batch of metadata-only UpdateShots (a multi-cell comment paste) must not
    // heal a drifted merge as a side effect.
    let state = AppState::new().unwrap();
    let mut project = Project::new("DriftBatch".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![test_shot_data(5.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty(), "starts aligned");

    let shot0 = project.get_section(a).unwrap().shots[0].id;
    let shot1 = project.get_section(a).unwrap().shots[1].id;
    project.get_section_mut(a).unwrap().shots[0].length = 9.0;
    assert!(
        !project.check_merge_alignment().is_empty(),
        "drift present before the batch"
    );
    state.set_project_for_tests(project);

    // Multi-cell comment paste → one batch of two comment-only UpdateShots.
    let mk = |shot_id| Command::UpdateShot {
        section_id: a,
        shot_id,
        updates: ShotUpdates {
            comment: Some(Some("note".to_string())),
            ..Default::default()
        },
    };
    let result = state.execute(Command::Batch {
        commands: vec![mk(shot0), mk(shot1)],
    });
    assert!(result.success, "{:?}", result.error);
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a metadata-only batch must not reconcile (heal) a drifted merge"
    );
}

#[test]
fn repair_noop_on_clean_project_reports_unchanged() {
    // A repair that finds nothing to fix must not report `changed` (which the
    // frontend dirties on) or push a spurious undo entry. This depends on
    // resolve_offsets() no longer touch()ing an already-consistent project - if it
    // did, the snapshot's modified_at would differ every time and `changed` would
    // be permanently true.
    let state = AppState::new().unwrap();
    let mut project = Project::new("Clean".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    project.resolve_offsets();
    state.set_project_for_tests(project);

    let result = state.repair_open_project_integrity();
    assert_eq!(result.fixed, 0);
    assert!(!result.changed, "a no-op repair must not report changed");
    assert_eq!(
        state.get_undo_stack().len(),
        0,
        "a no-op repair adds no undo entry"
    );
}

#[test]
fn save_remints_duplicate_shot_uuids_before_persisting() {
    // A within-section duplicate shot UUID is Critical (resolve_key snaps
    // AfterShot(uuid) to the FIRST match), but the unresolvable-cross-reference save
    // gate doesn't catch it (the ref still resolves, just to the wrong shot). The
    // save path re-mints the duplicate first so unique UUIDs reach disk - lossless,
    // so it auto-heals rather than blocking.
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("Dup".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![test_shot_data(1.0), test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Force a within-section duplicate.
    let dup = project.get_section(a).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    project.get_section_mut(a).unwrap().shots[1].uuid = Some(dup.clone());
    assert!(
        project.has_critical_integrity_violations(),
        "dup UUID is Critical"
    );
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("dup")));
    assert!(
        result.success,
        "save auto-heals dup UUIDs rather than blocking: {:?}",
        result.error
    );

    // The persisted (and in-memory) project now has unique UUIDs, no Critical.
    let saved = state.get_project_state().unwrap();
    let uuids: Vec<_> = saved
        .get_section(a)
        .unwrap()
        .shots
        .iter()
        .filter_map(|s| s.uuid.clone())
        .collect();
    let mut uniq = uuids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(
        uuids.len(),
        uniq.len(),
        "duplicate shot UUID re-minted before persist"
    );
    assert!(!saved.has_critical_integrity_violations());
}

#[test]
fn restore_snapshot_canonicalizes_legacy_index_anchors() {
    // A snapshot persisted by a PRE-ANCHOR build can carry positional Index
    // anchors (undo/redo history is persisted and reloaded across builds, and the
    // back-compat StationKey deserializer accepts the legacy shape). Restoring it
    // must canonicalize to AfterShot - otherwise a later shot insert/delete won't
    // shift the ref and the merge/datum/comment silently points at the wrong station.
    let mut project = Project::new("Legacy".to_string(), None);
    let s = project
        .add_section(SectionData {
            shots: vec![
                test_shot_data(1.0),
                test_shot_data(2.0),
                test_shot_data(3.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // A station offset stored in the legacy positional (Index) form.
    project
        .station_offsets
        .insert(StationKey::new(s, StationAnchor::Index(2)), (1.0, 2.0, 3.0));
    let snapshot = serde_json::to_string(&project).unwrap();

    let mut slot: Option<Project> = None;
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));
    let restored = slot.unwrap();

    // No Index anchor survives into the live project after restore.
    let mut saw_index = false;
    restored.for_each_station_ref(|_label, key| {
        if matches!(key.anchor, StationAnchor::Index(_)) {
            saw_index = true;
        }
    });
    assert!(
        !saw_index,
        "restore_snapshot_into must canonicalize legacy Index anchors"
    );
}

#[test]
fn repair_serializes_under_command_lock() {
    // Repair must hold command_lock for its whole duration so a concurrent command
    // can't slip a mutation between repair's before-snapshot and its own mutation
    // (which would make undo-of-repair drop that command). Verify by holding the
    // lock and asserting repair blocks until it's released.
    use std::sync::mpsc;
    use std::time::Duration;
    let state = Arc::new(AppState::new().unwrap());
    let mut project = Project::new("LockRepair".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    state.set_project_for_tests(project);

    let guard = state.command_lock.lock().unwrap();
    let state2 = Arc::clone(&state);
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(state2.repair_open_project_integrity());
    });
    // While we hold command_lock, repair must block (no result arrives).
    assert!(
        rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "repair ran without acquiring command_lock"
    );
    drop(guard);
    // Once released, repair completes.
    let result = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("repair completes after command_lock is released");
    handle.join().unwrap();
    assert_eq!(result.fixed, 0);
}

#[test]
fn save_drops_orphan_section_offset() {
    // A section_offsets entry for a section that no longer exists has a dangling
    // SQLite FK; persisting it aborts the WHOLE save with a generic error. It must
    // be dropped on save instead (the cross-reference gate doesn't see section_offsets).
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("Orphan".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    project.section_offsets.insert(99999, (5.0, 6.0)); // no such section
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("orphan")));
    assert!(
        result.success,
        "orphan section_offset must not abort the save: {:?}",
        result.error
    );
    let saved = state.get_project_state().unwrap();
    assert!(
        saved
            .section_offsets
            .keys()
            .all(|sid| saved.get_section(*sid).is_some()),
        "orphan section_offsets must be dropped on save"
    );
}

/// [#469] A `.stalx` written before the dust gate widened can carry a
/// `section_offsets` value that was REAL to the old 1e-12 gate and is dust to
/// `Project::OFFSET_DUST_M` (1e-6). Opening drops the key and moves that section
/// by the value it held - the deliberate on-disk consequence of widening the
/// gate, pinned here rather than left to be discovered.
///
/// Three claims, because each is a separate way this could go wrong: the key is
/// GONE after the open (`normalize_section_offsets` at the load boundary, see
/// `Project::redrive_offsets_after_load`), the geometry moved by less than a
/// micrometre rather than by anything a survey could notice, and a save/reopen
/// CONVERGES - the row does not come back on the next round trip.
#[test]
fn opening_drops_a_legacy_sub_gate_section_offset() {
    // 5e-7 m, half the gate: the old bound preserved it, the current writer
    // cannot produce it (`Project::write_section_offset` removes the key
    // instead), so only a legacy FILE can hold one.
    const LEGACY_OFFSET: (f64, f64) = (5e-7, -5e-7);

    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("legacy-dust.stalx");

        let section_id = {
            let mut project = Project::new("LegacyDust".to_string(), None);
            let section_id = project
                .add_section(SectionData {
                    shots: vec![test_shot_data(10.0)],
                    ..Default::default()
                })
                .id;
            project.ensure_shot_uuids();
            // Straight into the map: routing through the writer would normalize it
            // away, which is the point - this state is unrepresentable in memory.
            project.section_offsets.insert(section_id, LEGACY_OFFSET);

            // The drop is `normalize_section_offsets`' doing. Asked directly, it
            // claims exactly this one row.
            let mut probe = project.clone();
            assert_eq!(
                probe.normalize_section_offsets(),
                1,
                "the sub-gate row is what the load-boundary normalize drops"
            );

            let mut db = Database::open(&path).unwrap();
            db.save_project(&project).unwrap();
            section_id
        };

        // Precondition: the value really is on disk. The save does not normalize -
        // if it ever starts to, this test is measuring nothing.
        assert_eq!(
            Database::open_existing(&path)
                .unwrap()
                .load_project()
                .unwrap()
                .section_offsets
                .get(&section_id)
                .copied(),
            Some(LEGACY_OFFSET),
            "precondition: the legacy file carries the sub-gate offset"
        );

        let state = AppState::new().unwrap();
        assert!(
            state.open_project(path.clone()).success,
            "open must succeed"
        );
        let opened = state.get_project_state().unwrap();
        assert!(
            !opened.section_offsets.contains_key(&section_id),
            "the load boundary must drop a sub-gate offset: {:?}",
            opened.section_offsets.get(&section_id)
        );

        // ...and the geometry it moved by is the value that went: under a
        // micrometre, four decades under anything the app displays.
        let station = StationRef::new(section_id, 0);
        let moved = opened.compute_station_raw_coords(station).unwrap();
        let (dx, dy) = (moved.x - LEGACY_OFFSET.0, moved.y - LEGACY_OFFSET.1);
        assert!(
            dx.abs() < Project::OFFSET_DUST_M && dy.abs() < Project::OFFSET_DUST_M,
            "dropping the key moved station 0 by ({dx}, {dy}), which is not sub-gate"
        );

        // Convergence: saving what the open produced and reopening it leaves the
        // key absent. A boundary that dropped and a writer that re-minted would
        // make every round trip a fresh edit.
        assert!(state.save_project(Some(path.clone())).success);
        let reopened_state = AppState::new().unwrap();
        assert!(reopened_state.open_project(path).success);
        let reopened = reopened_state.get_project_state().unwrap();
        assert!(
            !reopened.section_offsets.contains_key(&section_id),
            "a sub-gate offset must not resurrect across save/reopen: {:?}",
            reopened.section_offsets.get(&section_id)
        );
        let again = reopened.compute_station_raw_coords(station).unwrap();
        assert_eq!(
            (again.x.to_bits(), again.y.to_bits()),
            (moved.x.to_bits(), moved.y.to_bits()),
            "the second round trip must not move the geometry again - bit-exact, \
             because a converged round trip has nothing left to round off"
        );
    });
}

#[test]
fn save_sanitizes_non_finite_geometry() {
    // A non-finite shot value would abort the save (NaN → NULL in a NOT NULL
    // column) or persist garbage (inf); it must be sanitized on save instead.
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("NonFinite".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    project.sections[0].shots[0].length = f64::NAN;
    assert!(!project.sections[0].shots[0].all_finite());
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("nonfinite")));
    assert!(
        result.success,
        "non-finite geometry must be sanitized, not abort the save: {:?}",
        result.error
    );
    let saved = state.get_project_state().unwrap();
    assert!(
        saved.sections[0].shots[0].all_finite(),
        "non-finite geometry sanitized on save"
    );
}

#[test]
fn save_sanitizes_non_finite_section_offset() {
    // section_offsets is the one geometry field with no finite guard. A non-finite
    // offset (corrupt/hand-edited DB, or an inf round-trip) would abort the save
    // (NaN → NULL) or persist and poison the component's world coords. It must be
    // sanitized on save, mirroring the shot sanitize and the load-boundary guard.
    //
    // And sanitized THE WAY the map spells (0, 0): by dropping the key. A fully
    // non-finite offset sanitizes to no displacement at all, and this map says
    // that with an absent key - writing a zero row instead persisted an inert
    // entry no other route to the same project has (#413 finding 2).
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("NonFiniteOffset".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![test_shot_data(1.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.section_offsets.insert(a, (f64::NAN, f64::INFINITY));
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("nf_offset")));
    assert!(
        result.success,
        "a non-finite section offset must be sanitized, not abort the save: {:?}",
        result.error
    );
    let saved = state.get_project_state().unwrap();
    let off = saved.section_offsets.get(&a).copied().unwrap_or((0.0, 0.0));
    assert!(
        off.0.is_finite() && off.1.is_finite(),
        "section offset sanitized on save"
    );
    assert!(
        !saved.section_offsets.contains_key(&a),
        "a wholly non-finite offset sanitizes to no displacement, so the KEY must go - \
         a (0, 0) row is an inert entry the next save serializes again: {:?}",
        saved.section_offsets.get(&a)
    );

    // A half-non-finite offset keeps the component that IS a position: only the
    // poisoned axis is neutralized, so the key stays with the finite value.
    let mut project = state.get_project_state().unwrap();
    project.section_offsets.insert(a, (7.5, f64::NAN));
    state.set_project_for_tests(project);
    let result = state.save_project(Some(temp_dir.path().join("nf_offset_half")));
    assert!(result.success, "half-non-finite: {:?}", result.error);
    let saved = state.get_project_state().unwrap();
    assert_eq!(
        saved.section_offsets.get(&a).copied(),
        Some((7.5, 0.0)),
        "only the non-finite component is neutralized; the section is still displaced"
    );
}

#[test]
fn blocked_save_does_not_mutate_non_finite_geometry() {
    // A save refused for a dangling cross-reference must NOT first run the
    // destructive non-finite sanitize on the live project - that mutation has no
    // undo entry, so a refused save would silently lose the user's shot values.
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("BlockedNoMutate".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![test_shot_data(1.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // A dangling station offset → unresolvable cross-reference (save-blocking).
    project.station_offsets.insert(
        StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        (1.0, 2.0, 3.0),
    );
    // ...and a non-finite shot the (now later) sanitize would otherwise destroy.
    project.sections[0].shots[0].length = f64::NAN;
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("blocked")));
    assert!(
        !result.success,
        "save must be blocked by the dangling cross-reference"
    );
    // The gate runs BEFORE the destructive sanitize, so the live shot is untouched.
    let unsaved = state.get_project_state().unwrap();
    assert!(
        !unsaved.sections[0].shots[0].all_finite(),
        "a refused save must not destructively sanitize the live project"
    );
}

#[test]
fn undo_of_duplicate_uuid_repair_restores_the_duplicates() {
    // The pre-repair state of a duplicate_shot_uuid repair is a CURRENT snapshot
    // that intentionally holds duplicate UUIDs. Undo must restore it EXACTLY -
    // restore_snapshot_into must not re-dedup a non-legacy (no Index anchor)
    // snapshot, or the repair can't be reverted.
    let state = AppState::new().unwrap();
    let mut project = Project::new("DupRepairUndo".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![test_shot_data(1.0), test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // A within-section duplicate UUID: a current corrupt state (AfterShot anchors).
    let dup = project.get_section(a).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    project.get_section_mut(a).unwrap().shots[1].uuid = Some(dup.clone());
    assert!(project.has_critical_integrity_violations());
    state.set_project_for_tests(project);

    // Repair re-mints the duplicate (now unique) and records the dup state as undo `before`.
    let r = state.repair_open_project_integrity();
    assert!(r.changed, "repair changed the project");
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .has_critical_integrity_violations(),
        "repair deduped the UUIDs"
    );

    // Undo must restore the duplicate-UUID state EXACTLY (and its warning).
    assert!(state.undo().success);
    let after_undo = state.get_project_state().unwrap();
    let uuids: Vec<_> = after_undo
        .get_section(a)
        .unwrap()
        .shots
        .iter()
        .filter_map(|s| s.uuid.clone())
        .collect();
    assert_eq!(
        uuids[0], uuids[1],
        "undo restored the duplicate UUID exactly"
    );
    assert!(
        after_undo.has_critical_integrity_violations(),
        "the duplicate_shot_uuid warning is back after undo"
    );
}

#[test]
fn restore_snapshot_mints_null_uuids_before_reanchor() {
    // A legacy (pre-v6) snapshot can carry shots with uuid:null AND positional Index
    // anchors. restore must MINT the missing UUIDs before re-anchoring -
    // ensure_unique only dedups EXISTING UUIDs - or key_for_ref can't canonicalize
    // the Index anchor and it stays live (the gap S4 closes in the P1 fix).
    let mut project = Project::new("LegacyNullUuid".to_string(), None);
    let s = project
        .add_section(SectionData {
            shots: vec![
                test_shot_data(1.0),
                test_shot_data(2.0),
                test_shot_data(3.0),
            ],
            ..Default::default()
        })
        .id;
    // Force legacy null UUIDs (pre-v6 files load with NULL uuid columns).
    for shot in &mut project.get_section_mut(s).unwrap().shots {
        shot.uuid = None;
    }
    // A station offset stored in the legacy positional (Index) form.
    project
        .station_offsets
        .insert(StationKey::new(s, StationAnchor::Index(2)), (1.0, 2.0, 3.0));
    let snapshot = serde_json::to_string(&project).unwrap();

    let mut slot: Option<Project> = None;
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));
    let restored = slot.unwrap();

    // No Index anchor survives: the null UUIDs were minted, so reanchor could
    // canonicalize Index → AfterShot.
    let mut saw_index = false;
    restored.for_each_station_ref(|_label, key| {
        if matches!(key.anchor, StationAnchor::Index(_)) {
            saw_index = true;
        }
    });
    assert!(
        !saw_index,
        "restore must mint null UUIDs so Index anchors canonicalize"
    );
}

#[test]
fn finalize_structural_edit_ignores_pre_existing_critical() {
    // A project opened ALREADY critical (e.g. a dangling cross-reference) must not
    // have an otherwise-clean import rolled back: finalize signals a
    // rollback only for NEWLY introduced corruption, matching execute()'s guard.
    let state = AppState::new().unwrap();
    let mut project = Project::new("AlreadyCritical".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    // Inject a non-finite shot value → non_finite_geometry (a Critical violation).
    project.sections[0].shots[0].length = f64::NAN;
    assert!(project.has_critical_integrity_violations());
    state.set_project_for_tests(project);

    // pre_had_critical = true: do NOT signal rollback (the corruption pre-existed).
    assert!(
        !state.finalize_structural_edit(true),
        "must not block an otherwise-clean edit on an already-critical project"
    );
    // pre_had_critical = false (clean before): DOES signal rollback (corruption present).
    assert!(
        state.finalize_structural_edit(false),
        "must block when this edit introduced the critical violation"
    );
}

#[test]
fn redo_legacy_prepend_createshot_reanchors_section_start() {
    // A CreateShot undo entry persisted by a PRE-anchor build has after_shot_id /
    // before_shot_id == None even for a prepend, so redo used to hit a raw insert
    // that skipped the SectionStart re-anchor - leaving a station-0 datum pointing at
    // the newly-prepended station instead of the original first station. Redo now
    // routes a legacy (anchor-less) entry through the insert helper by position.
    let state = AppState::new().unwrap();
    let mut project = Project::new("LegacyRedo".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![test_shot_data(1.0), test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Zero datum on station 0 → stored as a SectionStart anchor.
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
        )
        .unwrap();
    state.set_project_for_tests(project);

    // A LEGACY redo entry: prepend (position 0) with NO anchor fields, as a
    // pre-anchor build would have persisted it.
    let entry = UndoEntry {
        command_bytes: None,
        command: Command::CreateShot {
            section_id: a,
            shot: test_shot_data(5.0),
            after_shot_id: None,
            before_shot_id: None,
        },
        undo_data: UndoData::CreatedShotSnapshot {
            section_id: a,
            shot_id: 999,
            shot_data: test_shot_data(5.0),
            position: 0,
            station_shift_from: 0,
            after_shot_id: None,
            before_shot_id: None,
            after_shot_uuid: None,
            before_shot_uuid: None,
            uuid: Some("prep-uuid".to_string()),
            previous_offsets: HashMap::new(),
        },
        timestamp: None,
    };
    state.undo_stack.lock().unwrap().push_redo(entry);
    assert!(state.redo().success, "legacy prepend redo");

    let restored = state.get_project_state().unwrap();
    assert_eq!(
        restored.get_section(a).unwrap().shots.len(),
        3,
        "prepend re-applied"
    );
    assert_eq!(
        restored.get_section(a).unwrap().shots[0].uuid.as_deref(),
        Some("prep-uuid"),
    );
    // The station-0 datum must still resolve to the ORIGINAL first station (now
    // index 1), not the freshly-prepended station (index 0).
    let datum_key = restored.zero_datums[0].station_ref.clone();
    let resolved = restored
        .resolve_key(&datum_key)
        .expect("datum still resolves");
    assert_eq!(
        resolved.station_index, 1,
        "SectionStart datum re-anchored to the original first station after a legacy prepend redo"
    );
}

#[test]
fn test_open_project_fails_on_invalid_file() {
    let temp_dir = TempDir::new().unwrap();
    let invalid_path = temp_dir.path().join("invalid.stalx");
    std::fs::write(&invalid_path, b"not a database").unwrap();

    let state = AppState::new().unwrap();
    let result = state.open_project(invalid_path);
    assert!(!result.success);
    let err = result.error.unwrap_or_default();
    let err_lower = err.to_lowercase();
    assert!(
        err_lower.contains("database") || err_lower.contains("corrupt"),
        "Expected database/corruption error, got: {err}"
    );
}

#[test]
fn test_save_project_persists_display_settings_cache() {
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Cache Save".to_string(),
        location: None,
    });

    state.save_display_setting("showStations", "false").unwrap();

    let save_path = temp_dir.path().join("cache-save");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    let saved_path = save_path.with_extension("stalx");
    let db = Database::open_readonly(&saved_path).unwrap();
    let settings = db.load_display_settings().unwrap();
    assert_eq!(settings.get("showStations"), Some(&"false".to_string()));

    let cache = state.display_settings_cache.lock().unwrap();
    assert!(cache.is_empty());
}

#[test]
fn test_undo_fails_with_invalid_previous_values_shot() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::UpdateShot {
            section_id: 1,
            shot_id: 1,
            updates: ShotUpdates::default(),
        },
        UndoData::PreviousValues {
            section_id: 1,
            shot_id: Some(1),
            shot_uuid: None,
            values_json: "not-json".to_string(),
            previous_offsets: HashMap::new(),
        },
    );
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to parse undo data"));
}

#[test]
fn test_undo_fails_with_invalid_previous_values_section() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::UpdateSectionMetadata {
            section_id: 1,
            updates: SectionUpdates::default(),
        },
        UndoData::PreviousValues {
            section_id: 1,
            shot_id: None,
            shot_uuid: None,
            values_json: "not-json".to_string(),
            previous_offsets: HashMap::new(),
        },
    );
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to parse undo data"));
}

#[test]
fn test_undo_fails_with_invalid_project_snapshot() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshot("not-json".to_string()),
    );
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to parse project snapshot")
    );
}

#[test]
fn test_redo_fails_with_invalid_project_snapshot_pair() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock().unwrap();
    let entry = UndoEntry {
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair {
            before: "{}".to_string(),
            after: "not-json".to_string(),
        },
        timestamp: None,
    };
    stack.push_redo(entry);
    drop(stack);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to parse project snapshot")
    );
}

#[test]
fn test_redo_fails_for_command_not_allowed_in_undo_context() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock().unwrap();
    let entry = UndoEntry {
        command_bytes: None,
        command: Command::MergeProjects {
            source_paths: Vec::new(),
            output_path: PathBuf::from("out.stalx"),
            output_name: "Merged".to_string(),
            archive_originals: false,
        },
        undo_data: UndoData::None,
        timestamp: None,
    };
    stack.push_redo(entry);
    drop(stack);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Command cannot be executed in undo context")
    );
}

#[test]
fn test_create_project_without_location() {
    let state = AppState::new().unwrap();

    let result = state.execute(Command::CreateProject {
        name: "Unnamed Cave".to_string(),
        location: None,
    });

    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.name, "Unnamed Cave");
    assert!(project.location.is_none());
}

#[test]
fn test_close_project() {
    let state = AppState::new().unwrap();

    // Create a project first
    state.execute(Command::CreateProject {
        name: "To Close".to_string(),
        location: None,
    });

    // Close it
    let result = state.execute(Command::CloseProject);
    assert!(result.success);

    // Project should be None
    assert!(state.get_project_state().is_none());
}

// === Section Operations Tests ===

#[test]
fn test_create_section() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: Some("John".to_string()),
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
    });

    assert!(result.success);
    assert!(result.section_id.is_some());

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].name, Some("Section A".to_string()));
    assert_eq!(project.sections[0].shots.len(), 2);
}

#[test]
fn get_adjusted_stations_none_without_project() {
    let state = AppState::new().unwrap();
    assert!(state.get_adjusted_stations().is_none());
}

#[test]
fn get_adjusted_stations_caches_and_invalidates() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Adj".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
    });

    // Two consecutive calls return the same result (second served from cache).
    let adj1 = state.get_adjusted_stations().expect("adjusted stations");
    let adj1_again = state
        .get_adjusted_stations()
        .expect("adjusted stations (cached)");
    assert_eq!(adj1.len(), adj1_again.len());
    assert_eq!(adj1.len(), 3, "two shots → three stations");

    // A mutating command must invalidate the cache: the adjusted view reflects
    // the newly added section.
    state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#222222".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![test_shot_data(5.0)],
    });
    let adj2 = state
        .get_adjusted_stations()
        .expect("adjusted stations after edit");
    assert_eq!(
        adj2.len(),
        5,
        "adjusted view must reflect the added section after cache invalidation"
    );
}

#[test]
fn distance_field_none_without_project() {
    let state = AppState::new().unwrap();
    assert!(state.distance_field(None).is_none());
}

#[test]
fn distance_field_from_origin_over_appstate() {
    // End-to-end through the atomic accessor (#34): one reconciled snapshot →
    // adjusted coords → distance_field_from, all at a single consistent version.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Dist".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
    });
    let section_id = state.get_project_state().unwrap().sections[0].id;
    let origin = StationRef::new(section_id, 0);

    let field = state.distance_field(Some(&origin)).expect("distance field");
    assert_eq!(field.distances.len(), 3, "two shots → three stations");
    assert_eq!(field.station_refs.len(), 3);
    let oi = field
        .station_refs
        .iter()
        .position(|r| *r == origin)
        .expect("origin present");
    assert_eq!(field.distances[oi], Some(0.0), "origin distance is zero");
    assert!(
        field.distances.iter().all(Option::is_some),
        "single connected section: every station reachable"
    );
    assert!(field.max_reachable > 0.0);
}

#[test]
fn repair_invalidates_adjusted_cache() {
    use std::sync::atomic::Ordering::SeqCst;
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Repair".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
    });

    // Populate the adjusted cache.
    let _ = state.get_adjusted_stations().expect("adjusted stations");
    assert!(
        state.adjusted_cache.lock().unwrap().is_some(),
        "cache should be populated after a solve"
    );
    let gen_before = state.adjusted_cache_generation.load(SeqCst);

    // Repair mutates measured geometry without necessarily bumping modified_at, so
    // it must drop the cached adjusted solve (otherwise a later read serves stale
    // geometry keyed on an unchanged version).
    let _ = state.repair_open_project_integrity();
    assert!(
        state.adjusted_cache.lock().unwrap().is_none(),
        "repair must invalidate the adjusted cache"
    );
    assert!(
        state.adjusted_cache_generation.load(SeqCst) > gen_before,
        "repair must bump the invalidation generation"
    );
}

#[test]
fn unmerge_converts_orphaned_bridge_closure() {
    // Deleting the merge that was a loop's other path strands the section onto
    // the closure - which must then become a merge (same #8 principle as delete).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closure.success, "{:?}", closure.error);
    assert_eq!(state.get_project_state().unwrap().loop_closures.len(), 1);

    // Delete the merge that closed the loop's other path.
    let r = state.execute(Command::UnmergeStationPair {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);

    let p = state.get_project_state().unwrap();
    assert!(
        p.loop_closures.is_empty(),
        "unmerge should convert the now-sole-bridge closure to a merge: {:?}",
        p.loop_closures
    );
    assert!(p.check_orphan_closures().is_empty());
}

#[test]
fn shot_sanitize_non_finite_neutralizes_geometry() {
    // The load-boundary sanitizer (used by the SQLite loader) replaces non-finite
    // core fields with 0.0 and non-finite optional fields with None.
    use crate::models::ShotData;
    let mut shot = crate::models::Shot::new(
        1,
        ShotData {
            azimuth_in: 1.0,
            azimuth_out: 1.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        },
    );
    shot.length = f64::NAN;
    shot.depth_out = f64::INFINITY;
    shot.pitch_in = Some(f64::NEG_INFINITY);
    shot.left = Some(2.0); // finite optional stays

    assert!(shot.sanitize_non_finite(), "must report a change");
    assert!(shot.all_finite(), "result must be finite");
    assert_eq!(shot.length, 0.0);
    assert_eq!(shot.depth_out, 0.0);
    assert_eq!(shot.pitch_in, None);
    assert_eq!(shot.left, Some(2.0), "finite optional preserved");
    assert!(
        !shot.sanitize_non_finite(),
        "already-finite shot reports no change"
    );
}

#[test]
fn coordinates_new_checked_rejects_non_finite() {
    assert!(Coordinates::new_checked(1.0, 2.0, 3.0).is_ok());
    assert!(Coordinates::new_checked(f64::NAN, 0.0, 0.0).is_err());
    assert!(Coordinates::new_checked(0.0, f64::INFINITY, 0.0).is_err());
    assert!(Coordinates::new_checked(0.0, 0.0, f64::NEG_INFINITY).is_err());
}

#[test]
fn move_section_and_segment_reject_non_finite_offset() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let nan = Coordinates::new(f64::NAN, 0.0, 0.0);
    assert!(
        !state
            .execute(Command::MoveSection {
                section_id: a,
                offset: nan
            })
            .success
    );
    assert!(
        !state
            .execute(Command::MoveSegment {
                section_id: a,
                offset: nan
            })
            .success
    );
    // A finite move still works.
    assert!(
        state
            .execute(Command::MoveSegment {
                section_id: a,
                offset: Coordinates::new(1.0, 2.0, 0.0),
            })
            .success
    );
}

#[test]
fn merge_allowed_across_inert_closure_but_rejected_across_aligned() {
    // A merge is rejected only when the two sections are already RIGIDLY connected
    // (merges or ALIGNED closures). An inert (unaligned) closure does not constrain
    // them, so merging across one - the first real link - must be allowed; the old
    // connectivity walk followed inert closures and wrongly rejected it, forcing the
    // sole-bridge-closure-that-should-be-a-merge bug (catalog #8).
    fn setup(aligned: bool) -> (AppState, SectionId, SectionId) {
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: "X".to_string(),
            location: None,
        });
        let a = state
            .execute(Command::CreateSection {
                name: Some("A".to_string()),
                surveyor: None,
                date: None,
                color: "#111111".to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![test_shot_data(5.0)],
            })
            .section_id
            .unwrap();
        let b = state
            .execute(Command::CreateSection {
                name: Some("B".to_string()),
                surveyor: None,
                date: None,
                color: "#222222".to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![test_shot_data(5.0)],
            })
            .section_id
            .unwrap();
        // Inject a single closure A0 <-> B1 between the otherwise-unconnected sections.
        {
            let mut g = state.project.lock().unwrap();
            let p = g.as_mut().unwrap();
            let id = p.next_closure_id;
            p.loop_closures.push(LoopClosure {
                id,
                station_a: StationRef::new(a, 0).into(),
                station_b: StationRef::new(b, 1).into(),
                path_length: 10.0,
                misclosure_distance: 0.1,
                error_percentage: 1.0,
                horizontal_misclosure: 0.08,
                vertical_misclosure: 0.06,
                strategy: ClosureStrategy::default(),
                is_aligned: aligned,
            });
            p.next_closure_id += 1;
        }
        (state, a, b)
    }

    // Inert closure → the merge is the first rigid link → ALLOWED, result sound.
    let (state, a, b) = setup(false);
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(
        r.success,
        "merge across an inert closure must be allowed: {:?}",
        r.error
    );
    assert!(!state
        .get_project_state()
        .unwrap()
        .has_critical_integrity_violations());

    // Aligned closure → already rigidly connected → merge REJECTED (would be a cycle).
    let (state2, a2, b2) = setup(true);
    let r2 = state2.execute(Command::MergeStations {
        station_a: StationRef::new(a2, 1),
        station_b: StationRef::new(b2, 0),
    });
    assert!(
        !r2.success,
        "merge across an aligned closure must be rejected (rigid cycle - use a loop closure)"
    );
}

#[test]
fn restore_snapshot_into_rebuilds_transient_caches() {
    // The single restore chokepoint must rebuild the serde-skipped section index,
    // so no rollback/undo/redo path can install a project that silently
    // mis-resolves a section (catalog #14/#39). The merge adjacency is not a
    // field here - it is derived on demand - so the restored project must answer
    // merge-group queries correctly with no rebuild call at all.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 2),
                station_b: StationRef::new(b, 0),
            })
            .success
    );

    let snapshot = match state.capture_project_snapshot() {
        Ok(s) => s,
        Err(e) => panic!("snapshot failed: {e:?}"),
    };

    // Deserialize into a fresh slot via the chokepoint; serde drops the caches, so
    // restore_snapshot_into must repopulate them.
    let mut slot: Option<Project> = None;
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));
    let p = slot.expect("restored");
    assert!(
        !p.section_index_cache.is_empty(),
        "section index cache must be rebuilt"
    );
    let merged = p.get_merged_section_ids(a);
    assert_eq!(
        merged,
        HashSet::from([a, b]),
        "a restored project must resolve the merge-connected component from its \
         own merges, with no rebuild step: {merged:?}"
    );
    // Sanity: the rebuilt index resolves both sections.
    assert!(p.get_section(a).is_some() && p.get_section(b).is_some());
}

/// A snapshot restore must return the project BIT for BIT, not to within a
/// printed decimal.
///
/// `serde_json`'s default float parser is best-effort: it writes
/// `-26.759999999999998` faithfully and reads it back as `-26.76`, one ULP away.
/// Every snapshot-pair command (Reverse section, Delete shot, Delete section,
/// every Batch import) stores a whole serialized `Project` and undoes by parsing
/// it back through [`AppState::restore_snapshot_into`], so with the default
/// parser an undo silently rewrote survey measurements in their last bit. It is
/// not cosmetic: the closure-adjusted view is a least-squares solve whose
/// elimination order turns on scale-relative pivot tests, so a one-ULP input
/// change moved a station 28 microns while the stored project compared equal
/// (issue #413, finding 25). `serde_json`'s `float_roundtrip` feature is what
/// makes the parse exact, and this test is the guard on that feature staying on.
#[test]
fn a_snapshot_restore_is_bit_exact() {
    // Doubles whose shortest round-trip decimal the best-effort parser gets
    // wrong. `pitch_out` is the one issue #413 hit; the others are on the same
    // ULP boundary and cover the plain (non-optional) measurement fields.
    let pitch = -26.759_999_999_999_998_f64;
    let length = 26.759_999_999_999_998_f64;
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "bits".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![ShotData {
            azimuth_in: 12.0,
            azimuth_out: 12.0,
            length,
            depth_in: 1.0,
            depth_out: 2.0,
            pitch_out: Some(pitch),
            ..Default::default()
        }],
    });
    let before = state.get_project_state().unwrap();
    assert_eq!(
        before.sections[0].shots[0].pitch_out.map(f64::to_bits),
        Some(pitch.to_bits()),
        "fixture: the shot must hold the exact double under test"
    );

    let snapshot = AppState::serialize_project_snapshot(&before).unwrap();
    let mut slot = Some(before);
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));
    let after = slot.unwrap();

    assert_eq!(
        after.sections[0].shots[0].pitch_out.map(f64::to_bits),
        Some(pitch.to_bits()),
        "restoring a snapshot changed pitch_out by {} ULP",
        (after.sections[0].shots[0].pitch_out.unwrap().to_bits() as i64 - pitch.to_bits() as i64)
            .abs()
    );
    assert_eq!(
        after.sections[0].shots[0].length.to_bits(),
        length.to_bits(),
        "restoring a snapshot changed length in its last bits"
    );
}

/// A snapshot persisted by a build predating #431/#448 has no
/// "surface_elevation" key at all, so serde's #[serde(default)] would
/// otherwise silently restore it to None. Mirrors the `validation`
/// legacy-carry-over immediately above `restore_snapshot_into`.
#[test]
fn restore_snapshot_carries_current_surface_elevation_over_a_legacy_blob() {
    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let mut legacy_json = serde_json::to_value(&project).unwrap();
    legacy_json
        .as_object_mut()
        .unwrap()
        .remove("surface_elevation");
    let snapshot = serde_json::to_string(&legacy_json).unwrap();
    assert!(
        !snapshot.contains("\"surface_elevation\":"),
        "precondition: the fixture blob must look pre-#431/#448"
    );

    let mut slot = Some(project.clone());
    slot.as_mut().unwrap().surface_elevation = Some(123.5);
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));

    assert_eq!(
        slot.unwrap().surface_elevation,
        Some(123.5),
        "restoring a legacy snapshot must carry the CURRENT surface_elevation forward, not drop it to None"
    );
}

/// The inverse of the legacy-carry-over above: a MODERN snapshot that
/// genuinely serializes `"surface_elevation":null` (an explicit clear, not an
/// absent key) must restore `None`, not the pre-undo project's `Some` value.
/// `surface_elevation` has no `skip_serializing_if`, so this is safe today -
/// but nothing guarded it, and the legacy-carry-over above reads similar
/// enough on a diff to invite a regression that carries the current value
/// forward unconditionally.
#[test]
fn restore_snapshot_clears_surface_elevation_when_the_modern_snapshot_says_none() {
    let mut project = Project::new("Cleared".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    project.surface_elevation = None;
    let snapshot = AppState::serialize_project_snapshot(&project).unwrap();
    assert!(
        snapshot.contains("\"surface_elevation\":null"),
        "precondition: a modern snapshot must serialize the explicit null, not omit the key: {snapshot}"
    );

    let mut slot = Some(project);
    slot.as_mut().unwrap().surface_elevation = Some(123.5);
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));

    assert_eq!(
        slot.unwrap().surface_elevation,
        None,
        "a modern snapshot's explicit null must clear surface_elevation, not carry the pre-undo value forward"
    );
}

#[test]
fn undo_unmerge_restores_converted_closure_and_position() {
    // The orphan-closure conversion unmerge triggers is a structural side-effect;
    // undo must reverse it completely - the re-created merge AND the
    // closure-that-became-a-merge AND the original section position must all come
    // back, with no leftover merge cycle. (User report: undoing the unmerge left
    // the converted merge in place and the geometry shifted.)
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });

    // Snapshot the pre-unmerge state we expect undo to restore exactly.
    let before = state.get_project_state().unwrap();
    let offset_before = before.section_offset(b);
    let merges_before = before.station_merges.len();
    assert_eq!(before.loop_closures.len(), 1);

    // Unmerge → closure becomes a merge, section B repositions onto A0.
    let r = state.execute(Command::UnmergeStationPair {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    let converted = state.get_project_state().unwrap();
    assert!(
        converted.loop_closures.is_empty(),
        "closure should have converted to a merge"
    );
    let offset_converted = converted.section_offset(b);
    assert!(
        (offset_converted.0 - offset_before.0).abs() > 1e-9
            || (offset_converted.1 - offset_before.1).abs() > 1e-9,
        "precondition: conversion must actually move B, else the position check is vacuous"
    );

    // Undo → merge, closure, and position all come back.
    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = state.get_project_state().unwrap();
    assert_eq!(
        restored.loop_closures.len(),
        1,
        "undo must turn the converted merge back into a closure"
    );
    assert_eq!(
        restored.station_merges.len(),
        merges_before,
        "undo must restore exactly the original merge - no leftover cycle"
    );
    assert!(
        restored.check_orphan_closures().is_empty(),
        "with the merge restored the closure is no longer a sole bridge"
    );
    assert!(
        !restored.has_critical_integrity_violations(),
        "restored state must be structurally clean"
    );
    let offset_restored = restored.section_offset(b);
    assert!(
        (offset_restored.0 - offset_before.0).abs() < 1e-9
            && (offset_restored.1 - offset_before.1).abs() < 1e-9,
        "undo must reset section position: before={offset_before:?} after={offset_restored:?}"
    );

    // Redo → back to the converted state.
    let re = state.redo();
    assert!(re.success, "{:?}", re.error);
    let redone = state.get_project_state().unwrap();
    assert!(
        redone.loop_closures.is_empty(),
        "redo must re-apply the closure→merge conversion"
    );
    assert!(redone.check_orphan_closures().is_empty());
    let offset_redone = redone.section_offset(b);
    assert!(
        (offset_redone.0 - offset_converted.0).abs() < 1e-9
            && (offset_redone.1 - offset_converted.1).abs() < 1e-9,
        "redo must restore the converted position"
    );
}

#[test]
fn undo_unmerge_singular_restores_converted_closure_and_position() {
    // Same round-trip as the pair variant, but via the singular Command::UnmergeStations
    // (which removes ALL merges on a station). It shares the conversion + snapshot path,
    // so this guards against the two unmerge variants diverging.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });

    let offset_before = state.get_project_state().unwrap().section_offset(b);

    // Singular unmerge of the merged station strands B onto the closure.
    let r = state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(a, 2),
    });
    assert!(r.success, "{:?}", r.error);
    let converted = state.get_project_state().unwrap();
    assert!(
        converted.loop_closures.is_empty(),
        "closure should have converted to a merge"
    );

    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = state.get_project_state().unwrap();
    assert_eq!(
        restored.loop_closures.len(),
        1,
        "undo reverts the converted merge to a closure"
    );
    assert!(restored.check_orphan_closures().is_empty());
    assert!(!restored.has_critical_integrity_violations());
    let offset_restored = restored.section_offset(b);
    assert!(
        (offset_restored.0 - offset_before.0).abs() < 1e-9
            && (offset_restored.1 - offset_before.1).abs() < 1e-9,
        "undo must reset position: before={offset_before:?} after={offset_restored:?}"
    );
}

#[test]
fn undo_merge_restores_aligned_closure_section_outside_merge_groups() {
    // A merge re-anchors not just the two merge groups but any section reachable
    // through an ALIGNED loop closure (cascade phase 2). The old lightweight MergeInfo
    // undo captured only the two groups' offsets, so such an external section stayed
    // drifted on undo - and realign_merges() can't self-heal closure drift (it inspects
    // merges only). With the full-snapshot undo, undo must restore EVERY section the
    // cascade could touch.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let z = state
        .execute(Command::CreateSection {
            name: Some("Z".to_string()),
            surveyor: None,
            date: None,
            color: "#333333".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    // Anchor A to GPS so the merge moves the B side (and, via the aligned closure, Z).
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, 90.0, 0.0).unwrap(),
    });

    // Inject an ALIGNED loop closure B.1 <-> Z.0 (no merge). Z is reachable from the
    // rest only through this aligned closure, so the cascade re-anchors it on merge.
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(b, 1).into(),
            station_b: StationRef::new(z, 0).into(),
            path_length: 10.0,
            misclosure_distance: 0.1,
            error_percentage: 1.0,
            horizontal_misclosure: 0.08,
            vertical_misclosure: 0.06,
            strategy: ClosureStrategy::default(),
            is_aligned: true,
        });
        project.next_closure_id += 1;
    }

    let z_before = state.get_project_state().unwrap().section_offset(z);

    // Merge A.1 <-> B.0: A holds the datum, so B (and Z via the aligned closure) move.
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);

    let z_merged = state.get_project_state().unwrap().section_offset(z);
    assert!(
        (z_merged.0 - z_before.0).abs() > 1e-6 || (z_merged.1 - z_before.1).abs() > 1e-6,
        "precondition: the merge cascade must re-anchor Z (else vacuous): \
         before={z_before:?} merged={z_merged:?}"
    );

    // Undo must restore Z exactly - not just the two merge groups.
    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let z_restored = state.get_project_state().unwrap().section_offset(z);
    assert!(
        (z_restored.0 - z_before.0).abs() < 1e-9 && (z_restored.1 - z_before.1).abs() < 1e-9,
        "undo must restore the aligned-closure section outside the merge groups: \
         before={z_before:?} after_undo={z_restored:?}"
    );
}

#[test]
fn undo_move_station_restores_pitch_and_adjacent_shots() {
    // MoveStation rewrites the two adjacent shots' raw geometry including pitch.
    // The old lightweight StationMoveInfo undo restored azimuth/length/depth but
    // NOT pitch_in/pitch_out, so undo left pitch corrupted. Full-snapshot undo must
    // restore every field.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    // test_shot_data leaves pitch unset (None). Capture the adjacent shots' state.
    let before = state.get_project_state().unwrap();
    let sec_before = before.get_section(a).unwrap();
    let shot0_before = sec_before.shots[0].clone();
    let shot1_before = sec_before.shots[1].clone();
    assert_eq!(shot0_before.pitch_in, None);
    assert_eq!(shot1_before.pitch_in, None);

    // Move station 1 (the boundary between shot 0 and shot 1).
    let r = state.execute(Command::MoveStation {
        station_ref: StationRef::new(a, 1),
        offset: Coordinates::new(2.0, 3.0, 1.5),
    });
    assert!(r.success, "{:?}", r.error);

    // Precondition: the move set pitch on the adjacent shots (so undo has work to do).
    let moved = state.get_project_state().unwrap();
    let sec_moved = moved.get_section(a).unwrap();
    assert!(
        sec_moved.shots[0].pitch_in.is_some() || sec_moved.shots[1].pitch_in.is_some(),
        "precondition: the move must set pitch on an adjacent shot"
    );

    // Undo must restore pitch (the bug) and every other field exactly.
    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = state.get_project_state().unwrap();
    let sec_r = restored.get_section(a).unwrap();
    assert_eq!(
        sec_r.shots[0].pitch_in, shot0_before.pitch_in,
        "pitch_in[0] restored"
    );
    assert_eq!(
        sec_r.shots[0].pitch_out, shot0_before.pitch_out,
        "pitch_out[0] restored"
    );
    assert_eq!(
        sec_r.shots[1].pitch_in, shot1_before.pitch_in,
        "pitch_in[1] restored"
    );
    assert_eq!(
        sec_r.shots[1].pitch_out, shot1_before.pitch_out,
        "pitch_out[1] restored"
    );
    assert!(
        (sec_r.shots[0].length - shot0_before.length).abs() < 1e-9,
        "length[0] restored"
    );
    assert!(
        (sec_r.shots[1].depth_in - shot1_before.depth_in).abs() < 1e-9,
        "depth_in[1] restored"
    );
    assert!(
        (sec_r.shots[0].azimuth_in - shot0_before.azimuth_in).abs() < 1e-9,
        "azimuth_in[0] restored"
    );
}

#[test]
fn split_converts_orphaned_bridge_closure_immediately() {
    // The split path bypasses execute()'s reconcile; confirm it still converts a
    // closure that the split strands as a section's sole bridge (the reported
    // "not transformed immediately on split" bug).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    // Section A: 3 shots → stations 0..3 (so we can split at the middle shot).
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(5.0),
                test_shot_data(5.0),
            ],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // B hangs off A's last station; a closure A0=B1 closes the loop.
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 0),
    });
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closure.success, "{:?}", closure.error);
    assert_eq!(state.get_project_state().unwrap().loop_closures.len(), 1);

    // Split A at its MIDDLE shot - the loop's path through A breaks, leaving the
    // closure as the only connection between A's first half and B.
    let mid = state.get_project_state().unwrap().sections[0].shots[1].id;
    let r = state.execute_delete_shot_and_split(a, mid, "A-1".to_string(), "A-2".to_string());
    assert!(r.success, "{:?}", r.error);

    let p = state.get_project_state().unwrap();
    assert!(
        p.loop_closures.is_empty(),
        "split should immediately convert the orphaned closure to a merge: {:?}",
        p.loop_closures
    );
    assert!(p.check_orphan_closures().is_empty());
}

/// A level shot on a fixed heading. Keeps the split/merge topology regressions
/// below easy to reason about: every section is a straight, flat run.
fn heading_shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: (azimuth + 180.0) % 360.0,
        length,
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    }
}

#[test]
fn split_section_does_not_promote_a_bridge_closure_across_two_anchored_components() {
    // The split rewrites every cross-reference through `map_station_refs`, which
    // can move a MERGE endpoint into the newly created section. The merge-only
    // adjacency the double-anchor guard consults must reflect that immediately:
    // answering from the pre-split shape reports the moved section as unanchored,
    // and `convert_orphan_closures_to_merges` then promotes a legitimate bridge
    // closure into a rigid merge - fusing two independently GPS-anchored
    // components ("Merged stations X and Y are N m apart" + "Merge-connected group
    // has 2 zero datums").
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Bridge".to_string(),
        location: None,
    });
    let mk = |name: &str, color: &str, azimuth: f64| {
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: color.to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![
                    heading_shot(azimuth, 5.0),
                    heading_shot(azimuth, 5.0),
                    heading_shot(azimuth, 5.0),
                ],
            })
            .section_id
            .unwrap()
    };
    // Three straight 3-shot runs (stations 0..3 each).
    let a = mk("A", "#111111", 0.0);
    let b = mk("B", "#222222", 90.0);
    let c = mk("C", "#333333", 0.0);

    // A is GPS-anchored.
    let r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(r.success, "{:?}", r.error);

    // B hangs rigidly off A by its LAST station, so the split below moves that
    // merge endpoint into the new section.
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 3),
    });
    assert!(r.success, "{:?}", r.error);

    // C is a second, independently GPS-anchored survey.
    let r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(c, 0),
        coordinates: GpsCoordinates::new(10.001, 20.0, 0.0).unwrap(),
    });
    assert!(r.success, "{:?}", r.error);

    // The ONLY link between the two anchored components: a soft loop closure.
    // Legitimate as a closure, illegal as a merge.
    let r = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(b, 2),
        station_b: StationRef::new(c, 0),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(r.success, "{:?}", r.error);

    // Split B at its middle shot: stations 2 (closure endpoint) and 3 (merge
    // endpoint) both move into the new section.
    let mid = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == b)
        .expect("section B")
        .shots[1]
        .id;
    let r = state.execute_delete_shot_and_split(b, mid, "B-1".to_string(), "B-2".to_string());
    assert!(r.success, "{:?}", r.error);

    let p = state.get_project_state().unwrap();
    assert_eq!(
        p.loop_closures.len(),
        1,
        "the bridge closure must survive the split as a closure: {:?}",
        p.loop_closures
    );
    assert_eq!(
        p.station_merges.len(),
        1,
        "only the original A-B merge may exist; the split must not promote the bridge closure: {:?}",
        p.station_merges
    );
    assert!(
        p.station_merges
            .iter()
            .all(|m| m.station_a.section_id != c && m.station_b.section_id != c),
        "no merge may bridge into the second anchored component: {:?}",
        p.station_merges
    );
    assert!(
        p.check_merge_alignment().is_empty(),
        "a merge fusing two anchored components tears apart: {:?}",
        p.check_merge_alignment()
    );
    assert!(
        p.over_anchored_merge_components().is_empty(),
        "the two datums must stay in separate merge components: {:?}",
        p.over_anchored_merge_components()
    );
}

#[test]
fn create_section_rejects_non_finite_geometry() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let mut bad = test_shot_data(5.0);
    bad.length = f64::NAN;
    let r = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ffffff".to_string(),
        direction: None,
        shots: vec![bad],
    });
    assert!(!r.success, "create_section must reject non-finite geometry");
    assert_eq!(
        state.get_project_state().unwrap().sections.len(),
        0,
        "nothing committed"
    );
}

#[test]
fn move_station_rejects_non_finite_offset() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let r = state.execute(Command::MoveStation {
        station_ref: StationRef::new(a, 1),
        offset: Coordinates::new(f64::INFINITY, 0.0, 0.0),
    });
    assert!(!r.success, "move_station must reject a non-finite offset");
    let p = state.get_project_state().unwrap();
    assert!(
        p.sections[0].shots[0].all_finite(),
        "shot geometry left untouched"
    );
}

#[test]
fn redo_insert_shot_keeps_merged_stations_aligned() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Align".to_string(),
        location: None,
    });

    // Section A (id 0): two shots → stations A0, A1, A2.
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
        })
        .section_id
        .unwrap();
    // Section B (id 1): one shot → stations B0, B1.
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    // Anchor A at a zero datum, then merge B0 onto A2 (B hangs off A's tail).
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    let merge = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge.success, "{:?}", merge.error);

    let aligned = |state: &AppState| {
        state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty()
    };
    assert!(aligned(&state), "merge should start aligned");

    // Insert a shot after A's first shot (between A1 and A2): shifts the tail and
    // the merge point. The forward path cascades B back into alignment.
    let after_id = state.get_project_state().unwrap().sections[0].shots[0].id;
    let ins = state.execute(Command::CreateShot {
        section_id: a,
        shot: test_shot_data(6.0),
        after_shot_id: Some(after_id),
        before_shot_id: None,
    });
    assert!(ins.success, "{:?}", ins.error);
    assert!(aligned(&state), "forward insert should stay aligned");

    // Undo (already correct) then redo - redo used to omit the offset cascade and
    // leave B drifted tens of metres. The post-history reconcile must prevent it.
    assert!(state.undo().success);
    assert!(aligned(&state), "undo should stay aligned");
    assert!(state.redo().success);
    assert!(aligned(&state), "redo must not desync the merge geometry");
}

#[test]
fn test_create_section_returns_id() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::CreateSection {
        name: Some("First".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    assert!(result.success);
    assert_eq!(result.section_id, Some(0));

    // Create another section
    let result2 = state.execute(Command::CreateSection {
        name: Some("Second".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    assert!(result2.success);
    assert_eq!(result2.section_id, Some(1));
}

#[test]
fn test_create_section_no_project() {
    let state = AppState::new().unwrap();

    let result = state.execute(Command::CreateSection {
        name: Some("Orphan".to_string()),
        surveyor: None,
        date: None,
        color: "#000000".to_string(),
        direction: None,
        shots: vec![],
    });

    assert!(!result.success);
    assert!(result.error.is_some());
}

#[test]
fn test_delete_section() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("To Delete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });

    let section_id = create_result.section_id.unwrap();

    let result = state.execute(Command::DeleteSection { section_id });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert!(project.sections.is_empty());
}

#[test]
fn test_delete_nonexistent_section() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::DeleteSection { section_id: 999 });
    assert!(!result.success);
}

#[test]
fn test_update_section_metadata() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    let section_id = create_result.section_id.unwrap();

    let result = state.execute(Command::UpdateSectionMetadata {
        section_id,
        updates: SectionUpdates {
            name: Some(Some("Updated".to_string())),
            color: Some("#00ff00".to_string()),
            ..Default::default()
        },
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].name, Some("Updated".to_string()));
    assert_eq!(project.sections[0].color, "#00ff00");
}

// === Shot Operations Tests ===

#[test]
fn test_create_shot() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    let section_id = create_result.section_id.unwrap();

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(10.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 1);
    assert_eq!(project.sections[0].shots[0].length, 10.0);
}

#[test]
fn test_update_shot() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(15.0),
            comment: Some(Some("Updated shot".to_string())),
            ..Default::default()
        },
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots[0].length, 15.0);
    assert_eq!(
        project.sections[0].shots[0].comment,
        Some("Updated shot".to_string())
    );
}

#[test]
fn test_delete_shot() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    let result = state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 1);
}

#[test]
fn test_delete_first_shot_preserves_station_positions() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project_before = state.get_project_state().unwrap();
    let station_1_before = project_before
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    let station_2_before = project_before
        .compute_station_raw_coords(StationRef::new(section_id, 2))
        .unwrap();

    let first_shot_id = project_before.sections[0].shots[0].id;
    let delete_result = state.execute(Command::DeleteShot {
        section_id,
        shot_id: first_shot_id,
    });
    assert!(delete_result.success);

    let project_after = state.get_project_state().unwrap();
    let station_0_after = project_after
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    let station_1_after = project_after
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();

    let eps = 1e-6;
    // X,Y positions preserved: station_0_after matches station_1_before in X,Y
    assert!(
        (station_0_after.x - station_1_before.x).abs() < eps,
        "x mismatch after delete"
    );
    assert!(
        (station_0_after.y - station_1_before.y).abs() < eps,
        "y mismatch after delete"
    );
    assert!(
        (station_1_after.x - station_2_before.x).abs() < eps,
        "x mismatch after delete"
    );
    assert!(
        (station_1_after.y - station_2_before.y).abs() < eps,
        "y mismatch after delete"
    );
    // Z is absolute from shot depth measurements - after deleting first shot,
    // remaining stations get Z from the remaining shot's absolute depths.
    // Remaining shot: depth_in=10, depth_out=11
    assert!(
        (station_0_after.z - 10.0).abs() < eps,
        "z station 0 after delete should be depth_in of remaining shot"
    );
    assert!(
        (station_1_after.z - 11.0).abs() < eps,
        "z station 1 after delete should be depth_out of remaining shot"
    );
}

#[test]
fn test_delete_shot_rewires_merges() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(8.0)],
        })
        .section_id
        .unwrap();

    let merge_ab = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 0),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_ab.success);

    let merge_ac = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 0),
        station_b: StationRef::new(sec_c, 0),
    });
    assert!(merge_ac.success);

    let project_before = state.get_project_state().unwrap();
    let first_shot_id = project_before
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;

    let delete_result = state.execute(Command::DeleteShot {
        section_id: sec_a,
        shot_id: first_shot_id,
    });
    assert!(delete_result.success);

    let project_after = state.get_project_state().unwrap();
    let has_merge_bc = project_after.station_merges.iter().any(|m| {
        (project_after.key_is_ref(&m.station_a, StationRef::new(sec_b, 0))
            && project_after.key_is_ref(&m.station_b, StationRef::new(sec_c, 0)))
            || (project_after.key_is_ref(&m.station_a, StationRef::new(sec_c, 0))
                && project_after.key_is_ref(&m.station_b, StationRef::new(sec_b, 0)))
    });
    assert!(
        has_merge_bc,
        "Expected replacement merge between B:0 and C:0"
    );
}

#[test]
fn test_delete_shot_rejects_frozen_shot() {
    // Issue #57 (Item 3): a frozen shot (geometry anchored to
    // an adjusted survey network) must not be deletable via the resolve flow.
    // Build a two-section loop with a datum at each end so every shot between the
    // datums is frozen, then assert DeleteShot is refused and the shot survives.
    //
    // The loop topology (a:0<->b:0 and a:2<->b:2, two datums) is built in memory
    // directly - the command-level MergeStations validation refuses merging two
    // stations already in the same connected component, which the second
    // end-to-end merge would be. The real delete_shot command guard is still
    // exercised below.
    let mut project = Project::new("Frozen".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
            ..Default::default()
        })
        .id;

    // Close the loop: both ends of A merged onto both ends of B.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 0).into(),
        station_b: StationRef::new(sec_b, 0).into(),
        merged_at: chrono::Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 2).into(),
        station_b: StationRef::new(sec_b, 2).into(),
        merged_at: chrono::Utc::now(),
    });
    // Two datums, one per section head, freeze every shot in the loop.
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_a, 0).into(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_b, 0).into(),
        gps: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
    });

    // SELF-VERIFY the chosen shot is genuinely frozen before asserting the
    // rejection, so the test fails loudly if the setup didn't freeze it.
    let frozen = project.compute_frozen_shots();
    assert!(
        frozen
            .iter()
            .any(|f| f.section_id == sec_a && f.shot_index == 0),
        "precondition: sec_a shot[0] must be frozen by the loop+two-datum setup"
    );
    let target_shot_id = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;
    let shots_before = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .len();

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    // The delete must be refused with a clear frozen message.
    let result = state.execute(Command::DeleteShot {
        section_id: sec_a,
        shot_id: target_shot_id,
    });
    assert!(!result.success, "deleting a frozen shot must be refused");
    assert!(
        result.error.as_deref().unwrap_or("").contains("frozen"),
        "error must mention the shot is frozen, got: {:?}",
        result.error
    );

    // The shot still exists - nothing was removed.
    let after = state.get_project_state().unwrap();
    let shots_after = after
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .len();
    assert_eq!(
        shots_after, shots_before,
        "the frozen shot must still exist"
    );
}

#[test]
fn test_delete_shot_and_split_rejects_frozen_shot() {
    // #241 (P0): the single-shot delete+split path (execute_delete_shot_and_split)
    // must enforce the same frozen-geometry guard as delete_shot and the range
    // split/shift paths. Without it, a frozen MID-section shot can be deleted
    // through the split, silently breaking an LSQ-adjusted network. Build a
    // two-section loop with a datum at each end so every shot is frozen, then
    // split at a genuine INTERIOR shot (so it clears the edge-shot pre-check and
    // reaches the frozen guard) and assert it is refused with nothing changed.
    let mut project = Project::new("FrozenSplit".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(7.0),
                test_shot_data(4.0),
            ],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(7.0),
                test_shot_data(4.0),
            ],
            ..Default::default()
        })
        .id;

    // Close the loop: both ends of A merged onto both ends of B (3 shots => the
    // section tail is station index 3).
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 0).into(),
        station_b: StationRef::new(sec_b, 0).into(),
        merged_at: chrono::Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 3).into(),
        station_b: StationRef::new(sec_b, 3).into(),
        merged_at: chrono::Utc::now(),
    });
    // Two datums, one per section head, freeze every shot in the loop.
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_a, 0).into(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_b, 0).into(),
        gps: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
    });

    // SELF-VERIFY the interior split shot (index 1) is genuinely frozen.
    let frozen = project.compute_frozen_shots();
    assert!(
        frozen
            .iter()
            .any(|f| f.section_id == sec_a && f.shot_index == 1),
        "precondition: sec_a interior shot[1] must be frozen by the loop+two-datum setup"
    );

    let sec_a_shots = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .clone();
    let mid_shot_id = sec_a_shots[1].id;
    let shots_before = sec_a_shots.len();
    let sections_before = project.sections.len();

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_delete_shot_and_split(
        sec_a,
        mid_shot_id,
        "A-1".to_string(),
        "A-2".to_string(),
    );
    assert!(
        !result.success,
        "splitting by deleting a frozen shot must be refused"
    );
    assert!(
        result.error.as_deref().unwrap_or("").contains("frozen"),
        "error must mention the shot is frozen, got: {:?}",
        result.error
    );

    // Nothing changed: no new section, the frozen shot still exists.
    let after = state.get_project_state().unwrap();
    assert_eq!(
        after.sections.len(),
        sections_before,
        "a refused split must not create a new section"
    );
    let shots_after = after
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .len();
    assert_eq!(
        shots_after, shots_before,
        "the frozen shot must still exist"
    );
}

// === split_shot_at_distance tests (#59: insert a station mid-shot) ===

#[test]
fn test_split_shot_at_distance_happy_path() {
    let mut project = Project::new("Split".to_string(), None);
    let sec = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![
                test_shot_data(5.0),
                ShotData {
                    uuid: None,
                    azimuth_in: 30.0,
                    azimuth_out: 32.0,
                    length: 10.0,
                    depth_in: 10.0,
                    depth_out: 20.0,
                    pitch_in: Some(10.0),
                    pitch_out: Some(-5.0),
                    left: Some(1.0),
                    right: Some(2.0),
                    up: Some(0.5),
                    down: Some(0.25),
                    comment: Some("mid comment".to_string()),
                    kind: ShotKind::Normal,
                    marker_index: Some(3),
                    temperature: Some(15.0),
                    credit: None,
                },
                test_shot_data(5.0),
            ],
            ..Default::default()
        })
        .id;

    let target = project.sections[0].shots[1].clone();
    let target_shot_id = target.id;
    let target_uuid = target.uuid;

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_split_shot_at_distance(sec, target_shot_id, 4.0);
    assert!(result.success, "{:?}", result.error);
    let new_shot_id = result.shot_id.expect("new shot id");

    let after = state.get_project_state().unwrap();
    let section = after.get_section(sec).unwrap();
    assert_eq!(section.shots.len(), 4);

    let head = &section.shots[1];
    let tail = &section.shots[2];

    assert_eq!(head.id, new_shot_id);
    assert_ne!(
        head.uuid, target_uuid,
        "the head shot must get a fresh uuid"
    );
    assert_eq!(head.length, 4.0);
    assert_eq!(head.depth_in, 10.0);
    assert!(
        (head.depth_out - 14.0).abs() < 1e-9,
        "mid depth interpolated"
    );
    assert_eq!(head.azimuth_in, 30.0);
    assert_eq!(head.azimuth_out, 32.0);
    assert_eq!(head.pitch_in, Some(10.0));
    assert_eq!(head.pitch_out, Some(-5.0));
    assert_eq!(head.left, Some(1.0));
    assert_eq!(head.right, Some(2.0));
    assert_eq!(head.up, Some(0.5));
    assert_eq!(head.down, Some(0.25));
    assert_eq!(head.comment, None);
    assert_eq!(head.marker_index, None);
    assert_eq!(head.temperature, Some(15.0));
    assert!(!head.kind.is_virtual());

    assert_eq!(
        tail.id, target_shot_id,
        "the tail keeps the original shot id"
    );
    assert_eq!(tail.uuid, target_uuid, "the tail keeps the original uuid");
    assert_eq!(tail.length, 6.0);
    assert!((tail.depth_in - 14.0).abs() < 1e-9);
    assert_eq!(tail.depth_out, 20.0);
    assert_eq!(tail.comment, Some("mid comment".to_string()));
    assert_eq!(tail.marker_index, Some(3));

    // Both children pin the parent shot's LRUD unchanged (a deliberate
    // mid-station estimate -- see the split_shot_at_distance docstring).
    assert_eq!(head.left, tail.left, "head LRUD must equal tail LRUD");
    assert_eq!(head.right, tail.right, "head LRUD must equal tail LRUD");
    assert_eq!(head.up, tail.up, "head LRUD must equal tail LRUD");
    assert_eq!(head.down, tail.down, "head LRUD must equal tail LRUD");
    assert_eq!(
        (head.left, head.right, head.up, head.down),
        (Some(1.0), Some(2.0), Some(0.5), Some(0.25)),
        "head/tail LRUD must equal the parent shot's LRUD"
    );
}

#[test]
fn test_split_shot_at_distance_preserves_anchor_on_endpoint_station() {
    // A cross-reference anchored on the parent shot's ENDPOINT station (the
    // boundary right after it) must resolve to the shifted-by-one endpoint
    // after the split, NOT to the new mid station -- the original shot's uuid
    // staying on the tail child is what makes this automatic.
    // sec_a's first two (flat, opposite-azimuth, equal-length) shots cancel
    // out, so station A:2 lands back at the section's own origin (0,0,0) --
    // matching sec_b's station B:0 (always at its own origin) without needing
    // any offset-alignment machinery, so the merge is geometrically valid.
    let mut project = Project::new("Anchor".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 3.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 180.0,
                    azimuth_out: 180.0,
                    length: 3.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                },
                test_shot_data(5.0),
            ],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![test_shot_data(5.0)],
            ..Default::default()
        })
        .id;

    let target_shot_id = project.sections[0].shots[1].id;
    let endpoint_key = project.key_for_ref(StationRef::new(sec_a, 2)).unwrap();
    project.station_merges.push(StationMerge {
        station_a: endpoint_key.clone(),
        station_b: project.key_for_ref(StationRef::new(sec_b, 0)).unwrap(),
        merged_at: chrono::Utc::now(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: endpoint_key.clone(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    project
        .station_comments
        .insert(endpoint_key, "endpoint note".to_string());

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_split_shot_at_distance(sec_a, target_shot_id, 1.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();

    let merge = after
        .station_merges
        .iter()
        .find(|m| {
            after.key_is_ref(&m.station_a, StationRef::new(sec_b, 0))
                || after.key_is_ref(&m.station_b, StationRef::new(sec_b, 0))
        })
        .expect("merge survives the split");
    let our_key = if after.key_is_ref(&merge.station_a, StationRef::new(sec_b, 0)) {
        &merge.station_b
    } else {
        &merge.station_a
    };
    assert_eq!(
        after.resolve_key(our_key),
        Some(StationRef::new(sec_a, 3)),
        "the endpoint merge must resolve to index p+2, not the new mid station"
    );

    assert_eq!(after.zero_datums.len(), 1);
    assert_eq!(
        after.resolve_key(&after.zero_datums[0].station_ref),
        Some(StationRef::new(sec_a, 3))
    );

    assert_eq!(after.station_comments.len(), 1);
    let (comment_key, comment) = after.station_comments.iter().next().unwrap();
    assert_eq!(comment, "endpoint note");
    assert_eq!(
        after.resolve_key(comment_key),
        Some(StationRef::new(sec_a, 3))
    );
}

#[test]
fn test_split_shot_at_distance_preserves_loop_closure_anchor_on_endpoint_station() {
    // Same shape as test_split_shot_at_distance_preserves_anchor_on_endpoint_station,
    // but for a LoopClosure anchored on the parent shot's endpoint station -- the
    // one anchor class that test doesn't cover. sec_a's first two (flat,
    // opposite-azimuth, equal-length) shots cancel out, so station A:2 lands
    // back at the section's own origin (0,0,0) -- a genuine loop back to A:0.
    let mut project = Project::new("ClosureAnchor".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 3.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 180.0,
                    azimuth_out: 180.0,
                    length: 3.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                },
                test_shot_data(5.0),
            ],
            ..Default::default()
        })
        .id;

    let target_shot_id = project.sections[0].shots[1].id;
    let start_key = project.key_for_ref(StationRef::new(sec_a, 0)).unwrap();
    let endpoint_key = project.key_for_ref(StationRef::new(sec_a, 2)).unwrap();
    project.loop_closures.push(LoopClosure {
        id: 0,
        station_a: endpoint_key,
        station_b: start_key,
        path_length: 6.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_split_shot_at_distance(sec_a, target_shot_id, 1.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    assert_eq!(after.loop_closures.len(), 1);
    let closure = &after.loop_closures[0];
    assert_eq!(
        after.resolve_key(&closure.station_a),
        Some(StationRef::new(sec_a, 3)),
        "the endpoint closure anchor must resolve to index p+2, not the new mid station"
    );
    assert_eq!(
        after.resolve_key(&closure.station_b),
        Some(StationRef::new(sec_a, 0)),
        "the closure's other endpoint (before the split point) is unaffected"
    );
}

#[test]
fn test_split_shot_at_distance_first_shot_keeps_section_start_anchor() {
    let mut project = Project::new("FirstSplit".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![test_shot_data(10.0), test_shot_data(5.0)],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![test_shot_data(5.0)],
            ..Default::default()
        })
        .id;
    let target_shot_id = project.sections[0].shots[0].id;

    let start_key = project.key_for_ref(StationRef::new(sec_a, 0)).unwrap();
    assert_eq!(start_key.anchor, StationAnchor::SectionStart);
    project.station_merges.push(StationMerge {
        station_a: start_key.clone(),
        station_b: project.key_for_ref(StationRef::new(sec_b, 0)).unwrap(),
        merged_at: chrono::Utc::now(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: start_key,
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_split_shot_at_distance(sec_a, target_shot_id, 4.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    assert_eq!(
        after.resolve_key(&after.zero_datums[0].station_ref),
        Some(StationRef::new(sec_a, 0)),
        "SectionStart-anchored datum must still resolve to station 0"
    );
    let merge = after
        .station_merges
        .iter()
        .find(|m| {
            after.key_is_ref(&m.station_a, StationRef::new(sec_b, 0))
                || after.key_is_ref(&m.station_b, StationRef::new(sec_b, 0))
        })
        .expect("merge survives");
    let our_key = if after.key_is_ref(&merge.station_a, StationRef::new(sec_b, 0)) {
        &merge.station_b
    } else {
        &merge.station_a
    };
    assert_eq!(after.resolve_key(our_key), Some(StationRef::new(sec_a, 0)));
}

#[test]
fn test_split_shot_at_distance_rejects_invalid_distances() {
    let mut project = Project::new("Invalid".to_string(), None);
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![test_shot_data(10.0)],
        ..Default::default()
    });
    let sec = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;
    let before_json = serde_json::to_string(&project).unwrap();

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    // 0.005 and length - 0.005 = 9.995 both fall under the MIN_SPLIT_CHILD_LENGTH_M
    // (0.01m) floor and must be rejected even though they are strictly between
    // 0 and the shot's length.
    for distance in [0.0, 10.0, -1.0, f64::NAN, f64::INFINITY, 11.0, 0.005, 9.995] {
        let result = state.execute_split_shot_at_distance(sec, shot_id, distance);
        assert!(!result.success, "distance {distance} must be rejected");
    }

    let after = state.get_project_state().unwrap();
    let after_json = serde_json::to_string(&after).unwrap();
    assert_eq!(
        before_json, after_json,
        "a rejected split must leave the project untouched"
    );
}

#[test]
fn test_split_shot_at_distance_accepts_exact_min_child_length_boundary() {
    // A 1m shot split exactly at the MIN_SPLIT_CHILD_LENGTH_M (0.01m) floor
    // must be accepted: both children end up exactly at the floor, not below it.
    let mut project = Project::new("Boundary".to_string(), None);
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let sec = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_split_shot_at_distance(sec, shot_id, MIN_SPLIT_CHILD_LENGTH_M);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    let section = after.get_section(sec).unwrap();
    assert_eq!(section.shots.len(), 2);
    assert!((section.shots[0].length - MIN_SPLIT_CHILD_LENGTH_M).abs() < 1e-12);
    assert!((section.shots[1].length - (1.0 - MIN_SPLIT_CHILD_LENGTH_M)).abs() < 1e-12);
}

#[test]
fn test_split_shot_at_distance_rejects_frozen_shot() {
    // Mirrors test_delete_shot_and_split_rejects_frozen_shot: a two-section
    // loop with a datum at each end freezes every shot in it.
    let mut project = Project::new("FrozenSplit".to_string(), None);
    let sec_a = project
        .add_section(SectionData {
            color: "#ff0000".to_string(),
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(7.0),
                test_shot_data(4.0),
            ],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            color: "#00ff00".to_string(),
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(7.0),
                test_shot_data(4.0),
            ],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 0).into(),
        station_b: StationRef::new(sec_b, 0).into(),
        merged_at: chrono::Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec_a, 3).into(),
        station_b: StationRef::new(sec_b, 3).into(),
        merged_at: chrono::Utc::now(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_a, 0).into(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    project.zero_datums.push(crate::models::ZeroDatum {
        station_ref: StationRef::new(sec_b, 0).into(),
        gps: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
    });

    let frozen = project.compute_frozen_shots();
    assert!(
        frozen
            .iter()
            .any(|f| f.section_id == sec_a && f.shot_index == 1),
        "precondition: sec_a interior shot[1] must be frozen"
    );
    let target_shot_id = project.sections[0].shots[1].id;
    let shots_before = project.sections[0].shots.len();

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_split_shot_at_distance(sec_a, target_shot_id, 3.0);
    assert!(!result.success, "splitting a frozen shot must be refused");
    assert!(
        result.error.as_deref().unwrap_or("").contains("frozen"),
        "error must mention the shot is frozen, got: {:?}",
        result.error
    );

    let after = state.get_project_state().unwrap();
    assert_eq!(
        after.get_section(sec_a).unwrap().shots.len(),
        shots_before,
        "the frozen shot must not be split"
    );
}

#[test]
fn test_split_shot_at_distance_undo_redo_round_trip() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Split Undo".to_string(),
        location: None,
    });
    let create_result = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(10.0),
            test_shot_data(5.0),
        ],
    });
    let section_id = create_result.section_id.unwrap();

    let before_json = {
        let proj = state.get_project_state().unwrap();
        serde_json::to_string(&proj).unwrap()
    };
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result = state.execute_split_shot_at_distance(section_id, shot_id, 4.0);
    assert!(result.success, "{:?}", result.error);
    let split_json = {
        let proj = state.get_project_state().unwrap();
        assert_eq!(proj.get_section(section_id).unwrap().shots.len(), 4);
        serde_json::to_string(&proj).unwrap()
    };

    let undo_result = state.undo();
    assert!(undo_result.success, "{:?}", undo_result.error);
    let after_undo = state.get_project_state().unwrap();
    assert_eq!(after_undo.get_section(section_id).unwrap().shots.len(), 3);
    assert_eq!(
        after_undo.get_section(section_id).unwrap().shots[1].id,
        shot_id,
        "undo restores the original single shot"
    );
    assert_eq!(
        serde_json::to_string(&after_undo).unwrap(),
        before_json,
        "undo must restore the exact pre-split project"
    );

    let redo_result = state.redo();
    assert!(redo_result.success, "{:?}", redo_result.error);
    let after_redo = state.get_project_state().unwrap();
    assert_eq!(
        serde_json::to_string(&after_redo).unwrap(),
        split_json,
        "redo must re-split identically"
    );
}

#[test]
fn test_split_shot_at_distance_preserves_downstream_geometry() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Geo".to_string(),
        location: None,
    });
    let create_result = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            ShotData {
                azimuth_in: 10.0,
                azimuth_out: 10.0,
                length: 5.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 50.0,
                azimuth_out: 50.0,
                length: 8.0,
                depth_in: 1.0,
                depth_out: 4.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 120.0,
                azimuth_out: 120.0,
                length: 6.0,
                depth_in: 4.0,
                depth_out: 2.0,
                ..Default::default()
            },
        ],
    });
    let section_id = create_result.section_id.unwrap();

    let before_offset = state
        .get_project_state()
        .unwrap()
        .section_offset(section_id);
    let before_coords = abs_coord_map(&state.get_project_state().unwrap());
    let start_before = before_coords[&StationRef::new(section_id, 0)];
    let last_before = before_coords[&StationRef::new(section_id, 3)];

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result = state.execute_split_shot_at_distance(section_id, shot_id, 3.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    assert_eq!(
        before_offset,
        after.section_offset(section_id),
        "a collinear split must not require a section-offset adjustment"
    );

    let after_coords = abs_coord_map(&after);
    assert_coords_close(start_before, after_coords[&StationRef::new(section_id, 0)]);
    // Station 3 shifts to index 4 (one more shot inserted before it); its
    // absolute position must be exactly preserved.
    assert_coords_close(last_before, after_coords[&StationRef::new(section_id, 4)]);
}

#[test]
fn test_split_shot_at_distance_reference_shot_both_children_reference() {
    let mut project = Project::new("Ref".to_string(), None);
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![ShotData {
            kind: ShotKind::Jump,
            ..test_shot_data(10.0)
        }],
        ..Default::default()
    });
    let sec = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_split_shot_at_distance(sec, shot_id, 4.0);
    assert!(result.success, "{:?}", result.error);

    let after = state.get_project_state().unwrap();
    let section = after.get_section(sec).unwrap();
    assert!(
        section.shots[0].kind.is_virtual(),
        "head must inherit the shot kind"
    );
    assert!(
        section.shots[1].kind.is_virtual(),
        "tail keeps its own kind"
    );
}

#[test]
fn test_split_shot_at_distance_rejects_at_max_shots_cap() {
    let mut project = Project::new("Cap".to_string(), None);
    let max = crate::models::FEATURE_ID_STRIDE as usize - 1;
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![test_shot_data(1.0); max],
        ..Default::default()
    });
    let sec = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.execute_split_shot_at_distance(sec, shot_id, 0.5);
    assert!(
        !result.success,
        "split must be refused when the section is at the max-shots cap"
    );
    assert!(
        result.error.as_deref().unwrap_or("").contains("maximum"),
        "error should mention the max-shots cap, got: {:?}",
        result.error
    );
}

#[test]
fn test_legacy_project_seeds_validation_from_global_config() {
    // Issue #57 (Item 5): a legacy project saved before per-project validation
    // (no `validation` row in project_settings) must open with the user's GLOBAL
    // thresholds - the "defaults for new projects" - not the hardcoded
    // ValidationConfig::default(). A project that DOES carry an explicit row keeps
    // its own thresholds.
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();

        // A 1.0 m shot is fine under the hardcoded default (warn_shot_length_min
        // = 0.3) but SHORT under a raised global minimum.
        let custom = crate::config::ValidationConfig {
            warn_shot_length_min: 5.0,
            ..crate::config::ValidationConfig::default()
        };

        // --- Legacy file: saved, then strip the validation row to simulate a
        //     project written before the feature existed. ---
        let legacy_path = dir.path().join("legacy.stalx");
        {
            let mut project = Project::new("Legacy".to_string(), None);
            project.add_section(SectionData {
                shots: vec![test_shot_data(1.0)],
                ..Default::default()
            });
            let mut db = Database::open(&legacy_path).unwrap();
            db.save_project(&project).unwrap();
            drop(db);
            let conn = rusqlite::Connection::open(&legacy_path).unwrap();
            conn.execute("DELETE FROM project_settings WHERE key = 'validation'", [])
                .unwrap();
            drop(conn);
        }
        // Sanity: the file now looks legacy (no stored validation row).
        assert!(
            Database::open_existing(&legacy_path)
                .unwrap()
                .load_project_validation()
                .unwrap()
                .is_none(),
            "precondition: legacy file has no stored validation row"
        );

        // Open via the production command path on a fresh AppState whose GLOBAL
        // config carries the customized threshold.
        let state = AppState::new().unwrap();
        let mut config = state.get_config();
        config.validation = custom.clone();
        state.update_config(config).unwrap();

        assert!(state.open_project(legacy_path).success, "open must succeed");

        let opened = state.get_project_state().unwrap();
        assert_eq!(
            opened.validation.warn_shot_length_min, custom.warn_shot_length_min,
            "legacy project must inherit the user's global thresholds"
        );
        // The customized threshold actually drives warnings: the 1.0 m shot is now SHORT.
        assert!(
            state
                .compute_project_warnings(&opened, None)
                .iter()
                .any(|w| w.code == "SHORT_SHOT"),
            "the seeded global threshold must produce the SHORT_SHOT warning"
        );

        // --- Explicit-row file: a project that carries its OWN validation row
        //     (different from global) must NOT be overridden on open. ---
        let explicit_path = dir.path().join("explicit.stalx");
        {
            let mut project = Project::new("Explicit".to_string(), None);
            project.validation = crate::config::ValidationConfig {
                warn_shot_length_min: 0.1,
                ..crate::config::ValidationConfig::default()
            };
            project.add_section(SectionData {
                shots: vec![test_shot_data(1.0)],
                ..Default::default()
            });
            let mut db = Database::open(&explicit_path).unwrap();
            db.save_project(&project).unwrap();
        }
        let state2 = AppState::new().unwrap();
        let mut config2 = state2.get_config();
        config2.validation = custom.clone();
        state2.update_config(config2).unwrap();
        assert!(state2.open_project(explicit_path).success);
        let opened2 = state2.get_project_state().unwrap();
        assert_eq!(
            opened2.validation.warn_shot_length_min, 0.1,
            "a project with an explicit validation row keeps its own thresholds"
        );
    });
}

// === #41 synthetic repro: deleting a shot must not shift unrelated/upstream geometry ===
//
// "On macOS, deleting a shot sometimes causes an erroneous geometry shift - the
// entire map moves." The investigation pinned this on coordinate-frame
// re-anchoring (section offsets + `cascade_connection_offsets`), not a viewport
// recenter. These exercise the real `Command::DeleteShot` path and assert on
// absolute coordinates so any global shift fails the test deterministically -
// no UI clicking required.

/// Deleting a shot in one connected component must NEVER move a station in an
/// unrelated component. The strongest "whole map jumps" sentinel: two sections,
/// each its own zero datum, sharing no merge/closure.
///
/// The delete here is the first shot of A, which removes A's station 0 and with
/// it the zero datum keyed `StationAnchor::SectionStart` - and that datum was the
/// PRIMARY one. So this command also changes the georeference frame, and the
/// oracle has to be stated in the frame-independent terms it always meant (#469,
/// item 1): B does not move IN THE WORLD. Local (0, 0) means `zero_datums[0]`'s
/// GPS, so when B's datum is promoted to first the whole survey is translated so
/// that B's datum station lands on the origin, and B's real-world position - what
/// the map draws and what the exports write - is unchanged precisely BECAUSE its
/// local coordinates moved by that one vector.
///
/// The assertion is therefore "B translated by exactly the promoted datum's own
/// former position, and by nothing else", which is strictly sharper than the
/// byte-for-byte form it replaces: a spurious shift of any size still fails, and
/// so does a translation of the wrong magnitude. Section A's own stations are not
/// comparable across the delete (a shot went away), which is why the sentinel was
/// always read off B.
#[test]
fn delete_shot_does_not_shift_independent_component() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0), test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Independent datums → each component has a fixed absolute frame.
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(b, 0),
                coordinates: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
            })
            .success
    );

    let before = abs_coord_map(&state.get_project_state().unwrap());

    let a_shot0 = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots[0]
        .id;
    assert!(
        state
            .execute(Command::DeleteShot {
                section_id: a,
                shot_id: a_shot0,
            })
            .success
    );

    let after = abs_coord_map(&state.get_project_state().unwrap());

    // The delete took A's station 0, so A's datum went with it and B's was
    // promoted to primary. B's datum station is the new local origin.
    let promoted = StationRef::new(b, 0);
    let promoted_now = after[&promoted];
    assert!(
        promoted_now.x.abs() < 1e-6 && promoted_now.y.abs() < 1e-6,
        "the promoted datum's station must sit at the local origin, got {promoted_now:?}: local \
         (0, 0) means zero_datums[0]'s GPS, so a promotion that leaves the survey where it was \
         re-georeferences it by the whole inter-datum vector"
    );

    // Section B (unrelated component) is unmoved in the WORLD: every station
    // translated by exactly the vector that put the promoted datum on the origin,
    // and z (which section offsets do not carry) is untouched.
    let (dx, dy) = (before[&promoted].x, before[&promoted].y);
    assert!(
        dx.abs() > 1000.0,
        "setup: the two datums must sit kilometres apart, got ({dx}, {dy})"
    );
    for idx in 0..=2 {
        let r = StationRef::new(b, idx);
        assert_coords_close(
            after[&r],
            Coordinates {
                x: before[&r].x - dx,
                y: before[&r].y - dy,
                z: before[&r].z,
            },
        );
    }
}

/// Deleting the LAST shot of a section removes only its end station; the section
/// head and a neighbor merged onto that head must not move.
#[test]
fn delete_tail_shot_does_not_shift_merged_head_neighbor() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0), test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // B hangs off A's HEAD (A:0), which is upstream of the tail shot we delete.
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 0),
                station_b: StationRef::new(b, 0),
            })
            .success
    );

    let before = abs_coord_map(&state.get_project_state().unwrap());

    let a_last = {
        let p = state.get_project_state().unwrap();
        let sec = p.sections.iter().find(|s| s.id == a).unwrap();
        sec.shots[sec.shots.len() - 1].id
    };
    assert!(
        state
            .execute(Command::DeleteShot {
                section_id: a,
                shot_id: a_last,
            })
            .success
    );

    let after = abs_coord_map(&state.get_project_state().unwrap());

    // A's head (A:0) and the merged neighbor B are fully untouched by a tail delete.
    let a0 = StationRef::new(a, 0);
    assert_coords_close(after[&a0], before[&a0]);
    for idx in 0..=2 {
        let r = StationRef::new(b, idx);
        assert_coords_close(after[&r], before[&r]);
    }
    // A:1 becomes the new end station: its X/Y (the map) must not move, though its
    // Z legitimately changes as the deleted shot's depth conflict at this interior
    // station is resolved. The #41 bug is an X/Y shift, so guard X/Y here.
    let a1 = StationRef::new(a, 1);
    let eps = 1e-6;
    assert!(
        (after[&a1].x - before[&a1].x).abs() < eps,
        "A:1 x shifted (map moved)"
    );
    assert!(
        (after[&a1].y - before[&a1].y).abs() < eps,
        "A:1 y shifted (map moved)"
    );
}

/// Deleting the FIRST shot keeps the remaining stations in place (offset is
/// adjusted by the removed shot's displacement); a neighbor merged onto the
/// downstream/tail end must therefore stay put too - i.e. the offset cascade
/// must not over-propagate into a global shift.
#[test]
fn delete_first_shot_keeps_downstream_merged_neighbor_put() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
        })
        .section_id
        .unwrap();
    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(8.0)],
        })
        .section_id
        .unwrap();

    // C hangs off A's TAIL (A:2). Deleting A's first shot must keep A:2 (and thus
    // C) at the same absolute position.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 2),
                station_b: StationRef::new(c, 0),
            })
            .success
    );

    let before = abs_coord_map(&state.get_project_state().unwrap());

    let a_shot0 = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots[0]
        .id;
    assert!(
        state
            .execute(Command::DeleteShot {
                section_id: a,
                shot_id: a_shot0,
            })
            .success
    );

    let after = abs_coord_map(&state.get_project_state().unwrap());

    // C is downstream of the kept side; its absolute position must not shift.
    for idx in 0..=1 {
        let r = StationRef::new(c, idx);
        assert_coords_close(after[&r], before[&r]);
    }
}

/// #27: unreadable-kind diagnostics belong to the load that found them.
///
/// Two loads can overlap - opening B cancels A, but a cancelled load keeps
/// running to its next checkpoint - so a slower A can reach the setter AFTER B
/// stored its own. An unversioned slot would then hand B the section and shot
/// ids of a project the user navigated away from, and applying that resolution
/// would write kinds onto whatever shots in B happen to share those ids.
#[test]
fn unrecognised_shot_kinds_are_scoped_to_their_load() {
    use crate::models::UnrecognisedShotKind;

    let state = AppState::new().unwrap();
    let entry = |section_id: u32| UnrecognisedShotKind {
        section_id,
        shot_id: section_id * crate::models::FEATURE_ID_STRIDE,
        shot_number: 1,
        section_name: Some(format!("s{section_id}")),
        raw_value: "7".to_string(),
    };

    // Load A (id 1), then load B (id 2) - the project the user actually opened.
    state.set_unrecognised_shot_kinds(1, vec![entry(11)]);
    state.set_unrecognised_shot_kinds(2, vec![entry(22)]);

    // The cancelled A finally reaches the setter. It must NOT clobber B.
    state.set_unrecognised_shot_kinds(1, vec![entry(11)]);

    // A asking gets nothing rather than B's list.
    assert!(
        state.take_unrecognised_shot_kinds(1).is_empty(),
        "a stale load must not receive the current load's diagnostics"
    );

    let for_b = state.take_unrecognised_shot_kinds(2);
    assert_eq!(for_b.len(), 1);
    assert_eq!(
        for_b[0].section_id, 22,
        "B must get its OWN diagnostics, not the cancelled load's"
    );

    // One-shot: a second ask returns nothing.
    assert!(state.take_unrecognised_shot_kinds(2).is_empty());
}

#[test]
fn test_set_shot_kind() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Initially not reference
    assert!(!project.sections[0].shots[0].kind.is_virtual());

    let result = state.execute(Command::SetShotKind {
        section_id,
        shot_id,
        kind: ShotKind::Jump,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert!(project.sections[0].shots[0].kind.is_virtual());
}

// === Undo/Redo Tests ===

#[test]
fn test_undo_create_section() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    state.execute(Command::CreateSection {
        name: Some("To Undo".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
    assert!(state.can_undo());

    let result = state.undo();
    assert!(result.success);

    assert!(state.get_project_state().unwrap().sections.is_empty());
}

#[test]
fn test_undo_delete_section() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Will Delete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    state.execute(Command::DeleteSection { section_id });
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Undo the delete
    let result = state.undo();
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].name, Some("Will Delete".to_string()));
}

#[test]
fn test_undo_update_shot() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(15.0),
            ..Default::default()
        },
    });

    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots[0].length,
        15.0
    );

    let result = state.undo();
    assert!(result.success);

    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots[0].length,
        5.0
    );
}

#[test]
fn test_redo_create_section() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    state.execute(Command::CreateSection {
        name: Some("Redo Me".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    state.undo();
    assert!(state.get_project_state().unwrap().sections.is_empty());
    assert!(state.can_redo());

    let result = state.redo();
    assert!(result.success);

    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
}

#[test]
fn test_undo_nothing() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    assert!(!state.can_undo());

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Nothing to undo"));
}

#[test]
fn test_redo_nothing() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    assert!(!state.can_redo());

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Nothing to redo"));
}

#[test]
fn test_redo_cleared_on_new_command() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    state.execute(Command::CreateSection {
        name: Some("First".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    state.undo();
    assert!(state.can_redo());

    // New command clears redo stack
    state.execute(Command::CreateSection {
        name: Some("Different".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    assert!(!state.can_redo());
}

#[test]
fn test_undo_fails_when_undo_stack_poisoned() {
    let state = AppState::new().unwrap();
    poison_mutex(&state.undo_stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock undo stack"));
}

#[test]
fn test_redo_fails_when_undo_stack_poisoned() {
    let state = AppState::new().unwrap();
    poison_mutex(&state.undo_stack);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock undo stack"));
}

#[test]
fn test_undo_fails_for_non_undoable_entry() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(Command::CloseProject, UndoData::None);
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("This command cannot be undone")
    );
    assert!(!state.can_redo());
}

#[test]
fn test_project_snapshot_pair_undo_redo_roundtrip() {
    let state = AppState::new().unwrap();

    let mut before = Project::new("Before".to_string(), None);
    before.add_section(SectionData {
        name: Some("Before Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots: vec![test_shot_data(1.0)],
    });

    let mut after = Project::new("After".to_string(), None);
    after.add_section(SectionData {
        name: Some("After Section".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 1.0,
        shots: vec![test_shot_data(2.0), test_shot_data(3.0)],
    });

    let before_json = serde_json::to_string(&before).unwrap();
    let after_json = serde_json::to_string(&after).unwrap();

    state.set_project_for_tests(after.clone());

    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::MergeProjects {
            source_paths: Vec::new(),
            output_path: PathBuf::from("out.stalx"),
            output_name: "Merged".to_string(),
            archive_originals: false,
        },
        UndoData::ProjectSnapshotPair {
            before: before_json,
            after: after_json,
        },
    );
    drop(stack);

    assert!(state.can_undo());

    let undo_result = state.undo();
    assert!(undo_result.success);

    let project_after_undo = state.get_project_state().unwrap();
    assert_eq!(project_after_undo.name, "Before");
    assert_eq!(project_after_undo.sections[0].shots.len(), 1);
    assert!(state.can_redo());

    let redo_result = state.redo();
    assert!(redo_result.success);

    let project_after_redo = state.get_project_state().unwrap();
    assert_eq!(project_after_redo.name, "After");
    assert_eq!(project_after_redo.sections[0].shots.len(), 2);
    assert!(state.can_undo());
    assert!(!state.can_redo());
}

#[test]
fn test_undo_project_snapshot_fails_when_project_lock_poisoned() {
    let state = AppState::new().unwrap();
    let snapshot = serde_json::to_string(&Project::new("Test".to_string(), None)).unwrap();

    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshot(snapshot),
    );
    drop(stack);

    poison_mutex(&state.project);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock project"));
}

#[test]
fn test_undo_project_snapshot_pair_fails_when_project_lock_poisoned() {
    let state = AppState::new().unwrap();
    let before = serde_json::to_string(&Project::new("Before".to_string(), None)).unwrap();
    let after = serde_json::to_string(&Project::new("After".to_string(), None)).unwrap();

    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshotPair { before, after },
    );
    drop(stack);

    poison_mutex(&state.project);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock project"));
}

#[test]
fn test_redo_project_snapshot_pair_fails_when_project_lock_poisoned() {
    let state = AppState::new().unwrap();
    let before = serde_json::to_string(&Project::new("Before".to_string(), None)).unwrap();
    let after = serde_json::to_string(&Project::new("After".to_string(), None)).unwrap();

    let entry = UndoEntry {
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair { before, after },
        timestamp: None,
    };

    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_redo(entry);
    drop(stack);

    poison_mutex(&state.project);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock project"));
}

#[test]
fn test_undo_merge_info_fails_when_project_lock_poisoned() {
    let state = AppState::new().unwrap();

    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::MergeStations {
            station_a: StationRef::new(1, 0),
            station_b: StationRef::new(1, 1),
        },
        UndoData::MergeInfo {
            station_a: StationRef::new(1, 0),
            station_b: StationRef::new(1, 1),
            removed_closures: Vec::new(),
            previous_offsets: HashMap::new(),
        },
    );
    drop(stack);

    poison_mutex(&state.project);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock project"));
}

// === State Transition Tests ===

#[test]
fn test_project_modification_tracking() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let initial_modified = state.get_project_state().unwrap().modified_at;

    // Sleep a tiny bit to ensure timestamp changes
    std::thread::sleep(std::time::Duration::from_millis(1));

    state.execute(Command::CreateSection {
        name: Some("Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    let after_section = state.get_project_state().unwrap().modified_at;
    assert!(after_section > initial_modified);
}

#[test]
fn test_is_undoable_commands() {
    // Non-undoable commands
    assert!(!AppState::is_undoable(&Command::OpenProject {
        path: PathBuf::from("/test.stalx"),
    }));
    assert!(!AppState::is_undoable(&Command::SaveProject { path: None }));
    assert!(!AppState::is_undoable(&Command::CloseProject));
    assert!(!AppState::is_undoable(&Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    }));

    // Undoable commands
    assert!(AppState::is_undoable(&Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#000000".to_string(),
        direction: None,
        shots: vec![],
    }));
    assert!(AppState::is_undoable(&Command::DeleteSection {
        section_id: 0
    }));
    assert!(AppState::is_undoable(&Command::CreateShot {
        section_id: 0,
        shot: ShotData::default(),
        after_shot_id: None,
        before_shot_id: None,
    }));
}

#[test]
fn path_bearing_commands_never_enter_shared_undo_history() {
    let private = PathBuf::from("/home/alice/private/survey.stalx");
    let commands = [
        Command::OpenProject {
            path: private.clone(),
        },
        Command::SaveProject {
            path: Some(private.clone()),
        },
        Command::MergeProjects {
            source_paths: vec![private.clone()],
            output_path: private,
            output_name: "Merged".to_string(),
            archive_originals: false,
        },
    ];

    for command in commands {
        assert!(
            !AppState::is_undoable(&command),
            "a filesystem-path command must never enter Full-mode shared undo: {command:?}"
        );
        assert!(
            !AppState::is_batch_allowed(&command),
            "a filesystem-path command must not bypass the privacy fence inside Batch: {command:?}"
        );
    }
}

#[test]
fn confirmed_push_persists_the_pruned_undo_and_redo_stacks() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("post-push-history.stalx");
    let cutoff = chrono::Utc::now();
    let row = |name: &str, stamp: chrono::DateTime<chrono::Utc>| {
        (
            serde_json::to_string(&Command::CreateProject {
                name: name.to_string(),
                location: None,
            })
            .unwrap(),
            serde_json::to_string(&UndoData::InverseCommand(Command::CloseProject)).unwrap(),
            Some(stamp.to_rfc3339()),
        )
    };
    let old = row("old", cutoff - chrono::Duration::hours(1));
    let late = row("late", cutoff + chrono::Duration::hours(1));
    let redo = row("redo", cutoff + chrono::Duration::hours(2));

    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Post push".to_string(), None))
        .unwrap();
    db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
    db.save_undo_stack_with_timestamps(vec![old.clone(), late.clone()])
        .unwrap();
    db.save_redo_stack_with_timestamps(vec![redo.clone()])
        .unwrap();
    db.save_speleo_undo_chain(Some("previous-tip")).unwrap();
    db.save_speleo_undo_watermark(Some(&cutoff.to_rfc3339()))
        .unwrap();

    let state = AppState::new().unwrap();
    state
        .undo_stack
        .lock()
        .unwrap()
        .load_from_persisted_with_timestamps(vec![old, late], vec![redo]);
    *state.database.lock().unwrap() = Some(db);

    state.speleodb_clear_undo_after_push(&cutoff.to_rfc3339());

    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(stack.len(), 1, "the late edit remains undoable in memory");
    assert_eq!(stack.redo_len(), 0, "redo never survives a confirmed push");
    drop(stack);

    let database = state.database.lock().unwrap();
    let db = database.as_ref().unwrap();
    let disk_undo = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(disk_undo.len(), 1, "uploaded rows are removed from SQLite");
    assert!(disk_undo[0].1.contains("late"));
    assert!(db.load_redo_stack_with_timestamps().unwrap().is_empty());
    assert_eq!(db.load_speleo_undo_chain().unwrap(), None);
    assert_eq!(db.load_speleo_undo_watermark().unwrap(), None);
}

// === Config Tests ===

#[test]
fn test_get_config() {
    // Isolate the config path to a fresh temp dir so this default-value
    // assertion can't read a value left in the real config file by a
    // config-writing test (which previously made this fail with 42 vs 100).
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        let config = state.get_config();

        // Should have default values
        assert!(config.general.auto_save);
        assert_eq!(config.general.undo_history_limit, 100);
    });
}

#[test]
fn test_update_config_preserves_backend_only_webkit_memory() {
    // Regression: `webkit_memory` is `#[ts(skip)]` + `#[serde(skip_serializing)]`,
    // so the frontend never receives it and a settings-save payload always carries
    // the serde default. `update_config` must preserve the hand-tuned in-memory
    // value; otherwise saving any setting silently resets a user's
    // `[webkit_memory]` `config.toml` knobs to defaults.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();

        // Seed a non-default value, as if loaded from a hand-edited config.toml.
        {
            let mut cfg = state.config.lock().unwrap();
            cfg.webkit_memory.memory_limit_mb = 1234;
            cfg.webkit_memory.jsc_force_ram_size_mb = 4096;
        }

        // Simulate the frontend saving an unrelated setting: its payload carries a
        // default webkit_memory because it never saw the real one.
        let mut payload = state.get_config();
        payload.webkit_memory = crate::config::WebkitMemoryConfig::default();
        payload.general.auto_save = !payload.general.auto_save;
        let expected_auto_save = payload.general.auto_save;
        state.update_config(payload).expect("update config");

        let after = state.get_config();
        assert_eq!(
            after.webkit_memory.memory_limit_mb, 1234,
            "hand-tuned webkit_memory.memory_limit_mb must survive a settings save"
        );
        assert_eq!(
            after.webkit_memory.jsc_force_ram_size_mb, 4096,
            "hand-tuned webkit_memory.jsc_force_ram_size_mb must survive a settings save"
        );
        // The unrelated setting the "frontend" changed still applies.
        assert_eq!(after.general.auto_save, expected_auto_save);
    });
}

#[test]
fn test_webkit_memory_normalize_clamps_corrupt_thresholds() {
    // A hand-edited config.toml can carry non-finite / out-of-range thresholds;
    // Config::normalize() (run on load) must clamp them before they reach the wry
    // env vars.
    let mut cfg = crate::config::WebkitMemoryConfig {
        conservative_threshold: f64::NAN,
        strict_threshold: 5.0,
        kill_threshold: -1.0,
        poll_interval_seconds: 0.0,
        ..Default::default()
    };
    cfg.normalize();
    assert!(
        cfg.conservative_threshold.is_finite() && (0.0..=1.0).contains(&cfg.conservative_threshold)
    );
    assert!((0.0..=1.0).contains(&cfg.strict_threshold));
    assert_eq!(cfg.kill_threshold, 0.0);
    assert!(cfg.poll_interval_seconds > 0.0);
    assert!(
        cfg.conservative_threshold <= cfg.strict_threshold,
        "pressure ladder must stay monotonic after normalize"
    );
}

#[test]
fn test_webkit_memory_normalize_resets_inverted_ladder_to_exact_defaults() {
    // A VALID-but-inverted pair (conservative > strict, both finite and in
    // range) is meaningless, so normalize() resets BOTH to their exact defaults.
    // test_webkit_memory_normalize_clamps_corrupt_thresholds cannot reach this
    // branch: its NaN / out-of-range inputs are clamped to defaults (already
    // monotonic) before the `conservative > strict` check runs.
    let mut cfg = crate::config::WebkitMemoryConfig {
        conservative_threshold: 0.6,
        strict_threshold: 0.3,
        ..Default::default()
    };
    cfg.normalize();
    assert_eq!(
        cfg.conservative_threshold,
        crate::config::default_webkit_mem_conservative(),
        "inverted ladder must reset conservative_threshold to the exact default"
    );
    assert_eq!(
        cfg.strict_threshold,
        crate::config::default_webkit_mem_strict(),
        "inverted ladder must reset strict_threshold to the exact default"
    );
}

#[test]
fn test_webkit_memory_normalize_resets_kill_below_strict() {
    // A nonzero kill threshold below strict would terminate the web process
    // before the pressure ladder runs; normalize() fails it safe back to 0
    // (never kill), while a valid kill >= strict and the 0 sentinel are kept.
    let mut below = crate::config::WebkitMemoryConfig {
        conservative_threshold: 0.25,
        strict_threshold: 0.4,
        kill_threshold: 0.2,
        ..Default::default()
    };
    below.normalize();
    assert_eq!(
        below.kill_threshold, 0.0,
        "kill below strict must fail safe to 0"
    );

    let mut valid = crate::config::WebkitMemoryConfig {
        conservative_threshold: 0.25,
        strict_threshold: 0.4,
        kill_threshold: 0.9,
        ..Default::default()
    };
    valid.normalize();
    assert_eq!(valid.kill_threshold, 0.9, "a kill >= strict is preserved");

    let mut disabled = crate::config::WebkitMemoryConfig {
        strict_threshold: 0.4,
        kill_threshold: 0.0,
        ..Default::default()
    };
    disabled.normalize();
    assert_eq!(
        disabled.kill_threshold, 0.0,
        "the 0 never-kill sentinel is preserved"
    );
}

#[test]
fn test_display_settings_cache() {
    let state = AppState::new().unwrap();

    // Create a project (no database, so settings go to cache)
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Save a display setting
    let result = state.save_display_setting("test_key", "test_value");
    assert!(result.is_ok());

    // Retrieve it
    let value = state.get_display_setting("test_key").unwrap();
    assert_eq!(value, Some("test_value".to_string()));

    // Load all settings
    let settings = state.load_display_settings().unwrap();
    assert_eq!(settings.get("test_key"), Some(&"test_value".to_string()));
}

#[test]
fn test_save_project_prefers_database_display_settings_over_cache() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("source.stalx");
    let mut project = Project::new("Display Settings".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });

    let mut db = Database::open(&db_path).unwrap();
    db.save_project(&project).unwrap();
    db.save_display_setting("showStations", "false").unwrap();
    drop(db);

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    let db = Database::open_existing(&db_path).unwrap();
    state.set_database_for_tests(db);

    {
        let mut cache = state.display_settings_cache.lock().unwrap();
        cache.insert("showStations".to_string(), "true".to_string());
    }

    let save_path = temp_dir.path().join("dest");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    let saved_path = save_path.with_extension("stalx");
    let saved_db = Database::open_readonly(&saved_path).unwrap();
    let settings = saved_db.load_display_settings().unwrap();
    assert_eq!(settings.get("showStations"), Some(&"false".to_string()));
}

#[test]
fn test_save_project_failure_does_not_change_existing_file() {
    let temp_dir = TempDir::new().unwrap();
    let original_path = temp_dir.path().join("original.stalx");

    let mut project = Project::new("Original".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });

    let mut db = Database::open(&original_path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);

    project.file_path = Some(original_path.clone());

    let state = AppState::new().unwrap();
    state.set_project_for_tests(project.clone());

    let blocked_path = temp_dir.path().join("blocked.stalx");
    std::fs::create_dir_all(&blocked_path).unwrap();

    let result = state.save_project(Some(blocked_path.clone()));
    assert!(!result.success);

    let reloaded = Database::open_existing(&original_path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(reloaded.name, project.name);
    assert_eq!(reloaded.sections.len(), project.sections.len());

    let project_after = state.get_project_state().unwrap();
    assert_eq!(project_after.file_path, Some(original_path));

    let temp_path = blocked_path.with_extension("stalx.tmp");
    assert!(!temp_path.exists());
}

// ── #39: macOS lost-folder-access detection ──────────────────────────────
// macOS forgets previously-granted folder access after an OS/app update,
// which the user sees as a bogus "corrupted database" / save failure. The
// backend classifies the underlying permission denial so the frontend can
// show folder-access guidance instead.

#[test]
fn is_permission_denied_classifies_os_errors() {
    use crate::core::loading::is_permission_denied;
    // The portable io kind - covers ERROR_ACCESS_DENIED on Windows and
    // EACCES/EPERM on unix without hardcoding platform errnos.
    let kind_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    assert!(is_permission_denied(&anyhow::Error::from(kind_err)));
    // A denial that only reaches us as text still counts.
    assert!(is_permission_denied(&anyhow::anyhow!(
        "Failed to create save file: Permission denied"
    )));
    // EPERM (the "os error 1" from the bug report) and EACCES decode to
    // PermissionDenied on unix; the libc constants keep this portable.
    #[cfg(unix)]
    {
        assert!(is_permission_denied(&anyhow::Error::from(
            std::io::Error::from_raw_os_error(libc::EPERM)
        )));
        assert!(is_permission_denied(&anyhow::Error::from(
            std::io::Error::from_raw_os_error(libc::EACCES)
        )));
    }
}

#[test]
fn is_permission_denied_rejects_unrelated_errors() {
    use crate::core::loading::is_permission_denied;
    // The crux of the fix: corruption must NOT be read as a permission
    // problem, or we'd show the wrong remediation for a real data issue.
    assert!(!is_permission_denied(&anyhow::anyhow!(
        "Database integrity check failed: database disk image is malformed"
    )));
    assert!(!is_permission_denied(&anyhow::anyhow!("file is corrupted")));
    assert!(!is_permission_denied(&anyhow::anyhow!(
        "Failed to save project"
    )));
    // ENOENT is not a permission denial.
    #[cfg(unix)]
    assert!(!is_permission_denied(&anyhow::Error::from(
        std::io::Error::from_raw_os_error(libc::ENOENT)
    )));
}

#[test]
fn sqlite_cannot_open_alone_is_not_a_permission_denial() {
    // #429: SQLite's `SQLITE_CANTOPEN` message carries no errno, and on Windows
    // it is what a missing folder, a disconnected drive and an over-MAX_PATH
    // path all produce. Treating it as a denial on its own is what showed a
    // Windows user macOS folder-permission instructions, so the predicate must
    // refuse to answer on the string alone - the callers corroborate it.
    use crate::core::loading::{is_permission_denied, mentions_unopenable_database};
    let err = anyhow::anyhow!("unable to open database file: C:\\surveys\\cave.stalx");
    assert!(!is_permission_denied(&err));
    assert!(mentions_unopenable_database(&err));
    // ...including when it is buried in an anyhow context chain, which is how
    // `Database::open` reports it ("Failed to open database" on top).
    let chained = anyhow::Error::from(anyhow::anyhow!("unable to open database file"))
        .context("Failed to open database");
    assert!(!is_permission_denied(&chained));
    assert!(mentions_unopenable_database(&chained));
}

#[test]
fn is_path_not_found_classifies_missing_and_unreachable_paths() {
    use crate::core::loading::is_path_not_found;
    assert!(is_path_not_found(&anyhow::Error::from(
        std::io::Error::new(std::io::ErrorKind::NotFound, "gone")
    )));
    // Windows wording for an unmapped drive / deleted folder, and the unix one.
    assert!(is_path_not_found(&anyhow::anyhow!(
        "The system cannot find the path specified. (os error 3)"
    )));
    assert!(is_path_not_found(&anyhow::anyhow!(
        "No such file or directory (os error 2)"
    )));
    assert!(is_path_not_found(&anyhow::anyhow!(
        "The device is not ready. (os error 21)"
    )));
    // A denial is not a missing path, and neither is corruption.
    assert!(!is_path_not_found(&anyhow::Error::from(
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied")
    )));
    assert!(!is_path_not_found(&anyhow::anyhow!(
        "database disk image is malformed"
    )));
}

#[test]
fn over_legacy_path_limit_measures_the_windows_max_path_budget() {
    // The 260-character budget INCLUDES the terminating NUL, so 259 characters
    // is the longest name SQLite's win32 VFS can open. Pure arithmetic, so it
    // is pinned on every platform even though only Windows consults it.
    use crate::core::loading::over_legacy_path_limit;
    let root = "C:\\s\\";
    let make = |len: usize| PathBuf::from(format!("{root}{}", "a".repeat(len - root.len())));
    assert!(!over_legacy_path_limit(&make(259)));
    assert!(over_legacy_path_limit(&make(260)));
    assert!(over_legacy_path_limit(&make(335)));
    assert!(!over_legacy_path_limit(&PathBuf::from(
        "C:\\surveys\\cave.stalx"
    )));
}

// The save-failure classification matrix (#429 items 2 + 3). Each arm decides
// which remediation the user is shown, so each gets an explicit case.

#[test]
fn save_failure_tags_a_corroborated_denial_with_the_permission_code() {
    use crate::core::loading::PERMISSION_DENIED_CODE;
    let temp_dir = TempDir::new().unwrap();
    let target = temp_dir.path().join("cave.stalx");
    let err = anyhow::Error::from(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "denied",
    ))
    .context("Failed to create save file");

    let result = project_io::save_failure("Failed to create save file", &target, &err);

    let msg = result.error.unwrap_or_default();
    assert!(
        msg.starts_with(PERMISSION_DENIED_CODE),
        "a typed PermissionDenied chain is a denial, got: {msg}"
    );
}

#[test]
fn save_failure_tags_a_vanished_folder_with_the_path_code() {
    use crate::core::loading::{PATH_NOT_FOUND_CODE, PERMISSION_DENIED_CODE};
    let temp_dir = TempDir::new().unwrap();
    // Nothing was ever created at `gone/`, so the destination folder is absent.
    let target = temp_dir.path().join("gone").join("cave.stalx");
    let err = anyhow::Error::from(anyhow::anyhow!("unable to open database file"))
        .context("Failed to open database");

    let result = project_io::save_failure("Failed to create save file", &target, &err);

    let msg = result.error.unwrap_or_default();
    assert!(
        msg.starts_with(PATH_NOT_FOUND_CODE),
        "a missing destination folder is a path error, got: {msg}"
    );
    assert!(!msg.starts_with(PERMISSION_DENIED_CODE));
}

#[test]
fn save_failure_leaves_a_generic_open_failure_unclassified() {
    use crate::core::loading::{PATH_NOT_FOUND_CODE, PERMISSION_DENIED_CODE};
    // The regression that started #429: the folder exists AND accepts files, so
    // SQLite's ambiguous "unable to open database file" is corroborated by
    // nothing. It must stay an ordinary failure rather than send the user to
    // Privacy & Security (or claim their drive is unplugged).
    let temp_dir = TempDir::new().unwrap();
    let target = temp_dir.path().join("cave.stalx");
    let err = anyhow::Error::from(anyhow::anyhow!("unable to open database file"))
        .context("Failed to open database");

    let result = project_io::save_failure("Failed to create save file", &target, &err);

    let msg = result.error.unwrap_or_default();
    assert!(
        !msg.starts_with(PERMISSION_DENIED_CODE),
        "a writable destination folder is denying nothing, got: {msg}"
    );
    assert!(!msg.starts_with(PATH_NOT_FOUND_CODE), "got: {msg}");
    assert_eq!(
        msg, "Failed to create save file: Failed to open database",
        "the backend message is passed through unmarked"
    );
    // The write probe must leave nothing behind.
    let leftovers: Vec<_> = std::fs::read_dir(temp_dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "probe file left behind: {leftovers:?}"
    );
}

#[test]
fn write_probe_refuses_a_name_something_else_already_holds() {
    // `create_new`, so the probe can only ever create its OWN file: it never
    // opens - and so never truncates - a file that is already there.
    let temp_dir = TempDir::new().unwrap();
    let taken = temp_dir.path().join("survey.stalx");
    std::fs::write(&taken, b"survey data").unwrap();

    let kind = project_io::create_write_probe(&taken)
        .err()
        .map(|e| e.kind());

    assert_eq!(kind, Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&taken).unwrap(), b"survey data".as_slice());
}

#[cfg(unix)]
#[test]
fn write_probe_does_not_write_through_a_planted_symlink() {
    // The probe name carries our pid, so it is guessable; in a save directory
    // other users can write to, one of them can plant a symlink at it ahead of
    // time. `File::create` would follow the link and truncate its target.
    let temp_dir = TempDir::new().unwrap();
    let victim = temp_dir.path().join("victim.stalx");
    std::fs::write(&victim, b"survey data").unwrap();
    let planted = temp_dir.path().join(".stalagmight-write-probe-1-0");
    std::os::unix::fs::symlink(&victim, &planted).unwrap();

    let kind = project_io::create_write_probe(&planted)
        .err()
        .map(|e| e.kind());

    assert_eq!(kind, Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&victim).unwrap(), b"survey data".as_slice());
}

#[cfg(windows)]
#[test]
fn save_failure_tags_an_over_max_path_destination_as_too_long() {
    use crate::core::loading::{PATH_TOO_LONG_CODE, PERMISSION_DENIED_CODE};
    // #429 item 2's repro: a ~335-character destination whose folders all
    // exist. Rust stats the parent fine (std uses `\\?\` verbatim paths) while
    // SQLite's win32 VFS cannot open the file, so the parent probe alone would
    // still call this a denial.
    let temp_dir = TempDir::new().unwrap();
    let long_name = format!("{}.stalx", "n".repeat(300));
    let target = temp_dir.path().join(long_name);
    let err = anyhow::Error::from(anyhow::anyhow!("unable to open database file"))
        .context("Failed to open database");

    let result = project_io::save_failure("Failed to create save file", &target, &err);

    let msg = result.error.unwrap_or_default();
    assert!(
        msg.starts_with(PATH_TOO_LONG_CODE),
        "an over-MAX_PATH destination is a path-length error, got: {msg}"
    );
    assert!(!msg.starts_with(PERMISSION_DENIED_CODE));
}

#[cfg(unix)]
#[test]
fn save_into_unwritable_dir_reports_permission_code() {
    use crate::core::loading::PERMISSION_DENIED_CODE;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let locked = temp_dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    // If we can still write despite 0o000 (e.g. running as root, as some CI
    // does), the permission scenario doesn't apply - skip rather than
    // false-fail. Restore perms first so TempDir cleanup can remove it.
    if std::fs::File::create(locked.join(".probe")).is_ok() {
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        return;
    }

    let mut project = Project::new("Perm".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.save_project(Some(locked.join("p.stalx")));

    // Restore perms so TempDir's drop can clean up the directory.
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    assert!(!result.success, "save into an unwritable dir should fail");
    let err = result.error.unwrap_or_default();
    assert!(
        err.starts_with(PERMISSION_DENIED_CODE),
        "expected permission code prefix, got: {err}"
    );
}

#[cfg(unix)]
#[test]
fn open_project_into_unreadable_dir_reports_permission_code() {
    // open_project switched exists() -> try_exists() so a metadata-access denial
    // surfaces as PERMISSION_DENIED_CODE instead of collapsing to "File not found"
    // (#169). try_exists() on a path inside a 0o000 dir returns Err(PermissionDenied)
    // because the dir can't be traversed. Same CI-safe 0o000 idiom (root-skip) as
    // save_into_unwritable_dir_reports_permission_code.
    use crate::core::loading::PERMISSION_DENIED_CODE;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let locked = temp_dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    // Root (some CI) bypasses 0o000, so try_exists would succeed and the permission
    // scenario wouldn't apply - skip rather than false-fail. Restore perms first so
    // TempDir cleanup can remove the directory.
    if std::fs::File::create(locked.join(".probe")).is_ok() {
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        return;
    }

    let state = AppState::new().unwrap();
    let result = state.open_project(locked.join("p.stalx"));

    // Restore perms so TempDir's drop can clean up the directory.
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    assert!(!result.success, "open inside an unreadable dir should fail");
    let err = result.error.unwrap_or_default();
    assert!(
        err.starts_with(PERMISSION_DENIED_CODE),
        "open_project should tag a metadata-access denial with the permission code, got: {err}"
    );
}

#[test]
fn save_into_missing_dir_is_not_a_permission_error() {
    use crate::core::loading::{PATH_NOT_FOUND_CODE, PERMISSION_DENIED_CODE};

    // A deleted/renamed save folder yields SQLite's generic "unable to open
    // database file" too - but that's a vanished location, not a permission
    // denial, so it must NOT be stamped with the permission code (which would
    // wrongly show the user Privacy & Security guidance). It is refused up
    // front instead, with the code that says the folder is gone (#429).
    let temp_dir = TempDir::new().unwrap();
    let missing = temp_dir.path().join("gone").join("p.stalx");

    let mut project = Project::new("Missing".to_string(), None);
    project.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result = state.save_project(Some(missing));

    assert!(!result.success, "save into a missing dir should fail");
    let err = result.error.unwrap_or_default();
    assert!(
        !err.starts_with(PERMISSION_DENIED_CODE),
        "missing dir should not be a permission failure, got: {err}"
    );
    assert!(
        err.starts_with(PATH_NOT_FOUND_CODE),
        "missing dir should be reported as a missing destination, got: {err}"
    );
    assert!(
        err.contains("the folder no longer exists or the drive is not connected"),
        "the user needs to be told what is wrong with the destination, got: {err}"
    );
    // Refused before anything is opened: no temp file, no stray folder.
    assert!(!temp_dir.path().join("gone").exists());
}

#[test]
fn test_display_settings_fail_when_cache_lock_poisoned() {
    let state = AppState::new().unwrap();

    poison_mutex(&state.display_settings_cache);

    let err = state.load_display_settings().unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock display settings cache");

    let err = state.save_display_setting("key", "value").unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock display settings cache");

    let err = state.get_display_setting("key").unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock display settings cache");
}

#[test]
fn test_display_settings_fail_when_database_lock_poisoned() {
    let state = AppState::new().unwrap();

    poison_mutex(&state.database);

    let err = state.load_display_settings().unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock database");

    let err = state.save_display_setting("key", "value").unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock database");

    let err = state.get_display_setting("key").unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock database");
}

#[test]
fn test_resolved_warning_methods_fail_when_database_lock_poisoned() {
    let state = AppState::new().unwrap();

    poison_mutex(&state.database);

    let err = state.load_resolved_warnings().unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock database");

    let warning = crate::persistence::ResolvedWarning {
        section_id: 1,
        shot_id: 2,
        warning_code: "TEST".to_string(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    };

    let err = state.save_resolved_warning(&warning).unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock database");

    let err = state
        .delete_resolved_warning(1, 2, "TEST", None)
        .unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock database");
}

#[test]
fn test_remove_recent_projects_fails_when_lock_poisoned() {
    let state = AppState::new().unwrap();
    poison_mutex(&state.recent_projects);

    let result = state.remove_from_recent_projects(PathBuf::from("missing.stalx"));
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to lock recent projects")
    );
}

#[test]
fn test_remove_recent_project_removes_present_entry() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("local.stalx");
        let managed = temp.path().join("managed.stalx");
        let original = b"synthetic original survey file";
        std::fs::write(&local, original).unwrap();
        std::fs::write(&managed, b"synthetic managed survey file").unwrap();
        let state = AppState::new().unwrap();
        *state.recent_projects.lock().unwrap() = vec![local.clone(), managed.clone()];
        let result = state.remove_from_recent_projects(local.clone());
        assert!(result.success);
        assert_eq!(
            *state.recent_projects.lock().unwrap(),
            vec![managed.clone()]
        );
        assert_eq!(
            crate::config::Config::load()
                .unwrap()
                .general
                .recent_projects,
            vec![managed.clone()]
        );
        assert_eq!(std::fs::read(&local).unwrap(), original);
        assert_eq!(
            std::fs::read(&managed).unwrap(),
            b"synthetic managed survey file"
        );
    });
}

#[test]
fn test_remove_recent_project_failed_save_preserves_retry_and_original_file() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("synthetic-local.stalx");
        std::fs::write(&local, b"synthetic original survey file").unwrap();
        let state = AppState::new().unwrap();
        *state.recent_projects.lock().unwrap() = vec![local.clone()];
        {
            let mut config = state.config.lock().unwrap();
            config.general.recent_projects = vec![local.clone()];
            config.save().unwrap();
        }
        let config_path = crate::config::Config::config_path().unwrap();
        let before = std::fs::read(&config_path).unwrap();
        // A directory at the destination makes atomic replacement fail on every OS.
        std::fs::remove_file(&config_path).unwrap();
        std::fs::create_dir(&config_path).unwrap();
        assert!(!state.remove_from_recent_projects(local.clone()).success);
        assert_eq!(*state.recent_projects.lock().unwrap(), vec![local.clone()]);
        assert_eq!(
            state.get_config().general.recent_projects,
            vec![local.clone()]
        );
        std::fs::remove_dir(&config_path).unwrap();
        std::fs::write(&config_path, before).unwrap();
        assert!(state.remove_from_recent_projects(local.clone()).success);
        assert!(crate::config::Config::load()
            .unwrap()
            .general
            .recent_projects
            .is_empty());
        assert_eq!(
            std::fs::read(&local).unwrap(),
            b"synthetic original survey file"
        );
    });
}

#[test]
fn test_remove_recent_project_absent_is_idempotent_success() {
    // A path that is not in the list is a NO-OP SUCCESS, not a failure: "remove from
    // recent" means "ensure it's absent". Returning a failure used to strand a zombie
    // "missing" row that the frontend (whose config copy lagged the trimmed backend
    // list) could never clear, because it only refreshes on success.
    // Isolated for the same reason as the sibling above: this asserts on `success`,
    // which a concurrently-forced config-save failure would flip.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        {
            let mut recent = state.recent_projects.lock().unwrap();
            *recent = vec![PathBuf::from("/b.stalx")];
        }

        let result = state.remove_from_recent_projects(PathBuf::from("/gone.stalx"));
        assert!(result.success, "removing an absent recent must succeed");
        // The list is left untouched (nothing to remove).
        let recent = state.recent_projects.lock().unwrap();
        assert_eq!(*recent, vec![PathBuf::from("/b.stalx")]);
    });
}

#[test]
fn test_import_tracking_fails_when_project_lock_poisoned() {
    let state = AppState::new().unwrap();
    poison_mutex(&state.project);

    let err = state.check_duplicate_import("hash").unwrap_err();
    assert_eq!(err.to_string(), "Failed to lock project");

    let result = state.track_imported_file("hash".to_string());
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock project"));
}

// === Batch Command Tests ===

#[test]
fn test_batch_create_sections() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::CreateSection {
                name: Some("First".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![test_shot_data(5.0)],
            },
            Command::CreateSection {
                name: Some("Second".to_string()),
                surveyor: None,
                date: None,
                color: "#00ff00".to_string(),
                direction: None,
                shots: vec![test_shot_data(7.0)],
            },
        ],
    });

    assert!(result.success);
    assert_eq!(result.section_ids.len(), 2);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
}

#[test]
fn test_batch_fails_on_first_error() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // First command succeeds, second fails (delete nonexistent section)
    let result = state.execute(Command::Batch {
        commands: vec![
            Command::CreateSection {
                name: Some("First".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![],
            },
            Command::DeleteSection { section_id: 999 },
        ],
    });

    assert!(!result.success);
}

// === Station Merge Tests ===

#[test]
fn test_merge_stations_same_section_fails() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Try to merge stations from the same section
    let result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_id, 0),
        station_b: StationRef::new(section_id, 1),
    });

    assert!(!result.success);
    assert!(result
        .error
        .unwrap()
        .contains("Cannot merge stations from the same section"));
}

// === Zero Datum Tests ===

#[test]
fn test_set_and_remove_zero_datum() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 0);

    // Set zero datum
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.zero_datums.len(), 1);
    assert_eq!(project.zero_datums[0].gps.latitude, 45.0);

    // Remove zero datum
    let result = state.execute(Command::RemoveZeroDatum { station_ref });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert!(project.zero_datums.is_empty());
}

#[test]
fn test_remove_nonexistent_zero_datum() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Try to remove a datum that was never set
    let result = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(section_id, 0),
    });

    assert!(!result.success);
}

// === Move Section Tests ===

#[test]
fn test_move_section() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // First move
    let result = state.execute(Command::MoveSection {
        section_id,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.section_offsets.get(&section_id),
        Some(&(10.0, 20.0)),
        "First move should set offset to (10, 20)"
    );

    // Second move should accumulate
    let result = state.execute(Command::MoveSection {
        section_id,
        offset: Coordinates::new(5.0, -10.0, 0.0),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.section_offsets.get(&section_id),
        Some(&(15.0, 10.0)),
        "Second move should accumulate to (15, 10)"
    );
}

#[test]
fn test_move_section_undo_redo() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Move section
    state.execute(Command::MoveSection {
        section_id,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.section_offsets.get(&section_id),
        Some(&(10.0, 20.0))
    );

    // Undo should reverse the move
    let undo_result = state.undo();
    assert!(undo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project
            .section_offsets
            .get(&section_id)
            .copied()
            .unwrap_or((0.0, 0.0)),
        (0.0, 0.0),
        "Undo should restore offset to origin"
    );

    // Redo should re-apply
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.section_offsets.get(&section_id),
        Some(&(10.0, 20.0)),
        "Redo should re-apply the move"
    );
}

#[test]
fn move_section_moves_the_whole_merged_group_and_sticks() {
    // A merged section is rigidly bound to its partner: "Adjust Position"
    // (MoveSection) must move the WHOLE merge group together so the merge stays
    // aligned and the move isn't snapped back by the post-command reconcile - not
    // shove one member off the other (finding #6).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Merged".to_string(),
        location: None,
    });

    let mk = |name: &str, color: &str| Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: color.to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    };
    let a = state.execute(mk("A", "#ff0000")).section_id.unwrap();
    let b = state.execute(mk("B", "#00ff00")).section_id.unwrap();

    // Merge A:0 with B:0 → A and B are now one rigid group.
    let merge = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge.success, "merge failed: {:?}", merge.error);
    assert!(
        state
            .get_project_state()
            .unwrap()
            .get_rigid_connected_section_ids(a)
            .contains(&b),
        "A and B must be one rigid group after the merge"
    );

    // Offsets right after the merge (the merge/reconcile may have set them to align).
    let off = |st: &AppState, id| {
        st.get_project_state()
            .unwrap()
            .section_offsets
            .get(&id)
            .copied()
            .unwrap_or((0.0, 0.0))
    };
    let a_before = off(&state, a);
    let b_before = off(&state, b);

    // Move section A - the whole group must translate by the same offset and STICK.
    let r = state.execute(Command::MoveSection {
        section_id: a,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(r.success);

    assert_eq!(
        off(&state, a),
        (a_before.0 + 10.0, a_before.1 + 20.0),
        "the moved section translated and the move stuck (not reverted by reconcile)"
    );
    assert_eq!(
        off(&state, b),
        (b_before.0 + 10.0, b_before.1 + 20.0),
        "the merge partner translated by the same offset (moved as a rigid group)"
    );

    // Undo restores both; redo re-applies to both.
    assert!(state.undo().success);
    assert_eq!(off(&state, a), a_before, "undo restores the moved section");
    assert_eq!(
        off(&state, b),
        b_before,
        "undo restores the merge partner too"
    );

    assert!(state.redo().success);
    assert_eq!(off(&state, a), (a_before.0 + 10.0, a_before.1 + 20.0));
    assert_eq!(off(&state, b), (b_before.0 + 10.0, b_before.1 + 20.0));
}

#[test]
fn test_move_nonexistent_section() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::MoveSection {
        section_id: 999,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });

    assert!(!result.success);
}

// === Validation Tests ===

#[test]
fn test_create_section_rejects_zero_length_shot() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section with a zero-length shot (should be rejected)
    let result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 0.0, // Zero length triggers warning
            depth_in: 10.0,
            depth_out: 10.0,
            ..Default::default()
        }],
    });

    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("zero length"));
}

// === Undo/Redo with Shot Restoration ===

#[test]
fn test_undo_delete_shot_restores_at_correct_position() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(9.0),
        ],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let middle_shot_id = project.sections[0].shots[1].id;

    // Delete the middle shot
    state.execute(Command::DeleteShot {
        section_id,
        shot_id: middle_shot_id,
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);

    // Undo the delete
    let result = state.undo();
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 3);
    // The shot should be restored at position 1
    assert_eq!(project.sections[0].shots[1].length, 7.0);
}

// === Multiple Undo/Redo Sequence ===

#[test]
fn test_multiple_undo_redo_sequence() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create 3 sections
    for i in 1..=3 {
        state.execute(Command::CreateSection {
            name: Some(format!("Section {i}")),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        });
    }

    assert_eq!(state.get_project_state().unwrap().sections.len(), 3);

    // Undo all 3 creates
    state.undo();
    state.undo();
    state.undo();

    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Redo 2 of them
    state.redo();
    state.redo();

    assert_eq!(state.get_project_state().unwrap().sections.len(), 2);
}

// === Command Execution Tests (fn-34-nvq spec) ===
// These tests verify specific command behaviors as specified in the epic.

#[test]
fn test_execute_create_section_command() {
    // Verify CreateSection creates section correctly with all fields
    use chrono::NaiveDate;
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let test_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
    let result = state.execute(Command::CreateSection {
        name: Some("Main Passage".to_string()),
        surveyor: Some("Alice".to_string()),
        date: Some(test_date),
        color: "#00ff00".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![
            test_shot_data(10.0),
            test_shot_data(15.0),
            test_shot_data(8.0),
        ],
    });

    assert!(result.success);
    let section_id = result.section_id.expect("Section ID should be returned");

    let project = state.get_project_state().unwrap();
    let section = project
        .get_section(section_id)
        .expect("Section should exist");

    assert_eq!(section.name, Some("Main Passage".to_string()));
    assert_eq!(section.surveyor_names(), Some("Alice".to_string()));
    assert_eq!(section.first_survey_date(), Some(test_date));
    assert_eq!(section.color, "#00ff00");
    assert_eq!(section.direction, SurveyDirection::Out);
    assert_eq!(section.shots.len(), 3);
    assert_eq!(section.shots[0].length, 10.0);
    assert_eq!(section.shots[1].length, 15.0);
    assert_eq!(section.shots[2].length, 8.0);
}

#[test]
fn test_execute_merge_stations_command() {
    // Verify MergeStations updates section offsets correctly
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
    });
    let section_a_id = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0), test_shot_data(6.0)],
    });
    let section_b_id = result_b.section_id.unwrap();

    // Merge station 1 of section A with station 0 of section B
    let station_a = StationRef::new(section_a_id, 1);
    let station_b = StationRef::new(section_b_id, 0);

    let result = state.execute(Command::MergeStations {
        station_a,
        station_b,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();

    // Verify merge was recorded
    assert_eq!(project.station_merges.len(), 1);
    let merge = &project.station_merges[0];
    assert!(project.key_is_ref(&merge.station_a, station_a));
    assert!(project.key_is_ref(&merge.station_b, station_b));

    // Verify section offsets were updated - one section should have an offset
    // (the moving section gets an offset to align with the anchor)
    let has_offset = project.section_offsets.contains_key(&section_a_id)
        || project.section_offsets.contains_key(&section_b_id);
    assert!(has_offset, "Section offsets should be updated after merge");
}

#[test]
fn test_execute_set_zero_datum_command() {
    // Verify SetZeroDatum stores GPS coordinates correctly
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 0);
    let gps = GpsCoordinates::new(48.8584, 2.2945, 35.0).unwrap();

    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.zero_datums.len(), 1);

    let datum = &project.zero_datums[0];
    assert!(project.key_is_ref(&datum.station_ref, station_ref));
    assert!((datum.gps.latitude - 48.8584).abs() < 0.0001);
    assert!((datum.gps.longitude - 2.2945).abs() < 0.0001);
    assert!((datum.gps.depth_reference - 35.0).abs() < 0.0001);
}

#[test]
fn test_undo_set_zero_datum_restores_section_offsets() {
    // Verify undo restores exact previous section_offsets, including
    // removing keys that were newly created by the command
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections with shots
    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_b = result_b.section_id.unwrap();

    // Set an existing offset on section A only
    // (section B has no offset - key doesn't exist)
    {
        let mut proj = state.project.lock().unwrap();
        let project = proj.as_mut().unwrap();
        project.section_offsets.insert(section_a, (100.0, 200.0));
    }

    // Merge the sections so they're connected
    state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a, 1),
        station_b: StationRef::new(section_b, 0),
    });

    // Clear undo stack to focus on SetZeroDatum
    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.clear();
    }

    // Capture pre-command state
    let (pre_offset_a, pre_offset_b, pre_datum_count) = {
        let proj = state.project.lock().unwrap();
        let project = proj.as_ref().unwrap();
        (
            project.section_offsets.get(&section_a).copied(),
            project.section_offsets.get(&section_b).copied(),
            project.zero_datums.len(),
        )
    };

    // Set zero datum - this will create/modify offsets for both connected sections
    let station_ref = StationRef::new(section_a, 0);
    let gps = GpsCoordinates::new(45.0, -73.0, 100.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
    });
    assert!(result.success);

    // Verify offsets changed after SetZeroDatum
    let (post_offset_a, post_offset_b, post_datum_count) = {
        let proj = state.project.lock().unwrap();
        let project = proj.as_ref().unwrap();
        (
            project.section_offsets.get(&section_a).copied(),
            project.section_offsets.get(&section_b).copied(),
            project.zero_datums.len(),
        )
    };
    assert_ne!(
        pre_offset_a, post_offset_a,
        "Section A offset should change"
    );
    assert!(
        post_offset_b.is_some(),
        "Section B should now have an offset"
    );
    assert_eq!(
        post_datum_count,
        pre_datum_count + 1,
        "Datum should be added"
    );

    // Undo the SetZeroDatum
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    // Verify exact previous state is restored
    let (restored_offset_a, restored_offset_b, restored_datum_count) = {
        let proj = state.project.lock().unwrap();
        let project = proj.as_ref().unwrap();
        (
            project.section_offsets.get(&section_a).copied(),
            project.section_offsets.get(&section_b).copied(),
            project.zero_datums.len(),
        )
    };

    // Section A should have its original offset restored
    assert_eq!(
        restored_offset_a, pre_offset_a,
        "Section A offset should be restored to original value"
    );

    // Section B key should be removed (it didn't exist before)
    assert_eq!(
        restored_offset_b, pre_offset_b,
        "Section B offset key should be removed (was None before)"
    );
    assert!(
        pre_offset_b.is_none() && restored_offset_b.is_none(),
        "Section B offset key must not exist after undo"
    );

    // Datum should be removed
    assert_eq!(
        restored_datum_count, pre_datum_count,
        "Datum should be removed after undo"
    );
}

#[test]
fn test_execute_update_shot_command() {
    // Verify UpdateShot modifies shot data correctly
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 270.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 6.0,
            left: None,
            right: None,
            up: None,
            down: None,
            comment: None,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Update multiple fields
    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            azimuth_in: Some(45.0),
            length: Some(20.0),
            left: Some(Some(1.5)),
            right: Some(Some(2.0)),
            up: Some(Some(3.0)),
            down: Some(Some(0.5)),
            comment: Some(Some("Narrow squeeze".to_string())),
            ..Default::default()
        },
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.sections[0].shots[0];

    assert_eq!(shot.azimuth_in, 45.0);
    assert_eq!(shot.azimuth_out, 270.0); // Unchanged
    assert_eq!(shot.length, 20.0);
    assert_eq!(shot.depth_in, 5.0); // Unchanged
    assert_eq!(shot.left, Some(1.5));
    assert_eq!(shot.right, Some(2.0));
    assert_eq!(shot.up, Some(3.0));
    assert_eq!(shot.down, Some(0.5));
    assert_eq!(shot.comment, Some("Narrow squeeze".to_string()));
}

#[test]
fn test_update_shot_rejects_non_finite_nested_field_atomically() {
    // ShotUpdates::all_finite must reject a non-finite value hidden in a nested
    // `Option<Option<f64>>` field (pitch_in), not just the flat `length` case
    // covered elsewhere. The whole update is rejected atomically: a valid field
    // set alongside the poisoned one (azimuth_in) must NOT be partially applied.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 10.0,
            length: 5.0,
            ..Default::default()
        }],
    });
    let section_id = create.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            azimuth_in: Some(45.0),         // valid -- must NOT be applied
            pitch_in: Some(Some(f64::NAN)), // poisoned nested field
            ..Default::default()
        },
    });
    assert!(
        !result.success,
        "a non-finite nested field must be rejected"
    );
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("non-finite"));

    let after = state.get_project_state().unwrap();
    let shot = &after.sections[0].shots[0];
    assert_eq!(
        shot.azimuth_in, 10.0,
        "no partial apply: azimuth_in unchanged"
    );
    assert_eq!(shot.pitch_in, None, "poisoned pitch_in must not be written");
}

#[test]
fn test_update_shot_rejects_positive_infinity() {
    // Only NaN was covered before; +inf must be rejected too (`all_finite`
    // checks `is_finite`, which is false for both).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
    });
    let section_id = create.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            left: Some(Some(f64::INFINITY)),
            ..Default::default()
        },
    });
    assert!(!result.success, "+inf must be rejected");
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("non-finite"));
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots[0].left,
        None,
        "poisoned left must not be written"
    );
}

#[test]
fn test_import_shot_updates_reject_atomically_when_one_is_poisoned() {
    // Batch atomicity: two existing-shot updates where the SECOND carries a
    // non-finite value (+inf). The import validates every update before any
    // mutation, so the whole batch is rejected -- NEITHER shot may change, not
    // even the first (valid) one.
    use crate::models::ExistingShotUpdate;
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });

    let seed = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0), test_shot_data(6.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };
    let r = state.execute_import_sections(
        vec![seed],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "seed import failed: {:?}", r.error);
    let project = state.get_project_state().unwrap();
    let section_id = project.sections[0].id;
    let shot0 = project.sections[0].shots[0].id;
    let shot1 = project.sections[0].shots[1].id;
    let len0 = project.sections[0].shots[0].length;
    let len1 = project.sections[0].shots[1].length;

    let incoming = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Incoming".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(3.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };
    let r = state.execute_import_sections(
        vec![incoming],
        Vec::new(),
        None,
        Vec::new(),
        vec![
            ExistingShotUpdate {
                section_id,
                shot_id: shot0,
                updates: ShotUpdates {
                    length: Some(9.0), // valid
                    ..Default::default()
                },
            },
            ExistingShotUpdate {
                section_id,
                shot_id: shot1,
                updates: ShotUpdates {
                    length: Some(f64::INFINITY), // poisoned
                    ..Default::default()
                },
            },
        ],
        VirtualShotMode::Auto,
    );
    assert!(
        !r.success,
        "a batch with one poisoned update must be rejected"
    );

    let after = state.get_project_state().unwrap();
    assert_eq!(
        after.sections[0].shots[0].length, len0,
        "the first (valid) update must NOT apply when the batch is rejected"
    );
    assert_eq!(
        after.sections[0].shots[1].length, len1,
        "the second (poisoned) update must NOT apply"
    );
}

#[test]
fn doubly_anchored_guard_reports_receipt_warning_and_skips_the_shot() {
    // F12b: a section with zero datums on BOTH sides of a modified shot range
    // is exactly the case the doubly-anchored guard exists for - the update
    // must be silently kept (not applied) AND the receipt must say so, not
    // just log it. The two zero datums are pushed directly onto the project
    // (bypassing the command layer's "one zero datum per connected group"
    // guard, which exists to stop a USER from creating this state through the
    // UI) because the guard under test only cares about the raw
    // `zero_datums` data, and this is the cheapest fixture that produces it.
    use crate::models::ZeroDatum;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(5.0),
                test_shot_data(5.0),
                test_shot_data(5.0),
            ],
        })
        .section_id
        .unwrap();

    let (mid_shot_id, orig_length) = {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        let key_front = project.key_for_ref(StationRef::new(a, 0)).unwrap();
        let key_back = project.key_for_ref(StationRef::new(a, 4)).unwrap();
        let gps = GpsCoordinates::new(10.0, 20.0, 0.0).unwrap();
        project.zero_datums.push(ZeroDatum {
            station_ref: key_front,
            gps: gps.clone(),
        });
        project.zero_datums.push(ZeroDatum {
            station_ref: key_back,
            gps,
        });
        let shot = &project.sections[0].shots[1];
        (shot.id, shot.length)
    };

    // A trivial carrier section: `execute_import_sections_inner` discards
    // `existing_shot_updates` outright when `sections` is empty (the DMP
    // wizard always sends updates alongside the sections they belong to).
    let carrier = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Carrier".to_string()),
        surveyor: None,
        date: None,
        color: "#222222".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(1.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };

    let r = state.execute_import_sections(
        vec![carrier],
        Vec::new(),
        None,
        Vec::new(),
        vec![crate::models::ExistingShotUpdate {
            section_id: a,
            shot_id: mid_shot_id,
            updates: ShotUpdates {
                length: Some(orig_length + 50.0),
                ..Default::default()
            },
        }],
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);

    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert!(
        receipt
            .warnings
            .iter()
            .any(|w| w.contains("anchored on both sides")),
        "receipt warnings must name the doubly-anchored skip, got {:?}",
        receipt.warnings
    );

    let after = state.get_project_state().unwrap();
    let after_shot = after.sections[0]
        .shots
        .iter()
        .find(|s| s.id == mid_shot_id)
        .unwrap();
    assert_eq!(
        after_shot.length, orig_length,
        "the doubly-anchored shot must keep its original length, not the update"
    );
}

#[test]
fn zero_datum_import_placement_sets_receipt_zero_datum_created() {
    // F12c: a section placed via `ImportPlacement::ZeroDatum` must name itself
    // on the receipt, not just in the trace logs.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });

    let config = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("GPS Anchor".to_string()),
        surveyor: None,
        date: None,
        color: "#333333".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::ZeroDatum {
            coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
            station_index: 0,
        },
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };

    let r = state.execute_import_sections(
        vec![config],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);

    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert_eq!(
        receipt.zero_datum_created.as_deref(),
        Some("GPS Anchor"),
        "receipt must name the section a zero datum was created on"
    );
}

#[test]
fn multiple_zero_datum_placements_name_the_first_and_count_the_rest() {
    // F12c/item-9: a batch that creates more than one zero datum must not
    // let naming only the first one read as a claim of completeness.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });

    let make = |name: &str, station_index: u32| ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#333333".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::ZeroDatum {
            coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
            station_index,
        },
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };

    let r = state.execute_import_sections(
        vec![make("First Anchor", 0), make("Second Anchor", 0)],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);

    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert_eq!(
        receipt.zero_datum_created.as_deref(),
        Some("First Anchor (and 1 more)"),
        "must name the first section and count the rest, not claim completeness"
    );
}

#[test]
fn import_with_no_zero_datum_placement_leaves_receipt_field_none() {
    // Negative case for F12c: an ordinary Origin-placed import must not claim
    // a zero datum was created.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "T".to_string(),
        location: None,
    });

    let config = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Plain".to_string()),
        surveyor: None,
        date: None,
        color: "#444444".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };

    let r = state.execute_import_sections(
        vec![config],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);

    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert_eq!(receipt.zero_datum_created, None);
}

#[test]
fn test_execute_delete_shot_command() {
    // Verify DeleteShot removes shot and maintains section integrity
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(9.0),
        ],
    });

    let section_id = create_result.section_id.unwrap();

    // Get the middle shot's ID
    let project = state.get_project_state().unwrap();
    let middle_shot_id = project.sections[0].shots[1].id;
    let first_shot_id = project.sections[0].shots[0].id;
    let last_shot_id = project.sections[0].shots[2].id;

    // Delete the middle shot
    let result = state.execute(Command::DeleteShot {
        section_id,
        shot_id: middle_shot_id,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let section = project.get_section(section_id).unwrap();

    // Should have 2 shots remaining
    assert_eq!(section.shots.len(), 2);

    // First and last shots should still exist
    assert!(section.shots.iter().any(|s| s.id == first_shot_id));
    assert!(section.shots.iter().any(|s| s.id == last_shot_id));

    // Middle shot should be gone
    assert!(!section.shots.iter().any(|s| s.id == middle_shot_id));

    // Verify shot lengths are correct (first and last)
    assert_eq!(section.shots[0].length, 5.0);
    assert_eq!(section.shots[1].length, 9.0);
}

#[test]
fn test_execute_create_shot_command() {
    // Verify CreateShot inserts shot at correct position
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(10.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Get the first shot's ID
    let project = state.get_project_state().unwrap();
    let first_shot_id = project.sections[0].shots[0].id;

    // Insert a new shot after the first shot
    let result = state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(7.5), // Intermediate length
        after_shot_id: Some(first_shot_id),
        before_shot_id: None,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let section = project.get_section(section_id).unwrap();

    // Should have 3 shots now
    assert_eq!(section.shots.len(), 3);

    // Verify order by length: 5.0, 7.5, 10.0
    assert_eq!(section.shots[0].length, 5.0);
    assert_eq!(section.shots[1].length, 7.5);
    assert_eq!(section.shots[2].length, 10.0);
}

#[test]
fn test_execute_delete_section_command() {
    // Verify DeleteSection cleans up merges and related data
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_a_id = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(7.0)],
    });
    let section_b_id = result_b.section_id.unwrap();

    // Set a zero datum on section A
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(section_a_id, 0),
        coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });

    // Verify initial state
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert_eq!(project.zero_datums.len(), 1);

    // Delete section A
    let result = state.execute(Command::DeleteSection {
        section_id: section_a_id,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();

    // Section should be removed
    assert_eq!(project.sections.len(), 1);
    assert!(project.get_section(section_a_id).is_none());
    assert!(project.get_section(section_b_id).is_some());

    // Section B should be the only one remaining
    assert_eq!(project.sections[0].id, section_b_id);
    assert_eq!(project.sections[0].name, Some("Section B".to_string()));
}

#[test]
fn test_execute_update_section_metadata_command() {
    // Verify UpdateSectionMetadata modifies metadata correctly
    use chrono::NaiveDate;
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let initial_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
    let create_result = state.execute(Command::CreateSection {
        name: Some("Original Name".to_string()),
        surveyor: Some("Bob".to_string()),
        date: Some(initial_date),
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Update all metadata fields
    let updated_date = NaiveDate::from_ymd_opt(2024, 2, 20).unwrap();
    let result = state.execute(Command::UpdateSectionMetadata {
        section_id,
        updates: SectionUpdates {
            name: Some(Some("Updated Name".to_string())),
            surveyor: Some(Some("Carol".to_string())),
            date: Some(Some(updated_date)),
            color: Some("#0000ff".to_string()),
            direction: Some(SurveyDirection::Out),
            quality: Some(1.25),
        },
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let section = project.get_section(section_id).unwrap();

    assert_eq!(section.name, Some("Updated Name".to_string()));
    assert_eq!(section.surveyor_names(), Some("Carol".to_string()));
    assert_eq!(section.first_survey_date(), Some(updated_date));
    assert_eq!(section.color, "#0000ff");
    assert_eq!(section.direction, SurveyDirection::Out);
    assert_eq!(section.quality, 1.25);

    // Verify shots were not affected
    assert_eq!(section.shots.len(), 1);
    assert_eq!(section.shots[0].length, 5.0);
}

// === Edge Case Tests for Validation Errors ===

#[test]
fn test_execute_command_no_project_open() {
    let state = AppState::new().unwrap();

    // Try to execute commands without an open project
    let commands_to_test = vec![
        Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        },
        Command::DeleteSection { section_id: 0 },
        Command::UpdateSectionMetadata {
            section_id: 0,
            updates: SectionUpdates::default(),
        },
        Command::CreateShot {
            section_id: 0,
            shot: test_shot_data(5.0),
            after_shot_id: None,
            before_shot_id: None,
        },
        Command::UpdateShot {
            section_id: 0,
            shot_id: 0,
            updates: ShotUpdates::default(),
        },
        Command::DeleteShot {
            section_id: 0,
            shot_id: 0,
        },
        Command::MergeStations {
            station_a: StationRef::new(0, 0),
            station_b: StationRef::new(1, 0),
        },
        Command::SetZeroDatum {
            station_ref: StationRef::new(0, 0),
            coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
        },
    ];

    for cmd in commands_to_test {
        let result = state.execute(cmd);
        assert!(
            !result.success,
            "Command should fail when no project is open"
        );
        assert!(
            result.error.is_some(),
            "Should have error message when no project open"
        );
    }
}

#[test]
fn test_execute_commands_with_invalid_ids() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Test with nonexistent section ID
    let result = state.execute(Command::DeleteSection { section_id: 999 });
    assert!(!result.success);

    let result = state.execute(Command::UpdateSectionMetadata {
        section_id: 999,
        updates: SectionUpdates::default(),
    });
    assert!(!result.success);

    // Test with nonexistent shot ID
    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id: 999_999,
        updates: ShotUpdates::default(),
    });
    assert!(!result.success);

    let result = state.execute(Command::DeleteShot {
        section_id,
        shot_id: 999_999,
    });
    assert!(!result.success);
}

// === Undo/Redo Tests for fn-35-f4v.1 ===

#[test]
fn test_undo_delete_shot() {
    // Verify undoing DeleteShot restores the shot
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(10.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Delete the shot
    state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 1);
    assert_eq!(project.sections[0].shots[0].length, 10.0);

    // Undo the delete
    let result = state.undo();
    assert!(result.success);

    // Shot should be restored
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    assert!(project.sections[0].shots.iter().any(|s| s.length == 5.0));
}

#[test]
fn test_undo_merge_stations() {
    // Verify undoing MergeStations removes the merge
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
    });
    let section_a_id = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0), test_shot_data(6.0)],
    });
    let section_b_id = result_b.section_id.unwrap();

    // Verify no merges exist before
    let project_before = state.get_project_state().unwrap();
    assert!(project_before.station_merges.is_empty());

    // Merge stations
    let station_a = StationRef::new(section_a_id, 1);
    let station_b = StationRef::new(section_b_id, 0);

    let result = state.execute(Command::MergeStations {
        station_a,
        station_b,
    });
    assert!(result.success);

    // Verify merge was created
    let project_after = state.get_project_state().unwrap();
    assert_eq!(project_after.station_merges.len(), 1);
    assert!(project_after.key_is_ref(&project_after.station_merges[0].station_a, station_a));
    assert!(project_after.key_is_ref(&project_after.station_merges[0].station_b, station_b));

    // Undo the merge
    let result = state.undo();
    assert!(result.success);

    // Verify merge is gone
    let project_undone = state.get_project_state().unwrap();
    assert!(project_undone.station_merges.is_empty());
}

#[test]
fn test_multiple_undo_sequence() {
    // Verify multiple undos in LIFO order
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create sections in sequence
    state.execute(Command::CreateSection {
        name: Some("First".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    state.execute(Command::CreateSection {
        name: Some("Second".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    state.execute(Command::CreateSection {
        name: Some("Third".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![],
    });

    assert_eq!(state.get_project_state().unwrap().sections.len(), 3);

    // Undo should remove Third first (LIFO)
    let result = state.undo();
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert!(!project
        .sections
        .iter()
        .any(|s| s.name == Some("Third".to_string())));

    // Undo should remove Second
    let result = state.undo();
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert!(!project
        .sections
        .iter()
        .any(|s| s.name == Some("Second".to_string())));

    // Undo should remove First
    let result = state.undo();
    assert!(result.success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
}

#[test]
fn test_redo_clears_on_new_command() {
    // Verify new command clears redo stack
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section
    state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    // Undo it
    state.undo();
    assert!(state.can_redo());

    // Execute a new command (should clear redo stack)
    state.execute(Command::CreateSection {
        name: Some("New".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    // Redo should no longer be available
    assert!(!state.can_redo());

    // Verify we have the new section, not the original
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].name, Some("New".to_string()));
}

#[test]
fn test_undo_stack_limits() {
    // Verify stack limits and overflow handling
    // Use a small limit to test overflow behavior
    let state = AppState::new().unwrap();

    // Set a small undo history limit via config
    {
        let mut config = state.config.lock().unwrap();
        config.general.undo_history_limit = 3;
    }

    // Reinitialize undo stack with new limit
    {
        let mut undo_stack = state.undo_stack.lock().unwrap();
        undo_stack.set_history_limit(3);
    }

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create 5 sections (more than the limit of 3)
    for i in 0..5 {
        state.execute(Command::CreateSection {
            name: Some(format!("Section {i}")),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        });
    }

    assert_eq!(state.get_project_state().unwrap().sections.len(), 5);

    // We should only be able to undo 3 times (the limit)
    // The first 2 section creates should have been dropped from the stack
    let result1 = state.undo();
    assert!(result1.success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 4);

    let result2 = state.undo();
    assert!(result2.success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 3);

    let result3 = state.undo();
    assert!(result3.success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 2);

    // Fourth undo should fail (stack exhausted)
    let result4 = state.undo();
    assert!(!result4.success);

    // Verify we still have the first 2 sections (they couldn't be undone)
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert!(project
        .sections
        .iter()
        .any(|s| s.name == Some("Section 0".to_string())));
    assert!(project
        .sections
        .iter()
        .any(|s| s.name == Some("Section 1".to_string())));
}

// === Undo/Redo Entry Preservation on Failure Tests (fn-28-rpu.2) ===

#[test]
fn test_undo_failure_preserves_entry_on_undo_stack() {
    // When undo() fails (e.g., invalid JSON in PreviousValues), the entry
    // must be pushed back to the top of the undo stack, not lost.
    let state = AppState::new().unwrap();

    // Push an entry with invalid undo data that will fail during undo
    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::UpdateShot {
            section_id: 1,
            shot_id: 1,
            updates: ShotUpdates::default(),
        },
        UndoData::PreviousValues {
            section_id: 1,
            shot_id: Some(1),
            shot_uuid: None,
            values_json: "not-valid-json".to_string(),
            previous_offsets: HashMap::new(),
        },
    );
    assert_eq!(stack.len(), 1);
    assert!(stack.can_undo());
    assert!(!stack.can_redo());
    drop(stack);

    // Attempt undo - should fail because of invalid JSON
    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to parse undo data"));

    // Entry must still be on the undo stack (preserved, not lost)
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(
        stack.len(),
        1,
        "Entry must remain on undo stack after failure"
    );
    assert!(
        stack.can_undo(),
        "can_undo must be true - entry was preserved"
    );
    // Entry must NOT have moved to redo stack
    assert!(!stack.can_redo(), "Entry must not appear on redo stack");
}

#[test]
fn test_redo_failure_preserves_entry_on_redo_stack() {
    // When redo() fails, the entry must be pushed back to the redo stack.
    let state = AppState::new().unwrap();

    // Push an entry onto the redo stack that will fail when re-executed
    let mut stack = state.undo_stack.lock().unwrap();
    let entry = UndoEntry {
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair {
            before: "{}".to_string(),
            after: "not-valid-json".to_string(),
        },
        timestamp: None,
    };
    stack.push_redo(entry);
    assert_eq!(stack.redo_len(), 1);
    assert!(!stack.can_undo());
    drop(stack);

    // Attempt redo - should fail because "after" is invalid JSON
    let result = state.redo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to parse project snapshot")
    );

    // Entry must still be on the redo stack (preserved, not lost)
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(
        stack.redo_len(),
        1,
        "Entry must remain on redo stack after failure"
    );
    assert!(
        stack.can_redo(),
        "can_redo must be true - entry was preserved"
    );
    // Entry must NOT have moved to undo stack
    assert!(!stack.can_undo(), "Entry must not appear on undo stack");
}

#[test]
fn test_undo_failure_preserves_entry_at_top_lifo_order() {
    // After a failed undo, the entry must be at the TOP of the undo stack.
    // Push two entries: a valid one at the bottom and an invalid one at the top.
    // After the failed undo, the invalid entry must still be on top (LIFO preserved).
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Push a valid section create (will be at bottom of undo stack)
    state.execute(Command::CreateSection {
        name: Some("Valid Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    // Push an invalid entry on top (will fail on undo)
    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.push_with_data(
            Command::UpdateShot {
                section_id: 999,
                shot_id: 999,
                updates: ShotUpdates::default(),
            },
            UndoData::PreviousValues {
                section_id: 999,
                shot_id: Some(999),
                shot_uuid: None,
                values_json: "not-json".to_string(),
                previous_offsets: HashMap::new(),
            },
        );
        // undo stack: [CreateSection, InvalidUpdateShot] (top = InvalidUpdateShot)
        assert_eq!(stack.len(), 2);
    }

    // Attempt undo - the top entry (invalid) should fail
    let result = state.undo();
    assert!(!result.success);

    // The failed entry must be pushed back to the TOP of the undo stack
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(stack.len(), 2, "Both entries must still be on the stack");

    // Peek at the top: it should be the invalid UpdateShot (LIFO preserved)
    let top_cmd = stack.peek_undo().unwrap();
    assert!(
        matches!(
            top_cmd,
            Command::UpdateShot {
                section_id: 999,
                ..
            }
        ),
        "Failed entry must be at TOP of undo stack (LIFO order preserved)"
    );
}

#[test]
fn test_redo_failure_preserves_entry_at_top_lifo_order() {
    // After a failed redo, the entry must be at the TOP of the redo stack.
    let state = AppState::new().unwrap();

    // Push two entries to redo stack: valid at bottom, invalid at top
    let mut stack = state.undo_stack.lock().unwrap();

    // Valid redo entry (bottom)
    stack.push_redo(UndoEntry {
        command_bytes: None,
        command: Command::CreateSection {
            name: Some("Valid".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        },
        undo_data: UndoData::None,
        timestamp: None,
    });

    // Invalid redo entry (top) - will fail because "after" JSON is invalid
    stack.push_redo(UndoEntry {
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair {
            before: "{}".to_string(),
            after: "invalid-json".to_string(),
        },
        timestamp: None,
    });

    assert_eq!(stack.redo_len(), 2);
    drop(stack);

    // Attempt redo - the top entry (invalid) should fail
    let result = state.redo();
    assert!(!result.success);

    // The failed entry must be pushed back to the TOP of the redo stack
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(
        stack.redo_len(),
        2,
        "Both entries must still be on redo stack"
    );

    // Peek at the top: it should be the invalid ProjectSnapshotPair
    let top_cmd = stack.peek_redo().unwrap();
    assert!(
        matches!(top_cmd, Command::CloseProject),
        "Failed entry must be at TOP of redo stack (LIFO order preserved)"
    );
}

#[test]
fn test_undo_to_partial_completion_stops_at_failed_step() {
    // undo_to() calls undo() in a loop. If an intermediate step fails,
    // it should stop and return an enhanced error message with step count.
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create 3 valid sections (3 undo entries)
    for i in 0..3 {
        state.execute(Command::CreateSection {
            name: Some(format!("Section {i}")),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        });
    }

    // Push an invalid entry that will fail when undone
    // This goes on top of the stack, so it's the FIRST to be undone
    // But we want it to fail at step 2 or 3, so we need to place it lower.
    // Strategy: undo the top entry to make room, then insert the invalid entry,
    // then redo to restore the valid one on top.
    //
    // Actually, let's structure it differently:
    // Stack bottom-to-top: [Section0, InvalidEntry, Section2]
    // undo_to(2) will try 3 undos: Section2 (ok), InvalidEntry (fail at step 2)
    {
        // Remove the top two entries, insert invalid, put one back
        let mut stack = state.undo_stack.lock().unwrap();
        let top = stack.pop_undo().unwrap(); // Section 2
        let _middle = stack.pop_undo(); // Section 1

        // Push invalid entry at position where Section 1 was
        stack.push_undo(UndoEntry {
            command_bytes: None,
            command: Command::UpdateShot {
                section_id: 999,
                shot_id: 999,
                updates: ShotUpdates::default(),
            },
            undo_data: UndoData::PreviousValues {
                section_id: 999,
                shot_id: Some(999),
                shot_uuid: None,
                values_json: "invalid-json".to_string(),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        });

        // Push Section 2 back on top
        stack.push_undo(top);
    }

    // Undo stack is now: [Section0, InvalidEntry, Section2]
    // undo_to(2) means execute 3 undos:
    //   Step 1: undo Section2 -> success
    //   Step 2: undo InvalidEntry -> failure
    // Should stop and return enhanced message format
    let result = state.undo_to(2);
    assert!(!result.success);

    let error_msg = result.error.as_deref().unwrap();
    // Verify enhanced message format: "Undo failed at step {current} of {total}: {reason}"
    assert!(
        error_msg.contains("Undo failed at step 2 of 3"),
        "Expected enhanced message with step count, got: {error_msg}"
    );
    assert!(
        error_msg.contains("Failed to parse undo data"),
        "Expected original failure reason in message, got: {error_msg}"
    );

    // Verify partial completion: Section 2 was undone (step 1 succeeded),
    // but Section 0 was not undone (step 2 failed before reaching it).
    // The invalid entry should still be on the undo stack (preserved).
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        2,
        "Only Section 2 should have been undone (step 1); Section 0 remains"
    );
    assert!(project
        .sections
        .iter()
        .any(|s| s.name == Some("Section 0".to_string())));

    // Verify the failed entry was preserved on the undo stack
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(
        stack.len(),
        2,
        "Section 0 and invalid entry must remain on undo stack"
    );
    let top_cmd = stack.peek_undo().unwrap();
    assert!(
        matches!(
            top_cmd,
            Command::UpdateShot {
                section_id: 999,
                ..
            }
        ),
        "Failed invalid entry must be at top of undo stack after partial undo_to"
    );
}

#[test]
fn test_redo_to_partial_completion_stops_at_failed_step() {
    // redo_to() calls redo() in a loop. If an intermediate step fails,
    // it should stop and return an enhanced error message with step count.
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create 2 sections, then undo both so they're on the redo stack
    state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });
    state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    // Undo both: redo stack bottom-to-top = [SectionB, SectionA]
    // (B undone first, then A; LIFO means A is on top for redo)
    state.undo();
    state.undo();
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Now manipulate the redo stack: insert an invalid entry between the two valid ones
    // Redo stack: [SectionA, InvalidEntry, SectionB]
    {
        let mut stack = state.undo_stack.lock().unwrap();
        let top = stack.pop_redo().unwrap(); // SectionA (last undone = first to redo)
                                             // Push invalid entry
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: Command::CloseProject,
            undo_data: UndoData::ProjectSnapshotPair {
                before: "{}".to_string(),
                after: "not-json".to_string(),
            },
            timestamp: None,
        });
        // Put SectionA back on top
        stack.push_redo(top);
        // Redo stack bottom-to-top: [SectionB, InvalidEntry, SectionA]
        assert_eq!(stack.redo_len(), 3);
    }

    // redo_to(2) means execute 3 redos:
    //   Step 1: redo SectionA (top) -> success
    //   Step 2: redo InvalidEntry -> failure
    // Should stop and return enhanced message
    let result = state.redo_to(2);
    assert!(!result.success);

    let error_msg = result.error.as_deref().unwrap();
    assert!(
        error_msg.contains("Redo failed at step 2 of 3"),
        "Expected enhanced message with step count, got: {error_msg}"
    );
    assert!(
        error_msg.contains("Failed to parse project snapshot"),
        "Expected original failure reason in message, got: {error_msg}"
    );

    // Verify partial completion: SectionA was re-created (step 1 succeeded)
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        1,
        "Only SectionA should have been redone (step 1)"
    );
    assert_eq!(project.sections[0].name, Some("Section A".to_string()));

    // Verify the failed entry was preserved on the redo stack
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(
        stack.redo_len(),
        2,
        "Invalid entry and SectionB must remain on redo stack"
    );
    let top_cmd = stack.peek_redo().unwrap();
    assert!(
        matches!(top_cmd, Command::CloseProject),
        "Failed invalid entry must be at top of redo stack after partial redo_to"
    );
}

#[test]
fn test_undo_failure_with_project_snapshot_preserves_entry() {
    // Verify that a ProjectSnapshot undo failure (invalid JSON) preserves the entry
    let state = AppState::new().unwrap();

    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshot("not-valid-json".to_string()),
    );
    assert_eq!(stack.len(), 1);
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to parse project snapshot")
    );

    // Entry must still be on the undo stack
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(stack.len(), 1, "Entry must be preserved on undo stack");
    assert!(!stack.can_redo(), "Entry must not appear on redo stack");
}

#[test]
fn test_undo_failure_with_project_lock_preserves_entry() {
    // When the project mutex is poisoned, undo fails but the entry
    // must still be pushed back to the undo stack.
    let state = AppState::new().unwrap();

    let snapshot = serde_json::to_string(&Project::new("Test".to_string(), None)).unwrap();
    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshot(snapshot),
    );
    assert_eq!(stack.len(), 1);
    drop(stack);

    // Poison the project mutex
    poison_mutex(&state.project);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock project"));

    // Entry must still be preserved on the undo stack
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(
        stack.len(),
        1,
        "Entry must be preserved on undo stack after project lock failure"
    );
    assert!(!stack.can_redo(), "Entry must not appear on redo stack");
}

#[test]
fn test_redo_failure_with_project_lock_preserves_entry() {
    // When the project mutex is poisoned, redo fails but the entry
    // must still be pushed back to the redo stack.
    let state = AppState::new().unwrap();

    let before = serde_json::to_string(&Project::new("Before".to_string(), None)).unwrap();
    let after = serde_json::to_string(&Project::new("After".to_string(), None)).unwrap();

    let entry = UndoEntry {
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair { before, after },
        timestamp: None,
    };

    let mut stack = state.undo_stack.lock().unwrap();
    stack.push_redo(entry);
    assert_eq!(stack.redo_len(), 1);
    drop(stack);

    // Poison the project mutex
    poison_mutex(&state.project);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to lock project"));

    // Entry must still be preserved on the redo stack
    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(
        stack.redo_len(),
        1,
        "Entry must be preserved on redo stack after project lock failure"
    );
    assert!(!stack.can_undo(), "Entry must not appear on undo stack");
}

#[test]
fn test_set_zero_datum_update_captures_previous_gps_and_offsets() {
    // Verify that calling SetZeroDatum on an existing datum captures previous_gps = Some(old)
    // and previous_offsets correctly in the undo data.
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section with shots
    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let station_ref = StationRef::new(section_a, 0);

    // Set initial zero datum with GPS-A
    let gps_a = GpsCoordinates::new(25.0, -80.0, 50.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps_a,
    });
    assert!(result.success, "Initial SetZeroDatum should succeed");

    // Capture the section offsets before the update for later assertion
    let offsets_before_update = {
        let proj = state.project.lock().unwrap();
        let project = proj.as_ref().unwrap();
        project.section_offsets.clone()
    };

    // Clear undo stack to focus on the update
    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.clear();
    }

    // Now update the datum with GPS-B
    let gps_b = GpsCoordinates::new(26.0, -81.0, 55.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps_b,
    });
    assert!(result.success, "Update SetZeroDatum should succeed");

    // Inspect the undo entry's undo_data
    let stack = state.undo_stack.lock().unwrap();
    assert!(stack.can_undo(), "Should have an undo entry");

    let undo_data = stack.peek_undo_data().expect("Should have undo data");
    match undo_data {
        UndoData::ZeroDatumInfo {
            station_ref: ref_in_entry,
            previous_gps,
            previous_offsets,
            ..
        } => {
            assert_eq!(*ref_in_entry, station_ref, "Station ref should match");
            assert!(
                previous_gps.is_some(),
                "previous_gps must be Some(old) when updating existing datum"
            );
            let prev = previous_gps.unwrap();
            assert!(
                (prev.latitude - 25.0).abs() < f64::EPSILON,
                "previous_gps latitude should be GPS-A's latitude (25.0), got {}",
                prev.latitude
            );
            assert!(
                (prev.longitude - (-80.0)).abs() < f64::EPSILON,
                "previous_gps longitude should be GPS-A's longitude (-80.0), got {}",
                prev.longitude
            );
            // Assert previous_offsets contains the datum section and matches the pre-update value
            assert!(
                previous_offsets.contains_key(&section_a),
                "previous_offsets must contain the datum section ({section_a})"
            );
            let captured_offset = previous_offsets.get(&section_a).unwrap();
            let expected_offset = offsets_before_update.get(&section_a).copied();
            assert_eq!(
                *captured_offset, expected_offset,
                "previous_offsets for datum section should match pre-update offset"
            );
        }
        other => panic!(
            "Expected UndoData::ZeroDatumInfo, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn test_set_zero_datum_update_undo_restores_previous_gps_and_offsets() {
    // After updating a zero datum, undo should restore the previous GPS and section offsets.
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let station_ref = StationRef::new(section_a, 0);

    // Set initial zero datum with GPS-A
    let gps_a = GpsCoordinates::new(25.0, -80.0, 50.0).unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps_a,
    });

    // Capture state after initial set (GPS-A applied)
    let (offsets_after_a, gps_after_a) = {
        let proj = state.project.lock().unwrap();
        let project = proj.as_ref().unwrap();
        let offsets = project.section_offsets.clone();
        let gps = project
            .zero_datums
            .iter()
            .find(|zd| project.key_is_ref(&zd.station_ref, station_ref))
            .map(|zd| zd.gps);
        (offsets, gps)
    };
    assert!(gps_after_a.is_some(), "GPS-A should be set");
    assert!(
        (gps_after_a.unwrap().latitude - 25.0).abs() < f64::EPSILON,
        "GPS should be A"
    );

    // Clear undo stack so we only track the update
    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.clear();
    }

    // Update datum with GPS-B
    let gps_b = GpsCoordinates::new(26.0, -81.0, 55.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps_b,
    });
    assert!(result.success);

    // Verify GPS-B is now active
    {
        let proj = state.project.lock().unwrap();
        let project = proj.as_ref().unwrap();
        let current_gps = project
            .zero_datums
            .iter()
            .find(|zd| project.key_is_ref(&zd.station_ref, station_ref))
            .map(|zd| zd.gps)
            .unwrap();
        assert!(
            (current_gps.latitude - 26.0).abs() < f64::EPSILON,
            "GPS should now be B"
        );
    }

    // Undo the update
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    // Verify GPS is restored to A
    let (restored_offsets, restored_gps) = {
        let proj = state.project.lock().unwrap();
        let project = proj.as_ref().unwrap();
        let offsets = project.section_offsets.clone();
        let gps = project
            .zero_datums
            .iter()
            .find(|zd| project.key_is_ref(&zd.station_ref, station_ref))
            .map(|zd| zd.gps);
        (offsets, gps)
    };

    let restored = restored_gps.expect("GPS should be restored after undo");
    assert!(
        (restored.latitude - 25.0).abs() < f64::EPSILON,
        "GPS latitude should be restored to GPS-A (25.0), got {}",
        restored.latitude
    );
    assert!(
        (restored.longitude - (-80.0)).abs() < f64::EPSILON,
        "GPS longitude should be restored to GPS-A (-80.0), got {}",
        restored.longitude
    );

    // Verify section offsets are restored
    assert_eq!(
        restored_offsets, offsets_after_a,
        "Section offsets should be restored to pre-update values"
    );
}

#[test]
fn test_set_zero_datum_no_change_still_creates_undo_entry() {
    // The no-change guard is frontend-only (lat/lon comparison in EditStationDialog).
    // The backend always executes SetZeroDatum and always creates an undo entry.
    // This test confirms that calling SetZeroDatum with the same GPS coordinates
    // still produces an undo entry (the guard must be tested in E2E).
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let station_ref = StationRef::new(section_a, 0);
    let gps = GpsCoordinates::new(25.0, -80.0, 50.0).unwrap();

    // Set initial datum
    state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
    });

    // Clear undo stack
    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.clear();
    }

    // Call SetZeroDatum again with identical GPS
    let same_gps = GpsCoordinates::new(25.0, -80.0, 50.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: same_gps,
    });
    assert!(result.success, "SetZeroDatum with same GPS should succeed");

    // Backend still creates an undo entry (no backend-level no-change guard)
    let stack = state.undo_stack.lock().unwrap();
    assert!(
        stack.can_undo(),
        "Backend should still create an undo entry even with identical GPS; \
         the no-change guard is frontend-only"
    );
}

// === CreateLoopClosure undo/redo tests ===

#[test]
fn test_undo_create_loop_closure() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections
    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_b = result_b.section_id.unwrap();

    // Merge them so they're connected
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a, 1),
        station_b: StationRef::new(section_b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    // Create loop closure
    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(section_a, 0),
        station_b: StationRef::new(section_b, 1),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(closure_result.success, "CreateLoopClosure should succeed");

    // Verify closure exists
    {
        let project = state.get_project_state().unwrap();
        assert_eq!(project.loop_closures.len(), 1, "Should have 1 closure");
    }

    // Undo: closure should be removed
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    {
        let project = state.get_project_state().unwrap();
        assert_eq!(
            project.loop_closures.len(),
            0,
            "Closure should be removed after undo"
        );
    }

    // Redo: closure should be re-added
    let redo_result = state.redo();
    assert!(redo_result.success, "Redo should succeed");

    {
        let project = state.get_project_state().unwrap();
        assert_eq!(
            project.loop_closures.len(),
            1,
            "Closure should be restored after redo"
        );
    }
}

#[test]
fn test_undo_create_loop_closure_has_closure_info() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_b = result_b.section_id.unwrap();

    state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a, 1),
        station_b: StationRef::new(section_b, 0),
    });

    state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(section_a, 0),
        station_b: StationRef::new(section_b, 1),
        closure_strategy: ClosureStrategy::Batch,
    });

    // Check undo stack has ClosureInfo data
    let proj = state.get_project_state().unwrap();
    let stack = state.undo_stack.lock().unwrap();
    let undo_data = stack.peek_undo_data().unwrap();
    match undo_data {
        UndoData::ClosureInfo { closure, .. } => {
            assert!(proj.key_is_ref(&closure.station_a, StationRef::new(section_a, 0)));
            assert!(proj.key_is_ref(&closure.station_b, StationRef::new(section_b, 1)));
            assert!(closure.path_length > 0.0);
        }
        other => panic!("Expected ClosureInfo, got {:?}", other.variant_name()),
    }
}

#[test]
fn test_create_loop_closure_deferred_strategy_is_rejected() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "DistUndo".to_string(),
        location: None,
    });

    let result = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0), test_shot_data(6.0)],
    });
    let section_id = result.section_id.unwrap();

    let before_project = state.get_project_state().unwrap();
    let before_shots = before_project
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots
        .clone();

    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(section_id, 0),
        station_b: StationRef::new(section_id, 2),
        closure_strategy: ClosureStrategy::ProportionalByLength,
    });
    assert!(!closure_result.success);

    let after_project = state.get_project_state().unwrap();
    let after_shots = after_project
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots
        .clone();
    assert!((before_shots[0].length - after_shots[0].length).abs() < 1e-6);
}

#[test]
fn test_undo_unmerge_restores_removed_closures() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_b = result_b.section_id.unwrap();

    let merge_station_a = StationRef::new(section_a, 1);
    let merge_station_b = StationRef::new(section_b, 0);

    // Merge A.1 with B.0
    state.execute(Command::MergeStations {
        station_a: merge_station_a,
        station_b: merge_station_b,
    });

    // Inject a closure matching the merge pair directly into project state.
    // (create_loop_closure won't let us create one for merged-pair stations,
    // so we simulate legacy data by injecting directly.)
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: merge_station_a.into(),
            station_b: merge_station_b.into(),
            path_length: 15.0,
            misclosure_distance: 0.3,
            error_percentage: 2.0,
            horizontal_misclosure: 0.2,
            vertical_misclosure: 0.1,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
    }

    {
        let project = state.get_project_state().unwrap();
        assert_eq!(
            project.loop_closures.len(),
            1,
            "Should have 1 closure before unmerge"
        );
    }

    // Unmerge the stations - should remove closures matching the merge pair (A.1, B.0)
    let unmerge_result = state.execute(Command::UnmergeStations {
        station_ref: merge_station_a,
    });
    assert!(unmerge_result.success, "Unmerge should succeed");

    {
        let project = state.get_project_state().unwrap();
        assert!(project.station_merges.is_empty(), "Merge should be removed");
        assert_eq!(
            project.loop_closures.len(),
            0,
            "Closure matching merge pair should be removed by unmerge"
        );
    }

    // Undo the unmerge - should restore both the merge AND the removed closure
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo unmerge should succeed");

    {
        let project = state.get_project_state().unwrap();
        assert_eq!(project.station_merges.len(), 1, "Merge should be restored");
        assert_eq!(
            project.loop_closures.len(),
            1,
            "Closure should be restored after undo of unmerge"
        );
        let restored_closure = &project.loop_closures[0];
        assert!(project.key_is_ref(&restored_closure.station_a, merge_station_a));
        assert!(project.key_is_ref(&restored_closure.station_b, merge_station_b));
        assert!((restored_closure.path_length - 15.0).abs() < 1e-6);
    }
}

// === Move Station Tests ===

#[test]
fn test_move_station_updates_shot_data() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create section with a shot pointing due east (azimuth 90°), length 10
    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 1); // last station

    // Move station 1 by (5, 0, 0) - extend eastward
    // Shot 0: vector was (10, 0, 0), now (15, 0, 0)
    let result = state.execute(Command::MoveStation {
        station_ref,
        offset: Coordinates::new(5.0, 0.0, 0.0),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 15.0).abs() < 1e-6,
        "Length should be 15.0, got {}",
        shot.length
    );
    assert!(
        (shot.azimuth_in - 90.0).abs() < 1e-6,
        "Azimuth should still be 90°"
    );
    assert!(
        !project.station_offset_for_ref(station_ref).is_some(),
        "Station offset should be cleared"
    );

    // Second move: (0, 10, 3) - shot vector becomes (15, 10, 3)
    let result = state.execute(Command::MoveStation {
        station_ref,
        offset: Coordinates::new(0.0, 10.0, 3.0),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    let expected_len = (15.0_f64.powi(2) + 10.0_f64.powi(2) + 3.0_f64.powi(2)).sqrt(); // slope length
    assert!(
        (shot.length - expected_len).abs() < 1e-6,
        "Length should be {}, got {}",
        expected_len,
        shot.length
    );
    let expected_azi = (15.0_f64).atan2(10.0).to_degrees(); // atan2(dx, dy)
    assert!(
        (shot.azimuth_in - expected_azi).abs() < 1e-6,
        "Azimuth should be {:.1}°, got {:.1}°",
        expected_azi,
        shot.azimuth_in
    );
    assert!(
        (shot.depth_out - 3.0).abs() < 1e-6,
        "Depth out should be 3.0, got {}",
        shot.depth_out
    );
}

#[test]
fn test_move_station_zero_updates_section_offset() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create section with a shot pointing due east (azimuth 90°), length 10
    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let station_0 = StationRef::new(section_id, 0);
    let station_1 = StationRef::new(section_id, 1);

    let before = state.get_project_state().unwrap();
    let before_station_1 = before.compute_station_raw_coords(station_1).unwrap();

    // Move station 0 by (5, 0, 0) - should shift section offset and keep station 1 fixed
    let result = state.execute(Command::MoveStation {
        station_ref: station_0,
        offset: Coordinates::new(5.0, 0.0, 0.0),
    });
    assert!(result.success);

    let after = state.get_project_state().unwrap();
    let after_station_0 = after.compute_station_raw_coords(station_0).unwrap();
    let after_station_1 = after.compute_station_raw_coords(station_1).unwrap();

    assert!(
        (after_station_0.x - 5.0).abs() < 1e-6,
        "Station 0 x should be 5.0"
    );
    assert!(
        (after_station_0.y).abs() < 1e-6,
        "Station 0 y should remain 0.0"
    );
    assert!(
        (after_station_1.x - before_station_1.x).abs() < 1e-6,
        "Station 1 x should remain unchanged"
    );
    assert!(
        (after_station_1.y - before_station_1.y).abs() < 1e-6,
        "Station 1 y should remain unchanged"
    );

    let offset = after
        .section_offsets
        .get(&section_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (offset.0 - 5.0).abs() < 1e-6,
        "Section offset x should be 5.0"
    );
    assert!(
        (offset.1).abs() < 1e-6,
        "Section offset y should remain 0.0"
    );
}

#[test]
fn test_move_station_undo() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Due east shot, length 10
    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 0); // first station

    // Move station 0 by (5, 0, 0) - following shot vector -= (5, 0, 0) → (5, 0, 0)
    state.execute(Command::MoveStation {
        station_ref,
        offset: Coordinates::new(5.0, 0.0, 0.0),
    });

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 5.0).abs() < 1e-6,
        "Shot length after move should be 5.0"
    );

    // Undo should restore original shot data
    let undo_result = state.undo();
    assert!(undo_result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 10.0).abs() < 1e-6,
        "Undo should restore length to 10.0, got {}",
        shot.length
    );
    assert!(
        (shot.azimuth_in - 90.0).abs() < 1e-6,
        "Undo should restore azimuth_in to 90.0"
    );
    assert!(
        (shot.azimuth_out - 90.0).abs() < 1e-6,
        "Undo should restore azimuth_out to 90.0"
    );
    assert!(
        (shot.depth_in - 0.0).abs() < 1e-6,
        "Undo should restore depth_in to 0.0"
    );
    assert!(
        (shot.depth_out - 0.0).abs() < 1e-6,
        "Undo should restore depth_out to 0.0"
    );
}

#[test]
fn test_move_station_redo() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Due east shot, length 10
    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 1); // last station

    // Move station 1 by (5, 0, 0) - preceding shot vector += (5, 0, 0) → (15, 0, 0)
    state.execute(Command::MoveStation {
        station_ref,
        offset: Coordinates::new(5.0, 0.0, 0.0),
    });

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 15.0).abs() < 1e-6,
        "After move: length should be 15.0"
    );

    // Undo
    let undo_result = state.undo();
    assert!(undo_result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 10.0).abs() < 1e-6,
        "After undo: length should be 10.0"
    );

    // Redo should re-apply the move
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 15.0).abs() < 1e-6,
        "After redo: length should be 15.0, got {}",
        shot.length
    );
    assert!(
        (shot.azimuth_in - 90.0).abs() < 1e-6,
        "After redo: azimuth should be 90°"
    );
}

#[test]
fn test_station_offset_applied_in_coordinates() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 1); // end station of the shot

    // Compute coordinates before any move
    let coords_before = {
        let project = state.get_project_state().unwrap();
        project.compute_station_raw_coords(station_ref).unwrap()
    };

    // Move the station
    let move_offset = Coordinates::new(100.0, 200.0, 50.0);
    let result = state.execute(Command::MoveStation {
        station_ref,
        offset: move_offset,
    });
    assert!(result.success);

    // Compute coordinates after move
    let coords_after = {
        let project = state.get_project_state().unwrap();
        project.compute_station_raw_coords(station_ref).unwrap()
    };

    // The station coordinates should differ by exactly the move offset
    let dx = coords_after.x - coords_before.x;
    let dy = coords_after.y - coords_before.y;
    let dz = coords_after.z - coords_before.z;

    assert!(
        (dx - 100.0).abs() < 1e-6,
        "X coordinate should shift by 100.0, got delta {dx}"
    );
    assert!(
        (dy - 200.0).abs() < 1e-6,
        "Y coordinate should shift by 200.0, got delta {dy}"
    );
    assert!(
        (dz - 50.0).abs() < 1e-6,
        "Z coordinate should shift by 50.0, got delta {dz}"
    );
}

// === CreateSection undo/redo ID preservation tests ===

#[test]
fn test_create_section_undo_redo_preserves_id() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section with shots
    let create_result = state.execute(Command::CreateSection {
        name: Some("ID Preservation".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(10.0)],
    });

    let original_section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].id, original_section_id);
    let original_shot_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(original_shot_ids.len(), 2);

    // Undo the create
    let undo_result = state.undo();
    assert!(undo_result.success);
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Redo the create - should preserve original section ID
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(
        project.sections[0].id, original_section_id,
        "Section ID must be preserved after undo/redo, expected {} got {}",
        original_section_id, project.sections[0].id
    );
    assert_eq!(
        project.sections[0].name,
        Some("ID Preservation".to_string())
    );

    // Verify shot IDs are preserved too
    let restored_shot_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(
        restored_shot_ids, original_shot_ids,
        "Shot IDs must be preserved after undo/redo"
    );
}

#[test]
fn test_create_section_double_undo_redo_cycle() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section with shots
    let create_result = state.execute(Command::CreateSection {
        name: Some("Double Cycle".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(3.0),
            test_shot_data(7.0),
            test_shot_data(11.0),
        ],
    });

    let original_section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let original_shot_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();

    // First cycle: undo -> redo
    assert!(state.undo().success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
    assert!(state.redo().success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].id, original_section_id);
    let shot_ids_after_first: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(shot_ids_after_first, original_shot_ids);

    // Second cycle: undo -> redo
    assert!(state.undo().success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
    assert!(state.redo().success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections[0].id, original_section_id,
        "Section ID must be stable through double undo/redo cycle"
    );
    let shot_ids_after_second: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(
        shot_ids_after_second, original_shot_ids,
        "Shot IDs must be stable through double undo/redo cycle"
    );
}

#[test]
fn test_create_section_undo_redo_then_edit() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Editable".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Undo and redo
    assert!(state.undo().success);
    assert!(state.redo().success);

    // Verify we can edit the restored section
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    let update_result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(99.0),
            ..Default::default()
        },
    });
    assert!(update_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots[0].length, 99.0);
}

#[test]
fn test_create_section_undo_redo_then_delete() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Deletable".to_string()),
        surveyor: None,
        date: None,
        color: "#ff00ff".to_string(),
        direction: None,
        shots: vec![],
    });

    let section_id = create_result.section_id.unwrap();

    // Undo and redo
    assert!(state.undo().success);
    assert!(state.redo().success);

    // Delete the restored section
    let delete_result = state.execute(Command::DeleteSection { section_id });
    assert!(delete_result.success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
}

#[test]
fn test_create_section_created_snapshot_distinct_from_section_snapshot() {
    // Verify that CreateSection uses CreatedSectionSnapshot while
    // DeleteSection still uses SectionSnapshot
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section
    let create_result = state.execute(Command::CreateSection {
        name: Some("Check Variant".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(1.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Check that CreateSection uses CreatedSectionSnapshot
    {
        let stack = state.undo_stack.lock().unwrap();
        let undo_data = stack.peek_undo_data().unwrap();
        assert_eq!(
            undo_data.variant_name(),
            "CreatedSectionSnapshot",
            "CreateSection should use CreatedSectionSnapshot, got {}",
            undo_data.variant_name()
        );
    }

    // Now delete the section and verify DeleteSection uses ProjectSnapshotPair
    // (changed from SectionSnapshot to support cleanup of orphaned merges/closures/offsets)
    state.execute(Command::DeleteSection { section_id });

    {
        let stack = state.undo_stack.lock().unwrap();
        let undo_data = stack.peek_undo_data().unwrap();
        assert_eq!(
            undo_data.variant_name(),
            "ProjectSnapshotPair",
            "DeleteSection should use ProjectSnapshotPair, got {}",
            undo_data.variant_name()
        );
    }
}

#[test]
fn test_delete_section_undo_redo_still_works() {
    // Regression test: ensure DeleteSection undo/redo behavior is unchanged
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Delete Test".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Delete the section
    state.execute(Command::DeleteSection { section_id });
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Undo should restore it
    assert!(state.undo().success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].id, section_id);
    assert_eq!(project.sections[0].name, Some("Delete Test".to_string()));

    // Redo should delete it again
    assert!(state.redo().success);
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Undo again to restore
    assert!(state.undo().success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
}

// === CreateShot undo/redo ID preservation tests ===

#[test]
fn test_create_shot_undo_redo_preserves_id() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section first
    let section_result = state.execute(Command::CreateSection {
        name: Some("Shot Test".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = section_result.section_id.unwrap();

    // Get the initial shot ID
    let project = state.get_project_state().unwrap();
    let initial_shot_id = project.sections[0].shots[0].id;

    // Create a new shot
    state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(10.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    let new_shot_id = project.sections[0].shots[1].id;
    assert_ne!(new_shot_id, initial_shot_id);

    // Undo the create shot
    let undo_result = state.undo();
    assert!(undo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 1);

    // Redo the create shot - should preserve original shot ID
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    assert_eq!(
        project.sections[0].shots[1].id, new_shot_id,
        "Shot ID must be preserved after undo/redo, expected {} got {}",
        new_shot_id, project.sections[0].shots[1].id
    );
    assert_eq!(project.sections[0].shots[1].length, 10.0);
}

#[test]
fn test_create_shot_mid_section_undo_redo_preserves_position() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section with 3 shots
    let section_result = state.execute(Command::CreateSection {
        name: Some("Mid Insert".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(1.0),
            test_shot_data(2.0),
            test_shot_data(3.0),
        ],
    });
    let section_id = section_result.section_id.unwrap();

    let project = state.get_project_state().unwrap();
    let first_shot_id = project.sections[0].shots[0].id;
    let original_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(original_ids.len(), 3);

    // Insert a shot after the first shot (mid-section insert)
    state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(99.0),
        after_shot_id: Some(first_shot_id),
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 4);
    // The new shot should be at position 1 (after first)
    let inserted_shot_id = project.sections[0].shots[1].id;
    assert_eq!(project.sections[0].shots[1].length, 99.0);

    // Verify order: [original[0], new, original[1], original[2]]
    assert_eq!(project.sections[0].shots[0].id, original_ids[0]);
    assert_eq!(project.sections[0].shots[1].id, inserted_shot_id);
    assert_eq!(project.sections[0].shots[2].id, original_ids[1]);
    assert_eq!(project.sections[0].shots[3].id, original_ids[2]);

    // Undo the mid-section insert
    let undo_result = state.undo();
    assert!(undo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 3);
    // Should be back to original order
    let after_undo_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(after_undo_ids, original_ids);

    // Redo - should restore at position 1 with original ID
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 4);
    assert_eq!(
        project.sections[0].shots[1].id, inserted_shot_id,
        "Shot ID must be preserved after mid-section undo/redo"
    );
    assert_eq!(project.sections[0].shots[1].length, 99.0);

    // Verify correct position restored: [original[0], new, original[1], original[2]]
    assert_eq!(project.sections[0].shots[0].id, original_ids[0]);
    assert_eq!(project.sections[0].shots[2].id, original_ids[1]);
    assert_eq!(project.sections[0].shots[3].id, original_ids[2]);
}

#[test]
fn test_create_shot_double_undo_redo_cycle() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let section_result = state.execute(Command::CreateSection {
        name: Some("Double Cycle".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = section_result.section_id.unwrap();

    // Create a shot
    state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(20.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    let original_shot_id = project.sections[0].shots[1].id;

    // First cycle: undo -> redo
    assert!(state.undo().success);
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        1
    );
    assert!(state.redo().success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots[1].id, original_shot_id);

    // Second cycle: undo -> redo
    assert!(state.undo().success);
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        1
    );
    assert!(state.redo().success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections[0].shots[1].id, original_shot_id,
        "Shot ID must be stable through double undo/redo cycle"
    );
    assert_eq!(project.sections[0].shots[1].length, 20.0);
}

#[test]
fn test_create_shot_undo_redo_then_edit() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let section_result = state.execute(Command::CreateSection {
        name: Some("Edit After Redo".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });
    let section_id = section_result.section_id.unwrap();

    // Create a shot
    state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(15.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Undo and redo
    assert!(state.undo().success);
    assert!(state.redo().success);

    // Verify we can edit the restored shot
    let update_result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(42.0),
            ..Default::default()
        },
    });
    assert!(update_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots[0].length, 42.0);
}

#[test]
fn test_create_shot_undo_redo_then_delete() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let section_result = state.execute(Command::CreateSection {
        name: Some("Delete After Redo".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });
    let section_id = section_result.section_id.unwrap();

    // Create a shot
    state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(15.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Undo and redo
    assert!(state.undo().success);
    assert!(state.redo().success);

    // Delete the restored shot
    let delete_result = state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });
    assert!(delete_result.success);
    assert!(state.get_project_state().unwrap().sections[0]
        .shots
        .is_empty());
}

#[test]
fn test_create_shot_created_snapshot_distinct_from_shot_snapshot() {
    // Verify that CreateShot uses CreatedShotSnapshot while
    // DeleteShot uses ProjectSnapshotPair
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let section_result = state.execute(Command::CreateSection {
        name: Some("Variant Check".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = section_result.section_id.unwrap();

    // Create a shot
    state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(10.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[1].id;

    // Check that CreateShot uses CreatedShotSnapshot
    {
        let stack = state.undo_stack.lock().unwrap();
        let undo_data = stack.peek_undo_data().unwrap();
        assert_eq!(
            undo_data.variant_name(),
            "CreatedShotSnapshot",
            "CreateShot should use CreatedShotSnapshot, got {}",
            undo_data.variant_name()
        );
    }

    // Now delete the shot and verify DeleteShot uses ProjectSnapshotPair
    // (changed from ShotSnapshot to handle station ref shifting correctly)
    state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });

    {
        let stack = state.undo_stack.lock().unwrap();
        let undo_data = stack.peek_undo_data().unwrap();
        assert_eq!(
            undo_data.variant_name(),
            "ProjectSnapshotPair",
            "DeleteShot should use ProjectSnapshotPair, got {}",
            undo_data.variant_name()
        );
    }
}

#[test]
fn test_insert_shot_before_first_position_and_undo_redo() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let section_result = state.execute(Command::CreateSection {
        name: Some("Before Insert".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0), test_shot_data(20.0)],
    });
    let section_id = section_result.section_id.unwrap();

    let project = state.get_project_state().unwrap();
    let first_shot_id = project.sections[0].shots[0].id;

    // Insert before the first shot (position 0 / prepend)
    let result = state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(result.success);
    assert!(
        result.shot_id.is_some(),
        "shot_id must be returned on success"
    );
    let new_shot_id = result.shot_id.unwrap();

    // Verify insertion at position 0
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 3);
    assert_eq!(
        project.sections[0].shots[0].length, 5.0,
        "Prepended shot should be first"
    );
    assert_eq!(project.sections[0].shots[0].id, new_shot_id);
    assert_eq!(project.sections[0].shots[1].length, 10.0);
    assert_eq!(project.sections[0].shots[2].length, 20.0);

    // Undo the prepend
    let undo_result = state.undo();
    assert!(undo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    assert_eq!(project.sections[0].shots[0].length, 10.0);
    assert_eq!(project.sections[0].shots[1].length, 20.0);

    // Redo the prepend - should restore at position 0 with same ID
    let redo_result = state.redo();
    assert!(redo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 3);
    assert_eq!(project.sections[0].shots[0].length, 5.0);
    assert_eq!(
        project.sections[0].shots[0].id, new_shot_id,
        "Shot ID must be preserved after undo/redo"
    );
}

#[test]
fn test_prepend_shot_undo_preserves_geometry() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let section_result = state.execute(Command::CreateSection {
        name: Some("Prepend Geometry".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });
    let section_id = section_result.section_id.unwrap();

    let before = state.get_project_state().unwrap();
    let before_station_0 = before
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    let before_station_1 = before
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    let first_shot_id = before.sections[0].shots[0].id;

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(result.success);

    let after_prepend = state.get_project_state().unwrap();
    let station_1_after = after_prepend
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    let station_2_after = after_prepend
        .compute_station_raw_coords(StationRef::new(section_id, 2))
        .unwrap();
    assert_coords_close(station_1_after, before_station_0);
    assert_coords_close(station_2_after, before_station_1);

    let undo_result = state.undo();
    assert!(undo_result.success);
    let after_undo = state.get_project_state().unwrap();
    let undo_station_0 = after_undo
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    let undo_station_1 = after_undo
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    assert_coords_close(undo_station_0, before_station_0);
    assert_coords_close(undo_station_1, before_station_1);
}

#[test]
fn test_create_shot_rejects_both_after_and_before() {
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let section_result = state.execute(Command::CreateSection {
        name: Some("Reject Both".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0), test_shot_data(20.0)],
    });
    let section_id = section_result.section_id.unwrap();

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: test_shot_data(5.0),
        after_shot_id: Some(0),
        before_shot_id: Some(1),
    });
    assert!(!result.success);
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("Cannot specify both"));
}

#[test]
fn test_delete_shot_undo_redo_still_works() {
    // Regression test: ensure DeleteShot undo/redo behavior is unchanged
    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let section_result = state.execute(Command::CreateSection {
        name: Some("Delete Shot Regression".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(10.0)],
    });
    let section_id = section_result.section_id.unwrap();

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Delete the first shot
    state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        1
    );

    // Undo should restore it at its original position
    assert!(state.undo().success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    assert_eq!(project.sections[0].shots[0].id, shot_id);
    assert_eq!(project.sections[0].shots[0].length, 5.0);

    // Redo should delete it again
    assert!(state.redo().success);
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        1
    );

    // Undo again to restore
    assert!(state.undo().success);
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        2
    );
}

// === Zero datum merge validation tests ===

#[test]
fn test_check_merge_connectivity_both_zero_datums_blocked() {
    // Both sections have zero datum → can_merge=false
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "BothZD".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Set zero datum on both sections
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(b, 0),
        coordinates: GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
    });

    let result = state
        .check_merge_connectivity(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    assert!(
        !result.can_merge,
        "Both sections with zero datums should not be mergeable"
    );
    assert!(
        !result.connected,
        "Unconnected sections should not be connected"
    );
    assert_eq!(
        result.reason.as_deref(),
        Some("Both sections have fixed coordinates"),
    );
}

#[test]
fn test_check_merge_connectivity_one_zero_datum_allowed() {
    // Only one section has zero datum → can_merge=true
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "OneZD".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Set zero datum on section A only
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });

    let result = state
        .check_merge_connectivity(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    assert!(
        result.can_merge,
        "One section with zero datum should allow merge"
    );
    assert!(!result.connected);
    assert!(result.reason.is_none());
}

#[test]
fn test_check_merge_connectivity_neither_zero_datum_no_loop() {
    // Neither section has zero datum, no loop → can_merge=true
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "NoZD".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let b = result_b.section_id.unwrap();

    let result = state
        .check_merge_connectivity(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    assert!(
        result.can_merge,
        "Neither having zero datum and no loop should allow merge"
    );
    assert!(!result.connected);
    assert!(result.reason.is_none());
}

#[test]
fn test_check_merge_connectivity_connected_creates_loop() {
    // Connected sections (neither has zero datum, loop exists) → can_merge=false
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Loop".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Merge A.1 to B.0 to make them connected
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });

    // Now check connectivity between A.0 and B.1 - they are connected via merge
    let result = state
        .check_merge_connectivity(StationRef::new(a, 0), StationRef::new(b, 1))
        .unwrap();

    assert!(
        !result.can_merge,
        "Connected sections should not be mergeable (creates loop)"
    );
    assert!(result.connected, "Merged sections should be connected");
    assert_eq!(result.reason.as_deref(), Some("Creates survey loop"),);
    assert!(
        result.preview.is_some(),
        "Connected sections should have closure preview"
    );
}

#[test]
fn test_check_merge_connectivity_both_zd_and_connected() {
    // When sections are connected AND both have zero datums, zero datum
    // takes priority for reason, but connected/preview remain correct.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "BothZDConnected".to_string(),
        location: None,
    });

    // Create sections A (with ZD), C (no ZD, will bridge), B (with ZD)
    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let b = result_b.section_id.unwrap();

    let result_c = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let c = result_c.section_id.unwrap();

    // Set ZD on A
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });

    // Set ZD on B (separate group)
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(b, 0),
        coordinates: GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
    });

    // Merge C.0 into A.1 (C joins A's merge group)
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(c, 0),
    });

    // Connect C.1 to B.0 via an injected closure (can't merge - different ZD groups,
    // and can't create closure through command because they're not yet connected).
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: StationRef::new(c, 1).into(),
            station_b: StationRef::new(b, 0).into(),
            path_length: 8.0,
            misclosure_distance: 0.2,
            error_percentage: 2.5,
            horizontal_misclosure: 0.2,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Now: A-C merged (A has ZD), B has ZD, connected via closure C.1-B.0
    let result = state
        .check_merge_connectivity(StationRef::new(a, 0), StationRef::new(b, 1))
        .unwrap();

    // connected must be true (they ARE connected via merge+closure)
    assert!(
        result.connected,
        "Sections connected via merge+closure should report connected=true"
    );
    assert!(
        result.preview.is_some(),
        "Connected stations should have closure preview"
    );
    // can_merge must be false (both have ZD - zero datum takes priority)
    assert!(
        !result.can_merge,
        "Both sections with ZD should not be mergeable"
    );
    assert_eq!(
        result.reason.as_deref(),
        Some("Creates survey loop"),
        "Connected sections should report loop reason"
    );
}

// === Full pipeline: transitive closures through AppState ===

#[test]
fn test_transitive_closures_in_computed_stations() {
    // Test the full pipeline: create sections, merge, create closure, then
    // verify closures_with (including transitive) appears in computed stations.
    //
    // Setup: 3 sections A, B, C
    //   Merge: A.1 ↔ B.0 (connects A and B)
    //   Merge: B.1 ↔ C.0 (connects B and C)
    //   Closure: A.0 ↔ C.1 (loop closure between endpoints)
    //
    // Expected closures_with:
    //   A.0: direct closure to C.1 (A.0 is endpoint, not merged)
    //   C.1: direct closure to A.0 (C.1 is endpoint, not merged)
    //   No transitive closures since neither endpoint is in a merge group.
    //
    // Then add ANOTHER closure: A.1 ↔ C.1
    //   A.1 is merged with B.0, so:
    //   A.1: direct closure to C.1
    //   B.0: transitive closure to C.1 (via A.1)
    //   C.1: direct closure to A.1, transitive closure to B.0 (via A.1)
    //   C.0 is merged with B.1, so:
    //   C.1: is endpoint, not in merge group for this closure
    //   Wait - C.1 is NOT merged with anything. C.0 is merged with B.1.
    //   So C.1 gets direct closure to A.1, and transitive closure to B.0 (via A.1).

    let state = AppState::new().unwrap();

    state.execute(Command::CreateProject {
        name: "TransitiveTest".to_string(),
        location: None,
    });

    // Create section A with 2 shots (stations: A.0, A.1, A.2)
    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0), test_shot_data(8.0)],
    });
    let a = result_a.section_id.unwrap();

    // Create section B with 1 shot (stations: B.0, B.1)
    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Create section C with 1 shot (stations: C.0, C.1)
    let result_c = state.execute(Command::CreateSection {
        name: Some("C".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let c = result_c.section_id.unwrap();

    // Merge A.2 ↔ B.0
    let merge1 = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge1.success, "Merge A.2↔B.0 should succeed");

    // Merge B.1 ↔ C.0
    let merge2 = state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });
    assert!(merge2.success, "Merge B.1↔C.0 should succeed");

    // Now create a loop closure between A.1 and C.1
    // (A.1 is in merge group with B.0, so B.0 should get transitive closure)
    // Wait - A.1 is NOT merged. A.2 is merged with B.0. Let me fix: use A.2 as endpoint.
    // Actually, let me just inject the closure directly since the CreateLoopClosure command
    // requires specific connectivity conditions.
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(a, 2).into(), // A.2 (merged with B.0)
            station_b: StationRef::new(c, 1).into(), // C.1 (not merged with anything)
            path_length: 20.0,
            misclosure_distance: 0.5,
            error_percentage: 2.5,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.3,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Get computed stations through the full pipeline
    let project = state.get_project_state().unwrap();
    let stations = project.compute_station_coordinates();

    // A.2: direct closure to C.1 (A.2 is the endpoint)
    let a2 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 2))
        .unwrap();
    let a2_direct = a2
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 1) && !cr.is_transitive);
    assert!(
        a2_direct.is_some(),
        "A.2 should have direct closure to C.1, got: {:?}",
        a2.closures()
    );

    // B.0: transitive closure to C.1 via A.2 (B.0 is merged with A.2)
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    let b0_transitive = b0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 1) && cr.is_transitive);
    assert!(
        b0_transitive.is_some(),
        "B.0 should have transitive closure to C.1 (via A.2), got: {:?}",
        b0.closures()
    );
    assert_eq!(
        b0_transitive.unwrap().via_station,
        Some(StationRef::new(a, 2))
    );

    // C.1: direct closure to A.2
    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    let c1_direct = c1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(a, 2) && !cr.is_transitive);
    assert!(
        c1_direct.is_some(),
        "C.1 should have direct closure to A.2, got: {:?}",
        c1.closures()
    );

    // C.1: transitive closure to B.0 (via A.2, since B.0 is in A.2's merge group)
    let c1_transitive = c1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(b, 0) && cr.is_transitive);
    assert!(
        c1_transitive.is_some(),
        "C.1 should have transitive closure to B.0 (via A.2), got: {:?}",
        c1.closures()
    );
    assert_eq!(
        c1_transitive.unwrap().via_station,
        Some(StationRef::new(a, 2))
    );

    // A.0 and A.1: should NOT have any closures (not endpoints, not merged with endpoints)
    let a0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap();
    assert!(
        a0.closures().is_empty(),
        "A.0 should have no closures, got: {:?}",
        a0.closures()
    );
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    assert!(
        a1.closures().is_empty(),
        "A.1 should have no closures, got: {:?}",
        a1.closures()
    );

    // B.1: NOT in A.2's merge group, IS merged with C.0 but C.0 is not a closure endpoint
    let b1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 1))
        .unwrap();
    assert!(
        b1.closures().is_empty(),
        "B.1 should have no closures, got: {:?}",
        b1.closures()
    );

    // C.0: merged with B.1, but B.1 is not a closure endpoint. C.0 is NOT in C.1's group.
    let c0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 0))
        .unwrap();
    assert!(
        c0.closures().is_empty(),
        "C.0 should have no closures, got: {:?}",
        c0.closures()
    );
}

#[test]
fn test_transitive_closures_serialization_preserves_data() {
    // Test that closures_with survives JSON serialization (the Tauri IPC path).
    // This tests that `#[serde(default, skip_serializing_if =
    // "closures_absent_or_empty")]` skips ONLY the empty case and never costs a
    // non-empty list any data.
    use crate::models::Station;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "SerTest".to_string(),
        location: None,
    });

    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Merge A.1 ↔ B.0
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });

    // Inject a closure between A.1 and... we need a third section
    let result_c = state.execute(Command::CreateSection {
        name: Some("C".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let c = result_c.section_id.unwrap();

    // Merge B.1 ↔ C.0 to connect everything
    state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });

    // Inject closure A.1 ↔ C.1
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: StationRef::new(a, 1).into(),
            station_b: StationRef::new(c, 1).into(),
            path_length: 15.0,
            misclosure_distance: 0.3,
            error_percentage: 2.0,
            horizontal_misclosure: 0.2,
            vertical_misclosure: 0.2,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Get stations through the normal pipeline
    let project = state.get_project_state().unwrap();
    let stations = project.compute_station_coordinates();

    // Verify B.0 has transitive closure (it's merged with A.1)
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    assert!(
        !b0.closures().is_empty(),
        "B.0 should have transitive closures"
    );

    // Now serialize to JSON and deserialize back (simulating Tauri IPC)
    let json = serde_json::to_string(&stations).unwrap();
    let deserialized: Vec<Station> = serde_json::from_str(&json).unwrap();

    // Find B.0 in deserialized
    let b0_de = deserialized
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    assert_eq!(
        b0_de.closures().len(),
        b0.closures().len(),
        "closures_with should survive serialization round-trip"
    );
    assert!(
        b0_de.closures().iter().any(|cr| cr.is_transitive),
        "Transitive closure should survive serialization"
    );

    // Verify A.1 has direct closure
    let a1_de = deserialized
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    assert!(
        a1_de.closures().iter().any(|cr| !cr.is_transitive),
        "Direct closure on A.1 should survive serialization"
    );

    // Verify C.1 has closures (direct + transitive)
    let c1_de = deserialized
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    assert!(
        c1_de.closures().iter().any(|cr| !cr.is_transitive),
        "C.1 direct closure should survive serialization"
    );
    assert!(
        c1_de.closures().iter().any(|cr| cr.is_transitive),
        "C.1 transitive closure should survive serialization"
    );

    // #441, the other half of the wire contract: a station with NO closures omits
    // the key entirely, so the generated type has to declare it optional. As a
    // plain `Vec` it generated as a required `Array<ClosureRef>` and `tsc` could
    // not see a bare `station.closures_with.map` on the absent shape.
    let wire: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    let (station, closure_free) = stations
        .iter()
        .zip(&wire)
        .find(|(s, _)| s.closures().is_empty())
        .expect("this fixture has stations with no closures");
    // The zip pairs by position; prove the pairing rather than assume it, or a
    // reordering serializer would let this assert about the wrong station.
    assert_eq!(
        closure_free.get("station_ref"),
        Some(&serde_json::to_value(station.station_ref).unwrap()),
        "the JSON element must be the station we tested for emptiness"
    );
    assert!(
        !closure_free
            .as_object()
            .unwrap()
            .contains_key("closures_with"),
        "a closure-free station must omit the key, not send []: {closure_free}"
    );
    assert!(
        <Station as ts_rs::TS>::decl().contains("closures_with?:"),
        "the key is absent above, so the generated declaration must mark it optional: {}",
        <Station as ts_rs::TS>::decl()
    );
}

// === Move Segment Tests ===

#[test]
fn test_move_segment() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections and merge them so they form a segment
    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    // Merge station A.1 with B.0
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sid_a, 1),
        station_b: StationRef::new(sid_b, 0),
    });
    assert!(merge_result.success);

    // Record offsets after merge (merge may set non-zero offsets)
    let project = state.get_project_state().unwrap();
    let a_before = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_before = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment starting from section A
    let result = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(result.success);

    // Both sections should have been moved by the offset delta
    let project = state.get_project_state().unwrap();
    let a_after = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!(
        (a_after.0 - a_before.0 - 10.0).abs() < 0.001,
        "Section A x should increase by 10"
    );
    assert!(
        (a_after.1 - a_before.1 - 20.0).abs() < 0.001,
        "Section A y should increase by 20"
    );
    assert!(
        (b_after.0 - b_before.0 - 10.0).abs() < 0.001,
        "Section B x (connected via merge) should increase by 10"
    );
    assert!(
        (b_after.1 - b_before.1 - 20.0).abs() < 0.001,
        "Section B y (connected via merge) should increase by 20"
    );
}

#[test]
fn test_move_segment_undo_redo() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections and merge
    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    state.execute(Command::MergeStations {
        station_a: StationRef::new(sid_a, 1),
        station_b: StationRef::new(sid_b, 0),
    });

    // Record offsets after merge (merge may set non-zero offsets)
    let project = state.get_project_state().unwrap();
    let a_pre_move = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_pre_move = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment
    state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });

    // Verify both moved by delta
    let project = state.get_project_state().unwrap();
    let a_post_move = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_post_move = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!((a_post_move.0 - a_pre_move.0 - 10.0).abs() < 0.001);
    assert!((a_post_move.1 - a_pre_move.1 - 20.0).abs() < 0.001);
    assert!((b_post_move.0 - b_pre_move.0 - 10.0).abs() < 0.001);
    assert!((b_post_move.1 - b_pre_move.1 - 20.0).abs() < 0.001);

    // Undo should restore exact pre-move offsets
    let undo_result = state.undo();
    assert!(undo_result.success);

    let project = state.get_project_state().unwrap();
    let a_after_undo = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after_undo = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (a_after_undo.0 - a_pre_move.0).abs() < 0.001,
        "Undo should restore A's exact pre-move x offset"
    );
    assert!(
        (a_after_undo.1 - a_pre_move.1).abs() < 0.001,
        "Undo should restore A's exact pre-move y offset"
    );
    assert!(
        (b_after_undo.0 - b_pre_move.0).abs() < 0.001,
        "Undo should restore B's exact pre-move x offset"
    );
    assert!(
        (b_after_undo.1 - b_pre_move.1).abs() < 0.001,
        "Undo should restore B's exact pre-move y offset"
    );

    // Redo should reapply the delta
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    let a_after_redo = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after_redo = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (a_after_redo.0 - a_after_undo.0 - 10.0).abs() < 0.001,
        "Redo should add back the x delta to section A"
    );
    assert!(
        (a_after_redo.1 - a_after_undo.1 - 20.0).abs() < 0.001,
        "Redo should add back the y delta to section A"
    );
    assert!(
        (b_after_redo.0 - b_after_undo.0 - 10.0).abs() < 0.001,
        "Redo should add back the x delta to section B"
    );
    assert!(
        (b_after_redo.1 - b_after_undo.1 - 20.0).abs() < 0.001,
        "Redo should add back the y delta to section B"
    );
}

#[test]
fn test_move_segment_nonexistent_section() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::MoveSegment {
        section_id: 999,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(
        !result.success,
        "MoveSegment should fail for nonexistent section"
    );
}

#[test]
fn test_move_segment_unaligned_closure_independent() {
    // Sections connected only via an unaligned loop closure should move independently
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    // Inject a loop closure directly (no merge needed) connecting A.1 ↔ B.0
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(sid_a, 1).into(),
            station_b: StationRef::new(sid_b, 0).into(),
            path_length: 10.0,
            misclosure_distance: 0.1,
            error_percentage: 1.0,
            horizontal_misclosure: 0.08,
            vertical_misclosure: 0.06,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Record offsets before move
    let project = state.get_project_state().unwrap();
    let a_before = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_before = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment starting from section A
    let result = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(5.0, 15.0, 0.0),
    });
    assert!(result.success);

    // Only the starting section should move (closure is not aligned yet)
    let project = state.get_project_state().unwrap();
    let a_after = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!(
        (a_after.0 - a_before.0 - 5.0).abs() < 0.001,
        "Section A x should increase by 5"
    );
    assert!(
        (a_after.1 - a_before.1 - 15.0).abs() < 0.001,
        "Section A y should increase by 15"
    );
    assert!(
        (b_after.0 - b_before.0).abs() < 0.001,
        "Section B x should remain unchanged before closure alignment"
    );
    assert!(
        (b_after.1 - b_before.1).abs() < 0.001,
        "Section B y should remain unchanged before closure alignment"
    );
}

#[test]
fn test_move_segment_aligned_closure_connectivity() {
    // Sections connected via an aligned loop closure should move together
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    // Inject an aligned loop closure directly (no merge needed) connecting A.1 ↔ B.0
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(sid_a, 1).into(),
            station_b: StationRef::new(sid_b, 0).into(),
            path_length: 10.0,
            misclosure_distance: 0.1,
            error_percentage: 1.0,
            horizontal_misclosure: 0.08,
            vertical_misclosure: 0.06,
            strategy: ClosureStrategy::default(),
            is_aligned: true,
        });
        project.next_closure_id += 1;
    }

    // Record offsets before move
    let project = state.get_project_state().unwrap();
    let a_before = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_before = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment starting from section A
    let result = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(5.0, 15.0, 0.0),
    });
    assert!(result.success);

    // Both sections should have moved (connected via aligned closure)
    let project = state.get_project_state().unwrap();
    let a_after = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!(
        (a_after.0 - a_before.0 - 5.0).abs() < 0.001,
        "Section A x should increase by 5"
    );
    assert!(
        (a_after.1 - a_before.1 - 15.0).abs() < 0.001,
        "Section A y should increase by 15"
    );
    assert!(
        (b_after.0 - b_before.0 - 5.0).abs() < 0.001,
        "Section B x (aligned closure) should increase by 5"
    );
    assert!(
        (b_after.1 - b_before.1 - 15.0).abs() < 0.001,
        "Section B y (aligned closure) should increase by 15"
    );
}

#[test]
fn test_move_segment_blocked_by_datum_over_a_closure_only_link() {
    // SYNTHETIC STATE - read this before trusting the test. The closure-only link
    // built below cannot arise through the app: `convert_orphan_closures_to_merges`
    // (models/project/offsets.rs) runs after every command and promotes a
    // sole-bridge closure to a merge, skipping only the both-endpoints-anchored
    // case, which the narrow guard would catch anyway. So this pins HARDENING, not
    // a reachable bug: it fails if the guard ever stops covering the set
    // move_segment actually offsets. The closure is pushed directly for exactly
    // that reason - the command path would convert it away.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "ClosureDatum".to_string(),
        location: None,
    });

    let sid_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let sid_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    // A <-> B joined ONLY by an aligned closure: no merge edge, so
    // get_merged_section_ids(B) is {B} while get_rigid_connected_section_ids(B)
    // is {A, B}. That divergence is what the widened guard covers.
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(sid_a, 1).into(),
            station_b: StationRef::new(sid_b, 0).into(),
            path_length: 10.0,
            misclosure_distance: 0.1,
            error_percentage: 1.0,
            horizontal_misclosure: 0.08,
            vertical_misclosure: 0.06,
            strategy: ClosureStrategy::default(),
            is_aligned: true,
        });
        project.next_closure_id += 1;
    }

    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sid_a, 0),
        coordinates: GpsCoordinates::new(24.55, -81.78, 0.0).unwrap(),
    });
    assert!(datum.success, "datum setup must succeed");

    let before = state.get_project_state().unwrap();
    let a_before = before
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Dragging B would drag A - the datum holder - along with it.
    let result = state.execute(Command::MoveSegment {
        section_id: sid_b,
        offset: Coordinates::new(5.0, 15.0, 0.0),
    });
    assert!(
        !result.success,
        "moving a segment whose closure-connected group holds a datum must be refused"
    );

    let after = state.get_project_state().unwrap();
    let a_after = after
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (a_after.0 - a_before.0).abs() < 1e-9 && (a_after.1 - a_before.1).abs() < 1e-9,
        "the datum-bearing section must not have moved"
    );
}

#[test]
fn test_move_segment_isolated() {
    // An unconnected section should NOT be moved when moving a segment
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections WITHOUT merging them
    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    // Record offsets before move
    let project = state.get_project_state().unwrap();
    let a_before = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_before = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment starting from section A (B is NOT connected)
    let result = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(result.success);

    // Section A should have moved (entry must exist after MoveSegment)
    let project = state.get_project_state().unwrap();
    let a_after = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .expect("Section A offset should exist after MoveSegment");
    // Section B was not moved; entry may or may not exist
    let b_after = project
        .section_offsets
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!(
        (a_after.0 - a_before.0 - 10.0).abs() < 0.001,
        "Section A x should increase by 10"
    );
    assert!(
        (a_after.1 - a_before.1 - 20.0).abs() < 0.001,
        "Section A y should increase by 20"
    );

    // Section B should NOT have moved (not connected)
    assert!(
        (b_after.0 - b_before.0).abs() < 0.001,
        "Isolated Section B x should NOT change"
    );
    assert!(
        (b_after.1 - b_before.1).abs() < 0.001,
        "Isolated Section B y should NOT change"
    );
}

#[test]
fn test_move_segment_accumulates() {
    // Multiple MoveSegment calls should accumulate offsets
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    // Record initial offset
    let project = state.get_project_state().unwrap();
    let initial = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));

    // First move
    let result1 = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(10.0, 5.0, 0.0),
    });
    assert!(result1.success);

    let project = state.get_project_state().unwrap();
    let after_first = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .expect("Offset should exist after first MoveSegment");
    assert!(
        (after_first.0 - initial.0 - 10.0).abs() < 0.001,
        "After first move, x should increase by 10"
    );
    assert!(
        (after_first.1 - initial.1 - 5.0).abs() < 0.001,
        "After first move, y should increase by 5"
    );

    // Second move
    let result2 = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(3.0, 7.0, 0.0),
    });
    assert!(result2.success);

    let project = state.get_project_state().unwrap();
    let after_second = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .expect("Offset should exist after second MoveSegment");
    assert!(
        (after_second.0 - initial.0 - 13.0).abs() < 0.001,
        "After second move, x should be initial + 10 + 3 = +13"
    );
    assert!(
        (after_second.1 - initial.1 - 12.0).abs() < 0.001,
        "After second move, y should be initial + 5 + 7 = +12"
    );

    // Third move with negative offset
    let result3 = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(-5.0, -2.0, 0.0),
    });
    assert!(result3.success);

    let project = state.get_project_state().unwrap();
    let after_third = project
        .section_offsets
        .get(&sid_a)
        .copied()
        .expect("Offset should exist after third MoveSegment");
    assert!(
        (after_third.0 - initial.0 - 8.0).abs() < 0.001,
        "After third move, x should be initial + 13 - 5 = +8"
    );
    assert!(
        (after_third.1 - initial.1 - 10.0).abs() < 0.001,
        "After third move, y should be initial + 12 - 2 = +10"
    );
}

// === Batch Tests ===

#[test]
fn test_batch_rollback_on_failure() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create a section first
    let r1 = state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let _existing_id = r1.section_id.unwrap();

    // Capture state before batch
    let project_before = state.get_project_state().unwrap();
    let sections_before = project_before.sections.len();

    // Execute batch with valid command followed by invalid command
    let result = state.execute(Command::Batch {
        commands: vec![
            Command::CreateSection {
                name: Some("New".to_string()),
                surveyor: None,
                date: None,
                color: "#00ff00".to_string(),
                direction: None,
                shots: vec![],
            },
            // Move a nonexistent section - should fail
            Command::MoveSection {
                section_id: 99999,
                offset: Coordinates::new(1.0, 1.0, 0.0),
            },
        ],
    });
    assert!(!result.success, "Batch should fail");

    // Verify rollback: project should be exactly as before
    let project_after = state.get_project_state().unwrap();
    assert_eq!(
        project_after.sections.len(),
        sections_before,
        "Batch rollback should restore exact section count"
    );
}

#[test]
fn test_batch_disallowed_command() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Try to open a project inside a batch (should be rejected by whitelist)
    let result = state.execute(Command::Batch {
        commands: vec![Command::OpenProject {
            path: PathBuf::from("/nonexistent"),
        }],
    });
    assert!(
        !result.success,
        "Batch should reject non-whitelisted commands"
    );
    assert!(
        result
            .error
            .unwrap_or_default()
            .contains("not allowed in batch"),
        "Error should mention batch whitelist"
    );
}

// === Delete Shot and Split Section Tests ===

fn shot_data_with_azimuth(length: f64, az_in: f64, az_out: f64) -> ShotData {
    ShotData {
        azimuth_in: az_in,
        azimuth_out: az_out,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

#[test]
fn test_split_section_basic() {
    // 5-shot section, split at shot index 2 -> A has 2 shots, B has 2 shots
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Split Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: Some("TestSurveyor".to_string()),
        date: None,
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
        ],
    });
    assert!(create_result.success);
    let section_id = create_result.section_id.unwrap();

    // Get the shot_id for the middle shot (index 2)
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        let section = proj.get_section(section_id).unwrap();
        section.shots[2].id
    };

    let result = state.execute_delete_shot_and_split(
        section_id,
        shot_id,
        "Section A".to_string(),
        "Section B".to_string(),
    );

    assert!(result.success, "Split should succeed: {:?}", result.error);
    let new_section_id = result.section_id.unwrap();

    // Verify sections
    let proj = state.get_project_state().unwrap();
    let section_a = proj.get_section(section_id).unwrap();
    let section_b = proj.get_section(new_section_id).unwrap();

    assert_eq!(section_a.name, Some("Section A".to_string()));
    assert_eq!(section_a.shots.len(), 2);
    assert_eq!(section_a.shots[0].length, 5.0);
    assert_eq!(section_a.shots[1].length, 7.0);

    assert_eq!(section_b.name, Some("Section B".to_string()));
    assert_eq!(section_b.shots.len(), 2);
    assert_eq!(section_b.shots[0].length, 4.0);
    assert_eq!(section_b.shots[1].length, 6.0);
}

#[test]
fn test_split_preserves_metadata() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Metadata Test".to_string(),
        location: None,
    });

    let date = chrono::NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: Some("Jane".to_string()),
        date: Some(date),
        color: "#aabbcc".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
        ],
    });
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);
    let new_id = result.section_id.unwrap();

    let proj = state.get_project_state().unwrap();
    let section_a = proj.get_section(section_id).unwrap();
    let section_b = proj.get_section(new_id).unwrap();

    // Both sections inherit metadata
    assert_eq!(section_a.surveyor_names(), Some("Jane".to_string()));
    assert_eq!(section_a.first_survey_date(), Some(date));
    assert_eq!(section_a.color, "#aabbcc");
    assert_eq!(section_a.direction, SurveyDirection::Out);

    assert_eq!(section_b.surveyor_names(), Some("Jane".to_string()));
    assert_eq!(section_b.first_survey_date(), Some(date));
    assert_eq!(section_b.color, "#aabbcc");
    assert_eq!(section_b.direction, SurveyDirection::Out);
}

#[test]
fn test_split_recalculates_offset_b() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Offset Test".to_string(),
        location: None,
    });

    // Create section with known geometry: 3 shots going east (azimuth 90)
    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot_data_with_azimuth(10.0, 90.0, 90.0), // East
            shot_data_with_azimuth(5.0, 90.0, 90.0),  // East (deleted)
            shot_data_with_azimuth(8.0, 90.0, 90.0),  // East
        ],
    });
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);
    let new_id = result.section_id.unwrap();

    // Section B offset should be at (accumulated shots_before + deleted shot)
    // Shot lengths are slope distances; horizontal = sqrt(length^2 - dz^2)
    // With azimuth 90, sin(90) = 1, cos(90) = 0
    // So bx = horizontal_sum, by = 0 (approximately)
    let proj = state.get_project_state().unwrap();
    let b_offset = proj.section_offsets.get(&new_id);
    assert!(b_offset.is_some(), "Section B should have an offset");
    let (bx, by) = b_offset.unwrap();
    let expected =
        (10.0_f64.powi(2) - 1.0_f64.powi(2)).sqrt() + (5.0_f64.powi(2) - 1.0_f64.powi(2)).sqrt();
    assert!(
        (*bx - expected).abs() < 0.001,
        "bx should be ~{expected}, got {bx}"
    );
    assert!(by.abs() < 0.001, "by should be ~0, got {by}");
}

#[test]
fn test_split_remaps_merges() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Merge Remap Test".to_string(),
        location: None,
    });

    // Create two sections
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToSplit".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
        ],
    });
    let section_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let section_b_id = create_b.section_id.unwrap();

    // Merge station 3 of section A (after split, belongs to B index 0) with station 0 of section B
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a_id, 3), // This is after split point
        station_b: StationRef::new(section_b_id, 0),
    });
    assert!(
        merge_result.success,
        "Merge should succeed: {:?}",
        merge_result.error
    );

    // Now split at shot index 1
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_a_id).unwrap().shots[1].id
    };

    let result = state.execute_delete_shot_and_split(
        section_a_id,
        shot_id,
        "A".to_string(),
        "B".to_string(),
    );
    assert!(result.success);
    let new_section_id = result.section_id.unwrap();

    // The merge with station_index 3 should now be remapped to new_section_id, station_index 1
    // (original index 3, split_station = 2, so new index = 3 - 2 = 1)
    let proj = state.get_project_state().unwrap();
    let has_remapped_merge = proj.station_merges.iter().any(|m| {
        let a = proj.resolve_key(&m.station_a);
        let b = proj.resolve_key(&m.station_b);
        (a.is_some_and(|sr| sr.section_id == new_section_id && sr.station_index == 1)
            && m.station_b.section_id == section_b_id)
            || (b.is_some_and(|sr| sr.section_id == new_section_id && sr.station_index == 1)
                && m.station_a.section_id == section_b_id)
    });
    assert!(
        has_remapped_merge,
        "Merge should be remapped to new section with adjusted index"
    );
}

#[test]
fn test_split_remaps_closures() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Closure Remap Test".to_string(),
        location: None,
    });

    // Create two sections and merge them first so we can create a closure
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToSplit".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
        ],
    });
    let sec_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_b_id = create_b.section_id.unwrap();

    // First merge station 0 of A with station 0 of B (so they are connected)
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a_id, 0),
        station_b: StationRef::new(sec_b_id, 0),
    });
    assert!(merge_result.success);

    // Create a loop closure between station 4 of A (will be in B after split) and station 1 of B
    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(sec_a_id, 4),
        station_b: StationRef::new(sec_b_id, 1),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(
        closure_result.success,
        "Closure should succeed: {:?}",
        closure_result.error
    );

    // Split at shot index 2
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(sec_a_id).unwrap().shots[2].id
    };

    let result = state.execute_delete_shot_and_split(
        sec_a_id,
        shot_id,
        "A".to_string(),
        "B-split".to_string(),
    );
    assert!(result.success);
    let new_section_id = result.section_id.unwrap();

    // The closure's A-side is remapped: (sec_a_id, 4) -> (new_section_id, 1)
    // (split_station = 3, so 4 - 3 = 1). But the split detaches new_section so
    // that closure is now its ONLY connection to B - a sole bridge is a merge,
    // not a loop (#8), so it is converted. Assert it was remapped AND merged.
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.loop_closures.is_empty(),
        "the orphaned (sole-bridge) closure should have been converted to a merge: {:?}",
        proj.loop_closures
    );
    let has_remapped_merge = proj.station_merges.iter().any(|m| {
        proj.resolve_key(&m.station_a)
            .is_some_and(|sr| sr.section_id == new_section_id && sr.station_index == 1)
            || proj
                .resolve_key(&m.station_b)
                .is_some_and(|sr| sr.section_id == new_section_id && sr.station_index == 1)
    });
    assert!(
        has_remapped_merge,
        "the remapped closure should have become a merge on the new section: {:?}",
        proj.station_merges
    );
}

// === Bulk shot range deletion (#95) ===

#[test]
fn test_delete_range_split_interior() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Range Split".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: Some("Surv".to_string()),
        date: None,
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    // Delete shots [2..=3] (the 3.0 and 4.0 shots) and split.
    let result =
        state.execute_delete_shot_range_split(section_id, s2, s3, "A".to_string(), "B".to_string());
    assert!(
        result.success,
        "range split should succeed: {:?}",
        result.error
    );
    let new_id = result.section_id.unwrap();

    let proj = state.get_project_state().unwrap();
    let a = proj.get_section(section_id).unwrap();
    let b = proj.get_section(new_id).unwrap();
    assert_eq!(a.name, Some("A".to_string()));
    assert_eq!(
        a.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![5.0, 7.0]
    );
    assert_eq!(b.name, Some("B".to_string()));
    assert_eq!(
        b.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![6.0, 8.0]
    );
    // Metadata is inherited by both halves.
    assert_eq!(b.surveyor_names(), Some("Surv".to_string()));
    assert_eq!(b.direction, SurveyDirection::Out);
}

#[test]
fn test_delete_range_split_preserves_after_position() {
    // East-going shots: section B's offset should equal the displacement of
    // (shots before the range ++ the deleted shots).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Range Offset".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot_data_with_azimuth(10.0, 90.0, 90.0), // before
            shot_data_with_azimuth(5.0, 90.0, 90.0),  // deleted
            shot_data_with_azimuth(7.0, 90.0, 90.0),  // deleted
            shot_data_with_azimuth(8.0, 90.0, 90.0),  // after
        ],
    });
    let section_id = create.section_id.unwrap();
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let result =
        state.execute_delete_shot_range_split(section_id, s1, s2, "A".to_string(), "B".to_string());
    assert!(result.success, "{:?}", result.error);
    let new_id = result.section_id.unwrap();

    let proj = state.get_project_state().unwrap();
    let (bx, by) = *proj.section_offsets.get(&new_id).expect("B offset");
    let h = |l: f64| (l * l - 1.0_f64).sqrt();
    let expected = h(10.0) + h(5.0) + h(7.0);
    assert!((bx - expected).abs() < 0.001, "bx ~{expected}, got {bx}");
    assert!(by.abs() < 0.001, "by ~0, got {by}");
}

#[test]
fn test_delete_range_shift_interior() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Range Shift".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let result = state.execute_delete_shot_range_shift(section_id, s2, s3);
    assert!(
        result.success,
        "range shift should succeed: {:?}",
        result.error
    );

    // No new section - the survivors gap-close into one section [5,7,6,8].
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.sections.len(), 1);
    let sec = proj.get_section(section_id).unwrap();
    assert_eq!(
        sec.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![5.0, 7.0, 6.0, 8.0]
    );
}

#[test]
fn test_delete_range_prefix_shift() {
    // Delete shots [0..=1] of a 5-shot section; the survivors keep their absolute
    // positions (the section offset is shifted to compensate).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Prefix".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot_data_with_azimuth(10.0, 90.0, 90.0),
            shot_data_with_azimuth(5.0, 90.0, 90.0),
            shot_data_with_azimuth(7.0, 90.0, 90.0),
            shot_data_with_azimuth(8.0, 90.0, 90.0),
            shot_data_with_azimuth(6.0, 90.0, 90.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let before_abs = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let kept_before = *before_abs.get(&StationRef::new(section_id, 2)).unwrap();
    let (s0, s1) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[0].id, sec.shots[1].id)
    };
    let result = state.execute_delete_shot_range_shift(section_id, s0, s1);
    assert!(result.success, "{:?}", result.error);

    let proj = state.get_project_state().unwrap();
    let sec = proj.get_section(section_id).unwrap();
    assert_eq!(
        sec.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![7.0, 8.0, 6.0]
    );
    // Old station 2 (start of the first surviving shot) is now station 0, same place.
    let after_abs = abs_coord_map(&proj);
    let kept_after = *after_abs.get(&StationRef::new(section_id, 0)).unwrap();
    assert_xy_close(kept_before, kept_after);
}

#[test]
fn test_delete_range_suffix_shift() {
    // Delete shots [3..=4] of a 5-shot section; the head stays put.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Suffix".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot_data_with_azimuth(10.0, 90.0, 90.0),
            shot_data_with_azimuth(5.0, 90.0, 90.0),
            shot_data_with_azimuth(7.0, 90.0, 90.0),
            shot_data_with_azimuth(8.0, 90.0, 90.0),
            shot_data_with_azimuth(6.0, 90.0, 90.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let head_before = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let (s3, s4) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[3].id, sec.shots[4].id)
    };
    let result = state.execute_delete_shot_range_shift(section_id, s3, s4);
    assert!(result.success, "{:?}", result.error);

    let proj = state.get_project_state().unwrap();
    let sec = proj.get_section(section_id).unwrap();
    assert_eq!(
        sec.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![10.0, 5.0, 7.0]
    );
    let after = abs_coord_map(&proj);
    for i in 0..=3u32 {
        let r = StationRef::new(section_id, i);
        assert_xy_close(*head_before.get(&r).unwrap(), *after.get(&r).unwrap());
    }
}

#[test]
fn test_delete_range_split_remaps_merge() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Range Merge Remap".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToSplit".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_c = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_c = create_c.section_id.unwrap();
    // Merge A:5 (-> new section index 1 after split [2..=3], split_station 4) with C:0.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 5),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let result =
        state.execute_delete_shot_range_split(sec_a, s2, s3, "A".to_string(), "B".to_string());
    assert!(result.success, "{:?}", result.error);
    let new_id = result.section_id.unwrap();

    let proj = state.get_project_state().unwrap();
    let has_remapped = proj.station_merges.iter().any(|m| {
        let a = proj.resolve_key(&m.station_a);
        let b = proj.resolve_key(&m.station_b);
        (a.is_some_and(|sr| sr.section_id == new_id && sr.station_index == 1)
            && m.station_b.section_id == sec_c)
            || (b.is_some_and(|sr| sr.section_id == new_id && sr.station_index == 1)
                && m.station_a.section_id == sec_c)
    });
    assert!(
        has_remapped,
        "merge A:5 should remap to new section index 1: {:?}",
        proj.station_merges
    );
}

#[test]
fn test_delete_range_preserves_interior_merge_connectivity() {
    // Two other sections (P, Q) bridge ONLY through an interior station A:3 that
    // the range delete removes. A replacement merge must keep P and Q connected.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Interior Bridge".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_p = state.execute(Command::CreateSection {
        name: Some("P".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_p = create_p.section_id.unwrap();
    let create_q = state.execute(Command::CreateSection {
        name: Some("Q".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_q = create_q.section_id.unwrap();
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_p, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_q, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );
    {
        let proj = state.get_project_state().unwrap();
        assert!(
            proj.are_stations_merged(StationRef::new(sec_p, 0), StationRef::new(sec_q, 0)),
            "P and Q should be connected before the delete"
        );
    }

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    // Range [2..=3] removes interior station A:3 (the only P<->Q bridge).
    let result =
        state.execute_delete_shot_range_split(sec_a, s2, s3, "A".to_string(), "B".to_string());
    assert!(result.success, "{:?}", result.error);

    let proj = state.get_project_state().unwrap();
    assert!(
        proj.are_stations_merged(StationRef::new(sec_p, 0), StationRef::new(sec_q, 0)),
        "P and Q must stay connected via a replacement merge after the interior bridge is \
         removed: {:?}",
        proj.station_merges
    );
}

#[test]
fn test_delete_range_shift_blocked_aborts() {
    // A loop around section A's middle makes the shift path unavailable; the
    // shift range delete must fail and leave the project untouched.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Shift Blocked".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // Form a loop directly (A:0-B:0 and A:4-B:1). MergeStations refuses the second
    // edge because the stations are already connected, so push raw merges like the
    // model-level shift_blocked test does.
    {
        let mut guard = state.project.lock().unwrap();
        let proj = guard.as_mut().unwrap();
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_a, 0).into(),
            station_b: StationRef::new(sec_b, 0).into(),
            merged_at: chrono::Utc::now(),
        });
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_a, 4).into(),
            station_b: StationRef::new(sec_b, 1).into(),
            merged_at: chrono::Utc::now(),
        });
    }

    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let result = state.execute_delete_shot_range_shift(sec_a, s1, s2);
    assert!(!result.success, "shift over a loop must be blocked");
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.get_section(sec_a).unwrap().shots.len(),
        4,
        "blocked shift must not mutate the project"
    );
}

#[test]
fn test_delete_range_undo_redo_roundtrip() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Range Undo".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot_data_with_azimuth(10.0, 90.0, 90.0),
            shot_data_with_azimuth(5.0, 45.0, 45.0),
            shot_data_with_azimuth(7.0, 90.0, 90.0),
            shot_data_with_azimuth(8.0, 135.0, 135.0),
            shot_data_with_azimuth(6.0, 90.0, 90.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let before_abs = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let undo_len_before = state.get_undo_stack().len();
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let result =
        state.execute_delete_shot_range_split(section_id, s1, s2, "A".to_string(), "B".to_string());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        state.get_undo_stack().len(),
        undo_len_before + 1,
        "the whole range split is one undo entry"
    );

    assert!(state.undo().success);
    let after_undo = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    assert_eq!(before_abs.len(), after_undo.len());
    for (r, c) in &before_abs {
        let c2 = after_undo
            .get(r)
            .unwrap_or_else(|| panic!("missing {r:?} after undo"));
        assert_coords_close(*c, *c2);
    }

    assert!(state.redo().success);
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.sections.len(),
        2,
        "redo restores the split (two sections)"
    );
}

#[test]
fn test_preview_delete_shot_range_classifies_kind() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Kind".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let proj = state.get_project_state().unwrap();
    let ids: Vec<ShotId> = proj
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();

    use crate::models::RangeDeleteKind;
    assert_eq!(
        proj.preview_delete_shot_range(section_id, ids[0], ids[4])
            .unwrap()
            .kind,
        RangeDeleteKind::WholeSection
    );
    assert_eq!(
        proj.preview_delete_shot_range(section_id, ids[0], ids[2])
            .unwrap()
            .kind,
        RangeDeleteKind::Prefix
    );
    assert_eq!(
        proj.preview_delete_shot_range(section_id, ids[2], ids[4])
            .unwrap()
            .kind,
        RangeDeleteKind::Suffix
    );
    let interior = proj
        .preview_delete_shot_range(section_id, ids[1], ids[3])
        .unwrap();
    assert_eq!(interior.kind, RangeDeleteKind::Interior);
    // Reversed endpoints normalize to the same range.
    let rev = proj
        .preview_delete_shot_range(section_id, ids[3], ids[1])
        .unwrap();
    assert_eq!((rev.start_pos, rev.end_pos), (1, 3));
}

#[test]
fn test_delete_range_split_rejects_non_interior() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Reject".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    let ids: Vec<ShotId> = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id)
            .unwrap()
            .shots
            .iter()
            .map(|s| s.id)
            .collect()
    };
    // Prefix range [0..=1] is not interior - split must refuse.
    assert!(
        !state
            .execute_delete_shot_range_split(
                section_id,
                ids[0],
                ids[1],
                "A".to_string(),
                "B".to_string()
            )
            .success
    );
    // Whole-section shift must refuse (delete the section instead).
    assert!(
        !state
            .execute_delete_shot_range_shift(section_id, ids[0], ids[2])
            .success
    );

    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.get_section(section_id).unwrap().shots.len(),
        3,
        "rejections must not mutate"
    );
}

#[test]
fn test_delete_range_interior_shift_blocked_by_tail_datum() {
    // Regression (#95): an interior-range SHIFT gap-closes THROUGH station
    // end_pos+1 (which the split keeps as B:0). A zero datum anchored there must
    // block the shift - the preview previously scanned only the split's interior
    // set and missed it, so the gap-close silently destroyed the datum.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Tail Datum".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    // Datum on station 3 = end_pos+1 for the interior range [1..=2].
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(section_id, 3),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    {
        let proj = state.get_project_state().unwrap();
        let impact = proj.preview_delete_shot_range(section_id, s1, s2).unwrap();
        assert!(
            impact.shift_blocked,
            "shift must be blocked: it would drop the datum on station end_pos+1"
        );
        assert!(
            impact.removed_datums.is_empty(),
            "the split (display) set keeps station end_pos+1, so its datum is not in removed_datums"
        );
    }
    // The shift executor must refuse and leave the datum intact.
    let result = state.execute_delete_shot_range_shift(section_id, s1, s2);
    assert!(
        !result.success,
        "interior shift over a tail datum must be blocked"
    );
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.zero_datums.len(),
        1,
        "blocked shift must not drop the datum"
    );
    assert_eq!(
        proj.get_section(section_id).unwrap().shots.len(),
        5,
        "blocked shift must not mutate the section"
    );
}

#[test]
fn test_delete_range_prefix_shift_blocked_by_datum() {
    // Coverage (#95): a prefix range whose removed boundary station carries a
    // zero datum must block the shift rather than silently drop the anchor.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Prefix Datum".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(section_id, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    let (s0, s1) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[0].id, sec.shots[1].id)
    };
    {
        let proj = state.get_project_state().unwrap();
        let impact = proj.preview_delete_shot_range(section_id, s0, s1).unwrap();
        assert!(
            impact.shift_blocked,
            "prefix shift dropping a datum must be blocked"
        );
        assert!(
            !impact.removed_datums.is_empty(),
            "the removed station-0 datum is surfaced"
        );
    }
    let result = state.execute_delete_shot_range_shift(section_id, s0, s1);
    assert!(!result.success, "prefix shift over a datum must be blocked");
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.zero_datums.len(),
        1,
        "blocked shift must not drop the datum"
    );
}

#[test]
fn test_delete_range_preview_surfaces_interior_split_datum() {
    // #95: a zero datum on a strictly-interior station the split drops must be
    // surfaced in removed_datums so the dialog can warn (the interior split is
    // not lossless for interior-station datums/comments).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Interior Datum".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    // Datum on station 2 - strictly interior to the range [1..=2] (interior set {2}).
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(section_id, 2),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(section_id, s1, s2).unwrap();
    assert!(
        impact.removed_datums.iter().any(|sr| sr.station_index == 2),
        "the interior-station datum the split drops must be surfaced: {:?}",
        impact.removed_datums
    );
}

#[test]
fn test_delete_range_preview_reports_replacement_merge() {
    // #95: when a removed interior station bridges two external sections, the
    // preview must report the replacement merge the delete adds (not just the
    // removed merges), so the dialog doesn't overstate connectivity loss.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Replacement Preview".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_p = state.execute(Command::CreateSection {
        name: Some("P".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_p = create_p.section_id.unwrap();
    let create_q = state.execute(Command::CreateSection {
        name: Some("Q".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_q = create_q.section_id.unwrap();
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_p, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_q, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );

    // Range [2..=3] removes the bridging interior station A:3.
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        !impact.replacement_merges.is_empty(),
        "removing the bridging interior station must report a replacement merge: {impact:?}"
    );
}

#[test]
fn test_delete_range_prefix_shift_not_blocked_by_internal_loop() {
    // Codex review #1: deleting a prefix range must not be blocked just because
    // an in-range shot looks loop-connected THROUGH another in-range shot.
    // Merge A:0<->A:2 makes the per-shot preview see shot 1 as part of a loop,
    // but deleting [0..=1] removes that path entirely, so the bulk shift is valid.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Internal Loop".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    // Same-section merge A:0 <-> A:2 (a loop through shots 0,1). MergeStations
    // would refuse it (already connected via shots), so push it raw.
    {
        let mut guard = state.project.lock().unwrap();
        let proj = guard.as_mut().unwrap();
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(section_id, 0).into(),
            station_b: StationRef::new(section_id, 2).into(),
            merged_at: chrono::Utc::now(),
        });
    }
    let (s0, s1) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[0].id, sec.shots[1].id)
    };
    {
        let proj = state.get_project_state().unwrap();
        let impact = proj.preview_delete_shot_range(section_id, s0, s1).unwrap();
        assert!(
            !impact.shift_blocked,
            "prefix delete must not be blocked by an in-range loop: {:?}",
            impact.shift_blocked_reasons
        );
    }
    let result = state.execute_delete_shot_range_shift(section_id, s0, s1);
    assert!(
        result.success,
        "prefix shift should succeed: {:?}",
        result.error
    );
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .get_section(section_id)
            .unwrap()
            .shots
            .len(),
        3
    );
}

#[test]
fn test_delete_range_suffix_shift_keeps_head_with_doomed_tail_anchor() {
    // Codex review #4: a suffix gap-close must keep the HEAD in place even when a
    // doomed tail station is anchored (merged to a datum-bearing section). The
    // per-shot keep-side decision must ignore anchors on stations the rest of
    // the range will also delete.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Doomed Tail".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot_data_with_azimuth(10.0, 90.0, 90.0),
            shot_data_with_azimuth(5.0, 90.0, 90.0),
            shot_data_with_azimuth(7.0, 90.0, 90.0),
            shot_data_with_azimuth(8.0, 90.0, 90.0),
            shot_data_with_azimuth(6.0, 90.0, 90.0),
        ],
    });
    let sec_a = create.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot_data_with_azimuth(4.0, 90.0, 90.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // B is datum-bearing and merged to A's END station (station 5) - the doomed tail.
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_b, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 5),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    let head_before = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let (s3, s4) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[3].id, sec.shots[4].id)
    };
    let result = state.execute_delete_shot_range_shift(sec_a, s3, s4);
    assert!(
        result.success,
        "suffix shift should succeed: {:?}",
        result.error
    );

    let proj = state.get_project_state().unwrap();
    let sec = proj.get_section(sec_a).unwrap();
    assert_eq!(
        sec.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![10.0, 5.0, 7.0]
    );
    // Head stations 0..=3 must stay put - the doomed tail anchor must NOT shift them.
    let after = abs_coord_map(&proj);
    for i in 0..=3u32 {
        let r = StationRef::new(sec_a, i);
        assert_xy_close(*head_before.get(&r).unwrap(), *after.get(&r).unwrap());
    }
}

#[test]
fn test_delete_range_preview_surfaces_interior_split_comment() {
    // Codex review #2: an interior split drops station comments on the removed
    // interior stations; the preview must surface them so the dialog can warn.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Interior Comment".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    // Comment on interior station 2 of the range [1..=2].
    assert!(
        state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(section_id, 2),
                comment: Some("note".to_string()),
            })
            .success
    );
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(section_id, s1, s2).unwrap();
    assert!(
        impact
            .removed_comments
            .iter()
            .any(|sr| sr.station_index == 2),
        "the interior-station comment the split drops must be surfaced: {:?}",
        impact.removed_comments
    );
}

/// [#469] The preview's `removed_comments` comes out SORTED, like the `removed_stations`
/// list built beside it.
///
/// It is collected by iterating the `station_comments` HashMap, so before the sort the
/// delete dialog listed the same comments in a different order on every run of the same
/// preview - and nothing downstream re-sorts (`BulkDeleteDialog.tsx` renders the vector
/// as given). Six commented stations make an accidentally-ascending hash order a
/// 1-in-720 coincidence rather than a coin flip.
#[test]
fn test_delete_range_preview_sorts_the_removed_comments() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Comment Order".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: (0..8).map(|i| test_shot_data(2.0 + f64::from(i))).collect(),
    });
    let section_id = create.section_id.unwrap();
    for idx in 1..=6u32 {
        assert!(
            state
                .execute(Command::SetStationComment {
                    station_ref: StationRef::new(section_id, idx),
                    comment: Some(format!("note {idx}")),
                })
                .success
        );
    }

    let (first, last) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[5].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj
        .preview_delete_shot_range(section_id, first, last)
        .unwrap();

    let indices: Vec<u32> = impact
        .removed_comments
        .iter()
        .map(|sr| sr.station_index)
        .collect();
    assert!(
        indices.len() >= 5,
        "the fixture must actually carry the comments it is sorting, got {indices:?}"
    );
    assert!(
        indices.windows(2).all(|w| w[0] < w[1]),
        "removed_comments must be in ascending station order, got {indices:?}"
    );
}

#[test]
fn test_delete_range_interior_preview_includes_shift_boundary_merge() {
    // Codex review #3: the interior preview's relationship set must cover the
    // gap-close (shift) removed station end_pos+1, so a merge anchored there is
    // shown before the user closes the gap (the split keeps it; the shift does not).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Shift Boundary".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // Merge on station 4 = end_pos+1 for the interior range [2..=3].
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 4),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        impact.removed_merges.iter().any(|p| {
            (p.station_a.section_id == sec_a && p.station_a.station_index == 4)
                || (p.station_b.section_id == sec_a && p.station_b.station_index == 4)
        }),
        "merge on the shift-boundary station end_pos+1 must be surfaced: {:?}",
        impact.removed_merges
    );
}

#[test]
fn test_delete_range_interior_shift_not_blocked_by_closure_loop() {
    // User report: a range whose two sides are connected only through a LOOP
    // CLOSURE must NOT block the gap-close - a closure is a soft/redundant
    // constraint the delete removes/converts, so the result is still valid. (A
    // merge-based loop still blocks; see test_delete_range_shift_blocked_aborts.)
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Closure Loop".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // Head A:2 <-> B via a merge; tail A:5 <-> B via a LOOP CLOSURE. So for the
    // interior range [2..=3] the only head<->tail path is through the closure.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 2),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );
    assert!(
        state
            .execute(Command::CreateLoopClosure {
                station_a: StationRef::new(sec_a, 5),
                station_b: StationRef::new(sec_b, 1),
                closure_strategy: ClosureStrategy::default(),
            })
            .success
    );

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    {
        let proj = state.get_project_state().unwrap();
        let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
        assert!(
            !impact.shift_blocked,
            "a closure-only loop must not block the gap-close: {:?}",
            impact.shift_blocked_reasons
        );
    }
    // And the gap-close executes cleanly (no over-constraint / alignment panic).
    let result = state.execute_delete_shot_range_shift(sec_a, s2, s3);
    assert!(
        result.success,
        "closure-loop gap-close should succeed: {:?}",
        result.error
    );
}

#[test]
fn test_delete_range_blocked_interior_impact_describes_split() {
    // Review finding (P2): when the gap-close is blocked, an interior range can
    // only SPLIT (which keeps both boundary stations), so the relationship panel
    // must describe the split - NOT list the kept tail boundary (end_pos+1) or
    // its merge as removed. (Complements the not-blocked case above, where the
    // gap-close IS offered and the boundary merge is surfaced.)
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Blocked Impact".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
        ],
    });
    let sec_a = create.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // Datum on interior station A:3 blocks the gap-close (it would drop the datum).
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 3),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Merge on the tail boundary A:4 (= end_pos+1 for range [2..=3]) - kept by split.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 4),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        impact.shift_blocked,
        "datum on interior station blocks the gap-close"
    );
    assert!(
        !impact
            .removed_stations
            .iter()
            .any(|sr| sr.section_id == sec_a && sr.station_index == 4),
        "split-only impact must not list the kept tail boundary: {:?}",
        impact.removed_stations
    );
    assert!(
        !impact.removed_merges.iter().any(|p| {
            (p.station_a.section_id == sec_a && p.station_a.station_index == 4)
                || (p.station_b.section_id == sec_a && p.station_b.station_index == 4)
        }),
        "the kept tail boundary's merge must not be reported as removed: {:?}",
        impact.removed_merges
    );
}

#[test]
fn test_delete_range_shift_does_not_displace_free_floating_merged_section() {
    // Codex re-review (P1): a suffix gap-close keeps the head fixed and shifts
    // the doomed tail. A per-shot cascade would push a free-floating (undatumed)
    // section merged to that tail, then the tail+merge are deleted - leaving it
    // displaced with no edge to pull it back. Deferring to one final cascade
    // keeps it in place.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Free Float".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot_data_with_azimuth(10.0, 90.0, 90.0),
            shot_data_with_azimuth(5.0, 90.0, 90.0),
            shot_data_with_azimuth(7.0, 90.0, 90.0),
            shot_data_with_azimuth(8.0, 90.0, 90.0),
            shot_data_with_azimuth(6.0, 90.0, 90.0),
        ],
    });
    let sec_a = create.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot_data_with_azimuth(4.0, 45.0, 45.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // B is free-floating (NO datum), merged to A's END station (A:5) - the doomed tail.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 5),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    let before = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let b0_before = *before.get(&StationRef::new(sec_b, 0)).unwrap();
    let b1_before = *before.get(&StationRef::new(sec_b, 1)).unwrap();

    let (s3, s4) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[3].id, sec.shots[4].id)
    };
    assert!(state.execute_delete_shot_range_shift(sec_a, s3, s4).success);

    let proj = state.get_project_state().unwrap();
    let after = abs_coord_map(&proj);
    // B is now disconnected from A; it must stay where it was, not be dragged by
    // the doomed tail's intermediate shift.
    assert_xy_close(b0_before, *after.get(&StationRef::new(sec_b, 0)).unwrap());
    assert_xy_close(b1_before, *after.get(&StationRef::new(sec_b, 1)).unwrap());
}

/// #464 defect B: a range gap-close whose first removal orphans a loop closure
/// promotes it to a rigid merge, and the promotion used to carry the loop's REAL
/// misclosure into a constraint that says the two endpoints coincide. The single
/// cascade the range driver runs at the end is seeded with the edited section
/// plus the datum sections, and by then the promoted pair sits in a datum-less
/// component of its own - so the BFS never enters it and
/// `debug_assert_merge_alignment("delete_shot_range_shift")` fires on a gap
/// nothing in that command could close.
///
/// Y - X - Z with a closure Y1 <-> Z1 closing the loop; the range takes out both
/// of X's merge stations, so Y is stranded on the first removal (promotion) and
/// Z on the second (the component floats free).
#[test]
fn delete_range_shift_leaves_a_promoted_closure_aligned() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Range Promotion".to_string(),
        location: None,
    });
    let sec_x = state
        .execute(Command::CreateSection {
            name: Some("X".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![
                shot_data_with_azimuth(10.0, 0.0, 0.0),
                shot_data_with_azimuth(10.0, 0.0, 0.0),
                shot_data_with_azimuth(10.0, 0.0, 0.0),
                shot_data_with_azimuth(10.0, 0.0, 0.0),
                shot_data_with_azimuth(10.0, 0.0, 0.0),
            ],
        })
        .section_id
        .unwrap();
    let sec_y = state
        .execute(Command::CreateSection {
            name: Some("Y".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot_data_with_azimuth(10.0, 90.0, 90.0)],
        })
        .section_id
        .unwrap();
    let sec_z = state
        .execute(Command::CreateSection {
            name: Some("Z".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot_data_with_azimuth(10.0, 270.0, 270.0)],
        })
        .section_id
        .unwrap();
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec_x, 0),
                coordinates: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
            })
            .success
    );
    // Y and Z hang off the two stations the range is about to remove.
    for (station, other) in [((sec_x, 2), (sec_y, 0)), ((sec_x, 3), (sec_z, 0))] {
        assert!(
            state
                .execute(Command::MergeStations {
                    station_a: StationRef::new(station.0, station.1),
                    station_b: StationRef::new(other.0, other.1),
                })
                .success
        );
    }
    // The loop's other leg, carrying a genuine 22.4 m misclosure.
    assert!(
        state
            .execute(Command::CreateLoopClosure {
                station_a: StationRef::new(sec_y, 1),
                station_b: StationRef::new(sec_z, 1),
                closure_strategy: ClosureStrategy::Batch,
            })
            .success
    );

    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_x).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let deleted = state.execute_delete_shot_range_shift(sec_x, s1, s2);
    assert!(
        deleted.success,
        "delete_shot_range_shift failed: {:?}",
        deleted.error
    );

    let proj = state.get_project_state().unwrap();
    assert!(
        proj.loop_closures.is_empty(),
        "the stranded closure must be promoted: {:?}",
        proj.loop_closures
    );
    let beyond = proj.merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M);
    assert!(
        beyond.is_empty(),
        "the range gap-close left a merge out of alignment: {beyond:?}"
    );
}

#[test]
fn test_delete_range_interior_shift_not_blocked_via_removed_tail_boundary() {
    // Codex re-review: a path from the head to the REMOVED tail boundary
    // (end_pos+1) through an external section is not a real loop - that station
    // and its merge are deleted by the gap-close - so it must not block.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Tail Boundary Loop".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_e = state.execute(Command::CreateSection {
        name: Some("E".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_e = create_e.section_id.unwrap();
    // Head side A:1 <-> E:0. A:4 (= end_pos+1 for range [2..=3]) <-> E:1 is
    // already connected (A:4->A:1->E:0->E:1), so push it raw to form the loop
    // that runs through the to-be-removed tail boundary A:4.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_e, 0),
            })
            .success
    );
    {
        let mut guard = state.project.lock().unwrap();
        let proj = guard.as_mut().unwrap();
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_a, 4).into(),
            station_b: StationRef::new(sec_e, 1).into(),
            merged_at: chrono::Utc::now(),
        });
    }
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        !impact.shift_blocked,
        "close-gap connected only through the removed tail boundary must not block: {:?}",
        impact.shift_blocked_reasons
    );
}

#[test]
fn test_delete_range_interior_shift_not_blocked_when_stitch_is_redundant() {
    // Two external sections P and Q are each merged to a removed interior station
    // AND to a surviving station of the same section A. The stitch the deletion
    // would once have invented, A:2<->Q:0, is rigid-REDUNDANT: Q is already
    // merged to A:5, so section A and section Q are rigidly connected without it
    // (the merge would only close the cycle A:2-Q:0-A:5-shots-A:2, which
    // merge_stations refuses interactively). `replacement_merge_allowed` skips it,
    // so no head<->tail loop re-forms and the gap-close must be OFFERED.
    //
    // This test previously asserted the opposite. Blocking here was circular: the
    // auto-stitch fabricated the very loop that then blocked the shift. With the
    // merge gone the premise dissolves - after the gap-close A:2 and old A:5 are
    // adjacent through the closed gap with P merged to one end and Q to the
    // other, so the rigid graph is the tree P-A-Q and nothing is over-constrained.
    // P and Q stay rigidly connected through A; only their merge-graph adjacency
    // (which described a bridge station that no longer exists) is gone.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Redundant Stitch".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
            test_shot_data(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_p = state.execute(Command::CreateSection {
        name: Some("P".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_p = create_p.section_id.unwrap();
    let create_q = state.execute(Command::CreateSection {
        name: Some("Q".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let sec_q = create_q.section_id.unwrap();
    // P:0 <-> head A:2, Q:0 <-> tail A:5 (valid merges).
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_p, 0),
                station_b: StationRef::new(sec_a, 2),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_q, 0),
                station_b: StationRef::new(sec_a, 5),
            })
            .success
    );
    // P:0 and Q:0 each also merged to the REMOVED interior A:3 (already connected,
    // so push raw). A:3 is the only P<->Q bridge.
    {
        let mut guard = state.project.lock().unwrap();
        let proj = guard.as_mut().unwrap();
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_p, 0).into(),
            station_b: StationRef::new(sec_a, 3).into(),
            merged_at: chrono::Utc::now(),
        });
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_q, 0).into(),
            station_b: StationRef::new(sec_a, 3).into(),
            merged_at: chrono::Utc::now(),
        });
    }
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        !impact.shift_blocked,
        "no loop re-forms once the redundant stitch is skipped, so the gap-close must be \
         offered: {:?}",
        impact.shift_blocked_reasons
    );
    assert!(
        !impact.replacement_merges.iter().any(|pair| {
            (pair.station_a.section_id == sec_a && pair.station_b.section_id == sec_q)
                || (pair.station_a.section_id == sec_q && pair.station_b.section_id == sec_a)
        }),
        "A and Q are already rigidly connected through Q:0<->A:5; the dialog must not promise \
         a redundant stitch: {:?}",
        impact.replacement_merges
    );
}

#[test]
fn test_delete_range_shift_undo_redo_roundtrip() {
    // Self-review (#6): the only range undo/redo test exercised the SPLIT path.
    // Cover the SHIFT path too - including a replacement merge added during the
    // deferred-cascade gap-close - so snapshot restore/redo is verified.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Shift Undo".to_string(),
        location: None,
    });
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot_data_with_azimuth(10.0, 90.0, 90.0),
            shot_data_with_azimuth(5.0, 45.0, 45.0),
            shot_data_with_azimuth(7.0, 90.0, 90.0),
            shot_data_with_azimuth(8.0, 135.0, 135.0),
            shot_data_with_azimuth(6.0, 90.0, 90.0),
            shot_data_with_azimuth(4.0, 90.0, 90.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_p = state.execute(Command::CreateSection {
        name: Some("P".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot_data_with_azimuth(4.0, 90.0, 90.0)],
    });
    let sec_p = create_p.section_id.unwrap();
    let create_q = state.execute(Command::CreateSection {
        name: Some("Q".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot_data_with_azimuth(4.0, 90.0, 90.0)],
    });
    let sec_q = create_q.section_id.unwrap();
    // P and Q bridge ONLY through interior A:3 (removed by the range) - the
    // gap-close adds a replacement merge P:0<->Q:0 to keep them connected.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_p, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_q, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );

    let before_abs = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let undo_len_before = state.get_undo_stack().len();
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let result = state.execute_delete_shot_range_shift(sec_a, s2, s3);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        state.get_undo_stack().len(),
        undo_len_before + 1,
        "one undo entry"
    );
    {
        let proj = state.get_project_state().unwrap();
        assert_eq!(
            proj.get_section(sec_a).unwrap().shots.len(),
            4,
            "two shots removed"
        );
        assert!(
            proj.are_stations_merged(StationRef::new(sec_p, 0), StationRef::new(sec_q, 0)),
            "P and Q stay connected via a replacement merge after the gap-close"
        );
    }

    assert!(state.undo().success);
    {
        let proj = state.get_project_state().unwrap();
        let after_undo = abs_coord_map(&proj);
        assert_eq!(before_abs.len(), after_undo.len());
        for (r, c) in &before_abs {
            let c2 = after_undo
                .get(r)
                .unwrap_or_else(|| panic!("missing {r:?} after undo"));
            assert_coords_close(*c, *c2);
        }
        assert_eq!(
            proj.get_section(sec_a).unwrap().shots.len(),
            6,
            "undo restores all shots"
        );
    }

    assert!(state.redo().success);
    {
        let proj = state.get_project_state().unwrap();
        assert_eq!(
            proj.get_section(sec_a).unwrap().shots.len(),
            4,
            "redo re-applies the shift"
        );
        assert!(proj.are_stations_merged(StationRef::new(sec_p, 0), StationRef::new(sec_q, 0)));
    }
}

#[test]
fn test_delete_range_prefix_shift_surfaces_comment() {
    // Codex re-review: prefix/suffix shifts drop station comments on removed
    // stations; the preview must surface them so the dialog can warn (the
    // warning was previously gated to interior splits only).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Prefix Comment".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    // Comment on station 1 (removed by the prefix range [0..=1]).
    assert!(
        state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(section_id, 1),
                comment: Some("note".to_string()),
            })
            .success
    );
    let (s0, s1) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[0].id, sec.shots[1].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(section_id, s0, s1).unwrap();
    assert!(
        impact
            .removed_comments
            .iter()
            .any(|sr| sr.station_index == 1),
        "prefix shift comment loss must be surfaced: {:?}",
        impact.removed_comments
    );
}

#[test]
fn test_delete_range_interior_shift_surfaces_boundary_comment() {
    // Codex re-review: the interior close-gap deletes end_pos+1 and drops a
    // comment there. With the gap-close available, the comment set covers the
    // shift removed set, so the boundary comment is surfaced before closing.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Boundary Comment".to_string(),
        location: None,
    });
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
            test_shot_data(4.0),
            test_shot_data(6.0),
        ],
    });
    let section_id = create.section_id.unwrap();
    // Comment on station 4 = end_pos+1 for the interior range [2..=3]. The split
    // keeps it; the gap-close drops it.
    assert!(
        state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(section_id, 4),
                comment: Some("boundary".to_string()),
            })
            .success
    );
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(section_id, s2, s3).unwrap();
    assert!(
        !impact.shift_blocked,
        "no datum, so the gap-close is available"
    );
    assert!(
        impact
            .removed_comments
            .iter()
            .any(|sr| sr.station_index == 4),
        "close-gap tail-boundary comment must be surfaced: {:?}",
        impact.removed_comments
    );
}

#[test]
fn test_split_remaps_station_offsets() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Station Offset Remap".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
        ],
    });
    let section_id = create_result.section_id.unwrap();

    // Directly insert a station offset for station 3 (simulates legacy offset data)
    {
        let mut proj = state.get_project_state().unwrap();
        proj.station_offsets
            .insert(StationRef::new(section_id, 3).into(), (1.0, 2.0, 3.0));
        state.set_project_for_tests(proj);
    }

    // Split at shot index 1
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);
    let new_section_id = result.section_id.unwrap();

    // Station offset should be remapped from (section_id, 3) to (new_section_id, 1)
    // split_station = 2, so 3 - 2 = 1
    let proj = state.get_project_state().unwrap();
    let new_ref = StationRef::new(new_section_id, 1);
    let offset = proj.station_offset_for_ref(new_ref);
    assert!(
        offset.is_some(),
        "Station offset should be remapped to new section"
    );
    let (ox, oy, oz) = offset.unwrap();
    assert!((ox - 1.0).abs() < 0.001);
    assert!((oy - 2.0).abs() < 0.001);
    assert!((oz - 3.0).abs() < 0.001);

    // Old ref should not exist
    let old_ref = StationRef::new(section_id, 3);
    assert!(
        !proj.station_offset_for_ref(old_ref).is_some(),
        "Old station offset should be removed"
    );
}

#[test]
fn test_split_undo_restores_original() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Undo Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
        ],
    });
    let section_id = create_result.section_id.unwrap();

    // Record initial state
    let initial_shot_count = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots.len()
    };
    let initial_section_count = state.get_project_state().unwrap().sections.len();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);

    // Verify split happened
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.sections.len(), initial_section_count + 1);

    // Undo
    let undo_result = state.undo();
    assert!(
        undo_result.success,
        "Undo should succeed: {:?}",
        undo_result.error
    );

    // Verify restored
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.sections.len(), initial_section_count);
    let section = proj.get_section(section_id).unwrap();
    assert_eq!(section.shots.len(), initial_shot_count);
    assert_eq!(section.name, Some("Original".to_string()));
}

#[test]
fn test_split_redo_resplits() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Redo Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
        ],
    });
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);
    let new_section_id = result.section_id.unwrap();

    // Undo
    let undo_result = state.undo();
    assert!(undo_result.success);

    // Redo
    let redo_result = state.redo();
    assert!(
        redo_result.success,
        "Redo should succeed: {:?}",
        redo_result.error
    );

    // Verify re-split: should have section A and section B again
    let proj = state.get_project_state().unwrap();
    let section_a = proj.get_section(section_id).unwrap();
    let section_b = proj.get_section(new_section_id).unwrap();
    assert_eq!(section_a.shots.len(), 1);
    assert_eq!(section_b.shots.len(), 1);
    assert_eq!(section_a.name, Some("A".to_string()));
    assert_eq!(section_b.name, Some("B".to_string()));
}

#[test]
fn test_split_rejects_edge_shot() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Edge Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(3.0),
        ],
    });
    let section_id = create_result.section_id.unwrap();

    // Try to split at first shot
    let first_shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };
    let result = state.execute_delete_shot_and_split(
        section_id,
        first_shot_id,
        "A".to_string(),
        "B".to_string(),
    );
    assert!(!result.success, "Should reject first shot");
    assert!(result.error.unwrap().contains("edge shot"));

    // Try to split at last shot
    let last_shot_id = {
        let proj = state.get_project_state().unwrap();
        let section = proj.get_section(section_id).unwrap();
        section.shots[section.shots.len() - 1].id
    };
    let result = state.execute_delete_shot_and_split(
        section_id,
        last_shot_id,
        "A".to_string(),
        "B".to_string(),
    );
    assert!(!result.success, "Should reject last shot");
    assert!(result.error.unwrap().contains("edge shot"));
}

// === delete_last_shot_and_section tests ===

#[test]
fn test_delete_last_shot_removes_section() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Delete Last Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Single".to_string()),
        surveyor: Some("Tester".to_string()),
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    assert!(create_result.success);
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(
        result.success,
        "Delete last shot should succeed: {:?}",
        result.error
    );

    // Verify section is gone
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.get_section(section_id).is_none(),
        "Section should be removed"
    );
}

#[test]
fn test_delete_last_shot_fails_if_multiple_shots() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Multi Shot Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("Multi".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
    });
    assert!(create_result.success);
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(!result.success, "Should fail with multiple shots");
    assert!(result.error.unwrap().contains("2 shots"));
}

#[test]
fn test_delete_last_shot_undo_restores() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Undo Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("WillDelete".to_string()),
        surveyor: Some("UndoTester".to_string()),
        date: None,
        color: "#aabbcc".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(result.success);

    // Verify deleted
    {
        let proj = state.get_project_state().unwrap();
        assert!(proj.get_section(section_id).is_none());
    }

    // Undo
    let undo_result = state.undo();
    assert!(
        undo_result.success,
        "Undo should succeed: {:?}",
        undo_result.error
    );

    // Verify restored
    let proj = state.get_project_state().unwrap();
    let section = proj
        .get_section(section_id)
        .expect("Section should be restored");
    assert_eq!(section.name, Some("WillDelete".to_string()));
    assert_eq!(section.surveyor_names(), Some("UndoTester".to_string()));
    assert_eq!(section.shots.len(), 1);
    assert_eq!(section.shots[0].id, shot_id);
    assert_eq!(section.shots[0].length, 5.0);
}

#[test]
fn test_delete_last_shot_cleans_up_offsets() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Offset Cleanup Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("WithOffset".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Manually set a section offset
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        project.section_offsets.insert(section_id, (10.0, 20.0));
    }

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(result.success);

    // Verify section offset is cleaned up
    let proj = state.get_project_state().unwrap();
    assert!(
        !proj.section_offsets.contains_key(&section_id),
        "Section offset should be removed"
    );
}

#[test]
fn test_delete_last_shot_cleans_up_merges() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Merge Cleanup Test".to_string(),
        location: None,
    });

    // Create two sections
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToDelete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let section_b_id = create_b.section_id.unwrap();

    // Merge station 1 of A with station 0 of B
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a_id, 1),
        station_b: StationRef::new(section_b_id, 0),
    });
    assert!(merge_result.success);

    // Verify merge exists
    {
        let proj = state.get_project_state().unwrap();
        assert!(
            !proj.station_merges.is_empty(),
            "Should have merge before delete"
        );
    }

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_a_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_a_id, shot_id);
    assert!(result.success);

    // Verify merge is cleaned up
    let proj = state.get_project_state().unwrap();
    let remaining_merges: Vec<_> = proj
        .station_merges
        .iter()
        .filter(|m| {
            m.station_a.section_id == section_a_id || m.station_b.section_id == section_a_id
        })
        .collect();
    assert!(
        remaining_merges.is_empty(),
        "Merges referencing deleted section should be removed"
    );
}

#[test]
fn test_delete_last_shot_cleans_up_station_offsets() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Station Offset Cleanup Test".to_string(),
        location: None,
    });

    let create_result = state.execute(Command::CreateSection {
        name: Some("WithStationOffset".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Manually set a station offset
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        let station_ref = StationRef::new(section_id, 0);
        project
            .station_offsets
            .insert(station_ref.into(), (1.0, 2.0, 3.0));
    }

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(result.success);

    // Verify station offsets for this section are cleaned up
    let proj = state.get_project_state().unwrap();
    let remaining_station_offsets: Vec<_> = proj
        .station_offsets
        .keys()
        .filter(|sr| sr.section_id == section_id)
        .collect();
    assert!(
        remaining_station_offsets.is_empty(),
        "Station offsets referencing deleted section should be removed"
    );
}

#[test]
fn test_delete_last_shot_cleans_up_closures() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Closure Cleanup Test".to_string(),
        location: None,
    });

    // Create two sections
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToDelete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0), test_shot_data(3.0)],
    });
    let section_b_id = create_b.section_id.unwrap();

    // Manually insert a loop closure referencing section A
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: StationRef::new(section_a_id, 0).into(),
            station_b: StationRef::new(section_b_id, 1).into(),
            path_length: 10.0,
            misclosure_distance: 0.5,
            error_percentage: 5.0,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.3,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Verify closure exists
    {
        let proj = state.get_project_state().unwrap();
        assert_eq!(
            proj.loop_closures.len(),
            1,
            "Should have 1 closure before delete"
        );
    }

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_a_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_a_id, shot_id);
    assert!(result.success);

    // Verify closure is cleaned up
    let proj = state.get_project_state().unwrap();
    let remaining_closures: Vec<_> = proj
        .loop_closures
        .iter()
        .filter(|lc| {
            lc.station_a.section_id == section_a_id || lc.station_b.section_id == section_a_id
        })
        .collect();
    assert!(
        remaining_closures.is_empty(),
        "Closures referencing deleted section should be removed"
    );
}

#[test]
fn test_delete_last_shot_undo_restores_references() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Undo Refs Test".to_string(),
        location: None,
    });

    // Create two sections
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToDelete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let section_b_id = create_b.section_id.unwrap();

    // Merge station 1 of A with station 0 of B
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a_id, 1),
        station_b: StationRef::new(section_b_id, 0),
    });
    assert!(merge_result.success);

    // Manually add a section offset and station offset for section A
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        // Note: merge already sets section_offsets, so add station offset
        project
            .station_offsets
            .insert(StationRef::new(section_a_id, 0).into(), (1.0, 2.0, 3.0));
        // Add a closure referencing section A
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: StationRef::new(section_a_id, 0).into(),
            station_b: StationRef::new(section_b_id, 1).into(),
            path_length: 10.0,
            misclosure_distance: 0.5,
            error_percentage: 5.0,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.3,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Capture counts before deletion
    let (merges_before, closures_before, station_offsets_a_before) = {
        let proj = state.get_project_state().unwrap();
        let merges = proj.station_merges.len();
        let closures = proj.loop_closures.len();
        let so = proj
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == section_a_id)
            .count();
        (merges, closures, so)
    };
    assert!(merges_before > 0);
    assert!(closures_before > 0);
    assert!(station_offsets_a_before > 0);

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_a_id).unwrap().shots[0].id
    };

    // Delete section A
    let result = state.execute_delete_last_shot_and_section(section_a_id, shot_id);
    assert!(result.success);

    // Verify everything cleaned up
    {
        let proj = state.get_project_state().unwrap();
        assert!(proj.get_section(section_a_id).is_none());
        let a_merges: Vec<_> = proj
            .station_merges
            .iter()
            .filter(|m| {
                m.station_a.section_id == section_a_id || m.station_b.section_id == section_a_id
            })
            .collect();
        assert!(a_merges.is_empty());
        let a_closures: Vec<_> = proj
            .loop_closures
            .iter()
            .filter(|lc| {
                lc.station_a.section_id == section_a_id || lc.station_b.section_id == section_a_id
            })
            .collect();
        assert!(a_closures.is_empty());
    }

    // Undo
    let undo_result = state.undo();
    assert!(
        undo_result.success,
        "Undo should succeed: {:?}",
        undo_result.error
    );

    // Verify all references restored
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.get_section(section_a_id).is_some(),
        "Section should be restored"
    );
    assert_eq!(
        proj.station_merges.len(),
        merges_before,
        "Merges should be restored"
    );
    assert_eq!(
        proj.loop_closures.len(),
        closures_before,
        "Closures should be restored"
    );
    let so_after = proj
        .station_offsets
        .keys()
        .filter(|sr| sr.section_id == section_a_id)
        .count();
    assert_eq!(
        so_after, station_offsets_a_before,
        "Station offsets should be restored"
    );
}

// === Section deletion cleanup undo/redo tests (fn-47-ok1.7) ===

#[test]
fn test_delete_section_undo_restores_relationships() {
    // Delete a section with merges, closures, offsets, zero datums.
    // Undo should restore all of them.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Cleanup Undo Test".to_string(),
        location: None,
    });

    // Create two sections
    let sec_a_result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let sec_a = sec_a_result.section_id.unwrap();

    let sec_b_result = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    let sec_b = sec_b_result.section_id.unwrap();

    // Merge sec_a:1 -> sec_b:0
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    // Set a zero datum on sec_a:0 so cleanup removes it
    let datum_result = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 0),
        coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    assert!(datum_result.success, "SetZeroDatum should succeed");

    // Directly insert a station offset for sec_a:0 (MoveStation now modifies shot data directly)
    {
        let mut proj = state.get_project_state().unwrap();
        proj.station_offsets
            .insert(StationRef::new(sec_a, 0).into(), (1.0, 2.0, 0.0));
        state.set_project_for_tests(proj);
    }

    // Capture state before deletion
    let proj_before = state.get_project_state().unwrap();
    let merges_before = proj_before.station_merges.len();
    let zero_datums_before = proj_before.zero_datums.len();
    let station_offsets_a_before = proj_before
        .station_offsets
        .keys()
        .filter(|sr| sr.section_id == sec_a)
        .count();
    assert!(merges_before >= 1, "Should have at least 1 merge");
    assert!(zero_datums_before >= 1, "Should have at least 1 zero datum");
    assert!(
        station_offsets_a_before >= 1,
        "Should have at least 1 station offset for sec_a"
    );

    // Delete section A (triggers cleanup of merges, offsets, zero datums)
    let del_result = state.execute(Command::DeleteSection { section_id: sec_a });
    assert!(del_result.success, "Delete should succeed");

    // Verify cleanup happened: merges, zero datums, station offsets
    let proj_after = state.get_project_state().unwrap();
    assert!(
        proj_after.get_section(sec_a).is_none(),
        "Section A should be gone"
    );
    assert_eq!(
        proj_after
            .station_merges
            .iter()
            .filter(|m| m.station_a.section_id == sec_a || m.station_b.section_id == sec_a)
            .count(),
        0,
        "No merges should reference deleted section A"
    );
    assert_eq!(
        proj_after
            .zero_datums
            .iter()
            .filter(|zd| zd.station_ref.section_id == sec_a)
            .count(),
        0,
        "No zero datums should reference deleted section A"
    );
    assert_eq!(
        proj_after
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == sec_a)
            .count(),
        0,
        "No station offsets should reference deleted section A"
    );

    // Undo should restore everything
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    let proj_restored = state.get_project_state().unwrap();
    assert!(
        proj_restored.get_section(sec_a).is_some(),
        "Section A should be restored"
    );
    assert_eq!(
        proj_restored.station_merges.len(),
        merges_before,
        "Merges should be fully restored"
    );
    assert_eq!(
        proj_restored.zero_datums.len(),
        zero_datums_before,
        "Zero datums should be fully restored"
    );
    assert_eq!(
        proj_restored
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == sec_a)
            .count(),
        station_offsets_a_before,
        "Station offsets should be fully restored"
    );

    // Redo should re-delete and re-cleanup
    let redo_result = state.redo();
    assert!(redo_result.success, "Redo should succeed");

    let proj_redone = state.get_project_state().unwrap();
    assert!(
        proj_redone.get_section(sec_a).is_none(),
        "Section A should be gone after redo"
    );
    assert_eq!(
        proj_redone
            .station_merges
            .iter()
            .filter(|m| m.station_a.section_id == sec_a || m.station_b.section_id == sec_a)
            .count(),
        0,
        "No merges should reference deleted section after redo"
    );
    assert_eq!(
        proj_redone
            .zero_datums
            .iter()
            .filter(|zd| zd.station_ref.section_id == sec_a)
            .count(),
        0,
        "No zero datums should reference deleted section after redo"
    );
    assert_eq!(
        proj_redone
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == sec_a)
            .count(),
        0,
        "No station offsets should reference deleted section after redo"
    );
}

// === Station Ref Shifting Tests ===

/// Helper: create a project with two sections, each with 2 shots, merged at station 0 of each.
/// Returns (state, section_a_id, section_b_id).
fn setup_merged_project() -> (AppState, u32, u32) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Shift Test".to_string(),
        location: None,
    });

    // Section A with 2 shots
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0), test_shot_data(20.0)],
    });
    let sec_a = result_a.section_id.unwrap();

    // Section B with 2 shots
    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(15.0), test_shot_data(25.0)],
    });
    let sec_b = result_b.section_id.unwrap();

    // Merge station 0 of A with station 0 of B
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 0),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    (state, sec_a, sec_b)
}

#[test]
fn test_insert_before_shifts_station_merge() {
    let (state, sec_a, sec_b) = setup_merged_project();

    // Verify initial merge is at station 0 of both sections
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.station_merges.len(), 1);
    assert!(proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 0)));
    assert!(proj.key_is_ref(&proj.station_merges[0].station_b, StationRef::new(sec_b, 0)));

    // Get first shot ID of section A
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;

    // Insert shot before first shot of section A
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: test_shot_data(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify merge shifted: station_a should now be station 1 of A (not 0)
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.station_merges.len(), 1);
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 1)),
        "Merge station_a should shift from 0 to 1 after insert_before"
    );
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_b, StationRef::new(sec_b, 0)),
        "Merge station_b (different section) should not shift"
    );
}

#[test]
fn test_insert_before_shifts_zero_datum() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "ZD Shift Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::CreateSection {
        name: Some("Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0), test_shot_data(20.0)],
    });
    let sec_id = result.section_id.unwrap();

    // Set zero datum on station 0
    let zd_result = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_id, 0),
        coordinates: GpsCoordinates::new(45.0, -73.5, 100.0).unwrap(),
    });
    assert!(zd_result.success);

    let proj = state.get_project_state().unwrap();
    assert!(proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_id, 0)));

    // Get first shot ID
    let first_shot_id = proj.sections[0].shots[0].id;

    // Insert shot before first shot
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_id,
        shot: test_shot_data(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify zero datum shifted to station 1
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_id, 1)),
        "Zero datum should shift from station 0 to station 1 after insert_before"
    );
}

#[test]
fn test_insert_before_shifts_station_offset() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Offset Shift Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::CreateSection {
        name: Some("Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0), test_shot_data(20.0)],
    });
    let sec_id = result.section_id.unwrap();

    // Directly add a station offset at station 0
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        project
            .station_offsets
            .insert(StationRef::new(sec_id, 0).into(), (1.0, 2.0, 3.0));
    }

    let proj = state.get_project_state().unwrap();
    assert!(proj
        .station_offset_for_ref(StationRef::new(sec_id, 0))
        .is_some());

    // Get first shot ID
    let first_shot_id = proj.sections[0].shots[0].id;

    // Insert before first shot
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_id,
        shot: test_shot_data(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify station offset key shifted from station 0 to station 1
    let proj = state.get_project_state().unwrap();
    assert!(
        !proj
            .station_offset_for_ref(StationRef::new(sec_id, 0))
            .is_some(),
        "Station offset at old index 0 should be gone"
    );
    assert_eq!(
        proj.station_offset_for_ref(StationRef::new(sec_id, 1)),
        Some((1.0, 2.0, 3.0)),
        "Station offset should have moved to station 1"
    );
}

#[test]
fn test_insert_before_shifts_loop_closure() {
    let (state, sec_a, sec_b) = setup_merged_project();

    // Need a third section connected to form a loop for closure
    let result_c = state.execute(Command::CreateSection {
        name: Some("Section C".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(12.0)],
    });
    let sec_c = result_c.section_id.unwrap();

    // Merge section C station 0 with section A station 2 (end of A)
    let merge2 = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 2),
        station_b: StationRef::new(sec_c, 0),
    });
    assert!(merge2.success);

    // Create loop closure between section B station 2 and section C station 1
    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(sec_b, 2),
        station_b: StationRef::new(sec_c, 1),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(closure_result.success);

    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.loop_closures.len(), 1);
    // The closure should reference sec_a station 2 for the merge
    assert_eq!(proj.station_merges.len(), 2);

    // Now insert shot before first shot of section A
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: test_shot_data(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify: merge at sec_a station 0 should shift to station 1
    // Verify: merge at sec_a station 2 should shift to station 3
    let proj = state.get_project_state().unwrap();
    let a_merges: Vec<_> = proj
        .station_merges
        .iter()
        .filter(|m| m.station_a.section_id == sec_a || m.station_b.section_id == sec_a)
        .collect();
    assert_eq!(a_merges.len(), 2);

    // Check the merge that was at station 0 of A
    let merge_a0 = proj
        .station_merges
        .iter()
        .find(|m| proj.key_is_ref(&m.station_b, StationRef::new(sec_b, 0)))
        .unwrap();
    assert!(
        proj.key_is_ref(&merge_a0.station_a, StationRef::new(sec_a, 1)),
        "Merge at original station 0 should shift to station 1"
    );

    // Check the merge that was at station 2 of A (now station 3)
    let merge_a2 = proj
        .station_merges
        .iter()
        .find(|m| {
            proj.key_is_ref(&m.station_b, StationRef::new(sec_c, 0))
                || proj.key_is_ref(&m.station_a, StationRef::new(sec_c, 0))
        })
        .unwrap();
    let a_ref = if merge_a2.station_a.section_id == sec_a {
        merge_a2.station_a.clone()
    } else {
        merge_a2.station_b.clone()
    };
    assert!(
        proj.key_is_ref(&a_ref, StationRef::new(sec_a, 3)),
        "Merge at original station 2 should shift to station 3"
    );

    // Loop closure refs should NOT have changed (they reference sec_b and sec_c, not sec_a)
    assert!(proj.key_is_ref(&proj.loop_closures[0].station_a, StationRef::new(sec_b, 2)));
    assert!(proj.key_is_ref(&proj.loop_closures[0].station_b, StationRef::new(sec_c, 1)));
}

#[test]
fn test_insert_before_undo_restores_refs() {
    let (state, sec_a, sec_b) = setup_merged_project();

    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;

    // Insert before first shot of section A
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: test_shot_data(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify merge shifted
    let proj = state.get_project_state().unwrap();
    assert!(proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 1)));

    // Undo the insert
    let undo_result = state.undo();
    assert!(undo_result.success);

    // Verify merge returned to original position
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 0)),
        "Merge should return to station 0 after undo"
    );
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_b, StationRef::new(sec_b, 0)),
        "Merge station_b should still be station 0 of B"
    );
}

#[test]
fn test_insert_before_redo_shifts_refs_again() {
    let (state, sec_a, sec_b) = setup_merged_project();

    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;

    // Insert before first shot
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: test_shot_data(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Undo
    let undo_result = state.undo();
    assert!(undo_result.success);

    // Redo
    let redo_result = state.redo();
    assert!(redo_result.success);

    // Verify merge shifted back to station 1
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 1)),
        "Merge should shift to station 1 after redo"
    );
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_b, StationRef::new(sec_b, 0)),
        "Merge station_b should stay at station 0 of B"
    );
}

#[test]
fn test_insert_after_only_shifts_refs_beyond() {
    let (state, sec_a, _sec_b) = setup_merged_project();

    // Also add a station offset at station 1 of section A
    {
        let mut proj_guard = state.project.lock().unwrap();
        let project = proj_guard.as_mut().unwrap();
        project
            .station_offsets
            .insert(StationRef::new(sec_a, 1).into(), (5.0, 6.0, 7.0));
    }

    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;

    // Insert after first shot (position 0) - new shot at position 1
    // This should shift station refs >= station 2 (pos+2=2)
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: test_shot_data(7.0),
        after_shot_id: Some(first_shot_id),
        before_shot_id: None,
    });
    assert!(insert_result.success);

    let proj = state.get_project_state().unwrap();

    // Station merge at sec_a station 0 should NOT shift (0 < 2)
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 0)),
        "Merge at station 0 should not shift for insert_after shot 0"
    );

    // Station offset at station 1 should NOT shift (1 < 2)
    assert!(
        proj.station_offset_for_ref(StationRef::new(sec_a, 1))
            .is_some(),
        "Station offset at station 1 should not shift for insert_after shot 0"
    );
}

#[test]
fn test_delete_shot_undo_redo_with_snapshot() {
    let (state, sec_a, _sec_b) = setup_merged_project();

    // Also set a zero datum at station 2 of section A
    let zd_result = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 2),
        coordinates: GpsCoordinates::new(45.0, -73.5, 100.0).unwrap(),
    });
    assert!(zd_result.success);

    let proj = state.get_project_state().unwrap();
    let sec_a_shots: Vec<_> = proj
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .iter()
        .collect();
    let first_shot_id = sec_a_shots[0].id;

    // Delete first shot - station refs should shift
    let delete_result = state.execute(Command::DeleteShot {
        section_id: sec_a,
        shot_id: first_shot_id,
    });
    assert!(delete_result.success);

    let proj = state.get_project_state().unwrap();
    // After deleting shot 0: merge referencing deleted station 0 should be removed
    assert!(
        proj.station_merges.is_empty(),
        "Merges referencing deleted station should be removed"
    );
    // Zero datum was at station 2, should shift to station 1
    assert!(
        proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_a, 1)),
        "Zero datum should shift from station 2 to station 1 after delete shot 0"
    );

    // Undo the delete - should restore via ProjectSnapshotPair
    let undo_result = state.undo();
    assert!(undo_result.success);

    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.sections
            .iter()
            .find(|s| s.id == sec_a)
            .unwrap()
            .shots
            .len(),
        2
    );
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 0)),
        "Merge should be at station 0 after undo"
    );
    assert!(
        proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_a, 2)),
        "Zero datum should be at station 2 after undo"
    );

    // Redo the delete
    let redo_result = state.redo();
    assert!(redo_result.success);

    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.sections
            .iter()
            .find(|s| s.id == sec_a)
            .unwrap()
            .shots
            .len(),
        1
    );
    assert!(
        proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_a, 1)),
        "Zero datum should be at station 1 after redo"
    );
}

// === Section offset adjustment tests for insert_before at pos=0 ===

/// Create a shot with a known azimuth and length (both readings same = no ambiguity)
fn directional_shot_data(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    }
}

#[test]
fn test_insert_before_pos0_adjusts_section_offsets() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Offset Test".to_string(),
        location: None,
    });

    // Section with 2 shots heading north (azimuth 0)
    let result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            directional_shot_data(0.0, 10.0),
            directional_shot_data(0.0, 5.0),
        ],
    });
    let sec_a = result.section_id.unwrap();

    // Capture initial coordinates
    let orig_coords: Vec<_> = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        stations
            .iter()
            .filter(|s| s.station_ref.section_id == sec_a)
            .map(|s| {
                (
                    s.station_ref.station_index,
                    s.coordinates.x,
                    s.coordinates.y,
                )
            })
            .collect()
    };
    // station 0: (0, 0), station 1: (0, 10), station 2: (0, 15)
    assert!((orig_coords[0].1).abs() < 1e-6); // x=0
    assert!((orig_coords[0].2).abs() < 1e-6); // y=0
    assert!((orig_coords[1].2 - 10.0).abs() < 1e-6); // y=10
    assert!((orig_coords[2].2 - 15.0).abs() < 1e-6); // y=15

    // Insert shot heading east (azimuth 90, length 3) before first shot
    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: directional_shot_data(90.0, 3.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // After insert: old station 0 is now station 1.
    // New shot displacement: dx=3*sin(90)=3, dy=3*cos(90)=0
    // Section offset adjusted by (-3, 0)
    // Station 1 (old 0) should be at (0, 0) - unchanged
    // Station 2 (old 1) should be at (0, 10) - unchanged
    // Station 3 (old 2) should be at (0, 15) - unchanged
    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();

    let s0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 0))
        .unwrap();
    let s1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 1))
        .unwrap();
    let s2 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 2))
        .unwrap();
    let s3 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 3))
        .unwrap();

    // Station 0 (new) at section_offset = (-3, 0)
    assert!(
        (s0.coordinates.x - (-3.0)).abs() < 1e-6,
        "New station 0 x={}",
        s0.coordinates.x
    );
    assert!(
        (s0.coordinates.y).abs() < 1e-6,
        "New station 0 y={}",
        s0.coordinates.y
    );

    // Station 1 (was station 0): (0, 0) - same as original station 0
    assert!(
        (s1.coordinates.x).abs() < 1e-6,
        "Station 1 (was 0) x={}",
        s1.coordinates.x
    );
    assert!(
        (s1.coordinates.y).abs() < 1e-6,
        "Station 1 (was 0) y={}",
        s1.coordinates.y
    );

    // Station 2 (was station 1): (0, 10) - same as original station 1
    assert!(
        (s2.coordinates.x).abs() < 1e-6,
        "Station 2 (was 1) x={}",
        s2.coordinates.x
    );
    assert!(
        (s2.coordinates.y - 10.0).abs() < 1e-6,
        "Station 2 (was 1) y={}",
        s2.coordinates.y
    );

    // Station 3 (was station 2): (0, 15) - same as original station 2
    assert!(
        (s3.coordinates.x).abs() < 1e-6,
        "Station 3 (was 2) x={}",
        s3.coordinates.x
    );
    assert!(
        (s3.coordinates.y - 15.0).abs() < 1e-6,
        "Station 3 (was 2) y={}",
        s3.coordinates.y
    );
}

#[test]
fn test_insert_before_pos0_adjusts_merged_section_offsets() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Merged Offset Test".to_string(),
        location: None,
    });

    // Section A: 2 shots north
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            directional_shot_data(0.0, 10.0),
            directional_shot_data(0.0, 5.0),
        ],
    });
    let sec_a = result_a.section_id.unwrap();

    // Section B: 1 shot east
    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![directional_shot_data(90.0, 8.0)],
    });
    let sec_b = result_b.section_id.unwrap();

    // Merge station 0 of A with station 0 of B
    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 0),
        station_b: StationRef::new(sec_b, 0),
    });

    // Capture section B station coordinates before insert
    let (b0_before, b1_before) = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        let b0 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(sec_b, 0))
            .unwrap()
            .coordinates;
        let b1 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(sec_b, 1))
            .unwrap()
            .coordinates;
        (b0, b1)
    };

    // Insert shot heading east before first shot of section A
    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: directional_shot_data(90.0, 3.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Section B is in the same merged group - its stations should NOT have moved
    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    let b0_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_b, 0))
        .unwrap()
        .coordinates;
    let b1_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_b, 1))
        .unwrap()
        .coordinates;

    assert!(
        (b0_after.x - b0_before.x).abs() < 1e-6,
        "B station 0 x moved: {} -> {}",
        b0_before.x,
        b0_after.x
    );
    assert!(
        (b0_after.y - b0_before.y).abs() < 1e-6,
        "B station 0 y moved: {} -> {}",
        b0_before.y,
        b0_after.y
    );
    assert!(
        (b1_after.x - b1_before.x).abs() < 1e-6,
        "B station 1 x moved: {} -> {}",
        b1_before.x,
        b1_after.x
    );
    assert!(
        (b1_after.y - b1_before.y).abs() < 1e-6,
        "B station 1 y moved: {} -> {}",
        b1_before.y,
        b1_after.y
    );
}

#[test]
fn test_insert_before_pos0_undo_restores_offsets() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Undo Offset Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![directional_shot_data(0.0, 10.0)],
    });
    let sec_a = result.section_id.unwrap();

    // Capture coordinates before insert
    let orig_coords: Vec<_> = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        stations
            .iter()
            .filter(|s| s.station_ref.section_id == sec_a)
            .map(|s| (s.station_ref, s.coordinates))
            .collect()
    };

    // Insert shot heading east before first shot
    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: directional_shot_data(90.0, 5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify offset was adjusted
    {
        let proj = state.get_project_state().unwrap();
        let offset = proj
            .section_offsets
            .get(&sec_a)
            .copied()
            .unwrap_or((0.0, 0.0));
        assert!(
            (offset.0 - (-5.0)).abs() < 1e-6,
            "Section offset x should be -5, got {}",
            offset.0
        );
    }

    // Undo
    let undo_result = state.undo();
    assert!(undo_result.success);

    // Coordinates should be exactly as before (section has 2 stations again)
    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    for (ref_orig, coord_orig) in &orig_coords {
        let station = stations
            .iter()
            .find(|s| s.station_ref == *ref_orig)
            .unwrap();
        assert!(
            (station.coordinates.x - coord_orig.x).abs() < 1e-6,
            "Station {:?} x should match after undo: {} vs {}",
            ref_orig,
            station.coordinates.x,
            coord_orig.x
        );
        assert!(
            (station.coordinates.y - coord_orig.y).abs() < 1e-6,
            "Station {:?} y should match after undo: {} vs {}",
            ref_orig,
            station.coordinates.y,
            coord_orig.y
        );
    }

    // Redo
    let redo_result = state.redo();
    assert!(redo_result.success);

    // Old stations should be at the same position again (not moved)
    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    // Old station 0 is now station 1
    let s1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 1))
        .unwrap();
    assert!(
        (s1.coordinates.x - orig_coords[0].1.x).abs() < 1e-6,
        "Station 1 (was 0) x after redo: {} vs {}",
        s1.coordinates.x,
        orig_coords[0].1.x
    );
    assert!(
        (s1.coordinates.y - orig_coords[0].1.y).abs() < 1e-6,
        "Station 1 (was 0) y after redo: {} vs {}",
        s1.coordinates.y,
        orig_coords[0].1.y
    );
    // Old station 1 is now station 2
    let s2 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 2))
        .unwrap();
    assert!(
        (s2.coordinates.x - orig_coords[1].1.x).abs() < 1e-6,
        "Station 2 (was 1) x after redo: {} vs {}",
        s2.coordinates.x,
        orig_coords[1].1.x
    );
    assert!(
        (s2.coordinates.y - orig_coords[1].1.y).abs() < 1e-6,
        "Station 2 (was 1) y after redo: {} vs {}",
        s2.coordinates.y,
        orig_coords[1].1.y
    );
}

#[test]
fn test_insert_before_pos_gt0_no_offset_adjustment() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "No Offset Test".to_string(),
        location: None,
    });

    let result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            directional_shot_data(0.0, 10.0),
            directional_shot_data(0.0, 5.0),
        ],
    });
    let sec_a = result.section_id.unwrap();

    // Get the section offset before insert
    let offset_before = {
        let proj = state.get_project_state().unwrap();
        proj.section_offsets
            .get(&sec_a)
            .copied()
            .unwrap_or((0.0, 0.0))
    };

    // Insert before the SECOND shot (pos=1, not pos=0)
    let proj = state.get_project_state().unwrap();
    let second_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[1].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: directional_shot_data(90.0, 3.0),
        after_shot_id: None,
        before_shot_id: Some(second_shot_id),
    });
    assert!(insert_result.success);

    // Section offset should NOT have changed (only pos=0 triggers adjustment)
    let proj = state.get_project_state().unwrap();
    let offset_after = proj
        .section_offsets
        .get(&sec_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (offset_after.0 - offset_before.0).abs() < 1e-6,
        "Section offset x should not change for pos>0"
    );
    assert!(
        (offset_after.1 - offset_before.1).abs() < 1e-6,
        "Section offset y should not change for pos>0"
    );
}

// ============================================================================
// Coverage gap tests: project_io.rs
// ============================================================================

#[test]
fn test_save_project_no_project_open() {
    let state = AppState::new().unwrap();
    let result = state.save_project(Some(PathBuf::from("/tmp/test.stalx")));
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No project open"));
}

#[test]
fn test_save_project_no_path_specified() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    // No file_path on project and no path argument
    let result = state.save_project(None);
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No save path"));
}

#[test]
fn test_save_project_implicit_first_save_autorename_when_target_exists() {
    let temp_dir = TempDir::new().unwrap();
    let existing_path = temp_dir.path().join("existing.stalx");
    let expected_autorenamed_path = temp_dir.path().join("existing_1.stalx");
    std::fs::write(&existing_path, b"ORIGINAL").unwrap();

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Simulate deferred-create behavior: reserved file_path but no open database yet.
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        project.file_path = Some(existing_path.clone());
    }
    {
        let mut db_guard = state.database.lock().unwrap();
        *db_guard = None;
    }

    let result = state.save_project(None);
    assert!(
        result.success,
        "unexpected save failure: {:?}",
        result.error
    );

    // Existing file must remain untouched.
    let bytes = std::fs::read(&existing_path).unwrap();
    assert_eq!(bytes, b"ORIGINAL");

    // Autosave should pick an available suffixed filename.
    assert!(expected_autorenamed_path.exists());
    let project = state.get_project_state().unwrap();
    assert_eq!(project.file_path.as_ref(), Some(&expected_autorenamed_path));
}

#[test]
fn test_save_project_adds_stalx_extension() {
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    // Save without .stalx extension
    let save_path = temp_dir.path().join("myproject");
    let result = state.save_project(Some(save_path));
    assert!(result.success);
    assert!(temp_dir.path().join("myproject.stalx").exists());
}

#[test]
fn test_save_project_cleans_up_temp_on_reopen_failure() {
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let save_path = temp_dir.path().join("project.stalx");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    // Verify temp file was cleaned up
    let temp_path = save_path.with_extension("stalx.tmp");
    assert!(
        !temp_path.exists(),
        "temp file should be cleaned up after save"
    );

    // Verify the project file_path was updated
    let project = state.get_project_state().unwrap();
    assert_eq!(project.file_path.as_ref(), Some(&save_path));
}

#[test]
fn test_save_project_atomic_rename_preserves_original_on_temp_failure() {
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Original".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });

    // First save
    let save_path = temp_dir.path().join("atomic.stalx");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    // Modify project
    state.execute(Command::CreateSection {
        name: Some("Sec2".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });

    // Save again - should succeed and update the file
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    // Reload and verify both sections are there
    let db = Database::open_readonly(&save_path).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.sections.len(), 2);
}

#[test]
fn test_save_project_persists_undo_redo_stacks() {
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "UndoPersist".to_string(),
        location: None,
    });

    // Create section (pushes to undo stack)
    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    // Create another section, then undo it (creates redo entry)
    state.execute(Command::CreateSection {
        name: Some("Sec2".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    state.undo();

    let save_path = temp_dir.path().join("undo-persist.stalx");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    // Reload and verify undo/redo stacks are persisted
    let db = Database::open_existing(&save_path).unwrap();
    let undo = db.load_undo_stack_with_timestamps().unwrap_or_default();
    let redo = db.load_redo_stack_with_timestamps().unwrap_or_default();
    assert!(!undo.is_empty(), "undo stack should be persisted");
    assert!(!redo.is_empty(), "redo stack should be persisted");
}

#[test]
fn failed_atomic_history_replacement_falls_back_to_empty_stacks() {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("history-fallback.stalx");
    let db = Database::open(&path).unwrap();
    db.save_undo_stack_with_timestamps(vec![(
        r#"{"type":"OldUndo"}"#.to_owned(),
        r#"{"kind":"old"}"#.to_owned(),
        None,
    )])
    .unwrap();
    db.save_redo_stack_with_timestamps(vec![(
        r#"{"type":"OldRedo"}"#.to_owned(),
        r#"{"kind":"old"}"#.to_owned(),
        None,
    )])
    .unwrap();

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "History fallback".to_owned(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("Redo candidate".to_owned()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_owned(),
        direction: None,
        shots: vec![test_shot_data(2.0)],
    });
    state.undo();

    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_redo_insert BEFORE INSERT ON redo_stack \
             BEGIN SELECT RAISE(FAIL, 'redo rejected'); END;",
        )
        .unwrap();

    let dropped = state
        .persist_or_clear_undo_stacks_into(&db)
        .expect("the empty-history fallback should succeed");

    assert!(dropped, "the caller must surface the history-loss warning");
    assert!(db.load_undo_stack_with_timestamps().unwrap().is_empty());
    assert!(db.load_redo_stack_with_timestamps().unwrap().is_empty());
}

#[test]
fn unreadable_undo_markers_save_without_history() {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("unreadable-undo-markers.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Marker fallback".to_owned(), None))
            .unwrap();
        db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
        db.save_speleo_undo_chain(Some("older-tip")).unwrap();
        db.save_speleo_undo_watermark(Some("2026-08-01T00:00:00+00:00"))
            .unwrap();
    }

    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    state.undo_stack.lock().unwrap().push_with_data(
        Command::CreateSection {
            name: Some("Local edit".to_owned()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_owned(),
            direction: None,
            shots: Vec::new(),
        },
        UndoData::None,
    );
    state.undo_stack.lock().unwrap().push_with_data(
        Command::CreateSection {
            name: Some("Redo candidate".to_owned()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_owned(),
            direction: None,
            shots: Vec::new(),
        },
        UndoData::None,
    );
    state.undo();

    // Keep the table present but remove the column the marker reader requires. The
    // survey database and undo rows remain readable, isolating the marker-failure path.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "DROP TABLE speleo_undo_watermark;
             CREATE TABLE speleo_undo_watermark (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 broken_value TEXT NOT NULL
             );
             INSERT INTO speleo_undo_watermark (id, broken_value) VALUES (1, 'broken');",
        )
        .unwrap();

    let result = state.save_project(None);

    assert!(result.success, "the survey save continues: {result:?}");
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.code == "UNDO_HISTORY_DROPPED"),
        "history loss must be visible to the caller: {result:?}"
    );
    let db_guard = state.database.lock().unwrap();
    let db = db_guard.as_ref().expect("saved database reopened");
    assert!(db.load_undo_stack_with_timestamps().unwrap().is_empty());
    assert!(db.load_redo_stack_with_timestamps().unwrap().is_empty());
    assert_eq!(db.load_speleo_undo_chain().unwrap(), None);
    assert_eq!(db.load_speleo_undo_watermark().unwrap(), None);
    drop(db_guard);

    let undo_stack = state.undo_stack.lock().unwrap();
    assert!(
        !undo_stack.can_undo(),
        "the live undo stack must be cleared"
    );
    assert!(
        !undo_stack.can_redo(),
        "the live redo stack must be cleared"
    );
    drop(undo_stack);

    // The replacement database is valid now. A later save must not resurrect the
    // unclassified rows from memory after the marker-failure fallback has cleared them.
    assert!(state.save_project(None).success);
    let db_guard = state.database.lock().unwrap();
    let db = db_guard
        .as_ref()
        .expect("database reopened after second save");
    assert!(db.load_undo_stack_with_timestamps().unwrap().is_empty());
    assert!(db.load_redo_stack_with_timestamps().unwrap().is_empty());
}

/// [#303] Changing the undo mode really does need the edit gate.
///
/// The sibling test in `undo_policy.rs` asserts this with NO project open, so
/// `speleodb_validated_link()?` errors one line before the `edit_enabled()` gate is ever
/// reached - delete the gate and it stays green. This one opens a real linked project so
/// the gate is the thing under test, and its companion source-text tripwire only pins the
/// gate's POSITION, not that it fires.
#[test]
fn setting_the_undo_mode_on_a_linked_project_needs_the_edit_gate() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("undo-mode-gate.stalx");
    {
        let mut db = Database::open(&save_path).unwrap();
        db.save_project(&Project::new("Linked".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
    }

    let state = AppState::new().unwrap();
    assert!(state.open_project(save_path).success);
    // A remote open leaves editing disabled, which is exactly the state under test.
    assert!(!state.edit_enabled(), "a remote open starts read-only");

    let err = state
        .speleodb_set_undo_mode(crate::models::UndoMode::Disabled)
        .expect_err("a read-only session must not change what the project ships");
    assert!(
        err.message.contains("start editing"),
        "the refusal must be the edit-gate one, not the absent-link one, got {err:?}"
    );

    // ...and with the gate open the same call succeeds, so the test is pinning the gate
    // rather than some unrelated refusal.
    state.set_edit_enabled(true);
    state
        .speleodb_set_undo_mode(crate::models::UndoMode::Disabled)
        .expect("with an edit session the override is settable");

    state.set_edit_enabled(false);
    state
        .speleodb_set_undo_mode(crate::models::UndoMode::Full)
        .expect_err("a released session cannot change the project policy");
}

#[test]
fn setting_the_undo_mode_serializes_with_full_file_saves() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("undo-mode-save-lock.stalx");
    {
        let mut db = Database::open(&save_path).unwrap();
        db.save_project(&Project::new("Linked".to_owned(), None))
            .unwrap();
        db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
    }

    let state = Arc::new(AppState::new().unwrap());
    assert!(state.open_project(save_path).success);
    state.set_edit_enabled(true);
    let save_guard = state.project.lock().unwrap();
    let worker_state = Arc::clone(&state);
    let (sent, received) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = worker_state.speleodb_set_undo_mode(crate::models::UndoMode::Disabled);
        sent.send(result).unwrap();
    });

    assert!(
        received
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "the mode write ran while a full-file save owned the project lock"
    );
    drop(save_guard);
    received
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("mode write completes after the save lock is released")
        .expect("mode write succeeds");
    worker.join().unwrap();

    let db_guard = state.database.lock().unwrap();
    let link = db_guard
        .as_ref()
        .unwrap()
        .load_speleo_link()
        .unwrap()
        .unwrap();
    assert_eq!(link.undo_mode, Some(crate::models::UndoMode::Disabled));
    assert!(link.has_unpushed_edits);
}

/// [#303] An undo snapshot must not carry this machine's filesystem paths.
///
/// Full mode ships `undo_stack` to the server on purpose, and every `ProjectSnapshot` /
/// `ProjectSnapshotPair` is a whole serialized `Project` - so an unscrubbed `file_path`
/// writes `/home/<user>/…` or `C:\Users\<Real Name>\…` into the project's git history,
/// permanently and readable by every collaborator. Snapshot pairs come from Delete shot,
/// Delete section, Reverse section and every Batch import, so this is routine editing.
#[test]
fn a_project_snapshot_carries_no_local_filesystem_path() {
    let mut project = Project::new("Leaky".to_string(), None);
    project.file_path = Some(PathBuf::from("/home/victim/caves/secret-sump.stalx"));

    let json = AppState::serialize_project_snapshot(&project).expect("serializes");

    assert!(
        !json.contains("victim"),
        "the snapshot leaked a local path: {json}"
    );
    assert!(
        !json.contains("secret-sump.stalx"),
        "the snapshot leaked a local filename: {json}"
    );
    // The survey itself must still be there - scrubbing must not cost the payload.
    assert!(json.contains("Leaky"), "the snapshot lost the project name");
}

/// ...and restoring one must not lose track of where the file lives.
///
/// The path is cleared on the way out, so the way back has to re-apply the live value or
/// an undo would leave the app believing the project was never saved - and the next save
/// would prompt for a location.
#[test]
fn restoring_a_snapshot_keeps_the_live_file_path() {
    let mut live = Project::new("Cave".to_string(), None);
    live.file_path = Some(PathBuf::from("/home/me/cave.stalx"));
    let snapshot = AppState::serialize_project_snapshot(&live).expect("serializes");

    let mut slot = Some(live);
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));

    assert_eq!(
        slot.as_ref().and_then(|p| p.file_path.clone()),
        Some(PathBuf::from("/home/me/cave.stalx")),
        "the restore must carry the live path across, not the snapshot's (absent) one"
    );
}

#[test]
fn persisted_split_snapshot_carries_no_local_filesystem_path() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Private split".to_string(),
        location: None,
    });
    let created = state.execute(Command::CreateSection {
        name: Some("Single".to_string()),
        surveyor: None,
        date: None,
        color: "#123456".to_string(),
        direction: None,
        shots: vec![test_shot_data(10.0)],
    });
    let section_id = created.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;
    state.project.lock().unwrap().as_mut().unwrap().file_path =
        Some(PathBuf::from("/home/alice/private/split.stalx"));

    let result = state.execute_split_shot_at_distance(section_id, shot_id, 4.0);
    assert!(result.success, "split failed: {:?}", result.error);

    let stack = state.undo_stack.lock().unwrap();
    let entry = stack.undo_entries().last().expect("split undo entry");
    let (_, persisted_undo, _) = UndoStack::serialize_entry_with_timestamp(entry).unwrap();
    assert!(
        !persisted_undo.contains("alice"),
        "path leaked: {persisted_undo}"
    );
    assert!(
        !persisted_undo.contains("split.stalx"),
        "path leaked: {persisted_undo}"
    );
}

#[test]
fn persisted_delete_last_snapshot_carries_no_local_filesystem_path() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Private delete".to_string(),
        location: None,
    });
    let created = state.execute(Command::CreateSection {
        name: Some("Single".to_string()),
        surveyor: None,
        date: None,
        color: "#654321".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = created.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;
    state.project.lock().unwrap().as_mut().unwrap().file_path =
        Some(PathBuf::from("/home/bob/private/delete.stalx"));

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(result.success, "delete failed: {:?}", result.error);

    let stack = state.undo_stack.lock().unwrap();
    let entry = stack.undo_entries().last().expect("delete undo entry");
    let (_, persisted_undo, _) = UndoStack::serialize_entry_with_timestamp(entry).unwrap();
    assert!(
        !persisted_undo.contains("bob"),
        "path leaked: {persisted_undo}"
    );
    assert!(
        !persisted_undo.contains("delete.stalx"),
        "path leaked: {persisted_undo}"
    );
}

/// [#303] A Full-mode linked project persists BOTH stacks, redo included.
///
/// Pinned because the opposite was tried. The byte budget is applied to the undo and
/// redo stacks independently, so a Full-mode save can write up to twice
/// `UNDO_BYTE_BUDGET_BYTES` - which reads like a bug, and was briefly "fixed" by
/// dropping redo from the Full-mode save. It was reversed: 64 MiB is a ceiling rather
/// than growth (each stack is budgeted, and a confirmed push clears the undo history),
/// and persisting redo is what keeps a redo branch alive across a close and reopen.
///
/// So this test exists to fail if someone re-derives that fix. None of the redo half
/// reaches the server in any mode - `redo_stack` is unconditionally in
/// `PUSH_SNAPSHOT_LOCAL_TABLES` - so the cost is local and so is the benefit.
#[test]
fn full_mode_save_persists_both_stacks() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("full-mode-undo.stalx");

    // Seed a linked `.stalx` with the per-project override set explicitly, so the
    // policy does not depend on whatever the global default happens to be.
    {
        let mut db = Database::open(&save_path).unwrap();
        db.save_project(&Project::new("Linked".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&SpeleoLink {
            undo_mode: Some(crate::models::UndoMode::Full),
            ..fixture_speleo_link()
        }))
        .unwrap();
    }

    let state = AppState::new().unwrap();
    assert!(state.open_project(save_path.clone()).success);
    // A remote open leaves editing disabled; this test is about the save path, not the
    // lock, so take the write side the way a lock-acquire would.
    state.set_edit_enabled(true);

    state.execute(Command::CreateSection {
        name: Some("Kept".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    state.execute(Command::CreateSection {
        name: Some("Undone".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    state.undo();
    assert!(
        state.can_redo(),
        "the undo must leave a redo branch in memory"
    );

    let saved = state.save_project(None);
    assert!(saved.success, "save failed: {:?}", saved.error);

    let db = Database::open_existing(&save_path).unwrap();
    assert!(
        !db.load_undo_stack_with_timestamps()
            .unwrap_or_default()
            .is_empty(),
        "Full mode persists the undo history - that is what rides in the next commit"
    );
    assert!(
        !db.load_redo_stack_with_timestamps()
            .unwrap_or_default()
            .is_empty(),
        "Full mode persists redo too - the 2x save cost is an accepted ceiling, and \
         dropping it would cost redo-after-reopen"
    );
}

#[test]
fn test_open_project_missing_file() {
    let state = AppState::new().unwrap();
    let result = state.open_project(PathBuf::from("/nonexistent/path/project.stalx"));
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("File not found"));
}

#[test]
fn test_open_project_clears_previous_state() {
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();

    // Create and save project A
    state.execute(Command::CreateProject {
        name: "Project A".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("SecA".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let path_a = temp_dir.path().join("a.stalx");
    state.save_project(Some(path_a.clone()));

    // Create and save project B
    state.execute(Command::CreateProject {
        name: "Project B".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("SecB1".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(3.0)],
    });
    state.execute(Command::CreateSection {
        name: Some("SecB2".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![test_shot_data(4.0)],
    });
    let path_b = temp_dir.path().join("b.stalx");
    state.save_project(Some(path_b.clone()));

    // Open project A - should replace B's state
    let result = state.open_project(path_a);
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.name, "Project A");
    assert_eq!(project.sections.len(), 1);
}

#[test]
fn test_close_project_clears_all_state() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "ToClose".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    let result = state.close_project();
    assert!(result.success);
    assert!(state.get_project_state().is_none());
    let undo_stack = state.undo_stack.lock().unwrap();
    assert_eq!(undo_stack.len(), 0);
}

#[test]
fn test_close_project_clears_adjusted_cache() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "ToClose".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });

    // Warm the memoized closure-adjusted coordinates cache. For a large project
    // this is a project-scale allocation (one `Station` per station), so closing
    // must drop it for RSS to shrink back.
    assert!(state.get_adjusted_stations().is_some());
    assert!(
        state.adjusted_cache.lock().unwrap().is_some(),
        "adjusted cache should be warm after get_adjusted_stations"
    );

    let result = state.close_project();
    assert!(result.success);

    // close_project must free both project-scale in-memory caches. The undo stack
    // is reloaded from SQLite on the next open, so clearing it here is safe.
    assert!(
        state.adjusted_cache.lock().unwrap().is_none(),
        "adjusted cache must be cleared on close"
    );
    assert_eq!(state.undo_stack.lock().unwrap().len(), 0);
}

// ============================================================================
// Coverage gap tests: merge_projects edge cases
// ============================================================================

#[test]
fn test_merge_projects_output_already_exists() {
    let temp_dir = TempDir::new().unwrap();

    // Create two source projects
    let mut proj_a = Project::new("A".to_string(), None);
    proj_a.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let mut proj_b = Project::new("B".to_string(), None);
    proj_b.add_section(SectionData {
        shots: vec![test_shot_data(2.0)],
        ..Default::default()
    });

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&proj_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&proj_b).unwrap();
    drop(db_a);
    drop(db_b);

    // Create output file that already exists
    let output_path = temp_dir.path().join("output.stalx");
    std::fs::write(&output_path, b"existing").unwrap();

    let state = AppState::new().unwrap();
    let result = state.merge_projects(
        vec![source_a, source_b],
        output_path,
        "Merged".to_string(),
        false,
    );
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("already exists"));
}

#[test]
fn test_merge_projects_extension_less_output_detects_existing_stalx() {
    // An extension-less output path (e.g. `output`) must be normalized to
    // `output.stalx` BEFORE the existence check, so a merge can't silently
    // overwrite an existing `output.stalx` (#169). Without normalize-first,
    // `output.exists()` is false and the merge clobbers `output.stalx`.
    let temp_dir = TempDir::new().unwrap();

    let mut proj_a = Project::new("A".to_string(), None);
    proj_a.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let mut proj_b = Project::new("B".to_string(), None);
    proj_b.add_section(SectionData {
        shots: vec![test_shot_data(2.0)],
        ..Default::default()
    });
    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&proj_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&proj_b).unwrap();
    drop(db_a);
    drop(db_b);

    // An existing `output.stalx`, merged into via an EXTENSION-LESS output arg.
    let existing = temp_dir.path().join("output.stalx");
    std::fs::write(&existing, b"existing").unwrap();
    let output_extless = temp_dir.path().join("output"); // no .stalx

    let state = AppState::new().unwrap();
    let result = state.merge_projects(
        vec![source_a, source_b],
        output_extless,
        "Merged".to_string(),
        false,
    );
    assert!(
        !result.success,
        "merge must detect the existing output.stalx behind an extension-less path"
    );
    assert!(result.error.unwrap_or_default().contains("already exists"));
    // The existing file must be untouched (not clobbered).
    assert_eq!(std::fs::read(&existing).unwrap(), b"existing");
}

#[test]
fn test_merge_projects_source_missing() {
    let temp_dir = TempDir::new().unwrap();

    let mut proj_a = Project::new("A".to_string(), None);
    proj_a.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let source_a = temp_dir.path().join("a.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&proj_a).unwrap();
    drop(db_a);

    let missing = temp_dir.path().join("missing.stalx");
    let output_path = temp_dir.path().join("output.stalx");

    let state = AppState::new().unwrap();
    let result = state.merge_projects(
        vec![source_a, missing],
        output_path,
        "Merged".to_string(),
        false,
    );
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("not found"));
}

#[test]
fn test_merge_projects_combines_locations() {
    let temp_dir = TempDir::new().unwrap();

    let mut proj_a = Project::new("A".to_string(), Some("Cave Alpha".to_string()));
    proj_a.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let mut proj_b = Project::new("B".to_string(), Some("Cave Beta".to_string()));
    proj_b.add_section(SectionData {
        shots: vec![test_shot_data(2.0)],
        ..Default::default()
    });

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&proj_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&proj_b).unwrap();
    drop(db_a);
    drop(db_b);

    let output_path = temp_dir.path().join("merged.stalx");
    let state = AppState::new().unwrap();
    let result = state.merge_projects(
        vec![source_a, source_b],
        output_path.clone(),
        "Merged".to_string(),
        false,
    );
    assert!(result.success);

    let db = Database::open_existing(&output_path).unwrap();
    let merged = db.load_project().unwrap();
    let loc = merged.location.unwrap();
    assert!(
        loc.contains("Cave Alpha"),
        "should contain first location: {loc}"
    );
    assert!(
        loc.contains("Cave Beta"),
        "should contain second location: {loc}"
    );
}

#[test]
fn test_merge_projects_no_archive() {
    let temp_dir = TempDir::new().unwrap();

    let mut proj_a = Project::new("A".to_string(), None);
    proj_a.add_section(SectionData {
        shots: vec![test_shot_data(1.0)],
        ..Default::default()
    });
    let mut proj_b = Project::new("B".to_string(), None);
    proj_b.add_section(SectionData {
        shots: vec![test_shot_data(2.0)],
        ..Default::default()
    });

    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&proj_a).unwrap();
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&proj_b).unwrap();
    drop(db_a);
    drop(db_b);

    let output_path = temp_dir.path().join("merged.stalx");
    let state = AppState::new().unwrap();
    let result = state.merge_projects(
        vec![source_a.clone(), source_b.clone()],
        output_path,
        "Merged".to_string(),
        false,
    );
    assert!(result.success);
    // Source files should still exist (not archived)
    assert!(source_a.exists(), "source A should still exist");
    assert!(source_b.exists(), "source B should still exist");
}

#[test]
fn test_merge_projects_preserves_station_merges() {
    let temp_dir = TempDir::new().unwrap();

    let mut proj_a = Project::new("A".to_string(), None);
    let sec_a1 = proj_a
        .add_section(SectionData {
            name: Some("A1".to_string()),
            shots: vec![test_shot_data(1.0), test_shot_data(1.5)],
            ..Default::default()
        })
        .id;
    let sec_a2 = proj_a
        .add_section(SectionData {
            name: Some("A2".to_string()),
            shots: vec![test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    proj_a
        .merge_stations(StationRef::new(sec_a1, 1), StationRef::new(sec_a2, 0))
        .unwrap();

    let source_a = temp_dir.path().join("a.stalx");
    let mut db_a = Database::open(&source_a).unwrap();
    db_a.save_project(&proj_a).unwrap();
    drop(db_a);

    let mut proj_b = Project::new("B".to_string(), None);
    proj_b.add_section(SectionData {
        name: Some("B1".to_string()),
        shots: vec![test_shot_data(3.0)],
        ..Default::default()
    });
    let source_b = temp_dir.path().join("b.stalx");
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&proj_b).unwrap();
    drop(db_b);

    let output_path = temp_dir.path().join("merged.stalx");
    let state = AppState::new().unwrap();
    let result = state.merge_projects(
        vec![source_a, source_b],
        output_path.clone(),
        "Merged".to_string(),
        false,
    );
    assert!(result.success);

    let db = Database::open_existing(&output_path).unwrap();
    let merged = db.load_project().unwrap();
    assert_eq!(
        merged.station_merges.len(),
        1,
        "station merge should be preserved"
    );
    // The merge should reference sections that exist in the merged project
    let merge = &merged.station_merges[0];
    assert!(
        merged
            .sections
            .iter()
            .any(|s| s.id == merge.station_a.section_id),
        "merge station_a should reference an existing section"
    );
    assert!(
        merged
            .sections
            .iter()
            .any(|s| s.id == merge.station_b.section_id),
        "merge station_b should reference an existing section"
    );
    // Both stations should be from the same source project (A), so their
    // sections should be the remapped A1 and A2
    assert_ne!(
        merge.station_a.section_id, merge.station_b.section_id,
        "merge should be between different sections"
    );
}

// ============================================================================
// Coverage gap tests: commands.rs - execute_create_and_merge
// ============================================================================

#[test]
fn test_execute_create_and_merge_success() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    // Create an existing section to merge with
    state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let existing_section_id = state.get_project_state().unwrap().sections[0].id;

    let result = state.execute_create_and_merge(
        Some("Imported".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![test_shot_data(3.0)],
        0,                                       // import station 0
        StationRef::new(existing_section_id, 1), // merge to station 1 of existing
        ClosureStrategy::Batch,
    );
    assert!(
        result.success,
        "create_and_merge failed: {:?}",
        result.error
    );
    assert!(result.section_id.is_some());

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert_eq!(project.station_merges.len(), 1);

    // Verify undo works - should revert both operations atomically
    let undo_result = state.undo();
    assert!(undo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        1,
        "undo should remove imported section"
    );
    assert_eq!(project.station_merges.len(), 0, "undo should remove merge");
}

#[test]
fn test_execute_create_and_merge_rollback_on_merge_failure() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let sections_before = state.get_project_state().unwrap().sections.len();

    // Try merging where the existing station ref points to a nonexistent section
    let result = state.execute_create_and_merge(
        Some("Imported".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![test_shot_data(3.0)],
        0,
        StationRef::new(9999, 0), // nonexistent section
        ClosureStrategy::Batch,
    );
    assert!(
        !result.success,
        "merge with nonexistent section should fail"
    );

    // Project should be rolled back - section count unchanged
    let sections_after = state.get_project_state().unwrap().sections.len();
    assert_eq!(sections_before, sections_after,
        "rollback should restore original section count (before={sections_before}, after={sections_after})");
}

#[test]
fn test_execute_create_and_merge_no_project_open() {
    let state = AppState::new().unwrap();
    let result = state.execute_create_and_merge(
        Some("Test".to_string()),
        None,
        None,
        "#ff0000".to_string(),
        None,
        vec![test_shot_data(3.0)],
        0,
        StationRef::new(1, 0),
        ClosureStrategy::Batch,
    );
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No project open"));
}

// ============================================================================
// Coverage gap tests: commands.rs - execute_import_sections
// ============================================================================

#[test]
fn test_execute_import_sections_origin_placement() {
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sections = vec![
        ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Import1".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![test_shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        },
        ImportSectionConfig {
            reverse: false,
            section_index: 1,
            name: Some("Import2".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![test_shot_data(3.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        },
    ];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);

    // Verify undo reverts all sections atomically
    let undo_result = state.undo();
    assert!(undo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        0,
        "undo should remove all imported sections"
    );
}

/// A closure referencing a section index that never resolves (out of range of
/// the sections in this import) must be counted as skipped on the receipt, not
/// silently dropped: `closures_requested` still reflects what was asked for,
/// `closures_created` reflects what actually landed, and the gap is surfaced as
/// a receipt warning (never a `CommandResult` "SYSTEM" warning - see
/// `ImportReceipt`'s doc comment).
#[test]
fn test_execute_import_sections_unresolvable_closure_is_skipped_on_receipt() {
    use crate::models::{ImportClosure, ImportPlacement, ImportSectionConfig};

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Import1".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }];

    // section_index 99 never appears in `sections`, so `created_ids` never
    // resolves it.
    let closures = vec![ImportClosure {
        from_section_index: 0,
        from_station_index: 0,
        to_section_index: 99,
        to_station_index: 0,
    }];

    let result = state.execute_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let receipt = result
        .import_receipt
        .as_ref()
        .expect("import must attach an import receipt");
    assert_eq!(receipt.closures_requested, 1);
    assert_eq!(receipt.closures_created, 0);
    assert_eq!(
        receipt.warnings.len(),
        1,
        "unresolvable closure must surface exactly one receipt warning: {:?}",
        receipt.warnings
    );
}

/// #27: the Jump/Reference pass has to run INSIDE the import command, before
/// the `after` snapshot. When it ran afterwards, redo restored a snapshot that
/// predated it and every virtual shot silently reverted to the parser's
/// conservative `Reference` seed - dropping the far side of each jump out of
/// gas, penetration and the ruler with no user edit.
#[test]
fn imported_shot_kinds_survive_an_undo_redo_round_trip() {
    use crate::models::{ImportPlacement, ImportSectionConfig, ShotKind};

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Real - virtual - real: the chain rule calls the middle shot a Jump.
    let virtual_shot = ShotData {
        kind: ShotKind::Reference,
        ..test_shot_data(4.0)
    };
    let sections = vec![ImportSectionConfig {
        section_index: 0,
        name: Some("Imported".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0), virtual_shot, test_shot_data(6.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        reverse: false,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let kind_of_middle =
        |state: &AppState| state.get_project_state().unwrap().sections[0].shots[1].kind;
    assert_eq!(
        kind_of_middle(&state),
        ShotKind::Jump,
        "a virtual run bounded by real survey classifies as Jump at import"
    );

    assert!(state.undo().success);
    assert!(state.redo().success);
    assert_eq!(
        kind_of_middle(&state),
        ShotKind::Jump,
        "redo must restore the CLASSIFIED kind, not the parser's Reference seed"
    );
}

/// #27: the per-file override is per *file*. It used to walk the whole project,
/// so a second import silently re-kinded shots the user had hand-set in an
/// earlier session - with no undo entry and nothing in the UI to say so.
#[test]
fn importing_a_second_file_leaves_existing_shot_kinds_alone() {
    use crate::models::{ImportPlacement, ImportSectionConfig, ShotKind};

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let config = |name: &str, shots: Vec<ShotData>| ImportSectionConfig {
        section_index: 0,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots,
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        reverse: false,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };
    let reference_shot = ShotData {
        kind: ShotKind::Reference,
        ..test_shot_data(4.0)
    };

    // First file: a bounded virtual run, which the user then demotes by hand to
    // Reference (the non-swimmable tape tie the chain rule cannot detect).
    let first = state.execute_import_sections(
        vec![config(
            "First",
            vec![
                test_shot_data(5.0),
                reference_shot.clone(),
                test_shot_data(6.0),
            ],
        )],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(first.success, "first import failed: {:?}", first.error);

    let (section_id, shot_id) = {
        let project = state.get_project_state().unwrap();
        (project.sections[0].id, project.sections[0].shots[1].id)
    };
    state.execute(Command::SetShotKind {
        section_id,
        shot_id,
        kind: ShotKind::Reference,
    });
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots[1].kind,
        ShotKind::Reference,
    );

    // Second file, imported with the "everything here is a jump" override.
    let second = state.execute_import_sections(
        vec![config("Second", vec![reference_shot])],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::AllJumps,
    );
    assert!(second.success, "second import failed: {:?}", second.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections[0].shots[1].kind,
        ShotKind::Reference,
        "the hand-set kind in the FIRST file must survive a later import"
    );
    assert_eq!(
        project.sections[1].shots[0].kind,
        ShotKind::Jump,
        "the override still applies to the file being imported"
    );
}

/// #275: overlap-interpolation updates arrive from the DMP import wizard
/// (attacker-controllable input) and must hold the same all_finite invariant
/// as update_shot - a NaN smuggled through an ExistingShotUpdate must fail the
/// whole import before any mutation, not poison an existing shot.
#[test]
fn test_execute_import_sections_rejects_non_finite_shot_update() {
    use crate::models::ExistingShotUpdate;
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;
    use crate::models::ShotUpdates;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let seed = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };
    let result = state.execute_import_sections(
        vec![seed],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "seed import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    let section_id = project.sections[0].id;
    let shot_id = project.sections[0].shots[0].id;
    let clean_length = project.sections[0].shots[0].length;

    let poisoned = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Incoming".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(3.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };
    let result = state.execute_import_sections(
        vec![poisoned],
        Vec::new(),
        None,
        Vec::new(),
        vec![ExistingShotUpdate {
            section_id,
            shot_id,
            updates: ShotUpdates {
                length: Some(f64::NAN),
                ..Default::default()
            },
        }],
        VirtualShotMode::Auto,
    );
    assert!(
        !result.success,
        "a non-finite shot update must fail the import"
    );
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("non-finite"),
        "error should name the invariant: {:?}",
        result.error
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1, "no new section may be created");
    assert_eq!(
        project.sections[0].shots[0].length, clean_length,
        "the existing shot must be untouched"
    );
}

#[test]
fn test_execute_import_sections_from_importing_placement() {
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sections = vec![
        ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("First".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        },
        ImportSectionConfig {
            reverse: false,
            section_index: 1,
            name: Some("Second".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![test_shot_data(4.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: ImportPlacement::FromImporting {
                import_station: 0,
                target_section_index: 0,
                target_station_index: 1,
            },
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        },
    ];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        Some("abc123".to_string()),
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert_eq!(project.station_merges.len(), 1, "sections should be merged");
}

#[test]
fn test_execute_import_sections_from_existing_placement() {
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    // Create existing section to merge with
    state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
    });
    let existing_id = state.get_project_state().unwrap().sections[0].id;

    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Imported".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(4.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::FromExisting {
            import_station: 0,
            existing_station: StationRef::new(existing_id, 2),
        },
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert_eq!(project.station_merges.len(), 1);
}

#[test]
fn test_execute_import_sections_manual_with_offset() {
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Manual".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Manual {
            align_station_index: 0,
            offset: Some(Coordinates {
                x: 10.0,
                y: 20.0,
                z: 0.0,
            }),
        },
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let section_id = project.sections[0].id;
    let offset = project.section_offsets.get(&section_id);
    assert!(
        offset.is_some(),
        "section should have offset from Manual placement"
    );
    let offset = offset.unwrap();
    assert!((offset.0 - 10.0).abs() < 1e-6);
    assert!((offset.1 - 20.0).abs() < 1e-6);
}

#[test]
fn test_execute_import_sections_zero_datum_placement() {
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let gps = GpsCoordinates::new(10.0, 20.0, 5.0).unwrap();
    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("GPS Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::ZeroDatum {
            coordinates: gps,
            station_index: 0,
        },
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.zero_datums.len(), 1);
    assert!((project.zero_datums[0].gps.latitude - 10.0).abs() < 1e-6);
}

/// The import placement loop anchors on a station that is NOT its section's
/// start, and the datum still ends up on local (0, 0).
///
/// The loop calls `AppState::set_zero_datum` directly rather than going through
/// `execute()`, so it never reaches the `renormalize_frame_if_changed` epilogue:
/// whatever `Project::set_zero_datum` leaves behind is what the imported project
/// keeps, and an import whose datum does not sit on the origin hands every later
/// reader of local metres - the map's `gpsOrigin`, the GPS column, the Ariane
/// export's `START` - an origin the survey is not laid out against.
///
/// Non-vacuous by construction: `station_index: 1` is a whole shot away from the
/// section's start, so placing it on the origin requires a real displacement.
/// The sibling test above anchors station 0, which is already at the origin and
/// therefore cannot tell a placement that works from one that does nothing.
#[test]
fn test_execute_import_sections_zero_datum_placement_at_non_origin_station() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let gps = GpsCoordinates::new(10.0, 20.0, 5.0).unwrap();
    let imported_shot = test_shot_data(5.0);
    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("GPS Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![imported_shot.clone()],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::ZeroDatum {
            coordinates: gps,
            station_index: 1,
        },
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.zero_datums.len(), 1);
    let section_id = project.sections[0].id;
    let datum_station = project
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .expect("the datum station has no raw coordinates");
    assert!(
        datum_station.x.abs() < 1e-9 && datum_station.y.abs() < 1e-9,
        "the import's first datum has to land its station on local (0, 0), got ({}, {})",
        datum_station.x,
        datum_station.y
    );

    // Read off the result rather than asserted in prose: station 0 is a whole
    // shot away from the origin, which is exactly the displacement the placement
    // had to apply to get station 1 onto it.
    let station_zero = project
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .expect("station 0 has no raw coordinates");
    assert!(
        (station_zero.x.hypot(station_zero.y) - imported_shot.horizontal_length()).abs() < 1e-9,
        "the section had to move by the whole shot to put station 1 on the origin; station 0 is \
         at ({}, {})",
        station_zero.x,
        station_zero.y
    );
}

#[test]
fn test_execute_import_sections_rollback_on_failure() {
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // First section succeeds, second fails (merge to nonexistent existing section)
    let sections = vec![
        ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("GoodSection".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![test_shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        },
        ImportSectionConfig {
            reverse: false,
            section_index: 1,
            name: Some("BadSection".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![test_shot_data(3.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: ImportPlacement::FromExisting {
                import_station: 0,
                existing_station: StationRef::new(9999, 0), // nonexistent
            },
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        },
    ];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(
        !result.success,
        "should fail when merge target doesn't exist"
    );

    // Project should be rolled back - no sections added
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        0,
        "rollback should remove all sections from failed import"
    );
}

#[test]
fn test_execute_import_sections_empty_is_noop() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let result = state.execute_import_sections(
        vec![],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success);
}

#[test]
fn test_execute_import_sections_no_project() {
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Test".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(5.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No project open"));
}

#[test]
fn test_execute_import_sections_manual_snap_placement() {
    use crate::models::ImportPlacement;
    use crate::models::ImportSectionConfig;

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let existing_id = state.get_project_state().unwrap().sections[0].id;

    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Snapped".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![test_shot_data(4.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::ManualSnap {
            align_station_index: 0,
            snapped_to_station: StationRef::new(existing_id, 1),
        },
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }];

    let result = state.execute_import_sections(
        sections,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(
        result.success,
        "import with ManualSnap failed: {:?}",
        result.error
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(project.station_merges.len(), 1);
}

// ============================================================================
// Coverage gap tests: commands.rs - batch edge cases
// ============================================================================

#[test]
fn test_batch_empty_is_noop() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let result = state.execute(Command::Batch { commands: vec![] });
    assert!(result.success);
}

#[test]
fn test_batch_disallowed_command_save_project() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let result = state.execute(Command::Batch {
        commands: vec![Command::SaveProject {
            path: Some(PathBuf::from("/tmp/test.stalx")),
        }],
    });
    assert!(!result.success);
    assert!(result
        .error
        .unwrap_or_default()
        .contains("not allowed in batch"));
}

#[test]
fn test_batch_partial_failure_rolls_back() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sections_before = state.get_project_state().unwrap().sections.len();

    // First command succeeds (create section), second fails (merge nonexistent)
    let result = state.execute(Command::Batch {
        commands: vec![
            Command::CreateSection {
                name: Some("NewSec".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![test_shot_data(5.0)],
            },
            Command::MergeStations {
                station_a: StationRef::new(9999, 0),
                station_b: StationRef::new(9998, 0),
            },
        ],
    });
    assert!(!result.success);

    // Rollback should restore original state
    let sections_after = state.get_project_state().unwrap().sections.len();
    assert_eq!(
        sections_before, sections_after,
        "batch rollback should restore section count"
    );
}

/// Withdrawing an abandoned import (#394) removes N sections as ONE undo entry.
///
/// The DMP wizard imports eagerly - the zero-datum anchor lands on mount, each
/// reachable segment as it is placed - so "Cancel, and remove what was already
/// imported" names a whole list. Deleting them one command at a time would leave
/// the user N undo steps to walk back through, so `DeleteSection` is batch-allowed
/// and the frontend sends one `execute_batch`. The undo has to bring back ALL of
/// them, not the last one.
#[test]
fn batched_section_deletes_are_one_undo_entry() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Withdraw".to_string(),
        location: None,
    });

    let make = |name: &str| {
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![test_shot_data(5.0)],
            })
            .section_id
            .unwrap()
    };
    let a = make("A");
    let b = make("B");
    let c = make("C");
    assert_eq!(state.get_project_state().unwrap().sections.len(), 3);

    let removed = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteSection { section_id: a },
            Command::DeleteSection { section_id: b },
        ],
    });
    assert!(removed.success, "batched delete: {:?}", removed.error);
    let after: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(after, vec![c], "both named sections should be gone");

    // ONE undo entry, not two: a single undo restores both.
    assert!(state.undo().success);
    let restored: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(
        restored,
        vec![a, b, c],
        "one undo must restore every section the batch removed"
    );
}

/// The parity argument that lets `DeleteSection` into a batch, pinned.
///
/// `Command::reversed_section_ids` PARITY-REDUCES a batch's reverse log, and the
/// doc comment justifies that by saying a batch cannot change a surviving
/// section's shot count. Admitting `DeleteSection` puts one shape in reach that
/// the comment now has to answer for: reverse a section and then delete it in
/// the same batch. The claim is that `settle_reversed_sections` drops a section
/// that is gone (it resolves each id through `get_section`), so the reverse is
/// reported to nobody and there is nothing to fix up on a section that no longer
/// exists. This is that claim as a test rather than as prose.
#[test]
fn a_batch_may_reverse_a_section_and_then_delete_it() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Mixed".to_string(),
        location: None,
    });

    let doomed = state
        .execute(Command::CreateSection {
            name: Some("Doomed".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
        })
        .section_id
        .unwrap();
    let keeper = state
        .execute(Command::CreateSection {
            name: Some("Keeper".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(9.0)],
        })
        .section_id
        .unwrap();

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseSection {
                section_id: doomed,
                swap_left_right: false,
            },
            Command::DeleteSection { section_id: doomed },
        ],
    });

    assert!(result.success, "mixed batch: {:?}", result.error);
    // The reverse is reported to NOBODY: the section it names is gone, so there
    // is no shot count to divide by and nothing for the frontend to mirror.
    assert!(
        result
            .reversed_sections
            .as_ref()
            .is_none_or(|entries| entries.iter().all(|e| e.section_id != doomed)),
        "a section deleted in the same batch must not be reported as reversed"
    );
    let after: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(after, vec![keeper]);

    // And it all comes back together, still one undo entry.
    assert!(state.undo().success);
    let restored: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(restored, vec![doomed, keeper]);
}

/// The other order is a clean REFUSAL, not a corruption: the section is already
/// gone when the reverse runs, so the sub-command fails and `execute_batch`
/// rolls the whole thing back. Pinned because "the batch rejects it" is half of
/// why the parity argument holds - a batch cannot reverse a section twice around
/// a delete if the delete makes every later reference fail.
#[test]
fn a_batch_that_reverses_a_section_after_deleting_it_rolls_back() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Mixed".to_string(),
        location: None,
    });

    let section_id = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteSection { section_id },
            Command::ReverseSection {
                section_id,
                swap_left_right: false,
            },
        ],
    });

    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("not found"));
    let after: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(after, vec![section_id], "the delete must have rolled back");
}

/// A batch that adds two zero datums places the SECOND against the origin the
/// FIRST one established, not against wherever the survey happened to sit when
/// the batch started.
///
/// `execute_batch` runs its sub-commands through `execute_without_undo` and
/// applies `renormalize_frame_if_changed` ONCE, after the last of them - so the
/// establishing re-frame has to happen inside `Project::set_zero_datum` itself.
/// Left to the batch epilogue, sub-command two reads "the frame datum's station
/// sits at (0, 0)" while it does not yet, places B against that origin, and the
/// epilogue then translates the already-misplaced B along with everything else:
/// B lands off by the whole establishing delta and the two datums no longer sit
/// at their true GPS separation.
///
/// Non-vacuous by construction. A's datum station is 10 m north of its section's
/// start, so establishing the frame is a real translation rather than nothing;
/// the two GPS fixes are hundreds of metres apart, so the expected offset is
/// decades above `Project::OFFSET_DUST_M`; and the sections are separate
/// components, which is required rather than incidental - `set_zero_datum`
/// refuses a second datum inside one merge group.
#[test]
fn test_batch_second_zero_datum_lands_on_the_origin_the_first_established() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let north = |length: f64| ShotData {
        azimuth_in: 0.0,
        azimuth_out: 0.0,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    };
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![north(10.0)],
        })
        .section_id
        .unwrap();
    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![north(4.0)],
        })
        .section_id
        .unwrap();

    let gps_a = GpsCoordinates::new(20.4, -87.3, 0.0).unwrap();
    let gps_b = GpsCoordinates::new(20.4, -87.295, 0.0).unwrap();
    let expected_b = crate::models::gps_to_local(&gps_b, &gps_a);
    assert!(
        expected_b.0.hypot(expected_b.1) > 100.0,
        "setup: the two datums have to sit far enough apart for the expected offset to be \
         unmistakable, got {expected_b:?}"
    );
    let datum_a_before = state
        .get_project_state()
        .unwrap()
        .compute_station_raw_coords(StationRef::new(sec_a, 1))
        .expect("A's datum station has no raw coordinates");
    assert!(
        (datum_a_before.y - 10.0).abs() < 1e-9,
        "setup: A's datum station has to start away from the origin, got {datum_a_before:?}"
    );

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::SetZeroDatum {
                station_ref: StationRef::new(sec_a, 1),
                coordinates: gps_a,
            },
            Command::SetZeroDatum {
                station_ref: StationRef::new(sec_b, 0),
                coordinates: gps_b,
            },
        ],
    });
    assert!(result.success, "batch failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let a1 = project
        .compute_station_raw_coords(StationRef::new(sec_a, 1))
        .expect("A's datum station has no raw coordinates");
    assert!(
        a1.x.abs() < 1e-6 && a1.y.abs() < 1e-6,
        "the batch's FIRST datum establishes the frame, so its station has to sit at local \
         (0, 0), got ({}, {})",
        a1.x,
        a1.y
    );
    let b0 = project
        .compute_station_raw_coords(StationRef::new(sec_b, 0))
        .expect("B's datum station has no raw coordinates");
    assert!(
        (b0.x - expected_b.0).abs() < 1e-6 && (b0.y - expected_b.1).abs() < 1e-6,
        "the batch's SECOND datum has to sit at its GPS offset from the first, {expected_b:?}, \
         got ({}, {})",
        b0.x,
        b0.y
    );
}

// ============================================================================
// Coverage gap tests: commands.rs - delete_shot with shift-blocked
// ============================================================================

#[test]
fn test_create_section_rejects_negative_length_shot() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let result = state.execute(Command::CreateSection {
        name: Some("Bad".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            length: -5.0,
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("negative length"));
}

#[test]
fn test_create_shot_rejects_zero_length() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let section_id = {
        let result = state.execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        });
        result.section_id.unwrap()
    };

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: ShotData {
            length: 0.0,
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("zero length"));
}

#[test]
fn test_update_shot_rejects_zero_resulting_length() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let section_id = {
        let result = state.execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        });
        result.section_id.unwrap()
    };
    let shot_id = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots[0]
        .id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(0.0),
            ..Default::default()
        },
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("zero length"));
}

/// A negative length is refused the same way, and refusing it is still keyed on
/// the update SETTING one.
#[test]
fn test_update_shot_rejects_an_explicitly_negative_length() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let section_id = state
        .execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let shot_id = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots[0]
        .id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(-1.0),
            ..Default::default()
        },
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("negative length"));
}

/// Plant a zero-length shot at index 1 of a fresh two-shot section and return
/// `(section_id, [normal_shot_id, zero_length_shot_id])`.
///
/// Written onto the model directly because no command path can produce it -
/// `create_shot` and `update_shot` both refuse the value. It is a state that
/// LOADS, though: `Validator::validate_shot` reports ZERO_LENGTH as a validation
/// error on a legacy or collaborator file, it does not refuse the file.
fn section_holding_a_zero_length_shot(state: &AppState) -> (SectionId, [ShotId; 2]) {
    let section_id = state
        .execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(4.0)],
        })
        .section_id
        .unwrap();
    let mut guard = state.project.lock().unwrap();
    let project = guard.as_mut().unwrap();
    let section = project.get_section_mut(section_id).unwrap();
    section.shots[1].length = 0.0;
    (section_id, [section.shots[0].id, section.shots[1].id])
}

/// The stored length is not the update's length: an edit that does not touch it
/// must not be judged by it.
///
/// Falling back to the stored value made every edit on such a shot fail, which
/// locked the invalid length out of the very edits that could correct it (#413).
#[test]
fn test_update_shot_allows_a_kind_only_edit_on_a_zero_length_shot() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let (section_id, [_, zero_length]) = section_holding_a_zero_length_shot(&state);

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id: zero_length,
        updates: ShotUpdates {
            kind: Some(ShotKind::Jump),
            ..Default::default()
        },
    });
    assert!(
        result.success,
        "a kind-only edit must not be refused for a length it does not set: {:?}",
        result.error
    );
}

/// ...and the reason that matters most: `SetShotKind` records `PreviousValues`,
/// whose undo re-enters `update_shot` with only `kind` set. While the guard
/// judged that by the stored length, the undo failed forever - `undo_inner`'s
/// epilogue pushes a failed entry straight back, so the step never retires and
/// every older entry underneath it is sealed off.
#[test]
fn test_undo_of_a_kind_change_on_a_zero_length_shot_leaves_the_history_usable() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let (section_id, [normal, zero_length]) = section_holding_a_zero_length_shot(&state);

    // The older entry, so a wedge is visible as the step BENEATH the failed one.
    assert!(
        state
            .execute(Command::SetShotKind {
                section_id,
                shot_id: normal,
                kind: ShotKind::Jump,
            })
            .success
    );
    assert!(
        state
            .execute(Command::SetShotKind {
                section_id,
                shot_id: zero_length,
                kind: ShotKind::Reference,
            })
            .success
    );

    let kind_of = |shot_id: ShotId| {
        state
            .get_project_state()
            .unwrap()
            .sections
            .iter()
            .find(|s| s.id == section_id)
            .unwrap()
            .shots
            .iter()
            .find(|s| s.id == shot_id)
            .unwrap()
            .kind
    };

    let undone = state.undo();
    assert!(
        undone.success,
        "undo of a kind change on a zero-length shot failed: {:?}",
        undone.error
    );
    assert_eq!(kind_of(zero_length), ShotKind::Normal);

    let older = state.undo();
    assert!(
        older.success,
        "the entry beneath it must still be undoable: {:?}",
        older.error
    );
    assert_eq!(kind_of(normal), ShotKind::Normal);

    // ...all the way down: the `CreateSection` under both of them drains too.
    assert!(state.undo().success, "the whole history must drain");
    assert!(!state.can_undo());
}

// ============================================================================
// Coverage gap tests: commands.rs - auto-save
// ============================================================================

// test_auto_save_triggers_on_modifying_command was removed:
// per-command auto-save was removed in favor of frontend debounced auto-save

// ============================================================================
// Coverage gap tests: commands.rs - move_station edge cases
// ============================================================================

#[test]
fn test_move_station_rejects_zero_length_result() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 1.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });
    let section_id = result.section_id.unwrap();

    // Move station 1 backwards by exactly the shot length - should create zero-length shot
    let result = state.execute(Command::MoveStation {
        station_ref: StationRef::new(section_id, 1),
        offset: Coordinates {
            x: 0.0,
            y: -1.0,
            z: 0.0,
        },
    });
    assert!(
        !result.success,
        "should reject move that creates zero-length shot"
    );
    assert!(result.error.unwrap_or_default().contains("zero-length"));
}

#[test]
fn test_move_station_last_station() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
    });
    let section_id = result.section_id.unwrap();

    // Move last station (station index 2 for 2 shots)
    let result = state.execute(Command::MoveStation {
        station_ref: StationRef::new(section_id, 2),
        offset: Coordinates {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
    });
    assert!(
        result.success,
        "move last station failed: {:?}",
        result.error
    );

    // Verify only the previous shot was modified (not a next shot)
    let project = state.get_project_state().unwrap();
    let section = project
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap();
    assert_eq!(section.shots.len(), 2);
}

#[test]
fn test_move_station_invalid_index() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let section_id = result.section_id.unwrap();

    let result = state.execute(Command::MoveStation {
        station_ref: StationRef::new(section_id, 999),
        offset: Coordinates {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
    });
    assert!(!result.success);
    assert!(result
        .error
        .unwrap_or_default()
        .contains("Invalid station index"));
}

// ============================================================================
// Delete section cascade tests
// ============================================================================

#[test]
fn test_delete_section_cascades_merges() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create two sections
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    // Merge them
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_result.success);
    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 1);

    // Delete section A - should cascade-remove the merge
    let delete_result = state.execute(Command::DeleteSection { section_id: sec_a });
    assert!(delete_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(
        project.station_merges.len(),
        0,
        "merge should be cascaded on delete"
    );
}

#[test]
fn test_delete_section_cascades_closures_and_offsets() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0)],
        })
        .section_id
        .unwrap();

    // Set a zero datum on section A. Station 1, NOT station 0: the first datum
    // pins its station to the origin, and station 0 is already there, so a datum
    // on it displaces nothing - and `section_offsets` now means "sections that
    // are actually displaced" (#413 finding 2: an exactly-(0, 0) entry is
    // dropped rather than stored). Anchoring station 1 gives A a REAL offset, so
    // the delete-cascade assertion below has something to cascade. Note that the
    // add displaces EVERY section, not just A's merge group: establishing the
    // frame re-frames the whole survey rigidly (`Project::set_zero_datum` ->
    // `renormalize_frame`), so B picks up the same translation. Only A's offset
    // is asserted here because only A's is the one the delete has to cascade.
    let datum_result = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 1),
        coordinates: GpsCoordinates::new(45.0, 10.0, 0.0).unwrap(),
    });
    assert!(datum_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.zero_datums.len(), 1);
    // set_zero_datum displaced A, so it has an offset (stronger than the
    // "the map is non-empty" this used to assert).
    assert!(
        project.section_offsets.contains_key(&sec_a),
        "set_zero_datum should give the displaced section an offset"
    );

    // Delete section A - should cascade-remove the zero datum and section offsets
    let delete_result = state.execute(Command::DeleteSection { section_id: sec_a });
    assert!(delete_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].id, sec_b);
    assert_eq!(
        project.zero_datums.len(),
        0,
        "zero datum should be cascaded"
    );
    assert!(
        !project.section_offsets.contains_key(&sec_a),
        "section A offset should be cascaded"
    );
}

#[test]
fn test_delete_section_undo_restores_merges_and_datums() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    // Merge + set datum
    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 0),
        coordinates: GpsCoordinates::new(45.0, 10.0, 0.0).unwrap(),
    });

    // Delete section A
    state.execute(Command::DeleteSection { section_id: sec_a });
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.station_merges.len(), 0);
    assert_eq!(project.zero_datums.len(), 0);

    // Undo delete - should restore section A, merge, and datum
    let undo = state.undo();
    assert!(undo.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        2,
        "undo should restore deleted section"
    );
    assert_eq!(project.station_merges.len(), 1, "undo should restore merge");
    assert_eq!(
        project.zero_datums.len(),
        1,
        "undo should restore zero datum"
    );
}

#[test]
fn test_unmerge_removes_only_specific_merge() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0)],
        })
        .section_id
        .unwrap();

    // Merge A↔B and B↔C
    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_b, 1),
        station_b: StationRef::new(sec_c, 0),
    });

    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 2);

    // Unmerge at station A:1 (which is part of the A↔B merge)
    let result = state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(sec_a, 1),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.station_merges.len(),
        1,
        "only A↔B should be removed"
    );
    assert_eq!(project.station_merges[0].station_a.section_id, sec_b);
    assert_eq!(project.station_merges[0].station_b.section_id, sec_c);
}

#[test]
fn test_unmerge_undo_restores_merge() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });

    // Unmerge, then undo
    state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(sec_a, 1),
    });
    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 0);

    let undo = state.undo();
    assert!(undo.success);
    assert_eq!(
        state.get_project_state().unwrap().station_merges.len(),
        1,
        "undo should restore unmerged pair"
    );
}

/// Integration test: import kb5.dmp sections with overlap treatment and verify
/// the actual shot values in the project state after import.
///
/// Flow:
/// 1. Import section 002 at origin (becomes the "existing" section)
/// 2. Import section 001 (105 shots, overlap removed) with existing_shot_updates
///    that interpolate 002's overlap shots - all in one atomic import call
/// 3. Read back from project state and assert actual interpolated values
#[test]
#[ignore = "fixture-gated: needs e2e/fixtures/kb5.dmp, which the repo does not carry"]
fn kb5_import_overlap_interpolation_integration() {
    use crate::models::ExistingShotUpdate;
    use crate::parsers::dmp::{parse_dmp, DmpShotType};

    let dmp_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/kb5.dmp");
    let dmp = parse_dmp(&dmp_path).expect("should parse kb5.dmp");

    // Extract standard shots as ShotData for sections 001 (idx 0) and 002 (idx 1)
    let shots_001: Vec<ShotData> = dmp.sections[0]
        .shots
        .iter()
        .filter(|s| s.shot_type == DmpShotType::Standard)
        .map(|s| s.to_shot_data())
        .collect();
    let shots_002: Vec<ShotData> = dmp.sections[1]
        .shots
        .iter()
        .filter(|s| s.shot_type == DmpShotType::Standard)
        .map(|s| s.to_shot_data())
        .collect();
    assert_eq!(shots_001.len(), 107);
    assert_eq!(shots_002.len(), 364);

    // ── Step 1: Create project and import section 002 at origin ──
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "KB5 Overlap Test".to_string(),
        location: None,
    });

    let result_002 = state.execute_import_sections(
        vec![ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("002".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots: shots_002.clone(),
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        }],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(
        result_002.success,
        "import 002 failed: {:?}",
        result_002.error
    );

    let project = state.get_project_state().unwrap();
    let section_002_id = project.sections[0].id;
    assert_eq!(
        project.sections[0].shots.len(),
        364,
        "002 should have 364 shots"
    );

    // ── Step 2: Import section 001 with overlap shots removed + existing shot updates ──
    // 001→002 overlap: Back (impStn=107), Reverse, matched=2, exStn=182.
    // Target shot indices (Back+Reverse): (182-2)+k = 180, 181.
    // Import shots 105, 106 are deleted; 002 shots 180, 181 get interpolated values.

    fn circular_mean_deg(a: f64, b: f64) -> f64 {
        let (a_r, b_r) = (a.to_radians(), b.to_radians());
        let mean =
            f64::atan2((a_r.sin() + b_r.sin()) / 2.0, (a_r.cos() + b_r.cos()) / 2.0).to_degrees();
        if mean < 0.0 {
            mean + 360.0
        } else {
            mean
        }
    }

    fn avg_opt(a: Option<f64>, b: Option<f64>) -> Option<Option<f64>> {
        match (a, b) {
            (None, None) => None,
            (Some(v), None) | (None, Some(v)) => Some(Some(v)),
            (Some(a), Some(b)) => Some(Some((a + b) / 2.0)),
        }
    }

    fn interp_update(
        section_id: SectionId,
        shot_id: ShotId,
        imp: &ShotData,
        ext: &ShotData,
    ) -> ExistingShotUpdate {
        ExistingShotUpdate {
            section_id,
            shot_id,
            updates: ShotUpdates {
                azimuth_in: Some(circular_mean_deg(imp.azimuth_in, ext.azimuth_in)),
                azimuth_out: Some(circular_mean_deg(imp.azimuth_out, ext.azimuth_out)),
                length: Some((imp.length + ext.length) / 2.0),
                depth_in: Some((imp.depth_in + ext.depth_in) / 2.0),
                depth_out: Some((imp.depth_out + ext.depth_out) / 2.0),
                pitch_in: avg_opt(imp.pitch_in, ext.pitch_in),
                pitch_out: avg_opt(imp.pitch_out, ext.pitch_out),
                left: avg_opt(imp.left, ext.left),
                right: avg_opt(imp.right, ext.right),
                up: avg_opt(imp.up, ext.up),
                down: avg_opt(imp.down, ext.down),
                ..Default::default()
            },
        }
    }

    let shot_180_id = project.sections[0].shots[180].id;
    let shot_181_id = project.sections[0].shots[181].id;

    let existing_updates = vec![
        interp_update(
            section_002_id,
            shot_180_id,
            &shots_001[105],
            &shots_002[180],
        ),
        interp_update(
            section_002_id,
            shot_181_id,
            &shots_001[106],
            &shots_002[181],
        ),
    ];

    // Remove overlap shots 105, 106 from 001
    let mut shots_001_treated: Vec<ShotData> = shots_001.clone();
    shots_001_treated.remove(106); // remove higher index first
    shots_001_treated.remove(105);
    assert_eq!(
        shots_001_treated.len(),
        105,
        "001 should have 105 shots after overlap removal"
    );

    // Import 001 merged to 002 at station 182, with existing shot updates
    let result_001 = state.execute_import_sections(
        vec![ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("001".to_string()),
            surveyor: None,
            date: None,
            color: "#ff6b6b".to_string(),
            direction: None,
            quality: 1.0,
            shots: shots_001_treated,
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: ImportPlacement::FromExisting {
                import_station: 105,
                existing_station: StationRef::new(section_002_id, 182),
            },
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        }],
        Vec::new(),
        None,
        Vec::new(),
        existing_updates,
        VirtualShotMode::Auto,
    );
    assert!(
        result_001.success,
        "import 001 failed: {:?}",
        result_001.error
    );

    // ── Step 3: Read back from project state and verify ──
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2, "should have 2 sections");

    let sec_002 = project
        .sections
        .iter()
        .find(|s| s.id == section_002_id)
        .expect("section 002 should exist");
    let sec_001 = project
        .sections
        .iter()
        .find(|s| s.id != section_002_id)
        .expect("section 001 should exist");

    // Section 001: 105 shots (overlap shots removed)
    assert_eq!(
        sec_001.shots.len(),
        105,
        "001 should have 105 shots after import"
    );

    // Section 002: 364 shots, with shots 180-181 now having interpolated values
    assert_eq!(sec_002.shots.len(), 364, "002 should still have 364 shots");

    let actual_180 = &sec_002.shots[180];
    let actual_181 = &sec_002.shots[181];

    // Verify shot 180 has interpolated values (not the original 6.52m/329.7°)
    let eps = 0.01;
    assert!(
        (actual_180.length - 5.665).abs() < eps,
        "002 shot 180 length after import: expected ~5.665, got {}",
        actual_180.length
    );
    assert!(
        (actual_180.azimuth_in - 322.4).abs() < 0.5,
        "002 shot 180 azimuth_in after import: expected ~322.4°, got {}°",
        actual_180.azimuth_in
    );

    // Verify shot 181 has interpolated values (not the original 12.71m/355.8°)
    assert!(
        (actual_181.length - 12.375).abs() < eps,
        "002 shot 181 length after import: expected ~12.375, got {}",
        actual_181.length
    );
    assert!(
        (actual_181.azimuth_in - 349.75).abs() < 0.5,
        "002 shot 181 azimuth_in after import: expected ~349.75°, got {}°",
        actual_181.azimuth_in
    );

    println!(
        "002 shot 180: length={}, azi_in={}",
        actual_180.length, actual_180.azimuth_in
    );
    println!(
        "002 shot 181: length={}, azi_in={}",
        actual_181.length, actual_181.azimuth_in
    );
}

// === Merge chain tests ===

/// Test 2a: Transitive offset correctness in a 3-section chain.
///
/// When neither section has a datum, the merge logic makes the *second* argument
/// the anchor (B is the anchor, A moves). Station positions are:
///
///   A: 1 shot east (azimuth 90°), length 10m → A.0 at its offset, A.1 = A.0 + (10, 0)
///   B: 1 shot north (azimuth 0°), length 5m  → B.0=(0,0), B.1=(0,5)
///   C: 1 shot east (azimuth 90°), length 3m  → initially C.0=(0,0), C.1=(3,0)
///
///   Merge A.1 ↔ B.0: B is anchor, A moves so A.1 = B.0 = (0,0) → A.0=(-10,0)
///   Merge B.1 ↔ C.0: B is part of existing group (B is anchor), C moves so C.0 = B.1 = (0,5)
///                     → C.1 = (3, 5)
///
/// The key invariant is that both merge points are colocated and the relative
/// geometry within each section is preserved exactly.
#[test]
fn test_merge_chain_transitive_offsets_are_correct() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Chain".to_string(),
        location: None,
    });

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![directional_shot_data(90.0, 10.0)],
        })
        .section_id
        .unwrap();

    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![directional_shot_data(0.0, 5.0)],
        })
        .section_id
        .unwrap();

    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![directional_shot_data(90.0, 3.0)],
        })
        .section_id
        .unwrap();

    // Merge A.1 ↔ B.0
    let r1 = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(r1.success, "First merge should succeed");

    // Merge B.1 ↔ C.0
    let r2 = state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });
    assert!(r2.success, "Second merge should succeed");

    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    let coord = |sec: SectionId, idx: u32| -> Coordinates {
        stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(sec, idx))
            .unwrap_or_else(|| panic!("station {}:{} not found", sec, idx))
            .coordinates
    };

    let eps = 1e-4;

    // Invariant 1: merge points must be exactly colocated
    assert!(
        (coord(a, 1).x - coord(b, 0).x).abs() < eps,
        "A.1 and B.0 must be colocated (x): A.1={} B.0={}",
        coord(a, 1).x,
        coord(b, 0).x
    );
    assert!(
        (coord(a, 1).y - coord(b, 0).y).abs() < eps,
        "A.1 and B.0 must be colocated (y)"
    );
    assert!(
        (coord(b, 1).x - coord(c, 0).x).abs() < eps,
        "B.1 and C.0 must be colocated (x): B.1={} C.0={}",
        coord(b, 1).x,
        coord(c, 0).x
    );
    assert!(
        (coord(b, 1).y - coord(c, 0).y).abs() < eps,
        "B.1 and C.0 must be colocated (y)"
    );

    // Invariant 2: relative geometry within each section is preserved.
    // A: one shot east 10m, so A.1 = A.0 + (10, 0)
    assert!(
        (coord(a, 1).x - coord(a, 0).x - 10.0).abs() < eps,
        "A internal geometry: A.1.x - A.0.x should be 10, got {}",
        coord(a, 1).x - coord(a, 0).x
    );
    assert!(
        (coord(a, 1).y - coord(a, 0).y).abs() < eps,
        "A internal geometry: A.1.y - A.0.y should be 0"
    );

    // B: one shot north 5m, so B.1 = B.0 + (0, 5)
    assert!(
        (coord(b, 1).x - coord(b, 0).x).abs() < eps,
        "B internal geometry: B.1.x - B.0.x should be 0"
    );
    assert!(
        (coord(b, 1).y - coord(b, 0).y - 5.0).abs() < eps,
        "B internal geometry: B.1.y - B.0.y should be 5, got {}",
        coord(b, 1).y - coord(b, 0).y
    );

    // C: one shot east 3m, so C.1 = C.0 + (3, 0)
    assert!(
        (coord(c, 1).x - coord(c, 0).x - 3.0).abs() < eps,
        "C internal geometry: C.1.x - C.0.x should be 3"
    );
    assert!(
        (coord(c, 1).y - coord(c, 0).y).abs() < eps,
        "C internal geometry: C.1.y - C.0.y should be 0"
    );

    // Invariant 3: transitive positioning - C.0 must equal B.1, which equals B.0 + (0,5).
    // And B.0 equals A.1, which equals A.0 + (10, 0).
    // So the whole chain is geometrically consistent.
    let a1 = coord(a, 1);
    let b0 = coord(b, 0);
    let b1 = coord(b, 1);
    let c0 = coord(c, 0);
    assert!(
        (b1.x - b0.x).abs() < eps && (b1.y - b0.y - 5.0).abs() < eps,
        "B.1 should be 5m north of B.0"
    );
    // C.0 == B.1, and B.1 == A.1 + (0, 5) (since B is 5m north of its start and B.0 == A.1)
    assert!(
        (c0.x - a1.x).abs() < eps,
        "C.0.x should equal A.1.x (transitive via B): C.0.x={} A.1.x={}",
        c0.x,
        a1.x
    );
    assert!(
        (c0.y - a1.y - 5.0).abs() < eps,
        "C.0.y should be A.1.y + 5 (transitive via B): C.0.y={} A.1.y={}",
        c0.y,
        a1.y
    );
}

/// Test 2b: Inserting a shot at position 0 of the middle section B in a chain
/// A↔B↔C correctly shifts B's station merge indices.
///
/// After the insert the merge edge that pointed to B.0 (from A.1) should now
/// point to B.1, because all existing stations shifted up by one.
#[test]
fn test_insert_before_pos0_shifts_merge_refs_in_chain() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "ShiftChain".to_string(),
        location: None,
    });

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![directional_shot_data(90.0, 10.0)],
        })
        .section_id
        .unwrap();

    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![directional_shot_data(0.0, 5.0)],
        })
        .section_id
        .unwrap();

    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![directional_shot_data(90.0, 3.0)],
        })
        .section_id
        .unwrap();

    // Merge A.1 ↔ B.0  and  B.1 ↔ C.0
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });

    // Get first shot id of section B so we can insert before it
    let first_b_shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.sections.iter().find(|s| s.id == b).unwrap().shots[0].id
    };

    // Insert a new shot at the very start of section B (before_shot_id = first shot)
    let insert_result = state.execute(Command::CreateShot {
        section_id: b,
        shot: directional_shot_data(270.0, 2.0),
        after_shot_id: None,
        before_shot_id: Some(first_b_shot_id),
    });
    assert!(
        insert_result.success,
        "Insert before pos 0 in B should succeed"
    );

    let proj = state.get_project_state().unwrap();

    // After inserting at pos 0 in B, all original B station indices shift by +1.
    // The merge A.1 ↔ B.0 should now be A.1 ↔ B.1
    // The merge B.1 ↔ C.0 should now be B.2 ↔ C.0
    let merges = &proj.station_merges;
    assert_eq!(merges.len(), 2, "Should still have two merge edges");

    let has_a1_b1 = merges.iter().any(|m| {
        (proj.key_is_ref(&m.station_a, StationRef::new(a, 1))
            && proj.key_is_ref(&m.station_b, StationRef::new(b, 1)))
            || (proj.key_is_ref(&m.station_a, StationRef::new(b, 1))
                && proj.key_is_ref(&m.station_b, StationRef::new(a, 1)))
    });
    assert!(
        has_a1_b1,
        "After insert-before-0 in B, merge should be A.1↔B.1, got: {:?}",
        merges
    );

    let has_b2_c0 = merges.iter().any(|m| {
        (proj.key_is_ref(&m.station_a, StationRef::new(b, 2))
            && proj.key_is_ref(&m.station_b, StationRef::new(c, 0)))
            || (proj.key_is_ref(&m.station_a, StationRef::new(c, 0))
                && proj.key_is_ref(&m.station_b, StationRef::new(b, 2)))
    });
    assert!(
        has_b2_c0,
        "After insert-before-0 in B, merge should be B.2↔C.0, got: {:?}",
        merges
    );

    // The merge points must still be colocated after the shift
    let stations = proj.compute_station_coordinates();
    let coord = |sec: SectionId, idx: u32| -> Coordinates {
        stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(sec, idx))
            .unwrap_or_else(|| panic!("station {}:{} not found", sec, idx))
            .coordinates
    };
    let eps = 1e-4;
    assert!(
        (coord(a, 1).x - coord(b, 1).x).abs() < eps,
        "A.1 and B.1 must be colocated after insert"
    );
    assert!(
        (coord(a, 1).y - coord(b, 1).y).abs() < eps,
        "A.1 and B.1 must be colocated after insert"
    );
    assert!(
        (coord(b, 2).x - coord(c, 0).x).abs() < eps,
        "B.2 and C.0 must be colocated after insert"
    );
    assert!(
        (coord(b, 2).y - coord(c, 0).y).abs() < eps,
        "B.2 and C.0 must be colocated after insert"
    );
}

/// Test 2c: Deleting the middle section B from a chain A↔B↔C removes both
/// merge edges (A↔B and B↔C), leaving A and C isolated.
#[test]
fn test_delete_middle_section_removes_both_merge_edges() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "DeleteMiddle".to_string(),
        location: None,
    });

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0)],
        })
        .section_id
        .unwrap();

    // Build chain A↔B↔C
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });
    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 2);

    // Delete the middle section B
    let delete_result = state.execute(Command::DeleteSection { section_id: b });
    assert!(
        delete_result.success,
        "Deleting middle section should succeed"
    );

    let proj = state.get_project_state().unwrap();

    // Both merge edges should be gone
    assert_eq!(
        proj.station_merges.len(),
        0,
        "Both merge edges should be removed when middle section is deleted"
    );

    // A and C should still exist
    assert_eq!(proj.sections.len(), 2);
    let section_ids: Vec<SectionId> = proj.sections.iter().map(|s| s.id).collect();
    assert!(section_ids.contains(&a), "Section A should still exist");
    assert!(section_ids.contains(&c), "Section C should still exist");

    // A and C should be isolated (not connected to each other)
    let connected_a = proj.get_connected_section_ids(a);
    assert_eq!(
        connected_a.len(),
        1,
        "A should only be connected to itself after B deleted"
    );
    assert!(
        !connected_a.contains(&c),
        "A and C should not be connected after B deleted"
    );
}

/// Test 2d: Unmerging one edge in a chain A↔B↔C does not leave B with a
/// stale offset. After unmerging A↔B, B is isolated and should report itself
/// as disconnected from both A and C.  The remaining B↔C edge must still keep
/// B and C colocated at their merge point.
#[test]
fn test_unmerge_in_chain_does_not_leave_stale_offset() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "UnmergeChain".to_string(),
        location: None,
    });

    // A: 1 shot east (10m)
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![directional_shot_data(90.0, 10.0)],
        })
        .section_id
        .unwrap();

    // B: 1 shot north (5m)
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![directional_shot_data(0.0, 5.0)],
        })
        .section_id
        .unwrap();

    // C: 1 shot east (3m)
    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![directional_shot_data(90.0, 3.0)],
        })
        .section_id
        .unwrap();

    // Build chain: A.1↔B.0, B.1↔C.0
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });

    // Capture where B.1 and C.0 are (they should be colocated)
    let (b1_before, c0_before) = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        let b1 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(b, 1))
            .unwrap()
            .coordinates;
        let c0 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(c, 0))
            .unwrap()
            .coordinates;
        (b1, c0)
    };
    let eps = 1e-4;
    assert!(
        (b1_before.x - c0_before.x).abs() < eps,
        "B.1 and C.0 should be colocated before unmerge"
    );

    // Unmerge A↔B (the A.1 station is in the A↔B merge edge)
    let unmerge_result = state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(a, 1),
    });
    assert!(unmerge_result.success, "Unmerge should succeed");

    let proj = state.get_project_state().unwrap();

    // Only one merge edge should remain: B.1↔C.0
    assert_eq!(proj.station_merges.len(), 1, "Only B↔C edge should remain");

    // A should now be isolated (not connected to B or C)
    let connected_a = proj.get_connected_section_ids(a);
    assert_eq!(connected_a.len(), 1, "A should be isolated after unmerge");
    assert!(
        !connected_a.contains(&b),
        "A should not be connected to B after unmerge"
    );

    // B and C must still be colocated at their merge point
    let stations = proj.compute_station_coordinates();
    let b1_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 1))
        .unwrap()
        .coordinates;
    let c0_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 0))
        .unwrap()
        .coordinates;

    assert!(
        (b1_after.x - c0_after.x).abs() < eps,
        "B.1 and C.0 must remain colocated after unmerging A↔B: B.1=({},{}) C.0=({},{})",
        b1_after.x,
        b1_after.y,
        c0_after.x,
        c0_after.y
    );
    assert!(
        (b1_after.y - c0_after.y).abs() < eps,
        "B.1 and C.0 must remain colocated after unmerging A↔B"
    );
}

/// Test 2e: Datum-aware merge direction - when merging a datum-holding section
/// into a section without a datum, the datum section must not move; only the
/// non-datum section gets an offset.
///
/// Setup:
///   A: 1 shot east (10m), zero datum at A.0 pinning it to absolute world coords.
///   B: 1 shot north (5m), no datum.
///
///   Merge A.1 ↔ B.0  → B must move so B.0 = A.1; A must NOT move.
///
/// Assert A.0 remains at its original coordinates after the merge.
#[test]
fn test_merge_datum_section_does_not_move() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "DatumAware".to_string(),
        location: None,
    });

    // A: shot heading east
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![directional_shot_data(90.0, 10.0)],
        })
        .section_id
        .unwrap();

    // B: shot heading north
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![directional_shot_data(0.0, 5.0)],
        })
        .section_id
        .unwrap();

    // Pin A.0 to GPS - sets a zero datum on the A group
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    // Capture A's coordinates before merge
    let (a0_before, a1_before) = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        let a0 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(a, 0))
            .unwrap()
            .coordinates;
        let a1 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(a, 1))
            .unwrap()
            .coordinates;
        (a0, a1)
    };

    // Merge B into A: A.1 ↔ B.0
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    let eps = 1e-4;

    let a0_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap()
        .coordinates;
    let a1_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap()
        .coordinates;
    let b0_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap()
        .coordinates;

    // A must not have moved - coordinates must be identical before and after merge
    assert!(
        (a0_after.x - a0_before.x).abs() < eps,
        "A.0.x must not move: before={} after={}",
        a0_before.x,
        a0_after.x
    );
    assert!(
        (a0_after.y - a0_before.y).abs() < eps,
        "A.0.y must not move"
    );
    assert!(
        (a1_after.x - a1_before.x).abs() < eps,
        "A.1.x must not move: before={} after={}",
        a1_before.x,
        a1_after.x
    );
    assert!(
        (a1_after.y - a1_before.y).abs() < eps,
        "A.1.y must not move"
    );

    // B.0 must now be colocated with A.1
    assert!(
        (b0_after.x - a1_after.x).abs() < eps,
        "B.0 must align with A.1 (x): B.0={} A.1={}",
        b0_after.x,
        a1_after.x
    );
    assert!(
        (b0_after.y - a1_after.y).abs() < eps,
        "B.0 must align with A.1 (y)"
    );

    // The datum section A must not have received a non-zero section offset
    // (SetZeroDatum may write a (0,0) entry, but must not shift A during merge)
    if let Some(&(ox, oy)) = proj.section_offsets.get(&a) {
        assert!(
            ox.abs() < eps && oy.abs() < eps,
            "Datum section A must not receive a non-zero offset from merge: offset=({ox},{oy})"
        );
    }
    // B must have received an offset (it moved to align with A)
    assert!(
        proj.section_offsets.contains_key(&b),
        "Non-datum section B must have received a section offset from merge"
    );
}

/// Test 2f: A shot marked as reference (frozen) survives a merge followed by
/// an undo. After undo the shot must still be virtual.
#[test]
fn test_reference_shot_survives_merge_then_undo() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "RefShot".to_string(),
        location: None,
    });

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0)],
        })
        .section_id
        .unwrap();

    // Mark the first shot of A as a reference shot
    let first_shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.sections.iter().find(|s| s.id == a).unwrap().shots[0].id
    };
    let ref_result = state.execute(Command::SetShotKind {
        section_id: a,
        shot_id: first_shot_id,
        kind: ShotKind::Jump,
    });
    assert!(ref_result.success, "SetShotKind should succeed");

    // Confirm it's marked before merge
    {
        let proj = state.get_project_state().unwrap();
        let shot = proj
            .sections
            .iter()
            .find(|s| s.id == a)
            .unwrap()
            .shots
            .iter()
            .find(|sh| sh.id == first_shot_id)
            .unwrap();
        assert!(
            shot.kind.is_virtual(),
            "Shot should be virtual before merge"
        );
    }

    // Merge A.2 ↔ B.0
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    // Reference status must survive the merge
    {
        let proj = state.get_project_state().unwrap();
        let shot = proj
            .sections
            .iter()
            .find(|s| s.id == a)
            .unwrap()
            .shots
            .iter()
            .find(|sh| sh.id == first_shot_id)
            .unwrap();
        assert!(
            shot.kind.is_virtual(),
            "Shot must still be reference after merge"
        );
    }

    // Undo the merge
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    // Reference status must survive the undo
    {
        let proj = state.get_project_state().unwrap();
        let section_a = proj.sections.iter().find(|s| s.id == a).unwrap();
        let shot = section_a
            .shots
            .iter()
            .find(|sh| sh.id == first_shot_id)
            .unwrap();
        assert!(
            shot.kind.is_virtual(),
            "Shot must still be reference after undo of merge"
        );
        // Merge should be gone
        assert!(proj.station_merges.is_empty(), "Merge should be undone");
    }
}

// === Comment system tests ===

#[test]
fn test_set_station_comment_set_update_remove() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Comments".to_string(),
        location: None,
    });
    let sec = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let station = StationRef::new(sec, 1);
    // Set
    assert!(
        state
            .execute(Command::SetStationComment {
                station_ref: station,
                comment: Some("marker A".to_string()),
            })
            .success
    );
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("marker A")
    );
    // Update
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("marker B".to_string()),
    });
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("marker B")
    );
    // Remove via None
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: None,
    });
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .is_some(),
        "None removes comment"
    );
    // Remove via empty string
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("temp".to_string()),
    });
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some(String::new()),
    });
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .is_some(),
        "empty string removes comment"
    );
}

#[test]
fn test_set_station_comment_invalid_refs_fail() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "BadComments".to_string(),
        location: None,
    });
    let sec = state
        .execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(3.0)],
        })
        .section_id
        .unwrap();
    // 1 shot -> stations 0..1, index 3 is OOB
    assert!(
        !state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(sec, 3),
                comment: Some("oob".to_string()),
            })
            .success,
        "OOB index should fail"
    );
    assert!(
        !state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(9999, 0),
                comment: Some("ghost".to_string()),
            })
            .success,
        "nonexistent section should fail"
    );
}

#[test]
fn test_station_comment_survives_unrelated_undo_redo() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "CommentUndo".to_string(),
        location: None,
    });
    let sec = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();
    let station = StationRef::new(sec, 1);
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("keep me".to_string()),
    });
    // Unrelated op
    state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(2.0)],
    });
    assert!(state.undo().success);
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("keep me"),
        "survives undo"
    );
    assert!(state.redo().success);
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("keep me"),
        "survives redo"
    );
}

#[test]
fn test_station_comment_persists_across_save_open() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("comments.stalx");
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "CommentPersist".to_string(),
        location: None,
    });
    let sec = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    let station = StationRef::new(sec, 1);
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("persisted note".to_string()),
    });
    assert!(state.save_project(Some(save_path.clone())).success);
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(save_path).success);
    assert_eq!(
        state2
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("persisted note"),
        "comment survives save/open"
    );
}

#[test]
fn test_move_comment_transfers_between_stations() {
    use crate::core::command::CommentHolder;
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "MoveComment".to_string(),
        location: None,
    });
    let sec = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();
    let src = StationRef::new(sec, 0);
    let tgt = StationRef::new(sec, 2);
    state.execute(Command::SetStationComment {
        station_ref: src,
        comment: Some("source text".to_string()),
    });
    let r = state.execute(Command::MoveComment {
        source: CommentHolder::Station { station_ref: src },
        target: CommentHolder::Station { station_ref: tgt },
        swap: false,
    });
    assert!(r.success, "MoveComment should succeed");
    let proj = state.get_project_state().unwrap();
    assert!(
        !proj.station_comment_for_ref(src).is_some(),
        "source has no comment after move"
    );
    assert_eq!(
        proj.station_comment_for_ref(tgt).map(String::as_str),
        Some("source text"),
        "target has comment"
    );
}

#[test]
fn test_move_comment_swap_exchanges_comments() {
    use crate::core::command::CommentHolder;
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "SwapComment".to_string(),
        location: None,
    });
    let sec = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();
    let s1 = StationRef::new(sec, 0);
    let s2 = StationRef::new(sec, 2);
    state.execute(Command::SetStationComment {
        station_ref: s1,
        comment: Some("alpha".to_string()),
    });
    state.execute(Command::SetStationComment {
        station_ref: s2,
        comment: Some("beta".to_string()),
    });
    assert!(
        state
            .execute(Command::MoveComment {
                source: CommentHolder::Station { station_ref: s1 },
                target: CommentHolder::Station { station_ref: s2 },
                swap: true,
            })
            .success
    );
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.station_comment_for_ref(s1).map(String::as_str),
        Some("beta")
    );
    assert_eq!(
        proj.station_comment_for_ref(s2).map(String::as_str),
        Some("alpha")
    );
}

// === Import hash tracking tests ===

#[test]
fn test_import_tracking_happy_path() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Hashes".to_string(),
        location: None,
    });
    let hash = "abc123deadbeef".to_string();
    assert!(!state.check_duplicate_import(&hash).unwrap());
    assert!(state.track_imported_file(hash.clone()).success);
    assert!(state.check_duplicate_import(&hash).unwrap());
}

#[test]
fn test_import_tracking_idempotent() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "IdempHash".to_string(),
        location: None,
    });
    let hash = "deadbeef".to_string();
    state.track_imported_file(hash.clone());
    state.track_imported_file(hash.clone());
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.imported_file_hashes
            .iter()
            .filter(|h| h.as_str() == "deadbeef")
            .count(),
        1
    );
}

#[test]
fn test_section_hash_tracking_batch() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "SectionHashes".to_string(),
        location: None,
    });
    let hashes = vec!["h1".to_string(), "h2".to_string(), "h3".to_string()];
    assert_eq!(
        state.check_section_hashes(&hashes).unwrap(),
        vec![false, false, false]
    );
    state.track_section_hashes(vec!["h1".to_string(), "h3".to_string()]);
    assert_eq!(
        state.check_section_hashes(&hashes).unwrap(),
        vec![true, false, true]
    );
    state.track_section_hashes(hashes.clone());
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.imported_section_hashes
            .iter()
            .filter(|h| h.as_str() == "h1")
            .count(),
        1,
        "h1 must not be duplicated"
    );
}

#[test]
fn test_import_tracking_no_project_returns_false() {
    let state = AppState::new().unwrap();
    let result = state.check_duplicate_import("somehash");
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[test]
fn test_track_imported_file_no_project_fails() {
    let state = AppState::new().unwrap();
    assert!(!state.track_imported_file("hash".to_string()).success);
}

/// Withdrawing an import forgets its dedup bookkeeping, in the SAME undo entry
/// as the deletes (#394).
///
/// `execute_import_sections` records the file hash and each section's content
/// hash so a re-import is recognised as a duplicate, and `delete_section` does
/// not touch them - `cleanup_section_references` walks merges, closures,
/// offsets, comments and datums, nothing else. So removing the sections used to
/// leave the project claiming the file was already imported while none of it
/// remained, and the DMP selection dialog DESELECTS what it believes is a
/// duplicate: the obvious retry silently offered nothing to import.
#[test]
fn a_withdrawal_forgets_the_import_tracking_and_undo_restores_it() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Withdraw".to_string(),
        location: None,
    });

    let section_id = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();
    state.track_imported_file("file-hash".to_string());
    state.track_section_hashes(vec!["sec-a".to_string(), "sec-b".to_string()]);
    assert!(state.check_duplicate_import("file-hash").unwrap());

    // The withdrawal: delete the section AND forget what it was tracked as.
    let withdrawn = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteSection { section_id },
            Command::UntrackImport {
                file_hashes: vec!["file-hash".to_string()],
                section_hashes: vec!["sec-a".to_string()],
            },
        ],
    });
    assert!(withdrawn.success, "withdrawal: {:?}", withdrawn.error);

    assert!(
        !state.check_duplicate_import("file-hash").unwrap(),
        "the file must stop counting as imported once its sections are gone"
    );
    assert_eq!(
        state
            .check_section_hashes(&["sec-a".to_string(), "sec-b".to_string()])
            .unwrap(),
        vec![false, true],
        "only the named section hash is forgotten; an untouched one stays"
    );

    // ONE undo brings back the sections AND their tracking - both live on
    // `Project`, so the batch's snapshot pair covers them together.
    assert!(state.undo().success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
    assert!(
        state.check_duplicate_import("file-hash").unwrap(),
        "undoing the withdrawal must restore the duplicate flag with the sections"
    );
    assert_eq!(
        state.check_section_hashes(&["sec-a".to_string()]).unwrap(),
        vec![true]
    );
}

/// Untracking a hash nobody tracked is a no-op, not a failure: the wizard names
/// what its own run SENT to `import_sections`, and whether each hash is still
/// tracked is not something it can know. A failure here would roll back the
/// whole withdrawal batch.
#[test]
fn untracking_an_unknown_hash_succeeds() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Withdraw".to_string(),
        location: None,
    });

    let result = state.execute(Command::Batch {
        commands: vec![Command::UntrackImport {
            file_hashes: vec!["never-seen".to_string()],
            section_hashes: vec!["nor-this".to_string()],
        }],
    });

    assert!(result.success, "{:?}", result.error);
}

// === Undo-after-restart end-to-end test ===

#[test]
fn test_undo_actually_works_after_project_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("undo-restart.stalx");
    // Session 1
    let state1 = AppState::new().unwrap();
    state1.execute(Command::CreateProject {
        name: "UndoRestart".to_string(),
        location: None,
    });
    let create_result = state1.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    assert!(create_result.success);
    let section_id = create_result.section_id.unwrap();
    assert!(state1.save_project(Some(save_path.clone())).success);
    // Session 2: reopen and undo
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(save_path).success);
    {
        let stack = state2.undo_stack.lock().unwrap();
        assert!(stack.len() > 0, "undo stack should be restored");
    }
    assert!(state2.undo().success, "undo should work after reopen");
    assert!(
        !state2
            .get_project_state()
            .unwrap()
            .sections
            .iter()
            .any(|s| s.id == section_id),
        "section should be undone after reopen"
    );
}

// ==========================================================================
// Group 1: Round-trip fidelity tests
// ==========================================================================

/// Export project to TML, re-import it, verify shot data matches exactly.
#[test]
fn test_tml_roundtrip_preserves_shot_data() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "TML Roundtrip".to_string(),
        location: None,
    });

    let shot = ShotData {
        azimuth_in: 12.5,
        azimuth_out: 192.3,
        length: 8.75,
        depth_in: 3.2,
        depth_out: 5.8,
        left: Some(1.1),
        right: Some(0.9),
        up: Some(2.3),
        down: Some(0.4),
        comment: Some("test note".to_string()),
        ..Default::default()
    };
    let shot2 = ShotData {
        azimuth_in: 280.0,
        azimuth_out: 100.0,
        length: 3.25,
        depth_in: 5.8,
        depth_out: 4.0,
        ..Default::default()
    };
    state.execute(Command::CreateSection {
        name: Some("Main Passage".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot.clone(), shot2.clone()],
    });

    let project = state.get_project_state().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let tml_path = temp_dir.path().join("roundtrip.tmlu");

    crate::export::export_tml(&project, &tml_path).unwrap();
    let parsed = crate::parsers::tml::parse_tml(&tml_path).unwrap();
    let (configs, _closures, _warnings) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, true, false).unwrap();

    assert!(!configs.is_empty(), "should have at least one section");
    let imported_shots = &configs[0].shots;
    assert_eq!(imported_shots.len(), 2, "should have 2 shots");

    let eps = 0.05; // TML format has limited precision
    let s = &imported_shots[0];
    assert!(
        (s.length - 8.75).abs() < eps,
        "length: {} vs 8.75",
        s.length
    );
    assert!(
        (s.azimuth_in - 12.5).abs() < eps,
        "azimuth_in: {} vs 12.5",
        s.azimuth_in
    );
    assert!(
        (s.depth_in - 3.2).abs() < eps,
        "depth_in: {} vs 3.2",
        s.depth_in
    );
    assert!(
        (s.depth_out - 5.8).abs() < eps,
        "depth_out: {} vs 5.8",
        s.depth_out
    );

    let s2 = &imported_shots[1];
    assert!(
        (s2.length - 3.25).abs() < eps,
        "shot2 length: {} vs 3.25",
        s2.length
    );
    assert!(
        (s2.azimuth_in - 280.0).abs() < eps,
        "shot2 azimuth_in: {} vs 280.0",
        s2.azimuth_in
    );
}

/// Export project to TML with multiple sections, re-import, verify section count and names.
#[test]
fn test_tml_roundtrip_preserves_multiple_sections() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Multi Section".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("Gallery A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
    });
    state.execute(Command::CreateSection {
        name: Some("Gallery B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![test_shot_data(7.0)],
    });

    let project = state.get_project_state().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let tml_path = temp_dir.path().join("multi.tmlu");

    crate::export::export_tml(&project, &tml_path).unwrap();
    let parsed = crate::parsers::tml::parse_tml(&tml_path).unwrap();
    let (configs, _, _) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, true, false).unwrap();

    assert_eq!(configs.len(), 2, "should have 2 sections");
    // Total shots: 2 + 1 = 3
    let total_shots: usize = configs.iter().map(|c| c.shots.len()).sum();
    assert_eq!(total_shots, 3, "total shots across sections");
}

/// Stalx save/open round-trip preserves merges, closures, and offsets.
#[test]
fn test_stalx_roundtrip_preserves_merges_closures_offsets() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("full_roundtrip.stalx");

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Full Roundtrip".to_string(),
        location: None,
    });

    // Create two sections with 2 shots each
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0), test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:2 with B:0
    let merge_r = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 2),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_r.success, "merge should succeed");

    // Set a zero datum on A:0
    let datum_r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 0),
        coordinates: GpsCoordinates::new(45.5, -73.5, 100.0).unwrap(),
    });
    assert!(datum_r.success, "set zero datum should succeed");

    // Set a station comment
    state.execute(Command::SetStationComment {
        station_ref: StationRef::new(sec_b, 1),
        comment: Some("junction marker".to_string()),
    });

    // Snapshot state before save
    let project_before = state.get_project_state().unwrap();
    assert_eq!(project_before.station_merges.len(), 1);
    assert_eq!(project_before.zero_datums.len(), 1);

    // Save and reopen
    assert!(state.save_project(Some(save_path.clone())).success);
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(save_path).success);
    let project_after = state2.get_project_state().unwrap();

    // Verify merges
    assert_eq!(
        project_after.station_merges.len(),
        1,
        "merge should survive save/open"
    );
    let m = &project_after.station_merges[0];
    let merge_stations: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(merge_stations.contains(&StationRef::new(sec_a, 2)));
    assert!(merge_stations.contains(&StationRef::new(sec_b, 0)));

    // Verify zero datums
    assert_eq!(
        project_after.zero_datums.len(),
        1,
        "datum should survive save/open"
    );
    assert!(project_after.key_is_ref(
        &project_after.zero_datums[0].station_ref,
        StationRef::new(sec_a, 0)
    ));
    assert!((project_after.zero_datums[0].gps.latitude - 45.5).abs() < 1e-6);

    // Verify station comments
    assert_eq!(
        project_after
            .station_comment_for_ref(StationRef::new(sec_b, 1))
            .map(String::as_str),
        Some("junction marker"),
        "comment should survive save/open"
    );

    // Verify section offsets exist for merged sections
    assert!(
        project_after.section_offsets.contains_key(&sec_a)
            || project_after.section_offsets.contains_key(&sec_b),
        "at least one section should have an offset after merge"
    );

    // Verify section data integrity
    assert_eq!(project_after.sections.len(), 2);
    let sec_a_after = project_after
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap();
    let sec_b_after = project_after
        .sections
        .iter()
        .find(|s| s.id == sec_b)
        .unwrap();
    assert_eq!(sec_a_after.shots.len(), 2);
    assert_eq!(sec_b_after.shots.len(), 2);
}

/// CSV export preserves precision for azimuths near 360/0 boundary.
#[test]
fn test_csv_export_preserves_azimuth_precision() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "CSV Precision".to_string(),
        location: None,
    });

    let shot = ShotData {
        azimuth_in: 359.9,
        azimuth_out: 0.1,
        length: 4.567,
        depth_in: 12.345,
        depth_out: 12.678,
        left: Some(0.01),
        right: Some(99.99),
        ..Default::default()
    };
    state.execute(Command::CreateSection {
        name: Some("Boundary".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot],
    });

    let project = state.get_project_state().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let csv_path = temp_dir.path().join("precision.csv");
    crate::export::export_csv(&project, &csv_path).unwrap();

    let contents = std::fs::read_to_string(&csv_path).unwrap();
    let data_line = contents.lines().nth(1).unwrap();
    // Verify azimuths aren't truncated: 359.9 should not become 359 or 360
    assert!(
        data_line.contains("359.9"),
        "azimuth_in 359.9 should be preserved, got: {}",
        data_line
    );
    assert!(
        data_line.contains("0.1"),
        "azimuth_out 0.1 should be preserved, got: {}",
        data_line
    );
    // Verify length precision: 4.567 should appear as 4.57 (2 decimal CSV format)
    assert!(
        data_line.contains("4.57"),
        "length should be 4.57, got: {}",
        data_line
    );
}

// ==========================================================================
// Group 2: Shot insertion/deletion near merge points
// ==========================================================================

/// Insert a shot before a merged station - verify merge ref shifts correctly
/// and merge alignment is maintained via cascade offset propagation.
#[test]
fn test_insert_shot_before_merged_station_shifts_merge_ref() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "InsertBeforeMerge".to_string(),
        location: None,
    });

    // Section A: 3 shots (stations 0,1,2,3)
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(3.0),
                test_shot_data(4.0),
            ],
        })
        .section_id
        .unwrap();

    // Section B: 1 shot (stations 0,1)
    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:2 with B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 2),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Get shot ID of shot at index 1 in section A (before the merged station)
    let project = state.get_project_state().unwrap();
    let shot_id_1 = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[1]
        .id;

    // Insert a new shot before shot 1 - this adds a station, shifting station 2 to 3
    let insert_r = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: test_shot_data(2.0),
        after_shot_id: None,
        before_shot_id: Some(shot_id_1),
    });
    assert!(insert_r.success, "insert should succeed");

    // After insertion, A now has 4 shots (stations 0..4)
    // Original A:2 should now be A:3
    let project_after = state.get_project_state().unwrap();
    let sec_a_after = project_after
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap();
    assert_eq!(
        sec_a_after.shots.len(),
        4,
        "section A should now have 4 shots"
    );

    // The merge should now reference A:3 (shifted from A:2) and B:0 (unchanged)
    assert_eq!(
        project_after.station_merges.len(),
        1,
        "merge should still exist"
    );
    let m = &project_after.station_merges[0];
    let merge_refs: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(
        merge_refs.contains(&StationRef::new(sec_b, 0)),
        "B:0 should be unchanged in merge"
    );
    assert!(
        merge_refs.contains(&StationRef::new(sec_a, 3)),
        "A:2 should have shifted to A:3 in merge, got {:?}",
        m
    );

    // Merged stations should remain colocated (the fix)
    let a3 = project_after.compute_station_raw_coords(StationRef::new(sec_a, 3));
    let b0 = project_after.compute_station_raw_coords(StationRef::new(sec_b, 0));
    if let (Some(a3c), Some(b0c)) = (a3, b0) {
        let eps = 0.01;
        assert!(
            (a3c.x - b0c.x).abs() < eps,
            "A:3 and B:0 x should be colocated after insert: {} vs {}",
            a3c.x,
            b0c.x
        );
        assert!(
            (a3c.y - b0c.y).abs() < eps,
            "A:3 and B:0 y should be colocated after insert: {} vs {}",
            a3c.y,
            b0c.y
        );
    }
}

/// Insert a shot after a merged station - verify merge ref stays unchanged.
#[test]
fn test_insert_shot_after_merged_station_keeps_merge_ref() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "InsertAfterMerge".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:1 with B:0 (middle station of A)
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Insert a shot AFTER shot[0] in A (between stations 1 and 2)
    let project = state.get_project_state().unwrap();
    let shot_id_0 = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;

    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sec_a,
                shot: test_shot_data(2.0),
                after_shot_id: Some(shot_id_0),
                before_shot_id: None,
            })
            .success
    );

    // A:1 (the merged station) should remain at index 1 since we inserted after shot[0]
    // which creates a new station at index 2, shifting old station 2 to 3
    let project_after = state.get_project_state().unwrap();
    let m = &project_after.station_merges[0];
    let merge_refs: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(
        merge_refs.contains(&StationRef::new(sec_a, 1)),
        "A:1 should remain unchanged since insertion is after it"
    );
    assert!(
        merge_refs.contains(&StationRef::new(sec_b, 0)),
        "B:0 should remain unchanged"
    );
}

/// Delete a mid-shot in a merged section - verify merges are rewired and
/// no dangling station references remain after the section splits.
#[test]
fn test_delete_mid_shot_splits_section_with_merge_intact() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "DeleteMidMerge".to_string(),
        location: None,
    });

    // Section A: 3 shots (stations 0,1,2,3)
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(3.0),
                test_shot_data(4.0),
            ],
        })
        .section_id
        .unwrap();

    // Section B: 1 shot (stations 0,1)
    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:0 with B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 0),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Delete the middle shot (shot[1]) from section A - this splits A
    let project = state.get_project_state().unwrap();
    let shot_id_1 = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[1]
        .id;

    let delete_r = state.execute(Command::DeleteShot {
        section_id: sec_a,
        shot_id: shot_id_1,
    });
    assert!(delete_r.success, "delete should succeed");

    // After splitting, the project should be in a consistent state:
    // no merge should reference a nonexistent station
    let project_after = state.get_project_state().unwrap();
    for merge in &project_after.station_merges {
        let valid_a = project_after.resolve_key(&merge.station_a).is_some();
        let valid_b = project_after.resolve_key(&merge.station_b).is_some();
        assert!(
            valid_a,
            "merge station_a {:?} references valid station",
            merge.station_a
        );
        assert!(
            valid_b,
            "merge station_b {:?} references valid station",
            merge.station_b
        );
    }

    // The merge to B:0 should still exist (connected to one of the split fragments)
    let b_in_merge = project_after.station_merges.iter().any(|m| {
        project_after.key_is_ref(&m.station_a, StationRef::new(sec_b, 0))
            || project_after.key_is_ref(&m.station_b, StationRef::new(sec_b, 0))
    });
    assert!(
        b_in_merge,
        "B:0 merge should survive the split (rewired to fragment)"
    );
}

/// Insert shot at position 0 of a merged section - verify offset adjustment keeps
/// existing stations visually in place and merged partner remains colocated.
#[test]
fn test_insert_shot_at_position_0_adjusts_offset_for_merge() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "InsertPos0Merge".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:0 with B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 0),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Record coordinates of A:1 before insertion
    let project_before = state.get_project_state().unwrap();
    let coords_a1_before = project_before.compute_station_raw_coords(StationRef::new(sec_a, 1));

    // Insert a shot before shot[0] in section A (prepend)
    let shot_id_0 = project_before
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;
    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sec_a,
                shot: test_shot_data(3.0),
                after_shot_id: None,
                before_shot_id: Some(shot_id_0),
            })
            .success
    );

    let project_after = state.get_project_state().unwrap();

    // A:1 (previously A:0) was the merged station - it should have shifted to A:1
    // A:2 (previously A:1) should be at the same coordinates as before
    if let Some(before) = coords_a1_before {
        if let Some(after) = project_after.compute_station_raw_coords(StationRef::new(sec_a, 2)) {
            let eps = 0.1;
            assert!(
                (before.x - after.x).abs() < eps,
                "A:2 x should match old A:1 x: {} vs {}",
                after.x,
                before.x
            );
            assert!(
                (before.y - after.y).abs() < eps,
                "A:2 y should match old A:1 y: {} vs {}",
                after.y,
                before.y
            );
        }
    }

    // Merge should now reference the shifted station
    let m = &project_after.station_merges[0];
    let merge_refs: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(
        merge_refs.contains(&StationRef::new(sec_a, 1)),
        "A:0 should have shifted to A:1 in merge"
    );
    assert!(
        merge_refs.contains(&StationRef::new(sec_b, 0)),
        "B:0 should remain unchanged"
    );
}

/// Delete the only shot+section via execute_delete_last_shot_and_section
/// when section has merges - verify merges are cleaned up.
#[test]
fn test_delete_last_shot_and_section_cleans_merges() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "DeleteLastMerged".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:1 with B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Delete section A's only shot via the dedicated method
    let project = state.get_project_state().unwrap();
    let shot_id = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;
    assert!(
        state
            .execute_delete_last_shot_and_section(sec_a, shot_id)
            .success
    );

    let project_after = state.get_project_state().unwrap();
    // Section A should be gone
    assert!(
        !project_after.sections.iter().any(|s| s.id == sec_a),
        "section A should be deleted"
    );
    // Merge referencing section A should be cleaned up
    for merge in &project_after.station_merges {
        assert_ne!(
            merge.station_a.section_id, sec_a,
            "no merge should reference deleted section A"
        );
        assert_ne!(
            merge.station_b.section_id, sec_a,
            "no merge should reference deleted section A"
        );
    }
    // Section B should still exist
    assert!(
        project_after.sections.iter().any(|s| s.id == sec_b),
        "section B should remain"
    );
}

/// Insert shot with a zero datum present - verify datum station ref shifts correctly.
#[test]
fn test_insert_shot_before_zero_datum_shifts_datum_ref() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "InsertBeforeDatum".to_string(),
        location: None,
    });

    let sec = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    // Set zero datum at station 1
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec, 1),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );

    // Insert a shot before shot[0], shifting all stations by 1
    let project = state.get_project_state().unwrap();
    let shot_id_0 = project.sections.iter().find(|s| s.id == sec).unwrap().shots[0].id;
    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sec,
                shot: test_shot_data(2.0),
                after_shot_id: None,
                before_shot_id: Some(shot_id_0),
            })
            .success
    );

    let project_after = state.get_project_state().unwrap();
    assert_eq!(
        project_after.zero_datums.len(),
        1,
        "datum should still exist"
    );
    assert!(
        project_after.key_is_ref(
            &project_after.zero_datums[0].station_ref,
            StationRef::new(sec, 2)
        ),
        "datum should have shifted from station 1 to station 2"
    );
}

// ==========================================================================
// Group 3: Multi-section merge chain operations
// ==========================================================================

/// Delete the middle section in a 3-section merge chain (A↔B↔C).
/// Verify A and C become disconnected with no dangling merge references.
#[test]
fn test_delete_middle_section_in_merge_chain() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "ChainDelete".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Chain: A:1 ↔ B:0, B:1 ↔ C:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 1),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // Delete section B
    assert!(
        state
            .execute(Command::DeleteSection { section_id: sec_b })
            .success
    );

    let project_after = state.get_project_state().unwrap();

    // B should be gone
    assert!(
        !project_after.sections.iter().any(|s| s.id == sec_b),
        "section B should be deleted"
    );

    // No merge should reference section B
    for merge in &project_after.station_merges {
        assert_ne!(
            merge.station_a.section_id, sec_b,
            "merge should not reference deleted B"
        );
        assert_ne!(
            merge.station_b.section_id, sec_b,
            "merge should not reference deleted B"
        );
    }

    // No closure should reference section B
    for closure in &project_after.loop_closures {
        assert_ne!(
            closure.station_a.section_id, sec_b,
            "closure should not reference deleted B"
        );
        assert_ne!(
            closure.station_b.section_id, sec_b,
            "closure should not reference deleted B"
        );
    }

    // No zero datum should reference section B
    for datum in &project_after.zero_datums {
        assert_ne!(
            datum.station_ref.section_id, sec_b,
            "datum should not reference deleted B"
        );
    }

    // A and C should still exist
    assert!(
        project_after.sections.iter().any(|s| s.id == sec_a),
        "A should remain"
    );
    assert!(
        project_after.sections.iter().any(|s| s.id == sec_c),
        "C should remain"
    );
}

/// Set zero datum in a 3-section chain, then change GPS - verify all sections move together.
#[test]
fn test_zero_datum_in_chain_positions_all_sections() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "ChainDatum".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Chain: A:1 ↔ B:0, B:1 ↔ C:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 1),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // Set datum at A:0
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec_a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );

    let project = state.get_project_state().unwrap();

    // All stations in the chain should have computable coordinates
    let a0 = project.compute_station_raw_coords(StationRef::new(sec_a, 0));
    let a1 = project.compute_station_raw_coords(StationRef::new(sec_a, 1));
    let b0 = project.compute_station_raw_coords(StationRef::new(sec_b, 0));
    let b1 = project.compute_station_raw_coords(StationRef::new(sec_b, 1));
    let c0 = project.compute_station_raw_coords(StationRef::new(sec_c, 0));
    let c1 = project.compute_station_raw_coords(StationRef::new(sec_c, 1));

    assert!(a0.is_some(), "A:0 should have coordinates");
    assert!(c1.is_some(), "C:1 should have coordinates (end of chain)");

    // Merged stations should be colocated: A:1 ≈ B:0, B:1 ≈ C:0
    if let (Some(a1c), Some(b0c)) = (a1, b0) {
        let eps = 0.5;
        assert!(
            (a1c.x - b0c.x).abs() < eps,
            "A:1 and B:0 x should be colocated: {} vs {}",
            a1c.x,
            b0c.x
        );
        assert!(
            (a1c.y - b0c.y).abs() < eps,
            "A:1 and B:0 y should be colocated: {} vs {}",
            a1c.y,
            b0c.y
        );
    }
    if let (Some(b1c), Some(c0c)) = (b1, c0) {
        let eps = 0.5;
        assert!(
            (b1c.x - c0c.x).abs() < eps,
            "B:1 and C:0 x should be colocated: {} vs {}",
            b1c.x,
            c0c.x
        );
        assert!(
            (b1c.y - c0c.y).abs() < eps,
            "B:1 and C:0 y should be colocated: {} vs {}",
            b1c.y,
            c0c.y
        );
    }

    // Now update the GPS coordinates and verify the chain still holds
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec_a, 0),
                coordinates: GpsCoordinates::new(46.0, -74.0, 200.0).unwrap(),
            })
            .success
    );

    let project2 = state.get_project_state().unwrap();
    let a1_new = project2.compute_station_raw_coords(StationRef::new(sec_a, 1));
    let b0_new = project2.compute_station_raw_coords(StationRef::new(sec_b, 0));
    if let (Some(a1c), Some(b0c)) = (a1_new, b0_new) {
        let eps = 0.5;
        assert!(
            (a1c.x - b0c.x).abs() < eps,
            "after GPS update, A:1 and B:0 should remain colocated"
        );
    }
}

/// Undo a merge that was part of a chain - verify the undone section disconnects
/// while the remaining merge stays intact.
#[test]
fn test_undo_merge_in_chain_disconnects_correctly() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "UndoChainMerge".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:1 ↔ B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Merge B:1 ↔ C:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 1),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // Verify chain: 2 merges
    let project = state.get_project_state().unwrap();
    assert_eq!(project.station_merges.len(), 2);

    // Undo the B↔C merge (most recent operation)
    assert!(state.undo().success);

    let project_after = state.get_project_state().unwrap();
    assert_eq!(
        project_after.station_merges.len(),
        1,
        "only A↔B merge should remain"
    );

    // The remaining merge should be A:1 ↔ B:0
    let m = &project_after.station_merges[0];
    let merge_refs: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(
        merge_refs.contains(&StationRef::new(sec_a, 1)),
        "A:1 should be in remaining merge"
    );
    assert!(
        merge_refs.contains(&StationRef::new(sec_b, 0)),
        "B:0 should be in remaining merge"
    );

    // C should still exist but disconnected
    assert!(
        project_after.sections.iter().any(|s| s.id == sec_c),
        "C should still exist"
    );

    // Redo should restore the B↔C merge
    assert!(state.redo().success);
    let project_redo = state.get_project_state().unwrap();
    assert_eq!(
        project_redo.station_merges.len(),
        2,
        "both merges should be restored after redo"
    );
}

/// Verify a merge chain survives save/reopen with coordinates intact.
#[test]
fn test_merge_chain_survives_save_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("chain.stalx");

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "ChainPersist".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    // Chain: A:1 ↔ B:0, B:1 ↔ C:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 1),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // Set datum so coordinates are computable
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec_a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );

    // Snapshot coordinates
    let project_before = state.get_project_state().unwrap();
    let c1_before = project_before.compute_station_raw_coords(StationRef::new(sec_c, 1));

    // Save and reopen
    assert!(state.save_project(Some(save_path.clone())).success);
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(save_path).success);

    let project_after = state2.get_project_state().unwrap();

    // Verify structure
    assert_eq!(
        project_after.station_merges.len(),
        2,
        "both merges should persist"
    );
    assert_eq!(project_after.zero_datums.len(), 1, "datum should persist");
    assert_eq!(
        project_after.sections.len(),
        3,
        "all 3 sections should persist"
    );

    // Verify coordinates match
    let c1_after = project_after.compute_station_raw_coords(StationRef::new(sec_c, 1));
    if let (Some(before), Some(after)) = (c1_before, c1_after) {
        let eps = 0.01;
        assert!(
            (before.x - after.x).abs() < eps,
            "C:1 x should match: {} vs {}",
            before.x,
            after.x
        );
        assert!(
            (before.y - after.y).abs() < eps,
            "C:1 y should match: {} vs {}",
            before.y,
            after.y
        );
    }
}

/// Three sections forming a triangle: A↔B via merge, B↔C via merge, C↔A via loop closure.
/// Verifies that a loop closure can be created between non-merged stations in the same
/// connected group, and that all references remain valid.
#[test]
fn test_triangle_topology_with_loop_closure() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Triangle".to_string(),
        location: None,
    });

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0), test_shot_data(3.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![test_shot_data(4.0), test_shot_data(6.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![test_shot_data(7.0), test_shot_data(2.0)],
        })
        .section_id
        .unwrap();

    // A:2 ↔ B:0 (merge)
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 2),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // B:2 ↔ C:0 (merge)
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 2),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // C:2 ↔ A:0 - third merge should be rejected (already connected transitively)
    let third_merge = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_c, 2),
        station_b: StationRef::new(sec_a, 0),
    });
    assert!(
        !third_merge.success,
        "third merge should fail - stations already connected"
    );

    // Close the triangle with a loop closure C:2 ↔ A:0.
    // These are the free endpoints of their sections - the merge graph only has
    // edges A:2↔B:0 and B:2↔C:0, so are_stations_merged(C:2, A:0) is false.
    // The sections are connected (same group), so the closure succeeds and
    // measures the survey error around the A→B→C→A triangle.
    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(sec_c, 2),
        station_b: StationRef::new(sec_a, 0),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(
        closure_result.success,
        "loop closure between free endpoints should succeed"
    );

    // Verify the project is in a consistent state: 2 merges + 1 closure.
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 3, "all 3 sections should exist");
    assert_eq!(project.station_merges.len(), 2, "two merges should exist");
    assert_eq!(
        project.loop_closures.len(),
        1,
        "one loop closure should exist"
    );

    // All merges reference valid stations
    for merge in &project.station_merges {
        assert!(
            project.resolve_key(&merge.station_a).is_some(),
            "merge station_a {:?} should be valid",
            merge.station_a
        );
        assert!(
            project.resolve_key(&merge.station_b).is_some(),
            "merge station_b {:?} should be valid",
            merge.station_b
        );
    }

    // Loop closure references valid stations
    let lc = &project.loop_closures[0];
    assert!(
        project.resolve_key(&lc.station_a).is_some(),
        "closure station_a should be valid"
    );
    assert!(
        project.resolve_key(&lc.station_b).is_some(),
        "closure station_b should be valid"
    );
}

// ============================================================================
// Performance profiler (not a unit test - a measurement harness)
// ============================================================================
//
// Run with:
//   STALAGMIGHT_PROFILE_TML=/path/to/large-project.tml \
//     cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
//     state::tests::profile_tml_import_full -- --nocapture --ignored
//
// The harness drives parse_tml → convert_tml_to_import_configs →
// execute_import_sections end-to-end on an empty in-memory project,
// installs a real tracing-subscriber so the timing markers added to
// commands.rs land on stdout, and prints wall-clock for each phase.

#[test]
#[ignore = "profiling harness - run explicitly with STALAGMIGHT_PROFILE_TML=<path>"]
fn profile_tml_import_full() {
    use crate::models::Project;
    use crate::parsers::tml as tml_parser;
    use crate::persistence::Database;
    use std::time::Instant;

    let path = std::env::var("STALAGMIGHT_PROFILE_TML")
        .expect("set STALAGMIGHT_PROFILE_TML to the .tml file path");

    // Use the test framework's tracing-subscriber init guard so we
    // don't double-init when the test runs alongside others.
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let t0 = Instant::now();
    let tml = tml_parser::parse_tml(std::path::Path::new(&path)).expect("parse_tml");
    let t1 = Instant::now();
    let (sections, closures, _warnings) =
        tml_parser::convert_tml_to_import_configs(&tml, false, false).expect("convert");
    let t2 = Instant::now();

    let project = Project::new("profile".to_string(), None);
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("profile.stalx");
    let mut db = Database::open(&db_path).unwrap();
    db.save_project(&project).unwrap();
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    state.set_database_for_tests(db);
    let t3 = Instant::now();

    let total_shots: usize = sections.iter().map(|s| s.shots.len()).sum();

    eprintln!("\n┌─ profile_tml_import_full ─────────────────────────────");
    eprintln!("│ file:     {path}");
    eprintln!("│ sections: {}", sections.len());
    eprintln!("│ shots:    {}", total_shots);
    eprintln!("│ closures: {}", closures.len());
    eprintln!("├─ phase timings ───────────────────────────────────────");
    eprintln!(
        "│ parse_tml:                   {:>9.2} ms",
        (t1 - t0).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ convert_tml_to_import_configs:{:>9.2} ms",
        (t2 - t1).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ AppState setup (excluded):   {:>9.2} ms",
        (t3 - t2).as_secs_f64() * 1000.0
    );

    let t_exec_start = Instant::now();
    let result = state.execute_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    let exec_elapsed = t_exec_start.elapsed();
    eprintln!(
        "│ execute_import_sections:     {:>9.2} ms",
        exec_elapsed.as_secs_f64() * 1000.0
    );
    eprintln!("├─ totals ──────────────────────────────────────────────");
    eprintln!(
        "│ end-to-end (parse+convert+execute): {:>9.2} ms",
        ((t1 - t0) + (t2 - t1) + exec_elapsed).as_secs_f64() * 1000.0
    );
    eprintln!("└───────────────────────────────────────────────────────");

    assert!(
        result.success,
        "execute_import_sections failed: {:?}",
        result.error
    );
}

// ============================================================================
// Performance profiler for project load (not a unit test)
// ============================================================================
//
// Run with:
//   STALAGMIGHT_PROFILE_STALX=/home/user/Documents/stalagmight/92.stalx \
//     cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
//     state::tests::profile_stalx_load -- --nocapture --ignored

#[test]
#[ignore = "profiling harness - run explicitly with STALAGMIGHT_PROFILE_STALX=<path>"]
fn profile_stalx_load() {
    use crate::persistence::Database;
    use std::time::Instant;

    let path = std::env::var("STALAGMIGHT_PROFILE_STALX")
        .expect("set STALAGMIGHT_PROFILE_STALX to the .stalx file path");

    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    // === Phase 0: SQLite open ===
    let t0 = Instant::now();
    let db = Database::open_existing(std::path::Path::new(&path)).expect("open_existing");
    let t1 = Instant::now();

    // === Phase 1: load_project (the bulk query loop) ===
    let mut project = db.load_project().expect("load_project");
    let t2 = Instant::now();

    let section_count = project.sections.len();
    let shot_count: usize = project.sections.iter().map(|s| s.shots.len()).sum();

    // === Phase 2: station coordinate computation ===
    let stations = project.compute_station_coordinates();
    let t3 = Instant::now();
    let station_count = stations.len();

    // === Phase 3: project stats + penetration ===
    let mut stats = project.stats();
    stats.penetration_distance = project.calculate_penetration(&stations);
    let t4 = Instant::now();

    // === Phase 4: warnings (via AppState because the impl is on AppState) ===
    project.file_path = Some(PathBuf::from(&path));
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project.clone());
    let warnings = state.compute_project_warnings(&project, Some(&db));
    let t5 = Instant::now();

    // === Phase 5: load_display_settings ===
    let _settings = db.load_display_settings().unwrap_or_default();
    let t6 = Instant::now();

    // === Phase 6: load_undo_stack_with_timestamps ===
    let _undo = db.load_undo_stack_with_timestamps().unwrap_or_default();
    let _redo = db.load_redo_stack_with_timestamps().unwrap_or_default();
    let t7 = Instant::now();

    // === Phase 7: binary envelope encoding ===
    let display_settings = crate::core::loading::DisplaySettings::from_stored(
        &db.load_display_settings().unwrap_or_default(),
    );
    let result = crate::core::loading::LoadProjectFullResult {
        project,
        stations,
        stats,
        warnings,
        display_settings,
        loading_id: 0,
        is_remote: false,
    };
    let bin = crate::core::load_binary::encode(result).expect("encode");
    let t8 = Instant::now();

    eprintln!("\n┌─ profile_stalx_load ──────────────────────────────────");
    eprintln!("│ file:      {path}");
    eprintln!("│ sections:  {section_count}");
    eprintln!("│ shots:     {shot_count}");
    eprintln!("│ stations:  {station_count}");
    eprintln!(
        "│ binary envelope: {} bytes ({:.1} MB)",
        bin.len(),
        bin.len() as f64 / 1024.0 / 1024.0
    );
    eprintln!("├─ phase timings ───────────────────────────────────────");
    eprintln!(
        "│ Database::open_existing:        {:>9.2} ms",
        (t1 - t0).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ db.load_project (SQLite):       {:>9.2} ms",
        (t2 - t1).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ compute_station_coordinates:    {:>9.2} ms",
        (t3 - t2).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ project.stats + penetration:    {:>9.2} ms",
        (t4 - t3).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ compute_project_warnings:       {:>9.2} ms",
        (t5 - t4).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ load_display_settings:          {:>9.2} ms",
        (t6 - t5).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ load_undo+redo_stacks:          {:>9.2} ms",
        (t7 - t6).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ binary envelope encode:         {:>9.2} ms",
        (t8 - t7).as_secs_f64() * 1000.0
    );
    eprintln!("├─ totals ──────────────────────────────────────────────");
    eprintln!(
        "│ end-to-end:                     {:>9.2} ms",
        (t8 - t0).as_secs_f64() * 1000.0
    );
    eprintln!("└───────────────────────────────────────────────────────");
}

#[test]
fn redo_create_shot_preserves_uuid_all_insert_modes() {
    // Redo of CreateShot must restore the shot's ORIGINAL UUID (not mint a fresh
    // one) on every insert mode - append, insert_after, and prepend - because
    // cross-references and Ariane matching anchor on it. Each mode hits a
    // distinct redo branch (append: Shot::with_uuid; after/before: the forced
    // uuid threaded through insert_shot_*).
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let sid = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    let uuid_of = |s: &AppState, shot_id: u32| -> Option<String> {
        s.get_project_state()
            .unwrap()
            .get_section(sid)
            .unwrap()
            .get_shot(shot_id)
            .unwrap()
            .uuid
            .clone()
    };
    let first_id = state
        .get_project_state()
        .unwrap()
        .get_section(sid)
        .unwrap()
        .shots[0]
        .id;
    let last_id = {
        let p = state.get_project_state().unwrap();
        let shots = &p.get_section(sid).unwrap().shots;
        shots[shots.len() - 1].id
    };

    let cases = [
        // (after_shot_id, before_shot_id) → append / insert_after / prepend
        Command::CreateShot {
            section_id: sid,
            shot: test_shot_data(3.0),
            after_shot_id: None,
            before_shot_id: None,
        },
        Command::CreateShot {
            section_id: sid,
            shot: test_shot_data(3.0),
            after_shot_id: Some(last_id),
            before_shot_id: None,
        },
        Command::CreateShot {
            section_id: sid,
            shot: test_shot_data(3.0),
            after_shot_id: None,
            before_shot_id: Some(first_id),
        },
    ];
    for cmd in cases {
        let res = state.execute(cmd);
        assert!(res.success, "create shot succeeds");
        let new_id = res.shot_id.expect("create shot returns id");
        let before = uuid_of(&state, new_id);
        assert!(before.is_some(), "created shot has a uuid");

        assert!(state.undo().success, "undo create shot");
        assert!(state.redo().success, "redo create shot");

        assert_eq!(
            uuid_of(&state, new_id),
            before,
            "redo must preserve the shot UUID"
        );
    }
}

#[test]
fn repair_open_project_integrity_reports_fixed_and_remaining() {
    // Repair returns the genuine before→after delta over the FULL violation set,
    // so a repairable issue counts as `fixed` and a detect-only one (a drifted
    // aligned loop closure) counts as `remaining`.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "X".to_string(),
        location: None,
    });
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![test_shot_data(5.0)],
        })
        .section_id
        .unwrap();

    // Inject a dangling merge (repairable: repair drops it).
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        let dangling = StationKey::new(a, StationAnchor::AfterShot("ghost".to_string()));
        project.station_merges.push(StationMerge {
            station_a: dangling,
            station_b: StationRef::new(a, 0).into(),
            merged_at: chrono::Utc::now(),
        });
    }

    let before = state.integrity_violation_messages().len();
    assert!(before >= 1, "seeded at least one violation");

    let result = state.repair_open_project_integrity();
    assert_eq!(
        (result.fixed + result.remaining) as usize,
        before,
        "fixed + remaining accounts for every issue the banner showed"
    );
    assert!(result.fixed >= 1, "the dangling merge was repaired");
    assert_eq!(
        result.remaining as usize,
        state.integrity_violation_messages().len(),
        "remaining matches the true post-repair count"
    );
}

#[test]
fn repair_command_demotes_a_double_anchoring_bridge_merge() {
    // End-to-end through the Repair action on the damage a past bug left in saved
    // files: a RIGID merge bridging two independently GPS-anchored merge
    // components. `resolve_offsets` cannot satisfy it (each half is pinned to its
    // own datum), so before this repair the project reported `merge_misaligned` +
    // `multiple_datums_per_merge_component` on every check and Repair could never
    // clear them. Fully synthetic: three sections built by command, welded by hand.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Damaged".to_string(),
        location: None,
    });
    let section = |name: &str| {
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#111111".to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![test_shot_data(10.0), test_shot_data(10.0)],
            })
            .section_id
            .unwrap()
    };
    let (a, b, c) = (section("A"), section("B"), section("C"));

    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        // The legitimate merge inside the first component, made through the real
        // API so it is correctly aligned.
        project
            .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
            .unwrap();
        // One datum per component - individually legal, which is why the
        // interactive guards never fired on the way in.
        project.zero_datums.push(crate::models::ZeroDatum {
            station_ref: StationRef::new(a, 0).into(),
            gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        });
        project.zero_datums.push(crate::models::ZeroDatum {
            station_ref: StationRef::new(c, 0).into(),
            gps: GpsCoordinates::new(45.001, -90.0, 0.0).unwrap(),
        });
        // Park C 500 m away, then weld it to B with the merge the promotion bug
        // created out of what should have stayed a loop closure.
        project.set_section_offset(c, (500.0, 0.0));
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(b, 2).into(),
            station_b: StationRef::new(c, 0).into(),
            merged_at: chrono::Utc::now(),
        });
    }

    let before = state.integrity_violation_messages().len();
    assert!(
        before >= 2,
        "seeded a torn merge AND an over-anchored merge component, got {before}"
    );

    let result = state.repair_open_project_integrity();
    assert!(
        result.fixed >= 1,
        "the demotion is reported as a fix, got {result:?}"
    );
    assert!(result.changed, "repair mutated the project");
    assert_eq!(
        result.remaining,
        0,
        "both symptoms clear: {:?}",
        state.integrity_violation_messages()
    );

    let guard = state.project.lock().unwrap();
    let project = guard.as_ref().unwrap();
    assert_eq!(
        project.station_merges.len(),
        1,
        "only the legitimate A-B merge survives"
    );
    assert!(
        project.are_stations_in_closure(StationRef::new(b, 2), StationRef::new(c, 0)),
        "the demoted pair is now a loop closure - and it is still one after \
         convert_orphan_closures_to_merges ran later in the SAME repair pass"
    );
    assert!(
        project.loop_closures.iter().all(|lc| !lc.is_aligned),
        "an aligned closure would re-impose the constraint the demotion removed"
    );
}

// ===========================================================================
// #191 - read-only edit gate + dirty bit + chokepoint enumeration (Track B).
// Stamped-remote fixture: a temp .stalx carrying a speleo_link row, opened so
// edit_enabled flips false (remote-open-without-lock). See guard_mutation /
// mark_remote_dirty / is_lifecycle_passthrough.
// ===========================================================================

use crate::core::command::EDIT_DISABLED_CODE;
use crate::models::SpeleoLink;

/// A SpeleoLink fixture (clean: no unpushed edits).
fn fixture_speleo_link() -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://10.10.0.220:8000/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "Remote Cave".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

/// Build an AppState with an OPEN, STAMPED-REMOTE project: a temp `.stalx` saved
/// with one section + one shot, stamped with a speleo_link row, then opened via
/// `open_project` so `edit_enabled` flips false (remote-open-without-lock).
/// Returns `(state, section_id, shot_id, _temp)` - keep `_temp` alive.
fn remote_fixture() -> (AppState, SectionId, ShotId, TempDir) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("remote.stalx");

    let mut project = Project::new("Remote Cave".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![test_shot_data(2.0), test_shot_data(2.0)],
            ..Default::default()
        })
        .id;

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
    drop(db);

    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success, "open remote fixture");
    // open_project should have flipped edit_enabled false (link present).
    assert!(!state.edit_enabled(), "remote-open disables editing");

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;
    (state, section_id, shot_id, temp)
}

fn remote_fixture_editable() -> (AppState, SectionId, ShotId, TempDir) {
    let (state, sid, shid, temp) = remote_fixture();
    state.set_edit_enabled(true);
    (state, sid, shid, temp)
}

#[test]
fn unwound_pushes_require_an_explicit_backend_acknowledgement() {
    let (state, _sid, _shid, _temp) = remote_fixture_editable();
    state.mark_remote_dirty();
    state.speleodb_set_unwound_from(Some("abc123"));

    let refused = state
        .speleodb_push_only_acknowledged(None, false)
        .expect_err("unacknowledged older-version push must fail closed");
    assert!(
        refused.message.contains("older unwound revision"),
        "unexpected refusal: {refused:?}"
    );

    let acknowledged = state
        .speleodb_push_only_acknowledged(None, true)
        .expect_err("fixture is deliberately signed out");
    assert!(
        !acknowledged.message.contains("older unwound revision"),
        "the explicit acknowledgement must pass the older-version guard"
    );
}

/// Read has_unpushed_edits straight from the open database.
fn dirty_bit(state: &AppState) -> bool {
    let guard = state.database.lock().unwrap();
    guard
        .as_ref()
        .unwrap()
        .load_speleo_link()
        .unwrap()
        .unwrap()
        .has_unpushed_edits
}

#[test]
fn edit_enabled_defaults_true_and_local_project_passes() {
    // A fresh local-only project (no speleo_link) is always editable; the gate is
    // a no-op off-SpeleoDB.
    let state = AppState::new().unwrap();
    assert!(state.edit_enabled(), "default-true");
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Local".to_string(),
                location: None,
            })
            .success
    );
    assert!(state.edit_enabled(), "create keeps edit enabled");
    let result = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#abcdef".to_string(),
        direction: None,
        shots: vec![test_shot_data(1.0)],
    });
    assert!(result.success, "local project edits succeed");
}

#[test]
fn remote_open_without_lock_disables_edit() {
    let (state, _sid, _shid, _temp) = remote_fixture();
    assert!(!state.edit_enabled(), "remote project opens read-only");
    // Closing resets the gate so the next open isn't stuck.
    assert!(state.close_project().success);
    assert!(state.edit_enabled(), "close resets the gate");
}

#[test]
fn logout_closes_edit_gate_on_remote_edit_session() {
    // #197: logging out mid-edit on a remote project must drop the edit gate (so a
    // token-less session can't keep mutating a project it can never push) - the
    // lock release is best-effort and needs no token in this offline fixture.
    //
    // Under `with_isolated_config` because logout WRITES the config (it clears the
    // persisted account email, the rehydrate seed) and now REPORTS a failure to do
    // so, so the write needs a config file of its own to land in.
    crate::test_helpers::with_isolated_config(|| {
        let (state, _sid, _shid, _temp) = remote_fixture_editable();
        assert!(
            state.edit_enabled(),
            "precondition: remote edit session, gate on"
        );
        state.speleodb_logout().expect("logout");
        assert!(
            !state.edit_enabled(),
            "logout must drop the edit gate on a remote edit session"
        );
    });
}

#[test]
fn logout_leaves_local_project_editable() {
    // Logout must NOT touch a local-only project's gate (no remote link → no lock).
    //
    // Isolated for the same reason as `logout_closes_edit_gate_on_remote_edit_session`:
    // logout writes the config to clear the rehydrate seed and now reports a failure
    // to do so, so it needs a config file of its own.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        assert!(
            state
                .execute(Command::CreateProject {
                    name: "Local".to_string(),
                    location: None,
                })
                .success
        );
        assert!(state.edit_enabled(), "local project editable");
        state.speleodb_logout().expect("logout");
        assert!(
            state.edit_enabled(),
            "logout must not disable editing on a local-only project"
        );
    });
}

#[test]
fn done_without_edit_session_is_noop_not_a_release() {
    // #197 review: Done on a remote project opened READ-ONLY (never Started editing →
    // edit_enabled false, no lock held this session) must NOT POST /release/ for a
    // mutex it never acquired. It short-circuits with no server call - the offline
    // fixture has no live endpoint, so reaching active_token/release would error; the
    // `Ok` proves the guard fired before any network work.
    //
    // UPDATED: the arm was `Released`. That conflated "your edits are on the server,
    // the lock is gone" with "there was no edit session at all", and the two diverge
    // whenever the gate is closed over a project that is still dirty (a 401 auto-drop
    // closes the gate and deliberately keeps the dirty bit). The frontend clears its
    // `hasUnpushedEdits` on `Released`, which disarmed the quit guard over work
    // nothing had uploaded - so the no-session case now has its own arm.
    let (state, _sid, _shid, _temp) = remote_fixture();
    assert!(
        !state.edit_enabled(),
        "precondition: remote read-only, no lock held"
    );
    let done = state
        .speleodb_done(None)
        .expect("non-dirty no-session Done is a no-op, not an error");
    assert!(
        matches!(done, crate::speleodb::models::DoneResult::NotEditing),
        "Done without an edit session reports NotEditing, not a release, got {done:?}"
    );
    assert!(
        !state.edit_enabled(),
        "still read-only after the no-op Done"
    );
}

/// The chokepoint enumeration tripwire (D-B6): every SURVEY-CONTENT mutation entry
/// listed in MUTATION_ENTRYPOINTS must reject on a read-only remote project with the
/// EDIT_DISABLED code (or, for repair, changed == false).
///
/// CAVEAT - this list is hand-maintained and only checks its own length, so it
/// catches a forgotten guard on a KNOWN entry, NOT a brand-new `execute_*`/
/// `set_*_for_path` method added without a `guard_mutation` call (that would compile
/// and pass green). The exhaustive `is_lifecycle_passthrough` match closes the hole
/// for new `Command` *variants*; a source-scan test asserting every gated method body
/// contains `guard_mutation` would close it for new *methods* (follow-up). And note
/// the deliberately-ungated local-only writes documented on `guard_mutation`
/// (display settings, import-dedup) are NOT in this list - but warning
/// resolve/unresolve ARE gated (#197) and appear at the end.
const MUTATION_ENTRYPOINTS: &[&str] = &[
    // execute() + a representative spread of non-lifecycle Command variants. The
    // guard runs BEFORE the variant match, so one code path gates them all - but
    // probing the GEOMETRY variants the move/drag tools use (MoveStation/Segment/
    // Section/Batch) guards against a future misclassification into the
    // is_lifecycle_passthrough arm, which the lone UpdateShot probe would NOT
    // catch. That gap sat behind the move-shot read-only "shots vanish" report.
    "execute",
    "execute:MoveStation",
    "execute:MoveSegment",
    "execute:MoveSection",
    "execute:Batch",
    // ReverseSection renumbers stations and is newly reachable from three menus (#24), so it
    // is exactly the shape the geometry probes above exist for. Both the bare command and the
    // Batch wrapper the multi-select path uses.
    "execute:ReverseSection",
    "execute:Batch:ReverseSection",
    "execute:MoveComment",
    // Add .stalx (#674) sits next to MergeProjects, which IS a lifecycle
    // pass-through; this probe catches it being filed there by analogy.
    "execute:AddProjectFile",
    // history entrypoints
    "undo",
    "redo",
    "undo_to",
    "redo_to",
    // direct-lock bypass methods
    "update_project_settings",
    "set_surface_elevation",
    "repair_open_project_integrity",
    "execute_delete_shot_and_split",
    "execute_delete_shot_range_split",
    "execute_delete_shot_range_shift",
    "execute_delete_last_shot_and_section",
    "execute_split_shot_at_distance",
    "execute_create_and_merge",
    "execute_import_sections",
    // by-path / metadata methods (gated only when path == the OPEN project)
    "set_project_name_for_path",
    // warning resolve/unresolve (#197): gated because resolved_warnings proliferates
    // to the server (NOT stripped from the push snapshot).
    "save_resolved_warning_gated",
    "delete_resolved_warning_gated",
];

#[test]
fn gate_rejects_every_mutation_entrypoint_with_edit_disabled_code() {
    // Tripwire doc: when a new state-mutating entry is added, route it through
    // guard_mutation AND add its name to MUTATION_ENTRYPOINTS + a call below.
    assert_eq!(
        MUTATION_ENTRYPOINTS.len(),
        26,
        "update both the routing and this list when adding a mutation entry"
    );

    let assert_disabled = |label: &str, r: &CommandResult| {
        assert!(!r.success, "{label} must be rejected when edit disabled");
        assert!(
            r.error
                .as_deref()
                .is_some_and(|e| e.starts_with(EDIT_DISABLED_CODE)),
            "{label} must carry the EDIT_DISABLED code, got: {:?}",
            r.error
        );
    };

    // --- execute() and its in-execute arms ---
    {
        let (state, sid, shid, _temp) = remote_fixture();
        assert_disabled(
            "execute",
            &state.execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates::default(),
            }),
        );
        // Geometry variants (the move/drag tools) - explicitly gated so a future
        // reclassification into is_lifecycle_passthrough can't silently reopen the
        // move-shot read-only hole.
        assert_disabled(
            "execute:MoveStation",
            &state.execute(Command::MoveStation {
                station_ref: StationRef::new(sid, 1),
                offset: Coordinates::new(1.0, 2.0, 0.0),
            }),
        );
        // A per-shot credit edit is metadata, but still an edit of the project.
        assert_disabled(
            "execute:EditShotCredit",
            &state.execute(Command::EditShotCredit {
                targets: crate::models::CreditTargets::Section { section_id: sid },
                edit: crate::models::CreditEdit {
                    set_date: Some(None),
                    ..Default::default()
                },
            }),
        );
        assert_disabled(
            "execute:ReverseSection",
            &state.execute(Command::ReverseSection {
                section_id: sid,
                swap_left_right: false,
            }),
        );
        assert_disabled(
            "execute:Batch:ReverseSection",
            &state.execute(Command::Batch {
                commands: vec![Command::ReverseSection {
                    section_id: sid,
                    swap_left_right: false,
                }],
            }),
        );
        assert_disabled(
            "execute:MoveSegment",
            &state.execute(Command::MoveSegment {
                section_id: sid,
                offset: Coordinates::new(1.0, 2.0, 0.0),
            }),
        );
        assert_disabled(
            "execute:MoveSection",
            &state.execute(Command::MoveSection {
                section_id: sid,
                offset: Coordinates::new(1.0, 2.0, 0.0),
            }),
        );
        assert_disabled(
            "execute:Batch",
            &state.execute(Command::Batch {
                commands: vec![Command::MoveStation {
                    station_ref: StationRef::new(sid, 1),
                    offset: Coordinates::new(1.0, 2.0, 0.0),
                }],
            }),
        );
        assert_disabled(
            "execute:MoveComment",
            &state.execute(Command::MoveComment {
                source: crate::core::CommentHolder::Shot {
                    section_id: sid,
                    shot_id: shid,
                },
                target: crate::core::CommentHolder::Shot {
                    section_id: sid,
                    shot_id: shid,
                },
                swap: false,
            }),
        );
        // The gate runs before the file is even read, so a path that does not
        // exist still has to come back EDIT_DISABLED, not "file not found".
        assert_disabled(
            "execute:AddProjectFile",
            &state.execute(Command::add_project_file(PathBuf::from(
                "/nonexistent/added.stalx",
            ))),
        );
    }

    // --- history entrypoints ---
    {
        let (state, _sid, _shid, _temp) = remote_fixture();
        assert_disabled("undo", &state.undo());
        assert_disabled("redo", &state.redo());
        assert_disabled("undo_to", &state.undo_to(0));
        assert_disabled("redo_to", &state.redo_to(0));
    }

    // --- direct-lock bypass methods ---
    {
        let (state, sid, shid, _temp) = remote_fixture();
        assert_disabled(
            "update_project_settings",
            &state.update_project_settings("Renamed".to_string(), state.get_config().validation),
        );
        assert_disabled(
            "set_surface_elevation",
            &state.set_surface_elevation(Some(12.0)),
        );
        // repair returns IntegrityRepairResult (no coded failure surface) →
        // changed == false (D-B5).
        let repair = state.repair_open_project_integrity();
        assert!(!repair.changed, "repair on read-only remote is a no-op");
        assert_disabled(
            "execute_delete_shot_and_split",
            &state.execute_delete_shot_and_split(sid, shid, "A".to_string(), "B".to_string()),
        );
        assert_disabled(
            "execute_delete_shot_range_split",
            &state.execute_delete_shot_range_split(
                sid,
                shid,
                shid,
                "A".to_string(),
                "B".to_string(),
            ),
        );
        assert_disabled(
            "execute_delete_shot_range_shift",
            &state.execute_delete_shot_range_shift(sid, shid, shid),
        );
        assert_disabled(
            "execute_delete_last_shot_and_section",
            &state.execute_delete_last_shot_and_section(sid, shid),
        );
        assert_disabled(
            "execute_split_shot_at_distance",
            &state.execute_split_shot_at_distance(sid, shid, 1.0),
        );
        assert_disabled(
            "execute_create_and_merge",
            &state.execute_create_and_merge(
                Some("Imp".to_string()),
                None,
                None,
                "#00ff00".to_string(),
                None,
                vec![test_shot_data(3.0)],
                0,
                StationRef::new(sid, 1),
                ClosureStrategy::Batch,
            ),
        );
        assert_disabled(
            "execute_import_sections",
            &state.execute_import_sections(
                vec![ImportSectionConfig {
                    reverse: false,
                    section_index: 0,
                    name: Some("Imp".to_string()),
                    surveyor: None,
                    date: None,
                    color: "#00ff00".to_string(),
                    direction: None,
                    quality: 1.0,
                    shots: vec![test_shot_data(3.0)],
                    station_comments: Vec::new(),
                    station_names: Vec::new(),
                    placement: ImportPlacement::Origin,
                    azimuth_reference: None,
                    explorers: None,
                    override_date: None,
                    surveyors: None,
                }],
                Vec::new(),
                None,
                Vec::new(),
                Vec::new(),
                VirtualShotMode::Auto,
            ),
        );
    }

    // --- by-path metadata methods ---
    {
        let (state, _sid, _shid, _temp) = remote_fixture();
        // The by-path methods are gated only when path == the OPEN project.
        let open_path = state.get_project_state().unwrap().file_path.unwrap();
        assert_disabled(
            "set_project_name_for_path",
            &state.set_project_name_for_path(open_path, "Renamed Remote".to_string()),
        );
    }

    // --- warning resolve/unresolve (#197) ---
    // These return Result<(), String> (not CommandResult); the EDIT_DISABLED code is
    // carried as the String error prefix, exactly as the frontend's read-only handler
    // pattern-matches it.
    {
        let (state, sid, shid, _temp) = remote_fixture();
        let warning = crate::persistence::ResolvedWarning {
            section_id: sid,
            shot_id: shid,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        };
        let resolve_err = state
            .save_resolved_warning_gated(&warning)
            .expect_err("resolve must be rejected when edit disabled");
        assert!(
            resolve_err.starts_with(EDIT_DISABLED_CODE),
            "save_resolved_warning_gated must carry the EDIT_DISABLED code, got: {resolve_err:?}"
        );
        let unresolve_err = state
            .delete_resolved_warning_gated(sid, shid, "SHORT_SHOT", None)
            .expect_err("unresolve must be rejected when edit disabled");
        assert!(
            unresolve_err.starts_with(EDIT_DISABLED_CODE),
            "delete_resolved_warning_gated must carry the EDIT_DISABLED code, got: {unresolve_err:?}"
        );
        // The gate fires BEFORE any DB write, so the dirty bit must NOT have flipped.
        assert!(
            !dirty_bit(&state),
            "a rejected resolve/unresolve must not dirty the project"
        );
    }
}

#[test]
fn lifecycle_commands_pass_through_gate() {
    // Save / Close must NOT be blocked on a read-only remote project (autosave
    // must keep working). Open/Create/MergeProjects are lifecycle too.
    let (state, _sid, _shid, _temp) = remote_fixture();
    assert!(!state.edit_enabled());

    // Autosave (SaveProject) passes the gate.
    let save = state.execute(Command::SaveProject { path: None });
    assert!(save.success, "autosave must pass through the gate");

    // The dirty bit is NOT set by a local save.
    assert!(!dirty_bit(&state), "save/autosave never sets the dirty bit");

    // Close passes through (and resets the gate).
    assert!(state.close_project().success);
}

#[test]
fn warning_resolve_unresolve_in_edit_session_dirty_for_push() {
    // #197 data-safety: resolved_warnings PROLIFERATES to the server (it is NOT in
    // PUSH_SNAPSHOT_LOCAL_TABLES), so a resolve/unresolve made during an edit session
    // MUST mark the project dirty - otherwise Done (gated on the dirty bit) skips the
    // push and the change is silently dropped on the next pull. This guards the
    // `mark_remote_dirty()` calls in the gated wrappers against a future drop. (The
    // read-only REJECTION path is covered by
    // gate_rejects_every_mutation_entrypoint_with_edit_disabled_code.)
    let make_warning = |sid, shid| crate::persistence::ResolvedWarning {
        section_id: sid,
        shot_id: shid,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    };

    // resolve in an edit session → dirty
    {
        let (state, sid, shid, _temp) = remote_fixture_editable();
        assert!(!dirty_bit(&state), "fixture starts clean");
        state
            .save_resolved_warning_gated(&make_warning(sid, shid))
            .expect("resolve allowed in an edit session");
        assert!(
            dirty_bit(&state),
            "an edit-session resolve must dirty the project so Done pushes it"
        );
    }

    // unresolve in an edit session → dirty; and the low-level accessor stays UNGATED
    {
        let (state, sid, shid, _temp) = remote_fixture_editable();
        // Seed via the low-level accessor - it must NOT dirty (only the gated wrapper
        // does), which also pins that the backfill/test path is exempt by design.
        state
            .save_resolved_warning(&make_warning(sid, shid))
            .expect("seed a resolved row");
        assert!(
            !dirty_bit(&state),
            "the ungated low-level save_resolved_warning must not dirty the project"
        );
        state
            .delete_resolved_warning_gated(sid, shid, "SHORT_SHOT", None)
            .expect("unresolve allowed in an edit session");
        assert!(
            dirty_bit(&state),
            "an edit-session unresolve must dirty the project so Done pushes it"
        );
    }
}

#[test]
fn dirty_bit_flips_false_to_true_on_successful_remote_edit() {
    let (state, sid, shid, _temp) = remote_fixture_editable();
    assert!(!dirty_bit(&state), "starts clean");

    let r = state.execute(Command::UpdateShot {
        section_id: sid,
        shot_id: shid,
        updates: ShotUpdates {
            length: Some(9.0),
            ..Default::default()
        },
    });
    assert!(r.success);
    assert!(
        dirty_bit(&state),
        "successful remote edit flips the dirty bit"
    );
}

#[test]
fn dirty_bit_is_idempotent_and_set_only() {
    let (state, sid, shid, _temp) = remote_fixture_editable();

    // First edit sets it.
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(dirty_bit(&state));

    // A second edit + a local autosave both leave it set (idempotent; set-only -
    // a local save NEVER clears it).
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(8.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(dirty_bit(&state), "dirty bit stays set after autosave");
}

#[test]
fn dirty_bit_noop_for_local_only_project() {
    // A local-only project has no speleo_link → mark_remote_dirty is a no-op and
    // never errors.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("local.stalx");
    let mut project = Project::new("Local".to_string(), None);
    let sid = project
        .add_section(SectionData {
            shots: vec![test_shot_data(2.0)],
            ..Default::default()
        })
        .id;
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);

    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success);
    assert!(state.edit_enabled(), "local project stays editable");

    let shid = state.get_project_state().unwrap().sections[0].shots[0].id;
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(5.0),
                    ..Default::default()
                },
            })
            .success
    );
    // No speleo_link row exists; mark_remote_dirty was a silent no-op.
    let guard = state.database.lock().unwrap();
    assert!(guard
        .as_ref()
        .unwrap()
        .load_speleo_link()
        .unwrap()
        .is_none());
}

// ===========================================================================
// Save a Copy: an explicit destination on a LINKED project writes a detached
// unlinked copy and leaves the session on the managed file (see save_project).
// ===========================================================================

#[test]
fn save_as_on_a_linked_project_writes_an_unlinked_copy_and_stays_put() {
    let (state, _sid, _shid, temp) = remote_fixture();
    let open_path = state.open_project_path().unwrap();
    let copy_path = temp.path().join("copy.stalx");

    let result = state.save_project(Some(copy_path.clone()));

    assert!(result.success, "copy save succeeds: {:?}", result.error);
    assert_eq!(
        result.unlinked_copy_path.as_deref(),
        Some(copy_path.display().to_string().as_str()),
        "the copy is reported via unlinked_copy_path"
    );
    assert!(
        result.saved_path.is_none(),
        "saved_path must stay None - the frontend re-points file_path on it"
    );
    assert_eq!(
        state.open_project_path().unwrap(),
        open_path,
        "the session stays on the managed file"
    );
    assert!(
        state.speleodb_open_link().is_ok(),
        "the managed file keeps its link"
    );

    let copy_db = Database::open_for_metadata(&copy_path).unwrap();
    assert!(
        copy_db.load_speleo_link().unwrap().is_none(),
        "the copy carries NO speleo_link row"
    );
    drop(copy_db);
    let copy_db = Database::open_existing(&copy_path).unwrap();
    let copied = copy_db.load_project().unwrap();
    assert_eq!(copied.sections.len(), 1, "the survey data is in the copy");
    assert_eq!(copied.sections[0].shots.len(), 2);
}

#[test]
fn save_a_copy_onto_the_managed_file_itself_is_refused() {
    let (state, _sid, _shid, _temp) = remote_fixture();
    let open_path = state.open_project_path().unwrap();

    let result = state.save_project(Some(open_path.clone()));

    assert!(
        !result.success,
        "writing an unlinked copy over the managed file would strip its link in place"
    );
    let db = Database::open_for_metadata(&open_path).unwrap();
    assert!(
        db.load_speleo_link().unwrap().is_some(),
        "the refusal must not have touched the file"
    );
}

#[test]
fn implicit_save_on_a_linked_project_still_carries_the_link_and_the_dirty_bit() {
    // Pins speleodb_upload's pre-push flush (`save_project(None)`): an implicit
    // save is NOT a copy save - it must keep the link and the dirty bit.
    let (state, sid, shid, _temp) = remote_fixture_editable();
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(dirty_bit(&state), "the edit set the dirty bit");

    let result = state.save_project(None);

    assert!(result.success);
    assert!(
        result.saved_path.is_some(),
        "an implicit save reports saved_path"
    );
    assert!(
        result.unlinked_copy_path.is_none(),
        "an implicit save is never a copy save"
    );
    assert!(
        state.speleodb_open_link().is_ok(),
        "the link survived the save"
    );
    assert!(
        dirty_bit(&state),
        "a local save never clears the dirty bit - only a push does"
    );
}

#[test]
fn save_as_on_an_unlinked_project_still_relocates_the_session() {
    // The pre-existing Save As contract for plain local projects, pinned so the
    // copy_only branch cannot leak into it.
    let temp = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Local".to_string(),
                location: None,
            })
            .success
    );

    let dest = temp.path().join("relocated.stalx");
    let result = state.save_project(Some(dest.clone()));

    assert!(result.success);
    assert_eq!(
        result.saved_path.as_deref(),
        Some(dest.display().to_string().as_str()),
        "an unlinked Save As reports saved_path"
    );
    assert!(result.unlinked_copy_path.is_none());
    assert_eq!(
        state.open_project_path().unwrap(),
        dest,
        "an unlinked Save As relocates the session"
    );
}

#[test]
fn the_copy_carries_the_undo_history() {
    let (state, sid, shid, temp) = remote_fixture_editable();
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success
    );

    let copy_path = temp.path().join("copy.stalx");
    assert!(state.save_project(Some(copy_path.clone())).success);

    let copy_db = Database::open_existing(&copy_path).unwrap();
    assert!(
        !copy_db
            .load_undo_stack_with_timestamps()
            .unwrap()
            .is_empty(),
        "the copy carries the undo history (the detach flow reopens it and \
         must not lose the edit trail)"
    );
}

#[test]
fn edit_disabled_code_prefix_is_distinct() {
    // The EDIT_DISABLED code is distinct from the FS permission code, so the FE
    // can pattern-match it independently (#204).
    assert_eq!(EDIT_DISABLED_CODE, "EDIT_DISABLED");
    assert_ne!(
        EDIT_DISABLED_CODE,
        crate::core::loading::PERMISSION_DENIED_CODE
    );
    let (state, sid, shid, _temp) = remote_fixture();
    let err = state
        .execute(Command::UpdateShot {
            section_id: sid,
            shot_id: shid,
            updates: ShotUpdates::default(),
        })
        .error
        .unwrap();
    assert!(err.starts_with(EDIT_DISABLED_CODE));
    assert!(
        !err.starts_with(crate::core::loading::PERMISSION_DENIED_CODE),
        "must not collide with the permission-denied prefix"
    );
}

#[test]
fn repair_and_settings_gated_on_locked_remote() {
    let (state, _sid, _shid, _temp) = remote_fixture();
    // Repair: returns changed == false (no coded-failure surface; D-B5).
    let repair = state.repair_open_project_integrity();
    assert!(!repair.changed);
    assert_eq!(repair.fixed, 0);
    // Settings: returns the EDIT_DISABLED coded failure.
    let settings =
        state.update_project_settings("New Name".to_string(), state.get_config().validation);
    assert!(!settings.success);
    assert!(settings
        .error
        .as_deref()
        .is_some_and(|e| e.starts_with(EDIT_DISABLED_CODE)));
}

// === Wedge healing: entries `undo()` cannot apply (Q8) ===

/// A stack persisted before the forward guard existed still carries the poison on
/// disk, and `load_from_persisted_with_timestamps` used to restore it verbatim:
/// `undo()` pops the `UndoData::None` entry, maps it to a failure, and the epilogue
/// pushes it straight back, so every older entry is unreachable forever.
#[test]
fn a_persisted_none_entry_does_not_block_the_history_beneath_it() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Wedged".to_string(),
        location: None,
    });
    for name in ["First", "Second"] {
        let created = state.execute(Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        });
        assert!(created.success);
    }

    // Serialize the live stack, then splice the poison on top - exactly the shape
    // a pre-guard build wrote to the database.
    let mut persisted: Vec<(String, String, Option<String>)> = {
        let stack = state.undo_stack.lock().unwrap();
        assert_eq!(stack.len(), 2);
        stack.rows_to_persist(PersistStack::Undo, Some(20))
    };
    let poison_timestamp = persisted
        .last()
        .and_then(|(_, _, timestamp)| timestamp.as_deref())
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc) + chrono::Duration::nanoseconds(1));
    persisted.push(
        UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            command_bytes: None,
            command: Command::ReverseSection {
                section_id: 1,
                swap_left_right: false,
            },
            undo_data: UndoData::None,
            timestamp: poison_timestamp,
        })
        .unwrap(),
    );

    // Restore it the way `project_load.rs` does.
    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.load_from_persisted_with_timestamps(persisted, vec![]);
    }

    let first = state.undo();
    assert!(
        first.success,
        "undo blocked by the persisted unundoable entry: {:?}",
        first.error
    );
    let second = state.undo();
    assert!(
        second.success,
        "history beneath the persisted unundoable entry was unreachable: {:?}",
        second.error
    );

    let project = state.get_project_state().unwrap();
    assert!(
        project.sections.is_empty(),
        "both sections should be undone, still have: {:?}",
        project.sections.iter().map(|s| s.id).collect::<Vec<_>>()
    );
}

/// The redo stack is deliberately NOT filtered - a `UndoData::None` redo entry
/// re-executes its command correctly. But the redo epilogue must not then move it
/// onto the undo stack, or the wedge is recreated at runtime.
#[test]
fn redoing_an_unundoable_entry_does_not_re_wedge_the_undo_stack() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Redo Wedge".to_string(),
        location: None,
    });

    // A shot-less (waypoint-style) section, so the redone reverse is a no-op.
    let waypoint = state.execute(Command::CreateSection {
        name: Some("Waypoint".to_string()),
        surveyor: None,
        date: None,
        color: "#ff6b6b".to_string(),
        direction: None,
        shots: Vec::new(),
    });
    let waypoint_id = waypoint.section_id.unwrap();

    let real = state.execute(Command::CreateSection {
        name: Some("Real".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    let real_id = real.section_id.unwrap();

    // The kind of entry a pre-guard build could leave on the persisted redo stack.
    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: Command::ReverseSection {
                section_id: waypoint_id,
                swap_left_right: false,
            },
            undo_data: UndoData::None,
            timestamp: None,
        });
    }

    let redone = state.redo();
    assert!(redone.success, "redo failed: {:?}", redone.error);

    let undone = state.undo();
    assert!(
        undone.success,
        "the redone unundoable entry wedged the undo stack: {:?}",
        undone.error
    );
    let project = state.get_project_state().unwrap();
    assert!(
        !project.sections.iter().any(|s| s.id == real_id),
        "undo should have reached the real entry beneath"
    );
}

/// `UnmergeStations` on a station that is in no merge returns success
/// unconditionally while `generate_undo_data` yields `UndoData::None`, and it is
/// classified as affecting geometry - so the epilogue reconciles derived state with
/// no undo entry pushed. The redo branch replays onto a state the project has left,
/// so it must be cleared even though nothing was pushed.
#[test]
fn a_no_undo_data_command_still_clears_the_redo_branch() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Redo Clear".to_string(),
        location: None,
    });

    let mut ids: Vec<SectionId> = Vec::new();
    for name in ["A", "B", "C"] {
        let created = state.execute(Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![test_shot_data(5.0)],
        });
        assert!(created.success);
        ids.push(created.section_id.unwrap());
    }

    assert!(state.undo().success);
    assert!(state.undo().success);
    assert!(state.can_redo(), "two undos should leave a redo branch");

    // Section A survived both undos, and A:0 is in no merge.
    let unmerge = state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(ids[0], 0),
    });
    assert!(unmerge.success, "unmerge failed: {:?}", unmerge.error);

    assert!(
        !state.can_redo(),
        "a successful forward command must invalidate the redo branch even when it \
         produced no undo entry"
    );
}

/// A shot-less `ReverseSection` must NOT drop the redo branch (#24).
///
/// "A successful forward command invalidates redo" is the right rule for a command that did
/// something - including `UnmergeStations` on an unmerged station, which reconciles derived
/// state (see the test above). A reverse of a section with no shots is different in kind:
/// `Project::reverse_section` returns `true` without touching a field, so there is no state for
/// the redo branch to have diverged from. Dropping it - and stamping `has_unpushed_edits`, which
/// proliferates a phantom edit to SpeleoDB - was pure loss.
///
/// Guarded in the backend, not the dialog: the batch path, the raw IPC command and the agent
/// harness all reach `execute` without passing the dialog's filter.
#[test]
fn a_shotless_reverse_leaves_the_redo_branch_intact() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "No-op Reverse".to_string(),
        location: None,
    });

    let waypoint = state.execute(Command::CreateSection {
        name: Some("Waypoint".to_string()),
        surveyor: None,
        date: None,
        color: "#ff6b6b".to_string(),
        direction: None,
        shots: Vec::new(),
    });
    let waypoint_id = waypoint.section_id.unwrap();

    let real = state.execute(Command::CreateSection {
        name: Some("Real".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![test_shot_data(5.0)],
    });
    assert!(real.success);

    assert!(state.undo().success);
    assert!(state.can_redo());

    let reversed = state.execute(Command::ReverseSection {
        section_id: waypoint_id,
        swap_left_right: false,
    });
    assert!(reversed.success, "reverse failed: {:?}", reversed.error);

    assert!(
        state.can_redo(),
        "a provable no-op must leave the redo branch alone"
    );
}

/// `rows_to_persist` copies the stack verbatim, so an unfiltered load would
/// write the poison straight back to disk on the next save.
#[test]
fn a_loaded_none_entry_is_not_re_persisted() {
    let state = AppState::new().unwrap();

    let persisted: Vec<(String, String, Option<String>)> = vec![
        UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            command_bytes: None,
            command: Command::DeleteSection { section_id: 7 },
            undo_data: UndoData::InverseCommand(Command::DeleteSection { section_id: 7 }),
            timestamp: None,
        })
        .unwrap(),
        UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            command_bytes: None,
            command: Command::ReverseSection {
                section_id: 1,
                swap_left_right: false,
            },
            undo_data: UndoData::None,
            timestamp: None,
        })
        .unwrap(),
    ];

    let mut stack = state.undo_stack.lock().unwrap();
    stack.load_from_persisted_with_timestamps(persisted, vec![]);

    let to_persist = stack.rows_to_persist(PersistStack::Undo, Some(20));
    assert_eq!(to_persist.len(), 1);
    // The rows are already serialized, and a `UndoData::None` is the unit variant, so
    // it reaches the database as the bare JSON string `"None"`.
    assert!(
        !to_persist
            .iter()
            .any(|(_, undo_data_json, _)| undo_data_json == r#""None""#),
        "the poison must not be written back to disk"
    );
}

/// The load-time filter must not weaken the deliberate push-back-on-failure
/// contract: an entry that CAN be applied but fails while applying it stays on the
/// undo stack. `PreviousValues` with unparseable JSON is such an entry, and it is
/// fed through the same load path so the filter is provably in the way.
#[test]
fn a_failed_undo_still_pushes_its_entry_back() {
    let state = AppState::new().unwrap();

    let persisted: Vec<(String, String, Option<String>)> =
        vec![UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            command_bytes: None,
            command: Command::UpdateShot {
                section_id: 1,
                shot_id: 1,
                updates: ShotUpdates::default(),
            },
            undo_data: UndoData::PreviousValues {
                section_id: 1,
                shot_id: Some(1),
                shot_uuid: None,
                values_json: "not-valid-json".to_string(),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        })
        .unwrap()];

    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.load_from_persisted_with_timestamps(persisted, vec![]);
        assert_eq!(stack.len(), 1, "an applicable entry must survive the load");
    }

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to parse undo data"));

    let stack = state.undo_stack.lock().unwrap();
    assert_eq!(stack.len(), 1, "a failed undo must preserve its entry");
    assert!(
        !stack.can_redo(),
        "the entry must not move to the redo stack"
    );
}

// ========================================================================
// `AppState::update_config` normalization tests (#357 item 1:
// `normalize_except_shortcuts()` now runs on the whole-`Config` IPC write
// path, which previously normalized nothing).
// ========================================================================

#[test]
fn test_update_config_repairs_a_degenerate_whole_config_payload() {
    // Guard A: a whole-`Config` payload carrying junk in several unrelated
    // sections must be repaired on the way in, exactly as the load path
    // repairs a hand-edited config.toml - not persisted as-is and left for
    // the NEXT launch to quietly fix.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        let mut payload = state.get_config();
        payload.panels.sections_panel_width = 5; // out of the 150..400 bound
        payload.placement_suggestion.azimuth_sigma = f64::NAN;
        payload.general.locale = "   ".to_string(); // blank after trim

        state.update_config(payload).expect("settings saved");

        let after = state.get_config();
        assert_eq!(
            after.panels.sections_panel_width, 150,
            "an out-of-range panel width must be clamped on write, not just on the next load"
        );
        assert_eq!(
            after.placement_suggestion.azimuth_sigma,
            crate::config::default_ps_azimuth_sigma(),
            "a NaN sigma must be repaired on write"
        );
        assert_eq!(
            after.general.locale, "system",
            "a blank locale must be repaired on write to the default (\"system\")"
        );
    });
}

#[test]
fn test_update_config_leaves_an_explicitly_cleared_shortcut_empty() {
    // Guard B - the important one: `normalize_shortcut_field` reads an EMPTY
    // binding list as "key missing from the file" and re-seeds the stock
    // binding, which is right on LOAD and wrong on WRITE (the Settings UI
    // sends an empty array when the user clears the last binding for an
    // action - ShortcutsSettings.tsx). `update_config` must go through
    // `normalize_except_shortcuts`, not the full `normalize`, so a
    // deliberately cleared shortcut survives the write path.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        let mut payload = state.get_config();
        assert!(
            !payload.shortcuts.select.is_empty(),
            "test assumes the stock binding is non-empty by default"
        );
        payload.shortcuts.select = Vec::new();

        state.update_config(payload).expect("settings saved");

        let after = state.get_config();
        assert!(
            after.shortcuts.select.is_empty(),
            "a shortcut the user explicitly cleared must not be re-defaulted by a settings save"
        );
    });

    // The other direction, in the same test so the distinction stays pinned:
    // the LOAD path's plain `normalize()` DOES re-seed an empty binding, since
    // there an empty list can only mean "missing from the file".
    let mut loaded = crate::config::Config::default();
    loaded.shortcuts.select = Vec::new();
    loaded.normalize();
    assert_eq!(
        loaded.shortcuts.select,
        crate::config::Config::default().shortcuts.select,
        "the load path must still re-seed an empty binding read from config.toml"
    );
}

// === #413 finding 26: a shot edit must not re-georeference the survey ===

/// FINDING 26's reduction: removing the FIRST zero datum re-defines the world
/// frame, and the next geometry edit in a promoted group silently translates the
/// whole rigid group onto the new frame - unundoably.
///
/// `zero_datums[0]` IS the origin: `Project::set_zero_datum` places every later
/// datum at `gps_to_local(its gps, first.gps)`. `Project::remove_zero_datum` is
/// a bare `retain`, so dropping the first one promotes the next and moves
/// nothing - `resolve_offsets` cannot repair it either, because the cascade
/// marks datum sections AUTHORITATIVE and never re-derives their offsets. The
/// promoted group is then sitting kilometres from the position its own datum
/// implies, and nothing notices until the next shot edit, when
/// `realign_after_shot_edit` re-derives the ABSOLUTE target and yanks the group
/// onto it. That happens inside `update_shot`, whose `UndoData::PreviousValues`
/// carries the shot's fields and no offsets, so the undo cannot put the survey
/// back: it restores the shot, re-enters the same realign, and re-anchors to the
/// origin a second time.
///
/// This is the horizon-mode component-translation observation at 20 lines: the
/// fuzzer reached it at seed 2 depth 1189 (`HORIZON=1 SEEDS=2 ITERATIONS=2000`),
/// where sections 65 and 81 were both off by `(22230.09, -10480.74)` with 81 on
/// the origin. Four earlier synthetic reductions missed it because none of them
/// removed the FIRST datum, which is the whole mechanism.
///
/// Both halves are asserted: the forward edit must not move the group, and the
/// undo must restore every offset exactly.
#[test]
fn finding_26_a_shot_edit_does_not_translate_a_group_whose_first_datum_was_removed() {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "datum frame".to_string(),
                location: None,
            })
            .success
    );

    let section = |name: &str| {
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#111111".to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![test_shot_data(5.0), test_shot_data(5.0)],
            })
            .section_id
            .unwrap()
    };
    let anchor = section("anchor");
    let a = section("a");
    let b = section("b");

    // {a, b} is one rigid group.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 2),
                station_b: StationRef::new(b, 0),
            })
            .success
    );

    // The FIRST datum defines the world origin.
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(anchor, 0),
                coordinates: GpsCoordinates::new(20.30, -87.45, 0.0).unwrap(),
            })
            .success
    );
    // The SECOND datum places {a, b} at gps_to_local(G2, G1) - kilometres out.
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(20.20, -87.24, 0.0).unwrap(),
            })
            .success
    );
    let placed = state.get_project_state().unwrap().section_offset(a);
    assert!(
        placed.0.abs() > 1000.0,
        "the second datum should place the group kilometres out: {placed:?}"
    );

    // Promote a's datum to first WITHOUT moving anything.
    assert!(
        state
            .execute(Command::RemoveZeroDatum {
                station_ref: StationRef::new(anchor, 0),
            })
            .success
    );

    // The promotion is no longer silent: local (0, 0) means `zero_datums[0]`'s
    // GPS, so the removal translates the survey onto the new origin and a's datum
    // station - the new `zero_datums[0]` - sits there (#469, item 1). Everything
    // below is measured against that state.
    let promoted = state
        .get_project_state()
        .unwrap()
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        promoted.x.abs() < 1e-6 && promoted.y.abs() < 1e-6,
        "the promoted datum's station must sit at the local origin after the removal, got \
         ({}, {})",
        promoted.x,
        promoted.y
    );

    let before = state.get_project_state().unwrap().section_offsets.clone();
    let shot_id = state
        .get_project_state()
        .unwrap()
        .get_section(a)
        .unwrap()
        .shots[1]
        .id;

    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: a,
                shot_id,
                updates: ShotUpdates {
                    length: Some(6.0),
                    ..Default::default()
                },
            })
            .success
    );

    let edited = state.get_project_state().unwrap().section_offset(a);
    let placed_before = before.get(&a).copied().unwrap_or((0.0, 0.0));
    assert!(
        (edited.0 - placed_before.0).abs() < 1.0 && (edited.1 - placed_before.1).abs() < 1.0,
        "a shot edit re-georeferenced the survey: {placed_before:?} -> {edited:?}"
    );

    assert!(state.undo().success);
    let after = state.get_project_state().unwrap().section_offsets.clone();
    for (section_id, offset) in &before {
        let now = after.get(section_id).copied().unwrap_or((0.0, 0.0));
        assert!(
            (now.0 - offset.0).abs() < 1e-6 && (now.1 - offset.1).abs() < 1e-6,
            "undo of the shot edit did not restore section {section_id}: {offset:?} -> {now:?}"
        );
    }
}

// ===========================================================================
// #413 finding 7, second half: a jump reconciles after EVERY step
// ===========================================================================

/// A shot with plain, easily-recognised geometry.
fn straight_shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    }
}

/// An `AppState` with an empty project open, ready for commands.
fn state_with_project() -> AppState {
    let state = AppState::new().unwrap();
    let created = state.execute(Command::CreateProject {
        name: "JumpEpilogue".to_string(),
        location: None,
    });
    assert!(created.success, "CreateProject failed: {:?}", created.error);
    state
}

/// Create a section with `shots` and return its id.
fn section_with_shots(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    let created = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#3388FF".to_string(),
        direction: None,
        shots,
    });
    assert!(created.success, "CreateSection failed: {:?}", created.error);
    created.section_id.expect("CreateSection returned no id")
}

/// How many merges the project currently reports as misaligned.
fn misaligned_merges(state: &AppState) -> usize {
    let guard = state.project.lock().expect("project lock poisoned");
    guard
        .as_ref()
        .expect("no project open")
        .check_merge_alignment()
        .len()
}

/// Two merged sections, the first anchored by a zero datum. Returns
/// `(anchored, partner)`.
fn two_merged_sections(state: &AppState) -> (SectionId, SectionId) {
    let a = section_with_shots(
        state,
        "A",
        vec![straight_shot(0.0, 5.0), straight_shot(0.0, 5.0)],
    );
    let b = section_with_shots(state, "B", vec![straight_shot(90.0, 5.0)]);
    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(datum.success, "SetZeroDatum failed: {:?}", datum.error);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    (a, b)
}

/// Seven metres of drift written straight into the partner's offset - two
/// orders of magnitude past the alignment invariant, and far past anything the
/// command API can leave behind in a project this small.
fn inject_drift(state: &AppState, partner: SectionId) {
    let mut guard = state.project.lock().expect("project lock poisoned");
    let project = guard.as_mut().expect("no project open");
    project.section_offsets.insert(partner, (7.0, 0.0));
    assert!(
        !project.check_merge_alignment().is_empty(),
        "setup: the injected offset should leave the merge misaligned"
    );
}

/// Two merged sections with drift injected into the partner, and then ONE
/// `MoveStation` recorded over the top of it.
///
/// `MoveStation` captures a `ProjectSnapshotPair` BEFORE it mutates, so its
/// `before` half holds the drifted project while its own epilogue heals the
/// live one. After this runs, the only way back to the drift is that restore -
/// which is exactly the shape a jump's INTERMEDIATE step has.
fn history_recorded_over_drift() -> (AppState, SectionId) {
    let state = state_with_project();
    let (anchored, partner) = two_merged_sections(&state);
    inject_drift(&state, partner);
    let moved = state.execute(Command::MoveStation {
        station_ref: StationRef::new(anchored, 1),
        offset: Coordinates {
            x: 1.0,
            y: 2.0,
            z: 0.0,
        },
    });
    assert!(moved.success, "MoveStation failed: {:?}", moved.error);
    assert_eq!(
        misaligned_merges(&state),
        0,
        "setup: the command's own epilogue should have healed the live drift"
    );
    (state, partner)
}

/// An undo entry that always fails to undo, for slipping under a good one.
fn unusable_undo_entry() -> UndoEntry {
    UndoEntry {
        command_bytes: None,
        command: Command::UpdateShot {
            section_id: 999,
            shot_id: 999,
            updates: ShotUpdates::default(),
        },
        undo_data: UndoData::PreviousValues {
            section_id: 999,
            shot_id: Some(999),
            shot_uuid: None,
            values_json: "invalid-json".to_string(),
            previous_offsets: HashMap::new(),
        },
        timestamp: None,
    }
}

/// A jump that dies half way must still leave the steps it DID apply
/// reconciled (#413, finding 7).
///
/// `undo_to` reconciled once after its loop and returned early on a failed step
/// without ever reaching it, so a jump whose first step restored a drifted
/// snapshot and whose second step failed left the user holding a survey network
/// the app's own `debug_assert_merge_alignment` calls a violation. Pressing
/// Ctrl+Z twice never could: `undo()` reconciles per press.
#[test]
fn undo_to_reconciles_after_every_step_so_a_failed_jump_leaves_no_drift() {
    let (state, _partner) = history_recorded_over_drift();

    // Slip an unusable entry UNDER the MoveStation, so the jump completes one
    // real step (which restores the drift) and then fails on the second.
    {
        let mut stack = state.undo_stack.lock().unwrap();
        let move_entry = stack.pop_undo().expect("the MoveStation entry");
        stack.push_undo(unusable_undo_entry());
        stack.push_undo(move_entry);
    }

    let result = state.undo_to(1);
    assert!(
        !result.success,
        "the second step should have failed, so the jump should report failure"
    );
    let error = result.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("Undo failed at step 2 of 2"),
        "expected the step-counted message, got: {error}"
    );
    assert_eq!(
        misaligned_merges(&state),
        0,
        "the first step restored a snapshot carrying 7 m of drift and the jump then failed: \
         reconcile_geometry has to run per step, or the state the user is left holding is one \
         no sequence of single Ctrl+Z presses can produce"
    );
}

/// The redo mirror of the test above.
///
/// The pair is built by hand because a genuine redo entry's `after` half is
/// captured AFTER the forward command's own epilogue healed the project, so
/// nothing a command records can restore drift on the way forward. What the
/// fuzzer reaches - and what a persisted entry written by an older build can
/// hold - is a snapshot that carries it.
#[test]
fn redo_to_reconciles_after_every_step_so_a_failed_jump_leaves_no_drift() {
    let state = state_with_project();
    let (_anchored, partner) = two_merged_sections(&state);
    inject_drift(&state, partner);
    let drifted = state
        .capture_project_snapshot()
        .expect("capturing the drifted project");
    state.reconcile_geometry();
    assert_eq!(
        misaligned_merges(&state),
        0,
        "setup: the reconcile should have healed the live drift"
    );
    let healed = state
        .capture_project_snapshot()
        .expect("capturing the healed project");

    {
        let mut stack = state.undo_stack.lock().unwrap();
        // Bottom: an entry that cannot be redone. Top: the drift-carrying pair.
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: Command::CloseProject,
            undo_data: UndoData::ProjectSnapshotPair {
                before: "{}".to_string(),
                after: "not-json".to_string(),
            },
            timestamp: None,
        });
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: Command::MoveStation {
                station_ref: StationRef::new(partner, 0),
                offset: Coordinates {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
            },
            undo_data: UndoData::ProjectSnapshotPair {
                before: healed,
                after: drifted,
            },
            timestamp: None,
        });
    }

    let result = state.redo_to(1);
    assert!(
        !result.success,
        "the second step should have failed, so the jump should report failure"
    );
    let error = result.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("Redo failed at step 2 of 2"),
        "expected the step-counted message, got: {error}"
    );
    assert_eq!(
        misaligned_merges(&state),
        0,
        "the first redo step restored a snapshot carrying 7 m of drift and the jump then \
         failed: redo_to has to reconcile per step exactly as redo() does per press"
    );
}

/// The contract itself, over a jump whose drifted state is an INTERMEDIATE one:
/// a jump lands where the same number of single presses lands.
///
/// Two `MoveStation`s over the injected drift means the step that restores it
/// still has a step above it. Both routes are built on their own copy of the
/// same project.
#[test]
fn undo_to_lands_where_the_same_number_of_single_undos_lands_over_a_drifted_step() {
    fn build() -> (AppState, SectionId, SectionId) {
        let state = state_with_project();
        let (anchored, partner) = two_merged_sections(&state);
        inject_drift(&state, partner);
        for step in 0..2 {
            let moved = state.execute(Command::MoveStation {
                station_ref: StationRef::new(anchored, 1),
                offset: Coordinates {
                    x: 1.0,
                    y: 2.0,
                    z: 0.0,
                },
            });
            assert!(
                moved.success,
                "MoveStation {step} failed: {:?}",
                moved.error
            );
        }
        (state, anchored, partner)
    }

    let (jumped, jumped_anchored, jumped_partner) = build();
    // index 2 = three entries: both moves and the merge under them.
    let jump = jumped.undo_to(2);
    assert!(jump.success, "undo_to(2) failed: {:?}", jump.error);

    let (stepped, stepped_anchored, stepped_partner) = build();
    for step in 0..3 {
        let single = stepped.undo();
        assert!(
            single.success,
            "single undo {step} failed: {:?}",
            single.error
        );
    }

    assert_eq!(
        (jumped_anchored, jumped_partner),
        (stepped_anchored, stepped_partner),
        "the two routes were not built on the same topology, so nothing below compares anything"
    );
    let jumped_offsets = jumped.get_project_state().unwrap().section_offsets;
    let stepped_offsets = stepped.get_project_state().unwrap().section_offsets;
    assert_eq!(
        jumped_offsets, stepped_offsets,
        "a three-step undo_to left the survey somewhere three single undos do not"
    );
    assert_eq!(
        (misaligned_merges(&jumped), misaligned_merges(&stepped)),
        (0, 0),
        "a history move left the survey network broken"
    );
}

// ===========================================================================
// #413 finding 24's shape one file over: a persisted shot id is unbounded
// ===========================================================================

/// Redoing a `CreatedShotSnapshot` whose `shot_id` is `ShotId::MAX` must not
/// overflow the section's id counter.
///
/// The id arrives verbatim from a persisted entry, and the append branch bumped
/// `next_shot_id` to `id + 1` - a panic in a debug build, a wrap in a release
/// one, after which `Section::take_shot_id` hands the wrapped value straight
/// back out as an id another shot already holds.
#[test]
fn redo_of_a_created_shot_with_the_maximum_id_saturates_instead_of_overflowing() {
    let state = state_with_project();
    let section = section_with_shots(&state, "A", vec![]);

    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: Command::CreateShot {
                section_id: section,
                shot: straight_shot(0.0, 5.0),
                after_shot_id: None,
                before_shot_id: None,
            },
            undo_data: UndoData::CreatedShotSnapshot {
                section_id: section,
                shot_id: ShotId::MAX,
                shot_data: straight_shot(0.0, 5.0),
                position: 0,
                station_shift_from: 0,
                after_shot_id: None,
                before_shot_id: None,
                after_shot_uuid: None,
                before_shot_uuid: None,
                uuid: Some("11111111-1111-4111-8111-111111111111".to_string()),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        });
    }

    // Panics with "attempt to add with overflow" before the fix.
    let redone = state.redo();
    assert!(redone.success, "redo failed: {:?}", redone.error);

    let project = state.get_project_state().unwrap();
    let restored = project.get_section(section).expect("section missing");
    assert_eq!(
        restored.shots.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![ShotId::MAX],
        "the redo should have appended the recorded shot under its recorded id"
    );
    assert_eq!(
        restored.next_shot_id,
        ShotId::MAX,
        "the counter should saturate rather than wrap past the id it must stay ahead of"
    );
}

/// The same unbounded id, on the UNDO side: `AppState::restore_shot` takes the
/// id verbatim out of a `ShotSnapshot` and bumps the section counter past it.
///
/// `ShotSnapshot` is the LEGACY delete-a-shot capture (a live `DeleteShot`
/// records a `ProjectSnapshotPair`), so every entry of this shape reaching the
/// restore came off disk, where nothing bounds `shot_id` - a `.stalx` can carry
/// `4294967295` in that column. `id + 1` panics a debug build there and wraps a
/// release one to 0, after which the section hands 0 out again as a fresh id.
#[test]
fn undo_of_a_shot_snapshot_with_the_maximum_id_saturates_instead_of_overflowing() {
    let state = state_with_project();
    let section = section_with_shots(&state, "A", vec![straight_shot(0.0, 5.0)]);

    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.push_with_data(
            Command::DeleteShot {
                section_id: section,
                shot_id: ShotId::MAX,
            },
            UndoData::ShotSnapshot {
                section_id: section,
                shot_id: ShotId::MAX,
                shot_data: straight_shot(90.0, 3.0),
                position: 1,
                uuid: Some("33333333-3333-4333-8333-333333333333".to_string()),
            },
        );
    }

    // Panics with "attempt to add with overflow" before the fix.
    let undone = state.undo();
    assert!(undone.success, "undo failed: {:?}", undone.error);

    let project = state.get_project_state().unwrap();
    let restored = project.get_section(section).expect("section missing");
    assert_eq!(
        restored.shots.len(),
        2,
        "the undo should have restored the recorded shot"
    );
    assert_eq!(
        restored.shots[1].id,
        ShotId::MAX,
        "the restored shot should keep the id the entry recorded"
    );
    assert_eq!(
        restored.next_shot_id,
        ShotId::MAX,
        "the counter should saturate rather than wrap back onto ids the section already uses"
    );
}

/// The third door the same id walks through: `Section::take_shot_id`'s FORCED
/// arm, reached when the redo of a create still has an anchor to insert against.
///
/// The append branch (the test above it) does its own bump; an anchored redo
/// goes through `insert_shot_before`/`insert_shot_after` instead, and the
/// counter bump lives in `take_shot_id`. Same unbounded persisted id, same
/// `id + 1`, so it needs its own saturation and its own regression test.
#[test]
fn an_anchored_redo_forcing_the_maximum_shot_id_saturates_instead_of_overflowing() {
    let state = state_with_project();
    let section = section_with_shots(
        &state,
        "A",
        vec![straight_shot(0.0, 5.0), straight_shot(0.0, 7.0)],
    );
    let first_id = state
        .get_project_state()
        .unwrap()
        .get_section(section)
        .expect("section missing")
        .shots[0]
        .id;

    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: Command::CreateShot {
                section_id: section,
                shot: straight_shot(180.0, 4.0),
                after_shot_id: None,
                before_shot_id: Some(first_id),
            },
            undo_data: UndoData::CreatedShotSnapshot {
                section_id: section,
                shot_id: ShotId::MAX,
                shot_data: straight_shot(180.0, 4.0),
                position: 0,
                station_shift_from: 0,
                after_shot_id: None,
                // No uuid beside it: a legacy entry, so the anchor resolves by
                // the recorded id - which is what routes the redo through
                // `insert_shot_before` rather than the append branch.
                before_shot_id: Some(first_id),
                after_shot_uuid: None,
                before_shot_uuid: None,
                uuid: Some("44444444-4444-4444-8444-444444444444".to_string()),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        });
    }

    // Panics with "attempt to add with overflow" before the fix.
    let redone = state.redo();
    assert!(redone.success, "redo failed: {:?}", redone.error);

    let project = state.get_project_state().unwrap();
    let restored = project.get_section(section).expect("section missing");
    assert_eq!(
        restored.shots.len(),
        3,
        "the redo should have inserted the recorded shot"
    );
    assert_eq!(
        restored.shots[0].id,
        ShotId::MAX,
        "the insert should have landed BEFORE the anchor, under the forced id"
    );
    assert_eq!(
        restored.next_shot_id,
        ShotId::MAX,
        "the counter should saturate rather than wrap past the id it must stay ahead of"
    );
}

// ===========================================================================
// #413 finding 1's residue: only half of a CreatedSectionSnapshot was refreshed
// ===========================================================================

/// Every shot uuid in `section_id`, in order.
fn section_shot_uuids(state: &AppState, section_id: SectionId) -> Vec<Option<String>> {
    state
        .get_project_state()
        .expect("no project open")
        .get_section(section_id)
        .expect("section missing")
        .shots
        .iter()
        .map(|shot| shot.uuid.clone())
        .collect()
}

/// Every shot length in `section_id`, in order - the cheapest way to say WHICH
/// shot each uuid ended up on.
fn section_shot_lengths(state: &AppState, section_id: SectionId) -> Vec<f64> {
    state
        .get_project_state()
        .expect("no project open")
        .get_section(section_id)
        .expect("section missing")
        .shots
        .iter()
        .map(|shot| shot.length)
        .collect()
}

/// The undo of a `CreateSection` re-reads the live section, and it has to
/// re-read BOTH halves of what it re-creates.
///
/// `Section::restore_with_identity` pairs `section_data.shots` against
/// `identity.shot_uuids` POSITIONALLY. Refreshing only the identity - which is
/// what the undo did - hands the redo the shot list as the section was CREATED
/// wearing the uuids the section holds NOW, so a section whose shots moved
/// under it (an undo-less heal, or anything else with no entry of its own)
/// comes back as the original shots carrying somebody else's identity, and any
/// uuid past the end of the shorter list is dropped outright. Those uuids are
/// what every `StationKey::AfterShot` and the Ariane export resolve by.
#[test]
fn undo_of_create_section_re_reads_the_live_shots_so_redo_keeps_uuids_on_their_own_shots() {
    let state = state_with_project();
    let section = section_with_shots(
        &state,
        "A",
        vec![straight_shot(0.0, 5.0), straight_shot(0.0, 7.0)],
    );

    // An undo-less mutation: a shot appears at the FRONT of the section with no
    // undo entry of its own, the same way a save-time heal mutates a section.
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().expect("no project open");
        let live = project.get_section_mut(section).expect("section missing");
        let id = live.next_shot_id;
        live.next_shot_id += 1;
        live.shots.insert(
            0,
            crate::models::Shot::with_uuid(
                id,
                straight_shot(180.0, 3.0),
                Some("22222222-2222-4222-8222-222222222222".to_string()),
            ),
        );
        project.reindex_section_shot_uuids(section);
    }

    let uuids_before = section_shot_uuids(&state, section);
    let lengths_before = section_shot_lengths(&state, section);
    assert_eq!(
        lengths_before,
        vec![3.0, 5.0, 7.0],
        "setup: the inserted shot should be first"
    );

    let undone = state.undo();
    assert!(undone.success, "undo failed: {:?}", undone.error);
    assert!(
        state
            .get_project_state()
            .unwrap()
            .get_section(section)
            .is_none(),
        "undo of CreateSection should have deleted the section"
    );

    let redone = state.redo();
    assert!(redone.success, "redo failed: {:?}", redone.error);

    assert_eq!(
        section_shot_lengths(&state, section),
        lengths_before,
        "the redo put back the shot list as it was CREATED, not the one the undo destroyed"
    );
    let uuids_after = section_shot_uuids(&state, section);
    assert_eq!(
        uuids_after, uuids_before,
        "a uuid landed on a different shot than the one that owned it: restore_with_identity \
         zips the recorded shots against the recorded uuids, so both halves have to come from \
         the same live section"
    );
    let unique: HashSet<Option<String>> = uuids_after.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        uuids_after.len(),
        "the restored section holds a duplicate shot identity: {uuids_after:?}"
    );
}
