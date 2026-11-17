import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithLines,
  waitForDrawnStations,
  getStationScreenPositions,
  getShotScreenPositions,
  readCanvasPixel,
  type PixelRgba,
  type LineSeed,
} from './helpers/app';
import { DISTANCE_NEAR, DISTANCE_FAR } from '../src/utils/distanceGradient';

/**
 * E2E for the distance-gradient overlay (#34).
 *
 * The survey is drawn by the custom WebGL renderer, so the old oracle is gone:
 * there is no `survey-stations` layer whose `circle-color` paint could be checked
 * for a `feature-state` interpolate, and no `survey-data-stations` vector-tile
 * source to read a per-station `dist` feature-state from. The renderer resolves
 * the SAME ramp color per feature at build time instead
 * (surveyLayer/customRender/overlayColor.ts, byte-identical to the old
 * data-driven paint), so the oracle here is the PAINTED STATION PIXEL: "this
 * station carries a numeric dist" becomes "this station is painted its
 * distance-ramp color". That is strictly closer to what the diver sees.
 *
 * The mock's distance field is `|x|` of each station and the mock puts station
 * `i` at `x = i * 10`, so with a 2-shot line the scale is 0..20: station 0 is
 * at distance 0 (the exact NEAR end of the ramp), station 2 at the max (the exact
 * FAR end) and station 1 halfway (a genuinely interpolated color). Those two
 * endpoints are exact because MapLibre's `interpolate` short-circuits them, and
 * `packRampColor` reproduces that short-circuit deliberately.
 */

async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

const lineSeeds: LineSeed[] = [
  {
    name: 'Distance A',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 12, depth_in: 0, depth_out: -6 },
      { azimuth_in: 90, azimuth_out: 270, length: 8, depth_in: -6, depth_out: -12 },
    ],
  },
  {
    name: 'Distance B',
    direction: 'out',
    shots: [{ azimuth_in: 180, azimuth_out: 0, length: 6, depth_in: -12, depth_out: -3 }],
  },
];

/** `#rrggbb` -> the opaque RGBA bytes the renderer packs it to. */
function hexRgba(hex: string): PixelRgba {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff, 255];
}

const NEAR_RGBA = hexRgba(DISTANCE_NEAR);
const FAR_RGBA = hexRgba(DISTANCE_FAR);

async function openDisplayMenu(page: Page): Promise<void> {
  await page.locator('button:has-text("Display")').click();
  await expect(page.locator('[data-testid="display-toggle-showStations"]')).toBeVisible({ timeout: 2000 });
}

async function isToggleOn(page: Page, toggleKey: string): Promise<boolean> {
  return (await page.locator(`[data-testid="display-toggle-${toggleKey}"]`).getAttribute('aria-pressed')) === 'true';
}

async function toggleDisplayOption(page: Page, toggleKey: string): Promise<void> {
  const colorKeys = ['showDepthGradient', 'showDistanceGradient', 'showIssueSeverity'];
  const before = await isToggleOn(page, toggleKey);
  // Color modes are single-select: turn one OFF by selecting Line, not by re-clicking the pill.
  if (before && colorKeys.includes(toggleKey)) {
    await page.locator('[data-testid="display-color-section"]').click();
  } else {
    await page.locator(`[data-testid="display-toggle-${toggleKey}"]`).click();
  }
  await expect.poll(async () => isToggleOn(page, toggleKey), { timeout: 2000 }).not.toBe(before);
}

async function waitForMapLoaded(page: Page): Promise<void> {
  await expect.poll(async () => {
    return page.evaluate(() => {
      const map = (window as unknown as { __stalagmight_map__?: { loaded: () => boolean } }).__stalagmight_map__;
      return map?.loaded() ?? false;
    });
  }, { timeout: 10000 }).toBe(true);
}

async function mapZoom(page: Page): Promise<number> {
  return page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number } }).__stalagmight_map__;
    return map?.getZoom() ?? 0;
  });
}

/**
 * Jump the map to an absolute zoom (no animation), so the geometry helpers (which
 * project with the live map) return the new positions immediately.
 */
