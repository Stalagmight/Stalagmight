/**
 * [#576 Q-W44-7-D1] The SpeleoDB hand-off verbs the user starts, refused because the
 * project changed under them (the backend's PROJECT_CHANGED), show the short "the
 * project changed" line instead of their ordinary error, on the surface each already
 * uses: Start editing (a transient verb) a warning toast under its own key, Push and
 * Done the push-failure banner.
 *
 * `.tsx` so this runs in the jsdom tier (renderHook).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

vi.mock('../../utils/ipc');
stubGetErrorMessage(fallbackOr('mock error'));

import { useSpeleoActions } from '../useSpeleoActions';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';

const SHORT = 'The project changed before this finished.';
const CHANGED = {
  code: 'PROJECT_CHANGED',
  message: 'Starting to edit was not applied: another project is open now.',
};

let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  showToast = vi.fn();
  useProjectStore.setState({
    project: { zero_datums: [{}] } as never,
    saveProject: vi.fn(async () => ({ success: true })) as never,
  });
  useSpeleoStore.setState({
    busy: false,
    banner: null,
    applyDone: vi.fn(),
    applyPush: vi.fn(),
    bumpPoll: vi.fn(),
    clearSession: vi.fn(),
    raiseNoAnchorBanner: vi.fn(),
  });
  useUIStore.setState({ showToast });
});

const refuse = (command: string) =>
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === command) throw CHANGED;
    return null;
  });

describe('useSpeleoActions: a refusal because the project changed (#576 Q-W44-7-D1)', () => {
  it('Start editing toasts the short line as a warning, not the start-failed error', async () => {
    refuse('speleodb_start_editing');
    const { result } = renderHook(() => useSpeleoActions());

    let started: boolean | undefined;
    await act(async () => {
      started = await result.current.startEditing();
    });

    expect(started).toBe(false);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(SHORT, TOAST_KEYS.PROJECT_CHANGED, 'warning');
    expect(showToast.mock.calls.some((c) => c[2] === 'error')).toBe(false);
  });

  it.each([
    ['push', 'speleodb_push'],
    ['done', 'speleodb_done'],
  ] as const)('%s puts the short line on the push-failure banner', async (verb, command) => {
    refuse(command);
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      if (verb === 'push') await result.current.push();
      else await result.current.done();
    });

    expect(useSpeleoStore.getState().banner).toMatchObject({
      kind: 'push_failed',
      origin: verb,
      message: SHORT,
    });
  });
});
