/**
 * Move Comment tool: a second click on the holder whose comment is picked up
 * cancels the pickup (#576, A16). It must open no swap dialog, send no IPC and
 * leave the UI exactly as the tool's own cancel does (the empty-map click in
 * `handleEmptyClick`'s `move_comment` case).
 *
 * The controls pin what "same holder" means: the StationRef for a station, the
 * (section id, shot id) pair for a shot. A different station at the same map
 * position, or a shot whose ids happen to equal the picked-up station's ref, is
 * a different holder and still gets the swap dialog.
 *
 * NOTE: the `.tsx` extension is load-bearing - only `*.test.tsx` runs in jsdom,
 * which `renderHook` needs.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import type { MapMouseEvent } from 'maplibre-gl';

vi.mock('../../utils/ipc');
stubGetErrorMessage(messageOr('unknown error'));

import { useMapTools } from '../useMapTools';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import type { Project, Section, Station } from '../../types/project';
import { invokeIpcMock, stubGetErrorMessage, messageOr } from '../../test/factories/ipc';
import { makeProject as projectOf, makeStation as stationOf } from '../../test/factories/survey';

const SECTION_ID = 1;
const SHOT_A = 1;
const SHOT_B = 2;

function makeStation(station_index: number, x: number, comment?: string): Station {
  return stationOf({
    station_ref: { section_id: SECTION_ID, station_index },
    coordinates: { x, y: 0, z: 0 },
    closures_with: [],
    ...(comment !== undefined ? { comment } : {}),
  });
}

function makeProject(): Project {
  const shot = (id: number, comment: string | null) => ({ id, comment });
  return projectOf({
    sections: [
      {
        id: SECTION_ID,
        shots: [shot(SHOT_A, 'low airspace'), shot(SHOT_B, 'restriction')],
      } as unknown as Section,
    ],
    file_path: '/mock/a.stalx',
  });
}

// Station 1 and station 2 sit at the same map position (x = 10), both with a
// comment: a click on one of them is still a click on that ref only.
const STATIONS: Station[] = [
  makeStation(0, 0),
  makeStation(1, 10, 'sump'),
  makeStation(2, 10, 'silt'),
];

const clickEvent = { originalEvent: new MouseEvent('click') } as unknown as MapMouseEvent;

/** The UI state a cancel could touch. */
function uiSnapshot() {
  const s = useUIStore.getState();
  return {
    activeTool: s.activeTool,
    moveCommentSource: s.moveCommentSource,
    activeDialog: s.activeDialog,
    dialogData: s.dialogData,
    selection: s.selection,
    multiSelection: s.multiSelection,
    commentTarget: s.commentTarget,
    toasts: s.toasts.map((t) => t.key),
  };
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({
    project: makeProject(),
    stations: STATIONS,
    stationsAdjusted: null,
    surveyDataAdjusted: null,
  });
  useUIStore.setState({
    activeTool: 'move_comment',
    showStations: true,
    showShots: true,
    snappedStation: null,
    moveCommentSource: null,
    activeDialog: null,
    dialogData: {},
    selection: null,
    multiSelection: [],
    commentTarget: null,
    toasts: [],
  });
});

