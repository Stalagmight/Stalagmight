/**
 * One custom-renderer rebuild per survey-data change (#576 NF-W13FE-1).
 *
 * Every survey-data change rebuilt the map three times: the coalesced update() flush
 * (useSurveyLayerSync's layout effect), plus setDistanceGradient(false, null) and
 * setGasReach(false, null) from useAnalyticalOverlays' passive effects, each of which
 * re-synced the custom renderer from the store even with both overlays off and
 * unchanged. On the synthetic 384k-shot Walls import those two re-syncs were about
 * 45% of the remaining JS time.
 *
 * Drives the two REAL hooks over a REAL SurveyLayer (fake map, surveyLayerFakeMap.ts)
 * with the store changes as the app makes them; the renderer's build entry points
 * (`customRender.sync`, `customLabels.sync`) are counted, not run (no WebGL under node).
 * Each change must also leave exactly what a fresh rebuild from the current state
 * builds (the same rendered state as before the fix, which ended on such a rebuild).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useMemo, type MutableRefObject } from 'react';

vi.mock('../../../utils/ipc');
vi.mock('maplibre-gl', () => ({ default: {} }));

import SurveyLayer from '../SurveyLayer';
import { useSurveyLayerSync } from '../useSurveyLayerSync';
import { useAnalyticalOverlays } from '../useAnalyticalOverlays';
import { makeFakeSurveyMap } from './surveyLayerFakeMap';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { selectActiveStations } from '../../../store/geometrySelectors';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import { makeProject, makeSection, makeShot, makeStation } from '../../../test/factories/survey';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { actMicrotasks } from '../../../test/factories/act';
import type { DistanceField, Station } from '../../../types/project';
import type { GasReachField } from '../../../types/ipc.generated';

type SyncOptions = Parameters<typeof useSurveyLayerSync>[0];
type OverlayOptions = Parameters<typeof useAnalyticalOverlays>[0];

interface Overlays {
  showDistanceGradient: boolean;
  distanceField: DistanceField | null;
  showGasPlan: boolean;
  gasReachField: GasReachField | null;
}
const OFF: Overlays = { showDistanceGradient: false, distanceField: null, showGasPlan: false, gasReachField: null };

/** A synthetic project: `sections` sections of `shots` shots, `version` nudging every shot length. */
function build(sections: number, shots: number, version: number) {
  const secs = Array.from({ length: sections }, (_, s) =>
    makeSection({
      id: s + 1,
      name: `SectionA${s + 1}`,
      shots: Array.from({ length: shots }, (_, i) =>
        makeShot({ id: i + 1, azimuth_in: (s * 37 + i * 11) % 360, length: 3 + ((i + version) % 5) }),
      ),
    }),
  );
  const stations: Station[] = secs.flatMap((sec) =>
    Array.from({ length: sec.shots.length + 1 }, (_, i) =>
      makeStation({
        station_ref: { section_id: sec.id, station_index: i },
        coordinates: { x: sec.id * 10 + i * (3 + version), y: i * 2, z: -i },
      }),
    ),
  );
  return {
    project: makeProject({ sections: secs, modified_at: `2026-01-01T00:00:0${version}Z` }),
    stations,
    surveyData: surveyDataFromLegacy(secs, stations),
  };
}

const distanceField = (stations: Station[]): DistanceField =>
  ({
    station_refs: stations.map((s) => s.station_ref),
    distances: stations.map((_, i) => i * 1.5),
    max_distance: stations.length * 1.5,
  }) as unknown as DistanceField;

const gasField = (stations: Station[]): GasReachField =>
  ({
    station_refs: stations.map((s) => s.station_ref),
    gas_to_reach: stations.map((_, i) => i * 10),
    reachable: stations.map((_, i) => i % 7 !== 0),
    turn_frontier: [],
    max_reachable_gas: stations.length * 10,
    budget_litres: stations.length * 5,
    route: [],
    waypoint_route_indices: [],
    stage_drop_refs: [],
    final_turn_ref: null,
  }) as unknown as GasReachField;

const STABLE = {
  toolCallbacks: {},
  anchored: new Set<number>(),
  immovable: { stationIds: new Set<number>(), shotIds: new Set<number>() },
  depthRange: { min: 0, max: 50 },
  severityWarnings: [],
  gasComputeGate: { skip: false, origin: { section_id: 1, station_index: 0 }, endpoint: null },
};

