import { test, expect, type Page } from './fixtures/coverage';
import {
  seedNavigationTools,
  setupMockedPage,
  createProject,
  waitForNoDialogs,
  waitForActiveTool,
  seedProjectWithSections,
  createSectionViaStore,
  waitForDrawnShots,
  getDrawnGeometry,
  readCanvasPixel,
  countCanvasInk,
  type SectionSeed,
  type ScreenPoint,
} from './helpers/app';

/**
 * WebGL context loss (#576 D38). A GPU reset, driver crash, some sleep/resume
 * cycles or WebKit memory pressure lose the map's WebGL context. MapLibre then
 * drops every custom layer and, on restore, re-applies only a serialized style
 * that skips them, so the survey (drawn entirely by custom layers) vanished until
 * the map was remounted. The app now re-adds them itself.
 *
 * Chromium exposes WEBGL_lose_context, which forces exactly that loss + restore.
 * The oracle is the painted pixel: the custom layers have no MapLibre features.
 *
 * The Global View's survey layer comes back the same way, and a context that is
 * lost and NOT restored (WebKit may give up on it) is reported after ~5 s with
 * a toast whose "Reload map" rebuilds the map on a new context; the toast clears
 * itself if the context comes back after all (decided D38-LOST).
 */

const seeds: SectionSeed[] = [
  {
    name: 'CaveA Main',
    direction: 'in',
    shots: [
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -3, depth_out: -6 },
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -6, depth_out: -4 },
    ],
  },
];

const CUSTOM_LAYERS = ['survey-custom-render', 'survey-custom-labels'] as const;

async function openSeededAndZoom(page: Page, name: string): Promise<void> {
  await setupMockedPage(page);
  await createProject(page, name);
  await waitForNoDialogs(page);
  await seedProjectWithSections(page, seeds);
  await waitForDrawnShots(page, 3);
  // Spread the stations out so a shot midpoint is a few px of line, not a smear.
  await page.evaluate(() => {
    const w = window as unknown as {
      __stalagmight_map__: { jumpTo: (o: { center: [number, number]; zoom: number }) => void; getZoom: () => number };
      __stalagmight_geometry__: () => { stations: Array<{ lngLat: [number, number] }> };
    };
    const g = w.__stalagmight_geometry__();
    const lng = g.stations.reduce((s, x) => s + x.lngLat[0], 0) / g.stations.length;
    const lat = g.stations.reduce((s, x) => s + x.lngLat[1], 0) / g.stations.length;
    w.__stalagmight_map__.jumpTo({ center: [lng, lat], zoom: w.__stalagmight_map__.getZoom() + 3 });
  });
}

/** Total ink in small blocks around `points`, against the canvas-corner background. */
async function inkAt(page: Page, points: ScreenPoint[]): Promise<number> {
  const bg = await readCanvasPixel(page, 6, 6);
  let total = 0;
  for (const p of points) total += await countCanvasInk(page, p.x, p.y, bg, 9);
  return total;
}

/**
 * Page errors, plus any custom layer declaring itself dead: a frame drawn as the
 * context goes reports CONTEXT_LOST_WEBGL, which is no reason to self-disable.
 */
function collectErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => {
    if (m.text().includes('self-disabling')) errors.push(m.text());
  });
  return errors;
}

async function shotMidpoints(page: Page): Promise<ScreenPoint[]> {
  return (await getDrawnGeometry(page)).shots.map((s) => s.mid);
}

/** The map's layer ids, bottom to top. */
async function layerOrder(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    (window as unknown as { __stalagmight_map__: { getLayersOrder: () => string[] } }).__stalagmight_map__.getLayersOrder(),
  );
}

/** The window handle of the map under test: the survey map or the Global View's. */
type MapHandle = '__stalagmight_map__' | '__stalagmight_global_map__';

async function loseContext(page: Page, handle: MapHandle = '__stalagmight_map__'): Promise<void> {
  await page.evaluate(async (handle) => {
    const w = window as unknown as Record<string, unknown> & { __e2eLoseContext__?: WEBGL_lose_context };
    const map = w[handle] as { getCanvas: () => HTMLCanvasElement; once: (e: string, cb: () => void) => void };
    const ext = map.getCanvas().getContext('webgl2')!.getExtension('WEBGL_lose_context')!;
    w.__e2eLoseContext__ = ext;
    const lost = new Promise((resolve) => map.once('webglcontextlost', () => resolve(null)));
    ext.loseContext();
    await lost;
  }, handle);
}

