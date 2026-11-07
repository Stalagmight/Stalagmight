/**
 * E2E tests for section zoom margin behavior.
 *
 * Verifies that zooming to a section leaves visible margin from viewport edges
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
  seedProjectWithSections,
  waitForMapIdle,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';

/**
 * Multi-station section with distinct geometry to produce
 * a non-trivial bounding box on the map.
 */
const sectionSeeds: SectionSeed[] = [
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
 * Setup: create project, seed sections, wait for map.
 */
async function createProjectWithSections(page: Page): Promise<void> {
  await createProject(page, 'Section Zoom Margin Test');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
}

/**
 * Get the ID of the first section by name from the project store.
 */
async function getSectionIdByName(page: Page, name: string): Promise<number> {
  const id = await page.evaluate((sectionName) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => { project: { sections: Array<{ id: number; name: string }> } | null };
        };
      };
    }).__E2E_TEST__;
    const section = stores?.projectStore.getState().project?.sections.find(
      (s) => s.name === sectionName,
    );
    return section?.id ?? -1;
  }, name);
  expect(id).toBeGreaterThan(0);
  return id;
}

/**
 * Pan the map far off-target so the section is provably outside the viewport.
 * Uses a high zoom level (15) at a far-away center to ensure that a small
 * section near the GPS origin is completely off-screen rather than simply
 * tiny at a low zoom level.
 *
 * Memory pitfall: tests may pass falsely if map is already centered on the target section.
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
      // level high enough that the target section is off-screen
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
 * Assert precondition: after panMapOffTarget, the section must NOT satisfy
 * the margin requirement. This prevents false passes where the section was
 * already visible before the zoom action was triggered.
 */
async function assertSectionOffScreen(page: Page, sectionId: number): Promise<void> {
  const result = await getStationScreenMargins(page, sectionId);
  // Either null (map moving or data unavailable) or margins are violated
  if (result) {
    const allMarginsOk =
      result.minMarginLeftPct >= MIN_MARGIN_PCT &&
      result.minMarginRightPct >= MIN_MARGIN_PCT &&
      result.minMarginTopPct >= MIN_MARGIN_PCT &&
      result.minMarginBottomPct >= MIN_MARGIN_PCT;
    expect(allMarginsOk, 'Precondition failed: section should be off-screen after panMapOffTarget').toBe(false);
  }
  // null result also satisfies precondition (section data not visible)
}

/**
 * Project every DRAWN station of a section to screen pixels with map.project(),
 * and compute margin percentages from each viewport edge.
 * Returns null if data is not yet available or map is still moving (for use in polling).
 *
 * Station positions come from the app's drawn-geometry hook
 * (`__stalagmight_geometry__`, the same index the renderer and hit-testing use),
 * i.e. exactly the lng/lat that is painted. This used to re-derive them from the
 * station metres with an inlined flat 111 320 m/deg conversion that claimed to be
 * "the same algorithm as utils/coordinates.ts" - which is now an ellipsoidal
 * (WGS84) transform, so the copy measured a slightly different survey than the one
 * on screen (#576 NF-EXP-2). zoom_max comes from config.map.zoom_max in the
 * project store.
 */
