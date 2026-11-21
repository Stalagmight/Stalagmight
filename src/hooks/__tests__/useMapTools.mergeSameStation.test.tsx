/**
 * The merge tool on the station graph (plan 9.3): a junction is ONE station,
 * so its slots share one Station ID. Picking a junction's other slot as the
 * second station is picking the same station again: it clears the pick, as
 * clicking the same slot does, instead of asking the backend to merge a
 * station with itself.
 *
 * `.tsx` routes this file to jsdom (`environmentMatchGlobs`).
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import type { MapMouseEvent } from 'maplibre-gl';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown, fallback?: string) =>
    (e as { message?: string })?.message ?? fallback ?? 'unknown error',
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue({}) }));

import { useMapTools } from '../useMapTools';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import type { Station } from '../../types/project';

const slot = (line_id: number, station_index: number) => ({ line_id, station_index });
const station = (line_id: number, station_index: number, id: number): Station => ({
  station_ref: slot(line_id, station_index),
  coordinates: { x: station_index * 10, y: line_id, z: 0 },
  is_zero_datum: false,
  gps_coordinates: null,
  merged_with: [],
  id,
  name: String(id),
});

const clickEvent = { originalEvent: new MouseEvent('click') } as unknown as MapMouseEvent;

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue({ connected: false, can_merge: true, preview: null, reason: null });
  useProjectStore.setState({
    project: { lines: [], file_path: '/a.stalx', created_at: 't', modified_at: 't' } as never,
    surveyData: null,
    // Line 1's station 1 and line 2's station 0 are one station (id 40).
    stations: [station(1, 0, 10), station(1, 1, 40), station(2, 0, 40), station(2, 1, 41)],
    stationsAdjusted: null,
    surveyDataAdjusted: null,
  } as never);
  useUIStore.setState({ activeTool: 'merge', showStations: true, mergeStationA: null, mergeStationB: null } as never);
});

describe('merge tool, same station', () => {
  it("treats a junction's other slot as the same station and clears the pick", () => {
    const { result } = renderHook(() => useMapTools());
    act(() => {
      result.current.onStationClick?.(slot(1, 1), clickEvent);
    });
    expect(useUIStore.getState().mergeStationA).toEqual(slot(1, 1));
    act(() => {
      result.current.onStationClick?.(slot(2, 0), clickEvent);
    });
    expect(useUIStore.getState().mergeStationA).toBeNull();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('check_merge_connectivity', expect.anything(), expect.anything());
  });

  it('still checks two different stations', async () => {
    const { result } = renderHook(() => useMapTools());
    act(() => {
      result.current.onStationClick?.(slot(1, 0), clickEvent);
    });
    await act(async () => {
      result.current.onStationClick?.(slot(2, 1), clickEvent);
    });
    expect(invokeIpcMock).toHaveBeenCalledWith('check_merge_connectivity', expect.anything(), expect.anything());
  });
});

/**
 * Demo issue I-4: every merge-tool pick that can merge opens the Merge dialog
 * with the backend's preview of what the merge does (no merge-tool path skips
 * the dialog). Injection: drop `mergePreview` from the merge tool's
 * openDialog and the dialog has nothing to say.
 */
describe('merge tool, the preview', () => {
  it('hands the backend preview to the Merge dialog', async () => {
    const mergePreview = { moving_line_ids: [2], shift_m: 24.3, joins: slot(1, 0), depths: { a_m: 15, b_m: 4, joined_m: 9.5 } };
    invokeIpcMock.mockResolvedValue({ connected: false, can_merge: true, preview: null, reason: null, merge_preview: mergePreview });
    const { result } = renderHook(() => useMapTools());
    act(() => {
      result.current.onStationClick?.(slot(1, 0), clickEvent);
    });
    await act(async () => {
      result.current.onStationClick?.(slot(2, 1), clickEvent);
    });
    expect(useUIStore.getState().activeDialog).toBe('merge');
    expect(useUIStore.getState().dialogData.mergePreview).toEqual(mergePreview);
  });
});
