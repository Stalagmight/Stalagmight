/**
 * Regression test for #162 (Regression 2): data-table sub-header dropdowns must
 * stay fully visible and clickable when they flip UPWARD.
 *
 * The line picker and the column eye-menu open above their trigger when the
 * data table is docked low. Before the fix they used `position:absolute` and
 * were clipped by the Dock's `overflow-hidden` box, so clicks on their items
 * fell through to the MapLibre canvas behind the clip and did nothing. The fix
 * gives them `position:fixed` (via useAnchoredFixedMenu) so they escape the clip.
 *
 * To exercise the flip-up path deterministically (independent of the runner's
 * window size - the reason PR CI's default ~1280x720 missed this while the
 * 730x434 nightly caught it), we shrink the data-table panel to its minimum
 * height. That puts the sub-header near the bottom of the window (so both menus
 * flip up) and makes the 100px panel's clip box tightly clip any upward
 * `absolute` menu. We assert the click EFFECT (not mere visibility - a clipped
 * menu is still "visible" in the DOM), the menu's viewport bounds, and that
 * elementFromPoint over an item hits the menu rather than the map canvas.
 */

import { test, expect, type Page, type Locator } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
  type LineSeed,
} from './helpers/app';

const lineSeeds: LineSeed[] = [
  {
    name: 'Alpha Passage',
    direction: 'in',
    shots: [{ azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 }],
  },
  {
    name: 'Beta Tunnel',
    direction: 'out',
    shots: [{ azimuth_in: 180, azimuth_out: 0, length: 7, depth_in: -2, depth_out: -4 }],
  },
];

async function setDataTableHeight(page: Page, height: number): Promise<void> {
  await page.evaluate((h) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { setDataTableHeight: (h: number) => void } } };
    }).__E2E_TEST__;
    stores?.uiStore.getState().setDataTableHeight(h);
  }, height);
}

// Mirrors DATA_TABLE_MIN_HEIGHT in src/components/SingleSystemView/layoutConstants.ts
// (the Dock's enforced minimum). Kept as a local literal because Playwright specs
// don't import app source; the `expect.poll` on the pill's position below fails
// loudly if this ever drifts out of sync with the real minimum.
const DATA_TABLE_MIN_HEIGHT = 100;

/** Shrink the data-table panel to its minimum so the sub-header sits low and its
 *  dropdowns flip up into (and out of) the panel's overflow-hidden clip box. */
async function shrinkDataTable(page: Page): Promise<void> {
  await setDataTableHeight(page, DATA_TABLE_MIN_HEIGHT);
}

async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
}

/** Assert `menu` flips up (sits above its trigger), lies fully within the
 *  viewport, and stays clear of the opaque top toolbar. */
async function expectFlippedUpAndOnScreen(page: Page, menu: Locator, trigger: Locator): Promise<void> {
  const { width, height } = page.viewportSize()!;
  const m = (await menu.boundingBox())!;
  const t = (await trigger.boundingBox())!;
  const toolbar = (await page.getByTestId('toolbar').boundingBox())!;
  // Flipped up: the menu's bottom is at/above the trigger's top.
  expect(m.y + m.height).toBeLessThanOrEqual(t.y + 1);
  // Fully on screen (the bug pushed the top edge above y=0, into the clip).
  expect(m.x).toBeGreaterThanOrEqual(0);
  expect(m.y).toBeGreaterThanOrEqual(0);
  expect(m.x + m.width).toBeLessThanOrEqual(width + 1);
  expect(m.y + m.height).toBeLessThanOrEqual(height + 1);
  // Below the toolbar, which would otherwise clip/cover the top of the menu.
  expect(m.y).toBeGreaterThanOrEqual(toolbar.y + toolbar.height - 1);
}

/** The bug's mechanism: a clipped item returns the map canvas from elementFromPoint. */
async function expectItemHitsMenu(page: Page, item: Locator, menuTestId: string): Promise<void> {
  const b = (await item.boundingBox())!;
  const hitsMenu = await page.evaluate(
    ({ x, y, id }) => {
      const el = document.elementFromPoint(x, y);
      return el !== null && el.closest(`[data-testid="${id}"]`) !== null;
    },
    { x: b.x + b.width / 2, y: b.y + b.height / 2, id: menuTestId },
  );
  expect(hitsMenu).toBe(true);
}

