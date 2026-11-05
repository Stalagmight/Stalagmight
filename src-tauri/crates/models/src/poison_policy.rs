//! [#576 A11] The one lock policy: recover what a crash left behind, and say so.
//!
//! A `std::sync::Mutex` is POISONED when a panic unwinds through one of its guards.
//! The data is still there - possibly half-way through whatever the crashed code
//! was doing to it - and every later `lock()` returns `Err` for the rest of the
//! process: release builds unwind, and nothing here ever clears the flag.
//!
//! The backend used to answer that three ways at once. About half the lock sites
//! recovered the data, a quarter skipped their work silently (an edit could apply
//! without its undo step; a project create could report success over a slot it
//! never wrote), and the rest refused with a bare "Failed to lock ...". Nobody was
//! told that anything had gone wrong, which docs/notifications.md forbids.
//!
//! [`RecoveringMutex`] is the only way in now, and it has exactly one answer:
//!
//! - [`RecoveringMutex::lock`] RECOVERS the guarded data and hands back the guard.
//!   The first time a cell is found poisoned it logs which cell (and from where),
//!   and the cell's [`PoisonMonitor`] raises ONE notice for the whole session -
//!   the frontend's "internal error - save your work and restart" banner. Saving
//!   must keep working after a crash; refusing every lock would trap the user's
//!   unsaved work behind it.
//! - [`RecoveringMutex::refuse_if_poisoned`] is for the short, named list of
//!   operations where continuing on half-written data is worse than refusing.
//!   They are listed, with the reason for each, at `AppState::refuse_after_crash`
//!   in the core crate; a refusal raises the same notice.
//!
//! There is no method that returns the std `LockResult`, so a `.lock().unwrap()`,
//! `.lock().expect(..)`, `if let Ok(..) = .lock()`, `.lock().ok()` or
//! `.lock().map_err(..)` on a cell does not compile: the policy is enforced by the
//! type, not by review. [`LockCell`] lets a test enumerate the cells an owner
//! holds, poison each one and watch the policy answer.
//!
//! The guard is the ordinary [`std::sync::MutexGuard`], so code that names the
//! guard type is unchanged.

use std::fmt;
use std::panic::Location;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

/// What a [`PoisonMonitor`] calls, once, with the name of the first poisoned cell.
type PoisonHook = Box<dyn Fn(&'static str) + Send + Sync>;

/// The once-per-session notice behind a set of [`RecoveringMutex`] cells.
///
/// Each `AppState` owns one, so its cells (and a test's) are reported to its own
/// event sink; cells that live outside any `AppState` (process-wide caches, the
/// log ring buffer, telemetry) report to [`global_monitor`], which the app
/// forwards into the managed state's monitor at setup. Either way the user sees
/// one banner, however many cells a crash poisoned.
pub struct PoisonMonitor {
    /// The first cell found poisoned - what the notice names.
    first: OnceLock<&'static str>,
    /// How the notice leaves: installed once, by whoever can deliver it (the
    /// app's event sink, or a forward into another monitor).
    hook: OnceLock<PoisonHook>,
    /// Whether the hook has been called. Swapped BEFORE the call, so a hook that
    /// itself trips over a poisoned cell cannot recurse.
    raised: AtomicBool,
    /// How many cells were created against this monitor (see
    /// [`Self::registered_cells`]).
    cells: AtomicUsize,
}

impl PoisonMonitor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            first: OnceLock::new(),
            hook: OnceLock::new(),
            raised: AtomicBool::new(false),
            cells: AtomicUsize::new(0),
        })
    }

    /// Install how the notice is delivered. Only the first installation takes;
    /// returns whether this one did. A cell poisoned BEFORE the hook existed is
    /// delivered now, so a crash during startup still reaches the user.
    pub fn install_hook(&self, hook: impl Fn(&'static str) + Send + Sync + 'static) -> bool {
        let installed = self.hook.set(Box::new(hook)).is_ok();
        self.deliver();
        installed
    }

    /// Deliver this monitor's notice through `target` instead: the app routes the
    /// [`global_monitor`] into the managed state's monitor, so a poisoned cache and
    /// a poisoned project raise the same single banner.
    pub fn forward_to(&self, target: &Arc<Self>) -> bool {
        let target = Arc::clone(target);
        self.install_hook(move |cell| target.raise(cell))
    }

    /// The first cell found poisoned, if any has been.
    pub fn first_poisoned(&self) -> Option<&'static str> {
        self.first.get().copied()
    }

    /// Whether the notice has gone out.
    pub fn raised(&self) -> bool {
        self.raised.load(Ordering::Acquire)
    }

    /// How many [`RecoveringMutex`] cells were created against this monitor. An
    /// owner's test compares it with the cells the owner enumerates, so a cell
    /// added without being listed is caught.
    pub fn registered_cells(&self) -> usize {
        self.cells.load(Ordering::Acquire)
    }

    /// Record that `cell` was found poisoned and deliver the notice if it has not
    /// gone out yet.
    fn raise(&self, cell: &'static str) {
        // The first name wins; a later crash adds nothing the banner would say.
        let _ = self.first.set(cell);
        self.deliver();
    }

    fn deliver(&self) {
        let (Some(first), Some(hook)) = (self.first.get(), self.hook.get()) else {
            return;
        };
        if !self.raised.swap(true, Ordering::AcqRel) {
            hook(first);
        }
    }

    fn register(&self) {
        self.cells.fetch_add(1, Ordering::AcqRel);
    }
}

