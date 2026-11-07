import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { getMapCenter, seedNavigationTools, waitForActiveTool, waitForMapIdle } from './helpers/app';

/**
 * Helper to wait for the MapLibre map to be ready.
 */
async function waitForMap(page: Page): Promise<void> {
  await page.waitForFunction(
    () => {
      const map = (window as unknown as { __stalagmight_map__?: { loaded: () => boolean; getZoom: () => number } }).__stalagmight_map__;
      return map && map.loaded() && typeof map.getZoom() === 'number';
    },
    { timeout: 10000 }
  );
}

/**
 * Helper to create a new project and get to SingleSystemView with map canvas.
 */
async function createProjectWithMap(page: Page): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill('Zoom Tool Test');
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
  // Wait for map to be fully ready
  await waitForMap(page);
}

/**
 * Helper to get the current map zoom level. Returns null if map is not ready.
 */
async function getMapZoom(page: Page): Promise<number | null> {
  return page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number } }).__stalagmight_map__;
    return map?.getZoom() ?? null;
  });
}

/**
 * Helper to assert the zoom stays pinned at `expectedZoom` across a short
 * settle window (~400ms, 4x100ms samples). waitForMapIdle resolves near-
 * instantly when the map is already idle, so a single read right after it
 * can't catch a delayed animation sneaking in; this re-samples across the
 * window the old fixed 400ms sleep used to cover.
 */
async function assertZoomRemainsStable(page: Page, expectedZoom: number): Promise<void> {
  for (const interval of [100, 100, 100, 100]) {
    // settle: samples the zoom over 400 ms to prove it does NOT drift; a poll
    // would stop at the first matching read.
    await page.waitForTimeout(interval);
    expect(await getMapZoom(page)).toBe(expectedZoom);
  }
}

/**
 * Helper to wait for zoom to change from a starting value.
 */
async function waitForZoomChange(page: Page, startZoom: number, direction: 'in' | 'out'): Promise<number> {
  await page.waitForFunction(
    ({ start, dir }) => {
      const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number } }).__stalagmight_map__;
      const current = map?.getZoom();
      if (typeof current !== 'number') return false;
      return dir === 'in' ? current > start : current < start;
    },
    { start: startZoom, dir: direction },
    { timeout: 5000 }
  );
  const zoom = await getMapZoom(page);
  expect(zoom).not.toBeNull();
  return zoom as number;
}

