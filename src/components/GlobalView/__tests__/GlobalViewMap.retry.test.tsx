/**
 * #576 D10 / NF-AN6-1: the Global View retries a system whose map geometry
 * failed to load only when the user asks.
 *
 * `gpsProjects` was a fresh `.filter()` on every render, and it is a dependency
 * of the lazy-load effect, which loads the visible systems on every run - and the
 * store's batch loader re-requested systems in 'error'. A failing system set
 * 'loading' then 'error', each a store write that re-rendered the map, which
 * re-ran the effect, which retried it: an unbounded loop that re-read the whole
 * file from disk in Rust each time, for as long as the system was on screen.
 *
 * Decided ("Retry on user action only"): a failed system stays an error marker
 * until the user clicks it, reloads the recent list, or toggles the view mode.
 *
 * D10 proper: the marker effect depended on the loading states, the selection and
 * the parent's per-render callbacks, so every system load tore down and rebuilt
 * every marker's DOM (20 systems loading created 820 markers). Markers are now
 * created once per system and restyled in place.
 *
 * Component tier: the real GlobalViewMap and globalViewStore, with maplibre-gl,
 * the WebGL survey layer and the IPC bridge replaced by fakes.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { useMemo, useState } from 'react';

vi.mock('maplibre-gl', async () => (await import('../../../test/factories/globalViewMap')).maplibreModule);
vi.mock('../GlobalSurveyLayer', async () =>
  (await import('../../../test/factories/globalViewMap')).globalSurveyLayerModule,
);
vi.mock('../../../utils/ipc');

import GlobalViewMap from '../GlobalViewMap';
import { useGlobalViewStore } from '../../../store/globalViewStore';
import type { ProjectMetadata } from '../../../types/project';
import { testIdPart } from '../../../utils/testId';
import { useShallow } from 'zustand/react/shallow';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { fakeGlobalView as h } from '../../../test/factories/globalViewMap';

const GOOD = '/caves/CaveA.stalx';
const BROKEN = '/caves/CaveB.stalx';

const project = (file_path: string, lng: number): ProjectMetadata =>
  ({
    file_path,
    name: file_path.slice(7, -6),
    gps_center: [lng, 20],
    total_length: 100,
    station_count: 10,
  }) as unknown as ProjectMetadata;

const GEOMETRY = { origin: [0, 20], shots: [] };

/** Geometry IPC calls per file. */
function geometryCalls(path: string): number {
  return invokeIpcMock.mock.calls.filter(
    ([cmd, args]) => cmd === 'get_survey_geometry_for_file' && (args as { path: string }).path === path
  ).length;
}

const twoSystems = () => [project(GOOD, 10), project(BROKEN, 11)];
let harnessProjects: () => ProjectMetadata[] = twoSystems;

/** Subscribes to the store the way GlobalView does and can reload the recent list. */
let reloadRecentList: () => void = () => {};
function Harness() {
  const { geometryData, geometryStates } = useGlobalViewStore(
    useShallow((s) => ({ geometryData: s.geometryData, geometryStates: s.geometryStates })),
  );
  const [generation, setGeneration] = useState(0);
  reloadRecentList = () => setGeneration((g) => g + 1);
  // A reload yields a NEW array with the same entries, as loadRecentProjects does.
  const projects = useMemo(() => harnessProjects(), [generation]);
  return (
    <GlobalViewMap
      projects={projects}
      selectedProjectId={null}
      onProjectSelect={() => {}}
      onProjectOpen={() => {}}
      geometryData={geometryData}
      geometryStates={geometryStates}
    />
  );
}

/**
 * Let the queued IPC promises and the store writes they cause settle. Plain
 * wall-clock waiting rather than `act`: on the looping code `act` would keep
 * flushing the endless re-render chain and never return, where a fixed window
 * lets the test COUNT the calls the loop made.
 */