/** What MapCanvas wires: the publish (useSurveyLayerSync) and the overlays (useAnalyticalOverlays). */
function useMapWiring(layerRef: MutableRefObject<SurveyLayer | null>, o: Overlays): void {
  const project = useProjectStore((s) => s.project);
  const stations = useProjectStore((s) => s.stations);
  const surveyData = useProjectStore((s) => s.surveyData);
  const stationsAdjusted = useProjectStore((s) => s.stationsAdjusted);
  const surveyDataAdjusted = useProjectStore((s) => s.surveyDataAdjusted);
  const geometryMode = useUIStore((s) => s.geometryMode);
  const activeStations = useMemo(
    () => selectActiveStations({ stations, surveyData, stationsAdjusted, surveyDataAdjusted }, geometryMode),
    [stations, surveyData, stationsAdjusted, surveyDataAdjusted, geometryMode],
  );
  const map = { current: null };
  useSurveyLayerSync({
    map, surveyLayerRef: layerRef, mapLoaded: true, styleEpoch: 0, project, activeStations, gpsOrigin: null,
    effectiveDepthGradient: false, depthRange: STABLE.depthRange, precision: undefined, geometryMode, surveyDataAdjusted,
    effectiveIssueSeverity: false, severityWarnings: STABLE.severityWarnings, toolCallbacks: STABLE.toolCallbacks,
    config: null, resolvedTheme: 'dark', selection: null, snappedStation: null, mergeStationA: null, mergeStationB: null,
    showShotData: false, showShots: true, showSectionNames: false, showStationLabels: false, showStationComments: false,
    showStations: true, showShotComments: false, showReferenceShots: true, showJumps: true, moveCommentSource: null,
    anchoredSectionIds: STABLE.anchored, immovableFeatureIds: STABLE.immovable,
  } as unknown as SyncOptions);
  useAnalyticalOverlays({
    map, surveyLayerRef: layerRef, mapLoaded: true, project, config: null, surveyData, activeStations, gpsOrigin: null,
    effectiveIssueSeverity: false, showDistanceGradient: o.showDistanceGradient, distanceGradientOrigin: null,
    distanceField: o.distanceField, setDistanceField: vi.fn(), showGasPlan: o.showGasPlan, gasOriginChosen: o.showGasPlan,
    gasOrigin: null, gasComputeGate: STABLE.gasComputeGate, gasWaypoints: [], gasSegments: [], gasDiver: {},
    gasCylinders: [], gasPlanConfig: {}, gasReachField: o.gasReachField, setGasReachField: vi.fn(),
  } as unknown as OverlayOptions);
}

let layer: SurveyLayer;
let geometrySync: ReturnType<typeof vi.fn>;
let labelSync: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  // The overlay field fetches stay in flight: the fields come from the test.
  invokeIpcMock.mockReset();
  invokeIpcMock.mockReturnValue(new Promise(() => {}));
  layer = new SurveyLayer(makeFakeSurveyMap().map);
  const targets = layer as unknown as { customRender: { sync: () => void }; customLabels: { sync: () => void } };
  geometrySync = vi.spyOn(targets.customRender, 'sync').mockImplementation(() => {}) as unknown as ReturnType<typeof vi.fn>;
  labelSync = vi.spyOn(targets.customLabels, 'sync').mockImplementation(() => {}) as unknown as ReturnType<typeof vi.fn>;
  useUIStore.setState({ geometryMode: 'measured' });
  const first = build(6, 8, 0);
  useProjectStore.setState({ ...first, stationsAdjusted: null, surveyDataAdjusted: null });
});

afterEach(() => {
  layer.release();
  vi.restoreAllMocks();
});

/** Mount the wiring with `overlays`, settle, and forget the mount's own builds. */
async function mount(overlays: (s: Station[]) => Overlays) {
  const ref = { current: layer };
  const hook = renderHook((o: Overlays) => useMapWiring(ref, o), {
    initialProps: overlays(useProjectStore.getState().stations),
  });
  await actMicrotasks(4);
  geometrySync.mockClear();
  labelSync.mockClear();
  return hook;
}

