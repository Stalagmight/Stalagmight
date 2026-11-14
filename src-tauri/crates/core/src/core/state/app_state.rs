use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

use crate::cache::TileCache;
use crate::config::{ClosureAdjustmentConfig, Config, GasConfig};
use crate::models::{
    AdjustmentFailure, AdjustmentFailureReport, AdjustmentReport, AlongLineResult, Cylinder,
    DistanceField, Diver, GasReachField, PlanSegment, Project, RulerEndpoint, SkippedClosure,
    Station, StationAnchor, StationRef,
};
use crate::persistence::Database;
use crate::tile_stats::TileFetchStats;
use crate::tilemap_cache::TilemapCache;

use crate::core::{CommandResult, UndoStack};

use super::project_io::{reapply_heals_after_restore, RecordedGauges};

/// Trait for types that can represent project lock errors.
/// Enables `with_project_mut` and `with_project` to work with different return types.
pub(super) trait FromProjectLockError {
    fn lock_failed() -> Self;
    fn no_project() -> Self;
}

impl FromProjectLockError for CommandResult {
    fn lock_failed() -> Self {
        CommandResult::failure("Failed to lock project".to_string())
    }
    fn no_project() -> Self {
        CommandResult::failure("No project open".to_string())
    }
}

impl<T> FromProjectLockError for Result<T, String> {
    fn lock_failed() -> Self {
        Err("Failed to lock project".to_string())
    }
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
    /// Components that same solve could not adjust and shows measured (#737).
    /// Read by `get_adjustment_report` and the warnings pass, only while the
    /// entry matches the project version they are asked about.
    pub(super) failures: Vec<AdjustmentFailure>,
}

/// Hash the closure-adjustment config fields that affect the solve, for the
/// adjusted-stations cache key. Shared by `get_adjusted_stations` and
/// `distance_field` so both derive the same key.
fn adjusted_config_hash(ca: &ClosureAdjustmentConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ca.base_sigma_length_m.to_bits().hash(&mut h);
    ca.base_sigma_azimuth_deg.to_bits().hash(&mut h);
    ca.base_sigma_depth_m.to_bits().hash(&mut h);
    h.finish()
}

