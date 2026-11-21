/**
 * The Detach dialog says which name the detached line gets (review finding
 * UX-4): a line that was merged there gets back the Station ID and name it
 * had; the line the junction is its own keeps it when every other line knew
 * it by another name, and those lines get theirs back (I-1); otherwise the
 * line gets a new station named as it showed the junction. The words come
 * from the backend's `preview_detach_line` for the chosen line.
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

describe('Detach dialog naming', () => {
  it('says the merged line gets its own station back', async () => {
    render(<UnmergeStationsDialog />);
    const naming = await screen.findByTestId('unmerge-stations-naming');
    expect(naming).toHaveTextContent('Side gets back the station it had here before the merge: 0 (Station ID 7).');
    expect(invokeIpc).toHaveBeenCalledWith(
      'preview_detach_line',
      { stationRef: slot(SIDE, 0), lineId: SIDE },
      expect.anything(),
    );
  });

  it('follows the chosen line: the junction stays on its own line and the merged line gets its station back', async () => {
    render(<UnmergeStationsDialog />);
    await screen.findByTestId('unmerge-stations-naming');
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${MAIN}-3`));
    await waitFor(() =>
      expect(screen.getByTestId('unmerge-stations-naming')).toHaveTextContent(
        'Main keeps this station. Side gets back the station it had here before the merge: 0 (Station ID 7).',
      ),
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
