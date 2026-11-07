import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';

import { useLongPress } from '../useLongPress';

/**
 * The touch long press behind the sections-panel context menu (#338).
 *
 * `.tsx` so this runs in the jsdom tier (vite.config environmentMatchGlobs):
 * every claim here is about DOM events, and half of them are about events the
 * hook has to keep AWAY from the rest of the document.
 *
 * The harness mirrors the one consumer's shape - handlers spread on a row that
 * has children - because what the finger lands on is a child, not the row.
 */

function Harness({
  onLongPress,
}: {
  onLongPress: (position: { x: number; y: number }, target: HTMLElement) => boolean | void;
}) {
  const handlers = useLongPress(onLongPress);
  return (
    <div data-testid="row" {...handlers}>
      <span data-testid="child">row label</span>
      {/* A row's icon: the finger lands on the path, which is not HTML at all. */}
      <span data-testid="icon-wrap">
        <svg viewBox="0 0 8 8">
          <path data-testid="glyph" d="M0 0 L8 8" />
        </svg>
      </span>
    </div>
  );
}

/** A compatibility mouse event of the kind a lift synthesises. */
function compatEvent(target: Element, type: 'mousedown' | 'click'): MouseEvent {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

let onDocumentMouseDown: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  // Bubble phase on the document: where the app's dismissable layers listen,
  // and so the thing a swallowed compat mousedown must never reach.
  onDocumentMouseDown = vi.fn();
  document.addEventListener('mousedown', onDocumentMouseDown);
});

afterEach(() => {
  document.removeEventListener('mousedown', onDocumentMouseDown);
  // Disarm anything a test left armed, so it cannot outlive its own file.
  act(() => {
    vi.runOnlyPendingTimers();
  });
  vi.useRealTimers();
});

