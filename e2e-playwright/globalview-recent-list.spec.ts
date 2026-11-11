import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';
import {
  missingSystemCardRemoveTestId,
  missingSystemCardTestId,
  missingSystemRowTestId,
  systemCardTestId,
  systemRowTestId,
} from './helpers/testIds';

/**
 * E2E for the Recent Projects list-view improvements:
 *   - search filters the card/table list, with count feedback + no-results state
 *   - compact table view toggle (persisted)
 *   - missing-file entries (recent paths the backend can't load)
 *
 * Mock recent projects (mocks/tauri-ipc.ts): Alpha, Beta, Gamma, Local.
 */

const ALPHA = '/projects/cave-alpha.stalx';
const BETA = '/projects/cave-beta.stalx';
const GAMMA = '/projects/cave-gamma.stalx';
const LOCAL = '/projects/local-survey.stalx';
const GHOST = '/projects/ghost.stalx';

async function switchToListTab(page: Page): Promise<void> {
  const listTab = page.getByTestId('globalview-view-toggle-list');
  await expect(listTab).toBeVisible();
  await listTab.click();
  await expect(page.getByTestId(systemCardTestId(ALPHA))).toBeVisible();
}

/** Inject a config whose recent list includes a path with no loadable metadata. */
async function injectMissingRecent(page: Page): Promise<void> {
  await page.waitForFunction(() => Boolean(
    (window as unknown as { __E2E_TEST__?: { projectStore?: { getState: () => { config: unknown } } } })
      .__E2E_TEST__?.projectStore?.getState().config
  ));
  await page.evaluate((paths) => {
    const store = (window as unknown as {
      __E2E_TEST__: { projectStore: { getState: () => { config: { general: Record<string, unknown> }; setConfig: (c: unknown) => void } } };
    }).__E2E_TEST__.projectStore;
    const state = store.getState();
    state.setConfig({ ...state.config, general: { ...state.config.general, recent_projects: paths } });
  }, [ALPHA, BETA, GAMMA, LOCAL, GHOST]);
}

