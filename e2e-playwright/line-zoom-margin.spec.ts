/**
 * E2E tests for line zoom margin behavior.
 *
 * Verifies that zooming to a line leaves visible margin from viewport edges
 * and the zoom level stays within a reasonable range. Tests both the
 * double-click trigger (sections panel row) and the center button trigger
 * (detail panel header).
 *
 * Key pitfalls addressed:
 * - Pan map off-target before each zoom trigger to avoid false passes
 * - Assert precondition that margins are invalid before triggering zoom
 * - Wait for __stalagmight_map__ after canvas appears
 * - Use map.project() for screen-space assertions
 * - Use expect.poll() for all state change assertions
 * - Read zoom_max from config instead of hard-coding
 */

import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapIdle,
  waitForNoDialogs,
  type LineSeed,
} from './helpers/app';

/**
 * Multi-station line with distinct geometry to produce
 * a non-trivial bounding box on the map.
 */
const lineSeeds: LineSeed[] = [
  {
    name: 'Zoom Test Passage',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 15, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 20, depth_in: -3, depth_out: -6 },
      { azimuth_in: 135, azimuth_out: 315, length: 12, depth_in: -6, depth_out: -8 },
    ],
  },
  {
    name: 'Side Gallery',
    direction: 'out',
    shots: [
      { azimuth_in: 270, azimuth_out: 90, length: 8, depth_in: -2, depth_out: -4 },
    ],
  },
];

/** Minimum margin percentage for all assertions. */
const MIN_MARGIN_PCT = 5;

/**
 * Margin check result from page.evaluate().
 * Points are retained for debugging in failure messages.
 */
interface MarginResult {
  points: Array<{ x: number; y: number }>;
  containerWidth: number;
  containerHeight: number;
  zoom: number;
  zoomMax: number;
  minMarginLeftPct: number;
  minMarginRightPct: number;
  minMarginTopPct: number;
  minMarginBottomPct: number;
}

/**
 * Setup: create project, seed lines, wait for map.
 */
async function createProjectWithLines(page: Page): Promise<void> {
  await createProject(page, 'Line Zoom Margin Test');
  await seedProjectWithLines(page, lineSeeds);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
}

/**
 * Get the ID of the first line by name from the project store.
 */
async function getLineIdByName(page: Page, name: string): Promise<number> {
  const id = await page.evaluate((lineName) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => { project: { lines: Array<{ id: number; name: string }> } | null };
        };
      };
    }).__E2E_TEST__;
    const line = stores?.projectStore.getState().project?.lines.find(
      (s) => s.name === lineName,
    );
    return line?.id ?? -1;
  }, name);
  expect(id).toBeGreaterThan(0);
  return id;
}

/**
 * Pan the map far off-target so the line is provably outside the viewport.
 * Uses a high zoom level (15) at a far-away center to ensure that a small
 * line near the GPS origin is completely off-screen rather than simply
 * tiny at a low zoom level.
 *
 * Memory pitfall: tests may pass falsely if map is already centered on the target line.
 */
async function panMapOffTarget(page: Page): Promise<void> {
  await page.evaluate(() => {
    const map = (window as unknown as {
      __stalagmight_map__?: {
        jumpTo: (opts: { center: [number, number]; zoom: number }) => void;
        stop: () => void;
      };
    }).__stalagmight_map__;
    if (map) {
      // Stop any running animation, then instantly jump far away at a zoom
      // level high enough that the target line is off-screen
      map.stop();
      map.jumpTo({ center: [0, 0], zoom: 15 });
    }
  });
  // Poll until map has settled (jumpTo is synchronous but render may lag)
  await expect.poll(
    async () => {
      return page.evaluate(() => {
        const m = (window as unknown as {
          __stalagmight_map__?: { isMoving: () => boolean };
        }).__stalagmight_map__;
        return m ? !m.isMoving() : false;
      });
    },
    { timeout: 5000 },
  ).toBe(true);
}

/**
 * Assert precondition: after panMapOffTarget, the line must NOT satisfy
 * the margin requirement. This prevents false passes where the line was
 * already visible before the zoom action was triggered.
 */
