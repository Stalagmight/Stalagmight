//! [#576 RV5-PI-3] The internal-error banner's pull path.
//!
//! The backend pushes `internal-error` once per session, the first time a crash
//! leaves a lock poisoned. A push alone can be missed: a crash during startup
//! fires before the webview has registered its listener, and a webview reload
//! resets the frontend's in-memory store after the one event went out - leaving no
//! banner while undo, redo and the SpeleoDB push keep refusing. The frontend asks
//! this once its listener is registered.

use tauri::{AppHandle, Runtime};

use crate::commands::pool;
use crate::core::events::InternalErrorNotice;
use crate::core::CommandError;

/// The first lock a crash poisoned this session, or `None` when nothing has.
/// The same notice the `internal-error` event carries.
#[tauri::command]
pub async fn get_internal_error_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<InternalErrorNotice>, CommandError> {
    pool::run(app, |state| Ok(state.internal_error_status())).await
}
