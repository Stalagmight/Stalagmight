//! Recovery: numbered backups, listing them, open_for_recovery, attempt_repair, export_salvageable
//! and the try_read helpers.
//!
//! Lines 3238-3549 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

// =========================================================================
// Recovery: create_numbered_backup
// =========================================================================

#[test]
fn test_create_numbered_backup_first_creates_no_suffix() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");

    // Create a minimal source file
    let db = Database::open(&db_path).unwrap();
    drop(db);

    let backup = Database::create_numbered_backup(&db_path).unwrap();
    assert_eq!(backup, temp_dir.path().join("project.stalx.backup"));
    assert!(backup.exists());
}

#[test]
fn test_create_numbered_backup_second_gets_suffix_1() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");

    let db = Database::open(&db_path).unwrap();
    drop(db);

    // Create first backup
    let first = Database::create_numbered_backup(&db_path).unwrap();
    assert!(first.exists());

    // Second backup should be .backup.1
    let second = Database::create_numbered_backup(&db_path).unwrap();
    assert_eq!(second, temp_dir.path().join("project.stalx.backup.1"));
    assert!(second.exists());
}

#[test]
fn test_create_numbered_backup_increments_until_free_slot() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");

    let db = Database::open(&db_path).unwrap();
    drop(db);

    // Create base + .1 through .3
    for _ in 0..4 {
        Database::create_numbered_backup(&db_path).unwrap();
    }

    let fifth = Database::create_numbered_backup(&db_path).unwrap();
    assert_eq!(fifth, temp_dir.path().join("project.stalx.backup.4"));
}

// =========================================================================
// Recovery: list_backups
// =========================================================================

#[test]
fn test_list_backups_empty_when_no_backups() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");
    std::fs::write(&db_path, b"dummy").unwrap();

    let backups = Database::list_backups(&db_path);
    assert!(backups.is_empty());
}

#[test]
fn test_list_backups_finds_backup_files() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");
    std::fs::write(&db_path, b"original").unwrap();

    // Create backup-named files
    std::fs::write(temp_dir.path().join("project.stalx.backup"), b"backup0").unwrap();
    std::fs::write(temp_dir.path().join("project.stalx.backup.1"), b"backup1").unwrap();
    // A .tmp file for the same stem
    std::fs::write(temp_dir.path().join("project.stalx.tmp"), b"tmp").unwrap();
    // Unrelated file should not appear
    std::fs::write(temp_dir.path().join("other.stalx.backup"), b"other").unwrap();

    let backups = Database::list_backups(&db_path);
    // Should find 3 matching files (project.stalx.backup, .backup.1, .tmp)
    assert_eq!(backups.len(), 3);
    // All paths should contain "project.stalx"
    for b in &backups {
        assert!(b.path.contains("project.stalx"));
    }
}

#[test]
fn test_list_backups_sorted_newest_first() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.stalx");
    std::fs::write(&db_path, b"original").unwrap();

    // Create files with slightly different timestamps
    std::fs::write(temp_dir.path().join("project.stalx.backup"), b"old").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(temp_dir.path().join("project.stalx.backup.1"), b"newer").unwrap();

    let backups = Database::list_backups(&db_path);
    assert_eq!(backups.len(), 2);
    // Newest first
    assert!(backups[0].path.contains("backup.1"));
    assert!(backups[1].path.contains(".backup"));
    assert!(!backups[1].path.contains("backup.1"));
}

#[test]
fn test_list_backups_nonexistent_parent_returns_empty() {
    let path = std::path::PathBuf::from("/nonexistent/dir/project.stalx");
    let backups = Database::list_backups(&path);
    assert!(backups.is_empty());
}

#[test]
fn test_list_backups_no_parent_returns_empty() {
    // A bare filename with no parent directory
    let path = std::path::PathBuf::from("project.stalx");
    let backups = Database::list_backups(&path);
    // Should not panic, returns empty or finds matches in current dir
    // Just verify it doesn't crash
    let _ = backups;
}

// =========================================================================
// Recovery: open_for_recovery
// =========================================================================

#[test]
fn test_open_for_recovery_nonexistent_file_fails() {
    let result = Database::open_for_recovery(Path::new("/nonexistent/db.stalx"));
    assert!(result.is_err());
}

