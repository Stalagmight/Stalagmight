use super::AppState;
use crate::core::CommandResult;
use std::path::{Path, PathBuf};

use super::open_file::{FileOrigin, FileState, EXTERNALLY_MODIFIED_CODE, READ_ONLY_FILE_CODE};
use crate::models::survey::Project;
use crate::persistence::stalx_v2::{write_atomic, Guard, Mode, WriteError};

/// What a failed save actually was, once the error chain and the destination
/// have both been consulted. Each variant except `Other` carries a marker the
/// frontend keys on to pick the remediation it shows.
#[derive(Clone, Copy)]
enum SaveFailureKind {
    /// The OS refuses a destination that IS there (macOS forgetting a granted
    /// folder, a read-only mount).
    PermissionDenied,
    /// The destination is not reachable: the folder was deleted or renamed, or
    /// its drive is not connected.
    PathNotFound,
    /// The destination path is longer than the OS will open.
    PathTooLong,
    /// Anything else - disk full, corruption, a SQLite error of its own. The
    /// backend message is the best thing to show.
    Other,
}

impl SaveFailureKind {
    /// The marker prefixed onto the `CommandResult` error, or `None` when the
    /// message stands on its own.
    fn code(self) -> Option<&'static str> {
        match self {
            Self::PermissionDenied => Some(crate::core::loading::PERMISSION_DENIED_CODE),
            Self::PathNotFound => Some(crate::core::loading::PATH_NOT_FOUND_CODE),
            Self::PathTooLong => Some(crate::core::loading::PATH_TOO_LONG_CODE),
            Self::Other => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::PermissionDenied => "permission_denied",
            Self::PathNotFound => "path_not_found",
            Self::PathTooLong => "path_too_long",
            Self::Other => "other",
        }
    }
}

/// What the destination directory has to say for itself.
enum DirProbe {
    /// It accepted a file, so it is denying us nothing.
    Writable,
    /// It refused - the corroboration a permission denial needs.
    Denied,
    /// It is not there, or cannot be reached at all.
    Unreachable,
    /// It is there and the probe failed for some other reason (a full disk).
    Other,
}

