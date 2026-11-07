/**
 * #576 W71-NF10: the Global View map's imagery attribution keeps the import parse
 * indicator ("Reading <file>...") clear of the source names, as the project
 * view's does (4239f21ec). The indicator is centred on the window, and reaches
 * over the attribution in the map's bottom-right corner.
 *
 * Pinned here: the attribution registers its clearance for as long as it is on
 * screen, and the registration goes with it - when the map has no system to
 * show (its empty state draws no imagery) and when the map unmounts (the list
 * goes back to list mode, or the Global View is left). Where the indicator then
 * sits is pinned in e2e-playwright/import-parse-indicator.spec.ts.
 *
 * Component tier: the real GlobalViewMap, MapAttribution and uiStore, with
 * maplibre-gl and the WebGL survey layer replaced by fakes.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from '@testing-library/react';

vi.mock('maplibre-gl', async () => (await import('../../../test/factories/globalViewMap')).maplibreModule);
vi.mock('../GlobalSurveyLayer', async () =>
  (await import('../../../test/factories/globalViewMap')).globalSurveyLayerModule,
);
vi.mock('../../../utils/ipc');

import GlobalViewMap from '../GlobalViewMap';
import type { ProjectMetadata } from '../../../types/project';
import { useUIStore } from '../../../store/uiStore';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { fakeGlobalView as h } from '../../../test/factories/globalViewMap';

const OWNER = 'global-view-map-attribution';

const project = (file_path: string): ProjectMetadata =>
  ({
    file_path,
    name: file_path.slice(7, -6),
    gps_center: [10, 20],
    total_length: 100,
    station_count: 10,
  }) as unknown as ProjectMetadata;

function mapWith(projects: ProjectMetadata[]) {
  return (
    <GlobalViewMap projects={projects} selectedProjectId={null} onProjectSelect={() => {}} onProjectOpen={() => {}} />
  );
}

const registered = () => OWNER in useUIStore.getState().parseIndicatorBottomOffsets;

beforeEach(() => {
  h.state.map = null;
  invokeIpcMock.mockImplementation(async () => undefined);
  useUIStore.setState({ parseIndicatorBottomOffsets: {} });
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('the Global View map attribution clears the import parse indicator (#576 W71-NF10)', () => {
  it('registers while the attribution is on screen and unregisters when the map unmounts', () => {
    const { unmount } = render(mapWith([project('/caves/CaveA.stalx')]));
    expect(document.querySelector('[data-testid="map-attribution"]')).not.toBeNull();
    expect(registered(), 'registered while the attribution is up').toBe(true);

    unmount();
    expect(registered(), 'still registered after the map unmounted').toBe(false);
  });

  it('unregisters when the map shows its empty state, and registers again when a system returns', () => {
    const { rerender } = render(mapWith([project('/caves/CaveA.stalx')]));
    expect(registered()).toBe(true);

    rerender(mapWith([]));
    expect(document.querySelector('[data-testid="map-attribution"]')).toBeNull();
    expect(registered(), 'still registered with no attribution on screen').toBe(false);

    rerender(mapWith([project('/caves/CaveA.stalx')]));
    expect(registered(), 'registered again once the attribution is back').toBe(true);
  });
});
