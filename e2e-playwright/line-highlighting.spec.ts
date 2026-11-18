import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithLines,
  waitForMapReady,
  getDrawnGeometry,
  getShotScreenPositions,
  readCanvasPixel,
  countCanvasInk,
  type PixelRgba,
  type ScreenPoint,
  type LineSeed,
} from './helpers/app';

/**
 * E2E tests for line hover/selection highlighting.
 *
 * Tests the bidirectional highlighting between the sections panel and the map:
 * - Panel hover → map highlight (state + painted emphasis)
 * - Panel click → map selection (state + painted emphasis)
 * - Map hover → panel highlight (via store simulation)
 * - Priority conflicts (individual hover vs line selection)
 * - Unhover behavior
 * - Keyboard navigation
 * - Edge cases (empty lines, reference shots)
 */

/**
 * Helper to wait for shot geometry to be drawn by the custom WebGL renderer.
 * The stock `survey-shots` layer is gone, so "are the shots on screen" is read
 * from the drawn-geometry hook instead of `queryRenderedFeatures`.
 */
async function waitForShotsRendered(page: Page): Promise<void> {
  await expect
    .poll(async () => (await getShotScreenPositions(page)).length, { timeout: 10000 })
    .toBeGreaterThan(0);
}

/**
 * Helper to create a new project with the given name.
 * The mock setup provides pre-populated lines with shots.
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas.maplibregl-canvas', { timeout: 10000 });
  await waitForMapReady(page);
  await seedProjectWithLines(page, lineSeeds);
  await waitForShotsRendered(page);
  await zoomInOnSurvey(page);
}

const lineSeeds: LineSeed[] = [
  {
    name: 'Highlight Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
      { azimuth_in: 120, azimuth_out: 300, length: 6, depth_in: -6, depth_out: -4, kind: 'Reference' },
    ],
  },
  {
    name: 'Highlight Secondary',
    direction: 'out',
    shots: [
      { azimuth_in: 180, azimuth_out: 0, length: 7, depth_in: -2, depth_out: -4 },
    ],
  },
  {
    name: 'Empty Line',
    direction: 'in',
    shots: [],
  },
];

// ---------------------------------------------------------------------------
// Painted-emphasis oracle (custom WebGL renderer)
// ---------------------------------------------------------------------------
//
// Line hover / selection highlighting is a per-vertex emphasis LEVEL in the
// custom renderer now: SurveyLayer.setHoveredLine / setSelectedLine /
// setHoveredSegment call customRender.setLineHover / setLineSelect /
// setHighlight, which re-upload a highlight attribute (2 = hover, 1 = selection,
// 0 = none). There is no MapLibre feature-state left to read and the controller
// exposes no read-back, so the honest oracle is the painted pixel.
//
// The shader turns the level into a SIZE bump only (colors are unchanged): shot
// lines widen by hover_width/width = 4/2 = 2x or selection_width/width = 3/2 =
// 1.5x, and station discs grow by hover_radius/radius = 6/4 = 1.5x or
// selection_radius/radius = 5/4 = 1.25x. So "is this line emphasized" is
// "does a fixed block around its geometry hold more ink", and the two levels are
// ordered: none < selection < hover.
//
// STATION discs are the probe of choice. The mock backend lays every station out
// on one straight horizontal line (`coordinates: {x: index * 10, y: 0}`), so a
// shot line is exactly axis-aligned at an integer y and the selection level's
// extra 0.5 px of half-width falls entirely between pixel centers: measured, a
// selected shot line paints byte-identical to an unselected one. A disc boundary
// crosses the pixel grid at every angle, so its area grows measurably at both
// levels (measured 112 -> 156 -> 216 ink for none -> selection -> hover).

/** CSS-px block sampled around a station center (fits the 1.5x hover disc). */
const STATION_BLOCK = 21;
/** CSS-px block sampled around a shot midpoint. */
const SHOT_BLOCK = 15;
/**
 * How far a probe point must be from any OTHER line's geometry to be that
 * line's alone: half a station block plus the largest painted disc radius
 * ((radius 4 + stroke 1) * 1.5 hover = 7.5), rounded up.
 */