describe('useLongPress', () => {
  it('fires after the 500 ms hold, with the press position and the element under the finger', () => {
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 42, clientY: 84 });
    act(() => {
      vi.advanceTimersByTime(499);
    });
    expect(onLongPress).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(onLongPress.mock.calls[0]![0]).toEqual({ x: 42, y: 84 });
    // The child, not the row the handlers are bound to: a menu belongs to what
    // the finger actually touched.
    expect(onLongPress.mock.calls[0]![1]).toBe(child);
  });

  it('arms on an SVG press, reporting the nearest HTML ancestor', () => {
    // A press on an icon lands on an SVGPathElement inside an SVGSVGElement -
    // neither is an HTMLElement, so one step up is not enough and the press
    // would never arm at all.
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const glyph = screen.getByTestId('glyph');

    fireEvent.pointerDown(glyph, { pointerType: 'touch', clientX: 5, clientY: 5 });
    act(() => {
      vi.advanceTimersByTime(500);
    });

    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(onLongPress.mock.calls[0]![1]).toBe(screen.getByTestId('icon-wrap'));
  });

  it('ignores a mouse press entirely', () => {
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'mouse', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(2000);
    });

    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('cancels the press once the finger travels past 10 px', () => {
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 100, clientY: 100 });
    fireEvent.pointerMove(child, { pointerType: 'touch', clientX: 111, clientY: 100 });
    act(() => {
      vi.advanceTimersByTime(500);
    });

    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('drops the press on pointercancel', () => {
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    fireEvent.pointerCancel(child, { pointerType: 'touch' });
    act(() => {
      vi.advanceTimersByTime(500);
    });

    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('keeps the compatibility tap that follows a fired press out of the document', () => {
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(500);
    });
    fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });

    compatEvent(child, 'mousedown');
    expect(onDocumentMouseDown).not.toHaveBeenCalled();
    expect(compatEvent(child, 'click').defaultPrevented).toBe(true);
  });

  it('lets the mouse through again once the suppression window expires', () => {
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(500);
    });
    fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });

    // Platforms that synthesise nothing leave the window to time out; the app
    // must be usable by mouse the moment it does.
    act(() => {
      vi.advanceTimersByTime(700);
    });
    compatEvent(child, 'mousedown');

    expect(onDocumentMouseDown).toHaveBeenCalledTimes(1);
  });

  it('leaves a short tap alone', () => {
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    compatEvent(child, 'mousedown');

    expect(onLongPress).not.toHaveBeenCalled();
    // Tap-to-select rides on the compat mousedown, so a tap must reach it.
    expect(onDocumentMouseDown).toHaveBeenCalledTimes(1);
  });

  it('swallows the tap after a hold far longer than the suppression window', () => {
    // The compat events come from the LIFT, so the window has to be armed
    // there. Armed when the timer fired, a 2 s hold would outlive it and leak
    // the tap into the menu the hold just opened.
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(onLongPress).toHaveBeenCalledTimes(1);

    fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    compatEvent(child, 'mousedown');

    expect(onDocumentMouseDown).not.toHaveBeenCalled();
  });

  it('gets out of the way of the next real gesture', () => {
    // On a platform that suppresses the compat events itself, nothing arrives
    // to close the window early - and the user's genuine tap on the menu the
    // press opened is well inside 700 ms. A real gesture starts with a
    // pointerdown; the lift's compat sequence never does, so that is the signal.
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(500);
    });
    fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });

    act(() => {
      vi.advanceTimersByTime(300);
    });
    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    compatEvent(child, 'mousedown');

    expect(onDocumentMouseDown).toHaveBeenCalledTimes(1);
    expect(compatEvent(child, 'click').defaultPrevented).toBe(false);
  });

  it('prevents a compatibility contextmenu, not just its propagation', () => {
    // Stopping propagation alone leaves the browser default live, and for this
    // event that default is a native menu drawn over ours.
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(500);
    });
    fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });

    const menu = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    child.dispatchEvent(menu);
    expect(menu.defaultPrevented).toBe(true);
  });

  it('takes its listeners with it when the consumer unmounts', () => {
    // The menu a press opens can unmount the row underneath it.
    const onLongPress = vi.fn();
    const { unmount } = render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(500);
    });
    fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => unmount());

    compatEvent(document.body, 'mousedown');
    expect(onDocumentMouseDown).toHaveBeenCalledTimes(1);
  });

  it('leaves the tap alone when the callback says it opened nothing', () => {
    const onLongPress = vi.fn(() => false);
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(onLongPress).toHaveBeenCalledTimes(1);
    fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    compatEvent(child, 'mousedown');

    expect(onDocumentMouseDown).toHaveBeenCalledTimes(1);
  });

  it('re-arms for a second long press', () => {
    const onLongPress = vi.fn();
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    for (const _pass of [0, 1]) {
      fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
      act(() => {
        vi.advanceTimersByTime(500);
      });
      fireEvent.pointerUp(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
      compatEvent(child, 'mousedown');
      // The click both ends the tap and closes the window it was armed for.
      expect(compatEvent(child, 'click').defaultPrevented).toBe(true);
    }

    expect(onLongPress).toHaveBeenCalledTimes(2);
    expect(onDocumentMouseDown).not.toHaveBeenCalled();
  });

  /** A callback that records how many ms after `t0` each long press fired. */
  function timedCallback(t0: number): { fired: number[]; onLongPress: () => void } {
    const fired: number[] = [];
    return { fired, onLongPress: () => void fired.push(Date.now() - t0) };
  }

  it.each(['pointerUp', 'pointerCancel'] as const)(
    'a second finger does not orphan the first finger\'s pending press (%s ends both)',
    (end) => {
      // Two fingers on one row at 0 and 30 ms, both gone at 120 ms: nothing is
      // still held, so nothing may open. The second pointerdown must not leave
      // the first one's timer running where no lift can reach it.
      const { fired, onLongPress } = timedCallback(Date.now());
      render(<Harness onLongPress={onLongPress} />);
      const child = screen.getByTestId('child');

      fireEvent.pointerDown(child, { pointerType: 'touch', pointerId: 1, clientX: 10, clientY: 10 });
      act(() => {
        vi.advanceTimersByTime(30);
      });
      fireEvent.pointerDown(child, { pointerType: 'touch', pointerId: 2, clientX: 40, clientY: 10 });
      act(() => {
        vi.advanceTimersByTime(90);
      });
      fireEvent[end](child, { pointerType: 'touch', pointerId: 1, clientX: 10, clientY: 10 });
      fireEvent[end](child, { pointerType: 'touch', pointerId: 2, clientX: 40, clientY: 10 });
      act(() => {
        vi.advanceTimersByTime(1000);
      });

      expect(fired, 'ms at which a long press fired after both fingers lifted at 120 ms').toEqual(
        [],
      );
    },
  );

  it('a second finger restarts the hold rather than stacking a second one', () => {
    // Both fingers stay down: one press, timed from the latest pointerdown.
    const { fired, onLongPress } = timedCallback(Date.now());
    render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', pointerId: 1, clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(30);
    });
    fireEvent.pointerDown(child, { pointerType: 'touch', pointerId: 2, clientX: 40, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(1000);
    });

    expect(fired, 'ms at which each long press fired').toEqual([530]);
  });

  it('drops a pending press when the consumer unmounts', () => {
    // A virtualised row can be recycled mid-hold; its callback must not fire
    // for a row that is no longer there.
    const { fired, onLongPress } = timedCallback(Date.now());
    const { unmount } = render(<Harness onLongPress={onLongPress} />);
    const child = screen.getByTestId('child');

    fireEvent.pointerDown(child, { pointerType: 'touch', clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    act(() => unmount());
    act(() => {
      vi.advanceTimersByTime(1000);
    });

    expect(fired, 'ms at which a long press fired for the unmounted consumer').toEqual([]);
  });
});
