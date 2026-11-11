// @vitest-environment jsdom
/**
 * SpeleoDB settings: "Purge all cached files" keeps, or asks twice about, copies
 * with un-uploaded changes, and never touches the open project's copy.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: ipc, getErrorMessage: String }));

import '../../../i18n';
import { SpeleoCachePurgeSection } from './SpeleoCachePurgeSection';
import { useSpeleoCacheStore } from '../../../store/speleoCacheStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import type { SpeleoCachedProject, SpeleoPurgeReport } from '../../../types/ipc.generated';

function copy(id: string, name: string, o: Partial<SpeleoCachedProject> = {}): SpeleoCachedProject {
  return {
    project_id: id, project_name: name, local_path: `/cache/${id}/project.stalx`, ownership: 'mine',
    has_unpushed_edits: false, state_unknown: false, is_open: false, ...o,
  };
}

const REPORT: SpeleoPurgeReport = { removed: 2, kept_unpushed: [], kept_open: [], failed: [], locks_released: [], locks_unchecked: [] };

function answer(listing: SpeleoCachedProject[], report: SpeleoPurgeReport = REPORT) {
  ipc.mockImplementation(async (cmd: string) => {
    if (cmd === 'speleodb_cached_projects') return listing;
    if (cmd === 'speleodb_purge_cached_copies') return report;
    return null;
  });
}

const purges = () => ipc.mock.calls.filter((c) => c[0] === 'speleodb_purge_cached_copies').map((c) => c[1]);

beforeEach(() => {
  ipc.mockReset();
  useSpeleoCacheStore.setState({ copies: [], loaded: false });
  useSpeleoStore.setState({ busy: false });
  useProjectStore.setState({ loadRecentProjects: vi.fn(async () => {}) });
});

describe('Purge all cached files', () => {
  it('with nothing at risk, purges after one confirmation and reports the result', async () => {
    answer([copy('a', 'Alpha'), copy('b', 'Beta')]);
    render(<SpeleoCachePurgeSection />);
    await userEvent.click(screen.getByTestId('speleodb-purge-cache'));
    const dialog = await screen.findByTestId('speleodb-purge-dialog');
    expect(dialog).toHaveTextContent('Remove 2 downloaded copies');
    expect(screen.queryByTestId('speleodb-purge-at-risk')).toBeNull();
    await userEvent.click(screen.getByTestId('speleodb-purge-confirm'));
    expect(purges()).toEqual([{ discardUnpushed: false }]);
    expect(await screen.findByTestId('speleodb-purge-result')).toHaveTextContent('Removed 2 downloaded copies.');
  });

  it('lists copies with un-uploaded changes and keeps them by default', async () => {
    answer([copy('a', 'Alpha'), copy('b', 'Beta', { has_unpushed_edits: true }), copy('c', 'Gamma', { state_unknown: true })],
      { removed: 1, kept_unpushed: ['Beta', 'Gamma'], kept_open: [], failed: [], locks_released: ['Alpha'], locks_unchecked: [] });
    render(<SpeleoCachePurgeSection />);
    await userEvent.click(screen.getByTestId('speleodb-purge-cache'));
    expect(await screen.findByTestId('speleodb-purge-at-risk-list')).toHaveTextContent('BetaGamma');
    expect(screen.queryByTestId('speleodb-purge-confirm')).toBeNull();
    expect(screen.getByTestId('speleodb-purge-locks')).toHaveTextContent('Your editing locks on these projects are handed back.');
    await userEvent.click(screen.getByTestId('speleodb-purge-keep-at-risk'));
    expect(purges()).toEqual([{ discardUnpushed: false }]);
    const result = await screen.findByTestId('speleodb-purge-result');
    expect(result).toHaveTextContent('Kept, with changes that were never uploaded: Beta, Gamma');
    expect(result).toHaveTextContent('Handed back the editing lock of: Alpha');
  });

  it('signed out, says no lock is handed back, and reports locks it could not check', async () => {
    answer([copy('a', 'Alpha', { ownership: 'signed_out' })],
      { removed: 1, kept_unpushed: [], kept_open: [], failed: [], locks_released: [], locks_unchecked: ['Alpha'] });
    render(<SpeleoCachePurgeSection />);
    await userEvent.click(screen.getByTestId('speleodb-purge-cache'));
    expect(await screen.findByTestId('speleodb-purge-locks')).toHaveTextContent('You are not connected to SpeleoDB, so no editing lock is handed back.');
    await userEvent.click(screen.getByTestId('speleodb-purge-confirm'));
    const result = await screen.findByTestId('speleodb-purge-result');
    expect(result).toHaveTextContent('Could not hand back the editing lock of: Alpha.');
    expect(result).toHaveClass('text-warning-text');
  });

  it('deletes un-uploaded changes only after the second confirmation', async () => {
    answer([copy('b', 'Beta', { has_unpushed_edits: true })]);
    render(<SpeleoCachePurgeSection />);
    await userEvent.click(screen.getByTestId('speleodb-purge-cache'));
    // Nothing else to remove, so there is no "keep those, remove the rest".
    await screen.findByTestId('speleodb-purge-dialog');
    expect(screen.queryByTestId('speleodb-purge-keep-at-risk')).toBeNull();
    await userEvent.click(screen.getByTestId('speleodb-purge-delete-all'));
    expect(await screen.findByTestId('speleodb-purge-lost-dialog')).toHaveTextContent('lost for good');
    expect(purges()).toEqual([]);
    await userEvent.click(screen.getByTestId('speleodb-purge-lost-confirm'));
    expect(purges()).toEqual([{ discardUnpushed: true }]);
  });

  it('signed in, never counts or lists another account\'s copies, even with un-uploaded changes', async () => {
    answer([
      copy('a', 'Alpha'),
      copy('u', 'Legacy', { ownership: 'unknown', has_unpushed_edits: true }),
      copy('t', 'Theirs', { ownership: 'other_account', has_unpushed_edits: true }),
      copy('s', 'Theirs clean', { ownership: 'other_account' }),
    ]);
    render(<SpeleoCachePurgeSection />);
    await userEvent.click(screen.getByTestId('speleodb-purge-cache'));
    const dialog = await screen.findByTestId('speleodb-purge-dialog');
    expect(dialog).toHaveTextContent('Remove 2 downloaded copies');
    expect(screen.getByTestId('speleodb-purge-at-risk')).toHaveTextContent('1 copy has changes');
    expect(screen.getByTestId('speleodb-purge-at-risk-list')).toHaveTextContent('Legacy');
    expect(dialog).not.toHaveTextContent('Theirs');
    await userEvent.click(screen.getByTestId('speleodb-purge-delete-all'));
    const lost = await screen.findByTestId('speleodb-purge-lost-dialog');
    expect(lost).toHaveTextContent('Legacy');
    expect(lost).not.toHaveTextContent('Theirs');
  });

  it.each([
    ['no copy at all', []],
    ['only the open project', [copy('a', 'Alpha', { is_open: true })]],
    ['only another account\'s copies', [copy('t', 'Theirs', { ownership: 'other_account' })]],
  ])('is disabled, and says why, with %s cached', async (_label, listing) => {
    answer(listing);
    render(<SpeleoCachePurgeSection />);
    const button = screen.getByTestId('speleodb-purge-cache');
    const empty = await screen.findByTestId('speleodb-purge-empty');
    expect(empty).toHaveTextContent('There are no downloaded copies to remove.');
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription('There are no downloaded copies to remove.');
    await userEvent.click(button);
    expect(screen.queryByTestId('speleodb-purge-dialog')).toBeNull();
  });

  it('is enabled once the listing has a copy to remove', async () => {
    answer([copy('a', 'Alpha')]);
    render(<SpeleoCachePurgeSection />);
    await waitFor(() => expect(useSpeleoCacheStore.getState().loaded).toBe(true));
    expect(screen.getByTestId('speleodb-purge-cache')).toBeEnabled();
    expect(screen.queryByTestId('speleodb-purge-empty')).toBeNull();
  });

  it('announces the result in a live region mounted beforehand, and scrolls it into view', async () => {
    const scrolled = vi.fn();
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = scrolled;
    try {
      answer([copy('a', 'Alpha'), copy('b', 'Beta')]);
      render(<SpeleoCachePurgeSection />);
      const region = screen.getByRole('status');
      expect(region).toHaveAttribute('data-testid', 'speleodb-purge-status');
      expect(region).toBeEmptyDOMElement();
      await userEvent.click(screen.getByTestId('speleodb-purge-cache'));
      await userEvent.click(await screen.findByTestId('speleodb-purge-confirm'));
      await waitFor(() => expect(region).toHaveTextContent('Removed 2 downloaded copies.'));
      expect(scrolled).toHaveBeenCalledWith({ block: 'nearest' });
      expect(scrolled.mock.contexts).toContain(region);
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  });

  it('is disabled while a SpeleoDB operation runs', () => {
    useSpeleoStore.setState({ busy: true });
    render(<SpeleoCachePurgeSection />);
    expect(screen.getByTestId('speleodb-purge-cache')).toBeDisabled();
  });
});