async function getStationScreenMargins(page: Page, sectionId: number): Promise<MarginResult | null> {
  return page.evaluate((secId) => {
    const w = window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { config: { map: { zoom_max: number } } | null } };
      };
      __stalagmight_map__?: {
        project: (lngLat: [number, number]) => { x: number; y: number };
        getContainer: () => HTMLElement;
        getZoom: () => number;
        isMoving: () => boolean;
      };
      __stalagmight_geometry__?: () => {
        stations: Array<{ sectionId: number; lngLat: [number, number] }>;
      } | null;
    };
    const map = w.__stalagmight_map__;
    const geometry = w.__stalagmight_geometry__?.();
    if (!w.__E2E_TEST__ || !map || !geometry) return null;

    // Only measure when map is idle
    if (map.isMoving()) return null;

    const lngLats = geometry.stations.filter((s) => s.sectionId === secId).map((s) => s.lngLat);
    if (lngLats.length === 0) return null;

    // Read zoom_max from config instead of hard-coding
    const zoomMax = w.__E2E_TEST__.projectStore.getState().config?.map.zoom_max ?? 22;

    const container = map.getContainer();
    const containerWidth = container.clientWidth;
    const containerHeight = container.clientHeight;
    const zoom = map.getZoom();

    // Project the section's drawn stations to screen coordinates
    const points = lngLats.map((lngLat) => map.project(lngLat));

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
  }, sectionId);
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
  sectionId: number,
  minPct: number,
  timeout = 15000,
): Promise<MarginResult> {
  let lastResult: MarginResult | null = null;
  await expect.poll(
    async () => {
      const result = await getStationScreenMargins(page, sectionId);
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

test.describe('Section zoom margin', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProjectWithSections(page);
  });

  test('double-click on section row zooms with margin from viewport edges', async ({ page }) => {
    const sectionId = await getSectionIdByName(page, 'Zoom Test Passage');

    // Pan off-target first (memory pitfall: avoid false passes from pre-centered state)
    await panMapOffTarget(page);

    // Precondition: section must NOT satisfy margins after panning off-target
    await assertSectionOffScreen(page, sectionId);

    // Double-click the row: zooms/frames the section on the map (without entering
    // focus mode). Margins must hold against the current viewport.
    const sectionRow = page.locator(`[data-testid="section-row-${sectionId}"]`);
    await expect(sectionRow).toBeVisible();
    await sectionRow.dblclick();

    // Poll until map is idle and margins are correct (combined to avoid hanging)
    const result = await pollForMarginsSettled(page, sectionId, MIN_MARGIN_PCT);
    assertMargins(result, MIN_MARGIN_PCT);
  });

  test('single-click on section row selects WITHOUT moving the map', async ({ page }) => {
    // #50 reverts #64: a single click only selects; it must not move the map.
    const sectionId = await getSectionIdByName(page, 'Zoom Test Passage');

    // Pan off-target and record the camera so we can prove the click leaves it.
    await panMapOffTarget(page);
    const before = await page.evaluate(() => {
      const m = (window as unknown as {
        __stalagmight_map__?: { getZoom: () => number; getCenter: () => { lng: number; lat: number } };
      }).__stalagmight_map__!;
      const c = m.getCenter();
      return { lng: c.lng, lat: c.lat, zoom: m.getZoom() };
    });

    const sectionRow = page.locator(`[data-testid="section-row-${sectionId}"]`);
    await expect(sectionRow).toBeVisible();
    await sectionRow.click();

    // It selects the section...
    await expect.poll(async () =>
      page.evaluate(() => {
        const sel = (window as unknown as {
          __E2E_TEST__?: { uiStore?: { getState: () => { selection: { type: string; sectionId?: number } | null } } };
        }).__E2E_TEST__?.uiStore?.getState().selection;
        return sel?.type === 'section' ? sel.sectionId ?? null : null;
      }),
    ).toBe(sectionId);

    // ...but the camera must not have moved (no zoom-to-section on single click).
    // settle: allow any erroneous animation a moment to start, then assert the
    // camera is unchanged - a poll would pass before it began.
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
    const sectionId = await getSectionIdByName(page, 'Zoom Test Passage');

    // First, select the section to open the detail panel
    const sectionRow = page.locator(`[data-testid="section-row-${sectionId}"]`);
    await expect(sectionRow).toBeVisible();
    await sectionRow.click();

    // Wait for section detail panel to appear
    const detailPanel = page.getByTestId('section-detail');
    await expect(detailPanel).toBeVisible({ timeout: 5000 });

    // Wait for center button to be visible
    const centerButton = page.getByTestId('center-section-button');
    await expect(centerButton).toBeVisible();

    // Pan off-target first (memory pitfall: avoid false passes from pre-centered state)
    await panMapOffTarget(page);

    // Precondition: section must NOT satisfy margins after panning off-target
    await assertSectionOffScreen(page, sectionId);

    // Click the center button to trigger zoom-to-section
    await centerButton.click();

    // Poll until map is idle and margins are correct
    const result = await pollForMarginsSettled(page, sectionId, MIN_MARGIN_PCT);
    assertMargins(result, MIN_MARGIN_PCT);
  });

  test('re-fitting an already-framed section keeps it in viewport with margin', async ({ page }) => {
    const sectionId = await getSectionIdByName(page, 'Zoom Test Passage');

    // Frame the section via the store. (#50 makes single-click select-only and
    // double-click enter focus mode + re-fit on the enlarged map - two fits at
    // two container sizes, a racy baseline. Triggering the fit directly keeps the
    // container stable so this isolates fitToSection's re-fit idempotence.)
    await page.evaluate((secId) => {
      const ui = (window as unknown as {
        __E2E_TEST__?: { uiStore?: { getState: () => { triggerZoomToSection: (id: number) => void } } };
      }).__E2E_TEST__?.uiStore;
      ui?.getState().triggerZoomToSection(secId);
    }, sectionId);

    // Wait for initial zoom to settle with margins
    await pollForMarginsSettled(page, sectionId, MIN_MARGIN_PCT);

    // Get the zoom level after first zoom
    const zoomBefore = await page.evaluate(() => {
      const map = (window as unknown as {
        __stalagmight_map__?: { getZoom: () => number };
      }).__stalagmight_map__;
      return map?.getZoom() ?? 0;
    });

    // Re-fire the same zoom request via the store: re-fitting an already-framed
    // section (same container) must not jump the zoom.
    await page.evaluate((secId) => {
      const ui = (window as unknown as {
        __E2E_TEST__?: { uiStore?: { getState: () => { triggerZoomToSection: (id: number) => void } } };
      }).__E2E_TEST__?.uiStore;
      ui?.getState().triggerZoomToSection(secId);
    }, sectionId);

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

    // Section should still be within viewport with margin
    const finalResult = await getStationScreenMargins(page, sectionId);
    expect(finalResult).not.toBeNull();
    assertMargins(finalResult!, MIN_MARGIN_PCT);
  });

  test('exiting focus mode returns to the whole-map (fit-to-bounds) view', async ({ page }) => {
    const sectionId = await getSectionIdByName(page, 'Zoom Test Passage');
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
      // settle: ride past the post-action re-fit delay (180ms here + 100ms in
      // MapCanvas); the assertions after it check that no re-fit moved the camera.
      await page.waitForTimeout(400);
      await expect.poll(mapIdle, { timeout: 15000 }).toBe(true);
    };

    // Enter focus via the dedicated per-row focus button, then exit via the map's
    // top-left fit-to-view button (which doubles as exit-focus in focus mode).
    const sectionRow = page.locator(`[data-testid="section-row-${sectionId}"]`);
    await expect(sectionRow).toBeVisible();
    await sectionRow.hover();
    await page.getByTestId(`section-focus-${sectionId}`).click();
    await pollForMarginsSettled(page, sectionId, MIN_MARGIN_PCT);
    await page.getByTestId('fit-to-view-button').click();
    await expect(page.getByTestId('sections-panel')).toBeVisible();
    await settle();
    const exitView = await getView();

    // Exit must already be at the whole-map fit-to-bounds view: triggering
    // fit-to-bounds now must be a no-op (idempotent). If exit had instead stayed
    // zoomed into the section (the bug), this would move the camera.
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
