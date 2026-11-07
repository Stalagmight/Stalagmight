import { beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

import { useSidePanelSqueeze, resolveResize } from '../sidePanelSqueeze';
import { MAP_MIN_WIDTH, PANEL_SIZE_BOUNDS } from '../layoutConstants';
import { BAR } from '../Dock';
import { useUIStore } from '../../../store/uiStore';
import { invokeIpcMock } from '../../../test/factories/ipc';

// `.tsx` so this runs in the jsdom tier (vite.config environmentMatchGlobs).

vi.mock('../../../utils/ipc');

/**
 * The single most dangerous way to get #260 wrong: routing the viewport-driven
 * squeeze back through the resize path. A temporary 150px squeeze persisted to
 * config would leave the panel tiny FOREVER once the user zoomed back out - a
 * worse bug than the one being fixed, and an invisible one (it only shows on the
 * next launch). These tests pin that the squeeze is display-only: the store's
 * width fields and the `update_panel_sizes` IPC must both be untouched no matter
 * how narrow the container gets.
 */

/** A detached div reporting a fixed width (jsdom has no layout). */
function containerOfWidth(width: number): HTMLDivElement {
  const el = document.createElement('div');
  el.getBoundingClientRect = () => ({ width, height: 600, top: 0, left: 0, right: width, bottom: 600, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
  return el;
}

const USER_SECTIONS = 284;
const USER_DETAIL = 282;

function squeezeOpts() {
  return {
    mapMinWidth: MAP_MIN_WIDTH,
    barWidth: BAR,
    sections: {
      width: USER_SECTIONS,
      min: PANEL_SIZE_BOUNDS.sections.min,
      present: true,
      pinnedOpen: true,
    },
    detail: {
      width: USER_DETAIL,
      min: PANEL_SIZE_BOUNDS.detail.min,
      present: true,
      pinnedOpen: true,
    },
  };
}

describe('useSidePanelSqueeze - the squeeze never reaches the persisted store', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue(undefined);
    useUIStore.setState({ sectionsPanelWidth: USER_SECTIONS, detailPanelWidth: USER_DETAIL });
  });

  it('squeezes the rendered widths while leaving the store widths and the persist IPC alone', () => {
    // The measured 250%-zoom case: a squeeze AND a Details collapse.
    const { result } = renderHook(() =>
      useSidePanelSqueeze(containerOfWidth(614), squeezeOpts()),
    );

    expect(result.current.sections.width).toBeLessThan(USER_SECTIONS);
    expect(result.current.detail.collapsed).toBe(true);

    // The user's own widths are exactly as they left them...
    expect(useUIStore.getState().sectionsPanelWidth).toBe(USER_SECTIONS);
    expect(useUIStore.getState().detailPanelWidth).toBe(USER_DETAIL);
    // ...and nothing was written to config (savePanelSizes / savePanelLayout are
    // the only writers, and both go through invokeIpc).
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('writes nothing even when the container collapses to nearly zero', () => {
    const { rerender } = renderHook(
      ({ width }: { width: number }) => useSidePanelSqueeze(containerOfWidth(width), squeezeOpts()),
      { initialProps: { width: 1600 } },
    );

    for (const width of [900, 700, 614, 400, 120, 1]) rerender({ width });

    expect(useUIStore.getState().sectionsPanelWidth).toBe(USER_SECTIONS);
    expect(useUIStore.getState().detailPanelWidth).toBe(USER_DETAIL);
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('hands the user their widths back when the container grows again', () => {
    const { result, rerender } = renderHook(
      ({ width }: { width: number }) => useSidePanelSqueeze(containerOfWidth(width), squeezeOpts()),
      { initialProps: { width: 400 } },
    );

    expect(result.current.sections.collapsed).toBe(true);
    expect(result.current.detail.collapsed).toBe(true);

    rerender({ width: 1600 });

    expect(result.current).toEqual({
      sections: { width: USER_SECTIONS, collapsed: false },
      detail: { width: USER_DETAIL, collapsed: false },
    });
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });
});

/**
 * The same rule from the other direction, and the way it was actually broken:
 * the squeeze was kept out of the store, but a resize DRAG routed it back in.
 *
 * A Dock renders at the squeezed width, so `useResizeHandle` measures the
 * squeezed panel. Widening it produced sizes above the clamp that the squeeze
 * re-applied on the same frame - the edge never moved - and `onResize` /
 * `onResizeEnd` wrote them anyway. A panel squeezed from 400px to its 200px
 * minimum therefore looked inert to drag, and then came back at the dragged width
 * instead of 400 once the user returned to 100% zoom: a silent, unattributable
 * loss of their layout.
 *
 * The rule CLAMPS rather than refusing, and clamps to the user's own size rather
 * than to the rendering: refusing discarded whole drags (see the last case), and
 * clamping to the rendering would come back as 200 - the same loss from the other
 * side.
 */
describe('resolveResize - a squeezed drag cannot raise the persisted size', () => {
  const USER = 400;
  const SQUEEZED = 200;

  it('takes any request while the panel renders at the user\'s own size', () => {
    expect(resolveResize(USER, USER, 500)).toBe(500);
    expect(resolveResize(USER, USER, 250)).toBe(250);
  });

  it('keeps the user\'s own size for a request the squeezed rendering never reached', () => {
    // The whole widening direction: the first pixel of drag already exceeds the
    // clamp, so every one of these is a width the edge did not move to.
    expect(resolveResize(SQUEEZED, USER, SQUEEZED + 1)).toBe(USER);
    expect(resolveResize(SQUEEZED, USER, 300)).toBe(USER);
    expect(resolveResize(SQUEEZED, USER, USER + 100)).toBe(USER);
  });

  it('still takes a shrink, which IS reachable while squeezed', () => {
    // A smaller panel is always affordable, so dragging one narrower is a real
    // request and must persist - dropping it would make a squeezed panel
    // un-resizable in both directions rather than one.
    expect(resolveResize(SQUEEZED, USER, SQUEEZED)).toBe(SQUEEZED);
    expect(resolveResize(SQUEEZED, USER, 150)).toBe(150);
  });

  /**
   * The drag mutates its own inputs: both `display` and `user` are re-read every
   * render, so shrinking re-solves the squeeze under the pointer. Refusing the
   * unreachable END of such a drag threw away the reachable part of it too.
   */
  it('persists the reachable part of a shrink-then-grow inside one squeezed drag', () => {
    // Down from 400 to 150 - reachable, so the store now holds 150 and the panel is
    // no longer squeezed at that size.
    let user = USER;
    let display = SQUEEZED;
    user = resolveResize(display, user, 150);
    expect(user).toBe(150);
    display = 150;

    // Back up to 300: reachable at this instant (nothing is squeezed at 150), so it
    // lands - and the squeeze immediately re-applies, clamping the rendering to 200.
    user = resolveResize(display, user, 300);
    expect(user).toBe(300);
    display = SQUEEZED;

    // One more pixel of the same drag is now past the FRESH clamp. Refusing it left
    // the end-of-drag persist writing nothing at all, so the 400 the user had
    // deliberately moved away from came straight back on reload.
    const persisted = resolveResize(display, user, 310);
    expect(persisted).toBe(300);
    expect(persisted).not.toBe(USER);
  });
});
