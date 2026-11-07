/**
 * [#576 A11] The backend's one-shot `internal-error` event raises a persistent
 * banner: it appears when the event arrives (and not before), says to save and
 * restart, has no Dismiss, and a second event changes nothing.
 *
 * Real i18n, so the copy the user reads is checked. `.tsx` so vitest runs it under
 * jsdom.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, renderHook, screen } from '@testing-library/react';

type Handler = (event: { payload: { cell: string } }) => void;
const listenState = vi.hoisted(() => ({
  handlers: new Map<string, Handler>(),
  unlistened: 0,
  fail: false,
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: (event: string, handler: Handler) => {
    if (listenState.fail) return Promise.reject(new Error('event API unavailable'));
    listenState.handlers.set(event, handler);
    return Promise.resolve(() => {
      listenState.unlistened += 1;
    });
  },
}));

import '../../../i18n';
import { InternalErrorBanner } from '../InternalErrorBanner';
import { useInternalErrorEvent } from '../../../hooks/useInternalErrorEvent';
import { useInternalErrorStore } from '../../../store/internalErrorStore';
import { IPC_EVENTS } from '../../../utils/ipcEvents';
import { actNextTimer } from '../../../test/factories/act';

beforeEach(() => {
  listenState.handlers.clear();
  listenState.unlistened = 0;
  listenState.fail = false;
  useInternalErrorStore.setState({ raised: false, cell: null });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('InternalErrorBanner (#576 A11)', () => {
  it('is absent until the backend reports an internal error, then persists', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const { unmount } = renderHook(() => useInternalErrorEvent());
    render(<InternalErrorBanner />);
    await actNextTimer();

    expect(screen.queryByTestId('internal-error-banner')).not.toBeInTheDocument();
    expect(IPC_EVENTS.INTERNAL_ERROR).toBe('internal-error');
    const handler = listenState.handlers.get(IPC_EVENTS.INTERNAL_ERROR);
    expect(handler, 'the hook listens for the declared event').toBeDefined();

    act(() => handler!({ payload: { cell: 'project' } }));
    const banner = screen.getByTestId('internal-error-banner');
    expect(banner).toHaveAttribute('role', 'alert');
    expect(banner).toHaveTextContent('Internal error - save your work and restart Stalagmight.');
    expect(banner).toHaveTextContent('saving still works');
    expect(banner.querySelector('button'), 'no Dismiss: it lasts until the restart').toBeNull();

    // A second report (another poisoned lock) changes nothing.
    act(() => handler!({ payload: { cell: 'undo_stack' } }));
    expect(useInternalErrorStore.getState().cell).toBe('project');
    expect(screen.getAllByTestId('internal-error-banner')).toHaveLength(1);

    unmount();
    expect(listenState.unlistened).toBe(1);
  });

  it('a rejected registration is reported, not left unhandled', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    listenState.fail = true;

    renderHook(() => useInternalErrorEvent());
    await actNextTimer();

    expect(warn).toHaveBeenCalledWith(
      '[useInternalErrorEvent] internal-error listener not available:',
      expect.any(Error),
    );
    expect(useInternalErrorStore.getState().raised).toBe(false);
  });
});