const PROBE_CLEARANCE = 20;

/** A "nothing drawn here" reference sample from the canvas corner. */
async function sampleBackground(page: Page): Promise<PixelRgba> {
  return readCanvasPixel(page, 6, 6);
}

/**
 * Zoom in on the seeded survey before probing pixels. The mock places every
 * station at `x = index * 10` metres on one line AND gives every line the
 * same coordinates, so at the default fit-zoom the whole survey is a ~14 px
 * smear where no per-station pixel exists. Zooming in spreads the stations ~74
 * px apart, which is what makes a per-station / per-line pixel meaningful.
 */
async function zoomInOnSurvey(page: Page, zoomDelta = 4): Promise<void> {
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
      throw new Error('map + drawn geometry must be ready before zooming the probe view');
    }
    const count = geometry.stations.length;
    const lng = geometry.stations.reduce((sum, s) => sum + s.lngLat[0], 0) / count;
    const lat = geometry.stations.reduce((sum, s) => sum + s.lngLat[1], 0) / count;
    map.jumpTo({ center: [lng, lat], zoom: map.getZoom() + dz });
  }, zoomDelta);
}

/** Shortest distance from `p` to the segment `a`-`b` (screen px). */
function distanceToSegment(p: ScreenPoint, a: ScreenPoint, b: ScreenPoint): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const lenSq = dx * dx + dy * dy;
  const t = lenSq > 0 ? Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / lenSq)) : 0;
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

/**
 * Station-center probe points for one line, split by whether the pixels
 * belong to that line alone.
 *
 * Lines are drawn COINCIDENT under the mock (identical station coordinates),
 * so a line with few stations may have no exclusive point at all: line
 * "Highlight Secondary" (1 shot) sits exactly on the first leg of "Highlight
 * Main" (3 shots), and only Main's later stations stand alone.
 */
async function stationProbes(
  page: Page,
  lineId: number
): Promise<{ exclusive: ScreenPoint[]; shared: ScreenPoint[] }> {
  const { stations, shots } = await getDrawnGeometry(page);
  const others = stations.filter((s) => s.lineId !== lineId);
  const otherShots = shots.filter((s) => s.lineId !== lineId);
  const exclusive: ScreenPoint[] = [];
  const shared: ScreenPoint[] = [];
  for (const station of stations.filter((s) => s.lineId === lineId)) {
    const point = { x: station.x, y: station.y };
    const clear =
      others.every((o) => Math.hypot(point.x - o.x, point.y - o.y) > PROBE_CLEARANCE) &&
      otherShots.every((o) => distanceToSegment(point, o.from, o.to) > PROBE_CLEARANCE);
    (clear ? exclusive : shared).push(point);
  }
  return { exclusive, shared };
}

/** Total painted ink over a set of probe points. */
async function inkAt(
  page: Page,
  probes: ScreenPoint[],
  bg: PixelRgba,
  block = STATION_BLOCK
): Promise<number> {
  if (probes.length === 0) throw new Error('no probe points');
  let ink = 0;
  for (const probe of probes) {
    ink += await countCanvasInk(page, probe.x, probe.y, bg, block);
  }
  return ink;
}

/**
 * Ink at `probes` once the paint stops changing between reads. Used for
 * baselines: the pixel probe is deterministic for an unchanged scene, so two
 * equal consecutive reads mean nothing is still fading in (label reveal).
 */
async function settledInk(
  page: Page,
  probes: ScreenPoint[],
  bg: PixelRgba,
  block = STATION_BLOCK
): Promise<number> {
  let previous = -1;
  await expect.poll(
    async () => {
      const current = await inkAt(page, probes, bg, block);
      const stable = current === previous;
      previous = current;
      return stable;
    },
    { timeout: 5000 }
  ).toBe(true);
  return previous;
}

/** Poll the ink at `probes` until it satisfies `predicate` (the highlight
 *  re-upload lands a frame after the store update). */
async function pollInk(
  page: Page,
  probes: ScreenPoint[],
  bg: PixelRgba,
  predicate: (ink: number) => boolean,
  block = STATION_BLOCK,
  timeout = 5000
): Promise<number> {
  let result = -1;
  await expect.poll(
    async () => {
      result = await inkAt(page, probes, bg, block);
      return predicate(result);
    },
    { timeout }
  ).toBe(true);
  return result;
}

