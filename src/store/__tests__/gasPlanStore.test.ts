import { describe, it, expect, beforeEach, vi } from 'vitest';

// The store takes backend anchors for its pins on a debounce (#310). Nothing here exercises that
// path, and an un-mocked capture would reach for a Tauri bridge that does not exist under vitest.
vi.mock('../../utils/divePlanApi', () => ({
  planAnchorsForRefs: vi.fn(async () => []),
  planRefsForAnchors: vi.fn(async () => []),
}));

import { useGasPlanStore, selectIsPlanDirty, selectHasPins } from '../gasPlanStore';
import { divePlanSnapshotOf } from '../../utils/divePlan';
import { defaultConfig } from '../../types/config';
import type { GasConfig, GasReachField } from '../../types/ipc.generated';

const fakeField = (): GasReachField => ({
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
});

describe('gasPlanStore', () => {
  beforeEach(() => {
    useGasPlanStore.getState().reset();
  });

  it('seeds gasConfig from defaultConfig.gas and a twin-AL80 back-gas default rig', () => {
    const s = useGasPlanStore.getState();
    expect(s.origin).toBeNull();
    expect(s.diver.rmv_l_per_min).toBe(20);
    expect(s.cylinders).toHaveLength(2);
    expect(s.cylinders.every((c) => c.role === 'back_gas')).toBe(true);
    expect(s.gasConfig.default_reserve).toBe(defaultConfig.gas.default_reserve);
    expect(s.gasConfig.units).toBe(defaultConfig.gas.units);
  });

  it('does not share the gasConfig object reference with defaultConfig (no mutation bleed)', () => {
    expect(useGasPlanStore.getState().gasConfig).not.toBe(defaultConfig.gas);
  });

  it.each([
    ['setOrigin', () => useGasPlanStore.getState().setOrigin({ section_id: 1, station_index: 0 })],
    ['setEndpoint', () => useGasPlanStore.getState().setEndpoint({ section_id: 1, station_index: 2 })],
    ['setDiver', () => useGasPlanStore.getState().setDiver({ rmv_l_per_min: 18, label: null })],
    ['setCylinders', () => useGasPlanStore.getState().setCylinders([])],
    ['setGasConfig', () => useGasPlanStore.getState().setGasConfig({ ...defaultConfig.gas })],
  ])('%s nulls the cached reachField (no stale overlay under changed inputs)', (_name, mutate) => {
    useGasPlanStore.setState({ reachField: fakeField() });
    expect(useGasPlanStore.getState().reachField).not.toBeNull();
    mutate();
    expect(useGasPlanStore.getState().reachField).toBeNull();
  });

  it('setReachField stores the field; reset clears it and restores defaults', () => {
    useGasPlanStore.getState().setOrigin({ section_id: 3, station_index: 2 });
    useGasPlanStore.getState().setReachField(fakeField());
    expect(useGasPlanStore.getState().reachField).not.toBeNull();
    useGasPlanStore.getState().reset();
    const s = useGasPlanStore.getState();
    expect(s.reachField).toBeNull();
    expect(s.origin).toBeNull();
    expect(s.cylinders).toHaveLength(2);
  });

  it('endpoint defaults null and is cleared by reset (no leak across plan sessions)', () => {
    expect(useGasPlanStore.getState().endpoint).toBeNull();
    useGasPlanStore.getState().setEndpoint({ section_id: 2, station_index: 4 });
    expect(useGasPlanStore.getState().endpoint).toEqual({ section_id: 2, station_index: 4 });
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().endpoint).toBeNull();
  });

  it('pushyAck defaults false and is cleared by reset (re-prompt each Plan session)', () => {
    expect(useGasPlanStore.getState().pushyAck).toBe(false);
    useGasPlanStore.getState().setPushyAck(true);
    expect(useGasPlanStore.getState().pushyAck).toBe(true);
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().pushyAck).toBe(false);
  });

  it('setStageDropOverride pins a drop by index and pads sparse slots with null (#246)', () => {
    const ref = { section_id: 1, station_index: 3 };
    useGasPlanStore.getState().setStageDropOverride(1, ref);
    // Index 0 was never set, so it pads with null; index 1 holds the pinned station.
    expect(useGasPlanStore.getState().stageDropOverrides).toEqual([null, ref]);
    // A negative index is a no-op (never grows the array).
    useGasPlanStore.getState().setStageDropOverride(-1, ref);
    expect(useGasPlanStore.getState().stageDropOverrides).toEqual([null, ref]);
    // Passing null resets that drop back to the strategy default.
    useGasPlanStore.getState().setStageDropOverride(1, null);
    expect(useGasPlanStore.getState().stageDropOverrides).toEqual([null, null]);
  });

  it('setStageDropOverride does NOT null reachField (markers stay put during refetch) (#246)', () => {
    useGasPlanStore.getState().setReachField(fakeField());
    useGasPlanStore.getState().setStageDropOverride(0, { section_id: 1, station_index: 2 });
    expect(useGasPlanStore.getState().reachField).not.toBeNull();
  });

  it.each([
    ['setOrigin', () => useGasPlanStore.getState().setOrigin({ section_id: 1, station_index: 0 })],
    ['setEndpoint', () => useGasPlanStore.getState().setEndpoint({ section_id: 1, station_index: 5 })],
    ['setDiver', () => useGasPlanStore.getState().setDiver({ rmv_l_per_min: 18, label: null })],
    ['setCylinders', () => useGasPlanStore.getState().setCylinders([])],
    ['setGasConfig', () => useGasPlanStore.getState().setGasConfig({ ...defaultConfig.gas })],
  ])('%s clears stageDropOverrides (a reshuffled rig invalidates pinned drops) (#246)', (_n, mutate) => {
    useGasPlanStore.getState().setStageDropOverride(0, { section_id: 1, station_index: 2 });
    expect(useGasPlanStore.getState().stageDropOverrides).toHaveLength(1);
    mutate();
    expect(useGasPlanStore.getState().stageDropOverrides).toEqual([]);
  });

  it('reset clears stageDropOverrides (no leak across plan sessions) (#246)', () => {
    useGasPlanStore.getState().setStageDropOverride(0, { section_id: 1, station_index: 2 });
    expect(useGasPlanStore.getState().stageDropOverrides).toHaveLength(1);
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().stageDropOverrides).toEqual([]);
  });
});

