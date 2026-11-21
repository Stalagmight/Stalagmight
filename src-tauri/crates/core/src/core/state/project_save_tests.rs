//! The save path's edges on the station-graph core: configuration, the
//! save-failure classification the frontend routes its remediation on (#39,
//! #429), unwritable / unreadable / missing folders, the recent-projects list,
//! and the canonical round trip (the same model gives the same bytes, plan
//! 6.3). Ported from `state/tests.rs` "Config Tests", the save-failure
//! classification matrix and "Group 1: Round-trip fidelity tests"; the
//! report of package C13 lists the retired cases with reasons.
//!
//! All data is synthetic: projects built in memory and saved by this build.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{ShotData, ShotUpdates};
use crate::persistence::stalx_v2::{self, Extras, Format, Mode};

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

/// One line of one shot, placed.
fn survey(name: &str) -> Project {
    let mut p = Project::new(name, None);
    let line = p
        .add_line(LineMeta::default(), StartAt::New, vec![test_shot_data(1.0)])
        .unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    p
}

fn poison_mutex<T>(mutex: &std::sync::Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock().unwrap();
        panic!("poison mutex for lock failure coverage");
    }));
}

// === Config ===

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
    let chained =
        anyhow::anyhow!("unable to open database file").context("Failed to open database");
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

    let result = super::project_io::save_failure("Failed to create save file", &target, &err);

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
    let err = anyhow::anyhow!("unable to open database file").context("Failed to open database");

    let result = super::project_io::save_failure("Failed to create save file", &target, &err);

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
    let err = anyhow::anyhow!("unable to open database file").context("Failed to open database");

    let result = super::project_io::save_failure("Failed to create save file", &target, &err);

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

    let kind = super::project_io::create_write_probe(&taken)
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

    let kind = super::project_io::create_write_probe(&planted)
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
    let err = anyhow::anyhow!("unable to open database file").context("Failed to open database");

    let result = super::project_io::save_failure("Failed to create save file", &target, &err);

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

    let project = survey("Perm");
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
    // A metadata-access denial is a permission error, not "File not found"
    // (#169): a path inside a 0o000 dir cannot be read because the dir cannot
    // be traversed. Same CI-safe 0o000 idiom (root-skip) as
    // save_into_unwritable_dir_reports_permission_code.
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
    let read = state.read_project_file(&locked.join("p.stalx"));
    let result = state.open_project(locked.join("p.stalx"));

    // Restore perms so TempDir's drop can clean up the directory.
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    assert!(!result.success, "open inside an unreadable dir should fail");
    let Err(err) = read else {
        unreachable!("the read must fail");
    };
    // Both opens read through `read_project_file`; the load result carries
    // the code the frontend routes on.
    assert_eq!(
        err.code, "PERMISSION",
        "a metadata-access denial is the load's permission error: {}",
        err.message
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

    let project = survey("Missing");
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
    // Nothing is created: no temp file, no stray folder.
    assert!(!temp_dir.path().join("gone").exists());
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

    // Tracking is a recorded edit now (C7), and the edit pipeline takes the
    // project lock poison-tolerantly like every recorded edit: with no project
    // open it refuses as any edit does.
    let result = state.track_imported_file("hash".to_string());
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("No project open"));
}

/// A Save As onto something that cannot take the file (a folder) fails, and
/// changes neither the open file nor the session's path; no temp is left.
#[test]
fn test_save_project_failure_does_not_change_existing_file() {
    let dir = TempDir::new().unwrap();
    let original = dir.path().join("original.stalx");
    std::fs::write(
        &original,
        stalx_v2::encode(&survey("Original"), &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let before = std::fs::read(&original).unwrap();
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::OpenProject {
                path: original.clone()
            })
            .success
    );
    let blocked = dir.path().join("blocked.stalx");
    std::fs::create_dir_all(&blocked).unwrap();

    let result = state.save_project(Some(blocked));

    assert!(!result.success);
    assert_eq!(std::fs::read(&original).unwrap(), before);
    assert_eq!(state.get_project_state().unwrap().file_path, Some(original));
    assert!(!dir.path().join("blocked.stalx.tmp").exists());
}

// === The canonical round trip (plan 6.3) ===

/// Open a file, save it, and return the bytes; then reopen and save again.
fn save_twice(path: &Path) -> (Vec<u8>, Vec<u8>) {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::OpenProject {
                path: path.to_path_buf()
            })
            .success
    );
    // A save writes the file when asked, edit or not.
    let first = state.execute(Command::SaveProject { path: None });
    assert!(first.success, "{:?}", first.error);
    let once = std::fs::read(path).unwrap();
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::OpenProject {
                path: path.to_path_buf()
            })
            .success
    );
    assert!(state.execute(Command::SaveProject { path: None }).success);
    (once, std::fs::read(path).unwrap())
}

