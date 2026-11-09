/**
 * [#303] The confirmation that stands between "I looked at an old version" and "I
 * published a revert".
 *
 * The bug it exists for was not subtle once seen: the close prompt read the dirty bit,
 * said "You have unpushed changes", and made "Push, release & close" its primary action -
 * so looking at history and closing the project recorded the older survey over the newer
 * one for everyone, in two clicks, under a reassurance that the work was safe.
 *
 * So what matters here is not that the dialog renders. It is that the SAFE action is the
 * default, the destructive one is named for what it does, and the push cannot happen
 * without the acknowledgement.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown, f?: string) => (e as { message?: string })?.message ?? f ?? 'e',
}));

// [#303] The two exits this dialog can finish. Stubbed (the rest of utils/quit is real,
// `holdsWriteLock` included) so the cases below can assert that the routes with NO exit
// waiting still reach neither of them.
const performQuitMock = vi.hoisted(() => vi.fn());
const handBackProjectMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('../../utils/quit', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/quit')>()),
  performQuit: performQuitMock,
  handBackProject: handBackProjectMock,
}));

import SpeleoUnwoundPushDialog from './SpeleoUnwoundPushDialog';
import { reserveProjectRequest } from '../../utils/projectTransition';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

const open = () => {
  useUIStore.setState({
    activeDialog: 'speleodb-unwound-push',
    dialogData: { message: 'a commit message' },
  });
};

describe('SpeleoUnwoundPushDialog (#303)', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    performQuitMock.mockReset();
    handBackProjectMock.mockClear();
    invokeIpcMock.mockResolvedValue({ kind: 'editing' });
    useUIStore.setState({
      closeDialog: vi.fn(() => useUIStore.setState({ activeDialog: null })),
      openDialog: vi.fn(),
      showToast: vi.fn(),
    });
    useSpeleoStore.setState({
      remoteProjectId: 'p1',
      refreshLinkStatus: vi.fn(async () => {}),
      banner: null,
    });
    useProjectStore.setState({
      refreshProjectState: vi.fn(async () => {}),
      checkProjectIntegrity: vi.fn(async () => []),
      isDirty: true,
    });
  });

  it('says the copy is older rather than calling it unsaved work', () => {
    open();
    render(<SpeleoUnwoundPushDialog />);

    expect(screen.getByTestId('speleodb-unwound-push-body')).toHaveTextContent(/older version/);
    // The consequence is about everyone else, which is what makes it worth a stop.
    expect(screen.getByTestId('speleodb-unwound-push-warning')).toHaveTextContent(
      /over the newer one/,
    );
  });

  it('makes discarding the primary action and pushing the marked one', () => {
    open();
    render(<SpeleoUnwoundPushDialog />);

    const discard = screen.getByTestId('speleodb-unwound-push-discard');
    const pushAnyway = screen.getByTestId('speleodb-unwound-push-push-anyway');
    // The safe way out is what the dialog defaults to - the inverse of the close prompt
    // that caused this.
    expect(discard).toHaveAttribute('data-primary-action');
    expect(pushAnyway).not.toHaveAttribute('data-primary-action');
    // ...and the destructive one says what it does, rather than "Push".
    expect(pushAnyway).toHaveTextContent(/older version/);
  });

  it('discards by returning to the tip WITH the lock, so work can continue', async () => {
    open();
    render(<SpeleoUnwoundPushDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-discard'));

    await waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_discard_changes', { keepLock: true, acknowledged: true }));
    // Never a push: that is the whole point of this branch.
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_done', expect.anything());
  });

  it('carries the acknowledgement when the user insists, or it would loop', async () => {
    open();
    render(<SpeleoUnwoundPushDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));

    // `useSpeleoActions.done` re-raises this dialog unless told the user was shown it,
    // so without the flag "Upload this older version" would do nothing but reopen.
    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith(
        'speleodb_done',
        expect.objectContaining({ message: 'a commit message' }),
      ),
    );
  });

  it('keeps the message and retry surface when the release attempt fails', async () => {
    invokeIpcMock
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce({ kind: 'released' });
    open();
    render(<SpeleoUnwoundPushDialog />);

    const pushAnyway = screen.getByTestId('speleodb-unwound-push-push-anyway');
    await userEvent.click(pushAnyway);

    const notice = await screen.findByTestId('speleodb-push-failure-inline');
    expect(notice).toHaveTextContent('offline');
    expect(useUIStore.getState().activeDialog).toBe('speleodb-unwound-push');
    expect(pushAnyway).toBeEnabled();

    await userEvent.click(pushAnyway);
    await waitFor(() => expect(useUIStore.getState().activeDialog).toBeNull());
    const doneCalls = invokeIpcMock.mock.calls.filter(([command]) => command === 'speleodb_done');
    expect(doneCalls).toEqual([
      [
        'speleodb_done',
        expect.objectContaining({ message: 'a commit message', acceptOlderVersion: true }),
      ],
      [
        'speleodb_done',
        expect.objectContaining({ message: 'a commit message', acceptOlderVersion: true }),
      ],
    ]);
  });

  it('acknowledges an explicit push without releasing the lock', async () => {
    useUIStore.setState({ activeDialog: 'speleodb-unwound-push', dialogData: { message: 'push old', mode: 'push' } });
    invokeIpcMock.mockResolvedValue({ kind: 'pushed' });
    render(<SpeleoUnwoundPushDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));

    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_push', {
        message: 'push old',
        acceptOlderVersion: true,
      }),
    );
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_done', expect.anything());
  });

  it('offers both lock outcomes on the explicit Discard action', async () => {
    // Discarding the copy and deciding the lock are two questions. Someone resetting to
    // start again wants to keep editing; someone abandoning a session wants to hand the
    // project back. Folding them together forces the second user to discard, then go
    // hunting for Release.
    const { default: SpeleoDiscardChangesDialog } = await import('./SpeleoDiscardChangesDialog');
    // Both outcomes are only on offer to a session that HOLDS the lock, so the fixture
    // has to say so - `holdsWriteLock` is editState === 'editing' && !forceEditing.
    useSpeleoStore.setState({
      link: {
        unwound: false,
        hasUnpushedEdits: true,
        editState: 'editing',
        forceEditing: false,
      } as never,
      refreshLinkStatus: vi.fn(async () => {}),
    });
    useUIStore.setState({ activeDialog: 'speleodb-discard-changes', dialogData: {} });
    render(<SpeleoDiscardChangesDialog />);

    const keep = screen.getByTestId('speleodb-discard-changes-keep');
    expect(screen.getByTestId('speleodb-discard-changes-release')).toBeInTheDocument();
    // Keeping is primary: still holding the lock the user can release at any time,
    // whereas a released lock is a colleague's to take.
    expect(keep).toHaveAttribute('data-primary-action');

    invokeIpcMock.mockResolvedValueOnce(null);
    await userEvent.click(screen.getByTestId('speleodb-discard-changes-release'));
    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_discard_changes', { keepLock: false, acknowledged: true }),
    );
    await waitFor(() =>
      expect(useProjectStore.getState().refreshProjectState).toHaveBeenCalled(),
    );
    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('does not offer to release a lock a force-editor never held', async () => {
    // [#303] A #213 force-editor is dirty by design and holds no lock, so Discard is
    // exactly the escape hatch they want - but "and release the lock" would reach
    // `speleodb_release_current_lock`, the ADMIN-capable stuck-lock recovery verb, and
    // either fail or force-release the colleague's lock as a side effect. Discarding
    // itself must stay available; only the lock half goes.
    const { default: SpeleoDiscardChangesDialog } = await import('./SpeleoDiscardChangesDialog');
    useSpeleoStore.setState({
      link: {
        unwound: false,
        hasUnpushedEdits: true,
        editState: 'editing',
        forceEditing: true,
      } as never,
      refreshLinkStatus: vi.fn(async () => {}),
    });
    useUIStore.setState({ activeDialog: 'speleodb-discard-changes', dialogData: {} });
    render(<SpeleoDiscardChangesDialog />);

    expect(screen.queryByTestId('speleodb-discard-changes-release')).not.toBeInTheDocument();
    const keep = screen.getByTestId('speleodb-discard-changes-keep');
    expect(keep).toBeInTheDocument();

    await userEvent.click(keep);
    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_discard_changes', { keepLock: true, acknowledged: true }),
    );
  });

  it('does nothing at all on cancel', async () => {
    open();
    render(<SpeleoUnwoundPushDialog />);

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-cancel'));

    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  /**
   * [#303] The dialog now also finishes the quit / close it was raised out of, which it
   * knows about ONLY from `dialogData.pendingExit`. The routes that carry none - the
   * status button and the SpeleoDB lock menu, where this confirmation IS the whole
   * interaction - must be untouched by that: no exit, and the same result routing.
   */
  describe('the routes with no exit waiting are unchanged', () => {
    it('closes and exits nothing when Push anyway succeeds', async () => {
      invokeIpcMock.mockResolvedValueOnce({ kind: 'released' });
      open();
      render(<SpeleoUnwoundPushDialog />);

      await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));

      await waitFor(() =>
        expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_done', expect.anything()),
      );
      expect(useUIStore.getState().activeDialog).toBeNull();
      expect(performQuitMock).not.toHaveBeenCalled();
      expect(handBackProjectMock).not.toHaveBeenCalled();
    });

    it('exits nothing on the discard branch either', async () => {
      open();
      render(<SpeleoUnwoundPushDialog />);

      await userEvent.click(screen.getByTestId('speleodb-unwound-push-discard'));

      await waitFor(() => expect(useUIStore.getState().activeDialog).toBeNull());
      expect(performQuitMock).not.toHaveBeenCalled();
      expect(handBackProjectMock).not.toHaveBeenCalled();
    });

    it('exits nothing on cancel', async () => {
      open();
      render(<SpeleoUnwoundPushDialog />);

      await userEvent.click(screen.getByTestId('speleodb-unwound-push-cancel'));

      expect(performQuitMock).not.toHaveBeenCalled();
      expect(handBackProjectMock).not.toHaveBeenCalled();
    });
  });

  it.each(['request', 'session'])('refuses both actions after its %s is replaced', async (replacement) => {
    const request = reserveProjectRequest();
    const sessionGeneration = useSpeleoStore.getState().sessionGeneration;
    useUIStore.setState({ activeDialog: 'speleodb-unwound-push', dialogData: {
      message: 'old close', pendingExit: { kind: 'close', request, sessionGeneration, pendingOpenPath: '/synthetic/destination.stalx' },
    } });
    render(<SpeleoUnwoundPushDialog />);
    if (replacement === 'request') reserveProjectRequest();
    else useSpeleoStore.getState().clearSession();
    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));
    await userEvent.click(screen.getByTestId('speleodb-unwound-push-discard'));
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(handBackProjectMock).not.toHaveBeenCalled();
  });

  it('does not apply an old confirmation result or dismiss its replacement prompt', async () => {
    let resolve!: (value: unknown) => void;
    invokeIpcMock.mockReturnValue(new Promise((yes) => { resolve = yes; }));
    const request = reserveProjectRequest();
    useUIStore.setState({ activeDialog: 'speleodb-unwound-push', dialogData: {
      message: 'old close', pendingExit: { kind: 'close', request },
    } });
    render(<SpeleoUnwoundPushDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));
    const replacement = { message: 'replacement confirmation' };
    useUIStore.setState({ dialogData: replacement });
    await act(async () => { resolve({ kind: 'released' }); });
    expect(screen.getByTestId('speleodb-unwound-push-push-anyway')).toBeEnabled();
    expect(useUIStore.getState().dialogData).toBe(replacement);
    expect(useUIStore.getState().closeDialog).not.toHaveBeenCalled();
    expect(handBackProjectMock).not.toHaveBeenCalled();
  });


  it('restores dismissal after logout while an upload is pending', async () => {
    let resolve!: (value: unknown) => void;
    invokeIpcMock.mockReturnValue(new Promise((yes) => { resolve = yes; }));
    open();
    render(<SpeleoUnwoundPushDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));
    expect(screen.getByTestId('speleodb-unwound-push-cancel')).toBeDisabled();
    useSpeleoStore.getState().clearSession();
    await act(async () => { resolve({ kind: 'released' }); });
    expect(screen.getByTestId('speleodb-unwound-push-cancel')).toBeEnabled();
    expect(handBackProjectMock).not.toHaveBeenCalled();
    await userEvent.click(screen.getByTestId('speleodb-unwound-push-cancel'));
    expect(useUIStore.getState().activeDialog).toBeNull();
  });


  it.each(['speleodb-unwound-push-push-anyway', 'speleodb-unwound-push-discard'])('does not apply %s to a replacement project without a pending exit', async (button) => {
    open();
    render(<SpeleoUnwoundPushDialog />);
    useProjectStore.getState().startLoading('Replacement', '/synthetic/replacement.stalx');
    await userEvent.click(screen.getByTestId(button));
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(screen.getByTestId('speleodb-unwound-push-cancel')).toBeEnabled();
  });

});
