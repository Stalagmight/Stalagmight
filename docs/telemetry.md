# Telemetry

Anonymous, session-level usage telemetry. Opt-in only, no PII. IP addresses
are SHA-256 hashed server-side for abuse detection - no raw IPs are stored.

## Architecture

```
User action → telemetry.ts (batch counts) → IPC → Rust TelemetryCollector
                                                         │
                                          ┌──────────────┴──────────────┐
                                          ▼                             ▼
                                   HTTP POST to                  Buffer to disk
                                   telemetry server              (retry next flush)
```

**Frontend** (`src/utils/telemetry.ts`): Batches event counts in memory, flushes
to Rust every 60 seconds.

**Backend** (`src-tauri/crates/telemetry/src/telemetry/mod.rs`): Aggregates counts per session, flushes
to the configured telemetry endpoint every 60 seconds. Buffers to
`<app-local-data-dir>/telemetry-buffer/*.json` when offline (the per-app data
dir - see the path tables in [configuration.md](configuration.md)). The buffer
is capped at 100 payload files; beyond that the oldest are dropped, so a very
long offline stretch loses its oldest usage counts rather than growing the
directory without bound.

**Feature-gated**: All Rust telemetry is behind `#[cfg(feature = "telemetry")]`.
When disabled, all methods are zero-cost no-ops eliminated by the compiler.
The frontend self-gates by calling `is_telemetry_available` at startup.

**Opt-in**: Controlled by `general.telemetry_enabled` in config. Users are
prompted on first launch via `TelemetryOptInDialog`.

## Payload

### v3 - Self-describing JSON + gzip (wire format)

Sent with `Content-Type: application/json` and `Content-Encoding: gzip`. Event
names are sent **verbatim** as a `{name: count}` map - there is no numeric ID
registry on the wire, so a new event can never be silently dropped by an older
worker. This is also the on-disk buffer format, so the wire and buffer formats
are identical (no re-encoding on retry).

```json
{
  "v": 3,
  "app_version": "0.8.0-9f3c1a2b",
  "os": "linux",
  "session_id": "a1b2c3d4-...",
  "duration_secs": 3600,
  "project_seq": 1,
  "events": {
    "command.create_shot": 42,
    "action.tool.merge:keyboard": 10,
    "tile.fetch.esri": 250,
    "display-state.showGrid:true": 1
  }
}
```

The client gzips the JSON before POSTing (`flate2::write::GzEncoder` in
`send_payload`); the worker decompresses with `DecompressionStream('gzip')` and
parses the JSON (uncompressed JSON is also accepted, e.g. for manual testing).
Self-describing JSON+gzip is ~2.4× the size of the old packed format but still
sub-2 KB per flush, and D1 storage is unchanged (it always stored string names).

### Event vocabulary

`src-tauri/crates/telemetry/src/telemetry/registry.rs` keeps `EVENT_NAMES` / `SOURCE_NAMES` as the
canonical list of event names the app emits. It is **not** a wire ID map - it
exists only to back the coverage tests (every command / tool / dialog has a
tracked event; typo detection). Order is irrelevant, and there is no worker-side
copy to keep in sync.

**`project_seq`** - auto-incrementing project context counter. `0` means global
view (no project open) or Rust-originated events. Each time the user opens a
project (`setView('single')`), the sequence increments. A single flush never
mixes events from different project contexts - the frontend force-flushes
pending events before advancing the counter.

No file paths, project names, survey data, coordinates, or user identity.

## Event Reference

### Commands - `command.<name>` / `command.<name>:<source>`

