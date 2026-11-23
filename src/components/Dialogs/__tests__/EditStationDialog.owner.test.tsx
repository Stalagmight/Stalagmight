/**
 * RS 5.4 (U-P2-5): a rename or comment edit through a foreign slot or a loop
 * end changes the station the slot SHOWS, on every line it is on, so Edit
 * Station first says whose station it is and offers the way back to the
 * line's own: "12 belongs to Main. Rename it on every line / Detach Side to
 * get back its own station 0 / Cancel", or for a loop end "Shown as Main / 3
 * through a loop closure. Rename Main / 3 / Remove the closure / Cancel". A
 * native slot saves at once. The Station ID row reads the shown Station ID.
 * Injections: saving without the prompt fails "asks before renaming"; a
 * prompt on a native slot fails "saves a native slot at once".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: String }));
vi.mock('../../../hooks/useZeroDatumPreview', () => ({ useZeroDatumPreview: () => null }));

import '../../../i18n';
import EditStationDialog from '../EditStationDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { Project, Station } from '../../../types/project';
import type { StationOwner } from '../../../types/ipc/StationOwner';

const MAIN = 1;
const SIDE = 2;
const MUTATIONS = new Set(['set_station_name', 'set_station_comment']);
const mutations = () =>
  invokeIpcMock.mock.calls.filter(([command]) => MUTATIONS.has(command as string)).map(([command, args]) => [command, args]);

const SIDE0 = { line_id: SIDE, station_index: 0 };
const SIDE2 = { line_id: SIDE, station_index: 2 };
const MAIN2 = { line_id: MAIN, station_index: 2 };

const FOREIGN_OWNER: StationOwner = {
  station: SIDE0,
  node: 11,
  shown: { id: 11, name: '12', home_line: MAIN },
  lines: [MAIN, SIDE],
  foreign: true,
  own_station: { id: 40, name: '0', home_line: SIDE },
};
const LOOP_OWNER: StationOwner = {
  station: SIDE2,
  node: 22,
  shown: { id: 3, name: '3', home_line: MAIN },
  lines: [MAIN, SIDE],
  foreign: true,
  loop_closure: 9n,
};

function st(ref: { line_id: number; station_index: number }, id: number, name: string, extra: Partial<Station> = {}): Station {
  return {
    station_ref: ref, coordinates: { x: 0, y: 0, z: 0 }, is_zero_datum: false, gps_coordinates: null,
    merged_with: [], id, name, ...extra,
  };
}

function ipc(owner: StationOwner | null) {
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'preview_station_identity') return owner;
    if (MUTATIONS.has(command)) return { success: true, warnings: [], error: null, line_ids: [] };
    if (command === 'can_undo' || command === 'can_redo') return false;
    if (command === 'get_project_state') return null;
    if (command.startsWith('get_') || command.startsWith('validate')) return [];
    return null;
  });
}

function open(ref: { line_id: number; station_index: number }) {
  useUIStore.setState({ activeDialog: 'edit-station', dialogData: { stationRef: ref } });
  render(<EditStationDialog />);
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  ipc(FOREIGN_OWNER);
  useProjectStore.setState({
    project: {
      created_at: 'owner-prompt',
      lines: [
        { id: MAIN, name: 'Main', color: '#336699', shots: [{ id: 1 }, { id: 2 }] },
        { id: SIDE, name: 'Side', color: '#996633', shots: [{ id: 3 }, { id: 4 }] },
      ],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
    } as unknown as Project,
    surveyData: null,
    stations: [
      st({ line_id: MAIN, station_index: 0 }, 3, '3'),
      st({ line_id: MAIN, station_index: 1 }, 11, '12', { merged_with: [SIDE0] }),
      st(MAIN2, 13, '13'),
      st(SIDE0, 11, '12', {
        merged_with: [{ line_id: MAIN, station_index: 1 }],
        shown: { id: 11, home_line: MAIN, foreign: true, loop_end: false },
      }),
      st({ line_id: SIDE, station_index: 1 }, 21, '1'),
      st(SIDE2, 22, '3', { shown: { id: 3, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 9n } }),
    ],
    config: null,
  });
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', closeDialog: vi.fn(), openDialog: vi.fn() });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

async function save() {
  await act(async () => {
    fireEvent.click(screen.getByTestId('edit-station-save'));
  });
}

describe('Edit Station asks whose station it is (RS 5.4)', () => {
  it('asks before renaming through a foreign slot, and renames on going on', async () => {
    open(SIDE0);
    fireEvent.change(screen.getByTestId('station-name-input'), { target: { value: '12b' } });
    await save();
    expect(mutations()).toEqual([]);
    expect(await screen.findByTestId('shown-identity-prompt-lead')).toHaveTextContent('12 belongs to Main.');
    await waitFor(() =>
      expect(screen.getByTestId('shown-identity-prompt-lines')).toHaveTextContent(
        'A new name changes it on every line it is on: Main and Side.',
      ),
    );
    expect(screen.getByTestId('shown-identity-prompt-proceed')).toHaveTextContent('Rename it on every line');
    expect(screen.getByTestId('shown-identity-prompt-detach')).toHaveTextContent(
      'Detach Side to get back its own station 0',
    );
    await act(async () => {
      fireEvent.click(screen.getByTestId('shown-identity-prompt-proceed'));
    });
    await waitFor(() => expect(mutations()).toEqual([['set_station_name', { stationRef: SIDE0, name: '12b' }]]));
  });

  it('offers Detach Line from Station on the slot instead', async () => {
    open(SIDE0);
    fireEvent.change(screen.getByTestId('station-comment-input'), { target: { value: 'Arch' } });
    await save();
    fireEvent.click(await screen.findByTestId('shown-identity-prompt-detach'));
    expect(useUIStore.getState().openDialog).toHaveBeenCalledWith('unmerge-stations', {
      stationRef: SIDE0,
      mergedStations: [{ line_id: MAIN, station_index: 1 }],
      chosenLine: SIDE,
    });
    expect(mutations()).toEqual([]);
  });

  it('goes back to editing on Cancel', async () => {
    open(SIDE0);
    fireEvent.change(screen.getByTestId('station-name-input'), { target: { value: '12b' } });
    await save();
    fireEvent.click(await screen.findByTestId('shown-identity-prompt-cancel'));
    expect(screen.queryByTestId('shown-identity-prompt')).toBeNull();
    expect(screen.getByTestId('station-name-input')).toHaveValue('12b');
    expect(mutations()).toEqual([]);
  });

  it('says a loop end is shown as its kept end, with Remove the closure', async () => {
    ipc(LOOP_OWNER);
    open(SIDE2);
    expect(screen.getByTestId('edit-station-id')).toHaveTextContent('3');
    fireEvent.change(screen.getByTestId('station-comment-input'), { target: { value: 'Arch' } });
    await save();
    await waitFor(() =>
      expect(screen.getByTestId('shown-identity-prompt-lead')).toHaveTextContent(
        'Shown as Main / 3 through a loop closure.',
      ),
    );
    expect(screen.getByTestId('shown-identity-prompt-proceed')).toHaveTextContent('Change the comment of Main / 3');
    expect(screen.getByTestId('shown-identity-prompt-remove-closure')).toHaveTextContent('Remove the closure');
    expect(screen.queryByTestId('shown-identity-prompt-detach')).toBeNull();
  });

  it('saves a native slot at once', async () => {
    open(MAIN2);
    fireEvent.change(screen.getByTestId('station-name-input'), { target: { value: '13b' } });
    await save();
    expect(screen.queryByTestId('shown-identity-prompt')).toBeNull();
    await waitFor(() => expect(mutations()).toEqual([['set_station_name', { stationRef: MAIN2, name: '13b' }]]));
  });
});
