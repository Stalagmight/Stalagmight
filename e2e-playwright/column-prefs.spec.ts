import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';

/**
 * E2E tests for data-table column preferences (hide / reorder) and their
 * persistence through the debounced `update_data_table_view` call.
 *
 * Honesty note: the mock stores the view state in memory and folds it back
 * into get_config's panels, so these tests pin (a) the exact IPC payload the
 * debounce sends and (b) the config re-sync round trip the frontend performs
 * after saving - NOT real on-disk persistence (that's the Rust
 * update_data_table_view_impl tests).
 *
 * Calls are recorded in __MOCK_DATA_TABLE_VIEW_CALLS__ (camelCase payloads).
 * Default panels config hides section_id + shot_id and shows azimuth as avg /
 * inclination off, so the visible shot columns start as: _warnings, length,
 * _azimuth_avg, depth_in, depth_out, left, right, up, down, kind,
 * comment.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Column Prefs Section',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
      { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
    ],
  },
];

async function getViewCalls(page: Page): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(() => ((window as any).__MOCK_DATA_TABLE_VIEW_CALLS__ ?? []) as Array<Record<string, unknown>>);
}

/** panels section of the in-memory config (re-synced via get_config after a save). */
async function getPanelsConfig(page: Page): Promise<Record<string, unknown>> {
  return page.evaluate(() => (window as any).__E2E_TEST__?.projectStore.getState().config?.panels ?? {});
}

/** Visible grid column keys, in DOM order. */
async function visibleHeaderKeys(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll('[data-testid^="grid-header-"]')).map((el) =>
      ((el as HTMLElement).dataset.testid ?? '').replace('grid-header-', ''),
    ),
  );
}

/**
 * Persisted width for one grid column, from the LATEST recorded view-save call.
 *
 * NB the shots table has a column whose key is literally `length`, so a lookup
 * like `widths.length` is a column key, not an array length. These helpers take
 * the key explicitly to keep that unambiguous.
 */
async function latestPersistedWidth(page: Page, colKey: string): Promise<number | undefined> {
  const calls = await getViewCalls(page);
  const widths = calls[calls.length - 1]?.columnWidths as Record<string, number> | undefined;
  return widths?.[colKey];
}

/** How many recorded view-save calls carry a width for `colKey`. A bare call count
 *  races the data table's own settling snapshot, which carries an EMPTY
 *  columnWidths and flaked this test on its first run. */
async function widthCallCount(page: Page, colKey: string): Promise<number> {
  const calls = await getViewCalls(page);
  return calls.filter(
    (c) => (c.columnWidths as Record<string, number> | undefined)?.[colKey] !== undefined,
  ).length;
}

/** Drag a column's resize edge by `dx`, leaving the button DOWN. hover() rather
 *  than a mouse.move to a captured boundingBox: the handle is only a few px wide
 *  and straddles the column boundary, so a raw move landed beside it under
 *  parallel load and the drag silently did nothing. */
async function beginColumnDrag(page: Page, colKey: string, dx: number): Promise<number> {
  const handle = page.getByTestId(`grid-col-resize-${colKey}`);
  await handle.hover();
  // Measure once the pointer has settled, so the returned width is the laid-out
  // one rather than a mid-settle value.
  const before = (await page.getByTestId(`grid-header-${colKey}`).boundingBox())!.width;
  await page.mouse.down();
  const grip = (await handle.boundingBox())!;
  await page.mouse.move(grip.x + grip.width / 2 + dx, grip.y + grip.height / 2, { steps: 8 });
  return before;
}

/**
 * Registers `window.__E2E_SEED_DATA_TABLE_VIEW__` via its own `page.addInitScript`,
 * which the mock's `mockDataTableView` reads at init (see tauri-ipc.ts). Init
 * scripts run in registration order before any app JS on every navigation, so
 * this MUST be called before `setupMockedPage` - it is the only way to have a
 * seeded value present for the app's first `get_config()` call at boot, which
 * the DataTable's one-shot seed effect reads from.
 */
async function seedColumnWidths(page: Page, seed: Record<string, unknown>): Promise<void> {
  await page.addInitScript((s) => {
    (window as unknown as Record<string, unknown>).__E2E_SEED_DATA_TABLE_VIEW__ = s;
  }, seed);
}

/** Open the DataTable via keyboard shortcut. */
async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId('grid-header-length')).toBeVisible({ timeout: 5000 });
}

/** Wait for the debounced save to fire, then return its single recorded call. */
async function waitForSingleViewCall(page: Page): Promise<Record<string, unknown>> {
  await page.waitForFunction(
    () => (((window as any).__MOCK_DATA_TABLE_VIEW_CALLS__ ?? []) as unknown[]).length >= 1,
    undefined,
    { timeout: 5000 },
  );
  const calls = await getViewCalls(page);
  expect(calls).toHaveLength(1);
  return calls[0]!;
}

