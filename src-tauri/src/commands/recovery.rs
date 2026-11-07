use std::path::{Path, PathBuf};

use crate::commands::pool;
use crate::commands::save_target::file_name;
use crate::commands::write_path::{check_write_path, check_write_path_with, AppDirs, WriteTarget};
use crate::core::CommandError;
use crate::persistence::{BackupInfo, Database, RepairResult, SalvageWriteResult, SalvagedData};

/// Attempt to repair a corrupt database file.
///
/// Creates a numbered backup before attempting repair via REINDEX + VACUUM.
/// Returns the backup path and whether the repair restored integrity.
#[tauri::command]
pub async fn attempt_repair(path: PathBuf) -> Result<RepairResult, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(path = ?path, "IPC: attempt_repair");
        // [#576 C11-D1] The repair rewrites the file in place.
        check_write_path(&path, WriteTarget::Project)?;

        match Database::attempt_repair(&path) {
            Ok((backup_path, integrity_ok)) => {
                let message = if integrity_ok {
                    "Repair succeeded - database integrity restored.".to_string()
                } else {
                    "Repair completed but integrity check still fails. \
                     Try exporting salvageable data instead."
                        .to_string()
                };
                Ok(RepairResult {
                    success: integrity_ok,
                    backup_path: backup_path.display().to_string(),
                    message,
                })
            }
            Err(e) => {
                tracing::error!(error = %e, "Repair failed");
                Err(CommandError::failed(format!("Repair failed: {e}")))
            }
        }
    })
    .await
}

/// List the files a project can be restored from, labelled and checked.
///
/// Backups (`.backup`, `.backup.N`, `.pre-v<N>.backup`), an interrupted save's
/// `.tmp`, and the files earlier restores replaced (`.pre-restore.N`); each says
/// what it is and whether it can be restored (#576 C15).
#[tauri::command]
pub async fn list_backups(path: PathBuf) -> Result<Vec<BackupInfo>, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(path = ?path, "IPC: list_backups");
        Ok(Database::list_backups(&path))
    })
    .await
}

/// Restore a project from a backup file.
///
/// The candidate is checked on a staged copy first (refused, with the project
/// untouched, when it is not a usable project); the file it replaces is kept as
/// a numbered `.pre-restore.N` that never overwrites an earlier one; the staged
/// copy is renamed into place atomically (#576 C15). See
/// `Database::restore_from_backup`.
#[tauri::command]
pub async fn restore_from_backup(
    project_path: PathBuf,
    backup_path: PathBuf,
) -> Result<String, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(project = ?project_path, backup = ?backup_path, "IPC: restore_from_backup");
        // [#576 C11-D1] The restore replaces the project file.
        check_write_path(&project_path, WriteTarget::Project)?;

        let outcome = Database::restore_from_backup(&project_path, &backup_path).map_err(|e| {
            tracing::error!(error = %e, "Restore failed");
            format!("{e:#}")
        })?;
        let name = |path: &Path| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        let message = match outcome.safety_copy {
            Some(copy) => format!(
                "Restored from {}. The file it replaced is kept as {}, and is offered in this \
                 list too.",
                name(&backup_path),
                name(&copy)
            ),
            None => format!("Restored from {}.", name(&backup_path)),
        };
        Ok(message)
    })
    .await
}

/// Export whatever data can be salvaged from a corrupt database.
///
/// Reads each table independently, collecting recoverable data and errors.
#[tauri::command]
pub async fn export_salvageable_data(path: PathBuf) -> Result<SalvagedData, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(path = ?path, "IPC: export_salvageable_data");

        Database::export_salvageable(&path)
            .map_err(|e| CommandError::failed(format!("Salvage failed: {e}")))
    })
    .await
}

/// [#576 HFI-3] Write what can be salvaged from the corrupt project at `path`
/// to `dest_path`: the JSON file the recovery dialog's "Export Salvageable Data"
/// saves (the readable project as a whole, else its sections, else `{}`).
///
/// The dialog used to write that file itself, with the fs plugin's
/// `writeTextFile`, whose capability scope holds no user path: in the real app
/// every salvage export failed ("forbidden path") and nothing was written. The
/// backend writes it now, behind the write-path policy (a `.json` file, not
/// inside the app's own folders or a system folder), and salvages `path` again
/// itself rather than writing text the webview hands it - so the page gains no
/// general file write, only this one, exactly as an export is. The destination
/// is replaced atomically, as every export's is (`export::write_atomic_bytes`);
/// the damaged project is only read, and naming it as the destination is
/// refused.
///
/// [#576 W30-5] It answers the counts and warnings of the salvage it wrote, for
/// the dialog to report: those of the dialog's own earlier read need not be
/// what this second read found, since a damaged file can read differently twice.
#[tauri::command]
pub async fn write_salvaged_data(
    path: PathBuf,
    dest_path: PathBuf,
) -> Result<SalvageWriteResult, CommandError> {
    pool::run_stateless(move || write_salvaged_data_with(&path, &dest_path, &AppDirs::current()))
        .await
}

/// [`write_salvaged_data`] against given app directories (the testable core).
pub(crate) fn write_salvaged_data_with(
    path: &Path,
    dest_path: &Path,
    dirs: &AppDirs,
) -> Result<SalvageWriteResult, CommandError> {
    tracing::info!(path = ?path, dest = ?dest_path, "IPC: write_salvaged_data");
    check_write_path_with(dest_path, WriteTarget::Json, dirs)?;
    let name = file_name(dest_path);
    let same_file = matches!(
        (std::fs::canonicalize(path), std::fs::canonicalize(dest_path)),
        (Ok(source), Ok(dest)) if source == dest
    );
    if same_file {
        return Err(CommandError::failed(format!(
            "Refused to write the salvaged data to {name}: that is the damaged project itself"
        )));
    }

    let salvaged = Database::export_salvageable(path)
        .map_err(|e| CommandError::failed(format!("Salvage failed: {e}")))?;
    let written = SalvageWriteResult {
        section_count: salvaged.section_count,
        shot_count: salvaged.shot_count,
        merge_count: salvaged.merge_count,
        errors: salvaged.errors,
    };
    let content = salvaged
        .full_project_json
        .or(salvaged.sections_json)
        .unwrap_or_else(|| "{}".to_owned());
    // `FAILED` with the reason, as every export's write failure: the atomic
    // writer reports the OS error as text, so there is no kind to classify.
    crate::export::write_atomic_bytes(dest_path, content.as_bytes()).map_err(|e| {
        tracing::error!(dest = ?dest_path, error = %e, "Writing the salvaged data failed");
        CommandError::failed(format!("Could not write {name}: {e:#}"))
    })?;
    Ok(written)
}
