/**
 * RS 4.1, 7: Merge Stations asks which station is kept. One radio per station
 * (its pill: line, own name, Station ID), the backend's default checked; for
 * the chosen keep the dialog says which lines will show the kept name, which
 * station is archived ("with its comment" only when it has one), that Detach
 * Line from Station brings it back, and one row per name collision with its
 * two answers. Merge sends the keep and the answers. A loop closure between
 * the two forces the keep and the dialog says so; a station a loop closure
 * shows as another says "shown as K through a loop closure".
 *
 * Injections: sending no `keep` fails "merges keeping the station chosen";
 * sending the letter whatever the radio says fails "sends the other answer";
 * dropping "with its comment" fails the comment case.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { MergePreview } from '../../../types/ipc/MergePreview';
import type { NameCollision } from '../../../types/ipc/NameCollision';

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import MergeDialog, { defaultKeepOf, groupDepthReadings } from '../MergeDialog';

const MAIN = 1;
const SIDE = 2;
const A = { line_id: SIDE, station_index: 0 };
const B = { line_id: MAIN, station_index: 12 };
const mergeStations = vi.fn();
const executeBatch = vi.fn();
const closeDialog = vi.fn();

const main12 = { id: 12, name: '12', home_line: MAIN };
const side0 = { id: 40, name: '0', home_line: SIDE, comment: 'Silt bank' };

/** On Side, keeping Main / 12 shows 12 next to Side's own 12 (Station ID 41). */
const collision: NameCollision = {
  line_id: SIDE,
  name: '12',
  letter: { station: { id: 41, name: '12', home_line: SIDE }, to: '12a' },
  alternative: { station: main12, to: '12a' },
};

function preview(extra: Partial<MergePreview> = {}): MergePreview {
  return {
    moving_line_ids: [SIDE],
    shift_m: 24,
    joins: B,
    junctions: [],
    candidates: [
      { station: A, identity: side0, holds_datum: false },
      { station: B, identity: main12, holds_datum: true },
    ],
    default_keep: 12,
    keeps: [
      { keep: 40, renamed_lines: [MAIN], collisions: [], hidden: { id: 12, name: '12', home_line: MAIN }, comments_hidden: [], repeats: [] },
      { keep: 12, renamed_lines: [SIDE], collisions: [collision], hidden: side0, comments_hidden: [], repeats: [] },
    ],
    ...extra,
  };
}

function open(mergePreview: MergePreview, extra: Record<string, unknown> = {}) {
  useUIStore.setState({
    closeDialog,
    clearMergeStations: vi.fn(),
    dialogData: { stationA: A, stationB: B, mergePreview, ...extra },
  } as never);
  render(<MergeDialog />);
}

beforeEach(() => {
  // A local project: editing is not blocked (useEditingBlocked).
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  vi.clearAllMocks();
  mergeStations.mockResolvedValue({ success: true, warnings: [], line_ids: [] });
  executeBatch.mockResolvedValue({ success: true, warnings: [], line_ids: [] });
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
      ],
    },
    surveyData: null,
    stations: [
      { station_ref: A, id: 40, name: '0', merged_with: [] },
      { station_ref: B, id: 12, name: '12', merged_with: [] },
    ],
    mergeStations,
    executeBatch,
  } as never);
});

