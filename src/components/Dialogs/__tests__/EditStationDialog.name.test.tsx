/**
 * Edit Station's Name field (SPEC 4.1): a station name is required and unique
 * within its line. The dialog checks both inline (Save disabled, the message
 * under the field), sends a changed name through set_station_name trimmed,
 * shows a refusal the backend still makes at the field, and shows the
 * read-only Station ID - never the positional `line:index` handle. Through the
 * real store path, with only the IPC mocked.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: String }));
vi.mock('../../../hooks/useZeroDatumPreview', () => ({ useZeroDatumPreview: () => null }));

import '../../../i18n';
import EditStationDialog from '../EditStationDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { Project, Station } from '../../../types/project';

const MUTATIONS = new Set(['set_station_name', 'set_station_comment', 'set_zero_datum', 'remove_zero_datum']);
const mutations = () =>
  invokeIpcMock.mock.calls.filter(([command]) => MUTATIONS.has(command as string)).map(([command, args]) => [command, args]);

function station(index: number, name: string, id: number): Station {
  return {
    station_ref: { line_id: 3, station_index: index },
    coordinates: { x: index * 10, y: 0, z: -index },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: [],
    id,
    name,
  };
}

function ok() {
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (MUTATIONS.has(command)) return { success: true, warnings: [], error: null, line_ids: [3] };
    if (command === 'can_undo' || command === 'can_redo') return false;
    if (command === 'get_project_state') return null;
    if (command.includes('integrity')) return { messages: [] };
    if (command.startsWith('get_') || command.startsWith('validate')) return [];
    return null;
  });
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  ok();
  useProjectStore.setState({
    project: {
      created_at: 'rename-station',
      lines: [{ id: 3, name: 'Main Line', color: '#336699', shots: [{ id: 1 }, { id: 2 }] }],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
    } as unknown as Project,
    // No SurveyData: names and ids come from the station list.
    surveyData: null,
    stations: [station(0, '4', 40), station(1, '5', 41), station(2, '6', 42)],
    config: null,
  });
  useUIStore.setState({
    activeDialog: 'edit-station',
    dialogData: { stationRef: { line_id: 3, station_index: 1 } },
    appMode: 'survey',
    geometryMode: 'measured',
    closeDialog: vi.fn(),
  });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

const nameInput = () => screen.getByTestId('station-name-input');
const saveButton = () => screen.getByTestId('edit-station-save');

async function save() {
  await act(async () => {
    fireEvent.click(saveButton());
  });
}

describe('Edit Station name', () => {
  it('shows the name and the read-only Station ID, never line:index', () => {
    render(<EditStationDialog />);
    expect(nameInput()).toHaveValue('5');
    expect(screen.getByTestId('edit-station-id')).toHaveTextContent('41');
    expect(screen.getByTestId('edit-station-dialog')).not.toHaveTextContent('3:1');
    expect(saveButton()).toBeEnabled();
  });

  it('says a name is needed and disables Save while it is empty', async () => {
    render(<EditStationDialog />);
    fireEvent.change(nameInput(), { target: { value: '   ' } });
    expect(screen.getByTestId('station-name-error')).toHaveTextContent('A station needs a name.');
    expect(nameInput()).toHaveAttribute('aria-invalid', 'true');
    expect(saveButton()).toBeDisabled();
    await save();
    expect(mutations()).toEqual([]);
  });

  it("refuses a name another station of the line holds; the station's own is fine", async () => {
    render(<EditStationDialog />);
    fireEvent.change(nameInput(), { target: { value: ' 6 ' } });
    expect(screen.getByTestId('station-name-error')).toHaveTextContent(
      'Main Line already has a station named "6". Station names must be unique within their line.',
    );
    expect(saveButton()).toBeDisabled();
    await save();
    expect(mutations()).toEqual([]);

    fireEvent.change(nameInput(), { target: { value: '5' } });
    expect(screen.queryByTestId('station-name-error')).toBeNull();
    expect(saveButton()).toBeEnabled();
  });

  it('renames through set_station_name, trimmed, and closes', async () => {
    render(<EditStationDialog />);
    fireEvent.change(nameInput(), { target: { value: ' 5a ' } });
    await save();
    expect(mutations()).toEqual([
      ['set_station_name', { stationRef: { line_id: 3, station_index: 1 }, name: '5a' }],
    ]);
    expect(useUIStore.getState().closeDialog).toHaveBeenCalled();
  });

  it('sends nothing for an unchanged name', async () => {
    render(<EditStationDialog />);
    fireEvent.change(screen.getByTestId('station-comment-input'), { target: { value: 'Tie-off' } });
    await save();
    expect(mutations().map(([command]) => command)).toEqual(['set_station_comment']);
  });

  it('refuses control and invisible formatting characters inline', async () => {
    render(<EditStationDialog />);
    fireEvent.change(nameInput(), { target: { value: '5\u202Eb' } });
    expect(screen.getByTestId('station-name-error')).toHaveTextContent(
      'A name cannot contain control characters or invisible formatting characters.',
    );
    expect(saveButton()).toBeDisabled();
    await save();
    expect(mutations()).toEqual([]);
  });

  it('checks every field before sending anything: a datum without GPS keeps the rename back too', async () => {
    render(<EditStationDialog />);
    fireEvent.change(nameInput(), { target: { value: '5a' } });
    fireEvent.change(screen.getByTestId('station-comment-input'), { target: { value: 'Tie-off' } });
    fireEvent.click(screen.getByTestId('station-zero-datum-input'));
    await save();
    // Nothing reached the backend, the dialog stays open on the fix to make.
    expect(mutations()).toEqual([]);
    expect(screen.getByTestId('edit-station-dialog')).toHaveTextContent('Please enter valid GPS coordinates');
    expect(useUIStore.getState().closeDialog).not.toHaveBeenCalled();

    // With the GPS given, the save goes through: the rename first.
    fireEvent.change(screen.getByTestId('station-gps-input'), { target: { value: '20.5, -87.3' } });
    await save();
    expect(mutations().map(([command]) => command)).toEqual(['set_station_name', 'set_zero_datum', 'set_station_comment']);
  });

  it('sends no rename for the same name with spaces around it', async () => {
    render(<EditStationDialog />);
    fireEvent.change(nameInput(), { target: { value: ' 5 ' } });
    fireEvent.change(screen.getByTestId('station-comment-input'), { target: { value: 'Tie-off' } });
    await save();
    expect(mutations().map(([command]) => command)).toEqual(['set_station_comment']);
  });

  it('shows a refusal the backend still makes at the name field, and keeps the dialog open', async () => {
    const refusal = 'Line "Main Line" already has a station named "9"';
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'set_station_name') return { success: false, warnings: [], error: refusal, line_ids: [] };
      if (command === 'can_undo' || command === 'can_redo') return false;
      if (command === 'get_project_state') return null;
      if (command.includes('integrity')) return { messages: [] };
      if (command.startsWith('get_') || command.startsWith('validate')) return [];
      return null;
    });
    render(<EditStationDialog />);
    fireEvent.change(nameInput(), { target: { value: '9' } });
    await save();
    expect(screen.getByTestId('station-name-error')).toHaveTextContent(refusal);
    expect(nameInput()).toHaveValue('9');
    expect(useUIStore.getState().closeDialog).not.toHaveBeenCalled();

    // Typing again clears the refusal.
    fireEvent.change(nameInput(), { target: { value: '9b' } });
    expect(screen.queryByTestId('station-name-error')).toBeNull();
  });
});
