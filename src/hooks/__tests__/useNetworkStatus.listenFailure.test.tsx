/**
 * useNetworkStatus registers two Tauri event listeners. When `listen` rejects (the
 * event API is unavailable), the hook used to keep the bare rejected promises and
 * chain `.then(fn => fn())` on them at unmount with no catch - so each failed
 * registration escaped as an unhandled rejection, once when it failed and again
 * from the unmount chain. The success-path cases pin that the listeners are still
 * wired and still released on unmount, including one that resolves after unmount.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

type ListenImpl = (event: string, handler: () => void) => Promise<() => void>;
// A plain function, not vi.fn(): a spy attaches its own handler to every promise its
// implementation returns (to record settled results), which would hide the
// registration-time unhandled rejection this file is about.
const listenState = vi.hoisted(() => ({
  impl: null as ListenImpl | null,
  events: [] as string[],
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: (event: string, handler: () => void) => {
    listenState.events.push(event);
    return listenState.impl!(event, handler);
  },
}));
vi.mock('../../utils/ipc');

import { useNetworkStatus } from '../useNetworkStatus';
import { useNetworkStore } from '../../store/networkStore';
import { invokeIpcMock } from '../../test/factories/ipc';
import { timerTurns } from '../../test/factories/async';

let unhandled: string[] = [];
const onUnhandled = (reason: unknown) => {
  unhandled.push(reason instanceof Error ? reason.message : String(reason));
};

beforeEach(() => {
  // Every IPC call answers true. Re-armed per test: the afterEach restoreAllMocks drops it.
  invokeIpcMock.mockImplementation(async () => true);
  listenState.impl = null;
  listenState.events = [];
  unhandled = [];
  process.on('unhandledRejection', onUnhandled);
  useNetworkStore.setState({ isOnline: true, tileReloadGeneration: 0 });
});

afterEach(() => {
  process.off('unhandledRejection', onUnhandled);
  vi.restoreAllMocks();
});

describe('useNetworkStatus listener registration', () => {
  it('a rejected listen() is handled, not left as an unhandled rejection, on mount or unmount', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    listenState.impl = () => Promise.reject(new Error('event API unavailable'));

    const { unmount } = renderHook(() => useNetworkStatus());
    await timerTurns(2);
    unmount();
    await timerTurns(2);

    expect(unhandled, 'unhandled rejections seen').toEqual([]);
    expect(listenState.events).toEqual(['tile-transport-error', 'tile-transport-success']);
    expect(warn).toHaveBeenCalledTimes(2);
  });

  it('still wires the tile events and releases both listeners on unmount', async () => {
    const handlers = new Map<string, () => void>();
    const unlisten = vi.fn();
    listenState.impl = async (event, handler) => {
      handlers.set(event, handler);
      return unlisten;
    };

    const { unmount } = renderHook(() => useNetworkStatus());
    await timerTurns(2);

    act(() => handlers.get('tile-transport-error')?.());
    expect(useNetworkStore.getState().isOnline).toBe(false);
    act(() => handlers.get('tile-transport-success')?.());
    expect(useNetworkStore.getState().isOnline).toBe(true);
    expect(useNetworkStore.getState().tileReloadGeneration).toBe(1);

    unmount();
    await timerTurns(2);
    expect(unlisten).toHaveBeenCalledTimes(2);
  });

  // The real unlisten is async (it awaits an IPC round trip), so a failed release
  // is a rejected promise nobody awaits - it must be handled like a failed listen().
  it('a rejected unlisten() is handled, on unmount and for a late registration', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const unlisten = () => Promise.reject(new Error('unlisten failed'));
    listenState.impl = async () => unlisten;

    const { unmount } = renderHook(() => useNetworkStatus());
    await timerTurns(2);
    unmount();
    await timerTurns(2);

    const pending: Array<(fn: () => Promise<never>) => void> = [];
    listenState.impl = () => new Promise((resolve) => { pending.push(resolve); });
    renderHook(() => useNetworkStatus()).unmount();
    pending.forEach((resolve) => resolve(unlisten));
    await timerTurns(2);

    expect(unhandled, 'unhandled rejections seen').toEqual([]);
    expect(warn).toHaveBeenCalledTimes(4);
  });

  it('releases a listener whose registration resolves only after unmount', async () => {
    const unlisten = vi.fn();
    const pending: Array<(fn: () => void) => void> = [];
    listenState.impl = () => new Promise((resolve) => { pending.push(resolve); });

    const { unmount } = renderHook(() => useNetworkStatus());
    unmount();
    expect(unlisten).not.toHaveBeenCalled();

    pending.forEach((resolve) => resolve(unlisten));
    await timerTurns(2);
    expect(unlisten).toHaveBeenCalledTimes(2);
    expect(unhandled).toEqual([]);
  });
});
