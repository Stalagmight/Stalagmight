/**
 * RS 5.2, 7: Create Loop Closure asks what both ends show. "Both ends show:"
 * offers each end that may be kept (its pill) and "Keep both names"; the
 * default is rule K's end, or "Keep both names" when rule K's end may not be
 * kept (C1, C2), next to the other end. For the choice the dialog says which
 * end is hidden (its point stays, without a label; Remove Closure brings it
 * back). It lists no name collision: the hidden end becomes a loop end,
 * which counts for no name uniqueness (RS-FX5). Create sends the keep (none
 * for "Keep both names") and an empty renames list, so a collision the
 * backend found refuses rather than being lettered unseen.
 *
 * Injections: sending `keep` for "Keep both names" fails "keeps both names";
 * defaulting to the first allowed end fails "defaults to keep both names".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { LoopClosureNaming } from '../../../types/ipc/LoopClosureNaming';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import ClosureDialog, { defaultClosureChoice } from '../ClosureDialog';

const MAIN = 1;
const SIDE = 2;
const A = { line_id: MAIN, station_index: 3 };
const B = { line_id: SIDE, station_index: 17 };
const main3 = { id: 3, name: '3', home_line: MAIN };
const side17 = { id: 17, name: '17', home_line: SIDE, comment: 'Arch' };
const createLoopClosure = vi.fn();
const executeBatch = vi.fn();

function naming(extra: Partial<LoopClosureNaming> = {}): LoopClosureNaming {
  return {
    ends: [
      { station: A, identity: main3, holds_datum: false },
      { station: B, identity: side17, holds_datum: false },
    ],
    allowed: [3, 17],
    rule_k: 3,
    default_keep: 3,
    options: [
      { keep: 3, hidden: side17, renamed_lines: [SIDE] },
      { keep: 17, hidden: { id: 3, name: '3', home_line: MAIN }, renamed_lines: [MAIN] },
      { renamed_lines: [] },
    ],
    ...extra,
  };
}

/** `n` without a default: rule K's end may not be kept. */
function noDefault(n: LoopClosureNaming): LoopClosureNaming {
  const copy = { ...n };
  delete copy.default_keep;
  return copy;
}

function open(answer: LoopClosureNaming, extra: Record<string, unknown> = {}) {
  invokeIpc.mockResolvedValue(answer);
  useUIStore.setState({
    activeDialog: 'closure',
    closeDialog: vi.fn(),
    clearMergeStations: vi.fn(),
    dialogData: { stationA: A, stationB: B, ...extra },
  } as never);
  render(<ClosureDialog />);
}

beforeEach(() => {
  // A local project: editing is not blocked (useEditingBlocked).
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  vi.clearAllMocks();
  createLoopClosure.mockResolvedValue({ success: true, warnings: [], line_ids: [] });
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
      { station_ref: A, id: 3, name: '3', merged_with: [] },
      { station_ref: B, id: 17, name: '17', merged_with: [] },
    ],
    createLoopClosure,
    executeBatch,
  } as never);
});

describe('Create Loop Closure: both ends show one station (RS 5.2)', () => {
  it('asks the backend with the strategy and offers each allowed end and Keep both names', async () => {
    open(naming());
    expect(await screen.findByTestId('closure-keep')).toHaveTextContent('Both ends show:');
    expect(invokeIpc).toHaveBeenCalledWith(
      'preview_loop_closure',
      { stationA: A, stationB: B, closureStrategy: 'Batch' },
      expect.anything(),
    );
    expect(screen.getByTestId('closure-keep-3')).toBeChecked();
    expect(screen.getByTestId('closure-keep-17')).not.toBeChecked();
    expect(screen.getByTestId('closure-keep-both')).toHaveAccessibleName('Keep both names');
    expect(screen.getByTestId('closure-hidden')).toHaveTextContent(
      'Side / 17 and its comment are hidden: its point stays on the map, without a label. Remove Closure brings them back.',
    );
    // A loop end repeats no name (RS-FX5): no collision rows.
    expect(screen.queryByTestId('closure-collisions')).toBeNull();
  });

  it('creates keeping the chosen end, with no renames', async () => {
    open(naming());
    await screen.findByTestId('closure-keep');
    fireEvent.click(screen.getByTestId('closure-create'));
    await waitFor(() => expect(createLoopClosure).toHaveBeenCalled());
    expect(createLoopClosure).toHaveBeenCalledWith(A, B, 'Batch', { keep: 3, renames: [] });
  });

  it('keeps both names: no keep, nothing hidden', async () => {
    open(naming());
    await screen.findByTestId('closure-keep');
    fireEvent.click(screen.getByTestId('closure-keep-both'));
    expect(screen.getByTestId('closure-keep-outcome')).toHaveTextContent('Each end keeps its own name.');
    expect(screen.queryByTestId('closure-collisions')).toBeNull();
    fireEvent.click(screen.getByTestId('closure-create'));
    await waitFor(() => expect(createLoopClosure).toHaveBeenCalled());
    expect(createLoopClosure).toHaveBeenCalledWith(A, B, 'Batch', { renames: [] });
  });

  it("defaults to Keep both names when rule K's end may not be kept (U-P2-11)", async () => {
    open(
      noDefault(
        naming({
          allowed: [17],
          rule_k: 3,
          options: [
            { keep: 17, hidden: { id: 3, name: '3', home_line: MAIN }, renamed_lines: [MAIN] },
            { renamed_lines: [] },
          ],
        }),
      ),
    );
    await screen.findByTestId('closure-keep');
    expect(screen.getByTestId('closure-keep-both')).toBeChecked();
    expect(screen.queryByTestId('closure-keep-3')).toBeNull();
    expect(screen.getByTestId('closure-keep-17')).not.toBeChecked();
    expect(screen.getByTestId('closure-keep-not-allowed')).toBeInTheDocument();
  });

  it('carries the keep in a snap-to-merge batch with the Move strategy', async () => {
    const move = { type: 'move_station', station_ref: A, offset: { x: 1, y: 0, z: 0 } };
    open(naming(), { preCommands: [move] });
    await screen.findByTestId('closure-keep');
    expect(invokeIpc).toHaveBeenCalledWith(
      'preview_loop_closure',
      { stationA: A, stationB: B, closureStrategy: 'Move' },
      expect.anything(),
    );
    fireEvent.click(screen.getByTestId('closure-keep-17'));
    fireEvent.click(screen.getByTestId('closure-create'));
    await waitFor(() => expect(executeBatch).toHaveBeenCalled());
    expect(executeBatch).toHaveBeenCalledWith([
      move,
      { type: 'create_loop_closure', station_a: A, station_b: B, closure_strategy: 'Move', keep: 17, renames: [] },
    ]);
  });

  it('does not ask for a refused pair', () => {
    open(naming(), { closureRefusal: 'Refused.' });
    expect(invokeIpc).not.toHaveBeenCalled();
    expect(screen.queryByTestId('closure-keep')).toBeNull();
  });
});

describe('defaultClosureChoice', () => {
  it("is rule K's allowed end, else keep both names", () => {
    expect(defaultClosureChoice(naming())).toBe(3);
    expect(defaultClosureChoice(noDefault(naming()))).toBe('both');
  });
});
