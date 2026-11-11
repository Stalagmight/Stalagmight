/**
 * sdk#3: an upload over SpeleoDB's size limit leaves the undo history out of the
 * uploaded copy (the survey data still uploads in full). The backend reports it on
 * the landed result, and the push / unlock success toast says so instead - one
 * event, one surface - as a warning.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'mock error',
}));

import { useSpeleoActions } from '../useSpeleoActions';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import i18n from '../../i18n';

const showToast = vi.fn();

beforeEach(() => {
  invokeIpcMock.mockReset();
  showToast.mockReset();
  useProjectStore.setState({
    project: { file_path: '/remote.stalx', zero_datums: [{}] } as never,
    saveProject: vi.fn(async () => ({ success: true })) as never,
  });
  useSpeleoStore.setState({
    busy: false,
    applyDone: vi.fn(),
    applyPush: vi.fn(),
    clearSession: vi.fn(),
    raiseNoAnchorBanner: vi.fn(),
    link: { hasUnpushedEdits: true } as never,
  });
  useUIStore.setState({ showToast });
});

describe('useSpeleoActions - an upload without its undo history', () => {
  it('a push that left the history out warns instead of plain success', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'pushed', undo_history_omitted: true });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push('m');
    });

    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      i18n.t('speleodb.editBar.toasts.pushedWithoutHistory'),
      'speleodb-pushed',
      'warning',
    );
  });

  it('an unlock that left the history out warns instead of plain success', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'released', undo_history_omitted: true });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.done();
    });

    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      i18n.t('speleodb.editBar.toasts.releasedWithoutHistory'),
      'speleodb-released',
      'warning',
    );
  });

  it('an ordinary landed push keeps its success toast', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'pushed' });
    const { result } = renderHook(() => useSpeleoActions());

    await act(async () => {
      await result.current.push('m');
    });

    expect(showToast).toHaveBeenCalledWith(
      i18n.t('speleodb.editBar.toasts.pushed'),
      'speleodb-pushed',
      'success',
    );
  });
});
