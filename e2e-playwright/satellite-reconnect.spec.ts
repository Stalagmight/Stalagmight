import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, waitForMapIdle } from './helpers/app';

/**
 * Satellite tiles that failed while the connection was down come back when it
 * returns (#576 D18 / NF-MAP-2). All three maps (survey map, Global View, section
 * preview) had a reload-on-reconnect effect, but it called a `clearTiles` the
 * MapLibre source does not have, so it never ran: a tile that errored offline
 * stayed blank until the user panned it away.
 *
 * The tile protocol is stubbed by the IPC mock; here it is overridden to fail
 * while "offline" and serve after. The oracle is a re-request of a tile URL that
 * FAILED: MapLibre does fetch other tiles on its own after a failure (parent tiles
 * to stand in for errored children), so "more requests" alone proves nothing.
 */

const STUB_TILE_PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);

interface TileRoute {
  requests: () => number;
  /** Requests, once serving again, for a tile URL that had failed. */
  retriedFailures: () => number;
  setFailing: (failing: boolean) => void;
}

/** Override the mock's tile route (later routes win) with a switchable failure. */
async function routeTiles(page: Page): Promise<TileRoute> {
  let failing = true;
  let requests = 0;
  let retried = 0;
  const failed = new Set<string>();
  await page.route(/stalagmight-tiles(\.localhost)?/, (route) => {
    requests += 1;
    const url = route.request().url();
    if (failing) {
      failed.add(url);
      return route.abort('internetdisconnected');
    }
    if (failed.has(url)) retried += 1;
    return route.fulfill({
      status: 200,
      contentType: 'image/png',
      headers: { 'Access-Control-Allow-Origin': '*' },
      body: STUB_TILE_PNG,
    });
  });
  return { requests: () => requests, retriedFailures: () => retried, setFailing: (f) => (failing = f) };
}

/** Wait until no tile has been requested for half a second: the view has settled. */
async function waitForTileQuiet(tiles: TileRoute): Promise<void> {
  let last = -1;
  await expect
    .poll(
      () => {
        const now = tiles.requests();
        const quiet = now > 0 && now === last;
        last = now;
        return quiet;
      },
      { intervals: [500], timeout: 15000 },
    )
    .toBe(true);
}

async function isOnline(page: Page): Promise<boolean> {
  return page.evaluate(
    () =>
      (window as unknown as { __E2E_TEST__: { networkStore: { getState: () => { isOnline: boolean } } } }).__E2E_TEST__.networkStore.getState()
        .isOnline,
  );
}

/** Drive the app's own offline -> online transition (useNetworkStatus), as a dropped link would. */
async function goOfflineThenOnline(page: Page): Promise<void> {
  await page.evaluate(() => {
    const set = (window as unknown as Record<string, unknown>).__MOCK_SET_NETWORK_STATUS__ as (s: boolean) => void;
    set(false);
    window.dispatchEvent(new Event('offline'));
  });
  await expect.poll(() => isOnline(page)).toBe(false);
  await page.evaluate(() => {
    const set = (window as unknown as Record<string, unknown>).__MOCK_SET_NETWORK_STATUS__ as (s: boolean) => void;
    set(true);
    window.dispatchEvent(new Event('online'));
  });
  await expect.poll(() => isOnline(page)).toBe(true);
}

test.describe('Satellite tiles reload when the connection returns', () => {
  test('survey map re-requests the tiles that failed offline', async ({ page }) => {
    await setupMockedPage(page);
    const tiles = await routeTiles(page);
    await createProject(page, 'Reconnect Survey');
    await expect.poll(() => page.evaluate(() => {
      const map = (window as unknown as { __stalagmight_map__?: { getSource: (id: string) => unknown } }).__stalagmight_map__;
      return Boolean(map?.getSource('satellite-tiles'));
    })).toBe(true);
    await waitForMapIdle(page);
    await waitForTileQuiet(tiles);

    tiles.setFailing(false);
    await goOfflineThenOnline(page);
    await expect.poll(tiles.retriedFailures, { timeout: 10000 }).toBeGreaterThan(0);
  });

  test('Global View re-requests its basemap tiles that failed offline', async ({ page }) => {
    await setupMockedPage(page);
    await expect(page.getByTestId('global-view')).toBeVisible({ timeout: 10000 });
    const tiles = await routeTiles(page);
    await page.locator('[data-testid="globalview-view-toggle-map"]').click();
    await expect(page.getByTestId('globalview-map')).toBeVisible({ timeout: 5000 });
    // Let the initial view settle (fit + any persisted-camera move) first.
    await waitForTileQuiet(tiles);

    tiles.setFailing(false);
    await goOfflineThenOnline(page);
    await expect.poll(tiles.retriedFailures, { timeout: 10000 }).toBeGreaterThan(0);
  });
});
