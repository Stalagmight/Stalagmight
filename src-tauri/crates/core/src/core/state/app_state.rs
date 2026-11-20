use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::cache::TileCache;
use crate::config::{ClosureAdjustmentConfig, Config, GasConfig};
use crate::models::survey::{AdjustedPositions, Project, RawPositions};
use crate::models::{
    AdjustmentReport, AlongLineResult, Cylinder, DistanceField, Diver, GasReachField, PlanSegment,
    RulerEndpoint, Station, StationRef,
};
use crate::tile_stats::TileFetchStats;
use crate::tilemap_cache::TilemapCache;

use crate::core::{CommandResult, HistoryDropped, UndoStack};

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

/// The derived views of one project revision, memoised (C0 item 5): the raw
/// layout (solver v1) and, per closure-adjustment config, the Closures
/// Computed View. Nothing derived is stored in the model, so a revision is a
/// complete key; [`AppState::bump_revision`] runs on every change of the
/// project slot. Never persisted.
pub(super) struct DerivedCache {
    pub(super) revision: u64,
    pub(super) raw: Arc<RawPositions>,
    pub(super) adjusted: Option<(u64, Arc<AdjustedPositions>)>,
}

/// Hash the closure-adjustment config fields that affect the solve, for the
/// adjusted cache key.
pub(super) fn adjusted_config_hash(ca: &ClosureAdjustmentConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ca.base_sigma_length_m.to_bits().hash(&mut h);
    ca.base_sigma_azimuth_deg.to_bits().hash(&mut h);
    ca.base_sigma_depth_m.to_bits().hash(&mut h);
    h.finish()
}

/// Application state managed by Tauri
pub struct AppState {
    /// Current project (if any): the station-graph survey (`models::survey`).
    pub(super) project: Mutex<Option<Project>>,
    /// Everything the open project's file holds besides the survey and the
    /// history (SpeleoDB link and sync state, display settings, ignored
    /// warnings, preserved members, the external-modification guard). There
    /// is no live database handle (stream AS): see `open_file`.
    pub(super) file: Mutex<super::open_file::FileState>,
    /// Bumped on every change of the project slot (an edit, an undo or redo,
    /// a load, a close): the revision the derived-view cache is keyed on.
    pub(super) revision: AtomicU64,
    /// Raw and adjusted positions of the current revision (see
    /// [`DerivedCache`]). A leaf lock.
    pub(super) derived: Mutex<Option<DerivedCache>>,
    /// True while the open project's initial save is still deferred - created with
    /// no save path and never written to disk, so it has no `.stalx` and no Recent
    /// Projects entry yet. Tracked explicitly rather than inferred from
    /// `database.is_none()` (ambiguous for dropped handles) or file existence
    /// (ambiguous for reserved-path collisions), so the first import can durably
    /// persist it exactly once (#92 / PR #104).
    pub(super) initial_save_deferred: AtomicBool,
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
    /// An unversioned slot would then hand project B the line and shot ids
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
    /// Written ONLY inside the load's non-cancellable finalize line, under the same
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
    /// Whether the history [`Self::history_discarded`] reports was dropped because an
    /// EARLIER version of the app wrote it (`UndoStack::load_from_persisted_with_timestamps`
    /// returning `HistoryDropped::EarlierVersion`), not because it was unreadable. The
    /// banner says which. Reset with the flag it qualifies.
    pub(super) history_from_earlier_version: AtomicBool,
    /// The SpeleoDB SESSION generation: bumped on every session boundary - a
    /// project committed into (or cleared from) the `project` slot, and logout -
    /// and ONLY while holding the `project` mutex. Captured by Start Editing at
    /// entry and re-checked under the same mutex before its commit opens the edit
    /// gate, so a teardown landing inside its network window refuses the commit
    /// instead of opening a gate onto a torn-down world (F7). The same idiom as
    /// [`Self::revision`] / `unrecognised_shot_kinds`, applied to
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
            file: Mutex::new(super::open_file::FileState::default()),
            revision: AtomicU64::new(0),
            derived: Mutex::new(None),
            initial_save_deferred: AtomicBool::new(false),
            undo_stack: Mutex::new(UndoStack::new_with_limit(undo_limit)),
            config: Mutex::new(config),
            config_write_lock: Mutex::new(()),
            recent_projects: Mutex::new(recent),
            speleodb: Mutex::new(super::speleodb::SpeleoDbSession::default()),
            // Nothing open: unmanaged, so editing never waits on a lock (#191).
            speleodb_open_gate: Mutex::default(),
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
            history_from_earlier_version: AtomicBool::new(false),
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
            old_token.store(true, Ordering::SeqCst);
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
    /// (`execute` and the edit pipeline, `undo`/`redo`/`undo_to`/`redo_to`,
    /// `set_project_name_for_path`, the ignore verbs) calls this FIRST, after
    /// taking `command_lock` and before mutating. `Err` aborts the mutation when
    /// the open project is a SpeleoDB project with no edit session (no bound lock
    /// claim; see `speleodb_edit_gate`). No-op (`Ok`) for local-only projects, which
    /// are always editable. Display settings stay ungated (per-machine view state,
    /// never pushed).
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

