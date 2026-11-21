/**
 * Demo finding V-3: a disabled menu row's reason opened ABOVE the row, over
 * the two rows above it, on the translucent tooltip surface, so "Save Ctrl+S"
 * showed through its text. The reason bubble is now the opaque panel surface
 * and opens at the row's side, vertically centred on it, so it covers
 * neither neighbour; at the window's right edge it opens on the left.
 * Injection: give `DisabledReason` the default placement (`above`) and the
 * side cases fail; drop `opaque` and the surface case fails.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { DisabledReason, DISABLED_REASON_DELAY_MS } from '../DisabledReason';
import { tooltipPosition } from '../Tooltip';

const rect = (left: number, top: number, width: number, height: number) => ({
  left,
  top,
  width,
  height,
  right: left + width,
  bottom: top + height,
});
const VIEWPORT = { width: 1440, height: 900 };

/** The bubble's box for a placement result. */
function bubbleBox(
  at: { left: number; top: number; transform: string },
  size: { width: number; height: number },
): { left: number; right: number; top: number; bottom: number } {
  const [tx, ty] = (at.transform.match(/-?\d+%|0\b/g) ?? []).map((v) => (v === '0' ? 0 : parseFloat(v) / 100));
  const left = at.left + (tx ?? 0) * size.width;
  const top = at.top + (ty ?? 0) * size.height;
  return { left, right: left + size.width, top, bottom: top + size.height };
}

describe('tooltipPosition side placement (V-3)', () => {
  const bubble = { width: 320, height: 64 };

  it('puts the bubble beside a menu row, clear of the menu and centred on the row', () => {
    // A 220 px wide app menu at the left; the row "Save and Close" at y 96.
    const row = rect(16, 96, 220, 36);
    const at = tooltipPosition('side', row, bubble, VIEWPORT);
    const box = bubbleBox(at, bubble);
    expect(box.left).toBeGreaterThanOrEqual(row.right);
    expect((box.top + box.bottom) / 2).toBeCloseTo(row.top + row.height / 2);
  });

  it('opens on the left at the right edge of the window', () => {
    const row = rect(1200, 400, 220, 36);
    const box = bubbleBox(tooltipPosition('side', row, bubble, VIEWPORT), bubble);
    expect(box.right).toBeLessThanOrEqual(row.left);
  });

  it('stays inside the window at its top and bottom', () => {
    const top = bubbleBox(tooltipPosition('side', rect(16, 0, 220, 20), bubble, VIEWPORT), bubble);
    expect(top.top).toBeGreaterThanOrEqual(8);
    const bottom = bubbleBox(tooltipPosition('side', rect(16, 890, 220, 10), bubble, VIEWPORT), bubble);
    expect(bottom.bottom).toBeLessThanOrEqual(VIEWPORT.height - 8);
  });

  it('keeps the old above-then-below rule as the default, and below for the toolbar', () => {
    const button = rect(100, 8, 80, 30);
    // At the top of the window there is no room above: below.
    expect(bubbleBox(tooltipPosition('above', button, bubble, VIEWPORT), bubble).top).toBeGreaterThanOrEqual(
      button.bottom,
    );
    expect(bubbleBox(tooltipPosition('below', button, bubble, VIEWPORT), bubble).top).toBeGreaterThanOrEqual(
      button.bottom,
    );
    const low = rect(100, 400, 80, 30);
    expect(bubbleBox(tooltipPosition('above', low, bubble, VIEWPORT), bubble).bottom).toBeLessThanOrEqual(low.top);
  });
});

describe('DisabledReason', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('shows the reason on the opaque surface, at the side, after the delay', () => {
    vi.useFakeTimers();
    render(
      <DisabledReason reason="Synthetic: open read-only." testId="row-reason">
        <button type="button" aria-disabled="true">
          Save
        </button>
      </DisabledReason>,
    );
    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Save' }).parentElement!);
    expect(screen.queryByTestId('row-reason')).toBeNull();
    act(() => {
      vi.advanceTimersByTime(DISABLED_REASON_DELAY_MS);
    });
    const bubble = screen.getByTestId('row-reason');
    // The position is measured on the next frame.
    act(() => {
      vi.advanceTimersByTime(50);
    });
    expect(bubble).toHaveTextContent('Synthetic: open read-only.');
    expect(bubble.className).toMatch(/(^|\s)bg-panel(\s|$)/);
    expect(bubble.className).not.toMatch(/bg-panel-90/);
    // The side placement hangs the bubble off the trigger's right edge.
    expect(bubble.style.transform).toBe('translate(0, -50%)');
  });
});
