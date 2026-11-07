//! [#576 C11-D1] The one check every command that writes, replaces or deletes a
//! file at a path the webview hands it runs first.
//!
//! The real boundary is that only the app's bundled frontend runs in the webview
//! (a strict CSP, `script-src 'self'`; see docs/architecture.md, "IPC trust
//! boundary"). The custom commands take arbitrary paths from it - Tauri's fs
//! capability scope does not apply to them - so this check is defence in depth for
//! the day that boundary fails: a script that got into the page could otherwise
//! export over `config.toml`, the stored API key or a system file, or delete or
//! "repair" any file it names. Three rules, deliberately narrow so no place a user
//! saves to is refused:
//!
//! - the file has the extension of what is being written (`.stalx` for a project,
//!   the export format's own for an export) - and so does the file a link at that
//!   name leads to, even one that does not exist yet;
//! - it is not inside the app's own directories (configuration, data, cache) -
//!   except the SpeleoDB managed-project cache, where projects legitimately live;
//! - it is not inside a system directory (`/etc`, `/usr`, `C:\Windows`,
//!   `C:\Program Files`, ...). `/home`, `/var`, `/run/media`, `/mnt`, `/media`,
//!   `/opt` and `/tmp` are not system directories here: users save there.
//!
//! On Windows a fourth, from the text alone (#576 W24-D1): the path names a
//! plain file in a folder - not a device, volume or drive-relative path
//! (`\\?\GLOBALROOT\..`, `\??\..`, `C:x`), not an alternate data stream (a `:`
//! after the drive), not a device name (`NUL.stalx`, `COM1`), and not this
//! computer reached as a network share (`\\localhost\C$\..`).

use std::path::{Component, Path, PathBuf};

use crate::export::{ImageFormat, ProfileFormat};
use crate::persistence::UpgradeRefusal;

/// What a command is about to write, which decides the extension it must have.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WriteTarget {
    /// A project database (`.stalx`).
    Project,
    Csv,
    Json,
    /// Ariane `.tml` / `.tmlu`.
    Ariane,
    Image(ImageFormat),
    DiveProfile(ProfileFormat),
}

impl WriteTarget {
    /// Every target, for [`Self::of_path`].
    const ALL: [Self; 9] = [
        Self::Project,
        Self::Csv,
        Self::Json,
        Self::Ariane,
        Self::Image(ImageFormat::Png),
        Self::Image(ImageFormat::Svg),
        Self::Image(ImageFormat::Pdf),
        Self::DiveProfile(ProfileFormat::SubsurfaceXml),
        Self::DiveProfile(ProfileFormat::PlainText),
    ];

    /// [#576 HFI-2] What a file named `path` would be written as - the target
    /// whose extension it has - or `None` when the app writes no file of that
    /// type. For a check that serves every save and export alike (the
    /// existence probe, `commands::save_target`).
    pub(crate) fn of_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Self::ALL.into_iter().find(|target| {
            target
                .extensions()
                .iter()
                .any(|x| extension.eq_ignore_ascii_case(x))
        })
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Project => &["stalx"],
            Self::Csv => &["csv"],
            Self::Json => &["json"],
            Self::Ariane => &["tml", "tmlu"],
            Self::Image(ImageFormat::Png) => &["png"],
            Self::Image(ImageFormat::Svg) => &["svg"],
            Self::Image(ImageFormat::Pdf) => &["pdf"],
            Self::DiveProfile(ProfileFormat::SubsurfaceXml) => &["xml"],
            Self::DiveProfile(ProfileFormat::PlainText) => &["txt"],
        }
    }
}

/// The app's own directories, and the one place inside them projects may live.
pub(crate) struct AppDirs {
    /// Configuration, data (logs, telemetry) and cache directories.
    pub(crate) internal: Vec<PathBuf>,
    /// The SpeleoDB managed-project cache (inside the cache directory).
    pub(crate) managed_projects: Option<PathBuf>,
}

impl AppDirs {
    /// The directories this process resolved at startup. A directory that did not
    /// resolve (the `"."` fallback, or any relative path) is left out: guarding the
    /// working directory would refuse wherever the app happened to be started.
    pub(crate) fn current() -> Self {
        let usable = |dir: Option<&Path>| dir.filter(|d| d.is_absolute()).map(Path::to_path_buf);
        let cache = usable(crate::app_dirs::cache_dir());
        Self {
            internal: [
                usable(crate::app_dirs::config_dir()),
                usable(crate::app_dirs::data_local_dir()),
                cache.clone(),
            ]
            .into_iter()
            .flatten()
            .collect(),
            managed_projects: cache.map(|c| c.join("speleodb")),
        }
    }
}