/** A survey-data change as projectStore applies one (refreshProjectState / loadProject). */
async function change(version: number, rerender?: (o: Overlays) => void, overlays?: (s: Station[]) => Overlays) {
  const next = build(6, 8, version);
  act(() => {
    useProjectStore.setState({ ...next, stationsAdjusted: null, surveyDataAdjusted: null });
    if (rerender && overlays) rerender(overlays(next.stations));
  });
  await actMicrotasks(4);
}

/** The last build equals what a fresh rebuild from the current store + overlay state builds. */
function expectLastBuildIsCurrent(): void {
  const last = (spy: ReturnType<typeof vi.fn>) => spy.mock.calls[spy.mock.calls.length - 1];
  const lastGeometry = last(geometrySync);
  const lastLabels = last(labelSync);
  (layer as unknown as { resyncCustomRenderFromStore(): void }).resyncCustomRenderFromStore();
  expect(lastGeometry).toEqual(last(geometrySync));
  expect(lastLabels).toEqual(last(labelSync));
  const params = last(geometrySync)![0] as { surveyData: unknown };
  expect(params.surveyData).toBe(useProjectStore.getState().surveyData);
}

describe('one custom-render rebuild per survey-data change (#576 NF-W13FE-1)', () => {
  it('overlays off: one geometry build and one label build per change', async () => {
    await mount(() => OFF);
    for (const version of [1, 2, 3]) {
      geometrySync.mockClear();
      labelSync.mockClear();
      await change(version);
      expect(geometrySync, `change ${version}: geometry builds`).toHaveBeenCalledTimes(1);
      expect(labelSync, `change ${version}: label builds`).toHaveBeenCalledTimes(1);
      expectLastBuildIsCurrent();
    }
  });

  it('distance gradient on: one build per change, in the new field colours', async () => {
    const on = (s: Station[]): Overlays => ({ ...OFF, showDistanceGradient: true, distanceField: distanceField(s) });
    const { rerender } = await mount(on);
    await change(1, rerender, on);
    expect(geometrySync).toHaveBeenCalledTimes(1);
    const ctx = (geometrySync.mock.calls[0]![0] as { colorCtx: { overlay: { mode: string } | null } }).colorCtx;
    expect(ctx.overlay).toMatchObject({ mode: 'distance' });
    expectLastBuildIsCurrent();
  });

  it('gas reach on (Plan mode): one build per change, in the new field colours', async () => {
    const on = (s: Station[]): Overlays => ({ ...OFF, showGasPlan: true, gasReachField: gasField(s) });
    const { rerender } = await mount(on);
    await change(1, rerender, on);
    expect(geometrySync).toHaveBeenCalledTimes(1);
    const ctx = (geometrySync.mock.calls[0]![0] as { colorCtx: { overlay: { mode: string } | null } }).colorCtx;
    expect(ctx.overlay).toMatchObject({ mode: 'gasReach' });
    expectLastBuildIsCurrent();
  });

  it('a geometry-mode switch still rebuilds once, from the adjusted geometry', async () => {
    await mount(() => OFF);
    const adjusted = build(6, 8, 7);
    act(() => {
      useProjectStore.setState({ stationsAdjusted: adjusted.stations, surveyDataAdjusted: adjusted.surveyData });
      useUIStore.setState({ geometryMode: 'closureAdjusted' });
    });
    await actMicrotasks(4);
    expect(geometrySync).toHaveBeenCalledTimes(1);
    expect((geometrySync.mock.calls[0]![0] as { surveyData: unknown }).surveyData).toBe(adjusted.surveyData);
  });
});

describe('SurveyLayer overlay setters (#576 NF-W13FE-1)', () => {
  it('an unchanged overlay does not rebuild; a real change rebuilds once, also with no flush queued', async () => {
    await mount(() => OFF);
    layer.setDistanceGradient(false, null);
    layer.setGasReach(false, null);
    expect(geometrySync).not.toHaveBeenCalled();
    const state = { stations: new Map<number, number>(), shots: new Map<number, number>(), max: 1 };
    layer.setDistanceGradient(true, state as never);
    expect(geometrySync).toHaveBeenCalledTimes(1);
    layer.setDistanceGradient(true, state as never);
    expect(geometrySync).toHaveBeenCalledTimes(1);
    layer.setDistanceGradient(false, null);
    expect(geometrySync).toHaveBeenCalledTimes(2);
    expectLastBuildIsCurrent();
  });
});
