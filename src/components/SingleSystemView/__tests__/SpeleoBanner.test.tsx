
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { SpeleoBanner } from '../SpeleoBanner';
import { SpeleoWorkspace } from '../SpeleoWorkspace';
import { useProjectStore } from '../../../store/projectStore';
import { captureUploadOwner, getSetAsideUploadMessage, getUploadContinuation, setAsideUploadMessage, setUploadContinuation } from '../../../utils/uploadContinuation';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { askSpeleoExit, resetSpeleoExitPromptForTests } from '../../../utils/speleoExit';
import { useSpeleoSignOut } from '../../../utils/speleoSignOut';

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'editing',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: true,
  unwound: false,

  undoMode: 'full',
  remoteState: 'unknown',
  noAccess: false,
  serverError: null,
};

const seedLink = (o: Partial<SpeleoLinkState> = {}) =>
  useSpeleoStore.setState({ link: { ...LINK, ...o }, banner: null });

beforeEach(() => {
  setUploadContinuation(null);
  useProjectStore.setState({ project: { file_path: '/synthetic/remote.stalx', name: 'Synthetic survey', zero_datums: [] } as never, isDirty: false });
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue({ kind: 'editing' });
  useSpeleoStore.setState({ link: null, banner: null, busy: false, session: null, remoteProjectId: 'synthetic' });
  useUIStore.setState({ activeDialog: null, dialogData: {}, speleoWorkspaceSection: null });
});

describe('SpeleoBanner read-only state', () => {
  it.each([
    ['free lock', {}],
    ['someone else editing', { lockHolder: 'editor@example.com', lockedByOther: true }],
    ['no project access', { noAccess: true }],
  ])('draws no top bar for a read-only project (%s) without a banner', (_label, o) => {
    seedLink({ editState: 'read-only', ...o });
    const { container } = render(<SpeleoBanner />);
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByTestId('speleodb-access-notice')).not.toBeInTheDocument();
  });
});

describe('SpeleoBanner no_anchor notice', () => {
  it('shows the no-zero-datum notice with a Dismiss that clears it', async () => {
    seedLink();
    useSpeleoStore.getState().raiseNoAnchorBanner();
    render(<SpeleoBanner />);
    expect(screen.getByTestId('speleo-banner')).toHaveAttribute('data-kind', 'no_anchor');
    await userEvent.click(screen.getByTestId('speleo-banner-dismiss'));
    expect(useSpeleoStore.getState().banner).toBeNull();
  });
});

