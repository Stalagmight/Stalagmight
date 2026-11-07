//! [#576 RV6b-R4] How the app/system-folder check matches a path against a
//! folder: letter case, symlinks and Windows verbatim (`\\?\`) forms.
//!
//! The comparison is pure text under a platform's rules (`PathRules`), so each
//! platform's rules are tested here on every host: case folds on Windows and
//! macOS (APFS and HFS+ are case-insensitive by default) and not on Linux;
//! Windows' verbatim prefixes and forward slashes name the same folder as the
//! plain form. The symlink cases go through the real check on a Unix host.
//!
//! All paths are synthetic.

use std::path::{Path, PathBuf};

use crate::commands::write_path::{
    check_write_path_under, check_write_path_with, load_upgrade_refusal_with, path_is_within,
    resolved, AppDirs, LinkLookup, PathRules, Walk, WriteTarget,
};

const LINUX: PathRules = PathRules {
    windows: false,
    fold_case: false,
};
const MACOS: PathRules = PathRules {
    windows: false,
    fold_case: true,
};
const WINDOWS: PathRules = PathRules {
    windows: true,
    fold_case: true,
};

#[test]
fn the_host_rules_fold_case_where_the_file_system_does() {
    println!("host path rules: {:?}", PathRules::HOST);
    assert_eq!(PathRules::HOST.windows, cfg!(windows));
    assert_eq!(
        PathRules::HOST.fold_case,
        cfg!(any(windows, target_os = "macos")),
        "Windows and macOS file systems are case-insensitive by default"
    );
}

#[test]
fn each_platform_matches_a_path_against_a_folder_by_its_own_rules() {
    let cases: &[(&str, &str, &str, PathRules, bool)] = &[
        // Case.
        (
            "linux, other case",
            "/USR/share/CaveL.stalx",
            "/usr",
            LINUX,
            false,
        ),
        (
            "macos, other case",
            "/USR/share/CaveL.stalx",
            "/usr",
            MACOS,
            true,
        ),
        (
            "macos, app folder in other case",
            "/Users/a/library/application support/stalagmight/CaveL.stalx",
            "/Users/a/Library/Application Support/Stalagmight",
            MACOS,
            true,
        ),
        (
            "windows, other case",
            r"c:\WINDOWS\System32\CaveL.stalx",
            r"C:\Windows",
            WINDOWS,
            true,
        ),
        // Verbatim and separator forms.
        (
            "windows, verbatim path",
            r"\\?\C:\Windows\CaveL.stalx",
            r"C:\Windows",
            WINDOWS,
            true,
        ),
        (
            "windows, verbatim folder",
            r"C:\Program Files\CaveL.stalx",
            r"\\?\C:\Program Files",
            WINDOWS,
            true,
        ),
        (
            "windows, verbatim UNC",
            r"\\?\UNC\server\share\AppData\CaveL.stalx",
            r"\\server\share\AppData",
            WINDOWS,
            true,
        ),
        (
            "windows, forward slashes",
            "C:/Windows/CaveL.stalx",
            r"C:\Windows",
            WINDOWS,
            true,
        ),
        // Look-alikes that are not inside.
        (
            "linux, sibling prefix",
            "/usrlocal/CaveL.stalx",
            "/usr",
            LINUX,
            false,
        ),
        (
            "windows, sibling prefix",
            r"\\?\C:\WindowsOld\CaveL.stalx",
            r"C:\Windows",
            WINDOWS,
            false,
        ),
        (
            "windows, other drive",
            r"\\?\D:\Windows\CaveL.stalx",
            r"C:\Windows",
            WINDOWS,
            false,
        ),
        // [#576 RV8a-F5] A UNC path and a path rooted on the current drive that
        // spell the same names are different places.
        (
            "windows, UNC path vs a rooted path of the same names",
            r"\\server\share\AppData\CaveL.stalx",
            r"\server\share\AppData",
            WINDOWS,
            false,
        ),
        (
            "windows, rooted path vs a UNC path of the same names",
            r"\server\share\AppData\CaveL.stalx",
            r"\\server\share\AppData",
            WINDOWS,
            false,
        ),
        (
            "windows, UNC path inside a UNC folder",
            r"\\server\share\AppData\CaveL.stalx",
            r"\\server\share\AppData",
            WINDOWS,
            true,
        ),
        // Separators that do not change the place.
        (
            "linux, trailing separator",
            "/etc/CaveL.stalx",
            "/etc/",
            LINUX,
            true,
        ),
        (
            "linux, doubled separator",
            "/etc//CaveL.stalx",
            "/etc",
            LINUX,
            true,
        ),
        ("linux, the folder itself", "/etc", "/etc", LINUX, true),
    ];
    let mut wrong = Vec::new();
    for (what, path, dir, rules, inside) in cases {
        let got = path_is_within(path, dir, *rules);
        println!("{what}: {path:?} within {dir:?}: {got}");
        if got != *inside {
            wrong.push(*what);
        }
    }
    assert!(wrong.is_empty(), "wrong verdicts: {wrong:?}");
}

