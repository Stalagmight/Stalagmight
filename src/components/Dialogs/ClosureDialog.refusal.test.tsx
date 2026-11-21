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

import '../../i18n';

import ClosureDialog from './ClosureDialog';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';

const REFUSAL =
  'This closure would join the two ends of one shot (Main Line / 4 to Main Line / 5) and squeeze that shot to zero length. Pick two stations that are not the ends of a single shot.';

// Station names differ from the positional indices, so a positional label
// (`0:1`, `1`) can never pass for the name.
const stations = [
  { station_ref: { line_id: 0, station_index: 1 }, name: '4', id: 41 },
  { station_ref: { line_id: 0, station_index: 2 }, name: '5', id: 42 },
];

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
      stationA: { line_id: 0, station_index: 1 },
      stationB: { line_id: 0, station_index: 2 },
      preview,
      ...dialogData,
    },
  } as never);
}

describe('ClosureDialog - refusal guard (#737)', () => {
  const createLoopClosure = vi.fn();

  beforeEach(() => {
    createLoopClosure.mockReset();
    createLoopClosure.mockResolvedValue({ success: true, warnings: [], line_ids: [] });
    useProjectStore.setState({
      createLoopClosure,
      project: { lines: [{ id: 0, name: 'Main Line', shots: [] }] },
      stations,
      surveyData: null,
    } as never);
  });

  it('shows the backend refusal and disables Create', async () => {
    open({ closureRefusal: REFUSAL, reason: 'Creates survey loop' });
    render(<ClosureDialog />);

    // The merge redirect's "Creates survey loop" would contradict "no loop to
    // close": with a refusal it is not shown.
    expect(screen.queryByText('Creates survey loop')).toBeNull();
    expect(screen.queryByTestId('closure-reason')).toBeNull();

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

  it('names both stations and their line, never the positional handles', () => {
    open({});
    render(<ClosureDialog />);

    const dialog = screen.getByTestId('closure-dialog');
    expect(dialog).toHaveTextContent('Station A:Main Line / 4');
    expect(dialog).toHaveTextContent('Station B:Main Line / 5');
    expect(dialog.textContent).not.toMatch(/\d+:\d+|Line 0/);
  });

  it('keeps Create for a pair that closes a real loop', async () => {
    open({ reason: 'Creates survey loop' });
    render(<ClosureDialog />);

    expect(screen.queryByTestId('closure-refusal')).toBeNull();
    // Without a refusal the redirect reason is shown as before.
    expect(screen.getByTestId('closure-reason')).toHaveTextContent('Creates survey loop');
    const create = screen.getByTestId('closure-create');
    expect(create).toBeEnabled();
    await userEvent.click(create);
    expect(createLoopClosure).toHaveBeenCalledWith(
      { line_id: 0, station_index: 1 },
      { line_id: 0, station_index: 2 },
      'Batch',
    );
  });

  // Demo finding I-9: a user who asked to MERGE got "Create Loop Closure /
  // Creates survey loop" with nothing saying why. Opened by a merge, the
  // dialog says the stations are already connected. Injection: showing the
  // backend's `reason` for a merge too fails this.
  it('says why a merge became a loop closure', () => {
    open({ reason: 'Creates survey loop', fromMerge: true });
    render(<ClosureDialog />);
    expect(screen.getByTestId('closure-reason')).toHaveTextContent(
      'These stations are already connected, so merging them would make a loop. A loop closure is created instead.',
    );
    expect(screen.queryByText('Creates survey loop')).toBeNull();
    expect(screen.getByTestId('closure-create')).toBeEnabled();
  });

  it('still lets a refusal replace the merge explanation', () => {
    open({ reason: 'Creates survey loop', fromMerge: true, closureRefusal: REFUSAL });
    render(<ClosureDialog />);
    expect(screen.queryByTestId('closure-reason')).toBeNull();
    expect(screen.getByTestId('closure-refusal')).toHaveTextContent(REFUSAL);
  });
});
