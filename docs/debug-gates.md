# Debug Gates

Stalagmight ships with a fair amount of instrumentation - phase timings on the
load path, per-section progress markers during TML / Compass / DMP import,
console traces in the React store, and `debug_assert`-style cross-checks in the
project model. Most of it is **off by default in production**: the
`#[cfg(debug_assertions)]` (Rust) and `import.meta.env.DEV` (Vite) machinery
compiles to nothing in release. The backend `debug!` stream is the exception -
the default subscriber filter is `info,stalagmight=debug`, so it still reaches
the log in release builds unless you override `RUST_LOG`. This page is the map
of where each gate lives and how to turn the noise back on when you actually
want to see it.

Read this before adding new diagnostic code. The cost of a stray `tracing::info!`
or `console.log` on a per-shot / per-merge hot path is *real* - the import work
in [`performance.md`](performance.md) lists cases where untrimmed logs were a
measurable fraction of the wall clock.

## Rust backend

### Levels at a glance

| Level | When it fires by default | Use for |
|-------|--------------------------|---------|
| `error!` | always | Conditions a user should care about (corruption, save failed, FK violation). |
| `warn!`  | always | Recoverable anomalies (poisoned mutex, FK rebuild fallback). |
| `info!`  | always (default filter is `info,stalagmight=debug`) | **Once-per-lifecycle** events: app startup, project create / open / save / close success, schema migrations, full integrity check. |
| `debug!` | on by default (the default filter enables `stalagmight=debug`); silence with `RUST_LOG=info` | Per-operation chatter, sub-phase timings, load-loop progress, per-section import timing. |
| `trace!` | off everywhere by default | Per-shot / per-row spam. (A handful of callers: tilemap cache hits, closure correction/metrics.) |

The default release filter is set in `src-tauri/crates/telemetry/src/logging/mod.rs`. Override at
runtime via `RUST_LOG=…` exactly as in any tracing application. Log targets
follow module paths, so they carry the workspace crate names: the app crate is
`stalagmight_lib`, the members are `stalagmight_core`,
`stalagmight_persistence`, `stalagmight_parsers`, and so on. The bare prefix
`stalagmight` matches all of them.

```bash
# See per-operation chatter (load_metadata timings, save/load phases, etc.)
# across the app crate and every member crate
RUST_LOG=stalagmight=debug pnpm tauri dev

# Just the parts you care about
RUST_LOG=stalagmight_persistence::persistence::sqlite=debug,\
stalagmight_parsers::parsers::tml=debug pnpm tauri dev
```

### Categories of `debug!` worth knowing about

**Load-phase timings** (`debug!` in
`src-tauri/crates/persistence/src/persistence/sqlite/open.rs::open_existing` and
`src-tauri/crates/models/src/project/coordinates.rs::compute_station_coordinates_with_progress`):
break down `Database::open_existing` and `compute_station_coordinates` into
sub-phases (validate_integrity, has_project_data, ensure_schema_version,
validate_foreign_keys, build_colocated_groups, depths loop, xyz walk, …).
Enable when investigating slow project opens.

**TML import loop progress** (`debug!` in
`src-tauri/crates/core/src/core/state/command_import.rs::execute_import_sections`): per-100-section
batch timing - `batch_ms`, `create_ms`, `placement_ms`. Enable when debugging
super-linear growth during a batch import.

**Per-operation success acks** (`debug!` throughout the `command_*.rs` modules
under `src-tauri/crates/core/src/core/state/`): "Section created", "Stations
merged", "Shot updated", etc. On by default like every other `debug!` (see the
level table) - they fire N times during a 14k-section import and don't help in
release, so silence them with `RUST_LOG=info`.

**SQLite open / save chatter** (`debug!` across
`src-tauri/crates/persistence/src/persistence/sqlite/` - `open.rs`, `write.rs`,
`read.rs`): per-open / per-save / per-row-count
diagnostics. Enable when debugging persistence bugs.

**Recent-projects metadata timings** (`debug!` in
`sqlite/read.rs::load_metadata`): fires once per recent project on every
refresh. Enable when chasing slow startup.

### `debug_assert_*` and `#[cfg(debug_assertions)]`

`Project::debug_assert_merge_alignment` does an O(N) cross-check after every
mutating command. The whole body sits under `#[cfg(debug_assertions)]` -
release builds compile it to a no-op. **Don't remove the gate.** The function's
name lies if you don't believe me - there was a stretch where it ran in release
and was the dominant cost in batch imports.

If you add a similar O(N) sanity check, put it in `#[cfg(debug_assertions)]`
and document that fact at the call site, the way that function does.

### Profile harnesses

`profile_tml_import_full` and `profile_stalx_load` live in
`src-tauri/crates/core/src/core/state/tests.rs`. They are unit tests with `#[ignore]` -
they never run with `cargo test` unless explicitly opted in:

