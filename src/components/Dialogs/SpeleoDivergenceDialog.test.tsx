/**
 * F4R3-locks-push-7: the divergence prompt's "Keep my local version" re-runs the
 * push (or Start editing), which is refused again for as long as another upload's
 * revision is SpeleoDB's latest - another account's, or, since R3, another session
 * of this one. Its help used to promise an upload that resolves the conflict; it
 * now says it comes back here and points at the way out that keeps the edits.
 *
 * Demo item 2: Take the SpeleoDB version keeps the lock, and its confirmation says
 * what happens to it.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'e',
}));

import SpeleoDivergenceDialog from './SpeleoDivergenceDialog';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

beforeEach(() => {
  invokeIpcMock.mockReset();
  useSpeleoStore.setState({
    divergence: 'push',
    link: {
      permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: 'diver@example.com', lockedByOther: false,
      hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'conflict', noAccess: false,
      serverError: null,
    },
  });
  useUIStore.setState({ activeDialog: 'speleodb-divergence', dialogData: {} });
});

describe('SpeleoDivergenceDialog', () => {
  // Injection: the old help ("Upload your version to SpeleoDB to resolve the
  // conflict.") fails the first two expectations.
  it('says Keep my local version comes back here while SpeleoDB has a newer revision', () => {
    render(<SpeleoDivergenceDialog />);
    const keep = screen.getByTestId('speleodb-divergence-keep-local');
    expect(keep).toHaveTextContent('this comes back here');
    expect(keep).toHaveTextContent('save them as an unmanaged file');
    expect(keep).not.toHaveTextContent('to resolve the conflict');
  });

  // Demo item 2: taking the SpeleoDB version used to hand the account's lock back,
  // leaving another session of the account editing with no lock on the server.
  // It now releases nothing, and the confirmation says what happens to the lock.
  // Injection: drop the lock line from the confirmation and both cases fail.
  it('says an editing session keeps the lock and goes on editing the SpeleoDB version', async () => {
    render(<SpeleoDivergenceDialog />);
    await userEvent.click(screen.getByTestId('speleodb-divergence-take-remote'));
    const lock = screen.getByTestId('speleodb-take-remote-lock');
    expect(lock).toHaveTextContent('You keep the write lock');
    expect(lock).toHaveTextContent('go on editing');
    expect(lock).not.toHaveTextContent('read-only');
  });

  it('says a read-only session stays read-only with the lock left as it is', async () => {
    useSpeleoStore.setState({
      link: { ...useSpeleoStore.getState().link!, editState: 'read-only' },
    });
    render(<SpeleoDivergenceDialog />);
    await userEvent.click(screen.getByTestId('speleodb-divergence-take-remote'));
    const lock = screen.getByTestId('speleodb-take-remote-lock');
    expect(lock).toHaveTextContent('write lock is left as it is');
    expect(lock).toHaveTextContent('read-only');
  });

  it('still routes an editing session to the upload dialog', async () => {
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog });
    render(<SpeleoDivergenceDialog />);
    await userEvent.click(screen.getByTestId('speleodb-divergence-keep-local'));
    expect(openDialog).toHaveBeenCalledWith('speleodb-done', { mode: 'push' });
    expect(useSpeleoStore.getState().divergence).toBeNull();
  });
});
