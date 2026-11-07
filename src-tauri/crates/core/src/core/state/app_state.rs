use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
use std::sync::Arc;

use crate::models::poison_policy::{PoisonMonitor, RecoveringMutex};

use crate::cache::TileCache;
use crate::config::{ClosureAdjustmentConfig, Config, GasConfig};
use crate::models::{
    AlongLineResult, Cylinder, DistanceField, Diver, GasReachField, PlanSegment, Project,
    RulerEndpoint, Station, StationAnchor, StationRef,
};
use crate::persistence::Database;
use crate::tile_stats::TileFetchStats;
use crate::tilemap_cache::TilemapCache;

use crate::core::{CommandError, CommandErrorCode, CommandResult, UndoStack};

use super::project_io::reapply_heals_after_restore;
use super::project_slot::ProjectSlot;

/// Trait for types that can represent "no project open" for `with_project_mut` and
/// `with_project`, so they work with different return types. (A poisoned project
/// lock is no longer an error here: it is recovered, #576 A11.)
pub(super) trait FromProjectLockError {
    fn no_project() -> Self;
}

impl FromProjectLockError for CommandResult {
    fn no_project() -> Self {
        CommandResult::failure("No project open".to_string())
    }
}

impl<T> FromProjectLockError for Result<T, String> {
    fn no_project() -> Self {
        Err("No project open".to_string())
    }
}

/// Transient, never-persisted cache of the closure-adjusted station coordinates
/// for the current project. Keyed on the project's `modified_at` (nanoseconds)
/// plus a hash of the closure-adjustment config, and cleared on every
/// command/undo/redo so the adjusted display view always re-derives from
/// current measured data.
pub(super) struct AdjustedStationsCache {
    pub(super) version: i64,
    pub(super) config_hash: u64,
    pub(super) stations: Vec<Station>,
}

/// Hash the closure-adjustment config fields that affect the solve, for the
/// adjusted-stations cache key. Shared by `get_adjusted_stations` and
/// `distance_field` so both derive the same key.
pub(super) fn adjusted_config_hash(ca: &ClosureAdjustmentConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ca.base_sigma_length_m.to_bits().hash(&mut h);
    ca.base_sigma_azimuth_deg.to_bits().hash(&mut h);
    ca.base_sigma_depth_m.to_bits().hash(&mut h);
    h.finish()
}

/// [#576 RV4-R-NITS C-4] What a parked `execute` signals on and waits for (see
/// `AppState::park_after_mutation`).
#[cfg(test)]
pub(super) struct ParkAfterMutation {
    /// Sent once the command has mutated and before it pushes its undo entry.
    pub(super) reached: std::sync::mpsc::Sender<()>,
    /// Received before it goes on.
    pub(super) resume: std::sync::mpsc::Receiver<()>,
}

