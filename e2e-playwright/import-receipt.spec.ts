import { test, expect } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';

/**
 * Coverage for the post-import receipt (#391): the "what landed" summary shown
 * after a foreign-format import, and the "import as new project?" confirm
 * dialog that replaces the old close-first dead end when a new-project-only
 * format is picked while a project is already open.
 */

async function setImportPath(page: import('./fixtures/coverage').Page, path: string): Promise<void> {
  await page.evaluate((selectedPath) => {
    const overrides = (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ as
      | Record<string, unknown>
      | undefined;
    const next = overrides ?? {};
    next['plugin:dialog|open'] = selectedPath;
    (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = next;
  }, path);
}

test.describe('Import receipt dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('shows counts and a details toggle for hidden warnings', async ({ page }) => {
    await createProject(page, 'Receipt Content Project');

    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: { uiStore: { getState: () => { openDialog: (k: string, d: unknown) => void } } };
      }).__E2E_TEST__;
      e2e?.uiStore.getState().openDialog('import-receipt', {
        receipt: {
          source_format: 'Compass',
          file_name: 'cave.dat',
          lines_found: 6,
          lines_imported: 5,
          shot_count: 42,
          closures_requested: 2,
          closures_created: 1,
          unit_conversion: 'Lengths converted from feet to meters',
          warnings: ['Survey A had 2 unresolved stations', 'Survey B date is missing'],
          info: [],
        },
      });
    });

    const dialog = page.getByTestId('import-receipt-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('Compass');
    await expect(dialog).toContainText('cave.dat');

    await expect(page.getByTestId('import-receipt-summary')).toContainText('5');
    await expect(page.getByTestId('import-receipt-summary')).toContainText('42');
    // Exact text, not a substring: the rendered line must be exactly the
    // backend's already-complete sentence, never doubled up with frontend
    // prose (e.g. "Converted Lengths converted from feet to meters").
    await expect(page.getByTestId('import-receipt-unit-conversion')).toHaveText(
      'Lengths converted from feet to meters',
    );

    // 2 real warnings, no frontend-derived "not imported" note (lines_found >
    // lines_imported is not a warning - the wizards let users deliberately
    // deselect surveys) = 2 visible, none hidden yet.
    await expect(page.getByTestId('import-receipt-warnings')).toBeVisible();
    await expect(page.getByTestId('import-receipt-details')).toBeHidden();

    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: { uiStore: { getState: () => { openDialog: (k: string, d: unknown) => void } } };
      }).__E2E_TEST__;
      e2e?.uiStore.getState().openDialog('import-receipt', {
        receipt: {
          source_format: 'Compass',
          file_name: 'cave.dat',
          lines_found: 6,
          lines_imported: 4,
          shot_count: 42,
          closures_requested: 0,
          closures_created: 0,
          unit_conversion: null,
          warnings: [
            'Survey A had 2 unresolved stations',
            'Survey B date is missing',
            'Survey C was empty',
            'Survey D had a zero-shot line',
          ],
          info: [],
        },
      });
    });

    const detailsButton = page.getByTestId('import-receipt-details');
    await expect(detailsButton).toBeVisible();
    await expect(page.getByTestId('import-receipt-warnings-more')).toBeHidden();
    await detailsButton.click();
    await expect(page.getByTestId('import-receipt-warnings-more')).toBeVisible();

    await page.getByTestId('import-receipt-continue').click();
    await expect(dialog).toBeHidden();
  });

  test('offers to close the current project before a new-project-only import; Confirm re-routes, Cancel keeps it open', async ({
    page,
  }) => {
    await createProject(page, 'Close Before Import Project');
    await setImportPath(page, '/mock/reef-traverse.gpx');

    await page.keyboard.press('Control+i');

    const confirmDialog = page.getByTestId('import-as-new-project-dialog');
    await expect(confirmDialog).toBeVisible({ timeout: 10000 });
    await expect(confirmDialog).toContainText('Close Before Import Project');

    // Cancel: the current project stays open, nothing is imported.
    await page.getByTestId('import-as-new-project-cancel').click();
    await expect(confirmDialog).toBeHidden();
    await expect(page.getByTestId('single-system-view')).toBeVisible();

    const projectNameAfterCancel = await page.evaluate(() => {
      const state = (window as unknown as {
        __E2E_TEST__?: { projectStore: { getState: () => { project: { name: string } | null } } };
      }).__E2E_TEST__?.projectStore.getState();
      return state?.project?.name ?? null;
    });
    expect(projectNameAfterCancel).toBe('Close Before Import Project');

    // Confirm: the project closes and the picked file re-routes into the
    // new-project-for-import flow.
    await page.keyboard.press('Control+i');
    await expect(confirmDialog).toBeVisible({ timeout: 10000 });
    await page.getByTestId('import-as-new-project-confirm').click();

    await expect(page.getByTestId('new-project-for-import-dialog')).toBeVisible({ timeout: 10000 });
  });
});
