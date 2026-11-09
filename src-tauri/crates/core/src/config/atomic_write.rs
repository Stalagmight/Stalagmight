//! Crash-safe replacement of `config.toml`, and cleanup of what an interrupted
//! replacement leaves behind.
//!
//! Split out of `types.rs` rather than rebaselined when the pair outgrew the
//! file-size ratchet (#157). The two functions here are one concern: the write
//! creates the temp files, and the sweep is the only thing that collects them.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// How old an abandoned `config.toml.<pid>.<n>.<nonce>.tmp` must be before
/// [`sweep_stale_temp_files`] will delete it.
///
/// The sweep races live writers by construction, so this is a safety margin, not
/// a tuning knob: another process - or another `AppState` in this one - can be
/// between `File::create` and `fs::rename` at this instant, and deleting *that*
/// file reintroduces the exact "Failed to replace config file: No such file or
/// directory" the unique temp name exists to prevent. A real write holds the
/// temp file open for microseconds, so an hour is far past any live write while
/// still collecting leaked files within one session.
const STALE_TEMP_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// How many temp names [`write_atomically`] will try before giving up.
///
/// Each name carries 122 random bits, so a second collision after the first is
/// not a probability we are budgeting for - this bounds a directory that is
/// pathological for some other reason (an adversary watching `readdir` and
/// racing us, a filesystem handing back `EEXIST` for a name that is not there)
/// so the save fails with an error instead of spinning forever.
const TEMP_NAME_ATTEMPTS: usize = 8;

/// One candidate temp path beside `path`, named `config.toml.<pid>.<n>.<nonce>.tmp`.
///
/// The three parts answer three different failure modes and none of them is
/// decoration:
///
/// - `pid` and the monotonic counter make the name UNIQUE, which is what keeps
///   two writers on one config directory from clobbering each other's temp file
///   (the pid separates processes, the counter separates concurrent writers
///   inside one process - `AppState::config_write_lock` is per instance and
///   covers neither, and every tree on this machine shares one app-data dir).
/// - The random nonce makes it UNPREDICTABLE, which pid-plus-counter is not: the
///   counter starts at 0, so a bystander could compute `config.toml.<pid>.0.tmp`
///   for plausible pids and pre-create them. Against `create_new` (O_EXCL) that
///   is a denial of persistence - every settings save fails to create its temp
///   file - and the sweep would not clear it inside the hour.
///
/// `with_extension` replaces `.toml`, hence the `toml.` prefix in the format.
///
/// Reachable from the tests so they can assert against the name shape the sweep
/// has to keep matching, rather than hand-rolling a second copy of it that could
/// drift away from this one unnoticed.
pub(super) fn temp_candidate(path: &Path) -> PathBuf {
    static WRITE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = WRITE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nonce = uuid::Uuid::new_v4().simple();
    path.with_extension(format!("toml.{}.{seq}.{nonce}.tmp", std::process::id()))
}

/// Create the temp file, failing rather than following anything already at that
/// name.
///
/// `create_new` (O_CREAT|O_EXCL), NOT `create`: plain `create` is O_CREAT|O_TRUNC
/// WITHOUT O_EXCL, which follows symlinks. Anything able to write to the config
/// dir could pre-plant the name as a symlink, the write would truncate and fill
/// the link's target, and the rename would then move the symlink itself over
/// `config.toml` - leaving the live config path an attacker-controlled link for
/// every later write. O_EXCL fails outright on an existing name, symlink included.
///
/// The 0600 is the OTHER half, and it is about the file we are about to rename
/// INTO place. `fs::write` (what this replaced) opened the EXISTING `config.toml`
/// and kept its mode; a freshly created file is `0o666 & ~umask`, i.e. usually
/// world-readable, and the rename carries that mode onto the destination. So a
/// user who ran `chmod 600` on their config silently got it published back to
/// 0644 on the next settings save - and saves fire on every project open, create
/// and save. The file is not free of sensitive material: `[speleodb]` names the
/// SpeleoDB account and `general.recent_projects` holds absolute paths.
///
/// Created 0600 rather than chmod'ed after the fact so the window in which the
/// content exists at a wider mode is never open at all;
/// [`inherit_destination_mode`] then widens it back to whatever the destination
/// already was, if anything.
fn create_temp_file(temp_path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(temp_path)
}

/// Give the temp file the mode `config.toml` already has, so the rename replaces
/// the file without republishing it at the process umask.
///
/// Unix-only. Windows has no mode bits to carry: `set_permissions` there only
/// toggles the read-only attribute, and the ACL that actually governs access is
/// inherited from the containing directory by both the old file and the new one,
/// so the default behaviour is already the preserving one.
///
/// Best-effort: a save must not fail over the mode. Every failure direction here
/// leaves the file at 0600, which is more private than the user asked for rather
/// than less - the opposite direction is the defect.
///
/// The mask drops setuid/setgid/sticky rather than reproducing them on a file we
/// just created; none of them mean anything on a TOML document.
#[cfg(unix)]
fn inherit_destination_mode(temp_path: &Path, dest: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // No destination yet means there is no user intent to preserve, so the
    // private 0600 `create_temp_file` opened with stands.
    let Ok(metadata) = std::fs::metadata(dest) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o777;
    if let Err(e) = std::fs::set_permissions(temp_path, std::fs::Permissions::from_mode(mode)) {
        tracing::warn!(
            error = %e,
            "Could not carry the config file's permissions onto its replacement"
        );
    }
}

