/**
 * Global View map: what the arrow-key navigation and the marker tooltips read.
 *
 * - #576 NF-W3UI-4: `getSystemCentroids` (the arrow keys' system positions)
 *   indexed every shot endpoint unguarded. The geometry is IPC data typed as
 *   `Array<number>` and the default props were untyped Maps, so nothing checked
 *   it: one endpoint without two finite numbers turned the system's centroid into
 *   NaN, and arrow-key navigation could never reach that system.
 *
 * - #576 RV3-FE-NITS (tooltip precision): since D10 the markers are built once per
 *   system set, and the hover tooltip formatted the length with the decimal
 *   precision captured when the marker was BUILT, so changing the length
 *   precision in Settings did not reach the Global View tooltips until the recent
 *   list reloaded.
 *
 * Component tier: the real GlobalViewMap, with maplibre-gl and the WebGL survey
 * layer replaced by fakes.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { createRef } from 'react';

vi.mock('maplibre-gl', async () => (await import('../../../test/factories/globalViewMap')).maplibreModule);
vi.mock('../GlobalSurveyLayer', async () =>
  (await import('../../../test/factories/globalViewMap')).globalSurveyLayerModule,
);
vi.mock('../../../utils/ipc');
invokeIpcMock.mockImplementation(async () => undefined);

import GlobalViewMap, { type GlobalViewMapHandle } from '../GlobalViewMap';
import type { ProjectMetadata } from '../../../types/project';
import type { SurveyGeometry } from '../../../types/survey';
import { useProjectStore } from '../../../store/projectStore';
import { defaultConfig, type Config } from '../../../types/config';
import { testIdPart } from '../../../utils/testId';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { fakeGlobalView as h } from '../../../test/factories/globalViewMap';

function setLengthPrecision(length: number): void {
  const config = defaultConfig as Config;
  useProjectStore.setState({
    config: { ...config, units: { ...config.units, decimal_precision: { ...config.units.decimal_precision, length } } },
  });
}

const project = (file_path: string, gps_center: [number, number]): ProjectMetadata =>
  ({
    file_path,
    name: file_path.slice(7, -6),
    gps_center,
    total_length: 1234.5678,
    station_count: 10,
  }) as unknown as ProjectMetadata;

const shot = (from: number[], to: number[]) => ({ from, to, kind: 'Normal', section_id: 1 });

beforeEach(() => {
  h.state.map = null;
  try {
    localStorage.clear();
  } catch {
    // jsdom storage may be unavailable; the view mode then defaults to survey.
  }
});

afterEach(() => {
  document.body.innerHTML = '';
  useProjectStore.setState({ config: null });
});

describe('Global View system centroids (#576 NF-W3UI-4)', () => {
  it('skips endpoints without two finite coordinates instead of turning the centroid into NaN', () => {
    const A = '/caves/CaveA.stalx';
    const B = '/caves/CaveB.stalx';
    const geometry = new Map<string, SurveyGeometry>([
      // Two good endpoints, one empty and one NaN endpoint.
      [A, { origin: [10, 20], shots: [shot([10, 20], [11, 21]), shot([], [Number.NaN, 5])] } as unknown as SurveyGeometry],
      // Nothing usable at all: fall back to the system's GPS center.
      [B, { origin: [30, 40], shots: [shot([1], [Number.POSITIVE_INFINITY, 2])] } as unknown as SurveyGeometry],
    ]);
    const ref = createRef<GlobalViewMapHandle>();
    render(
      <GlobalViewMap
        ref={ref}
        projects={[project(A, [10.4, 20.4]), project(B, [30, 40])]}
        selectedProjectId={null}
        onProjectSelect={() => {}}
        onProjectOpen={() => {}}
        geometryData={geometry}
      />
    );
    act(() => h.state.map?.fire('load'));
    const centroids = ref.current!.getSystemCentroids();
    expect(centroids.get(A)).toEqual([10.5, 20.5]);
    expect(centroids.get(B)).toEqual([30, 40]);
  });
});

describe('Global View marker tooltip (#576 RV3-FE-NITS)', () => {
  it('formats the length with the precision current when it is shown, not when the marker was built', () => {
    const A = '/caves/CaveA.stalx';
    setLengthPrecision(1);
    render(
      <GlobalViewMap
        projects={[project(A, [10, 20])]}
        selectedProjectId={null}
        onProjectSelect={() => {}}
        onProjectOpen={() => {}}
      />
    );
    act(() => h.state.map?.fire('load'));
    const marker = screen.getByTestId(`system-marker-${testIdPart(A)}`);
    const tooltip = () => document.querySelector('.globalview-marker-tooltip')?.textContent ?? '';

    fireEvent.mouseEnter(marker);
    expect(tooltip()).toContain('1.2 km');
    fireEvent.mouseLeave(marker);

    // Settings -> length precision 3. The markers are not rebuilt for it.
    act(() => setLengthPrecision(3));
    fireEvent.mouseEnter(marker);
    expect(tooltip()).toContain('1.235 km');
  });
});
