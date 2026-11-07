import { test, expect, type Page } from './fixtures/coverage';
import type { CDPSession } from '@playwright/test';
import {
  setupMockedPage,
  createProject,
  getMapCenter,
  getSelection,
  seedProjectWithSections,
  waitForMapReady,
  waitForMapIdle,
  waitForNoDialogs,
  waitForActiveTool,
  waitForDrawnStations,
  type ScreenPoint,
  type SectionSeed,
} from './helpers/app';

/**
 * Touch gestures on the survey map (#409 / #338), in the DEFAULT configuration -
 * no `seedNavigationTools`, because a touch device is exactly the case the
 * hidden Pan tool is justified by: you pan by dragging the map.
 *
 * Playwright's own API cannot express these (no touch drag, no pinch), so the
 * gestures are dispatched through CDP `Input.dispatchTouchEvent`, which is what
 * Chromium's own touch emulation consumes. `test.use({ hasTouch: true })` is what
 * makes the browser context accept them at all.
 */

test.use({ hasTouch: true });

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Touch Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -2 },
      { azimuth_in: 90, azimuth_out: 270, length: 15, depth_in: -2, depth_out: -3 },
      { azimuth_in: 135, azimuth_out: 315, length: 12, depth_in: -3, depth_out: -4 },
    ],
  },
];

interface TouchPoint {
  x: number;
  y: number;
  id: number;
}

/**
 * One CDP touch event. `touchEnd` / `touchCancel` take NO points - Chromium
 * treats an empty list as "every live point released", which is the shape
 * Puppeteer's own touchscreen uses.
 */
async function touch(
  cdp: CDPSession,
  type: 'touchStart' | 'touchMove' | 'touchEnd' | 'touchCancel',
  points: TouchPoint[] = []
): Promise<void> {
  await cdp.send('Input.dispatchTouchEvent', {
    type,
    touchPoints: type === 'touchEnd' || type === 'touchCancel' ? [] : points,
  });
}

async function getMapCamera(page: Page): Promise<{ zoom: number; bearing: number }> {
  const camera = await page.evaluate(() => {
    const map = (window as unknown as {
      __stalagmight_map__?: { getZoom: () => number; getBearing: () => number };
    }).__stalagmight_map__;
    if (!map) return null;
    return { zoom: map.getZoom(), bearing: map.getBearing() };
  });
  expect(camera, 'map instance is available').not.toBeNull();
  return camera!;
}

async function canvasBox(page: Page): Promise<{ x: number; y: number; width: number; height: number }> {
  const box = await page.locator('canvas.maplibregl-canvas').boundingBox();
  expect(box, 'map canvas has a bounding box').not.toBeNull();
  return box!;
}

/** Zoom the camera onto the seeded survey so the stations are pixels apart. */
async function zoomInOnSurvey(page: Page, zoomDelta = 3): Promise<void> {
  await page.evaluate((dz) => {
    const w = window as unknown as {
      __stalagmight_map__?: {
        jumpTo: (opts: { center: [number, number]; zoom: number }) => void;
        getZoom: () => number;
      };
      __stalagmight_geometry__?: () => { stations: Array<{ lngLat: [number, number] }> } | null;
    };
    const map = w.__stalagmight_map__;
    const geometry = w.__stalagmight_geometry__?.();
    if (!map || !geometry || geometry.stations.length === 0) {
      throw new Error('map + drawn geometry must be ready before zooming');
    }
    const count = geometry.stations.length;
    const lng = geometry.stations.reduce((sum, s) => sum + s.lngLat[0], 0) / count;
    const lat = geometry.stations.reduce((sum, s) => sum + s.lngLat[1], 0) / count;
    map.jumpTo({ center: [lng, lat], zoom: map.getZoom() + dz });
  }, zoomDelta);
}

/** The station farthest from every other - an unambiguous long-press target. */
async function mostIsolatedStation(page: Page): Promise<ScreenPoint> {
  const stations = await waitForDrawnStations(page, 2);
  let best: ScreenPoint = stations[0]!;
  let bestMinOther = -Infinity;
  for (const candidate of stations) {
    let minOther = Infinity;
    for (const other of stations) {
      if (other === candidate) continue;
      minOther = Math.min(minOther, Math.hypot(candidate.x - other.x, candidate.y - other.y));
    }
    if (minOther > bestMinOther) {
      bestMinOther = minOther;
      best = { x: candidate.x, y: candidate.y };
    }
  }
  return best;
}

