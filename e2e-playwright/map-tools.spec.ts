import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  getMapCenter,
  seedNavigationTools,
  seedProjectWithSections,
  waitForMapReady,
  waitForActiveTool,
  waitForMapIdle,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Main Passage',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5, left: 1.5, right: 2.3, up: 3.0, down: 0.8 },
      { azimuth_in: 90, azimuth_out: 270, length: 15.2, depth_in: -2.5, depth_out: -5.0, left: 0.5, right: 0.7, up: 1.2, down: 1.5 },
    ],
  },
  {
    name: 'Side Tunnel',
    direction: 'out',
    shots: [
      { azimuth_in: 270, azimuth_out: 90, length: 5.0, depth_in: -3.0, depth_out: -4.0, left: 0, right: 0, up: 0, down: 0 },
    ],
  },
];

/**
 * Helper to create a project with sections for map tools testing.
 * Returns when the project is loaded with sections containing shots.
 *
 * Uses store seeding to avoid dialog overlays during tests.
 */
async function createProjectWithSections(page: Page): Promise<void> {
  await createProject(page, 'Map Tools Test Project');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
}

/**
 * Map tool tests use the two-step import flow and auto-placement helpers.
 */
test.describe('Map Tools', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('toolbar center holds the workspace carousel; filename stays out of it (#51, #38)', async ({ page }) => {
    await createProjectWithSections(page);
    // The center now holds the Survey/Plan/Export carousel (#38). The active tool moved
    // to the highlighted toolbar button (asserted in the shortcut test below), and the
    // filename is not in the toolbar center.
    await expect(page.getByTestId('mode-carousel')).toBeVisible();
    await expect(page.getByTestId('toolbar-project-name')).toHaveCount(0);
    // Tool shortcuts still switch the active tool.
    await page.keyboard.press('m');
    await waitForActiveTool(page, 'merge');
    await page.keyboard.press('r');
    await waitForActiveTool(page, 'measure');

    // The project name and full file path live in the Project Stats panel
    // (shown when nothing is selected).
    await expect(page.getByTestId('project-stats-name')).toContainText('Map Tools Test Project');
    await expect(page.getByTestId('project-stats-path')).toBeVisible();
  });

  test('should switch between tools with keyboard shortcuts @smoke', async ({ page }) => {
    // Create a project to access the SingleSystemView with tools
    await createProjectWithSections(page);

    // Verify canvas is visible (we're in SingleSystemView)
    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();

    // Test each tool shortcut as defined in DEFAULT_SHORTCUTS:
    // s: 'select', m: 'merge', r: 'measure', v: 'move_segment', t: 'move_station'.
    // p ('pan') and z ('zoom') are deliberately absent: the navigate group is
    // hidden unless `map.controls.show_navigation_tools` is on (#409), and while
    // it is hidden those two keys are inert. Their seeded behaviour is covered by
    // navigation-tools-setting.spec.ts and the seeded describes elsewhere.
    const toolShortcuts = [
      { key: 's', tool: 'select' },
      { key: 'm', tool: 'merge' },
      { key: 'r', tool: 'measure' },
      { key: 'v', tool: 'move_segment' },
      { key: 't', tool: 'move_station' },
    ];
    // Standalone toolbar buttons (not inside tool groups)
    const toolbarTools = new Set(['select', 'merge', 'measure']);

    for (const { key, tool } of toolShortcuts) {
      await page.keyboard.press(key);
      await waitForActiveTool(page, tool);

      // App should remain functional after each keypress
      await expect(canvas).toBeVisible();

      if (toolbarTools.has(tool)) {
        const activeButton = page.getByTestId(`tool-${tool}`);
        await expect(activeButton).toBeVisible();
      }
    }
  });

  test('should select stations with select tool @smoke', async ({ page }) => {
    // Create a project with sections
    await createProjectWithSections(page);

    // Verify canvas is visible
    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();

    // Switch to select tool
    await page.keyboard.press('s');
    await waitForActiveTool(page, 'select');

    // Click on the canvas to attempt selection
    const canvasBox = await canvas.boundingBox();
    expect(canvasBox).not.toBeNull();

    if (canvasBox) {
      const centerX = canvasBox.x + canvasBox.width / 2;
      const centerY = canvasBox.y + canvasBox.height / 2;

      // Single click for single selection
      await page.mouse.click(centerX, centerY);
      await waitForMapIdle(page);

      // The app should remain functional
      await expect(canvas).toBeVisible();

      // Modifier+click. Use `locator.click({ modifiers })`, not
      // `page.mouse.click` (which has no `modifiers` option) and not a
      // keyboard.down/up pair around it: `ControlOrMeta` resolves to Meta on
      // macOS, and holding literal Control there makes Chromium emit
      // contextmenu with no click event at all, so the interaction would stop
      // reaching the app on the one platform this suite is run on by hand.
      // NB this still does not reach multi-select: the mousedown-capture handler
      // claims ctrl+left for panning under every tool except 'pan' itself
      // (MapCanvas.tsx, `activeTool !== 'pan'`), and 'select' is one of them.
      // What it covers is that the app survives that path.
      await canvas.click({
        modifiers: ['ControlOrMeta'],
        position: { x: canvasBox.width / 2 + 50, y: canvasBox.height / 2 },
      });
      await waitForMapIdle(page);

      // The app should remain functional
      await expect(canvas).toBeVisible();

      // Test Ctrl+A for select all
      await page.keyboard.press('Control+a');
      await waitForMapIdle(page);

      // The app should remain functional
      await expect(canvas).toBeVisible();
    }
  });

  // #409: panning is not a tool. A plain left-drag pans under the DEFAULT tool,
  // with no `p` press first - which is the whole point of hiding the pan tool.
  // Middle-drag still pans too, and still leaves the active tool alone.
  test('should pan map with a plain drag under select, or the middle mouse button', async ({ page }) => {
    // Create a project with sections
    await createProjectWithSections(page);

    // Verify canvas is visible
    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();

    const canvasBox = await canvas.boundingBox();
    expect(canvasBox).not.toBeNull();

    if (canvasBox) {
      const centerX = canvasBox.x + canvasBox.width / 2;
      const centerY = canvasBox.y + canvasBox.height / 2;

      // The default tool, explicitly - no navigate group to switch to.
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      const centerBeforeDrag = await getMapCenter(page);
      expect(centerBeforeDrag).not.toBeNull();

      // Plain left-drag on empty canvas (offset from the seeded geometry).
      await page.mouse.move(centerX - 150, centerY - 120);
      await page.mouse.down();
      await page.mouse.move(centerX - 50, centerY - 20, { steps: 8 });
      await page.mouse.up();
      await waitForMapIdle(page);

      const centerAfterDrag = await getMapCenter(page);
      expect(centerAfterDrag).not.toBeNull();
      expect(
        Math.abs(centerAfterDrag!.lng - centerBeforeDrag!.lng) +
          Math.abs(centerAfterDrag!.lat - centerBeforeDrag!.lat)
      ).toBeGreaterThan(0.00001);

      // Panning did not become a tool switch.
      await waitForActiveTool(page, 'select');

      // Middle mouse button for panning (works regardless of tool).
      await page.mouse.move(centerX, centerY);
      await page.mouse.down({ button: 'middle' });
      await page.mouse.move(centerX - 50, centerY - 50, { steps: 6 });
      await page.mouse.up({ button: 'middle' });
      await waitForMapIdle(page);

      const centerAfterMiddle = await getMapCenter(page);
      expect(centerAfterMiddle).not.toBeNull();
      expect(
        Math.abs(centerAfterMiddle!.lng - centerAfterDrag!.lng) +
          Math.abs(centerAfterMiddle!.lat - centerAfterDrag!.lat)
      ).toBeGreaterThan(0.00001);

      // The app should remain functional
      await expect(canvas).toBeVisible();
    }
  });

  test('should measure distance with measure tool', async ({ page }) => {
    // Create a project with sections
    await createProjectWithSections(page);

    // Verify canvas is visible
    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();

    // Switch to measure tool
    await page.keyboard.press('r');
    await waitForActiveTool(page, 'measure');

    const canvasBox = await canvas.boundingBox();
    expect(canvasBox).not.toBeNull();

    if (canvasBox) {
      const centerX = canvasBox.x + canvasBox.width / 2;
      const centerY = canvasBox.y + canvasBox.height / 2;

      // Click to start measurement
      await page.mouse.click(centerX - 100, centerY);
      await waitForMapIdle(page);

      // Click to end measurement
      await page.mouse.click(centerX + 100, centerY);
      await waitForMapIdle(page);

      // The app should remain functional
      await expect(canvas).toBeVisible();

      // Look for any distance display or measurement overlay
      // The measure tool might display distance information on the map
      // or in a tooltip/overlay. This is implementation-specific.

      // Clear the measurement with Escape
      await page.keyboard.press('Escape');
      await waitForMapIdle(page);

      // The app should remain functional
      await expect(canvas).toBeVisible();
    }
  });

  test('should fit view to all data with Ctrl+0 @smoke', async ({ page }) => {
    // Create a project with sections
    await createProjectWithSections(page);

    // Verify canvas is visible
    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();

    // First, pan or zoom away from the default view
    const canvasBox = await canvas.boundingBox();
    expect(canvasBox).not.toBeNull();

    if (canvasBox) {
      // Zoom out with wheel to change the view
      const centerX = canvasBox.x + canvasBox.width / 2;
      const centerY = canvasBox.y + canvasBox.height / 2;

      await page.mouse.move(centerX, centerY);
      await page.mouse.wheel(0, 200); // Zoom out
      await waitForMapIdle(page);

      // Pan away from center. No tool switch: a plain left-drag pans under the
      // default tool (#409), and the navigate group is hidden by default.
      await page.mouse.move(centerX, centerY);
      await page.mouse.down();
      await page.mouse.move(centerX + 200, centerY + 200, { steps: 10 });
      await page.mouse.up();
      await waitForMapIdle(page);
    }

    // Use Ctrl+0 to fit view to all data
    await page.keyboard.press('Control+0');
    await waitForMapIdle(page);

    // The app should remain functional and the map should have adjusted
    await expect(canvas).toBeVisible();

    // The fit to bounds should have been triggered
    // Since this is a mocked environment, we mainly verify the app doesn't crash
    // and the keyboard shortcut is properly handled
  });

  test('should delete selected items with Delete key', async ({ page }) => {
    // Create a project with sections
    await createProjectWithSections(page);

    // Verify canvas is visible
    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();

    // First, select something - let's select a section from the sections panel
    const sectionRow = page.locator('[data-testid^="section-row-"]').filter({ hasText: 'Side Tunnel' });
    if (await sectionRow.isVisible()) {
      await sectionRow.click();
      await waitForMapIdle(page);

      // Verify section is selected (has highlight)
      await expect(sectionRow).toHaveClass(/bg-accent/);

      // Press Delete key to trigger deletion
      await page.keyboard.press('Delete');
      await waitForMapIdle(page);

      // A confirmation dialog should appear
      const confirmDialog = page.getByTestId('confirm-dialog');
      if (await confirmDialog.isVisible()) {
        // The dialog should mention delete or confirm
        const dialogText = await confirmDialog.textContent();
        const hasDeleteWarning = dialogText?.toLowerCase().includes('delete') ||
                                 dialogText?.toLowerCase().includes('confirm') ||
                                 dialogText?.toLowerCase().includes('remove');
        expect(hasDeleteWarning).toBeTruthy();

        // Should have Cancel and Confirm/Delete buttons
        const cancelButton = confirmDialog.locator('button:has-text("Cancel")');
        await expect(cancelButton).toBeVisible();

        // Click Cancel to abort deletion
        await cancelButton.click();
        await waitForMapIdle(page);

        // Dialog should close
        await expect(confirmDialog).not.toBeVisible({ timeout: 2000 });
      }
    }

    // Also test with a shot selected in the data table
    // First, open the data table
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible();

    // Select a shot from the data table
    const tableRow = page.locator('tbody tr').first();
    if (await tableRow.isVisible()) {
      await tableRow.click();
      await waitForMapIdle(page);

      // Press Delete key
      await page.keyboard.press('Delete');
      await waitForMapIdle(page);

      // If a confirmation dialog appears, cancel it
      const confirmDialog = page.getByTestId('confirm-dialog');
      if (await confirmDialog.isVisible()) {
        const cancelButton = confirmDialog.locator('button:has-text("Cancel")');
        if (await cancelButton.isVisible()) {
          await cancelButton.click();
          await waitForMapIdle(page);
        }
      }
    }

    // The app should remain functional
    await expect(canvas).toBeVisible();
  });
});

