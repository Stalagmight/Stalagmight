# Workspace modes and the read-only contract

An open project is in exactly one of three workspace modes - `survey`, `plan`,
`export` - held in `uiStore.appMode` and switched by the `ModeCarousel`.
[SPEC.md 6.1](SPEC.md#61-workspace-modes) describes what each one is for; this page is the contract
the code has to keep, because getting it wrong is silent: a mutation leaks into
a read-only workspace and nothing complains until a user edits a project they
were only planning a dive on.

## What each mode swaps

Every row is a render-time `appMode` branch in `SingleSystemView.tsx`,
`Toolbar.tsx`, or (for the Display menu's own contents) `DisplayMenu.tsx`. The
map itself is the same map in all three.

| | Survey | Plan | Export |
|---|---|---|---|
| Sections tab | shown | hidden | hidden |
| SpeleoDB workspace | available | on request or already visible | on request or already visible |
| Right dock | `DetailPanel` | `GasPlanPanel` | `ExportPanel` |
| Bottom dock | `DataTable` | `PlanTablesPanel` | not rendered |
| Toolbar tools | `TOOLBAR_ITEMS` | `READ_ONLY_TOOLBAR_ITEMS` | `READ_ONLY_TOOLBAR_ITEMS` |
| Display menu | full | no Color, no Geometry group | hidden |
| Map area | - | - | `ExportPreview` overlay for the image export type |

`READ_ONLY_TOOLBAR_ITEMS` is `TOOLBAR_ITEMS` filtered through `isMutatingTool`
(dangling separators collapsed), so mutating tools are **hidden, not disabled**
and the two lists cannot drift.

Whichever base layout the mode picks then goes through a second pass, dropping
the navigate group unless `map.controls.show_navigation_tools` is on (#409). So
Plan and Export are `select | navigate group | measure` with navigation tools on,
and `select | measure` with them off - the default. Both passes run through
`filterToolbarItems` (`Toolbar.tsx`), whose separator cleanup is what makes them
composable: each pass re-tidies the dividers the previous one stranded.

## The contract

The predicate is `isMapReadOnly` in `src/store/uiStore.ts`:
`geometryMode === 'closureAdjusted' || appMode !== 'survey'`.

Both terms are load-bearing, and the second is the one that keeps being
forgotten. Entering Plan or Export forces `geometryMode: 'closureAdjusted'` -
but `MapCanvas` reverts it to `'measured'` when the project has no eligible
closures, and again when an adjusted-geometry fetch fails. So on a freshly
imported project with no closures, Plan mode runs with
`geometryMode === 'measured'`, and a gate written as
`geometryMode === 'closureAdjusted'` reports "editable" in a workspace that must
not be. `DataTable`, `SectionsPanel`, `WarningResolution` and `DetailPanel` all
shipped with exactly that bug.

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
and keyboard shortcuts) gate on `appMode` alone, so they stay available in
Survey's Closures Computed View and the SpeleoDB case is left to the backend
rejection. `ContextMenu`'s raw `geometryMode` check sits inside a branch already
scoped to `appMode === 'survey'`, where it is equivalent.

## Mode entry and exit

`setAppMode` owns all of it. Entering Plan or Export exits section focus and
data-table fullscreen, then - **only when coming from Survey** - snapshots
`prePlanGeometryMode`, `prePlanDisplay` (the display/overlay flags plus grid
spacing) and `prePlanDocks` into module-level vars. The survey-only condition is
what keeps a plan -> export switch from overwriting the saved survey geometry
with `'closureAdjusted'`. The dock snapshot is taken *after* the two exits
above, because those may have just restored the real pre-focus dock state.

Everything the mode forces is written with a plain `set()`, never through
`setGeometryMode` / `toggleDetailPanel` / `toggleDataTable`. A mode-forced
change is not a user action: routing it through those would persist the forced
Closures Computed View as the remembered geometry preference, or write a
forced-open dock into the saved panel layout. For the same reason `toggleDisplay`
and `setGridSpacing` write to the saved config only in Survey; that, plus the
entry snapshot, is what scopes Plan's Display changes to Plan.

Returning to Survey restores all three snapshots, nulls the vars, and clears the
pending measurement (the ruler readout depends on which geometry is active). The
gas store is *not* reset on a mode switch, so a loaded plan survives a
Plan -> Export -> Plan round-trip within a session.

A **project load resets the mode**. `applyDisplaySettings` sets
`appMode: APP_MODE_DEFAULT` and clears all three snapshot vars (and the gas
store) before applying the loaded project's saved geometry. Those vars are
module-level and outlive the project that set them, so without the clear a later
`exitGasPlan()` would restore the *previous* project's geometry, display flags
and docks over the freshly loaded ones. `uiStore.planDocks.test.ts` pins it.

## Adding a mode-gated feature

- Gate every mutation entry point on `useEditingBlocked()` or
  `isEditingBlockedNow()`. Not on `geometryMode`, not on `appMode` alone.
- Re-check at commit time for anything that outlives the switch: a dialog left
  open across a mode change. `InsertStationDialog` re-checks in its confirm
  handler for exactly that reason.
- Hiding a control by `appMode` is a UI decision, not a gate. Gate the action
  too: keyboard shortcuts, the context menu and dialogs reach it another way.
- Any state the mode force-sets must be snapshotted on the survey -> non-survey
  edge, restored on the way back, and cleared in `applyDisplaySettings` - or it
  leaks into the next project.
- Run `pnpm lint:readonly-gates` before pushing; a new raw `closureAdjusted`
  comparison or a direct `isMapReadOnly` import fails the CI `check` job.
- For e2e: specs enter a mode by clicking `[data-testid="mode-plan"]` /
  `mode-export`. A new IPC command needs a case in
  `e2e-playwright/mocks/tauri-ipc.ts` (unknown commands fall through to a warning
  and `null`, which the UI reads as failure), and the mock must initialize any
  store state your new gate reads, or the control is silently dead in every spec.

## Left project workspace

Sections remains Survey-only. All projects expose SpeleoDB navigation in every
mode through the full toolbar button. Plan/Export keep the left dock hidden until
requested unless SpeleoDB was selected before leaving Survey. Returning restores
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
