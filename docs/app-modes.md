# Workspace modes and the read-only contract

An open project is in exactly one of four workspace modes, held in
`uiStore.appMode`:

- `survey` - the map with the Sections, Details and docked data-table panels. **Editable.**
- `data` - one full-window table of every section's stations and shots, with an
  optional map pane beside it. **Editable.**
- `plan` - dive and gas planning over the read-only closure map. **Read-only.**
- `export` - the WYSIWYG map export. **Read-only.**

The `ModeCarousel` in the toolbar switches between Survey, Data and Plan. Export is
not on the carousel: the project menu's Export item and Ctrl+E enter it
(`enterExport`, which remembers the mode it came from in `exportReturnMode`), and
the **Close export** button leaves it (`closeExport`, back to that mode). Clicking a
carousel button while Export is open (none is pressed then) also leaves it. Both
paths end in `setAppMode`, so everything below applies to Export unchanged.

[SPEC.md 6.1](SPEC.md#61-workspace-modes) describes what each mode is for; this page
is the contract the code has to keep, because getting it wrong is silent: a mutation
leaks into a read-only workspace and nothing complains until a user edits a project
they were only planning a dive on - or an editable mode is treated as read-only (or
as Survey) and a control quietly disappears from it.

## What each mode swaps

Every row is a render-time `appMode` branch in `SingleSystemView.tsx`,
`DataModeLayout.tsx`, `Toolbar.tsx`, `DisplayMenu.tsx`, `ContextMenu.tsx` or
`useKeyboardShortcuts.ts`. The map itself is the same MapLibre instance in all four.

| | Survey | Data | Plan | Export |
|---|---|---|---|---|
| Entered from | carousel | carousel, or "Open in Data" on Survey's data table | carousel | project menu, Ctrl+E |
| Sections tab | shown | hidden | hidden | hidden |
| SpeleoDB workspace | available | on request or already visible | on request or already visible | on request or already visible |
| Right dock | `DetailPanel` | not rendered | `GasPlanPanel` | `ExportPanel` |
| Bottom dock | `DataTable` | not rendered | `PlanTablesPanel` | not rendered |
| Map area | the map | `DataWorkspace` over the map, or beside a map pane | the map | `ExportPreview` overlay for the image export type |
| Import, Undo/Redo | shown | shown | hidden | hidden |
| Toolbar tools | `TOOLBAR_ITEMS` | `TOOLBAR_ITEMS` with the map pane open, none with it closed | `READ_ONLY_TOOLBAR_ITEMS` | `READ_ONLY_TOOLBAR_ITEMS` |
| Display menu | full | full with the map pane open, hidden with it closed | no Color, no Geometry group | hidden |
| Map context menu | survey menu | survey menu with the map pane open, none with it closed | route menu on a station | none |
| Map-aimed shortcuts | on | off with the pane closed or focus in the table | read-only | read-only |

`READ_ONLY_TOOLBAR_ITEMS` is `TOOLBAR_ITEMS` filtered through `isMutatingTool`
(dangling separators collapsed), so mutating tools are **hidden, not disabled**
and the two lists cannot drift. Data uses the full list because it is editable.

Whichever base layout the mode picks then goes through a second pass, dropping
the navigate group unless `map.controls.show_navigation_tools` is on (#409). So
Plan and Export are `select | navigate group | measure` with navigation tools on,
and `select | measure` with them off - the default. Both passes run through
`filterToolbarItems` (`Toolbar.tsx`), whose separator cleanup is what makes them
composable: each pass re-tidies the dividers the previous one stranded.

### Data's layout and its map pane

`DataModeLayout` (`src/components/SingleSystemView/DataModeLayout.tsx`) is the map
area in **every** mode, and it exists to keep one rule: `MapCanvas` is never
unmounted or remounted by a mode or map-pane change. A remount throws away the WebGL
context, the sources and the camera. Its children sit in fixed slots - the Data
workspace, the splitter, the map column - each conditional holding its index when
false, so the map column is always the same element.

- **Pane closed** (`useDataWorkspaceStore.mapPaneOpen` false): the workspace covers
  the whole area, opaque. The map keeps its full size underneath (no resize churn)
  but is `visibility: hidden` and `aria-hidden`, so nothing under the table paints
  or takes focus.
- **Pane open**: the map takes a right-hand pane of `mapPaneWidth` with
  `DataMapPaneHeader` on top, the workspace fills the rest, and a splitter sits
  between them (`data-map-pane-splitter`, an ARIA `separator`: arrows nudge 16 px,
  Shift 64 px, Home/End jump to the bounds, double-click resets to
  `MAP_PANE_DEFAULT_WIDTH`). The width is clamped to `MAP_PANE_MIN_WIDTH` .. (area
  width minus `DATA_WORKSPACE_MIN_WIDTH`) for display only - the stored width is the
  user's, and a wider window gives it back. A drag lives in local state and reaches
  the store once, on release; the keyboard writes on a 300 ms trailing debounce.
  MapLibre follows the container size itself.
- Closing the pane (or entering Data with it closed) drops a mutating tool back to
  Select (`Toolbar`), so reopening never resumes a half-done gesture.
- Flows that need what Data does not show switch to Survey first:
  `revealWarningsTab` (the Details panel's Warnings tab: Resolve, "Resolve now") and
  `startPlacementMode` (placing a section needs the full map).
- In Survey the map area also hosts `BackToDataChip`, the way back after Data's
  "Open in Survey".

## The contract

```ts
isEditableMode(mode) = mode === 'survey' || mode === 'data'
isMapReadOnly(s)     = s.geometryMode === 'closureAdjusted' || !isEditableMode(s.appMode)
```

Both live in `src/store/uiStore.ts`. `isMapReadOnly` is false only in Survey or Data
on the measured geometry. The Closures Computed View makes Data's table read-only
exactly as it does Survey's map; the Display menu's Geometry group is available in
Data (with the map pane open) so the user can get back.

Both terms of `isMapReadOnly` are load-bearing, and the second is the one that keeps
being forgotten. Entering Plan or Export forces `geometryMode: 'closureAdjusted'` -
but `MapCanvas` reverts it to `'measured'` when the project has no eligible
closures, and again when an adjusted-geometry fetch fails. So on a freshly
imported project with no closures, Plan mode runs with
`geometryMode === 'measured'`, and a gate written as
`geometryMode === 'closureAdjusted'` reports "editable" in a workspace that must
not be. `DataTable`, `SectionsPanel`, `WarningResolution` and `DetailPanel` all
shipped with exactly that bug.

The opposite mistake is new with Data: a check written as `appMode === 'survey'`
treats Data as read-only (or as not-Survey) and silently removes the control there.
Ask `isEditableMode` for "may this mode edit", and keep `appMode === 'survey'` for
the things that really exist only in Survey (the Details panel, the Sections tab,
the docked data table, the Warnings-tab visit).

**There is no server backstop for this half of the predicate.** The backend's
`AppState::guard_mutation` rejects a mutation only when `edit_enabled()` is
false. That is not a stored flag: it is derived from the lock claim, and it is
false only for a SpeleoDB project whose signed-in account holds no claim bound to
the current session (`src-tauri/crates/core/src/core/state/speleodb_edit_gate.rs`). A local
project in Plan or Export is always editable server-side, so read-only there
rests entirely on UI gating - which is why the gating is linted.

### The one sanctioned gate

Every mutation entry point goes through `src/hooks/useEditingBlocked.ts`:
`useEditingBlocked()` inside a component, `isEditingBlockedNow()` outside React
(raw keydown handlers, plain utilities).

Both compose `isMapReadOnly` with `selectSpeleoReadOnly`. Do not write that
composition inline. `useUIStore(isMapReadOnly) || useSpeleoStore(selectSpeleoReadOnly)`
is a Rules-of-Hooks violation: `||` short-circuits, so the second hook is skipped
on any render where the first term is already true, and React throws "Rendered
fewer hooks than expected" when it flips back. The hook calls both
unconditionally; `src/store/__tests__/mapReadOnlyComposition.test.tsx` pins that.

### What the lint enforces

`pnpm lint:readonly-gates` (`scripts/check-readonly-gates.mjs`, a `check`-job CI
gate, self-tested by `pnpm lint:readonly-gates:test`) scans tracked non-test
`src/` files for two things:

- **Raw `=== 'closureAdjusted'` / `!== 'closureAdjusted'` comparisons**, counted
  against an exact per-file allowlist. Exact in both directions: a count that
  drops below baseline usually means a gate was deleted rather than migrated.
  The allowlisted sites are display and coordinate-selection logic, the
  definition of `isMapReadOnly` itself, the geometry-mode radio control, and an
  import-blocked error-message selector - each carries a written reason in the
  script.
- **Direct references to `isMapReadOnly` or `selectSpeleoReadOnly`** outside the
  two store modules that define them and the helper that composes them. Three
  more files are exempt with a written reason.

Two carve-outs are deliberate and trip neither rule. Undo/redo (toolbar buttons
and keyboard shortcuts) gate on `isEditableMode(appMode)` alone, so they stay
available in the Closures Computed View of Survey and Data, and the SpeleoDB case is
left to the backend rejection. `ContextMenu`'s raw `geometryMode` check sits inside
a branch already scoped to an editable mode with a map (Survey, or Data with its
pane open), where it is equivalent.

## Mode entry and exit

`setAppMode` owns all of it. The rules, by edge:

- **Leaving Survey for any mode** exits section focus and data-table fullscreen
  (both are Survey sub-states), and points the left dock at SpeleoDB, remembering
  in `speleoOutsideSurvey` whether SpeleoDB was showing. **Returning to Survey**
  brings back `surveyLeftWorkspace`.
- **Survey <-> Data** changes nothing else. Both edit the same map, so the geometry
  mode, the display and colour flags, the docks and a pending measurement carry over
  as they are; no snapshot is taken or restored. Data hides the right and bottom
  docks by not rendering them and never touches their store flags, so Survey gets
  its own layout back.
- **Editable -> read-only** (Survey or Data -> Plan or Export) snapshots
  `prePlanGeometryMode`, `prePlanDisplay` (the display/overlay flags plus grid
  spacing), `prePlanDocks` and `preReadOnlyMode` (which editable mode it was) into
  module-level vars, then forces `closureAdjusted`, clears the colour overlays and
  the measurement, and reveals the right dock. The dock snapshot is taken *after*
  the exits above, because those may have just restored the real pre-focus dock
  state. A snapshot taken in Data is Survey's, since Data leaves the dock flags
  alone.
- **Read-only -> read-only** (Plan <-> Export) forces the same state again but
  takes no snapshot, so the editable view saved on the way in is not overwritten
  with `'closureAdjusted'`.
- **Read-only -> editable** restores all of it - into Survey or Data alike - nulls
  the vars, and clears the pending measurement (the ruler readout depends on which
  geometry is active).

`exitGasPlan()` is the "leave a read-only mode" funnel used by `setView`, the
colour-overlay actions (`setColorMode`, Shift+D's `toggleDisplay`, the distance and
issue overlays), `startPlacementMode` and a project close. It is a **no-op in any
editable mode** - otherwise picking a colour in Data's map pane would eject the user
to Survey - and from Plan or Export it returns to `preReadOnlyMode`, the editable
mode the user came from.

Everything the mode forces is written with a plain `set()`, never through
`setGeometryMode` / `toggleDetailPanel` / `toggleDataTable`. A mode-forced
change is not a user action: routing it through those would persist the forced
Closures Computed View as the remembered geometry preference, or write a
forced-open dock into the saved panel layout. For the same reason `toggleDisplay`
and `setGridSpacing` write to the saved config only in an editable mode; that,
plus the entry snapshot, is what scopes Plan's Display changes to Plan. A Display
change in Data is a Survey change and is saved.

The gas store is *not* reset on a mode switch, so a loaded plan survives a
Plan -> Export -> Plan round-trip within a session.

A **project load resets the mode**. `applyDisplaySettings` sets
`appMode: APP_MODE_DEFAULT` and clears all four snapshot vars (and the gas
store) before applying the loaded project's saved geometry. Those vars are
module-level and outlive the project that set them, so without the clear a later
`exitGasPlan()` would restore the *previous* project's geometry, display flags
and docks over the freshly loaded ones. `uiStore.planDocks.test.ts` pins it.

## Adding a mode-gated feature

- Gate every mutation entry point on `useEditingBlocked()` or
  `isEditingBlockedNow()`. Not on `geometryMode`, not on `appMode` alone.
- Decide "does this mode edit" with `isEditableMode(appMode)`, never with
  `appMode === 'survey'`, which silently drops Data. Keep `appMode === 'survey'`
  for what only Survey renders.
- Data's map can be hidden. Anything aimed at the map - a keyboard shortcut, the
  context menu, a tool gesture, map chrome - must also respect
  `useDataWorkspaceStore.mapPaneOpen` in Data (see `dataTableOwnsKeys` in
  `useKeyboardShortcuts.ts`).
- Map chrome goes inside `DataModeLayout`'s children as a `{cond && ...}` slot.
  Never move or wrap `MapCanvas` per mode: a changed tree position remounts it.
- A flow that needs the Details panel or the full map switches to Survey first, as
  `revealWarningsTab` and `startPlacementMode` do.
- Re-check at commit time for anything that outlives the switch: a dialog left
  open across a mode change. `InsertStationDialog` re-checks in its confirm
  handler for exactly that reason.
- Hiding a control by `appMode` is a UI decision, not a gate. Gate the action
  too: keyboard shortcuts, the context menu and dialogs reach it another way.
- Any state a read-only mode force-sets must be snapshotted on the editable ->
  read-only edge, restored on the way back, and cleared in `applyDisplaySettings`
  - or it leaks into the next project.
- Run `pnpm lint:readonly-gates` before pushing; a new raw `closureAdjusted`
  comparison or a direct `isMapReadOnly` import fails the CI `check` job.
- For e2e: specs switch modes by clicking `[data-testid="mode-survey"]`,
  `mode-data` or `mode-plan`, and enter Export through the project menu or Ctrl+E.
  `window.__E2E_TEST__.dataWorkspaceStore` opens and closes Data's map pane
  (`e2e-playwright/workspace-modes.spec.ts`). A new IPC command needs a case in
  `e2e-playwright/mocks/tauri-ipc.ts` (unknown commands fall through to a warning
  and `null`, which the UI reads as failure), and the mock must initialize any
  store state your new gate reads, or the control is silently dead in every spec.

## Left project workspace

Sections remains Survey-only. All projects expose SpeleoDB navigation in every
mode through the full toolbar button. Data, Plan and Export keep the left dock hidden
until requested unless SpeleoDB was selected before leaving Survey. Returning restores
the Survey tab. History, synchronization and Close Project controls and links to application Settings remain accessible in every
mode; survey and revision editing stay Survey-only. Hidden tab bodies retain their
state, and tab navigation does not acquire or release a lock.

Only Start Editing can open the remote edit gate, after acquiring the lock and
verifying live revision history: the gate is the lock claim bound to the editing
session, so editing without a held lock cannot happen. Releasing or losing the
lock shuts it, and an in-place reload keeps a session only while its claim is
bound - Take remote releases first, so it comes back read-only. Status refresh can
close a lost session but cannot resume one. Switch to Read Only Mode requires locally saved and uploaded work.
Reload Project preserves the existing mode and does not acquire a lock; pending
work requires Discard changes / Cancel. Local settings retains validation and
elevation gates, while linked names remain immutable. Account and connection forms
exist only in application Settings. Local project editors expose only the centered conversion prompt in their SpeleoDB tab.
The creation form saves, uploads and opens a fresh remote copy; successful conversion
then exposes the managed-project workspace.
