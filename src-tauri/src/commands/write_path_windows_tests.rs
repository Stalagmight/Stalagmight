//! [#576 W24] The write-path policy's Windows path model (`PathRules` with
//! `windows: true`) against the spellings Windows accepts for one place: the
//! network-path forms, the DOS device prefixes, separators, letter case and the
//! trailing dots and spaces Win32 drops from a name. Pure text, so every host
//! runs it; no Windows host was used to confirm the Windows behaviour each case
//! states.
//!
//! All paths are synthetic.

use std::path::Path;

use crate::commands::write_path::{
    check_write_path_with, path_is_within, AppDirs, PathRules, WriteTarget,
};

const WINDOWS: PathRules = PathRules {
    windows: true,
    fold_case: true,
};

/// The cases whose verdict is not the expected one, each `(what, path, folder,
/// inside)` judged under Windows rules, every verdict printed.
fn wrong_verdicts(cases: &[(&str, &str, &str, bool)]) -> Vec<String> {
    let mut wrong = Vec::new();
    for (what, path, dir, inside) in cases {
        let got = path_is_within(path, dir, WINDOWS);
        println!("W24 {what}: {path:?} within {dir:?}: {got}");
        if got != *inside {
            wrong.push(format!("{what}: {got}, not {inside}"));
        }
    }
    wrong
}

