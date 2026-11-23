/**
 * Demo finding I-7, the Detach dialog's default and the joined comment:
 * - clicking the junction on its OWNER line (the line no alias names, which a
 *   detach would give a NEW Station ID) used to preselect that line, so the
 *   main line, not the side line merged there, lost Station ID 3 and its
 *   comment. The line that gets its own station back is now preselected.
 *   Since I-1 the owner gets a new Station ID only while a line that shows
 *   the junction by its own name stays on it (here Main Line B, started from
 *   Main 3); with only merged lines left the owner keeps the junction
 *   (`kept`) while the others come off together. Demo finding V-1: that
 *   owner is no longer preselected either; a line that comes off alone with
 *   its own station back is the likely intent (on a two-line junction there
 *   is no choice at all, N-2: `UnmergeStationsDialog.same.test.tsx`);
 * - under RS a merge never joins comments: the archived station keeps its
 *   own (RS 4.1), so a line that gets its station back gets its comment
 *   back too, and the dialog says what comes back; the junction's comment
 *   stays with it and is not mentioned.
 * Injection: preselecting `slots[0]` always (dropping `defaultDetachSlot`)
 * fails the first case; keeping a clicked owner that keeps the junction (the
 * I-7 rule) fails the V-1 case; dropping the comes-back comment fails the
 * comment case.
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
import UnmergeStationsDialog, { defaultDetachSlot } from '../UnmergeStationsDialog';
import type { DetachLinePreview } from '../../../types/ipc.generated';

const MAIN = 0;
const NORTH = 1;
const SPLIT_B = 2;
const slot = (line_id: number, station_index: number) => ({ line_id, station_index });

const RESTORES: DetachLinePreview = { name: '0', station_id: 9, restored: true, kept: false, lines: [], together: [], rest_choices: [], collisions: [], shown_elsewhere: [], comments_hidden: [] };
const MINTS: DetachLinePreview = { name: '3', station_id: 30, restored: false, kept: false, lines: [], together: [], rest_choices: [], collisions: [], shown_elsewhere: [], comments_hidden: [] };
// The owner keeps the junction; North Passage gets back 0 (Station ID 9).
const KEEPS: DetachLinePreview = { name: '0', station_id: 9, restored: true, kept: true, lines: [], together: [], rest_choices: [], collisions: [], shown_elsewhere: [], comments_hidden: [] };

function project(comment: string) {
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main Line', shots: [] },
        { id: NORTH, name: 'North Passage', shots: [] },
        { id: SPLIT_B, name: 'Main Line B', shots: [] },
      ],
    },
    surveyData: null,
    // Main 3 is North Passage's start: one station (ID 3), North merged onto
    // it; Main Line B starts there too (no alias: it shows the junction's name).
    stations: [
      { station_ref: slot(MAIN, 3), id: 3, name: '3', comment, merged_with: [slot(NORTH, 0), slot(SPLIT_B, 0)] },
      { station_ref: slot(NORTH, 0), id: 3, name: '0', comment, merged_with: [slot(MAIN, 3), slot(SPLIT_B, 0)] },
      { station_ref: slot(SPLIT_B, 0), id: 3, name: '3', comment, merged_with: [slot(MAIN, 3), slot(NORTH, 0)] },
    ],
    unmergeStations: vi.fn(),
    unmergeStationPair: vi.fn(),
  } as never);
}

function openOn(clicked: ReturnType<typeof slot>, ...others: ReturnType<typeof slot>[]) {
  useUIStore.setState({
    closeDialog: vi.fn(),
    dialogData: { stationRef: clicked, mergedStations: others },
  } as never);
  render(<UnmergeStationsDialog />);
}

/** Main 3 shared by Main Line, North Passage (merged) and Main Line B. */
const HUB_OTHERS = [slot(NORTH, 0), slot(SPLIT_B, 0)];

/** The same hub when Main Line B was merged there too: Main Line keeps it. */
const KEEPS_HUB = new Map<number, DetachLinePreview>([
  [MAIN, KEEPS],
  [NORTH, RESTORES],
  [SPLIT_B, { name: '0b', station_id: 12, restored: true, kept: false, lines: [], together: [], rest_choices: [], collisions: [], shown_elsewhere: [], comments_hidden: [] }],
]);

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) =>
    args.lineId === NORTH ? RESTORES : MINTS,
  );
  project('T-junction, line arrow points out\n\nSilt bank, low visibility');
});

