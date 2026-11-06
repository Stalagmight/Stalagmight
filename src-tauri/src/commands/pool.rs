//! [#576 C2] Where a command body runs: on Tauri's blocking thread pool, never on
//! the main thread.
//!
//! Tauri runs a plain (non-`async`) `#[tauri::command]` inline in its IPC handler,
//! and that handler runs on the main thread (tauri-macros `body_blocking`): the
//! window cannot repaint, scroll or deliver an event until the command returns,
//! and a second IPC call - `cancel_loading` among them - is only dispatched after
//! it. A command therefore hands its body to [`run`], which moves it onto the
//! blocking pool and awaits it, so the main thread only parses
//! the arguments and spawns.
//!
//! The commands are generic over the [`Runtime`] so the tests call the registered
//! command itself with a `tauri::test` app (`MockRuntime`), not a hand-written
//! twin of its body (#576 C9).
//!
//! A panic in a body becomes the command's failure ([`PanicReply`]:
//! `Result<_, CommandError>` - `INTERNAL_ERROR` - and the settings commands'
//! `Result<_, String>`), so every call answers. That is why even a read that
//! cannot otherwise fail (`can_undo`, `check_project_integrity`) or a command that
//! only records something (`set_workspace_mode`) answers a `Result`: the frontend
//! awaits such calls inside its edit queue (`runSerialized`), and a call that never
//! answered held every later edit behind it, silently, for the rest of the session
//! (#576 RV5-6). No command re-raises a crash any more: `get_config`, the last one
//! that did (and so never resolved), answers a `Result` too (#576 NF-W7E-5).
//!
//! What a body may read, and how, is the read-consistency rule in
//! docs/architecture.md ("Where commands run"): a read of the project takes
//! `AppState::settled_snapshot` or runs under `AppState::settled_read`.

use tauri::{AppHandle, Manager, Runtime};

use crate::core::state::{for_project, ProjectIdentity};
use crate::core::{AppState, CommandError, MutationOutcome};

/// How a command's answer says its body panicked on the pool.
pub(crate) trait PanicReply {
    fn panicked(message: String) -> Self;
}

impl<T> PanicReply for Result<T, String> {
    fn panicked(message: String) -> Self {
        Err(message)
    }
}

/// [#576 C4] The one error envelope: a crashed body is `INTERNAL_ERROR`.
impl<T> PanicReply for Result<T, CommandError> {
    fn panicked(message: String) -> Self {
        Err(CommandError::internal(message))
    }
}

/// Run `f` against the app's [`AppState`] on the blocking pool.
pub(crate) async fn run<R, T>(
    app: AppHandle<R>,
    f: impl FnOnce(&AppState) -> T + Send + 'static,
) -> T
where
    R: Runtime,
    T: PanicReply + Send + 'static,
{
    match spawn(move || f(app.state::<AppState>().inner())).await {
        Ok(answer) => answer,
        Err(crash) => T::panicked(crash_message(&crash)),
    }
}

/// [#576 D22] [`run`] for a SURVEY MUTATION: its answer - success or failure -
/// carries the open project's SpeleoDB link bits ([`AppState::remote_link_bits`]),
/// read on the pool right after `f` returns.
///
/// The one writer of `remote_link`. The frontend applies the bits in one place
/// (`invokeIpc`), which replaced the hand-written copies of the backend's dirty
/// bookkeeping that each editing path used to keep - and get wrong when one was
/// missing or one too many (#303, #576 A18(a)). A command that mutates the survey
/// answers through here; `commands::app_contract_tests` checks that every
/// command whose answer carries the bits does.
///
/// [#576 FZ-FZ-INT-7] `expected_project` is the project the frontend made the
/// edit for (the command's `expected_project` argument): `f` runs inside
/// [`for_project`], so the mutation it reaches is refused with `PROJECT_CHANGED`
/// when another project is open by the time it holds `command_lock`. Every
/// survey mutation names one; `app_contract_tests` checks that each command
/// passes its own argument through.
pub(crate) async fn run_mutation<R, T>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    f: impl FnOnce(&AppState) -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError>
where
    R: Runtime,
    T: MutationOutcome + Send + 'static,
{
    let after_crash = app.clone();
    let answer = spawn(move || {
        let state = app.state::<AppState>().inner();
        let mut answer = for_project(expected_project, || f(state));
        let link = state.remote_link_bits();
        match &mut answer {
            Ok(outcome) => outcome.set_remote_link(link),
            Err(err) => err.payload.remote_link = link,
        }
        answer
    })
    .await;
    match answer {
        Ok(answer) => answer,
        Err(crash) => {
            // [#576 RV7a-2] The body may have marked the copy before it crashed (a
            // single undo stamps the link before its geometry reconcile): the
            // failure carries the bits as they stand now, read on the pool like any
            // answer's, so the frontend applies them as for every other failure.
            // `try_state`: a body that crashed because the state is missing has no
            // bits to read.
            let mut error = CommandError::internal(crash_message(&crash));
            error.payload.remote_link = spawn(move || {
                after_crash
                    .try_state::<AppState>()
                    .and_then(|state| state.remote_link_bits())
            })
            .await
            .ok()
            .flatten();
            Err(error)
        }
    }
}

