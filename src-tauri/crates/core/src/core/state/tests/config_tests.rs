//! Config, display settings and save-failure classification.
//!
//! Lines 6502-7263 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_holding, state_with_project};

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
            let mut cfg = state.config.lock();
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
    // Create a project (no database, so settings go to cache)
    let state = state_with_project("Test");

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
        shots: vec![shot(1.0)],
        ..Default::default()
    });

    let mut db = Database::open(&db_path).unwrap();
    db.save_project(&project).unwrap();
    db.save_display_setting("showStations", "false").unwrap();
    drop(db);

    let state = state_holding(project);
    let db = Database::open_existing(&db_path).unwrap();
    state.set_database_for_tests(db);

    state
        .display_settings_cache
        .lock()
        .insert("showStations".to_string(), "true".to_string());

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
        shots: vec![shot(1.0)],
        ..Default::default()
    });

    let mut db = Database::open(&original_path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);

    project.file_path = Some(original_path.clone());

    let state = state_holding(project.clone());

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
    use crate::models::CommandErrorCode;
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
        result.code == Some(CommandErrorCode::FsPermissionDenied) && !msg.starts_with("FS_"),
        "a typed PermissionDenied chain is a denial, got: {msg}"
    );
}

#[test]
fn save_failure_tags_a_vanished_folder_with_the_path_code() {
    use crate::models::CommandErrorCode;
    let temp_dir = TempDir::new().unwrap();
    // Nothing was ever created at `gone/`, so the destination folder is absent.
    let target = temp_dir.path().join("gone").join("cave.stalx");
    let err = anyhow::Error::from(anyhow::anyhow!("unable to open database file"))
        .context("Failed to open database");

    let result = project_io::save_failure("Failed to create save file", &target, &err);

    let msg = result.error.unwrap_or_default();
    assert!(
        result.code == Some(CommandErrorCode::FsPathNotFound) && !msg.starts_with("FS_"),
        "a missing destination folder is a path error, got: {msg}"
    );
    assert_ne!(result.code, Some(CommandErrorCode::FsPermissionDenied));
}

#[test]
fn save_failure_leaves_a_generic_open_failure_unclassified() {
    use crate::models::CommandErrorCode;
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
        result.code != Some(CommandErrorCode::FsPermissionDenied),
        "a writable destination folder is denying nothing, got: {msg}"
    );
    assert_ne!(
        result.code,
        Some(CommandErrorCode::FsPathNotFound),
        "got: {msg}"
    );
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
    use crate::models::CommandErrorCode;
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
        result.code == Some(CommandErrorCode::FsPathTooLong) && !msg.starts_with("FS_"),
        "an over-MAX_PATH destination is a path-length error, got: {msg}"
    );
    assert_ne!(result.code, Some(CommandErrorCode::FsPermissionDenied));
}

