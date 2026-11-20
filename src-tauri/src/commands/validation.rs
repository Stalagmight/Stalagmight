use chrono::Utc;
use tauri::State;

use crate::core::{AppState, IntegrityCheckResult, IntegrityRepairResult};
use crate::models::{warning_is_ignorable, LineId, ShotId};
use crate::persistence::ResolvedWarning;

// Re-export ProjectWarning from models for backward compatibility
pub use crate::models::warning::ProjectWarning;

/// Get all validation warnings for the current project
#[tauri::command]
pub fn get_project_warnings(state: State<AppState>) -> Vec<ProjectWarning> {
    get_project_warnings_impl(state.inner())
}

/// Structural-integrity violation messages for the open project (critical
/// first). Empty when the project is sound. The frontend uses a non-empty
/// result after load to warn the user and offer a repair.
#[tauri::command]
pub fn check_project_integrity(state: State<AppState>) -> IntegrityCheckResult {
    state.integrity_check()
}

/// Repair the open project's structural integrity, returning `{ fixed,
/// remaining }`. The project is left dirty so the user can save the repair.
/// `remaining > 0` means some issues are detect-only (drifted aligned loop
/// closures, non-finite geometry) and need manual attention.
#[tauri::command]
pub fn repair_project_integrity(state: State<AppState>) -> IntegrityRepairResult {
    state.repair_open_project_integrity()
}

pub(crate) fn get_project_warnings_impl(state: &AppState) -> Vec<ProjectWarning> {
    // Computed under the project lock from the memoised layout: no copy of
    // the project. Empty when none is open.
    let warnings = state.open_project_warnings();
    tracing::debug!(
        count = warnings.len(),
        purpose = "surface validation issues",
        "Computed project validation warnings"
    );
    warnings
}

/// Resolve a warning (mark it as acknowledged)
#[tauri::command]
pub fn resolve_warning(
    state: State<AppState>,
    line_id: LineId,
    shot_id: ShotId,
    warning_code: String,
    comment: Option<String>,
) -> Result<(), String> {
    resolve_warning_impl(state.inner(), line_id, shot_id, warning_code, comment)
}

pub(crate) fn resolve_warning_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
    warning_code: String,
    comment: Option<String>,
) -> Result<(), String> {
    // Errors are not ignorable (issue #57): refuse to persist a suppression for
    // any error-severity warning (over-anchoring, invalid pitch/azimuth, zero/
    // negative length, length-below-depth, negative LRUD). One predicate -
    // `warning_is_ignorable` - is the single policy source shared with the
    // warnings computation. Returning Ok keeps callers simple, and
    // compute_project_warnings never honors such a row regardless.
    if !warning_is_ignorable(&warning_code) {
        tracing::debug!(
            warning_code = %warning_code,
            purpose = "reject ignore of a non-ignorable (error) warning",
            "Ignoring of error-severity warnings is not allowed; not persisting"
        );
        return Ok(());
    }

    // Policy (issue #57): an ignore is keyed by the shot's uuid. shot_uuid_for
    // returns None only when no project is open, the lock is poisoned, or the
    // (line_id, shot_id) shot does not exist: there is no shot to bind, so refuse.
    let Some(shot_uuid) = state.shot_uuid_for(line_id, shot_id) else {
        tracing::warn!(
            line_id,
            shot_id,
            warning_code = %warning_code,
            purpose = "reject ignore without stable shot identity",
            "Refusing to resolve warning: no shot UUID"
        );
        return Err("Could not ignore this warning: the shot no longer exists.".to_string());
    };
    tracing::info!(
        line_id = line_id,
        shot_id = shot_id,
        warning_code = %warning_code,
        purpose = "acknowledge validation warning",
        "Resolving warning"
    );
    // Every shot's uuid is written with the survey, and the ignore rides the
    // same save, so the ignore is always keyed by it.
    let warning = ResolvedWarning {
        line_id,
        shot_id,
        warning_code,
        resolved_at: Utc::now(),
        resolved_comment: comment,
        shot_uuid: Some(shot_uuid),
    };
    state.save_resolved_warning_gated(&warning).map_err(|e| {
        // A read-only rejection (EDIT_DISABLED) is expected control flow on a remote
        // project opened without the lock, not a persistence failure worth an error log.
        if !e.starts_with(crate::core::command::EDIT_DISABLED_CODE) {
            tracing::error!(
                line_id = warning.line_id,
                shot_id = warning.shot_id,
                warning_code = %warning.warning_code,
                purpose = "acknowledge validation warning",
                reason = %e,
                "Failed to resolve warning"
            );
        }
        e
    })
}

/// Unresolve a warning (remove acknowledgment)
#[tauri::command]
pub fn unresolve_warning(
    state: State<AppState>,
    line_id: LineId,
    shot_id: ShotId,
    warning_code: String,
) -> Result<(), String> {
    unresolve_warning_impl(state.inner(), line_id, shot_id, warning_code)
}

pub(crate) fn unresolve_warning_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
    warning_code: String,
) -> Result<(), String> {
    let shot_uuid = state.shot_uuid_for(line_id, shot_id);
    tracing::info!(
        line_id = line_id,
        shot_id = shot_id,
        warning_code = %warning_code,
        purpose = "reopen validation warning",
        "Unresolving warning"
    );
    state
        .delete_resolved_warning_gated(line_id, shot_id, &warning_code, shot_uuid.as_deref())
        .map_err(|e| {
            // A read-only rejection (EDIT_DISABLED) is expected control flow, not a
            // persistence failure worth an error log.
            if !e.starts_with(crate::core::command::EDIT_DISABLED_CODE) {
                tracing::error!(
                    line_id = line_id,
                    shot_id = shot_id,
                    warning_code = %warning_code,
                    purpose = "reopen validation warning",
                    reason = %e,
                    "Failed to unresolve warning"
                );
            }
            e
        })
}
