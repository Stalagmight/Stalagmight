# Stalagmight Architecture

> For full data model, UI design, and workflows, see [SPEC.md](SPEC.md).

## Workspace layout

The Rust backend is a cargo workspace rooted at `src-tauri/Cargo.toml` (a
hybrid root: the `stalagmight` app package lives there too). Nine library
crates sit under `src-tauri/crates/`:

| Crate | Contents | Key deps |
|-------|----------|----------|
| `stalagmight-models` | The data model: `Project` / `Section` / `Shot` / `Station`, geometry (`geo.rs`), closures/merges/offsets, `app_dirs`, gas config + engine | - (base crate) |
| `stalagmight-telemetry` | Telemetry collector + event registry (`telemetry/`), tracing/logging setup (`logging/`); its `build.rs` owns `TELEMETRY_ENDPOINT` validation and `APP_VERSION` | models |
| `stalagmight-parsers` | Import parsers: DMP, Ariane TML/TMLU, Compass, Walls, GPX | models |
| `stalagmight-persistence` | `.stalx` SQLite store + `.plnx` dive-plan store (`persistence/`), tile caches (`cache/`, `tilemap_cache`) | models |
| `stalagmight-export` | CSV / JSON / image (SVG->PNG/PDF) / TML export, dive profile | models, parsers |
| `stalagmight-speleodb` | SpeleoDB client: HTTP surface, auth/token storage, dump handling, sync, TLS policy, metrics | models, telemetry |
| `stalagmight-core` | `AppState`, the command executors, undo/redo, project I/O orchestration, config I/O (`config/`), `tile_providers`; shared test helpers behind the `test-support` feature | export, models, parsers, persistence, speleodb |
| `stalagmight-tiles` | The map-tile engine behind the `stalagmight-tiles://` protocol and `fetch_tile`: RFC 9111 caching client over the persistence tile caches, retrying fetch, ArcGIS tilemap probe, stale-if-error, overzoom fallback. Tauri-free, so `cargo test -p stalagmight-tiles` never builds the app | core, persistence |
| `stalx-reader` | Read-only `.stalx` reader for external consumers (SpeleoDB server-side maps, #593); nothing in the app depends on it | models, persistence |
| `stalagmight` (app, at `src-tauri/`) | The thin Tauri layer: `commands/` (IPC handlers), `lib.rs` (the single `generate_handler!`, the tile protocol registration), `main.rs`, `tile_attribution.rs`, `webkit_memory.rs`, `examples/`, `tests/` | all of the above except stalx-reader |

The layering is strict and one-way - models -> telemetry / parsers /
persistence -> export / speleodb -> core -> tiles -> app, where `->` reads "is
depended on by" - so a lower crate never imports from a higher one. The workspace root also carries the shared
`[workspace.dependencies]`, the `[workspace.lints]` policy every member
inherits, and the `telemetry` feature, which forwards to the member that
implements it (`stalagmight-telemetry`).

## State Flow

```
User Action → Zustand Store → Tauri IPC → Rust Command Executor
     ↑                                              │
     └──────────── State Update ←──────── Command Result
```

### Reading the project back: binary for the app, JSON for the harness

Two commands return the open project with its stations, stats and frozen shots,
and both are kept on purpose (#576 REFRESH-CMDS):

- **`get_project_state_binary`** is the app's own read. `refreshProjectState`
  issues it after every edit; it is the load's msgpack + typed-array envelope
  (`core/load_binary.rs` `encode_refresh`, decoded by `utils/loadProjectBinary.ts`),
  and an empty answer means no project is open. The load itself uses
  `load_project_full_binary`, the same layout.
- **`get_project_state`** is the same read as JSON. Nothing in `src/` calls it.
  It stays for the agent harness (`e2e/agent/`: the journeys, the fuzz and import
  oracles read the project as data) and for debugging, where a readable payload
  from the devtools console beats a byte envelope. Do not remove it with the app's
  last caller gone, and do not route the app back through it: at 75,000 shots it
  is ~36 MB of JSON per edit (docs/performance.md, "The Refresh After an Edit").

Both take the settled snapshot and share `measured_state` (`commands/project.rs`),
so the two cannot describe different projects. Shot readings in the binary form
are f32 (the PRECISION note in `load_binary.rs`), which is why the edit paths send
only the fields the user changed (#576 F32-EDIT).

## Command Pattern

All persistent operations go through the Rust `Command` enum in
`src-tauri/crates/core/src/core/command.rs`. `AppState` (`src-tauri/crates/core/src/core/state/app_state.rs`)
executes them and manages undo/redo; the executor bodies are split one module per command
family (`command_shot.rs`, `command_station.rs`, `command_move.rs`, ...) under
`src-tauri/crates/core/src/core/state/`.

## Command results and errors

Every fallible IPC command returns `Result<T, CommandError>` (#576 C4). There is
one error envelope: `CommandError` (`crates/core/src/core/command_error.rs`) carries
a `code: CommandErrorCode` (`crates/models/src/command_error.rs`, one ts-rs enum for
the whole app, A6), a display-only `message`, and optional payloads flattened into
the same wire object (`details` and `speleodb_managed` for an open, the SpeleoDB
`status` / `transport` / `unpushed_local` / `dump_candidates`, `history_step` and
`reversed_sections` for a failed undo/redo, `session_closed`, `remote_link`). The
frontend routes on the code and never on the message: the string markers that used
to be prefixed onto the text (`EDIT_DISABLED: ...`, `FS_PERMISSION_DENIED: ...`) and
matched with `startsWith` are gone. The subsystems keep their own error types where
they need them and carry the same code: `LoadError` (open / create) and the SpeleoDB
crate's `SpeleoErrorDto` / `SpeleoDbError::Refused` convert into `CommandError`
losslessly at the boundary. A command that cannot otherwise fail (`can_undo`,
`check_project_integrity`, `set_workspace_mode`) answers a `Result` too, so a body that
crashes on the pool still answers, with `INTERNAL_ERROR`: the frontend awaits such
calls inside its edit queue, and one that never answered held every later edit behind
it (#576 RV5-6). `get_config` does too (#576 NF-W7E-5; it used to re-raise the crash,
so the call never resolved). One stays infallible: `get_network_status` (a crash reads
as offline). The settings
write commands in `commands/config.rs` still return `Result<_, String>` (rejected as a
plain string), except `save_display_setting`, whose refusal of a write made for
another project is coded (`PROJECT_CHANGED`, #576 RV6b-2).

The success side is per command (A12). `AppState` methods still build their answer
in the internal `CommandResult` accumulator, which no longer crosses the IPC: each
command converts it with `CommandResult::into_outcome::<T>()` into the type that says
exactly what it reports - `EditOutcome`, `SectionCreated`, `ShotCreated`,
`ReverseOutcome` (a parity-reduced `reversed_sections`), `BatchOutcome`,
`HistoryOutcome` (`reversed_steps`, the ordered per-step log a consumer composes),
`ImportOutcome`, `SaveOutcome`, `DeleteProjectOutcome`, `ProjectFileOutcome`
(`crates/core/src/core/outcome.rs`) - and a failure into a `CommandError` with the
producer's code (`CommandResult::refused`).

**SpeleoDB link bits (D22).** Every survey mutation answers through
`commands/pool.rs` `run_mutation`, which attaches `remote_link` (`has_unpushed_edits`,
`unwound`, read by `AppState::remote_link_bits` right after the command) to its
outcome and to its error. The frontend applies it in one place, `invokeIpc`
(`src/utils/ipc.ts`), so no editing path keeps its own copy of the backend's dirty
bookkeeping (the copies used to drift: a provable no-op such as reversing a shot-less
section marked the copy un-pushed on screen while the backend stayed clean). A source
tripwire fails when a command whose answer carries the bits is not run through
`run_mutation`.

## Where commands run

Tauri runs a plain (non-`async`) `#[tauri::command]` inline in its IPC handler, on
the **main thread**: the window cannot repaint, scroll or deliver an event until it
returns, and every later IPC call - `cancel_loading` included - waits behind it. So
no command does its work there (#576 C2). Every command is `async`, generic over the
Tauri `Runtime`, takes the `AppHandle`, and hands its body to the blocking pool
through `commands/pool.rs` (`pool::run` / `run_mutation`, and `run_stateless` for a body
that needs no `AppState`); the main thread only
parses the arguments.
The SpeleoDB commands do the same through their own `run_blocking`. A contract test
(`app_contract_tests.rs`) fails when a registered command is not `async`.

Because they are generic, tests call the registered command itself with a
`tauri::test` app (`run(cmd(h(&app), ..))` in `command_tests.rs`); there are no
`*_impl` twins for tests to call instead (#576 C9).

Commands now run **concurrently**, which makes two rules explicit:

- **Mutations stay serialized.** Every project mutation (`execute`, undo/redo, the
  imports, repair, settings, save, the project-slot commits) takes `command_lock`
  for its whole span, as before - see [undo-redo.md](undo-redo.md#lock-ordering). So
  does a write into a side table a save carries (a display setting, an Ignore or
  Un-ignore): the save rebuilds the file from a snapshot of those tables and renames
  it over the open one, so a write landing in between was lost (#576 RV5-1). The
  frontend additionally issues its edits one at a time through its mutation queue
  (`store/mutationQueue.ts`), so they apply in the order the user made them.
- **A read of the project is a settled read.** A command mutates the project and
  then settles it (orphan conversion, frame renormalisation, the reconcile, the undo
  push) under one `command_lock` span but several `project` acquisitions, so a read
  that takes only the `project` lock can catch a project that was changed and not
  yet settled - an edit without its undo entry, offsets not yet re-solved. Every IPC
  read of the project therefore either takes `AppState::settled_snapshot()` (the
  snapshot is taken under `command_lock`, then read with no lock held - for anything
  that does real work on it) or runs a short query under `AppState::settled_read`.
  Never call either while holding `command_lock` (it is not reentrant); inside a
  command, read `project_snapshot()`. Reads of other cells (config, recent projects,
  display settings, logs, caches) take only their own lock.

The open project is held copy-on-write (`core/state/project_slot.rs`, #576 A9): a
snapshot is an O(1) `Arc` clone that later edits never change, and a writer copies
the project only while a snapshot of it is still alive. `get_project_state()` is the
explicit deep copy, for the few callers that must own the project.

`cancel_loading` is not a command in this sense: it flips the active load's token
and takes no `command_lock`, so it is never queued behind the import or load it
cancels (`commands/import_offthread_tests.rs`, `core/state/command_ordering_tests.rs`).

## IPC trust boundary

The custom Tauri commands take paths and payloads straight from the webview, and
`src-tauri/capabilities/default.json` does not bound them: a capability grants the
**plugin** commands the page may call (the dialog and opener plugins), never the
app's own `#[tauri::command]`s. The fs plugin is not registered at all (#576
NF-W12FS-2): the two file operations the page used to ask it for - whether a save
would replace a file, and the corruption-recovery salvage write - are app commands
(`save_target_exists`, `write_salvaged_data`), because its scope held no user path,
so both failed for every path in the real app (#576 HFI-2, HFI-3). Nor does `withGlobalTauri` decide
reachability: `window.__TAURI_INTERNALS__.invoke` is injected regardless (see
docs/app-modes.md).

The real boundary is that **only the app's bundled frontend runs in the webview**:
a strict CSP (`script-src 'self'`, no inline script, no remote origin; see
`tauri.conf.json`) and React's escaping of survey text are what keep a script out.
Defence in depth for the day that fails (#576 C11-D1): every command that writes,
replaces or deletes a file at a path it is handed - the exports (CSV, JSON, Ariane,
image, dive profile), Save As, create, merge (and the sources it archives), delete,
rename-by-path, restore from backup, repair, the salvage write and the SpeleoDB
unlink - first runs `commands/write_path.rs` `check_write_path`: the file must have
the extension of
what is written, must not be inside the app's own configuration, data or cache
folders (the SpeleoDB managed-project cache excepted, for projects), and must not
be inside a system folder (`/etc`, `/usr`, `/bin`, ..., `C:\Windows`,
`C:\Program Files`). The list is deliberately narrow - `/home`, `/var`,
`/run/media`, `/mnt`, `/media`, `/opt` and `/tmp` are places users save to.
On Windows the path must also name a plain file in a folder, judged from its
text before the disk is asked (#576 W24-D1): a device, volume or drive-relative
path (`\\?\GLOBALROOT\...`, `\\?\Volume{...}\...`, `\\.\pipe\...`, the NT prefix
`\??\`, `C:x.stalx`), an alternate data stream (a `:` after the drive:
`notes.txt:x.stalx`, `C:\Windows:x.stalx`), a device name (CON, PRN, AUX, NUL,
COM0-9, LPT0-9, COM and LPT with the superscript digits ¹ ² ³, CONIN$, CONOUT$,
in any case, with any extension, trailing dots or spaces) and this computer
reached as a network share (`localhost`; 127.0.0.0/8 in any spelling Windows reads
as an IPv4 address, such as `127.1`, `2130706433`, `0x7f000001` or `0x7f.1`;
`::1`; its own name) are refused, as are older-file upgrades there. The `\\?\`
and `\\.\` forms of a drive or UNC path, `UNC` in any case, name the plain
folder, and a name is compared as Win32 opens it (`C:\Windows.\` is
`C:\Windows\`), except in a `\\?\` path.
A folder counts however the path reaches it: the path is compared folded and as
the write reaches it on disk, one component at a time, links followed - so
`<dir>/missing/../link-to-the-app-folder/x`, which a write that makes the missing
folders first goes through, is refused like the link itself (#576 RV12-1).
Opening a project (`load_project_full`, `load_project_full_binary`) opens it
wherever it is - viewing and exporting a project kept in the app's folders or a
system folder work as anywhere else - but it is not always read-only: an
older-schema file is upgraded in place with a `.pre-v<N>.backup` copy written
beside it. That write alone is checked (#576 RV4-R-M3-OPEN): the loads ask
`load_upgrade_refusal` (the same two folder rules, without the extension rule -
the open dialog offers "All Files"), and in those folders an older file is refused
before anything is written (`Database::open_existing_refusing_upgrade`, load error
`UPGRADE_REFUSED`) with a message saying to copy the project somewhere writable;
a current or newer file opens untouched. Commands that only read keep taking any
path, with one exception: the overwrite prompts' existence probe
(`save_target_exists`) answers only for a path the write after it may write (the
same three rules), so it tells a page script nothing about the app's own or a
system folder; a failed probe is an error, and the prompt then asks as if the file
were there (#576 E10-D1), while a refused one (`WRITE_PATH_REFUSED`) goes straight
to the write, which refuses it with the reason (#576 HFI-2-D2). The New Project
pre-check (`check_create_target`, #576 RV15-2) is the create's own check run
alone - same file, same answer, nothing created - so the webview can refuse a
create before it cancels a load in progress or shows its loading state; it tells
a page script nothing `create_project_full` would not.

## Data Model

- **Project** (`src-tauri/crates/models/src/project.rs`): the whole in-memory document - the
  sections, plus everything that cross-references them.
  - `station_merges`, `zero_datums`, `loop_closures` - the cross-section ties. All three
    are held with position-independent `StationKey` endpoints and re-encoded to the
    positional wire form only at IPC encode, so inserting or deleting a shot cannot
    silently rebind them. Closure detection and adjustment: [closures.md](closures.md).
  - `section_offsets` / `station_offsets` - displacements applied when sections are
    aligned through merges, or when a station is moved by hand.
  - `station_comments` - free text keyed by `StationKey`. A station is implicit (the
    boundary between consecutive shots), so it has no record of its own to hang a
    comment on.
  - `validation` - per-project thresholds, stored in the `.stalx` so they travel with the
    file between collaborators instead of following whoever opens it.
  - `imported_file_hashes` / `imported_section_hashes` - re-import de-duplication.
- **Section**: Group of shots with metadata (name, surveyor, date, color)
- **Shot**: Single measurement (azimuth pair, length, depth in/out, pitch, LRUD) plus its
  `ShotKind` - a permanent leg, a swum jump, or a survey reference. That distinction is
  what the reachability graph and the gas engine weight on.
- **Station**: Computed from shots, can be merged or set as zero datum

Two things a project owns are deliberately *not* fields on `Project`. The SpeleoDB link is
a single `speleo_link` row inside the `.stalx` (`src-tauri/crates/models/src/speleo_link.rs`). A
linked project is usable at exactly one path - the pull cache,
`<cache>/speleodb/<project_id>/project.stalx` - and two seams enforce that:
`do_load_project_full` refuses to open a linked file anywhere else
(`core/state/speleodb_foreign.rs`), and `save_project` carries the row forward only on an
implicit save (the save writes a fresh file, so without the carry-forward a local save
would silently un-remote the project and clear its un-pushed-edits bit); an explicit
destination on a linked project instead writes a detached copy WITHOUT the row and keeps
the session on the managed file (docs/saving.md, "Save a Copy"). Named dive plans live in
a sibling `<stem>.plnx` file.

## Subsystems

Most have their own page. What follows is the seam and where the code lives, nothing more.

**Workspace modes.** `uiStore.appMode` (`survey` | `plan` | `export`) switches the whole
workspace from the `ModeCarousel`; Plan and Export are read-only. Ask whether editing is
blocked through `useEditingBlocked()` / `isEditingBlockedNow()`
(`src/hooks/useEditingBlocked.ts`) and nothing else - `pnpm lint:readonly-gates` bans the
raw composition, because writing it inline as `useA() || useB()` short-circuits the second
store hook and violates the Rules of Hooks. Note the asymmetry: the map-read-only term has
no backend backstop - a local project keeps `edit_enabled` true in Rust whatever mode the
UI is in - so a missed gate is a real bug, not a cosmetic one.
-> [app-modes.md](app-modes.md)

**Gas and dive planning.** `Project::compute_gas_reach_field`
(`src-tauri/crates/models/src/project/gas.rs`) seeds a gas-weighted Dijkstra over the same survey
graph the distance overlay uses, weighting each shot by the gas it costs to swim in, and
marks every station reachable within the turn budget. It runs on measured coordinates -
reachability is coordinate-independent, so it never triggers the slow closure-adjusted
solve. Inputs live in `src/store/gasPlanStore.ts` (runtime only, never persisted); named
plans persist to the sibling `.plnx` (`src-tauri/crates/persistence/src/persistence/plnx.rs`) and store the
engine's *input tuple*, never the derived field, so a loaded plan cannot show a stale
overlay. -> [gas-planning.md](gas-planning.md)

**Survey rendering.** Shots, stations, labels, gradients and the measured-path tint are
drawn by a custom WebGL layer (`src/components/Map/surveyLayer/customRender/`), which is
the *sole* survey renderer: the stock bulk layers were deleted, so there is no fallback -
an unrecoverable GL failure removes the dead layer and surfaces an error. Hit-testing does
not depend on MapLibre either; it runs against the SoA geometry index. What is still stock
MapLibre in `SurveyLayer.ts` (`surveyLayer/auxiliaryLayers.ts`) are the small GeoJSON auxiliaries: loop closures, the
gas-route highlight, merge ghosts, and the interact hover/selection overlay - the last of
which holds only the few features the user is currently touching, so its size tracks
interaction state, not survey size. Global View is custom-only too
(`src/components/GlobalView/customRender/globalController.ts`).
-> [map-renderer.md](map-renderer.md)

**SpeleoDB.** `src-tauri/crates/speleodb/src/speleodb/` holds the client half: HTTP surface, token storage
(OS keychain with an in-memory fallback), ZIP-dump classification and `.stalx` extraction,
the base-SHA fence and ancestry recovery. Orchestration against `AppState` lives in
`core/state/speleodb.rs`, the IPC commands in `commands/speleodb.rs`. The model is a
single-writer hand-off: a pulled project is read-only until the write lock is acquired, and
unlike the mode-driven read-only above, that one has a real backend backstop -
`AppState::guard_mutation` refuses every gated mutation while the lock is missing.
No subsystem page yet; the closest thing is the test-setup + verified-API-contract
guide -> [speleodb-dev-testing.md](speleodb-dev-testing.md)

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
| `src-tauri/crates/core/src/core/state/` | `AppState` (`app_state.rs`) plus one module per command family |
| `src-tauri/crates/persistence/src/persistence/sqlite.rs` | `Database` struct + `SCHEMA_VERSION` and its migration history; the impl is split across `persistence/sqlite/` (open, read, write, schema, integrity, undo, repair, settings, warnings) |
| `src-tauri/crates/persistence/src/persistence/plnx.rs` | `.plnx` sibling store for named dive plans |
| `src-tauri/crates/parsers/src/parsers/dmp.rs` | mNemo DMP file parser |
| `src-tauri/crates/models/src/project.rs` | Project model with merge/offset logic |
| `src-tauri/crates/models/src/project/gas.rs` | Gas-weighted reachability engine (`compute_gas_reach_field`) |
| `src-tauri/crates/models/src/geo.rs` | GPS-to-local coordinate transformation (WGS84 → ENU) |
| `src-tauri/crates/speleodb/src/speleodb/mod.rs` | SpeleoDB client module root (HTTP, auth, dump, sync, TLS policy) |
| `src-tauri/crates/persistence/src/cache/tiles.rs` | Satellite tile cache (SQLite-based) |
| `src-tauri/crates/core/src/tile_providers.rs` | Tile provider registry (provider metadata, URL expansion) |
| `src-tauri/src/commands/cache.rs` | Cache IPC commands (stats, clear, fetch) |
| `src-tauri/src/commands/survey_geometry.rs` | Survey geometry IPC command (`get_survey_geometry_for_file`) |
| `src/store/projectStore.ts` | Frontend state, Tauri IPC calls |
| `src/store/speleoStoreHandle.ts` | Late-bound handle projectStore and its slices use to reach speleoStore, which imports projectStore (no value-import cycle; `pnpm lint:import-cycles`) |
| `src/store/uiStore.ts` | UI state (selection, tools, panels, `appMode`) |
| `src/store/gasPlanStore.ts` | Gas/dive-plan inputs (runtime only, never persisted) |
| `src/store/globalViewStore.ts` | Global View state (selection, geometry loading) |
| `src/hooks/useEditingBlocked.ts` | The one sanctioned read-only predicate (map mode + SpeleoDB lock) |
| `src/components/Map/MapCanvas.tsx` | MapLibre map initialization |
| `src/components/Map/createSurveyMap.ts` | The one map factory for the survey map, Global View and section preview: shared options, rotation/pitch disabling, the Android tile protocol, satellite reload on reconnect |
| `src/components/Map/SurveyLayer.ts` | Survey layer: custom-renderer wiring, GeoJSON auxiliaries, interaction (helpers under `surveyLayer/`) |
| `src/components/Map/surveyLayer/customRender/controller.ts` | Custom WebGL survey renderer (buffers, colors, layer lifecycle) |
| `src/components/GlobalView/GlobalView.tsx` | Global View container component |
| `src/components/GlobalView/GlobalViewMap.tsx` | MapLibre map for Global View |
| `src/components/GlobalView/GlobalSurveyLayer.ts` | Multi-system survey geometry rendering |
| `src/components/GlobalView/ViewModeToggle.tsx` | View mode toggle and `useViewMode` hook |
| `src/components/GlobalView/MetadataPanel.tsx` | System metadata display with aggregate stats |
| `src/utils/coordinates.ts` | Coordinate transforms (meters ↔ GPS) |
| `src/utils/tileProtocol.ts` | Android MapLibre tile protocol: registered once at app start, never removed |
| `src/utils/tileConfig.ts` | Tile source config; platform-specific `stalagmight-tiles` URL form |
| `src/hooks/useProject.ts` | Project file operations hook |
| `src/hooks/useMapTools.ts` | Map tool handlers |
| `src/hooks/useKeyboardShortcuts.ts` | Global keyboard shortcut handler |
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

Whole **test files** are measured too, under their own thresholds (warn 3000 / fail
5000 code LOC, the baseline's `test_files` section): a file under `src-tauri/tests/`, or a
`tests.rs` / `*_tests.rs` / `*_fuzz.rs` / `test_helpers.rs` that its parent declares
under `#[cfg(test)]` (a production file merely *named* `*_tests.rs` is measured as
production). The three test monoliths at/above 3000 (`commands/command_tests.rs`,
`core/state/fuzz_undo_tests.rs` and `models/project/gas/tests.rs`) are frozen: new tests go
into a new submodule, not into them. (`core/state/tests.rs`, once the largest, is now a
module root over topic files in `core/state/tests/`, #576 E1; `models/project/tests.rs` and
`persistence/sqlite/tests.rs` followed the same way, #576 AFK-D3.) The scan also covers the
first-party Rust outside the crates' `src/`: the build scripts, `src-tauri/examples`,
the agent harness's OS helper and `scripts/lib/slice-rs`.

The frontend has an identical twin - `pnpm lint:ts-file-size`
(`scripts/check-ts-file-size.mjs`, baseline `ts-file-size-baseline.json`), same 1500/2000
thresholds. It covers `src/` (tests and generated IPC types excluded) and every
`.ts`/`.tsx` under `e2e-playwright/` and `e2e/` (generated `e2e/agent/replays/`
excluded) - so the ~4.8k-LOC IPC mock is frozen too. It runs in the `check` job and the
`smoke` job (which sees `e2e-playwright/` changes). Both must pass before pushing.

### Mutex Lock Ordering

**Critical**: Never hold a Mutex lock while calling methods that acquire other locks. Release the first lock before calling methods that may acquire others.

**Pattern to avoid**:
```rust
// BAD - holds lock while calling other method
if let Ok(config) = self.config.lock() {
    self.do_something(); // may try to lock config again → deadlock!
}

// GOOD - release lock before calling
let value = self.config.lock().some_field;
self.do_something_with(value);
```

### Mutex Poisoning (one policy, #576 A11)

A panic that unwinds through a held guard POISONS the mutex: the data is still
there, possibly half-way through an edit, and std's `lock()` reports the crash on
every later call. Every lock in the backend crates is a
`models::poison_policy::RecoveringMutex`, whose only way in is the policy:

- `lock()` RECOVERS the data and returns the guard (never a `Result`). The first
  poisoned cell is logged once and raises the `internal-error` event once per
  session; the frontend shows a persistent "save your work and restart" banner
  (`InternalErrorBanner`). Saving keeps working.
- A short, named list of operations REFUSES instead, because continuing would
  replay or publish what the crash may have half-written: undo / redo / undo to /
  redo to, the SpeleoDB undo-chain walk, and the SpeleoDB upload. The save goes
  through but writes an empty Undo/Redo history (with the `UNDO_HISTORY_DROPPED`
  warning), so the restarted session cannot replay it either (#576 RV5-PI-2). The
  list and the reason for each live on `AppState::refuse_after_crash`; add to it
  there, not at a call site.

There is no method that returns std's `LockResult`, so `.lock().unwrap()`,
`.lock().expect(..)`, `if let Ok(..) = x.lock()` and `.lock().map_err(..)` do not
compile on a cell. `AppState`'s cells report to its own monitor; process-wide cells
(log ring buffer, telemetry, tile caches, the launch queue) report to
`poison_policy::global_monitor()`, which app setup forwards into the managed
state's monitor so the user sees one banner. `poison_policy_tests.rs` enumerates
every `AppState` cell by exhaustive destructuring, poisons each and drives the
public API through it. A lock outside `AppState` is not in that list, so
`models/src/raw_lock_scan_tests.rs` scans the backend sources and fails on any std
`Mutex`/`RwLock` in production code (test code and `poison_policy.rs` itself
excepted): wrap a new lock in `RecoveringMutex` (#576 RV5-PI-5).

### Safe File Save

Always use temp file + atomic rename pattern for saves:
1. Write to `file.stalx.tmp`
2. On success, rename to `file.stalx`
3. On failure, delete temp file

This prevents data loss if save fails mid-write.

### Section Offset Tracking

When stations are merged, section positions are recalculated:
1. Build graph of section connections through merges
2. Anchor section (first or with zero datum) stays fixed
3. Calculate offsets for connected sections
4. Store in `Project.section_offsets: HashMap<SectionId, (f64, f64)>`

Offsets are **derived** state that the file merely caches, which is why
`redrive_offsets_after_load` (`src-tauri/crates/models/src/project/offsets.rs`) re-solves them
from the merge and datum constraints on every open. A project therefore always opens
geometrically correct even if an older build persisted drift.

The offsets are read against ONE frame: local (0,0) means the FRAME datum's GPS -
the first entry of `zero_datums` that still resolves (`Project::frame_datum`; a
dangling datum is dropped from the wire and anchors nothing).
A command that promotes a different datum to first - or ADDS the survey's first
datum, which establishes the frame - therefore re-defines every stored coordinate,
and the answer is always the same: translate every section, free-floating
components included, so that datum's station lands on the origin
(`Project::renormalize_frame`, issue #469). The add does it inside
`Project::set_zero_datum` (the subsequent-datum placement reads that invariant, and
the import loop and redo never reach an epilogue); every other promotion does it in
the forward epilogue (`AppState::forward_epilogue`, which every forward entry point
runs, #576 A4). It runs forward only:
`resolve_offsets`' cascade never re-derives a seed's offset, so a whole-project
translation is a no-op for the load path, and every affected command's undo restores
the recorded map rather than re-deriving a frame.

### Database Operations

- Use `Database::open_existing()` for reading (doesn't create files)
- Use `Database::open()` for writing (creates if needed)
- Shot IDs are generated on load: `section_id * FEATURE_ID_STRIDE + position`, the
  shot's 0-based place in the stored `sequence_order` (the two agree for every file
  the app writes; a foreign file's gaps are closed). `FEATURE_ID_STRIDE` = 100000,
  `src-tauri/crates/models/src/id_stride.rs`, mirrored in `src/utils/ids.ts`; a file
  with a section of more than `FEATURE_ID_STRIDE - 1` shots is refused. They are
  therefore *not* stable identity - use `Shot::uuid` for anything that has to survive
  a save/load

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
- `local_to_gps(east, north, origin)` - the inverse, `Option<(lat, lon)>` (below)
- `unwrap_longitude(lon, reference)` - the explicit drawing step (below)

**The inverse, one algorithm** (#576 GEO-D1): `local_to_gps` finds where the
origin's vertical through (east, north) meets the WGS84 surface - a quadratic
for the up offset, then the closed-form latitude of a surface point. It is exact
to rounding (the round trip through `gps_to_local` is within nanometres out to
50 km), returns the longitude in [-180, 180], and is `None` only beyond the
horizon (about an earth radius out) or for a non-finite input. The frontend's
`enuToGps` (`src/utils/coordinates.ts`) runs the same solve operation for
operation, so the workspace map, the data table's GPS column and Global View put
a station on the same fix (#576 D36). The parity table `FRONTEND_PARITY` in
geo.rs is pinned to the backend there and read from source by
`src/utils/__tests__/coordinates.test.ts`, which holds `enuToGps` to it: the two
sides agree to within 1e-9 degrees.

| Caller | Longitude | Beyond the horizon |
|--------|-----------|--------------------|
| Global View (`survey_geometry.rs`) | `unwrap_longitude`: continuous with the origin across the antimeridian, may leave [-180, 180] | the project shows as failed to load, with the reason |
| Workspace map (`metersToMapCoords`) | `unwrapLongitude`, the same step | pinned near the horizon in its own direction (the vertical's closest approach to the ellipsoid, a point above the surface), so the camera and bounds stay finite |
| GPS column (`xyToGps`) | [-180, 180] | blank cell |
| TML import, datum carried past dropped shots | [-180, 180] (a storable `GpsCoordinates`) | anchored at the file's own fix, with a log warning |
| stalx reader (`stalx-reader`) | [-180, 180] | station left without coordinates |

**Important notes**:
- Altitude is always 0; `depth_reference` is meters below surface, not ellipsoid height
- The "Up" component is discarded; caves use depth measurements instead
- Section offsets are tracked in `Project.section_offsets` to maintain visual positions after datum changes

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
`register_asynchronous_uri_scheme_protocol` (SQLite caching + offline support);
the engine it calls is the `stalagmight-tiles` member (`src-tauri/crates/tiles`).
The frontend builds the MapLibre source URL in `src/utils/tileConfig.ts`
(`getTileConfig`).

The URL form **differs by platform** - this is a required gotcha, not a
preference:

| Platform | Tile URL template |
|----------|-------------------|
| macOS / Linux (WRY) | `stalagmight-tiles://<provider>/{z}/{y}/{x}` (provider = host) |
| **Windows (WebView2)** | `http://stalagmight-tiles.localhost/<provider>/{z}/{y}/{x}` (provider in path) |
| Android | `stalagmight-tiles://<provider>/{z}/{y}/{x}` via MapLibre `addProtocol` (see `tileProtocol.ts`; registered once at app start for every map, never unregistered) |

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
1. **Creation effect** - runs only when the set of systems (or the theme) changes; builds
   each marker once and sets its initial style and `el.style.display` from refs
   (`viewModeRef`, `geometryStatesRef`, `selectedProjectIdRef`). It must not depend on
   loading state or selection: that rebuilt every marker on every system load (#576 D10)
2. **Styles effect** - updates marker colors (selection, error), the loading spinner and
   `data-loading-state` in place
3. **Visibility-sync effect** - toggles `el.style.display` (`'none'` / `'flex'`) on `viewMode` and `geometryStates` changes

Switching to markers mode cancels pending geometry loads to save resources. A system
whose load failed is not retried on its own (the known causes do not heal); it stays a
red marker until the user clicks it, the recent list reloads, or the view switches back
to survey (#576 D10-D1).

### FPS Monitoring

There is no FPS-driven throttle. The `useFpsMonitor` hook written for Global View's
whole-source GeoJSON simplification lost its purpose with that source and was deleted
(#576 D27-32(e)); the live FPS readout is `useMapDevStats`' own loop.
