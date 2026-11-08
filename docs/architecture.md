# Stalagmight Architecture

> For full data model, UI design, and workflows, see [SPEC.md](SPEC.md).

## Workspace layout

The Rust backend is a cargo workspace rooted at `src-tauri/Cargo.toml` (a
hybrid root: the `stalagmight` app package lives there too). Eight library
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
| `stalx-reader` | Read-only `.stalx` reader for external consumers (SpeleoDB server-side maps, #593); nothing in the app depends on it | models, persistence |
| `stalagmight` (app, at `src-tauri/`) | The thin Tauri layer: `commands/` (IPC handlers), `lib.rs` (the single `generate_handler!`), `main.rs`, `tile_attribution.rs`, `webkit_memory.rs`, `examples/`, `tests/` | all of the above except stalx-reader |

The layering is strict and one-way - models -> telemetry / parsers /
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
`src-tauri/crates/core/src/core/command.rs`. `AppState` (`src-tauri/crates/core/src/core/state/app_state.rs`)
executes them and manages undo/redo; the executor bodies are split one module per command
family (`command_shot.rs`, `command_station.rs`, `command_move.rs`, ...) under
`src-tauri/crates/core/src/core/state/`.

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
MapLibre in `SurveyLayer.tsx` are the small GeoJSON auxiliaries: loop closures, the
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
| `src/store/uiStore.ts` | UI state (selection, tools, panels, `appMode`) |
| `src/store/gasPlanStore.ts` | Gas/dive-plan inputs (runtime only, never persisted) |
| `src/store/globalViewStore.ts` | Global View state (selection, geometry loading) |
| `src/hooks/useEditingBlocked.ts` | The one sanctioned read-only predicate (map mode + SpeleoDB lock) |
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
the forward epilogue. It runs forward only:
`resolve_offsets`' cascade never re-derives a seed's offset, so a whole-project
translation is a no-op for the load path, and every affected command's undo restores
the recorded map rather than re-deriving a frame.

### Database Operations

- Use `Database::open_existing()` for reading (doesn't create files)
- Use `Database::open()` for writing (creates if needed)
- Shot IDs are generated on load: `section_id * FEATURE_ID_STRIDE + sequence_order`
  (`FEATURE_ID_STRIDE` = 100000, `src-tauri/crates/models/src/id_stride.rs`, mirrored in
  `src/utils/ids.ts`). They are therefore *not* stable identity - use `Shot::uuid` for
  anything that has to survive a save/load

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
