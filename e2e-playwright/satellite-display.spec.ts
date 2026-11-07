import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage, createProject, waitForMapReady, openAppMenu, openSettingsFromMenu, CUSTOM_TILE_URL_RE } from './helpers/app';

/**
 * Helper to open the Display menu
 */
async function openDisplayMenu(page: Page): Promise<void> {
  await page.getByTestId('display-menu-button').click();
  await expect(page.locator('[data-testid="display-toggle-showSatellite"]')).toBeVisible({ timeout: 2000 });
}

/**
 * Helper to get the satellite toggle button and checkbox
 */
function getSatelliteToggle(page: Page) {
  const button = page.locator('[data-testid="display-toggle-showSatellite"]');
  const checkbox = button;
  return { button, checkbox };
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
 * Reads the current state rather than assuming one. Satellite is ON by default,
 * so a bare "click to enable" turns it OFF - which is how every one of these
 * tests broke when the default flipped. Asserting the destination instead of the
 * number of clicks keeps them honest whichever way the default goes next.
 */
async function setSatellite(page: Page, on: boolean): Promise<void> {
  await ensureDisplayMenuOpen(page);
  const { button, checkbox } = getSatelliteToggle(page);
  const isOn = await checkbox.evaluate((el) => el.className.includes('bg-accent'));
  if (isOn !== on) await button.click();
  if (on) {
    await expect(checkbox).toHaveClass(/bg-accent/);
  } else {
    await expect(checkbox).not.toHaveClass(/bg-accent/);
  }
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

/**
 * Set the mocked backend network status. Browser `context.setOffline()` alone is
 * not enough: useNetworkStatus polls `get_network_status` whenever it goes
 * offline, and the mock answers "online" by default, flipping state back. Tests
 * that assert the offline indicator must drive the mocked backend too.
 */
async function setMockNetwork(page: Page, online: boolean): Promise<void> {
  await page.evaluate((o) => {
    (window as unknown as { __MOCK_SET_NETWORK_STATUS__?: (b: boolean) => void }).__MOCK_SET_NETWORK_STATUS__?.(o);
  }, online);
}

test.describe('Satellite Display', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Satellite Test Project');
    await waitForMapReady(page);
  });

  test.describe('Toggle Behavior', () => {
    test('satellite is on by default', async ({ page }) => {
      await openDisplayMenu(page);
      const { checkbox } = getSatelliteToggle(page);
      await expect(checkbox).toHaveClass(/bg-accent/);
      expect(await waitForMapLayer(page, 'satellite-tiles')).toBe(true);
    });

    test('should toggle satellite OFF via display menu', async ({ page }) => {
      await openDisplayMenu(page);
      const { button, checkbox } = getSatelliteToggle(page);

      // On by default, so one click takes it off.
      await expect(checkbox).toHaveClass(/bg-accent/);
      await button.click();
      await expect(checkbox).not.toHaveClass(/bg-accent/);
    });

    test('should toggle satellite ON via display menu', async ({ page }) => {
      const { button, checkbox } = getSatelliteToggle(page);

      await setSatellite(page, false);

      await button.click();
      await expect(checkbox).toHaveClass(/bg-accent/);
    });

    test('should persist toggle state across menu interactions', async ({ page }) => {
      const { checkbox } = getSatelliteToggle(page);

      // Drive it AWAY from the default, so reopening proves the state was kept
      // rather than merely re-defaulted.
      await setSatellite(page, false);
      await expect(checkbox).not.toHaveClass(/bg-accent/);

      // Close menu (click outside or press Escape)
      await page.keyboard.press('Escape');
      // Wait for menu to close
      await expect(page.locator('[data-testid="display-toggle-showSatellite"]')).not.toBeVisible({ timeout: 2000 });

      // Reopen menu
      await openDisplayMenu(page);

      // Satellite should still be off
      const { checkbox: checkbox2 } = getSatelliteToggle(page);
      await expect(checkbox2).not.toHaveClass(/bg-accent/);
    });

    test('should handle rapid toggle stress test (10x without crash)', async ({ page }) => {
      await openDisplayMenu(page);
      const { button, checkbox } = getSatelliteToggle(page);

      // Rapid toggle 10 times, verifying state after each click. Starts ON, so
      // the first click takes it off and the parity is the reverse of what it
      // was when satellite defaulted to off.
      for (let i = 0; i < 10; i++) {
        await button.click();
        if (i % 2 === 0) {
          await expect(checkbox).not.toHaveClass(/bg-accent/);
        } else {
          await expect(checkbox).toHaveClass(/bg-accent/);
        }
      }

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Display menu should still work
      await expect(button).toBeVisible();
    });
  });

  test.describe('Layer Properties', () => {
    test('should add satellite layer to map when enabled', async ({ page }) => {
      await setSatellite(page, true);

      // Wait for layer using polling
      const hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);
    });

    test('should have correct layer opacity (0.7)', async ({ page }) => {
      await setSatellite(page, true);

      // Wait for layer first
      await waitForMapLayer(page, 'satellite-tiles');

      // Check layer opacity using polling
      await expect.poll(async () => {
        return await page.evaluate(() => {
          const map = (window as unknown as { __stalagmight_map__?: { getPaintProperty: (layer: string, prop: string) => number } }).__stalagmight_map__;
          return map ? map.getPaintProperty('satellite-tiles', 'raster-opacity') : null;
        });
      }, { timeout: 5000 }).toBe(0.7);
    });

    test('should have satellite layer ID as satellite-tiles', async ({ page }) => {
      await setSatellite(page, true);

      // Verify layer ID using polling
      const hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);
    });

    test('should use the custom tile protocol', async ({ page }) => {
      await setSatellite(page, true);

      // Wait for layer first
      await waitForMapLayer(page, 'satellite-tiles');

      // Check source uses custom protocol using polling
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

    test('should have source attribution present', async ({ page }) => {
      await setSatellite(page, true);

      // Wait for layer first
      await waitForMapLayer(page, 'satellite-tiles');

      // Check source attribution using polling
      await expect.poll(async () => {
        return await page.evaluate(() => {
          const map = (window as unknown as { __stalagmight_map__?: { getSource: (id: string) => { attribution?: string } | null } }).__stalagmight_map__;
          if (!map) return null;
          const source = map.getSource('satellite-tiles');
          return source?.attribution ?? null;
        });
      }, { timeout: 5000 }).toBeTruthy();

      const attributionText = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getSource: (id: string) => { attribution?: string } | null } }).__stalagmight_map__;
        if (!map) return '';
        const source = map.getSource('satellite-tiles');
        return source?.attribution ?? '';
      });

      expect(attributionText).toContain('Esri');
    });

    /**
     * The requirement these cover is display, not data. Every map in the app was
     * built with `attributionControl: false`, so the source string above was
     * present on the raster source and rendered nowhere - imagery drawn with no
     * attribution at all. Esri requires the source names to be visible for as
     * long as the imagery is, and permits "Powered by Esri" to live behind a
     * button instead: https://developers.arcgis.com/documentation/esri-and-data-attribution/interactive-maps/
     */
    test('shows the imagery source names on the map while satellite is on', async ({ page }) => {
      await setSatellite(page, true);
      await waitForMapLayer(page, 'satellite-tiles');

      await expect(page.getByTestId('map-attribution-text')).toBeVisible({ timeout: 5000 });
      // The mocked backend answers as an offline one would, so this is the
      // compiled-in fallback - which is exactly the string that has to hold up
      // in the field.
      await expect(page.getByTestId('map-attribution-text')).toContainText('Esri');
      await expect(page.getByTestId('map-attribution-text')).toContainText(
        'and the GIS User Community',
      );
    });

    test('removes the attribution when the imagery is switched off', async ({ page }) => {
      await setSatellite(page, true);
      await expect(page.getByTestId('map-attribution')).toBeVisible({ timeout: 5000 });

      await setSatellite(page, false);
      await waitForNoMapLayer(page, 'satellite-tiles');
      // Nothing to attribute once nothing is drawn.
      await expect(page.getByTestId('map-attribution')).toBeHidden();
    });

    test('keeps the "Powered by Esri" notice off the map and in About', async ({ page }) => {
      await setSatellite(page, true);
      await expect(page.getByTestId('map-attribution')).toBeVisible({ timeout: 5000 });
      await expect(page.getByTestId('map-attribution')).not.toContainText('Powered by');

      await openSettingsFromMenu(page);
      await page.getByTestId('settings-about-button').click();
      await expect(page.getByTestId('about-dialog')).toBeVisible({ timeout: 5000 });

      const notice = page.getByTestId('about-imagery-notice-link');
      await expect(notice).toBeVisible();
      await expect(notice).toHaveText('Powered by Esri');
      await expect(notice).toHaveAttribute('href', 'https://www.esri.com');
      await expect(page.getByTestId('about-imagery-sources')).toContainText('Esri');
    });

    test('should render satellite layer below survey data (z-order)', async ({ page }) => {
      await setSatellite(page, true);

      // Wait for layer first
      await waitForMapLayer(page, 'satellite-tiles');

      // Check layer order - satellite should be below survey layers
      const layerOrder = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getStyle: () => { layers: { id: string }[] } } }).__stalagmight_map__;
        if (!map) return [];
        const layers = map.getStyle().layers;
        return layers.map((l: { id: string }) => l.id);
      });

      const satelliteIndex = layerOrder.indexOf('satellite-tiles');
      // Satellite should be near the beginning (low index = rendered first = below)
      expect(satelliteIndex).toBeGreaterThanOrEqual(0);
      // If there are survey layers, satellite should be below them
      const surveyLayerIndex = layerOrder.findIndex((id: string) => id.includes('survey') || id.includes('section') || id.includes('shot'));
      if (surveyLayerIndex >= 0) {
        expect(satelliteIndex).toBeLessThan(surveyLayerIndex);
      }
    });
  });

  /**
   * Cache Status tests navigate back to GlobalView and open Settings. The way back
   * waits for either the unsaved-changes prompt or GlobalView, so a clean and a
   * dirty project both work. Settings from GlobalView is also covered in
   * e2e-playwright/ui-components.spec.ts.
   */
  test.describe('Cache Status', () => {
    /**
     * Helper to navigate back to GlobalView and open Settings dialog
     * Settings is only accessible from the GlobalView, not from within a project
     */
    async function openSettingsFromGlobalView(page: Page): Promise<void> {
      // Open the hamburger menu, then click Close Project (now a menu item)
      await openAppMenu(page);
      await page.getByTestId('back-button').click();

      // Close Project decides at the click (useProject's handleCloseProject reads
      // isDirty there): a dirty project asks about unsaved changes, a clean one
      // goes straight to GlobalView. Wait for whichever it did, instead of giving
      // an optional dialog 1.5 s to show up (#576 NF-W10FE-1).
      const discardButton = page.locator('button:has-text("Discard Changes")');
      await expect(discardButton.or(page.getByTestId('global-view'))).toBeVisible({ timeout: 8000 });
      if (await discardButton.isVisible()) {
        await discardButton.click();
      }

      // Wait for GlobalView to load
      await page.waitForSelector('[data-testid="global-view"], h1:has-text("Stalagmight")', { timeout: 8000 });

      // Open the hamburger menu, then click Settings
      await openAppMenu(page);
      await page.click('[data-testid="settings-button"], button:has-text("Settings")');
      await page.waitForSelector('[data-testid="settings-dialog"]', { timeout: 5000 });
    }

    test('should display cache stats in Settings Cache tab', async ({ page }) => {
      await openSettingsFromGlobalView(page);

      // Click Cache tab
      await page.click('[data-testid="settings-tab-cache"]');

      // Cache stats section should be visible
      const cacheStats = page.locator('[data-testid="cache-stats"]');
      await expect(cacheStats).toBeVisible({ timeout: 5000 });
    });

    test('should show tile count and size in cache stats', async ({ page }) => {
      await openSettingsFromGlobalView(page);
      await page.click('[data-testid="settings-tab-cache"]');

      // Check tile count is displayed
      const tileCount = page.locator('[data-testid="cache-tile-count"]');
      await expect(tileCount).toBeVisible();
      await expect(tileCount).toContainText('150'); // Mock data has 150 tiles

      // Check size is displayed
      const cacheSize = page.locator('[data-testid="cache-size"]');
      await expect(cacheSize).toBeVisible();
      await expect(cacheSize).toContainText('MB'); // Should show size in MB
    });

    test('should have Clear cache button visible', async ({ page }) => {
      await openSettingsFromGlobalView(page);
      await page.click('[data-testid="settings-tab-cache"]');

      const clearButton = page.locator('[data-testid="clear-cache-button"]');
      await expect(clearButton).toBeVisible();
    });

    test('should clear cache and reset stats to 0', async ({ page }) => {
      await openSettingsFromGlobalView(page);
      await page.click('[data-testid="settings-tab-cache"]');

      // Click clear cache
      const clearButton = page.locator('[data-testid="clear-cache-button"]');
      await clearButton.click();

      // Tile count should be 0 (using polling)
      const tileCount = page.locator('[data-testid="cache-tile-count"]');
      await expect(tileCount).toHaveText('0');

      // Clear button should be disabled when cache is empty
      await expect(clearButton).toBeDisabled();
    });

    test('should access cache tab via settings navigation', async ({ page }) => {
      await openSettingsFromGlobalView(page);

      // General tab should be visible
      const generalTab = page.locator('[data-testid="settings-tab-general"]');
      await expect(generalTab).toBeVisible();

      // Cache tab should be visible and clickable
      const cacheTab = page.locator('[data-testid="settings-tab-cache"]');
      await expect(cacheTab).toBeVisible();
      await cacheTab.click();

      // Cache content should now be visible
      await expect(page.locator('[data-testid="cache-stats"]')).toBeVisible();
    });
  });

  test.describe('Offline Mode', () => {
    test('should show offline indicator when browser is offline', async ({ page, context }) => {
      // Use Playwright's network emulation for proper offline simulation
      await context.setOffline(true);
      await setMockNetwork(page, false);

      // Trigger a network status check by simulating offline event
      await page.evaluate(() => {
        window.dispatchEvent(new Event('offline'));
      });

      // Wait for offline indicator to appear
      const offlineIndicator = page.locator('[data-testid="offline-indicator"]');
      await expect(offlineIndicator).toBeVisible({ timeout: 5000 });
      await expect(offlineIndicator).toContainText('Offline');
    });

    test('should hide offline indicator when back online', async ({ page, context }) => {
      // First go offline
      await context.setOffline(true);
      await setMockNetwork(page, false);
      await page.evaluate(() => {
        window.dispatchEvent(new Event('offline'));
      });

      // Verify offline indicator appears
      const offlineIndicator = page.locator('[data-testid="offline-indicator"]');
      await expect(offlineIndicator).toBeVisible({ timeout: 5000 });

      // Go back online
      await context.setOffline(false);
      await setMockNetwork(page, true);
      await page.evaluate(() => {
        window.dispatchEvent(new Event('online'));
      });

      // Offline indicator should disappear
      await expect(offlineIndicator).not.toBeVisible({ timeout: 5000 });
    });

    test('should handle tile load failures gracefully (no crash)', async ({ page, context }) => {
      await setSatellite(page, true);

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Go offline using proper network emulation
      await context.setOffline(true);
      await page.evaluate(() => {
        window.dispatchEvent(new Event('offline'));
      });

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Map should still be interactive
      const canvas = page.getByTestId('map-canvas');
      await canvas.hover();
      await page.mouse.wheel(0, -100);
      await expect(canvas).toBeVisible();
    });

    test('should serve cached tiles when offline (mock scenario)', async ({ page, context }) => {
      // Enable satellite first while online
      await setSatellite(page, true);

      // Wait for layer to be added
      await waitForMapLayer(page, 'satellite-tiles');

      // Go offline using proper network emulation
      await context.setOffline(true);
      await setMockNetwork(page, false);
      await page.evaluate(() => {
        window.dispatchEvent(new Event('offline'));
      });

      // Wait for offline indicator
      await expect(page.locator('[data-testid="offline-indicator"]')).toBeVisible({ timeout: 5000 });

      // Satellite layer should still exist (using cached tiles)
      const hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);
    });

    test('should toggle satellite while offline without crash', async ({ page, context }) => {
      // Go offline first using proper network emulation
      await context.setOffline(true);
      await setMockNetwork(page, false);
      await page.evaluate(() => {
        window.dispatchEvent(new Event('offline'));
      });

      // Wait for offline state
      await expect(page.locator('[data-testid="offline-indicator"]')).toBeVisible({ timeout: 5000 });

      // Toggle satellite both ways while offline
      await setSatellite(page, false);

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      await setSatellite(page, true);

      // App should still be functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });
});