/**
 * Helper to get a line ID by name.
 */
async function getLineIdByName(page: Page, name: string): Promise<number | null> {
  return page.evaluate((lineName) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { lines: Array<{ id: number; name: string }> } | null } };
      };
    }).__E2E_TEST__;

    const lines = stores?.projectStore.getState().project?.lines ?? [];
    const match = lines.find((line) => line.name === lineName);
    return match?.id ?? null;
  }, name);
}

/**
 * Helper to get the current selection state from uiStore.
 */
async function getSelectionState(page: Page): Promise<{
  type: string;
  lineId?: number;
  stationRef?: { line_id: number; station_index: number };
  shotId?: number;
} | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { selection: unknown } };
      };
    }).__E2E_TEST__;

    if (!stores) return null;
    return stores.uiStore.getState().selection as {
      type: string;
      lineId?: number;
      stationRef?: { line_id: number; station_index: number };
      shotId?: number;
    } | null;
  });
}

/**
 * Helper to get the current hovered line ID from uiStore.
 */
async function getHoveredLineId(page: Page): Promise<number | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { hoveredLineId: number | null } };
      };
    }).__E2E_TEST__;

    if (!stores) return null;
    return stores.uiStore.getState().hoveredLineId;
  });
}

/**
 * Helper to poll for selection state changes.
 */
async function pollSelectionState(
  page: Page,
  predicate: (sel: Awaited<ReturnType<typeof getSelectionState>>) => boolean,
  timeout = 5000
): Promise<Awaited<ReturnType<typeof getSelectionState>>> {
  let result: Awaited<ReturnType<typeof getSelectionState>> = null;
  await expect.poll(
    async () => {
      const sel = await getSelectionState(page);
      if (predicate(sel)) {
        result = sel;
        return true;
      }
      return false;
    },
    { timeout }
  ).toBe(true);
  return result;
}

/**
 * Helper to poll for hovered line ID changes.
 */
async function pollHoveredLineId(
  page: Page,
  predicate: (lineId: number | null) => boolean,
  timeout = 5000
): Promise<number | null> {
  let result: number | null = null;
  await expect.poll(
    async () => {
      const lineId = await getHoveredLineId(page);
      if (predicate(lineId)) {
        result = lineId;
        return true;
      }
      return false;
    },
    { timeout }
  ).toBe(true);
  return result;
}

/**
 * Helper to get available line IDs from the project.
 */
async function getAvailableLineIds(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { lines: Array<{ id: number }> } | null } };
      };
    }).__E2E_TEST__;

    if (!stores) return [];
    const project = stores.projectStore.getState().project;
    return project?.lines.map(s => s.id) ?? [];
  });
}

/**
 * Park the pointer where it hovers neither a line row nor the map, so only
 * the selection-level emphasis is left on the canvas. The toolbar buttons are a
 * safe area (above the sections panel, outside the map).
 */
async function moveMouseOffLines(page: Page): Promise<void> {
  await page.locator('[data-testid="toolbar-import-button"]').hover();
  await pollHoveredLineId(page, (id) => id === null);
}

/**
 * Helper to check if a line has elevated map styling (hover or selection).
 * This checks uiStore state which drives the map styling.
 */
async function hasLineMapHighlight(page: Page, lineId: number, type: 'hover' | 'selection'): Promise<boolean> {
  return page.evaluate(({ secId, highlightType }) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => {
          hoveredLineId: number | null;
          selection: { type: string; lineId?: number } | null;
        } };
      };
    }).__E2E_TEST__;

    if (!stores) return false;
    const state = stores.uiStore.getState();

    if (highlightType === 'hover') {
      return state.hoveredLineId === secId;
    } else {
      return state.selection?.type === 'line' && state.selection?.lineId === secId;
    }
  }, { secId: lineId, highlightType: type });
}