describe('gasPlanStore way points (#298)', () => {
  const a = { section_id: 1, station_index: 1 };
  const b = { section_id: 1, station_index: 5 };

  beforeEach(() => {
    useGasPlanStore.getState().reset();
  });

  it('adds in click order and removes by index', () => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().addWaypoint(b);
    expect(useGasPlanStore.getState().waypoints).toEqual([a, b]);
    useGasPlanStore.getState().removeWaypoint(0);
    expect(useGasPlanStore.getState().waypoints).toEqual([b]);
  });

  it('re-adding the same station is a no-op (a duplicate way point means nothing)', () => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().addWaypoint(b);
    useGasPlanStore.getState().addWaypoint(a);
    // Right-clicking an existing pin offers "Set as way point" again, so this is one gesture away.
    // A duplicate burns a cap slot, adds a second identical panel row, and makes that row's gas read
    // the LATER cumulative while the overlay paints the earlier pass.
    expect(useGasPlanStore.getState().waypoints).toEqual([a, b]);
  });

  it('stops at the backend cap instead of building a plan that cannot be saved', () => {
    // `save_dive_plan` REJECTS a list past 64. Without a cap here the diver pays an extra Dijkstra
    // per way point on every keystroke and only meets the wall at save time, on a message naming
    // neither the limit nor which entry to drop. Same reasoning as the label's maxLength.
    for (let i = 0; i < 70; i++) {
      useGasPlanStore.getState().addWaypoint({ section_id: 9, station_index: i });
    }
    expect(useGasPlanStore.getState().waypoints).toHaveLength(64);
    // Non-vacuous: the cap is what stopped it, not the duplicate guard - every ref here is distinct.
    const kept = useGasPlanStore.getState().waypoints;
    expect(kept[kept.length - 1]).toEqual({ section_id: 9, station_index: 63 });
  });

  it('moveWaypoint swaps with the neighbour and clamps at both ends', () => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().addWaypoint(b);
    useGasPlanStore.getState().moveWaypoint(1, -1);
    expect(useGasPlanStore.getState().waypoints).toEqual([b, a]);
    // Off either end is a no-op, so the panel's arrows need no bounds logic of their own.
    useGasPlanStore.getState().moveWaypoint(0, -1);
    useGasPlanStore.getState().moveWaypoint(1, 1);
    useGasPlanStore.getState().moveWaypoint(99, -1);
    expect(useGasPlanStore.getState().waypoints).toEqual([b, a]);
  });

  it('a stray remove index is a no-op (never truncates the list)', () => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().removeWaypoint(-1);
    useGasPlanStore.getState().removeWaypoint(7);
    expect(useGasPlanStore.getState().waypoints).toEqual([a]);
  });

  it.each([
    // A ref not already on the list - re-adding an existing one is deliberately a no-op, which
    // would make this case pass for the wrong reason (nothing changed, so nothing was invalidated).
    ['addWaypoint', () => useGasPlanStore.getState().addWaypoint({ section_id: 2, station_index: 3 })],
    ['removeWaypoint', () => useGasPlanStore.getState().removeWaypoint(0)],
    ['moveWaypoint', () => useGasPlanStore.getState().moveWaypoint(0, 1)],
  ])(
    '%s nulls reachField and clears stageDropOverrides (the route is re-based, so the old overlay and pinned drops are both wrong)',
    (_name, mutate) => {
      // Two way points so `moveWaypoint(0, 1)` is a real move rather than an out-of-range no-op.
      useGasPlanStore.getState().addWaypoint(a);
      useGasPlanStore.getState().addWaypoint(b);
      useGasPlanStore.getState().setStageDropOverride(0, { section_id: 1, station_index: 2 });
      useGasPlanStore.setState({ reachField: fakeField() });
      mutate();
      expect(useGasPlanStore.getState().reachField).toBeNull();
      expect(useGasPlanStore.getState().stageDropOverrides).toEqual([]);
    }
  );

  it('reset clears way points (no leak across plan sessions)', () => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().waypoints).toEqual([]);
  });
});

