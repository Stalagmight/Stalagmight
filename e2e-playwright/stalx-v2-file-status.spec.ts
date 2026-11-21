import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, createLineViaStore } from './helpers/app';

/**
 * The `.stalx` v2 file contract in the UI (plan 6.4, 6.5, 7.2.10):
 * - a save refused because another program changed the file
 *   (`EXTERNALLY_MODIFIED`) asks which version to keep, and "Use the file's
 *   version" is one reload that never closes the project first; one it moved
 *   or deleted (`FILE_MISSING`), or a file that appeared where a first save
 *   goes (`FILE_EXISTS`), says so and offers no reopen;
 * - a file made with a newer Stalagmight is open read-only, with a banner, and
 *   the editing controls are off;
 * - an old-format file the conversion moved stations in says so once, and the
 *   banner follows the converting save (demo issue I-5);
 * - a warning ignore rides the save, so it dirties the project.
 *
 * The mock answers through `__MOCK_SET_EXTERNALLY_MODIFIED__`,
 * `__MOCK_SET_READ_ONLY_REASON__`, `__MOCK_SET_CONVERSION_NOTICE__` and
 * `__MOCK_SET_LEGACY_ORIGIN__`; the
 * file status is read once a project is open, so the read-only reason and the
 * notice are seeded before the project is created.
 */

