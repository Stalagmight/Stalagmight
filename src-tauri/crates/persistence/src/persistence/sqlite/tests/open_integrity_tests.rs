//! Integrity checks, open refusals and the .stalx extension; individual station offsets and history
//! timestamps.
//!
//! Lines 2170-2468 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn test_integrity_check_passes_on_valid_db() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("integrity_valid.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Integrity Test".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    // open_existing runs validate_integrity internally - should succeed
    let db = Database::open_existing(&db_path).unwrap();
    // full_integrity_check should also pass
    let errors = db.full_integrity_check().unwrap();
    assert!(
        errors.is_empty(),
        "Expected no integrity errors, got: {errors:?}"
    );
}

#[test]
fn test_integrity_check_passes_on_readonly() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("integrity_readonly.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Readonly Integrity".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    // open_readonly runs validate_integrity internally - should succeed
    let _db = Database::open_readonly(&db_path).unwrap();
}

#[test]
fn test_integrity_check_fails_on_corrupt_file() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("corrupt.stalx");

    // Write garbage data to simulate a corrupt database file
    std::fs::write(&db_path, b"This is not a valid SQLite database").unwrap();

    // open_existing should fail (either at connection open or integrity check)
    let result = Database::open_existing(&db_path);
    assert!(result.is_err(), "Expected error for corrupt database");
}

#[test]
fn test_full_integrity_check_on_in_memory() {
    let db = Database::in_memory().unwrap();
    let errors = db.full_integrity_check().unwrap();
    assert!(
        errors.is_empty(),
        "In-memory DB should pass integrity check"
    );
}

// === Open Existing edge cases ===

#[test]
fn test_open_existing_fails_on_nonexistent_file() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("does_not_exist.stalx");

    let result = Database::open_existing(&db_path);
    assert!(result.is_err(), "Expected error for nonexistent file");
}

#[test]
fn test_open_existing_fails_on_empty_database() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("empty.stalx");

    // Create an empty SQLite database (no project tables)
    {
        let _conn = Connection::open(&db_path).unwrap();
        // Close without creating tables
    }

    let result = Database::open_existing(&db_path);
    assert!(result.is_err(), "Empty database should fail open_existing");
}

#[test]
fn test_open_readonly_fails_on_nonexistent_file() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("gone.stalx");

    let result = Database::open_readonly(&db_path);
    assert!(result.is_err());
}

#[test]
fn test_open_readonly_fails_on_non_stalx_extension() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("project.sqlite");

    // Create a valid SQLite database with wrong extension
    {
        let mut db = Database::open(&db_path).unwrap();
        let project = Project::new("Wrong Ext".to_string(), None);
        db.save_project(&project).unwrap();
    }

    let result = Database::open_readonly(&db_path);
    assert!(result.is_err(), "Non-.stalx extension should be rejected");
}

/// [#576 B33] `.STALX` is a project file too. The app opens one (the frontend
/// routes extensions case-insensitively), so the read-only open GlobalView's
/// geometry preview uses must not refuse it, or that project shows no geometry.
#[test]
fn test_open_readonly_accepts_an_uppercase_stalx_extension() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("CaveA.STALX");
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&Project::new("Upper Ext".to_string(), None))
            .unwrap();
    }

    let result = Database::open_readonly(&db_path);
    eprintln!(
        "open_readonly({}): {}",
        db_path.display(),
        match &result {
            Ok(_) => "Ok".to_string(),
            Err(e) => format!("Err({e})"),
        }
    );
    assert!(result.is_ok(), "a .STALX project must open read-only");
}

/// [#576 B33] Any spelling of `.stalx` is kept as typed; anything else gets it.
#[test]
fn test_with_stalx_extension_keeps_every_spelling_of_stalx() {
    let dir = Path::new("/projects");
    for kept in ["CaveA.stalx", "CaveA.STALX", "CaveA.Stalx"] {
        assert_eq!(
            with_stalx_extension(dir.join(kept)),
            dir.join(kept),
            "{kept} already has the extension"
        );
    }
    for (typed, written) in [("CaveA", "CaveA.stalx"), ("CaveA.tml", "CaveA.stalx")] {
        assert_eq!(with_stalx_extension(dir.join(typed)), dir.join(written));
    }
    assert!(!has_stalx_extension(Path::new("/projects/stalx")));
}