test.describe('#162 header dropdowns are usable when flipped up', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Clip Regression');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await shrinkDataTable(page);
    await openDataTable(page);
    // Wait for the panel to settle low (its sub-header in the bottom of the
    // window) before measuring/opening menus, so the flip-up is deterministic.
    const pill = page.getByTestId('data-table-line-pill');
    await expect.poll(async () => (await pill.boundingBox())?.y ?? 0).toBeGreaterThan(
      page.viewportSize()!.height / 2,
    );
  });

  test('line picker flips up, stays on screen, and a pick selects app-wide', async ({ page }) => {
    const pill = page.getByTestId('data-table-line-pill');
    await expect(pill).toContainText('Alpha Passage');

    await pill.click();
    const list = page.getByTestId('data-table-line-menu-list');
    await expect(list).toBeVisible();

    await expectFlippedUpAndOnScreen(page, list, pill);

    const betaItem = list.getByRole('button', { name: 'Beta Tunnel' });
    await expectItemHitsMenu(page, betaItem, 'data-table-line-menu-list');

    // The real repro: the click must land on the item, not the map behind it.
    await betaItem.click();
    await expect(list).not.toBeVisible();
    await expect(pill).toContainText('Beta Tunnel');
    await expect
      .poll(async () =>
        page.evaluate(() => {
          const stores = (window as unknown as {
            __E2E_TEST__?: { uiStore: { getState: () => { selection: { type: string } | null } } };
          }).__E2E_TEST__;
          return stores?.uiStore.getState().selection?.type ?? null;
        }),
      )
      .toBe('line');
  });

  test('a tall picker is bounded below the toolbar (not hidden behind it)', async ({ page }) => {
    // Many lines make the list taller than the space between the sub-header
    // and the toolbar; in a short window that forces the flipped-up list to be
    // capped at the toolbar's bottom instead of sliding up under the opaque bar.
    const many = Array.from({ length: 20 }, (_, i) => ({
      name: `Line ${String(i).padStart(2, '0')}`,
      direction: 'in' as const,
      shots: [{ azimuth_in: 45, azimuth_out: 225, length: 5, depth_in: -1, depth_out: -2 }],
    }));
    await seedProjectWithLines(page, many);
    await waitForNoDialogs(page);
    await page.setViewportSize({ width: 900, height: 460 });
    await shrinkDataTable(page);
    const pill = page.getByTestId('data-table-line-pill');
    await expect.poll(async () => (await pill.boundingBox())?.y ?? 0).toBeGreaterThan(230);

    await pill.click();
    const list = page.getByTestId('data-table-line-menu-list');
    await expect(list).toBeVisible();

    const m = (await list.boundingBox())!;
    const toolbar = (await page.getByTestId('toolbar').boundingBox())!;
    // Flipped up and capped below the toolbar, with an 8px breathing gap (not
    // flush against the bar).
    expect(m.y + m.height).toBeLessThanOrEqual((await pill.boundingBox())!.y + 1);
    expect(m.y).toBeGreaterThanOrEqual(toolbar.y + toolbar.height + 8 - 1);
    // The topmost item must be clickable (under the toolbar it would not be).
    await expectItemHitsMenu(page, list.getByRole('button').first(), 'data-table-line-menu-list');
  });

  test('column eye-menu flips up, stays on screen, and a toggle changes the grid', async ({ page }) => {
    const trigger = page.getByTestId('data-table-columns-menu');
    await trigger.click();
    const dropdown = page.getByTestId('data-table-columns-dropdown');
    await expect(dropdown).toBeVisible();

    await expectFlippedUpAndOnScreen(page, dropdown, trigger);

    const commentRow = dropdown.getByTestId('col-vis-comment');
    await expectItemHitsMenu(page, commentRow, 'data-table-columns-dropdown');

    // The click must toggle the column, not fall through to the map.
    await expect(page.getByTestId('grid-header-comment')).toBeVisible();
    await commentRow.click();
    await expect(page.getByTestId('grid-header-comment')).toHaveCount(0);
  });

  test('resizing the window while a picker is open dismisses it (no stale fixed position)', async ({
    page,
  }) => {
    const pill = page.getByTestId('data-table-line-pill');
    await pill.click();
    const list = page.getByTestId('data-table-line-menu-list');
    await expect(list).toBeVisible();
    // The list is position:fixed at a rect captured ON OPEN; useAnchoredFixedMenu
    // deliberately does NOT reposition on resize/scroll - it delegates stale-position
    // handling to useDismissableLayer, which must dismiss the menu on resize (else it
    // would float at a now-wrong spot). Pin that contract so a future change that
    // drops the resize listener fails here.
    await page.setViewportSize({ width: 1100, height: 700 });
    await expect(list).toBeHidden();
  });

  // The THIRD migrated dropdown - the raw-table picker (`data-table-raw-table-pill`)
  // - is intentionally not covered here. It is the very same `SubHeaderPickerMenu`
  // (and thus the same `useAnchoredFixedMenu` positioning) as the line picker
  // tested above; only its options/trigger content differ. Its real options are
  // also fetched only in fullscreen mode (DataTable's `get_raw_table_names` effect
  // is gated on `dataTableFullScreen`), so in the windowed docked-low layout this
  // suite exercises it would show only a disabled "loading" row - no click-through
  // to assert. The shared positioning path is fully covered by the line-picker
  // flip-up / flip-down / toolbar-clamp tests in this file.
});