type MockWindow = {
  __MOCK_SET_EXTERNALLY_MODIFIED__: (on: boolean | 'missing' | 'exists') => void;
  __MOCK_EXIT_CALLS__?: Array<{ cmd: string; args: unknown }>;
  __MOCK_SET_READ_ONLY_REASON__: (reason: string | null) => void;
  __MOCK_SET_CONVERSION_NOTICE__: (notice: unknown) => void;
  __MOCK_SET_LEGACY_ORIGIN__: (on: boolean) => void;
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

/** The recorded close and reload calls, in order. */
const closeAndReloadCalls = (page: Page) =>
  page.evaluate(() =>
    ((window as unknown as MockWindow).__MOCK_EXIT_CALLS__ ?? [])
      .map((c) => c.cmd)
      .filter((cmd) => cmd === 'close_project' || cmd === 'reload_project_full_binary'),
  );

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

  test('a save refused because the file was moved or deleted keeps the edits and offers no reopen', async ({ page }) => {
    await createProject(page, 'Moved Elsewhere');
    await makeDirty(page);
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_EXTERNALLY_MODIFIED__('missing'));

    await page.keyboard.press('Control+s');
    const dialog = page.getByTestId('save-conflict-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('The project file was moved or deleted');
    await expect(page.getByTestId('save-conflict-lead')).toContainText('was moved or deleted by another program');
    await expect(page.getByTestId('save-conflict-lead')).not.toContainText('Another program changed');
    // No file's version to take: only the answers that keep the edits.
    await expect(page.getByTestId('save-conflict-reload')).toHaveCount(0);
    await expect(page.getByTestId('save-conflict-save-as')).toBeVisible();
    await expect(page.getByTestId('save-conflict-overwrite')).toHaveText('Save it where it was');
    expect(await isDirty(page)).toBe(true);

    // Save it where it was puts it back.
    await page.getByTestId('save-conflict-overwrite').click();
    await expect(dialog).not.toBeVisible();
    await expect.poll(() => isDirty(page)).toBe(false);
    expect(await saveCalls(page)).toEqual(['save_project', 'save_project_overwriting']);
  });

  // Pre-PR delta review finding 2: "Use the file's version" is ONE reload in
  // place of the session; the project is never closed first.
  test('"Use the file\'s version" reopens the file in place, without closing the project first', async ({ page }) => {
    await createProject(page, 'Theirs Wins');
    await makeDirty(page);
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_EXTERNALLY_MODIFIED__(true));

    await page.keyboard.press('Control+s');
    const dialog = page.getByTestId('save-conflict-dialog');
    await expect(dialog).toBeVisible();
    await page.getByTestId('save-conflict-reload').click();
    await expect(dialog).not.toBeVisible();
    await expect.poll(() => isDirty(page)).toBe(false);
    expect(await closeAndReloadCalls(page)).toEqual(['reload_project_full_binary']);
    // Still in the project, now the file's version.
    await expect(page.getByTestId('save-status')).toBeVisible();
  });

  test('a reload of a file that would not open keeps the project, the edits and the prompt', async ({ page }) => {
    await createProject(page, 'Gone Before Reload');
    await makeDirty(page);
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_EXTERNALLY_MODIFIED__(true));
    await page.keyboard.press('Control+s');
    const dialog = page.getByTestId('save-conflict-dialog');
    await expect(dialog).toBeVisible();
    // The file goes away while the prompt is open.
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_EXTERNALLY_MODIFIED__('missing'));

    await page.getByTestId('save-conflict-reload').click();
    await expect(page.getByTestId('save-conflict-error')).toContainText('your unsaved changes were kept');
    await expect(dialog).toBeVisible();
    expect(await isDirty(page)).toBe(true);
    expect(await closeAndReloadCalls(page)).toEqual(['reload_project_full_binary']);
  });

  // Pre-PR delta review finding 5: a file that appeared where a first save goes
  // is not this project's, so there is no "file's version" to take.
  test('a file that appeared where a first save goes is named, and offers no reopen', async ({ page }) => {
    await createProject(page, 'Path Taken');
    await makeDirty(page);
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_EXTERNALLY_MODIFIED__('exists'));

    await page.keyboard.press('Control+s');
    const dialog = page.getByTestId('save-conflict-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('Another file is in the way');
    await expect(page.getByTestId('save-conflict-lead')).toContainText('appeared at this location');
    await expect(page.getByTestId('save-conflict-reload')).toHaveCount(0);
    await expect(page.getByTestId('save-conflict-save-as')).toBeVisible();
    await expect(page.getByTestId('save-conflict-overwrite')).toBeVisible();
    expect(await isDirty(page)).toBe(true);
    await page.getByTestId('save-conflict-cancel').click();
    await expect(dialog).toHaveCount(0);
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
      (window as unknown as MockWindow).__MOCK_SET_CONVERSION_NOTICE__({
        changes: ['4 stations moved by up to 0.42 m', '1 station changed depth by 0.30 m'],
        details: [],
        file: 'not_yet_saved',
      })
    );
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_LEGACY_ORIGIN__(true));
    await createProject(page, 'Old Format');

    const banner = page.getByTestId('conversion-notice-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText(/^Converted from the old file format\. 4 stations moved/);
    await expect(page.getByTestId('conversion-notice-text')).toHaveText(
      '4 stations moved by up to 0.42 m; 1 station changed depth by 0.30 m.'
    );
    await expect(page.getByTestId('conversion-notice-save')).toContainText('It is saved in the new format the next time you save');
    await expect(page.getByTestId('conversion-notice-details-button')).toHaveCount(0);

    // The converting save (Ctrl+S with no edits): the banner says it happened.
    await page.keyboard.press('Control+s');
    await expect(page.getByTestId('conversion-notice-save')).toHaveText(
      'Saved in the new format; the original file is kept and can be restored from Recent Projects (right-click, Restore an earlier copy).'
    );
    // One event, one surface: the banner says it, so no toast does.
    await expect(page.locator('[data-toast-key="legacy-converted"]')).toHaveCount(0);

    await page.getByTestId('conversion-notice-dismiss-button').click();
    await expect(banner).toHaveCount(0);
  });

  // Demo issue N-6: the status bar said "Saved" while the file on disk was
  // still the 0.6.0 file, here one whose conversion changed nothing a line
  // shows (so no banner either). It says so until the converting save.
  test('an old-format file says so in the status bar until the converting save', async ({ page }) => {
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_LEGACY_ORIGIN__(true));
    await createProject(page, 'Old Format Quiet');
    await expect(page.getByTestId('conversion-notice-banner')).toHaveCount(0);
    await expect(page.getByTestId('save-status')).toHaveText('Old format: converted when you save');

    await page.keyboard.press('Control+s');
    await expect(page.getByTestId('save-status')).toHaveText('Saved');
    await expect(page.locator('[data-toast-key="legacy-converted"]')).toBeVisible();
  });

  test('a long list of renamed stations sits behind Details', async ({ page }) => {
    const details = Array.from({ length: 30 }, (_, i) => `Branch ${i + 1} / 0 is now shown as ${(i + 1) * 19}`);
    await page.evaluate(
      (d) =>
        (window as unknown as MockWindow).__MOCK_SET_CONVERSION_NOTICE__({
          changes: ['30 stations are now shown under another name'],
          details: d,
          file: 'not_yet_saved',
        }),
      details
    );
    await createProject(page, 'Big Old Format');

    await expect(page.getByTestId('conversion-notice-text')).toHaveText('30 stations are now shown under another name.');
    await expect(page.getByTestId('conversion-notice-details')).toHaveCount(0);
    await page.getByTestId('conversion-notice-details-button').click();
    await expect(page.getByTestId('conversion-notice-details').locator('li')).toHaveCount(30);
    await expect(page.getByTestId('conversion-notice-details')).toContainText('Branch 1 / 0 is now shown as 19');
  });

  test('the converting save with the banner dismissed confirms with a toast', async ({ page }) => {
    await page.evaluate(() =>
      (window as unknown as MockWindow).__MOCK_SET_CONVERSION_NOTICE__({
        changes: ['1 station moved by 0.40 m'],
        details: [],
        file: 'not_yet_saved',
      })
    );
    await page.evaluate(() => (window as unknown as MockWindow).__MOCK_SET_LEGACY_ORIGIN__(true));
    await createProject(page, 'Old Format Dismissed');
    await page.getByTestId('conversion-notice-dismiss-button').click();
    await expect(page.getByTestId('conversion-notice-banner')).toHaveCount(0);

    await page.keyboard.press('Control+s');
    const toast = page.locator('[data-toast-key="legacy-converted"]');
    await expect(toast).toBeVisible();
    await expect(toast).toContainText('Saved in the new format; the original is kept in Recent Projects');
    await toast.getByRole('button').click();
    await expect(toast).toHaveCount(0);
    // Converted once: the next save says nothing more.
    await makeDirty(page);
    await page.keyboard.press('Control+s');
    await expect.poll(() => isDirty(page)).toBe(false);
    await expect(toast).toHaveCount(0);
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