async function restoreContext(page: Page, handle: MapHandle = '__stalagmight_map__'): Promise<void> {
  await page.evaluate(async (handle) => {
    const w = window as unknown as Record<string, unknown> & { __e2eLoseContext__: WEBGL_lose_context };
    const map = w[handle] as { once: (e: string, cb: () => void) => void };
    const restored = new Promise((resolve) => map.once('webglcontextrestored', () => resolve(null)));
    w.__e2eLoseContext__.restoreContext();
    await restored;
  }, handle);
}

/** Tag the current map object, to tell a rebuilt map from the old one. */
async function tagMap(page: Page, handle: MapHandle): Promise<void> {
  await page.evaluate((handle) => {
    ((window as unknown as Record<string, unknown>)[handle] as Record<string, unknown>).__e2eTag__ = true;
  }, handle);
}

async function mapIsRebuilt(page: Page, handle: MapHandle): Promise<boolean> {
  return page.evaluate((handle) => {
    const map = (window as unknown as Record<string, unknown>)[handle] as Record<string, unknown> | undefined;
    return Boolean(map) && map!.__e2eTag__ !== true;
  }, handle);
}

const LOST_TOAST = '[data-testid="toast"][data-toast-key="render-context-lost"]:not([data-toast-leaving])';

async function openGlobalViewMap(page: Page): Promise<void> {
  await setupMockedPage(page);
  await expect(page.getByTestId('global-view')).toBeVisible({ timeout: 10000 });
  await page.locator('[data-testid="globalview-view-toggle-map"]').click();
  await expect(page.getByTestId('globalview-map')).toBeVisible({ timeout: 5000 });
  await expect.poll(() => globalLayerOrder(page)).toContain('global-survey-custom');
}

/**
 * Load every Global View system's survey and frame it, so the global survey layer
 * has geometry on screen for {@link globalInk} to find (#576 RV4-FE-MF-7).
 */
async function showGlobalSurveys(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const w = window as unknown as {
      __E2E_TEST__: { globalViewStore: { getState: () => { loadGeometryBatch: (paths: string[]) => Promise<void> } } };
      __stalagmight_global_map__: { fitBounds: (b: [[number, number], [number, number]], o: object) => void };
    };
    await w.__E2E_TEST__.globalViewStore
      .getState()
      .loadGeometryBatch(['/projects/cave-alpha.stalx', '/projects/cave-beta.stalx', '/projects/cave-gamma.stalx']);
  });
  await expect.poll(() => globalShots(page).then((m) => m.length)).toBeGreaterThan(0);
  await page.evaluate(() => {
    const w = window as unknown as {
      __E2E_TEST__: { globalViewStore: { getState: () => { geometryData: Map<string, { shots: Array<{ from: number[]; to: number[] }> }> } } };
      __stalagmight_global_map__: { fitBounds: (b: [[number, number], [number, number]], o: object) => void };
    };
    const pts = [...w.__E2E_TEST__.globalViewStore.getState().geometryData.values()].flatMap((g) =>
      g.shots.flatMap((s) => [s.from, s.to]),
    );
    const lngs = pts.map((p) => p[0]!);
    const lats = pts.map((p) => p[1]!);
    w.__stalagmight_global_map__.fitBounds(
      [[Math.min(...lngs), Math.min(...lats)], [Math.max(...lngs), Math.max(...lats)]],
      { animate: false, padding: 60 },
    );
  });
}

/** Every loaded Global View shot, as `[lng, lat]` endpoints. */
async function globalShots(page: Page): Promise<Array<{ from: [number, number]; to: [number, number] }>> {
  return page.evaluate(() => {
    const w = window as unknown as {
      __E2E_TEST__: { globalViewStore: { getState: () => { geometryData: Map<string, { shots: Array<{ from: number[]; to: number[] }> }> } } };
    };
    return [...w.__E2E_TEST__.globalViewStore.getState().geometryData.values()].flatMap((g) =>
      g.shots.map((s) => ({ from: [s.from[0]!, s.from[1]!] as [number, number], to: [s.to[0]!, s.to[1]!] as [number, number] })),
    );
  });
}

