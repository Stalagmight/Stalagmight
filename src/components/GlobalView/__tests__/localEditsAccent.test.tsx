/**
 * Component test (#270): the recents list accenting projects whose work exists
 * only on this computer.
 *
 * The list is otherwise purely server-derived (the lock scan), so it cannot tell a
 * project that is fully pushed from one holding hours of un-pushed survey. The
 * local states arrive from ONE `speleodb_local_project_states` call for the whole
 * list; the accent is deliberately just an accent - no "!" badge in a chooser - and
 * a failed call must simply leave the list unaccented, never break it.
 */
import { describe, it, expect, vi } from 'vitest';
import { act, render, screen, waitFor, renderHook } from '@testing-library/react';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import SystemTable from '../SystemTable';
import SystemCard from '../SystemCard';
import { resolveSpeleoLock } from '../SpeleoLockBadge';
import type { RecentListItem } from '../recentListItems';
import type { ProjectMetadata } from '../../../types/project';
import { useSpeleoLocalStates } from '../../../hooks/useSpeleoLocalStates';
import { UNPUSHED_ACCENT, type SpeleoLocalStateMap } from '../../../utils/speleoLocalState';
import { testIdPart } from '../../../utils/testId';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';

const DIRTY = 'aaaaaaaa-1111-1111-1111-111111111111';
const CLEAN = 'bbbbbbbb-2222-2222-2222-222222222222';

const project = (id: string, name: string): ProjectMetadata => ({
  name,
  location: null,
  file_path: `/p/${name}.stalx`,
  modified_at: '2026-07-26T00:00:00Z',
  section_count: 1,
  total_length: 10,
  station_count: 2,
  gps_center: null,
  gps_bounds: null,
  is_remote: true,
  speleodb_project_id: id,
});

const DIRTY_PROJECT = project(DIRTY, 'Dirty');
const CLEAN_PROJECT = project(CLEAN, 'Clean');

const STATES: SpeleoLocalStateMap = {
  [DIRTY]: {
    has_local_copy: true,
    has_unpushed_edits: true,
    last_edit_at: '2026-07-26T00:00:00Z',
    is_open: false,
    state_unknown: false,
  },
  [CLEAN]: {
    has_local_copy: true,
    has_unpushed_edits: false,
    last_edit_at: null,
    is_open: false,
    state_unknown: false,
  },
};

const ITEMS: RecentListItem[] = [
  { kind: 'available', filePath: DIRTY_PROJECT.file_path, project: DIRTY_PROJECT },
  { kind: 'available', filePath: CLEAN_PROJECT.file_path, project: CLEAN_PROJECT },
];

function renderTable(localStates: SpeleoLocalStateMap) {
  render(
    <SystemTable
      items={ITEMS}
      selectedProjectId={null}
      multiSelectedIds={new Set()}
      sortField="name"
      sortDirection="asc"
      remoteLocks={{ [DIRTY]: null, [CLEAN]: null }}
      remoteProjects={{ [DIRTY]: 'Dirty', [CLEAN]: 'Clean' }}
      myEmail="me@x"
      localStates={localStates}
      onSort={vi.fn()}
      onSelect={vi.fn()}
      onOpen={vi.fn()}
      onContextMenu={vi.fn()}
      onRemoveMissing={vi.fn()}
      onOpenRemote={vi.fn()}
    />,
  );
}

describe('recents local-edit accent (#270)', () => {
  it('accents only the table rows the IPC reports as having local edits', () => {
    renderTable(STATES);
    const dirtyRow = screen.getByTestId(`system-row-${testIdPart(DIRTY_PROJECT.file_path)}`);
    const cleanRow = screen.getByTestId(`system-row-${testIdPart(CLEAN_PROJECT.file_path)}`);
    expect(dirtyRow).toHaveAttribute('data-local-edits', 'true');
    expect(cleanRow).toHaveAttribute('data-local-edits', 'false');

    // The accent must sit on the row's first CELL, not the row. The table is
    // border-collapse, under which a <tr> generates no box, so a border or a
    // box-shadow set on it never paints - the accent was invisible in the table
    // view while passing a test that only checked the <tr>'s style attribute.
    const accentOf = (row: HTMLElement) =>
      row.querySelector('td')?.getAttribute('style') ?? '';
    expect(accentOf(dirtyRow)).toContain('var(--sh-comment)');
    expect(accentOf(cleanRow)).not.toContain('var(--sh-comment)');
    expect(dirtyRow.getAttribute('style') ?? '').not.toContain('var(--sh-comment)');
  });

  it('leaves every row unaccented when the local state is unavailable', () => {
    // The graceful-degradation contract: an empty map (what a failed fetch yields)
    // must read as "no accent", never as an accent-everything or a crash.
    renderTable({});
    for (const p of [DIRTY_PROJECT, CLEAN_PROJECT]) {
      expect(screen.getByTestId(`system-row-${testIdPart(p.file_path)}`)).toHaveAttribute(
        'data-local-edits',
        'false',
      );
    }
  });

  it('accents the card grid on the same signal, with no "!" badge', () => {
    render(
      <SystemCard
        name={DIRTY_PROJECT.name}
        location={null}
        filePath={DIRTY_PROJECT.file_path}
        modifiedAt={DIRTY_PROJECT.modified_at}
        sectionCount={1}
        totalLength={10}
        speleoLock={resolveSpeleoLock(DIRTY_PROJECT, {}, null, {}, STATES)}
      />,
    );
    const cardTestId = `system-card-${testIdPart(DIRTY_PROJECT.file_path)}`;
    const card = screen.getByTestId(cardTestId);
    expect(card).toHaveAttribute('data-local-edits', 'true');
    expect(card.getAttribute('style')).toContain('var(--sh-comment)');
    // The badge belongs on the status button, where there is an action to take.
    expect(screen.queryByTestId('speleodb-status-unpushed-badge')).not.toBeInTheDocument();
    // The mark itself carries the same accent as the status button.
    expect(screen.getByTestId(`${cardTestId}-speleo-logo`)).toHaveAttribute(
      'data-tint',
      UNPUSHED_ACCENT,
    );
  });

  it('resolveSpeleoLock reports no local edits for an unlisted or unknown project', () => {
    expect(resolveSpeleoLock(CLEAN_PROJECT, {}, null, {}, STATES).hasLocalEdits).toBe(false);
    expect(resolveSpeleoLock(DIRTY_PROJECT, {}, null, {}, {}).hasLocalEdits).toBe(false);
    expect(
      resolveSpeleoLock({ is_remote: false }, {}, null, {}, STATES).hasLocalEdits,
    ).toBe(false);
  });
});

