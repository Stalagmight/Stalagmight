/**
 * Detach Line from Station's remaining-junction picker (RS 4.2 case 2, review
 * finding ui-1): what the dialog shows, the record it sends (`restKeep`) and
 * the renames it sends (built from the collision rows) always come from ONE
 * preview, the one on screen.
 *
 * - A pick asks the line's preview again with it; leaving the line clears the
 *   pick, and coming back shows the line's default preview again (not the
 *   pick's), so Detach sends no `restKeep` and no rename built from the
 *   pick's collisions.
 * - While the pick's preview is out, Detach waits.
 *
 * The lines that come off (Side and North) get the station back together,
 * so the lead says "They get back" (RS-FX6, P3-15), and it names the station
 * as the chosen answer leaves it: rule L's letter by default (P2-2).
 *
 * Injections (333e24ac5): the pick's preview overwrote the line's default
 * preview, so "coming back shows the default" fails (it reads "9 (Station ID
 * 50)" and sends 9a); Confirm stayed enabled while the pick's preview was
 * out, so "waits" fails.
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
import UnmergeStationsDialog, { recordsInside } from '../UnmergeStationsDialog';

const MAIN = 1;
const SIDE = 2;
const NORTH = 3;
const TWIG = 4;
const EAST = 5;
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
  shown_elsewhere: [],
  comments_hidden: [],
  ...extra,
});

/** Main keeps the junction; the others come off on rule K's record 40 by
 *  default, nothing collides. */
const MAIN_DEFAULT = base({
  kept: true,
  rest_choices: [
    { id: 40, name: '0', home_line: SIDE },
    { id: 50, name: '9', home_line: NORTH },
  ],
  rest_default: 40,
});
/** The same with record 50 picked: it shows 9, which North already shows. */
const MAIN_PICKED = base({
  ...MAIN_DEFAULT,
  name: '9',
  station_id: 50,
  collisions: [
    {
      line_id: NORTH,
      name: '9',
      letter: { station: { id: 50, name: '9', home_line: NORTH }, to: '9a' },
      alternative: { station: { id: 77, name: '9', home_line: NORTH }, to: '9a' },
    },
  ],
});

let resolvePicked: ((p: DetachLinePreview) => void) | null = null;
let deferPicked = false;

beforeEach(() => {
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  vi.clearAllMocks();
  resolvePicked = null;
  deferPicked = false;
  unmergeStations.mockResolvedValue({ success: true });
  unmergeStationPair.mockResolvedValue({ success: true });
  invokeIpc.mockImplementation((_cmd: string, args: { lineId: number; restKeep?: number }) => {
    if (args.lineId === MAIN) {
      if (args.restKeep === 50) {
        if (deferPicked) return new Promise<DetachLinePreview>((resolve) => (resolvePicked = resolve));
        return Promise.resolve(MAIN_PICKED);
      }
      return Promise.resolve(MAIN_DEFAULT);
    }
    if (args.lineId === SIDE) return Promise.resolve(base({ name: '0', station_id: 44 }));
    return Promise.resolve(base({ name: '9', station_id: 50 }));
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
  useUIStore.setState({
    closeDialog,
    dialogData: { stationRef: slot(MAIN, 3), mergedStations: [slot(SIDE, 0), slot(NORTH, 0)], chosenLine: MAIN },
  } as never);
});

describe('the remaining-junction pick and the preview on screen', () => {
  it('coming back to the line shows its default preview and sends no pick', async () => {
    render(<UnmergeStationsDialog />);
    await screen.findByTestId('unmerge-stations-rest');
    fireEvent.click(screen.getByTestId('unmerge-stations-rest-50'));
    await waitFor(() =>
      expect(screen.getByTestId('unmerge-stations-comes-back')).toHaveTextContent('They get back 9a (Station ID 50).'),
    );
    expect(screen.getByTestId('unmerge-stations-collision-0')).toBeInTheDocument();

    // Another line, then back: the pick belonged to the line it was made for.
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${SIDE}-0`));
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${MAIN}-3`));
    expect(await screen.findByTestId('unmerge-stations-rest-40')).toBeChecked();
    expect(screen.getByTestId('unmerge-stations-comes-back')).toHaveTextContent('They get back 0 (Station ID 40).');
    expect(screen.queryByTestId('unmerge-stations-collision-0')).toBeNull();

    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(unmergeStations).toHaveBeenCalledWith(slot(MAIN, 3), { renames: [] });
  });

  it("waits for the pick's preview before Detach, then sends the pick with its answers", async () => {
    deferPicked = true;
    render(<UnmergeStationsDialog />);
    await screen.findByTestId('unmerge-stations-rest');
    fireEvent.click(screen.getByTestId('unmerge-stations-rest-50'));
    await waitFor(() => expect(resolvePicked).not.toBeNull());
    expect(screen.getByTestId('unmerge-stations-confirm')).toBeDisabled();

    resolvePicked!(MAIN_PICKED);
    await waitFor(() => expect(screen.getByTestId('unmerge-stations-confirm')).not.toBeDisabled());
    expect(screen.getByTestId('unmerge-stations-comes-back')).toHaveTextContent('They get back 9a (Station ID 50).');
    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(unmergeStations).toHaveBeenCalledWith(slot(MAIN, 3), {
      renames: [{ station_id: 50, name: '9a' }],
      restKeep: 50,
    });
  });

  it('a pick whose preview fails goes back to the default and says so', async () => {
    invokeIpc.mockImplementation((_cmd: string, args: { lineId: number; restKeep?: number }) => {
      if (args.lineId === MAIN) {
        return args.restKeep === 50 ? Promise.reject(new Error('gone')) : Promise.resolve(MAIN_DEFAULT);
      }
      return Promise.resolve(base({ name: '0', station_id: 44 }));
    });
    render(<UnmergeStationsDialog />);
    await screen.findByTestId('unmerge-stations-rest');
    fireEvent.click(screen.getByTestId('unmerge-stations-rest-50'));
    await waitFor(() => expect(screen.getByTestId('unmerge-stations-rest-40')).toBeChecked());
    expect(screen.getByTestId('unmerge-stations-rest-failed')).toBeInTheDocument();
    expect(screen.getByTestId('unmerge-stations-confirm')).not.toBeDisabled();
  });
});