```bash
# Profile a TML import end-to-end (parse + convert + execute_import_sections)
STALAGMIGHT_PROFILE_TML=/path/to/file.tml \
  cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
  core::state::tests::profile_tml_import_full -- --nocapture --ignored

# Profile a stalx load end-to-end (open + load + station coords + warnings +
# undo/redo + binary encode)
STALAGMIGHT_PROFILE_STALX=/path/to/file.stalx \
  cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
  core::state::tests::profile_stalx_load -- --nocapture --ignored
```

Combine with `RUST_LOG=stalagmight=debug` to also get the in-code sub-phase
timings. Both harnesses print a pretty per-phase table to stderr and exit
clean - they're meant to be run on demand by hand.

## TypeScript frontend

### `src/utils/debugLog.ts`

A thin wrapper around `console.log`. The exported `debugLog` is bound to the
real `console.log` **only when `import.meta.env.DEV` is `true`** (Vite sets it
in dev / test). In a production build it compiles to `() => {}` and Vite's
tree-shaker drops the call sites entirely.

```typescript
import { debugLog } from '../utils/debugLog';

debugLog('[projectStore] refreshProjectState: done');
```

Use it for any per-operation diagnostic print. Keep raw `console.error` (and
`console.warn` for genuine runtime anomalies a user could act on) untouched -
those are signal, not noise.

### Env-flag-gated debug modules

A few standalone debug modules ship with the codebase but only activate behind
an env flag - they are off by default and tree-shaken in production.

- `src/utils/debugLockup.ts` - IPC in-flight tracking for the macOS UI lockup
  investigation. Enabled with `VITE_DEBUG_MACOS_LOCKUP=true`. See
  [`debugging-macos-pan-zoom.md`](debugging-macos-pan-zoom.md).
- `src/components/Map/panZoomDebug.ts` - MapLibre handler instrumentation,
  same flag, same target investigation.

To enable, set the env var when invoking the dev server:

```bash
VITE_DEBUG_MACOS_LOCKUP=true pnpm tauri dev
```

### `MemoryProbe` panel and the map dev-stats overlays

The in-project hamburger menu has a **Debug** submenu with four independent
toggles, each off by default and reset on project load / on returning to
GlobalView (`showDevStats`/`showMemoryStats`/`showMapStats`/`showZoomStats`
in `uiStore.ts`):

- **Dev Stats** - frame rate, render-pipeline profile (draw calls, triangles,
  layers, source feature counts), and the MapLibre built-in debug-layer
  toggle buttons (tile boundaries, collision boxes, overdraw, labels, all
  layers).
- **Map Stats** - map/tile load state, canvas size and center coordinates,
  WebGL info, satellite tile-fetch stats, network status, and tile-cache
  stats.
- **Zoom Stats** - current zoom level, the performance of the last completed
  zoom gesture (fps, duration, levels crossed, worst frame time), and
  wheel-chain diagnostics for the previous and live wheel gesture (event
  count, scroll/travel px, wheel-vs-trackpad classification, zoom start ->
  end - `WheelChainStats.tsx`). The wheel-chain part used to be a separate
  overlay mounted unconditionally in dev builds (a leftover from a Windows
  zoom-choppiness investigation); it's now folded into this toggle instead
  of always being on screen.
- **Memory Stats** - shows the in-window `MemoryProbe` panel, which polls
  backend RSS and frontend feature counts on a 2 s interval; the
  **Snapshot** button copies a JSON dump to clipboard for pasting into bug
  reports. Also force-visible via the `debug.memory_probe` config flag (see
  [`configuration.md`](configuration.md)), independent of the menu toggle.

Dev/Map/Zoom Stats are three panels sharing one `useMapDevStats` hook
(`src/components/Map/useMapDevStats.ts`) so the WebGL draw-call
instrumentation and the cache/tile-fetch IPC polling are only installed once,
regardless of how many of the three are open; see `MapDevStatsPanels.tsx` for
how they're composed.

The probe used to force-materialize every lazy `section.shots` on every tick
during the SoA migration - that bug is fixed; if you add a new field that's
lazily computed, make sure you read it from `SurveyData` (typed-array slice)
rather than from `Section.shots` (the legacy lazy getter). See the in-line
comment in `src/components/MemoryProbe/MemoryProbe.tsx::takeSnapshot`.

### Per-feature-flag config debug panels

A few panels are gated by a runtime config flag set in `config.toml` (locations
and full schema in [`configuration.md`](configuration.md)). The hit-detection
log at the click handler in `SurveyLayer.tsx` is the clearest example -
controlled by `debug.map.log_hit_detection`. Off by default; toggle in the
config file when you need to see which feature was picked on each click.

### Agent console bridge (dev-only)

