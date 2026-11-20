# Move Tools

Developer reference for the three move tools: **Move Station**, **Move Shot**, and **Move Segment**.

## Overview

| Tool | Shortcut | Scope | What moves |
|------|----------|-------|------------|
| Move Station | T | Single station | The station (every slot of a junction is one station) + the stations aligned closures tie to it |
| Move Shot | G | One shot | Both endpoint stations + the stations aligned closures tie to them |
| Move Segment | V | Entire branch | The rigid group: every line joined to it by shots at junctions or by aligned closures |

All three share the same drag preview engine and Ctrl+drag panning. They differ in scope and hover behavior.

On the station graph a station is one entity however many lines pass
through it: the map draws a junction once per line (one slot each), and those
slots always move together. The backend side is `survey/ops_move.rs`
(`move_station`, `move_segment`, `move_line`; plan 4.4) behind
`core/state/command_move.rs`:

- **Move Station** rewrites every shot incident to the station, on every line
  through it (a junction has three or more), so the station lands at its
  position plus the offset and every neighbour stays. A legacy hand move on
  the station is folded into the rewritten shots and deleted. When the station
  is its free piece's `Placement` station, the placement moves with it. Refused
  on a station holding a zero datum ("Cannot move a zero datum station") and
  when a rewritten shot would have no length.