/// Check `path` before a command writes `target` to it, replaces it or deletes it.
/// `Err` is the user-facing reason, naming the file.
pub(crate) fn check_write_path(path: &Path, target: WriteTarget) -> Result<(), String> {
    check_write_path_with(path, target, &AppDirs::current())
}

/// [`check_write_path`] against given app directories (the testable core).
pub(crate) fn check_write_path_with(
    path: &Path,
    target: WriteTarget,
    dirs: &AppDirs,
) -> Result<(), String> {
    check_write_path_under(path, target, dirs, Walk::HOST)
}

/// [`check_write_path_with`] under given path rules, on a given file system
/// (#576 RV13-2): another platform's rules, tested on any host against a model of
/// the disk.
pub(crate) fn check_write_path_under(
    path: &Path,
    target: WriteTarget,
    dirs: &AppDirs,
    walk: Walk<'_>,
) -> Result<(), String> {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let refuse = |why: String| {
        tracing::warn!(
            path = ?path,
            target = ?target,
            reason = %why,
            purpose = "refuse a write outside where the app writes (#576 C11-D1)",
            "Refused a file write"
        );
        Err(format!("Refused to change {name}: {why}"))
    };

    // [#576 NF-W13BE-1] The name, and the file a write through it lands on: a link
    // at the name (even one that leads nowhere yet) is followed by the writes that
    // open the path (SQLite creating a project does), so its target has to be a
    // file of this type too.
    let extensions = target.extensions();
    let expected = || {
        let expected: Vec<String> = extensions.iter().map(|e| format!(".{e}")).collect();
        expected.join(" or ")
    };
    let has_expected = |file: &Path| {
        file.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
    };
    if !has_expected(path) {
        return refuse(format!("the file must end in {}", expected()));
    }
    // [#576 W24-D1] A Windows path that names no plain file in a folder is
    // refused from its text, before the walk below asks the disk about it.
    if let Some(why) = windows_refusal(path, walk.rules) {
        return refuse(why);
    }
    // [#576 RV13-5] The name is right and the link at it leads to a file of
    // another type (or a folder): said of the target, not of the name.
    // A chain of links past what Windows follows has no landing to judge: the
    // location check below refuses it.
    if let Some(lands) = resolved(&absolute(path), walk) {
        if !has_expected(&lands) {
            let to = lands.file_name().map_or_else(
                || lands.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            return refuse(format!(
                "it is a link to {to}, which is not a {} file",
                expected()
            ));
        }
    }
    location_refusal(path, matches!(target, WriteTarget::Project), dirs, walk)
        .map_or(Ok(()), refuse)
}

/// [#576 RV4-R-M3-OPEN] What a load of `path` may not write, as the message to
/// refuse it with - or `None` where a user keeps projects.
///
/// Opening a project only reads it, with one exception: an older-format file is
/// upgraded IN PLACE, and a `.pre-v<N>.backup` copy of it is written beside it
/// first. So a load anywhere opens the file (viewing and exporting a project kept
/// in the app's own folders or a system folder work as anywhere else), and the
/// location rules of [`check_write_path`] - not inside the app's own folders (the
/// SpeleoDB managed-project cache excepted), not inside a system folder - apply
/// to that upgrade alone: it is refused there with this message, before anything
/// is written, and the file is left as it was. The extension rule does not apply:
/// the open dialog offers "All Files", and a load creates no file of that name.
///
/// [#576 RV6b-R3] Two messages, one per write the open would make: the load
/// refuses with the one that fits the file. An unstamped file whose project row
/// claims this version is not an older file and gets no backup copy, so it is not
/// told either.
pub(crate) fn load_upgrade_refusal(path: &Path) -> Option<UpgradeRefusal> {
    load_upgrade_refusal_with(path, &AppDirs::current())
}

/// [`load_upgrade_refusal`] against given app directories (the testable core).
pub(crate) fn load_upgrade_refusal_with(path: &Path, dirs: &AppDirs) -> Option<UpgradeRefusal> {
    load_upgrade_refusal_under(path, dirs, Walk::HOST)
}

/// [`load_upgrade_refusal_with`] under given path rules, on a given file system
/// (#576 RV18-W24-3), as [`check_write_path_under`]: the Windows refusals are
/// tested on any host.
pub(crate) fn load_upgrade_refusal_under(
    path: &Path,
    dirs: &AppDirs,
    walk: Walk<'_>,
) -> Option<UpgradeRefusal> {
    let why =
        windows_refusal(path, walk.rules).or_else(|| location_refusal(path, true, dirs, walk))?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let what_to_do = "where Stalagmight does not write. The file was left as it was. Copy the \
                      project to a folder of your own (for example Documents) and open the copy.";
    Some(UpgradeRefusal {
        older_format: format!(
            "{name} was saved by an older version of Stalagmight, and opening it would upgrade \
             it to the current file format in place and keep a backup copy beside it - but \
             {why}, {what_to_do}"
        ),
        unstamped: format!(
            "{name} has no file-format version stamp, so opening it would bring its file \
             format up to date in place - but {why}, {what_to_do}"
        ),
    })
}

/// Why `path` is in a place no command may write, or `None`. `project` lets a
/// project live in the SpeleoDB managed-project cache.
fn location_refusal(path: &Path, project: bool, dirs: &AppDirs, walk: Walk<'_>) -> Option<String> {
    // [#576 RV13-2] Past the links Windows follows on one path, where the write
    // would land cannot be told: refused, not judged by the last link's name.
    let Some(forms) = path_forms(path, walk) else {
        return Some("that is reached through more links than Windows follows".to_owned());
    };
    let inside = |dir: &Path| {
        let dir_forms = path_forms(dir, walk).unwrap_or_else(|| vec![lexical(&absolute(dir))]);
        forms
            .iter()
            .any(|p| dir_forms.iter().any(|d| starts_with_dir(p, d, walk.rules)))
    };
    let managed = project && dirs.managed_projects.as_deref().is_some_and(inside);
    if !managed && dirs.internal.iter().any(|d| inside(d)) {
        return Some("that is inside the app's own settings, data or cache folder".to_owned());
    }
    if system_dirs().iter().any(|d| inside(d)) {
        return Some("that is inside a system folder".to_owned());
    }
    None
}

/// [#576 W24-D1] Why, under Windows rules, `path` names no plain file in a
/// folder - or `None`. Judged on the text as given, before anything on the disk
/// is asked; under Unix and macOS rules every such text is a plain name.
fn windows_refusal(path: &Path, rules: PathRules) -> Option<String> {
    if !rules.windows {
        return None;
    }
    // This computer's own name is a loopback server too: known on Windows.
    let own_name = if cfg!(windows) {
        std::env::var("COMPUTERNAME").ok()
    } else {
        None
    };
    windows_path_refusal(&path.to_string_lossy(), own_name.as_deref()).map(str::to_owned)
}

/// [`windows_refusal`] on a path's text - pure, so every host tests it. A
/// `\\?\` or `\\.\` path must name a drive (`X:\`) or a UNC path: any other
/// (`\\?\GLOBALROOT\..`, `\\?\Volume{..}\..`, `\\.\HarddiskVolume3\..`,
/// `\\.\pipe\..`) is a device or volume, and so is a path under the NT prefix
/// `\??\`, and so is a `\\.\` path (or a `\\?\` one in forward slashes) with a
/// `.` or `..` after the prefix, which Win32 lets climb over the drive or
/// server. A drive letter not followed by `\` (`C:x`) names a place in that
/// drive's current folder, which the text does not tell. A UNC path whose
/// server is this computer ([`is_this_computer`]; `own_name` its name, when
/// known) reaches a local folder under another name.
pub(crate) fn windows_path_refusal(path: &str, own_name: Option<&str>) -> Option<&'static str> {
    let text = path.replace('/', "\\");
    let plain = windows_form(path);
    let drive = plain
        .as_bytes()
        .get(..2)
        .is_some_and(|d| d[0].is_ascii_alphabetic() && d[1] == b':');
    let after_drive = if drive { &plain[2..] } else { plain.as_str() };
    let drive_root = drive && after_drive.starts_with('\\');
    let prefixed = text.starts_with(r"\\?\") || text.starts_with(r"\\.\");
    // [#576 RV18-W24-1] Win32 takes the device prefix itself as the root of a
    // `\\.\` path, and of a `\\?\` one not spelled exactly so (forward slashes),
    // which it normalizes: a `.` or `..` after the prefix climbs over the drive
    // or the UNC server (`\\.\C:\..\GLOBALROOT\..` is the GLOBALROOT device).
    // Only an exact `\\?\` path is passed on as written.
    let climbs = prefixed
        && !path.starts_with(r"\\?\")
        && text
            .get(4..)
            .unwrap_or_default()
            .split('\\')
            .any(|name| matches!(name.trim_end_matches(' '), "." | ".."));
    if text.starts_with(r"\??\")
        || climbs
        || (prefixed && !drive_root && !plain.starts_with(r"\\"))
        || (drive && !drive_root)
    {
        return Some("that is a Windows device, volume or drive-relative path");
    }
    let server = plain
        .strip_prefix(r"\\")
        .and_then(|unc| unc.split('\\').next());
    if server.is_some_and(|server| is_this_computer(server, own_name)) {
        return Some("that is this computer reached as a network share");
    }
    // A `:` after the drive names an alternate data stream: of another file
    // (`notes.txt:x.stalx`) or of a folder itself (`C:\Windows:x.stalx`).
    if after_drive.contains(':') {
        return Some("that is an alternate data stream of a file or folder");
    }
    // A last name whose stem - up to its first dot, trailing spaces dropped -
    // is a device name is refused, extension or not (`NUL.stalx`): Microsoft
    // reserves both, and Windows before 11 opens the device for either. Windows
    // 11 opens a name with an extension as a file, but which Windows the app
    // runs on does not change what the name is, so the rule is the same.
    let last = after_drive.rsplit('\\').find(|name| !name.is_empty());
    let stem = last
        .and_then(|name| name.split('.').next())
        .unwrap_or_default()
        .trim_end_matches(' ');
    if DEVICE_NAMES.iter().any(|d| stem.eq_ignore_ascii_case(d)) {
        return Some("that is a Windows device name");
    }
    None
}

/// Whether a UNC server name is this computer (#576 W24-D1): `localhost`, an
/// IPv4 address in 127.0.0.0/8 in any spelling Windows reads as one
/// ([`ipv4_any_form`]: `127.1`, `2130706433`, `0x7f000001`, `0x7f.1`), `::1`
/// (also as an `ipv6-literal.net` name, `--1.ipv6-literal.net`), or `own_name` -
/// alone or as the start of a domain name. A trailing dot (the fully qualified
/// form) and letter case do not matter.
fn is_this_computer(server: &str, own_name: Option<&str>) -> bool {
    let server = server.trim_end_matches('.').to_ascii_lowercase();
    if server == "localhost"
        || (server.starts_with("127.") && server.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
        || ipv4_any_form(&server).is_some_and(|address| address.is_loopback())
    {
        return true;
    }
    // An ipv6-literal.net name spells `:` as `-` and a zone as `s<zone>`;
    // [#576 RV18-W24-5] an address may come in brackets, as in a URL (`[::1]`).
    let v6 = server.strip_suffix(".ipv6-literal.net").map_or_else(
        || {
            server
                .strip_prefix('[')
                .and_then(|inner| inner.strip_suffix(']'))
                .unwrap_or(&server)
                .to_owned()
        },
        |name| name.split('s').next().unwrap_or_default().replace('-', ":"),
    );
    if v6.parse::<std::net::Ipv6Addr>().is_ok_and(|address| {
        address.is_loopback() || address.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
    }) {
        return true;
    }
    own_name.is_some_and(|own| {
        let own = own.to_ascii_lowercase();
        !own.is_empty()
            && server
                .strip_prefix(own.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

/// [#576 W30-3] The IPv4 address `server` spells in any form Windows reads as
/// one (the `inet_addr` rules): one to four parts split by dots, each decimal,
/// hex (`0x7f`) or octal (a leading `0`, `0177`), the last part filling the bytes
/// the others leave - `127.1`, `0x7f.1`, the single numbers `2130706433` and
/// `0x7f000001`. `None` for anything else, which is a name.
fn ipv4_any_form(server: &str) -> Option<std::net::Ipv4Addr> {
    let parts: Vec<&str> = server.split('.').collect();
    if parts.len() > 4 {
        return None;
    }
    let mut values = Vec::with_capacity(parts.len());
    for part in parts {
        let (digits, radix) = match part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            Some(hex) => (hex, 16),
            None if part.len() > 1 && part.starts_with('0') => (&part[1..], 8),
            None => (part, 10),
        };
        // `from_str_radix` takes a leading `+`; `inet_addr` does not.
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        values.push(u32::from_str_radix(digits, radix).ok()?);
    }
    let (&last, leading) = values.split_last()?;
    let mut address = 0_u32;
    for (i, &value) in leading.iter().enumerate() {
        if value > 0xff {
            return None;
        }
        address |= value << (24 - 8 * i);
    }
    // The last part fills the 32, 24, 16 or 8 bits the leading bytes leave.
    let free_bits = 32 - 8 * leading.len();
    if free_bits < 32 && last >> free_bits != 0 {
        return None;
    }
    Some(std::net::Ipv4Addr::from(address | last))
}

/// The Windows device names a file name may not have (#576 W24-D1), compared
/// ignoring ASCII case. [#576 W30-3] Port zero, the ports spelled with the
/// ISO 8859-1 superscript digits Windows reads as digits (`COM¹`,
/// `LPT²`, ...) and the console's input and output are reserved too.
const DEVICE_NAMES: [&str; 32] = [
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³", "CONIN$", "CONOUT$",
];

/// The forms of `path` to compare: as given (made absolute) with `.`/`..` folded,
/// and as a write would reach it on disk ([`resolved`]: links followed), so `/etc`
/// reached as `/private/etc` or through a link is still recognised. The file
/// itself need not exist. `None` past the links Windows follows ([`resolved`]).
fn path_forms(path: &Path, walk: Walk<'_>) -> Option<Vec<PathBuf>> {
    let absolute = absolute(path);
    Some(vec![lexical(&absolute), resolved(&absolute, walk)?])
}

/// `path` made absolute against the working directory, without touching it
/// otherwise.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    }
}

/// How many links that lead nowhere [`resolved`] follows before it stops, under
/// POSIX rules: the Linux kernel's own limit for one lookup (`MAXSYMLINKS`). A
/// longer chain - or a loop - is one no write can follow either (`ELOOP`, nothing
/// written), so the walk judges the last link's own name.
const MAX_DANGLING_LINKS: usize = 40;

/// [#576 RV13-2] The same under Windows rules: the 63 reparse points Windows
/// follows on one path. Past them the walk refuses rather than judge the last
/// link's name - where the write would land cannot be told.
const MAX_DANGLING_LINKS_WINDOWS: usize = 63;

/// What [`resolved`] asks the file system (#576 RV13-2): the disk, or a test's
/// model of one.
pub(crate) trait LinkLookup {
    /// `path` with every link on it followed, when it exists (`canonicalize`).
    fn canonical(&self, path: &Path) -> Option<PathBuf>;
    /// What the link at `path` points to, when there is a link there.
    fn link_target(&self, path: &Path) -> Option<PathBuf>;
}

/// The disk.
struct Disk;

impl LinkLookup for Disk {
    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    fn link_target(&self, path: &Path) -> Option<PathBuf> {
        std::fs::read_link(path).ok()
    }
}

/// How [`resolved`] walks a path: under which platform's rules, asking which
/// file system.
#[derive(Clone, Copy)]
pub(crate) struct Walk<'a> {
    pub(crate) rules: PathRules,
    pub(crate) disk: &'a dyn LinkLookup,
}

impl Walk<'static> {
    /// This platform's rules, on the disk.
    const HOST: Self = Self {
        rules: PathRules::HOST,
        disk: &Disk,
    };
}

/// `path` as a write reaches it, component by component: a name that exists is
/// resolved (`canonicalize`: links followed), a name that does not is kept as
/// the folder or file the write would create, and `..` steps back from what was
/// reached so far.
///
/// [#576 RV12-1] Component by component, not "the deepest existing ancestor
/// resolved, the rest appended as given": a write that first makes the missing
/// folders (creating a project does) reaches `<dir>/nope/../app-link/x` as
/// `nope`, back out of it, then THROUGH `app-link`. Appended as given, the `..`
/// matched no folder and the link was never followed, so a link into the app's
/// own folder or a system folder passed the policy.
///
/// [#576 NF-W13BE-1] A link whose target does not exist yet cannot be
/// canonicalized, and was kept as its own name - but a write that opens the
/// path follows it and creates the target (SQLite creating a project at
/// `<user>/x.stalx` -> `<app>/planted.stalx` makes `<app>/planted.stalx`). Such a
/// link is followed here too: its target is read and resolved in its place,
/// relative to the link's folder when it is relative, so the policy judges the
/// place the write lands.
///
/// [#576 RV13-2] That walk is POSIX's: `..` steps back from the folder a link
/// led to. Windows folds `..` by name BEFORE it follows any link (Win32 path
/// normalization, before the path reaches the file system) - so under Windows
/// rules the path is folded first, and so is a link's target, from the link's
/// folder: `<u>\j1\..\applink\x` is `<u>\applink\x` whatever `j1` links to. And
/// Windows follows up to 63 links where Linux follows 40: `None` when a chain of
/// links leading nowhere runs past that under Windows rules (refused by the
/// policy); under POSIX rules the walk judges the last link's own name, as no
/// write gets through such a chain.
pub(crate) fn resolved(path: &Path, walk: Walk<'_>) -> Option<PathBuf> {
    let mut reached = PathBuf::new();
    let mut links_followed = 0;
    let start = if walk.rules.windows {
        lexical(path)
    } else {
        path.to_path_buf()
    };
    let complete = resolve_onto(&mut reached, &start, &mut links_followed, walk);
    (complete || !walk.rules.windows).then_some(reached)
}

/// [`resolved`]'s walk: `path`'s components resolved onto `reached`. `false` when
/// a link was left unfollowed at the cap.
fn resolve_onto(
    reached: &mut PathBuf,
    path: &Path,
    links_followed: &mut usize,
    walk: Walk<'_>,
) -> bool {
    let cap = if walk.rules.windows {
        MAX_DANGLING_LINKS_WINDOWS
    } else {
        MAX_DANGLING_LINKS
    };
    let mut complete = true;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                reached.pop();
            }
            Component::Normal(name) => {
                reached.push(name);
                if let Some(canonical) = walk.disk.canonical(reached) {
                    *reached = canonical;
                } else if let Some(target) = walk.disk.link_target(reached) {
                    // A link that leads nowhere (yet): continue from its target,
                    // which replaces `reached` when absolute and is taken from
                    // the link's folder when relative.
                    if *links_followed < cap {
                        *links_followed += 1;
                        reached.pop();
                        complete &= if walk.rules.windows {
                            let next = lexical(&reached.join(&target));
                            *reached = PathBuf::new();
                            resolve_onto(reached, &next, links_followed, walk)
                        } else {
                            resolve_onto(reached, &target, links_followed, walk)
                        };
                    } else {
                        complete = false;
                    }
                }
            }
            // The root and a Windows drive prefix are kept as they are:
            // canonicalizing a bare `C:` names that drive's working directory.
            other => reached.push(other),
        }
    }
    complete
}

/// `path` with `.` and `..` folded away, without touching the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// How a platform's file system compares paths (#576 RV6b-R4).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PathRules {
    /// Windows path syntax: `\` and `/` both separate, and a verbatim or device
    /// prefix (`\\?\C:\..`, `\\.\C:\..`, `\\?\UNC\server\share\..`,
    /// `\\.\UNC\server\share\..`, `UNC` in any case) names the same place as the
    /// plain form (`C:\..`, `\\server\share\..`); outside a `\\?\` path a name
    /// loses the trailing dots and spaces Win32 drops (`C:\Windows.\..` is
    /// `C:\Windows\..`).
    pub(crate) windows: bool,
    /// Names match whatever their letter case: Windows (NTFS) and macOS, whose
    /// default file systems (APFS, HFS+) are case-insensitive. On a macOS volume
    /// formatted case-sensitive this refuses a differently-cased folder too - the
    /// conservative side for a check that only ever refuses.
    pub(crate) fold_case: bool,
}

