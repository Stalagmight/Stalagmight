/**
 * [#576 RV5-PI-3] The internal-error banner does not depend on catching the one
 * push event: once its listener is registered, the hook asks the backend
 * (`get_internal_error_status`) for a report it may have missed - a crash during
 * startup, before the listener existed, or a webview reload after the event went
 * out. No event is fired in these tests; only the pull answers.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

const state = vi.hoisted(() => ({
  status: null as { cell: string } | null,
  asked: 0,
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
}));
vi.mock('../../utils/ipc', () => ({
  invokeIpc: (cmd: string) => {
    if (cmd !== 'get_internal_error_status') return Promise.reject(new Error(`unexpected ${cmd}`));
    state.asked += 1;
    return Promise.resolve(state.status);
  },
}));

import { useInternalErrorEvent } from '../useInternalErrorEvent';
import { useInternalErrorStore } from '../../store/internalErrorStore';
import { actNextTimer } from '../../test/factories/act';

beforeEach(() => {
  state.status = null;
  state.asked = 0;
  useInternalErrorStore.setState({ raised: false, cell: null });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useInternalErrorEvent pull path (#576 RV5-PI-3)', () => {
  it('raises the banner for a crash reported before the listener existed', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    state.status = { cell: 'project' };
    renderHook(() => useInternalErrorEvent());
    await actNextTimer();
    expect(state.asked).toBe(1);
    expect(useInternalErrorStore.getState()).toMatchObject({ raised: true, cell: 'project' });
  });

  it('raises nothing when the backend has nothing to report', async () => {
    renderHook(() => useInternalErrorEvent());
    await actNextTimer();
    expect(state.asked).toBe(1);
    expect(useInternalErrorStore.getState().raised).toBe(false);
  });
});
