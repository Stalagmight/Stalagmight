/**
 * Demo finding N-2: on a two-line junction where one line keeps the station
 * (`kept`: the other line was merged there) and the other gets back the
 * station it had, both choices give the same result. The dialog used to offer
 * two radio rows that read as different actions, preselected "Main Line (the
 * line you clicked)" and said "Main Line keeps this station", and the history
 * then read `Detach line "Main Line"` although North Passage came off. Now it
 * offers no choice: one sentence says which line comes off and what it gets
 * back, and Detach detaches THAT line (so the backend's history label names
 * it too). The list stays whenever the choice changes the result.
 *
 * Injections: rendering the radio list when `sameDetachResult` holds fails
 * "offers no choice"; detaching the clicked slot instead of `same.off` fails
 * "detaches the line that comes off"; dropping the previews' wait fails
 * "shows no choice while the previews are out".
 */
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import UnmergeStationsDialog, { sameDetachResult } from '../UnmergeStationsDialog';
import type { DetachLinePreview } from '../../../types/ipc.generated';

const MAIN = 0;
const NORTH = 1;
const slot = (line_id: number, station_index: number) => ({ line_id, station_index });

// Main Line keeps 3; North Passage gets back 0 (Station ID 9), whichever is chosen.
const KEEPS: DetachLinePreview = { name: '0', station_id: 9, restored: true, kept: true, lines: [], together: [], rest_choices: [], collisions: [] };
const RESTORES: DetachLinePreview = { name: '0', station_id: 9, restored: true, kept: false, lines: [], together: [], rest_choices: [], collisions: [] };

const unmergeStations = vi.fn();
const unmergeStationPair = vi.fn();
const closeDialog = vi.fn();

function project(comment: string) {
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main Line', shots: [] },
        { id: NORTH, name: 'North Passage', shots: [] },
      ],
    },
    surveyData: null,
    stations: [
      { station_ref: slot(MAIN, 3), id: 3, name: '3', comment, merged_with: [slot(NORTH, 0)] },
      { station_ref: slot(NORTH, 0), id: 3, name: '0', comment, merged_with: [slot(MAIN, 3)] },
    ],
    unmergeStations,
    unmergeStationPair,
  } as never);
}

function openOn(clicked: ReturnType<typeof slot>, other: ReturnType<typeof slot>) {
  useUIStore.setState({ closeDialog, dialogData: { stationRef: clicked, mergedStations: [other] } } as never);
  render(<UnmergeStationsDialog />);
}

// Demo finding V-9: "Main Line keeps 3." read oddly with a bare name; it
// reads "keeps station 3". Injection: drop "station" from `comesOff`.
const SENTENCE = 'North Passage comes off: it gets back 0 (Station ID 9); Main Line keeps station 3.';

beforeEach(() => {
  vi.clearAllMocks();
  unmergeStations.mockResolvedValue({ success: true });
  unmergeStationPair.mockResolvedValue({ success: true });
  invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) =>
    args.lineId === MAIN ? KEEPS : RESTORES,
  );
  project('');
});