test.describe('Zoom Tool', () => {
  test.beforeEach(async ({ page }) => {
    // The Zoom tool is off the toolbar (and Z is inert) unless
    // `map.controls.show_navigation_tools` is on (#409). Seeded BEFORE the mocks
    // so the flag is present for the app's first `get_config` at boot.
    await seedNavigationTools(page);
    await setupTauriMocks(page);
    await page.goto('/');
    await createProjectWithMap(page);
  });

  test.describe('Click Zoom', () => {
    test('left-click zooms in @smoke', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Left click to zoom in
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);

      // Wait for zoom to change
      const newZoom = await waitForZoomChange(page, initialZoom!, 'in');
      expect(newZoom).toBeGreaterThan(initialZoom!);
    });

    test('right-click zooms out @smoke', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // First zoom in a bit so we can zoom out. Poll for the zoom-in to
      // register before capturing the baseline rather than sleeping.
      const zoomBefore = await getMapZoom(page);
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);
      const initialZoom = await waitForZoomChange(page, zoomBefore!, 'in');
      expect(initialZoom).not.toBeNull();

      // Right click to zoom out
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2, { button: 'right' });

      // Wait for zoom to change
      const newZoom = await waitForZoomChange(page, initialZoom!, 'out');
      expect(newZoom).toBeLessThan(initialZoom!);
    });

    test('right-click does not show context menu', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Right click should zoom out, not show context menu
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2, { button: 'right' });

      // Wait for zoom to change (proves context menu didn't interfere with zoom)
      const newZoom = await waitForZoomChange(page, initialZoom!, 'out');
      expect(newZoom).toBeLessThan(initialZoom!);

      // Verify no context menu in UI store (more reliable than selector)
      const hasContextMenu = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { activeDialog: string | null } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.activeDialog === 'context-menu';
      });
      expect(hasContextMenu).toBe(false);
    });

    test('click at zoom 22 (max) is ignored', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Set zoom to max (22)
      await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { setZoom: (z: number) => void } }).__stalagmight_map__;
        map?.setZoom(22);
      });
      await expect.poll(() => getMapZoom(page)).toBe(22);

      const centerBefore = await getMapCenter(page);
      expect(centerBefore).not.toBeNull();

      // Left click should be ignored at max zoom (no recenter either). The
      // handler never calls easeTo when targetZoom === currentZoom, so the
      // map never starts moving; waitForMapIdle just settles any pending work.
      await page.mouse.click(box!.x + box!.width / 2 + 50, box!.y + box!.height / 2 + 50);
      await waitForMapIdle(page);
      await assertZoomRemainsStable(page, 22);

      const zoomAfter = await getMapZoom(page);
      const centerAfter = await getMapCenter(page);
      expect(zoomAfter).not.toBeNull();
      expect(centerAfter).not.toBeNull();

      expect(zoomAfter).toBe(22);
      // Center should not change (click ignored entirely)
      expect(Math.abs(centerAfter!.lng - centerBefore!.lng)).toBeLessThan(0.001);
      expect(Math.abs(centerAfter!.lat - centerBefore!.lat)).toBeLessThan(0.001);
    });

    test('right-click at zoom 1 (min) is ignored', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Set zoom to min (1 per mock config)
      await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { setZoom: (z: number) => void } }).__stalagmight_map__;
        map?.setZoom(1);
      });
      await expect.poll(() => getMapZoom(page)).toBe(1);

      const centerBefore = await getMapCenter(page);
      expect(centerBefore).not.toBeNull();

      // Right click should be ignored at min zoom (no recenter either). The
      // handler never calls easeTo when targetZoom === currentZoom, so the
      // map never starts moving; waitForMapIdle just settles any pending work.
      await page.mouse.click(box!.x + box!.width / 2 + 50, box!.y + box!.height / 2 + 50, { button: 'right' });
      await waitForMapIdle(page);
      await assertZoomRemainsStable(page, 1);

      const zoomAfter = await getMapZoom(page);
      const centerAfter = await getMapCenter(page);
      expect(zoomAfter).not.toBeNull();
      expect(centerAfter).not.toBeNull();

      expect(zoomAfter).toBe(1);
      // Center should not change (click ignored entirely)
      expect(Math.abs(centerAfter!.lng - centerBefore!.lng)).toBeLessThan(0.001);
      expect(Math.abs(centerAfter!.lat - centerBefore!.lat)).toBeLessThan(0.001);
    });
  });

  test.describe('Box Zoom', () => {
    test('drag draws rectangle overlay', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Start drag
      await page.mouse.move(box!.x + 100, box!.y + 100);
      await page.mouse.down();
      await page.mouse.move(box!.x + 200, box!.y + 200);

      // Box zoom overlay should be visible
      const overlay = page.locator('[data-testid="zoom-box-overlay"]');
      await expect(overlay).toBeVisible();

      // Release
      await page.mouse.up();
    });

    test('escape cancels box zoom', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Start drag
      await page.mouse.move(box!.x + 100, box!.y + 100);
      await page.mouse.down();
      await page.mouse.move(box!.x + 200, box!.y + 200);

      // Box zoom overlay should be visible
      const overlay = page.locator('[data-testid="zoom-box-overlay"]');
      await expect(overlay).toBeVisible();

      // Press Escape to cancel
      await page.keyboard.press('Escape');

      // Overlay should be gone (toBeVisible/not.toBeVisible already poll)
      await expect(overlay).not.toBeVisible();

      // Release mouse. Escape already cleared zoomBox.start, so releasing
      // never triggers easeTo/fitBounds; wait for any pending work to settle.
      await page.mouse.up();
      await waitForMapIdle(page);

      // Zoom should not have changed
      const finalZoom = await getMapZoom(page);
      expect(finalZoom).not.toBeNull();
      expect(Math.abs(finalZoom! - initialZoom!)).toBeLessThan(0.1);
    });

    test('small drag (<10px) treated as click', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Small drag (less than 10px)
      await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
      await page.mouse.down();
      await page.mouse.move(box!.x + box!.width / 2 + 5, box!.y + box!.height / 2 + 5);
      await page.mouse.up();

      // Wait for zoom to change (treated as click)
      const finalZoom = await waitForZoomChange(page, initialZoom!, 'in');
      expect(finalZoom).toBeGreaterThan(initialZoom!);
    });

    test('Ctrl+drag pans instead of box zoom', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      const initialZoom = await getMapZoom(page);
      const initialCenter = await getMapCenter(page);
      expect(initialZoom).not.toBeNull();
      expect(initialCenter).not.toBeNull();

      // Ctrl+drag should pan, not box zoom
      await page.keyboard.down('Control');
      await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
      await page.mouse.down();
      await page.mouse.move(box!.x + box!.width / 2 + 100, box!.y + box!.height / 2 + 100);

      // Box zoom overlay should NOT be visible
      const overlay = page.locator('[data-testid="zoom-box-overlay"]');
      await expect(overlay).not.toBeVisible();

      await page.mouse.up();
      await page.keyboard.up('Control');
      await waitForMapIdle(page);

      // Zoom should not have changed
      const finalZoom = await getMapZoom(page);
      expect(finalZoom).not.toBeNull();
      expect(Math.abs(finalZoom! - initialZoom!)).toBeLessThan(0.1);

      // Center should have changed (panned)
      const finalCenter = await getMapCenter(page);
      expect(finalCenter).not.toBeNull();
      const centerChanged = Math.abs(finalCenter!.lng - initialCenter!.lng) > 0.0001 ||
                            Math.abs(finalCenter!.lat - initialCenter!.lat) > 0.0001;
      expect(centerChanged).toBe(true);
    });
  });

  test.describe('Cursor', () => {
    test('shows zoom-in cursor when zoom tool is active', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const cursor = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement } }).__stalagmight_map__;
        return map?.getCanvas()?.style.cursor ?? null;
      });
      expect(cursor).not.toBeNull();
      expect(cursor).toBe('zoom-in');
    });

    test('cursor stays zoom-in when hovering over map', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Move mouse over different parts of the canvas
      await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);

      // Cursor should still be zoom-in (not pointer like with select tool)
      await expect.poll(() => page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement } }).__stalagmight_map__;
        return map?.getCanvas()?.style.cursor ?? null;
      })).toBe('zoom-in');
    });
  });

  test.describe('Keyboard Shortcuts', () => {
    test('= key zooms in @smoke', async ({ page }) => {
      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Press = to zoom in
      await page.keyboard.press('=');

      // Wait for zoom to change
      const newZoom = await waitForZoomChange(page, initialZoom!, 'in');
      expect(newZoom).toBeGreaterThan(initialZoom!);
      // Should zoom in (direction verified above, exact step may vary by animation)
    });

    test('- key zooms out', async ({ page }) => {
      // First zoom in a bit
      const beforeZoomIn = await getMapZoom(page);
      expect(beforeZoomIn).not.toBeNull();

      await page.keyboard.press('=');
      await waitForZoomChange(page, beforeZoomIn!, 'in');

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Press - to zoom out
      await page.keyboard.press('-');

      // Wait for zoom to change
      const newZoom = await waitForZoomChange(page, initialZoom!, 'out');
      expect(newZoom).toBeLessThan(initialZoom!);
      // Should zoom out (direction verified above, exact step may vary by animation)
    });

    test('keyboard zoom works regardless of active tool', async ({ page }) => {
      // Switch to select tool
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Press = to zoom in (should work even in select mode)
      await page.keyboard.press('=');

      // Wait for zoom to change
      const newZoom = await waitForZoomChange(page, initialZoom!, 'in');
      expect(newZoom).toBeGreaterThan(initialZoom!);
    });
  });

  test.describe('Toolbar Interaction', () => {
    test('Z key shortcut activates zoom tool via navigate group', async ({ page }) => {
      // First ensure we're not in zoom mode by switching to select
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      const selectButton = page.locator('[data-testid="tool-select"]');
      await expect(selectButton).toHaveClass(/sh-tool--active/);

      // Activate zoom tool via keyboard shortcut (zoom is now in Navigate group)
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      // Verify navigate group is active (zoom is a subtool of navigate)
      const navGroup = page.locator('[data-testid="tool-group-navigate"]');
      await expect(navGroup).toHaveClass(/sh-tool--active/);
      await expect(selectButton).not.toHaveClass(/sh-tool--active/);
    });

    test('switching between tools via keyboard updates cursor', async ({ page }) => {
      // Start in select mode
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      // Verify select cursor
      let cursor = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement } }).__stalagmight_map__;
        return map?.getCanvas()?.style.cursor ?? null;
      });
      expect(cursor === '' || cursor === 'default').toBe(true);

      // Activate zoom via keyboard (zoom is in Navigate group)
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      // Verify zoom cursor
      cursor = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement } }).__stalagmight_map__;
        return map?.getCanvas()?.style.cursor ?? null;
      });
      expect(cursor).toBe('zoom-in');

      // Activate pan via keyboard (pan is in Navigate group)
      await page.keyboard.press('p');
      await waitForActiveTool(page, 'pan');

      // Verify pan cursor
      cursor = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement } }).__stalagmight_map__;
        return map?.getCanvas()?.style.cursor ?? null;
      });
      expect(cursor).toBe('grab');
    });
  });

  test.describe('Config Integration', () => {
    test('uses zoom_step from config for click zoom', async ({ page }) => {
      // Wait for config to be loaded from mock backend
      await page.waitForFunction(
        () => {
          const stores = (window as unknown as {
            __E2E_TEST__?: {
              projectStore: {
                getState: () => {
                  config: { map: { zoom_step: number } } | null;
                };
              };
            };
          }).__E2E_TEST__;
          return stores?.projectStore?.getState()?.config != null;
        },
        { timeout: 5000 }
      );

      // Read the zoom_step value from config
      const configuredZoomStep = await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            projectStore: {
              getState: () => {
                config: { map: { zoom_step: number } } | null;
              };
            };
          };
        }).__E2E_TEST__;
        return stores?.projectStore?.getState()?.config?.map?.zoom_step ?? null;
      });

      expect(configuredZoomStep).not.toBeNull();
      expect(typeof configuredZoomStep).toBe('number');
      expect(configuredZoomStep).toBeGreaterThan(0);

      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Left click to zoom in
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);

      // Wait for the easeTo zoom animation to settle
      await waitForMapIdle(page);

      const newZoom = await getMapZoom(page);
      expect(newZoom).not.toBeNull();

      const actualDiff = newZoom! - initialZoom!;

      // Verify the zoom delta matches the configured zoom_step
      // Allow for animation/rounding variance (within 0.5 of configured value)
      expect(actualDiff).toBeGreaterThan(configuredZoomStep! - 0.5);
      expect(actualDiff).toBeLessThan(configuredZoomStep! + 0.5);
    });

    test('uses zoom_step from config for keyboard zoom', async ({ page }) => {
      // Wait for config to be loaded
      await page.waitForFunction(
        () => {
          const stores = (window as unknown as {
            __E2E_TEST__?: {
              projectStore: {
                getState: () => {
                  config: { map: { zoom_step: number } } | null;
                };
              };
            };
          }).__E2E_TEST__;
          return stores?.projectStore?.getState()?.config != null;
        },
        { timeout: 5000 }
      );

      // Read the zoom_step value from config
      const configuredZoomStep = await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            projectStore: {
              getState: () => {
                config: { map: { zoom_step: number } } | null;
              };
            };
          };
        }).__E2E_TEST__;
        return stores?.projectStore?.getState()?.config?.map?.zoom_step ?? null;
      });

      expect(configuredZoomStep).not.toBeNull();

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Press = to zoom in
      await page.keyboard.press('=');

      // Wait for the zoomTo animation to settle
      await waitForMapIdle(page);

      const newZoom = await getMapZoom(page);
      expect(newZoom).not.toBeNull();

      const actualDiff = newZoom! - initialZoom!;

      // Verify the zoom delta matches the configured zoom_step
      expect(actualDiff).toBeGreaterThan(configuredZoomStep! - 0.5);
      expect(actualDiff).toBeLessThan(configuredZoomStep! + 0.5);
    });

    test('respects zoom_min boundary', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Set zoom to min (1 per mock config)
      await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { setZoom: (z: number) => void } }).__stalagmight_map__;
        map?.setZoom(1);
      });
      await expect.poll(() => getMapZoom(page)).toBe(1);

      const zoomAtMin = await getMapZoom(page);
      expect(zoomAtMin).toBe(1);

      // Right click should be ignored at min zoom (no easeTo call, so nothing
      // to settle; waitForMapIdle still documents the intent).
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2, { button: 'right' });
      await waitForMapIdle(page);
      await assertZoomRemainsStable(page, 1);

      const zoomAfter = await getMapZoom(page);
      expect(zoomAfter).toBe(1);
    });

    test('respects zoom_max boundary', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Set zoom to max (22 per mock config)
      await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { setZoom: (z: number) => void } }).__stalagmight_map__;
        map?.setZoom(22);
      });
      await expect.poll(() => getMapZoom(page)).toBe(22);

      const zoomAtMax = await getMapZoom(page);
      expect(zoomAtMax).toBe(22);

      // Left click should be ignored at max zoom (no easeTo call, so nothing
      // to settle; waitForMapIdle still documents the intent).
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);
      await waitForMapIdle(page);
      await assertZoomRemainsStable(page, 22);

      const zoomAfter = await getMapZoom(page);
      expect(zoomAfter).toBe(22);
    });
  });

  test.describe('Wheel Zoom Interaction', () => {
    test('scroll wheel zooms in and out @smoke', async ({ page }) => {
      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Move mouse to center of canvas
      await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Scroll up (zoom in). Poll for the zoom to actually change rather
      // than sleeping a fixed 400ms - the scroll-zoom animation settles at
      // different rates under load (xvfb / 1 worker), which made the old
      // fixed wait flake even on master (issue #15).
      await page.mouse.wheel(0, -100);
      const zoomAfterScrollUp = await waitForZoomChange(page, initialZoom!, 'in');

      // Scroll down (zoom out)
      await page.mouse.wheel(0, 100);
      const zoomAfterScrollDown = await waitForZoomChange(page, zoomAfterScrollUp, 'out');
      expect(zoomAfterScrollDown).toBeLessThan(zoomAfterScrollUp);
    });

    test('wheel zoom works regardless of active tool', async ({ page }) => {
      // Switch to select tool
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);

      const initialZoom = await getMapZoom(page);
      expect(initialZoom).not.toBeNull();

      // Scroll up (zoom in) while in select mode - poll for the change
      // instead of a fixed sleep (see "scroll wheel zooms in and out").
      await page.mouse.wheel(0, -100);
      const newZoom = await waitForZoomChange(page, initialZoom!, 'in');
      expect(newZoom).toBeGreaterThan(initialZoom!);
    });
  });

  test.describe('Center on Click', () => {
    test('click-to-zoom centers map on click location', async ({ page }) => {
      // Switch to zoom tool
      await page.keyboard.press('z');
      await waitForActiveTool(page, 'zoom');

      const canvas = page.locator('canvas').first();
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      const initialCenter = await getMapCenter(page);
      expect(initialCenter).not.toBeNull();

      // Click off-center to zoom (but not too far to avoid map bounds issues)
      const clickX = box!.x + box!.width / 4;
      const clickY = box!.y + box!.height / 4;
      await page.mouse.click(clickX, clickY);

      // Wait for the easeTo zoom animation to settle
      await waitForMapIdle(page);

      const newCenter = await getMapCenter(page);
      expect(newCenter).not.toBeNull();

      // The map center should have changed toward the click location
      const centerChanged = Math.abs(newCenter!.lng - initialCenter!.lng) > 0.0001 ||
                            Math.abs(newCenter!.lat - initialCenter!.lat) > 0.0001;
      expect(centerChanged).toBe(true);
    });
  });
});
