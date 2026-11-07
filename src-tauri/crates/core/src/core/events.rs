//! The event boundary between core and the host application.
//!
//! Core never talks to the UI layer directly: anything it wants delivered
//! (loading progress today) goes through the [`EventSink`] installed on
//! [`AppState`] during app setup. The host application provides the one real
//! implementation (a thin wrapper over the Tauri app handle); unit tests run
//! with no sink installed, which every consumer treats as "no one listening".

use super::loading::LoadingProgress;
use super::state::AppState;

/// [#576 C13] One backend -> webview event: its wire name and its payload type,
/// declared once. Emitters take both from here (core through [`payload`], the app
/// crate through its `emit_event` helper), so a misspelled name or a payload of
/// the wrong type does not compile; `pnpm gen:ipc-types` writes every event below
/// to `src/types/ipc/IpcEvents.ts`, which the frontend's listeners use.
pub trait IpcEvent {
    /// The name `listen()` takes.
    const NAME: &'static str;
    /// What the event carries; `()` for a bare signal, which arrives as `null`.
    type Payload: serde::Serialize + ts_rs::TS + Clone;
}

/// Load / create / import / export progress ([`crate::core::loading::ProgressEmitter`]).
pub struct LoadingProgressEvent;
impl IpcEvent for LoadingProgressEvent {
    const NAME: &'static str = "loading-progress";
    type Payload = LoadingProgress;
}

/// The tile protocol reached the network again after a transport failure.
pub struct TileTransportSuccess;
impl IpcEvent for TileTransportSuccess {
    const NAME: &'static str = "tile-transport-success";
    type Payload = ();
}

/// A tile fetch failed at the transport (offline) after a success.
pub struct TileTransportError;
impl IpcEvent for TileTransportError {
    const NAME: &'static str = "tile-transport-error";
    type Payload = ();
}

/// A path the OS handed the app was queued; drain it with `take_pending_launch_files`.
pub struct LaunchFileQueued;
impl IpcEvent for LaunchFileQueued {
    const NAME: &'static str = "launch-file-queued";
    type Payload = ();
}

/// The user asked to close the window; the backend prevented it so the frontend
/// can confirm (unsaved changes) and then close it itself.
pub struct CloseRequested;
impl IpcEvent for CloseRequested {
    const NAME: &'static str = "tauri://close-requested";
    type Payload = ();
}

/// [#576 A11] Raised ONCE per session, the first time a lock is found poisoned (an
/// earlier operation panicked while holding it): the backend recovered the data and
/// carried on, and the frontend shows a persistent "internal error - save your work
/// and restart" banner. Raised through the open state's poison monitor
/// (`AppState::set_event_sink`), which the process-wide cells forward into.
pub struct InternalErrorEvent;
impl IpcEvent for InternalErrorEvent {
    const NAME: &'static str = "internal-error";
    type Payload = InternalErrorNotice;
}

/// [`InternalErrorEvent`]'s payload.
#[derive(Clone, serde::Serialize, ts_rs::TS)]
pub struct InternalErrorNotice {
    /// The first lock found poisoned (`project`, `undo_stack`, `log_buffer`, ...):
    /// for the log, not for the user.
    pub cell: String,
}

/// [#576 RV4-R-S6] A SpeleoDB Done or Push FAILED after the project save it runs
/// before its upload went through: that save's outcome, whose warnings say what it
/// could not keep. The successful answer carries them itself; this is how they
/// still reach the user when the verb fails, since they are one-shot.
pub struct SpeleoPrePushSave;
impl IpcEvent for SpeleoPrePushSave {
    const NAME: &'static str = "speleodb-pre-push-save";
    type Payload = crate::core::ProjectFileOutcome;
}

/// `E`'s payload as the JSON value an [`EventSink`] takes, typed by `E`.
pub fn payload<E: IpcEvent>(payload: &E::Payload) -> serde_json::Result<serde_json::Value> {
    serde_json::to_value(payload)
}

/// A type-erased route from core out to the running application.
pub trait EventSink: Send + Sync + 'static {
    /// Fire-and-forget UI event. Implementations log their own delivery
    /// failures; core neither sees nor retries them.
    fn emit(&self, event: &str, payload: serde_json::Value);

    /// Run `f` against the process's managed [`AppState`], if any. Returns
    /// `false` when there is no managed state (tests, pre-setup) and `f` was
    /// NOT run - the caller keeps ownership of whatever it would have moved in
    /// only in the sense that the closure is dropped uncalled, so capture
    /// clones when a fallback path still needs the data.
    fn with_app_state(&self, f: Box<dyn FnOnce(&AppState) + Send>) -> bool;
}

/// [#576 A8] The sink a unit test installs to drive the load and create paths
/// (`do_load_project_full`, `do_create_project_full`), which refuse to run without
/// one: it delivers nothing and has no managed state to lend. With it those paths
/// are ordinary unit-test entry points - a test that would otherwise pin their
/// behaviour by reading their source text drives them instead.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default, Clone, Copy)]
pub struct NullEventSink;

#[cfg(any(test, feature = "test-support"))]
impl EventSink for NullEventSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}
