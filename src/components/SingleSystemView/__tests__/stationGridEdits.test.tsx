/**
 * A paste, fill or range clear over the stations grid is one commit (review
 * finding ui-2). Its edits on foreign slots and loop ends share ONE
 * whose-station prompt, which lists each such station and the lines the edit
 * changes it on; the native edits of the same commit wait for the answer.
 * Going on sends every edit in order; Cancel drops them all. A lone edit
 * keeps the single-station prompt.
 *
 * Injection (333e24ac5): the grid had no onBatchEdit, so each cell opened its
 * own prompt (only the last survived, one dialog at a time) while the native
 * cells were sent at once; the Playwright case in stations-grid.spec.ts
 * shows it end to end. Here, dropping the hold (sending the native edits
 * before the prompt) fails "holds the native edits".
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
import ShownIdentityEditDialog from '../../Dialogs/ShownIdentityEditDialog';
import { commitStationGridEdits, planStationGridEdits, type StationGridEdit } from '../stationGridEdits';
import type { Station } from '../../../types/project';

const MAIN = 1;
const SIDE = 2;
const slot = (line_id: number, station_index: number) => ({ line_id, station_index });
const station = (ref: ReturnType<typeof slot>, id: number, extra: Partial<Station> = {}): Station =>
  ({ station_ref: ref, id, name: String(ref.station_index), merged_with: [], ...extra }) as Station;

/** Side's rows: 0 is a foreign slot of Main's 3, 1 is native, 2 a loop end
 *  shown as Main / 1. */
const ROWS: Station[] = [
  station(slot(SIDE, 0), 13, { name: '3', shown: { id: 13, home_line: MAIN, foreign: true, loop_end: false } }),
  station(slot(SIDE, 1), 21),
  station(slot(SIDE, 2), 22, {
    name: '1',
    shown: { id: 11, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 5n },
  }),
];

const setStationName = vi.fn();
const updateStationComment = vi.fn();
const openDialog = vi.fn();
const closeDialog = vi.fn();
const actions = { setStationName, updateStationComment };

beforeEach(() => {
  vi.clearAllMocks();
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  setStationName.mockResolvedValue({ success: true });
  updateStationComment.mockResolvedValue({ success: true });
  invokeIpc.mockImplementation(async (_cmd: string, args: { stationRef: { station_index: number } }) =>
    args.stationRef.station_index === 0
      ? { station: slot(SIDE, 0), node: 13, shown: { id: 13, name: '3', home_line: MAIN }, lines: [MAIN, SIDE], foreign: true }
      : {
          station: slot(SIDE, 2),
          node: 22,
          shown: { id: 11, name: '1', home_line: MAIN },
          lines: [MAIN, SIDE],
          foreign: true,
          loop_closure: 5n,
        },
  );
  useProjectStore.setState({
    project: { lines: [{ id: MAIN, name: 'Main', shots: [] }, { id: SIDE, name: 'Side', shots: [] }] },
    surveyData: null,
    stations: ROWS,
    setStationName,
    updateStationComment,
  } as never);
});

const paste = (col: string, values: Array<string | null>) =>
  values.map((value, row) => ({ row, colKey: col, value }));

describe('planning a grid commit', () => {
  it('keeps the order, marks the slots that ask, and drops the no-ops', () => {
    const planned = planStationGridEdits(
      [...paste('comment', ['Arch', 'Wet', 'Low']), { row: 1, colKey: 'name', value: ' 1 ' }, { row: 9, colKey: 'name', value: 'X' }],
      ROWS,
    );
    expect(planned).toEqual([
      { stationRef: slot(SIDE, 0), kind: 'comment', value: 'Arch', ask: true },
      { stationRef: slot(SIDE, 1), kind: 'comment', value: 'Wet', ask: false },
      { stationRef: slot(SIDE, 2), kind: 'comment', value: 'Low', ask: true },
      // " 1 " is the row's own name, trimmed: no rename.
    ]);
  });
});

describe('committing a grid commit', () => {
  it('holds the native edits behind ONE prompt listing every asking station', () => {
    commitStationGridEdits(planStationGridEdits(paste('comment', ['Arch', 'Wet', 'Low']), ROWS), openDialog, actions);
    expect(updateStationComment).not.toHaveBeenCalled();
    expect(openDialog).toHaveBeenCalledTimes(1);
    const [dialog, data] = openDialog.mock.calls[0]!;
    expect(dialog).toBe('shown-identity-edit');
    expect((data as { edits: StationGridEdit[] }).edits.map((e) => e.stationRef.station_index)).toEqual([0, 1, 2]);
  });

  it('a lone edit keeps the single-station prompt', () => {
    commitStationGridEdits(planStationGridEdits([{ row: 0, colKey: 'name', value: 'J' }], ROWS), openDialog, actions);
    expect(openDialog).toHaveBeenCalledWith('shown-identity-edit', { stationRef: slot(SIDE, 0), kind: 'name', value: 'J' });
  });

  it('native edits only: sent at once, in order', async () => {
    commitStationGridEdits(planStationGridEdits([{ row: 1, colKey: 'comment', value: 'Wet' }, { row: 1, colKey: 'name', value: 'S1' }], ROWS), openDialog, actions);
    expect(openDialog).not.toHaveBeenCalled();
    await waitFor(() => expect(setStationName).toHaveBeenCalledWith(slot(SIDE, 1), 'S1'));
    expect(updateStationComment).toHaveBeenCalledWith(slot(SIDE, 1), 'Wet');
    expect(updateStationComment.mock.invocationCallOrder[0]).toBeLessThan(setStationName.mock.invocationCallOrder[0]!);
  });
});

describe('the batch prompt', () => {
  function open(edits: StationGridEdit[]) {
    useUIStore.setState({
      activeDialog: 'shown-identity-edit',
      appMode: 'survey',
      geometryMode: 'measured',
      closeDialog,
      dialogData: { edits },
    } as never);
    render(<ShownIdentityEditDialog />);
  }

  it('lists each station other lines show and the lines it changes on; going on sends everything', async () => {
    open(planStationGridEdits(paste('comment', ['Arch', 'Wet', 'Low']), ROWS));
    expect(screen.getByTestId('shown-identity-batch-lead')).toHaveTextContent(
      '2 stations this edit changes are shown on other lines too:',
    );
    await waitFor(() =>
      expect(screen.getByTestId('shown-identity-batch-row-0')).toHaveTextContent(
        'On Side, 3 belongs to Main. The edit changes it on Main and Side.',
      ),
    );
    expect(screen.getByTestId('shown-identity-batch-row-1')).toHaveTextContent(
      'On Side, 1 is shown as Main / 1 through a loop closure. The edit changes it on Main and Side.',
    );
    expect(screen.getByTestId('shown-identity-prompt-proceed')).toHaveTextContent('Apply all 3 edits');
    expect(updateStationComment).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(screen.getByTestId('shown-identity-prompt-proceed'));
    });
    await waitFor(() => expect(updateStationComment).toHaveBeenCalledTimes(3));
    expect(updateStationComment.mock.calls.map((c) => c[0].station_index)).toEqual([0, 1, 2]);
    expect(closeDialog).toHaveBeenCalled();
  });

  it('Cancel drops every edit of the commit', async () => {
    open(planStationGridEdits(paste('comment', ['Arch', 'Wet', 'Low']), ROWS));
    fireEvent.click(screen.getByTestId('shown-identity-prompt-cancel'));
    expect(closeDialog).toHaveBeenCalled();
    expect(updateStationComment).not.toHaveBeenCalled();
    expect(setStationName).not.toHaveBeenCalled();
  });
});