test.describe('#162 a downward-flipping picker is bounded above the status bar', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Clip Bottom Regression');
    // Many lines → a tall picker that would reach the bottom status bar.
    const many = Array.from({ length: 40 }, (_, i) => ({
      name: `Line ${String(i).padStart(2, '0')}`,
      direction: 'in' as const,
      shots: [{ azimuth_in: 45, azimuth_out: 225, length: 5, depth_in: -1, depth_out: -2 }],
    }));
    await seedProjectWithLines(page, many);
    await waitForNoDialogs(page);
    await waitForMapReady(page);
    await page.setViewportSize({ width: 900, height: 560 });
    // A tall data-table panel pushes the sub-header HIGH, so the picker flips
    // DOWN (toward the status bar) rather than up. 440 is past what the layout
    // will grant, deliberately: it asks for the tallest panel available.
    await setDataTableHeight(page, 440);
    await openDataTable(page);
    const pill = page.getByTestId('data-table-line-pill');
    // #260 clamps the panel so the map row keeps MAP_MIN_HEIGHT, capping it at
    // `main - 200` however much was asked for. That pins the sub-header near
    // `toolbar + 200` no matter how short the viewport is, so the original fixed
    // `< 160` became unreachable rather than merely tight. What the flip-down case
    // needs is only that the trigger sits in the upper half, leaving more room
    // below it than above - so assert that, derived from the viewport instead of
    // hard-coded against a layout constant this spec does not own.
    const viewport = page.viewportSize();
    expect(viewport).not.toBeNull();
    await expect
      .poll(async () => (await pill.boundingBox())?.y ?? 999)
      .toBeLessThan(viewport!.height / 2);
  });

  test('flips down and stops above the status bar (not clipped behind it)', async ({ page }) => {
    const pill = page.getByTestId('data-table-line-pill');
    await pill.click();
    const list = page.getByTestId('data-table-line-menu-list');
    await expect(list).toBeVisible();

    const m = (await list.boundingBox())!;
    const t = (await pill.boundingBox())!;
    const statusBar = (await page.getByTestId('status-bar').boundingBox())!;
    // Flipped down: the menu's top is at/below the trigger's bottom.
    expect(m.y).toBeGreaterThanOrEqual(t.y + t.height - 1);
    // Bottom edge stops above the status bar, with an 8px breathing gap (before
    // the fix it ran to the viewport bottom, clipped behind the status bar). This
    // already implies the menu clears the status bar entirely.
    expect(m.y + m.height).toBeLessThanOrEqual(statusBar.y - 8 + 1);
  });
});