- **Move Segment / Move Line** translates the rigid group's `Placement`s;
  refused, for a non-zero offset, when the rigid group holds any datum,
  placement datum or GPS fix ("Cannot move a segment whose rigid group holds a
  zero datum").
- A move by nothing (every axis within 1e-9 m) changes nothing and records no
  undo entry.

---

## Move Station

Click a station to begin dragging it. Every slot of the station moves (its `merged_with` slots), and so do the stations **aligned** loop closures tie to it (its tie group).

**Hover**: Only stations get hover highlight. Shot hover is suppressed.

**Drag flow**:
1. Click on station &rarr; `handleMoveDragStart` (MapCanvas) &rarr; immovable guard check
2. `beginStationDragPreview()` (SurveyLayer) &rarr; BFS to find co-located stations, create temp layers + dynamic sources for connected shots
3. Per-mousemove: `updateStationDragPreview(dx, dy)` updates *-translate and rebuilds connected shot geometry. Snap-to-merge detection runs each frame (see [Snap-to-Merge](#snap-to-merge)).
4. On the release of a press that stayed put (see [The drop is click-gated](#the-drop-is-click-gated)): if snapped, opens merge/closure dialog. Otherwise if movement > 1mm, `commitDragPreview()` + `moveStation()` backend call, ONE per Station ID (`onePerStation` in the identity labels drops the other slots of a junction); several stations of a tie group go as one batch. The backend also moves a station once when a batch names several of its slots (`StationMoves`, plan 9.1)

## Move Shot

Click a shot to begin dragging it. Both endpoints move rigidly together, along with any stations co-located at either endpoint (merges + aligned closures).

**Hover**: Only shots get hover highlight. Station hover is suppressed.

**Drag flow**: Same as Move Station but `beginStationDragPreview()` is seeded with both endpoint station refs. The drag preview includes all shots connected to either endpoint.

**Immovability**: If either endpoint station is immovable, the entire shot cannot be dragged.

## Move Segment

Click any station or shot in a line to drag the entire connected branch. Connected lines (joined at junctions or by aligned closures) move together as a rigid unit.

**Hover**: Both stations and shots get hover. Hovering highlights the full connected segment (all connected lines).

**Drag flow**:
1. Click on station/shot &rarr; `handleMoveDragStart` &rarr; anchored line guards
2. Async `getConnectedLineIds(lineId)` IPC call fetches connected lines
3. Connected lines filtered to exclude anchored ones
4. `beginDragPreview(connectedSet)` creates temp layers with line inclusion filters
5. Per-mousemove: `updateDragPreview(dx, dy)` updates *-translate on all temp layers (GPU uniform, no GeoJSON rebuild)
6. On the release of a press that stayed put (see [The drop is click-gated](#the-drop-is-click-gated)): `commitDragPreview()` + a single `moveSegment()` backend call with the grabbed line id (the backend expands it to the whole rigid connected group)

### Anchored Lines

Lines that cannot be moved (e.g., externally constrained). Fetched from backend when Move Segment tool activates via `getAnchoredLineIds()`. Drag is blocked until the fetch completes (`anchoredIdsLoaded` guard). Anchored lines appear dimmed (0.25 opacity via `feature-state: anchored`) and show no pointer cursor on hover.

### Snap-to-Merge

During station or segment drag, the user can drop onto another station to merge them:

1. **Detection**: Origin station's screen position is tracked + offset by drag amount. For station drag, co-located stations (being dragged together) are excluded from candidates. For segment drag, stations in the dragged segment are excluded.
2. **Preview**: When a snap target is found, drag preview offset is adjusted so origin station lands exactly on the target. A visual snap indicator is shown.
3. **Drop**: On a drop (a release that passed the click gate) with a snap target, `promptSnapMerge()` checks connectivity via the backend (`checkMergeConnectivity`) and opens the appropriate merge or closure dialog (`snapMergeDialogPendingRef` set to block further interactions). While that dialog is up the drag is *paused*, not ended: `moveDrag` is held alive so Cancel resumes it, and `handlePointerUp` returns on `snapMergeDialogPendingRef` before it reaches the click gate at all.
4. **Cancel**: Restores preview to raw offset; user can continue dragging.

### Segment Hover Resolution

When hovering in Move Segment mode, the connected line IDs are resolved asynchronously. A debounce (150ms) prevents flicker when transitioning between features. The `segmentHoverTokenRef` system discards stale async results (the drag path has its own `segmentDragTokenRef`).

---

## Immovable Stations

A station is immovable if it holds a zero datum, or is tied to one by **aligned** loop closures, transitively. Computed in `MapCanvas` via BFS (`computeImmovableStationKeys`, `src/utils/immovableStations.ts`) over `merged_with` (the other slots of a junction) and aligned `closures_with` links when Move Station or Move Shot is active. Returns `Set<string>` of `"lineId:stationIndex"` keys. The backend's derived set is `immovable_stations` (`survey/rigid.rs`; plan Deviation D15: the frontend walk also follows chain-transitive `closures_with` entries, so it can dim a station the backend would move).

"Zero datum" here means EVERY datum, placement datum and GPS fix alike (#741). A GPS fix
station is immovable like any datum station, although a fix never places raw geometry:

- Moving it would rewrite the shots around it and so silently change the gap between the survey
  and the fix's GPS, which the gap warning reports and the Closures Computed View solve pulls
  across. The fix is a measurement of where that station is; dragging the station away from it
  is not an edit of the survey the user can see.
- The two sides agree on the datum stations. The frontend seeds its BFS from the per-slot
  `is_zero_datum` flag, which the wire sets on every slot of a station that holds a datum,
  fixes included (W2); the backend's `move_station` refuses a non-zero offset on any station
  holding a datum ("Cannot move a zero datum station"). Exempting fixes on one side only
  would let the map offer a drag the backend refuses.

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
| Press and release under `CUSTOM_PAN_CLAIM_PX` (3px) of travel - a **click** | The drop: commit, or the snap-merge dialog |
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

1. **Ctrl held**: `grab` (overrides everything except active drag)
2. **Ctrl+drag active**: `grabbing`
3. **Hovering interactive feature**: `pointer` (unless anchored/immovable)
4. **Hovering anchored/immovable feature**: `default` (no pointer)
5. **No hover**: Tool default cursor

Move Station and Move Shot use `default` as their base cursor (with `pointer` on hover) rather than `move`, so the user sees a clear distinction between "hovering a movable feature" and "not hovering anything".

---

## Guards & Edge Cases

| Guard | Location | Purpose |
|-------|----------|---------|
| `snapMergeDialogPendingRef` | `handleMoveDragStart` | Block drag start while merge dialog is open after snap-to-merge |
| `anchoredIdsLoaded` | `handleMoveDragStart` | Block segment drag until anchored IDs fetched from backend |
| `anchoredLineIds.has()` | `handleMoveDragStart` | Silent no-op for clicks on anchored lines |
| `immovableStationKeys.has()` | `handleMoveDragStart` | Block station/shot drag on immovable stations |
| `segmentDragTokenRef` | Connected line fetch | Discard stale async results if drag cancelled or restarted |
| `customPanActiveRef` | Click capture handler | Block DOM click events during Ctrl+drag cooldown |
| Movement threshold (1mm) | `handleMoveDragEnd` | Distinguish click from drag; skip backend call for sub-millimeter moves |

---

## Key Files

| File | Responsibility |
|------|---------------|
| `src/components/Map/MapCanvas.tsx` | Drag state management, Ctrl+drag panning, immovable/anchored computation, snap-to-merge detection |
| `src/components/Map/SurveyLayer.tsx` | Hover detection, visual state (feature-state), drag preview layers, cursor management |
| `src/hooks/useMapTools.ts` | Per-tool click/hover handlers, immovable guards, drag initiation callbacks |
| `src/components/Map/surveyLayer/ghostFeatures.ts` | Ghost line preview GeoJSON (`buildGhostFeatures`) |
