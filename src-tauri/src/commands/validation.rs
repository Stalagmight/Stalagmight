use chrono::Utc;
use tauri::{AppHandle, Runtime};

use crate::commands::pool;
use crate::core::{
    AppState, CommandError, EditOutcome, IntegrityCheckResult, IntegrityRepairResult,
};
use crate::models::{warning_is_ignorable, SectionId, ShotId};
use crate::persistence::ResolvedWarning;

// Re-export ProjectWarning from models for backward compatibility
pub use crate::models::warning::ProjectWarning;

/// Get all validation warnings for the current project
#[tauri::command]
pub async fn get_project_warnings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<ProjectWarning>, CommandError> {
    // One settled snapshot (#576 C2): the warnings are computed on it with no lock held.
    pool::run(app, |state| {
        let Some(project) = state.settled_snapshot() else {
            return Ok(Vec::new());
        };

        // Delegate to AppState method (uses self.database since project is already loaded)
        let warnings = state.compute_project_warnings(&project, None);
        tracing::debug!(
            count = warnings.len(),
            purpose = "surface validation issues",
            "Computed project validation warnings"
        );
        Ok(warnings)
    })
    .await
}

/// Structural-integrity violation messages for the open project (critical
/// first). Empty when the project is sound. The frontend uses a non-empty
/// result after load to warn the user and offer a repair.
#[tauri::command]
pub async fn check_project_integrity<R: Runtime>(
    app: AppHandle<R>,
) -> Result<IntegrityCheckResult, CommandError> {
    pool::run(app, |state| {
        Ok(state
            .settled_snapshot()
            .map_or_else(IntegrityCheckResult::default, |project| {
                AppState::integrity_check_of(&project)
            }))
    })
    .await
}

/// Repair the open project's structural integrity, returning `{ fixed,
/// remaining }`. The project is left dirty so the user can save the repair.
/// `remaining > 0` means some issues are detect-only (drifted aligned loop
/// closures, non-finite geometry) and need manual attention. Rejects with the
/// edit gate's refusal (an edit-gate code, #576 C4) when editing is blocked, rather
/// than resolving with a zero result that reads as a clean repair (#576 D39(a)).
#[tauri::command]
pub async fn repair_project_integrity<R: Runtime>(
    app: AppHandle<R>,
) -> Result<IntegrityRepairResult, CommandError> {
    pool::run_mutation(app, AppState::repair_open_project_integrity).await
}

/// An ignore-list edit's answer: it reports no validation warnings of its own
/// (`remote_link` is filled by `pool::run_mutation`).
fn ignore_list_edited() -> EditOutcome {
    EditOutcome {
        warnings: Vec::new(),
        remote_link: None,
    }
}

/// Resolve a warning (mark it as acknowledged)
#[tauri::command]
pub async fn resolve_warning<R: Runtime>(
    app: AppHandle<R>,
    section_id: SectionId,
    shot_id: ShotId,
    warning_code: String,
    comment: Option<String>,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, move |state| {
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
            return Ok(ignore_list_edited());
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
            return Err(CommandError::failed(
                "Could not ignore this warning: the shot no longer exists.",
            ));
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
        // Section-scoped: two sections can hold the same shot UUID (`ensure_unique_shot_uuids`
        // de-dups within a section only), so an unscoped check would call a UUID persisted only in
        // ANOTHER section durable and key the ignore by it anyway.
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
        state
            .save_resolved_warning_gated(&warning)
            .map(|()| ignore_list_edited())
            .map_err(|e| {
                // A read-only rejection (an edit-gate code) is expected control flow on a
                // remote project opened without the lock, not a persistence failure worth an
                // error log.
                if !e.code.is_edit_gate() {
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
    })
    .await
}

/// Unresolve a warning (remove acknowledgment)
#[tauri::command]
pub async fn unresolve_warning<R: Runtime>(
    app: AppHandle<R>,
    section_id: SectionId,
    shot_id: ShotId,
    warning_code: String,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, move |state| {
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
            .map(|()| ignore_list_edited())
            .map_err(|e| {
                // A read-only rejection (an edit-gate code) is expected control flow, not a
                // persistence failure worth an error log.
                if !e.code.is_edit_gate() {
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
    })
    .await
}
