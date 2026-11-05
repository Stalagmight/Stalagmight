/**
 * Determinism of the gas-reach overlay masks (#576 fuzzers, FZ-DET-TS). See
 * `src/test/determinism.ts` for the kit and the env knob.
 *
 * The backend's `GasReachField` reports per-station facts as PARALLEL arrays
 * (`station_refs[i]`, `gas_to_reach[i]`, `reachable[i]`); which station comes
 * first is not part of the contract - it is a set of station facts. So:
 *
 * Asserted, per seed, on a synthetic survey and field:
 *  - `buildGasReachFeatureState` on the same field twice (another field in
 *    between) gives the same maps, entry order included;
 *  - the field's parallel arrays permuted together: the shot / station value
 *    maps are identical INCLUDING their insertion order (they are filled in
 *    survey order, gasReach.ts 204-220), so is the beyond-reach dim set
 *    (`gasBeyondReachFeatureIds`); the reachable-key set has the same members;
 *    `nearestOriginShotEndpoint` picks the same endpoint for every shot (with the
 *    active-station list shuffled too);
 *  - end to end through the REAL `SurveyCustomController` (only the GL layer is
 *    faked): the bytes the overlay draws - shot / station colors and the
 *    per-vertex dim levels - are identical for the permuted field and shuffled
 *    anchored / immovable sets, and identical whether they were reached by a
 *    fresh build or by a second `sync` + `setDimming` on a live controller
 *    (the color-only fast path).
 *
 * NOT asserted: the INSERTION order of `reachableStationKeySet` - it follows
 * the field (gasReach.ts 47-56) and its one consumer only asks `has`
 * (`useMapTools.ts` nearestReachableStationRef). Duplicate refs in one field
 * (last one wins, gasReach.ts 193-199) are malformed input, not generated.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { GasReachField } from '../../types/ipc.generated';
import type { StationRef } from '../../types/project';
import type { SurveyData } from '../../types/survey-data';
import {
  buildGasReachFeatureState,
  gasBeyondReachFeatureIds,
  nearestOriginShotEndpoint,
  reachableStationKeySet,
} from '../gasReach';
import { FEATURE_ID_STRIDE } from '../ids';
import { makeFakeMap } from '../../test/factories/fakeMap';
import {
  determinismSeeds,
  mulberry32,
  permutation,
  seededDiff,
  shuffled,
  shuffledSet,
  syntheticSurvey,
  type Rng,
} from '../../test/determinism';

// The controller's dev build log (bound to console.log at import) is noise here.
vi.mock('../debugLog', () => ({ debugLog: () => {} }));

/** The GL layer is the only fake: it keeps the data the controller hands it. */
const fake = vi.hoisted(() => ({ last: null as { data: unknown } | null }));
vi.mock('../../components/Map/surveyLayer/customRender/surveyCustomLayer', () => {
  class FakeLayer {
    readonly id: string;
    data: unknown;
    constructor(id: string, data: unknown) {
      this.id = id;
      this.data = data;
      fake.last = this;
    }
    onAdd(): void {}
    isDisabled(): boolean {
      return false;
    }
    getDisabledReason(): string | null {
      return null;
    }
    update(data: unknown): void {
      this.data = data;
    }
    setShotsVisible(): void {}
    setStationsVisible(): void {}
    setReferenceShotsVisible(): void {}
    setJumpShotsVisible(): void {}
    updateColors(): void {}
    updateDim(): void {}
    updateHide(): void {}
    updateHighlight(): void {}
    uploadLineVertexRange(): void {}
    uploadCircleVertexRange(): void {}
  }
  return { SurveyCustomLayer: FakeLayer };
});

const { SurveyCustomController, buildCustomDimState, GAS_DIM_LEVEL } = await import(
  '../../components/Map/surveyLayer/customRender/controller'
);
const { overlayInputFrom } = await import('../../components/Map/surveyLayer/customRender/overlayColor');
type CustomSyncParams = import('../../components/Map/surveyLayer/customRender/controller').CustomSyncParams;
type CustomRenderData = import('../../components/Map/surveyLayer/customRender/surveyCustomLayer').CustomRenderData;

const SEEDS = determinismSeeds();

