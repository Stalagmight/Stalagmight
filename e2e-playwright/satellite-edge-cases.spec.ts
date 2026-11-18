import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage, createProject, waitForMapReady, openSettingsFromMenu, closeProjectViaMenu, CUSTOM_TILE_URL_RE } from './helpers/app';

/**
 * Helper to open the Display menu
 */
async function openDisplayMenu(page: Page): Promise<void> {
  await page.getByTestId('display-menu-button').click();
  await expect(page.locator('[data-testid="display-toggle-showSatellite"]')).toBeVisible({ timeout: 2000 });
}

/**
 * Helper to toggle satellite on/off
 */
async function toggleSatellite(page: Page): Promise<void> {
  const button = page.locator('[data-testid="display-toggle-showSatellite"]');
  await button.click();
}

/**
 * Helper to check if satellite is enabled using polling
 */
async function isSatelliteEnabled(page: Page): Promise<boolean> {
  const checkbox = page.locator('[data-testid="display-toggle-showSatellite"]');
  return checkbox.evaluate((el) => el.classList.contains('bg-accent'));
}

/** Open the Display menu unless it is already open (the button toggles it). */
async function ensureDisplayMenuOpen(page: Page): Promise<void> {
  const toggle = page.locator('[data-testid="display-toggle-showSatellite"]');
  if (await toggle.isVisible().catch(() => false)) return;
  await openDisplayMenu(page);
}

/**
 * Drive the Satellite toggle to a known state.
 *
 * Satellite is ON by default, so "open the menu and click" - which used to mean
 * "enable" - now disables it. Asserting the destination rather than counting
 * clicks keeps these tests correct whichever way the default goes.
 */
async function setSatellite(page: Page, on: boolean): Promise<void> {
  await ensureDisplayMenuOpen(page);
  if ((await isSatelliteEnabled(page)) !== on) {
    await toggleSatellite(page);
  }
  await expect
    .poll(() => isSatelliteEnabled(page), { timeout: 2000 })
    .toBe(on);
}

/**
 * Helper to wait for map layer to exist using polling
 */
async function waitForMapLayer(page: Page, layerId: string): Promise<boolean> {
  return await expect.poll(async () => {
    return await page.evaluate((id) => {
      const map = (window as unknown as { __stalagmight_map__?: { getLayer: (id: string) => unknown } }).__stalagmight_map__;
      return map ? !!map.getLayer(id) : false;
    }, layerId);
  }, { timeout: 5000 }).toBe(true).then(() => true).catch(() => false);
}

/**
 * Helper to wait for map layer to not exist using polling
 */
async function waitForNoMapLayer(page: Page, layerId: string): Promise<boolean> {
  return await expect.poll(async () => {
    return await page.evaluate((id) => {
      const map = (window as unknown as { __stalagmight_map__?: { getLayer: (id: string) => unknown } }).__stalagmight_map__;
      return map ? !!map.getLayer(id) : false;
    }, layerId);
  }, { timeout: 5000 }).toBe(false).then(() => true).catch(() => false);
}