impl fmt::Debug for PoisonMonitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoisonMonitor")
            .field("first", &self.first.get())
            .field("raised", &self.raised())
            .field("cells", &self.registered_cells())
            .finish_non_exhaustive()
    }
}

/// The monitor for cells that live outside any `AppState`. Nothing is delivered
/// until the app forwards it ([`PoisonMonitor::forward_to`]); a cell poisoned
/// before then is delivered at that moment.
pub fn global_monitor() -> &'static Arc<PoisonMonitor> {
    static GLOBAL: OnceLock<Arc<PoisonMonitor>> = OnceLock::new();
    GLOBAL.get_or_init(PoisonMonitor::new)
}

/// A mutex whose only way in is the poison policy (see the module docs).
pub struct RecoveringMutex<T: ?Sized> {
    /// What the log line and the banner payload call this cell.
    name: &'static str,
    monitor: Arc<PoisonMonitor>,
    /// Whether this cell's recovery has been logged, so a poisoned cell that is
    /// locked on every frame logs once rather than flooding the log.
    logged: AtomicBool,
    inner: Mutex<T>,
}

impl<T> RecoveringMutex<T> {
    /// A cell that reports to the [`global_monitor`]: for state that lives outside
    /// an `AppState`.
    pub fn new(name: &'static str, value: T) -> Self {
        Self::with_monitor(name, value, global_monitor())
    }

    /// A cell that reports to `monitor`: for an `AppState`'s own cells.
    pub fn with_monitor(name: &'static str, value: T, monitor: &Arc<PoisonMonitor>) -> Self {
        monitor.register();
        Self {
            name,
            monitor: Arc::clone(monitor),
            logged: AtomicBool::new(false),
            inner: Mutex::new(value),
        }
    }
}

impl<T: ?Sized> RecoveringMutex<T> {
    /// Lock the cell. A poisoned cell is RECOVERED - the guard over whatever the
    /// crashed holder left - and reported (logged once per cell, one notice per
    /// monitor).
    ///
    /// The report is made BEFORE the lock is taken, so the log line and the hook
    /// never run while this cell is held: the log ring buffer is itself a cell,
    /// and reporting from inside its own guard would deadlock. The one window
    /// that check leaves - a crash landing between the check and the lock - is
    /// recovered without a report, and the next `lock()` of this cell makes it
    /// (the flag is sticky).
    #[track_caller]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        if self.inner.is_poisoned() {
            self.report(Location::caller());
        }
        // [#576 FZ-INT] Test builds only: a thread an interleaving fuzzer registered
        // takes the lock under its deterministic schedule (`interleave.rs`); every
        // other thread falls through after one atomic load.
        #[cfg(any(test, feature = "test-support"))]
        if let Some(scheduled) = crate::interleave::scheduled_lock(self.name, &self.inner) {
            return scheduled.unwrap_or_else(PoisonError::into_inner);
        }
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// For the named refusal sites only (see the module docs): `Err` when the cell
    /// is poisoned - reported like a recovery - and `Ok` when it is healthy. Takes
    /// no lock, so it is safe whatever the caller already holds.
    #[track_caller]
    pub fn refuse_if_poisoned(&self) -> Result<(), LockRefused> {
        if self.inner.is_poisoned() {
            self.report(Location::caller());
            return Err(LockRefused { cell: self.name });
        }
        Ok(())
    }

    /// Whether a crash has poisoned this cell. Never reports; for assertions and
    /// diagnostics.
    pub fn is_poisoned(&self) -> bool {
        self.inner.is_poisoned()
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    fn report(&self, at: &'static Location<'static>) {
        if !self.logged.swap(true, Ordering::AcqRel) {
            tracing::error!(
                lock = self.name,
                at = %at,
                purpose = "keep the app usable after an internal crash (#576 A11)",
                "A lock was poisoned by an earlier panic; continuing with the data it holds"
            );
        }
        self.monitor.raise(self.name);
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RecoveringMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveringMutex")
            .field("name", &self.name)
            .field("inner", &&self.inner)
            .finish_non_exhaustive()
    }
}

/// A [`RecoveringMutex`] with its payload type erased, so an owner can hand out
/// every cell it holds as one list. What the poison tests enumerate.
pub trait LockCell: Send + Sync {
    fn cell_name(&self) -> &'static str;

    fn cell_poisoned(&self) -> bool;

    /// Run `f` while holding the cell (recovering it if poisoned). A panic in `f`
    /// poisons the cell exactly as a crash in real code would.
    fn hold(&self, f: &mut dyn FnMut());
}

impl<T: ?Sized + Send> LockCell for RecoveringMutex<T> {
    fn cell_name(&self) -> &'static str {
        self.name
    }

    fn cell_poisoned(&self) -> bool {
        self.is_poisoned()
    }

    fn hold(&self, f: &mut dyn FnMut()) {
        let _held = self.lock();
        f();
    }
}

