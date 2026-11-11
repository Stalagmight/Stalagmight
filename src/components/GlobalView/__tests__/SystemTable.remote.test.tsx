/**
 * Component test (#218) - the SystemTable renders a purely-remote SpeleoDB row and
 * its Open action pulls the project via onOpenRemote.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import SystemTable from '../SystemTable';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { RecentListItem } from '../recentListItems';
import type { SpeleoProjectSummary } from '../../../types/ipc.generated';

const SUMMARY: SpeleoProjectSummary = {
  id: 'p1',
  name: 'Remote Cave',
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  type_tag: 'stalx',
  latest_commit_sha: null,
};

const REMOTE_ITEM: Extract<RecentListItem, { kind: 'remote' }> = { kind: 'remote', projectId: 'p1', summary: SUMMARY };

function renderTable(
  overrides: { onOpenRemote?: ReturnType<typeof vi.fn>; busy?: boolean; summary?: SpeleoProjectSummary; cached?: SpeleoProjectSummary } = {},
) {
  const onOpenRemote = overrides.onOpenRemote ?? vi.fn();
  useSpeleoStore.setState({ remoteSummaries: overrides.cached ? { p1: overrides.cached } : {} });
  render(
    <SystemTable
      items={[{ ...REMOTE_ITEM, summary: overrides.summary ?? SUMMARY }]}
      selectedProjectId={null}
      multiSelectedIds={new Set()}
      sortField="name"
      sortDirection="asc"
      remoteLocks={{ p1: null }}
      remoteProjects={{ p1: 'Remote Cave' }}
      myEmail="me@x"
      onSort={vi.fn()}
      onSelect={vi.fn()}
      onOpen={vi.fn()}
      onContextMenu={vi.fn()}
      onRemoveMissing={vi.fn()}
      onOpenRemote={onOpenRemote}
      busy={overrides.busy ?? false}
    />,
  );
  return { onOpenRemote };
}

describe('SystemTable remote row (#218)', () => {
  it('renders a remote row with the SpeleoDB name', () => {
    renderTable();
    expect(screen.getByTestId('system-row-remote-p1')).toBeInTheDocument();
    expect(screen.getByText('Remote Cave')).toBeInTheDocument();
  });

  it('the row itself opens (pulls) the project on click and on Enter; no Open button', async () => {
    const { onOpenRemote } = renderTable();
    expect(screen.queryByTestId('system-row-remote-open-p1')).not.toBeInTheDocument();
    const row = screen.getByTestId('system-row-remote-p1');
    await userEvent.click(row);
    expect(onOpenRemote).toHaveBeenCalledWith(SUMMARY);
    row.focus();
    await userEvent.keyboard('{Enter}');
    expect(onOpenRemote).toHaveBeenCalledTimes(2);
  });

  it('gates the row while busy (no concurrent open)', async () => {
    const { onOpenRemote } = renderTable({ busy: true });
    const row = screen.getByTestId('system-row-remote-p1');
    expect(row).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(row);
    row.focus();
    await userEvent.keyboard('{Enter}');
    expect(onOpenRemote).not.toHaveBeenCalled();
  });
});


it.each(['READ_ONLY', 'READ_AND_WRITE'] as const)('uses the remote row permission %s over a conflicting cache', (permission) => {
  renderTable({ summary: { ...SUMMARY, permission }, cached: { ...SUMMARY,
    permission: permission === 'READ_ONLY' ? 'READ_AND_WRITE' : 'READ_ONLY',
  } });
  expect(screen.getByTestId('system-row-remote-p1-speleo-permission')).toHaveTextContent(
    permission === 'READ_ONLY' ? 'Permission: Read-only' : 'Permission: Read and write',
  );
});

it('marks a cached copy with un-uploaded edits even when the local-state scan does not list it', () => {
  useSpeleoStore.setState({ remoteSummaries: {} });
  render(
    <SystemTable
      items={[{ ...REMOTE_ITEM, cached: {
        project_id: 'p1', project_name: 'Remote Cave', local_path: '/cache/speleodb/p1/project.stalx',
        ownership: 'signed_out', has_unpushed_edits: true, state_unknown: false, is_open: false,
      } }]}
      selectedProjectId={null}
      multiSelectedIds={new Set()}
      sortField="name"
      sortDirection="asc"
      remoteLocks={{}}
      remoteProjects={{}}
      myEmail={null}
      onSort={vi.fn()}
      onSelect={vi.fn()}
      onOpen={vi.fn()}
      onContextMenu={vi.fn()}
      onRemoveMissing={vi.fn()}
      onOpenRemote={vi.fn()}
    />,
  );
  expect(screen.getByTestId('system-row-remote-p1')).toHaveAttribute('data-recent-accent', 'unpushed');
});
