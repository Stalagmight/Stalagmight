//! Creating, closing and merging projects; the load-cancel token.
//!
//! Lines 70-760 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === Project Operations Tests ===

/// [#473] Creating a project clears the PREVIOUS project's discarded-history flag.
///
/// The flag is per-project and drives a banner reading "edit history could not be read
/// and was discarded". Create is a project-slot commit like a load, and every commit has
/// to write it - the first version of this fix reset `edit_enabled` and `force_editing`
/// here and forgot this one, so opening a project with an unreadable history and then
/// choosing File > New Project put a data-loss banner over a brand-new empty project.
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
    let state = state_with_project("Doomed");
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
        shots: vec![shot(1.0)],
    });

    let mut project_b = Project::new("B".to_string(), None);
    project_b.add_section(SectionData {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 1.0,
        shots: vec![shot(2.0)],
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
        shots: vec![shot(1.0)],
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
        shots: vec![shot(2.0)],
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
    // `shot`'s 45/225 pair is deliberately antipodal (its circular
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

/// A17 (#576): a merged section keeps the least-squares gauge its own project pins.
///
/// `merge_projects` rebuilt every section through `add_section(to_section_data())`,
/// and `SectionData` is the CREATE shape: `Section::new` hardcodes
/// `reverse_parity: false` and both gauge anchors `None`. For a datum-less
/// component with an eligible closure the gauge is the one station the
/// least-squares solve holds at its measured position, so losing the pair moved
/// it to the parity default (station 0) and the whole adjusted component
/// (Closures Computed, Plan, Export) translated by the closure residual. Not only
/// for reversed sections: any captured pair naming another station moved.
///
/// The source here: one square-ish loop that misses closing by 1 m, a
/// least-squares closure from its last station back to station 0, no datum,
/// reversed and then saved (the save captures the gauge pair on the far end).
#[test]
fn test_merge_projects_keeps_each_sections_gauge_station() {
    let temp_dir = TempDir::new().unwrap();
    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let output_path = temp_dir.path().join("merged-output.stalx");
    let level = |azimuth: f64, length: f64| ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };

    let state = state_with_project("CaveA");
    let section_id = state
        .execute(Command::CreateSection {
            name: Some("Loop".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![
                level(0.0, 10.0),
                level(90.0, 10.0),
                level(180.0, 10.0),
                level(270.0, 9.0),
            ],
        })
        .section_id
        .unwrap();
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(section_id, 4),
        station_b: StationRef::new(section_id, 0),
        closure_strategy: ClosureStrategy::LeastSquaresWeighted,
    });
    assert!(closure.success, "closure: {:?}", closure.error);
    let reversed = state.execute(Command::ReverseSection {
        section_id,
        swap_left_right: false,
    });
    assert!(reversed.success, "reverse: {:?}", reversed.error);
    let saved = state.execute(Command::SaveProject {
        path: Some(source_a.clone()),
    });
    assert!(saved.success, "save: {:?}", saved.error);

    let mut project_b = Project::new("CaveB".to_string(), None);
    project_b.add_section(SectionData {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#ff6b6b".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![level(45.0, 4.0)],
    });
    let mut db_b = Database::open(&source_b).unwrap();
    db_b.save_project(&project_b).unwrap();
    drop(db_b);

    let merge = AppState::new().unwrap().execute(Command::MergeProjects {
        source_paths: vec![source_a.clone(), source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(merge.success, "merge failed: {:?}", merge.error);

    let source = Database::open_existing(&source_a)
        .unwrap()
        .load_project()
        .unwrap();
    let merged = Database::open_existing(&output_path)
        .unwrap()
        .load_project()
        .unwrap();
    let source_section = source.get_section(section_id).unwrap();
    let merged_section = merged
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Loop"))
        .expect("the loop section survives the merge");
    let source_anchor = source.adjustment_anchor_for_tests(source_section.id);
    let merged_anchor = merged.adjustment_anchor_for_tests(merged_section.id);
    println!(
        "source: reverse_parity={} gauge_shot={:?} gauge_neighbour={:?} gauge station={:?}",
        source_section.reverse_parity,
        source_section.gauge_shot_uuid,
        source_section.gauge_neighbour_uuid,
        source_anchor.map(|a| a.station_index)
    );
    println!(
        "merged: reverse_parity={} gauge_shot={:?} gauge_neighbour={:?} gauge station={:?}",
        merged_section.reverse_parity,
        merged_section.gauge_shot_uuid,
        merged_section.gauge_neighbour_uuid,
        merged_anchor.map(|a| a.station_index)
    );

    // The adjusted (Closures Computed) position of every station of the loop, in
    // both files. The merge must not move a single one.
    let config = crate::config::ClosureAdjustmentConfig::default();
    let adjusted = |project: &Project, sid: SectionId| -> Vec<(u32, Coordinates)> {
        let mut stations: Vec<_> = project
            .compute_adjusted_station_coordinates(&config)
            .into_iter()
            .filter(|s| s.station_ref.section_id == sid)
            .map(|s| (s.station_ref.station_index, s.coordinates))
            .collect();
        stations.sort_by_key(|(index, _)| *index);
        stations
    };
    let before = adjusted(&source, source_section.id);
    let after = adjusted(&merged, merged_section.id);
    assert_eq!(before.len(), after.len(), "station count changed");
    let worst_shift = before
        .iter()
        .zip(&after)
        .map(|((_, a), (_, b))| (a.x - b.x).hypot(a.y - b.y))
        .fold(0.0_f64, f64::max);
    println!("largest adjusted-position shift across the merge: {worst_shift:.3} m");

    assert!(
        source_section.reverse_parity,
        "fixture: the source is reversed"
    );
    assert!(
        source_section.gauge_shot_uuid.is_some(),
        "fixture: the save captured the gauge pair"
    );
    assert_eq!(
        merged_section.reverse_parity, source_section.reverse_parity,
        "the merge dropped reverse_parity"
    );
    assert_eq!(
        (
            &merged_section.gauge_shot_uuid,
            &merged_section.gauge_neighbour_uuid
        ),
        (
            &source_section.gauge_shot_uuid,
            &source_section.gauge_neighbour_uuid
        ),
        "the merge dropped the captured gauge pair"
    );
    assert_eq!(
        merged_anchor.map(|a| a.station_index),
        source_anchor.map(|a| a.station_index),
        "the merged component is pinned at a different station"
    );
    assert!(
        worst_shift < 1e-6,
        "the merge moved the adjusted loop by {worst_shift} m"
    );
}

/// A17 (#576): the merged project remembers every file and section its sources
/// imported, so importing one of them again still warns about the duplicate.
///
/// `merge_projects` never copied `imported_file_hashes` /
/// `imported_section_hashes`, so the merged file forgot them all. A hash both
/// sources share must land once: the tables are `UNIQUE`, and a repeat would
/// fail the whole save.
#[test]
fn test_merge_projects_unions_the_import_hashes() {
    let temp_dir = TempDir::new().unwrap();
    let source_a = temp_dir.path().join("a.stalx");
    let source_b = temp_dir.path().join("b.stalx");
    let output_path = temp_dir.path().join("merged-output.stalx");
    let strings =
        |values: &[&str]| -> Vec<String> { values.iter().map(|v| (*v).to_string()).collect() };

    let mut project_a = Project::new("CaveA".to_string(), None);
    project_a.add_section(SectionData::default());
    project_a.imported_file_hashes = strings(&["file-a", "file-shared"]);
    project_a.imported_section_hashes = strings(&["section-a"]);
    let mut project_b = Project::new("CaveB".to_string(), None);
    project_b.add_section(SectionData::default());
    project_b.imported_file_hashes = strings(&["file-b", "file-shared"]);
    project_b.imported_section_hashes = strings(&["section-b", "section-a"]);
    for (path, project) in [(&source_a, &project_a), (&source_b, &project_b)] {
        let mut db = Database::open(path).unwrap();
        db.save_project(project).unwrap();
    }

    let merge = AppState::new().unwrap().execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(merge.success, "merge failed: {:?}", merge.error);

    let merged = Database::open_existing(&output_path)
        .unwrap()
        .load_project()
        .unwrap();
    let sorted = |mut values: Vec<String>| {
        values.sort();
        values
    };
    println!(
        "merged imported_file_hashes={:?} imported_section_hashes={:?}",
        merged.imported_file_hashes, merged.imported_section_hashes
    );
    assert_eq!(
        sorted(merged.imported_file_hashes.clone()),
        strings(&["file-a", "file-b", "file-shared"]),
        "every source's imported-file hashes, each once"
    );
    assert_eq!(
        sorted(merged.imported_section_hashes),
        strings(&["section-a", "section-b"]),
        "every source's imported-section hashes, each once"
    );

    // The user-visible half: open the merged file and ask the import wizard's
    // duplicate questions.
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::OpenProject { path: output_path })
            .success
    );
    let file_b_is_duplicate = state.check_duplicate_import("file-b").unwrap();
    let sections_seen = state
        .check_section_hashes(&strings(&["section-b", "section-new"]))
        .unwrap();
    println!(
        "re-import of a source file flagged as duplicate: {file_b_is_duplicate}; \
         section hashes [section-b, section-new] seen: {sections_seen:?}"
    );
    assert!(
        file_b_is_duplicate,
        "re-importing a file a source had imported must be flagged"
    );
    assert_eq!(sections_seen, vec![true, false]);
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
    let guard = state.active_loading.lock();
    assert!(guard.as_ref().is_some());
    assert!(Arc::ptr_eq(guard.as_ref().unwrap(), &second));
    drop(guard);

    state.clear_active_load(&second);
    assert!(state.active_loading.lock().is_none());
}

#[test]
fn test_close_project_succeeds_with_poisoned_database_lock() {
    let state = state_with_project("Test");

    poison_mutex(&state.database);

    let result = state.close_project();
    assert!(result.success);
    assert!(state.get_project_state().is_none());
}
