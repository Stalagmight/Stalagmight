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
 * Data workspace find / replace, filters, chips, sort bar and columns menu
 * (the toolbar above the Data workspace's grid).
 *
 * - Data mode is entered with the mode switcher's `mode-data` button.
 * - The grid inside `data-workspace` uses the `data-grid` test-id prefix
 *   (`data-grid-header-<colKey>`, DATA_GRID_PREFIX).
 * - Cell highlights render as `<mark>` inside the cell (MATCH_MARK).
 * The toolbar's own test ids are stable: `data-find-*`, `data-replace-*`,
 * `data-filter-*`, `data-sections-*`, `data-columns-*`, `data-chip-*`,
 * `data-sort-*`.
 */

const GRID = 'data-grid';
const MATCH_MARK = '[data-testid="data-workspace"] mark';

const shot = (length: number, comment: string | null = null) => ({
  azimuth_in: 90,
  azimuth_out: 90,
  length,
  depth_in: 1,
  depth_out: 2,
  comment,
});

const many = (n: number, at: Record<number, string>, lengthOf = (i: number) => 5 + (i % 7)) =>
  Array.from({ length: n }, (_, i) => shot(lengthOf(i), at[i] ?? null));

/** Four shot comments contain "sump" (one capitalised); the last sits far down the table. */
const sections: SectionSeed[] = [
  { name: 'Main', direction: 'in', shots: many(30, { 0: 'sump entrance', 3: 'dry passage' }) },
  { name: 'Side', direction: 'in', shots: many(5, { 2: 'Sump pool' }, () => 40) },
  { name: 'Deep', direction: 'in', shots: many(40, { 10: 'sump two', 38: 'deep sump' }) },
];

async function openData(page: Page): Promise<void> {
  await setupMockedPage(page);
  await createProject(page, 'Data Search');
  await seedProjectWithSections(page, sections);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await page.getByTestId('mode-data').click();
  await expect(page.getByTestId('data-toolbar')).toBeVisible({ timeout: 10000 });
}

async function find(page: Page, query: string): Promise<void> {
  const input = page.getByTestId('data-find-input');
  await input.click();
  await input.fill(query);
}

async function shotComments(page: Page): Promise<(string | null)[]> {
  return page.evaluate(() => {
    const e2e = (window as unknown as { __E2E_TEST__: { projectStore: { getState: () => { project: { sections: { shots: { comment?: string | null }[] }[] } } } } }).__E2E_TEST__;
    return e2e.projectStore
      .getState()
      .project.sections.flatMap((s) => s.shots.map((x) => x.comment ?? null))
      .filter((c) => c !== null);
  });
}

async function batchCalls(page: Page): Promise<unknown[][]> {
  return page.evaluate(() => ((window as unknown as { __MOCK_EXECUTE_BATCH_CALLS__?: unknown[][] }).__MOCK_EXECUTE_BATCH_CALLS__ ?? []));
}

test.describe('Data workspace - find / replace', () => {
  test.beforeEach(async ({ page }) => {
    await openData(page);
  });

  test('find counts and highlights matches', async ({ page }) => {
    await find(page, 'sump');
    await expect(page.getByTestId('data-find-count')).toHaveText('1 of 4');
    await expect(page.locator(MATCH_MARK).first()).toBeVisible();
    await page.getByTestId('data-find-match-case').click();
    await expect(page.getByTestId('data-find-count')).toHaveText('1 of 3');
    await page.getByTestId('data-find-match-case').click();
    await page.getByTestId('data-find-regex').click();
    await find(page, 'su(mp');
    await expect(page.getByTestId('data-find-error')).toBeVisible();
  });

  test('next / previous scroll the grid to the match', async ({ page }) => {
    await find(page, 'deep sump');
    await expect(page.getByTestId('data-find-count')).toHaveText('1 of 1');
    await expect(page.getByTestId('data-workspace').getByText('deep sump')).toBeInViewport();
    // A new query starts from the active row (the "deep sump" one): the last of four.
    await find(page, 'sump');
    await expect(page.getByTestId('data-find-count')).toHaveText('4 of 4');
    // Next wraps to the first match and scrolls back up to it...
    await page.keyboard.press('F3');
    await expect(page.getByTestId('data-find-count')).toHaveText('1 of 4');
    await expect(page.getByTestId('data-workspace').getByText('sump entrance')).toBeInViewport();
    // ...and previous wraps back down to the last one.
    await page.getByTestId('data-find-input').press('Shift+Enter');
    await expect(page.getByTestId('data-find-count')).toHaveText('4 of 4');
    await expect(page.getByTestId('data-workspace').getByText('deep sump')).toBeInViewport();
  });

  test('Replace changes only the current match, then moves on', async ({ page }) => {
    await find(page, 'sump');
    await expect(page.getByTestId('data-find-count')).toHaveText('1 of 4');
    await page.keyboard.press('Control+h');
    await page.getByTestId('data-replace-input').fill('siphon');
    await page.getByTestId('data-replace-one').click();
    await expect(page.getByTestId('data-find-count')).toHaveText(/of 3$/);
    expect(await shotComments(page)).toContain('siphon entrance');
    expect(await shotComments(page)).toContain('deep sump');
  });

  test('Replace all asks first, then is ONE batch (one undo step)', async ({ page }) => {
    await find(page, 'sump');
    await page.getByTestId('data-find-replace-toggle').click();
    await page.getByTestId('data-replace-input').fill('siphon');
    const before = (await batchCalls(page)).length;
    await page.getByTestId('data-replace-all').click();
    await expect(page.getByTestId('data-replace-confirm-text')).toHaveText('Replace 4 matches in 4 cells?');
    await page.getByTestId('data-replace-confirm-ok').click();
    await expect(page.getByTestId('data-find-count')).toHaveText('No results');
    const calls = await batchCalls(page);
    expect(calls.length).toBe(before + 1);
    expect(calls[calls.length - 1]).toHaveLength(4);
    expect(await shotComments(page)).toEqual(
      expect.arrayContaining(['siphon entrance', 'siphon pool', 'siphon two', 'deep siphon', 'dry passage']),
    );
  });

  test('searching all rows shows a filtered-out match, marked as revealed', async ({ page }) => {
    await page.getByTestId('data-sections-button').click();
    await page.getByTestId('data-sections-search').fill('Side');
    await page.locator('[data-testid^="data-sections-only-"]').first().click();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('data-chip-sections')).toHaveText(/Sections: Side/);
    await find(page, 'deep sump');
    await expect(page.getByTestId('data-find-count')).toHaveText('No results');
    await page.getByTestId('data-find-options').click();
    await page.getByTestId('data-find-rows-all').check();
    await expect(page.getByTestId('data-find-count')).toHaveText('1 of 1');
    await expect(page.getByTestId('data-chip-revealed')).toHaveText(/1 row shown by search/);
    await expect(page.getByTestId('data-workspace').getByText('deep sump')).toBeVisible();
    await page.keyboard.press('Escape');
    await page.getByTestId('data-chip-revealed-hide').click();
    await expect(page.getByTestId('data-workspace').getByText('deep sump')).toHaveCount(0);
  });

  test('searching all columns shows a hidden column that holds a match', async ({ page }) => {
    await page.getByTestId('data-columns-button').click();
    await page.getByTestId('data-columns-col-comment').uncheck();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId(`${GRID}-header-comment`)).toHaveCount(0);
    await find(page, 'sump');
    await expect(page.getByTestId('data-find-count')).toHaveText('No results');
    await page.getByTestId('data-find-options').click();
    await page.getByTestId('data-find-cols-all').check();
    await expect(page.getByTestId('data-find-count')).toHaveText('1 of 4');
    await expect(page.getByTestId('data-chip-revealed-columns')).toBeVisible();
    await expect(page.getByTestId(`${GRID}-header-comment`)).toBeVisible();
  });
});

