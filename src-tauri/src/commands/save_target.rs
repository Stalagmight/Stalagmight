//! [#576 HFI-2] Whether a file is already at the path a save or an export is
//! about to write.
//!
//! The overwrite prompts ask before a file is replaced: Save As, and a save or
//! export whose name the app rewrote after the OS dialog answered
//! (`resolveSavePath`, src/utils/overwriteGuard.ts). The webview used to ask the
//! fs plugin's `exists()`, whose capability scope holds no user path, so in the
//! real app it threw "forbidden path" for every path - and since a failed check
//! asks (#576 E10-D1, fail closed), every Save As said "could not check whether
//! ... already exists", even for a brand-new file. The backend answers instead,
//! and the webview gets no fs permission for it.
//!
//! The probe runs the write-path policy of the write it precedes
//! (`write_path::check_write_path`): the name must have the extension of a file
//! the app writes, and must not be inside the app's own folders or a system
//! folder. So it tells a page script nothing about a place the app would not
//! write anyway. A failed check is an error, which the frontend treats as "the
//! file may exist" and asks (E10-D1). A refused path carries its own code,
//! `WRITE_PATH_REFUSED` (#576 HFI-2-D2): the write after it refuses the same path
//! with the reason, so the frontend goes straight to it - one dialog, the
//! refusal - instead of first asking "could not check".

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::commands::pool;
use crate::commands::write_path::{check_write_path_with, AppDirs, WriteTarget};
use crate::core::{CommandError, CommandErrorCode};

/// Whether a file (or anything else - a folder, a link) is at `path`, which a
/// save or export is about to write. `Err` when the check itself failed (a
/// folder that may not be searched): the caller asks before writing, as for a
/// file that is there. `Err` with `WRITE_PATH_REFUSED` when the write-path policy
/// refuses the path: the write after it refuses it too.
#[tauri::command]
pub async fn save_target_exists(path: PathBuf) -> Result<bool, CommandError> {
    pool::run_stateless(move || save_target_exists_with(&path, &AppDirs::current())).await
}

/// [`save_target_exists`] against given app directories (the testable core).
pub(crate) fn save_target_exists_with(path: &Path, dirs: &AppDirs) -> Result<bool, CommandError> {
    let name = file_name(path);
    let target = WriteTarget::of_path(path).ok_or_else(|| {
        tracing::warn!(
            path = ?path,
            purpose = "check whether a save would replace a file (#576 HFI-2)",
            "Refused an existence check for a file type the app does not write"
        );
        CommandError::new(
            CommandErrorCode::WritePathRefused,
            format!("Refused to check {name}: Stalagmight writes no file of that type"),
        )
    })?;
    check_write_path_with(path, target, dirs)
        .map_err(|why| CommandError::new(CommandErrorCode::WritePathRefused, why))?;

    // `symlink_metadata`, not `exists`: a link at the name (even a dangling one)
    // is something the write would replace, and `exists` reads a check that
    // failed for any reason as "not there".
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => {
            tracing::warn!(
                path = ?path,
                error = %e,
                purpose = "check whether a save would replace a file (#576 HFI-2)",
                "Existence check failed"
            );
            // The OS refusing to let us look is `FS_PERMISSION_DENIED`; anything
            // else `FAILED`, the message saying what. (A missing file or folder is
            // no failure: it is "not there".)
            let code = if e.kind() == ErrorKind::PermissionDenied {
                CommandErrorCode::FsPermissionDenied
            } else {
                CommandErrorCode::Failed
            };
            Err(CommandError::new(
                code,
                format!("Could not check whether {name} already exists: {e}"),
            ))
        }
    }
}

/// `path`'s file name, for a message (the whole path when it has none).
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
