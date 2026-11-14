/**
 * #737: the adjustment report belongs to ONE adjusted geometry. It is shown
 * only while that geometry is on screen (an edit, undo or project switch that
 * clears or replaces `stationsAdjusted` hides it at once), a late answer for an
 * older solve never overwrites a newer one, and a failed part refreshes the
 * Warnings tab, whose entry is read from the same solve.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';

const invokeIpc = vi.hoisted(() => vi.fn());
vi.mock('../utils/ipc', () => ({ invokeIpc, getErrorMessage: String }));

import { useCurrentAdjustmentReport } from './useCurrentAdjustmentReport';
import { useAdjustmentReportStore } from '../store/adjustmentReportStore';
import { useProjectStore } from '../store/projectStore';
import type { AdjustmentReport } from '../types/ipc/AdjustmentReport';
import type { Station } from '../types/project';

const skippedOne: AdjustmentReport = {
  skipped_closures: [
    {
      closure_id: 9,
      station_a: { section_id: 0, station_index: 1 },
      station_b: { section_id: 0, station_index: 2 },
      shot_section_id: 0,
      shot_index: 1,
    },
  ],
  failures: [],
};
const failedOne: AdjustmentReport = {
  skipped_closures: [],
  failures: [{ anchor: { section_id: 2, station_index: 0 }, section_ids: [2, 3], reason: 'x' }],
};

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

describe('useCurrentAdjustmentReport', () => {
  beforeEach(() => {
    invokeIpc.mockReset();
    useAdjustmentReportStore.getState().clear();
    useProjectStore.setState({ stationsAdjusted: null } as never);
  });

  it('shows the report only for the adjusted geometry it was fetched for', async () => {
    const solved: Station[] = [];
    useProjectStore.setState({ stationsAdjusted: solved } as never);
    invokeIpc.mockResolvedValue(skippedOne);
    const { result } = renderHook(() => useCurrentAdjustmentReport());
    await act(() => useAdjustmentReportStore.getState().load(solved));
    expect(invokeIpc).toHaveBeenCalledWith('get_adjustment_report', undefined, expect.anything());
    expect(result.current).toEqual(skippedOne);

    // An edit clears the adjusted geometry: the report goes with it.
    act(() => useProjectStore.setState({ stationsAdjusted: null } as never));
    expect(result.current).toBeNull();
    // A new solve that has not been reported on yet shows nothing stale.
    act(() => useProjectStore.setState({ stationsAdjusted: [] as Station[] } as never));
    expect(result.current).toBeNull();
  });

  it('drops a late answer for an older solve', async () => {
    const older: Station[] = [];
    const newer: Station[] = [];
    const first = deferred<AdjustmentReport>();
    invokeIpc.mockReturnValueOnce(first.promise).mockResolvedValueOnce(failedOne);
    useProjectStore.setState({ stationsAdjusted: newer } as never);
    const { result } = renderHook(() => useCurrentAdjustmentReport());

    const slow = useAdjustmentReportStore.getState().load(older);
    await act(() => useAdjustmentReportStore.getState().load(newer));
    first.resolve(skippedOne);
    await act(() => slow);

    expect(useAdjustmentReportStore.getState().forStations).toBe(newer);
    expect(result.current).toEqual(failedOne);
  });

  it('refreshes the warnings when a failed part appears or goes, and only then', async () => {
    const solved: Station[] = [];
    const onFailures = vi.fn();
    invokeIpc
      .mockResolvedValueOnce(skippedOne)
      .mockResolvedValueOnce(failedOne)
      .mockResolvedValueOnce(skippedOne)
      .mockResolvedValueOnce(skippedOne);
    await useAdjustmentReportStore.getState().load(solved, onFailures);
    expect(onFailures).not.toHaveBeenCalled();
    // A failure appears: the Warnings tab gains its entry.
    await useAdjustmentReportStore.getState().load(solved, onFailures);
    await waitFor(() => expect(onFailures).toHaveBeenCalledTimes(1));
    // It goes (the data was fixed): the tab drops it.
    await useAdjustmentReportStore.getState().load(solved, onFailures);
    expect(onFailures).toHaveBeenCalledTimes(2);
    // Nothing failed before or now: no refresh.
    await useAdjustmentReportStore.getState().load(solved, onFailures);
    expect(onFailures).toHaveBeenCalledTimes(2);
  });

  it('forgets the report when the query fails', async () => {
    const solved: Station[] = [];
    useProjectStore.setState({ stationsAdjusted: solved } as never);
    invokeIpc.mockRejectedValue(new Error('gone'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { result } = renderHook(() => useCurrentAdjustmentReport());
    await act(() => useAdjustmentReportStore.getState().load(solved));
    expect(result.current).toBeNull();
    warn.mockRestore();
  });
});
