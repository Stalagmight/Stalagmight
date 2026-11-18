import { describe, it, expect } from 'vitest';
import { segmentColor, routeLegSegmentIndices } from '../segmentColors';
import type { GasReachField } from '../../types/ipc.generated';
import type { RouteLeg } from '../../types/ipc/RouteLeg';

// Content of `route` and `station_refs` etc. is irrelevant to `routeLegSegmentIndices` -
// it only reads `field.route.length` and `field.waypoint_route_indices` - so every leg is
// this one dummy, repeated, and every other GasReachField property is a neutral default.
const DUMMY_LEG: RouteLeg = {
  from: { line_id: 1, station_index: 0 },
  to: { line_id: 1, station_index: 1 },
  is_jump: false,
};

function makeField(routeLength: number, waypointRouteIndices: (number | null)[]): GasReachField {
  return {
    station_refs: [],
    gas_to_reach: [],
    reachable: [],
    turn_frontier: [],
    max_reachable_gas: 0,
    budget_litres: 0,
    route: Array.from({ length: routeLength }, () => DUMMY_LEG),
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
    waypoint_route_indices: waypointRouteIndices,
    scooter_distance_m: 0,
    scooter_range_limit_m: null,
    scooter_drop_ref: null,
    route_runtime_min: null,
    turn_on_scooter_range: false,
  };
}

describe('routeLegSegmentIndices', () => {
  it('puts every leg in segment 0 when there are no way points', () => {
    // placed = [] (nothing to place) -> filter(<=leg) is always empty -> every leg is 0.
    expect(routeLegSegmentIndices(makeField(3, []))).toEqual([0, 0, 0]);
  });

  it('splits a 4-leg route at one way point sitting at route station 2', () => {
    // placed = [2]. Leg i departs station i, so:
    //   leg0 (departs 0): count(idx<=0) over [2] = 0
    //   leg1 (departs 1): count(idx<=1) over [2] = 0  <- the leg ARRIVING at the way point
    //                                                     stays on the pre-way-point segment
    //   leg2 (departs 2): count(idx<=2) over [2] = 1  <- the leg LEAVING the way point starts
    //                                                     the next segment
    //   leg3 (departs 3): count(idx<=3) over [2] = 1
    expect(routeLegSegmentIndices(makeField(4, [2]))).toEqual([0, 0, 1, 1]);
  });

  it('splits a 4-leg route at two way points, stations 1 and 3', () => {
    // placed = [1, 3].
    //   leg0 (departs 0): count(idx<=0) over [1,3] = 0
    //   leg1 (departs 1): count(idx<=1) over [1,3] = 1        (station 1 placed, station 3 not)
    //   leg2 (departs 2): count(idx<=2) over [1,3] = 1        (still only station 1)
    //   leg3 (departs 3): count(idx<=3) over [1,3] = 2        (both way points now counted)
    expect(routeLegSegmentIndices(makeField(4, [1, 3]))).toEqual([0, 1, 1, 2]);
  });

  it('starts leg 0 already in segment 1 when a way point sits at the origin (station 0)', () => {
    // placed = [0]. Every leg departs at station >= 0, so every leg counts the origin way
    // point: leg0 (departs 0): count(idx<=0) over [0] = 1; leg1 (departs 1): also 1.
    expect(routeLegSegmentIndices(makeField(2, [0]))).toEqual([1, 1]);
  });

  it('stops counting entirely when the FIRST way point is null (stalled at the first pick)', () => {
    // The `for` loop `break`s at the first null, so placed = [] regardless of what follows -
    // the second entry (2) is never reached. Every leg falls back to segment 0.
    expect(routeLegSegmentIndices(makeField(3, [null, 2]))).toEqual([0, 0, 0]);
  });

  it('keeps the placed boundary when a LATER way point is null (stalled after the first pick)', () => {
    // placed = [1] (the loop pushes idx=1, then breaks on the null that follows - the boundary
    // it already placed is kept, nothing after it is counted).
    //   leg0 (departs 0): count(idx<=0) over [1] = 0
    //   leg1 (departs 1): count(idx<=1) over [1] = 1
    //   leg2 (departs 2): count(idx<=2) over [1] = 1  <- stays on the last placed segment
    //   leg3 (departs 3): count(idx<=3) over [1] = 1  <- forever, since nothing more was placed
    expect(routeLegSegmentIndices(makeField(4, [1, null]))).toEqual([0, 1, 1, 1]);
  });
});

describe('segmentColor', () => {
  // Hand-read from the SEGMENT_COLOR_PALETTE literal in segmentColors.ts (6 entries,
  // index 0 = cyan, the pre-#317 route color):
  //   0 '#22d3ee'  1 '#fbbf24'  2 '#a78bfa'  3 '#34d399'  4 '#f472b6'  5 '#fb923c'
  it('returns the palette entry at its index', () => {
    expect(segmentColor(0)).toBe('#22d3ee');
    expect(segmentColor(1)).toBe('#fbbf24');
    expect(segmentColor(5)).toBe('#fb923c');
  });

  it('cycles past the end of the 6-entry palette via modulo', () => {
    expect(segmentColor(6)).toBe('#22d3ee'); // 6 % 6 = 0
    expect(segmentColor(7)).toBe('#fbbf24'); // 7 % 6 = 1
    expect(segmentColor(13)).toBe('#fbbf24'); // 13 % 6 = 1
  });

  it('clamps a negative index to 0', () => {
    expect(segmentColor(-1)).toBe('#22d3ee');
    expect(segmentColor(-6)).toBe('#22d3ee');
  });

  it('clamps a non-integer index to 0', () => {
    expect(segmentColor(1.5)).toBe('#22d3ee');
    expect(segmentColor(NaN)).toBe('#22d3ee');
  });
});
