/**
 * RS 4.2, 7: Detach Line from Station says what comes back (name, comment,
 * Station ID), the lines that come off with the detached one because they
 * were one station, what a loop closure made the line show before, offers the
 * remaining junction's station when several archived stations could show it
 * (asking the preview again with the pick), lists each collision with its two
 * answers, and notes that earlier letters stay. Detach sends the answers and
 * the pick. A line the user already chose (the "Also on" button) is
 * preselected and stays so.
 *
 * Injections: dropping `restKeep` from the command fails "sends the remaining
 * junction's station"; ignoring `chosenLine` fails "preselects the line the
 * user chose".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { DetachLinePreview } from '../../../types/ipc/DetachLinePreview';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import UnmergeStationsDialog from '../UnmergeStationsDialog';

const MAIN = 1;
const SIDE = 2;
const NORTH = 3;
const slot = (line_id: number, station_index: number) => ({ line_id, station_index });
const unmergeStations = vi.fn();
const unmergeStationPair = vi.fn();
const closeDialog = vi.fn();

const base = (extra: Partial<DetachLinePreview>): DetachLinePreview => ({
  name: '0',
  station_id: 40,
  restored: true,
  kept: false,
  lines: [],
  together: [],
  rest_choices: [],
  collisions: [],
  ...extra,
});

/** Side gets back its own 0 (Station ID 40), with its comment, North comes
 *  off with it, and 0 already names another station on Side. */
const SIDE_BACK = base({
  comment: 'Silt bank',
  together: [NORTH],
  collisions: [
    {
      line_id: SIDE,
      name: '0',
      letter: { station: { id: 40, name: '0', home_line: SIDE }, to: '0a' },
      alternative: { station: { id: 44, name: '0', home_line: SIDE }, to: '0a' },
    },
  ],
});
/** Main keeps the junction; the others come off on one of two archived
 *  stations, rule K's (40) by default. */
const MAIN_KEEPS = base({
  kept: true,
  rest_choices: [
    { id: 40, name: '0', home_line: SIDE },
    { id: 50, name: '9', home_line: NORTH },
  ],
  rest_default: 40,
});

beforeEach(() => {
  // A local project: editing is not blocked (useEditingBlocked).
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  vi.clearAllMocks();
  unmergeStations.mockResolvedValue({ success: true });
  unmergeStationPair.mockResolvedValue({ success: true });
  invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number; restKeep?: number }) => {
    if (args.lineId === MAIN) return args.restKeep === 50 ? { ...MAIN_KEEPS, name: '9', station_id: 50 } : MAIN_KEEPS;
    if (args.lineId === SIDE) return SIDE_BACK;
    return base({ name: '9', station_id: 50 });
  });
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
        { id: NORTH, name: 'North', shots: [] },
      ],
    },
    surveyData: null,
    stations: [
      { station_ref: slot(MAIN, 3), id: 3, name: '3', merged_with: [slot(SIDE, 0), slot(NORTH, 0)] },
      { station_ref: slot(SIDE, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(NORTH, 0)] },
      { station_ref: slot(NORTH, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(SIDE, 0)] },
    ],
    unmergeStations,
    unmergeStationPair,
  } as never);
});

function open(extra: Record<string, unknown> = {}) {
  useUIStore.setState({
    closeDialog,
    dialogData: { stationRef: slot(MAIN, 3), mergedStations: [slot(SIDE, 0), slot(NORTH, 0)], ...extra },
  } as never);
  render(<UnmergeStationsDialog />);
}

describe('Detach Line from Station under RS', () => {
  it('preselects the line the user chose and says what comes back with it', async () => {
    open({ chosenLine: SIDE });
    expect(await screen.findByTestId('unmerge-stations-comes-back')).toHaveTextContent('It gets back 0 (Station ID 40).');
    expect(screen.getByTestId(`unmerge-station-option-${SIDE}-0`)).toBeChecked();
    expect(screen.getByTestId('unmerge-stations-comes-back-comment')).toHaveTextContent('With its comment: "Silt bank"');
    expect(screen.getByTestId('unmerge-stations-together')).toHaveTextContent(
      'North comes off with Side: they were one station before the merge.',
    );
    expect(screen.getByTestId('unmerge-stations-letters-stay')).toBeInTheDocument();
  });

  it('lists the collision and detaches with the chosen answer', async () => {
    open({ chosenLine: SIDE });
    const row = await screen.findByTestId('unmerge-stations-collision-0');
    expect(row).toHaveTextContent('On Side, 0 would name two stations');
    fireEvent.click(screen.getByTestId('unmerge-stations-collision-0-alternative'));
    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(unmergeStationPair).toHaveBeenCalledWith(slot(MAIN, 3), slot(SIDE, 0), {
      renames: [{ station_id: 44, name: '0a' }],
    });
  });

  it("offers the remaining junction's station and sends the pick", async () => {
    open({ chosenLine: MAIN });
    const picker = await screen.findByTestId('unmerge-stations-rest');
    expect(picker).toHaveTextContent('The lines that come off show:');
    expect(screen.getByTestId('unmerge-stations-rest-40')).toBeChecked();
    fireEvent.click(screen.getByTestId('unmerge-stations-rest-50'));
    await waitFor(() =>
      expect(invokeIpc).toHaveBeenCalledWith(
        'preview_detach_line',
        { stationRef: slot(MAIN, 3), lineId: MAIN, restKeep: 50 },
        expect.anything(),
      ),
    );
    await waitFor(() => expect(screen.getByTestId('unmerge-stations-rest-50')).toBeChecked());
    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(unmergeStations).toHaveBeenCalledWith(slot(MAIN, 3), { renames: [], restKeep: 50 });
  });

  it('says what a loop closure made the line show before', async () => {
    invokeIpc.mockImplementation(async () =>
      base({ name: '3', station_id: 60, restored: false, shown_before: { id: 8, name: '8', home_line: NORTH } }),
    );
    open({ chosenLine: SIDE });
    expect(await screen.findByTestId('unmerge-stations-shown-before')).toHaveTextContent(
      'Side showed North / 8 here through a loop closure; it will show 3.',
    );
    expect(screen.queryByTestId('unmerge-stations-comes-back')).toBeNull();
    expect(screen.queryByTestId('unmerge-stations-letters-stay')).toBeNull();
  });
});