/// The real check, through a symlink each way. A link in a user's folder that
/// leads into the app's folders is refused (its resolved form is inside); a link
/// inside the app's folders that leads out is refused too (its spelled form is
/// inside) - the conservative side.
#[cfg(unix)]
#[test]
fn a_symlink_into_or_out_of_an_app_folder_is_refused() -> std::io::Result<()> {
    let root = tempfile::TempDir::new()?;
    let config = root.path().join("app/config");
    let user = root.path().join("user");
    std::fs::create_dir_all(&config)?;
    std::fs::create_dir_all(&user)?;
    std::os::unix::fs::symlink(&config, user.join("into-app"))?;
    std::os::unix::fs::symlink(&user, config.join("out-to-user"))?;
    let dirs = AppDirs {
        internal: vec![config.clone()],
        managed_projects: None,
    };
    let cases: [(&str, &Path, bool); 3] = [
        (
            "a user file through a link into the app config",
            &user.join("into-app/CaveL.stalx"),
            true,
        ),
        (
            "an app-config file through a link out to the user folder",
            &config.join("out-to-user/CaveL.stalx"),
            true,
        ),
        ("a plain user file", &user.join("CaveL.stalx"), false),
    ];
    let mut wrong = Vec::new();
    for (what, path, refused) in cases {
        let write = check_write_path_with(path, WriteTarget::Project, &dirs);
        let load = load_upgrade_refusal_with(path, &dirs);
        println!(
            "{what}: write refused: {}, load upgrade refused: {}",
            write.is_err(),
            load.is_some()
        );
        if write.is_err() != refused || load.is_some() != refused {
            wrong.push(what);
        }
    }
    assert!(wrong.is_empty(), "wrong verdicts: {wrong:?}");
    Ok(())
}

/// [#576 RV13-2] A model of the disk for the walk in `resolved`: links that
/// lead somewhere (each with the place it resolves to) and links that lead
/// nowhere yet (each with its target). Every other name is missing - kept as
/// the name a write would create. Pure: no file is touched, so each platform's
/// rules run on every host.
struct ModelDisk {
    resolves: Vec<(PathBuf, PathBuf)>,
    dangling: Vec<(PathBuf, PathBuf)>,
}

impl LinkLookup for ModelDisk {
    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        let found = self.resolves.iter().find(|(at, _)| at == path);
        found.map(|(_, to)| to.clone())
    }

    fn link_target(&self, path: &Path) -> Option<PathBuf> {
        let found = self.dangling.iter().find(|(at, _)| at == path);
        found.map(|(_, to)| to.clone())
    }
}

/// An absolute root on this host, with no file touched: `C:\` on Windows.
fn model_root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" }).join("model")
}

/// A chain of `n` links leading nowhere yet, `<user>/c<n>-1.stalx` ->
/// `c<n>-2.stalx` -> ... -> `c<n>-<n>.stalx` -> `to`.
fn dangling_chain(user: &Path, n: usize, to: &Path) -> Vec<(PathBuf, PathBuf)> {
    let link = |i: usize| user.join(format!("c{n}-{i}.stalx"));
    (1..=n)
        .map(|i| {
            let next = if i == n {
                to.to_path_buf()
            } else {
                link(i + 1)
            };
            (link(i), next)
        })
        .collect()
}

