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
    expect(screen.getByTestId('remove-closure-option-5-back')).toHaveTextContent('(17 comes back)');
    // The preview's collision: by default rule L letters the station that
    // comes back, so it comes back as 17a (RS-FX8, N2).
    expect(await screen.findByTestId('remove-closure-comes-back')).toHaveTextContent(
      'Side / 17a comes back: Side shows it again instead of Main / 3.',
    );
    expect(screen.getByTestId('remove-closure-comes-back-comment')).toHaveTextContent('Its comment comes back too: "Arch"');
    expect(invokeIpc).toHaveBeenCalledWith('preview_remove_loop_closure', { closureId: 5 }, expect.anything());
  });

  // RS-FX6, P1-1: both answers rename a station named 17; each says which,
  // by its role and Station ID. Injection: word them by `identityLabelOf`
  // alone and both read "Rename Side / 17 to 17a".
  it('names each answer by role and Station ID', async () => {
    render(<RemoveClosureDialog />);
    const letter = await screen.findByTestId('remove-closure-collision-0-letter');
    expect(letter.parentElement).toHaveTextContent('Rename the station that comes back (Side / 17, Station ID 17) to 17a');
    expect(screen.getByTestId('remove-closure-collision-0-alternative').parentElement).toHaveTextContent(
      "Rename the 17 that is on Side now (Station ID 71) to 17a",
    );
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

  it('says no name changes for a closure that hides nothing', async () => {
    invokeIpc.mockResolvedValue({ closure_id: 5n, renamed_lines: [], collisions: [] });
    render(<RemoveClosureDialog />);
    expect(await screen.findByTestId('remove-closure-nothing-back')).toHaveTextContent(
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
    expect(screen.getByTestId('remove-closure-option-5-back')).toHaveTextContent('(17a comes back)');
    expect(sentence).toHaveTextContent('Side / 17a comes back: Side shows it again instead of Main / 3.');
    fireEvent.click(screen.getByTestId('remove-closure-collision-0-alternative'));
    expect(screen.getByTestId('remove-closure-option-5-back')).toHaveTextContent('(17 comes back)');
    expect(screen.getByTestId('remove-closure-comes-back')).toHaveTextContent('Side / 17 comes back');
  });
});
