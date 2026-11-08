//! [#576 C11-D1] Every command that writes, replaces or deletes a file at a path
//! the webview hands it checks that path first: the right extension, not inside
//! the app's own folders, not inside a system folder.
//!
//! All files are synthetic, in temp dirs.

use std::path::{Path, PathBuf};

use tauri::Manager;
use tempfile::TempDir;

use super::command_tests::{h, made_for, run};
use crate::commands::write_path::{check_write_path_with, AppDirs, WriteTarget};
use crate::core::AppState;
use crate::export::ImageFormat;
use stalagmight_core::test_fixtures::state_with_project;

fn no_app_dirs() -> AppDirs {
    AppDirs {
        internal: Vec::new(),
        managed_projects: None,
    }
}

fn verdict(path: &Path, target: WriteTarget, dirs: &AppDirs) -> String {
    match check_write_path_with(path, target, dirs) {
        Ok(()) => "allowed".to_owned(),
        Err(why) => why,
    }
}

/// A project with one section, open, so the exports have something to write.
fn open_project_state(dir: &Path) -> AppState {
    let state = state_with_project("CaveW");
    let saved = state.execute(crate::core::Command::SaveProject {
        path: Some(dir.join("CaveW.stalx")),
    });
    assert!(saved.success, "{:?}", saved.error);
    state
}

#[test]
fn the_extension_must_be_the_one_being_written() {
    let dir = TempDir::new().expect("tempdir");
    let dirs = no_app_dirs();
    for (file, target, allowed) in [
        ("a.csv", WriteTarget::Csv, true),
        ("a.CSV", WriteTarget::Csv, true),
        ("config.toml", WriteTarget::Csv, false),
        ("a.json", WriteTarget::Json, true),
        ("a.tmlu", WriteTarget::Ariane, true),
        ("a.png", WriteTarget::Image(ImageFormat::Png), true),
        ("a.svg", WriteTarget::Image(ImageFormat::Png), false),
        ("a.stalx", WriteTarget::Project, true),
        ("notes.txt", WriteTarget::Project, false),
        ("noext", WriteTarget::Project, false),
    ] {
        let got = verdict(&dir.path().join(file), target, &dirs);
        println!("{file} as {target:?}: {got}");
        assert_eq!(got == "allowed", allowed, "{file} as {target:?}: {got}");
    }
}

#[test]
fn the_app_s_own_folders_are_refused_except_the_managed_project_cache() {
    let config = TempDir::new().expect("config dir");
    let cache = TempDir::new().expect("cache dir");
    let user = TempDir::new().expect("user dir");
    let dirs = AppDirs {
        internal: vec![config.path().to_path_buf(), cache.path().to_path_buf()],
        managed_projects: Some(cache.path().join("speleodb")),
    };
    let cases: [(PathBuf, WriteTarget, bool); 5] = [
        (
            config.path().join("settings.json"),
            WriteTarget::Json,
            false,
        ),
        (
            config.path().join("sub").join("x.stalx"),
            WriteTarget::Project,
            false,
        ),
        (
            cache.path().join("speleodb").join("p").join("x.stalx"),
            WriteTarget::Project,
            true,
        ),
        (
            cache.path().join("speleodb").join("x.csv"),
            WriteTarget::Csv,
            false,
        ),
        (user.path().join("x.stalx"), WriteTarget::Project, true),
    ];
    for (path, target, allowed) in cases {
        let got = verdict(&path, target, &dirs);
        println!("{}: {got}", path.display());
        assert_eq!(got == "allowed", allowed, "{}: {got}", path.display());
    }
}