/// [#576 RV13-2] The walk modelled POSIX on every platform: `..` stepped back
/// from where a link led. Windows folds `..` by name before it follows any link,
/// so `<u>\j1\..\applink\x.stalx` (`j1` a junction elsewhere, `applink` one into
/// the app folder) is written INTO the app folder - the POSIX walk went to
/// `j1`'s target's parent and never met `applink`. The mirror shape
/// (`appsub\..\x.stalx`) lands in the user's folder on Windows, in the app
/// folder on Linux. The same for `..` in a link's target, taken from the link's
/// folder.
#[test]
fn each_platform_follows_dot_dot_and_links_in_its_own_order() {
    let m = model_root();
    let (user, app, elsewhere) = (m.join("u"), m.join("app"), m.join("elsewhere"));
    let disk = ModelDisk {
        resolves: vec![
            (user.join("j1"), elsewhere.join("t")),
            (user.join("applink"), app.clone()),
            (user.join("appsub"), app.join("sub")),
        ],
        dangling: vec![(
            user.join("t.stalx"),
            PathBuf::from("appsub").join("..").join("planted.stalx"),
        )],
    };
    let shown = |p: Option<&PathBuf>| {
        p.map(|p| {
            let p = p.strip_prefix(&m).map_or(p.clone(), Path::to_path_buf);
            p.display().to_string()
        })
    };
    let via_j1 = user.join("j1").join("..").join("applink").join("x.stalx");
    let via_appsub = user.join("appsub").join("..").join("x.stalx");
    let link_at = user.join("t.stalx");
    let cases = [
        (
            "j1/../applink/x.stalx",
            &via_j1,
            WINDOWS,
            app.join("x.stalx"),
        ),
        (
            "j1/../applink/x.stalx",
            &via_j1,
            LINUX,
            elsewhere.join("applink").join("x.stalx"),
        ),
        (
            "appsub/../x.stalx",
            &via_appsub,
            WINDOWS,
            user.join("x.stalx"),
        ),
        ("appsub/../x.stalx", &via_appsub, LINUX, app.join("x.stalx")),
        (
            "t.stalx -> appsub/../planted.stalx",
            &link_at,
            WINDOWS,
            user.join("planted.stalx"),
        ),
        (
            "t.stalx -> appsub/../planted.stalx",
            &link_at,
            LINUX,
            app.join("planted.stalx"),
        ),
    ];
    let mut wrong = Vec::new();
    for (what, path, rules, lands) in cases {
        let got = resolved(path, Walk { rules, disk: &disk });
        let platform = if rules.windows { "windows" } else { "linux" };
        println!(
            "RV13-2 {platform} {what}: lands at {:?}",
            shown(got.as_ref())
        );
        if got.as_ref() != Some(&lands) {
            wrong.push(format!(
                "{platform} {what}: {:?}, not {:?}",
                shown(got.as_ref()),
                shown(Some(&lands))
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// [#576 RV13-2] A chain of links that lead nowhere yet: Linux follows 40 (the
/// 41st is `ELOOP`, nothing written) and Windows 63. The walk capped at 40
/// everywhere and then judged link 41's own name - a name in the user's folder -
/// where Windows goes on to the target. Under Windows rules the walk now follows
/// 63 and refuses past them; Linux is unchanged.
#[test]
fn a_chain_of_dangling_links_is_followed_to_each_platform_s_own_limit() {
    let m = model_root();
    let (user, app) = (m.join("u"), m.join("app"));
    let planted = app.join("planted.stalx");
    let mut wrong = Vec::new();
    for (rules, n) in [(LINUX, 40), (LINUX, 41), (WINDOWS, 63), (WINDOWS, 64)] {
        let disk = ModelDisk {
            resolves: Vec::new(),
            dangling: dangling_chain(&user, n, &planted),
        };
        let platform = if rules.windows { "windows" } else { "linux" };
        let expected = match (rules.windows, n) {
            (_, 40 | 63) => Some(planted.clone()),
            (false, _) => Some(user.join(format!("c{n}-{n}.stalx"))),
            (true, _) => None,
        };
        let first = user.join(format!("c{n}-1.stalx"));
        let got = resolved(&first, Walk { rules, disk: &disk });
        println!("RV13-2 {platform} chain of {n}: lands at {got:?}");
        if got != expected {
            wrong.push(format!(
                "{platform} chain of {n}: {got:?}, not {expected:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// [#576 RV13-2] The policy under Windows rules, on any host: the folded path is
/// judged (refused in the app folder, allowed in the user's), and a chain of
/// links past the 63 Windows follows is refused, not judged by its last name.
#[test]
fn under_windows_rules_the_policy_judges_the_folded_path_and_refuses_past_the_cap() {
    let m = model_root();
    let (user, app, elsewhere) = (m.join("u"), m.join("app"), m.join("elsewhere"));
    let dirs = AppDirs {
        internal: vec![app.clone()],
        managed_projects: None,
    };
    let mut dangling = dangling_chain(&user, 63, &app.join("p63.stalx"));
    dangling.extend(dangling_chain(&user, 64, &user.join("p64.stalx")));
    let disk = ModelDisk {
        resolves: vec![
            (user.join("j1"), elsewhere.join("t")),
            (user.join("applink"), app.clone()),
            (user.join("appsub"), app.join("sub")),
        ],
        dangling,
    };
    let walk = Walk {
        rules: WINDOWS,
        disk: &disk,
    };
    let in_app = Some("app's own");
    let cases = [
        (
            "j1/../applink/x.stalx",
            user.join("j1").join("..").join("applink").join("x.stalx"),
            in_app,
        ),
        (
            "appsub/../x.stalx",
            user.join("appsub").join("..").join("x.stalx"),
            None,
        ),
        (
            "chain of 63 into the app folder",
            user.join("c63-1.stalx"),
            in_app,
        ),
        (
            "chain of 64 into the user's folder",
            user.join("c64-1.stalx"),
            Some("more links than Windows follows"),
        ),
    ];
    let mut wrong = Vec::new();
    for (what, path, refused) in cases {
        let got = check_write_path_under(&path, WriteTarget::Project, &dirs, walk);
        println!("RV13-2 windows policy {what}: {got:?}");
        let as_expected = match (&got, refused) {
            (Ok(()), None) => true,
            (Err(why), Some(reason)) => why.contains(reason),
            _ => false,
        };
        if !as_expected {
            wrong.push(format!("{what}: {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
