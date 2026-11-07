import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  waitForNoDialogs,
  seedProjectWithSections,
  waitForDrawnStations,
} from './helpers/app';

/**
 * Offline-first (#576 C12): the map must not need the network. Its stylesheet
 * used to come from unpkg.com (a 4.7.1 copy against the bundled 5.16.0), so on a
 * machine without a cached copy the map canvas, controls and DOM markers lost
 * their positioning rules - a Plan-mode or distance-origin marker landed far off
 * its station. Every non-localhost request is cut off here, as offline.
 */
test.describe('Map works with the network cut off', () => {
  test('makes no external request and still positions its markers', async ({ page }) => {
    const external: string[] = [];
    await page.route(
      (url) => url.protocol.startsWith('http') && url.hostname !== 'localhost' && url.hostname !== '127.0.0.1',
      (route) => {
        external.push(route.request().url());
        return route.abort();
      },
    );

    await setupMockedPage(page);
    await createProject(page, 'Offline Map');
    await waitForNoDialogs(page);
    await seedProjectWithSections(page, [
      {
        name: 'CaveA Main',
        direction: 'in',
        shots: [
          { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -1, depth_out: -3 },
          { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -3, depth_out: -6 },
        ],
      },
    ]);
    const stations = await waitForDrawnStations(page, 3);
    const origin = stations.find((s) => s.stationIndex === 1)!;

    // The distance-gradient origin ring is a MapLibre DOM marker.
    await page.evaluate((ref) => {
      const ui = (window as unknown as {
        __E2E_TEST__: { uiStore: { getState: () => {
          setColorMode: (m: string) => void;
          setDistanceGradientOrigin: (r: { section_id: number; station_index: number }) => void;
        } } };
      }).__E2E_TEST__.uiStore.getState();
      ui.setColorMode('distance');
      ui.setDistanceGradientOrigin(ref);
    }, { section_id: origin.sectionId, station_index: origin.stationIndex });
    await expect(page.getByTestId('distance-origin-marker')).toHaveCount(1, { timeout: 10000 });

    const placed = await page.evaluate(() => {
      const map = (window as unknown as { __stalagmight_map__: { getCanvas: () => HTMLCanvasElement } }).__stalagmight_map__;
      const canvas = map.getCanvas().getBoundingClientRect();
      const marker = document.querySelector('[data-testid="distance-origin-marker"]')!.getBoundingClientRect();
      return {
        x: marker.left + marker.width / 2 - canvas.left,
        y: marker.top + marker.height / 2 - canvas.top,
        canvasPosition: getComputedStyle(map.getCanvas()).position,
      };
    });
    // The marker sits on its station (canvas-relative), not wherever static flow put it.
    const current = (await waitForDrawnStations(page, 3)).find((s) => s.stationIndex === 1)!;
    expect(Math.hypot(placed.x - current.x, placed.y - current.y)).toBeLessThan(3);
    expect(placed.canvasPosition).toBe('absolute');
    expect(external).toEqual([]);
  });
});