async function assertLineOffScreen(page: Page, lineId: number): Promise<void> {
  const result = await getStationScreenMargins(page, lineId);
  // Either null (map moving or data unavailable) or margins are violated
  if (result) {
    const allMarginsOk =
      result.minMarginLeftPct >= MIN_MARGIN_PCT &&
      result.minMarginRightPct >= MIN_MARGIN_PCT &&
      result.minMarginTopPct >= MIN_MARGIN_PCT &&
      result.minMarginBottomPct >= MIN_MARGIN_PCT;
    expect(allMarginsOk, 'Precondition failed: line should be off-screen after panMapOffTarget').toBe(false);
  }
  // null result also satisfies precondition (line data not visible)
}

/**
 * Project all station coordinates belonging to a line to screen pixels
 * using map.project(), and compute margin percentages from each viewport edge.
 * Returns null if data is not yet available or map is still moving (for use in polling).
 *
 * GPS origin comes from config.map.satellite_gps_origin in the project store.
 * zoom_max comes from config.map.zoom_max in the project store.
 */
async function getStationScreenMargins(page: Page, lineId: number): Promise<MarginResult | null> {
  return page.evaluate((secId) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            project: {
              lines: Array<{ id: number }>;
            } | null;
            stations: Array<{
              station_ref: { line_id: number };
              coordinates: { x: number; y: number };
            }>;
            config: {
              map: {
                satellite_gps_origin: { latitude: number; longitude: number };
                zoom_max: number;
              };
            } | null;
          };
        };
      };
    }).__E2E_TEST__;

    const map = (window as unknown as {
      __stalagmight_map__?: {
        project: (lngLat: [number, number]) => { x: number; y: number };
        getContainer: () => HTMLElement;
        getZoom: () => number;
        isMoving: () => boolean;
      };
    }).__stalagmight_map__;

    if (!stores || !map) return null;

    // Only measure when map is idle
    if (map.isMoving()) return null;

    const state = stores.projectStore.getState();
    const lineStations = state.stations.filter(
      (s) => s.station_ref.line_id === secId,
    );

    if (lineStations.length === 0) return null;

    // Get GPS origin from config (same source as MapCanvas.tsx line 74)
    const gpsOrigin = state.config?.map.satellite_gps_origin;
    if (!gpsOrigin) return null;

    // Read zoom_max from config instead of hard-coding
    const zoomMax = state.config?.map.zoom_max ?? 22;

    // Replicate metersToMapCoords inline (same algorithm as utils/coordinates.ts)
    const METERS_PER_DEG_LAT = 111_320;
    function metersToMapCoords(
      east: number,
      north: number,
      originLat: number,
      originLng: number,
    ): [number, number] {
      const metersPerDegLng = METERS_PER_DEG_LAT * Math.cos((originLat * Math.PI) / 180);
      const lng = originLng + east / metersPerDegLng;
      const lat = originLat + north / METERS_PER_DEG_LAT;
      return [lng, lat];
    }

    const container = map.getContainer();
    const containerWidth = container.clientWidth;
    const containerHeight = container.clientHeight;
    const zoom = map.getZoom();

    // Project all line stations to screen coordinates
    const points = lineStations.map((station) => {
      const lngLat = metersToMapCoords(
        station.coordinates.x,
        station.coordinates.y,
        gpsOrigin.latitude,
        gpsOrigin.longitude,
      );
      return map.project(lngLat);
    });

    // Compute minimum margin from each edge as percentage
    let minLeft = Infinity;
    let minRight = Infinity;
    let minTop = Infinity;
    let minBottom = Infinity;

    for (const p of points) {
      minLeft = Math.min(minLeft, p.x);
      minRight = Math.min(minRight, containerWidth - p.x);
      minTop = Math.min(minTop, p.y);
      minBottom = Math.min(minBottom, containerHeight - p.y);
    }

    return {
      points: points.map((p) => ({ x: Math.round(p.x), y: Math.round(p.y) })),
      containerWidth,
      containerHeight,
      zoom,
      zoomMax,
      minMarginLeftPct: (minLeft / containerWidth) * 100,
      minMarginRightPct: (minRight / containerWidth) * 100,
      minMarginTopPct: (minTop / containerHeight) * 100,
      minMarginBottomPct: (minBottom / containerHeight) * 100,
    };
  }, lineId);
}

