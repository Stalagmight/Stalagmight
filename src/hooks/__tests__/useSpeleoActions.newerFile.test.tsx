/**
 * [#576 NF-W9BE-1] Start editing on a project a newer Stalagmight saved is refused
 * by the backend with NEWER_FILE_READ_ONLY - the code a refused save carries. The
 * toast read "Could not start editing: Not saved: this project was saved by a newer
 * version of Stalagmight", though nothing was being saved. It now says the project
 * is from a newer version and to update; Push and Done, which do save first, keep
 * the refused-save wording.
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
import i18n from '../../i18n';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';

const NEWER = {
  code: 'NEWER_FILE_READ_ONLY',
  message: 'this project was saved by a newer version of Stalagmight',
};
const noPeriod = (s: string) => s.replace(/\.\s*$/, '');

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
    if (cmd === command) throw NEWER;
    return null;
  });

describe('useSpeleoActions: a newer-version refusal (#576 NF-W9BE-1)', () => {
  it('Start editing says the project is from a newer version, not that a save was refused', async () => {
    refuse('speleodb_start_editing');
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.startEditing();
    });

    const errors = showToast.mock.calls.filter((call) => call[2] === 'error');
    expect(errors).toHaveLength(1);
    expect(errors[0]?.[0]).toBe(
      `${i18n.t('speleodb.editBar.toasts.startFailed')}: ${noPeriod(i18n.t('banners.newerFile.lead'))}`,
    );
    expect(errors[0]?.[0]).not.toMatch(/Not saved/);
  });

  it.each([
    ['push', 'speleodb_push', 'speleodb.editBar.toasts.pushFailed'],
    ['done', 'speleodb_done', 'speleodb.editBar.toasts.doneFailed'],
  ] as const)('%s, which saves first, keeps the refused-save wording', async (verb, command, prefixKey) => {
    refuse(command);
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      if (verb === 'push') await result.current.push();
      else await result.current.done();
    });

    expect(useSpeleoStore.getState().banner).toMatchObject({
      kind: 'push_failed',
      origin: verb,
      message: `${i18n.t(prefixKey)}: ${noPeriod(i18n.t('banners.newerFile.saveRefused'))}`,
    });
  });
});