/// Write `contents` to `path` via a sibling temp file and a rename, the same
/// shape the project save uses (`core::state::project_io`).
///
/// `fs::write` truncates the destination before it writes a byte, so a crash or
/// a full disk part-way through left `config.toml` empty or holding half a TOML
/// document. Neither is recoverable by the load path and the failure is SILENT
/// in the worst direction: `Config::load()` is called through
/// `unwrap_or_default()` (`AppState::new`), so a truncated file reads back as
/// "no config" and every setting the user ever changed is reset without an error
/// anywhere. The rename is atomic, so a reader sees the whole old file or the
/// whole new one.
///
/// The temp file is a SIBLING rather than one in `$TMPDIR`: a rename across
/// filesystems is not atomic and on most platforms fails outright, and the
/// config directory is the one place we have just confirmed is writable.
///
/// `sync_all` before the rename is the durability half, and it is not redundant
/// with the atomicity half: without it the rename can reach the disk ahead of
/// the data, which on a crash leaves a correctly-named file full of zeroes -
/// i.e. exactly the silent reset above, reintroduced.
pub(super) fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    write_atomically_with(path, contents, &mut || temp_candidate(path))
}

/// [`write_atomically`] with the temp-name source injected, so a test can force
/// the collision that [`temp_candidate`]'s 122 random bits make unreachable from
/// the outside. Production has exactly one caller, above.
pub(super) fn write_atomically_with(
    path: &Path,
    contents: &str,
    next_temp_path: &mut dyn FnMut() -> PathBuf,
) -> Result<()> {
    write_with_mode(path, contents.as_bytes(), next_temp_path, true)
}

/// Write a credential document with mode 0600 even when replacing a wider file.
pub(super) fn write_private_atomically(path: &Path, contents: &str) -> Result<()> {
    write_private_bytes_atomically(path, contents.as_bytes())
}

/// Restore credential bytes verbatim, including an unreadable old document.
pub(super) fn write_private_bytes_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    write_with_mode(path, contents, &mut || temp_candidate(path), false)
}

fn write_with_mode(
    path: &Path,
    contents: &[u8],
    next_temp_path: &mut dyn FnMut() -> PathBuf,
    _preserve_mode: bool,
) -> Result<()> {
    // A temp file is left behind by any crash, power loss or SIGKILL between the
    // create and the rename, and because the name is unique per write it is never
    // overwritten - so they ACCUMULATE, one per interrupted write.
    // `Config::load_from_path` age-sweeps them; see `sweep_stale_temp_files`.
    // Nothing else collects them - in particular `sqlite::repair::list_backups`
    // only LISTS, never deletes, and only beside a project file, never in the
    // app-data config directory.
    let mut attempts = 0;
    let temp_path = loop {
        attempts += 1;
        let candidate = next_temp_path();
        let file = match create_temp_file(&candidate) {
            Ok(file) => file,
            // O_EXCL is fail-CLOSED on an existing name, so a single name that
            // happens to be taken must not end the save: a crash can leak
            // `config.toml.<pid>.<n>.<nonce>.tmp`, the pid can be reused inside
            // the sweep's one-hour margin, and the sweep deliberately spares a
            // file that young because a live writer may still be about to rename
            // it. Without this retry that leak takes out every save of the
            // session. A fresh name is the whole remedy - there is nothing to
            // repair about the file in the way.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // NOT removed. We did not create it, so it is either someone
                // else's in-flight temp file or a plant; deleting a live writer's
                // temp file reintroduces the ENOENT-on-rename the unique naming
                // exists to prevent.
                if attempts < TEMP_NAME_ATTEMPTS {
                    continue;
                }
                return Err(anyhow::Error::new(e).context("Failed to create temporary config file"));
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context("Failed to create temporary config file"))
            }
        };
        if let Err(e) = fill_temp_file(file, contents) {
            let _ = std::fs::remove_file(&candidate);
            return Err(e);
        }
        break candidate;
    };

    // Before the rename, not after: after it, the mode is already live on
    // `config.toml` and every reader in that window sees the wider one.
    #[cfg(unix)]
    if _preserve_mode {
        inherit_destination_mode(&temp_path, path);
    }

    if let Err(e) = std::fs::rename(&temp_path, path) {
        // The destination is untouched, which is the recoverable direction:
        // the user keeps the settings they had rather than losing all of them.
        let _ = std::fs::remove_file(&temp_path);
        return Err(anyhow::Error::new(e).context("Failed to replace config file"));
    }
    Ok(())
}

/// Write the document into the already-created temp file and flush it to disk.
fn fill_temp_file(mut file: std::fs::File, contents: &[u8]) -> Result<()> {
    file.write_all(contents)
        .context("Failed to write temporary config file")?;
    file.sync_all()
        .context("Failed to flush temporary config file")?;
    Ok(())
}

/// Delete abandoned temp files left beside `config.toml` by an interrupted
/// [`write_atomically`].
///
/// Best-effort by design: every failure here (unreadable directory, a file
/// deleted by another sweeper mid-loop, a permission error) is ignored, because
/// failing to collect a stale temp file must never stop the config from loading.
pub(super) fn sweep_stale_temp_files(dir: &Path) {
    sweep_stale_temp_files_named(dir, "config.toml.");
}

pub(super) fn sweep_stale_temp_files_named(dir: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // Matches what `write_atomically` produces, and nothing else: the live
        // `config.toml` has no `.tmp` suffix, and a project's own `.stalx.tmp`
        // does not start with `config.toml.`. Deliberately a prefix/suffix test
        // rather than a parse of the middle, so it keeps matching as the unique
        // part of the name changes shape (it has already gained a random nonce).
        if !name.starts_with(prefix) || !name.ends_with(".tmp") {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= STALE_TEMP_AGE);
        if stale {
            let _ = std::fs::remove_file(entry.path());
            tracing::debug!(temp_file = ?entry.path(), "Swept stale config temp file");
        }
    }
}