A dev-only bridge lets an outside process on the same machine (for example
Claude Code, via `curl` / `tail`) read the live frontend console of the running
`pnpm tauri dev` webview and execute JavaScript inside it. It exists **only
under `vite serve`**: the Vite plugin (`scripts/lib/vite-agent-bridge.mjs`) is
registered with `apply: 'serve'`, and the webview client
(`src/dev/agentBridge.ts`) is dynamically imported behind `import.meta.env.DEV`
in `src/main.tsx`, so both tree-shake out of production builds. There are no Rust
changes, so it can be wired up without restarting the Tauri app.

On each dev-server start the plugin writes to `.agent-bridge/` at the repo root
(git-ignored):

- `token`: a fresh random token, mode 0600, regenerated every server start.
- `console.jsonl`: one JSON line (`{ts, level, text}`) per mirrored console
  entry. The webview wraps `console.log/info/warn/error/debug` and flushes a
  batch every 300 ms.
- `results.jsonl`: one JSON line per eval result.

Usage from a shell in the repo root:

```bash
# Follow the live frontend console
tail -f .agent-bridge/console.jsonl

# Evaluate an expression (holds up to `wait` seconds for the result)
curl -s -X POST "http://localhost:1420/__agent/eval?wait=10" \
  -H "x-agent-token: $(cat .agent-bridge/token)" \
  --data-binary '1+1'
# -> {"id":1,"ok":true,"value":2}

# Statement form: use an explicit `return` (the expression form is tried first,
# then this is the fallback on a construction-time SyntaxError)
curl -s -X POST "http://localhost:1420/__agent/eval?wait=10" \
  -H "x-agent-token: $(cat .agent-bridge/token)" \
  --data-binary 'const el = document.querySelector("#root"); return el?.childElementCount ?? 0;'
```

If the `wait` window elapses before the webview posts a result (page mid-reload,
say), the response is `{"id":N,"status":"pending"}` and the result still lands in
`results.jsonl` once it arrives.

Access is gated three ways: the `x-agent-token` header (or `?token=` for GETs)
must match the current token; the `Host` header must be `localhost:1420` or
`127.0.0.1:1420`; and any present `Origin` must be a known dev/tauri origin
(`http://localhost:1420`, `tauri://localhost`, `http://tauri.localhost`). No CORS
headers are set, so a cross-origin browser cannot read the token or drive the
bridge. None of this exists in a production build.

## Development-build-only options

A few user-facing options exist only in a development build. Each is gated on both
sides, so the backend never relies on the frontend hiding a control. Release builds
include betas and release candidates: they are all built with the release profile.

| Option | Backend gate | Frontend gate |
|--------|--------------|---------------|
| SpeleoDB "Allow insecure HTTP" (`speleodb.insecure_http`, [configuration](configuration.md)) | `INSECURE_HTTP_HONOURED` = `cfg!(debug_assertions)` in `src-tauri/crates/speleodb/src/speleodb/tls.rs`: the transport policy (`validate_endpoint`, and the client's `enforce`) ignores a stored or requested value in a release build | `import.meta.env.DEV` in `src/components/Dialogs/settings/SpeleoDbSettings.tsx`: no switch is rendered, and Connect sends no choice |

The config field stays in the schema, so a file a development build wrote loads in
every build. Write the build decision as a pure function of the build flag
(`honoured_insecure_http(requested, dev_build)`) so a debug-build unit test pins what
a release build does. Playwright runs against the dev server locally and a production
build in CI; a spec that asserts a development-build-only control reads
`window.__E2E_TEST__.devBuild` (set in `src/main.tsx`) and checks each mode.

## Conventions for new debug code

1. **Per-operation backend log?** Use `tracing::debug!`, not `tracing::info!`.
   `info!` is reserved for one-shot lifecycle events (project opened, schema
   migrated, integrity check passed). If you add a `tracing::info!` inside a
   command, ask yourself how many times it will fire on a 14 k-section import.
2. **Per-operation frontend log?** Use `debugLog` from
   `src/utils/debugLog.ts`, not `console.log`. Keep `console.error` and
   `console.warn` for actual runtime anomalies.
3. **O(N) cross-check?** Wrap in `#[cfg(debug_assertions)]` (Rust) or behind
   `import.meta.env.DEV` (TS). Look at `debug_assert_merge_alignment` for the
   pattern.
4. **Standalone debug module?** Gate it behind a `VITE_*` env flag and have
   the module's entry point early-return when off, so Vite tree-shakes the
   body out of production bundles. `debugLockup.ts` is the template.
5. **Profile harness?** Add it as a `#[test]` with `#[ignore]` plus an env-var
   gate, the way `profile_tml_import_full` does. It doesn't run with the
   regular suite and ships in the binary as zero overhead (it's `#[cfg(test)]`).