test.describe('Data table column preferences', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Column Prefs Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openDataTable(page);
  });

  test('opening the data table saves nothing', async ({ page }) => {
    // Seeding state from config must not look like a change and schedule a write.
    // Deliberately waits out the 500ms debounce instead of sampling immediately: the
    // bug this guards was a snapshot mismatch that fired ~500ms after mount, so an
    // instantaneous check passes whether or not it is present. The mismatch came from
    // comparing a sorted snapshot against the config's own unsorted order, and the
    // station default `['_x','_y','_lat','_long']` is unsorted, so this fired on every
    // open until DataTable's init sorted both hidden lists.
    await page.waitForTimeout(900);
    expect(await getViewCalls(page)).toHaveLength(0);
  });

  test('eye-menu toggles coalesce into one debounced update_data_table_view call', async ({ page }) => {
    // What proves the coalescing claim is waitForSingleViewCall() below - exactly one
    // call once everything settles. The `toHaveLength(0)` a few lines down is NOT part
    // of that proof: it samples the call count mid-debounce, so under parallel load the
    // 500ms timer can fire before it runs. It predates this change and is left alone
    // here rather than widening the diff, but do not copy the pattern - assert the
    // settled end state, which proves the same property without racing.
    await page.getByTestId('data-table-columns-menu').click();
    const dropdown = page.getByTestId('data-table-columns-dropdown');
    await expect(dropdown).toBeVisible();

    // Hide Comment, show Shot ID - two quick toggles, one debounced save.
    await dropdown.getByTestId('col-vis-comment').click();
    await dropdown.getByTestId('col-vis-shot_id').click();
    await page.keyboard.press('Escape');

    // The grid follows immediately...
    await expect(page.getByTestId('grid-header-comment')).toHaveCount(0);
    await expect(page.getByTestId('grid-header-shot_id')).toBeVisible();
    // ...but the save is debounced, not per-toggle.
    expect(await getViewCalls(page)).toHaveLength(0);

    const call = await waitForSingleViewCall(page);
    expect(call.hiddenColumns).toEqual(['comment', 'explorers', 'section_id', 'survey_date', 'surveyors']);
    expect(call).toMatchObject({ azimuthMode: 'avg', inclinationMode: 'off' });

    // Round trip: the post-save loadConfig() re-reads get_config, whose panels
    // now carry the stored view state.
    await page.waitForFunction(
      () => {
        const hidden = ((window as any).__E2E_TEST__?.projectStore.getState().config?.panels
          ?.data_table_shot_hidden_columns ?? []) as string[];
        return hidden.includes('comment') && !hidden.includes('shot_id');
      },
      undefined,
      { timeout: 5000 },
    );
    expect((await getPanelsConfig(page)).data_table_shot_hidden_columns).toEqual(['comment', 'explorers', 'section_id', 'survey_date', 'surveyors']);
  });

  test('dragging a header reorders the columns and persists the new order', async ({ page }) => {
    expect(await getViewCalls(page)).toHaveLength(0);
    expect((await visibleHeaderKeys(page)).slice(0, 5)).toEqual([
      '_warnings',
      'length',
      '_azimuth_avg',
      'depth_in',
      'depth_out',
    ]);

    // Select the depth_out column, then drag its header before length (drop
    // on the target header's left half).
    const source = page.getByTestId('grid-header-depth_out');
    await source.click();
    const from = (await source.boundingBox())!;
    const to = (await page.getByTestId('grid-header-length').boundingBox())!;
    await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
    await page.mouse.down();
    await page.mouse.move(to.x + to.width * 0.25, to.y + to.height / 2, { steps: 6 });
    await page.mouse.up();

    await expect(page.getByTestId('grid-selection-overlay')).toBeVisible();
    expect((await visibleHeaderKeys(page)).slice(0, 5)).toEqual([
      '_warnings',
      'depth_out',
      'length',
      '_azimuth_avg',
      'depth_in',
    ]);

    // The persisted order is the FULL key list (hidden columns keep their slots
    // so they reappear in place when re-shown). Assert on the LATEST recorded
    // `update_data_table_view` call - its order has depth_out immediately before
    // length - never an exact call count or the first call. Under slower /
    // contended CI timing the debounced save (DataTable.tsx) was observed to
    // record more than one call and/or an earlier call carrying a non-final
    // order, so the old "exactly one call, read calls[0]" assertion flaked two
    // ways on the self-hosted nightly runner. The trailing debounce always
    // wins, so the latest recorded call is authoritative. (The post-save
    // loadConfig() re-sync is NOT the cause - its re-seed effect is ref-guarded
    // against re-firing the save.)
    await page.waitForFunction(
      () => {
        const calls = ((window as any).__MOCK_DATA_TABLE_VIEW_CALLS__ ?? []) as Array<{
          columnOrder?: string[];
        }>;
        const order = (calls[calls.length - 1]?.columnOrder ?? []) as string[];
        return order.indexOf('depth_out') === order.indexOf('length') - 1;
      },
      undefined,
      { timeout: 5000 },
    );
    const recorded = await getViewCalls(page);
    const order = recorded[recorded.length - 1]!.columnOrder as string[];
    expect(order.indexOf('depth_out')).toBe(order.indexOf('length') - 1);
    expect(order).toContain('section_id');
    expect(order).toContain('shot_id');

    // The post-save config re-sync settles on that same persisted order.
    await expect
      .poll(async () => (await getPanelsConfig(page)).data_table_shot_column_order, { timeout: 5000 })
      .toEqual(order);
  });

  test('dragging a column edge persists the new width, and only on release', async ({ page }) => {
    expect(await widthCallCount(page, 'length')).toBe(0);

    const header = page.getByTestId('grid-header-length');
    const before = await beginColumnDrag(page, 'length', 60);

    // The grid follows the pointer immediately...
    await expect.poll(async () => (await header.boundingBox())!.width).toBeGreaterThan(before + 40);
    // ...but no width has been persisted yet. Were the width lifted to the host
    // as controlled state, this would already be writing once per pixel.
    expect(await widthCallCount(page, 'length')).toBe(0);

    await page.mouse.up();

    // The debounce can emit an interim snapshot, so poll the LATEST call rather
    // than asserting a count or reading calls[0] (#171).
    await expect
      .poll(async () => latestPersistedWidth(page, 'length'), { timeout: 5000 })
      .toBeGreaterThan(before + 40);

    // Integers only - the persisted config stores u32.
    const persisted = (await latestPersistedWidth(page, 'length'))!;
    expect(Number.isInteger(persisted)).toBe(true);

    // The post-save loadConfig() re-sync settles on the same width.
    await expect
      .poll(
        async () =>
          ((await getPanelsConfig(page)).data_table_shot_column_widths as Record<string, number>)
            ?.length,
        { timeout: 5000 },
      )
      .toBe(persisted);
  });

  test('a resize abandoned mid-drag is reverted and never rides along on a later save', async ({
    page,
  }) => {
    // Regression guard. Clearing the drag flag on abandon is not enough: the
    // in-progress width is already in the live map, and every commit spreads that
    // WHOLE map - so the abandoned value used to be persisted by the next resize of
    // an unrelated column, and rendered in the meantime as a width that was never
    // saved.
    const lengthHeader = page.getByTestId('grid-header-length');
    const before = await beginColumnDrag(page, 'length', 80);
    await expect.poll(async () => (await lengthHeader.boundingBox())!.width).toBeGreaterThan(before + 40);

    // Abandon it the way the window does when focus is stolen mid-drag.
    await page.evaluate(() => window.dispatchEvent(new Event('blur')));
    await page.mouse.up(); // release for real, so the browser's button state is clean

    // The column snaps back rather than showing a width that was never stored.
    await expect.poll(async () => (await lengthHeader.boundingBox())!.width).toBe(before);
    expect(await widthCallCount(page, 'length')).toBe(0);

    // Now resize a DIFFERENT column for real. This is the step that used to drag
    // the abandoned width along with it.
    const depthBefore = await beginColumnDrag(page, 'depth_in', 50);
    await page.mouse.up();

    await expect
      .poll(async () => latestPersistedWidth(page, 'depth_in'), { timeout: 5000 })
      .toBeGreaterThan(depthBefore + 20);
    expect(await latestPersistedWidth(page, 'length')).toBeUndefined();
  });

  test('a resize whose mouseup landed outside the window is kept, not reverted', async ({
    page,
  }) => {
    // The abandon guard above and this one share a single code path: cancelDragRef used
    // to serve both `blur` (a genuine abandon) and the buttonless-mousemove recovery
    // for a mouseup released outside the window. Making that path revert fixed the
    // first case and broke the second - the user DID finish this resize, so throwing
    // it away is data loss, not safety.
    const lengthHeader = page.getByTestId('grid-header-length');
    const before = await beginColumnDrag(page, 'length', 70);
    await expect
      .poll(async () => (await lengthHeader.boundingBox())!.width)
      .toBeGreaterThan(before + 30);

    // No mouseup reaches the window; the next pointer move arrives with no buttons
    // held, which is exactly how the grid learns the release already happened.
    await page.evaluate(() =>
      window.dispatchEvent(new MouseEvent('mousemove', { buttons: 0, bubbles: true }))
    );
    await page.mouse.up(); // keep the browser's own button state clean for later tests

    await expect
      .poll(async () => latestPersistedWidth(page, 'length'), { timeout: 5000 })
      .toBeGreaterThan(before + 30);
  });

  // A bare click on a resize handle must not commit. That is NOT asserted here: the
  // commit is deduped downstream by DataTable's snapshot compare, so no IPC fires
  // either way and this layer cannot tell the two apart - an e2e test here passes with
  // the guard deleted. It is pinned where the callback is observable, in
  // src/components/common/SpreadsheetGrid/__tests__/useColumnWidths.test.tsx.

  test('header right-click hide entry hides the column and persists', async ({ page }) => {
    expect(await getViewCalls(page)).toHaveLength(0);

    const menu = page.getByTestId('data-table-header-menu');
    // The right-click → `contextmenu` → menu-open gesture intermittently does not
    // register under CPU load: the menu never mounts (confirmed by repro - the
    // element count is 0 at failure, i.e. NOT a slow paint, so a longer
    // `toBeVisible` timeout cannot help). Retry the whole GESTURE, re-resolving the
    // header locator each attempt, until the menu actually opens. This is a
    // deterministic fix for a flaky input, not a masked timeout. (#171)
    await expect(async () => {
      await page.getByTestId('grid-header-comment').click({ button: 'right' });
      await expect(menu).toBeVisible({ timeout: 1000 });
    }).toPass({ timeout: 10_000 });
    await menu.getByRole('button', { name: 'Hide Comment' }).click();

    await expect(page.getByTestId('grid-header-comment')).toHaveCount(0);

    // The debounced view-save can emit an interim snapshot (e.g. the pre-hide
    // [section_id, shot_id, credit columns] state from the table settling) before the Hide save
    // lands, so grabbing the *first* recorded call races. Poll the latest call
    // until it settles on the expected hidden set. #171.
    await expect
      .poll(
        async () => {
          const calls = await getViewCalls(page);
          return calls[calls.length - 1]?.hiddenColumns;
        },
        { timeout: 5000 },
      )
      .toEqual(['comment', 'explorers', 'section_id', 'shot_id', 'survey_date', 'surveyors']);

    await page.waitForFunction(
      () => {
        const hidden = ((window as any).__E2E_TEST__?.projectStore.getState().config?.panels
          ?.data_table_shot_hidden_columns ?? []) as string[];
        return hidden.includes('comment');
      },
      undefined,
      { timeout: 5000 },
    );
    expect((await getPanelsConfig(page)).data_table_shot_hidden_columns).toEqual([
      'comment',
      'explorers',
      'section_id',
      'shot_id',
      'survey_date',
      'surveyors',
    ]);
  });
});

