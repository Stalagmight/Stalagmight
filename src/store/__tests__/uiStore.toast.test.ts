/**
 * The toast STACK model in uiStore: ordering, the visible-count cap, same-key
 * refresh, kind defaulting, and the exit/removal seam.
 *
 * The store deliberately runs no timers - the dwell/fade clock lives in
 * ToastStack, which is where hover/focus pausing is observable - so everything
 * here is synchronous and time-independent.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

import { useUIStore, MAX_VISIBLE_TOASTS, TOAST_DEDUP_WINDOW_MS } from '../uiStore';
import { MIN_VISIBLE_TOASTS } from '../../utils/toastCapacity';

const messages = () => useUIStore.getState().toasts.map((t) => t.message);
const live = () => useUIStore.getState().toasts.filter((t) => !t.leaving);

beforeEach(() => {
  useUIStore.setState({ toasts: [], toastCapacity: MAX_VISIBLE_TOASTS });
});

describe('toast stack - ordering', () => {
  it('appends newest last (the visual bottom of the stack)', () => {
    const { showToast } = useUIStore.getState();
    showToast('first', 'a', 'info');
    showToast('second', 'b', 'info');
    showToast('third', 'c', 'info');

    expect(messages()).toEqual(['first', 'second', 'third']);
  });

  it('gives every toast a distinct id', () => {
    const { showToast } = useUIStore.getState();
    showToast('one', 'a', 'info');
    showToast('two', 'b', 'info');

    const ids = useUIStore.getState().toasts.map((t) => t.id);
    expect(new Set(ids).size).toBe(2);
  });
});

describe('toast stack - the visible cap', () => {
  it(`keeps at most ${MAX_VISIBLE_TOASTS} live toasts`, () => {
    const { showToast } = useUIStore.getState();
    for (let i = 0; i < MAX_VISIBLE_TOASTS; i += 1) showToast(`m${i}`, `k${i}`, 'info');

    expect(live()).toHaveLength(MAX_VISIBLE_TOASTS);
  });

  it('evicts the OLDEST when one arrives past the cap', () => {
    const { showToast } = useUIStore.getState();
    for (let i = 0; i < MAX_VISIBLE_TOASTS; i += 1) showToast(`m${i}`, `k${i}`, 'info');
    showToast('overflow', 'k-overflow', 'info');

    expect(live().map((t) => t.message)).toEqual([
      ...Array.from({ length: MAX_VISIBLE_TOASTS - 1 }, (_, i) => `m${i + 1}`),
      'overflow',
    ]);
  });

  it('marks the evicted one leaving rather than dropping it, so it can animate out', () => {
    const { showToast } = useUIStore.getState();
    for (let i = 0; i < MAX_VISIBLE_TOASTS; i += 1) showToast(`m${i}`, `k${i}`, 'info');
    showToast('overflow', 'k-overflow', 'info');

    const evicted = useUIStore.getState().toasts.find((t) => t.message === 'm0');
    expect(evicted).toBeDefined();
    expect(evicted?.leaving).toBe(true);
  });

  it('never lets the array grow without bound when nothing reaps the leavers', () => {
    const { showToast } = useUIStore.getState();
    for (let i = 0; i < 50; i += 1) showToast(`m${i}`, `k${i}`, 'info');

    expect(useUIStore.getState().toasts.length).toBeLessThanOrEqual(MAX_VISIBLE_TOASTS + 4);
    expect(live()).toHaveLength(MAX_VISIBLE_TOASTS);
  });
});

describe('toast stack - the fitted capacity', () => {
  it('starts at the ceiling, so the store is sane with no stack mounted to measure', () => {
    expect(useUIStore.getState().toastCapacity).toBe(MAX_VISIBLE_TOASTS);
  });

  it.each([
    [-1, MIN_VISIBLE_TOASTS],
    [0, MIN_VISIBLE_TOASTS],
    [1, MIN_VISIBLE_TOASTS],
    [3, 3],
    [MAX_VISIBLE_TOASTS, MAX_VISIBLE_TOASTS],
    [99, MAX_VISIBLE_TOASTS],
  ])('clamps a reported capacity of %s to %s', (reported, expected) => {
    useUIStore.getState().setToastCapacity(reported);
    expect(useUIStore.getState().toastCapacity).toBe(expected);
  });

  it('honours the reported capacity on push', () => {
    const { showToast, setToastCapacity } = useUIStore.getState();
    setToastCapacity(2);
    for (let i = 0; i < 4; i += 1) showToast(`m${i}`, `k${i}`, 'info');

    expect(live().map((t) => t.message)).toEqual(['m2', 'm3']);
  });

  it('evicts immediately when the window shrinks, rather than hiding a live toast', () => {
    const { showToast, setToastCapacity } = useUIStore.getState();
    for (let i = 0; i < 5; i += 1) showToast(`m${i}`, `k${i}`, 'info');
    expect(live()).toHaveLength(5);

    setToastCapacity(2);

    expect(live().map((t) => t.message)).toEqual(['m3', 'm4']);
    // The evicted ones animate out rather than blinking away.
    expect(useUIStore.getState().toasts.filter((t) => t.leaving)).toHaveLength(3);
  });

  it('does not evict when the window grows', () => {
    const { showToast, setToastCapacity } = useUIStore.getState();
    setToastCapacity(2);
    showToast('a', 'a', 'info');
    showToast('b', 'b', 'info');

    setToastCapacity(5);

    expect(live().map((t) => t.message)).toEqual(['a', 'b']);
  });

  it('is a no-op when the number has not moved, so reporting it every frame is free', () => {
    const { showToast, setToastCapacity } = useUIStore.getState();
    showToast('a', 'a', 'info');
    const before = useUIStore.getState().toasts;

    setToastCapacity(MAX_VISIBLE_TOASTS);

    expect(useUIStore.getState().toasts).toBe(before);
  });
});

describe('toast stack - same-key de-duplication', () => {
  it('refreshes the existing toast instead of stacking a duplicate', () => {
    const { showToast } = useUIStore.getState();
    showToast('first wording', 'dup', 'info');
    showToast('second wording', 'dup', 'info');

    expect(messages()).toEqual(['second wording']);
  });

  it('keeps the same id (no remount) but bumps revision, which restarts the timer', () => {
    const { showToast } = useUIStore.getState();
    showToast('first', 'dup', 'info');
    const before = useUIStore.getState().toasts[0];
    showToast('second', 'dup', 'info');
    const after = useUIStore.getState().toasts[0];

    expect(after?.id).toBe(before?.id);
    expect(after?.revision).toBe((before?.revision ?? 0) + 1);
  });

  it('moves the refreshed toast to the newest slot', () => {
    const { showToast } = useUIStore.getState();
    showToast('old', 'dup', 'info');
    showToast('other', 'other', 'info');
    showToast('refreshed', 'dup', 'info');

    expect(messages()).toEqual(['other', 'refreshed']);
  });

  it('lets the newer kind win on refresh', () => {
    const { showToast } = useUIStore.getState();
    showToast('trying', 'dup', 'info');
    showToast('it failed', 'dup', 'error');

    expect(useUIStore.getState().toasts[0]?.kind).toBe('error');
  });

  it('does NOT refresh a toast that is already leaving - that one is on its way out', () => {
    const { showToast, dismissToast } = useUIStore.getState();
    showToast('first', 'dup', 'info');
    const id = useUIStore.getState().toasts[0]?.id ?? -1;
    dismissToast(id);

    showToast('second', 'dup', 'info');

    expect(messages()).toEqual(['first', 'second']);
    expect(live().map((t) => t.message)).toEqual(['second']);
  });

  it('different keys never merge', () => {
    const { showToast } = useUIStore.getState();
    showToast('same words', 'a', 'info');
    showToast('same words', 'b', 'info');

    expect(useUIStore.getState().toasts).toHaveLength(2);
  });
});

/**
 * Fake timers that also fake the MONOTONIC clock.
 *
 * The de-duplication window is measured with `performance.now()`, not
 * `Date.now()` (a wall clock can step backwards - see `ToastItem.refreshedAt`),
 * and vitest's default fake-timer set does NOT include `performance`. Left out,
 * `advanceTimersByTime` would move the wall clock while the monotonic one stood
 * still, and every repeat below would collapse as if it were a burst.
 */
