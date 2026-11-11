import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { systemCardTestId } from './helpers/testIds';

/**
 * E2E tests for the Recent Projects enhancements (#46, #47, #48):
 *   - file path shown under each card (full path on hover)
 *   - relative "Edited" time with the exact timestamp on hover
 *   - persisted "Sort by" control (field + direction)
 *
 * Mock recent projects (from mocks/tauri-ipc.ts), newest → oldest:
 *   Cave System Alpha   /projects/cave-alpha.stalx   5 sections  now
 *   Underwater Cave Beta /projects/cave-beta.stalx   3 sections  -1 day
 *   Deep Cave Gamma     /projects/cave-gamma.stalx   8 sections  -2 days
 *   Local Survey…       /projects/local-survey.stalx 2 sections  -3 days
 */

const cardTestId = systemCardTestId;
const ALPHA = cardTestId('/projects/cave-alpha.stalx');
const BETA = cardTestId('/projects/cave-beta.stalx');
const GAMMA = cardTestId('/projects/cave-gamma.stalx');
const LOCAL = cardTestId('/projects/local-survey.stalx');

async function switchToListTab(page: Page): Promise<void> {
  const listTab = page.getByTestId('globalview-view-toggle-list');
  await expect(listTab).toBeVisible();
  await listTab.click();
  await expect(page.getByTestId(ALPHA)).toBeVisible();
}

/** Document order of the rendered system cards (by their data-testid). */
async function cardOrder(page: Page): Promise<string[]> {
  return page
    .locator('[role="button"][data-testid^="system-card-%2F"]')
    .evaluateAll((els) => els.map((e) => e.getAttribute('data-testid') ?? ''));
}

/** Pick a sort field from the custom Select. */
async function selectSortField(page: Page, label: string): Promise<void> {
  await page.getByTestId('recent-sort-field').click();
  await page.getByRole('option', { name: label, exact: true }).click();
}

test.describe('GlobalView Recent Projects - path, relative time, sort', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await switchToListTab(page);
  });

  test('renders globally unique test IDs across repeated project cards', async ({ page }) => {
    const duplicates = await page.locator('[data-testid]').evaluateAll((elements) => {
      const counts = new Map<string, number>();
      for (const element of elements) {
        const id = element.getAttribute('data-testid');
        if (id) counts.set(id, (counts.get(id) ?? 0) + 1);
      }
      return [...counts.entries()].filter(([, count]) => count > 1);
    });
    expect(duplicates).toEqual([]);
  });

  test('shows the elided file path under each card, revealed in full on hover (no tooltip)', async ({ page }) => {
    const path = page.getByTestId(`${ALPHA}-path`);
    await expect(path).toBeVisible();
    await expect(path).toHaveAttribute('data-full-path', '/projects/cave-alpha.stalx');

    await path.hover();
    // No tooltip anymore - the full path is revealed in place (scrolls if it overflows).
    await expect(page.locator('[role="tooltip"]', { hasText: '/projects/cave-alpha.stalx' })).toHaveCount(0);
    await expect(path).toContainText('/projects/cave-alpha.stalx');
  });

  test('an overflowing path scrolls (marquee) in place on hover, not a tooltip', async ({ page }) => {
    // Force the path clip box narrow so the full path overflows regardless of length.
    await page.addStyleTag({ content: '[data-testid^="system-card-%2F"][data-testid$="-path"]{max-width:60px !important}' });
    const path = page.getByTestId(`${ALPHA}-path`);
    await path.hover();
    const overlay = path.locator('[data-elided-overlay]');
    await expect(overlay).toHaveCount(1); // marquee overlay appears
    // Out of flow, so it can't widen the table column (the reported bug).
    expect(await overlay.evaluate((el) => getComputedStyle(el).position)).toBe('absolute');
    expect(await overlay.evaluate((el) => getComputedStyle(el).animationName)).toContain('elided-path-scroll');
    await expect(page.locator('[role="tooltip"]')).toHaveCount(0);
  });

  test('table view: sticky header pins flush to the top when scrolling', async ({ page }) => {
    await page.setViewportSize({ width: 1000, height: 280 }); // short → the list scrolls
    await page.getByTestId('recent-density-table').click();
    await page.getByTestId('globalview-list-table').waitFor({ state: 'visible' });
    const gap = await page.evaluate(() => {
      const t = document.querySelector('[data-testid="globalview-list-table"]') as HTMLElement;
      let sc: HTMLElement | null = t.parentElement;
      while (sc) {
        const o = getComputedStyle(sc).overflowY;
        if (o === 'auto' || o === 'scroll') break;
        sc = sc.parentElement;
      }
      if (!sc) return -999;
      sc.scrollTop = 150;
      const th = t.querySelector('thead th') as HTMLElement;
      return Math.round(th.getBoundingClientRect().top - sc.getBoundingClientRect().top);
    });
    expect(Math.abs(gap)).toBeLessThanOrEqual(1); // no gap above the pinned header
  });

  test('shows relative edited time, with the exact timestamp on hover', async ({ page }) => {
    const betaModified = page.getByTestId(`${BETA}-modified`);
    await expect(betaModified).toBeVisible();
    // -1 day renders as "yesterday" in en locale (Intl.RelativeTimeFormat).
    await expect(betaModified).toContainText(/yesterday/i);

    await betaModified.hover();
    // Absolute tooltip includes a 4-digit year and a HH:MM time.
    const tooltip = page.locator('[role="tooltip"]').last();
    await expect(tooltip).toBeVisible();
    await expect(tooltip).toContainText(/\d{4}/);
    await expect(tooltip).toContainText(/\d{1,2}:\d{2}/);
  });

  test('sorts the list and persists field + direction across reloads', async ({ page }) => {
    // Default: last edited, newest first.
    expect(await cardOrder(page)).toEqual([ALPHA, BETA, GAMMA, LOCAL]);

    // Switching to Name starts alphabetically.
    await selectSortField(page, 'Name');
    expect(await cardOrder(page)).toEqual([ALPHA, GAMMA, LOCAL, BETA]);

    // Flip to descending; the preference survives reload.
    await page.getByTestId('recent-sort-direction').click();
    expect(await cardOrder(page)).toEqual([BETA, LOCAL, GAMMA, ALPHA]);

    // Preference survives a reload (persisted in localStorage).
    await page.reload();
    await switchToListTab(page);
    expect(await cardOrder(page)).toEqual([BETA, LOCAL, GAMMA, ALPHA]);
  });

  test('sorts by section count', async ({ page }) => {
    await selectSortField(page, 'Section count');
    // Descending by section_count: Gamma(8), Alpha(5), Beta(3), Local(2).
    expect(await cardOrder(page)).toEqual([GAMMA, ALPHA, BETA, LOCAL]);
  });

  test('sorts by last opened (backend recency order), reversible', async ({ page }) => {
    // Switch away first so the change back is observable.
    await selectSortField(page, 'Name');
    await selectSortField(page, 'Last opened');
    // Descending = most-recently-opened first = the order the backend returns.
    expect(await cardOrder(page)).toEqual([ALPHA, BETA, GAMMA, LOCAL]);

    // Ascending = oldest-opened first.
    await page.getByTestId('recent-sort-direction').click();
    expect(await cardOrder(page)).toEqual([LOCAL, GAMMA, BETA, ALPHA]);
  });
});