test.describe('GlobalView Recent Projects - search, table view, missing files', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await switchToListTab(page);
  });

  test('the hint describes double-click opening, and only that, for local projects', async ({ page }) => {
    const status = page.getByTestId('globalview-list-status');
    await expect(status).toContainText('Click to select, double-click to open, right-click for options.');
    await expect(status).not.toContainText('SpeleoDB');
  });

  test('search box is shown on the list tab and filters the cards', async ({ page }) => {
    const search = page.getByTestId('globalview-search-input');
    await expect(search).toBeVisible();

    await search.fill('alpha');
    await expect(page.getByTestId(systemCardTestId(ALPHA))).toBeVisible();
    await expect(page.getByTestId(systemCardTestId(BETA))).toHaveCount(0);
    await expect(page.getByTestId('globalview-list-status')).toContainText('Showing 1 of 4');
  });

  test('shows a no-results state and can clear the search', async ({ page }) => {
    await page.getByTestId('globalview-search-input').fill('zzz-nope');
    const empty = page.getByTestId('globalview-list-empty');
    await expect(empty).toBeVisible();
    await expect(empty).toContainText('zzz-nope');

    await page.getByTestId('globalview-list-clear-search').click();
    await expect(page.getByTestId(systemCardTestId(ALPHA))).toBeVisible();
    await expect(page.getByTestId('globalview-list-empty')).toHaveCount(0);
  });

  test('table view toggle renders rows and persists across reload', async ({ page }) => {
    await page.getByTestId('recent-density-table').click();
    await expect(page.getByTestId('globalview-list-table')).toBeVisible();
    await expect(page.getByTestId(systemRowTestId(ALPHA))).toBeVisible();
    // Cards are gone in table mode.
    await expect(page.getByTestId(systemCardTestId(ALPHA))).toHaveCount(0);

    await page.reload();
    await expect(page.getByTestId('globalview-view-toggle-list')).toBeVisible();
    await page.getByTestId('globalview-view-toggle-list').click();
    await expect(page.getByTestId('globalview-list-table')).toBeVisible();
  });

  test('clicking column headers sorts the table and shows the indicator', async ({ page }) => {
    const rowOrder = () =>
      page.locator('tr[data-testid^="system-row-%2F"]').evaluateAll((els) =>
        els.map((e) => e.getAttribute('data-testid'))
      );
    const row = systemRowTestId;

    await page.getByTestId('recent-density-table').click();
    await expect(page.getByTestId('globalview-list-table')).toBeVisible();

    // Click Name → ascending alphabetical, with an ascending indicator.
    await page.getByTestId('recent-col-name').click();
    expect(await rowOrder()).toEqual([row(ALPHA), row(GAMMA), row(LOCAL), row(BETA)]);
    await expect(page.locator('th[aria-sort="ascending"]')).toContainText('Name');

    // Click Name again → toggle to descending.
    await page.getByTestId('recent-col-name').click();
    expect(await rowOrder()).toEqual([row(BETA), row(LOCAL), row(GAMMA), row(ALPHA)]);
    await expect(page.locator('th[aria-sort="descending"]')).toContainText('Name');

    // Switch to Sections → defaults to descending (largest first).
    await page.getByTestId('recent-col-sections').click();
    expect(await rowOrder()).toEqual([row(GAMMA), row(ALPHA), row(BETA), row(LOCAL)]);
    await expect(page.locator('th[aria-sort="descending"]')).toContainText('Sections');
  });

  test('header sorting stays in sync with the sort dropdown', async ({ page }) => {
    await page.getByTestId('recent-density-table').click();
    await page.getByTestId('recent-col-name').click();
    // The dropdown control reflects the header-driven field.
    await expect(page.getByTestId('recent-sort-field')).toContainText('Name');
  });

  test('double-clicking a table row opens the project', async ({ page }) => {
    await page.getByTestId('recent-density-table').click();
    await page.getByTestId(systemRowTestId(GAMMA)).dblclick();
    // Asserted alone, not `.or(global-view)`: global-view is already visible
    // before the dblclick, so an `.or()` here would pass even with the open
    // routing deleted.
    await expect(page.getByTestId('single-system-view')).toBeVisible();
  });

  // #450: a recents card must route its open through the same open/import
  // router as every other .stalx entry point (Ctrl+O, drag-drop, OS file
  // association), not call loadProject directly. These pin the observable
  // behavior: a successful open, and an identical failure surface (the
  // loading-error overlay from error-handling.spec.ts's "File Open Errors"
  // suite) rather than any bespoke recents-only error path.
  test.describe('recents card open routing (#450)', () => {
    test('double-clicking a card opens the project', async ({ page }) => {
      // switchToListTab's beforeEach already lands us on the grid (card) view.
      // Asserted alone (see the table-row test above for why `.or(global-view)`
      // would be unfailable here).
      await page.getByTestId(systemCardTestId(GAMMA)).dblclick();
      await expect(page.getByTestId('single-system-view')).toBeVisible();
    });

    test('a missing/moved file surfaces the loading-error overlay, like any other open entry point', async ({ page }) => {
      const message = 'File not found: the specified file does not exist';
      await setIpcOverrides(page, {
        load_project_full: { __throw: message },
        load_project: { __throw: message },
      });

      const loadingError = page.getByTestId('loading-progress');
      await page.getByTestId(systemCardTestId(GAMMA)).dblclick();

      await expect(loadingError).toBeVisible({ timeout: 10000 });
      await expect(loadingError).toContainText(message);

      await loadingError.getByRole('button', { name: 'Dismiss' }).click();
      await expect(loadingError).toBeHidden({ timeout: 10000 });
      // GlobalView stays functional on the start page - the failed open didn't
      // corrupt the recents list.
      await expect(page.getByTestId(systemCardTestId(ALPHA))).toBeVisible();
    });
  });

  test('surfaces a missing recent file with a remove action', async ({ page }) => {
    await injectMissingRecent(page);

    const missing = page.getByTestId(missingSystemCardTestId(GHOST));
    await expect(missing).toBeVisible();
    await expect(missing).toContainText('File not found');
    await expect(missing).toContainText('ghost.stalx');

    const remove = page.getByTestId(missingSystemCardRemoveTestId(GHOST));
    await expect(remove).toBeVisible();
    await remove.click();
    // Existing (loadable) projects are unaffected.
    await expect(page.getByTestId(systemCardTestId(ALPHA))).toBeVisible();
  });

  test('missing files appear in the table view too', async ({ page }) => {
    await injectMissingRecent(page);
    await page.getByTestId('recent-density-table').click();
    await expect(page.getByTestId(missingSystemRowTestId(GHOST))).toBeVisible();
    await expect(page.getByTestId(missingSystemRowTestId(GHOST))).toContainText('File not found');
  });

  test('surfaces missing entries (not the empty state) when every recent file is missing', async ({ page }) => {
    // Reproduce the all-missing case: config still references a path, but the
    // loaded list is empty. Without gating on the combined count the UI would
    // fall back to the EmptyState and the user could never clear the entry.
    await page.waitForFunction(() => Boolean(
      (window as unknown as { __E2E_TEST__?: { projectStore?: { getState: () => { config: unknown } } } })
        .__E2E_TEST__?.projectStore?.getState().config
    ));
    await page.evaluate((ghost) => {
      const store = (window as unknown as {
        __E2E_TEST__: { projectStore: {
          getState: () => { config: { general: Record<string, unknown> }; setConfig: (c: unknown) => void };
          setState: (partial: { recentProjects: unknown[] }) => void;
        } };
      }).__E2E_TEST__.projectStore;
      const state = store.getState();
      state.setConfig({ ...state.config, general: { ...state.config.general, recent_projects: [ghost] } });
      store.setState({ recentProjects: [] });
    }, GHOST);

    // The missing entry is reachable and the EmptyState is not shown.
    await expect(page.getByTestId(missingSystemCardTestId(GHOST))).toBeVisible();
    await expect(page.getByTestId('empty-state-new-project')).toHaveCount(0);
    await expect(page.getByTestId(missingSystemCardRemoveTestId(GHOST))).toBeVisible();
  });
});