async function settle(ms = 300) {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

beforeEach(() => {
  harnessProjects = twoSystems;
  h.state.map = null;
  h.state.markersCreated = 0;
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string, args?: { path: string }) => {
    if (cmd !== 'get_survey_geometry_for_file') return undefined;
    // Resolve on a later task, as a real IPC round trip would.
    await new Promise((resolve) => setTimeout(resolve, 1));
    if (args?.path === BROKEN) throw 'No GPS reference for this survey';
    return GEOMETRY;
  });
  useGlobalViewStore.getState().clearGeometryCache();
  try {
    localStorage.clear();
  } catch {
    // jsdom storage may be unavailable; the view mode then defaults to survey.
  }
});

afterEach(() => {
  document.body.innerHTML = '';
});

async function mountAndLoad() {
  render(<Harness />);
  act(() => h.state.map?.fire('load'));
  await settle();
}

describe('Global View geometry retry (D10-D1 / NF-AN6-1)', () => {
  it('loads a failing system once, not in a loop, and leaves it an error marker', async () => {
    await mountAndLoad();
    expect(geometryCalls(GOOD), 'loads of the good system').toBe(1);
    expect(geometryCalls(BROKEN), 'loads of the failing system within 300 ms').toBe(1);
    expect(useGlobalViewStore.getState().geometryStates.get(BROKEN)?.state).toBe('error');
    // A map move does not retry it either.
    act(() => h.state.map?.fire('moveend'));
    await settle();
    expect(geometryCalls(BROKEN)).toBe(1);
  });

  it('retries once when the user clicks the error marker', async () => {
    await mountAndLoad();
    const before = geometryCalls(BROKEN);
    fireEvent.click(screen.getByTestId(`system-marker-${testIdPart(BROKEN)}`));
    await settle();
    expect(geometryCalls(BROKEN) - before).toBe(1);
    expect(geometryCalls(GOOD)).toBe(1);
  });

  it('retries once when the recent list reloads', async () => {
    await mountAndLoad();
    const before = geometryCalls(BROKEN);
    act(() => reloadRecentList());
    await settle();
    expect(geometryCalls(BROKEN) - before).toBe(1);
    expect(geometryCalls(GOOD)).toBe(1);
  });

  it('retries once when the view mode is toggled back to survey', async () => {
    await mountAndLoad();
    const before = geometryCalls(BROKEN);
    fireEvent.click(screen.getByTestId('view-mode-toggle')); // -> markers
    await settle();
    fireEvent.click(screen.getByTestId('view-mode-toggle')); // -> survey
    await settle();
    expect(geometryCalls(BROKEN) - before).toBe(1);
    expect(geometryCalls(GOOD)).toBe(1);
  });
});

describe('Global View markers are created once per system (D10)', () => {
  it('does not rebuild the marker DOM while systems load', async () => {
    harnessProjects = () => Array.from({ length: 20 }, (_, i) => project(`/caves/Cave${i}.stalx`, i));
    await mountAndLoad();
    const loadedCount = () =>
      [...useGlobalViewStore.getState().geometryStates.values()].filter((s) => s.state === 'loaded').length;
    for (let waited = 0; loadedCount() < 20 && waited < 3000; waited += 100) await settle(100);
    expect(h.state.markersCreated, 'markers created while 20 systems loaded').toBe(20);
    expect(loadedCount()).toBe(20);
  });

  it('keeps each marker current in place: no spinner after the load, error state on the failed one', async () => {
    await mountAndLoad();
    const broken = screen.getByTestId(`system-marker-${testIdPart(BROKEN)}`);
    const good = screen.getByTestId(`system-marker-${testIdPart(GOOD)}`);
    expect(broken.dataset.loadingState).toBe('error');
    expect(good.dataset.loadingState).toBe('loaded');
    expect(broken.querySelector('.marker-spinner')).toBeNull();
    expect(good.querySelector('.marker-spinner')).toBeNull();
    expect(h.state.markersCreated).toBe(2);
  });
});
