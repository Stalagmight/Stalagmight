/**
 * "Reload project" asks its discard question in Stalagmight's own dialog, not the
 * native confirm, and only an explicit "Discard changes" lets the reload discard.
 */
import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ ipc: vi.fn(), nativeConfirm: vi.fn(), replace: vi.fn() }));
vi.mock('../../../utils/ipc', () => ({ invokeIpc: mocks.ipc, getErrorMessage: String }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: mocks.nativeConfirm }));
vi.mock('../../../utils/speleoTreeReplacement', () => ({ applySpeleoTreeReplacement: mocks.replace }));

import { SpeleoReloadConfirmDialog } from '../SpeleoReloadConfirmDialog';
import { useSpeleoActions } from '../../../hooks/useSpeleoActions';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { answerSpeleoReloadConfirm, confirmSpeleoReloadDiscard, useSpeleoReloadConfirm } from '../../../utils/speleoReloadConfirm';

const expected = { session_generation: 'synthetic', project_modified_at: 'synthetic' };
beforeEach(() => {
  vi.clearAllMocks();
  useSpeleoReloadConfirm.setState({ pending: null });
  mocks.replace.mockResolvedValue(true);
  useProjectStore.setState({ project: { file_path: '/synthetic/remote.stalx' } as never, isDirty: true, projectTreeGeneration: 0,
    loadProject: vi.fn(async () => ({ success: true })) });
  useSpeleoStore.setState({ busy: false, remoteProjectId: 'remote', sessionGeneration: 0,
    link: { hasUnpushedEdits: false } as never, applyUpdate: vi.fn(), refreshLinkStatus: vi.fn() });
  mocks.ipc.mockImplementation(async (cmd) => cmd === 'speleodb_project_access_at' ? { release_context: expected } : { kind: 'up_to_date' });
});

const startReload = () => {
  const { result } = renderHook(useSpeleoActions);
  let done!: Promise<void>;
  act(() => { done = result.current.update(); });
  return done;
};

describe('SpeleoReloadConfirmDialog', () => {
  it('asks in the in-app dialog, never the native confirm', async () => {
    render(<SpeleoReloadConfirmDialog />);
    void startReload();
    const dialog = await screen.findByTestId('speleodb-reload-confirm-dialog');
    expect(dialog).toHaveTextContent('Reload Project');
    expect(screen.getByTestId('speleodb-reload-confirm-body')).toHaveTextContent('unsaved and unuploaded');
    expect(screen.getByTestId('speleodb-reload-confirm-discard')).toHaveTextContent('Discard changes');
    expect(screen.getByTestId('speleodb-reload-confirm-cancel')).toHaveTextContent('Cancel');
    expect(mocks.nativeConfirm).not.toHaveBeenCalled();
    expect(mocks.ipc).not.toHaveBeenCalled();
  });

  it('Discard changes reloads with discard acknowledged and closes the dialog', async () => {
    render(<SpeleoReloadConfirmDialog />);
    const done = startReload();
    await userEvent.click(await screen.findByTestId('speleodb-reload-confirm-discard'));
    await act(() => done);
    expect(mocks.ipc).toHaveBeenLastCalledWith('speleodb_update', { discardChanges: true, expected });
    expect(screen.queryByTestId('speleodb-reload-confirm-dialog')).not.toBeInTheDocument();
  });

  it.each([
    ['Cancel', async () => userEvent.click(screen.getByTestId('speleodb-reload-confirm-cancel'))],
    ['Escape', async () => userEvent.keyboard('{Escape}')],
    ['the close button', async () => userEvent.click(screen.getByLabelText('Close dialog'))],
  ])('%s keeps the work: no network call, nothing discarded', async (_label, dismiss) => {
    render(<SpeleoReloadConfirmDialog />);
    const done = startReload();
    await screen.findByTestId('speleodb-reload-confirm-dialog');
    await dismiss();
    await act(() => done);
    expect(mocks.ipc).not.toHaveBeenCalled();
    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(screen.queryByTestId('speleodb-reload-confirm-dialog')).not.toBeInTheDocument();
  });

  it('withdraws the question (keeping the work) when its project closes', async () => {
    render(<SpeleoReloadConfirmDialog />);
    const answer = confirmSpeleoReloadDiscard();
    await screen.findByTestId('speleodb-reload-confirm-dialog');
    act(() => useProjectStore.setState({ project: null }));
    await expect(answer).resolves.toBe(false);
    await waitFor(() => expect(screen.queryByTestId('speleodb-reload-confirm-dialog')).not.toBeInTheDocument());
  });

  it('a second request supersedes the first, which resolves as keep', async () => {
    const first = confirmSpeleoReloadDiscard();
    const second = confirmSpeleoReloadDiscard();
    await expect(first).resolves.toBe(false);
    expect(useSpeleoReloadConfirm.getState().pending).not.toBeNull();
    answerSpeleoReloadConfirm(true);
    await expect(second).resolves.toBe(true);
  });
});