const useMonotonicClock = () =>
  vi.useFakeTimers({
    toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date', 'performance'],
  });

describe('toast stack - the de-duplication WINDOW', () => {
  /**
   * Found by using the app: the user toggled the edit lock repeatedly and only
   * ever saw two toasts. Matching a key for the toast's whole visible life made
   * each key a single slot, so lock/unlock/lock/unlock had every new toast steal
   * the previous one of its kind. Those are four distinct things the user did.
   */
  it('stacks a repeat that arrives after the window instead of stealing the old one', () => {
    useMonotonicClock();
    try {
      const { showToast } = useUIStore.getState();
      showToast('Editing', 'speleodb-editing', 'success');
      vi.advanceTimersByTime(TOAST_DEDUP_WINDOW_MS + 1);
      showToast('Editing', 'speleodb-editing', 'success');

      expect(live()).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('reproduces the lock/unlock/lock/unlock sequence as four toasts', () => {
    useMonotonicClock();
    try {
      const { showToast } = useUIStore.getState();
      for (let i = 0; i < 2; i += 1) {
        showToast('Editing', 'speleodb-editing', 'success');
        vi.advanceTimersByTime(TOAST_DEDUP_WINDOW_MS + 1);
        showToast('Lock released', 'speleodb-released', 'success');
        vi.advanceTimersByTime(TOAST_DEDUP_WINDOW_MS + 1);
      }

      expect(live()).toHaveLength(4);
      expect(messages()).toEqual(['Editing', 'Lock released', 'Editing', 'Lock released']);
    } finally {
      vi.useRealTimers();
    }
  });

  it('still collapses a genuine burst inside the window', () => {
    useMonotonicClock();
    try {
      const { showToast } = useUIStore.getState();
      showToast('Stations are hidden', 'stations-hidden', 'warning');
      vi.advanceTimersByTime(50);
      showToast('Stations are hidden', 'stations-hidden', 'warning');
      vi.advanceTimersByTime(50);
      showToast('Stations are hidden', 'stations-hidden', 'warning');

      expect(live()).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  /**
   * The window used to be measured on the WALL clock, which can step backwards
   * (an NTP correction, a user fixing the system time). A negative age passes
   * `< TOAST_DEDUP_WINDOW_MS` exactly as happily as a fresh one, so every repeat
   * of a key collapsed into the toast already on screen for as long as the skew
   * lasted - however far apart the events really were.
   */
  it('is unaffected by a wall clock that steps BACKWARDS', () => {
    useMonotonicClock();
    try {
      const { showToast } = useUIStore.getState();
      showToast('Editing', 'speleodb-editing', 'success');
      vi.advanceTimersByTime(TOAST_DEDUP_WINDOW_MS + 1);
      // Wall clock jumps an hour into the past; the monotonic clock does not
      // (sinon keeps `performance` on its own timeline across setSystemTime).
      vi.setSystemTime(Date.now() - 60 * 60 * 1000);
      showToast('Editing', 'speleodb-editing', 'success');

      expect(live()).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps a sustained burst collapsed, because each refresh restarts the window', () => {
    useMonotonicClock();
    try {
      const { showToast } = useUIStore.getState();
      for (let i = 0; i < 20; i += 1) {
        showToast('spam', 'flood', 'warning');
        vi.advanceTimersByTime(TOAST_DEDUP_WINDOW_MS - 100);
      }

      expect(live()).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('toast stack - kinds', () => {
  it('defaults to info at runtime, even though `kind` is now type-mandatory', () => {
    // `kind` is required in the type (docs/notifications.md: never rely on the
    // default), but the runtime implementation still defaults defensively - a
    // stray call that slips past the type system should not crash or render an
    // undefined-kind toast.
    // @ts-expect-error - intentionally calling without the now-required `kind`.
    useUIStore.getState().showToast('neutral', 'k');
    expect(useUIStore.getState().toasts[0]?.kind).toBe('info');
  });

  it.each(['success', 'warning', 'error', 'info'] as const)('carries an explicit %s kind', (kind) => {
    useUIStore.getState().showToast('m', 'k', kind);
    expect(useUIStore.getState().toasts[0]?.kind).toBe(kind);
  });
});

describe('toast stack - removal', () => {
  it('dismissToast starts the exit without removing the toast yet', () => {
    const { showToast, dismissToast } = useUIStore.getState();
    showToast('a', 'a', 'info');
    showToast('b', 'b', 'info');
    const id = useUIStore.getState().toasts[0]?.id ?? -1;

    dismissToast(id);

    expect(useUIStore.getState().toasts).toHaveLength(2);
    expect(live().map((t) => t.message)).toEqual(['b']);
  });

  it('removeToast drops exactly one toast', () => {
    const { showToast, removeToast } = useUIStore.getState();
    showToast('a', 'a', 'info');
    showToast('b', 'b', 'info');
    showToast('c', 'c', 'info');
    const id = useUIStore.getState().toasts[1]?.id ?? -1;

    removeToast(id);

    expect(messages()).toEqual(['a', 'c']);
  });

  it('clearToast empties the stack', () => {
    const { showToast, clearToast } = useUIStore.getState();
    showToast('a', 'a', 'info');
    showToast('b', 'b', 'info');

    clearToast();

    expect(useUIStore.getState().toasts).toEqual([]);
  });
});

describe('toast stack - persistent toasts and the cap (#576 RV4-FE-MF1)', () => {
  // The lost-WebGL-context "Reload map" toast is persistent and raised ONCE per
  // loss; if the cap evicted it, nothing would offer the reload again.
  const persistentLive = () =>
    useUIStore.getState().toasts.find((t) => t.key === 'persist' && !t.leaving);

  it.each([MAX_VISIBLE_TOASTS, MIN_VISIBLE_TOASTS])(
    'keeps a persistent toast live through a burst of ordinary ones (capacity %s)',
    (capacity) => {
      const { showToast, setToastCapacity } = useUIStore.getState();
      setToastCapacity(capacity);
      showToast('reload me', 'persist', 'error', { persistent: true });
      for (let i = 0; i < capacity + 6; i += 1) showToast(`m${i}`, `k${i}`, 'info');

      expect(persistentLive()?.message).toBe('reload me');
      // It still counts toward the cap: the ordinary ones fill the rest.
      expect(live()).toHaveLength(capacity);
    },
  );

  it('keeps it when the window shrinks, evicting the ordinary ones instead', () => {
    const { showToast, setToastCapacity } = useUIStore.getState();
    showToast('reload me', 'persist', 'error', { persistent: true });
    for (let i = 0; i < 4; i += 1) showToast(`m${i}`, `k${i}`, 'info');

    setToastCapacity(MIN_VISIBLE_TOASTS);

    expect(live().map((t) => t.message)).toEqual(['reload me', 'm3']);
  });

  it('survives the hard array ceiling while the leavers pile up unreaped', () => {
    const { showToast } = useUIStore.getState();
    showToast('reload me', 'persist', 'error', { persistent: true });
    for (let i = 0; i < 50; i += 1) showToast(`m${i}`, `k${i}`, 'info');

    expect(persistentLive()).toBeDefined();
    expect(useUIStore.getState().toasts.length).toBeLessThanOrEqual(MAX_VISIBLE_TOASTS + 4);
  });

  it('still goes when only persistent toasts are left to evict', () => {
    const { showToast, setToastCapacity } = useUIStore.getState();
    setToastCapacity(MIN_VISIBLE_TOASTS);
    showToast('p0', 'p0', 'error', { persistent: true });
    showToast('p1', 'p1', 'error', { persistent: true });
    showToast('p2', 'p2', 'error', { persistent: true });

    expect(live().map((t) => t.message)).toEqual(['p1', 'p2']);
  });
});