/// A folder inside the managed-project cache that is a link into another of the
/// app's own folders does not carry the cache's exception: the write would land
/// in the config folder, so it is refused (Codex review on #594).
#[cfg(unix)]
#[test]
fn a_link_from_the_managed_cache_into_another_app_folder_is_refused() {
    let config = TempDir::new().expect("config dir");
    let cache = TempDir::new().expect("cache dir");
    let managed = cache.path().join("speleodb");
    std::fs::create_dir_all(&managed).expect("managed dir");
    std::os::unix::fs::symlink(config.path(), managed.join("p")).expect("link");
    let dirs = AppDirs {
        internal: vec![config.path().to_path_buf(), cache.path().to_path_buf()],
        managed_projects: Some(managed.clone()),
    };
    let through_link = managed.join("p").join("x.stalx");
    let got = verdict(&through_link, WriteTarget::Project, &dirs);
    assert!(
        got.contains("app's own settings, data or cache folder"),
        "a project path spelled under the cache but landing in the config folder: {got}"
    );
    // A real project inside the cache keeps its exception.
    let real = managed.join("q").join("x.stalx");
    assert_eq!(verdict(&real, WriteTarget::Project, &dirs), "allowed");
}

#[cfg(unix)]
#[test]
fn system_folders_are_refused_and_user_mounts_are_not() {
    let dirs = no_app_dirs();
    for (path, allowed) in [
        ("/etc/stalagmight-w4.csv", false),
        ("/usr/share/stalagmight-w4.csv", false),
        ("/etc/../etc/stalagmight-w4.csv", false),
        ("/run/media/someone/USB/cave.csv", true),
        ("/var/home/someone/cave.csv", true),
        ("/mnt/data/cave.csv", true),
    ] {
        let got = verdict(Path::new(path), WriteTarget::Csv, &dirs);
        println!("{path}: {got}");
        assert_eq!(got == "allowed", allowed, "{path}: {got}");
    }
}

#[test]
fn the_commands_refuse_before_writing_or_deleting() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let app = tauri::test::mock_app();
    app.manage(open_project_state(dir.path()));

    // An export named to a file that is not of its format: nothing written.
    let toml = dir.path().join("config.toml");
    let export = run(crate::commands::export::export_csv(h(&app), toml.clone()));
    println!(
        "export_csv to config.toml: {export:?}; file written: {}",
        toml.exists()
    );
    assert!(export.is_err(), "refused");
    assert!(!toml.exists(), "and nothing was written");

    // Delete a file that is not a project: nothing deleted.
    let keep = dir.path().join("keep.txt");
    std::fs::write(&keep, b"user data").expect("write the bystander");
    let deleted = run(crate::commands::project::delete_project(
        h(&app),
        keep.clone(),
    ));
    println!(
        "delete_project on keep.txt: success={} error={:?}; file still there: {}",
        deleted.is_ok(),
        deleted.as_ref().err(),
        keep.exists()
    );
    assert!(deleted.is_err(), "refused");
    assert!(keep.exists(), "and nothing was deleted");

    // Save As into a system folder.
    #[cfg(unix)]
    {
        let saved = run(crate::commands::project::save_project(
            h(&app),
            made_for(&app),
            Some(PathBuf::from("/etc/stalagmight-w4.stalx")),
            None,
        ));
        println!(
            "save as /etc/stalagmight-w4.stalx: {:?}",
            saved.as_ref().err()
        );
        assert!(saved.is_err());
        assert!(saved
            .expect_err("a save into a system folder is refused")
            .message
            .contains("system folder"));
    }

    // The ordinary cases still go through.
    let csv = dir.path().join("CaveW.csv");
    let export = run(crate::commands::export::export_csv(h(&app), csv.clone()));
    assert!(export.is_ok(), "{export:?}");
    assert!(csv.exists());
}