describe('gasPlanStore segments (#317)', () => {
  const a = { section_id: 1, station_index: 1 };
  const b = { section_id: 1, station_index: 5 };
  const c = { section_id: 1, station_index: 9 };

  beforeEach(() => {
    useGasPlanStore.getState().reset();
  });

  it('starts with exactly one segment (no way points yet - a plan always has waypoints.length + 1)', () => {
    expect(useGasPlanStore.getState().waypoints).toEqual([]);
    expect(useGasPlanStore.getState().segments).toHaveLength(1);
  });

  it('seeds every fresh span with NO speed of its own, so the configured speed still reaches the engine', () => {
    // ⚠ #317 regression guard, the store half. Seeding a number here - even the correct one -
    // permanently overrides Settings > Gas and the panel's Swim-speed field, because the panel
    // sends a full-length list on every compute and `resolve_segments` only falls back to the
    // plan speed for spans that carry `null`. The Rust twin is
    // `the_plan_speed_still_reaches_the_engine_through_a_full_length_segment_list` (gas/tests.rs).
    expect(useGasPlanStore.getState().segments).toEqual([{ speed_m_per_min: null, on_scooter: false }]);
    useGasPlanStore.getState().addWaypoint(a);
    expect(
      useGasPlanStore.getState().segments.every((s) => s.speed_m_per_min === null)
    ).toBe(true);
  });

  it('stays length-synced (segments.length === waypoints.length + 1) through add/remove/move', () => {
    useGasPlanStore.getState().addWaypoint(a);
    expect(useGasPlanStore.getState().segments).toHaveLength(2);
    useGasPlanStore.getState().addWaypoint(b);
    expect(useGasPlanStore.getState().segments).toHaveLength(3);
    useGasPlanStore.getState().moveWaypoint(1, -1);
    expect(useGasPlanStore.getState().segments).toHaveLength(3);
    useGasPlanStore.getState().addWaypoint(c);
    expect(useGasPlanStore.getState().segments).toHaveLength(4);
    useGasPlanStore.getState().removeWaypoint(1);
    expect(useGasPlanStore.getState().waypoints).toHaveLength(2);
    expect(useGasPlanStore.getState().segments).toHaveLength(3);
  });

  it('stays synced at the way-point cap too (64 way points -> 65 spans)', () => {
    for (let i = 0; i < 70; i++) {
      useGasPlanStore.getState().addWaypoint({ section_id: 9, station_index: i });
    }
    expect(useGasPlanStore.getState().waypoints).toHaveLength(64);
    expect(useGasPlanStore.getState().segments).toHaveLength(65);
  });

  it('addWaypoint splits the last span; both halves inherit the split segment\'s values', () => {
    useGasPlanStore.getState().setSegments([{ speed_m_per_min: 25, on_scooter: true }]);
    useGasPlanStore.getState().addWaypoint(a);
    expect(useGasPlanStore.getState().segments).toEqual([
      { speed_m_per_min: 25, on_scooter: true },
      { speed_m_per_min: 25, on_scooter: true },
    ]);
  });

  it('removeWaypoint merges its two adjacent spans to the SLOWER speed and ORs on_scooter', () => {
    // Two way points -> 3 segments; dropping way point 0 merges segments 0 and 1. The two halves
    // of the answer come from OPPOSITE sides - speed 8 from the second span, the scooter flag from
    // the first - so no single-side rule ("take the first", "take the second") reproduces it, and
    // neither does max(speed) nor AND(flag). Both directions are the conservative one, for
    // opposite reasons: a slower span costs more gas per metre, while `on_scooter` spends RANGE and
    // buys no reduced breathing rate, so dropping the flag would hand the merged span free scooter
    // range and reach FURTHER than either half could. Mirrors the Rust
    // `merge_segments_for_surviving_waypoints` reconciliation on a stale-way-point load.
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().addWaypoint(b);
    useGasPlanStore.getState().setSegments([
      { speed_m_per_min: 20, on_scooter: true },
      { speed_m_per_min: 8, on_scooter: false },
      { speed_m_per_min: 30, on_scooter: true },
    ]);
    useGasPlanStore.getState().removeWaypoint(0);
    expect(useGasPlanStore.getState().segments).toEqual([
      { speed_m_per_min: 8, on_scooter: true },
      { speed_m_per_min: 30, on_scooter: true },
    ]);
  });

  it('removeWaypoint ranks the spans on the domain the ENGINE flies, not on the raw number', () => {
    // 2000 m/min is outside `[MIN_SEGMENT_SPEED_M_PER_MIN, MAX_SEGMENT_SPEED_M_PER_MIN]`, so
    // `resolve_segments` (gas/scooter.rs) ignores it and flies that span at the plan speed - 10
    // here, which makes it the SLOWER of the pair and the one that must win. Ranking the stored
    // number instead makes it the fastest span in the plan, the merge keeps 20, and the merged
    // span is then priced at 20 m/min: less gas per metre and more reach than the diver has. A
    // stored 2000 reaches here from a hand-edited or badly round-tripped `.plnx`.
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().addWaypoint(b);
    useGasPlanStore.getState().setSegments([
      { speed_m_per_min: 20, on_scooter: false },
      { speed_m_per_min: 2000, on_scooter: false },
      { speed_m_per_min: 9, on_scooter: false },
    ]);
    useGasPlanStore.getState().removeWaypoint(0);
    // Carrying the out-of-domain value forward is the point: the engine reads it as "inherit the
    // plan speed", which is exactly the pace the merged span has to be flown at.
    expect(useGasPlanStore.getState().segments[0]).toEqual({
      speed_m_per_min: 2000,
      on_scooter: false,
    });
  });

  it('moveWaypoint resets the two OUTER spans and keeps the one between the swapped pair', () => {
    // Three way points -> four spans. Span `k` joins way points `k-1` and `k` (origin and open
    // field standing in at the ends), so swapping way points 1 and 2 leaves span 2 joining the
    // same two stations - only walked the other way - while spans 1 and 3 now describe legs that
    // did not exist before. Keeping their values would apply a pace, or a scooter flag, to a
    // passage it was never set for; span 0 is untouched either way.
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().addWaypoint(b);
    useGasPlanStore.getState().addWaypoint(c);
    useGasPlanStore.getState().setSegments([
      { speed_m_per_min: 5, on_scooter: false },
      { speed_m_per_min: 11, on_scooter: true },
      { speed_m_per_min: 22, on_scooter: false },
      { speed_m_per_min: 33, on_scooter: true },
    ]);

    useGasPlanStore.getState().moveWaypoint(1, 1);

    expect(useGasPlanStore.getState().segments).toEqual([
      { speed_m_per_min: 5, on_scooter: false },
      { speed_m_per_min: null, on_scooter: false },
      { speed_m_per_min: 22, on_scooter: false },
      { speed_m_per_min: null, on_scooter: false },
    ]);
  });

  it('removeWaypoint merges on the EFFECTIVE speeds, so an inheriting span is not treated as infinitely fast', () => {
    // A span with no speed of its own flies at the plan speed (10 by default here), which is
    // SLOWER than the 30 next to it - so the merged span must keep inheriting rather than
    // adopting 30. Comparing the raw `null` against a number would pick the wrong half and
    // understate the gas the merged span costs.
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().addWaypoint(b);
    useGasPlanStore.getState().setSegments([
      { speed_m_per_min: null, on_scooter: false },
      { speed_m_per_min: 30, on_scooter: false },
      { speed_m_per_min: 30, on_scooter: false },
    ]);
    useGasPlanStore.getState().removeWaypoint(0);
    expect(useGasPlanStore.getState().segments[0]).toEqual({
      speed_m_per_min: null,
      on_scooter: false,
    });

    // And the other direction: an explicit speed SLOWER than the plan speed wins over an
    // inheriting neighbour.
    useGasPlanStore.getState().setSegments([
      { speed_m_per_min: null, on_scooter: false },
      { speed_m_per_min: 4, on_scooter: false },
    ]);
    useGasPlanStore.getState().removeWaypoint(0);
    expect(useGasPlanStore.getState().segments[0]).toEqual({
      speed_m_per_min: 4,
      on_scooter: false,
    });
  });

  it('setSegments nulls reachField and clears stageDropOverrides (a gas input, like setCylinders)', () => {
    useGasPlanStore.getState().setStageDropOverride(0, { section_id: 1, station_index: 2 });
    useGasPlanStore.setState({ reachField: fakeField() });
    useGasPlanStore.getState().setSegments([{ speed_m_per_min: 15, on_scooter: false }]);
    expect(useGasPlanStore.getState().reachField).toBeNull();
    expect(useGasPlanStore.getState().stageDropOverrides).toEqual([]);
  });

  it.each([
    ['addWaypoint', () => useGasPlanStore.getState().addWaypoint(b)],
    ['removeWaypoint', () => useGasPlanStore.getState().removeWaypoint(0)],
  ])('%s (which restructures segments) also nulls reachField', (_name, mutate) => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.setState({ reachField: fakeField() });
    mutate();
    expect(useGasPlanStore.getState().reachField).toBeNull();
  });

  it('the snapshot carries the live segments verbatim (round-trips through divePlanSnapshotOf)', () => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().setSegments([
      { speed_m_per_min: 12, on_scooter: true },
      { speed_m_per_min: 9, on_scooter: false },
    ]);
    const snap = divePlanSnapshotOf(useGasPlanStore.getState());
    expect(snap.segments).toEqual(useGasPlanStore.getState().segments);
  });

  it('a segment edit is dirty-tracked, so an unsaved speed/scooter change still hits the close guard', () => {
    useGasPlanStore.getState().addWaypoint(a);
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(5, snapshot);
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);
    const segs = useGasPlanStore.getState().segments;
    useGasPlanStore.getState().setSegments(
      segs.map((s, i) => (i === 0 ? { ...s, on_scooter: !s.on_scooter } : s))
    );
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(true);
  });

  it('normalises a loaded plan that carries NO segments (every pre-#317 .plnx)', () => {
    // The regression this pins: `merge_segments_for_surviving_waypoints([], ..)` returns `[]`
    // unconditionally, `applyLoadedPlan` took the snapshot verbatim, and add/remove way point
    // both ASSUME the length invariant already holds - so they never healed it. Every plan
    // written before segments existed loaded with zero rows in the Segments tab, for a route
    // with two spans.
    useGasPlanStore.getState().addWaypoint(a);
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().reset();
    useGasPlanStore.getState().applyLoadedPlan(1, { ...snapshot, segments: [] });
    expect(useGasPlanStore.getState().waypoints).toHaveLength(1);
    expect(useGasPlanStore.getState().segments).toEqual([
      { speed_m_per_min: null, on_scooter: false },
      { speed_m_per_min: null, on_scooter: false },
    ]);
  });

  it('drops an engine-invalid rig on load, from the working config AND the baseline', () => {
    // Mirrors `GasConfig::normalize`'s drop-the-whole-rig rule at the compute door
    // (`validate_plan_inputs`): a hand-edited .plnx carrying a zero-burn unit would otherwise
    // show a live rig toggle + usable-range figure while the engine plans rig-less - the #233
    // effective-state class. The baseline is normalized identically, so the freshly loaded
    // plan reads clean rather than instantly dirty.
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(7, {
      ...snapshot,
      gas_config: {
        ...snapshot.gas_config,
        dpv: {
          primary: { speed_m_per_min: 45, burn_minutes: 0 },
          backup: { speed_m_per_min: 45, burn_minutes: 45 },
          range_reserve_fraction: 0.1,
        },
      },
    });
    expect(useGasPlanStore.getState().gasConfig.dpv).toBeNull();
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);
  });

  it('keeps a valid loaded rig untouched (the load-time drop is unit-invalidity only)', () => {
    const rig = {
      primary: { speed_m_per_min: 45, burn_minutes: 60 },
      backup: { speed_m_per_min: 45, burn_minutes: 45 },
      // Out-of-domain reserve does NOT drop the rig - normalize clamps it and keeps the rig,
      // and the editor preview normalizes for display the same way.
      range_reserve_fraction: 5.0,
    };
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(8, {
      ...snapshot,
      gas_config: { ...snapshot.gas_config, dpv: rig },
    });
    expect(useGasPlanStore.getState().gasConfig.dpv).toEqual(rig);
  });

  it('drops loaded span speeds outside the range the engine flies, and says how many (#576 NF-UI-2)', () => {
    // A hand-edited .plnx: the engine flies only [0.1, 1000] m/min and silently uses the plan
    // speed for anything else, while the Segments grid showed the stored number as the span's
    // explicit speed - one number on screen, another in the plan. The load drops them instead,
    // so the row shows the plan speed it is really flown at, and reports the count for one
    // notice. The baseline is the dropped snapshot, so the plan loads clean.
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    const waypoints = [1, 2, 3, 4, 5].map((i) => ({ section_id: 1, station_index: i }));
    const dropped = useGasPlanStore.getState().applyLoadedPlan(10, {
      ...snapshot,
      waypoints,
      segments: [
        { speed_m_per_min: 0, on_scooter: false },
        { speed_m_per_min: -12, on_scooter: false },
        { speed_m_per_min: 5000, on_scooter: false },
        { speed_m_per_min: 0.05, on_scooter: false },
        { speed_m_per_min: 12, on_scooter: false },
        { speed_m_per_min: null, on_scooter: false },
      ],
    });
    expect(useGasPlanStore.getState().segments.map((seg) => seg.speed_m_per_min)).toEqual([
      null,
      null,
      null,
      null,
      12,
      null,
    ]);
    expect(dropped).toEqual({ droppedSpeeds: 4 });
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);
  });

  it('keeps every in-range loaded span speed, the range ends included, and reports none dropped', () => {
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    const dropped = useGasPlanStore.getState().applyLoadedPlan(11, {
      ...snapshot,
      waypoints: [{ section_id: 1, station_index: 1 }, { section_id: 1, station_index: 2 }],
      segments: [
        { speed_m_per_min: 0.1, on_scooter: false },
        { speed_m_per_min: 1000, on_scooter: false },
        { speed_m_per_min: 15, on_scooter: false },
      ],
    });
    expect(useGasPlanStore.getState().segments.map((seg) => seg.speed_m_per_min)).toEqual([0.1, 1000, 15]);
    expect(dropped).toEqual({ droppedSpeeds: 0 });
  });

  it('grounds scootered spans when the load drops an invalid rig, exactly like setDpvRig(null)', () => {
    // The engine gates `on_scooter` off without a rig but still flies the span's auto-filled
    // scooter pace, and a latent flag would spring back to life on the next rig install - a
    // scootered plan reappearing from a rig the load just refused.
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(9, {
      ...snapshot,
      waypoints: [{ section_id: 1, station_index: 1 }],
      segments: [
        { speed_m_per_min: 45, on_scooter: true },
        { speed_m_per_min: 12, on_scooter: false },
      ],
      gas_config: {
        ...snapshot.gas_config,
        dpv: {
          primary: { speed_m_per_min: 45, burn_minutes: 0 },
          backup: { speed_m_per_min: 45, burn_minutes: 45 },
          range_reserve_fraction: 0.1,
        },
      },
    });
    const s = useGasPlanStore.getState();
    expect(s.gasConfig.dpv).toBeNull();
    // The scootered span is fully reset; a swum span's hand-set pace survives.
    expect(s.segments).toEqual([
      { speed_m_per_min: null, on_scooter: false },
      { speed_m_per_min: 12, on_scooter: false },
    ]);
    // The baseline got the same scrub, so the loaded plan still reads clean.
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);
  });

  it('FOLDS a loaded plan that carries more segments than it has spans, never truncates', () => {
    // A bare `slice(0, want)` discards the diver's statement about those spans, and discards every
    // field in the REACH-EXTENDING direction: a dropped slow speed reverts to the plan speed
    // (cheaper gas per metre) and a dropped `on_scooter` stops spending range. The fold keeps the
    // slowest effective speed and any scooter use, mirroring `normalize_segment_count`
    // (core/state/dive_plan.rs).
    //
    // The three values are distinct and land in three different places: the retained span inherits
    // (10 m/min), tail[0] contributes only the scooter flag, tail[1] only the speed. So truncating,
    // keeping the last tail entry, or keeping the first each give a different - wrong - answer.
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(1, {
      ...snapshot,
      waypoints: [],
      segments: [
        { speed_m_per_min: null, on_scooter: false },
        { speed_m_per_min: 30, on_scooter: true },
        { speed_m_per_min: 3, on_scooter: false },
      ],
    });
    expect(useGasPlanStore.getState().segments).toEqual([
      { speed_m_per_min: 3, on_scooter: true },
    ]);
  });

  it('a normalised plan stays length-synced through the next add and remove', () => {
    // The healing has to leave the store in the state the setters assume, not merely look right
    // once: `removeWaypoint` indexes `segments[index + 1]` with no fallback of its own.
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore
      .getState()
      .applyLoadedPlan(1, { ...snapshot, waypoints: [a, b], segments: [] });
    expect(useGasPlanStore.getState().segments).toHaveLength(3);
    useGasPlanStore.getState().addWaypoint(c);
    expect(useGasPlanStore.getState().segments).toHaveLength(4);
    useGasPlanStore.getState().removeWaypoint(1);
    expect(useGasPlanStore.getState().segments).toHaveLength(3);
  });

  it('setSegments pads or folds to the live span count', () => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().setSegments([{ speed_m_per_min: 7, on_scooter: false }]);
    expect(useGasPlanStore.getState().segments).toEqual([
      { speed_m_per_min: 7, on_scooter: false },
      { speed_m_per_min: null, on_scooter: false },
    ]);
    useGasPlanStore.getState().setSegments([
      { speed_m_per_min: 7, on_scooter: false },
      { speed_m_per_min: 8, on_scooter: false },
      { speed_m_per_min: 4, on_scooter: true },
    ]);
    // The overflow is folded into the closing span, not dropped: 4 is slower than 8 and the
    // scooter flag survives.
    expect(useGasPlanStore.getState().segments).toEqual([
      { speed_m_per_min: 7, on_scooter: false },
      { speed_m_per_min: 4, on_scooter: true },
    ]);
  });

  it('reset restores the single default segment (no leak across plan sessions)', () => {
    useGasPlanStore.getState().addWaypoint(a);
    useGasPlanStore.getState().setSegments([
      { speed_m_per_min: 5, on_scooter: false },
      { speed_m_per_min: 5, on_scooter: false },
    ]);
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().segments).toHaveLength(1);
  });
});

