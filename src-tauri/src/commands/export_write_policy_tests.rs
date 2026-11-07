//! [#570 x #576] An Ariane export is atomic AND goes through the write-path policy.
//!
//! #570 (master) and #576 B9 both made exports replace their destination through
//! a staged temp file and a rename; the merge kept one writer,
//! `export::atomic::write_atomic`. Before it runs, every export command runs the
//! write-path policy (`commands::write_path::check_write_path`, #576 C11-D1):
//! the extension of the format, not an app or system folder, judged on the path
//! as the write reaches it on disk - a link at the name, or anywhere on the way,
//! resolved one component at a time - and the Windows refusals. These drive the
//! registered `export_tml` command, so the two guarantees are checked together,
//! not each in its own module:
//!
//! - a link at the name to a file of another type is refused before anything is
//!   written, and the file it points at is left alone;
//! - a link to an Ariane file is followed, policy-checked, and that file is
//!   REPLACED by a rename (a second hard link still holds the old bytes), the
//!   link kept;
//! - a refused path writes nothing, and a failed write leaves the old file.
//!
//! All files are synthetic, in temp dirs.

use std::path::{Path, PathBuf};

use tauri::Manager;
use tempfile::TempDir;

use super::command_tests::{h, run};
use crate::commands::export;
use crate::commands::write_path::{check_write_path_with, AppDirs, WriteTarget};
use crate::core::AppState;
use stalagmight_core::test_fixtures::state_with_project;

/// What a destination held before the export: the previous good copy.
const OLD: &[u8] = b"OLD ARIANE EXPORT - the copy the user still has\n";

/// A mock app with a one-section project open, saved into `dir`.
fn app_with_project(dir: &Path) -> tauri::App<tauri::test::MockRuntime> {
    let state = state_with_project("CaveX");
    let saved = state.execute(crate::core::Command::SaveProject {
        path: Some(dir.join("CaveX.stalx")),
    });
    assert!(saved.success, "{:?}", saved.error);
    let app = tauri::test::mock_app();
    app.manage::<AppState>(state);
    app
}

/// Every entry in `dir`, sorted, so a stray temp file shows up by name.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read the dir")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// Whether `path` holds a new Ariane export: it parses as one, and it is not the
/// copy that was there before.
fn holds_a_new_export(path: &Path) -> bool {
    crate::parsers::tml::parse_tml(path).is_ok() && std::fs::read(path).is_ok_and(|b| b != OLD)
}

#[cfg(unix)]
#[test]
fn an_export_through_a_link_to_a_file_of_another_type_is_refused_and_writes_nothing() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let app = app_with_project(dir.path());
    let victim = dir.path().join("victim.stalx");
    std::fs::write(&victim, b"not an export").expect("the victim");

    for name in ["cave.tml", "cave.tmlu"] {
        let link = dir.path().join(name);
        std::os::unix::fs::symlink(&victim, &link).expect("the link");

        let refused = run(export::export_tml(h(&app), link.clone()))
            .expect_err("a .tml name that leads to a .stalx is refused");
        println!("{name} -> victim.stalx: {}", refused.message);
        assert!(
            refused.message.contains("is a link to victim.stalx"),
            "{name}: {}",
            refused.message
        );
        assert_eq!(
            std::fs::read(&victim).expect("read"),
            b"not an export",
            "{name}"
        );
        assert_eq!(
            std::fs::read_link(&link).expect("still a link"),
            victim,
            "{name}"
        );
        std::fs::remove_file(&link).expect("drop the link");
    }
    assert_eq!(names_in(dir.path()), ["CaveX.stalx", "victim.stalx"]);
}

