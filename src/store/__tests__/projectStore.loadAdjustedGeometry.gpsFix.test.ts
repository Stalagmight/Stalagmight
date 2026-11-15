/**
 * #741: a project whose only reason to adjust is a GPS fix still gets the
 * Closures Computed View. `loadAdjustedGeometry` used to skip the backend solve
 * whenever there was no eligible closure, which would have shown the Raw Data
 * View's geometry under the Computed View's name.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => String(e),
}));

import { useProjectStore } from '../projectStore';

const GPS = { latitude: 1, longitude: 2, depth_reference: 0 };

function seed(roles: Array<'Placement' | 'GpsFix'>) {
  useProjectStore.setState({
    project: {
      loop_closures: [],
      zero_datums: roles.map((role, i) => ({ station_ref: { section_id: 1, station_index: i }, gps: GPS, role })),
      sections: [],
    },
    stations: [],
    surveyData: null,
    adjustedLoading: false,
  } as never);
}

describe('loadAdjustedGeometry with GPS fixes (#741)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue([]);
  });

  it('asks the backend for the solve when a GPS fix is the only input', async () => {
    seed(['Placement', 'GpsFix']);
    await useProjectStore.getState().loadAdjustedGeometry();
    expect(invokeIpcMock).toHaveBeenCalledWith('get_adjusted_stations', undefined, expect.anything());
  });

  it('still skips the round-trip with only placement datums and no closure', async () => {
    seed(['Placement']);
    await useProjectStore.getState().loadAdjustedGeometry();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('get_adjusted_stations', expect.anything(), expect.anything());
  });
});
