/**
 * #576 RV4-FE-MH1: a claimed modifier pan whose listeners are re-registered
 * mid-gesture must not leave the cursor stuck on `grabbing`.
 *
 * The custom-pan effect re-registers when the tool or MapLibre's dragPan flips
 * (a tool hotkey during a middle-drag, Escape cancelling placement during a
 * Ctrl+drag). The new registration starts with no claimed pan, so the release
 * that ends the gesture is not "its" pan and never clears the cursor owner's
 * sticky `panning` input: the canvas showed `grabbing` over every tool, hover
 * and placement until the next completed modifier pan or a window blur.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import type * as maplibregl from 'maplibre-gl';
import { useModifierPan } from '../useModifierPan';
import { mapCursorFor } from '../mapCursor';
import type { ToolType } from '../../../store/uiStore';

function setup() {
  const container = document.createElement('div');
  const canvas = document.createElement('canvas');
  container.appendChild(canvas);
  document.body.appendChild(container);
  const fakeMap = { getCanvas: () => canvas, panBy: vi.fn() } as unknown as maplibregl.Map;
  return { container, canvas, fakeMap, map: { current: fakeMap }, mapContainer: { current: container } };
}

function renderPan(
  map: { current: maplibregl.Map },
  mapContainer: { current: HTMLDivElement },
  initialProps: { tool: ToolType; dragPan: boolean },
) {
  const refs = {
    customPanActiveRef: { current: false },
    ctrlPanActiveRef: { current: false },
    ctrlPanMouseEventTimeRef: { current: 0 },
    placementJustConfirmedRef: { current: false },
  };
  return renderHook(
    ({ tool, dragPan }: { tool: ToolType; dragPan: boolean }) =>
      useModifierPan({
        map,
        mapContainer,
        config: null,
        activeTool: tool,
        mapLibreDragPan: dragPan,
        ...refs,
        setCtrlHeld: vi.fn(),
        setZoomBox: vi.fn(),
      }),
    { initialProps },
  );
}

describe('modifier pan cursor across an effect re-registration (#576 RV4-FE-MH1)', () => {
  it('a tool hotkey during a middle-drag pan: the release shows the new tool cursor', () => {
    const { container, canvas, fakeMap, map, mapContainer } = setup();
    const { rerender } = renderPan(map, mapContainer, { tool: 'select', dragPan: true });
    mapCursorFor(fakeMap).set({ tool: 'select' });

    container.dispatchEvent(new MouseEvent('mousedown', { button: 1, buttons: 4, bubbles: true }));
    expect(canvas.style.cursor).toBe('grabbing');

    // The hotkey re-runs the pan effect (deps [activeTool, ...]); MapCanvas's
    // cursor effect pushes the new tool.
    rerender({ tool: 'measure', dragPan: true });
    mapCursorFor(fakeMap).set({ tool: 'measure' });

    container.dispatchEvent(new MouseEvent('mouseup', { button: 1, bubbles: true }));
    window.dispatchEvent(new MouseEvent('mouseup', { button: 1 }));
    mapCursorFor(fakeMap).set({ hoverPointer: true });
    mapCursorFor(fakeMap).set({ hoverPointer: false });

    console.log('[MH1] tool-switch case:', canvas.style.cursor, 'panning =', mapCursorFor(fakeMap).state.panning);
    expect(mapCursorFor(fakeMap).state.panning).toBe(false);
    expect(canvas.style.cursor).toBe('crosshair');
  });

  it('Escape cancelling placement during a Ctrl+drag pan: the release shows the tool cursor', () => {
    const { container, canvas, fakeMap, map, mapContainer } = setup();
    // Placement on -> mapGestureMode gives dragPan false, so Ctrl+left is claimed at once.
    const { rerender } = renderPan(map, mapContainer, { tool: 'select', dragPan: false });
    mapCursorFor(fakeMap).set({ tool: 'select', placement: true, ctrlHeld: true });

    container.dispatchEvent(new MouseEvent('mousedown', { button: 0, buttons: 1, ctrlKey: true, bubbles: true }));
    expect(canvas.style.cursor).toBe('grabbing');

    rerender({ tool: 'select', dragPan: true });
    mapCursorFor(fakeMap).set({ placement: false });
    container.dispatchEvent(new MouseEvent('mouseup', { button: 0, ctrlKey: false, bubbles: true }));
    window.dispatchEvent(new MouseEvent('mouseup', { button: 0, ctrlKey: false }));
    mapCursorFor(fakeMap).set({ ctrlHeld: false });

    console.log('[MH1] placement-cancel case:', canvas.style.cursor, 'panning =', mapCursorFor(fakeMap).state.panning);
    expect(mapCursorFor(fakeMap).state.panning).toBe(false);
    expect(canvas.style.cursor).toBe('default');
  });

  it('a re-registration with no pan in flight leaves the cursor alone', () => {
    const { canvas, fakeMap, map, mapContainer } = setup();
    const { rerender } = renderPan(map, mapContainer, { tool: 'select', dragPan: true });
    mapCursorFor(fakeMap).set({ tool: 'select', hoverPointer: true });
    expect(canvas.style.cursor).toBe('pointer');

    rerender({ tool: 'measure', dragPan: true });

    expect(mapCursorFor(fakeMap).state.panning).toBe(false);
    expect(canvas.style.cursor).toBe('pointer');
  });
});