describe('Merge Stations keeps one station (RS 4.1)', () => {
  it('offers both stations with the default checked, and says what the keep does', () => {
    open(preview());
    expect(screen.getByTestId('merge-keep')).toHaveTextContent('Keep (the station kept is shown on every line):');
    expect(screen.getByTestId('merge-keep-12')).toBeChecked();
    expect(screen.getByTestId('merge-keep-40')).not.toBeChecked();
    expect(screen.getByTestId('merge-keep-12-datum')).toHaveTextContent('(zero datum)');
    expect(screen.getByTestId('merge-renamed-lines')).toHaveTextContent('Side will show Main / 12.');
    expect(screen.getByTestId('merge-hidden')).toHaveTextContent('Side / 0 is archived with its comment.');
    expect(screen.getByTestId('merge-hidden-back')).toHaveTextContent('Detach Line from Station brings it back.');
    // The geometry is the same for either keep.
    expect(screen.getByTestId('merge-preview-move')).toHaveTextContent("Side moves 24 m onto Main / 12's position.");
  });

  it('follows the other keep: other lines, the other archived station, no comment', () => {
    open(preview());
    fireEvent.click(screen.getByTestId('merge-keep-40'));
    expect(screen.getByTestId('merge-renamed-lines')).toHaveTextContent('Main will show Side / 0.');
    expect(screen.getByTestId('merge-hidden')).toHaveTextContent(/^Main \/ 12 is archived\.$/);
    expect(screen.queryByTestId('merge-collisions')).toBeNull();
  });

  // RS-FX6, P1-1: both stations are named 12, so each answer says which it
  // renames, by its role and Station ID ("Rename Side / 12 to 12a" twice
  // could not be told apart). Injection: word both answers by
  // `identityLabelOf` alone again.
  it('lists a collision with its two answers, the letter first and checked, each by role and Station ID', () => {
    open(preview());
    const row = screen.getByTestId('merge-collision-0');
    expect(row).toHaveTextContent('On Side, 12 would name two stations');
    expect(screen.getByTestId('merge-collision-0-letter')).toBeChecked();
    expect(screen.getByTestId('merge-collision-0-letter').parentElement).toHaveTextContent(
      "Rename Side's own 12 (Station ID 41) to 12a",
    );
    expect(screen.getByTestId('merge-collision-0-alternative').parentElement).toHaveTextContent(
      'Rename the kept station (Main / 12, Station ID 12) to 12a',
    );
  });

  it('merges keeping the station chosen, with the letter answer', async () => {
    open(preview());
    fireEvent.click(screen.getByTestId('merge-confirm'));
    await waitFor(() => expect(mergeStations).toHaveBeenCalled());
    expect(mergeStations).toHaveBeenCalledWith(A, B, { keep: 12, renames: [{ station_id: 41, name: '12a' }] });
  });

  it('sends the other answer when the user picks it', async () => {
    open(preview());
    fireEvent.click(screen.getByTestId('merge-collision-0-alternative'));
    fireEvent.click(screen.getByTestId('merge-confirm'));
    await waitFor(() => expect(mergeStations).toHaveBeenCalled());
    expect(mergeStations).toHaveBeenCalledWith(A, B, { keep: 12, renames: [{ station_id: 12, name: '12a' }] });
  });

  it('carries the keep and the answers in a snap-to-merge batch', async () => {
    const move = { type: 'move_station', station_ref: A, offset: { x: 1, y: 0, z: 0 } };
    open(preview(), { preCommands: [move] });
    fireEvent.click(screen.getByTestId('merge-keep-40'));
    fireEvent.click(screen.getByTestId('merge-confirm'));
    await waitFor(() => expect(executeBatch).toHaveBeenCalled());
    expect(executeBatch).toHaveBeenCalledWith([
      move,
      { type: 'merge_stations', station_a: A, station_b: B, keep: 40, renames: [] },
    ]);
  });

  it('says a closure forces the keep and offers the other station disabled (RS 4.1 step 4)', () => {
    open(
      preview({
        keep_forced_by: 7n,
        keeps: [{ keep: 12, renamed_lines: [SIDE], collisions: [], hidden: side0, comments_hidden: [], repeats: [] }],
      }),
    );
    expect(screen.getByTestId('merge-keep-forced')).toHaveTextContent(
      'A loop closure already shows these two stations as one, so its kept station stays.',
    );
    expect(screen.getByTestId('merge-keep-12')).toBeChecked();
    expect(screen.getByTestId('merge-keep-40')).toBeDisabled();
  });

  it('says a closure-archived candidate is shown as another station (RS 4.1 step 5)', () => {
    open(
      preview({
        candidates: [
          { station: A, identity: side0, holds_datum: false, shown_as: { id: 3, name: '3', home_line: MAIN } },
          { station: B, identity: main12, holds_datum: false },
        ],
      }),
    );
    expect(screen.getByTestId('merge-keep-40-shown-as')).toHaveTextContent('shown as Main / 3 through a loop closure');
    // Its pill names its own station, not what the closure shows it as.
    expect(screen.getByTestId('merge-keep-40-pill')).toHaveTextContent('Side: 0');
  });

  it('keeps the old rows and sends no answers without a keep preview', async () => {
    open(preview({ candidates: [], keeps: [] }));
    expect(screen.queryByTestId('merge-keep')).toBeNull();
    expect(screen.getByText('Station A:')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('merge-confirm'));
    await waitFor(() => expect(mergeStations).toHaveBeenCalled());
    expect(mergeStations).toHaveBeenCalledWith(A, B, {});
  });
});

describe('Merge Stations in a read-only view', () => {
  it('offers no Merge in Plan mode', () => {
    useUIStore.setState({ appMode: 'plan' } as never);
    open(preview());
    expect(screen.getByTestId('merge-confirm')).toBeDisabled();
    useUIStore.setState({ appMode: 'survey' } as never);
  });
});

describe('defaultKeepOf', () => {
  it('takes the default when offered, else the one offered, else none', () => {
    expect(defaultKeepOf(preview())).toBe(12);
    expect(
      defaultKeepOf(preview({ default_keep: 99, keeps: [{ keep: 40, renamed_lines: [], collisions: [], hidden: main12, comments_hidden: [], repeats: [] }] })),
    ).toBe(40);
    expect(defaultKeepOf(preview({ keeps: [] }))).toBeNull();
    expect(defaultKeepOf(undefined)).toBeNull();
  });
});

/**
 * Package RS-FX1: the chosen keep also says when the station not kept was a
 * loop closure's hidden end (the closure then hides nothing, so the kept
 * station is shown there, RS 1.3) and every other comment the merge hides (a
 * promotion it makes, RS 12). Detach Line from Station gives the archived
 * station back on every line it covered, its own earlier merges included
 * (RS-FX4), so the dialog promises that without exceptions. Injection:
 * dropping `HiddenComments` fails the first case.
 */
