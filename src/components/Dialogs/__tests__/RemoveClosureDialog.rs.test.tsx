/**
 * RS 5.3, 7: Remove Closure says what comes back. Each closure row names the
 * station it hides ("17 comes back"); for the closure chosen the dialog asks
 * `preview_remove_loop_closure` and says the station that comes back, where,
 * and its comment, then lists each name that would repeat with its two
 * answers. Remove sends the answers. A closure that hides nothing says no
 * name changes.
 *
 * Injection: removing without `renames` fails "removes with the answers".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { RemoveClosureNaming } from '../../../types/ipc/RemoveClosureNaming';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import RemoveClosureDialog from '../RemoveClosureDialog';
import { pillDrawings, pillKinds, said } from './proseSentence';
import { accessibleText } from '../../../test/accessibleText';

const MAIN = 1;
const SIDE = 2;
const A = { line_id: MAIN, station_index: 3 };
const B = { line_id: SIDE, station_index: 17 };
const removeLoopClosure = vi.fn();
const closeDialog = vi.fn();

const NAMING: RemoveClosureNaming = {
  closure_id: 5n,
  archived_slot: B,
  comes_back: { id: 17, name: '17', home_line: SIDE, comment: 'Arch' },
  shown_as: { id: 3, name: '3', home_line: MAIN },
  renamed_lines: [SIDE],
  collisions: [
    {
      line_id: SIDE,
      name: '17',
      letter: { station: { id: 17, name: '17', home_line: SIDE }, to: '17a' },
      alternative: { station: { id: 71, name: '17', home_line: SIDE }, to: '17a' },
    },
  ],
};

beforeEach(() => {
  // A local project: editing is not blocked (useEditingBlocked).
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  vi.clearAllMocks();
  removeLoopClosure.mockResolvedValue({ success: true });
  invokeIpc.mockResolvedValue(NAMING);
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
      ],
      loop_closures: [
        {
          id: 5n,
          station_a: A,
          station_b: B,
          archived: { end: 'b', station: { id: 17, name: '17', home_line: SIDE }, shown_as: { id: 3, name: '3', home_line: MAIN } },
        },
      ],
    },
    surveyData: null,
    stations: [
      { station_ref: A, id: 3, name: '3', merged_with: [] },
      { station_ref: B, id: 17, name: '3', merged_with: [] },
    ],
    removeLoopClosure,
  } as never);
  useUIStore.setState({
    closeDialog,
    dialogData: { anchorStation: A, closures: [{ closureId: 5n, stationRef: B }] },
  } as never);
});

describe('Remove Closure under RS', () => {
  it('says what comes back on the row and for the closure chosen', async () => {
    render(<RemoveClosureDialog />);
    expect(said(screen.getByTestId('remove-closure-option-5-back'))).toContain('(17 comes back)');
    // The preview's collision: by default rule L letters the station that
    // comes back, so it comes back as 17a (RS-FX8, N2).
    expect(said(await screen.findByTestId('remove-closure-comes-back'))).toContain(
      'Side / 17a comes back: Side shows it again instead of Main / 3.',
    );
    expect(said(screen.getByTestId('remove-closure-comes-back-comment'))).toContain('Its comment comes back too: "Arch"');
    expect(invokeIpc).toHaveBeenCalledWith('preview_remove_loop_closure', { closureId: 5 }, expect.anything());
  });

  // RS-PROSE: the row's station that comes back is a pill by its bare name
  // (its home line's colour, its Station ID), and the sentence's stations
  // and line are pills; each reads as plain text wrote it. Injection: the
  // plain `displayStationName` in the row fails its pill list.
  it('draws the station that comes back and its lines as pills', async () => {
    render(<RemoveClosureDialog />);
    const sentence = await screen.findByTestId('remove-closure-comes-back');
    const row = screen.getByTestId('remove-closure-option-5-back');
    expect(accessibleText(row)).toBe('(17a comes back)');
    expect(pillDrawings(row)).toEqual(['17a#17']);
    const said17 = sentence.querySelector('p')!;
    expect(accessibleText(said17)).toBe('Side / 17a comes back: Side shows it again instead of Main / 3.');
    expect(pillKinds(said17)).toEqual(['station', 'line', 'station']);
  });

  // RS-FX6, P1-1: both answers rename a station named 17; each says which,
  // by its role and Station ID. Injection: word them by `identityLabelOf`
  // alone and both read "Rename Side / 17 to 17a".
  it('names each answer by role and Station ID', async () => {
    render(<RemoveClosureDialog />);
    const letter = await screen.findByTestId('remove-closure-collision-0-letter');
    expect(said(letter.parentElement)).toContain('Rename the station that comes back (Side / 17, Station ID 17) to 17a');
    expect(said(screen.getByTestId('remove-closure-collision-0-alternative').parentElement)).toContain(
      "Rename the 17 that is on Side now (Station ID 71) to 17a",
    );
    expect(screen.getByRole('radio', { name: "Rename the 17 that is on Side now (Station ID 71) to 17a" })).toBe(
      screen.getByTestId('remove-closure-collision-0-alternative'),
    );
    expect(pillKinds(letter.parentElement!)).toEqual(['station']);
  });

  it('removes with the answers to the collisions', async () => {
    render(<RemoveClosureDialog />);
    await screen.findByTestId('remove-closure-collision-0');
    fireEvent.click(screen.getByTestId('remove-closure-collision-0-alternative'));
    fireEvent.click(screen.getByTestId('remove-closure-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(removeLoopClosure).toHaveBeenCalledWith(5n, { renames: [{ station_id: 71, name: '17a' }] });
  });

  it('offers no Remove in the Closures Computed View', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' } as never);
    render(<RemoveClosureDialog />);
    expect(screen.getByTestId('remove-closure-confirm')).toBeDisabled();
    useUIStore.setState({ geometryMode: 'measured' } as never);
  });

  // RS-FX9 item 1: the command refuses a rename Edit Station would refuse,
  // so a dialog never sends one. Both answers letter a name that already held
  // a bidi override (an import's): Remove waits and says why, as Edit Station
  // does. Injection: on 3b6bdf77e Remove stayed enabled and sent the name.
  it('does not send a name Edit Station would refuse', async () => {
    const odd = '1\u202E7';
    invokeIpc.mockResolvedValue({
      ...NAMING,
      collisions: [
        {
          line_id: SIDE,
          name: odd,
          letter: { station: { id: 17, name: odd, home_line: SIDE }, to: `${odd}a` },
          alternative: { station: { id: 71, name: odd, home_line: SIDE }, to: `${odd}a` },
        },
      ],
    });
    render(<RemoveClosureDialog />);
    expect(said(await screen.findByTestId('remove-closure-rename-problem'))).toContain(
      'A name cannot contain control characters or invisible formatting characters.',
    );
    expect(screen.getByTestId('remove-closure-confirm')).toBeDisabled();
    fireEvent.click(screen.getByTestId('remove-closure-collision-0-alternative'));
    expect(screen.getByTestId('remove-closure-confirm')).toBeDisabled();
    fireEvent.click(screen.getByTestId('remove-closure-confirm'));
    expect(removeLoopClosure).not.toHaveBeenCalled();
  });

  it('says no name changes for a closure that hides nothing', async () => {
    invokeIpc.mockResolvedValue({ closure_id: 5n, renamed_lines: [], collisions: [] });
    render(<RemoveClosureDialog />);
    expect(said(await screen.findByTestId('remove-closure-nothing-back'))).toContain(
      'Both ends already show their own names, so no name changes.',
    );
  });
});

/**
 * RS-FX8, N2 (shot 54 of the RS capture): with the default answer (rule L
 * letters the station that comes back) the dialog said "(17 comes back)" and
 * "Side / 17 comes back", while the removal gave 17a. The row and the
 * sentence follow the chosen answer, as Detach's do (P2-2). Injection: print
 * `comes_back.name` again.
 */
