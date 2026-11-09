/**
 * Component test (#270): the status button surfacing work that exists only on
 * this computer.
 *
 * `hasUnpushedEdits` is NOT the unsaved-to-disk flag - it means "SpeleoDB does not
 * have these edits", it survives a save and a restart, and before #270 the
 * `unlocked` state was blind to it: a user with local work and no lock saw a plain
 * white mark and a tooltip that said nothing about it. The mark now turns yellow
 * whenever there are un-pushed edits, and gains a standalone "!" when no lock is
 * protecting them.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { SpeleoStatusButton } from '../SpeleoStatusButton';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { UNPUSHED_ACCENT } from '../../../utils/speleoLocalState';

/** Read-only (no lock held), nothing local outstanding. */
const CLEAN_UNLOCKED: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'read-only',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: false,
  unwound: false,
  forceEditing: false,
  forceEditAllowed: true,
  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

const seed = (o: Partial<SpeleoLinkState> = {}) =>
  useSpeleoStore.setState({ link: { ...CLEAN_UNLOCKED, ...o }, banner: null, busy: false });

const button = () => screen.getByTestId('speleodb-status-button');
const badge = () => screen.queryByTestId('speleodb-status-unpushed-badge');
const statusMark = () => screen.getByTestId('speleodb-status-icon-logo');

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  useSpeleoStore.setState({
    link: null,
    banner: null,
    busy: false,
    session: { loggedIn: true, userEmail: 'me@example.com', endpoint: null, },
  });
  useUIStore.setState({ activeDialog: null, dialogData: {}, appMode: 'survey' });
});

describe('SpeleoStatusButton un-pushed local edits (#270)', () => {
  it('leaves a clean project untinted (inherits currentColor), with no badge', () => {
    seed();
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'unlocked');
    expect(button()).toHaveAttribute('data-unpushed', 'false');
    expect(statusMark()).not.toHaveAttribute('data-tint');
    expect(badge()).not.toBeInTheDocument();
  });

  it('turns the mark yellow and shows "!" when unlocked with local changes', () => {
    // THE gap #270 closes: this state used to render identically to the clean one.
    seed({ hasUnpushedEdits: true });
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'unlocked');
    expect(button()).toHaveAttribute('data-unpushed', 'true');
    expect(statusMark()).toHaveAttribute('data-tint', UNPUSHED_ACCENT);
    expect(badge()).toBeInTheDocument();
  });

  it('reuses the toolbar Import icon accent, not a second yellow', () => {
    seed({ hasUnpushedEdits: true });
    render(<SpeleoStatusButton />);
    expect(UNPUSHED_ACCENT).toBe('var(--sh-comment)');
    expect(badge()).toHaveStyle({ color: UNPUSHED_ACCENT });
  });

  it('says the changes are only on this computer in the tooltip', () => {
    seed({ hasUnpushedEdits: true });
    render(<SpeleoStatusButton />);
    expect(button().getAttribute('title')).toContain('only on this computer');
    // The accessible name carries the same wording (the "!" itself is decorative).
    expect(button().getAttribute('aria-label')).toContain('only on this computer');
  });

  it('is yellow but badge-free while we hold the lock', () => {
    // The lock is protecting the work, so the standalone "!" would be noise; the
    // yellow mark still reports that SpeleoDB does not have the edits.
    seed({ hasUnpushedEdits: true, editState: 'editing' });
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'editing');
    expect(statusMark()).toHaveAttribute('data-tint', UNPUSHED_ACCENT);
    expect(badge()).not.toBeInTheDocument();
  });

  it('badges a force-edit session, which holds no lock despite "editing"', () => {
    seed({ hasUnpushedEdits: true, editState: 'editing', forceEditing: true });
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'force_edit');
    expect(badge()).toBeInTheDocument();
  });

  it('badges the remote-ahead state too when the edits are ours and unlocked', () => {
    seed({ hasUnpushedEdits: true, remoteState: 'remote_ahead' });
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'remote_ahead');
    expect(statusMark()).toHaveAttribute('data-tint', UNPUSHED_ACCENT);
    expect(badge()).toBeInTheDocument();
  });

  it('still reports local changes on the logged-out mark', () => {
    // Logged out, no lock, work only here: the most important time to say so, even
    // though the button itself is inert.
    useSpeleoStore.setState({
      session: { loggedIn: false, userEmail: null, endpoint: null, },
    });
    seed({ hasUnpushedEdits: true });
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'logged_out');
    expect(statusMark()).toHaveAttribute('data-tint', UNPUSHED_ACCENT);
    expect(badge()).toBeInTheDocument();
  });
});

describe('SpeleoStatusButton state badge', () => {
  const stateBadge = () => screen.queryByTestId('speleodb-status-icon-state-badge');

  it('leaves the resting unlocked state bare', () => {
    // The read-only, up-to-date, nothing-outstanding case: nothing is wrong and
    // nothing is held, so the mark carries no badge at all. Without this the
    // requirement is unpinnable - the corner badge has no other assertion on it.
    seed();
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'unlocked');
    expect(stateBadge()).not.toBeInTheDocument();
  });

  it('badges the state where we hold the write lock', () => {
    seed({ editState: 'editing' });
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'editing');
    expect(stateBadge()).toBeInTheDocument();
  });

  it('keeps badging the states that are waiting on someone else', () => {
    seed({ editState: 'read-only', lockedByOther: true, lockHolder: 'other@example.com' });
    render(<SpeleoStatusButton />);
    expect(button()).toHaveAttribute('data-kind', 'locked_by_other');
    expect(stateBadge()).toBeInTheDocument();
  });
});