/// Application state managed by Tauri
pub struct AppState {
    /// Current project (if any), copy-on-write (#576 A9): readers take an O(1)
    /// snapshot, a writer copies only while a snapshot is alive. See [`ProjectSlot`].
    pub(super) project: RecoveringMutex<ProjectSlot>,
    /// Database connection (if project is open)
    pub(super) database: RecoveringMutex<Option<Database>>,
    /// True while the open project's initial save is still deferred - created with
    /// no save path and never written to disk, so it has no `.stalx` and no Recent
    /// Projects entry yet. Tracked explicitly rather than inferred from
    /// `database.is_none()` (ambiguous for dropped handles) or file existence
    /// (ambiguous for reserved-path collisions), so the first import can durably
    /// persist it exactly once (#92 / PR #104).
    pub(super) initial_save_deferred: AtomicBool,
    /// Undo/redo stack
    pub(super) undo_stack: RecoveringMutex<UndoStack>,
    /// Application configuration
    pub(super) config: RecoveringMutex<Config>,
    /// Serialization lock for config READ-MODIFY-WRITE, held across all three
    /// steps. `config` above is only ever held for the read or the write, never
    /// both, so it cannot serialize a partial update: every partial-update command
    /// (`update_data_table_view`, `update_panel_sizes`, `update_panel_layout`)
    /// clones the whole `Config`, edits its own fields and writes the whole thing
    /// back. Two of them interleaving means A reads v1, B reads v1, A writes v1+A,
    /// B writes v1+B - and A is gone, from memory and from disk.
    ///
    /// That is reachable without contrivance: `savePanelSizes` (`store/uiStore.ts`)
    /// fires undebounced when a panel divider is released while the data-table view
    /// save fires 500 ms after the last change, and both live in `SingleSystemView`.
    ///
    /// NOT `command_lock`: that one serializes shot/project mutations and undo, and
    /// widening it to cover config would put a settings save behind an in-flight
    /// batch command for no benefit. Take it via [`AppState::modify_config`] rather
    /// than by hand - `update_config` takes it too, so a nested acquisition
    /// deadlocks.
    ///
    /// The recent-projects writers take it as well
    /// (`AppState::persist_recent_projects`, #576 RV4-R-M2), with `recent_projects`
    /// and possibly `project` already held: a holder of this lock must therefore
    /// never take `recent_projects`, `project` or `command_lock`.
    pub(super) config_write_lock: RecoveringMutex<()>,
    /// Recent projects list
    pub(super) recent_projects: RecoveringMutex<Vec<PathBuf>>,
    /// SpeleoDB per-session connection state (endpoint, email, token store).
    /// Read-only spike (#183); the token never crosses into the webview.
    pub(super) speleodb: RecoveringMutex<super::speleodb::SpeleoDbSession>,
    /// Whether the open project may be edited. TRANSIENT, never persisted,
    /// DEFAULT-TRUE; `false` ONLY for a remote SpeleoDB project opened without a
    /// write lock (#191). Reset to `true` on every open/close so a previously
    /// read-only project can't leave the flag stuck. The mutation chokepoint
    /// (`guard_mutation`) reads it.
    pub(super) edit_enabled: AtomicBool,
    /// Whether the open remote project is being edited WITHOUT the server write
    /// lock (optimistic force-edit, #213). TRANSIENT, never persisted, DEFAULT
    /// FALSE; `true` ONLY between `speleodb_force_edit` and the next reconnect
    /// (a successful `speleodb_start_editing`) / close. When true, `edit_enabled`
    /// is also true but NO lock is held, so `edit_enabled` alone must NOT be read
    /// as "we hold the lock": every lock-release / re-verify path checks this too.
    pub(super) force_editing: AtomicBool,
    /// In-memory display settings cache (used when no DB is open, e.g., new unsaved project)
    pub(super) display_settings_cache: RecoveringMutex<HashMap<String, String>>,
    /// [#576 NF-W8FE-3] The file the open project was committed from (its
    /// `file_path` at the project-slot commit; `None` for an unsaved project or no
    /// project). Written by every commit. Part of the identity a display-setting
    /// write is checked against.
    ///
    /// [#576 RV10b-4] Followed by every file a save has moved the open project off
    /// since that commit (`note_open_project_moved`), so it names each file this
    /// session's project has been at; the next commit starts it again.
    pub(super) opened_from: RecoveringMutex<Vec<Option<PathBuf>>>,
    /// Serialization lock for command execution.
    /// Held for the entire duration of execute(), undo(), and redo() to prevent
    /// concurrent commands from interleaving (especially with batch sub-commands).
    pub(super) command_lock: RecoveringMutex<()>,
    /// HTTP cache for map tiles, held strictly under the provider's own
    /// `Cache-Control` - imagery is never stored for offline use.
    ///
    /// Note: Uses Arc<Mutex<...>> rather than plain Mutex<...> because the protocol handler
    /// runs in a separate async task/thread that outlives the closure scope, requiring
    /// owned/cloneable access to the cache. This is a deliberate deviation from the spec
    /// for thread-safety requirements.
    ///
    /// A CacheConfig change (a settings save or Reload Config) reaches this cache at
    /// once through `sync_config_side_effects` (#576 A18(g)/(h)): it is opened,
    /// closed, or reconfigured in place.
    pub(super) tile_cache: Arc<RecoveringMutex<Option<TileCache>>>,
    /// In-memory cache for ArcGIS tilemap API responses.
    /// Used to efficiently check tile availability before fetching.
    pub(super) tilemap_cache: Arc<RecoveringMutex<TilemapCache>>,
    /// Lock-free tile fetch statistics (in-flight count, latency, errors).
    /// Uses Arc because the protocol handler needs a cloneable reference.
    pub(super) tile_fetch_stats: Arc<TileFetchStats>,
    /// Sink for emitting events to the host application (set during app setup)
    pub(super) event_sink: RecoveringMutex<Option<Arc<dyn crate::core::events::EventSink>>>,
    /// Active loading cancellation token (if any load operation is in progress)
    pub(super) active_loading: RecoveringMutex<Option<Arc<AtomicBool>>>,
    /// Shots a load found carrying a `kind` byte this build does not recognise
    /// (#27), tagged with the `loading_id` of the load that found them. Never
    /// persisted - the fix is the user's answer, applied as an ordinary
    /// `SetShotKind`.
    ///
    /// The id is load-bearing, not bookkeeping. Two loads can overlap: switching
    /// projects cancels the first, but a cancelled load keeps running until its
    /// next checkpoint, so it can write here AFTER the new project already did.
    /// An unversioned slot would then hand project B the section and shot ids
    /// belonging to project A, and applying that resolution would write kinds
    /// onto whatever shots in B happen to share those ids.
    pub(super) unrecognised_shot_kinds:
        RecoveringMutex<Option<(u64, Vec<crate::models::UnrecognisedShotKind>)>>,
    /// [#473] True when the OPEN project's persisted undo/redo history could not be
    /// read and was replaced by an empty stack.
    ///
    /// The load paths already degrade gracefully - a corrupt, truncated or hostile
    /// history table logs and continues with `Vec::new()`, because opening the project
    /// without its history beats not opening it. What they did NOT do is tell anyone:
    /// the user's entire undo AND redo history vanished behind a `tracing::warn!` in a
    /// file they will never read, which docs/notifications.md names as a "silent state
    /// change" and forbids. This flag is what the banner in `App.tsx` renders from.
    ///
    /// Written ONLY inside the load's non-cancellable finalize section, under the same
    /// `project` mutex acquisition that commits the project it describes. That is what
    /// keeps it honest under overlapping loads: a cancelled load never reaches finalize,
    /// so it cannot leave its verdict on the project the user actually opened. Every
    /// commit writes it, so a clean load clears a previous project's flag by assignment
    /// rather than by anyone remembering to reset it.
    ///
    /// ONE flag for TWO stacks, deliberately, and the banner copy is written to match.
    /// The undo and redo loads fail independently, so `true` means "at least one stack
    /// could not be read" - it cannot say which. That is not hypothetical: both stacks
    /// now share a single inflation budget, so a large-but-legitimate undo history can
    /// leave the redo pre-scan nothing to spend. Copy that named both stacks was
    /// therefore false in exactly that case, and now says only what this flag knows.
    /// Splitting it in two is the change to make if the copy ever needs to be specific.
    ///
    /// Transient, never persisted: it describes THIS read of the file, and the next save
    /// overwrites the unreadable rows anyway.
    pub(super) history_discarded: AtomicBool,
    /// [#576 B6-D1] The schema version stamped on the open project's file when a
    /// NEWER Stalagmight saved it; 0 when it did not (every real version is >= 1).
    /// Such a project is open READ-ONLY: `guard_mutation` refuses every edit and
    /// `save_project` every save, so this build can never rewrite the file at its
    /// own, older schema and silently drop what the newer one stored.
    ///
    /// Transient, never persisted, and deliberately separate from `edit_enabled`:
    /// that flag belongs to the SpeleoDB lock flows, which open it on a lock
    /// acquire - a newer file must stay closed whatever the lock says. Written on
    /// every project-slot commit (both loads, both creates, close), like
    /// `history_discarded`.
    pub(super) newer_file_schema: AtomicI32,
    /// [#576 C5] The workspace mode the frontend last reported. Plan and Export
    /// are read-only: `guard_mutation` refuses every guarded edit while either is
    /// active. Transient, never persisted, reset to Survey on every project-slot
    /// commit (both loads, both creates, close) beside `newer_file_schema`. See
    /// `workspace_mode.rs`.
    pub(super) workspace_mode: super::workspace_mode::WorkspaceModeCell,
    /// [#576 B15] The stored per-project settings (`project_settings` keys) the
    /// open project's load could not read. The project opened with defaults in
    /// their place, so `save_project` refuses every save while this is non-empty:
    /// a save writes these settings back from the model, and would put the
    /// defaults over the stored values. Transient, never persisted; written on
    /// every project-slot commit (both loads, both creates, close), like
    /// `newer_file_schema`.
    pub(super) unreadable_project_settings: RecoveringMutex<Vec<String>>,
    /// Memoized closure-adjusted station coordinates for the display-mode toggle.
    /// See [`AdjustedStationsCache`]. Transient; never persisted.
    pub(super) adjusted_cache: RecoveringMutex<Option<AdjustedStationsCache>>,
    /// Bumped on every adjusted-cache invalidation. Captured before the lock-free
    /// solve so a result computed against a since-invalidated project is not
    /// written back (covers mutations that don't bump `modified_at`, e.g. repair).
    pub(super) adjusted_cache_generation: AtomicU64,
    /// The SpeleoDB SESSION generation: bumped on every session boundary - a
    /// project committed into (or cleared from) the `project` slot, and logout -
    /// and ONLY while holding the `project` mutex. Captured by Start Editing at
    /// entry and re-checked under the same mutex before its commit opens the edit
    /// gate, so a teardown landing inside its network window refuses the commit
    /// instead of opening a gate onto a torn-down world (F7). The same idiom as
    /// [`Self::adjusted_cache_generation`] / `unrecognised_shot_kinds`, applied to
    /// the one slot that never had it. See `core::state::speleodb_session`.
    pub(super) speleodb_session_generation: AtomicU64,
    /// [#576 RV4-R-S1] The session generation, plus one, in which a Done finished
    /// editing HERE while the account's lock stayed held elsewhere
    /// (`DoneResult::LockHeldElsewhere`); 0 for none. One generation is one open
    /// project, so the generation names the project too. The status poll's
    /// inherited-lock reopen honours it: the poll cannot otherwise tell "finished
    /// here by Done" from "reopened after a restart", and switched editing back on
    /// under the other session's lock. Lapses at the next session boundary (a new
    /// open, a close, a logout bump the generation); an explicit Start editing
    /// clears it. Transient, never persisted; an atomic, so it is no lock cell.
    pub(super) speleodb_finished_here: AtomicU64,
    /// [#576 NF-W4C-3-EDIT] The session generation, plus one, an Ariane adoption
    /// shut the edit gate in, from its local span until its hand-over push has
    /// landed (0: not shut): the adopted project is open (and reopened editable,
    /// before its link was stamped) through the hand-over, and an edit there could
    /// ride the push or be lost by the adoption's final close. Read by
    /// `guard_metadata_mutation`; lapses at the next session boundary and is
    /// cleared when the adoption returns. Transient, never persisted; an atomic,
    /// so it is no lock cell.
    pub(super) speleodb_adoption_gate: AtomicU64,
    /// [#576 RV4-R-NITS C-4] Test seam: park the next `execute` once, between its
    /// mutation and its undo push, so a test can run a slot commit in exactly that
    /// window (`lifecycle_park_tests.rs`).
    #[cfg(test)]
    pub(super) park_after_mutation: RecoveringMutex<Option<ParkAfterMutation>>,
    /// Serializes SpeleoDB lock-acquisition spans against lock releases that run
    /// detached from the session that captured them (the delete's captured
    /// release, the stranded-claim janitor). Start Editing holds this across
    /// acquire -> commit, so a captured release either completes before a new
    /// acquire begins or observes the successor's claim and stands down - the F8
    /// "stale release frees the successor's mutex" interleaving cannot fit in
    /// between. ORDER: this mutex is OUTERMOST - it is held across network calls,
    /// and code holding `project`/`database`/`undo_stack` must never block on it
    /// (decide under `project`, drop it, then take this). See
    /// `core::state::speleodb_session`.
    pub(super) speleodb_lock_ops: RecoveringMutex<()>,
    /// [#576 A11] The poison policy's once-per-session notice for every lock cell
    /// above: a cell found poisoned (an earlier panic unwound through its guard)
    /// is recovered and reported here, and the first report raises the frontend's
    /// "internal error - save your work and restart" banner through the event
    /// sink. See `models::poison_policy` and [`AppState::refuse_after_crash`].
    pub(super) poison: Arc<PoisonMonitor>,
}

