import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithSections,
  waitForMapReady,
  getDrawnGeometry,
  getShotScreenPositions,
  readCanvasPixel,
  countCanvasInk,
  type PixelRgba,
  type ScreenPoint,
  type SectionSeed,
} from './helpers/app';

/**
 * E2E tests for section hover/selection highlighting.
 *
 * Tests the bidirectional highlighting between the sections panel and the map:
 * - Panel hover → map highlight (state + painted emphasis)
 * - Panel click → map selection (state + painted emphasis)
 * - Map hover → panel highlight (via store simulation)
 * - Priority conflicts (individual hover vs section selection)
 * - Unhover behavior
 * - Keyboard navigation
 * - Edge cases (empty sections, reference shots)
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
 * The mock setup provides pre-populated sections with shots.
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas.maplibregl-canvas', { timeout: 10000 });
  await waitForMapReady(page);
  await seedProjectWithSections(page, sectionSeeds);
  await waitForShotsRendered(page);
  await zoomInOnSurvey(page);
}

const sectionSeeds: SectionSeed[] = [
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
    name: 'Empty Section',
    direction: 'in',
    shots: [],
  },
];

// ---------------------------------------------------------------------------
// Painted-emphasis oracle (custom WebGL renderer)
// ---------------------------------------------------------------------------
//
// Section hover / selection highlighting is a per-vertex emphasis LEVEL in the
// custom renderer now: SurveyLayer.setHoveredSection / setSelectedSection /
// setHoveredSegment call customRender.setSectionHover / setSectionSelect /
// setHighlight, which re-upload a highlight attribute (2 = hover, 1 = selection,
// 0 = none). There is no MapLibre feature-state left to read and the controller
// exposes no read-back, so the honest oracle is the painted pixel.
//
// The shader turns the level into a SIZE bump only (colors are unchanged): shot
// lines widen by hover_width/width = 4/2 = 2x or selection_width/width = 3/2 =
// 1.5x, and station discs grow by hover_radius/radius = 6/4 = 1.5x or
// selection_radius/radius = 5/4 = 1.25x. So "is this section emphasized" is
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
 * How far a probe point must be from any OTHER section's geometry to be that
 * section's alone: half a station block plus the largest painted disc radius
 * ((radius 4 + stroke 1) * 1.5 hover = 7.5), rounded up.
 */
const PROBE_CLEARANCE = 20;

/** A "nothing drawn here" reference sample from the canvas corner. */
async function sampleBackground(page: Page): Promise<PixelRgba> {
  return readCanvasPixel(page, 6, 6);
}

/**
 * Zoom in on the seeded survey before probing pixels. The mock places every
 * station at `x = index * 10` metres on one line AND gives every section the
 * same coordinates, so at the default fit-zoom the whole survey is a ~14 px
 * smear where no per-station pixel exists. Zooming in spreads the stations ~74
 * px apart, which is what makes a per-station / per-section pixel meaningful.
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
 * Station-center probe points for one section, split by whether the pixels
 * belong to that section alone.
 *
 * Sections are drawn COINCIDENT under the mock (identical station coordinates),
 * so a section with few stations may have no exclusive point at all: section
 * "Highlight Secondary" (1 shot) sits exactly on the first leg of "Highlight
 * Main" (3 shots), and only Main's later stations stand alone.
 */
async function stationProbes(
  page: Page,
  sectionId: number
): Promise<{ exclusive: ScreenPoint[]; shared: ScreenPoint[] }> {
  const { stations, shots } = await getDrawnGeometry(page);
  const others = stations.filter((s) => s.sectionId !== sectionId);
  const otherShots = shots.filter((s) => s.sectionId !== sectionId);
  const exclusive: ScreenPoint[] = [];
  const shared: ScreenPoint[] = [];
  for (const station of stations.filter((s) => s.sectionId === sectionId)) {
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
 * Helper to get a section ID by name.
 */
async function getSectionIdByName(page: Page, name: string): Promise<number | null> {
  return page.evaluate((sectionName) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { sections: Array<{ id: number; name: string }> } | null } };
      };
    }).__E2E_TEST__;

    const sections = stores?.projectStore.getState().project?.sections ?? [];
    const match = sections.find((section) => section.name === sectionName);
    return match?.id ?? null;
  }, name);
}