/// [`run`] for a command that needs no [`AppState`] (a file parse, a hash; another
/// managed state is reached through an `AppHandle` moved into `f`).
pub(crate) async fn run_stateless<T>(f: impl FnOnce() -> T + Send + 'static) -> T
where
    T: PanicReply + Send + 'static,
{
    match spawn(f).await {
        Ok(answer) => answer,
        Err(crash) => T::panicked(crash_message(&crash)),
    }
}

/// Spawn `f` on the blocking pool.
///
/// Test builds hand the calling thread's config-file resolution to the worker:
/// tests isolate the config per thread (`test_helpers::with_isolated_config`), and
/// a body that reads or writes the config must see the test's file, not a fresh
/// one of the pool thread's own.
fn spawn<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> tauri::async_runtime::JoinHandle<T> {
    #[cfg(test)]
    let config = crate::test_helpers::ConfigHandoff::capture();
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(test)]
        let _config = config.adopt();
        f()
    })
}

fn crash_message(crash: &tauri::Error) -> String {
    tracing::error!(
        reason = %crash,
        purpose = "report a command that failed on the blocking pool",
        "Command body did not complete"
    );
    format!("The operation failed unexpectedly ({crash})")
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod tests {
    //! [#576 RV5-6] A command whose body panics on the pool answers `INTERNAL_ERROR`.
    //!
    //! The crash is real, in the registered command itself: the app has no state
    //! managed, so the body's first step on the pool thread - `app.state::<T>()` -
    //! panics (tauri's `Manager::state`, "state() called before manage()"). These
    //! commands used to re-raise that panic, and the call never resolved.

    use tauri::test::mock_app;

    use crate::commands::command_tests::{h, run};
    use crate::commands::{config, history, internal_error, launch, project, validation};
    use crate::core::{CommandError, CommandErrorCode, WorkspaceMode};

    fn assert_crash_answered<T>(command: &str, answer: Result<T, CommandError>) {
        let err = answer.err().expect("a crashed body answers a failure");
        println!("{command} with a crashed body -> {err:?}");
        assert_eq!(err.code, CommandErrorCode::InternalError, "{command}");
        assert!(
            err.message.starts_with("The operation failed unexpectedly"),
            "{command}: {}",
            err.message
        );
    }

    /// The reads and the recorder the frontend awaits inside its edit queue, and the
    /// banner's internal-error status read (awaited at startup).
    #[test]
    fn a_command_the_edit_queue_awaits_answers_a_crash() {
        let app = mock_app();
        assert_crash_answered("can_undo", run(history::can_undo(h(&app))));
        assert_crash_answered("can_redo", run(history::can_redo(h(&app))));
        assert_crash_answered(
            "check_project_integrity",
            run(validation::check_project_integrity(h(&app))),
        );
        assert_crash_answered(
            "set_workspace_mode",
            run(project::set_workspace_mode(h(&app), WorkspaceMode::Plan)),
        );
        assert_crash_answered(
            "get_internal_error_status",
            run(internal_error::get_internal_error_status(h(&app))),
        );
    }

    /// [#576 NF-W7E-5] The settings read, which used to re-raise the crash so the
    /// call never resolved.
    #[test]
    fn the_settings_read_answers_a_crash() {
        let app = mock_app();
        assert_crash_answered("get_config", run(config::get_config(h(&app))));
    }

    /// The same through [`super::run_stateless`]: the managed state a stateless
    /// body reaches through its `AppHandle` is missing too.
    #[test]
    fn a_stateless_command_answers_a_crash() {
        let app = mock_app();
        assert_crash_answered(
            "take_pending_launch_files",
            run(launch::take_pending_launch_files(h(&app))),
        );
    }
}
