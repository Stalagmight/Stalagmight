/**
 * RS 5.4: the data table's Name or Comment cell edited on a foreign slot or a
 * loop end opens "whose station is this?" before the edit is sent. Going on
 * sends the same command the cell would have; Cancel drops the edit. The
 * table's Station ID column reads the shown Station ID.
 * Injection: sending the edit on open fails "sends nothing until going on".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import ShownIdentityEditDialog from '../ShownIdentityEditDialog';
import { buildStationColumns } from '../../SingleSystemView/dataTableStationColumns';
import type { Station } from '../../../types/project';

const MAIN = 1;
const SIDE = 2;
const SIDE0 = { line_id: SIDE, station_index: 0 };
const setStationName = vi.fn();
const updateStationComment = vi.fn();
const closeDialog = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  setStationName.mockResolvedValue({ success: true });
  updateStationComment.mockResolvedValue({ success: true });
  invokeIpc.mockResolvedValue({
    station: SIDE0,
    node: 11,
    shown: { id: 11, name: '12', home_line: MAIN },
    lines: [MAIN, SIDE],
    foreign: true,
    own_station: { id: 40, name: '0', home_line: SIDE },
  });
  useProjectStore.setState({
    project: { lines: [{ id: MAIN, name: 'Main', shots: [] }, { id: SIDE, name: 'Side', shots: [] }] },
    surveyData: null,
    stations: [
      {
        station_ref: SIDE0, id: 11, name: '12', merged_with: [],
        shown: { id: 11, home_line: MAIN, foreign: true, loop_end: false },
      },
    ],
    setStationName,
    updateStationComment,
  } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

function open(kind: 'name' | 'comment', value: string | null) {
  useUIStore.setState({
    activeDialog: 'shown-identity-edit',
    appMode: 'survey',
    geometryMode: 'measured',
    closeDialog,
    dialogData: { stationRef: SIDE0, kind, value },
  } as never);
  render(<ShownIdentityEditDialog />);
}

describe("the data table's whose-station prompt (RS 5.4)", () => {
  it('sends nothing until going on, then renames the shown station', async () => {
    open('name', '12b');
    expect(await screen.findByTestId('shown-identity-prompt-lead')).toHaveTextContent('12 belongs to Main.');
    expect(setStationName).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(screen.getByTestId('shown-identity-prompt-proceed'));
    });
    await waitFor(() => expect(setStationName).toHaveBeenCalledWith(SIDE0, '12b'));
    expect(closeDialog).toHaveBeenCalled();
  });

  it('sends the comment edit, or nothing on Cancel', async () => {
    open('comment', 'Arch');
    await screen.findByTestId('shown-identity-prompt-lead');
    expect(screen.getByTestId('shown-identity-prompt-proceed')).toHaveTextContent('Change its comment on every line');
    fireEvent.click(screen.getByTestId('shown-identity-prompt-cancel'));
    expect(updateStationComment).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
  });
});

describe('the Station ID column (RS 6)', () => {
  it('reads the Station ID the slot shows', () => {
    const columns = buildStationColumns({
      t: ((key: string) => key) as never,
      precision: { coordinates: 2, depth: 1, gps: 6 },
      geometryReadOnly: false,
      gpsOrigin: null,
      onGpsCellClick: () => undefined,
      onConnectionsCellClick: () => undefined,
    });
    const idColumn = columns.find((c) => c.key === '_stationId')!;
    const loopEnd = { id: 22, shown: { id: 3, foreign: true, loop_end: true } } as unknown as Station;
    const native = { id: 13 } as unknown as Station;
    expect(idColumn.getValue(loopEnd)).toBe(3);
    expect(idColumn.getValue(native)).toBe(13);
  });
});