describe('useSpeleoLocalStates (#270)', () => {
  it('fetches the whole id set in ONE call, not one per project', async () => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockResolvedValue(STATES);
    const { result } = renderHook(() => useSpeleoLocalStates([DIRTY, CLEAN, DIRTY]));

    await waitFor(() => expect(result.current.loaded).toBe(true));
    const calls = invokeIpcMock.mock.calls.filter(
      (c) => c[0] === 'speleodb_local_project_states',
    );
    expect(calls).toHaveLength(1);
    expect((calls[0]?.[1] as { projectIds: string[] }).projectIds.sort()).toEqual(
      [DIRTY, CLEAN].sort(),
    );
    expect(result.current.states).toEqual(STATES);
    expect(result.current.failed).toBe(false);
  });

  it('degrades to "no states, and we know it" when the call fails', async () => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockRejectedValue(new Error('backend down'));
    const { result } = renderHook(() => useSpeleoLocalStates([DIRTY]));

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.states).toEqual({});
    // `failed` is what stops a caller reading the empty map as "everything is clean".
    expect(result.current.failed).toBe(true);
  });

  it('goes back to "checking" when the id set GROWS while mounted', async () => {
    // Only the initial state used to carry loaded:false, so the flag guarded the
    // FIRST fetch and nothing after it. An id added while mounted (a second lock
    // scan landing while the held-lock prompt is up) left `loaded` true over the OLD
    // map: the new id read as absent, i.e. clean, and HeldLockResumeDialog offered
    // an irreversible "Release write lock" for work it had never actually checked.
    invokeIpcMock.mockReset();
    // One resolver per in-flight call, so each fetch can be settled on demand.
    const pending: Array<(states: SpeleoLocalStateMap) => void> = [];
    invokeIpcMock.mockImplementation(
      () => new Promise<SpeleoLocalStateMap>((resolve) => pending.push(resolve)),
    );

    const { result, rerender } = renderHook(({ ids }: { ids: string[] }) => useSpeleoLocalStates(ids), {
      initialProps: { ids: [CLEAN] },
    });
    expect(result.current.loaded).toBe(false);
    await act(async () => pending[0]?.({ [CLEAN]: STATES[CLEAN]! }));
    expect(result.current.loaded).toBe(true);

    // The set grows: until the answer covering DIRTY arrives, callers must be told
    // we do not know yet.
    rerender({ ids: [CLEAN, DIRTY] });
    expect(result.current.loaded).toBe(false);

    await act(async () => pending[1]?.(STATES));
    expect(result.current.loaded).toBe(true);
    expect(result.current.states).toEqual(STATES);
  });

  it('re-issues the call on reload(), even for an unchanged id set', async () => {
    // The GlobalView refresh button re-scans the LOCKS; the un-pushed accent is
    // LOCAL state, which the effect keys on the id SET - so over an unchanged
    // recents list nothing re-issued it and the accent stayed at mount-time state.
    invokeIpcMock.mockReset();
    invokeIpcMock.mockResolvedValue({});
    const { result } = renderHook(() => useSpeleoLocalStates([DIRTY]));
    await waitFor(() => expect(result.current.loaded).toBe(true));
    const callsOf = () =>
      invokeIpcMock.mock.calls.filter((c) => c[0] === 'speleodb_local_project_states').length;
    expect(callsOf()).toBe(1);

    invokeIpcMock.mockResolvedValue(STATES);
    await act(async () => {
      result.current.reload();
    });
    await waitFor(() => expect(result.current.states).toEqual(STATES));
    expect(callsOf()).toBe(2);
  });
});