/// [#576 D35-D1] "Replace known, else append": only an extension naming a file
/// type the app reads or writes is replaced. A dotted name keeps its tail -
/// `CaveA 1.5` used to be written as `CaveA 1.stalx`, a different file.
#[test]
fn test_with_stalx_extension_replaces_known_types_and_appends_otherwise() {
    let dir = Path::new("/projects");
    for (typed, written) in [
        ("CaveA 1.5", "CaveA 1.5.stalx"),
        ("Survey v2.final", "Survey v2.final.stalx"),
        ("notes.txt", "notes.txt.stalx"),
        (".hidden", ".hidden.stalx"),
        ("CaveA.TMLU", "CaveA.stalx"),
        ("CaveA.dmp", "CaveA.stalx"),
        ("plan.plnx", "plan.stalx"),
        ("CaveA.", "CaveA.stalx"),
    ] {
        let got = with_stalx_extension(dir.join(typed));
        println!("{typed:?} -> {:?}", got.file_name().unwrap_or_default());
        assert_eq!(got, dir.join(written), "{typed}");
    }
}

// === Round-trip with station offsets (individual station moves) ===

#[test]
fn test_save_load_station_offsets_individual() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("station_offsets.stalx");

    let mut project = Project::new("Station Offsets".to_string(), None);
    let section_data = SectionData {
        shots: vec![
            ShotData {
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                length: 10.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                length: 5.0,
                depth_in: 1.0,
                depth_out: 2.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let section_id = project.add_section(section_data).id;

    // Add individual station offsets
    let ref_a = StationRef::new(section_id, 0);
    let ref_b = StationRef::new(section_id, 1);
    project
        .station_offsets
        .insert(ref_a.into(), (1.5, -2.3, 0.1));
    project
        .station_offsets
        .insert(ref_b.into(), (-0.5, 0.8, -0.3));

    // Save
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.station_offsets.len(), 2);
    let offset_a = loaded.station_offset_for_ref(ref_a).unwrap();
    assert!((offset_a.0 - 1.5).abs() < 1e-10);
    assert!((offset_a.1 - (-2.3)).abs() < 1e-10);
    assert!((offset_a.2 - 0.1).abs() < 1e-10);

    let offset_b = loaded.station_offset_for_ref(ref_b).unwrap();
    assert!((offset_b.0 - (-0.5)).abs() < 1e-10);
    assert!((offset_b.1 - 0.8).abs() < 1e-10);
    assert!((offset_b.2 - (-0.3)).abs() < 1e-10);
}

// === Undo/Redo stack persistence with timestamps ===

#[test]
fn test_undo_stack_timestamps_round_trip() {
    let db = Database::in_memory().unwrap();

    let entries = vec![
        (
            "cmd1".to_string(),
            "undo1".to_string(),
            Some("2025-01-15T10:00:00+00:00".to_string()),
        ),
        ("cmd2".to_string(), "undo2".to_string(), None),
        (
            "cmd3".to_string(),
            "undo3".to_string(),
            Some("2025-01-15T12:00:00+00:00".to_string()),
        ),
    ];

    db.save_undo_stack_with_timestamps(entries).unwrap();
    let loaded = db.load_undo_stack_with_timestamps().unwrap();

    assert_eq!(loaded.len(), 3);

    // Entry 0: explicit timestamp
    assert_eq!(loaded[0].0, 0); // sequence
    assert_eq!(loaded[0].1, "cmd1");
    assert_eq!(loaded[0].2, "undo1");
    assert_eq!(loaded[0].3.as_deref(), Some("2025-01-15T10:00:00+00:00"));

    // Entry 1: generated timestamp (should exist, not None)
    assert_eq!(loaded[1].1, "cmd2");
    assert!(
        loaded[1].3.is_some(),
        "Auto-generated timestamp should be present"
    );

    // Entry 2: explicit timestamp
    assert_eq!(loaded[2].3.as_deref(), Some("2025-01-15T12:00:00+00:00"));
}

#[test]
fn test_redo_stack_timestamps_round_trip() {
    let db = Database::in_memory().unwrap();

    let entries = vec![
        (
            "redo_cmd1".to_string(),
            "redo_undo1".to_string(),
            Some("2025-02-01T08:00:00+00:00".to_string()),
        ),
        ("redo_cmd2".to_string(), "redo_undo2".to_string(), None),
    ];

    db.save_redo_stack_with_timestamps(entries).unwrap();
    let loaded = db.load_redo_stack_with_timestamps().unwrap();

    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].1, "redo_cmd1");
    assert_eq!(loaded[0].3.as_deref(), Some("2025-02-01T08:00:00+00:00"));
    assert!(loaded[1].3.is_some());
}
