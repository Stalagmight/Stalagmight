/**
 * Component test for the toast stack: what renders, the accent per kind, the
 * dwell/fade lifecycle, hover/focus pausing, manual dismiss, and the
 * reduced-motion path.
 *
 * The lifecycle runs on a 50 ms interval, so every timing assertion here uses
 * fake timers - no sleeps, no wall-clock flakes.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, render, screen, within } from '@testing-library/react';

// Initialise the real i18next instance so the dismiss button's accessible name
// is the actual English copy (and this test fails if the key goes missing).
import '../../i18n';
import { ToastStack } from './ToastStack';
import { useUIStore } from '../../store/uiStore';
import { MAX_VISIBLE_TOASTS, TOAST_GAP_PX } from '../../utils/toastCapacity';

/**
 * jsdom has no layout engine: every box measures 0x0, and the stack is now
 * position-driven off measured heights. Stub the measurable box of the toast
 * elements (and only those) so the offsets and the fitted capacity are real
 * numbers rather than a column of zeroes.
 *
 * The stubbed height deliberately differs from the component's
 * ESTIMATED_TOAST_HEIGHT_PX, so an assertion on an offset can only pass if the
 * element was actually measured.
 */
const TOAST_HEIGHT = 60;
const TOAST_WIDTH = 320;
const SLOT = TOAST_HEIGHT + TOAST_GAP_PX;

/**
 * Where the stack's box sits, for the hover hit-test.
 *
 * The stack is click-through (`pointer-events-none`), so it receives no
 * mouseenter/mouseleave of its own - the pause is decided by testing the pointer
 * position against this box. jsdom's `getBoundingClientRect` is all zeros, which
 * would make every event at (0, 0) read as "inside", so the stack element (and
 * only it) reports a real one.
 */
const STACK_BOX = { left: 24, top: 600, right: 344, bottom: 660 };
const INSIDE = { clientX: 100, clientY: 630 };
const OUTSIDE = { clientX: 900, clientY: 100 };

const originalHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight');
const originalWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth');
const originalRect = Object.getOwnPropertyDescriptor(
  HTMLElement.prototype,
  'getBoundingClientRect',
);

/**
 * Move the pointer somewhere.
 *
 * The stack listens on document, capture-phase, for exactly ONE move event:
 * `pointermove` wherever PointerEvent exists (every webview the app ships in, and
 * the jsdom this runs under), `mousemove` only as the fallback for a runtime
 * without it.
 * The same choice is made here rather than firing both, so a component that went
 * back to double-registering - or that listened for the wrong one - shows up as a
 * failure instead of being papered over. A plain MouseEvent under the pointer
 * type is enough: the handler reads nothing but clientX/clientY.
 */
const MOVE_EVENT = typeof PointerEvent === 'function' ? 'pointermove' : 'mousemove';

function movePointerTo(at: { clientX: number; clientY: number }) {
  act(() => {
    document.dispatchEvent(new MouseEvent(MOVE_EVENT, { bubbles: true, ...at }));
  });
}

const hoverStack = () => movePointerTo(INSIDE);
const leaveStack = () => movePointerTo(OUTSIDE);

function stubLayout() {
  Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', {
    configurable: true,
    value(this: HTMLElement) {
      // Toasts must keep reporting a zero rect: `measure` prefers `rect.height`
      // and only falls back to the stubbed `offsetHeight`, so a real rect here
      // would quietly replace the height the offsets are asserted against.
      if (this.dataset?.testid !== 'toast-stack' && this.getAttribute('data-testid') !== 'toast-stack') {
        return { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 } as DOMRect;
      }
      return {
        ...STACK_BOX,
        x: STACK_BOX.left,
        y: STACK_BOX.top,
        width: STACK_BOX.right - STACK_BOX.left,
        height: STACK_BOX.bottom - STACK_BOX.top,
      } as DOMRect;
    },
  });
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset?.toastId ? TOAST_HEIGHT : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset?.toastId ? TOAST_WIDTH : 0;
    },
  });
}

function restoreLayout() {
  if (originalHeight) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', originalHeight);
  if (originalWidth) Object.defineProperty(HTMLElement.prototype, 'offsetWidth', originalWidth);
  if (originalRect)
    Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', originalRect);
}

/** jsdom's window.innerHeight is 768; the capacity rule needs a known one. */
function setViewportHeight(px: number) {
  Object.defineProperty(window, 'innerHeight', { configurable: true, writable: true, value: px });
}

function resizeTo(px: number) {
  act(() => {
    setViewportHeight(px);
    window.dispatchEvent(new Event('resize'));
  });
}

/** The `translateY(...)` px value currently applied to a toast. */
const offsetOf = (message: string) => toastFor(message)?.style.transform ?? '';

