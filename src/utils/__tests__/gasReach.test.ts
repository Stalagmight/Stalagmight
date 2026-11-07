import { describe, it, expect } from 'vitest';
import {
  buildGasReachFeatureState,
  gasReachColorExpression,
  gasBeyondReachFeatureIds,
  reachableStationKeySet,
  nearestOriginShotEndpoint,
  stageDropReached,
  UNREACHABLE_GAS,
  BEYOND_GAS_RANGE,
} from '../gasReach';
import { encodeFeatureId } from '../ids';
import type { SurveyData } from '../../types/survey-data';
import type { GasReachField } from '../../types/ipc.generated';
import type { StationRef } from '../../types/project';
import { stationRef } from '../../test/factories/survey';

describe('buildGasReachFeatureState', () => {
  // Section 1: shots id10 (st0→st1), id11 (st1→st2). Section 2: shot id20 (st0→st1).
  // SoA shape: section metadata (shotOffset/shotCount) + a flat shotIds array.
  const data = {
    sections: [
      { id: 1, shotOffset: 0, shotCount: 2 },
      { id: 2, shotOffset: 2, shotCount: 1 },
    ],
    shotIds: new Uint32Array([10, 11, 20]),
  } as unknown as Pick<SurveyData, 'sections' | 'shotIds'>;

  // (1,2) is finite-but-OVER-budget (gas_to_reach 1500 > budget 1000, reachable false)
  // - the adversarial case. (2,1) is disconnected (null). Budget is 1000.
  const field: GasReachField = {
    station_refs: [
      { section_id: 1, station_index: 0 },
      { section_id: 1, station_index: 1 },
      { section_id: 1, station_index: 2 },
      { section_id: 2, station_index: 0 },
      { section_id: 2, station_index: 1 },
    ],
    gas_to_reach: [0, 300, 1500, 200, null],
    reachable: [true, true, false, true, false],
    turn_frontier: [{ section_id: 1, station_index: 1 }],
    max_reachable_gas: 300,
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
    // #317: no scooter on this fixture - the overlay it exercises is gas-only.
    scooter_distance_m: 0,
    scooter_range_limit_m: null,
    scooter_drop_ref: null,
    route_runtime_min: null,
    turn_on_scooter_range: false,
  };

  it('maps each reachable station to its litres-to-reach, keyed by encodeFeatureId', () => {
    const { stations } = buildGasReachFeatureState(data, field);
    expect(stations.get(encodeFeatureId(1, 0))).toBe(0);
    expect(stations.get(encodeFeatureId(1, 1))).toBe(300);
    expect(stations.get(encodeFeatureId(2, 0))).toBe(200);
  });

  it('marks a finite-but-OVER-budget station BEYOND_GAS_RANGE (connected but past budget, not grey)', () => {
    // gas_to_reach is Some(1500) but reachable=false; it must NOT leak the 1500
    // value, AND must be distinguishable from a topologically unconnected station.
    const { stations } = buildGasReachFeatureState(data, field);
    expect(stations.get(encodeFeatureId(1, 2))).toBe(BEYOND_GAS_RANGE);
  });

  it('greys a disconnected station (null gas_to_reach)', () => {
    const { stations } = buildGasReachFeatureState(data, field);
    expect(stations.get(encodeFeatureId(2, 1))).toBe(UNREACHABLE_GAS);
  });

  it('colors a shot by the average of its two reachable endpoints, keyed by shot id', () => {
    const { shots } = buildGasReachFeatureState(data, field);
    // shot id10: endpoints st0=0, st1=300 → avg 150
    expect(shots.get(encodeFeatureId(1, 10))).toBe(150);
  });

  it('takes the WORSE class when a shot spans a beyond-range and an unconnected endpoint', () => {
    const { shots } = buildGasReachFeatureState(data, field);
    // id11: st1=300 reachable, st2 beyond-range → beyond-range (not grey)
    expect(shots.get(encodeFeatureId(1, 11))).toBe(BEYOND_GAS_RANGE);
    // id20: st0=200 reachable, st1 disconnected → unconnected dominates
    expect(shots.get(encodeFeatureId(2, 20))).toBe(UNREACHABLE_GAS);
  });

  it('gasBeyondReachFeatureIds picks out only the beyond-range features', () => {
    const { stations, shots } = buildGasReachFeatureState(data, field);
    expect(gasBeyondReachFeatureIds(stations)).toEqual(new Set([encodeFeatureId(1, 2)]));
    expect(gasBeyondReachFeatureIds(shots)).toEqual(new Set([encodeFeatureId(1, 11)]));
    expect(gasBeyondReachFeatureIds(undefined).size).toBe(0);
  });

  it('uses the turn budget (not max_reachable_gas) as the interpolate scale max', () => {
    // The scale must end at the budget so purple always means "at your turn pressure",
    // even when the whole reachable cave costs less than the budget.
    expect(buildGasReachFeatureState(data, field).max).toBe(1000);
  });
});

