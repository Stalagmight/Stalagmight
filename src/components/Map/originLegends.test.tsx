/**
 * The distance and gas legends name their origin station the way the user
 * knows it - by name with its line (`from Main Line / 17`, SPEC 4.1) - never by
 * the positional `line:index` handle the stores address it with. The station
 * is a prose pill (RS-PROSE) that a screen reader reads as that plain label.
 */
import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { DistanceLegend } from './DistanceLegend';
import { GasLegend } from './GasLegend';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useGasPlanStore } from '../../store/gasPlanStore';
import type { GasReachField } from '../../types/ipc.generated';
import { accessibleText } from '../../test/accessibleText';

/** The origin line's prose pills (their kinds) and its accessible text. */
function origin(testId: string) {
  const el = screen.getByTestId(testId);
  return {
    text: accessibleText(el),
    pills: [...el.querySelectorAll('.sh-ppill')].map((pill) => pill.getAttribute('data-prose-kind')),
    drawing: el.querySelector('.sh-ppill [aria-hidden="true"]')?.textContent ?? null,
  };
}

/** Line 3's station at position 2 is named `17`, so a positional label would read `3:2`. */
const ORIGIN = { line_id: 3, station_index: 2 };

function gasField(): GasReachField {
  return {
    station_refs: [],
    gas_to_reach: [],
    reachable: [],
    turn_frontier: [],
    max_reachable_gas: 0,
    budget_litres: 1000,
    route: [],
    stage_drop_refs: [],
    final_turn_ref: null,
    turn_toward_endpoint: false,
    backgas_turn_bar: null,
    stage_drop_bars: [],
    stage_drop_over_reserve: [],
    stage_drop_distances_m: [],
    cylinder_usable_bars: [],
    route_line_length_m: 0,
    route_max_depth_m: null,
    route_avg_depth_m: null,
    waypoint_gas_litres: [],
    route_profile: [],
    stage_drop_route_indices: [],
    waypoint_route_indices: [],
    scooter_distance_m: 0,
    scooter_range_limit_m: null,
    scooter_drop_ref: null,
    route_runtime_min: null,
    turn_on_scooter_range: false,
  };
}

beforeEach(() => {
  useProjectStore.setState({
    surveyData: null,
    stations: [{ station_ref: ORIGIN, name: '17', id: 204 }],
    project: { lines: [{ id: 3, name: 'Main Line' }] },
  } as never);
  useGasPlanStore.getState().reset();
  useUIStore.setState({ distanceGradientOrigin: null } as never);
});

describe('legend origin labels', () => {
  it('the distance legend reads "from Main Line / 17" for a picked station', () => {
    const { rerender } = render(<DistanceLegend max={120} visible />);
    expect(screen.getByTestId('distance-legend-origin')).toHaveTextContent('from entrance');

    useUIStore.setState({ distanceGradientOrigin: ORIGIN } as never);
    rerender(<DistanceLegend max={120} visible />);
    expect(origin('distance-legend-origin')).toEqual({
      text: 'from Main Line / 17',
      pills: ['station'],
      drawing: 'Main Line: 17#204',
    });
    expect(screen.getByTestId('distance-legend-origin')).not.toHaveTextContent('3:2');
  });

  it('the gas legend reads "from Main Line / 17" for a picked origin', () => {
    useGasPlanStore.setState({ origin: ORIGIN } as never);
    render(<GasLegend field={gasField()} units="metric" visible />);
    expect(origin('gas-legend-origin')).toEqual({
      text: 'from Main Line / 17',
      pills: ['station'],
      drawing: 'Main Line: 17#204',
    });
    expect(screen.getByTestId('gas-legend-origin')).not.toHaveTextContent('3:2');
  });
});