/**
 * `setDpvRig` (#317). The bug this closes: removing the rig used to leave every segment exactly
 * as it was, so a span the diver had marked scootered kept flying at the auto-filled scooter
 * speed with no rig behind it any more - the engine priced gas at a pace the diver could no
 * longer sustain, and the field/route read further than the dive could actually go.
 */
describe('gasPlanStore setDpvRig (#317)', () => {
  const rig = (speed: number) => ({
    primary: { speed_m_per_min: speed, burn_minutes: 120, label: null },
    backup: { speed_m_per_min: speed, burn_minutes: 60, label: null },
    range_reserve_fraction: 0.1,
  });

  beforeEach(() => {
    useGasPlanStore.getState().reset();
  });

  it('removing the rig resets every scootered segment to swim pace (the unsafe direction closed)', () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: rig(45) },
      segments: [
        { speed_m_per_min: 45, on_scooter: true },
        { speed_m_per_min: 10, on_scooter: false },
      ],
      reachField: fakeField(),
    });

    useGasPlanStore.getState().setDpvRig(null);

    const s = useGasPlanStore.getState();
    expect(s.gasConfig.dpv).toBeNull();
    expect(s.segments[0]).toEqual({ speed_m_per_min: null, on_scooter: false });
    expect(s.reachField).toBeNull();
  });

  it("leaves a non-scootered segment's explicit speed alone when the rig is removed", () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: rig(45) },
      segments: [{ speed_m_per_min: 8, on_scooter: false }],
    });

    useGasPlanStore.getState().setDpvRig(null);

    // A manually typed swim-pace speed has nothing to do with the rig - it is left untouched.
    expect(useGasPlanStore.getState().segments[0]).toEqual({
      speed_m_per_min: 8,
      on_scooter: false,
    });
  });

  it('installing a rig leaves segments untouched', () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: null },
      segments: [{ speed_m_per_min: 45, on_scooter: true }],
    });

    useGasPlanStore.getState().setDpvRig(rig(60));

    const s = useGasPlanStore.getState();
    expect(s.gasConfig.dpv).toEqual(rig(60));
    expect(s.segments[0]).toEqual({ speed_m_per_min: 45, on_scooter: true });
  });

  it('nulls the cached reachField (a gas input, like setGasConfig)', () => {
    useGasPlanStore.setState({ reachField: fakeField() });
    useGasPlanStore.getState().setDpvRig(rig(45));
    expect(useGasPlanStore.getState().reachField).toBeNull();
  });
});

