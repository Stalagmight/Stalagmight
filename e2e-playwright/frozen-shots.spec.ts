import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';

/**
 * E2E tests for frozen shot display and edit enforcement.
 *
 * Frozen shots have their geometry locked (length, azimuth, depth) and
 * display a lock icon with a warning background. Non-geometry fields
 * (like comment) remain editable.
 *
 * The shots view renders a SpreadsheetGrid: rows are grid-row-{rowIndex}
 * (index, not shot id), cells grid-cell-{rowIndex}-{colKey}, and editing
 * opens the grid-cell-editor textarea. Frozen rows get the sg-row-frozen
 * class; the lock icon keeps its frozen-{sectionId}-{shotId} testid.
 *
 * Strategy: After seeding sections, open the DataTable, then inject
 * frozenShots into the store so the DataTable re-renders with frozen state.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Frozen Test Section',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: 0, depth_out: -2, comment: 'First shot' },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -2, depth_out: -4, comment: 'Second shot' },
      { azimuth_in: 135, azimuth_out: 315, length: 8, depth_in: -4, depth_out: -6, comment: 'Third shot' },
    ],
  },
];

/** Get section and shot IDs. */
async function getProjectIds(page: Page): Promise<{
  sectionId: number;
  shotIds: number[];
}> {
  const ids = await page.evaluate(() => {
    const e2e = (window as E2EWindow).__E2E_TEST__;
    if (!e2e) return null;
    const state = e2e.projectStore.getState();
    const section = state.project?.sections?.[0];
    return {
      sectionId: section?.id ?? 0,
      shotIds: (section?.shots ?? []).map((s) => s.id),
    };
  });
  if (!ids) throw new Error('Could not retrieve section/shot IDs');
  return ids;
}

/**
 * Inject frozen shots into the project store directly.
 * The DataTable subscribes to this state and re-renders accordingly.
 */
async function injectFrozenShots(
  page: Page,
  frozenShots: Array<{ section_id: number; shot_index: number }>,
): Promise<void> {
  await page.evaluate(
    (frozen) => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return;
      e2e.projectStore.setState({ frozenShots: frozen });
    },
    frozenShots,
  );
  // Wait for the re-render itself: every frozen row of the shown section carries
  // the frozen tint (the injected shots all belong to it).
  for (const { shot_index } of frozenShots) {
    await expect(page.getByTestId(`grid-row-${shot_index}`)).toHaveClass(/sg-row-frozen/);
  }
}

/** Open the DataTable via keyboard shortcut. */
async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  // Wait for grid rows to render (auto-selects first section)
  await page.waitForSelector('[data-testid^="grid-row-"]', { timeout: 5000 });
}

test.describe('Frozen Shots', () => {
  let sectionId: number;
  let shotIds: number[];

  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Frozen Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    const ids = await getProjectIds(page);
    sectionId = ids.sectionId;
    shotIds = ids.shotIds;

    // Open DataTable first so it subscribes to store
    await openDataTable(page);

    // Then inject frozen shots - DataTable will re-render
    await injectFrozenShots(page, [{ section_id: sectionId, shot_index: 0 }]);
  });

  test('lock icon visible for frozen shot in warnings column', async ({ page }) => {
    const frozenIndicator = page.getByTestId(`frozen-${sectionId}-${shotIds[0]}`);
    await expect(frozenIndicator).toBeVisible();
  });

  test('frozen shot row has sg-row-frozen class', async ({ page }) => {
    // Shot index 0 is frozen (grid rows are addressed by index, not shot id)
    const row = page.getByTestId('grid-row-0');
    await expect(row).toBeVisible();
    await expect(row).toHaveClass(/sg-row-frozen/);

    // Non-frozen row does not get the tint
    const otherRow = page.getByTestId('grid-row-1');
    await expect(otherRow).not.toHaveClass(/sg-row-frozen/);
  });

  test('non-frozen shots do not show lock icon', async ({ page }) => {
    // Shot at index 1 is not frozen - should not have a frozen-* testid
    const frozenIndicator = page.getByTestId(`frozen-${sectionId}-${shotIds[1]}`);
    await expect(frozenIndicator).toHaveCount(0);
  });

  test('geometry field (length) not editable on frozen shot', async ({ page }) => {
    const editor = page.getByTestId('grid-cell-editor');

    // Positive control: the same gesture opens the editor on the non-frozen row,
    // so an editor-less frozen row below cannot pass vacuously.
    await page.getByTestId('grid-cell-1-length').dblclick();
    await expect(editor).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(editor).toHaveCount(0);

    // Frozen row: double-clicking the length cell must NOT open the editor
    await page.getByTestId('grid-cell-0-length').dblclick();
    // settle: proves the editor does NOT open; a poll for its absence would pass
    // on the first read, before a buggy dblclick handler had rendered it.
    await page.waitForTimeout(300);
    await expect(editor).toHaveCount(0);
  });

  test('non-frozen shot geometry fields remain editable', async ({ page }) => {
    const cell = page.getByTestId('grid-cell-1-length');
    await expect(cell).toBeVisible();

    // Double-click opens the cell editor, focused and ready for input
    await cell.dblclick();
    const editor = page.getByTestId('grid-cell-editor');
    await expect(editor).toBeVisible();
    await expect(editor).toBeFocused();
  });

  test('multiple frozen shots in same section all display lock icons', async ({ page }) => {
    // Freeze both shot 0 and shot 2
    await injectFrozenShots(page, [
      { section_id: sectionId, shot_index: 0 },
      { section_id: sectionId, shot_index: 2 },
    ]);

    const frozen0 = page.getByTestId(`frozen-${sectionId}-${shotIds[0]}`);
    const frozen2 = page.getByTestId(`frozen-${sectionId}-${shotIds[2]}`);

    await expect(frozen0).toBeVisible();
    await expect(frozen2).toBeVisible();
  });

  test('frozen shot tooltip contains frozen text', async ({ page }) => {
    const frozenIndicator = page.getByTestId(`frozen-${sectionId}-${shotIds[0]}`);
    const title = await frozenIndicator.getAttribute('title');
    // The tooltip should contain the i18n frozen message: "Geometry locked - part of adjusted network"
    expect(title).toContain('locked');
  });
});
