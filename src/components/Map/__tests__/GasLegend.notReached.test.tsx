/**
 * #576 RV9b-10: the gas legend keys the grey "not reached" stage drop (RV8c-2-D3) beside the
 * orange one, and only while the map draws one. RV10b-1: the orange key likewise only while the
 * map draws an orange (reached) drop.
 */
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';

// The real English strings, so the key reads what a user sees.
import '../../../i18n';
import { GasLegend } from '../GasLegend';
import { stationRef } from '../../../test/factories/survey';
import type { GasReachField } from '../../../types/ipc.generated';

/** Route 1:0 -> 1:1 -> 1:2; one drop at 1:1 and, with `pastTurn`, a second pinned at 1:4, past
 *  the turnaround (route index 4 beyond the two legs) - the engine's not-reached answer. */
function field(pastTurn: boolean): GasReachField {
  return twoDropField(pastTurn);
}

/** The orange (reached) and grey (not reached) drop fills the map draws (useGasPlanDropMarkers). */
const ORANGE = '#e08a1e';
const GREY = '#8a8f98';

function legendFills(): (string | null)[] {
  const legend = screen.getByTestId('gas-legend');
  return Array.from(legend.querySelectorAll('path')).map((p) => p.getAttribute('fill'));
}

/** A one-stage plan whose only drop is pinned at 1:4, past the turnaround: the map draws one grey
 *  drop and no orange one. */
function onlyNotReached(): GasReachField {
  return {
    ...twoDropField(true),
    stage_drop_refs: [stationRef(1, 4)],
    stage_drop_bars: [120],
    stage_drop_over_reserve: [false],
    stage_drop_distances_m: [40],
    stage_drop_route_indices: [4],
  };
}

function twoDropField(pastTurn: boolean): GasReachField {
  const leg = (from: number, to: number) => ({
    from: stationRef(1, from),
    to: stationRef(1, to),
    is_jump: false,
  });
  return {
    station_refs: [0, 1, 2, 3, 4].map((i) => stationRef(1, i)),
    gas_to_reach: [0, 100, 200, 300, 400],
    reachable: [true, true, true, false, false],
    turn_frontier: [],
    max_reachable_gas: 200,
    budget_litres: 250,
    route: [leg(0, 1), leg(1, 2)],
    stage_drop_refs: pastTurn ? [stationRef(1, 1), stationRef(1, 4)] : [stationRef(1, 1)],
    final_turn_ref: stationRef(1, 2),
    turn_toward_endpoint: false,
    backgas_turn_bar: 150,
    stage_drop_bars: pastTurn ? [140, 120] : [140],
    stage_drop_over_reserve: pastTurn ? [false, false] : [false],
    stage_drop_distances_m: pastTurn ? [10, 40] : [10],
    cylinder_usable_bars: [],
    route_line_length_m: 20,
    route_max_depth_m: 0,
    route_avg_depth_m: 0,
    waypoint_gas_litres: [],
    route_profile: [],
    stage_drop_route_indices: pastTurn ? [1, 4] : [1],
    waypoint_route_indices: [],
    scooter_distance_m: 0,
    scooter_range_limit_m: null,
    scooter_drop_ref: null,
    route_runtime_min: 2,
    turn_on_scooter_range: false,
  };
}

describe('GasLegend: the not-reached stage drop (#576 RV9b-10)', () => {
  it('keys the grey drop when the map draws one', () => {
    render(<GasLegend field={field(true)} units="metric" visible />);
    const legend = screen.getByTestId('gas-legend');
    expect(legend).toHaveTextContent('Stage drop not reached');
    expect(legend).toHaveTextContent('Stage drop');
    const fills = Array.from(legend.querySelectorAll('path')).map((p) => p.getAttribute('fill'));
    expect(fills).toContain('#8a8f98');
  });

  it('leaves the key out when every drop is reached', () => {
    render(<GasLegend field={field(false)} units="metric" visible />);
    const legend = screen.getByTestId('gas-legend');
    expect(legend).not.toHaveTextContent('not reached');
    expect(legend).toHaveTextContent('Stage drop');
    expect(legendFills()).toContain(ORANGE);
  });

  it('leaves the orange key out when no drop is reached (RV10b-1)', () => {
    render(<GasLegend field={onlyNotReached()} units="metric" visible />);
    const legend = screen.getByTestId('gas-legend');
    expect(legend).toHaveTextContent('Stage drop not reached');
    const fills = legendFills();
    expect(fills).toContain(GREY);
    expect(fills).not.toContain(ORANGE);
    // Only the grey row's label carries the words: no bare "Stage drop" row beside it.
    expect(legend.textContent?.match(/Stage drop/g)).toHaveLength(1);
  });
});