/// [#576 RV12-1] A folder reached through a link that follows a folder not made
/// yet and a `..`: `<user>/nope/../app-link/x`, where `app-link` points into the
/// app's own folder and `nope` does not exist. The OS cannot walk through `nope`,
/// but a write that first makes the missing folders (creating a project does,
/// `create_dir_all` on the parent) then goes `nope` -> `..` -> through the link.
/// The policy compared the path folded without looking at the disk (which cannot
/// see the link) and the deepest existing ancestor resolved with the rest
/// appended as given (whose `..` then matched nothing), so this passed every
/// caller of the policy. Each caller is asked here, for every file type.
#[cfg(unix)]
#[test]
fn a_link_reached_through_a_missing_folder_and_dot_dot_is_refused_by_every_caller() {
    use crate::commands::recovery::write_salvaged_data_with;
    use crate::commands::save_target::save_target_exists_with;
    use crate::commands::write_path::{check_write_path, load_upgrade_refusal_with};

    let app = TempDir::new().expect("app dir");
    let user = TempDir::new().expect("user dir");
    std::fs::write(app.path().join("settings.json"), b"{}").expect("an app file");
    std::os::unix::fs::symlink(app.path(), user.path().join("app-link")).expect("app link");
    std::os::unix::fs::symlink("/etc", user.path().join("etc-link")).expect("etc link");
    let dirs = AppDirs {
        internal: vec![app.path().to_path_buf()],
        managed_projects: None,
    };
    let shapes = |link: &str, file: &str| -> Vec<PathBuf> {
        let u = user.path();
        vec![
            u.join("nope").join("..").join(link).join(file),
            u.join("nope").join(".").join("..").join(link).join(file),
            u.join("nope")
                .join("deeper")
                .join("..")
                .join("..")
                .join(link)
                .join(file),
        ]
    };
    let files = [
        "x.stalx", "x.csv", "x.json", "x.tmlu", "x.png", "x.svg", "x.pdf", "x.xml", "x.txt",
    ];
    let mut passed: Vec<String> = Vec::new();
    for file in files {
        let target = WriteTarget::of_path(Path::new(file)).expect("a file type the app writes");
        for path in shapes("app-link", file) {
            let got = verdict(&path, target, &dirs);
            println!("RV12-1 check_write_path {}: {got}", path.display());
            if got == "allowed" {
                passed.push(format!("check_write_path {}", path.display()));
            }
            let probe = save_target_exists_with(&path, &dirs);
            if !probe
                .as_ref()
                .is_err_and(|e| e.code.as_str() == "WRITE_PATH_REFUSED")
            {
                passed.push(format!("save_target_exists {}: {probe:?}", path.display()));
            }
        }
        // The system folder, through the policy with the app's real directories:
        // the check `create_project_full` runs before it makes the missing folders.
        for path in shapes("etc-link", file) {
            if check_write_path(&path, target).is_ok() {
                passed.push(format!("check_write_path (system) {}", path.display()));
            }
        }
    }
    for path in shapes("app-link", "x.stalx") {
        if load_upgrade_refusal_with(&path, &dirs).is_none() {
            passed.push(format!("load_upgrade_refusal {}", path.display()));
        }
    }
    let damaged = user.path().join("damaged.stalx");
    std::fs::write(&damaged, b"not a database").expect("a damaged project");
    for path in shapes("app-link", "x.json") {
        let wrote = write_salvaged_data_with(&damaged, &path, &dirs);
        let refused = wrote
            .as_ref()
            .is_err_and(|e| e.message.starts_with("Refused to change"));
        if !refused {
            passed.push(format!("write_salvaged_data {}: {wrote:?}", path.display()));
        }
    }
    println!("RV12-1 passed the policy: {} of them", passed.len());
    for line in &passed {
        println!("RV12-1   {line}");
    }
    assert!(
        passed.is_empty(),
        "{} paths passed the policy",
        passed.len()
    );

    // The same shape that lands in a user folder is still allowed.
    let mine = user.path().join("nope").join("..").join("CaveA.stalx");
    assert_eq!(verdict(&mine, WriteTarget::Project, &dirs), "allowed");
    assert!(!app.path().join("x.stalx").exists(), "nothing written");
}

