//! [#576 W24-D1] Windows paths the write-path policy refuses from their text:
//! device, volume and drive-relative paths, alternate data streams, device
//! names and this computer reached as a network share. Judged under Windows
//! rules on an empty model disk, so every host runs them; Unix and macOS rules
//! take the same text as plain names.
//!
//! All paths are synthetic.

use std::path::{Path, PathBuf};

use crate::commands::write_path::{
    check_write_path_under, load_upgrade_refusal_under, windows_path_refusal, AppDirs, LinkLookup,
    PathRules, Walk, WriteTarget,
};

const WINDOWS: PathRules = PathRules {
    windows: true,
    fold_case: true,
};
const LINUX: PathRules = PathRules {
    windows: false,
    fold_case: false,
};
const MACOS: PathRules = PathRules {
    windows: false,
    fold_case: true,
};

/// A disk with nothing on it: no name exists and no link leads anywhere, so
/// nothing is touched.
struct NoDisk;

impl LinkLookup for NoDisk {
    fn canonical(&self, _: &Path) -> Option<PathBuf> {
        None
    }

    fn link_target(&self, _: &Path) -> Option<PathBuf> {
        None
    }
}

/// The cases a project write under `rules` does not answer as expected: each
/// `(what, path, refused)`, `Some(reason)` for a refusal whose message holds
/// `reason`, `None` for a write that is allowed. No app folders; every answer
/// printed.
fn wrong_writes(rules: PathRules, cases: &[(&str, &str, Option<&str>)]) -> Vec<String> {
    let dirs = AppDirs {
        internal: Vec::new(),
        managed_projects: None,
    };
    let walk = Walk {
        rules,
        disk: &NoDisk,
    };
    let mut wrong = Vec::new();
    for (what, path, refused) in cases {
        let got = check_write_path_under(Path::new(path), WriteTarget::Project, &dirs, walk);
        println!("W24-D1 {what}: {path:?}: {got:?}");
        let as_expected = match (&got, refused) {
            (Ok(()), None) => true,
            (Err(why), Some(reason)) => why.contains(reason),
            _ => false,
        };
        if !as_expected {
            wrong.push(format!("{what}: {got:?}"));
        }
    }
    wrong
}

const NOT_PLAIN: Option<&str> = Some("a Windows device, volume or drive-relative path");
const STREAM: Option<&str> = Some("an alternate data stream");
const DEVICE_NAME: Option<&str> = Some("a Windows device name");
const LOOPBACK: Option<&str> = Some("this computer reached as a network share");

