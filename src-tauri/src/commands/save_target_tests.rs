//! [#576 HFI-2] The overwrite prompts' existence probe, `save_target_exists`.
//!
//! The prompts used to ask the webview's fs plugin, whose `exists()` threw for
//! every user path in the real app (no capability scope), so every Save As asked
//! "could not check". The backend probe must answer true and false correctly,
//! and must refuse - not answer - where the write-path policy refuses the write
//! it precedes: the app's own folders, a system folder, a file type the app does
//! not write. A check that fails is an error (the frontend then asks, E10-D1);
//! a refusal carries `WRITE_PATH_REFUSED` (the frontend goes straight to the
//! write, which refuses with the reason, HFI-2-D2).
//!
//! All files are synthetic, in temp dirs.

use std::path::Path;

use tempfile::TempDir;

use crate::commands::save_target::{save_target_exists, save_target_exists_with};
use crate::commands::write_path::AppDirs;
use crate::core::{CommandError, CommandErrorCode};

fn no_app_dirs() -> AppDirs {
    AppDirs {
        internal: Vec::new(),
        managed_projects: None,
    }
}

/// The probe's answer as one line of evidence.
fn verdict(answer: &Result<bool, CommandError>) -> String {
    match answer {
        Ok(true) => "there".to_owned(),
        Ok(false) => "absent".to_owned(),
        Err(e) => format!("error {}: {}", e.code, e.message),
    }
}

fn probe(path: &Path, dirs: &AppDirs) -> Result<bool, CommandError> {
    let answer = save_target_exists_with(path, dirs);
    println!("{}: {}", path.display(), verdict(&answer));
    answer
}

#[test]
fn a_file_that_is_there_is_reported_there() {
    let dir = TempDir::new().unwrap();
    let dirs = no_app_dirs();
    for file in [
        "CaveA.stalx",
        "CaveA.csv",
        "map.PNG",
        "CaveA.tmlu",
        "plan.xml",
    ] {
        let path = dir.path().join(file);
        std::fs::write(&path, b"user data").unwrap();
        assert!(probe(&path, &dirs).unwrap(), "{file} is there");
    }
}

#[test]
fn a_file_that_is_not_there_is_reported_absent_even_in_a_folder_not_made_yet() {
    let dir = TempDir::new().unwrap();
    let dirs = no_app_dirs();
    let empty = dir.path().join("new-empty-folder");
    std::fs::create_dir(&empty).unwrap();
    for path in [
        empty.join("copy.stalx"),
        empty.join("map.png"),
        dir.path()
            .join("not-created")
            .join("deeper")
            .join("copy.stalx"),
    ] {
        assert!(
            !probe(&path, &dirs).unwrap(),
            "{} is not there",
            path.display()
        );
    }
}

/// Anything at the name is something the write would replace - a folder, a link
/// whose target is gone - so it counts as there.
#[test]
fn a_folder_or_a_dangling_link_at_the_name_counts_as_there() {
    let dir = TempDir::new().unwrap();
    let dirs = no_app_dirs();
    let folder = dir.path().join("odd.stalx");
    std::fs::create_dir(&folder).unwrap();
    assert!(probe(&folder, &dirs).unwrap(), "a folder at the name");

    #[cfg(unix)]
    {
        let link = dir.path().join("dangling.csv");
        std::os::unix::fs::symlink(dir.path().join("gone.csv"), &link).unwrap();
        assert!(probe(&link, &dirs).unwrap(), "a dangling link at the name");
    }
}

/// The app's own folders are refused, not answered - even for a file that is
/// there - exactly as the write after it would be refused.
#[test]
fn the_app_folders_are_refused_without_an_answer() {
    let app = TempDir::new().unwrap();
    let dirs = AppDirs {
        internal: vec![app.path().to_path_buf()],
        managed_projects: Some(app.path().join("speleodb")),
    };
    let stored = app.path().join("settings.json");
    std::fs::write(&stored, b"{}").unwrap();

    let err = probe(&stored, &dirs).unwrap_err();
    assert_eq!(err.code, CommandErrorCode::WritePathRefused);
    assert!(
        err.message.contains("app's own settings"),
        "{}",
        err.message
    );

    // A project in the SpeleoDB managed-project cache is where projects live, as
    // for the save itself.
    let managed = app.path().join("speleodb").join("CaveA.stalx");
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, b"project").unwrap();
    assert!(
        probe(&managed, &dirs).unwrap(),
        "a managed project is answered"
    );
}

#[test]
fn a_file_type_the_app_does_not_write_is_refused() {
    let dir = TempDir::new().unwrap();
    let dirs = no_app_dirs();
    for file in ["config.toml", ".bashrc", "noext"] {
        let path = dir.path().join(file);
        std::fs::write(&path, b"user data").unwrap();
        let err = probe(&path, &dirs).unwrap_err();
        assert!(
            err.message.contains("writes no file of that type"),
            "{file}: {}",
            err.message
        );
    }
}

/// Through the command itself: a system folder is refused before anything is
/// looked at, as the save there would be.
#[cfg(unix)]
#[test]
fn the_command_refuses_a_system_folder() {
    for path in [
        "/etc/stalagmight-hfi2.stalx",
        "/usr/share/stalagmight-hfi2.png",
    ] {
        let answer = tauri::async_runtime::block_on(save_target_exists(path.into()));
        println!("{path}: {}", verdict(&answer));
        let err = answer.unwrap_err();
        assert!(
            err.message.contains("inside a system folder"),
            "{path}: {}",
            err.message
        );
    }
}

/// [#576 HFI-2-D2] Every refusal by the write-path policy carries its own code,
/// `WRITE_PATH_REFUSED`: the write after the probe refuses the same path with the
/// reason, so the frontend goes straight to it instead of first asking "could
/// not check whether it exists". The probe used to refuse with the generic
/// `FAILED`, which the frontend cannot tell from a check that failed.
#[test]
fn hfi2_d2_a_policy_refusal_carries_the_refusal_code() {
    let app = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let dirs = AppDirs {
        internal: vec![app.path().to_path_buf()],
        managed_projects: None,
    };
    let mut refused = vec![
        app.path().join("CaveA.stalx"),
        user.path().join("config.toml"),
    ];
    if cfg!(unix) {
        refused.push("/etc/stalagmight-hfi2.stalx".into());
    }
    for path in refused {
        let err = probe(&path, &dirs).unwrap_err();
        assert_eq!(
            err.code.as_str(),
            "WRITE_PATH_REFUSED",
            "{}: {}",
            path.display(),
            err.message
        );
        assert!(err.message.starts_with("Refused to"), "{}", err.message);
    }
}

/// A folder the OS will not let us look into: the check fails, and says so with
/// the permission code - it must not read as "not there", which would save
/// without asking.
#[cfg(unix)]
#[test]
fn a_folder_that_may_not_be_searched_is_an_error_not_absent() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("CaveA.stalx"), b"project").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root (CAP_DAC_OVERRIDE) ignores the mode bits; fail rather than pass untested.
    crate::models::test_support::assert_dir_refuses_new_files(&locked);

    let answer = probe(&locked.join("CaveA.stalx"), &no_app_dirs());

    // Restore BEFORE asserting so the TempDir can be removed.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = answer.unwrap_err();
    assert_eq!(
        err.code,
        CommandErrorCode::FsPermissionDenied,
        "{}",
        err.message
    );
    assert!(
        err.message
            .contains("Could not check whether CaveA.stalx already exists"),
        "{}",
        err.message
    );
}
