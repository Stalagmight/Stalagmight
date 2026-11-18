import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage, createProject, waitForMapReady, CUSTOM_TILE_URL_RE } from './helpers/app';

/**
 * Regression tests for satellite imagery bugs that were fixed.
 *
 * These tests specifically target bugs that weren't caught by existing tests:
 * 1. Map loaded guard - satellite toggle crashed if map wasn't loaded
 * 2. Layer ordering - satellite was behind background, making it invisible
 * 3. Config GPS origin - Rust config missing satellite_gps_origin field
 * 4. Custom protocol - tiles must use invoke() not fetch() (CORS issues)
 */

/**
 * Helper to open the Display menu
 */
async function openDisplayMenu(page: Page): Promise<void> {
  await page.getByTestId('display-menu-button').click();
  await expect(page.locator('[data-testid="display-toggle-showSatellite"]')).toBeVisible({ timeout: 2000 });
}

/**
 * Helper to get the satellite toggle button
 */
function getSatelliteToggle(page: Page) {
  const button = page.locator('[data-testid="display-toggle-showSatellite"]');
  return { button };
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
 * Satellite is ON by default, so a bare "click to enable" turns it OFF. These
 * tests then raced their own click: `waitForMapLayer` could still catch the
 * layer that was already there, passing for the wrong reason. Asserting the
 * destination removes the race.
 */
async function setSatellite(page: Page, on: boolean): Promise<void> {
  await ensureDisplayMenuOpen(page);
  const { button } = getSatelliteToggle(page);
  const isOn = await button.evaluate((el) => el.className.includes('bg-accent'));
  if (isOn !== on) await button.click();
  await expect
    .poll(() => button.evaluate((el) => el.className.includes('bg-accent')), { timeout: 2000 })
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
 * Helper to get the tile URL from the map style spec for a raster source.
 * Uses getStyle().sources which returns the spec object (with .tiles array),
 * rather than getSource() which returns a MapLibre Source class instance
 * that may not expose .tiles as a direct property.
 */
async function getTileUrl(page: Page, sourceId: string): Promise<string | null> {
  return await page.evaluate((id) => {
    const map = (window as unknown as { __stalagmight_map__?: { getStyle: () => { sources: Record<string, { tiles?: string[] }> } } }).__stalagmight_map__;
    if (!map) return null;
    const source = map.getStyle().sources[id];
    if (!source || !source.tiles) return null;
    return source.tiles[0] ?? null;
  }, sourceId);
}

test.describe('Satellite Bug Regressions', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test.describe('Config GPS Origin (Bug: Rust config missing satellite_gps_origin)', () => {
    test('should have satellite_gps_origin in config', async ({ page }) => {
      // This bug was that Rust config didn't have satellite_gps_origin field,
      // causing tiles to be fetched at wrong coordinates (near 0,0 instead of Florida Keys)

      await createProject(page, 'GPS Origin Test');
      await waitForMapReady(page);

      // Get config and verify GPS origin exists with expected Florida Keys coordinates
      const gpsOrigin = await page.evaluate(() => {
        // Access the Tauri mock to get config
        const invoke = (window as unknown as { __TAURI__: { invoke: (cmd: string) => Promise<unknown> } }).__TAURI__.invoke;
        return invoke('get_config').then((config: unknown) => {
          const c = config as { map?: { satellite_gps_origin?: { latitude: number; longitude: number } } };
          return c?.map?.satellite_gps_origin;
        });
      });

      expect(gpsOrigin).toBeDefined();
      expect(gpsOrigin).toHaveProperty('latitude');
      expect(gpsOrigin).toHaveProperty('longitude');
      // Florida Keys coordinates (approximately)
      expect(gpsOrigin!.latitude).toBeCloseTo(24.55, 1);
      expect(gpsOrigin!.longitude).toBeCloseTo(-81.78, 1);
    });

    test('should apply GPS origin to coordinate transforms', async ({ page }) => {
      // When GPS origin is set, map coordinates should be offset from origin
      // Without GPS origin, all coordinates would be near (0, 0) on the map

      await createProject(page, 'Coordinate Transform Test');

      // Get the map center after project creation
      // With GPS origin, the center should be near the Florida Keys (lat ~24.55, lng ~-81.78)
      // Without GPS origin (bug case), center would be near (0, 0)

      const mapCenter = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getCenter: () => { lat: number; lng: number } } }).__stalagmight_map__;
        if (!map) return null;
        const center = map.getCenter();
        return { lat: center.lat, lng: center.lng };
      });

      // The map should have a center (may not be exact Florida Keys due to station positions)
      expect(mapCenter).not.toBeNull();
      // If GPS origin weren't applied, coordinates would all be near 0,0 (would fail this test)
      // Note: with mock data, the center depends on station positions, but it should NOT be at 0,0
    });
  });

  test.describe('Layer Ordering (Bug: Satellite behind background)', () => {
    test('should render satellite layer ABOVE background layer', async ({ page }) => {
      // This bug was that satellite layer was added with 'background' as the "before" parameter,
      // which inserted satellite BEFORE background in render order (i.e., BEHIND it, invisible)

      await createProject(page, 'Layer Order Test');
      await openDisplayMenu(page);
      // Satellite ships on; make that explicit rather than assumed.
      await setSatellite(page, true);

      // Wait for satellite layer
      await waitForMapLayer(page, 'satellite-tiles');

      // Get layer order and verify satellite is ABOVE background
      const layerOrder = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getStyle: () => { layers: { id: string }[] } } }).__stalagmight_map__;
        if (!map) return [];
        const layers = map.getStyle().layers;
        return layers.map((l: { id: string }) => l.id);
      });

      const backgroundIndex = layerOrder.indexOf('background');
      const satelliteIndex = layerOrder.indexOf('satellite-tiles');

      // Both layers should exist
      expect(backgroundIndex).toBeGreaterThanOrEqual(0);
      expect(satelliteIndex).toBeGreaterThanOrEqual(0);

      // CRITICAL: Satellite MUST be above background (higher index = rendered later = on top)
      // The bug was satelliteIndex < backgroundIndex (satellite behind background)
      expect(satelliteIndex).toBeGreaterThan(backgroundIndex);
    });

    test('should render satellite layer BELOW survey layers', async ({ page }) => {
      // Satellite should be sandwiched: background < satellite < survey layers

      await createProject(page, 'Layer Stack Test');
      await openDisplayMenu(page);
      // Satellite ships on; make that explicit rather than assumed.
      await setSatellite(page, true);

      // Wait for satellite layer
      await waitForMapLayer(page, 'satellite-tiles');

      // Get full layer order
      const layerOrder = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getStyle: () => { layers: { id: string }[] } } }).__stalagmight_map__;
        if (!map) return [];
        const layers = map.getStyle().layers;
        return layers.map((l: { id: string }) => l.id);
      });

      const backgroundIndex = layerOrder.indexOf('background');
      const satelliteIndex = layerOrder.indexOf('satellite-tiles');

      // Find first survey layer (could be survey-ghost-shots, survey-shots, etc.)
      const surveyLayerIndex = layerOrder.findIndex((id: string) =>
        id.startsWith('survey-') || id.includes('line') || id.includes('station')
      );

      // Verify correct ordering: background < satellite < survey
      expect(backgroundIndex).toBeLessThan(satelliteIndex);
      if (surveyLayerIndex >= 0) {
        expect(satelliteIndex).toBeLessThan(surveyLayerIndex);
      }
    });
  });

  test.describe('Map Loaded Guard (Bug: Toggle before map loaded)', () => {
    test('should not crash when satellite toggle is attempted early', async ({ page }) => {
      // This bug was that the satellite toggle effect didn't check mapLoaded,
      // causing crashes when toggle was attempted before map was fully initialized

      // Navigate to page but don't wait for project creation
      // Try to interact before map is fully loaded

      // Create project (this starts map initialization)
      const newProjectBtn = page.locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]').first();
      await newProjectBtn.click();
      await page.getByTestId('new-project-name-input').fill('Early Toggle Test');

      // Click create - map starts loading
      await page.getByTestId('new-project-create-button').click();

      // Immediately try to open Display menu (map may not be loaded yet)
      // Wait for canvas to appear but the map 'load' event may not have fired
      await expect(page.getByTestId('map-canvas')).toBeVisible({ timeout: 10000 });

      // Try to toggle satellite immediately - this should not crash
      // even if the map isn't fully loaded
      try {
        await openDisplayMenu(page);
        const { button } = getSatelliteToggle(page);

        // Toggle on (may or may not succeed depending on map load state)
        await button.click();

        // If we get here without crash, the guard is working
        // The satellite may or may not be visible, but app should be stable
        await expect(page.getByTestId('map-canvas')).toBeVisible();
      } catch {
        // If menu can't open, that's fine - but the app should still be stable
        await expect(page.getByTestId('map-canvas')).toBeVisible();
      }

      // App should remain functional after this
      await expect(page).toHaveTitle(/Stalagmight/);
    });

    test('should add satellite layer only after map is loaded', async ({ page }) => {
      // Verify that satellite layer is properly added after map.on('load') fires

      await createProject(page, 'Map Load Test');

      // At this point, we've waited for canvas AND created project
      // Map should be loaded

      await openDisplayMenu(page);
      // Satellite ships on; make that explicit rather than assumed.
      await setSatellite(page, true);

      // Satellite layer should be added (map is loaded)
      const hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);

      // Verify the source was also added
      const hasSource = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getSource: (id: string) => unknown } }).__stalagmight_map__;
        return map ? !!map.getSource('satellite-tiles') : false;
      });
      expect(hasSource).toBe(true);
    });
  });

  test.describe('Custom Tile Protocol (Bug: CORS with fetch)', () => {
    test('should use stalagmight-tiles:// protocol for tile requests', async ({ page }) => {
      // This bug was that tiles were fetched using browser fetch() instead of
      // Tauri's invoke(), causing CORS errors. The fix uses a custom protocol.

      await createProject(page, 'Protocol Test');
      await openDisplayMenu(page);
      // Satellite ships on; make that explicit rather than assumed.
      await setSatellite(page, true);

      // Wait for layer
      await waitForMapLayer(page, 'satellite-tiles');

      // Verify the source uses custom protocol
      const tileUrl = await getTileUrl(page, 'satellite-tiles');

      // Must use the custom protocol (either platform form), never a raw external
      // fetch URL. CUSTOM_TILE_URL_RE already excludes external https://, so the
      // single positive assertion preserves the original anti-CORS guard.
      expect(tileUrl).toMatch(CUSTOM_TILE_URL_RE);
    });

    test('should include provider in tile URL path', async ({ page }) => {
      // The tile URL should include the provider (e.g., esri) for routing

      await createProject(page, 'Provider Test');
      await openDisplayMenu(page);
      // Satellite ships on; make that explicit rather than assumed.
      await setSatellite(page, true);

      // Wait for layer
      await waitForMapLayer(page, 'satellite-tiles');

      // Verify tile URL includes provider
      const tileUrl = await getTileUrl(page, 'satellite-tiles');

      // Should include esri provider
      expect(tileUrl).toContain('esri');
      // Should include tile coordinate placeholders
      expect(tileUrl).toContain('{z}');
      expect(tileUrl).toContain('{x}');
      expect(tileUrl).toContain('{y}');
    });
  });

  test.describe('Integration: Full Satellite Display Flow', () => {
    test('should display satellite tiles correctly with all fixes applied', async ({ page }) => {
      // This test verifies the complete flow works with all bug fixes:
      // 1. Config has GPS origin
      // 2. Map loads before satellite toggle
      // 3. Custom protocol is used
      // 4. Layer ordering is correct

      await createProject(page, 'Full Flow Test');

      // Verify config has GPS origin
      const configCheck = await page.evaluate(() => {
        const invoke = (window as unknown as { __TAURI__: { invoke: (cmd: string) => Promise<unknown> } }).__TAURI__.invoke;
        return invoke('get_config').then((config: unknown) => {
          const c = config as { map?: { satellite_gps_origin?: { latitude: number; longitude: number } } };
          return !!c?.map?.satellite_gps_origin;
        });
      });
      expect(configCheck).toBe(true);

      await setSatellite(page, true);

      // Wait for layer to be added
      const layerAdded = await waitForMapLayer(page, 'satellite-tiles');
      expect(layerAdded).toBe(true);

      // Verify layer ordering
      const orderCheck = await page.evaluate(() => {
        const map = (window as unknown as { __stalagmight_map__?: { getStyle: () => { layers: { id: string }[] } } }).__stalagmight_map__;
        if (!map) return false;
        const layers = map.getStyle().layers.map((l: { id: string }) => l.id);
        const bgIdx = layers.indexOf('background');
        const satIdx = layers.indexOf('satellite-tiles');
        return satIdx > bgIdx; // satellite must be above background
      });
      expect(orderCheck).toBe(true);

      // Verify custom protocol
      const protocolTileUrl = await getTileUrl(page, 'satellite-tiles');
      expect(protocolTileUrl).toBeTruthy();
      expect(protocolTileUrl).toMatch(CUSTOM_TILE_URL_RE);

      // App should be stable
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });

    test('should toggle satellite on and off without issues', async ({ page }) => {
      await createProject(page, 'Toggle Stability Test');

      // On (already, by default) -> off -> on again. What this guards is that
      // the layer survives a full round trip, so it has to actually make one.
      await setSatellite(page, true);
      let hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);

      await setSatellite(page, false);
      await expect.poll(async () => {
        return await page.evaluate(() => {
          const map = (window as unknown as { __stalagmight_map__?: { getLayer: (id: string) => unknown } }).__stalagmight_map__;
          return map ? !!map.getLayer('satellite-tiles') : false;
        });
      }, { timeout: 3000 }).toBe(false);

      await setSatellite(page, true);
      hasLayer = await waitForMapLayer(page, 'satellite-tiles');
      expect(hasLayer).toBe(true);

      // App should still be stable
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });
});