/// Class 1: a `\\?\` or `\\.\` path that is not a drive or UNC path, the NT
/// prefix `\??\`, and a path relative to a drive's current folder (`C:x`).
#[test]
fn a_device_volume_or_drive_relative_path_is_refused() {
    let cases = [
        (
            "GLOBALROOT volume",
            r"\\?\GLOBALROOT\Device\HarddiskVolume3\Users\a\CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "GLOBALROOT network",
            r"\\?\GLOBALROOT\Device\Mup\server\share\CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "volume GUID",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "device volume",
            r"\\.\HarddiskVolume3\Users\a\CaveL.stalx",
            NOT_PLAIN,
        ),
        ("named pipe", r"\\.\pipe\CaveL.stalx", NOT_PLAIN),
        (
            "device prefix in forward slashes",
            "//?/GLOBALROOT/Device/Mup/server/share/CaveL.stalx",
            NOT_PLAIN,
        ),
        ("NT prefix, drive", r"\??\C:\Users\a\CaveL.stalx", NOT_PLAIN),
        (
            "NT prefix, UNC",
            r"\??\UNC\server\share\CaveL.stalx",
            NOT_PLAIN,
        ),
        ("drive-relative", "C:CaveL.stalx", NOT_PLAIN),
        ("drive-relative folder", r"c:Users\a\CaveL.stalx", NOT_PLAIN),
        ("drive-relative parent", r"C:..\CaveL.stalx", NOT_PLAIN),
        // [#576 RV18-W24-1] Win32 takes the device prefix as the root of a `\\.\`
        // path (and of a `\\?\` one in forward slashes, which it normalizes), so
        // a `..` after it climbs over the drive: this is the GLOBALROOT device.
        (
            "device drive, .. over the drive",
            r"\\.\C:\..\GLOBALROOT\Device\HarddiskVolume3\Windows\CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "device prefix in forward slashes, .. over the drive",
            "//?/C:/../GLOBALROOT/Device/HarddiskVolume3/Windows/CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "device drive, .. deeper in",
            r"\\.\C:\Users\..\..\pipe\CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "device drive, a . folder",
            r"\\.\C:\.\Users\a\CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "device drive in forward slashes, ..",
            "//./C:/Users/a/../CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "verbatim drive keeps .. as a name",
            r"\\?\C:\Users\a\..\b\CaveL.stalx",
            None,
        ),
        ("plain drive with ..", r"C:\Users\a\..\b\CaveL.stalx", None),
        ("plain drive", r"C:\Users\a\CaveL.stalx", None),
        ("verbatim drive", r"\\?\C:\Users\a\CaveL.stalx", None),
        ("device drive", r"\\.\C:\Users\a\CaveL.stalx", None),
        ("plain UNC", r"\\server\share\CaveL.stalx", None),
        ("verbatim UNC", r"\\?\UNC\server\share\CaveL.stalx", None),
        ("device UNC", r"\\.\unc\server\share\CaveL.stalx", None),
        ("rooted on the current drive", r"\Users\a\CaveL.stalx", None),
        ("relative", r"Documents\CaveL.stalx", None),
    ];
    let wrong = wrong_writes(WINDOWS, &cases);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Class 2: a `:` after the drive prefix names an alternate data stream - of
/// another file (`notes.txt:x.stalx`) or of a folder itself (`C:\Windows:x`).
#[test]
fn an_alternate_data_stream_is_refused() {
    let cases = [
        (
            "stream of another file",
            r"C:\Users\a\notes.txt:x.stalx",
            STREAM,
        ),
        ("stream of a folder", r"C:\Windows:CaveL.stalx", STREAM),
        (
            "stream named like the file",
            r"C:\Users\a\CaveL.stalx:x.stalx",
            STREAM,
        ),
        (
            "stream, verbatim",
            r"\\?\C:\Users\a\notes.txt:x.stalx",
            STREAM,
        ),
        (
            "stream on a share",
            r"\\server\share\notes.txt:x.stalx",
            STREAM,
        ),
        ("stream, relative", "notes.txt:x.stalx", STREAM),
        ("the drive's own colon", r"C:\Users\a\CaveL.stalx", None),
    ];
    let wrong = wrong_writes(WINDOWS, &cases);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Class 3: a last name that is a Windows device name, with or without an
/// extension, in any case, with trailing dots or spaces before the extension.
#[test]
fn a_device_name_is_refused() {
    let cases = [
        ("NUL", r"C:\Users\a\NUL.stalx", DEVICE_NAME),
        ("con, lower case", r"C:\Users\a\con.stalx", DEVICE_NAME),
        ("PRN", r"C:\Users\a\PRN.stalx", DEVICE_NAME),
        ("Aux, mixed case", r"C:\Users\a\Aux.stalx", DEVICE_NAME),
        ("COM1", r"C:\Users\a\COM1.stalx", DEVICE_NAME),
        ("com9", r"C:\Users\a\com9.stalx", DEVICE_NAME),
        ("LPT1", r"C:\Users\a\LPT1.stalx", DEVICE_NAME),
        ("lpt9", r"C:\Users\a\lpt9.stalx", DEVICE_NAME),
        (
            "two extensions",
            r"C:\Users\a\NUL.backup.stalx",
            DEVICE_NAME,
        ),
        (
            "space before the dot",
            r"C:\Users\a\nul .stalx",
            DEVICE_NAME,
        ),
        (
            "dots before the extension",
            r"C:\Users\a\CON..stalx",
            DEVICE_NAME,
        ),
        ("relative", "NUL.stalx", DEVICE_NAME),
        ("on a share", r"\\server\share\aux.stalx", DEVICE_NAME),
        // [#576 W30-3] The rest of the reserved names: port zero, the ISO 8859-1
        // superscript digits Windows reads as port digits, and the console's
        // input and output.
        ("COM0", r"C:\Users\a\COM0.stalx", DEVICE_NAME),
        ("LPT0", r"C:\Users\a\LPT0.stalx", DEVICE_NAME),
        (
            "COM superscript 1",
            "C:\\Users\\a\\COM\u{b9}.stalx",
            DEVICE_NAME,
        ),
        (
            "COM superscript 2",
            "C:\\Users\\a\\COM\u{b2}.stalx",
            DEVICE_NAME,
        ),
        (
            "COM superscript 3",
            "C:\\Users\\a\\COM\u{b3}.stalx",
            DEVICE_NAME,
        ),
        (
            "LPT superscript 1",
            "C:\\Users\\a\\LPT\u{b9}.stalx",
            DEVICE_NAME,
        ),
        (
            "LPT superscript 2",
            "C:\\Users\\a\\LPT\u{b2}.stalx",
            DEVICE_NAME,
        ),
        (
            "LPT superscript 3",
            "C:\\Users\\a\\LPT\u{b3}.stalx",
            DEVICE_NAME,
        ),
        ("CONIN$", r"C:\Users\a\CONIN$.stalx", DEVICE_NAME),
        ("CONOUT$", r"C:\Users\a\CONOUT$.stalx", DEVICE_NAME),
        ("COM10 is a file", r"C:\Users\a\COM10.stalx", None),
        ("NULL is a file", r"C:\Users\a\NULL.stalx", None),
        ("CONSOLE is a file", r"C:\Users\a\CONSOLE.stalx", None),
        (
            "COM superscript 4 is a file",
            "C:\\Users\\a\\COM\u{2074}.stalx",
            None,
        ),
        ("CONIN is a file", r"C:\Users\a\CONIN.stalx", None),
        ("a folder named NUL", r"C:\Users\a\NUL\CaveL.stalx", None),
    ];
    let wrong = wrong_writes(WINDOWS, &cases);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Class 3, every spelling of the names added in #576 W30-3 (the text judged
/// alone, so a name without an extension counts too): in any case, with or
/// without an extension, with trailing dots or spaces.
#[test]
fn every_spelling_of_the_other_device_names_is_refused() {
    let names = [
        "COM0",
        "LPT0",
        "COM\u{b9}",
        "COM\u{b2}",
        "COM\u{b3}",
        "LPT\u{b9}",
        "LPT\u{b2}",
        "LPT\u{b3}",
        "CONIN$",
        "CONOUT$",
    ];
    let mut wrong = Vec::new();
    for name in names {
        for spelling in [
            name.to_owned(),
            name.to_lowercase(),
            format!("{name}.stalx"),
            format!("{}.STALX", name.to_lowercase()),
            format!("{name}."),
            format!("{name} "),
            format!("{name}. ."),
            format!("{name} .stalx"),
            format!("{name}..stalx"),
            format!("{name} . .stalx"),
        ] {
            let path = format!(r"C:\Users\a\{spelling}");
            let got = windows_path_refusal(&path, None);
            println!("W30-3 device name: {path:?}: {got:?}");
            if !got.is_some_and(|why| why.contains("a Windows device name")) {
                wrong.push(format!("{path:?}: {got:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Class 4: a UNC path whose server is this computer - `localhost`, an address
/// in 127.0.0.0/8 or `::1` (also as an `ipv6-literal.net` name), in every UNC
/// spelling.
#[test]
fn this_computer_reached_as_a_network_share_is_refused() {
    let cases = [
        ("localhost", r"\\localhost\C$\Users\a\CaveL.stalx", LOOPBACK),
        ("LOCALHOST", r"\\LOCALHOST\share\CaveL.stalx", LOOPBACK),
        ("localhost.", r"\\localhost.\share\CaveL.stalx", LOOPBACK),
        ("127.0.0.1", r"\\127.0.0.1\c$\CaveL.stalx", LOOPBACK),
        ("127.1.2.3", r"\\127.1.2.3\share\CaveL.stalx", LOOPBACK),
        ("127.1", r"\\127.1\share\CaveL.stalx", LOOPBACK),
        // [#576 W30-3] Every spelling Windows reads as an IPv4 address: one
        // decimal or hex number, fewer than four parts, octal parts.
        ("2130706433", r"\\2130706433\c$\CaveL.stalx", LOOPBACK),
        ("0x7f000001", r"\\0x7f000001\c$\CaveL.stalx", LOOPBACK),
        ("0X7F000001", r"\\0X7F000001\c$\CaveL.stalx", LOOPBACK),
        ("0x7f.1", r"\\0x7f.1\share\CaveL.stalx", LOOPBACK),
        ("0x7f.0.0.1", r"\\0x7f.0.0.1\share\CaveL.stalx", LOOPBACK),
        ("127.0.1", r"\\127.0.1\share\CaveL.stalx", LOOPBACK),
        ("0177.0.0.1", r"\\0177.0.0.1\share\CaveL.stalx", LOOPBACK),
        ("0x7f.0x1", r"\\0x7f.0x1\share\CaveL.stalx", LOOPBACK),
        ("::1", r"\\::1\c$\CaveL.stalx", LOOPBACK),
        // [#576 RV18-W24-5] In brackets, as a URL writes it: this computer, not
        // an alternate data stream (its `:` comes before the share).
        ("[::1]", r"\\[::1]\c$\CaveL.stalx", LOOPBACK),
        (
            "[::ffff:127.0.0.1]",
            r"\\[::ffff:127.0.0.1]\c$\CaveL.stalx",
            LOOPBACK,
        ),
        (
            "::1 as an ipv6-literal.net name",
            r"\\--1.ipv6-literal.net\c$\CaveL.stalx",
            LOOPBACK,
        ),
        (
            "verbatim UNC",
            r"\\?\UNC\localhost\c$\CaveL.stalx",
            LOOPBACK,
        ),
        ("device UNC", r"\\.\unc\127.0.0.1\c$\CaveL.stalx", LOOPBACK),
        ("forward slashes", "//localhost/c$/CaveL.stalx", LOOPBACK),
        // [#576 RV18-W24-1] A `..` after a device prefix climbs over the UNC
        // server or the drive: refused as a device path before the server is read.
        (
            "device UNC, .. over the server",
            r"\\.\UNC\server\..\localhost\c$\Windows\CaveL.stalx",
            NOT_PLAIN,
        ),
        (
            "device drive, .. onto UNC",
            r"\\.\C:\..\UNC\localhost\c$\Windows\CaveL.stalx",
            NOT_PLAIN,
        ),
        ("another server", r"\\server\share\CaveL.stalx", None),
        ("128.0.0.1", r"\\128.0.0.1\share\CaveL.stalx", None),
        (
            "2147483649 (128.0.0.1)",
            r"\\2147483649\share\CaveL.stalx",
            None,
        ),
        (
            "0x80000001 (128.0.0.1)",
            r"\\0x80000001\share\CaveL.stalx",
            None,
        ),
        (
            "0x7f.1 past 24 bits",
            r"\\0x7f.0x1000000\share\CaveL.stalx",
            None,
        ),
        ("a hex-looking name", r"\\0x7g.1\share\CaveL.stalx", None),
        ("08.0.0.1, not octal", r"\\08.0.0.1\share\CaveL.stalx", None),
        ("localhostx", r"\\localhostx\share\CaveL.stalx", None),
        (
            "another ipv6-literal.net name",
            r"\\fe80--2.ipv6-literal.net\share\CaveL.stalx",
            None,
        ),
    ];
    let wrong = wrong_writes(WINDOWS, &cases);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Unix and macOS rules take every one of those spellings as plain names: a
/// `:`, `C:x`, `NUL.stalx` or `//localhost` is a file or folder name there.
#[test]
fn on_unix_and_macos_the_windows_spellings_are_plain_names() {
    let cases = [
        (
            "device prefix",
            r"\\?\GLOBALROOT\Device\Mup\CaveL.stalx",
            None,
        ),
        ("drive-relative", "C:CaveL.stalx", None),
        ("stream", "/home/a/notes.txt:x.stalx", None),
        ("device name", "/home/a/NUL.stalx", None),
        ("loopback", "//localhost/share/CaveL.stalx", None),
    ];
    let mut wrong = wrong_writes(LINUX, &cases);
    wrong.extend(wrong_writes(MACOS, &cases));
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Class 4, the rest: this computer's own name (read from `COMPUTERNAME` on
/// Windows, given here) is a loopback server too, alone or as the start of a
/// domain name; another computer whose name starts the same is not.
#[test]
fn this_computer_s_own_name_is_a_loopback_server() {
    let own = Some("CaveHost");
    let cases = [
        (r"\\CaveHost\share\CaveL.stalx", own, LOOPBACK),
        (r"\\cavehost\share\CaveL.stalx", own, LOOPBACK),
        (r"\\cavehost.example.org\share\CaveL.stalx", own, LOOPBACK),
        (r"\\?\UNC\CAVEHOST\c$\CaveL.stalx", own, LOOPBACK),
        (r"\\cavehost2\share\CaveL.stalx", own, None),
        (r"\\server\share\CaveL.stalx", own, None),
        (r"\\CaveHost\share\CaveL.stalx", None, None),
    ];
    let mut wrong = Vec::new();
    for (path, own_name, refused) in cases {
        let got = windows_path_refusal(path, own_name);
        println!("W24-D1 own name {own_name:?}: {path:?}: {got:?}");
        let as_expected = match (got, refused) {
            (None, None) => true,
            (Some(why), Some(reason)) => why.contains(reason),
            _ => false,
        };
        if !as_expected {
            wrong.push(format!("{path} ({own_name:?}): {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// [#576 RV18-W24-3] Opening an older project upgrades it in place, so a load
/// refuses the same Windows paths a write does - judged here under Windows rules
/// on every host (CI runs on Linux, where the host rules never take this path).
/// Under Linux rules the same text is a plain name and the load may upgrade.
#[test]
fn an_older_project_at_a_windows_device_path_is_not_upgraded() {
    let dirs = AppDirs {
        internal: Vec::new(),
        managed_projects: None,
    };
    let on = |rules| Walk {
        rules,
        disk: &NoDisk,
    };
    let cases = [
        (r"\\.\pipe\CaveL.stalx", NOT_PLAIN),
        ("C:CaveL.stalx", NOT_PLAIN),
        (r"C:\Users\a\notes.txt:x.stalx", STREAM),
        (r"C:\Users\a\NUL.stalx", DEVICE_NAME),
        (r"\\localhost\c$\Users\a\CaveL.stalx", LOOPBACK),
        (r"C:\Users\a\CaveL.stalx", None),
    ];
    let mut wrong = Vec::new();
    for (path, refused) in cases {
        let windows = load_upgrade_refusal_under(Path::new(path), &dirs, on(WINDOWS));
        let linux = load_upgrade_refusal_under(Path::new(path), &dirs, on(LINUX));
        println!(
            "RV18-W24-3 load {path:?}: windows {:?}, linux {:?}",
            windows.as_ref().map(|r| &r.older_format),
            linux.as_ref().map(|r| &r.older_format)
        );
        let as_expected = match (&windows, refused) {
            (None, None) => true,
            (Some(refusal), Some(reason)) => {
                refusal.older_format.contains(reason) && refusal.unstamped.contains(reason)
            }
            _ => false,
        };
        if !as_expected || linux.is_some() {
            wrong.push(format!("{path}: windows {windows:?}, linux {linux:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
