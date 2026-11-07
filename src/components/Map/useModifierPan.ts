import { useEffect, type Dispatch, type MutableRefObject, type RefObject, type SetStateAction } from 'react';
import type * as maplibregl from 'maplibre-gl';
import type { ProjectStore } from '../../store/projectStore';
import type { UIStoreState } from '../../store/uiSelectKeys';
import { attachDragReleaseForwarder } from '../../utils/mapDragRelease';
import { mapCursorFor } from './mapCursor';
import type { ZoomBoxState } from './useZoomBox';

/**
 * How far a WATCHED Ctrl+left press has to travel before the custom pan takes
 * it over. Equal to MapLibre's default `clickTolerance` (map init does not
 * override it), which is the distance at which MapLibre itself stops calling a
 * press-and-release a click: below it the press is left alone so Ctrl+click can
 * toggle a feature's selection, above it MapLibre suppresses its own click and
 * the gesture is unambiguously a drag.
 */
export const CUSTOM_PAN_CLAIM_PX = 3;

interface ModifierPanOptions {
  map: MutableRefObject<maplibregl.Map | null>;
  mapContainer: RefObject<HTMLDivElement>;
  config: ProjectStore['config'];
  activeTool: UIStoreState['activeTool'];
  /** MapCanvas's one `mapGestureMode` decision: whether MapLibre's dragPan owns a plain left-drag. */
  mapLibreDragPan: boolean;
  // The pan's flags. Owned by MapCanvas because its map init hands them to the
  // pan/zoom debug instrumentation (installPanZoomDebug).
  customPanActiveRef: MutableRefObject<boolean>;
  ctrlPanActiveRef: MutableRefObject<boolean>;
  ctrlPanMouseEventTimeRef: MutableRefObject<number>;
  placementJustConfirmedRef: MutableRefObject<boolean>;
  /** MapCanvas's `ctrlHeld` state, which it pushes into the cursor owner with the tool and placement. */
  setCtrlHeld: Dispatch<SetStateAction<boolean>>;
  setZoomBox: Dispatch<SetStateAction<ZoomBoxState>>;
}

/**
 * The Ctrl+left / middle custom pan (docs/move-tools.md, "Ctrl+Drag Panning"):
 * the Ctrl-held tracking (keydown / keyup / blur, plus the pointer-side recovery
 * of a swallowed release) and the capture-phase listeners that watch, claim and
 * run the pan. The pan pushes `panning` (and the release's `ctrlKey`) into the
 * cursor owner itself; `ctrlHeld` goes there through MapCanvas's cursor effect.
 *
 * Moved out of MapCanvas.tsx verbatim (#576 D3). The two effects used to have
 * the zoom box's Escape effect and the move-drag Escape effect between them;
 * those registered only window keydown listeners, which act on other keys and
 * neither stop propagation nor read `defaultPrevented`, so running the custom-pan
 * registration right after the Ctrl tracking changes nothing.
 */
