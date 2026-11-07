# Move Tools

Developer reference for the three move tools: **Move Station**, **Move Shot**, and **Move Segment**.

## Overview

| Tool | Shortcut | Scope | What moves |
|------|----------|-------|------------|
| Move Station | T | Single station | Station + co-located stations (merged/aligned-closure) |
| Move Shot | G | One shot | Both endpoints + all co-located stations |
| Move Segment | V | Entire branch | All sections connected via merges/aligned-closures |

All three share the same drag preview engine and Ctrl+drag panning. They differ in scope and hover behavior.

---

## Move Station

Click a station to begin dragging it. If the station shares coordinates with other stations (via merges or **aligned** loop closures), all co-located stations move together.

**Hover**: Only stations get hover highlight. Shot hover is suppressed.

**Drag flow**:
1. Click on station &rarr; `handleMoveDragStart` (`useMoveDrag.ts`) &rarr; immovable guard check
2. `beginStationDragPreview()` (SurveyLayer) &rarr; BFS to find co-located stations, create temp layers + dynamic sources for connected shots
3. Per-mousemove: `updateStationDragPreview(dx, dy)` updates *-translate and rebuilds connected shot geometry. Snap-to-merge detection runs each frame (see [Snap-to-Merge](#snap-to-merge)).
4. On the release of a press that stayed put (see [The drop is click-gated](#the-drop-is-click-gated)): if snapped, opens merge/closure dialog. Otherwise if movement > 1mm, `commitDragPreview()` + `moveStation()` backend call (batched for co-located stations)

## Move Shot

Click a shot to begin dragging it. Both endpoints move rigidly together, along with any stations co-located at either endpoint (merges + aligned closures).

**Hover**: Only shots get hover highlight. Station hover is suppressed.

**Drag flow**: Same as Move Station but `beginStationDragPreview()` is seeded with both endpoint station refs. The drag preview includes all shots connected to either endpoint.

**Immovability**: If either endpoint station is immovable, the entire shot cannot be dragged.

## Move Segment

Click any station or shot in a section to drag the entire connected branch. Connected sections (via merges/aligned closures) move together as a rigid unit.

**Hover**: Both stations and shots get hover. Hovering highlights the full connected segment (all connected sections).

**Drag flow**:
1. Click on station/shot &rarr; `handleMoveDragStart` &rarr; anchored section guards
2. Async `getConnectedSectionIds(sectionId)` IPC call fetches connected sections
3. Connected sections filtered to exclude anchored ones
4. `beginDragPreview(connectedSet)` creates temp layers with section inclusion filters
5. Per-mousemove: `updateDragPreview(dx, dy)` updates *-translate on all temp layers (GPU uniform, no GeoJSON rebuild)
6. On the release of a press that stayed put (see [The drop is click-gated](#the-drop-is-click-gated)): `commitDragPreview()` + a single `moveSegment()` backend call with the grabbed section id (the backend expands it to the whole rigid connected group)

### Anchored Sections

Sections that cannot be moved (e.g., externally constrained). Fetched from backend when Move Segment tool activates via `getAnchoredSectionIds()`. Drag is blocked until the fetch completes (`anchoredIdsLoaded` guard). Anchored sections appear dimmed (0.25 opacity via `feature-state: anchored`) and show no pointer cursor on hover.

### Snap-to-Merge

During station or segment drag, the user can drop onto another station to merge them:

1. **Detection**: Origin station's screen position is tracked + offset by drag amount. For station drag, co-located stations (being dragged together) are excluded from candidates. For segment drag, stations in the dragged segment are excluded.
2. **Preview**: When a snap target is found, drag preview offset is adjusted so origin station lands exactly on the target. A visual snap indicator is shown.
3. **Drop**: On a drop (a release that passed the click gate) with a snap target, `promptSnapMerge()` checks connectivity via the backend (`checkMergeConnectivity`) and opens the appropriate merge or closure dialog (`snapMergeDialogPendingRef` set to block further interactions). While that dialog is up the drag is *paused*, not ended: `moveDrag` is held alive so Cancel resumes it, and `handlePointerUp` returns on `snapMergeDialogPendingRef` before it reaches the click gate at all.
4. **Cancel**: Restores preview to raw offset; user can continue dragging.

### Segment Hover Resolution

When hovering in Move Segment mode, the connected section IDs are resolved asynchronously. A debounce (150ms) prevents flicker when transitioning between features. The `segmentHoverTokenRef` system discards stale async results (the drag path has its own `segmentDragTokenRef`).

---

## Immovable Stations

A station is immovable if it is a zero-datum or transitively reachable from one via merges and **aligned** loop closures. Computed in `MapCanvas` via BFS when Move Station or Move Shot is active. Returns `Set<string>` of `"sectionId:stationIndex"` keys.

Visual feedback:
- Immovable stations and shots with immovable endpoints are dimmed (0.25 opacity via `feature-state: anchored`)
- Hover does not show pointer cursor
- Drag start is blocked

The `setImmovableFeatures()` method on SurveyLayer sets per-feature `anchored` state and dims associated labels and closure lines.

---

## Drag Preview Engine

### Two-Layer Architecture

| Approach | Used for | Performance |
|----------|----------|-------------|
| Temp layers with `*-translate` paint | Static geometry (segment drag, station/shot circles) | GPU uniform update, zero GeoJSON cost |
| Dynamic GeoJSON sources | Geometry that must rebuild (connected shots during station drag) | Per-frame GeoJSON rebuild |

### Temp Layer Definitions

For each main layer (shots, stations, labels), a corresponding temp drag layer is created with the same paint/layout but an inclusion filter for only the dragged features. The main layers get exclusion filters to hide the originals.

### Flow

```
beginDragPreview()    -- save filters, apply exclusions, create temp layers
updateDragPreview()   -- update *-translate (+ rebuild dynamic sources)
commitDragPreview()   -- remove temp layers, keep exclusions (prevents blink during React re-render)
endDragPreview()      -- remove everything, restore original filters
```

### Why Refs Instead of React State

Drag tracking uses refs (`moveDragLngLatRef`, `lastCursorClientRef`, etc.) instead of React state because mousemove fires 60+ times/sec and state updates would cause massive re-renders. SurveyLayer is updated via direct method calls.

---

## The drop is click-gated

A plain left-drag pans the map under the move tools like it does under every
other tool (#409): MapLibre's own `dragPan` is enabled for them, and it stays
enabled while a ghost is in flight - `mapGestureMode`
(`src/utils/mapGestures.ts`) cannot even see the ghost. What distinguishes a
drop from a pan is not who owns the button but how far the press travelled:

| Gesture while carrying | What happens |
|------------------------|--------------|
| Primary-button press and release under `CUSTOM_PAN_CLAIM_PX` (3px) of travel - a **click** | The drop: commit, or the snap-merge dialog |
| A still right- or middle-click | Nothing: the ghost stays in flight. Only the primary button drops, and a right-click opens no context menu while carrying, on a station or shot or not (and no native browser menu) |
| Press, travel, release - a **drag** | MapLibre pans; nothing commits, the ghost stays in flight and keeps following the cursor |
| Release outside the canvas | Never reaches `handlePointerUp`; the drag-release forwarder ends MapLibre's pan, the ghost survives |
| Escape | Cancels the ghost outright (the `moveDrag` Escape effect), pan or no pan |

`handlePointerUp` (MapCanvas) implements it against `pressClientRef`, the
client-coordinate press point recorded by `handlePointerDown` for every press
that reaches the canvas. The decision **latches**: the first `pointermove` past
the threshold nulls the ref, so a drag that wanders out and comes back cannot
drop on release - exactly how the custom pan's own `customPanClaimed` never
un-claims. A null ref at release also covers a press that started off-canvas, so
neither of those is a drop.

The threshold is the same `CUSTOM_PAN_CLAIM_PX` constant the custom pan claims
on, deliberately: under it a gesture is a click on both sides of the boundary
(and to MapLibre, whose `clickTolerance` it equals), at or above it a drag. One
consequence worth stating: a *still* Ctrl+click drops the ghost like a plain one.
Nothing on the release path consults `ctrlKey`, and Ctrl+click's other meaning -
the selection toggle - is already ignored under the move tools by
`SurveyLayer.handleMapClick`, so there is nothing for the modifier to protect.

On touch this falls out for free: `touchAction: 'manipulation'`, so one finger
pans, a tap drops, and a pinch moves both fingers far past the threshold and
therefore cannot drop anything.

## Integrity while carrying

The ghost and the commit it ends in are computed against the survey as it stood
at the **pickup**, and station refs are positional (`section_id:station_index`).
Any other edit landing between the pickup and the drop could therefore make the
commit move the wrong station or apply a stale offset. So while a carry is in
flight no other survey mutation runs (GHOST-2, #576).

A carry is **in flight** from the pickup click until the drop click starts its
commit, or until it is cancelled. For Move Segment that includes the pending
`getConnectedSectionIds` window, before the ghost is even up. The snap-merge
dialog phase counts too, and that dialog's confirm is the carry's own commit.

A **survey mutation** is any IPC command whose backend handler calls
`AppState::guard_mutation`. Read-only queries (`check_merge_connectivity`,
`get_*`, `can_undo`, ...) and app-config writes are not mutations.

| Rule | What happens | Where it lives |
|------|--------------|----------------|
| R1: another mutation is issued | Not sent. The carry is cancelled as Escape cancels it, one `move-cancelled` warning toast says neither change happened, and the caller gets a refused result (`success: false`). Not refused: the direct `invokeIpc` routes that replace the survey instead of editing it, none of whose handlers calls `guard_mutation`. These are SpeleoDB revision restore/view, take-remote and open (`speleodb_restore_revision`, `speleodb_take_remote`, `speleodb_open_project` and its `_discarding_local` variant), and the corruption-recovery dialog's backup restore and repair (`restore_from_backup`, `attempt_repair`), which rewrite the file that failed to load and reach the open survey only through the dialog's Re-open. The carry is cancelled once the replacement reaches the store: by R2 when a load changes the project identity or the generation, and by R4 when `applySpeleoTreeReplacement` (restore/view, take-remote, and an open of the project already on screen) resets the tool to Select ahead of its refresh | `refuseMutationDuringCarry` in `src/store/moveCarry.ts`, called at ISSUE time (before `runSerialized`) by `executeCommand` and `executeUndoRedo` in `projectStore`, and by the routes that bypass them: `resolveWarning`, `unresolveWarning`, `repairProjectIntegrity` and the DMP wizard's four direct `import_sections` calls |
| R2: the survey changes underneath | `projectTreeGeneration` moves off the one captured at pickup (a refresh after a mutation that was already in flight, a SpeleoDB tree replacement, an import, a config refetch, the save-path heal): cancelled, with a toast. A project load, create or close (the project's `created_at` identity changes) cancels it silently: the ghost goes with its survey. Also when `isEditingBlockedNow()` turns true (Plan/Export, Closures Computed View, lost SpeleoDB lock), with its own toast | Store subscriptions in `src/components/Map/useMoveCarryGuards.ts` |
| R3: the commit is stale | The carry's own commit is re-checked inside the serialized section, right before `invokeIpc`, because it can queue behind a mutation that was in flight. A generation mismatch refuses it with nothing sent, restores the preview (`endDragPreview`) and toasts. A snap-merge dialog confirmed after its carry was cancelled sends nothing | `commitMoveCarry` marks the one command the commit issues (`takeMoveCarryCommit`); `refuseStaleCarryCommit` runs in `executeCommandInner` |
| R4: another tool is chosen | Switching to anything but the three move tools, or entering placement mode (DMP / new-section placement), cancels the carry silently, so one click can never both drop the ghost and run another tool's action. Switching among the move tools keeps it | `useMoveCarryGuards.ts` |
| R5: one cancel routine | Escape, R1, R2 and R4 all run `cancelMoveCarryState` (`useMoveDrag.ts`), which also closes a snap-merge dialog this carry opened. R3 restores the preview through the same registration | `useMoveDrag.ts` |

Choices worth knowing:

- **Refused at the chokepoint, not disabled in the UI.** The carry does not feed
  `useEditingBlocked`, so nothing is greyed out and no tool is reset by a
  pickup. The user can still reach every panel while carrying; whatever they
  change there cancels the move instead.
- **One notice.** A refusal is recognisable with `isMoveCarryRefusal(result)`,
  so callers that would surface a failure of their own stay quiet: Project
  Settings skips its error toast, the undo history dropdown its error dialog,
  the import dialogs (TML/Compass/Walls/GPX) stay open instead of routing to
  `surfaceImportFailure`, Repair skips its "failed" toast (the refusal is thrown,
  since `IntegrityRepairResult` has no failure shape), and the Warnings "Ignore"
  action checks before it calls `resolveWarning` (whose boolean result cannot say
  why it failed). Dialogs that show `result.error` inline show the refusal text,
  which is accurate. The failed result is not followed by a refresh: nothing
  changed, the table and editors read the store, and a refresh would mint a new
  generation.
- **Toasts.** A refused mutation and a survey change toast, and so does editing
  becoming blocked: none of them is something the user did to the move. A tool
  switch, like Escape, is the user's own gesture and stays silent.

## Ctrl+Drag Panning

Both modifier gestures work under every tool but Pan (`activeTool !== 'pan'` in
`handleMouseDownCapture`), and both are scaled by `map.controls.pan_speed`, where
`dragPan` is always 1:1 with the cursor. They are implemented by hand because
MapLibre cannot do either: its mouse-pan handler is built with
`checkCorrectEvent: (e) => mouseButton(e) === LEFT_BUTTON && !e.ctrlKey`
(maplibre-gl 5.16 `generateMousePanHandler`), so a Ctrl+left press never even
arms `DragHandler`, and Ctrl+left's own MapLibre gestures - rotate, pitch, box
zoom - are all disabled at map init. Handing a Ctrl+drag to the enabled `dragPan`
pans nothing at all, which `ctrl-drag-pan.spec.ts` pins under the Pan tool (the
one tool the custom handler keeps its hands off).

### When the Ctrl press is claimed

A Ctrl+left press is not the same gesture as a Ctrl+left DRAG: the press alone is
the multi-select toggle (`handleStationClick`'s `ctrlKey` branch in
`useMapTools`). So `handleMouseDownCapture` claims it outright only where
MapLibre's `dragPan` is already off - the two modal modes, whose own pointerdown
handling (placement confirm, zoom box) must not run just because Ctrl was held.
Everywhere else the press is only **watched**: it is left to propagate,
and the custom pan takes over on the first `mousemove` past
`CUSTOM_PAN_CLAIM_PX` (3px, MapLibre's own `clickTolerance`). Below that the
gesture ends as a click and reaches the map; above it MapLibre suppresses its own
click anyway, so only one of the two can ever fire. A middle press is always
claimed outright - it has no click meaning to protect.

### Event Capture Strategy

Capture-phase handlers on the map container intercept events **before** MapLibre's handlers on the canvas. All of it applies to a CLAIMED gesture; a watched press is untouched until it becomes one:

| Event | During Ctrl+drag | Purpose |
|-------|-----------------|---------|
| `pointerdown` | `stopPropagation` | Prevent MapLibre pointer tracking (outright claims only) |
| `mousedown` | `stopPropagation`, `preventDefault` | Start pan, prevent MapLibre click tracking (outright claims only) |
| `mousemove` | `stopPropagation` | Pan by delta, suppress MapLibre hover updates |
| `pointerup` | `stopPropagation` | Prevent MapLibre pointer tracking |
| `mouseup` | `stopPropagation`, `preventDefault` | End pan, restore cursor |
| `click` | `stopPropagation`, `preventDefault` | **Critical**: browser fires `click` as a separate DOM event after mouseup regardless of whether mousedown/mouseup were captured. Must be blocked to prevent MapLibre from processing a spurious click. |

The `click` capture uses `customPanActiveRef` which stays true for 50ms after mouseup, covering the window in which the browser dispatches the click event. That ref is set by the claim, never by the press, which is what leaves a plain Ctrl+click alone.

### Spurious Keyup Filtering

Tauri/WebKit fires spurious Ctrl keyup events at the moment of mousedown/mouseup while Ctrl is held. These are filtered by timestamp: keyups within 50ms of a Ctrl+drag mouse event (`ctrlPanMouseEventTimeRef`) are suppressed. Genuine Ctrl releases (which occur later) go through immediately so the cursor resets without delay.

### Cursor During Ctrl+Pan

| State | Cursor |
|-------|--------|
| Ctrl held, not dragging | `grab` |
| Ctrl+drag in progress | `grabbing` |
| Drag ends, Ctrl still held | `grab` |
| Drag ends, Ctrl released | Tool default |

### Global Fallback

A window-level `mouseup` handler (`handleMouseUpGlobal`) clears pan state if the mouse is released outside the map container.

---

## Cursor Behavior Summary

### Base Cursors

| Tool | Default cursor |
|------|---------------|
| Move Station | `default` |
| Move Shot | `default` |
| Move Segment | `move` |

### Override Rules (highest priority first)

1. **Ctrl+drag or middle-drag active**: `grabbing`
2. **Placement mode**: `crosshair`, whatever is hovered or held
3. **Hovering interactive feature**: `pointer` (unless anchored/immovable) - also while Ctrl is held, since Ctrl+click toggles that feature's selection
4. **Move Station / Move Shot, nothing movable hovered** (anchored/immovable included): `default`, unless Ctrl is held
5. **Ctrl held** (any tool but Pan): `grab`
6. **Otherwise**: the tool's resting cursor (`toolCursor`)

Move Station and Move Shot use `default` as their base cursor (with `pointer` on hover) rather than `move`, so the user sees a clear distinction between "hovering a movable feature" and "not hovering anything".

One owner applies these rules: `src/components/Map/mapCursor.ts` is the only
code that writes the survey canvas's `style.cursor` (#576 D3). MapCanvas pushes
the tool, Ctrl, placement and pan state into it and SurveyLayer pushes its hover
verdict (pointer or not, after the anchored/immovable suppression); the owner
derives the cursor from all of them at once, so the order the inputs arrive in
cannot change what shows. Six writers used to race here, which shipped #24 and
#316 and let a hover or a modifier pan take a placement's crosshair away.

---

## Guards & Edge Cases

| Guard | Location | Purpose |
|-------|----------|---------|
| `snapMergeDialogPendingRef` | `handleMoveDragStart` | Block drag start while merge dialog is open after snap-to-merge |
| `anchoredIdsLoaded` | `handleMoveDragStart` | Block segment drag until anchored IDs fetched from backend |
| `anchoredSectionIds.has()` | `handleMoveDragStart` | Silent no-op for clicks on anchored sections |
| `immovableStationKeys.has()` | `handleMoveDragStart` | Block station/shot drag on immovable stations |
| `segmentDragTokenRef` | Connected section fetch | Discard stale async results if drag cancelled or restarted |
| `customPanActiveRef` | Click capture handler | Block DOM click events during Ctrl+drag cooldown |
| Movement threshold (1mm) | `handleMoveDragEnd` | Distinguish click from drag; skip backend call for sub-millimeter moves |
| `refuseMutationDuringCarry` | `executeCommand` / `executeUndoRedo` and the direct mutation routes | Refuse any other survey mutation while a carry is in flight, cancelling the carry ([Integrity while carrying](#integrity-while-carrying)) |
| `refuseStaleCarryCommit` | `executeCommandInner`, right before `invokeIpc` | Refuse the carry's own commit when the survey generation moved since the pickup |
| `useMoveCarryGuards` | MapCanvas | Cancel the carry when the survey changes, editing becomes blocked, or the tool leaves the move tools |
| `isMoveCarryLive` | `promptSnapMerge`, after `checkMergeConnectivity` | Open no snap-merge dialog for a carry cancelled while the check was out |

---

## Key Files

| File | Responsibility |
|------|---------------|
| `src/components/Map/MapCanvas.tsx` | Immovable/anchored computation, the carry token and its guards, the pointer handlers and the drop's click gate (`pressClientRef`) |
| `src/components/Map/useMoveDrag.ts` | Drag state, pickup / per-move preview / drop, snap-to-merge detection and prompt, the one carry cancel routine |
| `src/components/Map/useModifierPan.ts` | Ctrl+drag / middle-drag panning and the Ctrl-held tracking |
| `src/components/Map/SurveyLayer.ts` | Hover detection, visual state (feature-state), the drag-preview entry points, the hover verdict for the cursor |
| `src/components/Map/surveyLayer/dragPreviewController.ts` | The drag preview session: capture (`beginStationDragPreview` BFS), drag exclusion, overlay follow, commit / end |
| `src/components/Map/mapCursor.ts` | The one writer of the canvas cursor: precedence of pan, placement, hover, Ctrl and tool |
| `src/hooks/useMapTools.ts` | Per-tool click/hover handlers, immovable guards, drag initiation callbacks |
| `src/components/Map/surveyLayer/ghostFeatures.ts` | Ghost section preview GeoJSON (`buildGhostFeatures`) |
| `src/store/moveCarry.ts` | The carry registry: in flight or not, the R1 refusal, the commit marker and the R3 stale check |
| `src/components/Map/useMoveCarryGuards.ts` | The map-side cancels: survey changed, editing blocked, tool switched |
