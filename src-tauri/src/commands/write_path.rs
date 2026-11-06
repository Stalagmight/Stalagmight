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
    let why = location_refusal(path, true, dirs, Walk::HOST)?;
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
    let forms_of =
        |dir: &Path| path_forms(dir, walk).unwrap_or_else(|| vec![lexical(&absolute(dir))]);
    let inside = |dir: &Path| {
        let dir_forms = forms_of(dir);
        forms
            .iter()
            .any(|p| dir_forms.iter().any(|d| starts_with_dir(p, d, walk.rules)))
    };
    // [#576 FZ-PERSIST-fs-1] The managed-cache exception is for a project the
    // write LANDS in the cache: judged by the place it reaches (the last form),
    // not by the path as given. `<cache>/speleodb/link/x.stalx`, with `link`
    // leading into the settings folder, writes into the settings folder - and
    // was let through because it was spelled inside the cache.
    let lands_inside = |dir: &Path| {
        let dir_forms = forms_of(dir);
        forms
            .last()
            .is_some_and(|p| dir_forms.iter().any(|d| starts_with_dir(p, d, walk.rules)))
    };
    let managed = project && dirs.managed_projects.as_deref().is_some_and(lands_inside);
    if !managed && dirs.internal.iter().any(|d| inside(d)) {
        return Some("that is inside the app's own settings, data or cache folder".to_owned());
    }
    if system_dirs().iter().any(|d| inside(d)) {
        return Some("that is inside a system folder".to_owned());
    }
    None
}

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
    /// Windows path syntax: `\` and `/` both separate, and a verbatim prefix
    /// (`\\?\C:\..`, `\\?\UNC\server\share\..`) names the same place as the plain
    /// form (`C:\..`, `\\server\share\..`).
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
fn path_components(path: &str, rules: PathRules) -> Vec<String> {
    let mut text = if rules.windows {
        let text = path.replace('/', "\\");
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else if let Some(rest) = text
            .strip_prefix(r"\\?\")
            .or_else(|| text.strip_prefix(r"\\.\"))
        {
            rest.to_owned()
        } else {
            text
        }
    } else {
        path.to_owned()
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
    parts.extend(
        text.split(separator)
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_owned),
    );
    parts
}

/// The system directories no command may write into.
#[cfg(not(test))]
fn system_dirs() -> Vec<PathBuf> {
    host_system_dirs()
}

/// [#576 FZ-PERSIST] The system directories, and the stand-ins a test added on
/// its thread ([`test_system_dirs`]).
#[cfg(test)]
fn system_dirs() -> Vec<PathBuf> {
    let mut dirs = host_system_dirs();
    dirs.extend(test_system_dirs::added());
    dirs
}

/// The system directories of the platform this build runs on.
fn host_system_dirs() -> Vec<PathBuf> {
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

/// [#576 FZ-PERSIST] Test-only stand-ins for system folders. The real ones
/// (`/etc`, `C:\Windows`, ...) lie outside any temp tree, so a test that really
/// writes - the write-policy fuzzer - names folders of its own temp tree, which
/// [`system_dirs`] then lists too, on that thread until the guard drops.
#[cfg(test)]
pub(crate) mod test_system_dirs {
    use std::cell::RefCell;
    use std::path::PathBuf;

    thread_local! {
        static ADDED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    /// `dirs` are system folders on this thread until the guard drops.
    #[must_use = "the folders are system folders only while the guard lives"]
    pub(crate) fn add(dirs: Vec<PathBuf>) -> Added {
        ADDED.with(|added| added.borrow_mut().extend(dirs.iter().cloned()));
        Added(dirs)
    }

    /// Takes its folders back on drop, so a failed assertion cannot leak them
    /// into the next test on the same thread.
    pub(crate) struct Added(Vec<PathBuf>);

    impl Drop for Added {
        fn drop(&mut self) {
            ADDED.with(|added| added.borrow_mut().retain(|dir| !self.0.contains(dir)));
        }
    }

    /// The folders added on this thread.
    pub(super) fn added() -> Vec<PathBuf> {
        ADDED.with(|added| added.borrow().clone())
    }
}
