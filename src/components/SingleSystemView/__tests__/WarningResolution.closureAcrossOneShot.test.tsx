/**
 * #737: CLOSURE_ACROSS_ONE_SHOT is an error (never ignorable). Its resolution
 * removes exactly the closure the warning names - by the id it carries, since
 * two closures can span one shot - and the option is off while editing is
 * blocked (the Closures Computed View). A "Go to" option lands on the
 * closure's station. CLOSURE_ADJUSTMENT_FAILED offers the way to the part of
 * the survey it is about instead of an empty panel.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

import { WarningResolution } from '../WarningResolution';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { ProjectWarning } from '../../../types/commands';
import type { Section, Shot } from '../../../types/project';

const section = { id: 0, name: 'A', shots: [] } as unknown as Section;
const shot = {
  id: 2,
  length: 10,
  azimuth_in: 90,
  azimuth_out: 90,
  depth_in: 0,
  depth_out: 0,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  kind: 'Normal' as const,
} as unknown as Shot;

const acrossOneShot = {
  code: 'CLOSURE_ACROSS_ONE_SHOT',
  message:
    'Shot 0:2 has both ends joined by loop closure 0:1 to 0:2, which would squeeze the shot to zero length, so the Closures Computed View skips that closure.',
  severity: 'error',
  sectionId: 0,
  shotId: 2,
  shotIndex: 1,
  isResolved: false,
  closureId: 9,
} as ProjectWarning;

const closure = (id: number, a: number, b: number) => ({
  id,
  station_a: { section_id: 0, station_index: a },
  station_b: { section_id: 0, station_index: b },
  path_length: 10,
  misclosure_distance: 0,
  error_percentage: 0,
  horizontal_misclosure: 0,
  vertical_misclosure: 0,
  strategy: 'Batch',
  is_aligned: false,
});

function renderResolution(warning: ProjectWarning, onResolved = vi.fn()) {
  render(
    <WarningResolution
      warning={warning}
      section={section}
      shot={shot}
      onExit={vi.fn()}
      onResolved={onResolved}
      onSkip={vi.fn()}
    />
  );
  return onResolved;
}

describe('WarningResolution - closures across one shot (#737)', () => {
  const removeLoopClosure = vi.fn();
  const setSelection = vi.fn();

  beforeEach(() => {
    removeLoopClosure.mockReset();
    removeLoopClosure.mockResolvedValue({ success: true, warnings: [], section_ids: [] });
    setSelection.mockReset();
    useProjectStore.setState({
      removeLoopClosure,
      project: { loop_closures: [closure(4, 1, 2), closure(9, 3, 4)], sections: [] },
    } as never);
    useUIStore.setState({ geometryMode: 'measured', appMode: 'survey', setSelection } as never);
    // A confirmed-local project (the read-only gate fails closed on 'unknown').
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local' } as never);
  });

  it('offers Remove closure (preselected) and Go to its station, never Ignore', () => {
    renderResolution(acrossOneShot);
    const remove = screen.getByTestId('warning-resolution-option-remove-closure');
    expect(remove).toBeEnabled();
    expect(remove).toBeChecked();
    expect(screen.getByText('Remove this loop closure')).toBeInTheDocument();
    // The station of closure 9 (the warning's), not of closure 4 on the same shot.
    expect(screen.getByText('Go to closure station 0:3')).toBeInTheDocument();
    expect(screen.queryByText(/ignore/i)).not.toBeInTheDocument();
  });

  it('removes exactly the closure the warning names', async () => {
    const onResolved = renderResolution(acrossOneShot);
    await userEvent.click(screen.getByTestId('warning-resolution-apply'));
    await waitFor(() => expect(removeLoopClosure).toHaveBeenCalledTimes(1));
    expect(removeLoopClosure).toHaveBeenCalledWith(9n);
    await waitFor(() => expect(onResolved).toHaveBeenCalled());
  });

  it('does not count a refused removal as resolved', async () => {
    removeLoopClosure.mockResolvedValue({ success: false, error: 'no', warnings: [], section_ids: [] });
    const onResolved = renderResolution(acrossOneShot);
    await userEvent.click(screen.getByTestId('warning-resolution-apply'));
    await waitFor(() => expect(removeLoopClosure).toHaveBeenCalled());
    expect(onResolved).not.toHaveBeenCalled();
  });

  it('disables Remove closure while editing is blocked', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' } as never);
    renderResolution(acrossOneShot);
    expect(screen.getByTestId('warning-resolution-option-remove-closure')).toBeDisabled();
    // Navigation still works there, so it is the preselected option.
    expect(screen.getByTestId('warning-resolution-option-go-to-station')).toBeChecked();
  });

  it('goes to the closure station without removing anything', async () => {
    renderResolution(acrossOneShot);
    await userEvent.click(screen.getByTestId('warning-resolution-option-go-to-station'));
    await userEvent.click(screen.getByTestId('warning-resolution-apply'));
    await waitFor(() =>
      expect(setSelection).toHaveBeenCalledWith({
        type: 'station',
        stationRef: { section_id: 0, station_index: 3 },
      })
    );
    expect(removeLoopClosure).not.toHaveBeenCalled();
  });

  it('offers a way to the part a failed solve left measured', () => {
    renderResolution({
      ...acrossOneShot,
      code: 'CLOSURE_ADJUSTMENT_FAILED',
      closureId: undefined,
    } as unknown as ProjectWarning);
    expect(screen.getByTestId('warning-resolution-option-go-to-station')).toBeInTheDocument();
    expect(screen.queryByText(/ignore/i)).not.toBeInTheDocument();
    expect(screen.queryByTestId('warning-resolution-option-remove-closure')).toBeNull();
  });
});
