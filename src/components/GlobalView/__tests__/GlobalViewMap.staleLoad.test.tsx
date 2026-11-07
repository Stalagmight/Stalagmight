/**
 * #576 D17: the Global View map's `load` handler read the systems list it closed
 * over when the map was created, on mount.
 *
 * MapLibre fires `load` asynchronously, once the style is ready. A system that
 * arrived in between (the recent list grew, a reload landed) had its geometry
 * buffered by the geometry effect, but the handler applied the buffer only for
 * the mount-time systems and then cleared it - so the newcomer's survey never
 * reached the layer (nothing re-applies it until the geometry changes again),
 * and the first fit framed the mount-time systems only. The handler now reads
 * the systems and the bounds through useLatest refs.
 *
 * Component tier: the real GlobalViewMap with maplibre-gl, the WebGL survey layer
 * and the IPC bridge replaced by fakes (as in GlobalViewMap.retry.test.tsx).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/react';

vi.mock('maplibre-gl', async () => (await import('../../../test/factories/globalViewMap')).maplibreModule);
vi.mock('../GlobalSurveyLayer', async () =>
  (await import('../../../test/factories/globalViewMap')).globalSurveyLayerModule,
);
vi.mock('../../../utils/ipc');

import GlobalViewMap from '../GlobalViewMap';
import { useGlobalViewStore } from '../../../store/globalViewStore';
import type { ProjectMetadata } from '../../../types/project';
import type { SurveyGeometry } from '../../../types/survey';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { fakeGlobalView as h } from '../../../test/factories/globalViewMap';

const CAVE_A = '/caves/CaveA.stalx';
const CAVE_B = '/caves/CaveB.stalx';

const project = (file_path: string, lng: number): ProjectMetadata =>
  ({
    file_path,
    name: file_path.slice(7, -6),
    gps_center: [lng, 20],
    total_length: 100,
    station_count: 10,
  }) as unknown as ProjectMetadata;

const geometry = { origin: [0, 20], shots: [] } as unknown as SurveyGeometry;

function view(projects: ProjectMetadata[], geometryData: Map<string, SurveyGeometry>) {
  return (
    <GlobalViewMap
      projects={projects}
      selectedProjectId={null}
      onProjectSelect={() => {}}
      onProjectOpen={() => {}}
      geometryData={geometryData}
    />
  );
}

beforeEach(() => {
  h.state.map = null;
  h.state.updates = [];
  h.state.fits = [];
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(undefined);
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

describe('Global View map load handler (#576 D17)', () => {
  it('applies the geometry and the first fit for the systems as they are at load, not at mount', () => {
    const { rerender } = render(view([project(CAVE_A, 10)], new Map([[CAVE_A, geometry]])));
    // A second system (and its geometry) arrives before the style has loaded.
    rerender(
      view(
        [project(CAVE_A, 10), project(CAVE_B, 30)],
        new Map([
          [CAVE_A, geometry],
          [CAVE_B, geometry],
        ])
      )
    );
    expect(h.state.updates, 'nothing reaches the layer before it exists').toEqual([]);

    act(() => h.state.map?.fire('load'));

    expect(h.state.updates[0], 'the buffered geometry applied at load').toEqual([CAVE_A, CAVE_B]);
    expect(h.state.fits[0]?.east, 'the first fit reaches the system at lng 30').toBeGreaterThanOrEqual(30);
  });
});