/// Save of open of save is the save: a survey with a junction, a closure,
/// datums, a hand move, Sections and a history comes back byte for byte.
#[test]
fn the_stalx_round_trip_is_a_byte_fixpoint() {
    use crate::models::survey::{ClosureStrategy, ZeroDatum};
    use crate::models::GpsCoordinates;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("fixpoint.stalx");
    let mut p = Project::new("Fixpoint", Some("Synthetic".to_string()));
    let a = p
        .add_line(
            LineMeta {
                name: Some("A".to_string()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![test_shot_data(5.0), test_shot_data(3.0)],
        )
        .unwrap();
    let junction = p.line(a).unwrap().station(1).unwrap();
    let b = p
        .add_line(
            LineMeta {
                name: Some("B".to_string()),
                ..LineMeta::default()
            },
            StartAt::Station(junction),
            vec![test_shot_data(4.0), test_shot_data(2.5)],
        )
        .unwrap();
    let a_end = p.line(a).unwrap().station(2).unwrap();
    let b_end = p.line(b).unwrap().station(2).unwrap();
    p.push_closure(a_end, Some(a), b_end, Some(b), ClosureStrategy::Batch);
    let start = p.line(a).unwrap().start;
    p.push_datum(ZeroDatum {
        station: start,
        via: Some(a),
        gps: GpsCoordinates::new(20.0, -87.0, 1.5).unwrap(),
        accuracy_m: Some(3.0),
        entrance: Some(true),
        correction: Some([0.25, -0.5]),
    });
    p.line_mut(b)
        .unwrap()
        .hand_moves
        .insert(b_end, [0.1, 0.2, 0.0]);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    std::fs::write(
        &path,
        stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    {
        // A history entry, so the round trip carries one.
        let state = AppState::new().unwrap();
        assert!(
            state
                .execute(Command::OpenProject { path: path.clone() })
                .success
        );
        let (line_id, shot_id) = {
            let p = state.get_project_state().unwrap();
            (p.lines()[0].id, p.lines()[0].shots[0].id)
        };
        assert!(
            state
                .execute(Command::UpdateShot {
                    line_id,
                    shot_id,
                    updates: ShotUpdates {
                        length: Some(5.5),
                        ..ShotUpdates::default()
                    },
                })
                .success
        );
        assert!(state.execute(Command::SaveProject { path: None }).success);
    }
    let saved = std::fs::read(&path).unwrap();

    let (once, twice) = save_twice(&path);

    assert_eq!(once, saved, "open then save writes the same bytes");
    assert_eq!(twice, saved, "and again");
    let decoded = stalx_v2::decode(&saved, &stalx_v2::DecodeOptions::default()).unwrap();
    assert_eq!(decoded.extras.history.undo.len(), 1);
    assert_eq!(decoded.project.closures.len(), 1);
    assert_eq!(decoded.project.lines()[1].hand_moves.len(), 1);
}

/// A legacy file's first save upgrades it; from then on the round trip is a
/// fixpoint like any version 2 file's.
#[test]
fn a_converted_legacy_file_round_trips_as_a_fixpoint() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let stem = format!("fixpoint-legacy-{}", uuid::Uuid::new_v4());
    let path = dir.path().join(format!("{stem}.stalx"));
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures/fractal_plant_iter3.stalx");
    std::fs::copy(&fixture, &path).unwrap();
    assert_eq!(
        stalx_v2::sniff(&std::fs::read(&path).unwrap()),
        Format::Legacy
    );

    let (upgraded, again) = save_twice(&path);
    if let Some(folder) = super::project_io::upgrade_backup_dir() {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(&stem) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    assert_eq!(stalx_v2::sniff(&upgraded), Format::V2);
    assert_eq!(again, upgraded);
}

/// A legacy file whose merge the 0.6.0 open never resolved (C hangs off B,
/// its stored offset drifted 5 m, and a merge row that resolves nothing makes
/// A the cascade's seed) converts with C's two stations moved onto B: the
/// open offers the conversion notice once. The same survey clean moves
/// nothing, but C's start now reads B's station's name, so its notice says
/// that only (review findings UX-1 and UX-3).
#[test]
fn a_drifted_legacy_merge_raises_the_conversion_notice_once() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let open = |path: &Path| {
        let state = AppState::new().unwrap();
        let opened = state.execute(Command::OpenProject {
            path: path.to_path_buf(),
        });
        assert!(opened.success, "{:?}", opened.error);
        state
    };

    let path = dir.path().join("drifted.stalx");
    write_legacy_drift(&path, true);
    let state = open(&path);
    let notice = state.take_conversion_notice().expect("a conversion notice");
    assert!(
        notice.starts_with("Converted from the old file format; 2 stations moved by up to 5.00 m"),
        "{notice}"
    );
    assert_eq!(state.take_conversion_notice(), None, "offered once");

    let path = dir.path().join("clean.stalx");
    write_legacy_drift(&path, false);
    let notice = open(&path)
        .take_conversion_notice()
        .expect("a name changed");
    assert_eq!(
        notice,
        "Converted from the old file format; 1 station name changed (a merged station has \
         one name on all its lines): Line 3 0 is now 1. \
         The new format is written the first time the project is saved, or already \
         was if you renamed it in Recent Projects or removed its SpeleoDB link; the \
         original file is kept: to go back to it, right-click the project in Recent \
         Projects and choose Restore an earlier copy."
    );
}

/// Review finding UX-1: a conversion that moved nothing still says what each
/// line now shows differently: here C's start reads B's station's name, and
/// the two members' comments are one station's comment now.
#[test]
fn a_conversion_that_joined_comments_says_so() {
    use super::legacy_fixture_kit::{frozen, old, slot, write_legacy_file, Legacy};
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let flat = |azimuth: f64| ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 10.0,
        ..ShotData::default()
    };
    let mut p = Legacy::new("Comments".to_string(), None);
    let mut add = |shots: Vec<ShotData>| {
        p.add_line(old::LineData {
            shots: shots.iter().map(frozen).collect(),
            quality: 1.0,
            ..Default::default()
        })
        .id
    };
    let b = add(vec![flat(0.0), flat(0.0)]);
    let c = add(vec![flat(90.0)]);
    p.merge_stations(slot(c, 0), slot(b, 1)).unwrap();
    for (sr, text) in [(slot(b, 1), "sump"), (slot(c, 0), "low air")] {
        let key = p.key_for_ref(sr).unwrap();
        p.station_comments.insert(key, text.to_string());
    }
    let path = dir.path().join("comments.stalx");
    write_legacy_file(&path, &p);
    let state = AppState::new().unwrap();
    let opened = state.execute(Command::OpenProject { path });
    assert!(opened.success, "{:?}", opened.error);
    let notice = state.take_conversion_notice().expect("a notice");
    assert!(!notice.contains("stations moved"), "{notice}");
    assert!(notice.contains("1 station name changed"), "{notice}");
    assert!(
        notice.contains("a merged station's two comments were joined"),
        "{notice}"
    );
}

/// A 0.6.0 file whose merge of C onto B the migration moves (`drift`: C's
/// line offset drifted 5 m and a dangling merge row names A, the cascade's
/// seed), or the same survey clean.
pub(super) fn write_legacy_drift(path: &Path, drift: bool) {
    write_legacy_merge(path, drift, false);
}

/// [`write_legacy_drift`]'s clean survey with both merged slots named `J`:
/// its conversion moves nothing and changes no name a line shows, so it
/// raises no conversion notice at all.
pub(super) fn write_legacy_unchanged(path: &Path) {
    write_legacy_merge(path, false, true);
}

fn write_legacy_merge(path: &Path, drift: bool, same_name: bool) {
    use super::legacy_fixture_kit::{frozen, old, slot, write_legacy_file, Legacy};

    let flat = |azimuth: f64| ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 10.0,
        ..ShotData::default()
    };
    let mut p = Legacy::new("Drift".to_string(), None);
    let mut add = |shots: Vec<ShotData>| {
        p.add_line(old::LineData {
            shots: shots.iter().map(frozen).collect(),
            quality: 1.0,
            ..Default::default()
        })
        .id
    };
    let a = add(vec![flat(90.0)]);
    let b = add(vec![flat(0.0), flat(0.0)]);
    let c = add(vec![flat(90.0)]);
    if same_name {
        for sr in [slot(b, 1), slot(c, 0)] {
            p.set_station_name(sr, "J").unwrap();
        }
    }
    p.merge_stations(slot(c, 0), slot(b, 1)).unwrap();
    if drift {
        p.adjust_line_offset(c, 5.0, 0.0);
    }
    write_legacy_file(path, &p);
    if drift {
        rusqlite::Connection::open(path)
            .unwrap()
            .execute(
                "INSERT INTO station_merges (station_a_section, station_a_index, \
                 station_b_section, station_b_index, merged_at) \
                 VALUES (?1, 999, ?2, 0, '2023-11-14T22:13:20+00:00')",
                [a, b],
            )
            .unwrap();
    }
}
