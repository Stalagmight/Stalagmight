/**
 * RS 6: a Plan mode pin on a loop end (a station a loop closure shows as its
 * kept end) names the loop end by its own station, then what it is shown as:
 * "Side / 4 (loop end, shown as Main / 3)". RS-FX6, P3-10: "Main / 3 (loop
 * end)" read as if Main / 3 were the loop end. A foreign slot keeps its
 * "(Main)" label, and a plain station its plain one.
 *
 * Injections: labelling the pins with `station(ref, true)` (333e24ac5) fails
 * every loop-end case: it reads "Main / 3 (loop end on Side)"; naming the
 * kept station first again fails them too.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('../DivePlanBar', () => ({ DivePlanBar: () => null }));

import '../../../i18n';
import { GasPlanPanel } from '../GasPlanPanel';
import { GasLegend } from '../../Map/GasLegend';
import { useGasPlanStore } from '../../../store/gasPlanStore';
import { useProjectStore } from '../../../store/projectStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import type { GasReachField } from '../../../types/ipc.generated';
import { identityLabels } from '../../../utils/stationIdentity';

const MAIN = 1;
const SIDE = 2;
const slot = (line_id: number, station_index: number) => ({ line_id, station_index });
/** Side / 4 is a loop end shown as Main / 3; Side / 1 is a foreign slot of
 *  Main's station 0; Main / 2 is plain. */
const LOOP_END = slot(SIDE, 4);
const FOREIGN = slot(SIDE, 1);
const PLAIN = slot(MAIN, 2);

const field = (): GasReachField => ({
  station_refs: [PLAIN, LOOP_END],
  gas_to_reach: [0, 10],
  reachable: [true, true],
  turn_frontier: [],
  max_reachable_gas: 10,
  budget_litres: 1000,
  route: [{ from: PLAIN, to: LOOP_END, is_jump: false }],
  stage_drop_refs: [],
  final_turn_ref: LOOP_END,
  turn_toward_endpoint: true,
  backgas_turn_bar: null,
  stage_drop_bars: [],
  stage_drop_over_reserve: [],
  stage_drop_distances_m: [],
  cylinder_usable_bars: [],
  route_line_length_m: 10,
  route_max_depth_m: 0,
  route_avg_depth_m: 0,
  waypoint_gas_litres: [10],
  route_profile: [],
  stage_drop_route_indices: [],
  waypoint_route_indices: [1],
  scooter_distance_m: 0,
  scooter_range_limit_m: null,
  scooter_drop_ref: null,
  route_runtime_min: 1,
  turn_on_scooter_range: false,
});

beforeEach(() => {
  useProjectStore.setState({
    config: structuredClone(defaultConfig) as Config,
    surveyData: null,
    project: {
      lines: [
        { id: MAIN, name: 'Main', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
      ],
      zero_datums: [{ station_ref: slot(MAIN, 0) }],
      loop_closures: [
        {
          id: 5n,
          station_a: slot(MAIN, 3),
          station_b: LOOP_END,
          archived: { end: 'b', station: { id: 24, name: '4', home_line: SIDE }, shown_as: { id: 13, name: '3', home_line: MAIN } },
        },
      ],
    } as never,
    stations: [
      { station_ref: slot(MAIN, 0), id: 10, name: '0', merged_with: [FOREIGN] },
      { station_ref: PLAIN, id: 12, name: '2', merged_with: [] },
      { station_ref: slot(MAIN, 3), id: 13, name: '3', merged_with: [] },
      {
        station_ref: FOREIGN,
        id: 10,
        name: '0',
        merged_with: [slot(MAIN, 0)],
        shown: { id: 10, home_line: MAIN, foreign: true, loop_end: false },
      },
      {
        station_ref: LOOP_END,
        id: 24,
        name: '3',
        merged_with: [],
        shown: { id: 13, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 5n },
      },
    ] as never,
  });
  useGasPlanStore.setState({
    originChosen: true,
    originPickArmed: false,
    origin: LOOP_END,
    endpoint: LOOP_END,
    waypoints: [FOREIGN, LOOP_END],
    markedPoints: [{ station: LOOP_END, label: null }],
    diver: { rmv_l_per_min: 20, label: null },
    cylinders: [],
    gasConfig: structuredClone(defaultConfig.gas),
    reachField: field(),
    stageDropOverrides: [],
    hydrated: true,
  } as never);
});

describe('Plan pins on a loop end (RS 6)', () => {
  it('the pin label names the loop end, then what it is shown as', () => {
    const labels = identityLabels({
      stations: useProjectStore.getState().stations,
      lines: useProjectStore.getState().project?.lines,
      closures: useProjectStore.getState().project?.loop_closures,
    });
    expect(labels.planStation(LOOP_END)).toBe('Side / 4 (loop end, shown as Main / 3)');
    // Without the closure's archive, by the line it is on.
    expect(
      identityLabels({ stations: useProjectStore.getState().stations, lines: useProjectStore.getState().project?.lines }).planStation(
        LOOP_END,
      ),
    ).toBe('Loop end on Side, shown as Main / 3');
    expect(labels.planStation(FOREIGN)).toBe('Side / 0 (Main)');
    expect(labels.planStation(PLAIN)).toBe('Main / 2');
    // Closure texts keep where the loop end is.
    expect(labels.station(LOOP_END, true)).toBe('Main / 3 (loop end on Side)');
  });

  it('the endpoint, way point and marked rows read it', () => {
    render(<GasPlanPanel />);
    expect(screen.getByTestId('gas-endpoint-readout')).toHaveTextContent(/^Side \/ 4 \(loop end, shown as Main \/ 3\)$/);
    expect(screen.getByTestId('gas-waypoint-row-0')).toHaveTextContent('Side / 0 (Main)');
    expect(screen.getByTestId('gas-waypoint-row-1')).toHaveTextContent('Side / 4 (loop end, shown as Main / 3)');
    expect(screen.getByTestId('gas-waypoint-row-1')).not.toHaveTextContent('loop end on');
    expect(screen.getByTestId('gas-marked-row-0')).toHaveTextContent('Side / 4 (loop end, shown as Main / 3)');
    expect(screen.getByTestId('gas-marked-row-0')).not.toHaveTextContent('loop end on');
  });

  it("the map legend's origin reads it", () => {
    render(<GasLegend visible field={field()} units="metric" />);
    expect(screen.getByTestId('gas-legend')).toHaveTextContent('Side / 4 (loop end, shown as Main / 3)');
    expect(screen.getByTestId('gas-legend')).not.toHaveTextContent('loop end on');
  });
});