/**
 * How many on-screen Global View shots are drawn: a shot counts when a pixel at
 * its midpoint differs from the pixels 12 px to either side of the line while
 * those two agree with each other (both are the background there). The Global
 * View's base imagery is not one color (the mock's tiles are squares of
 * different greens), so a fixed background sample would count the imagery as
 * ink, and a one-sided one an imagery edge; the line's own two-sided
 * neighbourhood does not. The Global View's counterpart of
 * {@link inkAt}: a global survey layer that is back in the stack but draws
 * nothing reads 0 (#576 RV4-FE-MF-7). `shiftPx` moves every probe point off the
 * survey, for the oracle's own check.
 */
async function globalInk(page: Page, shiftPx = 0): Promise<number> {
  const shots = await globalShots(page);
  return page.evaluate(
    ({ shots, shiftPx }) =>
      new Promise<number>((resolve) => {
        const map = (window as unknown as {
          __stalagmight_global_map__?: {
            getCanvas: () => HTMLCanvasElement;
            project: (p: [number, number]) => { x: number; y: number };
            once: (e: string, cb: () => void) => void;
            triggerRepaint: () => void;
          };
        }).__stalagmight_global_map__;
        if (!map) return resolve(0);
        const canvas = map.getCanvas();
        const gl = canvas.getContext('webgl2') as WebGL2RenderingContext | null;
        if (!gl) return resolve(0);
        const dpr = canvas.width / canvas.clientWidth;
        const inside = (p: { x: number; y: number }) =>
          p.x > 16 && p.y > 16 && p.x < canvas.clientWidth - 16 && p.y < canvas.clientHeight - 16;
        map.once('render', () => {
          const read = (x: number, y: number, size: number) => {
            const n = Math.max(1, Math.round(size * dpr));
            const out = new Uint8Array(n * n * 4);
            gl.readPixels(Math.round(x * dpr - n / 2), Math.round(canvas.height - y * dpr - n / 2), n, n, gl.RGBA, gl.UNSIGNED_BYTE, out);
            return out;
          };
          let drawn = 0;
          for (const s of shots) {
            const a = map.project(s.from);
            const b = map.project(s.to);
            const len = Math.hypot(b.x - a.x, b.y - a.y);
            if (len < 4) continue;
            const nx = -(b.y - a.y) / len;
            const ny = (b.x - a.x) / len;
            const mid = { x: (a.x + b.x) / 2 + nx * shiftPx, y: (a.y + b.y) / 2 + ny * shiftPx };
            const left = { x: mid.x + nx * 12, y: mid.y + ny * 12 };
            const right = { x: mid.x - nx * 12, y: mid.y - ny * 12 };
            if (!inside(mid) || !inside(left) || !inside(right)) continue;
            const differs = (p: Uint8Array, i: number, q: Uint8Array) =>
              Math.abs(p[i]! - q[0]!) > 30 || Math.abs(p[i + 1]! - q[1]!) > 30 || Math.abs(p[i + 2]! - q[2]!) > 30;
            const refA = read(left.x, left.y, 1);
            const refB = read(right.x, right.y, 1);
            if (differs(refA, 0, refB)) continue; // an imagery edge runs between the two sides
            const block = read(mid.x, mid.y, 3);
            for (let i = 0; i < block.length; i += 4) {
              if (differs(block, i, refA)) {
                drawn++;
                break;
              }
            }
          }
          resolve(drawn);
        });
        map.triggerRepaint();
      }),
    { shots, shiftPx },
  );
}

async function globalLayerOrder(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_global_map__?: { getLayersOrder: () => string[] } }).__stalagmight_global_map__;
    try {
      return map?.getLayersOrder() ?? [];
    } catch {
      return []; // no style while the context is lost
    }
  });
}

