/**
 * Delete Shot + Split on a shot inside a loop makes the loop closure left as
 * the only connection a junction, and the stranded piece moves onto it by the
 * misclosure, as 0.6.0 did. Both dialogs on the way say so, from the
 * backend's dry run in `preview_delete_shot` (review finding UX-5). Each
 * station of a closure is a prose pill (RS-PROSE); the notice reads, word for
 * word, the sentence plain text wrote.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import DeleteShotChoiceDialog from '../DeleteShotChoiceDialog';
import SplitLineDialog from '../SplitLineDialog';
import { accessibleText } from '../../../test/accessibleText';

const MAIN = 3;
const LOOP = 8;
const ref = (line_id: number, station_index: number) => ({ line_id, station_index });
const station = (line_id: number, station_index: number, name: string) => ({
  station_ref: ref(line_id, station_index),
  name,
  coordinates: { x: 0, y: 0, z: 0 },
  merged_with: [],
});
/** The notice as a screen reader reads it, after checking both stations are pills. */
const read = (notice: HTMLElement) => {
  expect([...notice.querySelectorAll('.sh-ppill')].map((el) => el.getAttribute('data-prose-kind'))).toEqual([
    'station',
    'station',
  ]);
  return accessibleText(notice);
};
const impact = (welds: boolean, moved = 2.19) => ({
  removed_station: ref(MAIN, 3),
  removed_merges: [],
  removed_closures: [],
  closure_to_merge: [],
  shift_blocked: true,
  shift_blocked_reasons: ['The two sides are already connected.'],
  split_closure_to_merge: welds ? [{ station_a: ref(LOOP, 3), station_b: ref(MAIN, 5) }] : [],
  split_max_move_m: welds ? moved : 0,
});

beforeEach(() => {
  vi.clearAllMocks();
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main Line', shots: [{ id: 30 }, { id: 31 }, { id: 32 }, { id: 33 }, { id: 34 }] },
        { id: LOOP, name: 'Loop', shots: [{ id: 80 }, { id: 81 }, { id: 82 }] },
      ],
      station_merges: [],
    },
    surveyData: null,
    stations: [station(LOOP, 3, 'L3'), station(MAIN, 5, 'M5')],
    isShotFrozen: () => false,
  } as never);
  useUIStore.setState({ dialogData: { lineId: MAIN, shotId: 32, lineName: 'Main Line' } } as never);
});

describe('Delete Shot + Split weld notice', () => {
  it('the choice dialog says the closure becomes a junction and how far the stations move', async () => {
    invokeIpc.mockResolvedValue(impact(true));
    render(<DeleteShotChoiceDialog />);
    const notice = await screen.findByTestId('split-weld-notice');
    expect(read(notice)).toMatch(
      /^If you choose Split Line, the loop closure Loop \/ L3 - Main Line \/ M5 becomes a junction; the stations past it move by up to 2\.\d+ m\.$/,
    );
  });

  it('the Delete & Split dialog says the same from its own preview', async () => {
    invokeIpc.mockResolvedValue(impact(true));
    render(<SplitLineDialog />);
    const notice = await screen.findByTestId('split-weld-notice');
    expect(read(notice)).toMatch(
      /^The loop closure Loop \/ L3 - Main Line \/ M5 becomes a junction; the stations past it move by up to 2\.\d+ m\.$/,
    );
    expect(invokeIpc).toHaveBeenCalledWith('preview_delete_shot', { lineId: MAIN, shotId: 32 }, expect.anything());
  });

  it('leaves out the move when nothing moves', async () => {
    invokeIpc.mockResolvedValue(impact(true, 0));
    render(<SplitLineDialog />);
    const notice = await screen.findByTestId('split-weld-notice');
    expect(read(notice)).toBe('The loop closure Loop / L3 - Main Line / M5 becomes a junction.');
  });

  it('says nothing when the split welds nothing', async () => {
    invokeIpc.mockResolvedValue(impact(false));
    render(<SplitLineDialog />);
    await waitFor(() => expect(invokeIpc).toHaveBeenCalled());
    expect(screen.queryByTestId('split-weld-notice')).toBeNull();
  });
});
