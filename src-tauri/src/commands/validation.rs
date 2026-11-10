use chrono::Utc;
use tauri::State;

use crate::core::{AppState, IntegrityCheckResult, IntegrityRepairResult};
use crate::models::{warning_is_ignorable, SectionId, ShotId};
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
    let Some(project) = state.get_project_state() else {
        return Vec::new();
    };

    // Delegate to AppState method (uses self.database since project is already loaded)
    let warnings = state.compute_project_warnings(&project, None);
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
    section_id: SectionId,
    shot_id: ShotId,
    warning_code: String,
    comment: Option<String>,
) -> Result<(), String> {
    resolve_warning_impl(state.inner(), section_id, shot_id, warning_code, comment)
}

pub(crate) fn resolve_warning_impl(
    state: &AppState,
    section_id: SectionId,
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

    // Policy (issue #57): UUID-keyed suppression is the only supported model for
    // new ignores. shot_uuid_for returns None only when no project is open, the
    // lock is poisoned, or the (section_id, shot_id) shot does not exist - in
    // every case we cannot bind a stable identity, so refuse rather than write a
    // position-keyed row that could later suppress an unrelated shot reusing the id.
    let Some(shot_uuid) = state.shot_uuid_for(section_id, shot_id) else {
        tracing::warn!(
            section_id,
            shot_id,
            warning_code = %warning_code,
            purpose = "reject ignore without stable shot identity",
            "Refusing to resolve warning: no shot UUID"
        );
        return Err("Could not ignore this warning: the shot no longer exists.".to_string());
    };
    tracing::info!(
        section_id = section_id,
        shot_id = shot_id,
        warning_code = %warning_code,
        purpose = "acknowledge validation warning",
        "Resolving warning"
    );
    // Key by UUID only when that UUID is already persisted on disk. A pre-v6
    // shot's UUID is minted in memory and not written until the project is saved;
    // keying the ignore by it would orphan the row on the next open (different
    // UUIDs are minted), losing the ignore for a legacy file closed without saving.
    // Fall back to a legacy position-keyed ignore for such a shot - it survives via
    // the position fallback and the load-time backfill upgrades it once the UUID is
    // persisted (issue #57 / Codex).
    // Section-scoped: a FILE saved before shot UUIDs were project-wide unique can hold one
    // id in two sections (the load renames one copy), so an unscoped check would call a UUID
    // persisted only in ANOTHER section durable and key the ignore by it anyway. A renamed
    // shot's derived id counts as persisted: every open derives it again.
    let shot_uuid = if state.shot_uuid_is_persisted(section_id, &shot_uuid) {
        Some(shot_uuid)
    } else {
        None
    };
    let warning = ResolvedWarning {
        section_id,
        shot_id,
        warning_code,
        resolved_at: Utc::now(),
        resolved_comment: comment,
        shot_uuid,
    };
    state.save_resolved_warning_gated(&warning).map_err(|e| {
        // A read-only rejection (EDIT_DISABLED) is expected control flow on a remote
        // project opened without the lock, not a persistence failure worth an error log.
        if !e.starts_with(crate::core::command::EDIT_DISABLED_CODE) {
            tracing::error!(
                section_id = warning.section_id,
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
    section_id: SectionId,
    shot_id: ShotId,
    warning_code: String,
) -> Result<(), String> {
    unresolve_warning_impl(state.inner(), section_id, shot_id, warning_code)
}

pub(crate) fn unresolve_warning_impl(
    state: &AppState,
    section_id: SectionId,
    shot_id: ShotId,
    warning_code: String,
) -> Result<(), String> {
    let shot_uuid = state.shot_uuid_for(section_id, shot_id);
    tracing::info!(
        section_id = section_id,
        shot_id = shot_id,
        warning_code = %warning_code,
        purpose = "reopen validation warning",
        "Unresolving warning"
    );
    state
        .delete_resolved_warning_gated(section_id, shot_id, &warning_code, shot_uuid.as_deref())
        .map_err(|e| {
            // A read-only rejection (EDIT_DISABLED) is expected control flow, not a
            // persistence failure worth an error log.
            if !e.starts_with(crate::core::command::EDIT_DISABLED_CODE) {
                tracing::error!(
                    section_id = section_id,
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