describe('gasPlanStore marked points (#298)', () => {
  const at = { section_id: 2, station_index: 3 };

  beforeEach(() => {
    useGasPlanStore.getState().reset();
  });

  it('adds unlabelled, relabels, and removes by index', () => {
    useGasPlanStore.getState().addMarkedPoint(at);
    expect(useGasPlanStore.getState().markedPoints).toEqual([{ station: at, label: null }]);
    useGasPlanStore.getState().setMarkedPointLabel(0, 'photo');
    expect(useGasPlanStore.getState().markedPoints[0]?.label).toBe('photo');
    useGasPlanStore.getState().removeMarkedPoint(0);
    expect(useGasPlanStore.getState().markedPoints).toEqual([]);
  });

  it('keeps a label EXACTLY as typed, so a space survives the next keystroke', () => {
    // The panel's input is controlled off this value. Trimming here made the store answer "photo"
    // to the DOM's "photo ", React restored the shorter string, and the space was eaten on every
    // keystroke - so a multi-word label could not be typed by hand at all. These three calls are
    // the exact per-keystroke sequence; an e2e `fill()` dispatches one event and never sees it.
    useGasPlanStore.getState().addMarkedPoint(at);
    useGasPlanStore.getState().setMarkedPointLabel(0, 'photo');
    useGasPlanStore.getState().setMarkedPointLabel(0, 'photo ');
    expect(useGasPlanStore.getState().markedPoints[0]?.label).toBe('photo ');
    useGasPlanStore.getState().setMarkedPointLabel(0, 'photo stop');
    expect(useGasPlanStore.getState().markedPoints[0]?.label).toBe('photo stop');
  });

  it('normalises the label at the SAVE boundary instead - padding trimmed, blank becomes null', () => {
    // The trim did not disappear, it moved to `divePlanSnapshotOf`, which is both what the backend
    // receives and what dirty-tracking diffs. So a trailing space cannot leave a plan permanently
    // dirty against a value the save would have normalised away.
    useGasPlanStore.getState().addMarkedPoint(at);
    useGasPlanStore.getState().setMarkedPointLabel(0, '  photo stop  ');
    expect(divePlanSnapshotOf(useGasPlanStore.getState()).marked_points[0]?.label).toBe(
      'photo stop'
    );
    useGasPlanStore.getState().setMarkedPointLabel(0, '   ');
    expect(divePlanSnapshotOf(useGasPlanStore.getState()).marked_points[0]?.label).toBeNull();
  });

  it.each([
    ['addMarkedPoint', () => useGasPlanStore.getState().addMarkedPoint(at)],
    ['setMarkedPointLabel', () => useGasPlanStore.getState().setMarkedPointLabel(0, 'x')],
    ['removeMarkedPoint', () => useGasPlanStore.getState().removeMarkedPoint(0)],
  ])(
    '%s does NOT null reachField (an annotation cannot change the plan, so flashing the overlay off would be a lie)',
    (_name, mutate) => {
      useGasPlanStore.getState().addMarkedPoint(at);
      useGasPlanStore.setState({ reachField: fakeField() });
      mutate();
      expect(useGasPlanStore.getState().reachField).not.toBeNull();
    }
  );

  it('but marking IS dirty-tracked, so an unsaved mark still hits the close guard', () => {
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(3, snapshot);
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);
    useGasPlanStore.getState().addMarkedPoint(at);
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(true);
  });

  it('reset clears marked points (no leak across plan sessions)', () => {
    useGasPlanStore.getState().addMarkedPoint(at);
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().markedPoints).toEqual([]);
  });
});

