//! Crash-safe replacement of an export's destination file (#576 B9).
//!
//! Every exporter used to open its destination with `O_TRUNC` and write into it, so an export that
//! failed part-way (a full disk, a network share dropping, the app being killed) had already
//! destroyed the previous copy and left a torn file under its name. [`write_atomic`] stages the
//! bytes in a sibling temp file, flushes them to the device and renames the temp file over the
//! destination - the same shape the `.stalx` save uses (docs/saving.md) - so whoever opens the
//! destination sees the whole old file or the whole new one, never a blend.
//!
//! Not `config::atomic_write`: that writer lives in another crate, its temp names are coupled to the
//! stale-temp sweep of the config directory, and it creates its file 0600 because the config is
//! private. An export lands in a user directory nobody sweeps and must come out with the
//! permissions an ordinary save gives it.

use anyhow::{Context, Result};
use std::fs::{File, Metadata, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// How many temp names [`write_atomic`] tries before giving up. Each name carries 122 random bits,
/// so this only bounds a directory that hands back `EEXIST` for some pathological reason, turning a
/// would-be endless loop into an error.
const TEMP_NAME_ATTEMPTS: usize = 8;

/// Replace `path` with `bytes`. See [`write_atomic`].
pub fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic(path, |file| Ok(file.write_all(bytes)?))
}

/// Replace `path` with whatever `fill` writes into the file it is handed.
///
/// `fill` gets the open temp file, which is seekable so a ZIP writer can wrap it; anything it
/// buffers it must flush before returning. The file is then `sync_all`ed, which is also where a
/// write-back error (a full disk under delayed allocation, a quota, an NFS or SMB server) finally
/// surfaces instead of being dropped with the handle, and only then renamed over `path`.
///
/// On any error the temp file is removed and `path` is left exactly as it was.
pub fn write_atomic(path: &Path, fill: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let dest = resolve_destination(path);
    let previous = existing_destination(&dest)?;
    let (temp_path, mut file) = create_temp_beside(&dest, previous.as_ref())?;
    let staged = fill(&mut file).and_then(|()| {
        file.sync_all()
            .context("Failed to flush the export to disk")
    });
    drop(file);
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&temp_path, &dest) {
        // The destination is untouched: the user keeps the copy they had.
        let _ = std::fs::remove_file(&temp_path);
        return Err(anyhow::anyhow!(
            "Failed to replace {} with the new export: {e}",
            dest.display()
        ));
    }
    sync_parent_dir(&dest);
    Ok(())
}

/// Where the bytes should land: the file a symlink points at, not the link itself.
///
/// The in-place write this replaces followed the link. Renaming onto the link's own name would
/// instead replace the link with a regular file, silently cutting whatever the user had linked it
/// to. A dangling link has no target to resolve, so it is replaced like any other name.
fn resolve_destination(path: &Path) -> PathBuf {
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if is_link {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

/// The destination's metadata when it is an existing regular file, after checking it could have
/// been overwritten in place.
///
/// A rename needs write access to the DIRECTORY, not to the file, so without the check an export
/// would now replace a file the user made read-only (or one another account owns) where the
/// in-place write was refused. Opening for write WITHOUT truncating is the access check
/// `File::create` used to make, minus the damage.
fn existing_destination(dest: &Path) -> Result<Option<Metadata>> {
    let Ok(metadata) = std::fs::metadata(dest) else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    OpenOptions::new()
        .write(true)
        .open(dest)
        .with_context(|| format!("Cannot overwrite {}", dest.display()))?;
    Ok(Some(metadata))
}

/// Create a fresh temp file beside `dest`, retrying on a name that is already taken.
///
/// A SIBLING, not a file in `$TMPDIR`: a rename across filesystems is not atomic and on most
/// platforms fails outright. Named `.<dest name>.<random>.tmp` so a file left by a crash between the
/// create and the rename is hidden and still says which export it belonged to.
fn create_temp_beside(dest: &Path, previous: Option<&Metadata>) -> Result<(PathBuf, File)> {
    let name = dest
        .file_name()
        .with_context(|| format!("Export destination {} has no file name", dest.display()))?
        .to_string_lossy();
    // Bounded so a long destination name cannot push the temp name past NAME_MAX.
    let prefix: String = name.chars().take(40).collect();
    let mut attempts = 0;
    loop {
        attempts += 1;
        let candidate =
            dest.with_file_name(format!(".{prefix}.{}.tmp", uuid::Uuid::new_v4().simple()));
        match create_temp_file(&candidate, previous) {
            Ok(file) => return Ok((candidate, file)),
            // Not removed: it is not ours, so it is someone else's in-flight temp file.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if attempts >= TEMP_NAME_ATTEMPTS {
                    return Err(anyhow::anyhow!(
                        "Failed to create a temporary file beside {}: {e}",
                        dest.display()
                    ));
                }
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to create a temporary file beside {}: {e}",
                    dest.display()
                ))
            }
        }
    }
}

