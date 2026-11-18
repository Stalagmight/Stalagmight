/**
 * A plan with an unplaceable pin must not be saved (#310).
 *
 * When the backend can no longer resolve a pin's anchor - the shots on BOTH sides of the station
 * are gone - the store KEEPS the pin and flags it, so an undo can bring the station back. What it
 * must not do is let that plan reach the `.plnx`: a persisted pin the backend cannot anchor is
 * exactly how a saved plan acquires a silently wrong station. The backend's own `to_persisted`
 * refuses it as well, but that refusal arrives as an error string after the diver has named and
 * confirmed the save; blocking here says which plan is unsavable, and why, before that.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('../../../utils/divePlanApi', () => ({
  listDivePlans: vi.fn(async () => []),
  loadDivePlan: vi.fn(),
  saveDivePlan: vi.fn(),
  deleteDivePlan: vi.fn(),
  planAnchorsOf: vi.fn(() => null),
  planAnchorsForRefs: vi.fn(async () => []),
  planRefsForAnchors: vi.fn(async () => []),
}));

import { DivePlanBar } from '../DivePlanBar';
import { useGasPlanStore } from '../../../store/gasPlanStore';
import { useProjectStore } from '../../../store/projectStore';

const savedProject = {
  name: 'Cave',
  created_at: '2026-07-28T00:00:00Z',
  file_path: '/mock/cave.stalx',
} as never;

describe('DivePlanBar refuses to save an unresolved pin (#310)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useGasPlanStore.getState().reset();
    useProjectStore.setState({ project: savedProject, databaseOpen: true });
  });

  it('enables Save as while every pin resolves', () => {
    render(<DivePlanBar />);
    expect(screen.getByTestId('dive-plan-save-as')).toBeEnabled();
    expect(screen.queryByTestId('dive-plan-unresolved-pins')).not.toBeInTheDocument();
  });

  it('disables Save as and explains why once a pin is flagged', () => {
    useGasPlanStore.setState({
      endpoint: { line_id: 1, station_index: 3 },
      unresolved: {
        origin: false,
        endpoint: true,
        waypoints: [],
        markedPoints: [],
        stageDropOverrides: [],
      },
    });
    render(<DivePlanBar />);
    expect(screen.getByTestId('dive-plan-save-as')).toBeDisabled();
    expect(screen.getByTestId('dive-plan-unresolved-pins')).toBeInTheDocument();
  });

  it('blocks a flagged WAY POINT too, not just the endpoint', () => {
    useGasPlanStore.setState({
      waypoints: [{ line_id: 1, station_index: 2 }],
      unresolved: {
        origin: false,
        endpoint: false,
        waypoints: [true],
        markedPoints: [],
        stageDropOverrides: [],
      },
    });
    render(<DivePlanBar />);
    expect(screen.getByTestId('dive-plan-save-as')).toBeDisabled();
  });
});
