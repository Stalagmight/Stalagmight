use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::core::AppState;
use crate::models::survey::Project;

#[cfg(any(test, feature = "test-support"))]
use tempfile::TempDir;

/// A project written as a `.stalx` version 2 file in a temp dir.
#[cfg(any(test, feature = "test-support"))]
pub struct TestDb {
    _dir: TempDir,
    path: PathBuf,
}

#[cfg(any(test, feature = "test-support"))]
impl TestDb {
    /// Write `project` (empty history, no link) as `project.stalx`.
    pub fn with_project(project: &Project) -> Result<Self> {
        let dir = TempDir::new()?;
        let path = dir.path().join("project.stalx");
        let bytes = crate::persistence::stalx_v2::encode(
            project,
            &crate::persistence::stalx_v2::Extras::default(),
            crate::persistence::stalx_v2::Mode::Normal,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        std::fs::write(&path, bytes)?;
        Ok(Self { _dir: dir, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// An `AppState` with `project` written to a temp `.stalx` and opened from
/// it (the normal open path).
#[cfg(any(test, feature = "test-support"))]
pub fn app_state_with_project_db(project: Project) -> Result<(AppState, TestDb)> {
    let test_db = TestDb::with_project(&project)?;
    let state = AppState::new()?;
    let opened = state.execute(crate::core::Command::OpenProject {
        path: test_db.path().to_path_buf(),
    });
    if !opened.success {
        anyhow::bail!("open failed: {:?}", opened.error);
    }
    Ok((state, test_db))
}

// === Config isolation =====================================================
//
// `AppState::new()` LOADS the global config via `Config::load()` and half the
// state module writes it back (`note_recent_project` -> `Config::save()`), so
// any test that builds an `AppState` reads and writes ONE file. `app_dirs` is
// never initialized in unit tests, so unredirected that file is the developer's
// REAL `~/.config/stalagmight/config.toml`.
//
// Isolation is therefore NOT opt-in. `Config::config_path()` resolves through
// this module, and the default for a thread that never asked for anything is a
// temp dir of its own, created on first use and removed when the thread exits.
// libtest runs each `#[test]` on a thread of its own, so every test in the
// binary is isolated from the real file and from its siblings whether it says so
// or not. (The exception is real but remote: when libtest cannot spawn a thread
// it runs that test on the MAIN thread, which shares the main thread's dir with
// every other test that lands there. A test that wants a dir nothing else has
// ever written to still has to take `isolated_config`.)
//
// The opt-in helpers used to be the whole mechanism, and that cost two distinct
// hazards - the second one is #522:
//
//   * With no redirect live, an unguarded test resolved to the developer's REAL
//     `~/.config/stalagmight/config.toml` and rewrote it (its `recent_projects`
//     filling with dead `/tmp/.tmpXXXX/*.stalx` entries).
//   * The redirect was a process-global cell, so while ANY test held it an
//     unguarded test's save resolved INTO that test's isolated dir. That is
//     #522: an unguarded test atomically renamed its own `config.toml` over the
//     isolated test's file, between that test's save and its read-back, and
//     failed an assertion about credential persistence with an environment
//     problem.
//
// Per-thread resolution closes both, and needs no process-wide mutex, so
// isolated tests no longer serialize.
//
// THE ONE TRAP is a SPAWNED THREAD: it is a new thread, so by default it gets a
// dir of its own rather than the spawning test's. That is the right default -
// a worker writing recents into its own dir is what keeps it off everyone
// else's file - but a test whose workers must share ONE config file has to say
// so: [`ConfigHandoff::capture`] on the parent, [`ConfigHandoff::adopt`] inside
// the worker.
//
// [`real_config_resolution`] is the way out for the handful of tests that drive
// `config_path()` itself, by pointing XDG_CONFIG_HOME / HOME at an unwritable
// directory to force a save failure. Those guards serialize against EACH OTHER,
// because the environment they mutate is process-wide. Nothing else resolves the
// CONFIG PATH through the environment - but "nothing else reads the environment"
// would be false: `TempDir::new()` reads TMPDIR, and non-config code reads HOME
// (`directories::UserDirs`, for the default project location). Those readers were
// never serialized against these three tests and still are not; the cure is a
// config root injected rather than an environment variable mutated, which is not
// this module's to give.

#[cfg(any(test, feature = "test-support"))]
use std::cell::{OnceCell, RefCell};
#[cfg(any(test, feature = "test-support"))]
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// How the calling thread resolves `Config::config_path()`.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
enum ConfigMode {
    /// Into this directory, whatever the environment says.
    ///
    /// Shared OWNERSHIP of the dir, not just its path: a worker that adopted a
    /// [`ConfigHandoff`] keeps it alive even if the test that made it drops its
    /// guard first. Naming a removed dir would not fail the worker's save -
    /// `save_to_path` calls `create_dir_all` and would resurrect it as an orphan,
    /// so the write would "succeed" into a directory the parent has stopped
    /// reading.
    Isolated(Arc<TempDir>),
    /// For real: `app_dirs`, then `directories` over the environment.
    Real,
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// What a guard asked this thread for; `None` selects [`own_config_dir`].
    static CONFIG_MODE: RefCell<Option<ConfigMode>> = const { RefCell::new(None) };
    /// The default: this thread's own config dir, created on first use and
    /// removed when the thread exits (nothing else holds a handle to it unless a
    /// handoff was captured).
    static OWN_CONFIG_DIR: OnceCell<Arc<TempDir>> = const { OnceCell::new() };
}

/// This thread's own config dir, created on first use.
///
/// PANICS rather than reporting failure, because there is no second-best answer:
/// the only other path `Config::config_path()` can produce is the developer's
/// real `config.toml`, and every thread in the process pointed at that one file
/// is precisely the failure #522 was. A machine that cannot create a temp dir
/// cannot run these tests meaningfully, and should say so at the first config
/// touch rather than silently rewrite `~/.config`.
#[cfg(any(test, feature = "test-support"))]
#[expect(
    clippy::expect_used,
    reason = "the alternative to a temp dir is the real config file; failing loudly is the point"
)]
fn own_config_dir() -> Arc<TempDir> {
    OWN_CONFIG_DIR
        .try_with(|cell| {
            Arc::clone(cell.get_or_init(|| {
                Arc::new(TempDir::new().expect("create this thread's temp config dir"))
            }))
        })
        .unwrap_or_else(|_| teardown_config_dir())
}

/// Where a config touch lands when it arrives after the thread's own TLS has
/// been destroyed (a `Drop` running at thread exit). There is no per-thread slot
/// left to own a dir by then, so one shared dir answers for all of them; it is
/// never dropped, so it outlives the process rather than being cleaned up.
///
/// Panics for the same reason [`own_config_dir`] does. In a destructor that
/// means the process goes down loudly - which is the intended trade: no branch
/// of this module may answer with the real config file.
#[cfg(any(test, feature = "test-support"))]
#[expect(
    clippy::expect_used,
    reason = "the alternative to a temp dir is the real config file; failing loudly is the point"
)]
fn teardown_config_dir() -> Arc<TempDir> {
    static DIR: OnceLock<Arc<TempDir>> = OnceLock::new();
    Arc::clone(DIR.get_or_init(|| {
        Arc::new(TempDir::new().expect("create the shared thread-teardown config dir"))
    }))
}

/// The directory THIS THREAD resolves the config file into.
///
/// `None` means one thing and one thing only: this thread resolves for real (see
/// [`real_config_resolution`]). Every other case answers with a directory or
/// panics - the real path is never reached by accident, however the temp dir
/// behind it was obtained.
///
/// `Config::config_path()` consults this under
/// `cfg(any(test, feature = "test-support"))` - the same gate as this module
/// rather than `cfg(test)`, because the root crate's tests isolate through this
/// hook too and for THEIR builds this crate is a dependency.
///
/// `test-support` must therefore stay a DEV-dependency-only feature of the root
/// crate: enabled in a shipping build, this hook would answer for the app and it
/// would keep its config in a temp dir that dies with the process.
#[cfg(any(test, feature = "test-support"))]
pub fn current_config_dir() -> Option<PathBuf> {
    match CONFIG_MODE.try_with(|mode| mode.borrow().clone()) {
        Ok(Some(ConfigMode::Isolated(dir))) => Some(dir.path().to_path_buf()),
        Ok(Some(ConfigMode::Real)) => None,
        Ok(None) | Err(_) => Some(own_config_dir().path().to_path_buf()),
    }
}

/// Puts back whatever the thread resolved to before, so guards nest.
#[cfg(any(test, feature = "test-support"))]
struct ModeRestore(Option<ConfigMode>);

#[cfg(any(test, feature = "test-support"))]
impl ModeRestore {
    fn install(mode: ConfigMode) -> Self {
        Self(CONFIG_MODE.with(|slot| slot.replace(Some(mode))))
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for ModeRestore {
    fn drop(&mut self) {
        let previous = self.0.take();
        let _ = CONFIG_MODE.try_with(|slot| slot.replace(previous));
    }
}

/// RAII guard owning the temp dir this thread's config resolves into. Shared
/// ownership: an adopted [`ConfigHandoff`] outliving this guard keeps the dir.
#[cfg(any(test, feature = "test-support"))]
pub struct IsolatedConfig {
    // Declared first so the mode is restored BEFORE this guard lets go of the
    // dir it names.
    _restore: ModeRestore,
    dir: Arc<TempDir>,
}

#[cfg(any(test, feature = "test-support"))]
impl IsolatedConfig {
    /// The directory this guard resolves the config into.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// Redirect THIS THREAD's config path to a fresh temp directory until the
/// returned guard drops.
///
/// Every thread is already isolated (see the module header), so what this adds
/// is a dir the test knows the identity of - to seed a config file in, to squat
/// to make a save fail, or to hand to a worker thread - and one dir for the
/// whole test rather than one per thread it spawns.
///
/// # Panics
///
/// If the temp dir cannot be created, which is a broken test environment
/// rather than a failure the caller could act on.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "test-support helper; panic on setup failure is acceptable"
)]
pub fn isolated_config() -> IsolatedConfig {
    let dir = Arc::new(TempDir::new().expect("create temp config dir"));
    let restore = ModeRestore::install(ConfigMode::Isolated(Arc::clone(&dir)));
    IsolatedConfig {
        _restore: restore,
        dir,
    }
}

/// Run `f` with the config path redirected to a fresh temp dir (see
/// [`isolated_config`]).
#[cfg(any(test, feature = "test-support"))]
pub fn with_isolated_config<T>(f: impl FnOnce() -> T) -> T {
    let _cfg = isolated_config();
    f()
}

/// How the thread that built it resolves the config path, ready to be moved
/// into a thread it spawns.
///
/// Resolution is per-thread, so a worker starts out with a dir of its own. For
/// a test whose workers must share the parent's config FILE - a concurrent-save
/// race, or a worker whose write the parent reads back - capture on the parent
/// and adopt inside the worker:
///
/// ```
/// use stalagmight_core::test_helpers::ConfigHandoff;
///
/// let handoff = ConfigHandoff::capture();
/// std::thread::spawn(move || {
///     let _config = handoff.adopt();
///     // ... every Config::load()/save() here is the parent's file
/// }).join().expect("config worker should finish");
/// ```
///
/// `Clone` because a `'static` thread closure has to own its copy: capture once
/// and clone it per worker. A handoff also holds the dir OPEN for as long as it
/// exists, so a worker cannot outlive the directory it was handed.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
pub struct ConfigHandoff(ConfigMode);

#[cfg(any(test, feature = "test-support"))]
impl ConfigHandoff {
    /// Capture the calling thread's resolution, materializing its own dir if it
    /// never had one.
    #[must_use]
    pub fn capture() -> Self {
        Self(
            CONFIG_MODE
                .with(|slot| slot.borrow().clone())
                .unwrap_or_else(|| ConfigMode::Isolated(own_config_dir())),
        )
    }