Every IPC call (except telemetry's own) is tracked via `trackCommand`. When
a source hint is set (e.g., `keyboard`, `toolbar`), it's appended after `:`.

```
command.undo:keyboard       - undo triggered by keyboard shortcut
command.save_project        - save with no known source
command.create_shot:toolbar - shot created from toolbar action
```

**Excluded from tracking**: `track_events`, `flush_telemetry`,
`is_telemetry_available`, `set_telemetry_enabled`.

### Actions - `action.<name>:<source>`

UI-only events tracked via `trackAction`. Always include a source.

| Category | Events | Sources |
|----------|--------|---------|
| Tool switches | `tool.select`, `tool.merge`, `tool.measure`, ... | `keyboard`, `toolbar` |
| Navigation | `nav.zoom`, `nav.pan`, `nav.fit-to-bounds`, `nav.zoom-to-section`, ... | `mouse`, `touch`, `keyboard`, `dblclick` |
| Display toggles | `toggle.showStations`, `toggle.showGrid`, ... | `display-menu` |
| Selection | `select.station`, `select.shot`, `select.section` | `map`, `data-table`, `sections-panel`, `detail-panel` |
| Multi-select | `select-add.station`, `select-add.shot` | `map`, `data-table` |
| Dialogs | `dialog.import`, `dialog.settings`, ... | `unknown`, `context-menu` |
| Panel toggles | `panel-open.sections`, `panel-close.detail`, `panel-open.data-table` | `keyboard`, `unknown` (= collapse button) |
| Context menu | `unmerge`, `remove-closure`, `set-shot-kind`, `cycle-shot-kind` | `context-menu`, `detail-panel` |

### Dialog Cancellations - `dialog-cancel.<name>`

Emitted when a dialog closes without its success path calling
`trackDialogComplete()`. Tracks user abandonment rate per dialog.

```
dialog-cancel.import        - user opened import dialog then cancelled
dialog-cancel.merge         - user cancelled merge dialog
```

**Completion-flag pattern**: Success handlers call `trackDialogComplete(name)`
before `closeDialog()`. Then `closeDialog()` checks the flag - if unset, emits
the cancel event. Dialog chaining (dialog A opens dialog B) is handled
automatically - the previous dialog is marked as completed.

### Undo Regret - `undo-after.<command>`

When the user undoes within 15 seconds of an undoable command, the pair is
recorded. High `undo-after.X` rates relative to `command.X` signal that
operation X frequently produces unwanted results.

```
undo-after.merge_stations   - user merged then immediately undid
undo-after.delete_shot      - user deleted then immediately undid
```

### Display Steady-State - `display-state.<key>:<value>`

Emitted once at project load for each display toggle that differs from
`DISPLAY_DEFAULTS`. Answers "which defaults do users override most?"

```
display-state.showGrid:true          - user has grid on (default: off)
display-state.showStationLabels:true - user has labels on (default: off)
display-state.gridSpacing:5          - user changed spacing from default 10
```

New projects with default settings emit nothing.

### Workflow Dwell Time - `dwell.<state>`

Tracks cumulative seconds spent in each workflow state per session. States are
mutually exclusive - switching to a new state flushes the accumulated time on
the previous one.

```
dwell.tool.select              - seconds with Select tool active
dwell.dialog.settings          - seconds in Settings dialog
dwell.dialog.import            - seconds in Import dialog
dwell.placement                - seconds in placement mode
dwell.global-view              - seconds on the Welcome/project-list screen
```

**Integration points** (all in `uiStore.ts`):

| Trigger | Dwell key |
|---------|-----------|
| `setView('single')` | `tool.{activeTool}` |
| `setView('global')` | `global-view` |
| `setActiveTool(tool)` | `tool.{tool}` (skipped when dialog/placement active) |
| `openDialog(name)` | `dialog.{name}` |
| `closeDialog()` | Resumes `placement` or `tool.{activeTool}` |
| `startPlacementMode()` | `placement` |
| `cancelPlacementMode()` / `confirmPlacement()` | `tool.{activeTool}` |

**Flush on close**: `flushTelemetry()` calls `flushDwell()` to capture the
final state's accumulated time before sending.

**Same-key no-op**: Calling `setDwellState` with the same key is a no-op -
prevents timer resets from redundant calls.

### SpeleoDB traffic - `speleodb-api.<kind>.*` (#210, F1)

Every SpeleoDB HTTP call is instrumented at the Rust client's single choke
point (`SdkClient::call`) so we can size the
server load Stalagmight causes and spot a runaway poll/retry loop *before* it
reaches the server. Recorded **per HTTP attempt** (a retried idempotent 5xx
really re-hits the server, so a retry storm shows up as inflated counts +
latency). Emitted from the backend via `TelemetryCollector::track`, so they land
in the global `project_seq 0` bucket.

**No PII, by construction.** The only dimensions are the endpoint *kind*, the
outcome class, and a latency bucket - never a host, project id, email, token, or
file bytes. That is all sizing load / flagging abuse needs.

`<kind>` is one of the `speleodb::metrics::Endpoint` labels: `login`,
`list-projects`, `get-project`, `get-revisions`, `fetch-user`, `download-stalx`,
`upload`, `acquire`, `release`, `patch-project`, and `create-project`.

```
speleodb-api.<kind>.ok                  - completed exchange (2xx/3xx, incl. 304)
speleodb-api.<kind>.client-error        - 4xx
speleodb-api.<kind>.server-error        - 5xx
speleodb-api.<kind>.network-error       - transport failure (DNS/timeout/reset)
speleodb-api.<kind>.latency.<bucket>    - one hit per attempt, in its duration bucket
```

**Count** is implicit: the four outcome counters sum to the call count for that
endpoint. **Latency** follows the `dwell.<state>` precedent (a plain counter),
extended to a **histogram** because the flat `{name: count}` wire format cannot
carry a computed percentile: each attempt increments exactly one
`latency.<bucket>` counter, and the backend reconstructs per-endpoint p50/p95
per session from the bucket distribution. Buckets (ms): `le-100`, `le-500`,
`le-1000`, `le-5000`, `le-15000`, `gt-15000`.

Adding an `Endpoint` variant is CI-gated: `every_speleodb_endpoint_has_traffic_events`
regenerates the exact `speleodb-api.*` set from `Endpoint::ALL x Outcome::ALL x
LATENCY_BUCKETS` and fails until the new kind's ten names are in `EVENT_NAMES`
(and it flags any orphan `speleodb-api.*` name that no longer maps to a kind).

### Map-tile consumption - `tile.<source>.<provider>` (#31)

One event per **served** tile, tagged by provider and by where its bytes came
from. Emitted from the custom tile protocol handler in `lib.rs`. Errors (nothing
served) are not counted, so the three buckets are disjoint and sum to the tiles a
session used for that provider.

```
tile.fetch.<provider>       - imagery bytes pulled from the provider
tile.revalidate.<provider>  - served from cache after a 304: a round trip, but
                              no imagery crossed the wire
tile.cache.<provider>       - bytes came from cache without a successful
                              network response: a clean cache hit, a stale copy
                              under stale-if-error, or an overzoomed crop of a
                              cached parent
```

**Why revalidations are their own bucket.** The cache stores a tile for as long
as the provider's `Cache-Control` grants (Esri: a day) and then revalidates with
`If-None-Match` rather than re-downloading. Past that first day nearly every
request is a `304`, so folding revalidations into `tile.fetch` would report
near-total provider consumption for a session that downloaded almost nothing -
and consumption is the one thing this metric exists to size. The split also makes
the bandwidth saving visible directly: `revalidate` against `fetch` is how much
imagery the conditional request avoided pulling.

**These buckets measure where the BYTES came from, not whether the network was
touched.** `tile.cache` covers a failed attempt too: stale-if-error tries a
revalidation first and only serves the stored copy when that fails, and the
overzoom fallback reaches a cached parent after the child fetch failed. Both are
right for consumption - no imagery was pulled - but neither means "we did not
contact the provider". For that question use `TileFetchStats`
(`total_fetched` / `total_errors`), which counts attempts and is what drives the
offline indicator.

One thing that IS enforced: a clean cache hit makes no network call, which takes
care to keep true. The tilemap availability probe is itself a network request
when its in-memory cache is cold, so the fresh tile cache is consulted **before**
every probe - at the requested zoom and at each parent level of the overzoom
walk. Probing first meant a request answered entirely from disk could still reach
the provider.

The classification is per request (`TileServeSource`), deliberately not the
shared `TileFetchStats` diff, which concurrent tile requests would
cross-contaminate. It is also **not** what drives the offline indicator: that
runs off the stats snapshot, where a `304` correctly counts as the network having
been reached.

Emitted from **both** tile paths, which are platform-split: desktop serves tiles
through Tauri's native URI-scheme handler, while Android bypasses that (it
serialises every request onto one WebView IO thread) and calls the `fetch_tile`
IPC command instead. Both classify with the same `TileServeSource`, so the two
platforms' numbers are directly comparable.