impl AppState {
    /// Create a new application state
    pub fn new() -> Result<Self> {
        let config = Config::load().unwrap_or_default();
        // Load recent projects from config
        let recent = config.general.recent_projects.clone();
        let undo_limit = config.general.undo_history_limit;

        // Initialize tile cache if enabled
        let tile_cache = if config.cache.tile_cache_enabled {
            match Self::init_tile_cache(&config) {
                Ok(cache) => Some(cache),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        purpose = "enable tile caching at startup",
                        "Failed to initialize tile cache; every tile will be re-fetched"
                    );
                    None
                }
            }
        } else {
            tracing::info!(
                purpose = "respect user cache settings",
                "Tile cache disabled in configuration"
            );
            None
        };

        // [#576 A11] Every lock cell below reports to this state's own monitor, so a
        // test's poisoned cell raises that test's banner and nobody else's.
        let poison = PoisonMonitor::new();
        Ok(Self {
            project: RecoveringMutex::with_monitor("project", ProjectSlot::default(), &poison),
            database: RecoveringMutex::with_monitor("database", None, &poison),
            initial_save_deferred: AtomicBool::new(false),
            undo_stack: RecoveringMutex::with_monitor(
                "undo_stack",
                UndoStack::new_with_limit(undo_limit),
                &poison,
            ),
            config: RecoveringMutex::with_monitor("config", config, &poison),
            config_write_lock: RecoveringMutex::with_monitor("config_write_lock", (), &poison),
            recent_projects: RecoveringMutex::with_monitor("recent_projects", recent, &poison),
            speleodb: RecoveringMutex::with_monitor(
                "speleodb",
                super::speleodb::SpeleoDbSession::default(),
                &poison,
            ),
            // Default-true: editing is enabled unless a remote project is opened
            // read-only (#191). Reset on open/close.
            edit_enabled: AtomicBool::new(true),
            // Default-false: force-edit (edit without the lock, #213) is opt-in and
            // per-session. Reset on open/close alongside edit_enabled.
            force_editing: AtomicBool::new(false),
            display_settings_cache: RecoveringMutex::with_monitor(
                "display_settings_cache",
                HashMap::new(),
                &poison,
            ),
            opened_from: RecoveringMutex::with_monitor("opened_from", Vec::new(), &poison),
            command_lock: RecoveringMutex::with_monitor("command_lock", (), &poison),
            tile_cache: Arc::new(RecoveringMutex::with_monitor(
                "tile_cache",
                tile_cache,
                &poison,
            )),
            tilemap_cache: Arc::new(RecoveringMutex::with_monitor(
                "tilemap_cache",
                TilemapCache::new(),
                &poison,
            )),
            tile_fetch_stats: Arc::new(TileFetchStats::new()),
            event_sink: RecoveringMutex::with_monitor("event_sink", None, &poison),
            active_loading: RecoveringMutex::with_monitor("active_loading", None, &poison),
            unrecognised_shot_kinds: RecoveringMutex::with_monitor(
                "unrecognised_shot_kinds",
                None,
                &poison,
            ),
            // Default-false: no project is open, so no history has been discarded.
            history_discarded: AtomicBool::new(false),
            // Default 0 (none): no project is open, so none came from a newer build.
            newer_file_schema: AtomicI32::new(0),
            // Survey: no project is open, and every project opens in Survey.
            workspace_mode: super::workspace_mode::WorkspaceModeCell::default(),
            // Empty: no project is open, so no stored setting went unread.
            unreadable_project_settings: RecoveringMutex::with_monitor(
                "unreadable_project_settings",
                Vec::new(),
                &poison,
            ),
            adjusted_cache: RecoveringMutex::with_monitor("adjusted_cache", None, &poison),
            adjusted_cache_generation: AtomicU64::new(0),
            speleodb_session_generation: AtomicU64::new(0),
            speleodb_finished_here: AtomicU64::new(0),
            speleodb_adoption_gate: AtomicU64::new(0),
            #[cfg(test)]
            park_after_mutation: RecoveringMutex::with_monitor(
                "park_after_mutation",
                None,
                &poison,
            ),
            speleodb_lock_ops: RecoveringMutex::with_monitor("speleodb_lock_ops", (), &poison),
            poison,
        })
    }

    /// Initialize the tile cache based on config
    pub(super) fn init_tile_cache(config: &Config) -> Result<TileCache> {
        let cache_dir = crate::app_dirs::cache_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine app cache directory"))?;
        let mut cache = TileCache::open(
            cache_dir,
            config.cache.tile_cache_max_mb,
            config.cache.tile_ttl_days,
        )?;
        cache.set_provider_ttls(&config.cache.provider_ttl_days());
        // After the overrides, never before - see `purge_retired`. A failure
        // here is housekeeping, not a reason to run without a cache.
        if let Err(e) = cache.purge_retired() {
            tracing::warn!(error = %e, "Failed to drop tiles past retention at startup");
        }
        Ok(cache)
    }

    /// Get a clone of the tile cache Arc for sharing across threads
    pub fn tile_cache(&self) -> Arc<RecoveringMutex<Option<TileCache>>> {
        Arc::clone(&self.tile_cache)
    }

    /// Get a clone of the tilemap cache Arc for sharing across threads
    pub fn tilemap_cache(&self) -> Arc<RecoveringMutex<TilemapCache>> {
        Arc::clone(&self.tilemap_cache)
    }

    /// Get a clone of the tile fetch stats Arc for sharing across threads
    pub fn tile_fetch_stats(&self) -> Arc<TileFetchStats> {
        Arc::clone(&self.tile_fetch_stats)
    }

    /// Set the event sink (called during app setup).
    ///
    /// [#576 A11] Also where this state's poison notice learns how to leave: the
    /// first sink installed carries the one `internal-error` event (a crash that
    /// poisoned a lock before now is delivered at once). A later sink replaces the
    /// one events go to, but not the notice's - it fires once per session anyway.
    pub fn set_event_sink(&self, sink: Arc<dyn crate::core::events::EventSink>) {
        use crate::core::events::{self, InternalErrorEvent, InternalErrorNotice, IpcEvent};
        let notice_sink = Arc::clone(&sink);
        self.poison.install_hook(move |cell| {
            let notice = InternalErrorNotice {
                cell: cell.to_owned(),
            };
            match events::payload::<InternalErrorEvent>(&notice) {
                Ok(payload) => notice_sink.emit(InternalErrorEvent::NAME, payload),
                Err(e) => tracing::error!(
                    error = %e,
                    cell,
                    purpose = "tell the user an internal error happened (#576 A11)",
                    "Failed to serialize the internal-error notice"
                ),
            }
        });
        *self.event_sink.lock() = Some(sink);
    }

    /// Get the event sink
    pub fn get_event_sink(&self) -> Option<Arc<dyn crate::core::events::EventSink>> {
        self.event_sink.lock().clone()
    }

    /// [#576 A11] The monitor behind this state's lock cells, so the app can route
    /// the process-wide cells' notice into the same single banner (see
    /// [`PoisonMonitor::forward_to`]).
    pub fn poison_monitor(&self) -> &Arc<PoisonMonitor> {
        &self.poison
    }

    /// [#576 RV5-PI-3] The first lock a crash poisoned this session - the notice
    /// the one-shot `internal-error` event carried - or `None`. The frontend's pull
    /// path for a banner whose event it missed (a crash during startup, before its
    /// listener existed, or a webview reload after the event).
    pub fn internal_error_status(&self) -> Option<crate::core::events::InternalErrorNotice> {
        self.poison
            .first_poisoned()
            .map(|cell| crate::core::events::InternalErrorNotice {
                cell: cell.to_owned(),
            })
    }

    /// Start a new loading operation, cancelling any existing one.
    /// Returns the cancellation token for the new operation.
    pub fn start_new_load(&self) -> Arc<AtomicBool> {
        let mut guard = self.active_loading.lock();

        // Cancel any existing load
        if let Some(old_token) = guard.take() {
            old_token.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        // Create new token
        let token = Arc::new(AtomicBool::new(false));
        *guard = Some(Arc::clone(&token));
        token
    }

    /// Clear the active loading token (called when load completes or fails).
    /// Identity-safe: only clears if the provided token matches the current active token.
    /// This prevents a stale load from clearing a newer load's token.
    pub fn clear_active_load(&self, expected_token: &Arc<AtomicBool>) {
        {
            let mut guard = self.active_loading.lock();
            if let Some(ref current) = *guard {
                if Arc::ptr_eq(current, expected_token) {
                    *guard = None;
                }
            }
        }
    }

    // === Read-only edit gate + dirty bit (#191) ===

    /// Whether the open project may currently be edited (transient; #191).
    pub(super) fn edit_enabled(&self) -> bool {
        self.edit_enabled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set the transient edit-enabled flag. `false` only for a remote SpeleoDB
    /// project opened without a write lock; reset to `true` on every open/close.
    pub(crate) fn set_edit_enabled(&self, v: bool) {
        self.edit_enabled
            .store(v, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the open remote project is being force-edited without the write lock
    /// (#213). When `true`, `edit_enabled` is also `true` but no lock is held, so a
    /// caller that needs "we hold the lock" must AND this out (`edit_enabled &&
    /// !force_editing`).
    pub(super) fn force_editing(&self) -> bool {
        self.force_editing
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set the transient force-edit flag (#213). `true` only between
    /// `speleodb_force_edit` and the next reconnect/close; reset to `false` on every
    /// open/close alongside `edit_enabled`.
    pub(crate) fn set_force_editing(&self, v: bool) {
        self.force_editing
            .store(v, std::sync::atomic::Ordering::Relaxed);
    }

    /// THE mutation chokepoint (#191, D-B6). Every SURVEY-CONTENT mutation entry
    /// (`execute` + all its `execute_*` siblings, `undo`/`redo`/`undo_to`/`redo_to`;
    /// `set_project_name_for_path` and `update_project_settings` through
    /// [`Self::guard_metadata_mutation`]) calls this FIRST, after
    /// taking `command_lock` and before mutating. `Err` aborts the mutation when
    /// the open project is a remote SpeleoDB project opened read-only. No-op (`Ok`)
    /// for local-only (default-true `edit_enabled`) projects and non-mutating reads.
    ///
    /// Non-survey project-DB writes are handled deliberately (the exemption the
    /// push/release flow #197 had to revisit):
    /// - `resolve_warning`/`unresolve_warning` ARE gated as of #197 (via
    ///   `save_resolved_warning_gated`/`delete_resolved_warning_gated`): a resolved
    ///   warning PROLIFERATES to the server (it is NOT stripped from the push
    ///   snapshot), so it is a real remote-dirtying edit - gate it AND
    ///   `mark_remote_dirty` so a read-only project rejects it and an edit-session
    ///   resolve is pushed by `Done` rather than silently dropped on the next pull.
    /// - `save_display_setting` (view prefs) stays UNGATED and that is correct:
    ///   `display_settings` is stripped from the push snapshot and re-applied locally
    ///   across pull/FF - purely local view state that never proliferates, never lost.
    /// - `track_imported_file`/`track_section_hashes` import-dedup bookkeeping is
    ///   reached only from inside an import command, which gates + dirties; there
    ///   is no standalone IPC command for it (#576 NF-TRACK-IMPORTED removed the
    ///   unused `track_imported_file` one).
    ///
    /// `MUTATION_ENTRYPOINTS` (tests.rs) enumerates the gated set (incl. the two
    /// warning verbs).
    ///
    /// [#576 B6-D1] A project saved by a NEWER Stalagmight is refused FIRST, ahead
    /// of the lock: it is read-only whatever the SpeleoDB lock state says, and the
    /// reason the user is told must be the version, not the lock. Its own edit-gate
    /// code (`NEWER_FILE_EDIT_DISABLED`, [`CommandErrorCode::is_edit_gate`]), so
    /// every caller that already treats a gated refusal as "read-only, not a
    /// failure" keeps doing so, and the frontend's notice can say which read-only
    /// this is without trusting its own copy of the flag.
    ///
    /// [#576 C5] Then, while the frontend's workspace is Plan or Export, EVERY
    /// caller is refused (`WORKSPACE_READ_ONLY`, also an edit-gate code): deny by
    /// default, so a new entry point is read-only in those modes without anyone
    /// remembering it. The two deliberate exceptions call
    /// [`Self::guard_metadata_mutation`] instead.
    ///
    /// [#576 C4] The refusal is a [`CommandError`]: the code says which gate, the
    /// message only what the user reads. A `CommandResult` producer returns it
    /// through [`CommandResult::refused`].
    pub(super) fn guard_mutation(&self, what: &str) -> Result<(), CommandError> {
        self.guard_metadata_mutation(what)?;
        self.guard_workspace(what)
    }

    /// [#576 A11] The REFUSING half of the poison policy, and the complete list of
    /// where it applies. Everywhere else a poisoned lock is recovered (see
    /// `models::poison_policy`): the user is told by the banner, and saving keeps
    /// working so no unsaved work is trapped.
    ///
    /// A crash while a command held `command_lock`, `project` or `undo_stack` can
    /// leave the survey half-way through an edit, and the history out of step with
    /// it (the edit applied, its undo step never recorded - or the reverse). Edits
    /// and saves of that state are allowed: it is what the user has, and they are
    /// told to save it and restart. What is refused is anything that REPLAYS
    /// history onto it or PUBLISHES it:
    ///
    /// 1. Undo, Redo, Undo to, Redo to (`undo.rs`): each applies an inverse captured
    ///    against the state BEFORE the crash, onto the state the crash left. With a
    ///    step missing or half-applied, that inverse lands on the wrong data and
    ///    compounds the damage instead of reverting it.
    /// 2. The SpeleoDB undo-chain walk (`speleodb_guard_unwind`): it is the same
    ///    traversal carried across server revisions, gated on the stack being walked
    ///    to its end - which a poisoned stack cannot vouch for. It used to fail closed
    ///    for this reason already; it now says why.
    /// 3. The SpeleoDB upload (`speleodb_upload`, behind Done, Push and the first
    ///    publish): it ships the survey AND its undo history to a server that other
    ///    people pull from. A half-written edit there becomes everyone's. The local
    ///    save stays available, and after a restart the reopened file passes the
    ///    load-time integrity checks before it can be pushed.
    /// 4. Persisting the history (`persist_or_clear_undo_stacks_into`, #576
    ///    RV5-PI-2): the save writes the survey with an EMPTY Undo/Redo history and
    ///    the `UNDO_HISTORY_DROPPED` warning. The restarted session has nothing
    ///    poisoned, so a saved history would be replayed there.
    ///
    /// Takes no lock (the caller may hold `command_lock`); a poisoned cell found
    /// here is reported like any other, so the banner explains the refusal. The
    /// refusal's code is `REFUSED_AFTER_INTERNAL_ERROR` (#576 C4).
    pub(super) fn refuse_after_crash(&self, what: &str) -> Result<(), CommandError> {
        for cell in [
            self.command_lock.refuse_if_poisoned(),
            self.project.refuse_if_poisoned(),
            self.undo_stack.refuse_if_poisoned(),
        ] {
            if let Err(refused) = cell {
                tracing::warn!(
                    what,
                    reason = %refused,
                    purpose = "never replay or publish half-written data (#576 A11)",
                    "Refused after an internal error"
                );
                return Err(CommandError::new(
                    CommandErrorCode::RefusedAfterInternalError,
                    format!(
                        "{what} is unavailable after an internal error. Save your work and \
                         restart Stalagmight."
                    ),
                ));
            }
        }
        Ok(())
    }

    /// [#576 C5] `guard_mutation` WITHOUT the read-only-workspace check: the
    /// newer-file and SpeleoDB gates only. The allowlist of what may be edited in
    /// Plan and Export, and it is exactly two callers - keep it that short:
    /// - `update_project_settings` (project name + validation thresholds; the
    ///   default north reference that rides the same save is a survey edit and
    ///   is checked with `guard_workspace` there, #576 NF-W3-2-PLAN) and
    /// - `set_project_name_for_path` (project rename),
    ///
    /// which ProjectSettingsDialog deliberately keeps enabled in every mode
    /// (docs/app-modes.md). Everything else that mutates calls `guard_mutation`.
    pub(super) fn guard_metadata_mutation(&self, what: &str) -> Result<(), CommandError> {
        if let Some(version) = self.newer_file_schema() {
            return Err(newer_file_edit_refusal(what, version));
        }
        // [#576 NF-W4C-3-EDIT] An Ariane adoption is handing this project over to
        // SpeleoDB: no edit until its push has landed. `ADOPTION_IN_PROGRESS` is in
        // the edit-gate family (`is_edit_gate`), so every caller treats it as
        // read-only, and the frontend's notice says "being handed over" rather than
        // inviting a Start editing that would race the adoption's lock operations
        // (RV5-F4-5).
        if self.speleodb_adoption_holds_gate() {
            return Err(CommandError::new(
                CommandErrorCode::AdoptionInProgress,
                format!(
                    "{what} is disabled: this project is being handed over to SpeleoDB. \
                     Edit it once the upload has finished."
                ),
            ));
        }
        if self.edit_enabled() {
            return Ok(());
        }
        Err(CommandError::new(
            CommandErrorCode::EditDisabled,
            format!(
                "{what} is disabled: this SpeleoDB project is open read-only. \
                 Acquire the lock to edit."
            ),
        ))
    }

    /// Flip `has_unpushed_edits` `false → true` AND refresh `last_edit_at` on a
    /// successful edit of a remote project (#191, D-B16; v10 last_edit_at). No-op
    /// for local-only (`load_speleo_link → None`). The dirty bit is SET-ONLY in
    /// #191 - local save/autosave NEVER clears it; only a successful server push
    /// (Sync/Release) does. `last_edit_at` is refreshed on EVERY accepted
    /// mutation (not idempotent) because guardrail-7 compares it to push-start to
    /// decide whether a landed-by-ancestry push may clear the dirty bit.
    pub(super) fn mark_remote_dirty(&self) {
        let db_guard = self.database.lock();
        let Some(db) = db_guard.as_ref() else {
            return; // no open database (e.g. unsaved project) - nothing to stamp
        };
        // One targeted UPDATE: set the dirty bit + refresh last_edit_at (the g7
        // push-start comparison needs the LATEST edit time, so this runs every edit,
        // not idempotently). A local-only project has no link row → 0 rows → no-op,
        // so this stays cheap on the hot edit path (no SELECT, no DDL, no full upsert).
        let now = chrono::Utc::now().to_rfc3339();
        if let Err(e) = db.touch_speleo_link_dirty(&now) {
            tracing::warn!(
                error = %e,
                purpose = "flip SpeleoDB dirty bit after a remote edit",
                "mark_remote_dirty: failed to persist has_unpushed_edits"
            );
        }
    }

    /// Record a successful forward mutation of the open project.
    ///
    /// A forward edit both dirties the remote link and invalidates the current Unwind
    /// position. Keep the two operations behind one verb so a new direct command path
    /// cannot leave real work classified as disposable chain state.
    pub(super) fn mark_remote_forward_edit(&self) {
        self.mark_remote_dirty();
        self.speleodb_forget_unwind_position();
    }

    /// [#576 D22] The open project's SpeleoDB link bits - the dirty bit and the
    /// Unwind pointer, exactly the two `speleodb_link_status` reports - read from
    /// local state only (no network, no lock held across the two reads).
    ///
    /// The IPC layer attaches this to every survey mutation's answer
    /// (`pool::run_mutation`), so the frontend learns what [`Self::mark_remote_dirty`]
    /// / [`Self::mark_remote_forward_edit`] did from the backend itself instead of
    /// re-deriving it at every editing call site. `None` for a local project (no
    /// link row) and when the link cannot be read - the frontend then leaves its
    /// state alone, and the next status poll reports what is known.
    pub fn remote_link_bits(&self) -> Option<crate::core::outcome::RemoteLinkBits> {
        let link = self.speleodb_open_link().ok()?;
        Some(crate::core::outcome::RemoteLinkBits {
            has_unpushed_edits: link.has_unpushed_edits,
            unwound: self.speleodb_unwound_from().is_some(),
        })
    }

    /// Cancel any active loading operation (idempotent - safe to call anytime)
    pub fn cancel_loading(&self) {
        {
            let guard = self.active_loading.lock();
            if let Some(token) = guard.as_ref() {
                token.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        // No error if no active load - cancel_loading is idempotent
    }

    /// Lock the project mutex and pass a mutable reference to the callback.
    /// Returns the callback's result, or an error if the lock fails or no project is open.
    pub(super) fn with_project_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Project) -> R,
        R: FromProjectLockError,
    {
        let mut guard = self.project.lock();
        let Some(project) = guard.as_mut() else {
            return R::no_project();
        };
        f(project)
    }

    /// Lock the project mutex and pass an immutable reference to the callback.
    /// Returns the callback's result, or an error if the lock fails or no project is open.
    pub(super) fn with_project<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Project) -> R,
        R: FromProjectLockError,
    {
        let guard = self.project.lock();
        let Some(project) = guard.as_ref() else {
            return R::no_project();
        };
        f(project)
    }

    // === Closure-adjusted geometry (non-destructive derived view) ===

    /// Compute (or return cached) closure-adjusted station coordinates for the
    /// current project. Non-destructive: the measured project is never mutated -
    /// the adjustment runs on the project clone this method already takes (see
    /// [`Project::into_adjusted_station_coordinates`]) and only the resulting
    /// coordinates are returned.
    ///
    /// The result is memoized in a transient cache keyed on the project's
    /// `modified_at` + closure-adjustment config and cleared on every
    /// command/undo/redo, so repeatedly toggling to the adjusted display view is
    /// cheap while any edit forces a fresh re-solve from intact measured data.
    /// Returns `None` if no project is open; the IPC boundary uses
    /// [`Self::adjusted_stations_checked`], which keeps the fallible envelope.
    pub fn get_adjusted_stations(&self) -> Option<Vec<Station>> {
        self.adjusted_stations_checked().ok().flatten()
    }

    /// [#576 C7] [`Self::get_adjusted_stations`] for the IPC boundary: `Ok(None)` ONLY
    /// when no project is open. It used to answer `Err` for a poisoned project or
    /// command lock; under the one poison policy (#576 A11) those locks are recovered
    /// and the internal-error banner raised instead, so the adjusted view keeps
    /// drawing from the data the lock holds. The `Result` stays: it is the envelope
    /// the command settles a failed solve task into (`settle_blocking`), which the
    /// frontend's fallback to the measured view still answers.
    pub fn adjusted_stations_checked(&self) -> Result<Option<Vec<Station>>, String> {
        let config = self.get_config();
        let config_hash = adjusted_config_hash(&config.closure_adjustment);

        // Cheap project-version read under a short lock.
        let version = {
            let guard = self.project.lock();
            let Some(project) = guard.as_ref() else {
                return Ok(None);
            };
            project.modified_at.timestamp_nanos_opt().unwrap_or(0)
        };

        {
            let cache = self.adjusted_cache.lock();
            if let Some(entry) = cache.as_ref() {
                if entry.version == version && entry.config_hash == config_hash {
                    return Ok(Some(entry.stations.clone()));
                }
            }
        }

        // Miss: clone the project out, then solve without holding any lock (the
        // solve is CPU-heavy and runs on a blocking thread). The clone is taken
        // under `command_lock` so it is serialized with `execute()`/undo/redo:
        // those hold `command_lock` across both the mutation *and* the
        // `reconcile_geometry()` epilogue, which repairs derived offsets without
        // bumping `modified_at`. Cloning outside that lock could capture a project
        // after the mutation but before reconcile and then cache that partially
        // reconciled solve under the final version. Re-read the version from the
        // clone so the cache key matches exactly the data solved, and hand the
        // clone to the by-value solver so it isn't cloned a second time.
        let (project, generation) = {
            let _cmd_guard = self.command_lock.lock();
            // Capture the invalidation generation alongside the (reconciled) clone,
            // under command_lock, so it reflects exactly the data being solved.
            let generation = self
                .adjusted_cache_generation
                .load(std::sync::atomic::Ordering::SeqCst);
            let Some(project) = self.project_snapshot() else {
                return Ok(None);
            };
            (project, generation)
        };
        let solved_version = project.modified_at.timestamp_nanos_opt().unwrap_or(0);
        // The solve adjusts a project of its own: this is the one copy it makes
        // (none when the slot has moved on and the snapshot is the last holder).
        let stations = Arc::unwrap_or_clone(project)
            .into_adjusted_station_coordinates(&config.closure_adjustment);

        {
            let mut cache = self.adjusted_cache.lock();
            // Only store if nothing invalidated the cache while command_lock was
            // released for the heavy, lock-free solve. Otherwise an edit/open/repair
            // that ran during the solve - including a repair that doesn't bump
            // modified_at - could be clobbered by this now-stale result.
            if self
                .adjusted_cache_generation
                .load(std::sync::atomic::Ordering::SeqCst)
                == generation
            {
                *cache = Some(AdjustedStationsCache {
                    version: solved_version,
                    config_hash,
                    stations: stations.clone(),
                });
            }
        }
        Ok(Some(stations))
    }

    /// Per-station distance field for the distance-gradient overlay (#34),
    /// computed on the MEASURED geometry of ONE settled project snapshot (taken
    /// under `command_lock`, so never in the middle of a command, #576 RV5-4).
    pub fn distance_field(&self, from: Option<&StationRef>) -> Option<DistanceField> {
        self.distance_field_checked(from).ok().flatten()
    }

    /// [#576 C7] [`Self::distance_field`] for the IPC boundary: `Ok(None)` only when no
    /// project is open. A poisoned lock is recovered (#576 A11, see
    /// [`Self::adjusted_stations_checked`]); the `Result` is the command's envelope.
    pub fn distance_field_checked(
        &self,
        from: Option<&StationRef>,
    ) -> Result<Option<DistanceField>, String> {
        // [#576 RV5-4] Settled: a plain snapshot taken while a command is between
        // its mutation and its epilogue is a project no user ever sees.
        let Some(project) = self.settled_snapshot() else {
            return Ok(None);
        };
        // Distance uses MEASURED geometry (surveyed shot lengths via the plain
        // coordinate walk) - instant. The closure-adjusted solve is intentionally
        // NOT used: it dominated the latency (~2s release / ~40s debug on a 6k-
        // station survey; ~O(n^2)) for a sub-meter difference, and reachability is
        // identical either way (it's topological - shots/merges/closures - not
        // coordinate-dependent). Closure-solve perf is tracked separately (#81).
        let stations = project.compute_station_coordinates();
        // Reference ties are not swimmable, so they leave the route unless the
        // user opts them back in (#27 q13).
        let include_reference = self.get_config().gas.include_reference_legs;
        Ok(Some(project.distance_field_from_with(
            &stations,
            from,
            include_reference,
        )))
    }

    /// Single-diver open-circuit gas reachability field (#38). Like [`distance_field`], the
    /// engine consumes the instant MEASURED geometry (reachability is coordinate-independent -
    /// never the slow closure-adjusted solve). `None` when no project is open. The diver /
    /// cylinders are assumed already validated and `gas_config` normalized by the command.
    pub fn gas_reach_field(
        &self,
        origin: Option<&StationRef>,
        endpoint: Option<&StationRef>,
        waypoints: &[StationRef],
        segments: &[PlanSegment],
        diver: &Diver,
        cylinders: &[Cylinder],
        gas_config: &GasConfig,
        stage_drop_overrides: &[Option<StationRef>],
    ) -> Option<GasReachField> {
        // [#576 RV5-4] Settled, like every IPC read of the project.
        let project = self.settled_snapshot()?;
        Some(project.compute_gas_reach_field_with_overrides(
            origin,
            endpoint,
            waypoints,
            segments,
            diver,
            cylinders,
            gas_config,
            stage_drop_overrides,
        ))
    }

    /// Shortest along-cave-line distance + traversed shots between two ruler
    /// endpoints (#26). The distance sums the CLOSURE-ADJUSTED shot lengths, so it
    /// uses the memoized adjusted-stations cache (`get_adjusted_stations`): warm
    /// while the adjusted view is in use, and on a cold cache it pays one closure
    /// solve (then repeat measurements are instant). Falls back to measured
    /// coordinates only if the adjusted solve is unavailable - without loop
    /// closures the two coincide anyway.
    pub fn along_line_between(
        &self,
        a: RulerEndpoint,
        b: RulerEndpoint,
    ) -> Option<AlongLineResult> {
        self.along_line_between_checked(a, b).ok().flatten()
    }

    /// [#576 C7] [`Self::along_line_between`] for the IPC boundary: `Ok(None)` only when
    /// no project is open. A poisoned lock is recovered (#576 A11, see
    /// [`Self::adjusted_stations_checked`]); the `Result` is the command's envelope.
    /// The measured fallback stays for an adjusted view that is merely absent.
    pub fn along_line_between_checked(
        &self,
        a: RulerEndpoint,
        b: RulerEndpoint,
    ) -> Result<Option<AlongLineResult>, String> {
        // [#576 RV5-4] Settled: the topology is read between commands, never in the
        // middle of one.
        let Some(project) = self.settled_snapshot() else {
            return Ok(None);
        };
        // `project` (topology) and `stations` (adjusted coords) are two independent
        // settled reads, so an edit landing between them could yield a slightly
        // skewed snapshot. That's safe here - `along_line_between` looks everything
        // up by `StationRef` through guarded `idx.get(..)` and indexes coords only
        // via those resolved indices, so a mismatch drops an edge at worst (never
        // panics) and the ruler re-queries. We deliberately avoid a single
        // command-locked snapshot to keep reusing the adjusted-stations cache.
        let stations = match self.adjusted_stations_checked()? {
            Some(stations) => stations,
            None => project.compute_station_coordinates(),
        };
        let include_reference = self.get_config().gas.include_reference_legs;
        Ok(Some(project.along_line_between_with(
            &stations,
            a,
            b,
            include_reference,
        )))
    }

    /// Drop any cached closure-adjusted coordinates. Called from every
    /// command/undo/redo path so the adjusted view never serves stale geometry.
    pub(super) fn invalidate_adjusted_cache(&self) {
        {
            let mut cache = self.adjusted_cache.lock();
            // Bump the generation under the cache lock so an in-flight solve's
            // generation check (also under this lock) reliably observes it.
            self.adjusted_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *cache = None;
        }
    }

    // === Snapshot helpers for snapshot-based undo ===

    /// Capture a JSON snapshot of the current project for undo.
    /// Returns the serialized JSON, or a `CommandResult` failure if unavailable.
    pub(super) fn capture_project_snapshot(&self) -> Result<String, CommandResult> {
        let guard = self.project.lock();
        let Some(p) = guard.as_ref() else {
            return Err(CommandResult::failure("No project open".to_string()));
        };
        Self::serialize_project_snapshot(p)
            .map_err(|e| CommandResult::failure(format!("Failed to serialize project: {e}")))
    }

    /// [#303] Serialize a project FOR AN UNDO SNAPSHOT, with machine-local fields cleared.
    ///
    /// Snapshot entries stopped being local-only when Full mode started shipping undo
    /// history: `sanitize_push_snapshot` deliberately keeps `undo_stack` in the upload, and
    /// every `ProjectSnapshot` / `ProjectSnapshotPair` is a whole serialized `Project`.
    /// `file_path` is an absolute path on THIS machine - `/home/<user>/…`,
    /// `C:\Users\<Real Name>\…` - so shipping it writes the pusher's username and directory
    /// layout into the project's git history, permanently, readable by every collaborator
    /// and by whoever runs the server. Snapshot pairs come from Delete shot, Delete section,
    /// Reverse section and every Batch import, so this is routine editing, not an edge case.
    ///
    /// Cleared HERE, at the one place a snapshot is produced, rather than scrubbed out of
    /// the upload: a snapshot that never holds the value cannot leak it through a log, a bug
    /// report, or some future consumer either. `restore_snapshot_into` carries the LIVE path
    /// back across, exactly as it already does for `validation`, so an undo does not lose
    /// track of where the file lives.
    ///
    /// The clone is the cost of doing it correctly: `Project` is megabytes on a large
    /// survey. It is proportionate - this path already builds a JSON string of the same
    /// project - and it only runs for the snapshot commands, not for ordinary edits.
    ///
    /// `file_path` is the only machine-local field on `Project`: the four transient caches
    /// are `#[serde(skip)]`, and `imported_file_hashes` is content hashes that are meant to
    /// proliferate. A new field of that kind must be added here too.
    pub(super) fn serialize_project_snapshot(project: &Project) -> serde_json::Result<String> {
        if project.file_path.is_none() {
            // An unsaved project has nothing to scrub, so skip the clone entirely.
            return serde_json::to_string(project);
        }
        let mut scrubbed = project.clone();
        scrubbed.file_path = None;
        serde_json::to_string(&scrubbed)
    }

    /// Deserialize a project snapshot and install it into `slot`, rebuilding the
    /// transient lookup caches (`section_index_cache`, `shot_uuid_index`) that
    /// serde skips. THE single place a restored project enters the live slot:
    /// every rollback/undo/redo path routes through here so none can forget to
    /// rebuild a cache and leave a half-populated / stale map behind (which would
    /// silently mis-resolve sections) - see catalog #14/#39. The merge adjacency
    /// needs nothing here: [`MergeTopology`](crate::models::MergeTopology) builds
    /// it on first use, so a freshly deserialized project is correct by
    /// construction.
    /// Returns false and leaves `slot` untouched if the snapshot can't be parsed.
    #[must_use]
    pub(super) fn restore_snapshot_into(slot: &mut ProjectSlot, snapshot: &str) -> bool {
        // A snapshot persisted by a build predating the per-project `validation`
        // field (#57) has no "validation" key, so serde (the field is
        // #[serde(default)]) fills ValidationConfig::default() - silently changing
        // the project's thresholds and its warnings when an undo/redo of a legacy
        // persisted entry is restored. Detect that legacy blob and carry the CURRENT
        // project's validation across instead. Undo of a real settings change always
        // serializes the key, so it still restores the intended thresholds (Codex).
        let snapshot_has_validation = snapshot.contains("\"validation\":");
        let current_validation = slot.as_ref().map(|p| p.validation.clone());
        // Same idiom for `surface_elevation` (#431/#448): a snapshot persisted by a
        // build predating that field has no "surface_elevation" key, so serde's
        // #[serde(default)] fills None - silently dropping a real entrance
        // elevation on undo/redo of a legacy persisted entry. Carry the CURRENT
        // project's value across for those legacy blobs only; a real elevation
        // edit always serializes the key, so it still restores correctly.
        let snapshot_has_surface_elevation = snapshot.contains("\"surface_elevation\":");
        let current_surface_elevation = slot.as_ref().and_then(|p| p.surface_elevation);
        // A pre-anchor build wrote every cross-reference as a positional
        // `StationRef` (`{section_id, station_index}`); this build writes
        // `StationKey`s (`{section_id, anchor}`). See the canonicalization below.
        let snapshot_is_pre_anchor = snapshot.contains("\"station_index\":");
        // [#303] Where the file lives is a property of THIS machine, not of the survey, so
        // `serialize_project_snapshot` clears it rather than write a username into every
        // pushed commit. Carry the live value across on the way back, or an undo would leave
        // the app thinking the project has never been saved - and the next save would ask
        // for a path. Unconditional, unlike the `validation` carry-over above: the snapshot
        // never has a useful path in it, whether it was written before this change (someone
        // else's path) or after (none at all).
        let current_file_path = slot.as_ref().and_then(|p| p.file_path.clone());
        match serde_json::from_str::<Project>(snapshot) {
            Ok(mut restored) => {
                // #557: the snapshot blob is FILE-SUPPLIED (persisted undo is
                // loaded from the .stalx), and serde rebuilds `Section`s
                // field-by-field - never through `Section::new`, so neither the
                // open-refusal in `persistence/sqlite/read.rs` nor the
                // `restore_section` bound can see a hostile value arriving
                // here. Two fields are load-bearing for the shot-id encode and
                // both come straight from the JSON:
                // - `id` past MAX_SECTION_ID puts the first uuid-index or
                //   save-path encode back in the debug-panic / release-wrap
                //   regime the bound exists to end;
                // - `next_shot_id` outside the section's stride window makes
                //   the next `add_shot` (`self.next_shot_id += 1`) panic in
                //   debug or wrap into another section's id space in release
                //   (Codex, PR #575). A legit counter always sits inside
                //   [id * STRIDE, (id + 1) * STRIDE] - `Section::new` starts it
                //   at the window base and the max-shots-per-section check caps
                //   growth. Upper bound in u64: for id = MAX_SECTION_ID the
                //   window end is u32::MAX + 1.
                // Refuse the whole restore rather than repair it: a snapshot is
                // a before/after pair, and a "fixed" one no longer restores the
                // state it recorded.
                let window_violation = |s: &crate::models::Section| {
                    let base = u64::from(s.id) * u64::from(crate::models::FEATURE_ID_STRIDE);
                    let end = base + u64::from(crate::models::FEATURE_ID_STRIDE);
                    let counter = u64::from(s.next_shot_id);
                    counter < base || counter > end
                };
                if let Some(section) = restored
                    .sections
                    .iter()
                    .find(|s| s.id > crate::models::MAX_SECTION_ID || window_violation(s))
                {
                    tracing::warn!(
                        section_id = section.id,
                        next_shot_id = section.next_shot_id,
                        max = crate::models::MAX_SECTION_ID,
                        "Refusing snapshot restore: section id or shot counter outside the shot-id encode bound"
                    );
                    return false;
                }
                if !snapshot_has_validation {
                    if let Some(validation) = current_validation {
                        restored.validation = validation;
                    }
                }
                if !snapshot_has_surface_elevation {
                    if let Some(surface_elevation) = current_surface_elevation {
                        restored.surface_elevation = Some(surface_elevation);
                    }
                }
                restored.file_path = current_file_path;
                restored.rebuild_section_index();
                restored.rebuild_shot_uuid_index();
                // A snapshot blob persisted by a PRE-ANCHOR build stores every
                // cross-reference positionally (undo/redo history is persisted to
                // SQLite and reloaded across builds, and the back-compat StationKey
                // deserializer reads the legacy `{section_id, station_index}` shape as
                // a transitional `Index` anchor). A later shot insert/delete leaves an
                // `Index` key at its position, so it would silently start naming
                // another station. Canonicalize such a snapshot - the same step the
                // load path runs - to AfterShot/SectionStart. The re-anchor drops no
                // comment: two that land on one key are joined
                // (`Project::map_station_refs`).
                //
                // CRITICALLY, gate this on the snapshot being PRE-ANCHOR, not on an
                // `Index` anchor alone (#576 W52-1). A snapshot this build wrote can
                // hold one too: a key that did not resolve at the open (a comment row
                // past the end of its section) stays `Index` in the live project, and
                // every snapshot taken while it is there records it. Re-anchoring that
                // snapshot moved the key onto whatever shot then sat at its position -
                // one an append had added, so undoing the append deleted the comment
                // with the shot. And ensure_unique would RE-MINT intentional duplicate
                // UUIDs, e.g. the pre-repair state that an undo of a duplicate_shot_uuid
                // Repair must restore EXACTLY. So a snapshot this build wrote is
                // restored with its keys and UUIDs exactly as recorded (undo/redo is a
                // true before/after pair). This build never writes the positional
                // shape - `StationKey` serializes `{section_id, anchor}`, and no other
                // field of a `Project` is named `station_index`
                // (`a_snapshot_this_build_writes_never_holds_the_positional_key_shape`
                // pins it) - and a JSON string cannot fake it, as its quotes are
                // escaped. The `Index` check stays beside it, so a pre-anchor snapshot
                // whose refs all sit at a section start is left as before. (For a
                // pre-anchor snapshot, ensure_shot_uuids runs FIRST: a pre-v6 blob can
                // have uuid:null shots, and ensure_unique only dedups existing UUIDs -
                // without minting nulls, reanchor can't canonicalize the Index anchor
                // of a null-uuid station.)
                let has_legacy_index = snapshot_is_pre_anchor && {
                    let mut found = false;
                    restored.for_each_station_ref(|_, key| {
                        if matches!(key.anchor, StationAnchor::Index(_)) {
                            found = true;
                        }
                    });
                    found
                };
                if has_legacy_index {
                    restored.ensure_shot_uuids();
                    restored.ensure_unique_shot_uuids();
                    restored.reanchor_station_refs();
                }
                // A save heals the live project with no undo entry of its own,
                // and a snapshot recorded BEFORE that save holds the pre-heal
                // project - so restoring one reverts the heal and makes the
                // project at a given undo depth depend on the route taken to
                // reach it (#413, finding 11). Bring the restored project back
                // up to the standard the live one is already held to; the
                // heal-by-heal reasoning for what that includes (and what it
                // deliberately does not) is on the function.
                reapply_heals_after_restore(&mut restored, slot.as_ref());
                // The `section_offsets` invariant (`Project::write_section_offset`)
                // has to hold for a map this app did not write, and a snapshot
                // payload is exactly that case: undo/redo history is persisted to
                // SQLite and reloaded across sessions, so an entry recorded by a
                // build that predates the invariant can carry a dust-zero key. The
                // load boundary drops one out of a `.stalx`; this is the same drop
                // one payload over. Not a heal decided per-heal like the block
                // above - it is an invariant, and a restore may not install a state
                // the app cannot otherwise reach.
                let dropped = restored.normalize_section_offsets();
                if dropped > 0 {
                    tracing::info!(
                        dropped,
                        purpose = "hold the section_offsets invariant across a snapshot restore",
                        "Dropped restored section offsets that were within dust of the origin"
                    );
                }
                slot.set(Some(restored));
                true
            }
            Err(e) => {
                tracing::error!(
                    purpose = "restore project snapshot",
                    reason = %e,
                    "Failed to deserialize project snapshot; slot left unchanged"
                );
                false
            }
        }
    }

    /// Rollback the project to a previously captured snapshot.
    /// Handles poisoned mutex gracefully (recovery is more important than panic).
    pub(super) fn rollback_project(&self, snapshot: &str) {
        let mut proj = self.project.lock();
        if Self::restore_snapshot_into(&mut proj, snapshot) {
            tracing::info!(
                purpose = "restore project after failed command",
                "Successfully rolled back to pre-mutation snapshot"
            );
        }
    }

    /// Capture an "after" snapshot of the project, rolling back on failure.
    /// Returns the serialized JSON, or rolls back and returns a `CommandResult` failure.
    pub(super) fn capture_after_snapshot_or_rollback(
        &self,
        before_snapshot: &str,
    ) -> Result<String, CommandResult> {
        let mut proj_guard = self.project.lock();
        let Some(p) = proj_guard.as_ref() else {
            tracing::error!(
                purpose = "restore project after mutation error",
                "Project became None after mutation; rolling back"
            );
            let _ = Self::restore_snapshot_into(&mut proj_guard, before_snapshot);
            return Err(CommandResult::failure(
                "No project for after snapshot".to_string(),
            ));
        };
        match Self::serialize_project_snapshot(p) {
            Ok(json) => Ok(json),
            Err(e) => {
                tracing::error!(
                    purpose = "finalize undo snapshot",
                    reason = %e,
                    "Failed to serialize after snapshot; rolling back"
                );
                if Self::restore_snapshot_into(&mut proj_guard, before_snapshot) {
                    tracing::info!(
                        purpose = "restore project after snapshot failure",
                        "Successfully rolled back after serialization failure"
                    );
                }
                Err(CommandResult::failure(format!(
                    "Failed to serialize after snapshot: {e}"
                )))
            }
        }
    }

    /// Push a snapshot-pair undo entry onto the undo stack.
    pub(super) fn push_snapshot_undo(
        &self,
        cmd: crate::core::Command,
        before: String,
        after: String,
    ) {
        {
            let mut undo_stack = self.undo_stack.lock();
            undo_stack.push_with_data(
                cmd,
                crate::core::UndoData::ProjectSnapshotPair { before, after },
            );
        }
    }
}

impl AppState {
    /// Record the shots a load found with an unreadable `kind` (#27), tagged
    /// with that load's `loading_id`. Called once per load.
    pub(crate) fn set_unrecognised_shot_kinds(
        &self,
        loading_id: u64,
        found: Vec<crate::models::UnrecognisedShotKind>,
    ) {
        let mut guard = self.unrecognised_shot_kinds.lock();
        // Never let an OLDER load clobber a newer one's diagnostics. A cancelled
        // load runs on to its next checkpoint, so it can reach this line after
        // the project the user actually opened already stored its own.
        if guard.as_ref().is_some_and(|(seen, _)| *seen > loading_id) {
            return;
        }
        *guard = Some((loading_id, found));
    }

    /// Take the recorded set IF it belongs to `loading_id`, leaving the slot
    /// empty. Taking rather than cloning makes the prompt one-shot: the frontend
    /// asks once per load, and a refresh cannot resurrect a dialog the user has
    /// already answered. The id check makes a mismatch return nothing rather
    /// than another project's shot ids.
    pub fn take_unrecognised_shot_kinds(
        &self,
        loading_id: u64,
    ) -> Vec<crate::models::UnrecognisedShotKind> {
        let mut guard = self.unrecognised_shot_kinds.lock();
        match guard.as_ref() {
            Some((seen, _)) if *seen == loading_id => {
                guard.take().map(|(_, found)| found).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// [#473] Record whether the just-committed project's history survived its load.
    ///
    /// Unconditional assignment, not a set-on-failure: see [`Self::history_discarded`]
    /// for why every commit must write it.
    pub fn set_history_discarded(&self, discarded: bool) {
        self.history_discarded
            .store(discarded, std::sync::atomic::Ordering::SeqCst);
    }

    /// [#473] Whether the open project lost its edit history on load.
    ///
    /// A plain read rather than a take, unlike [`Self::take_unrecognised_shot_kinds`]:
    /// that one drives a one-shot prompt the user answers, this one describes an ongoing
    /// condition (docs/notifications.md: banner-class), so re-asking must keep returning
    /// the same answer for as long as the project is open. Dismissal is the frontend's
    /// business, exactly as it is for the integrity banner.
    pub fn history_discarded(&self) -> bool {
        self.history_discarded
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// [#576 B6-D1] Record the schema version of the just-committed project's file
    /// when a newer build saved it (`None` otherwise). Unconditional assignment on
    /// every project-slot commit, like [`Self::set_history_discarded`], so a newer
    /// project's read-only state can never leak into the next one.
    pub(crate) fn set_newer_file_schema(&self, version: Option<i32>) {
        self.newer_file_schema
            .store(version.unwrap_or(0), std::sync::atomic::Ordering::SeqCst);
    }

    /// [#576 B6-D1] The open project's file schema version when a NEWER
    /// Stalagmight saved it - the project is then open read-only - else `None`.
    pub fn newer_file_schema(&self) -> Option<i32> {
        match self
            .newer_file_schema
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            0 => None,
            version => Some(version),
        }
    }

    /// [#576 NF-W2F-1] The open file's flags as they stand NOW: the committed
    /// newer-file stamp, the unread-settings list (RV4-R-S5), and whether the open
    /// database's `speleo_link` row can be read - the same read the load and every save make, so the answer always
    /// matches what the next save would do. No project (or no database) open reads
    /// as neither.
    pub fn open_file_flags(&self) -> crate::core::loading::OpenFileFlags {
        let speleo_link_unreadable = self
            .database
            .lock()
            .as_ref()
            .is_some_and(|db| db.load_speleo_link().is_err());
        crate::core::loading::OpenFileFlags {
            newer_schema_version: self.newer_file_schema(),
            speleo_link_unreadable,
            // [#576 RV4-R-S5] The list the slot commit set for the file open now.
            unreadable_project_settings: self.unreadable_project_settings(),
        }
    }

    /// [#576 B15] Record which stored per-project settings the just-committed
    /// project's load could not read. Unconditional assignment on every
    /// project-slot commit, like [`Self::set_newer_file_schema`].
    pub(crate) fn set_unreadable_project_settings(&self, keys: Vec<String>) {
        *self.unreadable_project_settings.lock() = keys;
    }

    /// [#576 B15 follow-up] The user set `key` again (Project Settings): the value
    /// in memory is now theirs, not a default standing in for an unread one, so
    /// writing it over the stored value is what they asked for and `key` no longer
    /// refuses a save. Saves work again once nothing is left on the list.
    pub(crate) fn lift_unreadable_project_setting(&self, key: &str) {
        let mut keys = self.unreadable_project_settings.lock();
        if keys.iter().any(|k| k == key) {
            keys.retain(|k| k != key);
            tracing::info!(
                key,
                remaining = keys.len(),
                purpose = "let a save write a setting the user set again",
                "An unread project setting was set again; it no longer refuses saves"
            );
        }
    }

    /// [#576 B15] The stored per-project settings the open project's load could
    /// not read; while any are listed, every save is refused.
    pub fn unreadable_project_settings(&self) -> Vec<String> {
        self.unreadable_project_settings.lock().clone()
    }

    /// [#576 B6-D1] The refusal for anything that would write the open project when
    /// a newer build saved it, or `None` when the project is this build's to write.
    /// Shared by the save and the SpeleoDB push/lock verbs so they all say the same
    /// thing; `guard_mutation` carries the same reason behind its own code.
    pub(super) fn newer_file_refusal(&self) -> Option<String> {
        self.newer_file_schema().map(newer_file_reason)
    }
}

/// [#576 B6-D1] Why a project saved by a newer Stalagmight cannot be written here.
/// Plain words: it reaches the log and, through the coded refusals, the user.
fn newer_file_reason(version: i32) -> String {
    format!(
        "this project was saved by a newer version of Stalagmight (file format {version}) \
         and is open read-only. Update Stalagmight to edit it."
    )
}

/// [#576 B6-D1] The refusal of an edit (`what`) to a file a newer build saved:
/// `NEWER_FILE_EDIT_DISABLED` (#576 C4), an edit-gate code - a refused SAVE of
/// the same file is `NEWER_FILE_READ_ONLY` instead.
pub(super) fn newer_file_edit_refusal(what: &str, version: i32) -> CommandError {
    CommandError::new(
        CommandErrorCode::NewerFileEditDisabled,
        format!("{what} is disabled: {}", newer_file_reason(version)),
    )
}
