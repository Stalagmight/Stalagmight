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
| Left Sections dock | shown | not rendered | not rendered |
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

**The backend backstops the `appMode` half (#576 C5), not the `geometryMode`
half.** The frontend reports every `appMode` change to the backend
(`set_workspace_mode`, sent by `src/store/workspaceModeSync.ts`, started from
`main.tsx`), and the backend resets its copy to Survey on every project open,
create and close - the same reset `applyDisplaySettings` does. While the mode is
Plan or Export, `AppState::guard_mutation` refuses every entry point that calls it
with the code `WORKSPACE_READ_ONLY` (#576 C4), which the frontend shows as its own
notice ("switch to Survey", `editGate.workspaceReadOnlyNotice`), not as the
SpeleoDB lock notice. It is **deny by default**: a new entry point that calls
`guard_mutation` is refused in Plan and Export without anyone having to remember.
The allowlist is two entries, on purpose: the project-settings save
(`update_project_settings`: the name and the validation thresholds) and the project
rename (`set_project_name_for_path`), which `ProjectSettingsDialog` keeps enabled in
every mode; they call `guard_metadata_mutation`, which skips only the workspace check.
The default north reference (#576 NF-W3-2) rides the same save but is NOT on the
allowlist (NF-W3-2-PLAN): it turns every section that declares no reference of its
own, so the map, the closures and every exported bearing move with it - a survey
edit, like the entrance elevation. In Plan and Export a save that would change it is
refused whole with the workspace notice, an unread one is not set from there (saves
stay off until it is set in Survey), and the dialog disables its row with the reason.

The SpeleoDB verbs that replace the survey - Restore revision and Unwind
(`speleodb_restore_revision` with `restore`), Take remote (`speleodb_take_remote`)
and Discard changes (`speleodb_discard_changes`) - run outside the command
chokepoint and call `guard_workspace` themselves (through
`speleodb_guard_workspace`), with the same notice and before any network call
(#576 RV2-R-15, RV4-R-NITS S-8); a View is a read-only peek and stays allowed. Recording an imported file's hash
happens inside the import command itself - there is no separate IPC command for
it (#576 NF-TRACK-IMPORTED) - so the import's own refusal covers it.

That gate sits after the two older ones, which still refuse first: a project saved
by a newer Stalagmight (`newer_file_schema`, #576 B6-D1; see
[saving.md](saving.md#files-saved-by-a-newer-version-open-read-only)) and a remote
SpeleoDB project opened without the write lock (`edit_enabled == false`). All three
live in `src-tauri/crates/core/src/core/state/app_state.rs` and
`state/workspace_mode.rs`; the refused set is pinned by `MUTATION_ENTRYPOINTS` and
`state/workspace_mode_tests.rs`. That list is still hand-maintained; it is to be
derived from the IPC command manifest when that lands (#576 C3).

Survey's **Closures Computed View is not mirrored**: it is a Survey sub-view, and
its undo/redo carve-out (below) has to keep working - the backend cannot tell that
undo from any other. Read-only there rests on UI gating alone, and so does every
mode in the instant before the mode change reaches the backend. The backend gate
is the backstop, not the gate: keep gating every entry point in the UI, which is
why the gating is linted.

Command reachability is not a boundary to lean on either. `tauri.conf.json` sets
`withGlobalTauri: true` (the agent harness drives the app through
`window.__TAURI__`), but turning it off would not make the commands unreachable
from a script in the webview: Tauri 2 always injects `window.__TAURI_INTERNALS__`,
which `@tauri-apps/api` itself invokes through. Whatever a page script can do, the
backend gates above are what stop it.

### The one sanctioned gate

Every mutation entry point goes through `src/hooks/useEditingBlocked.ts`:
`useEditingBlocked()` inside a component, `isEditingBlockedNow()` outside React
(raw keydown handlers, plain utilities).

Both compose `isMapReadOnly` with `selectSpeleoReadOnly` and, for a project saved
by a newer Stalagmight, `selectNewerFileReadOnly` (`projectStore.ts`, set from the
load result in the same `set()` as the project). A fourth term,
`selectUpdateInstalling` (`updateStore.ts`), holds while an update restart installs:
the user has already passed the unsaved-changes prompt and the restart would drop
any edit, so nothing may change the project until the install ends. It goes up as
soon as the restart's close request is past the tier-1 confirm, so it also covers
the exit guard's wait for queued edits, and comes down if the guard prompts. It has no
backend backstop; the mutation queue (`executeCommand` / `executeUndoRedo`) refuses
every edit and undo/redo on it instead, and `UpdateInstallOverlay` holds the screen
and keyboard. A dialog asked for during the hold (`openDialog`) opens only once it
lifts, never above the overlay (`afterUpdateInstallHold`). Do not write that composition inline. `useUIStore(isMapReadOnly) || useSpeleoStore(selectSpeleoReadOnly)`
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

A third carve-out covers the SpeleoDB verbs that swap the survey tree
(#576 RV4-FE-SP4, RV5-F4-8): Discard and Take remote (both
`refuseSurveySwapOutsideSurvey` in `useSpeleoActions.ts`), Restore and the Revert built on it (`useRevertRevision.ts`,
`SpeleoRevisionHistory.tsx`) and their hamburger entries (`useSpeleoMenuItems.tsx`).
They gate on the workspace alone (`appMode !== 'survey'`), not on
`useEditingBlocked()`: they must work without the write lock - they take it
themselves, or the project is read-only precisely because the user is about to
discard or replace it - so the lock half of the composed gate would refuse them
exactly where they are needed. The backend refuses them in Plan and Export too
(`speleodb_guard_workspace`).

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
pending measurement (the ruler readout depends on which geometry is active).

A move ghost in flight does not survive the switch: leaving Survey makes
`isEditingBlockedNow()` true, which cancels the carry with a toast before the
isMutatingTool effect drops the tool back to Select, so no click in Plan or Export can
drop it (see [move-tools.md, "Integrity while carrying"](move-tools.md#integrity-while-carrying)).
The same holds for the Closures Computed View and a lost SpeleoDB lock. The
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
  `isEditingBlockedNow()`. Not on `geometryMode`, not on `appMode` alone -
  except the carve-outs listed under "What the lint enforces".
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
- A new backend mutation entry point calls `guard_mutation` (refused in Plan
  and Export) unless it is deliberately editable there; then it calls
  `guard_metadata_mutation`, joins the allowlist in `workspace_mode_tests.rs`,
  and this page says why.
- For e2e: specs enter a mode by clicking `[data-testid="mode-plan"]` /
  `mode-export`, and the mock refuses the guarded commands there as the backend
  does (`WORKSPACE_GUARDED_COMMANDS`). A new IPC command needs a case in
  `e2e-playwright/mocks/tauri-ipc.ts` (unknown commands fall through to a warning
  and `null`, which the UI reads as failure), and the mock must initialize any
  store state your new gate reads, or the control is silently dead in every spec.
