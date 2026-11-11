/**
 * [#303] Component test - the Unwind confirmation (spec item 18's second half).
 *
 * Two things it has to get right. Cancelling must change NOTHING: the action downloads a
 * revision and replaces the working copy, so a dialog that acted on open (or on dismiss)
 * would be destructive by accident. And the two shapes of the same action have to READ
 * differently - reloading the current commit's history does not move the survey, while
 * stepping to an ancestor does - because that is the whole difference a user needs before
 * pressing it.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
}));

import SpeleoUnwindDialog from './SpeleoUnwindDialog';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

const HEXSHA = 'abcdef0123456789';

let closeDialogMock: ReturnType<typeof vi.fn>;

const open = (isCurrent: boolean, discardsUnpushed = false) => {
  useSpeleoStore.setState({ remoteProjectId: 'p1' });
  useProjectStore.setState({ project: { name: 'Cave A' } as never });
  useUIStore.setState({
    activeDialog: 'speleodb-unwind',
    dialogData: { hexsha: HEXSHA, isCurrent, discardsUnpushed },
  });
};

/** [#303] The same dialog, opened for a FORWARD chain step. */
const openForward = (isLatest: boolean) => {
  useSpeleoStore.setState({ remoteProjectId: 'p1' });
  useProjectStore.setState({ project: { name: 'Cave A' } as never });
  useUIStore.setState({
    activeDialog: 'speleodb-unwind',
    dialogData: { hexsha: HEXSHA, forward: true, isLatest, discardsUnpushed: false },
  });
};