/**
 * Click-to-zoom tool tests - separate from skipped tests above.
 * These use a simpler project setup that doesn't require the DMP import flow.
 */
test.describe('Zoom Tool Click Behavior', () => {
  test.beforeEach(async ({ page }) => {
    // The Zoom tool needs `map.controls.show_navigation_tools` on (#409), and the
    // seed must be registered BEFORE setupMockedPage so it reaches the app's
    // first `get_config`.
    await seedNavigationTools(page);
    await setupMockedPage(page);
  });

  test('should zoom in on left click and zoom out on right click', async ({ page }) => {
    await createProjectWithSections(page);

    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();

    // Switch to zoom tool
    await page.keyboard.press('z');
    await waitForActiveTool(page, 'zoom');

    const canvasBox = await canvas.boundingBox();
    expect(canvasBox).not.toBeNull();

    if (canvasBox) {
      const centerX = canvasBox.x + canvasBox.width / 2;
      const centerY = canvasBox.y + canvasBox.height / 2;

      // Get initial zoom level via exposed map instance
      const initialZoom = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number } }).__stalagmight_map__;
        return map?.getZoom() ?? 0;
      });

      // Left click should zoom in. Wait for the zoom animation to settle at
      // a higher level (idle + changed) rather than sleeping a fixed 400ms.
      await page.mouse.click(centerX, centerY);
      await page.waitForFunction((z) => {
        const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number; isMoving: () => boolean } }).__stalagmight_map__;
        return Boolean(map && !map.isMoving() && map.getZoom() > z);
      }, initialZoom, { timeout: 5000 });

      const zoomAfterClick = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number } }).__stalagmight_map__;
        return map?.getZoom() ?? 0;
      });

      expect(zoomAfterClick).toBeGreaterThan(initialZoom);

      // Right click should zoom out - settle before asserting it returned to
      // ~the original level (within 0.1), so we need the full animation done.
      await page.mouse.click(centerX, centerY, { button: 'right' });
      await page.waitForFunction((z) => {
        const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number; isMoving: () => boolean } }).__stalagmight_map__;
        return Boolean(map && !map.isMoving() && map.getZoom() < z);
      }, zoomAfterClick, { timeout: 5000 });

      const zoomAfterRightClick = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number } }).__stalagmight_map__;
        return map?.getZoom() ?? 0;
      });

      // Should be back to approximately original zoom (within 0.1)
      expect(zoomAfterRightClick).toBeLessThan(zoomAfterClick);
      expect(Math.abs(zoomAfterRightClick - initialZoom)).toBeLessThan(0.1);
    }
  });

  test('should not select stations when zoom tool is active', async ({ page }) => {
    await createProjectWithSections(page);

    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();

    // Verify no selection initially
    const initialSelection = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { selection: unknown } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.selection ?? null;
    });
    expect(initialSelection).toBeNull();

    // Switch to zoom tool
    await page.keyboard.press('z');
    await waitForActiveTool(page, 'zoom');

    const canvasBox = await canvas.boundingBox();
    expect(canvasBox).not.toBeNull();

    if (canvasBox) {
      const centerX = canvasBox.x + canvasBox.width / 2;
      const centerY = canvasBox.y + canvasBox.height / 2;

      // Click on map with zoom tool - should NOT trigger selection
      const zoomBefore = await page.evaluate(() => (window as unknown as { __stalagmight_map__: { getZoom: () => number } }).__stalagmight_map__.getZoom());
      await page.mouse.click(centerX, centerY);
      // The click zoomed in, and the zoom animation has finished.
      await expect.poll(() => page.evaluate(() => (window as unknown as { __stalagmight_map__: { getZoom: () => number } }).__stalagmight_map__.getZoom())).toBeGreaterThan(zoomBefore);
      await waitForMapIdle(page);

      // Verify selection is still null (click-to-zoom doesn't select)
      const selectionAfterClick = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { selection: unknown } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.selection ?? null;
      });
      expect(selectionAfterClick).toBeNull();

      // Canvas should still be functional
      await expect(canvas).toBeVisible();
    }
  });

  test('should show zoom-in cursor when zoom tool is active', async ({ page }) => {
    // Create a minimal project
    const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
    await newProjectBtn.click();
    await page.waitForSelector('.fixed.inset-0');
    await page.locator('.fixed.inset-0 input').first().fill('Zoom Cursor Test');
    await page.click('.fixed.inset-0 button:has-text("Create")');
    await page.waitForSelector('canvas', { timeout: 10000 });

    const canvas = page.locator('canvas').first();
    await expect(canvas).toBeVisible();

    // Switch to zoom tool
    await page.keyboard.press('z');
    await waitForActiveTool(page, 'zoom');

    // Check cursor on canvas
    const cursorStyle = await page.evaluate(() => {
      const map = (window as unknown as { __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement } }).__stalagmight_map__;
      return map?.getCanvas()?.style.cursor ?? '';
    });

    expect(cursorStyle).toBe('zoom-in');
  });
});