async function setMapZoom(page: Page, zoom: number): Promise<void> {
  await page.evaluate((z) => {
    const map = (window as unknown as { __stalagmight_map__?: { setZoom: (z: number) => void } }).__stalagmight_map__;
    map?.setZoom(z);
  }, zoom);
}

/**
 * Zoom in before probing painted pixels. The seeded survey is only a few metres
 * across, so at the fit-to-bounds zoom the whole thing is ~9 screen px wide and
 * neighbouring station discs cover each other, so a probe there cannot tell one
 * station's ramp color from its neighbour's.
 */
async function zoomIn(page: Page, delta = 4): Promise<number> {
  const target = (await mapZoom(page)) + delta;
  await setMapZoom(page, target);
  return target;
}

/**
 * The painted pixel at the center of one station's disc, identified by line
 * ORDER (the seeded lines' ids are assigned by the mock) and station index.
 * The center pixel is the station's fill, which is exactly what the overlay owns.
 */
async function stationPixel(page: Page, lineOrder: number, stationIndex: number): Promise<PixelRgba> {
  const stations = await getStationScreenPositions(page);
  const lineIds = [...new Set(stations.map((s) => s.lineId))].sort((a, b) => a - b);
  const lineId = lineIds[lineOrder];
  const station = stations.find((s) => s.lineId === lineId && s.stationIndex === stationIndex);
  if (!station) throw new Error(`no drawn station for line #${lineOrder} index ${stationIndex}`);
  return readCanvasPixel(page, station.x, station.y);
}

/**
 * The painted pixel at the midpoint of one shot's line. The overlay owns the shot
 * line color too, and a shot's value is the average of its endpoint stations, so a
 * mid-survey shot always lands strictly inside the ramp.
 */
async function shotPixel(page: Page, lineOrder: number, shotOrder: number): Promise<PixelRgba> {
  const shots = await getShotScreenPositions(page);
  const lineIds = [...new Set(shots.map((s) => s.lineId))].sort((a, b) => a - b);
  const lineId = lineIds[lineOrder];
  const inLine = shots.filter((s) => s.lineId === lineId).sort((a, b) => a.shotId - b.shotId);
  const shot = inLine[shotOrder];
  if (!shot) throw new Error(`no drawn shot #${shotOrder} for line #${lineOrder}`);
  return readCanvasPixel(page, shot.mid.x, shot.mid.y);
}

/** Every channel strictly between the two ramp endpoints: a genuinely
 *  interpolated ramp color, not either endpoint and not a flat base color. */
function expectBetweenRampEnds(px: PixelRgba): void {
  for (let c = 0; c < 3; c++) {
    const lo = Math.min(NEAR_RGBA[c]!, FAR_RGBA[c]!);
    const hi = Math.max(NEAR_RGBA[c]!, FAR_RGBA[c]!);
    expect(px[c]!).toBeGreaterThan(lo);
    expect(px[c]!).toBeLessThan(hi);
  }
}