/**
 * Poll until map is idle and station screen positions satisfy margin requirements.
 * Combines idle wait + margin check in a single poll to avoid separate
 * waitForMapIdle that can hang if the map animation never fully settles.
 *
 * Uses config-derived zoom_max instead of hard-coded value.
 */
async function pollForMarginsSettled(
  page: Page,
  lineId: number,
  minPct: number,
  timeout = 15000,
): Promise<MarginResult> {
  let lastResult: MarginResult | null = null;
  await expect.poll(
    async () => {
      const result = await getStationScreenMargins(page, lineId);
      if (!result) return false;
      lastResult = result;
      return (
        result.minMarginLeftPct >= minPct &&
        result.minMarginRightPct >= minPct &&
        result.minMarginTopPct >= minPct &&
        result.minMarginBottomPct >= minPct &&
        result.zoom <= result.zoomMax
      );
    },
    { timeout },
  )
    .toBe(true)
    // `message` on expect.poll is a plain string, so it cannot describe a result
    // that only exists once polling has given up. Compose the diagnostic here.
    // `cause` keeps the original failure attached: if the poll rejected for a
    // reason other than the timeout, the diagnostic below would otherwise
    // replace it with a margin story that was never the problem.
    .catch((cause: unknown) => {
      const r: MarginResult | null = lastResult;
      if (!r) {
        throw new Error(
          `No margin data available after ${timeout}ms (map still moving or no stations)`,
          { cause },
        );
      }
      throw new Error(
        [
          `Station screen positions did not settle with >= ${minPct}% margin and zoom <= ${r.zoomMax}`,
          `  Container: ${r.containerWidth}x${r.containerHeight}`,
          `  Zoom: ${r.zoom.toFixed(2)} (max: ${r.zoomMax})`,
          `  Margins: L=${r.minMarginLeftPct.toFixed(1)}% R=${r.minMarginRightPct.toFixed(1)}% T=${r.minMarginTopPct.toFixed(1)}% B=${r.minMarginBottomPct.toFixed(1)}%`,
          `  Points: ${JSON.stringify(r.points)}`,
        ].join('\n'),
        { cause },
      );
    });
  return lastResult!;
}

/**
 * Assert that all station screen positions have at least minPct% margin from
 * every viewport edge and zoom is within the config-derived zoom_max.
 */
function assertMargins(result: MarginResult, minPct: number): void {
  expect(result.minMarginLeftPct).toBeGreaterThanOrEqual(minPct);
  expect(result.minMarginRightPct).toBeGreaterThanOrEqual(minPct);
  expect(result.minMarginTopPct).toBeGreaterThanOrEqual(minPct);
  expect(result.minMarginBottomPct).toBeGreaterThanOrEqual(minPct);
  expect(result.zoom).toBeLessThanOrEqual(result.zoomMax);
}