/// A refusal site found its cell poisoned (see
/// [`RecoveringMutex::refuse_if_poisoned`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockRefused {
    cell: &'static str,
}

impl LockRefused {
    /// The poisoned cell, for the log; the user-facing wording is the refusal
    /// site's.
    pub fn cell(&self) -> &'static str {
        self.cell
    }
}

impl fmt::Display for LockRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the {} lock was poisoned by an earlier internal error",
            self.cell
        )
    }
}

impl std::error::Error for LockRefused {}

#[cfg(test)]
#[expect(
    clippy::panic,
    clippy::unwrap_used,
    reason = "test module: each test panics ON PURPOSE to poison a cell"
)]
mod tests {
    use super::{LockCell, PoisonMonitor, RecoveringMutex};
    use std::sync::{Arc, Mutex};

    /// Poison `cell` the way a crash does: a thread panics while holding it.
    fn poison(cell: &dyn LockCell) {
        let crashed = std::thread::scope(|scope| {
            scope
                .spawn(|| cell.hold(&mut || panic!("synthetic panic (#576 A11 test)")))
                .join()
        });
        assert!(crashed.is_err(), "the helper thread panicked on purpose");
        assert!(cell.cell_poisoned());
    }

    /// A monitor whose notices are counted.
    fn counted() -> (Arc<PoisonMonitor>, Arc<Mutex<Vec<&'static str>>>) {
        let monitor = PoisonMonitor::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        assert!(monitor.install_hook(move |cell| sink.lock().unwrap().push(cell)));
        (monitor, seen)
    }

    #[test]
    fn a_poisoned_cell_recovers_the_data_the_crash_left() {
        let (monitor, seen) = counted();
        let cell = RecoveringMutex::with_monitor("numbers", vec![1, 2], &monitor);
        let crashed = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let mut held = cell.lock();
                    held.push(3);
                    panic!("crash half-way through an edit");
                })
                .join()
        });
        assert!(crashed.is_err());
        assert!(cell.is_poisoned());

        // The recovered guard sees what the crashed holder wrote, and keeps working.
        cell.lock().push(4);
        assert_eq!(*cell.lock(), vec![1, 2, 3, 4]);
        assert_eq!(*seen.lock().unwrap(), vec!["numbers"]);
        assert_eq!(monitor.first_poisoned(), Some("numbers"));
    }

    #[test]
    fn the_notice_goes_out_once_however_many_cells_and_locks() {
        let (monitor, seen) = counted();
        let a = RecoveringMutex::with_monitor("a", 0_u8, &monitor);
        let b = RecoveringMutex::with_monitor("b", 0_u8, &monitor);
        assert_eq!(monitor.registered_cells(), 2);

        // A healthy cell reports nothing.
        drop(a.lock());
        assert!(seen.lock().unwrap().is_empty());
        assert!(!monitor.raised());

        poison(&a);
        poison(&b);
        for _ in 0..3 {
            drop(a.lock());
            drop(b.lock());
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["a"],
            "one notice, naming the first"
        );
        assert!(monitor.raised());
    }

    #[test]
    fn refusal_refuses_only_a_poisoned_cell_and_reports_it() {
        let (monitor, seen) = counted();
        let cell = RecoveringMutex::with_monitor("history", (), &monitor);
        assert_eq!(cell.refuse_if_poisoned(), Ok(()));
        assert!(seen.lock().unwrap().is_empty());

        poison(&cell);
        let refused = cell.refuse_if_poisoned().unwrap_err();
        assert_eq!(refused.cell(), "history");
        assert!(refused.to_string().contains("internal error"), "{refused}");
        assert_eq!(*seen.lock().unwrap(), vec!["history"]);
    }

    #[test]
    fn a_crash_before_the_hook_is_installed_is_delivered_at_installation() {
        let monitor = PoisonMonitor::new();
        let cell = RecoveringMutex::with_monitor("early", (), &monitor);
        poison(&cell);
        drop(cell.lock());
        assert_eq!(monitor.first_poisoned(), Some("early"));
        assert!(!monitor.raised(), "nobody to tell yet");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        assert!(monitor.install_hook(move |cell| sink.lock().unwrap().push(cell)));
        assert_eq!(*seen.lock().unwrap(), vec!["early"]);
        // Only the first installation takes.
        assert!(!monitor.install_hook(|_| panic!("a second hook is never called")));
    }

    #[test]
    fn a_forwarded_monitor_shares_the_target_s_single_notice() {
        let (target, seen) = counted();
        let local = RecoveringMutex::with_monitor("project", (), &target);
        let outside_monitor = PoisonMonitor::new();
        assert!(outside_monitor.forward_to(&target));
        let outside = RecoveringMutex::with_monitor("tile_cache", (), &outside_monitor);

        poison(&outside);
        drop(outside.lock());
        poison(&local);
        drop(local.lock());
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["tile_cache"],
            "a poisoned cache and a poisoned project raise one banner"
        );
    }
}
