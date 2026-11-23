/**
 * The closure dialogs' notes say what is true (review finding ui-6):
 *
 * - When rule K's end may not be kept, the note gives the backend's reason
 *   (`rule_k_refused`): C1, the OTHER end is already a loop end of another
 *   closure; C2, rule K's end is already shown as the other end. It names
 *   both ends and never says "the end on the longer line" (rule K has tie
 *   rules).
 * - "Keep both names" and a Remove that brings nothing back say no name
 *   changes, but not that each end shows its own name when an end is already
 *   shown as another station through another closure.
 *
 * Injection (333e24ac5): one fixed note blamed the end on the longer line in
 * both cases, and the two outcome sentences ignored an end shown as another
 * station, so every case below fails there.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import type { LoopClosureNaming } from '../../../types/ipc/LoopClosureNaming';
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
import ClosureDialog from '../ClosureDialog';
import RemoveClosureDialog from '../RemoveClosureDialog';

const MAIN = 1;
const SIDE = 2;
const NORTH = 3;
const A = { line_id: MAIN, station_index: 3 };
const B = { line_id: SIDE, station_index: 17 };
const main3 = { id: 3, name: '3', home_line: MAIN };
const side17 = { id: 17, name: '17', home_line: SIDE };
const north9 = { id: 9, name: '9', home_line: NORTH };

beforeEach(() => {
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  vi.clearAllMocks();
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
        { id: NORTH, name: 'North', shots: [] },
      ],
      loop_closures: [],
    },
    surveyData: null,
    stations: [
      { station_ref: A, id: 3, name: '3', merged_with: [] },
      { station_ref: B, id: 17, name: '17', merged_with: [] },
    ],
    createLoopClosure: vi.fn(),
    executeBatch: vi.fn(),
    removeLoopClosure: vi.fn(),
  } as never);
});

function openClosure(answer: LoopClosureNaming) {
  invokeIpc.mockResolvedValue(answer);
  useUIStore.setState({
    activeDialog: 'closure',
    closeDialog: vi.fn(),
    clearMergeStations: vi.fn(),
    dialogData: { stationA: A, stationB: B },
  } as never);
  render(<ClosureDialog />);
}

describe("Create Loop Closure: why rule K's end may not be kept", () => {
  it('C1: the other end is already a loop end of another closure', async () => {
    openClosure({
      ends: [
        { station: A, identity: main3, holds_datum: false },
        { station: B, identity: side17, shown_as: north9, holds_datum: false },
      ],
      allowed: [17],
      rule_k: 3,
      rule_k_refused: 'other_end_archived',
      options: [
        { keep: 17, hidden: main3, renamed_lines: [MAIN] },
        { renamed_lines: [] },
      ],
    });
    const note = await screen.findByTestId('closure-keep-not-allowed');
    expect(note).toHaveTextContent(
      'Main / 3 cannot be kept here: it would hide Side / 17, which another loop closure already shows as North / 9.',
    );
    expect(note).not.toHaveTextContent('longer line');
  });

  it("C2: rule K's end is already shown as the other end", async () => {
    openClosure({
      ends: [
        { station: A, identity: main3, holds_datum: false },
        { station: B, identity: side17, shown_as: main3, holds_datum: false },
      ],
      allowed: [],
      rule_k: 17,
      rule_k_refused: 'kept_shows_other_end',
      options: [{ renamed_lines: [] }],
    });
    // No end may be kept, so the keep choice is not offered; the outcome
    // still says why rule K's end is not.
    const note = await screen.findByTestId('closure-keep-not-allowed');
    expect(note).toHaveTextContent(
      'Side / 17 cannot be kept here: a loop closure already shows it as Main / 3.',
    );
  });

  it('Keep both names says an end stays shown as another station', async () => {
    openClosure({
      ends: [
        { station: A, identity: main3, holds_datum: false },
        { station: B, identity: side17, shown_as: north9, holds_datum: false },
      ],
      allowed: [17],
      rule_k: 3,
      rule_k_refused: 'other_end_archived',
      options: [
        { keep: 17, hidden: main3, renamed_lines: [MAIN] },
        { renamed_lines: [] },
      ],
    });
    await screen.findByTestId('closure-keep');
    expect(screen.getByTestId('closure-keep-both')).toBeChecked();
    const outcome = screen.getByTestId('closure-keep-outcome');
    expect(outcome).toHaveTextContent(
      'No name changes. Side / 17 stays shown as North / 9 through another loop closure.',
    );
    expect(outcome).not.toHaveTextContent('Each end keeps its own name.');
    // Without an end shown as another, the old sentence is true.
    fireEvent.click(screen.getByTestId('closure-keep-17'));
    fireEvent.click(screen.getByTestId('closure-keep-both'));
    expect(screen.getByTestId('closure-keep-outcome')).not.toHaveTextContent('Each end keeps');
  });

  it('Keep both names with two plain ends: each end keeps its own name', async () => {
    openClosure({
      ends: [
        { station: A, identity: main3, holds_datum: false },
        { station: B, identity: side17, holds_datum: false },
      ],
      allowed: [3, 17],
      rule_k: 3,
      default_keep: 3,
      options: [
        { keep: 3, hidden: side17, renamed_lines: [SIDE] },
        { keep: 17, hidden: main3, renamed_lines: [MAIN] },
        { renamed_lines: [] },
      ],
    });
    await screen.findByTestId('closure-keep');
    fireEvent.click(screen.getByTestId('closure-keep-both'));
    expect(screen.getByTestId('closure-keep-outcome')).toHaveTextContent('Each end keeps its own name.');
    expect(screen.queryByTestId('closure-keep-not-allowed')).toBeNull();
  });
});

describe('Remove Closure of a closure that hides nothing', () => {
  const C = { line_id: SIDE, station_index: 9 };
  function openRemove() {
    const naming: RemoveClosureNaming = { closure_id: 6n, renamed_lines: [], collisions: [] };
    invokeIpc.mockResolvedValue(naming);
    useProjectStore.setState({
      project: {
        lines: [
          { id: MAIN, name: 'Main', shots: [] },
          { id: SIDE, name: 'Side', shots: [] },
        ],
        loop_closures: [
          {
            id: 5n,
            station_a: { line_id: MAIN, station_index: 2 },
            station_b: C,
            archived: {
              end: 'b',
              station: { id: 9, name: '9', home_line: SIDE },
              shown_as: { id: 2, name: '2', home_line: MAIN },
            },
          },
          { id: 6n, station_a: A, station_b: C },
        ],
      },
      stations: [
        { station_ref: A, id: 3, name: '3', merged_with: [] },
        {
          station_ref: C,
          id: 9,
          name: '2',
          merged_with: [],
          shown: { id: 2, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 5n },
        },
      ],
    } as never);
    useUIStore.setState({
      closeDialog: vi.fn(),
      dialogData: { anchorStation: A, closures: [{ closureId: 6n, stationRef: C }] },
    } as never);
    render(<RemoveClosureDialog />);
  }

  it('says the end shown as another station stays so', async () => {
    openRemove();
    const note = await screen.findByTestId('remove-closure-nothing-back');
    expect(note).toHaveTextContent(
      'No name changes. Side / 9 stays shown as Main / 2 through another loop closure.',
    );
    expect(note).not.toHaveTextContent('Both ends already show their own names');
  });
});
