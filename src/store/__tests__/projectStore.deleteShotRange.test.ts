import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

describe('projectStore range deletion', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue({ success: true });
    // Stub the post-command refresh so executeCommand's success path doesn't
    // fan out into the rest of the IPC surface.
    useProjectStore.setState({
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
      checkProjectIntegrity: vi.fn().mockResolvedValue(undefined),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
  });

  it('deleteShotRangeSplit invokes delete_shot_range_split with both section names', async () => {
    const result = await useProjectStore
      .getState()
      .deleteShotRangeSplit(1, 10, 13, 'Left', 'Right');

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'delete_shot_range_split',
      { sectionId: 1, startShotId: 10, endShotId: 13, nameA: 'Left', nameB: 'Right' },
      expect.objectContaining({ purpose: expect.any(String) })
    );
    expect(result.success).toBe(true);
  });

  it('deleteShotRangeShift invokes delete_shot_range_shift with the range endpoints', async () => {
    const result = await useProjectStore.getState().deleteShotRangeShift(2, 20, 22);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'delete_shot_range_shift',
      { sectionId: 2, startShotId: 20, endShotId: 22 },
      expect.objectContaining({ purpose: expect.any(String) })
    );
    expect(result.success).toBe(true);
  });

  it('refreshes project state after a successful range delete', async () => {
    await useProjectStore.getState().deleteShotRangeShift(2, 20, 22);
    expect(useProjectStore.getState().refreshProjectState).toHaveBeenCalled();
  });
});