#[cfg(unix)]
#[test]
fn an_export_through_a_link_to_an_ariane_file_replaces_that_file_by_rename_and_keeps_the_link() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let app = app_with_project(dir.path());

    for ext in ["tml", "tmlu"] {
        let real = dir.path().join(format!("real.{ext}"));
        std::fs::write(&real, OLD).expect("the previous export");
        // A second name for the OLD bytes: an in-place rewrite would change them too.
        let pin = dir.path().join(format!("previous-{ext}"));
        std::fs::hard_link(&real, &pin).expect("pin the old bytes");
        let link = dir.path().join(format!("cave.{ext}"));
        std::os::unix::fs::symlink(&real, &link).expect("the link");

        run(export::export_tml(h(&app), link.clone())).expect("the export goes through");

        assert_eq!(
            std::fs::read_link(&link).expect("still a link"),
            real,
            "{ext}"
        );
        assert!(
            holds_a_new_export(&real),
            "{ext}: the link's file holds the new export"
        );
        assert_eq!(
            std::fs::read(&pin).expect("read"),
            OLD,
            "{ext}: replaced, not rewritten in place"
        );
    }
    // No temp file is left beside either destination.
    assert_eq!(
        names_in(dir.path()),
        [
            "CaveX.stalx",
            "cave.tml",
            "cave.tmlu",
            "previous-tml",
            "previous-tmlu",
            "real.tml",
            "real.tmlu"
        ]
    );
}

#[test]
fn an_export_to_a_refused_path_writes_nothing() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let app = app_with_project(dir.path());

    // Not an Ariane file name.
    let wrong = dir.path().join("cave.stalx");
    let refused = run(export::export_tml(h(&app), wrong.clone())).expect_err("refused");
    assert!(
        refused.message.contains("must end in .tml or .tmlu"),
        "{}",
        refused.message
    );
    assert!(!wrong.exists());

    // A system folder.
    #[cfg(unix)]
    {
        let system = PathBuf::from("/etc/stalagmight-sync2-export.tml");
        let refused = run(export::export_tml(h(&app), system.clone())).expect_err("refused");
        assert!(
            refused.message.contains("system folder"),
            "{}",
            refused.message
        );
        assert!(!system.exists());
    }
    assert_eq!(names_in(dir.path()), ["CaveX.stalx"]);
}

/// The app-folder rule, through a link on the way: `<user>/to-app/cave.tml`,
/// where `to-app` leads into the app's own folder, is judged where the write
/// lands. (The command reads the app folders the process resolved at startup,
/// which a unit test has none of, so the policy is asked with given ones.)
#[cfg(unix)]
#[test]
fn an_export_reached_through_a_link_into_the_app_folder_is_refused() {
    let app_dir = TempDir::new().expect("app dir");
    let user = TempDir::new().expect("user dir");
    std::os::unix::fs::symlink(app_dir.path(), user.path().join("to-app")).expect("the link");
    let dirs = AppDirs {
        internal: vec![app_dir.path().to_path_buf()],
        managed_projects: None,
    };

    let through = user.path().join("to-app").join("cave.tml");
    let why = check_write_path_with(&through, WriteTarget::Ariane, &dirs).expect_err("refused");
    assert!(
        why.contains("app's own settings, data or cache folder"),
        "{why}"
    );
    let beside = user.path().join("cave.tml");
    assert_eq!(
        check_write_path_with(&beside, WriteTarget::Ariane, &dirs),
        Ok(())
    );
}

/// A rename needs to make a file in the folder; an in-place rewrite only needs
/// the file. So a re-export into a folder that takes no new file fails - and the
/// previous export is exactly as it was, not truncated.
#[cfg(unix)]
#[test]
fn a_failed_export_keeps_the_previous_file() {
    use std::os::unix::fs::PermissionsExt;
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let app = app_with_project(dir.path());
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).expect("the folder");
    let target = locked.join("cave.tml");
    std::fs::write(&target, OLD).expect("the previous export");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).expect("lock");
    // Root writes into 0555 anyway, and then there is no failure to observe.
    crate::models::test_support::assert_dir_refuses_new_files(&locked);

    let failed = run(export::export_tml(h(&app), target.clone()));

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("unlock");
    let err = failed.expect_err("the export cannot stage its file");
    assert!(err.message.contains("temporary file"), "{}", err.message);
    assert_eq!(std::fs::read(&target).expect("read"), OLD);
    assert_eq!(names_in(&locked), ["cave.tml"]);
}