/**
 * RS-FX8, N7 (shots 20, 21 of the RS capture): when the detached line keeps
 * the junction, the lines that come off take every other archived station
 * with them, inside the one they get back. Picking North / 9 as that station
 * left Side / 0 archived in it, unsaid. The dialog names each one for either
 * pick, and the ones archived inside the picked station as before; a history
 * record stays with the junction.
 *
 * RS-FX9 item 6: the stations nested inside one that moves move too (the
 * backend's `detach_others` takes every record but the junction's history),
 * so the lead names them with the station they are inside: picking 50, Twig
 * / 7 (41, archived inside Side / 0 when Side's junction was merged) is
 * named on Side / 0's row. The fixture's records follow A3 (one record per
 * line: 41 names Twig, not Side). Injection: on 3b6bdf77e the row named Side
 * / 0 alone and 41 nowhere.
 */
describe('what stays archived inside the remaining junction (RS-FX8)', () => {
  beforeEach(() => {
    useProjectStore.setState({
      project: {
        lines: [
          { id: MAIN, name: 'Main', shots: [] },
          { id: SIDE, name: 'Side', shots: [] },
          { id: NORTH, name: 'North', shots: [] },
          { id: TWIG, name: 'Twig', shots: [] },
        ],
      },
      stations: [
        {
          station_ref: slot(MAIN, 3),
          id: 3,
          name: '3',
          merged_with: [slot(SIDE, 0), slot(NORTH, 0), slot(TWIG, 0)],
          archived: [
            { id: 40, name: '0', lines: [SIDE] },
            { id: 41, name: '7', into: 40, lines: [TWIG] },
            { id: 50, name: '9', lines: [NORTH] },
            { id: 60, name: '5', lines: [] },
          ],
        },
        { station_ref: slot(SIDE, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(NORTH, 0), slot(TWIG, 0)] },
        { station_ref: slot(NORTH, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(SIDE, 0), slot(TWIG, 0)] },
        { station_ref: slot(TWIG, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(SIDE, 0), slot(NORTH, 0)] },
      ],
    } as never);
    useUIStore.setState({
      dialogData: {
        stationRef: slot(MAIN, 3),
        mergedStations: [slot(SIDE, 0), slot(NORTH, 0), slot(TWIG, 0)],
        chosenLine: MAIN,
      },
    } as never);
  });

  it('names every station that moves, nested ones with the station they are inside', async () => {
    render(<UnmergeStationsDialog />);
    await screen.findByTestId('unmerge-stations-rest');
    // Default pick 40: 41 was inside it and is directly inside the station
    // they get back now; North / 9 goes with the lines too.
    expect(screen.getByTestId('unmerge-stations-inside-41')).toHaveTextContent(
      'Twig / 7 (Station ID 41) stays archived inside it: detach Twig again to get it back.',
    );
    expect(screen.getByTestId('unmerge-stations-inside-50')).toHaveTextContent(
      'North / 9 (Station ID 50) stays archived inside it: detach North again to get it back.',
    );
    expect(screen.queryByTestId('unmerge-stations-inside-60')).toBeNull();

    fireEvent.click(screen.getByTestId('unmerge-stations-rest-50'));
    await waitFor(() =>
      expect(screen.getByTestId('unmerge-stations-comes-back')).toHaveTextContent('They get back 9a (Station ID 50).'),
    );
    // Picked 50: Side / 0 stays archived in it, with Twig / 7 still inside
    // Side / 0, and nothing names 50 itself or the history record.
    expect(screen.getByTestId('unmerge-stations-inside-40')).toHaveTextContent(
      'Side / 0 (Station ID 40) stays archived inside it, with Twig / 7 (Station ID 41) archived inside Side / 0: ' +
        'detach Side again to get it back.',
    );
    expect(screen.queryByTestId('unmerge-stations-inside-41')).toBeNull();
    expect(screen.queryByTestId('unmerge-stations-inside-50')).toBeNull();
    expect(screen.queryByTestId('unmerge-stations-inside-60')).toBeNull();
  });
});

describe('recordsInside', () => {
  const records = [
    { id: 40, name: '0', lines: [SIDE] },
    { id: 41, name: '7', into: 40, lines: [TWIG] },
    { id: 42, name: '8', into: 41, lines: [EAST] },
    { id: 50, name: '9', lines: [NORTH] },
    { id: 60, name: '5', lines: [] },
    { id: 61, name: '6', into: 41, lines: [] },
  ];
  const rows = (id: number, kept?: boolean) =>
    recordsInside(records, id, kept).map((row) => [row.record.id, row.inner.map((r) => r.id)]);
  it('lists every station that moves inside the one that comes back, nested ones under their row', () => {
    // 40 comes back: 41 directly inside it, 42 inside 41; history never.
    expect(rows(40)).toEqual([[41, [42]]]);
    expect(rows(41)).toEqual([[42, []]]);
    // Kept (case 2): every record but the junction's history moves.
    expect(rows(40, true)).toEqual([
      [41, [42]],
      [50, []],
    ]);
    expect(rows(50, true)).toEqual([[40, [41, 42]]]);
  });
});
