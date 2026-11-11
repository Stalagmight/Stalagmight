/**
 * A SpeleoDB project card has no separate Open button: the card itself is the
 * Open action, by pointer and by keyboard, and it is gated while a pull runs.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import RemoteSystemCard from '../RemoteSystemCard';
import type { SpeleoCachedProject, SpeleoProjectSummary } from '../../../types/ipc.generated';
import { UNPUSHED_ACCENT } from '../../../utils/speleoLocalState';
import type { SpeleoLockInfo } from '../SpeleoLockBadge';

const SUMMARY: SpeleoProjectSummary = {
  id: 'p1', name: 'Synthetic remote cave', permission: 'READ_AND_WRITE',
  lock_holder: null, type_tag: 'stalx', latest_commit_sha: null,
};
const LOCK: SpeleoLockInfo = {
  isRemote: true, lockHolder: null, lockedByMe: false, speleoName: 'Synthetic remote cave', hasLocalEdits: false,
};

const renderCard = (busy = false) => {
  const onOpen = vi.fn();
  render(<RemoteSystemCard summary={SUMMARY} lock={LOCK} onOpen={onOpen} busy={busy} />);
  return { onOpen, card: screen.getByTestId('system-card-remote-p1') };
};

describe('RemoteSystemCard', () => {
  it('has no Open button; the focusable card is a button', () => {
    const { card } = renderCard();
    expect(screen.queryByTestId('system-card-remote-open-p1')).not.toBeInTheDocument();
    expect(screen.getAllByRole('button')).toEqual([card]);
    expect(card).toHaveAttribute('tabindex', '0');
  });

  it('opens on click, Enter and Space', async () => {
    const { onOpen, card } = renderCard();
    await userEvent.click(card);
    card.focus();
    await userEvent.keyboard('{Enter}');
    await userEvent.keyboard(' ');
    expect(onOpen).toHaveBeenCalledTimes(3);
    expect(onOpen).toHaveBeenCalledWith(SUMMARY);
  });

  it('does not open while busy', async () => {
    const { onOpen, card } = renderCard(true);
    expect(card).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(card);
    card.focus();
    await userEvent.keyboard('{Enter}');
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('shows the updated-time footer only when the server reported a time', () => {
    const { rerender } = render(<RemoteSystemCard summary={SUMMARY} lock={LOCK} onOpen={vi.fn()} />);
    expect(screen.queryByTestId('system-card-remote-p1-footer')).not.toBeInTheDocument();
    rerender(<RemoteSystemCard summary={{ ...SUMMARY, modified_date: '2026-10-01T10:00:00Z' }} lock={LOCK} onOpen={vi.fn()} />);
    expect(screen.getByTestId('system-card-remote-p1-footer').querySelector('time')).not.toBeNull();
  });
});

/**
 * Un-uploaded work on this computer gets the same yellow leading edge on the card
 * as on the table row, whether the local-state scan found it or the cached copy's
 * own listing says so (a signed-out copy may be in neither recents nor the server
 * listing, which feed the scan).
 */
describe('RemoteSystemCard un-uploaded marker', () => {
  const COPY: SpeleoCachedProject = {
    project_id: 'p1', project_name: 'Synthetic remote cave', local_path: '/cache/speleodb/p1/project.stalx',
    ownership: 'signed_out', has_unpushed_edits: true, state_unknown: false, is_open: false,
  };
  const card = () => screen.getByTestId('system-card-remote-p1');

  it('is marked when the local-state scan reports un-uploaded edits', () => {
    render(<RemoteSystemCard summary={SUMMARY} lock={{ ...LOCK, hasLocalEdits: true }} onOpen={vi.fn()} />);
    expect(card()).toHaveAttribute('data-recent-accent', 'unpushed');
    expect(card().style.boxShadow).toContain(UNPUSHED_ACCENT);
  });

  it('is marked when only the cached copy says so', () => {
    render(<RemoteSystemCard summary={SUMMARY} lock={LOCK} cached={COPY} onOpen={vi.fn()} />);
    expect(card()).toHaveAttribute('data-recent-accent', 'unpushed');
  });

  it('is not marked for a clean copy', () => {
    render(<RemoteSystemCard summary={SUMMARY} lock={LOCK} cached={{ ...COPY, has_unpushed_edits: false }} onOpen={vi.fn()} />);
    expect(card()).toHaveAttribute('data-recent-accent', 'none');
    expect(card().style.boxShadow).toBe('');
  });
});