describe('a detach whose result does not depend on the choice (N-2)', () => {
  it('offers no choice and says which line comes off, from the keeper', async () => {
    openOn(slot(MAIN, 3), slot(NORTH, 0));
    expect(await screen.findByTestId('unmerge-stations-naming')).toHaveTextContent(SENTENCE);
    expect(screen.queryByTestId(`unmerge-station-option-${MAIN}-3`)).toBeNull();
    expect(screen.queryByTestId(`unmerge-station-option-${NORTH}-0`)).toBeNull();
    expect(screen.queryByTestId('unmerge-stations-lead')).toBeNull();
    expect(screen.queryByText(/the line you clicked/)).toBeNull();
    expect(screen.getByTestId('unmerge-stations-explain')).toHaveTextContent(
      'North Passage keeps its shots, and a datum or loop closure set on it goes with it. Undo puts it back.',
    );
    // No comment comes back: nothing to say about one.
    expect(screen.queryByTestId('unmerge-stations-comes-back-comment')).toBeNull();
  });

  it('detaches the line that comes off, never the keeper the user clicked', async () => {
    openOn(slot(MAIN, 3), slot(NORTH, 0));
    await screen.findByTestId('unmerge-stations-naming');
    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    // The pair form detaches the partner's line: North Passage. The preview
    // named no collision, so the answers are an empty list (RS 4.2): a
    // collision it did not show refuses rather than letters.
    expect(unmergeStationPair).toHaveBeenCalledWith(slot(MAIN, 3), slot(NORTH, 0), { renames: [] });
    expect(unmergeStations).not.toHaveBeenCalled();
  });

  it('reads the same and detaches the same line when opened on the line that comes off', async () => {
    openOn(slot(NORTH, 0), slot(MAIN, 3));
    expect(await screen.findByTestId('unmerge-stations-naming')).toHaveTextContent(SENTENCE);
    expect(screen.queryByTestId(`unmerge-station-option-${NORTH}-0`)).toBeNull();
    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(unmergeStations).toHaveBeenCalledWith(slot(NORTH, 0), { renames: [] });
    expect(unmergeStationPair).not.toHaveBeenCalled();
  });

  // RS 4.2: the station that comes back has its own comment (a merge never
  // joins comments), so the dialog says it, and says nothing of the
  // junction's own comment, which stays where it is.
  it("says the comment that comes back with the line's own station", async () => {
    invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) =>
      args.lineId === MAIN ? KEEPS : { ...RESTORES, comment: 'Silt bank, low visibility' },
    );
    project('T-junction, line arrow points out');
    openOn(slot(MAIN, 3), slot(NORTH, 0));
    expect(await screen.findByTestId('unmerge-stations-comes-back-comment')).toHaveTextContent(
      'With its comment: "Silt bank, low visibility"',
    );
    expect(screen.queryByText(/T-junction/)).toBeNull();
  });

  it('shows no choice while the previews are out, and Detach waits for them', async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) => {
      await held;
      return args.lineId === MAIN ? KEEPS : RESTORES;
    });
    openOn(slot(MAIN, 3), slot(NORTH, 0));
    expect(screen.getByTestId('unmerge-stations-lead')).toHaveTextContent('3 is where 2 lines meet.');
    expect(screen.queryByTestId(`unmerge-station-option-${MAIN}-3`)).toBeNull();
    expect(screen.getByTestId('unmerge-stations-confirm')).toBeDisabled();
    await act(async () => {
      release();
      await held;
    });
    expect(await screen.findByTestId('unmerge-stations-naming')).toHaveTextContent(SENTENCE);
    expect(screen.getByTestId('unmerge-stations-confirm')).toBeEnabled();
  });

  it('keeps the choice when the line would get back another station than the keeper names', async () => {
    invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) =>
      args.lineId === MAIN ? KEEPS : { ...RESTORES, station_id: 12, name: '0b' },
    );
    openOn(slot(MAIN, 3), slot(NORTH, 0));
    await screen.findByTestId('unmerge-stations-naming');
    // The line that comes off alone is the likely intent (V-1).
    expect(screen.getByTestId(`unmerge-station-option-${NORTH}-0`)).toBeChecked();
    expect(screen.getByTestId(`unmerge-station-option-${MAIN}-3`)).toBeInTheDocument();
  });

  it('keeps the choice when both previews fail', async () => {
    invokeIpc.mockRejectedValue(new Error('No project open'));
    openOn(slot(MAIN, 3), slot(NORTH, 0));
    expect(await screen.findByTestId(`unmerge-station-option-${MAIN}-3`)).toBeChecked();
    expect(screen.getByTestId('unmerge-stations-confirm')).toBeEnabled();
  });
});

describe('sameDetachResult', () => {
  const a = slot(MAIN, 3);
  const b = slot(NORTH, 0);
  const both = new Map([
    [MAIN, KEEPS],
    [NORTH, RESTORES],
  ]);
  it('finds the line that comes off from either side', () => {
    expect(sameDetachResult([a, b], both)).toEqual({ off: b, keeper: a, preview: RESTORES });
    expect(sameDetachResult([b, a], both)).toEqual({ off: b, keeper: a, preview: RESTORES });
  });
  it('is null when the choice matters or is not known yet', () => {
    expect(sameDetachResult([a, b], new Map())).toBeNull();
    expect(sameDetachResult([a, b], new Map([[MAIN, KEEPS]]))).toBeNull();
    // A new station for the other line: the keeper does not keep it then.
    const mints: DetachLinePreview = { name: '3', station_id: 30, restored: false, kept: false, lines: [], together: [], rest_choices: [], collisions: [] };
    expect(sameDetachResult([a, b], new Map([[MAIN, mints], [NORTH, RESTORES]]))).toBeNull();
    // A hub: the other lines would come off together, so the choice matters.
    const c = slot(2, 0);
    expect(sameDetachResult([a, b, c], new Map([...both, [2, RESTORES]]))).toBeNull();
    // Two slots of one line are not two lines.
    expect(sameDetachResult([a, slot(MAIN, 5)], both)).toBeNull();
  });
});
