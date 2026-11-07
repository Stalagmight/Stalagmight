# Touch support

Stalagmight is a desktop app that also has an Android tablet build. Touch
support is partial: a handful of surfaces were adapted for it, the rest inherit
whatever the browser's compatibility mouse events give them. It no longer stops
at the map surfaces: the shot grid's context menu grew a long press (#338), and
two Playwright specs dispatch real touch gestures - one at the map, one at the
grid. Read the gaps section before assuming a gesture works.

The Android target is sideload-only (`scripts/build-android.sh`,
`scripts/deploy-android.sh`, `src-tauri/gen/android/`, `tauri.android.conf.json`).
It is not built, run, or tested in CI.

## Where the touch-specific code lives

| File | Symbol | What it does |
|------|--------|--------------|
| `src/hooks/useLongPress.ts` | `useLongPress`, `armCompatEventSuppressor` | Long-press detection for React surfaces; no-op unless `pointerType === 'touch'`. Swallows the compatibility tap the lift synthesizes |
| `src/components/SingleSystemView/SectionsPanel.tsx` | `handleLongPress` / `longPressHandlers` | Long-press a section row -> section context menu. The only `useLongPress` consumer in the tree |
| `src/hooks/useTouchContextMenu.ts` | `useTouchContextMenu` | Container-level touch path into a right-click-only context menu: on fire it dispatches a synthetic `contextmenu` on whatever the finger landed on |
| `src/components/common/SpreadsheetGrid/SpreadsheetGrid.tsx` | the `useTouchContextMenu(scrollRef)` call (`:1911`) | Long-press a grid cell, gutter or header -> the grid's own context menu |
| `src/utils/longPressTouch.ts` | `attachLongPressTouch` | The raw-element long-press machine (500 ms hold, 10 px cancel, swallowed lift), shared by both map layers and the grid |
| `src/components/Map/SurveyLayer.ts` | `initializeEventHandlers` (the `attachLongPressTouch` call), gated by `modalInteractionActive` | Long-press the survey canvas -> survey context menu |
| `src/components/GlobalView/GlobalSurveyLayer.ts` | `initializeEventHandlers` (the `attachLongPressTouch` call) | Long-press the global canvas -> system context menu |
| `src/components/GlobalView/GlobalViewMap.tsx` | per-marker `touch*` listeners, inline in the marker builder; the handler block at `:376-381` | Long-press a project marker -> marker context menu (its own copy - see Long press). The handler block disables `dragRotate`, `touchPitch` and, since #338, the rotation half of the pinch (`:381`) |
| `src/components/Dialogs/SectionPreview.tsx` | the handler block at `:281-286` | The third MapLibre surface: same `dragRotate` / `touchPitch` / `disableRotation` set as the other two |
| `src/utils/mapGestures.ts` | `mapGestureMode` | The one predicate deciding which gestures the map keeps for itself |
| `src/components/Map/MapCanvas.tsx` | the `mapGestureMode` call site, feeding the `dragPan` effect and the container's `touchAction` | Consumes that predicate |
| `src/components/SingleSystemView/useResizeHandle.ts` | `useResizeHandle` | Pointer-capture drag for every resize handle |
| `src/components/common/Panel.tsx`, `src/components/SingleSystemView/Dock.tsx` | `touchAction: 'none'` on the handle | Stops the browser claiming the drag as a scroll. Without it the pointer-capture drag above never gets move events on touch |
| `src/components/Map/PlacementOverlay.tsx` | the `navigator.maxTouchPoints` guard | Renders a Cancel button when `navigator.maxTouchPoints > 0` |
| `src/components/SingleSystemView/Toolbar.tsx` | `lastPointerTypeRef` (`ToolButton` and `ToolGroupButton` each keep one) | Tap-vs-hover behavior for tool buttons and flyouts |
| `src/index.css` | `@media (pointer: coarse)` | Widens `.resize-handle-h` / `.resize-handle-v` by 12px |

## Pointer events vs mouse events

There is no single rule. Three patterns coexist:

- **Unified via Pointer Events.** `MapCanvas`'s outer container uses
  `onPointerDown/Move/Up/Leave`; resize handles use pointer events plus
  `setPointerCapture` (`useResizeHandle.ts:56`). One code path serves mouse and
  touch, with `e.pointerType` consulted only where the behavior must differ.