Adding a provider is CI-gated: `every_tile_provider_has_consumption_events`
walks the registry and fails until all three names for it are in `EVENT_NAMES`.

## Key Files

| File | Role |
|------|------|
| `src/utils/telemetry.ts` | Frontend batching, source hints, dialog/display tracking |
| `src/utils/ipc.ts` | Calls `trackCommand` for every IPC invocation |
| `src/store/uiStore.ts` | Wires tracking into `closeDialog`, `openDialog`, `setSelection`, `addToSelection`, panel toggles, display settings |
| `src-tauri/crates/telemetry/src/telemetry/mod.rs` | Rust collector, HTTP send, disk buffer, periodic flush |
| `src-tauri/crates/telemetry/src/telemetry/registry.rs` | Canonical event/source name vocabulary (backs the coverage tests) |
| `src-tauri/src/commands/telemetry_coverage_tests.rs` | The `registry_covers_all_*` coverage suite (app crate - it parses `lib.rs`'s `generate_handler![]` and the frontend sources) |
| `src-tauri/crates/speleodb/src/speleodb/metrics.rs` | Per-endpoint SpeleoDB traffic instrumentation (`speleodb-api.*`, #210) |
| `src-tauri/src/commands/telemetry.rs` | IPC commands: `track_events`, `flush_telemetry`, etc. |
| `src/store/displaySelectors.ts` | `DISPLAY_DEFAULTS` - single source of truth for default values |

## Source Tracking

The `setSource(name)` / `consumeSource()` pattern provides attribution for
where an action originated. Source is a global singleton - set before an action,
consumed by the first `trackAction` or `trackCommand` that fires.

Auto-resets after 50ms to prevent bleed to unrelated actions.

| Source | Meaning |
|--------|---------|
| `keyboard` | Keyboard shortcut (set in `useKeyboardShortcuts`) |
| `toolbar` | Toolbar button click |
| `map` | Map canvas interaction |
| `data-table` | Data table row/cell click |
| `sections-panel` | Sections panel click |
| `detail-panel` | Detail panel link/button |
| `display-menu` | Display toggle menu |
| `context-menu` | Right-click context menu action |
| `dialog` | Dialog-initiated (dialog chaining) |
| `dblclick` | Double-click open |
| `recent-list` | Recent-projects list (card/table) double-click, or the "open from file" hint link |
| `recent-map` | Recent-projects map marker |
| `hamburger` | Global-view hamburger application menu |
| `empty-state` | Empty-state cards (first run) |
| `mouse` | Map pan/zoom by wheel or mouse drag |
| `touch` | Map pan/zoom by touch gesture |
| `drag-drop` | File dropped onto the window (open or import) |
| `os-file-association` | File handed to us by the OS: double-click / "Open with" |
| `unknown` | No source set (default) |

### Caveat: single consumption

Source is consumed by whichever `trackAction` or `trackCommand` fires first. If
a single handler triggers both (e.g., selection + IPC), only the first gets the
source. In practice, current code paths don't mix these in the same synchronous
call, but be aware of this when adding new handlers.

### Project open/create/import attribution

The 50 ms `setSource` reset is too short for project open/create/import, whose
terminal IPC (`load_project_full`, `create_project_full`, `execute_*_import`,
`import_sections`) fires only after a file picker and/or dialog - long after the
originating click. A separate **pending project-action source** bridges that gap:
`setProjectActionSource(name)` is called synchronously at the entry point (recent
list/map, toolbar, hamburger, keyboard, empty-state) and `peekProjectActionSource()`
is applied at each terminal command. It is `peek` (not consumed) so multi-command
flows (e.g. DMP import = `create_project_full` + `import_sections`) all attribute
to the same entry point, and it is cleared on `setView('single')` (flow complete)
and on file-picker cancel. Project opens - which bypass the tracked `invokeIpc` via
the raw binary `load_project_full_binary` invoke - are explicitly counted as
`command.load_project_full[:source]` inside `loadProject`.

## Suppression

`withTrackingSuppressed(fn)` runs `fn` with all `trackAction` calls suppressed.
Used for programmatic UI changes that shouldn't appear as user actions (e.g.,
onboarding auto-opening panels).

```typescript
import { withTrackingSuppressed } from '../../utils/telemetry';

// Onboarding opens the sections panel - not a user action
withTrackingSuppressed(() => toggleSectionsPanel());
```

## Event Vocabulary & Coverage

`EVENT_NAMES` / `SOURCE_NAMES` in `src-tauri/crates/telemetry/src/telemetry/registry.rs` are the
canonical list of event names the app emits. Because the wire format sends names
verbatim (self-describing JSON), this is **not** an ID map - order is irrelevant,
entries can be added/renamed/removed freely, and there is no worker-side copy to
keep in sync. The vocabulary exists only to power the coverage tests below, which
guarantee every command / tool / dialog actually emits telemetry and catch typos
before they reach the database.

### Adding a new event name

1. Add the bare event name (e.g. `command.my_command`) to `EVENT_NAMES` in
   `registry.rs` - anywhere; order does not matter - and bump the `event_count`
   test.
2. Run the registry + coverage tests. They span three workspace crates - the
   registry's own tests live with the vocabulary in `stalagmight-telemetry`,
   the SpeleoDB traffic-event test in `stalagmight-speleodb`
   (`speleodb/metrics.rs`), and the `registry_covers_all_*` suite in the app
   crate (`src-tauri/src/commands/telemetry_coverage_tests.rs`, next to the
   `generate_handler![]` it parses) - so use the workspace form:

   ```bash
   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test --manifest-path src-tauri/Cargo.toml \
     --workspace --features telemetry -- telemetry metrics
   ```

The coverage tests flag a new command / tool / dialog that is missing its event
name (or an event name with no corresponding feature).

### Coverage tests

| Test | What it checks |
|------|----------------|
| `registry_covers_all_commands` | Every `generate_handler![]` command has a `command.*` entry (and vice versa) |
| `no_duplicate_event_names` | No duplicate strings in `EVENT_NAMES` |
| `event_count` / `source_count` | Array lengths match expected totals |
| `active_dwell_mirrors_dwell` | Every `dwell.<state>` has a matching `active-dwell.<state>` |
| `every_tile_provider_has_consumption_events` | Every tile provider has `tile.fetch.*` / `tile.revalidate.*` / `tile.cache.*` |
| `every_speleodb_endpoint_has_traffic_events` | Every `speleodb::metrics::Endpoint` has its `speleodb-api.<kind>.*` outcome + latency events (and no orphans) |
| `registry_covers_all_tools` | `ToolType` union ↔ `action.tool.*` / `dwell.tool.*` |
| `registry_covers_all_undoable_commands` | `UNDOABLE_COMMANDS` ↔ `undo-after.*` |
| `registry_covers_all_display_toggles` | `DISPLAY_DEFAULTS` keys ↔ `action.toggle.*` |
| `registry_covers_all_dialogs` | App.tsx `activeDialog` values ↔ `action.dialog.*` / `dialog-cancel.*` / `dwell.dialog.*` |
| `registry_covers_all_sources` | `SourceName` union ↔ `SOURCE_NAMES` |
| `registry_covers_all_nav_events` | `NavigationAction` union ↔ `action.nav.*` |
| `registry_covers_all_panel_events` | `PanelName` union ↔ `action.panel-open.*` / `action.panel-close.*` / `action.panel-pin.*` / `action.panel-unpin.*` |
| `registry_covers_all_selections` | `SelectionType` union ↔ `action.select.*` / `action.select-add.*` |
| `registry_covers_all_context_menu_actions` | `ContextMenuAction` union ↔ `action.*` (miscellaneous) |
| `dialog_complete_calls_match_dialogs` | All `trackDialogComplete('...')` call sites use valid dialog names |

## Adding Telemetry to New Features

### Type-safe helpers

All telemetry emission should go through typed helpers in `telemetry.ts`
rather than calling `trackAction()` with raw strings. Each helper has a
corresponding type union that is parsed by Rust exhaustiveness tests.

| Helper | Type union | Registry entries |
|--------|-----------|-----------------|
| `trackNav(nav, source?)` | `NavigationAction` | `action.nav.*` |
| `trackPanelToggle(panel, opening)` | `PanelName` | `action.panel-open.*` / `action.panel-close.*` |
| `trackSelect(type)` | `SelectionType` | `action.select.*` |
| `trackSelectAdd(type)` | `SelectionType` | `action.select-add.*` |
| `trackContextMenuAction(action, source?)` | `ContextMenuAction` | `action.*` |
| `setSource(source)` | `SourceName` | `SOURCE_NAMES` |

### New dialog

1. Add `trackDialogComplete('my-dialog')` before every success-path
   `closeDialog()` call.
2. Cancel/dismiss paths need no changes - `closeDialog()` auto-detects.
3. For informational dialogs (no meaningful cancel), add
   `trackDialogComplete` before every close path.
4. Add four registry entries: `action.dialog.my-dialog`,
   `dialog-cancel.my-dialog`, `dwell.dialog.my-dialog` and
   `active-dwell.dialog.my-dialog` - every dwell state is mirrored 1:1 by an
   active-dwell one (#31), so the fourth is not optional.
5. Tests `registry_covers_all_dialogs`, `active_dwell_mirrors_dwell` and
   `dialog_complete_calls_match_dialogs` will fail if anything is missing.

### New IPC command

Automatic - `invokeIpc` calls `trackCommand` for all non-telemetry commands.
If the command is undoable, add it to `UNDOABLE_COMMANDS` in `telemetry.ts`.

You must also add a `command.<name>` entry to `EVENT_NAMES` in `registry.rs`,
plus an `undo-after.<name>` entry if you added it to `UNDOABLE_COMMANDS`. The
`registry_covers_all_commands` and `registry_covers_all_undoable_commands` tests
will fail if you forget either.

### New tool

1. Add the tool name to `ToolType` in `uiStore.ts`.
2. Add `action.tool.<name>` and `dwell.tool.<name>` to `EVENT_NAMES`.
3. The `registry_covers_all_tools` test will fail if anything is missing.

### New navigation action

1. Add the action to the `NavigationAction` type union in `telemetry.ts`.
2. Use `trackNav('my-action')` at the call site.
3. Add `action.nav.<name>` to `EVENT_NAMES`.
4. The `registry_covers_all_nav_events` test will fail if anything is missing.

### New panel

1. Add the panel name to the `PanelName` type union in `telemetry.ts`.
2. Use `trackPanelToggle('my-panel', opening)` in the toggle function.
3. Add `action.panel-open.<name>`, `action.panel-close.<name>`,
   `action.panel-pin.<name>` and `action.panel-unpin.<name>` to `EVENT_NAMES`.
4. The `registry_covers_all_panel_events` test will fail if anything is missing.

### New selection source

1. Add the source to the `SourceName` type union in `telemetry.ts`.
2. Call `setSource('my-panel')` before `setSelection()` or `addToSelection()`.
3. Add the source to `SOURCE_NAMES` in `registry.rs`.
4. The `registry_covers_all_sources` test will fail if anything is missing.

### New keyboard shortcut

If added to `useKeyboardShortcuts`, source is automatic (set at handler entry).
For keyboard handlers elsewhere, call `setSource('keyboard')` before the action.

### New context-menu action

1. Add the action to the `ContextMenuAction` type union in `telemetry.ts`.
2. Use `trackContextMenuAction('my-action', 'context-menu')` at the call site.
3. Add `action.<name>` to `EVENT_NAMES`.
4. The `registry_covers_all_context_menu_actions` test will fail if anything
   is missing.

## Deployment

### Infrastructure

| Component | Location |
|-----------|----------|
| Worker | Cloudflare Workers (`stalagmight-telemetry`) |
| Database | Cloudflare D1 (`stalagmight-telemetry`, region WEUR) |
| Endpoint | Set via `TELEMETRY_ENDPOINT` CI secret |
| Worker URL | Disabled (`workers_dev = false`) |

### Build configuration

Telemetry is behind the Rust `telemetry` feature flag. CI and release builds
enable it. The telemetry endpoint URL is injected at build time via the
`TELEMETRY_ENDPOINT` environment variable - it is **not** stored in the repo.

**`TELEMETRY_ENDPOINT`** (required CI/release secret): The full URL where
telemetry payloads are POSTed.
The build will fail with a clear error if this variable is missing when the
`telemetry` feature is enabled. Forks that enable telemetry without setting the
secret get a build error - no data is ever sent to our server by accident.

CI tests use a dummy endpoint (`https://test.invalid/v1/events`) since they
don't make HTTP calls.

```yaml
# CI - runs registry exhaustiveness tests (dummy endpoint, no HTTP)
TELEMETRY_ENDPOINT=https://test.invalid/v1/events \
  cargo test --manifest-path src-tauri/Cargo.toml --workspace --features telemetry

# Release - compiles telemetry into the binary (real endpoint from secret)
TELEMETRY_ENDPOINT=$TELEMETRY_ENDPOINT \
  pnpm tauri build -- --features telemetry
```

### Deploying the worker

Requires a Cloudflare API token with `Workers Scripts: Edit`, `D1: Edit`,
and `DNS: Edit` on the `stalagmight.com` zone.

```bash
# Copy the template and fill in the database ID
cp telemetry-worker/wrangler.toml.example telemetry-worker/wrangler.toml
# Look up the database ID if needed:
#   CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=... wrangler d1 list

# Deploy
cd telemetry-worker
# --ignore-workspace is REQUIRED: the repo root has a pnpm-workspace.yaml (it is
# where pnpm 10+ reads build-script approvals), and pnpm walks upward to find it.
# Without the flag pnpm installs the ROOT project instead, exits 0, and leaves
# telemetry-worker/node_modules empty - so the wrangler pin below is never installed.
pnpm install --ignore-workspace
CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=... npx wrangler deploy
```

### Schema changes

Apply with `--remote` to target the production database:

```bash
CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=... \
  npx wrangler d1 execute stalagmight-telemetry --file=schema.sql --remote
```

### Request filtering

The Rust client sends `User-Agent: Stalagmight/<version>` on every request.
The worker rejects any request whose `User-Agent` does not start with
`Stalagmight/` - returning a silent 404 before any parsing or DB work.
This blocks automated scanners and bots at near-zero cost.

For additional protection, a Cloudflare WAF custom rule can block non-Stalagmight
user agents at the edge (before the worker is invoked):

- **Rule name**: `Block non-Stalagmight UA on usage subdomain`
- **Expression**: `(http.host eq "<telemetry-hostname>" and not starts_with(http.request.headers["user-agent"][0], "Stalagmight/"))`
- **Action**: Block

### IP hash for abuse detection

The worker hashes the `CF-Connecting-IP` header (SHA-256) and stores the hex
digest in `sessions.ip_hash`. No raw IPs are stored. This allows bulk cleanup
if a single source floods the database:

```sql
-- Find sessions from a specific IP hash
SELECT * FROM sessions WHERE ip_hash = '<hash>';

-- Delete all data from an abusive source
DELETE FROM events WHERE session_id IN (SELECT session_id FROM sessions WHERE ip_hash = '<hash>');
DELETE FROM sessions WHERE ip_hash = '<hash>';
```

### Observability

Workers Logs are enabled via `[observability]` in `wrangler.toml` (100%
sampling), so invocations and their response status are visible in the
Cloudflare dashboard and via `wrangler tail`. The worker emits no `console.*`
output of its own - each path returns a status code and a short text body,
nothing more.

**Live tail** (streams logs in real time):

```bash
(cd telemetry-worker && CLOUDFLARE_API_TOKEN=... npx wrangler tail)
```

**Dashboard**: Workers & Pages > stalagmight-telemetry > Logs - searchable,
filterable by status code, with invocation traces (wall time, CPU time,
subrequests).

**Response statuses returned by the worker**:

| Status | Meaning |
|--------|---------|
| 200 | Event batch ingested |
| 400 | Body is not valid gzip/JSON (including a gzip stream expanding past the 256 KB cap) or has the wrong shape; `session_id`/`app_version`/`os` over its length limit (64/32/32); or more than 500 distinct events |
| 404 | Not `POST /v1/events`, or a `User-Agent` that does not start with `Stalagmight/` |
| 413 | `Content-Length` over 64 KB |
| 429 | The same IP hash already has 10 flushes recorded in the last minute |
| 500 | D1 batch write failed |

### Configuration files

`telemetry-worker/wrangler.toml` is gitignored (contains deployment-specific
database ID). `wrangler.toml.example` is the committed template.