describe('Merge Stations names what else the keep hides (RS-FX1)', () => {
  it('says the loop end and every other hidden comment', () => {
    open(
      preview({
        keeps: [
          { keep: 40, renamed_lines: [MAIN], collisions: [], hidden: main12, comments_hidden: [], repeats: [] },
          {
            keep: 12,
            renamed_lines: [SIDE],
            collisions: [],
            hidden: side0,
            unarchived: 3n,
            comments_hidden: [{ id: 28, name: '3', home_line: MAIN, comment: 'Low air' }],
            repeats: [],
          },
        ],
      }),
    );
    expect(screen.getByTestId('merge-loop-end-shown')).toHaveTextContent(
      'It was the hidden end of a loop closure: the closure stays, and Main / 12 is shown at that end too.',
    );
    expect(screen.getByTestId('merge-hidden-back')).toHaveTextContent(/^Detach Line from Station brings it back\.$/);
    expect(screen.getByTestId('merge-comments-hidden-28')).toHaveTextContent(
      'Main / 3\'s comment ("Low air") is hidden. Undo brings it back.',
    );
  });

  it('says none of it when the keep hides nothing else', () => {
    open(preview());
    expect(screen.queryByTestId('merge-loop-end-shown')).toBeNull();
    expect(screen.queryByTestId('merge-comments-hidden')).toBeNull();
    expect(screen.getByTestId('merge-hidden-back')).toHaveTextContent(/^Detach Line from Station brings it back\.$/);
  });
});

/**
 * RS-FX6: what else the dialog says before the user confirms. P2-6: a line
 * that will show a name twice (RS-FX5 lets the junction's dotted 12 stand
 * beside Side's own 12) is named, each repeat with the line's own station's
 * Station ID. P3-12: a station archived inside the one the merge archives
 * goes with it. P3-19: the depth list gives a line's equal readings once,
 * with their count. Injections: dropping `NameRepeats`, the inside records
 * or the grouping fails its case.
 */
describe('Merge Stations says the name repeats and what travels inside (RS-FX6)', () => {
  it('names each name a line will show twice', () => {
    open(
      preview({
        keeps: [
          { keep: 40, renamed_lines: [MAIN], collisions: [], hidden: main12, comments_hidden: [], repeats: [] },
          {
            keep: 12,
            renamed_lines: [SIDE],
            collisions: [],
            hidden: side0,
            comments_hidden: [],
            repeats: [
              {
                line_id: SIDE,
                name: '12',
                station: { id: 41, name: '12', home_line: SIDE },
                shown: main12,
                loop_end: false,
              },
            ],
          },
        ],
      }),
    );
    expect(screen.getByTestId(`merge-repeat-${SIDE}-41`)).toHaveTextContent(
      "Side already has a station 12 (Station ID 41); it keeps its name, and Side shows 12 twice, the junction with Main's dot.",
    );
    // The other keep makes no repeat: nothing is said.
    fireEvent.click(screen.getByTestId('merge-keep-40'));
    expect(screen.queryByTestId('merge-repeats')).toBeNull();
  });

  it('says a station archived inside the one archived goes with it', () => {
    useProjectStore.setState({
      stations: [
        {
          station_ref: A,
          id: 40,
          name: '0',
          merged_with: [],
          archived: [
            { id: 46, name: '0', lines: [MAIN] },
            { id: 47, name: '9', lines: [] },
          ],
        },
        { station_ref: B, id: 12, name: '12', merged_with: [] },
      ],
    } as never);
    open(preview());
    expect(screen.getByTestId('merge-hidden-inside-46')).toHaveTextContent(
      'Main / 0 (Station ID 46), archived inside Side / 0 by an earlier merge, stays inside it: a second Detach brings it back.',
    );
    // A history record (no line) is not said.
    expect(screen.queryByTestId('merge-hidden-inside-47')).toBeNull();
  });

  it('lists equal readings on one line once, with their count', () => {
    open(
      preview({
        depths: {
          a_m: 6,
          b_m: 7.8,
          joined_m: 7.2,
          readings: [
            { line_id: SIDE, depth_m: 6 },
            { line_id: MAIN, depth_m: 7.8 },
            { line_id: MAIN, depth_m: 7.8 },
          ],
        },
      }),
    );
    expect(screen.getByTestId('merge-preview-depths')).toHaveTextContent(
      'Depths 6.0 m (Side), 7.8 m (Main, 2 readings) become 7.2 m',
    );
  });
});

describe('groupDepthReadings', () => {
  it('groups equal readings by line in order, keeping different ones apart', () => {
    const format = (v: number) => v.toFixed(1);
    expect(
      groupDepthReadings(
        [
          { line_id: 1, depth_m: 7.8 },
          { line_id: 2, depth_m: 7.8 },
          { line_id: 1, depth_m: 7.81 },
          { line_id: 1, depth_m: 7.9 },
        ],
        format,
      ),
    ).toEqual([
      { line_id: 1, depth: '7.8', count: 2 },
      { line_id: 2, depth: '7.8', count: 1 },
      { line_id: 1, depth: '7.9', count: 1 },
    ]);
  });
});
