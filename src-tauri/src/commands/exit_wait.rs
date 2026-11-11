//! The app's exit never cuts a user-visible SpeleoDB transfer (lock#6, frontend#4).
//!
//! Before, `RunEvent::ExitRequested` ran the bounded lock release, which queued
//! behind the upload holding the lock-ops span for 5 seconds and then let the
//! process exit anyway: the upload was killed mid-transfer with an unknown
//! outcome, and the lock stayed held (SpeleoDB has no lock timeout).
//!
//! Now the exit is the LAST step of one "quit after in-flight work" path:
//!
//! 1. The frontend's close listener (`src/utils/quitAfterWork.ts`) is the first
//!    gate for every quit route that has a window - menu Quit, Ctrl+Q, the window
//!    X, an OS close, the update restart. While a transfer is in flight it keeps
//!    the window, toasts that the app will close when the work finishes, shows
//!    progress, and quits once it is done. "Quit now" is an explicit in-app
//!    choice behind a warning; it arms [`arm_forced_exit`], and
//!    [`disarm_forced_exit`] takes the arm back when the window stays after all.
//! 2. This hook is the backstop for a transfer that is still running when the
//!    exit arrives anyway (the window destroyed without the listener, or a quit
//!    route that never asks the window to close - on macOS the native app-menu
//!    Quit / Cmd+Q may be one, #640): with one in flight and no forced quit
//!    armed, it PREVENTS the exit, waits for it off the main thread, runs the
//!    bounded release, and then exits.
//!
//! Only the transfers the frontend gate itself waits on hold the exit: exactly
//! the IPC commands it counts as `exclusive` (`src/utils/speleoNetworkCommands.ts`),
//! that is the ones that upload, acquire, download, release or change the
//! account, and the exit / sign-out actions (`close_project`, the cached-copy
//! discard). Each registers with [`user_transfer`] for its whole run, and a test
//! here pins the two sets equal, both ways, across every command module.
//! Backend-internal work the user never sees (the background lock re-verify, the
//! stranded-claim cleanup, a detached release from a project switch) never keeps
//! a windowless process alive: the exit keeps the old bounded behaviour for it -
//! the release waits at most [`crate::EXIT_LOCK_RELEASE_TIMEOUT`] behind it, then
//! the app exits.
//!
//! The update restart (`app.restart()`) cannot be prevented by Tauri once it is
//! requested, so this hook lets its exit through. The install command guards it
//! instead (`app_update::install_app_update_and_restart`): it refuses while a
//! user transfer is in flight unless the user armed the forced quit
//! ([`ExitWait::restart_blocked`]), checked before its lock release and again
//! right before the install. The frontend gate stays the first gate; this one
//! does not rely on it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, State};

use crate::core::AppState;

/// How long an armed forced quit stays valid. It covers one hop - the frontend
/// arms it right before it destroys the window - so a stale arm can never let a
/// later, ordinary quit cut a transfer.
const FORCED_EXIT_TTL: Duration = Duration::from_secs(10);

/// The user-visible transfers running right now.
#[derive(Default)]
struct Transfers {
    count: Mutex<usize>,
    idle: Condvar,
}

impl Transfers {
    fn count(&self) -> std::sync::MutexGuard<'_, usize> {
        self.count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One user-visible transfer, registered for as long as this lives.
#[must_use = "the transfer holds the exit only while this guard lives"]
pub struct UserTransfer(Arc<Transfers>);

impl Drop for UserTransfer {
    fn drop(&mut self) {
        let mut count = self.0.count();
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.0.idle.notify_all();
        }
    }
}

/// Managed exit state.
#[derive(Default)]
pub struct ExitWait {
    /// Until when the user's explicit "Quit now" may cut an in-flight transfer.
    forced_until: Mutex<Option<Instant>>,
    /// A background wait is already running for this exit.
    waiting: AtomicBool,
    /// The background wait finished and released: the exit it re-issues goes on.
    drained: AtomicBool,
    transfers: Arc<Transfers>,
}

impl ExitWait {
    fn phase(&self) -> WaitPhase {
        if self.drained.load(Ordering::SeqCst) {
            WaitPhase::Drained
        } else if self.waiting.load(Ordering::SeqCst) {
            WaitPhase::Waiting
        } else {
            WaitPhase::Idle
        }
    }