/** A persisted gas config that differs from the compiled default only where asked. */
const gas = (overrides: Partial<GasConfig> = {}): GasConfig => ({
  ...structuredClone(defaultConfig.gas),
  ...overrides,
});

describe('gasPlanStore dive-plan dirty tracking (#244)', () => {
  beforeEach(() => {
    useGasPlanStore.getState().reset();
  });

  it('is not dirty when no plan is loaded (a never-saved exploration is unguarded)', () => {
    useGasPlanStore.getState().setOrigin({ section_id: 1, station_index: 0 });
    expect(useGasPlanStore.getState().loadedPlanId).toBeNull();
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);
  });

  it('applyLoadedPlan sets inputs + baseline, nulls reachField, and reads clean', () => {
    useGasPlanStore.getState().setReachField(fakeField());
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    snapshot.origin = { section_id: 4, station_index: 1 };
    useGasPlanStore.getState().applyLoadedPlan(7, snapshot);
    const s = useGasPlanStore.getState();
    expect(s.loadedPlanId).toBe(7);
    expect(s.origin).toEqual({ section_id: 4, station_index: 1 });
    expect(s.reachField).toBeNull(); // stale overlay dropped
    expect(selectIsPlanDirty(s)).toBe(false);
  });

  it('applyLoadedPlan disarms a stale origin pick (#395 review)', () => {
    // A pick left armed from before the load (e.g. the panel unmounted mid-pick) must not
    // consume the next plain map click as an overwrite of the plan that just loaded.
    useGasPlanStore.getState().setOriginPickArmed(true);
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(7, snapshot);
    expect(useGasPlanStore.getState().originPickArmed).toBe(false);
  });

  it('selectHasPins counts BASELINE pins, so the anchor round trip survives clearing the live ones', () => {
    // Load a plan with an endpoint, then clear it live: `selectIsPlanDirty` still compares
    // against the baseline's ref, which a renumbering edit would silently rebind if the
    // mutation seam read "no live pins" as "nothing to remap".
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(3, {
      ...snapshot,
      endpoint: { section_id: 1, station_index: 4 },
    });
    useGasPlanStore.getState().setEndpoint(null);
    expect(selectHasPins(useGasPlanStore.getState())).toBe(true);
    // Unloading the plan drops the baseline, and with it the last reason to anchor anything.
    useGasPlanStore.getState().clearLoadedPlan();
    expect(selectHasPins(useGasPlanStore.getState())).toBe(false);
  });

  it('becomes dirty when an input diverges from the loaded baseline', () => {
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(1, snapshot);
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);
    useGasPlanStore.getState().setOrigin({ section_id: 9, station_index: 9 });
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(true);
  });

  it('markPlanSaved rebaselines to the current inputs (clean again)', () => {
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(1, snapshot);
    useGasPlanStore.getState().setDiver({ rmv_l_per_min: 15, label: null });
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(true);
    // markPlanSaved takes the exact persisted snapshot (the current inputs here) as the baseline.
    useGasPlanStore.getState().markPlanSaved(1, divePlanSnapshotOf(useGasPlanStore.getState()));
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);
    expect(useGasPlanStore.getState().loadedPlanId).toBe(1);
  });

  it('clearLoadedPlan forgets the plan without touching inputs (and reads clean)', () => {
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(1, snapshot);
    useGasPlanStore.getState().setOrigin({ section_id: 2, station_index: 2 });
    useGasPlanStore.getState().clearLoadedPlan();
    const s = useGasPlanStore.getState();
    expect(s.loadedPlanId).toBeNull();
    expect(s.origin).toEqual({ section_id: 2, station_index: 2 }); // inputs untouched
    expect(selectIsPlanDirty(s)).toBe(false);
  });

  it('reset clears the loaded plan (no leak across plan sessions)', () => {
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(3, snapshot);
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().loadedPlanId).toBeNull();
    expect(useGasPlanStore.getState().loadedPlanBaseline).toBeNull();
  });

  it('markHydrated seeds the Settings-only baseline, so a later sync is not read as an edit', () => {
    useGasPlanStore.getState().markHydrated(gas({ jump_rmv_multiplier: 3 }));
    expect(useGasPlanStore.getState().lastSeenPersistedGas?.jump_rmv_multiplier).toBe(3);
    // Same values arriving from the panel's first sync must not count as a change.
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 3 }));
    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(
      defaultConfig.gas.jump_rmv_multiplier
    );
  });

  it('markHydrated sets the session seed flag; reset clears it (re-seed on new session)', () => {
    // Gates the panel's one-time config seed so re-entering Plan mode does not clobber
    // a loaded plan / working inputs (the store persists across mode switches now).
    expect(useGasPlanStore.getState().hydrated).toBe(false);
    useGasPlanStore.getState().markHydrated();
    expect(useGasPlanStore.getState().hydrated).toBe(true);
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().hydrated).toBe(false);
  });

  it('applyLoadedPlan with a null-origin snapshot restores the entrance origin (#480)', () => {
    // A saved plan whose own origin was the entrance persists `origin: null` in its snapshot
    // (the same value `chooseEntrance` leaves it at); loading it must answer the entry
    // question the same way clicking "Start from entrance" does, not just carry the null
    // through unexamined.
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    expect(snapshot.origin).toBeNull();
    useGasPlanStore.getState().applyLoadedPlan(11, snapshot);
    const s = useGasPlanStore.getState();
    expect(s.originChosen).toBe(true);
    expect(s.origin).toBeNull();
  });
});

