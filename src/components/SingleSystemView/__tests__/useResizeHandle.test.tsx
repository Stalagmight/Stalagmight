import { describe, it, expect, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { type PointerEvent as ReactPointerEvent, type RefObject } from 'react';

import { useResizeHandle } from '../useResizeHandle';

// `.tsx` so this runs in the jsdom tier (vite.config environmentMatchGlobs) -
// renderHook needs a DOM.

type Rect = Pick<DOMRect, 'top' | 'bottom' | 'left' | 'right'>;

function measureRefWith(rect: Rect): RefObject<HTMLElement | null> {
  return {
    current: { getBoundingClientRect: () => rect as DOMRect },
  } as unknown as RefObject<HTMLElement | null>;
}

function pointerEvent(coords: { clientX?: number; clientY?: number } = {}): ReactPointerEvent {
  return {
    preventDefault: () => {},
    pointerId: 1,
    clientX: coords.clientX ?? 0,
    clientY: coords.clientY ?? 0,
    target: { setPointerCapture: () => {} },
  } as unknown as ReactPointerEvent;
}

/**
 * Regression for the u32 panel-size contract at the *live resize* entry point.
 * Panel sizes persist as a Rust u32; a fractional value (getBoundingClientRect
 * is sub-pixel under the webview UI zoom / HiDPI) fails serde and the persist is
 * silently dropped. useResizeHandle must therefore only ever emit whole pixels.
 * The uiStore.savePanelSizes test covers the store choke-point; this covers the
 * hook, so a dropped round here can't hide behind the store's re-rounding.
 */
describe('useResizeHandle - whole-pixel rounding', () => {
  it('rounds a sub-pixel vertical drag to a whole pixel for onResize and onResizeEnd', () => {
    const onResize = vi.fn();
    const onResizeEnd = vi.fn();
    // bottom 347.9, pointer at clientY 0 → raw size 347.9 → rounds to 348.
    const measureRef = measureRefWith({ top: 0, bottom: 347.9, left: 0, right: 0 });

    const { result } = renderHook(() =>
      useResizeHandle({ direction: 'vertical', minSize: 100, maxSize: 500, measureRef, onResize, onResizeEnd }),
    );

    act(() => result.current.handleProps.onPointerDown(pointerEvent()));
    act(() => result.current.handleProps.onPointerMove(pointerEvent({ clientY: 0 })));
    act(() => result.current.handleProps.onPointerUp(pointerEvent({ clientY: 0 })));

    expect(onResize).toHaveBeenCalledWith(348);
    expect(onResizeEnd).toHaveBeenCalledWith(348);
    for (const [size] of onResize.mock.calls) expect(Number.isInteger(size)).toBe(true);
    expect(Number.isInteger(onResizeEnd.mock.calls[0]?.[0])).toBe(true);
  });

  it('clamps an overshooting drag to a rounded maxSize', () => {
    const onResize = vi.fn();
    const onResizeEnd = vi.fn();
    // bottom 999.4, pointer at 0 → raw 999.4 → clamp to 500.
    const measureRef = measureRefWith({ top: 0, bottom: 999.4, left: 0, right: 0 });

    const { result } = renderHook(() =>
      useResizeHandle({ direction: 'vertical', minSize: 100, maxSize: 500, measureRef, onResize, onResizeEnd }),
    );

    act(() => result.current.handleProps.onPointerDown(pointerEvent()));
    act(() => result.current.handleProps.onPointerMove(pointerEvent({ clientY: 0 })));
    act(() => result.current.handleProps.onPointerUp(pointerEvent({ clientY: 0 })));

    expect(onResize).toHaveBeenCalledWith(500);
    expect(onResizeEnd).toHaveBeenCalledWith(500);
  });

  it('does not fire onResizeEnd on a press-release with no movement', () => {
    const onResize = vi.fn();
    const onResizeEnd = vi.fn();
    const measureRef = measureRefWith({ top: 0, bottom: 300, left: 0, right: 0 });

    const { result } = renderHook(() =>
      useResizeHandle({ direction: 'vertical', minSize: 100, maxSize: 500, measureRef, onResize, onResizeEnd }),
    );

    act(() => result.current.handleProps.onPointerDown(pointerEvent()));
    act(() => result.current.handleProps.onPointerUp(pointerEvent()));

    expect(onResize).not.toHaveBeenCalled();
    expect(onResizeEnd).not.toHaveBeenCalled();
  });
});

/**
 * If the handle element unmounts while a drag is active (view/project switch, a
 * line-focus collapse that hides the panel), no pointerup reaches the captured
 * node - the in-progress size must still be persisted exactly once via the
 * unmount cleanup, and a completed drag must NOT persist a second time on unmount.
 */
describe('useResizeHandle - unmount-mid-drag persistence', () => {
  it('persists the last live size on unmount when no pointerup occurred', () => {
    const onResize = vi.fn();
    const onResizeEnd = vi.fn();
    // bottom 347.9 → raw 347.9 → rounds to 348.
    const measureRef = measureRefWith({ top: 0, bottom: 347.9, left: 0, right: 0 });

    const { result, unmount } = renderHook(() =>
      useResizeHandle({ direction: 'vertical', minSize: 100, maxSize: 500, measureRef, onResize, onResizeEnd }),
    );

    act(() => result.current.handleProps.onPointerDown(pointerEvent()));
    act(() => result.current.handleProps.onPointerMove(pointerEvent({ clientY: 0 })));
    // No pointerup - the host unmounts mid-drag.
    expect(onResizeEnd).not.toHaveBeenCalled();

    act(() => unmount());

    expect(onResizeEnd).toHaveBeenCalledTimes(1);
    expect(onResizeEnd).toHaveBeenCalledWith(348);
  });

  it('does not double-persist when unmounting after a completed pointerup', () => {
    const onResize = vi.fn();
    const onResizeEnd = vi.fn();
    const measureRef = measureRefWith({ top: 0, bottom: 300, left: 0, right: 0 });

    const { result, unmount } = renderHook(() =>
      useResizeHandle({ direction: 'vertical', minSize: 100, maxSize: 500, measureRef, onResize, onResizeEnd }),
    );

    act(() => result.current.handleProps.onPointerDown(pointerEvent()));
    act(() => result.current.handleProps.onPointerMove(pointerEvent({ clientY: 0 })));
    act(() => result.current.handleProps.onPointerUp(pointerEvent({ clientY: 0 })));
    expect(onResizeEnd).toHaveBeenCalledTimes(1);

    act(() => unmount());

    // The completed pointerup cleared isResizingRef, so the unmount cleanup is a no-op.
    expect(onResizeEnd).toHaveBeenCalledTimes(1);
  });
});
