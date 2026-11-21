/**
 * The Detach dialog says which name the detached line gets (review finding
 * UX-4): a line that was merged there gets back the Station ID and name it
 * had; the line the junction is its own keeps it when every other line knew
 * it by another name, and those lines get theirs back (I-1); otherwise the
 * line gets a new station named as it showed the junction. The words come
 * from the backend's `preview_detach_line` for the chosen line. On two lines
 * where either choice gives the same result there is no choice to word: one
 * sentence names the line that comes off (N-2).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import UnmergeStationsDialog from '../UnmergeStationsDialog';

const MAIN = 3;
const SIDE = 8;
const BRANCH = 9;
const slot = (line_id: number, station_index: number) => ({ line_id, station_index });

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) =>
    args.lineId === SIDE
      ? { name: '0', station_id: 7, restored: true, kept: false }
      : { name: '0', station_id: 7, restored: true, kept: true },
  );
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
      ],
    },
    surveyData: null,
    stations: [
      { station_ref: slot(MAIN, 3), id: 3, name: '3a', merged_with: [slot(SIDE, 0)] },
      { station_ref: slot(SIDE, 0), id: 3, name: '0', merged_with: [slot(MAIN, 3)] },
    ],
    unmergeStations: vi.fn(),
    unmergeStationPair: vi.fn(),
  } as never);
  useUIStore.setState({
    closeDialog: vi.fn(),
    dialogData: { stationRef: slot(SIDE, 0), mergedStations: [slot(MAIN, 3)] },
  } as never);
});

/** Main 3 is a hub: Side and Branch were merged there (Branch's old station
 *  is 0b, Station ID 12), so detaching Main keeps it on Main and Side and
 *  Branch come off together; the choice changes the result. */
function hub() {
  const { project, stations } = useProjectStore.getState();
  useProjectStore.setState({
    project: { ...project!, lines: [...project!.lines, { id: BRANCH, name: 'Branch', shots: [] }] },
    stations: [
      ...stations.map((s) => ({ ...s, merged_with: [...s.merged_with, slot(BRANCH, 0)] })),
      { station_ref: slot(BRANCH, 0), id: 3, name: '0b', merged_with: [slot(MAIN, 3), slot(SIDE, 0)] },
    ],
  } as never);
  useUIStore.setState({
    dialogData: { stationRef: slot(SIDE, 0), mergedStations: [slot(MAIN, 3), slot(BRANCH, 0)] },
  } as never);
  invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) =>
    args.lineId === SIDE
      ? { name: '0', station_id: 7, restored: true, kept: false }
      : args.lineId === BRANCH
        ? { name: '0b', station_id: 12, restored: true, kept: false }
        : { name: '0', station_id: 7, restored: true, kept: true },
  );
}

describe('Detach dialog naming', () => {
  it('names the line that comes off when either choice gives the same result', async () => {
    render(<UnmergeStationsDialog />);
    const naming = await screen.findByTestId('unmerge-stations-naming');
    expect(naming).toHaveTextContent('Side comes off: it gets back 0 (Station ID 7); Main keeps 3a.');
    expect(screen.getByTestId('unmerge-stations-explain')).toHaveTextContent(
      'Side keeps its shots, and a datum or loop closure set on it goes with it.',
    );
    expect(invokeIpc).toHaveBeenCalledWith(
      'preview_detach_line',
      { stationRef: slot(SIDE, 0), lineId: SIDE },
      expect.anything(),
    );
  });

  it('says the merged line gets its own station back', async () => {
    hub();
    render(<UnmergeStationsDialog />);
    const naming = await screen.findByTestId('unmerge-stations-naming');
    expect(naming).toHaveTextContent('Side gets back the station it had here before the merge: 0 (Station ID 7).');
    expect(screen.getByTestId('unmerge-stations-explain')).toHaveTextContent(
      'The line you detach keeps its shots and gets its own station here, with no comment.',
    );
  });

  it('follows the chosen line: the junction stays on its own line and the merged lines get their station back', async () => {
    hub();
    render(<UnmergeStationsDialog />);
    await screen.findByTestId('unmerge-stations-naming');
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${MAIN}-3`));
    await waitFor(() =>
      expect(screen.getByTestId('unmerge-stations-naming')).toHaveTextContent(
        'Main keeps this station. Side and Branch get back the station they had here before the merge: 0 (Station ID 7).',
      ),
    );
    // What stays and what goes is said the same way round.
    expect(screen.getByTestId('unmerge-stations-explain')).toHaveTextContent(
      'The line you detach keeps its shots and this station, with its comment.',
    );
  });

  it('says a line gets a new station named as it showed the junction when another line keeps it', async () => {
    invokeIpc.mockImplementation(async () => ({ name: '3a', station_id: 19, restored: false, kept: false }));
    render(<UnmergeStationsDialog />);
    await screen.findByTestId('unmerge-stations-naming');
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${MAIN}-3`));
    await waitFor(() =>
      expect(screen.getByTestId('unmerge-stations-naming')).toHaveTextContent('Main gets a new station here named 3a.'),
    );
  });

  it('says nothing when the preview fails', async () => {
    invokeIpc.mockRejectedValue(new Error('No project open'));
    render(<UnmergeStationsDialog />);
    await waitFor(() => expect(invokeIpc).toHaveBeenCalled());
    expect(screen.queryByTestId('unmerge-stations-naming')).toBeNull();
  });
});
