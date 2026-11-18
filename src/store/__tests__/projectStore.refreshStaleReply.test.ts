/**
 * A `get_project_state` reply that lands after the open project changed must
 * not be applied to the project that replaced it (#24's `created_at` identity),
 * and must never prune the hidden-Sections setting (#327): the prune SAVES
 * through `save_display_setting`, which writes into whatever the backend has
 * open, so a stale reply would drop valid hidden ids and persist that, or write
 * one project's list into another.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => String(e),
}));

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { HIDDEN_SECTIONS_SETTING } from '../sectionsViewSlice';

type Sections = Array<{ id: number; name: string; color: string }>;

function projectState(createdAt: string, sections: Sections) {
  return {
    project: { name: createdAt, created_at: createdAt, lines: [], zero_datums: [], cave_sections: sections },
    stations: [],
    stats: null,
    frozen_shots: [],
  };
}

/** The store project the tests open. */
function storeProject(createdAt: string) {
  return { name: createdAt, created_at: createdAt, lines: [], cave_sections: [] } as never;
}

/** The hidden Section ids (the store field is named like the setting key). */
function hidden(): unknown {
  return (useUIStore.getState() as unknown as Record<string, unknown>)[HIDDEN_SECTIONS_SETTING];
}

function savedSettings(): unknown[] {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === 'save_display_setting').map((c) => c[1]);
}

/** Make the next `get_project_state` wait until the returned function answers it. */
function holdNextStateReply(): (reply: unknown) => void {
  let answer!: (reply: unknown) => void;
  const pending = new Promise((resolve) => {
    answer = resolve;
  });
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'get_project_state') return pending;
    if (cmd === 'get_project_warnings') return [];
    return null;
  });
  return answer;
}

describe('refreshProjectState and a reply for another project (#327, #24)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useProjectStore.setState({
      project: storeProject('A'),
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
    } as never);
    useUIStore.setState({ [HIDDEN_SECTIONS_SETTING]: [1, 2] } as never);
  });

  it('drops a reply that lands after another project opened', async () => {
    const answer = holdNextStateReply();
    const refresh = useProjectStore.getState().refreshProjectState();
    // B opens while A's refresh is in flight.
    useProjectStore.setState({ project: storeProject('B') } as never);
    answer(projectState('A', [{ id: 1, name: 'Upper', color: '#111111' }]));
    await refresh;

    expect(useProjectStore.getState().project?.created_at).toBe('B');
    expect(hidden()).toEqual([1, 2]);
    expect(savedSettings()).toEqual([]);
  });

  it('drops a reply that lands after the project closed', async () => {
    const answer = holdNextStateReply();
    const refresh = useProjectStore.getState().refreshProjectState();
    useProjectStore.setState({ project: null } as never);
    answer(projectState('A', [{ id: 1, name: 'Upper', color: '#111111' }]));
    await refresh;

    expect(useProjectStore.getState().project).toBeNull();
    expect(savedSettings()).toEqual([]);
  });

  it('never prunes against a project the backend switched to first', async () => {
    // The backend already holds B (its load reply is not in the store yet):
    // B's Sections say nothing about A's hidden list.
    const answer = holdNextStateReply();
    const refresh = useProjectStore.getState().refreshProjectState();
    answer(projectState('B', [{ id: 7, name: 'Other', color: '#222222' }]));
    await refresh;

    expect(hidden()).toEqual([1, 2]);
    expect(savedSettings()).toEqual([]);
  });

  it('still prunes, and saves, for the project it was issued for', async () => {
    const answer = holdNextStateReply();
    const refresh = useProjectStore.getState().refreshProjectState();
    answer(projectState('A', [{ id: 1, name: 'Upper', color: '#111111' }]));
    await refresh;

    expect(hidden()).toEqual([1]);
    expect(savedSettings()).toEqual([{ key: HIDDEN_SECTIONS_SETTING, value: '[1]' }]);
  });

  it('installs the first project into an empty store', async () => {
    useProjectStore.setState({ project: null } as never);
    const answer = holdNextStateReply();
    const refresh = useProjectStore.getState().refreshProjectState();
    answer(projectState('A', [{ id: 1, name: 'Upper', color: '#111111' }, { id: 2, name: 'Lower', color: '#222222' }]));
    await refresh;

    expect(useProjectStore.getState().project?.created_at).toBe('A');
  });
});