    /// Resolve the captured way on THIS thread until the returned guard drops.
    /// Call it inside the spawned thread, and BIND the guard: the adoption lasts
    /// exactly as long as the value returned here.
    #[must_use]
    pub fn adopt(&self) -> AdoptedConfig {
        AdoptedConfig(ModeRestore::install(self.0.clone()))
    }
}

/// RAII guard for [`ConfigHandoff::adopt`]: gives the thread its own resolution
/// back when it drops.
#[cfg(any(test, feature = "test-support"))]
pub struct AdoptedConfig(#[expect(dead_code, reason = "held for its Drop")] ModeRestore);

#[cfg(any(test, feature = "test-support"))]
fn real_resolution_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII guard for [`real_config_resolution`].
#[cfg(any(test, feature = "test-support"))]
pub struct RealConfigResolution {
    // Declared first so this thread stops resolving for real BEFORE the next
    // real-resolution test is let in.
    _restore: ModeRestore,
    _serialized: MutexGuard<'static, ()>,
}

/// Resolve `Config::config_path()` FOR REAL on this thread - `app_dirs`, then
/// `directories` over the environment - until the returned guard drops.
///
/// For the tests that drive resolution itself, by pointing XDG_CONFIG_HOME (or
/// HOME, which is what `directories` reads on macOS) at an unwritable directory
/// to force a save failure. The environment is process-wide, so these serialize
/// against each other; nothing else resolves the CONFIG PATH through the
/// environment. What this guard does NOT do is protect the code that reads the
/// environment for other purposes - `TempDir::new()` (TMPDIR) and
/// `directories::UserDirs` (HOME) among it - which races the mutation in those
/// tests now as it did before. Recovers from a poisoned lock - the guarded value
/// is `()`, so only the serialization matters.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn real_config_resolution() -> RealConfigResolution {
    let serialized = real_resolution_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    RealConfigResolution {
        _restore: ModeRestore::install(ConfigMode::Real),
        _serialized: serialized,
    }
}

/// The seeds an ignored robustness sweep runs: `first..first + count`, where
/// `first` is read from `first_var` (default 1, the sweeps' historical
/// start). Rolling seed windows (a fuzz farm, or two machines splitting one
/// sweep) set it so each window covers new seeds instead of re-running
/// `1..=count`.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn sweep_seeds(first_var: &str, count: u32) -> std::ops::Range<u32> {
    let first: u32 = std::env::var(first_var)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    first..first.saturating_add(count)
}

/// The wall time of an ignored robustness sweep, seed by seed, so a window a
/// fuzz farm runs out of time on still says how far it got, at what rate,
/// and which seeds were slow (the farm's crash-property window 19004214
/// timed out after an hour with nothing in its log but "running 1 test": no
/// seed hung, every seed of the build took about 4 s where an earlier one
/// took 1 s, and the window had been sized for the earlier rate).
///
/// Its lines start with the sweep's name, never with `seed N:` (a failure's
/// form). Seeds over `slow_secs` get a line each; every `every` seeds a
/// progress line gives the count, the time so far and the mean.
#[cfg(any(test, feature = "test-support"))]
pub struct SweepClock {
    name: &'static str,
    slow_secs: f64,
    every: u32,
    seeds: u32,
    total_secs: f64,
    slowest: Option<(u32, f64)>,
}

#[cfg(any(test, feature = "test-support"))]
impl SweepClock {
    /// Seeds slower than this many seconds get a line of their own, unless
    /// `STALAGMIGHT_RUST_SWEEP_SLOW_SECS` says otherwise.
    pub const SLOW_SECS: f64 = 30.0;
    /// A progress line every this many seeds.
    pub const EVERY: u32 = 25;