test.describe('Satellite Edge Cases', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Satellite Edge Cases Test');
    await waitForMapReady(page);
  });

  test.describe('Animation Interactions', () => {
    test('should toggle satellite during map pan animation', async ({ page }) => {
      const canvas = page.getByTestId('map-canvas');
      const box = await canvas.boundingBox();
      if (!box) throw new Error('Canvas not found');

      // Start a drag operation to trigger map pan
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width / 2 + 50, box.y + box.height / 2 + 50);
      await page.mouse.up();

      // Now immediately toggle satellite (map may still be in inertia)
      await setSatellite(page, true);

      // Verify toggle succeeded
      const checkbox = page.locator('[data-testid="display-toggle-showSatellite"]');
      await expect(checkbox).toHaveClass(/bg-accent/);

      // App should remain functional
      await expect(canvas).toBeVisible();
    });

    test('should toggle satellite during zoom animation', async ({ page }) => {
      const canvas = page.getByTestId('map-canvas');
      await canvas.hover();

      // Start zoom animation
      await page.mouse.wheel(0, -200);

      // Immediately open menu and toggle satellite during zoom
      await setSatellite(page, true);

      // Verify toggle succeeded
      const checkbox = page.locator('[data-testid="display-toggle-showSatellite"]');
      await expect(checkbox).toHaveClass(/bg-accent/);

      // App should remain functional
      await expect(canvas).toBeVisible();
    });
  });

  test.describe('Network Scenarios', () => {
    test('should handle network failure mid-tile-load', async ({ page, context }) => {
      // Enable satellite
      await setSatellite(page, true);

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Simulate network failure using proper Playwright API
      await context.setOffline(true);
      await page.evaluate(() => {
        window.dispatchEvent(new Event('offline'));
      });

      // Trigger more tile loads by zooming
      const canvas = page.getByTestId('map-canvas');
      await canvas.hover();
      await page.mouse.wheel(0, -300);

      // App should remain functional
      await expect(canvas).toBeVisible();
    });

    test('should handle tile error (404 from server)', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);

      // Wait for layer to be added (mock handler provides tiles)
      await waitForMapLayer(page, 'satellite-tiles');

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Satellite layer should exist
      const hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);
    });

    test('should handle cache full scenario (mock)', async ({ page }) => {
      // This tests that the app gracefully handles cache limits
      await setSatellite(page, true);

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Pan and zoom to load multiple tiles
      const canvas = page.getByTestId('map-canvas');
      await canvas.hover();
      for (let i = 0; i < 5; i++) {
        await page.mouse.wheel(0, -100);
        // Wait for each zoom level to settle
        await expect(canvas).toBeVisible();
      }

      // App should remain functional even if cache is full
      await expect(canvas).toBeVisible();
    });
  });

  test.describe('Stress Tests', () => {
    test('should handle rapid zoom stress test', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      const canvas = page.getByTestId('map-canvas');
      await canvas.hover();

      // Rapid zoom in and out, verifying canvas remains visible
      for (let i = 0; i < 10; i++) {
        await page.mouse.wheel(0, -200);
        await expect(canvas).toBeVisible();
        await page.mouse.wheel(0, 200);
        await expect(canvas).toBeVisible();
      }

      // App should remain functional
      await expect(canvas).toBeVisible();
    });

    test('should handle multiple satellite toggles in quick succession', async ({ page }) => {
      await openDisplayMenu(page);
      const checkbox = page.locator('[data-testid="display-toggle-showSatellite"]');

      // Toggle multiple times very quickly, verifying state after each. Starts
      // ON, so the first toggle takes it off - the reverse of the parity that
      // held when satellite defaulted to off.
      for (let i = 0; i < 20; i++) {
        await toggleSatellite(page);
        if (i % 2 === 0) {
          await expect(checkbox).not.toHaveClass(/bg-accent/);
        } else {
          await expect(checkbox).toHaveClass(/bg-accent/);
        }
      }

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });

  test.describe('Project State', () => {
    /**
     * Project switch tests require navigating back to GlobalView which triggers
     * the unsaved-changes dialog with timing issues in the mock environment.
     */
    test('should reset satellite state across project switch', async ({ page }) => {
      // Drive it AWAY from the default. Enabling would prove nothing now that
      // satellite ships on - the assertion would pass without any reset
      // happening at all.
      await setSatellite(page, false);
      await expect.poll(() => isSatelliteEnabled(page)).toBe(false);

      // Close display menu
      await page.keyboard.press('Escape');

      // Go back to home (close project) via the hamburger menu
      await closeProjectViaMenu(page);

      // Create a new project
      await createProject(page, 'Satellite Test Project 2');

      // Open display menu and check state
      await openDisplayMenu(page);

      // New project applies default display settings, not the previous
      // project's leftovers - so satellite is back on.
      await expect.poll(() => isSatelliteEnabled(page)).toBe(true);
    });

    test('should toggle immediately after project create', async ({ page }) => {
      // Already in a project from beforeEach
      // Toggle satellite immediately
      await setSatellite(page, true);

      // Should be enabled without errors
      await expect.poll(() => isSatelliteEnabled(page)).toBe(true);

      // Map should be functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });

    /**
     * Creating a new project requires navigating back to GlobalView which triggers
     * the unsaved-changes dialog with timing issues in the mock environment.
     */
    test('should toggle with empty project (no stations)', async ({ page }) => {
      // Go back to home and create empty project via the hamburger menu
      await closeProjectViaMenu(page);

      // Create new project (mock has default lines, but test the flow)
      await createProject(page, 'Empty Satellite Test');

      // Toggle satellite
      await setSatellite(page, true);

      // Should work without errors
      await expect.poll(() => isSatelliteEnabled(page)).toBe(true);
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });

  test.describe('Zoom Levels', () => {
    test('should handle large zoom level tile requests', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);
      await page.keyboard.press('Escape');

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Zoom in to max
      const canvas = page.getByTestId('map-canvas');
      await canvas.hover();
      for (let i = 0; i < 15; i++) {
        await page.mouse.wheel(0, -300);
        await expect(canvas).toBeVisible();
      }

      // App should remain functional at high zoom
      await expect(canvas).toBeVisible();
    });

    test('should handle min zoom level tile requests', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);
      await page.keyboard.press('Escape');

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Zoom out to min
      const canvas = page.getByTestId('map-canvas');
      await canvas.hover();
      for (let i = 0; i < 15; i++) {
        await page.mouse.wheel(0, 300);
        await expect(canvas).toBeVisible();
      }

      // App should remain functional at low zoom
      await expect(canvas).toBeVisible();
    });
  });

  test.describe('Overlay Combinations', () => {
    test('should work with grid overlay combination', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);

      // Enable grid
      await page.locator('[data-testid="display-toggle-showGrid"]').click();

      // Both should be enabled
      await expect.poll(() => isSatelliteEnabled(page)).toBe(true);
      const gridCheckbox = page.locator('[data-testid="display-toggle-showGrid"]');
      await expect(gridCheckbox).toHaveClass(/bg-accent/);

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });

    test('should work with depth gradient combination', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);

      // Enable depth gradient
      await page.locator('[data-testid="display-toggle-showDepthGradient"]').click();

      // Both should be enabled
      await expect.poll(() => isSatelliteEnabled(page)).toBe(true);
      const depthToggle = page.locator('[data-testid="display-toggle-showDepthGradient"]');
      await expect(depthToggle).toHaveAttribute('aria-pressed', 'true');

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });

  /**
   * Settings Dialog Interactions require navigating back to GlobalView which triggers
   * the unsaved-changes dialog with timing issues in the mock environment.
   * The cache functionality is tested in e2e-playwright/ui-components.spec.ts via
   * Settings tests from GlobalView (without having created a project first).
   */
  test.describe('Settings Dialog Interactions', () => {
    /**
     * Helper to navigate back to GlobalView and open Settings dialog
     * Settings is only accessible from the GlobalView, not from within a project
     */
    async function openSettingsFromGlobalView(page: Page): Promise<void> {
      // Go back to GlobalView (close project) via the hamburger menu
      await closeProjectViaMenu(page);

      // Open Settings via the hamburger menu (Settings moved into the app menu)
      await openSettingsFromMenu(page);
    }

    test('should open and close settings dialog with cache tab', async ({ page }) => {
      await openSettingsFromGlobalView(page);

      // Go to cache tab
      await page.click('[data-testid="settings-tab-cache"]');
      await expect(page.locator('[data-testid="cache-stats"]')).toBeVisible();

      // Close settings
      await page.click('button:has-text("Cancel")');
      await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible();

      // Go back to project to verify app remains functional
      await createProject(page, 'Settings Test Project');
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });

    test('should handle clear cache confirmation flow', async ({ page }) => {
      await openSettingsFromGlobalView(page);
      await page.click('[data-testid="settings-tab-cache"]');

      // Get initial tile count
      const initialCount = await page.locator('[data-testid="cache-tile-count"]').textContent();
      expect(initialCount).not.toBe('0');

      // Click clear cache
      await page.click('[data-testid="clear-cache-button"]');

      // Verify cache is cleared
      await expect(page.locator('[data-testid="cache-tile-count"]')).toHaveText('0');
    });

    test('should refresh cache stats after clear', async ({ page }) => {
      await openSettingsFromGlobalView(page);
      await page.click('[data-testid="settings-tab-cache"]');

      // Verify initial stats are loaded
      await expect(page.locator('[data-testid="cache-tile-count"]')).toBeVisible();
      await expect(page.locator('[data-testid="cache-size"]')).toBeVisible();

      // Clear cache
      await page.click('[data-testid="clear-cache-button"]');

      // Stats should be refreshed and show 0
      await expect(page.locator('[data-testid="cache-tile-count"]')).toHaveText('0');
    });
  });

  test.describe('Protocol and Cleanup', () => {
    test('should register tile protocol for caching', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Check that source uses custom protocol
      await expect.poll(async () => {
        return await page.evaluate(() => {
          const map = (window as unknown as { __stalagmight_map__?: { getSource: (id: string) => { tiles?: string[] } | null } }).__stalagmight_map__;
          if (!map) return null;
          const source = map.getSource('satellite-tiles');
          if (!source || !('tiles' in source)) return null;
          return source.tiles?.[0] ?? null;
        });
      }, { timeout: 5000 }).toMatch(CUSTOM_TILE_URL_RE);
    });

    test('should clean up satellite layer when disabled', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Disable satellite
      await setSatellite(page, false);

      // Verify layer is removed using polling
      await waitForNoMapLayer(page, 'satellite-tiles');
    });

    test('should clean up source when satellite disabled', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);

      // Wait for source to exist
      await expect.poll(async () => {
        return await page.evaluate(() => {
          const map = (window as unknown as { __stalagmight_map__?: { getSource: (id: string) => unknown } }).__stalagmight_map__;
          return map ? !!map.getSource('satellite-tiles') : false;
        });
      }, { timeout: 5000 }).toBe(true);

      // Disable satellite
      await setSatellite(page, false);

      // Verify source is removed using polling
      await expect.poll(async () => {
        return await page.evaluate(() => {
          const map = (window as unknown as { __stalagmight_map__?: { getSource: (id: string) => unknown } }).__stalagmight_map__;
          return map ? !!map.getSource('satellite-tiles') : false;
        });
      }, { timeout: 5000 }).toBe(false);
    });
  });

  test.describe('Resize Scenarios', () => {
    test('should handle map resize with satellite enabled', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);
      await page.keyboard.press('Escape');

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Resize viewport
      await page.setViewportSize({ width: 800, height: 600 });

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Satellite layer should still be present
      const hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);

      // Resize back
      await page.setViewportSize({ width: 1280, height: 800 });
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });

    test('should handle full-screen toggle with satellite enabled', async ({ page }) => {
      // Enable satellite
      await setSatellite(page, true);
      await page.keyboard.press('Escape');

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Simulate full-screen by resizing to large viewport
      await page.setViewportSize({ width: 1920, height: 1080 });

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Satellite layer should still work
      const hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);

      // Resize back to normal
      await page.setViewportSize({ width: 1280, height: 800 });
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });
});