test.describe('Map touch gestures', () => {
  let cdp: CDPSession;

  test.beforeEach(async ({ page }) => {
    cdp = await page.context().newCDPSession(page);
    await setupMockedPage(page);
    await createProject(page, 'Touch Gestures');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await waitForMapIdle(page);
    await waitForDrawnStations(page, 2);
    await zoomInOnSurvey(page);
    await waitForMapIdle(page);
    await waitForActiveTool(page, 'select');
  });

  test('a one-finger drag pans the map under the default tool', async ({ page }) => {
    const box = await canvasBox(page);
    const start = { x: box.x + box.width * 0.35, y: box.y + box.height * 0.6 };
    const centerBefore = await getMapCenter(page);

    await touch(cdp, 'touchStart', [{ ...start, id: 1 }]);
    for (let step = 1; step <= 8; step += 1) {
      await touch(cdp, 'touchMove', [
        { x: start.x + step * 15, y: start.y + step * 10, id: 1 },
      ]);
    }
    await touch(cdp, 'touchEnd');
    await waitForMapIdle(page);

    const centerAfter = await getMapCenter(page);
    expect(
      Math.abs(centerAfter.lng - centerBefore.lng) + Math.abs(centerAfter.lat - centerBefore.lat)
    ).toBeGreaterThan(0.00001);
  });

  test('a long-press opens the context menu and its lift is swallowed (#338)', async ({ page }) => {
    // Record what the touchend looked like AFTER SurveyLayer's canvas listener
    // ran: a document-level bubble listener sees the same event object, with
    // `defaultPrevented` already set by the handler below it. That flag is the
    // whole mechanism - it is what stops the browser synthesising the
    // compatibility mouse sequence that would tap the tool underneath.
    await page.evaluate(() => {
      const w = window as unknown as { __touchEndDefaultPrevented__?: boolean | null };
      w.__touchEndDefaultPrevented__ = null;
      document.addEventListener(
        'touchend',
        (e) => {
          w.__touchEndDefaultPrevented__ = e.defaultPrevented;
        },
        false
      );
    });

    const box = await canvasBox(page);
    const station = await mostIsolatedStation(page);
    const point = { x: box.x + station.x, y: box.y + station.y, id: 1 };

    await touch(cdp, 'touchStart', [point]);

    // The long-press timer is 500ms; waiting for the menu it opens is both the
    // ">600ms hold" and the assertion that the press was recognised.
    const menu = page.getByTestId('context-menu');
    await expect(menu).toBeVisible({ timeout: 5000 });

    // Opening the menu selects its target - that is the menu's own doing, and it
    // is the state the LIFT must not disturb.
    const selectionWithMenu = await getSelection(page);
    expect(selectionWithMenu).not.toBeNull();

    await touch(cdp, 'touchEnd');

    expect(
      await page.evaluate(
        () => (window as unknown as { __touchEndDefaultPrevented__?: boolean | null })
          .__touchEndDefaultPrevented__
      ),
      'the touchend ending a fired long-press is preventDefault-ed'
    ).toBe(true);

    // settle: then one read - this proves the lift changed NOTHING, and a poll
    // would accept the pre-update value.
    await page.waitForTimeout(200);
    expect(await getSelection(page)).toEqual(selectionWithMenu);
    await expect(menu).toBeVisible();
  });

  test('a two-finger twist zooms but never rotates the plan', async ({ page }) => {
    const box = await canvasBox(page);
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;
    const before = await getMapCamera(page);
    expect(before.bearing).toBeCloseTo(0, 5);

    // Two fingers on a horizontal line, then swept through 60 degrees while
    // spreading apart. The spread is deliberate: it makes the gesture leave a
    // trace (zoom changes), so "bearing is still 0" cannot pass simply because
    // MapLibre never saw the gesture at all.
    const startRadius = 60;
    const steps = 10;
    await touch(cdp, 'touchStart', [
      { x: cx - startRadius, y: cy, id: 1 },
      { x: cx + startRadius, y: cy, id: 2 },
    ]);
    for (let step = 1; step <= steps; step += 1) {
      const angle = (Math.PI / 3) * (step / steps);
      const radius = startRadius + step * 6;
      const dx = Math.cos(angle) * radius;
      const dy = Math.sin(angle) * radius;
      await touch(cdp, 'touchMove', [
        { x: cx - dx, y: cy - dy, id: 1 },
        { x: cx + dx, y: cy + dy, id: 2 },
      ]);
    }
    await touch(cdp, 'touchEnd');
    await waitForMapIdle(page);

    const after = await getMapCamera(page);
    expect(after.zoom, 'the pinch reached MapLibre').not.toBeCloseTo(before.zoom, 3);
    expect(after.bearing, 'the plan is top-down: a twist must not rotate it').toBeCloseTo(0, 5);
  });
});