    /// A clock for the sweep `name`, with the slow threshold from the
    /// environment.
    #[must_use]
    pub fn from_env(name: &'static str) -> Self {
        let slow_secs = std::env::var("STALAGMIGHT_RUST_SWEEP_SLOW_SECS")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|secs| secs.is_finite() && *secs >= 0.0)
            .unwrap_or(Self::SLOW_SECS);
        Self::new(name, slow_secs, Self::EVERY)
    }

    #[must_use]
    pub fn new(name: &'static str, slow_secs: f64, every: u32) -> Self {
        Self {
            name,
            slow_secs,
            every: every.max(1),
            seeds: 0,
            total_secs: 0.0,
            slowest: None,
        }
    }

    /// Record that `seed` took `secs` (`detail` says where the time went);
    /// the lines to print now.
    pub fn seed_done(&mut self, seed: u32, secs: f64, detail: &str) -> Vec<String> {
        self.seeds += 1;
        self.total_secs += secs;
        if self.slowest.is_none_or(|(_, worst)| secs > worst) {
            self.slowest = Some((seed, secs));
        }
        let mut lines = Vec::new();
        if secs > self.slow_secs {
            lines.push(format!(
                "{}: slow seed {seed} took {secs:.1} s ({detail})",
                self.name
            ));
        }
        if self.seeds.is_multiple_of(self.every) {
            lines.push(format!(
                "{}: {} seed(s) in {:.0} s, {:.2} s per seed, last {seed}",
                self.name,
                self.seeds,
                self.total_secs,
                self.mean_secs()
            ));
        }
        lines
    }

