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
import { STATION_FLAG_IS_GPS_FIX } from '../../types/survey-data';

const GPS = { latitude: 1, longitude: 2, depth_reference: 0 };

function seed(roles: Array<'Placement' | 'GpsFix'>) {
  useProjectStore.setState({
    project: {
      loop_closures: [],
      zero_datums: roles.map((role, i) => ({ station_ref: { line_id: 1, station_index: i }, gps: GPS, role })),
      lines: [],
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

  it('flags the fix station on the computed geometry, from the wire role', async () => {
    seed(['Placement', 'GpsFix']);
    const station = (station_index: number) => ({
      station_ref: { line_id: 1, station_index },
      coordinates: { x: 0, y: station_index, z: 0 },
      is_zero_datum: true,
      gps_coordinates: GPS,
      merged_with: [],
    });
    const empty = { skipped_closures: [], failures: [], far_gps_fixes: [] };
    invokeIpcMock.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === 'get_adjusted_stations' ? [station(0), station(1)] : empty)
    );
    await useProjectStore.getState().loadAdjustedGeometry();
    const flags = useProjectStore.getState().surveyDataAdjusted?.stationFlags;
    expect(flags && [...flags].map((f) => (f & STATION_FLAG_IS_GPS_FIX) !== 0)).toEqual([false, true]);
  });

  it('still skips the round-trip with only placement datums and no closure', async () => {
    seed(['Placement']);
    await useProjectStore.getState().loadAdjustedGeometry();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('get_adjusted_stations', expect.anything(), expect.anything());
  });
});
