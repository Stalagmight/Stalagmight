import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

describe('projectStore range deletion', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue({ success: true });
    // Stub the post-command refresh so executeCommand's success path doesn't
    // fan out into the rest of the IPC surface.
    useProjectStore.setState({
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
  });

  it('deleteShotRangeSplit invokes delete_shot_range_split with both line names', async () => {
    const result = await useProjectStore
      .getState()
      .deleteShotRangeSplit(1, 10, 13, 'Left', 'Right');

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'delete_shot_range_split',
      { lineId: 1, startShotId: 10, endShotId: 13, nameA: 'Left', nameB: 'Right' },
      expect.objectContaining({ purpose: expect.any(String) })
    );
    expect(result.success).toBe(true);
  });

  // Demo finding I-9: a Delete & Split left both halves in the line's
  // colour, while Split Line Here gives the second half the colour a new line
  // gets. Both delete-and-split actions now send that colour (the next palette
  // colour no line uses, the split line's own included). Injection: dropping
  // `colorB` from either call fails it.
  describe('the second half gets a new colour, as Split Line Here gives it', () => {
    beforeEach(() => {
      useUIStore.setState({ resolvedTheme: 'dark' } as never);
      useProjectStore.setState({
        project: {
          lines: [
            // Teal is the split line's (stored in capitals), Coral another line's.
            { id: 1, name: 'Main', color: '#4ECDC4', shots: [] },
            { id: 2, name: 'Side', color: '#ff6b6b', shots: [] },
          ],
        },
      } as never);
    });

    it('deleteShotAndSplit sends the next unused palette colour', async () => {
      await useProjectStore.getState().deleteShotAndSplit(1, 10, 'Main A', 'Main B');
      expect(invokeIpcMock).toHaveBeenCalledWith(
        'delete_shot_and_split',
        { lineId: 1, shotId: 10, nameA: 'Main A', nameB: 'Main B', colorB: '#95e1d3' },
        expect.anything()
      );
    });

    it('deleteShotRangeSplit sends it too', async () => {
      await useProjectStore.getState().deleteShotRangeSplit(1, 10, 13, 'Left', 'Right');
      expect(invokeIpcMock.mock.calls[0]?.[1]).toMatchObject({ colorB: '#95e1d3' });
    });

    it('lets the backend keep the colour of a line it does not know', async () => {
      await useProjectStore.getState().deleteShotAndSplit(9, 10, 'A', 'B');
      expect(invokeIpcMock.mock.calls[0]?.[1]).toEqual({ lineId: 9, shotId: 10, nameA: 'A', nameB: 'B', colorB: undefined });
    });
  });

  it('deleteShotRangeShift invokes delete_shot_range_shift with the range endpoints', async () => {
    const result = await useProjectStore.getState().deleteShotRangeShift(2, 20, 22);

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'delete_shot_range_shift',
      { lineId: 2, startShotId: 20, endShotId: 22 },
      expect.objectContaining({ purpose: expect.any(String) })
    );
    expect(result.success).toBe(true);
  });

  it('refreshes project state after a successful range delete', async () => {
    await useProjectStore.getState().deleteShotRangeShift(2, 20, 22);
    expect(useProjectStore.getState().refreshProjectState).toHaveBeenCalled();
  });
});