/// [#576 NF-W13BE-1] A dangling link as the file itself: `<user>/x.stalx` ->
/// `<app>/planted.stalx`, a file that does not exist yet. A link that leads to
/// nothing cannot be canonicalized, so the policy kept its own name - a file in
/// the user's folder - and let it through. But a write that follows the link
/// lands on the target: creating a project opens the name with SQLite, which
/// follows the link and creates `<app>/planted.stalx` inside the app's own
/// folder. Each caller of the policy is asked here, for every file type and for
/// each link shape that reaches a missing target: absolute, relative, a chain of
/// dangling links, a target in a missing folder, and a system folder.
#[cfg(unix)]
#[test]
fn a_dangling_link_as_the_file_is_judged_by_its_target_by_every_caller() {
    use crate::commands::recovery::write_salvaged_data_with;
    use crate::commands::save_target::save_target_exists_with;
    use crate::commands::write_path::{check_write_path, load_upgrade_refusal_with};
    use std::os::unix::fs::symlink;

    let app = TempDir::new().expect("app dir");
    let user = TempDir::new().expect("user dir");
    let dirs = AppDirs {
        internal: vec![app.path().to_path_buf()],
        managed_projects: None,
    };
    let app_name = app.path().file_name().expect("app dir name");
    let files = [
        "x.stalx", "x.csv", "x.json", "x.tmlu", "x.png", "x.svg", "x.pdf", "x.xml", "x.txt",
    ];
    let mut passed: Vec<String> = Vec::new();
    for file in files {
        let target = WriteTarget::of_path(Path::new(file)).expect("a file type the app writes");
        let planted = format!("planted-{file}");
        let u = user.path();
        let mut links: Vec<PathBuf> = Vec::new();
        let mut link = |name: String, to: PathBuf| {
            let at = u.join(name);
            symlink(&to, &at).expect("a dangling link");
            links.push(at);
        };
        link(format!("abs-{file}"), app.path().join(&planted));
        link(
            format!("rel-{file}"),
            Path::new("..").join(app_name).join(&planted),
        );
        link(format!("hop-{file}"), app.path().join(&planted));
        link(format!("chain-{file}"), u.join(format!("hop-{file}")));
        link(
            format!("deep-{file}"),
            app.path().join("not-yet").join(&planted),
        );
        for path in &links {
            assert!(!path.exists(), "the link dangles: {}", path.display());
            let got = verdict(path, target, &dirs);
            println!("NF-W13BE-1 check_write_path {}: {got}", path.display());
            if got == "allowed" {
                passed.push(format!("check_write_path {}", path.display()));
            }
            let probe = save_target_exists_with(path, &dirs);
            if !probe
                .as_ref()
                .is_err_and(|e| e.code.as_str() == "WRITE_PATH_REFUSED")
            {
                passed.push(format!("save_target_exists {}: {probe:?}", path.display()));
            }
        }
        // The system folder, through the policy with the app's real directories.
        let etc = u.join(format!("etc-{file}"));
        symlink(Path::new("/etc").join(&planted), &etc).expect("a link into /etc");
        if check_write_path(&etc, target).is_ok() {
            passed.push(format!("check_write_path (system) {}", etc.display()));
        }
    }
    for name in [
        "abs-x.stalx",
        "rel-x.stalx",
        "chain-x.stalx",
        "deep-x.stalx",
    ] {
        let path = user.path().join(name);
        if load_upgrade_refusal_with(&path, &dirs).is_none() {
            passed.push(format!("load_upgrade_refusal {}", path.display()));
        }
    }
    let damaged = user.path().join("damaged.stalx");
    std::fs::write(&damaged, b"not a database").expect("a damaged project");
    for name in ["abs-x.json", "rel-x.json", "chain-x.json", "deep-x.json"] {
        let path = user.path().join(name);
        let wrote = write_salvaged_data_with(&damaged, &path, &dirs);
        let refused = wrote
            .as_ref()
            .is_err_and(|e| e.message.starts_with("Refused to change"));
        if !refused {
            passed.push(format!("write_salvaged_data {}: {wrote:?}", path.display()));
        }
    }
    println!("NF-W13BE-1 passed the policy: {} of them", passed.len());
    for line in &passed {
        println!("NF-W13BE-1   {line}");
    }
    assert!(
        passed.is_empty(),
        "{} paths passed the policy",
        passed.len()
    );

    // A dangling link whose target is in a user folder is still allowed.
    let mine = user.path().join("mine.stalx");
    symlink(user.path().join("CaveA.stalx"), &mine).expect("a link to a user file");
    assert_eq!(verdict(&mine, WriteTarget::Project, &dirs), "allowed");
    // A target of another file type is refused as that name would be.
    let other = user.path().join("other.stalx");
    symlink(user.path().join("notes.txt"), &other).expect("a link to another type");
    let got = verdict(&other, WriteTarget::Project, &dirs);
    println!("NF-W13BE-1 other.stalx -> notes.txt: {got}");
    assert!(got.ends_with("which is not a .stalx file"), "{got}");
    let entries = std::fs::read_dir(app.path()).expect("app dir").count();
    assert_eq!(entries, 0, "nothing written into the app folder");
}