describe('gasReachColorExpression', () => {
  it('greys unconnected gasReach and interpolates near→far over 0→budget', () => {
    const s = JSON.stringify(gasReachColorExpression('#aaaaaa', '#bbbbbb', 1000, '#888888'));
    expect(s).toContain('interpolate');
    expect(s).toContain('coalesce');
    expect(s).toContain('gasReach'); // reads the dedicated feature-state key
    expect(s).toContain('#888888'); // grey branch for the unconnected sentinel
  });

  it('clamps the beyond-range sentinel to the ramp far color, matched BEFORE the generic grey case', () => {
    // Structural, not string-substring: pins the actual ['case', ...] branch
    // shape and ORDER, so reordering the branches (grey first) fails this test
    // even though it would still contain both substrings.
    const expr = gasReachColorExpression('#aaaaaa', '#bbbbbb', 1000, '#888888');
    // expr = ['let', 'g', ['coalesce', ...], caseExpr] - the case lives at index 3.
    const caseExpr = expr[3] as unknown[];
    expect(caseExpr[0]).toBe('case');
    // First branch: BEYOND_GAS_RANGE -> the ramp's far color, clamped.
    expect(caseExpr[1]).toEqual(['==', ['var', 'g'], BEYOND_GAS_RANGE]);
    expect(caseExpr[2]).toBe('#bbbbbb');
    // Second branch: any other negative value (the unconnected sentinel) -> grey.
    expect(caseExpr[3]).toEqual(['<', ['var', 'g'], 0]);
    expect(caseExpr[4]).toBe('#888888');
  });

  it('guards a non-positive budget so the interpolate stops never collide', () => {
    const s = JSON.stringify(gasReachColorExpression('#aaaaaa', '#bbbbbb', 0, '#888888'));
    // safeMax falls back to 1 (no throw, valid expression).
    expect(s).toContain('interpolate');
  });
});

describe('reachableStationKeySet', () => {
  const field = {
    station_refs: [
      { section_id: 1, station_index: 0 },
      { section_id: 1, station_index: 1 },
      { section_id: 1, station_index: 2 },
      { section_id: 2, station_index: 0 },
    ],
    reachable: [true, false, true, true],
    // remaining GasReachField fields are unused by this helper
  } as unknown as GasReachField;

  it('keys only the reachable stations, by section_id:station_index', () => {
    const keys = reachableStationKeySet(field);
    expect(keys).toEqual(new Set(['1:0', '1:2', '2:0']));
    expect(keys.has('1:1')).toBe(false); // unreachable is excluded
  });

  it('returns an empty set when nothing is reachable', () => {
    const none = { ...field, reachable: [false, false, false, false] } as unknown as GasReachField;
    expect(reachableStationKeySet(none).size).toBe(0);
  });
});

