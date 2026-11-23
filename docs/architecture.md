# Stalagmight Architecture

> For full data model, UI design, and workflows, see [SPEC.md](SPEC.md). The
> survey model itself is [survey-model.md](survey-model.md); the project file is
> [file-format.md](file-format.md).

## Workspace layout

The Rust backend is a cargo workspace rooted at `src-tauri/Cargo.toml` (a
hybrid root: the `stalagmight` app package lives there too). Nine library
crates sit under `src-tauri/crates/`:

| Crate | Contents | Key deps |
|-------|----------|----------|
| `stalagmight-models` | The data model: the station-graph survey (`survey.rs`, `survey/`: stations, lines, shots, datums, placements, closures, the derived views, the changeset undo engine, the wire projection), the shared DTOs, geometry (`geo.rs`), `app_dirs`, gas config | - (base crate) |
| `stalagmight-telemetry` | Telemetry collector + event registry (`telemetry/`), tracing/logging setup (`logging/`); its `build.rs` owns `TELEMETRY_ENDPOINT` validation and `APP_VERSION` | models |
| `stalagmight-parsers` | Import parsers: DMP, Ariane TML/TMLU, Compass, Walls, GPX | models |
| `stalagmight-legacy` | A frozen copy of the 0.6.0 model, SQLite reader and load pipeline: opens every old SQLite `.stalx` exactly as the 0.6.0 app did. Read-only in production; the `writer` feature (tests only) is the 0.6.0 save path. Published to crates.io with persistence and stalx-reader, as an internal crate | - (standalone) |
| `stalagmight-persistence` | The `.stalx` version 2 codec (`persistence/stalx_v2/`), the legacy migration (`persistence/migrate/`), the legacy recovery dialog's SQLite repair and salvage (`persistence/recovery.rs`), the `.plnx` dive-plan store, tile caches (`cache/`, `tilemap_cache`) | models, legacy |
| `stalagmight-export` | CSV / JSON / image (SVG->PNG/PDF) / TML export, dive profile | models, parsers |
| `stalagmight-speleodb` | SpeleoDB client: HTTP surface, auth/token storage, project file transfer, sync, TLS policy, metrics | models, telemetry |
| `stalagmight-core` | `AppState`, the execute pipeline and the command families, undo/redo, project I/O orchestration, config I/O (`config/`), `tile_providers`; shared test helpers behind the `test-support` feature | export, models, parsers, persistence, speleodb |
| `stalx-reader` | Read-only `.stalx` reader for external consumers (SpeleoDB server-side maps, #593): both formats, the legacy one through the frozen reader; nothing in the app depends on it | models, persistence, legacy |
| `stalagmight` (app, at `src-tauri/`) | The thin Tauri layer: `commands/` (IPC handlers), `lib.rs` (the single `generate_handler!`), `main.rs`, `tile_attribution.rs`, `webkit_memory.rs`, `examples/`, `tests/` | all of the above except stalx-reader |

The layering is strict and one-way - models / legacy -> telemetry / parsers /
persistence -> export / speleodb -> core -> app, where `->` reads "is depended
on by" - so a lower crate never imports from a higher one. The workspace root also carries the shared
`[workspace.dependencies]`, the `[workspace.lints]` policy every member
inherits, and the `telemetry` feature, which forwards to the members that
implement it (`stalagmight-telemetry`, `stalagmight-speleodb`).

## State Flow

```
User Action → Zustand Store → Tauri IPC → Rust Command Executor
     ↑                                              │
     └──────────── State Update ←──────── Command Result
```

## Command Pattern

All persistent operations go through the Rust `Command` enum in
`src-tauri/crates/core/src/core/command.rs`. `AppState`
(`src-tauri/crates/core/src/core/state/app_state.rs`) executes them through one
pipeline (`core/state/commands.rs`); each command family's mutation is one
`apply(project, record, cmd)` in its own file under
`src-tauri/crates/core/src/core/state/` (`command_line.rs`, `command_shot.rs`,
`command_station.rs`, `command_move.rs`, `command_comment.rs`,
`command_station_name.rs`, `command_credit.rs`, `command_cave_section.rs`,
`import_tracking.rs`), dispatched by `commands::apply_command`. See
[AppState and the open file](#appstate-and-the-open-file) below.

## Data Model

The survey is `models::survey::Project`
(`src-tauri/crates/models/src/survey.rs`; the guide is
[survey-model.md](survey-model.md)). It stores observations and user
decisions only; every position, misclosure, alignment flag and datum role is
derived from it by a deterministic solver.

- **Stations** are entities keyed by `StationId`, the user-visible Station ID:
  a name, an optional comment, the stations merged into it and archived there
  (each with its own id, name, comment and the lines it was the station of:
  [kept stations](plans/2026-11-23_kept-stations.md)), an optional external
  (imported) id. A junction shows its own identity on every line, and a loop
  closure may show one end as the other.
- **Lines** are simple chains: `start`, then each shot's `to`. A station on two
  or more lines is a **junction** (what a merge was). Line metadata: name,
  colour, direction, quality, azimuth reference, Section, legacy surveyor and
  date. Legacy hand moves (`hand_moves`, per line and station) only come from
  migrated files.
- **Shots** carry their measurements and a project-unique `uuid` (the
  persistent identity). `Shot::id` is an in-session handle
  (`line_id * FEATURE_ID_STRIDE + seq`, `src-tauri/crates/models/src/id_stride.rs`,
  mirrored in `src/utils/ids.ts`), never persisted: use the uuid for anything
  that has to survive a save and a load.
- **Datums** (`ZeroDatum`): a station, its GPS, accuracy, entrance override, a
  `via` line hint and a maintained `correction`. The first datum of a
  connected piece places it; every later one in it is a GPS fix.
- **Placements**: where each datum-less piece sits (one station at local x, y).
- **Loop closures**: two stations with `via` hints and a strategy; `Tie` is the
  legacy rigid tie only migrated files hold.
- **Sections** (#327), settings (validation thresholds, surface elevation,
  default azimuth reference), import hashes and the id counters.

`StationRef { line_id, station_index }` (a *slot*: the i-th station along a
line) is the view address the IPC and the frontend use; the command boundary
resolves it to a `StationId` (`resolve_slot`, refusing an index past the line's
last station). `Project::validate` (plan section 2) holds the invariants every
edit, load and save checks.

The IPC sends the frontend the shapes it always read (`survey::wire`): one
`Station` per slot, merge pairs derived from junctions, closure metrics and
`is_aligned` derived from the raw layout.

Two things a project owns are deliberately *not* in the survey model. The
SpeleoDB link is the file's `speleodb` member (identity, it rides uploads) plus
its sync state in `local.speleodb` (per machine); in memory both live in
`FileState` (below). A linked project is usable at exactly one path - the pull
cache, `<cache>/speleodb/<project_id>/project.stalx` - and two seams enforce
that: `do_load_project_full` refuses to open a linked file anywhere else
(`core/state/speleodb_foreign.rs`), and an explicit save destination on a linked
project writes a detached copy WITHOUT the link and keeps the session on the
managed file (docs/saving.md, "Save a Copy"). Named dive plans live in a
sibling `<stem>.plnx` file.

## AppState and the open file

`AppState` holds the open survey and no database handle:

| Field | What |
|---|---|
| `project: Mutex<Option<survey::Project>>` | the survey in memory, the authority |
| `file: Mutex<FileState>` | everything else the file holds (`core/state/open_file.rs`) |
| `revision: AtomicU64` | bumped on every change of the project slot: an edit, an undo or redo, a load, a close |
| `derived: Mutex<Option<DerivedCache>>` | the derived views of the current revision |
| `undo_stack: Mutex<UndoStack>` | both history stacks (`core/undo.rs`) |
| `command_lock: Mutex<()>` | serializes execute, undo, redo and the recipes |

The session, edit-gate, config, tile and loading fields are as before; see the
field docs in `app_state.rs`.

**`FileState`** (`core/state/open_file.rs`) is what the open file holds besides
the survey and the history, in memory: the v2 `Extras` (the SpeleoDB link
identity, `local` with display settings, sync state, Unwind chain and
watermark, ignored warnings, preserved unknown members, `requires`, the
read-only reason), the history as last saved, the external-modification
guard's `Fingerprint`, the origin (new, version 2, or migrated legacy), the
`dirty` and `local_pending` flags, the one-time conversion notice and the
legacy record parity the dive-plan resolver reads. It reaches the disk two
ways: a normal save writes everything, and the **local-only durable save**
(`AppState::persist_local_state`, `persist_local_state_at` for a caller holding
the `project` guard) rewrites `speleodb`, `local` and `history` and copies
every other member of the file verbatim. SpeleoDB state changes and display
toggles use it, so they are durable at once. [Saving](saving.md) has the
whole save side.

**The pipeline** (`core/state/commands.rs`). `AppState::execute` takes
`command_lock`, handles the lifecycle commands itself (create, open, save,
close; Merge Projects and Add .stalx go to their own modules), runs the gates
(the SpeleoDB lock re-verify and edit session through `guard_mutation`), then
calls `run_recorded`:

1. refuse a file opened read-only (`READ_ONLY_FILE`);
2. build the `Record`: kind, label worded now with the names before the edit,
   stable refs, preview rows (`core/state/labels.rs`);
3. `before = project.clone()`;
4. run the mutation: `apply_command` -> the family's `apply`, which calls the
   survey operations (they resolve slots, settle anchors and run the
   orphan-closure sweep themselves); a refusal restores `before`;
5. `validate()`: a violation restores `before` and refuses;
6. `capture(&before, &project)`: an empty changeset is a provable no-op (no
   entry, the redo branch kept, nothing dirtied);
7. stamp `modified_at`, bump the revision, push the entry (redo cleared), mark
   the file dirty, then set the SpeleoDB dirty bit and drop the Unwind
   position (`mark_remote_forward_edit`).

**Recipes.** One user action that is several survey steps is an `AppState`
method rather than a `Command` variant: Split Line Here
(`command_split_line.rs`), Combine Lines (`command_combine_lines.rs`), the
delete-and-split family (`command_delete_split.rs`), split shot at distance and
Insert Station (`command_split_shot.rs`), imports and create-and-merge
(`command_import.rs`), Add .stalx (`project_add.rs`), project settings and
surface elevation (`project_create.rs`). Each takes `command_lock`, calls
`guard_mutation(..)`, and runs `run_recorded` with its own `Record`, so it gets
the same rollback, validation, capture and one undo step.

**Derived views.** Nothing derived is stored in the model, so the revision is a
complete cache key. `AppState::raw_positions_of(project)` memoises the raw
layout (solver v1) per revision; `adjusted_positions()` memoises the Closures
Computed View per revision and closure-adjustment config, solving a clone
outside the project lock and storing the result only if no edit landed
meanwhile. The adjustment report and the ruler read the Computed View, the
distance field and `get_project_state` the raw layout.

**Lock order:** `project` -> `file` -> `undo_stack` -> `derived` (`file` took the
place the database handle had). `command_lock` is outermost and
`speleodb_lock_ops` sits outside even that; see
[Undo/Redo, Lock Ordering](undo-redo.md#lock-ordering).

## Subsystems

Most have their own page. What follows is the seam and where the code lives, nothing more.

**Workspace modes.** `uiStore.appMode` (`survey` | `plan` | `export`) switches the whole
workspace from the `ModeCarousel`; Plan and Export are read-only. Ask whether editing is
blocked through `useEditingBlocked()` / `isEditingBlockedNow()`
(`src/hooks/useEditingBlocked.ts`) and nothing else - `pnpm lint:readonly-gates` bans the
raw composition, because writing it inline as `useA() || useB()` short-circuits the second
store hook and violates the Rules of Hooks. Note the asymmetry: the map-read-only term has
no backend backstop - a local project is always editable in Rust whatever mode the UI is
in (the backend gate, `edit_enabled()`, is derived from the SpeleoDB lock claim and only
ever closes a SpeleoDB project) - so a missed gate is a real bug, not a cosmetic one. The
third term, a file made with a newer Stalagmight, has one (`READ_ONLY_FILE`).
-> [app-modes.md](app-modes.md)

**Gas and dive planning.** `gas_reach` (`src-tauri/crates/models/src/survey/gas.rs`) seeds
a gas-weighted Dijkstra over the same survey graph the distance overlay uses (a junction is
one node, closures zero-cost edges), weighting each shot by the gas it costs to swim in, and
marks every station reachable within the turn budget. It runs on measured coordinates -
reachability is coordinate-independent, so it never triggers the slow closure-adjusted
solve. Inputs live in `src/store/gasPlanStore.ts` (runtime only, never persisted); named
plans persist to the sibling `.plnx` (`src-tauri/crates/persistence/src/persistence/plnx.rs`) and store the
engine's *input tuple*, never the derived field, so a loaded plan cannot show a stale
overlay. A plan pins a station by its neighbour shots plus its Station ID
(`core/state/dive_plan.rs`). -> [gas-planning.md](gas-planning.md)

**Survey rendering.** Shots, stations, labels, gradients and the measured-path tint are
drawn by a custom WebGL layer (`src/components/Map/surveyLayer/customRender/`), which is
the *sole* survey renderer: the stock bulk layers were deleted, so there is no fallback -
an unrecoverable GL failure removes the dead layer and surfaces an error. Hit-testing does
not depend on MapLibre either; it runs against the SoA geometry index. What is still stock
MapLibre in `SurveyLayer.tsx` are the small GeoJSON auxiliaries: loop closures, the
gas-route highlight, merge ghosts, and the interact hover/selection overlay - the last of
which holds only the few features the user is currently touching, so its size tracks
interaction state, not survey size. Global View is custom-only too
(`src/components/GlobalView/customRender/globalController.ts`).
-> [map-renderer.md](map-renderer.md)

**SpeleoDB.** `src-tauri/crates/speleodb/src/speleodb/` holds the client half: HTTP surface, token storage
in the backend, `.stalx` transfer through `speleodb-api` (a version 2 upload is sent as
`application/json`, a legacy one as `application/vnd.sqlite3`),
the base-SHA fence and ancestry recovery. Orchestration against `AppState` lives in
`core/state/speleodb.rs` and its `speleodb_*.rs` siblings, the IPC commands in
`commands/speleodb.rs`. The model is a
single-writer hand-off: a pulled project is read-only until the write lock is acquired, and
unlike the mode-driven read-only above, that one has a real backend backstop -
`AppState::guard_mutation` refuses every gated mutation while the lock is missing.
See [SpeleoDB.md](SpeleoDB.md) for the integration contract and
[speleodb-dev-testing.md](speleodb-dev-testing.md) for API test setup.

**Telemetry.** `src-tauri/crates/telemetry/src/telemetry/` is always compiled but real only behind the
`telemetry` Cargo feature; with it off every method is an empty no-op. Anonymous
session-level counters, off by default and opt-in via config, buffered to disk on a failed
flush and retried. `registry.rs` owns the event vocabulary; the `registry_covers_all_*`
tests that fail when a new command / tool / dialog is added without a matching event live
in `src-tauri/src/commands/telemetry_coverage_tests.rs`.
-> [telemetry.md](telemetry.md)

## Key Files

| File | Purpose |
|------|---------|
| `src-tauri/crates/core/src/core/command.rs` | Command enum and result types |
| `src-tauri/crates/core/src/core/state/` | `AppState` (`app_state.rs`), the pipeline (`commands.rs`), the open file (`open_file.rs`), one module per command family |
| `src-tauri/crates/core/src/core/state/project_io.rs`, `project_load.rs`, `legacy_upgrade.rs` | Save, open, and the first write over a legacy file |
| `src-tauri/crates/core/src/core/undo.rs`, `state/undo.rs` | The history stacks; undo, redo and the jumps |
| `src-tauri/crates/models/src/survey.rs`, `survey/` | The station-graph survey model ([survey-model.md](survey-model.md)) |
| `src-tauri/crates/models/src/survey/changeset.rs` | The undo engine: id-keyed changesets, history entries |
| `src-tauri/crates/models/src/survey/wire.rs` | What the IPC sends the frontend (`ProjectWire`, per-slot `Station`s, previews) |
| `src-tauri/crates/models/src/survey/gas.rs` | Gas-weighted reachability engine |
| `src-tauri/crates/models/src/survey/edit/split.rs`, `edit/combine.rs` | Split Line Here and Combine Lines on the station graph |
| `src-tauri/crates/persistence/src/persistence/stalx_v2.rs` | The `.stalx` version 2 codec: encode, decode, probe, salvage, the atomic write ([file-format.md](file-format.md)) |
| `src-tauri/crates/persistence/src/persistence/migrate/mod.rs` | Legacy SQLite `.stalx` -> survey model, with its verification |
| `src-tauri/crates/legacy/src/open.rs` | The frozen 0.6.0 open of a legacy file |
| `src-tauri/crates/persistence/src/persistence/plnx.rs` | `.plnx` sibling store for named dive plans |
| `src-tauri/crates/parsers/src/parsers/dmp.rs` | MNemo DMP file parser |
| `src-tauri/crates/models/src/geo.rs` | GPS-to-local coordinate transformation (WGS84 → ENU) |
| `src-tauri/crates/speleodb/src/speleodb/mod.rs` | SpeleoDB client module root (HTTP, auth, native files, sync, TLS policy) |
| `src-tauri/crates/persistence/src/cache/tiles.rs` | Satellite tile cache (SQLite-based) |
| `src-tauri/crates/core/src/tile_providers.rs` | Tile provider registry (provider metadata, URL expansion) |
| `src-tauri/src/commands/cache.rs` | Cache IPC commands (stats, clear, fetch) |
| `src-tauri/src/commands/survey_geometry.rs` | Survey geometry IPC command (`get_survey_geometry_for_file`) |
| `src/store/projectStore.ts` | Frontend state, Tauri IPC calls |
| `src/store/uiStore.ts` | UI state (selection, tools, panels, `appMode`) |
| `src/store/gasPlanStore.ts` | Gas/dive-plan inputs (runtime only, never persisted) |
| `src/store/globalViewStore.ts` | Global View state (selection, geometry loading) |
| `src/hooks/useEditingBlocked.ts` | The one sanctioned read-only predicate (map mode + SpeleoDB lock + read-only file) |
| `src/components/Map/MapCanvas.tsx` | MapLibre map initialization |
| `src/components/Map/SurveyLayer.tsx` | Survey layer: custom-renderer wiring, GeoJSON auxiliaries, interaction |
| `src/components/Map/surveyLayer/customRender/controller.ts` | Custom WebGL survey renderer (buffers, colors, layer lifecycle) |
| `src/components/GlobalView/GlobalView.tsx` | Global View container component |
| `src/components/GlobalView/GlobalViewMap.tsx` | MapLibre map for Global View |
| `src/components/GlobalView/GlobalSurveyLayer.ts` | Multi-system survey geometry rendering |
| `src/components/GlobalView/ViewModeToggle.tsx` | View mode toggle and `useViewMode` hook |
| `src/components/GlobalView/MetadataPanel.tsx` | System metadata display with aggregate stats |
| `src/utils/coordinates.ts` | Coordinate transforms (meters ↔ GPS) |
| `src/utils/tileProtocol.ts` | MapLibre tile protocol for caching |
| `src/utils/tileConfig.ts` | Tile source config; platform-specific `stalagmight-tiles` URL form |
| `src/hooks/useProject.ts` | Project file operations hook |
| `src/hooks/useMapTools.ts` | Map tool handlers |
| `src/hooks/useKeyboardShortcuts.ts` | Global keyboard shortcut handler |
| `src/hooks/useFpsMonitor.ts` | FPS monitoring (no consumer today - see below) |
| `src/theme/tokens.ts` | Color theme token definitions (10 themes) |
| `src/components/Map/GridOverlay.tsx` | Theme-aware coordinate grid overlay |
| `src/components/Dialogs/KeyboardShortcutsDialog.tsx` | Keyboard shortcuts overlay (`?` key) |
| `src/components/Dialogs/settings/AppearanceSettings.tsx` | Theme mode & color theme selection UI |
| `src/components/Dialogs/settings/ThemePreviewCard.tsx` | Mini app mockup preview cards for themes |
| `src-tauri/crates/core/src/config/types.rs` | Config types incl. `AppearanceConfig`, `ImportConfig` (`PlacementSuggestionConfig` is defined in `src-tauri/crates/models/src/config/mod.rs`) |

## Color Theme System

Stalagmight supports 10 color themes with independent day (light) and night (dark) palettes:

| Theme | Type | Description |
|-------|------|-------------|
| `dark` | Dark | Default dark blue/purple with teal accent |
| `light` | Light | Default warm beige/paper with teal accent |
| `cave` | Dark | Very dark, desaturated with gold accent |
| `dracula` | Dark | Dracula-inspired with purple accent |
| `one_dark` | Dark | Atom One Dark-inspired with blue accent |
| `solarized_light` | Light | Solarized light with blue accent |
| `catppuccin_latte` | Light | Catppuccin Latte (light) with blue accent |
| `nord` | Dark | Nord-inspired with cyan accent |
| `catppuccin_mocha` | Dark | Catppuccin Mocha (dark) with blue accent |
| `kaleido_eink` | Light | Pure black on pure white for e-ink panels; all CSS transitions and animations are force-disabled under this theme (`src/index.css`) to prevent ghosting |

### Theme Architecture

```
AppearanceConfig (Rust)          UIStore (Zustand)           CSS
  theme: ThemeMode          →    resolvedTheme: ResolvedTheme  →  CSS custom properties
  day_color_theme: ColorTheme        ↑                              (--cc-* vars)
  night_color_theme: ColorTheme      │
                               OS prefers-color-scheme
                               (when theme = "auto")
```

- **Backend**: `AppearanceConfig` in `config/types.rs` stores `ThemeMode` (dark/light/auto) and separate `ColorTheme` values for day and night
- **Frontend**: `uiStore.resolvedTheme` resolves the active theme. When `theme = "auto"`, the OS preference selects day or night palette
- **Tokens**: `src/theme/tokens.ts` maps each theme name to a set of CSS custom properties (`--cc-*`) covering UI chrome, map visualization, and map controls
- **Preview**: `ThemePreviewCard` renders mini app mockups; `ColorThemeCircle` shows 3-segment color swatches for quick selection

## Keyboard Shortcuts Overlay

Pressing `?` opens the `KeyboardShortcutsDialog`, a filterable grid showing all shortcuts grouped by category (Tools, Edit, View, Panels, File) plus reserved shortcuts. Includes a "Customize" button linking to Settings > Shortcuts.

## Grid Overlay

`GridOverlay.tsx` renders a coordinate grid on the map that scales with survey data. Grid spacing is chosen by the user from `[1, 5, 10, 25, 50, 100]` meters in the Display menu (default 10 m). The grid subscribes to `resolvedTheme` for theme-aware colors via the `--cc-grid-default` CSS variable.

## Rust Backend Patterns

### Module size (file-size ratchet)

Non-test `.rs` files under `src-tauri/src` and `src-tauri/crates/*/src` are kept
under a size budget by a CI gate
(`check` job → `pnpm lint:rust-file-size`, `scripts/check-rust-file-size.mjs`) that
ratchets against `src-tauri/file-size-baseline.json` (baseline keys are the
repo-relative paths under both roots). It **fails** if a file at/above
1500 production LOC grows past its recorded baseline, if a *new* file is introduced
at/above 1500 prod LOC, or if a new/grown file crosses the 2000-prod-LOC hard cap.
Files below 1500 grow freely (ordinary edits don't need a rebaseline). Keep modules
focused; a file approaching ~1000 prod LOC should split at an API boundary (the
module-split effort is tracked on #157).
The baseline is a high-water mark: a legitimate growth bumps it, and a split lowers it,
**in the same PR** (`pnpm lint:rust-file-size --update`) so the change is visible in review
(shrinks aren't auto-captured). Metric = code LOC minus comments/blanks and `#[cfg(test)]`
items.

The frontend has an identical twin - `pnpm lint:ts-file-size`
(`scripts/check-ts-file-size.mjs`, baseline `ts-file-size-baseline.json`), same 1500/2000
thresholds, also a `check`-job gate. Both must pass before pushing.

### Mutex Lock Ordering

**Critical**: Never hold a Mutex lock while calling methods that acquire other locks. Release the first lock before calling methods that may acquire others.

**Pattern to avoid**:
```rust
// BAD - holds lock while calling other method
if let Ok(config) = self.config.lock() {
    self.do_something(); // may try to lock config again → deadlock!
}

// GOOD - release lock before calling
let value = self.config.lock().map(|c| c.some_field).unwrap_or(default);
self.do_something_with(value);
```

Where several are held, the order is `project` -> `file` -> `undo_stack` ->
`derived`. `std::sync::Mutex` is not reentrant: a caller that already holds the
`project` guard uses the `_at` / `_held` variants
(`persist_local_state_at`, `speleodb_mutate_link_held`) instead of the ones that
take it. The full rules are in [Undo/Redo, Lock Ordering](undo-redo.md#lock-ordering).

### Safe File Save

Every write of a project file goes through `stalx_v2::write_atomic`
(`persistence/stalx_v2/atomic.rs`):
1. Write `<file>.tmp` beside the destination and `sync_all` it
2. Check the external-modification guard (the file on disk must still be the one last read or written)
3. Rename the temp over the destination, then sync the folder
4. On any failure, delete the temp; the destination is untouched

The file on disk is always either the old or the new version. See
[Saving](saving.md).

### Positions are derived

Nothing positional is stored. Each connected piece of the survey has one
anchor: its first datum in list order (at its GPS in the frame's local metres,
plus the datum's `correction`), else its `Placement`. Every station's raw
position is the anchor plus the sum of shot vectors along the chain
(`Project::raw_positions`, solver v1); the Closures Computed View is a
least-squares solve over it. Commands keep things still through one post-edit
anchor algorithm (`Project::settle_anchors`, `survey/anchors.rs`): datum pieces
stay on their datum, a free piece keeps its edited line's first station where
it was, a merge moves the free side onto the placed one.

The frame is the first resolvable datum in list order; local (0, 0) is its
station. When the frame changes (its datum removed or moved down the list, or
the first datum set), nothing moves relative to anything else: the survey is
translated rigidly so the new frame station is at the origin, every
`Placement` translated with it and every placement datum's `correction`
re-measured so its piece keeps its place despite tangent-plane non-additivity
(plan 4.3). A GPS edit of a placement datum clears its correction and places
its piece on the new GPS; a new GPS on the frame datum's own station moves the
other pieces by the change, as before (#769).

### Session shot ids

Shot ids are generated on load: `line_id * FEATURE_ID_STRIDE + sequence_order`
(`FEATURE_ID_STRIDE` = 100000, `src-tauri/crates/models/src/id_stride.rs`, mirrored in
`src/utils/ids.ts`). They are therefore *not* stable identity - use `Shot::uuid` for
anything that has to survive a save/load. The history never records them: an
entry captured in the session carries them in memory only, and an entry read from
a file mints fresh ones for the shots it re-creates.

### Coordinate Transformation (Zero Datum)

When a station is set as zero datum with GPS coordinates, the system converts GPS (WGS84) to local ENU (East-North-Up) meters:

**Algorithm** (in `src-tauri/crates/models/src/geo.rs`):
1. **Geodetic → ECEF**: Convert lat/lon to Earth-Centered Earth-Fixed coordinates using WGS84 ellipsoid
2. **ECEF → ENU**: Rotate ECEF difference to local tangent plane at origin
3. **Result**: (east, north) offset in meters relative to zero datum

**Key functions**:
- `geodetic_to_ecef(lat, lon, alt)` - WGS84 to ECEF (public)
- `ecef_to_enu(dx, dy, dz, lat0, lon0)` - ECEF diff to ENU (private)
- `gps_to_local(gps, origin)` - Main entry point returning (east, north)

**Important notes**:
- Altitude is always 0; `depth_reference` is meters below surface, not ellipsoid height
- The "Up" component is discarded; caves use depth measurements instead
- A datum's `correction` keeps its piece where it was when the frame changes (above)

## Global View Data Flow

The Global View renders survey geometry for multiple GPS-positioned systems:

```
User loads Global View
    │
    ▼
GlobalView.tsx (container)
    ├── Reads recentProjects (ProjectMetadata[], IPC list_recent_projects)
    ├── Filters to GPS-positioned systems (gps_center != null)
    └── Renders GlobalViewMap
            │
            ▼
GlobalViewMap.tsx
    ├── Initializes MapLibre with Esri World Imagery via stalagmight-tiles:// protocol
    ├── Restores persisted map position from localStorage
    ├── Creates GlobalSurveyLayer for geometry rendering
    └── On map moveend → loadGeometryBatch(visible systems)
            │
            ▼
globalViewStore.ts (Zustand)
    ├── loadGeometryBatch() sequences geometry fetches
    ├── Calls get_survey_geometry_for_file IPC for each system
    ├── Tracks loading/loaded/error state per system
    └── Stores SurveyGeometry in Map<filePath, geometry>
            │
            ▼
GlobalSurveyLayer.ts
    ├── Receives geometry updates via update()
    ├── Hands buffers to GlobalCustomController (custom WebGL; no MapLibre sources)
    ├── Owns event wiring + tooltip, resolving hits from the geometry hit index
    ├── Applies search filter via setSearchFilter()
    └── Updates selection highlighting via setSelectedSystems()
```

### Tile Protocol (cross-platform)

Map tiles route through the `stalagmight-tiles` custom protocol, served by the
Rust handler registered in `src-tauri/src/lib.rs` via
`register_asynchronous_uri_scheme_protocol` (SQLite caching + offline support).
The frontend builds the MapLibre source URL in `src/utils/tileConfig.ts`
(`getTileConfig`).

The URL form **differs by platform** - this is a required gotcha, not a
preference:

| Platform | Tile URL template |
|----------|-------------------|
| macOS / Linux (WRY) | `stalagmight-tiles://<provider>/{z}/{y}/{x}` (provider = host) |
| **Windows (WebView2)** | `http://stalagmight-tiles.localhost/<provider>/{z}/{y}/{x}` (provider in path) |
| Android | `stalagmight-tiles://<provider>/{z}/{y}/{x}` via MapLibre `addProtocol` (see `tileProtocol.ts`) |

**Why:** WebView2 cannot navigate a bare custom URI scheme, so it serves
Tauri-registered protocols only under `http://<scheme>.localhost/...`. Using the
bare `stalagmight-tiles://` scheme on Windows means requests never reach the Rust
handler and **tiles silently fail to load** (no error, just blank tiles) while
macOS/Linux work fine. The Rust `parse_tile_uri` accepts both host-based and
path-based forms, so only the frontend URL needs to branch on platform
(`navigator.userAgent.includes('Windows')`).

### View Mode Toggle

The `useViewMode` hook persists view mode (`survey` | `markers`) to localStorage:

```typescript
const [viewMode, setViewMode] = useViewMode();
// - 'survey': shows geometry; markers hidden when geometry loaded,
//   visible with spinner during loading, visible (red) on error
// - 'markers': shows all markers (no geometry), all interactions preserved
```

Marker visibility is controlled by three effects:
1. **Creation effect** - sets initial `el.style.display` based on `viewModeRef.current` and geometry state
2. **Styles effect** - updates marker colors (selection, error) without recreating markers
3. **Visibility-sync effect** - toggles `el.style.display` (`'none'` / `'flex'`) on `viewMode` and `geometryStates` changes

Switching to markers mode cancels pending geometry loads to save resources.

### FPS Monitoring

`src/hooks/useFpsMonitor.ts` tracks frame rate with `requestAnimationFrame` and fires
`onThrottleNeeded` / `onThrottleRestore` around a threshold pair (default 30 / 45 fps).

It currently has **no consumer**. The auto-throttle it was written for increased
whole-source GeoJSON simplification in Global View; that source no longer exists, so there
is nothing left to coarsen. Wire it to something real or delete it - do not assume Global
View is still throttling itself.