/**
 * Helper to get the current selection state from uiStore.
 */
async function getSelectionState(page: Page): Promise<{
  type: string;
  sectionId?: number;
  stationRef?: { section_id: number; station_index: number };
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
      sectionId?: number;
      stationRef?: { section_id: number; station_index: number };
      shotId?: number;
    } | null;
  });
}

/**
 * Helper to get the current hovered section ID from uiStore.
 */
async function getHoveredSectionId(page: Page): Promise<number | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { hoveredSectionId: number | null } };
      };
    }).__E2E_TEST__;

    if (!stores) return null;
    return stores.uiStore.getState().hoveredSectionId;
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
 * Helper to poll for hovered section ID changes.
 */
async function pollHoveredSectionId(
  page: Page,
  predicate: (sectionId: number | null) => boolean,
  timeout = 5000
): Promise<number | null> {
  let result: number | null = null;
  await expect.poll(
    async () => {
      const sectionId = await getHoveredSectionId(page);
      if (predicate(sectionId)) {
        result = sectionId;
        return true;
      }
      return false;
    },
    { timeout }
  ).toBe(true);
  return result;
}

/**
 * Helper to get available section IDs from the project.
 */
async function getAvailableSectionIds(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { sections: Array<{ id: number }> } | null } };
      };
    }).__E2E_TEST__;

    if (!stores) return [];
    const project = stores.projectStore.getState().project;
    return project?.sections.map(s => s.id) ?? [];
  });
}

/**
 * Park the pointer where it hovers neither a section row nor the map, so only
 * the selection-level emphasis is left on the canvas. The toolbar buttons are a
 * safe area (above the sections panel, outside the map).
 */
async function moveMouseOffSections(page: Page): Promise<void> {
  await page.locator('[data-testid="toolbar-import-button"]').hover();
  await pollHoveredSectionId(page, (id) => id === null);
}

/**
 * Helper to check if a section has elevated map styling (hover or selection).
 * This checks uiStore state which drives the map styling.
 */
async function hasSectionMapHighlight(page: Page, sectionId: number, type: 'hover' | 'selection'): Promise<boolean> {
  return page.evaluate(({ secId, highlightType }) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => {
          hoveredSectionId: number | null;
          selection: { type: string; sectionId?: number } | null;
        } };
      };
    }).__E2E_TEST__;

    if (!stores) return false;
    const state = stores.uiStore.getState();

    if (highlightType === 'hover') {
      return state.hoveredSectionId === secId;
    } else {
      return state.selection?.type === 'section' && state.selection?.sectionId === secId;
    }
  }, { secId: sectionId, highlightType: type });
}