test.describe('Line zoom margin', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProjectWithLines(page);
  });

  test('double-click on line row zooms with margin from viewport edges', async ({ page }) => {
    const lineId = await getLineIdByName(page, 'Zoom Test Passage');

    // Pan off-target first (memory pitfall: avoid false passes from pre-centered state)
    await panMapOffTarget(page);

    // Precondition: line must NOT satisfy margins after panning off-target
    await assertLineOffScreen(page, lineId);

    // Double-click the row: zooms/frames the line on the map (without entering
    // focus mode). Margins must hold against the current viewport.
    const lineRow = page.locator(`[data-testid="line-row-${lineId}"]`);
    await expect(lineRow).toBeVisible();
    await lineRow.dblclick();

    // Poll until map is idle and margins are correct (combined to avoid hanging)
    const result = await pollForMarginsSettled(page, lineId, MIN_MARGIN_PCT);
    assertMargins(result, MIN_MARGIN_PCT);
  });

  test('single-click on line row selects WITHOUT moving the map', async ({ page }) => {
    // #50 reverts #64: a single click only selects; it must not move the map.
    const lineId = await getLineIdByName(page, 'Zoom Test Passage');

    // Pan off-target and record the camera so we can prove the click leaves it.
    await panMapOffTarget(page);
    const before = await page.evaluate(() => {
      const m = (window as unknown as {
        __stalagmight_map__?: { getZoom: () => number; getCenter: () => { lng: number; lat: number } };
      }).__stalagmight_map__!;
      const c = m.getCenter();
      return { lng: c.lng, lat: c.lat, zoom: m.getZoom() };
    });

    const lineRow = page.locator(`[data-testid="line-row-${lineId}"]`);
    await expect(lineRow).toBeVisible();
    await lineRow.click();

    // It selects the line...
    await expect.poll(async () =>
      page.evaluate(() => {
        const sel = (window as unknown as {
          __E2E_TEST__?: { uiStore?: { getState: () => { selection: { type: string; lineId?: number } | null } } };
        }).__E2E_TEST__?.uiStore?.getState().selection;
        return sel?.type === 'line' ? sel.lineId ?? null : null;
      }),
    ).toBe(lineId);

    // ...but the camera must not have moved (no zoom-to-line on single click).
    // Allow any erroneous animation a moment to start, then assert unchanged.
    await page.waitForTimeout(400);
    const after = await page.evaluate(() => {
      const m = (window as unknown as {
        __stalagmight_map__?: { getZoom: () => number; getCenter: () => { lng: number; lat: number } };
      }).__stalagmight_map__!;
      const c = m.getCenter();
      return { lng: c.lng, lat: c.lat, zoom: m.getZoom() };
    });
    expect(Math.abs(after.zoom - before.zoom)).toBeLessThan(0.01);
    expect(Math.abs(after.lng - before.lng)).toBeLessThan(1e-6);
    expect(Math.abs(after.lat - before.lat)).toBeLessThan(1e-6);
  });

  test('center button zooms with margin from viewport edges', async ({ page }) => {
    const lineId = await getLineIdByName(page, 'Zoom Test Passage');

    // First, select the line to open the detail panel
    const lineRow = page.locator(`[data-testid="line-row-${lineId}"]`);
    await expect(lineRow).toBeVisible();
    await lineRow.click();

    // Wait for line detail panel to appear
    const detailPanel = page.getByTestId('line-detail');
    await expect(detailPanel).toBeVisible({ timeout: 5000 });

    // Wait for center button to be visible
    const centerButton = page.getByTestId('center-line-button');
    await expect(centerButton).toBeVisible();

    // Pan off-target first (memory pitfall: avoid false passes from pre-centered state)
    await panMapOffTarget(page);

    // Precondition: line must NOT satisfy margins after panning off-target
    await assertLineOffScreen(page, lineId);

    // Click the center button to trigger zoom-to-line
    await centerButton.click();

    // Poll until map is idle and margins are correct
    const result = await pollForMarginsSettled(page, lineId, MIN_MARGIN_PCT);
    assertMargins(result, MIN_MARGIN_PCT);
  });

  test('re-fitting an already-framed line keeps it in viewport with margin', async ({ page }) => {
    const lineId = await getLineIdByName(page, 'Zoom Test Passage');

    // Frame the line via the store. (#50 makes single-click select-only and
    // double-click enter focus mode + re-fit on the enlarged map - two fits at
    // two container sizes, a racy baseline. Triggering the fit directly keeps the
    // container stable so this isolates fitToLine's re-fit idempotence.)
    await page.evaluate((secId) => {
      const ui = (window as unknown as {
        __E2E_TEST__?: { uiStore?: { getState: () => { triggerZoomToLine: (id: number) => void } } };
      }).__E2E_TEST__?.uiStore;
      ui?.getState().triggerZoomToLine(secId);
    }, lineId);

    // Wait for initial zoom to settle with margins
    await pollForMarginsSettled(page, lineId, MIN_MARGIN_PCT);

    // Get the zoom level after first zoom
    const zoomBefore = await page.evaluate(() => {
      const map = (window as unknown as {
        __stalagmight_map__?: { getZoom: () => number };
      }).__stalagmight_map__;
      return map?.getZoom() ?? 0;
    });

    // Re-fire the same zoom request via the store: re-fitting an already-framed
    // line (same container) must not jump the zoom.
    await page.evaluate((secId) => {
      const ui = (window as unknown as {
        __E2E_TEST__?: { uiStore?: { getState: () => { triggerZoomToLine: (id: number) => void } } };
      }).__E2E_TEST__?.uiStore;
      ui?.getState().triggerZoomToLine(secId);
    }, lineId);

    // Wait for second animation to complete
    await expect.poll(
      async () => {
        return page.evaluate(() => {
          const m = (window as unknown as {
            __stalagmight_map__?: { isMoving: () => boolean };
          }).__stalagmight_map__;
          return m ? !m.isMoving() : false;
        });
      },
      { timeout: 10000 },
    ).toBe(true);

    // Zoom level should not have changed significantly (only centering occurred)
    const zoomAfter = await page.evaluate(() => {
      const map = (window as unknown as {
        __stalagmight_map__?: { getZoom: () => number };
      }).__stalagmight_map__;
      return map?.getZoom() ?? 0;
    });
    expect(Math.abs(zoomAfter - zoomBefore)).toBeLessThan(0.5);

    // Line should still be within viewport with margin
    const finalResult = await getStationScreenMargins(page, lineId);
    expect(finalResult).not.toBeNull();
    assertMargins(finalResult!, MIN_MARGIN_PCT);
  });

  test('exiting focus mode returns to the whole-map (fit-to-bounds) view', async ({ page }) => {
    const lineId = await getLineIdByName(page, 'Zoom Test Passage');
    const getView = () =>
      page.evaluate(() => {
        const m = (window as unknown as {
          __stalagmight_map__?: { getZoom: () => number; getCenter: () => { lng: number; lat: number } };
        }).__stalagmight_map__!;
        const c = m.getCenter();
        return { lng: c.lng, lat: c.lat, zoom: m.getZoom() };
      });
    const mapIdle = () =>
      page.evaluate(() => {
        const m = (window as unknown as { __stalagmight_map__?: { isMoving: () => boolean } }).__stalagmight_map__;
        return m ? !m.isMoving() : false;
      });
    const settle = async () => {
      // Ride past the post-action re-fit delay (180ms here + 100ms in MapCanvas).
      await page.waitForTimeout(400);
      await expect.poll(mapIdle, { timeout: 15000 }).toBe(true);
    };

    // Enter focus via the dedicated per-row focus button, then exit via the map's
    // top-left fit-to-view button (which doubles as exit-focus in focus mode).
    const lineRow = page.locator(`[data-testid="line-row-${lineId}"]`);
    await expect(lineRow).toBeVisible();
    await lineRow.hover();
    await page.getByTestId(`line-focus-${lineId}`).click();
    await pollForMarginsSettled(page, lineId, MIN_MARGIN_PCT);
    await page.getByTestId('fit-to-view-button').click();
    await expect(page.getByTestId('sections-panel')).toBeVisible();
    await settle();
    const exitView = await getView();

    // Exit must already be at the whole-map fit-to-bounds view: triggering
    // fit-to-bounds now must be a no-op (idempotent). If exit had instead stayed
    // zoomed into the line (the bug), this would move the camera.
    await page.evaluate(() => {
      const ui = (window as unknown as {
        __E2E_TEST__?: { uiStore?: { getState: () => { triggerFitToBounds: () => void } } };
      }).__E2E_TEST__?.uiStore;
      ui?.getState().triggerFitToBounds();
    });
    await settle();
    const fitView = await getView();

    expect(Math.abs(exitView.zoom - fitView.zoom)).toBeLessThan(0.25);
    expect(Math.abs(exitView.lng - fitView.lng)).toBeLessThan(1e-5);
    expect(Math.abs(exitView.lat - fitView.lat)).toBeLessThan(1e-5);
  });
});
