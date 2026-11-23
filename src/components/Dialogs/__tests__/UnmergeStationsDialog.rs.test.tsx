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
import UnmergeStationsDialog, { letterTakenEarlier } from '../UnmergeStationsDialog';

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
  shown_elsewhere: [],
  comments_hidden: [],
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
    // Rule L's default answer letters the station that comes back.
    expect(await screen.findByTestId('unmerge-stations-comes-back')).toHaveTextContent('It gets back 0a (Station ID 40).');
    expect(screen.getByTestId(`unmerge-station-option-${SIDE}-0`)).toBeChecked();
    expect(screen.getByTestId('unmerge-stations-comes-back-comment')).toHaveTextContent('With its comment: "Silt bank"');
    expect(screen.getByTestId('unmerge-stations-together')).toHaveTextContent(
      'North comes off with Side: they were one station before the merge.',
    );
    // No line shows a letter after 0: the note on earlier letters says
    // nothing true here (RS-FX6, P3-13).
    expect(screen.queryByTestId('unmerge-stations-letters-stay')).toBeNull();
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

/**
 * Package RS-FX1 (model review finding 2, farm 647783847b78): the detach
 * also says which stations elsewhere show another station afterwards (a loop
 * end whose closure's kept end comes off), every comment it hides (a
 * promotion its sweep makes), and, when the line keeps the junction, the
 * junction's own name as what the line shows instead of a loop closure's
 * station. Injections: dropping `ShownElsewhere` or `HiddenComments` fails
 * the first case; saying `preview.name` in `shownBefore` fails the second.
 */
describe('Detach Line from Station names what changes elsewhere (RS-FX1)', () => {
  it('lists the stations shown differently elsewhere and the hidden comments', async () => {
    invokeIpc.mockImplementation(async () =>
      base({
        shown_elsewhere: [
          {
            station: slot(NORTH, 2),
            before: { id: 3, name: '3', home_line: MAIN },
            after: { id: 40, name: '0', home_line: SIDE },
          },
        ],
        comments_hidden: [{ id: 28, name: '7', home_line: NORTH, comment: 'Low air' }],
      }),
    );
    open({ chosenLine: SIDE });
    expect(await screen.findByTestId('unmerge-stations-shown-elsewhere')).toHaveTextContent(
      'Elsewhere, a loop closure or a merge the detach makes changes what these stations show:',
    );
    expect(screen.getByTestId(`unmerge-stations-shown-elsewhere-${NORTH}-2`)).toHaveTextContent(
      'North: Side / 0 where it showed Main / 3',
    );
    expect(screen.getByTestId('unmerge-stations-comments-hidden-28')).toHaveTextContent(
      'North / 7\'s comment ("Low air") is hidden. Undo brings it back.',
    );
  });

  it("says the junction's own name when the line keeps the junction", async () => {
    invokeIpc.mockImplementation(async () =>
      base({
        name: '0',
        station_id: 40,
        kept: true,
        shown_before: { id: 8, name: '8', home_line: NORTH },
        lines: [
          { line_id: MAIN, name: '3', station_id: 3, comes_off: false, joined_with: [] },
          { line_id: SIDE, name: '0', station_id: 40, comes_off: true, joined_with: [NORTH] },
          { line_id: NORTH, name: '0', station_id: 40, comes_off: true, joined_with: [SIDE] },
        ],
      }),
    );
    open({ chosenLine: MAIN });
    expect(await screen.findByTestId('unmerge-stations-shown-before')).toHaveTextContent(
      'Main showed North / 8 here through a loop closure; it will show 3.',
    );
    expect(screen.queryByTestId('unmerge-stations-shown-elsewhere')).toBeNull();
    expect(screen.queryByTestId('unmerge-stations-comments-hidden')).toBeNull();
  });
});

/**
 * RS-FX6, the real-app capture of the RS redesign:
 * - P1-1: the two stations of a collision share a name, so each answer names
 *   the one it renames by its role and Station ID. Before, both read "Rename
 *   Side / 0 to 0a".
 * - P2-2: the lead and the After Detach list follow the chosen answer: with
 *   the other answer the station comes back as 0, not rule L's 0a.
 * - P3-13: the note on earlier letters shows only when a line the station
 *   comes back to carries a letter after its name.
 * - P3-15: the remaining-junction picker draws pills.
 * - P3-12: a station an earlier merge archived inside the one that comes
 *   back stays inside it, and the dialog says a second Detach gives it back.
 * Injections: word the answers by `identityLabelOf` alone (P1-1); print
 * `preview.name` in `ComesBack` (P2-2); show the note whenever a station is
 * restored (P3-13); drop `recordsInside` (P3-12).
 */