describe('Remove Closure names what comes back as the answers leave it (RS-FX8)', () => {
  it('says 17a for the letter, 17 when the other station takes it', async () => {
    render(<RemoveClosureDialog />);
    const sentence = await screen.findByTestId('remove-closure-comes-back');
    expect(said(screen.getByTestId('remove-closure-option-5-back'))).toContain('(17a comes back)');
    expect(said(sentence)).toContain('Side / 17a comes back: Side shows it again instead of Main / 3.');
    fireEvent.click(screen.getByTestId('remove-closure-collision-0-alternative'));
    expect(said(screen.getByTestId('remove-closure-option-5-back'))).toContain('(17 comes back)');
    expect(said(screen.getByTestId('remove-closure-comes-back'))).toContain('Side / 17 comes back');
  });
});

/**
 * RS-FX9 item 7: a closure id reaches the dialog as a bigint from the store
 * and as a plain number from an IPC answer (JSON has no bigint), so the
 * dialog compares ids as strings, as its preview check does. With the
 * preview's `closure_id` a number the row still follows the chosen answer.
 * Injection: on 3b6bdf77e the row compared `5 === 5n` and said "(17 comes
 * back)" while the removal gave 17a.
 */
describe('closure ids of mixed number and bigint', () => {
  it('the row follows the answer whatever type the preview gives the id', async () => {
    invokeIpc.mockResolvedValue({ ...NAMING, closure_id: 5 });
    render(<RemoveClosureDialog />);
    await screen.findByTestId('remove-closure-comes-back');
    expect(said(screen.getByTestId('remove-closure-option-5-back'))).toContain('(17a comes back)');
    fireEvent.click(screen.getByTestId('remove-closure-collision-0-alternative'));
    expect(said(screen.getByTestId('remove-closure-option-5-back'))).toContain('(17 comes back)');
  });

  it('the chosen closure and the ends another closure shows match by value', async () => {
    useProjectStore.setState((state: never) => ({
      project: {
        ...(state as { project: object }).project,
        loop_closures: [
          {
            id: 5,
            station_a: A,
            station_b: B,
            archived: { end: 'b', station: { id: 17, name: '17', home_line: SIDE }, shown_as: { id: 3, name: '3', home_line: MAIN } },
          },
        ],
      },
    }) as never);
    render(<RemoveClosureDialog />);
    expect(said(screen.getByTestId('remove-closure-option-5-back'))).toContain('(17');
    expect(said(await screen.findByTestId('remove-closure-comes-back'))).toContain('Side / 17a comes back');
  });
});