/// Application state managed by Tauri
pub struct AppState {
    /// Current project (if any)
    pub(super) project: Mutex<Option<Project>>,
    /// Database connection (if project is open)
    pub(super) database: Mutex<Option<Database>>,
    /// True while the open project's initial save is still deferred - created with
    /// no save path and never written to disk, so it has no `.stalx` and no Recent
    /// Projects entry yet. Tracked explicitly rather than inferred from
    /// `database.is_none()` (ambiguous for dropped handles) or file existence
    /// (ambiguous for reserved-path collisions), so the first import can durably
    /// persist it exactly once (#92 / PR #104).
    pub(super) initial_save_deferred: AtomicBool,
    /// Whether the open project's shot UUIDs a file held in more than one
    /// section were renamed (`Project::make_shot_uuids_project_unique`), by its
    /// load or by a Repair. Until a save re-keys them, ignored warnings can be
    /// stored against the old ids, and only then is it worth deriving the
    /// replacement to match them. Set by both load paths once the project is
    /// committed and by Repair; cleared on close and on project create.
    pub(super) load_renamed_shots: AtomicBool,
    /// Undo/redo stack
    pub(super) undo_stack: Mutex<UndoStack>,
    /// Application configuration
    pub(super) config: Mutex<Config>,
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
    pub(super) config_write_lock: Mutex<()>,
    /// Recent projects list
    pub(super) recent_projects: Mutex<Vec<PathBuf>>,
    /// SpeleoDB per-session connection state (endpoint, email, token store).
    /// Read-only spike (#183); the token never crosses into the webview.
    pub(super) speleodb: Mutex<super::speleodb::SpeleoDbSession>,
    /// What the open project is as far as the edit gate is concerned: unmanaged
    /// (always editable) or a SpeleoDB-managed project (editable only through a
    /// bound lock claim). TRANSIENT, never persisted. There is deliberately NO
    /// edit-gate boolean: `edit_enabled()` is derived from this, the claims and
    /// the session generation - see `speleodb_edit_gate`. A leaf lock: nothing
    /// else is ever taken while it is held.
    pub(super) speleodb_open_gate: Mutex<super::speleodb_edit_gate::OpenProjectGate>,
    /// In-memory display settings cache (used when no DB is open, e.g., new unsaved project)
    pub(super) display_settings_cache: Mutex<HashMap<String, String>>,
    /// The SpeleoDB collaborator listings already fetched, per (endpoint,
    /// account, project) - survey-credit suggestions (`speleodb_collaborators`).
    /// A leaf lock.
    pub(super) speleodb_collaborators: Mutex<super::speleodb_collaborators::CollaboratorCache>,
    /// Serialization lock for command execution.
    /// Held for the entire duration of execute(), undo(), and redo() to prevent
    /// concurrent commands from interleaving (especially with batch sub-commands).
    pub(super) command_lock: Mutex<()>,
    /// HTTP cache for map tiles, held strictly under the provider's own
    /// `Cache-Control` - imagery is never stored for offline use.
    ///
    /// Note: Uses Arc<Mutex<...>> rather than plain Mutex<...> because the protocol handler
    /// runs in a separate async task/thread that outlives the closure scope, requiring
    /// owned/cloneable access to the cache. This is a deliberate deviation from the spec
    /// for thread-safety requirements.
    ///
    /// Note: Changes to CacheConfig at runtime (via update_config) do NOT re-initialize
    /// this cache. Users must restart the app for cache config changes to take effect.
    pub(super) tile_cache: Arc<Mutex<Option<TileCache>>>,
    /// In-memory cache for ArcGIS tilemap API responses.
    /// Used to efficiently check tile availability before fetching.
    pub(super) tilemap_cache: Arc<Mutex<TilemapCache>>,
    /// Lock-free tile fetch statistics (in-flight count, latency, errors).
    /// Uses Arc because the protocol handler needs a cloneable reference.
    pub(super) tile_fetch_stats: Arc<TileFetchStats>,
    /// Sink for emitting events to the host application (set during app setup)
    pub(super) event_sink: Mutex<Option<Arc<dyn crate::core::events::EventSink>>>,
    /// Active loading cancellation token (if any load operation is in progress)
    pub(super) active_loading: Mutex<Option<Arc<AtomicBool>>>,
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
        Mutex<Option<(u64, Vec<crate::models::UnrecognisedShotKind>)>>,
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
    /// Ignored warnings an "Add .stalx" carried in for sections the open file's
    /// database does not hold yet (#674; see `project_add.rs`). Written by the
    /// next save, cleared with every project-slot commit. Transient.
    pub(super) pending_ignores: Mutex<Vec<crate::persistence::ResolvedWarning>>,
    /// Memoized closure-adjusted station coordinates for the display-mode toggle.
    /// See [`AdjustedStationsCache`]. Transient; never persisted.
    pub(super) adjusted_cache: Mutex<Option<AdjustedStationsCache>>,
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
    /// Serializes SpeleoDB lock-acquisition spans against lock releases that run
    /// detached from the session that captured them (the delete's captured
    /// release, the stranded-claim janitor). Start Editing holds this across
    /// acquire -> commit, so a captured release either completes before a new
    /// acquire begins or observes the successor's claim and stands down - the F8
    /// "stale release frees the successor's mutex" interleaving cannot fit in
    /// between. ORDER: this mutex is OUTERMOST - it is held across network calls,
    /// and code holding `project`/`database`/`undo_stack` must never block on it
    /// (decide under `project`, drop it, then take this). Nor may a synchronous
    /// Tauri command: those run on the main thread, and an upload or download
    /// holds this for minutes. See `core::state::speleodb_session`.
    pub(super) speleodb_lock_ops: Mutex<()>,
    /// The #202 lock re-verify running in the background, if any: started by one
    /// edit, collected by a later one, so `execute()` never waits on the network.
    /// See `AppState::speleodb_verify_lock_if_expired`. Held only briefly, never
    /// across I/O.
    pub(super) speleodb_reverify: Mutex<Option<super::speleodb::PendingReverify>>,
}