/**
 * Settings-only gas fields, mirrored into the working config mid-session (#317).
 *
 * The bug this replaces: the marker for "what did Settings last say" lived in a `useRef` inside
 * `GasPlanPanel`, which UNMOUNTS on every mode switch. So the sequence a diver actually performs -
 * open Plan, go to Survey, nudge the jump multiplier in Settings, come back - threw the marker
 * away and re-seeded it from the NEW value, making a real edit look like a first observation. The
 * new value never appeared, and there is no in-panel control that could have explained why.
 */
describe('gasPlanStore Settings-only gas sync (#317)', () => {
  const rig = (speed: number) => ({
    primary: { speed_m_per_min: speed, burn_minutes: 120, label: null },
    backup: { speed_m_per_min: speed, burn_minutes: 60, label: null },
    range_reserve_fraction: 0.1,
  });

  beforeEach(() => {
    useGasPlanStore.getState().reset();
  });

  it('ignores the first observation, then adopts a later change', () => {
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 2 }));
    // The hydrate seed already applied this config; treating it as an edit would clobber a
    // loaded plan's own multiplier with the diver's personal default on every panel mount.
    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(
      defaultConfig.gas.jump_rmv_multiplier
    );

    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 3 }));
    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(3);
  });

  it('survives a panel REMOUNT - the marker is store state, not a component ref', () => {
    // A remount re-runs the panel's hydrate. If that re-seeded the marker, the edit below would
    // read as a first observation and be swallowed. This is the exact #317 M2 regression, and it
    // only reproduces across an unmount: a test that merely re-renders cannot see it.
    useGasPlanStore.getState().markHydrated(gas({ jump_rmv_multiplier: 1.5 }));
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 1.5 }));
    // ... diver leaves Plan mode (panel unmounts), nudges the jump multiplier, comes back (panel
    // remounts, hydrate runs again against the already-hydrated store).
    useGasPlanStore.getState().markHydrated(gas({ jump_rmv_multiplier: 2.5 }));
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 2.5 }));
    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(2.5);
  });

  it('applies with a plan loaded, and the plan goes DIRTY rather than the edit being dropped', () => {
    const snapshot = divePlanSnapshotOf(useGasPlanStore.getState());
    useGasPlanStore.getState().applyLoadedPlan(4, snapshot);
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 1.5 }));
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);

    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 2.5 }));
    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(2.5);
    // Deliberate: the setting has no in-panel control, so silently ignoring it while a plan is
    // open would be indistinguishable from a broken setting. The diver saves or discards.
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(true);
    // And the baseline is NOT rebaselined - that would hide the divergence it just created.
    expect(useGasPlanStore.getState().loadedPlanBaseline?.gas_config.jump_rmv_multiplier).toBe(
      snapshot.gas_config.jump_rmv_multiplier
    );
  });

  it('nulls the cached reachField - both fields are gas inputs', () => {
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ include_reference_legs: false }));
    useGasPlanStore.getState().setReachField(fakeField());
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ include_reference_legs: true }));
    expect(useGasPlanStore.getState().reachField).toBeNull();
  });

  it("adopts ONLY the field that moved, so a jump edit cannot flip a loaded plan's reference-legs toggle", () => {
    // The two fields share nothing but the accident of having no in-panel control. Diffing them
    // as a PAIR and then spreading both copies the one that did NOT change - so nudging the jump
    // multiplier in Settings would silently flip the reference-legs toggle the loaded plan was
    // built around, in the one place a plan's snapshot is allowed to move underneath it.
    useGasPlanStore.getState().markHydrated(
      gas({ jump_rmv_multiplier: 1.5, include_reference_legs: true })
    );
    const snapshot = divePlanSnapshotOf({
      ...useGasPlanStore.getState(),
      gasConfig: gas({ jump_rmv_multiplier: 1.5, include_reference_legs: false }),
    });
    useGasPlanStore.getState().applyLoadedPlan(3, snapshot);
    expect(useGasPlanStore.getState().gasConfig.include_reference_legs).toBe(false);
    expect(selectIsPlanDirty(useGasPlanStore.getState())).toBe(false);

    // Settings still holds the same reference-legs toggle; the diver changes only the jump
    // multiplier.
    useGasPlanStore.getState().syncSettingsOnlyGasFields(
      gas({ jump_rmv_multiplier: 2.5, include_reference_legs: true })
    );

    const after = useGasPlanStore.getState();
    expect(after.gasConfig.jump_rmv_multiplier).toBe(2.5);
    expect(after.gasConfig.include_reference_legs).toBe(snapshot.gas_config.include_reference_legs);
    // And the plan is dirty for that one field alone - not for a toggle it never lost.
    expect(after.loadedPlanBaseline?.gas_config.include_reference_legs).toBe(
      snapshot.gas_config.include_reference_legs
    );
  });

  it('a no-change call is a true no-op (same state object, no wasted refetch)', () => {
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 1.5 }));
    const before = useGasPlanStore.getState();
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 1.5 }));
    expect(useGasPlanStore.getState()).toBe(before);
  });

  it('does not mirror a persisted dpv change - that field now has its own setter (setDpvRig)', () => {
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ dpv: null, jump_rmv_multiplier: 1.5 }));
    expect(useGasPlanStore.getState().gasConfig.dpv).toBeNull();

    // Settings gains a rig, and the jump multiplier moves too, in the SAME call.
    useGasPlanStore.getState().syncSettingsOnlyGasFields(
      gas({ dpv: rig(45), jump_rmv_multiplier: 2.5 })
    );

    const s = useGasPlanStore.getState();
    // jump_rmv_multiplier is still a Settings-only field and syncs...
    expect(s.gasConfig.jump_rmv_multiplier).toBe(2.5);
    // ...but dpv is not - a Settings-side rig change must never overwrite whatever the panel's
    // own DPV editor is holding.
    expect(s.gasConfig.dpv).toBeNull();
  });

  it('a sync clears pinned stage drops, like every other route-input setter', () => {
    // The jump multiplier re-prices legs and the reference toggle changes the graph - either can
    // move the route and its drop positions, so a ▼ dragged on the old route must not be sent
    // into the recompute against the new one.
    useGasPlanStore.getState().markHydrated(gas({ jump_rmv_multiplier: 1.5 }));
    useGasPlanStore.getState().setStageDropOverride(0, { section_id: 1, station_index: 2 });
    expect(useGasPlanStore.getState().stageDropOverrides).toHaveLength(1);
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 2.5 }));
    expect(useGasPlanStore.getState().stageDropOverrides).toEqual([]);
    expect(useGasPlanStore.getState().unresolved.stageDropOverrides).toEqual([]);
  });

  it('reset clears the marker, so a new project re-seeds instead of comparing across projects', () => {
    useGasPlanStore.getState().syncSettingsOnlyGasFields(gas({ jump_rmv_multiplier: 1.5 }));
    expect(useGasPlanStore.getState().lastSeenPersistedGas).not.toBeNull();
    useGasPlanStore.getState().reset();
    expect(useGasPlanStore.getState().lastSeenPersistedGas).toBeNull();
  });
});