test.describe('Section Highlighting', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
  });

  test.describe('Panel hover → map highlight', () => {
    test('hovering section row sets hoveredSectionId and applies map styling', async ({ page }) => {
      await createProject(page, 'Section Highlight Test');

      // Get section IDs from project
      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(1);
      const firstSectionId = sectionIds[0]!;
      const otherSectionId = sectionIds[1]!;

      // Baseline paint (nothing emphasized) on pixels that belong to the first
      // section alone.
      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstSectionId)).exclusive;
      expect(probes.length).toBeGreaterThan(0);
      const baseInk = await settledInk(page, probes, bg);
      expect(baseInk).toBeGreaterThan(0);

      // Find the section row using data-testid
      const sectionRow = page.locator(`[data-testid="section-row-${firstSectionId}"]`);
      await expect(sectionRow).toBeVisible();

      // Hover the section row
      await sectionRow.hover();

      // Poll for hoveredSectionId to be set
      const hoveredId = await pollHoveredSectionId(page, (id) => id === firstSectionId);
      expect(hoveredId).toBe(firstSectionId);

      // Verify map styling state is set (drives SurveyLayer.setHoveredSection)
      const hasMapHover = await hasSectionMapHighlight(page, firstSectionId, 'hover');
      expect(hasMapHover).toBe(true);

      // The hovered section is actually PAINTED emphasized.
      const hoverInk = await pollInk(page, probes, bg, (ink) => ink > baseInk);
      expect(hoverInk).toBeGreaterThan(baseInk);

      // Only the hovered section is emphasized: hovering the OTHER section's row
      // returns these pixels to the unemphasized baseline.
      const otherRow = page.locator(`[data-testid="section-row-${otherSectionId}"]`);
      await otherRow.hover();
      await pollHoveredSectionId(page, (id) => id === otherSectionId);
      expect(await pollInk(page, probes, bg, (ink) => ink === baseInk)).toBe(baseInk);
    });
  });

  test.describe('Panel click → map selection', () => {
    test('clicking section row selects section and applies map styling', async ({ page }) => {
      await createProject(page, 'Section Selection Test');

      // Get section IDs from project
      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(0);
      const firstSectionId = sectionIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstSectionId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Find and click the section row
      const sectionRow = page.locator(`[data-testid="section-row-${firstSectionId}"]`);
      await expect(sectionRow).toBeVisible();
      await sectionRow.click();

      // Poll for selection state
      const selection = await pollSelectionState(page, (sel) => sel?.type === 'section');
      expect(selection).not.toBeNull();
      expect(selection?.type).toBe('section');
      expect(selection?.sectionId).toBe(firstSectionId);

      // Verify map selection state is set
      const hasMapSelection = await hasSectionMapHighlight(page, firstSectionId, 'selection');
      expect(hasMapSelection).toBe(true);

      // Clicking the row leaves the pointer on it, so the section is hovered too.
      // Park the pointer elsewhere so only the SELECTION level is left, then
      // verify the section is still painted emphasized.
      await moveMouseOffSections(page);
      const selectedInk = await pollInk(page, probes, bg, (ink) => ink > baseInk);
      expect(selectedInk).toBeGreaterThan(baseInk);

      // settle: the selection must persist (not be a transient flash), so it is
      // re-read after a fixed interval rather than polled.
      await page.waitForTimeout(200);
      const persistedSelection = await getSelectionState(page);
      expect(persistedSelection?.sectionId).toBe(firstSectionId);

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

    test('hoveredSectionId from map propagates to panel highlight', async ({ page }) => {
      await createProject(page, 'Bidirectional Hover Test');

      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(0);
      const firstSectionId = sectionIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstSectionId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Simulate map hover by setting hoveredSectionId via store
      await page.evaluate((secId) => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { setHoveredSectionId: (id: number | null) => void } };
          };
        }).__E2E_TEST__;

        if (stores) {
          stores.uiStore.getState().setHoveredSectionId(secId);
        }
      }, firstSectionId);

      // Verify map hover state is set
      const hasMapHover = await hasSectionMapHighlight(page, firstSectionId, 'hover');
      expect(hasMapHover).toBe(true);

      // A store-driven hover reaches the renderer too (MapCanvas effect →
      // SurveyLayer.setHoveredSection → customRender.setSectionHover).
      const hoverInk = await pollInk(page, probes, bg, (ink) => ink > baseInk);
      expect(hoverInk).toBeGreaterThan(baseInk);

      // Clear hover
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { setHoveredSectionId: (id: number | null) => void } };
          };
        }).__E2E_TEST__;

        if (stores) {
          stores.uiStore.getState().setHoveredSectionId(null);
        }
      });

      // Verify hover state clears
      const hasMapHoverAfter = await hasSectionMapHighlight(page, firstSectionId, 'hover');
      expect(hasMapHoverAfter).toBe(false);

      // ...and the paint goes back to the unemphasized baseline.
      expect(await pollInk(page, probes, bg, (ink) => ink === baseInk)).toBe(baseInk);
    });
  });

  test.describe('Priority conflicts', () => {
    test('hover on different section overrides selection display', async ({ page }) => {
      await createProject(page, 'Different Section Hover Test');

      // Get section IDs from project
      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(1);
      const sectionA = sectionIds[0]!;
      const sectionB = sectionIds[1]!;

      const bg = await sampleBackground(page);
      const probesA = (await stationProbes(page, sectionA)).exclusive;
      const baseInkA = await settledInk(page, probesA, bg);

      // Select section A by clicking the row
      const sectionRowA = page.locator(`[data-testid="section-row-${sectionA}"]`);
      await sectionRowA.click();

      // Verify selection
      const selection = await pollSelectionState(page, (sel) => sel?.type === 'section');
      expect(selection?.sectionId).toBe(sectionA);

      // Now hover section B row in panel
      const sectionRowB = page.locator(`[data-testid="section-row-${sectionB}"]`);
      await sectionRowB.hover();

      // Poll for hoveredSectionId to change to section B
      const hoveredId = await pollHoveredSectionId(page, (id) => id === sectionB);
      expect(hoveredId).toBe(sectionB);

      // Section B should have map hover state
      const hasBMapHover = await hasSectionMapHighlight(page, sectionB, 'hover');
      expect(hasBMapHover).toBe(true);

      // Section A selection state persists in store
      const currentSelection = await getSelectionState(page);
      expect(currentSelection?.sectionId).toBe(sectionA);

      // ...and A stays painted emphasized while the pointer emphasizes B.
      expect(await pollInk(page, probesA, bg, (ink) => ink > baseInkA)).toBeGreaterThan(baseInkA);
    });
  });

  test.describe('Unhover behavior', () => {
    test('hoveredSectionId clears when mouse leaves section row', async ({ page }) => {
      await createProject(page, 'Unhover Panel Test');

      // Get section IDs from project
      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(0);
      const firstSectionId = sectionIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstSectionId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Hover the section row
      const sectionRow = page.locator(`[data-testid="section-row-${firstSectionId}"]`);
      await sectionRow.hover();

      // Verify hover is set
      await pollHoveredSectionId(page, (id) => id === firstSectionId);
      await pollInk(page, probes, bg, (ink) => ink > baseInk);

      // Move mouse away from the row to the toolbar area (above sections panel, outside map)
      // The toolbar buttons are a safe area that won't trigger map hover
      const toolbarSafeArea = page.locator('[data-testid="toolbar-import-button"]');
      await toolbarSafeArea.hover();

      // Poll for hoveredSectionId to clear
      const hoveredId = await pollHoveredSectionId(page, (id) => id === null);
      expect(hoveredId).toBeNull();

      // Verify map hover state is cleared
      const hasMapHover = await hasSectionMapHighlight(page, firstSectionId, 'hover');
      expect(hasMapHover).toBe(false);

      // Nothing else is emphasized, so the paint is back to exactly the baseline.
      const inkAfterUnhover = await pollInk(page, probes, bg, (ink) => ink === baseInk);
      expect(inkAfterUnhover).toBe(baseInk);
    });
  });

  test.describe('Keyboard navigation', () => {
    test('arrow keys navigate between section rows', async ({ page }) => {
      await createProject(page, 'Keyboard Navigation Test');

      // Get section IDs from project
      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(1);
      const firstSectionId = sectionIds[0]!;
      const secondSectionId = sectionIds[1]!;

      // Focus the first section row
      const firstRow = page.locator(`[data-testid="section-row-${firstSectionId}"]`);
      await firstRow.focus();

      // Verify focus styling and hover state
      await expect(firstRow).toBeFocused();
      await pollHoveredSectionId(page, (id) => id === firstSectionId);

      // Press ArrowDown to move to next row
      await page.keyboard.press('ArrowDown');

      // The second row should now be focused
      const secondRow = page.locator(`[data-testid="section-row-${secondSectionId}"]`);
      await expect(secondRow).toBeFocused();

      // Hovered section should update
      const hoveredId = await pollHoveredSectionId(page, (id) => id === secondSectionId);
      expect(hoveredId).toBe(secondSectionId);
    });

    test('Enter selects focused section', async ({ page }) => {
      await createProject(page, 'Enter Selection Test');

      // Get section IDs from project
      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(0);
      const firstSectionId = sectionIds[0]!;

      // Focus the first section row
      const firstRow = page.locator(`[data-testid="section-row-${firstSectionId}"]`);
      await firstRow.focus();

      // Press Enter to select
      await page.keyboard.press('Enter');

      // Verify selection
      const selection = await pollSelectionState(page, (sel) => sel?.type === 'section');
      expect(selection).not.toBeNull();
      expect(selection?.type).toBe('section');
      expect(selection?.sectionId).toBe(firstSectionId);

      // Verify map selection state
      const hasMapSelection = await hasSectionMapHighlight(page, firstSectionId, 'selection');
      expect(hasMapSelection).toBe(true);
    });

    test('Escape clears selection', async ({ page }) => {
      await createProject(page, 'Escape Clear Test');

      // Get section IDs from project
      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(0);
      const firstSectionId = sectionIds[0]!;

      // First, select a section
      const sectionRow = page.locator(`[data-testid="section-row-${firstSectionId}"]`);
      await sectionRow.click();

      // Verify selection
      await pollSelectionState(page, (sel) => sel?.type === 'section');

      // Focus the row (needed to receive keyboard events)
      await sectionRow.focus();

      // Press Escape to clear selection
      await page.keyboard.press('Escape');

      // Verify selection is cleared
      const selection = await pollSelectionState(page, (sel) => sel === null);
      expect(selection).toBeNull();

      // Verify map selection state is cleared
      const hasMapSelection = await hasSectionMapHighlight(page, firstSectionId, 'selection');
      expect(hasMapSelection).toBe(false);
    });
  });

  test.describe('Edge cases', () => {
    test('empty section is interactive and shows panel highlight', async ({ page }) => {
      await createProject(page, 'Empty Section Test');

      // Find the empty section row by name
      const emptySectionRow = page.locator('[data-testid^="section-item-"]').filter({ hasText: 'Empty Section' }).first();
      await expect(emptySectionRow).toBeVisible();

      // Hover should work
      await emptySectionRow.hover();

      // Get the section ID for the empty section
      const emptySectionId = await getSectionIdByName(page, 'Empty Section');
      expect(emptySectionId).not.toBeNull();
      const resolvedEmptySectionId = emptySectionId as number;

      // Verify hover state is set
      const hoveredId = await pollHoveredSectionId(page, (id) => id === resolvedEmptySectionId);
      expect(hoveredId).toBe(resolvedEmptySectionId);

      // Click should select it
      await emptySectionRow.click();

      // Should be able to select (even though no shots)
      const selection = await pollSelectionState(page, (sel) => sel?.type === 'section');
      expect(selection?.type).toBe('section');
      expect(selection?.sectionId).toBe(resolvedEmptySectionId);

      // Verify no drawn shot geometry for the empty section. The drawn-geometry
      // hook replaces the deleted `survey-shots` layer query.
      const drawnShots = await getShotScreenPositions(page);
      expect(drawnShots.length).toBeGreaterThan(0);
      expect(drawnShots.filter((s) => s.sectionId === resolvedEmptySectionId)).toHaveLength(0);
    });

    test('reference shots are included in section highlighting', async ({ page }) => {
      await createProject(page, 'Reference Shot Highlight Test');

      const reference = await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            projectStore: { getState: () => {
              project: { sections: Array<{ id: number; shots: Array<{ id: number; kind?: string }> }> } | null
            } };
          };
        }).__E2E_TEST__;

        const sections = stores?.projectStore.getState().project?.sections ?? [];
        for (const section of sections) {
          const shot = section.shots?.find((s) => s.kind && s.kind !== 'Normal');
          if (shot) return { sectionId: section.id, shotId: shot.id };
        }
        return null;
      });
      expect(reference).not.toBeNull();
      const { sectionId: referenceSectionId, shotId: referenceShotId } = reference!;

      // The reference leg is actually drawn (reference shots are shown by
      // default), so it is part of the section's paint.
      const drawnShots = await getShotScreenPositions(page);
      const sectionShots = drawnShots.filter((s) => s.sectionId === referenceSectionId);
      expect(sectionShots.length).toBeGreaterThan(0);
      const referenceShot = sectionShots.find((s) => s.shotId === referenceShotId);
      expect(referenceShot).toBeDefined();

      // Baseline paint around the REFERENCE leg's midpoint specifically.
      const bg = await sampleBackground(page);
      const refProbe = [referenceShot!.mid];
      const baseRefInk = await settledInk(page, refProbe, bg, SHOT_BLOCK);
      expect(baseRefInk).toBeGreaterThan(0);

      // Hover the section row
      const sectionRow = page.locator(`[data-testid="section-row-${referenceSectionId}"]`);
      await sectionRow.hover();

      // Verify hover state is set
      const hoveredId = await pollHoveredSectionId(page, (id) => id === referenceSectionId);
      expect(hoveredId).toBe(referenceSectionId);

      // Verify the map state includes the section for highlighting
      const hasMapHover = await hasSectionMapHighlight(page, referenceSectionId, 'hover');
      expect(hasMapHover).toBe(true);

      // The reference leg is emphasized along with the rest of the section: the
      // emphasis level is resolved per SECTION, so the translucent reference line
      // widens too (its own hover pixels, not a neighbour's - the leg is the
      // section's last one and no other section reaches it).
      expect(
        await pollInk(page, refProbe, bg, (ink) => ink > baseRefInk, SHOT_BLOCK)
      ).toBeGreaterThan(baseRefInk);
    });
  });

  test.describe('Emphasis-level coexistence', () => {
    test('section selection and different section hover coexist on map', async ({ page }) => {
      await createProject(page, 'Emphasis Coexist Test');

      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(1);
      const sectionA = sectionIds[0]!;
      const sectionB = sectionIds[1]!;

      const bg = await sampleBackground(page);
      // Pixels that belong to A alone, plus pixels where B is drawn. Under the
      // mock B sits exactly on A's first leg, so B has no exclusive pixel of its
      // own; the pair of probes is what separates the two emphasis levels below.
      const probesA = (await stationProbes(page, sectionA)).exclusive;
      const probesB = (await stationProbes(page, sectionB)).shared;
      expect(probesA.length).toBeGreaterThan(0);
      expect(probesB.length).toBeGreaterThan(0);
      const baseInkA = await settledInk(page, probesA, bg);

      // Select section A, then park the pointer so A carries the selection level
      // and nothing carries hover.
      const sectionRowA = page.locator(`[data-testid="section-row-${sectionA}"]`);
      await sectionRowA.click();
      await pollSelectionState(page, (sel) => sel?.type === 'section' && sel?.sectionId === sectionA);
      await moveMouseOffSections(page);
      const inkASelected = await pollInk(page, probesA, bg, (ink) => ink > baseInkA);
      const inkBWithASelectedOnly = await settledInk(page, probesB, bg);

      // Hover section B.
      const sectionRowB = page.locator(`[data-testid="section-row-${sectionB}"]`);
      await sectionRowB.hover();
      await pollHoveredSectionId(page, (id) => id === sectionB);

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

      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(0);
      const firstSectionId = sectionIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstSectionId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Select section
      const sectionRow = page.locator(`[data-testid="section-row-${firstSectionId}"]`);
      await sectionRow.click();
      await pollSelectionState(page, (sel) => sel?.type === 'section' && sel?.sectionId === firstSectionId);

      // Park the pointer so only the selection emphasis is painted, and verify
      // it IS painted.
      await moveMouseOffSections(page);
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
    test('section highlighting persists after data update (buffer rebuild)', async ({ page }) => {
      await createProject(page, 'Data Update Emphasis Test');

      const sectionIds = await getAvailableSectionIds(page);
      expect(sectionIds.length).toBeGreaterThan(0);
      const firstSectionId = sectionIds[0]!;

      const bg = await sampleBackground(page);
      const probes = (await stationProbes(page, firstSectionId)).exclusive;
      const baseInk = await settledInk(page, probes, bg);

      // Select section
      const sectionRow = page.locator(`[data-testid="section-row-${firstSectionId}"]`);
      await sectionRow.click();
      await pollSelectionState(page, (sel) => sel?.type === 'section' && sel?.sectionId === firstSectionId);

      // Pointer parked: only the selection emphasis remains, and it is painted.
      await moveMouseOffSections(page);
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
      // stored section sets (the custom-renderer equivalent of re-applying
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