test.describe('Map survives a WebGL context loss', () => {
  test('survey geometry and labels come back, in their old stacking order', async ({ page }) => {
    const errors = collectErrors(page);
    await openSeededAndZoom(page, 'Context Loss Restore');

    const mids = await shotMidpoints(page);
    await expect.poll(() => inkAt(page, mids)).toBeGreaterThan(0);
    const orderBefore = await layerOrder(page);
    for (const id of CUSTOM_LAYERS) expect(orderBefore).toContain(id);

    await loseContext(page);
    await restoreContext(page);

    await expect
      .poll(async () => (await layerOrder(page)).filter((id) => (CUSTOM_LAYERS as readonly string[]).includes(id)))
      .toEqual(orderBefore.filter((id) => (CUSTOM_LAYERS as readonly string[]).includes(id)));
    // Same stack as before the loss, custom layers at their old positions.
    expect(await layerOrder(page)).toEqual(orderBefore);
    await expect.poll(() => inkAt(page, mids), { timeout: 10000 }).toBeGreaterThan(0);
    expect(errors).toEqual([]);
  });

  test('an edit made while the context is lost is drawn after the restore', async ({ page }) => {
    const errors = collectErrors(page);
    await openSeededAndZoom(page, 'Context Loss Edit');

    await loseContext(page);
    // Longer than the seeded section, so its far end is drawn nowhere else.
    await createSectionViaStore(page, {
      name: 'CaveA Extension',
      direction: 'in',
      shots: Array.from({ length: 6 }, () => ({
        azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -1, depth_out: -2,
      })),
    });
    await restoreContext(page);

    await expect
      .poll(async () => (await getDrawnGeometry(page)).stations.filter((s) => s.stationIndex === 6).length)
      .toBe(1);
    const farEnd = (await getDrawnGeometry(page)).stations.find((s) => s.stationIndex === 6)!;
    await expect.poll(() => inkAt(page, [farEnd]), { timeout: 10000 }).toBeGreaterThan(0);
    expect(errors).toEqual([]);
  });
});

/** Read map style state, or null while there is no style (context lost). */
async function styleState(page: Page): Promise<{ satellite: boolean; grid: boolean; closures: unknown; interactStations: unknown } | null> {
  return page.evaluate(() => {
    const map = (window as unknown as {
      __stalagmight_map__: { getLayer: (id: string) => unknown; getLayoutProperty: (id: string, p: string) => unknown };
    }).__stalagmight_map__;
    try {
      return {
        satellite: Boolean(map.getLayer('satellite-tiles')),
        grid: Boolean(map.getLayer('grid-lines')),
        closures: map.getLayoutProperty('survey-loop-closures', 'visibility'),
        interactStations: map.getLayoutProperty('survey-interact-stations', 'visibility'),
      };
    } catch {
      return null;
    }
  });
}

/** Run store actions in the page, the way the toolbar / Display menu do. */
async function act(page: Page, code: string): Promise<void> {
  await page.evaluate((code) => {
    const w = window as unknown as { __E2E_TEST__: { uiStore: { getState: () => unknown }; projectStore: { getState: () => unknown } } };
    new Function('ui', 'ps', code)(w.__E2E_TEST__.uiStore.getState(), w.__E2E_TEST__.projectStore.getState());
  }, code);
}

test.describe('Map work while the WebGL context is lost (#576 NF-MAP-5)', () => {
  test('throws nothing, and the style catches up after the restore', async ({ page }) => {
    const errors = collectErrors(page);
    await openSeededAndZoom(page, 'Context Lost Edits');
    const before = await styleState(page);
    expect(before).toMatchObject({ satellite: true, grid: false });

    await loseContext(page);
    // Display menu toggles that write the style: satellite off, grid on, stations off.
    await act(page, "ui.toggleDisplay('showSatellite')");
    await act(page, "ui.toggleDisplay('showGrid')");
    await act(page, "ui.toggleDisplay('showStations')");
    // Theme, tools, selection, overlays and modes, which repaint or re-dim.
    await act(page, "ui.setResolvedTheme(ui.resolvedTheme === 'dark' ? 'light' : 'dark')");
    for (const tool of ['move_station', 'move_segment', 'move_shot', 'measure', 'merge', 'comment', 'select']) {
      await act(page, `ui.setActiveTool('${tool}')`);
    }
    await act(page, "ui.setSelection({ type: 'shot', sectionId: ps.project.sections[0].id, shotId: ps.project.sections[0].shots[0].id })");
    await act(page, 'ui.toggleDistanceGradient()');
    await act(page, "ui.setAppMode('plan')");
    await act(page, "ui.setAppMode('survey')");
    await expect(page.getByText('Map encountered an error')).toHaveCount(0);

    await restoreContext(page);
    // The writes skipped during the loss land once the restored style has loaded.
    await expect
      .poll(() => styleState(page))
      .toEqual({ satellite: false, grid: true, closures: 'none', interactStations: 'none' });
    await expect.poll(async () => inkAt(page, await shotMidpoints(page)), { timeout: 10000 }).toBeGreaterThan(0);
    await expect(page.getByText('Map encountered an error')).toHaveCount(0);
    expect(errors).toEqual([]);
  });
});

