/**
 * Remove Closure waits for its naming preview (review finding ui-7): while
 * `preview_remove_loop_closure` is out for the closure chosen (on opening,
 * and after picking another closure), Remove is disabled, so it never sends
 * no renames for collisions the dialog is about to show. A preview that
 * fails leaves Remove enabled, without renames (rule L letters, with a
 * notice), as the RS-UI deviation allows.
 *
 * Injection (333e24ac5): Remove was enabled with `naming === null` while the
 * preview was in flight, so both "waits" cases fail.
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
const C = { line_id: SIDE, station_index: 9 };
const removeLoopClosure = vi.fn();
const closeDialog = vi.fn();

const naming = (closure_id: bigint): RemoveClosureNaming => ({
  closure_id,
  archived_slot: B,
  comes_back: { id: 17, name: '17', home_line: SIDE },
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
});

/** Every preview call waits here until the test answers it. */
const pending: Array<{ id: bigint; resolve: (n: RemoveClosureNaming) => void; reject: (e: Error) => void }> = [];

beforeEach(() => {
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  vi.clearAllMocks();
  pending.length = 0;
  removeLoopClosure.mockResolvedValue({ success: true });
  invokeIpc.mockImplementation(
    (_cmd: string, args: { closureId: number }) =>
      new Promise<RemoveClosureNaming>((resolve, reject) => pending.push({ id: BigInt(args.closureId), resolve, reject })),
  );
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
      ],
      loop_closures: [
        { id: 5n, station_a: A, station_b: B },
        { id: 6n, station_a: A, station_b: C },
      ],
    },
    surveyData: null,
    stations: [
      { station_ref: A, id: 3, name: '3', merged_with: [] },
      { station_ref: B, id: 17, name: '3', merged_with: [] },
      { station_ref: C, id: 9, name: '9', merged_with: [] },
    ],
    removeLoopClosure,
  } as never);
  useUIStore.setState({
    closeDialog,
    dialogData: {
      anchorStation: A,
      closures: [
        { closureId: 5n, stationRef: B },
        { closureId: 6n, stationRef: C },
      ],
    },
  } as never);
});

describe('Remove Closure waits for its preview', () => {
  it('on opening: Remove is disabled until the preview answers, then sends its answers', async () => {
    render(<RemoveClosureDialog />);
    await waitFor(() => expect(pending).toHaveLength(1));
    expect(screen.getByTestId('remove-closure-confirm')).toBeDisabled();
    pending[0]!.resolve(naming(5n));
    await screen.findByTestId('remove-closure-collision-0');
    expect(screen.getByTestId('remove-closure-confirm')).not.toBeDisabled();
    fireEvent.click(screen.getByTestId('remove-closure-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(removeLoopClosure).toHaveBeenCalledWith(5n, { renames: [{ station_id: 17, name: '17a' }] });
  });

  it('after picking another closure: Remove waits for that closure\'s preview', async () => {
    render(<RemoveClosureDialog />);
    await waitFor(() => expect(pending).toHaveLength(1));
    pending[0]!.resolve(naming(5n));
    await screen.findByTestId('remove-closure-collision-0');
    fireEvent.click(screen.getByTestId('remove-closure-option-6'));
    await waitFor(() => expect(pending).toHaveLength(2));
    expect(screen.getByTestId('remove-closure-confirm')).toBeDisabled();
    pending[1]!.resolve(naming(6n));
    await waitFor(() => expect(screen.getByTestId('remove-closure-confirm')).not.toBeDisabled());
  });

  it('a failed preview leaves Remove enabled, without renames', async () => {
    render(<RemoveClosureDialog />);
    await waitFor(() => expect(pending).toHaveLength(1));
    pending[0]!.reject(new Error('gone'));
    await waitFor(() => expect(screen.getByTestId('remove-closure-confirm')).not.toBeDisabled());
    fireEvent.click(screen.getByTestId('remove-closure-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(removeLoopClosure).toHaveBeenCalledWith(5n);
  });
});
