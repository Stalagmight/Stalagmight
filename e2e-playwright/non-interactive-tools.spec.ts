import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedNavigationTools,
  seedProjectWithSections,
  waitForMapReady,
  waitForActiveTool,
  getStationScreenPositions,
  waitForDrawnStations,
  type SectionSeed,
} from './helpers/app';

/**
 * E2E tests for non-interactive tool behavior (pan, zoom).
 *
 * When pan or zoom tools are active, feature interactions should be disabled:
 * - No station/shot selection on click
 * - No hover effects
 * - No context menu on right-click (for pan)
 * - Ctrl+A does nothing
 */

/**
 * Helper to create a project with imported sections so stations/shots are visible.
 */
async function createProjectWithStations(page: Page): Promise<void> {
  await createProject(page, 'Non-Interactive Tool Test');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapReady(page);
  // The survey is drawn by the custom WebGL renderer, so there is no stock
  // `survey-stations` layer for `queryRenderedFeatures` to read. Wait on the
  // app's drawn-geometry hook instead (see helpers/app.ts).
  await waitForDrawnStations(page, 1);
}

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Non-Interactive Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5 },
      { azimuth_in: 90, azimuth_out: 270, length: 15.2, depth_in: -2.5, depth_out: -5.0 },
    ],
  },
  {
    name: 'Non-Interactive Side',
    direction: 'out',
    shots: [
      { azimuth_in: 270, azimuth_out: 90, length: 5.0, depth_in: -3.0, depth_out: -4.0 },
    ],
  },
];

/** The most isolated point (farthest from every OTHER point). A click on it
 *  unambiguously resolves to the one station rendered there. */
function mostIsolatedPoint(points: Array<{ x: number; y: number }>): { x: number; y: number } {
  let bestIdx = 0;
  let bestMinOther = -Infinity;
  for (let i = 0; i < points.length; i++) {
    let minOther = Infinity;
    for (let j = 0; j < points.length; j++) {
      if (i === j) continue;
      minOther = Math.min(minOther, Math.hypot(points[i]!.x - points[j]!.x, points[i]!.y - points[j]!.y));
    }
    if (minOther > bestMinOther) {
      bestMinOther = minOther;
      bestIdx = i;
    }
  }
  return points[bestIdx]!;
}

/**
 * Helper to find a station screen coordinate. Returns the most isolated station
 * point so a click on it resolves unambiguously to a single station.
 */
async function getStationPosition(page: Page): Promise<{ x: number; y: number } | null> {
  const points = await getStationScreenPositions(page);
  if (points.length === 0) return null;
  return mostIsolatedPoint(points);
}

/**
 * Helper to get canvas bounding box for calculating click coordinates.
 */
async function getCanvasBoundingBox(page: Page): Promise<{
  x: number;
  y: number;
  width: number;
  height: number;
}> {
  const canvas = page.locator('canvas.maplibregl-canvas');
  const box = await canvas.boundingBox();
  if (!box) throw new Error('Canvas not found');
  return box;
}