/// [#576 RV13-5] A link at a name of the right type whose target is a file of
/// another type (or a folder, or a file with no extension) is refused - and the
/// message says the link's target is the problem. It used to say "the file must
/// end in .stalx" of a name that does end in .stalx.
#[cfg(unix)]
#[test]
fn a_link_to_a_file_of_another_type_is_refused_naming_its_target() {
    use std::os::unix::fs::symlink;

    let user = TempDir::new().expect("user dir");
    let dirs = no_app_dirs();
    let u = user.path();
    std::fs::write(u.join("notes.txt"), b"user notes").expect("a text file");
    std::fs::write(u.join("noext"), b"user data").expect("a file with no extension");
    std::fs::create_dir(u.join("folder")).expect("a folder");
    for (link, to, target, expected) in [
        (
            "n.stalx",
            "notes.txt",
            WriteTarget::Project,
            "notes.txt, which is not a .stalx file",
        ),
        (
            "d.stalx",
            "folder",
            WriteTarget::Project,
            "folder, which is not a .stalx file",
        ),
        (
            "e.stalx",
            "noext",
            WriteTarget::Project,
            "noext, which is not a .stalx file",
        ),
        (
            "m.stalx",
            "missing.txt",
            WriteTarget::Project,
            "missing.txt, which is not a .stalx file",
        ),
        (
            "a.tmlu",
            "notes.txt",
            WriteTarget::Ariane,
            "notes.txt, which is not a .tml or .tmlu file",
        ),
    ] {
        symlink(u.join(to), u.join(link)).expect("a link");
        let got = verdict(&u.join(link), target, &dirs);
        println!("RV13-5 {link} -> {to}: {got}");
        assert_eq!(
            got,
            format!("Refused to change {link}: it is a link to {expected}"),
            "{link} -> {to}"
        );
    }
    // A name of another type is still refused by its name.
    let got = verdict(&u.join("notes.txt"), WriteTarget::Project, &dirs);
    println!("RV13-5 notes.txt: {got}");
    assert_eq!(
        got,
        "Refused to change notes.txt: the file must end in .stalx"
    );
}