describe('Detach dialog default (I-7)', () => {
  it("preselects the merged line when the owner's slot was clicked", async () => {
    openOn(slot(MAIN, 3), ...HUB_OTHERS);
    await waitFor(() => expect(screen.getByTestId(`unmerge-station-option-${NORTH}-0`)).toBeChecked());
    expect(screen.getByTestId(`unmerge-station-option-${MAIN}-3`)).not.toBeChecked();
    // The clicked line is still named as the one clicked, and listed first.
    expect(screen.getByText('Main Line (the line you clicked)')).toBeInTheDocument();
    expect(screen.getByTestId('unmerge-stations-naming')).toHaveTextContent(
      'North Passage comes off and gets back the station it had here before the merge.',
    );
  });

  it('preselects a line that comes off alone, not a clicked owner that keeps the junction (V-1)', async () => {
    // A hub of merged lines: detaching Main Line keeps it there, and the
    // other two come off together; the likely intent is one of those lines.
    invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) => KEEPS_HUB.get(args.lineId));
    openOn(slot(MAIN, 3), ...HUB_OTHERS);
    await waitFor(() => expect(screen.getByTestId(`unmerge-station-option-${NORTH}-0`)).toBeChecked());
    expect(screen.getByTestId(`unmerge-station-option-${MAIN}-3`)).not.toBeChecked();
    // The owner's row says what it does: the others come off together.
    expect(
      screen.getByText('North Passage and Main Line B, together (Main Line, the line you clicked, keeps this station)'),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${MAIN}-3`));
    expect(screen.getByTestId('unmerge-stations-naming')).toHaveTextContent(
      'Main Line keeps this station, and North Passage and Main Line B come off together: they stay joined.',
    );
  });

  it('keeps the clicked slot when it is the merged line', async () => {
    openOn(slot(NORTH, 0), slot(MAIN, 3));
    await screen.findByTestId('unmerge-stations-naming');
    expect(screen.getByTestId(`unmerge-station-option-${NORTH}-0`)).toBeChecked();
  });

  it('keeps the clicked slot when no line was merged there (a split junction)', async () => {
    invokeIpc.mockResolvedValue(MINTS);
    openOn(slot(MAIN, 3), slot(SPLIT_B, 0));
    await screen.findByTestId('unmerge-stations-naming');
    expect(screen.getByTestId(`unmerge-station-option-${MAIN}-3`)).toBeChecked();
  });

  it('never overrides a line the user already picked', async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) => {
      await held;
      return args.lineId === NORTH ? RESTORES : MINTS;
    });
    openOn(slot(MAIN, 3), ...HUB_OTHERS);
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${NORTH}-0`));
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${MAIN}-3`));
    await act(async () => {
      release();
      await held;
    });
    await screen.findByTestId('unmerge-stations-naming');
    expect(screen.getByTestId(`unmerge-station-option-${MAIN}-3`)).toBeChecked();
  });
});

describe('Detach dialog and the comment that comes back (RS 4.2)', () => {
  it("says the comment of the station that comes back, and only for the line that gets it", async () => {
    invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) =>
      args.lineId === NORTH ? { ...RESTORES, comment: 'Silt bank' } : MINTS,
    );
    openOn(slot(MAIN, 3), ...HUB_OTHERS);
    expect(await screen.findByTestId('unmerge-stations-comes-back')).toHaveTextContent('It gets back 0 (Station ID 9).');
    expect(screen.getByTestId('unmerge-stations-comes-back-comment')).toHaveTextContent('With its comment: "Silt bank"');
    // The owner gets a new station: nothing comes back.
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${MAIN}-3`));
    await waitFor(() => expect(screen.queryByTestId('unmerge-stations-comes-back')).toBeNull());
  });

  it('says the clicked owner keeps the junction and its comment', async () => {
    invokeIpc.mockImplementation(async (_cmd: string, args: { lineId: number }) => KEEPS_HUB.get(args.lineId));
    openOn(slot(MAIN, 3), ...HUB_OTHERS);
    await screen.findByTestId('unmerge-stations-naming');
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${MAIN}-3`));
    expect(screen.getByTestId('unmerge-stations-explain')).toHaveTextContent(
      'Main Line keeps this station and its comment.',
    );
    expect(screen.queryByTestId('unmerge-stations-comes-back-comment')).toBeNull();
  });

  it('says nothing about a comment the station that comes back does not have', async () => {
    project('');
    openOn(slot(MAIN, 3), ...HUB_OTHERS);
    await screen.findByTestId('unmerge-stations-naming');
    expect(screen.queryByTestId('unmerge-stations-comes-back-comment')).toBeNull();
  });
});

describe('defaultDetachSlot', () => {
  const a = slot(MAIN, 3);
  const b = slot(NORTH, 0);
  const c = slot(SPLIT_B, 0);
  it('follows the previews', () => {
    const previews = new Map([
      [MAIN, MINTS],
      [NORTH, RESTORES],
      [SPLIT_B, MINTS],
    ]);
    expect(defaultDetachSlot([a, c, b], previews)).toEqual(b);
    expect(defaultDetachSlot([b, a], previews)).toEqual(b);
    expect(defaultDetachSlot([a, c], previews)).toEqual(a);
    // Still asking, or the clicked line's preview failed: the clicked slot.
    expect(defaultDetachSlot([a, b], new Map())).toEqual(a);
    expect(defaultDetachSlot([a, b], new Map([[NORTH, RESTORES]]))).toEqual(a);
    // A clicked owner that keeps the junction gives way to a line that
    // comes off alone (V-1); with none, it stays selected.
    expect(defaultDetachSlot([a, b], new Map([[MAIN, KEEPS], [NORTH, RESTORES]]))).toEqual(b);
    expect(defaultDetachSlot([a, c], new Map([[MAIN, KEEPS], [SPLIT_B, MINTS]]))).toEqual(a);
    expect(defaultDetachSlot([], previews)).toBeUndefined();
  });
});