impl AppState {
    /// Create a new application state
    pub fn new() -> Result<Self> {
        let mut config = Config::load().unwrap_or_default();
        // A hand-edited endpoint gets the same cleaning as a settings save (#675).
        config.speleodb.endpoint =
            super::config::clean_endpoint(config.speleodb.endpoint.as_deref());
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

        Ok(Self {
            project: Mutex::new(None),
            database: Mutex::new(None),
            initial_save_deferred: AtomicBool::new(false),
            load_renamed_shots: AtomicBool::new(false),
            undo_stack: Mutex::new(UndoStack::new_with_limit(undo_limit)),
            config: Mutex::new(config),
            config_write_lock: Mutex::new(()),
            recent_projects: Mutex::new(recent),
            speleodb: Mutex::new(super::speleodb::SpeleoDbSession::default()),
            // Nothing open: unmanaged, so editing never waits on a lock (#191).
            speleodb_open_gate: Mutex::default(),
            display_settings_cache: Mutex::new(HashMap::new()),
            speleodb_collaborators: Mutex::new(HashMap::new()),
            command_lock: Mutex::new(()),
            tile_cache: Arc::new(Mutex::new(tile_cache)),
            tilemap_cache: Arc::new(Mutex::new(TilemapCache::new())),
            tile_fetch_stats: Arc::new(TileFetchStats::new()),
            event_sink: Mutex::new(None),
            active_loading: Mutex::new(None),
            unrecognised_shot_kinds: Mutex::new(None),
            // Default-false: no project is open, so no history has been discarded.
            history_discarded: AtomicBool::new(false),
            pending_ignores: Mutex::new(Vec::new()),
            adjusted_cache: Mutex::new(None),
            adjusted_cache_generation: AtomicU64::new(0),
            speleodb_session_generation: AtomicU64::new(0),
            speleodb_lock_ops: Mutex::new(()),
            speleodb_reverify: Mutex::new(None),
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
    pub fn tile_cache(&self) -> Arc<Mutex<Option<TileCache>>> {
        Arc::clone(&self.tile_cache)
    }

    /// Get a clone of the tilemap cache Arc for sharing across threads
    pub fn tilemap_cache(&self) -> Arc<Mutex<TilemapCache>> {
        Arc::clone(&self.tilemap_cache)
    }

    /// Get a clone of the tile fetch stats Arc for sharing across threads
    pub fn tile_fetch_stats(&self) -> Arc<TileFetchStats> {
        Arc::clone(&self.tile_fetch_stats)
    }

    /// Set the event sink (called during app setup)
    pub fn set_event_sink(&self, sink: Arc<dyn crate::core::events::EventSink>) {
        if let Ok(mut h) = self.event_sink.lock() {
            *h = Some(sink);
        }
    }

    /// Get the event sink
    pub fn get_event_sink(&self) -> Option<Arc<dyn crate::core::events::EventSink>> {
        self.event_sink.lock().ok().and_then(|h| h.clone())
    }

    /// Start a new loading operation, cancelling any existing one.
    /// Returns the cancellation token for the new operation.
    pub fn start_new_load(&self) -> Arc<AtomicBool> {
        let mut guard = self
            .active_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

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
        if let Ok(mut guard) = self.active_loading.lock() {
            if let Some(ref current) = *guard {
                if Arc::ptr_eq(current, expected_token) {
                    *guard = None;
                }
            }
        }
    }

    // === Read-only edit gate + dirty bit (#191) ===
    //
    // `edit_enabled()` lives in `speleodb_edit_gate`: it is derived from the lock
    // claim, and there is no setter.

    /// THE mutation chokepoint (#191, D-B6). Every SURVEY-CONTENT mutation entry
    /// (`execute` + all its `execute_*` siblings, `undo`/`redo`/`undo_to`/`redo_to`,
    /// `set_project_name_for_path`) calls this FIRST, after
    /// taking `command_lock` and before mutating. `Err` aborts the mutation when
    /// the open project is a SpeleoDB project with no edit session (no bound lock
    /// claim; see `speleodb_edit_gate`). No-op (`Ok`) for local-only projects, which
    /// are always editable, and non-mutating reads.
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
    /// - `track_imported_file`/`track_section_hashes` import-dedup bookkeeping stays
    ///   UNGATED for now: the in-import path gates + dirties, but the standalone
    ///   `track_imported_file` command does not - a deferred follow-up, lower-impact
    ///   than a lost annotation (re-import is the worst case, not silent data loss).
    ///
    /// `MUTATION_ENTRYPOINTS` (tests.rs) enumerates the gated set (incl. the two
    /// warning verbs).
    pub(super) fn guard_mutation(&self, what: &str) -> Result<(), CommandResult> {
        if self.edit_enabled() {
            return Ok(());
        }
        Err(CommandResult::failure(format!(
            "{}: {what} is disabled: this SpeleoDB project is open read-only. \
             Acquire the lock to edit.",
            crate::core::command::EDIT_DISABLED_CODE
        )))
    }

    /// Flip `has_unpushed_edits` `false → true` AND refresh `last_edit_at` on a
    /// successful edit of a remote project (#191, D-B16; v10 last_edit_at). No-op
    /// for local-only (`load_speleo_link → None`). The dirty bit is SET-ONLY in
    /// #191 - local save/autosave NEVER clears it; only a successful server push
    /// (Sync/Release) does. `last_edit_at` is refreshed on EVERY accepted
    /// mutation (not idempotent) because guardrail-7 compares it to push-start to
    /// decide whether a landed-by-ancestry push may clear the dirty bit.
    pub(super) fn mark_remote_dirty(&self) {
        let Ok(db_guard) = self.database.lock() else {
            return;
        };
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

    /// Cancel any active loading operation (idempotent - safe to call anytime)
    pub fn cancel_loading(&self) {
        // A confirmed frontend transition (including one that later fails) must
        // invalidate older downloads before it probes or reads another cache.
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.bump_speleodb_session();
        if let Ok(guard) = self.active_loading.lock() {
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
        let Ok(mut guard) = self.project.lock() else {
            return R::lock_failed();
        };
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
        let Ok(guard) = self.project.lock() else {
            return R::lock_failed();
        };
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
    /// Returns `None` if no project is open.
    pub fn get_adjusted_stations(&self) -> Option<Vec<Station>> {
        let config = self.get_config();
        let config_hash = adjusted_config_hash(&config.closure_adjustment);

        // Cheap project-version read under a short lock.
        let version = {
            let guard = self.project.lock().ok()?;
            guard
                .as_ref()?
                .modified_at
                .timestamp_nanos_opt()
                .unwrap_or(0)
        };

        if let Ok(cache) = self.adjusted_cache.lock() {
            if let Some(entry) = cache.as_ref() {
                if entry.version == version && entry.config_hash == config_hash {
                    return Some(entry.stations.clone());
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
            let _cmd_guard = self.command_lock.lock().ok()?;
            // Capture the invalidation generation alongside the (reconciled) clone,
            // under command_lock, so it reflects exactly the data being solved.
            let generation = self
                .adjusted_cache_generation
                .load(std::sync::atomic::Ordering::SeqCst);
            (self.get_project_state()?, generation)
        };
        let solved_version = project.modified_at.timestamp_nanos_opt().unwrap_or(0);
        let adjusted = project.into_adjusted_stations(&config.closure_adjustment);
        let stations = adjusted.stations;

        if let Ok(mut cache) = self.adjusted_cache.lock() {
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
                    failures: adjusted.failures,
                });
            }
        }
        Some(stations)
    }

    /// The components the last closure-adjusted solve of `project` could not
    /// adjust (#737). Read-only on the memoized cache: empty unless that cache
    /// was solved from exactly this project version under the current
    /// closure-adjustment config, so it never runs the solve itself and never
    /// reports a failure the data has since moved past.
    ///
    /// Takes only the cache lock, which no path holds while taking another, so
    /// a caller may hold the project lock (the warnings pass does).
    pub(super) fn cached_adjustment_failures(&self, project: &Project) -> Vec<AdjustmentFailure> {
        let version = project.modified_at.timestamp_nanos_opt().unwrap_or(0);
        let config_hash = adjusted_config_hash(&self.get_config().closure_adjustment);
        let Ok(cache) = self.adjusted_cache.lock() else {
            return Vec::new();
        };
        match cache.as_ref() {
            Some(entry) if entry.version == version && entry.config_hash == config_hash => {
                entry.failures.clone()
            }
            _ => Vec::new(),
        }
    }

    /// What the Closures Computed View leaves out, for the notice above the map
    /// (#737): the closures it skips because they span one shot (computed now,
    /// `O(stations)`), and the components whose solve failed (from the cache
    /// only - see [`Self::cached_adjustment_failures`]). `None` when no project
    /// is open.
    pub fn get_adjustment_report(&self) -> Option<AdjustmentReport> {
        let guard = self.project.lock().ok()?;
        let project = guard.as_ref()?;
        Some(AdjustmentReport {
            skipped_closures: project
                .closures_across_one_shot()
                .iter()
                .map(SkippedClosure::from)
                .collect(),
            failures: self
                .cached_adjustment_failures(project)
                .iter()
                .map(AdjustmentFailureReport::from)
                .collect(),
        })
    }

    /// Per-station distance field for the distance-gradient overlay (#34),
    /// computed on the closure-adjusted geometry. Takes ONE reconciled project
    /// snapshot under `command_lock` (so the graph topology and the adjusted
    /// coordinates come from the same version - no read-skew between two separate
    /// state reads), and reuses the memoized adjusted-stations cache read-only
    /// when it matches that snapshot (keeps re-root instant in the adjusted view).
    pub fn distance_field(&self, from: Option<&StationRef>) -> Option<DistanceField> {
        let project = self.get_project_state()?;
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
        Some(project.distance_field_from_with(&stations, from, include_reference))
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
        let project = self.get_project_state()?;
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
        let project = self.get_project_state()?;
        // `project` (topology) and `stations` (adjusted coords) are two independent
        // locked reads, so a concurrent edit between them could yield a slightly
        // skewed snapshot. That's safe here - `along_line_between` looks everything
        // up by `StationRef` through guarded `idx.get(..)` and indexes coords only
        // via those resolved indices, so a mismatch drops an edge at worst (never
        // panics) and the ruler re-queries. We deliberately avoid a single
        // command-locked snapshot to keep reusing the adjusted-stations cache.
        let stations = self
            .get_adjusted_stations()
            .unwrap_or_else(|| project.compute_station_coordinates());
        let include_reference = self.get_config().gas.include_reference_legs;
        Some(project.along_line_between_with(&stations, a, b, include_reference))
    }

    /// Drop any cached closure-adjusted coordinates. Called from every
    /// command/undo/redo path so the adjusted view never serves stale geometry.
    pub(super) fn invalidate_adjusted_cache(&self) {
        if let Ok(mut cache) = self.adjusted_cache.lock() {
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
        let Ok(guard) = self.project.lock() else {
            return Err(CommandResult::failure("Failed to lock project".to_string()));
        };
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
    pub(super) fn restore_snapshot_into(slot: &mut Option<Project>, snapshot: &str) -> bool {
        Self::restore_snapshot_into_with(slot, snapshot, None)
    }

    /// [`Self::restore_snapshot_into`] for one step through a snapshot pair:
    /// `edited` holds the gauge pairs the half being left behind recorded (see
    /// [`reapply_heals_after_restore`]).
    #[must_use]
    pub(super) fn restore_snapshot_into_with(
        slot: &mut Option<Project>,
        snapshot: &str,
        edited: Option<&RecordedGauges>,
    ) -> bool {
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
                // A snapshot written before the survey credit moved onto the
                // shots carries each section's surveyor/date at section level;
                // give it to the shots that have none (a no-op for a current
                // snapshot, which never serializes those keys).
                restored.absorb_legacy_section_credit();
                restored.rebuild_section_index();
                restored.rebuild_shot_uuid_index();
                // A snapshot blob persisted by a PRE-ANCHOR build can carry
                // transitional positional `Index` cross-reference anchors (undo/redo
                // history is persisted to SQLite and reloaded across sessions, and the
                // back-compat StationKey deserializer accepts the legacy
                // `{section_id, station_index}` shape). The live-model invariant is
                // "no Index anchors": a later shot insert/delete only shifts
                // AfterShot/SectionStart refs, so a live Index ref would silently start
                // resolving to the wrong station. Canonicalize such a snapshot - the
                // same step the load path runs - to AfterShot/SectionStart.
                //
                // CRITICALLY, gate this on the snapshot actually being LEGACY (an Index
                // anchor is present). A CURRENT same-session snapshot is already
                // canonical, so mint/reanchor would be no-ops - but ensure_unique would
                // RE-MINT intentional duplicate UUIDs, e.g. the pre-repair state that an
                // undo of a duplicate_shot_uuid Repair must restore EXACTLY. Skipping
                // the block when there is no Index anchor keeps current snapshots
                // byte-for-byte (so undo/redo is a true before/after pair) while legacy
                // snapshots are still fully canonicalized. (For a legacy snapshot,
                // ensure_shot_uuids runs FIRST: a pre-v6 blob can have uuid:null shots,
                // and ensure_unique only dedups existing UUIDs - without minting nulls,
                // reanchor can't canonicalize the Index anchor of a null-uuid station.)
                let has_legacy_index = {
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
                reapply_heals_after_restore(&mut restored, slot.as_ref(), edited);
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
                *slot = Some(restored);
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
        let mut proj = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
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
        let mut proj_guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!(
                    purpose = "finalize undo snapshot",
                    "Project mutex poisoned during after-snapshot; recovering"
                );
                poisoned.into_inner()
            }
        };
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
        if let Ok(mut undo_stack) = self.undo_stack.lock() {
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
        let mut guard = match self.unrecognised_shot_kinds.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
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
        let mut guard = match self.unrecognised_shot_kinds.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
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
}
