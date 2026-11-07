use std::collections::HashMap;

use tauri::{AppHandle, Manager, Runtime};

use crate::commands::pool;
use crate::core::CommandError;
use crate::telemetry::TelemetryCollector;

/// Receive a batch of event counts from the frontend.
#[tauri::command]
pub async fn track_events<R: Runtime>(
    app: AppHandle<R>,
    events: HashMap<String, u64>,
    project_seq: u32,
) -> Result<(), CommandError> {
    pool::run_stateless(move || {
        app.state::<TelemetryCollector>()
            .track_batch(&events, project_seq);
        Ok(())
    })
    .await
}

/// Hand the pending telemetry to the offline buffer - the app-close path.
///
/// No network: sending here made quitting wait on the server, one request per
/// open project bucket (#576 B17). The next launch's background drain sends
/// what this writes.
#[tauri::command]
pub async fn flush_telemetry<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    pool::run_stateless(move || {
        app.state::<TelemetryCollector>().buffer_pending();
        Ok(())
    })
    .await
}

/// Returns true when the telemetry feature is compiled in.
/// The frontend uses this to detect feature availability.
#[tauri::command]
pub async fn is_telemetry_available() -> Result<bool, CommandError> {
    pool::run_stateless(|| Ok(cfg!(feature = "telemetry"))).await
}

/// Update the telemetry enabled state (called when user toggles in settings).
#[tauri::command]
pub async fn set_telemetry_enabled<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<(), CommandError> {
    pool::run_stateless(move || {
        app.state::<TelemetryCollector>().set_enabled(enabled);
        Ok(())
    })
    .await
}
