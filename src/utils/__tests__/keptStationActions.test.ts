/**
 * RS 7, U-P2-7: the one-click actions (the "Also on" detach button, the
 * context menu's single Remove Closure, the Closures section's remove, the
 * warning resolution's remove) open their dialog whenever the preview says
 * something comes back or a name would repeat; otherwise they act at once.
 * Injection: acting without the preview fails every "opens" case.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpc = vi.fn();
vi.mock('../ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { detachLineAsking, detachNeedsDialog, removalNeedsDialog, removeClosureAsking } from '../keptStationActions';
import type { DetachLinePreview } from '../../types/ipc/DetachLinePreview';

const MAIN = { line_id: 1, station_index: 3 };
const SIDE = { line_id: 2, station_index: 0 };
const unmergeStationPair = vi.fn();
const removeLoopClosure = vi.fn();
const openDialog = vi.fn();

const preview = (extra: Partial<DetachLinePreview> = {}): DetachLinePreview => ({
  name: '3',
  station_id: 60,
  restored: false,
  kept: false,
  lines: [],
  together: [],
  rest_choices: [],
  collisions: [],
  ...extra,
});

beforeEach(() => {
  vi.clearAllMocks();
  unmergeStationPair.mockResolvedValue({ success: true });
  removeLoopClosure.mockResolvedValue({ success: true });
  useProjectStore.setState({
    unmergeStationPair,
    removeLoopClosure,
    project: { lines: [], loop_closures: [{ id: 5n, station_a: MAIN, station_b: SIDE }] },
  } as never);
  useUIStore.setState({ openDialog } as never);
});

describe('detachNeedsDialog', () => {
  it('asks when something comes back, comes off together, changes name or collides', () => {
    expect(detachNeedsDialog(preview())).toBe(false);
    expect(detachNeedsDialog(preview({ restored: true }))).toBe(true);
    expect(detachNeedsDialog(preview({ kept: true }))).toBe(true);
    expect(detachNeedsDialog(preview({ together: [4] }))).toBe(true);
    expect(detachNeedsDialog(preview({ shown_before: { id: 8, name: '8' } }))).toBe(true);
    expect(
      detachNeedsDialog(
        preview({
          collisions: [
            {
              line_id: 2,
              name: '3',
              letter: { station: { id: 60, name: '3' }, to: '3a' },
              alternative: { station: { id: 61, name: '3' }, to: '3a' },
            },
          ],
        }),
      ),
    ).toBe(true);
  });
});

describe('detachLineAsking', () => {
  it('detaches at once when nothing comes back', async () => {
    invokeIpc.mockResolvedValue(preview());
    await expect(detachLineAsking(MAIN, SIDE, [SIDE])).resolves.toBe('done');
    expect(invokeIpc).toHaveBeenCalledWith('preview_detach_line', { stationRef: MAIN, lineId: SIDE.line_id }, expect.anything());
    expect(unmergeStationPair).toHaveBeenCalledWith(MAIN, SIDE);
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('opens Detach Line from Station on that line when its own station comes back', async () => {
    invokeIpc.mockResolvedValue(preview({ restored: true }));
    await expect(detachLineAsking(MAIN, SIDE, [SIDE])).resolves.toBe('asked');
    expect(openDialog).toHaveBeenCalledWith('unmerge-stations', {
      stationRef: MAIN,
      mergedStations: [SIDE],
      chosenLine: SIDE.line_id,
    });
    expect(unmergeStationPair).not.toHaveBeenCalled();
  });

  it('detaches at once when the preview fails (the command reports its refusal)', async () => {
    invokeIpc.mockRejectedValue(new Error('refused'));
    await expect(detachLineAsking(MAIN, SIDE, [SIDE])).resolves.toBe('done');
    expect(unmergeStationPair).toHaveBeenCalled();
  });
});

describe('removeClosureAsking', () => {
  it('removes at once when the closure hides nothing and nothing collides', async () => {
    invokeIpc.mockResolvedValue({ closure_id: 5n, renamed_lines: [], collisions: [] });
    await expect(removeClosureAsking(5n, MAIN)).resolves.toBe('done');
    expect(removeLoopClosure).toHaveBeenCalledWith(5n);
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('opens Remove Closure on that closure, named by its other end, when a station comes back', async () => {
    const naming = { closure_id: 5n, comes_back: { id: 17, name: '17' }, renamed_lines: [2], collisions: [] };
    expect(removalNeedsDialog(naming)).toBe(true);
    invokeIpc.mockResolvedValue(naming);
    await expect(removeClosureAsking(5n, MAIN)).resolves.toBe('asked');
    expect(openDialog).toHaveBeenCalledWith('remove-closure', {
      anchorStation: MAIN,
      closures: [{ closureId: 5n, stationRef: SIDE }],
    });
    expect(removeLoopClosure).not.toHaveBeenCalled();
  });

  it('anchors on the first end without a station (the warning resolution)', async () => {
    invokeIpc.mockResolvedValue({ closure_id: 5n, comes_back: { id: 17, name: '17' }, renamed_lines: [], collisions: [] });
    await removeClosureAsking(5n);
    expect(openDialog).toHaveBeenCalledWith('remove-closure', {
      anchorStation: MAIN,
      closures: [{ closureId: 5n, stationRef: SIDE }],
    });
  });

  it('reports a refused removal as failed', async () => {
    invokeIpc.mockResolvedValue({ closure_id: 5n, renamed_lines: [], collisions: [] });
    removeLoopClosure.mockResolvedValue({ success: false, error: 'no' });
    await expect(removeClosureAsking(5n)).resolves.toBe('failed');
  });
});
