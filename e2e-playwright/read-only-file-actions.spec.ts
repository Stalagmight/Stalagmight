import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, createLineViaStore } from './helpers/app';

/**
 * A file made with a newer Stalagmight is open read-only (plan 6.4). Demo
 * issues N-1, N-7 and N-12:
 * - Save, Ctrl+S and Save and Close raised the "Save Failed ... report it as
 *   a bug" dialog, and Save and Close left the project open. Now the save and
 *   import rows are off, every save and import shortcut gets the worded
 *   read-only notice (as a typed key does), nothing is saved, and Close
 *   Project closes.
 * - The status bar said "Saved". Now it says "Read-only".
 * - A disabled row explained itself only in a native `title` WebKitGTK never
 *   draws, and a click on it did nothing. Now the app's tooltip says why, and a
 *   click shows the notice once.
 *
 * The mock refuses every save of the file (`READ_ONLY_FILE`) while
 * `__MOCK_SET_READ_ONLY_REASON__` is set; the reason is read once a project is
 * open, so it is set before the project is created.
 */

type MockWindow = {
  __MOCK_SET_READ_ONLY_REASON__: (reason: string | null) => void;
  __MOCK_GET_SAVE_CALLS__: () => Array<{ command: string; path: string | null }>;
  __E2E_TEST__: {
    projectStore: { getState: () => { project: { lines: Array<{ id: number }> } | null } };
    uiStore: { getState: () => { openDialog: (dialog: string, data?: Record<string, unknown>) => void } };
  };
};

const NOTICE = 'This project was made with a newer version of Stalagmight, so it is open read-only.';
const notice = (page: Page) => page.locator('[data-toast-key="edit-disabled"]');
const saveCalls = (page: Page) =>
  page.evaluate(() => (window as unknown as MockWindow).__MOCK_GET_SAVE_CALLS__().length);

async function openReadOnlyProject(page: Page, name: string): Promise<void> {
  await page.evaluate(() =>
    (window as unknown as MockWindow).__MOCK_SET_READ_ONLY_REASON__(
      'made with a newer Stalagmight; it requires features this version does not know: zz-future'
    )
  );
  await createProject(page, name);
  await expect(page.getByTestId('read-only-file-banner')).toBeVisible();
}

/** The notice is on screen once, worded, and no error dialog came with it; then dismiss it. */
async function expectNoticeOnce(page: Page): Promise<void> {
  await expect(notice(page)).toHaveCount(1);
  await expect(notice(page)).toContainText(NOTICE);
  await expect(page.getByTestId('error-dialog')).toHaveCount(0);
  await notice(page).getByRole('button').click();
  await expect(notice(page)).toHaveCount(0);
}

test.describe('a file made with a newer Stalagmight', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('cannot be saved or imported into, says why every time, and closes without saving', async ({ page }) => {
    await openReadOnlyProject(page, 'From The Future');
    await expect(page.getByTestId('save-status')).toHaveText('Read-only');
    const savesBefore = await saveCalls(page);

    // The menu: every row that would save or add is off; Open, Export and
    // Close Project stay.
    await page.getByTestId('app-menu-trigger').click();
    for (const id of [
      'toolbar-save-button',
      'project-menu-save-as',
      'project-menu-save-and-close',
      'global-import-menu-button',
      'app-menu-import-from-mnemo',
    ]) {
      await expect(page.getByTestId(id), id).toBeDisabled();
    }
    for (const id of ['global-open-project-button', 'toolbar-export-button', 'back-button']) {
      await expect(page.getByTestId(id), id).toBeEnabled();
    }
    // Pointing at a row says why, in the app's tooltip.
    await page.getByTestId('toolbar-save-button').hover();
    await expect(page.getByTestId('toolbar-save-button-reason')).toContainText(NOTICE);
    // Clicking one says it too, once, and does nothing else: no Save Failed
    // dialog, and the project stays open.
    await page.getByTestId('project-menu-save-and-close').click({ force: true });
    await page.getByTestId('project-menu-save-and-close').click({ force: true });
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    await page.keyboard.press('Escape');
    await expectNoticeOnce(page);

    // Every shortcut that would save or add gets the same notice.
    for (const keys of ['Control+s', 'Control+Shift+s', 'Control+i']) {
      await page.keyboard.press(keys);
      await expectNoticeOnce(page);
    }
    expect(await saveCalls(page)).toBe(savesBefore);

    // Close Project closes, with nothing to save and nothing asked.
    await page.getByTestId('app-menu-trigger').click();
    await page.getByTestId('back-button').click();
    await expect(page.getByTestId('global-view')).toBeVisible();
    await expect(page.getByTestId('unsaved-changes-dialog')).toHaveCount(0);
    expect(await saveCalls(page)).toBe(savesBefore);
  });

  test('station menu rows say why they are off, on hover and on a click', async ({ page }) => {
    await openReadOnlyProject(page, 'Future Stations');
    await createLineViaStore(page, {
      name: 'Main Line',
      shots: [
        { azimuth_in: 90, azimuth_out: 90, length: 10, depth_in: 0, depth_out: 2 },
        { azimuth_in: 90, azimuth_out: 90, length: 10, depth_in: 2, depth_out: 4 },
      ],
    });
    const lineId = await page.evaluate(
      () => (window as unknown as MockWindow).__E2E_TEST__.projectStore.getState().project?.lines[0]?.id ?? null
    );
    expect(lineId).not.toBeNull();
    await page.evaluate((line) => {
      (window as unknown as MockWindow).__E2E_TEST__.uiStore.getState().openDialog('context-menu', {
        target: { type: 'station', ref: { line_id: line, station_index: 1 } },
        position: { x: 300, y: 300 },
      });
    }, lineId);

    const edit = page.getByRole('button', { name: 'Edit Station' });
    await expect(edit).toBeDisabled();
    await edit.hover();
    await expect(page.getByRole('tooltip')).toContainText(NOTICE);
    await edit.click({ force: true });
    await edit.click({ force: true });
    await expect(notice(page)).toHaveCount(1);
    await expect(notice(page)).toContainText(NOTICE);
    // The click never reached the action.
    await expect(page.getByTestId('edit-station-dialog')).toHaveCount(0);
  });
});
