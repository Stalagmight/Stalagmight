/**
 * RS 7 (U-P2-7): the Station details' one-click buttons open their dialog
 * whenever the preview says something comes back or a name would repeat: the
 * "Also on" detach button opens Detach Line from Station on that line, the
 * Closures section's remove opens Remove Closure on that closure. Otherwise
 * they act at once. The Closures section also says which end a closure hides.
 * The stations stat says loop ends count.
 * Injection: calling the command straight from the button fails every
 * "opens" case.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import DetailPanel from '../DetailPanel';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import type { Project, Line, Shot } from '../../../types/project';
import type { Station } from '../../../types/ipc/Station';

function shot(id: number): Shot {
  return {
    id, azimuth_in: 90, azimuth_out: 90, length: 5, depth_in: 1, depth_out: 2,
    pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
    comment: null, kind: 'Normal', marker_index: null, temperature: null,
  } as Shot;
}
function line(id: number, name: string, shots: number[]): Line {
  return { id, name, color: '#336699', direction: 'in', quality: 1, next_shot_id: 9, shots: shots.map(shot) } as unknown as Line;
}

const MAIN = 1;
const SIDE = 2;
const ref = (line_id: number, station_index: number) => ({ line_id, station_index });
const unmergeStationPair = vi.fn();
const removeLoopClosure = vi.fn();
const openDialog = vi.fn();

function stations(): Station[] {
  const plain = (line_id: number, i: number, id: number, name: string): Station => ({
    station_ref: ref(line_id, i), coordinates: { x: i, y: 0, z: 0 }, is_zero_datum: false,
    gps_coordinates: null, merged_with: [], id, name,
  });
  return [
    plain(MAIN, 0, 10, '2'),
    {
      ...plain(MAIN, 1, 11, '3a'),
      merged_with: [ref(SIDE, 0)],
      archived: [{ id: 20, name: '0', lines: [SIDE] }],
      closures_with: [{ closure_id: 5n, station_ref: ref(MAIN, 2), is_transitive: false }],
    },
    plain(MAIN, 2, 12, '4'),
    {
      ...plain(SIDE, 0, 11, '3a'),
      merged_with: [ref(MAIN, 1)],
      archived: [{ id: 20, name: '0', lines: [SIDE] }],
      shown: { id: 11, home_line: MAIN, foreign: true, loop_end: false },
    },
    plain(SIDE, 1, 22, '1'),
  ];
}

beforeEach(() => {
  vi.clearAllMocks();
  unmergeStationPair.mockResolvedValue({ success: true });
  removeLoopClosure.mockResolvedValue({ success: true });
  const lines = [line(MAIN, 'Main', [101, 102]), line(SIDE, 'Side', [201])];
  const loop_closures = [
    {
      id: 5n, station_a: ref(MAIN, 1), station_b: ref(MAIN, 2), path_length: 10, misclosure_distance: 0.5,
      error_percentage: 5, horizontal_misclosure: 0.5, vertical_misclosure: 0, strategy: 'Batch', is_aligned: false,
      archived: { end: 'b', station: { id: 12, name: '4', home_line: MAIN }, shown_as: { id: 11, name: '3a', home_line: MAIN } },
    },
  ];
  const project = { lines, station_merges: [], zero_datums: [], loop_closures } as unknown as Project;
  const list = stations();
  useProjectStore.setState({
    project, stations: list, surveyData: surveyDataFromLegacy(lines, list),
    warnings: [], stats: null, config: null, unmergeStationPair, removeLoopClosure,
  } as never);
  useUIStore.setState({ detailPanelTab: 'details', appMode: 'survey', geometryMode: 'measured', multiSelection: [], openDialog });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useUIStore.getState().setSelection({ type: 'station', stationRef: ref(MAIN, 1) });
});

describe('the Station details entry points (RS 7)', () => {
  it("opens Detach Line from Station on the row's line when its own station comes back", async () => {
    invokeIpc.mockResolvedValue({ name: '0', station_id: 20, restored: true, kept: false, lines: [], together: [], rest_choices: [], collisions: [] });
    render(<DetailPanel />);
    await act(async () => {
      fireEvent.click(screen.getByTestId(`detail-unmerge-${SIDE}-0`));
    });
    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('unmerge-stations', {
        stationRef: ref(MAIN, 1),
        mergedStations: [ref(SIDE, 0)],
        chosenLine: SIDE,
      }),
    );
    expect(unmergeStationPair).not.toHaveBeenCalled();
  });

  it('detaches at once when the line gets a new station and nothing collides', async () => {
    invokeIpc.mockResolvedValue({ name: '3a', station_id: 60, restored: false, kept: false, lines: [], together: [], rest_choices: [], collisions: [] });
    render(<DetailPanel />);
    await act(async () => {
      fireEvent.click(screen.getByTestId(`detail-unmerge-${SIDE}-0`));
    });
    await waitFor(() => expect(unmergeStationPair).toHaveBeenCalledWith(ref(MAIN, 1), ref(SIDE, 0)));
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('says which end a closure hides, and opens Remove Closure when it comes back', async () => {
    invokeIpc.mockResolvedValue({
      closure_id: 5n, comes_back: { id: 12, name: '4', home_line: MAIN }, renamed_lines: [MAIN], collisions: [],
    });
    render(<DetailPanel />);
    expect(screen.getByTestId('closure-archive-5')).toHaveTextContent(
      'Main / 4 is shown as Main / 3a (loop end). Removing the closure brings it back.',
    );
    await act(async () => {
      fireEvent.click(screen.getByTestId('detail-remove-closure-5'));
    });
    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('remove-closure', {
        anchorStation: ref(MAIN, 1),
        closures: [{ closureId: 5n, stationRef: ref(MAIN, 2) }],
      }),
    );
    expect(removeLoopClosure).not.toHaveBeenCalled();
  });
});

describe('the stations stat (RS 6, U-P3-11)', () => {
  it('says loop closure ends count as stations', () => {
    useProjectStore.setState({
      stats: {
        total_length: 10, penetration_distance: 10, line_count: 2, shot_count: 3, station_count: 5,
        loop_count: 1, junction_count: 1, min_depth: 0, max_depth: 2,
      },
    } as never);
    useUIStore.getState().setSelection(null);
    render(<DetailPanel />);
    expect(screen.getByTestId('project-stats-stations')).toHaveAttribute(
      'title',
      'Every surveyed point counts once. The two ends of a loop closure count as two, even when they show one name.',
    );
  });
});