- **Mouse-only, deliberately.** The custom Ctrl+drag / middle-drag map pan uses
  raw `mousedown`/`mousemove`/`mouseup` capture listeners on the map container
  (`useModifierPan.ts`) and its pointer-capture guards explicitly require
  `e.pointerType === 'mouse'` (`handlePointerDownCapture` / `handlePointerUpCapture`
  there). There is no modifier key on
  touch, so this is intentional - and since #409 no loss either, because plain
  one-finger drag pans.
- **Mouse-only, by omission.** Most of the app was never converted. The
  in-house data grid (`src/components/common/SpreadsheetGrid/`) is otherwise
  `onMouseDown` / `onContextMenu` throughout: its context menu is reachable by a
  long press (see below), but every drag - fill handle, column resize, range
  select, row and column move - is driven by `window` `mousemove`/`mouseup`
  (`SpreadsheetGrid.tsx:1867`) and stays mouse-only. Popover dismissal
  (`src/hooks/useDismissableLayer.ts:136`) listens on `mousedown`. These work on
  touch only to the extent that the browser synthesizes compatibility mouse
  events from a tap; drags do not.

Touch-only behavior is always gated on `pointerType === 'touch'` (React
surfaces) or lives in a separate raw `touch*` listener (the two map canvases and
the grid's scroll container).

`src/utils/inputModality.ts` listens on `pointerdown`, `mousedown` and
`touchstart` to tag `<html data-input-modality>`, which suppresses focus rings
for pointer input. Touch counts as pointer input there.

## Long press

Long press is how touch reaches the context menus that right-click opens on
desktop. Two implementations, split by world rather than by surface: React
surfaces spread the `useLongPress` hook's handlers on an element, and the two
MapLibre canvases - DOM nodes React never sees - share `attachLongPressTouch`
(`src/utils/longPressTouch.ts`), which binds raw `touch*` listeners. The grid is
the in-between case: React renders it, but one `attachLongPressTouch` on its
scroll container serves every cell (see `useTouchContextMenu` below).

| | `useLongPress` | `attachLongPressTouch` |
|---|---|---|
| Delay | `delay` option, default 500 ms (`useLongPress.ts:121`) | `LONG_PRESS_MS` = 500 ms |
| Move cancel | `moveThreshold` option, default 10 px | `MOVE_CANCEL_PX` = 10 px, compared squared |
| Multi-touch | a second finger restarts the hold from that finger (`useLongPress.ts:152`), so the press can still fire - the opposite of `attachLongPressTouch` | armed only at `e.touches.length === 1`, and a second finger cancels a press already pending |
| Events | pointer events, spread onto an element | raw `touch*` listeners on the element |

The canvas constants used to be inline literals in a state machine each map
layer kept its own byte-identical copy of; they are one copy now (#338), so
changing the feel of the map gesture means editing `longPressTouch.ts` and
nothing else. The one remaining duplicate is `GlobalViewMap`'s per-marker
listeners: same 500 ms / 10 px hold, but no fired flag and no swallowed
`touchend`, so a marker long-press still leaves the trailing tap to run. It was
left alone rather than folded in, because folding it in would have changed that
behavior.

`useLongPress` returns `{ onPointerDown, onPointerMove, onPointerUp,
onPointerCancel }` to spread onto an element, and hands the callback the press
position plus the element the finger actually landed on: `e.target` walked up to
the nearest `HTMLElement` (`useLongPress.ts:102` - a press on a row's icon lands
on an `SVGPathElement`, and its parent `<svg>` is not HTML either), not the
element the handlers happen to be bound to. A callback that returns `false` says
it opened nothing and gets no swallow; returning nothing - what `SectionsPanel`
does - keeps it. On fire the hook sets a flag, and the `onPointerUp` that sees
that flag arms a one-shot suppressor (`useLongPress.ts:51`): capture-phase
`document` listeners for `mousedown`, `mouseup`, `click` and `contextmenu`,
which stop propagation and, for the two cancelable ones, prevent the default as
well - stopping a compat `contextmenu` without preventing it would leave the
browser's own native menu to open over the one the press just gave you. Capture
on `document` is the one position ahead of both React's root delegation and the
bubble-phase `mousedown` dismissable layers close themselves on
(`useDismissableLayer.ts:136`), so the menu the hold just opened survives the
tap that ends the hold.

The window closes at the first of three things: the compat `click`, a
`pointerdown`, or 700 ms. The `pointerdown` matters on platforms that suppress
the compat events themselves - nothing arrives to close the window early there,
and the user's genuine tap on the menu is well inside 700 ms. A real gesture
always begins with a `pointerdown` and a compat sequence synthesized from a lift
never does, so that signal cannot fire ahead of the events the window exists to
eat. The suppressor is also torn down when the consumer unmounts
(`useLongPress.ts:204`), since the menu a press opens can take the row
underneath it away.

Two details there are load-bearing. The suppressor is armed at the LIFT, not
when the timer fires: the browser synthesizes those events from the lift, so a
hold longer than the window would outlive a fire-time arming and leak its tap.
And the suppression point the Pointer Events spec blesses - `preventDefault()`
on `pointerdown` - is deliberately not used, because it would kill the
compatibility `mousedown` of every ordinary tap, and `SectionsPanel`'s
tap-to-select rides on exactly that (`SectionsPanel.tsx:766`). The
`preventDefault()` on `pointerup` stays as belt-and-braces for engines that
honour it. `src/hooks/__tests__/useLongPress.test.tsx` pins all of this in jsdom.

That asymmetry is the whole reason the two mechanisms differ. The Touch Events
spec makes `preventDefault()` on `touchend` the sanctioned way to suppress the
compatibility mouse events, so the raw-touch path simply does that and is done;
the Pointer Events spec's equivalent point is `pointerdown`, which this hook
cannot use, so the pointer path needs the document-capture window instead.

`attachLongPressTouch(target, { onLongPress, isEnabled? })` returns a detach
function (both map layers call it from `destroy()`; the grid's effect returns it
as its cleanup). Its `onLongPress` receives a snapshot of how the press began -
the client coords copied out of the starting
`Touch`, plus the `e.target` the `touchstart` carried
(`longPressTouch.ts:29-39`) - and returns whether a menu actually opened. The
target is snapshotted rather than looked up at fire time, because "what did the
finger come down on" is the question a menu is answering, and the 10 px
wobble-cancel bounds how far the finger can have drifted from it since. Both
layers synthesize a `MouseEvent('contextmenu')` at that position and feed it to
the same handler the real right-click uses (`SurveyLayer.handleContextMenu`,
`GlobalSurveyLayer.callbacks.onSystemContextMenu`), so the resulting menu is
identical. `SurveyLayer` always returns `true`; `GlobalSurveyLayer` returns
`false` when `pickSystemAt` finds nothing under the finger, since no menu opened.

That return value is what arms the swallowed lift, and the `touchend`
listener's `{ passive: false }` registration is what makes it possible: the
`touchend` ending a press that opened a menu calls `preventDefault()`, which
suppresses the compatibility mouse events the browser would otherwise
synthesize - so lifting the finger cannot also run the active tool's click
underneath the open menu (#338). Deliberately no `stopPropagation`: MapLibre's
own `touchend` still has to run or pan and pinch break. `touchcancel` clears the
flag without preventing anything.

The grid is the third consumer shape. `useTouchContextMenu(ref)`
(`src/hooks/useTouchContextMenu.ts`) attaches one `attachLongPressTouch` to the
scroll container (`SpreadsheetGrid.tsx:1911`) and, on fire, dispatches a
synthetic bubbling `MouseEvent('contextmenu')` on the snapshotted target. The
React `onContextMenu` handlers already on the cells, the gutter and the header
do the rest - target resolution, moving the selection, focus - so the touch menu
IS the right-click menu rather than a second implementation of it. Whether
`dispatchEvent` returns false is the precise "a menu opened" signal the swallow
wants: every handler that opens one prevents the default, and a handler with
nothing to show does not. Two presses stay inert: a target outside the
container, and one inside `input`, `textarea`, `select` or
`[contenteditable="true"]` (`useTouchContextMenu.ts:40`) - the cell editor is
the grid's one text-entry island, and the native caret/paste gesture wins there.

`isEnabled` is the gate, consulted when the finger lands and again before the
menu opens (the modal state it reads can change during the hold). Only
`SurveyLayer` passes one that ever says no - `modalInteractionActive`, true while placement mode
is armed (uiStore) or a move ghost is in flight (`dragPreviewActive`, between
`beginStationDragPreview`/`beginDragPreview` and `endDragPreview`). Those two
interactions commit through MapCanvas's POINTER handlers - placement on
`pointerdown`, a move drop on `pointerup` - which are not compatibility mouse
events, so the swallowed `touchend` cannot stop them; ungated, one hold would
both mutate the survey and open a menu over the result. Note pointer events
precede touch events, so a placement confirmed by that very touch has already
cleared the store before the press is armed; `MapCanvas` therefore stamps the
confirm via `notePlacementConfirmed` and the gate honours a short
`PLACEMENT_CONFIRM_GRACE_MS` window, while the store read covers holds that
begin while placement is still armed. `GlobalSurveyLayer` has
neither mode and passes no gate; `useTouchContextMenu` always passes one, but it
reads an optional `enabled` flag that defaults to true and the grid never sets.

## Map and renderer

**Gesture ownership.** One pure predicate decides it: `mapGestureMode`
(`src/utils/mapGestures.ts`) takes the active tool and whether placement mode is
on, and returns `{ dragPan, touchAction }`. The default answer is
`dragPan: true` / `touchAction: 'manipulation'`, so MapLibre's own one-finger pan
and pinch-zoom run under **every** tool (#409). Two modes take the gesture back,
because in each the drag itself is the interaction: the zoom tool, whose drag
draws the zoom box, and placement mode, which confirms on `pointerdown`. Those
two get `touchAction: 'none'` and `dragPan.disable()`.

Carrying a move ghost is **not** one of them, and is not an input to the
predicate at all: one finger still pans and a tap drops, because the drop is
click-gated in `handlePointerUp` (a release within 3px of its press) rather than
owning every release. A pinch moves both fingers far past that threshold, so it
zooms without dropping the ghost. See [move-tools.md](move-tools.md).

MapCanvas makes a single `mapGestureMode` call and every consumer - the
`dragPan` effect, the custom modifier-pan claim, and the container's
`touchAction` - reads that one result, so they cannot disagree. This replaced
three hand-kept copies of one tool list plus
a `pointerType === 'touch' && activeTool === 'pan'` early return in
`handlePointerDown`, which is why **adding a `ToolType` no longer means updating
any list.**

**MapLibre handlers.** `dragRotate`, `touchPitch` and `boxZoom` are disabled at
map construction (`MapCanvas.tsx:1789-1796`), and so is the rotation half of the
pinch: `touchZoomRotate.disableRotation()` (`MapCanvas.tsx:1794`) leaves a
two-finger twist zooming only. The other two MapLibre surfaces get the same
treatment minus the box zoom - the global view (`GlobalViewMap.tsx:376-381`, the
`disableRotation()` at `:381`) and the section preview dialog
(`SectionPreview.tsx:281-286`, at `:286`). Both were missing that line until
#338, so a twist on either used to leave the plan sitting at an angle. The survey is a top-down plan,
so a bearing means nothing here, and a non-zero one would drop the `axisAligned`
fast path in the hit-test/geometry index (`cameraSnapshot` in `surveyLayer/surveyHitTester.ts`)
with no way to reset it from the touch UI.

**Custom WebGL renderer.** Touch needs nothing renderer-specific. The survey is
drawn by the custom WebGL layer, and the stock bulk layers were deleted rather
than hidden, so hit-testing does not use `queryRenderedFeatures`; it goes through the SoA
geometry index (`SurveyLayer.hitCandidateSections`, `resolveStationAt` /
`resolveShotAt`, documented in `surveyLayer/surveyHitTester.ts`). Long-press hit-testing
calls the same `handleContextMenu` as right-click, so it inherits that path for
free. `GlobalSurveyLayer` likewise resolves via its own `pickSystemAt` index.

**Placement mode** is the one map interaction explicitly written for touch:
`handlePlacementClick` recomputes the cursor position from the event because
`pointerdown` precedes any `pointermove` on touch (`usePlacement.ts`), and
`PlacementOverlay` shows a Cancel button when `navigator.maxTouchPoints > 0`
because right-click-to-cancel is unavailable.

## Toolbar

`ToolButton` and `ToolGroupButton` record `e.pointerType` into
`lastPointerTypeRef` on `onPointerDown` (`Toolbar.tsx:700`, `:930`) and branch on
it:

- A tap on a tool group toggles its flyout open instead of cycling through the
  group's tools (`Toolbar.tsx:783`), so touch users can see and pick a subtool.
- `handleMouseEnter` returns early after a touch (`Toolbar.tsx:652`, `:812`), so
  the hover-open flyout and the informational flyout do not appear.

## Form input

Numeric fields carry `inputMode="decimal"` so tablets raise a numeric soft
keyboard. Applied across `EditShotDialog`, `InsertStationDialog`,
`DrawShotDialog`, `SectionMetadataForm` and the other shot/geometry entry
dialogs (the `SpreadsheetGrid`-based shot editor sets no `inputMode`); grep
`inputMode="decimal"` for the full set. `HelpTooltip` toggles on `onClick`
(`HelpTooltip.tsx:211`) rather than hover-only, so its text is reachable by tap.

## Gaps and known-broken cases

- **Test coverage reaches two surfaces.** Both Playwright specs opt into
  `test.use({ hasTouch: true })` and dispatch gestures through CDP
  `Input.dispatchTouchEvent` (Playwright's own API expresses neither a touch drag
  nor a pinch): `e2e-playwright/map-touch-gestures.spec.ts` covers one-finger
  pan, the long-press menu and its swallowed lift, and a two-finger twist that
  zooms without rotating; `e2e-playwright/grid-touch-context-menu.spec.ts`
  covers the grid menu surviving the lift that opened it, a tap on a menu entry,
  and a moving finger that scrolls instead of firing. Below them, three jsdom
  suites pin the same machines: `src/utils/__tests__/longPressTouch.test.ts`
  (hold, sub-threshold wobble, cancel, multi-touch, swallowed lift, the gate,
  the snapshotted target), `src/hooks/__tests__/useLongPress.test.tsx` (the
  suppressor, including a hold far longer than its window) and
  `src/components/common/SpreadsheetGrid/__tests__/spreadsheetGridLongPress.test.tsx`.
  Still missing: a mobile device profile in the Playwright projects
  (`playwright.config.ts:112` runs Desktop Chrome only); the WDIO suite that
  might have carried one is retired (#549). Outside those two specs, every claim on this
  page about *behavior on a device* is untested; the claims about code are what
  was read from source.
- **Two surfaces outside the maps have a long press.** `SectionsPanel`'s section
  rows (`useLongPress`) and the shot grid's context menu (`useTouchContextMenu`).
  This page used to also document `ShotTableEditor`'s row long-press and the
  `e.currentTarget`-vs-`e.target` bug that stopped it firing (filed as #338);
  that component was rewritten onto `SpreadsheetGrid` and has neither - the
  grid's own menu is what replaced it. Everything else reachable by right-click
  on desktop is reachable on touch only through the two map canvases.
- **The data table's drags are mouse-only.** `SpreadsheetGrid` has a touch path
  to its context menu and nothing else. Range drag, fill-handle drag, column
  resize and the move-edge drags all listen for `mousemove`/`mouseup`, which a
  touch drag does not produce.
- **A long press on the fill handle or a move edge is a no-op.** Those are the
  two `pointer-events: auto` islands inside the otherwise transparent selection
  overlay (`SpreadsheetGrid.css:131`, `:199`), and neither carries an
  `onContextMenu`, so a hold on one dispatches a `contextmenu` nobody claims: no
  menu opens, and the tap is left alone (correctly - there is nothing for it to
  dismiss). The overlay's body is `pointer-events: none`, so a hold there falls
  through to the cell underneath and behaves normally.
- **A marker long-press on the global map still leaks its tap.**
  `GlobalViewMap`'s per-marker listeners are the last copy of the hold state
  machine, with no fired flag and no swallowed `touchend`. Left alone
  deliberately: folding them into the shared helper would change that behavior.
- **Move tools are untested on touch and structurally awkward.** A move drag is
  started by MapLibre's `click` (the `move_station` / `move_shot` /
  `move_segment` cases in `useMapTools`, via `onMoveDragStart`), previews on
  `pointermove` over the container, and commits on `pointerup`
  (`MapCanvas.handlePointerUp` -> `handleMoveDragEnd`). With a mouse the pointer
  keeps moving between grab and drop; with a finger it does not, so the gesture
  becomes tap-to-grab followed by a separate touch-drag-release - which is
  exactly why the long-press gate above exists: the second touch of that
  sequence lands with the ghost already in flight. Nobody has confirmed the
  gesture itself on hardware.
- **Popover dismissal depends on compatibility mouse events.**
  `useDismissableLayer` listens on `mousedown` only, so an outside tap dismisses
  a flyout or menu only if the browser synthesizes the compatibility event.
- **Enlarged touch targets stop at the resize handles.** The only
  `@media (pointer: coarse)` rule in the stylesheet widens the two resize-handle
  classes. Buttons, toolbar items, table cells and menu rows keep their desktop
  sizes.