/// [#576 RV13-1] `create_project_full` with a folder and no file name ran the
/// policy on a stand-in `<folder>/project.stalx`, but the create opens
/// `<folder>/<Name>.stalx` - or the `_N` name it moves on to when that one is
/// taken. A link at that name leading nowhere yet, into the app's own folder,
/// passed: the create took it for a free name (`exists()` is false for a link
/// that leads nowhere) and SQLite created its target inside the app folder. The
/// same with a file named: `<user>/Taken.stalx` taken, `Taken_1.stalx` such a
/// link. And a stand-in link at `project.stalx` refused a create that never
/// touches that name. The policy now runs on the file the create opens, before
/// anything is created, and a link at a name is a name taken.
#[cfg(unix)]
#[test]
fn a_create_checks_the_file_it_opens_and_never_opens_a_link() {
    use crate::commands::project::{check_create_target_checked, create_project_full_checked};
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;

    let _cfg = crate::test_helpers::isolated_config();
    let app = TempDir::new().expect("app dir");
    let user = TempDir::new().expect("user dir");
    let u = user.path();
    let dirs = AppDirs {
        internal: vec![app.path().to_path_buf()],
        managed_projects: None,
    };
    let checked: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    let policy = |path: &Path| {
        checked.borrow_mut().push(path.to_path_buf());
        check_write_path_with(path, WriteTarget::Project, &dirs)
    };
    let state = stalagmight_core::test_fixtures::state_with_null_sink();
    let shown = |p: &Path| -> String {
        if let Ok(rest) = p.strip_prefix(u) {
            format!("<user>/{}", rest.display())
        } else if let Ok(rest) = p.strip_prefix(app.path()) {
            format!("<app>/{}", rest.display())
        } else {
            p.display().to_string()
        }
    };
    let mut wrong: Vec<String> = Vec::new();
    let mut create = |what: &str,
                      dir: Option<PathBuf>,
                      file: Option<PathBuf>,
                      want: Result<PathBuf, &str>,
                      want_checked: PathBuf| {
        // [#576 RV15-2] The webview's pre-check first, on the same disk: the same
        // verdict on the same file, and nothing made.
        checked.borrow_mut().clear();
        let pre = check_create_target_checked(&state, &policy, "Cave", dir.clone(), file.clone())
            .map_err(|e| e.message);
        let pre_ran_on = checked.borrow().clone();
        checked.borrow_mut().clear();
        let created = create_project_full_checked(
            &state,
            &policy,
            None,
            "Cave".into(),
            None,
            dir,
            file,
            false,
        )
        .map(|c| c.project.file_path.unwrap_or_default())
        .map_err(|e| e.message);
        let ran_on = checked.borrow().clone();
        let in_app: Vec<String> = std::fs::read_dir(app.path())
            .expect("app dir")
            .map(|e| shown(&e.expect("entry").path()))
            .collect();
        println!(
            "RV13-1 {what}: created {:?}; policy run on {:?}; the app folder holds {in_app:?}",
            created.as_deref().map(shown),
            ran_on.iter().map(|p| shown(p)).collect::<Vec<_>>(),
        );
        let as_wanted = match (&created, &want) {
            (Ok(at), Ok(want)) => at == want,
            (Err(why), Err(want)) => why.contains(want),
            _ => false,
        };
        let pre_agrees = match (&pre, &created) {
            (Ok(()), Ok(_)) => true,
            (Err(said), Err(why)) => said == why,
            _ => false,
        };
        println!(
            "RV15-2 {what}: pre-check {pre:?} on {:?}",
            pre_ran_on.iter().map(|p| shown(p)).collect::<Vec<_>>()
        );
        if !as_wanted || ran_on != [want_checked.clone()] || !in_app.is_empty() {
            wrong.push(what.to_owned());
        }
        if !pre_agrees || pre_ran_on != [want_checked] {
            wrong.push(format!("pre-check: {what}"));
        }
    };

    // A folder named; a dangling link at `<user>/Cave.stalx` into the app folder:
    // a name taken, so `Cave_1.stalx`, and that is the file checked.
    symlink(
        app.path().join("planted-create.stalx"),
        u.join("Cave.stalx"),
    )
    .expect("a link");
    create(
        "Cave in <user>, Cave.stalx -> <app>/planted-create.stalx",
        Some(u.to_path_buf()),
        None,
        Ok(u.join("Cave_1.stalx")),
        u.join("Cave_1.stalx"),
    );

    // A file named and taken; the `_1` name a dangling link into the app folder.
    std::fs::write(u.join("Taken.stalx"), b"a project").expect("a taken name");
    symlink(app.path().join("planted-1.stalx"), u.join("Taken_1.stalx")).expect("a link");
    create(
        "<user>/Taken.stalx (taken), Taken_1.stalx -> <app>/planted-1.stalx",
        None,
        Some(u.join("Taken.stalx")),
        Ok(u.join("Taken_2.stalx")),
        u.join("Taken_2.stalx"),
    );

    // A link at `project.stalx` is no reason to refuse a create of Cave.
    let other = u.join("other");
    std::fs::create_dir(&other).expect("a user folder");
    symlink(
        app.path().join("planted-p.stalx"),
        other.join("project.stalx"),
    )
    .expect("a link");
    create(
        "Cave in <user>/other, project.stalx -> <app>/planted-p.stalx",
        Some(other.clone()),
        None,
        Ok(other.join("Cave.stalx")),
        other.join("Cave.stalx"),
    );

    // A refusal comes before anything is made - not the missing folder either.
    let refused_dir = app.path().join("new-folder");
    create(
        "Cave in <app>/new-folder",
        Some(refused_dir.clone()),
        None,
        Err("app's own settings, data or cache folder"),
        refused_dir.join("Cave.stalx"),
    );

    println!("RV13-1 not as they should be: {wrong:?}");
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// [#576 RV15-2] The create checks nothing at the default location, so neither
/// does its pre-check (which would otherwise resolve, and so create, the default
/// folder).
#[test]
fn the_pre_check_of_a_create_at_the_default_location_checks_nothing() {
    use crate::commands::project::check_create_target_checked;

    let state = stalagmight_core::test_fixtures::state_with_null_sink();
    let ran = std::cell::Cell::new(false);
    let refuse = |_: &Path| {
        ran.set(true);
        Err("refused".to_string())
    };
    assert!(check_create_target_checked(&state, &refuse, "Cave", None, None).is_ok());
    assert!(
        !ran.get(),
        "the default location is not the webview's to name"
    );
}

/// [#576 RV13-2] Linux, on the disk, unchanged by the Windows rules: a chain of
/// 40 links leading nowhere into the app folder is refused (the kernel creates
/// through 40); a chain of 41 is judged by its last link's own name and allowed -
/// and no write gets through it: the kernel stops at 40 (`ELOOP`) and nothing is
/// created.
#[cfg(target_os = "linux")]
#[test]
fn on_linux_a_chain_past_40_links_is_one_no_write_gets_through() {
    use std::os::unix::fs::symlink;

    let app = TempDir::new().expect("app dir");
    let user = TempDir::new().expect("user dir");
    let dirs = AppDirs {
        internal: vec![app.path().to_path_buf()],
        managed_projects: None,
    };
    for n in [40_usize, 41] {
        let link = |i: usize| user.path().join(format!("c{n}-{i}.stalx"));
        let planted = app.path().join(format!("planted-{n}.stalx"));
        for i in 1..=n {
            let to = if i == n { planted.clone() } else { link(i + 1) };
            symlink(&to, link(i)).expect("a link");
        }
        let got = verdict(&link(1), WriteTarget::Project, &dirs);
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(link(1));
        println!(
            "RV13-2 linux chain of {n}: policy {got}; kernel open(O_CREAT): {:?}; planted \
             exists: {}",
            opened
                .as_ref()
                .map(|_| ())
                .map_err(std::io::Error::raw_os_error),
            planted.exists()
        );
        if n == 40 {
            assert!(got.contains("app's own"), "{got}");
            assert!(
                opened.is_ok() && planted.exists(),
                "the kernel creates through 40"
            );
        } else {
            assert_eq!(got, "allowed");
            assert!(
                opened.is_err() && !planted.exists(),
                "ELOOP: nothing written"
            );
        }
    }
}