    /// [`Self::guard_mutation`] for a command, named the way the user reads it
    /// ([`Self::command_label`]); the label is only built for a refusal.
    pub(super) fn guard_mutation_for(
        &self,
        cmd: &crate::core::Command,
    ) -> Result<(), CommandResult> {
        if self.edit_enabled() {
            return Ok(());
        }
        self.guard_mutation(&self.command_label(cmd))
    }

    /// What `cmd` is called in a message the user reads (a read-only refusal,
    /// a batch refusal) and in the undo history: worded with the names the
    /// open project holds now (`super::labels`). Takes the project lock
    /// briefly; never call it holding that lock.
    pub(super) fn command_label(&self, cmd: &crate::core::Command) -> String {
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.as_ref() {
            Some(project) => super::labels::command_label(project, cmd),
            None => cmd.description(),
        }
    }

    /// Flip the SpeleoDB dirty bit `false -> true` AND refresh `last_edit_at` on
    /// a successful edit of a linked project (#191, D-B16). No-op for a local
    /// project. Set-only: only a confirmed push clears it. In memory, made
    /// durable at once by a local-only save (B3), so a crash after an edit
    /// still knows the copy has unpushed work.
    pub(super) fn mark_remote_dirty(&self) {
        let result = self.mutate_speleo_link(|link| {
            let Some(link) = link.as_mut() else {
                return false;
            };
            link.has_unpushed_edits = true;
            link.last_edit_at = Some(chrono::Utc::now().to_rfc3339());
            true
        });
        if let Err(e) = result {
            tracing::warn!(
                error = %e,
                purpose = "flip SpeleoDB dirty bit after a remote edit",
                "mark_remote_dirty: the dirty bit is set in memory only"
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
                token.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Lock the project mutex and pass a mutable reference to the callback.
    /// Returns the callback's result, or an error if the lock fails or no project is open.
    /// Bumps the revision (whatever the callback did): a caller that only
    /// reads uses [`Self::with_project`]. Test-only: production edits go
    /// through the execute pipeline (`run_recorded`).
    #[cfg(test)]
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
        let result = f(project);
        self.bump_revision();
        result
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

    // === Derived views, memoised per revision (C0 item 5) ===

    /// A new revision of the project slot: every cached derived view is stale.
    /// Called under the `project` lock by everything that changes the slot.
    pub(super) fn bump_revision(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn current_revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }

    /// The raw layout of `project` (the project in the slot, read under its
    /// lock), from the cache when this revision has one.
    pub(super) fn raw_positions_of(&self, project: &Project) -> Arc<RawPositions> {
        let revision = self.current_revision();
        let mut cache = self
            .derived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = cache.as_ref().filter(|e| e.revision == revision) {
            return Arc::clone(&entry.raw);
        }
        let raw = Arc::new(project.raw_positions());
        *cache = Some(DerivedCache {
            revision,
            raw: Arc::clone(&raw),
            adjusted: None,
        });
        raw
    }

    /// The Closures Computed View of the open project under the current
    /// closure-adjustment config, memoised per revision. On a miss the
    /// project is cloned under its lock and solved without it (the solve is
    /// CPU-heavy); the result is stored only if no edit landed meanwhile.
    pub(super) fn adjusted_positions(&self) -> Option<(Project, Arc<AdjustedPositions>)> {
        let config = self.get_config().closure_adjustment;
        let key = adjusted_config_hash(&config);
        let (project, revision, raw) = {
            let guard = self.project.lock().ok()?;
            let project = guard.as_ref()?;
            let revision = self.current_revision();
            let raw = self.raw_positions_of(project);
            let cache = self
                .derived
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((hash, adjusted)) = cache
                .as_ref()
                .filter(|e| e.revision == revision)
                .and_then(|e| e.adjusted.as_ref())
            {
                if *hash == key {
                    return Some((project.clone(), Arc::clone(adjusted)));
                }
            }
            (project.clone(), revision, raw)
        };
        let adjusted = Arc::new(project.adjusted_positions_from(&raw, &config));
        let mut cache = self
            .derived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = cache.as_mut().filter(|e| e.revision == revision) {
            entry.adjusted = Some((key, Arc::clone(&adjusted)));
        }
        Some((project, adjusted))
    }

    /// The closure-adjusted station coordinates (per slot) for the
    /// display-mode toggle. Non-destructive and memoised per revision.
    /// Returns `None` if no project is open.
    pub fn get_adjusted_stations(&self) -> Option<Vec<Station>> {
        let (project, adjusted) = self.adjusted_positions()?;
        Some(project.adjusted_stations_wire(&adjusted))
    }

    /// The adjustment report (skipped closures, failed components, far GPS
    /// fixes) of the Computed View under the current config.
    pub fn get_adjustment_report(&self) -> Option<AdjustmentReport> {
        let (project, adjusted) = self.adjusted_positions()?;
        Some(project.adjustment_report(Some(&adjusted)))
    }

    /// Per-slot along-cave distance from `from` (or from the entrances when
    /// `None`), on the measured layout: reachability is topological, and the
    /// raw layout is instant.
    pub fn distance_field(&self, from: Option<&StationRef>) -> Option<DistanceField> {
        let include_reference = self.get_config().gas.include_reference_legs;
        let guard = self.project.lock().ok()?;
        let project = guard.as_ref()?;
        let raw = self.raw_positions_of(project);
        Some(project.distance_field_wire(raw.as_ref(), from.copied(), include_reference))
    }

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
        let guard = self.project.lock().ok()?;
        let project = guard.as_ref()?;
        Some(project.gas_reach_field_wire(
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

    /// The ruler: the along-cave distance between two endpoints, on the
    /// adjusted layout (as 0.6.0).
    pub fn along_line_between(
        &self,
        a: RulerEndpoint,
        b: RulerEndpoint,
    ) -> Option<AlongLineResult> {
        let include_reference = self.get_config().gas.include_reference_legs;
        let (project, adjusted) = self.adjusted_positions()?;
        Some(project.along_line_wire(adjusted.as_ref(), &a, &b, include_reference))
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
        self.history_discarded.store(discarded, Ordering::SeqCst);
        self.history_from_earlier_version
            .store(false, Ordering::SeqCst);
    }

    /// Record what the history load of the just-committed project dropped, if
    /// anything: either reason raises the discarded-history banner, and an earlier
    /// version's history gets its own words there (it was not damaged, only written in
    /// the format before compact undo entries).
    pub fn note_history_dropped(&self, dropped: Option<HistoryDropped>) {
        let Some(reason) = dropped else {
            return;
        };
        self.history_discarded.store(true, Ordering::SeqCst);
        self.history_from_earlier_version
            .store(reason == HistoryDropped::EarlierVersion, Ordering::SeqCst);
    }

    /// Whether [`Self::history_discarded`]'s history was an earlier version's.
    pub fn history_discarded_from_earlier_version(&self) -> bool {
        self.history_discarded() && self.history_from_earlier_version.load(Ordering::SeqCst)
    }

    /// [#473] Whether the open project lost its edit history on load.
    ///
    /// A plain read rather than a take, unlike [`Self::take_unrecognised_shot_kinds`]:
    /// that one drives a one-shot prompt the user answers, this one describes an ongoing
    /// condition (docs/notifications.md: banner-class), so re-asking must keep returning
    /// the same answer for as long as the project is open. Dismissal is the frontend's
    /// business, exactly as it is for the integrity banner.
    pub fn history_discarded(&self) -> bool {
        self.history_discarded.load(Ordering::SeqCst)
    }
}