describe('SpeleoUnwindDialog (#303)', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockResolvedValue({ local_path: '/cache/p1.stalx' });
    // Stub the post-swap refresh the shared hook runs. Left real, it feeds the restore
    // RESULT to `get_project_state` and logs two failures per test - noise that would
    // hide a genuine one. What this file is about is the IPC payload and the copy.
    closeDialogMock = vi.fn(() => useUIStore.setState({ activeDialog: null }));
    useUIStore.setState({ closeDialog: closeDialogMock, showToast: vi.fn(), setView: vi.fn() });
    useProjectStore.setState({
      refreshProjectState: vi.fn(async () => {}),
      checkProjectIntegrity: vi.fn(async () => []),
    });
    useSpeleoStore.setState({ refreshLinkStatus: vi.fn(async () => {}) });
  });

  it('words the current-commit case as loading history, not moving the survey', () => {
    open(true);
    render(<SpeleoUnwindDialog />);
    const body = screen.getByTestId('speleodb-unwind-body').textContent ?? '';
    expect(body).toContain('Load the edit history');
    expect(body).toContain('does not change');
  });

  it('words the ancestor case as moving the survey back', () => {
    open(false);
    render(<SpeleoUnwindDialog />);
    const body = screen.getByTestId('speleodb-unwind-body').textContent ?? '';
    expect(body).toContain('Check the survey out to revision');
    // ...and says what a later push does, since SpeleoDB has no revert.
    expect(screen.getByText(/records the older survey as a new revision/)).toBeInTheDocument();
  });

  it('does nothing on cancel', async () => {
    open(true);
    render(<SpeleoUnwindDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwind-cancel'));

    expect(invokeIpcMock).not.toHaveBeenCalledWith(
      'speleodb_restore_revision',
      expect.anything(),
    );
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('confirms as a Restore that KEEPS the downloaded history', async () => {
    open(true);
    render(<SpeleoUnwindDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwind-confirm-btn'));

    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith(
        'speleodb_restore_revision',
        expect.objectContaining({
          hexsha: HEXSHA,
          restore: true,
          keepUndoHistory: true,
          // The waiver: from the second step on, the copy is dirty because the PREVIOUS
          // step made it so. The backend only honours it when its chain pointer agrees.
          discardLocal: true,
        }),
      ),
    );
  });

  // [#303] The dropdown offers Unwind whenever the UNDO stack is empty, which undoing
  // everything also satisfies - and that copy is dirty, with the undone steps parked on the
  // redo stack. The backend refused it, so the entry existed only to fail once pressed. The
  // consequence now has to be on screen BEFORE the user commits, and it has to name the
  // survey changes too: a copy can be dirty for reasons no undo explains.
  it('warns about discarded work when the dialog OPENS, not when it is pressed', () => {
    open(true, true);
    render(<SpeleoUnwindDialog />);

    const warning = screen.getByTestId('speleodb-unwind-discard-warning');
    expect(warning).toBeInTheDocument();
    expect(warning).toHaveTextContent(/have not pushed/);
    // The survey changes, not just the history - under-stating this would be the bug again
    // in the cases where content really is replaced.
    expect(warning).toHaveTextContent(/discarded/);
    // Red, per the same reasoning: it is the one shape of this action that destroys work.
    expect(warning.className).toContain('text-error');
    // ...and nothing has happened yet - the warning is a statement, not an outcome.
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('stays quiet when there is no un-pushed work to lose', () => {
    open(true, false);
    render(<SpeleoUnwindDialog />);

    expect(screen.queryByTestId('speleodb-unwind-discard-warning')).not.toBeInTheDocument();
  });

  it('reports the acknowledgement only when it actually warned', async () => {
    open(true, true);
    render(<SpeleoUnwindDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwind-confirm-btn'));

    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith(
        'speleodb_restore_revision',
        expect.objectContaining({ discardUnpushed: true }),
      ),
    );
  });

  it('does not claim an acknowledgement it never showed', async () => {
    open(true, false);
    render(<SpeleoUnwindDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwind-confirm-btn'));

    // The backend re-derives the predicate; a blanket `true` here would hand it a waiver
    // for a copy the user was told nothing about.
    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith(
        'speleodb_restore_revision',
        expect.objectContaining({ discardUnpushed: false }),
      ),
    );
  });

  // [#303] The forward half of the walk. Same component, same single revision swap - the
  // direction only picks which stack the backend requires empty, and the wording.
  it('words a forward step as moving toward the latest, not back', () => {
    openForward(false);
    render(<SpeleoUnwindDialog />);
    const body = screen.getByTestId('speleodb-unwind-body').textContent ?? '';
    expect(body).toContain('next version forward');
    expect(body).not.toContain('earlier');
  });

  it('says a forward step that lands on BASE returns to the latest version', () => {
    openForward(true);
    render(<SpeleoUnwindDialog />);
    const body = screen.getByTestId('speleodb-unwind-body').textContent ?? '';
    expect(body).toContain('latest version');
    // Naming a revision here would be wrong: the point of the last step is where it
    // lands, not which sha that happens to be.
    expect(body).not.toContain(HEXSHA.slice(0, 8));
  });

  it('sends the direction so the backend guards the redo stack, not the undo stack', async () => {
    openForward(false);
    render(<SpeleoUnwindDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwind-confirm-btn'));

    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith(
        'speleodb_restore_revision',
        expect.objectContaining({
          restore: true,
          keepUndoHistory: true,
          chainForward: true,
          // A forward step is only defined while a chain pointer exists, and a pointer is
          // exactly what makes the dirty bit excusable - so it can never be discarding
          // un-pushed work, and must never claim an acknowledgement.
          discardUnpushed: false,
        }),
      ),
    );
  });

  it('leaves the backward step guarding the undo stack', async () => {
    open(true);
    render(<SpeleoUnwindDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwind-confirm-btn'));

    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith(
        'speleodb_restore_revision',
        expect.objectContaining({ chainForward: false }),
      ),
    );
  });

  it('keeps the dialog open and shows the refusal when the backend says no', async () => {
    open(true);
    invokeIpcMock.mockRejectedValueOnce(new Error('undo the remaining steps first'));
    render(<SpeleoUnwindDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwind-confirm-btn'));

    await waitFor(() => expect(screen.getByTestId('speleodb-unwind-error')).toBeInTheDocument());
    // Still open, so the user can read what happened rather than watching it vanish.
    expect(screen.getByTestId('speleodb-unwind-dialog')).toBeInTheDocument();
  });
});