test.describe('Distance Gradient', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Distance Gradient Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapLoaded(page);
    await waitForDrawnStations(page, 1);
  });

  test('toggle paints the distance ramp (+ legend) and off restores the line colors', async ({ page }) => {
    await zoomIn(page);
    await openDisplayMenu(page);
    expect(await isToggleOn(page, 'showDistanceGradient')).toBe(false);

    // Baseline: in plain line-color mode every station of the line shares one
    // color, and it is neither end of the distance ramp.
    const baseline = await stationPixel(page, 0, 0);
    expect(await stationPixel(page, 0, 1)).toEqual(baseline);
    expect(await stationPixel(page, 0, 2)).toEqual(baseline);
    expect(baseline).not.toEqual(NEAR_RGBA);
    expect(baseline).not.toEqual(FAR_RGBA);

    await toggleDisplayOption(page, 'showDistanceGradient');

    // Legend appears.
    await expect(page.locator('[data-testid="distance-legend"]')).toBeVisible({ timeout: 3000 });
    // Default origin label.
    await expect(page.locator('[data-testid="distance-legend-origin"]')).toContainText('entrance');
    // The stations are now painted their distance-ramp colors: station 0 sits at
    // distance 0 (the exact NEAR end), station 2 at the field max (the exact FAR
    // end), and station 1 halfway between them.
    await expect.poll(() => stationPixel(page, 0, 0), { timeout: 3000 }).toEqual(NEAR_RGBA);
    expect(await stationPixel(page, 0, 2)).toEqual(FAR_RGBA);
    expectBetweenRampEnds(await stationPixel(page, 0, 1));

    // Toggle off -> legend gone, line colors restored.
    await toggleDisplayOption(page, 'showDistanceGradient');
    await expect(page.locator('[data-testid="distance-legend"]')).not.toBeVisible();
    await expect.poll(() => stationPixel(page, 0, 0), { timeout: 3000 }).toEqual(baseline);
    expect(await stationPixel(page, 0, 2)).toEqual(baseline);
  });

  test('enabling depth gradient disables the distance overlay (mutually exclusive)', async ({ page }) => {
    await zoomIn(page);
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showDistanceGradient');
    await expect.poll(() => stationPixel(page, 0, 0), { timeout: 3000 }).toEqual(NEAR_RGBA);
    await expect(page.locator('[data-testid="distance-legend"]')).toBeVisible({ timeout: 3000 });

    // Depth and distance both paint the shots layer, so they're mutually exclusive
    // (#34, report of mixed depth-colored shots + distance-colored
    // stations). Enabling depth turns the distance overlay off.
    await toggleDisplayOption(page, 'showDepthGradient');

    expect(await isToggleOn(page, 'showDistanceGradient')).toBe(false);
    await expect(page.locator('[data-testid="distance-legend"]')).not.toBeVisible();
    await expect(page.locator('[data-testid="depth-legend"]')).toBeVisible();
    // Neither ramp end is painted any more: the depth ramp owns the color now.
    await expect.poll(() => stationPixel(page, 0, 0), { timeout: 3000 }).not.toEqual(NEAR_RGBA);
    expect(await stationPixel(page, 0, 2)).not.toEqual(FAR_RGBA);
  });

  test('the distance ramp survives a feature rebuild (re-applied)', async ({ page }) => {
    const zoom = await zoomIn(page);
    await openDisplayMenu(page);

    // Baseline line color of the two features probed after the rebuild, so
    // "still ramp-painted" is provably not "reverted to the base colors".
    const stationBase = await stationPixel(page, 0, 2);
    const shotBase = await shotPixel(page, 0, 1);

    await toggleDisplayOption(page, 'showDistanceGradient');
    await expect.poll(() => stationPixel(page, 0, 2), { timeout: 3000 }).toEqual(FAR_RGBA);
    expectBetweenRampEnds(await shotPixel(page, 0, 1));

    // A survey edit rebuilds every feature (the custom renderer rebuilds its vertex
    // buffers from scratch; the stock path had setData wipe the feature-state). The
    // overlay must re-apply its ramp afterward, the key correctness risk. (Depth
    // toggle can no longer be the trigger since it now disables distance.)
    await seedProjectWithLines(page, [
      {
        name: 'Distance C',
        direction: 'in',
        shots: [{ azimuth_in: 0, azimuth_out: 180, length: 5, depth_in: -3, depth_out: -9 }],
      },
    ]);
    // The rebuild can refit the view, so re-pin the zoom the probe was calibrated at.
    await setMapZoom(page, zoom);
    await waitForDrawnStations(page, 1);

    // Still painted ramp colors, NOT the base line colors: the re-apply held for
    // both the station fill and the shot line.
    //
    // Probe the far end of the FIRST line: the mock lays every line out along
    // the same axis (station i at x = i*10), so the new line's stations sit
    // exactly on top of the first two of this one, and the mock never bumps
    // `modified_at` on create_line (the refetch key), so the field the ramp
    // re-applies from still predates the new line and paints it grey
    // (unreachable) where they overlap. Station 2 / shot 2 of the first line are
    // beyond the new line's extent, so they read the ramp cleanly.
    await expect.poll(() => stationPixel(page, 0, 2), { timeout: 3000 }).toEqual(FAR_RGBA);
    expect(await stationPixel(page, 0, 2)).not.toEqual(stationBase);
    const shotAfter = await shotPixel(page, 0, 1);
    expectBetweenRampEnds(shotAfter);
    expect(shotAfter).not.toEqual(shotBase);
    await expect(page.locator('[data-testid="distance-legend"]')).toBeVisible();
  });
});
