/**
 * Component test (#213 force-edit banner, #260 push-failure banner).
 *
 * The big-red banner appears whenever the link is force-editing (edit gate on, no
 * lock held), outranking the transient yellow notices; its Reconnect action runs the
 * existing acquire verb (speleodb_start_editing). It names the holder when locked.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import { SpeleoBanner } from '../SpeleoBanner';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';
import { makeSpeleoLink } from '../../../test/factories/speleo';

/** [#576 NF-W8FE-2] Done and Push answer the wire envelope, never a bare outcome. */
function report<R>(result: R): { result: R; save_warnings: [] } {
  return { result, save_warnings: [] };
}

const LINK: SpeleoLinkState = makeSpeleoLink({
  editState: 'editing',
  hasUnpushedEdits: true,
  forceEditing: true,
  forceEditAllowed: true,
  remoteState: 'unknown',
});

const seedLink = (o: Partial<SpeleoLinkState> = {}) =>
  useSpeleoStore.setState({ link: { ...LINK, ...o }, banner: null });

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue({ kind: 'editing' });
  useSpeleoStore.setState({ link: null, banner: null, busy: false });
  useUIStore.setState({ activeDialog: null, dialogData: {} });
});

describe('SpeleoBanner force-edit (#213)', () => {
  it('renders the persistent force-edit banner while force-editing', () => {
    seedLink();
    render(<SpeleoBanner />);
    const banner = screen.getByTestId('speleo-banner');
    expect(banner).toHaveAttribute('data-kind', 'force_edit_active');
    // No dismiss - it is backend-derived and re-raised every poll.
    expect(screen.queryByTestId('speleo-banner-dismiss')).not.toBeInTheDocument();
  });

  it('Reconnect runs the acquire verb (speleodb_start_editing)', async () => {
    seedLink();
    render(<SpeleoBanner />);
    await userEvent.click(screen.getByTestId('speleo-banner-reconnect'));
    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
  });

  it('names the lock holder in the banner when someone else holds it', () => {
    seedLink({ lockedByOther: true, lockHolder: 'holder@example.com' });
    render(<SpeleoBanner />);
    expect(screen.getByTestId('speleo-banner').textContent).toContain('holder@example.com');
  });

  it('does not render for a normal (non-force) editing session', () => {
    seedLink({ forceEditing: false });
    render(<SpeleoBanner />);
    expect(screen.queryByTestId('speleo-banner')).not.toBeInTheDocument();
  });
});

/**
 * #260 - the persistent push-failure banner. A failed push/unlock only raised a
 * toast, which auto-clears after 3 s, so the failure left no durable surface. The
 * banner keeps the humanized error on screen and its Retry replays the SAME verb
 * with the SAME commit message.
 */
