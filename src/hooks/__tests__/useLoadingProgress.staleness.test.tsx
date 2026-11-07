/**
 * [#486] The store's `loading.loadingId` and the Tauri `loading-progress`
 * event's `loadingId` must compare as the same kind of value. The event arrives
 * as a plain JSON number; the ids used to be declared `bigint`, and a store
 * holding `2n` discarded the event carrying `2` as "stale" (`2 !== 2n`),
 * freezing the progress UI for the rest of the load. Since #576 C6 every
 * generated loadingId is declared `number`, the shape every path delivers, and
 * the event id is compared and stored as given.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import type { LoadingProgress } from '../../types/ipc/LoadingProgress';

const listeners = vi.hoisted(
  () => [] as Array<(event: { payload: LoadingProgress }) => Promise<void> | void>
);
const unlisten = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: (event: { payload: LoadingProgress }) => void) => {
    listeners.push(cb);
    return unlisten;
  }),
}));

import { useLoadingProgress } from '../useLoadingProgress';
import { useProjectStore } from '../../store/projectStore';

function Harness() {
  useLoadingProgress();
  return null;
}

function progressPayload(overrides: Partial<LoadingProgress>): LoadingProgress {
  return {
    loadingId: 0,
    phase: 'loading_project_data',
    phaseIndex: 0,
    phaseCount: 1,
    current: null,
    total: null,
    overallPercent: 0,
    isIndeterminate: true,
    ...overrides,
  };
}

/** Fire a `loading-progress` event, mimicking Tauri's JSON decode (a plain number id). */
async function emitProgress(loadingId: number, overrides: Partial<LoadingProgress> = {}) {
  for (const cb of [...listeners]) {
    await cb({ payload: progressPayload({ loadingId, ...overrides }) });
  }
}

beforeEach(() => {
  listeners.length = 0;
  unlisten.mockClear();
  useProjectStore.setState((s) => ({
    loading: {
      ...s.loading,
      isLoading: true,
      error: undefined,
      loadingId: 0,
      clientOpId: 1,
      phase: undefined,
    },
  }));
});

describe('useLoadingProgress - event loadingId (#486, #576 C6)', () => {
  it('accepts a JSON-number event id equal to the store id', async () => {
    useProjectStore.setState((s) => ({ loading: { ...s.loading, loadingId: 2 } }));
    render(<Harness />);
    await Promise.resolve();

    await emitProgress(2, { phaseIndex: 3, overallPercent: 42 });

    expect(useProjectStore.getState().loading.overallProgress).toBe(42);
    expect(useProjectStore.getState().loading.loadingId).toBe(2);
  });

  it('rejects a genuinely stale event id, then accepts a matching one (positive control)', async () => {
    // Seed a known, non-default overallProgress so an unchanged value is
    // unambiguous - `.not.toBe(99)` alone would also pass if the listener
    // never registered at all. The second event is a positive control: it
    // proves the listener IS wired up and DOES apply a matching id, so the
    // first assertion's "unchanged" reading isn't a false negative.
    useProjectStore.setState((s) => ({
      loading: { ...s.loading, loadingId: 2, overallProgress: 7 },
    }));
    render(<Harness />);
    await Promise.resolve();

    await emitProgress(1, { phaseIndex: 3, overallPercent: 99 });
    expect(useProjectStore.getState().loading.overallProgress).toBe(7);

    await emitProgress(2, { phaseIndex: 3, overallPercent: 99 });
    expect(useProjectStore.getState().loading.overallProgress).toBe(99);
  });

  it('stores the event loadingId as the plain number it arrived as', async () => {
    // Seed the store's OWN loadingId (rather than leaving it at the initial
    // 0 "awaiting handshake" state) so this exercises the equality branch
    // directly, not the module-global `lastSeenBackendLoadingId` tracker used
    // only while awaiting the first event - that counter is never reset
    // between tests in this file, so a 0-seeded assertion would silently
    // depend on every earlier test's emitted ids staying below this one.
    useProjectStore.setState((s) => ({ loading: { ...s.loading, loadingId: 5 } }));
    render(<Harness />);
    await Promise.resolve();

    await emitProgress(5, { phaseIndex: 0 });

    expect(useProjectStore.getState().loading.loadingId).toBe(5);
    expect(typeof useProjectStore.getState().loading.loadingId).toBe('number');
  });
});