test.describe('Data workspace - filters, sort, columns', () => {
  test.beforeEach(async ({ page }) => {
    await openData(page);
  });

  test('filters show as removable chips; Clear all removes them', async ({ page }) => {
    await page.getByTestId('data-rowtype-shots').click();
    await expect(page.getByTestId('data-chip-rowtype')).toHaveText(/Shots only/);
    await page.getByTestId('data-filter-button').click();
    await page.getByTestId('data-filter-quick-comments').check();
    await page.getByTestId('data-filter-add-op').click();
    await page.getByTestId('data-filter-add-op-option-gt').click();
    await page.getByTestId('data-filter-add-value').fill('30');
    await page.getByTestId('data-filter-add-submit').click();
    await page.keyboard.press('Escape');
    // the label may be icon-only on a narrow toolbar; the accessible name keeps it
    await expect(page.getByTestId('data-filter-button')).toHaveAccessibleName(/Filter \(2\)/);
    await expect(page.getByTestId('data-chip-quick-comments')).toBeVisible();
    // only "Sump pool" (Side, 40 m) has a comment and is longer than 30
    await expect(page.locator('[data-testid^="data-chip-condition-"]').first()).toHaveText(/Length \(m\) > 30/);
    await expect(page.getByTestId('data-workspace').getByText('Sump pool')).toBeVisible();
    await expect(page.getByTestId('data-workspace').getByText('sump entrance')).toHaveCount(0);
    await page.getByTestId('data-chip-quick-comments-remove').click();
    await expect(page.getByTestId('data-chip-quick-comments')).toHaveCount(0);
    await page.getByTestId('data-chips-clear-all').click();
    await expect(page.getByTestId('data-chips')).toHaveCount(0);
    await expect(page.getByTestId('data-workspace').getByText('sump entrance')).toBeVisible();
  });

  test('sorting shows the sort bar; Back to survey order resets it', async ({ page }) => {
    // The header's sort button (shown on hover) sorts; a plain header click selects the column.
    await page.getByTestId(`${GRID}-header-length`).hover();
    await page.getByTestId(`${GRID}-sort-length`).click();
    await expect(page.getByTestId('data-sort-bar')).toBeVisible();
    await expect(page.getByTestId('data-sort-bar-text')).toHaveText(/Sorted by Length/);
    await page.getByTestId('data-sort-reset').click();
    await expect(page.getByTestId('data-sort-bar')).toHaveCount(0);
  });

  test('columns menu: hide, Show all, Reset to default', async ({ page }) => {
    await page.getByTestId('data-columns-button').click();
    const count = page.getByTestId('data-columns-count');
    const initial = await count.textContent();
    await page.getByTestId('data-columns-show-all').click();
    await expect(count).toHaveText(/^30 of 30 shown$/);
    await expect(page.getByTestId(`${GRID}-header-temperature`)).toBeVisible();
    await page.getByTestId('data-columns-col-temperature').uncheck();
    await expect(page.getByTestId(`${GRID}-header-temperature`)).toHaveCount(0);
    await page.getByTestId('data-columns-reset').click();
    await expect(count).toHaveText(initial ?? '');
  });
});
