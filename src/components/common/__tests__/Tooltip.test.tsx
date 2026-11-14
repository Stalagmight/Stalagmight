/**
 * Tooltip placement: the default 'top' (unchanged for existing callers) and
 * 'right', which sits beside an anchor (the trigger, or `anchorRef`) and only
 * flips left, then above, when the viewport has no room.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { useRef } from 'react';
import { Tooltip, tooltipPosition } from '../Tooltip';

const rect = (left: number, top: number, width: number, height: number) =>
  ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) }) as DOMRect;

beforeEach(() => {
  vi.stubGlobal('innerWidth', 1000);
  vi.stubGlobal('innerHeight', 800);
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('tooltipPosition', () => {
  const trigger = rect(40, 300, 100, 20);
  const gutter = rect(0, 296, 232, 33);
  const bubble = rect(0, 0, 300, 200);

  it("'top' centres above the trigger, as before", () => {
    expect(tooltipPosition('top', trigger, gutter, bubble)).toEqual({ left: 158, top: 290, transform: 'translate(-50%, -100%)' });
  });

  it("'right' sits 8px right of the anchor, top-aligned with it", () => {
    expect(tooltipPosition('right', trigger, gutter, bubble)).toEqual({ left: 240, top: 296, transform: 'none' });
    // Not measured yet: the same spot.
    expect(tooltipPosition('right', trigger, gutter, null)).toEqual({ left: 240, top: 296, transform: 'none' });
  });

  it("'right' is clamped to the viewport's bottom and top", () => {
    expect(tooltipPosition('right', trigger, rect(0, 700, 232, 33), bubble).top).toBe(800 - 8 - 200);
    expect(tooltipPosition('right', trigger, rect(0, -20, 232, 33), bubble).top).toBe(8);
  });

  it("'right' flips to the anchor's left when the right has no room, then above", () => {
    const farRight = rect(700, 296, 200, 33);
    expect(tooltipPosition('right', trigger, farRight, bubble)).toEqual({ left: 700 - 8 - 300, top: 296, transform: 'none' });
    const wide = rect(100, 296, 700, 33);
    expect(tooltipPosition('right', trigger, wide, bubble).transform).toBe('translate(-50%, -100%)');
  });
});

describe('<Tooltip placement="right">', () => {
  function Harness() {
    const anchor = useRef<HTMLDivElement>(null);
    return (
      <div ref={anchor} data-testid="anchor">
        <Tooltip content="details" placement="right" anchorRef={anchor} tooltipTestId="bubble">
          <span data-testid="name">Main:3</span>
        </Tooltip>
      </div>
    );
  }

  it('places the bubble beside the anchor element, not above the trigger', () => {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this.dataset.testid === 'anchor') return rect(0, 400, 232, 33);
      if (this.dataset.testid === 'bubble') return rect(0, 0, 260, 120);
      return rect(40, 405, 80, 20);
    });
    render(<Harness />);
    act(() => {
      fireEvent.mouseEnter(screen.getByTestId('name').parentElement!);
    });
    const bubble = screen.getByTestId('bubble');
    expect(bubble.style.left).toBe('240px');
    expect(bubble.style.top).toBe('400px');
    expect(bubble.style.transform).toBe('none');
  });
});
