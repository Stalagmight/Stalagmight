import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithSections,
  getShotScreenPositions,
  getStationScreenPositions,
  waitForDrawnShots,
  waitForDrawnStations,
  readCanvasPixel,
  type PixelRgba,
  type SectionSeed,
  type ShotScreenPos,
} from './helpers/app';
import { recordIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';

/**
 * E2E tests for depth gradient functionality.
 * Tests toggle behavior, depth legend visibility, and settings persistence.
 */

/**
 * Helper to create a new project with mock sections/stations
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Depth Gradient A',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 12, depth_in: 0, depth_out: -6 },
      { azimuth_in: 90, azimuth_out: 270, length: 8, depth_in: -6, depth_out: -12 },
    ],
  },
  {
    name: 'Depth Gradient B',
    direction: 'out',
    shots: [
      { azimuth_in: 180, azimuth_out: 0, length: 6, depth_in: -12, depth_out: -3 },
    ],
  },
];

/**
 * Helper to open the Display menu
 */
async function openDisplayMenu(page: Page): Promise<void> {
  const displayButton = page.locator('button:has-text("Display")');
  await displayButton.click();
  await expect(page.locator('[data-testid="display-toggle-showStations"]')).toBeVisible({ timeout: 2000 });
}

/**
 * Helper to get the toggle checkbox state
 */
async function isToggleOn(page: Page, toggleKey: string): Promise<boolean> {
  const toggle = page.locator(`[data-testid="display-toggle-${toggleKey}"]`);
  return (await toggle.getAttribute('aria-pressed')) === 'true';
}

/**
 * Helper to toggle a display option and wait for state change
 */
async function toggleDisplayOption(page: Page, toggleKey: string): Promise<void> {
  const colorKeys = ['showDepthGradient', 'showDistanceGradient', 'showIssueSeverity'];
  const currentState = await isToggleOn(page, toggleKey);
  // Color modes are single-select: turn one OFF by selecting Section, not by re-clicking the pill.
  if (currentState && colorKeys.includes(toggleKey)) {
    await page.locator('[data-testid="display-color-section"]').click();
  } else {
    await page.locator(`[data-testid="display-toggle-${toggleKey}"]`).click();
  }
  await expect.poll(async () => isToggleOn(page, toggleKey), { timeout: 2000 }).not.toBe(currentState);
}

/**
 * Helper to wait for map to be loaded
 */
async function waitForMapLoaded(page: Page): Promise<void> {
  await expect.poll(async () => {
    const loaded = await page.evaluate(() => {
      const map = (window as unknown as { __stalagmight_map__?: { loaded: () => boolean } }).__stalagmight_map__;
      return map?.loaded() ?? false;
    });
    return loaded;
  }, { timeout: 10000 }).toBe(true);
}

// ---------------------------------------------------------------------------
// Painted-pixel color probes (custom WebGL renderer)
// ---------------------------------------------------------------------------
//
// The survey is drawn by a custom WebGL layer: every shot's / station's color is
// resolved at build time and packed into a vertex attribute. There is no
// `survey-stations` / `survey-shots` layer left to read a `circle-color` /
// `line-color` paint off, and no per-feature `colorIndex` to resolve through it,
// so the painted pixel is the color oracle.
//
// Depth mode packs PER-ENDPOINT shot colors and lets the GPU interpolate between
// them along the segment (colorResolvers.ts `useDepthEndpoints`), which is exactly
// what the deleted `survey-data-shot-gradients` sub-segment source existed to
// produce. So "the shot's color ramps with depth" is now read as "the pixels near
// its two ends are painted different colors, with the midpoint in between".

interface MapZoomHandle {
  getZoom: () => number;
  setZoom: (z: number) => void;
}

/**
 * Zoom in before probing painted pixels. The seeded survey is only a few metres
 * across, so at the fit-to-bounds zoom the whole thing is ~9 screen px wide and
 * the station discs cover the shot lines entirely, so a probe there cannot tell a
 * shot's color from a station's, and two different shots can land on the same
 * pixel. `setZoom` is a jump (no animation) and the geometry helpers project with
 * the live map, so positions read after this call already match the new view.
 *
 * A fit-to-bounds queued by the seed can still land after `map.loaded()` and take
 * the zoom back, so hold the target zoom until the drawn shots really are spread
 * out on screen (`minSpanPx`).
 */
async function zoomIn(page: Page, delta = 4, minSpanPx = 40): Promise<void> {
  const target = await page.evaluate((d) => {
    const map = (window as unknown as { __stalagmight_map__?: MapZoomHandle }).__stalagmight_map__;
    if (!map) return 0;
    const z = map.getZoom() + d;
    map.setZoom(z);
    return z;
  }, delta);
  await expect.poll(async () => {
    await page.evaluate((z) => {
      const map = (window as unknown as { __stalagmight_map__?: MapZoomHandle }).__stalagmight_map__;
      if (map && map.getZoom() < z - 0.01) map.setZoom(z);
    }, target);
    const shots = await getShotScreenPositions(page);
    return shots.reduce((m, s) => Math.max(m, Math.hypot(s.to.x - s.from.x, s.to.y - s.from.y)), 0);
  }, { timeout: 10000 }).toBeGreaterThan(minSpanPx);
}

/** Stable key for a shot's identity (`getShotScreenPositions` order is not). */
function shotKey(shot: ShotScreenPos): string {
  return `${shot.sectionId}:${shot.shotId}`;
}

/**
 * Painted color a fraction `t` along a drawn shot (0 = from endpoint, 1 = to).
 * Stay clear of 0 / 1: a station disc paints over the line ends.
 */
async function shotPixelAt(page: Page, shot: ShotScreenPos, t: number): Promise<PixelRgba> {
  return readCanvasPixel(
    page,
    shot.from.x + (shot.to.x - shot.from.x) * t,
    shot.from.y + (shot.to.y - shot.from.y) * t,
  );
}

/** True when two painted samples differ by more than `tolerance` on any channel. */
function differs(a: PixelRgba, b: PixelRgba, tolerance = 8): boolean {
  return (
    Math.abs(a[0] - b[0]) > tolerance ||
    Math.abs(a[1] - b[1]) > tolerance ||
    Math.abs(a[2] - b[2]) > tolerance
  );
}

/**
 * True when `mid` lies between `a` and `b` on every channel (within `slack`),
 * i.e. the color really interpolates along the segment rather than stepping.
 */
function isBetween(mid: PixelRgba, a: PixelRgba, b: PixelRgba, slack = 6): boolean {
  for (let c = 0; c < 3; c++) {
    // In range: c < 3 indexes the r/g/b members of the 4-channel PixelRgba tuple.
    const lo = Math.min(a[c]!, b[c]!) - slack;
    const hi = Math.max(a[c]!, b[c]!) + slack;
    if (mid[c]! < lo || mid[c]! > hi) return false;
  }
  return true;
}

/** Painted fill of every drawn station, keyed `sectionId:stationIndex`. */
async function stationFills(page: Page): Promise<Record<string, PixelRgba>> {
  const out: Record<string, PixelRgba> = {};
  for (const st of await getStationScreenPositions(page)) {
    out[`${st.sectionId}:${st.stationIndex}`] = await readCanvasPixel(page, st.x, st.y);
  }
  return out;
}

test.describe('Depth Gradient', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Depth Gradient Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapLoaded(page);
  });

  test('toggle enables depth coloring on map', async ({ page }) => {
    // Open Display menu
    await openDisplayMenu(page);

    // Verify depth gradient is OFF by default
    expect(await isToggleOn(page, 'showDepthGradient')).toBe(false);

    // Verify store state matches
    const depthGradientBefore = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { showDepthGradient: boolean } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.showDepthGradient ?? false;
    });
    expect(depthGradientBefore).toBe(false);

    // Wait for the stations to be drawn, then capture their PAINTED fill before
    // enabling depth coloring. Zoom in first so a station's disc is wider than
    // the shot lines crossing it (see zoomIn).
    await zoomIn(page);
    await waitForDrawnStations(page, 1);
    const colorsBefore = await stationFills(page);

    // Ensure we have stations to compare
    expect(Object.keys(colorsBefore).length).toBeGreaterThan(0);

    // Toggle depth gradient ON
    await toggleDisplayOption(page, 'showDepthGradient');

    // Verify toggle state changed
    expect(await isToggleOn(page, 'showDepthGradient')).toBe(true);

    // Verify store state changed
    const depthGradientAfter = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { showDepthGradient: boolean } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.showDepthGradient ?? false;
    });
    expect(depthGradientAfter).toBe(true);

    // Verify the depth legend is visible (rendering indicator)
    await expect(page.locator('[data-testid="depth-legend"]')).toBeVisible();

    // Poll for the painted station fills to change after enabling depth gradient.
    // Every station moves off the flat section color onto its depth-ramp color, so
    // require all of them to repaint (not just one).
    await expect.poll(async () => {
      const colorsAfter = await stationFills(page);
      const keys = Object.keys(colorsBefore);
      // Keep polling if the geometry is momentarily unavailable (map still loading)
      if (keys.some((k) => colorsAfter[k] === undefined)) return 'waiting';
      return keys.every((k) => differs(colorsBefore[k]!, colorsAfter[k]!)) ? 'changed' : 'waiting';
    }, { timeout: 3000 }).toBe('changed');
  });

  test('shots use endpoint depth colors for line gradients', async ({ page }) => {
    // The old oracle for this was the `survey-data-shot-gradients` vector-tile
    // source: one feature per gradient sub-segment, each with its own colorIndex,
    // empty outside depth mode. That source is gone. Depth mode now packs the
    // shot's from/to endpoint colors into its vertices and the fragment shader
    // interpolates between them, so the observable is the PAINTED PIXEL at points
    // along the shot: flat before, ramping after.
    await zoomIn(page);
    const shots = await waitForDrawnShots(page, 3);

    // Sanity check + baseline: in default (non-depth) mode every shot is one solid
    // color for its whole length. Sample away from the endpoints, because a station disc
    // paints over the line ends.
    const flatMid: Record<string, PixelRgba> = {};
    for (const shot of shots) {
      const near = await shotPixelAt(page, shot, 0.3);
      const mid = await shotPixelAt(page, shot, 0.5);
      const far = await shotPixelAt(page, shot, 0.7);
      expect(differs(near, far), `shot ${shotKey(shot)} must be flat before depth mode`).toBe(false);
      expect(differs(near, mid), `shot ${shotKey(shot)} must be flat before depth mode`).toBe(false);
      flatMid[shotKey(shot)] = mid;
    }

    const depthGradientBefore = await page.evaluate(() => {
      const stores = (window as unknown as {
        __E2E_TEST__?: {
          uiStore: { getState: () => { showDepthGradient: boolean } };
        };
      }).__E2E_TEST__;
      return stores?.uiStore.getState().showDepthGradient ?? null;
    });
    expect(depthGradientBefore).toBe(false);

    await page.evaluate(() => {
      const stores = (window as unknown as {
        __E2E_TEST__?: {
          uiStore: { getState: () => { toggleDisplay: (key: string) => void } };
        };
      }).__E2E_TEST__;
      if (!stores) throw new Error('__E2E_TEST__ stores not found');
      stores.uiStore.getState().toggleDisplay('showDepthGradient');
    });

    await expect.poll(async () => {
      const enabled = await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { showDepthGradient: boolean } };
          };
        }).__E2E_TEST__;
        return stores?.uiStore.getState().showDepthGradient ?? false;
      });
      return enabled;
    }, { timeout: 2000 }).toBe(true);

    // Wait for the depth build to land on the canvas: every shot repaints away
    // from its flat section color.
    await expect.poll(async () => {
      const now = await getShotScreenPositions(page);
      if (now.length !== shots.length) return false;
      for (const shot of now) {
        const mid = await shotPixelAt(page, shot, 0.5);
        if (!differs(mid, flatMid[shotKey(shot)]!)) return false;
      }
      return true;
    }, { timeout: 4000 }).toBe(true);

    // Every seeded shot has a different depth at each end, so the ramp must be
    // visible ALONG the shot: the two ends are painted different colors and the
    // midpoint sits between them (this is what the deleted per-segment gradient
    // source used to produce).
    const samples: Array<{ shot: string; slot: string; mid: string }> = [];
    for (const shot of await getShotScreenPositions(page)) {
      const near = await shotPixelAt(page, shot, 0.3);
      const mid = await shotPixelAt(page, shot, 0.5);
      const far = await shotPixelAt(page, shot, 0.7);
      expect(differs(near, far), `shot ${shotKey(shot)} endpoints must differ in depth mode`).toBe(true);
      expect(isBetween(mid, near, far), `shot ${shotKey(shot)} midpoint must interpolate`).toBe(true);
      samples.push({
        shot: shotKey(shot),
        slot: `${Math.round(shot.from.x)},${Math.round(shot.from.y)}->${Math.round(shot.to.x)},${Math.round(shot.to.y)}`,
        mid: mid.slice(0, 3).join(','),
      });
    }

    // ...and shots at different depths are painted different colors from each other.
    // Only shots that occupy DISTINCT screen geometry are comparable: the e2e mock
    // lays every section's stations out on the same synthetic line (`x: i * 10,
    // y: 0` in mocks/tauri-ipc.ts, azimuth/length ignored), so a second section's
    // first leg is drawn exactly on top of the first section's and a pixel there
    // cannot be attributed to either. Deduping by screen slot keeps the real
    // comparison (the same section's consecutive shots, which sit at different
    // depths) while dropping the pairs the mock made unobservable.
    const bySlot = new Map<string, { shot: string; mid: string }>();
    for (const s of samples) if (!bySlot.has(s.slot)) bySlot.set(s.slot, s);
    const diag = samples.map((s) => `${s.shot}@${s.slot}=${s.mid}`).join(' ');
    expect(bySlot.size, diag).toBeGreaterThan(1);
    expect(new Set([...bySlot.values()].map((s) => s.mid)).size, diag).toBe(bySlot.size);
  });

  test('toggle shows the depth legend, and hides it again', async ({ page }) => {
    // [#576 E17-D1] One test for what three asserted separately (shown when enabled,
    // hidden when disabled, both immediately): hidden before, shown on, hidden off.
    const legend = page.locator('[data-testid="depth-legend"]');
    await expect(legend).not.toBeVisible();

    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showDepthGradient');
    await expect(legend).toBeVisible();

    await toggleDisplayOption(page, 'showDepthGradient');
    await expect(legend).not.toBeVisible();
  });

  test('depth gradient toggle is independent of other display toggles', async ({ page }) => {
    await openDisplayMenu(page);

    // Enable depth gradient
    await toggleDisplayOption(page, 'showDepthGradient');
    expect(await isToggleOn(page, 'showDepthGradient')).toBe(true);

    // Verify other toggles remain unchanged
    expect(await isToggleOn(page, 'showStations')).toBe(true);
    expect(await isToggleOn(page, 'showShots')).toBe(true);
    expect(await isToggleOn(page, 'showSatellite')).toBe(true);
    expect(await isToggleOn(page, 'showGrid')).toBe(false);

    // Toggle stations off
    await toggleDisplayOption(page, 'showStations');

    // Depth gradient should still be on
    expect(await isToggleOn(page, 'showDepthGradient')).toBe(true);
    await expect(page.locator('[data-testid="depth-legend"]')).toBeVisible();
  });

  // Note: Reload persistence cannot be tested with mocks because the mock's
  // in-memory storage is re-initialized on page reload. Config persistence is
  // pinned against the real backend in the Rust config tier (crates/core config
  // tests + loading paths).
  test('toggling saves showDepthGradient to the backend, on and then off', async ({ page }) => {
    // [#576 E17-D1] One test for the two that scraped the mock's console log; this one
    // reads the recorded IPC calls, so it sees the key and the value the app sent.
    await recordIpcCalls(page, ['save_display_setting']);

    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showDepthGradient');
    await expect(page.locator('[data-testid="depth-legend"]')).toBeVisible();
    const [on] = await waitForIpcCall(page, 'save_display_setting');
    // The write also carries the project's identity (#576 RV6b-2), pinned in vitest
    // and Rust; this test is about the key and the value the toggle sends.
    expect(on.args).toMatchObject({ key: 'showDepthGradient', value: 'true' });

    await toggleDisplayOption(page, 'showDepthGradient');
    const calls = await waitForIpcCall(page, 'save_display_setting', 2);
    expect(
      calls
        .map((c) => c.args)
        .filter((a) => a.key === 'showDepthGradient')
        .map((a) => ({ key: a.key, value: a.value })),
    ).toEqual([
      { key: 'showDepthGradient', value: 'true' },
      { key: 'showDepthGradient', value: 'false' },
    ]);
  });
});

test.describe('Depth Gradient with Menu Interactions', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Depth Gradient Menu Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapLoaded(page);
  });

  test('toggle states persist when menu is closed and reopened', async ({ page }) => {
    // Open menu and enable depth gradient
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showDepthGradient');
    expect(await isToggleOn(page, 'showDepthGradient')).toBe(true);

    // Close menu
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-testid="display-toggle-showDepthGradient"]')).not.toBeVisible();

    // Reopen menu
    await openDisplayMenu(page);

    // Verify state is preserved
    expect(await isToggleOn(page, 'showDepthGradient')).toBe(true);
  });

  test('depth gradient label shows in Display menu', async ({ page }) => {
    await openDisplayMenu(page);

    // Verify the pill has the proper label + descriptive tooltip
    const toggle = page.locator('[data-testid="display-toggle-showDepthGradient"]');
    await expect(toggle).toContainText('Depth');
    await expect(toggle).toHaveAttribute('title', 'Color by depth');
  });
});