describe('nearestOriginShotEndpoint (nearest to the CURRENT origin, not always the entrance)', () => {
  const a: StationRef = stationRef(1, 0);
  const b: StationRef = stationRef(1, 1);

  const fieldWith = (aGas: number | null, bGas: number | null): GasReachField =>
    ({
      station_refs: [a, b],
      gas_to_reach: [aGas, bGas],
      reachable: [aGas != null, bGas != null],
    }) as unknown as GasReachField;

  it('picks the smaller gas_to_reach when a field is loaded', () => {
    expect(nearestOriginShotEndpoint(a, b, fieldWith(50, 200), [])).toBe(a);
    expect(nearestOriginShotEndpoint(a, b, fieldWith(200, 50), [])).toBe(b);
  });

  it('treats null gas_to_reach as infinitely far', () => {
    expect(nearestOriginShotEndpoint(a, b, fieldWith(null, 50), [])).toBe(b);
    expect(nearestOriginShotEndpoint(a, b, fieldWith(50, null), [])).toBe(a);
  });

  it('re-armed pick-on-map: with a field loaded, ranks against the OUTGOING origin, not the ' +
    'entrance - even when that disagrees with straight-line distance to the nearest zero datum', () => {
    const stations = [
      { station_ref: a, coordinates: { x: 0, y: 0, z: 0 }, is_zero_datum: false },
      { station_ref: b, coordinates: { x: 10, y: 0, z: 0 }, is_zero_datum: false },
      {
        station_ref: { section_id: 9, station_index: 0 },
        coordinates: { x: 10, y: 0, z: 0 }, // right next to b: b is the entrance-nearest endpoint
        is_zero_datum: true,
      },
    ];
    // A loaded field rooted at the diver's CURRENT (outgoing) origin, not the entrance: a costs
    // less gas to reach from there even though b sits right on top of the zero datum. The field
    // wins outright - this is the whole point of the re-pick regime.
    expect(nearestOriginShotEndpoint(a, b, fieldWith(10, 200), stations)).toBe(a);
  });

  it('falls through to the euclidean rule when both endpoints are null', () => {
    const stations = [
      { station_ref: a, coordinates: { x: 0, y: 0, z: 0 }, is_zero_datum: false },
      { station_ref: b, coordinates: { x: 10, y: 0, z: 0 }, is_zero_datum: false },
      {
        station_ref: { section_id: 9, station_index: 0 },
        coordinates: { x: 1, y: 0, z: 0 },
        is_zero_datum: true,
      },
    ];
    // a is 1m from the zero datum, b is 9m - a wins even though the field claims neither is
    // reachable.
    expect(nearestOriginShotEndpoint(a, b, fieldWith(null, null), stations)).toBe(a);
  });

  it('with no field loaded, picks the endpoint nearest a zero-datum station', () => {
    const stations = [
      { station_ref: a, coordinates: { x: 0, y: 0, z: 0 }, is_zero_datum: false },
      { station_ref: b, coordinates: { x: 10, y: 0, z: 0 }, is_zero_datum: false },
      {
        station_ref: { section_id: 9, station_index: 0 },
        coordinates: { x: 9, y: 0, z: 0 },
        is_zero_datum: true,
      },
    ];
    expect(nearestOriginShotEndpoint(a, b, null, stations)).toBe(b);
  });

  it('with no zero datum on the project, deterministically keeps the first endpoint', () => {
    const stations = [
      { station_ref: a, coordinates: { x: 0, y: 0, z: 0 }, is_zero_datum: false },
      { station_ref: b, coordinates: { x: 10, y: 0, z: 0 }, is_zero_datum: false },
    ];
    expect(nearestOriginShotEndpoint(a, b, null, stations)).toBe(a);
    expect(nearestOriginShotEndpoint(b, a, null, stations)).toBe(b);
  });
});

describe('stageDropReached (#576 RV8c-2-D3)', () => {
  // A route of two legs: stations 0, 1, 2. The engine reports a pinned drop the dive turns short
  // of at its pin past the turnaround, i.e. with a route index beyond 2.
  const leg = (from: number, to: number) => ({
    from: stationRef(1, from),
    to: stationRef(1, to),
    is_jump: false,
  });
  const field = (indices: number[]) =>
    ({
      route: [leg(0, 1), leg(1, 2)],
      stage_drop_route_indices: indices,
    }) as unknown as GasReachField;

  it('reads a drop on the route, the turnaround included, as reached', () => {
    expect(stageDropReached(field([0, 2]), 0)).toBe(true);
    expect(stageDropReached(field([0, 2]), 1)).toBe(true);
  });

  it('reads a drop past the turnaround as not reached', () => {
    expect(stageDropReached(field([1, 3]), 1)).toBe(false);
    expect(stageDropReached(field([5]), 0)).toBe(false);
  });

  it('treats a drop the field carries no index for as reached (an older or partial field)', () => {
    expect(stageDropReached(field([]), 0)).toBe(true);
  });
});