describe('Move Comment: second click on the picked-up holder (#576, A16)', () => {
  it('a station: cancels the pickup like an empty-map click, with no swap dialog and no IPC', async () => {
    const { result } = renderHook(() => useMapTools());

    // Reference: the tool's own cancel (pick up, then click empty map).
    await act(async () => {
      await result.current.onStationClick?.({ section_id: SECTION_ID, station_index: 1 }, clickEvent);
    });
    act(() => {
      result.current.onEmptyClick?.(clickEvent);
    });
    const cancelledByEmptyClick = uiSnapshot();
    expect(cancelledByEmptyClick.moveCommentSource).toBeNull();

    // Same pickup, then a second click on the same station.
    await act(async () => {
      await result.current.onStationClick?.({ section_id: SECTION_ID, station_index: 1 }, clickEvent);
    });
    expect(useUIStore.getState().moveCommentSource).toMatchObject({
      type: 'station',
      stationRef: { section_id: SECTION_ID, station_index: 1 },
      comment: 'sump',
    });
    await act(async () => {
      await result.current.onStationClick?.({ section_id: SECTION_ID, station_index: 1 }, clickEvent);
    });

    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(useUIStore.getState().moveCommentSource).toBeNull();
    expect(uiSnapshot()).toEqual(cancelledByEmptyClick);
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().stations[1]?.comment).toBe('sump');
  });

  it('a shot: cancels the pickup like an empty-map click, with no swap dialog and no IPC', async () => {
    const { result } = renderHook(() => useMapTools());

    await act(async () => {
      await result.current.onShotClick?.(SECTION_ID, SHOT_A, clickEvent);
    });
    act(() => {
      result.current.onEmptyClick?.(clickEvent);
    });
    const cancelledByEmptyClick = uiSnapshot();
    expect(cancelledByEmptyClick.moveCommentSource).toBeNull();

    await act(async () => {
      await result.current.onShotClick?.(SECTION_ID, SHOT_A, clickEvent);
    });
    expect(useUIStore.getState().moveCommentSource).toMatchObject({
      type: 'shot',
      sectionId: SECTION_ID,
      shotId: SHOT_A,
      comment: 'low airspace',
    });
    await act(async () => {
      await result.current.onShotClick?.(SECTION_ID, SHOT_A, clickEvent);
    });

    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(useUIStore.getState().moveCommentSource).toBeNull();
    expect(uiSnapshot()).toEqual(cancelledByEmptyClick);
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().project?.sections[0]?.shots[0]?.comment).toBe('low airspace');
  });

  it('control: a different station at the same map position still opens the swap dialog', async () => {
    const { result } = renderHook(() => useMapTools());
    await act(async () => {
      await result.current.onStationClick?.({ section_id: SECTION_ID, station_index: 1 }, clickEvent);
    });
    await act(async () => {
      await result.current.onStationClick?.({ section_id: SECTION_ID, station_index: 2 }, clickEvent);
    });

    const s = useUIStore.getState();
    expect(s.activeDialog).toBe('swap-comments');
    expect(s.dialogData.target).toEqual({
      type: 'station',
      stationRef: { section_id: SECTION_ID, station_index: 2 },
      comment: 'silt',
    });
    expect(s.moveCommentSource).not.toBeNull();
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('control: a shot whose ids equal the picked-up station ref is a different holder', async () => {
    const { result } = renderHook(() => useMapTools());
    await act(async () => {
      await result.current.onStationClick?.({ section_id: SECTION_ID, station_index: 1 }, clickEvent);
    });
    // Shot (section 1, shot id 1) vs station (section 1, index 1).
    await act(async () => {
      await result.current.onShotClick?.(SECTION_ID, SHOT_A, clickEvent);
    });

    const s = useUIStore.getState();
    expect(s.activeDialog).toBe('swap-comments');
    expect(s.dialogData.target).toEqual({
      type: 'shot',
      sectionId: SECTION_ID,
      shotId: SHOT_A,
      comment: 'low airspace',
    });
  });

  it('control: another shot in the same section still opens the swap dialog', async () => {
    const { result } = renderHook(() => useMapTools());
    await act(async () => {
      await result.current.onShotClick?.(SECTION_ID, SHOT_A, clickEvent);
    });
    await act(async () => {
      await result.current.onShotClick?.(SECTION_ID, SHOT_B, clickEvent);
    });

    const s = useUIStore.getState();
    expect(s.activeDialog).toBe('swap-comments');
    expect(s.dialogData.target).toEqual({
      type: 'shot',
      sectionId: SECTION_ID,
      shotId: SHOT_B,
      comment: 'restriction',
    });
  });
});