/// Create the probe file, refusing to write through anything already at that
/// name.
///
/// `create_new` is what makes the probe safe to run in a directory other
/// people can write to: a plain `File::create` FOLLOWS a symlink planted at the
/// name and truncates whatever it points at, and the name is guessable (it
/// carries our pid). `create_new` fails with `AlreadyExists` instead - not a
/// permission denial and not a missing directory, so the caller reads it as the
/// non-committal outcome, which is the honest answer: something else owns that
/// name and this probe learned nothing about the directory.
pub(super) fn create_write_probe(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Ask the destination directory directly, because the error chain would not
/// say: create a file in it and delete it again.
///
/// Only ever called on a save that has ALREADY failed, so the transient probe
/// file costs nothing on the happy path.
fn probe_destination_dir(dir: &Path) -> DirProbe {
    match dir.try_exists() {
        Ok(true) => {}
        Ok(false) => return DirProbe::Unreachable,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return DirProbe::Denied,
        // A directory whose metadata cannot be read for any other reason (a
        // removed stick, a dropped share, a name the OS will not resolve) is
        // not somewhere a file can be written either.
        Err(_) => return DirProbe::Unreachable,
    }
    let probe = dir.join(format!(
        ".stalagmight-write-probe-{}-{}",
        std::process::id(),
        PROBE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    match create_write_probe(&probe) {
        Ok(file) => {
            drop(file);
            if let Err(e) = std::fs::remove_file(&probe) {
                tracing::debug!(
                    path = ?probe,
                    error = %e,
                    "Could not remove the write probe; it is left behind"
                );
            }
            DirProbe::Writable
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => DirProbe::Denied,
        Err(_) => DirProbe::Other,
    }
}

/// Distinguishes concurrent probes in the same process.
static PROBE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Classify a failed save of `target`.
///
/// Three signals, in order:
///
/// 1. the error chain itself, when it is explicit (a typed `PermissionDenied`
///    or `NotFound`, or a message that names one);
/// 2. the destination being longer than SQLite's win32 VFS can open - which its
///    parent directory stat-ing perfectly fine does NOT rule out, because Rust's
///    own std calls go through `\\?\` verbatim paths and SQLite's do not;
/// 3. SQLite's ambiguous `SQLITE_CANTOPEN` message, resolved by probing the
///    destination directory. A directory that accepts a file is denying us
///    nothing, so ONLY a probe that is itself refused makes this a permission
///    denial - the corroboration that stops an over-`MAX_PATH` Windows path
///    being answered with macOS Privacy & Security steps (#429).
fn classify_save_failure(target: &Path, err: &anyhow::Error) -> SaveFailureKind {
    if crate::core::loading::is_permission_denied(err) {
        return SaveFailureKind::PermissionDenied;
    }
    if crate::core::loading::is_path_not_found(err) {
        return SaveFailureKind::PathNotFound;
    }
    if !crate::core::loading::mentions_unopenable_database(err) {
        return SaveFailureKind::Other;
    }
    // The temp file the pipeline opens FIRST is `<name>.stalx.tmp`, four
    // characters longer than the destination, so measure that one.
    if cfg!(windows)
        && crate::core::loading::over_legacy_path_limit(&target.with_extension("stalx.tmp"))
    {
        return SaveFailureKind::PathTooLong;
    }
    let Some(dir) = target.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return SaveFailureKind::Other;
    };
    match probe_destination_dir(dir) {
        DirProbe::Denied => SaveFailureKind::PermissionDenied,
        DirProbe::Unreachable => SaveFailureKind::PathNotFound,
        DirProbe::Writable | DirProbe::Other => SaveFailureKind::Other,
    }
}

/// Build a save-failure `CommandResult`, tagging it with the marker that tells
/// the frontend which remediation to show, and logging the full error chain -
/// the one place every failing save passes through, so a failure that only ever
/// happens on a user's machine is still diagnosable from their log (#429).
pub(super) fn save_failure(context: &str, target: &Path, err: &anyhow::Error) -> CommandResult {
    let detail = format!("{context}: {err}");
    let kind = classify_save_failure(target, err);
    tracing::warn!(
        path = ?target,
        stage = context,
        classification = kind.as_str(),
        chain = %crate::core::loading::error_chain(err),
        purpose = "persist edits safely",
        "Save failed"
    );
    match kind.code() {
        Some(code) => CommandResult::failure(format!("{code}: {detail}")),
        None => CommandResult::failure(detail),
    }
}

/// How the save treats a destination that is not what it last read or wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnDisk {
    /// The guard (plan 6.5): refuse with [`EXTERNALLY_MODIFIED_CODE`].
    Checked,
    /// Replace it (the user chose Overwrite after the guard refused).
    Overwrite,
}

/// The generator a save writes (`generator`), the app's version when the
/// host set it ([`set_generator`]).
static GENERATOR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Set the `generator` string every save writes (`Stalagmight <version>`);
/// called once by the app at startup.
pub fn set_generator(version: &str) {
    let _ = GENERATOR.set(format!("Stalagmight {version}"));
}

pub(super) fn generator() -> String {
    GENERATOR
        .get()
        .cloned()
        .unwrap_or_else(|| crate::persistence::stalx_v2::DEFAULT_GENERATOR.to_string())
}

/// The app-data folder the pre-upgrade originals go to (plan 7.2.10).
pub(super) fn upgrade_backup_dir() -> Option<PathBuf> {
    Some(crate::app_dirs::data_local_dir()?.join("backups"))
}

/// The files a legacy original was moved or copied into before its first
/// version 2 write, so a failed write can put the sidecars back.
struct UpgradeBackup {
    /// `(sidecar at the project, where it went)`.
    moved: Vec<(PathBuf, PathBuf)>,
}

impl UpgradeBackup {
    /// Copy the SQLite original into the backups folder and MOVE its
    /// `-journal` / `-wal` / `-shm` sidecars there (R2 L19: a stale journal
    /// beside the JSON file could be rolled into it by an old build).
    fn take(original: &Path) -> Result<Self, String> {
        let dir = upgrade_backup_dir()
            .ok_or_else(|| "the app data folder is not available".to_string())?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        let stem = original
            .file_stem()
            .map_or_else(|| "project".into(), |s| s.to_string_lossy().into_owned());
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ");
        let base = dir.join(format!("{stem}.{stamp}.pre-v2.stalx"));
        std::fs::copy(original, &base)
            .map_err(|e| format!("could not back up {}: {e}", original.display()))?;
        let mut moved = Vec::new();
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut name = original.as_os_str().to_os_string();
            name.push(suffix);
            let sidecar = PathBuf::from(name);
            if sidecar.exists() {
                let mut to = base.as_os_str().to_os_string();
                to.push(suffix);
                let to = PathBuf::from(to);
                std::fs::rename(&sidecar, &to)
                    .map_err(|e| format!("could not move {}: {e}", sidecar.display()))?;
                moved.push((sidecar, to));
            }
        }
        tracing::info!(
            backup = ?base,
            sidecars = moved.len(),
            purpose = "keep the original of a legacy file the first save upgrades",
            "Backed up the legacy original before writing version 2"
        );
        Ok(Self { moved })
    }

    /// The write failed: put the sidecars back beside the untouched original.
    fn put_back(self) {
        for (sidecar, to) in self.moved {
            if let Err(e) = std::fs::rename(&to, &sidecar) {
                tracing::error!(error = %e, sidecar = ?sidecar, "could not put a legacy sidecar back");
            }
        }
    }
}

impl AppState {
    /// [#92] Whether the open project's initial save is still deferred.
    pub fn initial_save_deferred(&self) -> bool {
        self.initial_save_deferred
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(super) fn save_project(&self, path: Option<PathBuf>) -> CommandResult {
        self.save_project_at_generation(path, None)
    }

    /// Save, refusing if the SpeleoDB session changed since `expected_generation`.
    pub(super) fn save_project_at_generation(
        &self,
        path: Option<PathBuf>,
        expected_generation: Option<u64>,
    ) -> CommandResult {
        self.save_with(path, expected_generation, OnDisk::Checked)
    }

    /// Save over a file another program changed since it was opened (the
    /// guard's "Overwrite" answer).
    pub fn save_project_overwriting(&self, path: Option<PathBuf>) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        self.save_with(path, None, OnDisk::Overwrite)
    }

    /// The save (plan 6.5): validate, encode in memory, write atomically with
    /// the external-modification guard. `path` `None` saves to the open
    /// file; an explicit `path` on a SpeleoDB-managed project writes a
    /// detached copy (no link, no SpeleoDB state, no inherited history) and
    /// leaves the session on the managed file. The first save of a legacy
    /// file upgrades it, after backing up the original.
    fn save_with(
        &self,
        path: Option<PathBuf>,
        expected_generation: Option<u64>,
        on_disk: OnDisk,
    ) -> CommandResult {
        let mut proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if expected_generation
            .is_some_and(|generation| self.speleodb_session_generation() != generation)
        {
            return CommandResult::failure("The project session changed before saving".to_string());
        }
        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };
        let open_path = project.file_path.clone();
        let Some(target) = path.clone().or_else(|| open_path.clone()) else {
            return CommandResult::failure("The project has no file to save to".to_string());
        };
        let to_open_file = open_path.as_ref().is_some_and(|p| same_file(p, &target));
        let violations = project.validate();
        if let Some(first) = violations.first() {
            tracing::error!(
                count = violations.len(),
                purpose = "never write an invalid survey",
                "Refused to save a project that breaks an invariant"
            );
            return CommandResult::failure(format!("Can't save: {first}"));
        }
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if file.extras.read_only.is_some() {
            return CommandResult::failure(format!(
                "{READ_ONLY_FILE_CODE}: this project was made with a newer Stalagmight; \
                 it is open read-only and cannot be saved"
            ));
        }
        let detached = file.extras.speleodb.is_some() && path.is_some() && !to_open_file;
        let mut extras = file.extras.clone();
        extras.generator = generator();
        extras.history = self
            .undo_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .persisted();
        let mode = if detached {
            Mode::DetachedCopy
        } else {
            Mode::Normal
        };
        let bytes = match crate::persistence::stalx_v2::encode(project, &extras, mode) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::error!(error = %e, "Encoding the project failed");
                return CommandResult::failure(format!("Can't save: {e}"));
            }
        };
        let guard = match (on_disk, to_open_file, file.fingerprint.as_ref()) {
            (OnDisk::Overwrite, _, _) => Guard::Overwrite,
            (OnDisk::Checked, true, Some(fp)) => Guard::Expect(fp),
            // The first write of a project created in this session: its path
            // was chosen free; something appearing there meanwhile is not ours.
            (OnDisk::Checked, true, None) => Guard::Absent,
            // An explicit destination the save dialog confirmed.
            (OnDisk::Checked, false, _) => Guard::Overwrite,
        };
        let upgrade = !detached && to_open_file && file.origin == FileOrigin::Legacy;
        let backup = if upgrade {
            match UpgradeBackup::take(&target) {
                Ok(backup) => Some(backup),
                Err(e) => {
                    tracing::error!(error = %e, "Could not back up the legacy original");
                    return CommandResult::failure(format!(
                        "Can't save: the original could not be backed up before the upgrade ({e})"
                    ));
                }
            }
        } else {
            None
        };
        let fingerprint = match write_atomic(&target, &bytes, guard) {
            Ok(fp) => fp,
            Err(e) => {
                if let Some(backup) = backup {
                    backup.put_back();
                }
                return match e {
                    WriteError::ExternallyModified { .. } | WriteError::Exists { .. } => {
                        tracing::warn!(error = %e, "Save refused: the file changed on disk");
                        CommandResult::failure(format!("{EXTERNALLY_MODIFIED_CODE}: {e}"))
                    }
                    WriteError::Io { error, .. } => {
                        save_failure("Failed to save", &target, &anyhow::Error::from(error))
                    }
                };
            }
        };
        if detached {
            drop(file);
            drop(proj_guard);
            self.note_recent_project(&target);
            tracing::info!(path = ?target, "Saved an unlinked copy of a SpeleoDB-managed project");
            return CommandResult::success().with_unlinked_copy_path(target.display().to_string());
        }
        project.file_path = Some(target.clone());
        file.fingerprint = Some(fingerprint);
        file.origin = FileOrigin::V2;
        file.dirty = false;
        file.local_pending = false;
        file.saved_history = extras.history;
        drop(file);
        drop(proj_guard);
        self.initial_save_deferred
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.note_recent_project(&target);
        tracing::info!(
            path = ?target,
            bytes = bytes.len(),
            upgraded = upgrade,
            purpose = "persist project",
            "Saved project"
        );
        let mut result = CommandResult::success();
        result.saved_path = Some(target.display().to_string());
        result
    }

    /// Put `path` at the head of the recent-projects list and persist it to
    /// config. Takes no state locks other than `recent_projects` + `config`;
    /// callers must NOT hold `project` (the save paths drop it first).
    pub(super) fn note_recent_project(&self, path: &Path) {
        if let Ok(mut recent) = self.recent_projects.lock() {
            recent.retain(|p| p != path);
            recent.insert(0, path.to_path_buf());
            if let Ok(mut config) = self.config.lock() {
                recent.truncate(config.general.recent_projects_max);
                config.general.recent_projects.clone_from(&recent);
                if let Err(e) = config.save() {
                    tracing::warn!(
                        error = %e,
                        purpose = "persist recent projects list",
                        "Failed to save config with recent projects"
                    );
                }
            }
        }
    }

    pub(super) fn close_project(&self) -> CommandResult {
        let mut proj = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.close_open_project_locked(&mut proj)
    }

    /// Drop the open project and everything that describes it. The caller
    /// holds the `project` guard (passed in). Unsaved changes are the
    /// caller's to have asked about.
    pub(super) fn close_open_project_locked(&self, project: &mut Option<Project>) -> CommandResult {
        *project = None;
        // A session boundary: the caller holds the `project` guard, which is the
        // bump rule (`speleodb_session`).
        self.bump_speleodb_session();
        self.bump_revision();
        self.set_history_discarded(false);
        *self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = FileState::default();
        {
            let mut undo_stack = self
                .undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            undo_stack.clear();
            undo_stack.set_inherited_through(None);
            // [#303] Back onto the configured entry window.
            undo_stack.use_count_limit();
        }
        *self
            .derived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.speleodb_set_open_gate(None);
        tracing::info!(purpose = "release project resources", "Project closed");
        CommandResult::success()
    }

    pub(super) fn open_project_path(&self) -> Option<PathBuf> {
        self.open_file_path()
    }
}

pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}