export function useModifierPan({
  map,
  mapContainer,
  config,
  activeTool,
  mapLibreDragPan,
  customPanActiveRef,
  ctrlPanActiveRef,
  ctrlPanMouseEventTimeRef,
  placementJustConfirmedRef,
  setCtrlHeld,
  setZoomBox,
}: ModifierPanOptions): void {
  // Track Ctrl key for cursor updates and zoom-box cancellation
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Recover from a genuine Ctrl release that the keyup workaround below
      // swallowed. That suppression cannot tell a spurious keyup from a real one at
      // the moment it fires - but the two diverge immediately afterwards: with the
      // key still physically down every later event reports `ctrlKey: true`, so an
      // event reporting `false` is proof the key is up. Without this, one release
      // timed inside the 50ms window latched `ctrlHeld` on until the user pressed
      // and released Ctrl again - leaving the pan grab over every tool they
      // switched to, since no further keyup was ever coming.
      if (!e.ctrlKey) setCtrlHeld(false);
      if (e.key === 'Control' && !e.repeat) {
        setCtrlHeld(true);

        // Cancel any active zoom box when Ctrl is pressed (mid-drag Ctrl converts to pan)
        setZoomBox({ start: null, end: null });
      }
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      // Same recovery as on keydown: releasing any OTHER key also proves whether
      // Ctrl is still down.
      if (e.key !== 'Control' && !e.ctrlKey) setCtrlHeld(false);
      if (e.key === 'Control') {
        // WORKAROUND: Tauri/WebKit fires spurious Ctrl keyup events at the moment of
        // mousedown/mouseup while holding Ctrl. Only suppress keyups that occur within
        // 50ms of a mouse event during a Ctrl+drag - genuine Ctrl releases go through
        // immediately so the cursor resets without delay.
        if (ctrlPanActiveRef.current && Date.now() - ctrlPanMouseEventTimeRef.current < 50) {
          return;
        }
        setCtrlHeld(false);
      }
    };
    // Also handle window blur (Ctrl held, switch windows, release outside)
    const handleBlur = () => {
      customPanActiveRef.current = false;
      ctrlPanActiveRef.current = false;
      setCtrlHeld(false);
      if (map.current) mapCursorFor(map.current).set({ panning: false });
      // Cancel any active zoom box
      setZoomBox({ start: null, end: null });
    };

    /**
     * The same correction from the POINTER, which is the half the keyboard cannot
     * cover: after a Ctrl+drag whose release landed inside the suppression window,
     * the next thing a user does is usually click a toolbar tool or just move the
     * mouse - neither of which is a key event, so `ctrlHeld` stayed latched and the
     * pan grab followed them onto the tool they had just chosen.
     *
     * Deliberately NOT skipped while a Ctrl+pan is running. Exempting that window
     * looked safer and was not: the release is swallowed precisely at the end of a
     * drag, so a click landing inside the same 50ms - reaching straight for the
     * toolbar, which is the whole scenario - was exempted too, and the flag stayed
     * latched until some later stray mouse move happened to clear it. The quirk
     * being worked around is spurious KEYUPS, not mis-reported mouse modifiers; the
     * Ctrl+pan mouseup handler already trusts `e.ctrlKey` the same way. And the flag
     * only drives the cursor - the pan itself runs off `ctrlPanActiveRef` - so even
     * on a platform that did lie here the cost is a cursor, not a broken gesture.
     *
     * Re-setting `false` when it is already `false` is free: React bails out of the
     * render when the state is unchanged, which is the common case on every move.
     */
    const clearIfReleased = (e: MouseEvent | PointerEvent) => {
      if (!e.ctrlKey) setCtrlHeld(false);
    };

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
    window.addEventListener('blur', handleBlur);
    window.addEventListener('pointerdown', clearIfReleased);
    window.addEventListener('mousemove', clearIfReleased);

    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
      window.removeEventListener('blur', handleBlur);
      window.removeEventListener('pointerdown', clearIfReleased);
      window.removeEventListener('mousemove', clearIfReleased);
    };
  }, []);

  // Custom temporary panning implementation for Ctrl+left-drag and middle-drag.
  // MapLibre cannot do either itself: its mouse-pan handler ignores any press
  // that is not a bare left button - maplibre-gl 5.16 `generateMousePanHandler`
  // uses `checkCorrectEvent: (e) => mouseButton(e) === LEFT_BUTTON && !e.ctrlKey`
  // - and with `dragRotate`/`boxZoom` disabled (map init) nothing else claims a
  // Ctrl+left drag either. So we track the movement and call panBy() ourselves,
  // which is also what makes `pan_speed` a thing the modifier gestures can honour.
  const rawPanSpeed = config?.map.controls.pan_speed ?? 1.0;
  const panSpeed = Number.isFinite(rawPanSpeed) ? Math.min(5.0, Math.max(0.1, rawPanSpeed)) : 1.0;
  useEffect(() => {
    const mapElement = mapContainer.current;
    if (!mapElement) return;

    // Track drag start state for custom panning.
    let customPanStart: { x: number; y: number } | null = null;
    let customPanButtonMask: number | null = null;
    // The press point, and whether the gesture has actually been TAKEN OVER.
    // The two differ only for a Ctrl+left press in the modes where a plain drag
    // already pans: there the press is merely WATCHED until it travels far
    // enough to be a drag. Claiming it at mousedown is what made the documented
    // Ctrl+click selection toggle unreachable - the press was swallowed and so
    // was the click behind it, so `handleStationClick`'s `ctrlKey` branch
    // (useMapTools) never ran under any tool.
    let customPanOrigin: { x: number; y: number } | null = null;
    let customPanClaimed = false;
    // The map whose cursor the claim set to `panning`, for the cleanup below.
    let claimedMap: NonNullable<typeof map.current> | null = null;

    /** Take the gesture over: suppress it, flag it, and show the pan cursor. */
    const claimCustomPan = (e: MouseEvent, isCtrlLeftPan: boolean) => {
      if (!map.current) return;
      customPanClaimed = true;
      claimedMap = map.current;
      customPanActiveRef.current = true;
      ctrlPanActiveRef.current = isCtrlLeftPan;
      if (isCtrlLeftPan) {
        ctrlPanMouseEventTimeRef.current = Date.now();
      }

      // Suppress default browser behavior during custom pan
      e.stopPropagation();
      e.preventDefault();

      // Show the panning cursor until the gesture ends (restoreCursorAfterPan).
      mapCursorFor(map.current).set({ panning: true });
    };

    const handleMouseDownCapture = (e: MouseEvent) => {
      const isCtrlLeftPan = e.ctrlKey && e.button === 0;
      const isMiddlePan = e.button === 1;
      // Every tool but pan: the pan tool owns plain left-drag itself, so the
      // modifier gesture would be redundant there.
      if ((isCtrlLeftPan || isMiddlePan) && activeTool !== 'pan' && map.current) {
        customPanStart = { x: e.clientX, y: e.clientY };
        customPanOrigin = { x: e.clientX, y: e.clientY };
        customPanButtonMask = isMiddlePan ? 4 : 1;
        customPanClaimed = false;

        // Claim the press outright where nothing else can want it: a middle
        // press has no other meaning, and where MapLibre's dragPan is OFF the
        // mode owns the press itself (placement confirms on pointerdown, the
        // zoom tool starts its box) - neither of which may happen because Ctrl
        // was held. Where a plain drag already pans, a Ctrl+left press is only
        // watched: see `customPanClaimed`. Carrying a move ghost is in that
        // second group since #409 - the drop is click-gated, so a Ctrl press
        // that travels becomes a pan on the same 3px threshold as everywhere
        // else, and one that does not travel drops the ghost.
        if (isMiddlePan || !mapLibreDragPan) {
          claimCustomPan(e, isCtrlLeftPan);
        }
      }
    };

    // Implement custom panning since modified/middle drag interactions are handled manually.
    const handleMouseMoveCapture = (e: MouseEvent) => {
      if (customPanStart && customPanButtonMask !== null && (e.buttons & customPanButtonMask) !== 0 && map.current) {
        if (!customPanClaimed) {
          // A watched Ctrl+left press becomes a pan the moment it stops being a
          // click. The threshold is MapLibre's own `clickTolerance` (default 3px,
          // map init does not override it), so the handover lands exactly where
          // MapLibre itself stops calling the gesture a click: below it the press
          // is left alone and the click goes through to the selection toggle,
          // above it MapLibre would suppress its own click anyway.
          if (!customPanOrigin) return;
          const travel = Math.hypot(e.clientX - customPanOrigin.x, e.clientY - customPanOrigin.y);
          if (travel < CUSTOM_PAN_CLAIM_PX) return;
          // Pans from the PRESS point, not from here: the map follows the whole
          // travel, so the takeover is invisible rather than a 3px jump.
          claimCustomPan(e, true);
        }
        const dx = e.clientX - customPanStart.x;
        const dy = e.clientY - customPanStart.y;

        // Pan the map by the delta (negative because we're dragging the "ground").
        // pan_speed scales this modifier-drag pan only: the pan TOOL's dragPan is
        // direct manipulation (map glued to cursor), so a multiplier there would
        // read as broken tracking, but Ctrl/middle drag is already a shortcut
        // gesture where sensitivity is a preference.
        if (Math.abs(dx) > 1 || Math.abs(dy) > 1) {
          map.current.panBy([-dx * panSpeed, -dy * panSpeed], { animate: false });
          customPanStart = { x: e.clientX, y: e.clientY };
        }
        if (ctrlPanActiveRef.current) {
          ctrlPanMouseEventTimeRef.current = Date.now();
        }
        // Stop propagation to prevent MapLibre from updating hover state during pan
        e.stopPropagation();
        e.preventDefault();
      }
    };

    /**
     * End the pan cursor after a claimed pan. The release's own `ctrlKey` is the
     * freshest word on the modifier (a Ctrl keyup near the mouseup can be a
     * spurious one the keyup filter swallowed), so it updates the owner's Ctrl
     * input too: still held shows the grab for the next pan, released falls back
     * to whatever the tool, placement and hover say.
     */
    const restoreCursorAfterPan = (e: MouseEvent) => {
      if (!map.current) return;
      mapCursorFor(map.current).set({ panning: false, ctrlHeld: e.ctrlKey });
    };

    const handleMouseUpCapture = (e: MouseEvent) => {
      if (customPanStart && map.current) {
        const wasClaimed = customPanClaimed;
        customPanStart = null;
        customPanButtonMask = null;
        customPanOrigin = null;
        customPanClaimed = false;
        // A watched press that never travelled is a Ctrl+CLICK, not a pan. None
        // of the suppression below may touch it - and `customPanActiveRef`, which
        // the click capture reads, was never set, so the click reaches MapLibre.
        if (!wasClaimed) return;
        // Record timestamp for spurious keyup filter (Tauri/WebKit can fire one near mouseup too)
        if (ctrlPanActiveRef.current) {
          ctrlPanMouseEventTimeRef.current = Date.now();
        }
        // Suppress default browser behavior; the Ctrl+click guard in
        // SurveyLayer.handleMapClick prevents move/select actions
        e.stopPropagation();
        e.preventDefault();
        // Clear custom pan flags after a small delay (can't rely on handleMouseUpGlobal
        // because stopPropagation prevents it from firing on window bubble)
        setTimeout(() => {
          customPanActiveRef.current = false;
          ctrlPanActiveRef.current = false;
        }, 50);
        restoreCursorAfterPan(e);
      }
    };

    // Capture the DOM click event during/after a custom pan gesture.
    // The browser fires 'click' as a separate event after mouseup - stopping propagation
    // on mousedown/mouseup does NOT prevent click from being dispatched. MapLibre listens
    // for 'click' on the canvas, so we must capture it to prevent spurious map clicks.
    const handleClickCapture = (e: MouseEvent) => {
      if (customPanActiveRef.current || placementJustConfirmedRef.current) {
        e.stopPropagation();
        e.preventDefault();
      }
    };

    // Capture pointer events during custom pan - MapLibre may use pointer events
    // internally, so capturing only mouse events is insufficient to prevent click leakage
    const handlePointerDownCapture = (e: PointerEvent) => {
      const isCtrlLeftPan = e.ctrlKey && e.button === 0;
      const isMiddlePan = e.button === 1;
      // Mirrors the outright claim in handleMouseDownCapture. A merely WATCHED
      // Ctrl press must reach the app's own pointerdown handling untouched - in
      // those modes it does nothing with it anyway (placement and the zoom box
      // are both modal), and stopping it here would be a second way to swallow
      // the gesture behind the mousedown capture's back.
      if ((isMiddlePan || (isCtrlLeftPan && !mapLibreDragPan)) && activeTool !== 'pan' && map.current && e.pointerType === 'mouse') {
        e.stopPropagation();
      }
    };

    const handlePointerUpCapture = (e: PointerEvent) => {
      // If we're in custom pan, stop pointer events from reaching MapLibre.
      // (customPanStart is still set here - cleared by handleMouseUpCapture which fires after)
      if (customPanStart && customPanClaimed && e.pointerType === 'mouse') {
        e.stopPropagation();
      }
    };

    // Track if mouse is currently down (for filtering spurious keyup)
    let mouseIsDown = false;

    const handleMouseDownGlobal = () => {
      mouseIsDown = true;
    };

    const handleMouseUpGlobal = (e: MouseEvent) => {
      mouseIsDown = false;
      // Clear pan state in case mouse was released outside the map container
      // (handleMouseUpCapture only fires for events on the container element)
      const wasClaimed = customPanClaimed;
      customPanStart = null;
      customPanButtonMask = null;
      customPanOrigin = null;
      customPanClaimed = false;
      if (ctrlPanActiveRef.current) {
        ctrlPanMouseEventTimeRef.current = Date.now();
      }
      // The pan now runs past the container edge, so releasing outside it is an
      // ordinary way to end one, and this is the only handler that sees it -
      // the grabbing cursor has to be undone here too or it sticks.
      if (wasClaimed) {
        restoreCursorAfterPan(e);
      }
      // Fallback: clear custom pan flags for mouseups outside the map container
      // (handleMouseUpCapture handles the in-container case)
      if (customPanActiveRef.current) {
        setTimeout(() => {
          if (!mouseIsDown) {
            customPanActiveRef.current = false;
            ctrlPanActiveRef.current = false;
          }
        }, 50);
      }
    };

    // Use capture: true to intercept BEFORE MapLibre's handlers
    mapElement.addEventListener('mousedown', handleMouseDownCapture, { capture: true });
    // The moves go on WINDOW, not the map element: a drag that leaves the
    // element still has to steer the camera, and an element listener simply
    // stops receiving moves once the cursor is outside it - a 300px gesture
    // delivered the 135px to the edge and then froze (#429). The handler
    // no-ops unless `customPanStart` is set, which only a mousedown on the map
    // element does, so widening the listener does not widen what it acts on;
    // and because it suppresses at window-capture, MapLibre still never sees
    // the moves it would have taken from the element listener.
    window.addEventListener('mousemove', handleMouseMoveCapture, { capture: true });
    mapElement.addEventListener('mouseup', handleMouseUpCapture, { capture: true });
    mapElement.addEventListener('click', handleClickCapture, { capture: true });
    mapElement.addEventListener('pointerdown', handlePointerDownCapture, { capture: true });
    mapElement.addEventListener('pointerup', handlePointerUpCapture, { capture: true });
    window.addEventListener('mousedown', handleMouseDownGlobal, { capture: true });
    window.addEventListener('mouseup', handleMouseUpGlobal);
    // A plain left-drag - the default pan gesture since #409 - released over the
    // floating chrome (dock reveal handles, the Data tab, MapLibre's own
    // navigation/scale controls) would otherwise leave the map stuck "moving";
    // src/utils/mapDragRelease.ts has why MapLibre needs the help.
    const detachDragReleaseForwarder = attachDragReleaseForwarder(() => map.current);

    return () => {
      mapElement.removeEventListener('mousedown', handleMouseDownCapture, { capture: true });
      window.removeEventListener('mousemove', handleMouseMoveCapture, { capture: true });
      mapElement.removeEventListener('mouseup', handleMouseUpCapture, { capture: true });
      mapElement.removeEventListener('click', handleClickCapture, { capture: true });
      mapElement.removeEventListener('pointerdown', handlePointerDownCapture, { capture: true });
      mapElement.removeEventListener('pointerup', handlePointerUpCapture, { capture: true });
      window.removeEventListener('mousedown', handleMouseDownGlobal, { capture: true });
      window.removeEventListener('mouseup', handleMouseUpGlobal);
      detachDragReleaseForwarder();
      // A pan this registration claimed is abandoned here (a tool hotkey during
      // a middle-drag, Escape ending placement during a Ctrl+drag): the next
      // registration starts with no claim, so the release that ends the gesture
      // would never undo `panning`, the cursor owner's highest-precedence input,
      // and the canvas would stay `grabbing` (#576 RV4-FE-MH1).
      if (customPanClaimed && claimedMap) {
        mapCursorFor(claimedMap).set({ panning: false });
      }
    };
    // `mapLibreDragPan` decides whether a Ctrl+left press is claimed outright or
    // only watched, so the handlers have to be re-registered when it flips. That
    // re-registers the drag-release forwarder with them; it is attach/detach
    // symmetric and holds no in-flight state of its own, so re-arming it between
    // gestures is inert.
  }, [activeTool, mapLibreDragPan, panSpeed]);
}
