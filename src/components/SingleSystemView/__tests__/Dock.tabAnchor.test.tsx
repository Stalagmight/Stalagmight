import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

import Dock from '../Dock';
import type { PanelReveal } from '../usePanelReveal';

/**
 * #260, in the component: the opener tab's inset is clamped against the CONTAINER
 * MEASUREMENT, and the measurement that matters is the anchor's OWN axis - the
 * bottom tab is inset from its container's right edge (width), a side tab from the
 * top edge (height).
 *
 * `clampTabInset` is unit-tested next door; this pins the wiring, including the two
 * readings that used to be conflated: an unmeasured container passes the requested
 * inset through, while a MEASURED zero - which is exactly what a side bar squeezed
 * by an over-tall Data panel reports - is clamped like any other number.
 */

/** The reveal a CLOSED panel presents (the state whose tab has to stay reachable). */
const closedReveal: PanelReveal = {
  isPinnedOpen: false,
  isFloatingOpen: false,
  isOpen: false,
  onRegionFocus: () => {},
  onRegionBlur: () => {},
  revealTemporarily: () => {},
  onRegionEnter: () => {},
  onRegionLeave: () => {},
  scheduleClose: () => {},
  openClick: () => {},
  pin: () => {},
  unpin: () => {},
  close: () => {},
};

const LABELS = { open: 'Open', pin: 'Pin', unpin: 'Unpin', close: 'Close' };

/** Fake ResizeObserver: jsdom has none, and the test drives the callback itself. */
function installResizeObserver(): { fire: () => void } {
  const callbacks: Array<() => void> = [];
  class FakeResizeObserver {
    constructor(cb: () => void) {
      callbacks.push(cb);
    }
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  return {
    fire: () => {
      act(() => {
        for (const cb of callbacks) cb();
      });
    },
  };
}

/** Make `el` report a fixed box (jsdom measures everything as 0x0). */
function sizeOf(el: Element, width: number, height: number) {
  el.getBoundingClientRect = () =>
    ({ width, height, top: 0, left: 0, right: width, bottom: height, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
}

function renderDock(edge: 'bottom' | 'left') {
  render(
    <Dock
      reveal={closedReveal}
      edge={edge}
      size={200}
      minSize={100}
      maxSize={500}
      onResize={() => {}}
      onResizeEnd={() => {}}
      label="Data"
      openArrow={<span />}
      closeArrow={<span />}
      testId="data"
      tabLabels={LABELS}
      tabInset={40}
      tabTop={79}
    >
      <div />
    </Dock>,
  );
  const bar = screen.getByTestId('data-panel-bar');
  const outer = bar.parentElement!;
  // outer > tab wrapper (the positioned box) > tab box > button
  const tabWrapper = screen.getByTestId('panel-tab-data').parentElement!.parentElement!;
  return { outer, tabWrapper };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('Dock opener-tab anchor', () => {
  it('passes the requested inset through while nothing has been measured', () => {
    // No ResizeObserver at all (the node tier, and the first paint before the effect
    // runs): the extents stay `null`, which is the ONLY "not measured yet" state.
    // Clamping against a container nobody has measured would yank every tab to 0.
    vi.stubGlobal('ResizeObserver', undefined);
    const { tabWrapper } = renderDock('bottom');
    expect(tabWrapper.style.right).toBe('40px');
  });

  it('clamps the bottom tab against the container WIDTH', () => {
    const ro = installResizeObserver();
    const { outer, tabWrapper } = renderDock('bottom');
    // A 30px-wide container holding a 28px tab: the largest inset that keeps the
    // tab inside is 2, not the requested 40.
    sizeOf(outer, 30, 600);
    sizeOf(tabWrapper, 28, 28);
    ro.fire();
    expect(tabWrapper.style.right).toBe('2px');

    // The OTHER axis is not part of the anchor: a height-only change (which is what
    // a Data-panel resize drag reports, ~60x/s) must not move the inset.
    sizeOf(outer, 30, 120);
    ro.fire();
    expect(tabWrapper.style.right).toBe('2px');
  });

  it('clamps a MEASURED zero rather than treating it as "not measured yet"', () => {
    const ro = installResizeObserver();
    const { outer, tabWrapper } = renderDock('bottom');
    // A real zero: the side bars' overlays compute to height 0 whenever the Data
    // panel's height reaches the container's, and the old `container > 0` guard
    // passed the full inset through in exactly that case - straight outside the box.
    sizeOf(outer, 0, 0);
    sizeOf(tabWrapper, 28, 28);
    ro.fire();
    expect(tabWrapper.style.right).toBe('0px');
  });

  it('clamps a side tab against the container HEIGHT instead', () => {
    const ro = installResizeObserver();
    const { outer, tabWrapper } = renderDock('left');
    sizeOf(outer, 600, 30);
    sizeOf(tabWrapper, 28, 28);
    ro.fire();
    expect(tabWrapper.style.top).toBe('2px');

    // Width-only change: not the side tabs' axis, so the inset holds.
    sizeOf(outer, 120, 30);
    ro.fire();
    expect(tabWrapper.style.top).toBe('2px');
  });
});
