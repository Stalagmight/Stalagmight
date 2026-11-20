import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

describe('projectStore.updateStationName', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'set_station_name') return { success: true, warnings: [], section_ids: [] };
      return undefined;
    });
  });

  const stationRef = { section_id: 3, station_index: 2 };
  const nameCalls = () => invokeIpcMock.mock.calls.filter(([command]) => command === 'set_station_name');

  it('sends set_station_name with the camelCase top-level args the Tauri command takes', async () => {
    const result = await useProjectStore.getState().updateStationName(stationRef, 'A2');

    expect(result.success).toBe(true);
    expect(nameCalls()).toHaveLength(1);
    expect(nameCalls()[0]![1]).toEqual({ stationRef, name: 'A2' });
  });

  it('sends null to remove the name', async () => {
    await useProjectStore.getState().updateStationName(stationRef, null);

    expect(nameCalls()[0]![1]).toEqual({ stationRef, name: null });
  });
});
