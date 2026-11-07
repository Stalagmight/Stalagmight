//! Which files a survey project's links may name, and where they resolve: ONE
//! rule set for the Walls `.PATH` and the Compass `.MAK` `#file` link (#576
//! SEC-1, B8). They used to be two hand-kept copies of the same rules and
//! refusal strings (RV3-R-18), so a fix to one (RV3-R-10) had to be made twice.
//!
//! [`resolve_link`] works on the link's TEXT alone and refuses what must never
//! reach the file system; [`outside_root`] then asks the file system where an
//! accepted link really leads.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::parsers::encoding::FileFamily;

/// The longest path a link may resolve to. No file system opens a longer
/// path (Windows' extended-length limit is 32,767 UTF-16 units, Linux's is
/// 4,096 bytes), so such a file could only ever be reported missing; the bound
/// keeps a crafted chain of Walls `.PATH` lines from growing every open book's
/// copy of its folder (#576 B12).
pub(crate) const MAX_RESOLVED_PATH_BYTES: usize = 32 * 1024;

/// Windows' reserved DOS device names. A path whose component is one of them,
/// in any case, with or without an extension (`NUL.dat`), trailing dots and
/// spaces ignored, opens the device, not a file (#576 RV2-R-10).
const DOS_DEVICE_NAMES: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "CONIN$",
    "CONOUT$",
    "COM0",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT0",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// Whether one path component names a DOS device: its stem - up to the first
/// `:` or `.`, trailing dots and spaces dropped - is a reserved name in any
/// case. Win32 opens the device for a reserved name followed by `:` too
/// (`COM1:`, `CON:`, and `nul:x`, the alternate-data-stream form), so
/// everything from the first `:` is dropped first (#576 RV4-R-P4).
pub(crate) fn is_dos_device_name(component: &str) -> bool {
    let before_colon = component
        .split_once(':')
        .map_or(component, |(head, _)| head);
    let trimmed = before_colon.trim_end_matches(['.', ' ']);
    let stem = trimmed
        .split_once('.')
        .map_or(trimmed, |(stem, _)| stem)
        .trim_end_matches(' ');
    DOS_DEVICE_NAMES
        .iter()
        .any(|device| device.eq_ignore_ascii_case(stem))
}

/// `path` with `.` dropped and each `..` taking off the component before it,
/// never above the root - what the operating system will make of it.
fn lexically_normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    normalized.components().next_back(),
                    None | Some(Component::RootDir | Component::Prefix(_))
                ) {
                    normalized.pop();
                }
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Resolve a link's text (a Walls `.PATH`, a Compass `.MAK` `#file`) against
/// `base`, by the rules both importers follow (#576 SEC-1, B8).
///
/// Backslashes are separators (both programs are Windows programs). An
/// absolute local path REPLACES `base` (genuine Walls and Compass honor it)
/// and comes back normalized, so `..` in it cannot hide where it leads; the
/// caller warns when it leaves the project ([`outside_root`]). A relative path
/// builds on `base`, with `..` clamped at `root` (the project file's own
/// folder), so a crafted `../../../../` cannot walk out of the project tree.
///
/// Refused, from the text alone and before anything touches the file system:
/// - a network (UNC) or device-namespace path (`\\server\share`,
///   `\\?\C:\...`, `\\.\COM1`) - on Windows even asking whether
///   `\\server\share\x` exists hands the user's credentials to that server;
/// - a component that is a DOS device name (`CON`, `NUL.dat`, `COM1`), which
///   opens the device on Windows (#576 RV2-R-10);
/// - a drive letter that is not part of an absolute path on this system
///   (`D:\Caves` on macOS/Linux, `D:Caves` anywhere), which would otherwise
///   resolve against the process's working directory;
/// - a path longer than any file system opens.
pub(crate) fn resolve_link(base: &Path, root: &Path, link: &str) -> Result<PathBuf, &'static str> {
    let normalized = link.replace('\\', "/");
    // `\\server\share`, `\\?\C:\...` and `\\.\COM1` all start with two separators.
    if normalized.starts_with("//") {
        return Err("network (UNC) and device paths are never read");
    }
    if normalized.split('/').any(is_dos_device_name) {
        return Err("it names a Windows device (CON, NUL, COM1 ...), not a file");
    }
    let candidate = Path::new(&normalized);
    let resolved = if candidate.is_absolute() {
        lexically_normalized(candidate)
    } else if normalized.contains(':') {
        return Err("its drive letter cannot be resolved on this system");
    } else {
        let mut resolved = base.to_path_buf();
        for part in normalized.split('/') {
            match part {
                "" | "." => {}
                // Never pop above the project root (traversal guard). When
                // `resolved` is not under `root` at all - a parent set an
                // absolute `.PATH`, so it is a different tree - `starts_with`
                // is false and it is left alone: that absolute base is the
                // author's explicit choice.
                ".." => {
                    if resolved != root && resolved.starts_with(root) {
                        resolved.pop();
                    }
                }
                other => resolved.push(other),
            }
        }
        resolved
    };
    if resolved.as_os_str().len() > MAX_RESOLVED_PATH_BYTES {
        return Err("its folder path is longer than any file system allows");
    }
    Ok(resolved)
}

