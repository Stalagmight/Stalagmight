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
 * E2E tests for the DataTable with large datasets.
 *
 * The shots view renders a SpreadsheetGrid backed by @tanstack/react-virtual:
 *   ROW_HEIGHT = 33px
 *   overscan = 8
 *
 * Legacy structural selectors remain available, while semantic row/cell aliases
 * use section and shot IDs. Tests verify both contracts across virtualization.
 */

const SHOT_COUNT = 120;

/** Generate a section with many shots. */
function buildLargeSection(): SectionSeed {
  const shots = Array.from({ length: SHOT_COUNT }, (_, i) => ({
    azimuth_in: (i * 3) % 360,
    azimuth_out: ((i * 3) + 180) % 360,
    length: 5 + (i % 10),
    depth_in: -(i * 0.5),
    depth_out: -((i + 1) * 0.5),
  }));

  return {
    name: 'Long Passage',
    direction: 'in' as const,
    shots,
  };
}

/** Open the DataTable via keyboard shortcut. */
async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  // Wait for grid rows to render (auto-selects first section)
  await page.waitForSelector('[data-testid^="grid-row-"]', { timeout: 10000 });
}

test.describe('Virtual Scrolling in DataTable', () => {
  // Seeding 120 shots can be slow; extend test timeout
  test.setTimeout(60000);

  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Virtual Scroll Test');
    await seedProjectWithSections(page, [buildLargeSection()]);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openDataTable(page);
  });

  test('DataTable virtualizes rows while preserving full section data', async ({ page }) => {
    const totalShots = await page.evaluate(() => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return 0;
      return e2e.projectStore.getState().project?.sections?.[0]?.shots?.length ?? 0;
    });
    expect(totalShots).toBe(SHOT_COUNT);

    const renderedRows = page.locator('[data-testid^="grid-row-"]');
    const rowCount = await renderedRows.count();
    expect(rowCount).toBeGreaterThan(0);
    expect(rowCount).toBeLessThan(SHOT_COUNT);
  });

  test('first row exposes unique semantic shot and cell selectors', async ({ page }) => {
    const ids = await page.evaluate(() => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return null;
      const section = e2e.projectStore.getState().project?.sections?.[0];
      return section ? { sectionId: section.id, shotId: section.shots[0]?.id ?? null } : null;
    });
    if (!ids || ids.shotId == null) throw new Error('No first shot identity');

    const firstRow = page.getByTestId('grid-row-0');
    await expect(firstRow).toBeVisible();
    // The row gutter shows the section:position shot id (positions are 1-based)
    await expect(page.getByTestId('grid-gutter-0')).toContainText(`${ids.sectionId}:1`);

    const semanticRow = page.getByTestId(`shot-row-${ids.sectionId}-${ids.shotId}`);
    const semanticCell = page.getByTestId(`shot-cell-${ids.sectionId}-${ids.shotId}-length`);
    await expect(semanticRow).toHaveCount(1);
    await expect(semanticRow).toBeVisible();
    await expect(semanticCell).toHaveCount(1);
    await semanticCell.click();
    await expect(firstRow).toHaveAttribute('data-active', 'true');
  });

  test('last row exists in the DOM', async ({ page }) => {
    const lastShotId = await page.evaluate(() => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return null;
      const state = e2e.projectStore.getState();
      const section = state.project?.sections?.[0];
      const shots = section?.shots ?? [];
      return shots[shots.length - 1]?.id ?? null;
    });
    expect(lastShotId).not.toBeNull();

    await page.evaluate((shotId) => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return;
      const section = e2e.projectStore.getState().project?.sections?.[0];
      if (!section || shotId == null) return;
      e2e.uiStore.getState().setSelection({ type: 'shot', sectionId: section.id, shotId });
    }, lastShotId);

    // The grid scrolls to the externally-selected row and marks it active
    const lastRow = page.getByTestId(`grid-row-${SHOT_COUNT - 1}`);
    await expect(lastRow).toBeVisible({ timeout: 5000 });
    await expect(lastRow).toHaveAttribute('data-active', 'true');
  });

  test('selection via store updates row highlight', async ({ page }) => {
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
    if (!ids) throw new Error('No project IDs');

    // Select a shot near the end (index 100) via store
    const targetShotId = ids.shotIds[100]!; // in range: beforeEach seeds SHOT_COUNT (120) shots
    await page.evaluate(
      ({ sectionId, shotId }) => {
        const e2e = (window as E2EWindow).__E2E_TEST__;
        if (!e2e) return;
        e2e.uiStore.getState().setSelection({ type: 'shot', sectionId, shotId });
      },
      { sectionId: ids.sectionId, shotId: targetShotId },
    );

    // The grid moves its active cell to the externally-selected row
    // (exercises the selectedRow prop, including the scroll into view)
    const targetRow = page.getByTestId('grid-row-100');
    await expect(targetRow).toHaveAttribute('data-active', 'true', { timeout: 5000 });
  });

  test('row click selects shot in large table', async ({ page }) => {
    await page.getByTestId('grid-cell-0-length').click();

    // Clicked row becomes the grid's active row
    await expect(page.getByTestId('grid-row-0')).toHaveAttribute('data-active', 'true');
  });

  test('selected row has visual highlight class', async ({ page }) => {
    await page.getByTestId('grid-cell-0-length').click();

    // The row-number gutter of the active row gets the active highlight
    await expect(page.getByTestId('grid-gutter-0')).toHaveClass(/sg-gutter-active/);
  });

  test('DataTable header section pill shows section name', async ({ page }) => {
    const sectionPill = page.getByTestId('data-table-section-pill');
    await expect(sectionPill).toBeVisible();
    await expect(sectionPill).toContainText('Long Passage');
  });
});