    fn forced(&self) -> bool {
        self.forced_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|until| Instant::now() < until)
    }

    /// Arm (`Some`) or disarm (`None`) the forced exit; see [`arm_forced_exit`]
    /// and [`disarm_forced_exit`].
    fn set_forced(&self, armed: bool) {
        *self
            .forced_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            armed.then(|| Instant::now() + FORCED_EXIT_TTL);
    }

    /// Register a user-visible transfer (see [`user_transfer`]).
    pub fn begin_transfer(&self) -> UserTransfer {
        *self.transfers.count() += 1;
        UserTransfer(Arc::clone(&self.transfers))
    }

    fn transfers_in_flight(&self) -> bool {
        *self.transfers.count() > 0
    }

    /// Whether an update restart must not go ahead now: a user-visible transfer
    /// is in flight and the user did not choose "Quit now" over it. Tauri ignores
    /// `prevent_exit` for a restart, so the install command asks this itself
    /// before it releases and installs (see the module doc).
    pub(crate) fn restart_blocked(&self) -> bool {
        self.transfers_in_flight() && !self.forced()
    }

    /// Block until no user-visible transfer is running. Bounded only by the
    /// transfers' own network timeouts, so never call this on the main thread.
    fn wait_for_transfers(&self) {
        let mut count = self.transfers.count();
        while *count > 0 {
            count = self
                .transfers
                .idle
                .wait(count)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// What to do with one `ExitRequested` carrying `code`.
    fn step(&self, code: Option<i32>) -> ExitStep {
        exit_step(
            self.phase(),
            self.forced() || code == Some(tauri::RESTART_EXIT_CODE),
            self.transfers_in_flight(),
        )
    }
}

/// Register a user-visible transfer for as long as the returned guard lives, so
/// the app's exit waits for it. `None` before the state is managed (nothing can
/// exit then either).
pub fn user_transfer(app: &AppHandle) -> Option<UserTransfer> {
    app.try_state::<ExitWait>()
        .map(|wait| wait.begin_transfer())
}

/// What the exit hook does with one `ExitRequested`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExitStep {
    /// Our own re-issued exit after the wait: everything is already done.
    Proceed,
    /// Nothing the user started is running (or the user chose to cut it): the
    /// bounded release, then exit.
    ReleaseAndExit,
    /// Keep the process, wait for the transfer off the main thread, then exit.
    WaitThenExit,
    /// A wait for this exit is already running; keep the process.
    KeepWaiting,
}

/// Where this process's exit wait stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitPhase {
    Idle,
    Waiting,
    /// The wait finished and released; the exit it re-issued goes on.
    Drained,
}

/// The decision, kept pure so it is testable without a running app. `may_cut`:
/// the user chose "Quit now", or this is a restart (Tauri ignores
/// `prevent_exit` for one). `in_flight`: a user-visible transfer is running.
pub(crate) fn exit_step(phase: WaitPhase, may_cut: bool, in_flight: bool) -> ExitStep {
    match phase {
        WaitPhase::Drained => ExitStep::Proceed,
        _ if may_cut || !in_flight => ExitStep::ReleaseAndExit,
        WaitPhase::Waiting => ExitStep::KeepWaiting,
        WaitPhase::Idle => ExitStep::WaitThenExit,
    }
}

/// The `RunEvent::ExitRequested` hook (see the module doc).
pub fn on_exit_requested(app: &AppHandle, code: Option<i32>, api: &tauri::ExitRequestApi) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let wait = app.state::<ExitWait>();
    match wait.step(code) {
        ExitStep::Proceed => {}
        ExitStep::ReleaseAndExit => {
            state.speleodb_release_held_lock_bounded(crate::EXIT_LOCK_RELEASE_TIMEOUT);
        }
        ExitStep::KeepWaiting => api.prevent_exit(),
        ExitStep::WaitThenExit => {
            api.prevent_exit();
            wait.waiting.store(true, Ordering::SeqCst);
            tracing::info!(
                purpose = "let an in-flight SpeleoDB transfer finish before exiting",
                "exit deferred until the SpeleoDB transfer in flight has finished"
            );
            let app = app.clone();
            std::thread::spawn(move || {
                let wait = app.state::<ExitWait>();
                wait.wait_for_transfers();
                app.state::<AppState>()
                    .speleodb_release_held_lock_bounded(crate::EXIT_LOCK_RELEASE_TIMEOUT);
                wait.drained.store(true, Ordering::SeqCst);
                app.exit(0);
            });
        }
    }
}

/// The user's explicit "Quit now" from the in-app waiting prompt: the exit that
/// follows within [`FORCED_EXIT_TTL`] may cut the transfer in flight instead of
/// waiting for it.
#[tauri::command]
pub fn arm_forced_exit(wait: State<'_, ExitWait>) {
    wait.set_forced(true);
    tracing::warn!(
        purpose = "quit without waiting for an in-flight SpeleoDB transfer (user chose it)",
        "forced exit armed"
    );
}

