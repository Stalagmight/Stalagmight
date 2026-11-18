/**
 * `createLineAndMerge` forwards the New Line dialog's Section (#327) to
 * `create_line_and_merge`, so a line placed by snapping onto a station joins
 * its Section inside the one undo step (the backend's
 * `a_snapped_new_line_joins_its_section_in_one_undo_step`), instead of a
 * second `move_lines_to_cave_section` step.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => String(e),
}));

import { useProjectStore } from '../projectStore';
import type { LineData } from '../../types/project';

const DATA = { name: 'Snapped', color: '#123456', shots: [], quality: 1 } as unknown as LineData;
const TARGET = { line_id: 4, station_index: 2 };

function argsOf(cmd: string): Record<string, unknown> | null {
  const call = invokeIpcMock.mock.calls.find((c) => c[0] === cmd);
  return (call?.[1] as Record<string, unknown> | undefined) ?? null;
}

describe('projectStore.createLineAndMerge', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue({ success: true, line_id: 9 });
    useProjectStore.setState({
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
      checkProjectIntegrity: vi.fn().mockResolvedValue(undefined),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
  });

  it('sends the Section with the create-and-merge, in one command', async () => {
    await useProjectStore.getState().createLineAndMerge(DATA, 0, TARGET, 'Move', 3);
    expect(argsOf('create_line_and_merge')).toMatchObject({
      importStationIndex: 0,
      targetStation: TARGET,
      caveSectionId: 3,
    });
    expect(argsOf('move_lines_to_cave_section')).toBeNull();
  });

  it('sends null when there is no Section', async () => {
    await useProjectStore.getState().createLineAndMerge(DATA, 0, TARGET, 'Move');
    expect(argsOf('create_line_and_merge')?.caveSectionId).toBeNull();
  });
});