impl PathRules {
    /// The rules of the platform this build runs on.
    pub(crate) const HOST: Self = Self {
        windows: cfg!(windows),
        fold_case: cfg!(any(windows, target_os = "macos")),
    };
}

/// Whether `path` is `dir` or inside it, under `rules` (this platform's, but for
/// a test of another's).
fn starts_with_dir(path: &Path, dir: &Path, rules: PathRules) -> bool {
    path_is_within(&path.to_string_lossy(), &dir.to_string_lossy(), rules)
}

/// Whether `path` is `dir` or inside it, compared component by component as text
/// under `rules` - pure, so every platform's rules can be tested on any host.
/// Trailing and repeated separators are ignored; `.`/`..` are expected folded
/// already (`lexical`, or a canonical form).
pub(crate) fn path_is_within(path: &str, dir: &str, rules: PathRules) -> bool {
    let (path, dir) = (path_components(path, rules), path_components(dir, rules));
    !dir.is_empty() && path.len() >= dir.len() && path[..dir.len()] == dir[..]
}

/// `path` as the components [`path_is_within`] compares: a leading separator (the
/// root, or a UNC path's `\\`) is a component of its own - and under Windows rules
/// a UNC path's `\\` is a different one from a rooted path's `\` (#576 RV8a-F5):
/// `\\server\share\x` and `\server\share\x` (that folder on the current drive)
/// are different places, which split into the same names otherwise.
///
/// [#576 W24] Under Windows rules a name is compared as Win32 opens it
/// ([`win32_name`]), except in a verbatim (`\\?\`) path, which reaches the file
/// system as typed.
fn path_components(path: &str, rules: PathRules) -> Vec<String> {
    let (mut text, win32_names) = if rules.windows {
        (windows_form(path), !path.starts_with(r"\\?\"))
    } else {
        (path.to_owned(), false)
    };
    if rules.fold_case {
        text = text.to_lowercase();
    }
    let separator = if rules.windows { '\\' } else { '/' };
    let mut parts = Vec::new();
    if rules.windows && text.starts_with(r"\\") {
        parts.push(r"\\".to_owned());
    } else if text.starts_with(separator) {
        parts.push(separator.to_string());
    }
    let names: Vec<&str> = text
        .split(separator)
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    // The last name, unless the path ends in a separator: then it ends in a folder.
    let last = names
        .len()
        .checked_sub(1)
        .filter(|_| !text.ends_with(separator));
    parts.extend(
        names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                if win32_names {
                    win32_name(name, Some(i) == last)
                } else {
                    name
                }
            })
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
    );
    parts
}

