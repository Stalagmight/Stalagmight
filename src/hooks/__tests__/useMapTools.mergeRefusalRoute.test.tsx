/**
 * [#576 NF-W7E-1] A refused merge is routed on its code, not on its English text.
 *
 * The merge tool opens the loop-closure dialog for a pair the backend refuses to
 * merge because both merge groups are fixed by a zero datum, and a toast for any
 * other refusal. It decided that by comparing `ConnectivityResult.reason` with the
 * line "Both sections have fixed coordinates", so a reworded message sent a fixed
 * pair to the toast. It now reads `reason_code`; the text is only shown.
 *
 * `.tsx` so vitest runs it under jsdom (`renderHook`). All data is synthetic.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import type { MapMouseEvent } from 'maplibre-gl';

import { invokeIpcMock, stubGetErrorMessage, messageOr } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');
invokeIpcMock.mockResolvedValue(undefined);
stubGetErrorMessage(messageOr('error'));

import { useMapTools } from '../useMapTools';
import { refusedMergeOffersClosure } from '../../utils/mergeConnectivity';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import type { ConnectivityResult } from '../../types/commands';
import type { Project, StationRef } from '../../types/project';

const A: StationRef = { section_id: 1, station_index: 1 };
const B: StationRef = { section_id: 2, station_index: 0 };
const clickEvent = { originalEvent: new MouseEvent('click') } as unknown as MapMouseEvent;

/** The fixed-pair refusal, with its display text reworded. */
const FIXED_PAIR: ConnectivityResult = {
  connected: false,
  preview: null,
  can_merge: false,
  reason: 'Both merge groups are pinned by a zero datum',
  reason_code: 'BOTH_FIXED_COORDINATES',
};

beforeEach(() => {
  useProjectStore.setState({
    project: { name: 'CaveA', created_at: '2026-01-01T00:00:00Z', sections: [] } as unknown as Project,
    stations: [],
    checkMergeConnectivity: vi.fn().mockResolvedValue(FIXED_PAIR),
  });
  useUIStore.setState({
    activeTool: 'merge',
    showStations: true,
    mergeStationA: A,
    mergeStationB: null,
    activeDialog: null,
    dialogData: {},
    toasts: [],
  });
});

describe('merge refusal routing (#576 NF-W7E-1)', () => {
  it('offers the loop closure for a fixed pair whatever its reason text says', async () => {
    const { result } = renderHook(() => useMapTools());
    await act(async () => {
      await result.current.onStationClick?.(B, clickEvent);
    });
    // The connectivity check runs unawaited behind the click; let it answer.
    await vi.waitFor(() => expect(useProjectStore.getState().checkMergeConnectivity).toHaveBeenCalled());
    await act(async () => {});
    const { activeDialog, toasts } = useUIStore.getState();
    console.log(
      `[NF-W7E-1] fixed pair with reworded text: activeDialog=${activeDialog}; toasts=${JSON.stringify(
        toasts.map((toast) => toast.message),
      )}`,
    );
    expect(activeDialog).toBe('closure');
    expect(useUIStore.getState().dialogData.reason).toBe(FIXED_PAIR.reason);
  });

  it('decides on the code alone', () => {
    expect(refusedMergeOffersClosure({ connected: false, reason_code: 'BOTH_FIXED_COORDINATES' })).toBe(true);
    expect(refusedMergeOffersClosure({ connected: true, reason_code: 'CREATES_SURVEY_LOOP' })).toBe(true);
    expect(refusedMergeOffersClosure({ connected: false, reason_code: null })).toBe(false);
  });
});
