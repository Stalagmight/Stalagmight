//! Replace an export target atomically (#570).
//!
//! The export target is commonly the very file the user is re-exporting over, so a
//! direct truncating write (`std::fs::write`, `File::create`) destroys it the moment
//! the export fails or is interrupted. Every exporter goes through [`write_atomic`]
//! instead, the same contract the project save keeps for its `.stalx.tmp`
//! (docs/saving.md): build the whole file beside the target, then rename it into place.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// Write `path` through a temp file in the same directory, renamed over `path`
/// only once `write` has filled it and it is synced to disk.
///
/// The temp lives in the SAME directory so the rename never crosses filesystems.
/// On any failure - `write` erroring, the flush/sync, or the rename itself - the
/// temp is removed and whatever was at `path` is left exactly as it was.
///
/// Renaming replaces a symlink at `path` rather than writing through it, as the
/// project save does.
pub fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<()>,
) -> Result<()> {
    let temp = temp_path_for(path)?;
    // `create_new` refuses anything already at the name, so a symlink planted
    // there is never followed and its target never truncated.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .with_context(|| {
            format!(
                "Failed to create a temporary file beside {}",
                path.display()
            )
        })?;
    // Before any content is written, so the export never exists at a wider mode
    // than the file it replaces.
    #[cfg(unix)]
    inherit_destination_mode(&file, path);
    // The file handle is closed by the time `fill_and_sync` returns, which
    // Windows requires before the temp can be renamed or removed.
    let result = fill_and_sync(file, write).and_then(|()| {
        std::fs::rename(&temp, path)
            .with_context(|| format!("Failed to move the export into place at {}", path.display()))
    });
    if result.is_err() {
        if let Err(e) = std::fs::remove_file(&temp) {
            tracing::warn!(
                path = ?temp,
                error = %e,
                "Could not remove the temporary export file; it is left behind"
            );
        }
    }
    result
}

fn fill_and_sync(file: File, write: impl FnOnce(&mut BufWriter<File>) -> Result<()>) -> Result<()> {
    let mut writer = BufWriter::new(file);
    write(&mut writer)?;
    let file = writer
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)
        .context("Failed to flush the export")?;
    file.sync_all().context("Failed to sync the export to disk")
}

/// Give the temp the mode of the file it will replace. A truncating write kept
/// the existing inode and so its mode; a freshly created temp is
/// `0o666 & ~umask`, and the rename would carry that onto the target, so an
/// export the user had `chmod 600`'d would come back world-readable. A NEW
/// export (no target yet) keeps the umask default, as before.
///
/// Unix-only: on Windows the governing ACL is inherited from the directory by
/// both files. Best-effort, as in the config writer
/// (`crates/core/src/config/atomic_write.rs`): an export must not fail over the
/// mode. Setuid/setgid/sticky are dropped.
#[cfg(unix)]
fn inherit_destination_mode(temp: &File, dest: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::metadata(dest) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o777;
    if let Err(e) = temp.set_permissions(std::fs::Permissions::from_mode(mode)) {
        tracing::warn!(
            error = %e,
            "Could not carry the export target's permissions onto its replacement"
        );
    }
}

/// `<dir>/.<name>.<uuid>.stalagmight-export.tmp`: hidden on Unix, unpredictable,
/// and (unless the project file itself starts with a dot) never a prefix match
/// for the project's own backup scan.
fn temp_path_for(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .with_context(|| format!("Export path has no file name: {}", path.display()))?;
    let mut temp_name = OsString::from(".");
    temp_name.push(name);
    // A random nonce, so neither concurrent exports nor another user with
    // write access to the folder can predict (and pre-create) the name.
    temp_name.push(format!(
        ".{}.stalagmight-export.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    Ok(path.with_file_name(temp_name))
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Every entry in `dir`, sorted, so a stray temp file shows up by name.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn failed_write_leaves_existing_target_and_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cave.tmlu");
        std::fs::write(&target, b"the previous export").unwrap();

        let result = write_atomic(&target, |w| {
            w.write_all(b"half an export")?;
            w.flush()?;
            anyhow::bail!("interrupted mid-stream")
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"the previous export");
        assert_eq!(entries(dir.path()), vec!["cave.tmlu".to_string()]);
    }

    #[test]
    fn failed_rename_removes_the_temp() {
        // A non-empty directory squatting the target makes the rename fail after
        // the temp is fully written - for root too, so this is root-safe.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cave.tml");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"x").unwrap();

        let result = write_atomic(&target, |w| Ok(w.write_all(b"new export")?));

        assert!(result.is_err());
        assert_eq!(entries(dir.path()), vec!["cave.tml".to_string()]);
        assert_eq!(std::fs::read(target.join("keep")).unwrap(), b"x");
    }

    #[test]
    fn missing_parent_dir_errors_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("missing").join("cave.tml");

        assert!(write_atomic(&target, |w| Ok(w.write_all(b"x")?)).is_err());
        assert!(entries(dir.path()).is_empty());
    }

    #[test]
    fn success_replaces_existing_target_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cave.csv");
        std::fs::write(&target, b"an older, longer export than the new one").unwrap();

        write_atomic(&target, |w| Ok(w.write_all(b"new")?)).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(entries(dir.path()), vec!["cave.csv".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_at_target_is_replaced_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.stalx");
        std::fs::write(&victim, b"not yours").unwrap();
        let target = dir.path().join("cave.tmlu");
        std::os::unix::fs::symlink(&victim, &target).unwrap();

        write_atomic(&target, |w| Ok(w.write_all(b"export")?)).unwrap();

        assert_eq!(std::fs::read(&victim).unwrap(), b"not yours");
        assert!(!std::fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), b"export");
    }

    #[cfg(unix)]
    #[test]
    fn re_export_keeps_the_target_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cave.csv");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&target, |w| Ok(w.write_all(b"new")?)).unwrap();

        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }
}
