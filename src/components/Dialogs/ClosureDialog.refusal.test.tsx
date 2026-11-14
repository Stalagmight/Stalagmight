/**
 * Component test - the closure dialog's guard (#737). When the connectivity
 * check says the backend would refuse the pair (a closure across one shot),
 * the dialog shows that reason up front and offers no Create; any other pair
 * keeps Create enabled. The backend stays the
 * authority, so the reason shown is the backend's own text, verbatim.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

import ClosureDialog from './ClosureDialog';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';

const REFUSAL =
  'This closure would tie together the two ends of one shot (0:1 to 0:2), so there is no loop to close. Pick stations joined by at least two separate paths.';

const preview = {
  path_length: 10,
  misclosure_distance: 10,
  error_percentage: 100,
  horizontal_misclosure: 10,
  vertical_misclosure: 0,
};

function open(dialogData: Record<string, unknown>) {
  useUIStore.setState({
    activeDialog: 'closure',
    dialogData: {
      stationA: { section_id: 0, station_index: 1 },
      stationB: { section_id: 0, station_index: 2 },
      preview,
      ...dialogData,
    },
  } as never);
}

describe('ClosureDialog - refusal guard (#737)', () => {
  const createLoopClosure = vi.fn();

  beforeEach(() => {
    createLoopClosure.mockReset();
    createLoopClosure.mockResolvedValue({ success: true, warnings: [], section_ids: [] });
    useProjectStore.setState({ createLoopClosure, project: null } as never);
  });

  it('shows the backend refusal and disables Create', async () => {
    open({ closureRefusal: REFUSAL });
    render(<ClosureDialog />);

    const refusal = screen.getByTestId('closure-refusal');
    expect(refusal).toHaveTextContent(REFUSAL);
    expect(refusal).toHaveAttribute('role', 'alert');
    const create = screen.getByTestId('closure-create');
    expect(create).toBeDisabled();
    expect(create).toHaveAttribute('title', REFUSAL);

    await userEvent.click(create);
    expect(createLoopClosure).not.toHaveBeenCalled();
    // Cancel still works: the dialog is a dead end only for Create.
    expect(screen.getByTestId('closure-cancel')).toBeEnabled();
  });

  it('keeps Create for a pair that closes a real loop', async () => {
    open({});
    render(<ClosureDialog />);

    expect(screen.queryByTestId('closure-refusal')).toBeNull();
    const create = screen.getByTestId('closure-create');
    expect(create).toBeEnabled();
    await userEvent.click(create);
    expect(createLoopClosure).toHaveBeenCalledWith(
      { section_id: 0, station_index: 1 },
      { section_id: 0, station_index: 2 },
      'Batch',
    );
  });
});
