import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForDrawnStations,
  getDrawnGeometry,
  readCanvasPixel,
  countCanvasInk,
  type PixelRgba,
  type SectionSeed,
} from './helpers/app';

/**
 * Renderer-health canary for the custom WebGL survey renderer (#customRender).
 *
 * After #297 the custom WebGL layer is the SOLE survey renderer: there is no gate
 * and no stock MapLibre fallback (surveyLayer/customRender/gate.ts). If the WebGL
 * path fails to initialize or self-disables at runtime, the survey silently
 * vanishes with nothing behind it. The rest of the e2e suite runs on this renderer
 * implicitly, but no spec asserts the WebGL path is actually healthy - so a
 * regression into self-disable could pass every pixel-agnostic spec while shipping
 * an invisible survey.
 *
 * This spec is that missing assertion. It boots the app on the standard mocked
 * harness (headless Chromium -> SwiftShader WebGL2) and proves:
 *   1. WebGL2 came up and drew a frame without a GL error - the one-shot lifecycle
 *      logs `[customRender] enabled` and `[customRender] first frame drawn OK`
 *      fired. (These fire on the initial paint, which happens on the empty project
 *      before geometry is seeded - so on their own they only prove the context is
 *      alive, not that the survey rendered.)
 *   2. The seeded survey geometry actually populated the custom layer - the survey
 *      layer's own geometry enumeration reports nonzero stations AND shot segments
 *      (this is the survey layer, distinct from the label layer).
 *   3. The seeded survey actually painted - a canvas-ink check finds real pixels at
 *      an interior shot midpoint. A shot midpoint is survey-LINE geometry with no
 *      label glyphs over it, so this proves the SURVEY layer drew, not labels.
 *   4. The survey layer never self-disabled or failed to initialize - checked LAST,
 *      after the ink probes, because `readCanvasPixel`/`countCanvasInk` each call
 *      `map.triggerRepaint()` and the layer's `gl.getError()` check runs during
 *      those frames; a self-disable triggered by a probe frame must be observed.
 *
 * Kept @smoke so a WebGL regression trips the merge gate, not just the nightly.
 * Stability-validated (10x, 4 workers, 0 flaky) before being placed at smoke.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Renderer Health Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5, left: 1.5, right: 2.3, up: 3.0, down: 0.8 },
      { azimuth_in: 90, azimuth_out: 270, length: 15.2, depth_in: -2.5, depth_out: -5.0, left: 0.5, right: 0.7, up: 1.2, down: 1.5 },
    ],
  },
];

test.describe('Custom renderer health', () => {
  test('custom WebGL survey layer initializes and draws, never self-disables @smoke', async ({ page }) => {
    // Capture the renderer's one-shot lifecycle logs from the very first frame.
    // These fire once, during the initial paint, so the listener must be attached
    // before boot.
    const info: string[] = [];
    const warnings: string[] = [];
    page.on('console', (msg) => {
      const text = msg.text();
      if (!text.includes('[customRender]')) return;
      if (msg.type() === 'warning' || msg.type() === 'error') warnings.push(text);
      else info.push(text);
    });

    await setupMockedPage(page);
    await createProject(page, 'Renderer Health');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);

    // (1) The WebGL context came up and drew a frame: both one-shot logs fired.
    await expect
      .poll(() => info.some((t) => t.includes('[customRender] enabled')), { timeout: 10000 })
      .toBe(true);
    await expect
      .poll(() => info.some((t) => t.includes('[customRender] first frame drawn OK')), { timeout: 10000 })
      .toBe(true);

    // (2) The seeded survey geometry actually reached the custom SURVEY layer (not
    // just the label layer): its enumeration reports real stations and segments.
    const geom = await getDrawnGeometry(page);
    expect(geom.stations.length, 'survey layer enumerates no seeded stations').toBeGreaterThan(0);
    expect(geom.shots.length, 'survey layer enumerates no seeded shot segments').toBeGreaterThan(0);

    // (3) The seeded survey actually painted. Sample at an interior shot midpoint:
    // survey-LINE geometry, label-free, so ink here is the survey layer (not
    // labels), and an interior point keeps the fixed-size read block inside the
    // canvas (an out-of-bounds readPixels returns zeros that would be false ink).
    // Background is sampled from an empty corner (theme-independent).
    const shot = geom.shots[Math.floor(geom.shots.length / 2)]!; // in range: length > 0 asserted above
    const bg: PixelRgba = await readCanvasPixel(page, 8, 8);
    const ink = await countCanvasInk(page, Math.round(shot.mid.x), Math.round(shot.mid.y), bg, 9);
    expect(ink, 'expected visible custom-rendered survey ink at a shot midpoint').toBeGreaterThan(0);

    // (4) Checked LAST: the ink probes above each triggered a repaint, and the
    // layer's gl.getError() self-disable check runs during those frames. Flush two
    // animation frames so any probe-triggered self-disable has been logged, then
    // require the survey layer reported no failure at any point. The WebGL2-
    // unavailable clause is scoped to `disabling custom layer` so an isolated
    // LABEL-layer fallback (`disabling custom label layer`) does not trip this.
    await page.evaluate(
      () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
    );
    const surveyFailure = warnings.filter(
      (t) =>
        t.includes('custom layer self-disabled') ||
        t.includes('custom layer failed to initialize') ||
        t.includes('WebGL2 unavailable; disabling custom layer'),
    );
    expect(surveyFailure, `custom survey layer reported a failure: ${surveyFailure.join(' | ')}`).toEqual([]);
  });
});