/// `name` as Win32 opens it (#576 W24): a folder's name loses one trailing dot,
/// the last name every trailing dot and space. `C:\Windows.\x` is written in
/// `C:\Windows`; `C:\Windows \x` and `C:\Windows..\x` in other folders. `..`
/// is left as it is (folded before this compare).
fn win32_name(name: &str, last: bool) -> &str {
    if name.bytes().all(|b| b == b'.') {
        name
    } else if last {
        name.trim_end_matches(['.', ' '])
    } else {
        name.strip_suffix('.').unwrap_or(name)
    }
}

/// `path` as the Windows rules compare it: `/` read as `\`, and a DOS device
/// prefix (`\\?\` verbatim, `\\.\` device) taken off, leaving the drive path it
/// names - or, before `UNC\`, the UNC path (#576 W24): `\\?\UNC\s\x`,
/// `\\?\unc\s\x` and `\\.\UNC\s\x` are all `\\s\x`. `UNC` in any letter case:
/// Windows looks it up in its object namespace, which ignores case.
fn windows_form(path: &str) -> String {
    let text = path.replace('/', "\\");
    let Some(rest) = text
        .strip_prefix(r"\\?\")
        .or_else(|| text.strip_prefix(r"\\.\"))
    else {
        return text;
    };
    match rest.split_at_checked(4) {
        Some((unc, share)) if unc.eq_ignore_ascii_case(r"UNC\") => format!(r"\\{share}"),
        _ => rest.to_owned(),
    }
}

/// The system directories no command may write into.
fn system_dirs() -> Vec<PathBuf> {
    if cfg!(windows) {
        [
            "SystemRoot",
            "windir",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
        ]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .collect()
    } else {
        [
            "/bin",
            "/boot",
            "/dev",
            "/etc",
            "/lib",
            "/lib32",
            "/lib64",
            "/libx32",
            "/proc",
            "/sbin",
            "/sys",
            "/usr",
            "/System",
            "/private/etc",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }
}