#[test]
fn test_open_for_recovery_valid_db_succeeds() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("valid.stalx");

    // Create a valid database
    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Recovery Test".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    // Open for recovery should succeed even on a healthy file
    let db = Database::open_for_recovery(&db_path).unwrap();
    // Should be able to query basic info
    let result: String = db
        .conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(result, "ok");
}

// =========================================================================
// Recovery: attempt_repair
// =========================================================================

#[test]
fn test_attempt_repair_on_healthy_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("healthy.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Repair Test".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    let (backup_path, ok) = Database::attempt_repair(&db_path).unwrap();
    assert!(
        ok,
        "Healthy database should pass integrity check after repair"
    );
    assert!(backup_path.exists(), "Backup should exist");

    // Verify original file is still valid
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.name, "Repair Test");
}

// =========================================================================
// Recovery: export_salvageable
// =========================================================================

#[test]
fn test_export_salvageable_from_healthy_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("salvage.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Salvage Test".to_string(), Some("Cave X".to_string()));
    let section = SectionData {
        name: Some("Passage A".to_string()),
        color: "#ff0000".to_string(),
        shots: vec![ShotData::default(), ShotData::default()],
        ..Default::default()
    };
    project.add_section(section);
    db.save_project(&project).unwrap();
    drop(db);

    let salvaged = Database::export_salvageable(&db_path).unwrap();
    assert_eq!(salvaged.project_name.as_deref(), Some("Salvage Test"));
    assert_eq!(salvaged.project_location.as_deref(), Some("Cave X"));
    assert_eq!(salvaged.section_count, 1);
    assert_eq!(salvaged.shot_count, 2);
    assert!(salvaged.sections_json.is_some());
    assert!(salvaged.full_project_json.is_some());
    assert!(salvaged.errors.is_empty());
}

#[test]
fn test_export_salvageable_nonexistent_file_fails() {
    let result = Database::export_salvageable(Path::new("/nonexistent/db.stalx"));
    assert!(result.is_err());
}

// =========================================================================
// Recovery: try_read helpers
// =========================================================================

#[test]
fn test_try_read_metadata_from_empty_db() {
    // In-memory DB with schema but no project data
    let db = Database::in_memory().unwrap();
    let result = db.try_read_project_metadata().unwrap();
    assert!(result.is_none());
}

#[test]
fn test_try_read_metadata_from_populated_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("metadata.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Meta Project".to_string(), Some("Location X".to_string()));
    db.save_project(&project).unwrap();

    let result = db.try_read_project_metadata().unwrap();
    assert!(result.is_some());
    let (name, location) = result.unwrap();
    assert_eq!(name, "Meta Project");
    assert_eq!(location.as_deref(), Some("Location X"));
}

#[test]
fn test_try_read_sections_from_empty_db() {
    let db = Database::in_memory().unwrap();
    let sections = db.try_read_sections().unwrap();
    assert!(sections.is_empty());
}

#[test]
fn test_try_read_sections_from_populated_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("sections.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Sections Test".to_string(), None);
    project.add_section(SectionData {
        name: Some("Passage A".to_string()),
        color: "#ff0000".to_string(),
        ..Default::default()
    });
    project.add_section(SectionData {
        name: Some("Passage B".to_string()),
        color: "#00ff00".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    let sections = db.try_read_sections().unwrap();
    assert_eq!(sections.len(), 2);
}

#[test]
fn test_try_read_shot_count_empty() {
    let db = Database::in_memory().unwrap();
    let count = db.try_read_shot_count().unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_try_read_merge_count_empty() {
    let db = Database::in_memory().unwrap();
    let count = db.try_read_merge_count().unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_has_project_data_empty_db() {
    let db = Database::in_memory().unwrap();
    assert!(!db.has_project_data().unwrap());
}

#[test]
fn test_has_project_data_with_project() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("has_data.stalx");

    let mut db = Database::open(&db_path).unwrap();
    assert!(!db.has_project_data().unwrap());

    let project = Project::new("Test".to_string(), None);
    db.save_project(&project).unwrap();
    assert!(db.has_project_data().unwrap());
}
