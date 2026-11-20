/**
 * #327: the hidden-Sections display setting forgets a Section that no longer
 * exists, so a Section created later with the same id does not start hidden.
 * Pruned on every load and every refresh of the project, which is what a
 * Delete Section (and an Undo of a creation) ends in.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => String(e),
}));

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

let sections: Array<{ id: number; name: string; color: string }> = [];

function respond(cmd: string): unknown {
  switch (cmd) {
    case 'delete_cave_section':
      sections = sections.filter((s) => s.id !== 2);
      return { success: true, line_ids: [], warnings: [] };
    case 'get_project_state':
      return {
        project: { name: 'T', created_at: 'c', lines: [], zero_datums: [], cave_sections: sections },
        stations: [],
        stats: null,
        frozen_shots: [],
      };
    case 'get_project_warnings':
      return [];
    default:
      return null;
  }
}

describe('hidden Sections pruning (#327)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockImplementation(async (cmd: string) => respond(cmd));
    sections = [
      { id: 1, name: 'Upper', color: '#111111' },
      { id: 2, name: 'Lower', color: '#222222' },
    ];
    useProjectStore.setState({
      project: { name: 'T', created_at: 'c', lines: [], cave_sections: sections } as never,
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
    } as never);
    useUIStore.setState({ hiddenSections: [1, 2] });
  });

  it('a deleted Section leaves the hidden list, and the pruned list is saved', async () => {
    const result = await useProjectStore.getState().deleteCaveSection(2);
    expect(result.success).toBe(true);
    expect(useUIStore.getState().hiddenSections).toEqual([1]);
    const saved = invokeIpcMock.mock.calls.filter((c) => c[0] === 'save_display_setting').map((c) => c[1]);
    expect(saved).toEqual([{ key: 'hiddenSections', value: '[1]' }]);
  });

  it('a refresh that still holds every hidden Section changes nothing', async () => {
    await useProjectStore.getState().refreshProjectState();
    expect(useUIStore.getState().hiddenSections).toEqual([1, 2]);
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'save_display_setting')).toBe(false);
  });
});