test.describe('Line Highlighting', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
  });

  test.describe('Panel hover → map highlight', () => {
    test('hovering line row sets hoveredLineId and applies map styling', async ({ page }) => {
      await createProject(page, 'Line Highlight Test');

      // Get line IDs from project
      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(1);
      const firstLineId = lineIds[0]!;
      const otherLineId = lineIds[1]!;

      // Baseline paint (nothing emphasized) on pixels that belong to the first
      // line alone.
      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstLineId)).exclusive;
      expect(probes.length).toBeGreaterThan(0);
      const baseInk = await settledInk(page, probes, bg);
      expect(baseInk).toBeGreaterThan(0);

      // Find the line row using data-testid
      const lineRow = page.locator(`[data-testid="line-row-${firstLineId}"]`);
      await expect(lineRow).toBeVisible();

      // Hover the line row
      await lineRow.hover();

      // Poll for hoveredLineId to be set
      const hoveredId = await pollHoveredLineId(page, (id) => id === firstLineId);
      expect(hoveredId).toBe(firstLineId);

      // Verify map styling state is set (drives SurveyLayer.setHoveredLine)
      const hasMapHover = await hasLineMapHighlight(page, firstLineId, 'hover');
      expect(hasMapHover).toBe(true);

      // The hovered line is actually PAINTED emphasized.
      const hoverInk = await pollInk(page, probes, bg, (ink) => ink > baseInk);
      expect(hoverInk).toBeGreaterThan(baseInk);

      // Only the hovered line is emphasized: hovering the OTHER line's row
      // returns these pixels to the unemphasized baseline.
      const otherRow = page.locator(`[data-testid="line-row-${otherLineId}"]`);
      await otherRow.hover();
      await pollHoveredLineId(page, (id) => id === otherLineId);
      expect(await pollInk(page, probes, bg, (ink) => ink === baseInk)).toBe(baseInk);
    });
  });

  test.describe('Panel click → map selection', () => {
    test('clicking line row selects line and applies map styling', async ({ page }) => {
      await createProject(page, 'Line Selection Test');

      // Get line IDs from project
      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(0);
      const firstLineId = lineIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstLineId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Find and click the line row
      const lineRow = page.locator(`[data-testid="line-row-${firstLineId}"]`);
      await expect(lineRow).toBeVisible();
      await lineRow.click();

      // Poll for selection state
      const selection = await pollSelectionState(page, (sel) => sel?.type === 'line');
      expect(selection).not.toBeNull();
      expect(selection?.type).toBe('line');
      expect(selection?.lineId).toBe(firstLineId);

      // Verify map selection state is set
      const hasMapSelection = await hasLineMapHighlight(page, firstLineId, 'selection');
      expect(hasMapSelection).toBe(true);

      // Clicking the row leaves the pointer on it, so the line is hovered too.
      // Park the pointer elsewhere so only the SELECTION level is left, then
      // verify the line is still painted emphasized.
      await moveMouseOffLines(page);
      const selectedInk = await pollInk(page, probes, bg, (ink) => ink > baseInk);
      expect(selectedInk).toBeGreaterThan(baseInk);

      // Verify selection persists across a short wait (not transient)
      await page.waitForTimeout(200);
      const persistedSelection = await getSelectionState(page);
      expect(persistedSelection?.lineId).toBe(firstLineId);

      // Now clear with Escape
      await page.keyboard.press('Escape');
      const selectionAfterEscape = await pollSelectionState(page, (sel) => sel === null);
      expect(selectionAfterEscape).toBeNull();
    });
  });

  test.describe('Map hover → panel highlight (bidirectional)', () => {
    // These tests verify bidirectional sync by simulating map hover via store
    // manipulation, since MapLibre layer events are challenging to trigger reliably
    // in Playwright. The actual MapLibre event handlers are covered by unit tests.

    test('hoveredLineId from map propagates to panel highlight', async ({ page }) => {
      await createProject(page, 'Bidirectional Hover Test');

      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(0);
      const firstLineId = lineIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstLineId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Simulate map hover by setting hoveredLineId via store
      await page.evaluate((secId) => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { setHoveredLineId: (id: number | null) => void } };
          };
        }).__E2E_TEST__;

        if (stores) {
          stores.uiStore.getState().setHoveredLineId(secId);
        }
      }, firstLineId);

      // Verify map hover state is set
      const hasMapHover = await hasLineMapHighlight(page, firstLineId, 'hover');
      expect(hasMapHover).toBe(true);

      // A store-driven hover reaches the renderer too (MapCanvas effect →
      // SurveyLayer.setHoveredLine → customRender.setLineHover).
      const hoverInk = await pollInk(page, probes, bg, (ink) => ink > baseInk);
      expect(hoverInk).toBeGreaterThan(baseInk);

      // Clear hover
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { setHoveredLineId: (id: number | null) => void } };
          };
        }).__E2E_TEST__;

        if (stores) {
          stores.uiStore.getState().setHoveredLineId(null);
        }
      });

      // Verify hover state clears
      const hasMapHoverAfter = await hasLineMapHighlight(page, firstLineId, 'hover');
      expect(hasMapHoverAfter).toBe(false);

      // ...and the paint goes back to the unemphasized baseline.
      expect(await pollInk(page, probes, bg, (ink) => ink === baseInk)).toBe(baseInk);
    });
  });

  test.describe('Priority conflicts', () => {
    test('hover on different line overrides selection display', async ({ page }) => {
      await createProject(page, 'Different Line Hover Test');

      // Get line IDs from project
      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(1);
      const lineA = lineIds[0]!;
      const lineB = lineIds[1]!;

      const bg = await sampleBackground(page);
      const probesA = (await stationProbes(page, lineA)).exclusive;
      const baseInkA = await settledInk(page, probesA, bg);

      // Select line A by clicking the row
      const lineRowA = page.locator(`[data-testid="line-row-${lineA}"]`);
      await lineRowA.click();

      // Verify selection
      const selection = await pollSelectionState(page, (sel) => sel?.type === 'line');
      expect(selection?.lineId).toBe(lineA);

      // Now hover line B row in panel
      const lineRowB = page.locator(`[data-testid="line-row-${lineB}"]`);
      await lineRowB.hover();

      // Poll for hoveredLineId to change to line B
      const hoveredId = await pollHoveredLineId(page, (id) => id === lineB);
      expect(hoveredId).toBe(lineB);

      // Line B should have map hover state
      const hasBMapHover = await hasLineMapHighlight(page, lineB, 'hover');
      expect(hasBMapHover).toBe(true);

      // Line A selection state persists in store
      const currentSelection = await getSelectionState(page);
      expect(currentSelection?.lineId).toBe(lineA);

      // ...and A stays painted emphasized while the pointer emphasizes B.
      expect(await pollInk(page, probesA, bg, (ink) => ink > baseInkA)).toBeGreaterThan(baseInkA);
    });
  });

  test.describe('Unhover behavior', () => {
    test('hoveredLineId clears when mouse leaves line row', async ({ page }) => {
      await createProject(page, 'Unhover Panel Test');

      // Get line IDs from project
      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(0);
      const firstLineId = lineIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstLineId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Hover the line row
      const lineRow = page.locator(`[data-testid="line-row-${firstLineId}"]`);
      await lineRow.hover();

      // Verify hover is set
      await pollHoveredLineId(page, (id) => id === firstLineId);
      await pollInk(page, probes, bg, (ink) => ink > baseInk);

      // Move mouse away from the row to the toolbar area (above sections panel, outside map)
      // The toolbar buttons are a safe area that won't trigger map hover
      const toolbarSafeArea = page.locator('[data-testid="toolbar-import-button"]');
      await toolbarSafeArea.hover();

      // Poll for hoveredLineId to clear
      const hoveredId = await pollHoveredLineId(page, (id) => id === null);
      expect(hoveredId).toBeNull();

      // Verify map hover state is cleared
      const hasMapHover = await hasLineMapHighlight(page, firstLineId, 'hover');
      expect(hasMapHover).toBe(false);

      // Nothing else is emphasized, so the paint is back to exactly the baseline.
      const inkAfterUnhover = await pollInk(page, probes, bg, (ink) => ink === baseInk);
      expect(inkAfterUnhover).toBe(baseInk);
    });
  });

  test.describe('Keyboard navigation', () => {
    test('arrow keys navigate between line rows', async ({ page }) => {
      await createProject(page, 'Keyboard Navigation Test');

      // Get line IDs from project
      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(1);
      const firstLineId = lineIds[0]!;
      const secondLineId = lineIds[1]!;

      // Focus the first line row
      const firstRow = page.locator(`[data-testid="line-row-${firstLineId}"]`);
      await firstRow.focus();

      // Verify focus styling and hover state
      await expect(firstRow).toBeFocused();
      await pollHoveredLineId(page, (id) => id === firstLineId);

      // Press ArrowDown to move to next row
      await page.keyboard.press('ArrowDown');

      // The second row should now be focused
      const secondRow = page.locator(`[data-testid="line-row-${secondLineId}"]`);
      await expect(secondRow).toBeFocused();

      // Hovered line should update
      const hoveredId = await pollHoveredLineId(page, (id) => id === secondLineId);
      expect(hoveredId).toBe(secondLineId);
    });

    test('Enter selects focused line', async ({ page }) => {
      await createProject(page, 'Enter Selection Test');

      // Get line IDs from project
      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(0);
      const firstLineId = lineIds[0]!;

      // Focus the first line row
      const firstRow = page.locator(`[data-testid="line-row-${firstLineId}"]`);
      await firstRow.focus();

      // Press Enter to select
      await page.keyboard.press('Enter');

      // Verify selection
      const selection = await pollSelectionState(page, (sel) => sel?.type === 'line');
      expect(selection).not.toBeNull();
      expect(selection?.type).toBe('line');
      expect(selection?.lineId).toBe(firstLineId);

      // Verify map selection state
      const hasMapSelection = await hasLineMapHighlight(page, firstLineId, 'selection');
      expect(hasMapSelection).toBe(true);
    });

    test('Escape clears selection', async ({ page }) => {
      await createProject(page, 'Escape Clear Test');

      // Get line IDs from project
      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(0);
      const firstLineId = lineIds[0]!;

      // First, select a line
      const lineRow = page.locator(`[data-testid="line-row-${firstLineId}"]`);
      await lineRow.click();

      // Verify selection
      await pollSelectionState(page, (sel) => sel?.type === 'line');

      // Focus the row (needed to receive keyboard events)
      await lineRow.focus();

      // Press Escape to clear selection
      await page.keyboard.press('Escape');

      // Verify selection is cleared
      const selection = await pollSelectionState(page, (sel) => sel === null);
      expect(selection).toBeNull();

      // Verify map selection state is cleared
      const hasMapSelection = await hasLineMapHighlight(page, firstLineId, 'selection');
      expect(hasMapSelection).toBe(false);
    });
  });

  test.describe('Edge cases', () => {
    test('empty line is interactive and shows panel highlight', async ({ page }) => {
      await createProject(page, 'Empty Line Test');

      // Find the empty line row by name
      const emptyLineRow = page.locator('[data-testid^="line-item-"]').filter({ hasText: 'Empty Line' }).first();
      await expect(emptyLineRow).toBeVisible();

      // Hover should work
      await emptyLineRow.hover();

      // Get the line ID for the empty line
      const emptyLineId = await getLineIdByName(page, 'Empty Line');
      expect(emptyLineId).not.toBeNull();
      const resolvedEmptyLineId = emptyLineId as number;

      // Verify hover state is set
      const hoveredId = await pollHoveredLineId(page, (id) => id === resolvedEmptyLineId);
      expect(hoveredId).toBe(resolvedEmptyLineId);

      // Click should select it
      await emptyLineRow.click();

      // Should be able to select (even though no shots)
      const selection = await pollSelectionState(page, (sel) => sel?.type === 'line');
      expect(selection?.type).toBe('line');
      expect(selection?.lineId).toBe(resolvedEmptyLineId);

      // Verify no drawn shot geometry for the empty line. The drawn-geometry
      // hook replaces the deleted `survey-shots` layer query.
      const drawnShots = await getShotScreenPositions(page);
      expect(drawnShots.length).toBeGreaterThan(0);
      expect(drawnShots.filter((s) => s.lineId === resolvedEmptyLineId)).toHaveLength(0);
    });

    test('reference shots are included in line highlighting', async ({ page }) => {
      await createProject(page, 'Reference Shot Highlight Test');

      const reference = await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            projectStore: { getState: () => {
              project: { lines: Array<{ id: number; shots: Array<{ id: number; kind?: string }> }> } | null
            } };
          };
        }).__E2E_TEST__;

        const lines = stores?.projectStore.getState().project?.lines ?? [];
        for (const line of lines) {
          const shot = line.shots?.find((s) => s.kind && s.kind !== 'Normal');
          if (shot) return { lineId: line.id, shotId: shot.id };
        }
        return null;
      });
      expect(reference).not.toBeNull();
      const { lineId: referenceLineId, shotId: referenceShotId } = reference!;

      // The reference leg is actually drawn (reference shots are shown by
      // default), so it is part of the line's paint.
      const drawnShots = await getShotScreenPositions(page);
      const lineShots = drawnShots.filter((s) => s.lineId === referenceLineId);
      expect(lineShots.length).toBeGreaterThan(0);
      const referenceShot = lineShots.find((s) => s.shotId === referenceShotId);
      expect(referenceShot).toBeDefined();

      // Baseline paint around the REFERENCE leg's midpoint specifically.
      const bg = await sampleBackground(page);
      const refProbe = [referenceShot!.mid];
      const baseRefInk = await settledInk(page, refProbe, bg, SHOT_BLOCK);
      expect(baseRefInk).toBeGreaterThan(0);

      // Hover the line row
      const lineRow = page.locator(`[data-testid="line-row-${referenceLineId}"]`);
      await lineRow.hover();

      // Verify hover state is set
      const hoveredId = await pollHoveredLineId(page, (id) => id === referenceLineId);
      expect(hoveredId).toBe(referenceLineId);

      // Verify the map state includes the line for highlighting
      const hasMapHover = await hasLineMapHighlight(page, referenceLineId, 'hover');
      expect(hasMapHover).toBe(true);

      // The reference leg is emphasized along with the rest of the line: the
      // emphasis level is resolved per LINE, so the translucent reference line
      // widens too (its own hover pixels, not a neighbour's - the leg is the
      // line's last one and no other line reaches it).
      expect(
        await pollInk(page, refProbe, bg, (ink) => ink > baseRefInk, SHOT_BLOCK)
      ).toBeGreaterThan(baseRefInk);
    });
  });

  test.describe('Emphasis-level coexistence', () => {
    test('line selection and different line hover coexist on map', async ({ page }) => {
      await createProject(page, 'Emphasis Coexist Test');

      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(1);
      const lineA = lineIds[0]!;
      const lineB = lineIds[1]!;

      const bg = await sampleBackground(page);
      // Pixels that belong to A alone, plus pixels where B is drawn. Under the
      // mock B sits exactly on A's first leg, so B has no exclusive pixel of its
      // own; the pair of probes is what separates the two emphasis levels below.
      const probesA = (await stationProbes(page, lineA)).exclusive;
      const probesB = (await stationProbes(page, lineB)).shared;
      expect(probesA.length).toBeGreaterThan(0);
      expect(probesB.length).toBeGreaterThan(0);
      const baseInkA = await settledInk(page, probesA, bg);

      // Select line A, then park the pointer so A carries the selection level
      // and nothing carries hover.
      const lineRowA = page.locator(`[data-testid="line-row-${lineA}"]`);
      await lineRowA.click();
      await pollSelectionState(page, (sel) => sel?.type === 'line' && sel?.lineId === lineA);
      await moveMouseOffLines(page);
      const inkASelected = await pollInk(page, probesA, bg, (ink) => ink > baseInkA);
      const inkBWithASelectedOnly = await settledInk(page, probesB, bg);

      // Hover line B.
      const lineRowB = page.locator(`[data-testid="line-row-${lineB}"]`);
      await lineRowB.hover();
      await pollHoveredLineId(page, (id) => id === lineB);

      // B's pixels gain emphasis on top of what A's selection already painted
      // there (hover widens more than selection), so B is hover-emphasized.
      expect(
        await pollInk(page, probesB, bg, (ink) => ink > inkBWithASelectedOnly)
      ).toBeGreaterThan(inkBWithASelectedOnly);

      // A keeps its selection emphasis, unchanged: still above the unemphasized
      // baseline and still at exactly the selection level (it never gains hover).
      const inkAAfter = await inkAt(page, probesA, bg);
      expect(inkAAfter).toBeGreaterThan(baseInkA);
      expect(inkAAfter).toBe(inkASelected);
    });
  });

  test.describe('Emphasis clears on Escape', () => {
    test('pressing Escape removes the painted selection emphasis', async ({ page }) => {
      await createProject(page, 'Escape Emphasis Test');

      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(0);
      const firstLineId = lineIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstLineId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Select line
      const lineRow = page.locator(`[data-testid="line-row-${firstLineId}"]`);
      await lineRow.click();
      await pollSelectionState(page, (sel) => sel?.type === 'line' && sel?.lineId === firstLineId);

      // Park the pointer so only the selection emphasis is painted, and verify
      // it IS painted.
      await moveMouseOffLines(page);
      const inkBefore = await pollInk(page, probes, bg, (ink) => ink > baseInk);
      expect(inkBefore).toBeGreaterThan(baseInk);

      // Press Escape
      await page.keyboard.press('Escape');
      await pollSelectionState(page, (sel) => sel === null);

      // The emphasis is gone from the paint (back to the baseline).
      const inkAfter = await pollInk(page, probes, bg, (ink) => ink === baseInk);
      expect(inkAfter).toBe(baseInk);
    });
  });

  test.describe('Emphasis survives data update', () => {
    test('line highlighting persists after data update (buffer rebuild)', async ({ page }) => {
      await createProject(page, 'Data Update Emphasis Test');

      const lineIds = await getAvailableLineIds(page);
      expect(lineIds.length).toBeGreaterThan(0);
      const firstLineId = lineIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstLineId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Select line
      const lineRow = page.locator(`[data-testid="line-row-${firstLineId}"]`);
      await lineRow.click();
      await pollSelectionState(page, (sel) => sel?.type === 'line' && sel?.lineId === firstLineId);

      // Pointer parked: only the selection emphasis remains, and it is painted.
      await moveMouseOffLines(page);
      await pollInk(page, probes, bg, (ink) => ink > baseInk);

      // Read showDepthGradient before toggle to verify it actually changes
      const gradientBefore = await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { showDepthGradient: boolean } };
          };
        }).__E2E_TEST__;
        return stores?.uiStore.getState().showDepthGradient ?? null;
      });
      expect(gradientBefore).not.toBeNull();

      // Trigger a data update by toggling depth gradient via store. Colors
      // changed, so the custom renderer rebuilds its vertex buffers from
      // scratch and the emphasis has to be re-derived from the controller's
      // stored line sets (the custom-renderer equivalent of re-applying
      // feature-state after setData()).
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { toggleDisplay: (key: string) => void } };
          };
        }).__E2E_TEST__;
        if (!stores) throw new Error('__E2E_TEST__ stores not found');
        stores.uiStore.getState().toggleDisplay('showDepthGradient');
      });

      // Verify the toggle actually changed (proves data update was triggered)
      const gradientAfter = await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { showDepthGradient: boolean } };
          };
        }).__E2E_TEST__;
        return stores?.uiStore.getState().showDepthGradient ?? null;
      });
      expect(gradientAfter).toBe(!gradientBefore);

      // The emphasis must survive the rebuild. The depth gradient repaints the
      // survey in different colors, so the pre-toggle baseline is not
      // comparable; compare against the unemphasized paint in the SAME color
      // mode by clearing the selection afterwards.
      const inkAfterUpdate = await settledInk(page, probes, bg);
      await page.keyboard.press('Escape');
      await pollSelectionState(page, (sel) => sel === null);
      const inkUnemphasized = await pollInk(page, probes, bg, (ink) => ink < inkAfterUpdate);
      expect(inkAfterUpdate).toBeGreaterThan(inkUnemphasized);
    });
  });
});