#[cfg(unix)]
#[test]
fn save_into_unwritable_dir_reports_permission_code() {
    use crate::models::CommandErrorCode;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let locked = temp_dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root writes despite 0o000: fail, never pass on nothing (#576 NF-ROOT-PERM).
    crate::models::test_support::assert_dir_refuses_new_files(&locked);

    let mut project = Project::new("Perm".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    let state = state_holding(project);

    let result = state.save_project(Some(locked.join("p.stalx")));

    // Restore perms so TempDir's drop can clean up the directory.
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    assert!(!result.success, "save into an unwritable dir should fail");
    let err = result.error.unwrap_or_default();
    assert!(
        result.code == Some(CommandErrorCode::FsPermissionDenied) && !err.starts_with("FS_"),
        "expected permission code prefix, got: {err}"
    );
}

#[cfg(unix)]
#[test]
fn open_project_into_unreadable_dir_reports_permission_code() {
    // open_project switched exists() -> try_exists() so a metadata-access denial
    // surfaces as FS_PERMISSION_DENIED instead of collapsing to "File not found"
    // (#169). try_exists() on a path inside a 0o000 dir returns Err(PermissionDenied)
    // because the dir can't be traversed. Same 0o000 idiom (precondition asserted)
    // as save_into_unwritable_dir_reports_permission_code.
    use crate::models::CommandErrorCode;
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let locked = temp_dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root walks through 0o000: fail, never pass on nothing (#576 NF-ROOT-PERM).
    crate::models::test_support::assert_lookup_refused(&locked.join("p.stalx"));

    let state = AppState::new().unwrap();
    let result = state.open_project(locked.join("p.stalx"));

    // Restore perms so TempDir's drop can clean up the directory.
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    assert!(!result.success, "open inside an unreadable dir should fail");
    let err = result.error.unwrap_or_default();
    assert!(
        result.code == Some(CommandErrorCode::FsPermissionDenied) && !err.starts_with("FS_"),
        "open_project should tag a metadata-access denial with the permission code, got: {err}"
    );
}

#[test]
fn save_into_missing_dir_is_not_a_permission_error() {
    use crate::models::CommandErrorCode;

    // A deleted/renamed save folder yields SQLite's generic "unable to open
    // database file" too - but that's a vanished location, not a permission
    // denial, so it must NOT be stamped with the permission code (which would
    // wrongly show the user Privacy & Security guidance). It is refused up
    // front instead, with the code that says the folder is gone (#429).
    let temp_dir = TempDir::new().unwrap();
    let missing = temp_dir.path().join("gone").join("p.stalx");

    let mut project = Project::new("Missing".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    let state = state_holding(project);

    let result = state.save_project(Some(missing));

    assert!(!result.success, "save into a missing dir should fail");
    let err = result.error.unwrap_or_default();
    assert!(
        result.code != Some(CommandErrorCode::FsPermissionDenied),
        "missing dir should not be a permission failure, got: {err}"
    );
    assert!(
        result.code == Some(CommandErrorCode::FsPathNotFound) && !err.starts_with("FS_"),
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
fn test_display_settings_recover_a_poisoned_cache_lock() {
    // [#576 A11] Recovered, not refused: no database is open, so the in-memory
    // cache is where the settings live, and it keeps working after a crash.
    let state = AppState::new().unwrap();

    poison_mutex(&state.display_settings_cache);

    state.save_display_setting("key", "value").unwrap();
    assert_eq!(
        state.get_display_setting("key").unwrap().as_deref(),
        Some("value")
    );
    assert_eq!(
        state
            .load_display_settings()
            .unwrap()
            .get("key")
            .map(String::as_str),
        Some("value")
    );
    assert_eq!(
        state.poison_monitor().first_poisoned(),
        Some("display_settings_cache")
    );
}

#[test]
fn test_display_settings_recover_a_poisoned_database_lock() {
    // [#576 A11] Recovered, not refused: the recovered slot holds no database, so the
    // settings fall through to the in-memory cache exactly as they would unpoisoned.
    let state = AppState::new().unwrap();

    poison_mutex(&state.database);

    state.save_display_setting("key", "value").unwrap();
    assert_eq!(
        state.get_display_setting("key").unwrap().as_deref(),
        Some("value")
    );
    assert_eq!(
        state
            .load_display_settings()
            .unwrap()
            .get("key")
            .map(String::as_str),
        Some("value")
    );
    assert_eq!(state.poison_monitor().first_poisoned(), Some("database"));
}

#[test]
fn test_resolved_warning_methods_recover_a_poisoned_database_lock() {
    let state = AppState::new().unwrap();

    poison_mutex(&state.database);

    let err = state.load_resolved_warnings().unwrap_err();
    // [#576 A11] The lock recovers; what is missing is the database itself.
    assert_eq!(err.to_string(), "No database open");

    let warning = crate::persistence::ResolvedWarning {
        section_id: 1,
        shot_id: 2,
        warning_code: "TEST".to_string(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    };

    let err = state.save_resolved_warning(&warning).unwrap_err();
    // [#576 A11] The lock recovers; what is missing is the database itself.
    assert_eq!(err.to_string(), "No database open");

    let err = state
        .delete_resolved_warning(1, 2, "TEST", None)
        .unwrap_err();
    // [#576 A11] The lock recovers; what is missing is the database itself.
    assert_eq!(err.to_string(), "No database open");
}

#[test]
fn test_remove_recent_projects_recovers_a_poisoned_lock() {
    // [#576 A11] Recovered, not refused. Isolated because the removal persists the
    // list, and a save failure would read as the lock failing.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        *state.recent_projects.lock() = vec![PathBuf::from("/a.stalx"), PathBuf::from("/b.stalx")];
        poison_mutex(&state.recent_projects);

        let result = state.remove_from_recent_projects(PathBuf::from("/a.stalx"));
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            *state.recent_projects.lock(),
            vec![PathBuf::from("/b.stalx")]
        );
        assert_eq!(
            state.poison_monitor().first_poisoned(),
            Some("recent_projects")
        );
    });
}

#[test]
fn test_remove_recent_project_removes_present_entry() {
    // Isolated because `remove_from_recent_projects` persists the trimmed list and
    // reports a save failure as `success: false`, so this assert is only about the
    // removal if the save has a writable config file of its own to land in.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        {
            let mut recent = state.recent_projects.lock();
            *recent = vec![PathBuf::from("/a.stalx"), PathBuf::from("/b.stalx")];
        }

        let result = state.remove_from_recent_projects(PathBuf::from("/a.stalx"));
        assert!(result.success);
        let recent = state.recent_projects.lock();
        assert_eq!(*recent, vec![PathBuf::from("/b.stalx")]);
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
            let mut recent = state.recent_projects.lock();
            *recent = vec![PathBuf::from("/b.stalx")];
        }

        let result = state.remove_from_recent_projects(PathBuf::from("/gone.stalx"));
        assert!(result.success, "removing an absent recent must succeed");
        // The list is left untouched (nothing to remove).
        let recent = state.recent_projects.lock();
        assert_eq!(*recent, vec![PathBuf::from("/b.stalx")]);
    });
}

#[test]
fn test_import_tracking_recovers_a_poisoned_project_lock() {
    // [#576 A11] Recovered, not refused: the bookkeeping reaches the project the lock
    // still holds.
    let state = state_with_project("CaveA");
    poison_mutex(&state.project);

    assert!(!state.check_duplicate_import("hash").unwrap());
    let result = state.track_imported_file("hash".to_string());
    assert!(result.success, "{:?}", result.error);
    assert!(state.check_duplicate_import("hash").unwrap());
    assert_eq!(state.poison_monitor().first_poisoned(), Some("project"));
}