test.describe('Global View survives a WebGL context loss (#576 D38)', () => {
  test('the global survey layer comes back, in its old stacking order', async ({ page }) => {
    const errors = collectErrors(page);
    await openGlobalViewMap(page);
    await showGlobalSurveys(page);
    await expect.poll(() => globalInk(page)).toBeGreaterThan(0);
    const orderBefore = await globalLayerOrder(page);

    await loseContext(page, '__stalagmight_global_map__');
    await restoreContext(page, '__stalagmight_global_map__');

    await expect.poll(() => globalLayerOrder(page)).toEqual(orderBefore);
    // Back in the stack is not enough: it draws again (#576 RV4-FE-MF-7).
    await expect.poll(() => globalInk(page), { timeout: 10000 }).toBeGreaterThan(0);
    expect(errors).toEqual([]);
  });
});

test.describe('A WebGL context that does not come back (#576 D38-LOST)', () => {
  test('survey map: after ~5 s a toast offers Reload map, which rebuilds the map on a new context', async ({ page }) => {
    const errors = collectErrors(page);
    await openSeededAndZoom(page, 'Context Lost For Good');
    await tagMap(page, '__stalagmight_map__');

    await loseContext(page);
    const toast = page.locator(LOST_TOAST);
    await expect(toast).toBeVisible({ timeout: 10000 });
    await expect(toast).toContainText('Survey rendering unavailable (WebGL2/GPU)');

    await toast.getByTestId('map-context-lost-reload').click();
    await expect.poll(() => mapIsRebuilt(page, '__stalagmight_map__'), { timeout: 10000 }).toBe(true);
    await expect(page.locator(LOST_TOAST)).toHaveCount(0);
    await waitForDrawnShots(page, 3);
    const order = await layerOrder(page);
    for (const id of CUSTOM_LAYERS) expect(order).toContain(id);
    await expect.poll(async () => inkAt(page, await shotMidpoints(page)), { timeout: 10000 }).toBeGreaterThan(0);
    expect(errors).toEqual([]);
  });

  // [#576 RV4-FE-MF-3] The rebuilt map took its center from the survey's bounding
  // box (only the zoom came back), so a user zoomed in on one end of the cave
  // landed somewhere else - possibly on empty ground.
  test('survey map: Reload map keeps the camera where it was', async ({ page }) => {
    await openSeededAndZoom(page, 'Context Lost Camera');
    const camera = () =>
      page.evaluate(() => {
        const m = (window as unknown as {
          __stalagmight_map__: { getCenter: () => { lng: number; lat: number }; getZoom: () => number };
        }).__stalagmight_map__;
        const c = m.getCenter();
        return { lng: c.lng, lat: c.lat, zoom: m.getZoom() };
      });
    // Look at the far end of the survey, away from its bounding-box center.
    await page.evaluate(() => {
      const w = window as unknown as {
        __stalagmight_map__: { jumpTo: (o: { center: [number, number] }) => void };
        __stalagmight_geometry__: () => { stations: Array<{ lngLat: [number, number] }> };
      };
      const east = w.__stalagmight_geometry__().stations.reduce((a, b) => (b.lngLat[0] > a.lngLat[0] ? b : a));
      w.__stalagmight_map__.jumpTo({ center: east.lngLat });
    });
    const before = await camera();
    await tagMap(page, '__stalagmight_map__');

    await loseContext(page);
    const toast = page.locator(LOST_TOAST);
    await expect(toast).toBeVisible({ timeout: 10000 });
    await toast.getByTestId('map-context-lost-reload').click();
    await expect.poll(() => mapIsRebuilt(page, '__stalagmight_map__'), { timeout: 10000 }).toBe(true);
    await waitForDrawnShots(page, 3);

    const after = await camera();
    expect(after.lng).toBeCloseTo(before.lng, 6);
    expect(after.lat).toBeCloseTo(before.lat, 6);
    expect(after.zoom).toBeCloseTo(before.zoom, 6);
  });

  // [#576 W26-NF2] Under Pan, the section row the pointer rests on kept its hover
  // in the store across the rebuild, but the rebuilt map painted none: on load
  // MapCanvas's tool effect runs AFTER MapHoverSync pushed the hover in (a child's
  // effects run first), and SurveyLayer.setActiveTool drops section hover under
  // Pan/Zoom. Reload is pressed from the keyboard so the pointer stays on the row.
  test('survey map: under Pan, Reload map keeps the hovered section row highlighted', async ({ page }) => {
    await seedNavigationTools(page);
    await openSeededAndZoom(page, 'Context Lost Pan Hover');
    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');
    const sectionId = await page.evaluate(() =>
      (window as unknown as {
        __E2E_TEST__: { projectStore: { getState: () => { project: { sections: Array<{ id: number }> } } } };
      }).__E2E_TEST__.projectStore.getState().project.sections[0]!.id,
    );
    // Hover grows the station discs 1.5x, so the 21 px blocks around them hold more ink.
    const discInk = async (): Promise<number> => {
      const bg = await readCanvasPixel(page, 6, 6);
      let total = 0;
      for (const s of (await getDrawnGeometry(page)).stations) total += await countCanvasInk(page, s.x, s.y, bg, 21);
      return total;
    };
    // Read once every tile is in and two reads agree, so a tile still loading (its
    // bare background counts as ink) or a label fading in cannot pass for hover.
    const settledDiscInk = async (): Promise<number> => {
      await expect
        .poll(() =>
          page.evaluate(() =>
            (window as unknown as { __stalagmight_map__: { loaded: () => boolean } }).__stalagmight_map__.loaded(),
          ),
        )
        .toBe(true);
      let ink = -1;
      await expect
        .poll(async () => {
          const previous = ink;
          ink = await discInk();
          return ink === previous;
        })
        .toBe(true);
      return ink;
    };
    const baseInk = await settledDiscInk();
    await page.getByTestId(`section-row-${sectionId}`).hover();
    await expect.poll(discInk).toBeGreaterThan(baseInk);
    const hoverInk = await settledDiscInk();

    await tagMap(page, '__stalagmight_map__');
    await loseContext(page);
    const toast = page.locator(LOST_TOAST);
    await expect(toast).toBeVisible({ timeout: 10000 });
    await toast.getByTestId('map-context-lost-reload').focus();
    await page.keyboard.press('Enter');
    await expect.poll(() => mapIsRebuilt(page, '__stalagmight_map__'), { timeout: 10000 }).toBe(true);
    await waitForDrawnShots(page, 3);
    await waitForActiveTool(page, 'pan');

    const hovered = await page.evaluate(() =>
      (window as unknown as {
        __E2E_TEST__: { uiStore: { getState: () => { hoveredSectionId: number | null } } };
      }).__E2E_TEST__.uiStore.getState().hoveredSectionId,
    );
    expect(hovered).toBe(sectionId);
    // Painted at the hover level, not the bare one (measured 892 vs 484 ink).
    expect(await settledDiscInk()).toBeGreaterThan((baseInk + hoverInk) / 2);
  });

  test('survey map: the toast clears itself when the context comes back after all', async ({ page }) => {
    const errors = collectErrors(page);
    await openSeededAndZoom(page, 'Context Back Late');

    await loseContext(page);
    await expect(page.locator(LOST_TOAST)).toBeVisible({ timeout: 10000 });
    await restoreContext(page);
    await expect(page.locator(LOST_TOAST)).toHaveCount(0);
    await expect.poll(async () => inkAt(page, await shotMidpoints(page)), { timeout: 10000 }).toBeGreaterThan(0);
    expect(errors).toEqual([]);
  });

  test('Global View: the same toast rebuilds the Global View map', async ({ page }) => {
    const errors = collectErrors(page);
    await openGlobalViewMap(page);
    await showGlobalSurveys(page);
    await expect.poll(() => globalInk(page)).toBeGreaterThan(0);
    await tagMap(page, '__stalagmight_global_map__');

    await loseContext(page, '__stalagmight_global_map__');
    const toast = page.locator(LOST_TOAST);
    await expect(toast).toBeVisible({ timeout: 10000 });
    await toast.getByTestId('map-context-lost-reload').click();
    await expect.poll(() => mapIsRebuilt(page, '__stalagmight_global_map__'), { timeout: 10000 }).toBe(true);
    await expect(page.locator(LOST_TOAST)).toHaveCount(0);
    await expect.poll(() => globalLayerOrder(page)).toContain('global-survey-custom');
    // The rebuilt map draws the surveys, not just lists their layer (#576 RV4-FE-MF-7).
    await expect.poll(() => globalInk(page), { timeout: 10000 }).toBeGreaterThan(0);
    expect(errors).toEqual([]);
  });
});
