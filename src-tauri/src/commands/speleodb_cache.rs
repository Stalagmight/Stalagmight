//! IPC for the SpeleoDB copies cached on this machine: the home screen's listing
//! (with each copy's owner relative to the signed-in account), removing one copy,
//! and Settings' "Purge all cached files".
//!
//! The listing is purely local (no token, no network). Removing a copy and the
//! purge delete files and then, for each removed copy of the signed-in account,
//! contact SpeleoDB with the session's token: a short probe of who holds the
//! project's lock, and the normal release when it is this account's
//! (`core::state::speleodb_cached`). All three run off the main thread; the two
//! removals also hold the app's exit while they run (`exit_wait`).

use tauri::{AppHandle, Manager};

use crate::core::state::{SpeleoCachedProject, SpeleoPurgeReport};
use crate::core::{AppState, CommandResult};

/// Every cached SpeleoDB copy, classified against the current session. The home
/// screen lists the ones whose `ownership` is not `other_account`.
#[tauri::command]
pub async fn speleodb_cached_projects(app: AppHandle) -> Vec<SpeleoCachedProject> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>().inner().speleodb_cached_projects()
    })
    .await
    .unwrap_or_default()
}

/// Remove one cached copy (not the open project). `discard_unpushed` is the user's
/// explicit consent to lose edits that were never uploaded; without it such a copy
/// is refused.
#[tauri::command]
pub async fn speleodb_remove_cached_copy(
    app: AppHandle,
    project_id: String,
    discard_unpushed: bool,
) -> CommandResult {
    // Counted as exclusive by the frontend gate, so the app's exit waits for it
    // like for every other such call (lock#6): a quit never cuts a removal.
    let _transfer = super::exit_wait::user_transfer(&app);
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>()
            .inner()
            .speleodb_remove_cached_copy(&project_id, discard_unpushed)
    })
    .await
    .unwrap_or_else(|e| CommandResult::failure(format!("Removing the copy failed: {e}")))
}

/// Remove every cached copy except the open one, keeping copies with un-uploaded
/// (or unreadable) work unless `discard_unpushed` carries the user's consent.
#[tauri::command]
pub async fn speleodb_purge_cached_copies(
    app: AppHandle,
    discard_unpushed: bool,
) -> SpeleoPurgeReport {
    // Exclusive at the frontend gate too, so the exit waits for it (lock#6).
    let _transfer = super::exit_wait::user_transfer(&app);
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>()
            .inner()
            .speleodb_purge_cached_copies(discard_unpushed)
    })
    .await
    .unwrap_or_else(|e| SpeleoPurgeReport {
        failed: vec![format!("The purge stopped unexpectedly: {e}")],
        ..SpeleoPurgeReport::default()
    })
}