/// Where `path` - a file a link resolved to, about to be read - really is,
/// when that is outside `root` (#576 RV3-R-10, RV2-R-9); `None` when it is
/// inside. Both are resolved on disk first, so an absolute link and a link
/// through a symlink inside the project that points out of it are caught
/// alike. Touches the file system, so only for a link [`resolve_link`]
/// accepted. When either cannot be resolved on disk, they are compared as
/// [`resolve_link`] normalized them.
pub(crate) fn outside_root(path: &Path, root: &Path) -> Option<PathBuf> {
    match (std::fs::canonicalize(path), std::fs::canonicalize(root)) {
        (Ok(real), Ok(real_root)) => (!real.starts_with(real_root)).then_some(real),
        _ => (!path.starts_with(lexically_normalized(root))).then(|| path.to_path_buf()),
    }
}

/// The most bytes the survey files one project links may add up to: a Compass
/// `.MAK`'s `.DAT` files (#576 RV2-R-12), a Walls `.WPJ`'s `.SRV` files
/// (NF-FIX3R-2). Each file has its own ceiling ([`FileFamily::SurveyText`]);
/// this bounds a project that lists many of them. Four of those ceilings, far
/// above any real project.
pub(crate) const MAX_LINKED_BYTES: u64 = 4 * FileFamily::SurveyText.byte_limit();

/// What the survey files one project links have used up so far.
pub(crate) struct LinkBudget {
    /// Every file read so far, as resolved on disk, so a further listing of
    /// one is skipped instead of read, held and imported once more.
    read: HashSet<PathBuf>,
    /// Their sizes, added up against `limit`.
    bytes: u64,
    limit: u64,
}

/// Whether a linked file that exists may be read ([`LinkBudget::admit`]).
pub(crate) enum Admission {
    /// The project already read this file, under this or another spelling.
    AlreadyRead,
    /// With it, the project's files would add up to more than `limit` bytes.
    OverLimit { limit: u64 },
    /// Read it, then [`LinkBudget::record`] the ticket.
    Fits(Ticket),
}

/// A file [`LinkBudget::admit`] let through: what to record once it is read.
pub(crate) struct Ticket {
    identity: PathBuf,
    size: u64,
}

impl LinkBudget {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            read: HashSet::new(),
            bytes: 0,
            limit,
        }
    }

    /// Whether the file at `resolved` (it exists) may be read.
    pub(crate) fn admit(&self, resolved: &Path) -> Admission {
        let identity = std::fs::canonicalize(resolved).unwrap_or_else(|_| resolved.to_path_buf());
        if self.read.contains(&identity) {
            return Admission::AlreadyRead;
        }
        let size = std::fs::metadata(resolved).map_or(0, |m| m.len());
        if self.bytes.saturating_add(size) > self.limit {
            return Admission::OverLimit { limit: self.limit };
        }
        Admission::Fits(Ticket { identity, size })
    }

    /// The admitted file was read: it counts against the budget from now on.
    pub(crate) fn record(&mut self, ticket: Ticket) {
        self.read.insert(ticket.identity);
        self.bytes = self.bytes.saturating_add(ticket.size);
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_recognised_with_or_without_an_extension() {
        for device in [
            "CON",
            "con",
            "Nul",
            "NUL.dat",
            "nul.tar.gz",
            "COM1",
            "com9.txt",
            "LPT1",
            "AUX",
            "aux .dat",
            "PRN.",
            "PRN ",
            "CONIN$",
            "conout$.x",
            "COM\u{b9}",
        ] {
            assert!(is_dos_device_name(device), "{device:?}");
        }
        for file in [
            "CONSOLE.dat",
            "COM10.dat",
            "LPT",
            "COM",
            "NULL",
            "con-1.dat",
            "a.con",
            "xNUL",
            "",
            ".",
            "..",
        ] {
            assert!(!is_dos_device_name(file), "{file:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_absolute_link_comes_back_normalized() {
        let root = Path::new("/proj");
        assert_eq!(
            resolve_link(root, root, "/proj/../etc"),
            Ok(PathBuf::from("/etc"))
        );
        assert_eq!(
            resolve_link(root, root, "/proj/./a/../b"),
            Ok(PathBuf::from("/proj/b"))
        );
        assert_eq!(
            resolve_link(root, root, "/../../x"),
            Ok(PathBuf::from("/x"))
        );
        assert_eq!(
            resolve_link(root, root, "\\proj\\sub\\NUL.dat"),
            Err("it names a Windows device (CON, NUL, COM1 ...), not a file")
        );
    }

    #[cfg(unix)]
    #[test]
    fn outside_root_follows_symlinks_and_dotdot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("outside.dat");
        std::fs::write(&outside, b"x").unwrap();
        std::fs::write(root.join("inside.dat"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.dat")).unwrap();

        assert_eq!(outside_root(&root.join("inside.dat"), &root), None);
        let real_outside = std::fs::canonicalize(&outside).unwrap();
        assert_eq!(
            outside_root(&root.join("link.dat"), &root),
            Some(real_outside.clone())
        );
        assert_eq!(
            outside_root(&root.join("..").join("outside.dat"), &root),
            Some(real_outside)
        );
        // Not on disk: compared as written (normalized).
        assert_eq!(outside_root(&root.join("missing.dat"), &root), None);
        assert!(outside_root(&dir.path().join("missing.dat"), &root).is_some());
    }
}
