//! Coverage gap tests: merge_projects, create_and_merge, import_sections.
//!
//! Lines 18046-19293 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// ============================================================================
// Coverage gap tests: merge_projects edge cases
// ============================================================================

#[test]
fn test_merge_projects_output_already_exists() {
    let temp_dir = TempDir::new().unwrap();

    // Create two source projects
    let mut proj_a = Project::new("A".to_string(), None);
    proj_a.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    let mut proj_b = Project::new("B".to_string(), None);
    proj_b.add_section(SectionData {
        shots: vec![shot(2.0)],
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
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    let mut proj_b = Project::new("B".to_string(), None);
    proj_b.add_section(SectionData {
        shots: vec![shot(2.0)],
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
        shots: vec![shot(1.0)],
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
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    let mut proj_b = Project::new("B".to_string(), Some("Cave Beta".to_string()));
    proj_b.add_section(SectionData {
        shots: vec![shot(2.0)],
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
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    let mut proj_b = Project::new("B".to_string(), None);
    proj_b.add_section(SectionData {
        shots: vec![shot(2.0)],
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
            shots: vec![shot(1.0), shot(1.5)],
            ..Default::default()
        })
        .id;
    let sec_a2 = proj_a
        .add_section(SectionData {
            name: Some("A2".to_string()),
            shots: vec![shot(2.0)],
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
        shots: vec![shot(3.0)],
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
    let state = state_with_project("Test");
    // Create an existing section to merge with
    state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let existing_section_id = state.get_project_state().unwrap().sections[0].id;

    let result = state.execute_create_and_merge(
        Some("Imported".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![shot(3.0)],
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
    let state = state_with_project("Test");
    state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let sections_before = state.get_project_state().unwrap().sections.len();

    // Try merging where the existing station ref points to a nonexistent section
    let result = state.execute_create_and_merge(
        Some("Imported".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![shot(3.0)],
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
        vec![shot(3.0)],
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

    let state = state_with_project("Test");

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
            shots: vec![shot(5.0)],
            station_comments: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
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
            shots: vec![shot(3.0)],
            station_comments: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
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

    let state = state_with_project("Test");

    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Import1".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(5.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
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

    let state = state_with_project("Test");

    // Real - virtual - real: the chain rule calls the middle shot a Jump.
    let virtual_shot = ShotData {
        kind: ShotKind::Reference,
        ..shot(4.0)
    };
    let sections = vec![ImportSectionConfig {
        section_index: 0,
        name: Some("Imported".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(5.0), virtual_shot, shot(6.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        reverse: false,
        azimuth_reference: None,
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

    let state = state_with_project("Test");

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
        placement: ImportPlacement::Origin,
        reverse: false,
        azimuth_reference: None,
    };
    let reference_shot = ShotData {
        kind: ShotKind::Reference,
        ..shot(4.0)
    };

    // First file: a bounded virtual run, which the user then demotes by hand to
    // Reference (the non-swimmable tape tie the chain rule cannot detect).
    let first = state.execute_import_sections(
        vec![config(
            "First",
            vec![shot(5.0), reference_shot.clone(), shot(6.0)],
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

    let state = state_with_project("Test");

    let seed = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(5.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
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
        shots: vec![shot(3.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
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

    let state = state_with_project("Test");

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
            shots: vec![shot(5.0), shot(3.0)],
            station_comments: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
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
            shots: vec![shot(4.0)],
            station_comments: Vec::new(),
            placement: ImportPlacement::FromImporting {
                import_station: 0,
                target_section_index: 0,
                target_station_index: 1,
            },
            azimuth_reference: None,
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

    let state = state_with_project("Test");
    // Create existing section to merge with
    state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(3.0)],
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
        shots: vec![shot(4.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::FromExisting {
            import_station: 0,
            existing_station: StationRef::new(existing_id, 2),
        },
        azimuth_reference: None,
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

    let state = state_with_project("Test");

    let sections = vec![ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Manual".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(5.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Manual {
            align_station_index: 0,
            offset: Some(Coordinates {
                x: 10.0,
                y: 20.0,
                z: 0.0,
            }),
        },
        azimuth_reference: None,
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
    let offset = project.section_offsets().get(&section_id);
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

    let state = state_with_project("Test");

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
        shots: vec![shot(5.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::ZeroDatum {
            coordinates: gps,
            station_index: 0,
        },
        azimuth_reference: None,
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
    let state = state_with_project("Test");

    let gps = GpsCoordinates::new(10.0, 20.0, 5.0).unwrap();
    let imported_shot = shot(5.0);
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
        placement: ImportPlacement::ZeroDatum {
            coordinates: gps,
            station_index: 1,
        },
        azimuth_reference: None,
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

    let state = state_with_project("Test");

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
            shots: vec![shot(5.0)],
            station_comments: Vec::new(),
            placement: ImportPlacement::Origin,
            azimuth_reference: None,
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
            shots: vec![shot(3.0)],
            station_comments: Vec::new(),
            placement: ImportPlacement::FromExisting {
                import_station: 0,
                existing_station: StationRef::new(9999, 0), // nonexistent
            },
            azimuth_reference: None,
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
    let state = state_with_project("Test");

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
        shots: vec![shot(5.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
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

    let state = state_with_project("Test");
    state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
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
        shots: vec![shot(4.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::ManualSnap {
            align_station_index: 0,
            snapped_to_station: StationRef::new(existing_id, 1),
        },
        azimuth_reference: None,
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
