/**
 * RS 4.1, 7: Merge Stations asks which station is kept. One radio per station
 * (its pill: line, own name, Station ID), the backend's default checked; for
 * the chosen keep the dialog says which lines will show the kept name, which
 * station is archived ("with its comment" only when it has one), that Detach
 * Line from Station brings that station back, and one row per name collision with its
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
import { pillDrawings, pillKinds, said } from './proseSentence';
import { accessibleText } from '../../../test/accessibleText';

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
    expect(said(screen.getByTestId('merge-keep'))).toContain('Keep (the station kept is shown on every line):');
    expect(screen.getByTestId('merge-keep-12')).toBeChecked();
    expect(screen.getByTestId('merge-keep-40')).not.toBeChecked();
    expect(said(screen.getByTestId('merge-keep-12-datum'))).toContain('(zero datum)');
    expect(said(screen.getByTestId('merge-renamed-lines'))).toContain('Side will show Main / 12.');
    expect(said(screen.getByTestId('merge-hidden'))).toContain('Side / 0 is archived with its comment.');
    expect(said(screen.getByTestId('merge-hidden-back'))).toContain('Detach Line from Station brings Side / 0 back.');
    // The geometry is the same for either keep.
    expect(said(screen.getByTestId('merge-preview-move'))).toContain("Side moves 24 m onto Main / 12's position.");
  });

  // RS-PROSE: each sentence draws the lines and stations it names as prose
  // pills (an identity by its home line, with its Station ID), and reads
  // word for word as plain text wrote it. Injection: render a sentence's
  // plain string fails the pill lists; render the token's words visibly
  // fails the exact sentences.
  it('draws the names of what the keep does as prose pills', () => {
    open(preview());
    const renamed = screen.getByTestId('merge-renamed-lines');
    expect(accessibleText(renamed)).toBe('Side will show Main / 12.');
    expect(pillKinds(renamed)).toEqual(['line', 'station']);
    expect(pillDrawings(renamed)).toEqual(['Side', 'Main: 12#12']);
    const hidden = screen.getByTestId('merge-hidden');
    expect(accessibleText(hidden)).toBe('Side / 0 is archived with its comment.');
    expect(pillDrawings(hidden)).toEqual(['Side: 0#40']);
    const back = screen.getByTestId('merge-hidden-back');
    expect(accessibleText(back)).toBe('Detach Line from Station brings Side / 0 back.');
    expect(pillKinds(back)).toEqual(['station']);
    const move = screen.getByTestId('merge-preview-move');
    expect(accessibleText(move)).toBe("Side moves 24 m onto Main / 12's position.");
    expect(pillKinds(move)).toEqual(['line', 'station']);
  });

  it('follows the other keep: other lines, the other archived station, no comment', () => {
    open(preview());
    fireEvent.click(screen.getByTestId('merge-keep-40'));
    expect(said(screen.getByTestId('merge-renamed-lines'))).toContain('Main will show Side / 0.');
    expect(said(screen.getByTestId('merge-hidden'))).toMatch(/^Main \/ 12 is archived\.$/);
    expect(screen.queryByTestId('merge-collisions')).toBeNull();
  });

  // RS-FX6, P1-1: both stations are named 12, so each answer says which it
  // renames, by its role and Station ID ("Rename Side / 12 to 12a" twice
  // could not be told apart). Injection: word both answers by
  // `identityLabelOf` alone again.
  it('lists a collision with its two answers, the letter first and checked, each by role and Station ID', () => {
    open(preview());
    const row = screen.getByTestId('merge-collision-0');
    expect(said(row)).toContain('On Side, 12 would name two stations');
    expect(screen.getByTestId('merge-collision-0-letter')).toBeChecked();
    expect(said(screen.getByTestId('merge-collision-0-letter').parentElement)).toContain(
      "Rename the 12 that is on Side now (Station ID 41) to 12a",
    );
    expect(said(screen.getByTestId('merge-collision-0-alternative').parentElement)).toContain(
      'Rename the kept station (Main / 12, Station ID 12) to 12a',
    );
  });

  // RS-PROSE: the legend's line and each answer's station and line are
  // pills; the answer says the Station ID in words, so its pill draws none.
  // Each radio's accessible name is still the plain sentence. Injection: the
  // plain `collisionAnswerText` in the label fails the pill lists.
  it('draws a collision row\'s names as pills, its radios named by the plain sentence', () => {
    open(preview());
    const legend = screen.getByTestId('merge-collision-0').querySelector('legend')!;
    expect(accessibleText(legend)).toBe('On Side, 12 would name two stations');
    expect(pillKinds(legend)).toEqual(['line']);
    const letter = screen.getByRole('radio', { name: 'Rename the 12 that is on Side now (Station ID 41) to 12a' });
    expect(letter).toBe(screen.getByTestId('merge-collision-0-letter'));
    expect(pillKinds(letter.parentElement!)).toEqual(['station', 'line']);
    expect(pillDrawings(letter.parentElement!)).toEqual(['12', 'Side']);
    const alternative = screen.getByRole('radio', { name: 'Rename the kept station (Main / 12, Station ID 12) to 12a' });
    expect(alternative).toBe(screen.getByTestId('merge-collision-0-alternative'));
    expect(pillDrawings(alternative.parentElement!)).toEqual(['Main: 12']);
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
    expect(said(screen.getByTestId('merge-keep-forced'))).toContain(
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
    expect(said(screen.getByTestId('merge-keep-40-shown-as'))).toContain('shown as Main / 3 through a loop closure');
    expect(pillKinds(screen.getByTestId('merge-keep-40-shown-as'))).toEqual(['station']);
    // Its pill names its own station, not what the closure shows it as.
    expect(said(screen.getByTestId('merge-keep-40-pill'))).toContain('Side: 0');
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
    expect(said(screen.getByTestId('merge-loop-end-shown'))).toContain(
      'It was the hidden end of a loop closure: the closure stays, and Main / 12 is shown at that end too.',
    );
    expect(said(screen.getByTestId('merge-hidden-back'))).toMatch(/^Detach Line from Station brings Side \/ 0 back\.$/);
    expect(said(screen.getByTestId('merge-comments-hidden-28'))).toContain(
      'Main / 3\'s comment ("Low air") is hidden. Undo brings it back.',
    );
    // RS-PROSE: the kept station and the hidden comment's station are pills.
    expect(pillKinds(screen.getByTestId('merge-loop-end-shown'))).toEqual(['station']);
    expect(accessibleText(screen.getByTestId('merge-comments-hidden-28'))).toBe(
      'Main / 3\'s comment ("Low air") is hidden. Undo brings it back.',
    );
    expect(pillDrawings(screen.getByTestId('merge-comments-hidden-28'))).toEqual(['Main: 3#28']);
  });

  it('says none of it when the keep hides nothing else', () => {
    open(preview());
    expect(screen.queryByTestId('merge-loop-end-shown')).toBeNull();
    expect(screen.queryByTestId('merge-comments-hidden')).toBeNull();
    expect(said(screen.getByTestId('merge-hidden-back'))).toMatch(/^Detach Line from Station brings Side \/ 0 back\.$/);
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
    expect(said(screen.getByTestId(`merge-repeat-${SIDE}-41`))).toContain(
      "Side already has a station 12 (Station ID 41); it keeps its name, and Side shows 12 twice, the junction with Main's dot.",
    );
    // RS-PROSE: the lines are pills; the name it repeats stays a name.
    expect(pillKinds(screen.getByTestId(`merge-repeat-${SIDE}-41`))).toEqual(['line', 'line', 'line']);
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
    expect(said(screen.getByTestId('merge-hidden-inside-46'))).toContain(
      'Main / 0 (Station ID 46), archived inside Side / 0 by an earlier merge, stays inside it: a second Detach brings it back.',
    );
    // RS-PROSE: the station whose Station ID the words say draws none.
    expect(pillDrawings(screen.getByTestId('merge-hidden-inside-46'))).toEqual(['Main: 0', 'Side: 0#40']);
    // RS-FX8, N4: Detach names the station it brings back and comes before
    // the inside sentence, so no "it" points at the station inside.
    const back = screen.getByTestId('merge-hidden-back');
    expect(said(back)).toContain('Detach Line from Station brings Side / 0 back.');
    expect(back.compareDocumentPosition(screen.getByTestId('merge-hidden-inside-46')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
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
    expect(said(screen.getByTestId('merge-preview-depths'))).toContain(
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

/**
 * RS-FX8, N2 (checked in Merge as in Remove Closure): the answer that letters
 * the kept station renames it everywhere, so "Side will show Main / 12"
 * follows the answer. Injection: word the kept station by its preview name.
 */
describe('Merge Stations names the kept station as the answers leave it (RS-FX8)', () => {
  it('says Main / 12a once the kept station takes the letter', () => {
    open(preview());
    expect(said(screen.getByTestId('merge-renamed-lines'))).toContain('Side will show Main / 12.');
    fireEvent.click(screen.getByTestId('merge-collision-0-alternative'));
    expect(said(screen.getByTestId('merge-renamed-lines'))).toContain('Side will show Main / 12a.');
  });
});