test.describe('Non-interactive tools', () => {
  test.beforeEach(async ({ page }) => {
    // Pan and Zoom ARE the non-interactive tools, and both are off the toolbar
    // (P/Z inert) unless `map.controls.show_navigation_tools` is on (#409).
    // Seeded BEFORE setupMockedPage: the flag has to be present for the app's
    // first `get_config` at boot.
    await seedNavigationTools(page);
    await setupMockedPage(page);
    await createProjectWithStations(page);
  });

  test('Pan tool prevents station selection', async ({ page }) => {
    // Switch to pan tool
    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');

    // Get station position
    const stationPos = await getStationPosition(page);
    expect(stationPos).not.toBeNull();

    const canvasBox = await getCanvasBoundingBox(page);

    // Click on station position
    await page.mouse.click(canvasBox.x + stationPos!.x, canvasBox.y + stationPos!.y);

    // Verify selection is empty
    const selection = await page.evaluate(() => {
      return (window as unknown as {
        __E2E_TEST__?: {
          uiStore: { getState: () => { selection: unknown } };
        };
      }).__E2E_TEST__?.uiStore?.getState()?.selection ?? null;
    });
    expect(selection).toBeNull();
  });

  test('Pan tool prevents context menu', async ({ page }) => {
    // NOTE: Use pan, not zoom (zoom right-click does zoom-out)
    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');

    const canvasBox = await getCanvasBoundingBox(page);

    // Right-click on canvas
    await page.mouse.click(canvasBox.x + 200, canvasBox.y + 200, { button: 'right' });

    // Verify context menu dialog is NOT open (poll for a short window to catch async updates)
    // We check multiple times to ensure it never becomes 'context-menu'
    let sawContextMenu = false;
    for (let i = 0; i < 5; i++) {
      const activeDialog = await page.evaluate(() => {
        return (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { activeDialog: string | null } };
          };
        }).__E2E_TEST__?.uiStore?.getState()?.activeDialog;
      });
      if (activeDialog === 'context-menu') {
        sawContextMenu = true;
        break;
      }
      // settle: a 5 x 50 ms sampling window for a menu that must NEVER open; a
      // poll would stop at the first (pre-update) read.
      await page.waitForTimeout(50);
    }
    expect(sawContextMenu).toBe(false);
  });

  test('Switching to pan clears existing hover', async ({ page }) => {
    // This test verifies that switching to pan tool clears hoveredStation state.
    // The clearing is done by uiStore.setActiveTool when switching to a non-interactive tool.

    // Ensure select tool is active first
    await page.keyboard.press('s');
    await waitForActiveTool(page, 'select');

    // Click the most isolated station point (unambiguous single-station
    // resolution) and read which station the selection resolver picked. The
    // store keys the ref by snake_case (section_id/station_index), so this
    // still reads it back from the resolved selection rather than the
    // drawn-geometry hook's camelCase identity.
    const points = await getStationScreenPositions(page);
    expect(points.length).toBeGreaterThan(0);
    const target = mostIsolatedPoint(points);

    const canvasBox = await getCanvasBoundingBox(page);
    await page.mouse.click(canvasBox.x + target.x, canvasBox.y + target.y);

    let stationRef: { section_id: number; station_index: number } | undefined;
    await expect.poll(
      async () => {
        const sel = await page.evaluate(() => {
          return (window as unknown as {
            __E2E_TEST__?: {
              uiStore: {
                getState: () => {
                  selection: { type: string; stationRef?: { section_id: number; station_index: number } } | null;
                };
              };
            };
          }).__E2E_TEST__?.uiStore?.getState()?.selection ?? null;
        });
        if (sel?.type === 'station' && sel.stationRef) {
          stationRef = sel.stationRef;
          return true;
        }
        return false;
      },
      { timeout: 2000, message: 'Clicking a station should resolve a station ref' }
    ).toBe(true);
    expect(stationRef).toBeTruthy();

    // Clear the click selection, then set hover state directly via uiStore
    // (simulating what the SurveyLayer callback would do). The MapCanvas useEffect
    // syncs this to SurveyLayer.setHoveredStation automatically.
    await page.keyboard.press('Escape');
    const hoverSetup = await page.evaluate((ref) => {
      const stores = (window as unknown as {
        __E2E_TEST__?: {
          uiStore: {
            getState: () => { setHoveredStation: (r: { section_id: number; station_index: number } | null) => void };
          };
        };
      }).__E2E_TEST__;

      if (!stores) return { success: false, error: 'No __E2E_TEST__' };

      // Set hovered station in uiStore
      stores.uiStore.getState().setHoveredStation(ref);

      return { success: true };
    }, stationRef!);

    expect(hoverSetup.success).toBe(true);

    // Verify hover is set
    await expect.poll(
      async () => {
        return page.evaluate(() => {
          return (window as unknown as {
            __E2E_TEST__?: {
              uiStore: { getState: () => { hoveredStation: unknown } };
            };
          }).__E2E_TEST__?.uiStore?.getState()?.hoveredStation ?? null;
        });
      },
      { timeout: 2000, message: 'Hover state should be set' }
    ).not.toBeNull();

    // Switch to pan - this triggers setActiveTool
    // The setActiveTool function should clear hoveredStation via the callback chain
    await page.keyboard.press('p');

    // Poll until hover state is cleared
    await expect.poll(
      async () => {
        return page.evaluate(() => {
          return (window as unknown as {
            __E2E_TEST__?: {
              uiStore: { getState: () => { hoveredStation: unknown } };
            };
          }).__E2E_TEST__?.uiStore?.getState()?.hoveredStation ?? null;
        });
      },
      { timeout: 2000, message: 'Hover state should be cleared after switching to pan' }
    ).toBeNull();
  });

  // [#576 W33-NF8] The tool effect now puts back a panel row's shot/station hover
  // that setActiveTool's Pan/Zoom clear took away. A map-pointer hover must still
  // go on a switch to Pan, in the store AND on the map, with the pointer held still.
  test('Switching to pan with the pointer on a station clears its map highlight', async ({ page }) => {
    await page.keyboard.press('s');
    await waitForActiveTool(page, 'select');
    const pos = await getStationPosition(page);
    expect(pos).not.toBeNull();
    const canvasBox = await getCanvasBoundingBox(page);
    // getStyle(), not getSource(): the latter hands back a MapLibre class instance.
    const hover = (): Promise<{ store: unknown; overlay: number }> =>
      page.evaluate(() => {
        const w = window as unknown as {
          __E2E_TEST__: { uiStore: { getState: () => { hoveredStation: unknown } } };
          __stalagmight_map__?: { getStyle: () => { sources: Record<string, unknown> } };
        };
        const src = w.__stalagmight_map__?.getStyle().sources['survey-interact'] as
          | { data?: { features?: Array<{ properties?: { kind?: string } | null }> } }
          | undefined;
        return {
          store: w.__E2E_TEST__.uiStore.getState().hoveredStation,
          overlay: (src?.data?.features ?? []).filter((f) => f.properties?.kind === 'station').length,
        };
      });

    await page.mouse.move(canvasBox.x + pos!.x, canvasBox.y + pos!.y, { steps: 4 });
    await expect.poll(async () => (await hover()).overlay).toBe(1);
    expect((await hover()).store).not.toBeNull();

    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');
    await expect.poll(async () => (await hover()).overlay).toBe(0);
    expect((await hover()).store).toBeNull();
  });

  test('Ctrl+A does nothing when pan active', async ({ page }) => {
    // Switch to pan tool
    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');

    // Press Ctrl+A
    await page.keyboard.press('Control+a');

    // Verify selection is still empty
    const selection = await page.evaluate(() => {
      return (window as unknown as {
        __E2E_TEST__?: {
          uiStore: { getState: () => { selection: unknown } };
        };
      }).__E2E_TEST__?.uiStore?.getState()?.selection ?? null;
    });
    expect(selection).toBeNull();
  });

  test('Selection still works after switching back to select tool', async ({ page }) => {
    // Switch to pan tool, then back to select
    await page.keyboard.press('p');
    await page.keyboard.press('s');

    const pos = await getStationPosition(page);
    expect(pos).not.toBeNull();

    const canvasBox = await getCanvasBoundingBox(page);
    await page.mouse.click(canvasBox.x + pos!.x, canvasBox.y + pos!.y);

    await expect.poll(async () => {
      return page.evaluate(() => {
        return (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { selection: { type: string } | null } };
          };
        }).__E2E_TEST__?.uiStore?.getState()?.selection?.type ?? null;
      });
    }, { timeout: 2000 }).toBe('station');
  });
});
