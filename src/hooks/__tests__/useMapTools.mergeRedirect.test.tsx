/**
 * Demo finding I-9, the merge tool's two notices:
 * - the "Checking merge compatibility..." toast lingered behind the dialog the
 *   check opened; it now leaves when the check is over (a merge dialog, the
 *   closure dialog or a refusal);
 * - a merge of two stations that are already connected opens the closure
 *   dialog marked `fromMerge`, so it can say why a loop closure is offered.
 * Injection: dropping the `dismissToastsByKey` call from the check's `finally`
 * fails the toast assertions; dropping `fromMerge` fails the redirect one.
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

import '../../i18n';
import { useMapTools } from '../useMapTools';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
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

const liveChecks = () =>
  useUIStore.getState().toasts.filter((t) => t.key === TOAST_KEYS.MERGE_CHECK && !t.leaving);

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({
    project: { lines: [], file_path: '/a.stalx', created_at: 't', modified_at: 't' } as never,
    surveyData: null,
    stations: [station(1, 0, 10), station(1, 1, 11), station(2, 0, 20), station(2, 1, 21)],
    stationsAdjusted: null,
    surveyDataAdjusted: null,
  } as never);
  useUIStore.setState({
    activeTool: 'merge',
    showStations: true,
    mergeStationA: null,
    mergeStationB: null,
    activeDialog: null,
    dialogData: {},
    toasts: [],
  } as never);
});

async function mergeTwo(result: { current: ReturnType<typeof useMapTools> }) {
  act(() => {
    result.current.onStationClick?.(slot(1, 1), clickEvent);
  });
  await act(async () => {
    result.current.onStationClick?.(slot(2, 1), clickEvent);
  });
}

describe('merge tool notices (I-9)', () => {
  it('opens the closure dialog marked as a merge, and the check toast leaves', async () => {
    let answer: (value: unknown) => void = () => undefined;
    invokeIpcMock.mockImplementation(
      () => new Promise((resolve) => {
        answer = resolve;
      }),
    );
    const { result } = renderHook(() => useMapTools());
    await mergeTwo(result);
    // While the check runs, it says so.
    expect(liveChecks()).toHaveLength(1);
    await act(async () => {
      answer({ connected: true, can_merge: false, preview: null, reason: 'Creates survey loop', closure_refusal: null });
    });
    expect(useUIStore.getState().activeDialog).toBe('closure');
    expect(useUIStore.getState().dialogData).toMatchObject({ fromMerge: true });
    expect(liveChecks()).toHaveLength(0);
  });

  it('lets the check toast leave when the merge dialog opens', async () => {
    invokeIpcMock.mockResolvedValue({ connected: false, can_merge: true, preview: null, reason: null });
    const { result } = renderHook(() => useMapTools());
    await mergeTwo(result);
    expect(useUIStore.getState().activeDialog).toBe('merge');
    expect(liveChecks()).toHaveLength(0);
  });
});