test('remote projects follow date and name sorting in cards and table', async ({ page }) => {
  await setupMockedPage(page);
  await switchToListTab(page);
  await page.evaluate(() => {
    const store = (window as unknown as { __E2E_TEST__: {
      speleoStore: { setState: (state: unknown) => void };
    } }).__E2E_TEST__.speleoStore;
    store.setState({ remoteSummaries: Object.fromEntries([
      ['older', 'A Remote', '2020-01-01T00:00:00Z'],
      ['newer', 'Z Remote', '2030-01-01T00:00:00Z'],
    ].map(([id, name, modified_date]) => [id, { id, name, modified_date,
      permission: 'READ_AND_WRITE', lock_holder: null, type_tag: 'stalagmight', latest_commit_sha: null }])) });
  });
  const remoteOrder = (prefix: string) => page.locator(`[data-testid^="${prefix}"][data-remote="true"]`)
    .evaluateAll((els) => els.map((el) => el.getAttribute('data-testid')));
  await selectSortField(page, 'Last edited');
  await expect.poll(() => remoteOrder('system-card-remote-')).toEqual(['system-card-remote-newer', 'system-card-remote-older']);
  await page.getByTestId('recent-sort-direction').click();
  await expect.poll(() => remoteOrder('system-card-remote-')).toEqual(['system-card-remote-older', 'system-card-remote-newer']);
  await page.getByTestId('recent-density-table').click();
  await page.getByTestId('recent-col-edited').click();
  await expect.poll(() => remoteOrder('system-row-remote-')).toEqual(['system-row-remote-newer', 'system-row-remote-older']);
  await expect(page.getByTestId('system-row-remote-newer').locator('time')).toHaveAttribute('datetime', '2030-01-01T00:00:00Z');
  await page.getByTestId('recent-col-name').click();
  await expect.poll(() => remoteOrder('system-row-remote-')).toEqual(['system-row-remote-older', 'system-row-remote-newer']);
  await page.getByTestId('recent-col-name').click();
  await expect.poll(() => remoteOrder('system-row-remote-')).toEqual(['system-row-remote-newer', 'system-row-remote-older']);
});
