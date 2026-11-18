import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  openDmpSelectionDialog,
  waitForMapReady,
  waitForNoDialogs,
  waitForMapIdle,
  type LineSeed,
} from './helpers/app';

const lineSeeds: LineSeed[] = [
  {
    name: 'Alignment Test',
    direction: 'in',
    shots: [
      {
        azimuth_in: 45,
        azimuth_out: 225,
        length: 10.5,
        depth_in: -2,
        depth_out: -2.5,
        left: 1.5,
        right: 2.3,
        up: 3.0,
        down: 0.8,
        comment: 'Test comment',
      },
      {
        azimuth_in: 90,
        azimuth_out: 270,
        length: 15.2,
        depth_in: -2.5,
        depth_out: -5.0,
        left: 0.5,
        right: 0.7,
        up: 1.2,
        down: 1.5,
        comment: null,
      },
    ],
  },
];

async function createProjectWithLines(page: Page): Promise<void> {
  await createProject(page, 'Alignment Test Project');
  await seedProjectWithLines(page, lineSeeds);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
}

async function openEditLineDialog(page: Page): Promise<void> {
  const lineId = await page.evaluate(() => {
    const project = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { lines: Array<{ id: number; name: string }> } | null } };
      };
    }).__E2E_TEST__?.projectStore.getState().project;

    return project?.lines.find((s) => s.name === 'Alignment Test')?.id ?? null;
  });

  if (lineId === null) {
    throw new Error('Line not found: Alignment Test');
  }

  await page.evaluate((id) => {
    const uiStore = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { openDialog: (dialog: string, data?: Record<string, unknown>) => void } } };
    }).__E2E_TEST__?.uiStore;

    if (!uiStore) return;

    uiStore.getState().openDialog('edit-line', { lineId: id });
  }, lineId);

  await expect(page.getByTestId('edit-line-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('Table Numeric Column Alignment', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProjectWithLines(page);
  });

  test('numeric columns render in the shot-table grid', async ({ page }) => {
    await openEditLineDialog(page);

    // The shot table is a SpreadsheetGrid now, so headers are divs keyed by
    // column rather than <th>s, and (as everywhere else in the app) they are
    // left-aligned regardless of the column's data alignment.
    for (const key of ['length', '_azimuth_avg', 'depth_in', 'depth_out', 'left', 'right', 'up', 'down']) {
      await expect(page.getByTestId(`shot-editor-header-${key}`)).toBeVisible();
    }
  });

  test('numeric cells are right-aligned, the comment cell is not', async ({ page }) => {
    await openEditLineDialog(page);

    const dialog = page.getByTestId('edit-line-dialog');
    await expect(dialog).toBeVisible();

    // The grid aligns with flex rather than `text-align`: the cell content
    // wrapper (which carries the cell test ID) is justified to the end.
    for (const key of ['length', '_azimuth_avg', 'depth_in', 'depth_out', 'left', 'right', 'up', 'down']) {
      const cell = dialog.getByTestId(`shot-editor-cell-0-${key}`);
      await expect(cell).toBeVisible();
      await expect(cell).toHaveCSS('justify-content', 'flex-end');
    }

    const comment = dialog.getByTestId('shot-editor-cell-0-comment');
    await expect(comment).toBeVisible();
    await expect(comment).not.toHaveCSS('justify-content', 'flex-end');
  });

  test('numeric cells use the tabular-nums font variant', async ({ page }) => {
    await openEditLineDialog(page);

    const dialog = page.getByTestId('edit-line-dialog');
    await expect(dialog).toBeVisible();

    // Fixed-width digits so a column of numbers lines up. The old table set this
    // per input; the grid sets it on .sg-cell, which covers the data table too.
    await expect(dialog.getByTestId('shot-editor-cell-0-length')).toHaveCSS(
      'font-variant-numeric',
      'tabular-nums'
    );
    await expect(dialog.getByTestId('shot-editor-cell-0-_azimuth_avg')).toHaveCSS(
      'font-variant-numeric',
      'tabular-nums'
    );
  });

  test('the warnings column is centered', async ({ page }) => {
    await openEditLineDialog(page);

    const dialog = page.getByTestId('edit-line-dialog');
    await expect(dialog.getByTestId('shot-editor-cell-0-_warnings')).toHaveCSS(
      'justify-content',
      'center'
    );
  });
});

test.describe('DmpSelectionDialog Numeric Cell Alignment', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'DMP Alignment Test');
  });

  test('Shots column td cells should be right-aligned with tabular-nums', async ({ page }) => {
    await openDmpSelectionDialog(page);

    const dialog = page.getByTestId('dmp-selection-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId('line-row-0')).toBeVisible({ timeout: 5000 });

    // The Shots column is the last td in each row
    const firstRow = dialog.getByTestId('line-row-0');
    const lastTd = firstRow.locator('td').last();

    // Numeric <td> cells should be right-aligned
    await expect(lastTd).toHaveCSS('text-align', 'right');

    // Numeric <td> cells should use tabular-nums
    await expect(lastTd).toHaveCSS('font-variant-numeric', 'tabular-nums');

    // Verify text columns remain left-aligned (Name column is second td)
    const nameTd = firstRow.locator('td').nth(1);
    const nameAlign = await nameTd.evaluate((el) => getComputedStyle(el).textAlign);
    expect(['left', 'start']).toContain(nameAlign);
  });
});
