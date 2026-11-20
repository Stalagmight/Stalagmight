import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, createLineViaStore } from './helpers/app';

/**
 * The `.stalx` v2 file contract in the UI (plan 6.4, 6.5, 7.2.10):
 * - a save refused because another program changed the file
 *   (`EXTERNALLY_MODIFIED`) asks which version to keep;
 * - a file made with a newer Stalagmight is open read-only, with a banner, and
 *   the editing controls are off;
 * - an old-format file the conversion moved stations in says so once;
 * - a warning ignore rides the save, so it dirties the project.
 *
 * The mock answers through `__MOCK_SET_EXTERNALLY_MODIFIED__`,
 * `__MOCK_SET_READ_ONLY_REASON__` and `__MOCK_SET_CONVERSION_NOTICE__`; the
 * file status is read once a project is open, so the read-only reason and the
 * notice are seeded before the project is created.
 */

type MockWindow = {
  __MOCK_SET_EXTERNALLY_MODIFIED__: (on: boolean) => void;
  __MOCK_SET_READ_ONLY_REASON__: (reason: string | null) => void;
  __MOCK_SET_CONVERSION_NOTICE__: (notice: string | null) => void;
  __MOCK_GET_SAVE_CALLS__: () => Array<{ command: string; path: string | null }>;
  __E2E_TEST__: {
    projectStore: {
      getState: () => {
        isDirty: boolean;
        resolveWarning: (lineId: number, shotId: number, code: string) => Promise<boolean>;
      };
    };
  };
};

async function makeDirty(page: Page): Promise<void> {
  await createLineViaStore(page, {
    name: 'Changed Line',
    shots: [{ azimuth_in: 0, azimuth_out: 0, length: 5, depth_in: 0, depth_out: 0 }],
  });
  await page.waitForFunction(() => (window as unknown as MockWindow).__E2E_TEST__.projectStore.getState().isDirty);
}

const isDirty = (page: Page) =>
  page.evaluate(() => (window as unknown as MockWindow).__E2E_TEST__.projectStore.getState().isDirty);

const saveCalls = (page: Page) =>
  page.evaluate(() => (window as unknown as MockWindow).__MOCK_GET_SAVE_CALLS__().map((c) => c.command));

test.describe('.stalx v2 file status', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('a save refused because the file changed on disk asks which version to keep', async ({ page }) => {
    await createProject(page, 'Changed Elsewhere');
    await makeDirty(page);
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_EXTERNALLY_MODIFIED__(true));

    await page.keyboard.press('Control+s');
    const dialog = page.getByTestId('save-conflict-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('save-conflict-lead')).toContainText('after Stalagmight opened it');
    await expect(page.getByTestId('save-conflict-overwrite')).toHaveText('Keep my version');
    await expect(page.getByTestId('save-conflict-save-as')).toBeVisible();
    await expect(page.getByTestId('save-conflict-reload')).toBeVisible();
    // Nothing was written, and the edits are still unsaved.
    expect(await isDirty(page)).toBe(true);
    // Not a raw error.
    await expect(page.getByTestId('error-dialog')).toHaveCount(0);

    await page.getByTestId('save-conflict-overwrite').click();
    await expect(dialog).not.toBeVisible();
    await expect.poll(() => isDirty(page)).toBe(false);
    expect(await saveCalls(page)).toEqual(['save_project', 'save_project_overwriting']);
  });

  test('an autosave hit by the change says so in the banner, and Save now asks', async ({ page }) => {
    await createProject(page, 'Changed Behind Autosave');
    await makeDirty(page);
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_EXTERNALLY_MODIFIED__(true));
    await page.evaluate(async () => {
      await (window as unknown as {
        __E2E_TEST__: { projectStore: { getState: () => { saveProject: (p?: string, a?: boolean) => Promise<unknown> } } };
      }).__E2E_TEST__.projectStore.getState().saveProject(undefined, true);
    });

    const banner = page.getByTestId('autosave-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('changed by another program');
    await expect(banner).not.toContainText('EXTERNALLY_MODIFIED');
    // A background save does not raise a modal by itself...
    await expect(page.getByTestId('save-conflict-dialog')).toHaveCount(0);
    // ...the user's Save now does.
    await page.getByTestId('autosave-save-button').click();
    await expect(page.getByTestId('save-conflict-dialog')).toBeVisible();
    await page.getByTestId('save-conflict-cancel').click();
    await expect(page.getByTestId('save-conflict-dialog')).toHaveCount(0);
    expect(await isDirty(page)).toBe(true);
  });

  test('a file made with a newer Stalagmight opens read-only, with the reason', async ({ page }) => {
    await page.evaluate(() =>
      (window as unknown as MockWindow).__MOCK_SET_READ_ONLY_REASON__(
        'made with a newer Stalagmight; it requires features this version does not know: lofts'
      )
    );
    await createProject(page, 'From The Future');

    const banner = page.getByTestId('read-only-file-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('made with a newer version of Stalagmight');
    await expect(banner).toContainText('edits and saves are turned off');
    await expect(page.getByTestId('read-only-file-reason')).toContainText('lofts');
    // An ongoing condition with nothing to do here: no button, no dismiss.
    await expect(banner.getByRole('button')).toHaveCount(0);
    // The editing controls are off (the shared gate, useEditingBlocked).
    await expect(page.getByTestId('sections-add-button')).toBeDisabled();
    await expect(page.getByTestId('sections-new-section-button')).toBeDisabled();
  });

  test('an old-format file the conversion moved says so once, until dismissed', async ({ page }) => {
    await page.evaluate(() =>
      (window as unknown as MockWindow).__MOCK_SET_CONVERSION_NOTICE__(
        'Converted from the old file format; 4 stations moved by up to 0.42 m; the original is kept (Recent Projects: right-click, Restore an earlier copy).'
      )
    );
    await createProject(page, 'Old Format');

    const banner = page.getByTestId('conversion-notice-banner');
    await expect(banner).toBeVisible();
    await expect(page.getByTestId('conversion-notice-text')).toContainText('4 stations moved by up to 0.42 m');
    await expect(banner).toContainText('saved the next time you save');
    await page.getByTestId('conversion-notice-dismiss-button').click();
    await expect(banner).toHaveCount(0);
  });

  test('a warning ignore dirties the project: it rides the next save', async ({ page }) => {
    await createProject(page, 'Ignore Rides Save');
    expect(await isDirty(page)).toBe(false);
    const resolved = await page.evaluate(() =>
      (window as unknown as MockWindow).__E2E_TEST__.projectStore.getState().resolveWarning(1, 100000, 'LONG_SHOT')
    );
    expect(resolved).toBe(true);
    await expect.poll(() => isDirty(page)).toBe(true);
  });
});