describe('SpeleoBanner push failure (#260)', () => {
  const seedFailure = (origin: 'push' | 'done', pushMessage: string | null = null) => {
    seedLink({ });
    useSpeleoStore.setState({
      banner: { kind: 'push_failed', message: 'Could not push changes: boom', origin, pushMessage },
    });
  };

  it('renders the failure message and a Retry action', () => {
    seedFailure('push');
    render(<SpeleoBanner />);
    const banner = screen.getByTestId('speleo-banner');
    expect(banner).toHaveAttribute('data-kind', 'push_failed');
    expect(banner.textContent).toContain('Could not push changes: boom');
    expect(screen.getByTestId('speleo-banner-retry-push')).toBeInTheDocument();
    // Transient action feedback stays dismissable.
    expect(screen.getByTestId('speleo-banner-dismiss')).toBeInTheDocument();
  });

  it('Retry opens Save confirmation with the original description without uploading', async () => {
    seedFailure('push', 'my message');
    // Signed in: an editing session that can upload (signed out, Save keeps the
    // work on this computer instead - see dispatchProjectSave).
    useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' } });
    render(<SpeleoBanner />);
    await userEvent.click(screen.getByTestId('speleo-banner-retry-push'));
    expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
    expect(useUIStore.getState().dialogData.mode).toBe('push');
    expect(getUploadContinuation('/synthetic/remote.stalx')?.message).toBe('my message');
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('Retry starts from a description set aside by Resolve now, not the failed one', async () => {
    seedFailure('push', 'the failed description');
    useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' } });
    // The user edited the description after the failure, then chose Resolve now.
    setAsideUploadMessage('edited after the failure', captureUploadOwner());
    render(<SpeleoBanner />);
    await userEvent.click(screen.getByTestId('speleo-banner-retry-push'));
    expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
    expect(getUploadContinuation('/synthetic/remote.stalx')?.message).toBe('edited after the failure');
    expect(getSetAsideUploadMessage('/synthetic/remote.stalx')).toBeNull();
  });

  it('Retry opens release confirmation with the original description after a Done failure', async () => {
    seedFailure('done', 'wrap up');
    render(<SpeleoBanner />);
    await userEvent.click(screen.getByTestId('speleo-banner-retry-push'));
    expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
    expect(useUIStore.getState().dialogData.mode).toBe('release');
    expect(getUploadContinuation('/synthetic/remote.stalx')?.message).toBe('wrap up');
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('Retry is disabled while a verb is already in flight (no double-fire)', async () => {
    seedFailure('push');
    useSpeleoStore.setState({ busy: true });
    render(<SpeleoBanner />);

    const retry = screen.getByTestId('speleo-banner-retry-push');
    expect(retry).toBeDisabled();
    await userEvent.click(retry);
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_push')).toBe(false);
  });

  it('a landed push clears the banner (applyPush)', () => {
    seedFailure('push');
    useSpeleoStore.getState().applyPush({ kind: 'pushed' } as never);
    expect(useSpeleoStore.getState().banner).toBeNull();
  });

  it('a released lock clears the banner (applyDone)', () => {
    seedFailure('done');
    useSpeleoStore.getState().applyDone({ kind: 'released' } as never);
    expect(useSpeleoStore.getState().banner).toBeNull();
  });

  it('closing the project clears the banner (clearLink)', () => {
    seedFailure('push');
    useSpeleoStore.getState().clearLink();
    expect(useSpeleoStore.getState().banner).toBeNull();
  });

  /**
   * #260 defect: the banner rendered BEHIND the modal scrim of the dialog that
   * issued the failed upload - dimmed, unreadable, its Retry/Dismiss unclickable -
   * exactly when the user needed it. While such a dialog is open it owns the
   * failure (rendering it inline); the banner is the surface for AFTER it is
   * dismissed unresolved.
   */
  it('hides itself while the Finish-editing dialog owns the failure', () => {
    seedFailure('done');
    useUIStore.setState({ activeDialog: 'speleodb-done' });
    render(<SpeleoBanner />);
    expect(screen.queryByTestId('speleo-banner')).not.toBeInTheDocument();
  });

  // The exit prompt (Close / Quit / switch) and the sign-out prompt are overlays of
  // their own, not an `activeDialog`, and render the same failure inline in their
  // upload step: the banner used to show it a second time behind them.
  it.each([
    ['the exit prompt', () => {
      void askSpeleoExit('quit');
      return () => resetSpeleoExitPromptForTests();
    }],
    ['the sign-out prompt', () => {
      useSpeleoSignOut.setState({ request: { id: 1, kind: 'signOut', survey: null, settle: () => {} } });
      return () => useSpeleoSignOut.setState({ request: null });
    }],
  ])('hides itself while %s is open, and comes back once it is gone', (_label, open) => {
    seedFailure('done');
    const close = open();
    const { rerender } = render(<SpeleoBanner />);
    expect(screen.queryByTestId('speleo-banner')).not.toBeInTheDocument();
    act(() => close());
    rerender(<SpeleoBanner />);
    expect(screen.getByTestId('speleo-banner')).toHaveAttribute('data-kind', 'push_failed');
  });

  it('renders again once that dialog is dismissed with the failure unresolved', () => {
    seedFailure('done', 'wrap up');
    useUIStore.setState({ activeDialog: 'speleodb-done' });
    const { rerender } = render(<SpeleoBanner />);
    expect(screen.queryByTestId('speleo-banner')).not.toBeInTheDocument();

    // Cancel / X / Escape / backdrop all land here: the dialog closes, the failure
    // is still unresolved, so the durable reminder comes back.
    useUIStore.getState().closeDialog();
    rerender(<SpeleoBanner />);
    const banner = screen.getByTestId('speleo-banner');
    expect(banner).toHaveAttribute('data-kind', 'push_failed');
    expect(screen.getByTestId('speleo-banner-retry-push')).toBeInTheDocument();
  });

  it('still shows an unrelated banner while such a dialog is open', () => {
    // Only the push-failure kind is owned by the dialog; the informational
    // notices are not duplicated there, so they must not be suppressed.
    seedLink({ });
    useSpeleoStore.setState({ banner: { kind: 'no_anchor' } });
    useUIStore.setState({ activeDialog: 'speleodb-done' });
    render(<SpeleoBanner />);
    expect(screen.getByTestId('speleo-banner')).toHaveAttribute('data-kind', 'no_anchor');
  });

  it('clearPushFailedBanner leaves an unrelated banner alone', () => {
    seedLink({ });
    useSpeleoStore.setState({ banner: { kind: 'no_anchor' } });
    useSpeleoStore.getState().clearPushFailedBanner();
    expect(useSpeleoStore.getState().banner).toEqual({ kind: 'no_anchor' });
  });
});

/**
 * #270 - "Retry upload" is only honest while the edit gate is open.
 *
 * Both retry verbs (push / done) upload nothing when no lock is held: the backend
 * answers `not_editing`. The banner offered Retry gated on `busy` ALONE, and a 401 is
 * the very thing that gets a project into this state - `clearSession()` drops to
 * read-only while keeping the unpushed edits, which is also what raised the banner.
 * So the one control on screen was a button that could only ever no-op.
 */
describe('SpeleoBanner retry needs an open edit gate (#270)', () => {
  const seedReadOnlyFailure = () => {
    seedLink({ editState: 'read-only' });
    useSpeleoStore.setState({
      banner: {
        kind: 'push_failed',
        message: 'Could not push changes: unauthorized',
        origin: 'push',
        pushMessage: 'm',
      },
    });
  };

  it('offers Start-editing instead of Retry while read-only', () => {
    seedReadOnlyFailure();
    render(<SpeleoBanner />);
    expect(screen.queryByTestId('speleo-banner-retry-push')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleo-banner-start-editing-to-retry')).toBeInTheDocument();
  });

  it('opens Settings for signed-out recovery without acquiring or uploading', async () => {
    seedReadOnlyFailure();
    render(<SpeleoBanner />);
    await userEvent.click(screen.getByTestId('speleo-banner-start-editing-to-retry'));
    expect(useUIStore.getState().activeDialog).toBe('settings');
    expect(useUIStore.getState().dialogData.initialCategory).toBe('speleodb');
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('still offers Retry once the gate is open again', () => {
    seedLink({ editState: 'editing' });
    useSpeleoStore.setState({
      banner: { kind: 'push_failed', message: 'boom', origin: 'push', pushMessage: 'm' },
    });
    render(<SpeleoBanner />);
    expect(screen.getByTestId('speleo-banner-retry-push')).toBeInTheDocument();
    expect(
      screen.queryByTestId('speleo-banner-start-editing-to-retry'),
    ).not.toBeInTheDocument();
  });

  it('preserves the failure while workspace Resume editing recovers the session', async () => {
    seedReadOnlyFailure();
    useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' } });
    invokeIpcMock.mockResolvedValue({ kind: 'editing' });
    render(<><SpeleoBanner /><SpeleoWorkspace /></>);
    await userEvent.click(screen.getByTestId('speleo-banner-start-editing-to-retry'));
    expect(useUIStore.getState().leftWorkspace).toBe('speleodb');
    expect(invokeIpcMock).not.toHaveBeenCalled();
    await userEvent.click(screen.getByTestId('speleodb-workspace-editing'));
    await waitFor(() => expect(screen.getByTestId('speleo-banner-retry-push')).toBeInTheDocument());
    expect(useSpeleoStore.getState().banner).toMatchObject({ origin: 'push', pushMessage: 'm' });
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_start_editing');
  });

  it('a verified workspace fast-forward clears the obsolete upload failure', async () => {
    seedReadOnlyFailure();
    useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' },
      link: { ...LINK, editState: 'read-only', hasUnpushedEdits: false }, refreshLinkStatus: vi.fn(async () => {}) });
    useProjectStore.setState({ loadProject: vi.fn(async () => ({ success: true })),
      refreshProjectState: vi.fn(async () => {}), checkProjectIntegrity: vi.fn(async () => []), checkHistoryDiscarded: vi.fn(async () => {}) });
    invokeIpcMock.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/tmp/p.stalx' });
    render(<><SpeleoBanner /><SpeleoWorkspace /></>);
    await userEvent.click(screen.getByTestId('speleodb-workspace-editing'));
    await waitFor(() => expect(useSpeleoStore.getState().banner).toBeNull());
  });
});