    /// The mean seconds per seed so far (0 before the first seed).
    #[must_use]
    pub fn mean_secs(&self) -> f64 {
        if self.seeds == 0 {
            0.0
        } else {
            self.total_secs / f64::from(self.seeds)
        }
    }

    /// For the sweep's last line: the time, the mean and the slowest seed.
    #[must_use]
    pub fn summary(&self) -> String {
        let slowest = self.slowest.map_or_else(
            || "none".to_string(),
            |(seed, secs)| format!("{seed} ({secs:.1} s)"),
        );
        format!(
            "{:.0} s, {:.2} s per seed, slowest seed {slowest}",
            self.total_secs,
            self.mean_secs()
        )
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::{
        current_config_dir, isolated_config, real_config_resolution, ConfigHandoff, SweepClock,
    };
    use crate::config::Config;
    use std::path::PathBuf;

    /// Save a config carrying a distinctive marker into whatever file the
    /// CALLING THREAD resolves to, and report where it landed and whether the
    /// write is really on disk. Both halves matter: a test that only compares
    /// paths would pass just as well if the save had failed.
    fn save_marked_config(marked: bool) -> (PathBuf, bool) {
        let mut config = Config::default();
        config.general.auto_save = marked;
        config.save().expect("save the config");
        let path = Config::config_path().expect("config path");
        let landed = Config::load().expect("read it back").general.auto_save == marked;
        (path, landed)
    }

    #[test]
    fn a_thread_that_never_asked_for_isolation_cannot_reach_a_test_that_did() {
        // #522 as a permanent test. The unguarded writer here is what the
        // logout tests were failed by: a save on another thread, atomically
        // renaming a config.toml of its own into place. It must be a different
        // file, or the assertion below is one scheduler away from failing.
        let cfg = isolated_config();
        let (ours, ours_landed) = save_marked_config(true);
        assert!(ours_landed, "precondition: our own save landed");
        assert!(
            ours.starts_with(cfg.path()),
            "the guard's dir is where we resolve: {} vs {}",
            ours.display(),
            cfg.path().display()
        );

        let (theirs, theirs_landed) = std::thread::spawn(|| save_marked_config(false))
            .join()
            .expect("the foreign writer did not panic");
        assert!(
            theirs_landed,
            "precondition: the foreign save landed somewhere, or this proves nothing"
        );
        assert_ne!(
            theirs, ours,
            "a thread that never asked for isolation still gets a config file of its own"
        );
        assert!(
            Config::load()
                .expect("read our config back")
                .general
                .auto_save,
            "the foreign write must not be able to land in the file this test wrote"
        );
        // And it took its directory with it: `join` returns after the worker's
        // thread-locals have been destroyed, and nothing else held a handle to
        // that dir, so it is already gone. Unix only - `TempDir`'s `Drop`
        // swallows removal errors, and on Windows a lingering handle makes the
        // removal fail, which would make this a flake rather than a finding.
        #[cfg(unix)]
        assert!(
            !theirs.exists(),
            "the per-thread dir is removed when its thread exits: {}",
            theirs.display()
        );
    }

    #[test]
    fn a_handoff_outliving_the_parent_guard_keeps_the_dir_alive() {
        // The handoff shares OWNERSHIP of the dir, not just its path. Carrying a
        // path alone, a worker that outlived the parent's guard would write into a
        // REMOVED directory that `save_to_path`'s `create_dir_all` recreates as an
        // orphan: the save "succeeds" and the parent reads its own emptiness.
        // Every call site joins its workers today, and nothing makes them.
        let (handoff, dir) = {
            let cfg = isolated_config();
            (ConfigHandoff::capture(), cfg.path().to_path_buf())
        };
        assert!(
            dir.exists(),
            "a live handoff keeps the dir its parent guard made: {}",
            dir.display()
        );

        let (path, landed) = std::thread::spawn({
            let handoff = handoff.clone();
            move || {
                let _config = handoff.adopt();
                save_marked_config(true)
            }
        })
        .join()
        .expect("the adopting writer did not panic");
        assert!(landed, "and it is still writable: {}", path.display());
        assert!(
            path.starts_with(&dir),
            "into that same dir: {}",
            path.display()
        );

        drop(handoff);
        // Unix only, for the reason the sibling test above gives.
        #[cfg(unix)]
        assert!(
            !dir.exists(),
            "and the last handle taken away removes it: {}",
            dir.display()
        );
    }

    #[test]
    fn a_worker_that_adopts_the_handoff_writes_the_parent_thread_s_file() {
        // The escape hatch from the isolation above, and the one the concurrency
        // gate and the config lost-update race depend on: without it their
        // workers would each race a file nobody else can see.
        let _cfg = isolated_config();
        let (ours, _) = save_marked_config(false);

        let handoff = ConfigHandoff::capture();
        let (theirs, theirs_landed) = std::thread::spawn(move || {
            let _config = handoff.adopt();
            save_marked_config(true)
        })
        .join()
        .expect("the adopting writer did not panic");

        assert!(theirs_landed, "precondition: the worker's save landed");
        assert_eq!(
            theirs, ours,
            "an adopted worker resolves to the parent's file"
        );
        assert!(
            Config::load()
                .expect("read our config back")
                .general
                .auto_save,
            "and the parent reads what the worker wrote"
        );
    }

    #[test]
    fn a_nested_guard_hands_the_outer_dir_back_when_it_drops() {
        let outer = isolated_config();
        {
            let inner = isolated_config();
            assert_ne!(inner.path(), outer.path(), "a fresh dir per guard");
            assert_eq!(
                current_config_dir().as_deref(),
                Some(inner.path()),
                "the innermost guard wins while it is alive"
            );
        }
        assert_eq!(
            current_config_dir().as_deref(),
            Some(outer.path()),
            "and dropping it restores the guard it replaced rather than clearing the mode"
        );
    }

    #[test]
    fn real_resolution_stops_at_the_guard() {
        // Only the PATH is resolved here - never opened - so this says nothing
        // about, and does nothing to, the developer's real config file.
        let cfg = isolated_config();
        {
            let _real = real_config_resolution();
            assert!(
                current_config_dir().is_none(),
                "under this guard the config resolves through app_dirs / the environment"
            );
        }
        assert_eq!(
            current_config_dir().as_deref(),
            Some(cfg.path()),
            "and the thread's isolation comes back afterwards"
        );
    }

    /// A sweep's clock names a slow seed when it finishes, gives progress at
    /// a fixed stride, and ends with the slowest seed: what a farm window
    /// that times out needs in its log.
    #[test]
    fn the_sweep_clock_names_slow_seeds_and_reports_progress() {
        let mut clock = SweepClock::new("crash sweep", 30.0, 3);
        assert_eq!(clock.summary(), "0 s, 0.00 s per seed, slowest seed none");
        assert!(clock.seed_done(7, 4.0, "v2 1.0 s").is_empty());
        assert_eq!(
            clock.seed_done(8, 95.5, "v2 1.0 s, upgrade 94.5 s"),
            vec!["crash sweep: slow seed 8 took 95.5 s (v2 1.0 s, upgrade 94.5 s)".to_string()]
        );
        // The third seed: a progress line, and no slow line at 30 s exactly.
        assert_eq!(
            clock.seed_done(9, 30.0, "v2 1.0 s"),
            vec!["crash sweep: 3 seed(s) in 130 s, 43.17 s per seed, last 9".to_string()]
        );
        assert_eq!(
            clock.summary(),
            "130 s, 43.17 s per seed, slowest seed 8 (95.5 s)"
        );
        // Every line starts with the sweep's name, never a failure's `seed N:`.
        let mut clock = SweepClock::new("crash sweep", 0.0, 1);
        for line in clock.seed_done(1, 1.0, "") {
            assert!(line.starts_with("crash sweep: "), "{line}");
        }
    }
}
