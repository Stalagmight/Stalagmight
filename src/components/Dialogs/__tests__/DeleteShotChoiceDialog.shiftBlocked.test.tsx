/**
 * Delete Shot's choice dialog lists what Shift Geometry would change only
 * while Shift Geometry can be chosen: with it blocked, the list described a
 * change the user cannot make (review finding UX-5).
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

const MAIN = 3;
const SIDE = 8;
const ref = (line_id: number, station_index: number) => ({ line_id, station_index });
const impact = (shiftBlocked: boolean) => ({
  removed_merges: [{ station_a: ref(MAIN, 2), station_b: ref(SIDE, 0) }],
  removed_closures: [],
  closure_to_merge: [],
  shift_blocked: shiftBlocked,
  shift_blocked_reasons: shiftBlocked ? ['A datum holds this line.'] : [],
});

beforeEach(() => {
  vi.clearAllMocks();
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main Line', shots: [{ id: 30 }, { id: 31 }, { id: 32 }] },
        { id: SIDE, name: 'Side', shots: [{ id: 80 }] },
      ],
      station_merges: [],
    },
    surveyData: null,
    stations: [],
    isShotFrozen: () => false,
  } as never);
  useUIStore.setState({ dialogData: { lineId: MAIN, shotId: 31, lineName: 'Main Line' } } as never);
});

describe('DeleteShotChoiceDialog', () => {
  it('lists what Shift Geometry would change while it can be chosen', async () => {
    invokeIpc.mockResolvedValue(impact(false));
    render(<DeleteShotChoiceDialog />);
    await waitFor(() =>
      expect(screen.getByText('If you choose Shift Geometry, these relationships will update:')).toBeInTheDocument(),
    );
  });

  it('lists nothing for Shift Geometry when it is blocked, and says why', async () => {
    invokeIpc.mockResolvedValue(impact(true));
    render(<DeleteShotChoiceDialog />);
    await waitFor(() => expect(screen.getByText('A datum holds this line.')).toBeInTheDocument());
    expect(screen.queryByText('If you choose Shift Geometry, these relationships will update:')).toBeNull();
    expect(screen.getByTestId('delete-shot-choice-shift')).toBeDisabled();
  });
});