/** Install a matchMedia stub; jsdom does not provide one. */
function stubReducedMotion(reduce: boolean) {
  const listeners = new Set<() => void>();
  vi.stubGlobal(
    'matchMedia',
    vi.fn((query: string) => ({
      matches: reduce && query.includes('reduce'),
      media: query,
      onchange: null,
      addEventListener: (_: string, fn: () => void) => listeners.add(fn),
      removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  );
}

/**
 * A `MediaQueryList` as the OLD WebKit the macOS build can run in exposes it:
 * the deprecated `addListener` / `removeListener` pair and no
 * `addEventListener` at all. `stubReducedMotion` above offers both, so it can
 * never catch a fallback that forgets to subscribe.
 *
 * Returns handles to flip the preference (fanning the change out to whoever
 * subscribed) and to count live subscriptions, so cleanup is checkable.
 */
function stubLegacyReducedMotion(reduce: boolean) {
  const listeners = new Set<(event: MediaQueryListEvent) => void>();
  const mq = {
    matches: reduce,
    media: '(prefers-reduced-motion: reduce)',
    onchange: null,
    addListener: (fn: (event: MediaQueryListEvent) => void) => {
      listeners.add(fn);
    },
    removeListener: (fn: (event: MediaQueryListEvent) => void) => {
      listeners.delete(fn);
    },
    dispatchEvent: () => false,
  };
  vi.stubGlobal('matchMedia', vi.fn(() => mq));
  return {
    listenerCount: () => listeners.size,
    set(next: boolean) {
      act(() => {
        mq.matches = next;
        for (const fn of [...listeners]) fn({ matches: next } as MediaQueryListEvent);
      });
    },
  };
}

type Kind = 'info' | 'success' | 'warning' | 'error';

function push(message: string, key: string, kind: Kind = 'info') {
  act(() => {
    useUIStore.getState().showToast(message, key, kind);
  });
}

/**
 * Push several toasts inside ONE React batch, the way a single event handler (or
 * one synchronous run of store code) does: React 18 coalesces the store writes,
 * so the stack's very next render already carries all of them and there is no
 * intermediate render holding only the first.
 */
function pushBatch(...entries: Array<[message: string, key: string, kind?: Kind]>) {
  act(() => {
    const { showToast } = useUIStore.getState();
    for (const [message, key, kind] of entries) showToast(message, key, kind ?? 'info');
  });
}

/** Advance the lifecycle clock by `ms`, flushing React updates as it goes. */
function advance(ms: number) {
  act(() => {
    vi.advanceTimersByTime(ms);
  });
}

const toasts = () => screen.queryAllByTestId('toast');
const toastFor = (message: string) =>
  toasts().find((el) => within(el).queryByText(message) !== null);

beforeEach(() => {
  stubReducedMotion(false);
  stubLayout();
  // Tall enough that the 20% budget is not the binding constraint: 5 x 60 plus
  // gaps is 332px, well inside 20% of 2000.
  setViewportHeight(2000);
  vi.useFakeTimers();
  useUIStore.setState({ toasts: [], toastCapacity: MAX_VISIBLE_TOASTS });
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  restoreLayout();
  // Empty the shared store by writing the field, exactly as `beforeEach` does.
  // Not through a store action: the component is already unmounted by now (RTL's
  // cleanup runs first), so this is plain state hygiene between cases and there
  // is nothing left to animate. It used to go through `clearToast()`, a store
  // action with no production caller that survived only to serve this one line.
  useUIStore.setState({ toasts: [] });
});

describe('ToastStack - rendering', () => {
  it('renders nothing visible until a toast arrives', () => {
    render(<ToastStack />);
    expect(toasts()).toHaveLength(0);
  });

  it('renders one element per toast, oldest first (so the newest sits at the bottom)', () => {
    render(<ToastStack />);
    push('first', 'a');
    push('second', 'b');
    push('third', 'c');

    expect(toasts().map((el) => el.textContent)).toEqual([
      expect.stringContaining('first'),
      expect.stringContaining('second'),
      expect.stringContaining('third'),
    ]);
  });

  it('shows only the capped set once the stack overflows', () => {
    render(<ToastStack />);
    for (let i = 0; i < 8; i += 1) push(`m${i}`, `k${i}`);

    // A 2000px window has room for the ceiling of five; the rest are evicted and
    // are still playing their exit animation.
    expect(toasts().filter((el) => el.dataset.toastLeaving !== 'true')).toHaveLength(5);

    // The exits finish and the evicted ones are gone.
    advance(300);
    expect(toasts()).toHaveLength(5);
    expect(toastFor('m0')).toBeUndefined();
    expect(toastFor('m7')).toBeDefined();
  });
});

describe('ToastStack - capacity fitted to the window', () => {
  it('caps at five in a tall window', () => {
    render(<ToastStack />);
    for (let i = 0; i < 7; i += 1) push(`m${i}`, `k${i}`);
    advance(300);

    expect(toasts()).toHaveLength(5);
    expect(useUIStore.getState().toastCapacity).toBe(5);
  });

  it('fits fewer toasts in a short window', () => {
    // 20% of 700 is 140px: 60 + 8 + 60 = 128 fits, a third does not.
    setViewportHeight(700);
    render(<ToastStack />);
    for (let i = 0; i < 5; i += 1) push(`m${i}`, `k${i}`);
    advance(300);

    expect(useUIStore.getState().toastCapacity).toBe(2);
    expect(toasts()).toHaveLength(2);
    expect(toastFor('m4')).toBeDefined();
  });

  it('still shows two when two do not fit the budget', () => {
    // 20% of 320 is 64px - room for one 60px toast and no more.
    setViewportHeight(320);
    render(<ToastStack />);
    push('first', 'a');
    push('second', 'b');
    advance(300);

    expect(useUIStore.getState().toastCapacity).toBe(2);
    expect(toasts()).toHaveLength(2);
  });

  it('shrinks the capacity budget when a bottom offset is registered (F8)', () => {
    // A tall window (2000px) alone fits all five ("caps at five in a tall
    // window" above pins that at zero offset). Registering a 1900px offset
    // (mimicking a tall dialog footer) leaves only 100px of real headroom
    // above the raised anchor, 20% of which is 20px - room for none of the
    // 60px toasts, clamped to the MIN_VISIBLE_TOASTS floor of 2. If the
    // offset were applied only to the anchor's position and not subtracted
    // from the capacity budget, this would still read 5.
    setViewportHeight(2000);
    render(<ToastStack />);
    useUIStore.getState().registerToastBottomOffset('test-owner', 1900);
    for (let i = 0; i < 5; i += 1) push(`m${i}`, `k${i}`);
    advance(300);

    expect(useUIStore.getState().toastCapacity).toBe(2);
    expect(toasts()).toHaveLength(2);

    useUIStore.getState().unregisterToastBottomOffset('test-owner');
  });

  it('re-fits on resize, evicting what no longer has room', () => {
    render(<ToastStack />);
    for (let i = 0; i < 5; i += 1) push(`m${i}`, `k${i}`);
    advance(100);
    expect(toasts().filter((el) => el.dataset.toastLeaving !== 'true')).toHaveLength(5);

    resizeTo(700);
    advance(300);

    expect(useUIStore.getState().toastCapacity).toBe(2);
    // The overflow is evicted rather than left off-screen with a live timer.
    expect(toasts()).toHaveLength(2);
    expect(toastFor('m0')).toBeUndefined();
    expect(toastFor('m4')).toBeDefined();
  });

  it('re-fits when the window grows again', () => {
    setViewportHeight(700);
    render(<ToastStack />);
    push('one', 'a');
    advance(100);
    expect(useUIStore.getState().toastCapacity).toBe(2);

    resizeTo(2000);
    advance(100);
    expect(useUIStore.getState().toastCapacity).toBe(5);
  });
});

describe('ToastStack - motion', () => {
  it('positions the newest toast on the anchor and lifts the older ones off it', () => {
    render(<ToastStack />);
    push('older', 'a');
    push('newer', 'b');
    advance(100);

    expect(offsetOf('newer')).toBe('translateY(0px)');
    expect(offsetOf('older')).toBe(`translateY(-${SLOT}px)`);
  });

  it('recomputes every offset when a new toast arrives (the push)', () => {
    render(<ToastStack />);
    push('first', 'a');
    advance(100);
    expect(offsetOf('first')).toBe('translateY(0px)');

    push('second', 'b');
    advance(100);
    expect(offsetOf('first')).toBe(`translateY(-${SLOT}px)`);

    push('third', 'c');
    advance(100);
    expect(offsetOf('first')).toBe(`translateY(-${SLOT * 2}px)`);
    expect(offsetOf('second')).toBe(`translateY(-${SLOT}px)`);
    expect(offsetOf('third')).toBe('translateY(0px)');
  });

  it('uses the MEASURED height rather than a nominal row height', () => {
    render(<ToastStack />);
    push('older', 'a');
    push('newer', 'b');
    advance(100);

    // 68 = the stubbed 60px box plus the 8px gap. A hardcoded row height would
    // put this somewhere else.
    expect(offsetOf('older')).toBe('translateY(-68px)');
  });

  it('closes the gap when a toast in the middle is dismissed', () => {
    render(<ToastStack />);
    push('top', 'a');
    push('middle', 'b');
    push('bottom', 'c');
    advance(100);
    expect(offsetOf('top')).toBe(`translateY(-${SLOT * 2}px)`);

    act(() => {
      within(toastFor('middle')!).getByRole('button').click();
    });

    // The survivor slides down into the freed slot; the leaver drifts up out of
    // its own (it keeps the offset it held, plus the exit lift).
    expect(offsetOf('top')).toBe(`translateY(-${SLOT}px)`);
    expect(offsetOf('middle')).toBe(`translateY(-${SLOT + 12}px)`);
  });

  it('enters from below its slot at zero opacity, then settles into it', () => {
    render(<ToastStack />);
    push('arriving', 'a');

    // First frame: below the slot, invisible.
    expect(offsetOf('arriving')).toBe('translateY(12px)');
    expect(toastFor('arriving')?.style.opacity).toBe('0');

    advance(100);
    expect(offsetOf('arriving')).toBe('translateY(0px)');
    expect(toastFor('arriving')?.style.opacity).toBe('1');
  });

  it('keeps a leaving toast mounted for the exit, drifting up, then removes it', () => {
    render(<ToastStack />);
    push('bye', 'a');
    advance(100);

    act(() => {
      within(toastFor('bye')!).getByRole('button').click();
    });

    // Still mounted, above its slot, transparent.
    expect(toastFor('bye')).toBeDefined();
    expect(toastFor('bye')?.dataset.toastLeaving).toBe('true');
    expect(offsetOf('bye')).toBe('translateY(-12px)');
    expect(toastFor('bye')?.style.opacity).toBe('0');

    // Part-way through the 180ms exit it is still there.
    advance(100);
    expect(toastFor('bye')).toBeDefined();

    // Past it, gone.
    advance(150);
    expect(toastFor('bye')).toBeUndefined();
  });

  it('carries a transition so the offsets animate rather than jump', () => {
    render(<ToastStack />);
    push('moving', 'a');
    advance(100);

    const el = toastFor('moving');
    expect(el?.className).toContain('transition-[opacity,transform]');
    expect(el?.style.transitionDuration).toBe('180ms');
  });
});

describe('ToastStack - accents per kind', () => {
  it.each([
    ['success', 'border-success', 'bg-success-10'],
    ['warning', 'border-warning', 'bg-warning-10'],
    ['error', 'border-error', 'bg-error-10'],
  ] as const)('%s carries its theme-token accent', (kind, border, tint) => {
    render(<ToastStack />);
    push('accented', 'k', kind);

    const el = toastFor('accented');
    expect(el?.dataset.toastKind).toBe(kind);
    const body = el?.querySelector('div');
    expect(body?.className).toContain(border);
    expect(body?.className).toContain(tint);
  });

  it('info keeps the neutral look - no accent tint', () => {
    render(<ToastStack />);
    push('neutral', 'k');

    const el = toastFor('neutral');
    expect(el?.dataset.toastKind).toBe('info');
    const body = el?.querySelector('div');
    expect(body?.className).not.toMatch(/bg-(success|warning|error)-10/);
  });
});

describe('ToastStack - lifecycle', () => {
  it('holds full opacity through the dwell, then fades, then removes itself', () => {
    render(<ToastStack />);
    push('temporary', 'k');

    advance(4000);
    expect(toastFor('temporary')?.style.opacity).toBe('1');

    // Into the fade: still on screen, but no longer fully opaque.
    advance(2000);
    const midFade = Number(toastFor('temporary')?.style.opacity);
    expect(midFade).toBeGreaterThan(0);
    expect(midFade).toBeLessThan(1);

    // Past dwell + fade.
    advance(1000);
    expect(toastFor('temporary')).toBeUndefined();
    expect(useUIStore.getState().toasts).toHaveLength(0);
  });

  it('gives an error a longer dwell than a success', () => {
    render(<ToastStack />);
    push('went well', 'ok', 'success');
    push('went badly', 'bad', 'error');

    // Past the success lifetime (5s dwell + 1.5s fade) but not the error's.
    advance(6600);
    expect(toastFor('went well')).toBeUndefined();
    expect(toastFor('went badly')).toBeDefined();
    expect(toastFor('went badly')?.style.opacity).toBe('1');

    advance(4000);
    expect(toastFor('went badly')).toBeUndefined();
  });

  it('rises as it fades, not after it (the fade-in-place regression)', () => {
    render(<ToastStack />);
    push('drifts away', 'k');

    // Sitting on its slot at full opacity: no travel yet. (Read the number
    // rather than the string - the sole toast sits at -0, which serialises to
    // "0px".)
    advance(4000);
    const settled = Number(
      /translateY\((-?[\d.]+)px\)/.exec(toastFor('drifts away')?.style.transform ?? '')?.[1],
    );
    expect(settled).toBe(0);

    // Mid-fade the toast must ALREADY have travelled upward. The bug was that
    // the 12px exit drift was only applied once the toast was marked `leaving`,
    // which happens after the tick has taken it to opacity 0 - so the travel was
    // applied to an invisible box and it simply faded in place.
    advance(1500);
    const el = toastFor('drifts away');
    const opacity = Number(el?.style.opacity);
    expect(opacity).toBeGreaterThan(0);
    expect(opacity).toBeLessThan(1);

    const y = Number(/translateY\((-?[\d.]+)px\)/.exec(el?.style.transform ?? '')?.[1]);
    expect(y).toBeLessThan(0);
  });

  it('restarts the clock when the same key refreshes the toast', () => {
    render(<ToastStack />);
    push('attempt one', 'dup');

    advance(4000);
    push('attempt two', 'dup');

    // Without the reset this would already be past its lifetime.
    advance(4000);
    expect(toastFor('attempt two')?.style.opacity).toBe('1');
    expect(toasts()).toHaveLength(1);
  });
});

/**
 * The stack floats over the survey map and the side panels, and it is transient
 * feedback the user never asked for - so it must not eat a click aimed at what is
 * underneath it. It briefly did: the measured box carried `pointer-events-auto`
 * over the whole column, gaps included, so a station click, a shot selection or a
 * panel row click that happened to land in the lower-left corner was swallowed
 * with no cue that anything had intercepted it.
 */
describe('ToastStack - click-through', () => {
  it('opts exactly one element - the dismiss button - back into pointer events', () => {
    // Asserted on the classes, since jsdom loads no CSS and `getComputedStyle`
    // would report the default for all of them. That still catches the regression
    // that matters: the column going back to `pointer-events-auto`, or a toast
    // body opting in and re-creating the dead zone a little smaller.
    render(<ToastStack />);
    push('working…', 'k');
    push('and another', 'k2');
    // Let the entrance settle first: a toast's first frame is deliberately at
    // opacity 0, and an invisible dismiss button opts OUT of hit-testing (see
    // "an invisible toast is not clickable" below). This case is about the steady
    // state - which single element is clickable once a toast is up.
    advance(100);

    const stack = screen.getByTestId('toast-stack');
    expect(stack.className).toContain('pointer-events-none');
    expect(stack.className).not.toContain('pointer-events-auto');

    const opted = [...stack.querySelectorAll<HTMLElement>('*')].filter((el) =>
      el.className.toString().includes('pointer-events-auto'),
    );
    expect(opted.map((el) => el.tagName)).toEqual(['BUTTON', 'BUTTON']);
    expect(opted.every((el) => el.getAttribute('aria-label')?.startsWith('Dismiss'))).toBe(true);

    // And it is still a working button.
    act(() => (opted[0] as HTMLButtonElement).click());
    advance(300);
    expect(toasts()).toHaveLength(1);
  });

  it('still pauses on hover, which is now a hit-test rather than an event', () => {
    // The pause is what `pointer-events` used to buy; losing it would be a real
    // regression, so it is asserted right next to the click-through.
    render(<ToastStack />);
    push('read me', 'k');
    advance(5500);
    expect(Number(toastFor('read me')?.style.opacity)).toBeLessThan(1);

    hoverStack();
    advance(20000);
    expect(toastFor('read me')?.style.opacity).toBe('1');
  });
});

/**
 * Opacity has no effect on hit-testing, and the stack portals into a
 * `fixed inset-0 z-[99999]` layer that sits above every dialog (`DialogWrapper`
 * is `z-50`). So the dismiss button - the only `pointer-events-auto` element in
 * the whole stack - stayed a live ~24px target in the bottom-left corner for the
 * full 180ms exit and the 1.5s fade tail, long after there was anything to see,
 * swallowing clicks aimed at a modal underneath it.
 */
describe('ToastStack - an invisible toast is not clickable', () => {
  const dismissFor = (message: string) => within(toastFor(message)!).getByRole('button');

  it('keeps the dismiss button hit-testable while the toast is visible', () => {
    render(<ToastStack />);
    push('read me', 'k');
    advance(100);

    expect(toastFor('read me')?.style.opacity).toBe('1');
    expect(dismissFor('read me').className).toContain('pointer-events-auto');

    // Still well inside the fade, but still plainly visible - and still a
    // working control, which is the half of this that must not regress.
    advance(5400);
    expect(Number(toastFor('read me')?.style.opacity)).toBeGreaterThan(0.5);
    expect(dismissFor('read me').className).toContain('pointer-events-auto');

    act(() => dismissFor('read me').click());
    advance(300);
    expect(toasts()).toHaveLength(0);
  });

  it('takes the dismiss button out of hit-testing while the toast is leaving', () => {
    render(<ToastStack />);
    push('going', 'a');
    push('staying', 'b');
    advance(100);

    act(() => dismissFor('going').click());

    // Mounted for the whole 180ms exit, at opacity 0 the entire time.
    expect(toastFor('going')?.dataset.toastLeaving).toBe('true');
    expect(toastFor('going')?.style.opacity).toBe('0');
    expect(dismissFor('going').className).not.toContain('pointer-events-auto');
    expect(dismissFor('going').className).toContain('pointer-events-none');

    // Its neighbour is untouched - this is per-toast, not a stack-wide switch.
    expect(dismissFor('staying').className).toContain('pointer-events-auto');
  });

  it('takes it out in the fade tail too, once the toast is all but gone', () => {
    render(<ToastStack />);
    push('fading out', 'k');

    // 5s dwell + 1.4s of the 1.5s fade: ~7% opacity, i.e. nothing a user could
    // see, let alone aim at. The toast is still mounted for another 100ms.
    advance(6400);
    const opacity = Number(toastFor('fading out')?.style.opacity);
    expect(opacity).toBeGreaterThan(0);
    expect(opacity).toBeLessThan(0.1);
    expect(dismissFor('fading out').className).not.toContain('pointer-events-auto');
  });

  it('re-arms the button the moment hovering brings the toast back', () => {
    render(<ToastStack />);
    push('nearly gone', 'k');
    advance(6400);
    expect(dismissFor('nearly gone').className).not.toContain('pointer-events-auto');

    // The pause is hit-tested on pointer position, so a pointer over the column
    // restores full opacity - and with it the button - before any click lands.
    hoverStack();
    expect(toastFor('nearly gone')?.style.opacity).toBe('1');
    expect(dismissFor('nearly gone').className).toContain('pointer-events-auto');
  });
});

/**
 * The stack is mounted for the whole session (App renders it unconditionally),
 * so anything it accumulates or repeats per toast, it accumulates or repeats for
 * the session - over a live WebGL map.
 */
describe('ToastStack - long-session cost', () => {
  /**
   * jsdom ships no ResizeObserver, so the component's observer path is dead in
   * these tests unless one is installed. This one only records what it is asked
   * to watch and stop watching; it never reports a resize.
   */
  function recordResizeObserver() {
    const observed: HTMLElement[] = [];
    const unobserved: HTMLElement[] = [];
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(_callback: ResizeObserverCallback) {}
        observe(el: HTMLElement) {
          observed.push(el);
        }
        unobserve(el: HTMLElement) {
          unobserved.push(el);
        }
        disconnect() {}
      },
    );
    // `useViewportHeight` watches the document element through the same global;
    // only the toast boxes are interesting here.
    const toastsOnly = (els: HTMLElement[]) => els.filter((el) => el.dataset.toastId);
    return { observed: () => toastsOnly(observed), unobserved: () => toastsOnly(unobserved) };
  }

  it('unobserves a toast element once the toast has left the stack', () => {
    const ro = recordResizeObserver();
    render(<ToastStack />);
    push('short lived', 'k');
    advance(100);

    const el = toastFor('short lived')!;
    expect(ro.observed()).toContain(el);
    expect(ro.unobserved()).toHaveLength(0);

    // Live out the dwell and the fade.
    advance(7000);
    expect(toasts()).toHaveLength(0);

    // Dropping the id from the node map is NOT enough: the observer keeps its own
    // strong reference to every element it watches, and this component never
    // unmounts, so its disconnect-on-unmount never runs. Without the unobserve
    // every toast element the session ever showed stayed alive in it.
    expect(ro.unobserved()).toContain(el);
  });

  it('leaves nothing observed after a churn of toasts', () => {
    const ro = recordResizeObserver();
    render(<ToastStack />);
    for (let i = 0; i < 4; i += 1) push(`m${i}`, `k${i}`);
    advance(7000);

    expect(toasts()).toHaveLength(0);
    expect(ro.observed()).toHaveLength(4);
    expect(ro.unobserved()).toHaveLength(4);
  });

  it('does not re-measure the mounted toasts on every frame of the dwell', () => {
    render(<ToastStack />);
    push('sits there', 'k');
    advance(200);

    const original = HTMLElement.prototype.getBoundingClientRect;
    const measured: HTMLElement[] = [];
    const spy = vi
      .spyOn(HTMLElement.prototype, 'getBoundingClientRect')
      .mockImplementation(function measureSpy(this: HTMLElement) {
        measured.push(this);
        return original.call(this);
      });

    // 40 ticks, every one of them inside the 5s dwell, where nothing rendered
    // moves. A dependency-less layout effect used to re-run
    // getBoundingClientRect over every mounted toast on every render, and the
    // tick used to force a render 20 times a second - together ~200 forced
    // pre-paint layout flushes per toast, for the frame already on screen.
    advance(2000);
    spy.mockRestore();

    expect(measured.filter((el) => el.dataset.toastId)).toHaveLength(0);
  });

  it('stops re-rendering the stack on every tick of the dwell', () => {
    render(<ToastStack />);
    push('sits there', 'k');
    advance(200);

    // The capacity report is a dependency-less effect, so it runs exactly once
    // per render of the stack: swapping the store action in turns it into a
    // render counter. The swap is itself one render, hence the clear after it.
    const real = useUIStore.getState().setToastCapacity;
    const rendered = vi.fn();
    try {
      act(() => {
        useUIStore.setState({
          setToastCapacity: (capacity: number) => {
            rendered();
            real(capacity);
          },
        });
      });
      rendered.mockClear();

      advance(2000);
      expect(rendered).not.toHaveBeenCalled();

      // The fade still animates, so what was removed is repaint of an unchanged
      // frame, not the lifecycle itself.
      advance(3000);
      expect(rendered).toHaveBeenCalled();
    } finally {
      useUIStore.setState({ setToastCapacity: real });
    }
  });

  it('registers ONE document move listener, not a pointermove/mousemove pair', () => {
    const isMove = (type: unknown) => type === 'pointermove' || type === 'mousemove';
    const added = vi.spyOn(document, 'addEventListener');
    const removed = vi.spyOn(document, 'removeEventListener');

    render(<ToastStack />);
    push('hover me', 'k');
    advance(300);

    const moves = (spy: typeof added) => spy.mock.calls.filter((call) => isMove(call[0])).length;
    // Net of the re-registrations the effect makes as the stack's geometry
    // settles. Both were registered in capture phase on document, so every mouse
    // move anywhere in the app ran the hover hit-test twice.
    const liveListeners = moves(added) - moves(removed);
    added.mockRestore();
    removed.mockRestore();

    expect(liveListeners).toBe(1);
  });
});

describe('ToastStack - pausing', () => {
  it('hover pauses dismissal and restores full opacity', () => {
    render(<ToastStack />);
    push('read me', 'k');

    // Let it start fading.
    advance(5500);
    expect(Number(toastFor('read me')?.style.opacity)).toBeLessThan(1);

    hoverStack();
    expect(toastFor('read me')?.style.opacity).toBe('1');

    // Well past its lifetime, but frozen.
    advance(20000);
    expect(toastFor('read me')).toBeDefined();
    expect(toastFor('read me')?.style.opacity).toBe('1');
  });

  it('gives the FULL dwell back on mouse-out rather than resuming mid-fade', () => {
    render(<ToastStack />);
    push('read me', 'k');

    // Deep into the fade before the pointer arrives.
    advance(5500);
    hoverStack();
    advance(20000);
    leaveStack();

    // Resuming where it stopped would have left ~1s of fade and killed it almost
    // at once. Having read it, the user has spent exactly the time the dwell
    // exists to give, so the clock starts over: still fully opaque a full dwell
    // later, gone once that dwell plus the fade has run.
    advance(4500);
    expect(toastFor('read me')?.style.opacity).toBe('1');
    advance(2500);
    expect(toastFor('read me')).toBeUndefined();
  });

  it('un-pauses itself when the pointer is gone but no mouse-out ever arrived', () => {
    // mouseleave is not guaranteed: the node under the pointer can be REMOVED
    // (a toast finishing its exit, the stack reflowing) and a node that no longer
    // exists fires no leave event. Without a self-heal `paused` sticks true and
    // nothing dismisses again - the reported "once I hover them they never go
    // away". jsdom reports no :hover, so the tick's DOM re-check should recover.
    render(<ToastStack />);
    push('stuck?', 'k');
    // Let the entrance settle - a toast's first frame is deliberately at
    // opacity 0, below its slot.
    advance(200);

    hoverStack();
    expect(toastFor('stuck?')?.style.opacity).toBe('1');

    // No mouseout is ever dispatched. Instead the pointer simply turns up
    // somewhere else, which is positive evidence it has left the stack.
    leaveStack();
    advance(8000);
    expect(toastFor('stuck?')).toBeUndefined();
  });

  it('does NOT un-pause on a pointer move that is still inside the stack', () => {
    render(<ToastStack />);
    push('read me', 'k');

    hoverStack();
    // A second move, still within the box.
    movePointerTo({ clientX: INSIDE.clientX + 10, clientY: INSIDE.clientY + 5 });

    advance(20000);
    expect(toastFor('read me')).toBeDefined();
  });

  it('keyboard focus pauses the stack too', () => {
    render(<ToastStack />);
    push('read me', 'k');

    advance(5500);
    act(() => {
      screen.getAllByRole('button')[0]?.focus();
    });

    expect(toastFor('read me')?.style.opacity).toBe('1');
    advance(20000);
    expect(toastFor('read me')).toBeDefined();
  });

  it('pausing is stack-wide, so the column cannot reshuffle under the pointer', () => {
    render(<ToastStack />);
    push('older', 'a');
    push('newer', 'b');

    hoverStack();
    advance(30000);

    expect(toasts().map((el) => el.dataset.toastKey)).toEqual(['a', 'b']);
  });
});

describe('ToastStack - manual dismiss', () => {
  it('removes only the dismissed toast', () => {
    render(<ToastStack />);
    push('keep me', 'a');
    push('drop me', 'b');

    const dismiss = within(toastFor('drop me')!).getByRole('button');
    act(() => {
      dismiss.click();
    });
    advance(300);

    expect(toastFor('drop me')).toBeUndefined();
    expect(toastFor('keep me')).toBeDefined();
  });

  it('does not leave the stack stuck paused after the last toast is dismissed', () => {
    render(<ToastStack />);
    push('only one', 'a');

    // Hover, then dismiss the last toast: the container unmounts, so nothing
    // would ever tell the stack the pointer had left it.
    hoverStack();
    act(() => {
      within(toastFor('only one')!).getByRole('button').click();
    });
    advance(300);
    expect(toasts()).toHaveLength(0);

    // The next toast must still expire on its own.
    push('a later one', 'b');
    advance(7000);
    expect(toastFor('a later one')).toBeUndefined();
  });

  it('does not leave the stack stuck paused after a keyboard dismiss while toasts remain', () => {
    render(<ToastStack />);
    push('dismiss me', 'a');
    push('keep me', 'b');

    // Focus the dismiss button (which pauses the stack), then activate it. The
    // focused element is UNMOUNTED by the removal, and browsers dispatch no
    // focusout for a removed node - so the blur that would lift the pause never
    // arrives. The empty-stack un-pause cannot help either: a toast remains.
    act(() => {
      within(toastFor('dismiss me')!).getByRole('button').focus();
    });
    act(() => {
      within(toastFor('dismiss me')!).getByRole('button').click();
    });
    advance(300);
    expect(toastFor('dismiss me')).toBeUndefined();

    // The survivor must still expire on its own...
    advance(20000);
    expect(toastFor('keep me')).toBeUndefined();

    // ...and so must the next toast, which used to arrive pre-frozen and hold
    // `pointer-events-auto` over the map for the rest of the session.
    push('a later one', 'c');
    advance(20000);
    expect(toastFor('a later one')).toBeUndefined();
  });

  it('keeps the stack paused while focus is still inside it', () => {
    render(<ToastStack />);
    push('older', 'a');
    push('newer', 'b');

    // Dismissing one toast must not lift a pause that a STILL-FOCUSED control in
    // another toast is holding - the re-check is per-tick, not per-removal.
    act(() => {
      within(toastFor('newer')!).getByRole('button').focus();
    });
    act(() => {
      within(toastFor('older')!).getByRole('button').click();
    });
    advance(20000);

    expect(toastFor('newer')).toBeDefined();
  });

  it('the dismiss control is a real button with an accessible name', () => {
    render(<ToastStack />);
    push('something happened', 'a');

    const button = within(toastFor('something happened')!).getByRole('button');
    expect(button.tagName).toBe('BUTTON');
    expect(button).toHaveAccessibleName(/something happened/);
  });
});

describe('ToastStack - accessibility', () => {
  it('announces info/success politely and error/warning assertively', () => {
    render(<ToastStack />);
    push('all good', 'a', 'success');
    push('it broke', 'b', 'error');

    expect(screen.getByRole('status')).toHaveTextContent('all good');
    expect(screen.getByRole('alert')).toHaveTextContent('it broke');
  });

  it('mounts both live regions before any toast exists', () => {
    render(<ToastStack />);
    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toBeInTheDocument();
  });

  it('does not re-announce an older message when the newest one expires', () => {
    // A live region announces whenever its text CHANGES. Publishing "the newest
    // message" therefore re-read the PREVIOUS toast on every removal: drop the
    // newest, the one before it becomes newest, the text changes, and the screen
    // reader repeats something the user has already been told - every five seconds
    // with info toasts on screen.
    render(<ToastStack />);
    push('first thing', 'a');
    advance(3000);
    push('second thing', 'b');
    expect(screen.getByRole('status')).toHaveTextContent('second thing');

    // 'first thing' expires first (it is older), which changes nothing.
    advance(4000);
    expect(toastFor('first thing')).toBeUndefined();
    expect(toastFor('second thing')).toBeDefined();
    expect(screen.getByRole('status')).toHaveTextContent('second thing');

    // Now dismiss the NEWEST while an older sibling is still on screen - the case
    // that used to revert the region's text to the older message.
    push('third thing', 'c');
    expect(screen.getByRole('status')).toHaveTextContent('third thing');
    act(() => {
      within(toastFor('third thing')!).getByRole('button').click();
    });
    advance(300);
    expect(toastFor('third thing')).toBeUndefined();
    expect(toastFor('second thing')).toBeDefined();
    expect(screen.getByRole('status')).toHaveTextContent('third thing');
  });

  it('announces a NEW toast, and a refresh in place, but never a removal', () => {
    render(<ToastStack />);
    push('working', 'job');
    expect(screen.getByRole('status')).toHaveTextContent('working');

    // Same key inside the de-dup window refreshes the toast in place (same id, new
    // revision). The message can have changed, so that IS a new announcement.
    push('working harder', 'job');
    expect(screen.getByRole('status')).toHaveTextContent('working harder');

    // The urgent region is latched independently of the polite one.
    push('it broke', 'e', 'error');
    expect(screen.getByRole('alert')).toHaveTextContent('it broke');
    expect(screen.getByRole('status')).toHaveTextContent('working harder');

    // Emptying the stack clears both, so an identical message pushed later is
    // still a text CHANGE and is therefore still announced.
    advance(20000);
    expect(toasts()).toHaveLength(0);
    expect(screen.getByRole('status').textContent).toBe('');
    push('working harder', 'job');
    expect(screen.getByRole('status')).toHaveTextContent('working harder');
  });

  it('announces BOTH toasts of a batch, oldest first', () => {
    // Two showToast calls in one event handler are ONE React update, so the
    // stack's next render already carries both. Reading only the newest item
    // announced the second and dropped the first: it is not the newest in that
    // render, and it never becomes the newest in a later one either, so nothing
    // downstream ever picks it up.
    render(<ToastStack />);
    pushBatch(['saved the section', 'a'], ['recomputed the loops', 'b']);

    expect(toasts()).toHaveLength(2);
    const text = screen.getByRole('status').textContent ?? '';
    expect(text).toContain('saved the section');
    expect(text).toContain('recomputed the loops');
    // Announced in the order they happened, not newest-first.
    expect(text.indexOf('saved the section')).toBeLessThan(text.indexOf('recomputed the loops'));
  });

  it('keeps the two regions apart when one batch fills both', () => {
    // A batch is split by urgency before it is queued, so neither region can pick
    // up the other's messages - polite stays polite, assertive stays assertive.
    render(<ToastStack />);
    pushBatch(
      ['a quiet note', 'a', 'info'],
      ['something broke', 'b', 'error'],
      ['and another note', 'c', 'success'],
    );

    const polite = screen.getByRole('status').textContent ?? '';
    const assertive = screen.getByRole('alert').textContent ?? '';
    expect(polite).toContain('a quiet note');
    expect(polite).toContain('and another note');
    expect(polite).not.toContain('something broke');
    expect(assertive).toBe('something broke');
  });

  it('announces the older of a batch even though both expire on the same tick', () => {
    // Same kind means the same lifetime, and a batch starts both clocks together,
    // so the stack goes straight from two toasts to none. There is no render in
    // which the older one is the newest, so under a newest-only rule it is never
    // announced AT ALL - not merely announced late. This records every text the
    // region ever carries across the whole life of the pair.
    render(<ToastStack />);
    const everSaid = new Set<string>();
    const record = () => everSaid.add(screen.getByRole('status').textContent ?? '');

    pushBatch(['older of the pair', 'a'], ['newer of the pair', 'b']);
    record();
    for (let i = 0; i < 140; i += 1) {
      advance(50);
      record();
    }

    expect(toasts()).toHaveLength(0);
    expect([...everSaid].some((said) => said.includes('older of the pair'))).toBe(true);
    expect([...everSaid].some((said) => said.includes('newer of the pair'))).toBe(true);
  });
});

describe('ToastStack - prefers-reduced-motion', () => {
  it('skips the animation and the fade, but keeps the same lifetime', () => {
    stubReducedMotion(true);
    render(<ToastStack />);
    push('no motion please', 'k');

    const el = toastFor('no motion please');
    expect(el?.className).not.toContain('animate-toast-in');
    expect(el?.className).not.toContain('transition-');

    // Full opacity right through what would have been the fade.
    advance(6000);
    expect(toastFor('no motion please')?.style.opacity).toBe('1');

    // Then it simply disappears at the end of its (unchanged) lifetime.
    advance(600);
    expect(toastFor('no motion please')).toBeUndefined();
  });

  it('snaps straight to its slot - no entrance travel, no exit drift', () => {
    stubReducedMotion(true);
    render(<ToastStack />);
    push('older', 'a');
    push('newer', 'b');

    // No entrance offset even on the very first frame, and full opacity at once.
    expect(offsetOf('newer')).toBe('translateY(0px)');
    expect(toastFor('newer')?.style.opacity).toBe('1');

    advance(100);
    expect(offsetOf('older')).toBe(`translateY(-${SLOT}px)`);

    // Dismissing places it nowhere new: it is simply taken away.
    act(() => {
      within(toastFor('newer')!).getByRole('button').click();
    });
    expect(offsetOf('newer')).toBe('translateY(0px)');
  });

  it('reads the preference through the legacy MediaQueryList API', () => {
    // Old WebKit exposes no addEventListener on a MediaQueryList at all. The
    // start-up read has always worked here; this pins that it still does once the
    // subscription below is wired through the deprecated pair.
    stubLegacyReducedMotion(true);
    render(<ToastStack />);
    push('no motion please', 'k');

    expect(toastFor('no motion please')?.className).not.toContain('transition-');
  });

  it('follows a live Reduce Motion change through the legacy addListener API', () => {
    // The macOS build runs in a WKWebView, and older WebKit has only the
    // deprecated addListener/removeListener pair. The compatibility branch used to
    // fall straight through to NO subscription, so flipping Reduce Motion while
    // the app was running left the animations as they were until a reload - a
    // WebKit-only gap that Chromium (and so Playwright) cannot see.
    const mq = stubLegacyReducedMotion(false);
    const view = render(<ToastStack />);
    push('moving toast', 'k');

    expect(mq.listenerCount()).toBe(1);
    expect(toastFor('moving toast')?.className).toContain('transition-');

    mq.set(true);
    expect(toastFor('moving toast')?.className).not.toContain('transition-');

    // And back again, so this is a live subscription rather than a one-way latch.
    mq.set(false);
    expect(toastFor('moving toast')?.className).toContain('transition-');

    // Unsubscribed on unmount, through the matching legacy remover.
    view.unmount();
    expect(mq.listenerCount()).toBe(0);
  });

  it('keeps the fitted capacity - reduced motion is about movement, not count', () => {
    stubReducedMotion(true);
    setViewportHeight(700);
    render(<ToastStack />);
    for (let i = 0; i < 5; i += 1) push(`m${i}`, `k${i}`);
    advance(300);

    expect(useUIStore.getState().toastCapacity).toBe(2);
    expect(toasts()).toHaveLength(2);
  });
});

describe('ToastStack - the lost-WebGL-context toast (#576 D38-LOST)', () => {
  it('a persistent toast neither fades nor leaves until it is dismissed by key', () => {
    render(<ToastStack />);
    act(() => {
      useUIStore.getState().showToast('Survey rendering unavailable (WebGL2/GPU)', 'render-context-lost', 'error', {
        persistent: true,
      });
    });
    // Far past an error's 9 s dwell + 1.5 s fade.
    advance(60000);
    expect(toastFor('Survey rendering unavailable (WebGL2/GPU)')?.style.opacity).toBe('1');

    act(() => useUIStore.getState().dismissToastByKey('render-context-lost'));
    advance(1000);
    expect(toastFor('Survey rendering unavailable (WebGL2/GPU)')).toBeUndefined();
  });

  it('its action button runs the action, then dismisses the toast', () => {
    const onAction = vi.fn();
    render(<ToastStack />);
    act(() => {
      useUIStore.getState().showToast('Survey rendering unavailable (WebGL2/GPU)', 'render-context-lost', 'error', {
        persistent: true,
        action: { label: 'Reload map', onAction, testId: 'map-context-lost-reload' },
      });
    });
    advance(100);
    const button = screen.getByTestId('map-context-lost-reload');
    expect(button.textContent).toBe('Reload map');
    expect(button.getAttribute('aria-label')).toBe('Reload map: Survey rendering unavailable (WebGL2/GPU)');
    act(() => button.click());
    expect(onAction).toHaveBeenCalledTimes(1);
    advance(1000);
    expect(toastFor('Survey rendering unavailable (WebGL2/GPU)')).toBeUndefined();
  });

  it('an ordinary toast has no action button', () => {
    render(<ToastStack />);
    push('saved', 'k', 'success');
    advance(100);
    expect(within(toastFor('saved')!).getAllByRole('button')).toHaveLength(1);
  });
});
