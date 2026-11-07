//! [#576 RV4-R-M3-OPEN] A project kept in the app's own folders or a system folder
//! opens like any other; only the write a load would make there is refused.
//!
//! Opening a project reads it, with one exception: an older-format file is
//! upgraded IN PLACE (the migration chain) and a `<name>.stalx.pre-v<N>.backup` is
//! written beside it first. The load commands took any webview path, so a page
//! script past the CSP could upgrade an old `.stalx` anywhere and drop a sibling
//! next to it. The first answer refused the whole load in those folders, which
//! also refused viewing and exporting a project kept there. Now the file opens,
//! and the upgrade alone is refused - before anything is written - with a message
//! saying to copy the project somewhere writable. The SpeleoDB managed-project
//! cache is where projects legitimately live, and is not refused anything.
//!
//! Here: which locations refuse the upgrade, the message, and that both load
//! commands hand the refusal to the load. What the load does with it (opens a
//! current file untouched; refuses an older one before the first write, with no
//! backup) is `core::state::protected_folder_load_tests`.
//!
//! All paths are synthetic.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::commands::write_path::{load_upgrade_refusal_with, AppDirs};

/// The app's own folders, inside `app`.
fn app_dirs(app: &Path) -> AppDirs {
    AppDirs {
        internal: vec![app.join("config"), app.join("cache")],
        managed_projects: Some(app.join("cache").join("speleodb")),
    }
}

#[test]
fn only_the_app_folders_and_system_folders_refuse_the_upgrade() {
    let app = TempDir::new().expect("app dir");
    let user = TempDir::new().expect("user dir");
    let dirs = app_dirs(app.path());
    let cases: Vec<(&str, PathBuf, bool)> = vec![
        ("a user's project", user.path().join("CaveL.stalx"), false),
        (
            "a user's file with another extension",
            user.path().join("CaveL.bak"),
            false,
        ),
        (
            "a SpeleoDB managed copy",
            app.path().join("cache/speleodb/p1/project.stalx"),
            false,
        ),
        (
            "inside the app config",
            app.path().join("config/CaveL.stalx"),
            true,
        ),
        (
            "inside the app cache",
            app.path().join("cache/CaveL.stalx"),
            true,
        ),
        (
            "inside a system folder",
            PathBuf::from("/etc/CaveL.stalx"),
            !cfg!(windows),
        ),
    ];
    let mut wrong = Vec::new();
    for (what, path, refused) in cases {
        let got = load_upgrade_refusal_with(&path, &dirs);
        println!("{what}: {got:?}");
        if got.is_some() != refused {
            wrong.push(what);
        }
        if let Some(refusal) = got {
            for message in [&refusal.older_format, &refusal.unstamped] {
                assert!(
                    message.starts_with("CaveL.stalx ") && message.contains("Copy the project"),
                    "{what}: the message names the file and says what to do: {message}"
                );
            }
            // [#576 RV6b-R3] An unstamped file is not an older one, and the open
            // would keep no backup copy of it.
            assert!(
                refusal.older_format.contains("older version")
                    && !refusal.unstamped.contains("older version")
                    && !refusal.unstamped.contains("backup"),
                "{what}: each message describes its own case: {refusal:?}"
            );
        }
    }
    assert!(wrong.is_empty(), "wrong verdicts: {wrong:?}");
}

#[test]
fn both_load_commands_hand_the_location_to_the_load() {
    let source = include_str!("project.rs");
    for command in [
        "pub async fn load_project_full(",
        "pub async fn load_project_full_binary(",
    ] {
        let body = source
            .split_once(command)
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap_or_default();
        let refusal = body.find("load_upgrade_refusal(&path)");
        let load = body.find(
            "do_load_project_full_with(path, upgrade_refusal.as_ref(), load_token.as_deref())",
        );
        println!("{command}: refusal at {refusal:?}, load at {load:?}");
        assert!(
            refusal.is_some() && refusal < load,
            "{command} must work out the location's refusal and hand it to the load"
        );
    }
}