/// Every spelling of a path on a network share is that UNC path: the verbatim
/// (`\\?\UNC\`) and the device (`\\.\UNC\`) prefix, in any letter case -
/// Windows looks `UNC` up in its object namespace, which ignores case - and with
/// either separator. A relative folder that happens to be called `unc`, and a
/// folder rooted on the current drive, are not.
#[test]
fn every_spelling_of_a_network_path_is_the_unc_path() {
    let share = r"\\server\share\AppData";
    let cases = [
        ("plain", r"\\server\share\AppData\CaveL.stalx", share, true),
        (
            "server and share in upper case",
            r"\\SERVER\SHARE\AppData\CaveL.stalx",
            share,
            true,
        ),
        (
            "forward slashes",
            "//server/share/AppData/CaveL.stalx",
            share,
            true,
        ),
        (
            "mixed separators",
            r"\/server/share\AppData/CaveL.stalx",
            share,
            true,
        ),
        (
            "verbatim",
            r"\\?\UNC\server\share\AppData\CaveL.stalx",
            share,
            true,
        ),
        (
            "verbatim, lower case",
            r"\\?\unc\server\share\AppData\CaveL.stalx",
            share,
            true,
        ),
        (
            "verbatim, mixed case",
            r"\\?\Unc\server\share\AppData\CaveL.stalx",
            share,
            true,
        ),
        (
            "device",
            r"\\.\UNC\server\share\AppData\CaveL.stalx",
            share,
            true,
        ),
        (
            "device, lower case",
            r"\\.\unc\server\share\AppData\CaveL.stalx",
            share,
            true,
        ),
        (
            "device, forward slashes",
            "//./UNC/server/share/AppData/CaveL.stalx",
            share,
            true,
        ),
        (
            "folder spelled verbatim in lower case",
            r"\\server\share\AppData\CaveL.stalx",
            r"\\?\unc\server\share\AppData",
            true,
        ),
        (
            "another share",
            r"\\?\unc\server\other\AppData\CaveL.stalx",
            share,
            false,
        ),
        (
            "rooted on the current drive",
            r"\server\share\AppData\CaveL.stalx",
            share,
            false,
        ),
        (
            "a relative folder named unc",
            r"unc\server\share\AppData\CaveL.stalx",
            share,
            false,
        ),
        (
            "a drive",
            r"C:\server\share\AppData\CaveL.stalx",
            share,
            false,
        ),
    ];
    let wrong = wrong_verdicts(&cases);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Win32 opens every path but a verbatim (`\\?\`) one normalized: a folder's
/// name loses one trailing dot, and the last name every trailing dot and space,
/// so `C:\Windows.\x` is written in `C:\Windows`. A trailing space before a
/// separator stays (`C:\Windows \` is another folder), so does all but one of
/// several dots, and a verbatim path keeps its names as they are.
#[test]
fn win32_drops_a_trailing_dot_where_a_verbatim_path_keeps_it() {
    let windows = r"C:\Windows";
    let app = r"C:\Users\a\AppData\Roaming\Stalagmight";
    let share = r"\\server\share\AppData";
    let cases = [
        (
            "dot after a system folder",
            r"C:\Windows.\CaveL.stalx",
            windows,
            true,
        ),
        (
            "dot after the user folder",
            r"C:\Users\a.\AppData\Roaming\Stalagmight\CaveL.stalx",
            app,
            true,
        ),
        (
            "dot after the app folder",
            r"C:\Users\a\AppData\Roaming\Stalagmight.\CaveL.stalx",
            app,
            true,
        ),
        (
            "dot after a folder, device prefix",
            r"\\.\C:\Windows.\CaveL.stalx",
            windows,
            true,
        ),
        (
            "dot after a folder, verbatim prefix in forward slashes",
            "//?/C:/Windows./CaveL.stalx",
            windows,
            true,
        ),
        (
            "dot after a folder on a share",
            r"\\server\share\AppData.\CaveL.stalx",
            share,
            true,
        ),
        (
            "dots and spaces at the end",
            r"C:\Windows. .",
            windows,
            true,
        ),
        (
            "space before a separator",
            r"C:\Windows \CaveL.stalx",
            windows,
            false,
        ),
        (
            "two dots before a separator",
            r"C:\Windows..\CaveL.stalx",
            windows,
            false,
        ),
        (
            "dot after a folder, verbatim",
            r"\\?\C:\Windows.\CaveL.stalx",
            windows,
            false,
        ),
        (
            "dot after a folder on a share, verbatim",
            r"\\?\UNC\server\share\AppData.\CaveL.stalx",
            share,
            false,
        ),
    ];
    let wrong = wrong_verdicts(&cases);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Drive paths in their device and verbatim spellings, either separator and any
/// letter case name the plain folder; a stream or a device name in a folder is
/// in that folder. An 8.3 short name (`PROGRA~1`) is not guessed from the text:
/// which folder it names is on the disk (and may be none), where the walk in
/// `resolved` expands it.
#[test]
fn drive_spellings_name_the_plain_folder() {
    let windows = r"C:\Windows";
    let program_files = r"C:\Program Files";
    let app = r"C:\Users\a\AppData\Roaming\Stalagmight";
    let cases = [
        ("plain", r"C:\Windows\CaveL.stalx", windows, true),
        ("verbatim", r"\\?\C:\Windows\CaveL.stalx", windows, true),
        ("device", r"\\.\C:\Windows\CaveL.stalx", windows, true),
        (
            "verbatim in forward slashes",
            "//?/C:/Windows/CaveL.stalx",
            windows,
            true,
        ),
        (
            "mixed separators",
            r"C:/Windows\System32/CaveL.stalx",
            windows,
            true,
        ),
        (
            "lower case",
            r"c:\program files\CaveL.stalx",
            program_files,
            true,
        ),
        (
            "upper case",
            r"C:\PROGRAM FILES\Stalagmight\CaveL.stalx",
            program_files,
            true,
        ),
        (
            "app folder in another case",
            r"c:\USERS\A\appdata\ROAMING\stalagmight\CaveL.stalx",
            app,
            true,
        ),
        (
            "app folder verbatim in lower case",
            r"\\?\c:\users\a\appdata\roaming\stalagmight\CaveL.stalx",
            app,
            true,
        ),
        (
            "a stream of a file in the app folder",
            r"C:\Users\a\AppData\Roaming\Stalagmight\config.toml:x.stalx",
            app,
            true,
        ),
        (
            "a device name in a system folder",
            r"C:\Windows\NUL.stalx",
            windows,
            true,
        ),
        ("another drive", r"D:\Windows\CaveL.stalx", windows, false),
        (
            "an 8.3 short name",
            r"C:\PROGRA~1\CaveL.stalx",
            program_files,
            false,
        ),
    ];
    let wrong = wrong_verdicts(&cases);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A stream name after the extension, a device name with none and the stream
/// type suffix fail the extension rule, on every host: the name does not end in
/// `.stalx`.
#[test]
fn a_stream_or_device_name_without_the_extension_is_refused() {
    let dirs = AppDirs {
        internal: Vec::new(),
        managed_projects: None,
    };
    let mut wrong = Vec::new();
    for name in [
        r"C:\Users\a\CaveL.stalx:stream",
        r"C:\Users\a\CaveL.stalx::$DATA",
        "CON",
        r"\\.\NUL",
    ] {
        let got = check_write_path_with(Path::new(name), WriteTarget::Project, &dirs);
        println!("W24 {name:?}: {got:?}");
        if !got
            .as_ref()
            .is_err_and(|why| why.contains("must end in .stalx"))
        {
            wrong.push(format!("{name}: {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
