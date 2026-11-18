/**
 * The XML export re-checks unresolved pins AFTER the save dialog (#310).
 *
 * The render-time `disabled` gate proves the pins resolved when the button was clicked - but the
 * dialog is not modal everywhere (Android's in-app picker), so an edit behind it can strand a pin
 * between the click and the write. Exporting anyway would route the document through whichever
 * station the stale positional ref now names - the exact bake the Save gate refuses.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/fileDialogs', () => ({
  saveDialog: vi.fn(),
  exportSaveFilters: vi.fn(() => []),
  ensureExtension: vi.fn((p: string) => p),
}));
vi.mock('../../../utils/divePlanApi', () => ({
  exportDiveProfile: vi.fn(async () => undefined),
  renderDiveProfileText: vi.fn(async () => ''),
  planAnchorsForRefs: vi.fn(async () => []),
  planRefsForAnchors: vi.fn(async () => []),
}));

import { ProfileChart } from '../ProfileChart';
import { useGasPlanStore } from '../../../store/gasPlanStore';
import { saveDialog } from '../../../utils/fileDialogs';
import { exportDiveProfile } from '../../../utils/divePlanApi';
import type { GasReachField } from '../../../types/ipc.generated';

/** The smallest field the chart draws: two outbound points spanning real distance. */
const profileField = (): GasReachField => ({
  station_refs: [],
  gas_to_reach: [],
  reachable: [],
  turn_frontier: [],
  max_reachable_gas: 0,
  budget_litres: 0,
  route: [],
  stage_drop_refs: [],
  final_turn_ref: null,
  turn_toward_endpoint: false,
  backgas_turn_bar: null,
  stage_drop_bars: [],
  stage_drop_over_reserve: [],
  stage_drop_distances_m: [],
  cylinder_usable_bars: [],
  route_line_length_m: 100,
  route_max_depth_m: null,
  route_avg_depth_m: null,
  waypoint_gas_litres: [],
  route_profile: [
    {
      route_index: 0,
      station_ref: { line_id: 1, station_index: 0 },
      distance_m: 0,
      depth_m: 0,
      gas_l: 0,
      elapsed_min: 0,
      outbound: true,
    },
    {
      route_index: 1,
      station_ref: { line_id: 1, station_index: 1 },
      distance_m: 100,
      depth_m: 10,
      gas_l: 50,
      elapsed_min: 5,
      outbound: true,
    },
    {
      route_index: 0,
      station_ref: { line_id: 1, station_index: 0 },
      distance_m: 200,
      depth_m: 0,
      gas_l: 80,
      elapsed_min: 10,
      outbound: false,
    },
  ],
  stage_drop_route_indices: [],
  waypoint_route_indices: [],
  scooter_distance_m: 0,
  scooter_range_limit_m: null,
  scooter_drop_ref: null,
  route_runtime_min: null,
  turn_on_scooter_range: false,
});

describe('ProfileChart export re-checks unresolved pins after the save dialog (#310)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useGasPlanStore.getState().reset();
    useGasPlanStore.setState({ reachField: profileField() });
  });

  it('refuses the export when a pin went unresolved behind the dialog', async () => {
    // The dialog "edit": a pin strands while the picker is open, after the button's own
    // render-time gate has already passed.
    vi.mocked(saveDialog).mockImplementation(async () => {
      useGasPlanStore.setState({
        unresolved: {
          origin: false,
          endpoint: true,
          waypoints: [],
          markedPoints: [],
          stageDropOverrides: [],
        },
      });
      return '/tmp/dive.xml';
    });
    render(<ProfileChart />);
    const btn = screen.getByTestId('plan-profile-export-xml');
    expect(btn).toBeEnabled();
    await userEvent.click(btn);
    await waitFor(() => expect(saveDialog).toHaveBeenCalledTimes(1));
    expect(exportDiveProfile).not.toHaveBeenCalled();
  });

  it('exports when the pins still resolve (the control: the harness reaches the write)', async () => {
    vi.mocked(saveDialog).mockResolvedValue('/tmp/dive.xml');
    render(<ProfileChart />);
    await userEvent.click(screen.getByTestId('plan-profile-export-xml'));
    await waitFor(() => expect(exportDiveProfile).toHaveBeenCalledTimes(1));
  });
});