function randomField(rng: Rng, data: SurveyData): GasReachField {
  const refs: StationRef[] = [];
  const gas: (number | null)[] = [];
  const reachable: boolean[] = [];
  for (const section of data.sections) {
    for (let idx = 0; idx <= section.shotCount; idx++) {
      if (rng() < 0.1) continue; // a station the field does not mention
      refs.push({ section_id: section.id, station_index: idx });
      const cost = rng() < 0.1 ? null : Math.round(rng() * 3000) / 10;
      gas.push(cost);
      reachable.push(cost !== null && rng() < 0.7);
    }
  }
  const order = permutation(refs.length, rng);
  return {
    station_refs: order.map((i) => refs[i]!),
    gas_to_reach: order.map((i) => gas[i]!),
    reachable: order.map((i) => reachable[i]!),
    turn_frontier: [],
    max_reachable_gas: 300,
    budget_litres: 250,
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

/** The same station facts, the parallel arrays permuted together. */
function permutedField(field: GasReachField, rng: Rng): GasReachField {
  const order = permutation(field.station_refs.length, rng);
  return {
    ...field,
    station_refs: order.map((i) => field.station_refs[i]!),
    gas_to_reach: order.map((i) => field.gas_to_reach[i]!),
    reachable: order.map((i) => field.reachable[i]!),
  };
}

function activeStations(data: SurveyData) {
  return Array.from({ length: data.stationCount }, (_, g) => ({
    station_ref: { section_id: data.stationSectionIds[g]!, station_index: data.stationIndices[g]! },
    coordinates: { x: data.stationCoords[g * 3]!, y: data.stationCoords[g * 3 + 1]!, z: data.stationCoords[g * 3 + 2]! },
    is_zero_datum: (data.stationFlags[g]! & 1) !== 0,
  }));
}

function masks(data: SurveyData, field: GasReachField) {
  const state = buildGasReachFeatureState(data, field);
  return {
    state,
    dimShots: gasBeyondReachFeatureIds(state.shots),
    dimStations: gasBeyondReachFeatureIds(state.stations),
  };
}

const THEME = {
  stationDefault: '#ffffff',
  zeroDatum: '#00ffff',
  strokeColor: '#000000',
  depthShallow: '#0000ff',
  depthDeep: '#ff0000',
  severityError: '#ff0000',
  severityWarning: '#ffff00',
  severityClear: '#00000000',
  grayLine: '#808080',
};

function syncParams(data: SurveyData, field: GasReachField): CustomSyncParams {
  const state = buildGasReachFeatureState(data, field);
  return {
    surveyData: data,
    gpsOrigin: { latitude: 20.21, longitude: -87.46 },
    colorCtx: {
      showDepthGradient: false,
      showIssueSeverity: false,
      depthRange: { min: 0, max: 40 },
      themeColors: THEME as unknown as CustomSyncParams['colorCtx']['themeColors'],
      overlay: overlayInputFrom(false, null, true, state),
    },
    shotWidthPx: 2,
    stationRadiusPx: 3,
    stationStrokePx: 1,
  };
}

/** The bytes the gas overlay puts on screen: colors + per-vertex dim levels. */
function drawn(data: CustomRenderData) {
  return {
    lineColorFrom: data.line.colorFrom,
    lineColorTo: data.line.colorTo,
    lineDim: data.line.dim,
    circleFill: data.circle.fill,
    circleDim: data.circle.dim,
  };
}

interface DimSets {
  anchored: Set<number>;
  immovableStations: Set<number>;
  immovableShots: Set<number>;
}

function randomDimSets(rng: Rng, data: SurveyData): DimSets {
  const pickSome = (xs: number[], rate: number) => new Set(xs.filter(() => rng() < rate));
  return {
    anchored: pickSome(data.sections.map((s) => s.id), 0.2),
    immovableStations: pickSome(
      Array.from({ length: data.stationCount }, (_, g) => data.stationSectionIds[g]! * FEATURE_ID_STRIDE + data.stationIndices[g]!),
      0.1,
    ),
    immovableShots: pickSome(
      Array.from({ length: data.shotCount }, (_, g) => data.shotSectionIds[g]! * FEATURE_ID_STRIDE + data.shotIds[g]!),
      0.1,
    ),
  };
}

function dimState(data: SurveyData, field: GasReachField, sets: DimSets) {
  return buildCustomDimState(
    sets.anchored, sets.immovableStations, sets.immovableShots, true, buildGasReachFeatureState(data, field), false,
  );
}

/** A fresh controller, one sync + one setDimming: what a first overlay frame draws. */
function freshDraw(data: SurveyData, field: GasReachField, sets: DimSets) {
  const { map } = makeFakeMap({ visibility: {} });
  const c = new SurveyCustomController(map);
  c.sync(syncParams(data, field));
  c.setDimming(dimState(data, field, sets));
  const out = drawn(fake.last!.data as CustomRenderData);
  c.dispose();
  return out;
}

describe('gas-reach overlay masks are deterministic (FZ-DET-TS)', () => {
  beforeEach(() => {
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('same field twice (another field in between) -> identical maps and dim sets', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const data = syntheticSurvey(rng);
      const field = randomField(rng, data);
      const first = masks(data, field);
      const otherRng = mulberry32(seed + 1_000_003);
      const other = syntheticSurvey(otherRng);
      masks(other, randomField(otherRng, other));
      const second = masks(data, field);
      expect(seededDiff(seed, 'masks', first, second)).toBeNull();
    }
  });

  it('field arrays permuted together -> identical maps, dim sets, reachable members, endpoint picks', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const data = syntheticSurvey(rng);
      const field = randomField(rng, data);
      const reference = masks(data, field);
      const stations = activeStations(data);
      for (let round = 0; round < 3; round++) {
        const shuffleRng = mulberry32(seed * 31 + round);
        const variantField = permutedField(field, shuffleRng);
        expect(seededDiff(seed, `masks (round ${round})`, reference, masks(data, variantField))).toBeNull();
        expect(
          seededDiff(seed, `reachable keys (round ${round})`, reachableStationKeySet(field), reachableStationKeySet(variantField), {
            ordered: false,
          }),
        ).toBeNull();
        const shuffledStations = shuffled(stations, shuffleRng);
        for (const section of data.sections) {
          for (let i = 0; i < section.shotCount; i++) {
            const a = { section_id: section.id, station_index: i };
            const b = { section_id: section.id, station_index: i + 1 };
            const want = nearestOriginShotEndpoint(a, b, field, stations);
            const got = nearestOriginShotEndpoint(a, b, variantField, shuffledStations);
            expect(seededDiff(seed, `endpoint ${section.id}:${i} (round ${round})`, want, got)).toBeNull();
            // Without a field the pick falls back to the zero datums.
            const wantNoField = nearestOriginShotEndpoint(a, b, null, stations);
            const gotNoField = nearestOriginShotEndpoint(a, b, null, shuffledStations);
            expect(seededDiff(seed, `datum endpoint ${section.id}:${i} (round ${round})`, wantNoField, gotNoField)).toBeNull();
          }
        }
      }
    }
  });

  it('what the overlay draws (real controller): permuted field + shuffled sets, fresh or live -> identical bytes', () => {
    let sawGasDim = false;
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const data = syntheticSurvey(rng);
      const field = randomField(rng, data);
      const sets = randomDimSets(rng, data);
      const reference = freshDraw(data, field, sets);

      const shuffleRng = mulberry32(seed * 31);
      const variantField = permutedField(field, shuffleRng);
      const variantSets: DimSets = {
        anchored: shuffledSet(sets.anchored, shuffleRng),
        immovableStations: shuffledSet(sets.immovableStations, shuffleRng),
        immovableShots: shuffledSet(sets.immovableShots, shuffleRng),
      };
      expect(seededDiff(seed, 'fresh draw, permuted field', reference, freshDraw(data, variantField, variantSets))).toBeNull();

      // A live controller that first drew ANOTHER field, then got this one: the
      // color-only fast path + the dim recompute must land on the same bytes.
      const { map } = makeFakeMap({ visibility: {} });
      const live = new SurveyCustomController(map);
      const before = randomField(mulberry32(seed + 1_000_003), data);
      live.sync(syncParams(data, before));
      live.setDimming(dimState(data, before, randomDimSets(rng, data)));
      live.sync(syncParams(data, variantField));
      live.setDimming(dimState(data, variantField, variantSets));
      expect(seededDiff(seed, 'live re-sync vs fresh draw', reference, drawn(fake.last!.data as CustomRenderData))).toBeNull();
      live.dispose();
      sawGasDim ||= reference.circleDim.includes(GAS_DIM_LEVEL);
    }
    // Not vacuous: the beyond-reach dim really reached the drawn bytes.
    expect(sawGasDim).toBe(true);
  });
});
