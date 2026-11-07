import { test, expect } from './fixtures/coverage';
import { setupMockedPage, createProject, selectDropdownOption } from './helpers/app';

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

test.describe('GPX Import Workflow', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('imports GPX from home screen through create-project and placement flow @smoke', async ({ page }) => {
    await setImportPath(page, '/mock/reef-traverse.gpx');

    await page.keyboard.press('Control+i');
    await expect(page.getByTestId('new-project-for-import-dialog')).toBeVisible({ timeout: 10000 });

    await page.getByTestId('project-name-input').fill('Reef Traverse Import');
    await page.getByTestId('create-and-continue-button').click();

    await expect(page.getByTestId('gpx-selection-dialog')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('gpx-import-waypoints')).toBeChecked();
    await expect(page.getByTestId('gpx-import-tracks')).toBeChecked();
    await expect(page.getByTestId('gpx-import-routes')).toBeChecked();

    await page.getByTestId('gpx-continue-button').click();
    await expect(page.getByTestId('gpx-placement-dialog')).toBeVisible({ timeout: 10000 });

    await selectDropdownOption(page, 'gpx-placement-zero-datum-0', '1');
    await page.getByTestId('gpx-import-confirm').click();

    await expect(page.getByTestId('gpx-placement-dialog')).toBeHidden({ timeout: 15000 });

    // Warning-free receipt: #411/#420 downgrade it to a toast, not the modal -
    // it carries the same counts but never blocks continuing to the map. Assert
    // the toast FIRST (it waits for the receipt to actually resolve) so the
    // modal check that follows isn't a same-instant false pass that would let
    // a regression opening BOTH surfaces through green.
    // `.filter` because the toast testid is per-toast and strict mode errors
    // on a second concurrent toast.
    await expect(page.getByTestId('toast').filter({ hasText: 'GPX' })).toContainText(
      'GPX import:',
      { timeout: 10000 }
    );
    await expect(page.getByTestId('import-receipt-dialog')).toBeHidden();

    await expect(page.getByTestId('single-system-view')).toBeVisible();

    const projectSummary = await page.evaluate(() => {
      const state = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              project: { sections: unknown[]; zero_datums: unknown[] } | null;
            };
          };
        };
      }).__E2E_TEST__?.projectStore.getState();

      return {
        sectionCount: state?.project?.sections.length ?? 0,
        datumCount: state?.project?.zero_datums.length ?? 0,
      };
    });

    expect(projectSummary.sectionCount).toBeGreaterThan(0);
    expect(projectSummary.datumCount).toBeGreaterThan(0);
  });

  test('offers to close the current project before importing GPX (new-project only)', async ({ page }) => {
    // GPX import is new-project-only: with a project open, #391 offers to close
    // and re-route rather than the old dead-end error - and Cancel leaves the
    // current project open, untouched.
    await createProject(page, 'Existing GPX Project');
    await setImportPath(page, '/mock/waypoints-only.gpx');

    await page.keyboard.press('Control+i');

    const confirmDialog = page.getByTestId('import-as-new-project-dialog');
    await expect(confirmDialog).toBeVisible({ timeout: 10000 });
    await expect(confirmDialog).toContainText('Existing GPX Project');
    // The in-project GPX selection flow must NOT open.
    await expect(page.getByTestId('gpx-selection-dialog')).toBeHidden();

    await page.getByTestId('import-as-new-project-cancel').click();
    await expect(confirmDialog).toBeHidden();

    // The current project is unchanged (nothing was imported into it, and the
    // project was not closed).
    const sectionCount = await page.evaluate(() => {
      const state = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: { getState: () => { project: { sections: unknown[] } | null } };
        };
      }).__E2E_TEST__?.projectStore.getState();
      return state?.project?.sections.length ?? 0;
    });
    expect(sectionCount).toBe(0);
  });
});
