/**
 * RS-FX6, P2-4 (shots 44-45 of the RS capture): with the conversion banner
 * above the toolbar, the full-screen data table started at the toolbar's
 * HEIGHT from the top, so its line picker and column header lay under the
 * toolbar, and a click on Back to Map, Undo, Redo or the menu landed on the
 * hidden sub-header. The table now starts at the toolbar's bottom EDGE,
 * which a banner moves down. Injections: publish only `--toolbar-h`; or keep
 * `var(--toolbar-h)` as the overlay's top.
 */
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { FULLSCREEN_TOP, publishChromeMetrics } from '../chromeMetrics';
import Dock from '../Dock';
import type { PanelReveal } from '../usePanelReveal';

function box(el: HTMLElement, top: number, height: number) {
  Object.defineProperty(el, 'offsetHeight', { configurable: true, value: height });
  el.getBoundingClientRect = () =>
    ({ top, bottom: top + height, height, width: 100, left: 0, right: 100, x: 0, y: top, toJSON: () => ({}) }) as DOMRect;
}

describe('publishChromeMetrics', () => {
  it("publishes the toolbar's bottom edge, which a banner above it moves down", () => {
    const root = document.createElement('div');
    const toolbar = document.createElement('div');
    const status = document.createElement('div');
    box(toolbar, 56, 53);
    box(status, 860, 40);
    publishChromeMetrics(root, toolbar, status);
    expect(root.style.getPropertyValue('--toolbar-h')).toBe('53px');
    expect(root.style.getPropertyValue('--toolbar-bottom')).toBe('109px');
    expect(root.style.getPropertyValue('--statusbar-h')).toBe('40px');
  });
});

const openReveal: PanelReveal = {
  isPinnedOpen: true,
  isFloatingOpen: false,
  isOpen: true,
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

describe('the full-screen data table', () => {
  it("starts at the toolbar's bottom edge", () => {
    render(
      <Dock
        reveal={openReveal}
        edge="bottom"
        size={200}
        minSize={100}
        maxSize={500}
        onResize={() => {}}
        onResizeEnd={() => {}}
        label="Data"
        openArrow={<span />}
        closeArrow={<span />}
        testId="data"
        tabLabels={{ open: 'Open', pin: 'Pin', unpin: 'Unpin', close: 'Close' }}
        fullscreen
      >
        <div data-testid="table-body" />
      </Dock>,
    );
    const overlay = screen.getByTestId('table-body').parentElement!;
    expect(FULLSCREEN_TOP).toContain('--toolbar-bottom');
    expect(overlay.style.top).toBe(FULLSCREEN_TOP);
  });
});