test.describe('Data table column preferences - restore direction (#357 item 16)', () => {
  /**
   * Every width test above (242, 277) proves the WRITE half of persistence: a
   * resize fires `update_data_table_view`. None of them ever mounts against a
   * config that already carries a width, so the READ half - config ->
   * DataTable's one-shot seed effect (DataTable.tsx ~687-734) -> columnWidths
   * prop -> computeColumnWidths -> rendered header width - had no coverage at
   * any tier, though it is exactly the half the CHANGELOG's "survive a
   * restart" claim rests on.
   *
   * The seed must be in place before the app's FIRST get_config() call (the
   * boot-time loadConfig() the seed effect only ever runs against once), so
   * each test here registers its own `page.addInitScript` via
   * `seedColumnWidths` BEFORE `setupMockedPage`, rather than sharing this
   * file's `beforeEach`.
   */

  test('a shot-grid width restored from config renders on first paint, not the declared default', async ({
    page,
  }) => {
    // 187px: `length`'s declared default is 100 (dataTableShotColumns.tsx), and
    // `length` is not a `grow` column (only `comment` is), so an override is
    // always its final rendered width - never touched by the leftover/grow
    // distribution. A rendered width this far from 100 can only have come from
    // the seeded config.
    await seedColumnWidths(page, { data_table_shot_column_widths: { length: 187 } });
    await setupMockedPage(page);
    await createProject(page, 'Restore Shot Width Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openDataTable(page);

    await expect
      .poll(async () => (await page.getByTestId('grid-header-length').boundingBox())!.width)
      .toBeGreaterThan(150);
  });

  test('a station-grid width restored from config renders on first paint too', async ({ page }) => {
    // 205px: `_depth`'s declared default is 90 (dataTableStationColumns.tsx),
    // also not a `grow` column - same reasoning as the shot-grid case above.
    await seedColumnWidths(page, { data_table_station_column_widths: { _depth: 205 } });
    await setupMockedPage(page);
    await createProject(page, 'Restore Station Width Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openDataTable(page);
    await page.getByTestId('data-table-view-stations').click();
    await expect(page.getByTestId('grid-header-_depth')).toBeVisible({ timeout: 5000 });

    await expect
      .poll(async () => (await page.getByTestId('grid-header-_depth').boundingBox())!.width)
      .toBeGreaterThan(150);
  });
});