describe('Detach Line from Station after the RS capture (RS-FX6)', () => {
  it('names each answer by role and Station ID, and the lead follows the answer', async () => {
    invokeIpc.mockImplementation(async () =>
      base({
        ...SIDE_BACK,
        together: [],
        name: '0a',
        lines: [
          { line_id: MAIN, name: '3', station_id: 3, comes_off: false, joined_with: [] },
          { line_id: SIDE, name: '0a', station_id: 40, comes_off: true, joined_with: [] },
        ],
      }),
    );
    open({ chosenLine: SIDE });
    const letter = await screen.findByTestId('unmerge-stations-collision-0-letter');
    expect(letter.parentElement).toHaveTextContent('Rename the station that comes back (Side / 0, Station ID 40) to 0a');
    expect(screen.getByTestId('unmerge-stations-collision-0-alternative').parentElement).toHaveTextContent(
      "Rename the 0 that is on Side now (Station ID 44) to 0a",
    );
    expect(screen.getByTestId('unmerge-stations-comes-back')).toHaveTextContent('It gets back 0a (Station ID 40).');
    expect(screen.getByTestId(`unmerge-stations-result-${SIDE}`)).toHaveTextContent('Side: 0a, Station ID 40 (comes off)');

    fireEvent.click(screen.getByTestId('unmerge-stations-collision-0-alternative'));
    expect(screen.getByTestId('unmerge-stations-comes-back')).toHaveTextContent('It gets back 0 (Station ID 40).');
    expect(screen.getByTestId(`unmerge-stations-result-${SIDE}`)).toHaveTextContent('Side: 0, Station ID 40 (comes off)');
  });

  it('notes earlier letters only when a line shows one', async () => {
    useProjectStore.setState({
      stations: [
        { station_ref: slot(MAIN, 3), id: 3, name: '3', merged_with: [slot(SIDE, 0), slot(NORTH, 0)] },
        { station_ref: slot(SIDE, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(NORTH, 0)] },
        { station_ref: slot(SIDE, 1), id: 61, name: '0a', merged_with: [] },
        { station_ref: slot(NORTH, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(SIDE, 0)] },
      ],
    } as never);
    invokeIpc.mockImplementation(async () =>
      base({ lines: [{ line_id: SIDE, name: '0', station_id: 40, comes_off: true, joined_with: [] }] }),
    );
    open({ chosenLine: SIDE });
    expect(await screen.findByTestId('unmerge-stations-letters-stay')).toHaveTextContent(
      'A name that took a letter earlier because of this station keeps it.',
    );
  });

  it('draws the remaining-junction choices as pills', async () => {
    open({ chosenLine: MAIN });
    await screen.findByTestId('unmerge-stations-rest');
    expect(screen.getByTestId('unmerge-stations-rest-50-pill')).toHaveTextContent('North: 9');
    expect(screen.getByTestId('unmerge-stations-comes-back')).toHaveTextContent('They get back 0 (Station ID 40).');
  });

  it('says a station archived inside the one that comes back stays inside it', async () => {
    useProjectStore.setState({
      stations: [
        {
          station_ref: slot(MAIN, 3),
          id: 3,
          name: '3',
          merged_with: [slot(SIDE, 0), slot(NORTH, 0)],
          archived: [
            { id: 40, name: '0', lines: [SIDE] },
            { id: 46, name: '0', into: 40, lines: [NORTH] },
          ],
        },
        { station_ref: slot(SIDE, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(NORTH, 0)] },
        { station_ref: slot(NORTH, 0), id: 3, name: '3', merged_with: [slot(MAIN, 3), slot(SIDE, 0)] },
      ],
    } as never);
    invokeIpc.mockImplementation(async () => base({}));
    open({ chosenLine: SIDE });
    expect(await screen.findByTestId('unmerge-stations-inside-46')).toHaveTextContent(
      'North / 0 (Station ID 46) stays archived inside it: detach North again to get it back.',
    );
  });
});

describe('letterTakenEarlier', () => {
  const names = (line: number) => (line === 1 ? ['0', '1', '0a'] : ['0', '10', '0A', '0-1']);
  it('finds a letter after the name, and nothing else', () => {
    expect(letterTakenEarlier('0', [1], names)).toBe(true);
    expect(letterTakenEarlier('0', [2], names)).toBe(false);
    expect(letterTakenEarlier('', [1], names)).toBe(false);
  });
});
