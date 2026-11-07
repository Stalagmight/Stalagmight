import type { Page } from '@playwright/test';
import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForDrawnStations,
  getDrawnGeometry,
  getMeasuredPathCounts,
  getStationScreenPositions,
  readCanvasPixel,
  countCanvasInk,
  type SectionSeed,
} from './helpers/app';

/**
 * MapLibre 6 rebuilds the style from `style.serialize()` after a WebGL context
 * loss and after a worker restart, and custom layers cannot be serialized, so
 * both paths drop every survey custom layer (#606).
 * `surveyLayer/glContextRecovery.ts` re-adds them, timed by
 * `customRender/styleRestore.ts` (#576 D38). Each test forces one restore path for real and requires the same style
 * layers back, custom and stock, in the same order, and survey ink at a shot
 * midpoint again.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Restore Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5, left: 1.5, right: 2.3, up: 3.0, down: 0.8 },
      { azimuth_in: 90, azimuth_out: 270, length: 15.2, depth_in: -2.5, depth_out: -5.0, left: 0.5, right: 0.7, up: 1.2, down: 1.5 },
    ],
  },
];

type RestoreMap = {
  getLayersOrder: () => string[];
  once: (ev: string, cb: () => void) => void;
  painter: { context: { gl: WebGL2RenderingContext } };
  _handleWorkersRestarted: () => void;
};

type UiStore = {
  getState: () => Record<string, unknown>;
  setState: (partial: Record<string, unknown>) => void;
};

type LoseContextWindow = {
  __stalagmight_map__: RestoreMap;
  __restoreLoseContext__?: WEBGL_lose_context;
};

function layerOrder(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: RestoreMap }).__stalagmight_map__;
    return map?.getLayersOrder() ?? [];
  });
}

async function surveyInk(page: Page): Promise<number> {
  const geom = await getDrawnGeometry(page);
  const shot = geom.shots[Math.floor(geom.shots.length / 2)];
  if (!shot) return 0;
  const bg = await readCanvasPixel(page, 8, 8);
  return countCanvasInk(page, Math.round(shot.mid.x), Math.round(shot.mid.y), bg, 9);
}

async function bootSurvey(page: Page): Promise<string[]> {
  await setupMockedPage(page);
  await createProject(page, 'Restore');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapReady(page);
  await waitForDrawnStations(page, 1);
  await expect.poll(() => surveyInk(page), { timeout: 10000 }).toBeGreaterThan(0);
  const before = await layerOrder(page);
  expect(before).toContain('survey-custom-render');
  return before;
}

async function expectRestored(page: Page, before: string[]): Promise<void> {
  await expect.poll(() => layerOrder(page), { timeout: 10000 }).toEqual(before);
  await expect.poll(() => surveyInk(page), { timeout: 10000 }).toBeGreaterThan(0);
}

/** Lose the WebGL context for real and wait until MapLibre has handled it. */
async function loseContext(page: Page): Promise<boolean> {
  return page.evaluate(async () => {
    const w = window as unknown as LoseContextWindow;
    const map = w.__stalagmight_map__;
    const ext = map.painter.context.gl.getExtension('WEBGL_lose_context');
    if (!ext) return false;
    w.__restoreLoseContext__ = ext;
    const lost = new Promise<void>((resolve) => map.once('webglcontextlost', resolve));
    ext.loseContext();
    await lost;
    // Chromium allows restoreContext only once the lost event's dispatch,
    // whose preventDefault grants it, has finished.
    await new Promise((resolve) => setTimeout(resolve, 0));
    return true;
  });
}

async function restoreContext(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const w = window as unknown as LoseContextWindow;
    const back = new Promise<void>((resolve) => w.__stalagmight_map__.once('webglcontextrestored', resolve));
    w.__restoreLoseContext__?.restoreContext();
    await back;
  });
}

async function clickStation(page: Page, index: number): Promise<void> {
  const target = (await getStationScreenPositions(page))[index];
  const box = await page.locator('canvas').first().boundingBox();
  if (!box || !target) throw new Error('no canvas/station');
  await page.mouse.move(box.x + target.x, box.y + target.y);
  await page.mouse.click(box.x + target.x, box.y + target.y);
}

test.describe('Custom renderer restore', () => {
  test('survey layers come back in order after a worker restart', async ({ page }) => {
    const before = await bootSurvey(page);
    await page.evaluate(() => {
      const map = (window as unknown as { __stalagmight_map__: RestoreMap }).__stalagmight_map__;
      map._handleWorkersRestarted();
    });
    await expectRestored(page, before);
  });

  test('survey layers come back in order after a WebGL context loss', async ({ page }) => {
    const before = await bootSurvey(page);
    expect(await loseContext(page), 'WEBGL_lose_context unavailable').toBe(true);
    await restoreContext(page);
    await expectRestored(page, before);
  });

  test('a section added while the context is lost is drawn after the restore', async ({ page }) => {
    const before = await bootSurvey(page);
    const stationsBefore = (await getDrawnGeometry(page)).stations.length;
    expect(await loseContext(page), 'WEBGL_lose_context unavailable').toBe(true);
    await seedProjectWithSections(page, [
      {
        name: 'Restore Added',
        direction: 'in',
        shots: [
          { azimuth_in: 180, azimuth_out: 0, length: 12, depth_in: -1, depth_out: -2, left: 1, right: 1, up: 1, down: 1 },
        ],
      },
    ]);
    await restoreContext(page);
    await expectRestored(page, before);
    await expect
      .poll(async () => (await getDrawnGeometry(page)).stations.length, { timeout: 10000 })
      .toBeGreaterThan(stationsBefore);
  });

  test('a ruler path that starts drawing while the context is lost draws after the restore', async ({ page }) => {
    await bootSurvey(page);
    await page.keyboard.press('r');
    await clickStation(page, 0);
    await clickStation(page, 1);
    await expect.poll(async () => (await getMeasuredPathCounts(page)).shots, { timeout: 10000 }).toBeGreaterThan(0);
    // Hit-testing is off while the style is gone, so the measurement is taken
    // first, cleared, and handed back to the store inside the loss window: the
    // tint layer's first add then lands on the null style.
    await page.evaluate(() => {
      const w = window as unknown as { __E2E_TEST__: { uiStore: UiStore }; __savedMeasure__?: unknown };
      const { measureStart, measureEnd, measureAlongLine } = w.__E2E_TEST__.uiStore.getState();
      w.__savedMeasure__ = { measureStart, measureEnd, measureAlongLine };
    });
    await page.keyboard.press('Escape');
    await expect.poll(async () => (await getMeasuredPathCounts(page)).shots).toBe(0);
    expect(await loseContext(page), 'WEBGL_lose_context unavailable').toBe(true);
    await page.evaluate(() => {
      const w = window as unknown as { __E2E_TEST__: { uiStore: UiStore }; __savedMeasure__: Record<string, unknown> };
      w.__E2E_TEST__.uiStore.setState(w.__savedMeasure__);
    });
    await restoreContext(page);
    await expect.poll(() => layerOrder(page), { timeout: 10000 }).toContain('survey-measure-path-custom');
    await expect.poll(async () => (await getMeasuredPathCounts(page)).shots, { timeout: 10000 }).toBeGreaterThan(0);
    await expect.poll(() => surveyInk(page), { timeout: 10000 }).toBeGreaterThan(0);
  });
});