/// The forced quit did not happen after all: the window survived its destroy, so
/// the app is still running and the user can go on working. Left armed, the next
/// exit within [`FORCED_EXIT_TTL`] - an ordinary quit the user starts a moment
/// later, or the OS ending the app - would cut a transfer the user never chose to
/// cut. Idempotent.
#[tauri::command]
pub fn disarm_forced_exit(wait: State<'_, ExitWait>) {
    wait.set_forced(false);
    tracing::info!(
        purpose = "a forced quit whose window close failed must not cut a later transfer",
        "forced exit disarmed"
    );
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_user_transfer_in_flight_holds_the_exit_until_it_has_finished() {
        let wait = ExitWait::default();
        let upload = wait.begin_transfer();
        assert_eq!(wait.step(None), ExitStep::WaitThenExit);
        // A second request while waiting keeps waiting; it never spawns a second wait.
        wait.waiting.store(true, Ordering::SeqCst);
        assert_eq!(wait.step(Some(0)), ExitStep::KeepWaiting);
        // The exit the wait re-issues goes straight through.
        wait.drained.store(true, Ordering::SeqCst);
        assert_eq!(wait.step(Some(0)), ExitStep::Proceed);
        drop(upload);
    }

    #[test]
    fn backend_internal_work_never_holds_the_exit() {
        // The background re-verify, the stranded-claim cleanup and a detached
        // switch release never register a user transfer, so whatever they hold,
        // the exit keeps the old bounded release-then-exit (the release's own
        // budget is pinned in `speleodb_lock_release`'s tests).
        let wait = ExitWait::default();
        assert_eq!(wait.step(None), ExitStep::ReleaseAndExit);
        // And a finished transfer no longer counts.
        drop(wait.begin_transfer());
        assert_eq!(wait.step(None), ExitStep::ReleaseAndExit);
    }

    #[test]
    fn only_the_users_explicit_quit_now_or_a_restart_cuts_a_transfer() {
        let wait = ExitWait::default();
        let _upload = wait.begin_transfer();
        assert_eq!(
            wait.step(Some(tauri::RESTART_EXIT_CODE)),
            ExitStep::ReleaseAndExit
        );
        *wait.forced_until.lock().expect("lock") = Some(Instant::now() + FORCED_EXIT_TTL);
        assert_eq!(wait.step(None), ExitStep::ReleaseAndExit);
    }

    #[test]
    fn the_exit_wait_returns_only_once_every_transfer_has_finished() {
        let wait = ExitWait::default();
        let upload = wait.begin_transfer();
        let acquire = wait.begin_transfer();
        let (done, finished) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                wait.wait_for_transfers();
                done.send(()).expect("report");
            });
            drop(upload);
            // One still running: the wait must not give up on a deadline.
            assert!(finished.recv_timeout(Duration::from_millis(300)).is_err());
            drop(acquire);
            finished
                .recv_timeout(Duration::from_secs(5))
                .expect("the wait ends with the last transfer");
        });
        assert!(!wait.transfers_in_flight());
    }

    /// The update restart's own gate: Tauri ignores `prevent_exit` for a restart
    /// (the exit step above lets it through), so the install command must refuse
    /// a restart that would cut a transfer - unless the user chose Quit now.
    #[test]
    fn a_restart_is_blocked_by_a_transfer_unless_the_user_chose_quit_now() {
        let wait = ExitWait::default();
        assert!(!wait.restart_blocked(), "nothing in flight");
        let upload = wait.begin_transfer();
        assert!(wait.restart_blocked(), "an upload in flight");
        *wait.forced_until.lock().expect("lock") = Some(Instant::now() + FORCED_EXIT_TTL);
        assert!(!wait.restart_blocked(), "Quit now armed");
        *wait.forced_until.lock().expect("lock") = Some(Instant::now());
        assert!(wait.restart_blocked(), "an expired arm cuts nothing");
        drop(upload);
        assert!(!wait.restart_blocked(), "the upload finished");
    }

    /// A forced quit whose window survived (its destroy failed) is disarmed: the
    /// next exit waits for a transfer again instead of cutting it, and an update
    /// restart is refused over one again.
    #[test]
    fn a_disarmed_forced_quit_cuts_nothing() {
        let wait = ExitWait::default();
        let _upload = wait.begin_transfer();
        wait.set_forced(true);
        assert!(wait.forced());
        assert_eq!(wait.step(None), ExitStep::ReleaseAndExit);
        assert!(!wait.restart_blocked());

        wait.set_forced(false);
        assert!(!wait.forced());
        assert_eq!(wait.step(None), ExitStep::WaitThenExit);
        assert!(wait.restart_blocked());
        // Disarming what is not armed is a no-op.
        wait.set_forced(false);
        assert!(!wait.forced());
    }

    #[test]
    fn a_forced_quit_arm_expires() {
        let wait = ExitWait::default();
        assert!(!wait.forced());
        *wait.forced_until.lock().expect("lock") = Some(Instant::now() + FORCED_EXIT_TTL);
        assert!(wait.forced());
        *wait.forced_until.lock().expect("lock") = Some(Instant::now());
        assert!(!wait.forced());
    }

    /// Whether this source line keeps a user transfer registered for the rest of
    /// its scope: a NAMED binding (`let _transfer = ...user_transfer(&app);`). A
    /// `let _ =` (typed or not) or a bare call drops the guard on the spot, so the
    /// command would hold nothing while its text still mentions the call.
    fn binds_the_transfer(line: &str) -> bool {
        line.contains("user_transfer(&app)")
            && line
                .trim_start()
                .strip_prefix("let ")
                .and_then(|rest| rest.split_once('='))
                .map(|(pattern, _)| {
                    let pattern = pattern.split(':').next().unwrap_or_default().trim();
                    pattern.strip_prefix("mut ").unwrap_or(pattern).trim()
                })
                .is_some_and(|name| !name.is_empty() && name != "_")
    }

    /// [`binds_the_transfer`] for any line of `src`.
    fn holds_a_transfer(src: &str) -> bool {
        src.lines().any(binds_the_transfer)
    }

    #[test]
    fn only_a_named_binding_keeps_the_transfer_registered() {
        assert!(binds_the_transfer(
            "    let _transfer = super::exit_wait::user_transfer(&app);"
        ));
        assert!(binds_the_transfer(
            "    let mut guard: Option<UserTransfer> = user_transfer(&app);"
        ));
        for dropped in [
            "    let _ = super::exit_wait::user_transfer(&app);",
            "    let _: Option<UserTransfer> = super::exit_wait::user_transfer(&app);",
            "    super::exit_wait::user_transfer(&app);",
            "    drop(super::exit_wait::user_transfer(&app));",
        ] {
            assert!(!binds_the_transfer(dropped), "{dropped}");
        }
    }

    /// One top-level `fn` item found in a module's source.
    struct FnSource {
        file: String,
        name: String,
        /// Its 0-based lines, signature to closing brace.
        lines: std::ops::RangeInclusive<usize>,
        body: String,
    }

    /// The `fn` item that starts at line `start` of `lines` (the line holding
    /// `fn `), up to the first closing brace at column 0.
    fn fn_at(file: &str, lines: &[&str], start: usize) -> FnSource {
        let end = (start..lines.len())
            .find(|&j| lines[j] == "}")
            .unwrap_or_else(|| panic!("{file}:{}: a fn body that never ends", start + 1));
        let line = lines[start];
        let after_fn = &line[line.find("fn ").expect("a fn line") + 3..];
        FnSource {
            file: file.to_owned(),
            name: after_fn
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect(),
            lines: start..=end,
            body: lines[start..=end].join("\n"),
        }
    }

    /// Every `#[tauri::command]` in `src` (commands are top-level items).
    fn commands_in(file: &str, src: &str) -> Vec<FnSource> {
        let lines: Vec<&str> = src.lines().collect();
        let mut found = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            if !lines[i].trim_start().starts_with("#[tauri::command") {
                i += 1;
                continue;
            }
            let start = (i + 1..lines.len())
                .find(|&j| lines[j].contains("fn "))
                .unwrap_or_else(|| panic!("{file}:{}: a command without a fn", i + 1));
            let command = fn_at(file, &lines, start);
            i = command.lines.end() + 1;
            found.push(command);
        }
        found
    }

    /// Every `.rs` file under `dir`, recursively, as (path, source). Read at test
    /// time, so a module added later is scanned without anyone listing it here.
    fn rust_sources(dir: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
        let mut found = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).expect("a readable source directory") {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let src = std::fs::read_to_string(&path).expect("a readable module");
                    found.push((path, src));
                }
            }
        }
        found.sort();
        found
    }

    /// The `exclusive` and the `read` rows of the frontend gate's table
    /// (`speleoNetworkCommands.ts`).
    fn frontend_kinds() -> (Vec<String>, Vec<String>) {
        let table = include_str!("../../../src/utils/speleoNetworkCommands.ts");
        let (mut exclusive, mut read) = (Vec::new(), Vec::new());
        for line in table.lines() {
            let Some((name, kind)) = line.trim().split_once(':') else {
                continue;
            };
            let identifier = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            match kind.trim() {
                "'exclusive'," if identifier => exclusive.push(name.to_owned()),
                "'read'," if identifier => read.push(name.to_owned()),
                _ => {}
            }
        }
        (exclusive, read)
    }

    /// The commands whose backend holds the app's exit are EXACTLY the ones the
    /// frontend's quit gate counts as `exclusive` (`speleoNetworkCommands.ts`),
    /// both ways, across every command module; no read ever holds the exit.
    ///
    /// A command the backend holds but the frontend does not count lets a quit
    /// destroy the window while the process lives on, windowless, until it ends:
    /// the gap `close_project` and the sign-out discard had, because this test
    /// used to read the SpeleoDB modules only. The reverse makes a quit wait for
    /// work the backend would not wait for.
    #[test]
    fn exactly_the_frontend_gates_transfers_hold_the_exit() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let commands_dir = root.join("commands");
        let sources = rust_sources(&root);
        let mut commands = Vec::new();
        let mut run_transfer = None;
        for (path, src) in &sources {
            if path.parent() != Some(commands_dir.as_path()) {
                continue;
            }
            let file = path.file_name().and_then(|n| n.to_str()).expect("UTF-8");
            commands.extend(commands_in(file, src));
            let lines: Vec<&str> = src.lines().collect();
            if let Some(start) = lines
                .iter()
                .position(|l| l.starts_with("async fn run_transfer<"))
            {
                run_transfer = Some(fn_at(file, &lines, start));
            }
        }
        assert!(
            commands.len() > 100,
            "every command module was scanned ({} commands)",
            commands.len()
        );
        let run_transfer = run_transfer.expect("run_transfer is defined");
        // `run_transfer` itself holds the guard for the whole run.
        assert!(
            holds_a_transfer(&run_transfer.body),
            "run_transfer must keep its transfer registered"
        );

        // Every registration anywhere in the app crate is a named binding (a
        // `let _ =` would read as holding the exit and hold nothing), inside a
        // command or `run_transfer` - so no helper can hold the exit for a command
        // this scan does not attribute it to.
        for (path, src) in &sources {
            if path.ends_with("commands/exit_wait.rs") {
                continue;
            }
            let file = path.file_name().and_then(|n| n.to_str()).expect("UTF-8");
            let in_commands = path.parent() == Some(commands_dir.as_path());
            for (index, line) in src.lines().enumerate() {
                let code = line.trim_start();
                let registers = code.contains("user_transfer(");
                if code.starts_with("//") || !(registers || code.contains("run_transfer(")) {
                    continue;
                }
                if registers {
                    assert!(
                        binds_the_transfer(line),
                        "{file}: the transfer guard is dropped at once: {line}"
                    );
                }
                let inside = |f: &FnSource| f.file == file && f.lines.contains(&index);
                assert!(
                    in_commands && (inside(&run_transfer) || commands.iter().any(inside)),
                    "{file}:{}: an exit-holding transfer outside an IPC command: {line}",
                    index + 1
                );
            }
        }

        let holds = |c: &FnSource| c.body.contains("run_transfer(") || holds_a_transfer(&c.body);
        let (exclusive, read) = frontend_kinds();
        assert!(
            exclusive.len() + read.len() > 20,
            "the frontend table was read ({} rows)",
            exclusive.len() + read.len()
        );
        let command = |name: &str| {
            let named: Vec<&FnSource> = commands.iter().filter(|c| c.name == name).collect();
            assert_eq!(
                named.len(),
                1,
                "{name}: exactly one IPC command by this name"
            );
            named[0]
        };
        // Frontend -> backend.
        for name in &exclusive {
            assert!(
                holds(command(name)),
                "{name} is exclusive at the quit gate: its backend must hold the exit"
            );
        }
        for name in &read {
            assert!(
                !holds(command(name)),
                "{name} is a read: it must never hold the exit"
            );
        }
        // Backend -> frontend, whatever module the command lives in.
        let held: Vec<&str> = commands
            .iter()
            .filter(|c| holds(c))
            .map(|c| c.name.as_str())
            .collect();
        for name in &held {
            assert!(
                exclusive.iter().any(|e| e == name),
                "{name} holds the backend's exit: the frontend's quit gate must count it \
                 (an `exclusive` row in speleoNetworkCommands.ts)"
            );
        }
        // The exit / sign-out actions in project.rs are among them.
        for name in ["close_project", "speleodb_discard_cached_copy"] {
            assert!(held.contains(&name), "{name} holds the exit");
        }
    }
}
