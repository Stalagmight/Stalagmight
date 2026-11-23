/**
 * Two actions close together each keep their notice (review finding ui-9,
 * RS 7 "one notice per action"). The notices share the dedup key, and the
 * toast stack refreshes a toast of the same key in place inside its burst
 * window; for a no-dialog path the notice is the only report of the letters,
 * so a later notice joins the earlier one instead of replacing it. Outside
 * the window each action gets its own toast, as before.
 *
 * Injection (333e24ac5): the second notice replaced the first, so "keeps
 * both" fails (only the second action's letters are on screen).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import '../../i18n';
import { TOAST_DEDUP_WINDOW_MS, useUIStore } from '../../store/uiStore';
import { notifyIdentityChanges } from '../identityNotice';
import { TOAST_KEYS } from '../toastKeys';
import type { CommandResult } from '../../types/commands';

const lineName = (id: number) => (id === 1 ? 'Main' : 'Side');
const lettered = (from: string, to: string): CommandResult =>
  ({
    success: true,
    warnings: [],
    error: null,
    line_ids: [],
    identities: {
      lettered: [{ station_id: 41, line_id: 2, other_id: 12, from, to }],
      restored: [],
      unarchived: [],
      hidden: [],
    },
  }) as CommandResult;

const notices = () =>
  useUIStore
    .getState()
    .toasts.filter((t) => t.key === TOAST_KEYS.STATION_IDENTITIES && !t.leaving)
    .map((t) => t.message);

beforeEach(() => {
  vi.useFakeTimers();
  useUIStore.getState().clearToast();
});
afterEach(() => {
  useUIStore.getState().clearToast();
  vi.useRealTimers();
});

describe('identity notices of actions close together', () => {
  it('keeps both: the second joins the first', () => {
    notifyIdentityChanges(lettered('12', '12a'), lineName);
    notifyIdentityChanges(lettered('13', '13a'), lineName);
    const all = notices().join(' ');
    expect(all).toContain('Side / 12 to 12a');
    expect(all).toContain('Side / 13 to 13a');
    expect(notices()).toHaveLength(1);
  });

  it('the same notice twice in a burst is said once', () => {
    notifyIdentityChanges(lettered('12', '12a'), lineName);
    notifyIdentityChanges(lettered('12', '12a'), lineName);
    expect(notices()).toHaveLength(1);
    expect(notices()[0]!.match(/12a/g)).toHaveLength(1);
  });

  it('outside the burst window each action has its own toast', () => {
    notifyIdentityChanges(lettered('12', '12a'), lineName);
    vi.advanceTimersByTime(TOAST_DEDUP_WINDOW_MS + 1);
    notifyIdentityChanges(lettered('13', '13a'), lineName);
    expect(notices()).toHaveLength(2);
    expect(notices()[0]).not.toContain('13a');
  });
});