/// `create_new` (O_EXCL) so a name planted as a symlink is refused rather than followed.
///
/// On unix the mode is the one the result should end up with: what `File::create` gives a new file
/// (0666 under the umask), or the mode the existing destination already has - re-applied after the
/// open, because the umask narrows the create mode and the in-place write kept the user's mode
/// exactly. Best-effort: a failure to widen leaves the file narrower, never wider, than the user
/// chose. Windows has no mode bits to carry; access there is the directory's inherited ACL, the
/// same for the old file and the new one.
fn create_temp_file(path: &Path, previous: Option<&Metadata>) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let keep = previous.map(|m| m.permissions().mode() & 0o777);
        options.mode(keep.unwrap_or(0o666));
        let file = options.open(path)?;
        if let Some(mode) = keep {
            if let Err(e) = file.set_permissions(std::fs::Permissions::from_mode(mode)) {
                tracing::warn!(error = %e, "Could not carry the export's permissions onto its replacement");
            }
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = previous;
        options.open(path)
    }
}

/// Best-effort: makes the rename itself durable. Opening a directory as a file is not portable (it
/// fails on Windows, whose filesystem journals the rename anyway), and a failure only means the
/// directory entry may lag the data, which is not worth failing a completed export over.
fn sync_parent_dir(dest: &Path) {
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "tests build their fixtures in a fresh temp dir; a failed unwrap should fail the test"
    )]

    use super::*;
    use crate::export::{
        export_csv, export_image, export_json, export_tml, ExportImageOptions, ImageFormat,
    };
    use crate::models::{Project, SectionData, ShotData};

    /// What the destination held before the export: the previous good copy.
    const OLD: &[u8] = b"OLD EXPORT - the previous copy the user still has\n";

    type Exporter = fn(&Project, &Path) -> Result<()>;

    fn project() -> Project {
        let mut project = Project::new("CaveA".to_string(), None);
        project.add_section(SectionData {
            name: Some("SectionA".to_string()),
            color: "#4ecdc4".to_string(),
            quality: 1.0,
            shots: vec![
                ShotData {
                    azimuth_in: 45.0,
                    azimuth_out: 225.0,
                    length: 10.0,
                    depth_in: 5.0,
                    depth_out: 6.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 90.0,
                    azimuth_out: 270.0,
                    length: 8.0,
                    depth_in: 6.0,
                    depth_out: 7.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project
    }

    fn image(format: ImageFormat) -> ExportImageOptions {
        ExportImageOptions {
            format,
            dpi: 72,
            ..Default::default()
        }
    }

    /// Every write path the export crate has, by destination extension. `tml` (a ZIP) and `tmlu`
    /// (bare XML) are separate write paths inside `export_tml`.
    fn exporters() -> [(&'static str, Exporter); 7] {
        [
            ("csv", export_csv),
            ("json", export_json),
            ("tml", |p, path| export_tml(p, path).map(|_| ())),
            ("tmlu", |p, path| export_tml(p, path).map(|_| ())),
            ("svg", |p, path| {
                export_image(p, path, &image(ImageFormat::Svg))
            }),
            ("png", |p, path| {
                export_image(p, path, &image(ImageFormat::Png))
            }),
            ("pdf", |p, path| {
                export_image(p, path, &image(ImageFormat::Pdf))
            }),
        ]
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The previous file's bytes are never rewritten in place (#576 B9).
    ///
    /// A second hard link pins the OLD file's bytes under another name. An export that truncates
    /// and refills its destination writes into those very bytes, so the second name would show the
    /// new (or, on a failure part-way, a torn) export; an export that stages a temp file and renames
    /// it over the destination leaves them alone. Unix-only because the assertion leans on inode
    /// semantics; the failure-path behaviour itself is pinned portably by the `write_atomic` tests.
    #[cfg(unix)]
    #[test]
    fn every_export_replaces_its_destination_instead_of_rewriting_it_in_place() {
        let mut failures = Vec::new();
        for (ext, export) in exporters() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("CaveA.{ext}"));
            std::fs::write(&path, OLD).unwrap();
            let previous = dir.path().join("previous");
            std::fs::hard_link(&path, &previous).unwrap();

            export(&project(), &path).unwrap();

            let kept = std::fs::read(&previous).unwrap();
            if kept != OLD {
                failures.push(format!(
                    "{ext}: rewrote the previous file in place (its bytes are now {} bytes of the \
                     new export, not the {}-byte OLD copy)",
                    kept.len(),
                    OLD.len()
                ));
            }
            let written = std::fs::read(&path).unwrap();
            if written.is_empty() || written == OLD {
                failures.push(format!(
                    "{ext}: the destination does not hold the new export"
                ));
            }
            let names = names_in(dir.path());
            if names != [format!("CaveA.{ext}"), "previous".to_string()] {
                failures.push(format!(
                    "{ext}: unexpected files beside the export: {names:?}"
                ));
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    #[test]
    fn a_write_that_fails_part_way_leaves_the_previous_file_and_no_temp_file() {
        // The failure the issue is about: the writer gets some way in and then errors (a full disk,
        // a dropped share). The old code had truncated the destination before the first byte.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CaveA.csv");
        std::fs::write(&path, OLD).unwrap();

        let err = write_atomic(&path, |file| {
            file.write_all(&[b'x'; 4096])?;
            Err(anyhow::anyhow!("No space left on device"))
        })
        .unwrap_err();

        assert_eq!(err.to_string(), "No space left on device");
        assert_eq!(std::fs::read(&path).unwrap(), OLD);
        assert_eq!(names_in(dir.path()), ["CaveA.csv"]);
    }

    #[test]
    fn a_successful_write_replaces_the_file_or_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("CaveA.json");
        std::fs::write(&existing, OLD).unwrap();
        write_atomic_bytes(&existing, b"new").unwrap();
        assert_eq!(std::fs::read(&existing).unwrap(), b"new");

        let fresh = dir.path().join("CaveB.json");
        write_atomic_bytes(&fresh, b"fresh").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"fresh");
        assert_eq!(names_in(dir.path()), ["CaveA.json", "CaveB.json"]);
    }

    #[test]
    fn a_missing_directory_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-dir").join("CaveA.csv");
        let err = write_atomic_bytes(&path, b"x").unwrap_err();
        assert!(
            err.to_string()
                .starts_with("Failed to create a temporary file beside"),
            "{err}"
        );
    }

    /// The in-place write kept the destination's mode; a replacement must too, and a new file must
    /// get exactly what `File::create` would have given it.
    #[cfg(unix)]
    #[test]
    fn the_destination_keeps_its_mode_and_a_new_file_gets_the_create_default() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();

        let existing = dir.path().join("CaveA.svg");
        std::fs::write(&existing, OLD).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o664)).unwrap();
        write_atomic_bytes(&existing, b"new").unwrap();
        assert_eq!(mode(&existing), 0o664);

        let probe = dir.path().join("probe");
        File::create(&probe).unwrap();
        let fresh = dir.path().join("CaveB.svg");
        write_atomic_bytes(&fresh, b"fresh").unwrap();
        assert_eq!(mode(&fresh), mode(&probe));
    }

    /// The in-place write followed a symlink into its target; the rename must not instead swap the
    /// link for a regular file.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_destination_is_written_through_and_stays_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.pdf");
        std::fs::write(&real, OLD).unwrap();
        let link = dir.path().join("CaveA.pdf");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic_bytes(&link, b"new").unwrap();

        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&real).unwrap(), b"new");
        assert_eq!(names_in(dir.path()), ["CaveA.pdf", "real.pdf"]);
    }

    /// A rename only needs the directory to be writable, so without the access probe a read-only
    /// destination the in-place write refused would now be replaced.
    #[cfg(unix)]
    #[test]
    fn a_read_only_destination_is_still_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CaveA.png");
        std::fs::write(&path, OLD).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        // Root writes through 0444, and then there is no refusal to preserve:
        // fail rather than pass on nothing (#576 NF-ROOT-PERM).
        crate::models::test_support::assert_file_refuses_writes(&path);

        let err = write_atomic_bytes(&path, b"new").unwrap_err();

        assert!(err.to_string().starts_with("Cannot overwrite"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), OLD);
        assert_eq!(names_in(dir.path()), ["CaveA.png"]);
    }
}