describe('SpeleoBanner push failure (#260)', () => {
  const seedFailure = (origin: 'push' | 'done', pushMessage: string | null = null) => {
    seedLink({ forceEditing: false });
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

  it('Retry re-issues the push verb with the original commit message', async () => {
    seedFailure('push', 'my message');
    invokeIpcMock.mockResolvedValue(report({ kind: 'pushed' }));
    render(<SpeleoBanner />);

    await userEvent.click(screen.getByTestId('speleo-banner-retry-push'));

    await waitFor(() =>
      expect(
        invokeIpcMock.mock.calls.some(
          (c) => c[0] === 'speleodb_push' && (c[1] as { message?: string }).message === 'my message',
        ),
      ).toBe(true),
    );
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_done')).toBe(false);
  });

  it('Retry re-issues the unlock verb when the failure came from Done', async () => {
    seedFailure('done', 'wrap up');
    invokeIpcMock.mockResolvedValue(report({ kind: 'released' }));
    render(<SpeleoBanner />);

    await userEvent.click(screen.getByTestId('speleo-banner-retry-push'));

    await waitFor(() =>
      expect(
        invokeIpcMock.mock.calls.some(
          (c) => c[0] === 'speleodb_done' && (c[1] as { message?: string }).message === 'wrap up',
        ),
      ).toBe(true),
    );
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_push')).toBe(false);
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

  it('hides itself while the close-flow dialog owns the failure', () => {
    seedFailure('done');
    useUIStore.setState({ activeDialog: 'speleodb-close' });
    render(<SpeleoBanner />);
    expect(screen.queryByTestId('speleo-banner')).not.toBeInTheDocument();
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
    seedLink({ forceEditing: false });
    useSpeleoStore.setState({ banner: { kind: 'no_anchor' } });
    useUIStore.setState({ activeDialog: 'speleodb-done' });
    render(<SpeleoBanner />);
    expect(screen.getByTestId('speleo-banner')).toHaveAttribute('data-kind', 'no_anchor');
  });

  it('clearPushFailedBanner leaves an unrelated banner alone', () => {
    seedLink({ forceEditing: false });
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
    seedLink({ forceEditing: false, editState: 'read-only' });
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

  it('that action acquires the lock rather than re-issuing the doomed push', async () => {
    seedReadOnlyFailure();
    invokeIpcMock.mockResolvedValue({ kind: 'editing' });
    render(<SpeleoBanner />);

    await userEvent.click(screen.getByTestId('speleo-banner-start-editing-to-retry'));

    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_push')).toBe(false);
  });

  it('still offers Retry once the gate is open again', () => {
    seedLink({ forceEditing: false, editState: 'editing' });
    useSpeleoStore.setState({
      banner: { kind: 'push_failed', message: 'boom', origin: 'push', pushMessage: 'm' },
    });
    render(<SpeleoBanner />);
    expect(screen.getByTestId('speleo-banner-retry-push')).toBeInTheDocument();
    expect(
      screen.queryByTestId('speleo-banner-start-editing-to-retry'),
    ).not.toBeInTheDocument();
  });

  it('survives the start-editing it asks for, so Retry is actually reachable', async () => {
    // The test above seeds the post-gate state by hand; this one gets there the ONLY
    // way a user can - by pressing the banner's own Start-editing button. That path
    // used to destroy the banner: `applyStartEditing`'s success arm cleared the whole
    // slot, so the surface vanished the instant it succeeded, taking the Retry control
    // and the typed commit message with it and leaving the user holding the lock with
    // the edits still unpushed and nothing on screen saying so.
    seedReadOnlyFailure();
    invokeIpcMock.mockResolvedValue({ kind: 'editing' });
    render(<SpeleoBanner />);

    await userEvent.click(screen.getByTestId('speleo-banner-start-editing-to-retry'));

    await waitFor(() =>
      expect(screen.getByTestId('speleo-banner-retry-push')).toBeInTheDocument(),
    );
    const banner = useSpeleoStore.getState().banner;
    expect(banner?.kind).toBe('push_failed');
    // The commit message the retry replays came through intact.
    expect(banner).toMatchObject({ origin: 'push', pushMessage: 'm' });
  });

  it('but a fast-forward does clear it - the working copy is the server`s now', async () => {
    // A fast-forward can only run over a copy with nothing unpushed (the backend's
    // `LocalEditsPolicy::Protect` refuses otherwise) and it replaces the working copy
    // with the server's, so an older failure no longer describes anything here.
    seedReadOnlyFailure();
    invokeIpcMock.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/tmp/p.stalx' });
    render(<SpeleoBanner />);

    await userEvent.click(screen.getByTestId('speleo-banner-start-editing-to-retry'));

    await waitFor(() => expect(useSpeleoStore.getState().banner).toBeNull());
  });
});

/**
 * #576 B35(e) - the backend switched editing off because SpeleoDB says the write lock
 * this session was editing under is gone or someone else's. Say so, and who holds it.
 */
describe('SpeleoBanner lock lost (#576 B35(e))', () => {
  it('names the account that holds the lock now, with nothing to start', () => {
    seedLink({ forceEditing: false, editState: 'read-only' });
    useSpeleoStore.setState({ banner: { kind: 'lock_lost', holder: 'colleague@example.com' } });
    render(<SpeleoBanner />);
    const banner = screen.getByTestId('speleo-banner');
    expect(banner).toHaveAttribute('data-kind', 'lock_lost');
    expect(banner.textContent).toContain('colleague@example.com');
    expect(banner.textContent).toContain('editing has been switched off');
    expect(screen.queryByTestId('speleo-banner-lock-lost-start-editing')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleo-banner-dismiss')).toBeInTheDocument();
  });

  it('offers Start editing when nobody holds the lock any more', async () => {
    seedLink({ forceEditing: false, editState: 'read-only' });
    useSpeleoStore.setState({ banner: { kind: 'lock_lost', holder: null } });
    render(<SpeleoBanner />);
    expect(screen.getByTestId('speleo-banner').textContent).toContain(
      'You no longer hold the SpeleoDB write lock',
    );

    await userEvent.click(screen.getByTestId('speleo-banner-lock-lost-start-editing'));

    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_start_editing')).toBe(true),
    );
  });
});
